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
//!   `versions.new_tenant` (at boot and when a tenant is pinned), and as
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

/// The setting naming the version a new tenant starts on (§7).
pub const NEW_TENANT_SETTING: &str = "versions.new_tenant";
/// Its environment variable.
pub const NEW_TENANT_ENV: &str = "ORDO_VERSIONS_NEW_TENANT";

/// The setting naming which versions this deployment serves (§1.6, D15).
pub const SERVED_SETTING: &str = "versions.served";
/// Its environment variable: `in-use`, the default, or a comma list.
pub const SERVED_ENV: &str = "ORDO_VERSIONS_SERVED";
/// The setting naming the oldest version a tenant may stay on (§1.6).
pub const MINIMUM_SETTING: &str = "versions.minimum";
/// Its environment variable. Unset: none.
pub const MINIMUM_ENV: &str = "ORDO_VERSIONS_MINIMUM";

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
/// A change that would strand a tenant is refused at boot, naming the
/// setting and the value to add ([`Versions::check_policy`]).
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

    /// From [`SERVED_ENV`] and [`MINIMUM_ENV`].
    pub fn from_env() -> ServedPolicy {
        ServedPolicy::parse(
            std::env::var(SERVED_ENV).ok().as_deref(),
            std::env::var(MINIMUM_ENV).ok().as_deref(),
        )
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

static POLICY: std::sync::OnceLock<ServedPolicy> = std::sync::OnceLock::new();
static NO_POLICY: ServedPolicy = ServedPolicy {
    served: None,
    minimum: None,
};

/// Install the deployment's policy, once, at boot, after
/// [`Versions::check_policy`] resolved it. A second call is ignored.
pub fn install_policy(policy: ServedPolicy) {
    let _ = POLICY.set(policy);
}

/// The policy every [`Versions::on`] checks pins against: what boot
/// installed, or `in-use` and no minimum.
pub fn policy() -> &'static ServedPolicy {
    POLICY.get().unwrap_or(&NO_POLICY)
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
/// deadline (§2.1). Created now so the shape is settled; nothing writes it
/// until increment 7.
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
  \"started_at\" bigint NOT NULL \
)";

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
    Ok(())
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
    Ok(())
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
        tx.rows(&format!("SELECT pg_notify({}, '')", sql_lit(PINS_CHANNEL)))
            .await?;
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
        var: &'static str,
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
    /// `versions.new_tenant` named it: `(setting, variable)`.
    Revoked {
        named: String,
        mark: Box<Revocation>,
        setting: Option<(&'static str, &'static str)>,
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
    /// would leave new tenants unpinned and refuse the next boot, so the
    /// setting changes first.
    NamedBySetting {
        named: String,
        setting: &'static str,
        var: &'static str,
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
    /// boot refuses it, naming the value to add.
    Stranded {
        version: String,
        tenants: u64,
        list: Vec<String>,
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
            | VersionRefusal::Stranded { .. }
            | VersionRefusal::Revoked {
                setting: Some(_), ..
            } => Remedy::FixInput,
            VersionRefusal::Collision { .. }
            | VersionRefusal::Revoked { .. }
            | VersionRefusal::AlreadyRevoked { .. }
            | VersionRefusal::NotOnRevoked { .. }
            | VersionRefusal::NotRevoked { .. } => Remedy::Refuse,
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
            VersionRefusal::Label(_)
            | VersionRefusal::GitCommit(_)
            | VersionRefusal::MoveTo(_)
            | VersionRefusal::Reason(_) => ExecError::new(ErrorCode::InvalidParam, message),
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
            | VersionRefusal::NamedBySetting { .. } => {
                ExecError::new(ErrorCode::ApplyRefused, message)
            }
            VersionRefusal::NotRevoked { .. }
            | VersionRefusal::NoTenant { .. }
            | VersionRefusal::NotOnRevoked { .. } => ExecError::new(ErrorCode::NotFound, message),
            VersionRefusal::Setting { .. } | VersionRefusal::Stranded { .. } => {
                ExecError::new(ErrorCode::Internal, message)
            }
            VersionRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
        }
    }
}

impl std::fmt::Display for VersionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
                var,
                value,
                stored,
                hint,
            } => write!(
                f,
                "{setting} ({var}) is `{value}`, and no version is stored under that name. \
                 Stored: {}. Set it to one of them, {hint}",
                if stored.is_empty() {
                    "none".to_string()
                } else {
                    stored.join(", ")
                }
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
                if let Some((setting, var)) = setting {
                    write!(
                        f,
                        ". {setting} ({var}) names it: set it to a version that is not \
                         revoked, or unset it to start new tenants on the active version"
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
            VersionRefusal::NamedBySetting {
                named,
                setting,
                var,
            } => write!(
                f,
                "revoke_named_by_setting: {setting} ({var}) names {named}, so new tenants \
                 start on it. Set it to another version, or unset it to start them on the \
                 active version, then revoke"
            ),
            VersionRefusal::NoTenant { space } => write!(
                f,
                "no_tenant: no tenant `{space}` is provisioning, active or paused here, so \
                 there is no pin to move. `ordo tenants list` names them"
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
                 {SERVED_SETTING} ({SERVED_ENV}) is `{}`. Add it: {SERVED_ENV}={},{version}, \
                 or set it to `in-use`",
                list.join(","),
                list.join(",")
            ),
            VersionRefusal::BelowMinimum { named, minimum } => write!(
                f,
                "below_minimum: {named} is older than {MINIMUM_SETTING} ({MINIMUM_ENV}), \
                 {minimum}. Name a version at or above it, or lower {MINIMUM_ENV}"
            ),
            VersionRefusal::Stranded {
                version,
                tenants,
                list,
            } => write!(
                f,
                "{SERVED_SETTING} ({SERVED_ENV}) is `{}` and leaves out {version}, which \
                 {tenants} tenant(s) are pinned to. Add it: {SERVED_ENV}={},{version}, or \
                 move them first (`ordo versions pin <tenant> <version>`)",
                list.join(","),
                list.join(",")
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
    /// `versions.new_tenant`, as this node read it. `None`: the active
    /// version.
    new_tenant: Option<&'a str>,
    /// `versions.served` and `versions.minimum`: what boot installed
    /// ([`policy`]), unless a caller names another.
    policy: &'a ServedPolicy,
}

impl<'a> Versions<'a> {
    pub fn on(pool: &'a Pool) -> Versions<'a> {
        Versions {
            pool,
            new_tenant: None,
            policy: policy(),
        }
    }

    /// The same client, under another served policy.
    pub fn with_policy(self, policy: &'a ServedPolicy) -> Versions<'a> {
        Versions { policy, ..self }
    }

    /// **May a tenant be pinned to `version`** under `versions.served` and
    /// `versions.minimum`? The active version is always served.
    pub async fn check_served(&self, version: &str) -> Result<(), VersionRefusal> {
        let p = self.policy;
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

    /// The stored versions below `versions.minimum`. Empty with none.
    pub async fn below_minimum(&self) -> Result<Vec<String>, VersionRefusal> {
        let Some(min) = &self.policy.minimum else {
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

    /// **Check `versions.served` and `versions.minimum` at boot**, and
    /// answer them resolved: each `#n` named as its version. Refused: a
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
        let resolve = |name: &str, setting: &'static str, var: &'static str| {
            let name = name.to_string();
            let stored = stored.clone();
            async move {
                match self.resolve(&name).await {
                    Ok(v) => Ok(v),
                    Err(VersionRefusal::NotStored { .. })
                    | Err(VersionRefusal::UnknownVersion(_)) => Err(VersionRefusal::Setting {
                        setting,
                        var,
                        value: name,
                        stored,
                        hint: "or unset it",
                    }),
                    Err(e) => Err(e),
                }
            }
        };
        let mut out = ServedPolicy::default();
        if let Some(list) = &p.served {
            let mut resolved = Vec::new();
            for name in list {
                resolved.push(resolve(name, SERVED_SETTING, SERVED_ENV).await?);
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
            out.minimum = Some(resolve(min, MINIMUM_SETTING, MINIMUM_ENV).await?);
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
                    "SELECT \"version\" FROM \"$artifacts\" WHERE \"ordinal\" = {n}"
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
                 ORDER BY \"ordinal\" NULLS LAST, \"version\"",
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
    pub async fn activate(
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
        // Every tenant would land below `versions.minimum`.
        if self.below_minimum().await?.iter().any(|v| v == version) {
            return Err(VersionRefusal::BelowMinimum {
                named: self.named(version).await?,
                minimum: self
                    .named(self.policy.minimum.as_deref().unwrap_or_default())
                    .await?,
            });
        }
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
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, version).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
        }
        registry::record_activation(&tx, version, actor, at).await?;
        repin(&tx, &repin_sql(version, actor, cause, at.0, None)).await?;
        tx.commit().await?;
        Ok(Activated::Switched {
            from,
            to: version.to_string(),
        })
    }

    /// The version a tenant created now starts on: `versions.new_tenant`
    /// when set, the active version otherwise.
    ///
    /// The plan's default is the last version that finished a rollout
    /// (§1.3, D14). Rollouts do not exist yet, so the active version stands
    /// in for it until increment 10.
    pub async fn new_tenant_version(&self) -> Result<Option<String>, VersionRefusal> {
        if let Some(v) = self.new_tenant {
            return Ok(Some(v.to_string()));
        }
        match registry::active_version(self.pool).await {
            Ok(v) => Ok(v),
            // No registry: nothing has ever been applied here.
            Err(e) if e.is_missing_table() => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Refuse a `versions.new_tenant` that names no stored version. Boot
    /// calls this, so a deployment never starts tenants on a version it
    /// cannot load.
    pub async fn check_new_tenant(&self) -> Result<(), VersionRefusal> {
        let Some(value) = self.new_tenant else {
            return Ok(());
        };
        if registry::get(self.pool, value).await?.is_some() {
            return match self.revocation_of(value).await? {
                Some(mark) => Err(self
                    .revoked(mark, Some((NEW_TENANT_SETTING, NEW_TENANT_ENV)))
                    .await),
                None => self.check_served(value).await,
            };
        }
        Err(VersionRefusal::Setting {
            setting: NEW_TENANT_SETTING,
            var: NEW_TENANT_ENV,
            value: value.to_string(),
            stored: registry::stored(self.pool)
                .await?
                .into_iter()
                .map(|(v, _, _)| v)
                .collect(),
            hint: "or unset it to start new tenants on the active version",
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
        let version = match self.new_tenant {
            Some(v) => Some(v.to_string()),
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
            let setting = self
                .new_tenant
                .is_some()
                .then_some((NEW_TENANT_SETTING, NEW_TENANT_ENV));
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
    pub async fn pin(
        &self,
        space: &str,
        name: &str,
        actor: &str,
        at: u64,
    ) -> Result<PinChange, VersionRefusal> {
        let version = self.resolve(name).await?;
        self.check_served(&version).await?;
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        if let Some(mark) = mark_in(&tx, &version).await? {
            tx.rollback().await?;
            return Err(self.revoked(mark, None).await);
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
        let filter = format!("AND \"$space\" = {}", sql_lit(space));
        repin(
            &tx,
            &repin_sql(&version, actor, PinCause::Pin, at, Some(&filter)),
        )
        .await?;
        tx.commit().await?;
        let epoch = self.pin_of(space).await?.map(|p| p.epoch).unwrap_or(0);
        Ok(PinChange {
            seq: 0,
            space: space.to_string(),
            from,
            to: version,
            epoch,
            actor: actor.to_string(),
            cause: PinCause::Pin.as_str().to_string(),
            at,
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
        Ok((
            rows.into_iter()
                .filter_map(|r| {
                    Some((
                        r.first()?.clone()?,
                        r.get(1)?.clone()?,
                        r.get(2)?.clone()?.parse().ok()?,
                    ))
                })
                .collect(),
            head,
        ))
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
    async fn revoked(
        &self,
        mark: Revocation,
        setting: Option<(&'static str, &'static str)>,
    ) -> VersionRefusal {
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
        if let Some(v) = &version {
            self.check_served(v).await?;
        }
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
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
        let below = self.below_minimum().await?;
        ensure_revocations(self.pool).await?;
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
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
    pub async fn on_revoked(&self, limit: usize) -> Result<Vec<(String, String)>, VersionRefusal> {
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
                   (SELECT \"version\" FROM ({}) m) {below}) \
                 ORDER BY \"$space\" LIMIT {limit}",
                mark_sql(None)
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
            var: NEW_TENANT_ENV,
            value: "9.9.9".into(),
            stored: vec!["0.1.0".into()],
            hint: "or unset it",
        };
        let text = e.to_string();
        assert!(text.contains("versions.new_tenant"), "{text}");
        assert!(text.contains("ORDO_VERSIONS_NEW_TENANT"), "{text}");
        assert!(text.contains("9.9.9"), "{text}");
        assert!(text.contains("0.1.0"), "{text}");
        assert_eq!(e.remedy(), Remedy::FixInput);
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
