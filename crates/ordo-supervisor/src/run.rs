//! The loops that own the children (`plan-identity-service.md` 3.1).
//!
//! One task per child. It starts the child, polls `/health/ready`,
//! restarts with backoff, marks a crash loop, and carries out every
//! restart and, for the deployment, every apply, one at a time. A child
//! that dies, hangs or loops never stops the other 2.
//!
//! The rules, with their defaults ([`Timings::plan`]):
//! - restart after 1 s, then 2, 4, up to 60; the delay resets after 10
//!   minutes ready;
//! - 5 exits in 10 minutes is a crash loop: retry every 60 s;
//! - the deployment only: a crash loop within 10 minutes of an apply rolls
//!   back to the revision before it, once; a second one stops automatic
//!   changes (`held`);
//! - an apply runs `ordo-host check-config` first (G17), then stops the old
//!   process (SIGTERM, the grace period, SIGKILL), starts the new one and
//!   waits for ready; not ready, it starts the live revision again.
//!
//! Before the console's first start in each boot, the supervisor runs
//! `ordo-console forget-door-key` as the console's user (step 6 made a
//! console on the identity service refuse to boot while it holds a console
//! key). The command writes the console's own audit row, `unix:` the
//! supervisor, whether it forgot a key or found none. The owner does
//! nothing.

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::child::{self, Launch, LogRing};
use crate::children::{self, Split};
use crate::config::{ChildName, ChildUser, SupervisorConfig, RUN_DIR_ENV};
use crate::error::SupervisorError;
use crate::spec::{self, ApplyRequest, Revision, Spec, SpecStore};
use crate::state::{Applied, ChildState, ExitInfo, Phase, Privilege, Status, Timings};

/// Seconds since the epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The console's command that forgets a held console key.
pub const FORGET_ARG: &str = "forget-door-key";
/// How the forget step names who ran it in the console's audit row.
pub const FORGET_USER: &str = "ordo-supervisor";
const FORGET_TIMEOUT: Duration = Duration::from_secs(60);

enum Cmd {
    Apply {
        rev: Box<Revision>,
        reply: oneshot::Sender<Result<(), (String, u64)>>,
    },
    Restart {
        reply: oneshot::Sender<()>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// Where a child's settings come from. Three exist, one per child, so
/// the size of the larger variant costs nothing.
#[allow(clippy::large_enum_variant)]
enum Source {
    /// Built once at boot from the container's variables.
    Fixed { spec: Spec, probe: SocketAddr },
    /// The deployment: its live revision, on the supervisor's volume.
    Deployment {
        store: SpecStore,
        live: Mutex<Revision>,
        good: Mutex<Option<Revision>>,
        wiring: BTreeMap<String, String>,
    },
}

/// The supervisor's handle. Cheap to clone.
#[derive(Clone)]
pub struct Supervisor(Arc<Shared>);

struct Shared {
    children: BTreeMap<ChildName, Arc<Core>>,
    /// Ports the deployment may not take: the console's, the identity
    /// service's, the supervisor's own.
    taken: Vec<(u16, &'static str)>,
    apply_lock: tokio::sync::Mutex<()>,
}

struct Core {
    name: ChildName,
    launcher: PathBuf,
    bin: PathBuf,
    user: Option<ChildUser>,
    /// Start it under a relay, so a stop needs no CAP_KILL.
    relay: bool,
    dir: PathBuf,
    timings: Timings,
    source: Source,
    /// The console only: `forget-door-key` has run in this boot.
    forget_pending: Mutex<bool>,
    state: Mutex<Status>,
    logs: LogRing,
    tx: mpsc::Sender<Cmd>,
}

/// The running child, as the loop holds it.
struct Running {
    proc: Child,
    /// The relay's control pipe ([`child::stop`]); dropping it stops the
    /// child.
    ctl: Option<tokio::process::ChildStdin>,
    ready_since: Option<Instant>,
}

/// The loop's own state.
struct Ctx {
    child: Option<Running>,
    restart_at: Option<Instant>,
    poll_at: Option<Instant>,
    backoff: Duration,
    exits: VecDeque<Instant>,
    /// When the last apply came up, and the revision before it.
    last_apply: Option<(Instant, Revision)>,
    auto_rollback_spent: bool,
}

fn fmt_dur(d: Duration) -> String {
    if d.subsec_millis() == 0 {
        format!("{} s", d.as_secs())
    } else {
        format!("{} ms", d.as_millis())
    }
}

async fn sleep_opt(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn wait_exit(child: &mut Option<Running>) -> std::io::Result<std::process::ExitStatus> {
    match child {
        Some(r) => r.proc.wait().await,
        None => std::future::pending().await,
    }
}

fn port_of(spec: &Spec, setting: &str, default: &str) -> SocketAddr {
    spec::probe_addr(spec.settings.get(setting).map(String::as_str), default)
}

impl Supervisor {
    /// Make the run folder, split the settings, read or build the live
    /// spec, settle each child's unix user, and start the 3 loops.
    pub async fn start(
        cfg: &SupervisorConfig,
        launcher: PathBuf,
        timings: Timings,
    ) -> Result<Supervisor, SupervisorError> {
        child::run_dir(&cfg.run_dir).map_err(|e| {
            SupervisorError::setting_caused(
                RUN_DIR_ENV,
                format!(
                    "is `{}`, and the supervisor could not make it",
                    cfg.run_dir.display()
                ),
                format!(
                    "Mount a tmpfs there owned by the supervisor's user, for example --tmpfs \
                     {}:uid=65530,mode=0711, or set {RUN_DIR_ENV} to a folder it can write",
                    cfg.run_dir.display()
                ),
                e,
            )
        })?;
        let t = now();
        // A database URL file the supervisor's own user may not read, such
        // as a console-database-url the old image's setup made readable by
        // the console alone (65532, mode 0400): read once as each child's
        // user through the launcher, then handed to each child as a private
        // copy. Nothing on the host changes (owner: an upgrade affects
        // nothing).
        let mut handed: BTreeMap<PathBuf, String> = BTreeMap::new();
        for path in children::unreadable_files(&cfg.container_env, &|p| std::fs::read(p)) {
            for user in [cfg.console.user, cfg.identity.user].into_iter().flatten() {
                match child::read_as(&launcher, user, &path).await {
                    Ok(text) => {
                        eprintln!(
                            "ordo-supervisor: {} is not readable by the supervisor's user; read \
                             it as uid {} and gave each child a private copy",
                            path.display(),
                            user.uid
                        );
                        handed.insert(path.clone(), text);
                        break;
                    }
                    Err(e) => eprintln!(
                        "ordo-supervisor: {} could not be read as uid {} either: {e}",
                        path.display(),
                        user.uid
                    ),
                }
            }
        }
        let direct = |p: &Path| std::fs::read_to_string(p);
        let read = children::reader(&handed, &direct);
        let mut split: Split = children::split(&cfg.container_env, &read, t);
        // The console manages the deployment through the socket
        // (`plan-identity-service.md` 3.2): it is in supervise mode because
        // this is set.
        split.console.settings.insert(
            children::CONSOLE_SUPERVISOR_SOCKET.into(),
            cfg.socket.display().to_string(),
        );
        for line in split.describe_filled() {
            eprintln!("ordo-supervisor: {line}");
        }
        for line in &split.notes {
            eprintln!("ordo-supervisor: {line}");
        }

        let store = SpecStore::open(&cfg.state_dir, &cfg.key_file)?;
        let (live, good) = live_or_fallback(&store, &cfg.container_env, t);
        let live = match live {
            Some(r) => {
                let ignored: Vec<String> = r.spec.ignored_container_vars(
                    &cfg.container_env
                        .iter()
                        .filter(|(k, _)| spec::is_deployment_var(k))
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                if !ignored.is_empty() {
                    eprintln!(
                        "ordo-supervisor: the deployment runs revision {} from the supervisor's \
                         volume; these container variables differ and are ignored: {}",
                        r.number,
                        ignored.join(", ")
                    );
                }
                r
            }
            None => {
                let r = Revision::from_container(&cfg.container_env, t);
                store.set_live(&r)?;
                eprintln!(
                    "ordo-supervisor: no live spec on {}: built revision 1 from the container's \
                     ORDO_* variables",
                    cfg.state_dir.display()
                );
                r
            }
        };

        let console_addr = port_of(&split.console, "ORDO_CONSOLE_LISTEN", "127.0.0.1:8081");
        let identity_addr = port_of(&split.identity, children::IDENTITY_LISTEN, "127.0.0.1:8082");
        let taken = vec![
            (console_addr.port(), "the console"),
            (identity_addr.port(), "the identity service"),
            (cfg.listen.port(), "the supervisor"),
        ];

        let mut sources: BTreeMap<ChildName, Source> = BTreeMap::new();
        sources.insert(
            ChildName::Identity,
            Source::Fixed {
                spec: split.identity.clone(),
                probe: identity_addr,
            },
        );
        sources.insert(
            ChildName::Console,
            Source::Fixed {
                spec: split.console.clone(),
                probe: console_addr,
            },
        );
        sources.insert(
            ChildName::Deployment,
            Source::Deployment {
                store,
                live: Mutex::new(live.clone()),
                good: Mutex::new(good),
                wiring: split.wiring.clone(),
            },
        );

        let identity_on = split.identity_enabled;
        let mut children = BTreeMap::new();
        let mut loops = Vec::new();
        for name in ChildName::ALL {
            let settings = cfg.child(name);
            if !settings.bin.is_file() {
                return Err(SupervisorError::setting(
                    name.bin_setting(),
                    format!(
                        "is `{}`, and there is no file there",
                        settings.bin.display()
                    ),
                    format!(
                        "Set {} to the {} binary; the image has it in /usr/local/bin",
                        name.bin_setting(),
                        name.program()
                    ),
                ));
            }
            let dir = cfg.run_dir.join(name.as_str());
            // Not started at all: an upgraded deployment that names no
            // identity database (owner, 2026-10-30). Said, and shown as
            // stopped.
            let off = name == ChildName::Identity && !identity_on;
            let (user, privilege) = if off {
                (
                    None,
                    Privilege {
                        separate: false,
                        uid: None,
                        note: Some(children::identity_off_note()),
                    },
                )
            } else {
                settle_user(name, &cfg.run_dir, settings.user, &launcher).await?
            };
            eprintln!(
                "ordo-supervisor: {}: {}",
                name.as_str(),
                privilege.note.clone().unwrap_or_else(|| format!(
                    "runs as its own unix user, uid {}",
                    privilege.uid.unwrap_or_default()
                ))
            );
            let (tx, rx) = mpsc::channel(8);
            let core = Arc::new(Core {
                name,
                launcher: launcher.clone(),
                bin: settings.bin.clone(),
                user,
                relay: user.is_some() || cfg.relay_always,
                dir,
                timings: timings.clone(),
                source: sources.remove(&name).expect("one source per child"),
                // The old console key is forgotten only when the console
                // signs people in at the identity service; without it, the
                // console keeps its own sign-in and needs its key.
                forget_pending: Mutex::new(name == ChildName::Console && identity_on),
                state: Mutex::new(Status {
                    child: name.as_str().to_string(),
                    state: if off {
                        ChildState::Stopped
                    } else {
                        ChildState::Starting
                    },
                    phase: if off { Phase::Stopped } else { Phase::Starting },
                    pid: None,
                    revision: None,
                    live: if name == ChildName::Deployment {
                        live.number
                    } else {
                        0
                    },
                    restarts: 0,
                    last_exit: None,
                    ready_since: None,
                    held: false,
                    applying: None,
                    privilege,
                }),
                logs: Arc::new(Mutex::new(VecDeque::new())),
                tx,
            });
            if !off {
                loops.push((core.clone(), rx));
            }
            children.insert(name, core);
        }
        for (core, rx) in loops {
            tokio::spawn(core.run(rx));
        }
        Ok(Supervisor(Arc::new(Shared {
            children,
            taken,
            apply_lock: tokio::sync::Mutex::new(()),
        })))
    }

    fn core(&self, name: ChildName) -> &Arc<Core> {
        &self.0.children[&name]
    }

    pub fn status(&self, name: ChildName) -> Status {
        let mut s = self.core(name).with_state(|s| s.clone());
        s.state = ChildState::of(s.phase, s.held);
        s
    }

    /// The newest `tail` lines of a child's output, oldest first.
    pub fn logs(&self, name: ChildName, tail: usize) -> Vec<String> {
        let all: Vec<String> = self
            .core(name)
            .logs
            .lock()
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default();
        let skip = all.len().saturating_sub(tail);
        all.into_iter().skip(skip).collect()
    }

    /// The deployment's live revision.
    pub fn current(&self) -> Revision {
        self.core(ChildName::Deployment).live_revision()
    }

    /// Apply a revision the console resolved (`plan-identity-service.md`
    /// 3.2): check it, run `check-config` on it, then bring it up in place
    /// of the live one.
    pub async fn apply(&self, req: &ApplyRequest) -> Result<Applied, SupervisorError> {
        let _one = self.0.apply_lock.lock().await;
        let core = self.core(ChildName::Deployment);
        let old = core.live_revision();
        let new = req.resolve(&old, &self.0.taken, now())?;
        if new.spec == old.spec {
            return Err(SupervisorError::BadRequest(
                "that matches the live revision; there is nothing to apply (restart starts a \
                 new process with the same settings)"
                    .into(),
            ));
        }
        let diff = spec::diff_summary(&old.spec, &new.spec);
        // G17: the new settings, checked by a short separate process with
        // its secret files in a folder of their own. Nothing running is
        // touched.
        let check_dir = core.dir.with_file_name("deployment-check");
        let checked = match core.build_env(&new.spec, Some(&new), &check_dir) {
            Ok(env) => {
                let launch = Launch {
                    launcher: &core.launcher,
                    bin: &core.bin,
                    args: &["check-config"],
                    env: &env,
                    user: core.user,
                    relay: core.relay,
                };
                child::run_once(&launch, core.timings.check_timeout)
                    .await
                    .map(|_| ())
            }
            Err(e) => Err(e.to_string()),
        };
        let _ = std::fs::remove_dir_all(&check_dir);
        if let Err(output) = checked {
            core.note(format!("revision {} refused by check-config", new.number));
            return Err(SupervisorError::SettingsRefused {
                revision: new.number,
                output,
            });
        }
        let number = new.number;
        let (reply, answer) = oneshot::channel();
        core.tx
            .send(Cmd::Apply {
                rev: Box::new(new),
                reply,
            })
            .await
            .map_err(|_| SupervisorError::stopped())?;
        match answer.await {
            Ok(Ok(())) => Ok(Applied {
                revision: number,
                diff,
            }),
            Ok(Err((why, to))) => Err(SupervisorError::RolledBack {
                revision: number,
                to,
                why,
            }),
            Err(_) => Err(SupervisorError::stopped()),
        }
    }

    /// The same settings, a new process.
    pub async fn restart(&self, name: ChildName) -> Result<(), SupervisorError> {
        let (reply, done) = oneshot::channel();
        self.core(name)
            .tx
            .send(Cmd::Restart { reply })
            .await
            .map_err(|_| SupervisorError::stopped())?;
        done.await.map_err(|_| SupervisorError::stopped())
    }

    /// Stop every child in [`ChildName::STOP_ORDER`], each with its grace
    /// period, one after the other.
    pub async fn shutdown(&self) {
        for name in ChildName::STOP_ORDER {
            let core = self.core(name);
            eprintln!("ordo-supervisor: stopping the {name}");
            let (reply, done) = oneshot::channel();
            if core.tx.send(Cmd::Shutdown { reply }).await.is_ok() {
                let _ = done.await;
            }
            eprintln!("ordo-supervisor: the {name} has stopped");
        }
    }
}

/// The live and last good revisions from the supervisor's volume. A sealed
/// file that does not open never stops the supervisor (review, step 7):
/// it is moved aside, kept for the deployment owner, and the deployment
/// starts from the last good revision, or else from revision 1 built from
/// the container's variables. Each step is said loudly. The other 2
/// children start either way. `None` live means there was no file at all.
fn live_or_fallback(
    store: &SpecStore,
    container_env: &[(String, String)],
    t: u64,
) -> (Option<Revision>, Option<Revision>) {
    let loud = |line: String| eprintln!("ordo-supervisor: WARNING: {line}");
    let mut good = match store.good() {
        Ok(g) => g,
        Err(e) => {
            let kept = store.set_aside(spec::GOOD, t);
            loud(format!(
                "the last good revision could not be read ({e}); the file was moved to {kept}"
            ));
            None
        }
    };
    let live = match store.live() {
        Ok(l) => l,
        Err(e) => {
            let kept = store.set_aside(spec::LIVE, t);
            let (rev, from) = match good.take() {
                Some(g) => (g, "the last good revision"),
                None => (
                    Revision::from_container(container_env, t),
                    "revision 1, built from the container's ORDO_* variables",
                ),
            };
            loud(format!(
                "the deployment's live revision could not be read ({e}). The file was moved to \
                 {kept}, and the deployment starts from {from} (revision {}). Restore {} from \
                 the backup made with this volume and move the file back to read it again",
                rev.number,
                crate::config::KEY_FILE_ENV
            ));
            if let Err(e) = store.set_live(&rev) {
                loud(format!("revision {} could not be saved: {e}", rev.number));
            }
            Some(rev)
        }
    };
    (live, good)
}

/// The deployment's environment from its revision's: every name a
/// deployment setting may hold ([`spec::check_name`]), then the identity
/// wiring and the revision label on top. The wiring always overwrites: a
/// revision never chooses which identity service the door trusts. A
/// revision saved before that rule may still hold such a name, so the
/// names left out are returned for the log.
fn deployment_env(
    env: BTreeMap<String, String>,
    wiring: &BTreeMap<String, String>,
    rev: &Revision,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut dropped = Vec::new();
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in env {
        let stem = k.strip_suffix("_FILE").unwrap_or(&k);
        if spec::check_name(stem).is_ok() || spec::check_name(&k).is_ok() {
            out.insert(k, v);
        } else {
            dropped.push(k);
        }
    }
    for (k, v) in wiring {
        out.insert(k.clone(), v.clone());
    }
    out.insert(spec::REVISION_ENV.into(), rev.number.to_string());
    // G16: the deployment records who applied what started it.
    out.insert(spec::APPLIED_BY_ENV.into(), rev.actor.clone());
    (out, dropped)
}

/// Decide a child's unix user. Unset: the supervisor's own, said plainly.
/// Set: that user, or the supervisor refuses to start, naming the setting.
async fn settle_user(
    name: ChildName,
    run_dir: &Path,
    wanted: Option<ChildUser>,
    launcher: &Path,
) -> Result<(Option<ChildUser>, Privilege), SupervisorError> {
    let Some(user) = wanted else {
        return decide_user(name, None, Ok(()));
    };
    decide_user(name, Some(user), can_switch(run_dir, user, launcher).await)
}

/// Whether the supervisor may run a child as `user`: hand it a file, then
/// have the launcher switch to it and exit. `Err` says why not.
async fn can_switch(run_dir: &Path, user: ChildUser, launcher: &Path) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("switching user is built for Linux only".into());
    }
    let probe = run_dir.join(format!("probe-{}", user.uid));
    let handed = child::fresh_dir(&probe)
        .and_then(|()| child::write_secret(&probe.join("file"), b"probe", Some(user)));
    let _ = std::fs::remove_dir_all(&probe);
    handed.map_err(|e| format!("the supervisor may not hand that user a file: {e}"))?;
    child::probe_user(launcher, user)
        .await
        .map_err(|e| format!("the supervisor may not switch to it: {e}"))
}

/// The decision itself, apart from the probes. It fails closed: with the
/// setting given, a failed switch never falls back to the supervisor's own
/// unix user, which would let the child read its siblings' secrets.
fn decide_user(
    name: ChildName,
    wanted: Option<ChildUser>,
    switch: Result<(), String>,
) -> Result<(Option<ChildUser>, Privilege), SupervisorError> {
    let setting = name.user_setting();
    let Some(user) = wanted else {
        return Ok((
            None,
            Privilege {
                separate: false,
                uid: None,
                note: Some(format!(
                    "{setting} is unset, so the {name} runs as the supervisor's own unix user \
                     and can read what the supervisor holds. Set {setting} to uid:gid to run \
                     it as its own user"
                )),
            },
        ));
    };
    match switch {
        Ok(()) => Ok((
            Some(user),
            Privilege {
                separate: true,
                uid: Some(user.uid),
                note: None,
            },
        )),
        Err(why) => Err(SupervisorError::setting(
            setting,
            format!(
                "is {}:{}, and the supervisor cannot run the {name} as that user ({why}); \
                 nothing was started",
                user.uid, user.gid
            ),
            format!(
                "On Linux, grant the container CAP_SETUID, CAP_SETGID, CAP_SETPCAP and \
                 CAP_CHOWN (for example --cap-add SETUID --cap-add SETGID --cap-add SETPCAP \
                 --cap-add CHOWN, or in compose cap_add: [SETUID, SETGID, CHOWN, SETPCAP]) and \
                 keep the supervisor binary's file capabilities. Or set \
                 {setting} to an empty value to run the {name} as the supervisor's own unix \
                 user, which lets it read what the supervisor holds"
            ),
        )),
    }
}

impl Core {
    fn with_state<T>(&self, f: impl FnOnce(&mut Status) -> T) -> T {
        match self.state.lock() {
            Ok(mut s) => f(&mut s),
            Err(p) => f(&mut p.into_inner()),
        }
    }

    fn note(&self, line: String) {
        eprintln!("ordo-supervisor: [{}] {line}", self.name.as_str());
        child::push_log(&self.logs, format!("[supervisor] {line}"));
    }

    fn live_revision(&self) -> Revision {
        match &self.source {
            Source::Deployment { live, .. } => match live.lock() {
                Ok(l) => l.clone(),
                Err(p) => p.into_inner().clone(),
            },
            Source::Fixed { .. } => unreachable!("only the deployment has revisions"),
        }
    }

    /// The child's whole environment: its settings, `<NAME>_FILE` for each
    /// secret, and for the deployment the identity wiring and the revision
    /// label. Secret values are written to `dir`, mode 0600, the child's.
    fn build_env(
        &self,
        spec: &Spec,
        rev: Option<&Revision>,
        dir: &Path,
    ) -> Result<Vec<(String, String)>, SupervisorError> {
        let refuse = |e: std::io::Error| {
            SupervisorError::setting_caused(
                RUN_DIR_ENV,
                format!(
                    "holds `{}`, where the {}'s secret files could not be written",
                    dir.display(),
                    self.name
                ),
                format!("Set {RUN_DIR_ENV} to a folder the supervisor can write, ideally a tmpfs"),
                e,
            )
        };
        let env = children::write_env(spec, dir, self.user).map_err(refuse)?;
        let env = match (&self.source, rev) {
            (Source::Deployment { wiring, .. }, Some(rev)) => {
                let (env, dropped) = deployment_env(env, wiring, rev);
                if !dropped.is_empty() {
                    self.note(format!(
                        "revision {} names {}, which are not deployment settings; they were \
                         left out",
                        rev.number,
                        dropped.join(", ")
                    ));
                }
                env
            }
            _ => env,
        };
        Ok(env.into_iter().collect())
    }

    fn probe_addr(&self, rev: Option<&Revision>) -> SocketAddr {
        match (&self.source, rev) {
            (Source::Fixed { probe, .. }, _) => *probe,
            (Source::Deployment { .. }, Some(r)) => r.spec.host_probe(),
            (Source::Deployment { .. }, None) => self.live_revision().spec.host_probe(),
        }
    }

    /// The console only, once per boot: forget a held console key before
    /// it starts. A failure is said and tried again before the next start.
    async fn forget_door_key(&self, spec: &Spec) {
        let pending = self.forget_pending.lock().map(|p| *p).unwrap_or(false);
        if !pending {
            return;
        }
        let dir = self.dir.with_file_name("console-forget");
        let env = match self.build_env(spec, None, &dir) {
            Ok(mut env) => {
                env.push(("USER".into(), FORGET_USER.into()));
                env
            }
            Err(e) => {
                self.note(format!("{FORGET_ARG} could not be prepared: {e}"));
                return;
            }
        };
        let launch = Launch {
            launcher: &self.launcher,
            bin: &self.bin,
            args: &[FORGET_ARG],
            env: &env,
            user: self.user,
            relay: self.relay,
        };
        let result = child::run_once(&launch, FORGET_TIMEOUT).await;
        let _ = std::fs::remove_dir_all(&dir);
        match result {
            Ok(out) => {
                self.note(format!("{FORGET_ARG}: {}", out.replace('\n', " ")));
                if let Ok(mut p) = self.forget_pending.lock() {
                    *p = false;
                }
            }
            Err(e) => self.note(format!(
                "{FORGET_ARG} failed, and runs again before the next start: {}",
                e.replace('\n', " ")
            )),
        }
    }

    /// Start one process. `rev` is the deployment's revision to start.
    /// `Err` says why it could not start.
    async fn spawn(&self, ctx: &mut Ctx, rev: Option<&Revision>) -> Result<(), String> {
        let spec = match (&self.source, rev) {
            (Source::Fixed { spec, .. }, _) => spec.clone(),
            (Source::Deployment { .. }, Some(r)) => r.spec.clone(),
            (Source::Deployment { .. }, None) => return Err("no revision to start".into()),
        };
        if self.name == ChildName::Console {
            self.forget_door_key(&spec).await;
        }
        let env = self
            .build_env(&spec, rev, &self.dir)
            .map_err(|e| e.to_string())?;
        let launch = Launch {
            launcher: &self.launcher,
            bin: &self.bin,
            args: &[],
            env: &env,
            user: self.user,
            relay: self.relay,
        };
        let started =
            child::spawn(&launch, &self.logs, self.name.program()).map_err(|e| e.to_string())?;
        let pid = started.proc.id();
        let revision = rev.map(|r| r.number);
        ctx.child = Some(Running {
            proc: started.proc,
            ctl: started.ctl,
            ready_since: None,
        });
        self.with_state(|s| {
            s.phase = Phase::Starting;
            s.pid = pid;
            s.revision = revision;
            s.ready_since = None;
        });
        Ok(())
    }

    fn mark_ready(&self, ctx: &mut Ctx) {
        if let Some(r) = ctx.child.as_mut() {
            r.ready_since = Some(Instant::now());
        }
        let t = now();
        self.with_state(|s| {
            s.phase = Phase::Running;
            s.ready_since = Some(t);
        });
    }

    /// Start the live settings; a failure to start counts as an exit.
    async fn start_live(&self, ctx: &mut Ctx) {
        ctx.restart_at = None;
        let rev = match self.source {
            Source::Deployment { .. } => Some(self.live_revision()),
            Source::Fixed { .. } => None,
        };
        match self.spawn(ctx, rev.as_ref()).await {
            Ok(()) => ctx.poll_at = Some(Instant::now() + self.timings.ready_poll),
            Err(e) => {
                self.note(format!("could not start: {e}"));
                self.exited(ctx, ExitInfo::failed(e, now())).await;
            }
        }
    }

    async fn poll(&self, ctx: &mut Ctx) {
        ctx.poll_at = None;
        if ctx.child.is_none() {
            return;
        }
        if child::ready(self.probe_addr(None)).await {
            self.mark_ready(ctx);
        } else {
            ctx.poll_at = Some(Instant::now() + self.timings.ready_poll);
        }
    }

    /// Put `rev` in force as the deployment's live revision, on disk and in
    /// memory, keeping `prior` as the last good one.
    fn put_live(&self, rev: &Revision, prior: Option<&Revision>) -> Result<(), SupervisorError> {
        let Source::Deployment {
            store, live, good, ..
        } = &self.source
        else {
            return Ok(());
        };
        if let Some(p) = prior {
            store.set_good(p)?;
            if let Ok(mut g) = good.lock() {
                *g = Some(p.clone());
            }
        }
        store.set_live(rev)?;
        if let Ok(mut l) = live.lock() {
            *l = rev.clone();
        }
        self.with_state(|s| s.live = rev.number);
        Ok(())
    }

    /// The child ended without being asked to: count it, then restart after
    /// the backoff, or mark a crash loop.
    async fn exited(&self, ctx: &mut Ctx, info: ExitInfo) {
        let t = &self.timings;
        let was_ready = ctx.child.take().and_then(|r| r.ready_since);
        ctx.poll_at = None;
        let at = Instant::now();
        if was_ready.is_some_and(|r| at.duration_since(r) >= t.backoff_reset) {
            ctx.backoff = t.backoff_first;
        }
        // One change, so a reader never sees a child ready with no process.
        self.with_state(|s| {
            s.phase = Phase::Restarting;
            s.pid = None;
            s.revision = None;
            s.ready_since = None;
            s.restarts += 1;
            s.last_exit = Some(info.clone());
        });
        ctx.exits.push_back(at);
        while ctx
            .exits
            .front()
            .is_some_and(|e| at.duration_since(*e) > t.loop_window)
        {
            ctx.exits.pop_front();
        }
        if ctx.exits.len() < t.loop_exits {
            self.note(format!(
                "{}; starting it again in {}",
                info.describe(),
                fmt_dur(ctx.backoff)
            ));
            self.with_state(|s| s.phase = Phase::Restarting);
            ctx.restart_at = Some(at + ctx.backoff);
            ctx.backoff = (ctx.backoff * 2).min(t.backoff_max);
            return;
        }
        if let Source::Deployment { .. } = self.source {
            let live = self.live_revision();
            let after_apply = ctx
                .last_apply
                .clone()
                .filter(|(when, _)| at.duration_since(*when) <= t.loop_window);
            if let Some((_, prior)) = after_apply {
                if !ctx.auto_rollback_spent && prior.number != live.number {
                    ctx.auto_rollback_spent = true;
                    let why = format!(
                        "{} exits within {} of its apply; rolled back to revision {}",
                        ctx.exits.len(),
                        fmt_dur(t.loop_window),
                        prior.number
                    );
                    if let Err(e) = self.put_live(&prior, None) {
                        self.note(format!("the rollback could not be saved: {e}"));
                    }
                    self.with_state(|s| s.phase = Phase::Restarting);
                    self.note(format!("revision {} is crash-looping: {why}", live.number));
                    ctx.exits.clear();
                    ctx.backoff = t.backoff_first;
                    ctx.restart_at = Some(at);
                    return;
                }
                if ctx.auto_rollback_spent && !self.with_state(|s| s.held) {
                    self.with_state(|s| s.held = true);
                    self.note(format!(
                        "revision {} is crash-looping after an automatic rollback; no more \
                         automatic changes until the owner applies or restarts",
                        live.number
                    ));
                }
            }
        }
        self.with_state(|s| s.phase = Phase::CrashLooping);
        self.note(format!(
            "crash-looping ({} exits in {}; last {}); trying again every {}",
            ctx.exits.len(),
            fmt_dur(t.loop_window),
            info.describe(),
            fmt_dur(t.loop_retry)
        ));
        ctx.restart_at = Some(at + t.loop_retry);
    }

    /// Stop the running child, if any.
    async fn stop_child(&self, ctx: &mut Ctx) {
        ctx.poll_at = None;
        if let Some(mut r) = ctx.child.take() {
            child::stop(&mut r.proc, &mut r.ctl, self.timings.grace).await;
        }
        self.with_state(|s| {
            s.phase = Phase::Restarting;
            s.pid = None;
            s.revision = None;
            s.ready_since = None;
        });
    }

    /// Start a revision and wait until it is ready, exits, or the ready
    /// limit passes. On `Err` a child that is still starting is left
    /// running.
    async fn bring_up(&self, ctx: &mut Ctx, rev: &Revision) -> Result<(), String> {
        self.spawn(ctx, Some(rev)).await?;
        let addr = self.probe_addr(Some(rev));
        let deadline = Instant::now() + self.timings.ready_timeout;
        loop {
            let slept = tokio::select! {
                status = wait_exit(&mut ctx.child) => Err(status),
                _ = tokio::time::sleep(self.timings.ready_poll) => Ok(()),
            };
            match slept {
                Err(status) => {
                    let info = match status {
                        Ok(s) => ExitInfo::from_status(&s, now()),
                        Err(e) => ExitInfo::failed(e.to_string(), now()),
                    };
                    ctx.child = None;
                    let why = info.describe();
                    self.with_state(|s| {
                        s.pid = None;
                        s.revision = None;
                        s.last_exit = Some(info);
                    });
                    return Err(why);
                }
                Ok(()) => {
                    if child::ready(addr).await {
                        self.mark_ready(ctx);
                        return Ok(());
                    }
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "not ready within {}",
                            fmt_dur(self.timings.ready_timeout)
                        ));
                    }
                }
            }
        }
    }

    /// Stop, start, wait, and on failure put the live revision back. `Err`
    /// holds why and the revision restored.
    async fn apply_now(&self, ctx: &mut Ctx, rev: Revision) -> Result<(), (String, u64)> {
        let prior = self.live_revision();
        self.with_state(|s| s.applying = Some(rev.number));
        self.note(format!(
            "applying revision {} (live is {})",
            rev.number, prior.number
        ));
        self.stop_child(ctx).await;
        ctx.restart_at = None;
        ctx.exits.clear();
        ctx.backoff = self.timings.backoff_first;
        let result = match self.bring_up(ctx, &rev).await {
            Ok(()) => match self.put_live(&rev, Some(&prior)) {
                Ok(()) => {
                    self.with_state(|s| s.held = false);
                    ctx.last_apply = Some((Instant::now(), prior.clone()));
                    ctx.auto_rollback_spent = false;
                    self.note(format!("revision {} is ready and live", rev.number));
                    Ok(())
                }
                Err(e) => {
                    let why = format!("it came up, and could not be saved ({e})");
                    self.note(format!("revision {} {why}", rev.number));
                    self.stop_child(ctx).await;
                    self.restore(ctx, &prior).await;
                    Err((why, prior.number))
                }
            },
            Err(why) => {
                self.stop_child(ctx).await;
                self.note(format!(
                    "revision {} {why}; rolled back to revision {}",
                    rev.number, prior.number
                ));
                self.restore(ctx, &prior).await;
                Err((why, prior.number))
            }
        };
        self.with_state(|s| s.applying = None);
        result
    }

    async fn restore(&self, ctx: &mut Ctx, prior: &Revision) {
        match self.bring_up(ctx, prior).await {
            Ok(()) => {}
            Err(e) if ctx.child.is_some() => {
                self.note(format!("revision {}, rolled back to, {e}", prior.number));
                ctx.poll_at = Some(Instant::now() + self.timings.ready_poll);
            }
            Err(e) => {
                self.note(format!("revision {}, rolled back to, {e}", prior.number));
                self.with_state(|s| s.phase = Phase::Restarting);
                ctx.restart_at = Some(Instant::now() + ctx.backoff);
            }
        }
    }

    async fn run(self: Arc<Self>, mut rx: mpsc::Receiver<Cmd>) {
        enum Ev {
            Cmd(Option<Cmd>),
            Exit(std::io::Result<std::process::ExitStatus>),
            Start,
            Poll,
        }
        let mut ctx = Ctx {
            child: None,
            restart_at: Some(Instant::now()),
            poll_at: None,
            backoff: self.timings.backoff_first,
            exits: VecDeque::new(),
            last_apply: None,
            auto_rollback_spent: false,
        };
        loop {
            let ev = tokio::select! {
                biased;
                c = rx.recv() => Ev::Cmd(c),
                s = wait_exit(&mut ctx.child) => Ev::Exit(s),
                _ = sleep_opt(ctx.restart_at) => Ev::Start,
                _ = sleep_opt(ctx.poll_at) => Ev::Poll,
            };
            match ev {
                Ev::Cmd(None) => {
                    self.stop_child(&mut ctx).await;
                    self.with_state(|s| s.phase = Phase::Stopped);
                    return;
                }
                Ev::Cmd(Some(Cmd::Shutdown { reply })) => {
                    ctx.restart_at = None;
                    self.stop_child(&mut ctx).await;
                    self.with_state(|s| s.phase = Phase::Stopped);
                    let _ = reply.send(());
                    return;
                }
                Ev::Cmd(Some(Cmd::Apply { rev, reply })) => {
                    let r = self.apply_now(&mut ctx, *rev).await;
                    let _ = reply.send(r);
                }
                Ev::Cmd(Some(Cmd::Restart { reply })) => {
                    // The owner's restart answers a hold, as an apply does:
                    // the exit count and the apply window start again.
                    self.stop_child(&mut ctx).await;
                    ctx.backoff = self.timings.backoff_first;
                    ctx.exits.clear();
                    ctx.last_apply = None;
                    self.with_state(|s| s.held = false);
                    self.start_live(&mut ctx).await;
                    let _ = reply.send(());
                }
                Ev::Exit(status) => {
                    let info = match status {
                        Ok(s) => ExitInfo::from_status(&s, now()),
                        Err(e) => ExitInfo::failed(e.to_string(), now()),
                    };
                    self.exited(&mut ctx, info).await;
                }
                Ev::Start => self.start_live(&mut ctx).await,
                Ev::Poll => self.poll(&mut ctx).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHILD: ChildUser = ChildUser {
        uid: 65533,
        gid: 65533,
    };

    /// Images run 40172394128: the container lacked CAP_SETPCAP, the
    /// launcher's probe failed, and the console ran ordo-host as its own
    /// uid. A failed switch with the setting given must refuse, naming it.
    #[test]
    fn a_failed_switch_refuses_and_never_falls_back_to_the_supervisors_user() {
        let refused = decide_user(
            ChildName::Deployment,
            Some(CHILD),
            Err("switching user needs CAP_SETPCAP as well".into()),
        );
        let e = match refused {
            Ok((user, p)) => panic!("started as {user:?}, separate {}", p.separate),
            Err(e) => e,
        };
        assert_eq!(e.setting_name(), Some(crate::config::DEPLOYMENT_USER_ENV));
        let text = e.to_string();
        assert!(text.contains("CAP_SETPCAP"), "{text}");
        assert!(text.contains("nothing was started"), "{text}");
    }

    /// Review, step 7: a sealed file that does not open falls back to the
    /// last good revision, then to revision 1, and is kept aside.
    #[test]
    fn a_sealed_spec_that_does_not_open_falls_back_and_is_kept() {
        let dir = std::env::temp_dir().join(format!("ordo-sup-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SpecStore::open(&dir.join("state"), &dir.join("k")).unwrap();
        let env = vec![("ORDO_PG_HOST".to_string(), "db".to_string())];
        let one = Revision::from_container(&env, 1);
        let mut two = one.clone();
        two.number = 2;
        two.spec.settings.insert("ORDO_WORKERS".into(), "4".into());
        store.set_good(&one).unwrap();
        store.set_live(&two).unwrap();
        let (live, good) = live_or_fallback(&store, &env, 5);
        assert_eq!((live.unwrap().number, good.unwrap().number), (2, 1));

        // The live file is damaged: the last good one runs.
        std::fs::write(dir.join("state/deployment-live.sealed"), "not sealed").unwrap();
        let (live, good) = live_or_fallback(&store, &env, 6);
        assert_eq!(live.unwrap().number, 1);
        assert!(good.is_none());
        assert!(dir
            .join("state/deployment-live.sealed.unopened-6")
            .is_file());

        // Both damaged: revision 1 from the container's variables.
        std::fs::write(dir.join("state/deployment-live.sealed"), "x").unwrap();
        std::fs::write(dir.join("state/deployment-good.sealed"), "y").unwrap();
        let (live, good) = live_or_fallback(&store, &env, 7);
        let live = live.unwrap();
        assert_eq!((live.number, live.origin), (1, spec::Origin::Container));
        assert!(good.is_none());
        assert!(dir
            .join("state/deployment-good.sealed.unopened-7")
            .is_file());
        // And it is saved, so the next boot reads it.
        assert_eq!(store.live().unwrap().unwrap().number, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review, step 7: a revision that names the identity wiring or the
    /// dynamic loader never reaches the deployment's environment; the
    /// supervisor's wiring always wins.
    #[test]
    fn the_wiring_overwrites_and_foreign_names_are_left_out() {
        let rev = Revision::from_container(&[("ORDO_PG_HOST".into(), "db".into())], 1);
        let env: BTreeMap<String, String> = [
            ("ORDO_PG_HOST", "db"),
            (
                "ORDO_PG_PASSWORD_FILE",
                "/run/ordo/deployment/ORDO_PG_PASSWORD",
            ),
            ("ORDO_IDENTITY_URL", "http://evil.example"),
            ("ORDO_IDENTITY_PLAIN_HTTP", "true"),
            ("LD_PRELOAD", "/tmp/x.so"),
            (spec::REVISION_ENV, "99"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let wiring: BTreeMap<String, String> = [(
            "ORDO_IDENTITY_URL".to_string(),
            "http://127.0.0.1:8082".to_string(),
        )]
        .into();
        let (out, dropped) = deployment_env(env, &wiring, &rev);
        assert_eq!(out["ORDO_IDENTITY_URL"], "http://127.0.0.1:8082");
        assert!(!out.contains_key("ORDO_IDENTITY_PLAIN_HTTP"));
        assert!(!out.contains_key("LD_PRELOAD"));
        assert_eq!(out[spec::REVISION_ENV], "1");
        assert_eq!(out["ORDO_PG_HOST"], "db");
        assert!(out.contains_key("ORDO_PG_PASSWORD_FILE"));
        assert_eq!(
            dropped,
            vec![
                "LD_PRELOAD".to_string(),
                "ORDO_IDENTITY_PLAIN_HTTP".to_string(),
                "ORDO_IDENTITY_URL".to_string(),
                spec::REVISION_ENV.to_string()
            ]
        );
    }

    #[test]
    fn an_unset_user_runs_as_the_supervisor_and_says_so() {
        let (user, p) = decide_user(ChildName::Console, None, Ok(())).unwrap();
        assert_eq!(user, None);
        assert!(!p.separate);
        assert!(p.note.unwrap().contains(crate::config::CONSOLE_USER_ENV));
        let (user, p) = decide_user(ChildName::Identity, Some(CHILD), Ok(())).unwrap();
        assert_eq!(user, Some(CHILD));
        assert_eq!(p.uid, Some(65533));
    }
}
