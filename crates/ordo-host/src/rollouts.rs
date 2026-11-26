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
//!   moves first, or last when `rollout.deployment_space` is `last`. It
//!   moves like a tenant: its pin (`$own_pin`, [`Versions::pin_own`]) names
//!   the target, and every node serves it there.
//! - **"Active" flips at the end** (Q5). When the rollout finishes, or is
//!   finished by hand, the target is activated with no tenant moved
//!   ([`Versions::activate_alone`]). Until then sign-in (`AuthApp`) and new
//!   tenants stay on the version that was active.
//! - **Strategies**: `rolling`, a batch every `rollout.batch_interval`,
//!   sized to finish within `rollout.window`; `all`, every tenant in one
//!   batch. Increment 10c: `canary`, `group` and `query` move a first stage,
//!   then the rest once it is healthy (`crate::rollout_strategies`).
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

/// How a pause by the first stage's health begins its reason: `resume`
/// then passes the gate.
pub const UNHEALTHY: &str = "unhealthy";
/// The reason a rollout paused on a first stage with too few calls starts
/// with this (ruled 2026-11-25).
pub const QUIET: &str = "quiet";

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

/// Increment 10c.
pub const CANARY_COUNT_SETTING: &str = "rollout.canary_count";
pub const CANARY_WAIT_SETTING: &str = "rollout.canary_wait";
pub const HEALTH_ERROR_RATE_SETTING: &str = "rollout.health_error_rate";
pub const ALERT_CHANNEL_SETTING: &str = "rollout.alert_channel";
pub const SHARE_PARTNERS_SETTING: &str = "rollout.share_partners";
pub const GROUP_SETTING: &str = "rollout.group";
pub const QUERY_SETTING: &str = "rollout.query";
pub const ORDER_SETTING: &str = "rollout.order";
pub const LIMIT_SETTING: &str = "rollout.limit";
/// Ruled 2026-11-25: the calls a first stage must have before it passes.
pub const HEALTH_MIN_CALLS_SETTING: &str = "rollout.health_min_calls";

pub const SETTINGS: [&str; 15] = [
    STRATEGY_SETTING,
    BATCH_INTERVAL_SETTING,
    WINDOW_SETTING,
    FAILURE_THRESHOLD_SETTING,
    DEPLOYMENT_SPACE_SETTING,
    CANARY_COUNT_SETTING,
    CANARY_WAIT_SETTING,
    HEALTH_ERROR_RATE_SETTING,
    ALERT_CHANNEL_SETTING,
    SHARE_PARTNERS_SETTING,
    GROUP_SETTING,
    QUERY_SETTING,
    ORDER_SETTING,
    LIMIT_SETTING,
    HEALTH_MIN_CALLS_SETTING,
];

/// The key a setting answers under in [`RolloutSettings::to_json`].
pub fn json_key(setting: &str) -> &'static str {
    match setting {
        STRATEGY_SETTING => "strategy",
        BATCH_INTERVAL_SETTING => "batchInterval",
        WINDOW_SETTING => "window",
        FAILURE_THRESHOLD_SETTING => "failureThreshold",
        CANARY_COUNT_SETTING => "canaryCount",
        CANARY_WAIT_SETTING => "canaryWait",
        HEALTH_ERROR_RATE_SETTING => "healthErrorRate",
        ALERT_CHANNEL_SETTING => "alertChannel",
        SHARE_PARTNERS_SETTING => "sharePartners",
        GROUP_SETTING => "group",
        QUERY_SETTING => "query",
        ORDER_SETTING => "order",
        LIMIT_SETTING => "limit",
        HEALTH_MIN_CALLS_SETTING => "healthMinCalls",
        _ => "deploymentSpace",
    }
}

/// `rollout.batch_interval`'s default: 5 minutes (owner, 2026-09-21).
pub const BATCH_INTERVAL_DEFAULT: i64 = 300;
/// `rollout.window`'s default: 1 hour, a target.
pub const WINDOW_DEFAULT: i64 = 3_600;
/// `rollout.failure_threshold`'s default: 1 tenant.
pub const FAILURE_THRESHOLD_DEFAULT: i64 = 1;
/// `rollout.canary_count`'s default: 1 tenant.
pub const CANARY_COUNT_DEFAULT: i64 = 1;
/// `rollout.canary_wait`'s default: 1 hour.
pub const CANARY_WAIT_DEFAULT: i64 = 3_600;
// The most tenants a group names (`limits.rollout_group`) and the longest
// query or order (`limits.query_chars`) are settings: `crate::input_caps`.

// ── the words ────────────────────────────────────────────────────────────

/// Which pins move when (§3.5). Increment 10 built `rolling` and `all`;
/// 10c the three with a first stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// A batch every `rollout.batch_interval`.
    Rolling,
    /// Every tenant in one batch.
    All,
    /// `rollout.canary_count` tenants at random first; the rest, rolling,
    /// once they are healthy for `rollout.canary_wait`.
    Canary,
    /// A named group first (`rollout.group`); the rest, rolling, once it is
    /// healthy for `rollout.canary_wait`.
    Group,
    /// The tenants a query over the tenant facts selects (`rollout.query`),
    /// in its order (`rollout.order`), a batch at a time; the rest once
    /// they are healthy for `rollout.canary_wait`.
    Query,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Rolling => "rolling",
            Strategy::All => "all",
            Strategy::Canary => "canary",
            Strategy::Group => "group",
            Strategy::Query => "query",
        }
    }

    pub fn parse(text: &str) -> Option<Strategy> {
        match text.trim().replace('_', "-").as_str() {
            "rolling" => Some(Strategy::Rolling),
            "all" => Some(Strategy::All),
            "canary" => Some(Strategy::Canary),
            "group" | "group-first" => Some(Strategy::Group),
            "query" | "by-query" => Some(Strategy::Query),
            _ => None,
        }
    }

    /// A first stage, then a health gate, then the rest.
    pub fn staged(self) -> bool {
        matches!(self, Strategy::Canary | Strategy::Group | Strategy::Query)
    }
}

/// The words a strategy is one of.
pub const STRATEGIES: &str = "`rolling`, `all`, `canary`, `group` or `query`";

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
    /// `None`: unset, and [`RolloutSettings::default_strategy`] holds
    /// (ruled 2026-11-25). See [`RolloutSettings::apply_strategy`] for what
    /// an apply does with it.
    pub strategy: Option<Strategy>,
    /// What an unset strategy is here: `rolling` in every environment but
    /// dev, `all` in dev, where `ordo apply` activates every tenant at once
    /// as before rollouts (ruled 2026-11-25). Read from the environment
    /// registry by [`Rollouts::settings`].
    pub default_strategy: Strategy,
    pub batch_interval: i64,
    pub window: i64,
    pub failure_threshold: i64,
    pub deployment_space: DeploymentSpace,
    pub canary_count: i64,
    /// Seconds a first stage stays healthy before the rest move.
    pub canary_wait: i64,
    /// In hundredths of a percent.
    pub health_error_rate: i64,
    /// The calls the first stage must have had before it passes; fewer
    /// pause the rollout (ruled 2026-11-25). 0 judges a quiet stage healthy.
    pub health_min_calls: i64,
    /// `stream`, or `mail:<address>`.
    pub alert_channel: String,
    pub share_partners: bool,
    /// The tenants `group` moves first.
    pub group: Vec<String>,
    /// The condition `query` selects with, and its order.
    pub query: Option<String>,
    pub order: Option<String>,
    /// At most this many of the tenants the query selects: "the 5 least
    /// used". `None`: every one.
    pub limit: Option<i64>,
}

impl Default for RolloutSettings {
    fn default() -> RolloutSettings {
        RolloutSettings {
            strategy: None,
            default_strategy: Strategy::Rolling,
            batch_interval: BATCH_INTERVAL_DEFAULT,
            window: WINDOW_DEFAULT,
            failure_threshold: FAILURE_THRESHOLD_DEFAULT,
            deployment_space: DeploymentSpace::First,
            canary_count: CANARY_COUNT_DEFAULT,
            canary_wait: CANARY_WAIT_DEFAULT,
            health_error_rate: crate::rollout_strategies::HEALTH_ERROR_RATE_DEFAULT,
            health_min_calls: crate::rollout_strategies::HEALTH_MIN_CALLS_DEFAULT,
            alert_channel: crate::rollout_strategies::ALERT_STREAM.to_string(),
            share_partners: false,
            group: Vec::new(),
            query: None,
            order: None,
            limit: None,
        }
    }
}

/// A group as typed: tenant ids, comma or space separated.
pub fn group_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in text
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        if !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// The environment whose unset `rollout.strategy` moves every tenant at once
/// (ruled 2026-11-25).
pub const DEV_ENVIRONMENT: &str = "dev";

/// What an unset `rollout.strategy` is in the environment `own`: `all` in
/// dev, `rolling` everywhere else (ruled 2026-11-25).
pub fn default_strategy_in(own: &str) -> Strategy {
    if own == DEV_ENVIRONMENT {
        Strategy::All
    } else {
        Strategy::Rolling
    }
}

impl RolloutSettings {
    /// **Whether `ordo apply` starts a rollout, and with which strategy.**
    /// The strategy set, or the default where none is: `rolling` in every
    /// environment but dev (ruled 2026-11-25). `all`, set or dev's default,
    /// is the activation as before rollouts: every tenant at once, "active"
    /// at once, so a save in dev stays quick (§3.7: "the strategy `all` is
    /// today's behaviour"). `None` then. `ordo rollout start --strategy all`
    /// still starts a rollout that moves every tenant in one batch.
    pub fn apply_strategy(&self) -> Option<Strategy> {
        match self.strategy.unwrap_or(self.default_strategy) {
            Strategy::All => None,
            s => Some(s),
        }
    }

    /// The value of one as it reads, `None` when unset.
    pub fn to_json(&self) -> Value {
        json!({
            "strategy": self.strategy.map(Strategy::as_str),
            "strategyDefault": self.default_strategy.as_str(),
            "batchInterval": self.batch_interval,
            "window": self.window,
            "failureThreshold": self.failure_threshold,
            "deploymentSpace": self.deployment_space.as_str(),
            "canaryCount": self.canary_count,
            "canaryWait": self.canary_wait,
            "healthErrorRate": crate::rollout_strategies::format_rate(self.health_error_rate),
            "healthMinCalls": self.health_min_calls,
            "alertChannel": self.alert_channel,
            "sharePartners": self.share_partners,
            "group": self.group,
            "query": self.query,
            "order": self.order,
            "limit": self.limit,
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
            (CANARY_COUNT_SETTING, v) => {
                self.canary_count = v
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(CANARY_COUNT_DEFAULT)
            }
            (CANARY_WAIT_SETTING, v) => {
                self.canary_wait = v.and_then(parse_secs).unwrap_or(CANARY_WAIT_DEFAULT)
            }
            (HEALTH_ERROR_RATE_SETTING, v) => {
                self.health_error_rate = v
                    .and_then(crate::rollout_strategies::parse_rate)
                    .unwrap_or(crate::rollout_strategies::HEALTH_ERROR_RATE_DEFAULT)
            }
            (HEALTH_MIN_CALLS_SETTING, v) => {
                self.health_min_calls = v
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(crate::rollout_strategies::HEALTH_MIN_CALLS_DEFAULT)
            }
            (ALERT_CHANNEL_SETTING, v) => {
                self.alert_channel = v
                    .and_then(|v| crate::rollout_strategies::check_channel(v).ok())
                    .unwrap_or_else(|| crate::rollout_strategies::ALERT_STREAM.to_string())
            }
            (SHARE_PARTNERS_SETTING, v) => {
                self.share_partners = v.and_then(on_off).unwrap_or(false)
            }
            (GROUP_SETTING, v) => self.group = v.map(group_list).unwrap_or_default(),
            (QUERY_SETTING, v) => self.query = v.map(str::to_string),
            (ORDER_SETTING, v) => self.order = v.map(str::to_string),
            (LIMIT_SETTING, v) => self.limit = v.and_then(|v| v.trim().parse().ok()),
            _ => {}
        }
    }
}

fn parse_secs(text: &str) -> Option<i64> {
    crate::windows::parse_length(text).ok()
}

/// `on`/`off` and their usual spellings.
pub fn on_off(text: &str) -> Option<bool> {
    match text.trim() {
        "on" | "true" | "yes" => Some(true),
        "off" | "false" | "no" => Some(false),
        _ => None,
    }
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
                bad(&format!(
                    "it is {STRATEGIES}. `all` makes `ordo apply` move every tenant at once, \
                     as before rollouts; `--clear` gives the default: `rolling`, or `all` in \
                     dev"
                ))
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
        CANARY_COUNT_SETTING => match v.parse::<i64>() {
            Ok(n) if n >= 1 => Ok(Some(n.to_string())),
            _ => Err(bad("it is a whole number of tenants, 1 or more")),
        },
        CANARY_WAIT_SETTING => match parse_secs(v) {
            Some(n) if n >= 0 => Ok(Some(n.to_string())),
            _ => Err(bad("it is a length, like `0s`, `30m` or `1h`")),
        },
        HEALTH_ERROR_RATE_SETTING => crate::rollout_strategies::parse_rate(v)
            .map(|r| Some(crate::rollout_strategies::format_rate(r)))
            .ok_or_else(|| bad("it is a percentage from 0.01% to 100%, like `1%`")),
        HEALTH_MIN_CALLS_SETTING => match v.parse::<i64>() {
            Ok(n) if (0..=crate::rollout_strategies::MAX_MIN_CALLS).contains(&n) => {
                Ok(Some(n.to_string()))
            }
            _ => Err(bad(&format!(
                "it is a whole number of calls from 0 to {}; 0 lets a first stage with no \
                 calls pass",
                crate::rollout_strategies::MAX_MIN_CALLS
            ))),
        },
        ALERT_CHANNEL_SETTING => crate::rollout_strategies::check_channel(v)
            .map(Some)
            .map_err(|why| bad(&why)),
        SHARE_PARTNERS_SETTING => on_off(v)
            .map(|b| Some(if b { "on" } else { "off" }.to_string()))
            .ok_or_else(|| bad("it is `on` or `off`")),
        GROUP_SETTING => {
            let list = group_list(v);
            let max_group = crate::input_caps::InputCaps::current().rollout_group;
            if list.len() > max_group {
                return Err(bad(&format!(
                    "a group names {max_group} tenants at most (`limits.rollout_group`; \
                     `ordo versions set limits.rollout_group <n> --reason \"...\"` changes it)"
                )));
            }
            Ok(Some(list.join(",")))
        }
        QUERY_SETTING => {
            let max_query = crate::input_caps::InputCaps::current().query_chars;
            if v.chars().count() > max_query {
                return Err(bad(&format!(
                    "a query is {max_query} characters at most (`limits.query_chars`; \
                     `ordo versions set limits.query_chars <n> --reason \"...\"` changes it)"
                )));
            }
            crate::rollout_strategies::Query::parse(v, None)
                .map(|_| Some(v.to_string()))
                .map_err(|why| bad(&why))
        }
        ORDER_SETTING => {
            let max_query = crate::input_caps::InputCaps::current().query_chars;
            if v.chars().count() > max_query {
                return Err(bad(&format!(
                    "an order is {max_query} characters at most (`limits.query_chars`; \
                     `ordo versions set limits.query_chars <n> --reason \"...\"` changes it)"
                )));
            }
            crate::rollout_strategies::Query::parse("true", Some(v))
                .map(|_| Some(v.to_string()))
                .map_err(|why| bad(&why))
        }
        LIMIT_SETTING => match v.parse::<i64>() {
            Ok(n) if n >= 1 => Ok(Some(n.to_string())),
            _ => Err(bad("it is a whole number of tenants, 1 or more")),
        },
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

/// Increment 10c's columns, added to a table increment 10 made. `gate` is
/// `''` for a strategy with no first stage, `pending` until the first stage
/// is healthy for its wait, then `passed`. A tenant's `stage` is 0 in the
/// first stage and 1 in the rest.
const DDL_10C: &str = "\
ALTER TABLE \"$rollouts\" ADD COLUMN IF NOT EXISTS \"canary_count\" bigint NOT NULL DEFAULT 0; \
ALTER TABLE \"$rollouts\" ADD COLUMN IF NOT EXISTS \"canary_wait\" bigint NOT NULL DEFAULT 0; \
ALTER TABLE \"$rollouts\" ADD COLUMN IF NOT EXISTS \"health_error_rate\" bigint NOT NULL \
  DEFAULT 100; \
ALTER TABLE \"$rollouts\" ADD COLUMN IF NOT EXISTS \"alert_channel\" text NOT NULL \
  DEFAULT 'stream'; \
ALTER TABLE \"$rollouts\" ADD COLUMN IF NOT EXISTS \"share_partners\" boolean NOT NULL \
  DEFAULT false; \
ALTER TABLE \"$rollouts\" ADD COLUMN IF NOT EXISTS \"selection\" text NOT NULL DEFAULT ''; \
ALTER TABLE \"$rollouts\" ADD COLUMN IF NOT EXISTS \"gate\" text NOT NULL DEFAULT ''; \
ALTER TABLE \"$rollouts\" ADD COLUMN IF NOT EXISTS \"first_done_at\" bigint NOT NULL DEFAULT 0; \
ALTER TABLE \"$rollout_tenants\" ADD COLUMN IF NOT EXISTS \"stage\" bigint NOT NULL DEFAULT 1; \
ALTER TABLE \"$rollouts\" ADD COLUMN IF NOT EXISTS \"health_min_calls\" bigint NOT NULL \
  DEFAULT 0";

const COLUMNS: &str =
    "\"id\", \"target\", \"from\", \"strategy\", \"batch_interval\", \"window\", \
  \"batch_size\", \"failure_threshold\", \"deployment_space\", \"state\", \"reason\", \"actor\", \
  \"started_at\", \"ended_at\", \"last_batch_at\", \"batches\", \"failed_base\", \"pin_seq\", \
  \"leader\", \"led_at\", \"canary_count\", \"canary_wait\", \"health_error_rate\", \
  \"alert_channel\", \"share_partners\", \"selection\", \"gate\", \"first_done_at\", \
  \"health_min_calls\"";

/// Where a staged rollout's first stage stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// No first stage: `rolling` and `all`.
    None,
    /// The first stage moves, or waits to be healthy for its wait.
    Pending,
    /// The rest move.
    Passed,
}

impl Gate {
    pub fn as_str(self) -> &'static str {
        match self {
            Gate::None => "",
            Gate::Pending => "pending",
            Gate::Passed => "passed",
        }
    }

    fn parse(text: &str) -> Gate {
        match text {
            "pending" => Gate::Pending,
            "passed" => Gate::Passed,
            _ => Gate::None,
        }
    }
}

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
    /// Increment 10c: how many tenants `canary` moves first.
    pub canary_count: i64,
    /// Seconds the first stage stays healthy before the rest move.
    pub canary_wait: i64,
    /// In hundredths of a percent.
    pub health_error_rate: i64,
    pub alert_channel: String,
    pub share_partners: bool,
    /// The group or the query (and its order) the first stage was chosen
    /// by, as given.
    pub selection: String,
    pub gate: Gate,
    /// When the first stage had nothing left to move; 0 before.
    pub first_done_at: u64,
    /// The calls the first stage must have had before it passes (ruled
    /// 2026-11-25). A rollout started before the setting reads 0.
    pub health_min_calls: i64,
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
        canary_count: n(20).unwrap_or(0),
        canary_wait: n(21).unwrap_or(0),
        health_error_rate: n(22).unwrap_or(crate::rollout_strategies::HEALTH_ERROR_RATE_DEFAULT),
        alert_channel: s(23).unwrap_or_else(|| crate::rollout_strategies::ALERT_STREAM.to_string()),
        share_partners: matches!(s(24).as_deref(), Some("t" | "true")),
        selection: s(25).unwrap_or_default(),
        gate: Gate::parse(s(26).as_deref().unwrap_or_default()),
        first_done_at: n(27).unwrap_or(0) as u64,
        health_min_calls: n(28).unwrap_or(0),
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
            "canaryCount": self.canary_count,
            "canaryWait": self.canary_wait,
            "healthErrorRate": crate::rollout_strategies::format_rate(self.health_error_rate),
            "healthMinCalls": self.health_min_calls,
            "alertChannel": self.alert_channel,
            "sharePartners": self.share_partners,
            "selection": self.selection,
            "gate": self.gate.as_str(),
            "firstDoneAt": (self.first_done_at > 0).then_some(self.first_done_at),
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
    /// 0 in a staged rollout's first stage, 1 otherwise.
    pub stage: i64,
}

impl RolloutTenant {
    pub fn to_json(&self) -> Value {
        json!({
            "tenant": self.space,
            "state": self.state.as_str(),
            "reason": self.reason,
            "at": self.at,
            "firstStage": self.stage == 0,
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
        stage: r
            .get(5)
            .cloned()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
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
    /// The calls and server faults every node counted for the moved
    /// tenants while the first stage's health was judged (10c).
    pub health: (u64, u64),
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
        let (calls, faults) = self.health;
        row["health"] = json!({
            "calls": calls,
            "faults": faults,
            "healthy": !crate::rollout_strategies::unhealthy(
                calls,
                faults,
                self.rollout.health_error_rate
            ),
        });
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
    /// Increment 10c.
    pub canary_count: Option<i64>,
    pub canary_wait: Option<i64>,
    /// In hundredths of a percent.
    pub health_error_rate: Option<i64>,
    pub health_min_calls: Option<i64>,
    pub alert_channel: Option<String>,
    pub share_partners: Option<bool>,
    /// The tenants `group` moves first.
    pub group: Option<Vec<String>>,
    /// The condition `query` selects with, and its order.
    pub query: Option<String>,
    pub order: Option<String>,
    /// At most this many of the tenants the query selects.
    pub limit: Option<i64>,
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
    /// Where tenants with their own database are reached, for the facts a
    /// query reads there. `None` reads the deployment's database only.
    pools: Option<&'a dyn ordo_exec::route::PoolResolver>,
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
        Rollouts { pool, pools: None }
    }

    /// The same client, reaching tenants with their own database through
    /// `pools`: a query's facts are then counted there too.
    pub fn reaching(self, pools: &'a dyn ordo_exec::route::PoolResolver) -> Rollouts<'a> {
        Rollouts {
            pools: Some(pools),
            ..self
        }
    }

    /// Create the three tables. Idempotent; retried once, as `$pins` is,
    /// because `IF NOT EXISTS` is not race-free between 2 booting nodes.
    pub async fn ensure(&self) -> Result<(), RolloutRefusal> {
        Versions::on(self.pool).ensure().await?;
        if self.pool.rows(DDL).await.is_err() {
            self.pool.rows(DDL).await?;
        }
        for ddl in [DDL_10C, crate::rollout_strategies::health_ddl()] {
            if self.pool.rows(ddl).await.is_err() {
                self.pool.rows(ddl).await?;
            }
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
            Err(e) if e.is_missing_table() => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        // The environment registry answers production when nothing was
        // registered; a registry that cannot be read is a store error.
        let default_strategy = match crate::environments::Environments::on(self.pool).own().await {
            Ok(env) => default_strategy_in(&env.name),
            Err(crate::environments::EnvironmentRefusal::Store(e)) => {
                return Err(RolloutRefusal::Store(e))
            }
            Err(e) => {
                return Err(RolloutRefusal::Store(PoolError::db(format!(
                    "the environment registry could not be read: {e}"
                ))))
            }
        };
        let mut out = RolloutSettings {
            default_strategy,
            ..RolloutSettings::default()
        };
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
        let rows = match self.pool.rows(&sql).await {
            // A table an older build made: its new columns first.
            Err(e) if e.is_missing_column() => {
                self.ensure().await?;
                self.pool.rows(&sql).await
            }
            other => other,
        };
        match rows {
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
                "SELECT \"space\", \"ord\", \"state\", \"reason\", \"at\", \"stage\" FROM \"$rollout_tenants\" \
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
                "SELECT \"space\", \"ord\", \"state\", \"reason\", \"at\", \"stage\" FROM \"$rollout_tenants\" \
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
        let health = crate::rollout_strategies::health_of(self.pool, rollout.id).await?;
        Ok(Some(Status {
            rollout,
            counts,
            tenants,
            deployment,
            health,
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
            // A rename the tenants there take by moves with dual writes,
            // as each tenant with a database of its own does (11b part 3).
            Ok(_) if crate::every_tenant::moves_there(self.pool, &target).await => {}
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
            .unwrap_or(settings.default_strategy);
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
            (_, Some(n)) => n,
            (_, None) => {
                // Enough per batch to finish within the window: a target,
                // never a deadline (§1.5).
                let batches = (window / batch_interval.max(1)).max(1);
                let total = off_target.max(1) as i64;
                ((total + batches - 1) / batches).max(1)
            }
        };

        // Increment 10c: the first stage, its health and the channel.
        let bad = |setting: &str, value: &str, why: &str| RolloutRefusal::Setting {
            setting: setting.to_string(),
            value: value.chars().take(40).collect(),
            why: why.to_string(),
        };
        let canary_count = opts.canary_count.unwrap_or(settings.canary_count);
        if canary_count < 1 {
            return Err(bad(
                CANARY_COUNT_SETTING,
                &canary_count.to_string(),
                "it is 1 or more",
            ));
        }
        let canary_wait = opts.canary_wait.unwrap_or(settings.canary_wait);
        if canary_wait < 0 {
            return Err(bad(
                CANARY_WAIT_SETTING,
                &canary_wait.to_string(),
                "it is 0 seconds or more",
            ));
        }
        let health_error_rate = opts.health_error_rate.unwrap_or(settings.health_error_rate);
        if !(1..=10_000).contains(&health_error_rate) {
            return Err(bad(
                HEALTH_ERROR_RATE_SETTING,
                &health_error_rate.to_string(),
                "it is a percentage from 0.01% to 100%",
            ));
        }
        let health_min_calls = opts.health_min_calls.unwrap_or(settings.health_min_calls);
        if !(0..=crate::rollout_strategies::MAX_MIN_CALLS).contains(&health_min_calls) {
            return Err(bad(
                HEALTH_MIN_CALLS_SETTING,
                &health_min_calls.to_string(),
                &format!(
                    "it is a whole number of calls from 0 to {}",
                    crate::rollout_strategies::MAX_MIN_CALLS
                ),
            ));
        }
        let alert_channel = match &opts.alert_channel {
            Some(c) => crate::rollout_strategies::check_channel(c)
                .map_err(|why| bad(ALERT_CHANNEL_SETTING, c, &why))?,
            None => settings.alert_channel.clone(),
        };
        let share_partners = opts.share_partners.unwrap_or(settings.share_partners);
        // The first stage, in its order, before the rows are written.
        let (first, selection): (Option<Vec<String>>, String) = match strategy {
            Strategy::Group => {
                let group = opts.group.clone().unwrap_or(settings.group.clone());
                if group.is_empty() {
                    return Err(bad(
                        GROUP_SETTING,
                        "",
                        "the group strategy names the tenants that move first: `--group a,b` \
                         or `ordo rollout set rollout.group a,b --reason \"...\"`",
                    ));
                }
                let max_group = crate::input_caps::InputCaps::current().rollout_group;
                if group.len() > max_group {
                    return Err(bad(
                        GROUP_SETTING,
                        &group.len().to_string(),
                        &format!(
                            "a group names {max_group} tenants at most (`limits.rollout_group`; \
                             `ordo versions set limits.rollout_group <n> --reason \"...\"` \
                             changes it)"
                        ),
                    ));
                }
                let known = crate::rollout_strategies::facts(
                    self.pool,
                    None,
                    &crate::rollout_strategies::Needs::default(),
                    at,
                )
                .await?;
                if let Some(t) = group
                    .iter()
                    .find(|t| !known.iter().any(|f| &f.tenant == *t))
                {
                    return Err(RolloutRefusal::Tenant {
                        tenant: t.clone(),
                        why: "the group names it, and it is not a tenant this deployment \
                              serves. `std.deployment.tenants` lists them"
                            .into(),
                    });
                }
                let text = format!("group: {}", group.join(","));
                (Some(group), text)
            }
            Strategy::Query => {
                let q = opts.query.clone().or(settings.query.clone());
                let order = opts.order.clone().or(settings.order.clone());
                let Some(q) = q.filter(|q| !q.trim().is_empty()) else {
                    return Err(bad(
                        QUERY_SETTING,
                        "",
                        "the query strategy selects with a condition over the tenant facts: \
                         `--query \"members < 5\"` or `ordo rollout set rollout.query ...`",
                    ));
                };
                let max_query = crate::input_caps::InputCaps::current().query_chars;
                if q.chars().count() > max_query
                    || order.as_deref().unwrap_or_default().chars().count() > max_query
                {
                    return Err(bad(
                        QUERY_SETTING,
                        &q,
                        &format!(
                            "a query and its order are {max_query} characters at most \
                             (`limits.query_chars`; `ordo versions set limits.query_chars <n> \
                             --reason \"...\"` changes it)"
                        ),
                    ));
                }
                let limit = opts.limit.or(settings.limit);
                if let Some(n) = limit.filter(|n| *n < 1) {
                    return Err(bad(LIMIT_SETTING, &n.to_string(), "it is 1 or more"));
                }
                let parsed = crate::rollout_strategies::Query::parse(&q, order.as_deref())
                    .map_err(|why| bad(QUERY_SETTING, &q, &why))?
                    .limited(limit.map(|n| n as usize));
                // `rows.<entity>` names an entity of the version rolled out.
                if let Some(e) = parsed
                    .needs()
                    .rows
                    .iter()
                    .find(|e| !program.schema.entities.iter().any(|x| &x.name == *e))
                {
                    return Err(bad(
                        QUERY_SETTING,
                        &q,
                        &format!(
                            "`rows.{e}` names no entity of {target}; it has {}",
                            program
                                .schema
                                .entities
                                .iter()
                                .map(|x| x.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
                let facts =
                    crate::rollout_strategies::facts(self.pool, self.pools, parsed.needs(), at)
                        .await?;
                let mut text = match &order {
                    Some(o) => format!("query: {q}; order: {o}"),
                    None => format!("query: {q}"),
                };
                if let Some(n) = limit {
                    text.push_str(&format!("; limit: {n}"));
                }
                (Some(parsed.select(facts)), text)
            }
            Strategy::Canary => (None, format!("canary: {canary_count}")),
            _ => (None, String::new()),
        };

        crate::versions::ensure_build_tables(self.pool).await?;
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
                   \"state\", \"actor\", \"started_at\", \"pin_seq\", \"canary_count\", \
                   \"canary_wait\", \"health_error_rate\", \"alert_channel\", \"share_partners\", \
                   \"selection\", \"gate\", \"health_min_calls\") \
                 VALUES ({}, {}, {}, {batch_interval}, {window}, {batch_size}, \
                   {failure_threshold}, {}, 'running', {}, {at}, {head}, {canary_count}, \
                   {canary_wait}, {health_error_rate}, {}, {share_partners}, {}, {}, \
                   {health_min_calls}) \
                 RETURNING \"id\"",
                sql_lit(&target),
                active.as_deref().map(sql_lit).unwrap_or_else(|| "NULL".into()),
                sql_lit(strategy.as_str()),
                sql_lit(deployment_space.as_str()),
                sql_lit(actor),
                sql_lit(&alert_channel),
                sql_lit(&selection),
                sql_lit(if strategy.staged() {
                    Gate::Pending.as_str()
                } else {
                    Gate::None.as_str()
                }),
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
        // A tenant a tenant build holds is not the rollout's: its build
        // moves it (increment 16).
        tx.rows(&format!(
            "INSERT INTO \"$rollout_tenants\" (\"rollout\", \"space\", \"ord\", \"state\", \"at\") \
             SELECT {id}, \"$space\", row_number() OVER (ORDER BY \"$space\"), 'queued', {at} \
             FROM \"$spaces\" \
             WHERE \"state\" IN ('provisioning', 'active', 'paused') \
               AND \"version\" IS NOT NULL AND \"version\" <> {} {}",
            sql_lit(&target),
            crate::versions::NOT_IN_A_BUILD,
        ))
        .await?;
        // The first stage: a random few for a canary; the group or the
        // query's tenants, in their order, ahead of every other tenant.
        if strategy == Strategy::Canary {
            tx.rows(&format!(
                "UPDATE \"$rollout_tenants\" SET \"stage\" = 0 \
                 WHERE \"rollout\" = {id} AND \"space\" IN ( \
                   SELECT \"space\" FROM \"$rollout_tenants\" \
                   WHERE \"rollout\" = {id} AND \"space\" <> {step} \
                   ORDER BY random() LIMIT {canary_count})",
                step = sql_lit(DEPLOYMENT_STEP)
            ))
            .await?;
        }
        for (i, t) in first.iter().flatten().enumerate() {
            tx.rows(&format!(
                "UPDATE \"$rollout_tenants\" SET \"stage\" = 0, \"ord\" = {} \
                 WHERE \"rollout\" = {id} AND \"space\" = {}",
                i + 1,
                sql_lit(t)
            ))
            .await?;
        }
        if strategy.staged() {
            // A first stage with nothing in it has nothing to wait on.
            let none = tx
                .rows(&format!(
                    "SELECT count(*) FROM \"$rollout_tenants\" WHERE \"rollout\" = {id} \
                     AND \"stage\" = 0"
                ))
                .await?
                .first()
                .and_then(|r| r.first().cloned().flatten())
                .is_some_and(|n| n == "0");
            if none {
                tx.rows(&format!(
                    "UPDATE \"$rollouts\" SET \"gate\" = 'passed' WHERE \"id\" = {id}"
                ))
                .await?;
            }
        }
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
        // A resume after an unhealthy first stage is the deployment owner
        // moving the rest anyway: the gate is passed.
        let gate = if r.gate == Gate::Pending
            && (r.reason.starts_with(UNHEALTHY) || r.reason.starts_with(QUIET))
        {
            ", \"gate\" = 'passed', \"last_batch_at\" = 0"
        } else {
            ""
        };
        self.set_state(
            r.id,
            RolloutState::Running,
            "",
            at,
            &format!(", \"failed_base\" = {failed}{gate}"),
        )
        .await?;
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
    /// skipped. The deployment's own space (`home`) moves now when it has
    /// not, and the target becomes active (Q5). Refused when
    /// `versions.served` would leave a skipped tenant on a version it no
    /// longer serves.
    pub async fn finish(
        &self,
        home: &str,
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
        self.deployment_step(&r, home, actor, at).await?;
        self.activate(&r, actor, at).await?;
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

    /// Move the deployment's own space (`home`), when the rollout has that
    /// step and it has not moved: its pin names the target (Q4). "Active"
    /// does not move.
    async fn deployment_step(
        &self,
        r: &Rollout,
        home: &str,
        actor: &str,
        at: u64,
    ) -> Result<bool, RolloutRefusal> {
        let Some(step) = self.tenant(r.id, DEPLOYMENT_STEP).await? else {
            return Ok(false);
        };
        if step.state == TenantState::Moved {
            return Ok(false);
        }
        // Its database still records the version before, and the tenants
        // there move with dual writes: the own space waits for the first
        // move to expand and copy it (11b part 3).
        if crate::every_tenant::moves_there(self.pool, &r.target).await
            && !crate::every_tenant::copied_there(self.pool, &r.target).await
        {
            self.set_tenant(
                r.id,
                DEPLOYMENT_STEP,
                TenantState::Waiting,
                "waits for the first move on the deployment's own database to copy its rows",
                at,
            )
            .await?;
            return Ok(false);
        }
        Versions::on(self.pool)
            .pin_own(home, &r.target, r.id, actor, at)
            .await?;
        self.set_tenant(r.id, DEPLOYMENT_STEP, TenantState::Moved, "", at)
            .await?;
        Ok(true)
    }

    /// The end of a rollout: "active" flips to the target, so sign-in and
    /// new tenants follow it (Q5). No tenant moves.
    async fn activate(&self, r: &Rollout, actor: &str, at: u64) -> Result<(), RolloutRefusal> {
        Versions::on(self.pool)
            .activate_alone(&r.target, actor, Timestamp(at))
            .await?;
        Ok(())
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
    /// The first stage's health paused it in this tick (10c).
    pub unhealthy: bool,
    /// The first stage had fewer calls than `rollout.health_min_calls`
    /// when its wait ended, and paused it in this tick.
    pub quiet: bool,
    /// The first stage was healthy for its wait: the rest move (10c).
    pub gate_passed: bool,
    /// What the alert channel could not do, when it failed.
    pub alert_failed: Option<String>,
    /// The most tenants' moves that ran at once in this tick
    /// (`backfill.concurrency`, `backfill.per_database`).
    pub at_once: usize,
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
        if self.at_once > 0 {
            out.push(format!(
                "rollout {id}: at most {} tenant moves ran at once ({} and {})",
                self.at_once,
                crate::backfill::CONCURRENCY_SETTING,
                crate::backfill::PER_DATABASE_SETTING
            ));
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
        if self.paused && self.unhealthy {
            out.push(format!(
                "rollout {id}: paused by {HEALTH_ERROR_RATE_SETTING}; `ordo rollout status`"
            ));
        } else if self.paused && self.quiet {
            out.push(format!(
                "rollout {id}: paused by {HEALTH_MIN_CALLS_SETTING}; `ordo rollout status`"
            ));
        } else if self.paused {
            out.push(format!(
                "rollout {id}: paused by {FAILURE_THRESHOLD_SETTING}; `ordo rollout status`"
            ));
        }
        if self.gate_passed {
            out.push(format!(
                "rollout {id}: the first stage is healthy; the rest move"
            ));
        }
        if let Some(why) = &self.alert_failed {
            out.push(format!("rollout {id}: the alert was not sent: {why}"));
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
    /// Partway through a move that takes several turns (a backfill under
    /// its budget, increment 11b). Not a failure, and not waiting.
    Moving(String),
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
    crate::versions::ensure_build_tables(pool).await?;
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
             WHERE t.\"rollout\" = {id} AND t.\"space\" = s.\"$space\") {not_in_a_build}",
        id = r.id,
        target = sql_lit(&r.target),
        not_in_a_build = crate::versions::NOT_IN_A_BUILD,
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
                 ORDER BY p.\"seq\" DESC LIMIT 1), \
               (SELECT b.\"build\" FROM \"$tenant_build_members\" b \
                 WHERE b.\"space\" = t.\"space\") \
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
        let build = row.get(4).cloned().flatten();
        let serving = matches!(state.as_deref(), Some("provisioning" | "active" | "paused"));
        if let Some(build) = build {
            // It joined a tenant build after the rollout started: the build
            // moves it now (increment 16).
            let why = format!("it is in tenant build `{build}`, which moves it");
            rollouts
                .set_tenant(r.id, &space, TenantState::Skipped, &why, now)
                .await?;
            out.skipped.push((space, why));
        } else if !serving {
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
        out.deployment_moved = rollouts.deployment_step(&r, &ops.home, ACTOR, now).await?;
    }

    // **The first stage's gate** (10c). While it is pending only the first
    // stage moves. Its health is judged on every turn; it passes once the
    // first stage has nothing left to move and has been healthy for
    // `rollout.canary_wait`.
    let mut r = r;
    if r.gate == Gate::Pending {
        let (calls, faults) = crate::rollout_strategies::health_of(pool, r.id).await?;
        if crate::rollout_strategies::unhealthy(calls, faults, r.health_error_rate) {
            let why = format!(
                "{UNHEALTHY}: {faults} of {calls} calls to the tenants that moved failed on the \
                 server, at or over {HEALTH_ERROR_RATE_SETTING} {}. The rest stay on {}. \
                 `ordo rollout resume` moves them anyway; `ordo rollout abort` stops here, \
                 and `ordo rollback` moves the moved tenants back",
                crate::rollout_strategies::format_rate(r.health_error_rate),
                r.from.as_deref().unwrap_or("their version"),
            );
            rollouts
                .set_state(r.id, RolloutState::Paused, &why, now, "")
                .await?;
            ops.record(
                ACTOR,
                STATUS,
                "rollout-paused",
                format!(
                    "rollout={} target={} unhealthy calls={calls} faults={faults}",
                    r.id, r.target
                ),
            );
            alert(
                ops,
                &r,
                &format!("Rollout {} to {} paused: unhealthy", r.id, r.target),
                &why,
                now,
                out,
            )
            .await;
            out.paused = true;
            out.unhealthy = true;
            return Ok(());
        }
        let first_left: i64 = pool
            .rows(&format!(
                "SELECT count(*) FROM \"$rollout_tenants\" WHERE \"rollout\" = {} \
                 AND \"stage\" = 0 AND \"state\" IN ('queued', 'waiting', 'moving')",
                r.id
            ))
            .await?
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if first_left == 0 {
            if r.first_done_at == 0 {
                pool.rows(&format!(
                    "UPDATE \"$rollouts\" SET \"first_done_at\" = {now} WHERE \"id\" = {}",
                    r.id
                ))
                .await?;
                r.first_done_at = now;
            }
            let waited = now >= r.first_done_at.saturating_add(r.canary_wait.max(0) as u64);
            // **A quiet first stage has proved nothing** (ruled 2026-11-25):
            // fewer calls than `rollout.health_min_calls` when its wait ends
            // pause the rollout, naming the setting.
            if waited && crate::rollout_strategies::too_quiet(calls, r.health_min_calls) {
                let why = format!(
                    "{QUIET}: the tenants that moved first had {calls} call(s) while the \
                     rollout waited, fewer than {HEALTH_MIN_CALLS_SETTING} {}, so their \
                     health proves nothing. The rest stay on {}. `ordo rollout resume` moves \
                     them anyway; `ordo rollout set {HEALTH_MIN_CALLS_SETTING} <n> --reason \
                     \"...\"` lowers the number for the next rollout; `ordo rollout abort` \
                     stops here",
                    r.health_min_calls,
                    r.from.as_deref().unwrap_or("their version"),
                );
                rollouts
                    .set_state(r.id, RolloutState::Paused, &why, now, "")
                    .await?;
                ops.record(
                    ACTOR,
                    STATUS,
                    "rollout-paused",
                    format!(
                        "rollout={} target={} quiet calls={calls} min={}",
                        r.id, r.target, r.health_min_calls
                    ),
                );
                alert(
                    ops,
                    &r,
                    &format!("Rollout {} to {} paused: too few calls", r.id, r.target),
                    &why,
                    now,
                    out,
                )
                .await;
                out.paused = true;
                out.quiet = true;
                return Ok(());
            }
            if waited {
                pool.rows(&format!(
                    "UPDATE \"$rollouts\" SET \"gate\" = 'passed', \"last_batch_at\" = 0 \
                     WHERE \"id\" = {}",
                    r.id
                ))
                .await?;
                ops.record(
                    ACTOR,
                    STATUS,
                    "rollout-first-stage-healthy",
                    format!(
                        "rollout={} target={} calls={calls} faults={faults}",
                        r.id, r.target
                    ),
                );
                r.gate = Gate::Passed;
                r.last_batch_at = 0;
                out.gate_passed = true;
            }
        }
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
        alert(
            ops,
            &r,
            &format!("Rollout {} to {} paused: tenants failed", r.id, r.target),
            &why,
            now,
            out,
        )
        .await;
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
    // A first stage still judged keeps the rollout open, even with nothing
    // else to move: "active" flips only once it is healthy.
    if left == 0 && r.gate != Gate::Pending {
        if r.deployment_space == DeploymentSpace::Last {
            out.deployment_moved |= rollouts.deployment_step(&r, &ops.home, ACTOR, now).await?;
        }
        // "Active" flips last (Q5): sign-in and new tenants follow now.
        rollouts.activate(&r, ACTOR, now).await?;
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
        alert(
            ops,
            &r,
            &format!("Rollout {} to {} finished", r.id, r.target),
            &why,
            now,
            out,
        )
        .await;
        out.finished = true;
    }
    Ok(())
}

/// Tell the deployment owner on the rollout's channel beyond the stream
/// entry its caller wrote (§3.5 "told").
async fn alert(ops: &ApplyOps, r: &Rollout, subject: &str, body: &str, now: u64, out: &mut Ticked) {
    if let Err(why) =
        crate::rollout_strategies::tell(&ops.pool, &r.alert_channel, subject, body, now).await
    {
        ops.record(
            ACTOR,
            STATUS,
            "rollout-alert-failed",
            format!("rollout={} channel={} why={why}", r.id, r.alert_channel),
        );
        out.alert_failed = Some(why);
    }
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
    let pending: Vec<RolloutTenant> = rollouts
        .tenants(r.id)
        .await?
        .into_iter()
        .filter(|t| t.space != DEPLOYMENT_STEP && t.state.is_pending())
        .collect();
    // While the first stage's gate is pending, only the first stage moves.
    let first_only = r.gate == Gate::Pending;
    let candidates: Vec<RolloutTenant> = {
        let mut c: Vec<RolloutTenant> = pending
            .iter()
            .filter(|t| !first_only || t.stage == 0)
            .cloned()
            .collect();
        // The first stage first; then a move a dead leader left half done,
        // then the waiting, then the queue in order.
        c.sort_by_key(|t| {
            (
                t.stage,
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
    // A canary or a group moves its first stage in one batch; a query's
    // moves a batch at a time, in its order.
    let batch_size = if first_only && matches!(r.strategy, Strategy::Canary | Strategy::Group) {
        i64::MAX / 4
    } else {
        r.batch_size
    };
    // Share partners move together (§1.7, §3.5): a tenant's partners still
    // to move join its batch, whatever their stage.
    let partners = if r.share_partners {
        crate::rollout_strategies::partners(pool, Some(ops.pools.as_ref()), now).await?
    } else {
        BTreeMap::new()
    };
    let by_space: BTreeMap<String, RolloutTenant> = pending
        .iter()
        .map(|t| (t.space.clone(), t.clone()))
        .collect();
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
    // Each tenant with a database of its own, and that database.
    let own: BTreeMap<String, String> = match ordo_exec::spacedb::all(pool).await {
        Ok(a) => a
            .into_iter()
            .map(|(s, a)| (s.0, ordo_exec::route::target_label(&a.host, a.port, &a.db)))
            .collect(),
        Err(e) if e.is_missing_table() => BTreeMap::new(),
        Err(e) => return Err(e.into()),
    };
    // `backfill.concurrency` moves at once, at most `backfill.per_database`
    // of them in one database (increment 11b part 2).
    let limits = crate::backfill::stored_settings(pool)
        .await
        .unwrap_or_default();
    let at_once = limits.concurrency.max(1) as usize;
    let per_database = limits.per_database.max(1) as usize;
    let mut program: Option<Program> = None;
    let mut apply_lock: Option<Option<crate::apply_op::ApplyLock>> = None;
    let mut tried = 0i64;
    let mut work: std::collections::VecDeque<(RolloutTenant, bool)> =
        candidates.into_iter().map(|t| (t, false)).collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut wave: Vec<RolloutTenant> = Vec::new();
    let mut s = Settle {
        rollouts: &rollouts,
        r,
        now,
        partners: &partners,
        by_space: &by_space,
    };
    loop {
        let next = if wave.len() >= at_once {
            None
        } else {
            work.pop_front()
        };
        let Some((t, partner)) = next else {
            if wave.is_empty() {
                break;
            }
            let program = program
                .as_ref()
                .expect("compiled before a move joins a wave");
            let (moved, peak) = Box::pin(wave_of(
                ops,
                r,
                program,
                std::mem::take(&mut wave),
                &own,
                per_database,
                now,
            ))
            .await;
            out.at_once = out.at_once.max(peak);
            for (t, outcome) in moved {
                s.settle(t, outcome?, &seen, &mut work, out).await?;
            }
            continue;
        };
        if !seen.insert(t.space.clone()) {
            continue;
        }
        if !partner && tried >= batch_size {
            // The batch is full: what is queued waits for the next turn,
            // and the partners of the moves still running join this one.
            work.retain(|(_, p)| *p);
            continue;
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
        let outcome = if own.contains_key(&t.space) {
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
                    // It joins the wave, which runs once full or once the
                    // queue is empty.
                    wave.push(t);
                    continue;
                }
            }
        } else {
            // On the deployment's own database: its stored step as a move
            // with dual writes when its path is one (11b part 3), under the
            // apply lock; else a pin, that database holding the target.
            if apply_lock.is_none() {
                apply_lock = Some(crate::apply_op::ApplyLock::take(pool).await.ok().flatten());
            }
            match apply_lock.as_ref().and_then(|l| l.as_ref()) {
                None => Outcome::Waiting(
                    "an apply holds the apply lock; it is tried again on the next turn".into(),
                ),
                Some(_) => match Box::pin(by_stored_path(ops, r, &t.space, now)).await? {
                    Some(outcome) => outcome,
                    None => pin(pool, r, &t.space, now).await?,
                },
            }
        };
        s.settle(t, outcome, &seen, &mut work, out).await?;
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

/// What a turn does with each tenant's outcome: its state, the lists the
/// tick reports, and a moved tenant's share partners into the turn.
struct Settle<'a> {
    rollouts: &'a Rollouts<'a>,
    r: &'a Rollout,
    now: u64,
    partners: &'a BTreeMap<String, BTreeSet<String>>,
    by_space: &'a BTreeMap<String, RolloutTenant>,
}

impl Settle<'_> {
    async fn settle(
        &mut self,
        t: RolloutTenant,
        outcome: Outcome,
        seen: &BTreeSet<String>,
        work: &mut std::collections::VecDeque<(RolloutTenant, bool)>,
        out: &mut Ticked,
    ) -> Result<(), RolloutRefusal> {
        let (r, now) = (self.r, self.now);
        if matches!(outcome, Outcome::Moved) {
            for p in self.partners.get(&t.space).into_iter().flatten().rev() {
                if let Some(row) = self.by_space.get(p).filter(|_| !seen.contains(p)) {
                    work.push_front((row.clone(), true));
                }
            }
        }
        match outcome {
            Outcome::Moved => {
                self.rollouts
                    .set_tenant(r.id, &t.space, TenantState::Moved, "", now)
                    .await?;
                out.moved.push(t.space);
            }
            Outcome::Moving(why) => {
                self.rollouts
                    .set_tenant(r.id, &t.space, TenantState::Moving, &why, now)
                    .await?;
                out.waiting.push((t.space, why));
            }
            Outcome::Waiting(why) => {
                self.rollouts
                    .set_tenant(r.id, &t.space, TenantState::Waiting, &why, now)
                    .await?;
                out.waiting.push((t.space, why));
            }
            Outcome::Failed(why) => {
                self.rollouts
                    .set_tenant(r.id, &t.space, TenantState::Failed, &why, now)
                    .await?;
                out.failed.push((t.space, why));
            }
            Outcome::Gone(why) => {
                self.rollouts
                    .set_tenant(r.id, &t.space, TenantState::Skipped, &why, now)
                    .await?;
                out.skipped.push((t.space, why));
            }
        }
        Ok(())
    }
}

/// **One wave of moves, at once** (`backfill.concurrency`, increment 11b
/// part 2): each tenant with a database of its own in `wave` moves as
/// [`move_own`] moves it, all together, with at most `per_database` in one
/// database (`backfill.per_database`). Each tenant's outcome, in the order
/// given, and the most that ran at once.
async fn wave_of(
    ops: &ApplyOps,
    r: &Rollout,
    program: &Program,
    wave: Vec<RolloutTenant>,
    own: &BTreeMap<String, String>,
    per_database: usize,
    now: u64,
) -> (Vec<(RolloutTenant, Result<Outcome, RolloutRefusal>)>, usize) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let gates: BTreeMap<String, std::sync::Arc<tokio::sync::Semaphore>> = wave
        .iter()
        .filter_map(|t| own.get(&t.space))
        .map(|db| {
            (
                db.clone(),
                std::sync::Arc::new(tokio::sync::Semaphore::new(per_database)),
            )
        })
        .collect();
    let running = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let moved = futures::future::join_all(wave.into_iter().map(|t| {
        let gate = own.get(&t.space).and_then(|db| gates.get(db)).cloned();
        let (running, peak) = (&running, &peak);
        async move {
            let _held = match &gate {
                Some(g) => g.clone().acquire_owned().await.ok(),
                None => None,
            };
            let n = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            let outcome = Box::pin(move_own(ops, r, &t.space, program, now)).await;
            running.fetch_sub(1, Ordering::SeqCst);
            (t, outcome)
        }
    }))
    .await;
    (moved, peak.load(Ordering::SeqCst))
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

/// Move one tenant whose database is its own: through its stored path
/// when one leads to the target (increment 11b,
/// `crate::every_tenant::forward_tenant`), or else expand it to the target
/// (`schema_apply::bring_forward_from`); gate it, then pin it. A database
/// that cannot be reached waits (Q10); a change refused by name fails.
async fn move_own(
    ops: &ApplyOps,
    r: &Rollout,
    space: &str,
    program: &Program,
    now: u64,
) -> Result<Outcome, RolloutRefusal> {
    if let Some(outcome) = by_stored_path(ops, r, space, now).await? {
        return Ok(outcome);
    }
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

/// **The tenant's stored path to the target** (increment 11b): a move
/// with dual writes for a rewriting step, which pins the tenant at its
/// switch and starts its soak; a replay and a pin for any other. A
/// backfill takes `backfill.budget` chunks per turn, and the tenant is
/// `moving` until its switch. `None` when no stored path leads there.
async fn by_stored_path(
    ops: &ApplyOps,
    r: &Rollout,
    space: &str,
    now: u64,
) -> Result<Option<Outcome>, RolloutRefusal> {
    use crate::every_tenant::Forward;
    let ctx = match ops.step_context().await {
        Ok(c) => c,
        Err(e) => {
            return Ok(Some(Outcome::Waiting(format!(
                "the backfill settings could not be read ({e}); it is tried again"
            ))))
        }
    };
    let cx = crate::dual_writes::MoveContext {
        control: &ops.pool,
        pools: ops.pools.as_ref(),
        spaces: ops.spaces.get().map(|s| s.as_ref()),
    };
    let forward = Box::pin(crate::every_tenant::forward_tenant(
        &cx,
        &ctx,
        space,
        &r.target,
        match crate::dual_writes::stored_soak_secs(&ops.pool).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Some(Outcome::Waiting(format!(
                    "soak.length could not be read ({e}); it is tried again"
                ))))
            }
        },
        ACTOR,
    ))
    .await;
    Ok(Some(match forward {
        Ok(Forward::NoPath) => return Ok(None),
        Ok(Forward::Ready) => pin(&ops.pool, r, space, now).await?,
        Ok(Forward::Backfilling(why)) => Outcome::Moving(why),
        Ok(Forward::Switched(_)) => {
            Versions::on(&ops.pool).tag_rollout(space, r.id).await?;
            Outcome::Moved
        }
        Err(e) if e.class() == "unreachable" => Outcome::Waiting(format!(
            "its database cannot be reached ({e}); it is tried again"
        )),
        Err(e) if e.transient() => Outcome::Waiting(format!("{e}; it is tried again")),
        Err(e) => Outcome::Failed(format!("{}: {e}", e.class())),
    }))
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
        "canaryCount",
        "canaryWait",
        "healthErrorRate",
        "healthMinCalls",
        "alertChannel",
        "sharePartners",
        "selection",
        "gate",
        "firstDoneAt",
        "health",
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
                int("canaryCount"),
                text("canaryWait", 32, false),
                text("healthErrorRate", 16, false),
                int("healthMinCalls"),
                text("alertChannel", 320, false),
                flag("sharePartners"),
                text("group", 65_536, false),
                text(
                    "query",
                    crate::input_caps::InputCaps::current().query_chars as u32,
                    false,
                ),
                text(
                    "order",
                    crate::input_caps::InputCaps::current().query_chars as u32,
                    false,
                ),
                int("limit"),
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
    let rollouts = Rollouts::on(pool).reaching(ops.pools.as_ref());
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
                        why: format!("it is {STRATEGIES}"),
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
                canary_count: int("canaryCount"),
                canary_wait: match text("canaryWait") {
                    None => None,
                    Some(v) => Some(parse_secs(v).filter(|n| *n >= 0).ok_or_else(|| {
                        RolloutRefusal::Setting {
                            setting: CANARY_WAIT_SETTING.into(),
                            value: v.chars().take(40).collect(),
                            why: "it is a length, like `0s`, `30m` or `1h`".into(),
                        }
                        .to_exec_error(WHAT)
                    })?),
                },
                health_error_rate: match text("healthErrorRate") {
                    None => None,
                    Some(v) => Some(crate::rollout_strategies::parse_rate(v).ok_or_else(|| {
                        RolloutRefusal::Setting {
                            setting: HEALTH_ERROR_RATE_SETTING.into(),
                            value: v.chars().take(40).collect(),
                            why: "it is a percentage from 0.01% to 100%, like `1%`".into(),
                        }
                        .to_exec_error(WHAT)
                    })?),
                },
                health_min_calls: int("healthMinCalls"),
                alert_channel: text("alertChannel").map(str::to_string),
                share_partners: bound.0.get("sharePartners").and_then(|v| v.as_bool()),
                group: text("group").map(group_list),
                query: text("query").map(str::to_string),
                order: text("order").map(str::to_string),
                limit: int("limit"),
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
                .finish(&ops.home, caller, text("reason"), now)
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
        assert_eq!(setting_name("canary-count"), Some(CANARY_COUNT_SETTING));
        assert_eq!(setting_name("seats"), None);
        assert_eq!(
            check_setting(BATCH_INTERVAL_SETTING, Some("5m")).unwrap(),
            Some("300".into())
        );
        let e = check_setting(STRATEGY_SETTING, Some("blue-green")).unwrap_err();
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
