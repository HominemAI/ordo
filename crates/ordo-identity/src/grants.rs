//! The grant registry, and what it is deliberately not for.
//!
//! `AccessEngine::grant_for` mints a [`CapabilityGrant`] into a token and,
//! until now, **nothing recorded that it had happened**. Two consequences,
//! and only the second is the one that gets noticed:
//!
//! 1. **A grant could not be revoked.** Its only end was expiry. `security-
//!    logging-design.md` §5 pins *"mandatory expiry — no unexpiring grants"*,
//!    which bounds the damage, but "wait up to the expiry" is not revocation
//!    and cannot answer a user who wants an app's access to stop now.
//! 2. **`std.space.callersOf` could not be exhaustive.** §6 rule 4 requires
//!    it to enumerate principals *"alongside role, override, and **token**"*.
//!    Role, group and override are computable from the profile; the token
//!    path was not computable from anything, so the op would have answered
//!    confidently while omitting exactly the callers most worth auditing.
//!    That is why checklist 8r was left open rather than approximated.
//!
//! # What this is NOT for, and why that boundary is load-bearing
//!
//! **It is not consulted to admit a request.** §5 is explicit that the
//! pinning comparison is *"the decidable form, needing no registry
//! history"* — a token carries its own scope and its own per-op pins, and
//! the door decides from the token alone. Keeping it that way means a
//! registry outage cannot become an authorization outage, and it means the
//! door's decision does not depend on mutable state that could be edited
//! behind it.
//!
//! Revocation is the one place that boundary bends, and it bends in a
//! stated direction: a revoked grant is refused only where the registry is
//! reachable and consulted. A deployment that wants revocation to be
//! immediate and unconditional must consult it on every call and accept
//! that the registry is then in the request path. This crate does not make
//! that choice — it provides the record and the query, and names the
//! trade-off so the choice is somebody's rather than an accident.
//!
//! # Why the record is per (grant, op) rather than per grant
//!
//! `callersOf($op)` asks a question keyed on the OP. A grant holding a set
//! of ops in one row would make that a scan with a string search inside it,
//! and the audit surface would get slower exactly as a space got more
//! interesting. One row per (grant, op) makes it an index lookup. The cost
//! is that a revocation touches N rows, which is one statement.

use std::collections::BTreeMap;

use crate::pins::OpPin;
use crate::principal::{ClientId, GrantId, ManifestHash, PrincipalId, SpaceId, Timestamp};

/// Everything a mint knows, handed over in one piece.
///
/// A struct rather than nine positional arguments, and not only because
/// clippy counts: `space`, `principal` and `client` are all `String`
/// newtypes, and a caller who transposed two of them would record a grant
/// against the wrong principal with nothing to catch it. Named fields make
/// that transposition unwritable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGrant {
    pub id: GrantId,
    pub space: SpaceId,
    pub principal: PrincipalId,
    pub client: Option<ClientId>,
    /// What each listed op meant at consent time — the allowlist and the pin
    /// in one value, so a record cannot name an op it has no pin for.
    pub pins: BTreeMap<String, OpPin>,
    pub granted_at: Timestamp,
    pub expires: Timestamp,
}

/// One recorded grant, flattened per op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRecord {
    pub id: GrantId,
    pub space: SpaceId,
    pub principal: PrincipalId,
    /// The client the grant was consented to, when there was one. A grant
    /// with no client is a principal acting directly.
    pub client: Option<ClientId>,
    pub op: String,
    /// The manifest hash pinned at consent time, **for this op** — per-op
    /// now, not the combined hash a single-hash grant had to store here.
    pub manifest_hash: ManifestHash,
    /// The full pinned shape for this op: the field set and the row-scope
    /// shape, canonically encoded ([`crate::pins::encode_pins`]).
    ///
    /// This closes the gap the Postgres registry recorded as *"the consented
    /// field set is not stored, and that absence is a known gap rather than a
    /// decision"*. It was a gap because `CapabilityGrant` carried nothing to
    /// store; it does now, and a person asking *"what did I agree to?"* is
    /// asking for exactly this. The **door still does not read it** — the
    /// token carries its own pins, and a registry outage must not become an
    /// authorization outage.
    pub pin: String,
    pub granted_at: Timestamp,
    pub expires: Timestamp,
    /// When somebody withdrew it. `None` while live.
    pub revoked_at: Option<Timestamp>,
}

impl GrantRecord {
    /// Live means consented, not withdrawn, and not yet expired — all three,
    /// evaluated against one clock the caller passes in rather than a clock
    /// this crate reaches for. A record that is merely un-revoked is not a
    /// grant anybody holds.
    pub fn is_live(&self, now: Timestamp) -> bool {
        self.revoked_at.is_none() && self.expires.0 > now.0
    }
}

/// Why a registry call did not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GrantError {
    /// The store could not be reached or refused the statement. Deliberately
    /// one variant: a caller's only sound response to any of them is the
    /// same, and a taxonomy here would invite treating some as benign. The
    /// fault carries the class (busy or broken) for the edge to answer.
    Unavailable(crate::StoreFault),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantError::Unavailable(m) => write!(f, "grant registry unavailable: {m}"),
        }
    }
}

impl std::error::Error for GrantError {}

/// The durable record of what has been consented to.
#[async_trait::async_trait]
pub trait GrantRegistry: Send + Sync {
    /// Record a grant at the moment it is minted.
    ///
    /// Fallible and **must be treated as fallible by the consent page**: a
    /// grant that reaches a token without reaching the registry is one
    /// nobody can revoke or audit, so the honest handling is to fail the
    /// consent rather than issue a token that is invisible. This trait
    /// cannot enforce that, so it is said here.
    async fn record(&self, grant: &NewGrant) -> Result<(), GrantError>;

    /// Withdraw a grant. Idempotent: revoking an already-revoked grant is
    /// not an error, because a user clicking twice is not a mistake and the
    /// second click must not resurrect the first revocation's timestamp.
    async fn revoke(&self, space: &SpaceId, id: &GrantId, at: Timestamp) -> Result<(), GrantError>;

    /// Every principal holding a live grant that includes this op — the
    /// **token** column of `callersOf`'s via.
    async fn callers_of(
        &self,
        space: &SpaceId,
        op: &str,
        now: Timestamp,
    ) -> Result<Vec<GrantRecord>, GrantError>;

    /// Every grant a principal holds in a space, live or not.
    ///
    /// Includes withdrawn and expired records on purpose: this is what a
    /// person is shown when they ask what they have consented to, and a list
    /// that silently drops what was revoked cannot answer *"did I already
    /// turn that off?"* — which is the question people actually have.
    async fn grants_of(
        &self,
        space: &SpaceId,
        principal: &PrincipalId,
    ) -> Result<Vec<GrantRecord>, GrantError>;

    /// Every live grant in a space whose id begins with `prefix`, one record
    /// per op, for a listing of what a space has handed out
    /// (`std.space.shares` over the `shr_` grants). A registry that cannot
    /// list says so, and the op that needs it refuses rather than answer an
    /// empty list.
    async fn live_with_prefix(
        &self,
        _space: &SpaceId,
        _prefix: &str,
        _now: Timestamp,
    ) -> Result<Vec<GrantRecord>, GrantError> {
        Err(GrantError::Unavailable(
            "this grant registry cannot list a space's grants".into(),
        ))
    }

    /// Is this grant still live? The one query a door would make if a
    /// deployment chooses to enforce revocation in the request path.
    async fn is_live(
        &self,
        space: &SpaceId,
        id: &GrantId,
        now: Timestamp,
    ) -> Result<bool, GrantError>;
}

// ── share registration ───────────────────────────────────────────────────

/// Why a share was not registered.
///
/// One variant, because there is one rule. If a second ever appears, ask
/// whether it is a rule about the share or a rule about the grant, because
/// everything the grant already refuses stays the grant's to refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShareError {
    /// **`guest_exposure_unguarded`.** The share names an op that bypasses
    /// its guards, so the guest would see whatever the op returns with no
    /// per-row filter anywhere.
    ///
    /// This is the difference between sharing *a view of some rows* and
    /// sharing *an operation over all of them*. A guard-bearing op shown
    /// to a guest still answers only what that guest's own walk reaches,
    /// so the sharer is granting a window. An `unguarded` op shown to a
    /// guest answers everything, so the sharer is granting the table, and
    /// almost nobody who clicks "share" means that.
    ///
    /// `guards-conformance.md` reserved this class and recorded it as
    /// unreachable, "and the feature it guards is unbuilt… It becomes
    /// raisable when sharing an op to a guest principal is a thing you can
    /// register." This is that.
    UnguardedExposure { op: String },
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareError::UnguardedExposure { op } => write!(
                f,
                "guest_exposure_unguarded: `{op}` bypasses its guards, so sharing it grants \
                 every row it can reach rather than a view of some of them"
            ),
        }
    }
}

impl std::error::Error for ShareError {}

/// Register a share: a guest, a space, and the ops they may call there.
///
/// **Grant-shaped, and no syntax**, which is the architecture the owner
/// chose. A share is not a new authorization concept and a guest is not a
/// new principal kind: it is a capability grant, checked by the branch of
/// [`crate::AccessEngine::check`] that already exists, and the guest is
/// "an ordinary member holding one grant" in the corpus's own words.
///
/// So this function is small on purpose. It builds a [`NewGrant`] and
/// enforces the one rule a share has that an ordinary grant does not: a
/// shared op must be guard-bearing, or the share is not a window.
///
/// # What it deliberately does not do
///
/// It writes no acl row and names no rows. **Which** rows a guest sees is
/// the guard's answer, evaluated per query against the sharer's own data,
/// and a share that tried to enumerate rows at registration time would be
/// a second row-visibility rule competing with the one the planner already
/// compiles into every statement.
///
/// It also does not decide expiry. An open-ended share is a legitimate
/// thing to want and a dangerous default, so the caller states it.
pub fn register_share(
    id: GrantId,
    space: SpaceId,
    guest: PrincipalId,
    pins: BTreeMap<String, OpPin>,
    granted_at: Timestamp,
    expires: Timestamp,
) -> Result<NewGrant, ShareError> {
    // Checked before anything is built, so a refused share leaves no
    // half-made grant for a caller to record by mistake.
    for (op, pin) in &pins {
        if pin.unguarded {
            return Err(ShareError::UnguardedExposure { op: op.clone() });
        }
    }
    Ok(NewGrant {
        id,
        space,
        principal: guest,
        // A share is consented to by a person, not carried by a client
        // application. A share minted against a client would be a
        // delegation wearing a share's name.
        client: None,
        pins,
        granted_at,
        expires,
    })
}

#[cfg(test)]
mod share_tests {
    use super::*;
    use crate::principal::ManifestHash;

    fn pin(unguarded: bool) -> OpPin {
        OpPin {
            manifest_hash: ManifestHash::of(b"an op"),
            unguarded,
            entities: Vec::new(),
        }
    }

    fn ids() -> (GrantId, SpaceId, PrincipalId) {
        (
            GrantId("grn_share".into()),
            SpaceId("spc_host".into()),
            PrincipalId("prn_guest".into()),
        )
    }

    /// **A guard-bearing op may be shared, and the result is an ordinary
    /// grant.** No new principal kind and no new authorization path: the
    /// guest is a principal holding a capability grant, checked by the
    /// branch of `AccessEngine::check` that already exists.
    #[test]
    fn a_guarded_op_is_shareable() {
        let (id, space, guest) = ids();
        let mut pins = BTreeMap::new();
        pins.insert("listInvoices".to_string(), pin(false));

        let grant = register_share(
            id.clone(),
            space.clone(),
            guest.clone(),
            pins,
            Timestamp(100),
            Timestamp(1_000),
        )
        .expect("a guarded op is a window");

        assert_eq!(grant.id, id);
        assert_eq!(grant.space, space);
        assert_eq!(grant.principal, guest);
        assert_eq!(grant.expires, Timestamp(1_000));
        // A share is consented to by a person. A grant carrying a client
        // would be a delegation wearing a share's name.
        assert_eq!(grant.client, None);
        assert!(grant.pins.contains_key("listInvoices"));
    }

    /// **An unguarded op is refused by name.** Sharing it grants every row
    /// the op reaches rather than a view of some of them, because
    /// bypassing the guard removes the only per-row filter there is.
    #[test]
    fn an_unguarded_op_is_refused_by_name() {
        let (id, space, guest) = ids();
        let mut pins = BTreeMap::new();
        pins.insert("listInvoices".to_string(), pin(false));
        pins.insert("exportEverything".to_string(), pin(true));

        let err = register_share(id, space, guest, pins, Timestamp(100), Timestamp(1_000))
            .expect_err("an unguarded op is not a window");

        assert_eq!(
            err,
            ShareError::UnguardedExposure {
                op: "exportEverything".to_string()
            }
        );
        let msg = err.to_string();
        assert!(msg.starts_with("guest_exposure_unguarded"), "{msg}");
        assert!(msg.contains("every row it can reach"), "{msg}");
    }

    /// **One unguarded op refuses the whole share**, and nothing partial is
    /// built. A caller that recorded a half-made grant would have shared
    /// the guarded ops while believing the request failed.
    #[test]
    fn one_unguarded_op_refuses_the_whole_share() {
        let (id, space, guest) = ids();
        let mut pins = BTreeMap::new();
        for name in ["a", "b", "c"] {
            pins.insert(name.to_string(), pin(false));
        }
        pins.insert("d".to_string(), pin(true));

        assert!(matches!(
            register_share(id, space, guest, pins, Timestamp(100), Timestamp(1_000)),
            Err(ShareError::UnguardedExposure { .. })
        ));
    }
}
