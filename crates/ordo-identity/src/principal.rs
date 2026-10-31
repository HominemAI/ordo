//! Principal taxonomy and access-model data (spec §8). A principal is
//! anything whose actions must be attributable and permissible — the unit
//! identity, grants, and the command log hang off.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::pins::OpPin;

/// Seconds since the Unix epoch, UTC. The platform never trusts client
/// clocks; every `Timestamp` here is server-observed. A newtype so that a
/// bare integer can never be passed where an instant is meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub u64);

/// Typed principal id (`usr_…`, `agt_…`, `src_…`) — server-minted, always;
/// determinable on sight per the spec's typed-id rule (§2).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrincipalId(pub String);

impl std::fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Typed space id (`spc_…`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpaceId(pub String);

impl std::fmt::Display for SpaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Typed client id (`cli_…`) — one registered relying party. Access tokens
/// name the authorized party they were issued to, so the door can tell which
/// client is calling and apply that client's *additional* step-up demands
/// (never fewer — see [`crate::access::StepUpRequirement`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientId(pub String);

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A membership's state in one space (`definition-language.md` §3: the
/// platform `members` entity's `status`). **Not display state** — it is an
/// input to the access check (Z-005 / SH3-005): suspension is how an operator
/// stops a compromised member *now*, and a check that cannot see it lets
/// already-minted tokens keep working until they expire.
///
/// Suspension is a command, not a delete: the membership, its groups, and its
/// overrides all survive, so reactivation restores exactly what was there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum MembershipStatus {
    #[default]
    Active,
    Suspended,
}

/// The spec's principal taxonomy, verbatim. Only externally-originating
/// identities (agent, source, client) are declared in IaC; humans arrive via
/// the identity service; routines and workers are platform-born and implicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    Human,
    Routine,
    Worker,
    Agent,
    Source,
    Client,
}

impl PrincipalKind {
    /// Typed-id prefix for this kind — the id names its species on sight.
    pub fn id_prefix(self) -> &'static str {
        match self {
            PrincipalKind::Human => "usr",
            PrincipalKind::Routine => "rtn",
            PrincipalKind::Worker => "wrk",
            PrincipalKind::Agent => "agt",
            PrincipalKind::Source => "src",
            PrincipalKind::Client => "cli",
        }
    }

    /// The inverse of [`PrincipalKind::id_prefix`], for a durable store that
    /// persists the prefix as the kind column rather than inventing a
    /// second vocabulary for the same six values.
    pub fn from_prefix(s: &str) -> Option<PrincipalKind> {
        Some(match s {
            "usr" => PrincipalKind::Human,
            "rtn" => PrincipalKind::Routine,
            "wrk" => PrincipalKind::Worker,
            "agt" => PrincipalKind::Agent,
            "src" => PrincipalKind::Source,
            "cli" => PrincipalKind::Client,
            _ => return None,
        })
    }
}

/// A directory entry. `display` is presentation only — never identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub id: PrincipalId,
    pub kind: PrincipalKind,
    pub display: String,
    pub created: Timestamp,
}

/// Membership roles — layer 1 of the access model, per space, user-managed.
/// The closed set; there are no custom roles in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Owner,
    Manager,
    Editor,
    Viewer,
}

impl Role {
    /// Ordering *level* used when several sources confer a role on the same
    /// principal (a direct membership plus one or more groups). Combination is
    /// **max**: holding `viewer` directly and `manager` through a group makes
    /// the principal a manager for the purpose of naming their level.
    ///
    /// This is a level, **not** a hierarchy: a higher level does not inherit a
    /// lower one's ops. Op resolution stays a union of explicit allowlists
    /// ([`AccessProfile::effective_ops`]).
    pub fn level(self) -> u8 {
        match self {
            Role::Viewer => 0,
            Role::Editor => 1,
            Role::Manager => 2,
            Role::Owner => 3,
        }
    }

    /// The role's wire word — the same spelling the platform's `members`
    /// entity stores and `ordo_host::deployment_engine` reads back, so a
    /// role that travels out of the engine and a role that travels in are
    /// one vocabulary rather than two.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Manager => "manager",
            Role::Editor => "editor",
            Role::Viewer => "viewer",
        }
    }

    /// The higher-level of two roles (the combination rule, in one place).
    pub fn max_level(self, other: Role) -> Role {
        if other.level() > self.level() {
            other
        } else {
            self
        }
    }
}

/// Typed group id (`grp_…`). A group is a named set of principals inside one
/// space; it may be granted ops and may hold a role, exactly like a member
/// (design §6 "Group resolution" [settled]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupId(pub String);

impl std::fmt::Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A per-member exception to what role and groups resolved. Applied **last**
/// and able to subtract: deny-by-override beats allow-by-group, so there is
/// exactly one ordering and it is written here (design §6 rule 2 [settled]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberOverride {
    pub allow: BTreeSet<String>,
    pub deny: BTreeSet<String>,
}

/// Layer 2: the per-space access profile. Runtime data, never syntax: a
/// domain ships a default profile as a template, the space owns it afterward.
///
/// Three grant sources, resolved per space and **non-transitive** (nested
/// groups are out of v1, deliberately — design §6 rule 1 [settled]):
///
/// 1. `ops_by_role` — the caller's role allowlist. Roles carry **no implicit
///    hierarchy**; "owner sees everything" is how templates ship, not an
///    engine rule.
/// 2. `ops_by_group` / `role_by_group` — every group the caller belongs to in
///    *this* space adds its allowlist, and may confer a role. Conferred roles
///    combine with the direct role by **max level** ([`Role::max_level`]).
/// 3. `overrides` — per-member allow/deny, applied last (rule 2 [settled]).
///
/// The effective op set is `(role allowlists) ∪ (group allowlists) ∪
/// (override.allow) − (override.deny)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessProfile {
    pub ops_by_role: BTreeMap<Role, BTreeSet<String>>,
    pub ops_by_group: BTreeMap<GroupId, BTreeSet<String>>,
    pub role_by_group: BTreeMap<GroupId, Role>,
    pub overrides: BTreeMap<PrincipalId, MemberOverride>,
}

impl AccessProfile {
    pub fn new() -> Self {
        AccessProfile::default()
    }

    /// Grant `op` to `role` (builder-style, used by templates and tests).
    pub fn allow(mut self, role: Role, op: &str) -> Self {
        self.ops_by_role
            .entry(role)
            .or_default()
            .insert(op.to_string());
        self
    }

    /// Grant `op` to every member of `group`.
    pub fn allow_group(mut self, group: &GroupId, op: &str) -> Self {
        self.ops_by_group
            .entry(group.clone())
            .or_default()
            .insert(op.to_string());
        self
    }

    /// Let `group` confer `role` on its members (a group is nameable wherever
    /// a principal is). Combines with the direct role by max level.
    pub fn group_holds_role(mut self, group: &GroupId, role: Role) -> Self {
        self.role_by_group.insert(group.clone(), role);
        self
    }

    /// Per-member grant, applied after role and group resolution.
    pub fn override_allow(mut self, principal: &PrincipalId, op: &str) -> Self {
        self.overrides
            .entry(principal.clone())
            .or_default()
            .allow
            .insert(op.to_string());
        self
    }

    /// Per-member refusal. Wins over anything role or group granted.
    pub fn override_deny(mut self, principal: &PrincipalId, op: &str) -> Self {
        self.overrides
            .entry(principal.clone())
            .or_default()
            .deny
            .insert(op.to_string());
        self
    }

    /// Does this profile list `op` for `role`? Absent role = empty allowlist.
    /// The **role layer alone** — group membership and overrides are resolved
    /// by [`AccessProfile::effective_ops`], which is what the door calls.
    pub fn allows(&self, role: Role, op: &str) -> bool {
        self.ops_by_role
            .get(&role)
            .is_some_and(|ops| ops.contains(op))
    }

    /// The principal's level in this space: their direct role combined with
    /// every role their groups confer, by **max**.
    pub fn effective_role(&self, direct: Role, groups: &BTreeSet<GroupId>) -> Role {
        groups
            .iter()
            .filter_map(|g| self.role_by_group.get(g))
            .fold(direct, |acc, conferred| acc.max_level(*conferred))
    }

    /// The principal's effective op set: union of the allowlists of every role
    /// they hold (directly or through a group) and of every group they belong
    /// to, plus per-member allows, minus per-member denies. Overrides are
    /// applied last, and deny wins.
    pub fn effective_ops(
        &self,
        principal: &PrincipalId,
        direct: Role,
        groups: &BTreeSet<GroupId>,
    ) -> BTreeSet<String> {
        let mut ops: BTreeSet<String> = BTreeSet::new();
        for role in std::iter::once(direct).chain(
            groups
                .iter()
                .filter_map(|g| self.role_by_group.get(g))
                .copied(),
        ) {
            if let Some(role_ops) = self.ops_by_role.get(&role) {
                ops.extend(role_ops.iter().cloned());
            }
        }
        for group in groups {
            if let Some(group_ops) = self.ops_by_group.get(group) {
                ops.extend(group_ops.iter().cloned());
            }
        }
        if let Some(over) = self.overrides.get(principal) {
            ops.extend(over.allow.iter().cloned());
            for denied in &over.deny {
                ops.remove(denied);
            }
        }
        ops
    }

    /// The door's question: may this principal call `op` here?
    pub fn allows_principal(
        &self,
        principal: &PrincipalId,
        direct: Role,
        groups: &BTreeSet<GroupId>,
        op: &str,
    ) -> bool {
        // A per-member deny short-circuits: it is the one rule that subtracts,
        // and it must not be re-derivable through any other path.
        if self
            .overrides
            .get(principal)
            .is_some_and(|o| o.deny.contains(op))
        {
            return false;
        }
        self.effective_ops(principal, direct, groups).contains(op)
    }
}

/// A minted grant's identifier.
///
/// Opaque and unguessable by construction — it is minted where randomness
/// lives (the consent page) and passed in, so `ordo-identity` stays a leaf
/// crate with no RNG and the value is a test's to choose. It is not derived
/// from the grant's contents: two consents to the same scope by the same
/// principal are two grants, and a content-derived id would make them one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GrantId(pub String);

/// A recorded delegation's identifier (`dlg_…`).
///
/// Opaque and unguessable for the same reason a [`GrantId`] is: it is what a
/// revocation names, and it travels inside a token. Minted where randomness
/// lives — the consent ceremony — and passed in, so this crate stays a leaf
/// with no RNG.
///
/// It is deliberately **not** a [`GrantId`]. DL §8 makes a delegation
/// *"distinct from the capability token's op allowlist"*, and one id type
/// serving both would let a revocation aimed at a consent to *act as a
/// person* land on a consent to *call some ops* — which is exactly the
/// confusion the two records exist to keep apart.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DelegationId(pub String);

impl std::fmt::Display for DelegationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Opaque scope-manifest hash: **SHA-256** over a domain-separated,
/// length-prefixed encoding of the manifest (SH-27).
///
/// The value gates consent — a grant is dead the moment its pinned hash stops
/// matching the deployment — so a collision is a scope expansion nobody
/// consented to, in the one code path whose inputs a package author controls.
/// A 64-bit fold is not a defensible primitive under that; this type carries
/// the full digest and nothing truncates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ManifestHash(pub [u8; 32]);

impl ManifestHash {
    /// Hash raw manifest bytes under the manifest domain tag.
    pub fn of(bytes: &[u8]) -> ManifestHash {
        let mut hasher = Sha256::new();
        hasher.update(b"ordo.manifest.v1");
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
        ManifestHash(hasher.finalize().into())
    }
}

/// Layer 3: a capability grant — what a synthetic caller (agent, source, API
/// client) may do. Enumerated by a human on the consent page; **never**
/// inherits role defaults.
///
/// - `space`: **mandatory** (SH2-2 / Y-002). A grant authorizes ops in exactly
///   one space. Op names are deployment-wide, so a space-blind grant is a
///   deployment-wide key: anyone holding an agent token could name any space
///   id — and space ids leak through invitations, share links, URLs, and
///   support tickets — and be allowed. A grant spanning two spaces is two
///   grants, consented twice, which is already what the tier-2 bridge says.
/// - `pins`: the explicit op-name allowlist **and** what each of those ops
///   meant at consent time. Not listed = denied, always. The pin is
///   **per op**, never one combined hash over all of them (DL §8, and §5's
///   own reasoning): an unrelated op's change must not invalidate a consent
///   by arithmetic, and a single hash cannot separate narrowing from
///   expansion because narrowing changes the hash too. Each pin carries the
///   field set **and the row-scope shape** — see [`crate::pins`] for why the
///   field set alone was a silent privilege escalation.
/// - `expires`: mandatory — no unexpiring grants exist. The expiry instant
///   itself is already expired (`now >= expires` denies).
///
/// Fields are private and there is exactly one constructor, so a grant
/// **cannot be built without naming its space** — not by struct literal, not
/// by functional update from another grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGrant {
    id: GrantId,
    space: SpaceId,
    pins: BTreeMap<String, OpPin>,
    expires: Timestamp,
}

impl CapabilityGrant {
    /// The only way to make a grant. Ordinarily reached through
    /// [`crate::access::AccessEngine::grant_for`], which reads the pins off
    /// the live deployment rather than taking them on trust.
    pub fn new(
        id: GrantId,
        space: SpaceId,
        pins: BTreeMap<String, OpPin>,
        expires: Timestamp,
    ) -> CapabilityGrant {
        CapabilityGrant {
            id,
            space,
            pins,
            expires,
        }
    }

    /// The identifier a revocation names, and the join key between a token
    /// in the wild and the registry row recording that it was issued.
    ///
    /// A grant needs an identity of its own because the alternatives do not
    /// identify anything: `(principal, client, ops)` collides the moment a
    /// principal consents twice to the same scope — which is the ordinary
    /// case after a re-consent — so revoking "that grant" would revoke both,
    /// including the one just issued to replace it.
    pub fn id(&self) -> &GrantId {
        &self.id
    }

    /// The one space this grant is good for.
    pub fn space(&self) -> &SpaceId {
        &self.space
    }

    /// The allowlist, op-name ordered.
    pub fn ops(&self) -> impl Iterator<Item = &String> + '_ {
        self.pins.keys()
    }

    /// Is this op on the allowlist at all? Separate from the pin comparison
    /// on purpose: "you never consented to this op" and "what you consented
    /// to has changed" are different denials with different remedies.
    pub fn allows(&self, op: &str) -> bool {
        self.pins.contains_key(op)
    }

    /// What each listed op meant when the person said yes.
    pub fn pins(&self) -> &BTreeMap<String, OpPin> {
        &self.pins
    }

    pub fn expires(&self) -> Timestamp {
        self.expires
    }
}

/// The closed set of authentication method families. Every deployment's
/// `auth { methods … }` block resolves each configured method to one of
/// these. `Guest` is the seventh: a guest secret Ordo minted, with no email
/// and no provider (`plan-auth-federated.md` §7A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethodKind {
    Password,
    EmailOtp,
    Passkey,
    Oidc,
    Saml,
    OAuth2,
    Guest,
}

impl AuthMethodKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMethodKind::Password => "password",
            AuthMethodKind::EmailOtp => "email-otp",
            AuthMethodKind::Passkey => "passkey",
            AuthMethodKind::Oidc => "oidc",
            AuthMethodKind::Saml => "saml",
            AuthMethodKind::OAuth2 => "oauth2",
            AuthMethodKind::Guest => "guest",
        }
    }
}
