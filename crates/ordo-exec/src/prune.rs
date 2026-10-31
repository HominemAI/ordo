//! The pruning floor, the consumer cursors that hold it, and the prune.
//!
//! Pruning is the one operation in the platform that **destroys audit
//! evidence on purpose**, so almost all of this module is the argument for
//! why a given delete is lawful, and only a few lines are the delete.
//!
//! `security-logging-design.md` §7 gives the rule in three parts, and each
//! is implemented here as written rather than approximated:
//!
//! ```text
//! floor = greatest checkpoint position ≤ min(live consumer positions,
//!                                            position at now − undoHorizon)
//! ```
//!
//! - **A checkpoint is not a competing lower bound — it is the only legal
//!   cut point.** Verification resumes from a checkpoint, and a suffix that
//!   begins mid-chain is unanchored. So the floor is never "wherever the
//!   consumers are"; it is the greatest *checkpoint* at or below that.
//!   [`cadence::Attestation`](crate::cadence::Attestation)s live in their own
//!   table precisely so this query cannot accidentally find one.
//! - **`min()` over positions and a duration is ill-typed.** The duration is
//!   resolved to a position first — see [`horizon_position`].
//! - **The floor never passes the slowest registered consumer**, where
//!   "registered" is exactly the four kinds in [`ConsumerKind`] and only one
//!   of them is evictable.
//!
//! There is a fourth bound the specification does not state and the platform
//! needs anyway: **a prune never removes the chain's last entry**. It is not
//! a policy choice — an empty chain is read as *"never written"* from genesis
//! and as *"tampered"* from its own checkpoint, so a full prune would make a
//! lawful operation report both of the two things it is not. See
//! [`FloorHolder::RetainedTail`].
//!
//! # Resolving `now − undoHorizon` to a position, without touching the spine
//!
//! `$commandlog` has no timestamp column. Every command carries its own
//! `at`, inside the canonically encoded `body` the chain hashed. Adding a
//! column would be a migration on the audit spine to make a background job
//! cheaper, which is the wrong trade: the column would then be a second
//! statement of when a group happened, unhashed, editable, and disagreeing
//! with the body the moment anybody touched it.
//!
//! So [`horizon_position`] scans forward in `VERIFY_BATCH` windows, decodes
//! each entry, and **stops at the first entry any of whose commands has
//! `at > cutoff`**. That entry's index is the horizon, exclusive.
//!
//! The trade is written down because it is a real one: the scan is monotone
//! by construction and therefore **conservative under clock skew**. A group
//! whose `at` runs ahead of its neighbours — two replicas, one drifting
//! clock — stops the scan early, and the floor comes out lower than a
//! timestamp index would have put it. That under-prunes. Under-pruning
//! costs disk; over-pruning destroys history inside the undo horizon, which
//! is a guarantee. The scan takes the direction that can only be wrong
//! about cost.
//!
//! # The layer-1 attestation pin is not protected by the floor, and cannot be
//!
//! This is the interaction worth being explicit about, because the floor
//! rule reads as though it covers everybody and it does not.
//!
//! A client pin (`ordo_api::attest::HeadPin`, produced by `ordo-host`'s
//! `PgAttestation`) is `(upto, head, key_id, signature)`, kept by the
//! client. `security-logging-design.md` is emphatic that layer 1 "asks
//! nothing of anyone", is "always on", and "requires no setup" — which
//! means, structurally, that **the deployment does not know a pin exists**.
//! No pin is a registered consumer; no pin appears in the `min()` above; the
//! floor rule protects cursors the platform was told about, and a pin is
//! deliberately not one of those.
//!
//! What follows:
//!
//! - **A pin at or above the floor is unharmed.** A client verifies
//!   *forward*: it holds the head at `upto` and recomputes from there to the
//!   current head. Everything at or above the floor is retained, so every
//!   byte that recomputation needs is still there.
//! - **A pin below the floor can no longer be verified by recomputation.**
//!   The entries between it and the floor are gone. The client is left with
//!   "trust the signed checkpoint at the floor" — which is exactly the thing
//!   attestation exists so that nobody has to do. `security-logging-
//!   design.md` says the chain "is not tamper-evident against the platform
//!   itself, which could recompute from genesis and re-sign"; a pruned
//!   prefix puts a returning client back in that position for the span it
//!   was away.
//! - **So the prune must publish the floor.** Without it, a client whose pin
//!   has fallen below the floor asks for continuity, gets entries it cannot
//!   chain to, and has no way to tell a lawful prune from a deployment
//!   quietly dropping the history it is being asked about. `undo`'s
//!   [`crate::undo::VerifyFailure::PrefixMissing`]
//!   already draws that line for the operator — "the prefix was pruned
//!   (lawfully) or dropped (not), and from genesis the two are
//!   indistinguishable". The client needs the same distinction, and
//!   [`verifiable_from`] is the value that gives it: the earliest position
//!   from which recomputation is possible, served on every response as
//!   `x-ordo-verifiable-from` so a returning client learns it in one request
//!   rather than by failing. It is deliberately **not** covered by the pin's
//!   signature; `ordo_api::attest` carries that argument, and the short form
//!   is that a floor is an instruction about feasibility rather than
//!   evidence, and cannot make a client accept anything.
//!
//! This does not *fix* the loss — nothing can, short of making every pin a
//! registered consumer, which would mean an abandoned browser tab pinning a
//! space's history forever. It makes the loss **visible at the moment it
//! matters**, which is the same posture the whole attestation design takes:
//! detect, disclose, do not pretend to prevent.
//!
//! # Order of operations, and why it is the argument
//!
//! [`prune`] does five things under the space's own append lock, and the
//! order is load-bearing:
//!
//! 1. compute the floor — no anchor is *not* a failure, it is "not yet";
//! 2. the anchoring checkpoint's signature must verify, or the cut point is
//!    a position nobody has vouched for;
//! 3. **the prefix about to be destroyed must verify** — the mirror of
//!    `sign_checkpoint`'s refusal to anchor tampered history, and the
//!    stronger case of the two: signing over a bad prefix publishes a lie,
//!    deleting one destroys the only evidence the lie was ever told;
//! 4. delete;
//! 5. collect the sealed payloads the delete just orphaned, **in the same
//!    transaction**.
//!
//! Step 5 is not tidiness. A payload is reachable only through the image
//! that names it, and `sealed::collect_pruned`'s query is an anti-join
//! against `$commandlog`. If the delete commits and the collection does
//! not, every payload of the pruned groups becomes ciphertext that nothing
//! can name and *nothing can ever find again* — the query that would find
//! it needs the very rows that were removed. Unreachable and undeletable is
//! worse than either alone.
//!
//! The lock is `pg_advisory_xact_lock` in the deleting transaction — the
//! same lock, in the same namespace, that append and checkpoint take. A
//! writer waits rather than racing a delete, and the commit that ends the
//! prune releases it.

use std::time::Duration;

use ordo_commandlog::{
    chain::decode_group, ChainHash, Checkpoint, CheckpointVerifier, SpaceId, Timestamp,
    VERIFY_BATCH,
};

use crate::cadence::LogPolicy;
use crate::commandlog::{bytea_of, chain_hash_of, space_lock_key, APPEND_LOCK_NS};
use crate::sealed;
use crate::undo::{self, Agreement, Anchor, Db, VerifyFailure};
use crate::{sql_lit, OpClass, Pool, PoolError, TxConn};

/// Create `$logconsumers` if it is not there. See
/// [`crate::cadence::ensure_tables`] for why this is not in
/// `Pool::ensure_platform_tables`.
pub const CONSUMER_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$logconsumers\" ( \
  \"space\" text NOT NULL, \
  \"consumer\" text NOT NULL, \
  \"kind\" text NOT NULL, \
  \"position\" bigint NOT NULL, \
  \"advanced_at\" bigint NOT NULL, \
  PRIMARY KEY (\"space\", \"consumer\") \
)";

pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(CONSUMER_DDL).await.is_err() {
        pool.rows(CONSUMER_DDL).await?;
    }
    Ok(())
}

// ── consumers ────────────────────────────────────────────────────────────

/// The four kinds of registered consumer, and the one that may be evicted.
///
/// `security-logging-design.md` §7's table, transcribed rather than
/// summarised: the kinds "are not interchangeable, and eviction applies to
/// exactly one of them".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumerKind {
    /// A managed or BYO-custody mirror. **Evictable.** It is a cache:
    /// returning past the floor costs it a refetch and nothing else.
    Mirror,
    /// A mirror in a client-first space. **Not evictable.** The device *is*
    /// the authoritative store, so there is nothing to refetch from and
    /// eviction would destroy data rather than a cache. The custody dial
    /// changes what a mirror *is*, and this rule follows it. The remedy for
    /// an abandoned device is to un-enrol it — a membership operation,
    /// attributable and logged.
    ClientFirstMirror,
    /// A routine's trigger position. **Not evictable.** Delivery is
    /// at-least-once *because* the floor never passes the slowest consumer
    /// — the two sentences are the same rule. Evicting a listener and
    /// re-registering it at the current floor turns at-least-once into
    /// zero-times for precisely the routine that most needed the events. A
    /// listener that fails retries with backoff and then dead-letters as an
    /// operator alert; that path is lossless and already exists.
    Listener,
    /// An archive sink. **Not evictable.** A failing archive is an
    /// operational fault to fix, not a laggard to route around; pruning
    /// past it destroys the copy of record.
    Archive,
}

impl ConsumerKind {
    /// True for [`ConsumerKind::Mirror`] and nothing else.
    pub fn evictable(self) -> bool {
        matches!(self, ConsumerKind::Mirror)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ConsumerKind::Mirror => "mirror",
            ConsumerKind::ClientFirstMirror => "client_first_mirror",
            ConsumerKind::Listener => "listener",
            ConsumerKind::Archive => "archive",
        }
    }

    pub fn parse(s: &str) -> Option<ConsumerKind> {
        match s {
            "mirror" => Some(ConsumerKind::Mirror),
            "client_first_mirror" => Some(ConsumerKind::ClientFirstMirror),
            "listener" => Some(ConsumerKind::Listener),
            "archive" => Some(ConsumerKind::Archive),
            _ => None,
        }
    }
}

impl std::fmt::Display for ConsumerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One registered cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consumer {
    pub name: String,
    pub kind: ConsumerKind,
    /// The position it has consumed up to, in the same `upto` units the
    /// chain and its checkpoints use.
    pub position: u64,
    /// When it last moved. Only meaningful for an evictable kind; recorded
    /// for all of them so the dashboard's log-weight view can name a stuck
    /// consumer whether or not it may be evicted.
    pub advanced_at: u64,
}

/// A mirror that has left the floor calculation, with everything an
/// operator alert needs. Eviction is **loud** by construction: it is a
/// value the caller must handle, not a silently narrower bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictedConsumer {
    pub name: String,
    pub position: u64,
    /// Seconds since it last advanced.
    pub idle_for: u64,
}

/// Register a consumer, or leave an existing one where it is.
///
/// `DO NOTHING` on conflict: re-registering must not reset a cursor
/// backwards or forwards. Moving one is [`advance_consumer`]'s job and is a
/// separate, attributable act.
pub async fn register_consumer(
    pool: &Pool,
    space: &SpaceId,
    name: &str,
    kind: ConsumerKind,
    position: u64,
    now: u64,
) -> Result<(), PoolError> {
    pool.rows(&format!(
        "INSERT INTO \"$logconsumers\" \
         (\"space\", \"consumer\", \"kind\", \"position\", \"advanced_at\") \
         VALUES ({}, {}, {}, {position}, {now}) \
         ON CONFLICT (\"space\", \"consumer\") DO NOTHING",
        sql_lit(&space.0),
        sql_lit(name),
        sql_lit(kind.as_str()),
    ))
    .await
    .map(|_| ())
}

/// Move a cursor forward. Never backwards: `GREATEST` in the update, so a
/// consumer replaying an old position cannot lower the floor it holds and
/// hand a pruner permission it did not have a moment ago.
pub async fn advance_consumer(
    pool: &Pool,
    space: &SpaceId,
    name: &str,
    position: u64,
    now: u64,
) -> Result<(), PoolError> {
    pool.rows(&format!(
        "UPDATE \"$logconsumers\" SET \"position\" = GREATEST(\"position\", {position}), \
         \"advanced_at\" = {now} WHERE \"space\" = {} AND \"consumer\" = {}",
        sql_lit(&space.0),
        sql_lit(name),
    ))
    .await
    .map(|_| ())
}

/// Stop pinning the floor, explicitly.
///
/// `security-logging-design.md`: "a listener cursor that is genuinely dead
/// … stops being a registered consumer by that act, not by timing out.
/// Deregistration is the supported way to stop pinning the floor, and it is
/// explicit, attributable, and already logged." This is that act, and it is
/// the *only* way a non-evictable kind leaves the calculation.
pub async fn deregister_consumer(
    pool: &Pool,
    space: &SpaceId,
    name: &str,
) -> Result<bool, PoolError> {
    let rows = pool
        .rows(&format!(
            "DELETE FROM \"$logconsumers\" WHERE \"space\" = {} AND \"consumer\" = {} \
             RETURNING \"consumer\"",
            sql_lit(&space.0),
            sql_lit(name),
        ))
        .await?;
    Ok(!rows.is_empty())
}

/// [`deregister_consumer`] inside a transaction the caller owns, returning
/// the position the cursor held. `None` means no such consumer.
///
/// The position comes back so the act can be recorded with what it
/// released, and the transaction is the caller's so the record and the
/// delete commit together or not at all (`ordo triggers deregister`).
pub async fn deregister_consumer_in(
    tx: &TxConn,
    space: &SpaceId,
    name: &str,
) -> Result<Option<u64>, PoolError> {
    let rows = tx
        .rows(&format!(
            "DELETE FROM \"$logconsumers\" WHERE \"space\" = {} AND \"consumer\" = {} \
             RETURNING \"position\"",
            sql_lit(&space.0),
            sql_lit(name),
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok()))
}

/// Every registered consumer in a space, by name.
pub async fn consumers(pool: &Pool, space: &SpaceId) -> Result<Vec<Consumer>, PoolError> {
    let tx = pool.begin_for(OpClass::Read).await?;
    let out = consumers_in(&tx, space).await;
    tx.commit().await?;
    out
}

async fn consumers_in(tx: &TxConn, space: &SpaceId) -> Result<Vec<Consumer>, PoolError> {
    let rows = tx
        .rows(&format!(
            "SELECT \"consumer\", \"kind\", \"position\", \"advanced_at\" \
             FROM \"$logconsumers\" WHERE \"space\" = {} ORDER BY \"consumer\"",
            sql_lit(&space.0)
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let text = |i: usize| row.get(i).and_then(|c| c.clone());
        let num = |i: usize| -> Option<u64> {
            row.get(i)
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
        };
        let (Some(name), Some(kind), Some(position), Some(advanced_at)) =
            (text(0), text(1), num(2), num(3))
        else {
            return Err(PoolError::Db("malformed consumer row".into()));
        };
        // An unrecognised kind is a refusal, not a default. Defaulting it to
        // `Mirror` would make an unknown consumer the *evictable* one, which
        // is the worst possible guess: the floor would move past a cursor
        // whose contract nothing in this build understands.
        let kind = ConsumerKind::parse(&kind)
            .ok_or_else(|| PoolError::db(format!("consumer `{name}` has unknown kind `{kind}`")))?;
        out.push(Consumer {
            name,
            kind,
            position,
            advanced_at,
        });
    }
    Ok(out)
}

// ── the floor ────────────────────────────────────────────────────────────

/// What set the live bound. An operator who cannot see which consumer is
/// holding the floor cannot act on it, and "the log is not shrinking" with
/// no name attached is the report that gets ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloorHolder {
    /// The undo horizon is the binding constraint — the ordinary healthy
    /// case, and the one where nothing is wrong.
    UndoHorizon,
    /// A registered consumer is further behind than the horizon.
    Consumer {
        name: String,
        kind: ConsumerKind,
        position: u64,
    },
    /// **The chain's own last entry: a prune never empties the chain**,
    /// whatever the horizon and the consumers permit.
    ///
    /// This is not tidiness, and it is not a guess about what an operator
    /// wants. It is forced by what an empty chain *means* to the two readers
    /// that interpret one, and both of them get it wrong in opposite
    /// directions.
    ///
    /// A checkpoint at `upto = U` covers entries `0..U-1`, and a prune
    /// deletes `idx < U`. When the checkpoint sits at the head —
    /// `U = last + 1`, which [`horizon_position`] returns as soon as
    /// **every** entry is older than the undo horizon, the ordinary state of
    /// a quiet space at ninety days — the delete takes the whole table. What
    /// that leaves:
    ///
    /// - **From genesis, the chain verifies vacuously.** `undo::verify`
    ///   answers `Ok` with `checked: 0` for an empty chain at `base == 0`,
    ///   because a space that was never written to is exactly that. A space
    ///   whose entire history was pruned is not, and the two become one
    ///   answer.
    /// - **From its own checkpoint, the chain reads as tampered.** With the
    ///   key supplied, `choose_anchor` returns the checkpoint at `U`,
    ///   `verify` finds nothing at or after `U` and no head at all, and the
    ///   `base != 0` arm calls it `TamperKind::Unreadable`. A lawful prune
    ///   reporting tampering is the failure this whole module exists to
    ///   avoid.
    /// - **And [`verifiable_from`] would answer `0`** — "recompute from
    ///   genesis" — about a chain that retains nothing to recompute.
    ///
    /// Retaining one entry costs one row and makes all three answers correct
    /// at once: from genesis it is `PrefixMissing { first_stored }`, from the
    /// checkpoint the retained suffix verifies, and `verifiable_from` names a
    /// position that is really there. The bound is applied where every other
    /// bound is — in the floor calculation — so it appears in
    /// [`Floor::holder`] rather than as a silent clamp inside the delete.
    RetainedTail { last_index: u64 },
    /// **A restore marker an orphaned checkpoint above the cut still needs.**
    ///
    /// A cut at `F` deletes every entry below `F` and leaves `$checkpoints`
    /// alone, so an orphan at `upto >= F` is still classified after the cut
    /// and still has to be explained. What explains it is a marker's
    /// `pre_restore_head`, and the marker is a chain entry: cut past its
    /// index and the explanation is gone while the orphan remains, which
    /// turns a lawfully recorded restore into
    /// [`crate::undo::VerifyFailure::UnexplainedOrphan`] on the next verify.
    ///
    /// The cut therefore drops to the greatest current checkpoint at or
    /// below the marker's index. The clamp is temporary by construction: it
    /// lifts as soon as the live bound passes the orphan's position, because
    /// the orphan is then below the floor too, and the marker and the
    /// orphans it explains fall out of the chain in the same cut. What is
    /// retained past the horizon is bounded by the history the restore
    /// discarded, not by the age of the marker.
    RestoreMarker { index: u64, orphan_upto: u64 },
}

impl std::fmt::Display for FloorHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FloorHolder::UndoHorizon => f.write_str("the undo horizon"),
            FloorHolder::Consumer {
                name,
                kind,
                position,
            } => write!(f, "consumer `{name}` ({kind}) at position {position}"),
            FloorHolder::RetainedTail { last_index } => write!(
                f,
                "the retained tail — a prune never removes the last entry ({last_index})"
            ),
            FloorHolder::RestoreMarker { index, orphan_upto } => write!(
                f,
                "the restore marker at index {index}, which explains the orphaned checkpoint at \
                 position {orphan_upto}; the cut lifts once that position falls below the floor"
            ),
        }
    }
}

/// Where the floor is, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Floor {
    pub space: SpaceId,
    /// The cut point: the greatest checkpoint position at or below
    /// [`Floor::live_bound`]. `0` when no checkpoint qualifies, which means
    /// nothing may be pruned yet.
    pub position: u64,
    /// `min(live consumer positions, horizon_position)`.
    pub live_bound: u64,
    /// `now − undoHorizon`, resolved to a position.
    pub horizon_position: u64,
    /// Which constraint produced [`Floor::live_bound`].
    pub holder: FloorHolder,
    /// Mirrors that left the calculation because they had stalled. Never
    /// empty silently: the caller raises the operator alert this list is
    /// the content of.
    pub evicted: Vec<EvictedConsumer>,
    /// The anchoring checkpoint, when one qualifies.
    pub anchor: Option<Checkpoint>,
    /// The chain's first stored index right now — what [`verifiable_from`]
    /// would answer before this prune runs.
    pub first_stored: u64,
}

/// Compute the floor without touching anything.
///
/// What `ordo prune --dry-run` and the dashboard's log-weight view read.
pub async fn floor(
    pool: &Pool,
    space: &SpaceId,
    policy: &LogPolicy,
    now: u64,
) -> Result<Floor, PoolError> {
    let tx = pool.begin_for(OpClass::Read).await?;
    let out = floor_in(&tx, space, policy, now).await;
    tx.commit().await?;
    out
}

async fn floor_in(
    tx: &TxConn,
    space: &SpaceId,
    policy: &LogPolicy,
    now: u64,
) -> Result<Floor, PoolError> {
    let cutoff = now.saturating_sub(policy.undo_horizon.as_secs());
    let horizon = horizon_position_in(tx, space, cutoff).await?;

    // The duration is already a position; only now is `min` well typed.
    let mut live_bound = horizon;
    let mut holder = FloorHolder::UndoHorizon;
    let mut evicted = Vec::new();
    for c in consumers_in(tx, space).await? {
        let idle = now.saturating_sub(c.advanced_at);
        if c.kind.evictable() && idle > policy.consumer_stall.as_secs() {
            evicted.push(EvictedConsumer {
                name: c.name,
                position: c.position,
                idle_for: idle,
            });
            continue;
        }
        if c.position < live_bound {
            live_bound = c.position;
            holder = FloorHolder::Consumer {
                name: c.name,
                kind: c.kind,
                position: c.position,
            };
        }
    }

    // The last bound, applied after the others because it overrides all of
    // them: see `retained_tail`. An empty chain has no tail to retain and
    // nothing to prune either, so there is nothing to cap.
    if let Some(last) = last_stored_in(tx, space).await? {
        if last < live_bound {
            live_bound = last;
            holder = FloorHolder::RetainedTail { last_index: last };
        }
    }

    let mut anchor = checkpoint_at_or_below(tx, space, live_bound).await?;

    // The last bound of all, because it is about what the *verifier* will
    // still be able to explain after the cut rather than about what any
    // consumer has read. See `FloorHolder::RestoreMarker`.
    if let Some(cut) = anchor.as_ref().map(|c| c.upto) {
        if let Some((index, orphan_upto)) = marker_holding_the_floor(tx, space, cut).await? {
            live_bound = index;
            holder = FloorHolder::RestoreMarker { index, orphan_upto };
            anchor = checkpoint_at_or_below(tx, space, index).await?;
        }
    }

    Ok(Floor {
        space: space.clone(),
        position: anchor.as_ref().map(|c| c.upto).unwrap_or(0),
        live_bound,
        horizon_position: horizon,
        holder,
        evicted,
        anchor,
        first_stored: first_stored_in(tx, space).await?,
    })
}

/// `now − undoHorizon`, resolved to a chain position.
///
/// See the module doc for why this scans bodies rather than reading a
/// timestamp column, and for the direction its error takes under clock
/// skew.
pub async fn horizon_position(pool: &Pool, space: &SpaceId, cutoff: u64) -> Result<u64, PoolError> {
    let tx = pool.begin_for(OpClass::Read).await?;
    let out = horizon_position_in(&tx, space, cutoff).await;
    tx.commit().await?;
    out
}

async fn horizon_position_in(tx: &TxConn, space: &SpaceId, cutoff: u64) -> Result<u64, PoolError> {
    let mut from = first_stored_in(tx, space).await?;
    loop {
        let rows = tx
            .rows(&format!(
                "SELECT \"idx\", \"body\" FROM \"$commandlog\" WHERE \"space\" = {} \
                 AND \"idx\" >= {from} ORDER BY \"idx\" LIMIT {VERIFY_BATCH}",
                sql_lit(&space.0)
            ))
            .await?;
        if rows.is_empty() {
            return Ok(from);
        }
        for row in &rows {
            let idx: u64 = row
                .first()
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| PoolError::Db("chain entry has no index".into()))?;
            let body = bytea_of(row.get(1).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::db(format!("chain entry {idx} has no body")))?;
            // A body that will not decode is not something to skip past. It
            // is exactly what `verify` exists to name, and the prune will
            // meet it again in step 3; stopping here keeps the horizon from
            // being computed over entries nobody could read.
            let group = decode_group(&body)
                .map_err(|_| PoolError::db(format!("chain entry {idx} will not decode")))?;
            let latest = group.commands.iter().map(|c| c.at.0).max().unwrap_or(0);
            if latest > cutoff {
                return Ok(idx);
            }
        }
        let last: u64 = rows
            .last()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| PoolError::Db("chain entry has no index".into()))?;
        from = last + 1;
        if rows.len() < VERIFY_BATCH {
            return Ok(from);
        }
    }
}

/// The wall-clock instant of the group at `idx` — its newest command's
/// `at`, which is what [`horizon_position`] compares against a cutoff.
///
/// Exists so the **blob sweep** can express `Floor::live_bound` in the
/// coordinate a `$blobtomb` row carries. A log position says "everything
/// below this is prunable"; a tombstone carries a timestamp; and this is
/// the one place that converts between them, so the two floors are the
/// same floor rather than two numbers that usually agree.
///
/// `None` when the position is past the head — nothing is retained above
/// the horizon, so the horizon itself is the answer.
pub async fn group_time_at(
    pool: &Pool,
    space: &SpaceId,
    idx: u64,
) -> Result<Option<u64>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = {} AND \"idx\" = {idx}",
            sql_lit(&space.0)
        ))
        .await?;
    let Some(body) = rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_ref())
        .and_then(|c| bytea_of(Some(c)))
    else {
        return Ok(None);
    };
    // A body that will not decode is not skipped past, for
    // `horizon_position_in`'s reason: it is exactly what `verify` exists
    // to name, and a sweep computing a cutoff over an entry nobody can
    // read would be deciding to delete bytes on the strength of a
    // corrupted record.
    let group = decode_group(&body)
        .map_err(|_| PoolError::db(format!("chain entry {idx} will not decode")))?;
    Ok(Some(
        group.commands.iter().map(|c| c.at.0).max().unwrap_or(0),
    ))
}

/// The lowest restore marker a cut at `cut` would delete while an orphaned
/// checkpoint at or above the cut still needs it, as
/// `(marker index, orphan upto)`.
///
/// An orphan below the cut needs nothing: after the delete its position is
/// below the chain's first stored index, [`Agreement::BelowFloor`] reports
/// that the chain cannot say, and nothing has to explain it. An orphan at or
/// above the cut is still judged, so the marker that bounds it has to
/// survive with it.
async fn marker_holding_the_floor(
    tx: &TxConn,
    space: &SpaceId,
    cut: u64,
) -> Result<Option<(u64, u64)>, PoolError> {
    let db = Db::Tx(tx);
    let markers = undo::restore_markers(db, space).await?;
    if markers.is_empty() {
        return Ok(None);
    }
    let mut held: Option<(u64, u64)> = None;
    for cp in undo::stored_checkpoints(db, space).await? {
        if cp.upto < cut {
            continue;
        }
        if undo::agreement(db, space, cp.upto, cp.head).await? != Agreement::Orphaned {
            continue;
        }
        let Some(index) = undo::explained_by(&markers, cp.upto) else {
            continue;
        };
        let index = index as u64;
        if index >= cut {
            continue;
        }
        if held.is_none_or(|(lowest, _)| index < lowest) {
            held = Some((index, cp.upto));
        }
    }
    Ok(held)
}

/// The greatest checkpoint at or below `bound` **that the stored chain
/// agrees with**.
///
/// **`$checkpoints` and nothing else.** An attestation is not an anchor —
/// it lives in `$attestations` and no query in this module names that
/// table.
///
/// The agreement filter is `orphan-checkpoints` (2026-09-19) applied where
/// a checkpoint is *used*. A cut point is a position the platform promises
/// a reader can verify from, and an orphan is a head the chain no longer
/// holds there: cutting to one would delete the entries that disprove it
/// and leave nothing to verify against.
async fn checkpoint_at_or_below(
    tx: &TxConn,
    space: &SpaceId,
    bound: u64,
) -> Result<Option<Checkpoint>, PoolError> {
    current_checkpoint(
        tx,
        space,
        &format!("AND \"upto\" <= {bound} ORDER BY \"upto\" DESC, \"head\" DESC"),
    )
    .await
}

/// The checkpoint at exactly `upto` the chain agrees with, if any. Two rows
/// can share a position since `checkpoint-key`, and the base of a prune is
/// the current one.
async fn checkpoint_exactly_at(
    tx: &TxConn,
    space: &SpaceId,
    upto: u64,
) -> Result<Option<Checkpoint>, PoolError> {
    current_checkpoint(
        tx,
        space,
        &format!("AND \"upto\" = {upto} ORDER BY \"head\" DESC"),
    )
    .await
}

/// The first checkpoint matching `tail` that is [`Agreement::Current`].
async fn current_checkpoint(
    tx: &TxConn,
    space: &SpaceId,
    tail: &str,
) -> Result<Option<Checkpoint>, PoolError> {
    for cp in read_checkpoints(tx, space, tail).await? {
        if crate::undo::agreement(Db::Tx(tx), space, cp.upto, cp.head).await? == Agreement::Current
        {
            return Ok(Some(cp));
        }
    }
    Ok(None)
}

async fn read_checkpoints(
    tx: &TxConn,
    space: &SpaceId,
    tail: &str,
) -> Result<Vec<Checkpoint>, PoolError> {
    let rows = tx
        .rows(&format!(
            "SELECT \"upto\", \"head\", \"at\", \"key_id\", \"signature\" FROM \"$checkpoints\" \
             WHERE \"space\" = {} {tail}",
            sql_lit(&space.0)
        ))
        .await?;
    rows.iter()
        .map(|row| {
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
        })
        .collect()
}

/// The earliest position from which a client can still verify by
/// **recomputation** — the chain's first stored index.
///
/// This is the client-facing half of the prune, and the module doc explains
/// why it has to exist: a layer-1 pin is unregistered, so the floor rule
/// does not protect it, and a client whose pin has fallen below the floor
/// must be able to tell a lawful prune from a deployment dropping history.
/// `ordo-host` serves it on every response beside the head pin
/// (`x-ordo-verifiable-from`), so the client learns it in one request rather
/// than by failing to chain.
///
/// An empty chain answers `0`: genesis is where verification would start,
/// and nothing has been removed. That answer is only honest because
/// [`FloorHolder::RetainedTail`] guarantees a pruned chain is never empty —
/// without it, a chain pruned to the head would claim recomputation from
/// genesis while retaining nothing at all.
pub async fn verifiable_from(pool: &Pool, space: &SpaceId) -> Result<u64, PoolError> {
    let tx = pool.begin_for(OpClass::Read).await?;
    let out = first_stored_in(&tx, space).await;
    tx.commit().await?;
    out
}

async fn first_stored_in(tx: &TxConn, space: &SpaceId) -> Result<u64, PoolError> {
    Ok(endpoint_in(tx, space, "").await?.unwrap_or(0))
}

/// The chain's last stored index, or `None` for an empty chain.
///
/// `None` rather than `0`, because the two are different facts here and
/// [`floor_in`] acts on the difference: index `0` is a real entry a prune
/// must not remove, and an empty chain has nothing to cap the floor with.
async fn last_stored_in(tx: &TxConn, space: &SpaceId) -> Result<Option<u64>, PoolError> {
    endpoint_in(tx, space, " DESC").await
}

async fn endpoint_in(tx: &TxConn, space: &SpaceId, dir: &str) -> Result<Option<u64>, PoolError> {
    let rows = tx
        .rows(&format!(
            "SELECT \"idx\" FROM \"$commandlog\" WHERE \"space\" = {} ORDER BY \"idx\"{dir} LIMIT 1",
            sql_lit(&space.0)
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok()))
}

// ── the prune ────────────────────────────────────────────────────────────

/// What a prune did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pruned {
    /// Entries below the floor are gone, and so are the sealed payloads
    /// they orphaned.
    Removed {
        floor: Floor,
        entries: u64,
        payloads: u64,
        /// The new [`verifiable_from`] — the value clients need in order to
        /// know what this deployment can still prove by recomputation.
        verifiable_from: u64,
    },
    /// Lawfully nothing to do. Not a refusal: a space with no qualifying
    /// checkpoint is a space whose anchor has not come round yet, which is
    /// the ordinary state of a young deployment.
    NothingToPrune { floor: Floor, reason: Idle },
}

/// Why a prune had nothing to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idle {
    /// No checkpoint at or below the live bound. A checkpoint is the only
    /// legal cut point, so there is nowhere lawful to cut.
    NoAnchorBelowBound,
    /// The chain already begins at the floor.
    AlreadyAtFloor,
}

impl std::fmt::Display for Idle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Idle::NoAnchorBelowBound => f.write_str(
                "no checkpoint at or below the live bound — a checkpoint is the only legal \
                 cut point, so there is nothing to prune to yet",
            ),
            Idle::AlreadyAtFloor => f.write_str("the chain already begins at the floor"),
        }
    }
}

/// Why a prune did not happen. Every variant says what an operator does
/// next, because a refusal on the audit spine that reads as "no" is a
/// refusal somebody will work around.
#[derive(Debug)]
#[non_exhaustive]
pub enum PruneRefusal {
    /// The checkpoint the floor lands on does not verify under the supplied
    /// key. Cutting there would leave the chain anchored at a position
    /// nobody has vouched for — the same objection
    /// [`VerifyFailure::UnverifiedCheckpoint`] makes to *verifying* from
    /// one, with the difference that this one is irreversible.
    UnverifiedAnchor {
        upto: u64,
        key_id: String,
    },
    /// The chain already begins somewhere no checkpoint anchors, so the
    /// prefix that would be destroyed cannot be verified before destroying
    /// it. Pruning further would compound an unverifiable state.
    UnanchoredBase {
        first_stored: u64,
    },
    /// The prefix about to be deleted does not verify. Deleting it would
    /// destroy the only evidence that it was tampered with, which is
    /// strictly worse than the refusal `sign_checkpoint` already makes
    /// against *signing* over a bad prefix.
    WouldDestroyTamperedPrefix(VerifyFailure),
    Store(PoolError),
}

impl std::fmt::Display for PruneRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PruneRefusal::UnverifiedAnchor { upto, key_id } => write!(
                f,
                "refusing to prune to the checkpoint at position {upto}: it does not verify \
                 under key `{key_id}`. Supply the key that signed it, or find out why the \
                 deployment is holding a marker nobody can check — do not prune to it."
            ),
            PruneRefusal::UnanchoredBase { first_stored } => write!(
                f,
                "the chain begins at {first_stored} and no checkpoint anchors that position, \
                 so the prefix cannot be verified before it is destroyed. Sign a checkpoint \
                 the retained chain can be verified from before pruning again."
            ),
            PruneRefusal::WouldDestroyTamperedPrefix(e) => write!(
                f,
                "refusing to prune history that does not verify — deleting it would destroy \
                 the evidence: {e}"
            ),
            PruneRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PruneRefusal {}

/// Prune a space's chain to its floor.
///
/// See the module doc for the order of the five steps and why each one is
/// where it is. The whole thing runs in one transaction holding the space's
/// append lock, so a concurrent writer waits and the delete cannot race a
/// head read.
pub async fn prune(
    pool: &Pool,
    space: &SpaceId,
    verifier: &CheckpointVerifier,
    policy: &LogPolicy,
    now: u64,
) -> Result<Pruned, PruneRefusal> {
    let tx = pool
        .begin_for(OpClass::AddressedWrite)
        .await
        .map_err(PruneRefusal::Store)?;
    tx.rows(&format!(
        "SELECT pg_advisory_xact_lock({APPEND_LOCK_NS}, {})",
        space_lock_key(space)
    ))
    .await
    .map_err(PruneRefusal::Store)?;

    // 1. Where may we cut, and why there.
    let floor = floor_in(&tx, space, policy, now)
        .await
        .map_err(PruneRefusal::Store)?;
    let Some(anchor) = floor.anchor.clone() else {
        return Ok(Pruned::NothingToPrune {
            floor,
            reason: Idle::NoAnchorBelowBound,
        });
    };
    if floor.first_stored >= anchor.upto {
        return Ok(Pruned::NothingToPrune {
            floor,
            reason: Idle::AlreadyAtFloor,
        });
    }

    // 2. The cut point must be vouched for.
    if !verifier.verify_checkpoint(&anchor) {
        return Err(PruneRefusal::UnverifiedAnchor {
            upto: anchor.upto,
            key_id: anchor.key_id,
        });
    }

    // 3. The prefix about to be destroyed must verify. The anchor for that
    //    is where the chain currently *begins*, not the latest checkpoint —
    //    verifying from the latest would skip the very entries the delete
    //    is about to remove.
    let (base_anchor, base_hash) = if floor.first_stored == 0 {
        (Anchor::Genesis, ChainHash::GENESIS)
    } else {
        let Some(base) = checkpoint_exactly_at(&tx, space, floor.first_stored)
            .await
            .map_err(PruneRefusal::Store)?
        else {
            return Err(PruneRefusal::UnanchoredBase {
                first_stored: floor.first_stored,
            });
        };
        if !verifier.verify_checkpoint(&base) {
            return Err(PruneRefusal::UnverifiedAnchor {
                upto: base.upto,
                key_id: base.key_id,
            });
        }
        (
            Anchor::Checkpoint {
                upto: base.upto as usize,
                at: base.at,
                key_id: base.key_id.clone(),
            },
            base.head,
        )
    };
    undo::verify(Db::Tx(&tx), space, base_anchor, base_hash)
        .await
        .map_err(PruneRefusal::WouldDestroyTamperedPrefix)?;

    // 4. Delete.
    let before = count_entries(&tx, space)
        .await
        .map_err(PruneRefusal::Store)?;
    tx.rows(&format!(
        "DELETE FROM \"$commandlog\" WHERE \"space\" = {} AND \"idx\" < {}",
        sql_lit(&space.0),
        anchor.upto
    ))
    .await
    .map_err(PruneRefusal::Store)?;
    let after = count_entries(&tx, space)
        .await
        .map_err(PruneRefusal::Store)?;

    // 5. Collect what the delete just orphaned, here, in this transaction.
    let payloads = sealed::collect_pruned_in(&tx, &space_of(space))
        .await
        .map_err(PruneRefusal::Store)?;

    let verifiable_from = first_stored_in(&tx, space)
        .await
        .map_err(PruneRefusal::Store)?;
    tx.commit().await.map_err(PruneRefusal::Store)?;

    Ok(Pruned::Removed {
        floor,
        entries: before.saturating_sub(after),
        payloads,
        verifiable_from,
    })
}

async fn count_entries(tx: &TxConn, space: &SpaceId) -> Result<u64, PoolError> {
    let rows = tx
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

/// The log's space id and the crypto crate's are the same string in two
/// newtypes; the sealed store is keyed on the second.
fn space_of(space: &SpaceId) -> ordo_crypto::SpaceId {
    ordo_crypto::SpaceId(space.0.clone())
}

/// How long a mirror may sit still before the floor stops waiting for it —
/// re-exported so a caller configuring `consumerStall` does not have to
/// reach into `cadence` for the unit.
pub const DEFAULT_CONSUMER_STALL: Duration = LogPolicy::DEFAULT.consumer_stall;
