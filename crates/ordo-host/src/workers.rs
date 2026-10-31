//! The worker set: the loops that finally call the steps.
//!
//! Five of these rows carry a decision rather than a transcription, and the
//! decisions are on the types.

use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{Ed25519Signer, SpaceId as LogSpace, Timestamp};
use ordo_exec::reseal;
use ordo_exec::route;
use ordo_exec::spacedb;
use ordo_exec::triggers;
use ordo_exec::usage;

use crate::metrics::Metrics;
use crate::quotas::PgQuotas;
use crate::supervisor::{BoxFut, Cadence, Progress, Worker, WorkerCtx, WorkerError};

/// How many runs one `triggers.runner` pass claims. A bound rather than
/// "everything due": a claim is a transaction, and one that claimed a
/// backlog would hold rows for the length of the whole batch.
const RUN_BATCH: i64 = 16;
/// How much log one drain pass reads. `Progress::more` carries the rest, so
/// this is a chunk size and not a ceiling.
const DRAIN_LIMIT: usize = 256;

// ── triggers.drain ───────────────────────────────────────────────────────

/// Log → batches, cursor last.
///
/// **No leadership, and the module doc is why.** Two replicas draining the
/// same listener both read forward from the same cursor and both accumulate
/// the same events into `$triggerbatches`. `drain_listener` already argued
/// that for the crash case — *"a crash mid-drain replays the window and
/// re-accumulates — at-least-once, exactly as promised, and harmless
/// because row ids are a set"* — and two replicas are the same situation as
/// one replica crashing. Adding leadership here would buy nothing and would
/// make a leader's death a delivery stall.
pub struct TriggersDrain {
    /// The listeners and tenants in force now, read on every pass so a
    /// version switch changes them with no restart. A wiring with no
    /// tenants drains the one space this process booted with, on the
    /// control database.
    pub live: Arc<crate::routines::LiveRoutines>,
    pub store: ordo_exec::commandlog::PgStore,
}

impl Worker for TriggersDrain {
    fn name(&self) -> &'static str {
        "triggers.drain"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(Duration::from_secs(1))
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let now = ctx.now();
            let mut units = 0;
            let mut more = false;
            let wiring = self.live.current();
            if wiring.listeners.is_empty() {
                return Ok(Progress::idle());
            }
            if let Some(tenants) = &wiring.tenants {
                // A tenant whose chain has not moved is visited less often,
                // and a routine's cursor under an earlier trigger is retired
                // once caught up (audit 2026-10-08 finding 7).
                let live = ctx.deployment.current();
                let pass = tenants
                    .drain_pass(
                        &ctx.pool,
                        &wiring.listeners,
                        &live.version,
                        DRAIN_LIMIT,
                        now,
                    )
                    .await?;
                for m in &pass.misses {
                    eprintln!("worker triggers.drain: not drained: {m}");
                }
                for (space, old) in &pass.retired {
                    eprintln!(
                        "worker triggers.drain: retired listener cursor `{old}` in {space}: \
                         the routine's new trigger has passed it"
                    );
                }
                return Ok(Progress {
                    units: pass.units,
                    more: pass.more,
                });
            }
            for l in wiring.listeners.iter() {
                let read = triggers::drain_listener(
                    &ctx.pool,
                    &self.store,
                    &LogSpace(ctx.space.0.clone()),
                    l,
                    DRAIN_LIMIT,
                    now,
                )
                .await?;
                units += read as u64;
                more |= read == DRAIN_LIMIT;
            }
            Ok(Progress { units, more })
        })
    }
}

// ── triggers.batch ───────────────────────────────────────────────────────

/// Batches → runs, one transaction.
///
/// **Separate from `triggers.drain`, and that is not tidiness.** `fire_due`
/// closes a debounce window, and a window closes on the *clock*: `batch_due`
/// is true when `now - last_at >= window`. A batch whose last event arrived
/// five minutes ago with no writes since must still fire. A
/// drain-triggered-only design would leave every debounced batch of a quiet
/// listener open forever — the failure that looks exactly like "the trigger
/// did not match".
pub struct TriggersBatch {
    pub live: Arc<crate::routines::LiveRoutines>,
}

impl Worker for TriggersBatch {
    fn name(&self) -> &'static str {
        "triggers.batch"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(Duration::from_secs(1))
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let wiring = self.live.current();
            if wiring.listeners.is_empty() {
                return Ok(Progress::idle());
            }
            let tx = ctx.pool.begin().await?;
            let fired = triggers::fire_due(&tx, &wiring.listeners, ctx.now()).await?;
            tx.commit().await?;
            Ok(Progress::did(fired.len() as u64))
        })
    }
}

// ── triggers.runner ──────────────────────────────────────────────────────

/// Claim → handle → dispose.
///
/// Enabled **only when a `RunHandler` is registered**. A runner that
/// started, claimed runs and dead-lettered every one of them would look
/// like a working trigger system destroying work.
///
/// Disposition is all already built: success is `finish_run`; failure is
/// `fail_run`, which retries with `backoff_secs` until `MAX_ATTEMPTS` and
/// then dead-letters. Dead letters are an operator alert, which loses
/// nothing — and R3 gives that alert a surface.
pub struct TriggersRunner {
    /// The handler in force now. None while the active version declares no
    /// routine, and then the runner claims nothing.
    pub live: Arc<crate::routines::LiveRoutines>,
}

impl Worker for TriggersRunner {
    fn name(&self) -> &'static str {
        "triggers.runner"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(Duration::from_millis(250))
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let Some(handler) = self.live.current().handler.clone() else {
                return Ok(Progress::idle());
            };
            let now = ctx.now();
            let tx = ctx.pool.begin().await?;
            let runs = triggers::claim_runs(&tx, now, RUN_BATCH).await?;
            tx.commit().await?;
            if runs.is_empty() {
                return Ok(Progress::idle());
            }
            let claimed = runs.len();
            let mut done = 0;
            for run in &runs {
                // The handler runs OUTSIDE the claim's transaction. A body
                // that took a minute would otherwise hold the claim's rows
                // for a minute, and the claim already marked the run as
                // taken — `claimed = true` is what stops a second replica
                // picking it up.
                let outcome = handler.handle(run, ctx).await;
                let tx = ctx.pool.begin().await?;
                match outcome {
                    Ok(()) => {
                        triggers::finish_run(&tx, run).await?;
                        done += 1;
                    }
                    Err(why) => {
                        // A failing body is the run's failure, not this
                        // worker's: it is retried on the backoff and
                        // dead-lettered after MAX_ATTEMPTS. The reason is
                        // logged so an operator sees it before the dead
                        // letter does.
                        eprintln!(
                            "worker triggers.runner: run {} in {} failed (attempt {}): {why}",
                            run.run_key, run.space.0, run.attempts
                        );
                        let what = triggers::fail_run(&tx, run, &why, ctx.now()).await?;
                        if matches!(what, triggers::Failed::DeadLettered) {
                            ctx.metrics.incr(
                                crate::supervisor::TRIGGER_DEADLETTERS,
                                &[("listener", &run.listener)],
                            );
                        }
                    }
                }
                tx.commit().await?;
                // 17n M1a: an attempt executed a body either way, so the
                // count happens after disposition regardless of which arm
                // ran, not only on success.
                ctx.metrics.usage_worker_run(&run.space.0);
            }
            Ok(Progress {
                units: done,
                more: claimed as i64 == RUN_BATCH,
            })
        })
    }
}

// ── triggers.clock ───────────────────────────────────────────────────────

/// `$schedules` → runs.
///
/// **The only trigger worker that takes leadership, and it takes it as an
/// optimisation rather than for correctness.** `fire_clock` calls
/// `claim_schedule` per routine, and `claim_schedule` is fires-once by
/// `FOR UPDATE SKIP LOCKED` plus the committed `next_due` advance.
/// Leadership keeps a hundred replicas from all trying at once; a
/// supervisor whose leadership is broken is wasteful, not wrong.
pub struct TriggersClock {
    /// The clock routines and tenants in force now. A due schedule enqueues
    /// one run per tenant; a wiring with no tenants enqueues for the boot
    /// space alone.
    pub live: Arc<crate::routines::LiveRoutines>,
}

impl Worker for TriggersClock {
    fn name(&self) -> &'static str {
        "triggers.clock"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(Duration::from_secs(1))
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            // Read the schedules first and list the tenants only when one is
            // due, so an idle clock costs one query a second whatever the
            // number of tenants (audit 2026-10-08 finding 7).
            let wiring = self.live.current();
            if !triggers::clock_due(&ctx.pool, &wiring.clock, ctx.now()).await? {
                return Ok(Progress::idle());
            }
            let spaces: Vec<LogSpace> = match &wiring.tenants {
                Some(t) => t.list(&ctx.pool).await.into_iter().map(LogSpace).collect(),
                None => vec![LogSpace(ctx.space.0.clone())],
            };
            let fired =
                triggers::fire_clock_in(&ctx.pool, &spaces, &wiring.clock, ctx.now()).await?;
            Ok(Progress::did(fired.len() as u64))
        })
    }
}

// ── commandlog.checkpoint / commandlog.attest ────────────────────────────

/// Sign a marker at the head, on the program's own checkpoint cadence.
pub struct CommandlogCheckpoint {
    pub signer: Arc<Ed25519Signer>,
    pub policy: ordo_exec::cadence::LogPolicy,
    /// The control database's record of every signed head
    /// (`restore-witness`, R2d-i). Built from `Runtime.pool`, which is the
    /// control pool; not optional, because an unwitnessed checkpoint is
    /// what the ruling removes.
    pub witness: ordo_exec::witness::Witness,
}

impl Worker for CommandlogCheckpoint {
    fn name(&self) -> &'static str {
        "commandlog.checkpoint"
    }
    fn cadence(&self) -> Cadence {
        // A `Tick`, not a `Schedule`, because `claim_checkpoint_tick` IS the
        // schedule claim and it advances by `policy.checkpoint` — the
        // worker's own period. Letting the supervisor claim as well would
        // be two claims of one row with two periods, which is precisely the
        // bug obligation 2 exists to prevent.
        Cadence::Tick(Duration::from_secs(5))
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let now = ctx.now();
            if !ordo_exec::cadence::claim_checkpoint_tick(&ctx.pool, &self.policy, now as i64)
                .await?
            {
                return Ok(Progress::idle());
            }
            let space = LogSpace(ctx.space.0.clone());
            match ordo_exec::cadence::checkpoint_due(
                &ctx.pool,
                &space,
                &self.signer,
                Timestamp(now),
                &self.witness,
            )
            .await
            {
                Ok(ordo_exec::undo::Checkpointed::AlreadyAt(_)) => Ok(Progress::idle()),
                Ok(_) => Ok(Progress::did(1)),
                Err(e) => Err(WorkerError(e.to_string())),
            }
        })
    }
}

/// Publish an attestation: **volume or time, whichever lands first.**
pub struct CommandlogAttest {
    pub signer: Arc<Ed25519Signer>,
    pub policy: ordo_exec::cadence::LogPolicy,
}

impl Worker for CommandlogAttest {
    fn name(&self) -> &'static str {
        "commandlog.attest"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(Duration::from_secs(5))
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let now = ctx.now();
            if !ordo_exec::cadence::claim_attest_tick(&ctx.pool, &self.policy, now as i64).await? {
                return Ok(Progress::idle());
            }
            let space = LogSpace(ctx.space.0.clone());
            match ordo_exec::cadence::attest_due(
                &ctx.pool,
                &space,
                &self.signer,
                &self.policy,
                Timestamp(now),
            )
            .await?
            {
                ordo_exec::cadence::Attested::Signed(_) => Ok(Progress::did(1)),
                _ => Ok(Progress::idle()),
            }
        })
    }
}

// ── secevent.checkpoint ──────────────────────────────────────────────────

/// Sign a marker at the head of every security stream, on the stream's own
/// checkpoint cadence.
///
/// # Why this is not part of `secevent.retain`
///
/// It was, and that is the defect. `retain_scope`'s step 1 signed the head,
/// so the marker that makes `audit_stream`'s fourth check — *nothing was
/// removed from the end of the stream* — reachable at all was signed on the
/// **retention** cadence (six hours, by default) and only when retention was
/// enabled. [`CommandlogAttest`] is the precedent and it sits right above
/// this one: the command chain separates its heavyweight monthly anchor from
/// its hourly attestation because *"wiring the two together leaves a month
/// of unattested history — a long time for a rewrite to sit undetected"*.
/// The security stream has no second artifact, so its one marker carries both
/// jobs and takes the tighter cadence, on its own row.
///
/// The enabling condition is the sharper half. `DrainConfig::retain = false`
/// is an ordinary storage choice — keep every denial forever — and it used
/// to switch off truncation detection as a side effect. A deployment that
/// keeps everything is more interested in whether anything was deleted, not
/// less.
///
/// A `Tick` for the reason [`CommandlogCheckpoint`] gives:
/// `claim_checkpoint_tick` **is** the claim of
/// `SECEVENT_CHECKPOINT_SCHEDULE`, advanced by the checkpoint period, so a
/// supervisor-side `Schedule` claim of the same row would be the
/// two-claims-two-periods bug. The tick decides how often this replica asks;
/// the schedule decides whether the answer is yes.
///
/// Leader-only as an optimisation, like `triggers.clock` and
/// `secevent.retain`: the claim is fires-once by `FOR UPDATE SKIP LOCKED`,
/// so **a fleet signs once per period rather than once per replica** whether
/// or not a leader exists, and leadership only keeps a hundred replicas from
/// all asking at the same instant.
pub struct SecEventCheckpoint {
    pub checkpointer: crate::secdrain::Checkpointer,
    pub tick: Duration,
}

impl Worker for SecEventCheckpoint {
    fn name(&self) -> &'static str {
        "secevent.checkpoint"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.tick)
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            Ok(match self.checkpointer.pass(ctx.now()).await? {
                // Zero is `Progress::idle()` and not `did(0)`: a claimed tick
                // on which every stream's head was already signed is a quiet
                // period, and a supervisor that counted it as work would
                // report a deployment with no security traffic as busy.
                Some(signed) if signed > 0 => Progress::did(signed),
                _ => Progress::idle(),
            })
        })
    }
}

// ── secevent.retain ──────────────────────────────────────────────────────

/// Cut the security streams back to their floor, on the retention cadence.
///
/// It signs a marker too — `retain_scope` still checkpoints before it cuts,
/// because a checkpoint is the only lawful cut point and signing after
/// pruning would anchor a stream at a position the cut had already passed.
/// But that is no longer the *only* thing that signs one, and the difference
/// matters: this worker's job is to reclaim storage on a storage cadence, and
/// [`SecEventCheckpoint`] above owns the evidence cadence. See its doc for
/// why the two are not one worker.
///
/// A `Tick` for the reason [`CommandlogCheckpoint`] gives:
/// `claim_retention_tick` **is** the claim of `SECEVENT_RETENTION_SCHEDULE`,
/// advanced by the retention period, so a supervisor-side `Schedule` claim
/// of the same row would be the two-claims-two-periods bug. The tick only
/// decides how often this replica asks; the schedule decides whether the
/// answer is yes.
///
/// Leader-only as an optimisation, like `triggers.clock`: the claim is
/// fires-once by `FOR UPDATE SKIP LOCKED`, so a fleet without a leader
/// would still run one pass per period between them.
///
/// Enabled only when `ORDO_SECURITY_CHECKPOINT_KEY` is set, because a
/// retention cut is legal only at a signed checkpoint. The pass itself is
/// [`crate::secdrain::Retainer`], the same one the drain runs when it
/// retains by itself, so the drain's health counters stay true.
pub struct SecEventRetain {
    pub retainer: crate::secdrain::Retainer,
    pub tick: Duration,
}

impl Worker for SecEventRetain {
    fn name(&self) -> &'static str {
        "secevent.retain"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.tick)
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            Ok(match self.retainer.pass(ctx.now()).await? {
                Some(units) => Progress::did(units),
                None => Progress::idle(),
            })
        })
    }
}

// ── attest.deliver ───────────────────────────────────────────────────────

/// Fan new attestations into the outbox and deliver what is due.
///
/// `attestdrain::run_pass` was written with this shape in mind — *"this is
/// the shape `Worker::run(&self, ctx: &WorkerCtx)` will have when R3's
/// supervisor lands, so adopting the trait is a rename"* — and it is. The
/// standalone `AttestDelivery` loop is **not** started when this worker is
/// registered: two loops over one outbox would claim against each other.
///
/// The health counters it reports (`sent`, `refused`, `dead_lettered`,
/// `pending`) are incremented inside `run_pass` rather than by the loop,
/// which is what makes them true when the pass is driven from here. That
/// was a real defect once — a caller driving `run_pass` directly read zero
/// `sent` after a delivery that demonstrably landed — and the fix was to
/// move the counters to where the deliveries happen.
pub struct AttestDeliver {
    pub cfg: crate::attestdrain::DeliveryConfig,
    pub health: Arc<crate::attestdrain::DeliveryHealth>,
}

impl Worker for AttestDeliver {
    fn name(&self) -> &'static str {
        "attest.deliver"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.cfg.tick)
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let pass = crate::attestdrain::run_pass(
                &ctx.pool,
                &ctx.metrics,
                &self.cfg,
                &self.health,
                ctx.now(),
            )
            .await?;
            Ok(Progress {
                units: (pass.sent + pass.retrying + pass.dead_lettered) as u64,
                more: pass.more,
            })
        })
    }
}

// ── commandlog.prune ─────────────────────────────────────────────────────

/// Cut the chain back to its floor.
///
/// **Off by default, and it stays an operator act.** Pruning is the one
/// worker whose failure mode is destructive rather than delayed. It is
/// *safe* — `prune` refuses anything the floor does not permit and returns
/// a typed `PruneRefusal` — and it is heavyweight, taking the append lock.
/// So the slot exists, the cadence is a named schedule, and enabling it is
/// one deliberate variable. A deployment that never sets it loses nothing
/// except automation.
pub struct CommandlogPrune {
    pub verifier: ordo_commandlog::CheckpointVerifier,
    pub policy: ordo_exec::cadence::LogPolicy,
    pub period: Duration,
}

impl Worker for CommandlogPrune {
    fn name(&self) -> &'static str {
        "commandlog.prune"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Schedule {
            name: "commandlog.prune",
            period: self.period,
        }
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let space = LogSpace(ctx.space.0.clone());
            match ordo_exec::prune::prune(
                &ctx.pool,
                &space,
                &self.verifier,
                &self.policy,
                ctx.now(),
            )
            .await
            {
                Ok(ordo_exec::prune::Pruned::Removed { entries, .. }) => Ok(Progress::did(entries)),
                // Not a refusal: a space with no qualifying checkpoint is a
                // space whose anchor has not come round yet.
                Ok(ordo_exec::prune::Pruned::NothingToPrune { .. }) => Ok(Progress::idle()),
                Err(e) => Err(WorkerError(e.to_string())),
            }
        })
    }
}

// ── usage.flush ──────────────────────────────────────────────────────────

/// Drain this replica's in-process usage ledger into `$usage_replica`, once
/// a tick, on **every** replica.
///
/// **No leadership**, and for the reason `TriggersDrain` states above it:
/// a per-replica quantity (`Metrics::usage_take`) lives only in this
/// process's memory, and the leader cannot read a follower's. Every
/// replica flushes its own counts under its own id, so two replicas in one
/// period are two `$usage_replica` rows that `usage.rollup` sums rather
/// than one row a race could drop.
///
/// The period is this worker's own tick, not a `$schedules` claim — like
/// `CommandlogCheckpoint`, letting the supervisor claim a schedule on top
/// would be two claims of one cadence, obligation 2's bug.
pub struct UsageFlush {
    pub metrics: Arc<Metrics>,
    pub replica: String,
    pub period: Duration,
    /// 17n M2: refreshed on every tick, **after** this replica's rows are
    /// written — so a limit this replica's own traffic just crossed is
    /// visible on the very next request rather than one flush interval
    /// later. `None` when quotas are off (the ledger is off), the same
    /// posture `usage.rollup`'s registration takes.
    pub quotas: Option<Arc<PgQuotas>>,
}

impl UsageFlush {
    /// Drain the ledger and write one `$usage_replica` row per space that
    /// had traffic, all under the current period. Returns the number of
    /// spaces written and the period's own bounds, so `run` and
    /// `on_shutdown` can each decide what to close.
    async fn flush(&self, ctx: &WorkerCtx) -> Result<(u64, u64, u64), WorkerError> {
        let taken = self.metrics.usage_take();
        let units = taken.len() as u64;
        let (period_start, period_end) = usage::period_of(ctx.now(), self.period.as_secs());
        for (space, counters) in taken {
            let row = usage::ReplicaRow {
                space: LogSpace(space),
                period_start,
                period_end,
                replica: self.replica.clone(),
                requests: counters
                    .requests
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
                rows_returned: counters.rows_returned,
                worker_runs: counters.worker_runs,
                state: usage::ReplicaState::Open,
                method_version: usage::METHOD_VERSION.to_string(),
            };
            usage::add_replica(&ctx.pool, &row).await?;
        }
        Ok((units, period_start, period_end))
    }
}

impl Worker for UsageFlush {
    fn name(&self) -> &'static str {
        "usage.flush"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.period)
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let (units, period_start, _) = self.flush(ctx).await?;
            // Every period this replica has moved past closes; the one it
            // just wrote stays `'open'` until a later tick moves past it
            // too, or `on_shutdown` closes it directly.
            usage::close_replica_periods(&ctx.pool, &self.replica, period_start).await?;
            // 17n M2: after this replica's own rows are durable, reload the
            // quota snapshot the door reads — never before, or a limit this
            // tick just crossed would wait a full period to take effect.
            if let Some(quotas) = &self.quotas {
                quotas.refresh(&ctx.pool, ctx.now()).await?;
            }
            Ok(Progress::did(units))
        })
    }
    fn on_shutdown<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>> {
        Box::pin(async move {
            let (_, _, period_end) = self.flush(ctx).await?;
            // Unlike `run`, also close the period just written: this
            // replica will never flush again, so nothing it wrote this
            // period can still arrive.
            usage::close_replica_periods(&ctx.pool, &self.replica, period_end).await?;
            Ok(())
        })
    }
}

// ── usage.rollup ─────────────────────────────────────────────────────────

/// One `run`'s cap on spaces touched, so a very large fleet does not hold
/// the leader on a single pass. `Progress::more` carries the rest — cheap
/// to re-enter, since a space this pass already rolled up finds nothing
/// new on the next one and costs one no-op read.
const ROLLUP_SPACE_BATCH: usize = 64;

/// Roll `$commandlog`, `$sealedpayload` and `$blobref` into one `$usage`
/// row per space per period. **Leader-only**, on its own named schedule
/// (obligation 2: the schedule carries its own name and period, so this
/// cadence can never inherit another worker's).
///
/// Walks every database [`crate::spaces::Spaces::pools`] can reach
/// (`ordo_exec::route::PoolResolver::targets`), not only the one this
/// process booted with — a space's rows can be in an attached database
/// since S4, and a rollup that only read the default pool would silently
/// stop counting an attached space's writes.
///
/// # `chain_from`/`chain_to` and why the lower bound has two shapes
///
/// A space's first-ever rollup has no prior `$usage` row to continue from,
/// so `scan_from` is its smallest stored `idx` — inclusive, because that
/// entry has never been counted. Every rollup after continues from the
/// previous row's `chain_to + 1` — the entry AT `chain_to` was already
/// counted by that row, so this window must not touch it again. Both
/// arrive at [`usage::chain_window`] as one inclusive `>=` lower bound;
/// which shape it took is decided here, not there.
pub struct UsageRollup {
    pub spaces: Arc<crate::spaces::Spaces>,
    pub period: Duration,
}

impl Worker for UsageRollup {
    fn name(&self) -> &'static str {
        "usage.rollup"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Schedule {
            name: "usage.rollup",
            period: self.period,
        }
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let (period_start, period_end) = usage::period_of(ctx.now(), self.period.as_secs());
            let targets = self
                .spaces
                .pools()
                .targets()
                .await
                .map_err(|e| WorkerError(format!("usage.rollup: the target list: {e}")))?;

            // Every space with chain entries anywhere, paired with the pool
            // that reaches it. **An unreachable target no longer aborts the
            // whole pass** (18d R-O5/D-G): the earlier shape returned here,
            // which meant one down cluster left every tenant unmeasured for
            // the period, including every tenant on a target that answered
            // fine — the metering ledger's own single point of failure,
            // and a stall rather than an explicit gap. What cannot be
            // measured is recorded as one instead, per space, and the rest
            // of the fleet is rolled up as if the down target were not
            // there, because for them it is not.
            let mut pending: Vec<(String, ordo_exec::Pool, u64, u64)> = Vec::new();
            let mut unreachable: Vec<(&str, &str)> = Vec::new();
            for t in &targets {
                match &t.pool {
                    Ok(p) => {
                        for (space, min_idx, max_idx) in usage::chain_heads(p).await? {
                            pending.push((space, p.clone(), min_idx, max_idx));
                        }
                    }
                    Err(e) => unreachable.push((t.label.as_str(), e.as_str())),
                }
            }
            if !unreachable.is_empty() {
                // Which tenants this target actually holds, read from the
                // control database rather than assumed: a target's own
                // pool being down says nothing about `$spacedb`, which
                // lives on the pool this worker is already running on.
                // `default` (the control database) can never land here —
                // if it were unreachable this worker would not be
                // running (R-F5) — so every gap below names a real
                // routed target.
                for (space, attachment) in spacedb::all(&ctx.pool).await? {
                    let label =
                        route::target_label(&attachment.host, attachment.port, &attachment.db);
                    if let Some((_, reason)) = unreachable.iter().find(|(l, _)| *l == label) {
                        usage::record_gap(
                            &ctx.pool,
                            &space.0,
                            period_start,
                            period_end,
                            &label,
                            reason,
                            ctx.now(),
                        )
                        .await?;
                    }
                }
            }

            let more = pending.len() > ROLLUP_SPACE_BATCH;
            pending.truncate(ROLLUP_SPACE_BATCH);

            let live = ctx.deployment.current();
            let entities: Vec<String> = live
                .fixture
                .schema
                .entities
                .iter()
                .map(|e| e.name.clone())
                .collect();
            drop(live);

            let mut touched = 0u64;
            for (space, pool, min_idx, max_idx) in pending {
                let prior = usage::last_space_row(&ctx.pool, &space).await?;
                let (scan_from, chain_from) = match &prior {
                    Some(r) => (r.chain_to + 1, r.chain_to),
                    None => (min_idx, min_idx),
                };
                if scan_from > max_idx {
                    // Nothing new since the last rollup: no row to write,
                    // and nothing to count a second time.
                    continue;
                }
                let (writes, rows_written) =
                    usage::chain_window(&pool, &space, scan_from, max_idx).await?;
                let bytes_sealed = usage::bytes_sealed(&pool, &space).await?;
                let bytes_chain = usage::bytes_chain(&pool, &space).await?;
                let bytes_blob = usage::bytes_blob(&pool, &space).await?;
                let mut bytes_rows_estimated = 0u64;
                for entity in &entities {
                    bytes_rows_estimated +=
                        usage::table_bytes_estimate(&pool, entity, &space).await?;
                }
                usage::upsert_space(
                    &ctx.pool,
                    &usage::SpaceRow {
                        space: LogSpace(space),
                        period_start,
                        period_end,
                        chain_from,
                        chain_to: max_idx,
                        writes,
                        rows_written,
                        bytes_sealed,
                        bytes_chain,
                        bytes_blob,
                        bytes_rows_estimated,
                        method_version: usage::METHOD_VERSION.to_string(),
                    },
                )
                .await?;
                touched += 1;
            }
            Ok(Progress {
                units: touched,
                more,
            })
        })
    }
}

// ── keys.reseal ──────────────────────────────────────────────────────────

/// K1: move every ciphertext a space holds off a retired epoch, one chunk
/// per run.
///
/// The pass itself is `ordo_exec::reseal`; this is the loop that calls it,
/// and three decisions live here rather than there:
///
/// - **Leader-only.** Two replicas stepping one pass would both read a
///   chunk and both try the CAS; the second would count every row as
///   skipped and the pass would still terminate, so leadership is an
///   optimisation and not a correctness rule, exactly as it is for
///   `triggers.clock`.
/// - **The tick is the throttle.** A chunk is one `run`, `more` is never
///   set, and `pause` is the inter-chunk delay `implementation.md`
///   requires beside "resumable". `ORDO_RESEAL_CHUNK` and
///   `ORDO_RESEAL_PAUSE_MS` are the two knobs.
/// - **The gauges are set before the step, from the table.** A wedged pass
///   fails its step and backs off; if the gauges were set after, a wedged
///   pass would report nothing, which is the one state the stuck gauge
///   exists to make visible.
///
/// Completion is recorded as `CustodyAction { action: "reseal_completed" }`
/// in the space's own security stream, the exit-ops precedent: a re-seal
/// changes no row's value and appends no command group.
pub struct KeysReseal {
    pub chunk: usize,
    pub pause: Duration,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
    /// Where the space's objects are. `None` on a deployment with no blob
    /// tier: the object population is then counted, empty, and closed.
    pub stores: Option<Arc<dyn ordo_blob::StoreResolver>>,
}

impl KeysReseal {
    pub const DEFAULT_CHUNK: usize = 256;
    pub const DEFAULT_PAUSE: Duration = Duration::from_millis(250);

    /// Read the two knobs, falling back to the defaults. A value that does
    /// not parse is the default.
    pub fn from_env(
        stream: Option<Arc<ordo_secevent::Recorder>>,
        stores: Option<Arc<dyn ordo_blob::StoreResolver>>,
    ) -> KeysReseal {
        let chunk = std::env::var("ORDO_RESEAL_CHUNK")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(Self::DEFAULT_CHUNK);
        let pause = std::env::var("ORDO_RESEAL_PAUSE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Self::DEFAULT_PAUSE);
        KeysReseal {
            chunk,
            pause,
            stream,
            stores,
        }
    }

    /// Publish the two K3 gauges for whatever pass is open, and return
    /// whether one is.
    async fn report(
        &self,
        ctx: &WorkerCtx,
        schema: &ordo_schema::Schema,
    ) -> Result<bool, WorkerError> {
        let now = ctx.now();
        let rows = reseal::passes(&ctx.pool, &ctx.space).await?;
        let open: Vec<&reseal::PassRow> = rows
            .iter()
            .filter(|r| r.state != reseal::PassState::Done)
            .collect();
        let Some(from) = open.iter().map(|r| r.from_epoch).min() else {
            ctx.metrics
                .gauge(crate::metrics::RESEAL_STUCK, &[("space", &ctx.space.0)], 0);
            return Ok(false);
        };
        let left = reseal::remaining(&ctx.pool, schema, &ctx.space, from).await?;
        for p in reseal::Population::ALL {
            ctx.metrics.gauge(
                crate::metrics::RESEAL_REMAINING,
                &[("space", &ctx.space.0), ("population", p.as_str())],
                left.of(p).max(0) as u64,
            );
        }
        let advanced = open
            .iter()
            .filter(|r| r.from_epoch == from)
            .map(|r| r.advanced_at)
            .max()
            .unwrap_or(now);
        let stuck = if left.total() > 0 {
            now.saturating_sub(advanced)
        } else {
            0
        };
        ctx.metrics.gauge(
            crate::metrics::RESEAL_STUCK,
            &[("space", &ctx.space.0)],
            stuck,
        );
        Ok(true)
    }

    fn record_completed(&self, ctx: &WorkerCtx, from: ordo_crypto::Epoch) {
        let Some(stream) = &self.stream else { return };
        stream.observe(
            ordo_secevent::Observation {
                space: Some(LogSpace(ctx.space.0.clone())),
                subject: ordo_secevent::Subject::platform("keys.reseal"),
                op: "keys.reseal".into(),
                kind: ordo_secevent::EventKind::CustodyAction {
                    action: format!("reseal_completed epoch={from}"),
                },
                at: Timestamp(ctx.now()),
                correlation: ordo_secevent::correlation::mint(),
            },
            ordo_secevent::Relationship::Custodian,
        );
    }
}

impl Worker for KeysReseal {
    fn name(&self) -> &'static str {
        "keys.reseal"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.pause)
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let loaded = ctx.deployment.current();
            let schema = &loaded.fixture.schema;
            if !self.report(ctx, schema).await? {
                return Ok(Progress::idle());
            }
            let engine = reseal::Reseal {
                pool: &ctx.pool,
                keys: ctx.keys.as_ref(),
                schema,
                space: &ctx.space,
                chunk: self.chunk,
                stores: self.stores.as_deref(),
            };
            match engine.step(ctx.now()).await? {
                reseal::Step::Idle | reseal::Step::Blocked { .. } => Ok(Progress::idle()),
                reseal::Step::Moved { done, .. } => Ok(Progress::did(done)),
                reseal::Step::Reopened { .. } => Ok(Progress::idle()),
                reseal::Step::Completed { from_epoch } => {
                    self.record_completed(ctx, from_epoch);
                    // The gauges say zero now rather than on the next tick.
                    self.report(ctx, schema).await?;
                    Ok(Progress::did(1))
                }
            }
        })
    }
}

// ── blob.gc ──────────────────────────────────────────────────────────────

/// The storage sweep's runner (`plan-tenant-storage.md` U8): one pass over
/// every database and every store, on one cadence for the whole deployment.
///
/// `blobgc::sweep` was built and floored long before this ran it. What it
/// waited on was a cadence, and the owner ruled it one deployment-wide
/// setting (D-S3, 2026-09-21): a cadence per attachment would be many
/// schedules to reason about, and would let a tenant defer their own
/// deletions past whatever retention their own disclosure promised. What
/// varies per tenant is the cost of a sweep, and the floor and the sweep's
/// own bounds already carry that.
///
/// **Two dimensions, databases first** (§6 B-G3). `$blobref` lives in a
/// tenant's database and the objects live in a tenant's store, so the outer
/// loop is `PoolResolver::targets()` and the inner is each space's resolved
/// store, through the same `StoreResolver` the door writes through. The
/// database walk is outer because the reference set bounds the work; there
/// is no way to enumerate from the store side and there must not be one
/// (B-G5). Running this against a `SinglePool` would collect only the
/// default database's tenants while reporting a complete pass, which is why
/// U8 waited for T2's single resolver.
///
/// **An unreachable target or store is an outcome, not a fault** (B-G4).
/// A database that cannot be dialled, or a store that refuses a delete,
/// stops that tenant's sweep and stamps nothing: the tombstone stays open
/// and the next pass retries. The rest of the fleet is swept as if the down
/// one were not there, each miss is counted on
/// [`crate::metrics::BLOB_GC_UNCOLLECTED`] by cause and written to the
/// log, and the pass fails only when it could sweep nothing at all.
///
/// **Leader-only on a named schedule**, like `usage.rollup`: the sweep is
/// idempotent, so leadership is an optimisation rather than a correctness
/// rule, and the schedule row carries its own name and period (obligation
/// 2) so this cadence can never inherit another worker's.
///
/// The floor's tables are ensured on every database visited. The control
/// database gets them from `Supervisor::start`; a tenant's own database
/// gets them here, because nothing else reads the prune floor there and the
/// guard is `CREATE TABLE IF NOT EXISTS`.
///
/// **Deletions are recorded in the tenant's own security stream** (owner,
/// 2026-10-05, `audit-2026-10-04` ask 4): one entry per pass per space, not
/// one per object. It is a `CustodyAction` attributed to the platform
/// component `blob.gc`, routed as a custodian the way `keys.reseal` records
/// completion, so it lands in the space's stream. Its action line is
/// `blob_collected deleted=N not_deleted_store=N not_deleted_database=N
/// pass=<id>`, and every entry of one pass shares that pass's correlation. A
/// pass that stopped writes its entry even when it deleted nothing
/// (`deleted=0`, owner, 2026-10-05, `audit-2026-10-05` ask 2): storage that
/// refused every object is a fact the tenant's stream should hold. A space
/// with nothing due gets no entry.
pub struct BlobGc {
    pub spaces: Arc<crate::spaces::Spaces>,
    pub stores: Arc<dyn ordo_blob::StoreResolver>,
    /// The undo horizon the sweep is floored at: the same policy the log
    /// prune reads, which is the whole point of `blobgc`'s floor.
    pub policy: ordo_exec::cadence::LogPolicy,
    /// `ORDO_BLOB_GC_SECS`, default an hour.
    pub period: Duration,
    /// Where each pass's deletions are recorded. `None` records nothing,
    /// which only a test without a stream wants.
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
}

impl BlobGc {
    /// The shipped cadence: an hour. The floor is ninety days by default,
    /// so a sweep that comes round hourly collects within an hour of an
    /// object becoming collectable, and a pass over a quiet tenant is a few
    /// indexed reads.
    pub const DEFAULT_SECS: u64 = 3600;

    /// `ORDO_BLOB_GC_SECS`. Zero or unparseable falls back to the default
    /// rather than disabling the sweep, the posture `ORDO_RESEAL_CHUNK` and
    /// `ORDO_ACCESS_REFRESH_SECS` take.
    pub fn period_from_env() -> Duration {
        let secs = std::env::var("ORDO_BLOB_GC_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|s| *s > 0)
            .unwrap_or(Self::DEFAULT_SECS);
        Duration::from_secs(secs)
    }

    /// One space's entry for this pass. Nothing when the pass deleted
    /// nothing and did not stop, which is a space with nothing due.
    fn record_pass(
        &self,
        ctx: &WorkerCtx,
        pass: &ordo_commandlog::CorrelationId,
        report: &ordo_exec::blobgc::SweepReport,
    ) {
        let Some(stream) = &self.stream else { return };
        if report.deleted == 0 && report.stopped.is_none() {
            return;
        }
        let not_deleted = |cause: ordo_exec::blobgc::Uncollected| {
            report
                .stopped
                .as_ref()
                .filter(|s| s.cause == cause)
                .map_or(0, |s| s.not_deleted)
        };
        stream.observe(
            ordo_secevent::Observation {
                space: Some(LogSpace(report.space.0.clone())),
                subject: ordo_secevent::Subject::platform("blob.gc"),
                op: "blob.gc".into(),
                kind: ordo_secevent::EventKind::CustodyAction {
                    action: format!(
                        "blob_collected deleted={} not_deleted_store={} \
                         not_deleted_database={} pass={}",
                        report.deleted,
                        not_deleted(ordo_exec::blobgc::Uncollected::Store),
                        not_deleted(ordo_exec::blobgc::Uncollected::Database),
                        pass.0,
                    ),
                },
                at: Timestamp(ctx.now()),
                correlation: pass.clone(),
            },
            ordo_secevent::Relationship::Custodian,
        );
    }
}

impl Worker for BlobGc {
    fn name(&self) -> &'static str {
        "blob.gc"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Schedule {
            name: "blob.gc",
            period: self.period,
        }
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let targets = self
                .spaces
                .pools()
                .targets()
                .await
                .map_err(|e| WorkerError(format!("blob.gc: the target list: {e}")))?;
            let loaded = ctx.deployment.current();
            let schema = &loaded.fixture.schema;
            // Tombstones are stamped in milliseconds; the supervisor's clock
            // is seconds. `blobgc::sweep` converts for the floor itself.
            let now_ms = ctx.now().saturating_mul(1_000);

            // One id for the whole pass: every space's entry carries it, so
            // a reader can gather what one pass did across the fleet.
            let pass = ordo_secevent::correlation::mint();
            let mut units = 0u64;
            let mut swept = 0usize;
            let mut notes: Vec<String> = Vec::new();
            let mut miss = |cause: &str, note: String| {
                ctx.metrics
                    .incr(crate::metrics::BLOB_GC_UNCOLLECTED, &[("cause", cause)]);
                notes.push(note);
            };
            for t in &targets {
                let pool = match &t.pool {
                    Ok(p) => p,
                    Err(e) => {
                        miss(
                            "target",
                            format!("{}: not reached, so not swept: {e}", t.label),
                        );
                        continue;
                    }
                };
                if let Err(e) = ordo_exec::prune::ensure_tables(pool).await {
                    miss("database", format!("{}: the floor's tables: {e}", t.label));
                    continue;
                }
                if let Err(e) = ordo_exec::cadence::ensure_tables(pool).await {
                    miss("database", format!("{}: the floor's tables: {e}", t.label));
                    continue;
                }
                let spaces = match ordo_exec::blobgc::spaces_with_objects(pool).await {
                    Ok(s) => s,
                    Err(e) => {
                        miss("database", format!("{}: the reference set: {e}", t.label));
                        continue;
                    }
                };
                swept += 1;
                for space in &spaces {
                    match ordo_exec::blobgc::sweep(
                        pool,
                        self.stores.as_ref(),
                        schema,
                        space,
                        &self.policy,
                        now_ms,
                    )
                    .await
                    {
                        Ok(r) => {
                            units += r.marked + r.cancelled + r.deleted;
                            if let Some(stop) = &r.stopped {
                                miss(
                                    stop.cause.as_str(),
                                    format!(
                                        "{}/{}: {} object(s) not collected: {}",
                                        t.label, space.0, stop.not_deleted, stop.error
                                    ),
                                );
                            }
                            self.record_pass(ctx, &pass, &r);
                        }
                        Err(ordo_exec::blobgc::SweepError::Blob(e)) => miss(
                            "store",
                            format!(
                                "{}/{}: store unreachable, objects not collected: {e}",
                                t.label, space.0
                            ),
                        ),
                        Err(e) => miss(
                            "database",
                            format!("{}/{}: not swept: {e}", t.label, space.0),
                        ),
                    }
                }
            }
            for n in &notes {
                eprintln!("worker blob.gc: {n}");
            }
            if swept == 0 && !targets.is_empty() {
                return Err(WorkerError(format!(
                    "no database could be swept: {}",
                    notes.join("; ")
                )));
            }
            Ok(if units == 0 {
                Progress::idle()
            } else {
                Progress::did(units)
            })
        })
    }
}

/// Time out a provision that stopped part way (`bootstrap.md` §4.1,
/// checklist 11-15).
///
/// > *"`provisioning` older than `bootstrapTimeout` (default 15 min) is a
/// > reconciler target. The reconciler is an ordinary job-queue worker."*
///
/// # What it does, and deliberately does not do
///
/// It moves a stale `provisioning` row to `failed`, and **nothing else**.
/// It does not release the tenant's resources, because releasing them is
/// `std.space.reap`, which is operator-only and step-up gated for a reason
/// §4.5 states plainly: a process that destroys databases without a human
/// asking is a process that will eventually destroy the wrong one. The
/// reconciler's whole job is to make a stuck provision **visible** —
/// `failed` is what puts it in `tenants::troubled`, which is §4.1's one
/// detection query and the operator surface's mandatory pane.
///
/// So the loop is: creation dies, the reconciler notices within the
/// timeout and says so, and a person decides whether to reap.
///
/// # Why the timeout is generous
///
/// Fifteen minutes is the plan's default and it is long on purpose. A
/// provision creates a database and applies a schema to it, which on a
/// slow or contended cluster is minutes rather than seconds, and a
/// reconciler that fired early would mark a *working* provision failed and
/// race the call that is still running. `activate` is guarded on
/// `provisioning`, so that race is caught rather than silently doubled —
/// the create would find its row no longer `provisioning` and say so — but
/// the right answer is not to have the race.
pub struct ProvisioningReconciler {
    /// How long a provision may stay `provisioning` before it is called
    /// failed. `ORDO_BOOTSTRAP_TIMEOUT_SECS`, default 15 minutes.
    pub timeout: Duration,
}

impl ProvisioningReconciler {
    /// The plan's default, and the environment override.
    pub fn from_env() -> ProvisioningReconciler {
        let secs = std::env::var("ORDO_BOOTSTRAP_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|s| *s > 0)
            .unwrap_or(15 * 60);
        ProvisioningReconciler {
            timeout: Duration::from_secs(secs),
        }
    }
}

impl Worker for ProvisioningReconciler {
    fn name(&self) -> &'static str {
        "provisioning.reconciler"
    }
    fn cadence(&self) -> Cadence {
        // A minute. The timeout it enforces is fifteen, so this decides
        // only how promptly a stuck provision surfaces within that window,
        // and the work is one indexed read of a small table.
        Cadence::Tick(Duration::from_secs(60))
    }
    fn requires_leadership(&self) -> bool {
        // One replica. Two would both read the same stale rows and both
        // write `failed`; the write is idempotent and guarded on
        // `provisioning`, so the result would be correct and the second
        // replica's work wasted. Leadership is the cheaper way to say it.
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let now = ctx.now();
            // The sweep itself lives on `tenants`, beside the table it is
            // about and testable without a supervisor. This worker is the
            // cadence and the leadership, nothing more.
            let timed_out = crate::tenants::time_out_stale(&ctx.pool, self.timeout.as_secs(), now)
                .await
                .map_err(|e| WorkerError(e.to_string()))?;
            Ok(if timed_out == 0 {
                Progress::idle()
            } else {
                Progress::did(timed_out)
            })
        })
    }
}

// ── access.refresh ───────────────────────────────────────────────────────

/// Re-read who the members are, on a timer, **without restarting
/// anything**.
///
/// The door answers from an in-memory engine built from the `members`
/// table. Until 2026-09-28 that engine was rebuilt in exactly two
/// situations, a deploy and the creation of a tenant, so a member who was
/// removed or suspended kept being served until the next deploy or restart
/// (`security-review-6.md` S6-004, S6-005). The owner's ruling on it was one
/// sentence and is the requirement: *"We shouldn't be restarting deployments
/// because someone got or lost access."*
///
/// **What a refresh is.** One read of `members`, a new engine built from
/// it, and an atomic swap of the pointer the door reads through
/// (`SwappableAccess::install`). A request already in flight finishes
/// against the engine it started with; the next one sees the new one. No
/// connection is dropped, no program is reloaded, and no other worker
/// notices.
///
/// **Why a timer and not only a notification.** `MEMBERS_CHANNEL` already
/// refreshes every replica at once when something announces on it, and an
/// op that changes membership should announce. The timer is the **ceiling**
/// for every case a notification cannot cover: a row changed by hand, a
/// notification dropped while a replica was reconnecting, and a tenant whose
/// rows live in a database of their own, where a `NOTIFY` fires in a
/// database nobody is listening on (`routing-notify`, still open). With the
/// timer the worst case is its period. Without it the worst case was the
/// next deploy.
///
/// **No leadership.** Each replica holds its own engine, so each one
/// refreshes its own. One leader refreshing would leave every other replica
/// exactly as stale as before.
pub struct AccessRefresh {
    pub access: Arc<crate::runtime::SwappableAccess>,
    pub token_secret: [u8; 32],
    pub delegations: Arc<crate::delegation::PgDelegationRegistry>,
    pub credentials: Arc<dyn ordo_exec::route::DbCredentials>,
    /// The invalidation bus, refreshed on the same tick so that a tenant
    /// whose database this node learned of without a notification (a missed
    /// one, or another node's provisioning) is listened to within one
    /// period. Its `unheard` databases are retried here too.
    pub bus: Option<Arc<crate::invalidation::PgInvalidationBus>>,
    /// `ORDO_ACCESS_REFRESH_SECS`, default 30.
    pub every: Duration,
}

impl AccessRefresh {
    /// The default period, and the floor.
    ///
    /// Thirty seconds: long enough that the read is noise beside a
    /// deployment's ordinary traffic, short enough that "remove them and
    /// wait" is an answer an operator can give during an incident.
    pub const DEFAULT_SECS: u64 = 30;

    pub fn period_from_env() -> Duration {
        let secs = std::env::var("ORDO_ACCESS_REFRESH_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|s| *s > 0)
            .unwrap_or(Self::DEFAULT_SECS);
        Duration::from_secs(secs)
    }
}

impl Worker for AccessRefresh {
    fn name(&self) -> &'static str {
        "access.refresh"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            crate::runtime::refresh_access(
                &ctx.deployment,
                &ctx.pool,
                &self.token_secret,
                &ctx.metrics,
                &self.access,
                self.delegations.clone(),
                &self.credentials,
            )
            .await
            .map_err(|e| WorkerError(format!("the door could not be rebuilt: {e}")))?;
            if let Some(bus) = &self.bus {
                bus.refresh().await.map_err(|e| {
                    WorkerError(format!("live updates could not be refreshed: {e}"))
                })?;
            }
            Ok(Progress {
                units: 1,
                more: false,
            })
        })
    }
    fn on_shutdown<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

// ── tenant-origins.refresh ───────────────────────────────────────────────

/// Re-read the tenants' own CORS origins from `$spaceorigins`
/// (`crate::tenant_origins`). The node that took a change has it at once;
/// this is how the others learn.
pub struct TenantOriginsRefresh {
    pub origins: Arc<crate::tenant_origins::TenantOriginMap>,
    pub every: Duration,
}

impl Worker for TenantOriginsRefresh {
    fn name(&self) -> &'static str {
        "tenant-origins.refresh"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn run<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let n = self.origins.load().await.map_err(WorkerError)?;
            Ok(Progress {
                units: n as u64,
                more: false,
            })
        })
    }
    fn on_shutdown<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

// ── second-step.levels.refresh ───────────────────────────────────────────

/// Re-read the tenants' second-step levels from `$admissionpolicy`.
///
/// The door answers a tenant's requirement from memory. An owner who sets a
/// level updates the replica that took the call at once; this is how the
/// others learn. Every replica runs it, for the reason `access.refresh` does.
pub struct SecondStepLevelsRefresh {
    pub levels: Arc<crate::second_factors::SecondStepLevels>,
    pub every: Duration,
}

impl Worker for SecondStepLevelsRefresh {
    fn name(&self) -> &'static str {
        "second-step.levels.refresh"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let n = self
                .levels
                .load(&ctx.pool)
                .await
                .map_err(|e| WorkerError(e.to_string()))?;
            // The same row holds the connections a tenant accepts a two-step
            // claim from (`plan-auth-federated.md` §8.2), re-read on the
            // same tick for the same reason.
            if let Some(acceptance) = crate::presence::upstream() {
                crate::admission::load_presence_from(&ctx.pool, &acceptance)
                    .await
                    .map_err(|e| WorkerError(e.to_string()))?;
            }
            Ok(Progress {
                units: n as u64,
                more: false,
            })
        })
    }
    fn on_shutdown<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

// ── tenants.pause.refresh ────────────────────────────────────────────────

/// Re-read which tenants are paused (27ar).
///
/// The pause gate answers from memory. A pause or resume updates the replica
/// that took it at once and announces on `ordo.members`; this is the ceiling
/// for a replica that missed the announcement. Every replica runs it, for
/// the reason `access.refresh` does.
pub struct PauseRefresh {
    pub spaces: Arc<crate::spaces::Spaces>,
    pub every: Duration,
}

impl Worker for PauseRefresh {
    fn name(&self) -> &'static str {
        "tenants.pause.refresh"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let n = crate::pause::reload(&self.spaces, &ctx.pool)
                .await
                .map_err(WorkerError)?;
            Ok(Progress {
                units: n as u64,
                more: false,
            })
        })
    }
    fn on_shutdown<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

// ── auth.ceremony.sweep ──────────────────────────────────────────────────

/// Delete the ceremonies that expired.
///
/// An expired row answers nothing whether or not it has been deleted
/// (`CeremonyStore::take` checks the expiry), so this is housekeeping and
/// not a security control: without it the table grows with every abandoned
/// page load. Every replica sweeps, and the delete is idempotent, so no
/// leadership is needed.
pub struct CeremonySweep {
    pub store: Arc<dyn ordo_identity::CeremonyStore>,
    /// The emailed-code counters and device markers (`$authrate`), when
    /// `email` is a method. Swept on the same tick.
    pub email: Option<Arc<dyn crate::email_code::EmailCodeStore>>,
    pub every: Duration,
}

impl CeremonySweep {
    pub const DEFAULT_SECS: u64 = 60;
}

impl Worker for CeremonySweep {
    fn name(&self) -> &'static str {
        "auth.ceremony.sweep"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn run<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            // A synchronous store behind a blocking call: off the worker's
            // thread.
            let store = self.store.clone();
            let email = self.email.clone();
            let now = ordo_identity::Timestamp(crate::runtime::now_secs());
            let swept = tokio::task::spawn_blocking(move || {
                let mut n = store.sweep(now).map_err(|e| e.to_string())?;
                if let Some(email) = email {
                    n += email.sweep(now.0).map_err(|e| e.0.to_string())?;
                }
                Ok::<u64, String>(n)
            })
            .await
            .map_err(|e| WorkerError(format!("the sweep task failed: {e}")))?
            .map_err(WorkerError)?;
            Ok(Progress {
                units: swept,
                more: false,
            })
        })
    }
    fn on_shutdown<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

// ── auth.recovery.sweep ──────────────────────────────────────────────────

/// End the contest window of every recovery that has run its course.
///
/// A recovery keeps the passkeys it retired for the window so the real owner
/// can contest. When the window closes this removes them and their public
/// keys, and closes the case. An expired window already refuses a contest
/// (`Directory::contest` checks it), so the sweep is what frees the
/// credentials and the case, not what enforces the deadline. Every replica
/// sweeps: closing a case is one atomic update, and removing a link twice is
/// harmless.
pub struct RecoverySweep {
    pub auth: Arc<crate::auth::AuthState>,
    pub every: Duration,
}

impl RecoverySweep {
    pub const DEFAULT_SECS: u64 = 60;
}

impl Worker for RecoverySweep {
    fn name(&self) -> &'static str {
        "auth.recovery.sweep"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn run<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let auth = self.auth.clone();
            let swept =
                tokio::task::spawn_blocking(move || {
                    let (Some(recovery), Some(passkey)) = (auth.recovery(), auth.passkey.as_ref())
                    else {
                        return Ok(0u64);
                    };
                    let mut dir = auth.lock_directory();
                    let gone = recovery.sweep(&mut dir).map_err(|e| e.to_string())?;
                    for (authenticator, _, _) in &gone {
                        if let Some(raw) = authenticator.0.strip_prefix("passkey:").and_then(|id| {
                            ordo_identity::passkey::credential_id_from_b64url(id).ok()
                        }) {
                            passkey.remove_credential(&raw).map_err(|e| e.to_string())?;
                        }
                    }
                    Ok::<u64, String>(gone.len() as u64)
                })
                .await
                .map_err(|e| WorkerError(format!("the sweep task failed: {e}")))?
                .map_err(WorkerError)?;
            Ok(Progress {
                units: swept,
                more: false,
            })
        })
    }
    fn on_shutdown<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

// ── sessions.refresh ─────────────────────────────────────────────────────

/// Re-read the session deny list, so a logout and an ended set of sessions
/// reach every replica without a restart.
///
/// `PgSessionRevocations` answers `is_revoked` and `principal_ended` from
/// memory, on every request, with no I/O. That is what keeps the door cheap,
/// and it means a replica learns of a revocation written by another only when
/// something re-reads the table. Nothing did before this worker: a second
/// replica would have honoured a logged-out session until it restarted.
/// **The period is the longest a second replica keeps honouring an ended
/// session**, so it is a security setting like `ORDO_ACCESS_REFRESH_SECS`.
///
/// The refresh also deletes rows past their ceiling, which keeps the read
/// cheap. **No leadership**: each replica holds its own memory, so each one
/// refreshes its own.
pub struct SessionsRefresh {
    pub store: Arc<crate::sessions::PgSessionRevocations>,
    /// Renewals that idled out are dropped on the same tick
    /// ([`crate::renewal`]). `None` when there are none.
    pub renewals: Option<Arc<dyn crate::renewal::RenewalStore>>,
    /// Which tenant each session was for, dropped past its ceiling
    /// ([`crate::session_policy`]).
    pub policies: Option<Arc<crate::session_policy::SessionPolicies>>,
    pub every: Duration,
}

impl SessionsRefresh {
    /// The default period. Short, because the alternative is a session that
    /// a person ended and another replica still serves.
    pub const DEFAULT_SECS: u64 = 15;

    /// `ORDO_SESSION_REFRESH_SECS`. Zero or unparseable falls back to the
    /// default rather than disabling the refresh.
    pub fn period_from_env() -> Duration {
        let secs = std::env::var("ORDO_SESSION_REFRESH_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|s| *s > 0)
            .unwrap_or(Self::DEFAULT_SECS);
        Duration::from_secs(secs)
    }
}

impl Worker for SessionsRefresh {
    fn name(&self) -> &'static str {
        "sessions.refresh"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn run<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            self.store
                .refresh()
                .await
                .map_err(|e| WorkerError(format!("the deny list could not be re-read: {e}")))?;
            if let Some(renewals) = self.renewals.clone() {
                let now = crate::runtime::now_secs();
                tokio::task::spawn_blocking(move || renewals.sweep(now))
                    .await
                    .map_err(|e| WorkerError(format!("the renewal sweep stopped: {e}")))?
                    .map_err(|e| WorkerError(format!("idle renewals could not be dropped: {e}")))?;
            }
            if let Some(policies) = &self.policies {
                policies
                    .sweep(crate::runtime::now_secs())
                    .await
                    .map_err(|e| {
                        WorkerError(format!("ended sessions could not be dropped: {e}"))
                    })?;
            }
            Ok(Progress {
                units: 1,
                more: false,
            })
        })
    }
    fn on_shutdown<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>> {
        Box::pin(async { Ok(()) })
    }
}
