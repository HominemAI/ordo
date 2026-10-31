//! Checkpoint scheduling and the **separate** attestation cadence.
//!
//! `policy commandlog` carries two periods that look alike and are not, and
//! four documents say so in almost the same words —
//! `security-logging-design.md` §6, `engine-plan.md`'s F2 notes,
//! `bootstrap.md` question 31, and the closed key list in
//! `definition-language.md`. The ruling is one sentence: **attestation must
//! not inherit `checkpoint`'s cadence.** A checkpoint is the heavyweight
//! pruning/verification anchor, and `1mo` is right for it; an attestation is
//! a few hundred bytes and one signature, and wiring the two together leaves
//! a month of unattested history — a long time for a rewrite to sit
//! undetected.
//!
//! A rule restated four times is a rule somebody keeps breaking, so this
//! module is built so that breaking it is not expressible rather than
//! merely discouraged. Three separations, each load-bearing:
//!
//! - **Two schedule rows, two periods, two claims.** [`CHECKPOINT_SCHEDULE`]
//!   and [`ATTEST_SCHEDULE`] are distinct names in `$schedules`, each
//!   advanced by its own period. Nothing in this module reads one period to
//!   compute the other.
//! - **Two tables.** An attestation lands in `$attestations`, never in
//!   `$checkpoints`. This is the separation that matters most, and the
//!   reason is in [`prune`](crate::prune): a checkpoint is the **only legal
//!   cut point** for a prune, because verification resumes from a checkpoint
//!   and a suffix that begins mid-chain is unanchored. One table holding
//!   both artifacts would put "anchor a prune at an attestation" one `WHERE`
//!   clause away — and attestations arrive a thousand times more often, so
//!   the wrong query would look like it worked.
//! - **Two types.** [`Attestation`] is not [`ordo_commandlog::Checkpoint`],
//!   so no function that wants an anchor can be handed a record that is not
//!   one. What the two share, deliberately, is the **signed bytes**: an
//!   attestation signs `Checkpoint::signed_bytes` so a receiver checks both
//!   artifacts with one routine, which is what `security-logging-design.md`
//!   promises third parties. Same bytes, disjoint types, disjoint tables.
//!
//! # The scheduler API forced a small addition, and the reason is this bug
//!
//! `jobs::TxConn::claim_due(now, period)` claims **every** due schedule and
//! advances all of them by **one** period. Two schedules with different
//! cadences cannot coexist through that call: whichever period the caller
//! passes becomes both. That is not a hypothetical — it is precisely the
//! mechanism by which attestation would end up inheriting checkpoint's
//! cadence, arriving through the job queue rather than through a
//! misreading of the spec. Hence `claim_schedule(name, now, period)` beside
//! it, which claims one named schedule and advances that one by its own
//! period.
//!
//! # What is built here, and what is owed
//!
//! Built: the record. [`attest_due`] signs the head on cadence and stores
//! it, so a space accumulates a published-rate series of signed heads and
//! **silence becomes the signal** — `security-logging-design.md`'s stated
//! reason the cadence has to be fixed and known ahead.
//!
//! Owed: **layer 2, the attestation webhook, is not built.** It is an
//! outbound POST to a customer-named URL from inside the deployment's own
//! perimeter, which is the exact shape §7b pins six ways: `https` only with
//! mandatory certificate validation; resolve-then-pin with the connection
//! made to the *resolved address*, re-checked at connect time; no redirect
//! following, ever; signed deliveries under a per-sink secret; an egress
//! identity with no instance credentials and no `KeyProvider` reachability;
//! and a per-space allowlist consented at sink creation. Shipping the POST
//! before those pins exist is the SSRF-into-key-custody path §7b was
//! written to close, so the record ships and the delivery is owed. Layer 1
//! (per-request client pinning) is unaffected — it is always on, needs no
//! cadence, and lives in `ordo-host`'s `attest` module.

use std::time::Duration;

use ordo_commandlog::{ChainHash, Checkpoint, CheckpointVerifier, SpaceId, Timestamp};

use crate::commandlog::{bytea_of, chain_hash_of};
use crate::undo::{self, CheckpointFailure, Checkpointed};
use crate::{sql_lit, OpClass, Pool, PoolError};

/// `policy commandlog`'s closed key list, as a deployment supplies it.
///
/// **Parsing `policy commandlog { … }` out of `.ordo` source is not built.**
/// The declaration-form surface is its own item; until it lands a deployment
/// constructs this struct, and the values below are the spec's own defaults
/// rather than numbers invented here.
///
/// The key list is closed and this struct is the machine-readable half of
/// that sentence: a key `definition-language.md` carries and this struct
/// does not is a bug in this struct. One key is deliberately absent —
/// `archive`'s destination — because no archive sink exists to point at.
/// `prune` still models an archive *consumer*, so a deployment that
/// registers one pins the floor and never prunes past it, which is the
/// correct behaviour for a sink that cannot yet confirm it has the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogPolicy {
    /// `undoHorizon 90d` — fine-grained revert is guaranteed this far back,
    /// so nothing below it may be pruned. Also gates when a retired DEK
    /// epoch key may be destroyed.
    pub undo_horizon: Duration,
    /// `checkpoint 1mo` — the signed anchor interval. Heavyweight: it
    /// verifies a suffix and takes the space's append lock.
    pub checkpoint: Duration,
    /// `attest every N commands …` — the volume half.
    pub attest_every: u64,
    /// `… or T` — the time half. Whichever lands first.
    pub attest_or: Duration,
    /// `consumerStall 30d` — a **mirror** cursor idle this long leaves the
    /// floor calculation. Only a mirror; see [`crate::prune::ConsumerKind`].
    pub consumer_stall: Duration,
}

impl LogPolicy {
    /// A month is 30 days here. The spec writes `1mo`; the platform has no
    /// calendar type and a checkpoint interval that drifted with month
    /// length would make "the cadence is fixed and known ahead" false by a
    /// three-day margin twice a year.
    pub const MONTH: Duration = Duration::from_secs(30 * 24 * 60 * 60);

    pub const DEFAULT: LogPolicy = LogPolicy {
        undo_horizon: Duration::from_secs(90 * 24 * 60 * 60),
        checkpoint: LogPolicy::MONTH,
        attest_every: 1000,
        attest_or: Duration::from_secs(60 * 60),
        consumer_stall: Duration::from_secs(30 * 24 * 60 * 60),
    };
}

impl Default for LogPolicy {
    fn default() -> LogPolicy {
        LogPolicy::DEFAULT
    }
}

/// The heavyweight anchor's schedule.
pub const CHECKPOINT_SCHEDULE: &str = "commandlog.checkpoint";

/// The light, frequent one. A separate row so it carries a separate period;
/// see the module doc for the `claim_due` bug that makes sharing one row
/// silently wrong rather than merely untidy.
pub const ATTEST_SCHEDULE: &str = "commandlog.attest";

/// Signed heads on the attestation cadence.
///
/// Deliberately **not** `$checkpoints`. Two artifacts, two tables, so that
/// the pruner's "greatest checkpoint at or below the live bound" cannot be
/// satisfied by an attestation. The column list matches `$checkpoints`
/// because the signed material does; the separation is the point, not the
/// shape.
pub const ATTESTATION_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$attestations\" ( \
  \"space\" text NOT NULL, \
  \"upto\" bigint NOT NULL, \
  \"head\" bytea NOT NULL, \
  \"at\" bigint NOT NULL, \
  \"key_id\" text NOT NULL, \
  \"signature\" bytea NOT NULL \
); \
ALTER TABLE \"$attestations\" DROP CONSTRAINT IF EXISTS \"$attestations_pkey\"; \
CREATE UNIQUE INDEX IF NOT EXISTS \"$attestations_position_head\" \
  ON \"$attestations\" (\"space\", \"upto\", \"head\")";

/// Create `$attestations` if it is not there.
///
/// Separate from `Pool::ensure_platform_tables` only because that function
/// lives in a file this change does not own; a deployment should call both,
/// and the retry-once shape is copied for the same reason it exists there —
/// `CREATE TABLE IF NOT EXISTS` is not race-free between two booting
/// replicas.
pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(ATTESTATION_DDL).await.is_err() {
        pool.rows(ATTESTATION_DDL).await?;
    }
    Ok(())
}

/// A signed head published on the attestation cadence.
///
/// A distinct type from [`Checkpoint`] so that nothing which needs an
/// anchor can be handed one of these. [`Attestation::signed_bytes`]
/// delegates to `Checkpoint::signed_bytes`, so the *signature* a receiver
/// checks is produced by one routine across both artifacts — which is the
/// half `security-logging-design.md`'s documentation obligations require to
/// be shared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    pub space: SpaceId,
    pub upto: u64,
    pub head: ChainHash,
    pub at: Timestamp,
    pub key_id: String,
    pub signature: Vec<u8>,
}

impl Attestation {
    /// The bytes signed — identical to a checkpoint's at the same position,
    /// on purpose. A receiver holding one verifier checks either artifact.
    pub fn signed_bytes(space: &SpaceId, upto: u64, head: ChainHash, at: Timestamp) -> Vec<u8> {
        Checkpoint::signed_bytes(space, upto, head, at)
    }

    /// Check the signature and the key binding, exactly as
    /// `CheckpointVerifier::verify_checkpoint` does for the other artifact.
    /// The key id is compared, because a receiver that trusts key `B` must
    /// not accept a record signed by `A` merely because it was handed `A`'s
    /// public half.
    pub fn verify(&self, v: &CheckpointVerifier) -> bool {
        self.key_id == v.key_id()
            && v.verify(
                &Attestation::signed_bytes(&self.space, self.upto, self.head, self.at),
                &self.signature,
            )
    }
}

/// What an attestation tick did. Three answers, because a cron job that
/// cannot tell "I signed nothing" from "I signed the head" cannot alarm on
/// silence — and silence being the signal is the whole reason the cadence
/// is fixed and published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attested {
    /// A new signed head at this position.
    Signed(Attestation),
    /// The cadence has not come round. Both halves are reported so an
    /// operator can see which one is holding it back.
    NotDue {
        /// Groups appended since the last attestation.
        since: u64,
        /// Seconds since the last attestation, or `None` if there has never
        /// been one.
        elapsed: Option<u64>,
    },
    /// No history to attest to. Distinct from `NotDue`: a space that has
    /// never been written to is not a space whose attestations stopped.
    EmptyChain,
}

/// Register both schedules, each with its own period.
///
/// Two rows. `ON CONFLICT DO NOTHING` under the hood, so a replica booting
/// into an existing deployment does not reset a schedule that is mid-cycle.
pub async fn register_schedules(
    pool: &Pool,
    policy: &LogPolicy,
    now: i64,
) -> Result<(), PoolError> {
    let tx = pool.begin_for(OpClass::AddressedWrite).await?;
    tx.schedule(
        CHECKPOINT_SCHEDULE,
        now + policy.checkpoint.as_secs() as i64,
    )
    .await?;
    tx.schedule(ATTEST_SCHEDULE, now + policy.attest_or.as_secs() as i64)
        .await?;
    tx.commit().await
}

/// Claim the checkpoint schedule if it is due, advancing it by
/// **`policy.checkpoint`** and nothing else.
pub async fn claim_checkpoint_tick(
    pool: &Pool,
    policy: &LogPolicy,
    now: i64,
) -> Result<bool, PoolError> {
    claim(pool, CHECKPOINT_SCHEDULE, now, policy.checkpoint).await
}

/// Claim the attestation schedule if it is due, advancing it by
/// **`policy.attest_or`** and nothing else.
///
/// The time half of `every N commands or T` lives here; the volume half
/// cannot, because a schedule row knows about clocks and not about chains.
/// [`attest_due`] evaluates both and is the thing that decides.
pub async fn claim_attest_tick(
    pool: &Pool,
    policy: &LogPolicy,
    now: i64,
) -> Result<bool, PoolError> {
    claim(pool, ATTEST_SCHEDULE, now, policy.attest_or).await
}

async fn claim(pool: &Pool, name: &str, now: i64, period: Duration) -> Result<bool, PoolError> {
    let tx = pool.begin_for(OpClass::AddressedWrite).await?;
    let claimed = tx
        .claim_schedule(name, now, period.as_secs() as i64)
        .await?;
    tx.commit().await?;
    Ok(claimed)
}

/// The checkpoint tick: verify the suffix and sign a marker at the head.
///
/// A thin delegation to [`undo::sign_checkpoint`] on purpose. Checkpointing
/// already takes the append lock, already refuses to anchor history that
/// does not verify, and already reports `AlreadyAt` rather than silently
/// no-oping. A second implementation on the schedule path would be a second
/// set of those decisions, and they would drift.
pub async fn checkpoint_due(
    pool: &Pool,
    space: &SpaceId,
    signer: &ordo_commandlog::Ed25519Signer,
    at: Timestamp,
    witness: &crate::witness::Witness,
) -> Result<Checkpointed, CheckpointFailure> {
    undo::sign_checkpoint(pool, space, signer, at, witness).await
}

/// The attestation tick: **volume or time, whichever lands first.**
///
/// Due when either `head - last.upto >= policy.attest_every` or
/// `now - last.at >= policy.attest_or`.
///
/// **A head that has not moved is never due**, whichever period has
/// elapsed. `security-logging-design.md` is explicit that "a quiet one
/// costs nothing, because the head only moves when a write commits" — so
/// the time half is a bound on how long *new* history may sit unattested,
/// not a heartbeat. A heartbeat would re-sign identical bytes forever and
/// fill the table of a space nobody writes to.
///
/// The first attestation of a non-empty chain is always due: there is no
/// previous record for either half to measure against, and a space whose
/// series has not started cannot have its silence noticed.
pub async fn attest_due(
    pool: &Pool,
    space: &SpaceId,
    signer: &ordo_commandlog::Ed25519Signer,
    policy: &LogPolicy,
    now: Timestamp,
) -> Result<Attested, PoolError> {
    use ordo_commandlog::Signer as _;

    let Some((upto, head)) = head_position(pool, space).await? else {
        return Ok(Attested::EmptyChain);
    };

    let last = latest_attestation(pool, space).await?;
    let (since, elapsed) = match &last {
        Some(a) => (
            upto.saturating_sub(a.upto),
            Some(now.0.saturating_sub(a.at.0)),
        ),
        None => (upto, None),
    };
    if let Some(a) = &last {
        // **An orphan at this position does not cover this head** (R2e).
        // After a restore the chain grows back through positions an earlier
        // attestation already holds, and `upto > a.upto` would read that as
        // "the head has not moved" and never attest again at a reused
        // position. A receiver would then never hear the post-restore
        // history there. The stored record is classified against the chain,
        // and one the chain contradicts is not a statement about this head.
        let covered = a.head == head
            || undo::agreement(undo::Db::Pool(pool), space, a.upto, a.head).await?
                == undo::Agreement::Current;
        let moved = upto > a.upto;
        let by_volume = since >= policy.attest_every;
        let by_time = now.0.saturating_sub(a.at.0) >= policy.attest_or.as_secs();
        if covered && (!moved || !(by_volume || by_time)) {
            return Ok(Attested::NotDue { since, elapsed });
        }
    }

    let signature = signer.sign(&Attestation::signed_bytes(space, upto, head, now));
    let record = Attestation {
        space: space.clone(),
        upto,
        head,
        at: now,
        key_id: signer.key_id().to_string(),
        signature,
    };
    put_attestation(pool, &record).await?;
    Ok(Attested::Signed(record))
}

/// The chain's `(upto, head)` — `max(idx) + 1`, never `count(*)`.
///
/// The same rule `attest.rs` and `undo::head_of` follow, and for the same
/// reason: the two agree only on a chain that has never been pruned, and
/// this module exists in a world where pruning happens.
async fn head_position(
    pool: &Pool,
    space: &SpaceId,
) -> Result<Option<(u64, ChainHash)>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"idx\", \"hash\" FROM \"$commandlog\" WHERE \"space\" = {} \
             ORDER BY \"idx\" DESC LIMIT 1",
            sql_lit(&space.0)
        ))
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let idx: u64 = row
        .first()
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| PoolError::Db("chain head has no index".into()))?;
    let head = chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
        .ok_or_else(|| PoolError::Db("chain head has no hash".into()))?;
    Ok(Some((idx + 1, head)))
}

/// The chain's **first retained index** — where a pruned deployment can
/// still be verified by recomputation.
///
/// `ordo-host`'s `attest` module reads this value too, in the same statement
/// as the head, because a second round trip for one integer would double the
/// per-request cost of a layer whose whole claim is that it is ~free. This
/// one is for the paths that are *not* on the request path — the attestation
/// payload and `chainProof` — where one clear query beats sharing a
/// two-endpoint statement whose shape exists for a different reason. (That
/// shared statement is also where 5f found the ambiguous `ORDER BY` that
/// would have silently disabled layer-1 pinning; two readers of one clever
/// query is how that class of bug gets a second victim.)
///
/// `0` for an empty chain, and that is the honest answer: recompute from
/// genesis, of which there is nothing.
pub async fn verifiable_from(pool: &Pool, space: &SpaceId) -> Result<u64, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"idx\" FROM \"$commandlog\" WHERE \"space\" = {} \
             ORDER BY \"idx\" LIMIT 1",
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

/// Store one attestation. `DO NOTHING` on a repeated position, so two
/// replicas that both decided the same head was due leave one record rather
/// than a conflict — the bytes they signed are identical only if their
/// clocks agree, and the first one to land is as good as the second.
pub async fn put_attestation(pool: &Pool, a: &Attestation) -> Result<(), PoolError> {
    pool.rows(&format!(
        "INSERT INTO \"$attestations\" \
         (\"space\", \"upto\", \"head\", \"at\", \"key_id\", \"signature\") \
         VALUES ({}, {}, {}, {}, {}, {}) \
         ON CONFLICT (\"space\", \"upto\", \"head\") DO NOTHING",
        sql_lit(&a.space.0),
        a.upto,
        ordo_crypto::sealed::bytea_lit(&a.head.0),
        a.at.0,
        sql_lit(&a.key_id),
        ordo_crypto::sealed::bytea_lit(&a.signature),
    ))
    .await
    .map(|_| ())
}

/// The most recent attestation for a space.
pub async fn latest_attestation(
    pool: &Pool,
    space: &SpaceId,
) -> Result<Option<Attestation>, PoolError> {
    Ok(read(pool, space, "ORDER BY \"upto\" DESC LIMIT 1")
        .await?
        .pop())
}

/// Every attestation for a space, oldest first — what an operator reads to
/// check the series against the published cadence. Silence is the signal,
/// and this is where silence is visible.
pub async fn attestations(pool: &Pool, space: &SpaceId) -> Result<Vec<Attestation>, PoolError> {
    read(pool, space, "ORDER BY \"upto\"").await
}

async fn read(pool: &Pool, space: &SpaceId, tail: &str) -> Result<Vec<Attestation>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"upto\", \"head\", \"at\", \"key_id\", \"signature\" FROM \"$attestations\" \
             WHERE \"space\" = {} {tail}",
            sql_lit(&space.0)
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let num = |i: usize| -> Result<u64, PoolError> {
            row.get(i)
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| PoolError::Db("malformed attestation row".into()))
        };
        out.push(Attestation {
            space: space.clone(),
            upto: num(0)?,
            head: chain_hash_of(row.get(1).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("attestation has no head".into()))?,
            at: Timestamp(num(2)?),
            key_id: row
                .get(3)
                .and_then(|c| c.clone())
                .ok_or_else(|| PoolError::Db("attestation has no key id".into()))?,
            signature: bytea_of(row.get(4).and_then(|c| c.as_ref()))
                .ok_or_else(|| PoolError::Db("attestation has no signature".into()))?,
        });
    }
    Ok(out)
}
