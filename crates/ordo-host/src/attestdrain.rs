//! `attest.deliver` — the R3 worker slot that joins the attestation outbox
//! to the egress client.
//!
//! This is the only place in the platform where `ordo_exec::webhook` (which
//! holds a `Pool`) and `ordo_egress` (which holds nothing) meet, and the
//! shape of the meeting is the point: the join **reads** a sink row and
//! **hands values** to the egress crate — a URL string, a body, a secret. It
//! never hands it a `Pool`, a `KeyProvider`, or a connection, because
//! `ordo-egress` has no type that could receive one.
//!
//! # Why this is a struct with a `JoinHandle` and not a `Worker`
//!
//! `plan-runtime.md` R3 defines a `Worker` trait, a `Supervisor`, a
//! `Cadence` and a `WorkerCtx`. **None of them exist in `crates/`** — R3 is a
//! plan. So this follows the one worker that *is* wired, `secdrain`: a struct
//! owning its task, stopped by `Drop`, with a health surface published on
//! `/health/deep`. What it does adopt from R3, because those are the parts
//! that cost nothing now and are expensive to retrofit:
//!
//! - the **slot name** is `attest.deliver`
//!   ([`ordo_exec::webhook::DELIVER_SLOT`]) — `<subsystem>.<verb>`,
//!   `'static`, and simultaneously a metric label, a boot line and a
//!   `/health/deep` key, exactly as R3 §3.4 requires;
//! - the **backoff is `triggers::backoff_secs`**, R3's *"one shape of backoff
//!   in the platform, not two"*;
//! - a run reports **units and whether there is more**
//!   ([`Pass`]), which is `Progress`'s shape, so adopting the trait later is
//!   a rename rather than a redesign;
//! - a worker that is **not running is named** as not running, with its
//!   reason, in [`DeliveryHealth::note`] — R3's obligation 8, and `secdrain`'s
//!   `wired: false` pattern.
//!
//! # Why this slot must be schedulable on its own
//!
//! §7b's fifth pin wants egress under an identity that holds nothing. In one
//! process that is a dependency-graph property (see `ordo-egress`'s
//! `holds_nothing.rs`). The **deployment** half of it is that this slot can
//! run in a pod of its own with a credential-less service account, and the
//! mechanism for that is R3's `ORDO_WORKERS` allowlist plus a slot name.
//! Naming the slot now is what makes that a configuration change later
//! instead of a refactor. `deploying.md` carries the obligation.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ordo_commandlog::SpaceId;
use ordo_egress::{deliver, Allowlist, Delivery, DenyList, HttpsUrl, SinkSecret, TlsRoots};
use ordo_exec::cadence::LogPolicy;
use ordo_exec::webhook::{self, AttestSink, DELIVER_SLOT};
use ordo_exec::{Pool, PoolError};

use crate::metrics::Metrics;

/// `ordo_attest_deliveries_total{outcome}` — `sent`, `retrying`,
/// `dead_lettered`.
pub const DELIVERIES: &str = "ordo_attest_deliveries_total";
/// `ordo_attest_refusals_total{kind}` — the egress refusal's stable word.
/// This is the series an operator alerts on: a sustained `denied_address` is
/// somebody probing the perimeter through a sink.
pub const REFUSALS: &str = "ordo_attest_refusals_total";
/// `ordo_attest_outbox_depth` — pending deliveries, set each pass.
pub const OUTBOX_DEPTH: &str = "ordo_attest_outbox_depth";
/// `ordo_attest_deadletters_total` — the count, set each pass.
pub const DEADLETTERS: &str = "ordo_attest_deadletters_total";

/// How often the outbox is swept. An attestation arrives at most once per
/// `attest_every` commands or `attest_or` seconds, so a second is generous
/// and a delivery is never more than a second behind its record.
pub const DEFAULT_TICK: Duration = Duration::from_secs(1);
/// Deliveries claimed per pass. Small: each one is a TLS handshake to a host
/// this deployment does not control, and a large batch would let one slow
/// receiver hold the pass open for everyone else.
pub const CLAIM_BATCH: i64 = 8;

/// What one pass did. [`Pass::more`] is R3's `Progress::more`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pass {
    pub sent: u64,
    pub retrying: u64,
    pub dead_lettered: u64,
    pub more: bool,
}

impl Pass {
    pub fn units(&self) -> u64 {
        self.sent + self.retrying + self.dead_lettered
    }
}

/// What the delivery loop has done, and what it last failed to do.
///
/// Same discipline as `secdrain::DrainHealth`: every field is atomic or
/// behind a short-lived lock, because the reader is `/health/deep` on a
/// request thread and it must never wait on the worker.
#[derive(Debug, Default)]
pub struct DeliveryHealth {
    sent: AtomicU64,
    dead_lettered: AtomicU64,
    refused: AtomicU64,
    pending: AtomicU64,
    last_success: AtomicU64,
    failing_since: AtomicU64,
    last_error: Mutex<Option<String>>,
    /// Why the loop is not running, when it is not. R3's obligation 8.
    note: Mutex<Option<String>>,
}

impl DeliveryHealth {
    /// How long delivery may fail before `/health/deep` calls the deployment
    /// degraded. Ten minutes, matching `secdrain` — and for the same reason:
    /// long enough to ride out a receiver's deploy, short enough that a real
    /// outage is visible well before the outbox becomes a backlog.
    pub const DEGRADED_SECS: u64 = 600;

    /// A pass completed. **Only the clock** — the per-delivery counters are
    /// incremented inside [`run_pass`], where the deliveries happen.
    ///
    /// The split matters and was a bug before it was a rule: with the
    /// counters here, a caller driving `run_pass` directly (a test, and
    /// tomorrow R3's supervisor calling `Worker::run`) got a health surface
    /// that had counted nothing, so `sent` read zero after a delivery that
    /// demonstrably landed. A health field only some callers update is a
    /// health field that lies to the others.
    fn ok(&self, at: u64) {
        self.last_success.store(at, Ordering::Relaxed);
        self.failing_since.store(0, Ordering::Relaxed);
        *self.last_error.lock().expect("delivery health") = None;
    }

    fn failed(&self, at: u64, error: String) {
        // Only the first failure of a streak sets the clock, so a ten-minute
        // outage reads as ten minutes rather than as one minute ten times.
        let _ = self
            .failing_since
            .compare_exchange(0, at, Ordering::Relaxed, Ordering::Relaxed);
        *self.last_error.lock().expect("delivery health") = Some(error);
    }

    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }
    pub fn dead_lettered(&self) -> u64 {
        self.dead_lettered.load(Ordering::Relaxed)
    }
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }
    pub fn pending(&self) -> u64 {
        self.pending.load(Ordering::Relaxed)
    }
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().expect("delivery health").clone()
    }
    pub fn note(&self) -> Option<String> {
        self.note.lock().expect("delivery health").clone()
    }
    pub fn failing_for(&self, now: u64) -> Option<u64> {
        match self.failing_since.load(Ordering::Relaxed) {
            0 => None,
            since => Some(now.saturating_sub(since)),
        }
    }
    pub fn degraded(&self, now: u64) -> bool {
        self.failing_for(now)
            .is_some_and(|s| s >= DeliveryHealth::DEGRADED_SECS)
    }
}

/// How the loop is configured.
pub struct DeliveryConfig {
    pub tick: Duration,
    /// The whole per-delivery bound: connect, handshake, write, read.
    pub timeout: Duration,
    /// The space this deployment serves. Single-space today, as everywhere
    /// else in `ordo-host`; the loop reads its sink and nobody else's.
    pub space: SpaceId,
    pub policy: LogPolicy,
    /// The categories are compiled in; this is the deployment's own pod and
    /// service CIDRs, from `ORDO_EGRESS_DENY_CIDRS`.
    pub deny: DenyList,
    pub roots: TlsRoots,
}

impl DeliveryConfig {
    /// The defaults, plus the deny list and trust anchors from the
    /// environment.
    ///
    /// A malformed `ORDO_EGRESS_DENY_CIDRS` is an **error**, not a warning:
    /// the failure mode of a silently-dropped pod CIDR is that a sink can
    /// reach the cluster's own service network, which is the whole of §7b.
    pub fn from_env(space: SpaceId) -> Result<DeliveryConfig, String> {
        let cidrs = match std::env::var("ORDO_EGRESS_DENY_CIDRS") {
            Ok(s) => DenyList::parse_cidrs(&s)?,
            Err(_) => Vec::new(),
        };
        Ok(DeliveryConfig {
            tick: DEFAULT_TICK,
            timeout: ordo_egress::DEFAULT_TIMEOUT,
            space,
            policy: LogPolicy::DEFAULT,
            deny: DenyList::categories_only().with_deployment_cidrs(&cidrs),
            roots: TlsRoots::webpki(),
        })
    }
}

/// The running delivery loop. Dropping it stops the task.
pub struct AttestDelivery {
    stop: Arc<AtomicBool>,
    health: Arc<DeliveryHealth>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl AttestDelivery {
    /// Start delivering. Creates the outbox tables if they are not there —
    /// R3 §3.7 puts a worker's own DDL in the supervisor's bootstrap rather
    /// than in `ensure_platform_tables`, and this is that bootstrap.
    pub async fn start(
        pool: Pool,
        metrics: Arc<Metrics>,
        cfg: DeliveryConfig,
    ) -> Result<AttestDelivery, PoolError> {
        webhook::ensure_tables(&pool).await?;
        ordo_exec::cadence::ensure_tables(&pool).await?;
        let stop = Arc::new(AtomicBool::new(false));
        let health = Arc::new(DeliveryHealth::default());
        let task = tokio::spawn(delivery_loop(Loop {
            pool,
            metrics,
            cfg,
            stop: stop.clone(),
            health: health.clone(),
        }));
        Ok(AttestDelivery {
            stop,
            health,
            task: Some(task),
        })
    }

    /// The health surface, for `/health/deep` and for tests.
    pub fn health(&self) -> Arc<DeliveryHealth> {
        self.health.clone()
    }
}

impl Drop for AttestDelivery {
    /// Stops the loop. There is deliberately no flush: an outbox row survives
    /// the process by construction, so an unsent delivery is picked up by the
    /// next replica rather than lost — which is the whole reason the outbox
    /// is a table and not a channel.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct Loop {
    pool: Pool,
    metrics: Arc<Metrics>,
    cfg: DeliveryConfig,
    stop: Arc<AtomicBool>,
    health: Arc<DeliveryHealth>,
}

async fn delivery_loop(l: Loop) {
    loop {
        tokio::time::sleep(l.cfg.tick).await;
        if l.stop.load(Ordering::Relaxed) {
            return;
        }
        let now = now_secs();
        match run_pass(&l.pool, &l.metrics, &l.cfg, &l.health, now).await {
            Ok(_) => l.health.ok(now),
            Err(e) => l.health.failed(now, e.to_string()),
        }
    }
}

/// One sweep: fan new attestations into the outbox, then deliver what is due.
///
/// Collaborators are arguments rather than fields, which is deliberate two
/// ways: a test drives a **deterministic** pass instead of sleeping on a tick,
/// and this is the shape `Worker::run(&self, ctx: &WorkerCtx)` will have when
/// R3's supervisor lands — the pool, the metrics and the clock are exactly
/// what `WorkerCtx` carries, so adopting the trait is a rename.
pub async fn run_pass(
    pool: &Pool,
    metrics: &Metrics,
    cfg: &DeliveryConfig,
    health: &DeliveryHealth,
    now: u64,
) -> Result<Pass, PoolError> {
    webhook::enqueue_due(pool, &cfg.space, now).await?;

    let claimed = webhook::claim_deliveries(pool, now, CLAIM_BATCH).await?;
    let mut out = Pass {
        more: claimed.len() as i64 == CLAIM_BATCH,
        ..Pass::default()
    };
    for d in &claimed {
        match deliver_one(pool, cfg, &d.space, d.upto, &d.head).await {
            Ok(()) => {
                webhook::finish_delivery(pool, d).await?;
                metrics.incr(DELIVERIES, &[("outcome", "sent")]);
                health.sent.fetch_add(1, Ordering::Relaxed);
                out.sent += 1;
            }
            Err(fault) => {
                health.refused.fetch_add(1, Ordering::Relaxed);
                metrics.incr(REFUSALS, &[("kind", fault.kind)]);
                let what = webhook::fail_delivery(
                    pool,
                    d,
                    &fault.destination,
                    &fault.reason,
                    fault.retryable,
                    now,
                )
                .await?;
                match what {
                    webhook::Failed::Retrying { .. } => {
                        metrics.incr(DELIVERIES, &[("outcome", "retrying")]);
                        out.retrying += 1;
                    }
                    webhook::Failed::DeadLettered => {
                        metrics.incr(DELIVERIES, &[("outcome", "dead_lettered")]);
                        health.dead_lettered.fetch_add(1, Ordering::Relaxed);
                        out.dead_lettered += 1;
                    }
                }
            }
        }
    }

    let (pending, dead) = webhook::outbox_depth(pool).await?;
    health.pending.store(pending, Ordering::Relaxed);
    metrics.set(OUTBOX_DEPTH, &[], pending);
    metrics.set(DEADLETTERS, &[], dead);
    Ok(out)
}

/// Why a single delivery did not land, in the three shapes the outbox needs.
pub struct Fault {
    /// The egress refusal's stable word, or `sink` / `record` for the two
    /// faults that are not egress ones. A metric label.
    pub kind: &'static str,
    /// What the dead-letter row records. The operational rendering, since an
    /// operator reads it — the customer-facing rendering is what the ops op
    /// returns.
    pub reason: String,
    pub destination: String,
    pub retryable: bool,
}

/// Deliver one attestation. **This is where the pins are applied**, in order,
/// and the order is the argument:
///
/// 1. read the sink — no sink, nothing to do;
/// 2. **re-verify the owner's signature over the destination**, before the
///    URL is parsed and long before it is resolved. A destination the owner
///    did not sign is not a destination this platform will look up, let alone
///    connect to;
/// 3. parse it as `https` (pin 1);
/// 4. resolve and pin, against an allowlist that is **exactly this
///    destination's host** (pins 6 then 2);
/// 5. POST it, signed, with no redirect following (pins 4 and 3).
async fn deliver_one(
    pool: &Pool,
    cfg: &DeliveryConfig,
    space: &SpaceId,
    upto: u64,
    head: &ordo_commandlog::ChainHash,
) -> Result<(), Fault> {
    let sink = read_sink_or_fault(pool, space).await?;

    // Step 2. Every delivery, not only at setup: this is what makes an edit
    // of the `destination` column a refused delivery rather than a
    // redirected one.
    sink.verify_destination().map_err(|e| Fault {
        kind: "sink",
        reason: format!("destination refused: {e}"),
        destination: sink.destination.clone(),
        // Not retryable: an unsigned destination will still be unsigned in
        // sixty-four seconds, and retrying it is how a redirect attempt gets
        // five more chances.
        retryable: false,
    })?;

    let record = ordo_exec::cadence::attestations(pool, space)
        .await
        .map_err(|e| Fault {
            kind: "record",
            reason: e.to_string(),
            destination: sink.destination.clone(),
            retryable: true,
        })?
        .into_iter()
        // By position **and head**: a position can carry a pre-restore
        // attestation and a post-restore one since R2e, and delivering the
        // wrong one would send a receiver a head this chain has moved away
        // from as though it were current.
        .find(|a| a.upto == upto && a.head == *head)
        .ok_or_else(|| Fault {
            kind: "record",
            reason: format!("no attestation at position {upto} with that head"),
            destination: sink.destination.clone(),
            retryable: false,
        })?;

    let verifiable_from = ordo_exec::cadence::verifiable_from(pool, space)
        .await
        .unwrap_or(0);
    let body = webhook::payload(&record, &sink, &cfg.policy, verifiable_from);

    let url = HttpsUrl::parse(&sink.destination).map_err(|e| fault_of(&e, &sink))?;
    // Pin 6: for attestation the allowlist *is* the owner-signed
    // destination. Not a table lookup — the consent and the allowlist are one
    // artifact, so they cannot drift.
    let allow = Allowlist::exactly(url.host());
    let dest = ordo_egress::resolve_and_pin(&url, &allow, &cfg.deny)
        .await
        .map_err(|e| fault_of(&e, &sink))?;

    let Some(secret) = SinkSecret::from_hex(&sink.secret) else {
        return Err(Fault {
            kind: "sink",
            reason: "the sink secret is not hex".into(),
            destination: sink.destination.clone(),
            retryable: false,
        });
    };
    let headers = [
        ("x-ordo-space", space.0.clone()),
        ("x-ordo-position", upto.to_string()),
        ("x-ordo-schema", webhook::PAYLOAD_SCHEMA.to_string()),
    ];
    deliver(
        dest,
        &cfg.roots,
        Delivery {
            body: &body,
            content_type: "application/json",
            timestamp: now_secs(),
            headers: &headers,
            secret: &secret,
        },
        cfg.timeout,
    )
    .await
    .map(|_| ())
    .map_err(|e| fault_of(&e, &sink))
}

fn fault_of(e: &ordo_egress::Refusal, sink: &AttestSink) -> Fault {
    Fault {
        kind: e.kind(),
        reason: e.operational(),
        destination: sink.destination.clone(),
        retryable: e.retryable(),
    }
}

async fn read_sink_or_fault(pool: &Pool, space: &SpaceId) -> Result<AttestSink, Fault> {
    match webhook::read_sink(pool, space).await {
        Ok(Some(s)) => Ok(s),
        Ok(None) => Err(Fault {
            kind: "sink",
            reason: "the space has no attestation destination".into(),
            destination: String::new(),
            retryable: false,
        }),
        Err(e) => Err(Fault {
            kind: "sink",
            reason: e.to_string(),
            destination: String::new(),
            retryable: true,
        }),
    }
}

/// The `/health/deep` section. `wired: false` with a reason when the slot is
/// not running — R3's obligation 8, `secdrain`'s shape.
pub fn section(health: Option<&Arc<DeliveryHealth>>, now: u64) -> serde_json::Value {
    let Some(h) = health else {
        return serde_json::json!({
            "slot": DELIVER_SLOT,
            "wired": false,
            "detail": "no attestation delivery worker is running in this process; \
                       layer 2 is opt-in per space and a deployment that enables no \
                       sink has chosen the default posture, not a broken one",
        });
    };
    serde_json::json!({
        "slot": DELIVER_SLOT,
        "wired": true,
        "sent": h.sent(),
        "pending": h.pending(),
        "deadLettered": h.dead_lettered(),
        "refused": h.refused(),
        "failingForSeconds": h.failing_for(now),
        "degraded": h.degraded(now),
        "lastError": h.last_error(),
        "note": h.note(),
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
