//! What the stream records: scope, subject, op, kind, decision.
//!
//! Everything here is *content*. The chain arithmetic is [`crate::chain`];
//! storage is [`crate::store`]. This module's job is to make the record
//! say true things, which means three rules it enforces in the type system
//! rather than in review:
//!
//! 1. **A subject is what was proven, never what was claimed.** There is no
//!    way to build an attributed [`Subject`] out of an unverified token —
//!    the only other constructor is [`Subject::Unverified`], which carries
//!    nothing at all.
//! 2. **The decision is recorded, not the wire code.**
//!    [`RecordedDecision`] is a closed set that keeps every distinction the
//!    wire deliberately collapses, and it is built from
//!    [`ordo_identity::DenyReason`] by an exhaustive match, so a new deny
//!    reason is a compile error here rather than a silently lost
//!    distinction.
//! 3. **Caller-chosen text is bounded before it is hashed.** An op name is
//!    attacker-influenced (`/op/{name}` takes any string). §6.1 forbids the
//!    attempted *arguments* for exactly this reason and then requires the
//!    attempted *op*, so the op is admitted through
//!    [`sanitize_op`]: an ordinary op-shaped name is recorded verbatim,
//!    anything else becomes one sentinel. A denied caller cannot write
//!    chosen bytes into a record a space owner reads. The space a
//!    deployment entry is [`StreamEntry::about`] is the same class of
//!    input and is admitted through [`sanitize_space`], for the same
//!    reason and with the same all-or-nothing rule.

use std::collections::BTreeSet;

use ordo_commandlog::{Actor, CorrelationId, SpaceId, Timestamp};
use ordo_identity::DenyReason;

/// Which stream an entry belongs to.
///
/// The routing rule that picks between them is §6.1's load-bearing one and
/// lives in [`crate::recorder`]; this is only the name of the destination.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// A space's own stream. Only reachable for a principal with a
    /// relationship to that space.
    Space(SpaceId),
    /// The deployment stream: everything with no space relationship —
    /// unverified callers, non-members, tokens naming a space they have
    /// nothing to do with — plus the platform's own custody and key-release
    /// events, which belong to the operator and not to any tenant.
    Deployment,
}

/// The reserved key the deployment stream is stored under.
///
/// Not a valid space id (space ids are `spc_…`), so no caller can name it
/// and no space can collide with it.
pub const DEPLOYMENT_KEY: &str = "$deployment";

impl Scope {
    /// The storage key, and the identifier a [`ordo_commandlog::TamperAt`]
    /// reports against.
    pub fn key(&self) -> SpaceId {
        match self {
            Scope::Space(s) => s.clone(),
            Scope::Deployment => SpaceId(DEPLOYMENT_KEY.to_string()),
        }
    }

    pub fn is_deployment(&self) -> bool {
        matches!(self, Scope::Deployment)
    }
}

/// What kind of authority the subject held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubjectKind {
    /// A human or service principal acting through their membership.
    Member,
    /// A capability token — the grant is the whole authority.
    Capability,
    /// The platform itself (a worker, a custody component). Pairs with
    /// [`Actor::System`] / [`Actor::Routine`].
    Platform,
}

impl SubjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SubjectKind::Member => "member",
            SubjectKind::Capability => "capability",
            SubjectKind::Platform => "platform",
        }
    }

    fn tag(self) -> u8 {
        match self {
            SubjectKind::Member => 0,
            SubjectKind::Capability => 1,
            SubjectKind::Platform => 2,
        }
    }

    fn from_tag(t: u8) -> Option<SubjectKind> {
        match t {
            0 => Some(SubjectKind::Member),
            1 => Some(SubjectKind::Capability),
            2 => Some(SubjectKind::Platform),
            _ => None,
        }
    }
}

/// Who the event is about.
///
/// # The unverified case, and why it carries nothing
///
/// A token that did not verify proves no principal. The tempting fields —
/// the subject the token *claimed*, a hash of the bearer string, the
/// client id — are all worse than absence:
///
/// - A claimed subject is an attribution the deployment cannot stand
///   behind. Written into a hash-chained record that a space owner reads as
///   evidence, it is a way for a stranger to make the log say a name.
/// - A token fingerprint is a verifier for a secret. Storing
///   `H(bearer)` in a durable record means anyone who later reads the
///   record can confirm a guessed token, and it is also a stable
///   correlation handle an attacker chooses.
///
/// So the variant has no payload. What survives is the shape of the event
/// — an unverifiable presentation happened, at this instant, against this
/// op — plus the deployment-scope routing, which is where a sweep is
/// visible. "Unverified" is a smaller record than a name, and it is the
/// only true one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A proven identity, rendered in §7's actor taxonomy — the same
    /// strings a command group is attributed with, so the two records join
    /// on attribution without a translation table.
    ///
    /// `token` is set when the proof was a deployment token
    /// (`plan-identity-service.md` 4.1 and 7): its `kind`, `client` and
    /// `sid` sit beside `sub`, so the record says which session and which
    /// client made each call. `None` for every other proof, and for every
    /// entry written before the field existed (see [`crate::chain`]).
    Attributed {
        actor: Actor,
        kind: SubjectKind,
        token: Option<TokenFacts>,
    },
    /// Nothing was proven. See the type doc.
    Unverified,
}

/// What a verified deployment token says about its holder, beside `sub`.
///
/// Read from a token the door verified, never from one it refused: an
/// unverified presentation is [`Subject::Unverified`] and carries nothing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokenFacts {
    /// `person`, `machine` or `breakglass`.
    pub kind: String,
    /// The client the token was issued to (`console`, `cli`, ...).
    pub client: String,
    /// The identity service's session id.
    pub sid: String,
}

/// Ordered by its **canonical rendering**, not by variant shape.
///
/// [`Actor`] is deliberately not `Ord` — nothing in the command chain needs
/// to sort attributions — but the collapse key does, and the rendering is
/// the one form the design already calls canonical. Sorting by it means the
/// order a window's entries close in is the order an operator reads them
/// in, and it is stable across builds because the rendering is frozen.
impl Ord for Subject {
    fn cmp(&self, other: &Subject) -> std::cmp::Ordering {
        (
            subject_tag(self),
            self.render(),
            self.kind_str(),
            self.token(),
        )
            .cmp(&(
                subject_tag(other),
                other.render(),
                other.kind_str(),
                other.token(),
            ))
    }
}

impl PartialOrd for Subject {
    fn partial_cmp(&self, other: &Subject) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Subject {
    /// A verified human or service principal.
    pub fn member(principal: &str) -> Subject {
        Subject::Attributed {
            actor: Actor::User(principal.to_string()),
            kind: SubjectKind::Member,
            token: None,
        }
    }

    /// The holder of a verified deployment token: `sub` as the principal,
    /// with the token's `kind`, `client` and `sid` beside it.
    pub fn deployment(sub: &str, token: TokenFacts) -> Subject {
        Subject::Attributed {
            actor: Actor::User(sub.to_string()),
            kind: SubjectKind::Member,
            token: Some(token),
        }
    }

    /// The deployment token's facts, when the proof was one.
    pub fn token(&self) -> Option<&TokenFacts> {
        match self {
            Subject::Attributed { token, .. } => token.as_ref(),
            Subject::Unverified => None,
        }
    }

    /// A verified capability token's principal.
    pub fn capability(principal: &str) -> Subject {
        Subject::Attributed {
            actor: Actor::User(principal.to_string()),
            kind: SubjectKind::Capability,
            token: None,
        }
    }

    /// The platform acting as itself, by component.
    pub fn platform(component: &str) -> Subject {
        Subject::Attributed {
            actor: Actor::System(component.to_string()),
            kind: SubjectKind::Platform,
            token: None,
        }
    }

    /// The attribution string, or `"unverified"`. Never a claimed name.
    pub fn render(&self) -> String {
        match self {
            Subject::Attributed { actor, .. } => actor.to_string(),
            Subject::Unverified => "unverified".to_string(),
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            Subject::Attributed { kind, .. } => kind.as_str(),
            Subject::Unverified => "unverified",
        }
    }

    /// Does this subject name a principal at all?
    pub fn is_attributed(&self) -> bool {
        matches!(self, Subject::Attributed { .. })
    }
}

/// The **decision**, which is what the record keeps — never the wire code.
///
/// `api-engine-design.md` §4.1 makes the wire uniform on purpose: an op the
/// caller may not call and an op that does not exist are the same 403 with
/// the same body. That uniformity is a property of the *answer*, not of the
/// *record*: the caller must learn nothing, the space owner must learn
/// everything. So every distinction the wire erases is a separate variant
/// here.
///
/// Deliberately payload-free. `OpNotInRole` used to want to carry the
/// resolved role and `StepUpStale` the age; both are recoverable from the
/// directory and the clock, and neither is worth a variable-length field in
/// a frozen hashed encoding. What the doc requires is that the
/// *distinctions* survive, and they do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordedDecision {
    /// The bearer string did not verify at all — signature, expiry, or
    /// shape. No principal was established.
    TokenUnverifiable,
    /// The capability grant names a different space than the door bound.
    GrantSpaceMismatch,
    GrantExpired,
    /// The grant's pinned manifest hash no longer matches the deployment.
    ManifestChanged,
    OpNotInAllowlist,
    NoMembership,
    PrincipalSuspended,
    NoProfileForSpace,
    OpNotInRole,
    StepUpRequired,
    StepUpStale,
    /// The name is not a registered op. On the wire this is byte-identical
    /// to `OpNotInRole` (non-enumerability); in the record it is the
    /// difference between a probe and a permission problem.
    UnregisteredOp,
    /// The door opened and a **row-level** guard refused — the executor's
    /// `access_guard`, never the door's `access_denied`. This is the half
    /// P5 supplies (`engine-plan.md` P5 → F3).
    GuardRejected,
    /// The row was not there. Distinct from `GuardRejected` although both
    /// answer `not_found` on the wire — the whole reason the two surfaces
    /// exist.
    RowAbsent,
    /// The token claimed a delegation issued for a different space.
    DelegationSpaceMismatch,
    /// The claimed delegation is past its expiry.
    DelegationExpired,
    /// The claimed delegation is live and the **delegator** cannot call the
    /// op themselves — the intersection rule (`a delegation carries only
    /// the delegator's authority`).
    ///
    /// The one the space owner most needs separated out: it is the shape a
    /// prompt-injected agent takes when it is talked into reaching past
    /// what its human can do, and folding it into `OpNotInAllowlist` would
    /// bury that under the ordinary "wrong scope" bucket.
    DelegationBeyondDelegator,
    /// The claimed delegation is not live in the registry — withdrawn,
    /// unknown, or the registry could not be reached.
    ///
    /// This is the only recorded decision the **wire** does not erase: it
    /// is answered as `delegation_required` rather than `access_denied`,
    /// because it is decided from the token before any op or space is
    /// consulted and therefore discloses nothing the holder does not
    /// already know. See `ordo_api::ErrorCode::DelegationRequired`.
    DelegationNotLive,
    /// The token names a session that was logged out. The bearer string
    /// itself is perfectly valid (correct signature, still inside its
    /// expiry) and is refused because the deployment was told to stop
    /// honouring it.
    ///
    /// Worth its own variant rather than folding into `TokenUnverifiable`:
    /// to an operator the two are opposite readings of the same wire
    /// answer. One is somebody presenting a string this deployment never
    /// minted; this one is somebody still presenting a token after the
    /// logout, which is either a client that did not discard it or a
    /// credential in the wrong hands.
    SessionRevoked,
    /// The caller holds a seat with no role, a guest, and presented a
    /// session token. A guest reaches only what a share grant lists
    /// (`plan-sharing.md` §3.1). Distinct from `NoMembership`: the seat
    /// exists, and the tenant owner reads that difference.
    NoRole,
    /// The token holds a share grant (`shr_`) that is no longer live:
    /// revoked, expired in the registry, unknown, or the registry could not
    /// be reached. Decided before any op is known, so the wire answer is the
    /// uniform `access_denied` and only this record tells the tenant owner
    /// it was a closed window.
    ShareNotLive,
    /// A tenant requires a second step (`plan-second-factor.md` §5.4) and the
    /// session presented had not taken one. Decided at the door for a person
    /// who signed in without naming the tenant; the answer on the wire is the
    /// uniform `access_denied` and only this record says the person should
    /// sign in again naming the tenant.
    SecondStepRequired,
    /// A delegated token called an op the deployment closes to delegation
    /// (SH-20), whatever the token's allowlist and its delegator's reach.
    /// The wire answers the uniform `access_denied`; this record tells the
    /// tenant owner an agent tried something only a person may do.
    OpNotDelegable,
    /// A console key session (`plan-console.md` 8.2) reached for a space
    /// other than the deployment's own. Its token names no space, so the
    /// door binds it to that one.
    ConsoleSessionOffHome,
    /// The call carried no bearer token and is not one the anonymous door
    /// serves. Refused before `check`, so no principal is named.
    TokenMissing,
    /// A person's own download was asked for and the app developer marked
    /// no op for it: the download is off.
    DownloadOff,
    /// A person's own download was asked for with a token that is not the
    /// person's own (a delegated token, a share guest).
    DownloadNotForCaller,
    /// Too many wrong passwords in a row on one account or one share link:
    /// the try was refused unchecked while its wait ran (owner, 2026-10-25,
    /// the guessing slowdown). A refusal like the others, so a burst is one
    /// collapsed entry carrying its count.
    Throttled,
    /// A deployment token (`plan-identity-service.md` 4.1) did not verify:
    /// its signature, its key (an unknown or retired `kid`), its shape, or
    /// the door holds no keys of the identity service at all
    /// (`ORDO_IDENTITY_URL` unset). No principal was established.
    DeploymentTokenRefused,
    /// A deployment token named another identity service in `iss`.
    DeploymentTokenWrongIssuer,
    /// A deployment token was for another deployment (`aud`).
    DeploymentTokenWrongAudience,
    /// A deployment token's `exp` had passed.
    DeploymentTokenExpired,
    /// A verified deployment token reached for something outside the
    /// deployment realm (`plan-identity-service.md` 4.3): an app op, a
    /// tenant op off the open list, or a machine key in a tenant.
    OutsideDeploymentRealm,
    /// A console key session reached the door while the door verifies the
    /// identity service's tokens (`ORDO_IDENTITY_URL` set): the console
    /// signs people in there, so a console key reaches nothing
    /// (`plan-identity-service.md` 6, G14).
    ConsoleKeyClosed,
}

impl RecordedDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            RecordedDecision::TokenUnverifiable => "TokenUnverifiable",
            RecordedDecision::GrantSpaceMismatch => "GrantSpaceMismatch",
            RecordedDecision::GrantExpired => "GrantExpired",
            RecordedDecision::ManifestChanged => "ManifestChanged",
            RecordedDecision::OpNotInAllowlist => "OpNotInAllowlist",
            RecordedDecision::NoMembership => "NoMembership",
            RecordedDecision::PrincipalSuspended => "PrincipalSuspended",
            RecordedDecision::NoProfileForSpace => "NoProfileForSpace",
            RecordedDecision::OpNotInRole => "OpNotInRole",
            RecordedDecision::StepUpRequired => "StepUpRequired",
            RecordedDecision::StepUpStale => "StepUpStale",
            RecordedDecision::UnregisteredOp => "UnregisteredOp",
            RecordedDecision::GuardRejected => "GuardRejected",
            RecordedDecision::RowAbsent => "RowAbsent",
            RecordedDecision::DelegationSpaceMismatch => "DelegationSpaceMismatch",
            RecordedDecision::DelegationExpired => "DelegationExpired",
            RecordedDecision::DelegationBeyondDelegator => "DelegationBeyondDelegator",
            RecordedDecision::DelegationNotLive => "DelegationNotLive",
            RecordedDecision::SessionRevoked => "SessionRevoked",
            RecordedDecision::NoRole => "NoRole",
            RecordedDecision::ShareNotLive => "ShareNotLive",
            RecordedDecision::SecondStepRequired => "SecondStepRequired",
            RecordedDecision::OpNotDelegable => "OpNotDelegable",
            RecordedDecision::ConsoleSessionOffHome => "ConsoleSessionOffHome",
            RecordedDecision::TokenMissing => "TokenMissing",
            RecordedDecision::DownloadOff => "DownloadOff",
            RecordedDecision::DownloadNotForCaller => "DownloadNotForCaller",
            RecordedDecision::Throttled => "Throttled",
            RecordedDecision::DeploymentTokenRefused => "DeploymentTokenRefused",
            RecordedDecision::DeploymentTokenWrongIssuer => "DeploymentTokenWrongIssuer",
            RecordedDecision::DeploymentTokenWrongAudience => "DeploymentTokenWrongAudience",
            RecordedDecision::DeploymentTokenExpired => "DeploymentTokenExpired",
            RecordedDecision::OutsideDeploymentRealm => "OutsideDeploymentRealm",
            RecordedDecision::ConsoleKeyClosed => "ConsoleKeyClosed",
        }
    }

    pub(crate) fn tag(self) -> u8 {
        match self {
            RecordedDecision::TokenUnverifiable => 0,
            RecordedDecision::GrantSpaceMismatch => 1,
            RecordedDecision::GrantExpired => 2,
            RecordedDecision::ManifestChanged => 3,
            RecordedDecision::OpNotInAllowlist => 4,
            RecordedDecision::NoMembership => 5,
            RecordedDecision::PrincipalSuspended => 6,
            RecordedDecision::NoProfileForSpace => 7,
            RecordedDecision::OpNotInRole => 8,
            RecordedDecision::StepUpRequired => 9,
            RecordedDecision::StepUpStale => 10,
            RecordedDecision::UnregisteredOp => 11,
            RecordedDecision::GuardRejected => 12,
            RecordedDecision::RowAbsent => 13,
            // Appended, never renumbered: the tag is hashed into the
            // security stream's chain, so an existing entry's meaning may
            // not move under it.
            RecordedDecision::DelegationSpaceMismatch => 14,
            RecordedDecision::DelegationExpired => 15,
            RecordedDecision::DelegationBeyondDelegator => 16,
            RecordedDecision::DelegationNotLive => 17,
            RecordedDecision::SessionRevoked => 18,
            RecordedDecision::NoRole => 19,
            RecordedDecision::ShareNotLive => 20,
            // Numbered well clear of 20, which the sharing work is still
            // adding to, so the two branches cannot both claim the next tag.
            RecordedDecision::SecondStepRequired => 30,
            RecordedDecision::OpNotDelegable => 31,
            RecordedDecision::ConsoleSessionOffHome => 32,
            RecordedDecision::TokenMissing => 33,
            RecordedDecision::DownloadOff => 34,
            RecordedDecision::DownloadNotForCaller => 35,
            // Clear of 36 onward, which other work in flight may claim.
            RecordedDecision::Throttled => 40,
            // The identity service's tokens at the door, clear of 40.
            RecordedDecision::DeploymentTokenRefused => 50,
            RecordedDecision::DeploymentTokenWrongIssuer => 51,
            RecordedDecision::DeploymentTokenWrongAudience => 52,
            RecordedDecision::DeploymentTokenExpired => 53,
            RecordedDecision::OutsideDeploymentRealm => 54,
            RecordedDecision::ConsoleKeyClosed => 55,
        }
    }

    pub(crate) fn from_tag(t: u8) -> Option<RecordedDecision> {
        Some(match t {
            0 => RecordedDecision::TokenUnverifiable,
            1 => RecordedDecision::GrantSpaceMismatch,
            2 => RecordedDecision::GrantExpired,
            3 => RecordedDecision::ManifestChanged,
            4 => RecordedDecision::OpNotInAllowlist,
            5 => RecordedDecision::NoMembership,
            6 => RecordedDecision::PrincipalSuspended,
            7 => RecordedDecision::NoProfileForSpace,
            8 => RecordedDecision::OpNotInRole,
            9 => RecordedDecision::StepUpRequired,
            10 => RecordedDecision::StepUpStale,
            11 => RecordedDecision::UnregisteredOp,
            12 => RecordedDecision::GuardRejected,
            13 => RecordedDecision::RowAbsent,
            14 => RecordedDecision::DelegationSpaceMismatch,
            15 => RecordedDecision::DelegationExpired,
            16 => RecordedDecision::DelegationBeyondDelegator,
            17 => RecordedDecision::DelegationNotLive,
            18 => RecordedDecision::SessionRevoked,
            19 => RecordedDecision::NoRole,
            20 => RecordedDecision::ShareNotLive,
            30 => RecordedDecision::SecondStepRequired,
            31 => RecordedDecision::OpNotDelegable,
            32 => RecordedDecision::ConsoleSessionOffHome,
            33 => RecordedDecision::TokenMissing,
            34 => RecordedDecision::DownloadOff,
            35 => RecordedDecision::DownloadNotForCaller,
            40 => RecordedDecision::Throttled,
            50 => RecordedDecision::DeploymentTokenRefused,
            51 => RecordedDecision::DeploymentTokenWrongIssuer,
            52 => RecordedDecision::DeploymentTokenWrongAudience,
            53 => RecordedDecision::DeploymentTokenExpired,
            54 => RecordedDecision::OutsideDeploymentRealm,
            55 => RecordedDecision::ConsoleKeyClosed,
            _ => return None,
        })
    }
}

impl From<&DenyReason> for RecordedDecision {
    /// Exhaustive on purpose. A new [`DenyReason`] added to the door
    /// stops this crate compiling, which is the only way to guarantee the
    /// record never quietly folds a new refusal into an existing bucket.
    fn from(reason: &DenyReason) -> RecordedDecision {
        match reason {
            DenyReason::GrantSpaceMismatch => RecordedDecision::GrantSpaceMismatch,
            DenyReason::GrantExpired => RecordedDecision::GrantExpired,
            DenyReason::ManifestChanged => RecordedDecision::ManifestChanged,
            DenyReason::OpNotInAllowlist => RecordedDecision::OpNotInAllowlist,
            DenyReason::NoMembership => RecordedDecision::NoMembership,
            DenyReason::NoRole => RecordedDecision::NoRole,
            DenyReason::PrincipalSuspended => RecordedDecision::PrincipalSuspended,
            DenyReason::NoProfileForSpace => RecordedDecision::NoProfileForSpace,
            DenyReason::OpNotInRole { .. } => RecordedDecision::OpNotInRole,
            DenyReason::StepUpRequired => RecordedDecision::StepUpRequired,
            DenyReason::StepUpStale { .. } => RecordedDecision::StepUpStale,
            DenyReason::DelegationSpaceMismatch => RecordedDecision::DelegationSpaceMismatch,
            DenyReason::DelegationExpired => RecordedDecision::DelegationExpired,
            DenyReason::DelegationBeyondDelegator => RecordedDecision::DelegationBeyondDelegator,
            DenyReason::DelegationNotLive => RecordedDecision::DelegationNotLive,
            DenyReason::OpNotDelegable => RecordedDecision::OpNotDelegable,
        }
    }
}

/// What happened. The doc's list, exactly: denials at the door,
/// authentication and step-up, key releases, suspensions, custody actions,
/// and the link ceremony — plus the overflow marker the cap emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// One or more refusals collapsed under [`crate::collapse`]. The set is
    /// every distinct decision seen in the window; a single denial is a
    /// one-element set.
    Denied(BTreeSet<RecordedDecision>),
    /// A bearer credential was successfully verified into claims.
    Authenticated,
    /// A presentation failed verification. The subject is
    /// [`Subject::Unverified`] by construction. `reason` is a word from the
    /// producer's own fixed vocabulary, never text the caller supplied, so
    /// the stream can say why while the caller is told one thing.
    AuthenticationFailed {
        reason: String,
    },
    /// A step-up-gated op demanded a fresh presence assertion.
    StepUpDemanded,
    /// A fresh presence assertion satisfied one.
    StepUpSatisfied,
    /// `KeyProvider::release(space, purpose, correlation)` handed out a
    /// lease.
    KeyReleased {
        purpose: String,
    },
    /// A membership was suspended / lifted. Both are commands in the domain
    /// chain too — this is the security record of the same act, and it is
    /// the one a domain revert cannot rewrite.
    MembershipSuspended,
    MembershipReactivated,
    /// Epoch rotation, epoch destruction, custody conversion. Break-glass,
    /// when built as the grant-gated operation the owner ruled
    /// (corpus-rulings.md, 2026-09-07), lands here too.
    CustodyAction {
        action: String,
    },
    /// The dual-proof ceremony, naming both authenticators (§ "Linking an
    /// authenticator"): what was added and what vouched for it.
    AuthenticatorLinked {
        added: String,
        vouched_by: Option<String>,
    },
    AuthenticatorUnlinked {
        removed: String,
    },
    PrincipalOnboarded,
    /// The per-scope per-window cap fired: `further` events were observed
    /// and are not individually recorded. The record degrades before it
    /// grows.
    Suppressed {
        further: u64,
    },
    /// The recorder's outbox was full — durable storage was unreachable for
    /// long enough that `lost` entries could not be held — and they are
    /// gone.
    ///
    /// **Not the same event as [`EventKind::Suppressed`], and the
    /// distinction is the reason this is its own variant.** Suppression is
    /// the record refusing to grow with an attacker's *variety*: it is
    /// working as designed, it happens under load, and the entries it
    /// declines were about to be near-duplicates of ones already in the
    /// window. This is the record admitting a *storage fault*: the entries
    /// were distinct, they were already summarized, and nothing about them
    /// survives. An operator seeing the first should look at the caller;
    /// an operator seeing the second should look at the database. Folding
    /// them into one marker would make the second invisible inside the
    /// first, which is precisely the burst in which it happens.
    ///
    /// It is a **hashed link in the stream**, not a counter beside it, and
    /// that is the whole point: a process that dropped events and then
    /// restarted takes its counters with it, while this is a gap declared
    /// inside the artifact whose integrity a reviewer is checking.
    EventsDropped {
        lost: u64,
    },
    /// A schema migration moved phase: the DDL of an `ordo apply` against
    /// an empty target, or of an `ordo migrate --apply`, was applied or
    /// failed. Deployment-scoped by construction: one set of tables holds
    /// every space, so a schema change is nobody's tenant record
    /// (`plan-schema-evolution.md` §7.1). `migration` is the `$migrations`
    /// row it describes; `phase` is that row's phase.
    SchemaMigration {
        migration: String,
        phase: String,
    },
    /// An act of enrolment (`plan-enrolment.md` §4.4): an identity enrolled,
    /// a member admitted, a ticket issued or redeemed, a credential reset,
    /// a membership changed. `act` is a short fixed word (`ticket-issued`,
    /// `member-admitted`, `credential-reset`, ...) and `detail` is one line
    /// of `key=value` pairs naming who did it, to whom and by which
    /// authority. One variant for the family, so a new act is a word here
    /// and not a change to the chain's encoding.
    Enrolment {
        act: String,
        detail: String,
    },
    /// Every session of a principal was ended at once
    /// (`plan-passkey-recovery.md` §4.3). `reason` is a fixed word:
    /// `logout-all`, `recovery`, `credential-reset`. Deployment-scoped: a
    /// session belongs to the deployment, not to a tenant.
    SessionsEnded {
        reason: String,
    },
    /// An act of credential recovery (`plan-passkey-recovery.md` §8.1): a
    /// case opened, a ticket issued, a recovery completed, cancelled or
    /// contested, presence released. `act` is a short fixed word and
    /// `detail` is one line of `key=value` pairs naming the case, the person,
    /// the path, the authority and the evidence class. One variant for the
    /// family, as [`EventKind::Enrolment`] is one for enrolment, so a new act
    /// is a word here and not a change to the chain's encoding.
    Recovery {
        act: String,
        detail: String,
    },
    /// An act on a second factor (`plan-second-factor.md` §7.2): one enrolled,
    /// one removed, one locked by the throttle, a policy changed, the
    /// second-step requirement met or refused. `act` is a short fixed word and
    /// `detail` one line of `key=value` pairs naming the person, the kind of
    /// factor and who acted. One variant for the family, as
    /// [`EventKind::Recovery`] is one for recovery, so a new act is a word
    /// here and not a change to the chain's encoding. Never a secret and never
    /// a code.
    SecondFactor {
        act: String,
        detail: String,
    },
    /// A sign-in handed from one site to another (`plan-subdomain-signin.md`):
    /// a hand-off code issued by a signed-in page, or redeemed by the page it
    /// was issued for. `act` is `issued` or `redeemed`, and `detail` one line
    /// of `key=value` pairs naming the person, the return origin and the way
    /// in of the session that issued it. Never the code.
    SessionHandoff {
        act: String,
        detail: String,
    },
    /// What an outside provider said about how a person signed in there
    /// (`plan-auth-federated.md` §8.2): its `amr`, `acr` and `auth_time`,
    /// recorded for every sign-in that carried them whether or not they
    /// counted. `act` is `presence-vouched` when the connection's two-step
    /// claim counted as presence, `presence-not-minted` when one was asked
    /// for and did not, and `noted` otherwise. `detail` is one line of
    /// `key=value` pairs naming the person and the connection.
    UpstreamClaim {
        act: String,
        detail: String,
    },
    /// A sealed value failed to authenticate or decrypt while its key was
    /// present: unreadable, possibly tampered (owner, 2026-10-23). Names
    /// where, never the value and never key material. Rate-limited per
    /// entity, field and epoch by the producer, so a corrupted table does
    /// not flood the stream.
    SealedUnreadable {
        entity: String,
        field: String,
        epoch: u8,
    },
    /// An act on a console key (`plan-console.md` 8.2, G14): one added,
    /// revoked, or used to sign in. `act` is a short fixed word and `detail`
    /// one line of `key=value` pairs naming the key, its owner and who
    /// acted. Never key material.
    ConsoleKey {
        act: String,
        detail: String,
    },
    /// What started this deployment (`plan-console.md` 7, G16): the
    /// console's runtime revision, who applied it, the image and a
    /// fingerprint of the settings, as one line of `key=value` pairs.
    /// Never a setting's secret value.
    DeploymentBooted {
        detail: String,
    },
    /// An act on the deployment's program through the door
    /// (`plan-console.md` 7, G12): a program applied (`applied`) or a stored
    /// version made the one that serves (`activated`, a program rollback).
    /// `detail` is one line of `key=value` pairs naming the version, the one
    /// it replaced and who acted. Never the program's source.
    Deploy {
        act: String,
        detail: String,
    },
    /// An act on an app secret (`plan-backend-functions.md` §5): one set
    /// (`set`) or removed (`removed`) by the deployment owner. `detail` is
    /// one line of `key=value` pairs naming the secret, its version and who
    /// acted. Never the value.
    Secret {
        act: String,
        detail: String,
    },
}

impl EventKind {
    /// A single denial.
    pub fn denied(decision: RecordedDecision) -> EventKind {
        EventKind::Denied(BTreeSet::from([decision]))
    }

    /// Only denials collapse. Everything else is a discrete act that
    /// happened once and is recorded once.
    pub fn collapses(&self) -> bool {
        matches!(self, EventKind::Denied(_))
    }

    pub fn label(&self) -> &'static str {
        match self {
            EventKind::Denied(_) => "denied",
            EventKind::Authenticated => "authenticated",
            EventKind::AuthenticationFailed { .. } => "authentication-failed",
            EventKind::StepUpDemanded => "step-up-demanded",
            EventKind::StepUpSatisfied => "step-up-satisfied",
            EventKind::KeyReleased { .. } => "key-released",
            EventKind::MembershipSuspended => "membership-suspended",
            EventKind::MembershipReactivated => "membership-reactivated",
            EventKind::CustodyAction { .. } => "custody-action",
            EventKind::AuthenticatorLinked { .. } => "authenticator-linked",
            EventKind::AuthenticatorUnlinked { .. } => "authenticator-unlinked",
            EventKind::PrincipalOnboarded => "principal-onboarded",
            EventKind::Suppressed { .. } => "suppressed",
            EventKind::EventsDropped { .. } => "events-dropped",
            EventKind::SchemaMigration { .. } => "schema-migration",
            EventKind::Enrolment { .. } => "enrolment",
            EventKind::SessionsEnded { .. } => "sessions-ended",
            EventKind::Recovery { .. } => "recovery",
            EventKind::SecondFactor { .. } => "second-factor",
            EventKind::SessionHandoff { .. } => "session-handoff",
            EventKind::UpstreamClaim { .. } => "upstream-claim",
            EventKind::SealedUnreadable { .. } => "sealed-unreadable",
            EventKind::ConsoleKey { .. } => "console-key",
            EventKind::DeploymentBooted { .. } => "deployment-booted",
            EventKind::Deploy { .. } => "deploy",
            EventKind::Secret { .. } => "secret",
        }
    }

    /// Is this the record talking about itself rather than about a caller?
    ///
    /// The two markers are the record's own disclosures, and the drainer
    /// treats them differently from ordinary entries in exactly one way:
    /// they are never the entries a full outbox discards. Dropping the
    /// declaration that events were dropped is the one loss that cannot be
    /// noticed afterwards.
    pub fn is_marker(&self) -> bool {
        matches!(
            self,
            EventKind::Suppressed { .. } | EventKind::EventsDropped { .. }
        )
    }
}

impl From<&ordo_identity::AuditEvent> for EventKind {
    fn from(e: &ordo_identity::AuditEvent) -> EventKind {
        use ordo_identity::AuditEventKind as K;
        match e.kind {
            K::AuthenticatorLinked => EventKind::AuthenticatorLinked {
                added: e
                    .authenticator
                    .as_ref()
                    .map(|a| a.0.clone())
                    .unwrap_or_default(),
                vouched_by: e.existing_authenticator.as_ref().map(|a| a.0.clone()),
            },
            K::AuthenticatorUnlinked => EventKind::AuthenticatorUnlinked {
                removed: e
                    .authenticator
                    .as_ref()
                    .map(|a| a.0.clone())
                    .unwrap_or_default(),
            },
            K::PrincipalOnboarded => EventKind::PrincipalOnboarded,
        }
    }
}

/// The sentinel an op name that is not op-shaped is recorded as.
pub const OP_UNPRINTABLE: &str = "<unprintable>";
/// Longest op name recorded verbatim.
pub const OP_MAX_LEN: usize = 64;

/// Admit a caller-supplied op name into a record others read.
///
/// The rule is all-or-nothing rather than a filter: a name made entirely of
/// `[A-Za-z0-9._-]` and no longer than [`OP_MAX_LEN`] is recorded exactly
/// as presented, and anything else becomes [`OP_UNPRINTABLE`]. A *partially*
/// sanitized string is the bad option — it looks like the caller's input, so
/// a reader trusts it, while the stripping is precisely what lets a hostile
/// name masquerade as a legitimate one.
///
/// The distinction "was it a registered op at all" is not this function's
/// job and is not lost by it: it is carried by
/// [`RecordedDecision::UnregisteredOp`].
pub fn sanitize_op(raw: &str) -> String {
    let ok = !raw.is_empty()
        && raw.len() <= OP_MAX_LEN
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        raw.to_string()
    } else {
        OP_UNPRINTABLE.to_string()
    }
}

/// The sentinel a space id that is not space-id-shaped is recorded as.
pub const SPACE_UNPRINTABLE: &str = "<unprintable>";
/// Longest space id recorded verbatim, the same bound `bind_space` applies
/// at the HTTP door (`ordo-api::router`'s `MAX_SPACE_LEN`), so an ordinary
/// space id is never rewritten by the one artifact that only ever repeats a
/// bound the door already enforced.
pub const SPACE_MAX_LEN: usize = 128;

/// Admit a caller-supplied space id into a record others read.
///
/// `about` names the space a stranger probed (§1.2), and the value is
/// exactly the class of input [`sanitize_op`] exists for: `bind_space`
/// bounds an ordinary space id at the HTTP door (non-empty, at most
/// [`SPACE_MAX_LEN`] bytes, `[A-Za-z0-9_-]`), but there is no space-id
/// validator anywhere in `ordo-identity` or this crate for the leaf crate
/// to lean on. This crate does not trust its callers with the op name and
/// must not start trusting them with the space name, so the space is
/// admitted the same way: all-or-nothing rather than a filter. A *partially*
/// sanitized string is the bad option: it looks like the caller's input,
/// so a reader trusts it, while the stripping is precisely what lets a
/// hostile name masquerade as a legitimate one.
///
/// Deliberately not `sanitize_op`'s alphabet: a space id has no `.` in it
/// (`bind_space` never admits one), so a name carrying one is already
/// off-shape rather than merely long, and the sentinel says so instead of
/// passing it through.
pub fn sanitize_space(raw: &str) -> String {
    let ok = !raw.is_empty()
        && raw.len() <= SPACE_MAX_LEN
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
    if ok {
        raw.to_string()
    } else {
        SPACE_UNPRINTABLE.to_string()
    }
}

/// How many distinct correlation ids a collapsed entry keeps.
///
/// Bounded on purpose. The metric answers "how many"; the stream answers
/// "which requests, roughly" — enough handles to pull the matching command
/// groups and key leases out of the other two artifacts without letting one
/// entry grow with the size of the burst it is summarizing.
pub const CORRELATION_SAMPLE: usize = 4;

/// One hashed unit of the stream.
///
/// A single event is `count == 1` with `first_at == last_at`. A collapsed
/// window is the same shape with a larger count — there is deliberately no
/// separate "summary" type, because a reader that has to branch on which
/// kind of entry it is holding is a reader that will one day forget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEntry {
    pub scope: Scope,
    /// The space this entry is **about**, when it is not the entry's home.
    ///
    /// `Some` only when `scope` is [`Scope::Deployment`]: a space-scope
    /// entry already IS the space, and duplicating it into a second field
    /// would be one fact with two spellings that can drift apart
    /// (`plan-opslog.md` §1.2). Every producer that can name a target goes
    /// through [`crate::recorder::Recorder::observe`], which is the one
    /// place that decides whether the space it was handed is a *home* or an
    /// *object*; nothing else sets this field. See [`sanitize_space`] for
    /// how a caller-chosen value is admitted, and [`crate::chain`] for how
    /// it is encoded so no entry that predates this field moves.
    pub about: Option<SpaceId>,
    pub subject: Subject,
    /// Already through [`sanitize_op`]. Empty for events that are not about
    /// an op (a key release, a suspension).
    pub op: String,
    pub kind: EventKind,
    pub first_at: Timestamp,
    pub last_at: Timestamp,
    /// How many observations this entry stands for. Inside the hashed body,
    /// fixed at window close — never incremented after hashing, which is
    /// the property §6.1 says a hash chain cannot express and this design
    /// therefore never asks it to.
    pub count: u64,
    /// A bounded sample of the correlation ids that produced it. See
    /// [`CORRELATION_SAMPLE`].
    pub correlations: Vec<CorrelationId>,
}

impl StreamEntry {
    /// A one-shot entry: one observation, recorded as itself.
    pub fn single(
        scope: Scope,
        subject: Subject,
        op: &str,
        kind: EventKind,
        at: Timestamp,
        correlation: CorrelationId,
    ) -> StreamEntry {
        StreamEntry {
            scope,
            about: None,
            subject,
            op: sanitize_op(op),
            kind,
            first_at: at,
            last_at: at,
            count: 1,
            correlations: vec![correlation],
        }
    }

    /// Operator-facing one-liner. This is what replaces the host's old
    /// `Mutex<Vec<String>>` audit buffer: the same prose, rendered *from*
    /// the record rather than in place of it.
    pub fn render(&self) -> String {
        let where_ = match (&self.scope, &self.about) {
            (Scope::Space(s), _) => format!("space `{s}`"),
            (Scope::Deployment, Some(about)) => format!("deployment (about space `{about}`)"),
            (Scope::Deployment, None) => "deployment".to_string(),
        };
        let what = match &self.kind {
            EventKind::Denied(decisions) => {
                let names: Vec<&str> = decisions.iter().map(|d| d.as_str()).collect();
                format!("deny [{}]", names.join(","))
            }
            EventKind::KeyReleased { purpose } => format!("key-released purpose={purpose}"),
            EventKind::CustodyAction { action } => format!("custody {action}"),
            EventKind::AuthenticationFailed { reason } => {
                format!("authentication-failed reason={reason}")
            }
            EventKind::AuthenticatorLinked { added, vouched_by } => format!(
                "authenticator-linked {added} vouched-by={}",
                vouched_by.as_deref().unwrap_or("bootstrap")
            ),
            EventKind::AuthenticatorUnlinked { removed } => {
                format!("authenticator-unlinked {removed}")
            }
            EventKind::Suppressed { further } => format!("suppressed {further} further"),
            EventKind::EventsDropped { lost } => {
                format!("events-dropped {lost} (durable store unreachable)")
            }
            EventKind::SchemaMigration { migration, phase } => {
                format!("schema-migration {migration} {phase}")
            }
            EventKind::SessionsEnded { reason } => format!("sessions-ended reason={reason}"),
            EventKind::Recovery { act, detail } => format!("recovery {act} {detail}"),
            EventKind::SecondFactor { act, detail } => format!("second-factor {act} {detail}"),
            EventKind::SessionHandoff { act, detail } => {
                format!("session-handoff {act} {detail}")
            }
            EventKind::SealedUnreadable {
                entity,
                field,
                epoch,
            } => format!("sealed-unreadable {entity}.{field} epoch={epoch}"),
            EventKind::ConsoleKey { act, detail } => format!("console-key {act} {detail}"),
            EventKind::DeploymentBooted { detail } => format!("deployment-booted {detail}"),
            EventKind::Deploy { act, detail } => format!("deploy {act} {detail}"),
            EventKind::Secret { act, detail } => format!("secret {act} {detail}"),
            EventKind::UpstreamClaim { act, detail } => {
                format!("upstream-claim {act} {detail}")
            }
            other => other.label().to_string(),
        };
        let op = if self.op.is_empty() {
            String::new()
        } else {
            format!(" on `{}`", self.op)
        };
        format!(
            "{what} {}{op} in {where_} ×{} [{}..{}]",
            self.subject.render(),
            self.count,
            self.first_at.0,
            self.last_at.0
        )
    }
}

pub(crate) fn subject_tag(s: &Subject) -> u8 {
    match s {
        Subject::Unverified => 0,
        Subject::Attributed { token: None, .. } => 1,
        Subject::Attributed { token: Some(_), .. } => SUBJECT_WITH_TOKEN,
    }
}

/// The subject tag of an attributed subject that carries [`TokenFacts`].
/// Added after 0 and 1, so every entry written before it keeps its bytes.
pub(crate) const SUBJECT_WITH_TOKEN: u8 = 2;

pub(crate) fn subject_kind_tag(k: SubjectKind) -> u8 {
    k.tag()
}

pub(crate) fn subject_kind_from_tag(t: u8) -> Option<SubjectKind> {
    SubjectKind::from_tag(t)
}
