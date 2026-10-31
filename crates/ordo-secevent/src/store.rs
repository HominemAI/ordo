//! Storage, checkpoints, and retention — the three things that make this a
//! *separate* record rather than more rows in the command chain.
//!
//! # Separately keyed
//!
//! [`StreamCheckpoint`] is signed with `ordo_commandlog::Signer`, the same
//! trait the command chain's checkpoints use, by a **different instance
//! holding a different key** — [`security_signer_from_env`] reads
//! `ORDO_SECURITY_CHECKPOINT_KEY` where `Ed25519Signer::from_env` reads
//! `ORDO_CHECKPOINT_KEY`. `sign.rs` already argues why the checkpoint
//! signer holds its own keypair rather than riding `KeyProvider`; the same
//! three arguments, applied a second time, are why *this* signer is not the
//! command chain's:
//!
//! 1. **The lifecycles differ.** Command-chain checkpoints are published to
//!    receivers as the attestation of a space's history. Security-stream
//!    checkpoints anchor a record whose retention is set by the operator's
//!    incident policy and whose readers are the space owner and the
//!    deployment's security function. A key rotated for one reason should
//!    not invalidate the other artifact's whole verifiable past.
//! 2. **The blast radius differs.** If one signing key covers both streams,
//!    then compromising the key that signs routine domain attestations is
//!    also the ability to forge the record of the compromise. The security
//!    stream exists to be the thing you can still believe after the domain
//!    record is in doubt, and one key deletes that property.
//! 3. **The audiences differ.** A space's attestations may go to a customer,
//!    a WORM bucket, a transparency log. The security stream's checkpoints
//!    go to whoever answers for the deployment. Two audiences, two published
//!    identities, two key-discovery documents.
//!
//! Belt and braces on top of the separation: [`StreamCheckpoint`]'s signed
//! bytes carry their own domain tag, so even a deployment that
//! misconfigures both variables to the same seed cannot have a command-chain
//! checkpoint verify as a security-stream one, or the reverse.
//!
//! # Separately retained
//!
//! §6.1: "Denial retention is set independently of command-history
//! retention, because a denial carries no state and is evidence about
//! behaviour, not about data." The mechanism is [`Retention`] +
//! [`prune_to_checkpoint`], and the shape of it is forced by the chain:
//!
//! - Retention drops a **whole prefix ending at a checkpoint**, never an
//!   interior entry. Removing a link from the middle breaks recomputation
//!   at the first gap, so an interior delete and tampering are the same
//!   observation.
//! - A prune is refused unless the checkpoint it cuts at **verifies** and
//!   its head matches the entry actually stored at that position. Otherwise
//!   "prune" is a way to delete evidence and blame retention.
//! - A prune never removes the **last** entry
//!   ([`StreamError::RetainedTail`]) — see that variant for why an empty
//!   stream is the one outcome retention must not be able to produce.
//! - After a prune, verification resumes from the checkpoint's head via
//!   [`SecurityStore::verify_from`]. The pruned prefix is gone; the promise
//!   that the surviving suffix is intact is not.
//!
//! The command chain's retention is a different number, set by a different
//! policy, and neither prune reads the other's checkpoints.
//!
//! # Why this floor is simpler than the command log's, stated rather than
//! assumed
//!
//! `ordo_exec::prune` computes
//! `floor = greatest checkpoint ≤ min(live consumer positions, position at
//! now − undoHorizon)` and publishes the result as `x-ordo-verifiable-from`
//! on every response. Three of those four inputs have **no counterpart
//! here**, and the reason is structural rather than a simplification anyone
//! chose:
//!
//! - **No registered consumers.** `$logconsumers` exists because mirrors,
//!   client-first devices, projections and exports read the command chain
//!   forward from a cursor they told the platform about. Nothing reads the
//!   security stream from a cursor; it has no wire surface at all (see this
//!   crate's own "What is deliberately not here"). There is therefore no
//!   position a prune could overtake.
//! - **No undo horizon.** The command chain's floor protects the window in
//!   which a group can still be inverted. A security entry cannot be undone
//!   — there is no state to restore — so there is no horizon to respect.
//! - **No client pin, so no `x-ordo-verifiable-from`.** A layer-1 head pin
//!   is minted by `ordo_api::attest` on *op responses*, over the command
//!   chain. No client ever holds a pinned security-stream head, so no
//!   client's forward recomputation can be broken by a cut, so there is
//!   nobody to disclose a floor to. The corresponding value is still
//!   *computable* — [`SecurityStore::base_index`] is exactly "the earliest
//!   position from which recomputation is possible" — and
//!   [`verifiable_from`] names it, so the day a reader surface exists the
//!   disclosure is one field and not a redesign. Publishing it before that
//!   reader exists would be a header with no audience.
//!
//! What is left is **age, anchored, with a tail**: an entry older than
//! [`Retention::keep_secs`] is eligible, the cut lands on a verified
//! checkpoint at or below it, and the last entry stays. That is the whole
//! policy, and it is smaller than the command log's because the security
//! stream genuinely owes less to fewer parties — not because the same debts
//! were quietly written off.

use std::collections::BTreeMap;

use ordo_commandlog::{
    ChainHash, CheckpointVerifier, Ed25519Signer, Signer, SignerError, TamperAt, TamperKind,
    Timestamp,
};
use ordo_fault::{FaultClass, StoreFault};

use crate::chain::{hash_entry, verify_stream_from, ChainedEntry};
use crate::event::{Scope, StreamEntry};

/// How many entries a streaming verification pulls at once. Same reasoning
/// and same number as the command chain's `VERIFY_BATCH`: bounded memory
/// over an unbounded stream.
pub const VERIFY_BATCH: usize = 512;

/// Environment variable holding the security stream's 32-byte signing seed
/// (64 hex characters). Deliberately **not** `ORDO_CHECKPOINT_KEY`.
pub const SECURITY_KEY_ENV: &str = "ORDO_SECURITY_CHECKPOINT_KEY";
/// Environment variable holding the security stream's published key id.
pub const SECURITY_KEY_ID_ENV: &str = "ORDO_SECURITY_CHECKPOINT_KEY_ID";
/// Environment variable holding the security stream's **public** key, hex,
/// for a verifier that holds no seed.
///
/// The signing seed and the published key are deliberately two variables:
/// a verifier — `ordo verify`, an auditor, a receiver — must be able to
/// check this stream while holding nothing that could sign it. This one
/// existed for the command chain (`ORDO_CHECKPOINT_PUBKEY`) from the
/// beginning and was missing here, which is a small part of why nothing
/// ever verified the security stream: there was no way to name the key
/// without also handing over the ability to forge.
pub const SECURITY_PUBKEY_ENV: &str = "ORDO_SECURITY_CHECKPOINT_PUBKEY";

/// The security stream's checkpoint signer, from the deployment's own
/// variables. Unset is a refusal, not a default — the same rule
/// `Ed25519Signer::from_env` follows, for the same reason.
pub fn security_signer_from_env() -> Result<Ed25519Signer, SignerError> {
    // Or `ORDO_SECURITY_CHECKPOINT_KEY_FILE`, a file holding the same hex.
    let hex = ordo_env::secret(SECURITY_KEY_ENV)
        .map_err(|e| SignerError::BadKey(e.to_string()))?
        .ok_or_else(|| SignerError::Missing(SECURITY_KEY_ENV.into()))?;
    let key_id = std::env::var(SECURITY_KEY_ID_ENV)
        .map_err(|_| SignerError::Missing(SECURITY_KEY_ID_ENV.into()))?;
    let mut seed = [0u8; 32];
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(SignerError::BadKey(format!(
            "seed must be 64 hex characters (32 bytes), got {}",
            hex.len()
        )));
    }
    for (i, slot) in seed.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| SignerError::BadKey("seed is not hexadecimal".into()))?;
    }
    Ok(Ed25519Signer::from_seed(key_id, &seed))
}

/// Append rejections.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamError {
    /// The backing store failed. An append failure here is an **operational
    /// alert, never a failed request** (§6.1, pinned): the stream is
    /// evidence, not a precondition, and a denial that cannot be recorded
    /// is still a denial.
    ///
    /// The fault carries its class and the cause ([`StreamError::class`],
    /// `source()`), so a full pool reads as busy and nothing is decided by
    /// reading the words.
    Store(StoreFault),
    /// A prune was asked for at a position no verified checkpoint covers.
    UnanchoredPrune { upto: u64 },
    /// The checkpoint offered for a prune did not verify under the
    /// deployment's published security key, or its head is not the head
    /// actually stored at that position.
    BadCheckpoint,
    /// The cut would have removed the stream's last remaining entry.
    ///
    /// The fourth bound, and the one no policy asked for. The command
    /// chain's prune has the same rule (`ordo_exec::prune`'s
    /// `FloorHolder::RetainedTail`) for the first reason below; the second
    /// is this artifact's own and is the harder of the two:
    ///
    /// 1. **An empty stream is ambiguous.** It reads as *"nothing was ever
    ///    written here"* from genesis and as *"the entries the checkpoint
    ///    covers are gone"* from its own checkpoint, so a lawful full prune
    ///    would make retention report both of the two things it is not.
    /// 2. **The head is where the next append chains from.**
    ///    `ordo_exec::secevent`'s append reads `(idx, hash)` of the last
    ///    stored row and continues from it; with no row it starts at index
    ///    0 with `ChainHash::GENESIS`. So emptying a scope's table does not
    ///    merely lose history — the next denial **silently forks a second
    ///    chain from genesis**, reusing the indices the prune freed, and
    ///    every checkpoint over the old chain stops matching anything
    ///    stored. Retention would have manufactured exactly the shape
    ///    tampering produces. Keeping one row keeps one head, and the chain
    ///    stays one chain.
    RetainedTail { upto: u64 },
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Store(m) => write!(f, "security-event store: {m}"),
            StreamError::UnanchoredPrune { upto } => write!(
                f,
                "refusing to prune to {upto}: retention drops whole prefixes \
                 ending at a checkpoint, never an interior entry"
            ),
            StreamError::BadCheckpoint => {
                f.write_str("the checkpoint offered for this prune does not verify")
            }
            StreamError::RetainedTail { upto } => write!(
                f,
                "refusing to prune to {upto}: it would empty the stream, and an empty \
                 stream reads as `never written` from genesis and as `entries missing` \
                 from its own checkpoint — a lawful prune must not report either"
            ),
        }
    }
}

impl StreamError {
    /// A store fault with no cause to keep: broken.
    pub fn broken(reason: impl Into<String>) -> StreamError {
        StreamError::Store(StoreFault::unavailable(reason))
    }

    /// What a caller can do about this. A store fault answers with its own
    /// class; the rest are refusals of the request.
    pub fn class(&self) -> FaultClass {
        match self {
            StreamError::Store(f) => f.class(),
            _ => FaultClass::Refused,
        }
    }

    /// Was the store busy rather than broken?
    pub fn is_busy(&self) -> bool {
        self.class().is_busy()
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StreamError::Store(f) => Some(f),
            _ => None,
        }
    }
}

/// The storage contract. Streams are per [`Scope`]; indices are 0-based
/// append order within a scope.
///
/// Accessors return owned entries for the reason `ordo_commandlog::Store`
/// does: a durable store holds rows and decodes on the way out, so a
/// borrowing signature is one only an in-memory store can satisfy.
pub trait SecurityStore {
    /// Append one entry to its scope's stream.
    fn append(&mut self, entry: StreamEntry) -> Result<ChainedEntry, StreamError>;

    /// Number of entries **still stored** in a scope's stream. After a
    /// prune this is smaller than the number ever appended; the position of
    /// the first stored entry is [`SecurityStore::base_index`].
    fn len(&self, scope: &Scope) -> usize;

    fn is_empty(&self, scope: &Scope) -> bool {
        self.len(scope) == 0
    }

    /// The full-stream index of the oldest entry still stored. Zero until a
    /// prune moves it.
    fn base_index(&self, scope: &Scope) -> usize;

    /// At most `limit` stored entries in order, starting at the `from`-th
    /// **stored** entry.
    fn entries(&self, scope: &Scope, from: usize, limit: usize) -> Vec<ChainedEntry>;

    fn head(&self, scope: &Scope) -> Option<ChainHash>;

    /// Drop every entry before full-stream index `upto`. Callers should go
    /// through [`prune_to_checkpoint`], which is the only path that checks
    /// the cut is anchored.
    fn prune_before(&mut self, scope: &Scope, upto: usize) -> Result<usize, StreamError>;

    /// Recompute the stored suffix against an anchor, streamed in
    /// [`VERIFY_BATCH`] windows.
    fn verify_from(
        &self,
        scope: &Scope,
        anchor: ChainHash,
        base_index: usize,
    ) -> Result<(), TamperAt> {
        let mut expected = anchor;
        let mut from = 0usize;
        loop {
            let batch = self.entries(scope, from, VERIFY_BATCH);
            if batch.is_empty() {
                return Ok(());
            }
            verify_stream_from(scope, &batch, expected, base_index + from)?;
            expected = batch.last().expect("non-empty").hash;
            from += batch.len();
            if batch.len() < VERIFY_BATCH {
                return Ok(());
            }
        }
    }

    /// Verify a never-pruned stream from genesis.
    fn verify(&self, scope: &Scope) -> Result<(), TamperAt> {
        let base = self.base_index(scope);
        if base != 0 {
            return Err(TamperAt {
                space: scope.key(),
                index: 0,
                kind: TamperKind::Unreadable,
            });
        }
        self.verify_from(scope, ChainHash::GENESIS, 0)
    }
}

/// In-memory stream store — the reference implementation and the test
/// double. The durable one is `ordo_exec::secevent::PgSecurityStore`, and
/// it lives there for the layering reason 8e settled: the leaf crate does
/// not pull a database driver.
#[derive(Default)]
pub struct InMemorySecurityStore {
    streams: BTreeMap<Scope, Vec<ChainedEntry>>,
    /// Full-stream index of the first entry still stored, per scope.
    base: BTreeMap<Scope, usize>,
}

impl InMemorySecurityStore {
    pub fn new() -> InMemorySecurityStore {
        InMemorySecurityStore::default()
    }

    /// Every scope with any stored history. Deployment scope included.
    pub fn scopes(&self) -> Vec<Scope> {
        self.streams.keys().cloned().collect()
    }

    /// Test-only mutable access, for tamper simulations.
    #[cfg(test)]
    pub(crate) fn stream_mut(&mut self, scope: &Scope) -> &mut Vec<ChainedEntry> {
        self.streams.entry(scope.clone()).or_default()
    }
}

impl SecurityStore for InMemorySecurityStore {
    fn append(&mut self, entry: StreamEntry) -> Result<ChainedEntry, StreamError> {
        let scope = entry.scope.clone();
        let stream = self.streams.entry(scope).or_default();
        let prev = stream.last().map(|e| e.hash).unwrap_or(ChainHash::GENESIS);
        let hash = hash_entry(prev, &entry);
        let chained = ChainedEntry { entry, prev, hash };
        stream.push(chained.clone());
        Ok(chained)
    }

    fn len(&self, scope: &Scope) -> usize {
        self.streams.get(scope).map_or(0, Vec::len)
    }

    fn base_index(&self, scope: &Scope) -> usize {
        self.base.get(scope).copied().unwrap_or(0)
    }

    fn entries(&self, scope: &Scope, from: usize, limit: usize) -> Vec<ChainedEntry> {
        let Some(stream) = self.streams.get(scope) else {
            return Vec::new();
        };
        stream
            .get(from..)
            .map(|s| s.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    fn head(&self, scope: &Scope) -> Option<ChainHash> {
        self.streams.get(scope)?.last().map(|e| e.hash)
    }

    fn prune_before(&mut self, scope: &Scope, upto: usize) -> Result<usize, StreamError> {
        let base = self.base_index(scope);
        let stream = self.streams.entry(scope.clone()).or_default();
        let drop = upto.saturating_sub(base).min(stream.len());
        stream.drain(0..drop);
        self.base.insert(scope.clone(), base + drop);
        Ok(drop)
    }
}

// ───────────────────────────── checkpoints ──────────────────────────────

/// A signed marker in a security stream. Its own type rather than
/// `ordo_commandlog::Checkpoint` for one reason that is worth a type: the
/// signed bytes carry [`StreamCheckpoint::DOMAIN`], so a command-chain
/// checkpoint and a security-stream checkpoint over the same position and
/// head are different messages even under the same key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamCheckpoint {
    pub scope: Scope,
    /// Number of entries covered — the checkpoint sits after full-stream
    /// index `upto - 1`, and a prune anchored here drops indices `< upto`.
    pub upto: u64,
    pub head: ChainHash,
    pub at: Timestamp,
    pub key_id: String,
    pub signature: Vec<u8>,
}

impl StreamCheckpoint {
    /// The domain tag inside the signed bytes.
    pub const DOMAIN: &'static [u8] = b"ordo.secevent.checkpoint.v1";

    /// The exact bytes the signature covers.
    pub fn signed_bytes(scope: &Scope, upto: u64, head: ChainHash, at: Timestamp) -> Vec<u8> {
        let key = scope.key();
        let mut out = Vec::with_capacity(96);
        out.extend_from_slice(StreamCheckpoint::DOMAIN);
        out.extend_from_slice(&(key.0.len() as u64).to_be_bytes());
        out.extend_from_slice(key.0.as_bytes());
        out.extend_from_slice(&upto.to_be_bytes());
        out.extend_from_slice(&head.0);
        out.extend_from_slice(&at.0.to_be_bytes());
        out
    }

    /// Check under a published verifying key. The key id is checked against
    /// the checkpoint's, so key discovery is part of verification — the same
    /// binding `CheckpointVerifier::verify_checkpoint` makes, and for the
    /// same reason: without it a receiver handed key A's public bytes would
    /// accept a checkpoint that claims to be signed by B.
    pub fn verify(&self, verifier: &CheckpointVerifier) -> bool {
        self.key_id == verifier.key_id()
            && verifier.verify(
                &StreamCheckpoint::signed_bytes(&self.scope, self.upto, self.head, self.at),
                &self.signature,
            )
    }
}

/// Mark the current head of a scope's stream, signed by the **security**
/// signer. `None` for a stream with no history: an empty stream has no head
/// and a checkpoint over nothing anchors nothing.
pub fn checkpoint(
    store: &dyn SecurityStore,
    scope: &Scope,
    signer: &dyn Signer,
    at: Timestamp,
) -> Option<StreamCheckpoint> {
    let head = store.head(scope)?;
    let upto = (store.base_index(scope) + store.len(scope)) as u64;
    Some(checkpoint_at(scope, upto, head, signer, at))
}

/// Sign a marker at a position and head the **caller** has established,
/// with no store attached.
///
/// The same split [`check_prune`] makes, for the same reason and with the
/// same two callers in mind. [`checkpoint`] is the synchronous path, where
/// "the current head" is two blocking trait calls;
/// `ordo_exec::secevent::checkpoint_scope` is the pooled asynchronous one,
/// where it is two awaits and a trait whose methods block cannot serve it
/// (`block_on` inside a runtime panics). What the two must share is the
/// *signed bytes* — the domain tag, the field order, the key id — because a
/// second copy of those is a second definition of what a security-stream
/// checkpoint **is**, and the day they drifted the deployment would be
/// signing markers its own verifier rejects.
///
/// No emptiness check here, and that is the type doing the work rather than
/// a rule: a `ChainHash` head is an argument, so there is no way to ask
/// this function for a checkpoint over nothing. [`checkpoint`]'s `None` and
/// `checkpoint_scope`'s `EmptyStream` are each that impossibility named in
/// their own caller's vocabulary.
pub fn checkpoint_at(
    scope: &Scope,
    upto: u64,
    head: ChainHash,
    signer: &dyn Signer,
    at: Timestamp,
) -> StreamCheckpoint {
    let bytes = StreamCheckpoint::signed_bytes(scope, upto, head, at);
    StreamCheckpoint {
        scope: scope.clone(),
        upto,
        head,
        at,
        key_id: signer.key_id().to_string(),
        signature: signer.sign(&bytes),
    }
}

/// How long a scope's security entries are kept. Independent of the command
/// chain's retention by construction — nothing here reads that policy and
/// nothing there reads this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// Entries whose `last_at` is older than this are eligible to be
    /// dropped, subject to the "whole prefix ending at a checkpoint" rule.
    /// Applies to every `Scope::Space` stream.
    pub keep_secs: u64,
    /// The same, for `Scope::Deployment`. Its own number (`plan-opslog.md`
    /// §1.4): the deployment stream is the deployment's abuse record and
    /// the only artifact where a slow sweep is legible, and a slow sweep is
    /// slow *because* it is paced to outlast an ordinary retention window.
    /// Defaults to the same ninety days as `keep_secs` so nothing changes
    /// the day this field lands; a deployment that wants the deployment
    /// stream kept longer sets [`DEPLOYMENT_RETENTION_DAYS_ENV`] without
    /// touching tenant retention at all.
    pub deployment_keep_secs: u64,
}

/// Environment variable holding the deployment stream's own retention, in
/// days. Unset keeps [`Retention::DEFAULT_KEEP_SECS`], the same default
/// `keep_secs` already has, so landing this field changes nothing until a
/// deployment opts into a longer number.
pub const DEPLOYMENT_RETENTION_DAYS_ENV: &str = "ORDO_DEPLOYMENT_SECLOG_RETENTION_DAYS";

impl Retention {
    /// Ninety days. A denial is evidence about behaviour, and the incident
    /// it belongs to is usually noticed weeks after the probing starts — so
    /// this default is longer than a service log's and shorter than a
    /// space's command history, which is the whole point of the number
    /// being its own knob.
    pub const DEFAULT_KEEP_SECS: u64 = 90 * 24 * 60 * 60;

    pub fn days(days: u64) -> Retention {
        let secs = days * 24 * 60 * 60;
        Retention {
            keep_secs: secs,
            deployment_keep_secs: secs,
        }
    }

    /// [`DEPLOYMENT_RETENTION_DAYS_ENV`], read once. `None` when unset or
    /// unparsable, never a silent "keep forever" or "prune immediately",
    /// so the caller leaves [`Retention::deployment_keep_secs`] at whatever
    /// it already was, which is the default until something overrides it.
    pub fn deployment_days_from_env() -> Option<u64> {
        std::env::var(DEPLOYMENT_RETENTION_DAYS_ENV)
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// The number that applies to `scope`: [`Retention::keep_secs`] for a
    /// space, [`Retention::deployment_keep_secs`] for the deployment. The
    /// only place this distinction is read: [`Retention::prunable_upto`]
    /// and its async twin in `ordo_exec::secevent`, so a caller never has
    /// to remember which field a given scope wants.
    pub fn keep_secs_for(&self, scope: &Scope) -> u64 {
        if scope.is_deployment() {
            self.deployment_keep_secs
        } else {
            self.keep_secs
        }
    }

    /// The largest `upto` a prune may cut at under this policy: the number
    /// of leading entries whose `last_at` is already outside the window,
    /// **never including the last stored entry**.
    ///
    /// Returns a **candidate**, not permission. The cut still has to land on
    /// a checkpoint, which is what [`prune_to_checkpoint`] enforces — and it
    /// still has to leave the tail, which [`prune_to_checkpoint`] enforces
    /// too. This function stops one short so the ordinary path (a stream
    /// every one of whose entries has aged out) reaches the floor by
    /// arithmetic rather than by refusal.
    pub fn prunable_upto(&self, store: &dyn SecurityStore, scope: &Scope, now: Timestamp) -> u64 {
        let cutoff = now.0.saturating_sub(self.keep_secs_for(scope));
        let base = store.base_index(scope);
        let end = base + store.len(scope);
        let mut n = 0usize;
        let mut aged = loop {
            let batch = store.entries(scope, n, VERIFY_BATCH);
            if batch.is_empty() {
                break base + n;
            }
            let taken = batch.len();
            let mut hit = None;
            for e in batch {
                if e.entry.last_at.0 >= cutoff {
                    hit = Some(base + n);
                    break;
                }
                n += 1;
            }
            if let Some(at) = hit {
                break at;
            }
            if taken < VERIFY_BATCH {
                break base + n;
            }
        };
        // The retained tail: the floor never reaches the last entry.
        if end > 0 {
            aged = aged.min(end - 1);
        }
        aged as u64
    }
}

impl Default for Retention {
    fn default() -> Retention {
        Retention {
            keep_secs: Retention::DEFAULT_KEEP_SECS,
            deployment_keep_secs: Retention::DEFAULT_KEEP_SECS,
        }
    }
}

/// The earliest full-stream position from which this scope's stream can
/// still be recomputed — the security stream's analogue of the command
/// log's `x-ordo-verifiable-from`.
///
/// Zero for a stream that has never been pruned, and the base index
/// afterwards. See this module's floor discussion for why the value is
/// computed and not published: there is no reader holding a pin that a
/// prune could invalidate, so there is nobody to hand it to yet.
pub fn verifiable_from(store: &dyn SecurityStore, scope: &Scope) -> u64 {
    store.base_index(scope) as u64
}

/// Drop the prefix a verified checkpoint covers, and hand back the anchor
/// verification must resume from.
///
/// Three checks, none skippable:
///
/// 1. The checkpoint **verifies** under the deployment's published security
///    key. An unverified checkpoint is a caller's claim about where the
///    chain was, and pruning on it is deleting evidence on request.
/// 2. Its `upto` is at or after the stream's current base and at or before
///    its end.
/// 3. The stored entry at `upto - 1` has exactly the checkpoint's head. A
///    checkpoint that verifies but does not match the stored chain means the
///    stream and the marker are describing different histories, and the
///    answer to that is to refuse, loudly, not to cut.
///
/// Plus the fourth, which no policy asked for: the cut must leave at least
/// one entry standing ([`StreamError::RetainedTail`]).
pub fn prune_to_checkpoint(
    store: &mut dyn SecurityStore,
    cp: &StreamCheckpoint,
    verifier: &CheckpointVerifier,
) -> Result<ChainHash, StreamError> {
    let base = store.base_index(&cp.scope);
    let end = base + store.len(&cp.scope);
    let upto = cp.upto as usize;
    let stored_head = if upto > base && upto <= end {
        store
            .entries(&cp.scope, upto - base - 1, 1)
            .first()
            .map(|e| e.hash)
    } else {
        None
    };
    check_prune(cp, verifier, base, end, stored_head)?;
    store.prune_before(&cp.scope, upto)?;
    Ok(cp.head)
}

/// The four rules, with no store attached.
///
/// [`prune_to_checkpoint`] is the synchronous path and
/// `ordo_exec::secevent::retain_scope` is the pooled asynchronous one; a
/// trait whose methods block cannot serve both (`block_on` inside a runtime
/// panics), so what they share is *this* — the decision — rather than a
/// second copy of it on each side. The delete is three lines and differs
/// per store; the argument for whether a delete is lawful is the part that
/// must not exist twice.
///
/// `stored_head` is the hash of the entry at `upto - 1` as storage actually
/// holds it, or `None` when there is no such entry.
pub fn check_prune(
    cp: &StreamCheckpoint,
    verifier: &CheckpointVerifier,
    base: usize,
    end: usize,
    stored_head: Option<ChainHash>,
) -> Result<(), StreamError> {
    if !cp.verify(verifier) {
        return Err(StreamError::BadCheckpoint);
    }
    let upto = cp.upto as usize;
    if upto < base || upto > end {
        return Err(StreamError::UnanchoredPrune { upto: cp.upto });
    }
    // The retained tail. Checked before the head comparison because it is
    // the more specific complaint: a cut at `end` on a healthy stream is a
    // policy that ran off the end, not a checkpoint that disagrees with
    // storage.
    if upto == end && end > 0 {
        return Err(StreamError::RetainedTail { upto: cp.upto });
    }
    if upto > base && stored_head != Some(cp.head) {
        return Err(StreamError::BadCheckpoint);
    }
    Ok(())
}
