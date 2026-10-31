//! `PgCredentialStore`: the [`CredentialStore`] a deployment boots on.
//!
//! `ordo_identity::credentials` states what may be stored (a PHC string,
//! never a password) and why the trait is synchronous. What follows is only
//! what building the Postgres one decided.
//!
//! # The same shape as `$keycustody`, for the same reasons
//!
//! `ordo_exec::keystore::PgKeyStore` is the pattern: a table of its own, a
//! thread of its own with a current-thread runtime on it, and synchronous
//! trait methods that hand that thread a statement and block on the answer.
//! [`ordo_identity::provider::AuthProvider::complete`] is synchronous and is
//! called from inside an axum handler, so something has to bridge; doing it
//! on a thread this store owns means `main`, a test and an enrolment command
//! all use one constructor and none of them has to be async.
//!
//! # No cache, and that is the difference from the key store
//!
//! `PgKeyStore` caches because a sealed write asks it for a key per field
//! per row. This store is asked once per login. A cache would buy a round
//! trip on a path that already spends tens of milliseconds in argon2id, and
//! it would cost the thing that matters most here: a password change, a
//! disabled account or a credential wiped in an incident takes effect on the
//! next login rather than on the next restart. Every read is a query.
//!
//! **What one login costs**: one round trip on this store's connection for
//! the hash, plus one argon2id verification at the pinned profile (about 19
//! MiB and tens of milliseconds). The verification dominates, which is the
//! design working: the whole point of the profile is that a login is
//! expensive for whoever is guessing.
//!
//! # A restart must not quietly start over
//!
//! Two ways that could happen and both are shut. `open` **refuses** if
//! `$credential` is not there, rather than reporting an empty credential
//! table: "you have not provisioned this deployment" and "nobody is
//! enrolled" are the same words for very different situations, and the
//! second one locks every user out while looking healthy. And
//! `ordo_host::runtime` refuses the boot when a password method is
//! configured and this store cannot be opened, rather than falling back to
//! the in-memory one: a silent fallback is the same lockout with no error at
//! all, and it would come back a second time on the next restart.
//!
//! # Nothing here is sealed, and nothing here is reversible
//!
//! The stored value is an argon2id PHC string. It is not ciphertext and
//! there is no key that opens it, so the table is safe in a backup in the
//! only sense that matters: what leaks is what an offline attacker must
//! spend 19 MiB and two passes per guess against.

use std::sync::mpsc::SyncSender;

use ordo_identity::credentials::{CredentialStore, CredentialStoreError};

use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError, TextRow};

/// The credential table.
///
/// `identifier` is the primary key, which is what makes an enrolment an
/// upsert and a password change a one-statement act. There is no `principal`
/// column: the directory links an *authenticator id*
/// (`password:<identifier>`) to a principal, and duplicating that link here
/// would be a second place for it to be wrong.
pub const CREDENTIAL_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$credential" (
  "identifier"  text   NOT NULL,
  "hash"        text   NOT NULL,
  "enrolled_at" bigint NOT NULL DEFAULT 0,
  "updated_at"  bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("identifier")
)"#;

/// Create `$credential`.
///
/// Called at boot beside the other platform tables, the way
/// `PgDelegationRegistry::ensure_table` is, rather than from an operator
/// command the way `$keycustody` is. The difference is what an empty table
/// means: an empty custody table is a deployment with no keys, which must
/// refuse; an empty credential table is a deployment where nobody has signed
/// up yet, which is the ordinary first minute of one.
///
/// Retry-once for the reason `Pool::ensure_platform_tables` does it:
/// `CREATE TABLE IF NOT EXISTS` is not race-free between two processes.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(CREDENTIAL_DDL).await.is_err() {
        pool.rows(CREDENTIAL_DDL).await?;
    }
    Ok(())
}

/// One statement handed to the store's own thread, and where its answer
/// goes.
struct Job {
    sql: String,
    reply: SyncSender<Result<Vec<TextRow>, PoolError>>,
}

/// The Postgres credential store.
pub struct PgCredentialStore {
    /// `Option` so [`Drop`] can close the channel *before* joining the
    /// thread. Joining while still holding a sender is a deadlock, and it is
    /// the kind that only shows up when somebody drops a store.
    jobs: Option<tokio::sync::mpsc::UnboundedSender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
}

/// Where the store's connection comes from: one it dials, or one the
/// deployment already holds and three stores share.
enum Source {
    Dial(PoolConfig),
    Shared(Pool),
}

impl PgCredentialStore {
    /// Dial and stand the store up.
    ///
    /// **Synchronous, and safe to call from inside a runtime**, because the
    /// dial happens on a thread this function creates rather than on the
    /// caller's.
    ///
    /// Refuses when `$credential` does not exist. See the module doc.
    pub fn open(cfg: PoolConfig) -> Result<PgCredentialStore, CredentialStoreError> {
        PgCredentialStore::open_with_clock(cfg, Box::new(crate::runtime::now_secs))
    }

    /// The same, **on a connection somebody else owns**.
    ///
    /// The three single-connection stores (this one,
    /// [`crate::principal_directory::PgDirectoryStore`] and
    /// `ordo_exec::keystore::PgKeyStore`) each opened a pool of their own,
    /// so a deployment held three connections for three consumers that are
    /// each serialised through a single job loop and can never use more
    /// than one at a time between them. Sharing one costs a deployment two
    /// fewer connections.
    ///
    /// **What must still not happen is sharing the request pool.** A
    /// request checks out a `TxConn` and, while holding it, reaches the
    /// keystore to unwrap a data key (`ordo_exec::sealed`). A store drawing
    /// from the same pool would be a task holding one permit and waiting
    /// for a second from a pool of N, which at N concurrent requests is a
    /// circular wait that `acquire_timeout` turns into `Saturated` for
    /// everyone. This pool is a separate one (`ORDO_PG_STORE_POOL_SIZE`).
    ///
    /// **The trade, stated because it is not free.** While the shared pool
    /// was one connection, one store blocked on a lock or a slow commit held
    /// up every other. It has more than one since 2026-10-24
    /// (`ORDO_PG_STORE_POOL_SIZE`, board task `store-pool-held-blocks-signin`),
    /// so that takes every connection stuck at once.
    pub fn open_on(pool: Pool) -> Result<PgCredentialStore, CredentialStoreError> {
        PgCredentialStore::start(Source::Shared(pool), Box::new(crate::runtime::now_secs))
    }

    /// The one thread, runtime and job loop both constructors use.
    fn start(
        source: Source,
        now: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<PgCredentialStore, CredentialStoreError> {
        let (jobs, mut rx) = tokio::sync::mpsc::unbounded_channel::<Job>();
        let (ready, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        let thread = std::thread::Builder::new()
            .name("ordo-credentials".into())
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
                        Source::Shared(p) => p.labelled("credentials"),
                        Source::Dial(cfg) => match Pool::connect(cfg).await {
                            Ok(p) => p,
                            Err(e) => {
                                let _ = ready.send(Err(e.to_string()));
                                return;
                            }
                        },
                    };
                    let _ = ready.send(Ok(()));
                    // **The driver task is not on this runtime any more.**
                    // When this store dialled its own connection, the
                    // driver lived here and a blocking receive would have
                    // left a socket nobody polled. The shared pool was
                    // dialled on the deployment's main runtime, which polls
                    // it, so that hazard moved rather than vanished: this
                    // loop still awaits, because the pool's futures resolve
                    // only when that runtime drives them.
                    while let Some(job) = rx.recv().await {
                        let out = pool.rows(&job.sql).await;
                        let _ = job.reply.send(out);
                    }
                });
            })
            .map_err(|e| CredentialStoreError(format!("credential store thread: {e}").into()))?;
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(CredentialStoreError(e.into())),
            Err(_) => return Err(CredentialStoreError("credential store thread died".into())),
        }
        let store = PgCredentialStore {
            jobs: Some(jobs),
            thread: Some(thread),
            now,
        };
        store.sql(r#"SELECT 1 FROM "$credential" LIMIT 1"#)?;
        Ok(store)
    }

    /// The same, with the clock as an input, for a test that wants to know
    /// what timestamp a row was written with.
    pub fn open_with_clock(
        cfg: PoolConfig,
        now: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<PgCredentialStore, CredentialStoreError> {
        // One connection: every statement is serialized through the job
        // loop anyway, so a larger pool would only hold sockets open. It
        // also keeps this store's footprint off the pool the request path
        // shares — see `open_on` for why that separation is a rule rather
        // than a preference.
        PgCredentialStore::start(Source::Dial(PoolConfig { size: 1, ..cfg }), now)
    }

    /// Run one statement on the store's own connection and wait for it.
    fn sql(&self, sql: &str) -> Result<Vec<TextRow>, CredentialStoreError> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.jobs
            .as_ref()
            .ok_or_else(|| CredentialStoreError("credential store is closed".into()))?
            .send(Job {
                sql: sql.to_string(),
                reply,
            })
            .map_err(|_| CredentialStoreError("credential store is closed".into()))?;
        match ordo_exec::bridge::wait(&rx) {
            Ok(Ok(rows)) => Ok(rows),
            Ok(Err(e)) => Err(store_error(e)),
            Err(_) => Err(CredentialStoreError("credential store is closed".into())),
        }
    }
}

/// `$credential` missing is a different fact from "nobody is enrolled", and
/// an operator who conflates them looks for the wrong thing.
///
/// **The text is scrubbed of everything but the table.** A `PoolError` can
/// carry the statement that failed, and a statement here carries an
/// identifier. Identifiers are not secrets, but they are the input to the
/// enumeration this whole path is careful about, and an error that travels
/// to a log with one in it has published it. What comes back names the
/// table, the operation class and nothing else.
fn store_error(e: PoolError) -> CredentialStoreError {
    // A full pool carries no identifier, so it is kept as the cause, typed
    // busy: the door answers it `server_busy` instead of a broken store.
    if e.is_busy() {
        return CredentialStoreError(crate::passkeys::scrub_fault("$credential", e));
    }
    if e.is_missing_table() && e.to_string().contains("$credential") {
        return CredentialStoreError(
            "`$credential` does not exist: the deployment has not created its platform tables"
                .into(),
        );
    }
    CredentialStoreError("the credential table could not be read".into())
}

impl Drop for PgCredentialStore {
    fn drop(&mut self) {
        // Dropping the sender ends the job loop, which ends `block_on`,
        // which ends the thread. Joining here means a dropped store has
        // released its connection by the time the drop returns, which is
        // what a test that drops a store and immediately opens another
        // against the same database needs, and what makes the restart test
        // a restart rather than two live stores.
        self.jobs.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl CredentialStore for PgCredentialStore {
    fn hash_for(&self, identifier: &str) -> Result<Option<String>, CredentialStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "hash" FROM "$credential" WHERE "identifier" = {}"#,
            sql_lit(identifier)
        ))?;
        Ok(rows.first().and_then(|r| r.first()).and_then(|c| c.clone()))
    }

    fn put(&self, identifier: &str, hash: &str) -> Result<(), CredentialStoreError> {
        let at = (self.now)();
        // An upsert, because re-enrolling is what a password change is. The
        // `enrolled_at` of the original row is kept on the update path:
        // "when did this account first get a password" survives every change
        // to it, which is the question an incident asks.
        self.sql(&format!(
            r#"INSERT INTO "$credential" ("identifier", "hash", "enrolled_at", "updated_at")
               VALUES ({}, {}, {at}, {at})
               ON CONFLICT ("identifier")
               DO UPDATE SET "hash" = EXCLUDED."hash", "updated_at" = EXCLUDED."updated_at""#,
            sql_lit(identifier),
            sql_lit(hash)
        ))?;
        Ok(())
    }

    fn remove(&self, identifier: &str) -> Result<(), CredentialStoreError> {
        self.sql(&format!(
            r#"DELETE FROM "$credential" WHERE "identifier" = {}"#,
            sql_lit(identifier)
        ))?;
        Ok(())
    }
}
