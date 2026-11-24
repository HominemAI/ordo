//! **Rollouts, the core** (`plan-versions.md` §3.5, §3.7, increment 10).
//!
//! A rollout moves the deployment from one version to another a batch of
//! tenants at a time. Its record is `$rollouts`, one row per rollout, and
//! `$rollout_tenants`, one row per tenant it moves. Both are in the control
//! database. Each tenant's move is also its `$pins` row, cause `rollout`,
//! naming the rollout (§1.12). [`Rollouts`] is the one client of the three
//! tables, with one typed error, [`RolloutRefusal`], which wraps
//! [`VersionRefusal`] for what the pins refuse (rule 9). It sits beside
//! [`Versions`], as `crate::evolve::Plans` does.
//!
//! # The rules
//!
//! - **One deployment-wide rollout at a time** (ruled 2026-11-23, Q3). A
//!   second is refused, naming `ordo rollout status` and `ordo rollout
//!   abort`, and so is any activation while one is open
//!   ([`VersionRefusal::RolloutOpen`]).
//! - **One tenant may always move on its own** to a hotfix version, during
//!   a rollout or not (`ordo versions pin`, Q3). The rollout then skips it:
//!   a tenant whose newest `$pins` row since the rollout started is a
//!   person's `pin` is `skipped`.
//! - **The deployment's own space is a named step** (Q4), `$deployment`. It
//!   moves first, or last when `rollout.deployment_space` is `last`. Its
//!   move is the activation ([`Versions::activate_alone`]): every node loads
//!   the version and swaps, so the deployment's own space, the public path
//!   and new tenants move, and no tenant does.
//! - **Strategies**: `rolling`, a batch every `rollout.batch_interval`,
//!   sized to finish within `rollout.window`; `all`, every tenant in one
//!   batch. Canary, group first, by query and health are increment 10c.
//! - **Windows and holds.** A tenant a maintenance window covers moves only
//!   while one holds; outside, it is `waiting`. A held tenant is `waiting`.
//!   A tenant no window covers moves in its batch.
//! - **Unreachable is waiting, never failed** (ruled 2026-11-23, Q10). A
//!   tenant whose own database cannot be reached waits and is tried again.
//! - **The failure threshold.** `rollout.failure_threshold` failed tenants
//!   since the last resume pause the rollout. A failed tenant serves its old
//!   version.
//! - **The leading worker.** Every node runs [`tick`]. A tick takes a
//!   transaction-scoped advisory lock on the control database first and does
//!   nothing when another node holds it. So one node leads at a time, and
//!   when the leader dies its connection closes, the lock is free, and the
//!   next node's tick goes on from the tables.
//! - **Acts by a person** (start, resume, skip, retry, abort, finish, hold)
//!   are one entry each on the deployment's hash-chained stream (ruled
//!   2026-11-23, Q9). A pause by the threshold and the finish are entries
//!   too. Per-tenant moves stay in `$pins` and `$rollout_tenants`.
//! - **The settings** live in `$version_settings`, beside the 3 version
//!   settings, set through the door with who, when and why (Q15).

use std::collections::{BTreeMap, BTreeSet};

use ordo_api::{
    BoundParams, ErrorCode, ExecError, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec, ParamSpec,
    ParamTy, RefineTier, Syncability,
};
use ordo_commandlog::Timestamp;
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_fault::FaultClass;
use serde_json::{json, Value};

use crate::apply_op::ApplyOps;
use crate::schema_apply::{self as sa, Program};
use crate::versions::{Remedy, VersionRefusal, Versions};
use crate::windows::Windows;

// ── names ────────────────────────────────────────────────────────────────

/// Start a rollout of a stored version.
pub const START: &str = "std.deployment.rollout.start";
/// The open rollout, or the newest, or one by id, and its tenants.
pub const STATUS: &str = "std.deployment.rollout.status";
/// Resume a paused rollout.
pub const RESUME: &str = "std.deployment.rollout.resume";
/// Skip one tenant: it stays where it is.
pub const SKIP: &str = "std.deployment.rollout.skip";
/// Try one failed tenant again, or one tenant's refused move (§2.1).
pub const RETRY: &str = "std.deployment.rollout.retry";
/// Stop the open rollout. Moved tenants stay; `ordo rollback` moves them.
pub const ABORT: &str = "std.deployment.rollout.abort";
/// Close the open rollout by hand, the rest skipped.
pub const FINISH: &str = "std.deployment.rollout.finish";
/// Hold one tenant's moves, or lift the hold.
pub const HOLD: &str = "std.deployment.rollout.hold";

pub const OPS: [&str; 8] = [START, STATUS, RESUME, SKIP, RETRY, ABORT, FINISH, HOLD];

/// The rollout row naming the deployment's own space (§1.14, Q4). A
/// tenant's id never starts with `$`.
pub const DEPLOYMENT_STEP: &str = "$deployment";

/// Who a rollout's own acts are recorded as: its moves, a pause by the
/// threshold, the finish.
pub const ACTOR: &str = "ordo:rollout";

/// The advisory lock the leading worker takes for each tick, at
/// transaction scope (`ORRL`).
pub const ROLLOUT_LOCK_NS: i32 = 0x4F52_524C;

/// How often each node ticks, in seconds. The batch interval decides when a
/// batch moves; this only bounds how late.
pub const TICK_ENV: &str = "ORDO_ROLLOUT_TICK_SECS";
/// Its default.
pub const TICK_DEFAULT: u64 = 5;

/// How many tenants [`Rollouts::status`] lists at most. The counts are
/// always whole.
pub const MAX_LISTED: usize = 1_000;

/// The settings (`plan-versions.md` §7), stored in `$version_settings`.
pub const STRATEGY_SETTING: &str = "rollout.strategy";
pub const BATCH_INTERVAL_SETTING: &str = "rollout.batch_interval";
pub const WINDOW_SETTING: &str = "rollout.window";
pub const FAILURE_THRESHOLD_SETTING: &str = "rollout.failure_threshold";
pub const DEPLOYMENT_SPACE_SETTING: &str = "rollout.deployment_space";

pub const SETTINGS: [&str; 5] = [
    STRATEGY_SETTING,
    BATCH_INTERVAL_SETTING,
    WINDOW_SETTING,
    FAILURE_THRESHOLD_SETTING,
    DEPLOYMENT_SPACE_SETTING,
];

/// `rollout.batch_interval`'s default: 5 minutes (owner, 2026-09-21).
pub const BATCH_INTERVAL_DEFAULT: i64 = 300;
/// `rollout.window`'s default: 1 hour, a target.
pub const WINDOW_DEFAULT: i64 = 3_600;
/// `rollout.failure_threshold`'s default: 1 tenant.
pub const FAILURE_THRESHOLD_DEFAULT: i64 = 1;

// ── the words ────────────────────────────────────────────────────────────

/// Which pins move when (§3.5). Increment 10 builds 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// A batch every `rollout.batch_interval`.
    Rolling,
    /// Every tenant in one batch.
    All,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Rolling => "rolling",
            Strategy::All => "all",
        }
    }

    pub fn parse(text: &str) -> Option<Strategy> {
        match text.trim() {
            "rolling" => Some(Strategy::Rolling),
            "all" => Some(Strategy::All),
            _ => None,
        }
    }
}

/// When the deployment's own space moves (Q4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentSpace {
    First,
    Last,
}

impl DeploymentSpace {
    pub fn as_str(self) -> &'static str {
        match self {
            DeploymentSpace::First => "first",
            DeploymentSpace::Last => "last",
        }
    }

    pub fn parse(text: &str) -> Option<DeploymentSpace> {
        match text.trim() {
            "first" => Some(DeploymentSpace::First),
            "last" => Some(DeploymentSpace::Last),
            _ => None,
        }
    }
}

/// A rollout's state (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RolloutState {
    Running,
    Paused,
    Finished,
    Aborted,
}

impl RolloutState {
    pub fn as_str(self) -> &'static str {
        match self {
            RolloutState::Running => "running",
            RolloutState::Paused => "paused",
            RolloutState::Finished => "finished",
            RolloutState::Aborted => "aborted",
        }
    }

    fn parse(text: &str) -> RolloutState {
        match text {
            "paused" => RolloutState::Paused,
            "finished" => RolloutState::Finished,
            "aborted" => RolloutState::Aborted,
            _ => RolloutState::Running,
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, RolloutState::Running | RolloutState::Paused)
    }
}

/// A tenant's state in a rollout (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TenantState {
    /// In a later batch.
    Queued,
    /// Outside its window, held, or its database cannot be reached.
    Waiting,
    /// Its move is under way.
    Moving,
    /// Pinned to the target.
    Moved,
    /// Refused; it serves the old version.
    Failed,
    /// Skipped by the deployment owner, or moved on its own.
    Skipped,
}

impl TenantState {
    pub const ALL: [TenantState; 6] = [
        TenantState::Queued,
        TenantState::Waiting,
        TenantState::Moving,
        TenantState::Moved,
        TenantState::Failed,
        TenantState::Skipped,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TenantState::Queued => "queued",
            TenantState::Waiting => "waiting",
            TenantState::Moving => "moving",
            TenantState::Moved => "moved",
            TenantState::Failed => "failed",
            TenantState::Skipped => "skipped",
        }
    }

    fn parse(text: &str) -> TenantState {
        match text {
            "waiting" => TenantState::Waiting,
            "moving" => TenantState::Moving,
            "moved" => TenantState::Moved,
            "failed" => TenantState::Failed,
            "skipped" => TenantState::Skipped,
            _ => TenantState::Queued,
        }
    }

    /// Still to move: what keeps a rollout open.
    pub fn is_pending(self) -> bool {
        matches!(
            self,
            TenantState::Queued | TenantState::Waiting | TenantState::Moving
        )
    }
}

// ── the settings ─────────────────────────────────────────────────────────

/// The rollout settings as stored (§7, Q15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutSettings {
    /// `None`: unset, and `ordo apply` activates as it did before rollouts
    /// existed, every tenant at once. A value makes each apply start a
    /// rollout with that strategy.
    pub strategy: Option<Strategy>,
    pub batch_interval: i64,
    pub window: i64,
    pub failure_threshold: i64,
    pub deployment_space: DeploymentSpace,
}

impl Default for RolloutSettings {
    fn default() -> RolloutSettings {
        RolloutSettings {
            strategy: None,
            batch_interval: BATCH_INTERVAL_DEFAULT,
            window: WINDOW_DEFAULT,
            failure_threshold: FAILURE_THRESHOLD_DEFAULT,
            deployment_space: DeploymentSpace::First,
        }
    }
}

impl RolloutSettings {
    /// The value of one as it reads, `None` when unset.
    pub fn to_json(&self) -> Value {
        json!({
            "strategy": self.strategy.map(Strategy::as_str),
            "batchInterval": self.batch_interval,
            "window": self.window,
            "failureThreshold": self.failure_threshold,
            "deploymentSpace": self.deployment_space.as_str(),
        })
    }

    fn with(&mut self, setting: &str, value: Option<&str>) {
        match (setting, value) {
            (STRATEGY_SETTING, v) => self.strategy = v.and_then(Strategy::parse),
            (BATCH_INTERVAL_SETTING, Some(v)) => {
                self.batch_interval = parse_secs(v).unwrap_or(BATCH_INTERVAL_DEFAULT)
            }
            (BATCH_INTERVAL_SETTING, None) => self.batch_interval = BATCH_INTERVAL_DEFAULT,
            (WINDOW_SETTING, Some(v)) => self.window = parse_secs(v).unwrap_or(WINDOW_DEFAULT),
            (WINDOW_SETTING, None) => self.window = WINDOW_DEFAULT,
            (FAILURE_THRESHOLD_SETTING, Some(v)) => {
                self.failure_threshold = v.trim().parse().unwrap_or(FAILURE_THRESHOLD_DEFAULT)
            }
            (FAILURE_THRESHOLD_SETTING, None) => self.failure_threshold = FAILURE_THRESHOLD_DEFAULT,
            (DEPLOYMENT_SPACE_SETTING, v) => {
                self.deployment_space = v
                    .and_then(DeploymentSpace::parse)
                    .unwrap_or(DeploymentSpace::First)
            }
            _ => {}
        }
    }
}

fn parse_secs(text: &str) -> Option<i64> {
    crate::windows::parse_length(text).ok()
}

/// The full name of a rollout setting from what a person typed:
/// `rollout.batch_interval`, `batch_interval` or `batch-interval`.
pub fn setting_name(text: &str) -> Option<&'static str> {
    let t = text.trim();
    let t = t.strip_prefix("rollout.").unwrap_or(t).replace('-', "_");
    SETTINGS
        .into_iter()
        .find(|s| s.strip_prefix("rollout.") == Some(t.as_str()))
}

/// Check a value for `setting` and answer it as stored. `None` clears it.
pub fn check_setting(setting: &str, value: Option<&str>) -> Result<Option<String>, RolloutRefusal> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let bad = |why: &str| RolloutRefusal::Setting {
        setting: setting.to_string(),
        value: v.chars().take(40).collect(),
        why: why.to_string(),
    };
    match setting {
        STRATEGY_SETTING => Strategy::parse(v)
            .map(|s| Some(s.as_str().to_string()))
            .ok_or_else(|| {
                bad(
                    "it is `rolling` or `all`; canary, group first and by query come later. \
                     `--clear` makes `ordo apply` move every tenant at once, as before rollouts",
                )
            }),
        BATCH_INTERVAL_SETTING | WINDOW_SETTING => match parse_secs(v) {
            Some(n) if n >= 1 => Ok(Some(n.to_string())),
            _ => Err(bad(
                "it is a length of at least 1 second, like `30s`, `5m` or `1h`",
            )),
        },
        FAILURE_THRESHOLD_SETTING => match v.parse::<i64>() {
            Ok(n) if n >= 1 => Ok(Some(n.to_string())),
            _ => Err(bad("it is a whole number of tenants, 1 or more")),
        },
        DEPLOYMENT_SPACE_SETTING => DeploymentSpace::parse(v)
            .map(|d| Some(d.as_str().to_string()))
            .ok_or_else(|| bad("it is `first` or `last`")),
        _ => Err(bad("there is no such rollout setting")),
    }
}

// ── the record ───────────────────────────────────────────────────────────

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$rollouts\" ( \
  \"id\" bigserial PRIMARY KEY, \
  \"target\" text NOT NULL, \
  \"from\" text NULL, \
  \"strategy\" text NOT NULL, \
  \"batch_interval\" bigint NOT NULL, \
  \"window\" bigint NOT NULL, \
  \"batch_size\" bigint NOT NULL, \
  \"failure_threshold\" bigint NOT NULL, \
  \"deployment_space\" text NOT NULL, \
  \"state\" text NOT NULL, \
  \"reason\" text NOT NULL DEFAULT '', \
  \"actor\" text NOT NULL, \
  \"started_at\" bigint NOT NULL, \
  \"ended_at\" bigint NULL, \
  \"last_batch_at\" bigint NOT NULL DEFAULT 0, \
  \"batches\" bigint NOT NULL DEFAULT 0, \
  \"failed_base\" bigint NOT NULL DEFAULT 0, \
  \"pin_seq\" bigint NOT NULL DEFAULT 0, \
  \"leader\" text NULL, \
  \"led_at\" bigint NULL \
); \
CREATE UNIQUE INDEX IF NOT EXISTS \"$rollouts_one_open\" ON \"$rollouts\" ((1)) \
  WHERE \"state\" IN ('running', 'paused'); \
CREATE TABLE IF NOT EXISTS \"$rollout_tenants\" ( \
  \"rollout\" bigint NOT NULL, \
  \"space\" text NOT NULL, \
  \"ord\" bigint NOT NULL, \
  \"state\" text NOT NULL, \
  \"reason\" text NOT NULL DEFAULT '', \
  \"at\" bigint NOT NULL, \
  PRIMARY KEY (\"rollout\", \"space\") \
); \
CREATE INDEX IF NOT EXISTS \"$rollout_tenants_space\" ON \"$rollout_tenants\" (\"space\"); \
CREATE TABLE IF NOT EXISTS \"$rollout_holds\" ( \
  \"space\" text PRIMARY KEY, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

const COLUMNS: &str =
    "\"id\", \"target\", \"from\", \"strategy\", \"batch_interval\", \"window\", \
  \"batch_size\", \"failure_threshold\", \"deployment_space\", \"state\", \"reason\", \"actor\", \
  \"started_at\", \"ended_at\", \"last_batch_at\", \"batches\", \"failed_base\", \"pin_seq\", \
  \"leader\", \"led_at\"";

/// One rollout as `$rollouts` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rollout {
    pub id: i64,
    pub target: String,
    /// The active version when it started.
    pub from: Option<String>,
    pub strategy: Strategy,
    pub batch_interval: i64,
    pub window: i64,
    pub batch_size: i64,
    pub failure_threshold: i64,
    pub deployment_space: DeploymentSpace,
    pub state: RolloutState,
    /// Why it paused, was aborted or finished. Empty while it runs.
    pub reason: String,
    pub actor: String,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub last_batch_at: u64,
    pub batches: i64,
    /// Failed tenants counted at the last resume: the threshold counts the
    /// failures after it.
    pub failed_base: i64,
    /// The newest `$pins` row when it started: a person's pin after it is a
    /// hotfix the rollout skips.
    pub pin_seq: i64,
    /// The node that ran the last tick, and when.
    pub leader: Option<String>,
    pub led_at: Option<u64>,
}

fn rollout_row(r: &[Option<String>]) -> Option<Rollout> {
    let s = |i: usize| r.get(i).cloned().flatten();
    let n = |i: usize| s(i).and_then(|v| v.parse::<i64>().ok());
    Some(Rollout {
        id: n(0)?,
        target: s(1)?,
        from: s(2),
        strategy: Strategy::parse(&s(3)?).unwrap_or(Strategy::Rolling),
        batch_interval: n(4)?,
        window: n(5)?,
        batch_size: n(6)?,
        failure_threshold: n(7)?,
        deployment_space: DeploymentSpace::parse(&s(8)?).unwrap_or(DeploymentSpace::First),
        state: RolloutState::parse(&s(9)?),
        reason: s(10).unwrap_or_default(),
        actor: s(11)?,
        started_at: n(12)? as u64,
        ended_at: n(13).map(|v| v as u64),
        last_batch_at: n(14).unwrap_or(0) as u64,
        batches: n(15).unwrap_or(0),
        failed_base: n(16).unwrap_or(0),
        pin_seq: n(17).unwrap_or(0),
        leader: s(18),
        led_at: n(19).map(|v| v as u64),
    })
}

impl Rollout {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "target": self.target,
            "from": self.from,
            "strategy": self.strategy.as_str(),
            "batchInterval": self.batch_interval,
            "window": self.window,
            "batchSize": self.batch_size,
            "failureThreshold": self.failure_threshold,
            "deploymentSpace": self.deployment_space.as_str(),
            "state": self.state.as_str(),
            "reason": self.reason,
            "actor": self.actor,
            "startedAt": self.started_at,
            "endedAt": self.ended_at,
            "lastBatchAt": self.last_batch_at,
            "batches": self.batches,
            "leader": self.leader,
            "ledAt": self.led_at,
        })
    }
}

/// One tenant's row in a rollout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutTenant {
    pub space: String,
    pub ord: i64,
    pub state: TenantState,
    /// Why it waits, failed or was skipped. Never shown to the tenant's
    /// people.
    pub reason: String,
    pub at: u64,
}

impl RolloutTenant {
    pub fn to_json(&self) -> Value {
        json!({
            "tenant": self.space,
            "state": self.state.as_str(),
            "reason": self.reason,
            "at": self.at,
        })
    }
}

fn tenant_row(r: &[Option<String>]) -> Option<RolloutTenant> {
    Some(RolloutTenant {
        space: r.first()?.clone()?,
        ord: r.get(1)?.clone()?.parse().ok()?,
        state: TenantState::parse(r.get(2)?.as_deref()?),
        reason: r.get(3).cloned().flatten().unwrap_or_default(),
        at: r.get(4)?.clone()?.parse().ok()?,
    })
}

/// A rollout, its counts per state, and its tenants.
#[derive(Debug, Clone)]
pub struct Status {
    pub rollout: Rollout,
    pub counts: BTreeMap<&'static str, u64>,
    /// Up to [`MAX_LISTED`]: failed and waiting first. The deployment's
    /// own space is not among them; see `deployment`.
    pub tenants: Vec<RolloutTenant>,
    /// The deployment's own space's step, when the rollout has one.
    pub deployment: Option<RolloutTenant>,
}

impl Status {
    pub fn to_json(&self) -> Value {
        let mut row = self.rollout.to_json();
        row["counts"] = json!(self.counts);
        row["deploymentStep"] = self
            .deployment
            .as_ref()
            .map(|d| json!({ "state": d.state.as_str(), "reason": d.reason, "at": d.at }))
            .unwrap_or(Value::Null);
        row["tenants"] = json!(self
            .tenants
            .iter()
            .map(RolloutTenant::to_json)
            .collect::<Vec<_>>());
        row
    }

    /// The tenants still to move.
    pub fn pending(&self) -> u64 {
        TenantState::ALL
            .into_iter()
            .filter(|s| s.is_pending())
            .map(|s| self.counts.get(s.as_str()).copied().unwrap_or(0))
            .sum()
    }
}

/// A hold on one tenant's moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    pub space: String,
    pub actor: String,
    pub reason: String,
    pub at: u64,
}

/// What [`Rollouts::start`] may be told beyond the stored settings.
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    pub strategy: Option<Strategy>,
    pub batch_interval: Option<i64>,
    pub window: Option<i64>,
    pub failure_threshold: Option<i64>,
    pub deployment_space: Option<DeploymentSpace>,
    /// Tenants per batch. Default: enough to finish within the window.
    pub batch_size: Option<i64>,
}

/// What a retry did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retried {
    /// A failed tenant of the open rollout is queued again.
    InRollout { rollout: i64 },
    /// A refused move (§2.1) is open again for the catch-up.
    Move { to: String },
}

// ── the refusal ──────────────────────────────────────────────────────────

/// The one error of [`Rollouts`] (rule 9).
#[derive(Debug)]
#[non_exhaustive]
pub enum RolloutRefusal {
    /// A second deployment-wide rollout (Q3).
    Open { id: i64, target: String },
    /// A control with no open rollout.
    NoneOpen { what: &'static str },
    /// No rollout under this id.
    NoSuch { id: i64 },
    /// `resume` of a rollout that is not paused.
    NotPaused { id: i64, state: RolloutState },
    /// The target is active already and every tenant is on it.
    Nothing { version: String },
    /// The deployment's own database does not hold the target yet.
    DeploymentBehind {
        version: String,
        from: String,
        why: String,
    },
    /// `versions.served` leaves out the active version, which tenants that
    /// wait, fail or are skipped stay on.
    Stranded {
        version: String,
        list: Vec<String>,
        tenants: u64,
    },
    /// A setting's value, or a start option, that is not one.
    Setting {
        setting: String,
        value: String,
        why: String,
    },
    /// The tenant is not in the open rollout, or not in a state the
    /// control acts on.
    Tenant { tenant: String, why: String },
    /// A reason that is empty or too long.
    Reason(String),
    /// What the pins or stored versions refused.
    Version(VersionRefusal),
    /// The database.
    Store(PoolError),
}

impl From<PoolError> for RolloutRefusal {
    fn from(e: PoolError) -> RolloutRefusal {
        RolloutRefusal::Store(e)
    }
}

impl From<VersionRefusal> for RolloutRefusal {
    fn from(e: VersionRefusal) -> RolloutRefusal {
        match e {
            VersionRefusal::Store(p) => RolloutRefusal::Store(p),
            VersionRefusal::RolloutOpen { id, target } => RolloutRefusal::Open { id, target },
            other => RolloutRefusal::Version(other),
        }
    }
}

impl RolloutRefusal {
    pub fn remedy(&self) -> Remedy {
        match self {
            RolloutRefusal::Version(v) => v.remedy(),
            RolloutRefusal::Store(e) => match e.class() {
                FaultClass::Busy | FaultClass::Retryable | FaultClass::Broken => Remedy::Retry,
                _ => Remedy::Refuse,
            },
            RolloutRefusal::Setting { .. }
            | RolloutRefusal::Reason(_)
            | RolloutRefusal::NoSuch { .. }
            | RolloutRefusal::Tenant { .. } => Remedy::FixInput,
            _ => Remedy::Refuse,
        }
    }

    /// **The one place a rollout refusal becomes a door answer.**
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            RolloutRefusal::Version(v) => v.to_exec_error(what),
            RolloutRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
            RolloutRefusal::Setting { .. } | RolloutRefusal::Reason(_) => {
                ExecError::new(ErrorCode::InvalidParam, message)
            }
            RolloutRefusal::NoSuch { .. }
            | RolloutRefusal::NoneOpen { .. }
            | RolloutRefusal::Tenant { .. } => ExecError::new(ErrorCode::NotFound, message),
            RolloutRefusal::Open { .. }
            | RolloutRefusal::NotPaused { .. }
            | RolloutRefusal::Nothing { .. }
            | RolloutRefusal::DeploymentBehind { .. }
            | RolloutRefusal::Stranded { .. } => ExecError::new(ErrorCode::ApplyRefused, message),
        }
    }
}

impl std::fmt::Display for RolloutRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RolloutRefusal::Open { id, target } => write!(
                f,
                "rollout_open: rollout {id} to {target} is open, and one deployment-wide \
                 rollout runs at a time. `ordo rollout status` shows where it stands; `ordo \
                 rollout finish` or `ordo rollout abort` closes it. One tenant may still move \
                 on its own: `ordo versions pin <tenant> <version>`"
            ),
            RolloutRefusal::NoneOpen { what } => write!(
                f,
                "no_rollout: no rollout is open, so there is nothing to {what}. `ordo rollout \
                 status` shows the newest"
            ),
            RolloutRefusal::NoSuch { id } => write!(
                f,
                "no_rollout: there is no rollout {id}. `ordo rollout status` shows the newest"
            ),
            RolloutRefusal::NotPaused { id, state } => write!(
                f,
                "not_paused: rollout {id} is {}, so there is nothing to resume",
                state.as_str()
            ),
            RolloutRefusal::Nothing { version } => write!(
                f,
                "nothing_to_roll_out: {version} is the active version and every tenant is on \
                 it"
            ),
            RolloutRefusal::DeploymentBehind { version, from, why } => write!(
                f,
                "deployment_behind: the deployment's own database does not hold {version} \
                 yet ({why}). Bring it there first: `ordo evolve --from {from} --to {version} \
                 --run`, or `ordo apply` the program"
            ),
            RolloutRefusal::Stranded {
                version,
                list,
                tenants,
            } => write!(
                f,
                "stranded: {} `{}` leaves out {version}, the active version, which {tenants} \
                 tenant(s) are on. A tenant that waits, fails or is skipped stays on it and \
                 would not be served. Add it first: `ordo versions set served {},{version}`, \
                 or `ordo versions set served in-use`",
                crate::versions::SERVED_SETTING,
                list.join(","),
                list.join(",")
            ),
            RolloutRefusal::Setting {
                setting,
                value,
                why,
            } => write!(
                f,
                "{setting} `{value}` is not one: {why}. `ordo rollout set {setting} <value> \
                 --reason \"...\"` sets it"
            ),
            RolloutRefusal::Tenant { tenant, why } => {
                write!(f, "tenant `{tenant}`: {why}")
            }
            RolloutRefusal::Reason(why) => write!(f, "reason refused: {why}"),
            RolloutRefusal::Version(v) => write!(f, "{v}"),
            RolloutRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RolloutRefusal {}

// ── the client ───────────────────────────────────────────────────────────

/// The client. Borrowed over the control database's pool.
#[derive(Clone, Copy)]
pub struct Rollouts<'a> {
    pool: &'a Pool,
}

fn pin_lock_sql() -> String {
    format!(
        "SELECT pg_advisory_xact_lock({}, 0)",
        crate::versions::PIN_LOCK_NS
    )
}

fn reason_of(text: Option<&str>) -> Result<Option<String>, RolloutRefusal> {
    match text.map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(t) => crate::versions::check_reason(t)
            .map(Some)
            .map_err(|e| RolloutRefusal::Reason(e.to_string())),
    }
}

impl<'a> Rollouts<'a> {
    pub fn on(pool: &'a Pool) -> Rollouts<'a> {
        Rollouts { pool }
    }

    /// Create the three tables. Idempotent; retried once, as `$pins` is,
    /// because `IF NOT EXISTS` is not race-free between 2 booting nodes.
    pub async fn ensure(&self) -> Result<(), RolloutRefusal> {
        Versions::on(self.pool).ensure().await?;
        if self.pool.rows(DDL).await.is_err() {
            self.pool.rows(DDL).await?;
        }
        Ok(())
    }

    // ── settings ────────────────────────────────────────────────────────

    /// The rollout settings as stored. Unset ones read as their defaults.
    pub async fn settings(&self) -> Result<RolloutSettings, RolloutRefusal> {
        let rows = match self
            .pool
            .rows(
                "SELECT DISTINCT ON (\"setting\") \"setting\", \"value\" \
                 FROM \"$version_settings\" WHERE \"setting\" LIKE 'rollout.%' \
                 ORDER BY \"setting\", \"seq\" DESC",
            )
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_missing_table() => return Ok(RolloutSettings::default()),
            Err(e) => return Err(e.into()),
        };
        let mut out = RolloutSettings::default();
        for r in rows {
            let (Some(Some(name)), value) = (r.first().cloned(), r.get(1).cloned().flatten())
            else {
                continue;
            };
            out.with(&name, value.as_deref());
        }
        Ok(out)
    }

    /// **Change one rollout setting**, or clear it with `None`, with who
    /// and why (Q15). Checked first ([`check_setting`]). The row is written
    /// beside the version settings, under the pin lock. `true` when it
    /// changed.
    pub async fn set_setting(
        &self,
        setting: &str,
        value: Option<&str>,
        actor: &str,
        reason: &str,
        at: u64,
    ) -> Result<(bool, RolloutSettings), RolloutRefusal> {
        let reason = crate::versions::check_reason(reason)
            .map_err(|e| RolloutRefusal::Reason(e.to_string()))?;
        let Some(name) = setting_name(setting) else {
            return Err(RolloutRefusal::Setting {
                setting: setting.chars().take(60).collect(),
                value: value.unwrap_or_default().chars().take(40).collect(),
                why: format!(
                    "there is no such rollout setting; there are {}",
                    SETTINGS.join(", ")
                ),
            });
        };
        let value = check_setting(name, value)?;
        self.ensure().await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        let now = tx
            .rows(&format!(
                "SELECT \"value\" FROM \"$version_settings\" WHERE \"setting\" = {} \
                 ORDER BY \"seq\" DESC LIMIT 1",
                sql_lit(name)
            ))
            .await?;
        let current = now.first().and_then(|r| r.first().cloned().flatten());
        if current == value && (value.is_some() || !now.is_empty()) {
            tx.rollback().await?;
            return Ok((false, self.settings().await?));
        }
        if value.is_none() && now.is_empty() {
            tx.rollback().await?;
            return Ok((false, self.settings().await?));
        }
        tx.rows(&format!(
            "INSERT INTO \"$version_settings\" (\"setting\", \"value\", \"actor\", \"reason\", \"at\") \
             VALUES ({}, {}, {}, {}, {at})",
            sql_lit(name),
            value.as_deref().map(sql_lit).unwrap_or_else(|| "NULL".into()),
            sql_lit(actor),
            sql_lit(&reason),
        ))
        .await?;
        tx.commit().await?;
        Ok((true, self.settings().await?))
    }

    // ── reading ─────────────────────────────────────────────────────────

    async fn one(&self, filter: &str) -> Result<Option<Rollout>, RolloutRefusal> {
        let sql =
            format!("SELECT {COLUMNS} FROM \"$rollouts\" {filter} ORDER BY \"id\" DESC LIMIT 1");
        match self.pool.rows(&sql).await {
            Ok(rows) => Ok(rows.first().and_then(|r| rollout_row(r))),
            Err(e) if e.is_missing_table() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The open rollout, running or paused.
    pub async fn open(&self) -> Result<Option<Rollout>, RolloutRefusal> {
        self.one("WHERE \"state\" IN ('running', 'paused')").await
    }

    /// One rollout by id.
    pub async fn get(&self, id: i64) -> Result<Option<Rollout>, RolloutRefusal> {
        self.one(&format!("WHERE \"id\" = {id}")).await
    }

    /// The newest rollout, open or not.
    pub async fn latest(&self) -> Result<Option<Rollout>, RolloutRefusal> {
        self.one("").await
    }

    /// The rows of rollout `id`, the deployment's own step included.
    pub async fn tenants(&self, id: i64) -> Result<Vec<RolloutTenant>, RolloutRefusal> {
        let rows = match self
            .pool
            .rows(&format!(
                "SELECT \"space\", \"ord\", \"state\", \"reason\", \"at\" FROM \"$rollout_tenants\" \
                 WHERE \"rollout\" = {id} ORDER BY \"ord\", \"space\""
            ))
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_missing_table() => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        Ok(rows.iter().filter_map(|r| tenant_row(r)).collect())
    }

    /// One tenant's row in rollout `id`.
    pub async fn tenant(
        &self,
        id: i64,
        space: &str,
    ) -> Result<Option<RolloutTenant>, RolloutRefusal> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"space\", \"ord\", \"state\", \"reason\", \"at\" FROM \"$rollout_tenants\" \
                 WHERE \"rollout\" = {id} AND \"space\" = {}",
                sql_lit(space)
            ))
            .await?;
        Ok(rows.first().and_then(|r| tenant_row(r)))
    }

    /// **Where a rollout stands**: `id`, else the open one, else the
    /// newest. `None` when there has never been one.
    pub async fn status(&self, id: Option<i64>) -> Result<Option<Status>, RolloutRefusal> {
        let rollout = match id {
            Some(id) => match self.get(id).await? {
                Some(r) => r,
                None => return Err(RolloutRefusal::NoSuch { id }),
            },
            None => match self.open().await? {
                Some(r) => r,
                None => match self.latest().await? {
                    Some(r) => r,
                    None => return Ok(None),
                },
            },
        };
        let all = self.tenants(rollout.id).await?;
        let mut counts: BTreeMap<&'static str, u64> =
            TenantState::ALL.iter().map(|s| (s.as_str(), 0)).collect();
        let mut deployment = None;
        let mut tenants = Vec::new();
        for t in all {
            if t.space == DEPLOYMENT_STEP {
                deployment = Some(t);
                continue;
            }
            *counts.entry(t.state.as_str()).or_default() += 1;
            tenants.push(t);
        }
        // Failed and waiting first: what the deployment owner acts on.
        tenants.sort_by_key(|t| {
            (
                match t.state {
                    TenantState::Failed => 0,
                    TenantState::Waiting => 1,
                    TenantState::Moving => 2,
                    TenantState::Queued => 3,
                    TenantState::Skipped => 4,
                    TenantState::Moved => 5,
                },
                t.ord,
            )
        });
        tenants.truncate(MAX_LISTED);
        Ok(Some(Status {
            rollout,
            counts,
            tenants,
            deployment,
        }))
    }

    /// Every hold, by tenant.
    pub async fn holds(&self) -> Result<Vec<Hold>, RolloutRefusal> {
        let rows = match self
            .pool
            .rows(
                "SELECT \"space\", \"actor\", \"reason\", \"at\" FROM \"$rollout_holds\" \
                 ORDER BY \"space\"",
            )
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_missing_table() => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some(Hold {
                    space: r.first()?.clone()?,
                    actor: r.get(1)?.clone()?,
                    reason: r.get(2)?.clone()?,
                    at: r.get(3)?.clone()?.parse().ok()?,
                })
            })
            .collect())
    }

    // ── the controls ────────────────────────────────────────────────────

    /// **Start a rollout of `name`** (a stored version, by name or `#n`).
    ///
    /// Refused: another rollout open (Q3); a version not stored, revoked,
    /// not through the promotion gate, or below `versions.minimum`; a
    /// `versions.served` list that leaves out the active version (a tenant
    /// that waits, fails or is skipped would be stranded on it); the
    /// deployment's own database not holding the version; nothing to move.
    pub async fn start(
        &self,
        name: &str,
        opts: &StartOptions,
        actor: &str,
        at: u64,
    ) -> Result<Rollout, RolloutRefusal> {
        self.ensure().await?;
        let v = Versions::on(self.pool);
        let target = v.resolve(name).await?;
        if let Some(mark) = v.revocation_of(&target).await? {
            return Err(VersionRefusal::Revoked {
                named: v.named(&target).await?,
                mark: Box::new(mark),
                setting: None,
            }
            .into());
        }
        v.require_arrival(&target).await?;
        let settings = self.settings().await?;
        let stored = v.settings().await?;
        if v.below_minimum().await?.contains(&target) {
            return Err(VersionRefusal::BelowMinimum {
                named: v.named(&target).await?,
                minimum: v
                    .named(stored.policy.minimum.as_deref().unwrap_or_default())
                    .await?,
            }
            .into());
        }
        let active = v.active_version().await?;
        let per_version = v.tenants_per_version().await?;
        if let (Some(list), Some(active)) = (&stored.policy.served, &active) {
            if active != &target && !list.contains(active) {
                return Err(RolloutRefusal::Stranded {
                    version: active.clone(),
                    list: list.clone(),
                    tenants: per_version.get(active).copied().unwrap_or(0),
                });
            }
        }
        let off_target: u64 = per_version
            .iter()
            .filter(|(ver, _)| *ver != &target)
            .map(|(_, n)| *n)
            .sum();
        if active.as_deref() == Some(target.as_str()) && off_target == 0 {
            return Err(RolloutRefusal::Nothing { version: target });
        }
        // The deployment's own database holds every table the version
        // asserts: the tenants on it are pinned with no expand of their own.
        let artifact = v
            .get(&target)
            .await?
            .ok_or_else(|| VersionRefusal::UnknownVersion(target.clone()))?;
        let program =
            Program::from_artifact(&artifact).map_err(|why| RolloutRefusal::DeploymentBehind {
                version: target.clone(),
                from: active.clone().unwrap_or_default(),
                why: format!("it does not compile here: {why}"),
            })?;
        match sa::schema_state(self.pool, &program.schema).await {
            Ok(sa::SchemaState::Matches { .. }) => {}
            Ok(other) => {
                return Err(RolloutRefusal::DeploymentBehind {
                    version: target,
                    from: active.unwrap_or_default(),
                    why: format!("{other:?}"),
                })
            }
            Err(e) => return Err(RolloutRefusal::Store(PoolError::db(e))),
        }

        let strategy = opts
            .strategy
            .or(settings.strategy)
            .unwrap_or(Strategy::Rolling);
        let check = |setting: &str, n: Option<i64>| -> Result<(), RolloutRefusal> {
            match n {
                Some(n) if n < 1 => Err(RolloutRefusal::Setting {
                    setting: setting.to_string(),
                    value: n.to_string(),
                    why: "it is 1 or more".into(),
                }),
                _ => Ok(()),
            }
        };
        check(BATCH_INTERVAL_SETTING, opts.batch_interval)?;
        check(WINDOW_SETTING, opts.window)?;
        check(FAILURE_THRESHOLD_SETTING, opts.failure_threshold)?;
        check("batchSize", opts.batch_size)?;
        let batch_interval = opts.batch_interval.unwrap_or(settings.batch_interval);
        let window = opts.window.unwrap_or(settings.window);
        let failure_threshold = opts.failure_threshold.unwrap_or(settings.failure_threshold);
        let deployment_space = opts.deployment_space.unwrap_or(settings.deployment_space);
        let batch_size = match (strategy, opts.batch_size) {
            (Strategy::All, _) => i64::MAX / 4,
            (Strategy::Rolling, Some(n)) => n,
            (Strategy::Rolling, None) => {
                // Enough per batch to finish within the window: a target,
                // never a deadline (§1.5).
                let batches = (window / batch_interval.max(1)).max(1);
                let total = off_target.max(1) as i64;
                ((total + batches - 1) / batches).max(1)
            }
        };

        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some((id, target)) = crate::versions::open_rollout_in(&tx).await? {
            tx.rollback().await?;
            return Err(RolloutRefusal::Open { id, target });
        }
        let head = tx
            .rows("SELECT coalesce(max(\"seq\"), 0) FROM \"$pins\"")
            .await?
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let id = tx
            .rows(&format!(
                "INSERT INTO \"$rollouts\" (\"target\", \"from\", \"strategy\", \"batch_interval\", \
                   \"window\", \"batch_size\", \"failure_threshold\", \"deployment_space\", \
                   \"state\", \"actor\", \"started_at\", \"pin_seq\") \
                 VALUES ({}, {}, {}, {batch_interval}, {window}, {batch_size}, \
                   {failure_threshold}, {}, 'running', {}, {at}, {head}) RETURNING \"id\"",
                sql_lit(&target),
                active.as_deref().map(sql_lit).unwrap_or_else(|| "NULL".into()),
                sql_lit(strategy.as_str()),
                sql_lit(deployment_space.as_str()),
                sql_lit(actor),
            ))
            .await?
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|v| v.parse::<i64>().ok())
            .ok_or_else(|| PoolError::db("the rollout's id was not returned"))?;
        if active.as_deref() != Some(target.as_str()) {
            tx.rows(&format!(
                "INSERT INTO \"$rollout_tenants\" (\"rollout\", \"space\", \"ord\", \"state\", \"at\") \
                 VALUES ({id}, {}, 0, 'queued', {at})",
                sql_lit(DEPLOYMENT_STEP)
            ))
            .await?;
        }
        tx.rows(&format!(
            "INSERT INTO \"$rollout_tenants\" (\"rollout\", \"space\", \"ord\", \"state\", \"at\") \
             SELECT {id}, \"$space\", row_number() OVER (ORDER BY \"$space\"), 'queued', {at} \
             FROM \"$spaces\" \
             WHERE \"state\" IN ('provisioning', 'active', 'paused') \
               AND \"version\" IS NOT NULL AND \"version\" <> {}",
            sql_lit(&target)
        ))
        .await?;
        tx.commit().await?;
        self.get(id).await?.ok_or(RolloutRefusal::NoSuch { id })
    }

    async fn set_state(
        &self,
        id: i64,
        state: RolloutState,
        reason: &str,
        at: u64,
        extra: &str,
    ) -> Result<(), RolloutRefusal> {
        let ended = if state.is_open() {
            "NULL".to_string()
        } else {
            at.to_string()
        };
        self.pool
            .rows(&format!(
                "UPDATE \"$rollouts\" SET \"state\" = {}, \"reason\" = {}, \"ended_at\" = {ended}{extra} \
                 WHERE \"id\" = {id}",
                sql_lit(state.as_str()),
                sql_lit(reason),
            ))
            .await?;
        Ok(())
    }

    async fn set_tenant(
        &self,
        id: i64,
        space: &str,
        state: TenantState,
        reason: &str,
        at: u64,
    ) -> Result<(), RolloutRefusal> {
        self.pool
            .rows(&format!(
                "UPDATE \"$rollout_tenants\" SET \"state\" = {}, \"reason\" = {}, \"at\" = {at} \
                 WHERE \"rollout\" = {id} AND \"space\" = {}",
                sql_lit(state.as_str()),
                sql_lit(reason),
                sql_lit(space),
            ))
            .await?;
        Ok(())
    }

    async fn count(&self, id: i64, state: TenantState) -> Result<i64, RolloutRefusal> {
        Ok(self
            .pool
            .rows(&format!(
                "SELECT count(*) FROM \"$rollout_tenants\" WHERE \"rollout\" = {id} \
                 AND \"state\" = {} AND \"space\" <> {}",
                sql_lit(state.as_str()),
                sql_lit(DEPLOYMENT_STEP)
            ))
            .await?
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// **Resume a paused rollout.** The threshold counts failures from now.
    pub async fn resume(&self, at: u64) -> Result<Rollout, RolloutRefusal> {
        let r = self
            .open()
            .await?
            .ok_or(RolloutRefusal::NoneOpen { what: "resume" })?;
        if r.state != RolloutState::Paused {
            return Err(RolloutRefusal::NotPaused {
                id: r.id,
                state: r.state,
            });
        }
        let failed = self.count(r.id, TenantState::Failed).await?;
        self.set_state(
            r.id,
            RolloutState::Running,
            "",
            at,
            &format!(", \"failed_base\" = {failed}"),
        )
        .await?;
        let _ = at;
        self.get(r.id)
            .await?
            .ok_or(RolloutRefusal::NoSuch { id: r.id })
    }

    /// **Skip one tenant** of the open rollout: it stays on its version.
    pub async fn skip(
        &self,
        tenant: &str,
        actor: &str,
        reason: Option<&str>,
        at: u64,
    ) -> Result<(Rollout, RolloutTenant), RolloutRefusal> {
        let reason = reason_of(reason)?;
        let r = self
            .open()
            .await?
            .ok_or(RolloutRefusal::NoneOpen { what: "skip" })?;
        let row = self.in_rollout(&r, tenant).await?;
        if !(row.state.is_pending() || row.state == TenantState::Failed) {
            return Err(RolloutRefusal::Tenant {
                tenant: tenant.to_string(),
                why: format!(
                    "it is {} in rollout {}, so there is nothing to skip",
                    row.state.as_str(),
                    r.id
                ),
            });
        }
        let why = match reason {
            Some(t) => format!("skipped by {actor}: {t}"),
            None => format!("skipped by {actor}"),
        };
        self.set_tenant(r.id, tenant, TenantState::Skipped, &why, at)
            .await?;
        let row = self.tenant(r.id, tenant).await?.unwrap_or(row);
        Ok((r, row))
    }

    async fn in_rollout(&self, r: &Rollout, tenant: &str) -> Result<RolloutTenant, RolloutRefusal> {
        match self.tenant(r.id, tenant).await? {
            Some(t) if t.space != DEPLOYMENT_STEP => Ok(t),
            _ => Err(RolloutRefusal::Tenant {
                tenant: tenant.to_string(),
                why: format!(
                    "it is not in rollout {} to {}. `ordo rollout status` lists its tenants",
                    r.id, r.target
                ),
            }),
        }
    }

    /// **Try one tenant again**: a failed tenant of the open rollout goes
    /// back in the queue; with no rollout holding it, a tenant's refused
    /// move (§2.1) is opened again for the catch-up (increment 7 left it
    /// retried only by a new version).
    pub async fn retry(&self, tenant: &str, at: u64) -> Result<Retried, RolloutRefusal> {
        if let Some(r) = self.open().await? {
            if let Some(row) = self.tenant(r.id, tenant).await? {
                if row.space != DEPLOYMENT_STEP {
                    if row.state != TenantState::Failed {
                        return Err(RolloutRefusal::Tenant {
                            tenant: tenant.to_string(),
                            why: format!(
                                "it is {} in rollout {}; only a failed tenant is retried",
                                row.state.as_str(),
                                r.id
                            ),
                        });
                    }
                    self.set_tenant(r.id, tenant, TenantState::Queued, "", at)
                        .await?;
                    return Ok(Retried::InRollout { rollout: r.id });
                }
            }
        }
        match Versions::on(self.pool).retry_move(tenant).await? {
            Some(m) => Ok(Retried::Move { to: m.to }),
            None => Err(RolloutRefusal::Tenant {
                tenant: tenant.to_string(),
                why: "it has no failed rollout move and no refused move to try again. \
                      `ordo rollout status` and `std.deployment.tenants` show where it stands"
                    .into(),
            }),
        }
    }

    /// **Stop the open rollout.** No tenant moves after this; moved tenants
    /// stay, and `ordo rollback` is what moves them back (§3.7).
    pub async fn abort(&self, reason: &str, at: u64) -> Result<Rollout, RolloutRefusal> {
        let reason = crate::versions::check_reason(reason)
            .map_err(|e| RolloutRefusal::Reason(e.to_string()))?;
        let r = self
            .open()
            .await?
            .ok_or(RolloutRefusal::NoneOpen { what: "abort" })?;
        self.set_state(r.id, RolloutState::Aborted, &reason, at, "")
            .await?;
        self.get(r.id)
            .await?
            .ok_or(RolloutRefusal::NoSuch { id: r.id })
    }

    /// **Close the open rollout by hand**, every tenant still to move
    /// skipped. The deployment's own space moves now when it has not, which
    /// makes the target active. Refused when `versions.served` would leave
    /// a skipped tenant on a version it no longer serves.
    pub async fn finish(
        &self,
        actor: &str,
        reason: Option<&str>,
        at: u64,
    ) -> Result<Rollout, RolloutRefusal> {
        let reason = reason_of(reason)?;
        let r = self
            .open()
            .await?
            .ok_or(RolloutRefusal::NoneOpen { what: "finish" })?;
        let v = Versions::on(self.pool);
        if let Some(list) = v.settings().await?.policy.served {
            for (version, tenants) in v.tenants_per_version().await? {
                if version != r.target && !list.contains(&version) {
                    return Err(RolloutRefusal::Stranded {
                        version,
                        list,
                        tenants,
                    });
                }
            }
        }
        let why = match &reason {
            Some(t) => format!("finished by hand by {actor}: {t}"),
            None => format!("finished by hand by {actor}"),
        };
        self.pool
            .rows(&format!(
                "UPDATE \"$rollout_tenants\" SET \"state\" = 'skipped', \"reason\" = {}, \
                   \"at\" = {at} \
                 WHERE \"rollout\" = {} AND \"space\" <> {} \
                   AND \"state\" IN ('queued', 'waiting', 'moving', 'failed')",
                sql_lit(&why),
                r.id,
                sql_lit(DEPLOYMENT_STEP)
            ))
            .await?;
        self.deployment_step(&r, actor, at).await?;
        self.set_state(r.id, RolloutState::Finished, &why, at, "")
            .await?;
        self.get(r.id)
            .await?
            .ok_or(RolloutRefusal::NoSuch { id: r.id })
    }

    /// **Hold one tenant's moves**, with a reason, or lift the hold with
    /// `None`. A held tenant waits in every rollout until it is lifted.
    pub async fn hold(
        &self,
        tenant: &str,
        reason: Option<&str>,
        actor: &str,
        at: u64,
    ) -> Result<Option<Hold>, RolloutRefusal> {
        self.ensure().await?;
        match reason {
            None => {
                self.pool
                    .rows(&format!(
                        "DELETE FROM \"$rollout_holds\" WHERE \"space\" = {}",
                        sql_lit(tenant)
                    ))
                    .await?;
                Ok(None)
            }
            Some(text) => {
                let reason = crate::versions::check_reason(text)
                    .map_err(|e| RolloutRefusal::Reason(e.to_string()))?;
                if Versions::on(self.pool).pin_of(tenant).await?.is_none() {
                    return Err(VersionRefusal::NoTenant {
                        space: tenant.to_string(),
                    }
                    .into());
                }
                self.pool
                    .rows(&format!(
                        "INSERT INTO \"$rollout_holds\" (\"space\", \"actor\", \"reason\", \"at\") \
                         VALUES ({t}, {a}, {r}, {at}) ON CONFLICT (\"space\") DO UPDATE \
                         SET \"actor\" = {a}, \"reason\" = {r}, \"at\" = {at}",
                        t = sql_lit(tenant),
                        a = sql_lit(actor),
                        r = sql_lit(&reason),
                    ))
                    .await?;
                Ok(Some(Hold {
                    space: tenant.to_string(),
                    actor: actor.to_string(),
                    reason,
                    at,
                }))
            }
        }
    }

    /// Move the deployment's own space, when the rollout has that step and
    /// it has not moved: the activation that makes the target active.
    async fn deployment_step(
        &self,
        r: &Rollout,
        actor: &str,
        at: u64,
    ) -> Result<bool, RolloutRefusal> {
        let Some(step) = self.tenant(r.id, DEPLOYMENT_STEP).await? else {
            return Ok(false);
        };
        if step.state == TenantState::Moved {
            return Ok(false);
        }
        Versions::on(self.pool)
            .activate_alone(&r.target, actor, Timestamp(at))
            .await?;
        self.set_tenant(r.id, DEPLOYMENT_STEP, TenantState::Moved, "", at)
            .await?;
        Ok(true)
    }
}

// ── the worker ───────────────────────────────────────────────────────────

/// What one [`tick`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Ticked {
    /// Another node holds the lock: it leads this tick.
    pub not_leader: bool,
    /// The rollout this tick worked on.
    pub rollout: Option<i64>,
    /// The deployment's own space moved in this tick.
    pub deployment_moved: bool,
    pub moved: Vec<String>,
    /// `(tenant, why)`.
    pub waiting: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
    /// `(tenant, why)`: moved on their own, or gone.
    pub skipped: Vec<(String, String)>,
    /// The threshold paused it in this tick.
    pub paused: bool,
    pub finished: bool,
}

impl Ticked {
    /// One line per thing that happened, for the node's log.
    pub fn log_lines(&self) -> Vec<String> {
        let Some(id) = self.rollout else {
            return vec![];
        };
        let mut out = Vec::new();
        if self.deployment_moved {
            out.push(format!(
                "rollout {id}: the deployment's own space moved; nodes load the version"
            ));
        }
        for t in &self.moved {
            out.push(format!("rollout {id}: tenant {t} moved"));
        }
        for (t, why) in &self.waiting {
            out.push(format!("rollout {id}: tenant {t} waits: {why}"));
        }
        for (t, why) in &self.failed {
            out.push(format!(
                "rollout {id}: tenant {t} failed and keeps serving its version: {why}"
            ));
        }
        for (t, why) in &self.skipped {
            out.push(format!("rollout {id}: tenant {t} skipped: {why}"));
        }
        if self.paused {
            out.push(format!(
                "rollout {id}: paused by {FAILURE_THRESHOLD_SETTING}; `ordo rollout status`"
            ));
        }
        if self.finished {
            out.push(format!("rollout {id}: finished"));
        }
        out
    }
}

/// How one tenant's move ended.
enum Outcome {
    Moved,
    Waiting(String),
    Failed(String),
    Gone(String),
}

/// **One turn of the leading worker** (§3.7). Every node calls it every
/// [`TICK_ENV`] seconds; one leads each turn.
///
/// 1. Take the rollout lock for this turn, or do nothing.
/// 2. Read the open rollout; a paused one waits for `resume`.
/// 3. Add tenants made since it started, and skip tenants a person moved
///    on their own (a hotfix, Q3).
/// 4. Move the deployment's own space when it goes first.
/// 5. When a batch is due, move the next tenants that may move now: a held
///    tenant or one outside its window waits, and does not count.
/// 6. Pause at the failure threshold; finish when nothing is left.
pub async fn tick(ops: &ApplyOps, node: &str, now: u64) -> Result<Ticked, RolloutRefusal> {
    let pool = &ops.pool;
    let mut out = Ticked::default();
    let there = pool
        .rows("SELECT to_regclass('\"$rollouts\"') IS NOT NULL")
        .await?
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .is_some_and(|v| v == "t" || v == "true");
    if !there {
        return Ok(out);
    }
    let lock = pool.begin().await?;
    let led = lock
        .rows(&format!(
            "SELECT pg_try_advisory_xact_lock({ROLLOUT_LOCK_NS}, 0)"
        ))
        .await?
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .is_some_and(|v| v == "t" || v == "true");
    if !led {
        let _ = lock.rollback().await;
        out.not_leader = true;
        return Ok(out);
    }
    let result = lead(ops, node, now, &mut out).await;
    let _ = lock.rollback().await;
    result.map(|_| out)
}

async fn lead(
    ops: &ApplyOps,
    node: &str,
    now: u64,
    out: &mut Ticked,
) -> Result<(), RolloutRefusal> {
    let pool = &ops.pool;
    let rollouts = Rollouts::on(pool);
    let Some(r) = rollouts.open().await? else {
        return Ok(());
    };
    out.rollout = Some(r.id);
    pool.rows(&format!(
        "UPDATE \"$rollouts\" SET \"leader\" = {}, \"led_at\" = {now} WHERE \"id\" = {}",
        sql_lit(node),
        r.id
    ))
    .await?;
    if r.state != RolloutState::Running {
        return Ok(());
    }

    // Tenants made since it started, and any still off the target: a
    // rollout reads its tenant set again each turn (§1.3).
    pool.rows(&format!(
        "INSERT INTO \"$rollout_tenants\" (\"rollout\", \"space\", \"ord\", \"state\", \"at\") \
         SELECT {id}, s.\"$space\", \
           (SELECT coalesce(max(\"ord\"), 0) FROM \"$rollout_tenants\" WHERE \"rollout\" = {id}) \
             + row_number() OVER (ORDER BY s.\"$space\"), 'queued', {now} \
         FROM \"$spaces\" s \
         WHERE s.\"state\" IN ('provisioning', 'active', 'paused') \
           AND s.\"version\" IS NOT NULL AND s.\"version\" <> {target} \
           AND NOT EXISTS (SELECT 1 FROM \"$rollout_tenants\" t \
             WHERE t.\"rollout\" = {id} AND t.\"space\" = s.\"$space\")",
        id = r.id,
        target = sql_lit(&r.target)
    ))
    .await?;

    // Where each pending tenant is now: on the target already (a catch-up
    // move, a new tenant), moved by a person on its own (a hotfix, Q3), or
    // gone.
    let rows = pool
        .rows(&format!(
            "SELECT t.\"space\", s.\"version\", s.\"state\", \
               (SELECT p.\"cause\" || ' ' || p.\"actor\" || ' ' || p.\"to\" FROM \"$pins\" p \
                 WHERE p.\"space\" = t.\"space\" AND p.\"seq\" > {pin_seq} \
                 ORDER BY p.\"seq\" DESC LIMIT 1) \
             FROM \"$rollout_tenants\" t \
             LEFT JOIN \"$spaces\" s ON s.\"$space\" = t.\"space\" \
             WHERE t.\"rollout\" = {id} AND t.\"space\" <> {step} \
               AND t.\"state\" IN ('queued', 'waiting', 'moving', 'failed')",
            pin_seq = r.pin_seq,
            id = r.id,
            step = sql_lit(DEPLOYMENT_STEP),
        ))
        .await?;
    for row in rows {
        let space = row.first().cloned().flatten().unwrap_or_default();
        let version = row.get(1).cloned().flatten();
        let state = row.get(2).cloned().flatten();
        let last = row.get(3).cloned().flatten().unwrap_or_default();
        let serving = matches!(state.as_deref(), Some("provisioning" | "active" | "paused"));
        if !serving {
            let why = "it no longer serves (reaped or failed)".to_string();
            rollouts
                .set_tenant(r.id, &space, TenantState::Skipped, &why, now)
                .await?;
            out.skipped.push((space, why));
        } else if version.as_deref() == Some(r.target.as_str()) {
            rollouts
                .set_tenant(r.id, &space, TenantState::Moved, "", now)
                .await?;
        } else if let Some(rest) = last.strip_prefix("pin ") {
            let (who, to) = rest.rsplit_once(' ').unwrap_or((rest, ""));
            let why = format!("moved on its own to {to} by {who}");
            rollouts
                .set_tenant(r.id, &space, TenantState::Skipped, &why, now)
                .await?;
            out.skipped.push((space, why));
        }
    }

    if r.deployment_space == DeploymentSpace::First {
        out.deployment_moved = rollouts.deployment_step(&r, ACTOR, now).await?;
    }

    let due = r.batches == 0
        || r.strategy == Strategy::All
        || now
            >= r.last_batch_at
                .saturating_add(r.batch_interval.max(0) as u64);
    if due {
        batch(ops, &r, now, out).await?;
    }

    // The threshold counts failures since the last resume.
    let failed = rollouts.count(r.id, TenantState::Failed).await?;
    if !out.failed.is_empty() && failed - r.failed_base >= r.failure_threshold {
        let names: Vec<String> = out.failed.iter().map(|(t, _)| t.clone()).collect();
        let why = format!(
            "{} tenant(s) failed, {FAILURE_THRESHOLD_SETTING} is {}: {}. `ordo rollout \
             retry <tenant>` or `ordo rollout skip <tenant>`, then `ordo rollout resume`",
            failed - r.failed_base,
            r.failure_threshold,
            names.join(", ")
        );
        rollouts
            .set_state(r.id, RolloutState::Paused, &why, now, "")
            .await?;
        ops.record(
            ACTOR,
            STATUS,
            "rollout-paused",
            format!(
                "rollout={} target={} failed={}",
                r.id,
                r.target,
                names.join(",")
            ),
        );
        out.paused = true;
        return Ok(());
    }

    // Finished when nothing is left to move: the deployment's own space
    // last when it goes last.
    let left: i64 = pool
        .rows(&format!(
            "SELECT count(*) FROM \"$rollout_tenants\" WHERE \"rollout\" = {} \
             AND \"space\" <> {} AND \"state\" IN ('queued', 'waiting', 'moving')",
            r.id,
            sql_lit(DEPLOYMENT_STEP)
        ))
        .await?
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if left == 0 {
        if r.deployment_space == DeploymentSpace::Last {
            out.deployment_moved |= rollouts.deployment_step(&r, ACTOR, now).await?;
        }
        let moved = rollouts.count(r.id, TenantState::Moved).await?;
        let failed = rollouts.count(r.id, TenantState::Failed).await?;
        let skipped = rollouts.count(r.id, TenantState::Skipped).await?;
        let why = format!("{moved} moved, {failed} failed, {skipped} skipped");
        rollouts
            .set_state(r.id, RolloutState::Finished, &why, now, "")
            .await?;
        ops.record(
            ACTOR,
            STATUS,
            "rollout-finished",
            format!("rollout={} target={} {why}", r.id, r.target),
        );
        out.finished = true;
    }
    Ok(())
}

/// Move the next batch of rollout `r`.
async fn batch(
    ops: &ApplyOps,
    r: &Rollout,
    now: u64,
    out: &mut Ticked,
) -> Result<(), RolloutRefusal> {
    let pool = &ops.pool;
    let rollouts = Rollouts::on(pool);
    let candidates: Vec<RolloutTenant> = {
        let mut c: Vec<RolloutTenant> = rollouts
            .tenants(r.id)
            .await?
            .into_iter()
            .filter(|t| t.space != DEPLOYMENT_STEP && t.state.is_pending())
            .collect();
        // A move a dead leader left half done first, then the waiting,
        // then the queue in order.
        c.sort_by_key(|t| {
            (
                match t.state {
                    TenantState::Moving => 0,
                    TenantState::Waiting => 1,
                    _ => 2,
                },
                t.ord,
            )
        });
        c
    };
    if candidates.is_empty() {
        return Ok(());
    }
    let holds: BTreeMap<String, Hold> = rollouts
        .holds()
        .await?
        .into_iter()
        .map(|h| (h.space.clone(), h))
        .collect();
    let windows = Windows::on(pool);
    let all_windows = windows.all().await.unwrap_or_default();
    let open: Vec<crate::windows::Window> = if all_windows.is_empty() {
        vec![]
    } else {
        windows
            .open_at(now as i64)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(w, _, _)| w)
            .collect()
    };
    let own: BTreeSet<String> = match ordo_exec::spacedb::all(pool).await {
        Ok(a) => a.into_iter().map(|(s, _)| s.0).collect(),
        Err(e) if e.is_missing_table() => BTreeSet::new(),
        Err(e) => return Err(e.into()),
    };
    let mut program: Option<Program> = None;
    let mut apply_lock: Option<Option<crate::apply_op::ApplyLock>> = None;
    let mut tried = 0i64;
    for t in candidates {
        if tried >= r.batch_size {
            break;
        }
        if let Some(h) = holds.get(&t.space) {
            let why = format!(
                "on hold by {}: {}. `ordo rollout hold {} --clear` lifts it",
                h.actor, h.reason, t.space
            );
            rollouts
                .set_tenant(r.id, &t.space, TenantState::Waiting, &why, now)
                .await?;
            out.waiting.push((t.space, why));
            continue;
        }
        let covered = all_windows.iter().any(|w| w.target.covers(&t.space));
        if covered && !open.iter().any(|w| w.target.covers(&t.space)) {
            let next = windows
                .next_for(&[t.space.as_str()], now as i64)
                .await
                .ok()
                .and_then(|m| m.get(&t.space).copied());
            let why = match next {
                Some(at) => format!(
                    "outside its maintenance window; the next opens {}",
                    crate::windows::format_utc(at)
                ),
                None => "outside its maintenance window, and none opens in the next 12 months. \
                         `ordo rollout skip` or `ordo windows set` moves it on"
                    .to_string(),
            };
            rollouts
                .set_tenant(r.id, &t.space, TenantState::Waiting, &why, now)
                .await?;
            out.waiting.push((t.space, why));
            continue;
        }
        tried += 1;
        rollouts
            .set_tenant(r.id, &t.space, TenantState::Moving, "", now)
            .await?;
        let outcome = if own.contains(&t.space) {
            if program.is_none() {
                let artifact = Versions::on(pool)
                    .get(&r.target)
                    .await?
                    .ok_or_else(|| VersionRefusal::UnknownVersion(r.target.clone()))?;
                program = Some(Program::from_artifact(&artifact).map_err(|why| {
                    RolloutRefusal::DeploymentBehind {
                        version: r.target.clone(),
                        from: r.from.clone().unwrap_or_default(),
                        why,
                    }
                })?);
            }
            if apply_lock.is_none() {
                apply_lock = Some(crate::apply_op::ApplyLock::take(pool).await.ok().flatten());
            }
            match apply_lock.as_ref().and_then(|l| l.as_ref()) {
                None => Outcome::Waiting(
                    "an apply holds the apply lock; it is tried again on the next turn".into(),
                ),
                Some(_) => {
                    move_own(
                        ops,
                        r,
                        &t.space,
                        program.as_ref().expect("compiled above"),
                        now,
                    )
                    .await?
                }
            }
        } else {
            pin(pool, r, &t.space, now).await?
        };
        match outcome {
            Outcome::Moved => {
                rollouts
                    .set_tenant(r.id, &t.space, TenantState::Moved, "", now)
                    .await?;
                out.moved.push(t.space);
            }
            Outcome::Waiting(why) => {
                rollouts
                    .set_tenant(r.id, &t.space, TenantState::Waiting, &why, now)
                    .await?;
                out.waiting.push((t.space, why));
            }
            Outcome::Failed(why) => {
                rollouts
                    .set_tenant(r.id, &t.space, TenantState::Failed, &why, now)
                    .await?;
                out.failed.push((t.space, why));
            }
            Outcome::Gone(why) => {
                rollouts
                    .set_tenant(r.id, &t.space, TenantState::Skipped, &why, now)
                    .await?;
                out.skipped.push((t.space, why));
            }
        }
    }
    if let Some(Some(l)) = apply_lock {
        l.release().await;
    }
    if tried > 0 {
        pool.rows(&format!(
            "UPDATE \"$rollouts\" SET \"last_batch_at\" = {now}, \"batches\" = \"batches\" + 1 \
             WHERE \"id\" = {}",
            r.id
        ))
        .await?;
    }
    Ok(())
}

/// Pin one tenant on the deployment's own database: that database holds
/// the target (checked at the start).
async fn pin(pool: &Pool, r: &Rollout, space: &str, now: u64) -> Result<Outcome, RolloutRefusal> {
    match Versions::on(pool)
        .pin_in_rollout(space, &r.target, r.id, ACTOR, now)
        .await
    {
        Ok(_) => Ok(Outcome::Moved),
        Err(VersionRefusal::NoTenant { .. }) => Ok(Outcome::Gone(
            "it no longer serves (reaped or failed)".into(),
        )),
        Err(VersionRefusal::Store(e)) if e.class() != FaultClass::Refused => Ok(Outcome::Waiting(
            format!("the control database did not answer: {e}"),
        )),
        Err(VersionRefusal::Store(e)) => Err(RolloutRefusal::Store(e)),
        Err(other) => Ok(Outcome::Failed(other.to_string())),
    }
}

/// Move one tenant whose database is its own: expand it to the target
/// (`schema_apply::bring_forward_from`), gate it, then pin it. A database
/// that cannot be reached waits (Q10); a change refused by name fails.
async fn move_own(
    ops: &ApplyOps,
    r: &Rollout,
    space: &str,
    program: &Program,
    now: u64,
) -> Result<Outcome, RolloutRefusal> {
    let id = ordo_crypto::SpaceId(space.to_string());
    let target = match ops.pools.resolve(&id).await {
        Ok(p) => p,
        Err(e) => {
            return Ok(Outcome::Waiting(format!(
                "its database cannot be reached ({e}); it is tried again"
            )))
        }
    };
    match sa::bring_forward_from(&target, Some(&ops.pool), program, ACTOR, START).await {
        Ok(_) => {}
        Err(e) if e.transient => {
            return Ok(Outcome::Waiting(format!(
                "its database cannot be reached ({}): {}; it is tried again",
                e.class, e.why
            )))
        }
        Err(e) => return Ok(Outcome::Failed(format!("{}: {}", e.class, e.why))),
    }
    match sa::schema_state(&target, &program.schema).await {
        Ok(sa::SchemaState::Matches { .. }) => {}
        Ok(other) => {
            return Ok(Outcome::Failed(format!(
                "still_behind: its database lacks part of {}: {other:?}",
                r.target
            )))
        }
        Err(e) => {
            return Ok(Outcome::Waiting(format!(
                "its database cannot be read ({e}); it is tried again"
            )))
        }
    }
    pin(&ops.pool, r, space, now).await
}

/// [`TICK_ENV`], or the default. A value that is not a whole number of
/// seconds from 1 is refused at boot, naming it.
pub fn tick_from_env() -> Result<std::time::Duration, String> {
    match std::env::var(TICK_ENV) {
        Err(_) => Ok(std::time::Duration::from_secs(TICK_DEFAULT)),
        Ok(v) if v.trim().is_empty() => Ok(std::time::Duration::from_secs(TICK_DEFAULT)),
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(n) if n >= 1 => Ok(std::time::Duration::from_secs(n)),
            _ => Err(format!(
                "{TICK_ENV} is `{v}`: it is a whole number of seconds, 1 or more. Set \
                 {TICK_ENV}={TICK_DEFAULT}, the default, or unset it"
            )),
        },
    }
}

// ── the door ─────────────────────────────────────────────────────────────

/// The specs of the 8 ops.
pub fn specs() -> Vec<OpSpec> {
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let int = |name: &str| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Int,
        required: false,
        bounds: None,
    };
    let flag = |name: &str| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Bool,
        required: false,
        bounds: None,
    };
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, fields: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: fields.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        reads_external: false,
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
    let rollout_fields: &[&str] = &[
        "id",
        "target",
        "from",
        "strategy",
        "batchInterval",
        "window",
        "batchSize",
        "failureThreshold",
        "deploymentSpace",
        "state",
        "reason",
        "actor",
        "startedAt",
        "endedAt",
        "lastBatchAt",
        "batches",
        "leader",
        "ledAt",
        "counts",
        "deploymentStep",
        "tenants",
    ];
    vec![
        spec(
            START,
            OpKind::Write,
            vec![
                text("version", 128, true),
                text("strategy", 32, false),
                text("batchInterval", 32, false),
                text("window", 32, false),
                int("failureThreshold"),
                text("deploymentSpace", 16, false),
                int("batchSize"),
            ],
            rollout_fields,
        ),
        spec(STATUS, OpKind::Read, vec![int("id")], rollout_fields),
        spec(RESUME, OpKind::Write, vec![], rollout_fields),
        spec(
            SKIP,
            OpKind::Write,
            vec![text("tenant", 128, true), text("reason", 4096, false)],
            &["rollout", "tenant", "state", "reason"],
        ),
        spec(
            RETRY,
            OpKind::Write,
            vec![text("tenant", 128, true)],
            &["tenant", "rollout", "move"],
        ),
        spec(
            ABORT,
            OpKind::Write,
            vec![text("reason", 4096, true)],
            rollout_fields,
        ),
        spec(
            FINISH,
            OpKind::Write,
            vec![text("reason", 4096, false)],
            rollout_fields,
        ),
        spec(
            HOLD,
            OpKind::Write,
            vec![
                text("tenant", 128, true),
                text("reason", 4096, false),
                flag("clear"),
            ],
            &["tenant", "held", "reason", "actor", "at"],
        ),
    ]
}

fn secs_param(bound: &BoundParams, key: &str, setting: &str) -> Result<Option<i64>, ExecError> {
    let Some(v) = bound.0.get(key) else {
        return Ok(None);
    };
    let text = match v {
        Value::Null => return Ok(None),
        Value::Number(n) => n.to_string(),
        Value::String(s) if s.trim().is_empty() => return Ok(None),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    match parse_secs(&text) {
        Some(n) if n >= 1 => Ok(Some(n)),
        _ => Err(RolloutRefusal::Setting {
            setting: setting.to_string(),
            value: text.chars().take(40).collect(),
            why: "it is a length of at least 1 second, like `30s`, `5m` or `1h`".into(),
        }
        .to_exec_error("the rollout was not started")),
    }
}

/// Run one of the 8. The caller is the deployment owner, or a machine key
/// with the `deployment` grant, on the home space (`deploy_ops::execute`
/// checked). No reauth (owner, 2026-11-05).
pub async fn execute(
    ops: &ApplyOps,
    name: &str,
    bound: &BoundParams,
    caller: &str,
) -> Result<Value, ExecError> {
    let text = |k: &str| {
        bound
            .0
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let int = |k: &str| bound.0.get(k).and_then(|v| v.as_i64());
    let pool = &ops.pool;
    let rollouts = Rollouts::on(pool);
    let now = crate::runtime::now_secs();
    let status_of = |id: i64| async move {
        Rollouts::on(pool)
            .status(Some(id))
            .await
            .map(|s| s.map(|s| s.to_json()).unwrap_or(Value::Null))
    };
    match name {
        START => {
            const WHAT: &str = "the rollout was not started";
            let strategy = match text("strategy") {
                None => None,
                Some(s) => Some(Strategy::parse(s).ok_or_else(|| {
                    RolloutRefusal::Setting {
                        setting: STRATEGY_SETTING.into(),
                        value: s.chars().take(40).collect(),
                        why: "it is `rolling` or `all`".into(),
                    }
                    .to_exec_error(WHAT)
                })?),
            };
            let deployment_space = match text("deploymentSpace") {
                None => None,
                Some(s) => Some(DeploymentSpace::parse(s).ok_or_else(|| {
                    RolloutRefusal::Setting {
                        setting: DEPLOYMENT_SPACE_SETTING.into(),
                        value: s.chars().take(40).collect(),
                        why: "it is `first` or `last`".into(),
                    }
                    .to_exec_error(WHAT)
                })?),
            };
            let opts = StartOptions {
                strategy,
                batch_interval: secs_param(bound, "batchInterval", BATCH_INTERVAL_SETTING)?,
                window: secs_param(bound, "window", WINDOW_SETTING)?,
                failure_threshold: int("failureThreshold"),
                deployment_space,
                batch_size: int("batchSize"),
            };
            let r = rollouts
                .start(text("version").unwrap_or_default(), &opts, caller, now)
                .await
                .map_err(|e| e.to_exec_error(WHAT))?;
            ops.record(
                caller,
                START,
                "rollout-started",
                format!(
                    "rollout={} target={} from={} strategy={} by={caller}",
                    r.id,
                    r.target,
                    r.from.as_deref().unwrap_or("none"),
                    r.strategy.as_str()
                ),
            );
            status_of(r.id)
                .await
                .map_err(|e| e.to_exec_error("the rollout started and could not be read"))
        }
        STATUS => {
            let s = rollouts
                .status(int("id"))
                .await
                .map_err(|e| e.to_exec_error("the rollout could not be read"))?;
            Ok(match s {
                Some(s) => s.to_json(),
                None => json!({ "id": null, "state": null, "tenants": [] }),
            })
        }
        RESUME => {
            let r = rollouts
                .resume(now)
                .await
                .map_err(|e| e.to_exec_error("the rollout was not resumed"))?;
            ops.record(
                caller,
                RESUME,
                "rollout-resumed",
                format!("rollout={} target={} by={caller}", r.id, r.target),
            );
            status_of(r.id)
                .await
                .map_err(|e| e.to_exec_error("the rollout could not be read"))
        }
        SKIP => {
            let tenant = text("tenant").unwrap_or_default();
            let (r, t) = rollouts
                .skip(tenant, caller, text("reason"), now)
                .await
                .map_err(|e| e.to_exec_error("the tenant was not skipped"))?;
            ops.record(
                caller,
                SKIP,
                "rollout-skipped",
                format!("rollout={} tenant={tenant} by={caller}", r.id),
            );
            Ok(json!({
                "rollout": r.id, "tenant": t.space, "state": t.state.as_str(), "reason": t.reason,
            }))
        }
        RETRY => {
            let tenant = text("tenant").unwrap_or_default();
            let done = rollouts
                .retry(tenant, now)
                .await
                .map_err(|e| e.to_exec_error("the tenant was not tried again"))?;
            let detail = match &done {
                Retried::InRollout { rollout } => format!("rollout={rollout} tenant={tenant}"),
                Retried::Move { to } => format!("move tenant={tenant} to={to}"),
            };
            ops.record(
                caller,
                RETRY,
                "rollout-retried",
                format!("{detail} by={caller}"),
            );
            Ok(match done {
                Retried::InRollout { rollout } => {
                    json!({ "tenant": tenant, "rollout": rollout, "move": null })
                }
                Retried::Move { to } => {
                    json!({ "tenant": tenant, "rollout": null, "move": { "to": to } })
                }
            })
        }
        ABORT => {
            let r = rollouts
                .abort(text("reason").unwrap_or_default(), now)
                .await
                .map_err(|e| e.to_exec_error("the rollout was not aborted"))?;
            ops.record(
                caller,
                ABORT,
                "rollout-aborted",
                format!("rollout={} target={} by={caller}", r.id, r.target),
            );
            status_of(r.id)
                .await
                .map_err(|e| e.to_exec_error("the rollout could not be read"))
        }
        FINISH => {
            let r = rollouts
                .finish(caller, text("reason"), now)
                .await
                .map_err(|e| e.to_exec_error("the rollout was not finished"))?;
            ops.record(
                caller,
                FINISH,
                "rollout-finished",
                format!("rollout={} target={} by={caller}", r.id, r.target),
            );
            status_of(r.id)
                .await
                .map_err(|e| e.to_exec_error("the rollout could not be read"))
        }
        HOLD => {
            let tenant = text("tenant").unwrap_or_default();
            let clear = bound
                .0
                .get("clear")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let reason = if clear {
                None
            } else {
                Some(text("reason").ok_or_else(|| {
                    RolloutRefusal::Reason(
                        "a hold says why: `--reason \"...\"`, or `--clear` lifts it".into(),
                    )
                    .to_exec_error("the tenant was not held")
                })?)
            };
            let held = rollouts
                .hold(tenant, reason, caller, now)
                .await
                .map_err(|e| e.to_exec_error("the hold was not changed"))?;
            ops.record(
                caller,
                HOLD,
                if held.is_some() {
                    "rollout-held"
                } else {
                    "rollout-unheld"
                },
                format!("tenant={tenant} by={caller}"),
            );
            Ok(match held {
                Some(h) => json!({
                    "tenant": h.space, "held": true, "reason": h.reason, "actor": h.actor,
                    "at": h.at,
                }),
                None => json!({ "tenant": tenant, "held": false, "reason": null, "actor": null,
                    "at": null }),
            })
        }
        other => Err(ExecError::internal(format!(
            "`{other}` is not a rollout op"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_is_named_three_ways_and_checked() {
        assert_eq!(setting_name("batch-interval"), Some(BATCH_INTERVAL_SETTING));
        assert_eq!(setting_name("rollout.window"), Some(WINDOW_SETTING));
        assert_eq!(
            setting_name("deployment_space"),
            Some(DEPLOYMENT_SPACE_SETTING)
        );
        assert_eq!(setting_name("canary_count"), None);
        assert_eq!(
            check_setting(BATCH_INTERVAL_SETTING, Some("5m")).unwrap(),
            Some("300".into())
        );
        let e = check_setting(STRATEGY_SETTING, Some("canary")).unwrap_err();
        assert!(
            e.to_string().contains("ordo rollout set rollout.strategy"),
            "{e}"
        );
        assert_eq!(e.to_exec_error("x").code, ErrorCode::InvalidParam);
        assert!(check_setting(FAILURE_THRESHOLD_SETTING, Some("0")).is_err());
        assert_eq!(check_setting(DEPLOYMENT_SPACE_SETTING, None).unwrap(), None);
    }

    #[test]
    fn a_second_rollout_names_status_and_abort() {
        let e = RolloutRefusal::Open {
            id: 3,
            target: "0.2.0".into(),
        };
        let text = e.to_string();
        assert!(text.contains("ordo rollout status"), "{text}");
        assert!(text.contains("ordo rollout abort"), "{text}");
        assert_eq!(e.to_exec_error("x").code, ErrorCode::ApplyRefused);
    }
}
