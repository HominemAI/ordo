//! The Postgres delegation registry.
//!
//! [`ordo_identity::delegation`] states the contract, the boundary, and the
//! one place that boundary deliberately differs from the grant registry's
//! (this one **is** consulted to admit a request). What follows is only what
//! building it decided.
//!
//! # `"$space"`, not `"space"`
//!
//! Same reason `crate::grants` gives: a delegation is per-space by SH-20's
//! pin, and giving the column the name the write protocol's `SpaceAudit`
//! already polices means a future statement that forgets to constrain it is
//! caught by the mechanism that catches every other one.
//!
//! # Why the uniqueness is enforced in the statement rather than by an index
//!
//! The rule is *at most one **live** delegation per `(space, delegator,
//! agent)`*, and "live" is three facts: not revoked, not expired, and the
//! clock. A partial unique index can express `WHERE revoked_at IS NULL`; it
//! cannot express "and `expires` is still in the future", because the
//! predicate would have to be evaluated against `now()` and a unique index
//! must be immutable. An index over the un-revoked rows alone would be
//! *wrong in the ordinary direction*: a delegation that lapsed a year ago
//! and was never explicitly withdrawn would block the delegator from ever
//! consenting again.
//!
//! So the insert is a conditional one — `INSERT … SELECT … WHERE NOT EXISTS
//! (a live row)` — and it returns the id it wrote, so "nothing inserted" is
//! observable rather than silent. The two statements run in one, which is
//! what keeps a concurrent double-consent from producing two live rows: the
//! `NOT EXISTS` is evaluated inside the same statement that inserts.
//!
//! The read-committed caveat, stated: two consents arriving in truly
//! overlapping transactions can both see no live row and both insert. That
//! window is bounded by a human clicking "allow" twice within one
//! statement's execution, and its consequence is two live delegations
//! rather than an escalation — the audit join becomes ambiguous, which the
//! delegator can repair by revoking one. Closing it properly wants
//! SERIALIZABLE on the consent path, which is a decision for the consent
//! ceremony rather than for this table.
//!
//! # Nothing here is sealed
//!
//! A delegation record is metadata about consent — who let whom act for
//! them, when, until when — queried by space, by principal and by id, all of
//! which sealing would prevent. It holds no row content and no payload.

use ordo_identity::delegation::{
    DelegationError, DelegationRecord, DelegationRegistry, NewDelegation,
};
use ordo_identity::{DelegationId, PrincipalId, SpaceId, Timestamp};

use ordo_exec::Pool;

/// A platform table, created at boot beside the others.
pub const DELEGATION_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS "$delegation" (
    "$space"              text   NOT NULL,
    "delegation_id"       text   NOT NULL,
    "delegator"           text   NOT NULL,
    "agent"               text   NOT NULL,
    "granted_at"          bigint NOT NULL,
    "expires"             bigint NOT NULL,
    "revoked_at"          bigint,
    "consent_correlation" text   NOT NULL DEFAULT '',
    PRIMARY KEY ("$space", "delegation_id")
)"#;

/// The door asks *"is this delegation live"* on every delegated call, keyed
/// on `(space, delegation_id)` — which the primary key already serves. This
/// index is for the other two questions: `live_in` (the `callersOf` column,
/// keyed on the space) and the uniqueness probe (keyed on the space, the
/// delegator and the agent).
pub const DELEGATION_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS "$delegation$pair"
    ON "$delegation" ("$space", "delegator", "agent")"#;

pub struct PgDelegationRegistry {
    pool: Pool,
}

impl PgDelegationRegistry {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn ensure_table(&self) -> Result<(), DelegationError> {
        for ddl in [DELEGATION_DDL, DELEGATION_INDEX_DDL] {
            self.pool.rows(ddl).await.map_err(|e| {
                DelegationError::Unavailable(crate::auth::store_failure::IntoFault::fault(e))
            })?;
        }
        Ok(())
    }

    async fn query(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, DelegationError> {
        self.pool.rows(sql).await.map_err(|e| {
            DelegationError::Unavailable(crate::auth::store_failure::IntoFault::fault(e))
        })
    }
}

fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

const COLUMNS: &str = "\"delegation_id\", \"$space\", \"delegator\", \"agent\", \
                       \"granted_at\", \"expires\", \"revoked_at\", \"consent_correlation\"";

fn row_to_record(row: &[Option<String>]) -> Option<DelegationRecord> {
    let get = |i: usize| row.get(i).cloned().flatten();
    Some(DelegationRecord {
        id: DelegationId(get(0)?),
        space: SpaceId(get(1)?),
        delegator: PrincipalId(get(2)?),
        agent: PrincipalId(get(3)?),
        granted_at: Timestamp(get(4)?.parse().ok()?),
        expires: Timestamp(get(5)?.parse().ok()?),
        revoked_at: get(6).and_then(|s| s.parse().ok()).map(Timestamp),
        consent_correlation: get(7).unwrap_or_default(),
    })
}

/// The `WHERE` clause for *live at `now`*, written once so the door's
/// question, the audit surface's question and the uniqueness probe cannot
/// drift apart.
fn live_at(now: Timestamp) -> String {
    format!("\"revoked_at\" IS NULL AND \"expires\" > {}", now.0)
}

#[async_trait::async_trait]
impl DelegationRegistry for PgDelegationRegistry {
    async fn record(&self, d: &NewDelegation) -> Result<(), DelegationError> {
        // Conditional on there being no live delegation for this triple —
        // see the module doc. The `RETURNING` is what makes the refusal
        // observable: an unconditional insert that quietly did nothing would
        // hand back a delegation id nobody can revoke.
        let sql = format!(
            "INSERT INTO \"$delegation\" ({COLUMNS}) \
             SELECT {}, {}, {}, {}, {}, {}, NULL, {} \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM \"$delegation\" \
                 WHERE \"$space\" = {} AND \"delegator\" = {} AND \"agent\" = {} AND {} \
             ) \
             ON CONFLICT (\"$space\", \"delegation_id\") DO NOTHING \
             RETURNING \"delegation_id\"",
            lit(&d.id.0),
            lit(&d.space.0),
            lit(&d.delegator.0),
            lit(&d.agent.0),
            d.granted_at.0,
            d.expires.0,
            lit(&d.consent_correlation),
            lit(&d.space.0),
            lit(&d.delegator.0),
            lit(&d.agent.0),
            live_at(d.granted_at),
        );
        if !self.query(&sql).await?.is_empty() {
            return Ok(());
        }
        // Nothing was written. Two causes, and the caller needs to know
        // which: a live one already covers the pair, or this exact id was
        // already recorded (a retried consent, which is not an error).
        let existing = format!(
            "SELECT \"delegation_id\" FROM \"$delegation\" \
             WHERE \"$space\" = {} AND \"delegator\" = {} AND \"agent\" = {} AND {} \
             ORDER BY \"granted_at\" DESC LIMIT 1",
            lit(&d.space.0),
            lit(&d.delegator.0),
            lit(&d.agent.0),
            live_at(d.granted_at),
        );
        match self
            .query(&existing)
            .await?
            .first()
            .and_then(|r| r[0].clone())
        {
            // Idempotent replay of the same consent: the row is already
            // there and says exactly what this call says.
            Some(id) if id == d.id.0 => Ok(()),
            Some(id) => Err(DelegationError::AlreadyLive {
                existing: DelegationId(id),
            }),
            // No live row and still nothing inserted means the id collided
            // with a dead one. Refusing is right: a revoked delegation's id
            // must never come back to life.
            None => Err(DelegationError::AlreadyLive {
                existing: d.id.clone(),
            }),
        }
    }

    async fn revoke(
        &self,
        space: &SpaceId,
        id: &DelegationId,
        at: Timestamp,
    ) -> Result<(), DelegationError> {
        // `WHERE revoked_at IS NULL` is what makes this idempotent in the
        // way that matters: a second withdrawal must not overwrite the first
        // one's timestamp, because that timestamp is the audit answer to
        // "when did this stop".
        let sql = format!(
            "UPDATE \"$delegation\" SET \"revoked_at\" = {} \
             WHERE \"$space\" = {} AND \"delegation_id\" = {} AND \"revoked_at\" IS NULL",
            at.0,
            lit(&space.0),
            lit(&id.0),
        );
        self.query(&sql).await.map(|_| ())
    }

    async fn is_live(
        &self,
        space: &SpaceId,
        id: &DelegationId,
        now: Timestamp,
    ) -> Result<bool, DelegationError> {
        let sql = format!(
            "SELECT 1 FROM \"$delegation\" \
             WHERE \"$space\" = {} AND \"delegation_id\" = {} AND {} LIMIT 1",
            lit(&space.0),
            lit(&id.0),
            live_at(now),
        );
        Ok(!self.query(&sql).await?.is_empty())
    }

    async fn live_in(
        &self,
        space: &SpaceId,
        now: Timestamp,
    ) -> Result<Vec<DelegationRecord>, DelegationError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM \"$delegation\" \
             WHERE \"$space\" = {} AND {} \
             ORDER BY \"agent\", \"delegator\", \"delegation_id\"",
            lit(&space.0),
            live_at(now),
        );
        Ok(self
            .query(&sql)
            .await?
            .iter()
            .filter_map(|r| row_to_record(r))
            .collect())
    }

    async fn granted_by(
        &self,
        space: &SpaceId,
        delegator: &PrincipalId,
    ) -> Result<Vec<DelegationRecord>, DelegationError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM \"$delegation\" \
             WHERE \"$space\" = {} AND \"delegator\" = {} \
             ORDER BY \"granted_at\" DESC, \"delegation_id\"",
            lit(&space.0),
            lit(&delegator.0),
        );
        Ok(self
            .query(&sql)
            .await?
            .iter()
            .filter_map(|r| row_to_record(r))
            .collect())
    }
}
