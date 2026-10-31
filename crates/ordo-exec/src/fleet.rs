//! The **fleet view** (18d R-S2): one row per target, in the control
//! database, saying which schema version that target is known to be at and
//! when that was last confirmed.
//!
//! # It is a cache, and the distinction is the whole design
//!
//! R-S2 is explicit: *"The fleet view is a cache of the targets' own rows
//! and is never the authority; a target's `$migrations` is."* Every read
//! here is therefore a **report about the past**, and the type says so —
//! [`Confirmed`] carries the moment it was taken, and there is no accessor
//! that returns a version without it.
//!
//! That is not decoration. C-M1 (§9.7) forbids a second cache of the
//! attachment and requires that *"§9.2's fleet view and §9.6's operator
//! surface must read through the resolver or be marked plainly as stale
//! reports"*, because a tenant move has to reach one cache rather than
//! five. This is the marked-stale-report half of that permission, and it
//! keeps that permission only as long as nothing routes on it. **Nothing
//! may resolve a pool, gate a request or refuse an apply from this table.**
//! Those decisions read `spacedb::recorded_version` against the target
//! itself.
//!
//! # Why it exists at all, then
//!
//! Because the question an operator asks is about the fleet and the answer
//! is spread across N databases, some of which are down. Asking every
//! target costs N dials and fails when one is unreachable, which is the
//! worst possible behaviour for a diagnostic: the surface that tells you a
//! database is down would itself be down. A row written at the moment an
//! apply confirmed a version answers *"what did we last know, and when"*
//! without dialling anything, and an entry whose `confirmed_at` is old is
//! itself the signal.
//!
//! # The key is the label
//!
//! `default`, or `host:port/db` — [`crate::route::Target::label`], which is
//! what the operator surface prints and what `TargetApply` already reports.
//! Deliberately **not** `DbAttachment::identity`: that string carries the
//! credential label and the custody, so a target re-attached under a
//! rotated credential would become a second row describing the same
//! database. The label names the place, which is what this table is about.

use crate::{sql_lit, Pool, PoolError};

pub const FLEET_TABLE: &str = "$fleet";

/// No credential column, and no space column. A row is about a **database**,
/// not about a tenant: one target may hold one tenant (18d D-D) but the
/// version is a property of the database either way, and putting a space id
/// here would make the table look like a second `$spacedb`.
pub const FLEET_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$fleet\" ( \
  \"label\" text NOT NULL PRIMARY KEY, \
  \"version\" text NOT NULL, \
  \"confirmed_at\" bigint NOT NULL \
)";

/// What the fleet view knows about one target, and when it knew it.
///
/// There is no `version()` without `confirmed_at`, on purpose: a caller
/// that could read the version alone would be a caller treating a cache as
/// an authority, which is the one thing R-S2 forbids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    /// `default`, or `host:port/db`.
    pub label: String,
    /// The version the target reported. `(unknown)` is a pre-R-S1 database
    /// that has a schema and no record of which one — stored as the same
    /// token `spacedb::TargetVersion` renders, so the two surfaces cannot
    /// disagree about how to spell "we do not know".
    pub version: String,
    /// Seconds since the epoch, at the moment a target actually answered.
    /// **Not** the moment this row was read.
    pub confirmed_at: i64,
}

/// The token stored for a database whose version is unrecorded.
pub const UNKNOWN: &str = "(unknown)";

pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    // Retried once for `spacedb::ensure_table`'s reason: `CREATE TABLE IF
    // NOT EXISTS` is not race-free between two booting replicas.
    if pool.rows(FLEET_DDL).await.is_err() {
        pool.rows(FLEET_DDL).await?;
    }
    Ok(())
}

/// Record what a target just reported.
///
/// **Last writer wins, and that is correct here.** Two replicas applying
/// and confirming the same target write the same version; a replica that
/// confirmed an older version later is reporting a real observation, and a
/// cache that refused it would hide a target that had been rolled back.
/// The authority is the target's own `$migrations` either way.
pub async fn confirm(pool: &Pool, label: &str, version: &str, at: i64) -> Result<(), PoolError> {
    pool.rows(&format!(
        "INSERT INTO \"{FLEET_TABLE}\" (\"label\", \"version\", \"confirmed_at\") \
         VALUES ({}, {}, {at}) \
         ON CONFLICT (\"label\") DO UPDATE SET \
           \"version\" = EXCLUDED.\"version\", \
           \"confirmed_at\" = EXCLUDED.\"confirmed_at\"",
        sql_lit(label),
        sql_lit(version),
    ))
    .await?;
    Ok(())
}

/// Every target the fleet view has heard from, in label order.
///
/// A target that has **never** been confirmed is absent rather than
/// present-with-a-null, and absence is the honest answer: this table says
/// what was observed, and nothing was. A caller listing the fleet reconciles
/// against `spacedb::all` to see which attachments have no report yet.
pub async fn all(pool: &Pool) -> Result<Vec<Confirmed>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"label\", \"version\", \"confirmed_at\" \
             FROM \"{FLEET_TABLE}\" ORDER BY \"label\""
        ))
        .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let cell = |i: usize| row.get(i).and_then(|c| c.as_deref()).unwrap_or_default();
            Confirmed {
                label: cell(0).to_string(),
                version: cell(1).to_string(),
                confirmed_at: cell(2).parse().unwrap_or(0),
            }
        })
        .collect())
}

/// One target's last report, or `None` if it has never made one.
pub async fn lookup(pool: &Pool, label: &str) -> Result<Option<Confirmed>, PoolError> {
    Ok(all(pool).await?.into_iter().find(|c| c.label == label))
}

/// Forget a target's report.
///
/// For a target that is **detached**, so the fleet view does not keep
/// reporting a database this deployment no longer serves from. Not called
/// by anything yet: detaching a tenant is 18e, and this exists so that
/// increment has one place to call rather than a table to discover.
pub async fn forget(pool: &Pool, label: &str) -> Result<(), PoolError> {
    pool.rows(&format!(
        "DELETE FROM \"{FLEET_TABLE}\" WHERE \"label\" = {}",
        sql_lit(label)
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DDL holds no credential and no space. Asserted by reading the
    /// string, the way `spacedb`'s own test does, so a column added later
    /// has to face this test rather than a reviewer.
    #[test]
    fn the_fleet_table_holds_no_secret_and_no_tenant() {
        let ddl = FLEET_DDL.to_ascii_lowercase();
        for forbidden in ["credential", "password", "secret", "$space", "user"] {
            assert!(
                !ddl.contains(forbidden),
                "`{forbidden}` is in the fleet table: {FLEET_DDL}"
            );
        }
    }
}
