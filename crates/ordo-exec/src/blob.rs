//! The durable half of the blob tier: resolving a handle, finding the row
//! that holds it, and answering *may this principal reach this row*.
//!
//! `ordo_crypto::blobref` owns the table's shape because two crates that
//! cannot see each other both create it; `ordo-blob` owns the bytes and
//! the sealer. This is the half that needs a database.
//!
//! # Where the write went
//!
//! This module used to own `attach`, which inserted the `$blobref` row and
//! `UPDATE`d the entity column in the blob door's own transaction. It
//! appended no command group, so an upload was unrevertible, fired no
//! `converged` and no `written`, and did not move `rev`. It is gone: the
//! door mints a token and hands the write to
//! `Fixture::run_platform_converge`, and an upload is now an ordinary
//! converge (`docs/plan-blob-chain.md` §2). The test that keeps it gone is
//! `only_the_write_protocol_updates_entity_rows`.
//!
//! # Why the row and the reference are still one transaction
//!
//! The argument moved with the statements and is unchanged. A `$blobref`
//! row without a column pointing at it is a leaked object: the sweep will
//! find it unreferenced and collect it, which is the designed behaviour
//! and costs only money. A **column without a `$blobref` row** is the
//! opposite and is not survivable: the handle names nothing, `f.mime` and
//! `f.size` have nowhere to come from, and the shape gate cannot see it
//! because the column's type is still right. So the two writes go in one
//! transaction and the object is stored *before* it opens, in that order.
//! That is the same asymmetry `plan-blob.md` §5.3 draws for collection,
//! for the same reason: prefer the failure that costs money over the
//! failure that costs a guarantee.

use ordo_crypto::blobref::{self, BlobRef};
use ordo_crypto::custody::{BlobHandle, SpaceId};
use ordo_schema::Schema;
use ordo_sema::guard_filter_query;

use crate::{sql_lit, Pool, PoolError, TxConn};

/// Why a blob request did not proceed. Each maps onto an error class the
/// door already has — no new class, per the standing rule that a new
/// refusal class is an oracle until argued.
#[derive(Debug)]
#[non_exhaustive]
pub enum BlobRefusal {
    /// No such handle in this space, or no such row. **One answer for
    /// both**, for `door_denial`'s reason: distinguishing them would let a
    /// caller who can guess sixteen bytes learn which handles exist.
    NotFound,
    /// The row exists and the caller's guard does not admit it.
    Guard(String),
    /// The field is not a blob field, or the entity has no such field.
    NotABlobField(String),
    Store(PoolError),
}

impl std::fmt::Display for BlobRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobRefusal::NotFound => f.write_str("no such blob"),
            BlobRefusal::Guard(m) => write!(f, "{m}"),
            BlobRefusal::NotABlobField(m) => write!(f, "{m}"),
            BlobRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<PoolError> for BlobRefusal {
    fn from(e: PoolError) -> Self {
        BlobRefusal::Store(e)
    }
}

fn q(s: &str) -> String {
    ordo_schema::ddl::q(s)
}

/// Is `id` inside `principal`'s **read** view?
///
/// **The obligation the blob door's signature does not show.** The access
/// check the router makes answers "may this principal call this op at
/// all"; it does not answer "may this principal see this row". A blob
/// served without this is every byte delivered correctly to the wrong
/// person, and nothing about it looks wrong.
///
/// The predicate is `ordo_sema::guard_filter_query` — the write protocol's
/// own source for this question, and `undo::require_writable`'s. One
/// definition, three callers, so a guard that hides a row hides its file
/// by construction rather than by a second implementation agreeing.
///
/// **Reads only, since C4.** This used to take a `write: bool` and the
/// blob door's upload called it with `true`, inside a transaction of its
/// own. An upload is now an ordinary converge, so the write guard on that
/// path is the write protocol's: the row is locked before the guard is
/// evaluated, and the guard is re-evaluated on the row as it now is once
/// the mutation has run. Both of those are stronger than anything a single
/// pre-write check can be, and keeping the parameter would have left a
/// second write-guard implementation alive with no caller, which is the
/// arrangement that produces two answers to one question the first time
/// one of them is edited.
pub async fn require_visible(
    tx: &TxConn,
    schema: &Schema,
    space: &SpaceId,
    entity: &str,
    id: &str,
    principal: &str,
) -> Result<(), BlobRefusal> {
    let sql = match guard_filter_query(schema, entity, false, Some(principal))
        .map_err(|e| BlobRefusal::Guard(format!("bind: {e}")))?
    {
        Some(query) => format!(
            "SELECT 1 FROM ({}) v WHERE v.{} = {}",
            // Scoped like every other statement: a blob read that looked
            // at another tenant's rows to decide this one would be the
            // cross-space read the space column exists to prevent.
            ordo_plan::plan(&query, &ordo_plan::Space::Bound(space.0.clone()))
                .map_err(|e| BlobRefusal::Guard(format!("plan: {e}")))?
                .sql
                .trim_end_matches(';'),
            q("id"),
            sql_lit(id)
        ),
        None => format!(
            "SELECT 1 FROM {} WHERE {} = {} AND {} = {}",
            q(entity),
            q("id"),
            sql_lit(id),
            q(ordo_schema::SPACE_COLUMN),
            sql_lit(&space.0),
        ),
    };
    if tx.rows(&sql).await?.is_empty() {
        return Err(BlobRefusal::Guard(format!(
            "access_guard: `{principal}` may not read `{id}` ({entity})"
        )));
    }
    Ok(())
}

/// The reference a handle names, confined to its space.
pub async fn lookup(
    pool: &Pool,
    space: &SpaceId,
    handle: &BlobHandle,
) -> Result<Option<BlobRef>, PoolError> {
    let rows = pool.rows(&blobref::lookup_sql(space, handle)).await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    blobref::blob_ref_of(row)
        .map(Some)
        .ok_or_else(|| PoolError::Db("a stored blob reference will not decode".into()))
}

/// Which row a handle is attached to, if any.
///
/// Answered by scanning the entity's own column rather than by storing a
/// row id on `$blobref`, and that is a considered choice: a row id there
/// would be a second statement of the same fact, and the two would
/// disagree the first time a revert restored a row. The column IS the
/// reference — that is what makes the GC anti-join sound — so it is also
/// the authority on which row holds it.
pub async fn owning_row(
    pool: &Pool,
    space: &SpaceId,
    entity: &str,
    field: &str,
    inline: bool,
    handle: &BlobHandle,
) -> Result<Option<String>, PoolError> {
    let value = if inline {
        ordo_crypto::sealed::bytea_lit(handle)
    } else {
        sql_lit(&ordo_crypto::hex(handle))
    };
    let rows = pool
        .rows(&format!(
            "SELECT {} FROM {} WHERE {} = {} AND {} = {value} LIMIT 1",
            q("id"),
            q(entity),
            q(ordo_schema::SPACE_COLUMN),
            sql_lit(&space.0),
            q(field),
        ))
        .await?;
    Ok(rows.first().and_then(|r| r.first()).and_then(|c| c.clone()))
}

/// One blob field, as the collector needs to know it.
///
/// A struct rather than a wider tuple: the caller (`live_reference_clause`)
/// now has to test two independent flags before deciding whether a field
/// belongs in the live-reference query, and `(entity, field, inline,
/// sealed)` reads as "which of these four positions was the tier I meant"
/// at every call site touching it. Named fields do not.
pub struct BlobField {
    pub entity: String,
    pub field: String,
    pub inline: bool,
    /// `blob sealed`: declarable, refused at the door
    /// (`ordo_host::blob::PlatformBlobs`), and excluded here for the same
    /// reason the door refuses it: the collector cannot compare a sealed
    /// handle against itself. See `live_reference_clause`'s doc.
    pub sealed: bool,
}

/// Every blob field in a schema.
///
/// The compiler knows every blob field — DL §2 says so as the premise of
/// the whole GC story — and this is that knowledge, derived from the
/// schema rather than from a registry somebody has to remember to update.
pub fn blob_fields(schema: &Schema) -> Vec<BlobField> {
    let mut out = Vec::new();
    for e in &schema.entities {
        for f in e.declared() {
            if f.is_content() {
                out.push(BlobField {
                    entity: e.name.clone(),
                    field: f.name.clone(),
                    inline: f.inline,
                    sealed: f.sealed,
                });
            }
        }
    }
    out
}
