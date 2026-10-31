//! The durable command log: `ordo_commandlog::Store` over Postgres, and
//! the per-space append lock that makes the chain a real total order.
//!
//! # Why this lives in `ordo-exec` and not in `ordo-commandlog`
//!
//! `ordo-commandlog` depends on `sha2` and nothing else, and that is a
//! property worth keeping rather than an accident. It holds the units of
//! history, the frozen canonical encoding, the chain arithmetic, revert
//! planning and checkpoint signing — all of it pure, none of it I/O. Two
//! crates depend on it today (`ordo-conformance`, `ordo-playground`) and
//! neither has a database driver; the SDK-side reference verifier the
//! design owes third parties would be a third such consumer, and it must be
//! able to check a chain with no server anywhere.
//!
//! Putting `tokio-postgres` into `ordo-commandlog` would invert that: the
//! leaf of the graph would pull the driver, an async runtime and a TLS
//! surface into every consumer of a hash function. So the dependency runs
//! the other way — `ordo-exec` already owns the driver, the pool and the
//! transaction seam, and it is the crate that can implement the trait
//! without anybody else paying for it. The trait is the seam; this is one
//! implementation of it, and the in-memory one stays the reference.
//!
//! # The append rides the caller's transaction
//!
//! [`TxConn::append_group`] takes an already-open transaction. That is the
//! whole point: 8f got as close as an in-memory store can get by *holding*
//! the group until commit, and said so — "until then the ordering IS the
//! guarantee". Here the `INSERT` is one of the transaction's own
//! statements, so a rolled-back write leaves no link and a committed one
//! cannot fail to leave one. There is no window, not a small one.
//!
//! # The lock is `pg_advisory_xact_lock`, and the difference matters
//!
//! `jobs.rs` uses `pg_try_advisory_lock` for clock leadership: **session**
//! scoped and **non-blocking**, which is exactly right there — a replica
//! that does not get leadership should carry on rather than queue, and the
//! claim should die with the process holding it.
//!
//! Chain append wants the opposite of both, and `implementation.md` pins
//! it: `pg_advisory_xact_lock` is **transaction** scoped and **blocking**.
//!
//! - *Blocking*, because a second writer in the same space must wait its
//!   turn, not fail. `try` would turn every concurrent write into an error
//!   the caller has to interpret.
//! - *Transaction scoped*, because the lock must be released by the commit
//!   or rollback that ends the append and by nothing else. A session lock
//!   on a pooled connection outlives the work it protected: under a
//!   transaction pooler the backend is handed to the next client still
//!   holding it, and `implementation.md` calls that out as the settled
//!   reason the write path never takes session locks.
//!
//! The lock is taken as late as possible — after parsing, planning, access
//! checks and statement execution — so the critical section covers hashing
//! and insert only. `implementation.md` names "anything that widens the
//! critical section" as the failure mode to watch for in review.
//!
//! # Belt and braces
//!
//! The lock is the mechanism; the schema is the proof. `(space, idx)` is
//! the primary key, so two appends that somehow interleaved would collide
//! on it rather than silently produce a forked chain, and `(space,
//! correlation)` is unique, so the duplicate-correlation rule is enforced
//! by the database and not only by a read-then-write check that a race
//! could straddle. Neither should ever fire. Both are cheap.

use ordo_commandlog::{
    chain::{decode_group, encode_group, hash_group},
    AppendError, ChainHash, ChainedGroup, Checkpoint, CommandGroup, CorrelationId, SpaceId, Store,
    TamperAt, TamperKind,
};
use sha2::{Digest, Sha256};

use crate::{sql_lit, Pool, PoolError, TxConn};

/// The durable chain and its signed checkpoints.
///
/// Created once at startup by [`crate::Pool::ensure_platform_tables`] for
/// the same reason the other platform tables are: `CREATE TABLE IF NOT
/// EXISTS` takes a lock even as a no-op, and running it inside an open
/// transaction blocks every other transaction that runs the same guard.
///
/// `body` is the **canonical encoding** — the exact bytes the chain hashed,
/// not a re-serialization of them. Storing anything else would mean the
/// thing verified and the thing stored were two different artifacts, and
/// the difference would be invisible until it mattered.
///
/// # `$checkpoints` is keyed by position **and head**
///
/// Ruled 2026-09-20 (`checkpoint-key`). A restore rewinds the chain and it
/// grows again through positions an earlier checkpoint already holds, so one
/// position can carry both an orphaned checkpoint, signed over history the
/// restore discarded, and a current one signed over the history the chain
/// holds now. Under `PRIMARY KEY ("space", "upto")` the second of those is
/// dropped by the insert's `ON CONFLICT`, and the space has no current
/// anchor until it writes past the orphan.
///
/// The change is **additive and idempotent**: the primary key is dropped if
/// it is there and a unique index over the three columns is created if it is
/// not, so a database created before the rule and one created after it end
/// in the same shape and neither loses a row. Deleting the orphan instead is
/// what a dishonest operator would do, and `plan-production-ops.md` rules it
/// out by name.
pub const COMMANDLOG_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$commandlog\" ( \
  \"space\" text NOT NULL, \
  \"idx\" bigint NOT NULL, \
  \"correlation\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"prev\" bytea NOT NULL, \
  \"hash\" bytea NOT NULL, \
  \"body\" bytea NOT NULL, \
  PRIMARY KEY (\"space\", \"idx\") \
); \
CREATE UNIQUE INDEX IF NOT EXISTS \"$commandlog_correlation\" \
  ON \"$commandlog\" (\"space\", \"correlation\"); \
CREATE INDEX IF NOT EXISTS \"$commandlog_elsewhere\" \
  ON \"$commandlog\" (\"correlation\"); \
CREATE TABLE IF NOT EXISTS \"$checkpoints\" ( \
  \"space\" text NOT NULL, \
  \"upto\" bigint NOT NULL, \
  \"head\" bytea NOT NULL, \
  \"at\" bigint NOT NULL, \
  \"key_id\" text NOT NULL, \
  \"signature\" bytea NOT NULL \
); \
ALTER TABLE \"$checkpoints\" DROP CONSTRAINT IF EXISTS \"$checkpoints_pkey\"; \
CREATE UNIQUE INDEX IF NOT EXISTS \"$checkpoints_position_head\" \
  ON \"$checkpoints\" (\"space\", \"upto\", \"head\")";

/// The advisory-lock namespace for chain append. Advisory lock space is
/// global to the database, so every user of it must agree on a namespace or
/// two unrelated subsystems will collide on a number. `0x4F52_444F` is
/// ASCII `ORDO`; the classifier goes in the low int and the space key in
/// the high one via the two-argument form.
// `pub` so the security stream's own tests can name BOTH namespaces and
// assert they do not interact — §6.1 says a denial's append must not
// queue behind a command append, and that is worth asserting rather than
// reviewing.
pub const APPEND_LOCK_NS: i32 = 0x4F52_444F;

/// The lock key for a space: the first four bytes of `SHA-256(space)`.
///
/// Deliberately **not** Postgres's `hashtext`. That function's value is not
/// stable across server versions, so a rolling upgrade would put two
/// replicas on different lock keys for the same space — the lock would
/// still be *held*, just not against the writer it was meant to exclude,
/// and the chain would fork under a lock that appeared to be working. A
/// hash computed here is the same number on every node forever.
pub fn space_lock_key(space: &SpaceId) -> i32 {
    let digest = Sha256::digest(space.0.as_bytes());
    i32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]])
}

pub(crate) fn bytea_lit(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2 + 12);
    s.push_str("'\\x");
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s.push_str("'::bytea");
    s
}

/// Decode Postgres's text rendering of `bytea` (`\x0a1b…`).
pub fn bytea_of(cell: Option<&String>) -> Option<Vec<u8>> {
    let text = cell?.strip_prefix("\\x")?;
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

pub(crate) fn chain_hash_of(cell: Option<&String>) -> Option<ChainHash> {
    Some(ChainHash(<[u8; 32]>::try_from(bytea_of(cell)?).ok()?))
}

/// A pool failure, typed for the caller's retry decision.
///
/// `AppendError::Store`'s own comment says a write path that cannot tell a
/// storage fault from a caller's bug retries the wrong one. The same
/// applies one level in: a serialization conflict is a storage fault that
/// SHOULD be retried, and flattening it into `Store` loses that. Two
/// spaces take different append locks and write disjoint rows, and they
/// still share `$commandlog`, so SSI raises `40001` between them under the
/// SERIALIZABLE level a command group runs at. This is the one place that
/// still holds the typed error, so it is the one place that can say so.
fn store_err(e: PoolError) -> AppendError {
    if e.is_retryable() {
        return AppendError::Retryable(e.to_string());
    }
    AppendError::Store(e.to_string())
}

impl TxConn {
    /// Append a group to its space's chain **inside this transaction**,
    /// under the per-space append lock.
    ///
    /// Order of operations, and every step is load-bearing:
    ///
    /// 1. **Validate the shape.** Malformed history never enters the chain,
    ///    and validating before taking the lock keeps a caller's bug out of
    ///    the critical section.
    /// 2. **Take `pg_advisory_xact_lock`.** From here to commit, this is
    ///    the only writer appending to this space.
    /// 3. **Read the head** — the last `(idx, hash)`. Under the lock this
    ///    is a stable answer; without it, two writers read the same head
    ///    and build two links claiming the same predecessor.
    /// 4. **Refuse a duplicate correlation**, scoped to this space.
    /// 5. **Hash and insert.** The insert is one of the caller's own
    ///    statements, so it commits with the effect or not at all.
    pub async fn append_group(&self, group: &CommandGroup) -> Result<ChainedGroup, AppendError> {
        let space = group.validate()?.clone();

        // (2) The serialization point. Blocking and transaction-scoped —
        // see the module doc for why neither half is negotiable.
        //
        // Timed, and timed AROUND THIS STATEMENT ALONE (5f). The statement
        // does nothing but block: `pg_advisory_xact_lock` returns void the
        // instant the lock is granted, so its wall time IS the wait, plus
        // one round trip that is also what an uncontended append costs.
        // Widening the span to include the head read would fold the work
        // into the wait and make the series unable to answer the only
        // question it exists for.
        let waited = std::time::Instant::now();
        self.rows(&format!(
            "SELECT pg_advisory_xact_lock({APPEND_LOCK_NS}, {})",
            space_lock_key(&space)
        ))
        .await
        .map_err(store_err)?;
        // Observed on the success path only: a lock acquisition that failed
        // never waited for a lock, it failed to talk to a database, and
        // charging that to the append lock would make an outage look like
        // contention.
        if let Some(obs) = self.pool.observer() {
            obs.lock_wait(&space.0, waited.elapsed().as_secs_f64());
        }
        // Steps (3)–(5) are the critical section this crate owns and this
        // crate can widen. Timed as one span in a helper rather than inline,
        // so that EVERY exit from it is measured — a duplicate correlation
        // and a failed insert each held the lock for as long as they took,
        // and a series that recorded only the successes would report the
        // section as narrower than it is exactly when something is wrong.
        let under_lock = std::time::Instant::now();
        let appended = self.append_under_lock(&space, group).await;
        if let Some(obs) = self.pool.observer() {
            obs.append_work(&space.0, under_lock.elapsed().as_secs_f64());
        }
        appended
    }

    /// Steps 3–5 of [`TxConn::append_group`], with the lock already held.
    ///
    /// Split out for the timing span above and for nothing else: the order
    /// of operations and every reason for it are documented on the caller,
    /// which is where a reader looks.
    async fn append_under_lock(
        &self,
        space: &SpaceId,
        group: &CommandGroup,
    ) -> Result<ChainedGroup, AppendError> {
        // (3) The head, read under the lock.
        let head = self
            .rows(&format!(
                "SELECT \"idx\", \"hash\" FROM \"$commandlog\" WHERE \"space\" = {} \
                 ORDER BY \"idx\" DESC LIMIT 1",
                sql_lit(&space.0)
            ))
            .await
            .map_err(store_err)?;
        let (next_idx, prev) = match head.first() {
            Some(row) => {
                let idx: i64 = row
                    .first()
                    .and_then(|c| c.as_deref())
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| AppendError::Store("chain head has no index".into()))?;
                let hash = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
                    .ok_or_else(|| AppendError::Store("chain head has no hash".into()))?;
                (idx + 1, hash)
            }
            None => (0, ChainHash::GENESIS),
        };

        // (4) `(space, correlation)` uniqueness — per space, never global.
        let dup = self
            .rows(&format!(
                "SELECT 1 FROM \"$commandlog\" WHERE \"space\" = {} AND \"correlation\" = {}",
                sql_lit(&space.0),
                sql_lit(&group.correlation.0)
            ))
            .await
            .map_err(store_err)?;
        if !dup.is_empty() {
            return Err(AppendError::DuplicateCorrelation(group.correlation.clone()));
        }

        // (5) Hash and insert.
        let hash = hash_group(prev, group);
        let body = encode_group(group);
        self.rows(&format!(
            "INSERT INTO \"$commandlog\" \
             (\"space\", \"idx\", \"correlation\", \"actor\", \"prev\", \"hash\", \"body\") \
             VALUES ({}, {next_idx}, {}, {}, {}, {}, {})",
            sql_lit(&space.0),
            sql_lit(&group.correlation.0),
            sql_lit(&group.actor.to_string()),
            bytea_lit(&prev.0),
            bytea_lit(&hash.0),
            bytea_lit(&body),
        ))
        .await
        .map_err(store_err)?;

        Ok(ChainedGroup {
            group: group.clone(),
            prev,
            hash,
        })
    }

    /// Record a signed checkpoint in this transaction.
    pub async fn put_checkpoint(&self, cp: &Checkpoint) -> Result<(), PoolError> {
        self.rows(&format!(
            "INSERT INTO \"$checkpoints\" \
             (\"space\", \"upto\", \"head\", \"at\", \"key_id\", \"signature\") \
             VALUES ({}, {}, {}, {}, {}, {}) \
             ON CONFLICT (\"space\", \"upto\", \"head\") DO NOTHING",
            sql_lit(&cp.space.0),
            cp.upto,
            bytea_lit(&cp.head.0),
            cp.at.0,
            sql_lit(&cp.key_id),
            bytea_lit(&cp.signature),
        ))
        .await
        .map(|_| ())
    }
}

/// The durable [`Store`].
///
/// # Blocking, and where it is legal
///
/// `Store` is a synchronous trait and the driver is asynchronous, so the
/// trait methods bridge through a [`tokio::runtime::Handle`] — the same
/// arrangement `BlockingTx` uses for the write protocol, and with the same
/// rule: **a `PgStore` may only be driven from a blocking thread**
/// (`spawn_blocking`, or a plain thread), never from a runtime worker,
/// where `block_on` panics.
///
/// Async callers should use the `try_*` inherent methods instead, which are
/// also the ones that can report a storage fault: the trait's accessors are
/// infallible by signature, so a database outage reads as "not found" to
/// them. That is tolerable for a lookup and **not** tolerable for
/// verification, which is why [`Store::verify_from`] is overridden below to
/// fail as [`TamperKind::Unreadable`] rather than to succeed vacuously on a
/// chain it could not read.
pub struct PgStore {
    pool: Pool,
    handle: tokio::runtime::Handle,
}

impl PgStore {
    pub fn new(pool: Pool, handle: tokio::runtime::Handle) -> PgStore {
        PgStore { pool, handle }
    }

    /// Append in a transaction of its own.
    ///
    /// READ COMMITTED, not SERIALIZABLE, and that is not an oversight: the
    /// advisory lock already imposes a total order on this space's
    /// appends, so there is no serialization anomaly left for SSI to
    /// detect and a serializable snapshot would only add `40001`s to a path
    /// that is already single-file. A group appended *with* an effect rides
    /// the effect's transaction and inherits its level — see
    /// [`TxConn::append_group`].
    pub async fn try_append(&self, group: &CommandGroup) -> Result<ChainedGroup, AppendError> {
        let tx = self
            .pool
            .begin_for(crate::OpClass::AddressedWrite)
            .await
            .map_err(store_err)?;
        let entry = tx.append_group(group).await?;
        tx.commit().await.map_err(store_err)?;
        Ok(entry)
    }

    pub async fn try_len(&self, space: &SpaceId) -> Result<usize, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT count(*) FROM \"$commandlog\" WHERE \"space\" = {}",
                sql_lit(&space.0)
            ))
            .await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    pub async fn try_head(&self, space: &SpaceId) -> Result<Option<ChainHash>, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"hash\" FROM \"$commandlog\" WHERE \"space\" = {} \
                 ORDER BY \"idx\" DESC LIMIT 1",
                sql_lit(&space.0)
            ))
            .await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_ref())
            .and_then(|c| chain_hash_of(Some(c))))
    }

    /// Entries in chain order. `where_` is already-escaped SQL.
    async fn select(&self, where_: &str) -> Result<Vec<ChainedGroup>, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"prev\", \"hash\", \"body\" FROM \"$commandlog\" \
                 WHERE {where_} ORDER BY \"idx\""
            ))
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let prev = chain_hash_of(row.first().and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("chain row has no prev".into()))?;
            let hash = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("chain row has no hash".into()))?;
            let body = bytea_of(row.get(2).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("chain row has no body".into()))?;
            // A body that does not decode is corruption, and it is reported
            // as such rather than skipped: a store that quietly drops rows
            // it cannot read would make `verify` pass over the gap.
            let group = decode_group(&body).map_err(|e| PoolError::db(e.to_string()))?;
            out.push(ChainedGroup { group, prev, hash });
        }
        Ok(out)
    }

    pub async fn try_get(
        &self,
        space: &SpaceId,
        index: usize,
    ) -> Result<Option<ChainedGroup>, PoolError> {
        Ok(self
            .select(&format!(
                "\"space\" = {} AND \"idx\" = {index}",
                sql_lit(&space.0)
            ))
            .await?
            .into_iter()
            .next())
    }

    pub async fn try_find(
        &self,
        space: &SpaceId,
        correlation: &CorrelationId,
    ) -> Result<Option<ChainedGroup>, PoolError> {
        Ok(self
            .select(&format!(
                "\"space\" = {} AND \"correlation\" = {}",
                sql_lit(&space.0),
                sql_lit(&correlation.0)
            ))
            .await?
            .into_iter()
            .next())
    }

    pub async fn try_entries(
        &self,
        space: &SpaceId,
        from: usize,
        limit: usize,
    ) -> Result<Vec<ChainedGroup>, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"prev\", \"hash\", \"body\" FROM \"$commandlog\" \
                 WHERE \"space\" = {} ORDER BY \"idx\" OFFSET {from} LIMIT {limit}",
                sql_lit(&space.0)
            ))
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let prev = chain_hash_of(row.first().and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("chain row has no prev".into()))?;
            let hash = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("chain row has no hash".into()))?;
            let body = bytea_of(row.get(2).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("chain row has no body".into()))?;
            let group = decode_group(&body).map_err(|e| PoolError::db(e.to_string()))?;
            out.push(ChainedGroup { group, prev, hash });
        }
        Ok(out)
    }

    /// Is this correlation logged in some **other** space? A boolean and
    /// nothing else — never the space, never the group, never an image.
    /// An existence probe on the correlation index, which is what the
    /// in-memory store's cross-chain scan stands in for.
    pub async fn try_is_logged_elsewhere(
        &self,
        space: &SpaceId,
        correlation: &CorrelationId,
    ) -> Result<bool, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT 1 FROM \"$commandlog\" WHERE \"correlation\" = {} \
                 AND \"space\" <> {} LIMIT 1",
                sql_lit(&correlation.0),
                sql_lit(&space.0)
            ))
            .await?;
        Ok(!rows.is_empty())
    }

    /// The most recent stored checkpoint for a space **the stored chain
    /// agrees with**, if any.
    ///
    /// The filter is `orphan-checkpoints` (2026-09-19) applied at the one
    /// place this is read: a chain proof's anchor
    /// (`ordo_host::attest_ops::build_proof`). A proof is handed to a third
    /// party, and an orphaned checkpoint names a head the chain no longer
    /// holds at that position, so a proof anchored there asks the receiver
    /// to check a suffix against history this deployment discarded. Better
    /// no anchor than a wrong one.
    pub async fn try_latest_checkpoint(
        &self,
        space: &SpaceId,
    ) -> Result<Option<Checkpoint>, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"upto\", \"head\", \"at\", \"key_id\", \"signature\" \
                 FROM \"$checkpoints\" WHERE \"space\" = {} \
                 ORDER BY \"upto\" DESC, \"head\" DESC",
                sql_lit(&space.0)
            ))
            .await?;
        for row in &rows {
            let upto: u64 = row
                .first()
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| PoolError::Db("checkpoint has no position".into()))?;
            let head = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("checkpoint has no head".into()))?;
            if crate::undo::agreement(crate::undo::Db::Pool(&self.pool), space, upto, head).await?
                != crate::undo::Agreement::Current
            {
                continue;
            }
            let at: u64 = row
                .get(2)
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| PoolError::Db("checkpoint has no instant".into()))?;
            let key_id = row
                .get(3)
                .and_then(|c| c.clone())
                .ok_or_else(|| PoolError::Db("checkpoint has no key id".into()))?;
            let signature = bytea_of(row.get(4).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("checkpoint has no signature".into()))?;
            return Ok(Some(Checkpoint {
                space: space.clone(),
                upto,
                head,
                at: ordo_commandlog::Timestamp(at),
                key_id,
                signature,
            }));
        }
        Ok(None)
    }

    fn block<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        self.handle.block_on(fut)
    }
}

impl Store for PgStore {
    fn append(&mut self, group: CommandGroup) -> Result<ChainedGroup, AppendError> {
        self.block(self.try_append(&group))
    }

    fn len(&self, space: &SpaceId) -> usize {
        self.block(self.try_len(space)).unwrap_or(0)
    }

    fn get(&self, space: &SpaceId, index: usize) -> Option<ChainedGroup> {
        self.block(self.try_get(space, index)).ok().flatten()
    }

    fn entries(&self, space: &SpaceId, from: usize, limit: usize) -> Vec<ChainedGroup> {
        self.block(self.try_entries(space, from, limit))
            .unwrap_or_default()
    }

    fn head(&self, space: &SpaceId) -> Option<ChainHash> {
        self.block(self.try_head(space)).ok().flatten()
    }

    fn find(&self, space: &SpaceId, correlation: &CorrelationId) -> Option<ChainedGroup> {
        self.block(self.try_find(space, correlation)).ok().flatten()
    }

    fn is_logged_elsewhere(&self, space: &SpaceId, correlation: &CorrelationId) -> bool {
        self.block(self.try_is_logged_elsewhere(space, correlation))
            .unwrap_or(false)
    }

    /// Streamed like the trait's default — but a read that *fails* is a
    /// verification failure, not an empty chain.
    ///
    /// The default implementation reads batches through `entries`, whose
    /// signature cannot report an error, so an unreachable database would
    /// answer "no entries" and `verify` would return `Ok`. A green
    /// verification of a chain nobody could read is the worst possible
    /// answer, so this override uses the fallible path and reports
    /// [`TamperKind::Unreadable`] at the index it got stuck on.
    fn verify_from(
        &self,
        space: &SpaceId,
        anchor: ChainHash,
        base_index: usize,
    ) -> Result<(), TamperAt> {
        let batch_size = ordo_commandlog::VERIFY_BATCH;
        let mut expected = anchor;
        let mut from = 0usize;
        loop {
            let batch = self
                .block(self.try_entries(space, from, batch_size))
                .map_err(|_| TamperAt {
                    space: space.clone(),
                    index: base_index + from,
                    kind: TamperKind::Unreadable,
                })?;
            if batch.is_empty() {
                return Ok(());
            }
            ordo_commandlog::verify_chain_from(space, &batch, expected, base_index + from)?;
            expected = batch.last().expect("non-empty").hash;
            from += batch.len();
            if batch.len() < batch_size {
                return Ok(());
            }
        }
    }
}
