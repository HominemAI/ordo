//! The access engine: the spec's one check at the door (§8), before
//! planning. Three layers exactly — membership roles (with groups and
//! per-member overrides), per-space access profiles, capability allowlists —
//! and nothing else. Every denial carries the attempted op; deciding where
//! denials are *recorded* is the caller's job (operational log, per design
//! §6).
//!
//! Two gates sit outside the three layers because they are not grant sources:
//!
//! - **Membership status**, ahead of everything (Z-005). A suspended
//!   principal is denied before any layer can allow them, so suspension is an
//!   authorization input rather than a field the UI happens to render.
//! - **Step-up**, after inclusion (Z-003). The requirement belongs to the
//!   *op*; the calling client may only strengthen it. See
//!   [`StepUpRequirement`].

use std::collections::{BTreeMap, BTreeSet};

use crate::delegation::DelegationClaim;
use crate::pins::{self, OpPin, PinDrift};
use crate::principal::{
    AccessProfile, CapabilityGrant, ClientId, DelegationId, GrantId, GroupId, MembershipStatus,
    PrincipalId, Role, SpaceId, Timestamp,
};

/// How fresh a user-presence assertion must be for a step-up op, by default:
/// five minutes. The docs promise "a short freshness window"; a bare "this
/// token was minted with presence once" gates the *login*, not the operation
/// (SH-18 / SH2-16), and export rides this gate. Break-glass, when built as
/// the grant-gated operation the owner ruled (corpus-rulings.md,
/// 2026-09-07), would ride it too.
pub const DEFAULT_STEP_UP_MAX_AGE_SECS: u64 = 300;

/// Validated claims off a platform token. Transport-level validity
/// (signature, token expiry) is the token layer's job; the engine sees only
/// what survived it.
///
/// `capability: Some(_)` marks a synthetic caller (agent/source/API client);
/// its grant is the *entire* authority — such tokens never fall through to
/// membership roles, structurally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenClaims {
    pub principal: PrincipalId,
    pub capability: Option<CapabilityGrant>,
    /// The **authorized party** — which registered client this token was
    /// issued to (`azp`). It selects the client's *additional* step-up
    /// demands and nothing else (Z-003): the op's own minimum is looked up
    /// independently, so presenting a permissive client cannot subtract a
    /// requirement. `None` = no client claim, which contributes nothing.
    pub client: Option<ClientId>,
    /// **When** the WebAuthn user-presence assertion behind this token was
    /// made, not merely *that* one was (SH-18). `None` = no presence at all.
    /// Freshness is evaluated at check time against
    /// [`AccessEngine::step_up_max_age_secs`], so a long-lived token cannot
    /// carry step-up authority for its whole life. Presence is not delegable,
    /// so capability tokens never satisfy step-up whatever this says.
    pub user_presence_at: Option<Timestamp>,
    /// **On whose behalf**, when this token claims to be acting for
    /// somebody (DL §8). `None` — the ordinary case — is a principal acting
    /// only as itself, which is exactly what §8 says an agent without a
    /// delegation does.
    ///
    /// Orthogonal to `capability` on purpose: the allowlist says *which
    /// ops*, the delegation says *whose face*, and DL calls them distinct.
    /// A token may carry either, both, or neither.
    ///
    /// What the engine can decide from this alone is the delegation's
    /// space, its expiry, and the delegator-authority bound
    /// ([`DenyReason::DelegationBeyondDelegator`]). **Revocation is not
    /// decidable from a bearer string** and is the door's registry lookup —
    /// see [`crate::delegation`] for why that one check earns its place in
    /// the request path.
    pub delegation: Option<DelegationClaim>,
    /// **Which sign-in** this token belongs to, and when it began
    /// (`crate::session::SessionClaim`). `None` for a capability token and
    /// for a delegated one, neither of which is a session.
    ///
    /// The engine does not read this. Whether the session has been ended is
    /// not decidable from a bearer string, for the same reason a
    /// delegation's revocation is not: it is a fact about the deployment,
    /// and the door asks `crate::session::SessionRevocations` about it
    /// before the engine is reached. What the claim gives the engine's
    /// caller is the *referent*: the one thing a token needed before
    /// "log this session out" could mean anything.
    pub session: Option<crate::session::SessionClaim>,
}

/// Why a check denied. Paired with the attempted op in [`Decision::Deny`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    /// The capability grant names a different space than the one the request
    /// bound (SH2-2 / Y-002). Checked *first* in the capability path, so the
    /// cheapest check is also the isolating one.
    ///
    /// Deliberately names neither space: a denial record is attacker-
    /// influenced (the caller chose the space id), and echoing back a grant's
    /// home space would turn refusals into an oracle for it.
    GrantSpaceMismatch,
    /// Capability grant past its expiry (the expiry instant itself denies).
    GrantExpired,
    /// The grant's pinned manifest hash no longer matches the deployment —
    /// scope expanded or an op vanished. Dead until re-consent.
    ManifestChanged,
    /// Op absent from the capability allowlist.
    OpNotInAllowlist,
    /// The principal has no membership in the space.
    NoMembership,
    /// The principal's membership in this space is **suspended** (Z-005 /
    /// SH3-005). Checked before role, profile, group, override, and capability
    /// resolution, so nothing downstream can grant its way back in: a
    /// capability token does not escape it (the grant belongs to a principal,
    /// and this principal is stopped), and neither does a group that confers a
    /// role. Distinct from [`DenyReason::NoMembership`] on purpose — an
    /// operator reading a denial needs "we stopped this account" and "this
    /// account was never here" to be different sentences.
    PrincipalSuspended,
    /// The principal holds a seat here with **no role**: a guest
    /// (`plan-sharing.md` §3.1). A guest reaches only what a share grant
    /// lists, through a capability token, so the human path refuses it
    /// before any profile is read. Distinct from [`DenyReason::NoMembership`]
    /// because "this person has a seat and no standing" and "this person is
    /// not here" are different sentences for the tenant owner.
    NoRole,
    /// The space has no access profile (nothing is allowed by default).
    NoProfileForSpace,
    /// The profile does not list the op for the caller's resolved level —
    /// after role, group allowlists, and per-member overrides.
    OpNotInRole { role: Role },
    /// The op demands step-up and the token carries no user presence at all
    /// (or is a capability token, which can never carry it).
    StepUpRequired,
    /// The op demands step-up and the token's presence assertion is older
    /// than the freshness window (or claims to be from the future, which is a
    /// clock lie, not a presence).
    StepUpStale { age_secs: u64, max_age_secs: u64 },
    /// The token claims a delegation issued for a **different space**
    /// (SH-20: delegation is per-space, never per-principal-globally).
    ///
    /// Names neither space, for [`DenyReason::GrantSpaceMismatch`]'s
    /// reason: the caller chose the space id, so echoing back the
    /// delegation's home space turns refusals into an oracle for it.
    DelegationSpaceMismatch,
    /// The claimed delegation is past its expiry. Decided from the token,
    /// so a lapsed delegation stops working whether or not the registry is
    /// reachable.
    DelegationExpired,
    /// The claimed delegation is live and the **delegator cannot call this
    /// op themselves** — suspended, not a member, or simply not granted it.
    ///
    /// `security-logging-design.md` §6 rule 4: *"a delegation carries only
    /// the delegator's authority."* Without this the two consents multiply
    /// instead of intersecting: whoever administers the agent lists an op
    /// on its allowlist, the user consents to "act as me", and the agent
    /// reaches something the user never could. The delegation would then be
    /// a privilege-escalation primitive dressed as a convenience.
    ///
    /// Evaluated against the delegator's **membership** — role, groups and
    /// per-member overrides, suspension honoured — and not against step-up,
    /// because step-up asks whether a human is present *now* and presence
    /// is not delegable at all ([`AccessEngine::step_up_decision`] refuses
    /// every capability token regardless).
    DelegationBeyondDelegator,
    /// The claimed delegation is not live in the registry — withdrawn,
    /// unknown, or unreachable. Raised by the **door's** registry gate, not
    /// by [`AccessEngine::check`], which never does I/O.
    ///
    /// One variant for withdrawn/unknown/unreachable together: the
    /// registry answers one boolean, and a caller's only sound response to
    /// any of the three is the same — stop, and ask the delegator for a
    /// fresh consent.
    DelegationNotLive,
    /// The op is closed to every delegated token, whatever its allowlist
    /// and its delegator's reach ([`AccessEngine::forbid_delegation`]).
    ///
    /// SH-20: *"destructive ops non-delegable by default, opt-in per op
    /// listed individually"*. The deployment marks which ops; the engine
    /// only holds the set and checks it. An agent that may act as Ada may
    /// still not delete as Ada unless the app developer opened that op.
    OpNotDelegable,
}

/// The outcome of the one check. Denials always name the attempted op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny { op: String, reason: DenyReason },
}

impl Decision {
    fn deny(op: &str, reason: DenyReason) -> Decision {
        Decision::Deny {
            op: op.to_string(),
            reason,
        }
    }

    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }
}

/// The **drift report** for one grant: what each listed op meant when the
/// person consented, against what it means now.
///
/// This is what an audit record is written from. §5 requires narrowing to be
/// *"audit-noted"*, and a check that returns only allow/deny has nothing to
/// note with — so the classification is computed once, here, and both the
/// decision and the record read the same answer.
///
/// Ops are in name order, and **every listed op is evaluated**, not only the
/// one being called: consent was a single act over a set, and DL §8 says an
/// expansion makes the grant *"dead for that space until re-consent"*. The
/// per-op half of the ruling is about the **comparison** — an unrelated op's
/// *narrowing* must not kill anything, which the combined hash could not
/// express — not about letting a grant survive in pieces. A grant that is
/// alive for some of its ops and dead for others is a state nobody consented
/// to and no consent page can describe.
pub fn grant_drift(
    grant: &CapabilityGrant,
    deployed: &BTreeMap<String, OpPin>,
) -> Vec<(String, PinDrift)> {
    grant
        .pins()
        .iter()
        .map(|(op, consented)| {
            let drift = match deployed.get(op) {
                Some(current) => pins::compare(consented, current),
                None => PinDrift::Undeployed,
            };
            (op.clone(), drift)
        })
        .collect()
}

/// What a call must prove beyond the ordinary check.
///
/// # Why this is ordered, and why the op owns the minimum (Z-003 / SH3-003)
///
/// Step-up used to be a flat set resolved "from the calling client's
/// `stepUp […]` config". With one issuer and many apps that makes an invariant
/// into a client preference: register a second relying party that omits
/// `space.delete`, present its token to the one `/op/{name}` route, and the
/// export/delete/device gate is gone — no WebAuthn defeated, just a weaker
/// client chosen.
///
/// So the requirement is a property of the **op** (its artifact, or the
/// immutable platform-op definition), and a client's declaration is combined
/// with it by [`StepUpRequirement::strongest`] — a max, never a replacement.
/// A client can therefore only ever *add*: demand presence for an op that did
/// not need it, or demand a shorter freshness window than the deployment's.
/// There is no value a client can supply that removes the op's minimum,
/// because `None` is the identity of `strongest`, not an override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepUpRequirement {
    /// Nothing beyond the ordinary access check.
    None,
    /// A user-presence assertion no older than `max_age_secs`.
    ///
    /// `u64::MAX` means "the deployment's window" — whatever
    /// [`AccessEngine::step_up_max_age_secs`] is set to — so an op that merely
    /// demands presence does not pin a window at registration time and then
    /// quietly ignore a later, stricter deployment setting. The effective
    /// window is always the **min** of this and the deployment's.
    Presence { max_age_secs: u64 },
}

impl StepUpRequirement {
    /// Presence for a step-up op, at whatever window the deployment runs.
    pub const PRESENCE: StepUpRequirement = StepUpRequirement::Presence {
        max_age_secs: u64::MAX,
    };

    /// The **stronger** of two requirements. Presence beats nothing; between
    /// two presences the shorter freshness window wins. This is the whole of
    /// the "clients may add, never subtract" rule, in one place.
    pub fn strongest(self, other: StepUpRequirement) -> StepUpRequirement {
        match (self, other) {
            (StepUpRequirement::None, other) => other,
            (this, StepUpRequirement::None) => this,
            (
                StepUpRequirement::Presence { max_age_secs: a },
                StepUpRequirement::Presence { max_age_secs: b },
            ) => StepUpRequirement::Presence {
                max_age_secs: a.min(b),
            },
        }
    }

    pub fn demands_presence(self) -> bool {
        matches!(self, StepUpRequirement::Presence { .. })
    }
}

/// **How** a caller reaches an op — the *via* of `std.space.callersOf`.
///
/// The four words are `security-logging-design.md` §6 rule 4's, in its
/// order: *"the group named as the via alongside role, override, and
/// token"*. They are a closed set on purpose. A via is the thing a manager
/// acts on to remove access, and a fifth kind of access with no fifth via
/// would be access nobody can find — which is the exact failure the rule
/// was written against.
///
/// The ordering is deliberate and is what sorts the answer: role first
/// (the ordinary case), then group, then override (the exception), then
/// token (the delegated one). A reader scanning a space's callers meets
/// them in increasing order of *how much explaining they need*.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Via {
    /// The principal's own role, and its allowlist lists the op.
    Role(Role),
    /// A group they belong to in this space. `conferred` is the role the
    /// group grants when the op arrived through
    /// [`AccessProfile::role_by_group`] rather than through the group's own
    /// allowlist — two different edits to remove it, so they are two
    /// different answers.
    Group {
        group: GroupId,
        conferred: Option<Role>,
    },
    /// A per-member allow override. Named without the op because the op is
    /// the question.
    Override,
    /// A live capability grant. This is the column 8r was blocked on: until
    /// the grant registry existed, nothing recorded that a token had been
    /// minted, so this variant could not be produced and the answer would
    /// have omitted exactly the callers most worth auditing.
    Token {
        grant: GrantId,
        client: Option<ClientId>,
    },
    /// A live **delegation**: this principal reaches the op by acting as
    /// somebody who reaches it (DL §8).
    ///
    /// # Why rule 4's four words became five
    ///
    /// The four are closed *"on purpose"*, and the argument for the closure
    /// is the argument for extending it here: *"a via is the thing a
    /// manager acts on to remove access, and a fifth kind of access with no
    /// fifth via would be access nobody can find."* A delegated agent is a
    /// fifth kind of access — it is removed neither by editing the profile
    /// nor by revoking the token, but by the **delegator** withdrawing
    /// their consent, which is a different act by a different person. Under
    /// the four words it would have been invisible, or worse, indexed under
    /// `token`, which would send a manager to revoke a grant and find the
    /// caller still there.
    ///
    /// It sorts **last**, continuing the existing ordering rule: a reader
    /// meets vias in increasing order of how much explaining they need, and
    /// this one needs the most — the answer to *"how do I stop them"* is
    /// not the manager's to give.
    Delegation {
        delegation: DelegationId,
        delegator: PrincipalId,
    },
}

impl Via {
    /// The via's kind as rule 4 spells it — `role`, `group`, `override`,
    /// `token` — plus `delegation`. The wire word, stable across
    /// renderings.
    pub fn kind(&self) -> &'static str {
        match self {
            Via::Role(_) => "role",
            Via::Group { .. } => "group",
            Via::Override => "override",
            Via::Token { .. } => "token",
            Via::Delegation { .. } => "delegation",
        }
    }

    /// What to name alongside the kind: the role, the group (and any role it
    /// confers), the grant and its client. Empty for an override, whose
    /// whole content is that it is one.
    pub fn detail(&self) -> String {
        match self {
            Via::Role(r) => r.as_str().to_string(),
            Via::Group {
                group,
                conferred: None,
            } => group.0.clone(),
            Via::Group {
                group,
                conferred: Some(role),
            } => format!("{} (confers {})", group.0, role.as_str()),
            Via::Override => String::new(),
            Via::Token { grant, client } => match client {
                Some(c) => format!("{} (client {})", grant.0, c.0),
                None => grant.0.clone(),
            },
            // The delegator leads: the person is the answer to "who is this
            // agent being right now", and the id is what a revocation
            // names. Both, because a manager reading this needs to know
            // whom to ask, and the delegator needs to know what to revoke.
            Via::Delegation {
                delegation,
                delegator,
            } => format!("on behalf of {delegator} (delegation {delegation})"),
        }
    }
}

/// One principal, reached one way. A principal reachable three ways is
/// three of these — see [`AccessEngine::callers_of`] for why that is the
/// answer rather than a deduplication.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Caller {
    pub principal: PrincipalId,
    pub via: Via,
}

/// A principal's standing in one space: their direct role, the groups they
/// belong to *there*, and whether the membership is active. Group membership
/// is per space and non-transitive — nested groups are out of v1 (design §6
/// rule 1 [settled]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Membership {
    /// The `members` row id: what `$caller` binds to inside the space and
    /// what a guard compares against. The map key holds the **principal**,
    /// the person the deployment knows; this is that person in this space.
    member_id: String,
    /// `None` is a **guest**: a seat with no role, whose authority is a
    /// share grant and nothing else (`plan-sharing.md` §3.1).
    role: Option<Role>,
    groups: BTreeSet<GroupId>,
    /// Z-005: an authorization input, not a label. A suspended membership
    /// keeps its role and groups so reactivation restores them exactly, and
    /// denies every op until it does.
    status: MembershipStatus,
}

/// The engine. Holds the runtime access state the check reads: memberships
/// and profiles (user-owned runtime data, mutated only by commands through
/// the gate), the deployed per-op **pins**, the **op-intrinsic** step-up
/// minimums, and each registered client's additional demands.
pub struct AccessEngine {
    memberships: BTreeMap<(SpaceId, PrincipalId), Membership>,
    /// The reverse of `memberships`: `(space, member id)` to the principal
    /// holding it. `$caller` arrives at the platform ops as a member id, and
    /// an engine question ("does this asker reach this op") is asked of a
    /// principal.
    member_ids: BTreeMap<(SpaceId, String), PrincipalId>,
    profiles: BTreeMap<SpaceId, AccessProfile>,
    /// op name → what the op means now: its manifest hash, its field set and
    /// its row-scope shape. Updated on deploy, and the right-hand side of
    /// every pin comparison.
    deployed: BTreeMap<String, OpPin>,
    /// The op's own minimum — from its artifact or the immutable platform-op
    /// definition. No client can lower an entry here (Z-003).
    op_step_up: BTreeMap<String, StepUpRequirement>,
    /// Per-client *additions*, folded in by `strongest`. A client that lists
    /// nothing for an op contributes [`StepUpRequirement::None`], which is the
    /// identity — so omitting an op is not a way to exempt it.
    client_step_up: BTreeMap<ClientId, BTreeMap<String, StepUpRequirement>>,
    /// How old a presence assertion may be and still satisfy step-up, unless
    /// an op or client demands something shorter.
    step_up_max_age_secs: u64,
    /// Ops no delegated token may call (SH-20). Empty is "every op the
    /// allowlist and the delegator's reach admit"; the deployment fills it.
    not_delegable: BTreeSet<String>,
    /// Whose two-step claims count as presence, and where
    /// (`plan-auth-federated.md` §8.2). `None` is the default: no provider's
    /// claim satisfies step-up anywhere.
    upstream: Option<std::sync::Arc<crate::upstream::UpstreamAcceptance>>,
}

impl Default for AccessEngine {
    fn default() -> Self {
        AccessEngine {
            memberships: BTreeMap::new(),
            member_ids: BTreeMap::new(),
            profiles: BTreeMap::new(),
            deployed: BTreeMap::new(),
            op_step_up: BTreeMap::new(),
            client_step_up: BTreeMap::new(),
            step_up_max_age_secs: DEFAULT_STEP_UP_MAX_AGE_SECS,
            not_delegable: BTreeSet::new(),
            upstream: None,
        }
    }
}

impl AccessEngine {
    pub fn new() -> Self {
        AccessEngine::default()
    }

    /// Narrow (or widen) the step-up freshness window. Export-class ops want
    /// minutes, not hours; the window is a deployment decision, never a
    /// hard-coded constant hiding in the check.
    pub fn with_step_up_max_age(mut self, secs: u64) -> Self {
        self.step_up_max_age_secs = secs;
        self
    }

    pub fn step_up_max_age_secs(&self) -> u64 {
        self.step_up_max_age_secs
    }

    /// Let a provider's two-step claim satisfy step-up where `acceptance`
    /// says so. The object is shared, so a tenant's choice written to it is
    /// read by the next check.
    pub fn accept_upstream(
        &mut self,
        acceptance: std::sync::Arc<crate::upstream::UpstreamAcceptance>,
    ) {
        self.upstream = Some(acceptance);
    }

    /// Record a membership (layer 1). In the platform this is a command;
    /// here it is a direct mutation for the in-memory build. Existing group
    /// memberships in the space survive a role change.
    ///
    /// The member id is the principal's own string, which is what every
    /// caller before enrolment E1 meant by a member. A deployment reading
    /// the `members` table uses [`AccessEngine::set_member`], where the two
    /// differ.
    pub fn set_membership(&mut self, space: &SpaceId, principal: &PrincipalId, role: Role) {
        self.set_member(space, principal, &principal.0, Some(role));
    }

    /// Record that `principal` is member `member_id` of `space`, at `role`.
    ///
    /// **One person, one member id per space.** The same principal is a
    /// different member in each tenant (`plan-enrolment.md` §2.4), and the
    /// door resolves `(space, principal)` to that member. A second call for
    /// the same pair replaces the member id and the role and keeps the
    /// groups. A `role` of `None` records a guest seat.
    pub fn set_member(
        &mut self,
        space: &SpaceId,
        principal: &PrincipalId,
        member_id: &str,
        role: Option<Role>,
    ) {
        let key = (space.clone(), principal.clone());
        if let Some(old) = self.memberships.get(&key) {
            self.member_ids
                .remove(&(space.clone(), old.member_id.clone()));
        }
        self.member_ids
            .insert((space.clone(), member_id.to_string()), principal.clone());
        self.memberships
            .entry(key)
            .and_modify(|m| {
                m.role = role;
                m.member_id = member_id.to_string();
            })
            .or_insert(Membership {
                member_id: member_id.to_string(),
                role,
                groups: BTreeSet::new(),
                status: MembershipStatus::Active,
            });
    }

    /// The member id this principal holds in this space, if any. What the
    /// door hands the executor as `$caller`.
    pub fn member_id_of(&self, space: &SpaceId, principal: &PrincipalId) -> Option<&str> {
        self.memberships
            .get(&(space.clone(), principal.clone()))
            .map(|m| m.member_id.as_str())
    }

    /// The principal holding this member id in this space, if any. The
    /// reverse of [`AccessEngine::member_id_of`], for a platform op that is
    /// handed `$caller` and must ask the engine about a person.
    pub fn principal_of_member(&self, space: &SpaceId, member_id: &str) -> Option<&PrincipalId> {
        self.member_ids.get(&(space.clone(), member_id.to_string()))
    }

    /// Every space this principal is a member of, with the role and the
    /// status held there, in space order.
    ///
    /// **Only ever asked about the caller.** The list of tenants somebody
    /// belongs to is a disclosure in itself (owner, 2026-09-29: a person may
    /// see their own, nobody else sees it), so the one op that reads this
    /// derives the principal from the door's proof and takes no parameter
    /// that could name another person.
    pub fn spaces_of(
        &self,
        principal: &PrincipalId,
    ) -> Vec<(SpaceId, Option<Role>, MembershipStatus)> {
        self.memberships
            .iter()
            .filter(|((_, p), _)| p == principal)
            .map(|((space, _), m)| (space.clone(), m.role, m.status))
            .collect()
    }

    /// Suspend a membership (Z-005). Reversible and non-destructive: the role,
    /// the groups, and the per-member overrides all stay, and every check for
    /// this principal in this space denies with
    /// [`DenyReason::PrincipalSuspended`] until [`AccessEngine::reactivate`].
    ///
    /// The engine is only the door half of the ruling. A suspension command
    /// must also revoke the principal's session/refresh-token families for the
    /// space, close their live subscriptions, refuse their sync proposals,
    /// and — where a suspended device held client-first keys — open a new
    /// epoch for the remaining devices. None of that is reachable from here;
    /// what *is* guaranteed here is that an already-minted token stops opening
    /// the door the moment the status flips.
    ///
    /// No-op for a principal with no membership in the space: there is nothing
    /// to suspend, and minting a suspended-but-absent membership would make
    /// suspension a way to create rows.
    pub fn suspend(&mut self, space: &SpaceId, principal: &PrincipalId) {
        if let Some(m) = self
            .memberships
            .get_mut(&(space.clone(), principal.clone()))
        {
            m.status = MembershipStatus::Suspended;
        }
    }

    /// Lift a suspension. A deliberate, separately-gated command in the
    /// platform (SH3-005 asks for step-up on it): it restores exactly the role
    /// and groups that were suspended, and revives no credential — the tokens
    /// revoked at suspension stay revoked.
    pub fn reactivate(&mut self, space: &SpaceId, principal: &PrincipalId) {
        if let Some(m) = self
            .memberships
            .get_mut(&(space.clone(), principal.clone()))
        {
            m.status = MembershipStatus::Active;
        }
    }

    /// This principal's membership status in the space, if they have one.
    pub fn membership_status(
        &self,
        space: &SpaceId,
        principal: &PrincipalId,
    ) -> Option<MembershipStatus> {
        self.memberships
            .get(&(space.clone(), principal.clone()))
            .map(|m| m.status)
    }

    /// A principal's direct role and groups in the space, if they hold a
    /// membership. A guest has no role.
    pub fn standing(
        &self,
        space: &SpaceId,
        principal: &PrincipalId,
    ) -> Option<(Option<Role>, BTreeSet<GroupId>)> {
        self.memberships
            .get(&(space.clone(), principal.clone()))
            .map(|m| (m.role, m.groups.clone()))
    }

    /// Add `principal` to `group` **in this space**. Requires a membership:
    /// a group cannot smuggle in a principal who is not in the space, which
    /// would make group edits a membership-minting path.
    pub fn add_to_group(&mut self, space: &SpaceId, principal: &PrincipalId, group: &GroupId) {
        // A suspended member keeps their groups; adding one grants nothing
        // while the suspension stands, because status is checked first.
        if let Some(m) = self
            .memberships
            .get_mut(&(space.clone(), principal.clone()))
        {
            m.groups.insert(group.clone());
        }
    }

    /// Install a space's access profile (layer 2).
    pub fn set_profile(&mut self, space: &SpaceId, profile: AccessProfile) {
        self.profiles.insert(space.clone(), profile);
    }

    /// The access profile installed in a space, if any. Read by the op that
    /// shows a tenant its own profile, so the answer is the one the door
    /// decides with.
    pub fn profile(&self, space: &SpaceId) -> Option<&AccessProfile> {
        self.profiles.get(space)
    }

    /// The principals holding a membership in `space`, with the member id
    /// each holds there.
    pub fn members_of(&self, space: &SpaceId) -> Vec<(PrincipalId, String)> {
        self.memberships
            .iter()
            .filter(|((s, _), _)| s == space)
            .map(|((_, p), m)| (p.clone(), m.member_id.clone()))
            .collect()
    }

    /// Register/refresh what an op means (deploy-time event): its manifest
    /// hash, its field set, and its row-scope shape.
    ///
    /// The pin is supplied rather than derived here, because deriving it means
    /// compiling, and this crate is a leaf that never learns what a query is —
    /// see [`crate::pins`]. `ordo_host::deployment_engine` is the one caller
    /// that fills it from a real compiled program.
    pub fn deploy_op(&mut self, op: &str, pin: OpPin) {
        self.deployed.insert(op.to_string(), pin);
    }

    /// What the deployment currently says each op means — the right-hand side
    /// of every comparison, exposed so a consent page can render the diff a
    /// person is being asked to re-approve.
    pub fn deployed(&self) -> &BTreeMap<String, OpPin> {
        &self.deployed
    }

    /// Close `op` to every delegated token (SH-20). A token that acts for
    /// somebody is refused it with [`DenyReason::OpNotDelegable`] even when
    /// its allowlist names it and its delegator could call it. Tokens that
    /// act as themselves are untouched.
    ///
    /// The engine does not decide which ops these are. The deployment does,
    /// from its own default and the app developer's setting.
    pub fn forbid_delegation(&mut self, op: &str) {
        self.not_delegable.insert(op.to_string());
    }

    /// May a delegated token call `op` at all? The question a consent page
    /// asks to say "this agent may not ...".
    pub fn delegable(&self, op: &str) -> bool {
        !self.not_delegable.contains(op)
    }

    /// Pin an op's **intrinsic** step-up minimum: fresh user presence, at the
    /// deployment's window. This is where `space.delete`, full export, custody
    /// conversion, device enrollment/removal, recovery-mode changes, and
    /// authenticator linking are registered — no client can lower it (Z-003).
    /// Break-glass, when built as the grant-gated operation the owner ruled
    /// (corpus-rulings.md, 2026-09-07), would be pinned here too.
    pub fn require_step_up(&mut self, op: &str) {
        self.op_step_up
            .insert(op.to_string(), StepUpRequirement::PRESENCE);
    }

    /// The same, with a window shorter than the deployment's. The effective
    /// window is always the min of this and
    /// [`AccessEngine::step_up_max_age_secs`], so this can only tighten.
    pub fn require_step_up_within(&mut self, op: &str, max_age_secs: u64) {
        self.op_step_up
            .insert(op.to_string(), StepUpRequirement::Presence { max_age_secs });
    }

    /// Register a client's *additional* demand for an op. Folded into the op's
    /// minimum by [`StepUpRequirement::strongest`] — a client can add presence
    /// where the op required none, or shorten the window, and can do nothing
    /// else. There is deliberately no way to register a client *exemption*.
    pub fn client_requires_step_up(
        &mut self,
        client: &ClientId,
        op: &str,
        requirement: StepUpRequirement,
    ) {
        self.client_step_up
            .entry(client.clone())
            .or_default()
            .insert(op.to_string(), requirement);
    }

    /// The requirement actually in force for `op` when called through
    /// `client`: the op's intrinsic minimum, strengthened (never weakened) by
    /// whatever that client declared. A client the deployment does not know,
    /// or a token with no client claim, contributes nothing — which leaves the
    /// op's minimum standing rather than dropping it.
    pub fn step_up_for(&self, op: &str, client: Option<&ClientId>) -> StepUpRequirement {
        let intrinsic = self
            .op_step_up
            .get(op)
            .copied()
            .unwrap_or(StepUpRequirement::None);
        let from_client = client
            .and_then(|c| self.client_step_up.get(c))
            .and_then(|ops| ops.get(op))
            .copied()
            .unwrap_or(StepUpRequirement::None);
        intrinsic.strongest(from_client)
    }

    /// The consent page's pinning step: mint a grant **for one space**,
    /// pinning what each listed op means *right now* — its manifest hash, its
    /// field set, and its row-scope shape, **per op**.
    ///
    /// `None` if any op is not deployed: consent to a nonexistent scope is
    /// unmintable. Two spaces means two grants, consented twice (SH2-2).
    ///
    /// `id` is supplied rather than minted here so this crate stays a leaf
    /// with no RNG, and so a test can name the grant it is about to assert
    /// on. It must be unguessable: it is what a revocation names.
    pub fn grant_for(
        &self,
        id: GrantId,
        space: &SpaceId,
        ops: BTreeSet<String>,
        expires: Timestamp,
    ) -> Option<CapabilityGrant> {
        let mut pins = BTreeMap::new();
        for op in ops {
            let pin = self.deployed.get(&op)?.clone();
            pins.insert(op, pin);
        }
        Some(CapabilityGrant::new(id, space.clone(), pins, expires))
    }

    /// The drift report for a grant against this deployment — the input to an
    /// audit record, and to a consent page rendering what changed.
    pub fn drift(&self, grant: &CapabilityGrant) -> Vec<(String, PinDrift)> {
        grant_drift(grant, &self.deployed)
    }

    /// The one check at the door. Membership status → (role + groups +
    /// overrides → profile) OR capability allowlist → op included? → step-up.
    ///
    /// Status comes **first**, ahead of every grant source (Z-005): a
    /// suspended principal is stopped before any layer has a chance to allow
    /// them, so neither a capability grant nor a group-conferred role can
    /// route around the suspension. Inclusion is checked before step-up so a
    /// denial names the real gap first.
    pub fn check(
        &self,
        claims: &TokenClaims,
        space: &SpaceId,
        op: &str,
        now: Timestamp,
    ) -> Decision {
        // Suspension is an authorization input, not display state. Checked
        // before the token's shape is even consulted: an operator who suspends
        // a compromised member must not have to wait for a token to expire,
        // and a synthetic caller acting as that principal is stopped too.
        if self.membership_status(space, &claims.principal) == Some(MembershipStatus::Suspended) {
            return Decision::deny(op, DenyReason::PrincipalSuspended);
        }
        // Delegation, before the authority layers, because it asks a
        // question that precedes them: *whose* authority is even in play.
        // Everything here is decidable from the token; revocation is the
        // door's registry gate and is not reachable from a leaf crate that
        // does no I/O (`crate::delegation`).
        if let Some(d) = &claims.delegation {
            // Space first, for the reason the capability path checks it
            // first: it is the cheapest check and the isolating one, and a
            // delegation good elsewhere must not be good here (SH-20).
            if d.space != *space {
                return Decision::deny(op, DenyReason::DelegationSpaceMismatch);
            }
            if now >= d.expires {
                return Decision::deny(op, DenyReason::DelegationExpired);
            }
            // Before the delegator's reach: an op closed to delegation is
            // closed whoever the delegator is, and the record should say
            // that rather than blame the person who consented.
            if !self.delegable(op) {
                return Decision::deny(op, DenyReason::OpNotDelegable);
            }
            // "A delegation carries only the delegator's authority." The
            // two consents INTERSECT — they do not add. Checked before the
            // caller's own layers so a denial names the delegation rather
            // than the allowlist, which is the difference between "ask Ada
            // to re-consent" and "ask your admin for the op".
            if !self.membership_reaches(space, &d.delegator, op) {
                return Decision::deny(op, DenyReason::DelegationBeyondDelegator);
            }
        }
        // Capability path — exclusive by token shape; never falls through
        // to roles.
        if let Some(grant) = &claims.capability {
            // Space first (SH2-2): the isolating check is also the cheapest,
            // and nothing downstream can un-deny it.
            if grant.space() != space {
                return Decision::deny(op, DenyReason::GrantSpaceMismatch);
            }
            if now >= grant.expires() {
                return Decision::deny(op, DenyReason::GrantExpired);
            }
            // The pin comparison, per op, over every op the grant lists.
            //
            // **Narrowing is not a denial.** That is the whole of what the
            // combined hash got wrong: it could only answer "same or
            // different", and both of the outcomes it had to separate produce
            // "different". An op that now reaches fewer fields, or is now
            // guarded where it was not, applies silently — and the drift
            // report ([`AccessEngine::drift`]) is what the caller writes the
            // audit note from. A consent screen people are made to click
            // through after every deploy is a consent screen they stop
            // reading, and that is a security property, not a convenience.
            if self.drift(grant).iter().any(|(_, d)| d.is_fatal()) {
                return Decision::deny(op, DenyReason::ManifestChanged);
            }
            if !grant.allows(op) {
                return Decision::deny(op, DenyReason::OpNotInAllowlist);
            }
        } else {
            // Human path: membership role + group memberships → per-space
            // profile → union of allowlists, then per-member overrides.
            let Some(membership) = self
                .memberships
                .get(&(space.clone(), claims.principal.clone()))
            else {
                return Decision::deny(op, DenyReason::NoMembership);
            };
            // A guest has a seat and no role: nothing on this path can
            // allow it, so it is refused before the profile is read.
            let Some(direct) = membership.role else {
                return Decision::deny(op, DenyReason::NoRole);
            };
            let Some(profile) = self.profiles.get(space) else {
                return Decision::deny(op, DenyReason::NoProfileForSpace);
            };
            if !profile.allows_principal(&claims.principal, direct, &membership.groups, op) {
                // The denial names the level the caller actually resolved to,
                // which is the max of their direct role and any role a group
                // confers — otherwise the record blames the wrong grant.
                let role = profile.effective_role(direct, &membership.groups);
                return Decision::deny(op, DenyReason::OpNotInRole { role });
            }
        }
        // Step-up gate: applies to whichever path allowed. The requirement is
        // the op's, strengthened by the caller's client — a client cannot
        // shop its way out of it (Z-003).
        match self.step_up_for(op, claims.client.as_ref()) {
            StepUpRequirement::None => Decision::Allow,
            StepUpRequirement::Presence { max_age_secs } => {
                // The deployment's window is itself a ceiling: a requirement
                // that names no window inherits it, and one that names a
                // shorter window keeps it.
                let window = max_age_secs.min(self.step_up_max_age_secs);
                match self.step_up_decision(claims, op, now, window) {
                    Decision::Allow => Decision::Allow,
                    passkey => self.upstream_decision(claims, space, op, now, window, passkey),
                }
            }
        }
    }

    /// Does this principal's **membership** reach `op` in this space — role,
    /// group and per-member overrides resolved, suspension honoured?
    ///
    /// Deliberately narrower than [`AccessEngine::check`] in two ways, and
    /// both are the point:
    ///
    /// - **It ignores capability tokens**, because a membership is what it
    ///   asks about. The two paths are exclusive by token shape, and this is
    ///   the human one.
    /// - **It ignores step-up**, because step-up asks *"is this caller
    ///   present right now"* and this asks *"does this principal's standing
    ///   in this space include this op"*. A manager who has not touched
    ///   their passkey in an hour has not stopped being a manager.
    ///
    /// It exists because [`AccessEngine::callers_of`] needs exactly this
    /// predicate per principal, and because the surface that reads
    /// `callers_of` needs the same predicate for the *asker* — see
    /// `ordo_host::platform_ops`. One function, so the two can never
    /// disagree about who is included.
    pub fn membership_reaches(&self, space: &SpaceId, principal: &PrincipalId, op: &str) -> bool {
        let Some(m) = self.memberships.get(&(space.clone(), principal.clone())) else {
            return false;
        };
        if m.status == MembershipStatus::Suspended {
            return false;
        }
        // A guest reaches nothing by membership: its authority is a grant.
        let Some(role) = m.role else {
            return false;
        };
        self.profiles
            .get(space)
            .is_some_and(|p| p.allows_principal(principal, role, &m.groups, op))
    }

    /// **Every principal who can call `op` in `space` today, and how** —
    /// `security-logging-design.md` §6 rule 4.
    ///
    /// # Why the token path arrives as an argument
    ///
    /// Rule 4 requires the via to include **token**, and the capability
    /// grants that answer it live in a [`crate::grants::GrantRegistry`],
    /// which is I/O. This crate is a leaf: no RNG, no clock, no driver — the
    /// same discipline that makes `grant_for` take an id rather than mint
    /// one. So the caller performs the read
    /// ([`crate::grants::GrantRegistry::callers_of`]) and hands the records
    /// over; the engine folds them into the same answer under the same
    /// rules. The alternative — this crate opening a connection — would put
    /// a database behind a type whose whole value is that it does not need
    /// one.
    ///
    /// # Every via, not the first one
    ///
    /// A principal reached three ways appears three times. That is not
    /// redundancy: the question behind this op is usually *"how do I stop
    /// them calling it"*, and an answer naming one of three grants would
    /// have a manager revoke it and find the caller still there. Rule 4's
    /// *"a group grant that does not show up there silently breaks the
    /// model's best property"* is the same argument for the group case
    /// specifically.
    ///
    /// # What removes a caller, and what does not
    ///
    /// - A **per-member deny override** removes a principal entirely, with
    ///   no vias at all — it is the one rule that subtracts, and a union
    ///   that forgot it would list callers who are denied at the door.
    /// - **Suspension** removes a principal entirely, *including their
    ///   capability grants*, because [`AccessEngine::check`] tests status
    ///   before it looks at the token. The registry cannot know that, so the
    ///   filtering happens here.
    /// - A **revoked or expired grant** is already absent: the registry's
    ///   own query is live-only. [`crate::grants::GrantRecord::is_live`] is
    ///   applied again here anyway, because "the caller passed me the right
    ///   rows" is not a property this function should assume.
    ///
    /// # The one thing it does not re-check, stated
    ///
    /// A grant's **manifest pin**. [`AccessEngine::check`] refuses a
    /// capability whose combined manifest hash no longer matches what is
    /// deployed; a per-op grant record carries that combined hash but not
    /// the op set it was computed over, so it cannot be recomputed from one
    /// record. A grant whose pin has drifted is therefore listed here and
    /// would be refused at the door — a **false positive**, which is the
    /// safe direction for an audit surface: the manager revokes a token that
    /// had already stopped working, and no live token is hidden. The unsafe
    /// direction — omitting a caller — is the failure rule 4 exists to
    /// prevent, and nothing here can produce it.
    /// # The delegation column, and why it is a **conjunction**
    ///
    /// A delegated agent reaches an op only when *both* consents hold: a
    /// live delegation from somebody who reaches the op, **and** a live
    /// capability grant listing it. So a delegation via is emitted only for
    /// an agent that already appears here via `token` for this op. Emitting
    /// it on the delegation alone would name a caller the door refuses
    /// (`OpNotInAllowlist`), which rule 4 calls *"the same class of wrong
    /// as omitting one it admits"*.
    ///
    /// The agent then appears **twice**, and that is the rule's own
    /// "every via, not the first" applied honestly: revoking the token
    /// stops it, and so does withdrawing the delegation — two different
    /// edits, by two different people, so two rows. An answer with one of
    /// them would send whoever read it to do half the job.
    pub fn callers_of(
        &self,
        space: &SpaceId,
        op: &str,
        grants: &[crate::grants::GrantRecord],
        delegations: &[crate::delegation::DelegationRecord],
        now: Timestamp,
    ) -> Vec<Caller> {
        self.callers_among(space, op, None, grants, delegations, now)
    }

    /// **Every op `principal` can call in `space` today, and how**: the
    /// `std.space.callableBy` question, answered as [`AccessEngine::callers_of`]
    /// answers its own, one op at a time, by the same body
    /// ([`AccessEngine::callers_among`]) cut to one principal. One body, so
    /// "who can call X" and "what can Y call" can never disagree.
    ///
    /// `ops` is the set to ask about: the caller chooses it, because which
    /// ops an asker may see is the asker's own reach, and that is a fact
    /// about the asker, not about `principal`. Answers `(op, via)` pairs in
    /// op order; an op reached three ways is three pairs.
    pub fn callable_by<'o>(
        &self,
        space: &SpaceId,
        principal: &PrincipalId,
        ops: impl IntoIterator<Item = &'o str>,
        grants: &[crate::grants::GrantRecord],
        delegations: &[crate::delegation::DelegationRecord],
        now: Timestamp,
    ) -> Vec<(String, Via)> {
        let mut out = Vec::new();
        for op in ops {
            for c in self.callers_among(space, op, Some(principal), grants, delegations, now) {
                out.push((op.to_string(), c.via));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The ops this deployment has registered, by name.
    pub fn deployed_ops(&self) -> impl Iterator<Item = &str> {
        self.deployed.keys().map(String::as_str)
    }

    /// The body of [`AccessEngine::callers_of`], cut to one principal when
    /// `only` names one. Cutting is a filter on the same three sources and
    /// nothing else, so the two answers are one rule.
    fn callers_among(
        &self,
        space: &SpaceId,
        op: &str,
        only: Option<&PrincipalId>,
        grants: &[crate::grants::GrantRecord],
        delegations: &[crate::delegation::DelegationRecord],
        now: Timestamp,
    ) -> Vec<Caller> {
        let mut out: Vec<Caller> = Vec::new();
        let profile = self.profiles.get(space);
        let seats: Box<dyn Iterator<Item = (&(SpaceId, PrincipalId), &Membership)>> = match only {
            Some(p) => Box::new(
                self.memberships
                    .get_key_value(&(space.clone(), p.clone()))
                    .into_iter(),
            ),
            None => Box::new(self.memberships.iter()),
        };
        let wanted = |p: &PrincipalId| only.is_none_or(|o| o == p);

        for ((s, principal), m) in seats {
            if s != space || m.status == MembershipStatus::Suspended {
                continue;
            }
            let Some(profile) = profile else { continue };
            // A guest is refused on the human path (`NoRole`), so no role,
            // group or override via can name it. Its token via, if it holds
            // a share grant, is added from the registry below.
            let Some(role) = m.role else { continue };
            // Deny-by-override beats everything, so it is checked before any
            // via is collected rather than subtracted afterwards. A
            // subtraction that runs later is a subtraction somebody can add
            // a source below.
            if profile
                .overrides
                .get(principal)
                .is_some_and(|o| o.deny.contains(op))
            {
                continue;
            }
            if profile
                .ops_by_role
                .get(&role)
                .is_some_and(|ops| ops.contains(op))
            {
                out.push(Caller {
                    principal: principal.clone(),
                    via: Via::Role(role),
                });
            }
            for group in &m.groups {
                // A group grants two ways — its own allowlist, and a role it
                // confers whose allowlist has the op — and both are the
                // group. Naming the conferred role in the via is what lets a
                // manager tell "remove the op from the group" from "the
                // group makes them an editor".
                if profile
                    .ops_by_group
                    .get(group)
                    .is_some_and(|ops| ops.contains(op))
                {
                    out.push(Caller {
                        principal: principal.clone(),
                        via: Via::Group {
                            group: group.clone(),
                            conferred: None,
                        },
                    });
                }
                if let Some(role) = profile.role_by_group.get(group) {
                    if profile
                        .ops_by_role
                        .get(role)
                        .is_some_and(|ops| ops.contains(op))
                    {
                        out.push(Caller {
                            principal: principal.clone(),
                            via: Via::Group {
                                group: group.clone(),
                                conferred: Some(*role),
                            },
                        });
                    }
                }
            }
            if profile
                .overrides
                .get(principal)
                .is_some_and(|o| o.allow.contains(op))
            {
                out.push(Caller {
                    principal: principal.clone(),
                    via: Via::Override,
                });
            }
        }

        for record in grants {
            if record.space != *space
                || record.op != op
                || !record.is_live(now)
                || !wanted(&record.principal)
            {
                continue;
            }
            // A suspended principal is denied before their token is even
            // looked at (Z-005). The registry has no notion of suspension,
            // so this is the only place the two facts meet.
            if self.membership_status(space, &record.principal) == Some(MembershipStatus::Suspended)
            {
                continue;
            }
            out.push(Caller {
                principal: record.principal.clone(),
                via: Via::Token {
                    grant: record.id.clone(),
                    client: record.client.clone(),
                },
            });
        }

        // The delegation column. Conjunctive with the token column — see
        // this function's doc — so it is computed after it and reads the
        // answer already collected rather than re-deriving one.
        let by_token: BTreeSet<PrincipalId> = out
            .iter()
            .filter(|c| matches!(c.via, Via::Token { .. }))
            .map(|c| c.principal.clone())
            .collect();
        for d in delegations {
            if d.space != *space
                || !d.is_live(now)
                || !wanted(&d.agent)
                || !by_token.contains(&d.agent)
            {
                continue;
            }
            // The delegator's own reach is the bound the door applies
            // (`DenyReason::DelegationBeyondDelegator`), so the audit
            // surface applies it too — one predicate, so the two can never
            // come to disagree about who is included. It subsumes the
            // delegator's suspension, which `membership_reaches` refuses.
            if !self.membership_reaches(space, &d.delegator, op) {
                continue;
            }
            // …and the *agent's* suspension, which the delegator's reach
            // says nothing about.
            if self.membership_status(space, &d.agent) == Some(MembershipStatus::Suspended) {
                continue;
            }
            out.push(Caller {
                principal: d.agent.clone(),
                via: Via::Delegation {
                    delegation: d.id.clone(),
                    delegator: d.delegator.clone(),
                },
            });
        }

        // Deterministic: the same access state answers the same way twice,
        // which is what makes two readings of this surface comparable.
        out.sort();
        out.dedup();
        out
    }

    /// Step-up by a provider's two-step claim (`crate::upstream`), asked only
    /// when the passkey's presence did not satisfy it. `passkey` is that
    /// denial, which stands unless the claim counts here and is fresh.
    ///
    /// Only a human session can carry the claim: it rides in the session,
    /// and a capability or delegated token is refused before it is read.
    fn upstream_decision(
        &self,
        claims: &TokenClaims,
        space: &SpaceId,
        op: &str,
        now: Timestamp,
        window: u64,
        passkey: Decision,
    ) -> Decision {
        let Some(acceptance) = &self.upstream else {
            return passkey;
        };
        if claims.capability.is_some() || claims.delegation.is_some() {
            return passkey;
        }
        let Some(up) = claims
            .session
            .as_ref()
            .and_then(|s| s.upstream_presence.as_ref())
        else {
            return passkey;
        };
        if !acceptance.accepts(space, op, up.via()) {
            return passkey;
        }
        let window = window.min(acceptance.max_age_secs());
        match now.0.checked_sub(up.at().0) {
            Some(age) if age <= window => Decision::Allow,
            age => Decision::deny(
                op,
                DenyReason::StepUpStale {
                    age_secs: age.unwrap_or(u64::MAX),
                    max_age_secs: window,
                },
            ),
        }
    }

    /// Step-up: a *fresh* presence assertion, not a bit set once at login.
    /// Capability tokens are refused structurally — presence is not delegable,
    /// so step-up ops are human-only whatever a grant claims.
    fn step_up_decision(
        &self,
        claims: &TokenClaims,
        op: &str,
        now: Timestamp,
        max_age_secs: u64,
    ) -> Decision {
        if claims.capability.is_some() {
            return Decision::deny(op, DenyReason::StepUpRequired);
        }
        let Some(at) = claims.user_presence_at else {
            return Decision::deny(op, DenyReason::StepUpRequired);
        };
        // A presence stamped in the future is not fresh, it is a clock lie;
        // saturating the subtraction would silently accept it.
        let age = now.0.checked_sub(at.0);
        match age {
            Some(age) if age <= max_age_secs => Decision::Allow,
            Some(age) => Decision::deny(
                op,
                DenyReason::StepUpStale {
                    age_secs: age,
                    max_age_secs,
                },
            ),
            None => Decision::deny(
                op,
                DenyReason::StepUpStale {
                    age_secs: u64::MAX,
                    max_age_secs,
                },
            ),
        }
    }
}
