//! Layer 2: the attestation webhook — the sink, the owner-signed
//! destination, and the at-least-once delivery loop.
//!
//! `security-logging-design.md` §7 layer 2 promises a receiver three things,
//! and this module is built around which of the three is *guaranteed*:
//!
//! - **Cannot forge** — the payload carries the Ed25519 signature
//!   `ordo_exec::cadence::attest_due` already computed over
//!   `Checkpoint::signed_bytes`. This module copies those bytes; it never
//!   signs anything with the checkpoint key, and the crate that opens the
//!   socket cannot reach one. See `ordo-egress`'s `sign.rs`.
//! - **Cannot redirect** — [`consent_bytes`] is signed by the **owner**, with
//!   a key the platform does not hold, and [`AttestSink::verify_destination`]
//!   re-checks it **immediately before every send**, not only at setup. The
//!   limit of that promise is stated below rather than left to be discovered.
//! - **Cannot omit undetectably** — omission is not preventable and this
//!   module does not pretend otherwise. What it does is make the *cadence*
//!   part of the payload, so a receiver holding one delivery knows when the
//!   next one is due and silence is a thing they can alarm on without reading
//!   any documentation.
//!
//! # What "cannot redirect" actually buys, and where it stops
//!
//! The destination row is `(destination, owner_key, destination_sig)`. The
//! worker verifies the signature over the canonical destination before every
//! delivery, so **an attacker who can write to the database cannot redirect
//! deliveries** — they would need the owner's private key, which never
//! reaches this deployment. That is the threat this defends against, and it
//! is the realistic one: a compromised operator account, a SQL injection, a
//! backup restored from somewhere else.
//!
//! It does **not** stop a platform that rewrites the destination *and* the
//! owner key together, because a signature checked against a key the
//! attacker also chose proves nothing. No scheme with the verifying key
//! inside the perimeter can close that, and claiming otherwise would be the
//! kind of promise this corpus keeps striking out. Two things make the
//! remaining gap visible rather than silent, and both are built:
//!
//! - the owner key's **fingerprint travels in every payload**
//!   (`destinationKeyId`), so a receiver who recorded it once sees a swap on
//!   the next delivery — the same trust-on-first-use shape layer 1 uses;
//! - setting a destination is **owner-gated and step-up gated**, so it is in
//!   the security-event stream as a `CustodyAction` with a presence
//!   ceremony joined to it. A platform that changes it leaves the record, and
//!   a platform that changes it *without* the record has to have edited two
//!   chained artifacts.
//!
//! # At-least-once, and the cursor discipline it inherits
//!
//! `triggers.rs` established the rule and the reason: *the cursor advances
//! only after the work has durably committed; advancing first would be
//! at-most-once wearing the same code.* Here the work is a row in
//! [`ATTEST_DELIVERY_DDL`]'s outbox, keyed `(space, upto)` — so a redelivery
//! after a crash is `ON CONFLICT DO NOTHING`, enforced by the database rather
//! than by a read-then-write a race could straddle.
//!
//! **A new sink starts at the current head, not at genesis.** §7's second
//! stated limit is that *"you can only verify from the point you started
//! watching"*, and backfilling a thousand historical attestations into a
//! receiver's log would make that false-looking: the receiver would hold
//! signed heads for times it was not watching, which anchors nothing and
//! reads like it does. [`register_sink`] sets the cursor to the latest
//! attestation at registration and says so on the consent surface.
//!
//! # Why the delivery is not a `$jobs` row
//!
//! `$jobs` has no `attempts` and no `next_attempt`, so it cannot back off and
//! cannot dead-letter. `$triggerruns` has both, and this module reuses
//! `triggers::backoff_secs` and `triggers::MAX_ATTEMPTS` rather than growing
//! a second backoff — `plan-runtime.md` R3's rule is *"one shape of backoff
//! in the platform, not two"*.

use ordo_commandlog::{ChainHash, CheckpointVerifier, SpaceId};

use crate::cadence::{Attestation, LogPolicy};
use crate::commandlog::{bytea_lit, bytea_of, chain_hash_of};
use crate::triggers::{backoff_secs, MAX_ATTEMPTS};
use crate::{sql_lit, OpClass, Pool, PoolError};

/// The published payload schema. Versioned, and in the payload, so a receiver
/// writes a verifier against a name rather than against a sample.
pub const PAYLOAD_SCHEMA: &str = "ordo.attestation/1";

/// The domain tag inside the owner's consent signature. Distinct from every
/// other signed material in the platform, so an owner signature minted for
/// one purpose cannot be presented as another.
pub const CONSENT_DOMAIN: &str = "ordo.attest-destination/1";

/// `plan-runtime.md` R3's worker slot. `<subsystem>.<verb>`, `'static`,
/// simultaneously a metric label, a boot line and a `/health/deep` key.
pub const DELIVER_SLOT: &str = "attest.deliver";

/// One sink per space, and the primary key says so.
///
/// §7 layer 2 nominates *an* endpoint, singular. Keeping it singular is what
/// makes "the allowlist is exactly the owner-signed destination" true by
/// construction rather than by a join — the general multi-sink family is the
/// post-deployment egress rail and will carry its own allowlist table.
pub const ATTEST_SINK_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$attestsinks\" ( \
  \"space\" text PRIMARY KEY, \
  \"destination\" text NOT NULL, \
  \"owner_key_id\" text NOT NULL, \
  \"owner_key\" text NOT NULL, \
  \"destination_sig\" bytea NOT NULL, \
  \"secret\" text NOT NULL, \
  \"created_at\" bigint NOT NULL, \
  \"cursor\" bigint NOT NULL DEFAULT 0 \
)";

/// The outbox. Shaped after `$triggerruns`, not after `$jobs`; see the module
/// doc.
pub const ATTEST_DELIVERY_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$attestdeliveries\" ( \
  \"space\" text NOT NULL, \
  \"upto\" bigint NOT NULL, \
  \"attempts\" int NOT NULL DEFAULT 0, \
  \"next_attempt\" bigint NOT NULL DEFAULT 0, \
  \"claimed\" boolean NOT NULL DEFAULT false \
); \
ALTER TABLE \"$attestdeliveries\" ADD COLUMN IF NOT EXISTS \"head\" bytea NOT NULL DEFAULT ''; \
ALTER TABLE \"$attestdeliveries\" DROP CONSTRAINT IF EXISTS \"$attestdeliveries_pkey\"; \
CREATE UNIQUE INDEX IF NOT EXISTS \"$attestdeliveries_position_head\" \
  ON \"$attestdeliveries\" (\"space\", \"upto\", \"head\")";

/// Where a delivery goes when it has run out of attempts. Visible on the ops
/// surface; the row keeps the destination so an operator reading it does not
/// have to join against a sink that may since have been re-pointed.
pub const ATTEST_DEADLETTER_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$attestdeadletters\" ( \
  \"space\" text NOT NULL, \
  \"upto\" bigint NOT NULL, \
  \"destination\" text NOT NULL, \
  \"attempts\" int NOT NULL, \
  \"at\" bigint NOT NULL, \
  \"reason\" text NOT NULL \
); \
ALTER TABLE \"$attestdeadletters\" ADD COLUMN IF NOT EXISTS \"head\" bytea NOT NULL DEFAULT ''; \
ALTER TABLE \"$attestdeadletters\" DROP CONSTRAINT IF EXISTS \"$attestdeadletters_pkey\"; \
CREATE UNIQUE INDEX IF NOT EXISTS \"$attestdeadletters_position_head\" \
  ON \"$attestdeadletters\" (\"space\", \"upto\", \"head\")";

/// Create the three tables.
///
/// A worker's tables, so this follows `prune::ensure_tables` and
/// `cadence::ensure_tables` rather than going into
/// `Pool::ensure_platform_tables` — `plan-runtime.md` R3 §3.7 argues at
/// length that a table only workers use belongs in the supervisor's bootstrap
/// and not in every process that opens a pool. The retry-once shape is copied
/// for the reason it exists there: `CREATE TABLE IF NOT EXISTS` is not
/// race-free between two booting replicas.
pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [ATTEST_SINK_DDL, ATTEST_DELIVERY_DDL, ATTEST_DEADLETTER_DDL] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// The bytes an owner signs to nominate a destination.
///
/// ```text
/// consent := str("ordo.attest-destination/1") ‖ str(space) ‖ str(canonical-url)
/// str     := u64-BE byte-length ‖ bytes
/// ```
///
/// **The space is inside the signature**, and that is not decoration: an
/// owner who owns two spaces would otherwise be able to have one signature
/// replayed into the other, silently pointing space B's attestations at space
/// A's receiver. Length-prefixing for the reason every canonical encoding in
/// this repo gives — `("ab", "c")` and `("a", "bc")` must not collide — and
/// u64 big-endian because that is what `ordo-commandlog`'s chain encoding
/// uses and one convention is better than two.
///
/// The **secret is deliberately outside** it. The platform mints the per-sink
/// HMAC secret, so an owner cannot sign it at the moment they sign the URL,
/// and pretending otherwise would mean either a two-round ceremony or a
/// secret the owner chose — which is a worse secret.
pub fn consent_bytes(space: &SpaceId, canonical_url: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(canonical_url.len() + space.0.len() + 64);
    for part in [
        CONSENT_DOMAIN.as_bytes(),
        space.0.as_bytes(),
        canonical_url.as_bytes(),
    ] {
        out.extend_from_slice(&(part.len() as u64).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// A space's attestation destination, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestSink {
    pub space: SpaceId,
    /// The canonical `https://…` form. What the owner signed, and what the
    /// transport MAC binds.
    pub destination: String,
    /// The owner's key fingerprint, published in every payload so a receiver
    /// notices a swap.
    pub owner_key_id: String,
    /// The owner's Ed25519 public half, 64 lowercase hex.
    pub owner_key: String,
    /// The owner's signature over [`consent_bytes`].
    pub destination_sig: Vec<u8>,
    /// The per-sink HMAC secret, hex. Shown once at grant; stored because a
    /// MAC needs it at every delivery.
    pub secret: String,
    pub created_at: u64,
    /// The attestation position this sink has delivered up to. Advances only
    /// after the outbox row has committed.
    pub cursor: u64,
}

/// Why a destination was refused.
///
/// All of these fold into one wire class, `egress_refused`; see
/// `ordo_egress::refusal` on why a class per reason would be a
/// reconnaissance oracle. `NotSigned` is the exception worth noticing — it is
/// not an egress fact at all, it is the owner-signature check, and it reaches
/// the wire as `not_confirmed`, which `api-engine-design.md` already defines
/// as *"a refusal after the value bound correctly, made by the op because
/// what the value says is not what the op requires it to say"*. A destination
/// whose signature does not verify is exactly that.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SinkRefusal {
    /// The URL failed one of the destination pins.
    Egress(String),
    /// The owner's public key is not 64 hex characters of Ed25519.
    BadOwnerKey(String),
    /// The signature does not verify over `consent_bytes(space, canonical)`.
    NotSigned,
    /// No sink is configured for this space.
    NoSink,
}

impl std::fmt::Display for SinkRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SinkRefusal::Egress(d) => write!(f, "{d}"),
            SinkRefusal::BadOwnerKey(d) => write!(f, "the owner key is unusable: {d}"),
            SinkRefusal::NotSigned => f.write_str(
                "the destination signature does not verify under the owner key supplied; \
                 the platform stores a destination it cannot itself authorise",
            ),
            SinkRefusal::NoSink => f.write_str("this space has no attestation destination"),
        }
    }
}

impl AttestSink {
    /// Re-check the owner's signature over the stored destination.
    ///
    /// Called at setup **and immediately before every delivery**. The second
    /// call is the one that matters: it is what makes a direct edit of the
    /// `destination` column a refused delivery rather than a redirected one,
    /// and it costs one Ed25519 verification per attestation — which arrive
    /// at most once per `attest_every` commands.
    pub fn verify_destination(&self) -> Result<(), SinkRefusal> {
        let v = CheckpointVerifier::from_public_hex(&self.owner_key_id, &self.owner_key)
            .map_err(|e| SinkRefusal::BadOwnerKey(format!("{e:?}")))?;
        if v.verify(
            &consent_bytes(&self.space, &self.destination),
            &self.destination_sig,
        ) {
            Ok(())
        } else {
            Err(SinkRefusal::NotSigned)
        }
    }
}

/// Nominate a destination for a space.
///
/// The caller has already parsed and canonicalised the URL through
/// `ordo_egress::HttpsUrl` — this function takes the canonical form so that
/// the bytes stored, the bytes verified and the bytes the owner signed are
/// the same string by construction rather than by two parsers agreeing.
///
/// `ON CONFLICT … DO UPDATE`: re-pointing a sink is a legitimate act (an
/// endpoint moves), and it is owner-gated, step-up gated and recorded. The
/// cursor is **preserved** across a re-point, because the deliveries already
/// made were made, and resetting it would replay the whole series at a new
/// receiver as though it had been watching.
#[allow(clippy::too_many_arguments)]
pub async fn register_sink(
    pool: &Pool,
    space: &SpaceId,
    canonical_url: &str,
    owner_key_id: &str,
    owner_key: &str,
    destination_sig: &[u8],
    secret_hex: &str,
    now: u64,
) -> Result<AttestSink, SinkRefusal> {
    let sink = AttestSink {
        space: space.clone(),
        destination: canonical_url.to_string(),
        owner_key_id: owner_key_id.to_string(),
        owner_key: owner_key.to_string(),
        destination_sig: destination_sig.to_vec(),
        secret: secret_hex.to_string(),
        created_at: now,
        cursor: 0,
    };
    // Before anything is stored. A destination the platform cannot verify is
    // a destination the platform must not hold, because holding it is what
    // makes a later "we had a signature" claim possible.
    sink.verify_destination()?;

    // Start watching from now. See the module doc: backfilling would hand a
    // receiver signed heads for a window it was not watching.
    let cursor = crate::cadence::latest_attestation(pool, space)
        .await
        .map_err(|e| SinkRefusal::Egress(e.to_string()))?
        .map(|a| a.upto)
        .unwrap_or(0);

    pool.rows(&format!(
        "INSERT INTO \"$attestsinks\" (\"space\", \"destination\", \"owner_key_id\", \
         \"owner_key\", \"destination_sig\", \"secret\", \"created_at\", \"cursor\") \
         VALUES ({}, {}, {}, {}, {}, {}, {}, {}) \
         ON CONFLICT (\"space\") DO UPDATE SET \
           \"destination\" = EXCLUDED.\"destination\", \
           \"owner_key_id\" = EXCLUDED.\"owner_key_id\", \
           \"owner_key\" = EXCLUDED.\"owner_key\", \
           \"destination_sig\" = EXCLUDED.\"destination_sig\", \
           \"secret\" = EXCLUDED.\"secret\", \
           \"created_at\" = EXCLUDED.\"created_at\"",
        sql_lit(&space.0),
        sql_lit(canonical_url),
        sql_lit(owner_key_id),
        sql_lit(owner_key),
        ordo_crypto::sealed::bytea_lit(destination_sig),
        sql_lit(secret_hex),
        now,
        cursor,
    ))
    .await
    .map_err(|e| SinkRefusal::Egress(e.to_string()))?;

    read_sink(pool, space)
        .await
        .map_err(|e| SinkRefusal::Egress(e.to_string()))?
        .ok_or(SinkRefusal::NoSink)
}

/// Stop delivering. Returns whether there was one.
///
/// Deregistration is the supported way to stop — explicit, attributable, and
/// the same shape `triggers.rs` gives a listener. Outstanding outbox rows are
/// removed with it: a delivery to a destination the owner has withdrawn is
/// not a delivery anybody wants completed.
pub async fn deregister_sink(pool: &Pool, space: &SpaceId) -> Result<bool, PoolError> {
    let rows = pool
        .rows(&format!(
            "DELETE FROM \"$attestsinks\" WHERE \"space\" = {} RETURNING \"space\"",
            sql_lit(&space.0)
        ))
        .await?;
    pool.rows(&format!(
        "DELETE FROM \"$attestdeliveries\" WHERE \"space\" = {}",
        sql_lit(&space.0)
    ))
    .await?;
    Ok(!rows.is_empty())
}

/// Move a sink's cursor back to `to`, after a restore recorded that history
/// resumed there (R2e). Answers whether a sink was there to move.
///
/// Safe because nothing routes on it any more: since R2e `enqueue_due`
/// decides what to send from `(upto, head)` and the outbox, so the cursor is
/// the ops surface's number rather than the fan-out's. Moving it back is a
/// disclosure, not an instruction.
pub async fn reset_cursor(pool: &Pool, space: &SpaceId, to: u64) -> Result<bool, PoolError> {
    if read_sink(pool, space).await?.is_none() {
        return Ok(false);
    }
    pool.rows(&format!(
        "UPDATE \"$attestsinks\" SET \"cursor\" = {to} WHERE \"space\" = {}",
        sql_lit(&space.0)
    ))
    .await?;
    Ok(true)
}

/// The sink for a space, if there is one.
pub async fn read_sink(pool: &Pool, space: &SpaceId) -> Result<Option<AttestSink>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"destination\", \"owner_key_id\", \"owner_key\", \"destination_sig\", \
             \"secret\", \"created_at\", \"cursor\" FROM \"$attestsinks\" WHERE \"space\" = {}",
            sql_lit(&space.0)
        ))
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let text = |i: usize| -> Result<String, PoolError> {
        row.get(i)
            .and_then(|c| c.clone())
            .ok_or_else(|| PoolError::Db("malformed sink row".into()))
    };
    let num = |i: usize| -> Result<u64, PoolError> {
        row.get(i)
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| PoolError::Db("malformed sink row".into()))
    };
    Ok(Some(AttestSink {
        space: space.clone(),
        destination: text(0)?,
        owner_key_id: text(1)?,
        owner_key: text(2)?,
        destination_sig: bytea_of(row.get(3).and_then(|c| c.as_ref()))
            .ok_or_else(|| PoolError::Db("sink has no destination signature".into()))?,
        secret: text(4)?,
        created_at: num(5)?,
        cursor: num(6)?,
    }))
}

/// Fan attestations at or after the sink's cursor into the outbox, then
/// advance the cursor.
///
/// The cursor moves **after** the insert has committed, so a crash between
/// the two replays the window and re-inserts — which the primary key absorbs.
/// The reverse order would be at-most-once wearing the same code, which is
/// `triggers.rs`'s sentence and is repeated here because this is the second
/// place in the platform where getting it backwards would look like it
/// worked.
pub async fn enqueue_due(pool: &Pool, space: &SpaceId, now: u64) -> Result<usize, PoolError> {
    let Some(sink) = read_sink(pool, space).await? else {
        return Ok(0);
    };
    // The cursor is still the window, and it is still at-least-once: what
    // R2e adds is two filters inside it (`orphan-checkpoints` applied to the
    // outbound side).
    //
    // - **A head the chain no longer holds at that position is not sent.**
    //   The orphan is kept as evidence of what was signed; it is not a
    //   statement about this chain any more, and re-delivering it would tell
    //   a receiver the deployment still stands behind it.
    // - **A dead letter is not resurrected.** It ran out of attempts under
    //   its own `(upto, head)`, and the operator's remedy is the ops
    //   surface, not a silent retry the next time the window reopens.
    //
    // What makes a *reused* position reach the receiver is the cursor going
    // back: `undo::record_restore` sets it to `restored_from + 1`, so the
    // window covers the positions the restore rewound through and the new
    // heads there are fanned out beside the old rows rather than instead of
    // them. That is why the outbox is keyed `(space, upto, head)`.
    let mut pending = Vec::new();
    for a in crate::cadence::attestations(pool, space).await? {
        if a.upto <= sink.cursor {
            continue;
        }
        if crate::undo::agreement(crate::undo::Db::Pool(pool), space, a.upto, a.head).await?
            != crate::undo::Agreement::Current
        {
            continue;
        }
        if dead_lettered(pool, space, a.upto, &a.head).await? {
            continue;
        }
        pending.push(a);
    }
    if pending.is_empty() {
        return Ok(0);
    }
    let tx = pool.begin_for(OpClass::AddressedWrite).await?;
    for a in &pending {
        tx.rows(&format!(
            "INSERT INTO \"$attestdeliveries\" (\"space\", \"upto\", \"head\", \"next_attempt\") \
             VALUES ({}, {}, {}, {}) ON CONFLICT (\"space\", \"upto\", \"head\") DO NOTHING",
            sql_lit(&space.0),
            a.upto,
            bytea_lit(&a.head.0),
            now,
        ))
        .await?;
    }
    tx.commit().await?;

    let high = pending.iter().map(|a| a.upto).max().unwrap_or(sink.cursor);
    pool.rows(&format!(
        "UPDATE \"$attestsinks\" SET \"cursor\" = GREATEST(\"cursor\", {high}) \
         WHERE \"space\" = {}",
        sql_lit(&space.0)
    ))
    .await?;
    Ok(pending.len())
}

/// Did this exact `(upto, head)` already run out of attempts?
///
/// Asked because the cursor can go back (R2e) and would otherwise re-open a
/// window over a head the outbox has already given up on. A *pending* row
/// needs no such question: the insert's `ON CONFLICT` absorbs a repeat, and
/// a landed delivery leaves no row because the receiver holds the record.
async fn dead_lettered(
    pool: &Pool,
    space: &SpaceId,
    upto: u64,
    head: &ChainHash,
) -> Result<bool, PoolError> {
    Ok(!pool
        .rows(&format!(
            "SELECT 1 FROM \"$attestdeadletters\" WHERE \"space\" = {} AND \"upto\" = {upto} \
             AND \"head\" = {}",
            sql_lit(&space.0),
            bytea_lit(&head.0)
        ))
        .await?
        .is_empty())
}

/// One claimed delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub space: SpaceId,
    pub upto: u64,
    /// The head this delivery is for. Part of the key since R2e, because a
    /// position can carry a pre-restore head and a post-restore one, and
    /// they are two deliveries.
    pub head: ChainHash,
    pub attempts: i32,
}

/// Claim up to `n` deliveries that are due.
///
/// `FOR UPDATE SKIP LOCKED` and `attempts = attempts + 1` at claim time, both
/// copied from `triggers::claim_runs` — incrementing at claim rather than at
/// failure is what makes a worker that dies mid-delivery still count its
/// attempt, so a destination that reliably kills the worker still reaches the
/// dead-letter cut instead of looping forever.
pub async fn claim_deliveries(pool: &Pool, now: u64, n: i64) -> Result<Vec<Delivery>, PoolError> {
    let rows = pool
        .rows(&format!(
            // A MATERIALIZED CTE so at most `n` are claimed whatever the
            // plan (`jobs::claim_jobs`).
            "WITH \"next\" AS MATERIALIZED ( \
               SELECT \"space\", \"upto\", \"head\" FROM \"$attestdeliveries\" \
               WHERE NOT \"claimed\" AND \"next_attempt\" <= {now} \
               ORDER BY \"next_attempt\", \"space\", \"upto\" LIMIT {n} FOR UPDATE SKIP LOCKED) \
             UPDATE \"$attestdeliveries\" AS d SET \"claimed\" = true, \"attempts\" = d.\"attempts\" + 1 \
             FROM \"next\" WHERE d.\"space\" = \"next\".\"space\" AND d.\"upto\" = \"next\".\"upto\" \
               AND d.\"head\" = \"next\".\"head\" \
             RETURNING d.\"space\", d.\"upto\", d.\"attempts\", d.\"head\""
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Delivery {
            space: SpaceId(
                row.first()
                    .and_then(|c| c.clone())
                    .ok_or_else(|| PoolError::Db("delivery has no space".into()))?,
            ),
            upto: row
                .get(1)
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| PoolError::Db("delivery has no position".into()))?,
            attempts: row
                .get(2)
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
            head: chain_hash_of(row.get(3).and_then(|c| c.as_ref())).unwrap_or(ChainHash::GENESIS),
        });
    }
    Ok(out)
}

/// A delivery landed. The row goes away; the receiver holds the record.
pub async fn finish_delivery(pool: &Pool, d: &Delivery) -> Result<(), PoolError> {
    pool.rows(&format!(
        "DELETE FROM \"$attestdeliveries\" WHERE \"space\" = {} AND \"upto\" = {} \
         AND \"head\" = {}",
        sql_lit(&d.space.0),
        d.upto,
        bytea_lit(&d.head.0)
    ))
    .await
    .map(|_| ())
}

/// What a failure did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failed {
    Retrying { at: u64 },
    DeadLettered,
}

/// A delivery did not land.
///
/// `retryable` comes from `ordo_egress::Refusal::retryable`, and passing it in
/// rather than re-deciding here is the point: a destination pin failure is not
/// a transient, and five retries of "your host resolves to 127.0.0.1" are five
/// more DNS queries to a name somebody chose. A non-retryable refusal
/// dead-letters on its **first** attempt.
pub async fn fail_delivery(
    pool: &Pool,
    d: &Delivery,
    destination: &str,
    reason: &str,
    retryable: bool,
    now: u64,
) -> Result<Failed, PoolError> {
    if !retryable || d.attempts >= MAX_ATTEMPTS {
        let tx = pool.begin_for(OpClass::AddressedWrite).await?;
        tx.rows(&format!(
            "INSERT INTO \"$attestdeadletters\" \
             (\"space\", \"upto\", \"head\", \"destination\", \"attempts\", \"at\", \"reason\") \
             VALUES ({}, {}, {}, {}, {}, {}, {}) \
             ON CONFLICT (\"space\", \"upto\", \"head\") DO NOTHING",
            sql_lit(&d.space.0),
            d.upto,
            bytea_lit(&d.head.0),
            sql_lit(destination),
            d.attempts,
            now,
            sql_lit(reason),
        ))
        .await?;
        tx.rows(&format!(
            "DELETE FROM \"$attestdeliveries\" WHERE \"space\" = {} AND \"upto\" = {} \
             AND \"head\" = {}",
            sql_lit(&d.space.0),
            d.upto,
            bytea_lit(&d.head.0)
        ))
        .await?;
        tx.commit().await?;
        return Ok(Failed::DeadLettered);
    }
    let at = now + backoff_secs(d.attempts);
    pool.rows(&format!(
        "UPDATE \"$attestdeliveries\" SET \"claimed\" = false, \"next_attempt\" = {at} \
         WHERE \"space\" = {} AND \"upto\" = {} AND \"head\" = {}",
        sql_lit(&d.space.0),
        d.upto,
        bytea_lit(&d.head.0)
    ))
    .await?;
    Ok(Failed::Retrying { at })
}

/// A delivery that ran out of attempts. The ops surface reads these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetter {
    pub space: SpaceId,
    pub upto: u64,
    pub destination: String,
    pub attempts: i32,
    pub at: u64,
    pub reason: String,
}

/// Every dead-lettered delivery for a space, oldest position first.
pub async fn dead_letters(pool: &Pool, space: &SpaceId) -> Result<Vec<DeadLetter>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"upto\", \"destination\", \"attempts\", \"at\", \"reason\" \
             FROM \"$attestdeadletters\" WHERE \"space\" = {} ORDER BY \"upto\"",
            sql_lit(&space.0)
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(DeadLetter {
            space: space.clone(),
            upto: row
                .first()
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            destination: row.get(1).and_then(|c| c.clone()).unwrap_or_default(),
            attempts: row
                .get(2)
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            at: row
                .get(3)
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            reason: row.get(4).and_then(|c| c.clone()).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// The newest dead-lettered deliveries across every space, newest first, at
/// most `limit`, and how many there are in all. For the deployment owner's
/// supervisor overview, which lists them beside the trigger dead letters.
/// A deployment that never set up attestation has no table, and answers
/// none.
pub async fn recent_dead_letters(
    pool: &Pool,
    limit: usize,
) -> Result<(u64, Vec<DeadLetter>), PoolError> {
    let exists = pool
        .rows("SELECT to_regclass('\"$attestdeadletters\"') IS NOT NULL")
        .await?;
    if !matches!(
        exists.first().and_then(|r| r[0].as_deref()),
        Some("t") | Some("true")
    ) {
        return Ok((0, Vec::new()));
    }
    let total = pool
        .rows("SELECT count(*) FROM \"$attestdeadletters\"")
        .await?
        .first()
        .and_then(|r| r[0].as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let rows = pool
        .rows(&format!(
            "SELECT \"space\", \"upto\", \"destination\", \"attempts\", \"at\", \"reason\" \
             FROM \"$attestdeadletters\" ORDER BY \"at\" DESC, \"space\", \"upto\" DESC \
             LIMIT {limit}"
        ))
        .await?;
    let num = |row: &Vec<Option<String>>, i: usize| -> u64 {
        row.get(i)
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    };
    let recent = rows
        .iter()
        .map(|row| DeadLetter {
            space: SpaceId(row.first().and_then(|c| c.clone()).unwrap_or_default()),
            upto: num(row, 1),
            destination: row.get(2).and_then(|c| c.clone()).unwrap_or_default(),
            attempts: num(row, 3) as i32,
            at: num(row, 4),
            reason: row.get(5).and_then(|c| c.clone()).unwrap_or_default(),
        })
        .collect();
    Ok((total, recent))
}

/// How many deliveries are waiting, and how many have given up. The two
/// numbers `/health/deep` shows.
pub async fn outbox_depth(pool: &Pool) -> Result<(u64, u64), PoolError> {
    let count = |sql: &str| -> String { sql.to_string() };
    let pending = pool
        .rows(&count("SELECT count(*) FROM \"$attestdeliveries\""))
        .await?;
    let dead = pool
        .rows(&count("SELECT count(*) FROM \"$attestdeadletters\""))
        .await?;
    let read = |rows: &[crate::TextRow]| -> u64 {
        rows.first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    };
    Ok((read(&pending), read(&dead)))
}

/// The published payload, `ordo.attestation/1`.
///
/// Deterministic: `serde_json::Map` is a `BTreeMap`, so keys are emitted in
/// sorted order and two renderings of one attestation are byte-identical.
/// That matters because the bytes are inside the transport MAC — a payload
/// whose serialisation wobbled would produce a signature the receiver could
/// not reproduce from what they stored.
///
/// The fields, and why each is owed:
///
/// | field | why |
/// |---|---|
/// | `schema` | a receiver writes a verifier against a name, not a sample |
/// | `space`, `upto`, `head`, `at` | the four values inside the Ed25519 signature — a receiver reconstructs `signedBytes` from them |
/// | `keyId`, `signature` | *cannot forge* |
/// | `signedBytes` | the exact preimage, hex. Redundant on purpose: a receiver who gets the length-prefixing wrong finds out by comparison rather than by a failed verification they will blame on the key |
/// | `destinationKeyId` | *cannot redirect*, and the fingerprint whose change is a receiver's TOFU signal |
/// | `cadence` | *cannot omit undetectably* — the next delivery's due bound, in the payload, so silence is alarmable without reading the docs |
/// | `verifiableFrom` | the chain's first retained index, so a receiver knows what a `chainProof` could still cover. Outside the signature, exactly as layer 1's is, and for the identical reason: an unsigned floor cannot make anyone *accept* anything |
pub fn payload(
    a: &Attestation,
    sink: &AttestSink,
    policy: &LogPolicy,
    verifiable_from: u64,
) -> Vec<u8> {
    let signed = Attestation::signed_bytes(&a.space, a.upto, a.head, a.at);
    let doc = serde_json::json!({
        "schema": PAYLOAD_SCHEMA,
        "space": a.space.0,
        "upto": a.upto,
        "head": hex(&a.head.0),
        "at": a.at.0,
        "keyId": a.key_id,
        "signature": hex(&a.signature),
        "signedBytes": hex(&signed),
        "destinationKeyId": sink.owner_key_id,
        "cadence": {
            "everyCommands": policy.attest_every,
            "orSeconds": policy.attest_or.as_secs(),
        },
        "verifiableFrom": verifiable_from,
    });
    serde_json::to_vec(&doc).expect("a json document built from owned values")
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_commandlog::{ChainHash, Ed25519Signer, Signer, Timestamp};

    fn owner() -> Ed25519Signer {
        Ed25519Signer::from_seed("owner-2026", &[7u8; 32])
    }

    fn sink(space: &SpaceId, url: &str) -> AttestSink {
        let o = owner();
        AttestSink {
            space: space.clone(),
            destination: url.to_string(),
            owner_key_id: o.key_id().to_string(),
            owner_key: o.verifier().public_hex(),
            destination_sig: o.sign(&consent_bytes(space, url)),
            secret: "00ff".into(),
            created_at: 0,
            cursor: 0,
        }
    }

    #[test]
    fn an_owner_signed_destination_verifies() {
        let s = sink(&SpaceId("acme".into()), "https://hooks.example.com/attest");
        assert_eq!(s.verify_destination(), Ok(()));
    }

    #[test]
    fn rewriting_the_destination_breaks_the_signature() {
        // The hostile-platform test, at the unit level: change the column,
        // keep everything else, and the delivery path refuses.
        let mut s = sink(&SpaceId("acme".into()), "https://hooks.example.com/attest");
        s.destination = "https://attacker.example.net/attest".into();
        assert_eq!(s.verify_destination(), Err(SinkRefusal::NotSigned));
    }

    #[test]
    fn a_signature_for_one_space_does_not_move_to_another() {
        // The reason the space is inside `consent_bytes`. An owner of two
        // spaces would otherwise have signed a destination for both.
        let a = sink(&SpaceId("acme".into()), "https://hooks.example.com/attest");
        let moved = AttestSink {
            space: SpaceId("other".into()),
            ..a
        };
        assert_eq!(moved.verify_destination(), Err(SinkRefusal::NotSigned));
    }

    #[test]
    fn the_consent_domain_is_not_a_checkpoint() {
        // Cross-protocol: an owner signature is not interchangeable with any
        // other signed material in the platform.
        let space = SpaceId("acme".into());
        let c = consent_bytes(&space, "https://hooks.example.com/attest");
        let cp =
            ordo_commandlog::Checkpoint::signed_bytes(&space, 1, ChainHash::GENESIS, Timestamp(0));
        assert_ne!(c, cp);
        assert!(c.starts_with(&(CONSENT_DOMAIN.len() as u64).to_be_bytes()));
    }

    #[test]
    fn consent_parts_are_length_prefixed_so_they_cannot_slide() {
        assert_ne!(
            consent_bytes(&SpaceId("ab".into()), "https://x/"),
            consent_bytes(&SpaceId("a".into()), "bhttps://x/")
        );
    }

    #[test]
    fn the_payload_is_deterministic_and_carries_the_preimage() {
        let space = SpaceId("acme".into());
        let s = sink(&space, "https://hooks.example.com/attest");
        let a = Attestation {
            space: space.clone(),
            upto: 4242,
            head: ChainHash([0xab; 32]),
            at: Timestamp(1_757_000_000),
            key_id: "cp-2026".into(),
            signature: vec![0x11; 64],
        };
        let one = payload(&a, &s, &LogPolicy::DEFAULT, 40);
        assert_eq!(one, payload(&a, &s, &LogPolicy::DEFAULT, 40));

        let doc: serde_json::Value = serde_json::from_slice(&one).unwrap();
        assert_eq!(doc["schema"], PAYLOAD_SCHEMA);
        assert_eq!(doc["upto"], 4242);
        assert_eq!(doc["verifiableFrom"], 40);
        assert_eq!(doc["destinationKeyId"], "owner-2026");
        assert_eq!(doc["cadence"]["everyCommands"], 1000);
        assert_eq!(doc["cadence"]["orSeconds"], 3600);

        // A receiver reconstructing the preimage from the four fields must
        // get the bytes the platform signed. This is the assertion that makes
        // `signedBytes` a cross-check rather than a courtesy.
        let rebuilt = Attestation::signed_bytes(
            &space,
            4242,
            ChainHash([0xab; 32]),
            Timestamp(1_757_000_000),
        );
        assert_eq!(doc["signedBytes"], hex(&rebuilt));
    }
}
