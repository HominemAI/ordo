//! 25i, **the catch-up: a tenant's database follows the program.**
//!
//! An apply reaches the deployment's own database, and through the door it
//! reaches every attached database too ([`crate::apply_op`]). It cannot reach
//! a database that was down at that moment, one whose tenant was created
//! from an older version a moment before the swap, or any database when the
//! apply came from `ordo apply --db`, which by its own contract touches one
//! database. Each of those leaves a tenant whose database records an older
//! version than the one this deployment serves, and the schema comparison
//! ([`crate::spaces::Spaces`], R-S4) holds that tenant closed.
//!
//! This module is what opens it again. It is the **reconciler's tick**, one
//! more worker beside the ones [`crate::workers`] registers, and it needs no
//! new mechanism: the resolver already knows which spaces it is holding
//! closed for a database that is behind ([`Spaces::awaiting_schema`]), and
//! [`Spaces::reconcile`] already lifts a hold the moment the comparison
//! passes. The catch-up supplies the one missing act between the two: it
//! runs [`crate::schema_apply::bring_forward`] on the database.
//!
//! # When it runs
//!
//! - **At boot**, one sweep over every database the resolver lists.
//! - **When this node swaps to a new version** ([`CatchUp::nudge`]), on every
//!   replica, so a program applied straight to the deployment's database
//!   reaches the tenants within seconds and not at the next tick.
//! - **On the tick** (`ORDO_TENANT_CATCHUP_SECS`, default 15), for each space
//!   the register holds as behind, which is how a tenant that is asked for
//!   later finds its database moved. A sweep is repeated on the tick when
//!   the last one left a database it could not reach.
//!
//! The tick is not leader-only. Every replica runs it, because every replica
//! holds its own register and each must lift its own holds: one replica
//! brings the database forward under the apply lock and the others find it
//! already there, write nothing, and lift.
//!
//! # What it will not do
//!
//! It does not apply anything the deployment's own apply would refuse. A
//! refused change keeps the tenant closed with the refusal as its reason,
//! and is not asked again for the same version: asking a database the same
//! question every tick to hear the same refusal would fill its
//! `$migrations` with nothing. A new version, or a restart, asks again.
//!
//! It does not delete a dropped field's column
//! ([`crate::schema_apply::bring_forward`] says why).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_exec::spacedb::TargetVersion;

use crate::apply_op::ApplyLock;
use crate::schema_apply::{self as sa, Forward, Program};
use crate::spaces::Spaces;
use crate::supervisor::{BoxFut, Cadence, Progress, Worker, WorkerCtx, WorkerError};

/// The actor written into `$migrations` and the security stream for a
/// database the catch-up moved.
pub const ACTOR: &str = "platform:tenant-catch-up";
/// The op string beside it.
pub const OP: &str = "tenant.catch_up";

/// What the catch-up did for one space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caught {
    /// The database was brought to the serving version and the space lifted.
    Advanced,
    /// The database already records the serving version. The space was
    /// re-asked, so a replica that did not move it lifts its own hold.
    Current,
    /// The change was refused. The space stays closed with this reason, and
    /// is not asked again for this version.
    Refused(String),
    /// The database could not be asked or did not answer in time. Asked
    /// again on the next tick.
    Waiting(String),
    /// Another apply or attach holds the deployment, or the version moved
    /// while this waited. Asked again on the next tick.
    Busy,
    /// This node has no serving program to bring a database to.
    NoProgram,
}

/// What one sweep over the databases did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Swept {
    pub advanced: usize,
    pub current: usize,
    /// `(target, reason)` for each database that was not brought forward.
    pub held: Vec<(String, String)>,
    /// A database could not be reached or asked. Another sweep may find it.
    pub transient: bool,
}

/// The catch-up, one per node, shared by the worker and the deploy listener.
pub struct CatchUp {
    spaces: Arc<Spaces>,
    /// The deployment's own database: where the registry and the apply lock
    /// are.
    control: ordo_exec::Pool,
    /// The serving program, compiled once per version.
    program: Mutex<Option<Arc<Program>>>,
    /// Sweep on the next tick. Set at start, when the node swaps to a new
    /// version, and when a sweep left a database it could not reach.
    resweep: AtomicBool,
    /// One sweep at a time on this node.
    sweeping: tokio::sync::Mutex<()>,
    /// Databases being brought forward right now, and the most that were at
    /// once. The second is what the tests read to see the cap hold.
    in_flight: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
    /// Where a finished sweep is reported, so `ordo apply` can wait for it
    /// (`$nodes`). Unset in tests and embeddings that run no door.
    status: std::sync::OnceLock<Arc<crate::node_status::NodeStatus>>,
}

impl CatchUp {
    pub fn new(spaces: Arc<Spaces>, control: ordo_exec::Pool) -> Arc<CatchUp> {
        Arc::new(CatchUp {
            spaces,
            control,
            program: Mutex::new(None),
            resweep: AtomicBool::new(true),
            sweeping: tokio::sync::Mutex::new(()),
            in_flight: std::sync::atomic::AtomicUsize::new(0),
            peak: std::sync::atomic::AtomicUsize::new(0),
            status: std::sync::OnceLock::new(),
        })
    }

    /// Report each finished sweep in this node's `$nodes` row.
    pub fn report_to(&self, status: Arc<crate::node_status::NodeStatus>) {
        let _ = self.status.set(status);
    }

    /// Ask for a sweep now: called when this node swaps to a new version and
    /// once at boot. Returns at once; the work is a spawned task.
    pub fn nudge(self: &Arc<Self>) {
        self.resweep.store(true, Ordering::SeqCst);
        let me = self.clone();
        tokio::spawn(async move {
            me.tick().await;
        });
    }

    /// The program this node is serving, compiled from the registry's active
    /// artifact, or `None` when there is none, it is not the version this
    /// node serves, or it does not compile.
    async fn serving_program(&self) -> Option<Arc<Program>> {
        let serving = self.spaces.deployment().current().version.clone();
        if serving.is_empty() || serving == "(none)" {
            return None;
        }
        if let Some(p) = self
            .program
            .lock()
            .expect("program cache is not poisoned")
            .as_ref()
            .filter(|p| p.version == serving)
        {
            return Some(p.clone());
        }
        let artifact = sa::deployed_artifact(&self.control).await?;
        if artifact.version != serving {
            return None;
        }
        let program = Arc::new(Program::compile(&artifact.source).ok()?);
        if program.version != serving {
            return None;
        }
        *self.program.lock().expect("program cache is not poisoned") = Some(program.clone());
        Some(program)
    }

    /// Bring one space's database to the serving version, under the apply
    /// lock, then re-ask the resolver so the hold lifts.
    pub async fn forward_space(&self, space: &str) -> Caught {
        let Some(program) = self.serving_program().await else {
            return Caught::NoProgram;
        };
        let pool = match self.spaces.pool_of(space).await {
            Ok(p) => p,
            Err(e) => return Caught::Waiting(e.to_string()),
        };
        match ordo_exec::spacedb::recorded_version(&pool).await {
            Ok(TargetVersion::At(v)) if v == program.version => {
                self.spaces.reconcile(space).await;
                return Caught::Current;
            }
            Ok(TargetVersion::Unknown) => {
                self.spaces.reconcile(space).await;
                return Caught::Current;
            }
            Ok(_) => {}
            Err(e) => return Caught::Waiting(e.to_string()),
        }
        if let Some(why) = self.spaces.refused_at(space, &program.version) {
            return Caught::Refused(why);
        }
        let lock = match ApplyLock::take(&self.control).await {
            Ok(Some(l)) => l,
            Ok(None) => return Caught::Busy,
            Err(e) => return Caught::Waiting(e),
        };
        // An apply that landed while this waited for the lock changed what
        // is being served. What was compiled is no longer the target.
        if self.spaces.deployment().current().version != program.version {
            lock.release().await;
            return Caught::Busy;
        }
        let out = tokio::time::timeout(
            sa::FORWARD_TIMEOUT,
            sa::bring_forward_from(&pool, Some(&self.control), &program, ACTOR, OP),
        )
        .await;
        lock.release().await;
        match out {
            Ok(Ok(forward)) => {
                self.confirm_in_fleet(space, &program.version).await;
                self.spaces.reconcile(space).await;
                match forward {
                    Forward::Advanced { statements } => {
                        println!(
                            "tenant catch-up: space {space} brought to {} ({statements} \
                             statement(s)) and served",
                            program.version
                        );
                        Caught::Advanced
                    }
                    Forward::Current => Caught::Current,
                }
            }
            Ok(Err(r)) if r.transient => Caught::Waiting(r.to_string()),
            Ok(Err(r)) => {
                let why = r.to_string();
                eprintln!(
                    "tenant catch-up: space {space} stays closed: bringing it to {} was \
                     refused: {why}",
                    program.version
                );
                self.spaces.note_refused(space, &program.version, &why);
                Caught::Refused(why)
            }
            Err(_) => Caught::Waiting(format!(
                "no answer within {}s",
                sa::FORWARD_TIMEOUT.as_secs()
            )),
        }
    }

    /// The fleet view is a cache of what each database says (R-S2), so it is
    /// written from the database and never from this program's version, and
    /// a failure to write it is a log line.
    async fn confirm_in_fleet(&self, space: &str, _version: &str) {
        let id = ordo_crypto::SpaceId(space.to_string());
        let Ok(Some(attachment)) = ordo_exec::spacedb::lookup(&self.control, &id).await else {
            return;
        };
        let label =
            ordo_exec::route::target_label(&attachment.host, attachment.port, &attachment.db);
        let Ok(pool) = self.spaces.pool_of(space).await else {
            return;
        };
        let Ok(TargetVersion::At(v)) = ordo_exec::spacedb::recorded_version(&pool).await else {
            return;
        };
        if ordo_exec::fleet::ensure_table(&self.control).await.is_ok() {
            let at = crate::runtime::now_secs() as i64;
            let _ = ordo_exec::fleet::confirm(&self.control, &label, &v, at).await;
        }
    }

    /// The most databases that were being brought forward at once, since
    /// this catch-up was made.
    pub fn peak_concurrent(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    /// Ask one database what it records and bring it forward if it is
    /// behind. `Ok(None)` means it is already current.
    async fn check_target(
        &self,
        target: ordo_exec::route::Target,
        program: &Program,
    ) -> Result<Option<ordo_exec::route::Target>, (String, String)> {
        let pool = match &target.pool {
            Ok(p) => p.clone(),
            Err(why) => return Err((target.label.clone(), format!("unreachable: {why}"))),
        };
        match ordo_exec::spacedb::recorded_version(&pool).await {
            Ok(TargetVersion::At(v)) if v != program.version => Ok(Some(target)),
            Ok(_) => Ok(None),
            Err(e) => Err((target.label.clone(), format!("unreachable: {e}"))),
        }
    }

    /// One pass over every database the resolver lists, bringing each that
    /// records another version to the serving one.
    ///
    /// Several databases move at once, as many as the tenant dial cap
    /// ([`ordo_exec::route::PoolResolver::walk_concurrency`]) allows, in
    /// batches of that size. Each batch takes the apply lock once and holds
    /// it for the batch only, so an apply or an attach in between waits for
    /// one batch and not for the whole fleet. Each database is bounded, and
    /// one that fails is held and reported while the rest go on.
    pub async fn sweep(&self) -> Swept {
        let _one = self.sweeping.lock().await;
        let started = std::time::Instant::now();
        let mut out = Swept::default();
        let Some(program) = self.serving_program().await else {
            return out;
        };
        let targets = match self.spaces.pools().targets().await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("tenant catch-up: the databases could not be listed ({e})");
                out.transient = true;
                return out;
            }
        };
        let cap = self.spaces.pools().walk_concurrency().max(1);
        let targets: Vec<_> = targets
            .into_iter()
            .filter(|t| t.label != ordo_exec::route::DEFAULT_LABEL)
            .collect();
        let mut rest = targets.into_iter().peekable();
        while rest.peek().is_some() {
            let batch: Vec<_> = rest.by_ref().take(cap).collect();
            let checks = futures::future::join_all(
                batch.into_iter().map(|t| self.check_target(t, &program)),
            )
            .await;
            let mut behind = Vec::new();
            for c in checks {
                match c {
                    Ok(Some(t)) => behind.push(t),
                    Ok(None) => out.current += 1,
                    Err(held) => {
                        out.transient = true;
                        out.held.push(held);
                    }
                }
            }
            if behind.is_empty() {
                continue;
            }
            let lock = match ApplyLock::take(&self.control).await {
                Ok(Some(l)) => l,
                Ok(None) => {
                    out.transient = true;
                    for t in behind {
                        out.held
                            .push((t.label, "an apply holds the deployment".into()));
                    }
                    continue;
                }
                Err(e) => {
                    out.transient = true;
                    for t in behind {
                        out.held.push((t.label, e.clone()));
                    }
                    continue;
                }
            };
            if self.spaces.deployment().current().version != program.version {
                // The version moved: the swap that moved it asks for its own
                // sweep.
                lock.release().await;
                out.transient = true;
                return out;
            }
            let done = futures::future::join_all(behind.into_iter().map(|t| async {
                let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(now, Ordering::SeqCst);
                let d = sa::forward_target(t, &self.control, &program, ACTOR, OP).await;
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                d
            }))
            .await;
            lock.release().await;
            for d in done {
                match d.outcome {
                    Ok(Forward::Advanced { .. }) => out.advanced += 1,
                    Ok(Forward::Current) => out.current += 1,
                    Err(r) => {
                        out.transient |= r.transient;
                        out.held.push((d.label, r.to_string()));
                    }
                }
            }
        }
        // Every hold this node keeps for a database that is now current is
        // asked again, which lifts it.
        for space in self.spaces.awaiting_schema() {
            self.spaces.reconcile(&space).await;
        }
        if out.advanced > 0 || !out.held.is_empty() {
            println!(
                "tenant catch-up: {} database(s) brought to {}, {} already there, {} held",
                out.advanced,
                program.version,
                out.current,
                out.held.len()
            );
        }
        for (target, why) in &out.held {
            eprintln!("tenant catch-up: {target} not brought forward: {why}");
        }
        // Said in `$nodes`, which is what `ordo apply` waits on. Only when
        // the version swept is still the one served: a sweep for a version
        // this node has moved past says nothing about the new one.
        if let Some(status) = self.status.get() {
            if self.spaces.deployment().current().version == program.version {
                status
                    .swept(
                        &program.version,
                        out.held.clone(),
                        out.transient,
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
            }
        }
        out
    }

    /// One tick: a sweep when one is due, then each space this node holds
    /// closed for a database that is behind. Returns how many it moved.
    pub async fn tick(&self) -> u64 {
        let mut moved = 0;
        if self.resweep.swap(false, Ordering::SeqCst) {
            let swept = self.sweep().await;
            moved += swept.advanced as u64;
            if swept.transient {
                self.resweep.store(true, Ordering::SeqCst);
            }
        }
        for space in self.spaces.awaiting_schema() {
            if self.forward_space(&space).await == Caught::Advanced {
                moved += 1;
            }
        }
        moved
    }
}

/// The catch-up as a supervised worker.
pub struct TenantCatchUp {
    pub catchup: Arc<CatchUp>,
    /// `ORDO_TENANT_CATCHUP_SECS`, default 15.
    pub every: Duration,
}

impl TenantCatchUp {
    pub const DEFAULT_SECS: u64 = 15;

    pub fn period_from_env() -> Duration {
        let secs = std::env::var("ORDO_TENANT_CATCHUP_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|s| *s > 0)
            .unwrap_or(Self::DEFAULT_SECS);
        Duration::from_secs(secs)
    }
}

impl Worker for TenantCatchUp {
    fn name(&self) -> &'static str {
        "tenant.catchup"
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn run<'a>(&'a self, _ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let moved = self.catchup.tick().await;
            Ok(if moved == 0 {
                Progress::idle()
            } else {
                Progress::did(moved)
            })
        })
    }
}
