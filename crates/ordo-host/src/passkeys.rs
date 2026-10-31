//! `PgPasskeyStore`: the [`PasskeyStore`] a deployment boots on.
//!
//! `ordo_identity::passkey` states what a passkey row is and why the sign
//! counter is written before a subject is returned. What follows is only what
//! building the Postgres one decided.
//!
//! # The same shape as `$credential`
//!
//! A table of its own in the control database (`P1`, *"service owns the
//! credentialing"*), a thread of its own with a current-thread runtime, and
//! synchronous trait methods that hand that thread a statement and block on
//! the answer. [`SyncPg`] is that bridge, written once here and used by
//! [`crate::ceremonies::PgCeremonyStore`] too. It is
//! [`crate::credentials::PgCredentialStore`]'s pattern with the statement
//! source made generic, and it opens on the same shared single-connection
//! pool (`open_on`).
//!
//! # No principal column
//!
//! The link from a passkey to a person is `$authenticator`
//! (`principal_directory.rs`), and it stays in one place. A row here with no
//! link row is a credential nobody can sign in with, which is what a
//! revocation leaves behind until recovery deletes it.
//!
//! # A restart must not quietly start over
//!
//! `open` refuses when `$passkey` is not there, rather than reporting an
//! empty table. "You have not provisioned this deployment" and "nobody has
//! registered" are the same words for very different situations, and the
//! second locks every person out while looking healthy.
//!
//! # Nothing here is a secret
//!
//! A public key, a counter, a handle the authenticator already holds and two
//! flags. The table is safe in a backup in the only sense that matters: what
//! leaks verifies nothing.

use std::sync::mpsc::SyncSender;

use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError, TextRow};
use ordo_identity::passkey::{
    KeyRole, PasskeyCredential, PasskeyStore, PasskeyStoreError, StoredPasskey,
};
use ordo_identity::webauthn::{b64url_decode, b64url_encode, CoseKey};
use ordo_identity::Timestamp;

/// The passkey table. `authenticator` is the same string
/// `$authenticator.authenticator` links, `passkey:<base64url credential id>`,
/// so the two tables join without a translation.
pub const PASSKEY_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$passkey" (
  "authenticator"   text    NOT NULL,
  "algorithm"       bigint  NOT NULL,
  "public_key"      text    NOT NULL,
  "sign_count"      bigint  NOT NULL,
  "user_handle"     text    NOT NULL,
  "backup_eligible" boolean NOT NULL,
  "backed_up"       boolean NOT NULL,
  "registered_at"   bigint  NOT NULL,
  "last_used_at"    bigint  NOT NULL DEFAULT 0,
  "role"            text    NOT NULL DEFAULT 'passkey',
  PRIMARY KEY ("authenticator")
)"#;

/// A table made before a credential had a role gains the column, with every
/// existing row a first-factor passkey, which is what it was.
pub const PASSKEY_ROLE_DDL: &str =
    r#"ALTER TABLE "$passkey" ADD COLUMN IF NOT EXISTS "role" text NOT NULL DEFAULT 'passkey'"#;

/// Create `$passkey`. Retry-once for the reason
/// [`crate::credentials::ensure_table`] is: `CREATE TABLE IF NOT EXISTS` is
/// not race-free between two processes.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(PASSKEY_DDL).await.is_err() {
        pool.rows(PASSKEY_DDL).await?;
    }
    if pool.rows(PASSKEY_ROLE_DDL).await.is_err() {
        pool.rows(PASSKEY_ROLE_DDL).await?;
    }
    Ok(())
}

/// One statement handed to a store's own thread, and where its answer goes.
struct Job {
    sql: String,
    reply: SyncSender<Result<Vec<TextRow>, PoolError>>,
}

/// Where the store's connection comes from: one it dials, or one the
/// deployment already holds and its stores share.
pub(crate) enum Source {
    Dial(PoolConfig),
    Shared(Pool),
}

/// A thread with its own current-thread runtime, and a channel of
/// statements to run on the pool it holds. Synchronous callers hand it one
/// statement and block on the reply.
///
/// **Joining while still holding the sender is a deadlock**, so [`Drop`]
/// closes the channel first. A dropped bridge has released its connection by
/// the time the drop returns, which is what a restart test needs.
pub(crate) struct SyncPg {
    jobs: Option<tokio::sync::mpsc::UnboundedSender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SyncPg {
    pub(crate) fn start(source: Source, name: &str) -> Result<SyncPg, String> {
        let (jobs, mut rx) = tokio::sync::mpsc::unbounded_channel::<Job>();
        let (ready, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        let label = name.to_string();
        let thread = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready.send(Err(e.to_string()));
                        return;
                    }
                };
                rt.block_on(async move {
                    let pool = match source {
                        Source::Shared(p) => p.labelled(&label),
                        Source::Dial(cfg) => {
                            match Pool::connect(PoolConfig { size: 1, ..cfg }).await {
                                Ok(p) => p,
                                Err(e) => {
                                    let _ = ready.send(Err(e.to_string()));
                                    return;
                                }
                            }
                        }
                    };
                    let _ = ready.send(Ok(()));
                    while let Some(job) = rx.recv().await {
                        let out = pool.rows(&job.sql).await;
                        let _ = job.reply.send(out);
                    }
                });
            })
            .map_err(|e| format!("{name} thread: {e}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(format!("{name} thread died")),
        }
        Ok(SyncPg {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    /// Run one statement on this bridge's connection and wait for it.
    pub(crate) fn sql(&self, sql: &str) -> Result<Vec<TextRow>, PoolError> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        let closed = || PoolError::Db("the store is closed".into());
        self.jobs
            .as_ref()
            .ok_or_else(closed)?
            .send(Job {
                sql: sql.to_string(),
                reply,
            })
            .map_err(|_| closed())?;
        ordo_exec::bridge::wait(&rx).map_err(|_| closed())?
    }
}

impl Drop for SyncPg {
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A store error that names the table and nothing else. A `PoolError` can
/// carry the statement that failed, and a statement here carries a credential
/// id, which is the input to the enumeration the whole sign-in path is
/// careful about.
pub(crate) fn scrub(table: &str, e: PoolError) -> String {
    // A full pool names no table and no statement, and the door answers it
    // as `server_busy`, so its fixed words are kept whole.
    if e.is_busy() {
        return e.to_string();
    }
    // Missing is told by SQLSTATE; the text only says whether it is this
    // store's table that is missing.
    if e.is_missing_table() && e.to_string().contains(table) {
        return format!(
            "`{table}` does not exist: the deployment has not created its platform tables"
        );
    }
    format!("the `{table}` table could not be read")
}

/// [`scrub`], typed: the store's sentence, the client's class
/// (`PoolError::is_busy`), and the client error as the cause when it names
/// no statement (a full pool). What every identity store built on a pool
/// fails with.
pub(crate) fn scrub_fault(table: &str, e: PoolError) -> ordo_identity::StoreFault {
    if e.is_busy() {
        let words = e.to_string();
        return ordo_identity::StoreFault::caused_by(e.class(), words, e);
    }
    let class = e.class();
    ordo_identity::StoreFault::of_class(class, scrub(table, e))
}

/// The Postgres passkey store.
pub struct PgPasskeyStore {
    db: SyncPg,
}

impl PgPasskeyStore {
    /// Dial and stand the store up. Refuses when `$passkey` does not exist.
    pub fn open(cfg: PoolConfig) -> Result<PgPasskeyStore, PasskeyStoreError> {
        PgPasskeyStore::start(Source::Dial(cfg))
    }

    /// The same, **on a connection somebody else owns** (never the request
    /// pool; see [`crate::credentials::PgCredentialStore::open_on`]).
    pub fn open_on(pool: Pool) -> Result<PgPasskeyStore, PasskeyStoreError> {
        PgPasskeyStore::start(Source::Shared(pool))
    }

    fn start(source: Source) -> Result<PgPasskeyStore, PasskeyStoreError> {
        let db = SyncPg::start(source, "ordo-passkeys")
            .map_err(|e| PasskeyStoreError::Unavailable(e.into()))?;
        let store = PgPasskeyStore { db };
        store.sql(r#"SELECT 1 FROM "$passkey" LIMIT 1"#)?;
        Ok(store)
    }

    fn sql(&self, sql: &str) -> Result<Vec<TextRow>, PasskeyStoreError> {
        self.db
            .sql(sql)
            .map_err(|e| PasskeyStoreError::Unavailable(scrub_fault("$passkey", e)))
    }
}

fn authenticator(id: &[u8]) -> String {
    format!("passkey:{}", b64url_encode(id))
}

fn corrupt() -> PasskeyStoreError {
    PasskeyStoreError::Unavailable("a stored passkey row did not load".into())
}

impl PasskeyStore for PgPasskeyStore {
    fn get(&self, id: &[u8]) -> Result<Option<StoredPasskey>, PasskeyStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "algorithm", "public_key", "sign_count", "user_handle",
                      "backup_eligible"::int, "backed_up"::int, "registered_at", "last_used_at",
                      "role"
               FROM "$passkey" WHERE "authenticator" = {}"#,
            sql_lit(&authenticator(id))
        ))?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let col = |i: usize| row.get(i).and_then(|c| c.as_deref()).ok_or_else(corrupt);
        let num = |i: usize| col(i)?.parse::<i64>().map_err(|_| corrupt());
        let algorithm = num(0)?;
        let public_key = b64url_decode(col(1)?).map_err(|_| corrupt())?;
        // Re-runs the on-curve check, so a row edited by hand cannot load a
        // point that verifies something other than what registration saw.
        let key = CoseKey::from_bytes(algorithm, &public_key).map_err(|_| corrupt())?;
        let sign_count = u32::try_from(num(2)?).map_err(|_| corrupt())?;
        let user_handle = b64url_decode(col(3)?).map_err(|_| corrupt())?;
        let stamp = |i: usize| -> Result<Timestamp, PasskeyStoreError> {
            Ok(Timestamp(u64::try_from(num(i)?).map_err(|_| corrupt())?))
        };
        let role = col(8).and_then(|r| KeyRole::parse(r).ok_or_else(corrupt))?;
        Ok(Some(StoredPasskey {
            credential: PasskeyCredential::from_parts(id.to_vec(), key, sign_count),
            role,
            user_handle,
            backup_eligible: num(4)? != 0,
            backed_up: num(5)? != 0,
            registered_at: stamp(6)?,
            last_used_at: stamp(7)?,
        }))
    }

    fn insert(&self, cred: &StoredPasskey) -> Result<(), PasskeyStoreError> {
        let c = &cred.credential;
        // `DO NOTHING ... RETURNING` answers a row only when this statement
        // inserted one, so the database decides between two racing
        // registrations of one credential id.
        let rows = self.sql(&format!(
            r#"INSERT INTO "$passkey"
                 ("authenticator", "algorithm", "public_key", "sign_count", "user_handle",
                  "backup_eligible", "backed_up", "registered_at", "last_used_at", "role")
               VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {})
               ON CONFLICT ("authenticator") DO NOTHING
               RETURNING "authenticator""#,
            sql_lit(&authenticator(c.id())),
            c.algorithm(),
            sql_lit(&b64url_encode(&c.key().to_bytes())),
            c.sign_count(),
            sql_lit(&b64url_encode(&cred.user_handle)),
            cred.backup_eligible,
            cred.backed_up,
            cred.registered_at.0,
            cred.last_used_at.0,
            sql_lit(cred.role.as_str()),
        ))?;
        if rows.is_empty() {
            return Err(PasskeyStoreError::AlreadyExists);
        }
        Ok(())
    }

    fn set_sign_count(
        &self,
        id: &[u8],
        count: u32,
        at: Timestamp,
    ) -> Result<bool, PasskeyStoreError> {
        // One statement, so the comparison and the write cannot be pulled
        // apart by a second replica: the row advances only if `count` is
        // greater than what the row holds now.
        let rows = self.sql(&format!(
            r#"UPDATE "$passkey"
               SET "sign_count" = GREATEST("sign_count", {count}), "last_used_at" = {at}
               WHERE "authenticator" = {id}
                 AND ({count} = 0 OR "sign_count" < {count})
               RETURNING "authenticator""#,
            at = at.0,
            id = sql_lit(&authenticator(id)),
        ))?;
        Ok(!rows.is_empty())
    }

    fn remove(&self, id: &[u8]) -> Result<(), PasskeyStoreError> {
        self.sql(&format!(
            r#"DELETE FROM "$passkey" WHERE "authenticator" = {}"#,
            sql_lit(&authenticator(id))
        ))?;
        Ok(())
    }
}
