//! R3 — the worker supervisor.
//!
//! Every loop *step* in this platform was built and tested, and none of
//! them was called from a long-running task: `drain_listener`, `fire_due`,
//! `claim_runs`, `fire_clock`, the checkpoint and attestation cadences, the
//! security-event retention pass, `prune`. This is the thing that calls
//! them.
//!
//! # The five obligations, and where each is discharged
//!
//! `plan-production-ops.md` names this supervisor three times without
//! defining it. Five obligations fall out of those three sentences:
//!
//! 1. **A worker is registered, not spawned inline** — [`Supervisor::start`]
//!    takes a set.
//! 2. **The supervisor owns the tick; the worker owns the cadence
//!    semantics.** K1 and M1 both name `claim_schedule` explicitly, and
//!    `claim_due` advances every due schedule by whichever period the
//!    *caller* passed — so two cadences through one call means the loser
//!    adopts the winner's. A supervisor that owned periods would
//!    reintroduce that bug one layer up. Hence [`Cadence::Schedule`]
//!    carries its own name and period and the loop claims per worker.
//! 3. **Chunked work must not be killed by a tick.** A worker gets a
//!    cancellation token and never a deadline, and [`Progress::more`] lets
//!    a chunked pass re-enter immediately instead of waiting out its
//!    cadence.
//! 4. **A worker may register a final act** — [`Worker::on_shutdown`], run
//!    between "workers cancelled" and "drain flushed".
//! 5. **Per-space where the work is per-space** — [`WorkerCtx`] carries the
//!    space.
//!
//! And three more this module adds:
//!
//! 6. **A worker panic must not take the door down** — and, less obviously,
//!    must not be *silent*. The default is worse than a crash: a panic in a
//!    spawned tokio task kills that task alone and nothing anywhere
//!    notices. A trigger runner that panicked once on a malformed payload
//!    would stop delivering forever while the door kept answering 200 and
//!    `/health/ready` kept saying ready.
//! 7. **Restart with backoff, and the restart is counted.**
//! 8. **A worker that is not running is named as not running** — in the
//!    startup log and on `/health/deep`, with the reason.
//!
//! # What this cannot deliver, said now rather than discovered later
//!
//! There is no routine *body* executor on any production path.
//! `Actor::Routine` has no producer, and the declaration forms are
//! checklist 12a. So R3 ships the **delivery loop** — event to batch to run
//! to claim to disposition — and a [`RunHandler`] seam where 12a's executor
//! plugs in. With no handler registered `triggers.runner` does not start,
//! and the census says why: a runner that started, claimed runs and
//! dead-lettered every one of them would look like a working trigger system
//! destroying work.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ordo_crypto::{KeyProvider, SpaceId};
use ordo_exec::jobs::Leadership;
use ordo_exec::triggers::{self, Run};
use ordo_exec::{Pool, PoolConfig};

use crate::deploy::Deployment;
use crate::metrics::Metrics;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ── metrics ──────────────────────────────────────────────────────────────

/// `ordo_worker_runs_total{worker,outcome}` — `ok` / `error` / `panic`.
pub const WORKER_RUNS: &str = "ordo_worker_runs_total";
/// `ordo_worker_restarts_total{worker,reason}` — obligation 7.
pub const WORKER_RESTARTS: &str = "ordo_worker_restarts_total";
/// `ordo_worker_units_total{worker}`.
///
/// Deliberately separate from `runs_total`: a drain that runs a thousand
/// times and moves zero units looks healthy on the first series and is the
/// actual failure. The same distinction `DrainHealth` already draws between
/// `appended` and `failures`.
pub const WORKER_UNITS: &str = "ordo_worker_units_total";
/// `ordo_trigger_deadletters_total{listener}`.
pub const TRIGGER_DEADLETTERS: &str = "ordo_trigger_deadletters_total";
/// `ordo_supervisor_leader{}`: 1 while this replica holds leadership, 0
/// while it does not. Set only by a supervisor that has a leader-only
/// worker to run, so an absent series is a replica that never competed.
pub const SUPERVISOR_LEADER: &str = "ordo_supervisor_leader";
/// `ordo_supervisor_leadership_changes_total{to}`, `to` being `leader` or
/// `follower`. A counter beside the gauge because a scrape interval can
/// miss a flap that lasted one leadership tick, and a flapping leader is
/// the fault the operator is looking for.
pub const SUPERVISOR_LEADERSHIP_CHANGES: &str = "ordo_supervisor_leadership_changes_total";

/// How many consecutive failures before a worker stops being restarted.
///
/// An infinite retry against a permanently broken dependency is a log flood
/// that hides every other fault, which is how an incident gets longer. And
/// stopping is **visible in the data**, not only in a log, and in the right
/// place: a quarantined `triggers.drain` stops advancing its listener
/// cursor, and a listener cursor pins the pruning floor and is
/// non-evictable by design — so a broken listener shows up as a log that
/// stops shrinking, arriving without anyone adding a second alarm.
/// At-least-once is preserved throughout: nothing is dropped, delivery is
/// paused.
pub const QUARANTINE_AFTER: u64 = 10;

// ── the contract ─────────────────────────────────────────────────────────

/// What a worker did, and whether there is more waiting.
///
/// `more` is what makes obligation 3 cheap: a chunked pass returns
/// `more: true` and the supervisor re-enters `run` immediately instead of
/// waiting out the tick, so a re-seal pass gets a throttle knob (`units`
/// per chunk) without an inner loop the supervisor cannot cancel.
#[derive(Debug, Default, Clone, Copy)]
pub struct Progress {
    pub units: u64,
    pub more: bool,
}

impl Progress {
    pub fn idle() -> Progress {
        Progress::default()
    }
    pub fn did(units: u64) -> Progress {
        Progress { units, more: false }
    }
}

/// How often a worker runs.
pub enum Cadence {
    /// Fixed interval, this replica's own clock.
    Tick(Duration),
    /// A named row in `$schedules`, claimed by `claim_schedule`. Obligation
    /// 2: the name and the period travel together, so two cadences can
    /// never advance one another's `next_due`.
    Schedule {
        name: &'static str,
        period: Duration,
    },
    /// Runs continuously; `run` blocks on its own source.
    Continuous,
}

/// A worker failed. One shape: the supervisor's policy is the same either
/// way (count, back off, quarantine), and a taxonomy nothing branches on is
/// a taxonomy that drifts.
#[derive(Debug)]
pub struct WorkerError(pub String);

impl<E: std::fmt::Display> From<E> for WorkerError {
    fn from(e: E) -> WorkerError {
        WorkerError(e.to_string())
    }
}

/// The supervisor's own clock: how long a failure backs off per unit of
/// `backoff_secs`, and how often leadership and the orphan census are
/// looked at again.
///
/// `backoff_unit` is one second in production, which makes the backoff
/// exactly `triggers::backoff_secs` and so the platform's one backoff. A
/// test shrinks the unit to reach the tenth consecutive failure in seconds
/// rather than in five minutes; the sequence and the threshold stay the
/// production ones.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub backoff_unit: Duration,
    /// §3.5's slow tick: re-acquisition while a follower, a liveness probe
    /// of the lock's own session while the leader.
    pub leader_tick: Duration,
    /// How often the orphaned-cursor census is re-read.
    pub census_every: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            backoff_unit: Duration::from_secs(1),
            leader_tick: Duration::from_secs(5),
            census_every: Duration::from_secs(60),
        }
    }
}

impl Timing {
    fn backoff(&self, attempts: u64) -> Duration {
        self.backoff_unit
            .saturating_mul(triggers::backoff_secs(attempts as i32) as u32)
    }
}

/// A loop the supervisor reports on and does not run.
///
/// It exists for `secevent.drain`, and the reason that drain is not a
/// [`Worker`] is on `start_supervisor` in [`crate::runtime`]: its guarantees
/// are about *when* it stops and flushes, and those are the two things a
/// supervised worker hands over to the supervisor. An external appears in
/// the census with `supervised: false` beside whatever its own health says.
#[derive(Clone)]
pub struct External {
    pub name: &'static str,
    pub health: Arc<dyn Fn() -> serde_json::Value + Send + Sync>,
}

/// Everything a worker is given.
///
/// It holds the `Arc<Deployment>` rather than a snapshot, so a worker reads
/// the **live** program — the same reason `PlatformState` holds the
/// deployment rather than the registry.
pub struct WorkerCtx {
    pub pool: Pool,
    pub keys: Arc<dyn KeyProvider>,
    pub deployment: Arc<Deployment>,
    pub metrics: Arc<Metrics>,
    pub space: SpaceId,
    pub cancel: Cancel,
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
    timing: Timing,
}

impl WorkerCtx {
    pub fn now(&self) -> u64 {
        (self.now)()
    }
}

/// One unit of supervised work.
///
/// A trait rather than a closure for three mechanical reasons: the name has
/// to be `'static` because it is a metric label; the cadence has to be
/// inspectable *before* the worker runs, for the startup log's
/// enabled/disabled census; and `on_shutdown` has to exist on every worker
/// without every registration site carrying a second closure it will forget
/// to supply.
pub trait Worker: Send + Sync + 'static {
    /// Stable. It is a metric label, a startup-log line and a
    /// `/health/deep` key.
    fn name(&self) -> &'static str;
    fn cadence(&self) -> Cadence;
    fn requires_leadership(&self) -> bool {
        false
    }
    /// One unit of work. Returns what it did and whether there is more —
    /// **never loops internally**, or the cancellation token cannot reach
    /// it.
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>>;
    /// Obligation 4. Runs once, between "workers cancelled" and "drain
    /// flushed".
    fn on_shutdown<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Cancellation, and never a deadline (obligation 3).
///
/// A deadline would kill a chunked pass mid-chunk; a token lets it finish
/// the unit it is on and stop. `tokio_util::sync::CancellationToken` is the
/// obvious type and would be a new dependency for two fields.
#[derive(Clone, Default)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
    /// Sleep, unless cancelled first.
    pub async fn sleep(&self, d: Duration) {
        if self.is_cancelled() {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(d) => {}
            _ = self.notify.notified() => {}
        }
    }
}

// ── the run handler seam ─────────────────────────────────────────────────

/// What executes a routine body. **12a's**, not R3's.
///
/// `Run::context(at)` already builds the `$trigger` a body would receive
/// and `triggers::reachable` already resolves `$trigger.ids` into the row
/// bound R-G20 requires. The handler is the one thing missing.
pub trait RunHandler: Send + Sync + 'static {
    fn handle<'a>(&'a self, run: &'a Run, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), String>>;
}

// ── per-worker status ────────────────────────────────────────────────────

/// What `/health/deep` reports for one worker.
///
/// **Atomics, not a `Mutex`** — deliberately. The supervisor catches
/// worker panics, and a panic while holding a `std::sync::Mutex` poisons
/// it; accounting behind one would let a caught worker fault break the
/// health surface that exists to report it. Panic-free by construction is
/// the only version of this that survives its own purpose.
#[derive(Default)]
pub struct WorkerStatus {
    pub name: &'static str,
    pub enabled: AtomicBool,
    /// Why it is not enabled, when it is not. `''` when it is.
    pub reason: std::sync::Mutex<String>,
    pub quarantined: AtomicBool,
    pub runs: AtomicU64,
    pub units: AtomicU64,
    pub failures: AtomicU64,
    /// Consecutive failures, reset by a run that succeeded and lasted
    /// longer than the current backoff.
    pub consecutive: AtomicU64,
    pub last_success_secs: AtomicU64,
    pub leader: AtomicBool,
}

impl WorkerStatus {
    fn reason(&self) -> String {
        self.reason
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    /// The `/health/deep` shape, mirroring the security drain's section —
    /// including the posture that **not running is a first-class value**.
    pub fn json(&self) -> serde_json::Value {
        let mut o = serde_json::json!({
            "enabled": self.enabled.load(Ordering::Relaxed),
            "runs": self.runs.load(Ordering::Relaxed),
            "units": self.units.load(Ordering::Relaxed),
            "failures": self.failures.load(Ordering::Relaxed),
            "quarantined": self.quarantined.load(Ordering::Relaxed),
        });
        let reason = self.reason();
        if !reason.is_empty() {
            o["reason"] = serde_json::Value::String(reason);
        }
        let last = self.last_success_secs.load(Ordering::Relaxed);
        if last > 0 {
            o["last_success_secs"] = serde_json::json!(last);
        }
        if self.leader.load(Ordering::Relaxed) {
            o["leader"] = serde_json::json!(true);
        }
        o
    }
}

// ── leadership, on a connection of its own ───────────────────────────────

/// What this replica knows about leadership, for `/health/deep` and the
/// metrics. Atomics for the reason [`WorkerStatus`] gives.
#[derive(Default)]
pub struct LeaderState {
    /// Whether any registered worker needs leadership. A supervisor with
    /// none never dials the lock, so a replacement replica never waits on
    /// it.
    pub wanted: bool,
    pub leader: AtomicBool,
    /// Transitions in either direction since start.
    pub changes: AtomicU64,
    /// Unix seconds of the last transition, or 0.
    pub since_secs: AtomicU64,
    /// Why the last attempt could not ask the question at all (the dial
    /// failed), as distinct from "another replica is leader". Empty when
    /// the last attempt got an answer.
    pub last_error: std::sync::Mutex<String>,
}

impl LeaderState {
    fn json(&self) -> serde_json::Value {
        if !self.wanted {
            return serde_json::json!({ "wanted": false });
        }
        let mut o = serde_json::json!({
            "wanted": true,
            "leader": self.leader.load(Ordering::Relaxed),
            "changes": self.changes.load(Ordering::Relaxed),
        });
        let since = self.since_secs.load(Ordering::Relaxed);
        if since > 0 {
            o["since_secs"] = serde_json::json!(since);
        }
        let err = self
            .last_error
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone());
        if !err.is_empty() {
            o["last_error"] = serde_json::Value::String(err);
        }
        o
    }

    fn set_error(&self, e: String) {
        match self.last_error.lock() {
            Ok(mut g) => *g = e,
            Err(p) => *p.into_inner() = e,
        }
    }
}

/// How many dead letters `/health/deep` lists, newest first. The total is
/// reported beside them, so the cut is visible.
const DEAD_LETTERS_SHOWN: usize = 10;

/// The host of a webhook destination, and nothing after it.
fn destination_host(url: &str) -> String {
    ordo_egress::HttpsUrl::parse(url)
        .map(|u| u.host().to_string())
        .unwrap_or_else(|_| "-".to_string())
}

/// The dead-letter half of the census.
#[derive(Default, Clone)]
struct DeadLetters {
    total: usize,
    recent: Vec<triggers::DeadLetter>,
    /// Attestation webhook deliveries that ran out of attempts, across
    /// every tenant (owner ruling 2026-10-05: readable by the deployment
    /// owner). The total, and the newest [`DEAD_LETTERS_SHOWN`].
    attest_total: u64,
    attest_recent: Vec<ordo_exec::webhook::DeadLetter>,
}

/// One orphaned listener cursor: registered, non-evictable, and not a
/// listener this deployment serves. §3.7: reported, never acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub consumer: String,
    pub position: u64,
    pub advanced_at: u64,
}

// ── the supervisor ───────────────────────────────────────────────────────

/// A registered worker and everything the supervisor knows about it.
pub struct Registered {
    pub worker: Arc<dyn Worker>,
    pub status: Arc<WorkerStatus>,
    /// The tick this worker actually runs on: its own, unless the operator
    /// overrode it (`ORDO_WORKER_<NAME>_TICK_MS`, [`tick_overrides`]). Held
    /// here rather than recomputed, so the census prints the number the loop
    /// is using.
    pub tick: Duration,
}

/// The running set.
pub struct Supervisor {
    ctx: Arc<WorkerCtx>,
    registered: Vec<Registered>,
    /// Taken by [`Supervisor::shutdown`], which is why this is behind a lock
    /// and the method takes `&self`.
    ///
    /// **It takes `&self` because the running supervisor is shared.**
    /// `PlatformState` holds an `Arc<Supervisor>` so `/health/deep` can
    /// answer the census, and the router holds a clone of that state. A
    /// shutdown that consumed `self` therefore had to reach the value
    /// through `Arc::into_inner`, which returns `None` whenever any of those
    /// clones is still alive: at the point `serve` shuts down, they all are.
    /// The whole of §1.5 step 3 and obligation 4 then took the `None` arm,
    /// silently, and the process exited 0 having cancelled nothing, run no
    /// final act, and released leadership only when its socket closed. A
    /// `&self` shutdown cannot be defeated by a handle somebody else holds.
    ///
    /// The lock is held only long enough to `take` the vector, never across
    /// an `await`, and a poisoned guard is unwrapped into its value for the
    /// reason [`WorkerStatus`] gives: nothing in the supervisor's own
    /// accounting may panic because of a panic it caught.
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Held by the overseer task between ticks and taken by
    /// [`Supervisor::shutdown`] at §1.5 step 5. A `tokio` mutex because the
    /// overseer probes the lock's session while holding it.
    leadership: Arc<tokio::sync::Mutex<Option<Leadership>>>,
    lead: Arc<LeaderState>,
    orphans: Arc<std::sync::Mutex<Vec<Orphan>>>,
    dead: Arc<std::sync::Mutex<DeadLetters>>,
    external: Vec<External>,
    /// Workers the census names as not running, with the reason.
    pub disabled: Vec<(String, String)>,
}

/// What the supervisor was asked to run.
#[derive(Default)]
pub struct Plan {
    pub workers: Vec<Arc<dyn Worker>>,
    /// Names the operator excluded (`ORDO_WORKERS`), or `None` for "every
    /// worker whose enabling condition holds".
    pub allow: Option<Vec<String>>,
    pub disabled: Vec<(String, String)>,
    /// Loops reported in the census and run by somebody else.
    pub external: Vec<External>,
    /// The keys (`Listener::key`) of every listener this deployment serves.
    /// A `listener`-kind cursor in `$logconsumers` that is not among them is
    /// an orphan (§3.7).
    pub listeners: Vec<String>,
    pub timing: Timing,
    /// Per-worker tick overrides, keyed by [`Worker::name`]. Built from the
    /// environment by [`tick_overrides`]; empty means every worker runs its
    /// own default.
    pub ticks: std::collections::HashMap<String, Duration>,
}

/// `ORDO_WORKER_<NAME>_TICK_MS` for each of `names`, where `<NAME>` is the
/// worker's name uppercased with `.` written `_`: `triggers.drain` is
/// `ORDO_WORKER_TRIGGERS_DRAIN_TICK_MS`.
///
/// **An unreadable value is a refusal, not a default.** A variable that is
/// set, ignored and never mentioned is the configuration equivalent of a
/// worker that is not running and says nothing, which is the defect
/// obligation 8 exists to prevent. Zero is refused for the same reason it
/// would be a bug rather than a fast tick: a worker with a zero tick spins.
///
/// The override moves the **tick**, never a [`Cadence::Schedule`] period.
/// The period is what `claim_schedule` writes into `$schedules.next_due`,
/// and obligation 2 is that the worker owns it: a supervisor that let an
/// environment variable change it would be the `claim_due` bug one layer
/// up. On a scheduled worker the override changes only how often the
/// replica asks whether the schedule is due.
pub fn tick_overrides<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Result<std::collections::HashMap<String, Duration>, String> {
    let mut out = std::collections::HashMap::new();
    for name in names {
        let var = format!(
            "ORDO_WORKER_{}_TICK_MS",
            name.replace('.', "_").to_uppercase()
        );
        let Ok(raw) = std::env::var(&var) else {
            continue;
        };
        let ms: u64 = raw.trim().parse().map_err(|_| {
            format!("{var} is `{raw}`, which is not a whole number of milliseconds")
        })?;
        if ms == 0 {
            return Err(format!("{var} is 0, and a worker with no tick spins"));
        }
        out.insert(name.to_string(), Duration::from_millis(ms));
    }
    Ok(out)
}

/// The tables `ensure_platform_tables` deliberately does not create.
///
/// Called by [`Supervisor::start`] and again by whoever registers a
/// listener before starting one — **two callers of an idempotent DDL guard
/// is the correct number when two independent things both need the table**,
/// and `register_listener` writes a `$logconsumers` row, so it cannot run
/// before this has.
pub async fn bootstrap(pool: &Pool) -> Result<(), String> {
    ordo_exec::prune::ensure_tables(pool)
        .await
        .map_err(|e| format!("$logconsumers: {e}"))?;
    ordo_exec::cadence::ensure_tables(pool)
        .await
        .map_err(|e| format!("$attestations: {e}"))?;
    // K1: the re-seal pass table. Written by `ordo keys rotate` and by the
    // worker, and read by both; created by whoever starts the worker, for
    // the same reason `$logconsumers` is.
    ordo_exec::reseal::ensure_tables(pool)
        .await
        .map_err(|e| format!("$reseal: {e}"))?;
    Ok(())
}

impl Supervisor {
    /// Create `$logconsumers` and `$attestations`, then start every worker.
    ///
    /// **The bootstrap DDL goes here, not in `ensure_platform_tables`**, and
    /// the checklist already made that trade deliberately: the trigger
    /// tables are in `ensure_platform_tables` because accumulation rides the
    /// write transaction and must exist before the first write, while a
    /// cursor is registered by whoever starts the listener. The supervisor
    /// **is** whoever starts the listener. Moving `$logconsumers` down would
    /// create it in every process that opens a pool — the playground, every
    /// conformance suite, every `ordo` command — for a table only listeners
    /// and the pruner use, and would quietly reverse a decision somebody
    /// wrote down.
    pub async fn start(
        pool: Pool,
        keys: Arc<dyn KeyProvider>,
        deployment: Arc<Deployment>,
        metrics: Arc<Metrics>,
        space: SpaceId,
        pg: PoolConfig,
        plan: Plan,
    ) -> Result<Supervisor, String> {
        bootstrap(&pool).await?;

        let cancel = Cancel::new();
        let ctx = Arc::new(WorkerCtx {
            pool,
            keys,
            deployment,
            metrics,
            space: space.clone(),
            cancel: cancel.clone(),
            now: Arc::new(now_secs),
            timing: plan.timing,
        });

        let mut disabled = plan.disabled;
        let mut registered = Vec::new();
        let mut tasks = Vec::new();
        let mut wants_leader = false;
        let mut leader_only = Vec::new();

        for worker in plan.workers {
            let name = worker.name();
            if let Some(allow) = &plan.allow {
                if !allow.iter().any(|a| a == name) {
                    disabled.push((name.into(), "not in ORDO_WORKERS".into()));
                    continue;
                }
            }
            // Obligation 2's `$schedules` row must exist before `claim`
            // (`jobs::claim_schedule`) can ever find it due: the claim is
            // an `UPDATE ... WHERE name IN (SELECT ...)`, which matches
            // nothing for a name nobody has inserted, so a `Cadence::
            // Schedule` worker with no prior row would idle forever. This
            // is the one seed, `ON CONFLICT DO NOTHING` (`jobs::schedule`)
            // so a replica joining a deployment that already has the row
            // never resets a cycle in progress, and `next_due = 0` so the
            // replica that DOES create it runs at once rather than waiting
            // out a full period first. `usage.rollup` is the first worker
            // through this path; `keys.reseal` (K1) rides the same one.
            if let Cadence::Schedule { name: sched, .. } = worker.cadence() {
                let tx = ctx.pool.begin().await.map_err(|e| e.to_string())?;
                tx.schedule(sched, 0).await.map_err(|e| e.to_string())?;
                tx.commit().await.map_err(|e| e.to_string())?;
            }
            wants_leader |= worker.requires_leadership();
            let status = Arc::new(WorkerStatus {
                name,
                enabled: AtomicBool::new(true),
                ..WorkerStatus::default()
            });
            if worker.requires_leadership() {
                leader_only.push(status.clone());
            }
            registered.push(Registered {
                worker: worker.clone(),
                status: status.clone(),
                tick: resolve_tick(worker.cadence(), plan.ticks.get(name).copied()),
            });
        }

        // One leadership claim for the whole supervisor, contended only if
        // some worker wants it: a deployment running nothing leader-only
        // should not hold a lock a replacement replica then waits on.
        let lead = Arc::new(LeaderState {
            wanted: wants_leader,
            ..LeaderState::default()
        });
        let leadership = Arc::new(tokio::sync::Mutex::new(None));
        let orphans = Arc::new(std::sync::Mutex::new(Vec::new()));
        let dead = Arc::new(std::sync::Mutex::new(DeadLetters::default()));

        // The first attempt and the first census happen here, before
        // `start` returns, so the startup log and the first `/health/deep`
        // already answer. The overseer repeats both on its slow tick.
        let overseer = Overseer {
            ctx: ctx.clone(),
            pg,
            lead: lead.clone(),
            leadership: leadership.clone(),
            leader_only,
            orphans: orphans.clone(),
            dead: dead.clone(),
            listeners: plan.listeners,
        };
        overseer.contend().await;
        overseer.census().await;
        tasks.push(tokio::spawn(overseer.run()));

        // The loops start after the first leadership answer, so a
        // leader-only worker on the replica that won begins at once rather
        // than one leadership tick later.
        for r in &registered {
            tasks.push(tokio::spawn(worker_loop(
                r.worker.clone(),
                r.status.clone(),
                ctx.clone(),
                cancel.clone(),
                r.tick,
            )));
        }

        Ok(Supervisor {
            ctx,
            registered,
            tasks: std::sync::Mutex::new(tasks),
            leadership,
            lead,
            orphans,
            dead,
            external: plan.external,
            disabled,
        })
    }

    /// Whether this replica currently holds leadership.
    pub fn is_leader(&self) -> bool {
        self.lead.leader.load(Ordering::Relaxed)
    }

    /// The orphaned listener cursors the last census found.
    pub fn orphans(&self) -> Vec<Orphan> {
        self.orphans
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    /// The `/health/deep` `supervisor` section: leadership and the
    /// orphaned-cursor census. Separate from [`Supervisor::health`] because
    /// that section is keyed by worker name, and neither of these is a
    /// worker.
    pub fn overview(&self) -> serde_json::Value {
        let now = now_secs();
        let orphans: Vec<serde_json::Value> = self
            .orphans()
            .into_iter()
            .map(|o| {
                serde_json::json!({
                    "consumer": o.consumer,
                    "position": o.position,
                    "advanced_at": o.advanced_at,
                    "idle_secs": now.saturating_sub(o.advanced_at),
                })
            })
            .collect();
        let dead = self
            .dead
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone());
        serde_json::json!({
            "leadership": self.lead.json(),
            // §3.7: listed, never removed. Removing one is an operator act
            // (`ordo triggers deregister`, over `prune::deregister_consumer`),
            // because a replica on the next
            // version deregistering a listener a replica on this one still
            // serves would reset its at-least-once guarantee.
            "orphaned_listeners": orphans,
            // §3.6: dead letters are an operator alert, and this is where
            // the alert can be read. Paging stays the deployment's.
            "dead_letters": {
                "total": dead.total,
                "recent": dead.recent.iter().map(|d| serde_json::json!({
                    "run_key": d.run_key,
                    "listener": d.listener,
                    "attempts": d.attempts,
                    "at": d.at,
                    "reason": d.reason,
                })).collect::<Vec<_>>(),
            },
            // Attestation deliveries that gave up, every tenant's, newest
            // first. Read only, as the trigger ones are: there is no retry
            // yet. The destination is shown as its host alone, because a
            // webhook URL often carries a token in its path, and that is
            // the tenant owner's to read (`std.space.attestationDeadLetters`)
            // rather than every reader of this page's.
            "attestation_dead_letters": {
                "total": dead.attest_total,
                "recent": dead.attest_recent.iter().map(|d| serde_json::json!({
                    "space": d.space.0,
                    "upto": d.upto,
                    "host": destination_host(&d.destination),
                    "attempts": d.attempts,
                    "at": d.at,
                    "reason": d.reason,
                })).collect::<Vec<_>>(),
            },
        })
    }

    /// The `/health/deep` `workers` section.
    pub fn health(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        for r in &self.registered {
            o.insert(r.status.name.to_string(), r.status.json());
        }
        for x in &self.external {
            let mut v = (x.health)();
            if !v.is_object() {
                v = serde_json::json!({ "detail": v });
            }
            v["enabled"] = serde_json::json!(true);
            v["supervised"] = serde_json::json!(false);
            o.insert(x.name.to_string(), v);
        }
        for (name, why) in &self.disabled {
            o.insert(
                name.clone(),
                serde_json::json!({ "enabled": false, "reason": why }),
            );
        }
        serde_json::Value::Object(o)
    }

    /// Names and statuses, for the startup log's census.
    pub fn running(&self) -> Vec<&'static str> {
        self.registered.iter().map(|r| r.status.name).collect()
    }

    /// The running half of the startup log: each worker with the cadence it
    /// is actually on.
    ///
    /// The number is printed because a tick is overridable
    /// ([`tick_overrides`]) and an operator who set one needs to see it took
    /// effect. Printing it for every worker rather than only the overridden
    /// ones is the same posture as the DISABLED lines: a default nobody
    /// states is a default somebody will assume is something else.
    pub fn census(&self) -> Vec<(&'static str, String)> {
        self.registered
            .iter()
            .map(|r| {
                let how = match r.worker.cadence() {
                    Cadence::Tick(_) => format!("tick {}ms", r.tick.as_millis()),
                    Cadence::Schedule { name, period } => format!(
                        "schedule {name} every {}s, polled {}ms",
                        period.as_secs(),
                        r.tick.as_millis()
                    ),
                    Cadence::Continuous => "continuous".to_string(),
                };
                let how = if r.worker.requires_leadership() {
                    format!("{how}, leader only")
                } else {
                    how
                };
                (r.status.name, how)
            })
            .chain(self.external.iter().map(|x| {
                (
                    x.name,
                    "unsupervised: its own tick, its own flush".to_string(),
                )
            }))
            // An `External` is a loop this process runs and does not
            // supervise, and it belongs in the log for the same reason it
            // belongs on `/health/deep`: the census's claim is "these are the
            // loops in this process", and `secevent.drain` is one. Saying
            // `unsupervised` rather than nothing keeps the two surfaces
            // agreeing, where `/health/deep` says `supervised: false`.
            .collect()
    }

    /// Registered workers that are **quarantined**: stopped after
    /// [`QUARANTINE_AFTER`] consecutive failures, and not restarting on
    /// their own.
    ///
    /// For `/health/ready`'s "the workers that must run are running" check.
    /// Atomic loads over an in-memory `Vec`, so this is cheap enough to run
    /// on every probe — no dial, no lock that a worker's own fault could
    /// hold. A worker this supervisor never registered (nothing runs it) is
    /// not this check's business: quarantine is a fact about a worker that
    /// **was** running and stopped, not about coverage.
    pub fn quarantined(&self) -> Vec<&'static str> {
        self.registered
            .iter()
            .filter(|r| r.status.quarantined.load(Ordering::Relaxed))
            .map(|r| r.status.name)
            .collect()
    }

    /// §1.5 steps 3 and 5: cancel, let each worker finish its current unit,
    /// run every final act, then release leadership.
    ///
    /// `&self`, not `self`, and the field doc on `tasks` says why: the
    /// running supervisor is shared with `/health/deep`, so a shutdown that
    /// needed sole ownership did nothing at all in the shipped binary.
    /// Returns how many final acts ran, which is the number `serve` prints.
    pub async fn shutdown(&self, grace: Duration) -> usize {
        self.ctx.cancel.cancel();
        let deadline = tokio::time::Instant::now() + grace;
        // Locked only to take the vector. Awaiting a join handle while
        // holding a `std::sync::Mutex` would hold it across an await.
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|e| e.into_inner()));
        for t in tasks {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if tokio::time::timeout(left, t).await.is_err() {
                eprintln!("supervisor: a worker did not stop within the grace");
            }
        }
        // Obligation 4, after cancellation and before the security drain
        // flushes — M1's partial period belongs in the record the drain is
        // about to write.
        let mut acts = 0;
        for r in &self.registered {
            match r.worker.on_shutdown(&self.ctx).await {
                Ok(()) => acts += 1,
                Err(e) => eprintln!("supervisor: {}'s final act failed: {}", r.status.name, e.0),
            }
        }
        // Explicit, before the pool goes: closing the leadership connection
        // is what lets the replacement replica take over in milliseconds
        // rather than at TCP timeout.
        // The overseer has been joined above, so nothing re-acquires after
        // this.
        if self.leadership.lock().await.take().is_some() {
            self.lead.leader.store(false, Ordering::Relaxed);
            self.ctx.metrics.set(SUPERVISOR_LEADER, &[], 0);
        }
        acts
    }
}

/// The supervisor's own background task: leadership on §3.5's slow tick,
/// and the orphaned-cursor census.
///
/// **Why a probe and not only `acquire`.** A leader cannot learn it has
/// lost the lock by trying to take it again: advisory locks are re-entrant
/// per session, so the question has to be asked on the session that holds
/// it, and the only way a session-scoped lock is lost is that the session
/// ends. So the leader asks "is my session still there" each tick, bounded
/// by the tick, and a follower asks "can I take the lock" each tick. Both
/// answers update the per-worker `leader` flag the worker loops read, so a
/// follower that wins starts its leader-only workers on their next pass and
/// a leader whose session died stops them before their next unit.
struct Overseer {
    ctx: Arc<WorkerCtx>,
    pg: PoolConfig,
    lead: Arc<LeaderState>,
    leadership: Arc<tokio::sync::Mutex<Option<Leadership>>>,
    leader_only: Vec<Arc<WorkerStatus>>,
    orphans: Arc<std::sync::Mutex<Vec<Orphan>>>,
    dead: Arc<std::sync::Mutex<DeadLetters>>,
    listeners: Vec<String>,
}

impl Overseer {
    async fn run(self) {
        let tick = self.ctx.timing.leader_tick;
        let census_every = self.ctx.timing.census_every;
        let mut last_census = tokio::time::Instant::now();
        loop {
            self.ctx.cancel.sleep(tick).await;
            if self.ctx.cancel.is_cancelled() {
                return;
            }
            self.contend().await;
            if last_census.elapsed() >= census_every {
                self.census().await;
                last_census = tokio::time::Instant::now();
            }
        }
    }

    /// One leadership step: probe if leader, try to acquire if not.
    async fn contend(&self) {
        if !self.lead.wanted {
            return;
        }
        let bound = self.ctx.timing.leader_tick.max(Duration::from_secs(1));
        let mut held = self.leadership.lock().await;
        if let Some(l) = held.as_ref() {
            let alive = tokio::time::timeout(bound, l.is_held())
                .await
                .unwrap_or(false);
            if !alive {
                // Dropping it closes whatever is left of the session. The
                // loss is recorded before the retry below, so a lock that
                // was lost and retaken inside one tick still shows as two
                // transitions rather than as nothing.
                *held = None;
                self.flip(false);
            }
        }
        if held.is_none() {
            match tokio::time::timeout(bound, Leadership::acquire(&self.pg, &self.ctx.space.0))
                .await
            {
                Ok(Ok(l)) => {
                    self.lead.set_error(String::new());
                    self.flip(l.is_some());
                    *held = l;
                }
                Ok(Err(e)) => {
                    self.lead.set_error(e);
                    self.flip(false);
                }
                Err(_) => {
                    self.lead
                        .set_error("the leadership dial did not answer within its tick".into());
                    self.flip(false);
                }
            }
        }
        self.ctx
            .metrics
            .set(SUPERVISOR_LEADER, &[], u64::from(held.is_some()));
    }

    /// Record this replica's leadership, and count it if it changed.
    ///
    /// The per-worker flags are written first, so a leader-only worker has
    /// stopped taking new units by the time anything reports the loss.
    fn flip(&self, now: bool) {
        for s in &self.leader_only {
            s.leader.store(now, Ordering::SeqCst);
        }
        if self.lead.leader.swap(now, Ordering::SeqCst) != now {
            let to = if now { "leader" } else { "follower" };
            self.lead.changes.fetch_add(1, Ordering::Relaxed);
            self.lead.since_secs.store(now_secs(), Ordering::Relaxed);
            self.ctx
                .metrics
                .incr(SUPERVISOR_LEADERSHIP_CHANGES, &[("to", to)]);
            eprintln!("supervisor: this replica is now the {to}");
        }
    }

    /// Re-read the dead letters and the orphaned listener cursors. A failure
    /// keeps the last answer: a census that blanked itself whenever the
    /// database blinked would report "no orphans" at exactly the wrong
    /// moment.
    async fn census(&self) {
        let space = ordo_commandlog::SpaceId(self.ctx.space.0.clone());
        if let Ok(all) = triggers::dead_letters(&self.ctx.pool, &space).await {
            let total = all.len();
            let recent: Vec<_> = all.into_iter().rev().take(DEAD_LETTERS_SHOWN).collect();
            match self.dead.lock() {
                Ok(mut g) => {
                    g.total = total;
                    g.recent = recent;
                }
                Err(p) => {
                    let mut g = p.into_inner();
                    g.total = total;
                    g.recent = recent;
                }
            }
        }
        if let Ok((total, recent)) =
            ordo_exec::webhook::recent_dead_letters(&self.ctx.pool, DEAD_LETTERS_SHOWN).await
        {
            match self.dead.lock() {
                Ok(mut g) => {
                    g.attest_total = total;
                    g.attest_recent = recent;
                }
                Err(p) => {
                    let mut g = p.into_inner();
                    g.attest_total = total;
                    g.attest_recent = recent;
                }
            }
        }
        let Ok(all) = ordo_exec::prune::consumers(&self.ctx.pool, &space).await else {
            return;
        };
        let found: Vec<Orphan> = all
            .into_iter()
            .filter(|c| c.kind == ordo_exec::prune::ConsumerKind::Listener)
            .filter(|c| !self.listeners.iter().any(|k| k == &c.name))
            .map(|c| Orphan {
                consumer: c.name,
                position: c.position,
                advanced_at: c.advanced_at,
            })
            .collect();
        match self.orphans.lock() {
            Ok(mut g) => *g = found,
            Err(p) => *p.into_inner() = found,
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One worker's loop. Every obligation that is not about scheduling is
/// discharged here.
async fn worker_loop(
    worker: Arc<dyn Worker>,
    status: Arc<WorkerStatus>,
    ctx: Arc<WorkerCtx>,
    cancel: Cancel,
    tick: Duration,
) {
    use futures::FutureExt;

    let name = worker.name();

    while !cancel.is_cancelled() {
        // Leader-only workers idle rather than exit when this replica is a
        // follower: leadership can move, and a worker that exited would
        // need a restart to notice. The overseer flips the flag; this is
        // where "start" and "stop" take effect, between units.
        if worker.requires_leadership() && !status.leader.load(Ordering::Relaxed) {
            cancel.sleep(ctx.timing.leader_tick).await;
            continue;
        }
        // Obligation 2: the schedule is claimed with the worker's OWN
        // period, so two cadences cannot advance one another's `next_due`.
        if let Cadence::Schedule {
            name: sched,
            period,
        } = worker.cadence()
        {
            match claim(&ctx, sched, period).await {
                Ok(false) => {
                    cancel.sleep(tick).await;
                    continue;
                }
                Ok(true) => {}
                Err(e) => {
                    fail(&ctx, &status, name, "error", &e, &cancel, tick).await;
                    continue;
                }
            }
        }

        let started = std::time::Instant::now();
        // Obligation 6. `futures` is already a dependency and the workspace
        // sets no `panic = "abort"`, so unwinding is available.
        let outcome = std::panic::AssertUnwindSafe(worker.run(&ctx))
            .catch_unwind()
            .await;

        match outcome {
            Ok(Ok(progress)) => {
                status.runs.fetch_add(1, Ordering::Relaxed);
                ctx.metrics
                    .incr(WORKER_RUNS, &[("worker", name), ("outcome", "ok")]);
                if progress.units > 0 {
                    status.units.fetch_add(progress.units, Ordering::Relaxed);
                    ctx.metrics
                        .incr_by(WORKER_UNITS, &[("worker", name)], progress.units);
                }
                status.last_success_secs.store(ctx.now(), Ordering::Relaxed);
                // **The reset rule.** Consecutive failures reset on a run
                // that succeeded *and lasted longer than the current
                // backoff*, not on any success: a worker that fails after
                // doing one unit of work would otherwise sit at attempt 1
                // forever and hammer a broken dependency at one-second
                // intervals while reporting progress.
                let attempts = status.consecutive.load(Ordering::Relaxed);
                if attempts == 0 || started.elapsed() >= ctx.timing.backoff(attempts) {
                    status.consecutive.store(0, Ordering::Relaxed);
                }
                if progress.more {
                    // Obligation 3: re-enter immediately rather than wait
                    // out the tick.
                    continue;
                }
                if !matches!(worker.cadence(), Cadence::Continuous) {
                    cancel.sleep(tick).await;
                }
            }
            Ok(Err(e)) => fail(&ctx, &status, name, "error", &e.0, &cancel, tick).await,
            Err(panic) => {
                let what = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic with no message".into());
                fail(&ctx, &status, name, "panic", &what, &cancel, tick).await;
            }
        }

        if status.quarantined.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// The tick a worker's loop sleeps for, and where an override lands.
///
/// A `Schedule` worker polls at its period, capped at thirty seconds, so a
/// daily schedule is still picked up promptly by whichever replica is leader
/// when it falls due. The cap is a poll interval and never the period: the
/// period goes to `claim_schedule`, which is what advances `next_due`, and
/// obligation 2 keeps that the worker's.
///
/// `Continuous` blocks on its own source, so its tick is zero and an
/// override of it would be meaningless.
fn resolve_tick(cadence: Cadence, override_ms: Option<Duration>) -> Duration {
    match (cadence, override_ms) {
        (Cadence::Continuous, _) => Duration::from_millis(0),
        (_, Some(d)) => d,
        (Cadence::Tick(d), None) => d,
        (Cadence::Schedule { period, .. }, None) => period.min(Duration::from_secs(30)),
    }
}

async fn claim(ctx: &WorkerCtx, name: &str, period: Duration) -> Result<bool, String> {
    let tx = ctx.pool.begin().await.map_err(|e| e.to_string())?;
    let due = tx
        .claim_schedule(name, ctx.now() as i64, period.as_secs() as i64)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(due)
}

async fn fail(
    ctx: &WorkerCtx,
    status: &WorkerStatus,
    name: &'static str,
    outcome: &'static str,
    what: &str,
    cancel: &Cancel,
    tick: Duration,
) {
    status.runs.fetch_add(1, Ordering::Relaxed);
    status.failures.fetch_add(1, Ordering::Relaxed);
    let n = status.consecutive.fetch_add(1, Ordering::Relaxed) + 1;
    ctx.metrics
        .incr(WORKER_RUNS, &[("worker", name), ("outcome", outcome)]);
    ctx.metrics
        .incr(WORKER_RESTARTS, &[("worker", name), ("reason", outcome)]);
    eprintln!("worker {name}: {outcome} ({n} consecutive) — {what}");

    if n >= QUARANTINE_AFTER {
        status.quarantined.store(true, Ordering::Relaxed);
        status.enabled.store(false, Ordering::Relaxed);
        if let Ok(mut r) = status.reason.lock() {
            *r = format!("quarantined after {n} consecutive failures: {what}");
        }
        eprintln!(
            "worker {name}: QUARANTINED after {n} consecutive failures. \
             Nothing is dropped — delivery is paused, and a paused listener \
             stops shrinking the log, which is where an operator will see it."
        );
        return;
    }
    // The platform's ONE backoff, called rather than re-derived; the unit
    // is one second outside a test (see `Timing`).
    cancel.sleep(retry_wait(ctx.timing.backoff(n), tick)).await;
}

/// How long a failed worker waits before its next attempt: the backoff,
/// never longer than the worker's own period. A worker that ticks every
/// second must not sleep 64 s after an error, because that is slower than
/// the work it normally does. A zero tick (`Continuous`) has no period to
/// cap by.
fn retry_wait(backoff: Duration, tick: Duration) -> Duration {
    if tick.is_zero() {
        backoff
    } else {
        backoff.min(tick)
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[test]
    fn an_erroring_worker_backs_off_with_each_failure_and_never_waits_longer_than_its_period() {
        let timing = Timing::default();
        for period in [
            Duration::from_secs(1),
            Duration::from_secs(10),
            Duration::from_secs(600),
        ] {
            let mut last = Duration::ZERO;
            for n in 1..QUARANTINE_AFTER {
                let wait = retry_wait(timing.backoff(n), period);
                assert!(wait >= last, "attempt {n} waited less than the one before");
                assert!(
                    wait <= period,
                    "attempt {n} waited {wait:?}, past {period:?}"
                );
                assert!(!wait.is_zero(), "attempt {n} did not wait at all");
                last = wait;
            }
        }
        // A 600 s worker still backs off from 2 s upward, and a continuous
        // worker (no period) keeps the plain backoff.
        assert_eq!(
            retry_wait(timing.backoff(1), Duration::from_secs(600)),
            Duration::from_secs(2)
        );
        assert_eq!(
            retry_wait(timing.backoff(3), Duration::ZERO),
            Duration::from_secs(8)
        );
    }
}
