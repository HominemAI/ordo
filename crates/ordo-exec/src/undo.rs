//! The operator surface for the command log: **verify**, **checkpoint**,
//! **revert** — the three things `ordo-commandlog` could already compute
//! and nothing outside its unit tests could reach (8i).
//!
//! `ordo-commandlog` holds the arithmetic: the chain rules, the inversion
//! rules, the signed-checkpoint shape. This module is what makes them
//! happen *to a deployment* — one that has a Postgres `$commandlog`, a
//! schema, rows, and guards. It is the layer `ordo-cli` calls, and it is
//! deliberately small: every rule it applies is imported from the crate
//! that owns that rule, and what it adds is the database.
//!
//! # Verification must be able to say **where**
//!
//! [`verify`] streams the chain in keyset windows and reports the first
//! index that does not check out, with the kind of failure it was. Three
//! details are the difference between that sentence being true and being
//! decoration:
//!
//! - **The reported index is the stored `idx`, not a row offset.** A row
//!   deleted from the middle of `$commandlog` shifts every offset after
//!   it; a verification that counted rows would name the wrong position in
//!   exactly the case where the position matters most.
//! - **A read that fails is a failure.** [`crate::commandlog::PgStore`]'s
//!   `Store` accessors cannot report an error by signature, and its
//!   `verify_from` override already refuses to answer green on a chain it
//!   could not read — but it reports the *window's* start, because a batch
//!   is decoded as a batch. Here every row carries the `idx` the database
//!   gave it through decoding, so a body that will not decode names its own
//!   position rather than the 512-entry window it was in.
//! - **A checkpoint is a lower bound on length.** A verified checkpoint
//!   says the chain reached `upto`. If the stored chain does not reach it,
//!   the tail was truncated — the one rewrite a hash chain cannot notice on
//!   its own, and the reason [`verify`] checks it explicitly.
//!
//! # A revert is a write, and it is checked like one
//!
//! `security-logging-design.md` §7 settles the semantics: `revert_group`
//! returns inverse commands "which the platform applies **through the
//! normal gate** as a new attributed group. History is never rewritten; a
//! revert is more history." [`revert`] is that applier.
//!
//! There is no op to route it through — no author declares "restore this
//! row to this image", and the write protocol is driven by a declared op
//! and its statements — so what "through the normal gate" means here is
//! stated exactly, and it is the *checks*, not a shared code path:
//!
//! 1. **The row is locked** (`SELECT … FOR UPDATE`) before it is read, so
//!    the decision and the mutation share one snapshot.
//! 2. **The write guard must hold against the row as it stands**, built
//!    from `ordo_sema::guard_filter_query` — the same primitive the write
//!    protocol's own precondition uses, so there is one definition of "may
//!    this principal write this row" and this is a second caller of it,
//!    not a second copy.
//! 3. **The write guard must still hold against the row as the revert
//!    leaves it** — the post-image obligation. Without it a revert could
//!    re-point a ref and walk a row out of the reverter's own write view,
//!    which is the escalation 4a found on the ordinary write path.
//! 4. **The row must still be at the revision the group recorded.** The
//!    inverse is computed against a snapshot; if somebody has written the
//!    row since, restoring the old image would silently discard their
//!    write. This is `rev` used exactly as §6 rule 9 uses it — a stated
//!    belief about the row — and a mismatch is `cas_miss`, not a clobber.
//! 5. **Deletes refuse with their plain referrers**, in the platform's own
//!    vocabulary, rather than surfacing a foreign-key constraint name. Rows
//!    that reference the row through an `owned` ref are not referrers: they
//!    go with it, each recorded as its own command (owner ruling
//!    `owned-delete`, 2026-09-29), so undoing a create whose row has since
//!    gained owned rows is the same delete the write protocol performs.
//! 6. **The whole thing is one SERIALIZABLE transaction**, and the inverse
//!    group joins the chain *inside* it. A refused revert leaves neither an
//!    effect nor a link.
//!
//! What it does **not** check is authentication. `principal` is asserted by
//! whoever runs the tool, not proven: there is no token, no session, no
//! `ordo-identity` verification, no suspension check, no capability
//! allowlist, and no op-level access check (there is no op). See
//! [`revert`]'s own documentation, which states this where a caller will
//! read it.
//!
//! # What a revert refuses, and what it no longer refuses
//!
//! The rule is one sentence and it has not changed: a revert that cannot
//! reproduce the row it claims to be restoring is refused, never
//! approximated. What has changed is how much of the row the platform can
//! reproduce.
//!
//! - **Sealed fields resolve now (8j).** An image holds a `SealedRef`, and
//!   the sealed payload store holds the envelope that handle names. A
//!   revert opens the before-image's payload under a key release,
//!   **re-seals it under the current epoch with a fresh nonce**, and
//!   recomputes the blind index if the field carries one. It does not
//!   write the old envelope back: a revert is a write, so the value it
//!   leaves behind is sealed the way any write of that value today would
//!   seal it, which keeps rotation meaning what it says and keeps the
//!   store's no-deduplication rule true of reverts too.
//!
//!   The refusal narrows to the two cases where a payload is genuinely
//!   unreachable, and it names which:
//!   - **the payload was pruned** — its command group fell below a
//!     checkpoint and its payloads went with it;
//!   - **the epoch key was destroyed** — crypto-erasure, working. Design
//!     §7 gates destruction on the undo horizon precisely so this stays
//!     rare, and beyond the horizon it is the correct answer.
//!
//!   A deployment with **no key custody wired into this tool** is a third
//!   case, and it is refused for the honest reason rather than the old
//!   one: nothing here can open a payload without a `KeyProvider`, so the
//!   answer is "this tool has no keys", not "the platform cannot do it".
//! - **Blob objects are verified before they are restored (C6).** An
//!   image that names an external blob handle also names the digest the
//!   object had when it was uploaded (`"{field}$digest"`, C5), and a
//!   revert compares that digest against the object that is actually in
//!   the store, which is the owner's definition of verification in his
//!   own words. It happens in [`verify_restored_objects`], before any
//!   write, beside the sealed resolution that already refuses first.
//!
//!   **Absence is not a failed comparison**, and the two outcomes follow
//!   from that sentence. An object that is there and hashes differently
//!   refuses the revert (`blob_integrity`): that is proof of corruption
//!   or tampering. An object that is not there, or a store that does not
//!   answer, **restores the handle** and raises a security event: nothing
//!   was compared, so nothing was disproved, and the undo horizon must
//!   never come to depend on whether a bucket is up. An image written
//!   before the digest existed restores, raises nothing, and is recorded
//!   twice: transiently on [`Reverted::unverified`], for the caller that
//!   just ran the revert, and durably as `CommandGroup::restored_unverified`
//!   (`plan-opslog.md` D7), the trailer bit this revert's own group carries
//!   into the chain.
//! - **Tree entities are still refused outright.** The materialised
//!   `$path` column is physical, not a field, so it is not in any image.
//!   Restoring the fields would leave the path describing a parent the row
//!   no longer has. Nothing about the payload store touches this.

use std::collections::BTreeMap;

use ordo_commandlog::{
    chain::{decode_group, ChainHash, ChainedGroup, TamperAt, TamperKind},
    invert_group, verify_chain_from, Actor, Checkpoint, CheckpointVerifier, Command, CommandGroup,
    CommandId, CommandKind, CorrelationId, Ed25519Signer, Image, RestoreMarker, RestoreSource,
    RevertError, Signer, SpaceId, Timestamp, Value, VERIFY_BATCH,
};
use ordo_crypto::{KeyLease, Purpose, SpaceId as CryptoSpace};

/// Re-exported for the same reason [`KeyProvider`] is: a caller spells
/// `revert`'s two new optional handles without taking a dependency on
/// `ordo-blob` or `ordo-secevent` for one type name each.
pub use ordo_blob::BlobPipeline;
/// Re-exported so a caller can spell the `Option<&dyn KeyProvider>` this
/// module takes without depending on `ordo-crypto` for one type name.
pub use ordo_crypto::KeyProvider;
use ordo_schema::{Entity, FieldTy, Schema, SPACE_COLUMN};
pub use ordo_secevent::Recorder;
use ordo_sema::guard_filter_query;

use crate::commandlog::{bytea_of, chain_hash_of, space_lock_key, APPEND_LOCK_NS};
use crate::sealed::{self, Reader, Resolved};
use crate::{sql_lit, OpClass, Pool, PoolError, TxConn};

// ── key custody, when the caller has any ─────────────────────────────────

/// The key custody a revert runs under.
///
/// `Option` rather than a required argument, and the option is not
/// laziness. Opening a sealed payload is a key use, and this is an
/// **operator tool**: `ordo revert` holds database credentials and no
/// `KeyProvider`. A signature that pretended otherwise would either force
/// the CLI to fabricate custody or force every caller that legitimately has
/// none to pass a stub. So the absence is a value, and it produces a
/// refusal that says what is actually missing.
///
/// **The reason it holds none has changed** (R2c). It used to be that no
/// `WrappedKeyStore` over Postgres existed for a `LocalKeyProvider` to boot
/// against; [`crate::keystore::PgKeyStore`] is now that store. What is left
/// is the operator tool's own custody *selection* — which master key
/// source, and the refusal postures around it — and that arrives with
/// `plan-runtime.md` R2b's `ORDO_KEY_PROVIDER` and `ordo keys`. The
/// signature does not change when it does; a `Some` starts being passed.
///
/// A server-side caller (the executor, which already holds
/// `Executor::keys`) passes `Some` and gets the full behaviour.
pub struct Custody<'a> {
    keys: &'a dyn KeyProvider,
    lease: KeyLease,
    space: CryptoSpace,
}

impl<'a> Custody<'a> {
    /// Take the release a caller opens and (for a revert) seals under.
    ///
    /// `purpose` is the caller's to choose, and the two calls in this
    /// module choose differently for reasons worth keeping apart:
    /// [`revert`] releases under `Purpose::Write`, because a revert is a
    /// write and the trait already documents that purpose as covering
    /// "opening for converge recipes" — a revert being the converge
    /// recipe the platform computes on your behalf. [`verify_objects`]
    /// releases under `Purpose::Read`, because it writes nothing and
    /// mints nothing; it only ever opens a digest to compare it, which
    /// is the use `Purpose::Read` is named for. The correlation is the
    /// caller's own id, so the release is countable against the act that
    /// caused it rather than against nothing.
    pub fn release(
        keys: &'a dyn KeyProvider,
        purpose: Purpose,
        space: &SpaceId,
        correlation: &CorrelationId,
    ) -> Result<Custody<'a>, RevertFailure> {
        let space = CryptoSpace(space.0.clone());
        let lease = keys
            .release(&space, purpose, &correlation.0)
            .map_err(|e| RevertFailure::Refused(format!("key release: {e}")))?;
        Ok(Custody { keys, lease, space })
    }
}

/// Sealed values a revert has resolved and is about to write back, by
/// field name — plaintext, held only for the length of one command.
type Plaintexts = BTreeMap<String, Vec<u8>>;

// ── the one-statement database seam ──────────────────────────────────────

/// Somewhere to run a statement: a pooled connection, or an open
/// transaction.
///
/// Verification runs on both — from the pool for `ordo verify`, and from
/// inside the checkpointing transaction, where the suffix must be checked
/// under the same append lock that will hold the head still while it is
/// signed. One reader, two venues, rather than two readers that could
/// disagree about what a chain row means.
#[derive(Clone, Copy)]
pub enum Db<'a> {
    Pool(&'a Pool),
    Tx(&'a TxConn),
}

impl Db<'_> {
    async fn rows(&self, sql: &str) -> Result<Vec<crate::TextRow>, PoolError> {
        match self {
            Db::Pool(p) => p.rows(sql).await,
            Db::Tx(t) => t.rows(sql).await,
        }
    }
}

fn q_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// The tenancy predicate every row lookup in this module must carry.
///
/// Entity tables are `PRIMARY KEY ("$space", "id")`, and ids are minted
/// from a process counter with no space salt (`format!("{}_w{}", ...)` in
/// the write protocol), so two spaces in one deployment can mint the same
/// id. A `WHERE "id" = …` with no space conjunct answers with whichever
/// space's row Postgres happens to return, which is a different row than
/// the one this revert is running against as soon as a deployment serves
/// two spaces. Every statement here carries this predicate for the same
/// reason `Fixture::space_pred` carries it on the write path.
fn space_pred(space: &SpaceId) -> String {
    format!("{} = {}", q_ident(SPACE_COLUMN), sql_lit(&space.0))
}

// ── reading the chain ────────────────────────────────────────────────────

/// A stored chain entry, with the position the database gave it.
struct StoredEntry {
    idx: usize,
    chained: ChainedGroup,
}

/// One keyset window of the chain: `idx >= from`, in order, at most
/// `limit`. Keyset rather than `OFFSET` because the index is the answer —
/// an offset into surviving rows is not a chain position once a row is
/// missing, which is precisely the case verification exists to name.
///
/// The error carries the position, because the position is the answer.
/// Every row is decoded carrying the `idx` the database gave it, so a
/// corrupt entry names itself rather than naming the window it was in; only
/// a failure of the read itself has no row to blame, and then the honest
/// answer is where the read started.
async fn window(
    db: Db<'_>,
    space: &SpaceId,
    from: usize,
    limit: usize,
) -> Result<Vec<StoredEntry>, usize> {
    let rows = db
        .rows(&format!(
            "SELECT \"idx\", \"prev\", \"hash\", \"body\" FROM \"$commandlog\" \
             WHERE \"space\" = {} AND \"idx\" >= {from} ORDER BY \"idx\" LIMIT {limit}",
            sql_lit(&space.0)
        ))
        .await
        .map_err(|_| from)?;
    let mut out = Vec::with_capacity(rows.len());
    for (offset, row) in rows.iter().enumerate() {
        // `idx` is `bigint NOT NULL`, so it parses or the read was not of
        // this table; the positional fallback keeps the answer bounded
        // either way.
        let idx: usize = row
            .first()
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(from + offset);
        let prev = chain_hash_of(row.get(1).and_then(|c| c.as_ref())).ok_or(idx)?;
        let hash = chain_hash_of(row.get(2).and_then(|c| c.as_ref())).ok_or(idx)?;
        let body = bytea_of(row.get(3).and_then(|c| c.as_ref())).ok_or(idx)?;
        let group = decode_group(&body).map_err(|_| idx)?;
        out.push(StoredEntry {
            idx,
            chained: ChainedGroup { group, prev, hash },
        });
    }
    Ok(out)
}

/// The head of a space's chain as `(last index, hash)`, or `None` when the
/// space has no history.
///
/// `max(idx)`, never `count(*)`: the two agree only on a chain that has
/// never been pruned, and a checkpoint that recorded a count as a position
/// would anchor verification at the wrong place the first time a prefix was
/// dropped — which is the operation checkpoints exist to make legal.
pub async fn head_of(db: Db<'_>, space: &SpaceId) -> Result<Option<(usize, ChainHash)>, PoolError> {
    let rows = db
        .rows(&format!(
            "SELECT \"idx\", \"hash\" FROM \"$commandlog\" WHERE \"space\" = {} \
             ORDER BY \"idx\" DESC LIMIT 1",
            sql_lit(&space.0)
        ))
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let idx: usize = row
        .first()
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| PoolError::Db("chain head has no index".into()))?;
    let hash = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
        .ok_or_else(|| PoolError::Db("chain head has no hash".into()))?;
    Ok(Some((idx, hash)))
}

/// The most recent stored checkpoint for a space, whatever the stored
/// chain says about it.
///
/// Position only, no judgement: this is the CLI's "a checkpoint is stored
/// but no key was supplied" note and `record_restore`'s last-resort
/// default. Every reader that *picks* a checkpoint to stand on goes
/// through [`agreement`] instead, because a stored row can now be an
/// orphan a restore left behind.
pub async fn latest_checkpoint(
    db: Db<'_>,
    space: &SpaceId,
) -> Result<Option<Checkpoint>, PoolError> {
    Ok(
        checkpoints_where(db, space, "ORDER BY \"upto\" DESC, \"head\" DESC LIMIT 1")
            .await?
            .pop(),
    )
}

/// Every stored checkpoint for a space, newest position first.
///
/// Two rows may share a position since the `checkpoint-key` ruling
/// (2026-09-20), so this reads rows rather than a row.
pub async fn stored_checkpoints(db: Db<'_>, space: &SpaceId) -> Result<Vec<Checkpoint>, PoolError> {
    checkpoints_where(db, space, "ORDER BY \"upto\" DESC, \"head\" DESC").await
}

/// The stored checkpoints matching `tail` (an `ORDER BY …`, with any extra
/// `AND …` in front of it).
async fn checkpoints_where(
    db: Db<'_>,
    space: &SpaceId,
    tail: &str,
) -> Result<Vec<Checkpoint>, PoolError> {
    let rows = db
        .rows(&format!(
            "SELECT \"upto\", \"head\", \"at\", \"key_id\", \"signature\" FROM \"$checkpoints\" \
             WHERE \"space\" = {} {tail}",
            sql_lit(&space.0)
        ))
        .await?;
    rows.iter().map(|row| checkpoint_of(space, row)).collect()
}

/// One `$checkpoints` row as a [`Checkpoint`], given
/// `upto, head, at, key_id, signature` in that order.
fn checkpoint_of(space: &SpaceId, row: &crate::TextRow) -> Result<Checkpoint, PoolError> {
    let num = |i: usize| -> Result<u64, PoolError> {
        row.get(i)
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| PoolError::Db("malformed checkpoint row".into()))
    };
    Ok(Checkpoint {
        space: space.clone(),
        upto: num(0)?,
        head: chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
            .ok_or_else(|| PoolError::Db("checkpoint has no head".into()))?,
        at: Timestamp(num(2)?),
        key_id: row
            .get(3)
            .and_then(|c| c.clone())
            .ok_or_else(|| PoolError::Db("checkpoint has no key id".into()))?,
        signature: bytea_of(row.get(4).and_then(|c| c.as_ref()))
            .ok_or_else(|| PoolError::Db("checkpoint has no signature".into()))?,
    })
}

// ── does the stored chain agree with a record? (R2b′) ───────────────────

/// What the stored chain says about a record of a head at a position —
/// a `$checkpoints` row, a `$chainwitness` row, or a receiver's
/// attestation. Ruled 2026-09-19 (`orphan-checkpoints`): a checkpoint
/// counts when the stored chain agrees with its head, and the position of
/// the newest restore marker has nothing to do with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agreement {
    /// The chain holds the record's head at its position: current history.
    Current,
    /// The chain holds another hash there, or nothing that far: orphaned
    /// by a restore.
    Orphaned,
    /// Both entries that could confirm it are pruned; the chain cannot
    /// say. Reported, never judged.
    BelowFloor,
}

/// Classify one record of "this chain reached `upto` with head `head`"
/// against the stored chain.
///
/// At most two row reads, because two positions can confirm a head. The
/// entry at `upto - 1` *is* the head the record names, so its stored hash
/// settles it outright. When that entry is pruned the entry at `upto`
/// still carries the same hash as its `prev`, which is the case a cut
/// exactly at a checkpoint leaves. With neither stored the chain has no
/// opinion to offer: below its first stored index the record is
/// [`Agreement::BelowFloor`], and above its head it is an orphan, which is
/// the truncation a hash chain cannot see on its own.
pub async fn agreement(
    db: Db<'_>,
    space: &SpaceId,
    upto: u64,
    head: ChainHash,
) -> Result<Agreement, PoolError> {
    // A record over an empty prefix names genesis or it names nothing.
    let Some(at) = upto.checked_sub(1) else {
        return Ok(if head == ChainHash::GENESIS {
            Agreement::Current
        } else {
            Agreement::Orphaned
        });
    };
    let rows = db
        .rows(&format!(
            "SELECT \"idx\", \"hash\", \"prev\" FROM \"$commandlog\" \
             WHERE \"space\" = {} AND \"idx\" IN ({at}, {upto})",
            sql_lit(&space.0)
        ))
        .await?;
    let find = |want: u64| {
        rows.iter().find(|row| {
            row.first()
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse::<u64>().ok())
                == Some(want)
        })
    };
    if let Some(row) = find(at) {
        let stored = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
            .ok_or_else(|| PoolError::db(format!("chain entry {at} has no hash")))?;
        return Ok(if stored == head {
            Agreement::Current
        } else {
            Agreement::Orphaned
        });
    }
    if let Some(row) = find(upto) {
        let prev = chain_hash_of(row.get(2).and_then(|c| c.as_ref()))
            .ok_or_else(|| PoolError::db(format!("chain entry {upto} has no prev")))?;
        return Ok(if prev == head {
            Agreement::Current
        } else {
            Agreement::Orphaned
        });
    }
    let first = first_stored(db, space).await?;
    Ok(match first {
        // An empty chain confirms nothing and is short of everything.
        None => Agreement::Orphaned,
        Some(f) if upto < f => Agreement::BelowFloor,
        Some(_) => Agreement::Orphaned,
    })
}

/// The chain's lowest stored index, or `None` on an empty chain.
async fn first_stored(db: Db<'_>, space: &SpaceId) -> Result<Option<u64>, PoolError> {
    let rows = db
        .rows(&format!(
            "SELECT \"idx\" FROM \"$commandlog\" WHERE \"space\" = {} ORDER BY \"idx\" LIMIT 1",
            sql_lit(&space.0)
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|row| row.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok()))
}

// ── restore markers (`plan-production-ops.md` R2b) ──────────────────────

/// The actor column's rendering of a restore marker's actor. A marker is
/// located by this plaintext column and confirmed by decoding its body.
const RESTORE_ACTOR: &str = "system:restore";

/// Every restore marker in a space's stored chain, oldest first, as
/// `(index, marker)`.
///
/// Located by `$commandlog`'s plaintext `actor` column and confirmed by
/// decoding the row's body. The actor is hashed through the body, so on an
/// untampered chain the two agree; a row whose column says
/// `system:restore` and whose body carries no marker is refused, naming
/// its index, rather than skipped. `CommandGroup::validate` refuses a
/// marker under any other actor, so the column finds every marker the
/// append gate admitted.
pub async fn restore_markers(
    db: Db<'_>,
    space: &SpaceId,
) -> Result<Vec<(usize, RestoreMarker)>, PoolError> {
    markers_where(db, space, "ORDER BY \"idx\"").await
}

/// The newest restore marker in a space's stored chain, as
/// `(index, marker)`; see [`restore_markers`] for how it is found and
/// what is refused.
pub async fn latest_restore_marker(
    db: Db<'_>,
    space: &SpaceId,
) -> Result<Option<(usize, RestoreMarker)>, PoolError> {
    Ok(markers_where(db, space, "ORDER BY \"idx\" DESC LIMIT 1")
        .await?
        .pop())
}

async fn markers_where(
    db: Db<'_>,
    space: &SpaceId,
    tail: &str,
) -> Result<Vec<(usize, RestoreMarker)>, PoolError> {
    let rows = db
        .rows(&format!(
            "SELECT \"idx\", \"body\" FROM \"$commandlog\" \
             WHERE \"space\" = {} AND \"actor\" = {} {tail}",
            sql_lit(&space.0),
            sql_lit(RESTORE_ACTOR)
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let idx: usize = row
            .first()
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| PoolError::Db("restore marker row has no index".into()))?;
        let claims = || {
            PoolError::db(format!(
                "$commandlog index {idx} is attributed to `{RESTORE_ACTOR}` but its body \
                 carries no restore marker"
            ))
        };
        let body = bytea_of(row.get(1).and_then(|c| c.as_ref())).ok_or_else(claims)?;
        let group = decode_group(&body).map_err(|e| {
            PoolError::db(format!(
                "$commandlog index {idx} is attributed to `{RESTORE_ACTOR}` and its body \
                 does not decode: {e}"
            ))
        })?;
        let marker = group.restore.ok_or_else(claims)?;
        out.push((idx, marker));
    }
    Ok(out)
}

/// Which stored marker vouches for an orphan at position `upto`, if any:
/// **the bound rule**.
///
/// A marker's `pre_restore_head` is the operator's attestation of what the
/// chain said before the restore, so a record at or below that head is one
/// the restore is known to have discarded, and the marker explains it:
/// `upto <= pre_restore_head.idx + 1`. A marker whose `pre_restore_head`
/// is `None` explains nothing — absent means unattested, and an orphan it
/// cannot bound is an orphan nobody vouched for.
///
/// Every marker is considered, not only the ones a walk passed, because
/// the anchor can sit above a marker. The lowest explaining marker index
/// is returned, so the answer does not move when a later restore is
/// recorded.
pub fn explained_by(markers: &[(usize, RestoreMarker)], upto: u64) -> Option<usize> {
    markers
        .iter()
        .filter(|(_, m)| {
            m.pre_restore_head
                .is_some_and(|(idx, _)| upto <= idx.saturating_add(1))
        })
        .map(|(idx, _)| *idx)
        .min()
}

/// A restore marker the verifier walked through: where it sits and what it
/// says. A lawful event, reported beside a green verification; it is not a
/// [`TamperKind`], because a lawful event must not share a type with an
/// alarm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discontinuity {
    pub idx: usize,
    pub marker: RestoreMarker,
}

/// Whether every record of this space's head is one the stored chain still
/// agrees with, and if not, whether a restore marker explains the ones it
/// contradicts. The boot assertion (R2d) refuses to serve `Rewound`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continuity {
    /// Nothing recorded, or the chain agrees with everything recorded.
    Intact,
    /// The chain contradicts a recorded head, and a restore explains it.
    Explained {
        /// The marker's index. When the record was a witness row, this is
        /// the index the `restore` row names, which is its `upto` less one.
        marker_idx: usize,
        checkpoint_upto: u64,
        source: RecordSource,
    },
    /// The chain contradicts a recorded head that nothing attests: history
    /// moved backwards without saying so.
    Rewound {
        head: Option<usize>,
        checkpoint_upto: u64,
        checkpoint_head: ChainHash,
        source: RecordSource,
    },
}

/// Which database held the record the chain contradicts.
///
/// Printed, because the two are different findings with different remedies
/// and an operator has to know which table to look in. A `Stored` record is
/// in the space's own `$checkpoints` and survived a rewind that acted on
/// rows; a `Witness` record is in the control database, and it is the only
/// one that survives a point-in-time restore of the space's whole database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordSource {
    /// `$checkpoints`, in the space's own database.
    Stored,
    /// `$chainwitness`, in the control database.
    Witness,
}

impl std::fmt::Display for RecordSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RecordSource::Stored => "the space's own $checkpoints",
            RecordSource::Witness => "the control database",
        })
    }
}

impl std::fmt::Display for Continuity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Continuity::Intact => f.write_str("intact"),
            Continuity::Explained {
                marker_idx,
                checkpoint_upto,
                source,
            } => write!(
                f,
                "the checkpoint at {checkpoint_upto} recorded in {source} is orphaned, and the \
                 restore at index {marker_idx} accounts for it"
            ),
            Continuity::Rewound {
                head,
                checkpoint_upto,
                checkpoint_head,
                source,
            } => write!(
                f,
                "{source} recorded a checkpoint at {checkpoint_upto} (head {checkpoint_head}) \
                 that this space no longer reaches; the head is {}. Record the restore \
                 (`ordo restore --record`) or restore the missing suffix",
                match head {
                    Some(idx) => idx.to_string(),
                    None => "nothing — the chain is empty".to_string(),
                }
            ),
        }
    }
}

/// Does the stored chain agree with every checkpoint stored beside it?
///
/// Ruled 2026-09-19 (`orphan-checkpoints`): every stored checkpoint is
/// classified by [`agreement`], and an orphan needs an explanation. A
/// marker's `pre_restore_head` is what explains one ([`explained_by`]); a
/// marker that attests nothing explains nothing. The highest unexplained
/// orphan is the `Rewound` answer, the highest explained one is
/// `Explained`, and a chain with no orphan at all is `Intact`.
/// `BelowFloor` rows are neither: the chain has no opinion to offer about
/// a position both of whose confirming entries are pruned.
///
/// The old rule read the newest checkpoint only, which is how a later
/// rewind below a post-restore checkpoint passed as explained.
///
/// No signature is checked here: this is the question "did the chain move
/// backwards unannounced", and a checkpoint row is evidence of a length
/// whoever signed it saw. `verify` is what checks signatures.
///
/// # The witness, and the two explanations (R2d-ii)
///
/// `$checkpoints` lives in the space's own database and rolls back with a
/// point-in-time restore of it, so reading it alone answers `Intact` on a
/// deployment that has just lost history. Every witness `checkpoint` row is
/// therefore classified too, and it is the one record such a restore cannot
/// reach.
///
/// The two kinds of record get the two explanations they can each support.
///
/// - **A witness row is explained by a `restore` row with a greater `seq`**
///   whose own `(upto, head)` the chain agrees with, which says the marker
///   that row names is in the chain now. This is the reading a bound rule
///   cannot give: a checkpoint witnessed *after* a restore was signed over
///   the history the chain holds now, so a later rewind below it is fresh,
///   however far below an old marker's attested head it sits.
/// - **A stored row is explained through its witness copy** when one exists,
///   matched on `(upto, head)`, and otherwise by the bound rule over the
///   markers. The fallback is what a control database restored to before the
///   row was witnessed leaves available.
pub async fn continuity(
    db: Db<'_>,
    witness: &crate::witness::Witness,
    space: &SpaceId,
) -> Result<Continuity, PoolError> {
    use crate::witness::WitnessKind;

    let stored = stored_checkpoints(db, space).await?;
    let witnessed = witness.rows(space).await?;
    if stored.is_empty() && witnessed.is_empty() {
        return Ok(Continuity::Intact);
    }
    let markers = restore_markers(db, space).await?;
    let head = head_of(db, space).await?.map(|(idx, _)| idx);

    // A `restore` row explains a witnessed checkpoint written before it,
    // provided the marker it names is in the chain now. Collected once,
    // because the same handful of rows answers for every record.
    let mut restores = Vec::new();
    for w in witnessed.iter().filter(|w| w.kind == WitnessKind::Restore) {
        if agreement(db, space, w.upto, w.head).await? == Agreement::Current {
            restores.push((w.seq, w.upto));
        }
    }

    // Every record, both tables, newest position first, so the first answer
    // either way is about the highest position.
    struct Record {
        upto: u64,
        head: ChainHash,
        source: RecordSource,
        seq: Option<u64>,
    }
    let mut records: Vec<Record> = stored
        .iter()
        // A stored row with a witness copy is one fact recorded twice, and
        // the copy is the stronger record. Judging it once, as the witness
        // row, is what makes the `seq` rule apply to it.
        .filter(|cp| {
            !witnessed.iter().any(|w| {
                w.kind == WitnessKind::Checkpoint && w.upto == cp.upto && w.head == cp.head
            })
        })
        .map(|cp| Record {
            upto: cp.upto,
            head: cp.head,
            source: RecordSource::Stored,
            seq: None,
        })
        // Every witness row, both kinds. A `restore` row states that the
        // chain reached the marker's position with the marker's hash, so a
        // chain that no longer holds the marker contradicts it, and that is
        // a rewind that took a recorded restore with it.
        .chain(witnessed.iter().map(|w| Record {
            upto: w.upto,
            head: w.head,
            source: RecordSource::Witness,
            seq: Some(w.seq),
        }))
        .collect();
    records.sort_by(|a, b| b.upto.cmp(&a.upto).then(b.head.0.cmp(&a.head.0)));

    let mut explained: Option<Continuity> = None;
    for r in &records {
        if agreement(db, space, r.upto, r.head).await? != Agreement::Orphaned {
            continue;
        }
        // Both explanations name a marker index; the restore row's is its
        // `upto` less one.
        let by = match r.seq {
            Some(seq) => restores
                .iter()
                .filter(|(s, _)| *s > seq)
                .map(|(_, upto)| upto.saturating_sub(1) as usize)
                .min()
                .or_else(|| {
                    // **The bound rule is available only until a restore is
                    // recorded after this head was witnessed.** A checkpoint
                    // whose `seq` is above a `restore` row was signed over
                    // the history that existed *after* that restore, so no
                    // recorded restore accounts for it, however far below
                    // some older marker's attested head it sits. That is the
                    // reading `seq` gives and a bound over the markers
                    // cannot. Below every restore row, or with none recorded
                    // at all, the marker is still the explanation, which is
                    // what a control database restored to before this row
                    // was witnessed leaves available.
                    let recorded_earlier = witnessed
                        .iter()
                        .any(|w| w.kind == WitnessKind::Restore && w.seq < seq);
                    if recorded_earlier {
                        None
                    } else {
                        explained_by(&markers, r.upto)
                    }
                }),
            None => explained_by(&markers, r.upto),
        };
        match by {
            Some(marker_idx) => {
                explained.get_or_insert(Continuity::Explained {
                    marker_idx,
                    checkpoint_upto: r.upto,
                    source: r.source,
                });
            }
            None => {
                return Ok(Continuity::Rewound {
                    head,
                    checkpoint_upto: r.upto,
                    checkpoint_head: r.head,
                    source: r.source,
                })
            }
        }
    }
    Ok(explained.unwrap_or(Continuity::Intact))
}

// ── every record of a head, classified (R2d-iv) ──────────────────────────

/// Where a record of "this chain reached `upto` with head `head`" came
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOrigin {
    /// `$checkpoints`, in the space's own database.
    Stored,
    /// `$chainwitness`, in the control database.
    Witness { seq: u64 },
    /// An `ordo.attestation/1` document a receiver holds, supplied as a
    /// file. Its index in the order the operator gave them.
    Receiver { index: usize },
}

impl std::fmt::Display for RecordOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordOrigin::Stored => f.write_str("this space's $checkpoints"),
            RecordOrigin::Witness { seq } => write!(f, "the control database (seq {seq})"),
            RecordOrigin::Receiver { index } => write!(f, "receiver attestation {index}"),
        }
    }
}

/// What accounts for an orphaned record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Explanation {
    /// A restore marker in the chain attests a pre-restore head at or above
    /// this position.
    Marker { idx: usize },
    /// A `restore` row written after this record was witnessed, whose own
    /// marker the chain still holds.
    RestoreRow { seq: u64 },
}

impl std::fmt::Display for Explanation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Explanation::Marker { idx } => write!(f, "restore at index {idx}"),
            Explanation::RestoreRow { seq } => write!(f, "restore row {seq}"),
        }
    }
}

/// One record of a head, and what the stored chain says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordVerdict {
    pub origin: RecordOrigin,
    pub upto: u64,
    pub head: ChainHash,
    pub agreement: Agreement,
    /// What accounts for an orphan. `None` on a `Current` or `BelowFloor`
    /// record, and on an orphan nothing accounts for — which is the finding
    /// `ordo verify` exits 3 on.
    pub explained_by: Option<Explanation>,
}

/// Classify every record of this space's head: the stored checkpoints, the
/// control database's copies, and the attestations the receivers hold.
///
/// **A receiver's record is explained by the bound rule only.** No `seq`
/// exists outside the deployment, and that asymmetry is the point rather
/// than a limitation: a receiver's head the chain has moved away from, with
/// no marker attesting a pre-restore head at or above it, is a restore
/// nobody recorded, whatever the deployment's own records say about each
/// other.
///
/// `witness` is `None` when the operator did not name a control database.
/// The answer then omits those rows and says so at the call site; it does
/// not pretend they agreed.
pub async fn classify_records(
    db: Db<'_>,
    witness: Option<&crate::witness::Witness>,
    space: &SpaceId,
    receivers: &[(u64, ChainHash)],
) -> Result<Vec<RecordVerdict>, PoolError> {
    use crate::witness::WitnessKind;

    let markers = restore_markers(db, space).await?;
    let witnessed = match witness {
        Some(w) => w.rows(space).await?,
        None => Vec::new(),
    };
    let mut restores = Vec::new();
    for w in witnessed.iter().filter(|w| w.kind == WitnessKind::Restore) {
        if agreement(db, space, w.upto, w.head).await? == Agreement::Current {
            restores.push(w.seq);
        }
    }

    let mut out = Vec::new();
    for cp in stored_checkpoints(db, space).await? {
        out.push((RecordOrigin::Stored, cp.upto, cp.head, None));
    }
    for w in &witnessed {
        out.push((
            RecordOrigin::Witness { seq: w.seq },
            w.upto,
            w.head,
            Some(w.seq),
        ));
    }
    for (index, (upto, head)) in receivers.iter().enumerate() {
        out.push((RecordOrigin::Receiver { index }, *upto, *head, None));
    }

    let mut verdicts = Vec::with_capacity(out.len());
    for (origin, upto, head, seq) in out {
        let agreement = agreement(db, space, upto, head).await?;
        let explained_by = if agreement == Agreement::Orphaned {
            match seq {
                Some(seq) => restores
                    .iter()
                    .copied()
                    .filter(|s| *s > seq)
                    .min()
                    .map(|seq| Explanation::RestoreRow { seq })
                    .or_else(|| {
                        let recorded_earlier = witnessed
                            .iter()
                            .any(|w| w.kind == WitnessKind::Restore && w.seq < seq);
                        if recorded_earlier {
                            None
                        } else {
                            explained_by(&markers, upto).map(|idx| Explanation::Marker { idx })
                        }
                    }),
                None => explained_by(&markers, upto).map(|idx| Explanation::Marker { idx }),
            }
        } else {
            None
        };
        verdicts.push(RecordVerdict {
            origin,
            upto,
            head,
            agreement,
            explained_by,
        });
    }
    Ok(verdicts)
}

// ── verify ───────────────────────────────────────────────────────────────

/// Where verification started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Anchor {
    /// From the beginning of history. Correct only while the whole prefix
    /// is still stored.
    Genesis,
    /// From a checkpoint whose signature was checked against a published
    /// verifying key. The suffix is verified; the prefix is taken on the
    /// signature.
    Checkpoint {
        upto: usize,
        at: Timestamp,
        key_id: String,
    },
}

impl std::fmt::Display for Anchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Anchor::Genesis => f.write_str("genesis"),
            Anchor::Checkpoint { upto, at, key_id } => write!(
                f,
                "checkpoint upto={upto} at={} signed by `{key_id}` (signature verified)",
                at.0
            ),
        }
    }
}

/// What a green verification establishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub space: SpaceId,
    pub anchor: Anchor,
    /// Entries actually recomputed (0 when the anchor is already the head).
    pub checked: usize,
    /// The verified head, and its index — `None` for an empty chain.
    pub head: Option<(usize, ChainHash)>,
    /// Every restore marker the walk passed through, in chain order.
    pub discontinuities: Vec<Discontinuity>,
    /// Every stored checkpoint the chain contradicts at its position
    /// ([`Agreement::Orphaned`]), as `(upto, head)`: signed over history a
    /// restore discarded. Two rows may share a position since the
    /// `checkpoint-key` ruling, so the head is carried too. Classified on
    /// each run, listed, and kept; no column stores the verdict and
    /// nothing deletes the rows.
    ///
    /// Every entry here is one a restore marker explains: an orphan
    /// nothing attests fails verification rather than being listed.
    pub orphaned_checkpoints: Vec<(u64, ChainHash)>,
}

/// Why verification did not come back green.
#[derive(Debug)]
#[non_exhaustive]
pub enum VerifyFailure {
    /// The chain is bad, and here is where and how.
    Tamper(TamperAt),
    /// A stored checkpoint's signature did not check out under the key the
    /// operator supplied. Not folded into `Tamper`: it is a statement about
    /// the *anchor*, and continuing from it would be verifying a suffix
    /// against a head nobody has vouched for.
    UnverifiedCheckpoint {
        upto: u64,
        key_id: String,
    },
    /// The chain does not start where it must and no anchor was supplied —
    /// the prefix was pruned (lawfully) or dropped (not), and from genesis
    /// the two are indistinguishable. Naming it separately is what keeps a
    /// lawful prune from reading as tampering.
    PrefixMissing {
        first_stored: usize,
    },
    /// A stored checkpoint was signed over a head the chain no longer
    /// holds at that position, and no restore marker attests a
    /// pre-restore head at or above it: a restore nobody recorded.
    ///
    /// Judgement reads `$checkpoints` as data rather than trusting it, so
    /// a forged row fails verification naming itself rather than passing
    /// quietly. Reported from any anchor, because an orphan at or below
    /// the head is invisible to the truncation check.
    UnexplainedOrphan {
        upto: u64,
        head: ChainHash,
    },
    Store(PoolError),
}

impl std::fmt::Display for VerifyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyFailure::Tamper(t) => write!(f, "{t}"),
            VerifyFailure::UnverifiedCheckpoint { upto, key_id } => write!(
                f,
                "the stored checkpoint at position {upto} does not verify under key `{key_id}` \
                 — it is not an anchor, and a suffix verified against it would prove nothing"
            ),
            VerifyFailure::PrefixMissing { first_stored } => write!(
                f,
                "the chain's first stored entry is {first_stored}, not 0 — verify from a signed \
                 checkpoint, or this is a prune that cannot be told from a deletion"
            ),
            VerifyFailure::UnexplainedOrphan { upto, head } => write!(
                f,
                "the stored checkpoint at position {upto} was signed over head {head}, which \
                 the chain no longer holds there, and no restore marker attests a pre-restore \
                 head at or above it — record the restore (`ordo restore --record`) or restore \
                 the missing history"
            ),
            VerifyFailure::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for VerifyFailure {}

/// Pick the anchor: a stored checkpoint if there is one and the operator
/// can verify it, genesis otherwise.
///
/// An *unverifiable* checkpoint is a failure rather than a shrug back to
/// genesis. A deployment that has one either has the wrong key or has been
/// handed a forged marker, and both are things the operator must be told
/// before they read a green line.
///
/// **The anchor rule**, as `orphan-checkpoints` (2026-09-19) rules it: the
/// anchor is the **newest stored checkpoint the chain agrees with**.
/// Positions descending, walked until one is [`Agreement::Current`], which
/// is the first row on any chain that has never been restored. A row the
/// chain contradicts was signed over history a restore discarded, so its
/// head is not a hash the stored chain holds and anchoring there would
/// report every lawful restore as a truncation.
///
/// The markers are not read here. They explain orphans, in [`verify`] and
/// [`continuity`]; what anchors is decided by the chain itself, which is
/// what survives a prune that took the marker with it.
///
/// An agreeing checkpoint whose signature does not verify is still
/// `UnverifiedCheckpoint`: agreement is about position, not authority.
pub async fn choose_anchor(
    db: Db<'_>,
    space: &SpaceId,
    verifier: Option<&CheckpointVerifier>,
) -> Result<(Anchor, ChainHash), VerifyFailure> {
    let mut current = None;
    for cp in stored_checkpoints(db, space)
        .await
        .map_err(VerifyFailure::Store)?
    {
        if agreement(db, space, cp.upto, cp.head)
            .await
            .map_err(VerifyFailure::Store)?
            == Agreement::Current
        {
            current = Some(cp);
            break;
        }
    }
    let Some(cp) = current else {
        return Ok((Anchor::Genesis, ChainHash::GENESIS));
    };
    let Some(v) = verifier else {
        // No key: the checkpoint is data, not an anchor. Verify from
        // genesis and let the caller see which anchor was used.
        return Ok((Anchor::Genesis, ChainHash::GENESIS));
    };
    if !v.verify_checkpoint(&cp) {
        return Err(VerifyFailure::UnverifiedCheckpoint {
            upto: cp.upto,
            key_id: cp.key_id.clone(),
        });
    }
    Ok((
        Anchor::Checkpoint {
            upto: cp.upto as usize,
            at: cp.at,
            key_id: cp.key_id.clone(),
        },
        cp.head,
    ))
}

/// Recompute a space's chain from `anchor`, naming the first bad index.
pub async fn verify(
    db: Db<'_>,
    space: &SpaceId,
    anchor: Anchor,
    anchor_hash: ChainHash,
) -> Result<Verified, VerifyFailure> {
    let base = match &anchor {
        Anchor::Genesis => 0usize,
        Anchor::Checkpoint { upto, .. } => *upto,
    };
    let tamper = |index: usize, kind: TamperKind| {
        VerifyFailure::Tamper(TamperAt {
            space: space.clone(),
            index,
            kind,
        })
    };

    // Where the chain actually begins, relative to where the anchor says
    // it should. Settled before the arithmetic starts, because the three
    // answers are three different things to tell an operator and only one
    // of them is "tampered".
    match window(db, space, base, 1)
        .await
        .map_err(|i| tamper(i, TamperKind::Unreadable))?
        .first()
    {
        // Nothing at or after the anchor.
        None => {
            return match head_of(db, space).await.map_err(VerifyFailure::Store)? {
                // An empty chain verifies vacuously — but only from
                // genesis. A checkpoint over an empty chain is a marker
                // claiming history that is not there.
                None if base == 0 => Ok(Verified {
                    space: space.clone(),
                    anchor,
                    checked: 0,
                    head: None,
                    discontinuities: Vec::new(),
                    orphaned_checkpoints: judge_orphans(db, space, None).await?,
                }),
                None => Err(tamper(0, TamperKind::Unreadable)),
                // The anchor IS the head: the checkpoint covers everything
                // and there is no suffix to replay.
                Some((last, hash)) if last + 1 == base => Ok(Verified {
                    space: space.clone(),
                    anchor,
                    checked: 0,
                    head: Some((last, hash)),
                    discontinuities: Vec::new(),
                    orphaned_checkpoints: judge_orphans(db, space, Some(last)).await?,
                }),
                // **Truncation.** The checkpoint's signature says the chain
                // reached `base`; it does not. This is the one rewrite a
                // hash chain cannot notice on its own — every surviving
                // link is perfect — so it is checked here or not at all.
                Some((last, _)) => Err(tamper(last + 1, TamperKind::Unreadable)),
            };
        }
        // Stored, but not where the anchor points.
        Some(e) if e.idx != base => {
            return Err(if base == 0 {
                // A lawful prune and a deleted prefix look identical from
                // genesis, so this says so rather than crying tamper.
                VerifyFailure::PrefixMissing {
                    first_stored: e.idx,
                }
            } else {
                tamper(base, TamperKind::Unreadable)
            });
        }
        Some(_) => {}
    }

    let mut expected = anchor_hash;
    let mut next = base;
    let mut checked = 0usize;
    let mut head: Option<(usize, ChainHash)> = None;
    let mut discontinuities = Vec::new();
    loop {
        let batch = match window(db, space, next, VERIFY_BATCH).await {
            Ok(b) => b,
            // "Index 37 will not decode", never "somewhere in 0..512".
            // An operator cannot act on the second answer.
            Err(idx) => return Err(tamper(idx, TamperKind::Unreadable)),
        };
        if batch.is_empty() {
            break;
        }
        // Contiguity. A hole is not something the chain arithmetic can see
        // on its own — the entries either side of it are internally fine —
        // so the position of the missing entry is checked here, before the
        // link check reports the survivor after it.
        for (offset, e) in batch.iter().enumerate() {
            if e.idx != next + offset {
                return Err(tamper(next + offset, TamperKind::Unreadable));
            }
        }
        let slice: Vec<ChainedGroup> = batch.iter().map(|e| e.chained.clone()).collect();
        verify_chain_from(space, &slice, expected, next).map_err(VerifyFailure::Tamper)?;
        // Collected only once the window has verified: a marker is a
        // statement the chain vouches for, and one in an entry that did
        // not recompute is not a statement anybody made.
        discontinuities.extend(batch.iter().filter_map(|e| {
            e.chained
                .group
                .restore
                .clone()
                .map(|marker| Discontinuity { idx: e.idx, marker })
        }));
        let last = batch.last().expect("non-empty");
        expected = last.chained.hash;
        head = Some((last.idx, last.chained.hash));
        checked += batch.len();
        next = last.idx + 1;
        if batch.len() < VERIFY_BATCH {
            break;
        }
    }

    let orphaned_checkpoints = judge_orphans(db, space, head.map(|(idx, _)| idx)).await?;
    Ok(Verified {
        space: space.clone(),
        anchor,
        checked,
        head,
        discontinuities,
        orphaned_checkpoints,
    })
}

/// Classify every stored checkpoint against the chain and demand an
/// explanation for each orphan. Returns the explained orphans, ascending.
///
/// Run from **any** anchor, including genesis, and over `$checkpoints`
/// rather than over the markers the walk passed. That is what makes a
/// forged checkpoint row fail verification naming itself, and what catches
/// a truncation from genesis, where every surviving link recomputes and the
/// only evidence of the missing tail is a checkpoint row that outlasted it.
///
/// Where an unexplained orphan lands depends on whether the chain ever
/// reached it. Above the head it is the truncation finding, at `head + 1`,
/// which is the answer an operator has read there since 8i. At or below the
/// head the chain is long enough and holds another hash at that position,
/// which the truncation check cannot see, so it is
/// [`VerifyFailure::UnexplainedOrphan`].
async fn judge_orphans(
    db: Db<'_>,
    space: &SpaceId,
    head: Option<usize>,
) -> Result<Vec<(u64, ChainHash)>, VerifyFailure> {
    // The markers are read even when nothing is stored to judge. Reading
    // them is what refuses a row whose plaintext `actor` claims a marker its
    // body does not carry, and a chain with no checkpoint yet is exactly
    // where such a row would otherwise sit unexamined.
    let markers = restore_markers(db, space)
        .await
        .map_err(VerifyFailure::Store)?;
    let stored = stored_checkpoints(db, space)
        .await
        .map_err(VerifyFailure::Store)?;
    let reached = head.map_or(0, |idx| idx as u64 + 1);
    let mut orphans = Vec::new();
    for cp in stored {
        if agreement(db, space, cp.upto, cp.head)
            .await
            .map_err(VerifyFailure::Store)?
            != Agreement::Orphaned
        {
            continue;
        }
        if explained_by(&markers, cp.upto).is_none() {
            return Err(if cp.upto > reached {
                VerifyFailure::Tamper(TamperAt {
                    space: space.clone(),
                    index: reached as usize,
                    kind: TamperKind::Unreadable,
                })
            } else {
                VerifyFailure::UnexplainedOrphan {
                    upto: cp.upto,
                    head: cp.head,
                }
            });
        }
        orphans.push((cp.upto, cp.head));
    }
    // `stored_checkpoints` reads newest first; an operator reads a list of
    // positions upwards.
    orphans.reverse();
    Ok(orphans)
}

// ── checkpoint ───────────────────────────────────────────────────────────

/// The outcome of asking for a checkpoint.
#[derive(Debug)]
pub enum Checkpointed {
    /// A new signed marker at this head.
    Signed(Checkpoint),
    /// The head is already checkpointed at this position; nothing signed.
    /// Reported rather than silently no-oped, because "I signed nothing"
    /// and "I signed the head" are different facts to a cron job.
    AlreadyAt(u64),
}

#[derive(Debug)]
#[non_exhaustive]
pub enum CheckpointFailure {
    /// No history to mark.
    EmptyChain(SpaceId),
    /// The chain the checkpoint would anchor does not verify. Signing it
    /// would publish an operator's signature over a tampered history and
    /// hand every downstream receiver a marker that makes the damage look
    /// vouched for.
    WouldAnchorTamperedHistory(VerifyFailure),
    /// The control database would not take the witness row, so nothing was
    /// signed. An unwitnessed checkpoint is the thing `restore-witness`
    /// removes: a boot check that can only read `$checkpoints` cannot see a
    /// restore that rolled `$checkpoints` back with the chain.
    Unwitnessed(PoolError),
    /// The chain has moved backwards since something recorded its head, and
    /// no restore marker says so. Signing here would witness the rewind as
    /// current history and put the deployment's own signature on it.
    WouldWitnessARewind(Continuity),
    Store(PoolError),
}

impl std::fmt::Display for CheckpointFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointFailure::EmptyChain(s) => {
                write!(f, "space `{s}` has no history to checkpoint")
            }
            CheckpointFailure::WouldAnchorTamperedHistory(e) => write!(
                f,
                "refusing to sign a checkpoint over history that does not verify: {e}"
            ),
            CheckpointFailure::Unwitnessed(e) => write!(
                f,
                "nothing was signed: the checkpoint could not be recorded in the control \
                 database's `{}`, and a checkpoint no control-plane record vouches for cannot \
                 prove a restore did not happen — {e}",
                crate::witness::WITNESS_TABLE
            ),
            CheckpointFailure::WouldWitnessARewind(c) => write!(
                f,
                "refusing to sign a checkpoint over a chain that has moved backwards: {c} — \
                 record the restore (`ordo restore --record`) or restore the missing history"
            ),
            CheckpointFailure::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CheckpointFailure {}

/// Sign a checkpoint at the current head.
///
/// Three things happen under the space's **append lock**, and the lock is
/// why they mean anything together:
///
/// 1. the suffix since the last verified checkpoint is verified,
/// 2. the head is read,
/// 3. the marker is signed over that head and inserted.
///
/// Without the lock an append can land between (2) and (3) and the signed
/// `upto` names a different position than the signed head — a checkpoint
/// that verifies and anchors nothing. It is the *same* lock the append path
/// takes (`pg_advisory_xact_lock`, transaction-scoped), so a writer waits
/// rather than forking, and the commit that ends the checkpoint releases
/// it. The critical section is bounded by the checkpoint interval, not by
/// the length of history: verification starts at the previous checkpoint.
///
/// That last clause is the honest limit, and it is worth stating rather
/// than leaving to be discovered. A checkpoint over a space that already
/// has a verified one **inherits its prefix on that earlier signature** and
/// re-verifies only the suffix. It is the same rule that makes pruning
/// legal — a verified checkpoint is exactly the licence to stop replaying —
/// but it means a chain first checkpointed and then tampered with *below*
/// the marker will still take a new checkpoint. Catching that is
/// `verify --from-genesis`'s job, on a space whose prefix is still stored.
///
/// # The witness, and why the parameter is not optional
///
/// Ruled 2026-09-19 (`restore-witness`). Every signed head is also recorded
/// in the control database, **before** the checkpoint row and inside this
/// critical section, so that no stored checkpoint exists without a witness
/// row. An unwitnessed checkpoint is exactly the artifact the ruling
/// removes, so there is no shape of this call that produces one: a witness
/// write that fails rolls the transaction back and signs nothing
/// ([`CheckpointFailure::Unwitnessed`]). In a single-database deployment the
/// witness is a handle on the same pool; it is still written, because the
/// reading is the same and the deployment may grow a control plane later.
///
/// [`continuity`] runs before anything is signed. A chain restored under a
/// running process is then refused a checkpoint at the next tick
/// ([`CheckpointFailure::WouldWitnessARewind`]) rather than witnessed as
/// current, which is the one way a rewind could otherwise acquire the
/// deployment's own signature.
pub async fn sign_checkpoint(
    pool: &Pool,
    space: &SpaceId,
    signer: &Ed25519Signer,
    at: Timestamp,
    witness: &crate::witness::Witness,
) -> Result<Checkpointed, CheckpointFailure> {
    let tx = pool
        .begin_for(OpClass::AddressedWrite)
        .await
        .map_err(CheckpointFailure::Store)?;
    let db = Db::Tx(&tx);
    tx.rows(&format!(
        "SELECT pg_advisory_xact_lock({APPEND_LOCK_NS}, {})",
        space_lock_key(space)
    ))
    .await
    .map_err(CheckpointFailure::Store)?;

    match continuity(db, witness, space)
        .await
        .map_err(CheckpointFailure::Store)?
    {
        c @ Continuity::Rewound { .. } => return Err(CheckpointFailure::WouldWitnessARewind(c)),
        Continuity::Intact | Continuity::Explained { .. } => {}
    }

    // The signer verifies through its own public half, so a checkpointer
    // can check the previous marker without any extra configuration —
    // and a marker it cannot check is not used as an anchor.
    let verifier = signer.verifier();
    let (anchor, anchor_hash) = choose_anchor(db, space, Some(&verifier))
        .await
        .map_err(CheckpointFailure::WouldAnchorTamperedHistory)?;
    verify(db, space, anchor, anchor_hash)
        .await
        .map_err(CheckpointFailure::WouldAnchorTamperedHistory)?;

    let Some((last, head)) = head_of(db, space).await.map_err(CheckpointFailure::Store)? else {
        return Err(CheckpointFailure::EmptyChain(space.clone()));
    };
    let upto = last as u64 + 1;
    // **`AlreadyAt` means the head is already signed, not that the position
    // is taken.** After a restore the chain grows again through positions an
    // orphaned checkpoint already holds, and a row there signed over the
    // discarded history is not a signature over this head. Only a row the
    // chain agrees with says the work is done; an orphan is left where it is
    // and the new checkpoint is stored beside it, which is what the
    // `(space, upto, head)` key admits (`checkpoint-key`, 2026-09-20).
    if stored_checkpoints(db, space)
        .await
        .map_err(CheckpointFailure::Store)?
        .iter()
        .any(|cp| cp.upto == upto && cp.head == head)
    {
        return Ok(Checkpointed::AlreadyAt(upto));
    }
    let cp = Checkpoint {
        space: space.clone(),
        upto,
        head,
        at,
        key_id: signer.key_id().to_string(),
        signature: signer.sign(&Checkpoint::signed_bytes(space, upto, head, at)),
    };
    // The witness first, and on the control pool rather than in this
    // transaction: a routed deployment has no transaction across two
    // databases. See `witness`'s module doc for why this is the order the
    // two failure directions were chosen between.
    witness
        .record_checkpoint(&cp)
        .await
        .map_err(CheckpointFailure::Unwitnessed)?;
    tx.put_checkpoint(&cp)
        .await
        .map_err(CheckpointFailure::Store)?;
    tx.commit().await.map_err(CheckpointFailure::Store)?;
    Ok(Checkpointed::Signed(cp))
}

// ── record a restore (`plan-production-ops.md` R2c) ──────────────────────

/// What the operator is asserting about the restore they ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreRecord {
    /// The human, as `--as` names them. Not the actor: the actor is
    /// `system:restore`, because workers attribute as `system:<component>`.
    pub operator: String,
    pub source: RestoreSource,
    /// What the chain said before, when anybody can say. `None` is a legal,
    /// disclosed answer and it explains nothing: see [`explained_by`].
    pub pre_restore_head: Option<(u64, ChainHash)>,
    /// A real second restore to the same point, rather than a re-run.
    pub again: bool,
    /// Where history broke, when the operator knows and it is not the head.
    ///
    /// §2.4 defines `restored_from` as the last entry that survived the
    /// restore, and says it differs from the marker's `prev` when ordinary
    /// writes land before the operator records: a space with no record
    /// above the lost suffix keeps serving, because the boot assertion
    /// fires only when something proves the chain was longer. The chain
    /// cannot tell those writes from any other, so the operator says.
    /// `None` reads the head, which is the common case and the one where
    /// the two coincide.
    pub restored_from: Option<u64>,
}

/// What recording one did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub idx: usize,
    pub hash: ChainHash,
    pub restored_from: (u64, ChainHash),
    /// `idx - (restored_from.idx + 1)`: how many ordinary writes landed
    /// between the restore and the record. Non-zero is legal and printed,
    /// because then `restored_from` says where history broke and `prev`
    /// says where the marker was written, and both are true.
    pub gap: usize,
    /// What the security-event stream did with the joined `custody-action`.
    pub audit: String,
    /// Whether the control database took the `restore` witness row. The
    /// marker stands either way; an operator's re-run completes it.
    pub witnessed: bool,
    /// Which of the three defaults `pre_restore_head` came from, for the
    /// output line. `None` when the operator gave one.
    pub defaulted: Option<PreHeadSource>,
    /// Whether an attestation sink's cursor was moved back to the position
    /// history resumed at. `false` when there is no sink, which is most
    /// deployments.
    pub cursor_reset: bool,
}

/// Where a defaulted `pre_restore_head` came from. Printed, because "the
/// control database still holds the head this restore discarded" and "the
/// space's own table still holds a checkpoint, whatever its agreement" are
/// different strengths of evidence and the operator should see which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreHeadSource {
    /// The newest witnessed checkpoint. After a whole-database PITR this is
    /// exactly the head the restore discarded.
    Witness,
    /// The newest stored checkpoint, whatever its agreement.
    Stored,
    /// Nothing to default from: the marker is written unattested.
    Nothing,
}

impl std::fmt::Display for PreHeadSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PreHeadSource::Witness => "the newest witnessed checkpoint",
            PreHeadSource::Stored => "the newest stored checkpoint",
            PreHeadSource::Nothing => "nothing — unattested",
        })
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum RestoreError {
    /// An empty chain has no `restored_from` to name, so there is nothing
    /// a marker could truthfully say.
    NoHistory(SpaceId),
    /// The newest marker already resumes from this point. Almost always a
    /// re-run; a real second restore to the same point is what `--again`
    /// says.
    AlreadyRecorded {
        idx: usize,
    },
    /// `--restored-from` named a position the chain does not hold, so
    /// there is no hash to record and the marker would be a claim about
    /// nothing.
    NoSuchEntry {
        idx: u64,
    },
    /// The append gate refused the marker group.
    Refused(String),
    Store(PoolError),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestoreError::NoHistory(s) => write!(
                f,
                "space `{s}` has no history, so there is no restored-from point to record"
            ),
            RestoreError::AlreadyRecorded { idx } => write!(
                f,
                "the restore marker at index {idx} already resumes from this point — pass \
                 `--again` if this really is a second restore to the same point"
            ),
            RestoreError::NoSuchEntry { idx } => write!(
                f,
                "the chain holds no entry at index {idx}, so there is nothing for \
                 `--restored-from` to name"
            ),
            RestoreError::Refused(e) => write!(f, "the marker was refused: {e}"),
            RestoreError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RestoreError {}

/// Append a restore marker at the head, join it to the security stream, and
/// record it in the control database.
///
/// The three writes are deliberately not one act, because they cannot be:
/// the marker and the custody event share the space's transaction, and the
/// witness is in another database. The order is marker, then witness, and
/// what each failure leaves is stated rather than discovered.
///
/// - *The marker is refused or the commit fails.* Nothing happened. The
///   space is still refused at the door and the operator runs this again.
/// - *The witness write fails.* The marker stands, `witnessed` is false, and
///   the space stays refused, because the boot check reads the witness. A
///   re-run on the same point completes the missing row **without**
///   `--again`: `AlreadyRecorded` is answered only when the restore row
///   exists too, so a re-run is a repair rather than a second marker.
///
/// The operator attributes rather than authenticates. There is no step-up
/// and there cannot be one: the boot assertion refuses to serve the space
/// until this marker exists, so the door is closed exactly when the marker
/// is needed. The join to the security stream is the correlation.
pub async fn record_restore(
    pool: &Pool,
    witness: &crate::witness::Witness,
    space: &SpaceId,
    rec: RestoreRecord,
) -> Result<Recorded, RestoreError> {
    let tx = pool
        .begin_for(OpClass::AddressedWrite)
        .await
        .map_err(RestoreError::Store)?;
    let db = Db::Tx(&tx);
    tx.rows(&format!(
        "SELECT pg_advisory_xact_lock({APPEND_LOCK_NS}, {})",
        space_lock_key(space)
    ))
    .await
    .map_err(RestoreError::Store)?;

    let Some((last, head)) = head_of(db, space).await.map_err(RestoreError::Store)? else {
        return Err(RestoreError::NoHistory(space.clone()));
    };
    let restored_from = match rec.restored_from {
        None => (last as u64, head),
        Some(idx) => {
            let Some(hash) = entry_hash(db, space, idx as usize)
                .await
                .map_err(RestoreError::Store)?
            else {
                return Err(RestoreError::NoSuchEntry { idx });
            };
            (idx, hash)
        }
    };

    // A marker already resuming from this point is a re-run unless the
    // operator says otherwise. The witness row is completed either way, so
    // this refusal fires only when there is nothing left to repair.
    if !rec.again {
        if let Some((idx, m)) = latest_restore_marker(db, space)
            .await
            .map_err(RestoreError::Store)?
        {
            // Two readings of "this is a re-run", and either is enough.
            // The first is the plan's: the newest marker already resumes
            // from this point. The second is the one that catches the
            // ordinary accidental re-run, where the operator gave no
            // `--restored-from` and the first marker has itself moved the
            // head, so the point read from the chain is the marker: the
            // newest marker **is** the head, and nothing has happened
            // since it was written.
            if m.restored_from == restored_from || idx == last {
                let complete = witness
                    .rows(space)
                    .await
                    .map_err(RestoreError::Store)?
                    .iter()
                    .any(|w| {
                        w.kind == crate::witness::WitnessKind::Restore && w.upto == idx as u64 + 1
                    });
                if complete {
                    return Err(RestoreError::AlreadyRecorded { idx });
                }
                // The marker is there and the witness row is not: complete
                // it and report, rather than writing a second marker.
                let hash = entry_hash(db, space, idx)
                    .await
                    .map_err(RestoreError::Store)?
                    .unwrap_or(m.restored_from.1);
                drop(tx);
                let witnessed = witness
                    .record_restore(space, idx as u64, hash, &rec.operator, now_secs())
                    .await
                    .is_ok();
                return Ok(Recorded {
                    idx,
                    hash,
                    restored_from: m.restored_from,
                    gap: idx.saturating_sub(m.restored_from.0 as usize + 1),
                    audit: "not recorded (the marker was already in the chain)".into(),
                    witnessed,
                    defaulted: None,
                    cursor_reset: false,
                });
            }
        }
    }

    // **The default, in the order `restore-witness` gives it.** The witness
    // is preferred because after a whole-database PITR it is the one record
    // that still holds the head the restore discarded; the space's own
    // table is the fallback, whatever its agreement, because an orphan
    // there is still evidence of a length somebody signed.
    let (pre_restore_head, defaulted) = match rec.pre_restore_head {
        Some(h) => (Some(h), None),
        None => match witness
            .latest_checkpoint(space)
            .await
            .map_err(RestoreError::Store)?
        {
            Some(w) => (
                Some((w.upto.saturating_sub(1), w.head)),
                Some(PreHeadSource::Witness),
            ),
            None => match stored_checkpoints(db, space)
                .await
                .map_err(RestoreError::Store)?
                .first()
            {
                Some(cp) => (
                    Some((cp.upto.saturating_sub(1), cp.head)),
                    Some(PreHeadSource::Stored),
                ),
                None => (None, Some(PreHeadSource::Nothing)),
            },
        },
    };

    let correlation = ordo_secevent::correlation::mint();
    let group = CommandGroup {
        correlation: correlation.clone(),
        actor: Actor::System("restore".into()),
        commands: vec![],
        owner_administration: false,
        restored_unverified: false,
        restore: Some(RestoreMarker {
            space: space.clone(),
            restored_from,
            pre_restore_head,
            source: rec.source,
            operator: rec.operator.clone(),
        }),
    };
    let chained = tx
        .append_group(&group)
        .await
        .map_err(|e| RestoreError::Refused(format!("{e:?}")))?;
    let idx = last + 1;

    // The custody action, in the same transaction and under the group's own
    // correlation, so the two records join on one id.
    let audit = record_space_restored(&tx, space, &rec.operator, &correlation).await;
    tx.commit().await.map_err(RestoreError::Store)?;

    // The witness last, because the marker is the thing that has to exist
    // and a control database that is down must not stop it being written.
    let witnessed = witness
        .record_restore(space, idx as u64, chained.hash, &rec.operator, now_secs())
        .await
        .is_ok();

    // **The sink cursor** (R2e). `enqueue_due` decides what to send from
    // `(upto, head)` rather than from the cursor, so no delivery depends on
    // this number any more. It is the ops surface's reading of how far the
    // fan-out has reached, and after a restore the honest reading is the
    // position history resumed at: left where it was, an operator would read
    // a number describing history this chain no longer has.
    let cursor_reset = crate::webhook::reset_cursor(pool, space, restored_from.0 + 1)
        .await
        .unwrap_or(false);

    Ok(Recorded {
        idx,
        hash: chained.hash,
        restored_from,
        gap: idx.saturating_sub(restored_from.0 as usize + 1),
        audit,
        witnessed,
        defaulted,
        cursor_reset,
    })
}

/// The stored hash of one entry, or `None` when nothing is there.
async fn entry_hash(
    db: Db<'_>,
    space: &SpaceId,
    idx: usize,
) -> Result<Option<ChainHash>, PoolError> {
    Ok(db
        .rows(&format!(
            "SELECT \"hash\" FROM \"$commandlog\" WHERE \"space\" = {} AND \"idx\" = {idx}",
            sql_lit(&space.0)
        ))
        .await?
        .first()
        .and_then(|row| chain_hash_of(row.first().and_then(|c| c.as_ref()))))
}

/// The op a restore's custody record carries. Dotted, because the recorder
/// records any op with a space in it as `<unprintable>` (reported fix 25o).
pub const OP_RESTORE_RECORD: &str = "ordo.restore.record";

/// The `custody-action` half of the record, best effort and **said so**.
///
/// A restore that refused because an audit append failed would leave an
/// operator with a chain the platform will not serve and no lawful way to
/// unblock it. The stream is the record of the decision; the marker is the
/// mechanism, and the marker is in the chain by the time this runs.
async fn record_space_restored(
    tx: &TxConn,
    space: &SpaceId,
    operator: &str,
    correlation: &CorrelationId,
) -> String {
    let at = now_secs();
    let recorder = ordo_secevent::Recorder::default();
    recorder.observe(
        ordo_secevent::Observation {
            space: Some(space.clone()),
            subject: ordo_secevent::Subject::member(operator),
            op: OP_RESTORE_RECORD.into(),
            kind: ordo_secevent::EventKind::CustodyAction {
                action: "space_restored".into(),
            },
            at,
            correlation: ordo_secevent::correlation::adopt(&correlation.0),
        },
        ordo_secevent::Relationship::Member,
    );
    let entries = recorder.drain_all(at);
    let mut n = 0;
    for e in &entries {
        if let Err(err) = tx.append_security_entry(e).await {
            return format!("NOT RECORDED ({err}) — the marker is in the chain");
        }
        n += 1;
    }
    format!("recorded ({n} entry)")
}

fn now_secs() -> Timestamp {
    Timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    )
}

// ── revert ───────────────────────────────────────────────────────────────

/// What a revert calls itself where an op name is asked for.
///
/// There is no declared op behind a revert, because no author writes
/// "restore this row to this image", so the security stream is given the
/// name the operator typed rather than a registry entry. It is spelled once
/// here and used by both readers of it: the event this module raises when
/// an object could not be found, and the guard refusals, which name the
/// act a principal was not allowed to perform.
pub const REVERT_OP: &str = "revert";

/// Why a revert did not happen.
#[derive(Debug)]
#[non_exhaustive]
pub enum RevertFailure {
    /// The group could not be found, or its inverse could not be computed.
    Plan(RevertError),
    /// The group has already been reverted — its inverse is in the chain.
    /// The chain itself is what says so: `(space, correlation)` is unique
    /// and the inverse carries a derived correlation, so there is no second
    /// bookkeeping to fall out of step with the log.
    AlreadyReverted(CorrelationId),
    /// The platform refused, in its own vocabulary: `access_guard`,
    /// `cas_miss`, `not_found`, `has_referrers`, `expected_absent`,
    /// `blob_integrity`, or one of the two whole-class refusals
    /// (`unrevertable_sealed`, `unrevertable_tree`).
    Refused(String),
    Store(PoolError),
}

impl std::fmt::Display for RevertFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RevertFailure::Plan(e) => write!(f, "{e}"),
            RevertFailure::AlreadyReverted(c) => write!(
                f,
                "already_reverted: `{}` is in this space's chain, so the group it undoes has \
                 been reverted already",
                c.0
            ),
            RevertFailure::Refused(m) => f.write_str(m),
            RevertFailure::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RevertFailure {}

/// What a revert did: the new group, and where it landed in the chain.
#[derive(Debug, Clone)]
pub struct Reverted {
    pub original: CorrelationId,
    /// The group as **recorded** — its images are what the rows actually
    /// were and became, re-read from the database, not the plan's target
    /// values. A revert bumps `rev` forward like any write, so a recorded
    /// after-image copied from the plan would be wrong about the one field
    /// that says how many times the row has changed.
    pub group: CommandGroup,
    pub index: usize,
    pub hash: ChainHash,
    /// Objects this revert restored a handle to **without comparing
    /// anything**. Empty when every restored blob column verified, and
    /// empty by construction when no pipeline was handed in, because a
    /// caller that cannot verify has nothing to report about not having.
    ///
    /// Two reasons a comparison does not happen, and they are told apart
    /// rather than merged (see [`Unverified`]). The absent one is also a
    /// security event; the no-digest one is also
    /// `CommandGroup::restored_unverified` on [`Reverted::group`], the
    /// durable half, decided by `plan-opslog.md` D7. This field is the
    /// transient half: it exists for the caller that just made this call,
    /// which is why it carries the detail the trailer deliberately does
    /// not (which field, which handle, which reason) rather than only
    /// the one bit that rides the chain.
    pub unverified: Vec<UnverifiedObject>,
}

/// One restored blob column whose object was not compared against the
/// chain's recorded digest, and why.
#[derive(Debug, Clone)]
pub struct UnverifiedObject {
    /// The revert's own correlation, so one of these read on its own says
    /// which act it belongs to.
    pub correlation: CorrelationId,
    pub entity: String,
    pub row_id: String,
    pub field: String,
    /// The handle the restored image names, hex as the column carries it.
    pub handle: String,
    pub reason: Unverified,
}

/// Why nothing was compared. **Absence is not a failed comparison**, which
/// is the whole of `docs/plan-blob-chain.md` §4.3: there was nothing to
/// compare, so nothing was disproved, and neither of these refuses the
/// revert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unverified {
    /// The image carries no `"{field}$digest"` key: it was written before
    /// C5 put one there. No security event: see
    /// `CommandGroup::restored_unverified` (`plan-opslog.md` D7) for where
    /// this is durably recorded instead.
    NoRecordedDigest,
    /// There is no object, no `$blobref` row, or the store did not answer.
    /// Restored anyway, because the undo horizon must never depend on
    /// object-store availability, and reported to the security stream as
    /// `blob_object_absent`. Carries the store's or the platform's own
    /// words, for an operator who has to go and look.
    ObjectAbsent { detail: String },
}

impl std::fmt::Display for Unverified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unverified::NoRecordedDigest => {
                f.write_str("the image carries no recorded digest (written before C5)")
            }
            Unverified::ObjectAbsent { detail } => write!(f, "nothing was compared: {detail}"),
        }
    }
}

/// The inverse of a logged group, computed and checked but **not applied**
/// — `ordo revert --dry-run`. Under [`RevertPolicy::DEFAULT`].
pub async fn plan_revert(
    pool: &Pool,
    schema: &Schema,
    keys: Option<&dyn KeyProvider>,
    space: &SpaceId,
    correlation: &CorrelationId,
    principal: &str,
    now: Timestamp,
) -> Result<CommandGroup, RevertFailure> {
    plan_revert_with(
        pool,
        schema,
        keys,
        space,
        correlation,
        principal,
        now,
        &RevertPolicy::DEFAULT,
    )
    .await
}

/// [`plan_revert`] under the app developer's [`RevertPolicy`]. The amounts
/// it shows are converted past any money rescale since the group (25ad),
/// so a dry run refuses where the revert would.
#[allow(clippy::too_many_arguments)]
pub async fn plan_revert_with(
    pool: &Pool,
    schema: &Schema,
    keys: Option<&dyn KeyProvider>,
    space: &SpaceId,
    correlation: &CorrelationId,
    principal: &str,
    now: Timestamp,
    policy: &RevertPolicy,
) -> Result<CommandGroup, RevertFailure> {
    let (inverse, idx) = plan_inner(pool, schema, keys, space, correlation, principal, now).await?;
    let rescaled = rescaled_since(Db::Pool(pool), space, idx).await?;
    rescaled.convert(inverse, policy)
}

/// The inverse as the history holds it, unconverted, and the index of the
/// group it undoes.
async fn plan_inner(
    pool: &Pool,
    schema: &Schema,
    keys: Option<&dyn KeyProvider>,
    space: &SpaceId,
    correlation: &CorrelationId,
    principal: &str,
    now: Timestamp,
) -> Result<(CommandGroup, i64), RevertFailure> {
    let db = Db::Pool(pool);
    let entry = find_group(db, space, correlation).await?;
    let idx = group_index(db, space, correlation).await?;
    let inverse = invert_group(&entry.group, &Actor::User(principal.to_string()), now)
        .map_err(RevertFailure::Plan)?;
    refuse_unreproducible(schema, &inverse, keys.is_some())?;
    // Resolution is part of planning, not of applying: a payload this
    // deployment cannot produce refuses the revert before a single row is
    // locked. `--dry-run` therefore answers the question an operator is
    // actually asking — "will this work" — rather than "would it have
    // worked if the values were there".
    if let Some(keys) = keys {
        let custody = Custody::release(keys, Purpose::Write, space, &inverse.correlation)?;
        for cmd in &inverse.commands {
            for image in [cmd.before.as_ref(), cmd.after.as_ref()]
                .into_iter()
                .flatten()
            {
                resolve_sealed(Reader::Pool(pool), &custody, cmd, image).await?;
            }
        }
    }
    if find(db, space, &inverse.correlation).await?.is_some() {
        return Err(RevertFailure::AlreadyReverted(inverse.correlation.clone()));
    }
    Ok((inverse, idx))
}

/// Every `SealedRef` in one image, opened to its plaintext.
///
/// The two failure modes are the two the design names, and they are told
/// apart rather than merged: a payload that is not there (its group was
/// pruned) and a payload that will not open (its epoch key was destroyed).
/// Both refuse; neither is ever substituted with an empty value.
async fn resolve_sealed(
    db: Reader<'_>,
    custody: &Custody<'_>,
    cmd: &Command,
    image: &Image,
) -> Result<Plaintexts, RevertFailure> {
    let mut out = Plaintexts::new();
    for (field, value) in image {
        let Value::SealedRef(handle) = value else {
            continue;
        };
        let resolved = sealed::resolve(db, custody.keys, &custody.lease, &custody.space, handle)
            .await
            .map_err(RevertFailure::Store)?;
        match resolved {
            Resolved::Payload(plain) => {
                out.insert(field.clone(), plain);
            }
            other => {
                return Err(RevertFailure::Plan(RevertError::UnresolvableImage {
                    command: cmd.id.clone(),
                    field: field.clone(),
                    reason: other.refusal().expect("a non-payload answer has a reason"),
                }))
            }
        }
    }
    Ok(out)
}

/// Every `"{field}$digest"` key in one image, opened to its plaintext —
/// [`resolve_sealed`]'s narrower sibling, for [`verify_objects`].
///
/// `resolve_sealed` opens **every** `SealedRef` an image carries and
/// refuses the whole image if any one of them will not open, because a
/// revert needs the whole row's plaintext to write it back — a sealed
/// column it cannot restore is a row it cannot restore. `ordo verify
/// --objects` writes nothing back. It wants one column's digest, and an
/// image can carry a sealed column that has nothing to do with any blob
/// field (an ordinary `sealed` scalar, or a different row's history
/// walked in the same pass). Opening those too, and refusing the whole
/// walk when one of them will not — a re-seal in progress, or a member
/// whose epoch was destroyed on a row with no blob column at all — would
/// stop an object audit over a payload no blob verification was ever
/// going to touch.
///
/// A digest that will not open is not a refusal here either: the key is
/// simply left out of the returned map. [`verify_restored_objects`]
/// checks the **image** for `"{field}$digest"`, not this map, to tell "no
/// digest was ever recorded" (§4.2) from "one was recorded and it would
/// not open" — the second lands in [`Unverified::ObjectAbsent`], because
/// nothing was compared either way.
async fn resolve_digest_fields(
    db: Reader<'_>,
    custody: &Custody<'_>,
    image: &Image,
) -> Result<Plaintexts, RevertFailure> {
    let mut out = Plaintexts::new();
    for (field, value) in image {
        if !field.ends_with("$digest") {
            continue;
        }
        let Value::SealedRef(handle) = value else {
            continue;
        };
        let resolved = sealed::resolve(db, custody.keys, &custody.lease, &custody.space, handle)
            .await
            .map_err(RevertFailure::Store)?;
        if let Resolved::Payload(plain) = resolved {
            out.insert(field.clone(), plain);
        }
    }
    Ok(out)
}

/// Apply the inverse of a logged group as a **new attributed group**, in
/// one transaction, with the chain append inside it.
///
/// # Who may revert
///
/// Whoever the space's own write guards would let write those rows, and
/// nobody else. A revert of somebody else's write is a write on their rows,
/// and there is no separate "undo" permission in the language to appeal to
/// — so `principal` is put in the guard slot and every row the revert
/// touches is checked against the write guard twice, before and after. A
/// principal who could not have made the change cannot unmake it, and the
/// refusal they get is the refusal an ordinary write would have given them.
///
/// # What this does not check, stated rather than implied
///
/// **`principal` is asserted, not authenticated.** This is an operator tool
/// running with the deployment's database credentials. There is no bearer
/// token to verify, no session, no step-up, no suspension check, no
/// capability allowlist, and no op-level access check — a revert is not an
/// op, so `AccessEngine::check(claims, space, op, now)` has nothing to be
/// called with. Anybody holding the credentials this needs could do all of
/// this and more with `psql`, so the guard evaluation is not a containment
/// boundary against them; what it buys is that a revert run **by mistake**
/// cannot quietly escalate, that the platform's own answer is what decides,
/// and that the audit record names a principal whose authority actually
/// covered the rows.
///
/// The honest summary for an operator: this checks **authorization** and
/// not **authentication**.
///
/// # Re-running is safe
///
/// There is no retry loop: a `40001` surfaces as a storage error and the
/// operator runs the command again. That is sound rather than lazy — if the
/// first attempt rolled back, nothing landed and the second attempt does
/// the work; if it committed, the second attempt is refused as
/// [`RevertFailure::AlreadyReverted`] by the chain's own uniqueness. The
/// two outcomes are distinguishable and neither is a double undo.
#[allow(clippy::too_many_arguments)]
pub async fn revert(
    pool: &Pool,
    schema: &Schema,
    keys: Option<&dyn KeyProvider>,
    blobs: Option<&BlobPipeline>,
    events: Option<&ordo_secevent::Recorder>,
    space: &SpaceId,
    correlation: &CorrelationId,
    principal: &str,
    now: Timestamp,
) -> Result<Reverted, RevertFailure> {
    revert_with(
        pool,
        schema,
        keys,
        blobs,
        events,
        space,
        correlation,
        principal,
        now,
        &RevertPolicy::DEFAULT,
    )
    .await
}

/// [`revert`] under the app developer's [`RevertPolicy`], which decides
/// whether a money amount converted past a rescale may be rounded (25ad).
#[allow(clippy::too_many_arguments)]
pub async fn revert_with(
    pool: &Pool,
    schema: &Schema,
    keys: Option<&dyn KeyProvider>,
    blobs: Option<&BlobPipeline>,
    events: Option<&ordo_secevent::Recorder>,
    space: &SpaceId,
    correlation: &CorrelationId,
    principal: &str,
    now: Timestamp,
    policy: &RevertPolicy,
) -> Result<Reverted, RevertFailure> {
    // The plan is computed outside the transaction on purpose: the log is
    // append-only, so a committed group's bytes never change, and reading
    // them early keeps the refusals that need no lock (unknown group,
    // unresolvable payloads, already reverted) out of the critical
    // section. The places history is read again inside it, in `apply`, so
    // the conversion and the revision check see the `$places` the write
    // does.
    let (plan, idx) = plan_inner(pool, schema, keys, space, correlation, principal, now).await?;
    let custody = match keys {
        Some(keys) => Some(Custody::release(
            keys,
            Purpose::Write,
            space,
            &plan.correlation,
        )?),
        None => None,
    };

    let tx = pool
        .begin_for(OpClass::MultiStatementGroup)
        .await
        .map_err(RevertFailure::Store)?;
    match apply(
        &tx,
        schema,
        custody.as_ref(),
        blobs,
        space,
        &plan,
        principal,
        now,
        (idx, policy),
    )
    .await
    {
        Ok((recorded, unverified)) => {
            let entry = match tx.append_group(&recorded).await {
                Ok(e) => e,
                Err(ordo_commandlog::AppendError::DuplicateCorrelation(c)) => {
                    let _ = tx.rollback().await;
                    return Err(RevertFailure::AlreadyReverted(c));
                }
                Err(e) => {
                    let _ = tx.rollback().await;
                    return Err(RevertFailure::Store(PoolError::db(e.to_string())));
                }
            };
            // Invalidation rides the same transaction as the effect, which
            // is the whole of "fires only on commit": a rollback below
            // un-enqueues it. The signal names the entity, never the rows.
            let mut touched: Vec<&str> = recorded
                .commands
                .iter()
                .map(|c| c.entity.as_str())
                .collect();
            touched.sort_unstable();
            touched.dedup();
            for entity in touched {
                tx.enqueue("invalidate", entity)
                    .await
                    .map_err(RevertFailure::Store)?;
                tx.notify("ordo.invalidate", entity)
                    .await
                    .map_err(RevertFailure::Store)?;
            }
            let index = index_of(Db::Tx(&tx), space, &recorded.correlation).await?;
            tx.commit().await.map_err(RevertFailure::Store)?;
            // After the commit, not before it. The event says an object
            // that history names could not be found while a revert
            // restored a handle to it, and a revert that rolled back
            // restored nothing. An alert naming an act that did not
            // happen is worse than none, because it is the kind an
            // operator learns to dismiss.
            record_absent_objects(events, space, principal, &recorded, &unverified, now);
            Ok(Reverted {
                original: correlation.clone(),
                group: recorded,
                index,
                hash: entry.hash,
                unverified,
            })
        }
        // Every refusal aborts the whole revert. A half-applied undo is
        // worse than none: it is a state no write ever produced.
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

/// The action string the security stream carries for an object a revert
/// restored a handle to and could not find.
///
/// Asserted by a test, because an operator's alerting keys off it and a
/// renamed action is a silently disabled alert. That is the argument
/// `bucket_ops.rs` already makes about its own three.
pub const BLOB_OBJECT_ABSENT: &str = "blob_object_absent";

/// The security half of §4.4: one `CustodyAction` per object that was not
/// there, and nothing for the skip.
///
/// **No new `EventKind` variant**, under the standing rule that a new
/// class is an oracle until argued. The action string carries the
/// distinction and the stream's vocabulary stays at fourteen labels. The
/// precedent for `CustodyAction` covering a platform act that writes no
/// entity row is `ordo_host::bucket_ops`, whose comment argues exactly
/// this reinterpretation: what is being recorded is a decision about a
/// space rather than a change to its rows, which is the other half of the
/// spine from the command log.
///
/// The skip case is deliberately absent from this function. An image that
/// carries no digest is a fact about the encoding's age, the security
/// stream is for what means something is wrong, and a stream that carried
/// both would train an operator to read neither.
fn record_absent_objects(
    events: Option<&ordo_secevent::Recorder>,
    space: &SpaceId,
    principal: &str,
    recorded: &CommandGroup,
    unverified: &[UnverifiedObject],
    now: Timestamp,
) {
    let Some(events) = events else {
        return;
    };
    for entry in unverified {
        if !matches!(entry.reason, Unverified::ObjectAbsent { .. }) {
            continue;
        }
        events.observe(
            ordo_secevent::Observation {
                space: Some(space.clone()),
                subject: ordo_secevent::Subject::member(principal),
                op: REVERT_OP.to_string(),
                kind: ordo_secevent::EventKind::CustodyAction {
                    action: BLOB_OBJECT_ABSENT.to_string(),
                },
                at: now,
                correlation: recorded.correlation.clone(),
            },
            ordo_secevent::Relationship::Member,
        );
    }
}

/// The chain position a just-appended correlation landed at.
async fn index_of(
    db: Db<'_>,
    space: &SpaceId,
    correlation: &CorrelationId,
) -> Result<usize, RevertFailure> {
    let rows = db
        .rows(&format!(
            "SELECT \"idx\" FROM \"$commandlog\" WHERE \"space\" = {} AND \"correlation\" = {}",
            sql_lit(&space.0),
            sql_lit(&correlation.0)
        ))
        .await
        .map_err(RevertFailure::Store)?;
    rows.first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            RevertFailure::Store(PoolError::Db("the appended group has no index".into()))
        })
}

/// The class refusals — see the module doc.
///
/// `have_keys` is what used to be assumed false for everybody. A sealed
/// field is no longer an unrevertable *class*: its image resolves through
/// the payload store, and whether a particular handle resolves is decided
/// per handle, in [`resolve_sealed`]. What remains a class refusal is a
/// caller with no key custody at all, because no payload it meets can be
/// opened — refused here, once, rather than one field at a time.
fn refuse_unreproducible(
    schema: &Schema,
    inverse: &CommandGroup,
    have_keys: bool,
) -> Result<(), RevertFailure> {
    for cmd in &inverse.commands {
        let Some(e) = schema.entity(&cmd.entity) else {
            return Err(RevertFailure::Refused(format!(
                "unknown_entity: this history names `{}`, which the loaded schema does not \
                 declare — the schema and the log disagree, and the revert is refused rather \
                 than guessed",
                cmd.entity
            )));
        };
        if !have_keys && e.fields.iter().any(|f| f.sealed) {
            return Err(RevertFailure::Refused(format!(
                "unrevertable_sealed: `{}` has sealed fields, and no key custody is wired \
                 into this caller — the payload store holds what the images name, but \
                 nothing here can open it. Restoring the readable fields and leaving the \
                 sealed ones at their post-write content would put the row in a state it \
                 was never in",
                e.name
            )));
        }
        if e.is_tree {
            return Err(RevertFailure::Refused(format!(
                "unrevertable_tree: `{}` is a tree, and its materialised path is physical \
                 rather than a field, so no image records it",
                e.name
            )));
        }
    }
    Ok(())
}

/// Find the group this revert undoes, scoped to its space.
async fn find_group(
    db: Db<'_>,
    space: &SpaceId,
    correlation: &CorrelationId,
) -> Result<ChainedGroup, RevertFailure> {
    if let Some(entry) = find(db, space, correlation).await? {
        return Ok(entry);
    }
    // Logged, but not here: a loud, distinct answer. A cross-space revert
    // attempt is worth alerting on, and answering it identically to a typo
    // hides the one case that is.
    let elsewhere = db
        .rows(&format!(
            "SELECT 1 FROM \"$commandlog\" WHERE \"correlation\" = {} AND \"space\" <> {} LIMIT 1",
            sql_lit(&correlation.0),
            sql_lit(&space.0)
        ))
        .await
        .map_err(RevertFailure::Store)?;
    Err(RevertFailure::Plan(if elsewhere.is_empty() {
        RevertError::UnknownGroup(correlation.clone())
    } else {
        RevertError::CrossSpace {
            correlation: correlation.clone(),
            requested: space.clone(),
        }
    }))
}

async fn find(
    db: Db<'_>,
    space: &SpaceId,
    correlation: &CorrelationId,
) -> Result<Option<ChainedGroup>, RevertFailure> {
    let rows = db
        .rows(&format!(
            "SELECT \"prev\", \"hash\", \"body\" FROM \"$commandlog\" \
             WHERE \"space\" = {} AND \"correlation\" = {}",
            sql_lit(&space.0),
            sql_lit(&correlation.0)
        ))
        .await
        .map_err(RevertFailure::Store)?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let bad = || RevertFailure::Store(PoolError::Db("stored group will not decode".into()));
    let prev = chain_hash_of(row.first().and_then(|c| c.as_ref())).ok_or_else(bad)?;
    let hash = chain_hash_of(row.get(1).and_then(|c| c.as_ref())).ok_or_else(bad)?;
    let body = bytea_of(row.get(2).and_then(|c| c.as_ref())).ok_or_else(bad)?;
    let group = decode_group(&body).map_err(|_| bad())?;
    Ok(Some(ChainedGroup { group, prev, hash }))
}

/// Apply the planned inverse, returning the group as it should be
/// **recorded** — with the images the rows actually took.
#[allow(clippy::too_many_arguments)]
async fn apply(
    tx: &TxConn,
    schema: &Schema,
    custody: Option<&Custody<'_>>,
    blobs: Option<&BlobPipeline>,
    space: &SpaceId,
    plan: &CommandGroup,
    principal: &str,
    now: Timestamp,
    (group_idx, policy): (i64, &RevertPolicy),
) -> Result<(CommandGroup, Vec<UnverifiedObject>), RevertFailure> {
    let db = Db::Tx(tx);
    // **A revert past a money rescale converts the amount** (25ad, ruled
    // 2026-10-06). Read inside this transaction, so the factor and the
    // revision shift are the ones the rows were rewritten by.
    let rescaled = rescaled_since(db, space, group_idx).await?;
    let converted = rescaled.convert(plan.clone(), policy)?;
    let plan = &converted;
    let mut recorded = Vec::with_capacity(plan.commands.len());
    let mut unverified = Vec::new();
    let mut restored: Vec<(String, String)> = Vec::new();
    let mut judged = judge_deletes(db, schema, custody, space, plan, principal, &rescaled).await?;
    for (idx, cmd) in plan.commands.iter().enumerate() {
        let entity = &cmd.entity;
        let row_id = &cmd.row_id;
        // The plan's ids are `rev-<the id of the command being undone>`, so
        // the recorded history carries its own provenance: the entry that
        // undid something names what it undid.
        let id = |kind, before, after| Command {
            id: cmd.id.clone(),
            space: space.clone(),
            entity: entity.clone(),
            row_id: row_id.clone(),
            kind,
            before,
            after,
            at: now,
        };
        match cmd.kind {
            // Undo of a create: the row must still be there, still be the
            // revision the create left, still be writable by the reverter,
            // and still be unreferenced by any plain reference.
            //
            // Rows the created row has gained through `owned` references
            // since go with it (owner ruling `owned-delete`, 2026-09-29;
            // checklist 25h). The revert of a create is a delete, and a
            // delete takes what the row owns: the access rows derived for
            // an entity with an `acl`, the items of an order. Each goes as
            // its own `Delete` command with its before-image, owned rows
            // before their owner, exactly as the forward cascade records
            // itself, so this revert can itself be reverted.
            //
            // Locks, revision and writability were judged in
            // [`judge_deletes`], before this loop deleted anything. What is
            // left to ask here is what only the moment of deletion can
            // answer: that no plain reference still points in.
            CommandKind::Delete => {
                let (current, doomed) = judged.remove(&idx).expect("every delete is judged");
                refuse_plain_referrers(db, schema, space, &doomed).await?;
                let doomed = delete_order(db, schema, space, doomed).await?;
                let mut nth = 0;
                for (e, i) in &doomed {
                    let is_root = e == entity && i == row_id;
                    let image = if is_root {
                        current.clone()
                    } else {
                        require_row(db, schema, custody, space, &plan.correlation, e, i).await?
                    };
                    db.rows(&format!(
                        "DELETE FROM {} WHERE {} AND {} = {}",
                        q_ident(e),
                        space_pred(space),
                        q_ident("id"),
                        sql_lit(i)
                    ))
                    .await
                    .map_err(RevertFailure::Store)?;
                    if is_root {
                        recorded.push(id(CommandKind::Delete, Some(image), None));
                    } else {
                        // Not in the group being undone, so no `rev-` id of
                        // its own to carry: it names the undo that took it.
                        nth += 1;
                        recorded.push(Command {
                            id: CommandId(format!("{}-owned-{nth}", cmd.id.0)),
                            space: space.clone(),
                            entity: e.clone(),
                            row_id: i.clone(),
                            kind: CommandKind::Delete,
                            before: Some(image),
                            after: None,
                            at: now,
                        });
                    }
                }
            }
            // Undo of a converge: write the before-image back, bumping
            // `rev` forward. The revert is new history, so the row's
            // revision counter moves on like it does for any other write —
            // restoring the old number would make the log claim the row had
            // been written one time fewer than it has.
            CommandKind::Converge => {
                lock_row(db, space, entity, row_id).await?;
                let current = require_row(
                    db,
                    schema,
                    custody,
                    space,
                    &plan.correlation,
                    entity,
                    row_id,
                )
                .await?;
                same_revision(
                    &current,
                    cmd.before.as_ref(),
                    entity,
                    row_id,
                    rescaled.shift(entity),
                )?;
                require_writable(db, schema, space, entity, row_id, principal, REVERT_OP).await?;
                let target = cmd.after.as_ref().expect("a converge has an after-image");
                // The sealed half of the target, opened under the revert's
                // own key release. Refused here — before the UPDATE — if a
                // payload is unreachable, so a partly-restorable row is
                // never partly restored.
                let plain = match custody {
                    Some(c) => resolve_sealed(Reader::Tx(tx), c, cmd, target).await?,
                    None => Plaintexts::new(),
                };
                // Beside that resolution, and for the same reason it is
                // here rather than after the UPDATE: an object that does
                // not match what the chain recorded refuses before any
                // write.
                verify_restored_objects(
                    db,
                    schema,
                    blobs,
                    custody,
                    space,
                    &plan.correlation,
                    entity,
                    row_id,
                    target,
                    &plain,
                    &mut unverified,
                )
                .await?;
                let sets = assignments(schema, custody, entity, target, &plain)?;
                db.rows(&format!(
                    "UPDATE {} SET {}, {} = {} + 1 WHERE {} AND {} = {}",
                    q_ident(entity),
                    sets.join(", "),
                    q_ident("rev"),
                    q_ident("rev"),
                    space_pred(space),
                    q_ident("id"),
                    sql_lit(row_id)
                ))
                .await
                .map_err(RevertFailure::Store)?;
                let after = require_row(
                    db,
                    schema,
                    custody,
                    space,
                    &plan.correlation,
                    entity,
                    row_id,
                )
                .await?;
                // The post-image obligation: the row as it NOW is must
                // still be inside the reverter's write view.
                require_writable(
                    db,
                    schema,
                    space,
                    entity,
                    row_id,
                    principal,
                    "the reverted row",
                )
                .await?;
                recorded.push(id(CommandKind::Converge, Some(current), Some(after)));
            }
            // Undo of a delete: the id must still be free, and the row the
            // revert creates must land inside the reverter's write view —
            // exactly what a create has to prove.
            CommandKind::Create => {
                if row_exists(db, space, entity, row_id).await? {
                    return Err(RevertFailure::Refused(format!(
                        "expected_absent: `{row_id}` ({entity}) exists again, so the delete this \
                         revert would undo has already been undone or redone"
                    )));
                }
                let target = cmd.after.as_ref().expect("a create has an after-image");
                let plain = match custody {
                    Some(c) => resolve_sealed(Reader::Tx(tx), c, cmd, target).await?,
                    None => Plaintexts::new(),
                };
                verify_restored_objects(
                    db,
                    schema,
                    blobs,
                    custody,
                    space,
                    &plan.correlation,
                    entity,
                    row_id,
                    target,
                    &plain,
                    &mut unverified,
                )
                .await?;
                let (mut cols, mut vals) = insert_columns(schema, custody, entity, target, &plain)?;
                // `$space` is a physical column, not a field, so no image
                // records it — and the row has to land in the space this
                // revert is running in rather than nowhere. The schema's
                // NOT NULL caught this the moment the column existed,
                // which is the argument for the column in miniature.
                cols.insert(0, q_ident("$space"));
                vals.insert(0, sql_lit(&space.0));
                db.rows(&format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    q_ident(entity),
                    cols.join(", "),
                    vals.join(", ")
                ))
                .await
                .map_err(RevertFailure::Store)?;
                let after = require_row(
                    db,
                    schema,
                    custody,
                    space,
                    &plan.correlation,
                    entity,
                    row_id,
                )
                .await?;
                // Checked once the whole group is back, not row by row:
                // a cascaded delete took an owner together with the rows
                // that give the reverter their access to it (the derived
                // access rows), and the owner comes back first. Judged one
                // row at a time, the owner would be refused for lack of
                // rows that are restored a moment later in this same
                // transaction. Any refusal rolls the whole revert back.
                restored.push((entity.clone(), row_id.clone()));
                recorded.push(id(CommandKind::Create, None, Some(after)));
            }
        }
    }
    for (entity, row_id) in &restored {
        require_writable(
            db,
            schema,
            space,
            entity,
            row_id,
            principal,
            "the restored row",
        )
        .await?;
    }
    // `plan-opslog.md` D7: at least one restore in this group had no
    // recorded digest to verify against. Only that reason sets the flag:
    // `Unverified::ObjectAbsent` already has a durable, retained home in
    // the security stream (`record_absent_objects`), and flagging the
    // group for it too would make one fact ride two artifacts that could
    // fall out of step. Group-level rather than per-restore: which
    // column, entity or handle is derivable from the group's own images
    // (see `CommandGroup::restored_unverified`), so the one bit this
    // decides is whether the space owner has anything to go looking for.
    let restored_unverified = unverified
        .iter()
        .any(|u| u.reason == Unverified::NoRecordedDigest);
    Ok((
        CommandGroup {
            correlation: plan.correlation.clone(),
            actor: plan.actor.clone(),
            commands: recorded,
            // Never inherited from the group being reverted, for the
            // reason `ordo_commandlog::invert_group` states: the flag
            // records how THIS call was authorized, not what it undid. A
            // revert of an owner's rule-6 grant is an ordinary attributed
            // command by whoever asked for the revert.
            owner_administration: false,
            restored_unverified,
            restore: None,
        },
        unverified,
    ))
}

/// Judge every planned delete **before the loop deletes anything**: lock
/// the row, image it, check its revision and the reverter's right to write
/// it, then find the rows it owns and check those too.
///
/// Up front rather than row by row for the same reason the create branch
/// checks its restored rows at the end: a board's create group carries the
/// creator's derived admin row, and the inverse deletes that row first.
/// Judged at its own turn, the board would then be refused for want of the
/// access row that was deleted a moment earlier in this same transaction,
/// by the same reverter, whose right to undo the whole group was never in
/// question. So the question is asked while every row is still there.
///
/// Returns, by position in the plan, the root's image and the rows the
/// delete takes: the root and the rows it owns, each owned row before its
/// owner. A row the plan already deletes by a command of its own is left
/// to that command rather than cascaded, so it is deleted once and recorded
/// under its own `rev-` id. Locks are taken in plan order, which is the
/// order the forward writes are undone in.
async fn judge_deletes(
    db: Db<'_>,
    schema: &Schema,
    custody: Option<&Custody<'_>>,
    space: &SpaceId,
    plan: &CommandGroup,
    principal: &str,
    rescaled: &Rescaled,
) -> Result<std::collections::HashMap<usize, (Image, Vec<(String, String)>)>, RevertFailure> {
    let planned: std::collections::HashSet<(String, String)> = plan
        .commands
        .iter()
        .filter(|c| c.kind == CommandKind::Delete)
        .map(|c| (c.entity.clone(), c.row_id.clone()))
        .collect();
    let mut judged = std::collections::HashMap::new();
    for (idx, cmd) in plan.commands.iter().enumerate() {
        if cmd.kind != CommandKind::Delete {
            continue;
        }
        let entity = &cmd.entity;
        let row_id = &cmd.row_id;
        lock_row(db, space, entity, row_id).await?;
        let current = require_row(
            db,
            schema,
            custody,
            space,
            &plan.correlation,
            entity,
            row_id,
        )
        .await?;
        same_revision(
            &current,
            cmd.before.as_ref(),
            entity,
            row_id,
            rescaled.shift(entity),
        )?;
        require_writable(db, schema, space, entity, row_id, principal, REVERT_OP).await?;
        let doomed: Vec<(String, String)> = owned_cascade(db, schema, space, entity, row_id)
            .await?
            .into_iter()
            .filter(|v| (v.0 == *entity && v.1 == *row_id) || !planned.contains(v))
            .collect();
        let owned: Vec<(String, String)> = doomed
            .iter()
            .filter(|(e, i)| !(e == entity && i == row_id))
            .cloned()
            .collect();
        for (e, i) in &owned {
            lock_row(db, space, e, i).await?;
        }
        refuse_unwritable_owned(db, schema, space, &owned, principal).await?;
        judged.insert(idx, (current, doomed));
    }
    Ok(judged)
}

/// `rev` as a stated belief about the row (§6 rule 9). The inverse was
/// computed against the image the original write left behind; if the row
/// has moved on since, restoring that image would silently discard whoever
/// wrote it last.
///
/// `shift` is how many money rescales since the group rewrote every row of
/// this entity, each moving `rev` by one and appending no group (25ad). The
/// row is expected at the recorded revision plus that. Any other write is a
/// command in the chain and moves it further, which is still `cas_miss`.
fn same_revision(
    current: &Image,
    recorded: Option<&Image>,
    entity: &str,
    row_id: &str,
    shift: i64,
) -> Result<(), RevertFailure> {
    let expected = recorded.and_then(|i| i.get("rev"));
    let Some(Value::Int(want)) = expected else {
        return Err(RevertFailure::Refused(format!(
            "unrevertable_image: the logged image of `{row_id}` ({entity}) records no revision, \
             so this revert cannot tell whether the row has been written since"
        )));
    };
    let want = &(want + shift);
    match current.get("rev") {
        Some(Value::Int(have)) if have == want => Ok(()),
        Some(Value::Int(have)) => Err(RevertFailure::Refused(format!(
            "cas_miss: `{row_id}` ({entity}) is at revision {have}, and the group being reverted \
             left it at {want} — something has written this row since, and reverting would \
             discard that write"
        ))),
        _ => Err(RevertFailure::Refused(format!(
            "unrevertable_image: `{row_id}` ({entity}) has no readable revision"
        ))),
    }
}

/// Locks this space's row, and only this space's row.
///
/// The module's own contract (see the module doc) is that the row is
/// locked before it is read, so the decision and the mutation share one
/// snapshot. A lock taken on another space's same-id row satisfies
/// Postgres without satisfying that contract: it leaves the row this
/// revert is actually about to write completely unlocked for the whole
/// transaction, open to a concurrent write the lock exists to rule out.
async fn lock_row(
    db: Db<'_>,
    space: &SpaceId,
    entity: &str,
    id: &str,
) -> Result<(), RevertFailure> {
    db.rows(&format!(
        "SELECT {} FROM {} WHERE {} AND {} = {} FOR UPDATE",
        q_ident("id"),
        q_ident(entity),
        space_pred(space),
        q_ident("id"),
        sql_lit(id)
    ))
    .await
    .map(|_| ())
    .map_err(RevertFailure::Store)
}

/// Is `id` inside `principal`'s **write** view?
///
/// The predicate comes from `ordo_sema::guard_filter_query`, which is the
/// write protocol's own source for this question — one definition, two
/// callers. An unguarded entity answers with bare existence, exactly as it
/// does there.
async fn require_writable(
    db: Db<'_>,
    schema: &Schema,
    space: &SpaceId,
    entity: &str,
    id: &str,
    principal: &str,
    what: &str,
) -> Result<(), RevertFailure> {
    if !is_writable(db, schema, space, entity, id, principal).await? {
        return Err(RevertFailure::Refused(format!(
            "access_guard: `{principal}` may not write `{id}` ({entity}) — {what} is outside \
             their write view"
        )));
    }
    Ok(())
}

/// [`require_writable`]'s question without its refusal, for the cascade,
/// whose refusal counts rows rather than naming one.
async fn is_writable(
    db: Db<'_>,
    schema: &Schema,
    space: &SpaceId,
    entity: &str,
    id: &str,
    principal: &str,
) -> Result<bool, RevertFailure> {
    let sql = match guard_filter_query(schema, entity, true, Some(principal))
        .map_err(|e| RevertFailure::Refused(format!("bind: {e}")))?
    {
        Some(q) => format!(
            "SELECT 1 FROM ({}) v WHERE v.{} = {}",
            // The guard view is scoped like every other statement: a
            // revert that read another tenant's rows to decide this one
            // would be the cross-space read the space column exists to
            // make impossible.
            ordo_plan::plan(&q, &ordo_plan::Space::Bound(space.0.clone()))
                .map_err(|e| RevertFailure::Refused(format!("plan: {e}")))?
                .sql
                .trim_end_matches(';'),
            q_ident("id"),
            sql_lit(id)
        ),
        // An unguarded entity has no guard view to plan, but the bare
        // existence check still has to answer about THIS space's row: an
        // unscoped check is satisfiable by a foreign space's same-id row,
        // which would let a revert past a write-view obligation it never
        // actually held in this space.
        None => format!(
            "SELECT 1 FROM {} WHERE {} AND {} = {}",
            q_ident(entity),
            space_pred(space),
            q_ident("id"),
            sql_lit(id)
        ),
    };
    let rows = db.rows(&sql).await.map_err(RevertFailure::Store)?;
    Ok(!rows.is_empty())
}

/// The first column of every row, where it is not null.
fn first_col(rows: Vec<crate::TextRow>) -> Vec<String> {
    rows.into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect()
}

/// **Owned rows die with their owner** (owner ruling `owned-delete`,
/// 2026-09-29): every row that references the doomed row through an
/// `owned` ref goes with it, transitively, the access rows Ordo derives
/// for an entity with an `acl` included. Returns the doomed rows in an
/// order that puts each owned row before its owner, so the root is last.
///
/// The same walk as the write protocol's (`ordo_conformance::write`,
/// `owned_cascade`), kept in step by `tests/owned_delete_revert.rs`. A
/// tree's own parent ref is not followed: a plain `del` of a tree node
/// with children refuses, `tree owned` or not, and so does its revert.
async fn owned_cascade(
    db: Db<'_>,
    schema: &Schema,
    space: &SpaceId,
    entity: &str,
    id: &str,
) -> Result<Vec<(String, String)>, RevertFailure> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut stack: Vec<(String, String, bool)> = vec![(entity.to_string(), id.to_string(), false)];
    // An explicit stack rather than recursion: an `async fn` that calls
    // itself needs a boxed future, and the order wanted (children before
    // their owner) is a post-order, which the `expanded` flag gives.
    while let Some((e, i, expanded)) = stack.pop() {
        if expanded {
            out.push((e, i));
            continue;
        }
        if !seen.insert((e.clone(), i.clone())) {
            continue;
        }
        stack.push((e.clone(), i.clone(), true));
        for other in &schema.entities {
            for f in &other.fields {
                let FieldTy::Ref {
                    target,
                    owned: true,
                    tree,
                } = &f.ty
                else {
                    continue;
                };
                if *target != e || (*tree && other.name == e) {
                    continue;
                }
                let kids = first_col(
                    db.rows(&format!(
                        "SELECT {} FROM {} WHERE {} AND {} = {} ORDER BY {} DESC",
                        q_ident("id"),
                        q_ident(&other.name),
                        space_pred(space),
                        q_ident(&f.column()),
                        sql_lit(&i),
                        q_ident("id"),
                    ))
                    .await
                    .map_err(RevertFailure::Store)?,
                );
                // Pushed in descending id order so they pop ascending.
                for k in kids {
                    stack.push((other.name.clone(), k, false));
                }
            }
        }
    }
    Ok(out)
}

/// The reverter must be allowed to delete every row the cascade takes,
/// not the created row alone, and the cascade is all or nothing: a partial
/// one would leave a row without its owner. The same rule the write
/// protocol applies (`refuse_unwritable_cascade`), with the same refusal,
/// which counts the rows and never names them.
async fn refuse_unwritable_owned(
    db: Db<'_>,
    schema: &Schema,
    space: &SpaceId,
    owned: &[(String, String)],
    principal: &str,
) -> Result<(), RevertFailure> {
    let mut blocked = 0;
    for (e, i) in owned {
        if !is_writable(db, schema, space, e, i, principal).await? {
            blocked += 1;
        }
    }
    if blocked > 0 {
        return Err(RevertFailure::Refused(format!(
            "access_guard: the rows owned by what you are deleting include {blocked} row{} \
             you may not delete — an owner does not go without what it owns, so none of it \
             is deleted",
            if blocked == 1 { "" } else { "s" }
        )));
    }
    Ok(())
}

/// **Plain refs restrict deletes** (§write rules 6): the platform owes
/// the caller its own vocabulary here, before the statement runs, rather
/// than a foreign-key constraint name after it. A referrer that is itself
/// going in the same cascade does not count; owned referrers always are,
/// so they are not looked for. A tree's own parent ref is looked for even
/// when owned, because the cascade does not follow it.
///
/// This also covers a row the cascade takes through an owned ref that
/// something outside the cascade still references: the whole revert
/// refuses, as the forward delete does.
async fn refuse_plain_referrers(
    db: Db<'_>,
    schema: &Schema,
    space: &SpaceId,
    doomed: &[(String, String)],
) -> Result<(), RevertFailure> {
    for (entity, id) in doomed {
        for other in &schema.entities {
            for f in &other.fields {
                let FieldTy::Ref {
                    target,
                    owned,
                    tree,
                } = &f.ty
                else {
                    continue;
                };
                if target != entity || (*owned && !(*tree && other.name == *entity)) {
                    continue;
                }
                let going: Vec<String> = doomed
                    .iter()
                    .filter(|v| v.0 == other.name)
                    .map(|v| sql_lit(&v.1))
                    .collect();
                let excluded = if going.is_empty() {
                    String::new()
                } else {
                    format!(" AND {} NOT IN ({})", q_ident("id"), going.join(", "))
                };
                // The referring table has its own `$space` column, and it is
                // that table's column that scopes this lookup: a row in
                // another space referencing an id that happens to collide with
                // this one must not block a delete this space's revert is
                // otherwise entitled to make.
                let rows = db
                    .rows(&format!(
                        "SELECT {} FROM {} WHERE {} AND {} = {}{excluded} LIMIT 1",
                        q_ident("id"),
                        q_ident(&other.name),
                        space_pred(space),
                        q_ident(&f.column()),
                        sql_lit(id)
                    ))
                    .await
                    .map_err(RevertFailure::Store)?;
                if let Some(r) = rows.first().and_then(|r| r.first().cloned().flatten()) {
                    return Err(RevertFailure::Refused(format!(
                        "has_referrers: `{id}` is referenced by `{r}` ({}.{}) — remove or repoint \
                         the referrers first",
                        other.name, f.name
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Put the doomed rows in an order a foreign key accepts: a row goes only
/// after every doomed row that references it, by any ref. The foreign keys
/// are not deferrable, so an owner deleted before a row that points at it
/// is a violation. Ties keep the order the rows came in, which is owned
/// rows first and the root last.
async fn delete_order(
    db: Db<'_>,
    schema: &Schema,
    space: &SpaceId,
    doomed: Vec<(String, String)>,
) -> Result<Vec<(String, String)>, RevertFailure> {
    if doomed.len() < 2 {
        return Ok(doomed);
    }
    let index: std::collections::HashMap<(String, String), usize> = doomed
        .iter()
        .enumerate()
        .map(|(i, v)| (v.clone(), i))
        .collect();
    // waits[j] = how many doomed rows must go before row j;
    // after[i] = the rows that wait on row i.
    let mut waits = vec![0usize; doomed.len()];
    let mut after: Vec<Vec<usize>> = vec![Vec::new(); doomed.len()];
    for (j, (entity, id)) in doomed.iter().enumerate() {
        for other in &schema.entities {
            for f in &other.fields {
                let FieldTy::Ref { target, .. } = &f.ty else {
                    continue;
                };
                if target != entity {
                    continue;
                }
                let ids: Vec<String> = doomed
                    .iter()
                    .filter(|v| v.0 == other.name)
                    .map(|v| sql_lit(&v.1))
                    .collect();
                if ids.is_empty() {
                    continue;
                }
                let referrers = first_col(
                    db.rows(&format!(
                        "SELECT {} FROM {} WHERE {} AND {} = {} AND {} IN ({})",
                        q_ident("id"),
                        q_ident(&other.name),
                        space_pred(space),
                        q_ident(&f.column()),
                        sql_lit(id),
                        q_ident("id"),
                        ids.join(", "),
                    ))
                    .await
                    .map_err(RevertFailure::Store)?,
                );
                for r in referrers {
                    if let Some(&i) = index.get(&(other.name.clone(), r)) {
                        if i != j {
                            waits[j] += 1;
                            after[i].push(j);
                        }
                    }
                }
            }
        }
    }
    let mut ready: std::collections::BinaryHeap<std::cmp::Reverse<usize>> = waits
        .iter()
        .enumerate()
        .filter(|(_, w)| **w == 0)
        .map(|(i, _)| std::cmp::Reverse(i))
        .collect();
    let mut out = Vec::with_capacity(doomed.len());
    while let Some(std::cmp::Reverse(i)) = ready.pop() {
        out.push(doomed[i].clone());
        for &j in &after[i] {
            waits[j] -= 1;
            if waits[j] == 0 {
                ready.push(std::cmp::Reverse(j));
            }
        }
    }
    if out.len() != doomed.len() {
        return Err(RevertFailure::Refused(
            "has_referrers: the rows this revert takes reference each other in a cycle — \
             remove or repoint the referrers first"
                .to_string(),
        ));
    }
    Ok(out)
}

/// Bare existence, with no image built.
///
/// Deliberately not `row_image(..).is_some()`: imaging a row with a sealed
/// field now *mints handles and stores payloads*, and the create branch
/// asks this question only in order to refuse. Building an image to throw
/// it away would leave payload rows behind for a revert that never
/// happened.
async fn row_exists(
    db: Db<'_>,
    space: &SpaceId,
    entity: &str,
    id: &str,
) -> Result<bool, RevertFailure> {
    let rows = db
        .rows(&format!(
            "SELECT 1 FROM {} WHERE {} AND {} = {}",
            q_ident(entity),
            space_pred(space),
            q_ident("id"),
            sql_lit(id)
        ))
        .await
        .map_err(RevertFailure::Store)?;
    Ok(!rows.is_empty())
}

#[allow(clippy::too_many_arguments)]
async fn require_row(
    db: Db<'_>,
    schema: &Schema,
    custody: Option<&Custody<'_>>,
    space: &SpaceId,
    correlation: &CorrelationId,
    entity: &str,
    id: &str,
) -> Result<Image, RevertFailure> {
    row_image(db, schema, custody, space, correlation, entity, id)
        .await?
        .ok_or_else(|| {
            RevertFailure::Refused(format!(
                "not_found: `{id}` ({entity}) is gone, so there is nothing here to revert"
            ))
        })
}

/// [`row_image`]'s [`ordo_schema::image::SealSink`].
///
/// `image_of_row` is synchronous: it decodes, it does not do I/O. But
/// minting a handle and writing its payload are exactly I/O, and here that
/// I/O is an `async` insert over a pooled connection. So `seal_ref` only
/// mints (a pure computation against the key provider, `mint_sealed_ref`
/// derives no further I/O of its own) and stages the envelope; `row_image`
/// flushes every staged payload with its own `.await` once the image is
/// built, inside the same transaction the row was read under. This is the
/// same staging shape `ordo-conformance`'s sink uses for the opposite
/// reason (there it is staged because the group has no correlation yet;
/// here the correlation is already known, but the sink itself still cannot
/// `.await`), and it is why "one payload write per sealed field, in this
/// transaction" survives the consolidation unchanged even though the
/// decoder that discovers which fields need one no longer runs the insert
/// itself.
struct UndoSealSink<'a> {
    custody: Option<&'a Custody<'a>>,
    /// A payload minted but not yet written: the mirror of the write
    /// protocol's own `PendingPayload`, staged for the same reason.
    staged: Vec<(
        ordo_crypto::SealedHandle,
        ordo_crypto::Epoch,
        String,
        String,
        String,
        Vec<u8>,
    )>,
}

impl ordo_schema::image::SealSink for UndoSealSink<'_> {
    fn seal_ref(
        &mut self,
        entity: &str,
        field: &str,
        anchor: &str,
        envelope: &[u8],
    ) -> Result<Value, ordo_schema::image::ImageError> {
        let epoch = ordo_crypto::envelope_epoch(envelope).ok_or_else(|| {
            ordo_schema::image::ImageError::Sink(format!(
                "`{entity}.{field}` holds no sealed envelope"
            ))
        })?;
        // With no custody a sealed field cannot be imaged at all, and the
        // image is refused rather than quietly shortened. This path is
        // unreachable in practice (`refuse_unreproducible` turns a
        // keyless caller away from a sealed entity before any row is
        // read), but a partial image is exactly the failure this module
        // exists to prevent, so it is refused here too rather than left
        // to a caller ordering.
        let Some(c) = self.custody else {
            return Err(ordo_schema::image::ImageError::Sink(format!(
                "unrevertable_sealed: `{entity}.{field}` cannot be imaged without key custody, \
                 and an image missing a field is not an image"
            )));
        };
        let handle = c
            .keys
            .mint_sealed_ref(&c.lease, epoch)
            .map_err(|e| ordo_schema::image::ImageError::Sink(format!("sealed ref: {e}")))?;
        self.staged.push((
            handle,
            epoch,
            entity.to_string(),
            field.to_string(),
            anchor.to_string(),
            envelope.to_vec(),
        ));
        Ok(Value::SealedRef(handle))
    }
}

/// `$blobref.integrity` for every non-null external blob column of `row`,
/// keyed by field name: the one extra read `docs/plan-blob-chain.md`
/// §4.1 asks for per image, run here because `ordo_schema::image` has no
/// I/O and `row_image` is the caller it names. The mirror of
/// `Fixture::blob_digests` in `ordo-conformance`, over an async `Db`
/// instead of a synchronous `WriteDb`; the rule the two obey is one rule,
/// stated once at `ordo_schema::image::is_external_blob`.
///
/// A non-null external blob column with no matching `$blobref` row
/// refuses rather than imaging silently without a digest, for the same
/// reason the write protocol's own lookup refuses: the only writer of
/// such a column inserts the reference before the column ever names it,
/// so a live column with nothing behind it is corruption. `Store` rather
/// than `Refused`, matching the class a decode failure already gets a few
/// lines below: a fact about the stored bytes, not a class this
/// applier's authorization vocabulary has a word for.
async fn blob_digests(
    db: Db<'_>,
    space: &SpaceId,
    e: &Entity,
    row: &[Option<String>],
) -> Result<BTreeMap<String, Vec<u8>>, RevertFailure> {
    let cspace = CryptoSpace(space.0.clone());
    let mut out = BTreeMap::new();
    for (f, cell) in ordo_schema::image::image_columns(e).iter().zip(row) {
        if !ordo_schema::image::is_external_blob(f) {
            continue;
        }
        let Some(raw) = cell else { continue };
        let handle = ordo_crypto::blobref::parse_handle(raw).ok_or_else(|| {
            RevertFailure::Store(PoolError::db(format!(
                "`{}.{}` does not hold a well-formed blob handle",
                e.name, f.name
            )))
        })?;
        let rows = db
            .rows(&ordo_crypto::blobref::lookup_sql(&cspace, &handle))
            .await
            .map_err(RevertFailure::Store)?;
        let bref = rows
            .first()
            .and_then(|r| ordo_crypto::blobref::blob_ref_of(r))
            .ok_or_else(|| {
                RevertFailure::Store(PoolError::db(format!(
                    "`{}.{}` names a blob handle with no `$blobref` row",
                    e.name, f.name
                )))
            })?;
        out.insert(f.name.clone(), bref.integrity);
    }
    Ok(out)
}

/// **Verification is comparing the chain's recorded digest against the
/// object that exists** (`docs/plan-blob-chain.md` §4.3, the owner's own
/// words). This is that comparison, run over the image a revert is about
/// to write back, before it writes anything.
///
/// The four steps are §4.3's four, and the order is what makes the check
/// worth its cost:
///
/// 1. the image's `"{field}$digest"` has already been opened to its 32
///    plaintext bytes by the `resolve_sealed` pass above, the same pass
///    that opens every other sealed key, which is why nothing new had to
///    learn what that key means;
/// 2. `$blobref` is read for the handle the image's column names, and its
///    sealed `size` opened for the object's plaintext length;
/// 3. that length decides the object's stored length, and a store that
///    reports a different one has already answered: the comparison fails
///    at zero transfer cost, inside [`BlobPipeline::verify_digest`];
/// 4. otherwise the object is fetched and hashed, a chunk at a time.
///
/// # What it does to each of the four outcomes
///
/// **Mismatch** refuses, before any write, class `blob_integrity`: the
/// object is there and it is not the object the chain recorded, which is
/// proof of corruption or tampering, and undo's posture everywhere else is
/// to refuse before writing rather than to write and complain.
///
/// **Absent or unreachable** restores and reports. Nothing was compared,
/// so nothing was disproved, and the undo horizon must never come to
/// depend on whether a bucket is answering today.
///
/// **No recorded digest** restores, reports, and raises no security event.
///
/// **Past the undo horizon** never arrives here at all: the horizon
/// refuses earlier and for its own reason (`prune::Floor`, and the sweep's
/// cutoff in `blobgc.rs`), which is the answer §4.3 asks to leave
/// untouched.
///
/// # Which images are checked
///
/// The ones the revert **writes back**: a converge's target and a
/// re-create's. Undoing a create deletes the row, restores no handle, and
/// makes no claim about any object, so there is nothing there for a
/// comparison to be about.
#[allow(clippy::too_many_arguments)]
async fn verify_restored_objects(
    db: Db<'_>,
    schema: &Schema,
    blobs: Option<&BlobPipeline>,
    custody: Option<&Custody<'_>>,
    space: &SpaceId,
    correlation: &CorrelationId,
    entity: &str,
    row_id: &str,
    image: &Image,
    plain: &Plaintexts,
    out: &mut Vec<UnverifiedObject>,
) -> Result<(), RevertFailure> {
    // Two handles, and verification needs both. No pipeline is the CLI's
    // shape until C7 wires one; no custody is the same caller, because the
    // recorded digest is reached through the payload store and the
    // reference row's length is an envelope. Either absence skips the
    // check whole and records nothing, which is the honest answer: a
    // caller that cannot reach an object has learned nothing by not
    // reaching it, and recording a skip here would say "this image carried
    // no digest" about images that carry one.
    let (Some(blobs), Some(c)) = (blobs, custody) else {
        return Ok(());
    };
    let e = schema.entity(entity).ok_or_else(|| {
        RevertFailure::Refused(format!(
            "unknown_entity: the schema does not declare `{entity}`"
        ))
    })?;
    let cspace = CryptoSpace(space.0.clone());

    for f in &e.fields {
        if !ordo_schema::image::is_external_blob(f) {
            continue;
        }
        // A column imaged null or absent restores nothing to verify. The
        // image is the row as it was, so a null here is the row genuinely
        // not naming an object.
        let Some(Value::Text(hex)) = image.get(&f.name) else {
            continue;
        };
        let record = |reason| UnverifiedObject {
            correlation: correlation.clone(),
            entity: entity.to_string(),
            row_id: row_id.to_string(),
            field: f.name.clone(),
            handle: hex.clone(),
            reason,
        };

        // §4.2's fourth case: an image written before C5 carries the
        // handle and no digest at all. That is a fact about the
        // encoding's age and not evidence about an object, so it
        // restores, records the skip, and raises nothing. Checked
        // against the **image**, not against `plain`, so this is told
        // apart from the next case below.
        let digest_key = format!("{}$digest", f.name);
        if !image.contains_key(&digest_key) {
            out.push(record(Unverified::NoRecordedDigest));
            continue;
        }
        // The image does carry a digest, and it is not in `plain`: the
        // caller attempted to resolve it and the payload would not open
        // (a pruned or crypto-erased sealed payload — [`resolve_sealed`]
        // refuses the whole call before this function ever runs, so a
        // revert never reaches this branch; [`resolve_digest_fields`]
        // does not, so `ordo verify --objects` does). Nothing was
        // compared, which is the absent case's rule and not a reason to
        // stop auditing the rest of the chain over one row's key.
        let Some(want) = plain.get(&digest_key) else {
            out.push(record(Unverified::ObjectAbsent {
                detail: "the image's recorded digest will not open".into(),
            }));
            continue;
        };

        let handle = ordo_crypto::blobref::parse_handle(hex).ok_or_else(|| {
            // Not a refusal class: the logged image holds something that
            // is not a handle, which is a fact about stored bytes rather
            // than about this principal's authority. Same class, and the
            // same reason, as `blob_digests` above.
            RevertFailure::Store(PoolError::db(format!(
                "the logged image for `{entity}.{}` does not hold a well-formed blob handle",
                f.name
            )))
        })?;

        let rows = db
            .rows(&ordo_crypto::blobref::lookup_sql(&cspace, &handle))
            .await
            .map_err(RevertFailure::Store)?;
        let Some(bref) = rows
            .first()
            .and_then(|r| ordo_crypto::blobref::blob_ref_of(r))
        else {
            out.push(record(Unverified::ObjectAbsent {
                detail: "no `$blobref` row names this handle".into(),
            }));
            continue;
        };

        // The plaintext length, from the reference row's own sealed
        // `size`. Opened under the row's stored `(entity, field)` rather
        // than the image's, for `sealed::resolve`'s reason: the envelope
        // is AAD-bound to the pair the pipeline sealed it with, and a
        // resolver must not have to be told which column it is opening
        // for.
        let object = ordo_crypto::hex(&handle);
        let Ok(size) = c.keys.open_at(
            &c.lease,
            &bref.entity,
            &bref.field,
            ordo_crypto::Anchor::Object(&object),
            &bref.size,
        ) else {
            // The reference is there and its metadata will not open, so
            // there is no length to check an object against. Nothing was
            // compared, which is the absent case's rule and not the
            // mismatch case's: a key this deployment no longer holds says
            // nothing about whether the bytes are intact.
            out.push(record(Unverified::ObjectAbsent {
                detail: "the reference row's sealed length will not open".into(),
            }));
            continue;
        };
        let Ok(plain_len) = std::str::from_utf8(&size)
            .unwrap_or_default()
            .parse::<u64>()
        else {
            out.push(record(Unverified::ObjectAbsent {
                detail: "the reference row's length does not decode".into(),
            }));
            continue;
        };

        // From the store the reference row names: the object may have been
        // written to a bucket the tenant has since detached or replaced
        // (ruled 2026-10-05, `detach-upload-race` ask 2).
        match blobs
            .in_store(&bref.store)
            .verify_digest(
                &cspace,
                &bref.entity,
                &bref.field,
                &handle,
                plain_len,
                want,
                &correlation.0,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return Err(RevertFailure::Refused(format!(
                    "blob_integrity: `{entity}.{}` on `{row_id}` names object {hex}, and the \
                     object that is there does not match the digest this history recorded for \
                     it. The revert is refused before any write, because restoring a handle \
                     to bytes that have changed would record a restore that did not happen",
                    f.name
                )))
            }
            // The store did not answer, or has no such object. Nothing was
            // compared, so nothing was disproved: restore, and say so.
            Err(ordo_blob::PipelineError::Store(e)) => {
                out.push(record(Unverified::ObjectAbsent {
                    detail: e.to_string(),
                }));
            }
            // Everything else is the object failing to open. Custody has
            // already proved, two statements above, that it holds what
            // this reference row was sealed under, so a chunk that will
            // not open is about the bytes and not about the keys. That
            // is the same evidence a digest mismatch is, arriving through
            // the AEAD tag instead of through the hash. A tampered object
            // must not restore quietly just because the tampering was
            // caught one layer lower down.
            Err(e) => {
                return Err(RevertFailure::Refused(format!(
                    "blob_integrity: `{entity}.{}` on `{row_id}` names object {hex}, and the \
                     object that is there did not open: {e}",
                    f.name
                )))
            }
        }
    }
    Ok(())
}

/// What `ordo verify --objects` establishes (`docs/plan-blob-chain.md`
/// §4.5): the offline form of §4.3, run over every image the chain has
/// ever stored rather than the one a revert is about to write back.
#[derive(Debug, Clone)]
pub struct ObjectsVerified {
    /// Every image the walk visited that named at least one external
    /// blob column — whether or not that column carried a digest, and
    /// whether or not the comparison found the object. "How much of the
    /// chain did this actually look at" is the first thing an operator
    /// asks about a check with no size cap (§7.3).
    pub images: usize,
    /// Every handle a digest could not be compared for, in the same
    /// shape and for the same two reasons [`Reverted::unverified`]
    /// carries them.
    pub unverified: Vec<UnverifiedObject>,
}

/// The correlation `verify_objects` releases its one key lease under.
///
/// Not a real command group's id, because this walk does not append one —
/// it is a read from end to end. A literal, the way `ordo-exec`'s own
/// read paths already spell a correlation they invented rather than
/// received (`lib.rs`'s `open_sealed` releases under `"read"` for the
/// identical reason).
const OBJECT_VERIFY_CORRELATION: &str = "verify-objects";

/// **The offline form of §4.3.** Walks `space`'s whole stored chain, in
/// chain order, and runs [`verify_restored_objects`] — unchanged, the
/// same four-outcome comparison a revert makes over the one image it is
/// about to write back — over every image any command in it carries.
/// `ordo verify --objects`'s engine, and the machinery it runs on is C6's:
/// this function adds the walk, not a second verifier.
///
/// # Why this does not live inside [`verify`]
///
/// Chain integrity ("the record is intact") and object integrity ("the
/// world still matches it") are independent claims (§4.5): a broken hash
/// link says nothing about whether last year's upload is still in the
/// bucket, and a rotted object says nothing about whether the chain that
/// named it is intact. Folding the two walks into one function would make
/// them report through one code path for no reason connecting them, and
/// would mean a caller who only wants one has to pay for both. `verify`
/// recomputes hashes and never opens a payload; this opens payloads and
/// never recomputes a hash. Both read [`window`] because both are "walk
/// the stored chain in order", and that is all they share.
///
/// # Why every image, not only the ones a revert would restore
///
/// A revert only ever checks the target it is about to write, because
/// that is the only image it is about to act on. There is no "about to
/// act on" here: the question this answers is whether every object the
/// chain has ever named is still the object it named, which is a claim
/// about a `before`-image exactly as much as an `after`-image.
///
/// # One key release for the whole walk
///
/// [`ordo_crypto::Purpose::Read`], never `Write` — this opens digests and
/// mints nothing. Released once, the same economy `export.rs` takes for
/// the same reason: a lease per image would make a custody audit of one
/// run of this command read as a storm rather than as one act.
///
/// # Stopping at the first mismatch
///
/// [`verify_restored_objects`] already refuses (`Err`) the instant a
/// digest disagrees with the object that is there, before comparing
/// anything else in that image. Propagating that `Err` here rather than
/// collecting it is what makes "the first mismatch" a property of the
/// walk's own order rather than something this function has to compute
/// separately: the walk is sequential in chain order, so the first `Err`
/// it produces names the first mismatching handle the chain holds.
/// Everything that was absent, unopenable or pre-digest before that point
/// is still in `unverified` when the error is returned — an operator who
/// gets a `FAILED` line has not lost the census of what else the run had
/// already found.
fn names_a_blob_column(schema: &Schema, entity: &str, image: &Image) -> bool {
    let Some(e) = schema.entity(entity) else {
        return false;
    };
    ordo_schema::image::image_columns(e)
        .iter()
        .filter(|f| ordo_schema::image::is_external_blob(f))
        .any(|f| matches!(image.get(&f.name), Some(Value::Text(_))))
}

pub async fn verify_objects(
    db: Db<'_>,
    schema: &Schema,
    blobs: &BlobPipeline,
    keys: &dyn KeyProvider,
    space: &SpaceId,
) -> Result<ObjectsVerified, RevertFailure> {
    let correlation = CorrelationId(OBJECT_VERIFY_CORRELATION.to_string());
    let custody = Custody::release(keys, Purpose::Read, space, &correlation)?;
    let reader = match db {
        Db::Pool(p) => Reader::Pool(p),
        Db::Tx(t) => Reader::Tx(t),
    };
    let mut images = 0usize;
    let mut unverified = Vec::new();
    let mut next = 0usize;
    loop {
        let batch = window(db, space, next, VERIFY_BATCH).await.map_err(|idx| {
            RevertFailure::Store(PoolError::db(format!(
                "index {idx} of `{space}`'s chain will not decode"
            )))
        })?;
        if batch.is_empty() {
            break;
        }
        let read = batch.len();
        for entry in &batch {
            let group = &entry.chained.group;
            for cmd in &group.commands {
                for image in [cmd.before.as_ref(), cmd.after.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    if names_a_blob_column(schema, &cmd.entity, image) {
                        images += 1;
                    }
                    let plain = resolve_digest_fields(reader, &custody, image).await?;
                    verify_restored_objects(
                        db,
                        schema,
                        Some(blobs),
                        Some(&custody),
                        space,
                        &group.correlation,
                        &cmd.entity,
                        &cmd.row_id,
                        image,
                        &plain,
                        &mut unverified,
                    )
                    .await?;
                }
            }
        }
        next += read;
        if read < VERIFY_BATCH {
            break;
        }
    }
    Ok(ObjectsVerified { images, unverified })
}

/// The full field image of one row, as the log records it — the same
/// shape, and now the same **complete** field set, the write protocol
/// images, and (C2) the same decoding rule: [`ordo_schema::image`] owns
/// which column, which scalar type, a sealed field's envelope-or-null
/// branch. What stays here is the I/O: the select runs over this
/// revert's own [`Db`], and a sealed field's envelope is written
/// immediately, inside the transaction the row was read under, rather
/// than staged for a later flush the way the write protocol's own image
/// is (§0.4 difference 2, unchanged by this consolidation).
///
/// Sealed fields used to be dropped from the SELECT, which made a recorded
/// revert a partial record of the row it left behind: reverting the revert
/// would have restored the readable half and silently kept the sealed half.
/// They are read now and imaged as handles, with the envelope stored in
/// this same transaction — one payload per image, a fresh random handle
/// each time, never deduplicated.
#[allow(clippy::too_many_arguments)]
async fn row_image(
    db: Db<'_>,
    schema: &Schema,
    custody: Option<&Custody<'_>>,
    space: &SpaceId,
    correlation: &CorrelationId,
    entity: &str,
    id: &str,
) -> Result<Option<Image>, RevertFailure> {
    let e = schema.entity(entity).ok_or_else(|| {
        RevertFailure::Refused(format!(
            "unknown_entity: the schema does not declare `{entity}`"
        ))
    })?;
    let rows = db
        .rows(&ordo_schema::image::image_select(e, id, &space.0))
        .await
        .map_err(RevertFailure::Store)?;
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let blob_digests = blob_digests(db, space, e, &row).await?;
    let mut sink = UndoSealSink {
        custody,
        staged: Vec::new(),
    };
    // A decode failure is a fact about the stored bytes, so it stays a
    // `Store` error, which is the class it had before C2. Only what the
    // sink refused is a refusal: the `Refused` vocabulary is a closed
    // list this module documents (`access_guard`, `cas_miss`, `not_found`,
    // `has_referrers`, `expected_absent`, `unrevertable_sealed`,
    // `unrevertable_tree`), and a new class in it is an oracle until
    // argued.
    let img = ordo_schema::image::image_of_row(e, &row, &blob_digests, &mut sink).map_err(
        |err| match err {
            ordo_schema::image::ImageError::Sink(msg) => RevertFailure::Refused(msg),
            other => RevertFailure::Store(PoolError::db(other.to_string())),
        },
    )?;
    for (handle, epoch, entity, field, anchor, envelope) in sink.staged {
        db.rows(&ordo_crypto::sealed::insert_sql(
            &CryptoSpace(space.0.clone()),
            &handle,
            epoch,
            &entity,
            &field,
            &anchor,
            &correlation.0,
            &envelope,
        ))
        .await
        .map_err(RevertFailure::Store)?;
    }
    Ok(Some(img))
}

/// `SET` list for a converge: every imaged field except the two the
/// platform owns. `id` is the address, not a value; `rev` moves forward on
/// its own.
fn assignments(
    schema: &Schema,
    custody: Option<&Custody<'_>>,
    entity: &str,
    image: &Image,
    plain: &Plaintexts,
) -> Result<Vec<String>, RevertFailure> {
    let mut out = Vec::new();
    for (name, value) in imaged_fields(schema, custody, entity, image, plain)? {
        out.push(format!("{} = {}", q_ident(&name), value));
    }
    if out.is_empty() {
        return Err(RevertFailure::Refused(format!(
            "unrevertable_image: the logged image for `{entity}` carries no field to restore"
        )));
    }
    Ok(out)
}

/// Columns and values for a re-create. `rev` is left to its `DEFAULT 1`:
/// the restored row is a new row as far as concurrency control is
/// concerned, and a caller holding a stale `rev` for the row that used to
/// live at this id must not have their compare-and-set succeed.
fn insert_columns(
    schema: &Schema,
    custody: Option<&Custody<'_>>,
    entity: &str,
    image: &Image,
    plain: &Plaintexts,
) -> Result<(Vec<String>, Vec<String>), RevertFailure> {
    let mut cols = vec![q_ident("id")];
    let mut vals = vec![sql_lit(match image.get("id") {
        Some(Value::Id(v)) | Some(Value::Text(v)) => v.as_str(),
        _ => {
            return Err(RevertFailure::Refused(format!(
                "unrevertable_image: the logged image for `{entity}` has no id to restore it \
                     under"
            )))
        }
    })];
    for (name, value) in imaged_fields(schema, custody, entity, image, plain)? {
        cols.push(q_ident(&name));
        vals.push(value);
    }
    Ok((cols, vals))
}

/// The imaged fields a revert writes: everything in the image that the
/// schema still declares, minus `id`, `rev` and a blob field's
/// `"{field}$digest"` sibling, as `(column, literal)`.
///
/// A field the schema no longer declares is a refusal, not a skip. The
/// schema and the history have diverged, and a "revert" that silently
/// dropped a column would report success for a row it only partly
/// restored. `"{field}$digest"` is the one key skipped rather than
/// refused for exactly the reason `id` and `rev` are: it names no column
/// (`$` cannot appear in a declared identifier), so it is a platform key
/// by construction, not a field the schema forgot. It still resolves to
/// plaintext in `resolve_sealed` above (that pass iterates the whole
/// image and does not know or care which keys are fields), and those
/// bytes are simply unused here: C5 records the digest and skips it; C6
/// is what reads it, before any write, to verify the object being
/// restored still matches what was uploaded.
///
/// # A sealed field contributes up to two columns
///
/// The handle in the image has already been opened to its plaintext
/// (`plain`), and what goes back into the row is a **fresh seal of that
/// plaintext under the current epoch** — not the envelope that was stored.
/// Three reasons, and each is load-bearing:
///
/// - A revert is a write, so its value should be sealed the way a write of
///   that value today would seal it. Writing an old envelope back would
///   quietly re-introduce a retired epoch into a live row and undo part of
///   a rotation that a revocation forced.
/// - The seal draws a fresh nonce, so restoring a value the row already
///   held does not produce the byte pattern it had before. Equal sealed
///   values are never equal bytes, at rest or in the log.
/// - The blind index has to be recomputed anyway. A `sealed indexed` field
///   carries a `$bidx` column that is a function of the plaintext; leaving
///   it at its post-write digest would strand the restored row where
///   equality can never reach it and would leave a `unique` constraint
///   guarding the wrong value. It is emitted here, beside the envelope,
///   from the same plaintext — never copied from history, which is exactly
///   why the payload store keeps payloads and not digests.
fn imaged_fields(
    schema: &Schema,
    custody: Option<&Custody<'_>>,
    entity: &str,
    image: &Image,
    plain: &Plaintexts,
) -> Result<Vec<(String, String)>, RevertFailure> {
    let e = schema.entity(entity).ok_or_else(|| {
        RevertFailure::Refused(format!(
            "unknown_entity: the schema does not declare `{entity}`"
        ))
    })?;
    let mut out = Vec::new();
    for (name, value) in image {
        if name == "id" || name == "rev" || name.ends_with("$digest") {
            continue;
        }
        let Some(f) = e.field(name) else {
            return Err(RevertFailure::Refused(format!(
                "unrevertable_image: the logged image names `{entity}.{name}`, which the loaded \
                 schema no longer declares"
            )));
        };
        if !f.sealed {
            out.push((f.column(), literal(value)?));
            continue;
        }
        // A sealed field imaged as null was null in the row: the column
        // clears, and an indexed one moves to the reserved null sentinel
        // rather than to SQL NULL — the same rule the write path applies
        // when an op assigns `null`, and for the same reason (a cleared
        // digest makes `unique` stop constraining and makes the row
        // unreachable by equality forever).
        let Some(bytes) = plain.get(name) else {
            if !matches!(value, Value::Null) {
                return Err(RevertFailure::Refused(format!(
                    "unrevertable_sealed: `{entity}.{name}` was not resolved before it was \
                     written — this is a bug in the revert applier, not a fact about the data"
                )));
            }
            out.push((f.column(), "NULL".to_string()));
            if f.indexed {
                out.push((f.index_column(), blind(custody, entity, f, None)?));
            }
            continue;
        };
        let Some(c) = custody else {
            return Err(RevertFailure::Refused(format!(
                "unrevertable_sealed: `{entity}.{name}` needs key custody to be written back"
            )));
        };
        // Sealed at the row it is written back to, which is the row the
        // image is of.
        let Some(Value::Id(row_id)) = image.get("id") else {
            return Err(RevertFailure::Refused(format!(
                "unrevertable_image: the logged image for `{entity}` names no row id to seal                  `{name}` at"
            )));
        };
        let envelope = c
            .keys
            .seal_at(
                &c.lease,
                entity,
                name,
                ordo_crypto::Anchor::Row(row_id),
                bytes,
            )
            .map_err(|e| RevertFailure::Refused(format!("seal: {e}")))?;
        out.push((
            f.column(),
            format!("'\\x{}'::bytea", ordo_crypto::hex(&envelope)),
        ));
        if f.indexed {
            let text = std::str::from_utf8(bytes).map_err(|_| {
                RevertFailure::Refused(format!(
                    "unrevertable_sealed: `{entity}.{name}` is indexed, and what its payload \
                     holds is not text"
                ))
            })?;
            out.push((f.index_column(), blind(custody, entity, f, Some(text))?));
        }
    }
    Ok(out)
}

/// The `$bidx` literal for a restored sealed value.
///
/// `normalize` is a schema fact applied before the digest, exactly as the
/// write path applies it: it decides what "the same value" means for this
/// field, so a revert that skipped it could restore a row that no longer
/// answers its own equality.
fn blind(
    custody: Option<&Custody<'_>>,
    entity: &str,
    f: &ordo_schema::Field,
    text: Option<&str>,
) -> Result<String, RevertFailure> {
    let Some(c) = custody else {
        return Err(RevertFailure::Refused(format!(
            "unrevertable_sealed: `{entity}.{}` is indexed and needs key custody to digest",
            f.name
        )));
    };
    let normalized = text.map(|t| match f.normalize.as_deref() {
        Some("lower") => t.to_lowercase(),
        _ => t.to_string(),
    });
    let value = match &normalized {
        Some(t) => ordo_crypto::BlindValue::Text(t),
        None => ordo_crypto::BlindValue::Null,
    };
    let digest = c
        .keys
        .blind_index(&c.space, entity, &f.name, value)
        .map_err(|e| RevertFailure::Refused(format!("blind index: {e}")))?;
    Ok(format!("'\\x{}'::bytea", ordo_crypto::hex(&digest)))
}

/// A recorded value as a SQL literal.
///
/// `SealedRef` still cannot appear here: sealed fields take the branch
/// above, which writes a *fresh seal of the resolved plaintext* rather than
/// a rendering of the handle. A handle reaching this arm would mean a
/// sealed field was routed through the plain path, which would write the
/// 16-byte handle into the column as if it were a value — so it is a
/// refusal rather than an invented rendering.
fn literal(v: &Value) -> Result<String, RevertFailure> {
    Ok(match v {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Text(s) | Value::Id(s) => sql_lit(s),
        // The canonical spelling is a plain numeric literal (27ak).
        Value::Decimal(d) => d.to_string(),
        Value::SealedRef(_) => {
            return Err(RevertFailure::Refused(
                "unrevertable_sealed: a sealed handle reached the plain-value path".into(),
            ))
        }
    })
}

// ── a revert past a change of places (25ad) ─────────────────────────────

/// How a revert may treat a money amount whose conversion to the field's
/// current places would lose digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyRounding {
    /// Refuse the revert, naming the field (`places_revert_loses_digits`).
    /// The default: an amount a revert cannot reproduce is refused, never
    /// approximated, which is this module's one rule.
    Refuse,
    /// Round half to even ("banker's rounding"): a remainder of exactly
    /// half goes to the even neighbour, anything else to the nearer one.
    ///
    /// Chosen over half-up because a revert may convert many amounts at
    /// once, and half-up pushes every tie the same way, so a total of
    /// rounded amounts drifts upward. Half-even has no such bias. It is
    /// also the rule most ledgers and IEEE 754 use by default, so an app
    /// developer reconciling against one sees the same answer.
    HalfEven,
}

/// What a revert may do that the app developer decides (owner ruling
/// 2026-10-06, `revert-past-rescale`: *"precision etc should be up to the
/// dev to allow/agree to"*).
///
/// **Layered, with a default.** [`RevertPolicy::rounding_for`] answers, in
/// order: the setting for that one field, then the setting for the whole
/// program, then the platform default, which is [`MoneyRounding::Refuse`].
///
/// The program has no spelling for it yet: `policy commandlog` is lexed and
/// not parsed (`definition-language.md`, `settings.md`), the same as
/// [`crate::cadence::LogPolicy`]. Until it is, a caller constructs this.
/// `revertRounding` is the key it would take in that block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertPolicy {
    /// The program-wide setting. `None` falls to the platform default.
    pub money_rounding: Option<MoneyRounding>,
    /// Per `(entity, field)`, over the program-wide setting.
    pub money_rounding_fields: BTreeMap<(String, String), MoneyRounding>,
}

impl RevertPolicy {
    /// The platform's default: no rounding anywhere.
    pub const DEFAULT: RevertPolicy = RevertPolicy {
        money_rounding: None,
        money_rounding_fields: BTreeMap::new(),
    };

    /// The rounding a revert may use on one field, layer by layer.
    pub fn rounding_for(&self, entity: &str, field: &str) -> MoneyRounding {
        self.money_rounding_fields
            .get(&(entity.to_string(), field.to_string()))
            .copied()
            .or(self.money_rounding)
            .unwrap_or(MoneyRounding::Refuse)
    }
}

impl Default for RevertPolicy {
    fn default() -> RevertPolicy {
        RevertPolicy::DEFAULT
    }
}

/// The class a revert refuses with when converting a money amount to the
/// field's current places would lose digits and rounding is not allowed.
pub const PLACES_REVERT_LOSES_DIGITS: &str = "places_revert_loses_digits";
/// The class when the converted amount does not fit a bigint.
pub const PLACES_REVERT_OVERFLOWS: &str = "places_revert_overflows";
/// The class when a `$places` row cannot be placed against the chain, so
/// the revert cannot tell whether the amount needs converting.
pub const PLACES_REVERT_UNPLACED: &str = "places_revert_unplaced";

/// What the money rescales applied since one group did, for the entities
/// and fields a revert of it touches.
///
/// **Money only** (ruled 2026-10-06): a `decimal` is stored as its value,
/// so a change of its places leaves nothing to convert. **A `keep` converts
/// nothing**: it stated that the stored number was meant as written, so the
/// rows were not rewritten and neither is the image.
#[derive(Debug, Default)]
struct Rescaled {
    /// Per entity: how many rescales rewrote every row, each moving `rev`.
    shifts: BTreeMap<String, i64>,
    /// Per `(entity, field)`: the net power of ten the stored amounts were
    /// multiplied by. A chain of rescales adds up: 2 to 4 then 4 to 3 is
    /// `+1`.
    exponents: BTreeMap<(String, String), i32>,
    /// Fields with a rescale this revert cannot place against the chain.
    unplaced: std::collections::BTreeSet<(String, String)>,
}

impl Rescaled {
    fn shift(&self, entity: &str) -> i64 {
        self.shifts.get(entity).copied().unwrap_or(0)
    }

    /// The plan with every restored image's money amounts converted to the
    /// field's current places. Only the images the revert writes: a
    /// converge's after-image and a create's. The before-images are what
    /// the revision check compares, and only their `rev` is read.
    fn convert(
        &self,
        mut plan: CommandGroup,
        policy: &RevertPolicy,
    ) -> Result<CommandGroup, RevertFailure> {
        if self.exponents.is_empty() && self.unplaced.is_empty() {
            return Ok(plan);
        }
        for cmd in &mut plan.commands {
            if cmd.kind == CommandKind::Delete {
                continue;
            }
            let Some(image) = cmd.after.as_mut() else {
                continue;
            };
            for (entity, field) in &self.unplaced {
                if entity == &cmd.entity && image.contains_key(field.as_str()) {
                    return Err(RevertFailure::Refused(format!(
                        "{PLACES_REVERT_UNPLACED}: `{entity}.{field}` had its places changed by \
                         a row of `$places` that records no chain position, so this revert \
                         cannot tell whether `{}`'s amount predates it",
                        cmd.row_id
                    )));
                }
            }
            for ((entity, field), exp) in &self.exponents {
                if entity != &cmd.entity || *exp == 0 {
                    continue;
                }
                let Some(value) = image.get_mut(field.as_str()) else {
                    continue;
                };
                let units = match &*value {
                    Value::Int(n) => *n,
                    Value::Text(t) => match t.parse::<i64>() {
                        Ok(n) => n,
                        Err(_) => continue,
                    },
                    _ => continue,
                };
                let rounding = policy.rounding_for(entity, field);
                let converted = convert_units(units, *exp, rounding).map_err(|why| {
                    RevertFailure::Refused(match why {
                        Unconvertible::LosesDigits => format!(
                            "{PLACES_REVERT_LOSES_DIGITS}: `{entity}.{field}` on `{}` was \
                             {units} minor units before a rescale by 10^{exp}, which does not \
                             divide exactly, and a revert never rounds. No setting changes \
                             that today. Write the amount you want with an ordinary command \
                             instead, at the field's current places",
                            cmd.row_id
                        ),
                        Unconvertible::Overflows => format!(
                            "{PLACES_REVERT_OVERFLOWS}: `{entity}.{field}` on `{}` was {units} \
                             minor units, and 10^{exp} times that does not fit a bigint",
                            cmd.row_id
                        ),
                    })
                })?;
                *value = match value {
                    Value::Int(_) => Value::Int(converted),
                    _ => Value::Text(converted.to_string()),
                };
            }
        }
        Ok(plan)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Unconvertible {
    LosesDigits,
    Overflows,
}

/// `units` minor units, scaled by `10^exp`. Up is exact or overflows; down
/// is exact, rounded half to even when allowed, or refused.
fn convert_units(units: i64, exp: i32, rounding: MoneyRounding) -> Result<i64, Unconvertible> {
    let factor = 10i128
        .checked_pow(exp.unsigned_abs())
        .ok_or(Unconvertible::Overflows)?;
    let n = i128::from(units);
    let out = if exp >= 0 {
        n.checked_mul(factor).ok_or(Unconvertible::Overflows)?
    } else {
        let (q, r) = (n / factor, n % factor);
        if r == 0 {
            q
        } else {
            match rounding {
                MoneyRounding::Refuse => return Err(Unconvertible::LosesDigits),
                MoneyRounding::HalfEven => {
                    let twice = 2 * r.abs();
                    let away = twice > factor || (twice == factor && q % 2 != 0);
                    if away {
                        q + n.signum()
                    } else {
                        q
                    }
                }
            }
        }
    };
    i64::try_from(out).map_err(|_| Unconvertible::Overflows)
}

/// The chain index of the group being reverted.
async fn group_index(
    db: Db<'_>,
    space: &SpaceId,
    correlation: &CorrelationId,
) -> Result<i64, RevertFailure> {
    let rows = db
        .rows(&format!(
            "SELECT \"idx\" FROM \"$commandlog\" WHERE \"space\" = {} AND \"correlation\" = {}",
            sql_lit(&space.0),
            sql_lit(&correlation.0)
        ))
        .await
        .map_err(RevertFailure::Store)?;
    rows.first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| RevertFailure::Store(PoolError::Db("the group has no index".into())))
}

/// The money rescales applied after the group at `idx`, from `$places`.
///
/// A rescale is after the group when the chain head it recorded for this
/// space is at or past the group: the group was committed before the
/// rescale took its table lock, so its images hold the old minor units. A
/// space with no recorded head had no group yet, so every one of its groups
/// came after.
async fn rescaled_since(db: Db<'_>, space: &SpaceId, idx: i64) -> Result<Rescaled, RevertFailure> {
    let history = crate::migrate::money_places_history(
        |sql: String| async move { db.rows(&sql).await },
        &space.0,
    )
    .await
    .map_err(RevertFailure::Store)?;
    let mut out = Rescaled::default();
    for r in history {
        if r.choice != "rescale" || r.from == r.to {
            continue;
        }
        match r.head {
            Err(()) => {
                out.unplaced.insert((r.entity, r.field));
            }
            Ok(Some(head)) if idx <= head => {
                *out.shifts.entry(r.entity.clone()).or_default() += 1;
                *out.exponents.entry((r.entity, r.field)).or_default() +=
                    i32::from(r.to) - i32::from(r.from);
            }
            Ok(_) => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod places_tests {
    use super::*;

    #[test]
    fn up_is_exact() {
        assert_eq!(convert_units(2999, 2, MoneyRounding::Refuse), Ok(299_900));
        assert_eq!(convert_units(-5, 1, MoneyRounding::Refuse), Ok(-50));
    }

    #[test]
    fn down_refuses_lost_digits_unless_allowed() {
        assert_eq!(convert_units(2990, -1, MoneyRounding::Refuse), Ok(299));
        assert_eq!(
            convert_units(2999, -1, MoneyRounding::Refuse),
            Err(Unconvertible::LosesDigits)
        );
    }

    #[test]
    fn half_even_sends_ties_to_the_even_neighbour() {
        let r = |n, e| convert_units(n, e, MoneyRounding::HalfEven).unwrap();
        assert_eq!(r(2999, -1), 300);
        assert_eq!(r(2994, -1), 299);
        assert_eq!(r(2995, -1), 300);
        assert_eq!(r(2985, -1), 298);
        assert_eq!(r(-2985, -1), -298);
        assert_eq!(r(-2995, -1), -300);
        assert_eq!(r(-2996, -1), -300);
        assert_eq!(r(150, -2), 2);
        assert_eq!(r(250, -2), 2);
    }

    #[test]
    fn too_large_overflows() {
        assert_eq!(
            convert_units(i64::MAX / 2, 1, MoneyRounding::Refuse),
            Err(Unconvertible::Overflows)
        );
    }

    #[test]
    fn the_field_layer_wins_over_the_program_layer_over_the_default() {
        let mut p = RevertPolicy::DEFAULT;
        assert_eq!(p.rounding_for("invoices", "total"), MoneyRounding::Refuse);
        p.money_rounding = Some(MoneyRounding::HalfEven);
        assert_eq!(p.rounding_for("invoices", "total"), MoneyRounding::HalfEven);
        p.money_rounding_fields
            .insert(("invoices".into(), "total".into()), MoneyRounding::Refuse);
        assert_eq!(p.rounding_for("invoices", "total"), MoneyRounding::Refuse);
        assert_eq!(p.rounding_for("invoices", "fee"), MoneyRounding::HalfEven);
    }
}
