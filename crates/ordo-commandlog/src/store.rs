//! Log storage behind a trait (Postgres later, memory now), signed
//! checkpoints behind a [`Signer`], and revert *planning*.

use std::collections::BTreeMap;

use crate::chain::{
    hash_group, verify_chain, verify_chain_from, ChainHash, ChainedGroup, TamperAt,
};
use crate::command::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, GroupShapeError, SpaceId,
    Timestamp,
};

/// Append rejections. Structural problems are [`GroupShapeError`];
/// correlation ids are unique **per space** — `(space, correlation)`, never
/// globally (SH2-14): a global namespace lets a write in one tenant's space
/// permanently deny a correlation id in another's, and makes the hottest
/// write path contend on one index across every tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AppendError {
    Shape(GroupShapeError),
    DuplicateCorrelation(CorrelationId),
    /// The backing store failed or answered something it could not have
    /// written. Only a durable store can produce this; the in-memory one
    /// cannot fail. It is deliberately NOT folded into `Shape`: a storage
    /// fault is an operational event, a shape error is a caller's bug, and
    /// a write path that cannot tell them apart retries the wrong one.
    Store(String),
    /// The store failed in a way that re-running the whole unit of work is
    /// the right answer to: a serialization conflict, in practice.
    ///
    /// Separate from [`AppendError::Store`] for the reason that variant's
    /// own comment gives, applied one level further in. Two spaces take
    /// different append locks and write disjoint rows, and they still share
    /// `$commandlog`, so SSI can raise `40001` between them. Flattened into
    /// `Store` that arrives at the caller as an unretryable fault and one
    /// tenant's good write is refused because another tenant wrote at the
    /// same instant. The retry loop exists for exactly this and has to be
    /// able to see it.
    Retryable(String),
}

impl std::fmt::Display for AppendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppendError::Retryable(m) => write!(f, "{m}"),
            AppendError::Shape(e) => write!(f, "{e}"),
            AppendError::DuplicateCorrelation(c) => {
                write!(f, "a group with correlation `{}` is already logged", c.0)
            }
            AppendError::Store(m) => write!(f, "command log store: {m}"),
        }
    }
}

impl std::error::Error for AppendError {}

impl From<GroupShapeError> for AppendError {
    fn from(e: GroupShapeError) -> Self {
        AppendError::Shape(e)
    }
}

/// How many entries a streaming verification pulls at once. Big enough
/// that the round trips do not dominate, small enough that verifying a
/// million-group space is bounded memory rather than a heap the size of the
/// history.
pub const VERIFY_BATCH: usize = 512;

/// The storage contract. Chains are per-space; indices are 0-based append
/// order within a space.
///
/// # Why the accessors return owned entries
///
/// They returned `&ChainedGroup` while the only implementation was a `Vec`.
/// A durable store has nothing to borrow from — it holds rows, and the
/// `ChainedGroup` is decoded on the way out — so a borrowing signature is
/// one that only an in-memory store can satisfy. The trait describes
/// storage, so it is written to the shape storage can actually take.
pub trait Store {
    /// Validate and append a group to its space's chain; returns the stored
    /// entry (hash + prev).
    fn append(&mut self, group: CommandGroup) -> Result<ChainedGroup, AppendError>;

    /// Number of groups in a space's chain.
    fn len(&self, space: &SpaceId) -> usize;

    /// True when the space has no history.
    fn is_empty(&self, space: &SpaceId) -> bool {
        self.len(space) == 0
    }

    /// The chain entry at `index`, if present.
    fn get(&self, space: &SpaceId, index: usize) -> Option<ChainedGroup>;

    /// At most `limit` stored entries in chain order, starting at the
    /// `from`-th **stored** entry (0 = the oldest still held, which is not
    /// index 0 in a pruned space).
    ///
    /// This is the streaming window verification runs over, and the reason
    /// it exists rather than a `chain()` returning everything: `verify` on a
    /// space with a million groups must not be a million-group allocation.
    fn entries(&self, space: &SpaceId, from: usize, limit: usize) -> Vec<ChainedGroup>;

    /// Current head hash (None on an empty chain).
    fn head(&self, space: &SpaceId) -> Option<ChainHash>;

    /// Locate a group by correlation id **within one space**.
    ///
    /// The space is mandatory (SH-14 / SH2-14 / Y-032). A group is
    /// single-space by construction, chains are per space, and a lookup that
    /// scans every chain turns a guessed or leaked correlation id into
    /// another tenant's before/after images. There is no unscoped lookup on
    /// this trait, so no caller can accidentally have one.
    fn find(&self, space: &SpaceId, correlation: &CorrelationId) -> Option<ChainedGroup>;

    /// Is this correlation id logged in **some other** space? A boolean and
    /// nothing else — never the space, never the group, never an image.
    ///
    /// It exists for exactly one reason: so a revert aimed at the wrong space
    /// fails as a loud [`RevertError::CrossSpace`] instead of a silent
    /// "unknown group". The disclosure is deliberate and bounded — the caller
    /// already named the correlation id, and learning "not here" is what a
    /// scoped lookup tells them anyway.
    fn is_logged_elsewhere(&self, space: &SpaceId, correlation: &CorrelationId) -> bool;

    /// Recompute the whole chain from genesis; first tampered index on
    /// failure. Correct only for a space whose prefix is still stored — a
    /// pruned space must use [`Store::verify_from`], or a lawful prune reads
    /// as a broken link at index 0.
    fn verify(&self, space: &SpaceId) -> Result<(), TamperAt> {
        self.verify_from(space, ChainHash::GENESIS, 0)
    }

    /// Recompute the stored suffix against a verified checkpoint's head.
    /// `anchor` is that head and `base_index` is the checkpoint's `upto`, so
    /// a reported index still names a position in the full chain.
    ///
    /// Streamed in [`VERIFY_BATCH`]-sized windows: each window is checked
    /// against the previous window's last hash, which is exactly what
    /// [`verify_chain_from`]'s anchor parameter is for. The default is
    /// correct for any store whose `entries` is correct, so a durable store
    /// implements one method rather than re-deriving the chain rules.
    fn verify_from(
        &self,
        space: &SpaceId,
        anchor: ChainHash,
        base_index: usize,
    ) -> Result<(), TamperAt> {
        let mut expected = anchor;
        let mut from = 0usize;
        loop {
            let batch = self.entries(space, from, VERIFY_BATCH);
            if batch.is_empty() {
                return Ok(());
            }
            verify_chain_from(space, &batch, expected, base_index + from)?;
            expected = batch.last().expect("non-empty").hash;
            from += batch.len();
            if batch.len() < VERIFY_BATCH {
                return Ok(());
            }
        }
    }
}

/// In-memory store — the reference implementation and the test double.
#[derive(Default)]
pub struct InMemoryStore {
    chains: BTreeMap<SpaceId, Vec<ChainedGroup>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        InMemoryStore::default()
    }

    /// Test-only mutable access, for tamper simulations. Not part of the
    /// [`Store`] contract — real storage has no honest use for this.
    #[cfg(test)]
    pub(crate) fn chain_mut(&mut self, space: &SpaceId) -> &mut Vec<ChainedGroup> {
        self.chains.entry(space.clone()).or_default()
    }
}

impl Store for InMemoryStore {
    fn append(&mut self, group: CommandGroup) -> Result<ChainedGroup, AppendError> {
        let space = group.validate()?.clone();
        // Duplicate detection is scoped to the group's own space: uniqueness
        // is `(space, correlation)`.
        if self.find(&space, &group.correlation).is_some() {
            return Err(AppendError::DuplicateCorrelation(group.correlation.clone()));
        }
        let chain = self.chains.entry(space).or_default();
        let prev = chain.last().map(|e| e.hash).unwrap_or(ChainHash::GENESIS);
        let hash = hash_group(prev, &group);
        let entry = ChainedGroup { group, prev, hash };
        chain.push(entry.clone());
        Ok(entry)
    }

    fn len(&self, space: &SpaceId) -> usize {
        self.chains.get(space).map_or(0, Vec::len)
    }

    fn get(&self, space: &SpaceId, index: usize) -> Option<ChainedGroup> {
        self.chains.get(space)?.get(index).cloned()
    }

    fn entries(&self, space: &SpaceId, from: usize, limit: usize) -> Vec<ChainedGroup> {
        let Some(chain) = self.chains.get(space) else {
            return Vec::new();
        };
        chain
            .get(from..)
            .map(|s| s.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    fn head(&self, space: &SpaceId) -> Option<ChainHash> {
        self.chains.get(space)?.last().map(|e| e.hash)
    }

    fn find(&self, space: &SpaceId, correlation: &CorrelationId) -> Option<ChainedGroup> {
        // One chain, chosen by key — not a scan across every tenant's history.
        self.chains
            .get(space)?
            .iter()
            .find(|e| e.group.correlation == *correlation)
            .cloned()
    }

    fn is_logged_elsewhere(&self, space: &SpaceId, correlation: &CorrelationId) -> bool {
        // The reference implementation answers by scanning the other chains;
        // a Postgres store answers with an existence probe on the
        // `(space, correlation)` index. Either way only a boolean leaves.
        self.chains
            .iter()
            .filter(|(s, _)| *s != space)
            .any(|(_, chain)| chain.iter().any(|e| e.group.correlation == *correlation))
    }

    // `verify` / `verify_from` come from the trait's streaming defaults —
    // the in-memory chain is already resident, so batching costs it nothing
    // and one implementation of the chain rules is better than two.
    fn verify(&self, space: &SpaceId) -> Result<(), TamperAt> {
        let chain = self.chains.get(space).map_or(&[][..], Vec::as_slice);
        verify_chain(space, chain)
    }
}

// ───────────────────────────── checkpoints ──────────────────────────────

/// Checkpoint signing behind a trait. The production implementation is
/// [`crate::sign::Ed25519Signer`]; a KMS- or Vault-backed one is the same
/// trait over a remote `sign` call.
///
/// `verify` defaults to re-sign-and-compare, which is correct only for
/// MAC-shaped signers and is why the default is not the interesting path:
/// re-signing needs the *private* key, and the property the design actually
/// requires is that a receiver holding only the public key can check a
/// checkpoint. That is [`crate::sign::CheckpointVerifier`], and asymmetric
/// signers route their own `verify` through it.
pub trait Signer {
    fn key_id(&self) -> &str;
    fn sign(&self, bytes: &[u8]) -> Vec<u8>;
    fn verify(&self, bytes: &[u8], signature: &[u8]) -> bool {
        self.sign(bytes) == signature
    }
}

/// Deterministic test signer — the signature is a labeled fold of the
/// input, with **no key input at all**: `key_id` is a text prefix on the
/// output, so anyone who can read a checkpoint can write one.
///
/// It is `#[cfg(test)]` for the same reason F0 says a `KeyProvider`
/// returning plaintext must not be selectable outside unit tests: something
/// that secures nothing must not be *reachable* from a deployment. It
/// survives only as the fixture that proves the `Signer` seam is
/// implementable by something other than Ed25519.
#[cfg(test)]
pub struct StubSigner {
    pub key_id: String,
}

#[cfg(test)]
impl Signer for StubSigner {
    fn key_id(&self) -> &str {
        &self.key_id
    }

    fn sign(&self, bytes: &[u8]) -> Vec<u8> {
        let mut sig = format!("stub-sig:{}:", self.key_id).into_bytes();
        let fold = bytes.iter().fold(0u64, |acc, &b| {
            acc.wrapping_mul(0x0000_0100_0000_01b3) ^ u64::from(b)
        });
        sig.extend_from_slice(&fold.to_be_bytes());
        sig
    }
}

/// A signed chain marker (`policy commandlog { checkpoint 1mo }`): a
/// verified checkpoint lets verification and pruning start mid-chain.
/// Signed bytes: space ‖ upto (u64-BE) ‖ head ‖ at (u64-BE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub space: SpaceId,
    /// Number of groups covered (the checkpoint sits after index `upto-1`).
    pub upto: u64,
    pub head: ChainHash,
    pub at: Timestamp,
    pub key_id: String,
    pub signature: Vec<u8>,
}

impl Checkpoint {
    /// The exact bytes the signature covers.
    pub fn signed_bytes(space: &SpaceId, upto: u64, head: ChainHash, at: Timestamp) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(space.0.len() as u64).to_be_bytes());
        out.extend_from_slice(space.0.as_bytes());
        out.extend_from_slice(&upto.to_be_bytes());
        out.extend_from_slice(&head.0);
        out.extend_from_slice(&at.0.to_be_bytes());
        out
    }

    pub fn verify(&self, signer: &dyn Signer) -> bool {
        let bytes = Checkpoint::signed_bytes(&self.space, self.upto, self.head, self.at);
        signer.verify(&bytes, &self.signature)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CheckpointError {
    /// Nothing to mark — an empty chain has no head.
    EmptyChain(SpaceId),
}

impl std::fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointError::EmptyChain(s) => write!(f, "space `{s}` has no history to checkpoint"),
        }
    }
}

impl std::error::Error for CheckpointError {}

/// Mark the current head of a space's chain, signed.
pub fn checkpoint(
    store: &dyn Store,
    space: &SpaceId,
    signer: &dyn Signer,
    at: Timestamp,
) -> Result<Checkpoint, CheckpointError> {
    let head = store
        .head(space)
        .ok_or_else(|| CheckpointError::EmptyChain(space.clone()))?;
    let upto = store.len(space) as u64;
    let bytes = Checkpoint::signed_bytes(space, upto, head, at);
    Ok(Checkpoint {
        space: space.clone(),
        upto,
        head,
        at,
        key_id: signer.key_id().to_string(),
        signature: signer.sign(&bytes),
    })
}

// ─────────────────────────────── revert ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RevertError {
    UnknownGroup(CorrelationId),
    /// The correlation id is logged, but not in the space the revert was
    /// aimed at (Y-032). A distinct, loud error rather than a quiet
    /// "unknown": a cross-space revert attempt is a security event, and
    /// answering it identically to a typo hides the one case worth alerting
    /// on. It names the space the caller *asked for* — which they already
    /// know — and never the space the group actually lives in.
    CrossSpace {
        correlation: CorrelationId,
        requested: SpaceId,
    },
    /// An image this revert would have to reproduce names a sealed payload
    /// the platform cannot resolve, so the inverse cannot be computed.
    ///
    /// This is the undo horizon's rule, and it is a *refusal* rather than a
    /// best effort by design (design §7): "images whose epoch key is gone
    /// render as an explicit unresolvable marker … and refuse
    /// `revert_group` with a typed rejection — never a silent empty value
    /// and never a revert computed against a missing before-image". A
    /// revert that quietly restored every field it *could* read would put
    /// a row back into a state it was never in and record that state as
    /// history.
    ///
    /// **Who raises this, and why not here.** It used to be every
    /// [`crate::Value::SealedRef`], raised by [`invert_group`], because
    /// nothing persisted what a handle named (8j) and so every handle
    /// dangled. The sealed payload store now resolves them, and whether a
    /// particular handle resolves is a question about a *deployment* —
    /// does this space still hold the payload, is the epoch key still
    /// alive — which this crate cannot answer and must not guess. It holds
    /// the arithmetic and no I/O; the applier holds the store.
    ///
    /// So inversion carries handles through and the refusal is raised by
    /// whoever resolves them, for one of the two reasons the design names:
    /// the payload's group was **pruned**, or its epoch key was
    /// **destroyed**. `reason` carries which, because an operator's second
    /// question after "which field" is always "why" — and the two answers
    /// call for different actions: a pruned payload is gone for good, a
    /// destroyed epoch is a deliberate erasure somebody performed.
    UnresolvableImage {
        command: CommandId,
        field: String,
        reason: String,
    },
    /// The group is a restore marker (`plan-production-ops.md` R2a). It
    /// wrote no rows, so there is nothing to invert, and a "revert" of it
    /// would claim to undo a database restore, which no group can do.
    RestoreMarker(CorrelationId),
}

impl std::fmt::Display for RevertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RevertError::UnknownGroup(c) => write!(f, "no logged group `{}`", c.0),
            RevertError::CrossSpace {
                correlation,
                requested,
            } => write!(
                f,
                "group `{}` is not part of space `{requested}` — a command group is \
                 single-space and reverts never cross spaces",
                correlation.0
            ),
            RevertError::UnresolvableImage {
                command,
                field,
                reason,
            } => write!(
                f,
                "command `{}` images `{field}` as a sealed reference this deployment cannot \
                 resolve ({reason}) — the revert is refused rather than computed against a \
                 value it cannot read",
                command.0
            ),
            RevertError::RestoreMarker(c) => write!(
                f,
                "group `{}` is a restore marker: it records that the database was restored \
                 and wrote no rows, so there is nothing to undo",
                c.0
            ),
        }
    }
}

impl std::error::Error for RevertError {}

/// Compute the inverse of a logged group **without applying it** — applying
/// is the platform's job, through the normal gate, as a new attributed
/// group (history is never rewritten; a revert is more history).
///
/// Every parameter is load-bearing:
/// - `space` scopes the lookup (Y-032). Command groups are strictly
///   single-space, so a revert that does not name its space is a lookup
///   across every tenant's chain, and the images it returns are the whole
///   before/after content of someone else's write.
/// - `actor` attributes the *inverse*. A revert is a new attributed group, so
///   the person or routine asking for it is part of the record from the
///   moment the plan is computed — not a detail the applier adds later.
///
/// Inversion, per command, emitted in reverse order:
/// - Create  → Delete with `before` = the created image
/// - Converge → Converge with images swapped (converge-with-before)
/// - Delete  → Create with `after` = the deleted image
///
/// Ids are minted here as `rev-<original id>` (stub for server minting);
/// `now` stamps the plan — the log records when the inverse was computed,
/// never a back-dated time.
pub fn revert_group(
    store: &dyn Store,
    space: &SpaceId,
    correlation: &CorrelationId,
    actor: &Actor,
    now: Timestamp,
) -> Result<CommandGroup, RevertError> {
    let entry = match store.find(space, correlation) {
        Some(entry) => entry,
        None if store.is_logged_elsewhere(space, correlation) => {
            return Err(RevertError::CrossSpace {
                correlation: correlation.clone(),
                requested: space.clone(),
            });
        }
        None => return Err(RevertError::UnknownGroup(correlation.clone())),
    };
    invert_group(&entry.group, actor, now)
}

/// The correlation id a revert of `original` carries.
///
/// Public because it is load-bearing twice over, and both callers must
/// spell it the same way. `(space, correlation)` is unique, so appending
/// the inverse of an already-reverted group is refused **by the chain
/// itself** — "already reverted" needs no extra bookkeeping and cannot
/// disagree with the log. And a revert of a revert is `rev-rev-…`, which
/// is a redo and is allowed, because it is a different id.
pub fn revert_correlation(original: &CorrelationId) -> CorrelationId {
    CorrelationId(format!("rev-{}", original.0))
}

/// The inversion itself, over a group already in hand — the half of
/// [`revert_group`] that has no opinion about storage.
///
/// Split out because the durable store is asynchronous and [`Store`] is
/// not: an async applier finds its group with its own query and then needs
/// exactly this. Two spellings of the inversion rules would be one too
/// many, so there is one and both paths call it.
pub fn invert_group(
    group: &CommandGroup,
    actor: &Actor,
    now: Timestamp,
) -> Result<CommandGroup, RevertError> {
    if group.restore.is_some() {
        return Err(RevertError::RestoreMarker(group.correlation.clone()));
    }
    let mut inverse = Vec::with_capacity(group.commands.len());
    for cmd in group.commands.iter().rev() {
        // A `SealedRef` used to be refused right here, on the grounds that
        // nothing could resolve one. The sealed payload store resolves
        // them now, and "does THIS handle resolve" is a question about a
        // deployment's tables and its live epochs — which this crate has
        // no way to ask, and would have to answer by guessing. So handles
        // are carried through the inversion unchanged and the refusal
        // moves to the applier, which holds the store; see
        // [`RevertError::UnresolvableImage`]. Carrying them through is not
        // a weakening: the applier reproduces every imaged field or
        // refuses the whole group, exactly as before.
        let (kind, before, after) = match cmd.kind {
            CommandKind::Create => (CommandKind::Delete, cmd.after.clone(), None),
            CommandKind::Converge => (CommandKind::Converge, cmd.after.clone(), cmd.before.clone()),
            CommandKind::Delete => (CommandKind::Create, None, cmd.before.clone()),
        };
        inverse.push(Command {
            id: CommandId(format!("rev-{}", cmd.id.0)),
            space: cmd.space.clone(),
            entity: cmd.entity.clone(),
            row_id: cmd.row_id.clone(),
            kind,
            before,
            after,
            at: now,
        });
    }
    Ok(CommandGroup {
        correlation: revert_correlation(&group.correlation),
        actor: actor.clone(),
        commands: inverse,
        // **Never inherited from the group being reverted.** The flag
        // records how *this* call was authorized — that the door granted
        // rule 6's exemption for it — not what the call undid. A revert is
        // a separate call by a separate actor (`actor` above, which is
        // usually not the owner), authorized on its own terms; copying the
        // flag across would put a claim in the chain that no door ever
        // made. The reverted group keeps its own flag, where it is true.
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    })
}
