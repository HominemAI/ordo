//! What the supervisor reads at boot (`plan-identity-service.md` 3.1).
//!
//! Its own settings are `ORDO_SUPERVISOR_*`. Everything else in the
//! container's environment belongs to a child and is split among them by
//! [`crate::children`].
//!
//! Upgrade: the console used to supervise the deployment and read
//! `ORDO_CONSOLE_CHILD_USER`, `ORDO_CONSOLE_CHILD_GRACE_SECS`,
//! `ORDO_CONSOLE_CHILD_READY_SECS` and `ORDO_CONSOLE_HOST_BIN`. A compose
//! file that sets one keeps it: each is read as the default of the
//! supervisor's own setting for the deployment, so an upgrade changes
//! nothing (owner, 2026-10-27).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::error::SupervisorError;

pub const IDENTITY_USER_ENV: &str = "ORDO_SUPERVISOR_IDENTITY_USER";
pub const CONSOLE_USER_ENV: &str = "ORDO_SUPERVISOR_CONSOLE_USER";
pub const DEPLOYMENT_USER_ENV: &str = "ORDO_SUPERVISOR_DEPLOYMENT_USER";
pub const IDENTITY_BIN_ENV: &str = "ORDO_SUPERVISOR_IDENTITY_BIN";
pub const CONSOLE_BIN_ENV: &str = "ORDO_SUPERVISOR_CONSOLE_BIN";
pub const HOST_BIN_ENV: &str = "ORDO_SUPERVISOR_HOST_BIN";
pub const STATE_DIR_ENV: &str = "ORDO_SUPERVISOR_STATE_DIR";
pub const KEY_FILE_ENV: &str = "ORDO_SUPERVISOR_KEY_FILE";
pub const RUN_DIR_ENV: &str = "ORDO_SUPERVISOR_RUN_DIR";
pub const SOCKET_ENV: &str = "ORDO_SUPERVISOR_SOCKET";
pub const LISTEN_ENV: &str = "ORDO_SUPERVISOR_LISTEN";
pub const GRACE_ENV: &str = "ORDO_SUPERVISOR_GRACE_SECS";
pub const READY_ENV: &str = "ORDO_SUPERVISOR_READY_SECS";
/// `auto` (the default): a child of another unix user is started under a
/// relay, so a stop needs no CAP_KILL ([`crate::child::stop`]); `always`:
/// every child is, the supervisor's own user too.
pub const RELAY_ENV: &str = "ORDO_SUPERVISOR_RELAY";

/// The console's old names, read as defaults (module docs).
pub const OLD_CHILD_USER_ENV: &str = "ORDO_CONSOLE_CHILD_USER";
pub const OLD_GRACE_ENV: &str = "ORDO_CONSOLE_CHILD_GRACE_SECS";
pub const OLD_READY_ENV: &str = "ORDO_CONSOLE_CHILD_READY_SECS";
pub const OLD_HOST_BIN_ENV: &str = "ORDO_CONSOLE_HOST_BIN";
/// The console's old run folder setting, read as the run folder's default.
pub const OLD_RUN_DIR_ENV: &str = "ORDO_CONSOLE_RUN_DIR";
/// The run folder the image before step 7 used: a compose file of that
/// time mounts its tmpfs here (`uid=65532`), and none at `/run/ordo`.
pub const OLD_DEFAULT_RUN_DIR: &str = "/run/ordo-console";

/// The variable the launcher reads the user to switch to from. Set only by
/// the supervisor, and removed before the child's program starts.
pub const LAUNCH_USER_ENV: &str = "ORDO_SUPERVISOR_LAUNCH_USER";

pub const DEFAULT_IDENTITY_BIN: &str = "/usr/local/bin/ordo-identity-service";
pub const DEFAULT_CONSOLE_BIN: &str = "/usr/local/bin/ordo-console";
pub const DEFAULT_HOST_BIN: &str = "/usr/local/bin/ordo-host";
pub const DEFAULT_STATE_DIR: &str = "/var/lib/ordo-supervisor";
pub const DEFAULT_RUN_DIR: &str = "/run/ordo";
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8079";
pub const DEFAULT_GRACE_SECS: u64 = 30;
pub const DEFAULT_READY_SECS: u64 = 120;

/// A unix user and group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChildUser {
    pub uid: u32,
    pub gid: u32,
}

/// The 3 children, in the order they start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChildName {
    Identity,
    Console,
    Deployment,
}

impl ChildName {
    pub const ALL: [ChildName; 3] = [
        ChildName::Identity,
        ChildName::Console,
        ChildName::Deployment,
    ];
    /// Shutdown order (3.1): the deployment first, the identity service
    /// last, so the door and the console never outlive the keys they check.
    pub const STOP_ORDER: [ChildName; 3] = [
        ChildName::Deployment,
        ChildName::Console,
        ChildName::Identity,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ChildName::Identity => "identity",
            ChildName::Console => "console",
            ChildName::Deployment => "deployment",
        }
    }

    pub fn parse(s: &str) -> Option<ChildName> {
        match s {
            "identity" => Some(ChildName::Identity),
            "console" => Some(ChildName::Console),
            "deployment" => Some(ChildName::Deployment),
            _ => None,
        }
    }

    /// The program's name, for log lines.
    pub fn program(self) -> &'static str {
        match self {
            ChildName::Identity => "ordo-identity-service",
            ChildName::Console => "ordo-console",
            ChildName::Deployment => "ordo-host",
        }
    }

    pub fn user_setting(self) -> &'static str {
        match self {
            ChildName::Identity => IDENTITY_USER_ENV,
            ChildName::Console => CONSOLE_USER_ENV,
            ChildName::Deployment => DEPLOYMENT_USER_ENV,
        }
    }

    pub fn bin_setting(self) -> &'static str {
        match self {
            ChildName::Identity => IDENTITY_BIN_ENV,
            ChildName::Console => CONSOLE_BIN_ENV,
            ChildName::Deployment => HOST_BIN_ENV,
        }
    }
}

/// In prose: "identity service", "console", "deployment".
impl std::fmt::Display for ChildName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ChildName::Identity => "identity service",
            other => other.as_str(),
        })
    }
}

/// One child's program and unix user.
#[derive(Clone, Debug)]
pub struct ChildSettings {
    pub bin: PathBuf,
    /// `None`: the supervisor's own user, said in the log.
    pub user: Option<ChildUser>,
}

/// Everything the supervisor reads.
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    pub identity: ChildSettings,
    pub console: ChildSettings,
    pub deployment: ChildSettings,
    pub state_dir: PathBuf,
    pub key_file: PathBuf,
    pub run_dir: PathBuf,
    /// Where the run folder may be, in order, each with what named it
    /// ([`SupervisorConfig::settle_run_dir`] picks the first usable one).
    pub run_dir_candidates: Vec<(PathBuf, &'static str)>,
    /// [`SOCKET_ENV`] was set: the socket stays there whatever folder is
    /// picked.
    pub socket_set: bool,
    pub socket: PathBuf,
    pub listen: SocketAddr,
    pub grace: Duration,
    pub ready_timeout: Duration,
    /// [`RELAY_ENV`] is `always`.
    pub relay_always: bool,
    /// Every variable the container started with, sorted. Split among the
    /// children by [`crate::children`].
    pub container_env: Vec<(String, String)>,
}

impl SupervisorConfig {
    pub fn child(&self, name: ChildName) -> &ChildSettings {
        match name {
            ChildName::Identity => &self.identity,
            ChildName::Console => &self.console,
            ChildName::Deployment => &self.deployment,
        }
    }

    /// From the process environment.
    pub fn from_env() -> Result<SupervisorConfig, SupervisorError> {
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        env.sort();
        SupervisorConfig::from_list(env)
    }

    /// From a list of variables, as the container would hold them.
    pub fn from_list(env: Vec<(String, String)>) -> Result<SupervisorConfig, SupervisorError> {
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let child =
            |name: ChildName, default_bin: &str, old_bin: Option<&str>, old_user: Option<&str>| {
                let bin = get(name.bin_setting())
                    .or_else(|| old_bin.and_then(get))
                    .filter(|v| !v.trim().is_empty())
                    .unwrap_or_else(|| default_bin.to_string());
                let user = match get(name.user_setting()) {
                    Some(v) => user_from(name.user_setting(), &v)?,
                    None => match old_user.and_then(get) {
                        Some(v) => user_from(OLD_CHILD_USER_ENV, &v)?,
                        None => None,
                    },
                };
                Ok::<ChildSettings, SupervisorError>(ChildSettings {
                    bin: PathBuf::from(bin),
                    user,
                })
            };
        let identity = child(ChildName::Identity, DEFAULT_IDENTITY_BIN, None, None)?;
        let console = child(ChildName::Console, DEFAULT_CONSOLE_BIN, None, None)?;
        let deployment = child(
            ChildName::Deployment,
            DEFAULT_HOST_BIN,
            Some(OLD_HOST_BIN_ENV),
            Some(OLD_CHILD_USER_ENV),
        )?;
        let users = [
            (ChildName::Identity, identity.user),
            (ChildName::Console, console.user),
            (ChildName::Deployment, deployment.user),
        ];
        for (i, (a, ua)) in users.iter().enumerate() {
            for (b, ub) in users.iter().skip(i + 1) {
                if let (Some(x), Some(y)) = (ua, ub) {
                    if x.uid == y.uid {
                        return Err(SupervisorError::setting(
                            b.user_setting(),
                            format!(
                                "is uid {}, the same as {} for the {a}",
                                y.uid,
                                a.user_setting()
                            ),
                            "Give each child a unix user of its own; the image uses 65531 \
                                 for the identity service, 65532 for the console and 65533 for \
                                 the deployment. Two children that share a user can read each \
                                 other's secrets",
                        ));
                    }
                }
            }
        }
        let state_dir = PathBuf::from(
            get(STATE_DIR_ENV)
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_STATE_DIR.into()),
        );
        let key_file = get(KEY_FILE_ENV)
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| state_dir.join("supervisor.key"));
        // The run folder: the setting, else the console's old setting,
        // else /run/ordo, or the old image's /run/ordo-console when only
        // that one is usable (an upgraded compose file mounts its tmpfs
        // there, and the image's own root may be read-only).
        let set = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        let run_dir_candidates: Vec<(PathBuf, &'static str)> =
            match (set(RUN_DIR_ENV), set(OLD_RUN_DIR_ENV)) {
                (Some(d), _) => vec![(PathBuf::from(d), RUN_DIR_ENV)],
                (None, Some(d)) => vec![(PathBuf::from(d), OLD_RUN_DIR_ENV)],
                (None, None) => vec![
                    (PathBuf::from(DEFAULT_RUN_DIR), "the default"),
                    (
                        PathBuf::from(OLD_DEFAULT_RUN_DIR),
                        "the old image's default",
                    ),
                ],
            };
        let run_dir = run_dir_candidates[0].0.clone();
        let socket_set = set(SOCKET_ENV).is_some();
        let socket = set(SOCKET_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| run_dir.join("supervisor.sock"));
        let listen_text = get(LISTEN_ENV).unwrap_or_else(|| DEFAULT_LISTEN.to_string());
        let listen = listen_text.trim().parse::<SocketAddr>().map_err(|e| {
            SupervisorError::setting_caused(
                LISTEN_ENV,
                format!("is `{listen_text}`, which is not an address and port"),
                format!("Set {LISTEN_ENV} to an address such as 127.0.0.1:8079, or unset it"),
                e,
            )
        })?;
        let grace = secs(&get, GRACE_ENV, OLD_GRACE_ENV, DEFAULT_GRACE_SECS, 1)?;
        let ready_timeout = secs(&get, READY_ENV, OLD_READY_ENV, DEFAULT_READY_SECS, 5)?;
        match get(crate::children::IDENTITY_ADDRESS)
            .as_deref()
            .map(str::trim)
        {
            None | Some("") | Some("console-path") | Some("own-port") => {}
            Some(other) => {
                return Err(SupervisorError::setting(
                    crate::children::IDENTITY_ADDRESS,
                    format!("is `{other}`, which is not `console-path` or `own-port`"),
                    format!(
                        "Set {} to `console-path` (the console passes the identity service \
                         through at /identity on its own origin) or `own-port` (its own \
                         origin, port 8082), or unset it for `console-path`",
                        crate::children::IDENTITY_ADDRESS
                    ),
                ))
            }
        }
        let relay_always = match get(RELAY_ENV).as_deref().map(str::trim) {
            None | Some("") | Some("auto") => false,
            Some("always") => true,
            Some(other) => {
                return Err(SupervisorError::setting(
                    RELAY_ENV,
                    format!("is `{other}`, which is not `auto` or `always`"),
                    format!("Set {RELAY_ENV} to `auto`, or unset it"),
                ))
            }
        };
        Ok(SupervisorConfig {
            identity,
            console,
            deployment,
            state_dir,
            key_file,
            run_dir,
            run_dir_candidates,
            socket_set,
            socket,
            listen,
            grace,
            ready_timeout,
            relay_always,
            container_env: env,
        })
    }
}

impl SupervisorConfig {
    /// Pick the run folder: the first candidate the supervisor can make
    /// its own and write ([`crate::child::usable_run_dir`]), and the socket
    /// in it unless [`SOCKET_ENV`] says otherwise. None usable refuses,
    /// naming [`RUN_DIR_ENV`] and why each was not.
    pub fn settle_run_dir(&mut self) -> Result<(), SupervisorError> {
        let mut why = Vec::new();
        for (dir, from) in &self.run_dir_candidates {
            match crate::child::usable_run_dir(dir) {
                Ok(()) => {
                    if self.run_dir_candidates.len() > 1
                        && dir.as_path() != std::path::Path::new(DEFAULT_RUN_DIR)
                    {
                        eprintln!(
                            "ordo-supervisor: the run folder is {} ({from}): {}",
                            dir.display(),
                            why.join("; ")
                        );
                    }
                    self.run_dir = dir.clone();
                    if !self.socket_set {
                        self.socket = dir.join("supervisor.sock");
                    }
                    return Ok(());
                }
                Err(e) => why.push(format!("{} ({from}) {e}", dir.display())),
            }
        }
        Err(SupervisorError::setting(
            RUN_DIR_ENV,
            format!("names no folder the supervisor can use: {}", why.join("; ")),
            format!(
                "Mount a tmpfs at {DEFAULT_RUN_DIR} owned by the supervisor's user, for example \
                 in compose `tmpfs: - {DEFAULT_RUN_DIR}:uid=65530,mode=0711`, or set {RUN_DIR_ENV} \
                 to a folder it can write"
            ),
        ))
    }
}

/// `uid:gid`, or empty for the supervisor's own user.
fn user_from(setting: &'static str, v: &str) -> Result<Option<ChildUser>, SupervisorError> {
    let v = v.trim();
    if v.is_empty() {
        return Ok(None);
    }
    let parsed = v
        .split_once(':')
        .and_then(|(u, g)| Some((u.parse::<u32>().ok()?, g.parse::<u32>().ok()?)));
    match parsed {
        Some((0, _)) | Some((_, 0)) => Err(SupervisorError::setting(
            setting,
            format!("is `{v}`, which names root"),
            "Set it to a uid:gid that is not 0, such as 65533:65533; a child never runs as root",
        )),
        Some((uid, gid)) => Ok(Some(ChildUser { uid, gid })),
        None => Err(SupervisorError::setting(
            setting,
            format!("is `{v}`, which is not uid:gid"),
            "Set it to a uid:gid such as 65533:65533, or to an empty value to run the child as \
             the supervisor's own user",
        )),
    }
}

fn secs(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
    old: &'static str,
    default: u64,
    min: u64,
) -> Result<Duration, SupervisorError> {
    let (setting, v) = match (get(name), get(old)) {
        (Some(v), _) => (name, v),
        (None, Some(v)) => (old, v),
        (None, None) => return Ok(Duration::from_secs(default)),
    };
    match v.trim().parse::<u64>() {
        Ok(n) if n >= min => Ok(Duration::from_secs(n)),
        _ => Err(SupervisorError::setting(
            setting,
            format!("is `{v}`, which is not a whole number of seconds of {min} or more"),
            format!("Set {setting} to {min} or more, or unset it for {default}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(list: &[(&str, &str)]) -> Result<SupervisorConfig, SupervisorError> {
        SupervisorConfig::from_list(
            list.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn the_defaults_are_the_images_paths() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.socket, PathBuf::from("/run/ordo/supervisor.sock"));
        assert_eq!(
            c.key_file,
            PathBuf::from("/var/lib/ordo-supervisor/supervisor.key")
        );
        assert_eq!(c.listen.to_string(), DEFAULT_LISTEN);
        assert_eq!(c.deployment.user, None);
    }

    #[test]
    fn the_consoles_old_settings_still_count_for_the_deployment() {
        let c = cfg(&[
            (OLD_CHILD_USER_ENV, "65533:65533"),
            (OLD_GRACE_ENV, "7"),
            (OLD_HOST_BIN_ENV, "/opt/ordo-host"),
        ])
        .unwrap();
        assert_eq!(
            c.deployment.user,
            Some(ChildUser {
                uid: 65533,
                gid: 65533
            })
        );
        assert_eq!(c.grace, Duration::from_secs(7));
        assert_eq!(c.deployment.bin, PathBuf::from("/opt/ordo-host"));
        let c = cfg(&[
            (OLD_CHILD_USER_ENV, "65533:65533"),
            (DEPLOYMENT_USER_ENV, "65534:65534"),
        ])
        .unwrap();
        assert_eq!(c.deployment.user.unwrap().uid, 65534);
    }

    /// Upgrade (step 7): a compose file from before the one image sets no
    /// run folder and mounts its tmpfs at /run/ordo-console, while the
    /// image's root is read-only. The console's old setting is the default;
    /// with neither set, the first usable of /run/ordo and the old folder
    /// is taken; none usable refuses, naming the setting and each reason.
    #[test]
    fn the_run_folder_honours_the_old_setting_and_falls_back_to_the_old_folder() {
        let c = cfg(&[(OLD_RUN_DIR_ENV, "/run/old")]).unwrap();
        assert_eq!(c.run_dir_candidates.len(), 1);
        assert_eq!(c.run_dir, PathBuf::from("/run/old"));
        assert_eq!(c.socket, PathBuf::from("/run/old/supervisor.sock"));
        let c = cfg(&[(OLD_RUN_DIR_ENV, "/run/old"), (RUN_DIR_ENV, "/run/new")]).unwrap();
        assert_eq!(c.run_dir, PathBuf::from("/run/new"));
        let c = cfg(&[]).unwrap();
        assert_eq!(
            c.run_dir_candidates
                .iter()
                .map(|(d, _)| d.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from(DEFAULT_RUN_DIR),
                PathBuf::from(OLD_DEFAULT_RUN_DIR)
            ]
        );

        let tmp = std::env::temp_dir().join(format!("ordo-sup-rundir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // A path under a plain file can never be a folder.
        std::fs::write(tmp.join("file"), "").unwrap();
        let mut c = cfg(&[]).unwrap();
        c.run_dir_candidates = vec![
            (tmp.join("file/run"), "the default"),
            (tmp.join("old"), "the old image's default"),
        ];
        c.settle_run_dir().unwrap();
        assert_eq!(c.run_dir, tmp.join("old"));
        assert_eq!(c.socket, tmp.join("old/supervisor.sock"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let m = std::fs::metadata(tmp.join("old")).unwrap();
            assert_eq!(m.permissions().mode() & 0o777, 0o711);
        }
        c.run_dir_candidates = vec![(tmp.join("file/a"), "the default")];
        let e = c.settle_run_dir().unwrap_err();
        assert_eq!(e.setting_name(), Some(RUN_DIR_ENV));
        assert!(e.to_string().contains("could not be made"), "{e}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn root_and_a_shared_user_are_refused_naming_the_setting() {
        let e = cfg(&[(CONSOLE_USER_ENV, "0:0")]).unwrap_err();
        assert_eq!(e.setting_name(), Some(CONSOLE_USER_ENV));
        let e = cfg(&[
            (CONSOLE_USER_ENV, "65532:65532"),
            (DEPLOYMENT_USER_ENV, "65532:65533"),
        ])
        .unwrap_err();
        assert_eq!(e.setting_name(), Some(DEPLOYMENT_USER_ENV));
        let e = cfg(&[(IDENTITY_USER_ENV, "bob")]).unwrap_err();
        assert_eq!(e.setting_name(), Some(IDENTITY_USER_ENV));
    }
}
