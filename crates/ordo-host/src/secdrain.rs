//! 7c — the drainer, and what the stream does when its storage is not
//! there.
//!
//! # The gap this closes
//!
//! 7a built the record: the chain, `verify_stream`, `PgSecurityStore`, the
//! deny-volume monitor. What it did not build was anything that *moves* an
//! entry from the recorder to the database. `ordo_secevent::Recorder`
//! accumulated; `PgSecurityStore` was constructed only in tests. In a
//! long-running process that is two failures at once, and the second is the
//! serious one:
//!
//! - an in-process buffer that only grows, and
//! - **a restart that loses every unwritten denial** — which are precisely
//!   the events a security review would go looking for, because a process
//!   that restarts under a denial storm restarts *because* of it.
//!
//! [`SecurityDrain`] is the missing half. Its shape is deliberately the one
//! this deployment already uses for background work over Postgres — see
//! [`crate::invalidation::PgInvalidationBus`]: a constructor that starts a
//! task, a handle whose lifetime is the task's lifetime, and no broker.
//!
//! # Off the request path, and why that is not negotiable
//!
//! `security-logging-design.md` §6.1 pins it twice over: appends "run **off
//! the request path** through the ordinary job queue", and "a stream append
//! failure is an **operational alert**, never a failed request. The stream
//! is evidence, not a precondition."
//!
//! So the door's cost for recording a decision is a lock, a map lookup and
//! an increment (see the audit in the checklist). No database round trip
//! happens on any request thread. The drainer runs on its own task, at its
//! own cadence, and every fault it meets is reported rather than propagated.
//!
//! # When the store is unreachable
//!
//! The full argument for *drop-bounded-and-declare* over *refuse the
//! request* and *spill to disk* is in `ordo_secevent::recorder`'s module
//! doc, because the bound lives there. What lives here is the part that
//! makes the loss **visible**, and it is visible in four places rather than
//! one:
//!
//! 1. **In the stream itself.** A refused observation becomes an
//!    `EventKind::EventsDropped` link, appended as soon as the store is
//!    reachable. This is the one that survives a restart and the one an
//!    auditor is actually reading.
//! 2. **In the counters.** [`APPENDED`], [`APPEND_FAILURES`], [`DROPPED`]
//!    and [`OBSERVED`] are ordinary series on `/metrics`, so a deployment's
//!    existing alerting can page on them today.
//! 3. **On `/health/deep`**, as a `security_stream` block naming the
//!    backlog, the last error, and how long it has been failing.
//! 4. **In `/health/deep`'s top-level `status`**, which reads `degraded`
//!    once the drain has been failing for [`DrainHealth::DEGRADED_SECS`].
//!
//! **Not** on `/health/ready`, and that is a decision rather than an
//! oversight. Readiness is what a load balancer reads. Failing it because
//! the *audit* subsystem cannot write would pull a node that is serving
//! correct answers out of rotation, and if the cause is the shared database
//! it would pull every node out at once — converting a logging fault into
//! the outage. That is the same trade §6.1 already made at the request
//! level, applied one layer up, and it is the trade this whole module is
//! built around: the stream never takes traffic down, and it never goes
//! quiet without saying so.
//!
//! # Shutdown drains
//!
//! [`SecurityDrain::shutdown`] closes every open collapse window and
//! appends what is in the outbox before the process exits. It is what makes
//! a *planned* restart lossless — a rolling deploy, a scale-down — which is
//! the overwhelming majority of restarts. A `SIGKILL` still loses the
//! buffer, and nothing short of an append on the request path would change
//! that; the shutdown drain narrows the window to the crashes, and the
//! outbox bound keeps even those finite.
//!
//! Shutdown **joins** the loop rather than aborting it. A pass holds the
//! entries it drained from the recorder across its awaits (the resolver,
//! then the append), and an abort that lands inside one of those drops
//! them: neither written nor requeued. Found as a flake in
//! `attached_tenant.rs` under full-suite load, where a slow target made the
//! window wide enough to hit. So `shutdown` sets the stop flag, wakes the
//! loop out of its sleep, waits for the pass in flight to finish, and only
//! then drains what is left.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_commandlog::{Ed25519Signer, Timestamp};
use ordo_exec::route::{PoolResolver, SinglePool};
use ordo_exec::secevent::PgSecurityStore;
use ordo_exec::{Pool, PoolError};
use ordo_fault::{FaultClass, StoreFault};
use ordo_secevent::{Recorder, Retention, Scope, StreamEntry};

use crate::metrics::Metrics;

/// `ordo_secevent_appended_total{}` — entries committed to durable storage.
pub const APPENDED: &str = "ordo_secevent_appended_total";
/// `ordo_secevent_append_failures_total{}` — drain passes that could not
/// commit. A non-zero and climbing value is the alert.
pub const APPEND_FAILURES: &str = "ordo_secevent_append_failures_total";
/// `ordo_secevent_dropped_total{}` — observations the outbox bound refused.
/// **Any** non-zero value here means the record has a declared gap in it.
pub const DROPPED: &str = "ordo_secevent_dropped_total";
/// `ordo_secevent_observed_total{}` — everything handed to the recorder.
/// The denominator that makes [`DROPPED`] legible as a fraction.
pub const OBSERVED: &str = "ordo_secevent_observed_total";
/// `ordo_secevent_pruned_total{}` — entries retention removed.
pub const PRUNED: &str = "ordo_secevent_pruned_total";
/// `ordo_secevent_checkpoints_total{}` — stream checkpoints signed.
///
/// The counter that answers "is anything anchoring the end of the stream".
/// A stream with traffic and a flat value here is a stream on which
/// `ordo verify`'s fourth check — nothing was removed from the end — has
/// nothing to work with, which is not visible in any other number: the
/// entries chain to each other perfectly whether or not a marker exists.
pub const CHECKPOINTED: &str = "ordo_secevent_checkpoints_total";

/// How often the drainer looks for work.
///
/// Half the collapse window ([`ordo_secevent::DEFAULT_WINDOW_SECS`]), so a
/// closed window waits at most one window to be written rather than up to
/// two. Not shorter: a tick that finds nothing still costs a lock
/// acquisition on the recorder, and the entries it would find sooner are
/// summaries of the last sixty seconds either way.
pub const DEFAULT_TICK: Duration = Duration::from_secs(30);

/// How many entries go into one transaction.
///
/// The whole batch cannot: a denial storm can hand the drainer tens of
/// thousands of entries in one pass, and one transaction holding every
/// scope's append lock while it writes them is a long lock on an audit
/// table. Chunking bounds the lock hold and bounds what a single failure
/// puts back on the outbox.
pub const APPEND_BATCH: usize = 256;

/// How often retention runs, when it runs at all.
pub const DEFAULT_RETENTION_TICK: Duration = Duration::from_secs(6 * 60 * 60);

/// How often a stream checkpoint is signed.
///
/// `ordo_exec::secevent`'s own number, not a second one: that constant's doc
/// carries the argument for why the marker's cadence is the *attestation*
/// cadence and not the retention cadence, and a deployment reading this
/// module should be sent there rather than handed a duplicate.
pub const DEFAULT_CHECKPOINT_TICK: Duration = ordo_exec::secevent::DEFAULT_CHECKPOINT_PERIOD;

/// What the drain has done and what it last failed to do.
///
/// Every field is atomic or behind its own short-lived lock, because the
/// reader is `/health/deep` on a request thread and it must never wait on
/// the drain task.
#[derive(Debug, Default)]
pub struct DrainHealth {
    appended: AtomicU64,
    failures: AtomicU64,
    pruned: AtomicU64,
    /// Stream checkpoints signed. Its own counter beside `pruned`, because
    /// the two are now driven by two schedules and a deployment must be able
    /// to see that one of them has stopped without the other.
    checkpointed: AtomicU64,
    /// Unix seconds of the last pass that committed something, or 0.
    last_success: AtomicU64,
    /// Unix seconds of the first failure of the current failing streak, or
    /// 0 when the last pass succeeded. This is the value that answers "how
    /// long has this been broken", which is the only question an operator
    /// asks about a background writer.
    failing_since: AtomicU64,
    last_error: Mutex<Option<StoreFault>>,
    /// Why retention is not running, when it is not.
    retention_note: Mutex<Option<String>>,
    /// Why nothing is anchoring the end of the stream, when nothing is —
    /// either because no key is configured or because the last pass refused
    /// a scope. Kept apart from [`DrainHealth::retention_note`] for the
    /// reason the two schedules are kept apart: "we are keeping everything
    /// on purpose" and "nothing has signed the head for a week" are
    /// different operational facts and one field would show only the last
    /// one written.
    checkpoint_note: Mutex<Option<String>>,
    /// The checkpoint key's id and public half (hex), and where it came
    /// from, when there is one: what a verifier sets as
    /// `ORDO_SECURITY_CHECKPOINT_KEY_ID` and `ORDO_SECURITY_CHECKPOINT_PUBKEY`.
    checkpoint_key: Mutex<Option<(String, String, &'static str)>>,
}

impl DrainHealth {
    /// How long the drain may fail before `/health/deep` calls the
    /// deployment degraded.
    ///
    /// Ten minutes: long enough that a failover, a `pg_bouncer` restart or
    /// a brief network partition does not colour the health surface, short
    /// enough that a real outage is degraded well before the outbox bound
    /// is anywhere near being reached.
    pub const DEGRADED_SECS: u64 = 600;

    fn ok(&self, at: u64, appended: usize) {
        self.appended.fetch_add(appended as u64, Ordering::Relaxed);
        self.last_success.store(at, Ordering::Relaxed);
        self.failing_since.store(0, Ordering::Relaxed);
        *self.last_error.lock().expect("drain health") = None;
    }

    fn failed(&self, at: u64, error: StoreFault) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        // Only the *first* failure of a streak sets the clock, so a
        // ten-minute outage reads as ten minutes rather than as thirty
        // seconds twenty times over.
        let _ = self
            .failing_since
            .compare_exchange(0, at, Ordering::Relaxed, Ordering::Relaxed);
        *self.last_error.lock().expect("drain health") = Some(error);
    }

    pub fn appended(&self) -> u64 {
        self.appended.load(Ordering::Relaxed)
    }

    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    pub fn pruned(&self) -> u64 {
        self.pruned.load(Ordering::Relaxed)
    }

    pub fn checkpointed(&self) -> u64 {
        self.checkpointed.load(Ordering::Relaxed)
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_fault().map(|f| f.reason().to_string())
    }

    /// The class of the last failure: busy and broken are told apart here,
    /// not by reading `last_error`.
    pub fn last_fault(&self) -> Option<StoreFault> {
        self.last_error.lock().expect("drain health").clone()
    }

    pub fn retention_note(&self) -> Option<String> {
        self.retention_note.lock().expect("drain health").clone()
    }

    pub fn checkpoint_note(&self) -> Option<String> {
        self.checkpoint_note.lock().expect("drain health").clone()
    }

    /// The checkpoint key a verifier needs: `(key id, public key hex,
    /// source)`. `None` when nothing signs checkpoints.
    pub fn checkpoint_key(&self) -> Option<(String, String, &'static str)> {
        self.checkpoint_key.lock().expect("drain health").clone()
    }

    /// Seconds the drain has been failing without a success, or `None` when
    /// the last pass committed.
    pub fn failing_for(&self, now: u64) -> Option<u64> {
        match self.failing_since.load(Ordering::Relaxed) {
            0 => None,
            since => Some(now.saturating_sub(since)),
        }
    }

    /// Has the drain been down long enough to colour the deployment's
    /// health? See [`DrainHealth::DEGRADED_SECS`], and this module's doc
    /// for why this is `/health/deep` and never `/health/ready`.
    pub fn degraded(&self, now: u64) -> bool {
        self.failing_for(now)
            .is_some_and(|secs| secs >= DrainHealth::DEGRADED_SECS)
    }
}

/// How the drain is configured. Every field has a default that a
/// deployment can serve on.
#[derive(Debug, Clone)]
pub struct DrainConfig {
    pub tick: Duration,
    pub retention_tick: Duration,
    /// How often a stream checkpoint is signed. **Its own period**, not the
    /// retention one: see `ordo_exec::secevent::SECEVENT_CHECKPOINT_SCHEDULE`
    /// for the two reasons, of which the shorter is that six hours is a
    /// reasonable retention sweep and a poor bound on how long a deleted
    /// tail may go undetected.
    pub checkpoint_tick: Duration,
    pub retention: Retention,
    /// Run the retention pass at all. Off when the deployment has no
    /// security checkpoint key, because a cut point that nothing signed is
    /// not a legal cut point — see [`SecurityDrain::start`].
    pub retain: bool,
    /// Leave the security stream's **cadences** — the checkpoint pass and
    /// the retention pass — to R3's `secevent.checkpoint` and
    /// `secevent.retain` workers instead of running them from the drain
    /// loop. The drain still registers both schedules and holds the signer;
    /// [`SecurityDrain::checkpointer`] and [`SecurityDrain::retainer`] hand
    /// them to the workers. Off by default, so a drain started on its own
    /// (every test, and any embedding with no supervisor) keeps doing both
    /// itself.
    ///
    /// The name says `retention` because retention was the only pass when
    /// it landed. It has always meant "somebody else drives this drain's
    /// scheduled work", and the checkpoint pass is scheduled work of the
    /// same kind, so it is governed by the same flag rather than by a second
    /// one an embedder could set inconsistently — two loops over one
    /// schedule row is the bug both flags would exist to prevent.
    pub supervised_retention: bool,
    /// The checkpoint key. [`CheckpointSigner::FromEnv`] by default, which
    /// reads `ORDO_SECURITY_CHECKPOINT_KEY` as the drain always did; the
    /// runtime passes the key `security_key::resolve` settled on, which is
    /// the set one or the one made at first boot.
    pub checkpoint_signer: CheckpointSigner,
}

/// Where the drain's checkpoint signer comes from.
#[derive(Clone, Default)]
pub enum CheckpointSigner {
    /// `ordo_secevent::security_signer_from_env`.
    #[default]
    FromEnv,
    /// Resolved by the caller, with the word for where it came from.
    Resolved(Arc<Ed25519Signer>, &'static str),
    /// The caller found none, and why.
    Unavailable(String),
}

impl std::fmt::Debug for CheckpointSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointSigner::FromEnv => f.write_str("FromEnv"),
            CheckpointSigner::Resolved(s, source) => write!(
                f,
                "Resolved({}, {source})",
                ordo_commandlog::Signer::key_id(s.as_ref())
            ),
            CheckpointSigner::Unavailable(why) => write!(f, "Unavailable({why})"),
        }
    }
}

impl Default for DrainConfig {
    fn default() -> DrainConfig {
        let mut retention = Retention::default();
        // `plan-opslog.md` §1.4: the deployment stream's own retention
        // number, independent of every tenant's. Unset keeps the same
        // ninety days `Retention::default()` already gives it, so landing
        // this read changes nothing until a deployment opts in.
        if let Some(days) = Retention::deployment_days_from_env() {
            retention.deployment_keep_secs = days * 24 * 60 * 60;
        }
        DrainConfig {
            tick: DEFAULT_TICK,
            retention_tick: DEFAULT_RETENTION_TICK,
            checkpoint_tick: DEFAULT_CHECKPOINT_TICK,
            retention,
            retain: true,
            supervised_retention: false,
            checkpoint_signer: CheckpointSigner::FromEnv,
        }
    }
}

/// The running drain. Dropping it stops the task; [`SecurityDrain::shutdown`]
/// stops it *and* writes what is left.
pub struct SecurityDrain {
    stop: Arc<AtomicBool>,
    /// Wakes the loop out of its tick sleep so a shutdown does not wait a
    /// whole tick, and never has to abort a pass to be prompt.
    wake: Arc<tokio::sync::Notify>,
    health: Arc<DrainHealth>,
    recorder: Arc<Recorder>,
    router: Arc<Router>,
    metrics: Arc<Metrics>,
    task: Option<tokio::task::JoinHandle<()>>,
    /// Present when retention is active and handed to the supervisor
    /// (`DrainConfig::supervised_retention`).
    retainer: Option<Retainer>,
    /// Present when a security signing key is configured and the passes are
    /// handed to the supervisor. **Not** gated on `retain`: see
    /// [`SecurityDrain::checkpointer`].
    checkpointer: Option<Checkpointer>,
}

impl SecurityDrain {
    /// Start draining `recorder` into `pool`'s `$secevents`.
    ///
    /// Retention is enabled only when the deployment has set
    /// `ORDO_SECURITY_CHECKPOINT_KEY` / `_KEY_ID`. That is not a
    /// convenience: retention drops a whole prefix ending at a **verified**
    /// checkpoint, so with no signing key there is no lawful cut point and
    /// the honest behaviour is to keep everything and say so. The reason
    /// lands in [`DrainHealth::retention_note`] and on `/health/deep`,
    /// because a stream that grows forever because a variable is unset is
    /// exactly the kind of thing nobody discovers until the disk fills.
    pub async fn start(
        recorder: Arc<Recorder>,
        pool: Pool,
        metrics: Arc<Metrics>,
        cfg: DrainConfig,
    ) -> Result<SecurityDrain, PoolError> {
        let pools = Arc::new(SinglePool::new(pool.clone()));
        SecurityDrain::start_routed(recorder, pool, pools, metrics, cfg).await
    }

    /// [`SecurityDrain::start`], with each space's entries written to the
    /// database `pools` resolves that space to.
    ///
    /// `control` is where the deployment stream goes, and it is reached
    /// without the resolver: a caller can name a space that does not exist,
    /// so the deployment stream has to be writable without opening any
    /// target (`plan-space-routing.md` §3). A space's own entries go where
    /// the rest of that space's history is.
    ///
    /// Retention walks the control database and every database `pools`
    /// lists ([`retain_everywhere`]), so a space on an attached database is
    /// retained where its stream is. So does the checkpoint pass
    /// ([`checkpoint_everywhere`]), and for the sharper version of the same
    /// reason: a tenant's stream on an unwalked database would accumulate
    /// entries with nothing signed above them, and `ordo verify` would
    /// report that tenant's stream green while unable to tell whether its
    /// last week had been deleted.
    pub async fn start_routed(
        recorder: Arc<Recorder>,
        control: Pool,
        pools: Arc<dyn PoolResolver>,
        metrics: Arc<Metrics>,
        cfg: DrainConfig,
    ) -> Result<SecurityDrain, PoolError> {
        let pool = control;
        let router = Arc::new(Router {
            control: pool.clone(),
            pools,
        });
        let health = Arc::new(DrainHealth::default());
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());

        // One signer, shared by both passes: `Ed25519Signer` is
        // deliberately not `Clone` (it holds the seed), so the `Arc` is made
        // once here rather than each pass reading the environment again —
        // two reads is two chances to disagree about which key this
        // deployment signs with.
        let (signing, source): (Result<Arc<Ed25519Signer>, String>, &'static str) =
            match cfg.checkpoint_signer.clone() {
                CheckpointSigner::FromEnv => (
                    ordo_secevent::security_signer_from_env()
                        .map(Arc::new)
                        .map_err(|e| e.to_string()),
                    "set",
                ),
                CheckpointSigner::Resolved(s, source) => (Ok(s), source),
                CheckpointSigner::Unavailable(why) => (Err(why), ""),
            };
        let why_no_key = signing.as_ref().err().cloned();
        let signing: Option<Arc<Ed25519Signer>> = signing.ok();
        if let Some(s) = &signing {
            *health.checkpoint_key.lock().expect("drain health") = Some((
                ordo_commandlog::Signer::key_id(s.as_ref()).to_string(),
                s.verifier().public_hex(),
                source,
            ));
        }
        let retain = cfg.retain && signing.is_some();
        if !retain {
            let why = match (&cfg.retain, &why_no_key) {
                (false, _) => {
                    "retention is disabled by this deployment's configuration".to_string()
                }
                (_, Some(e)) => format!(
                    "retention is INACTIVE: {e}. Retention cuts at a signed checkpoint, so \
                     with no security signing key there is no lawful cut point and nothing \
                     is ever removed — the stream grows without bound until \
                     `{}` and `{}` are set",
                    ordo_secevent::SECURITY_KEY_ENV,
                    ordo_secevent::SECURITY_KEY_ID_ENV
                ),
                _ => String::new(),
            };
            *health.retention_note.lock().expect("drain health") = Some(why);
        }
        // **The checkpoint pass is gated on the key alone, never on
        // `retain`.** Retention is a storage policy and a deployment is
        // entitled to switch it off; the marker at the head of the stream is
        // the only thing that can show entries were deleted from the end, and
        // switching *that* off is not a storage choice. Before the two passes
        // were separated, `retain: false` silently disabled truncation
        // detection, which is the more consequential of the two settings
        // being decided by the less consequential one.
        match (&signing, &why_no_key) {
            (Some(_), _) => {
                ordo_exec::secevent::register_checkpoint_schedule(
                    &pool,
                    cfg.checkpoint_tick,
                    now_secs() as i64,
                )
                .await?;
            }
            (None, Some(e)) => {
                *health.checkpoint_note.lock().expect("drain health") = Some(format!(
                    "checkpointing is INACTIVE: {e}. Nothing signs the head of any security \
                     stream, so `ordo verify` recomputes the entries against each other and \
                     cannot tell whether any was removed from the END — the one property a \
                     chain walk structurally cannot have. Set `{}` and `{}`",
                    ordo_secevent::SECURITY_KEY_ENV,
                    ordo_secevent::SECURITY_KEY_ID_ENV
                ));
            }
            (None, None) => {}
        }
        if retain {
            ordo_exec::secevent::register_retention_schedule(
                &pool,
                cfg.retention_tick,
                now_secs() as i64,
            )
            .await?;
        }

        let signer = signing.clone().filter(|_| retain);
        let retainer = match (&signer, cfg.supervised_retention) {
            (Some(s), true) => Some(Retainer {
                pool: pool.clone(),
                router: router.clone(),
                metrics: metrics.clone(),
                health: health.clone(),
                signer: s.clone(),
                retention: cfg.retention,
                period: cfg.retention_tick,
            }),
            _ => None,
        };
        let checkpointer = match (&signing, cfg.supervised_retention) {
            (Some(s), true) => Some(Checkpointer {
                pool: pool.clone(),
                router: router.clone(),
                metrics: metrics.clone(),
                health: health.clone(),
                signer: s.clone(),
                period: cfg.checkpoint_tick,
            }),
            _ => None,
        };
        let task = tokio::spawn(drain_loop(DrainLoop {
            recorder: recorder.clone(),
            router: router.clone(),
            metrics: metrics.clone(),
            health: health.clone(),
            stop: stop.clone(),
            wake: wake.clone(),
            pool,
            // The loop runs each pass only when nobody else will.
            signer: signer.filter(|_| !cfg.supervised_retention),
            checkpoint_signer: signing.filter(|_| !cfg.supervised_retention),
            cfg,
        }));

        Ok(SecurityDrain {
            stop,
            wake,
            health,
            recorder,
            router,
            metrics,
            task: Some(task),
            retainer,
            checkpointer,
        })
    }

    /// The retention pass, for R3's `secevent.retain` worker to drive.
    ///
    /// `None` when retention is inactive (no security signing key, or
    /// `retain: false`) or when the drain was started to retain by itself.
    pub fn retainer(&self) -> Option<Retainer> {
        self.retainer.clone()
    }

    /// The checkpoint pass, for R3's `secevent.checkpoint` worker to drive.
    ///
    /// `None` when no security signing key is configured, or when the drain
    /// was started to run its own cadences. **Independent of `retain`**: a
    /// deployment that keeps every denial forever still needs the head
    /// signed, because keeping everything is not the same as being able to
    /// show that everything was kept.
    pub fn checkpointer(&self) -> Option<Checkpointer> {
        self.checkpointer.clone()
    }

    /// What the drain has done — for `/health/deep`, for tests, and for an
    /// operator who wants to know whether the record is being written.
    pub fn health(&self) -> Arc<DrainHealth> {
        self.health.clone()
    }

    /// Stop the loop and write everything that is left, including windows
    /// that have not elapsed.
    ///
    /// This is the difference between a planned restart that loses nothing
    /// and one that loses up to a window of denials. It is fallible and it
    /// returns the fault rather than swallowing it: a shutdown that could
    /// not flush is the last chance anyone has to know.
    ///
    /// The loop is joined, never aborted: a pass in flight holds drained
    /// entries, and aborting it would drop them (see the module doc,
    /// *Shutdown drains*). The wait is bounded by one pass, which the
    /// store's own statement timeouts bound in turn.
    pub async fn shutdown(mut self) -> Result<usize, StreamDrainError> {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.notify_one();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        let now = Timestamp(now_secs());
        let entries = self.recorder.drain_all(now);
        append_batched(
            &self.router,
            &self.recorder,
            &self.metrics,
            &self.health,
            entries,
            now_secs(),
        )
        .await
    }
}

impl Drop for SecurityDrain {
    /// Stops the loop. It deliberately does **not** flush: a `Drop` cannot
    /// await, and a blocking flush inside one would deadlock any caller
    /// dropping this from a runtime thread. A deployment that wants the
    /// last window written calls [`SecurityDrain::shutdown`].
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// A drain pass that could not commit everything, with what it managed.
#[derive(Debug, Clone)]
pub struct StreamDrainError {
    /// Entries written before the failure. They are durable; the rest went
    /// back on the outbox.
    pub appended: usize,
    pub error: StoreFault,
}

impl StreamDrainError {
    /// What a caller can do about this failure.
    pub fn class(&self) -> FaultClass {
        self.error.class()
    }
}

impl std::fmt::Display for StreamDrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "security-stream drain wrote {} entr{} and then failed: {}",
            self.appended,
            if self.appended == 1 { "y" } else { "ies" },
            self.error
        )
    }
}

impl std::error::Error for StreamDrainError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

struct DrainLoop {
    recorder: Arc<Recorder>,
    router: Arc<Router>,
    metrics: Arc<Metrics>,
    health: Arc<DrainHealth>,
    stop: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    pool: Pool,
    cfg: DrainConfig,
    signer: Option<Arc<Ed25519Signer>>,
    /// The same key, held separately because the two passes are enabled
    /// separately: `signer` is `None` when retention is off and this one is
    /// not.
    checkpoint_signer: Option<Arc<Ed25519Signer>>,
}

async fn drain_loop(l: DrainLoop) {
    loop {
        // The sleep is the only point a shutdown interrupts. Everything
        // after it runs to the end of the pass, so entries drained from
        // the recorder are always either written or requeued.
        tokio::select! {
            _ = tokio::time::sleep(l.cfg.tick) => {}
            _ = l.wake.notified() => {}
        }
        if l.stop.load(Ordering::Relaxed) {
            return;
        }
        let at = now_secs();
        let entries = l.recorder.drain_ready(Timestamp(at));
        let _ = append_batched(&l.router, &l.recorder, &l.metrics, &l.health, entries, at).await;

        // The counters that answer "is anything being lost", published every
        // pass whether or not there was work. A gauge-shaped truth in a
        // counter-only registry: these are lifetime totals from the
        // recorder, so the registry is *set* to them rather than
        // incremented, which `Metrics::set` is for.
        l.metrics.set(OBSERVED, &[], l.recorder.observed());
        l.metrics.set(DROPPED, &[], l.recorder.dropped());

        // Checkpointing first, and on its own schedule row. The order
        // matters on a tick where both are due: a marker at the head is what
        // makes the prefix underneath it prunable, so signing before cutting
        // is the same argument `retain_scope`'s step order already makes,
        // one level up.
        if let Some(signer) = &l.checkpoint_signer {
            let _ = checkpoint_once(
                &l.pool,
                &l.router,
                &l.metrics,
                &l.health,
                signer,
                l.cfg.checkpoint_tick,
                at,
            )
            .await;
        }
        if let Some(signer) = &l.signer {
            retention_pass(&l, signer, at).await;
        }
    }
}

/// One retention pass, if this replica claims the tick.
///
/// `claim_schedule` on the security stream's own row — never `claim_due`,
/// which would advance the command log's schedules by this period. The
/// leadership property is the job queue's: `FOR UPDATE SKIP LOCKED` means a
/// second replica examining the same row at the same instant sees nothing
/// to claim, so a fleet runs one pass per period between them.
async fn retention_pass(l: &DrainLoop, signer: &Ed25519Signer, at: u64) {
    let _ = retain_once(
        &l.pool,
        &l.router,
        &l.metrics,
        &l.health,
        signer,
        l.cfg.retention,
        l.cfg.retention_tick,
        at,
    )
    .await;
}

// ── the checkpoint pass: what finally signs a stream marker ──────────────
//
// The gap this closes, stated plainly, because it is easy to miss how narrow
// and how total it was. `ordo_secevent` has had signed stream checkpoints
// under their own key since 7a, and `ordo verify` has checked them since
// 7d(a). The fourth of `audit_stream`'s checks — *nothing was removed from
// the end of the stream* — is the only one a chain walk structurally cannot
// perform, because deleting the last N entries leaves a perfectly
// self-consistent chain and only a signed marker at a higher position knows
// they were ever there.
//
// And the only thing that had ever signed one in a running deployment was
// step 1 of `retain_scope`. So the reachability of check 4 depended on
// retention being switched on, its latency was the retention period, and it
// signed without verifying. Three separate ways for the one property the
// separately-keyed artifact exists to provide to be absent, none of them
// visible in any counter.
//
// This pass is its own schedule row, its own period, its own enabling
// condition, and its own counter.

/// The checkpoint pass behind R3's `secevent.checkpoint` worker: the same
/// claim and the same sweep the drain loop runs when it signs by itself,
/// with the same health counters, so `/health/deep` reads the same whichever
/// of the two drove it.
#[derive(Clone)]
pub struct Checkpointer {
    pool: Pool,
    router: Arc<Router>,
    metrics: Arc<Metrics>,
    health: Arc<DrainHealth>,
    signer: Arc<Ed25519Signer>,
    period: Duration,
}

impl Checkpointer {
    /// A checkpointer over one database with a signer the caller holds. The
    /// drain builds its own from the environment; this exists so a test can
    /// drive the worker without setting `ORDO_SECURITY_CHECKPOINT_KEY`.
    pub fn new(
        pool: Pool,
        metrics: Arc<Metrics>,
        health: Arc<DrainHealth>,
        signer: Arc<Ed25519Signer>,
        period: Duration,
    ) -> Checkpointer {
        Checkpointer {
            router: Arc::new(Router {
                control: pool.clone(),
                pools: Arc::new(SinglePool::new(pool.clone())),
            }),
            pool,
            metrics,
            health,
            signer,
            period,
        }
    }

    /// Claim the checkpoint tick and, if this replica won it, sign every
    /// scope's head. `None` is "not due, or another replica claimed it";
    /// `Some(n)` is markers signed.
    pub async fn pass(&self, at: u64) -> Result<Option<u64>, PoolError> {
        checkpoint_once(
            &self.pool,
            &self.router,
            &self.metrics,
            &self.health,
            &self.signer,
            self.period,
            at,
        )
        .await
    }
}

async fn checkpoint_once(
    pool: &Pool,
    router: &Router,
    metrics: &Metrics,
    health: &DrainHealth,
    signer: &Ed25519Signer,
    period: Duration,
    at: u64,
) -> Result<Option<u64>, PoolError> {
    if !ordo_exec::secevent::claim_checkpoint_tick(pool, period, at as i64).await? {
        return Ok(None);
    }
    let sweep = checkpoint_everywhere(pool, router.pools.as_ref(), signer, Timestamp(at)).await;
    if sweep.signed > 0 {
        health
            .checkpointed
            .fetch_add(sweep.signed, Ordering::Relaxed);
        metrics.set(CHECKPOINTED, &[], health.checkpointed());
    }
    // A refusal is the one thing here an operator must see, and it is not an
    // error: `checkpoint_scope` returns a refusal for every state in which
    // signing would make things worse, and each of those states is a finding
    // about the stream rather than a fault in this pass. The last one wins
    // the note for the same reason the retention note works that way, and
    // the counter above stays flat, which is the durable signal.
    if let Some(note) = sweep.notes.last() {
        *health.checkpoint_note.lock().expect("drain health") = Some(note.clone());
    } else if sweep.signed > 0 {
        *health.checkpoint_note.lock().expect("drain health") = None;
    }
    Ok(Some(sweep.signed))
}

/// What one checkpoint pass over every database did.
#[derive(Debug, Default)]
pub struct CheckpointSweep {
    /// Markers signed, across every database.
    pub signed: u64,
    /// Scopes whose head was already signed, as `database: scope`. Named
    /// rather than counted with the signed ones: a quiet stream and a stream
    /// that was just anchored are different states and only one of them
    /// moved.
    pub unchanged: Vec<String>,
    /// Why a scope or a database was not signed, one line each. Every entry
    /// here is a refusal from `ordo_exec::secevent::CheckpointRefusal` or a
    /// database that could not be reached, and both are operator-facing.
    pub notes: Vec<String>,
}

/// One checkpoint pass over the control database and every database a space
/// can live in.
///
/// The same walk [`retain_everywhere`] makes, through the same
/// [`databases`] helper, and for a reason that is *stronger* here than
/// there. A database retention missed keeps too much history, which is a
/// cost. A database this pass misses has streams nothing signs, so
/// `ordo verify` on any of its tenants reports a green chain walk it cannot
/// back with a truncation check — the failure is silent and it is in the
/// answer rather than in the storage.
///
/// Public so a test can drive a pass with a seeded signer.
pub async fn checkpoint_everywhere(
    control: &Pool,
    pools: &dyn PoolResolver,
    signer: &Ed25519Signer,
    now: Timestamp,
) -> CheckpointSweep {
    let handle = tokio::runtime::Handle::current();
    let mut sweep = CheckpointSweep::default();
    let databases = databases(control, pools, &mut sweep.notes).await;
    for (label, pool) in databases {
        let store = PgSecurityStore::new(pool, handle.clone());
        let scopes = match store.try_scopes().await {
            Ok(scopes) => scopes,
            Err(e) => {
                sweep
                    .notes
                    .push(format!("{label}: its scopes could not be read: {e}"));
                continue;
            }
        };
        for scope in scopes {
            use ordo_exec::secevent::StreamCheckpointed;
            match ordo_exec::secevent::checkpoint_scope(&store, &scope, signer, now).await {
                Ok(StreamCheckpointed::Signed(_)) => sweep.signed += 1,
                Ok(StreamCheckpointed::AlreadyAt(_)) => {
                    sweep.unchanged.push(format!("{label}: {}", scope.key()))
                }
                // Not a note. `try_scopes` lists scopes with a stored entry,
                // so an empty one is a scope whose every entry a lawful
                // prune could not have removed — except that a lawful prune
                // cannot empty a stream, which means `audit_stream`'s check 1
                // has already failed it. Saying it twice, here, would put a
                // second voice on a verdict the verifier owns.
                Ok(StreamCheckpointed::EmptyStream) => {}
                Ok(StreamCheckpointed::Refused(why)) => {
                    sweep.notes.push(format!("{label}: {}: {why}", scope.key()))
                }
                Err(e) => sweep.notes.push(format!(
                    "{label}: {}: checkpoint pass failed: {e}",
                    scope.key()
                )),
            }
        }
    }
    sweep
}

/// The control database and every database a space can live in, labelled.
///
/// Shared by [`checkpoint_everywhere`] and [`retain_everywhere`] because the
/// *list* is the same list and a second copy of it would be a second answer
/// to "where do this deployment's streams live" — the shape in which one
/// pass walks a target the other does not. The control database is always
/// first and is always walked, whatever the resolver reports: it holds the
/// deployment's own stream. A target the resolver could not reach is
/// recorded in `notes` rather than skipped, because a pass that left a
/// database out silently would report work it did not do.
async fn databases(
    control: &Pool,
    pools: &dyn PoolResolver,
    notes: &mut Vec<String>,
) -> Vec<(String, Pool)> {
    let mut databases: Vec<(String, Pool)> = vec![("control".to_string(), control.clone())];
    match pools.targets().await {
        Ok(targets) => {
            for t in targets {
                match t.pool {
                    Ok(p) if databases.iter().any(|(_, q)| q.same_handle(&p)) => {}
                    Ok(p) => databases.push((t.label, p)),
                    Err(e) => notes.push(format!("{}: not reached, so not swept: {e}", t.label)),
                }
            }
        }
        Err(e) => notes.push(format!(
            "the list of databases could not be read, so only the control database was swept: {e}"
        )),
    }
    databases
}

/// The retention pass behind R3's `secevent.retain` worker: the same claim
/// and the same sweep the drain loop runs when it retains by itself, with
/// the same health counters, so `/health/deep`'s `security_stream` block
/// reads the same whichever of the two drove it.
#[derive(Clone)]
pub struct Retainer {
    pool: Pool,
    router: Arc<Router>,
    metrics: Arc<Metrics>,
    health: Arc<DrainHealth>,
    signer: Arc<Ed25519Signer>,
    retention: Retention,
    period: Duration,
}

impl Retainer {
    /// A retainer over one database with a signer the caller holds. The
    /// drain builds its own from the environment; this exists so a test can
    /// drive the worker without setting `ORDO_SECURITY_CHECKPOINT_KEY`.
    pub fn new(
        pool: Pool,
        metrics: Arc<Metrics>,
        health: Arc<DrainHealth>,
        signer: Arc<Ed25519Signer>,
        retention: Retention,
        period: Duration,
    ) -> Retainer {
        Retainer {
            router: Arc::new(Router {
                control: pool.clone(),
                pools: Arc::new(SinglePool::new(pool.clone())),
            }),
            pool,
            metrics,
            health,
            signer,
            retention,
            period,
        }
    }

    /// Claim the retention tick and, if this replica won it, sweep. `None`
    /// is "not due, or another replica claimed it"; `Some(n)` is entries
    /// pruned plus scopes checkpointed.
    pub async fn pass(&self, at: u64) -> Result<Option<u64>, PoolError> {
        retain_once(
            &self.pool,
            &self.router,
            &self.metrics,
            &self.health,
            &self.signer,
            self.retention,
            self.period,
            at,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
async fn retain_once(
    pool: &Pool,
    router: &Router,
    metrics: &Metrics,
    health: &DrainHealth,
    signer: &Ed25519Signer,
    retention: Retention,
    period: Duration,
    at: u64,
) -> Result<Option<u64>, PoolError> {
    if !ordo_exec::secevent::claim_retention_tick(pool, period, at as i64).await? {
        return Ok(None);
    }
    // The signer's own public half. This is the *deployment* verifying its
    // own checkpoint before destroying the history under it, not a third
    // party verifying a published one — a third party supplies
    // `ORDO_SECURITY_CHECKPOINT_PUBKEY` and never reaches this path. The
    // check is still worth making: it is what stops a `$seccheckpoints`
    // row someone inserted by hand from becoming a lawful cut point.
    let verifier = signer.verifier();
    let sweep = retain_everywhere(
        pool,
        router.pools.as_ref(),
        signer,
        &verifier,
        retention,
        Timestamp(at),
    )
    .await;
    if sweep.pruned > 0 {
        health.pruned.fetch_add(sweep.pruned, Ordering::Relaxed);
        metrics.set(PRUNED, &[], health.pruned());
    }
    // A refused cut is not an error ("nothing has aged out yet" is the
    // ordinary answer), but it is the reason retention is not reclaiming, so
    // an operator gets to read it rather than infer it from a flat counter.
    if let Some(note) = sweep.notes.last() {
        *health.retention_note.lock().expect("drain health") = Some(note.clone());
    }
    Ok(Some(sweep.pruned + sweep.checkpointed.len() as u64))
}

/// What one retention pass over every database did.
#[derive(Debug, Default)]
pub struct RetentionSweep {
    /// Entries removed, across every database.
    pub pruned: u64,
    /// Scopes that took a new checkpoint this pass, as `database: scope`.
    pub checkpointed: Vec<String>,
    /// Why a scope or a database was not reclaimed, one line each.
    pub notes: Vec<String>,
}

/// One retention pass over the control database and every database a space
/// can live in.
///
/// Since S4 a space's stream is written to its own database, so retention
/// has to go there too, or an attached space would keep its whole stream.
/// The control database is always walked, and first: it holds the
/// deployment's stream, which must be retained whatever the resolver
/// reports. Each other database is walked once, however many spaces share
/// it. One the resolver could not reach is recorded in `notes` rather than
/// skipped, because a pass that left a database out silently would report
/// retention it did not do.
///
/// Public so a test can drive a pass with a seeded signer. The drain calls it
/// only after claiming the deployment's retention tick, so a fleet still runs
/// one pass per period between them.
pub async fn retain_everywhere(
    control: &Pool,
    pools: &dyn PoolResolver,
    signer: &Ed25519Signer,
    verifier: &ordo_commandlog::CheckpointVerifier,
    retention: Retention,
    now: Timestamp,
) -> RetentionSweep {
    let handle = tokio::runtime::Handle::current();
    let mut sweep = RetentionSweep::default();
    let databases = databases(control, pools, &mut sweep.notes).await;
    for (label, pool) in databases {
        let store = PgSecurityStore::new(pool, handle.clone());
        let scopes = match store.try_scopes().await {
            Ok(scopes) => scopes,
            Err(e) => {
                sweep
                    .notes
                    .push(format!("{label}: its scopes could not be read: {e}"));
                continue;
            }
        };
        for scope in scopes {
            match ordo_exec::secevent::retain_scope(
                &store, &scope, signer, verifier, retention, now,
            )
            .await
            {
                Ok(r) => {
                    sweep.pruned += r.pruned as u64;
                    if r.checkpointed.is_some() {
                        sweep.checkpointed.push(format!("{label}: {}", scope.key()));
                    }
                    if let Some(why) = r.refused {
                        sweep.notes.push(format!("{label}: {}: {why}", scope.key()));
                    }
                }
                Err(e) => sweep.notes.push(format!(
                    "{label}: {}: retention pass failed: {e}",
                    scope.key()
                )),
            }
        }
    }
    sweep
}

/// Where each entry is written.
///
/// Deployment-scope entries go to `control` and never touch the resolver.
/// Space-scope entries go to the database their space resolves to.
struct Router {
    control: Pool,
    pools: Arc<dyn PoolResolver>,
}

/// A destination: a pool, or the reason a space's pool could not be had.
type Destination = Result<Pool, StoreFault>;

fn same_destination(a: &Destination, b: &Destination) -> bool {
    match (a, b) {
        (Ok(x), Ok(y)) => x.same_handle(y),
        (Err(x), Err(y)) => x == y,
        _ => false,
    }
}

impl Router {
    /// Split `entries` by destination, keeping their order within each.
    ///
    /// Every entry of one scope lands in one group, because a scope is one
    /// space and a space resolves to one database; that is what lets a group
    /// fail on its own without reordering anyone's stream. Each space is
    /// resolved once per pass. A space whose database cannot be resolved is
    /// a group with no pool, and its entries go back on the outbox rather
    /// than anywhere else.
    async fn partition(&self, entries: Vec<StreamEntry>) -> Vec<(Destination, Vec<StreamEntry>)> {
        let mut groups: Vec<(Destination, Vec<StreamEntry>)> = Vec::new();
        let mut resolved: std::collections::HashMap<String, Destination> =
            std::collections::HashMap::new();
        for entry in entries {
            let dest = match &entry.scope {
                Scope::Deployment => Ok(self.control.clone()),
                Scope::Space(space) => match resolved.get(&space.0) {
                    Some(d) => d.clone(),
                    None => {
                        let d = self
                            .pools
                            .resolve(&ordo_crypto::SpaceId(space.0.clone()))
                            .await
                            .map_err(|e| {
                                StoreFault::caused_by(
                                    e.class(),
                                    format!("space `{}`: {e}", space.0),
                                    e,
                                )
                            });
                        resolved.insert(space.0.clone(), d.clone());
                        d
                    }
                },
            };
            match groups.iter().position(|(d, _)| same_destination(d, &dest)) {
                Some(i) => groups[i].1.push(entry),
                None => groups.push((dest, vec![entry])),
            }
        }
        groups
    }
}

/// Append each destination's entries in [`APPEND_BATCH`]-sized transactions,
/// and requeue what could not be written.
///
/// Split by destination first, then batched, and the order is the point. A
/// batch is one transaction, so it committed or it did not, and a
/// destination's suffix from its first failing batch is exactly what goes
/// back on the outbox: lossless and duplicate-free, as before routing. What
/// the split adds is that one destination failing does not stop the others,
/// and because a scope lives in one destination, requeuing that
/// destination's suffix never lets a later entry of the same scope be
/// written ahead of an earlier one.
async fn append_batched(
    router: &Router,
    recorder: &Recorder,
    metrics: &Metrics,
    health: &DrainHealth,
    entries: Vec<StreamEntry>,
    at: u64,
) -> Result<usize, StreamDrainError> {
    if entries.is_empty() {
        return Ok(0);
    }
    let handle = tokio::runtime::Handle::current();
    let mut written = 0usize;
    let mut held_back: Vec<StreamEntry> = Vec::new();
    let mut first_error: Option<StoreFault> = None;
    for (dest, group) in router.partition(entries).await {
        let pool = match dest {
            Ok(pool) => pool,
            Err(e) => {
                first_error.get_or_insert(e);
                held_back.extend(group);
                continue;
            }
        };
        let store = PgSecurityStore::new(pool, handle.clone());
        for (i, chunk) in group.chunks(APPEND_BATCH).enumerate() {
            match store.try_append_all(chunk).await {
                Ok(n) => {
                    written += n;
                    metrics.incr_by(APPENDED, &[], n as u64);
                }
                Err(e) => {
                    first_error.get_or_insert(StoreFault::caused_by(e.class(), e.to_string(), e));
                    held_back.extend_from_slice(&group[i * APPEND_BATCH..]);
                    break;
                }
            }
        }
    }
    let Some(error) = first_error else {
        health.ok(at, written);
        return Ok(written);
    };
    recorder.requeue(held_back);
    metrics.incr(APPEND_FAILURES, &[]);
    metrics.set(DROPPED, &[], recorder.dropped());
    // `ok` clears the failure streak, so a partial success is recorded first
    // and the failure last: an operator reading `failing_since` must see the
    // fault, not the half of the pass that worked.
    if written > 0 {
        health.ok(at, written);
    }
    health.failed(at, error.clone());
    Err(StreamDrainError {
        appended: written,
        error,
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
