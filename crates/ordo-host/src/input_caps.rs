//! **The input caps are settings** (owner, 2026-11-25,
//! `settings-3-choices-left-after-the` ask 3, *"yes"*: the remaining input
//! caps become settings; `versions-increment-3-three-defaults-for` ask 3:
//! *"Not number should be hard coded"*).
//!
//! Five numbers were constants. Each is now a setting stored in
//! `$version_settings` beside the other version and rollout settings, with
//! who and why, set with `ordo versions set limits.<name> <n> --reason
//! "..."` or `std.deployment.setVersionSetting`, and cleared with `--clear`
//! for its default (today's value).
//!
//! | Setting | Default | Range | What it caps |
//! |---|---|---|---|
//! | `limits.label_chars` | 80 | 1 to 1,000 | a version's label |
//! | `limits.reason_chars` | 500 | 1 to 10,000 | the reason of any evolution command |
//! | `limits.build_name_chars` | 40 | 1 to 200 | a tenant build's name |
//! | `limits.rollout_group` | 1,000 | 1 to 100,000 | tenants in `rollout.group` |
//! | `limits.query_chars` | 2,000 | 1 to 100,000 | `rollout.query` and `rollout.order` |
//!
//! # How a check reads them
//!
//! The checks are plain functions with no database in hand
//! ([`crate::versions::check_reason`] has about 25 callers), so the values
//! in force sit in this process ([`InputCaps::current`]). [`refresh`] reads
//! them from the database; it runs when a setting changes and before every
//! `std.deployment.*` op, so a change made through another node is in force
//! here from that node's next op. A deployment's caps are the one set of its
//! process: two deployments in one process share them.
//!
//! A refusal that a cap makes names it.

use std::sync::atomic::{AtomicUsize, Ordering};

use ordo_api::{ErrorCode, ExecError};
use ordo_exec::{Pool, PoolError};
use ordo_fault::FaultClass;

/// `limits.label_chars`: the longest label a version may carry.
pub const LABEL_CHARS_SETTING: &str = "limits.label_chars";
/// `limits.reason_chars`: the longest reason an evolution command may carry.
pub const REASON_CHARS_SETTING: &str = "limits.reason_chars";
/// `limits.build_name_chars`: the longest name a tenant build may have.
pub const BUILD_NAME_CHARS_SETTING: &str = "limits.build_name_chars";
/// `limits.rollout_group`: the most tenants `rollout.group` may name.
pub const ROLLOUT_GROUP_SETTING: &str = "limits.rollout_group";
/// `limits.query_chars`: the longest `rollout.query` or `rollout.order`.
pub const QUERY_CHARS_SETTING: &str = "limits.query_chars";

/// Every input cap: name, default (what the constant was), and the range a
/// value may take.
pub const SETTINGS: [(&str, i64, i64, i64); 5] = [
    (LABEL_CHARS_SETTING, 80, 1, 1_000),
    (REASON_CHARS_SETTING, 500, 1, 10_000),
    (BUILD_NAME_CHARS_SETTING, 40, 1, 200),
    (ROLLOUT_GROUP_SETTING, 1_000, 1, 100_000),
    (QUERY_CHARS_SETTING, 2_000, 1, 100_000),
];

static LABEL: AtomicUsize = AtomicUsize::new(80);
static REASON: AtomicUsize = AtomicUsize::new(500);
static BUILD_NAME: AtomicUsize = AtomicUsize::new(40);
static GROUP: AtomicUsize = AtomicUsize::new(1_000);
static QUERY: AtomicUsize = AtomicUsize::new(2_000);

/// **The input caps in force.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputCaps {
    pub label_chars: usize,
    pub reason_chars: usize,
    pub build_name_chars: usize,
    pub rollout_group: usize,
    pub query_chars: usize,
}

impl Default for InputCaps {
    fn default() -> InputCaps {
        InputCaps {
            label_chars: 80,
            reason_chars: 500,
            build_name_chars: 40,
            rollout_group: 1_000,
            query_chars: 2_000,
        }
    }
}

impl InputCaps {
    /// What this process holds now.
    pub fn current() -> InputCaps {
        InputCaps {
            label_chars: LABEL.load(Ordering::Relaxed),
            reason_chars: REASON.load(Ordering::Relaxed),
            build_name_chars: BUILD_NAME.load(Ordering::Relaxed),
            rollout_group: GROUP.load(Ordering::Relaxed),
            query_chars: QUERY.load(Ordering::Relaxed),
        }
    }

    fn install(&self) {
        LABEL.store(self.label_chars, Ordering::Relaxed);
        REASON.store(self.reason_chars, Ordering::Relaxed);
        BUILD_NAME.store(self.build_name_chars, Ordering::Relaxed);
        GROUP.store(self.rollout_group, Ordering::Relaxed);
        QUERY.store(self.query_chars, Ordering::Relaxed);
    }

    fn put(&mut self, name: &str, n: i64) {
        let u = usize::try_from(n).unwrap_or(0);
        match name {
            LABEL_CHARS_SETTING => self.label_chars = u,
            REASON_CHARS_SETTING => self.reason_chars = u,
            BUILD_NAME_CHARS_SETTING => self.build_name_chars = u,
            ROLLOUT_GROUP_SETTING => self.rollout_group = u,
            QUERY_CHARS_SETTING => self.query_chars = u,
            _ => {}
        }
    }

    /// As the settings op answers them.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "labelChars": self.label_chars,
            "reasonChars": self.reason_chars,
            "buildNameChars": self.build_name_chars,
            "rolloutGroup": self.rollout_group,
            "queryChars": self.query_chars,
        })
    }

    /// The JSON key of one setting.
    pub fn json_key(name: &str) -> &'static str {
        match name {
            LABEL_CHARS_SETTING => "labelChars",
            REASON_CHARS_SETTING => "reasonChars",
            BUILD_NAME_CHARS_SETTING => "buildNameChars",
            ROLLOUT_GROUP_SETTING => "rolloutGroup",
            _ => "queryChars",
        }
    }
}

/// Why an input cap was not changed. The one error of this module.
#[derive(Debug)]
pub enum CapRefusal {
    /// A value or a name that is not one. Fix the input.
    Input(String),
    /// The database. Its class says whether a retry can help.
    Store(PoolError),
}

impl CapRefusal {
    /// **The one place a `CapRefusal` becomes a door answer.**
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            CapRefusal::Input(_) => ExecError::new(ErrorCode::InvalidParam, message),
            CapRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
        }
    }

    /// Whether a retry can help.
    pub fn retryable(&self) -> bool {
        match self {
            CapRefusal::Input(_) => false,
            CapRefusal::Store(e) => matches!(
                e.class(),
                FaultClass::Busy | FaultClass::Retryable | FaultClass::Broken
            ),
        }
    }
}

impl std::fmt::Display for CapRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapRefusal::Input(why) => write!(f, "{why}"),
            CapRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CapRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CapRefusal::Store(e) => Some(e),
            CapRefusal::Input(_) => None,
        }
    }
}

impl From<PoolError> for CapRefusal {
    fn from(e: PoolError) -> CapRefusal {
        CapRefusal::Store(e)
    }
}

/// The full name of an input cap from what a person typed:
/// `limits.label_chars`, `label_chars` or `label-chars`.
pub fn setting_name(text: &str) -> Option<&'static str> {
    let t = text.trim();
    let t = t.strip_prefix("limits.").unwrap_or(t).replace('-', "_");
    SETTINGS
        .into_iter()
        .map(|(n, ..)| n)
        .find(|n| n.strip_prefix("limits.") == Some(t.as_str()))
}

/// Check a value for an input cap, and answer it as stored. `None` clears
/// it, back to the default. A refusal names the setting, the range and the
/// command.
pub fn check_setting(name: &str, value: Option<&str>) -> Result<Option<String>, CapRefusal> {
    let Some((name, _, lo, hi)) = SETTINGS.into_iter().find(|(n, ..)| *n == name) else {
        return Err(CapRefusal::Input(format!(
            "`{}` is not an input cap; there are {}",
            name.chars().take(60).collect::<String>(),
            SETTINGS.map(|(n, ..)| n).join(", ")
        )));
    };
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    match v.parse::<i64>() {
        Ok(n) if (lo..=hi).contains(&n) => Ok(Some(n.to_string())),
        _ => Err(CapRefusal::Input(format!(
            "`{name}` is a whole number from {lo} to {hi}, not `{}`. Set it with `ordo \
             versions set {name} <n> --reason \"...\"`, or `--clear` for its default",
            v.chars().take(40).collect::<String>()
        ))),
    }
}

/// **Read the caps from the database and put them in force** in this
/// process. A deployment with no table, or no row, holds the defaults.
pub async fn refresh(pool: &Pool) -> Result<InputCaps, CapRefusal> {
    let rows = match pool
        .rows(
            "SELECT DISTINCT ON (\"setting\") \"setting\", \"value\" \
             FROM \"$version_settings\" WHERE \"setting\" LIKE 'limits.%' \
             ORDER BY \"setting\", \"seq\" DESC",
        )
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_missing_table() => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let mut out = InputCaps::default();
    for r in rows {
        let (Some(Some(name)), Some(Some(value))) = (r.first().cloned(), r.get(1).cloned()) else {
            continue;
        };
        if let Ok(Some(v)) = check_setting(&name, Some(&value)) {
            if let Ok(n) = v.parse::<i64>() {
                out.put(&name, n);
            }
        }
    }
    out.install();
    Ok(out)
}

/// **Change one input cap**, or clear it with `None`, with who and why.
/// Checked first ([`check_setting`]); written beside the version settings
/// and put in force here. `true` when it changed.
pub async fn set_setting(
    pool: &Pool,
    setting: &str,
    value: Option<&str>,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<(bool, InputCaps), CapRefusal> {
    let reason =
        crate::versions::check_reason(reason).map_err(|e| CapRefusal::Input(e.to_string()))?;
    let Some(name) = setting_name(setting) else {
        return Err(check_setting(setting, value).expect_err("not an input cap"));
    };
    let value = check_setting(name, value)?;
    let changed =
        crate::versions::record_named(pool, name, value.as_deref(), actor, &reason, at).await?;
    Ok((changed, refresh(pool).await?))
}
