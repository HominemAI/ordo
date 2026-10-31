//! **Routines, switched on** (checklist 12a; board task `triggers`).
//!
//! The trigger loop has been built and tested since R3 and booted disabled,
//! because nothing supplied it listeners, clock routines or a body to run.
//! This module supplies all three from the program:
//!
//! - [`wiring`] turns the live program's `routine` declarations into the
//!   loop's values: a [`Listener`] per change routine, a [`ClockRoutine`]
//!   per scheduled one, and a [`RoutineRunner`] when there is any. A
//!   program that declares none gets the empty wiring, and every trigger
//!   worker stays `DISABLED` with the census line it always had.
//! - [`Tenants`] is the set a routine runs in: the space this process
//!   booted with and every tenant the registry holds active or paused. A
//!   schedule fires once for the deployment and enqueues one run per
//!   tenant; a change is drained from each tenant's own chain.
//! - [`RoutineRunner`] is the `RunHandler`: it runs the body through the
//!   write protocol in the run's tenant, as `routine:<name>@<version>`.
//!
//! # A paused tenant's runs are skipped, and the skip is written down
//!
//! `tenant-pause` ask 5, ruled: *"Skip and record."* Audit 2026-10-05d
//! finding 4 put the gate here, at the handler seam, so a run claimed
//! before a pause and executed during it is caught too. A routine whose
//! `run` header says `nopause` runs anyway: the app developer's choice,
//! written where the routine is, default not run. A skip is a row in
//! `$routineskips` on the control database and a line in the log, and the
//! run is finished rather than retried, because waiting does not lift a
//! pause.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use ordo_commandlog::SpaceId as LogSpace;
use ordo_exec::triggers::{self, ClockRoutine, Listener, Run};
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_syntax::{RoutineDecl, RoutineTrigger};

use crate::spaces::Spaces;
use crate::supervisor::{BoxFut, RunHandler, WorkerCtx};

/// The skip record. One row per run that was not executed, keyed like the
/// run, so a redelivered run records once.
pub const SKIPS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$routineskips\" ( \
  \"space\" text NOT NULL, \
  \"run_key\" text NOT NULL, \
  \"routine\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"at\" bigint NOT NULL, \
  PRIMARY KEY (\"space\", \"run_key\") \
)";

/// Why a run was not executed.
pub const SKIP_PAUSED: &str = "paused";
/// The live version no longer declares the routine the run was for.
pub const SKIP_UNDECLARED: &str = "undeclared";

/// One recorded skip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub space: String,
    pub run_key: String,
    pub routine: String,
    pub reason: String,
    pub at: u64,
}

pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError> {
    pool.rows(SKIPS_DDL).await.map(|_| ())
}

/// Every recorded skip in `space`, oldest first.
pub async fn skips(pool: &Pool, space: &str) -> Result<Vec<Skip>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"space\", \"run_key\", \"routine\", \"reason\", \"at\" \
             FROM \"$routineskips\" WHERE \"space\" = {} ORDER BY \"at\", \"run_key\"",
            sql_lit(space)
        ))
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let cell = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
            Skip {
                space: cell(0),
                run_key: cell(1),
                routine: cell(2),
                reason: cell(3),
                at: cell(4).parse().unwrap_or(0),
            }
        })
        .collect())
}

async fn record_skip(
    pool: &Pool,
    run: &Run,
    routine: &str,
    reason: &str,
    at: u64,
) -> Result<(), PoolError> {
    pool.rows(&format!(
        "INSERT INTO \"$routineskips\" (\"space\", \"run_key\", \"routine\", \"reason\", \"at\") \
         VALUES ({}, {}, {}, {}, {at}) ON CONFLICT (\"space\", \"run_key\") DO NOTHING",
        sql_lit(&run.space.0),
        sql_lit(&run.run_key),
        sql_lit(routine),
        sql_lit(reason),
    ))
    .await
    .map(|_| ())
}

// ── the tenants a routine runs in ────────────────────────────────────────

/// The spaces a routine runs in, read fresh on every pass so a tenant
/// created after boot is reached without a restart.
pub struct Tenants {
    pub spaces: Arc<Spaces>,
    /// `ORDO_SPACE`: the tenant this process booted with. Always included,
    /// because a single-tenant deployment has no registry row for it.
    pub boot: String,
    /// The program version whose routines this process serves.
    pub version: String,
    /// Spaces whose listener cursors this process has registered.
    registered: Mutex<HashSet<String>>,
    /// When this value was made, in seconds: the fallback for when the
    /// version was activated, for a database with no registry.
    made_at: u64,
    /// When `version` was activated, read once.
    since: Mutex<Option<u64>>,
    /// The longest gap, in seconds, between two drain visits to a tenant
    /// whose chain has not moved. [`IDLE_MAX_ENV`].
    idle_max: u64,
    /// How the drain paces each tenant.
    pace: Mutex<HashMap<String, Pace>>,
    /// Per tenant, the cursors a routine left under an earlier trigger,
    /// waiting for the new cursor to pass them.
    stale: Mutex<HashMap<String, Vec<Stale>>>,
}

/// The setting for [`Tenants`]' idle ceiling: the longest gap, in seconds,
/// between two drain visits to a tenant whose chain has not moved.
pub const IDLE_MAX_ENV: &str = "ORDO_TRIGGERS_IDLE_MAX_SECS";
/// Its default.
pub const DEFAULT_IDLE_MAX_SECS: u64 = 10;

/// [`IDLE_MAX_ENV`], or the default when it is unset, zero or not a number.
pub fn idle_max_from_env() -> u64 {
    std::env::var(IDLE_MAX_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(DEFAULT_IDLE_MAX_SECS)
}

/// The op name on the record of a cursor the drain retired.
pub const RETIRE_OP: &str = "ordo.triggers.drain";

/// How the drain paces one tenant (audit 2026-10-08 finding 7).
///
/// A visit first reads the head of the tenant's chain, one indexed row.
/// When every listener had caught up at the last visit and the head is the
/// same, nothing is drained and the gap to the next visit doubles, up to
/// the ceiling. When the head has moved, the listeners are drained and the
/// tenant is visited every second again.
#[derive(Debug, Clone, Default)]
struct Pace {
    head: Option<ordo_commandlog::ChainHash>,
    caught_up: bool,
    interval: u64,
    next_at: u64,
}

/// A routine's cursor under its previous trigger, and the listener that now
/// serves the routine.
#[derive(Debug, Clone)]
struct Stale {
    old: String,
    new: String,
    actor: String,
}

/// What one drain pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrainPass {
    /// The tenants visited, in order. A tenant whose next visit is not due
    /// is left out.
    pub visited: Vec<String>,
    /// Log entries read.
    pub units: u64,
    /// A listener stopped at the limit, with more to read.
    pub more: bool,
    /// `(tenant, cursor)` for each cursor retired.
    pub retired: Vec<(String, String)>,
    /// Tenants whose database could not be reached, with why.
    pub misses: Vec<String>,
}

/// One tenant a routine runs in, with when it was created. `None` is the
/// boot space with no registry row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant {
    pub id: String,
    pub created_at: Option<u64>,
}

/// Where a cursor first registered by the drain starts (audit 2026-10-08
/// finding 8). At the head, so a tenant that was unreachable at boot does
/// not replay its history. At 0 only for a tenant created after the version
/// was activated, since all of its chain was written while the routine
/// existed. The boot space with no registry row starts at the head.
pub fn drain_starts_at_head(created_at: Option<u64>, activated_at: u64) -> bool {
    match created_at {
        None => true,
        Some(created) => created <= activated_at,
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Tenants {
    pub fn new(spaces: Arc<Spaces>, boot: String, version: String) -> Tenants {
        Tenants {
            spaces,
            boot,
            version,
            registered: Mutex::new(HashSet::new()),
            made_at: now_secs(),
            since: Mutex::new(None),
            idle_max: DEFAULT_IDLE_MAX_SECS,
            pace: Mutex::new(HashMap::new()),
            stale: Mutex::new(HashMap::new()),
        }
    }

    /// Set the idle ceiling, in seconds. Zero is taken as 1.
    pub fn with_idle_max(mut self, secs: u64) -> Tenants {
        self.idle_max = secs.max(1);
        self
    }

    /// One drain pass over every tenant whose visit is due (audit
    /// 2026-10-08 finding 7).
    ///
    /// Each due tenant is registered on first reach, its head is read, and
    /// its listeners are drained only when the head moved or a listener had
    /// not caught up. When `live_version` is the version this value serves,
    /// a routine's cursor under an earlier trigger is retired once the
    /// routine's new cursor has passed it. A replica still on the earlier
    /// version retires nothing, because its listeners are the old ones.
    pub async fn drain_pass(
        &self,
        control: &Pool,
        listeners: &[Listener],
        live_version: &str,
        limit: usize,
        now: u64,
    ) -> Result<DrainPass, PoolError> {
        let mut pass = DrainPass::default();
        for tenant in self.list_created(control).await {
            let space = tenant.id.clone();
            if !self.due(&space, now) {
                continue;
            }
            pass.visited.push(space.clone());
            // Each tenant's chain is in its own database; the batches and
            // runs are on the control database, where they are claimed.
            // One unreachable tenant does not stop the rest.
            let pool = match self.spaces.pool_of(&space).await {
                Ok(p) => p,
                Err(e) => {
                    pass.misses.push(format!("{space}: {e}"));
                    continue;
                }
            };
            self.register_on_reach(control, &pool, &tenant, listeners, now)
                .await?;
            let id = LogSpace(space.clone());
            let store = ordo_exec::commandlog::PgStore::new(
                pool.clone(),
                tokio::runtime::Handle::current(),
            );
            let head = store.try_head(&id).await?;
            if self.unmoved(&space, head) {
                self.back_off(&space, now);
                continue;
            }
            let mut more = false;
            for l in listeners {
                let read =
                    triggers::drain_listener_to(&pool, &store, control, &id, l, limit, now).await?;
                pass.units += read as u64;
                more |= read == limit;
            }
            pass.more |= more;
            if live_version == self.version {
                for old in self.retire_caught_up(&pool, &id, now).await? {
                    pass.retired.push((space.clone(), old));
                }
            }
            self.moved(&space, head, !more, now);
        }
        Ok(pass)
    }

    fn due(&self, space: &str, now: u64) -> bool {
        self.pace
            .lock()
            .expect("pace is not poisoned")
            .get(space)
            .is_none_or(|p| p.next_at <= now)
    }

    fn unmoved(&self, space: &str, head: Option<ordo_commandlog::ChainHash>) -> bool {
        self.pace
            .lock()
            .expect("pace is not poisoned")
            .get(space)
            .is_some_and(|p| p.caught_up && p.head == head)
    }

    fn back_off(&self, space: &str, now: u64) {
        let mut all = self.pace.lock().expect("pace is not poisoned");
        let p = all.entry(space.to_string()).or_default();
        p.interval = (p.interval.max(1) * 2).min(self.idle_max);
        p.next_at = now + p.interval;
    }

    fn moved(
        &self,
        space: &str,
        head: Option<ordo_commandlog::ChainHash>,
        caught_up: bool,
        now: u64,
    ) {
        let mut all = self.pace.lock().expect("pace is not poisoned");
        let p = all.entry(space.to_string()).or_default();
        p.head = head;
        p.caught_up = caught_up;
        p.interval = 1;
        // A listener with more to read is visited again on the next pass,
        // which the supervisor runs at once.
        p.next_at = if caught_up { now + 1 } else { now };
    }

    /// Retire each of `space`'s stale cursors that the routine's new cursor
    /// has reached, through the one deregister path, and return their names.
    /// A stale cursor ahead of the new one stays: the events between them
    /// have not been handed to the new cursor yet.
    async fn retire_caught_up(
        &self,
        pool: &Pool,
        id: &LogSpace,
        now: u64,
    ) -> Result<Vec<String>, PoolError> {
        let pending = self
            .stale
            .lock()
            .expect("stale is not poisoned")
            .get(&id.0)
            .cloned()
            .unwrap_or_default();
        if pending.is_empty() {
            return Ok(Vec::new());
        }
        let held = ordo_exec::prune::consumers(pool, id).await?;
        let at = |name: &str| held.iter().find(|c| c.name == name).map(|c| c.position);
        let mut keep = Vec::new();
        let mut retired = Vec::new();
        for s in pending {
            match (at(&s.old), at(&s.new)) {
                (None, _) => {}
                (Some(old), Some(new)) if new >= old => {
                    triggers::retire_listener(pool, id, &s.old, &s.actor, RETIRE_OP, false, now)
                        .await
                        .map_err(PoolError::db)?;
                    retired.push(s.old);
                }
                _ => keep.push(s),
            }
        }
        self.stale
            .lock()
            .expect("stale is not poisoned")
            .insert(id.0.clone(), keep);
        Ok(retired)
    }

    /// The boot space, then every registry tenant that is `active` or
    /// `paused`, never the system space. A paused tenant is listed: its
    /// runs reach the handler, which skips and records them.
    pub async fn list(&self, control: &Pool) -> Vec<String> {
        self.list_created(control)
            .await
            .into_iter()
            .map(|t| t.id)
            .collect()
    }

    /// [`Tenants::list`], with each tenant's creation time.
    pub async fn list_created(&self, control: &Pool) -> Vec<Tenant> {
        let mut out = vec![Tenant {
            id: self.boot.clone(),
            created_at: None,
        }];
        // No registry table is a deployment that has created no tenant
        // through the platform, which is the single-tenant case.
        if let Ok(all) = crate::tenants::all(control).await {
            for t in all {
                if !matches!(
                    t.state,
                    crate::tenants::State::Active | crate::tenants::State::Paused
                ) {
                    continue;
                }
                match out.iter_mut().find(|o| o.id == t.id) {
                    Some(o) => o.created_at = Some(t.created_at),
                    None => out.push(Tenant {
                        id: t.id,
                        created_at: Some(t.created_at),
                    }),
                }
            }
        }
        out.retain(|s| s.id != crate::provision::SYSTEM_SPACE);
        out
    }

    /// When the served version was activated, in seconds. Read from the
    /// registry once; with no registry row, the time this value was made.
    pub async fn activated_at(&self, control: &Pool) -> u64 {
        if let Some(at) = *self.since.lock().expect("since is not poisoned") {
            return at;
        }
        let at = match ordo_exec::registry::activated_at(control, &self.version).await {
            Ok(Some(at)) => at,
            _ => self.made_at,
        };
        *self.since.lock().expect("since is not poisoned") = Some(at);
        at
    }

    /// Has this process registered `space`'s cursors?
    pub fn is_registered(&self, space: &str) -> bool {
        self.registered
            .lock()
            .expect("registered set is not poisoned")
            .contains(space)
    }

    /// Register `space`'s cursors on the drain's first reach of it, where
    /// [`drain_starts_at_head`] says (audit 2026-10-08 finding 8). Boot
    /// registers every tenant it reaches at the head; a tenant first
    /// reached here may be one that was unreachable at boot, with history
    /// the routine must not replay.
    pub async fn register_on_reach(
        &self,
        control: &Pool,
        pool: &Pool,
        tenant: &Tenant,
        listeners: &[Listener],
        now: u64,
    ) -> Result<(), PoolError> {
        if self.is_registered(&tenant.id) {
            return Ok(());
        }
        let since = self.activated_at(control).await;
        let at_head = drain_starts_at_head(tenant.created_at, since);
        self.register(pool, &tenant.id, listeners, at_head, now)
            .await
    }

    /// Register `listeners`' cursors in `space`'s database, once per
    /// process. `at_head` places a new cursor at the end of the chain, so a
    /// routine deployed into a tenant with history reacts to changes from
    /// now on rather than replaying the past. Without it the cursor starts
    /// at 0, which is right only for a tenant created after the version was
    /// activated ([`drain_starts_at_head`]).
    pub async fn register(
        &self,
        pool: &Pool,
        space: &str,
        listeners: &[Listener],
        at_head: bool,
        now: u64,
    ) -> Result<(), PoolError> {
        if self.is_registered(space) {
            return Ok(());
        }
        ordo_exec::prune::ensure_tables(pool).await?;
        let id = LogSpace(space.to_string());
        let position = if at_head {
            ordo_exec::commandlog::PgStore::new(pool.clone(), tokio::runtime::Handle::current())
                .try_len(&id)
                .await? as u64
        } else {
            0
        };
        // A routine whose trigger changed has a cursor under its earlier
        // key, `<routine>:<old pattern>`. The new cursor starts no later
        // than the old one, so the events the old one had not read are
        // handed to the new one, and the old one is retired once the new
        // one passes it (audit 2026-10-08 finding 7).
        let held = ordo_exec::prune::consumers(pool, &id).await?;
        let mut stale = Vec::new();
        for l in listeners {
            let key = l.key();
            let prefix = format!("{}:", l.routine);
            let olds: Vec<&ordo_exec::prune::Consumer> = held
                .iter()
                .filter(|c| {
                    c.kind == ordo_exec::prune::ConsumerKind::Listener
                        && c.name.starts_with(&prefix)
                        && c.name != key
                })
                .collect();
            let start = olds
                .iter()
                .map(|c| c.position)
                .min()
                .map_or(position, |p| p.min(position));
            triggers::register_listener(pool, &id, l, start, now).await?;
            for c in olds {
                stale.push(Stale {
                    old: c.name.clone(),
                    new: key.clone(),
                    actor: format!("routine:{}@{}", l.routine, l.version),
                });
            }
        }
        if !stale.is_empty() {
            self.stale
                .lock()
                .expect("stale is not poisoned")
                .insert(space.to_string(), stale);
        }
        self.registered
            .lock()
            .expect("registered set is not poisoned")
            .insert(space.to_string());
        Ok(())
    }
}

// ── from the program to the loop ─────────────────────────────────────────

/// The listener key and the clock key a routine's runs carry, so the
/// handler can find the routine a run is for.
fn run_keys(r: &RoutineDecl, domain: &str, version: &str) -> String {
    match &r.trigger {
        RoutineTrigger::Clock(schedule) => ClockRoutine {
            routine: r.name.name.clone(),
            version: version.to_string(),
            schedule: *schedule,
        }
        .key(),
        RoutineTrigger::Change { pattern, modifiers } => Listener {
            domain: domain.to_string(),
            pattern: pattern.clone(),
            routine: r.name.name.clone(),
            version: version.to_string(),
            modifiers: *modifiers,
        }
        .key(),
    }
}

/// The trigger wiring the live program declares.
///
/// Empty for a program with no routines, so the census keeps saying the
/// trigger workers are off and why.
pub fn wiring(
    schema: &ordo_schema::Schema,
    version: &str,
    spaces: Arc<Spaces>,
    boot: &str,
    mail: bool,
) -> crate::runtime::TriggerWiring {
    if schema.routines.is_empty() {
        return crate::runtime::TriggerWiring::default();
    }
    let domain = schema.domain.clone().unwrap_or_default();
    let mut listeners = Vec::new();
    let mut clock = Vec::new();
    for r in &schema.routines {
        match &r.trigger {
            RoutineTrigger::Clock(schedule) => clock.push(ClockRoutine {
                routine: r.name.name.clone(),
                version: version.to_string(),
                schedule: *schedule,
            }),
            RoutineTrigger::Change { pattern, modifiers } => listeners.push(Listener {
                domain: domain.clone(),
                pattern: pattern.clone(),
                routine: r.name.name.clone(),
                version: version.to_string(),
                modifiers: *modifiers,
            }),
        }
    }
    let tenants = Arc::new(
        Tenants::new(spaces.clone(), boot.to_string(), version.to_string())
            .with_idle_max(idle_max_from_env()),
    );
    crate::runtime::TriggerWiring {
        listeners,
        clock,
        handler: Some(Arc::new(RoutineRunner {
            spaces,
            each_max: ordo_plan::each::each_max_rows_from_env(),
            mail,
        })),
        tenants: Some(tenants),
    }
}

/// The routines this node runs **now**, switched with the program.
///
/// The `triggers.*` workers read it on every pass. When the door swaps to
/// a new version, [`LiveRoutines::switch`] builds that version's wiring and
/// puts it in place, so a routine added, retimed or removed by `ordo apply`
/// takes effect with no restart (notes v2 gap 26, 2026-10-22). Listener
/// cursors are registered as each tenant is next reached
/// ([`Tenants::register_on_reach`]), the way a tenant created after boot
/// gets them.
///
/// A wiring a test set by hand ([`crate::runtime::Runtime::with_triggers`])
/// does not follow the program: the switch leaves it alone.
pub struct LiveRoutines {
    current: std::sync::RwLock<Arc<crate::runtime::TriggerWiring>>,
    follows_program: bool,
    spaces: Arc<Spaces>,
    boot: String,
    mail: bool,
}

impl LiveRoutines {
    /// Routines that follow the program, starting from `boot_wiring`.
    pub fn following(
        boot_wiring: crate::runtime::TriggerWiring,
        spaces: Arc<Spaces>,
        boot: String,
        mail: bool,
    ) -> LiveRoutines {
        LiveRoutines {
            current: std::sync::RwLock::new(Arc::new(boot_wiring)),
            follows_program: true,
            spaces,
            boot,
            mail,
        }
    }

    /// A fixed wiring that no swap changes.
    pub fn fixed(wiring: crate::runtime::TriggerWiring, spaces: Arc<Spaces>) -> LiveRoutines {
        LiveRoutines {
            current: std::sync::RwLock::new(Arc::new(wiring)),
            follows_program: false,
            spaces,
            boot: String::new(),
            mail: false,
        }
    }

    pub fn follows_program(&self) -> bool {
        self.follows_program
    }

    /// The wiring in force now.
    pub fn current(&self) -> Arc<crate::runtime::TriggerWiring> {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Put `version`'s routines in place. Its clock schedules are written
    /// before the workers see them, so the clock never reads a routine with
    /// no schedule. Returns the line the log prints.
    pub async fn switch(
        &self,
        control: &Pool,
        schema: &ordo_schema::Schema,
        version: &str,
    ) -> Result<String, PoolError> {
        if !self.follows_program {
            return Ok("routines: fixed by the embedding; not switched".into());
        }
        let next = wiring(schema, version, self.spaces.clone(), &self.boot, self.mail);
        if next.handler.is_some() {
            ensure_tables(control).await?;
        }
        if !next.clock.is_empty() {
            triggers::register_clock(control, &next.clock, now_secs()).await?;
        }
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(next);
        Ok(format!(
            "routines: {} (version {version})",
            describe(schema)
        ))
    }
}

/// One line for `ordo check` and the startup log.
pub fn describe(schema: &ordo_schema::Schema) -> String {
    if schema.routines.is_empty() {
        return "none declared".into();
    }
    let each: Vec<String> = schema.routines.iter().map(describe_one).collect();
    format!("{} declared: {}", each.len(), each.join("; "))
}

/// `sweep (every 15m)`, `stamp (when notes.tasks.created, bounded)`,
/// `expire (every day 08:00Z, each row of leases, at most 500 a run)`.
pub fn describe_one(r: &RoutineDecl) -> String {
    let mut notes: Vec<String> = Vec::new();
    match &r.trigger {
        RoutineTrigger::Clock(s) => notes.push(s.render()),
        RoutineTrigger::Change { pattern, modifiers } => {
            let m = modifiers.render();
            notes.push(if m.is_empty() {
                format!("when {}", pattern.render())
            } else {
                format!("when {} {m}", pattern.render())
            });
            if !r.flags.unguarded {
                notes.push("bounded to the changed rows".into());
            }
        }
    }
    if let Some(pl) = &r.each {
        let sources: Vec<&str> = pl.sources.iter().map(|s| s.name.name.as_str()).collect();
        notes.push(format!(
            "each row of {}, at most {} a run",
            sources.join(" "),
            ordo_plan::each::each_max_rows_from_env()
        ));
    }
    if r.flags.unguarded {
        notes.push("unguarded".into());
    }
    if r.flags.nopause {
        notes.push("runs while paused".into());
    }
    if let Some(n) = &r.notify {
        notes.push(format!("emails the holders of `{}`", n.level.name));
    }
    format!("{} ({})", r.name.name, notes.join(", "))
}

// ── telling people: `notify` ─────────────────────────────────────────────

/// The outbox kind a routine's notice is queued under.
pub const NOTICE_KIND: &str = "routine.notice";

/// The line under every notice. The subject is the program's text. Nothing
/// from a row is in the mail, so the detail stays in the app.
pub const NOTICE_FOOTER: &str = "Open the app to see it.";

/// Writes a routine's notice at send time. The subject is the `notify`
/// text, which the payload carries.
pub struct RoutineNotice;

impl crate::mail::Composer for RoutineNotice {
    fn compose(
        &self,
        job: &crate::mail::QueuedMail,
        _now: u64,
    ) -> Result<Option<crate::mail::Message>, crate::mail::MailError> {
        let text = job.payload.trim();
        crate::mail::Message::new(
            job.to.clone(),
            text,
            &format!("{text}\n\n{NOTICE_FOOTER}\n"),
        )
        .map(Some)
    }
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn id_list(ids: &[String]) -> String {
    ids.iter()
        .map(|i| sql_lit(i))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The principals who hold `level` on the rows of `entity` with these ids.
///
/// Each row is followed to the row its acl lives on, then to the access
/// rows that name an active member with that level. It reads inside the
/// run's own transaction, so it sees what the run wrote.
fn holders(
    db: &dyn ordo_conformance::write::WriteDb,
    schema: &ordo_schema::Schema,
    entity: &str,
    ids: &[String],
    level: &str,
) -> Result<Vec<String>, String> {
    let Some(home) = ordo_schema::routine::acl_home(schema, entity) else {
        return Ok(Vec::new());
    };
    let mut ids: Vec<String> = ids.to_vec();
    let mut at = entity.to_string();
    for field in &home.path {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = db.rows(&format!(
            "SELECT DISTINCT {} FROM {} WHERE \"id\" IN ({})",
            quoted(field),
            quoted(&at),
            id_list(&ids)
        ))?;
        ids = rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect();
        at = schema
            .entity(&at)
            .and_then(|e| e.field(field))
            .and_then(|f| match &f.ty {
                ordo_schema::FieldTy::Ref { target, .. } => Some(target.clone()),
                _ => None,
            })
            .ok_or_else(|| format!("`{at}.{field}` is not a ref"))?;
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = db.rows(&format!(
        "SELECT DISTINCT m.\"principal\" FROM {} a JOIN \"members\" m ON m.\"id\" = a.\"member\" \
         WHERE a.{} IN ({}) AND a.\"level\" = {} AND m.\"status\" = 'active'",
        quoted(&home.link),
        quoted(&home.back),
        id_list(&ids),
        sql_lit(level)
    ))?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect())
}

/// Queue one notice for each principal Ordo knows a verified address of.
///
/// The address is the one the person added and confirmed
/// ([`crate::contact_email`]), else the email sign-in linked first. A
/// principal with neither, or whose account was deleted, is told in the app
/// only. Answers how many were queued.
pub async fn queue_notices(
    control: &Pool,
    principals: &[String],
    text: &str,
    now: u64,
) -> Result<usize, String> {
    if principals.is_empty() {
        return Ok(0);
    }
    let ids = id_list(principals);
    let rows = control
        .rows(&format!(
            "SELECT DISTINCT ON (x.\"principal\") 'email:' || x.\"address\" FROM ( \
               SELECT c.\"principal\", c.\"address\", 0 AS \"k\", 0::bigint AS \"t\" \
               FROM \"$contactemail\" c WHERE c.\"principal\" IN ({ids}) \
               UNION ALL \
               SELECT a.\"principal\", substr(a.\"authenticator\", 7), 1, a.\"linked_at\" \
               FROM \"$authenticator\" a WHERE a.\"principal\" IN ({ids}) \
                 AND a.\"authenticator\" LIKE 'email:%' AND a.\"retired_case\" = '' \
             ) x JOIN \"$principal\" p ON p.\"id\" = x.\"principal\" \
             WHERE p.\"deleted_at\" = 0 \
             ORDER BY x.\"principal\", x.\"k\", x.\"t\""
        ))
        .await
        .map_err(|e| format!("reading who has a verified address: {e}"))?;
    let mut queued = 0;
    for row in rows {
        let Some(Some(auth)) = row.into_iter().next() else {
            continue;
        };
        let Some(address) = auth.strip_prefix("email:") else {
            continue;
        };
        let Ok(to) = crate::mail::MailAddress::parse(address) else {
            continue;
        };
        let job = crate::mail::MailJob::new(NOTICE_KIND, to, text).map_err(|e| e.to_string())?;
        crate::mail::enqueue(control, &job, now)
            .await
            .map_err(|e| format!("queueing a notice: {e}"))?;
        queued += 1;
    }
    Ok(queued)
}

// ── the run handler ──────────────────────────────────────────────────────

/// What a routine's run comes to when nothing needed changing. The write
/// protocol builds no group for a statement that matched no rows, and the
/// executor refuses to commit a write without one, so the attempt is rolled
/// back (it wrote nothing) and the run is finished.
const NOTHING_CHANGED: &str = "routine_nothing_changed";

/// Runs a routine's body in the run's tenant.
pub struct RoutineRunner {
    pub spaces: Arc<Spaces>,
    /// The most rows an `each` body handles in one run: the deployment
    /// setting `ORDO_ROUTINE_EACH_MAX_ROWS`, 500 by default
    /// (`ordo_plan::each`).
    pub each_max: usize,
    /// Whether the deployment can send mail (`ORDO_MAIL_KIND` is set). A
    /// routine that says `notify` queues nothing without it, and says so.
    pub mail: bool,
}

impl RoutineRunner {
    /// Queue a committed run's notices. A failure here is logged and not
    /// retried: the run's writes stand, and a second run would tell twice.
    async fn tell(
        &self,
        control: &Pool,
        name: &str,
        space: &str,
        who: &[String],
        text: &str,
        now: u64,
    ) {
        if !self.mail {
            eprintln!(
                "routine {name}: {} people to tell in tenant {space}, and the deployment sends \
                 no mail. Set {} to send it",
                who.len(),
                crate::mail::KIND_ENV
            );
            return;
        }
        match queue_notices(control, who, text, now).await {
            Ok(k) => eprintln!(
                "routine {name}: queued {k} notices for {} people in tenant {space}",
                who.len()
            ),
            Err(e) => eprintln!("routine {name}: in tenant {space}, no notice was queued: {e}"),
        }
    }
}

impl RunHandler for RoutineRunner {
    fn handle<'a>(&'a self, run: &'a Run, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), String>> {
        Box::pin(async move {
            let live = ctx.deployment.current();
            let schema = &live.fixture.schema;
            let domain = schema.domain.clone().unwrap_or_default();
            let found = schema
                .routines
                .iter()
                .find(|r| run_keys(r, &domain, &live.version) == run.listener)
                .cloned();
            let now = ctx.now();
            let Some(routine) = found else {
                eprintln!(
                    "routine run {} in {}: the live version declares no routine for `{}`; \
                     recorded and not run",
                    run.run_key, run.space.0, run.listener
                );
                record_skip(&ctx.pool, run, &run.listener, SKIP_UNDECLARED, now)
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok(());
            };
            let name = routine.name.name.clone();
            if self.spaces.paused_record(&run.space.0).is_some() && !routine.flags.nopause {
                eprintln!(
                    "routine {name}: skipped in tenant {}: the tenant is paused (run {})",
                    run.space.0, run.run_key
                );
                record_skip(&ctx.pool, run, &name, SKIP_PAUSED, now)
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok(());
            }
            let bound = self
                .spaces
                .resolve(&run.space.0)
                .await
                .map_err(|e| format!("tenant {}: {e}", run.space.0))?;
            let fixture = bound.view.fixture.clone();
            let actor = format!("{name}@{}", live.version);
            let woken = match &routine.trigger {
                RoutineTrigger::Change { pattern, .. } if !routine.flags.unguarded => {
                    match &pattern.entity {
                        ordo_syntax::Pos::Named(e) => Some(e.clone()),
                        // `ordo check` refuses a wildcard entity; a run for
                        // one can reach nothing.
                        ordo_syntax::Pos::Any => Some(String::new()),
                    }
                }
                _ => None,
            };
            let ids = run.ids.clone();
            let each_max = self.each_max;
            let more = Arc::new(Mutex::new(None::<u64>));
            let seen = more.clone();
            // `notify`: who holds the level on the rows this run wrote. Read
            // in the run's own transaction, mailed once it commits.
            let notify = routine.notify.clone();
            let told = Arc::new(Mutex::new(Vec::<String>::new()));
            let telling = told.clone();
            let work = move |db: &dyn ordo_conformance::write::WriteDb,
                             log: &mut ordo_conformance::WriteLog| {
                let rows = match &woken {
                    None => None,
                    Some(entity) => {
                        let wanted =
                            ordo_schema::routine::touched_entities(&fixture.schema, &routine);
                        Some(triggers::reachable(db, &fixture.schema, entity, &ids, &wanted)?.0)
                    }
                };
                let out =
                    fixture.run_routine_on(db, log, &routine, &actor, &rows, now, each_max)?;
                // An `each` run that stopped at the limit says so, once it
                // has committed.
                if out.get("more").and_then(|m| m.as_bool()) == Some(true) {
                    *seen.lock().expect("more is not poisoned") =
                        out.get("rows").and_then(|n| n.as_u64());
                }
                let Some(group) = log.pending() else {
                    return Err(NOTHING_CHANGED.to_string());
                };
                if let Some(n) = &routine.notify {
                    let mut written: Vec<(String, Vec<String>)> = Vec::new();
                    for c in &group.commands {
                        if c.kind == ordo_commandlog::CommandKind::Delete {
                            continue;
                        }
                        match written.iter_mut().find(|(e, _)| *e == c.entity) {
                            Some((_, ids)) => ids.push(c.row_id.clone()),
                            None => written.push((c.entity.clone(), vec![c.row_id.clone()])),
                        }
                    }
                    let mut who: Vec<String> = Vec::new();
                    for (entity, ids) in &written {
                        for p in holders(db, &fixture.schema, entity, ids, &n.level.name)? {
                            if !who.contains(&p) {
                                who.push(p);
                            }
                        }
                    }
                    // An attempt that is tried again starts from nothing.
                    *telling.lock().expect("told is not poisoned") = who;
                }
                Ok(out)
            };
            match bound
                .view
                .exec
                .platform_write(bound.log.clone(), work)
                .await
            {
                Ok(_) => {
                    // After the commit, so a run that rolled back tells
                    // nobody. A committed run is not run again (its run is
                    // finished, and a body that marks its rows no longer
                    // matches them), so a notice is queued at most once. A
                    // crash between the commit and the queue loses it.
                    let who = std::mem::take(&mut *told.lock().expect("told is not poisoned"));
                    if let Some(n) = notify.as_ref().filter(|_| !who.is_empty()) {
                        self.tell(&ctx.pool, &name, &run.space.0, &who, &n.text, ctx.now())
                            .await;
                    }
                }
                Err(ordo_exec::ExecFail::Compile(m)) if m == NOTHING_CHANGED => {}
                Err(e) => return Err(format!("routine {name} in {}: {e}", run.space.0)),
            }
            if let Some(n) = *more.lock().expect("more is not poisoned") {
                eprintln!(
                    "routine {name}: handled {n} rows in tenant {} and more matched; the \
                     limit is {each_max} a run ({}). The next run reads again",
                    run.space.0,
                    ordo_plan::each::EACH_MAX_ROWS_ENV
                );
            }
            Ok(())
        })
    }
}
