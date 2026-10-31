//! Resolving a `SealedRef`: the durable half of the sealed payload store.
//!
//! `ordo_crypto::sealed` owns the table's shape and the statements, because
//! two crates that cannot see each other both have to create it. This is
//! the half that needs a database: looking a handle up, opening what it
//! names, and the collection query for payloads a prune orphaned.
//!
//! # Resolution has three answers, not two
//!
//! A handle either resolves, or fails in one of two ways that mean
//! genuinely different things to an operator — and `security-logging-
//! design.md` §7 requires the difference to reach them rather than be
//! flattened into "unresolvable":
//!
//! - [`Resolved::Payload`] — the store holds it and the epoch key opened
//!   it. This is the ordinary answer, and it is the answer that was
//!   impossible before the store existed.
//! - [`Resolved::Pruned`] — no row. The group that named this handle was
//!   pruned below a checkpoint and its payloads went with it, or the
//!   handle was never minted here at all (a handle from another space
//!   reads as absent, because the lookup is keyed on `("$space", "ref")`).
//! - [`Resolved::Destroyed`] — the row is there and its epoch key was
//!   deliberately erased: crypto-erasure, working exactly as intended.
//!   The payload is kept rather than deleted, so the platform can say
//!   *why* the value is gone instead of pretending there never was one.
//! - [`Resolved::Unreadable`] — the row is there, the key is present, and
//!   the envelope will not authenticate or decrypt: possibly tampered
//!   (owner, 2026-10-23). Never reported as destroyed.
//!
//! A revert refuses on either failure, but with the reason attached; a
//! history view renders an explicit unresolvable marker for the same
//! reason. Neither ever silently substitutes an empty value.

use ordo_crypto::sealed::{self, SealedPayload};
use ordo_crypto::{KeyLease, KeyProvider, SealedHandle, SpaceId};

use crate::{sql_lit, Pool, PoolError, TxConn};

/// What a handle resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// The plaintext, opened under a key release.
    Payload(Vec<u8>),
    /// No payload under this handle in this space.
    Pruned,
    /// The payload is stored, and its epoch key is gone.
    Destroyed(ordo_crypto::Epoch),
    /// The payload is stored, its epoch key is present, and it will not
    /// open. Unreadable, possibly tampered.
    Unreadable(ordo_crypto::Epoch),
}

impl Resolved {
    /// The reason a handle did not resolve, in the platform's own words,
    /// or `None` when it did.
    pub fn refusal(&self) -> Option<String> {
        match self {
            Resolved::Payload(_) => None,
            Resolved::Pruned => Some(
                "the payload it names is not in this space's sealed payload store — the \
                 command group that minted it has been pruned"
                    .into(),
            ),
            Resolved::Destroyed(epoch) => Some(format!(
                "epoch {epoch}'s key has been destroyed, so the payload it names can never \
                 be opened again"
            )),
            Resolved::Unreadable(epoch) => Some(format!(
                "the payload it names is unreadable, possibly tampered: epoch {epoch}'s key is \
                 present and the payload did not open"
            )),
        }
    }
}

/// Somewhere to run one statement — a pooled connection or an open
/// transaction. A resolution inside a revert must run in the revert's own
/// transaction; a resolution for a history view does not.
#[derive(Clone, Copy)]
pub enum Reader<'a> {
    Pool(&'a Pool),
    Tx(&'a TxConn),
}

impl Reader<'_> {
    async fn rows(&self, sql: &str) -> Result<Vec<crate::TextRow>, PoolError> {
        match self {
            Reader::Pool(p) => p.rows(sql).await,
            Reader::Tx(t) => t.rows(sql).await,
        }
    }
}

/// The stored payload a handle names, without opening it.
pub async fn lookup(
    db: Reader<'_>,
    space: &SpaceId,
    handle: &SealedHandle,
) -> Result<Option<SealedPayload>, PoolError> {
    let rows = db.rows(&sealed::lookup_sql(space, handle)).await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    sealed::payload_of(row)
        .map(Some)
        .ok_or_else(|| PoolError::Db("a stored sealed payload will not decode".into()))
}

/// Resolve a handle to its plaintext under a key release.
///
/// The lease is the caller's, not this function's: opening a sealed value
/// is a key use, and `KeyProvider` has no way to be asked for one without
/// a space, a purpose and a correlation. A revert opens under
/// [`ordo_crypto::Purpose::Write`], which is the purpose the trait already
/// documents as covering "opening for converge recipes" — a revert being
/// the converge recipe the platform computes for you.
pub async fn resolve(
    db: Reader<'_>,
    keys: &dyn KeyProvider,
    lease: &KeyLease,
    space: &SpaceId,
    handle: &SealedHandle,
) -> Result<Resolved, PoolError> {
    let Some(payload) = lookup(db, space, handle).await? else {
        return Ok(Resolved::Pruned);
    };
    // The envelope is AAD-bound to the entity and field it was sealed
    // for, and the store remembers both, so a payload that has been moved
    // between columns refuses here rather than opening into the wrong
    // place.
    match open_payload(keys, lease, &payload) {
        Ok(plain) => Ok(Resolved::Payload(plain)),
        // The key store failed, busy or broken. That says nothing about the
        // key, so it is never reported as crypto-erasure: a full pool is
        // `server_busy` (try again), anything else is the store's fault.
        Err(e) if e.is_store_fault() => Err(store_fault(e)),
        // The epoch is simply gone: a deliberately erased key, the one
        // certain "destroyed" case.
        Err(e) if e.is_destroyed() => Ok(Resolved::Destroyed(payload.epoch)),
        // The key is there and the envelope will not open: unreadable,
        // possibly tampered. Not destroyed (owner, 2026-10-23).
        Err(e) if e.is_unreadable() => Ok(Resolved::Unreadable(payload.epoch)),
        // A lease or space fault is this caller's, not a statement about
        // the payload.
        Err(e) => Err(PoolError::db(e.to_string())),
    }
}

/// A key store fault as the pool error a caller already handles: busy stays
/// busy ([`PoolError::Saturated`], answered `server_busy`), and anything else
/// is a store failure with its text.
fn store_fault(e: ordo_crypto::KeyError) -> PoolError {
    if e.is_busy() {
        PoolError::Saturated
    } else {
        PoolError::db(e.to_string())
    }
}

/// Open a stored payload at the anchor it was sealed at.
///
/// A label that names no anchor never opens.
fn open_payload(
    keys: &dyn KeyProvider,
    lease: &KeyLease,
    payload: &SealedPayload,
) -> Result<Vec<u8>, ordo_crypto::KeyError> {
    let anchor = ordo_crypto::Anchor::parse(&payload.anchor).ok_or(ordo_crypto::KeyError::Seal)?;
    keys.open_at(
        lease,
        &payload.entity,
        &payload.field,
        anchor,
        &payload.bytes,
    )
}

/// Store one payload, in the caller's transaction.
///
/// A plain `INSERT`: never an upsert, never a lookup-then-write. See
/// `ordo_crypto::sealed` for why deduplication is the one thing this store
/// must not do.
#[allow(clippy::too_many_arguments)]
pub async fn put(
    tx: &TxConn,
    space: &SpaceId,
    handle: &SealedHandle,
    epoch: ordo_crypto::Epoch,
    entity: &str,
    field: &str,
    anchor: &str,
    correlation: &str,
    payload: &[u8],
) -> Result<(), PoolError> {
    tx.rows(&sealed::insert_sql(
        space,
        handle,
        epoch,
        entity,
        field,
        anchor,
        correlation,
        payload,
    ))
    .await
    .map(|_| ())
}

/// How many payloads a space still holds under an epoch.
///
/// The number `KeyProvider::destroy_epoch`'s caller owes an answer to: the
/// provider takes the instruction and the caller owes the undo-horizon
/// check, and this is half of what that check needs to be more than a
/// hope.
pub async fn epoch_payload_count(
    pool: &Pool,
    space: &SpaceId,
    epoch: ordo_crypto::Epoch,
) -> Result<i64, PoolError> {
    let rows = pool.rows(&sealed::epoch_count_sql(space, epoch)).await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// Delete the payloads in a space whose command group is no longer in the
/// chain — the collection half of the garbage story.
///
/// A payload is reachable only through the image that names it, so when a
/// prune removes a group's chain entry its payloads become unnameable at
/// the same instant. This is that anti-join, and it is the whole of
/// collection: one index scan over `$sealedpayload_group`, no reference
/// counting, no mark phase, because a handle appears in exactly one group
/// by construction (it is minted per image write and never reused).
///
/// Something schedules this now: [`crate::prune::prune`] calls
/// [`collect_pruned_in`] as step 5 of the same transaction that deletes the
/// chain entries. That ordering is not tidiness — a payload is reachable
/// only through the image that names it, and the query below is an
/// anti-join against `$commandlog`. If the delete committed and the
/// collection did not, the orphaned payloads would be ciphertext that
/// nothing can name *and* that this query could never find again, because
/// it needs the very rows that were removed.
pub async fn collect_pruned(pool: &Pool, space: &SpaceId) -> Result<u64, PoolError> {
    collect(Reader::Pool(pool), space).await
}

/// The same collection, in the caller's transaction. What a prune uses.
pub async fn collect_pruned_in(tx: &TxConn, space: &SpaceId) -> Result<u64, PoolError> {
    collect(Reader::Tx(tx), space).await
}

async fn collect(db: Reader<'_>, space: &SpaceId) -> Result<u64, PoolError> {
    let before = payload_count(db, space).await?;
    db.rows(&format!(
        "DELETE FROM \"{}\" p WHERE p.\"{}\" = {sp} AND NOT EXISTS ( \
           SELECT 1 FROM \"$commandlog\" c \
           WHERE c.\"space\" = {sp} AND c.\"correlation\" = p.\"correlation\")",
        ordo_crypto::SEALED_PAYLOAD_TABLE,
        ordo_crypto::sealed::SEALED_SPACE_COLUMN,
        sp = sql_lit(&space.0),
    ))
    .await?;
    let after = payload_count(db, space).await?;
    Ok((before - after).max(0) as u64)
}

/// How many payloads a space holds in total.
pub async fn space_payload_count(pool: &Pool, space: &SpaceId) -> Result<i64, PoolError> {
    payload_count(Reader::Pool(pool), space).await
}

async fn payload_count(db: Reader<'_>, space: &SpaceId) -> Result<i64, PoolError> {
    let rows = db
        .rows(&format!(
            "SELECT count(*) FROM \"{}\" WHERE \"{}\" = {}",
            ordo_crypto::SEALED_PAYLOAD_TABLE,
            ordo_crypto::sealed::SEALED_SPACE_COLUMN,
            sql_lit(&space.0),
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}
