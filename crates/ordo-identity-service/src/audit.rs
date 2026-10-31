//! The audit log (`plan-identity-service.md` 7): every act, chained and
//! checkpointed.
//!
//! Each row's `hash = SHA-256(prev ‖ body)`, `prev` the row before's hash
//! and all zeros for the first ([`ChainHash::GENESIS`]). `body` is the
//! row's fields, each length-prefixed, so no two rows share an encoding.
//!
//! A checkpoint is signed every [`CHECKPOINT_EVERY`] rows, inside the
//! append that reaches it, and by [`checkpoint_if_due`] when
//! [`CHECKPOINT_SECS`] have passed with rows left uncovered. It signs
//! `ordo_commandlog::Checkpoint::signed_bytes` for the space
//! `identity:<handle>` with the service's audit key, so `ordo_commandlog`'s
//! verifier reads it unchanged.
//!
//! Rows are written inside the transaction of the act they record, so an
//! act and its row commit together or not at all.

use ordo_commandlog::{
    ChainHash, Checkpoint, CheckpointVerifier, Signer as _, SpaceId, TamperAt, TamperKind,
    Timestamp,
};
use ordo_exec::{sql_lit, Pool, TxConn};
use sha2::{Digest, Sha256};

use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::store::{cell, dep, num, text};

/// A checkpoint every 100 rows.
pub const CHECKPOINT_EVERY: u64 = 100;
/// Or every 10 minutes, when rows are left uncovered.
pub const CHECKPOINT_SECS: u64 = 600;

/// Who did what to whom. `actor` is `p_…`, `m_…`, `unix:<user>`, `boot` or
/// `service`; `subject` what it was done to, empty when nothing.
#[derive(Debug, Clone, Default)]
pub struct Entry<'a> {
    pub actor: &'a str,
    pub action: &'a str,
    pub subject: &'a str,
    pub outcome: &'a str,
    pub client: &'a str,
    pub detail: &'a str,
}

/// One row as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub seq: u64,
    pub at: u64,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub outcome: String,
    pub client: String,
    pub detail: String,
    pub prev: String,
    pub hash: String,
}

/// The space a checkpoint names: `identity:<handle>`.
pub fn space(handle: &str) -> SpaceId {
    SpaceId(format!("identity:{handle}"))
}

/// The bytes a row's hash covers after `prev`.
pub fn body(handle: &str, seq: u64, at: u64, e: &Entry<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut field = |b: &[u8]| {
        out.extend_from_slice(&(b.len() as u64).to_be_bytes());
        out.extend_from_slice(b);
    };
    field(b"ordo-identity/audit/v1");
    field(handle.as_bytes());
    field(&seq.to_be_bytes());
    field(&at.to_be_bytes());
    field(e.actor.as_bytes());
    field(e.action.as_bytes());
    field(e.subject.as_bytes());
    field(e.outcome.as_bytes());
    field(e.client.as_bytes());
    field(e.detail.as_bytes());
    out
}

/// `SHA-256(prev ‖ body)`.
pub fn link(prev: ChainHash, body: &[u8]) -> ChainHash {
    let mut h = Sha256::new();
    h.update(prev.0);
    h.update(body);
    ChainHash(h.finalize().into())
}

fn parse_hash(text: &str) -> Option<ChainHash> {
    let bytes = ordo_identity::sealing::unhex(text)?;
    Some(ChainHash(bytes.try_into().ok()?))
}

async fn lock(tx: &TxConn, handle: &str) -> Result<(), IdentityError> {
    tx.rows(&format!(
        "SELECT pg_advisory_xact_lock(hashtext({}))",
        sql_lit(&format!("ordo-identity-audit:{handle}"))
    ))
    .await?;
    Ok(())
}

async fn head(tx: &TxConn, handle: &str) -> Result<(u64, ChainHash), IdentityError> {
    let rows = tx
        .rows(&format!(
            "SELECT seq, hash FROM audit WHERE {} ORDER BY seq DESC LIMIT 1",
            dep(handle)
        ))
        .await?;
    match rows.first() {
        None => Ok((0, ChainHash::GENESIS)),
        Some(r) => {
            let hash = cell(r, 1).and_then(parse_hash).ok_or_else(|| {
                IdentityError::Internal("the audit log's last row holds no hash".into())
            })?;
            Ok((num(r, 0).unwrap_or(0), hash))
        }
    }
}

/// Append one row inside `tx`, and sign a checkpoint when it is the
/// hundredth since the last. Returns the row's sequence number.
pub async fn append(
    tx: &TxConn,
    handle: &str,
    keys: &KeyRing,
    e: &Entry<'_>,
    now: u64,
) -> Result<u64, IdentityError> {
    lock(tx, handle).await?;
    let (last, prev) = head(tx, handle).await?;
    let seq = last + 1;
    let hash = link(prev, &body(handle, seq, now, e));
    tx.rows(&format!(
        "INSERT INTO audit (deployment, seq, at, actor, action, subject, outcome, client, \
         detail, prev, hash, created_at) VALUES ({}, {seq}, {now}, {}, {}, {}, {}, {}, {}, \
         {}, {}, {now})",
        sql_lit(handle),
        sql_lit(e.actor),
        sql_lit(e.action),
        sql_lit(e.subject),
        sql_lit(e.outcome),
        sql_lit(e.client),
        sql_lit(e.detail),
        sql_lit(&prev.to_string()),
        sql_lit(&hash.to_string()),
    ))
    .await?;
    if seq % CHECKPOINT_EVERY == 0 {
        write_checkpoint(tx, handle, keys, seq, hash, now).await?;
    }
    Ok(seq)
}

/// Append one row in a transaction of its own: for an act that changed
/// nothing else, a refusal above all.
pub async fn record(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    e: &Entry<'_>,
    now: u64,
) -> Result<u64, IdentityError> {
    let tx = pool.begin().await?;
    let seq = append(&tx, handle, keys, e, now).await?;
    tx.commit().await?;
    Ok(seq)
}

async fn write_checkpoint(
    tx: &TxConn,
    handle: &str,
    keys: &KeyRing,
    upto: u64,
    head: ChainHash,
    now: u64,
) -> Result<(), IdentityError> {
    let signer = keys.audit_signer()?;
    let bytes = Checkpoint::signed_bytes(&space(handle), upto, head, Timestamp(now));
    let signature = signer.sign(&bytes);
    tx.rows(&format!(
        "INSERT INTO checkpoints (deployment, upto, head, at, kid, signature, created_at) \
         VALUES ({}, {upto}, {}, {now}, {}, {}, {now}) ON CONFLICT DO NOTHING",
        sql_lit(handle),
        sql_lit(&head.to_string()),
        sql_lit(ordo_commandlog::Signer::key_id(&signer)),
        sql_lit(&ordo_identity::sealing::hex(&signature)),
    ))
    .await?;
    Ok(())
}

/// Sign a checkpoint when rows are left uncovered and the last checkpoint
/// is [`CHECKPOINT_SECS`] old (or there is none and the oldest uncovered
/// row is). Run by the service's tick.
pub async fn checkpoint_if_due(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    now: u64,
) -> Result<bool, IdentityError> {
    let tx = pool.begin().await?;
    lock(&tx, handle).await?;
    let (last, hash) = head(&tx, handle).await?;
    let covered = tx
        .rows(&format!(
            "SELECT upto, at FROM checkpoints WHERE {} ORDER BY upto DESC LIMIT 1",
            dep(handle)
        ))
        .await?;
    let (upto, since) = match covered.first() {
        Some(r) => (num(r, 0).unwrap_or(0), num(r, 1).unwrap_or(0)),
        None => {
            let first = tx
                .rows(&format!(
                    "SELECT at FROM audit WHERE {} ORDER BY seq LIMIT 1",
                    dep(handle)
                ))
                .await?;
            (0, first.first().and_then(|r| num(r, 0)).unwrap_or(now))
        }
    };
    let due = last > upto && now.saturating_sub(since) >= CHECKPOINT_SECS;
    if due {
        write_checkpoint(&tx, handle, keys, last, hash, now).await?;
    }
    tx.commit().await?;
    Ok(due)
}

/// Every row, oldest first.
pub async fn rows(pool: &Pool, handle: &str) -> Result<Vec<Row>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT seq, at, actor, action, subject, outcome, client, detail, prev, hash \
             FROM audit WHERE {} ORDER BY seq",
            dep(handle)
        ))
        .await?
        .iter()
        .map(|r| Row {
            seq: num(r, 0).unwrap_or(0),
            at: num(r, 1).unwrap_or(0),
            actor: text(r, 2),
            action: text(r, 3),
            subject: text(r, 4),
            outcome: text(r, 5),
            client: text(r, 6),
            detail: text(r, 7),
            prev: text(r, 8),
            hash: text(r, 9),
        })
        .collect())
}

/// The most rows one page of [`page`] answers.
pub const PAGE_MAX: u64 = 200;

/// One page of the log, newest first: rows before `before` (or from the
/// newest), at most `limit` (capped at [`PAGE_MAX`]). For the console's
/// "Deployment people" screen (plan 7).
pub async fn page(
    pool: &Pool,
    handle: &str,
    before: Option<u64>,
    limit: u64,
) -> Result<Vec<Row>, IdentityError> {
    let limit = limit.clamp(1, PAGE_MAX);
    let before = before
        .map(|b| format!(" AND seq < {b}"))
        .unwrap_or_default();
    Ok(pool
        .rows(&format!(
            "SELECT seq, at, actor, action, subject, outcome, client, detail, prev, hash \
             FROM audit WHERE {}{before} ORDER BY seq DESC LIMIT {limit}",
            dep(handle)
        ))
        .await?
        .iter()
        .map(|r| Row {
            seq: num(r, 0).unwrap_or(0),
            at: num(r, 1).unwrap_or(0),
            actor: text(r, 2),
            action: text(r, 3),
            subject: text(r, 4),
            outcome: text(r, 5),
            client: text(r, 6),
            detail: text(r, 7),
            prev: text(r, 8),
            hash: text(r, 9),
        })
        .collect())
}

/// Where a check of the log failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditBreak {
    /// A row's link or content, in `ordo_commandlog`'s words.
    Chain(TamperAt),
    /// A checkpoint whose head is not the chain's at that point, whose key
    /// is not one of the audit keys, or whose signature does not verify.
    Checkpoint { upto: u64, why: &'static str },
}

impl std::fmt::Display for AuditBreak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditBreak::Chain(t) => t.fmt(f),
            AuditBreak::Checkpoint { upto, why } => {
                write!(f, "the checkpoint at row {upto} does not hold: {why}")
            }
        }
    }
}

impl std::error::Error for AuditBreak {}

/// What a clean check covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checked {
    pub rows: u64,
    pub checkpoints: u64,
}

/// The audit keys' public halves, every one ever made, to check old
/// checkpoints with.
pub async fn verifiers(
    pool: &Pool,
    handle: &str,
) -> Result<Vec<CheckpointVerifier>, IdentityError> {
    let rows = pool
        .rows(&format!(
            "SELECT kid, public_key FROM signing_keys WHERE {} AND purpose = 'audit'",
            dep(handle)
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let public = ordo_identity::webauthn::b64url_decode(cell(r, 1)?).ok()?;
            CheckpointVerifier::from_public_hex(text(r, 0), &ordo_identity::sealing::hex(&public))
                .ok()
        })
        .collect())
}

/// Check the whole chain and every checkpoint against `verifiers`. A
/// caller that distrusts the database hands in the published audit keys.
pub async fn verify(
    pool: &Pool,
    handle: &str,
    verifiers: &[CheckpointVerifier],
) -> Result<Checked, IdentityError> {
    let rows = rows(pool, handle).await?;
    let cps = pool
        .rows(&format!(
            "SELECT upto, head, at, kid, signature FROM checkpoints WHERE {} ORDER BY upto",
            dep(handle)
        ))
        .await?;
    let checkpoints: Vec<Checkpoint> = cps
        .iter()
        .map(|r| Checkpoint {
            space: space(handle),
            upto: num(r, 0).unwrap_or(0),
            head: cell(r, 1)
                .and_then(parse_hash)
                .unwrap_or(ChainHash::GENESIS),
            at: Timestamp(num(r, 2).unwrap_or(0)),
            key_id: text(r, 3),
            signature: cell(r, 4)
                .and_then(ordo_identity::sealing::unhex)
                .unwrap_or_default(),
        })
        .collect();
    check(handle, &rows, &checkpoints, verifiers).map_err(IdentityError::AuditBroken)
}

/// The check itself, over rows already read. Pure, so a test can tamper
/// with a row in memory.
pub fn check(
    handle: &str,
    rows: &[Row],
    checkpoints: &[Checkpoint],
    verifiers: &[CheckpointVerifier],
) -> Result<Checked, AuditBreak> {
    let tamper = |index: usize, kind| {
        AuditBreak::Chain(TamperAt {
            space: space(handle),
            index,
            kind,
        })
    };
    let mut prev = ChainHash::GENESIS;
    let mut heads = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        let (Some(stored_prev), Some(stored_hash)) = (parse_hash(&r.prev), parse_hash(&r.hash))
        else {
            return Err(tamper(i, TamperKind::Unreadable));
        };
        if r.seq != i as u64 + 1 || stored_prev != prev {
            return Err(tamper(i, TamperKind::BrokenLink));
        }
        let e = Entry {
            actor: &r.actor,
            action: &r.action,
            subject: &r.subject,
            outcome: &r.outcome,
            client: &r.client,
            detail: &r.detail,
        };
        if link(prev, &body(handle, r.seq, r.at, &e)) != stored_hash {
            return Err(tamper(i, TamperKind::HashMismatch));
        }
        heads.push(stored_hash);
        prev = stored_hash;
    }
    for cp in checkpoints {
        let at = |why| AuditBreak::Checkpoint { upto: cp.upto, why };
        let Some(head) = (cp.upto as usize).checked_sub(1).and_then(|i| heads.get(i)) else {
            return Err(at("it covers rows the log does not hold"));
        };
        if *head != cp.head {
            return Err(at("its head is not the chain's"));
        }
        let Some(v) = verifiers.iter().find(|v| v.key_id() == cp.key_id) else {
            return Err(at("it names a key that is not an audit key"));
        };
        if !v.verify_checkpoint(cp) {
            return Err(at("its signature does not verify"));
        }
    }
    Ok(Checked {
        rows: rows.len() as u64,
        checkpoints: checkpoints.len() as u64,
    })
}
