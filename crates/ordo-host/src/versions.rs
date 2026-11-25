//! **`Versions`: the one client for stored versions and the pins**
//! (`plan-versions.md` §1.8, AGENTS.md rule 9, owner 2026-10-23).
//!
//! Everything that reads or writes `$artifacts`, `$builds`,
//! `$activations`, a tenant's pin (`$spaces.version`, `$spaces.pin_epoch`),
//! `$pins` or `$moves` goes through [`Versions`]. Its one error is
//! [`VersionRefusal`], and [`VersionRefusal::to_exec_error`] is the one
//! place it becomes an answer at the door.
//!
//! `ordo_exec::registry` stays the SQL layer beneath it for the registry's
//! own tables. The pins' SQL lives here, because nothing below `ordo-host`
//! knows what a tenant is.
//!
//! # The pin, increment 2
//!
//! A pin is the version a tenant is served (§1.3). This increment records
//! pins and does not route on them: the door still serves the one loaded
//! version, and increment 3 makes the pin pick it.
//!
//! - **Every tenant is pinned to the active version.** An activation
//!   re-pins every tenant that is `provisioning`, `active` or `paused`, in
//!   the activation's own transaction. A `failed` or `reaped` tenant serves
//!   nothing and keeps the pin it had.
//! - **A new tenant** is pinned when its row is written, cause
//!   `new_tenant`, to `versions.new_tenant` when set and to the active
//!   version otherwise (owner, 2026-11-02, D14).
//! - **A tenant made before pins** has none. [`Versions::backfill`] pins it
//!   to the active version at boot, cause `backfill`.
//! - Each pin change sets `$spaces.version`, adds 1 to `pin_epoch`, and
//!   appends a `$pins` row, in one transaction.
//!
//! A space the deployment owner named and attached by hand, with no
//! `$spaces` row, has nowhere to hold a pin and is not pinned. It is
//! served the active version, as every space is until increment 3.
//!
//! # Labels, commits and names (increment 2, session 2)
//!
//! A person names a stored version by its name or by its order number,
//! `#4` ([`Versions::resolve`]). A version may carry a **label**, one line
//! of at most [`MAX_LABEL_CHARS`] characters, and the **git commit** of the
//! source it was applied from ([`Provenance`]). Neither is part of the
//! version's identity.
//!
//! # Revoking a version (increment 2b)
//!
//! Only the deployment owner marks a version revoked (D8, owner
//! 2026-11-02), with a reason, and may lift the mark again (owner
//! 2026-11-04, ask 20). Both acts are rows in `$revocations`, append-only,
//! with actor, reason and time. A version is revoked when its newest row
//! is a revoke.
//!
//! - A revoked version is refused as an activation or rollback target, as
//!   `versions.new_tenant` (when it is set and when a tenant is pinned), and as
//!   any pin target. Each refusal names the revoke and the command that
//!   lifts it ([`VersionRefusal::Revoked`]).
//! - Revoking the active version is refused unless the deployment owner
//!   names where its tenants go (ask 18). With it, the activation and the
//!   revoke are one transaction, and the pins move as any activation moves
//!   them. Ordo never picks the version.
//! - Tenants on a revoked version stay there until they move off it
//!   (below).
//!
//! # Moving off a revoked version (increment 3, session 2)
//!
//! Ruled 2026-11-05: no deadline. A tenant on a revoked version moves in
//! its maintenance window, or on demand; with neither it stays.
//! [`Versions::move_off_revoked`] is the one move, called by the window
//! sweep (`crate::revoked_moves`), by the deployment owner or a system
//! (`std.deployment.moveOffRevoked`), and by the tenant when the app
//! developer grants it (`std.space.moveOffRevoked`, off by default).
//!
//! The target is the app developer's choice, read in this order:
//!
//! 1. the tenant's own, set by [`Versions::set_move_target`]
//!    (`$move_targets`);
//! 2. the revoke's, `--move-to` on a version that is not active
//!    (`$revocations.move_to`);
//! 3. the active version.
//!
//! A target that is revoked is refused, never passed over for the next:
//! Ordo picks nothing the developer did not choose beyond the default.

use std::collections::BTreeMap;

use ordo_api::{ErrorCode, ExecError};
use ordo_commandlog::Timestamp;
use ordo_exec::registry::{self, RegistryRefusal};
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_fault::FaultClass;

pub use ordo_exec::registry::{Activated, Activation, Applied, Artifact, Revision, DEPLOY_CHANNEL};

/// The setting naming the version a new tenant starts on (§7). Stored in
/// the control database ([`VersionSetting`], owner 2026-11-05).
pub const NEW_TENANT_SETTING: &str = "versions.new_tenant";
/// The environment variable that set it before 2026-11-05. Read once, to
/// adopt its value ([`Versions::adopt_env`]); ignored after.
pub const NEW_TENANT_ENV: &str = "ORDO_VERSIONS_NEW_TENANT";

/// The setting naming which versions this deployment serves (§1.6, D15).
pub const SERVED_SETTING: &str = "versions.served";
/// Its former environment variable, read only to adopt it.
pub const SERVED_ENV: &str = "ORDO_VERSIONS_SERVED";
/// The setting naming the oldest version a tenant may stay on (§1.6).
pub const MINIMUM_SETTING: &str = "versions.minimum";
/// Its former environment variable, read only to adopt it.
pub const MINIMUM_ENV: &str = "ORDO_VERSIONS_MINIMUM";

/// **The three version settings** (owner, 2026-11-05: *"that lives within
/// ordo and should likely not be an env because it is dependany on what is
/// applied"*). Each is about program versions, so each is stored in the
/// control database, `$version_settings`, with who set it, when and why.
/// They are set through the door (`std.deployment.setVersionSetting`), the
/// CLI (`ordo versions set`) and the console's Deploys screen. Every node
/// reads them where it needs them, and the pin stream's nudge tells a node
/// to read them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSetting {
    Served,
    Minimum,
    NewTenant,
    /// `versions.min_residency`: how long a loaded version stays loaded
    /// at the least, so a pin moved away and back does not compile it
    /// twice (`plan-versions.md` §1.4 step 5, §7).
    MinResidency,
    /// `versions.idle_evict_after`: a version beside the active one that
    /// served no request for this long is unloaded, and loaded again on the
    /// next signed-in request for one of its tenants (§1.5).
    IdleEvictAfter,
    /// `versions.restore_window`: how long the deployment owner keeps
    /// database backups and exit bundles. A version some tenant or the
    /// deployment was on within it is one a restore may still need, so it
    /// is not removed, and a field it declares is not pruned (§1.6, §2.7,
    /// increment 17). Unset, it is the deployment's backup retention, or 30
    /// days when Ordo does not know that (owner, 2026-11-25).
    RestoreWindow,
    /// `versions.max_loaded`: how many versions a node holds loaded at
    /// once, the active one included (§1.5). Owner, 2026-11-25: no number
    /// is hard coded.
    MaxLoaded,
    /// `versions.memory_budget`: the memory a node gives per-space views
    /// across every loaded version (§1.5).
    MemoryBudget,
    /// `versions.max_concurrent_loads`: versions a node compiles and gates
    /// at once.
    MaxConcurrentLoads,
    /// `versions.at_capacity`: what a load past `versions.max_loaded` does.
    /// `evict-idle` (the default, ruled 2026-11-25) pushes out the least
    /// recently used idle version; `refuse` refuses the load.
    AtCapacity,
    /// `versions.wake_wait`: how long a request for a tenant whose version
    /// was unloaded for idleness waits for it to load again.
    WakeWait,
}

/// `versions.max_loaded`.
pub const MAX_LOADED_SETTING: &str = "versions.max_loaded";
/// `versions.memory_budget`.
pub const MEMORY_BUDGET_SETTING: &str = "versions.memory_budget";
/// `versions.max_concurrent_loads`.
pub const MAX_CONCURRENT_LOADS_SETTING: &str = "versions.max_concurrent_loads";
/// `versions.at_capacity`.
pub const AT_CAPACITY_SETTING: &str = "versions.at_capacity";
/// `versions.wake_wait`.
pub const WAKE_WAIT_SETTING: &str = "versions.wake_wait";
/// Their defaults (§7): 8 versions, 512 MB, 1 load at once, 5 seconds.
pub const DEFAULT_MAX_LOADED: usize = 8;
pub const DEFAULT_MEMORY_BUDGET: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MAX_CONCURRENT_LOADS: usize = 1;
pub const DEFAULT_WAKE_WAIT_SECS: u64 = 5;
/// The bounds a value is checked against: `max_loaded` 1 to 256, loads at
/// once 1 to 16, the budget at least one view (256 KB), the wake wait 0
/// to 5 minutes, so a request never holds its connection longer.
pub const MAX_MAX_LOADED: usize = 256;
pub const MAX_CONCURRENT_LOADS: usize = 16;
pub const MIN_MEMORY_BUDGET: u64 = 256 * 1024;
pub const MAX_WAKE_WAIT_SECS: u64 = 300;

/// What a load past `versions.max_loaded` does (`versions.at_capacity`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AtCapacity {
    /// Push out the least recently used idle version beside the active
    /// one; refuse only when none is idle (owner, 2026-11-25).
    #[default]
    EvictIdle,
    /// Refuse the load; the version's tenants stay closed on the node.
    Refuse,
}

impl AtCapacity {
    pub fn parse(v: &str) -> Option<AtCapacity> {
        match v.trim() {
            "evict-idle" | "evict_idle" => Some(AtCapacity::EvictIdle),
            "refuse" => Some(AtCapacity::Refuse),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AtCapacity::EvictIdle => "evict-idle",
            AtCapacity::Refuse => "refuse",
        }
    }
}

/// `512MB`, `2GB`, `256KB` or a number of bytes.
pub fn parse_bytes(v: &str) -> Option<u64> {
    let t = v.trim().to_ascii_uppercase();
    let (n, unit) = match t.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => (&t[..i], t[i..].trim()),
        None => (t.as_str(), ""),
    };
    let n: u64 = n.parse().ok()?;
    let mul = match unit {
        "" | "B" => 1,
        "KB" | "K" => 1024,
        "MB" | "M" => 1024 * 1024,
        "GB" | "G" => 1024 * 1024 * 1024,
        _ => return None,
    };
    n.checked_mul(mul)
}

/// **Check a value for a setting that names no version**, and answer it
/// as stored (trimmed). `None` when the value is not of the setting's
/// form; [`guard_form`] says what the form is.
pub fn check_guard(setting: VersionSetting, value: &str) -> Option<String> {
    let v = value.trim();
    let count = |hi: usize| match v.parse::<usize>() {
        Ok(n) if (1..=hi).contains(&n) => Some(n.to_string()),
        _ => None,
    };
    match setting {
        VersionSetting::MaxLoaded => count(MAX_MAX_LOADED),
        VersionSetting::MaxConcurrentLoads => count(MAX_CONCURRENT_LOADS),
        VersionSetting::MemoryBudget => parse_bytes(v)
            .filter(|b| *b >= MIN_MEMORY_BUDGET)
            .map(|_| v.to_string()),
        VersionSetting::AtCapacity => AtCapacity::parse(v).map(|a| a.as_str().to_string()),
        VersionSetting::WakeWait => guard_secs(setting, v)
            .flatten()
            .filter(|s| *s <= MAX_WAKE_WAIT_SECS)
            .map(|_| v.to_string()),
        VersionSetting::MinResidency
        | VersionSetting::IdleEvictAfter
        | VersionSetting::RestoreWindow => guard_secs(setting, v).map(|_| v.to_string()),
        VersionSetting::Served | VersionSetting::Minimum | VersionSetting::NewTenant => None,
    }
}

/// What a value of `setting` looks like, for its refusal.
pub fn guard_form(setting: &str) -> String {
    match VersionSetting::from_name(setting) {
        Some(VersionSetting::MaxLoaded) => {
            format!("a whole number of versions from 1 to {MAX_MAX_LOADED}")
        }
        Some(VersionSetting::MaxConcurrentLoads) => {
            format!("a whole number of loads from 1 to {MAX_CONCURRENT_LOADS}")
        }
        Some(VersionSetting::MemoryBudget) => {
            "a size like `512MB` or `2GB`, at least `256KB`".to_string()
        }
        Some(VersionSetting::AtCapacity) => "`evict-idle` or `refuse`".to_string(),
        Some(VersionSetting::WakeWait) => format!(
            "a duration like `5s` or `1m`, or `0`, at most {} minutes",
            MAX_WAKE_WAIT_SECS / 60
        ),
        Some(VersionSetting::IdleEvictAfter) => {
            "a duration like `10m`, `30s` or `2h`, or `0`, or `off`".to_string()
        }
        _ => "a duration like `10m`, `30s` or `2h`, or `0`".to_string(),
    }
}

/// `versions.min_residency`.
pub const MIN_RESIDENCY_SETTING: &str = "versions.min_residency";
/// `versions.idle_evict_after`.
pub const IDLE_EVICT_SETTING: &str = "versions.idle_evict_after";
/// `versions.restore_window`.
pub const RESTORE_WINDOW_SETTING: &str = "versions.restore_window";
/// Their defaults, in seconds: 10 minutes and 30 minutes (§7).
pub const DEFAULT_MIN_RESIDENCY_SECS: u64 = 600;
pub const DEFAULT_IDLE_EVICT_SECS: u64 = 1_800;
/// `versions.restore_window` when unset and the deployment's backup
/// retention is not known: 30 days (owner, 2026-11-25).
pub const DEFAULT_RESTORE_WINDOW_SECS: u64 = 30 * 86_400;

/// A load guard's value: a duration like `10m`, `30s` or `2h`, `0`, or
/// for `versions.idle_evict_after` also `off`. `None` when it is none of
/// these.
pub fn guard_secs(setting: VersionSetting, value: &str) -> Option<Option<u64>> {
    let v = value.trim();
    if v == "off" && setting == VersionSetting::IdleEvictAfter {
        return Some(None);
    }
    if v == "0" {
        return Some(Some(0));
    }
    crate::share_ops::duration_secs(v).map(Some)
}

impl VersionSetting {
    pub const ALL: [VersionSetting; 11] = [
        VersionSetting::Served,
        VersionSetting::Minimum,
        VersionSetting::NewTenant,
        VersionSetting::MinResidency,
        VersionSetting::IdleEvictAfter,
        VersionSetting::RestoreWindow,
        VersionSetting::MaxLoaded,
        VersionSetting::MemoryBudget,
        VersionSetting::MaxConcurrentLoads,
        VersionSetting::AtCapacity,
        VersionSetting::WakeWait,
    ];

    /// Names no version: the load guards, the residency limits and the
    /// restore window. Each is checked alone ([`check_guard`]).
    pub fn is_guard(self) -> bool {
        !matches!(
            self,
            VersionSetting::Served | VersionSetting::Minimum | VersionSetting::NewTenant
        )
    }

    /// `versions.served`, `versions.minimum`, `versions.new_tenant`.
    pub fn name(self) -> &'static str {
        match self {
            VersionSetting::Served => SERVED_SETTING,
            VersionSetting::Minimum => MINIMUM_SETTING,
            VersionSetting::NewTenant => NEW_TENANT_SETTING,
            VersionSetting::MinResidency => MIN_RESIDENCY_SETTING,
            VersionSetting::IdleEvictAfter => IDLE_EVICT_SETTING,
            VersionSetting::RestoreWindow => RESTORE_WINDOW_SETTING,
            VersionSetting::MaxLoaded => MAX_LOADED_SETTING,
            VersionSetting::MemoryBudget => MEMORY_BUDGET_SETTING,
            VersionSetting::MaxConcurrentLoads => MAX_CONCURRENT_LOADS_SETTING,
            VersionSetting::AtCapacity => AT_CAPACITY_SETTING,
            VersionSetting::WakeWait => WAKE_WAIT_SETTING,
        }
    }

    /// The environment variable it was read from before it was stored
    /// (2026-11-05 for the first three, 2026-11-25 for the node limits).
    /// Empty for one that never was.
    pub fn env(self) -> &'static str {
        match self {
            VersionSetting::Served => SERVED_ENV,
            VersionSetting::Minimum => MINIMUM_ENV,
            VersionSetting::NewTenant => NEW_TENANT_ENV,
            VersionSetting::MaxLoaded => "ORDO_VERSIONS_MAX_LOADED",
            VersionSetting::MemoryBudget => "ORDO_VERSIONS_MEMORY_BUDGET",
            VersionSetting::MaxConcurrentLoads => "ORDO_VERSIONS_MAX_CONCURRENT_LOADS",
            // Never an environment variable: stored in Ordo from the start.
            VersionSetting::MinResidency
            | VersionSetting::IdleEvictAfter
            | VersionSetting::RestoreWindow
            | VersionSetting::AtCapacity
            | VersionSetting::WakeWait => "",
        }
    }

    /// The command that sets it.
    pub fn command(self) -> &'static str {
        match self {
            VersionSetting::Served => "ordo versions set served",
            VersionSetting::Minimum => "ordo versions set minimum",
            VersionSetting::NewTenant => "ordo versions set new-tenant",
            VersionSetting::MinResidency => "ordo versions set min-residency",
            VersionSetting::IdleEvictAfter => "ordo versions set idle-evict-after",
            VersionSetting::RestoreWindow => "ordo versions set restore-window",
            VersionSetting::MaxLoaded => "ordo versions set max-loaded",
            VersionSetting::MemoryBudget => "ordo versions set memory-budget",
            VersionSetting::MaxConcurrentLoads => "ordo versions set max-concurrent-loads",
            VersionSetting::AtCapacity => "ordo versions set at-capacity",
            VersionSetting::WakeWait => "ordo versions set wake-wait",
        }
    }

    /// What it means when it is not set.
    pub fn unset_reads(self) -> &'static str {
        match self {
            VersionSetting::Served => "in-use",
            VersionSetting::Minimum => "none",
            VersionSetting::NewTenant => "the active version",
            VersionSetting::MinResidency => "10m",
            VersionSetting::IdleEvictAfter => "30m",
            VersionSetting::RestoreWindow => "30d, or your backup retention when known",
            VersionSetting::MaxLoaded => "8",
            VersionSetting::MemoryBudget => "512MB",
            VersionSetting::MaxConcurrentLoads => "1",
            VersionSetting::AtCapacity => "evict-idle",
            VersionSetting::WakeWait => "5s",
        }
    }

    /// From its name (`versions.served`), its short name (`served`), or
    /// `new-tenant` / `new_tenant`.
    pub fn parse(text: &str) -> Option<VersionSetting> {
        let t = text.trim();
        let t = t.strip_prefix("versions.").unwrap_or(t);
        match t {
            "served" => Some(VersionSetting::Served),
            "minimum" => Some(VersionSetting::Minimum),
            "new_tenant" | "new-tenant" | "newTenant" => Some(VersionSetting::NewTenant),
            "min_residency" | "min-residency" | "minResidency" => {
                Some(VersionSetting::MinResidency)
            }
            "idle_evict_after" | "idle-evict-after" | "idleEvictAfter" => {
                Some(VersionSetting::IdleEvictAfter)
            }
            "restore_window" | "restore-window" | "restoreWindow" => {
                Some(VersionSetting::RestoreWindow)
            }
            "max_loaded" | "max-loaded" | "maxLoaded" => Some(VersionSetting::MaxLoaded),
            "memory_budget" | "memory-budget" | "memoryBudget" => {
                Some(VersionSetting::MemoryBudget)
            }
            "max_concurrent_loads" | "max-concurrent-loads" | "maxConcurrentLoads" => {
                Some(VersionSetting::MaxConcurrentLoads)
            }
            "at_capacity" | "at-capacity" | "atCapacity" => Some(VersionSetting::AtCapacity),
            "wake_wait" | "wake-wait" | "wakeWait" => Some(VersionSetting::WakeWait),
            _ => None,
        }
    }

    fn from_name(name: &str) -> Option<VersionSetting> {
        VersionSetting::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// The command a refusal names for `setting`.
fn command_of(setting: &str) -> &'static str {
    VersionSetting::from_name(setting)
        .unwrap_or(VersionSetting::NewTenant)
        .command()
}

/// The three settings as stored. `Default` is every one unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionSettings {
    pub policy: ServedPolicy,
    pub new_tenant: Option<String>,
    /// `versions.min_residency` as stored, `None` for the default.
    pub min_residency: Option<String>,
    /// `versions.idle_evict_after` as stored, `None` for the default.
    pub idle_evict_after: Option<String>,
    /// `versions.restore_window` as stored, `None` when unset.
    pub restore_window: Option<String>,
    /// `versions.max_loaded`, `versions.memory_budget`,
    /// `versions.max_concurrent_loads`, `versions.at_capacity` and
    /// `versions.wake_wait` as stored, `None` for the default.
    pub max_loaded: Option<String>,
    pub memory_budget: Option<String>,
    pub max_concurrent_loads: Option<String>,
    pub at_capacity: Option<String>,
    pub wake_wait: Option<String>,
}

impl VersionSettings {
    /// The value of one, as it is stored: `None` when unset.
    pub fn value(&self, s: VersionSetting) -> Option<String> {
        match s {
            VersionSetting::Served => self.policy.served.as_ref().map(|l| l.join(",")),
            VersionSetting::Minimum => self.policy.minimum.clone(),
            VersionSetting::NewTenant => self.new_tenant.clone(),
            VersionSetting::MinResidency => self.min_residency.clone(),
            VersionSetting::IdleEvictAfter => self.idle_evict_after.clone(),
            VersionSetting::RestoreWindow => self.restore_window.clone(),
            VersionSetting::MaxLoaded => self.max_loaded.clone(),
            VersionSetting::MemoryBudget => self.memory_budget.clone(),
            VersionSetting::MaxConcurrentLoads => self.max_concurrent_loads.clone(),
            VersionSetting::AtCapacity => self.at_capacity.clone(),
            VersionSetting::WakeWait => self.wake_wait.clone(),
        }
    }

    /// The stored value of `s` checked again, `None` when unset or when it
    /// would be refused now (written by hand): the default then holds.
    fn checked(&self, s: VersionSetting) -> Option<String> {
        self.value(s).and_then(|v| check_guard(s, &v))
    }

    /// `versions.max_loaded`: the stored value or 8.
    pub fn max_loaded(&self) -> usize {
        self.checked(VersionSetting::MaxLoaded)
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_LOADED)
    }

    /// `versions.memory_budget` in bytes: the stored value or 512 MB.
    pub fn memory_budget(&self) -> u64 {
        self.checked(VersionSetting::MemoryBudget)
            .and_then(|v| parse_bytes(&v))
            .unwrap_or(DEFAULT_MEMORY_BUDGET)
    }

    /// `versions.max_concurrent_loads`: the stored value or 1.
    pub fn max_concurrent_loads(&self) -> usize {
        self.checked(VersionSetting::MaxConcurrentLoads)
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_CONCURRENT_LOADS)
    }

    /// `versions.at_capacity`: the stored value or `evict-idle`.
    pub fn at_capacity(&self) -> AtCapacity {
        self.checked(VersionSetting::AtCapacity)
            .and_then(|v| AtCapacity::parse(&v))
            .unwrap_or_default()
    }

    /// `versions.wake_wait` in seconds: the stored value or 5.
    pub fn wake_wait_secs(&self) -> u64 {
        self.checked(VersionSetting::WakeWait)
            .and_then(|v| guard_secs(VersionSetting::WakeWait, &v).flatten())
            .unwrap_or(DEFAULT_WAKE_WAIT_SECS)
    }

    /// `versions.restore_window` in seconds, as set. `None` when unset.
    pub fn restore_window_set_secs(&self) -> Option<u64> {
        self.restore_window
            .as_deref()
            .and_then(|v| guard_secs(VersionSetting::RestoreWindow, v))
            .flatten()
    }

    /// `versions.restore_window` in seconds: the stored value, or when
    /// unset the deployment's backup retention (`backup_retention`, which
    /// Ordo does not learn from any setting today), or 30 days.
    pub fn restore_window_secs(&self, backup_retention: Option<u64>) -> u64 {
        self.restore_window_set_secs()
            .or(backup_retention)
            .unwrap_or(DEFAULT_RESTORE_WINDOW_SECS)
    }

    /// `versions.min_residency` in seconds: the stored value, or the
    /// default when unset or unreadable.
    pub fn min_residency_secs(&self) -> u64 {
        self.min_residency
            .as_deref()
            .and_then(|v| guard_secs(VersionSetting::MinResidency, v))
            .flatten()
            .unwrap_or(DEFAULT_MIN_RESIDENCY_SECS)
    }

    /// `versions.idle_evict_after` in seconds, `None` when it is `off`.
    pub fn idle_evict_secs(&self) -> Option<u64> {
        match self.idle_evict_after.as_deref() {
            None => Some(DEFAULT_IDLE_EVICT_SECS),
            Some(v) => guard_secs(VersionSetting::IdleEvictAfter, v)
                .unwrap_or(Some(DEFAULT_IDLE_EVICT_SECS)),
        }
    }

    /// The census line.
    pub fn describe(&self) -> String {
        format!(
            "{}   new tenants: {}",
            self.policy.describe(),
            self.new_tenant.as_deref().unwrap_or("the active version")
        )
    }

    fn with(&self, s: VersionSetting, value: Option<String>) -> VersionSettings {
        let mut out = self.clone();
        match s {
            VersionSetting::Served => {
                out.policy.served = value.map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|x| !x.is_empty())
                        .map(str::to_string)
                        .collect()
                })
            }
            VersionSetting::Minimum => out.policy.minimum = value,
            VersionSetting::NewTenant => out.new_tenant = value,
            VersionSetting::MinResidency => out.min_residency = value,
            VersionSetting::IdleEvictAfter => out.idle_evict_after = value,
            VersionSetting::RestoreWindow => out.restore_window = value,
            VersionSetting::MaxLoaded => out.max_loaded = value,
            VersionSetting::MemoryBudget => out.memory_budget = value,
            VersionSetting::MaxConcurrentLoads => out.max_concurrent_loads = value,
            VersionSetting::AtCapacity => out.at_capacity = value,
            VersionSetting::WakeWait => out.wake_wait = value,
        }
        out
    }
}

/// One change to a version setting, as `$version_settings` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingChange {
    pub seq: i64,
    pub setting: String,
    /// `None`: cleared, back to the default.
    pub value: Option<String>,
    pub actor: String,
    pub reason: String,
    pub at: u64,
}

/// What a setting change did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingSet {
    /// Recorded; the settings now.
    Changed(VersionSettings),
    /// The setting already held that value; nothing was written.
    Unchanged(VersionSettings),
}

impl SettingSet {
    pub fn settings(&self) -> &VersionSettings {
        match self {
            SettingSet::Changed(s) | SettingSet::Unchanged(s) => s,
        }
    }
}

/// The most history rows one read answers.
pub const MAX_SETTING_HISTORY: usize = 200;

/// **Which versions stay served** (`plan-versions.md` §1.6; ruled
/// 2026-10-01 and 2026-11-02, D15: the app developer decides).
///
/// - `served`: `None` is `in-use`, every version some tenant is pinned to.
///   A list names the only versions a tenant may be pinned to beside the
///   active one.
/// - `minimum`: tenants on a version stored before it move in their
///   maintenance window, as off a revoked version, and nothing is pinned
///   below it.
///
/// A change that would strand a tenant is refused when it is made, naming
/// the setting and the value to add ([`Versions::check_policy`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServedPolicy {
    pub served: Option<Vec<String>>,
    pub minimum: Option<String>,
}

impl ServedPolicy {
    /// From the two settings' text. Empty is unset.
    pub fn parse(served: Option<&str>, minimum: Option<&str>) -> ServedPolicy {
        let served = served.map(str::trim).filter(|s| !s.is_empty());
        ServedPolicy {
            served: match served {
                None | Some("in-use") => None,
                Some(list) => Some(
                    list.split(',')
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                        .collect(),
                ),
            },
            minimum: minimum
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
        }
    }

    /// The census line.
    pub fn describe(&self) -> String {
        format!(
            "served: {}   minimum: {}",
            self.served
                .as_ref()
                .map(|l| l.join(","))
                .unwrap_or_else(|| "in-use".into()),
            self.minimum.as_deref().unwrap_or("none")
        )
    }
}

/// The longest label a version may carry, in characters.
pub const MAX_LABEL_CHARS: usize = 80;

/// The longest reason a revoke or unrevoke may carry, in characters.
pub const MAX_REASON_CHARS: usize = 500;

/// The most tenants [`Versions::pinned_to_revoked`] names for one version.
/// The count is always whole.
pub const MAX_REVOKED_TENANTS: usize = 1_000;

/// The longest git commit a version may record: a SHA-256 commit and
/// `-dirty`.
pub const MAX_COMMIT_CHARS: usize = 64 + "-dirty".len();

/// What a person said about a version, apart from its content: its label
/// and the git commit of the source it was applied from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provenance {
    pub label: Option<String>,
    pub git_commit: Option<String>,
}

/// A label as stored: trimmed, one line, at most [`MAX_LABEL_CHARS`]
/// characters. Anything else is refused, saying the limit.
pub fn check_label(text: &str) -> Result<String, VersionRefusal> {
    let label = text.trim();
    let n = label.chars().count();
    if n == 0 {
        return Err(VersionRefusal::Label(format!(
            "a label is 1 to {MAX_LABEL_CHARS} characters; this one is empty. \
             `ordo versions label <version> --clear` removes a label"
        )));
    }
    if label.chars().any(char::is_control) {
        return Err(VersionRefusal::Label(format!(
            "a label is one line of at most {MAX_LABEL_CHARS} characters; this one has a \
             line break or another control character"
        )));
    }
    if n > MAX_LABEL_CHARS {
        return Err(VersionRefusal::Label(format!(
            "a label is at most {MAX_LABEL_CHARS} characters; this one is {n}"
        )));
    }
    Ok(label.to_string())
}

/// A git commit as stored: 7 to 64 lowercase hex digits, then `-dirty`
/// when the applied files had changes not in it.
pub fn check_git_commit(text: &str) -> Result<String, VersionRefusal> {
    let commit = text.trim();
    let hex = commit.strip_suffix("-dirty").unwrap_or(commit);
    if (7..=64).contains(&hex.len())
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Ok(commit.to_string());
    }
    Err(VersionRefusal::GitCommit(format!(
        "`{}` is not a git commit: 7 to 64 lowercase hex digits, then `-dirty` or nothing",
        commit.chars().take(MAX_COMMIT_CHARS).collect::<String>()
    )))
}

/// A revoke's or unrevoke's reason as stored: trimmed, 1 to
/// [`MAX_REASON_CHARS`] characters, any text. Anything else is refused,
/// saying the limit.
pub fn check_reason(text: &str) -> Result<String, VersionRefusal> {
    let reason = text.trim();
    let n = reason.chars().count();
    if n == 0 {
        return Err(VersionRefusal::Reason(format!(
            "a reason is 1 to {MAX_REASON_CHARS} characters; this one is empty. \
             `--reason \"...\"` says why, and stays on record"
        )));
    }
    if n > MAX_REASON_CHARS {
        return Err(VersionRefusal::Reason(format!(
            "a reason is at most {MAX_REASON_CHARS} characters; this one is {n}"
        )));
    }
    Ok(reason.to_string())
}

/// A commit as a person reads it: 12 hex digits, and `-dirty` when it was.
pub fn short_commit(commit: &str) -> String {
    let (hex, dirty) = match commit.strip_suffix("-dirty") {
        Some(h) => (h, "-dirty"),
        None => (commit, ""),
    };
    format!("{}{dirty}", hex.get(..12).unwrap_or(hex))
}

/// Who a pin change made at boot, by no person, is recorded as.
pub const SYSTEM_ACTOR: &str = "ordo:versions";

/// The tenant states that hold a pin. The same three the own-of latch
/// covers (`crate::tenants`): a tenant that is being built, serving, or
/// held still. Kept as SQL because every query below uses it so.
const PINNED_STATES: &str = "('provisioning', 'active', 'paused')";

/// `$pins` and `$moves`, in the control database beside `$spaces`.
///
/// `$pins` is append-only: one row per pin change, written in the pin
/// change's own transaction. The plan's rollout id joins it when rollouts
/// exist (increment 10).
///
/// `$moves` holds one open move per tenant: from, to, phase and soak
/// deadline (§2.1). Written since increment 7 ([`Versions::activate_with`]):
/// a tenant whose own database has not yet taken the new version stays on
/// its version, and its move says why (`reason`, added by
/// [`ensure_revocations`] to a table made before it).
const PIN_TABLES_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$pins\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"space\" text NOT NULL, \
  \"from\" text NULL, \
  \"to\" text NOT NULL, \
  \"epoch\" bigint NOT NULL, \
  \"actor\" text NOT NULL, \
  \"cause\" text NOT NULL, \
  \"at\" bigint NOT NULL \
); \
CREATE INDEX IF NOT EXISTS \"$pins_space\" ON \"$pins\" (\"space\", \"seq\"); \
CREATE TABLE IF NOT EXISTS \"$moves\" ( \
  \"space\" text PRIMARY KEY, \
  \"from\" text NOT NULL, \
  \"to\" text NOT NULL, \
  \"phase\" text NOT NULL, \
  \"soak_until\" bigint NOT NULL DEFAULT 0, \
  \"started_at\" bigint NOT NULL, \
  \"reason\" text NOT NULL DEFAULT '', \
  \"step\" text NOT NULL DEFAULT '', \
  \"overlay\" text NOT NULL DEFAULT '' \
)";

/// `$tenant_builds` and `$tenant_build_members` (`plan-versions.md` §3.8,
/// increment 16): each named build and its version, and which tenants
/// each holds, one build per tenant. A member's row names a tenant and
/// holds none of its data.
const TENANT_BUILDS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$tenant_builds\" ( \
  \"name\" text PRIMARY KEY, \
  \"version\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"at\" bigint NOT NULL \
); \
CREATE TABLE IF NOT EXISTS \"$tenant_build_members\" ( \
  \"space\" text PRIMARY KEY, \
  \"build\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

/// **Not in a tenant build** (`plan-versions.md` §3.8, increment 16): an
/// `AND` on `"$space"` that leaves out every tenant a build holds. An
/// activation, a rollout and the revoked-version sweep move none of them;
/// only the build does (`crate::tenant_builds`).
pub const NOT_IN_A_BUILD: &str =
    "AND \"$space\" NOT IN (SELECT \"space\" FROM \"$tenant_build_members\")";

/// `$revocations`, beside `$artifacts`: each revoke and unrevoke of a
/// version, append-only. A version is revoked when its newest row says
/// `revoke`.
const REVOCATIONS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$revocations\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"version\" text NOT NULL, \
  \"act\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"at\" bigint NOT NULL, \
  \"move_to\" text NULL \
); \
CREATE INDEX IF NOT EXISTS \"$revocations_version\" ON \"$revocations\" (\"version\", \"seq\")";

/// `$version_removals`, beside `$artifacts`: each removal of a stored
/// version (§1.6), append-only: who, why, where its tenants went and how
/// many. The version's own row stays as its tombstone.
const REMOVALS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$version_removals\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"version\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"moved_to\" text NULL, \
  \"moved\" bigint NOT NULL, \
  \"at\" bigint NOT NULL \
)";

/// One removal of a stored version, as `$version_removals` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub version: String,
    pub actor: String,
    pub reason: String,
    pub moved_to: Option<String>,
    pub moved: u64,
    pub at: u64,
}

/// `$move_targets`, beside `$spaces`: where one tenant goes when it moves
/// off a revoked version, set by the deployment owner (ruled 2026-11-05).
/// It names a tenant and holds none of its data, so erasure deletes its row
/// (`ordo_exec::erase::ATTACHMENTS`).
const MOVE_TARGETS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$move_targets\" ( \
  \"space\" text PRIMARY KEY, \
  \"version\" text NOT NULL, \
  \"set_by\" text NOT NULL, \
  \"set_at\" bigint NOT NULL \
)";

/// Create `$revocations`. Every transaction that pins reads it, and a
/// statement that fails inside a transaction aborts it, so each makes sure
/// of the table first. Retried once, for [`ensure_pin_tables`]' reason.
async fn ensure_revocations(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(REVOCATIONS_DDL).await.is_err() {
        pool.rows(REVOCATIONS_DDL).await?;
    }
    // `move_to` came with increment 3 (2026-11-05). Added only when it is
    // missing: `ADD COLUMN` locks the table exclusively even when the
    // column is there, and every pin change runs this.
    match pool
        .rows("SELECT \"move_to\" FROM \"$revocations\" LIMIT 0")
        .await
    {
        Err(e) if e.is_missing_column() => {
            pool.rows(
                "ALTER TABLE \"$revocations\" ADD COLUMN IF NOT EXISTS \"move_to\" text NULL",
            )
            .await?;
        }
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    if pool.rows(MOVE_TARGETS_DDL).await.is_err() {
        pool.rows(MOVE_TARGETS_DDL).await?;
    }
    // `$moves.reason` came with increment 7 (2026-11-23), on the same terms
    // as `move_to` above.
    match pool.rows("SELECT \"reason\" FROM \"$moves\" LIMIT 0").await {
        Err(e) if e.is_missing_column() => {
            pool.rows(
                "ALTER TABLE \"$moves\" ADD COLUMN IF NOT EXISTS \"reason\" text NOT NULL \
                 DEFAULT ''",
            )
            .await?;
        }
        Err(e) if e.is_missing_table() => ensure_pin_tables(pool).await?,
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    // `$moves.step` and `overlay` came with increment 11a (dual writes).
    match pool
        .rows("SELECT \"step\", \"overlay\" FROM \"$moves\" LIMIT 0")
        .await
    {
        Err(e) if e.is_missing_column() => {
            pool.rows(
                "ALTER TABLE \"$moves\" ADD COLUMN IF NOT EXISTS \"step\" text NOT NULL \
                 DEFAULT ''; ALTER TABLE \"$moves\" ADD COLUMN IF NOT EXISTS \"overlay\" text \
                 NOT NULL DEFAULT ''",
            )
            .await?;
        }
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    if pool.rows(SETTINGS_DDL).await.is_err() {
        pool.rows(SETTINGS_DDL).await?;
    }
    // `$pins.rollout` came with increment 10 (2026-11-23): the rollout a
    // pin change was part of, NULL for every other cause.
    match pool.rows("SELECT \"rollout\" FROM \"$pins\" LIMIT 0").await {
        Err(e) if e.is_missing_column() => {
            pool.rows("ALTER TABLE \"$pins\" ADD COLUMN IF NOT EXISTS \"rollout\" bigint NULL")
                .await?;
        }
        Err(e) if e.is_missing_table() => ensure_pin_tables(pool).await?,
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    Ok(())
}

/// `$version_settings`, beside `$revocations`: every change to the three
/// version settings ([`VersionSetting`]), append-only, with who, why and
/// when. A setting's value is its newest row's; `value` NULL is unset.
pub(crate) const SETTINGS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$version_settings\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"setting\" text NOT NULL, \
  \"value\" text NULL, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"at\" bigint NOT NULL \
); \
CREATE INDEX IF NOT EXISTS \"$version_settings_setting\" \
  ON \"$version_settings\" (\"setting\", \"seq\")";

/// **One named setting's stored value** in `$version_settings`, for the
/// settings stored beside the version settings by other modules (the
/// window caps, `soak.length`). `None` when unset, cleared, or the table
/// is not there yet.
pub(crate) async fn stored_named(pool: &Pool, name: &str) -> Result<Option<String>, PoolError> {
    match pool
        .rows(&format!(
            "SELECT \"value\" FROM \"$version_settings\" WHERE \"setting\" = {} \
             ORDER BY \"seq\" DESC LIMIT 1",
            sql_lit(name)
        ))
        .await
    {
        Ok(rows) => Ok(rows.first().and_then(|r| r.first().cloned().flatten())),
        Err(e) if e.is_missing_table() => Ok(None),
        Err(e) => Err(e),
    }
}

/// **Write one named setting's row**, checked by the caller, under the pin
/// lock with the pin stream's nudge, with who and why. `false` when it
/// already held that value (or clearing what was never set): nothing is
/// written.
pub(crate) async fn record_named(
    pool: &Pool,
    name: &str,
    value: Option<&str>,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<bool, PoolError> {
    if pool.rows(SETTINGS_DDL).await.is_err() {
        pool.rows(SETTINGS_DDL).await?;
    }
    let tx = pool.begin().await?;
    tx.rows(&pin_lock_sql()).await?;
    let now = tx
        .rows(&format!(
            "SELECT \"value\" FROM \"$version_settings\" WHERE \"setting\" = {} \
             ORDER BY \"seq\" DESC LIMIT 1",
            sql_lit(name)
        ))
        .await?;
    let current = now.first().and_then(|r| r.first().cloned().flatten());
    if current.as_deref() == value {
        tx.rollback().await?;
        return Ok(false);
    }
    tx.rows(&format!(
        "INSERT INTO \"$version_settings\" (\"setting\", \"value\", \"actor\", \"reason\", \"at\") \
         VALUES ({}, {}, {}, {}, {at})",
        sql_lit(name),
        value.map(sql_lit).unwrap_or_else(|| "NULL".into()),
        sql_lit(actor),
        sql_lit(reason),
    ))
    .await?;
    tx.notify(PINS_CHANNEL, "").await?;
    tx.commit().await?;
    Ok(true)
}

/// The newest row of each setting.
const SETTINGS_NOW_SQL: &str = "SELECT DISTINCT ON (\"setting\") \"setting\", \"value\" \
     FROM \"$version_settings\" ORDER BY \"setting\", \"seq\" DESC";

fn settings_of(rows: &[Vec<Option<String>>]) -> VersionSettings {
    let mut out = VersionSettings::default();
    for r in rows {
        let Some(s) = r
            .first()
            .cloned()
            .flatten()
            .and_then(|n| VersionSetting::from_name(&n))
        else {
            continue;
        };
        let value = r.get(1).cloned().flatten().filter(|v| !v.is_empty());
        out = out.with(s, value);
    }
    out
}

/// The settings, read inside `tx`. The table exists: every transaction that
/// reads it ran [`ensure_revocations`] first.
async fn settings_in(tx: &ordo_exec::TxConn) -> Result<VersionSettings, PoolError> {
    Ok(settings_of(&tx.rows(SETTINGS_NOW_SQL).await?))
}

/// Read `$revocations` with `sql`. A table from before `move_to` gains the
/// column first; a database with none answers empty.
async fn revocation_rows(pool: &Pool, sql: &str) -> Result<Vec<Vec<Option<String>>>, PoolError> {
    match pool.rows(sql).await {
        Ok(rows) => Ok(rows),
        Err(e) if e.is_missing_table() => Ok(vec![]),
        Err(e) if e.is_missing_column() => {
            ensure_revocations(pool).await?;
            pool.rows(sql).await
        }
        Err(e) => Err(e),
    }
}

/// The current mark on `version`, read inside `tx`, after its pin lock.
async fn mark_in(tx: &ordo_exec::TxConn, version: &str) -> Result<Option<Revocation>, PoolError> {
    let rows = tx.rows(&mark_sql(Some(version))).await?;
    Ok(rows.first().and_then(|r| revocation_row(r)))
}

/// The newest `$revocations` row of each version whose newest is a revoke.
fn mark_sql(version: Option<&str>) -> String {
    format!(
        "SELECT \"seq\", \"version\", \"act\", \"actor\", \"reason\", \"at\", \"move_to\" \
         FROM ( \
           SELECT DISTINCT ON (\"version\") * FROM \"$revocations\" {} \
           ORDER BY \"version\", \"seq\" DESC) m WHERE \"act\" = 'revoke'",
        version
            .map(|v| format!("WHERE \"version\" = {}", sql_lit(v)))
            .unwrap_or_default()
    )
}

fn revocation_row(r: &[Option<String>]) -> Option<Revocation> {
    Some(Revocation {
        seq: r.first()?.clone()?.parse().ok()?,
        version: r.get(1)?.clone()?,
        act: match r.get(2)?.as_deref()? {
            "revoke" => RevokeAct::Revoke,
            _ => RevokeAct::Unrevoke,
        },
        actor: r.get(3)?.clone()?,
        reason: r.get(4)?.clone()?,
        at: r.get(5)?.clone()?.parse().ok()?,
        move_to: r.get(6).cloned().flatten(),
    })
}

/// Create `$pins` and `$moves`. Called from `crate::tenants::ensure_table`,
/// so the pins' history exists wherever `$spaces` does. Retried once for
/// that function's reason: `IF NOT EXISTS` is not race-free between two
/// booting replicas.
pub async fn ensure_pin_tables(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(PIN_TABLES_DDL).await.is_err() {
        pool.rows(PIN_TABLES_DDL).await?;
    }
    ensure_build_tables(pool).await
}

/// Create the tenant builds' tables. Every act that moves many pins reads
/// them (an activation, a rollout, the revoked-version sweep), so each
/// makes sure of them first. Retried once, for [`ensure_pin_tables`]'
/// reason.
pub async fn ensure_build_tables(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(TENANT_BUILDS_DDL).await.is_err() {
        pool.rows(TENANT_BUILDS_DDL).await?;
    }
    Ok(())
}

/// The build that holds `space`, and its version, read in `tx`.
pub(crate) async fn build_of_in(
    tx: &ordo_exec::TxConn,
    space: &str,
) -> Result<Option<(String, String)>, PoolError> {
    let rows = tx
        .rows(&format!(
            "SELECT m.\"build\", b.\"version\" FROM \"$tenant_build_members\" m \
             JOIN \"$tenant_builds\" b ON b.\"name\" = m.\"build\" WHERE m.\"space\" = {}",
            sql_lit(space)
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| Some((r.first()?.clone()?, r.get(1)?.clone()?))))
}

/// The advisory lock every pin change takes first, in its own
/// transaction: an activation, a new tenant's pin and the backfill. A new
/// tenant whose row committed while an activation's re-pin was running is
/// not in that statement's view, so without the turn it read the old active
/// version and kept it until the next activation (review, 2026-11-04).
pub const PIN_LOCK_NS: i32 = 0x4F52_5049;

/// **The pin stream's nudge** (`plan-versions.md` §1.8, ask 17): every
/// transaction that changes a pin notifies this channel before it commits.
/// The stream itself is `$pins`, read by `seq` ([`Versions::pins_since`]);
/// the nudge only says "read it now". A node that misses one reads it on
/// its reconcile tick, so a second region needs no shared `NOTIFY`.
///
/// Every pin change holds [`PIN_LOCK_NS`] until it commits, and `seq` is
/// drawn under it, so rows commit in `seq` order and a reader that has
/// seen row `n` has seen every row below it.
pub const PINS_CHANNEL: &str = "ordo_pins";

/// `pins.reconcile_interval` (§7): how often a node reads every pin again,
/// whatever the stream said. Seconds.
pub const PINS_RECONCILE_SETTING: &str = "pins.reconcile_interval";
/// Its environment variable.
pub const PINS_RECONCILE_ENV: &str = "ORDO_PINS_RECONCILE_SECS";
/// Its default, in seconds.
pub const PINS_RECONCILE_DEFAULT: u64 = 30;

/// [`PINS_RECONCILE_ENV`], or the default when unset. A value that is not
/// a whole number of seconds from 1 is refused at boot, naming it.
pub fn pins_reconcile_from_env() -> Result<std::time::Duration, String> {
    match std::env::var(PINS_RECONCILE_ENV) {
        Err(_) => Ok(std::time::Duration::from_secs(PINS_RECONCILE_DEFAULT)),
        Ok(v) if v.trim().is_empty() => Ok(std::time::Duration::from_secs(PINS_RECONCILE_DEFAULT)),
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(n) if n >= 1 => Ok(std::time::Duration::from_secs(n)),
            _ => Err(format!(
                "{PINS_RECONCILE_SETTING} ({PINS_RECONCILE_ENV}) is `{v}`: it is a whole number \
                 of seconds, 1 or more. Set {PINS_RECONCILE_ENV}={PINS_RECONCILE_DEFAULT}, the \
                 default, or unset it"
            )),
        },
    }
}

/// Run one pin-changing statement in `tx`, and nudge the pin stream in the
/// same transaction. The rows the statement returned.
async fn repin(tx: &ordo_exec::TxConn, sql: &str) -> Result<Vec<Vec<Option<String>>>, PoolError> {
    let rows = tx.rows(sql).await?;
    if !rows.is_empty() {
        tx.notify(PINS_CHANNEL, "").await?;
    }
    Ok(rows)
}

fn pin_lock_sql() -> String {
    format!("SELECT pg_advisory_xact_lock({PIN_LOCK_NS}, 0)")
}

/// The active version, read inside `tx`, after its pin lock.
async fn active_in(tx: &ordo_exec::TxConn) -> Result<Option<String>, PoolError> {
    let rows = tx
        .rows("SELECT \"version\" FROM \"$activations\" ORDER BY \"seq\" DESC LIMIT 1")
        .await?;
    Ok(rows.first().and_then(|r| r.first().cloned().flatten()))
}

/// The open deployment-wide rollout, `(id, target)`, read inside `tx` after
/// its pin lock (`crate::rollouts`). A deployment that never started one
/// has no `$rollouts`, and answers none: the table is looked up first, so a
/// missing one does not abort `tx`.
pub(crate) async fn open_rollout_in(
    tx: &ordo_exec::TxConn,
) -> Result<Option<(i64, String)>, PoolError> {
    let there = tx
        .rows("SELECT to_regclass('\"$rollouts\"') IS NOT NULL")
        .await?
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .is_some_and(|v| v == "t" || v == "true");
    if !there {
        return Ok(None);
    }
    let rows = tx
        .rows(
            "SELECT \"id\", \"target\" FROM \"$rollouts\" \
             WHERE \"state\" IN ('running', 'paused') ORDER BY \"id\" DESC LIMIT 1",
        )
        .await?;
    Ok(rows
        .first()
        .and_then(|r| Some((r.first()?.clone()?.parse().ok()?, r.get(1)?.clone()?))))
}

/// `$own_pin`: the version the deployment's own space is served, when a
/// rollout moved it apart from the active version (`plan-versions.md` §3.7,
/// ruled 2026-11-23, Q4 and Q5). One row at most. The own space has no
/// `$spaces` row, so its pin lives here; every change is a `$pins` row
/// too, so nodes take it from the pin stream like a tenant's.
const OWN_PIN_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$own_pin\" ( \
  \"space\" text PRIMARY KEY, \
  \"version\" text NOT NULL, \
  \"epoch\" bigint NOT NULL \
)";

/// Move the own space's pin to `to` inside `tx`, after its pin lock, when
/// it has one and it names another version: the row, its epoch, a `$pins`
/// row, the nudge. Every activation calls it, so the own space follows
/// every activation. With `space`, a row is made first, at the active
/// version. A deployment no rollout touched has no `$own_pin`.
async fn repin_own_in(
    tx: &ordo_exec::TxConn,
    to: &str,
    actor: &str,
    cause: PinCause,
    at: u64,
    rollout: Option<i64>,
    space: Option<&str>,
) -> Result<bool, PoolError> {
    let there = tx
        .rows("SELECT to_regclass('\"$own_pin\"') IS NOT NULL")
        .await?
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .is_some_and(|v| v == "t" || v == "true");
    if !there {
        return Ok(false);
    }
    if let Some(space) = space {
        tx.rows(&format!(
            "INSERT INTO \"$own_pin\" (\"space\", \"version\", \"epoch\") \
             SELECT {s}, coalesce((SELECT \"version\" FROM \"$activations\" \
               ORDER BY \"seq\" DESC LIMIT 1), {to}), 0 \
             WHERE NOT EXISTS (SELECT 1 FROM \"$own_pin\")",
            s = sql_lit(space),
            to = sql_lit(to),
        ))
        .await?;
    }
    let rows = tx
        .rows(&format!(
            "WITH old AS (SELECT \"space\", \"version\" FROM \"$own_pin\" \
               WHERE \"version\" <> {to} FOR UPDATE), \
             moved AS (UPDATE \"$own_pin\" o SET \"version\" = {to}, \
               \"epoch\" = o.\"epoch\" + 1 FROM old WHERE o.\"space\" = old.\"space\" \
               RETURNING o.\"space\", old.\"version\" AS \"from\", o.\"epoch\") \
             INSERT INTO \"$pins\" (\"space\", \"from\", \"to\", \"epoch\", \"actor\", \
               \"cause\", \"at\", \"rollout\") \
             SELECT \"space\", \"from\", {to}, \"epoch\", {actor}, {cause}, {at}, {rollout} \
             FROM moved RETURNING \"space\"",
            to = sql_lit(to),
            actor = sql_lit(actor),
            cause = sql_lit(cause.as_str()),
            rollout = rollout
                .map(|r| r.to_string())
                .unwrap_or_else(|| "NULL".into()),
        ))
        .await?;
    if !rows.is_empty() {
        tx.notify(PINS_CHANNEL, "").await?;
    }
    Ok(!rows.is_empty())
}

/// `versions.new_tenant` as the environment sets it. Empty is unset.
pub fn new_tenant_from_env() -> Option<String> {
    std::env::var(NEW_TENANT_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Why a pin moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinCause {
    /// An activation to a version stored after the one it replaced.
    Activate,
    /// An activation to a version stored before the one it replaced.
    Rollback,
    /// The tenant was just created.
    NewTenant,
    /// The tenant predates pins and was given one at boot.
    Backfill,
    /// One tenant moved on its own, by the deployment owner's act
    /// ([`Versions::pin`]): a move on demand.
    Pin,
    /// Off a revoked version, in the tenant's maintenance window
    /// ([`Versions::move_off_revoked`], ruled 2026-11-05).
    Window,
    /// Off a revoked version, on demand: the deployment owner or the app
    /// developer's own system.
    Demand,
    /// Off a revoked version, on demand by the tenant, which the app
    /// developer granted.
    Tenant,
    /// The end of a seamless move (`plan-versions.md` §2.1, increment 7):
    /// the tenant's own database took the active version, was gated
    /// against it, and the pin followed.
    Move,
    /// A rollout's batch moved it (`plan-versions.md` §3.7, increment
    /// 10). Its `$pins` row names the rollout.
    Rollout,
    /// The tenant's own database was restored from a snapshot taken on
    /// another version, and the pin followed the data (§2.9, increment 17).
    Restore,
    /// A tenant build moved it: it joined or left one, or its build was set
    /// to another version (`plan-versions.md` §3.8, increment 16).
    Build,
    /// Its version was removed, and the deployment owner named where its
    /// tenants go (`ordo versions remove --move-to`, §1.6).
    Remove,
}

/// Where an open move stands (`$moves.phase`, §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovePhase {
    /// Recorded by the activation; the catch-up has not tried it yet.
    Expanding,
    /// The tenant's database could not be reached or did not answer. It
    /// is tried again; ruled 2026-11-23 (Q10) as waiting, never failed.
    Waiting,
    /// The change was refused by name. The tenant stays on its version
    /// until a new version is activated.
    Refused,
    /// A move with dual writes (increment 11a): the expand landed, the
    /// backfill worker fills the new shape, and every write the tenant
    /// makes on `from` writes both shapes.
    Backfilling,
    /// Verified: both shapes are full and current. The tenant still
    /// serves `from` and writes both. A revert in the soak returns here.
    Gated,
    /// The pin switched to `to`. Every write the tenant makes writes the
    /// old shape too, so a revert is a pin switch, until `soak_until`.
    Soaking,
}

impl MovePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            MovePhase::Expanding => "expanding",
            MovePhase::Waiting => "waiting",
            MovePhase::Refused => "refused",
            MovePhase::Backfilling => "backfilling",
            MovePhase::Gated => "gated",
            MovePhase::Soaking => "soaking",
        }
    }

    pub fn parse(text: &str) -> MovePhase {
        match text {
            "waiting" => MovePhase::Waiting,
            "refused" => MovePhase::Refused,
            "backfilling" => MovePhase::Backfilling,
            "gated" => MovePhase::Gated,
            "soaking" => MovePhase::Soaking,
            _ => MovePhase::Expanding,
        }
    }
}

/// One open move: a tenant still on `from` while the active version is
/// `to`, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub space: String,
    pub from: String,
    pub to: String,
    pub phase: MovePhase,
    /// Empty until a try fails. Never shown to the tenant's people.
    pub reason: String,
    pub started_at: u64,
    /// The stored plan's digest, for a move with dual writes (increment
    /// 11a). Empty for an activation's move (increment 7).
    pub step: String,
    /// When the soak ends, in seconds. 0 outside `soaking`.
    pub soak_until: u64,
    /// The move overlays, as `crate::dual_writes` renders them. Empty for
    /// an activation's move.
    pub overlay: String,
}

impl Move {
    /// One line for the deployment owner.
    pub fn line(&self) -> String {
        match self.phase {
            MovePhase::Expanding => format!(
                "{} stays on {} while its database is brought to {}",
                self.space, self.from, self.to
            ),
            MovePhase::Waiting => format!(
                "{} stays on {} and waits for its database ({}); moving to {} is tried again",
                self.space, self.from, self.reason, self.to
            ),
            MovePhase::Refused => format!(
                "{} stays on {} and keeps serving it: moving to {} was refused: {}",
                self.space, self.from, self.to, self.reason
            ),
            MovePhase::Backfilling => format!(
                "{} serves {} while its database is filled for {}; every write keeps both shapes{}",
                self.space,
                self.from,
                self.to,
                if self.reason.is_empty() {
                    String::new()
                } else {
                    format!(" (last try: {})", self.reason)
                }
            ),
            MovePhase::Gated => format!(
                "{} serves {}; its database holds {} too, and every write keeps both shapes",
                self.space, self.from, self.to
            ),
            MovePhase::Soaking => format!(
                "{} serves {} in its soak until {}; every write keeps {}'s shape too, so a revert \
                 loses nothing",
                self.space,
                self.to,
                utc_minute(self.soak_until),
                self.from
            ),
        }
    }

    /// A move with dual writes (increment 11a), rather than an activation's.
    pub fn is_dual(&self) -> bool {
        !self.step.is_empty()
    }
}

/// What an activation knows about the tenants with a database of their
/// own ([`Versions::activate_with`]): which databases already took the
/// version, and why the others did not.
#[derive(Debug, Clone, Default)]
pub struct MovePlan {
    /// Tenants whose database records the version: pinned at once.
    pub ready: std::collections::BTreeSet<String>,
    /// Tenants whose database did not take it, with the phase and reason
    /// their move starts in.
    pub held: BTreeMap<String, (MovePhase, String)>,
}

const MOVE_COLUMNS: &str = "\"space\", \"from\", \"to\", \"phase\", \"reason\", \"started_at\", \
                            \"step\", \"soak_until\", \"overlay\"";

fn move_row(r: &[Option<String>]) -> Option<Move> {
    Some(Move {
        space: r.first()?.clone()?,
        from: r.get(1)?.clone()?,
        to: r.get(2)?.clone()?,
        phase: MovePhase::parse(r.get(3)?.as_deref()?),
        reason: r.get(4).cloned().flatten().unwrap_or_default(),
        started_at: r.get(5)?.clone()?.parse().ok()?,
        step: r.get(6).cloned().flatten().unwrap_or_default(),
        soak_until: r
            .get(7)
            .cloned()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        overlay: r.get(8).cloned().flatten().unwrap_or_default(),
    })
}

/// Raise one tenant's pin epoch without moving its version, cause `move`,
/// and nudge the pin stream: a move with dual writes changed what its
/// writes carry. The epoch after.
async fn bump_in(
    tx: &ordo_exec::TxConn,
    space: &str,
    actor: &str,
    at: u64,
) -> Result<i64, PoolError> {
    let rows = tx
        .rows(&format!(
            "WITH bumped AS ( \
               UPDATE \"$spaces\" SET \"pin_epoch\" = \"pin_epoch\" + 1 \
               WHERE \"$space\" = {space} AND \"version\" IS NOT NULL \
               RETURNING \"$space\" AS \"space\", \"version\", \"pin_epoch\") \
             INSERT INTO \"$pins\" (\"space\", \"from\", \"to\", \"epoch\", \"actor\", \"cause\", \"at\") \
             SELECT \"space\", \"version\", \"version\", \"pin_epoch\", {}, {}, {at} FROM bumped \
             RETURNING \"epoch\"",
            sql_lit(actor),
            sql_lit(PinCause::Move.as_str()),
            space = sql_lit(space),
        ))
        .await?;
    tx.notify(PINS_CHANNEL, "").await?;
    Ok(rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|e| e.parse().ok())
        .unwrap_or(0))
}

/// The tenant's open move with dual writes, read inside `tx`.
async fn dual_move_in(tx: &ordo_exec::TxConn, space: &str) -> Result<Option<Move>, PoolError> {
    let rows = tx
        .rows(&format!(
            "SELECT {MOVE_COLUMNS} FROM \"$moves\" WHERE \"space\" = {} AND \"step\" <> ''",
            sql_lit(space)
        ))
        .await?;
    Ok(rows.first().and_then(|r| move_row(r)))
}

fn sql_list(items: &[String]) -> String {
    items
        .iter()
        .map(|s| sql_lit(s))
        .collect::<Vec<_>>()
        .join(", ")
}

impl PinCause {
    pub fn as_str(self) -> &'static str {
        match self {
            PinCause::Activate => "activate",
            PinCause::Rollback => "rollback",
            PinCause::NewTenant => "new_tenant",
            PinCause::Backfill => "backfill",
            PinCause::Pin => "pin",
            PinCause::Window => "window",
            PinCause::Demand => "demand",
            PinCause::Tenant => "tenant",
            PinCause::Move => "move",
            PinCause::Rollout => "rollout",
            PinCause::Restore => "restore",
            PinCause::Build => "build",
            PinCause::Remove => "remove",
        }
    }
}

/// One tenant's pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub version: Option<String>,
    pub epoch: i64,
}

/// One `$pins` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinChange {
    pub seq: i64,
    pub space: String,
    pub from: Option<String>,
    pub to: String,
    pub epoch: i64,
    pub actor: String,
    pub cause: String,
    pub at: u64,
}

/// Where a restore left a tenant's pin ([`Versions::restore_pin`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorePin {
    /// The tenant was already pinned to the restored data's version.
    Unchanged { version: String },
    /// The pin followed the data, cause `restore`.
    Pinned { change: PinChange },
    /// The data's version may not be served now (revoked, left out of
    /// `versions.served`, or below `versions.minimum`). The restore stands,
    /// the pin stays ahead of the data, and the tenant is held closed until
    /// its database is brought to its pin. `why` names the fix.
    Held {
        data: String,
        pin: Option<String>,
        why: String,
    },
}

/// Two version names in order: dotted numbers compared as numbers, part by
/// part; anything else compared as text.
pub fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |v: &str| -> Option<Vec<u64>> {
        v.split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse::<u64>().ok())
            .collect()
    };
    match (parts(a), parts(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
}

/// A revoke or an unrevoke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeAct {
    Revoke,
    Unrevoke,
}

impl RevokeAct {
    pub fn as_str(self) -> &'static str {
        match self {
            RevokeAct::Revoke => "revoke",
            RevokeAct::Unrevoke => "unrevoke",
        }
    }
}

/// Seconds since the epoch as `2026-11-05 14:02 UTC`, to the minute.
pub fn utc_minute(at: u64) -> String {
    let days = (at / 86_400) as i64;
    let secs = at % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// One `$revocations` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revocation {
    pub seq: i64,
    pub version: String,
    pub act: RevokeAct,
    pub actor: String,
    pub reason: String,
    pub at: u64,
    /// Where this revoke's tenants move, when the deployment owner named
    /// it. `None`: the active version.
    pub move_to: Option<String>,
}

/// What [`Versions::revoke`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revoked {
    /// The version now revoked.
    pub version: String,
    /// The activation made first, when the version was the active one:
    /// `(from, to)`.
    pub moved: Option<(String, String)>,
    /// Where its tenants move off it, when named.
    pub move_to: Option<String>,
}

/// Whose choice a move's target was ([`Versions::move_target`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSource {
    /// The tenant's own, `$move_targets`.
    Tenant,
    /// The revoke's `--move-to`.
    Revoke,
    /// None was named: the active version, the default.
    Active,
}

impl TargetSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetSource::Tenant => "tenant",
            TargetSource::Revoke => "revoke",
            TargetSource::Active => "active",
        }
    }
}

/// Where a tenant goes off a revoked version, and whose choice it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveTarget {
    pub version: String,
    pub source: TargetSource,
}

/// What [`Versions::move_off_revoked`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedOff {
    pub change: PinChange,
    pub source: TargetSource,
}

/// A tenant pinned to a revoked version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedPin {
    pub space: String,
    pub name: String,
    pub state: String,
    /// The tenant's own move target, when one is set.
    pub target: Option<String>,
    /// When its next maintenance window starts, in seconds since 1970 UTC.
    /// `None`: no window covers it, or it was not read
    /// (`crate::revoked_moves::note_next_windows` fills it).
    pub next_window: Option<i64>,
}

/// One revoked version and the tenants pinned to it
/// ([`Versions::pinned_to_revoked`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedVersion {
    pub mark: Revocation,
    pub ordinal: Option<u64>,
    /// Every tenant pinned to it, up to [`MAX_REVOKED_TENANTS`].
    pub tenants: Vec<RevokedPin>,
    /// How many there are in all.
    pub tenant_count: u64,
}

/// What a caller can do about a [`VersionRefusal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// Try again later; nothing about the request was wrong.
    Retry,
    /// Refused, and refused again until the stored versions change.
    Refuse,
    /// The input named something wrong; fix it and send again.
    FixInput,
}

/// The one error of [`Versions`] (rule 9). Each variant keeps its cause
/// and says what the caller can do ([`VersionRefusal::remedy`]).
#[derive(Debug)]
#[non_exhaustive]
pub enum VersionRefusal {
    /// Nothing is stored under this version. Fix the input: the stored
    /// versions are listed by `std.deployment.versions`.
    UnknownVersion(String),
    /// This version is stored with different bytes. Refused, for
    /// [`RegistryRefusal::VersionCollision`]'s reason.
    Collision { version: String },
    /// A setting names a version that is not stored. The deployment owner
    /// fixes the setting; the refusal names it and the value.
    Setting {
        setting: &'static str,
        value: String,
        stored: Vec<String>,
        /// What else the deployment owner may do, after "Set it to one of
        /// them, ".
        hint: &'static str,
    },
    /// No version is stored under the name or `#n` a person gave. Fix the
    /// input; `stored` names every version there is, `#n · name`.
    NotStored { asked: String, stored: Vec<String> },
    /// A label that is empty, longer than [`MAX_LABEL_CHARS`], or more than
    /// one line. Fix the input.
    Label(String),
    /// A git commit that is not one. Fix the input.
    GitCommit(String),
    /// The version is revoked (D8), so it is not activated, rolled back
    /// to, pinned or given to new tenants. `setting` is set when
    /// `versions.new_tenant` named it.
    Revoked {
        named: String,
        mark: Box<Revocation>,
        setting: Option<&'static str>,
    },
    /// The active version, revoked with no `--move-to` (ask 18).
    /// `candidates` names every stored version that is not revoked.
    RevokeActive {
        named: String,
        candidates: Vec<String>,
    },
    /// `--move-to` that cannot be used: the version itself, or a version
    /// that is not active. Fix the input.
    MoveTo(String),
    /// A revoke of a version already revoked. The mark there stays.
    AlreadyRevoked {
        named: String,
        mark: Box<Revocation>,
    },
    /// An unrevoke of a version that is not revoked.
    NotRevoked { named: String },
    /// A revoke of the version `versions.new_tenant` names. Revoked, it
    /// would leave new tenants unpinned, so the setting changes first.
    NamedBySetting {
        named: String,
        setting: &'static str,
    },
    /// A reason that is empty or longer than [`MAX_REASON_CHARS`]. Fix the
    /// input.
    Reason(String),
    /// A pin of a tenant there is none of, or one that serves nothing
    /// (`failed`, `reaped`). Fix the input.
    NoTenant { space: String },
    /// A move off a revoked version for a tenant whose version is not
    /// revoked. Nothing to move.
    NotOnRevoked {
        space: String,
        version: Option<String>,
    },
    /// The target a move off a revoked version would take is revoked
    /// itself. Ordo never picks another (ruled 2026-11-05); the deployment
    /// owner names one, or lifts the mark.
    TargetRevoked {
        space: String,
        named: String,
        source: TargetSource,
        mark: Box<Revocation>,
    },
    /// A pin target `versions.served` leaves out. Fix the input, or add
    /// it to the setting.
    NotServed {
        named: String,
        version: String,
        list: Vec<String>,
    },
    /// A pin target stored before `versions.minimum`.
    BelowMinimum { named: String, minimum: String },
    /// `versions.served` leaves out a version tenants are pinned to: the
    /// change is refused, naming the value to add.
    Stranded {
        version: String,
        tenants: u64,
        list: Vec<String>,
    },
    /// `versions.minimum` names a version stored after the active one, so
    /// every tenant on the active version would count as below it and no
    /// move could land: the change is refused, naming the active version.
    MinimumAboveActive { minimum: String, active: String },
    /// A production deployment pins only a version that arrived through the
    /// promotion gate (`crate::promotion`). The text says which, and the
    /// command that promotes it.
    NotPromoted(String),
    /// **One deployment-wide rollout at a time** (ruled 2026-11-23, Q3):
    /// an activation while rollout `id` to `target` is open is refused,
    /// naming `ordo rollout status` and `ordo rollout abort`. Moving one
    /// tenant on its own is never refused for this.
    RolloutOpen { id: i64, target: String },
    /// A restored database is at a version this deployment does not store
    /// (§2.9, increment 17). `newer` when it is newer than every stored
    /// version. Refused until the version is stored here.
    RestoreNotStored {
        version: String,
        newer: bool,
        stored: Vec<String>,
    },
    /// A load guard's value that is not a duration. Fix the input.
    Guard {
        setting: &'static str,
        value: String,
    },
    /// A tenant in a move with dual writes (increment 11a): its pin moves
    /// only through the move's own commands, which keep both shapes
    /// current. The text names them.
    Moving {
        space: String,
        from: String,
        to: String,
        phase: String,
    },
    /// A revert or a soak's close for a tenant with no move with dual
    /// writes open. Fix the input.
    NoMove { space: String },
    /// **A tenant a build holds** (increment 16): only the build moves it.
    /// The text names `ordo tenant-builds leave`.
    InBuild {
        space: String,
        build: String,
        version: String,
    },
    /// The database. Its class says whether a retry can help.
    Store(PoolError),
}

impl VersionRefusal {
    pub fn remedy(&self) -> Remedy {
        match self {
            VersionRefusal::UnknownVersion(_)
            | VersionRefusal::Setting { .. }
            | VersionRefusal::NotStored { .. }
            | VersionRefusal::Label(_)
            | VersionRefusal::GitCommit(_)
            | VersionRefusal::RevokeActive { .. }
            | VersionRefusal::MoveTo(_)
            | VersionRefusal::Reason(_)
            | VersionRefusal::NamedBySetting { .. }
            | VersionRefusal::NoTenant { .. }
            | VersionRefusal::TargetRevoked { .. }
            | VersionRefusal::NotServed { .. }
            | VersionRefusal::BelowMinimum { .. }
            | VersionRefusal::NotPromoted(_)
            | VersionRefusal::Stranded { .. }
            | VersionRefusal::MinimumAboveActive { .. }
            | VersionRefusal::Guard { .. }
            | VersionRefusal::Revoked {
                setting: Some(_), ..
            } => Remedy::FixInput,
            VersionRefusal::Collision { .. }
            | VersionRefusal::Revoked { .. }
            | VersionRefusal::AlreadyRevoked { .. }
            | VersionRefusal::NotOnRevoked { .. }
            | VersionRefusal::NotRevoked { .. }
            | VersionRefusal::RestoreNotStored { .. }
            | VersionRefusal::Moving { .. }
            | VersionRefusal::RolloutOpen { .. }
            | VersionRefusal::InBuild { .. } => Remedy::Refuse,
            VersionRefusal::NoMove { .. } => Remedy::FixInput,
            VersionRefusal::Store(e) => match e.class() {
                // A broken store is the deployment's to mend, and the
                // same call may pass once it is.
                FaultClass::Busy | FaultClass::Retryable | FaultClass::Broken => Remedy::Retry,
                _ => Remedy::Refuse,
            },
        }
    }

    /// The fault class, for a caller that answers a store's typed fault
    /// its own way. Every refusal that is not the database's is `Refused`.
    pub fn class(&self) -> FaultClass {
        match self {
            VersionRefusal::Store(e) => e.class(),
            _ => FaultClass::Refused,
        }
    }

    /// A database that has never had the table asked for: a deployment no
    /// apply has reached, which a reader answers as empty.
    pub fn is_missing_table(&self) -> bool {
        matches!(self, VersionRefusal::Store(e) if e.is_missing_table())
    }

    /// **The one place a `VersionRefusal` becomes a door answer.** `what`
    /// says what was being done, and leads the message.
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            VersionRefusal::UnknownVersion(_) | VersionRefusal::NotStored { .. } => {
                ExecError::new(ErrorCode::NotFound, message)
            }
            // A setting's value that is refused is the caller's input,
            // now that a setting is changed at the door (2026-11-05).
            VersionRefusal::Label(_)
            | VersionRefusal::GitCommit(_)
            | VersionRefusal::MoveTo(_)
            | VersionRefusal::Reason(_)
            | VersionRefusal::Setting { .. }
            | VersionRefusal::Guard { .. }
            | VersionRefusal::Stranded { .. }
            | VersionRefusal::MinimumAboveActive { .. } => {
                ExecError::new(ErrorCode::InvalidParam, message)
            }
            VersionRefusal::Collision { .. } | VersionRefusal::AlreadyRevoked { .. } => {
                ExecError::new(ErrorCode::Duplicate, message)
            }
            // The deployment's program refused, as `apply_refused` is: the
            // same call lands once the mark is lifted or the move named.
            VersionRefusal::Revoked { .. }
            | VersionRefusal::RevokeActive { .. }
            | VersionRefusal::TargetRevoked { .. }
            | VersionRefusal::NotServed { .. }
            | VersionRefusal::BelowMinimum { .. }
            | VersionRefusal::NotPromoted(_)
            | VersionRefusal::RolloutOpen { .. }
            | VersionRefusal::RestoreNotStored { .. }
            | VersionRefusal::InBuild { .. }
            | VersionRefusal::NamedBySetting { .. }
            | VersionRefusal::Moving { .. } => ExecError::new(ErrorCode::ApplyRefused, message),
            VersionRefusal::NotRevoked { .. }
            | VersionRefusal::NoMove { .. }
            | VersionRefusal::NoTenant { .. }
            | VersionRefusal::NotOnRevoked { .. } => ExecError::new(ErrorCode::NotFound, message),
            VersionRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
        }
    }
}

impl std::fmt::Display for VersionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionRefusal::RestoreNotStored {
                version,
                newer: true,
                stored,
            } => write!(
                f,
                "the restored database is at {version}, newer than every version this \
                 deployment stores ({}). Store {version} first (`ordo apply` its source, or \
                 `ordo promote {version}`), then run `ordo restore --record` again",
                stored.join(", ")
            ),
            VersionRefusal::RestoreNotStored {
                version, stored, ..
            } => write!(
                f,
                "the restored database is at {version}, which this deployment does not store \
                 (stored: {}). Bring it here first (`ordo promote {version}` from the \
                 environment that holds it, or `ordo apply` its source), then run `ordo \
                 restore --record` again",
                stored.join(", ")
            ),
            VersionRefusal::Guard { setting, value } => write!(
                f,
                "`{setting}` is {}, not `{value}`. Set it with `{}`",
                guard_form(setting),
                command_of(setting)
            ),
            // The registry's own words, so a refusal reads the same moved.
            VersionRefusal::UnknownVersion(v) => {
                write!(f, "{}", RegistryRefusal::UnknownVersion(v.clone()))
            }
            VersionRefusal::Collision { version } => write!(
                f,
                "{}",
                RegistryRefusal::VersionCollision {
                    version: version.clone()
                }
            ),
            VersionRefusal::Setting {
                setting,
                value,
                stored,
                hint,
            } => write!(
                f,
                "{setting} `{value}` names no stored version. Stored: {}. Set it to one of \
                 them with `{} <version>`, {hint}",
                if stored.is_empty() {
                    "none".to_string()
                } else {
                    stored.join(", ")
                },
                command_of(setting)
            ),
            VersionRefusal::NotStored { asked, stored } => write!(
                f,
                "no version is stored under `{asked}`. Stored: {}",
                if stored.is_empty() {
                    "none".to_string()
                } else {
                    stored.join(", ")
                }
            ),
            VersionRefusal::Label(why) => write!(f, "label refused: {why}"),
            VersionRefusal::GitCommit(why) => write!(f, "git commit refused: {why}"),
            VersionRefusal::Revoked {
                named,
                mark,
                setting,
            } => {
                write!(
                    f,
                    "version_revoked: {named} was revoked by {} at {}: \"{}\". \
                     `ordo versions unrevoke {} --reason \"...\"` lifts the mark",
                    mark.actor,
                    utc_minute(mark.at),
                    mark.reason,
                    mark.version
                )?;
                if let Some(setting) = setting {
                    write!(
                        f,
                        ". {setting} names it: `{} <version>` names one that is not \
                         revoked, or `--clear` starts new tenants on the active version",
                        command_of(setting)
                    )?;
                }
                Ok(())
            }
            VersionRefusal::RevokeActive { named, candidates } => write!(
                f,
                "revoke_needs_move_to: {named} is the active version, and every tenant is \
                 on it. Name where they go with `--move-to <version>` (`moveTo` on \
                 `std.deployment.revoke`); Ordo never picks one. Stored and not revoked: {}",
                if candidates.is_empty() {
                    "none".to_string()
                } else {
                    candidates.join(", ")
                }
            ),
            VersionRefusal::MoveTo(why) => write!(f, "--move-to refused: {why}"),
            VersionRefusal::AlreadyRevoked { named, mark } => write!(
                f,
                "already_revoked: {named} was revoked by {} at {}: \"{}\". The mark stays \
                 as it is",
                mark.actor,
                utc_minute(mark.at),
                mark.reason
            ),
            VersionRefusal::NotRevoked { named } => write!(
                f,
                "not_revoked: {named} carries no revoke mark; `ordo versions revoked` lists \
                 the versions that do"
            ),
            VersionRefusal::Reason(why) => write!(f, "reason refused: {why}"),
            VersionRefusal::NamedBySetting { named, setting } => write!(
                f,
                "revoke_named_by_setting: {setting} names {named}, so new tenants start on \
                 it. Set another with `{cmd} <version>`, or `{cmd} --clear` to start them on \
                 the active version, then revoke",
                cmd = command_of(setting)
            ),
            VersionRefusal::Moving {
                space,
                from,
                to,
                phase,
            } => write!(
                f,
                "tenant_moving: tenant `{space}` is moving from {from} to {to} with dual writes \
                 ({phase}), and its pin moves only through the move. Finish it with `ordo \
                 evolve --tenant {space}`, revert it in its soak with `ordo evolve --tenant \
                 {space} --revert`, or close the soak with `ordo evolve --tenant {space} \
                 --close-soak`"
            ),
            VersionRefusal::NoMove { space } => write!(
                f,
                "no_move: tenant `{space}` has no move with dual writes open, so there is no \
                 soak to revert or close. `ordo fleet --host <h>` shows each tenant's move; \
                 `ordo evolve --tenant {space} --to <version>` starts one"
            ),
            VersionRefusal::NoTenant { space } => write!(
                f,
                "no_tenant: no tenant `{space}` is provisioning, active or paused here, so \
                 there is no pin to move. `ordo fleet --host <h>` names them"
            ),
            VersionRefusal::NotOnRevoked { space, version } => write!(
                f,
                "not_on_revoked: tenant `{space}` is on {}, which is not revoked or below \
                 {MINIMUM_SETTING}, so there is nothing to move it off. `ordo versions pin \
                 {space} <version>` moves it anyway",
                version.as_deref().unwrap_or("no version")
            ),
            VersionRefusal::TargetRevoked {
                space,
                named,
                source,
                mark,
            } => {
                write!(
                    f,
                    "move_target_revoked: tenant `{space}` would move to {named}, which was \
                     revoked by {} at {}: \"{}\". Ordo picks no other. ",
                    mark.actor,
                    utc_minute(mark.at),
                    mark.reason
                )?;
                match source {
                    TargetSource::Tenant => write!(
                        f,
                        "It is the tenant's own target: `ordo versions target {space} \
                         <version>` names another, or `--clear` falls back to the revoke's"
                    ),
                    TargetSource::Revoke => write!(
                        f,
                        "It is the revoke's `--move-to`: `ordo versions target {space} \
                         <version>` names one for this tenant"
                    ),
                    TargetSource::Active => write!(
                        f,
                        "`ordo versions target {space} <version>` names one for this tenant"
                    ),
                }
            }
            VersionRefusal::NotServed {
                named,
                version,
                list,
            } => write!(
                f,
                "not_served: {named} is not a version this deployment serves: \
                 {SERVED_SETTING} is `{}`. Add it: `ordo versions set served {},{version}`, \
                 or `ordo versions set served in-use`",
                list.join(","),
                list.join(",")
            ),
            VersionRefusal::NotPromoted(why) => write!(f, "not_promoted: {why}"),
            VersionRefusal::InBuild {
                space,
                build,
                version,
            } => write!(
                f,
                "in_tenant_build: tenant {space} is in tenant build `{build}` on {version}, and \
                 only the build moves it. `ordo tenant-builds set {build} <version>` moves the \
                 whole build; `ordo tenant-builds leave {space}` returns it to the active \
                 version"
            ),
            VersionRefusal::RolloutOpen { id, target } => write!(
                f,
                "rollout_open: rollout {id} to {target} is open, and one deployment-wide \
                 rollout runs at a time. `ordo rollout status` shows where it stands; `ordo \
                 rollout finish` or `ordo rollout abort` closes it. One tenant may still move \
                 on its own: `ordo versions pin <tenant> <version>`"
            ),
            VersionRefusal::BelowMinimum { named, minimum } => write!(
                f,
                "below_minimum: {named} is older than {MINIMUM_SETTING}, {minimum}. Name a \
                 version at or above it, or lower it: `ordo versions set minimum <version>` \
                 or `--clear`"
            ),
            VersionRefusal::Stranded {
                version,
                tenants,
                list,
            } => write!(
                f,
                "{SERVED_SETTING} `{}` leaves out {version}, which {tenants} tenant(s) are \
                 pinned to. Add it: `ordo versions set served {},{version}`, or move them \
                 first (`ordo versions pin <tenant> <version>`)",
                list.join(","),
                list.join(",")
            ),
            VersionRefusal::MinimumAboveActive { minimum, active } => write!(
                f,
                "{MINIMUM_SETTING} `{minimum}` is stored after the active version {active}, \
                 so every tenant on {active} would be below it. Name {active} or older \
                 (`ordo versions set minimum {active}`), activate {minimum} first, or \
                 `--clear` it"
            ),
            VersionRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for VersionRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            VersionRefusal::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<PoolError> for VersionRefusal {
    fn from(e: PoolError) -> VersionRefusal {
        VersionRefusal::Store(e)
    }
}

impl From<RegistryRefusal> for VersionRefusal {
    fn from(e: RegistryRefusal) -> VersionRefusal {
        match e {
            RegistryRefusal::UnknownVersion(v) => VersionRefusal::UnknownVersion(v),
            RegistryRefusal::VersionCollision { version } => VersionRefusal::Collision { version },
            RegistryRefusal::Store(e) => VersionRefusal::Store(e),
            // `RegistryRefusal` is non-exhaustive; a variant added below
            // reads as the database's until it is given its own.
            other => VersionRefusal::Store(PoolError::db(other.to_string())),
        }
    }
}

/// The client. Borrowed over the control database's pool: cheap to make
/// where it is needed, and it holds nothing a second one would not.
#[derive(Clone, Copy)]
pub struct Versions<'a> {
    pool: &'a Pool,
    /// `versions.new_tenant` named by the caller. `None`: the stored one,
    /// read where it is needed.
    new_tenant: Option<&'a str>,
    /// `versions.served` and `versions.minimum` named by the caller.
    /// `None`: the stored ones ([`Versions::settings`]), read each time.
    policy: Option<&'a ServedPolicy>,
}

impl<'a> Versions<'a> {
    pub fn on(pool: &'a Pool) -> Versions<'a> {
        Versions {
            pool,
            new_tenant: None,
            policy: None,
        }
    }

    /// The same client, under another served policy than the stored one.
    pub fn with_policy(self, policy: &'a ServedPolicy) -> Versions<'a> {
        Versions {
            policy: Some(policy),
            ..self
        }
    }

    /// The served policy in force, read inside `tx` after its pin lock: the
    /// caller's, or the stored one. A setting change writes under the same
    /// lock, so a pin never decides on settings a change just replaced.
    async fn policy_in(&self, tx: &ordo_exec::TxConn) -> Result<ServedPolicy, VersionRefusal> {
        match self.policy {
            Some(p) => Ok(p.clone()),
            None => Ok(settings_in(tx).await?.policy),
        }
    }

    /// The served policy in force: the caller's, or the stored one.
    async fn policy_now(&self) -> Result<ServedPolicy, VersionRefusal> {
        match self.policy {
            Some(p) => Ok(p.clone()),
            None => Ok(self.settings().await?.policy),
        }
    }

    /// **May a tenant be pinned to `version`** under `versions.served` and
    /// `versions.minimum`? The active version is always served.
    pub async fn check_served(&self, version: &str) -> Result<(), VersionRefusal> {
        let p = &self.policy_now().await?;
        if p.served.is_none() && p.minimum.is_none() {
            return Ok(());
        }
        if let Some(list) = &p.served {
            let active = registry::active_version(self.pool).await?;
            if active.as_deref() != Some(version) && !list.iter().any(|v| v == version) {
                return Err(VersionRefusal::NotServed {
                    named: self.named(version).await?,
                    version: version.to_string(),
                    list: list.clone(),
                });
            }
        }
        if let Some(min) = &p.minimum {
            let ords = registry::ordinals(self.pool).await?;
            if let (Some(v), Some(m)) = (ords.get(version), ords.get(min)) {
                if v < m {
                    return Err(VersionRefusal::BelowMinimum {
                        named: self.named(version).await?,
                        minimum: self.named(min).await?,
                    });
                }
            }
        }
        Ok(())
    }

    /// Refuse `version` when it is below `versions.minimum`.
    async fn refuse_below_minimum(&self, version: &str) -> Result<(), VersionRefusal> {
        let policy = self.policy_now().await?;
        if self
            .with_policy(&policy)
            .below_minimum()
            .await?
            .iter()
            .any(|v| v == version)
        {
            return Err(VersionRefusal::BelowMinimum {
                named: self.named(version).await?,
                minimum: self
                    .named(policy.minimum.as_deref().unwrap_or_default())
                    .await?,
            });
        }
        Ok(())
    }

    /// The stored versions below `versions.minimum`. Empty with none.
    pub async fn below_minimum(&self) -> Result<Vec<String>, VersionRefusal> {
        let p = self.policy_now().await?;
        let Some(min) = &p.minimum else {
            return Ok(vec![]);
        };
        let ords = registry::ordinals(self.pool).await?;
        let Some(m) = ords.get(min) else {
            return Ok(vec![]);
        };
        Ok(ords
            .iter()
            .filter(|(_, n)| *n < m)
            .map(|(v, _)| v.clone())
            .collect())
    }

    /// **Check `versions.served` and `versions.minimum`** before a change
    /// to either is stored, and answer them resolved: each `#n` named as
    /// its version. Boot runs it on the stored ones and only logs. Refused: a
    /// version not stored (naming the setting and the versions stored), and
    /// a served list that leaves out a version tenants are pinned to
    /// (naming the value to add). A deployment with nothing applied passes.
    pub async fn check_policy(&self, p: &ServedPolicy) -> Result<ServedPolicy, VersionRefusal> {
        if p.served.is_none() && p.minimum.is_none() {
            return Ok(p.clone());
        }
        let stored = match registry::stored(self.pool).await {
            Ok(s) => s.into_iter().map(|(v, _, _)| v).collect::<Vec<_>>(),
            Err(e) if e.is_missing_table() => return Ok(p.clone()),
            Err(e) => return Err(e.into()),
        };
        let resolve = |name: &str, setting: &'static str| {
            let name = name.to_string();
            let stored = stored.clone();
            async move {
                match self.resolve(&name).await {
                    Ok(v) => Ok(v),
                    Err(VersionRefusal::NotStored { .. })
                    | Err(VersionRefusal::UnknownVersion(_)) => Err(VersionRefusal::Setting {
                        setting,
                        value: name,
                        stored,
                        hint: "or `--clear` it",
                    }),
                    Err(e) => Err(e),
                }
            }
        };
        let mut out = ServedPolicy::default();
        if let Some(list) = &p.served {
            let mut resolved = Vec::new();
            for name in list {
                resolved.push(resolve(name, SERVED_SETTING).await?);
            }
            let active = registry::active_version(self.pool).await?;
            for (version, tenants) in self.tenants_per_version().await? {
                if active.as_deref() != Some(version.as_str()) && !resolved.contains(&version) {
                    return Err(VersionRefusal::Stranded {
                        version,
                        tenants,
                        list: resolved,
                    });
                }
            }
            out.served = Some(resolved);
        }
        if let Some(min) = &p.minimum {
            let minimum = resolve(min, MINIMUM_SETTING).await?;
            // A minimum above the active version puts every tenant on the
            // active version below it, and every move to it is refused.
            if let Some(active) = registry::active_version(self.pool).await? {
                let ords = registry::ordinals(self.pool).await?;
                if let (Some(m), Some(a)) = (ords.get(&minimum), ords.get(&active)) {
                    if m > a {
                        return Err(VersionRefusal::MinimumAboveActive { minimum, active });
                    }
                }
            }
            out.minimum = Some(minimum);
        }
        Ok(out)
    }

    /// The same client, with `versions.new_tenant` set.
    pub fn with_new_tenant(self, version: Option<&'a str>) -> Versions<'a> {
        Versions {
            new_tenant: version,
            ..self
        }
    }

    /// Create every table this client reads, and fill in what a row stored
    /// before a column existed lacks. Idempotent.
    ///
    /// `$spaces`' columns are added only when the pin's are missing. Its
    /// setup alters the table 18 times, each under `ACCESS EXCLUSIVE` even
    /// when the column is there, and this runs on every apply and
    /// activation: one slow reader of `$spaces` then queued the apply, and
    /// every tenant lookup queued behind it (review, 2026-11-04).
    pub async fn ensure(&self) -> Result<(), VersionRefusal> {
        registry::ensure_tables(self.pool).await?;
        ensure_revocations(self.pool).await?;
        let shaped = self
            .pool
            .rows(
                "SELECT to_regclass('\"$pins\"') IS NOT NULL AND EXISTS ( \
                 SELECT 1 FROM information_schema.columns \
                 WHERE table_schema = current_schema() AND table_name = '$spaces' \
                 AND column_name = 'pin_epoch')",
            )
            .await?;
        if shaped
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .as_deref()
            != Some("t")
        {
            crate::tenants::ensure_table(self.pool).await?;
        }
        Ok(())
    }

    // ── stored versions ────────────────────────────────────────────────

    /// Store a version. See [`registry::store`].
    pub async fn store(&self, a: &Artifact) -> Result<Applied, VersionRefusal> {
        Ok(registry::store(self.pool, a).await?)
    }

    /// Store this release's build of a stored version. See
    /// [`registry::rebuild`].
    pub async fn rebuild(&self, a: &Artifact) -> Result<Applied, VersionRefusal> {
        Ok(registry::rebuild(self.pool, a).await?)
    }

    pub async fn get(&self, version: &str) -> Result<Option<Artifact>, VersionRefusal> {
        Ok(registry::get(self.pool, version).await?)
    }

    pub async fn active_version(&self) -> Result<Option<String>, VersionRefusal> {
        Ok(registry::active_version(self.pool).await?)
    }

    pub async fn active(&self) -> Result<Option<Artifact>, VersionRefusal> {
        Ok(registry::active(self.pool).await?)
    }

    pub async fn ordinal_of(&self, version: &str) -> Result<Option<u64>, VersionRefusal> {
        Ok(registry::ordinal_of(self.pool, version).await?)
    }

    pub async fn ordinals(&self) -> Result<BTreeMap<String, u64>, VersionRefusal> {
        Ok(registry::ordinals(self.pool).await?)
    }

    /// `(version, applied at, by whom)`, newest first.
    pub async fn stored(&self) -> Result<Vec<(String, Timestamp, String)>, VersionRefusal> {
        Ok(registry::stored(self.pool).await?)
    }

    pub async fn activations(&self, limit: usize) -> Result<Vec<Activation>, VersionRefusal> {
        Ok(registry::activations(self.pool, limit).await?)
    }

    /// Every activation of one version, newest first.
    pub async fn activations_of(&self, version: &str) -> Result<Vec<Activation>, VersionRefusal> {
        Ok(registry::activations_of(self.pool, version).await?)
    }

    pub async fn activated_at(&self, version: &str) -> Result<Option<u64>, VersionRefusal> {
        Ok(registry::activated_at(self.pool, version).await?)
    }

    pub async fn revisions(&self, declared: Option<&str>) -> Result<Vec<Revision>, VersionRefusal> {
        Ok(registry::revisions(self.pool, declared).await?)
    }

    pub async fn declared_versions(&self) -> Result<Vec<String>, VersionRefusal> {
        Ok(registry::declared_versions(self.pool).await?)
    }

    /// One version's builds: `(build id, Ordo release, recorded at)`,
    /// oldest first.
    pub async fn builds(
        &self,
        version: &str,
    ) -> Result<Vec<(String, String, u64)>, VersionRefusal> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"build_id\", \"ordo_release\", \"recorded_at\" FROM \"$builds\" \
                 WHERE \"version\" = {} ORDER BY \"recorded_at\", \"build_id\"",
                sql_lit(version)
            ))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some((
                    r.first()?.clone()?,
                    r.get(1)?.clone()?,
                    r.get(2)?.clone()?.parse().ok()?,
                ))
            })
            .collect())
    }

    /// One version's declared `@` version and source fingerprint, as
    /// stored. `None` when nothing is stored under it.
    pub async fn identity(
        &self,
        version: &str,
    ) -> Result<Option<(String, String)>, VersionRefusal> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT coalesce(\"declared\", ''), coalesce(\"source_id\", '') \
                 FROM \"$artifacts\" WHERE \"version\" = {}",
                sql_lit(version)
            ))
            .await?;
        Ok(rows.first().map(|r| {
            (
                r.first().cloned().flatten().unwrap_or_default(),
                r.get(1).cloned().flatten().unwrap_or_default(),
            )
        }))
    }

    // ── names, labels and commits ──────────────────────────────────────

    /// The stored version a person named: by its name, or by its order
    /// number as `#4`. Refused naming every stored version when there is
    /// none.
    pub async fn resolve(&self, name: &str) -> Result<String, VersionRefusal> {
        let name = name.trim();
        let found = match name.strip_prefix('#').map(str::parse::<u64>) {
            Some(Ok(n)) => self
                .pool
                .rows(&format!(
                    "SELECT \"version\" FROM \"$artifacts\" WHERE \"ordinal\" = {n} \
                     AND \"removed_at\" IS NULL"
                ))
                .await?
                .first()
                .and_then(|r| r.first().cloned().flatten()),
            Some(Err(_)) => None,
            None => registry::get(self.pool, name).await?.map(|a| a.version),
        };
        match found {
            Some(v) => Ok(v),
            None => Err(VersionRefusal::NotStored {
                asked: name.to_string(),
                stored: self.named_all().await?,
            }),
        }
    }

    /// Every stored version as `#n · name`, oldest first.
    async fn named_all(&self) -> Result<Vec<String>, VersionRefusal> {
        let rows = self
            .pool
            .rows(
                "SELECT \"version\", \"ordinal\" FROM \"$artifacts\" \
                 WHERE \"removed_at\" IS NULL ORDER BY \"ordinal\" NULLS LAST, \"version\"",
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let v = r.first()?.clone()?;
                Some(match r.get(1).cloned().flatten() {
                    Some(n) => format!("#{n} · {v}"),
                    None => v,
                })
            })
            .collect())
    }

    /// Set or clear the label of the version a person named ([`resolve`]),
    /// and answer the version it was. `None` clears it. The label is
    /// checked first ([`check_label`]).
    ///
    /// [`resolve`]: Versions::resolve
    pub async fn set_label(
        &self,
        name: &str,
        label: Option<&str>,
    ) -> Result<String, VersionRefusal> {
        let label = label.map(check_label).transpose()?;
        let version = self.resolve(name).await?;
        self.pool
            .rows(&format!(
                "UPDATE \"$artifacts\" SET \"label\" = {} WHERE \"version\" = {}",
                label
                    .as_deref()
                    .map(sql_lit)
                    .unwrap_or_else(|| "NULL".into()),
                sql_lit(&version),
            ))
            .await?;
        Ok(version)
    }

    /// Record what an apply said about the version it stored. A label
    /// given replaces the one there. A commit is kept from the first apply
    /// that gave one: it says where the version came from, and a later
    /// apply of the same program from another checkout does not change
    /// that. Both are checked first.
    pub async fn record_provenance(
        &self,
        version: &str,
        p: &Provenance,
    ) -> Result<(), VersionRefusal> {
        if let Some(label) = &p.label {
            let label = check_label(label)?;
            self.pool
                .rows(&format!(
                    "UPDATE \"$artifacts\" SET \"label\" = {} WHERE \"version\" = {}",
                    sql_lit(&label),
                    sql_lit(version),
                ))
                .await?;
        }
        if let Some(commit) = &p.git_commit {
            let commit = check_git_commit(commit)?;
            self.pool
                .rows(&format!(
                    "UPDATE \"$artifacts\" SET \"git_commit\" = {} \
                     WHERE \"version\" = {} AND \"git_commit\" IS NULL",
                    sql_lit(&commit),
                    sql_lit(version),
                ))
                .await?;
        }
        Ok(())
    }

    /// One version's label and commit. Default when nothing is stored
    /// under it.
    pub async fn provenance(&self, version: &str) -> Result<Provenance, VersionRefusal> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"label\", \"git_commit\" FROM \"$artifacts\" WHERE \"version\" = {}",
                sql_lit(version)
            ))
            .await?;
        Ok(rows
            .first()
            .map(|r| Provenance {
                label: r.first().cloned().flatten(),
                git_commit: r.get(1).cloned().flatten(),
            })
            .unwrap_or_default())
    }

    /// Every version that carries a label or a commit, with them.
    pub async fn provenances(&self) -> Result<BTreeMap<String, Provenance>, VersionRefusal> {
        let rows = self
            .pool
            .rows(
                "SELECT \"version\", \"label\", \"git_commit\" FROM \"$artifacts\" \
                 WHERE \"label\" IS NOT NULL OR \"git_commit\" IS NOT NULL",
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some((
                    r.first()?.clone()?,
                    Provenance {
                        label: r.get(1).cloned().flatten(),
                        git_commit: r.get(2).cloned().flatten(),
                    },
                ))
            })
            .collect())
    }

    // ── activation and the pins ────────────────────────────────────────

    /// **Point the deployment at a stored version, and pin every tenant to
    /// it**, in one transaction with the activation row and its nudge.
    ///
    /// The checks are [`registry::activate`]'s. A version already active
    /// appends nothing and moves no pin. The pins' cause is `rollback` when
    /// the version was stored before the one it replaces, by order number,
    /// and `activate` otherwise.
    ///
    /// Every tenant moves, a tenant `versions.new_tenant` started elsewhere
    /// included, because rollouts do not exist yet (increment 10).
    /// **A production deployment pins only what arrived through the
    /// promotion gate** ([`crate::promotion::require_arrival`]).
    pub async fn require_arrival(&self, version: &str) -> Result<(), VersionRefusal> {
        crate::promotion::require_arrival(self.pool, version)
            .await
            .map_err(|e| match e {
                crate::promotion::PromotionRefusal::Store(p) => VersionRefusal::Store(p),
                other => VersionRefusal::NotPromoted(other.to_string()),
            })
    }

    pub async fn activate(
        &self,
        version: &str,
        actor: &str,
        at: Timestamp,
    ) -> Result<Activated, VersionRefusal> {
        self.activate_with(version, actor, at, &MovePlan::default())
            .await
    }

    /// [`Versions::activate`], with what the caller already knows about
    /// the tenants that have a database of their own.
    ///
    /// **The seamless move** (`plan-versions.md` §2.1, increment 7). A
    /// tenant on the deployment's own database is pinned to `version` in
    /// this transaction: that database was gated before the activation. A
    /// tenant with a database of its own (`$spacedb`) is pinned now only
    /// when `plan.ready` names it. Every other one keeps its pin and gets
    /// an open move in `$moves`, in the phase `plan.held` gives or
    /// `expanding`. It keeps serving its version. The catch-up
    /// ([`crate::tenant_forward`]) expands its database, gates it, and
    /// switches the pin with [`Versions::finish_moves`]. Before 2026-11-23
    /// every pin moved here and a tenant whose database was behind was
    /// held closed (25i).
    ///
    /// An activation supersedes every open move: each names the version it
    /// replaces.
    pub async fn activate_with(
        &self,
        version: &str,
        actor: &str,
        at: Timestamp,
        plan: &MovePlan,
    ) -> Result<Activated, VersionRefusal> {
        if registry::get(self.pool, version).await?.is_none() {
            return Err(VersionRefusal::UnknownVersion(version.to_string()));
        }
        let from = registry::active_version(self.pool).await?;
        if from.as_deref() == Some(version) {
            return Ok(Activated::AlreadyActive(version.to_string()));
        }
        self.require_arrival(version).await?;
        let cause = match &from {
            Some(f) => {
                let ordinals = registry::ordinals(self.pool).await?;
                match (ordinals.get(f), ordinals.get(version)) {
                    (Some(was), Some(to)) if to < was => PinCause::Rollback,
                    _ => PinCause::Activate,
                }
            }
            None => PinCause::Activate,
        };
        ensure_revocations(self.pool).await?;
        ensure_build_tables(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, version).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        // One deployment-wide rollout at a time (ruled 2026-11-23, Q3).
        // Read under the pin lock a rollout's start takes too.
        if let Some((id, target)) = open_rollout_in(&tx).await? {
            tx.rollback().await?;
            return Err(VersionRefusal::RolloutOpen { id, target });
        }
        // Every tenant would land below `versions.minimum`. Read under the
        // lock a setting change takes (audit L4).
        let policy = self.policy_in(&tx).await?;
        if let Err(e) = self
            .with_policy(&policy)
            .refuse_below_minimum(version)
            .await
        {
            tx.rollback().await?;
            return Err(e);
        }
        registry::record_activation(&tx, version, actor, at).await?;
        // Whether any tenant has a database of its own. A deployment that
        // never attached one has no `$spacedb`, and every tenant moves now.
        let attached = tx
            .rows("SELECT to_regclass('\"$spacedb\"') IS NOT NULL")
            .await?
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .is_some_and(|v| v == "t" || v == "true");
        // A tenant in a move with dual writes (increment 11a) keeps its
        // move and its pin: only the move's own commands switch it.
        tx.rows("DELETE FROM \"$moves\" WHERE \"step\" = ''")
            .await?;
        let moving = "AND \"$space\" NOT IN (SELECT \"space\" FROM \"$moves\")";
        if !attached {
            // A tenant in a tenant build keeps the build's version.
            let keep = format!("{moving} {NOT_IN_A_BUILD}");
            repin(&tx, &repin_sql(version, actor, cause, at.0, Some(&keep))).await?;
        } else {
            let ready: Vec<String> = plan.ready.iter().cloned().collect();
            let ready_sql = if ready.is_empty() {
                String::new()
            } else {
                format!(" OR \"$space\" IN ({})", sql_list(&ready))
            };
            // On the deployment's own database, never pinned before, or
            // already brought forward: pinned now.
            let now = format!(
                "AND (\"version\" IS NULL \
                  OR \"$space\" NOT IN (SELECT \"$space\" FROM \"$spacedb\"){ready_sql}) \
                 {moving} {NOT_IN_A_BUILD}"
            );
            repin(&tx, &repin_sql(version, actor, cause, at.0, Some(&now))).await?;
            // Every other tenant stays where it is, with a move open.
            tx.rows(&format!(
                "INSERT INTO \"$moves\" (\"space\", \"from\", \"to\", \"phase\", \"reason\", \
                   \"soak_until\", \"started_at\") \
                 SELECT \"$space\", \"version\", {to}, 'expanding', '', 0, {at} \
                 FROM \"$spaces\" \
                 WHERE \"state\" IN {PINNED_STATES} AND \"version\" IS NOT NULL \
                   AND \"version\" <> {to} \
                   AND \"$space\" IN (SELECT \"$space\" FROM \"$spacedb\") \
                   AND \"$space\" NOT IN (SELECT \"space\" FROM \"$moves\") \
                   {NOT_IN_A_BUILD}",
                to = sql_lit(version),
                at = at.0,
            ))
            .await?;
            for (space, (phase, reason)) in &plan.held {
                tx.rows(&format!(
                    "UPDATE \"$moves\" SET \"phase\" = {}, \"reason\" = {} \
                     WHERE \"space\" = {} AND \"step\" = ''",
                    sql_lit(phase.as_str()),
                    sql_lit(reason),
                    sql_lit(space)
                ))
                .await?;
            }
        }
        // The own space follows the activation, when a rollout pinned it.
        repin_own_in(&tx, version, actor, cause, at.0, None, None).await?;
        tx.commit().await?;
        Ok(Activated::Switched {
            from,
            to: version.to_string(),
        })
    }

    /// **"Active" flips to `version`, and no tenant moves**
    /// (`plan-versions.md` §3.7; ruled 2026-11-23, Q5): the end of a
    /// rollout. One activation row and its nudge, under the pin lock, so
    /// every node loads `version` and swaps: sign-in (`AuthApp`) and new
    /// tenants follow it from here. A tenant keeps its pin; only one that
    /// has none yet is pinned. Open moves to another version are closed.
    /// The own space's pin follows, as on every activation.
    ///
    /// Called by the rollout that is open, so it is not refused for it.
    /// Refused: a version not stored, or revoked.
    pub async fn activate_alone(
        &self,
        version: &str,
        actor: &str,
        at: Timestamp,
    ) -> Result<Activated, VersionRefusal> {
        if registry::get(self.pool, version).await?.is_none() {
            return Err(VersionRefusal::UnknownVersion(version.to_string()));
        }
        let from = registry::active_version(self.pool).await?;
        if from.as_deref() == Some(version) {
            return Ok(Activated::AlreadyActive(version.to_string()));
        }
        self.require_arrival(version).await?;
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, version).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        registry::record_activation(&tx, version, actor, at).await?;
        tx.rows(&format!(
            "DELETE FROM \"$moves\" WHERE \"to\" <> {}",
            sql_lit(version)
        ))
        .await?;
        repin(
            &tx,
            &repin_sql(
                version,
                actor,
                PinCause::Rollout,
                at.0,
                Some("AND \"version\" IS NULL"),
            ),
        )
        .await?;
        repin_own_in(&tx, version, actor, PinCause::Rollout, at.0, None, None).await?;
        tx.commit().await?;
        Ok(Activated::Switched {
            from,
            to: version.to_string(),
        })
    }

    /// **The deployment's own space moves to `version`, apart from the
    /// active version** (`plan-versions.md` §3.7, §1.14; ruled 2026-11-23,
    /// Q4): the rollout's named step. `space` is the own space's id. Its
    /// pin is `$own_pin`, a `$pins` row records the move, and the nudge
    /// tells every node, which loads `version` and serves the own space on
    /// it. "Active", sign-in and new tenants do not move until the rollout
    /// finishes ([`Versions::activate_alone`]). `true` when it moved.
    pub async fn pin_own(
        &self,
        space: &str,
        version: &str,
        rollout: i64,
        actor: &str,
        at: u64,
    ) -> Result<bool, VersionRefusal> {
        ensure_revocations(self.pool).await?;
        if self.pool.rows(OWN_PIN_DDL).await.is_err() {
            self.pool.rows(OWN_PIN_DDL).await?;
        }
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, version).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        let moved = repin_own_in(
            &tx,
            version,
            actor,
            PinCause::Rollout,
            at,
            Some(rollout),
            Some(space),
        )
        .await?;
        tx.commit().await?;
        Ok(moved)
    }

    /// **The own space's pin, made when it has none** (increment 13): at
    /// the active version, epoch 0, with a `$pins` row and the nudge, so
    /// every node serves the own space under an epoch, and its writes check
    /// the pin fence. A seal on the deployment's database fences them at
    /// its end. `(version, epoch)`; `None` when no version is active.
    pub async fn own_pin_made(
        &self,
        space: &str,
        actor: &str,
        at: u64,
    ) -> Result<Option<(String, i64)>, VersionRefusal> {
        ensure_revocations(self.pool).await?;
        if self.pool.rows(OWN_PIN_DDL).await.is_err() {
            self.pool.rows(OWN_PIN_DDL).await?;
        }
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        let made = tx
            .rows(&format!(
                "INSERT INTO \"$own_pin\" (\"space\", \"version\", \"epoch\") \
                 SELECT {s}, a.\"version\", 0 FROM (SELECT \"version\" FROM \"$activations\" \
                   ORDER BY \"seq\" DESC LIMIT 1) a \
                 WHERE NOT EXISTS (SELECT 1 FROM \"$own_pin\") RETURNING \"version\"",
                s = sql_lit(space),
            ))
            .await?;
        if let Some(v) = made.first().and_then(|r| r.first().cloned().flatten()) {
            tx.rows(&format!(
                "INSERT INTO \"$pins\" (\"space\", \"from\", \"to\", \"epoch\", \"actor\", \
                 \"cause\", \"at\") VALUES ({}, NULL, {}, 0, {}, {}, {at})",
                sql_lit(space),
                sql_lit(&v),
                sql_lit(actor),
                sql_lit(PinCause::Move.as_str()),
            ))
            .await?;
            tx.notify(PINS_CHANNEL, "").await?;
        }
        let row = tx
            .rows("SELECT \"version\", \"epoch\" FROM \"$own_pin\" LIMIT 1")
            .await?;
        tx.commit().await?;
        Ok(row
            .first()
            .and_then(|r| Some((r.first()?.clone()?, r.get(1)?.clone()?.parse().ok()?))))
    }

    /// **The own space moves to `version` with a seal's end on the
    /// deployment's database** (increment 13): the version before can no
    /// longer write the sealed field there, so the own space follows the
    /// data, first, as a rollout moves it (Q4). "Active", sign-in and new
    /// tenants stay until the activation. Cause `move`. `true` when it
    /// moved.
    pub async fn pin_own_for_step(
        &self,
        space: &str,
        version: &str,
        actor: &str,
        at: u64,
    ) -> Result<bool, VersionRefusal> {
        ensure_revocations(self.pool).await?;
        if self.pool.rows(OWN_PIN_DDL).await.is_err() {
            self.pool.rows(OWN_PIN_DDL).await?;
        }
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, version).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        let moved =
            repin_own_in(&tx, version, actor, PinCause::Move, at, None, Some(space)).await?;
        tx.commit().await?;
        Ok(moved)
    }

    /// The own space's pin, `(space, version, epoch)`, when a rollout gave
    /// it one.
    pub async fn own_pin(&self) -> Result<Option<(String, String, i64)>, VersionRefusal> {
        match self
            .pool
            .rows("SELECT \"space\", \"version\", \"epoch\" FROM \"$own_pin\" LIMIT 1")
            .await
        {
            Ok(rows) => Ok(rows.first().and_then(|r| {
                Some((
                    r.first()?.clone()?,
                    r.get(1)?.clone()?,
                    r.get(2)?.clone()?.parse().ok()?,
                ))
            })),
            Err(e) if e.is_missing_table() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// **Move one tenant as part of rollout `rollout`** (§3.7), cause
    /// `rollout`, its `$pins` row naming the rollout. The caller has brought
    /// a tenant's own database to `version` and gated it.
    ///
    /// `version` is the rollout's target, which becomes the active version,
    /// so `versions.served` does not refuse it; a revoked version and one
    /// below `versions.minimum` are refused. `None`: the tenant was on
    /// `version` already. Refused as [`VersionRefusal::NoTenant`] when it
    /// no longer holds a pin (reaped meanwhile).
    pub async fn pin_in_rollout(
        &self,
        space: &str,
        version: &str,
        rollout: i64,
        actor: &str,
        at: u64,
    ) -> Result<Option<PinChange>, VersionRefusal> {
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, version).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        let policy = self.policy_in(&tx).await?;
        if let Err(e) = self
            .with_policy(&policy)
            .refuse_below_minimum(version)
            .await
        {
            tx.rollback().await?;
            return Err(e);
        }
        let held = tx
            .rows(&format!(
                "SELECT \"version\" FROM \"$spaces\" \
                 WHERE \"$space\" = {} AND \"state\" IN {PINNED_STATES}",
                sql_lit(space)
            ))
            .await?;
        let Some(row) = held.first() else {
            tx.rollback().await?;
            return Err(VersionRefusal::NoTenant {
                space: space.to_string(),
            });
        };
        let from = row.first().cloned().flatten();
        let filter = format!("AND \"$space\" = {}", sql_lit(space));
        let moved = repin(
            &tx,
            &repin_sql(version, actor, PinCause::Rollout, at, Some(&filter)),
        )
        .await?;
        tx.rows(&format!(
            "UPDATE \"$pins\" SET \"rollout\" = {rollout} \
             WHERE \"seq\" = (SELECT max(\"seq\") FROM \"$pins\" WHERE \"space\" = {})",
            sql_lit(space)
        ))
        .await?;
        tx.rows(&format!(
            "DELETE FROM \"$moves\" WHERE \"space\" = {}",
            sql_lit(space)
        ))
        .await?;
        tx.commit().await?;
        if moved.is_empty() {
            return Ok(None);
        }
        let epoch = self.pin_of(space).await?.map(|p| p.epoch).unwrap_or(0);
        Ok(Some(PinChange {
            seq: 0,
            space: space.to_string(),
            from,
            to: version.to_string(),
            epoch,
            actor: actor.to_string(),
            cause: PinCause::Rollout.as_str().to_string(),
            at,
        }))
    }

    /// Name rollout `rollout` on the tenant's newest `$pins` row: a move
    /// with dual writes a rollout drove pinned it at its switch (increment
    /// 11b).
    pub async fn tag_rollout(&self, space: &str, rollout: i64) -> Result<(), VersionRefusal> {
        self.pool
            .rows(&format!(
                "UPDATE \"$pins\" SET \"rollout\" = {rollout} \
                 WHERE \"seq\" = (SELECT max(\"seq\") FROM \"$pins\" WHERE \"space\" = {})",
                sql_lit(space)
            ))
            .await?;
        Ok(())
    }

    /// Reopen one tenant's refused move (§2.1, increment 7), so the
    /// catch-up tries it again: phase `expanding`, reason cleared. The
    /// move it reopened; `None` when the tenant has no refused move.
    pub async fn retry_move(&self, space: &str) -> Result<Option<Move>, VersionRefusal> {
        let sql = format!(
            "UPDATE \"$moves\" SET \"phase\" = 'expanding', \"reason\" = '' \
             WHERE \"space\" = {} AND \"phase\" = 'refused' RETURNING {MOVE_COLUMNS}",
            sql_lit(space)
        );
        let rows = match self.pool.rows(&sql).await {
            Ok(r) => r,
            Err(e) if e.is_missing_table() => return Ok(None),
            Err(e) if e.is_missing_column() => {
                ensure_revocations(self.pool).await?;
                self.pool.rows(&sql).await?
            }
            Err(e) => return Err(e.into()),
        };
        Ok(rows.first().and_then(|r| move_row(r)))
    }

    /// **Every open move** (§2.1), oldest first. A database with no
    /// `$moves` answers empty.
    pub async fn moves(&self) -> Result<Vec<Move>, VersionRefusal> {
        let sql =
            format!("SELECT {MOVE_COLUMNS} FROM \"$moves\" ORDER BY \"started_at\", \"space\"");
        let rows = match self.pool.rows(&sql).await {
            Ok(r) => r,
            Err(e) if e.is_missing_table() => return Ok(vec![]),
            Err(e) if e.is_missing_column() => {
                ensure_revocations(self.pool).await?;
                self.pool.rows(&sql).await?
            }
            Err(e) => return Err(e.into()),
        };
        Ok(rows.iter().filter_map(|r| move_row(r)).collect())
    }

    /// Say where one open move to `to` stands, after a try. A move that is
    /// gone (finished, or superseded by another activation) is left gone.
    pub async fn note_move(
        &self,
        space: &str,
        to: &str,
        phase: MovePhase,
        reason: &str,
    ) -> Result<(), VersionRefusal> {
        ensure_revocations(self.pool).await?;
        self.pool
            .rows(&format!(
                "UPDATE \"$moves\" SET \"phase\" = {}, \"reason\" = {} \
                 WHERE \"space\" = {} AND \"to\" = {}",
                sql_lit(phase.as_str()),
                sql_lit(reason),
                sql_lit(space),
                sql_lit(to)
            ))
            .await?;
        Ok(())
    }

    /// **Switch the pins of moves that are done** (§2.1 step 5): each of
    /// `spaces` with an open move to `to` is pinned to `to`, cause `move`,
    /// and its move closed, in one transaction with its `$pins` row. The
    /// caller has brought each tenant's database to `to` and gated it. A
    /// space whose move is gone, or names another version, is left alone.
    /// The spaces that moved.
    pub async fn finish_moves(
        &self,
        spaces: &[String],
        to: &str,
        actor: &str,
        at: u64,
    ) -> Result<Vec<String>, VersionRefusal> {
        if spaces.is_empty() {
            return Ok(vec![]);
        }
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, to).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        let open = format!(
            "SELECT \"space\" FROM \"$moves\" WHERE \"to\" = {} AND \"space\" IN ({})",
            sql_lit(to),
            sql_list(spaces)
        );
        let filter = format!("AND \"$space\" IN ({open})");
        let moved: Vec<String> = repin(
            &tx,
            &repin_sql(to, actor, PinCause::Move, at, Some(&filter)),
        )
        .await?
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
        // A tenant already on `to` (pinned by hand meanwhile) closes too.
        tx.rows(&format!(
            "DELETE FROM \"$moves\" WHERE \"to\" = {} AND \"space\" IN ({})",
            sql_lit(to),
            sql_list(spaces)
        ))
        .await?;
        tx.commit().await?;
        Ok(moved)
    }

    // ── moves with dual writes (increment 11a) ─────────────────────────

    /// The tenant's open move with dual writes, if any.
    pub async fn dual_move(&self, space: &str) -> Result<Option<Move>, VersionRefusal> {
        Ok(self
            .moves()
            .await?
            .into_iter()
            .find(|m| m.space == space && m.is_dual()))
    }

    /// Every open move with dual writes.
    pub async fn dual_moves(&self) -> Result<Vec<Move>, VersionRefusal> {
        Ok(self
            .moves()
            .await?
            .into_iter()
            .filter(Move::is_dual)
            .collect())
    }

    /// **Open a move with dual writes** for one tenant pinned to `from`
    /// (`plan-backfill-dual-writes.md` §1): the move, its step and its
    /// overlays, in phase `expanding`, and the tenant's pin epoch raised in
    /// the same transaction, so every node takes the overlays before it
    /// writes again (the pin fence refuses one that has not). An
    /// activation's move for the tenant is replaced. The epoch after.
    // The move's two ends, its step and overlays, and who and when: each
    // is one column of the row it writes.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_dual_move(
        &self,
        space: &str,
        from: &str,
        to: &str,
        step: &str,
        overlay: &str,
        actor: &str,
        at: u64,
    ) -> Result<i64, VersionRefusal> {
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(m) = dual_move_in(&tx, space).await? {
            tx.rollback().await?;
            return Err(VersionRefusal::Moving {
                space: m.space,
                from: m.from,
                to: m.to,
                phase: m.phase.as_str().to_string(),
            });
        }
        let held = tx
            .rows(&format!(
                "SELECT \"version\" FROM \"$spaces\" WHERE \"$space\" = {} \
                 AND \"state\" IN {PINNED_STATES} FOR UPDATE",
                sql_lit(space)
            ))
            .await?;
        let Some(version) = held.first().and_then(|r| r.first().cloned().flatten()) else {
            tx.rollback().await?;
            return Err(VersionRefusal::NoTenant {
                space: space.to_string(),
            });
        };
        if version != from {
            tx.rollback().await?;
            return Err(VersionRefusal::MoveTo(format!(
                "tenant `{space}` is on {version}, and the step starts from {from}: replay it \
                 to {from} first (`ordo evolve --tenant {space} --to {from}`)"
            )));
        }
        tx.rows(&format!(
            "DELETE FROM \"$moves\" WHERE \"space\" = {}",
            sql_lit(space)
        ))
        .await?;
        tx.rows(&format!(
            "INSERT INTO \"$moves\" (\"space\", \"from\", \"to\", \"phase\", \"reason\", \
               \"soak_until\", \"started_at\", \"step\", \"overlay\") \
             VALUES ({}, {}, {}, 'expanding', '', 0, {at}, {}, {})",
            sql_lit(space),
            sql_lit(from),
            sql_lit(to),
            sql_lit(step),
            sql_lit(overlay),
        ))
        .await?;
        let epoch = bump_in(&tx, space, actor, at).await?;
        tx.commit().await?;
        Ok(epoch)
    }

    /// Say where a move with dual writes stands. The overlays do not
    /// change, so no pin epoch moves.
    pub async fn note_dual_move(
        &self,
        space: &str,
        phase: MovePhase,
        reason: &str,
    ) -> Result<(), VersionRefusal> {
        self.pool
            .rows(&format!(
                "UPDATE \"$moves\" SET \"phase\" = {}, \"reason\" = {} \
                 WHERE \"space\" = {} AND \"step\" <> ''",
                sql_lit(phase.as_str()),
                sql_lit(reason),
                sql_lit(space)
            ))
            .await?;
        Ok(())
    }

    /// **The switch** (§2.1 step 5): the tenant's pin moves from the move's
    /// `from` to its `to`, cause `move`, and the move enters its soak until
    /// `soak_until`, in one transaction. Only from `gated`.
    pub async fn switch_dual_move(
        &self,
        space: &str,
        actor: &str,
        at: u64,
        soak_until: u64,
    ) -> Result<PinChange, VersionRefusal> {
        self.flip_dual_move(
            space,
            MovePhase::Gated,
            MovePhase::Soaking,
            actor,
            at,
            soak_until,
        )
        .await
    }

    /// **A revert in the soak** (`plan-backfill-dual-writes.md` §1): the
    /// pin moves back to the move's `from`, cause `rollback`, and the move
    /// returns to `gated`, both shapes still current. Only from `soaking`.
    pub async fn revert_dual_move(
        &self,
        space: &str,
        actor: &str,
        at: u64,
    ) -> Result<PinChange, VersionRefusal> {
        self.flip_dual_move(space, MovePhase::Soaking, MovePhase::Gated, actor, at, 0)
            .await
    }

    async fn flip_dual_move(
        &self,
        space: &str,
        expect: MovePhase,
        next: MovePhase,
        actor: &str,
        at: u64,
        soak_until: u64,
    ) -> Result<PinChange, VersionRefusal> {
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        let Some(m) = dual_move_in(&tx, space).await? else {
            tx.rollback().await?;
            return Err(VersionRefusal::NoMove {
                space: space.to_string(),
            });
        };
        if m.phase != expect {
            tx.rollback().await?;
            return Err(VersionRefusal::Moving {
                space: m.space,
                from: m.from,
                to: m.to,
                phase: m.phase.as_str().to_string(),
            });
        }
        let (to, cause) = if next == MovePhase::Soaking {
            (&m.to, PinCause::Move)
        } else {
            (&m.from, PinCause::Rollback)
        };
        if let Some(mark) = mark_in(&tx, to).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        let filter = format!("AND \"$space\" = {}", sql_lit(space));
        repin(&tx, &repin_sql(to, actor, cause, at, Some(&filter))).await?;
        tx.rows(&format!(
            "UPDATE \"$moves\" SET \"phase\" = {}, \"soak_until\" = {soak_until}, \
             \"reason\" = '' WHERE \"space\" = {} AND \"step\" <> ''",
            sql_lit(next.as_str()),
            sql_lit(space)
        ))
        .await?;
        let epoch = tx
            .rows(&format!(
                "SELECT \"pin_epoch\" FROM \"$spaces\" WHERE \"$space\" = {}",
                sql_lit(space)
            ))
            .await?
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|e| e.parse().ok())
            .unwrap_or(0);
        tx.commit().await?;
        Ok(PinChange {
            seq: 0,
            space: space.to_string(),
            from: Some(if next == MovePhase::Soaking {
                m.from.clone()
            } else {
                m.to.clone()
            }),
            to: to.clone(),
            epoch,
            actor: actor.to_string(),
            cause: cause.as_str().to_string(),
            at,
        })
    }

    /// **The soak's close**: the move leaves `$moves` and the tenant's pin
    /// epoch rises, so every node drops its overlays. Only from `soaking`.
    /// The caller has made the database the move's `to` alone first. The
    /// epoch after.
    pub async fn close_dual_move(
        &self,
        space: &str,
        actor: &str,
        at: u64,
    ) -> Result<i64, VersionRefusal> {
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        match dual_move_in(&tx, space).await? {
            None => {
                tx.rollback().await?;
                return Err(VersionRefusal::NoMove {
                    space: space.to_string(),
                });
            }
            Some(m) if m.phase != MovePhase::Soaking => {
                tx.rollback().await?;
                return Err(VersionRefusal::Moving {
                    space: m.space,
                    from: m.from,
                    to: m.to,
                    phase: m.phase.as_str().to_string(),
                });
            }
            Some(_) => {}
        }
        tx.rows(&format!(
            "DELETE FROM \"$moves\" WHERE \"space\" = {}",
            sql_lit(space)
        ))
        .await?;
        let epoch = bump_in(&tx, space, actor, at).await?;
        tx.commit().await?;
        Ok(epoch)
    }

    /// The version a tenant created now starts on: `versions.new_tenant`
    /// when set, the active version otherwise.
    ///
    /// The plan's default is the last version that finished a rollout
    /// (§1.3, D14). Rollouts do not exist yet, so the active version stands
    /// in for it until increment 10.
    pub async fn new_tenant_version(&self) -> Result<Option<String>, VersionRefusal> {
        if let Some(v) = self.new_tenant_setting().await? {
            return Ok(Some(v));
        }
        match registry::active_version(self.pool).await {
            Ok(v) => Ok(v),
            // No registry: nothing has ever been applied here.
            Err(e) if e.is_missing_table() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// `versions.new_tenant`: the caller's, or the stored one. `None`:
    /// unset, the active version.
    async fn new_tenant_setting(&self) -> Result<Option<String>, VersionRefusal> {
        match self.new_tenant {
            Some(v) => Ok(Some(v.to_string())),
            None => Ok(self.settings().await?.new_tenant),
        }
    }

    /// Refuse a `versions.new_tenant` that names no stored version, a
    /// revoked one, or one the served policy leaves out. A change to the
    /// setting is checked so, and boot logs what it finds, so a deployment
    /// never starts tenants on a version it cannot load.
    pub async fn check_new_tenant(&self) -> Result<(), VersionRefusal> {
        let Some(value) = self.new_tenant_setting().await? else {
            return Ok(());
        };
        let value = value.as_str();
        if registry::get(self.pool, value).await?.is_some() {
            return match self.revocation_of(value).await? {
                Some(mark) => Err(self.revoked(mark, Some(NEW_TENANT_SETTING)).await),
                None => self.check_served(value).await,
            };
        }
        Err(VersionRefusal::Setting {
            setting: NEW_TENANT_SETTING,
            value: value.to_string(),
            stored: registry::stored(self.pool)
                .await?
                .into_iter()
                .map(|(v, _, _)| v)
                .collect(),
            hint: "or `--clear` it to start new tenants on the active version",
        })
    }

    /// Pin a tenant just created, cause `new_tenant`. The version it was
    /// pinned to; `None` when there is none to pin to, which is a
    /// deployment nothing was ever applied to. A tenant already pinned is
    /// left as it is.
    pub async fn pin_new_tenant(
        &self,
        space: &str,
        actor: &str,
        at: u64,
    ) -> Result<Option<String>, VersionRefusal> {
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        // The turn first, then the version: an activation that started
        // before this has committed by now, and is the one read.
        tx.rows(&pin_lock_sql()).await?;
        // The stored setting is read under the same turn, so a change to it
        // that committed first is the one this pin follows.
        let named = match self.new_tenant {
            Some(v) => Some(v.to_string()),
            None => settings_in(&tx).await?.new_tenant,
        };
        let version = match &named {
            Some(v) => Some(v.clone()),
            None => match active_in(&tx).await {
                Ok(v) => v,
                // No registry: nothing has ever been applied here.
                Err(e) if e.is_missing_table() => None,
                Err(e) => return Err(e.into()),
            },
        };
        let Some(version) = version else {
            return Ok(None);
        };
        if let Some(mark) = mark_in(&tx, &version).await? {
            tx.rollback().await?;
            let setting = named.is_some().then_some(NEW_TENANT_SETTING);
            return Err(self.revoked(mark, setting).await);
        }
        let filter = format!(
            "AND \"$space\" = {} AND \"version\" IS NULL",
            sql_lit(space)
        );
        repin(
            &tx,
            &repin_sql(&version, actor, PinCause::NewTenant, at, Some(&filter)),
        )
        .await?;
        tx.commit().await?;
        Ok(Some(version))
    }

    /// Pin every tenant that has no pin to the active version, cause
    /// `backfill`. The tenants pinned. Idempotent: a second call finds none.
    pub async fn backfill(&self, actor: &str, at: u64) -> Result<usize, VersionRefusal> {
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        let Some(active) = active_in(&tx).await? else {
            return Ok(0);
        };
        // The active version is never revoked: a revoke of it moves the
        // deployment first. Checked all the same, as every pin target is.
        if let Some(mark) = mark_in(&tx, &active).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        let moved = repin(
            &tx,
            &repin_sql(
                &active,
                actor,
                PinCause::Backfill,
                at,
                Some("AND \"version\" IS NULL"),
            ),
        )
        .await?
        .len();
        tx.commit().await?;
        Ok(moved)
    }

    /// **Move one tenant's pin** to the version a person named, now
    /// (`plan-versions.md` §1.3, increment 3), cause `pin`. The move on
    /// demand: the deployment owner's act today, through the door
    /// (`std.deployment.pin`) or the CLI.
    ///
    /// It is the one act every later single-tenant move calls: a move off
    /// a revoked version in the tenant's maintenance window, or on demand
    /// (ruled 2026-11-05), lands here with its own actor and the target
    /// the app developer chose, the active version by default.
    ///
    /// Refused: a version not stored or revoked, a tenant that is not
    /// `provisioning`, `active` or `paused`. A tenant already there moves
    /// nothing and answers its pin. The pin the tenant has after.
    ///
    /// A tenant a tenant build holds is refused ([`VersionRefusal::InBuild`]):
    /// only its build moves it (increment 16).
    pub async fn pin(
        &self,
        space: &str,
        name: &str,
        actor: &str,
        at: u64,
    ) -> Result<PinChange, VersionRefusal> {
        self.pin_as(space, name, actor, at, PinCause::Pin).await
    }

    /// [`Versions::pin`], with the cause written to `$pins`. Only
    /// [`PinCause::Build`] moves a tenant a build holds: the tenant builds'
    /// client (`crate::tenant_builds`) calls it so. A restore keeps an open
    /// move and points it at the restored version; every other cause ends it.
    pub async fn pin_as(
        &self,
        space: &str,
        name: &str,
        actor: &str,
        at: u64,
        cause: PinCause,
    ) -> Result<PinChange, VersionRefusal> {
        let version = self.resolve(name).await?;
        self.require_arrival(&version).await?;
        ensure_revocations(self.pool).await?;
        ensure_build_tables(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, &version).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        // Read under the lock a setting change takes, so a change racing
        // this pin is seen (security audit 2026-11-05, L4).
        let policy = self.policy_in(&tx).await?;
        if let Err(e) = self.with_policy(&policy).check_served(&version).await {
            tx.rollback().await?;
            return Err(e);
        }
        let held = tx
            .rows(&format!(
                "SELECT \"version\", \"pin_epoch\" FROM \"$spaces\" \
                 WHERE \"$space\" = {} AND \"state\" IN {PINNED_STATES}",
                sql_lit(space)
            ))
            .await?;
        let Some(row) = held.first() else {
            tx.rollback().await?;
            return Err(VersionRefusal::NoTenant {
                space: space.to_string(),
            });
        };
        let from = row.first().cloned().flatten();
        if let Some(m) = dual_move_in(&tx, space).await? {
            tx.rollback().await?;
            return Err(VersionRefusal::Moving {
                space: m.space,
                from: m.from,
                to: m.to,
                phase: m.phase.as_str().to_string(),
            });
        }
        if cause != PinCause::Build {
            if let Some((build, on)) = build_of_in(&tx, space).await? {
                tx.rollback().await?;
                return Err(VersionRefusal::InBuild {
                    space: space.to_string(),
                    build,
                    version: on,
                });
            }
        }
        let filter = format!("AND \"$space\" = {}", sql_lit(space));
        repin(&tx, &repin_sql(&version, actor, cause, at, Some(&filter))).await?;
        if cause == PinCause::Restore {
            // **The database's own cursor wins** (§2.9): a move that was
            // open goes on from the version the data is at now, and the
            // backfill reads its cursor from the restored `$backfills`. A
            // move to the version restored is no move at all.
            tx.rows(&format!(
                "DELETE FROM \"$moves\" WHERE \"space\" = {s} AND \"to\" = {v}; \
                 UPDATE \"$moves\" SET \"from\" = {v}, \"phase\" = 'expanding', \
                 \"reason\" = 'restored at ' || {v} WHERE \"space\" = {s}",
                s = sql_lit(space),
                v = sql_lit(&version)
            ))
            .await?;
        } else {
            // A person chose this tenant's version, so an open move (§2.1)
            // no longer applies to it.
            tx.rows(&format!(
                "DELETE FROM \"$moves\" WHERE \"space\" = {}",
                sql_lit(space)
            ))
            .await?;
        }
        tx.commit().await?;
        let epoch = self.pin_of(space).await?.map(|p| p.epoch).unwrap_or(0);
        Ok(PinChange {
            seq: 0,
            space: space.to_string(),
            from,
            to: version,
            epoch,
            actor: actor.to_string(),
            cause: cause.as_str().to_string(),
            at,
        })
    }

    /// **The pin after a tenant's own database was restored** (§2.9,
    /// increment 17). `data` is the version the restored database records
    /// (`$migrations`). The pin follows the data, so the tenant serves the
    /// version its rows are shaped for:
    ///
    /// - already pinned there: nothing moves;
    /// - stored, not revoked, served and not below `versions.minimum`:
    ///   pinned there, cause `restore`, a new epoch;
    /// - revoked, left out of `versions.served`, or below the minimum (Q13,
    ///   ruled 2026-11-23): the restore stands and the pin is left where it
    ///   is, ahead of the data. The tenant is held closed as any tenant
    ///   whose database is behind (`SchemaBehind`) until the database is
    ///   brought to its pin;
    /// - not stored: refused, naming the version and how to store it.
    ///
    /// Asked before the restore marker is written, so a refusal leaves the
    /// tenant closed and the same command runs again once the version is
    /// stored. Moves the pin only for a version that passes every check.
    pub async fn restore_pin(
        &self,
        space: &str,
        data: &str,
        actor: &str,
        at: u64,
    ) -> Result<RestorePin, VersionRefusal> {
        let check = self.restore_check(space, data).await?;
        match check {
            RestorePin::Pinned { .. } => Ok(RestorePin::Pinned {
                change: self
                    .pin_as(space, data, actor, at, PinCause::Restore)
                    .await?,
            }),
            other => Ok(other),
        }
    }

    /// What [`Versions::restore_pin`] would do, moving nothing: refused for
    /// a version not stored, otherwise the answer with no pin written (a
    /// `Pinned` here carries the change that would be made, epoch 0).
    pub async fn restore_check(
        &self,
        space: &str,
        data: &str,
    ) -> Result<RestorePin, VersionRefusal> {
        let pin = self
            .pin_of(space)
            .await?
            .ok_or_else(|| VersionRefusal::NoTenant {
                space: space.to_string(),
            })?;
        if registry::get(self.pool, data).await?.is_none() {
            let stored: Vec<String> = registry::stored(self.pool)
                .await?
                .into_iter()
                .map(|(v, _, _)| v)
                .collect();
            let newer = stored
                .iter()
                .all(|v| version_order(data, v) == std::cmp::Ordering::Greater);
            return Err(VersionRefusal::RestoreNotStored {
                version: data.to_string(),
                newer,
                stored,
            });
        }
        if pin.version.as_deref() == Some(data) {
            return Ok(RestorePin::Unchanged {
                version: data.to_string(),
            });
        }
        ensure_revocations(self.pool).await?;
        let held = if let Some(mark) = self.revocation_of(data).await? {
            Some(format!(
                "{data} is revoked ({}). Lift the mark (`ordo versions unrevoke {data}`), or \
                 bring the database to its pin",
                mark.reason
            ))
        } else {
            match self.check_served(data).await {
                Ok(()) => None,
                Err(
                    e @ (VersionRefusal::NotServed { .. } | VersionRefusal::BelowMinimum { .. }),
                ) => Some(e.to_string()),
                Err(e) => return Err(e),
            }
        };
        Ok(match held {
            Some(why) => RestorePin::Held {
                data: data.to_string(),
                pin: pin.version,
                why,
            },
            None => RestorePin::Pinned {
                change: PinChange {
                    seq: 0,
                    space: space.to_string(),
                    from: pin.version,
                    to: data.to_string(),
                    epoch: 0,
                    actor: String::new(),
                    cause: PinCause::Restore.as_str().to_string(),
                    at: 0,
                },
            },
        })
    }

    /// **Every pin now**, for a node's whole read (boot, the reconcile
    /// tick): `(space, version, epoch)` of every tenant that holds one, and
    /// the newest `$pins` row. A database with no `$spaces` answers empty.
    pub async fn all_pins(&self) -> Result<(Vec<(String, String, i64)>, i64), VersionRefusal> {
        // The head first: a change committed between the two reads is read
        // again from the stream, and a pin never moves back.
        let head = match self
            .pool
            .rows("SELECT coalesce(max(\"seq\"), 0) FROM \"$pins\"")
            .await
        {
            Ok(r) => r
                .first()
                .and_then(|r| r.first().cloned().flatten())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            Err(e) if e.is_missing_table() => return Ok((Vec::new(), 0)),
            Err(e) => return Err(e.into()),
        };
        let rows = match self
            .pool
            .rows(&format!(
                "SELECT \"$space\", \"version\", \"pin_epoch\" FROM \"$spaces\" \
                 WHERE \"version\" IS NOT NULL AND \"state\" IN {PINNED_STATES}"
            ))
            .await
        {
            Ok(rows) => rows,
            Err(e) if e.is_missing_table() || e.is_missing_column() => {
                return Ok((Vec::new(), head))
            }
            Err(e) => return Err(e.into()),
        };
        let mut pins: Vec<(String, String, i64)> = rows
            .into_iter()
            .filter_map(|r| {
                Some((
                    r.first()?.clone()?,
                    r.get(1)?.clone()?,
                    r.get(2)?.clone()?.parse().ok()?,
                ))
            })
            .collect();
        // The deployment's own space, when a rollout pinned it (§3.7).
        pins.extend(self.own_pin().await?);
        Ok((pins, head))
    }

    /// **The pin stream** (§1.8, ask 17): every `$pins` row after `seq`,
    /// oldest first, at most `limit`. See [`PINS_CHANNEL`] for why a reader
    /// that keeps the newest `seq` it read misses none.
    pub async fn pins_since(
        &self,
        seq: i64,
        limit: usize,
    ) -> Result<Vec<PinChange>, VersionRefusal> {
        let rows = match self
            .pool
            .rows(&format!(
                "SELECT \"seq\", \"space\", \"from\", \"to\", \"epoch\", \"actor\", \
                 \"cause\", \"at\" FROM \"$pins\" WHERE \"seq\" > {seq} \
                 ORDER BY \"seq\" LIMIT {limit}"
            ))
            .await
        {
            Ok(rows) => rows,
            Err(e) if e.is_missing_table() => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(rows.iter().filter_map(|r| pin_change_row(r)).collect())
    }

    /// One tenant's pin. `None` when there is no such tenant.
    pub async fn pin_of(&self, space: &str) -> Result<Option<Pin>, VersionRefusal> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"version\", \"pin_epoch\" FROM \"$spaces\" WHERE \"$space\" = {}",
                sql_lit(space)
            ))
            .await?;
        Ok(rows.first().map(|r| Pin {
            version: r.first().cloned().flatten(),
            epoch: r
                .get(1)
                .cloned()
                .flatten()
                .and_then(|e| e.parse().ok())
                .unwrap_or(0),
        }))
    }

    /// One tenant's pin changes, oldest first.
    pub async fn pin_history(&self, space: &str) -> Result<Vec<PinChange>, VersionRefusal> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"seq\", \"space\", \"from\", \"to\", \"epoch\", \"actor\", \"cause\", \
                 \"at\" FROM \"$pins\" WHERE \"space\" = {} ORDER BY \"seq\"",
                sql_lit(space)
            ))
            .await?;
        Ok(rows.iter().filter_map(|r| pin_change_row(r)).collect())
    }

    /// How many tenants are pinned to each version. A version no tenant is
    /// pinned to is absent. A database with no `$spaces` answers empty.
    pub async fn tenants_per_version(&self) -> Result<BTreeMap<String, u64>, VersionRefusal> {
        let rows = match self
            .pool
            .rows(&format!(
                "SELECT \"version\", count(*) FROM \"$spaces\" \
                 WHERE \"version\" IS NOT NULL AND \"state\" IN {PINNED_STATES} \
                 GROUP BY \"version\""
            ))
            .await
        {
            Ok(rows) => rows,
            Err(e) if e.is_missing_table() || e.is_missing_column() => return Ok(BTreeMap::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(rows
            .into_iter()
            .filter_map(|r| Some((r.first()?.clone()?, r.get(1)?.clone()?.parse().ok()?)))
            .collect())
    }

    /// **When `version` last served**, in seconds: `now` while it is
    /// active, the deployment's own space is on it, or a tenant is pinned
    /// to it; else the latest of a tenant leaving it (`$pins`) and the
    /// activation that replaced it (`$activations`). `None`: it never
    /// served, so no backup or bundle can be at it.
    pub async fn last_served(
        &self,
        version: &str,
        now: u64,
    ) -> Result<Option<u64>, VersionRefusal> {
        if self.active_version().await?.as_deref() == Some(version) {
            return Ok(Some(now));
        }
        if self.own_pin().await?.is_some_and(|(_, v, _)| v == version) {
            return Ok(Some(now));
        }
        if self
            .tenants_per_version()
            .await?
            .get(version)
            .is_some_and(|n| *n > 0)
        {
            return Ok(Some(now));
        }
        let v = sql_lit(version);
        let read = |sql: String| async move {
            match self.pool.rows(&sql).await {
                Ok(rows) => Ok(rows
                    .first()
                    .and_then(|r| r.first().cloned().flatten())
                    .and_then(|s| s.parse::<u64>().ok())),
                Err(e) if e.is_missing_table() => Ok(None),
                Err(e) => Err(VersionRefusal::from(e)),
            }
        };
        let left = read(format!(
            "SELECT max(\"at\") FROM \"$pins\" WHERE \"from\" = {v}"
        ))
        .await?;
        let replaced = read(format!(
            "SELECT max(n.\"at\") FROM \"$activations\" a, LATERAL ( \
               SELECT b.\"at\" FROM \"$activations\" b WHERE b.\"seq\" > a.\"seq\" \
               ORDER BY b.\"seq\" LIMIT 1) n \
             WHERE a.\"version\" = {v}"
        ))
        .await?;
        Ok(left.max(replaced))
    }

    /// **Whether a restore may still need `version`** (increment 17, §2.7):
    /// a database backup or an exit bundle taken while it served. Held when
    /// it served within `versions.restore_window`, or ever, when the window
    /// is unset. `None`: nothing can be at it, so it may be removed and a
    /// field only it declares may be pruned.
    pub async fn restore_hold(
        &self,
        version: &str,
        now: u64,
    ) -> Result<Option<RestoreHold>, VersionRefusal> {
        let Some(last) = self.last_served(version, now).await? else {
            return Ok(None);
        };
        let settings = self.settings().await?;
        let window = settings.restore_window_secs(None);
        let held = last.saturating_add(window) > now;
        Ok(held.then(|| RestoreHold {
            version: version.to_string(),
            last_served: last,
            serving: last >= now,
            window,
            by_default: settings.restore_window_set_secs().is_none(),
        }))
    }
}

/// A version a restore may still need ([`Versions::restore_hold`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreHold {
    pub version: String,
    /// When it last served, in seconds.
    pub last_served: u64,
    /// It serves now.
    pub serving: bool,
    /// The window in force, in seconds.
    pub window: u64,
    /// The window is the default, `versions.restore_window` being unset.
    pub by_default: bool,
}

impl RestoreHold {
    /// The hold in words, naming the setting that frees it.
    pub fn line(&self) -> String {
        let since = if self.serving {
            "serves now".to_string()
        } else {
            format!("last served {} UTC", utc_minute(self.last_served))
        };
        let source = if self.by_default {
            format!(
                "`{RESTORE_WINDOW_SETTING}` is unset, so it is the default, {} (your backup \
                 retention when Ordo knows it, else 30 days)",
                duration_words(self.window)
            )
        } else {
            format!(
                "`{RESTORE_WINDOW_SETTING}` is {}",
                duration_words(self.window)
            )
        };
        format!(
            "`{}` {since}, and {source}, so a backup or bundle taken then may still need it \
             until {} UTC. Wait, or set the window to how long you keep backups and bundles \
             (`ordo versions set restore-window <duration> --reason \"...\"`; `0` holds none)",
            self.version,
            utc_minute(self.last_served.saturating_add(self.window)),
        )
    }
}

/// A window in seconds as a person set it: `30d`, `12h`, `0`.
fn duration_words(secs: u64) -> String {
    match secs {
        0 => "0".into(),
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3_600 == 0 => format!("{}h", s / 3_600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

impl<'a> Versions<'a> {
    // ── revoking ───────────────────────────────────────────────────────

    /// A version as a person reads it, `#4 · 0.2.1`.
    pub async fn named(&self, version: &str) -> Result<String, VersionRefusal> {
        Ok(match registry::ordinal_of(self.pool, version).await? {
            Some(n) => format!("#{n} · {version}"),
            None => version.to_string(),
        })
    }

    /// The refusal for a revoked version, named as a person reads it.
    async fn revoked(&self, mark: Revocation, setting: Option<&'static str>) -> VersionRefusal {
        let named = self
            .named(&mark.version)
            .await
            .unwrap_or_else(|_| mark.version.clone());
        VersionRefusal::Revoked {
            named,
            mark: Box::new(mark),
            setting,
        }
    }

    /// **Mark the version a person named revoked** (D8), with a reason.
    ///
    /// When it is the active version, `move_to` must name a stored version
    /// that is not revoked (ask 18). The activation to it, the pins it
    /// moves and the revoke are then one transaction. A version that is not
    /// active is only marked, and `move_to`, when given, is where its
    /// tenants go when they move off it (ruled 2026-11-05): in their
    /// window or on demand, never at once.
    ///
    /// The checks an activation makes of the database (the load gate) are
    /// the caller's, before this: `crate::apply_op::revoke`.
    pub async fn revoke(
        &self,
        name: &str,
        reason: &str,
        move_to: Option<&str>,
        actor: &str,
        at: u64,
    ) -> Result<Revoked, VersionRefusal> {
        let reason = check_reason(reason)?;
        let version = self.resolve(name).await?;
        let move_to = match move_to {
            Some(m) => Some(self.resolve(m).await?),
            None => None,
        };
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, &version).await? {
            tx.rollback().await?;
            return Err(VersionRefusal::AlreadyRevoked {
                named: self.named(&version).await?,
                mark: Box::new(mark),
            });
        }
        let active = active_in(&tx).await?;
        let moved = match (active.as_deref() == Some(version.as_str()), move_to.clone()) {
            (true, None) => {
                tx.rollback().await?;
                return Err(VersionRefusal::RevokeActive {
                    named: self.named(&version).await?,
                    candidates: self.unrevoked().await?,
                });
            }
            (true, Some(to)) if to == version => {
                tx.rollback().await?;
                return Err(VersionRefusal::MoveTo(format!(
                    "it names {}, the version being revoked; name another",
                    self.named(&version).await?
                )));
            }
            (true, Some(to)) => {
                if let Some(mark) = mark_in(&tx, &to).await? {
                    tx.rollback().await?;
                    return Err(self.revoked(mark, None).await);
                }
                if let Err(e) = self.check_served(&to).await {
                    tx.rollback().await?;
                    return Err(e);
                }
                let ordinals = registry::ordinals(self.pool).await?;
                let cause = match (ordinals.get(&version), ordinals.get(&to)) {
                    (Some(was), Some(now)) if now < was => PinCause::Rollback,
                    _ => PinCause::Activate,
                };
                registry::record_activation(&tx, &to, actor, Timestamp(at)).await?;
                repin(&tx, &repin_sql(&to, actor, cause, at, None)).await?;
                Some((version.clone(), to))
            }
            (false, Some(to)) if to == version => {
                tx.rollback().await?;
                return Err(VersionRefusal::MoveTo(format!(
                    "it names {}, the version being revoked; name another, or leave it out \
                     to move its tenants to the active version",
                    self.named(&version).await?
                )));
            }
            (false, Some(to)) => {
                if let Some(mark) = mark_in(&tx, &to).await? {
                    tx.rollback().await?;
                    return Err(self.revoked(mark, None).await);
                }
                if let Err(e) = self.check_served(&to).await {
                    tx.rollback().await?;
                    return Err(e);
                }
                None
            }
            (false, None) => None,
        };
        tx.rows(&revocation_sql(
            &version,
            RevokeAct::Revoke,
            actor,
            &reason,
            at,
            move_to.as_deref(),
        ))
        .await?;
        tx.commit().await?;
        Ok(Revoked {
            version,
            moved,
            move_to,
        })
    }

    /// **Lift the revoke mark** from the version a person named (ask 20),
    /// with a reason. It can be activated and pinned again. Both acts stay
    /// in `$revocations`. Answers the version it was.
    pub async fn unrevoke(
        &self,
        name: &str,
        reason: &str,
        actor: &str,
        at: u64,
    ) -> Result<String, VersionRefusal> {
        let reason = check_reason(reason)?;
        let version = self.resolve(name).await?;
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if mark_in(&tx, &version).await?.is_none() {
            tx.rollback().await?;
            return Err(VersionRefusal::NotRevoked {
                named: self.named(&version).await?,
            });
        }
        tx.rows(&revocation_sql(
            &version,
            RevokeAct::Unrevoke,
            actor,
            &reason,
            at,
            None,
        ))
        .await?;
        tx.commit().await?;
        Ok(version)
    }

    /// **The last step of removing a stored version** (§1.6): under the pin
    /// lock, check again that it is not active and no tenant is pinned to
    /// it, then leave its tombstone ([`registry::mark_removed`]) and a
    /// `$version_removals` row with who, why, where its tenants went and
    /// how many. Every other check is the caller's, before this
    /// (`crate::version_removal`). Answers the tenants still pinned to it
    /// when there are any, and removes nothing then.
    pub async fn remove_stored(
        &self,
        version: &str,
        reason: &str,
        moved_to: Option<&str>,
        moved: usize,
        actor: &str,
        at: u64,
    ) -> Result<Vec<String>, VersionRefusal> {
        self.ensure().await?;
        self.pool.rows(REMOVALS_DDL).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if active_in(&tx).await?.as_deref() == Some(version) {
            tx.rollback().await?;
            return Err(VersionRefusal::MoveTo(format!(
                "{} became the active version meanwhile, and the active version is not removed",
                self.named(version).await?
            )));
        }
        let left: Vec<String> = tx
            .rows(&format!(
                "SELECT \"$space\" FROM \"$spaces\" WHERE \"version\" = {} \
                 AND \"state\" IN {PINNED_STATES} ORDER BY \"$space\"",
                sql_lit(version)
            ))
            .await?
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect();
        if !left.is_empty() {
            tx.rollback().await?;
            return Ok(left);
        }
        registry::mark_removed(&tx, version, at).await?;
        tx.rows(&format!(
            "INSERT INTO \"$version_removals\" \
               (\"version\", \"actor\", \"reason\", \"moved_to\", \"moved\", \"at\") \
             VALUES ({}, {}, {}, {}, {moved}, {at})",
            sql_lit(version),
            sql_lit(actor),
            sql_lit(reason),
            moved_to.map(sql_lit).unwrap_or_else(|| "NULL".into()),
        ))
        .await?;
        tx.commit().await?;
        Ok(Vec::new())
    }

    /// Every removal, newest first: `(version, actor, reason, moved_to,
    /// moved, at)`. Empty before the first.
    pub async fn removals(&self) -> Result<Vec<Removal>, VersionRefusal> {
        match self
            .pool
            .rows(
                "SELECT \"version\", \"actor\", \"reason\", \"moved_to\", \"moved\", \"at\" \
                 FROM \"$version_removals\" ORDER BY \"seq\" DESC",
            )
            .await
        {
            Ok(rows) => Ok(rows
                .into_iter()
                .filter_map(|r| {
                    Some(Removal {
                        version: r.first()?.clone()?,
                        actor: r.get(1)?.clone()?,
                        reason: r.get(2)?.clone()?,
                        moved_to: r.get(3).cloned().flatten(),
                        moved: r.get(4)?.clone()?.parse().ok()?,
                        at: r.get(5)?.clone()?.parse().ok()?,
                    })
                })
                .collect()),
            Err(e) if e.is_missing_table() => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Every revoked version's mark. A database with no `$revocations`
    /// answers empty.
    pub async fn revocations(&self) -> Result<BTreeMap<String, Revocation>, VersionRefusal> {
        Ok(revocation_rows(self.pool, &mark_sql(None))
            .await?
            .iter()
            .filter_map(|r| revocation_row(r))
            .map(|r| (r.version.clone(), r))
            .collect())
    }

    /// One version's mark, `None` when it is not revoked.
    pub async fn revocation_of(&self, version: &str) -> Result<Option<Revocation>, VersionRefusal> {
        Ok(revocation_rows(self.pool, &mark_sql(Some(version)))
            .await?
            .first()
            .and_then(|r| revocation_row(r)))
    }

    /// Every revoke and unrevoke of one version, oldest first.
    pub async fn revocation_history(
        &self,
        version: &str,
    ) -> Result<Vec<Revocation>, VersionRefusal> {
        let rows = revocation_rows(
            self.pool,
            &format!(
                "SELECT \"seq\", \"version\", \"act\", \"actor\", \"reason\", \"at\", \
                   \"move_to\" \
                 FROM \"$revocations\" WHERE \"version\" = {} ORDER BY \"seq\"",
                sql_lit(version)
            ),
        )
        .await?;
        Ok(rows.iter().filter_map(|r| revocation_row(r)).collect())
    }

    /// Every stored version that is not revoked, `#n · name`, oldest first.
    async fn unrevoked(&self) -> Result<Vec<String>, VersionRefusal> {
        let marks = self.revocations().await?;
        let ordinals = registry::ordinals(self.pool).await?;
        let mut out: Vec<(Option<u64>, String)> = registry::stored(self.pool)
            .await?
            .into_iter()
            .map(|(v, _, _)| v)
            .filter(|v| !marks.contains_key(v))
            .map(|v| (ordinals.get(&v).copied(), v))
            .collect();
        out.sort();
        Ok(out
            .into_iter()
            .map(|(n, v)| match n {
                Some(n) => format!("#{n} · {v}"),
                None => v,
            })
            .collect())
    }

    /// **The report D8 asks for**: each revoked version, and the tenants
    /// pinned to it in a pinned state, oldest version first.
    pub async fn pinned_to_revoked(&self) -> Result<Vec<RevokedVersion>, VersionRefusal> {
        let marks = self.revocations().await?;
        // Nothing revoked, nothing to report. A deployment with nothing
        // applied has no `$artifacts` to read order numbers from either.
        if marks.is_empty() {
            return Ok(Vec::new());
        }
        let ordinals = registry::ordinals(self.pool).await?;
        // The tenants' own targets live beside `$spaces`, made by the first
        // pin change since increment 3.
        ensure_revocations(self.pool).await?;
        let mut out = Vec::with_capacity(marks.len());
        for (version, mark) in marks {
            let rows = match self
                .pool
                .rows(&format!(
                    "SELECT s.\"$space\", coalesce(s.\"name\", ''), s.\"state\", \
                       count(*) OVER (), t.\"version\" FROM \"$spaces\" s \
                     LEFT JOIN \"$move_targets\" t ON t.\"space\" = s.\"$space\" \
                     WHERE s.\"version\" = {} AND s.\"state\" IN {PINNED_STATES} \
                     ORDER BY s.\"$space\" LIMIT {MAX_REVOKED_TENANTS}",
                    sql_lit(&version)
                ))
                .await
            {
                Ok(rows) => rows,
                Err(e) if e.is_missing_table() || e.is_missing_column() => vec![],
                Err(e) => return Err(e.into()),
            };
            let tenant_count = rows
                .first()
                .and_then(|r| r.get(3).cloned().flatten())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            out.push(RevokedVersion {
                ordinal: ordinals.get(&version).copied(),
                tenants: rows
                    .iter()
                    .filter_map(|r| {
                        Some(RevokedPin {
                            space: r.first()?.clone()?,
                            name: r.get(1)?.clone()?,
                            state: r.get(2)?.clone()?,
                            target: r.get(4).cloned().flatten(),
                            next_window: None,
                        })
                    })
                    .collect(),
                tenant_count,
                mark,
            });
        }
        out.sort_by_key(|r| r.ordinal);
        Ok(out)
    }

    // ── moving off a revoked version (ruled 2026-11-05) ───────────────────

    /// **Set where one tenant goes off a revoked version**, or clear it
    /// (`None`), so the revoke's target or the active version applies. The
    /// deployment owner's act. A version not stored, or revoked, is
    /// refused, and so is a tenant that is not `provisioning`, `active` or
    /// `paused`. Answers the version set.
    pub async fn set_move_target(
        &self,
        space: &str,
        name: Option<&str>,
        actor: &str,
        at: u64,
    ) -> Result<Option<String>, VersionRefusal> {
        let version = match name {
            Some(n) => Some(self.resolve(n).await?),
            None => None,
        };
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        // Under the lock, as a pin reads it (audit L4).
        let policy = self.policy_in(&tx).await?;
        if let Some(v) = &version {
            if let Err(e) = self.with_policy(&policy).check_served(v).await {
                tx.rollback().await?;
                return Err(e);
            }
        }
        let held = tx
            .rows(&format!(
                "SELECT 1 FROM \"$spaces\" WHERE \"$space\" = {} AND \"state\" IN {PINNED_STATES}",
                sql_lit(space)
            ))
            .await?;
        if held.is_empty() {
            tx.rollback().await?;
            return Err(VersionRefusal::NoTenant {
                space: space.to_string(),
            });
        }
        match &version {
            Some(v) => {
                if let Some(mark) = mark_in(&tx, v).await? {
                    tx.rollback().await?;
                    return Err(self.revoked(mark, None).await);
                }
                tx.rows(&format!(
                    "INSERT INTO \"$move_targets\" (\"space\", \"version\", \"set_by\", \"set_at\") \
                     VALUES ({s}, {v}, {a}, {at}) ON CONFLICT (\"space\") DO UPDATE SET \
                     \"version\" = EXCLUDED.\"version\", \"set_by\" = EXCLUDED.\"set_by\", \
                     \"set_at\" = EXCLUDED.\"set_at\"",
                    s = sql_lit(space),
                    v = sql_lit(v),
                    a = sql_lit(actor),
                ))
                .await?;
            }
            None => {
                tx.rows(&format!(
                    "DELETE FROM \"$move_targets\" WHERE \"space\" = {}",
                    sql_lit(space)
                ))
                .await?;
            }
        }
        tx.commit().await?;
        Ok(version)
    }

    /// The tenant's own move target, when one is set.
    pub async fn move_target_of(&self, space: &str) -> Result<Option<String>, VersionRefusal> {
        match self
            .pool
            .rows(&format!(
                "SELECT \"version\" FROM \"$move_targets\" WHERE \"space\" = {}",
                sql_lit(space)
            ))
            .await
        {
            Ok(rows) => Ok(rows.first().and_then(|r| r.first().cloned().flatten())),
            Err(e) if e.is_missing_table() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// **Where a tenant on a revoked version goes**, and whose choice that
    /// was: its own target, the revoke's, or the active version. `mark` is
    /// the revoke of the version it is on.
    async fn move_target_in(
        tx: &ordo_exec::TxConn,
        space: &str,
        mark: Option<&Revocation>,
    ) -> Result<Option<MoveTarget>, PoolError> {
        let own = tx
            .rows(&format!(
                "SELECT \"version\" FROM \"$move_targets\" WHERE \"space\" = {}",
                sql_lit(space)
            ))
            .await?;
        if let Some(v) = own.first().and_then(|r| r.first().cloned().flatten()) {
            return Ok(Some(MoveTarget {
                version: v,
                source: TargetSource::Tenant,
            }));
        }
        if let Some(v) = mark.and_then(|m| m.move_to.as_ref()) {
            return Ok(Some(MoveTarget {
                version: v.clone(),
                source: TargetSource::Revoke,
            }));
        }
        Ok(active_in(tx).await?.map(|v| MoveTarget {
            version: v,
            source: TargetSource::Active,
        }))
    }

    /// **Move one tenant off the revoked version it is on**, to the target
    /// the app developer chose (the module doc's order), now. The one move
    /// every path calls: the window sweep (`window`), the deployment owner
    /// or a system on demand (`demand`), the tenant when granted
    /// (`tenant`).
    ///
    /// Refused: a tenant that is not `provisioning`, `active` or `paused`,
    /// one whose version is not revoked, and a target that is revoked or
    /// no longer stored. One transaction under the pin lock, so two nodes
    /// sweeping the same window move the tenant once and the second is
    /// told it is not on a revoked version.
    pub async fn move_off_revoked(
        &self,
        space: &str,
        actor: &str,
        cause: PinCause,
        at: u64,
    ) -> Result<MovedOff, VersionRefusal> {
        ensure_revocations(self.pool).await?;
        ensure_build_tables(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        // A tenant build moves its own tenants (increment 16).
        if let Some((build, on)) = build_of_in(&tx, space).await? {
            tx.rollback().await?;
            return Err(VersionRefusal::InBuild {
                space: space.to_string(),
                build,
                version: on,
            });
        }
        // Under the lock, as a pin reads it (audit L4).
        let policy = self.policy_in(&tx).await?;
        let below = match self.with_policy(&policy).below_minimum().await {
            Ok(b) => b,
            Err(e) => {
                tx.rollback().await?;
                return Err(e);
            }
        };
        let held = tx
            .rows(&format!(
                "SELECT \"version\" FROM \"$spaces\" \
                 WHERE \"$space\" = {} AND \"state\" IN {PINNED_STATES}",
                sql_lit(space)
            ))
            .await?;
        let Some(row) = held.first() else {
            tx.rollback().await?;
            return Err(VersionRefusal::NoTenant {
                space: space.to_string(),
            });
        };
        let from = row.first().cloned().flatten();
        let mark = match &from {
            Some(v) => mark_in(&tx, v).await?,
            None => None,
        };
        // Off a revoked version, or one below `versions.minimum`.
        let below = from.as_ref().is_some_and(|v| below.contains(v));
        let (Some(from), true) = (from.clone(), mark.is_some() || below) else {
            tx.rollback().await?;
            return Err(VersionRefusal::NotOnRevoked {
                space: space.to_string(),
                version: match &from {
                    Some(v) => Some(self.named(v).await?),
                    None => None,
                },
            });
        };
        let Some(target) = Self::move_target_in(&tx, space, mark.as_ref()).await? else {
            tx.rollback().await?;
            return Err(VersionRefusal::NotStored {
                asked: "the active version".into(),
                stored: vec![],
            });
        };
        if let Some(bad) = mark_in(&tx, &target.version).await? {
            tx.rollback().await?;
            return Err(VersionRefusal::TargetRevoked {
                space: space.to_string(),
                named: self.named(&target.version).await?,
                source: target.source,
                mark: Box::new(bad),
            });
        }
        if registry::get(self.pool, &target.version).await?.is_none() {
            tx.rollback().await?;
            return Err(VersionRefusal::NotStored {
                asked: target.version,
                stored: self.unrevoked().await?,
            });
        }
        if let Err(e) = self.check_served(&target.version).await {
            tx.rollback().await?;
            return Err(e);
        }
        let filter = format!(
            "AND \"$space\" = {} AND \"version\" = {}",
            sql_lit(space),
            sql_lit(&from)
        );
        repin(
            &tx,
            &repin_sql(&target.version, actor, cause, at, Some(&filter)),
        )
        .await?;
        tx.commit().await?;
        let epoch = self.pin_of(space).await?.map(|p| p.epoch).unwrap_or(0);
        Ok(MovedOff {
            change: PinChange {
                seq: 0,
                space: space.to_string(),
                from: Some(from),
                to: target.version,
                epoch,
                actor: actor.to_string(),
                cause: cause.as_str().to_string(),
                at,
            },
            source: target.source,
        })
    }

    /// Every tenant on a revoked version, or on one below
    /// `versions.minimum`, `(space, version)`, at most `limit`, by tenant
    /// id. A database with no pins answers empty.
    ///
    /// `after` reads the next page: only tenants whose id sorts after it.
    /// `only` narrows the tenants in SQL (`AND ...` on `"$space"`), so a
    /// page holds only the tenants a caller can act on, and those it
    /// cannot never crowd out the rest.
    pub async fn on_revoked(
        &self,
        limit: usize,
        after: Option<&str>,
        only: Option<&str>,
    ) -> Result<Vec<(String, String)>, VersionRefusal> {
        let below = self.below_minimum().await?;
        if below.is_empty() && self.revocations().await?.is_empty() {
            return Ok(vec![]);
        }
        let below = if below.is_empty() {
            String::new()
        } else {
            format!(
                "OR \"version\" IN ({})",
                below
                    .iter()
                    .map(|v| sql_lit(v))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let rows = match self
            .pool
            .rows(&format!(
                "SELECT \"$space\", \"version\" FROM \"$spaces\" \
                 WHERE \"state\" IN {PINNED_STATES} AND (\"version\" IN \
                   (SELECT \"version\" FROM ({}) m) {below}) {after} {only} \
                 ORDER BY \"$space\" LIMIT {limit}",
                mark_sql(None),
                after = after
                    .map(|a| format!("AND \"$space\" > {}", sql_lit(a)))
                    .unwrap_or_default(),
                only = only.unwrap_or_default(),
            ))
            .await
        {
            Ok(rows) => rows,
            Err(e) if e.is_missing_table() || e.is_missing_column() => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        Ok(rows
            .into_iter()
            .filter_map(|r| Some((r.first()?.clone()?, r.get(1)?.clone()?)))
            .collect())
    }
}

// ── the version settings (owner, 2026-11-05) ───────────────────────────

impl<'a> Versions<'a> {
    /// **The three version settings as stored.** A database with no table
    /// yet answers each unset.
    pub async fn settings(&self) -> Result<VersionSettings, VersionRefusal> {
        match self.pool.rows(SETTINGS_NOW_SQL).await {
            Ok(rows) => Ok(settings_of(&rows)),
            Err(e) if e.is_missing_table() => Ok(VersionSettings::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Every change to them, newest first, at most `limit`
    /// ([`MAX_SETTING_HISTORY`]).
    pub async fn setting_history(
        &self,
        limit: usize,
    ) -> Result<Vec<SettingChange>, VersionRefusal> {
        let sql = format!(
            "SELECT \"seq\", \"setting\", \"value\", \"actor\", \"reason\", \"at\" \
             FROM \"$version_settings\" ORDER BY \"seq\" DESC LIMIT {}",
            limit.clamp(1, MAX_SETTING_HISTORY)
        );
        match self.pool.rows(&sql).await {
            Ok(rows) => Ok(rows.iter().filter_map(|r| change_row(r)).collect()),
            Err(e) if e.is_missing_table() => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    /// **Check a value for one setting** against the others as stored, and
    /// answer it resolved: `#n` named as its version, `None` for unset
    /// (`in-use` for served, `none` for minimum, empty for any). The checks
    /// that ran at boot before 2026-11-05:
    ///
    /// - each version named is stored ([`VersionRefusal::Setting`]);
    /// - `versions.served` strands no tenant ([`VersionRefusal::Stranded`]);
    /// - `versions.minimum` is not above the active version
    ///   ([`VersionRefusal::MinimumAboveActive`]);
    /// - `versions.new_tenant` is not revoked, and the served policy serves
    ///   it.
    ///
    /// Each refusal names the setting and the value.
    pub async fn check_setting(
        &self,
        setting: VersionSetting,
        value: Option<&str>,
    ) -> Result<(Option<String>, VersionSettings), VersionRefusal> {
        let value = value.map(str::trim).filter(|v| !v.is_empty());
        // A load guard is a duration, checked alone: it names no version.
        if setting.is_guard() {
            let now = self.settings().await?;
            let value = match value {
                Some(v) => match check_guard(setting, v) {
                    Some(v) => Some(v),
                    None => {
                        return Err(VersionRefusal::Guard {
                            setting: setting.name(),
                            value: v.chars().take(40).collect(),
                        })
                    }
                },
                None => None,
            };
            let resolved = now.with(setting, value);
            return Ok((resolved.value(setting), resolved));
        }
        let value = match (setting, value) {
            (VersionSetting::Served, Some("in-use")) | (VersionSetting::Minimum, Some("none")) => {
                None
            }
            (_, v) => v.map(str::to_string),
        };
        let now = self.settings().await?;
        let candidate = now.with(setting, value);
        let stored_only = Versions {
            policy: None,
            new_tenant: None,
            ..*self
        };
        let policy = match setting {
            // The policy is checked whole, so a change to either half
            // never leaves the pair stranding a tenant.
            VersionSetting::Served | VersionSetting::Minimum => {
                stored_only.check_policy(&candidate.policy).await?
            }
            _ => candidate.policy.clone(),
        };
        let mut resolved = VersionSettings {
            policy,
            new_tenant: None,
            ..candidate.clone()
        };
        if let Some(named) = &candidate.new_tenant {
            let version = match self.resolve(named).await {
                Ok(v) => v,
                Err(VersionRefusal::NotStored { .. }) | Err(VersionRefusal::UnknownVersion(_)) => {
                    return Err(VersionRefusal::Setting {
                        setting: NEW_TENANT_SETTING,
                        value: named.clone(),
                        stored: self.unrevoked().await?,
                        hint: "or `--clear` it to start new tenants on the active version",
                    })
                }
                Err(e) => return Err(e),
            };
            stored_only
                .with_policy(&resolved.policy)
                .with_new_tenant(Some(&version))
                .check_new_tenant()
                .await?;
            resolved.new_tenant = Some(version);
        }
        Ok((resolved.value(setting), resolved))
    }

    /// **Change one version setting**, or clear it with `None`, with who
    /// and why. Checked first ([`Versions::check_setting`]), then written
    /// under the pin lock with the pin stream's nudge, so every node reads
    /// it again at once ([`crate::pin_stream`]) and at the latest on its
    /// reconcile tick. A value the setting already holds writes nothing.
    pub async fn set_setting(
        &self,
        setting: VersionSetting,
        value: Option<&str>,
        actor: &str,
        reason: &str,
        at: u64,
    ) -> Result<SettingSet, VersionRefusal> {
        let reason = check_reason(reason)?;
        self.ensure().await?;
        let (value, _) = self.check_setting(setting, value).await?;
        self.record_setting(setting, value, actor, &reason, at, false)
            .await
            .map(|s| s.expect("an unconditional write answers"))
    }

    /// Write one setting's row. `only_if_never_set`: write nothing, and
    /// answer `None`, when the setting has any row at all.
    async fn record_setting(
        &self,
        setting: VersionSetting,
        value: Option<String>,
        actor: &str,
        reason: &str,
        at: u64,
        only_if_never_set: bool,
    ) -> Result<Option<SettingSet>, VersionRefusal> {
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if only_if_never_set {
            let any = tx
                .rows(&format!(
                    "SELECT 1 FROM \"$version_settings\" WHERE \"setting\" = {} LIMIT 1",
                    sql_lit(setting.name())
                ))
                .await?;
            if !any.is_empty() {
                tx.rollback().await?;
                return Ok(None);
            }
        }
        let now = settings_in(&tx).await?;
        if now.value(setting) == value {
            tx.rollback().await?;
            return Ok(Some(SettingSet::Unchanged(now)));
        }
        tx.rows(&format!(
            "INSERT INTO \"$version_settings\" (\"setting\", \"value\", \"actor\", \"reason\", \"at\") \
             VALUES ({}, {}, {}, {}, {at})",
            sql_lit(setting.name()),
            value.as_deref().map(sql_lit).unwrap_or_else(|| "NULL".into()),
            sql_lit(actor),
            sql_lit(reason),
        ))
        .await?;
        tx.notify(PINS_CHANNEL, "").await?;
        tx.commit().await?;
        Ok(Some(SettingSet::Changed(now.with(setting, value))))
    }

    /// **Adopt a version setting still set in the environment**, once
    /// (owner, 2026-11-05). For each of the three:
    ///
    /// - not in the environment: nothing;
    /// - set, and the setting was never stored: stored as [`SYSTEM_ACTOR`]
    ///   with the checks a change has, and a line says the variable is now
    ///   ignored and can be removed;
    /// - set, and the setting is stored: a line, on every boot, that it is
    ///   ignored.
    ///
    /// A value the checks refuse is not stored, and its line says why. It
    /// never refuses: an upgrade never stops a running deployment (owner,
    /// 2026-11-02). The lines, for the node's log.
    pub async fn adopt_env(&self, at: u64) -> Vec<String> {
        self.adopt_from(|var| std::env::var(var).ok(), at).await
    }

    /// [`Versions::adopt_env`], reading the environment through `read`.
    pub async fn adopt_from(&self, read: impl Fn(&str) -> Option<String>, at: u64) -> Vec<String> {
        let mut out = Vec::new();
        if let Err(e) = self.ensure().await {
            out.push(format!(
                "versions: the version settings were not read, so no environment value was \
                 adopted ({e})"
            ));
            return out;
        }
        for s in VersionSetting::ALL {
            if s.env().is_empty() {
                continue;
            }
            // Empty was unset before 2026-11-05 too.
            let Some(raw) = read(s.env())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            let checked = self.check_setting(s, Some(raw.as_str())).await;
            let written = match checked {
                Ok((value, _)) => {
                    self.record_setting(
                        s,
                        value,
                        SYSTEM_ACTOR,
                        &format!("adopted from {} at boot", s.env()),
                        at,
                        true,
                    )
                    .await
                }
                Err(e) => {
                    // Refused, unless something is stored already: then
                    // the variable is only ignored.
                    match self.record_setting_exists(s).await {
                        Ok(true) => Ok(None),
                        Ok(false) => {
                            out.push(format!(
                                "versions: {} is `{raw}` and was not stored, so it is ignored: \
                                 {e}. Set {} with `{} <value>`, then remove {}",
                                s.env(),
                                s.name(),
                                s.command(),
                                s.env()
                            ));
                            continue;
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            match written {
                Ok(Some(SettingSet::Changed(_))) => out.push(format!(
                    "versions: {} was set from {} (`{raw}`) and is now stored in Ordo. {} is \
                     ignored from now on and can be removed; `{}` changes the setting",
                    s.name(),
                    s.env(),
                    s.env(),
                    s.command()
                )),
                Ok(Some(SettingSet::Unchanged(_))) | Ok(None) => {
                    let stored = self
                        .settings()
                        .await
                        .ok()
                        .and_then(|now| now.value(s))
                        .unwrap_or_else(|| s.unset_reads().to_string());
                    out.push(format!(
                        "versions: {} is set and ignored: {} is stored in Ordo as `{stored}`. \
                         Remove the variable; `{}` changes the setting",
                        s.env(),
                        s.name(),
                        s.command()
                    ));
                }
                Err(e) => out.push(format!(
                    "versions: {} was not adopted ({e}); the next boot tries again",
                    s.env()
                )),
            }
        }
        out
    }

    /// Has `setting` ever been stored, cleared included?
    async fn record_setting_exists(&self, setting: VersionSetting) -> Result<bool, VersionRefusal> {
        match self
            .pool
            .rows(&format!(
                "SELECT 1 FROM \"$version_settings\" WHERE \"setting\" = {} LIMIT 1",
                sql_lit(setting.name())
            ))
            .await
        {
            Ok(rows) => Ok(!rows.is_empty()),
            Err(e) if e.is_missing_table() => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// **The stored settings, checked at boot.** One line per problem, for
    /// the log; nothing is refused, because an upgrade never stops a
    /// running deployment (owner, 2026-11-02). A problem here also refuses
    /// the pins it touches, naming the setting.
    pub async fn settings_problems(&self) -> Vec<String> {
        let stored = match self.settings().await {
            Ok(s) => s,
            Err(e) => {
                return vec![format!(
                    "versions: the version settings were not read ({e})"
                )]
            }
        };
        let mut out = Vec::new();
        if let Err(e) = self.check_policy(&stored.policy).await {
            out.push(format!("versions: a stored setting needs a look: {e}"));
        }
        if let Err(e) = self.check_new_tenant().await {
            out.push(format!("versions: a stored setting needs a look: {e}"));
        }
        out
    }
}

fn change_row(r: &[Option<String>]) -> Option<SettingChange> {
    Some(SettingChange {
        seq: r.first()?.clone()?.parse().ok()?,
        setting: r.get(1)?.clone()?,
        value: r.get(2).cloned().flatten(),
        actor: r.get(3)?.clone()?,
        reason: r.get(4)?.clone()?,
        at: r.get(5)?.clone()?.parse().ok()?,
    })
}

fn revocation_sql(
    version: &str,
    act: RevokeAct,
    actor: &str,
    reason: &str,
    at: u64,
    move_to: Option<&str>,
) -> String {
    format!(
        "INSERT INTO \"$revocations\" (\"version\", \"act\", \"actor\", \"reason\", \"at\", \
           \"move_to\") \
         VALUES ({}, {}, {}, {}, {at}, {})",
        sql_lit(version),
        sql_lit(act.as_str()),
        sql_lit(actor),
        sql_lit(reason),
        move_to.map(sql_lit).unwrap_or_else(|| "NULL".to_string()),
    )
}

/// One statement that moves every tenant in a pinned state, and not yet on
/// `to`, onto `to`: the pin, its epoch, and a `$pins` row each. `filter`
/// narrows the tenants (`AND ...`). Returns one row per tenant moved.
///
/// One statement, so the rows it reads are locked (`FOR UPDATE`) for the
/// update that follows, and the history it writes is the change it made.
fn pin_change_row(r: &[Option<String>]) -> Option<PinChange> {
    Some(PinChange {
        seq: r.first()?.clone()?.parse().ok()?,
        space: r.get(1)?.clone()?,
        from: r.get(2)?.clone(),
        to: r.get(3)?.clone()?,
        epoch: r.get(4)?.clone()?.parse().ok()?,
        actor: r.get(5)?.clone()?,
        cause: r.get(6)?.clone()?,
        at: r.get(7)?.clone()?.parse().ok()?,
    })
}

fn repin_sql(to: &str, actor: &str, cause: PinCause, at: u64, filter: Option<&str>) -> String {
    let to = sql_lit(to);
    format!(
        "WITH old AS ( \
           SELECT \"$space\", \"version\" FROM \"$spaces\" \
           WHERE \"state\" IN {PINNED_STATES} AND \"version\" IS DISTINCT FROM {to} {} \
           FOR UPDATE), \
         moved AS ( \
           UPDATE \"$spaces\" s SET \"version\" = {to}, \"pin_epoch\" = s.\"pin_epoch\" + 1 \
           FROM old WHERE s.\"$space\" = old.\"$space\" \
           RETURNING s.\"$space\" AS \"space\", old.\"version\" AS \"from\", s.\"pin_epoch\") \
         INSERT INTO \"$pins\" (\"space\", \"from\", \"to\", \"epoch\", \"actor\", \"cause\", \"at\") \
         SELECT \"space\", \"from\", {to}, \"pin_epoch\", {}, {}, {at} FROM moved \
         RETURNING \"space\"",
        filter.unwrap_or(""),
        sql_lit(actor),
        sql_lit(cause.as_str()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_print_as_utc_to_the_minute() {
        assert_eq!(utc_minute(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_minute(1_762_351_320), "2025-11-05 14:02 UTC");
        assert_eq!(utc_minute(1_709_164_799), "2024-02-28 23:59 UTC");
        assert_eq!(utc_minute(1_709_164_800), "2024-02-29 00:00 UTC");
    }

    #[test]
    fn a_bad_setting_names_the_setting_and_the_value() {
        let e = VersionRefusal::Setting {
            setting: NEW_TENANT_SETTING,
            value: "9.9.9".into(),
            stored: vec!["0.1.0".into()],
            hint: "or `--clear` it",
        };
        let text = e.to_string();
        assert!(text.contains("versions.new_tenant"), "{text}");
        assert!(text.contains("ordo versions set new-tenant"), "{text}");
        assert!(!text.contains(NEW_TENANT_ENV), "{text}");
        assert!(text.contains("9.9.9"), "{text}");
        assert!(text.contains("0.1.0"), "{text}");
        assert_eq!(e.remedy(), Remedy::FixInput);
        assert_eq!(e.to_exec_error("x").code, ErrorCode::InvalidParam);
    }

    #[test]
    fn a_setting_is_named_three_ways() {
        for (text, want) in [
            ("served", VersionSetting::Served),
            ("versions.served", VersionSetting::Served),
            ("minimum", VersionSetting::Minimum),
            ("new-tenant", VersionSetting::NewTenant),
            ("versions.new_tenant", VersionSetting::NewTenant),
            ("newTenant", VersionSetting::NewTenant),
        ] {
            assert_eq!(VersionSetting::parse(text), Some(want), "{text}");
        }
        assert_eq!(VersionSetting::parse("versions.max"), None);
        let s = VersionSettings::default().with(VersionSetting::Served, Some("a, b".into()));
        assert_eq!(s.policy.served, Some(vec!["a".into(), "b".into()]));
        assert_eq!(s.value(VersionSetting::Served).as_deref(), Some("a,b"));
        assert_eq!(s.value(VersionSetting::NewTenant), None);
    }

    #[test]
    fn each_refusal_has_one_door_answer() {
        let unknown = VersionRefusal::UnknownVersion("v9".into());
        assert_eq!(unknown.to_exec_error("x").code, ErrorCode::NotFound);
        let collision = VersionRefusal::Collision {
            version: "v1".into(),
        };
        assert_eq!(collision.to_exec_error("x").code, ErrorCode::Duplicate);
        assert_eq!(collision.remedy(), Remedy::Refuse);
    }

    #[test]
    fn a_label_is_one_line_of_at_most_80_characters() {
        assert_eq!(check_label("  launch day  ").unwrap(), "launch day");
        assert!(check_label(&"x".repeat(80)).is_ok());
        let long = check_label(&"x".repeat(81)).unwrap_err();
        assert!(long.to_string().contains("at most 80"), "{long}");
        assert_eq!(long.remedy(), Remedy::FixInput);
        assert_eq!(long.to_exec_error("x").code, ErrorCode::InvalidParam);
        let two = check_label("one\ntwo").unwrap_err().to_string();
        assert!(two.contains("one line"), "{two}");
        assert!(check_label("   ")
            .unwrap_err()
            .to_string()
            .contains("--clear"));
        // Characters, not bytes: 80 of them in a script of 3 bytes each.
        assert!(check_label(&"é".repeat(80)).is_ok());
    }

    #[test]
    fn a_commit_is_hex_and_may_be_dirty() {
        let full = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(check_git_commit(full).unwrap(), full);
        assert!(check_git_commit(&format!("{full}-dirty")).is_ok());
        assert!(check_git_commit("main").is_err());
        assert!(check_git_commit("ABCDEF0").is_err());
        assert_eq!(short_commit(full), "0123456789ab");
        assert_eq!(short_commit(&format!("{full}-dirty")), "0123456789ab-dirty");
    }

    #[test]
    fn a_missing_name_lists_what_is_stored() {
        let e = VersionRefusal::NotStored {
            asked: "#9".into(),
            stored: vec!["#1 · 0.1.0".into(), "#2 · 0.2.0".into()],
        };
        let text = e.to_string();
        assert!(text.contains("#9") && text.contains("#2 · 0.2.0"), "{text}");
        assert_eq!(e.to_exec_error("x").code, ErrorCode::NotFound);
    }

    #[test]
    fn a_moved_refusal_reads_as_the_registry_wrote_it() {
        let v = VersionRefusal::from(RegistryRefusal::UnknownVersion("v9".into()));
        assert_eq!(
            v.to_string(),
            RegistryRefusal::UnknownVersion("v9".into()).to_string()
        );
    }
}
