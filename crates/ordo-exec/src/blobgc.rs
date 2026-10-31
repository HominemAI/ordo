//! Reference GC for blobs, floored at the undo horizon.
//!
//! # The rule, stated before the mechanism
//!
//! **Bytes outlive the last reference until the horizon passes, because a
//! reverted delete must find its bytes.**
//!
//! A row deleted a minute ago is revertible for `LogPolicy::undo_horizon`
//! — ninety days by default — and the revert is an inverse `Create` that
//! re-`INSERT`s the row from its before-image. If a sweep deleted the
//! object in between, the revert *succeeds* and restores a row pointing at
//! nothing: a dangling reference the shape gate cannot see, produced by
//! the one mechanism whose whole job is to make deletes reversible.
//!
//! So blob GC's floor is [`crate::prune::Floor`] — the same value, from
//! the same function, reported with the same [`crate::prune::FloorHolder`].
//! Not a second horizon and not a `BlobPolicy` number. When a lagging
//! mirror consumer holds the log's floor it holds the blob sweep's floor
//! too, and the operator sees one name for one cause.
//!
//! # The ordering is the inverse of the payload case
//!
//! `$sealedpayload` collects **inside** the pruning transaction, because
//! deleting the log first would leave payloads nothing can name. Bytes in
//! a bucket cannot join that transaction, so for blobs the safe order
//! inverts: **delete the reference last.**
//!
//! 1. **Phase 1 — transactional, Postgres only.** The anti-join finds
//!    handles no live row names and inserts a tombstone. Nothing in the
//!    bucket is touched, so a crash here costs nothing.
//! 2. **Phase 2 — out of transaction, idempotent.** For each tombstone
//!    below the floor, re-check the anti-join under the row lock, delete
//!    the object, stamp `deleted_at`, then remove the `$blobref` row. A
//!    crash between the object delete and the stamp leaves the tombstone
//!    open and the sweep retries; bucket deletes are idempotent, which is
//!    a property the store contract asserts of every binding.
//! 3. **A revert cancels a tombstone rather than racing it** — the
//!    re-check in phase 2 is what makes that true without a lock across
//!    two systems.
//!
//! The asymmetry is deliberate and worth stating: this order can leak an
//! object, which costs money; the other order can break a revert, which
//! costs a guarantee.
//!
//! # The retained-image half, and why it is not a second query
//!
//! `plan-blob.md` §5.1 names two sources for the reference set: live rows,
//! and **retained command images** — every image naming a handle, which
//! for a sealed field means every `$sealedpayload` row whose plaintext
//! *is* a blob reference. That second source cannot be queried without
//! opening payloads, which would make garbage collection a key-using
//! operation.
//!
//! It does not have to be. **The floor comparison already subsumes it**,
//! given phase 1's timestamp:
//!
//! - Phase 1 stamps `unreferenced_since` at the moment no live row names
//!   the handle any more.
//! - Any command image that could restore the handle is at or before that
//!   instant — an image is written when the reference is written or
//!   removed, never after.
//! - Phase 2 deletes only when the floor has passed `unreferenced_since`,
//!   and the floor is the position below which every group is prunable.
//!
//! So by the time an object is deleted, every image that could name it is
//! already prunable, and the ones that were pruned took their payloads
//! with them in the pruning transaction. Enumerating the images would be a
//! second, weaker statement of the same guarantee — weaker because it
//! would need keys, and a payload whose epoch was destroyed cannot be
//! opened at all.
//!
//! What keeps it airtight is the re-check: if a revert restores the row
//! before the floor passes, phase 2 finds the live reference and cancels
//! the tombstone.
//!
//! # The second kind of tombstone, and what it does to the argument above
//!
//! Phase 1 tombstones handles it reads **out of** `$blobref`, so its rows
//! always have a reference row beside them. The blob door writes a second
//! kind: an **intent**, one row for the handle it is about to `put`,
//! written before a byte reaches the store
//! ([`ordo_crypto::blobref::tomb_declare_sql`]). It exists because the
//! object is written before the transaction that records it, and an upload
//! interrupted between the two leaves an object this module's anti-join can
//! never see. `BlobStore` has no `list`, so that object was unreachable
//! forever rather than merely uncollected.
//!
//! **Phase 2 needs no branch for it, and gets none.** An intent has no
//! `$blobref` row, so the re-check's `SELECT … FROM $blobref … EXISTS(live)`
//! comes back empty, the object is deleted, and the `DELETE` of the
//! reference row removes nothing because there never was one. "A
//! `$blobtomb` row with no `$blobref` row" already means "an upload that
//! never committed", with no column saying so.
//!
//! **The floor argument holds, and it holds for a different reason than it
//! does for a phase-1 tombstone.** The retained-image argument above turns
//! on *"any command image that could restore the handle is at or before
//! `unreferenced_since`"*, and for an intent that sentence is plainly false:
//! the intent is stamped before the upload's own converge group is
//! appended. What rescues it is that an intent only ever survives when no
//! group was appended at all. The group and the `$blobref` row commit in
//! one transaction. So either both landed, and then the intent did not,
//! because the reference insert cancels it in the same statement; or
//! neither did, and no retained image anywhere names the handle, and no
//! revert can restore it. An intent that is still standing is a handle
//! nothing can ever refer to, which makes the floor pure conservatism
//! rather than a load-bearing bound. It is applied anyway, because one rule
//! is worth more than ninety days of abandoned bytes.
//!
//! The dangerous case is the one that statement rules out: an intent left
//! standing **beside** a committed reference. `unreferenced_since` would
//! then be the instant the upload started, and a delete of that row two
//! years later would be priced against it. The tombstone would already be
//! past the floor, and the bytes would go while the delete was still
//! revertible. That is why the cancel is a CTE on the reference insert and
//! not a statement after it: the two states cannot both exist, so there is
//! no window to reason about and none to crash inside.
//!
//! # `blob inline` needs none of this
//!
//! Its bytes are a `$sealedpayload` row, so
//! [`crate::sealed::collect_pruned_in`] already collects them, inside the
//! pruning transaction, with no tombstone and no second horizon.
//! `$blobtomb` never learns that inline blobs exist. That is the strongest
//! argument for keeping `inline` a first-class tier rather than treating
//! it as an optimisation of the external path.

use std::sync::Arc;

use ordo_blob::{BlobError, StoreResolver};
use ordo_crypto::blobref::{BLOB_REF_TABLE, BLOB_SPACE_COLUMN, BLOB_TOMB_TABLE};
use ordo_crypto::custody::{BlobHandle, SpaceId};
use ordo_schema::Schema;

use crate::bucket::{Attachment, BucketCustody};
use crate::cadence::LogPolicy;
use crate::prune::{Floor, FloorHolder};
use crate::{sql_lit, OpClass, Pool, PoolError};

/// The command log spells its space id in its own type: `ordo-commandlog`
/// is a pure crate and cannot see `ordo-crypto`'s. One conversion, here,
/// rather than a `From` impl bridging two crates that deliberately do not
/// know about each other.
fn log_space(space: &SpaceId) -> ordo_commandlog::SpaceId {
    ordo_commandlog::SpaceId(space.0.clone())
}

fn q(s: &str) -> String {
    ordo_schema::ddl::q(s)
}

/// What one sweep did, in the operator's own vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepReport {
    pub space: SpaceId,
    /// Handles no live row names any more, newly tombstoned.
    pub marked: u64,
    /// Tombstones a revert cancelled — the row came back before the floor
    /// passed, which is the mechanism working, not a fault.
    pub cancelled: u64,
    /// Objects removed from the store, and the references removed with
    /// them.
    ///
    /// An abandoned upload counts here too and has no reference to remove:
    /// its tombstone is an intent the door wrote before a `put` whose
    /// transaction never committed. One number rather than two, because
    /// what an operator is reading it for is bytes that stopped costing
    /// money.
    pub deleted: u64,
    /// Tombstones still waiting on the floor. **The number that explains
    /// why storage is not shrinking**, and it is only meaningful beside
    /// `holder`.
    pub waiting: u64,
    /// Why the floor is where it is — the *same* value the log prune
    /// reports, for the same reason.
    pub holder: FloorHolder,
    /// The instant below which an unreferenced handle may be collected.
    pub cutoff: u64,
    /// Set when phase 2 stopped part way. The deletions before it stand
    /// and are counted in `deleted`. `waiting` is then a best-effort
    /// count, zero when the database could not answer.
    pub stopped: Option<Stopped>,
}

/// Why the objects past the floor were not all deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uncollected {
    /// The object store refused or could not be reached.
    Store,
    /// The space's database refused a statement.
    Database,
}

impl Uncollected {
    /// The word a metric label and a security entry use.
    pub fn as_str(self) -> &'static str {
        match self {
            Uncollected::Store => "store",
            Uncollected::Database => "database",
        }
    }
}

/// A phase 2 that stopped at its first failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stopped {
    pub cause: Uncollected,
    /// The failure's text. The deployment owner's to read, never a caller's.
    pub error: String,
    /// Objects past the floor that were left, the failed one included.
    pub not_deleted: u64,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum SweepError {
    Store(PoolError),
    Blob(BlobError),
}

impl std::fmt::Display for SweepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SweepError::Store(e) => write!(f, "{e}"),
            SweepError::Blob(e) => write!(f, "{e}"),
        }
    }
}

impl SweepError {
    /// The cause a record files this failure under.
    pub fn cause(&self) -> Uncollected {
        match self {
            SweepError::Store(_) => Uncollected::Database,
            SweepError::Blob(_) => Uncollected::Store,
        }
    }
}

impl From<PoolError> for SweepError {
    fn from(e: PoolError) -> Self {
        SweepError::Store(e)
    }
}
impl From<BlobError> for SweepError {
    fn from(e: BlobError) -> Self {
        SweepError::Blob(e)
    }
}

/// The `NOT EXISTS (…)` body that is true when **no live row** names a
/// `$blobref` row aliased `b`.
///
/// Derived from the schema, never from a list somebody maintains: DL §2
/// makes "the compiler knows every blob field" the premise of the whole GC
/// story, and this is that premise spelled as SQL.
///
/// Inline fields are excluded, and not by oversight — an inline blob has
/// no `$blobref` row at all, so it can neither be found nor be a
/// reference. Including its column would compare a payload handle against
/// a blob handle, two disjoint keyed streams, and the comparison would be
/// false forever in a way nothing would notice.
///
/// **`blob sealed` fields are excluded too, and for a sharper reason: the
/// comparison cannot be made to work at all, rather than merely coming out
/// false.** The column would hold a sealed envelope of the handle, minted
/// with a fresh nonce on every write, while `encode(b."ref", 'hex')` is the
/// plaintext handle as text. Postgres has no `bytea = text` operator, so
/// the query refuses before a row is read; and were the types reconciled,
/// two sealed envelopes of one handle are still never byte-equal, so no
/// keyless query could match them. Serving this tier needs a keyed
/// collector or a deterministic blind index derived from the handle.
///
/// Skipping a field here would ordinarily be a live-reference bug: an
/// object whose only reference this clause cannot see is an object the
/// sweep is free to delete. What makes it sound is the other half of the
/// argument, in `ordo_host::blob::PlatformBlobs`, which refuses
/// `blob sealed` by name on both the write and the read path. No sealed
/// column can ever come to hold a live handle, so there is no live
/// reference here to miss.
fn live_reference_clause(schema: &Schema, space: &SpaceId) -> Option<String> {
    let mut parts = Vec::new();
    for f in crate::blob::blob_fields(schema) {
        if f.inline || f.sealed {
            continue;
        }
        let (entity, field) = (f.entity, f.field);
        parts.push(format!(
            "SELECT 1 FROM {e} r WHERE r.{sp} = {s} AND r.{f} = encode(b.\"ref\", 'hex')",
            e = q(&entity),
            sp = q(ordo_schema::SPACE_COLUMN),
            s = sql_lit(&space.0),
            f = q(&field),
        ));
    }
    if parts.is_empty() {
        // A schema with no external blob fields: nothing can be a live
        // reference, so every `$blobref` row is unreferenced. That is the
        // right answer for a schema whose blob fields were dropped, and it
        // is why this returns `None` rather than a clause that is
        // accidentally true.
        return None;
    }
    Some(parts.join(" UNION ALL "))
}

/// **Phase 1.** Tombstone every handle no live row names. Transactional,
/// Postgres only, and nothing in the bucket is touched — so a crash here
/// costs nothing and a repeat is a no-op.
pub async fn mark(
    pool: &Pool,
    schema: &Schema,
    space: &SpaceId,
    now: u64,
) -> Result<Marked, PoolError> {
    let live = live_reference_clause(schema, space);
    let unreferenced = match &live {
        Some(clause) => format!("AND NOT EXISTS ({clause})"),
        None => String::new(),
    };
    let sql = format!(
        "INSERT INTO {tomb} ({sp}, \"ref\", \"unreferenced_since\", \"store\") \
         SELECT b.{sp}, b.\"ref\", {now}, b.\"store\" FROM {refs} b \
         WHERE b.{sp} = {s} \
           AND NOT EXISTS (SELECT 1 FROM {tomb} t WHERE t.{sp} = b.{sp} AND t.\"ref\" = b.\"ref\") \
           {unreferenced}",
        tomb = q(BLOB_TOMB_TABLE),
        refs = q(BLOB_REF_TABLE),
        sp = q(BLOB_SPACE_COLUMN),
        s = sql_lit(&space.0),
    );
    // **Cancel first, then mark.** An open tombstone for a handle that a
    // live row names again is stale, and leaving it costs the object.
    //
    // The sequence that does it: a row is deleted, phase 1 stamps the
    // handle at `t`, the delete is reverted inside the undo horizon so the
    // handle is referenced again, and the tombstone stays open because
    // phase 2 only ever examines tombstones that are **due**. Delete the
    // row a second time much later and this `INSERT` skips it, since a
    // tombstone already exists. The sweep then prices the second deletion
    // against `t`, the first one's clock, and collects bytes that are
    // still revertible.
    //
    // Phase 2's re-check does not save it: that runs only once a tombstone
    // is due, and by the time this one is due the handle is genuinely
    // unreferenced again, so the re-check agrees with deleting it. The
    // stale timestamp is the whole defect, and it is invisible at every
    // point where something looks.
    //
    // Cancelling here rather than re-stamping is deliberate. A re-stamp
    // would keep one row across two unrelated lifetimes, so the
    // `unreferenced_since` of a tombstone would no longer be the moment
    // its handle stopped being referenced. A cancel followed by a fresh
    // `INSERT` on the next deletion keeps that column meaning one thing.
    //
    // Closed tombstones are left alone. A closed row is the record that
    // the object was deleted, which is phase 2's crash-recovery evidence
    // and not a claim about the present.
    let cancel = live.as_ref().map(|clause| {
        format!(
            "DELETE FROM {tomb} t \
             WHERE t.{sp} = {s} AND t.\"deleted_at\" IS NULL \
               AND EXISTS ( \
                 SELECT 1 FROM {refs} b \
                 WHERE b.{sp} = t.{sp} AND b.\"ref\" = t.\"ref\" AND EXISTS ({clause}) \
               )",
            tomb = q(BLOB_TOMB_TABLE),
            refs = q(BLOB_REF_TABLE),
            sp = q(BLOB_SPACE_COLUMN),
            s = sql_lit(&space.0),
        )
    });

    let tx = pool.begin_for(OpClass::AddressedWrite).await?;
    let before = open_tombstones_in(&tx, space).await?;
    if let Some(cancel) = &cancel {
        tx.rows(cancel).await?;
    }
    // Counted from **after** the cancel, so a handle whose stale tombstone
    // was cancelled and then legitimately re-marked reads as one mark
    // rather than as a net zero.
    let after_cancel = open_tombstones_in(&tx, space).await?;
    tx.rows(&sql).await?;
    let after = open_tombstones_in(&tx, space).await?;
    tx.commit().await?;
    Ok(Marked {
        marked: after.saturating_sub(after_cancel),
        cancelled: before.saturating_sub(after_cancel),
    })
}

/// What phase 1 did.
///
/// Two numbers rather than one since the eager cancel landed. A cancel
/// here is the same fact phase 2's is, reached earlier: a handle a live
/// row names again, whose tombstone is therefore stale. They are summed
/// into one `cancelled` on the report, because an operator counting
/// cancelled tombstones is counting reverts, and which phase noticed is
/// this module's business rather than theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marked {
    /// Handles newly tombstoned, counted after the cancel so a handle
    /// whose stale tombstone was cancelled and then legitimately
    /// re-marked reads as one mark rather than as a net zero.
    pub marked: u64,
    /// Stale tombstones cancelled because a live row names the handle
    /// again.
    pub cancelled: u64,
}

async fn open_tombstones_in(tx: &crate::TxConn, space: &SpaceId) -> Result<u64, PoolError> {
    let rows = tx
        .rows(&format!(
            "SELECT count(*) FROM {tomb} WHERE {sp} = {s} AND \"deleted_at\" IS NULL",
            tomb = q(BLOB_TOMB_TABLE),
            sp = q(BLOB_SPACE_COLUMN),
            s = sql_lit(&space.0),
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// The instant below which an unreferenced handle may be collected.
///
/// **The same constraint the log prune reports**, expressed in the
/// coordinate a tombstone carries. `Floor::live_bound` is a log position:
/// everything below it is prunable, and it is `min(live consumer
/// positions, horizon_position)`. Its wall-clock equivalent is the
/// timestamp of the group sitting at that position — every group below it
/// is older, so every image below it is older.
///
/// When the bound is past the head there is nothing retained above the
/// horizon, and the cutoff is the horizon itself.
async fn cutoff_of(
    pool: &Pool,
    space: &SpaceId,
    floor: &Floor,
    policy: &LogPolicy,
    now: u64,
) -> Result<u64, PoolError> {
    let horizon = now.saturating_sub(policy.undo_horizon.as_millis() as u64);
    match crate::prune::group_time_at(pool, &log_space(space), floor.live_bound).await? {
        // A consumer holding the floor pins the cutoff to that group's
        // instant, which is at or before the horizon — so the sweep slows
        // down for exactly the reason the prune does, and says so.
        //
        // Converted first: a command's `at` is seconds (`Timestamp`'s own
        // doc, and the unit the prune compares in), while `now` and every
        // tombstone here are milliseconds. Taking the minimum of the two
        // raw numbers put the cutoff in January 1970, which stopped the
        // sweep rather than slowing it. That stayed invisible while the
        // host stamped commands from a counter (checklist 11-28).
        Some(at) => Ok(at.saturating_mul(1_000).min(horizon)),
        None => Ok(horizon),
    }
}

/// **Phase 2**, plus phase 1 ahead of it. The whole sweep for one space.
///
/// Out of transaction and idempotent by construction: every step is either
/// a no-op on repeat or re-derived from the database.
pub async fn sweep(
    pool: &Pool,
    stores: &dyn StoreResolver,
    schema: &Schema,
    space: &SpaceId,
    policy: &LogPolicy,
    now: u64,
) -> Result<SweepReport, SweepError> {
    let phase1 = mark(pool, schema, space, now).await?;
    let marked = phase1.marked;
    // The prune's floor is computed in the prune's unit. `now` here is
    // milliseconds, because tombstones are; the floor compares command
    // times, which are seconds, and it measures consumer idleness in
    // seconds too. Handing it milliseconds made every group look older than
    // the horizon, so the horizon never bound, and made every mirror look a
    // thousand times idler than it was (checklist 11-28).
    let floor = crate::prune::floor(pool, &log_space(space), policy, now / 1_000).await?;
    let cutoff = cutoff_of(pool, space, &floor, policy, now).await?;

    let due = pool
        .rows(&format!(
            "SELECT encode(\"ref\", 'hex'), \"store\" FROM {tomb} \
             WHERE {sp} = {s} AND \"deleted_at\" IS NULL AND \"unreferenced_since\" < {cutoff} \
             ORDER BY \"ref\"",
            tomb = q(BLOB_TOMB_TABLE),
            sp = q(BLOB_SPACE_COLUMN),
            s = sql_lit(&space.0),
        ))
        .await?;

    let live = live_reference_clause(schema, space);
    // **Each object is deleted from the store its tombstone names**, not
    // from the store the space writes to now (ruled 2026-10-05,
    // `detach-upload-race` ask 2). An upload that landed in a bucket while
    // it was being detached or replaced is in that bucket, and deleting
    // against the space's current route would succeed (an absent-key delete
    // is idempotent by design), retire the reference, and leave the bytes
    // where nothing can name them. Resolved once per name.
    let mut by_name: std::collections::HashMap<String, Arc<dyn ordo_blob::BlobStore>> =
        std::collections::HashMap::new();
    let mut deleted = 0u64;
    // Phase 1's eager cancels join phase 2's. Both are the same fact,
    // a handle a live row names again, and an operator counting cancelled
    // tombstones is counting reverts rather than phases.
    let mut cancelled = phase1.cancelled;

    let mut stopped: Option<Stopped> = None;
    for (i, row) in due.iter().enumerate() {
        let Some(hex) = row.first().and_then(|c| c.as_deref()) else {
            continue;
        };
        let Some(handle) = parse_handle(hex) else {
            continue;
        };
        let name = row.get(1).and_then(|c| c.as_deref()).unwrap_or_default();
        let store = match by_name.get(name) {
            Some(store) => Arc::clone(store),
            None => match stores.named(space, name).await {
                Ok(store) => {
                    by_name.insert(name.to_string(), Arc::clone(&store));
                    store
                }
                Err(e) => {
                    let e = SweepError::from(e);
                    stopped = Some(Stopped {
                        cause: e.cause(),
                        error: e.to_string(),
                        not_deleted: (due.len() - i) as u64,
                    });
                    break;
                }
            },
        };
        match collect_one(pool, store.as_ref(), live.as_deref(), space, &handle, now).await {
            Ok(Collected::Deleted) => deleted += 1,
            Ok(Collected::Cancelled) => cancelled += 1,
            Ok(Collected::Skipped) => {}
            // The first failure ends the space's pass. A store that refused
            // one delete will refuse the next, and each refusal can cost a
            // timeout. What is left is counted, so the record can say how
            // many objects past the floor stayed and why.
            Err(e) => {
                stopped = Some(Stopped {
                    cause: e.cause(),
                    error: e.to_string(),
                    not_deleted: (due.len() - i) as u64,
                });
                break;
            }
        }
    }

    if let Some(stopped) = stopped {
        // No reap, and the recount is best effort: the cause may be the
        // database itself, and the deletions already made must still be
        // reported.
        let waiting = match pool.begin_for(OpClass::Read).await {
            Ok(tx) => {
                let n = open_tombstones_in(&tx, space).await.unwrap_or(0);
                let _ = tx.commit().await;
                n
            }
            Err(_) => 0,
        };
        return Ok(SweepReport {
            space: space.clone(),
            marked,
            cancelled,
            deleted,
            waiting,
            holder: floor.holder,
            cutoff,
            stopped: Some(stopped),
        });
    }

    // Reap closed tombstones that are themselves below the cutoff. They
    // exist to make phase 2 idempotent across a crash; once the reference
    // is gone and the horizon has passed again they answer nothing, and
    // keeping them would make a table that only grows.
    pool.rows(&format!(
        "DELETE FROM {tomb} WHERE {sp} = {s} AND \"deleted_at\" IS NOT NULL \
         AND \"deleted_at\" < {cutoff}",
        tomb = q(BLOB_TOMB_TABLE),
        sp = q(BLOB_SPACE_COLUMN),
        s = sql_lit(&space.0),
    ))
    .await?;

    let tx = pool.begin_for(OpClass::Read).await?;
    let waiting = open_tombstones_in(&tx, space).await?;
    tx.commit().await?;

    Ok(SweepReport {
        space: space.clone(),
        marked,
        cancelled,
        deleted,
        waiting,
        holder: floor.holder,
        cutoff,
        stopped: None,
    })
}

/// What phase 2 did with one due handle.
enum Collected {
    /// The tombstone was already closed by a concurrent pass.
    Skipped,
    /// A live row names the handle again; the tombstone went.
    Cancelled,
    /// The object and its reference are gone.
    Deleted,
}

/// Phase 2 for one handle, in its own transaction.
async fn collect_one(
    pool: &Pool,
    store: &dyn ordo_blob::BlobStore,
    live: Option<&str>,
    space: &SpaceId,
    handle: &BlobHandle,
    now: u64,
) -> Result<Collected, SweepError> {
    let handle = *handle;
    // The re-check, under the tombstone's own row lock. This is what
    // makes "a revert cancels a tombstone rather than racing it" true
    // without a lock across two systems: a revert that restored the
    // row committed before this `SELECT … FOR UPDATE` returned, or it
    // will block until this transaction ends and find the object gone
    // — and a revert that finds a missing object refuses, loudly,
    // rather than restoring a dangling reference.
    let tx = pool.begin_for(OpClass::AddressedWrite).await?;
    let locked = tx
        .rows(&format!(
            "SELECT 1 FROM {tomb} WHERE {sp} = {s} AND \"ref\" = {r} \
                 AND \"deleted_at\" IS NULL FOR UPDATE",
            tomb = q(BLOB_TOMB_TABLE),
            sp = q(BLOB_SPACE_COLUMN),
            s = sql_lit(&space.0),
            r = ordo_crypto::sealed::bytea_lit(&handle),
        ))
        .await?;
    if locked.is_empty() {
        let _ = tx.rollback().await;
        return Ok(Collected::Skipped);
    }
    let referenced = match live {
        None => false,
        Some(clause) => {
            let sql = format!(
                "SELECT 1 FROM {refs} b WHERE b.{sp} = {s} AND b.\"ref\" = {r} \
                     AND EXISTS ({clause})",
                refs = q(BLOB_REF_TABLE),
                sp = q(BLOB_SPACE_COLUMN),
                s = sql_lit(&space.0),
                r = ordo_crypto::sealed::bytea_lit(&handle),
            );
            !tx.rows(&sql).await?.is_empty()
        }
    };
    if referenced {
        // A revert brought the row back. The tombstone goes, the bytes
        // stay, and the handle is live again — which is exactly the
        // guarantee this whole module exists to keep.
        tx.rows(&format!(
            "DELETE FROM {tomb} WHERE {sp} = {s} AND \"ref\" = {r}",
            tomb = q(BLOB_TOMB_TABLE),
            sp = q(BLOB_SPACE_COLUMN),
            s = sql_lit(&space.0),
            r = ordo_crypto::sealed::bytea_lit(&handle),
        ))
        .await?;
        tx.commit().await?;
        return Ok(Collected::Cancelled);
    }
    // The lock is held across the object delete on purpose: a revert
    // that would restore this row waits, and then finds the reference
    // gone rather than restoring one that points at nothing.
    //
    // For an abandoned upload's intent there is nothing to race: no
    // group was ever appended, so no revert can name this handle. The
    // same three statements serve it, and the `DELETE` two below
    // removes no row because there never was one.
    store.delete(space, &handle).await?;
    tx.rows(&format!(
        "UPDATE {tomb} SET \"deleted_at\" = {now} WHERE {sp} = {s} AND \"ref\" = {r}",
        tomb = q(BLOB_TOMB_TABLE),
        sp = q(BLOB_SPACE_COLUMN),
        s = sql_lit(&space.0),
        r = ordo_crypto::sealed::bytea_lit(&handle),
    ))
    .await?;
    // The reference LAST, per §5.3. Everything above this line is
    // repeatable; this is the line that makes the handle unnameable.
    tx.rows(&format!(
        "DELETE FROM {refs} WHERE {sp} = {s} AND \"ref\" = {r}",
        refs = q(BLOB_REF_TABLE),
        sp = q(BLOB_SPACE_COLUMN),
        s = sql_lit(&space.0),
        r = ordo_crypto::sealed::bytea_lit(&handle),
    ))
    .await?;
    tx.commit().await?;
    Ok(Collected::Deleted)
}

/// Every space with a `$blobref` or `$blobtomb` row on this pool, sorted:
/// the spaces a sweep of one database has to visit.
///
/// Read from the reference set rather than from `$spacedb` or from the
/// store, for the reason `plan-tenant-storage.md` §6 B-G3 gives: the
/// references are what bound the work, and there is no way to enumerate
/// from the store side. A space with an open tombstone and no reference
/// left (an abandoned upload's intent, or a reference phase 2 removed
/// before a crash stamped the tombstone) is still visited, which is what
/// the `UNION` is for.
pub async fn spaces_with_objects(pool: &Pool) -> Result<Vec<SpaceId>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT {sp} FROM {refs} UNION SELECT {sp} FROM {tomb} ORDER BY 1",
            refs = q(BLOB_REF_TABLE),
            tomb = q(BLOB_TOMB_TABLE),
            sp = q(BLOB_SPACE_COLUMN),
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.first().and_then(|c| c.clone()))
        .map(SpaceId)
        .collect())
}

/// How many objects a space still holds a reference to. What a shape
/// summary prices and what an operator compares against a bucket bill.
pub async fn reference_count(pool: &Pool, space: &SpaceId) -> Result<i64, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM {refs} WHERE {sp} = {s}",
            refs = q(BLOB_REF_TABLE),
            sp = q(BLOB_SPACE_COLUMN),
            s = sql_lit(&space.0),
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// Objects an erasure left where they were, in one store.
///
/// What the app developer's erasure receipt needs and what the custody
/// entry carries (D-S5, ruled 2026-09-21): which store, how many objects,
/// the epochs whose destruction made them unopenable, and a digest of the
/// list of keys. **Never key material and never a credential**: the store is
/// named by endpoint, bucket and key prefix, and the objects by a digest of
/// their handles, which are not secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectsLeft {
    /// `endpoint|bucket|keyPrefix` of the tenant-owned bucket.
    pub store: String,
    pub count: u64,
    /// The key epochs the left objects were sealed under, ascending.
    pub epochs: Vec<String>,
    /// `sha256:` over the sorted hex handles, one per line.
    pub digest: String,
}

impl ObjectsLeft {
    /// The custody action string, in the `verb key=value` form the other
    /// custody actions use. No credential label either: the attachment's
    /// label names material on the node and a reader of this has no use
    /// for it.
    pub fn action(&self) -> String {
        format!(
            "objects_left store={} count={} epochs={} digest={}",
            self.store,
            self.count,
            self.epochs.join(","),
            self.digest
        )
    }
}

/// What [`erase_space`] did to a space's objects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErasedObjects {
    /// Objects deleted from the deployment's own store.
    pub deleted: u64,
    /// Objects left in a tenant-owned bucket, one entry per bucket.
    pub left: Vec<ObjectsLeft>,
}

/// Does this recorded store name a bucket the tenant owns?
///
/// An empty name is the deployment's own store. Otherwise the name is an
/// attachment, and the tenant owns the bucket under
/// [`BucketCustody::OwnPerimeter`]. A `PlatformSealed` attachment is a
/// bucket the platform manages on the tenant's behalf, and erasure treats
/// it as it always did: the objects are deleted. A name that does not parse
/// returns `None`, and the caller resolves it as before, which refuses it.
fn tenant_owned(name: &str) -> Option<Attachment> {
    if name.is_empty() {
        return None;
    }
    Attachment::from_store_name(name).filter(|a| a.custody == BucketCustody::OwnPerimeter)
}

/// Erase a space's objects, for space erasure.
///
/// Not a sweep: erasure is not garbage collection and must not be floored
/// at the undo horizon, because there is nothing left to undo into. It
/// runs beside `KeyProvider::destroy_space`.
///
/// **Decided per file, from the store the row records** (D-S5):
///
/// - In the deployment's own store, or a bucket attached under
///   `PlatformSealed`, the object is deleted. Crypto-erasure alone would
///   leave the objects unopenable but still listable and still billed: an
///   object's existence and its size are observable without any key.
/// - In a bucket the tenant owns (`OwnPerimeter`), the object is **left**.
///   Removing it is the tenant's to do, and the platform does not reach into
///   a perimeter it was told is the tenant's. The destroyed keys are what
///   make it unopenable. What was left comes back as [`ObjectsLeft`], and
///   the caller records it. Ordo asserts its own act and never that the
///   bytes are gone.
///
/// A tenant whose files sit in several stores gets each treatment per file.
pub async fn erase_space(
    pool: &Pool,
    stores: &dyn StoreResolver,
    space: &SpaceId,
) -> Result<ErasedObjects, SweepError> {
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, BTreeSet};

    struct Left {
        store: String,
        epochs: BTreeSet<String>,
        handles: Vec<String>,
    }

    let rows = pool.rows(&ordo_crypto::blobref::list_sql(space)).await?;
    let mut out = ErasedObjects::default();
    let mut left: BTreeMap<String, Left> = BTreeMap::new();
    for row in &rows {
        let Some(hex) = row.first().and_then(|c| c.as_deref()) else {
            continue;
        };
        let Some(handle) = parse_handle(hex) else {
            continue;
        };
        // From the store the row names, for the sweep's reason: an object
        // written during a detach or a replace is in the bucket it landed
        // in (ruled 2026-10-05, `detach-upload-race` ask 2), and since
        // 2026-10-06 every file written before a change or a detach is
        // too (`st-attach-existing` ask 2).
        //
        // D-S5 (ruled 2026-09-21, not built) leaves the objects in a
        // tenant's own perimeter and records what remained. When it is
        // built, the custody is per file: the recorded name carries the
        // `custody` of the bucket the file went to, so a tenant who moved
        // from their own bucket to the platform's has files under both
        // answers, and each must be decided from its own row rather than
        // from the tenant's current attachment.
        let name = row
            .get(ordo_crypto::blobref::LIST_STORE_COLUMN)
            .and_then(|c| c.as_deref())
            .unwrap_or_default();
        if let Some(at) = tenant_owned(name) {
            let entry = left.entry(name.to_string()).or_insert_with(|| Left {
                store: format!(
                    "{}|{}|{}",
                    at.endpoint,
                    at.bucket,
                    at.key_prefix.as_deref().unwrap_or_default()
                ),
                epochs: BTreeSet::new(),
                handles: Vec::new(),
            });
            if let Some(epoch) = row.get(1).and_then(|c| c.as_deref()) {
                entry.epochs.insert(epoch.to_string());
            }
            entry.handles.push(hex.to_string());
            continue;
        }
        let store = stores.named(space, name).await?;
        store.delete(space, &handle).await?;
        out.deleted += 1;
    }
    for (_, mut l) in left {
        l.handles.sort();
        let mut h = Sha256::new();
        for handle in &l.handles {
            h.update(handle.as_bytes());
            h.update(b"\n");
        }
        let digest: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        out.left.push(ObjectsLeft {
            store: l.store,
            count: l.handles.len() as u64,
            epochs: l.epochs.into_iter().collect(),
            digest: format!("sha256:{digest}"),
        });
    }
    for table in [BLOB_REF_TABLE, BLOB_TOMB_TABLE] {
        pool.rows(&format!(
            "DELETE FROM {t} WHERE {sp} = {s}",
            t = q(table),
            sp = q(BLOB_SPACE_COLUMN),
            s = sql_lit(&space.0),
        ))
        .await?;
    }
    Ok(out)
}

fn parse_handle(hex: &str) -> Option<BlobHandle> {
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Convenience for a worker that holds a resolver behind an `Arc`.
pub async fn sweep_with(
    pool: &Pool,
    stores: Arc<dyn StoreResolver>,
    schema: &Schema,
    space: &SpaceId,
    policy: &LogPolicy,
    now: u64,
) -> Result<SweepReport, SweepError> {
    sweep(pool, stores.as_ref(), schema, space, policy, now).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_syntax::parse_program;

    fn schema_of(src: &str) -> Schema {
        ordo_schema::build(&parse_program(src).expect("parses")).expect("builds")
    }

    /// The live-reference clause is derived from the schema, and it names
    /// every external blob field and neither an inline one nor a sealed
    /// one.
    #[test]
    fn the_reference_set_comes_from_the_schema() {
        let s = schema_of(
            "domain d @ 1.0.0\n\
             entity docs (key: doc) { scan: blob?  avatar: blob? inline  note: text? }\n\
             entity invoices (key: inv) { pdf: blob? sealed }\n",
        );
        let clause = live_reference_clause(&s, &SpaceId("t".into())).expect("some blob fields");
        assert!(clause.contains("\"docs\""), "{clause}");
        assert!(clause.contains("\"scan\""), "{clause}");
        assert!(
            !clause.contains("\"avatar\""),
            "an inline blob has no `$blobref` row, so it can neither be found nor be a \
             reference: {clause}"
        );
        assert!(!clause.contains("\"note\""), "{clause}");
        assert!(
            !clause.contains("\"invoices\"") && !clause.contains("\"pdf\""),
            "a sealed handle's fresh nonce means no column value could ever equal another \
             sealed envelope of the same handle, so the comparison the collector would have \
             to make cannot be made to work at all; the door refuses `blob sealed` writes, \
             which is what makes leaving this field out of the reference set sound rather \
             than merely convenient: {clause}"
        );
        assert!(clause.contains("'t'"), "unscoped: {clause}");
    }

    /// A schema with no external blob fields makes every reference
    /// collectable — the right answer after a blob field is dropped, and
    /// the reason the clause is an `Option` rather than a string that is
    /// accidentally true.
    #[test]
    fn a_schema_with_no_blob_fields_has_no_live_references() {
        let s = schema_of("domain d @ 1.0.0\nentity docs (key: doc) { note: text? }\n");
        assert!(live_reference_clause(&s, &SpaceId("t".into())).is_none());
        let s = schema_of("domain d @ 1.0.0\nentity docs (key: doc) { a: blob? inline }\n");
        assert!(
            live_reference_clause(&s, &SpaceId("t".into())).is_none(),
            "inline-only is the same as none, for `$blobref` purposes"
        );
        let s = schema_of("domain d @ 1.0.0\nentity docs (key: doc) { a: blob? sealed }\n");
        assert!(
            live_reference_clause(&s, &SpaceId("t".into())).is_none(),
            "sealed-only is the same as none too, for the same `$blobref` purposes: the \
             door refuses every write that could populate the column"
        );
    }

    #[test]
    fn a_handle_parses_or_refuses() {
        assert_eq!(parse_handle(&"ab".repeat(16)), Some([0xab; 16]));
        assert!(parse_handle("abc").is_none());
        assert!(parse_handle(&"zz".repeat(16)).is_none());
    }
}
