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
}

impl PinCause {
    pub fn as_str(self) -> &'static str {
        match self {
            PinCause::Activate => "activate",
            PinCause::Rollback => "rollback",
            PinCause::NewTenant => "new_tenant",
            PinCause::Backfill => "backfill",
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
    },
    /// The database. Its class says whether a retry can help.
    Store(PoolError),
}

impl VersionRefusal {
    pub fn remedy(&self) -> Remedy {
        match self {
            VersionRefusal::UnknownVersion(_) | VersionRefusal::Setting { .. } => Remedy::FixInput,
            VersionRefusal::Collision { .. } => Remedy::Refuse,
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
            VersionRefusal::UnknownVersion(_) => ExecError::new(ErrorCode::NotFound, message),
            VersionRefusal::Collision { .. } => ExecError::new(ErrorCode::Duplicate, message),
            VersionRefusal::Setting { .. } => ExecError::new(ErrorCode::Internal, message),
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
            } => write!(
                f,
                "{setting} ({var}) is `{value}`, and no version is stored under that name. \
                 Stored: {}. Set it to one of them, or unset it to start new tenants on the \
                 active version",
                if stored.is_empty() {
                    "none".to_string()
                } else {
                    stored.join(", ")
                }
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
}

impl<'a> Versions<'a> {
    pub fn on(pool: &'a Pool) -> Versions<'a> {
        Versions {
            pool,
            new_tenant: None,
        }
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
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        registry::record_activation(&tx, version, actor, at).await?;
        tx.rows(&repin_sql(version, actor, cause, at.0, None))
            .await?;
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
            return Ok(());
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
        let filter = format!(
            "AND \"$space\" = {} AND \"version\" IS NULL",
            sql_lit(space)
        );
        tx.rows(&repin_sql(
            &version,
            actor,
            PinCause::NewTenant,
            at,
            Some(&filter),
        ))
        .await?;
        tx.commit().await?;
        Ok(Some(version))
    }

    /// Pin every tenant that has no pin to the active version, cause
    /// `backfill`. The tenants pinned. Idempotent: a second call finds none.
    pub async fn backfill(&self, actor: &str, at: u64) -> Result<usize, VersionRefusal> {
        let tx = self.pool.begin().await?;
        tx.rows(&pin_lock_sql()).await?;
        let Some(active) = active_in(&tx).await? else {
            return Ok(0);
        };
        let moved = tx
            .rows(&repin_sql(
                &active,
                actor,
                PinCause::Backfill,
                at,
                Some("AND \"version\" IS NULL"),
            ))
            .await?
            .len();
        tx.commit().await?;
        Ok(moved)
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
        Ok(rows
            .into_iter()
            .filter_map(|r| {
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
            })
            .collect())
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

/// One statement that moves every tenant in a pinned state, and not yet on
/// `to`, onto `to`: the pin, its epoch, and a `$pins` row each. `filter`
/// narrows the tenants (`AND ...`). Returns one row per tenant moved.
///
/// One statement, so the rows it reads are locked (`FOR UPDATE`) for the
/// update that follows, and the history it writes is the change it made.
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
    fn a_bad_setting_names_the_setting_and_the_value() {
        let e = VersionRefusal::Setting {
            setting: NEW_TENANT_SETTING,
            var: NEW_TENANT_ENV,
            value: "9.9.9".into(),
            stored: vec!["0.1.0".into()],
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
    fn a_moved_refusal_reads_as_the_registry_wrote_it() {
        let v = VersionRefusal::from(RegistryRefusal::UnknownVersion("v9".into()));
        assert_eq!(
            v.to_string(),
            RegistryRefusal::UnknownVersion("v9".into()).to_string()
        );
    }
}
