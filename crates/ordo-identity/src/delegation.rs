//! Delegation: one principal wearing another's face, with that person's
//! consent.
//!
//! `definition-language.md` §8 settles it in one sentence and the sentence
//! is the whole design: *"delegation is itself a grant — acting on behalf of
//! a user requires that user's own consent (user → agent, expiring,
//! revocable, command-logged), **distinct from the capability token's op
//! allowlist**. A manager granting an agent ops never grants it someone
//! else's face; without a live delegation grant the agent acts only as
//! itself."*
//!
//! # Why this is not a [`crate::principal::CapabilityGrant`]
//!
//! The two records answer different questions and are revoked by different
//! people, and folding them would silently merge those two facts:
//!
//! | | capability grant | delegation |
//! |---|---|---|
//! | asks | *may this token call these ops?* | *may this principal act as that person?* |
//! | consented by | whoever administers the agent | **the delegator, and nobody else** |
//! | revoked by | the same | **the delegator** |
//! | scope | an op allowlist, pinned per op | a person |
//!
//! A manager can hand an agent `orders.write` all day; only Ada can let it
//! be Ada. If one record carried both, a manager editing an allowlist would
//! be editing the reach of somebody else's consent — the "never grants it
//! someone else's face" rule, defeated by a data-model shortcut.
//!
//! # The two halves, and which one needs the registry
//!
//! Everything decidable **from the token** is decided by
//! [`crate::access::AccessEngine::check`] with no I/O, exactly as a
//! capability grant is: the delegation's space, its expiry, and the bound
//! below. What is *not* decidable from a bearer string is **revocation**,
//! and that is the one thing this registry is consulted for, at the door,
//! on every delegated call.
//!
//! That is a deliberate departure from [`crate::grants`], which states in
//! terms that it is *"not consulted to admit a request"*. The reason the
//! boundary moves here and only here:
//!
//! > A capability grant is the agent's **own** authority; the person who
//! > can revoke it is the person who suffers if it lingers, and
//! > "revocation takes effect at expiry" is a trade-off they made. A
//! > delegation is **someone else's face**. The user who withdraws it is
//! > not the party holding the token, gets no say in how long that token
//! > lives, and would be told that the thing acting as them stops
//! > "eventually". That is not revocation, and DL calls delegation
//! > revocable.
//!
//! The cost is stated rather than hidden: a delegation registry outage
//! **is** an authorization outage for delegated calls, and nothing else. A
//! deployment that cannot reach the registry refuses the calls that claim
//! to be somebody, and serves every ordinary call untouched. Failing closed
//! on exactly the calls whose authority cannot be confirmed is the only
//! answer that keeps the word "revocable" true.
//!
//! # One live delegation per (space, delegator, agent), and what it buys
//!
//! [`DelegationRegistry::record`] refuses a second live delegation for the
//! same triple ([`DelegationError::AlreadyLive`]). Re-consent is
//! revoke-then-record, which is two audited commands rather than one silent
//! overwrite.
//!
//! That pin is not tidiness — it is what makes the **audit join exact**. DL
//! pins the command log's attribution string verbatim as `agent:<name> on
//! behalf of user:<id>`, and that rendering is hashed into the chain, so the
//! delegation id cannot be smuggled into it without contradicting the spec
//! and changing a frozen encoding. The id therefore lives here, and a chain
//! entry joins to it on `(space, agent, delegator, at)` — a join that is
//! ambiguous only when two live delegations share the triple, which this
//! rule makes unrepresentable.

use crate::principal::{DelegationId, PrincipalId, SpaceId, Timestamp};

/// What a token **asserts** about acting for someone else.
///
/// It carries the delegation's own expiry rather than only its id so the
/// engine can refuse a lapsed delegation from the token alone — the same
/// reason a [`crate::principal::CapabilityGrant`] carries its expiry into
/// the token. An expired delegation must not depend on a registry being
/// reachable to stop working.
///
/// It carries the **delegator** because that is what the door must show the
/// audit trail and what the delegator-authority bound is evaluated against.
/// It is asserted, not proven, by the token — the token's MAC is what makes
/// the assertion trustworthy, and the registry is what makes it current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationClaim {
    pub id: DelegationId,
    /// The one space this delegation is good for. Per-space, never
    /// deployment-wide (SH-20): an agent delegated in a family space must
    /// not thereby be that person at work.
    pub space: SpaceId,
    /// The user being acted for.
    pub delegator: PrincipalId,
    /// Mandatory, like a grant's. There are no unexpiring delegations.
    pub expires: Timestamp,
}

/// Everything a consent ceremony knows, handed over in one piece.
///
/// Named fields rather than positional arguments for the reason
/// [`crate::grants::NewGrant`] gives and one more: `delegator` and `agent`
/// are the same type, and a transposition would record the *agent*
/// consenting to be acted for by the *user* — a backwards delegation that
/// nothing downstream could detect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDelegation {
    pub id: DelegationId,
    pub space: SpaceId,
    /// The human who consented. Their face is what is being lent.
    pub delegator: PrincipalId,
    /// The principal that may wear it.
    pub agent: PrincipalId,
    pub granted_at: Timestamp,
    pub expires: Timestamp,
    /// The correlation id of the command group that recorded the consent —
    /// DL's *"command-logged"*, made joinable.
    ///
    /// Stored rather than derived: a person asking *"where is the record of
    /// me agreeing to this?"* is asking for a chain entry, and a registry
    /// row that cannot name one is asking them to trust it.
    pub consent_correlation: String,
}

/// One recorded delegation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationRecord {
    pub id: DelegationId,
    pub space: SpaceId,
    pub delegator: PrincipalId,
    pub agent: PrincipalId,
    pub granted_at: Timestamp,
    pub expires: Timestamp,
    /// When the delegator withdrew it. `None` while live.
    pub revoked_at: Option<Timestamp>,
    pub consent_correlation: String,
}

impl DelegationRecord {
    /// Live means consented, not withdrawn, and not yet expired — all
    /// three, against one clock the caller passes in. The expiry instant
    /// itself is already expired, matching grants and tokens: one boundary
    /// convention across the platform.
    pub fn is_live(&self, now: Timestamp) -> bool {
        self.revoked_at.is_none() && self.expires.0 > now.0
    }

    /// The claim a token would carry for this record — so a minting path
    /// cannot describe a delegation differently from the way it was
    /// recorded.
    pub fn claim(&self) -> DelegationClaim {
        DelegationClaim {
            id: self.id.clone(),
            space: self.space.clone(),
            delegator: self.delegator.clone(),
            expires: self.expires,
        }
    }
}

/// Why a registry call did not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DelegationError {
    /// The store could not be reached or refused the statement. One
    /// variant, for [`crate::grants::GrantError`]'s reason: a caller's only
    /// sound response to any of them is the same one. The fault carries the
    /// class (busy or broken).
    Unavailable(crate::StoreFault),
    /// This delegator already has a live delegation to this agent in this
    /// space. Re-consent is revoke-then-record; a silent overwrite would
    /// leave the earlier consent unaccounted for and would break the audit
    /// join the uniqueness exists to guarantee.
    AlreadyLive { existing: DelegationId },
}

impl std::fmt::Display for DelegationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DelegationError::Unavailable(m) => write!(f, "delegation registry unavailable: {m}"),
            DelegationError::AlreadyLive { existing } => write!(
                f,
                "a live delegation (`{existing}`) already covers this delegator and agent in \
                 this space; revoke it before recording another"
            ),
        }
    }
}

impl std::error::Error for DelegationError {}

/// The durable record of who may act as whom.
#[async_trait::async_trait]
pub trait DelegationRegistry: Send + Sync {
    /// Record a delegation at the moment the delegator consents.
    ///
    /// Fallible and **must be treated as fallible by the consent
    /// ceremony**, for the reason [`crate::grants::GrantRegistry::record`]
    /// gives with more force here: a delegation that reaches a token
    /// without reaching the registry cannot be revoked *by the person whose
    /// face it is*. Issue no token until this has returned `Ok`.
    ///
    /// Refuses with [`DelegationError::AlreadyLive`] when the delegator
    /// already has a live delegation to this agent in this space —
    /// "live" evaluated at the new consent's own `granted_at`, which is the
    /// ceremony's server-observed instant, because a registry that reached
    /// for a clock of its own could disagree with the consent it is
    /// recording. Recording the **same** id twice is not a conflict: a
    /// retried ceremony must be idempotent, or one dropped response becomes
    /// a permanent refusal.
    async fn record(&self, delegation: &NewDelegation) -> Result<(), DelegationError>;

    /// Withdraw a delegation. Idempotent: a second withdrawal is not an
    /// error and must not overwrite the first one's timestamp, because that
    /// timestamp is the audit answer to *"when did this stop"*.
    async fn revoke(
        &self,
        space: &SpaceId,
        id: &DelegationId,
        at: Timestamp,
    ) -> Result<(), DelegationError>;

    /// Is this delegation still live? **The door's question**, asked on
    /// every call that claims one — see the module doc for why this one
    /// registry sits in the request path when the grant registry does not.
    async fn is_live(
        &self,
        space: &SpaceId,
        id: &DelegationId,
        now: Timestamp,
    ) -> Result<bool, DelegationError>;

    /// Every live delegation in a space — the input
    /// [`crate::access::AccessEngine::callers_of`] folds into its
    /// `delegation` via.
    async fn live_in(
        &self,
        space: &SpaceId,
        now: Timestamp,
    ) -> Result<Vec<DelegationRecord>, DelegationError>;

    /// Every delegation a **delegator** has ever granted in a space, live
    /// or not.
    ///
    /// Keyed on the delegator rather than the agent because this is the
    /// "what have I agreed to?" surface, and the person asking is the one
    /// who consented. Withdrawn and expired rows are included on purpose:
    /// a list that silently drops them cannot answer *"did I already turn
    /// that off?"*, which is the question people actually have.
    async fn granted_by(
        &self,
        space: &SpaceId,
        delegator: &PrincipalId,
    ) -> Result<Vec<DelegationRecord>, DelegationError>;
}
