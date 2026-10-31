//! `PgKeyStore` — the [`WrappedKeyStore`] a self-host deployment boots on.
//!
//! `ordo_crypto::WrappedKeyStore` exists precisely so *"a deployment's
//! storage choice and its custody choice are independent"*, and the
//! Postgres implementation its own doc names as the self-host default had
//! never been written: `MemoryKeyStore` was the only one, which is why
//! `ordo revert` refused a sealed entity ("the binary has no `KeyProvider`
//! — there is no `WrappedKeyStore` over Postgres for it to boot one
//! against") and why a deployment's custody could not survive a restart.
//!
//! # Only wrapped material is stored
//!
//! `$keycustody` holds SMKs wrapped under the deployment master key and
//! DEKs wrapped under their space's SMK. The master never appears here.
//! That is what makes the table safe to include in the database backup,
//! and it is what makes the binary's startup assertion coherent: **wrapped
//! keys in the backup are the design; the master in the backup is the
//! misconfiguration.** A restore restores ciphertext, and a crypto-erased
//! space stays erased through any restore that does not also restore the
//! provider's key material.
//!
//! # Why this crate and not `ordo-crypto`
//!
//! `ordo-crypto`'s entire dependency list is two crates, and that is an
//! asset the release matrix and the distroless base both rest on. A
//! Postgres store belongs beside the pool that talks to Postgres. The
//! trait is the seam; this is one implementation of it.
//!
//! # The threading model, stated rather than discovered
//!
//! [`WrappedKeyStore`] is a **synchronous** trait — it has to be, because
//! `KeyProvider::seal_at` and `::open_at` are called from inside the Stage-5
//! write protocol and the read path, neither of which is a place a caller
//! can `.await` a key. The pool is async. So this store owns **one thread
//! with a runtime of its own**, dials its own connection there, and the
//! synchronous trait methods hand it a statement and block on the answer.
//!
//! Two consequences, both deliberate:
//!
//! - **Reads are served from a cache, not from a query.** A sealed write
//!   asks the store for the SMK and the DEK list once per sealed field;
//!   one round trip per field per row is the cost that rules out a
//!   KMS-native provider, and it would rule this one out too. The cache is
//!   loaded once at [`PgKeyStore::open`] and written through on every
//!   mutation, so it is exact for everything this process did.
//! - **Writes block the calling thread on a round trip.** Every caller is
//!   an operator act — provision a space, rotate an epoch, erase — never
//!   the request path. Blocking there is a millisecond somebody asked for.
//!
//! The staleness this admits is named in [`PgKeyStore::refresh`], and it
//! is the benign direction: a second replica that rotates adds an epoch
//! this process has not seen, so this process keeps sealing under the
//! older one — which stays readable by construction, since a rotation
//! retires nothing. It is wasteful until a refresh or a restart, not
//! wrong. The direction that would be wrong — a process opening under a
//! key that no longer exists — is a `DELETE`, and a deleted epoch is
//! `UnknownEpoch` on both sides of the cache.

use std::collections::BTreeMap;
use std::sync::RwLock;

use ordo_crypto::{Epoch, KeyError, SpaceId, WrappedKeyStore};
use ordo_fault::FaultClass;

use crate::{sql_lit, Pool, PoolConfig, PoolError, TextRow};

/// The wrapped-key table. `epoch` is `0` for an SMK row — SMKs have no
/// epoch, and a sentinel in a `NOT NULL` column keeps the primary key one
/// shape rather than making the row's identity depend on a nullable
/// column.
pub const KEYCUSTODY_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$keycustody" (
  "space"   text  NOT NULL,
  "kind"    text  NOT NULL,
  "epoch"   int   NOT NULL,
  "wrapped" bytea NOT NULL,
  PRIMARY KEY ("space", "kind", "epoch")
)"#;

/// Create `$keycustody`.
///
/// **Called at boot and by `ordo keys init`.** It used to be the
/// operator's act alone, on the argument that a binary creating this table
/// on the way up would be one step from provisioning custody on the way up.
/// Checklist 17v retired that: the first real deployment met an empty
/// database and crash-looped on `$keycustody` not existing, which is the
/// state every first deployment starts in, and it is the same defect the
/// artifact registry had until its own `ensure_tables` moved into
/// `ordo_host::runtime::assemble` on 2026-09-19.
///
/// **The step this is not.** An empty `$keycustody` grants nothing:
/// `LocalKeyProvider::release` still refuses a space with no SMK in it
/// *"rather than silently creating keys, so a typo cannot mint a tenant"*,
/// and the boot still refuses an `ORDO_SPACE` naming a tenant it has no
/// custody for. What the boot mints without being asked is exactly one
/// space, `ordo_host::provision::SYSTEM_SPACE`, whose name is a constant in
/// the binary rather than anything an operator can type.
///
/// Retry-once for the same reason [`Pool::ensure_platform_tables`] does it:
/// `CREATE TABLE IF NOT EXISTS` is not race-free between two processes.
pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(KEYCUSTODY_DDL).await.is_err() {
        pool.rows(KEYCUSTODY_DDL).await?;
    }
    Ok(())
}

/// One statement handed to the store's own thread, and where its answer
/// goes.
struct Job {
    sql: String,
    reply: std::sync::mpsc::SyncSender<Result<Vec<TextRow>, PoolError>>,
}

/// `LocalKeyProvider`'s Postgres-backed store.
pub struct PgKeyStore {
    /// `Option` so [`Drop`] can close the channel *before* joining the
    /// thread. Joining while still holding a sender is a deadlock, and it
    /// is the kind that only shows up when somebody drops a store.
    jobs: Option<tokio::sync::mpsc::UnboundedSender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Wrapped SMKs by space. Mirrors `MemoryKeyStore`'s shape on purpose:
    /// the two stores must be substitutable, and two different in-memory
    /// layouts is one more thing that can differ between what tests
    /// exercise and what deployments run.
    smks: RwLock<BTreeMap<SpaceId, Vec<u8>>>,
    deks: RwLock<BTreeMap<(SpaceId, Epoch), Vec<u8>>>,
}

/// Where the store's connection comes from: one it dials, or one shared
/// with the deployment's other single-connection stores.
enum Source {
    Dial(PoolConfig),
    Shared(Pool),
}

impl PgKeyStore {
    /// Dial, load every wrapped key, and stand up the store.
    ///
    /// **Synchronous, and safe to call from inside a runtime**, because the
    /// dial happens on a thread this function creates rather than on the
    /// caller's. That is what lets `main`, `ordo keys init` and a test all
    /// use one constructor.
    ///
    /// Refuses when `$keycustody` does not exist, naming the command that
    /// creates it: a store that silently reported an empty custody table
    /// would turn "you have not provisioned this deployment" into "no
    /// custody for that space", which is the same words for two very
    /// different situations.
    ///
    /// That refusal is still the answer for an operator command pointed at
    /// a database nothing has ever run against. It is no longer what a
    /// server does, because `assemble` calls [`ensure_tables`] before it
    /// opens a store (17v); the refusal a server gives now is about a
    /// space, never about the table.
    pub fn open(cfg: PoolConfig) -> Result<PgKeyStore, KeyError> {
        // One connection: every statement this store issues is serialized
        // through its single job loop anyway, so a larger pool would only
        // hold sockets open.
        PgKeyStore::start(Source::Dial(PoolConfig { size: 1, ..cfg }))
    }

    /// The same, **on a connection somebody else owns**: the one the
    /// deployment's single-connection stores share.
    ///
    /// **This store is the reason none of them may share the request
    /// pool.** A request checks out a `TxConn` and, while still holding
    /// it, reaches here to unwrap a data key (`crate::sealed::resolve`).
    /// Drawing from the request pool would make that a task holding one
    /// permit and waiting for a second from the same pool of N; at N
    /// concurrent requests every permit is held by a waiter and nothing
    /// can finish. `acquire_timeout` turns the hang into `Saturated` for
    /// everyone, which is load-triggered and gets worse as the pool grows.
    /// The pool passed here is a separate one (`ORDO_PG_STORE_POOL_SIZE`).
    pub fn open_on(pool: Pool) -> Result<PgKeyStore, KeyError> {
        PgKeyStore::start(Source::Shared(pool))
    }

    /// The one thread, runtime and job loop both constructors use.
    fn start(source: Source) -> Result<PgKeyStore, KeyError> {
        let (jobs, mut rx) = tokio::sync::mpsc::unbounded_channel::<Job>();
        let (ready, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), KeyError>>(1);

        let thread = std::thread::Builder::new()
            .name("ordo-keystore".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready.send(Err(KeyError::caused_by(
                            FaultClass::Broken,
                            format!("key store runtime: {e}"),
                            e,
                        )));
                        return;
                    }
                };
                rt.block_on(async move {
                    let pool = match source {
                        Source::Shared(p) => p.labelled("keys"),
                        Source::Dial(cfg) => match Pool::connect(cfg).await {
                            Ok(p) => p,
                            Err(e) => {
                                let _ = ready.send(Err(store_error(e)));
                                return;
                            }
                        },
                    };
                    let _ = ready.send(Ok(()));
                    // `recv().await` rather than a blocking receive. When
                    // this store dials its own connection the driver task
                    // lives on this runtime, so a thread parked in a
                    // blocking recv would be a socket nobody polls. On a
                    // shared pool the driver lives on the runtime that
                    // dialled it, and this loop must still await, because
                    // the pool's futures resolve only when that runtime
                    // drives them.
                    while let Some(job) = rx.recv().await {
                        let out = pool.rows(&job.sql).await;
                        let _ = job.reply.send(out);
                    }
                });
            })
            .map_err(|e| {
                KeyError::caused_by(FaultClass::Broken, format!("key store thread: {e}"), e)
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(KeyError::broken("key store thread died")),
        }

        let store = PgKeyStore {
            jobs: Some(jobs),
            thread: Some(thread),
            smks: RwLock::new(BTreeMap::new()),
            deks: RwLock::new(BTreeMap::new()),
        };
        store.refresh()?;
        Ok(store)
    }

    /// Re-read every wrapped key from Postgres.
    ///
    /// The answer to the staleness the module doc names: another replica's
    /// `rotate`, or an `ordo keys init` that ran after this process
    /// started. Cheap — one query, a handful of rows per space — and it is
    /// a `Worker`-shaped act rather than something the seal path pays for.
    pub fn refresh(&self) -> Result<(), KeyError> {
        let rows = self.sql(
            r#"SELECT "space", "kind", "epoch", encode("wrapped", 'hex') FROM "$keycustody""#,
        )?;
        let mut smks = BTreeMap::new();
        let mut deks = BTreeMap::new();
        for r in rows {
            let space = SpaceId(col(&r, 0)?.to_string());
            let kind = col(&r, 1)?.to_string();
            let epoch: u32 = col(&r, 2)?
                .parse()
                .map_err(|_| KeyError::broken("$keycustody: epoch is not a number"))?;
            let wrapped = unhex(col(&r, 3)?)?;
            match kind.as_str() {
                "smk" => {
                    smks.insert(space, wrapped);
                }
                "dek" => {
                    let epoch = u8::try_from(epoch).map_err(|_| {
                        KeyError::broken(format!("$keycustody: epoch {epoch} is out of range"))
                    })?;
                    deks.insert((space, epoch), wrapped);
                }
                other => {
                    return Err(KeyError::broken(format!(
                        "$keycustody: unknown key kind `{other}`"
                    )))
                }
            }
        }
        *self.smks.write().expect("key store") = smks;
        *self.deks.write().expect("key store") = deks;
        Ok(())
    }

    /// Does any wrapped value the cache holds equal `needle`, byte for byte?
    ///
    /// `plan-production-ops.md` R1b: `$keycustody` holds SMKs and DEKs
    /// **wrapped** under a key, never a key in the clear, and this is the
    /// check that finds the misconfiguration where one landed there anyway
    /// — the master key itself, or a value `derive_from_secret` produces
    /// from it, stored as if it were a wrapped row. It reads the cache
    /// [`PgKeyStore::open`] already loaded rather than issuing a query,
    /// because the boot assertion runs once, right after `open`, and a scan
    /// of what is already in hand needs no round trip.
    pub fn holds_bytes(&self, needle: &[u8]) -> bool {
        self.smks
            .read()
            .expect("key store")
            .values()
            .any(|w| w.as_slice() == needle)
            || self
                .deks
                .read()
                .expect("key store")
                .values()
                .any(|w| w.as_slice() == needle)
    }

    /// Run one statement on the store's own connection and wait for it.
    fn sql(&self, sql: &str) -> Result<Vec<TextRow>, KeyError> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.jobs
            .as_ref()
            .ok_or_else(|| KeyError::broken("key store is closed"))?
            .send(Job {
                sql: sql.to_string(),
                reply,
            })
            .map_err(|_| KeyError::broken("key store is closed"))?;
        match crate::bridge::wait(&rx) {
            Ok(Ok(rows)) => Ok(rows),
            Ok(Err(e)) => Err(store_error(e)),
            Err(_) => Err(KeyError::broken("key store is closed")),
        }
    }
}

/// `$keycustody` missing is a different fact from "no custody for that
/// space", and an operator who conflates them looks for the wrong thing.
fn store_error(e: PoolError) -> KeyError {
    // The class is read once, from the pool's own type, and kept with the
    // cause: a full pool must never read as a missing or destroyed key.
    let class = e.class();
    // The only table this store reads is `$keycustody`, so a missing table
    // is that one, told by its SQLSTATE rather than its words.
    let reason = if e.is_missing_table() {
        "`$keycustody` does not exist — run `ordo keys init --space <space> --as <who>`".to_string()
    } else {
        e.to_string()
    };
    KeyError::caused_by(class, reason, e)
}

fn col(row: &TextRow, i: usize) -> Result<&str, KeyError> {
    row.get(i)
        .and_then(|c| c.as_deref())
        .ok_or_else(|| KeyError::broken("$keycustody: a NOT NULL column came back null"))
}

fn unhex(s: &str) -> Result<Vec<u8>, KeyError> {
    if !s.len().is_multiple_of(2) {
        return Err(KeyError::broken("$keycustody: malformed wrapped key"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| KeyError::broken("$keycustody: malformed wrapped key"))
        })
        .collect()
}

/// A `bytea` literal. `standard_conforming_strings` is pinned on in
/// [`PoolConfig::pg`], which is what makes `'\x…'` mean the hex format
/// rather than a backslash somebody has to escape.
fn bytea(b: &[u8]) -> String {
    format!("'\\x{}'::bytea", ordo_crypto::hex(b))
}

impl Drop for PgKeyStore {
    fn drop(&mut self) {
        // Dropping the sender ends the job loop, which ends `block_on`,
        // which ends the thread. Joining it here means a dropped store has
        // actually released its connection by the time the drop returns —
        // which is what a test that drops a store and immediately opens
        // another against the same database needs.
        self.jobs.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl WrappedKeyStore for PgKeyStore {
    fn smk(&self, space: &SpaceId) -> Result<Option<Vec<u8>>, KeyError> {
        if let Some(w) = self.smks.read().expect("key store").get(space) {
            return Ok(Some(w.clone()));
        }
        // A miss is either a space with no custody — the refusal path,
        // where one query costs nothing — or an `ordo keys init` that ran
        // in another process after this one started. Looking once turns
        // the second case into an answer instead of a restart.
        let rows = self.sql(&format!(
            r#"SELECT encode("wrapped", 'hex') FROM "$keycustody" WHERE "space" = {} AND "kind" = 'smk'"#,
            sql_lit(&space.0)
        ))?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let wrapped = unhex(col(row, 0)?)?;
        self.smks
            .write()
            .expect("key store")
            .insert(space.clone(), wrapped.clone());
        // The DEKs arrived with it, or this process would hold an SMK it
        // cannot derive an epoch under.
        self.load_deks(space)?;
        Ok(Some(wrapped))
    }

    fn put_smk(&self, space: &SpaceId, wrapped: &[u8]) -> Result<(), KeyError> {
        // Postgres first, cache second. A failed write must never leave a
        // process believing it holds custody the database does not have —
        // it would seal under a key no restart can find.
        self.sql(&format!(
            r#"INSERT INTO "$keycustody" ("space", "kind", "epoch", "wrapped")
               VALUES ({}, 'smk', 0, {})
               ON CONFLICT ("space", "kind", "epoch") DO UPDATE SET "wrapped" = EXCLUDED."wrapped""#,
            sql_lit(&space.0),
            bytea(wrapped)
        ))?;
        self.smks
            .write()
            .expect("key store")
            .insert(space.clone(), wrapped.to_vec());
        Ok(())
    }

    fn deks(&self, space: &SpaceId) -> Result<Vec<(Epoch, Vec<u8>)>, KeyError> {
        let cached = |deks: &BTreeMap<(SpaceId, Epoch), Vec<u8>>| -> Vec<(Epoch, Vec<u8>)> {
            deks.iter()
                .filter(|((s, _), _)| s == space)
                .map(|((_, e), v)| (*e, v.clone()))
                .collect()
        };
        let found = cached(&self.deks.read().expect("key store"));
        if !found.is_empty() {
            return Ok(found);
        }
        // A miss is looked up, as `smk` looks one up. `current_epoch` is
        // the first question `Spaces::resolve` asks, and it reads only the
        // DEKs: answered from the cache alone, a tenant created on another
        // replica after this one booted had "no key custody" here until
        // something else happened to call `smk` for it. A replica that
        // serves no request for the tenant (only its routines) never did,
        // so every routine run it claimed failed.
        self.load_deks(space)?;
        Ok(cached(&self.deks.read().expect("key store")))
    }

    fn put_dek(&self, space: &SpaceId, epoch: Epoch, wrapped: &[u8]) -> Result<(), KeyError> {
        self.sql(&format!(
            r#"INSERT INTO "$keycustody" ("space", "kind", "epoch", "wrapped")
               VALUES ({}, 'dek', {epoch}, {})
               ON CONFLICT ("space", "kind", "epoch") DO UPDATE SET "wrapped" = EXCLUDED."wrapped""#,
            sql_lit(&space.0),
            bytea(wrapped)
        ))?;
        self.deks
            .write()
            .expect("key store")
            .insert((space.clone(), epoch), wrapped.to_vec());
        Ok(())
    }

    fn drop_dek(&self, space: &SpaceId, epoch: Epoch) -> Result<(), KeyError> {
        self.sql(&format!(
            r#"DELETE FROM "$keycustody" WHERE "space" = {} AND "kind" = 'dek' AND "epoch" = {epoch}"#,
            sql_lit(&space.0)
        ))?;
        self.deks
            .write()
            .expect("key store")
            .remove(&(space.clone(), epoch));
        Ok(())
    }

    fn drop_space(&self, space: &SpaceId) -> Result<(), KeyError> {
        // **One statement**, which is one transaction, which is the whole
        // reason the trait made this a method rather than "drop every DEK,
        // then drop the SMK": a store that fails halfway through that loop
        // leaves a space whose erasure is partial, and partial
        // crypto-erasure is indistinguishable from none for the field it
        // did not reach. The SMK and its DEKs are rows of one table, so
        // one `DELETE` removes them together or removes nothing.
        self.sql(&format!(
            r#"DELETE FROM "$keycustody" WHERE "space" = {}"#,
            sql_lit(&space.0)
        ))?;
        // Database first here too, and for the sharper version of the same
        // reason: a cache cleared before a failed delete would report a
        // space erased that a restart brings back.
        let mut smks = self.smks.write().expect("key store");
        let mut deks = self.deks.write().expect("key store");
        deks.retain(|(s, _), _| s != space);
        smks.remove(space);
        Ok(())
    }
}

impl PgKeyStore {
    fn load_deks(&self, space: &SpaceId) -> Result<(), KeyError> {
        let rows = self.sql(&format!(
            r#"SELECT "epoch", encode("wrapped", 'hex') FROM "$keycustody" WHERE "space" = {} AND "kind" = 'dek'"#,
            sql_lit(&space.0)
        ))?;
        let mut deks = self.deks.write().expect("key store");
        for r in rows {
            let epoch: u8 = col(&r, 0)?
                .parse()
                .map_err(|_| KeyError::broken("$keycustody: epoch is not an epoch"))?;
            deks.insert((space.clone(), epoch), unhex(col(&r, 1)?)?);
        }
        Ok(())
    }
}

/// The epochs a space holds a wrapped DEK for, ascending.
///
/// Read from the table rather than through the provider, because the
/// provider deliberately exposes operations and never its key list, and
/// `ordo keys status` needs the list without a key.
pub async fn epochs(pool: &Pool, space: &SpaceId) -> Result<Vec<Epoch>, KeyError> {
    let rows = pool
        .rows(&format!(
            r#"SELECT "epoch" FROM "$keycustody" WHERE "space" = {} AND "kind" = 'dek' ORDER BY "epoch""#,
            sql_lit(&space.0)
        ))
        .await
        .map_err(store_error)?;
    rows.iter()
        .map(|r| {
            col(r, 0)?
                .parse::<u8>()
                .map_err(|_| KeyError::broken("$keycustody: epoch is not an epoch"))
        })
        .collect()
}

/// Re-wrap every space's SMK from one deployment master key to another, in
/// **one transaction**.
///
/// **This is not SMK rotation and the two must not be confused.** A re-wrap
/// moves the *wrapping* of each space's SMK: one row per space, and no
/// envelope and no blind index moves, because the SMK itself is unchanged
/// and BIKs are SMK-derived. An SMK rotation moves every blind-index key
/// and is a full re-index of every `sealed indexed` column. The two differ
/// by orders of magnitude in cost and this command is the cheap one.
///
/// DEK rows are **not** touched: a DEK is wrapped under its space's SMK,
/// not under the master, so a master re-wrap cannot reach it and does not
/// need to. That is the layering doing its job — and it is why the loop
/// below reads only `kind = 'smk'`.
///
/// One transaction because a store that failed halfway would leave some
/// spaces openable by the old master and some by the new, with nothing
/// recording which — a deployment that can open neither set completely.
/// `dry_run` does every unwrap and re-wrap and rolls back, so an operator
/// can find out that the old passphrase is wrong before anything moves.
///
/// Returns the spaces it moved, in order.
pub async fn rewrap_master(
    pool: &Pool,
    old: &[u8; 32],
    new: &[u8; 32],
    dry_run: bool,
) -> Result<Vec<String>, KeyError> {
    let tx = pool.begin().await.map_err(store_error)?;
    let rows = tx
        .rows(
            r#"SELECT "space", encode("wrapped", 'hex') FROM "$keycustody"
               WHERE "kind" = 'smk' ORDER BY "space""#,
        )
        .await
        .map_err(store_error)?;

    let mut moved = Vec::new();
    for r in &rows {
        let space = SpaceId(col(r, 0)?.to_string());
        let wrapped = unhex(col(r, 1)?)?;
        // Unwrap under the old master with the space's own AAD, then
        // re-wrap under the new one with the same AAD. Any failure is
        // `Seal` and is indistinct — a wrong `--from` passphrase and a
        // corrupted row are the same answer, deliberately.
        let plain = ordo_crypto::open_with(old, "smk", &space.0, &wrapped)?;
        if plain.len() != 32 {
            return Err(KeyError::Seal);
        }
        let mut smk = [0u8; 32];
        smk.copy_from_slice(&plain);
        let rewrapped = ordo_crypto::seal_with(new, 0, "smk", &space.0, &smk);
        tx.rows(&format!(
            r#"UPDATE "$keycustody" SET "wrapped" = {} WHERE "space" = {} AND "kind" = 'smk'"#,
            bytea(&rewrapped),
            sql_lit(&space.0)
        ))
        .await
        .map_err(store_error)?;
        moved.push(space.0);
    }

    if dry_run {
        tx.rollback().await.map_err(store_error)?;
    } else {
        tx.commit().await.map_err(store_error)?;
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_pool_is_a_busy_key_store_never_a_plain_failure() {
        let busy = store_error(PoolError::Saturated);
        assert!(busy.is_busy() && busy.is_store_fault(), "{busy:?}");
        let broken = store_error(PoolError::Db("connection reset".into()));
        assert!(!broken.is_busy() && broken.is_store_fault(), "{broken:?}");
    }
}
