//! 8o, third part — a metrics substrate.
//!
//! Before this there was no `tracing`, no `prometheus`, no
//! `opentelemetry` anywhere in the workspace, so none of
//! `implementation.md`'s five mandatory alerts had anything to count. Two
//! of them are security controls rather than service telemetry, and those
//! are the two this substrate is built for:
//!
//! - **denial volume per principal per space** — the only surface on which
//!   a probe *sweeping* spaces is visible at all, since the per-space
//!   security stream deliberately records members only; and
//! - **key-release breadth** — how much custody a principal's work is
//!   touching, against what their plan declared.
//!
//! # What this is
//!
//! An in-process metric registry of two shapes: **counters** — a name, a
//! sorted label set, a `u64` — and **histograms**, the same key over a
//! fixed bucket ladder plus a sum and a count.
//! [`Metrics::render_prometheus`] renders both in the Prometheus text
//! exposition format, so an ordinary scrape works and an alert rule can be
//! written today. It is deliberately small enough to read in one sitting.
//!
//! # The histograms, and the measurements they unblock (5f)
//!
//! Two of `implementation.md`'s three release-blocking measurements were
//! blocked on this file rather than on a deployment. Both are quantile
//! questions and a counter cannot answer one:
//!
//! - **[`CHAIN_APPEND_LOCK_WAIT`]** — `implementation.md` calls it *"the
//!   production answer to 'is this biting yet'"* and requires it to exist
//!   *before any tuning does*. It measures the time a writer spends
//!   **blocked in `pg_advisory_xact_lock`**, which is the question: a
//!   throughput run without it reports a rate and cannot say whether the
//!   per-space append lock is what bounded it.
//! - **[`CHAIN_APPEND_WORK`]** — the other half of the same fraction, and
//!   the reason the pair is worth more than either. Wait is only
//!   interpretable against how long the critical section is: the doc names
//!   *"anything that widens the critical section"* as the failure mode to
//!   watch for, and this is that width, measured rather than reviewed.
//! - **[`OP_DURATION`]** — `ordo_op_duration_seconds{op,outcome}`, the
//!   p99 measurement's prerequisite. Recorded by [`MeteredExecutor`],
//!   whose doc states exactly which part of a request it does *not* cover.
//!
//! [`Metrics::quantile`] computes the same estimate Prometheus's
//! `histogram_quantile` would, so a number asserted in a test and a number
//! read off a dashboard are the same number, computed the same way, with
//! the same bucket-resolution limits.
//!
//! # What this is NOT — stated, because a half-built substrate that looks
//! whole is worse than an absent one
//!
//! - **No traces.** `implementation.md` pins one OpenTelemetry span per
//!   request with a fixed attribute set and child spans mirroring the
//!   dispatch pipeline. None of that is here. `ordo.correlation` — the key
//!   that joins a trace to a command group — has no emitter.
//! - **A histogram is not a timer.** Nothing here samples a clock; every
//!   observation is pushed in by an instrumented call site, and a call
//!   site nobody instrumented is silent rather than zero. Two lock-wait
//!   sites are deliberately *not* instrumented: the security stream's own
//!   append lock (a different namespace, and §6.1 requires it never to
//!   queue behind a command append — its contention is a different
//!   question) and the prune path (an operator job, not the write path).
//! - **No structured logs.** The deployment security log and the per-space
//!   hash-chained security-event stream are separate artifacts with
//!   separate retentions; this is neither, and it is not a substitute for
//!   either. It is the *alerting* layer over them, and only that. The stream
//!   itself is built (7a, `ordo-secevent`); the JSON-per-request log is not,
//!   which is why a key release still cannot be resolved to a principal.
//! - **No push, no exporter process, no aggregation across replicas.**
//!   Counters live in one process's memory and reset when it restarts, so
//!   every series is per replica and a rate across a deployment is the
//!   scraper's sum, not ours.
//! - **One alert rule, and only one.** 7b added the mandatory deny-volume
//!   rule — [`Metrics::evaluate_alerts`] / [`Metrics::render_alerts`], over
//!   `ordo_secevent::DenyVolumeMonitor`, with the threshold read from the
//!   deployment's environment. The other four of the doc's five mandatory
//!   alerts (key-release breadth, chain verification / checkpoint age,
//!   dead-letter growth, shape-verification failure) still have counters and
//!   no rule. And even the one that exists sees **one process**: no
//!   cross-replica aggregation, no routing, no persistence across a restart.
//!   `ordo_secevent::alert` lists what a deployment must supply.
//! - **No gauges that need a producer.** `ordo_job_queue_depth`,
//!   `ordo_live_sessions` and friends need something to sample them on a
//!   tick; nothing does, so they are absent rather than stuck at zero,
//!   which would read as "healthy".
//!
//! # Cardinality
//!
//! `principal` and `op` are both open sets, and per-principal series are
//! exactly what the denial alert needs — so the label set is unbounded by
//! construction. Rather than let a token-guessing flood turn the metrics
//! map into the outage, the registry has a hard series cap: past it, new
//! series are dropped and `ordo_metrics_series_dropped_total` counts the
//! drops. An operator who sees that counter move knows the denial numbers
//! below it have gone incomplete, which is the honest failure.
//!
//! **A histogram is [`BUCKETS`]`.len() + 2` exposition lines, not one**, so
//! it is charged against the same cap and the cap means the same thing.
//! `space` is the label the doc singles out as high-cardinality, and
//! `implementation.md` makes per-space series **opt-in** — so the lock-wait
//! family carries no `space` label unless a deployment asks for one
//! ([`Metrics::with_per_space_series`], `ORDO_METRICS_PER_SPACE`). The
//! default answers *"is the append lock biting anywhere"*; the opt-in
//! answers *"in which space"*, and costs one histogram per space to do it.
//!
//! **Two counters are exempt and say why on themselves**
//! ([`POOL_SATURATED`], [`POOL_CAPPED`], 20a). They carry `space` with the
//! opt-in off because their population is the spaces a checkout was
//! refused for rather than the spaces a deployment has: nothing saturating
//! means no series, and the cardinality the opt-in refuses to pay is the
//! cost of watching everybody, not the cost of naming whoever is causing
//! an incident. The pool's healthy-path histogram ([`POOL_WAIT`]) is the
//! one that would cost a series per tenant, and it stays behind the
//! opt-in with the rest.
//!
//! # What never appears in a label
//!
//! Parameter values, row values, sealed plaintext, page tokens,
//! `Idempotency-Key` values, bearer tokens. Telemetry is envelope-tier by
//! the same rule the command log follows. Principal ids, op names, spaces
//! and error codes are envelope-tier and do appear.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ordo_commandlog::Timestamp;
use ordo_secevent::{DenyVolumeMonitor, DenyVolumeRule, Firing};

/// `ordo_denials_total{space,principal,principal_kind,op}` — the substrate
/// for the mandatory denial-volume alert (7b).
pub const DENIALS: &str = "ordo_denials_total";
/// `ordo_key_release_total{space,purpose}` — the substrate for the
/// mandatory key-release-breadth alert.
pub const KEY_RELEASES: &str = "ordo_key_release_total";
/// `ordo_sealed_unreadable_total{space,epoch}` — sealed values that failed
/// to open with their key present (unreadable, possibly tampered). Counts
/// every failure; only the security event is rate-limited.
pub const SEALED_UNREADABLE: &str = "ordo_sealed_unreadable_total";
/// `ordo_shape_verification_failures_total{}` — deploy health. Incremented
/// when the load gate refuses a registry.
pub const SHAPE_FAILURES: &str = "ordo_shape_verification_failures_total";
/// `ordo_invalidations_total{source}` — how many staleness signals the
/// deployment published (`source="listener"` for ones that arrived over
/// Postgres `LISTEN`, i.e. possibly from another node).
pub const INVALIDATIONS: &str = "ordo_invalidations_total";
/// The registry's own honesty counter: series refused by the cardinality
/// cap, so an incomplete count never reads as a low one.
pub const SERIES_DROPPED: &str = "ordo_metrics_series_dropped_total";
/// `ordo_reseal_rows_remaining{space,population}` (gauge) — how much of
/// a space is still under the epoch its open re-seal pass is moving off
/// (`plan-production-ops.md` K3). Carries `space` unconditionally: the
/// series count is bounded by spaces with a live pass, not by spaces.
pub const RESEAL_REMAINING: &str = "ordo_reseal_rows_remaining";
/// `ordo_reseal_stuck_seconds{space}` (gauge) — wall time since the pass's
/// cursor last advanced while rows remain. **The cursor, not the queue**: a
/// pass that is claimed and making no progress is invisible to a depth
/// gauge, and a rotation that silently stops is a space whose gate never
/// clears.
pub const RESEAL_STUCK: &str = "ordo_reseal_stuck_seconds";
/// `ordo_blob_gc_uncollected_total{cause}` — a database or a store the
/// storage sweep could not reach on a pass, so its objects were not
/// collected (`plan-tenant-storage.md` §6 B-G4). `cause` is `target` (the
/// database could not be dialled), `database` (it answered and a query
/// failed) or `store` (the object store refused). No `space` label: a tenant
/// whose bucket has been down for a month shows here as a count that keeps
/// rising, and the log line beside it names the tenant.
pub const BLOB_GC_UNCOLLECTED: &str = "ordo_blob_gc_uncollected_total";

/// `ordo_chain_append_lock_wait_seconds` (histogram) — seconds a writer
/// spent **blocked in `pg_advisory_xact_lock`** before its append could
/// proceed. `implementation.md`: *"the production answer to 'is this biting
/// yet', and it must exist before any tuning does"*.
///
/// The wait, not the append: a write's total latency moves for a dozen
/// reasons, and only this series says the per-space serialization point was
/// one of them.
pub const CHAIN_APPEND_LOCK_WAIT: &str = "ordo_chain_append_lock_wait_seconds";

/// `ordo_chain_append_seconds` (histogram) — seconds spent **inside** the
/// critical section: head read, duplicate-correlation check, hash, insert.
///
/// Not in the doc's table, and added anyway, because the wait alone is not
/// a diagnosis. Wait is (roughly) queue depth times this; a wait that grew
/// while this stayed flat is more writers, and a wait that grew because
/// this grew is the failure mode `implementation.md` names in terms —
/// *"anything that widens the critical section … is the failure mode to
/// guard against in code review"*. Measuring it turns a review habit into
/// a series an alert can watch.
pub const CHAIN_APPEND_WORK: &str = "ordo_chain_append_seconds";

/// `ordo_pool_saturated_total{space}` — checkouts refused because the
/// acquire budget elapsed, against the pool's bound or against the space's
/// share of it (20a).
///
/// # Why this one carries `space` with the opt-in off
///
/// `implementation.md` makes per-space series opt-in and the argument is
/// sound for a family with a series per tenant. This family has a series
/// per tenant **that was refused**, which is the incident population: a
/// deployment where nothing saturates carries no series at all, and one
/// where a single tenant is taking the pool carries one. The cardinality
/// the opt-in refuses to pay is the cost of watching everybody; the cost
/// here is the cost of naming whoever is currently causing an outage, and
/// `audit-topology-gaps.md` §2.7's disclosure is exactly that a deployment
/// which never set the variable "sees a slow deployment and no name".
///
/// The series cap and `ordo_metrics_series_dropped_total` remain the
/// backstop, so a flood of distinct spaces degrades the way every other
/// caller-influenced label already does.
pub const POOL_SATURATED: &str = "ordo_pool_saturated_total";

/// `ordo_pool_space_capped_total{space}` — checkouts that queued behind
/// their own space's share of the pool (20a). Labelled by space with the
/// opt-in off, for [`POOL_SATURATED`]'s reason.
///
/// **This is the series that answers "which space is consuming the
/// pool"**, and it answers it by construction: a space only reaches its
/// cap by holding every connection its cap allows. `POOL_SATURATED` says
/// who was hurt; this says who did it, and the two together are the
/// difference between an outage with a name and one without.
pub const POOL_CAPPED: &str = "ordo_pool_space_capped_total";

/// `ordo_pool_wait_seconds` (histogram) — seconds one checkout spent
/// waiting for admission: its space's share plus the pool's own bound,
/// which share a single budget.
///
/// The healthy path, so this is the family that would cost a histogram per
/// tenant, and it carries `space` only behind `ORDO_METRICS_PER_SPACE`.
/// Unlabelled it answers *"is the pool queueing"*; labelled it answers
/// *"for whom"*.
pub const POOL_WAIT: &str = "ordo_pool_wait_seconds";

/// `ordo_op_duration_seconds{op,outcome}` (histogram) — the ordinary
/// service view, and the p99 measurement's prerequisite. Recorded by
/// [`MeteredExecutor`]; see its doc for what the span does and does not
/// include.
pub const OP_DURATION: &str = "ordo_op_duration_seconds";

/// `ordo_quota_over_limit_total{space,quantity}` (17n M2) — every
/// [`crate::quotas::PgQuotas::check`] that answered `Warn` or `Exceeded`.
/// Labelled unconditionally, `POOL_CAPPED`'s reason: a space only reaches
/// its own quota by using its own traffic, so this is the same "who did it"
/// shape and not the cardinality bill the per-space opt-in exists to
/// refuse.
pub const QUOTA_OVER_LIMIT: &str = "ordo_quota_over_limit_total";

/// Hard cap on distinct label sets held in memory. `principal` and `op` are
/// caller-influenced, so this is a bound on the damage a flood can do, not
/// a capacity estimate.
const MAX_SERIES: usize = 10_000;

// ── the usage ledger (17n, M1a) ──────────────────────────────────────────

/// One space's in-process usage since the last [`Metrics::usage_take`].
///
/// Per-replica, in memory, and drained on a tick by `usage.flush`
/// (`plan-production-ops.md` §3.2a) — never a stream, never hash-chained.
/// A usage total is an accounting quantity, recomputed and back-filled, and
/// the two existing tamper-evident streams are the wrong shape for
/// something that gets corrected.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct UsageCounters {
    /// Requests the door served, by [`ordo_api::CostClass::as_str`]. Every
    /// executor outcome counts — `Ran` and `AlreadyApplied` alike — because
    /// a lease replay still did the accounting work the class describes.
    /// A door refusal never reaches the executor, so it is never counted
    /// here: counting denials would price the act of being attacked.
    pub requests: BTreeMap<&'static str, u64>,
    /// Rows a `Ran` outcome returned. `AlreadyApplied` adds nothing — there
    /// is no fresh result to count rows in.
    pub rows_returned: u64,
    /// Trigger runs this replica finished or failed, from
    /// `TriggersRunner::run`.
    pub worker_runs: u64,
}

/// The bucket ladder, in seconds, shared by every histogram family.
///
/// One ladder rather than one per family, because a Prometheus histogram's
/// resolution is its buckets and a reader comparing two families should not
/// have to check whether they were bucketed the same way.
///
/// It spans four decades on purpose. The bottom is where an *uncontended*
/// append lock lives — one round trip to a local Postgres, tens of
/// microseconds — and if the bottom were 1ms the whole healthy population
/// would fall in one bucket and the series could not show contention
/// arriving. The top is 10s because a wait that long is already an
/// incident, and everything past it is `+Inf`, which says "worse than the
/// ladder can measure" and is the right thing to say.
pub const BUCKETS: &[f64] = &[
    0.000_1, 0.000_25, 0.000_5, 0.001, 0.002_5, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
    5.0, 10.0,
];

type Labels = Vec<(String, String)>;

/// One histogram series: per-bucket counts (**not** cumulative — the
/// cumulation happens at render, so an observation is one `+= 1`), plus the
/// sum and count Prometheus needs to compute an average and a rate.
#[derive(Clone, Debug)]
struct Bins {
    counts: Vec<u64>,
    sum: f64,
    count: u64,
}

impl Bins {
    fn new() -> Bins {
        // One extra slot for `+Inf`.
        Bins {
            counts: vec![0; BUCKETS.len() + 1],
            sum: 0.0,
            count: 0,
        }
    }

    fn observe(&mut self, seconds: f64) {
        let slot = BUCKETS
            .iter()
            .position(|b| seconds <= *b)
            .unwrap_or(BUCKETS.len());
        self.counts[slot] += 1;
        self.sum += seconds;
        self.count += 1;
    }
}

/// A histogram series as an observer sees it: cumulative bucket counts
/// paired with their `le` bounds, then the sum and the count.
///
/// `le` is [`f64::INFINITY`] for the last pair, which is what the
/// exposition format spells `+Inf`.
#[derive(Clone, Debug, PartialEq)]
pub struct HistogramSnapshot {
    pub name: String,
    pub labels: Vec<(String, String)>,
    pub buckets: Vec<(f64, u64)>,
    pub sum: f64,
    pub count: u64,
}

pub struct Metrics {
    series: Mutex<BTreeMap<(String, Labels), u64>>,
    /// The histogram families, keyed exactly as the counters are. A separate
    /// map rather than a sum type in one, because the two are read by
    /// different code at every site: a counter is added to and a histogram
    /// is observed into, and nothing wants to branch on which it got.
    histograms: Mutex<BTreeMap<(String, Labels), Bins>>,
    /// The gauges: the one family that may go down. Its own map so the
    /// counter family stays declarable as `# TYPE counter` to the letter,
    /// and so [`Metrics::set`]'s "not a gauge" rule keeps its meaning. K3
    /// added the first two.
    gauges: Mutex<BTreeMap<(String, Labels), u64>>,
    /// Whether high-cardinality series carry a `space` label.
    /// `implementation.md` makes per-space series opt-in; this is that
    /// switch, and it is read at the call site rather than at render so a
    /// deployment that leaves it off never *allocates* the series.
    space_labels: bool,
    dropped: AtomicU64,
    /// 7b: the evaluator for the mandatory deny-volume alert, over the
    /// counters this registry holds.
    ///
    /// It lives beside the counters rather than in `ordo-secevent` because
    /// this is the only place the counters are, and it is *not* the
    /// security-event stream: metrics are the alerting substrate, the stream
    /// is the tamper-evident record, and the module doc above says why
    /// conflating them would be wrong. The rule itself is
    /// `ordo_secevent::DenyVolumeMonitor`, which knows nothing about this
    /// registry — it is fed a snapshot, so a scraper's federated view could
    /// drive the same rule.
    alerts: Mutex<DenyVolumeMonitor>,
    /// The usage ledger (17n, M1a). `None` when the ledger is off, so an
    /// unwrapped deployment allocates nothing per space — every method
    /// below is a no-op against `None` rather than an empty map that still
    /// pays a lock.
    usage: Option<Mutex<BTreeMap<String, UsageCounters>>>,
    /// Observed op cost (`plan-tiers.md`, ruled 2026-10-23): per op and
    /// per tenant, bounded by its own settings. See
    /// [`crate::observed_cost`].
    observed: crate::observed_cost::ObservedCosts,
    /// When a sealed value last opened, per key epoch, and the alert rate
    /// limit. See [`crate::decrypt_seen`].
    decrypts: Arc<crate::decrypt_seen::DecryptLedger>,
}

impl Default for Metrics {
    /// Reads the deployment's threshold at construction
    /// (`ORDO_DENY_ALERT_THRESHOLD` / `ORDO_DENY_ALERT_WINDOW_SECS`).
    ///
    /// Unset is **not** "off": the alert is mandatory, so it runs on the
    /// built-in default and reports itself as unconfigured, which
    /// [`Metrics::render_alerts`] prints. A mandatory control that a missing
    /// variable silences is not a control.
    fn default() -> Metrics {
        Metrics {
            series: Mutex::new(BTreeMap::new()),
            histograms: Mutex::new(BTreeMap::new()),
            gauges: Mutex::new(BTreeMap::new()),
            // Off unless the deployment asks. Unset is "off" here and is
            // *not* off for the deny-volume rule above, and the difference
            // is the point: an alert a missing variable silences is not a
            // control, while a label a missing variable adds is a
            // cardinality bill nobody agreed to.
            space_labels: matches!(
                std::env::var("ORDO_METRICS_PER_SPACE").as_deref(),
                Ok("1" | "true" | "yes")
            ),
            dropped: AtomicU64::new(0),
            decrypts: Arc::default(),
            alerts: Mutex::new(DenyVolumeMonitor::new(DenyVolumeRule::from_env())),
            // Off unless the deployment asks, the same posture as
            // `space_labels` above and for the same reason: a ledger a
            // missing variable silences is a tuning knob, not a control.
            usage: matches!(
                std::env::var("ORDO_USAGE_LEDGER").as_deref(),
                Ok("1" | "true" | "yes")
            )
            .then(|| Mutex::new(BTreeMap::new())),
            observed: crate::observed_cost::ObservedCosts::new(
                crate::observed_cost::ObservedCostConfig::from_env(),
            ),
        }
    }
}

/// Take a lock, **ignoring poison**.
///
/// Every access here used to be `.lock().expect(...)`, and that was a
/// latent way to turn one fault into an outage.
///
/// **Three accesses still are**, and this doc used to claim otherwise:
/// `Metrics::bins`, `evaluate_alerts` and `render_alerts` below. The last two
/// are on `/security/alerts`, so the hazard the rest of this comment
/// describes is still reachable there. Recorded in `plan-runtime.md` §7
/// rather than changed here, because R3's rule for this file is that the
/// supervisor makes **its own** accounting panic-free and leaves the
/// registry's locking to the crate that owns it.
///
/// A `std::sync::Mutex` is
/// poisoned when a thread panics while holding it, and every subsequent
/// lock then panics too — so a panic anywhere inside a metrics update
/// would make the *next denial the door records* panic, and the one after
/// that, forever.
///
/// R3 makes that reachable rather than theoretical: the supervisor catches
/// worker panics and restarts, by design, so a worker that panicked while
/// recording a counter would convert a worker fault into a **door** fault
/// through the shared registry — the exact coupling `catch_unwind` was
/// added to prevent.
///
/// Ignoring poison is right here specifically because of what this state
/// is: counters and histogram buckets. A panic mid-update can leave a
/// count off by one; it cannot leave an invariant broken, because there is
/// no invariant across the map beyond "these are numbers". Losing one
/// increment is strictly better than losing every future one.
fn unpoisoned<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Metrics {
    pub fn new() -> Metrics {
        Metrics::default()
    }

    /// Build with an explicit rule — for a deployment that configures in
    /// code rather than through the environment, and for tests.
    pub fn with_alert_rule(rule: DenyVolumeRule) -> Metrics {
        Metrics {
            alerts: Mutex::new(DenyVolumeMonitor::new(rule)),
            ..Metrics::default()
        }
    }

    /// Opt into per-space series on the high-cardinality families, in code
    /// rather than through `ORDO_METRICS_PER_SPACE`.
    ///
    /// What it buys: the lock-wait family stops answering *"is the append
    /// lock biting"* and starts answering *"in which space"*, which is the
    /// question an operator has once the first answer is yes. What it costs
    /// is one histogram — [`BUCKETS`]`.len() + 2` series — per space that
    /// takes a write, charged against the same cap everything else is.
    pub fn with_per_space_series(mut self, on: bool) -> Metrics {
        self.space_labels = on;
        self
    }

    /// Whether this registry labels by space. Read by `/health/deep` so an
    /// operator looking at a lock-wait series with no `space` on it can tell
    /// *"per-space is off"* from *"one space wrote"*.
    pub fn per_space_series(&self) -> bool {
        self.space_labels
    }

    /// Opt into the usage ledger (17n, M1a), in code rather than through
    /// `ORDO_USAGE_LEDGER` — for a program that configures itself and for
    /// tests. `on: false` clears whatever the environment set, matching
    /// `with_per_space_series`'s shape.
    pub fn with_usage_ledger(mut self, on: bool) -> Metrics {
        self.usage = on.then(|| Mutex::new(BTreeMap::new()));
        self
    }

    /// Set the observed-cost caps in code rather than through
    /// `ORDO_OBSERVED_COST_MAX_OPS` and `ORDO_OBSERVED_COST_MAX_SPACE_ENTRIES`.
    /// Starts the table empty.
    pub fn with_observed_cost(
        mut self,
        config: crate::observed_cost::ObservedCostConfig,
    ) -> Metrics {
        self.observed = crate::observed_cost::ObservedCosts::new(config);
        self
    }

    /// The observed op cost table, which [`MeteredExecutor`] fills.
    pub fn observed_cost(&self) -> &crate::observed_cost::ObservedCosts {
        &self.observed
    }

    /// Whether the usage ledger is on. Read by `usage.flush`'s enabling
    /// condition and by `/health/deep`.
    pub fn usage_enabled(&self) -> bool {
        self.usage.is_some()
    }

    /// One request through the door, counted by its effective cost class.
    /// `rows` is `Some(rows.len())` on [`ordo_api::ExecOutcome::Ran`] and
    /// `None` otherwise — see [`MeteredExecutor::execute`].
    ///
    /// A no-op when the ledger is off, so `MeteredExecutor` can call this
    /// unconditionally without a branch at the call site.
    pub fn usage_request(&self, space: &str, class: ordo_api::CostClass, rows: Option<usize>) {
        let Some(usage) = &self.usage else { return };
        let mut ledger = unpoisoned(usage);
        let counters = ledger.entry(space.to_string()).or_default();
        *counters.requests.entry(class.as_str()).or_insert(0) += 1;
        if let Some(n) = rows {
            counters.rows_returned += n as u64;
        }
    }

    /// One trigger run this replica finished or failed, from
    /// `TriggersRunner::run`. An attempt executed a body either way.
    pub fn usage_worker_run(&self, space: &str) {
        let Some(usage) = &self.usage else { return };
        let mut ledger = unpoisoned(usage);
        ledger.entry(space.to_string()).or_default().worker_runs += 1;
    }

    /// Drain every space's counters, resetting the ledger to empty.
    ///
    /// A swap rather than a read, because `usage.flush` owns what happens
    /// to a count once it has been read: a crash between this call and the
    /// write it feeds loses at most one flush interval, and nothing is
    /// double-counted into the next one. Empty when the ledger is off.
    pub fn usage_take(&self) -> BTreeMap<String, UsageCounters> {
        let Some(usage) = &self.usage else {
            return BTreeMap::new();
        };
        std::mem::take(&mut *unpoisoned(usage))
    }

    /// This space's counters since the last [`Metrics::usage_take`],
    /// **without draining them** — the live half `PgQuotas::check` (17n M2)
    /// adds to the durable snapshot `usage.flush` last wrote to
    /// `$usage_replica`, so a quota reflects requests this replica has
    /// already served but not yet flushed. A peek rather than a take: the
    /// quota gate must not be the thing that empties the ledger
    /// `usage.flush` is depending on draining once, on its own tick.
    ///
    /// `UsageCounters::default()` when the ledger is off or this space has
    /// no live counters yet — never a panic, since a quota check runs on
    /// every request and a deployment that never wrapped a space must still
    /// answer.
    pub fn usage_peek(&self, space: &str) -> UsageCounters {
        let Some(usage) = &self.usage else {
            return UsageCounters::default();
        };
        unpoisoned(usage).get(space).cloned().unwrap_or_default()
    }

    /// Record one observation, in **seconds**, into a histogram family.
    ///
    /// Non-finite and negative inputs are recorded as zero rather than
    /// dropped. A negative duration means a clock went backwards, which is
    /// real and worth counting; silently discarding it would make the
    /// `_count` disagree with the number of appends that happened, and a
    /// histogram whose count is wrong is worse than one whose sum is.
    pub fn observe(&self, name: &str, labels: &[(&str, &str)], seconds: f64) {
        let mut key: Labels = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        key.sort();
        let seconds = if seconds.is_finite() && seconds > 0.0 {
            seconds
        } else {
            0.0
        };
        let mut hist = unpoisoned(&self.histograms);
        let entry = (name.to_string(), key);
        if let Some(bins) = hist.get_mut(&entry) {
            bins.observe(seconds);
        } else if hist.len() < MAX_SERIES {
            let mut bins = Bins::new();
            bins.observe(seconds);
            hist.insert(entry, bins);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Time blocked on the per-space chain-append advisory lock (5f).
    ///
    /// The `space` argument is always passed and only sometimes recorded —
    /// see [`Metrics::with_per_space_series`]. Taking it unconditionally
    /// means the decision lives here, in the registry that owns the
    /// cardinality budget, rather than at every call site that would
    /// otherwise have to remember the policy.
    pub fn chain_append_lock_wait(&self, space: &str, seconds: f64) {
        self.observe(CHAIN_APPEND_LOCK_WAIT, &self.space_label(space), seconds);
    }

    /// Time inside the append's critical section — the other half of the
    /// lock-wait fraction.
    pub fn chain_append_work(&self, space: &str, seconds: f64) {
        self.observe(CHAIN_APPEND_WORK, &self.space_label(space), seconds);
    }

    /// One space's checkout wait, from asking to holding a connection
    /// (20a). The healthy path, and the one that would cost a series per
    /// tenant, so it carries `space` only under the same opt-in the
    /// lock-wait family uses.
    pub fn pool_wait(&self, space: &str, seconds: f64) {
        self.observe(POOL_WAIT, &self.space_label(space), seconds);
    }

    /// This space reached its own share of the pool and queued behind
    /// itself. **Labelled by space unconditionally**, and see
    /// [`POOL_CAPPED`] for why that is not the cardinality bill the
    /// opt-in exists to refuse.
    pub fn pool_capped(&self, space: &str) {
        self.incr(POOL_CAPPED, &[("space", space)]);
    }

    /// A checkout for this space was refused. Labelled by space
    /// unconditionally, for [`POOL_CAPPED`]'s reason.
    pub fn pool_saturated(&self, space: &str) {
        self.incr(POOL_SATURATED, &[("space", space)]);
    }

    /// This space's quota check (17n M2) answered `Warn` or `Exceeded` for
    /// `quantity`. Labelled by space unconditionally — see [`QUOTA_OVER_LIMIT`].
    pub fn quota_over_limit(&self, space: &str, quantity: &str) {
        self.incr(
            QUOTA_OVER_LIMIT,
            &[("space", space), ("quantity", quantity)],
        );
    }

    /// One op's execution, by op and outcome. The p99 series.
    pub fn op_duration(&self, op: &str, outcome: &str, seconds: f64) {
        self.observe(OP_DURATION, &[("op", op), ("outcome", outcome)], seconds);
    }

    fn space_label<'a>(&self, space: &'a str) -> Vec<(&'a str, &'a str)> {
        if self.space_labels {
            vec![("space", space)]
        } else {
            Vec::new()
        }
    }

    /// Every histogram series, cumulative, name-then-label sorted.
    pub fn histogram_snapshot(&self) -> Vec<HistogramSnapshot> {
        let hist = unpoisoned(&self.histograms);
        hist.iter()
            .map(|((name, labels), bins)| {
                let mut running = 0u64;
                let buckets = bins
                    .counts
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        running += n;
                        (BUCKETS.get(i).copied().unwrap_or(f64::INFINITY), running)
                    })
                    .collect();
                HistogramSnapshot {
                    name: name.clone(),
                    labels: labels.clone(),
                    buckets,
                    sum: bins.sum,
                    count: bins.count,
                }
            })
            .collect()
    }

    /// How many observations a histogram series holds. `None` when nothing
    /// has ever observed into it — which is not the same as zero, and a
    /// caller that cannot tell those apart cannot tell an instrumented
    /// quiet path from an uninstrumented busy one.
    pub fn histogram_count(&self, name: &str, labels: &[(&str, &str)]) -> Option<u64> {
        self.bins(name, labels).map(|b| b.count)
    }

    /// Total observed seconds in a histogram series.
    pub fn histogram_sum(&self, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
        self.bins(name, labels).map(|b| b.sum)
    }

    fn bins(&self, name: &str, labels: &[(&str, &str)]) -> Option<Bins> {
        let mut key: Labels = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        key.sort();
        self.histograms
            .lock()
            .expect("metrics")
            .get(&(name.to_string(), key))
            .cloned()
    }

    /// The `q`-quantile of a histogram series, in seconds.
    ///
    /// **This is Prometheus's `histogram_quantile`, deliberately** — the
    /// same bucket search and the same linear interpolation inside the
    /// chosen bucket — so a p99 asserted in a test and a p99 read off a
    /// dashboard are one number arrived at one way. Copying the algorithm
    /// also copies its honesty: the answer is bounded by the ladder's
    /// resolution, and
    ///
    /// - a quantile falling in the `+Inf` bucket returns the **largest
    ///   finite bound** rather than infinity, because the only truthful
    ///   thing a bucketed histogram can say there is *"at least this"*;
    /// - a series with no observations returns `None`, never `0.0`. A
    ///   quantile over nothing is not a fast quantile.
    pub fn quantile(&self, name: &str, labels: &[(&str, &str)], q: f64) -> Option<f64> {
        let bins = self.bins(name, labels)?;
        if bins.count == 0 || !(0.0..=1.0).contains(&q) {
            return None;
        }
        let target = q * bins.count as f64;
        let mut running = 0u64;
        for (i, n) in bins.counts.iter().enumerate() {
            running += n;
            if (running as f64) < target {
                continue;
            }
            let Some(upper) = BUCKETS.get(i).copied() else {
                // The `+Inf` bucket: everything measurable is below the
                // ladder's top, so that is the strongest true statement.
                return BUCKETS.last().copied();
            };
            let lower = if i == 0 { 0.0 } else { BUCKETS[i - 1] };
            let before = (running - n) as f64;
            let within = if *n == 0 {
                0.0
            } else {
                ((target - before) / *n as f64).clamp(0.0, 1.0)
            };
            return Some(lower + (upper - lower) * within);
        }
        BUCKETS.last().copied()
    }

    /// Add to a counter. Labels are sorted so the same set always names the
    /// same series regardless of the order a call site wrote them.
    pub fn incr(&self, name: &str, labels: &[(&str, &str)]) {
        let mut key: Labels = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        key.sort();
        let mut series = unpoisoned(&self.series);
        let entry = (name.to_string(), key);
        if let Some(n) = series.get_mut(&entry) {
            *n += 1;
        } else if series.len() < MAX_SERIES {
            series.insert(entry, 1);
        } else {
            // Past the cap: count the loss rather than the event, so the
            // gap is visible instead of silent.
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Add `n` to a counter in one lock acquisition.
    ///
    /// [`Metrics::incr`] in a loop is the same thing at N times the lock
    /// cost, and the drainer commits entries in batches of hundreds — this
    /// exists so a batch is one acquisition rather than a batch's worth.
    pub fn incr_by(&self, name: &str, labels: &[(&str, &str)], n: u64) {
        if n == 0 {
            return;
        }
        let mut key: Labels = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        key.sort();
        let mut series = unpoisoned(&self.series);
        let entry = (name.to_string(), key);
        if let Some(v) = series.get_mut(&entry) {
            *v += n;
        } else if series.len() < MAX_SERIES {
            series.insert(entry, n);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Publish a total that is authoritative somewhere else.
    ///
    /// A deliberate hole in an otherwise counter-only registry, and it is
    /// narrow on purpose: the security drainer's `observed` / `dropped` /
    /// `pruned` totals are **already monotone counters**, kept by
    /// `ordo_secevent::Recorder` and by the drain's own health. Mirroring
    /// them with `incr` would need a delta the caller does not have, and
    /// getting that delta wrong is how a loss counter under-reports —
    /// which is the one direction it must never be wrong in. So the
    /// registry copies the authoritative number instead of re-deriving it.
    ///
    /// This is **not** a gauge and must not be used as one: a series that
    /// can go down would break every `increase()` and `rate()` an operator
    /// writes over this endpoint, and the whole registry is declared
    /// `# TYPE counter`.
    pub fn set(&self, name: &str, labels: &[(&str, &str)], value: u64) {
        let mut key: Labels = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        key.sort();
        let mut series = unpoisoned(&self.series);
        let entry = (name.to_string(), key);
        if let Some(v) = series.get_mut(&entry) {
            // Monotone by construction, and clamped so a caller that
            // regresses cannot make a counter go backwards.
            *v = (*v).max(value);
        } else if series.len() < MAX_SERIES {
            series.insert(entry, value);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Set a gauge. The value replaces what was there: a gauge answers
    /// "how much now", so it goes down as readily as up, which is exactly
    /// what [`Metrics::set`] refuses to do for a counter.
    ///
    /// Charged against [`MAX_SERIES`] like everything else; a label set
    /// past the cap is dropped and counted, never silently pinned.
    pub fn gauge(&self, name: &str, labels: &[(&str, &str)], value: u64) {
        let mut key: Labels = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        key.sort();
        let mut gauges = unpoisoned(&self.gauges);
        let entry = (name.to_string(), key);
        if let Some(v) = gauges.get_mut(&entry) {
            *v = value;
        } else if gauges.len() < MAX_SERIES {
            gauges.insert(entry, value);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Every gauge series, sorted by name then labels.
    pub fn gauge_snapshot(&self) -> Vec<(String, Labels, u64)> {
        unpoisoned(&self.gauges)
            .iter()
            .map(|((n, l), v)| (n.clone(), l.clone(), *v))
            .collect()
    }

    /// One door refusal, attributed. This is the mandatory
    /// deny-volume-per-principal-per-space counter.
    ///
    /// `principal` is whatever the door could prove. A token that did not
    /// verify proves nothing, so it lands under `unknown` with
    /// `principal_kind="unverified"` — deliberately one bucket, because a
    /// per-token series would let a caller mint unbounded series by
    /// guessing, and because an unverifiable token has no principal to
    /// attribute to in the first place.
    pub fn deny(&self, space: &str, principal: &str, principal_kind: &str, op: &str) {
        self.incr(
            DENIALS,
            &[
                ("space", space),
                ("principal", principal),
                ("principal_kind", principal_kind),
                ("op", op),
            ],
        );
    }

    /// One key release, by space and purpose.
    ///
    /// **The gap, named:** `implementation.md` asks for release breadth
    /// *per principal* ("distinct spaces per principal"), and this counts
    /// per space and purpose. The reason is the interface:
    /// `KeyProvider::release(space, purpose, correlation)` never sees a
    /// principal — the correlation id is the join key, and resolving it to
    /// a principal needs the structured request log, which does not exist
    /// yet. So the breadth alert can be written over spaces-per-correlation
    /// today and needs that log to become spaces-per-principal.
    pub fn decrypts(&self) -> &Arc<crate::decrypt_seen::DecryptLedger> {
        &self.decrypts
    }

    pub fn key_release(&self, space: &str, purpose: &str) {
        self.incr(KEY_RELEASES, &[("space", space), ("purpose", purpose)]);
    }

    /// Every series, name-then-label sorted. Used by `/health/deep` and by
    /// the Prometheus rendering.
    pub fn snapshot(&self) -> Vec<(String, Labels, u64)> {
        let series = unpoisoned(&self.series);
        let mut out: Vec<(String, Labels, u64)> = series
            .iter()
            .map(|((n, l), v)| (n.clone(), l.clone(), *v))
            .collect();
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            out.push((SERIES_DROPPED.to_string(), Vec::new(), dropped));
        }
        out
    }

    /// 7b: evaluate the mandatory deny-volume rule against the counters as
    /// they stand, and return every `(space, principal)` currently over it.
    ///
    /// Stateful by necessity: a counter is monotone, so an increase needs
    /// two observations. Each call is one observation, which means the
    /// alert's resolution is however often something calls this — the
    /// `/security/alerts` route, or a deployment's own tick.
    pub fn evaluate_alerts(&self, now: u64) -> Vec<Firing> {
        let snapshot = self.snapshot();
        self.alerts
            .lock()
            .expect("alerts")
            .evaluate(&snapshot, Timestamp(now))
    }

    /// The operator-facing rendering: the rule, whether the deployment set
    /// it, what this process can and cannot see, and every firing.
    pub fn render_alerts(&self, now: u64) -> String {
        let snapshot = self.snapshot();
        self.alerts
            .lock()
            .expect("alerts")
            .render(&snapshot, Timestamp(now))
    }

    /// The rule in force, for `/health/deep` and for tests.
    pub fn alert_rule(&self) -> DenyVolumeRule {
        unpoisoned(&self.alerts).rule()
    }

    /// The Prometheus text exposition format. Not an exporter — no HTTP
    /// server, no registry of its own — just the rendering, so an ordinary
    /// scrape of the access-controlled `/metrics` route works.
    ///
    /// Counters first, then histograms, then gauges, each family under its
    /// own `# TYPE`.
    /// A histogram renders as the format requires it to: cumulative
    /// `_bucket{le="…"}` rows ending in `le="+Inf"`, then `_sum` and
    /// `_count`. Anything less and `histogram_quantile` cannot read it,
    /// which would leave the p99 unanswerable for a formatting reason.
    pub fn render_prometheus(&self) -> String {
        let mut out = String::new();
        let mut last: Option<String> = None;
        for (name, labels, value) in self.snapshot() {
            if last.as_deref() != Some(name.as_str()) {
                out.push_str(&format!("# TYPE {name} counter\n"));
                last = Some(name.clone());
            }
            out.push_str(&format!(
                "{}{} {value}\n",
                name,
                render_labels(&labels, &[])
            ));
        }
        let mut last: Option<String> = None;
        for h in self.histogram_snapshot() {
            if last.as_deref() != Some(h.name.as_str()) {
                out.push_str(&format!("# TYPE {} histogram\n", h.name));
                last = Some(h.name.clone());
            }
            for (le, cumulative) in &h.buckets {
                let le = if le.is_finite() {
                    format!("{le}")
                } else {
                    "+Inf".to_string()
                };
                out.push_str(&format!(
                    "{}_bucket{} {cumulative}\n",
                    h.name,
                    render_labels(&h.labels, &[("le", &le)])
                ));
            }
            out.push_str(&format!(
                "{}_sum{} {}\n",
                h.name,
                render_labels(&h.labels, &[]),
                h.sum
            ));
            out.push_str(&format!(
                "{}_count{} {}\n",
                h.name,
                render_labels(&h.labels, &[]),
                h.count
            ));
        }
        let mut last: Option<String> = None;
        for (name, labels, value) in self.gauge_snapshot() {
            if last.as_deref() != Some(name.as_str()) {
                out.push_str(&format!("# TYPE {name} gauge\n"));
                last = Some(name.clone());
            }
            out.push_str(&format!(
                "{}{} {value}\n",
                name,
                render_labels(&labels, &[])
            ));
        }
        self.observed.render_prometheus(&mut out, self.space_labels);
        out
    }
}

/// `{k="v",…}`, or the empty string when there is nothing to render.
/// `extra` is appended last so `le` sorts after the series' own labels,
/// which is the convention every exposition example follows.
fn render_labels(labels: &[(String, String)], extra: &[(&str, &str)]) -> String {
    if labels.is_empty() && extra.is_empty() {
        return String::new();
    }
    let rendered: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{k}=\"{}\"", escape(v)))
        .chain(extra.iter().map(|(k, v)| format!("{k}=\"{}\"", escape(v))))
        .collect();
    format!("{{{}}}", rendered.join(","))
}

/// Wall clock, seconds. The stream stamps server-observed instants — never
/// a client-claimed one — and this is the only clock the decorator has.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

// ── the chain-append lock's observer ─────────────────────────────────────

/// The registry IS the sink `ordo-exec` reports lock timings to (5f).
///
/// No decorator here, unlike [`MeteredKeys`]: the trait exists precisely
/// because `ordo-exec` cannot name this type, and there is nothing to wrap
/// — a deployment writes
/// `pool.observing(metrics.clone())` and every transaction begun from that
/// handle reports.
///
/// Both methods run inside an open transaction and one of them under the
/// append lock, so both must stay what they are: a `Mutex` and a `Vec`
/// index. See `ordo_exec::metrics` for why that budget is not negotiable.
impl ordo_exec::metrics::AppendObserver for Metrics {
    fn lock_wait(&self, space: &str, seconds: f64) {
        self.chain_append_lock_wait(space, seconds);
    }

    fn append_work(&self, space: &str, seconds: f64) {
        self.chain_append_work(space, seconds);
    }

    // The pool's own three (20a). The same seam because it is the same
    // question asked one layer down, which is who is holding the thing
    // everybody needs, and because the handle already carries this
    // observer: a second trait would be a second thing for a deployment
    // to wire and a second thing to forget.
    fn pool_wait(&self, space: &str, seconds: f64) {
        Metrics::pool_wait(self, space, seconds);
    }

    fn pool_capped(&self, space: &str) {
        Metrics::pool_capped(self, space);
    }

    fn pool_saturated(&self, space: &str) {
        Metrics::pool_saturated(self, space);
    }
}

// ── the metered OpExecutor ───────────────────────────────────────────────

/// An [`ordo_api::OpExecutor`] that times its inner executor into
/// `ordo_op_duration_seconds{op,outcome}` and otherwise does nothing at all.
///
/// A decorator for [`MeteredKeys`]'s reason and one more. The reason it
/// shares: a deployment that wants no metering simply does not wrap. The
/// reason it does not: `ordo-api`'s router is deliberately built on five
/// traits and **no sibling-crate dependency** (see [`crate::serve`]), so
/// adding a metrics handle to `ApiState` would be a sixth boundary member
/// invented to move a stopwatch. Wrapping the one trait that already
/// crosses the boundary costs nothing and moves nothing.
///
/// # What this span does NOT include, stated because a p99 read as
/// end-to-end when it is not is a wrong number rather than a partial one
///
/// The router's pipeline is *registry lookup → method enforcement →
/// envelope split → space bind → **access check** → param bind → refinement
/// validation → cursor decode → execute*. This times the last step. So the
/// series excludes token verification, the access decision, parameter
/// binding and refinement validation — and, on the way out, response
/// serialization and the attestation read.
///
/// That makes it the right series for *"which op is slow"* and the wrong
/// one for *"how long does a caller wait"*. The second needs a span at the
/// router, and the router is where `ordo.correlation` and the OpenTelemetry
/// span the doc pins would also live; none of that exists, and this does
/// not pretend to be it.
///
/// # The outcome label
///
/// `ok`, `already_applied`, or the refusal's **wire class** — never its
/// message. The class is a closed inventory ([`ordo_api::ErrorCode`]) so
/// the label set is bounded by construction, which is the property a label
/// on a histogram needs and a message could never have.
pub struct MeteredExecutor {
    pub inner: Arc<dyn ordo_api::OpExecutor>,
    pub metrics: Arc<Metrics>,
}

impl MeteredExecutor {
    pub fn new(inner: Arc<dyn ordo_api::OpExecutor>, metrics: Arc<Metrics>) -> MeteredExecutor {
        MeteredExecutor { inner, metrics }
    }
}

impl ordo_api::OpExecutor for MeteredExecutor {
    fn execute<'a>(
        &'a self,
        op: &'a ordo_api::OpSpec,
        space: &'a str,
        bound: ordo_api::BoundParams,
        refine: Option<ordo_api::Refinement>,
        page: Option<ordo_api::PageCursor>,
        lease: Option<ordo_api::ReplayLease>,
    ) -> futures::future::BoxFuture<'a, Result<ordo_api::ExecOutcome, ordo_api::ExecError>> {
        self.execute_sized(op, space, bound, refine, page, lease, None)
    }

    fn execute_sized<'a>(
        &'a self,
        op: &'a ordo_api::OpSpec,
        space: &'a str,
        bound: ordo_api::BoundParams,
        refine: Option<ordo_api::Refinement>,
        page: Option<ordo_api::PageCursor>,
        lease: Option<ordo_api::ReplayLease>,
        page_rows: Option<usize>,
    ) -> futures::future::BoxFuture<'a, Result<ordo_api::ExecOutcome, ordo_api::ExecError>> {
        Box::pin(async move {
            // The class this request runs at (`api-engine-design.md` §7.1),
            // taken before the refinement moves into the executor.
            let effective = match &refine {
                Some(r) => ordo_api::refine::effective_class(op, r),
                None => op.cost.class,
            };
            let started = std::time::Instant::now();
            let out = self
                .inner
                .execute_sized(op, space, bound, refine, page, lease, page_rows)
                .await;
            // A refusal took real time and is part of the service view: an
            // op whose failures are slow is exactly the op an operator
            // wants to find, and dropping the error path would hide it.
            let outcome = match &out {
                Ok(ordo_api::ExecOutcome::Ran(_)) => "ok",
                Ok(ordo_api::ExecOutcome::AlreadyApplied) => "already_applied",
                Err(e) => e.code.as_str(),
            };
            let elapsed = started.elapsed();
            self.metrics
                .op_duration(&op.name, outcome, elapsed.as_secs_f64());
            // 17n M1a: every executor outcome is a request; only `Ran`
            // carries rows to count. A door refusal never reaches here,
            // which is the rule "counting denials prices the act of being
            // attacked" needs, for free — this decorator sits inside that
            // boundary, not outside it.
            let rows = match &out {
                Ok(ordo_api::ExecOutcome::Ran(r)) => Some(r.rows.len()),
                _ => None,
            };
            self.metrics.usage_request(space, effective, rows);
            // Observed cost (`plan-tiers.md`): what this op cost, at the
            // class it really ran at.
            self.metrics.observed_cost().record(
                &op.name,
                space,
                crate::observed_cost::Sample {
                    micros: u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
                    rows: rows.unwrap_or(0) as u64,
                    ok: out.is_ok(),
                    registered: op.cost.class.as_str(),
                    effective: effective.as_str(),
                },
            );
            out
        })
    }
}

// ── the metered KeyProvider ──────────────────────────────────────────────

/// A [`ordo_crypto::KeyProvider`] that counts its releases and otherwise
/// does nothing at all.
///
/// A decorator rather than a change to the providers: custody is the one
/// component where "it also does some bookkeeping" is a bad property, and
/// a deployment that wants no metering simply does not wrap. It counts
/// `release` only — `seal`/`open`/`blind_index` all run under a lease that
/// was already counted, so counting them again would inflate breadth by
/// row count instead of measuring it by grant.
/// # Two artifacts, one call
///
/// A key release is both an *alerting* fact (breadth, against a plan) and a
/// *security event* (`implementation.md` lists key releases among the
/// deployment security log's contents). So the decorator writes to both: the
/// counter, and — when a recorder is attached — the security-event stream
/// of the space the release was for (owner, 2026-09-14,
/// `key-release-home`), carrying the lease's own correlation id. That is a
/// real join today: the release and any denial in the same request share
/// one correlation.
///
/// The serving runtime wraps its custody in this once, at boot, with the
/// door's recorder attached (`runtime::meter_custody`, 25ab). Every
/// component that holds the keys holds that one handle.
///
/// The subject is the platform, not a principal, and that is not laziness:
/// `release(space, purpose, correlation)` genuinely does not see one. The
/// correlation is the only handle back to whoever asked, which is exactly
/// why `key_release`'s doc calls the missing structured request log the
/// thing that turns breadth-per-correlation into breadth-per-principal.
pub struct MeteredKeys {
    pub inner: std::sync::Arc<dyn ordo_crypto::KeyProvider>,
    pub metrics: std::sync::Arc<Metrics>,
    /// Where the security event goes. `None` meters without recording —
    /// which is what a deployment that has not wired a stream drainer yet
    /// should have, rather than a queue that grows forever.
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
}

impl MeteredKeys {
    /// Metering only, no stream. The shape every existing call site had.
    pub fn new(
        inner: std::sync::Arc<dyn ordo_crypto::KeyProvider>,
        metrics: std::sync::Arc<Metrics>,
    ) -> MeteredKeys {
        MeteredKeys {
            inner,
            metrics,
            stream: None,
        }
    }

    /// What an open answered, kept for the deployment owner: a success
    /// advances the epoch's last-success time (memory only, no write per
    /// read), and a failure with the key present is counted, noted, and
    /// recorded as a `SealedUnreadable` event in the space's stream, at most
    /// once per entity, field and epoch per window. A missing epoch or a
    /// store fault is neither: it says nothing about the value.
    fn note_open(
        &self,
        lease: &ordo_crypto::KeyLease,
        entity: &str,
        field: &str,
        epoch: Option<ordo_crypto::Epoch>,
        out: &Result<Vec<u8>, ordo_crypto::KeyError>,
    ) {
        let Some(epoch) = epoch else { return };
        let space = lease.space.0.as_str();
        let now = now_secs();
        let ledger = self.metrics.decrypts();
        match out {
            Ok(_) => ledger.succeeded(space, epoch, now),
            Err(e) if e.is_unreadable() => {
                ledger.failed(space, epoch, now);
                self.metrics.incr(
                    SEALED_UNREADABLE,
                    &[("space", space), ("epoch", &epoch.to_string())],
                );
                if let Some(stream) = &self.stream {
                    if ledger.alert_due(space, entity, field, epoch, now) {
                        stream.observe(
                            ordo_secevent::Observation {
                                space: Some(ordo_commandlog::SpaceId(space.to_string())),
                                subject: ordo_secevent::Subject::platform("keyprovider"),
                                op: String::new(),
                                kind: ordo_secevent::EventKind::SealedUnreadable {
                                    entity: entity.to_string(),
                                    field: field.to_string(),
                                    epoch,
                                },
                                at: Timestamp(now),
                                correlation: ordo_secevent::correlation::adopt(&lease.correlation),
                            },
                            ordo_secevent::Relationship::Custodian,
                        );
                    }
                }
            }
            Err(_) => {}
        }
    }

    /// Also record each release as a deployment security event.
    pub fn with_stream(mut self, stream: Arc<ordo_secevent::Recorder>) -> MeteredKeys {
        self.stream = Some(stream);
        self
    }
}

impl ordo_crypto::KeyProvider for MeteredKeys {
    fn release(
        &self,
        space: &ordo_crypto::SpaceId,
        purpose: ordo_crypto::Purpose,
        correlation: &str,
    ) -> Result<ordo_crypto::KeyLease, ordo_crypto::KeyError> {
        let lease = self.inner.release(space, purpose, correlation)?;
        // Counted on success only: a refused release is not custody
        // touched, and counting it would make a provider's own refusals
        // look like breadth.
        self.metrics.key_release(&space.0, purpose.as_str());
        if let Some(stream) = &self.stream {
            stream.observe(
                ordo_secevent::Observation {
                    // The space the release touched, and its home: the
                    // `Custodian` relationship below routes the release to
                    // that space's own stream. Owner-ruled 2026-09-14
                    // (`key-release-home`): the tenant is the party who
                    // should see that the platform opened their key, for
                    // which purpose and when. This used to be recorded in
                    // the deployment stream with the space kept only as the
                    // entry's object (`plan-opslog.md` §0.4 / D2).
                    space: Some(ordo_commandlog::SpaceId(space.0.clone())),
                    subject: ordo_secevent::Subject::platform("keyprovider"),
                    op: String::new(),
                    kind: ordo_secevent::EventKind::KeyReleased {
                        purpose: purpose.as_str().to_string(),
                    },
                    at: Timestamp(now_secs()),
                    correlation: ordo_secevent::correlation::adopt(&lease.correlation),
                },
                ordo_secevent::Relationship::Custodian,
            );
        }
        Ok(lease)
    }

    // Forwarded, and forwarded because the trait made them REQUIRED. A
    // defaulted chunk sealer would have compiled here and silently left a
    // metered deployment unable to seal a blob above the inline cap —
    // which is exactly the failure mode `destroy_space`'s doc argues a
    // default produces in a wrapping provider.
    //
    // Not separately metered: a chunk is not a key release. The release
    // that authorized the whole object was counted once, on `release`, and
    // counting 1600 chunks of a 100 MB upload would make one upload look
    // like breadth across a tenant's custody.
    fn seal_at(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.inner.seal_at(l, e, f, a, v)
    }
    fn open_at(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        let out = self.inner.open_at(l, e, f, a, v);
        self.note_open(l, e, f, ordo_crypto::envelope_epoch(v), &out);
        out
    }
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        lease: &ordo_crypto::KeyLease,
        entity: &str,
        field: &str,
        anchor: ordo_crypto::Anchor<'_>,
        epoch: ordo_crypto::Epoch,
        base: &ordo_crypto::NonceBase,
        counter: u32,
        last: bool,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.inner.seal_chunk(
            lease, entity, field, anchor, epoch, base, counter, last, plaintext,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        lease: &ordo_crypto::KeyLease,
        entity: &str,
        field: &str,
        anchor: ordo_crypto::Anchor<'_>,
        epoch: ordo_crypto::Epoch,
        base: &ordo_crypto::NonceBase,
        counter: u32,
        last: bool,
        chunk: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        let out = self.inner.open_chunk(
            lease, entity, field, anchor, epoch, base, counter, last, chunk,
        );
        self.note_open(lease, entity, field, Some(epoch), &out);
        out
    }

    fn blind_index(
        &self,
        space: &ordo_crypto::SpaceId,
        entity: &str,
        field: &str,
        normalized: ordo_crypto::BlindValue<'_>,
    ) -> Result<[u8; 32], ordo_crypto::KeyError> {
        self.inner.blind_index(space, entity, field, normalized)
    }

    fn current_epoch(
        &self,
        space: &ordo_crypto::SpaceId,
    ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.inner.current_epoch(space)
    }

    fn rotate(
        &self,
        space: &ordo_crypto::SpaceId,
        reason: &str,
    ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.inner.rotate(space, reason)
    }

    fn destroy_epoch(
        &self,
        space: &ordo_crypto::SpaceId,
        epoch: ordo_crypto::Epoch,
    ) -> Result<(), ordo_crypto::KeyError> {
        self.inner.destroy_epoch(space, epoch)
    }

    /// Forwarded, not defaulted. A metering wrapper that answered this on
    /// its own would report a successful erasure while the real provider's
    /// keys sat untouched underneath it — a deletion that lies, which is
    /// the failure mode the required trait method exists to make
    /// impossible.
    ///
    /// Not counted as a release: nothing was released. Space deletion's
    /// record is the command group the op appends, which names the owner.
    fn destroy_space(&self, space: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
        self.inner.destroy_space(space)
    }

    /// Forwarded for [`MeteredKeys::destroy_space`]'s reason, read the
    /// other way: a wrapper answering on its own would report custody that
    /// does not exist, and the first write into the new space would be the
    /// place that failure surfaced.
    ///
    /// Not counted as a release either. Provisioning releases no key; the
    /// deployment's record of a space coming into existence is the founding
    /// member row, which is attributed.
    fn create_space(&self, space: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
        self.inner.create_space(space)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_crypto::{KeyProvider, Purpose, SpaceId as CSpaceId};
    use ordo_secevent::{EventKind, Recorder, Scope, Subject};

    fn metrics() -> Arc<Metrics> {
        Arc::new(Metrics::with_alert_rule(DenyVolumeRule::new(50, 300)))
    }

    /// 7b, end to end over the real registry: the counter the door writes is
    /// the counter the rule reads, and a burst crosses the line a trickle
    /// does not.
    #[test]
    fn a_burst_fires_the_mandatory_rule_and_a_trickle_does_not() {
        let m = metrics();
        // A baseline observation, so the monitor has something to measure
        // an increase against — and so does not mistake the first sample
        // for the whole history.
        assert!(m.evaluate_alerts(1_000).is_empty());

        // The ordinary trickle: one principal, a handful of refusals.
        for _ in 0..8 {
            m.deny("spc_alpha", "usr_normal", "member", "openTasks");
        }
        assert!(
            m.evaluate_alerts(1_060).is_empty(),
            "eight denials in a minute is a stale tab, not an incident"
        );

        // The sweep: one principal, one space, many ops. Per op every
        // series is small, which is exactly why the rule aggregates.
        for i in 0..60 {
            for _ in 0..3 {
                m.deny("spc_alpha", "usr_probe", "member", &format!("op{i}"));
            }
        }
        let firing = m.evaluate_alerts(1_120);
        assert_eq!(firing.len(), 1);
        assert_eq!(firing[0].principal, "usr_probe");
        assert_eq!(firing[0].space, "spc_alpha");
        assert_eq!(firing[0].count, 180);

        // And the operator surface says the rule, the firing, and what this
        // process cannot see.
        let text = m.render_alerts(1_180);
        assert!(text.contains("usr_probe"));
        assert!(text.contains("No cross-replica aggregation"));
        assert!(!text.contains("MISCONFIGURED"), "this rule was set");
    }

    /// The two artifacts stay two artifacts: the counter keeps the per-op
    /// detail the alert deliberately aggregates away.
    #[test]
    fn the_counter_keeps_the_per_op_detail_the_alert_aggregates_away() {
        let m = metrics();
        m.deny("spc_alpha", "usr_1", "member", "openTasks");
        m.deny("spc_alpha", "usr_1", "member", "listOrders");
        let text = m.render_prometheus();
        assert!(text.contains("op=\"openTasks\""));
        assert!(text.contains("op=\"listOrders\""));
    }

    /// An unconfigured deployment still alerts, and the surface says so —
    /// a mandatory control a missing variable silences is not a control.
    #[test]
    fn an_unconfigured_deployment_still_alerts_and_is_told() {
        let m = Metrics::with_alert_rule(DenyVolumeRule::default());
        assert!(!m.alert_rule().configured);
        for i in 0..200 {
            m.deny("spc_alpha", "usr_probe", "member", &format!("op{i}"));
        }
        let text = m.render_alerts(10);
        assert!(text.contains("MISCONFIGURED"));
        assert!(text.contains("FIRING deny-volume"));
    }

    /// A key release is metered *and* recorded, in the stream of the space
    /// whose key was opened, under the lease's own correlation.
    #[test]
    fn a_metered_release_is_recorded_in_the_space_it_touched() {
        let m = metrics();
        let stream = Arc::new(Recorder::default());
        let keys = MeteredKeys::new(
            Arc::new(ordo_crypto::TestProvider::with_space("spc_alpha")),
            m.clone(),
        )
        .with_stream(stream.clone());

        let lease = keys
            .release(&CSpaceId("spc_alpha".into()), Purpose::Read, "cor_abc123")
            .expect("release");
        assert_eq!(lease.correlation, "cor_abc123");

        let entries = stream.drain_all(Timestamp(0));
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].kind,
            EventKind::KeyReleased {
                purpose: "read".into()
            }
        );
        assert_eq!(
            entries[0].scope,
            Scope::Space(ordo_commandlog::SpaceId("spc_alpha".into())),
            "the tenant is the party who should see their key being opened \
             (key-release-home, 2026-09-14)"
        );
        assert_eq!(entries[0].subject, Subject::platform("keyprovider"));
        assert_eq!(entries[0].correlations[0].0, "cor_abc123");
        // The counter moved too — two artifacts, one call.
        assert!(m.render_prometheus().contains("ordo_key_release_total"));
    }

    /// Owner, 2026-10-23: a value that fails to open with its key present is
    /// unreadable, possibly tampered, and the deployment owner is told once;
    /// a destroyed epoch is not that; a good read advances the epoch's
    /// last-success time.
    #[test]
    fn a_tampered_value_records_one_event_and_a_destroyed_epoch_none() {
        let m = metrics();
        let stream = Arc::new(Recorder::default());
        let keys = MeteredKeys::new(
            Arc::new(ordo_crypto::TestProvider::with_space("spc_alpha")),
            m.clone(),
        )
        .with_stream(stream.clone());
        let space = CSpaceId("spc_alpha".into());
        let lease = keys.release(&space, Purpose::Write, "cor_1").unwrap();
        let anchor = ordo_crypto::Anchor::Row("ord_1");
        let sealed = keys
            .seal_at(&lease, "orders", "memo", anchor, b"secret")
            .unwrap();
        let epoch = ordo_crypto::envelope_epoch(&sealed).unwrap();
        let _ = stream.drain_all(Timestamp(0));

        // A good read advances the time.
        assert!(m.decrypts().snapshot().is_empty());
        keys.open_at(&lease, "orders", "memo", anchor, &sealed)
            .unwrap();
        let seen = m.decrypts().snapshot();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].2.last_success > 0 && seen[0].2.failing_since == 0);

        // A flipped byte: unreadable, one event however many times it is read.
        let mut bad = sealed.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        for _ in 0..5 {
            let e = keys
                .open_at(&lease, "orders", "memo", anchor, &bad)
                .unwrap_err();
            assert!(e.is_unreadable() && !e.is_destroyed());
            assert!(!e.to_string().contains("secret"));
        }
        let entries = stream.drain_all(Timestamp(0));
        assert_eq!(entries.len(), 1, "rate-limited to one event");
        assert_eq!(
            entries[0].kind,
            EventKind::SealedUnreadable {
                entity: "orders".into(),
                field: "memo".into(),
                epoch,
            }
        );
        assert!(m
            .render_prometheus()
            .contains("ordo_sealed_unreadable_total"));
        let seen = m.decrypts().snapshot();
        assert!(seen[0].2.failing_since > 0 && seen[0].2.last_failure > 0);

        // A good read clears the failing mark.
        keys.open_at(&lease, "orders", "memo", anchor, &sealed)
            .unwrap();
        assert_eq!(m.decrypts().snapshot()[0].2.failing_since, 0);

        // A destroyed epoch still answers destroyed, and records nothing.
        keys.rotate(&space, "test").unwrap();
        keys.destroy_epoch(&space, epoch).unwrap();
        let lease = keys.release(&space, Purpose::Read, "cor_2").unwrap();
        let _ = stream.drain_all(Timestamp(0));
        let e = keys
            .open_at(&lease, "orders", "memo", anchor, &sealed)
            .unwrap_err();
        assert!(e.is_destroyed() && !e.is_unreadable(), "{e}");
        assert!(stream.drain_all(Timestamp(0)).is_empty());
    }

    /// `implementation.md:488-492` specifies a key release as carrying
    /// "(space, purpose, correlation)". It now carries the space as its home
    /// K3: the first gauge family. A gauge goes down, renders under its
    /// own `# TYPE … gauge` after the counters and histograms, and is
    /// charged against the same series cap.
    #[test]
    fn a_gauge_can_go_down_and_renders_as_a_gauge() {
        let m = Metrics::new();
        m.gauge(
            RESEAL_REMAINING,
            &[("space", "acme"), ("population", "payloads")],
            7,
        );
        m.gauge(
            RESEAL_REMAINING,
            &[("space", "acme"), ("population", "payloads")],
            3,
        );
        m.gauge(RESEAL_STUCK, &[("space", "acme")], 0);
        let out = m.render_prometheus();
        assert!(
            out.contains("# TYPE ordo_reseal_rows_remaining gauge\n"),
            "{out}"
        );
        assert!(
            out.contains("ordo_reseal_rows_remaining{population=\"payloads\",space=\"acme\"} 3\n"),
            "the later, lower value wins:\n{out}"
        );
        assert!(
            out.contains(
                "# TYPE ordo_reseal_stuck_seconds gauge\nordo_reseal_stuck_seconds{space=\"acme\"} 0\n"
            ),
            "{out}"
        );
        assert!(!out.contains("# TYPE ordo_reseal_rows_remaining counter"));
        // Charged against the cap.
        for i in 0..MAX_SERIES + 10 {
            m.gauge("g", &[("i", &i.to_string())], 1);
        }
        assert_eq!(m.gauge_snapshot().len(), MAX_SERIES);
        assert!(m.render_prometheus().contains(SERIES_DROPPED));
    }

    /// rather than as an object, so `about` stays empty: an entry in a
    /// space's own stream already is that space.
    #[test]
    fn a_key_release_names_the_space_it_touched() {
        let m = metrics();
        let stream = Arc::new(Recorder::default());
        let keys = MeteredKeys::new(
            Arc::new(ordo_crypto::TestProvider::with_space("spc_alpha")),
            m.clone(),
        )
        .with_stream(stream.clone());

        keys.release(&CSpaceId("spc_alpha".into()), Purpose::Read, "cor_1")
            .expect("release");

        let entries = stream.drain_all(Timestamp(0));
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].scope,
            Scope::Space(ordo_commandlog::SpaceId("spc_alpha".into())),
            "a key release must name the space it touched"
        );
        assert_eq!(
            entries[0].about, None,
            "and an entry in the space's own stream carries no second copy of it"
        );
    }

    /// A provider that refuses everything. `TestProvider` creates spaces on
    /// demand, so it can never say no — and "does a refusal get recorded"
    /// needs a provider that can.
    struct RefusingKeys;

    impl KeyProvider for RefusingKeys {
        fn release(
            &self,
            _s: &CSpaceId,
            _p: Purpose,
            _c: &str,
        ) -> Result<ordo_crypto::KeyLease, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn blind_index(
            &self,
            _s: &CSpaceId,
            _e: &str,
            _f: &str,
            _v: ordo_crypto::BlindValue<'_>,
        ) -> Result<[u8; 32], ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn seal_at(
            &self,
            _l: &ordo_crypto::KeyLease,
            _e: &str,
            _f: &str,
            _a: ordo_crypto::Anchor<'_>,
            _v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::NotLeased)
        }
        fn open_at(
            &self,
            _l: &ordo_crypto::KeyLease,
            _e: &str,
            _f: &str,
            _a: ordo_crypto::Anchor<'_>,
            _v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::NotLeased)
        }
        #[allow(clippy::too_many_arguments)]
        fn seal_chunk(
            &self,
            _l: &ordo_crypto::KeyLease,
            _e: &str,
            _f: &str,
            _a: ordo_crypto::Anchor<'_>,
            _ep: ordo_crypto::Epoch,
            _b: &ordo_crypto::NonceBase,
            _c: u32,
            _last: bool,
            _v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::NotLeased)
        }
        #[allow(clippy::too_many_arguments)]
        fn open_chunk(
            &self,
            _l: &ordo_crypto::KeyLease,
            _e: &str,
            _f: &str,
            _a: ordo_crypto::Anchor<'_>,
            _ep: ordo_crypto::Epoch,
            _b: &ordo_crypto::NonceBase,
            _c: u32,
            _last: bool,
            _v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::NotLeased)
        }
        fn current_epoch(
            &self,
            _s: &CSpaceId,
        ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn rotate(
            &self,
            _s: &CSpaceId,
            _r: &str,
        ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn destroy_epoch(
            &self,
            _s: &CSpaceId,
            _e: ordo_crypto::Epoch,
        ) -> Result<(), ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn destroy_space(&self, _s: &CSpaceId) -> Result<(), ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn create_space(&self, _s: &CSpaceId) -> Result<(), ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
    }

    /// A refused release is not custody touched: neither artifact records
    /// one. Counting or recording a provider's own refusals would make them
    /// look like breadth.
    #[test]
    fn a_refused_release_is_recorded_nowhere() {
        let m = metrics();
        let stream = Arc::new(Recorder::default());
        let keys = MeteredKeys::new(Arc::new(RefusingKeys), m.clone()).with_stream(stream.clone());
        assert!(keys
            .release(&CSpaceId("spc_unknown".into()), Purpose::Read, "cor_x")
            .is_err());
        assert!(stream.drain_all(Timestamp(0)).is_empty());
        assert!(!m.render_prometheus().contains("ordo_key_release_total"));
    }

    // ── histograms (5f) ─────────────────────────────────────────────────
    //
    // The arithmetic, at the boundaries. `tests/lock_wait.rs` proves the
    // family observes a real contended lock; these prove that what it then
    // *says* about the observations is right, which is a different claim and
    // the one a wrong p99 would come from.

    const H: &str = "test_seconds";

    /// A value goes in the bucket whose `le` bound it does not exceed, and
    /// the rendering is cumulative. Off-by-one here is a p99 that is quietly
    /// one bucket wrong forever.
    #[test]
    fn an_observation_lands_at_or_below_its_bound_and_renders_cumulatively() {
        let m = Metrics::new();
        // Exactly on a bound belongs to that bound: `le` means ≤.
        m.observe(H, &[], 0.001);
        m.observe(H, &[], 0.0011);
        m.observe(H, &[], 30.0);

        let snap = m.histogram_snapshot();
        assert_eq!(snap.len(), 1);
        let by_le = |le: f64| {
            snap[0]
                .buckets
                .iter()
                .find(|(b, _)| *b == le)
                .map(|(_, n)| *n)
                .expect("bound in the ladder")
        };
        assert_eq!(by_le(0.0005), 0);
        assert_eq!(by_le(0.001), 1, "0.001 is ≤ 0.001");
        assert_eq!(by_le(0.0025), 2, "0.0011 joins it, cumulatively");
        assert_eq!(by_le(10.0), 2, "30s exceeds the ladder");
        assert_eq!(
            snap[0].buckets.last().map(|(le, n)| (*le, *n)),
            Some((f64::INFINITY, 3)),
            "+Inf holds everything"
        );
        assert_eq!(snap[0].count, 3);
        assert!((snap[0].sum - 30.0021).abs() < 1e-9);
    }

    /// The quantile is Prometheus's, including the two answers that are
    /// judgement calls rather than arithmetic: nothing observed is `None`
    /// (not zero), and a quantile in the `+Inf` bucket is the ladder's top
    /// (not infinity).
    #[test]
    fn the_quantile_is_prometheuss_including_its_two_honest_refusals() {
        let m = Metrics::new();
        assert_eq!(
            m.quantile(H, &[], 0.99),
            None,
            "a quantile over nothing is not a fast quantile"
        );

        // 99 fast, 1 slow — the shape a p99 exists to catch.
        for _ in 0..99 {
            m.observe(H, &[], 0.000_2);
        }
        m.observe(H, &[], 3.0);
        let p50 = m.quantile(H, &[], 0.5).expect("100 observations");
        let p99 = m.quantile(H, &[], 0.99).expect("100 observations");
        let p100 = m.quantile(H, &[], 1.0).expect("100 observations");
        assert!(p50 <= 0.000_25, "the fast population, got {p50}");
        assert!(p99 <= 0.000_25, "the 99th is still fast, got {p99}");
        assert!(
            (2.5..=5.0).contains(&p100),
            "the tail is the slow one, in its bucket: {p100}"
        );

        // Past the ladder, the strongest true statement is the ladder's top.
        let far = Metrics::new();
        far.observe(H, &[], 90.0);
        assert_eq!(far.quantile(H, &[], 0.99), BUCKETS.last().copied());

        // Out-of-range q is a caller error, answered rather than clamped.
        assert_eq!(m.quantile(H, &[], 1.5), None);
    }

    /// A clock that ran backwards is counted, not dropped. A `_count` that
    /// disagrees with the number of appends is a worse lie than a `_sum`
    /// that is 20ms light.
    #[test]
    fn a_nonsense_duration_is_counted_as_zero_rather_than_discarded() {
        let m = Metrics::new();
        m.observe(H, &[], -0.5);
        m.observe(H, &[], f64::NAN);
        assert_eq!(m.histogram_count(H, &[]), Some(2));
        assert_eq!(m.histogram_sum(H, &[]), Some(0.0));
    }

    /// The two shapes share one cardinality budget and one honesty counter.
    /// A histogram is many series, so it is charged like many series — and
    /// when the cap bites, the drop is visible rather than silent.
    #[test]
    fn histogram_series_are_capped_and_the_drops_are_counted() {
        let m = Metrics::new();
        for i in 0..MAX_SERIES + 10 {
            m.observe(H, &[("k", &format!("v{i}"))], 0.001);
        }
        assert_eq!(m.histogram_snapshot().len(), MAX_SERIES);
        assert!(m.render_prometheus().contains(SERIES_DROPPED));
    }

    /// The op-duration label set is bounded by the wire inventory: an
    /// executor refusal contributes its **class**, never its message.
    /// A message in a label is an unbounded series with a caller's string
    /// in it, which is the cardinality flood and the disclosure at once.
    #[test]
    fn op_duration_labels_by_class_not_by_message() {
        let m = Metrics::new();
        m.op_duration("openTasks", "ok", 0.004);
        m.op_duration("openTasks", ordo_api::ErrorCode::CasMiss.as_str(), 0.002);
        let text = m.render_prometheus();
        assert!(text.contains("op=\"openTasks\",outcome=\"ok\""), "{text}");
        assert!(
            text.contains("op=\"openTasks\",outcome=\"cas_miss\""),
            "{text}"
        );
        assert!(text.contains("# TYPE ordo_op_duration_seconds histogram"));
    }

    // ── the usage ledger (17n M1a) ───────────────────────────────────────

    /// Off unless `ORDO_USAGE_LEDGER` (or the builder) says otherwise, and
    /// `usage_take` on an off registry answers with nothing to write rather
    /// than an empty map that would read as "a period with zero traffic".
    #[test]
    fn usage_is_off_by_default_and_take_returns_empty() {
        let m = Metrics::new();
        assert!(!m.usage_enabled());
        m.usage_request("spc_a", ordo_api::CostClass::Sql, Some(3));
        m.usage_worker_run("spc_a");
        assert!(m.usage_take().is_empty());
    }

    /// Every executor outcome is a request, keyed by its cost class; a rows
    /// count is added only for the calls that pass one (a `Ran` outcome).
    #[test]
    fn usage_counts_requests_by_class_and_rows_only_on_ran() {
        let m = Metrics::new().with_usage_ledger(true);
        assert!(m.usage_enabled());
        m.usage_request("spc_a", ordo_api::CostClass::Sql, Some(4));
        // `AlreadyApplied`, or an inner executor `Err` — either way a
        // request with no rows to count.
        m.usage_request("spc_a", ordo_api::CostClass::Sql, None);
        m.usage_request("spc_a", ordo_api::CostClass::BoundedHost, Some(1));
        let taken = m.usage_take();
        let counters = taken.get("spc_a").expect("spc_a recorded");
        assert_eq!(counters.requests.get("sql"), Some(&2));
        assert_eq!(counters.requests.get("bounded-host"), Some(&1));
        assert_eq!(counters.rows_returned, 5);
    }

    /// `usage_take` drains: a second call right after the first sees
    /// nothing, which is what keeps one flush interval's counters
    /// exclusive to it rather than double-counted into the next.
    #[test]
    fn usage_take_drains_and_a_second_take_is_empty() {
        let m = Metrics::new().with_usage_ledger(true);
        m.usage_request("spc_a", ordo_api::CostClass::Sql, Some(1));
        m.usage_worker_run("spc_a");
        let first = m.usage_take();
        assert_eq!(first["spc_a"].requests.get("sql"), Some(&1));
        assert_eq!(first["spc_a"].worker_runs, 1);
        assert!(m.usage_take().is_empty());
    }
}
