//! `PgCeremonyStore`: open ceremonies in the control database.
//!
//! `ordo_identity::ceremony` says what a row is. This is the table
//! `plan-auth-federated.md` §3.1 defines, created here first in that exact
//! shape so that outside-provider sign-in extends a table rather than
//! creating one.
//!
//! # One-shot is a `DELETE ... RETURNING`
//!
//! [`CeremonyStore::take`] deletes the row and answers its payload in one
//! statement. Two callers racing on one handle, on one replica or two, cannot
//! both get a row back, so a captured response replayed against another
//! replica finds nothing. Absent, expired and of another phase are the same
//! empty answer.
//!
//! # The sweep
//!
//! An expired row answers nothing whether or not anything has deleted it.
//! The sweep ([`crate::workers::CeremonySweep`], every minute) exists so the
//! table does not grow with every abandoned page load.

use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError, TextRow};
use ordo_identity::ceremony::{CeremonyRow, CeremonyStore, CeremonyStoreError};
use ordo_identity::Timestamp;

use crate::passkeys::{Source, SyncPg};

/// The ceremony table, exactly as `plan-auth-federated.md` §3.1 defines it.
pub const CEREMONY_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$authceremony" (
  "handle"      text   NOT NULL,
  "connection"  text   NOT NULL,
  "phase"       text   NOT NULL,
  "binder"      text   NOT NULL,
  "payload"     text   NOT NULL,
  "minted_at"   bigint NOT NULL,
  "expires_at"  bigint NOT NULL,
  PRIMARY KEY ("handle")
)"#;

/// Create `$authceremony`. Retry-once, as the other `ensure_table`s are.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(CEREMONY_DDL).await.is_err() {
        pool.rows(CEREMONY_DDL).await?;
    }
    Ok(())
}

/// The Postgres ceremony store.
pub struct PgCeremonyStore {
    db: SyncPg,
}

impl PgCeremonyStore {
    /// Dial and stand the store up. Refuses when `$authceremony` does not
    /// exist: an empty table and a missing one are different facts.
    pub fn open(cfg: PoolConfig) -> Result<PgCeremonyStore, CeremonyStoreError> {
        PgCeremonyStore::start(Source::Dial(cfg))
    }

    /// The same, **on a connection somebody else owns**.
    pub fn open_on(pool: Pool) -> Result<PgCeremonyStore, CeremonyStoreError> {
        PgCeremonyStore::start(Source::Shared(pool))
    }

    fn start(source: Source) -> Result<PgCeremonyStore, CeremonyStoreError> {
        let db =
            SyncPg::start(source, "ordo-ceremonies").map_err(|e| CeremonyStoreError(e.into()))?;
        let store = PgCeremonyStore { db };
        store.sql(r#"SELECT 1 FROM "$authceremony" LIMIT 1"#)?;
        Ok(store)
    }

    fn sql(&self, sql: &str) -> Result<Vec<TextRow>, CeremonyStoreError> {
        self.db
            .sql(sql)
            .map_err(|e| CeremonyStoreError(crate::passkeys::scrub_fault("$authceremony", e)))
    }
}

impl CeremonyStore for PgCeremonyStore {
    fn put(&self, row: &CeremonyRow) -> Result<(), CeremonyStoreError> {
        self.sql(&format!(
            r#"INSERT INTO "$authceremony"
                 ("handle", "connection", "phase", "binder", "payload", "minted_at", "expires_at")
               VALUES ({}, {}, {}, {}, {}, {}, {})
               ON CONFLICT ("handle") DO UPDATE SET
                 "connection" = EXCLUDED."connection", "phase" = EXCLUDED."phase",
                 "binder" = EXCLUDED."binder", "payload" = EXCLUDED."payload",
                 "minted_at" = EXCLUDED."minted_at", "expires_at" = EXCLUDED."expires_at""#,
            sql_lit(&row.handle),
            sql_lit(&row.connection),
            sql_lit(&row.phase),
            sql_lit(&row.binder),
            sql_lit(&row.payload),
            row.minted_at.0,
            row.expires_at.0,
        ))?;
        Ok(())
    }

    fn take(
        &self,
        handle: &str,
        connection: &str,
        phase: &str,
        now: Timestamp,
    ) -> Result<Option<String>, CeremonyStoreError> {
        let rows = self.sql(&format!(
            r#"DELETE FROM "$authceremony"
               WHERE "handle" = {} AND "connection" = {} AND "phase" = {} AND "expires_at" > {}
               RETURNING "payload""#,
            sql_lit(handle),
            sql_lit(connection),
            sql_lit(phase),
            now.0,
        ))?;
        Ok(rows.first().and_then(|r| r.first()).and_then(|c| c.clone()))
    }

    fn take_bound(
        &self,
        handle: &str,
        connection: &str,
        phase: &str,
        binder: &str,
        now: Timestamp,
    ) -> Result<Option<String>, CeremonyStoreError> {
        let rows = self.sql(&format!(
            r#"DELETE FROM "$authceremony"
               WHERE "handle" = {} AND "connection" = {} AND "phase" = {} AND "binder" = {}
                 AND "expires_at" > {}
               RETURNING "payload""#,
            sql_lit(handle),
            sql_lit(connection),
            sql_lit(phase),
            sql_lit(binder),
            now.0,
        ))?;
        Ok(rows.first().and_then(|r| r.first()).and_then(|c| c.clone()))
    }

    fn peek(
        &self,
        handle: &str,
        connection: &str,
        phase: &str,
        now: Timestamp,
    ) -> Result<Option<String>, CeremonyStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "payload" FROM "$authceremony"
               WHERE "handle" = {} AND "connection" = {} AND "phase" = {} AND "expires_at" > {}"#,
            sql_lit(handle),
            sql_lit(connection),
            sql_lit(phase),
            now.0,
        ))?;
        Ok(rows.first().and_then(|r| r.first()).and_then(|c| c.clone()))
    }

    fn sweep(&self, now: Timestamp) -> Result<u64, CeremonyStoreError> {
        let rows = self.sql(&format!(
            r#"WITH gone AS (
                 DELETE FROM "$authceremony" WHERE "expires_at" <= {} RETURNING 1
               ) SELECT count(*) FROM gone"#,
            now.0
        ))?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|c| c.parse().ok())
            .unwrap_or(0))
    }
}
