//! `PgSessionRevocations`: the deny list the door consults, and the one
//! place in this work where a cost had to be argued rather than measured.
//!
//! `ordo_identity::session` states the contract and why the read cannot
//! fail. What follows is what the Postgres one decided.
//!
//! # What the door pays per request
//!
//! **One `RwLock` read acquisition and one `BTreeSet` lookup: a handful of
//! string comparisons over the revoked set, no allocation, no syscall, no
//! I/O, no lock contention in the ordinary case.** For a deployment with a
//! thousand logged-out-but-not-yet-expired sessions that is ten comparisons
//! of a 32-character string. It is the same order as the `HashMap` lookup
//! the access engine already does per check, and it is not measurable beside
//! the HMAC verification that runs immediately before it on the same
//! request.
//!
//! The alternative was a query per request. That is one round trip on the
//! request path for every call the deployment serves, which is the cost
//! `IdentityAccess::check` exists to avoid: the whole door is built to
//! decide from the token plus in-process state, and a session table would
//! have been the first thing to break that.
//!
//! # What the memory costs
//!
//! One entry per session that was logged out and has not yet reached its
//! ceiling. Bounded by **logouts**, not by logins, and swept: 32 bytes of id
//! plus the map node, so a hundred thousand outstanding revocations is a few
//! megabytes, and a hundred thousand logouts inside one session ceiling is
//! not a shape a deployment has.
//!
//! # The staleness, named
//!
//! The list is loaded when the store is built and written through on every
//! logout, so it is exact for every logout **this process** served. A second
//! replica's logout is not seen until this one's next
//! [`PgSessionRevocations::refresh`], which the runtime runs on a cadence.
//! That window is the honest cost of keeping the door's answer in memory,
//! and it is the same trade `PgKeyStore` names for custody. Two things bound
//! it: the cadence, and the token's own one-hour expiry, past which the
//! logged-out holder has to refresh, and **refresh reads the list on the
//! way through**, so a stale door admits a revoked token for at most the
//! shorter of the cadence and the remaining token life, after which the
//! session cannot be continued at all.
//!
//! Single-replica deployments, which is every deployment this binary
//! currently produces, see a logout immediately.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use ordo_identity::principal::PrincipalId;
use ordo_identity::session::{RevocationStoreError, SessionId, SessionRevocations};
use ordo_identity::Timestamp;

use ordo_exec::{sql_lit, Pool, PoolError};

/// The deny list.
///
/// `until` is the session's ceiling: past it every token naming the session
/// is refused by its own expiry, so the row proves nothing and is swept.
/// Storing it is what makes the table self-limiting instead of a log that
/// grows for the life of the deployment.
///
/// **No principal column, and no client column.** The row's only job is to
/// answer "has this id been ended". Attaching who it belonged to would make
/// the deny list a record of who logs out and when, which is a behavioural
/// log nobody asked for, retained past the session it describes.
pub const SESSION_REVOCATION_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$sessionrevocation" (
  "session_id" text   NOT NULL,
  "revoked_at" bigint NOT NULL,
  "until"      bigint NOT NULL,
  PRIMARY KEY ("session_id")
)"#;

/// The sweep reads by `until` and nothing else.
pub const SESSION_REVOCATION_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS "$sessionrevocation$until"
    ON "$sessionrevocation" ("until")"#;

/// "Sign out everywhere": one row per principal whose sessions were ended,
/// holding the instant before which every session that started is ended.
/// `until` is when the row proves nothing: the epoch plus the session
/// ceiling, past which every older token is dead by its own expiry.
///
/// Like the session list this holds **no behaviour**: a principal id and two
/// instants, kept only until they cannot matter.
pub const PRINCIPAL_EPOCH_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$principalepoch" (
  "principal"  text   NOT NULL,
  "not_before" bigint NOT NULL,
  "until"      bigint NOT NULL,
  PRIMARY KEY ("principal")
)"#;

/// Create the tables, at boot beside the other platform tables.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [
        SESSION_REVOCATION_DDL,
        SESSION_REVOCATION_INDEX_DDL,
        PRINCIPAL_EPOCH_DDL,
    ] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// Postgres-backed, memory-answered.
pub struct PgSessionRevocations {
    pool: Pool,
    /// What the door reads. Loaded by [`PgSessionRevocations::open`],
    /// written through by `revoke`, re-read by `refresh`.
    live: RwLock<BTreeSet<SessionId>>,
    /// principal -> the instant before which every session that started is
    /// ended. Loaded with the list above, written through by `end_all`.
    epochs: RwLock<BTreeMap<PrincipalId, u64>>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl PgSessionRevocations {
    /// Load the outstanding revocations and stand the store up.
    ///
    /// **Fails rather than starting empty.** An empty deny list is a
    /// deployment where nobody has logged out, and a deployment that could
    /// not read the table is one where it does not know. Those must not be
    /// the same thing: the second one silently honours tokens somebody
    /// revoked. Failing here makes it a refused boot, which an operator is
    /// standing in front of.
    pub async fn open(pool: Pool) -> Result<PgSessionRevocations, RevocationStoreError> {
        PgSessionRevocations::open_with_clock(pool, Box::new(crate::runtime::now_secs)).await
    }

    pub async fn open_with_clock(
        pool: Pool,
        now: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<PgSessionRevocations, RevocationStoreError> {
        let store = PgSessionRevocations {
            pool,
            live: RwLock::new(BTreeSet::new()),
            epochs: RwLock::new(BTreeMap::new()),
            now,
        };
        store.refresh().await?;
        Ok(store)
    }

    /// Re-read the outstanding revocations, and drop the expired rows while
    /// there.
    ///
    /// Runs on a cadence from the runtime's worker set rather than on the
    /// request path. It is the answer to the staleness the module doc names
    /// and it is the sweep at the same time, because the two read the same
    /// table and the sweep is what keeps the read cheap.
    ///
    /// **Delete first, then load.** The other order would load a row this
    /// call is about to delete and hold it in memory until the next
    /// refresh, which is harmless but is a deny list that is a cadence
    /// behind its own table for no reason.
    pub async fn refresh(&self) -> Result<(), RevocationStoreError> {
        let now = (self.now)();
        self.pool
            .rows(&format!(
                r#"DELETE FROM "$sessionrevocation" WHERE "until" <= {now}"#
            ))
            .await
            .map_err(store_error)?;
        let rows = self
            .pool
            .rows(r#"SELECT "session_id" FROM "$sessionrevocation""#)
            .await
            .map_err(store_error)?;
        let mut next = BTreeSet::new();
        for r in rows {
            if let Some(Some(id)) = r.first() {
                next.insert(SessionId(id.clone()));
            }
        }
        *self.live.write().expect("revocations") = next;

        // The epochs, the same way: drop what cannot matter, then load.
        self.pool
            .rows(&format!(
                r#"DELETE FROM "$principalepoch" WHERE "until" <= {now}"#
            ))
            .await
            .map_err(store_error)?;
        let rows = self
            .pool
            .rows(r#"SELECT "principal", "not_before" FROM "$principalepoch""#)
            .await
            .map_err(store_error)?;
        let mut epochs = BTreeMap::new();
        for r in rows {
            if let (Some(Some(p)), Some(Some(nb))) = (r.first(), r.get(1)) {
                if let Ok(nb) = nb.parse::<u64>() {
                    epochs.insert(PrincipalId(p.clone()), nb);
                }
            }
        }
        *self.epochs.write().expect("revocations") = epochs;
        Ok(())
    }

    /// How many sessions are on the list. Operator and test observation.
    pub fn len(&self) -> usize {
        self.live.read().expect("revocations").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn store_error(e: PoolError) -> RevocationStoreError {
    if e.is_busy() {
        return RevocationStoreError(crate::passkeys::scrub_fault("$sessionrevocation", e));
    }
    let text = e.to_string();
    if e.is_missing_table() && text.contains("$sessionrevocation") {
        return RevocationStoreError(
            "`$sessionrevocation` does not exist: the deployment has not created its \
             platform tables"
                .into(),
        );
    }
    RevocationStoreError(text.into())
}

impl SessionRevocations for PgSessionRevocations {
    /// The per-request check. See the module doc for what it costs.
    fn is_revoked(&self, id: &SessionId) -> bool {
        self.live.read().expect("revocations").contains(id)
    }

    /// Write the row, then the cache.
    ///
    /// **Postgres first**, the same rule `PgKeyStore::put_smk` states: a
    /// cache written before a failed insert would report a session ended
    /// that a restart brings back, and a logout that a restart undoes is a
    /// logout that did not happen. The caller is told the write failed and
    /// can say so.
    ///
    /// Blocking inside a synchronous trait method, on the logout path only.
    /// The pool is async and this is not, so the write goes through a
    /// blocking bridge; it is one round trip on a request somebody is
    /// already waiting on, and it is not the door's path.
    fn revoke(&self, id: &SessionId, until: Timestamp) -> Result<(), RevocationStoreError> {
        let at = (self.now)();
        let sql = format!(
            r#"INSERT INTO "$sessionrevocation" ("session_id", "revoked_at", "until")
               VALUES ({}, {at}, {})
               ON CONFLICT ("session_id") DO NOTHING"#,
            sql_lit(&id.0),
            until.0
        );
        // `DO NOTHING` rather than an update: a second logout of the same
        // session must not move `revoked_at`, because that timestamp is the
        // audit answer to "when did this stop". Same rule the delegation
        // registry's `revoke` states.
        let pool = self.pool.clone();
        block_on(async move { pool.rows(&sql).await })
            .map_err(store_error)
            .map(|_| ())?;
        self.live.write().expect("revocations").insert(id.clone());
        Ok(())
    }

    fn principal_ended(&self, principal: &PrincipalId, started_at: Timestamp) -> bool {
        self.epochs
            .read()
            .expect("revocations")
            .get(principal)
            .is_some_and(|not_before| started_at.0 < *not_before)
    }

    /// Write the row, then the cache, for `revoke`'s reason: a cache written
    /// before a failed insert would end sessions a restart brings back.
    /// Monotonic in the database as well as in memory, so two replicas
    /// ending the same person's sessions cannot move the epoch back.
    fn end_all(
        &self,
        principal: &PrincipalId,
        at: Timestamp,
        until: Timestamp,
    ) -> Result<(), RevocationStoreError> {
        let not_before = at.0.saturating_add(1);
        let sql = format!(
            r#"INSERT INTO "$principalepoch" ("principal", "not_before", "until")
               VALUES ({}, {not_before}, {})
               ON CONFLICT ("principal") DO UPDATE SET
                 "not_before" = GREATEST("$principalepoch"."not_before", EXCLUDED."not_before"),
                 "until" = GREATEST("$principalepoch"."until", EXCLUDED."until")"#,
            sql_lit(&principal.0),
            until.0
        );
        let pool = self.pool.clone();
        block_on(async move { pool.rows(&sql).await })
            .map_err(store_error)
            .map(|_| ())?;
        let mut epochs = self.epochs.write().expect("revocations");
        let e = epochs.entry(principal.clone()).or_insert(0);
        *e = (*e).max(not_before);
        Ok(())
    }
}

/// Run one future to completion from a synchronous method that may or may
/// not be inside a runtime.
///
/// `Handle::block_on` panics on a current-thread runtime and
/// `block_in_place` is unavailable on one, so neither is safe to reach for
/// from a trait method that any caller might invoke. A thread of its own
/// with a runtime of its own always works and costs one thread spawn on the
/// logout path, which is a path that is already paying for a database round
/// trip.
pub(crate) fn block_on<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    // `in_place`: the statement may ride a connection whose driver lives on
    // the door's runtime, so the calling worker must not stop it.
    ordo_exec::bridge::in_place(|| {
        std::thread::scope(|s| {
            s.spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("a current-thread runtime for one statement")
                    .block_on(fut)
            })
            .join()
            .expect("the revocation write thread panicked")
        })
    })
}
