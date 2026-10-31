//! The durable security-event stream: `ordo_secevent::SecurityStore` over
//! Postgres, with its own table, its own lock, and its own transaction.
//!
//! # Why this lives here and not in `ordo-secevent`
//!
//! The same reason [`crate::commandlog`] lives here and not in
//! `ordo-commandlog`, restated because it applies unchanged: `ordo-secevent`
//! depends on `sha2`, `ordo-commandlog` and `ordo-identity`, and none of
//! them pulls a database driver. The reference verifier a third party would
//! run against a published checkpoint must be able to check a stream with no
//! server anywhere. `ordo-exec` already owns the pool and the transaction
//! seam, so the dependency runs this way and nobody pays for a driver to get
//! a hash function.
//!
//! # Its own lock, and the assertion that matters
//!
//! `security-logging-design.md` §6.1 pins it: **no interaction with the
//! command chain's append lock**. A denial that took
//! `pg_advisory_xact_lock(ORDO, hash(space))` would let a caller with *no*
//! authority in a space contend with every legitimate writer in it — a
//! write-path denial of service handed out for free to anyone who can guess
//! a space id.
//!
//! So this module takes [`SECEVENT_LOCK_NS`], a different namespace from
//! [`crate::commandlog`]'s `APPEND_LOCK_NS`. The two-argument advisory-lock
//! form makes the namespace the high int, so no `(ns, key)` pair from one
//! subsystem can ever equal a pair from the other whatever the space hashes
//! to. `ordo-exec/tests/secevent.rs` holds the command chain's lock for a
//! space in one transaction and appends a security event for that same space
//! in another, and the append completes — asserted, not reviewed.
//!
//! The lock is still `pg_advisory_xact_lock` rather than the `try` form, and
//! for the command chain's reasons: a second appender to the same stream
//! must *wait* rather than fail, and the lock must be released by the
//! commit that ends the append and by nothing else, because a session lock
//! on a pooled connection outlives the work it protected.
//!
//! # Off the request path
//!
//! Nothing here is called from a request. `Recorder::flush_into` is driven
//! by a drainer — the ordinary job queue, in a deployment — and an append
//! failure is an operational alert, never a failed request: the caller was
//! answered long before. That is why [`PgSecurityStore::try_append`] opens
//! its own transaction rather than joining the caller's, which is the exact
//! opposite of `TxConn::append_group`'s design and deliberately so. A
//! command group must ride the effect's transaction because a group that
//! commits without its effect is a lie. A security entry has no effect to
//! ride, and binding it to a domain transaction would give a domain
//! rollback the power to erase a security record — the property this whole
//! artifact exists to deny.
//!
//! # Belt and braces
//!
//! `(scope, idx)` is the primary key, so two appends that somehow
//! interleaved collide rather than silently forking the stream. `body` is
//! the canonical encoding — the exact bytes hashed, domain-tagged — so a
//! `$commandlog` body inserted here fails to decode instead of verifying as
//! something.

use ordo_commandlog::{ChainHash, SpaceId, TamperAt, TamperKind, Timestamp};
use ordo_fault::StoreFault;
use ordo_secevent::chain::{decode_entry, encode_entry, hash_entry, ChainedEntry};
use ordo_secevent::store::{StreamCheckpoint, VERIFY_BATCH};
use ordo_secevent::{Scope, SecurityStore, StreamEntry, StreamError};
use sha2::{Digest, Sha256};

use crate::{sql_lit, Pool, PoolError, TxConn};

/// The stream's tables. Created at startup by
/// [`crate::Pool::ensure_platform_tables`], for the reason every other
/// platform table is: `CREATE TABLE IF NOT EXISTS` takes a lock even as a
/// no-op.
///
/// `scope` is the storage key — a space id, or the reserved
/// `ordo_secevent::DEPLOYMENT_KEY`, which is not a valid space id and so
/// cannot collide with one.
///
/// `subject`, `op`, `kind` and `about` are denormalized out of the body so
/// an operator can filter without decoding every row. They are a **copy**,
/// not the record: verification reads `body` and nothing else, so a
/// doctored `subject` (or `about`) column changes what a query returns and
/// does not change what `verify` recomputes, and the mismatch is then
/// visible by decoding, which is strictly better than having no queryable
/// columns at all.
///
/// `about` arrived after the table shipped (`plan-opslog.md` D1b). There is
/// no migration machinery for platform tables (`Pool::ensure_platform_tables`
/// runs `CREATE TABLE IF NOT EXISTS` on every boot and stops there), so the
/// column needs its own statement, `ALTER TABLE ... ADD COLUMN IF NOT
/// EXISTS`, idempotent the same way the table's own creation is. It is
/// nullable and defaults to nothing: a row written before this column
/// existed has no target to backfill, and null says exactly that rather
/// than a sentinel pretending to a value nobody recorded.
pub const SECEVENT_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$secevents\" ( \
  \"scope\" text NOT NULL, \
  \"idx\" bigint NOT NULL, \
  \"subject\" text NOT NULL, \
  \"subject_kind\" text NOT NULL, \
  \"op\" text NOT NULL, \
  \"kind\" text NOT NULL, \
  \"first_at\" bigint NOT NULL, \
  \"last_at\" bigint NOT NULL, \
  \"count\" bigint NOT NULL, \
  \"prev\" bytea NOT NULL, \
  \"hash\" bytea NOT NULL, \
  \"body\" bytea NOT NULL, \
  PRIMARY KEY (\"scope\", \"idx\") \
); \
ALTER TABLE \"$secevents\" ADD COLUMN IF NOT EXISTS \"about\" text; \
CREATE INDEX IF NOT EXISTS \"$secevents_subject\" \
  ON \"$secevents\" (\"scope\", \"subject\", \"last_at\"); \
CREATE TABLE IF NOT EXISTS \"$seccheckpoints\" ( \
  \"scope\" text NOT NULL, \
  \"upto\" bigint NOT NULL, \
  \"head\" bytea NOT NULL, \
  \"at\" bigint NOT NULL, \
  \"key_id\" text NOT NULL, \
  \"signature\" bytea NOT NULL, \
  PRIMARY KEY (\"scope\", \"upto\") \
)";

/// The advisory-lock namespace for security-stream appends. ASCII `ORDS`.
///
/// Deliberately **not** `commandlog::APPEND_LOCK_NS` (`ORDO`). The
/// two-argument advisory-lock form keys on the pair, so no space hash can
/// make a security append collide with a command append. This constant is
/// the whole of §6.1's "no interaction with the command chain's append
/// lock", and the test asserts it behaviourally rather than by reading it.
pub const SECEVENT_LOCK_NS: i32 = 0x4F52_4453;

/// The lock key for a scope: the first four bytes of `SHA-256(scope)`.
/// Computed here rather than with `hashtext`, for the reason 8g settled — a
/// server-version-dependent hash forks the stream under a lock that looks
/// healthy during a rolling upgrade.
pub fn scope_lock_key(scope: &Scope) -> i32 {
    let digest = Sha256::digest(scope.key().0.as_bytes());
    i32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]])
}

fn bytea_lit(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2 + 12);
    s.push_str("'\\x");
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s.push_str("'::bytea");
    s
}

fn bytea_of(cell: Option<&String>) -> Option<Vec<u8>> {
    let text = cell?.strip_prefix("\\x")?;
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn chain_hash_of(cell: Option<&String>) -> Option<ChainHash> {
    Some(ChainHash(<[u8; 32]>::try_from(bytea_of(cell)?).ok()?))
}

fn store_err(e: PoolError) -> StreamError {
    StreamError::Store(StoreFault::caused_by(e.class(), e.to_string(), e))
}

impl TxConn {
    /// Append one security entry inside this transaction, under the
    /// stream's own lock.
    ///
    /// Ordinarily called from [`PgSecurityStore::try_append`], which opens a
    /// transaction of its own. It is exposed on `TxConn` so a drainer can
    /// batch a window's worth of entries into one transaction — the entries
    /// of one closed window belong together, and appending them one
    /// transaction apiece would pay the lock round trip per entry.
    pub async fn append_security_entry(
        &self,
        entry: &StreamEntry,
    ) -> Result<ChainedEntry, StreamError> {
        let scope = entry.scope.clone();
        let key = scope.key();

        // The stream's own serialization point, in its own namespace.
        self.rows(&format!(
            "SELECT pg_advisory_xact_lock({SECEVENT_LOCK_NS}, {})",
            scope_lock_key(&scope)
        ))
        .await
        .map_err(store_err)?;

        let head = self
            .rows(&format!(
                "SELECT \"idx\", \"hash\" FROM \"$secevents\" WHERE \"scope\" = {} \
                 ORDER BY \"idx\" DESC LIMIT 1",
                sql_lit(&key.0)
            ))
            .await
            .map_err(store_err)?;
        let (next_idx, prev) = match head.first() {
            Some(row) => {
                let idx: i64 = row
                    .first()
                    .and_then(|c| c.as_deref())
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| StreamError::broken("stream head has no index"))?;
                let hash = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
                    .ok_or_else(|| StreamError::broken("stream head has no hash"))?;
                (idx + 1, hash)
            }
            None => (0, ChainHash::GENESIS),
        };

        let hash = hash_entry(prev, entry);
        let body = encode_entry(entry);
        let about_lit = entry
            .about
            .as_ref()
            .map(|s| sql_lit(&s.0))
            .unwrap_or_else(|| "NULL".into());
        self.rows(&format!(
            "INSERT INTO \"$secevents\" \
             (\"scope\", \"idx\", \"subject\", \"subject_kind\", \"op\", \"kind\", \"about\", \
              \"first_at\", \"last_at\", \"count\", \"prev\", \"hash\", \"body\") \
             VALUES ({}, {next_idx}, {}, {}, {}, {}, {about_lit}, {}, {}, {}, {}, {}, {})",
            sql_lit(&key.0),
            sql_lit(&entry.subject.render()),
            sql_lit(entry.subject.kind_str()),
            sql_lit(&entry.op),
            sql_lit(entry.kind.label()),
            entry.first_at.0,
            entry.last_at.0,
            entry.count,
            bytea_lit(&prev.0),
            bytea_lit(&hash.0),
            bytea_lit(&body),
        ))
        .await
        .map_err(store_err)?;

        Ok(ChainedEntry {
            entry: entry.clone(),
            prev,
            hash,
        })
    }

    /// Record a signed stream checkpoint in this transaction.
    pub async fn put_security_checkpoint(&self, cp: &StreamCheckpoint) -> Result<(), PoolError> {
        self.rows(&format!(
            "INSERT INTO \"$seccheckpoints\" \
             (\"scope\", \"upto\", \"head\", \"at\", \"key_id\", \"signature\") \
             VALUES ({}, {}, {}, {}, {}, {}) ON CONFLICT (\"scope\", \"upto\") DO NOTHING",
            sql_lit(&cp.scope.key().0),
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

/// What [`PgSecurityStore::try_entries_where`] keeps. Every field unset
/// keeps everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryFilter {
    /// The kind's label, as `EventKind::label` spells it (`denied`, `deploy`).
    pub kind: Option<String>,
    /// The space a deployment-scope entry is about.
    pub about: Option<String>,
    /// Entries whose last occurrence is at or after this, in seconds.
    pub since: Option<u64>,
    /// Entries whose first occurrence is at or before this, in seconds.
    pub until: Option<u64>,
}

impl EntryFilter {
    /// Does the decoded entry pass? The body is the record, so this is the
    /// check that counts.
    pub fn admits(&self, e: &StreamEntry) -> bool {
        self.kind.as_deref().is_none_or(|k| e.kind.label() == k)
            && self
                .about
                .as_deref()
                .is_none_or(|a| e.about.as_ref().is_some_and(|s| s.0 == a))
            && self.since.is_none_or(|t| e.last_at.0 >= t)
            && self.until.is_none_or(|t| e.first_at.0 <= t)
    }
}

/// Which way one page of [`PgSecurityStore::try_entries_where`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Oldest first, from this full-stream index on.
    Forward { from: usize },
    /// Newest first, from just before this index, or from the head.
    Back { before: Option<usize> },
}

/// The durable [`SecurityStore`].
///
/// Same blocking rule as [`crate::commandlog::PgStore`]: the trait is
/// synchronous and the driver is not, so the trait methods bridge through a
/// [`tokio::runtime::Handle`] and **may only be driven from a blocking
/// thread**. Async callers use the `try_*` inherent methods, which are also
/// the ones that can report a storage fault — and [`SecurityStore::verify_from`]
/// is overridden below so a database that cannot be read reports
/// [`TamperKind::Unreadable`] rather than verifying an empty stream.
pub struct PgSecurityStore {
    pool: Pool,
    handle: tokio::runtime::Handle,
}

impl PgSecurityStore {
    pub fn new(pool: Pool, handle: tokio::runtime::Handle) -> PgSecurityStore {
        PgSecurityStore { pool, handle }
    }

    /// Append in a transaction of its own — never the caller's. See the
    /// module doc: a security entry must not be revocable by a domain
    /// rollback.
    pub async fn try_append(&self, entry: &StreamEntry) -> Result<ChainedEntry, StreamError> {
        let tx = self
            .pool
            .begin_for(crate::OpClass::AddressedWrite)
            .await
            .map_err(store_err)?;
        let stored = tx.append_security_entry(entry).await?;
        tx.commit().await.map_err(store_err)?;
        Ok(stored)
    }

    /// Append a closed window's entries in one transaction, under one lock
    /// acquisition per scope.
    pub async fn try_append_all(&self, entries: &[StreamEntry]) -> Result<usize, StreamError> {
        if entries.is_empty() {
            return Ok(0);
        }
        let tx = self
            .pool
            .begin_for(crate::OpClass::AddressedWrite)
            .await
            .map_err(store_err)?;
        for e in entries {
            tx.append_security_entry(e).await?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(entries.len())
    }

    pub async fn try_len(&self, scope: &Scope) -> Result<usize, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT count(*) FROM \"$secevents\" WHERE \"scope\" = {}",
                sql_lit(&scope.key().0)
            ))
            .await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    /// The full-stream index of the oldest entry still stored. Read from
    /// the stored `idx` rather than tracked separately: after a prune the
    /// surviving rows keep the indices they were appended at, so the base
    /// is a fact about the table and not a number that could drift from it.
    pub async fn try_base_index(&self, scope: &Scope) -> Result<usize, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT min(\"idx\") FROM \"$secevents\" WHERE \"scope\" = {}",
                sql_lit(&scope.key().0)
            ))
            .await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    pub async fn try_head(&self, scope: &Scope) -> Result<Option<ChainHash>, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"hash\" FROM \"$secevents\" WHERE \"scope\" = {} \
                 ORDER BY \"idx\" DESC LIMIT 1",
                sql_lit(&scope.key().0)
            ))
            .await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_ref())
            .and_then(|c| chain_hash_of(Some(c))))
    }

    pub async fn try_entries(
        &self,
        scope: &Scope,
        from: usize,
        limit: usize,
    ) -> Result<Vec<ChainedEntry>, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"prev\", \"hash\", \"body\" FROM \"$secevents\" \
                 WHERE \"scope\" = {} ORDER BY \"idx\" OFFSET {from} LIMIT {limit}",
                sql_lit(&scope.key().0)
            ))
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let prev = chain_hash_of(row.first().and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("stream row has no prev".into()))?;
            let hash = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("stream row has no hash".into()))?;
            let body = bytea_of(row.get(2).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("stream row has no body".into()))?;
            // A body that does not decode is corruption and is reported as
            // such rather than skipped — including a command-chain body
            // stored here, which fails on the domain tag.
            let entry = decode_entry(&body).map_err(|e| PoolError::db(e.to_string()))?;
            out.push(ChainedEntry { entry, prev, hash });
        }
        Ok(out)
    }

    /// One page of a scope's entries that pass `filter`, each with its
    /// full-stream index (`plan-console.md` G8).
    ///
    /// `page` says which way to read: forward from an index, or back from
    /// just before one, newest first. The filter runs in SQL on the
    /// denormalized columns, then again on each decoded body, so a doctored
    /// column can hide a row from a query but never put a row into an
    /// answer it does not belong to.
    pub async fn try_entries_where(
        &self,
        scope: &Scope,
        filter: &EntryFilter,
        page: Page,
        limit: usize,
    ) -> Result<Vec<(usize, ChainedEntry)>, PoolError> {
        let mut clauses = vec![format!("\"scope\" = {}", sql_lit(&scope.key().0))];
        if let Some(k) = &filter.kind {
            clauses.push(format!("\"kind\" = {}", sql_lit(k)));
        }
        if let Some(a) = &filter.about {
            clauses.push(format!("\"about\" = {}", sql_lit(a)));
        }
        if let Some(t) = filter.since {
            clauses.push(format!("\"last_at\" >= {t}"));
        }
        if let Some(t) = filter.until {
            clauses.push(format!("\"first_at\" <= {t}"));
        }
        let order = match page {
            Page::Forward { from } => {
                clauses.push(format!("\"idx\" >= {from}"));
                "ASC"
            }
            Page::Back { before } => {
                if let Some(b) = before {
                    clauses.push(format!("\"idx\" < {b}"));
                }
                "DESC"
            }
        };
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"idx\", \"prev\", \"hash\", \"body\" FROM \"$secevents\" \
                 WHERE {} ORDER BY \"idx\" {order} LIMIT {limit}",
                clauses.join(" AND ")
            ))
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let idx: usize = row
                .first()
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| PoolError::Db("stream row has no index".into()))?;
            let prev = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("stream row has no prev".into()))?;
            let hash = chain_hash_of(row.get(2).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("stream row has no hash".into()))?;
            let body = bytea_of(row.get(3).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("stream row has no body".into()))?;
            let entry = decode_entry(&body).map_err(|e| PoolError::db(e.to_string()))?;
            if filter.admits(&entry) {
                out.push((idx, ChainedEntry { entry, prev, hash }));
            }
        }
        Ok(out)
    }

    /// Drop every row before full-stream index `upto`. Reached through
    /// `ordo_secevent::prune_to_checkpoint`, which is the only path that
    /// checks the cut is anchored at a verified checkpoint.
    pub async fn try_prune_before(&self, scope: &Scope, upto: usize) -> Result<usize, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "WITH gone AS (DELETE FROM \"$secevents\" WHERE \"scope\" = {} AND \"idx\" < {upto} \
                 RETURNING 1) SELECT count(*) FROM gone",
                sql_lit(&scope.key().0)
            ))
            .await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    /// The policy's candidate cut point: how many leading entries have aged
    /// out, never counting the last one.
    ///
    /// The async twin of `Retention::prunable_upto`, and it asks the
    /// database rather than decoding bodies — `last_at` is a denormalized
    /// column, so the whole scan is one indexed aggregate instead of a
    /// `VERIFY_BATCH` walk. The column is a **copy**, and the module doc
    /// says why that is safe here: a doctored `last_at` changes which rows
    /// this offers to delete, and `check_prune` then refuses the cut unless
    /// a *signed* checkpoint covers it and its head matches the stored
    /// chain. Retention cannot be steered into destroying evidence by
    /// editing an unhashed column.
    ///
    /// The retained tail is applied here, so the ordinary case — a stream
    /// every entry of which has aged out — reaches the floor by arithmetic
    /// rather than by refusal.
    pub async fn try_prunable_upto(
        &self,
        scope: &Scope,
        retention: ordo_secevent::Retention,
        now: Timestamp,
    ) -> Result<u64, PoolError> {
        let cutoff = now.0.saturating_sub(retention.keep_secs_for(scope));
        let key = sql_lit(&scope.key().0);
        // The first entry that is still inside the window. Everything below
        // it has aged out; nothing at or above it may be cut.
        let rows = self
            .pool
            .rows(&format!(
                "SELECT min(\"idx\") FROM \"$secevents\" \
                 WHERE \"scope\" = {key} AND \"last_at\" >= {cutoff}"
            ))
            .await?;
        let youngest: Option<u64> = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok());
        let base = self.try_base_index(scope).await? as u64;
        let end = base + self.try_len(scope).await? as u64;
        let aged = youngest.unwrap_or(end);
        Ok(if end > 0 { aged.min(end - 1) } else { 0 })
    }

    /// The greatest stored checkpoint whose position is at or below
    /// `candidate` — the only legal cut point at or under the policy's
    /// line.
    pub async fn try_checkpoint_at_or_below(
        &self,
        scope: &Scope,
        candidate: u64,
    ) -> Result<Option<StreamCheckpoint>, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"upto\", \"head\", \"at\", \"key_id\", \"signature\" \
                 FROM \"$seccheckpoints\" WHERE \"scope\" = {} AND \"upto\" <= {candidate} \
                 ORDER BY \"upto\" DESC LIMIT 1",
                sql_lit(&scope.key().0)
            ))
            .await?;
        self.decode_checkpoint(scope, rows.first())
    }

    /// The most recent stored checkpoint for a scope, if any.
    pub async fn try_latest_checkpoint(
        &self,
        scope: &Scope,
    ) -> Result<Option<StreamCheckpoint>, PoolError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"upto\", \"head\", \"at\", \"key_id\", \"signature\" \
                 FROM \"$seccheckpoints\" WHERE \"scope\" = {} ORDER BY \"upto\" DESC LIMIT 1",
                sql_lit(&scope.key().0)
            ))
            .await?;
        self.decode_checkpoint(scope, rows.first())
    }

    /// One `(upto, head, at, key_id, signature)` row, decoded. Shared by
    /// the two checkpoint queries so a column-order change cannot make one
    /// of them silently read the wrong field.
    fn decode_checkpoint(
        &self,
        scope: &Scope,
        row: Option<&crate::TextRow>,
    ) -> Result<Option<StreamCheckpoint>, PoolError> {
        let Some(row) = row else {
            return Ok(None);
        };
        let upto: u64 = row
            .first()
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| PoolError::Db("checkpoint has no position".into()))?;
        let head = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
            .ok_or_else(|| PoolError::Db("checkpoint has no head".into()))?;
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
        Ok(Some(StreamCheckpoint {
            scope: scope.clone(),
            upto,
            head,
            at: Timestamp(at),
            key_id,
            signature,
        }))
    }

    /// Every scope that has any stored entry, in storage-key order.
    ///
    /// The deployment scope is one of these and is not special: it is the
    /// scope whose key is [`ordo_secevent::DEPLOYMENT_KEY`], and a
    /// retention pass walks it exactly like a space's.
    pub async fn try_scopes(&self) -> Result<Vec<Scope>, PoolError> {
        let rows = self
            .pool
            .rows("SELECT DISTINCT \"scope\" FROM \"$secevents\" ORDER BY \"scope\"")
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.first()?.clone())
            .map(|key| {
                if key == ordo_secevent::DEPLOYMENT_KEY {
                    Scope::Deployment
                } else {
                    Scope::Space(SpaceId(key))
                }
            })
            .collect())
    }

    /// Recompute the stored suffix against `anchor`, in [`VERIFY_BATCH`]
    /// windows — the async form, and the one any caller already inside a
    /// runtime must use.
    ///
    /// The [`SecurityStore`] trait's `verify_from` is the same walk behind
    /// `Handle::block_on`, which **panics** when called from a thread that
    /// is already running the runtime. That is not a hypothetical: the
    /// obvious way to write `ordo verify`'s new half is to call the trait
    /// method from an `async fn`, and it would have panicked on the first
    /// stream with an entry in it. So the walk lives here, async, and the
    /// blocking method delegates to it rather than the other way round —
    /// one implementation, and the shape that cannot deadlock is the
    /// primitive.
    ///
    /// **A read that fails is a verification failure**, not an empty
    /// stream, for the trait's stated reason: a green verification of a
    /// record nobody could read is the worst answer this can give.
    pub async fn try_verify_from(
        &self,
        scope: &Scope,
        anchor: ChainHash,
        base_index: usize,
    ) -> Result<(), TamperAt> {
        self.try_verify_to_head(scope, anchor, base_index)
            .await
            .map(|_| ())
    }

    /// [`Self::try_verify_from`], returning the `(upto, head)` the walk
    /// **arrived at** — one past the last verified position, and that
    /// entry's hash. `None` for a stream with nothing stored.
    ///
    /// # Why the walk returns its head rather than the caller re-reading it
    ///
    /// This is what lets a checkpoint signer promise that the head it signs
    /// is a head it verified, **without holding the append lock**, and that
    /// is a stronger promise than the command chain's own signer makes.
    /// `undo::sign_checkpoint` takes `pg_advisory_xact_lock(APPEND_LOCK_NS,
    /// space)` for the whole of its verification precisely so that the
    /// `head_of` it reads afterwards is the head it just verified — the lock
    /// is standing in for this return value, and it pays for the guarantee
    /// with an append stall proportional to the length of the chain.
    ///
    /// Here the signed pair *is* the walk's own last observation, so a
    /// concurrent append cannot get between the verification and the
    /// signature. It can only land *after* the position signed, which is
    /// always allowed: a checkpoint is a claim about a prefix, never a claim
    /// that nothing came later. Nothing blocks the drainer, and the property
    /// is exact rather than protected by a window being closed.
    ///
    /// The alternative — verify, then `try_head` — is the shape that looks
    /// obviously correct and is not: an append committing between the two
    /// reads yields a signature over an entry nothing verified, which is
    /// `store.rs`'s "signing a head storage does not hold" arriving by
    /// accident rather than by malice.
    pub async fn try_verify_to_head(
        &self,
        scope: &Scope,
        anchor: ChainHash,
        base_index: usize,
    ) -> Result<Option<(u64, ChainHash)>, TamperAt> {
        let mut expected = anchor;
        let mut from = 0usize;
        let mut head: Option<(u64, ChainHash)> = None;
        loop {
            let batch = match self.try_entries(scope, from, VERIFY_BATCH).await {
                Ok(batch) => batch,
                // **Which entry, not which window.** `try_entries` fails the
                // whole read on the first row it cannot decode and its error
                // carries no position, so reporting it directly would name
                // the start of a 512-entry window and send an operator to
                // the wrong entry. The command chain does not do that — its
                // own test says the corrupt entry "names itself, not the
                // 512-entry window it sat in" — and a second artifact with a
                // worse diagnostic is a second artifact nobody can act on.
                //
                // So the window is re-read one row at a time to find it.
                // Only ever on the failure path, so the green case pays
                // nothing, and bounded by the window.
                Err(_) => {
                    return Err(TamperAt {
                        space: SpaceId(scope.key().0.clone()),
                        index: base_index + from + self.first_unreadable(scope, from).await,
                        kind: TamperKind::Unreadable,
                    })
                }
            };
            if batch.is_empty() {
                return Ok(head);
            }
            ordo_secevent::verify_stream_from(scope, &batch, expected, base_index + from)?;
            expected = batch.last().expect("non-empty").hash;
            from += batch.len();
            head = Some(((base_index + from) as u64, expected));
            if batch.len() < VERIFY_BATCH {
                return Ok(head);
            }
        }
    }

    /// The offset within the window starting at `from` of the first row
    /// that cannot be read, for [`Self::try_verify_from`]'s diagnostic.
    ///
    /// Zero when every row reads — which means the batch failed for a
    /// reason that is not one row, a transient fault being the likely one,
    /// and the window's own start is then the honest answer rather than a
    /// position picked to look precise.
    async fn first_unreadable(&self, scope: &Scope, from: usize) -> usize {
        for offset in 0..VERIFY_BATCH {
            match self.try_entries(scope, from + offset, 1).await {
                Ok(rows) if rows.is_empty() => return 0,
                Ok(_) => continue,
                Err(_) => return offset,
            }
        }
        0
    }

    fn block<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        self.handle.block_on(fut)
    }
}

// ── the audit: what a verification of this stream actually checked ───────

/// What verifying one scope's security stream found.
///
/// A struct rather than a `Result<(), _>` because *what was checked* is
/// half the answer and a bare verdict hides it. A stream that recomputes
/// from genesis and a stream whose retained tail recomputes from a signed
/// marker are different claims, and only one of them covers last month.
/// `ordo verify` prints these fields; nothing about them is decoration.
#[derive(Debug, Clone)]
pub struct StreamAudit {
    pub scope: Scope,
    /// The earliest full-stream position still recomputable — zero for a
    /// stream that has never been pruned. `ordo_secevent::verifiable_from`.
    pub verifiable_from: u64,
    /// One past the last stored position, in full-stream terms.
    pub end: u64,
    /// How many entries this verification actually recomputed.
    pub checked: u64,
    /// The checkpoint position the anchor came from, or `None` for genesis.
    pub anchored_at: Option<u64>,
    /// The key id that signed the anchoring or truncation-detecting
    /// checkpoint, when one was used.
    pub key_id: Option<String>,
    /// The verified head: its full-stream index and hash.
    pub head: Option<(u64, ChainHash)>,
    /// The highest stored checkpoint position, whether or not it was used.
    pub latest_checkpoint: Option<u64>,
    /// Set when the stream did not verify. The audit is still returned, so
    /// a caller reports what it managed to establish alongside the fault.
    pub failure: Option<String>,
    /// The key id of a stored marker this run **could not read**, because no
    /// supplied key claims it.
    ///
    /// Deliberately not a [`Self::failure`]. A marker signed by a key you did
    /// not bring is unreadable, and a stream whose end nobody could check is
    /// a *narrower answer*, not a bad one — which is exactly the distinction
    /// a key rotation needs and the one whose absence made `ordo verify`
    /// report tampering the day after a rotation.
    pub unreadable_marker: Option<String>,
}

impl StreamAudit {
    pub fn verified(&self) -> bool {
        self.failure.is_none()
    }
}

/// Verify one scope's security stream, and say what the verification
/// covered.
///
/// # Why this exists at all
///
/// `ordo_secevent` has had `verify_stream`, `verify_stream_from`, a
/// hash-chained stream, signed checkpoints under a **separate** key with a
/// separate domain tag, and a prune that refuses an unverified cut — and
/// until now **nothing ever called any of it**. The argument for the second
/// key, in `store.rs`'s own words, is that one key would mean *"compromising
/// the key that signs routine domain attestations is also the ability to
/// forge the record of the compromise"*. That argument only pays out if
/// something checks. This is the something.
///
/// # The four checks, and which of them a chain walk cannot do
///
/// 1. **Nothing was pruned to nothing.** A stream with a base above zero and
///    no stored entries is not "empty"; `prune_to_checkpoint` refuses to
///    leave a stream with no retained tail, so that state means entries left
///    by some other route.
/// 2. **The anchor is real.** A never-pruned stream anchors at genesis. A
///    pruned one anchors only at a checkpoint whose position is exactly its
///    base and whose signature verifies under the published key — otherwise
///    the retained tail is tied to nothing and the honest answer is that it
///    cannot be verified, not that it passed.
/// 3. **The stored suffix recomputes** from that anchor.
/// 4. **Nothing was removed from the end.** This is the one a chain walk
///    cannot see: deleting the last N entries leaves a perfectly
///    self-consistent chain. Only a signed marker at a higher position
///    catches it, so the newest stored checkpoint is verified and its
///    position compared against the stream's end, and the entry it names is
///    required to carry its head.
///
/// A `None` verifier is allowed and narrows the answer rather than widening
/// it: check 3 still runs, checks 2 and 4 are reported as not performed, and
/// a pruned stream fails because it has no anchor it is entitled to trust.
pub async fn audit_stream(
    store: &PgSecurityStore,
    scope: &Scope,
    verifiers: &[ordo_commandlog::CheckpointVerifier],
) -> Result<StreamAudit, PoolError> {
    // **A set of keys, selected by the marker's own `key_id`** (ruled
    // 2026-09-26, `secevent-key-rotation`).
    //
    // One key was the wrong shape for a rotation, and the failure was not
    // cosmetic: check 4 verifies the *newest* checkpoint, so after a rotation
    // every marker signed under the old key failed under the new one and the
    // stream reported **FAILED** — not "unverifiable", not "anchored
    // earlier", but the word this artifact uses for tampering. A deployment
    // rotates its security key because it suspects a compromise, so the
    // moment the record most needs to be readable was the moment it cried
    // wolf, and an operator who had just rotated could not tell that finding
    // apart from the real thing.
    //
    // Selecting by id rather than trying each in turn keeps
    // `StreamCheckpoint::verify`'s binding intact: a marker is checked
    // against the key it *claims*, so "some key we hold verifies it" is never
    // mistaken for "the key it names verifies it".
    //
    // Deliberately **not** solved by re-signing old markers at rotation.
    // That would make the new key retroactive authority over the old record,
    // which is exactly the property the second key exists to deny — see this
    // module's `store.rs` on why the security stream is separately keyed at
    // all.
    let pick = |cp: &StreamCheckpoint| {
        verifiers
            .iter()
            .find(|v| v.key_id() == cp.key_id)
            .map(|v| (v, cp.verify(v)))
    };
    let base = store.try_base_index(scope).await? as u64;
    let len = store.try_len(scope).await? as u64;
    let end = base + len;
    let latest = store.try_latest_checkpoint(scope).await?;
    let mut audit = StreamAudit {
        scope: scope.clone(),
        verifiable_from: base,
        end,
        checked: len,
        anchored_at: None,
        key_id: None,
        head: None,
        latest_checkpoint: latest.as_ref().map(|c| c.upto),
        failure: None,
        unreadable_marker: None,
    };

    // Check 1.
    if len == 0 {
        if base > 0 {
            audit.failure = Some(format!(
                "the stream is empty and its base is {base}, so {base} entries were removed \
                 without leaving the retained tail a prune is required to leave"
            ));
        }
        return Ok(audit);
    }

    // Check 2.
    let anchor = if base == 0 {
        ChainHash::GENESIS
    } else {
        if verifiers.is_empty() {
            audit.failure = Some(format!(
                "this stream was pruned to position {base} and no verifying key was supplied, \
                 so its retained tail is anchored to nothing that can be checked"
            ));
            return Ok(audit);
        }
        match store.try_checkpoint_at_or_below(scope, base).await? {
            Some(cp) if cp.upto == base && matches!(pick(&cp), Some((_, true))) => {
                audit.anchored_at = Some(cp.upto);
                audit.key_id = Some(cp.key_id.clone());
                cp.head
            }
            Some(cp) if cp.upto == base => {
                // Two different failures, and telling them apart is the
                // whole of the rotation fix: a marker signed by a key we do
                // not hold is *unreadable*, and a marker that does not verify
                // under the key it names is *wrong*. Reporting the first as
                // the second is what made a rotation look like tampering.
                audit.failure = Some(match pick(&cp) {
                    None => format!(
                        "the checkpoint at position {base} is signed by key `{}`, which was \
                         not supplied — so this stream cannot be checked, which is not the \
                         same as failing. Pass that key as well",
                        cp.key_id
                    ),
                    Some(_) => format!(
                        "the checkpoint at position {base} does not verify under the key it \
                         names (`{}`), so the anchor for this stream's retained tail is a \
                         claim rather than a signature",
                        cp.key_id
                    ),
                });
                return Ok(audit);
            }
            other => {
                audit.failure = Some(match other {
                    Some(cp) => format!(
                        "this stream was pruned to position {base} and the nearest checkpoint \
                         at or below it is at {}, so nothing signs the history that was cut",
                        cp.upto
                    ),
                    None => format!(
                        "this stream was pruned to position {base} and no checkpoint covers \
                         the history that was cut"
                    ),
                });
                return Ok(audit);
            }
        }
    };

    // Check 3.
    if let Err(t) = store.try_verify_from(scope, anchor, base as usize).await {
        audit.failure = Some(describe_tamper(&t));
        return Ok(audit);
    }
    // The head, read after the walk rather than before: it is the value the
    // walk arrived at, and reading it first would report a head that a
    // failure later in the stream had already disproved.
    audit.head = store
        .try_head(scope)
        .await?
        .map(|hash| (end.saturating_sub(1), hash));

    // Check 4.
    if let Some(cp) = &latest {
        match pick(cp) {
            // Signed by a key we do not hold. Not a failure — the marker is
            // simply unreadable, and saying otherwise is what turned a
            // rotation into a tampering report. The caveat the caller sees
            // is the keyless one: the end of the stream is unchecked.
            None if verifiers.is_empty() => return Ok(audit),
            None => {
                audit.unreadable_marker = Some(cp.key_id.clone());
                return Ok(audit);
            }
            Some((_, false)) => {
                audit.failure = Some(format!(
                    "the newest checkpoint, at position {}, does not verify under the key it \
                     names (`{}`)",
                    cp.upto, cp.key_id
                ));
                return Ok(audit);
            }
            Some((_, true)) => {}
        }
        audit.key_id = Some(cp.key_id.clone());
        if cp.upto > end {
            audit.failure = Some(format!(
                "a signed checkpoint covers {} entries and only {end} are stored, so {} \
                 entries were removed from the end of the stream",
                cp.upto,
                cp.upto - end
            ));
            return Ok(audit);
        }
        // The entry the checkpoint names must carry the head it claims.
        // Only meaningful while that entry is still retained.
        if cp.upto > base {
            let at = (cp.upto - 1 - base) as usize;
            match store.try_entries(scope, at, 1).await?.first() {
                Some(e) if e.hash == cp.head => {}
                Some(_) => {
                    audit.failure = Some(format!(
                        "the checkpoint at position {} names a head the stored entry there \
                         does not have, so the marker and the stream describe two histories",
                        cp.upto
                    ));
                    return Ok(audit);
                }
                None => {
                    audit.failure = Some(format!(
                        "the entry at position {} that the newest checkpoint names could not \
                         be read",
                        cp.upto - 1
                    ));
                    return Ok(audit);
                }
            }
        }
    }

    Ok(audit)
}

/// A `TamperAt` in the words an operator reads, matching the command
/// chain's vocabulary rather than inventing a second one.
fn describe_tamper(t: &TamperAt) -> String {
    let what = match t.kind {
        TamperKind::HashMismatch => "its recorded hash is not the hash of its contents",
        TamperKind::BrokenLink => "it does not follow the entry before it",
        TamperKind::Unreadable => "it could not be read",
    };
    format!("entry {} of {}: {what}", t.index, t.space.0)
}

// ── the checkpoint: the marker that makes check 4 reachable ──────────────

/// The schedule row the security stream's **checkpoint** pass claims.
///
/// Its own row, beside [`SECEVENT_RETENTION_SCHEDULE`], and the separation
/// is the whole point of this section. Until it existed, the only thing that
/// ever signed a stream checkpoint was step 1 of [`retain_scope`], so the
/// marker that makes [`audit_stream`]'s fourth check reachable was signed on
/// the **retention** cadence and only when retention was switched on. Both
/// halves of that are wrong, and `crate::cadence`'s module doc is the
/// argument, written for the command chain and applying here word for word:
///
/// 1. **The periods answer different questions.** Retention's period is
///    "how often do we sweep aged-out evidence", and six hours is a fine
///    answer. A checkpoint's period is "how long may entries sit at the end
///    of the stream where nothing signed knows they exist" — the window in
///    which a deleted tail is undetectable. That is the *attestation*
///    argument, not the checkpoint one: `cadence`'s doc calls a month of
///    unattested history "a long time for a rewrite to sit undetected", and
///    this artifact has no separate attestation series to close the gap,
///    because it has no wire surface at all. So the marker carries both
///    jobs and takes the tighter of the two cadences.
/// 2. **The enabling conditions differ.** `DrainConfig::retain = false` is
///    an ordinary retention choice — keep everything — and it must not
///    switch off truncation detection. A deployment that keeps every denial
///    forever is *more* interested in whether one was removed, not less.
///
/// Two rows, two periods, two claims. `claim_schedule` and never
/// `claim_due`, for the reason [`SECEVENT_RETENTION_SCHEDULE`] gives.
pub const SECEVENT_CHECKPOINT_SCHEDULE: &str = "secevent.checkpoint";

/// How often a stream checkpoint is signed, by default: one hour.
///
/// The same number as `LogPolicy::DEFAULT.attest_or` and the same reasoning,
/// stated rather than borrowed. This is the bound on how long new security
/// history may sit with nothing signed knowing it exists, so it is the
/// bound on how long a deleted tail can go undetected. It is deliberately
/// **not** `LogPolicy::DEFAULT.checkpoint` (a month): a month is right for
/// the command chain's heavyweight anchor because attestations arrive
/// hourly beside it and close the detection gap, and this stream has no
/// second artifact to lean on.
///
/// The cost of a quiet hour is nothing, because [`checkpoint_scope`]
/// reports [`StreamCheckpointed::AlreadyAt`] when the head has not moved
/// and writes no row. A stream nobody wrote to costs one position read and
/// one checkpoint read per hour.
pub const DEFAULT_CHECKPOINT_PERIOD: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Register the checkpoint schedule. `ON CONFLICT DO NOTHING`, so a replica
/// joining an existing deployment does not reset a cycle in progress.
pub async fn register_checkpoint_schedule(
    pool: &crate::Pool,
    period: std::time::Duration,
    now: i64,
) -> Result<(), PoolError> {
    let tx = pool.begin_for(crate::OpClass::AddressedWrite).await?;
    tx.schedule(SECEVENT_CHECKPOINT_SCHEDULE, now + period.as_secs() as i64)
        .await?;
    tx.commit().await
}

/// Claim the checkpoint schedule if it is due, advancing it by its **own**
/// period.
pub async fn claim_checkpoint_tick(
    pool: &crate::Pool,
    period: std::time::Duration,
    now: i64,
) -> Result<bool, PoolError> {
    let tx = pool.begin_for(crate::OpClass::AddressedWrite).await?;
    let claimed = tx
        .claim_schedule(SECEVENT_CHECKPOINT_SCHEDULE, now, period.as_secs() as i64)
        .await?;
    tx.commit().await?;
    Ok(claimed)
}

/// What one scope's checkpoint pass did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamCheckpointed {
    /// A new marker was signed and stored.
    Signed(StreamCheckpoint),
    /// The head is already signed at this position, by a marker this
    /// deployment's key verifies and whose head storage agrees with.
    ///
    /// The ordinary outcome of a quiet period, and the reason a heartbeat
    /// cadence costs nothing. `$seccheckpoints`'s `PRIMARY KEY (scope,
    /// upto)` would have turned an unconditional re-sign into a silent
    /// `ON CONFLICT DO NOTHING` — the row would be right and the write
    /// would be a lie about having happened — so the decision is made here,
    /// where it can be reported, rather than left to the index.
    AlreadyAt(u64),
    /// The scope has no stored entries, so there is no head to sign.
    /// `ordo_secevent::checkpoint`'s `None`, named.
    EmptyStream,
    /// Nothing was signed, and this is why. Never an `Err`: a refusal is a
    /// finding about the stream, and a caller that treated it as a
    /// transport fault would retry it forever.
    Refused(CheckpointRefusal),
}

/// Why a checkpoint pass signed nothing.
///
/// Every variant is a state in which signing would make things **worse**
/// rather than merely not better, which is why they are refusals and not
/// warnings. `store.rs`'s discipline, restated: a marker that verifies but
/// does not match the stored chain means marker and stream describe two
/// histories, and the answer to that is to refuse, loudly.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CheckpointRefusal {
    /// The stored suffix does not recompute. Signing its head would mint an
    /// anchor over tampered history — and because a checkpoint is the only
    /// lawful cut point, the *next* retention pass could then prune the
    /// doctored prefix away and leave a stream that verifies perfectly
    /// under a signature the deployment itself produced. Retention would
    /// have laundered the tamper. `undo::CheckpointFailure`'s own
    /// `WouldAnchorTamperedHistory`, for the same reason and with the same
    /// name.
    WouldAnchorTamperedHistory(TamperAt),
    /// The stream was pruned to `base` and no verified checkpoint sits
    /// exactly there, so the retained tail is anchored to nothing and a new
    /// marker over it would sign a suffix whose start nothing vouches for.
    /// [`audit_stream`]'s check 2 fails this stream; a signer that carried
    /// on would be handing the verifier a second marker over a history it
    /// has already declined to believe.
    Unanchored { base: u64, nearest: Option<u64> },
    /// A signed marker covers `signed` entries and only `stored` are there.
    /// The stream has already been truncated, [`audit_stream`]'s check 4
    /// says so, and adding a marker at the shorter position would be this
    /// deployment putting its own signature on the shortened history.
    TailRemoved { signed: u64, stored: u64 },
    /// `$seccheckpoints` already holds a row at the position this pass
    /// would sign, and it does not verify under this deployment's own key.
    ///
    /// The row wins: `put_security_checkpoint` is `ON CONFLICT DO NOTHING`
    /// on `(scope, upto)`, so an insert here would return success and store
    /// nothing. A hand-written row would therefore be able to **suppress**
    /// checkpointing at its position indefinitely, silently. Refusing is
    /// the only answer that surfaces it, and it agrees with the verifier:
    /// [`audit_stream`] already fails a stream whose newest checkpoint does
    /// not verify.
    ForeignMarker { upto: u64, key_id: String },
    /// A verifying row sits at this position with a different head. Two
    /// histories, and the loudest thing this module can say.
    TwoHistories {
        upto: u64,
        signed: ChainHash,
        stored: ChainHash,
    },
}

impl std::fmt::Display for CheckpointRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointRefusal::WouldAnchorTamperedHistory(t) => write!(
                f,
                "refusing to sign: this stream does not recompute — {}. A checkpoint is the \
                 only lawful cut point, so signing here would let retention destroy the \
                 evidence of the tamper and leave a stream that verifies",
                describe_tamper(t)
            ),
            CheckpointRefusal::Unanchored { base, nearest } => write!(
                f,
                "refusing to sign: this stream was pruned to position {base} and {}, so its \
                 retained tail is anchored to nothing a new marker could extend",
                match nearest {
                    Some(at) =>
                        format!("the nearest verified checkpoint at or below it is at {at}"),
                    None => "no verified checkpoint covers the history that was cut".to_string(),
                }
            ),
            CheckpointRefusal::TailRemoved { signed, stored } => write!(
                f,
                "refusing to sign: a signed checkpoint covers {signed} entries and only \
                 {stored} are stored, so {} were removed from the end of the stream — this \
                 deployment will not add its signature to the shortened history",
                signed - stored
            ),
            CheckpointRefusal::ForeignMarker { upto, key_id } => write!(
                f,
                "refusing to sign: `$seccheckpoints` already holds a row at position {upto} \
                 signed by `{key_id}`, which does not verify under this deployment's security \
                 key. The position is taken, so a real marker there could not be stored and \
                 the write would report a success it did not perform"
            ),
            CheckpointRefusal::TwoHistories {
                upto,
                signed,
                stored,
            } => write!(
                f,
                "refusing to sign: a verified checkpoint at position {upto} names head \
                 {signed} and the entry stored there has {stored} — the marker and the \
                 stream describe two histories"
            ),
        }
    }
}

/// Sign a marker at one scope's current head, under the **security**
/// signer, having first verified that the head is one this stream actually
/// arrives at.
///
/// # Verify, then sign, and the pair is the walk's own
///
/// `undo::sign_checkpoint` is the precedent and it refuses to anchor history
/// that does not verify (`CheckpointFailure::WouldAnchorTamperedHistory`).
/// Step 1 of [`retain_scope`] — the only thing that has ever signed a marker
/// on this artifact — did not: it read the head and signed it. That was the
/// more dangerous omission of the two, because retention then *uses* the
/// marker as a cut point, so an unverified signature at the head is a
/// licence to delete the prefix that proves the stream was edited.
///
/// So this walks the stream first, and signs the `(upto, head)` the walk
/// itself arrived at rather than a head read separately afterwards. See
/// [`PgSecurityStore::try_verify_to_head`] for why that is both stronger
/// than taking the append lock and cheaper.
///
/// # Every no-op is named
///
/// An empty stream, an unmoved head and each way the stream can already be
/// in a state a signature would make worse are distinct answers rather than
/// one quiet `Ok(())`. A signer that cannot tell "nothing to do" from "I
/// refuse" is a signer whose silence an operator cannot read.
pub async fn checkpoint_scope(
    store: &PgSecurityStore,
    scope: &Scope,
    signer: &ordo_commandlog::Ed25519Signer,
    at: Timestamp,
) -> Result<StreamCheckpointed, PoolError> {
    // The signer's own public half. A checkpointer checks the markers
    // already on the stream without any extra configuration, exactly as
    // `undo::sign_checkpoint` does and for its stated reason: "a marker it
    // cannot check is not used as an anchor". A third party supplies
    // `ORDO_SECURITY_CHECKPOINT_PUBKEY` and never reaches this path.
    let verifier = signer.verifier();
    let base = store.try_base_index(scope).await?;

    // The anchor, on [`audit_stream`]'s check-2 rules. A never-pruned
    // stream anchors at genesis; a pruned one only at a verified marker
    // sitting exactly at its base.
    let anchor = if base == 0 {
        ChainHash::GENESIS
    } else {
        match store.try_checkpoint_at_or_below(scope, base as u64).await? {
            Some(cp) if cp.upto == base as u64 && cp.verify(&verifier) => cp.head,
            other => {
                return Ok(StreamCheckpointed::Refused(CheckpointRefusal::Unanchored {
                    base: base as u64,
                    nearest: other.map(|c| c.upto),
                }))
            }
        }
    };

    let (upto, head) = match store.try_verify_to_head(scope, anchor, base).await {
        Ok(Some(head)) => head,
        Ok(None) => return Ok(StreamCheckpointed::EmptyStream),
        Err(t) => {
            return Ok(StreamCheckpointed::Refused(
                CheckpointRefusal::WouldAnchorTamperedHistory(t),
            ))
        }
    };

    // What is already signed. Three different answers live here, and
    // collapsing any two of them loses the one that matters.
    if let Some(last) = store.try_latest_checkpoint(scope).await? {
        if last.upto > upto && last.verify(&verifier) {
            return Ok(StreamCheckpointed::Refused(
                CheckpointRefusal::TailRemoved {
                    signed: last.upto,
                    stored: upto,
                },
            ));
        }
        if last.upto == upto {
            if !last.verify(&verifier) {
                return Ok(StreamCheckpointed::Refused(
                    CheckpointRefusal::ForeignMarker {
                        upto,
                        key_id: last.key_id,
                    },
                ));
            }
            if last.head != head {
                return Ok(StreamCheckpointed::Refused(
                    CheckpointRefusal::TwoHistories {
                        upto,
                        signed: last.head,
                        stored: head,
                    },
                ));
            }
            return Ok(StreamCheckpointed::AlreadyAt(upto));
        }
    }

    // One definition of what a stream checkpoint's bytes are, shared with
    // the synchronous `ordo_secevent::checkpoint`. See `checkpoint_at`.
    let cp = ordo_secevent::checkpoint_at(scope, upto, head, signer, at);
    let tx = store.pool.begin_for(crate::OpClass::AddressedWrite).await?;
    tx.put_security_checkpoint(&cp).await?;
    tx.commit().await?;
    Ok(StreamCheckpointed::Signed(cp))
}

// ── retention: the schedule, and the tick that runs it ───────────────────

/// The schedule row the security stream's retention pass claims.
///
/// Its **own** name, beside `commandlog.checkpoint` and `commandlog.attest`,
/// and for the reason [`crate::cadence`] spends a module doc on: two
/// artifacts whose retentions are set by different policies must not reach
/// the schedule table through one row, or the one that loses a `claim_due`
/// race adopts the other's period. `security-logging-design.md` §6.1 is
/// explicit that "denial retention is set independently of command-history
/// retention"; this constant is the machine-readable half of that sentence.
pub const SECEVENT_RETENTION_SCHEDULE: &str = "secevent.retention";

/// Register the retention schedule. `ON CONFLICT DO NOTHING`, so a replica
/// joining an existing deployment does not reset a cycle in progress.
pub async fn register_retention_schedule(
    pool: &crate::Pool,
    period: std::time::Duration,
    now: i64,
) -> Result<(), PoolError> {
    let tx = pool.begin_for(crate::OpClass::AddressedWrite).await?;
    tx.schedule(SECEVENT_RETENTION_SCHEDULE, now + period.as_secs() as i64)
        .await?;
    tx.commit().await
}

/// Claim the retention schedule if it is due, advancing it by its **own**
/// period.
///
/// `claim_schedule` and not `claim_due`, deliberately — see
/// [`SECEVENT_RETENTION_SCHEDULE`].
pub async fn claim_retention_tick(
    pool: &crate::Pool,
    period: std::time::Duration,
    now: i64,
) -> Result<bool, PoolError> {
    let tx = pool.begin_for(crate::OpClass::AddressedWrite).await?;
    let claimed = tx
        .claim_schedule(SECEVENT_RETENTION_SCHEDULE, now, period.as_secs() as i64)
        .await?;
    tx.commit().await?;
    Ok(claimed)
}

/// What one scope's retention pass did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retained {
    pub scope: Scope,
    /// A checkpoint was signed at this position, if the head had moved.
    pub checkpointed: Option<u64>,
    /// Entries removed.
    pub pruned: usize,
    /// The earliest position the stream can still be recomputed from —
    /// `ordo_secevent::verifiable_from` after the cut.
    pub verifiable_from: u64,
    /// Why nothing was pruned, when nothing was. Present rather than
    /// implied: "the policy found nothing old enough" and "the cut point
    /// was refused" are different facts and an operator has to be able to
    /// tell them apart.
    pub refused: Option<String>,
}

/// One scope's retention pass: checkpoint the head, then cut the prefix the
/// policy allows.
///
/// The order is the argument. A checkpoint is the **only legal cut point**
/// — verification resumes from one and a suffix that begins mid-chain is
/// unanchored — so signing first is what makes today's tail prunable in
/// ninety days. Signing *after* pruning would anchor a stream at a position
/// the cut had already passed.
///
/// The cut itself goes through `ordo_secevent::prune_to_checkpoint`, which
/// is the only path that verifies the anchor, matches its head against
/// storage, and leaves the tail standing.
///
/// # Step 1 is [`checkpoint_scope`], not its own signing
///
/// It used to be its own: it read the head and signed it. That is the one
/// place in this module where a missing check was *load-bearing* rather than
/// merely thin, because retention then treats the marker it just wrote as a
/// lawful cut point. Read the head, sign it unverified, prune the prefix
/// underneath it, and a doctored stream comes out the far side verifying
/// green under the deployment's own signature — with the evidence of the
/// edit deleted by retention, on schedule, as designed. So the signing moved
/// to [`checkpoint_scope`], which verifies first, and a refusal there stops
/// this pass **before** it looks for a cut point at all.
pub async fn retain_scope(
    store: &PgSecurityStore,
    scope: &Scope,
    signer: &ordo_commandlog::Ed25519Signer,
    verifier: &ordo_commandlog::CheckpointVerifier,
    retention: ordo_secevent::Retention,
    now: ordo_commandlog::Timestamp,
) -> Result<Retained, PoolError> {
    let base = store.try_base_index(scope).await?;
    let len = store.try_len(scope).await?;
    let end = (base + len) as u64;

    // 1 · Sign the head, unless a checkpoint already covers it — and never
    //     over history that does not recompute.
    let checkpointed = match checkpoint_scope(store, scope, signer, now).await? {
        StreamCheckpointed::Signed(cp) => Some(cp.upto),
        StreamCheckpointed::AlreadyAt(_) | StreamCheckpointed::EmptyStream => None,
        // **Nothing is pruned on a stream the signer refused.** Not even at
        // an older checkpoint that would pass `check_prune`: `check_prune`
        // matches the cut point's head against the entry stored there, so
        // a tamper anywhere *else* in the doomed prefix would sail through
        // it, and the pass would destroy the only surviving evidence of an
        // edit this deployment has already detected. A refusal to anchor is
        // a refusal to cut.
        StreamCheckpointed::Refused(why) => {
            return Ok(Retained {
                scope: scope.clone(),
                checkpointed: None,
                pruned: 0,
                verifiable_from: base as u64,
                refused: Some(why.to_string()),
            })
        }
    };

    // 2 · The policy's candidate, then the greatest *checkpoint* at or below
    //     it. A checkpoint is not a competing lower bound — it is the only
    //     legal cut point, the same rule `ordo_exec::prune` states for the
    //     command chain.
    let candidate = store.try_prunable_upto(scope, retention, now).await?;
    let refuse = |why: String| Retained {
        scope: scope.clone(),
        checkpointed,
        pruned: 0,
        verifiable_from: base as u64,
        refused: Some(why),
    };
    let Some(cp) = store.try_checkpoint_at_or_below(scope, candidate).await? else {
        return Ok(refuse(format!(
            "no signed checkpoint at or below the policy's cut point {candidate} — \
             nothing is prunable until one exists"
        )));
    };

    // The rules are `ordo_secevent::check_prune`'s, not a second copy of
    // them: the verified signature, the position inside the stored range,
    // the retained tail, and the head that storage actually holds.
    let upto = cp.upto as usize;
    let stored_head = if upto > base && upto <= end as usize {
        store
            .try_entries(scope, upto - base - 1, 1)
            .await?
            .first()
            .map(|e| e.hash)
    } else {
        None
    };
    if let Err(e) = ordo_secevent::check_prune(&cp, verifier, base, end as usize, stored_head) {
        return Ok(refuse(e.to_string()));
    }
    store.try_prune_before(scope, upto).await?;

    let after = store.try_base_index(scope).await?;
    Ok(Retained {
        scope: scope.clone(),
        checkpointed,
        pruned: after.saturating_sub(base),
        verifiable_from: after as u64,
        refused: None,
    })
}

impl SecurityStore for PgSecurityStore {
    fn append(&mut self, entry: StreamEntry) -> Result<ChainedEntry, StreamError> {
        self.block(self.try_append(&entry))
    }

    fn len(&self, scope: &Scope) -> usize {
        self.block(self.try_len(scope)).unwrap_or(0)
    }

    fn base_index(&self, scope: &Scope) -> usize {
        self.block(self.try_base_index(scope)).unwrap_or(0)
    }

    fn entries(&self, scope: &Scope, from: usize, limit: usize) -> Vec<ChainedEntry> {
        self.block(self.try_entries(scope, from, limit))
            .unwrap_or_default()
    }

    fn head(&self, scope: &Scope) -> Option<ChainHash> {
        self.block(self.try_head(scope)).ok().flatten()
    }

    fn prune_before(&mut self, scope: &Scope, upto: usize) -> Result<usize, StreamError> {
        self.block(self.try_prune_before(scope, upto))
            .map_err(store_err)
    }

    /// Streamed like the trait's default — but a read that *fails* is a
    /// verification failure, not an empty stream. A green verification of a
    /// record nobody could read is the worst answer this function can give.
    fn verify_from(
        &self,
        scope: &Scope,
        anchor: ChainHash,
        base_index: usize,
    ) -> Result<(), TamperAt> {
        self.block(self.try_verify_from(scope, anchor, base_index))
    }
}
