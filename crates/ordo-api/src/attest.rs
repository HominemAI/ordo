//! Layer-1 client pinning, checklist 8q.
//!
//! `bootstrap.md` puts this at the bottom of the attestation stack and is
//! specific about what it costs: layer 1 is *"**always on, ~free, and
//! requires no configuration at all**: responses already carry a command-log
//! position, they now also carry the **signed head** at that position, so
//! every request is an attestation to that client and detection latency for
//! anyone actually using the system is one request."*
//!
//! Responses carried no position, so none of that was true.
//!
//! # What this is not
//!
//! It is **not** read-your-writes. A position on a response looks like a
//! read-after-write token and is not one: nothing here waits for a replica,
//! and a client that treats it as a consistency barrier will be
//! disappointed. What it buys is **tamper detection**. The client keeps the
//! last `(upto, head, signature)` it was handed; if the deployment ever
//! serves a different head for a position it already attested, or a head
//! whose signature does not verify under the space's published key, history
//! was rewritten. Every client that keeps one 100-byte pin is a witness, so
//! detection latency is one request rather than one audit.
//!
//! It **detects and does not prevent** — the same honest bound bootstrap.md
//! states for the whole attestation stack.
//!
//! # Why "~free" is a design constraint and not a boast
//!
//! Signing per request would be neither free nor more truthful: the head
//! only moves when a group is appended, so a signature computed per request
//! covers the same bytes as the last one until a write happens. So an
//! implementation signs **when the head moves** and serves the cached
//! attestation to every reader in between. If pinning cost a signature or a
//! round trip per request, deployments would turn it off, and a layer
//! everyone disables is not a layer.
//!
//! # Why a deployment with no log must say so
//!
//! [`NoAttestation`] exists so that "this deployment cannot attest" is a
//! named, deliberate choice rather than a missing header nobody notices. A
//! client cannot tell absence-because-unconfigured from
//! absence-because-stripped, so the distinction has to be made where the
//! deployment is assembled.
//!
//! # Pruning, and why a pin has to say where recomputation still starts
//!
//! A pin is only usable because the client can **recompute forward**: it
//! holds the head at `upto` and re-derives the chain from there to the head
//! it is being served now. Pruning (`ordo_exec::prune`) deletes a verified
//! prefix, which takes exactly those bytes away.
//!
//! Nothing protects an outstanding pin from that, and nothing can. The
//! pruning floor never passes the slowest *registered* consumer — and a
//! layer-1 pin is deliberately not one. `bootstrap.md` promises layer 1
//! "asks nothing of anyone" and "requires no configuration at all", which
//! means structurally that the deployment does not know the pin exists.
//! Making every pin a registered consumer would let one abandoned browser
//! tab hold a space's whole history forever, which is a worse platform than
//! the one this trade buys.
//!
//! So the loss is not prevented; it is made **visible at the moment it
//! matters**. [`HeadPin::verifiable_from`] is the earliest position from
//! which recomputation is still possible. A returning client compares it to
//! its own `upto`: at or below, nothing has been lost; above, the deployment
//! is telling it — in one request, rather than after a failed fetch — that
//! the span it was away for can no longer be recomputed here.
//!
//! ## It is deliberately outside the signature, and that is not a hole
//!
//! `signature` covers `(space, upto, head)` and nothing else, because a pin
//! must be checkable with the same routine that checks a stored
//! `Checkpoint`, and a deployment publishing two attestations of one head
//! that could disagree would be worse than publishing one.
//!
//! An unsigned `verifiable_from` cannot make a client **accept** anything.
//! Acceptance rests on recomputation against a signed head. A value that is
//! too low sends the client to fetch entries that are not there, and it
//! learns of the gap one request later; a value that is too high makes it
//! give up early, which a deployment can achieve more simply by serving no
//! entries at all. Neither converts into a forged history being believed.
//!
//! What the field is, then, is an **instruction about feasibility**, not
//! evidence. The evidence for a lawful cut is the signed checkpoint at that
//! position, which `prune` requires by construction — it is the only legal
//! cut point — and which lives in `$checkpoints` for an auditor to read with
//! the verifier they already have.

/// The signed head a response pins, flattened for the wire.
///
/// This is a [`ordo_commandlog::Checkpoint`]'s content without depending on
/// that crate: `ordo-api` is a leaf and the door has no business linking the
/// log's types into its request path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadPin {
    /// Groups covered — the head sits after index `upto - 1`. Monotonic per
    /// space, so a client can order two pins without trusting a clock.
    pub upto: u64,
    /// The chain head at that position, hex.
    pub head: String,
    /// Which key signed it, so a client can find the public half.
    pub key_id: String,
    /// Detached signature over the log's canonical signed bytes, hex.
    ///
    /// Covers `(space, upto, head)`. It does **not** cover
    /// [`HeadPin::verifiable_from`] — see the module doc for why that is the
    /// right boundary rather than an omission.
    pub signature: String,
    /// The earliest position from which this deployment can still be
    /// verified by recomputation: the chain's first retained index.
    ///
    /// `0` means nothing has been pruned and genesis is still reachable. A
    /// client whose own pin is below this value has lost the ability to
    /// recompute the span between them, and this is where it finds that out
    /// — in the response, rather than by fetching entries that are gone.
    ///
    /// Unsigned by design. The module doc gives the argument in full; the
    /// short form is that it is an instruction about feasibility and cannot
    /// be used to make a client accept anything.
    pub verifiable_from: u64,
}

/// Where the door gets the current signed head for a space.
#[async_trait::async_trait]
pub trait AttestationSource: Send + Sync {
    /// The head to pin on this response, or `None` when the space has no
    /// history yet.
    ///
    /// Called on every response, so an implementation that is not cheap
    /// makes every request not cheap. It must never fail a request: a
    /// deployment that cannot attest right now serves an unattested
    /// response, because refusing the caller's read would convert an
    /// auditing convenience into an outage.
    async fn head(&self, space: &str) -> Option<HeadPin>;

    /// The head **at a position**: what a read pins when it knows the
    /// command-log position its rows were read at.
    ///
    /// The default answers only when the current head is at that position,
    /// and otherwise leaves the response unattested. Pinning the current head
    /// on rows read at an earlier position would sign a claim the rows do not
    /// support, and an unattested answer is this trait's standing reply to "I
    /// could not check".
    async fn head_at(&self, space: &str, upto: u64) -> Option<HeadPin> {
        self.head(space).await.filter(|pin| pin.upto == upto)
    }
}

/// A deployment with no command log behind it — the in-process test
/// harnesses and the playground.
///
/// Named rather than expressed as an `Option` on the door's state, so that
/// standing up a door which cannot attest is something somebody wrote down.
pub struct NoAttestation;

#[async_trait::async_trait]
impl AttestationSource for NoAttestation {
    async fn head(&self, _space: &str) -> Option<HeadPin> {
        None
    }
}
