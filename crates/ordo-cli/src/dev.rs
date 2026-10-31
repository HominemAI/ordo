//! `ordo dev` — the sub-second hot loop. `docs/plan-dev-loop.md`.
//!
//! D1 built the measurement before anything expensive was added to it:
//! watch the `.ordo` set, re-read, compile, print a spanned diagnostic or
//! the phase journal. D2 gave the loop its own database and a schema delta
//! on every save. **D3 is the composition nobody had performed**: bind a
//! socket to the same door a server binds, run the same gate, swap through
//! the same `Deployment`, over `ordo_host::runtime::assemble_embedded` — a
//! fixture that did not come from the artifact registry, gated exactly the
//! way one that did would be. Nothing here stands up a second serving
//! path; every property a server has, `ordo dev` gets by reusing the
//! function that builds one.
//!
//! [`DevTiming`] grows with the loop rather than being rewritten by it —
//! D3 adds `delta`/`reset` (already there, from D2), `gate` and `swap`.
//!
//! # The measurement is the point
//!
//! §3.2's rules, enforced here rather than merely followed:
//!
//! - **The phase numbers must sum to the total** — [`DevTiming::checked_total`]
//!   is how a caller (a test, in particular) gets that sum, and it panics on
//!   drift the way an assertion should rather than silently reporting a
//!   total that does not match its own parts.
//! - **Every number carries its denominator** — [`Phase`] carries an
//!   optional note string for exactly that (`"76 reads"`, `"0 changes"`);
//!   D1 has no denominators yet because compile has none to report.
//! - **The journal prints on every save, pass or fail.**

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ordo_exec::Pool;

use crate::{dev_db, load_sources_diag, LoadDiagnostic, Sources};

/// One named phase of a save, with the budget's own denominator when it has
/// one (`gate 297 (76 reads)` rather than `gate 297`).
#[derive(Debug, Clone)]
pub struct Phase {
    pub label: &'static str,
    pub elapsed: Duration,
    pub note: Option<String>,
}

/// What one save cost, phase by phase, and what it printed.
///
/// Built fresh per save. Increments after D1 add fields and phases; nothing
/// here is D1-specific except which phases are ever populated.
#[derive(Debug, Clone)]
pub struct DevTiming {
    /// The file whose change triggered this save, or the watched root on
    /// the loop's first, startup compile.
    pub file: String,
    pub ok: bool,
    pub total: Duration,
    pub phases: Vec<Phase>,
}

impl DevTiming {
    /// §3.2: "the phase numbers must sum to the total", asserted rather
    /// than merely printed. Returns the summed duration so a caller (the
    /// budget test) can compare it against both the phases and
    /// [`DevTiming::total`] independently.
    ///
    /// A few microseconds of slack are allowed for the bookkeeping between
    /// phases (an `Instant::now()` call, a `Vec::push`) that is real but not
    /// worth its own named phase; anything larger is exactly the
    /// unaccounted time §3.2 says a budget test exists to find.
    pub fn checked_total(&self) -> Duration {
        let summed: Duration = self.phases.iter().map(|p| p.elapsed).sum();
        let slack =
            Duration::from_millis(2) + self.phases.len() as u32 * Duration::from_micros(200);
        let (hi, lo) = (summed.max(self.total), summed.min(self.total));
        assert!(
            hi - lo <= slack,
            "dev journal phases summed to {summed:?} but the reported total was {:?} \
             (file {}) — the journal must not be able to lie by omission",
            self.total,
            self.file,
        );
        summed
    }

    /// The two-line form §3.2 specifies, e.g.:
    ///
    /// ```text
    /// saved  domain/market.ordo                                        ✓ 412ms
    ///        watch 3 · read 2 · compile 31
    /// ```
    pub fn journal(&self) -> String {
        let mark = if self.ok { "\u{2713}" } else { "\u{2717}" };
        let mut out = format!(
            "saved  {:<58} {mark} {}ms\n       ",
            self.file,
            self.total.as_millis()
        );
        let parts: Vec<String> = self
            .phases
            .iter()
            .map(|p| match &p.note {
                Some(n) => format!("{} {} ({n})", p.label, p.elapsed.as_millis()),
                None => format!("{} {}", p.label, p.elapsed.as_millis()),
            })
            .collect();
        out.push_str(&parts.join(" \u{b7} "));
        out
    }
}

/// The source line and a caret under the offending span — the frame D1
/// adds over [`Sources::at`], which already resolves `file:line:col`.
///
/// Byte spans are clipped to the line's own length so a span that runs to
/// end-of-file does not walk off a short last line.
pub fn frame(src: &Sources, span: ordo_syntax::Span) -> String {
    let line_no = span.line as usize;
    let line_text = src
        .text()
        .lines()
        .nth(line_no.saturating_sub(1))
        .unwrap_or("");
    let col = (span.col as usize).max(1);
    let width = (span.end.saturating_sub(span.start)).max(1);
    let avail = line_text.chars().count().saturating_sub(col - 1).max(1);
    let caret_len = width.min(avail);
    format!(
        "  {line_text}\n  {}{}",
        " ".repeat(col - 1),
        "^".repeat(caret_len)
    )
}

/// Render a diagnostic the way D1's done-when asks for: a spanned frame,
/// then the message the diagnostic carries.
///
/// `rule:`/`fix:` is `adoption-dx-plan.md` §3.3's committed shape, and this
/// prints toward it as far as today's error types can honestly go: they
/// carry a `code` and a `message`, not yet a message split into a rule
/// clause and a fix clause. Printing an invented split would be less
/// honest than printing what the compiler actually knows. Closing that gap
/// is §3.3's own work, across every error site in the compiler, not this
/// loop's.
pub fn render_diagnostic(src: &Sources, diag: &LoadDiagnostic) -> String {
    let mut out = String::new();
    let header = match diag.code() {
        Some(code) => format!("error[{code}]"),
        None => "error".to_string(),
    };
    match diag.span() {
        Some(span) => {
            out.push_str(&format!("{header} at {}\n", src.at(span)));
            out.push_str(&frame(src, span));
            out.push('\n');
        }
        None => out.push_str(&format!("{header}\n")),
    }
    out.push_str("  ");
    out.push_str(diag.message());
    out
}

/// A version label for [`ordo_host::deploy::Verified`] that is a pure
/// function of the compiled source: identical source gets the identical
/// label, so an unrelated re-save with unchanged content correctly reports
/// [`ordo_host::deploy::Swapped::AlreadyServing`] rather than swapping in an
/// identical `Loaded` under a fresh name every time (which would defeat
/// `Deployment::swap`'s own "nothing changed" check).
fn dev_version(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    format!(
        "dev-{}",
        digest
            .iter()
            .take(6)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// What one compile produced: the journal so far, the rendered diagnostic
/// when it failed, and — when it did not — the [`Schema`] D2's delta needs,
/// the [`ordo_conformance::Fixture`] D3's gate needs, the content-derived
/// version D3's swap needs, and the interface descriptor D4's `--out`
/// needs. A compile-only D1 caller ignores all four.
pub struct CompileOutcome {
    pub timing: DevTiming,
    pub diagnostic: Option<String>,
    pub schema: Option<ordo_schema::Schema>,
    pub fixture: Option<ordo_conformance::Fixture>,
    pub version: Option<String>,
    /// `interface.json`'s bytes — `ordo_cli::interface_json`, byte-stable
    /// and already digest-gated internally (`ordo_plan::interface::interface_document`
    /// sets its own `digest` field over the document minus that field).
    /// D4 uses this bytes-equality as its own change signal rather than
    /// re-parsing the digest back out: identical bytes mean an identical
    /// digest by construction, and different bytes mean a different one.
    pub interface: Option<String>,
}

/// Read and compile once — phases `read` and `compile`. No `watch` phase:
/// the caller that knows how long the fs event sat debounced adds it.
/// `descriptor`: D4's `--out`, opt-in — most saves have no reason to pay a
/// third compile pass for a descriptor nobody asked for.
///
/// `compile` does the pipeline at least twice on success: once through
/// [`load_sources_diag`] for the structured, spanned diagnostic D1 renders,
/// and once through [`ordo_conformance::Fixture::from_source`] for the
/// compiled unit D3's gate needs (its `seed` field is private to
/// `ordo-conformance`, so a `Fixture` cannot be assembled from the parts
/// `load_sources_diag` already has), and — with `descriptor: true` — a
/// third through [`crate::interface_json`]. Compile is "~30ms of pure CPU"
/// (`plan-dev-loop.md` §1) even once; three passes is still nowhere near
/// the budget, and one canonical pipeline per concern is worth more than a
/// shared one three callers would have to agree not to break.
pub fn compile_once(root: &Path, file: &str, descriptor: bool) -> CompileOutcome {
    let mut phases = Vec::new();

    let t = Instant::now();
    let src = Sources::read(root);
    let read_elapsed = t.elapsed();

    let src = match src {
        Ok(s) => s,
        Err(e) => {
            phases.push(Phase {
                label: "read",
                elapsed: read_elapsed,
                note: None,
            });
            let total: Duration = phases.iter().map(|p| p.elapsed).sum();
            return CompileOutcome {
                timing: DevTiming {
                    file: file.to_string(),
                    ok: false,
                    total,
                    phases,
                },
                diagnostic: Some(format!("error: {e}")),
                schema: None,
                fixture: None,
                version: None,
                interface: None,
            };
        }
    };
    phases.push(Phase {
        label: "read",
        elapsed: read_elapsed,
        note: None,
    });

    let t = Instant::now();
    let result = load_sources_diag(&src);
    let (fixture, version, interface) = if result.is_ok() {
        // `""` for the seed: `ordo dev`'s own seeding is `dev_db::reset`
        // reading `seed.sql` straight into the pool (§4.3); a `Fixture`'s
        // compiled-in seed is a fixture-harness convenience this path
        // never calls.
        (
            ordo_conformance::Fixture::from_source(src.text(), "").ok(),
            Some(dev_version(src.text())),
            if descriptor {
                crate::interface_json(&src).ok()
            } else {
                None
            },
        )
    } else {
        (None, None, None)
    };
    let compile_elapsed = t.elapsed();
    phases.push(Phase {
        label: "compile",
        elapsed: compile_elapsed,
        note: None,
    });

    let total: Duration = phases.iter().map(|p| p.elapsed).sum();
    let (ok, diag_text, schema) = match result {
        Ok((schema, _registry)) => (true, None, Some(schema)),
        Err(d) => (false, Some(render_diagnostic(&src, &d)), None),
    };
    CompileOutcome {
        timing: DevTiming {
            file: file.to_string(),
            ok,
            total,
            phases,
        },
        diagnostic: diag_text,
        schema,
        fixture,
        version,
        interface,
    }
}

/// Fold a measured `watch` phase into a timing [`compile_once`] already
/// built, keeping `total` the sum of every phase including it.
fn with_watch(mut timing: DevTiming, watch_elapsed: Duration) -> DevTiming {
    timing.phases.insert(
        0,
        Phase {
            label: "watch",
            elapsed: watch_elapsed,
            note: None,
        },
    );
    timing.total += watch_elapsed;
    timing
}

/// How long the watcher waits for the filesystem to go quiet before it
/// treats a burst of events as one save. Small on purpose: it is spent on
/// every save and the budget in `plan-dev-loop.md` §3.3 is a total, not a
/// per-phase allowance for debounce specifically.
pub const DEBOUNCE: Duration = Duration::from_millis(40);

fn touches_ordo(event: &notify::Event) -> bool {
    event
        .paths
        .iter()
        .any(|p| p.extension().is_some_and(|x| x == "ordo"))
}

/// A running watch, and the channel its callback feeds. Kept together so a
/// caller — [`run`], or a test that wants the real filesystem rather than a
/// direct [`compile_once`] call — holds the watcher alive for exactly as
/// long as it reads from the channel.
pub struct Watch {
    _watcher: notify::RecommendedWatcher,
    rx: std::sync::mpsc::Receiver<(Instant, notify::Event)>,
}

/// Start watching `root` recursively. The watcher must outlive every call
/// to [`next_save`] — dropping [`Watch`] stops it.
pub fn start_watch(root: &Path) -> Result<Watch, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let _ = tx.send((Instant::now(), event));
        }
    })
    .map_err(|e| format!("watcher: {e}"))?;
    notify::Watcher::watch(&mut watcher, root, notify::RecursiveMode::Recursive)
        .map_err(|e| format!("watcher: {e}"))?;
    Ok(Watch {
        _watcher: watcher,
        rx,
    })
}

/// Block for the next `.ordo` save, debounced (§6 D1: a burst of events from
/// one editor save becomes one recompile), and return the path that
/// triggered it plus how long the whole burst took from the first raw event
/// — the `watch` phase.
///
/// `deadline` bounds the wait so a caller (a test, in particular) does not
/// hang forever on a filesystem that never delivers an event; `run` passes
/// none, because the loop is meant to wait forever.
pub fn next_save(watch: &Watch, deadline: Option<Duration>) -> Option<(PathBuf, Duration)> {
    let started = Instant::now();
    let recv_first = |remaining: Option<Duration>| match remaining {
        Some(d) => watch.rx.recv_timeout(d).ok(),
        None => watch.rx.recv().ok(),
    };
    loop {
        let remaining = deadline.map(|d| d.saturating_sub(started.elapsed()));
        if deadline.is_some() && remaining == Some(Duration::ZERO) {
            return None;
        }
        let (first_seen, event) = recv_first(remaining)?;
        if !touches_ordo(&event) {
            continue;
        }
        // Debounce: an editor's atomic save is a burst of events (write,
        // rename, metadata), and `Sources::collect`'s watch set is exactly
        // the files this reads back, so a burst must become one recompile.
        loop {
            match watch.rx.recv_timeout(DEBOUNCE) {
                Ok((_, ev)) if touches_ordo(&ev) => continue,
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let label = event.paths.first().cloned().unwrap_or_default();
        return Some((label, first_seen.elapsed()));
    }
}

/// `ordo dev`'s CLI flags, threaded into [`run`].
#[derive(Default)]
pub struct DevOptions {
    /// `--fresh`: destroy an owned cluster's data directory before starting
    /// (§4.1) — the escape hatch for a warm database that will not come up
    /// clean, or a schema someone wants to start over from.
    pub fresh: bool,
    /// `--out <dir>` (D4): write `interface.json` there on every save whose
    /// descriptor's digest moved. `None` skips the whole of D4 — no third
    /// compile pass, no file.
    pub out: Option<PathBuf>,
    /// `--gen <command>` (D4): run after a digest change, with the
    /// descriptor's path in `ORDO_INTERFACE_PATH`. Ignored without `--out`.
    pub gen: Option<String>,
}

/// What `ordo dev` believes is live right now, carried from one save to the
/// next. `None` until the first successful reset — see [`dev_db::classify`]
/// on why drops are invisible without a prior declaration to compare
/// against.
struct DevState {
    last_declared: Option<ordo_schema::Schema>,
    /// D4's change signal: the last `interface.json` this loop wrote (or
    /// found already current — see [`write_descriptor`]), compared by
    /// value against each save's freshly rendered one. `None` until the
    /// first save with `--out` set.
    last_interface: Option<String>,
}

/// D3's serving context: everything a save needs to gate and swap, over the
/// same [`ordo_host::deploy::Deployment`] [`ordo_host::runtime::assemble_embedded`]
/// built at boot. Cloned out of the [`ordo_host::runtime::Runtime`] before
/// handing it to [`ordo_host::runtime::serve`], which takes it by value.
struct DevServing {
    pool: Pool,
    pg: ordo_exec::PoolConfig,
    keys: std::sync::Arc<dyn ordo_crypto::KeyProvider>,
    space: ordo_crypto::SpaceId,
    deployment: std::sync::Arc<ordo_host::deploy::Deployment>,
    /// The three pieces `ordo_host::runtime::refresh_access` needs besides
    /// `deployment`/`pool`: **not optional after a swap**. `Deployment::swap`
    /// moves the dispatch pointer; it does not touch the access engine, and
    /// a swap without this follow-up leaves the door refusing every op the
    /// save just added (`access_denied` — the engine has no policy for a
    /// name it was built without). `spawn_deploy_listener` needs the exact
    /// same follow-up for a second process's `ordo apply`, and now both
    /// callers get it from one function.
    metrics: std::sync::Arc<ordo_host::metrics::Metrics>,
    access: std::sync::Arc<ordo_host::runtime::SwappableAccess>,
    delegations: std::sync::Arc<ordo_host::delegation::PgDelegationRegistry>,
    token_secret: [u8; 32],
    /// The fourth piece `refresh_access` needs (18d T3, 18e U4): the dev
    /// loop composes no `ORDO_TENANT_CREDENTIALS` of its own (see
    /// `dev_runtime_config`'s doc), so this is always the empty
    /// `Installed`, held rather than rebuilt so a rebuild reuses the exact
    /// provider the boot did.
    credentials: std::sync::Arc<dyn ordo_exec::route::DbCredentials>,
}

/// D4: write `interface.json` when its content moved since the last save,
/// and — on a write — run the configured generator. Appends `descriptor`
/// and, when it ran, `generator` phases to `timing` in place, and pushes
/// any message worth printing (a write failure, a non-empty generator
/// output) onto `notes`.
///
/// "Moved" is bytes-equality against `state.last_interface`: `interface_json`
/// is already digest-gated internally (`interface_document` computes its
/// own `digest` field over the document minus that key), so identical
/// content means an identical digest by construction and there is nothing
/// this loop needs to recompute — see [`crate::dev::CompileOutcome::interface`]'s
/// own doc.
fn run_descriptor(
    timing: &mut DevTiming,
    notes: &mut Vec<String>,
    state: &mut DevState,
    out_dir: &Path,
    interface: &str,
    gen: Option<&str>,
) {
    let t = Instant::now();
    let changed = state.last_interface.as_deref() != Some(interface);
    let mut wrote = false;
    let descriptor_note = if !changed {
        "unchanged".to_string()
    } else {
        match write_descriptor(out_dir, interface) {
            Ok(()) => {
                state.last_interface = Some(interface.to_string());
                wrote = true;
                "written".to_string()
            }
            Err(e) => {
                notes.push(format!("error: writing interface.json: {e}"));
                format!("write failed: {e}")
            }
        }
    };
    timing.phases.push(Phase {
        label: "descriptor",
        elapsed: t.elapsed(),
        note: Some(descriptor_note),
    });
    timing.total += timing.phases.last().expect("just pushed").elapsed;

    // The seam, not the generator (§6 D4): a configured command runs after
    // a digest change and only then, with the descriptor's path in the
    // environment. A generator that exits non-zero prints its output and
    // does not stop the loop — `timing.ok` is untouched either way.
    if wrote {
        if let Some(cmd) = gen {
            let gt = Instant::now();
            let (ok, output) = run_generator(cmd, &out_dir.join("interface.json"));
            timing.phases.push(Phase {
                label: "generator",
                elapsed: gt.elapsed(),
                note: Some(if ok { "ok".into() } else { "failed".into() }),
            });
            timing.total += timing.phases.last().expect("just pushed").elapsed;
            if !ok || !output.trim().is_empty() {
                notes.push(format!(
                    "generator `{cmd}`{}:\n{output}",
                    if ok { "" } else { " (non-zero exit)" }
                ));
            }
        }
    }
}

/// Write `interface.json` under `out_dir`, creating the directory if it
/// does not exist yet — a fresh `--out` target is the ordinary case, not
/// an error.
fn write_descriptor(out_dir: &Path, interface: &str) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let path = out_dir.join("interface.json");
    std::fs::write(&path, interface).map_err(|e| format!("{}: {e}", path.display()))
}

/// Run `cmd` through the shell, with `ORDO_INTERFACE_PATH` naming the
/// descriptor just written. Returns whether it exited zero and its
/// combined output (stdout then stderr) — printed by the caller, never
/// this loop's job to interpret.
fn run_generator(cmd: &str, interface_path: &Path) -> (bool, String) {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("ORDO_INTERFACE_PATH", interface_path)
        .output();
    match out {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.success(), text)
        }
        Err(e) => (false, format!("could not run `{cmd}`: {e}")),
    }
}

/// The D2 half of a save: classify the schema delta and settle it. A delta
/// the production apply path takes whole is applied through it
/// (`ordo_host::schema_apply::execute`, the same function `ordo migrate
/// --apply` calls) and the rows survive; anything else, and an empty
/// database, is a reset (§4.3). Appends `delta` and then `apply` or
/// `reset` phases to `timing` in place and returns the message naming what
/// changed, when there was one. Factored out so the pre-boot call (D3 has
/// no `Deployment` yet) and every post-boot save run identical logic.
async fn settle_delta(
    timing: &mut DevTiming,
    pool: &Pool,
    root: &Path,
    schema: &ordo_schema::Schema,
    ddl: &str,
    state: &mut DevState,
) -> Option<String> {
    let t = Instant::now();
    let delta = dev_db::classify(pool, state.last_declared.as_ref(), schema).await;
    timing.phases.push(Phase {
        label: "delta",
        elapsed: t.elapsed(),
        note: None, // filled in below, once the outcome is known
    });
    timing.total += timing.phases.last().expect("just pushed").elapsed;

    let delta = match delta {
        Ok(d) => d,
        Err(e) => {
            timing.ok = false;
            return Some(format!("error: reading the live schema: {e}"));
        }
    };

    match delta {
        dev_db::Delta::NoChange => {
            timing.phases.last_mut().expect("delta phase").note = Some("0 changes".into());
            state.last_declared = Some(schema.clone());
            None
        }
        dev_db::Delta::Apply { changes, steps } => {
            let n = changes.len();
            timing.phases.last_mut().expect("delta phase").note =
                Some(format!("{n} change{}", if n == 1 { "" } else { "s" }));
            let names = dev_db::describe(&changes);

            let t = Instant::now();
            let applied = ordo_host::schema_apply::execute(pool, &steps).await;
            let apply_elapsed = t.elapsed();
            timing.total += apply_elapsed;
            match applied {
                Ok(x) => {
                    timing.phases.push(Phase {
                        label: "apply",
                        elapsed: apply_elapsed,
                        note: Some(format!(
                            "{} statement{}",
                            x.transactional + x.indexes,
                            if x.transactional + x.indexes == 1 {
                                ""
                            } else {
                                "s"
                            }
                        )),
                    });
                    state.last_declared = Some(schema.clone());
                    Some(format!(
                        "schema changed — applied through the production class-A path \
                         (plan-dev-loop.md §4.3); your rows are still there:\n{names}"
                    ))
                }
                Err(e) => {
                    timing.phases.push(Phase {
                        label: "apply",
                        elapsed: apply_elapsed,
                        note: None,
                    });
                    timing.ok = false;
                    Some(format!("error: apply failed: {e}\n{names}"))
                }
            }
        }
        dev_db::Delta::Reset { changes, refused } => {
            let n = changes.len();
            timing.phases.last_mut().expect("delta phase").note =
                Some(format!("{n} change{}", if n == 1 { "" } else { "s" }));
            let names = dev_db::describe(&changes);
            let because: String = refused
                .iter()
                .map(|x| format!("\n  {} {} — {}", x.change.code(), x.class, x.why))
                .collect();

            let t = Instant::now();
            let reset_result =
                dev_db::reset(pool, root, schema, state.last_declared.as_ref(), ddl).await;
            let reset_elapsed = t.elapsed();
            timing.total += reset_elapsed;
            match reset_result {
                Ok(report) => {
                    timing.phases.push(Phase {
                        label: "reset",
                        elapsed: reset_elapsed,
                        note: Some(if report.seeded {
                            "seeded".into()
                        } else {
                            "no seed.sql, empty".into()
                        }),
                    });
                    state.last_declared = Some(schema.clone());
                    // 27p: a reset drops Ordo's tables only. What else the
                    // database holds is named, so a dependency on another
                    // system stays visible rather than silently surviving.
                    let kept = if report.kept.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "\nleft alone, not this program's: {}",
                            report.kept.join(", ")
                        )
                    };
                    Some(if refused.is_empty() {
                        format!(
                            "schema created — the database held none of this program's \
                             tables, so the loop created and seeded it:\n{names}{kept}"
                        )
                    } else {
                        format!(
                            "schema changed — the loop reset the database rather than \
                             migrate it (plan-dev-loop.md §4.3: the production apply path \
                             refuses part of this delta, and `ordo dev` never runs a \
                             migration production would not run):{because}\n{names}{kept}"
                        )
                    })
                }
                Err(e) => {
                    timing.phases.push(Phase {
                        label: "reset",
                        elapsed: reset_elapsed,
                        note: None,
                    });
                    timing.ok = false;
                    Some(format!("error: reset failed: {e}\n{names}"))
                }
            }
        }
    }
}

/// One save's compile, schema delta, reset (D1/D2, unchanged), gate and
/// swap (D3). Everything between "a file changed" and "here is the
/// journal". `None` for `serving` on the very first call: the boot's own
/// gate-and-swap happens inside `assemble_embedded`, and this is called
/// once before `serving` exists, purely to compile and settle the database
/// ahead of that boot.
async fn process_save(
    pool: &Pool,
    root: &Path,
    label: &str,
    state: &mut DevState,
    serving: Option<&DevServing>,
    opts: &DevOptions,
) -> (DevTiming, Option<String>) {
    let outcome = compile_once(root, label, opts.out.is_some());
    let mut timing = outcome.timing;
    let (Some(schema), Some(fixture), Some(version)) =
        (outcome.schema, outcome.fixture, outcome.version)
    else {
        // A save that does not compile touches no database and swaps
        // nothing (§4.2/D3 are both about a program that DOES compile).
        // `state.last_declared` is left exactly as it was.
        return (timing, outcome.diagnostic);
    };

    // D4: the descriptor, digest-gated, independent of D3's serving — a
    // signature changes whether or not the gate/swap below ever runs.
    let mut extra_notes: Vec<String> = Vec::new();
    if let (Some(out_dir), Some(interface)) = (&opts.out, &outcome.interface) {
        run_descriptor(
            &mut timing,
            &mut extra_notes,
            state,
            out_dir,
            interface,
            opts.gen.as_deref(),
        );
    }

    let ddl = match ordo_schema::ddl_for(&schema) {
        Ok(d) => d,
        Err(e) => {
            timing.ok = false;
            return (
                timing,
                Some(format!("error: this program's schema has no DDL: {e}")),
            );
        }
    };

    let note = settle_delta(&mut timing, pool, root, &schema, &ddl, state).await;
    extra_notes.extend(note);
    let note = (!extra_notes.is_empty()).then(|| extra_notes.join("\n"));
    if !timing.ok {
        return (timing, note);
    }

    let Some(serving) = serving else {
        return (timing, note);
    };

    // The gate. Explicitly not skipped on a no-schema-change save (§6 D3:
    // "the gate still runs on every save" — an op's BODY can change with no
    // schema change at all).
    let t = Instant::now();
    let verified = ordo_host::deploy::verify_embedded(
        fixture,
        &serving.pool,
        &serving.pg,
        serving.keys.clone(),
        serving.space.clone(),
        &version,
    )
    .await;
    let gate_elapsed = t.elapsed();
    let verified = match verified {
        Ok(v) => v,
        Err(e) => {
            timing.phases.push(Phase {
                label: "gate",
                elapsed: gate_elapsed,
                note: None,
            });
            timing.total += gate_elapsed;
            timing.ok = false;
            return (
                timing,
                Some(format!("error: the load gate refused this save: {e}")),
            );
        }
    };
    let gate_note = format!("{} reads", verified.report().verified.len());
    timing.phases.push(Phase {
        label: "gate",
        elapsed: gate_elapsed,
        note: Some(gate_note),
    });
    timing.total += gate_elapsed;

    // The swap. `Deployment::swap` alone moves only the dispatch pointer —
    // an `Installed` result is followed by `refresh_access`, in the SAME
    // phase, because the two together are what "the op is callable" means
    // (see `refresh_access`'s own doc: without it the door keeps refusing
    // everything the save just added, `access_denied`, indistinguishable
    // from a caller who never had permission).
    let t = Instant::now();
    let swapped = serving.deployment.swap(verified);
    let swap_note = match &swapped {
        ordo_host::deploy::Swapped::Installed { .. } => {
            if let Err(e) = ordo_host::runtime::refresh_access(
                &serving.deployment,
                &serving.pool,
                &serving.token_secret,
                &serving.metrics,
                &serving.access,
                serving.delegations.clone(),
                &serving.credentials,
            )
            .await
            {
                let swap_elapsed = t.elapsed();
                timing.phases.push(Phase {
                    label: "swap",
                    elapsed: swap_elapsed,
                    note: Some("installed, access engine did not rebuild".into()),
                });
                timing.total += swap_elapsed;
                timing.ok = false;
                return (
                    timing,
                    Some(format!(
                        "error: swapped but the access engine did not rebuild — the door is \
                         still pinning the previous version's manifests: {e}"
                    )),
                );
            }
            "installed"
        }
        ordo_host::deploy::Swapped::AlreadyServing(_) => "unchanged",
        ordo_host::deploy::Swapped::Refused(_) => "refused",
    };
    let swap_elapsed = t.elapsed();
    timing.phases.push(Phase {
        label: "swap",
        elapsed: swap_elapsed,
        note: Some(swap_note.to_string()),
    });
    timing.total += swap_elapsed;
    if let ordo_host::deploy::Swapped::Refused(why) = swapped {
        timing.ok = false;
        return (timing, Some(format!("error: swap refused: {why}")));
    }

    (timing, note)
}

/// How the door's `RuntimeConfig` differs from a server's: composed by
/// `ordo dev` itself, over a struct, never over the environment
/// (`ordo_host::runtime`'s own module doc: *"this split is what lets a dev
/// loop build a config it composed itself and get exactly the deployment a
/// server gets"*). Custody is [`ordo_host::runtime::CustodyChoice::Test`] —
/// the provider production gates behind `ORDO_DEV_UNSAFE_TEST_KEYS` — which
/// is the correct default for a loop whose whole database is regenerated
/// seed data (§4.3) and never the correct default for a server, which is
/// why that gate exists at all; `ordo dev` simply IS the caller the gate
/// was written to let through.
///
/// 17w: `token_secret` reads `ORDO_TOKEN_KEY` the same way a server's own
/// `RuntimeConfig::from_env` does, so a script that wants a token good
/// across restarts (rather than the one this process prints, which dies
/// with it) can set the same variable it would set against a real
/// deployment. Unset — the ordinary case — falls back to the ephemeral,
/// process-derived key exactly as before; **set but malformed is a
/// refusal**, on the same posture `TokenKey::from_env` itself takes,
/// because a developer who typed `ORDO_TOKEN_KEY` meant something by it and
/// silently falling back would hide that they got it wrong.
fn dev_runtime_config(
    root: &Path,
    pg: ordo_exec::PoolConfig,
) -> Result<ordo_host::runtime::RuntimeConfig, String> {
    let addr_text = std::env::var("ORDO_DEV_ADDR").unwrap_or_else(|_| "127.0.0.1:4000".to_string());
    let addr: std::net::SocketAddr = addr_text
        .parse()
        .map_err(|_| format!("ORDO_DEV_ADDR: `{addr_text}` is not an address:port"))?;
    let token_secret = match ordo_identity::TokenKey::secret_from_env() {
        Ok(s) => s,
        Err(ordo_identity::TokenKeyError::Missing) => ephemeral_secret("ordo-dev-token", root),
        Err(e) => return Err(format!("ORDO_TOKEN_KEY: {e}")),
    };
    Ok(ordo_host::runtime::RuntimeConfig {
        // One bucket by construction: `blob` below is `None`, so there is
        // no store to attach a second one beside.
        blob_routing: ordo_host::runtime::BlobRouting::Single,
        pg,
        addr,
        shutdown_grace: Duration::from_secs(2),
        space: ordo_crypto::SpaceId("dev".into()),
        custody: ordo_host::runtime::CustodyChoice::Test,
        token_secret,
        page_key: ordo_api::PageKey::from_secret(&ephemeral_secret("ordo-dev-page", root)),
        checkpoint: None,
        security_checkpoint: false,
        // The sign-in settings a page needs are read from the same
        // variables a server reads, and only when they are set (25d):
        // unset, the loop is what it always was, with no login and no CORS.
        apex_domain: std::env::var("ORDO_APEX_DOMAIN").ok(),
        cors: ordo_host::runtime::RuntimeConfig::cors_from_env().map_err(|e| e.to_string())?,
        blob: None,
        exit_bundles: None,
        // The dev loop serves neither exit op and no import: both write or
        // read a directory of a space's whole contents beside the sources,
        // and the loop's database is disposable by design.
        import_bundles: None,
        // The dev loop is one process, so its replica id never has to
        // disambiguate against a sibling; the usage cadences match a
        // server's own defaults (`RuntimeConfig::from_env`) in case
        // `ORDO_USAGE_LEDGER` is set for a session that wants to exercise
        // metering, but nothing here reads `ORDO_USAGE_FLUSH_SECS` or
        // `ORDO_USAGE_ROLLUP_SECS` — a config `ordo dev` composes itself,
        // never over the environment, per this function's own doc.
        replica: "dev".to_string(),
        usage_flush: Duration::from_secs(60),
        usage_rollup: Duration::from_secs(3600),
        sync: Default::default(),
        // No login unless `ORDO_AUTH_METHODS` is set, and then the login
        // a server would mount: `passkey` with `ORDO_APEX_DOMAIN` and
        // `ORDO_AUTH_ORIGINS`, `ORDO_AUTH_PROVISIONING`, the session
        // lengths. Unset, the token secret is ephemeral and the whole
        // database is regenerated seed data, so a credential enrolled in
        // one session is meaningless in the next; a session that sets it
        // knows that, and the printed development token still works.
        auth: ordo_host::runtime::RuntimeConfig::auth_from_env().map_err(|e| e.to_string())?,
        // The dev loop composes no `ORDO_TENANT_CREDENTIALS` of its own,
        // the same "over a struct, never over the environment" posture
        // this function's own doc states for every other field: a
        // seeded dev database has no attached targets to hold a label
        // for, so there is nothing here for a provider to serve yet.
        credentials: None,
        presence: Default::default(),
        pausing: Default::default(),
        delegable: Default::default(),
        access_template: Default::default(),
        deployment_owners: Vec::new(),
        provisioning: Default::default(),
        recovery: Default::default(),
        second_factor: Default::default(),
        mail: None,
    })
}

/// 32 bytes good enough for a loopback-bound, single-process, ephemeral dev
/// session: derived from the label, the project path, the process id and
/// the current time, which is enough entropy that no two `ordo dev`
/// sessions mint the same token-signing key, and not a claim of
/// cryptographic randomness `ordo-crypto`'s real custody types would need
/// to make.
fn ephemeral_secret(label: &str, root: &Path) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(label.as_bytes());
    h.update(root.as_os_str().as_encoded_bytes());
    h.update(std::process::id().to_le_bytes());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    h.update(nanos.to_le_bytes());
    h.finalize().into()
}

/// `ordo dev <dir>` — D1's watch/compile/report, D2's own database and
/// schema delta, and D3's serve/gate/swap, all through
/// `ordo_host::runtime::assemble_embedded`/`serve` — the same assembly a
/// server gets. Stopped by Ctrl-C or SIGTERM, handled by `serve` itself
/// exactly as a server's are (drain flushed, workers cancelled, pool
/// closed) — `ordo dev` adds only tearing down its own owned cluster after.
pub fn run(root: &Path, opts: DevOptions) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    println!("ordo dev  watching {}", root.display());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    rt.block_on(run_async(root, opts))
}

async fn run_async(root: PathBuf, opts: DevOptions) -> Result<(), String> {
    let (cluster, pool_cfg) = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || dev_db::DevCluster::start(&root, opts.fresh))
            .await
            .map_err(|e| format!("cluster startup panicked: {e}"))??
    };
    println!(
        "  database    {}  {}@{}:{}/{}",
        match &cluster {
            dev_db::DevCluster::Owned(_) => "ordo dev's own cluster",
            dev_db::DevCluster::External => "external (ORDO_PG_* set)",
        },
        pool_cfg.user,
        pool_cfg.host,
        pool_cfg.port,
        pool_cfg.db,
    );
    let pool = Pool::connect(pool_cfg.clone())
        .await
        .map_err(|e| format!("database: {e}"))?;
    pool.ensure_platform_tables()
        .await
        .map_err(|e| format!("database: {e}"))?;

    let mut state = DevState {
        last_declared: None,
        last_interface: None,
    };

    // The pre-boot save: compile, settle the schema delta, but do not gate
    // or swap — `assemble_embedded` runs the FIRST gate itself, and there
    // is no `Deployment` to swap into before it returns one.
    let root_label = root.display().to_string();
    let (timing, diag) = process_save(&pool, &root, &root_label, &mut state, None, &opts).await;
    print_save(&timing, diag.as_deref());
    if !timing.ok {
        return Err("the initial compile failed — fix it and rerun `ordo dev`".to_string());
    }

    let fixture = compile_once(&root, &root_label, false)
        .fixture
        .ok_or("internal error: settled but did not recompile a Fixture")?;
    let cfg = dev_runtime_config(&root, pool_cfg.clone())?;
    let addr = cfg.addr;
    let t = Instant::now();
    let host_rt = ordo_host::runtime::assemble_embedded(cfg, fixture)
        .await
        .map_err(|e| format!("boot: {e}"))?;
    println!("  boot        gate + swap in {}ms", t.elapsed().as_millis());
    host_rt.print_startup();
    print_dev_token(addr, &host_rt.token_secret());

    let serving = DevServing {
        pool: host_rt.pool.clone(),
        pg: host_rt.pg.clone(),
        keys: host_rt.keys.clone(),
        space: host_rt.space.clone(),
        deployment: host_rt.deployment.clone(),
        metrics: host_rt.metrics.clone(),
        access: host_rt.access.clone(),
        delegations: host_rt.delegations(),
        token_secret: host_rt.token_secret(),
        credentials: host_rt.credentials.clone(),
    };

    // `serve` owns the full production shutdown sequence — stop accepting,
    // drain in-flight requests, cancel workers, flush the drain, close the
    // pool — for SIGINT/SIGTERM exactly as a server gets it. `ordo dev`
    // adds nothing here except noticing when it is done and tearing down
    // its own cluster after (§4.1).
    let mut server = tokio::spawn(ordo_host::runtime::serve(host_rt));

    let watch = start_watch(&root)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Some(event) = next_save(&watch, None) {
            if tx.send(event).is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            biased;
            outcome = &mut server => {
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => eprintln!("ordo dev: server: {e}"),
                    Err(e) => eprintln!("ordo dev: server task: {e}"),
                }
                break;
            }
            event = rx.recv() => {
                let Some((path, watch_elapsed)) = event else { break };
                let label = path
                    .strip_prefix(&root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string());
                let (timing, diag) =
                    process_save(&pool, &root, &label, &mut state, Some(&serving), &opts).await;
                let timing = with_watch(timing, watch_elapsed);
                print_save(&timing, diag.as_deref());
            }
        }
    }
    println!("ordo dev  stopping");
    // Kept alive until here on purpose: dropping it is what stops an owned
    // cluster (`OwnedCluster`'s `Drop`), and it must outlive every save
    // this loop processed and the server that just shut down.
    drop(cluster);
    Ok(())
}

fn print_save(timing: &DevTiming, diag: Option<&str>) {
    println!("{}", timing.journal());
    if let Some(d) = diag {
        println!("{d}");
    }
}

/// 17w — the wall a developer met calling their own op straight after `ordo
/// dev` came up clean: `{"error":{"code":"access_denied","message":"missing
/// bearer token"}}`, with no documented way past it, because
/// `dev_runtime_config` mounts no login route and the door refuses
/// `NoMembership` before it ever reaches a row guard. Printed once, at
/// boot, so the very next thing a developer can do after watching `ordo
/// dev` come up is paste this into a client: a bearer token, already good,
/// for the principal `dev_db::reset` just seeded into `members` with an
/// `owner` role in the `dev` space — no login, no membership concept
/// learned, before the op they just wrote answers.
///
/// **Local only, said plainly, because it is easy to mistake for something
/// that travels.** The key it is signed under is either `ephemeral_secret`
/// (dies with this process — a fresh one every `ordo dev`) or, when
/// `ORDO_TOKEN_KEY` is set, that key (see `dev_runtime_config`'s doc); the
/// principal it names exists only in the database this loop just reset.
/// Either way the token this prints answers nothing outside the deployment
/// that printed it.
fn print_dev_token(addr: std::net::SocketAddr, token_secret: &[u8; 32]) {
    let token = ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(token_secret),
        &ordo_identity::TokenSpec {
            principal: ordo_identity::PrincipalId(dev_db::DEV_PRINCIPAL_ID.to_string()),
            // Outlives any `ordo dev` session; there is nothing to renew
            // against and no session to log out of (`auth: None` — see
            // `dev_runtime_config`'s own doc on why there is no login
            // route).
            expires: ordo_identity::Timestamp(u64::MAX / 2),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    );
    println!("  token       Bearer {token}");
    println!("              local only — signed by a key that dies with this process");
    println!(
        "  try it      curl -H \"authorization: Bearer {token}\" -H \"x-ordo-space: dev\" \
         http://{addr}/op/<yourOp>"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One entity, one op — just enough for a real bearer token to answer
    /// with a real row.
    const SRC: &str = r#"
entity widgets {
  name: text
}

op listWidgets unbounded =
  widgets[name]
"#;

    fn free_addr() -> std::net::SocketAddr {
        std::net::TcpListener::bind("127.0.0.1:0")
            .expect("a port")
            .local_addr()
            .expect("addr")
    }

    /// 17w's own done-when, proved rather than claimed: the token `ordo
    /// dev` prints at startup calls a real op over a real socket and gets
    /// real rows back.
    ///
    /// **In-process, not a backgrounded `ordo dev` binary.** A backgrounded
    /// child gets killed quickly in this sandbox (noted against
    /// `ordo-host/tests/dev_loop.rs`'s own D3 test, which takes the same
    /// shortcut for the same reason). What is NOT a shortcut: every
    /// function under test is the real one — `dev_db::reset` seeds the
    /// real `members` row, `dev_runtime_config` composes the real token
    /// secret, `ordo_host::runtime::assemble_embedded`/`serve` bind a real
    /// `TcpListener` — and the call below is a real HTTP/1.1 request over
    /// that socket, `ordo_blob::http::send` (the same client `ordo apply
    /// --host` uses, `wire.rs`), never `tower::ServiceExt::oneshot`. A
    /// `oneshot` call proves the router; this proves the wire the
    /// quickstart's `curl` line actually uses.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_printed_token_calls_a_real_op_over_real_http() {
        let db = "ordo_cli_dev_wall_test";
        let pg = ordo_conformance::Pg::named(db);
        pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
            .expect("reset schema");
        let pg_cfg = ordo_exec::PoolConfig {
            db: db.into(),
            size: 2,
            ..ordo_exec::PoolConfig::from_env()
        };
        let pool = Pool::connect(pg_cfg.clone())
            .await
            .expect("connect to the test cluster");
        pool.ensure_platform_tables()
            .await
            .expect("platform tables");

        let (schema, _registry) = crate::load(SRC).expect("SRC compiles");
        let ddl = ordo_schema::ddl_for(&schema).expect("ddl");
        let root =
            std::env::temp_dir().join(format!("ordo-dev-wall-{}-{}", std::process::id(), "e2e"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");

        // The wall's own fix (17w): `ordo dev`'s own reset seeds `members`
        // with the dev principal — there is no `seed.sql` in this scratch
        // root, exactly like a fresh project's very first save.
        dev_db::reset(&pool, &root, &schema, None, &ddl)
            .await
            .expect("reset seeds the schema and the dev principal");

        // A row for the op to answer with — the developer's own saved data,
        // stood in for.
        pool.rows(
            "INSERT INTO widgets (\"$space\", id, rev, name) VALUES \
             ('dev', 'wid_1', 1, 'wall test')",
        )
        .await
        .expect("seed a row");

        let fixture = ordo_conformance::Fixture::from_source(SRC, "").expect("fixture compiles");
        let mut cfg = dev_runtime_config(&root, pg_cfg).expect("dev_runtime_config");
        // `ORDO_DEV_ADDR`'s default (4000) is not this test's to claim —
        // an allocated loopback port, the same way `dev_db`'s own cluster
        // picks one.
        cfg.addr = free_addr();
        let addr = cfg.addr;
        let secret = cfg.token_secret;

        let host_rt = ordo_host::runtime::assemble_embedded(cfg, fixture)
            .await
            .expect("assemble_embedded boots — the same gate a server's assemble() runs");
        tokio::spawn(ordo_host::runtime::serve(host_rt));

        // Minted exactly the way `print_dev_token` mints the line `ordo
        // dev` prints.
        let token = ordo_identity::mint(
            &ordo_identity::TokenKey::from_secret(&secret),
            &ordo_identity::TokenSpec {
                principal: ordo_identity::PrincipalId(dev_db::DEV_PRINCIPAL_ID.to_string()),
                expires: ordo_identity::Timestamp(u64::MAX / 2),
                client: None,
                user_presence_at: None,
                capability: None,
                delegation: None,
                session: None,
            },
        );

        let endpoint = ordo_blob::http::Endpoint {
            host: "127.0.0.1".to_string(),
            port: addr.port(),
            tls: false,
        };
        // `serve` binds its listener asynchronously after `assemble_embedded`
        // returns; poll rather than race it — bounded, so a door that never
        // comes up fails the test instead of hanging it.
        let deadline = Instant::now() + Duration::from_secs(5);
        let response = loop {
            let attempt = ordo_blob::http::send(
                &endpoint,
                ordo_blob::http::Request {
                    method: "GET",
                    path: "/op/listWidgets".to_string(),
                    headers: vec![
                        ("authorization".into(), format!("Bearer {token}")),
                        ("x-ordo-space".into(), "dev".into()),
                        ("accept".into(), "application/json".into()),
                    ],
                    body: None,
                },
            )
            .await;
            match attempt {
                Ok(r) => break r,
                Err(e) if Instant::now() < deadline => {
                    let _ = e;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => panic!("the door never came up on {addr}: {e}"),
            }
        };

        assert_eq!(
            response.status, 200,
            "the printed dev token must open the door for the op it names"
        );
        let mut body = Vec::new();
        {
            use futures::StreamExt;
            let mut stream = response.body;
            while let Some(chunk) = stream.next().await {
                body.extend_from_slice(&chunk.expect("body chunk"));
            }
        }
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(
            value["rows"][0]["name"], "wall test",
            "the op must answer with the row this test seeded: {value}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 17w: `ORDO_TOKEN_KEY`, read the same way a server's own
    /// `RuntimeConfig::from_env` reads it — a well-formed key overrides the
    /// ephemeral one, and a malformed one is a refusal rather than a
    /// silent fallback (a developer who set it meant something by it). One
    /// test, not two: both halves mutate the same process-global
    /// environment variable, and a single function is what keeps them from
    /// racing each other under a parallel test runner.
    /// Serialises the tests that set the sign-in variables, which are
    /// process-global.
    static SIGN_IN_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const SIGN_IN_VARS: [&str; 5] = [
        "ORDO_CORS_ORIGINS",
        "ORDO_AUTH_METHODS",
        "ORDO_APEX_DOMAIN",
        "ORDO_AUTH_ORIGINS",
        "ORDO_AUTH_PROVISIONING",
    ];

    /// 25d: `ordo dev` set `auth: None` and `cors: None`, so a page on
    /// another origin could not call it and an app with passkeys could not be
    /// developed on it. Both are now read from the variables a server reads,
    /// when they are set, and a door built from them answers a preflight and
    /// begins a passkey ceremony. Unset, the loop is what it was.
    #[tokio::test(flavor = "multi_thread")]
    async fn dev_honours_cors_and_passkey_settings_when_set_and_ignores_them_when_not() {
        let root = std::env::temp_dir().join(format!("ordo-dev-signin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        let db = "ordo_cli_dev_signin_test";
        let pg = ordo_conformance::Pg::named(db);
        pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
            .expect("reset schema");
        let pg_cfg = ordo_exec::PoolConfig {
            db: db.into(),
            size: 2,
            ..ordo_exec::PoolConfig::from_env()
        };

        // Unset: no CORS, no sign-in, as before.
        let mut cfg = {
            let _env = SIGN_IN_ENV.lock().unwrap_or_else(|e| e.into_inner());
            for v in SIGN_IN_VARS {
                std::env::remove_var(v);
            }
            let plain = dev_runtime_config(&root, pg_cfg.clone()).expect("plain config");
            assert!(plain.cors.is_none() && plain.auth.is_none() && plain.apex_domain.is_none());

            // Set: read the way a server reads them.
            std::env::set_var("ORDO_CORS_ORIGINS", "http://localhost:5173");
            std::env::set_var("ORDO_AUTH_METHODS", "passkey");
            std::env::set_var("ORDO_APEX_DOMAIN", "localhost");
            std::env::set_var("ORDO_AUTH_ORIGINS", "http://localhost:5173");
            std::env::set_var("ORDO_AUTH_PROVISIONING", "auto-onboard");
            let cfg = dev_runtime_config(&root, pg_cfg.clone()).expect("configured");
            for v in SIGN_IN_VARS {
                std::env::remove_var(v);
            }
            cfg
        };
        assert!(cfg.cors.is_some(), "ORDO_CORS_ORIGINS is honoured");
        let auth = cfg.auth.as_ref().expect("ORDO_AUTH_METHODS is honoured");
        assert_eq!(auth.methods, ["passkey"]);
        assert!(auth.auto_onboard && auth.relying_party.is_some());

        let pool = Pool::connect(pg_cfg.clone()).await.expect("connect");
        pool.ensure_platform_tables()
            .await
            .expect("platform tables");
        let (schema, _registry) = crate::load(SRC).expect("SRC compiles");
        let ddl = ordo_schema::ddl_for(&schema).expect("ddl");
        dev_db::reset(&pool, &root, &schema, None, &ddl)
            .await
            .expect("reset");
        let fixture = ordo_conformance::Fixture::from_source(SRC, "").expect("fixture compiles");
        cfg.addr = free_addr();
        let addr = cfg.addr;
        let host_rt = ordo_host::runtime::assemble_embedded(cfg, fixture)
            .await
            .expect("a door with sign-in boots");
        tokio::spawn(ordo_host::runtime::serve(host_rt));

        let endpoint = ordo_blob::http::Endpoint {
            host: "127.0.0.1".to_string(),
            port: addr.port(),
            tls: false,
        };
        let call = |method: &'static str,
                    path: &'static str,
                    headers: Vec<(String, String)>,
                    body: Option<&'static str>| {
            let endpoint = endpoint.clone();
            async move {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let attempt = ordo_blob::http::send(
                        &endpoint,
                        ordo_blob::http::Request {
                            method,
                            path: path.to_string(),
                            headers: headers.clone(),
                            body: body.map(|b| {
                                let bytes = b.as_bytes().to_vec();
                                let len = bytes.len() as u64;
                                let stream: ordo_blob::ByteStream =
                                    Box::pin(futures::stream::once(async move { Ok(bytes) }));
                                (len, stream)
                            }),
                        },
                    )
                    .await;
                    match attempt {
                        Ok(r) => return r,
                        Err(_) if Instant::now() < deadline => {
                            tokio::time::sleep(Duration::from_millis(20)).await
                        }
                        Err(e) => panic!("the door never came up on {addr}: {e}"),
                    }
                }
            }
        };

        let preflight = call(
            "OPTIONS",
            "/op/listWidgets",
            vec![
                ("origin".into(), "http://localhost:5173".into()),
                ("access-control-request-method".into(), "GET".into()),
            ],
            None,
        )
        .await;
        assert!(
            preflight.status == 204 || preflight.status == 200,
            "a listed origin's preflight is answered: {}",
            preflight.status
        );
        assert_eq!(
            preflight.header("access-control-allow-origin"),
            Some("http://localhost:5173")
        );

        let begin = call(
            "POST",
            "/auth/begin",
            vec![
                ("content-type".into(), "application/json".into()),
                ("origin".into(), "http://localhost:5173".into()),
            ],
            Some(r#"{"method":"passkey"}"#),
        )
        .await;
        assert_eq!(begin.status, 200, "the sign-in route is mounted");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// S6-009: `ordo dev` composes its custody as `Test` over a struct and
    /// never goes through the `ORDO_KEY_PROVIDER` reader, so refusing
    /// `local` outside development needs no setting from a developer.
    #[test]
    fn dev_custody_is_the_test_provider_with_no_setting() {
        let root = std::env::temp_dir().join("ordo-dev-wall-custody");
        let cfg = dev_runtime_config(&root, ordo_exec::PoolConfig::from_env())
            .expect("dev_runtime_config");
        assert_eq!(cfg.custody, ordo_host::runtime::CustodyChoice::Test);
    }

    #[test]
    fn ordo_token_key_overrides_or_refuses() {
        let root = std::env::temp_dir().join("ordo-dev-wall-token-key");

        let hex = "11".repeat(32);
        std::env::set_var("ORDO_TOKEN_KEY", &hex);
        let cfg = dev_runtime_config(&root, ordo_exec::PoolConfig::from_env())
            .expect("a well-formed ORDO_TOKEN_KEY is accepted");
        assert_eq!(
            cfg.token_secret, [0x11u8; 32],
            "the raw secret behind ORDO_TOKEN_KEY, unhashed — TokenKey::from_secret \
             derives from it, the same as a server's RuntimeConfig::from_env"
        );

        std::env::set_var("ORDO_TOKEN_KEY", "not-hex");
        // `RuntimeConfig` is not `Debug` (its token secret is not a thing
        // that should print), so this is a match rather than `expect_err`.
        let err = match dev_runtime_config(&root, ordo_exec::PoolConfig::from_env()) {
            Err(e) => e,
            Ok(_) => {
                panic!("a malformed key must refuse rather than fall back to the ephemeral one")
            }
        };
        assert!(err.contains("ORDO_TOKEN_KEY"), "{err}");

        std::env::remove_var("ORDO_TOKEN_KEY");
        let cfg = dev_runtime_config(&root, ordo_exec::PoolConfig::from_env())
            .expect("unset falls back to the ephemeral secret");
        assert_ne!(
            cfg.token_secret, [0x11u8; 32],
            "unset must not still read the override from a prior call"
        );
    }
}
