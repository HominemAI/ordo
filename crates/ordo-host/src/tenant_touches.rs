//! A tenant chooses its own passkey touches, within what the app developer
//! exposes (owner, 2026-11-05, `tenant-chooses-its-touches`).
//!
//! The owner: *"if you let a tennant decide their permissions they can
//! decide their permissions - the dev decides whichway. if its on by default
//! for the tenants they can set it lower. The dev exposes the options."*
//!
//! # Two layers
//!
//! | Who | Sets | Through |
//! |---|---|---|
//! | The app developer | which situations a tenant may change, and which way: `raise` (turn a touch on), `lower` (turn one off), `both`, or `none` | `std.deployment.exposeTouch`, `ordo touches expose`, the console's Security screen |
//! | A tenant's owner | its own touch, on or off, in an exposed situation, for its own space | `std.space.setTouch`, read with `std.space.touches` |
//!
//! **Nothing is exposed by default**, so a deployment that sets nothing
//! behaves as `ORDO_SECURITY_LEVEL` says, in every tenant.
//!
//! # What a tenant may choose
//!
//! The situations whose ops act inside one tenant
//! ([`Situation::tenant_choosable`]): export and import, membership and
//! admission, tenant storage and attestation, approving a delegation, and
//! the tenant's own ways in and origins (account security). The acts on the
//! deployment, recovery, platform reads and a person's contact email can
//! never be exposed; `exposeTouch` refuses them by name.
//!
//! # The order the door reads
//!
//! 1. The per-op settings: `ORDO_STEP_UP_EXEMPT_OPS`,
//!    `ORDO_STEP_UP_REQUIRED_OPS`, the version moves that never ask, the
//!    console keys that always do ([`crate::presence::PresencePolicy::decided_by`]).
//! 2. The tenant's choice, when the exposure in force allows its direction.
//!    A choice the app developer later stops exposing is passed over, not
//!    deleted: exposing it again brings it back.
//! 3. `ORDO_SECURITY_LEVEL`.
//!
//! A system (a machine key) is never asked, whatever a tenant chooses; a
//! choice only decides what a **person** is asked.
//!
//! # Where it lives
//!
//! Both layers are in the control database. `$touchexposure` is append-only:
//! each change is a row with who, when and why, and the latest row per
//! situation is in force, so the history is the table. `$tenanttouches` holds
//! each tenant's current choice; each change is on the security record.
//! Every rebuild of the door reads both ([`DoorTouches::read`]), so a change
//! reaches every node at its next rebuild, and the node that made it at once.

use std::collections::BTreeMap;
use std::sync::Arc;

use ordo_api::{
    BoundParams, Bounds, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind,
    OpSpec, ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_identity::principal::SpaceId;
use serde_json::{json, Value};

use crate::security_level::{SecurityLevel, Situation};

/// The app developer's exposure, and its history.
pub const TOUCHES: &str = "std.deployment.tenantTouches";
/// Change the exposure of one situation, with a reason.
pub const EXPOSE: &str = "std.deployment.exposeTouch";
/// What a tenant may choose, what it chose, and what is in force.
pub const SPACE_TOUCHES: &str = "std.space.touches";
/// A tenant's owner turns one exposed touch on or off, or back to the
/// deployment's.
pub const SET_TOUCH: &str = "std.space.setTouch";

/// Every op of this module.
pub const OPS: [&str; 4] = [TOUCHES, EXPOSE, SPACE_TOUCHES, SET_TOUCH];

/// Ops a tenant's choice never reaches, though their situation is
/// choosable: the op that changes the choice follows the level, so a stolen
/// session cannot lower the touch on lowering touches.
pub const NEVER_CHOSEN: [&str; 1] = [SET_TOUCH];

/// The ops only an owner reaches: the deployment's two (the deployment
/// owner, in its own space) and the tenant's setter.
pub fn owner_only(name: &str) -> bool {
    name == TOUCHES || name == EXPOSE || name == SET_TOUCH
}

/// The most history `tenantTouches` answers.
pub const MAX_HISTORY: u64 = 200;
const DEFAULT_HISTORY: u64 = 20;
/// The longest reason kept with a change.
pub const MAX_REASON: usize = 500;

// ── the exposure ──────────────────────────────────────────────────────────

/// Which way the app developer lets tenants change one touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// A tenant may turn the touch on.
    Raise,
    /// A tenant may turn the touch off.
    Lower,
    /// Either way.
    Both,
}

impl Direction {
    pub fn word(self) -> &'static str {
        match self {
            Direction::Raise => "raise",
            Direction::Lower => "lower",
            Direction::Both => "both",
        }
    }

    /// `raise`, `lower`, `both`, or `none` (`Ok(None)`: not exposed).
    pub fn parse(word: &str) -> Result<Option<Direction>, String> {
        match word.trim() {
            "none" => Ok(None),
            "raise" => Ok(Some(Direction::Raise)),
            "lower" => Ok(Some(Direction::Lower)),
            "both" => Ok(Some(Direction::Both)),
            other => Err(format!(
                "`{other}` is not a direction. Write raise (a tenant may turn the touch on), \
                 lower (off), both, or none"
            )),
        }
    }

    /// Does this direction let a tenant choose `on`?
    pub fn allows(self, on: bool) -> bool {
        match self {
            Direction::Both => true,
            Direction::Raise => on,
            Direction::Lower => !on,
        }
    }
}

/// The exposure in force: one direction per exposed situation. Empty, the
/// default, exposes nothing.
pub type Exposure = BTreeMap<Situation, Direction>;

fn exposed_word(d: Option<Direction>) -> &'static str {
    d.map_or("none", Direction::word)
}

// ── the typed error ───────────────────────────────────────────────────────

/// Why a touch change was refused, or the store did not answer. One error
/// for this client; [`TouchRefusal::to_exec_error`] is the one place it
/// becomes an answer.
#[derive(Debug)]
pub enum TouchRefusal {
    /// A word that names no situation. Fix the input.
    UnknownSituation(String),
    /// A situation that acts on the deployment. Never exposable.
    NotChoosable(Situation),
    /// The input was not one of the values. Fix the input.
    BadValue(String),
    /// The app developer has not exposed this situation, or not this way.
    /// The deployment owner may change it; the refusal names the command.
    NotExposed {
        situation: Situation,
        exposed: Option<Direction>,
        on: bool,
    },
    /// Bound to a tenant, an op of the deployment's, or the reverse.
    WrongSpace(&'static str),
    /// The control database did not answer. Retry.
    Store(PoolError),
}

impl From<PoolError> for TouchRefusal {
    fn from(e: PoolError) -> Self {
        TouchRefusal::Store(e)
    }
}

fn choosable_words() -> String {
    Situation::ALL
        .into_iter()
        .filter(|s| s.tenant_choosable())
        .map(Situation::word)
        .collect::<Vec<_>>()
        .join(", ")
}

impl TouchRefusal {
    pub fn to_exec_error(&self) -> ExecError {
        match self {
            TouchRefusal::UnknownSituation(w) => ExecError::new(
                ErrorCode::InvalidParam,
                format!(
                    "`{w}` is not a situation. A tenant may be let choose: {}",
                    choosable_words()
                ),
            ),
            TouchRefusal::NotChoosable(s) => ExecError::new(
                ErrorCode::InvalidParam,
                format!(
                    "`{}` acts on the deployment, not inside one tenant, so a tenant can never \
                     choose it. A tenant may be let choose: {}",
                    s.word(),
                    choosable_words()
                ),
            ),
            TouchRefusal::BadValue(why) => ExecError::new(ErrorCode::InvalidParam, why.clone()),
            TouchRefusal::NotExposed {
                situation,
                exposed,
                on,
            } => {
                let w = situation.word();
                let (way, need) = if *on {
                    ("on", "raise")
                } else {
                    ("off", "lower")
                };
                ExecError::new(
                    ErrorCode::TenantRefused,
                    format!(
                        "the app developer has not let tenants turn the {w} touch {way}: it is \
                         exposed as {} ({EXPOSE}). The deployment owner allows it with `ordo \
                         touches expose {w} {need}` (or both)",
                        exposed_word(*exposed)
                    ),
                )
            }
            TouchRefusal::WrongSpace(why) => ExecError::new(ErrorCode::NotFound, *why),
            TouchRefusal::Store(e) => ExecError::of_fault(
                e.class(),
                ErrorCode::SpaceUnavailable,
                format!("the touch settings could not be read or written: {e}"),
            ),
        }
    }
}

/// A situation word a tenant may be let choose.
pub fn choosable(word: &str) -> Result<Situation, TouchRefusal> {
    let s = Situation::parse(word).ok_or_else(|| TouchRefusal::UnknownSituation(word.into()))?;
    if !s.tenant_choosable() {
        return Err(TouchRefusal::NotChoosable(s));
    }
    Ok(s)
}

/// May a tenant choose `on` (or off) in `situation` under `exposure`?
pub fn check_choice(
    exposure: &Exposure,
    situation: Situation,
    on: bool,
) -> Result<(), TouchRefusal> {
    match exposure.get(&situation) {
        Some(d) if d.allows(on) => Ok(()),
        exposed => Err(TouchRefusal::NotExposed {
            situation,
            exposed: exposed.copied(),
            on,
        }),
    }
}

// ── what the door reads ───────────────────────────────────────────────────

/// The exposure and every tenant's choices, as the door is built from them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoorTouches {
    pub exposure: Exposure,
    /// Each tenant's stored choices, exposed or not.
    pub choices: BTreeMap<String, BTreeMap<Situation, bool>>,
}

/// The exposure in force: the latest row per situation.
pub const EXPOSURE_SQL: &str = r#"SELECT DISTINCT ON ("situation") "situation", "direction" FROM "$touchexposure" ORDER BY "situation", "id" DESC"#;
/// Every tenant's choices.
pub const CHOICES_SQL: &str =
    r#"SELECT "space", "situation", "touch" FROM "$tenanttouches" ORDER BY "space", "situation""#;

fn exists_sql(table: &str) -> String {
    format!(r#"SELECT to_regclass('"{table}"') IS NOT NULL"#)
}

fn truthy(rows: &[Vec<Option<String>>]) -> bool {
    matches!(
        rows.first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref()),
        Some("t") | Some("true")
    )
}

fn cell(r: &[Option<String>], i: usize) -> &str {
    r.get(i).and_then(|c| c.as_deref()).unwrap_or("")
}

fn exposure_from(rows: &[Vec<Option<String>>]) -> Exposure {
    rows.iter()
        .filter_map(|r| {
            let s = Situation::parse(cell(r, 0)).filter(|s| s.tenant_choosable())?;
            let d = Direction::parse(cell(r, 1)).ok().flatten()?;
            Some((s, d))
        })
        .collect()
}

fn choices_from(rows: &[Vec<Option<String>>]) -> BTreeMap<String, BTreeMap<Situation, bool>> {
    let mut out: BTreeMap<String, BTreeMap<Situation, bool>> = BTreeMap::new();
    for r in rows {
        let Some(s) = Situation::parse(cell(r, 1)) else {
            continue;
        };
        let on = match cell(r, 2) {
            "on" => true,
            "off" => false,
            _ => continue,
        };
        out.entry(cell(r, 0).to_string()).or_default().insert(s, on);
    }
    out
}

impl DoorTouches {
    /// From the control database, over the pool. Tables not made yet read
    /// as nothing exposed.
    pub async fn read(pool: &Pool) -> Result<DoorTouches, PoolError> {
        let mut out = DoorTouches::default();
        if truthy(&pool.rows(&exists_sql("$touchexposure")).await?) {
            out.exposure = exposure_from(&pool.rows(EXPOSURE_SQL).await?);
        }
        if truthy(&pool.rows(&exists_sql("$tenanttouches")).await?) {
            out.choices = choices_from(&pool.rows(CHOICES_SQL).await?);
        }
        Ok(out)
    }

    /// From one database, over the conformance harness.
    pub fn read_pg(pg: &ordo_conformance::Pg) -> Result<DoorTouches, String> {
        let mut out = DoorTouches::default();
        if truthy(&pg.query(&exists_sql("$touchexposure"))?) {
            out.exposure = exposure_from(&pg.query(EXPOSURE_SQL)?);
        }
        if truthy(&pg.query(&exists_sql("$tenanttouches"))?) {
            out.choices = choices_from(&pg.query(CHOICES_SQL)?);
        }
        Ok(out)
    }

    /// The tenant's choice in `situation` that the exposure in force
    /// allows, or `None` (the level decides).
    pub fn in_force(&self, space: &str, situation: Situation) -> Option<bool> {
        let on = *self.choices.get(space)?.get(&situation)?;
        let d = self.exposure.get(&situation)?;
        d.allows(on).then_some(on)
    }

    /// Keep the choices of the spaces `keep` names.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.choices.retain(|s, _| keep(s));
    }

    /// Add another read's choices; its exposure, when it has one.
    pub fn extend(&mut self, other: DoorTouches) {
        if !other.exposure.is_empty() {
            self.exposure = other.exposure;
        }
        for (space, c) in other.choices {
            self.choices.entry(space).or_default().extend(c);
        }
    }
}

/// Lay every tenant's choice for one platform op over the engine: the
/// middle step of the order the module doc gives. Called by the door's
/// build for each platform write that asks or may ask a person.
pub fn install(
    engine: &mut ordo_identity::AccessEngine,
    op: &str,
    kind: OpKind,
    policy: &crate::presence::PresencePolicy,
    touches: &DoorTouches,
) {
    if kind == OpKind::Read || touches.exposure.is_empty() || NEVER_CHOSEN.contains(&op) {
        return;
    }
    if policy.decided_by(op).is_some() {
        return;
    }
    let Some(situation) = crate::security_level::situation_of(op) else {
        return;
    };
    if !situation.tenant_choosable() {
        return;
    }
    let level = policy.requires(op, kind);
    for space in touches.choices.keys() {
        if let Some(on) = touches.in_force(space, situation) {
            if on != level {
                engine.set_step_up_in(&SpaceId(space.clone()), op, on);
            }
        }
    }
}

// ── the store ─────────────────────────────────────────────────────────────

/// The two tables. `$touchexposure` is append-only.
pub const EXPOSURE_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$touchexposure" (
  "id"        bigserial PRIMARY KEY,
  "situation" text   NOT NULL,
  "direction" text   NOT NULL,
  "actor"     text   NOT NULL,
  "reason"    text   NOT NULL,
  "at"        bigint NOT NULL
)"#;

pub const CHOICES_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$tenanttouches" (
  "space"      text   NOT NULL,
  "situation"  text   NOT NULL,
  "touch"      text   NOT NULL,
  "updated_by" text   NOT NULL,
  "updated_at" bigint NOT NULL,
  PRIMARY KEY ("space", "situation")
)"#;

pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [EXPOSURE_DDL, CHOICES_DDL] {
        // Two nodes booting at once can race on `CREATE TABLE IF NOT
        // EXISTS`; the second try sees the table.
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// One change of the exposure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposureChange {
    pub situation: String,
    pub exposed: String,
    pub actor: String,
    pub reason: String,
    pub at: u64,
}

/// The one client over the two tables.
#[derive(Clone)]
pub struct TouchStore {
    pool: Pool,
}

impl TouchStore {
    pub fn on(pool: &Pool) -> TouchStore {
        TouchStore { pool: pool.clone() }
    }

    pub async fn exposure(&self) -> Result<Exposure, TouchRefusal> {
        ensure_table(&self.pool).await?;
        Ok(exposure_from(&self.pool.rows(EXPOSURE_SQL).await?))
    }

    pub async fn history(&self, limit: u64) -> Result<Vec<ExposureChange>, TouchRefusal> {
        ensure_table(&self.pool).await?;
        let rows = self
            .pool
            .rows(&format!(
                r#"SELECT "situation", "direction", "actor", "reason", "at" FROM "$touchexposure" ORDER BY "id" DESC LIMIT {}"#,
                limit.clamp(1, MAX_HISTORY)
            ))
            .await?;
        Ok(rows
            .iter()
            .map(|r| ExposureChange {
                situation: cell(r, 0).to_string(),
                exposed: cell(r, 1).to_string(),
                actor: cell(r, 2).to_string(),
                reason: cell(r, 3).to_string(),
                at: cell(r, 4).parse().unwrap_or(0),
            })
            .collect())
    }

    /// Record a new exposure for `situation`. `false` when it was already
    /// that, and nothing was written.
    pub async fn expose(
        &self,
        situation: Situation,
        direction: Option<Direction>,
        actor: &str,
        reason: &str,
        at: u64,
    ) -> Result<bool, TouchRefusal> {
        if !situation.tenant_choosable() {
            return Err(TouchRefusal::NotChoosable(situation));
        }
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > MAX_REASON {
            return Err(TouchRefusal::BadValue(format!(
                "the exposure was not changed: a reason of 1 to {MAX_REASON} characters says \
                 why, and is kept with the change"
            )));
        }
        if self.exposure().await?.get(&situation).copied() == direction {
            return Ok(false);
        }
        self.pool
            .rows(&format!(
                r#"INSERT INTO "$touchexposure" ("situation", "direction", "actor", "reason", "at")
                   VALUES ({}, {}, {}, {}, {at})"#,
                sql_lit(situation.word()),
                sql_lit(exposed_word(direction)),
                sql_lit(actor),
                sql_lit(reason),
            ))
            .await?;
        Ok(true)
    }

    /// One tenant's stored choices.
    pub async fn choices(&self, space: &str) -> Result<BTreeMap<Situation, bool>, TouchRefusal> {
        ensure_table(&self.pool).await?;
        let rows = self
            .pool
            .rows(&format!(
                r#"SELECT "space", "situation", "touch" FROM "$tenanttouches" WHERE "space" = {}"#,
                sql_lit(space)
            ))
            .await?;
        Ok(choices_from(&rows).remove(space).unwrap_or_default())
    }

    /// Set one tenant's touch in `situation`: `Some(on)`, or `None` to go
    /// back to the deployment's. Checked against the exposure in force.
    pub async fn choose(
        &self,
        space: &str,
        situation: Situation,
        on: Option<bool>,
        actor: &str,
        at: u64,
    ) -> Result<(), TouchRefusal> {
        if !situation.tenant_choosable() {
            return Err(TouchRefusal::NotChoosable(situation));
        }
        match on {
            None => {
                ensure_table(&self.pool).await?;
                self.pool
                    .rows(&format!(
                        r#"DELETE FROM "$tenanttouches" WHERE "space" = {} AND "situation" = {}"#,
                        sql_lit(space),
                        sql_lit(situation.word())
                    ))
                    .await?;
            }
            Some(on) => {
                check_choice(&self.exposure().await?, situation, on)?;
                self.pool
                    .rows(&format!(
                        r#"INSERT INTO "$tenanttouches" ("space", "situation", "touch", "updated_by", "updated_at")
                           VALUES ({}, {}, {}, {}, {at})
                           ON CONFLICT ("space", "situation") DO UPDATE SET "touch" = EXCLUDED."touch",
                             "updated_by" = EXCLUDED."updated_by", "updated_at" = EXCLUDED."updated_at""#,
                        sql_lit(space),
                        sql_lit(situation.word()),
                        sql_lit(if on { "on" } else { "off" }),
                        sql_lit(actor),
                    ))
                    .await?;
            }
        }
        Ok(())
    }
}

// ── the ops ───────────────────────────────────────────────────────────────

/// The ops' state.
pub struct TouchOps {
    pub store: TouchStore,
    /// The deployment's own space: the app developer's two ops answer there
    /// only, and a tenant's two never do.
    pub home: String,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

fn text<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn one(row: Value) -> ExecResult {
    ExecResult {
        position: None,
        rows: vec![row],
        next_cursor: None,
    }
}

/// The platform writes in `situation` that a per-op setting decides, with
/// the setting.
fn decided(
    policy: &crate::presence::PresencePolicy,
    situation: Situation,
) -> Vec<(String, &'static str)> {
    crate::platform_ops::specs()
        .into_iter()
        .filter(|s| s.kind != OpKind::Read)
        .filter(|s| crate::security_level::situation_of(&s.name) == Some(situation))
        .filter_map(|s| policy.decided_by(&s.name).map(|by| (s.name, by)))
        .collect()
}

impl TouchOps {
    pub fn on_pool(pool: &Pool, home: impl Into<String>) -> TouchOps {
        TouchOps {
            store: TouchStore::on(pool),
            home: home.into(),
            stream: None,
            now: Arc::new(crate::runtime::now_secs),
        }
    }

    pub fn with_stream(mut self, stream: Arc<ordo_secevent::Recorder>) -> TouchOps {
        self.stream = Some(stream);
        self
    }

    /// Run one of this module's ops. `rebuild` rebuilds the door after a
    /// change (here, and announced to every node); its words go in the
    /// answer.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
        rebuild: Option<&crate::member_ops::MemberOps>,
    ) -> Result<ExecResult, ExecError> {
        self.run(op, space, bound, caller, rebuild)
            .await
            .map_err(|e| e.to_exec_error())
    }

    async fn run(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
        rebuild: Option<&crate::member_ops::MemberOps>,
    ) -> Result<ExecResult, TouchRefusal> {
        let deployment = op == TOUCHES || op == EXPOSE;
        if deployment && space != self.home {
            return Err(TouchRefusal::WrongSpace(
                "which touches tenants may choose is the app developer's, for every tenant at \
                 once: this op answers only in the deployment's own space",
            ));
        }
        if !deployment && space == self.home {
            return Err(TouchRefusal::WrongSpace(
                "the deployment's own space chooses no touches: its touches are \
                 ORDO_SECURITY_LEVEL's. Call this op bound to a tenant",
            ));
        }
        match op {
            TOUCHES => {
                let limit = bound
                    .0
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(DEFAULT_HISTORY);
                self.deployment_row(limit).await.map(one)
            }
            EXPOSE => {
                let word = text(bound, "situation").unwrap_or_default();
                let situation = choosable(word)?;
                let direction = Direction::parse(text(bound, "exposed").unwrap_or_default())
                    .map_err(TouchRefusal::BadValue)?;
                let reason = text(bound, "reason").unwrap_or_default();
                let now = (self.now)();
                let changed = self
                    .store
                    .expose(situation, direction, caller, reason, now)
                    .await?;
                let mut door = Value::Null;
                if changed {
                    self.record(
                        caller,
                        None,
                        "tenant-touch-exposed",
                        format!(
                            "situation={} exposed={} by={caller}",
                            situation.word(),
                            exposed_word(direction)
                        ),
                        now,
                    );
                    if let Some(m) = rebuild {
                        door = json!(m.rebuilt(space).await);
                    }
                }
                let mut row = self.deployment_row(DEFAULT_HISTORY).await?;
                row["situation"] = json!(situation.word());
                row["exposed"] = json!(exposed_word(direction));
                row["changed"] = json!(changed);
                row["door"] = door;
                Ok(one(row))
            }
            SPACE_TOUCHES => self.space_row(space).await.map(one),
            SET_TOUCH => {
                let word = text(bound, "situation").unwrap_or_default();
                let situation = choosable(word)?;
                let on = match text(bound, "touch").unwrap_or_default() {
                    "on" => Some(true),
                    "off" => Some(false),
                    "default" => None,
                    other => {
                        return Err(TouchRefusal::BadValue(format!(
                            "`{other}` is not a touch. Write on, off, or default (the \
                             deployment's, ORDO_SECURITY_LEVEL)"
                        )))
                    }
                };
                let now = (self.now)();
                self.store.choose(space, situation, on, caller, now).await?;
                self.record(
                    caller,
                    Some(space),
                    "tenant-touch-set",
                    format!(
                        "situation={} touch={} by={caller}",
                        situation.word(),
                        on.map_or("default", |o| if o { "on" } else { "off" })
                    ),
                    now,
                );
                let door = match rebuild {
                    Some(m) => json!(m.rebuilt(space).await),
                    None => Value::Null,
                };
                let mut row = self.space_row(space).await?;
                row["situation"] = json!(situation.word());
                row["touch"] = json!(on.map_or("default", |o| if o { "on" } else { "off" }));
                row["door"] = door;
                Ok(one(row))
            }
            other => Err(TouchRefusal::BadValue(format!(
                "`{other}` is not a touch op"
            ))),
        }
    }

    async fn deployment_row(&self, limit: u64) -> Result<Value, TouchRefusal> {
        let exposure = self.store.exposure().await?;
        let level = crate::presence::current().level();
        let situations: Vec<Value> = Situation::ALL
            .into_iter()
            .map(|s| {
                json!({
                    "situation": s.word(),
                    "label": s.label(),
                    "choosable": s.tenant_choosable(),
                    "exposed": exposed_word(exposure.get(&s).copied()),
                    "levelAsks": level.asks(s),
                })
            })
            .collect();
        let history: Vec<Value> = self
            .store
            .history(limit)
            .await?
            .into_iter()
            .map(|c| {
                json!({
                    "situation": c.situation, "exposed": c.exposed, "actor": c.actor,
                    "reason": c.reason, "at": c.at,
                })
            })
            .collect();
        Ok(json!({
            "level": level.as_str(),
            "situations": situations,
            "history": history,
        }))
    }

    async fn space_row(&self, space: &str) -> Result<Value, TouchRefusal> {
        let exposure = self.store.exposure().await?;
        let choices = self.store.choices(space).await?;
        let policy = crate::presence::current();
        let level: SecurityLevel = policy.level();
        let door = DoorTouches {
            exposure: exposure.clone(),
            choices: BTreeMap::from([(space.to_string(), choices.clone())]),
        };
        let situations: Vec<Value> = Situation::ALL
            .into_iter()
            .filter(|s| s.tenant_choosable())
            .map(|s| {
                let chosen = door.in_force(space, s);
                json!({
                    "situation": s.word(),
                    "label": s.label(),
                    "exposed": exposed_word(exposure.get(&s).copied()),
                    "levelAsks": level.asks(s),
                    "choice": choices.get(&s).map(|o| if *o { "on" } else { "off" }),
                    "inForce": chosen.unwrap_or_else(|| level.asks(s)),
                    "decidedBy": decided(&policy, s)
                        .into_iter()
                        .map(|(op, by)| json!({ "op": op, "setting": by }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({ "level": level.as_str(), "situations": situations }))
    }

    fn record(&self, by: &str, tenant: Option<&str>, act: &str, detail: String, at: u64) {
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act,
                    detail,
                    by: Some(by),
                    tenant,
                    // A tenant's own choice goes on its own record; the
                    // exposure on the deployment's.
                    deployment: tenant.is_none(),
                    at,
                },
            );
        }
    }
}

// ── the specs ─────────────────────────────────────────────────────────────

pub fn specs() -> Vec<OpSpec> {
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, fields: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: fields.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(if kind == OpKind::Read {
            NotSyncable::NoAnchor
        } else {
            NotSyncable::NotARead
        }),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    vec![
        spec(
            TOUCHES,
            OpKind::Read,
            vec![ParamSpec {
                name: "limit".to_string(),
                ty: ParamTy::Int,
                required: false,
                bounds: Some(Bounds {
                    min: Some("1".to_string()),
                    max: Some(MAX_HISTORY.to_string()),
                }),
            }],
            &["level", "situations", "history"],
        ),
        spec(
            EXPOSE,
            OpKind::Write,
            vec![
                text("situation", 64, true),
                text("exposed", 16, true),
                text("reason", 4096, true),
            ],
            &[
                "situation",
                "exposed",
                "changed",
                "door",
                "level",
                "situations",
                "history",
            ],
        ),
        spec(
            SPACE_TOUCHES,
            OpKind::Read,
            Vec::new(),
            &["level", "situations"],
        ),
        spec(
            SET_TOUCH,
            OpKind::Write,
            vec![text("situation", 64, true), text("touch", 16, true)],
            &["situation", "touch", "door", "level", "situations"],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_direction_allows_its_way_only() {
        assert!(Direction::Raise.allows(true));
        assert!(!Direction::Raise.allows(false));
        assert!(Direction::Lower.allows(false));
        assert!(!Direction::Lower.allows(true));
        assert!(Direction::Both.allows(true) && Direction::Both.allows(false));
        assert_eq!(Direction::parse("none"), Ok(None));
        assert!(Direction::parse("sideways").unwrap_err().contains("raise"));
    }

    #[test]
    fn nothing_exposed_refuses_every_choice_naming_the_command() {
        let e = check_choice(&Exposure::new(), Situation::Membership, false).unwrap_err();
        let m = e.to_exec_error().message;
        assert!(m.contains("exposed as none"), "{m}");
        assert!(m.contains("ordo touches expose membership lower"), "{m}");
    }

    #[test]
    fn a_choice_the_exposure_no_longer_allows_is_passed_over() {
        let mut door = DoorTouches::default();
        door.choices
            .entry("s1".into())
            .or_default()
            .insert(Situation::Membership, false);
        assert_eq!(door.in_force("s1", Situation::Membership), None);
        door.exposure
            .insert(Situation::Membership, Direction::Raise);
        assert_eq!(door.in_force("s1", Situation::Membership), None);
        door.exposure.insert(Situation::Membership, Direction::Both);
        assert_eq!(door.in_force("s1", Situation::Membership), Some(false));
        assert_eq!(door.in_force("s2", Situation::Membership), None);
    }

    #[test]
    fn deployment_situations_are_never_choosable() {
        for s in Situation::ALL.into_iter().filter(|s| !s.tenant_choosable()) {
            let e = choosable(s.word()).unwrap_err();
            assert!(matches!(e, TouchRefusal::NotChoosable(_)), "{s:?}");
        }
        assert!(matches!(
            choosable("everything"),
            Err(TouchRefusal::UnknownSituation(_))
        ));
    }
}
