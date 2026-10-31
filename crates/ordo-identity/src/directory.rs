//! The principal directory and account linking: one human principal, many
//! linked authenticators. Link/unlink/onboard are audited — each returns an
//! [`AuditEvent`] the platform records as a command through the gate.
//!
//! # Why linking is a ceremony and not a call (Z-006 / SH3-006)
//!
//! Unlinking was already protected — the last authenticator cannot be removed
//! — but linking was reachable from any ordinary authenticated session. That
//! asymmetry is the whole attack: steal a short-lived bearer token, or pick up
//! an unlocked mirrored device, and attach your own passkey before the session
//! expires. The theft was transient; the credential is not. When the stolen
//! token dies the attacker still signs in, can enroll a client-first device,
//! and receives every future key wrap.
//!
//! So a link now requires a [`LinkCeremony`]: a **fresh** user-presence
//! assertion (the same freshness window step-up runs on, not a bit set once at
//! login) naming an **already linked** authenticator of the same principal.
//! Both proofs travel together, and the resulting [`AuditEvent`] names both
//! authenticators — the one that vouched and the one that was added — because
//! "a new factor appeared" is only actionable if you can see what authorized
//! it. Capability tokens cannot build a ceremony at all: presence is not
//! delegable, so the refusal is structural rather than a check this path could
//! forget.
//!
//! What is deliberately **not** here, and belongs to the platform command that
//! wraps this: out-of-band notification to every existing authenticator,
//! refresh-token family rotation, and the immediate-rollback window. Recovery
//! when the old authenticator is genuinely gone is a separate, delayed and
//! notified path — never this one, which by construction requires a factor the
//! principal still holds. That path is [`Directory::recover`], below, and it
//! shares no function and no argument type with [`Directory::link`].

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use crate::access::{TokenClaims, DEFAULT_STEP_UP_MAX_AGE_SECS};
use crate::principal::{Principal, PrincipalId, PrincipalKind, Timestamp};
use crate::provider::{AuthenticatedSubject, AuthenticatorId};

/// First-login provisioning policy, per deployment (design §3):
/// invite-only is the default posture; auto-onboard mints a human principal
/// on first completed login from an unknown authenticator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provisioning {
    InviteOnly,
    AutoOnboard,
}

/// Why an unknown authenticator may be minted a principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Onboarding {
    /// Only when the deployment's [`Provisioning`] says so.
    ByPolicy,
    /// The caller spent a valid invitation for this person, so the person is
    /// minted whatever the provisioning. The caller checks and spends it: the
    /// directory knows nothing of tickets.
    Invited,
}

/// What happened to the directory — returned to the caller, who records it
/// through the gate. The directory itself never writes the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditEventKind {
    AuthenticatorLinked,
    AuthenticatorUnlinked,
    PrincipalOnboarded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub kind: AuditEventKind,
    pub principal: PrincipalId,
    /// The authenticator this event is *about* — the one linked, unlinked, or
    /// (for an onboarding) none.
    pub authenticator: Option<AuthenticatorId>,
    /// On [`AuditEventKind::AuthenticatorLinked`]: the already-linked
    /// authenticator whose fresh presence authorized the new one (Z-006).
    /// `None` only on the first-login bootstrap, where the principal had no
    /// authenticator to vouch and the login ceremony itself is the proof.
    ///
    /// It is a separate field rather than prose in a message because the
    /// question an operator asks after a takeover — *what authorized this
    /// factor?* — has to be answerable by reading the log, not by inference.
    pub existing_authenticator: Option<AuthenticatorId>,
    pub at: Timestamp,
}

/// Directory failures — loud and specific.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DirectoryError {
    UnknownPrincipal(PrincipalId),
    /// The authenticator is already linked — to `to`. Re-linking is never
    /// silent, even to the same principal.
    AlreadyLinked {
        authenticator: AuthenticatorId,
        to: PrincipalId,
    },
    NotLinked(AuthenticatorId),
    /// Unlinking a human's last authenticator is refused: lockout is not an
    /// operation. Account closure is a different, deliberate command.
    LastAuthenticator {
        principal: PrincipalId,
    },
    /// Invite-only deployment and this authenticator has no link.
    NotInvited(AuthenticatorId),
    /// A link was attempted from a session carrying no user-presence
    /// assertion at all — an ordinary authenticated session, which is exactly
    /// what a stolen bearer token is (Z-006).
    StepUpRequired {
        principal: PrincipalId,
    },
    /// Presence was asserted, but too long ago. A token minted with presence
    /// hours earlier gated the *login*, not this ceremony.
    StepUpStale {
        age_secs: u64,
        max_age_secs: u64,
    },
    /// A capability token attempted a link. Presence is not delegable, so an
    /// agent/source/API-client token can never mint a human credential — the
    /// refusal is structural: no [`LinkCeremony`] can be built from one.
    CapabilityCannotLink {
        principal: PrincipalId,
    },
    /// The ceremony vouches for a different principal than the one being
    /// linked. Both proofs must bind to the same human, or a fresh presence on
    /// one account authorizes a factor on another.
    CeremonyPrincipalMismatch {
        ceremony: PrincipalId,
        requested: PrincipalId,
    },
    /// The authenticator offered as the existing factor is not linked to this
    /// principal, so nothing the principal already holds vouched for the new
    /// factor.
    ExistingAuthenticatorNotLinked {
        authenticator: AuthenticatorId,
        principal: PrincipalId,
    },
    /// The authenticator belongs to a recovery that retired it. It may
    /// contest that recovery ([`Directory::contest`]) until `contest_until`
    /// and do nothing else: not sign in, not step up, not vouch.
    AuthenticatorRetired {
        authenticator: AuthenticatorId,
        case: String,
        contest_until: u64,
    },
    /// A contest was made by an authenticator no recovery has retired, or
    /// whose contest window has closed.
    NotRetired(AuthenticatorId),
    /// A recovery was asked for something it cannot do. The text is a fixed
    /// word, never something the caller supplied.
    RecoveryRefused(&'static str),
    /// A claim (`plan-auth-federated.md` step 7) was asked for something it
    /// cannot do: its window closed. The text is a fixed word, never something the caller
    /// supplied.
    ClaimRefused(&'static str),
    /// The person deleted this account (`POST /auth/account/delete`). It is
    /// kept, marked, and never signs in again: not by any authenticator
    /// linked to it, not by a recovery, not by a step-up. Its sign-in
    /// identifiers stay linked to it, so none of them can be used again
    /// until the app developer's removal setting removes the account.
    Deleted(PrincipalId),
    /// The durable store could not answer. Distinct from every refusal
    /// above on purpose, the same way [`crate::credentials::CredentialStoreError`]
    /// is distinct from a wrong password: whether the directory's table is
    /// reachable does not depend on which principal or authenticator was
    /// asked about, so this must never be folded into an ordinary refusal.
    StoreUnavailable(DirectoryStoreError),
}

impl From<DirectoryStoreError> for DirectoryError {
    fn from(e: DirectoryStoreError) -> DirectoryError {
        DirectoryError::StoreUnavailable(e)
    }
}

impl std::fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirectoryError::UnknownPrincipal(p) => write!(f, "no principal `{p}`"),
            DirectoryError::AlreadyLinked { authenticator, to } => {
                write!(
                    f,
                    "authenticator `{authenticator}` is already linked to `{to}`"
                )
            }
            DirectoryError::NotLinked(a) => write!(f, "authenticator `{a}` is not linked"),
            DirectoryError::LastAuthenticator { principal } => {
                write!(
                    f,
                    "refusing to unlink the last authenticator of `{principal}`"
                )
            }
            DirectoryError::NotInvited(a) => {
                write!(
                    f,
                    "authenticator `{a}` has no account here (invite-only deployment)"
                )
            }
            DirectoryError::StepUpRequired { principal } => write!(
                f,
                "linking an authenticator to `{principal}` needs a fresh user-presence \
                 assertion from an authenticator already linked to it"
            ),
            DirectoryError::StepUpStale {
                age_secs,
                max_age_secs,
            } => write!(
                f,
                "the presence assertion behind this link is {age_secs}s old — the window is \
                 {max_age_secs}s"
            ),
            DirectoryError::CapabilityCannotLink { principal } => write!(
                f,
                "a capability token cannot link an authenticator to `{principal}` — user \
                 presence is not delegable"
            ),
            DirectoryError::CeremonyPrincipalMismatch {
                ceremony,
                requested,
            } => write!(
                f,
                "the link ceremony proves presence for `{ceremony}` but the link names \
                 `{requested}`"
            ),
            DirectoryError::ExistingAuthenticatorNotLinked {
                authenticator,
                principal,
            } => write!(
                f,
                "authenticator `{authenticator}` is not linked to `{principal}`, so it cannot \
                 vouch for a new one"
            ),
            DirectoryError::AuthenticatorRetired {
                authenticator,
                case,
                contest_until,
            } => write!(
                f,
                "authenticator `{authenticator}` was retired by recovery `{case}`; it may \
                 contest it until {contest_until} and do nothing else"
            ),
            DirectoryError::NotRetired(a) => {
                write!(f, "authenticator `{a}` has no recovery to contest")
            }
            DirectoryError::RecoveryRefused(why) => write!(f, "recovery refused: {why}"),
            DirectoryError::ClaimRefused(why) => write!(f, "claim refused: {why}"),
            DirectoryError::Deleted(p) => write!(f, "the account `{p}` was deleted"),
            DirectoryError::StoreUnavailable(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DirectoryError {}

/// A step-up proof authorizing one link (Z-006).
///
/// The fields are private and [`LinkCeremony::from_claims`] is the **only**
/// constructor, so a ceremony cannot be forged by struct literal, cannot be
/// built from a capability token, and cannot be built from a session that
/// asserted no presence. Freshness is not decided here — it is evaluated at
/// link time against the directory's window, so a ceremony value cannot be
/// stashed and reused later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCeremony {
    principal: PrincipalId,
    existing: AuthenticatorId,
    presence_at: Timestamp,
}

impl LinkCeremony {
    /// Build the proof from a session's claims plus the already-linked
    /// authenticator that is vouching.
    ///
    /// Refuses, in order: capability tokens (presence is not delegable) and
    /// sessions with no presence assertion at all. What survives is a human
    /// session that completed a presence ceremony at a known instant, bound to
    /// one principal and one existing factor.
    pub fn from_claims(
        claims: &TokenClaims,
        existing: &AuthenticatorId,
    ) -> Result<LinkCeremony, DirectoryError> {
        if claims.capability.is_some() {
            return Err(DirectoryError::CapabilityCannotLink {
                principal: claims.principal.clone(),
            });
        }
        let Some(presence_at) = claims.user_presence_at else {
            return Err(DirectoryError::StepUpRequired {
                principal: claims.principal.clone(),
            });
        };
        Ok(LinkCeremony {
            principal: claims.principal.clone(),
            existing: existing.clone(),
            presence_at,
        })
    }

    pub fn principal(&self) -> &PrincipalId {
        &self.principal
    }

    pub fn existing_authenticator(&self) -> &AuthenticatorId {
        &self.existing
    }

    pub fn presence_at(&self) -> Timestamp {
        self.presence_at
    }
}

/// A link a recovery retired. It is kept, not deleted, for the contest
/// window: the person whose codes were stolen signs in with the passkey they
/// still hold and cancels the recovery with it. It can do that and nothing
/// else. At `until` it is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retired {
    /// The recovery case that retired it.
    pub case: String,
    /// The end of the contest window, in seconds.
    pub until: u64,
}

/// What the directory keeps about one link beyond whose it is.
///
/// The default is an ordinary link: no quarantine, no recovery behind it,
/// not retired. Every link written before recovery existed reads as this.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinkMeta {
    /// Presence from this authenticator counts only from this instant
    /// (`plan-passkey-recovery.md` §7). Zero for an ordinary link.
    pub presence_not_before: u64,
    /// The recovery that added this link, if one did.
    pub recovered_case: Option<String>,
    /// Set when a recovery retired this link.
    pub retired: Option<Retired>,
    /// When the link was made, in seconds. Zero for a store that does not
    /// keep it, which reads as "before any recovery case".
    pub linked_at: u64,
}

/// What a completed recovery is allowed to do, and nothing else.
///
/// **The fields are private and [`RecoveryTicket::new`] is the only
/// constructor**, the way [`LinkCeremony`] guards a link. The two are
/// different types on purpose: [`Directory::link`] takes a ceremony and
/// [`Directory::recover`] takes a ticket, and neither accepts the other's
/// argument. A recovery never needs presence from an authenticator the
/// person holds, and a link never needs a recovery case, so there is no
/// value that could satisfy both and no code path they share.
///
/// Built by the host from a recovery ticket it has redeemed against a case
/// that had matured. Redeeming it is the only way the directory is asked to
/// recover, and what it yields is a **passkey registration**, never a
/// password: there is no purpose a recovery ticket carries that accepts one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryTicket {
    principal: PrincipalId,
    case: String,
    quarantine_secs: u64,
    contest_secs: u64,
}

impl RecoveryTicket {
    /// `quarantine_secs` is how long the new passkey may not satisfy
    /// step-up. `contest_secs` is how long a retired authenticator may
    /// contest. The host takes both from the recovery policy.
    pub fn new(
        principal: PrincipalId,
        case: String,
        quarantine_secs: u64,
        contest_secs: u64,
    ) -> RecoveryTicket {
        RecoveryTicket {
            principal,
            case,
            quarantine_secs,
            contest_secs,
        }
    }

    pub fn principal(&self) -> &PrincipalId {
        &self.principal
    }

    pub fn case(&self) -> &str {
        &self.case
    }
}

/// What [`Directory::recover`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    pub principal: PrincipalId,
    pub case: String,
    /// The link the recovery added.
    pub added: AuthenticatorId,
    /// The links it retired, still held until `contest_until`.
    pub retired: Vec<AuthenticatorId>,
    /// From this instant the new passkey satisfies step-up.
    pub presence_from: Timestamp,
    /// Until this instant a retired authenticator may contest.
    pub contest_until: Timestamp,
}

/// What [`Directory::contest`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contested {
    pub principal: PrincipalId,
    pub case: String,
    /// The links the recovery added, now removed. The caller removes their
    /// public keys.
    pub removed: Vec<AuthenticatorId>,
    /// The links the recovery retired, now fully usable again.
    pub restored: Vec<AuthenticatorId>,
    /// The links made at or after the case opened, other than the one the
    /// recovery added, now removed. Each is in `removed` too.
    pub enrolled_since: Vec<AuthenticatorId>,
}

/// What a recovery says when it meets a sign-in an outside provider holds.
pub const OUTSIDE_PROVIDER: &str = "this sign-in belongs to an outside provider and is recovered \
                                    at the provider, not here";

/// Does `principal` sign in only through outside providers? See
/// [`Directory::only_outside_provider`]. For a caller that holds a store and
/// no directory.
pub fn only_outside_provider(
    store: &dyn DirectoryStore,
    principal: &PrincipalId,
) -> Result<bool, DirectoryError> {
    let held = store.links_of(principal)?;
    Ok(!held.is_empty() && held.iter().all(|(a, _)| a.is_outside_provider()))
}

/// The directory's durable half: which principals exist and which
/// authenticator belongs to which. [`Directory`] is the business logic —
/// the ceremony rules, the audit events, the provisioning policy — and is
/// the same code whether this trait is backed by a `BTreeMap` in a test or
/// by `ordo_host::principal_directory::PgDirectoryStore` in a deployment,
/// the same split [`crate::credentials::CredentialStore`] makes for a
/// password.
///
/// **`Send + Sync` and interior-mutable**, for the same reason
/// `CredentialStore` is: a directory is shared across requests for the
/// life of the process, so a store it holds is written through `&self` or
/// it could not be written at all.
pub trait DirectoryStore: Send + Sync {
    /// The principal a durable row names, or `None` if nothing was ever
    /// minted under this id.
    fn principal(&self, id: &PrincipalId) -> Result<Option<Principal>, DirectoryStoreError>;

    /// Which principal an authenticator belongs to, if any.
    fn principal_for(
        &self,
        authenticator: &AuthenticatorId,
    ) -> Result<Option<PrincipalId>, DirectoryStoreError>;

    /// How many authenticators are currently linked to `principal`. Backs
    /// both the last-authenticator refusal and the bootstrap-link guard
    /// (`link_first` refuses once any link exists), so it has to be exact,
    /// not cached.
    fn link_count(&self, principal: &PrincipalId) -> Result<usize, DirectoryStoreError>;

    /// Every authenticator linked to `principal`, in a stable order (sorted
    /// by id). Registration needs it for `exclude_credentials`, step-up for
    /// `allow_credentials`, and a person's list of their own factors for
    /// both. It is asked only about a principal the caller has already
    /// proven, never about a stranger's identifier.
    fn authenticators_of(
        &self,
        principal: &PrincipalId,
    ) -> Result<Vec<AuthenticatorId>, DirectoryStoreError>;

    /// What is kept about one link: its quarantine, the recovery behind it,
    /// whether a recovery retired it. `None` when it is not linked.
    ///
    /// The default is the answer for a store that knows nothing of recovery:
    /// every link is ordinary.
    fn link_meta(
        &self,
        authenticator: &AuthenticatorId,
    ) -> Result<Option<LinkMeta>, DirectoryStoreError> {
        Ok(self
            .principal_for(authenticator)?
            .map(|_| LinkMeta::default()))
    }

    /// Every link of `principal` with what is kept about it, sorted by id.
    fn links_of(
        &self,
        principal: &PrincipalId,
    ) -> Result<Vec<(AuthenticatorId, LinkMeta)>, DirectoryStoreError> {
        let mut out = Vec::new();
        for a in self.authenticators_of(principal)? {
            let meta = self.link_meta(&a)?.unwrap_or_default();
            out.push((a, meta));
        }
        Ok(out)
    }

    /// Persist a link a **recovery** adds: like [`DirectoryStore::insert_link`],
    /// and marked with the case and the instant its presence starts to count.
    fn insert_link_marked(
        &self,
        _authenticator: &AuthenticatorId,
        _principal: &Principal,
        _at: Timestamp,
        _presence_not_before: u64,
        _case: &str,
    ) -> Result<(), DirectoryStoreError> {
        Err(DirectoryStoreError(
            "this directory store does not support recovery".into(),
        ))
    }

    /// Retire a link (`Some`), or restore it (`None`).
    fn set_retired(
        &self,
        _authenticator: &AuthenticatorId,
        _retired: Option<&Retired>,
    ) -> Result<(), DirectoryStoreError> {
        Err(DirectoryStoreError(
            "this directory store does not support recovery".into(),
        ))
    }

    /// Every retired link whose window ended at or before `now`.
    fn retired_by(
        &self,
        _now: u64,
    ) -> Result<Vec<(AuthenticatorId, PrincipalId, Retired)>, DirectoryStoreError> {
        Ok(Vec::new())
    }

    /// Mint a fresh principal, persist it, and return its id.
    ///
    /// **The returned id has never named anything before and never will
    /// again name anything else.** An implementation draws the id at
    /// random and commits it with a uniqueness constraint the store
    /// itself enforces (a primary key, not a probability); a draw that
    /// collides with an existing row is retried with a fresh draw rather
    /// than surfaced, bounded so a broken id source refuses loudly instead
    /// of spinning.
    fn create_principal(
        &self,
        kind: PrincipalKind,
        display: &str,
        at: Timestamp,
    ) -> Result<PrincipalId, DirectoryStoreError>;

    /// Persist that `authenticator` now names `principal`, at `at`. The
    /// caller has already checked that `principal` exists and that
    /// `authenticator` is not linked to anything else.
    fn insert_link(
        &self,
        authenticator: &AuthenticatorId,
        principal: &Principal,
        at: Timestamp,
    ) -> Result<(), DirectoryStoreError>;

    /// Remove any link `authenticator` holds. Removing one that is not
    /// there is success, matching [`crate::credentials::CredentialStore::remove`].
    fn remove_link(&self, authenticator: &AuthenticatorId) -> Result<(), DirectoryStoreError>;

    /// Record that `principal` has completed a sign-in, at `at`. Only the
    /// **first** is kept: what it answers is *"has this identity ever been
    /// used"*, which tenant deletion asks to decide whether a person the
    /// tenant created is kept or removed (`plan-enrolment.md` §4.6, E7).
    ///
    /// The default does nothing, which is right for a store that nothing
    /// ever deletes an identity from. A durable store overrides it, and an
    /// error is a refused sign-in, because an unrecorded one could later be
    /// mistaken for an identity nobody used.
    fn note_sign_in(
        &self,
        _principal: &PrincipalId,
        _at: Timestamp,
    ) -> Result<(), DirectoryStoreError> {
        Ok(())
    }

    /// When the person deleted this account, or `None` while it is live.
    ///
    /// The default is the answer for a store nothing ever marks: no account
    /// is deleted.
    fn deleted_at(&self, _principal: &PrincipalId) -> Result<Option<u64>, DirectoryStoreError> {
        Ok(None)
    }

    /// Mark `principal` deleted at `at` (owner ruling 2026-10-06: an account
    /// is marked deleted and kept). Marking one already marked keeps the
    /// first instant. The default refuses: a store that cannot keep the
    /// mark must not report an account deleted.
    fn mark_deleted(
        &self,
        _principal: &PrincipalId,
        _at: Timestamp,
    ) -> Result<(), DirectoryStoreError> {
        Err(DirectoryStoreError(
            "this directory store cannot mark an account deleted".into(),
        ))
    }
}

/// The store could not answer. Never folded into an ordinary
/// [`DirectoryError`]: whether the directory's tables are reachable does
/// not depend on which principal or authenticator was asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryStoreError(pub crate::StoreFault);

impl std::fmt::Display for DirectoryStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "directory store unavailable: {}", self.0)
    }
}

impl DirectoryStoreError {
    /// Busy or unavailable, as the store's client said.
    pub fn class(&self) -> crate::FaultClass {
        self.0.class()
    }
}

impl std::error::Error for DirectoryStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// 128 bits from the OS CSPRNG, hex — the same shape
/// [`crate::session::SessionId::mint`] uses, and for the same reason: no
/// counter, so nothing about a restart or a process's history can produce
/// the same draw twice, and no creation order for an observer to read out
/// of the id itself.
pub fn random_principal_suffix() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b)
        .expect("the operating system CSPRNG is unavailable, refusing to mint a principal id");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// How many draws [`DirectoryStore::create_principal`] retries before
/// refusing. A collision on a 128-bit random draw is not a case any real
/// deployment hits; the bound exists so a broken or exhausted id source
/// fails loudly on the first call rather than spinning forever.
pub const MINT_ATTEMPTS: usize = 8;

/// The process-local store. **For tests, and for nothing a deployment
/// runs** — the same posture [`crate::credentials::MemoryCredentialStore`]
/// takes, and named for the same reason: a deployment wired to it is
/// visible in a diff rather than implied by the absence of configuration.
pub struct MemoryDirectoryStore {
    principals: RwLock<BTreeMap<PrincipalId, Principal>>,
    links: RwLock<BTreeMap<AuthenticatorId, PrincipalId>>,
    meta: RwLock<BTreeMap<AuthenticatorId, LinkMeta>>,
    deleted: RwLock<BTreeMap<PrincipalId, u64>>,
    id_source: Box<dyn Fn() -> String + Send + Sync>,
}

impl std::fmt::Debug for MemoryDirectoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryDirectoryStore")
            .finish_non_exhaustive()
    }
}

impl Default for MemoryDirectoryStore {
    fn default() -> Self {
        MemoryDirectoryStore::new()
    }
}

impl MemoryDirectoryStore {
    pub fn new() -> MemoryDirectoryStore {
        MemoryDirectoryStore {
            principals: RwLock::new(BTreeMap::new()),
            links: RwLock::new(BTreeMap::new()),
            meta: RwLock::new(BTreeMap::new()),
            deleted: RwLock::new(BTreeMap::new()),
            id_source: Box::new(random_principal_suffix),
        }
    }

    /// Override the id draw. **Test-only**: it is what makes "a colliding
    /// draw is retried, not minted" a test that does not depend on
    /// astronomically unlikely luck — the injected source can be made to
    /// repeat a draw on purpose.
    pub fn with_id_source(mut self, source: Box<dyn Fn() -> String + Send + Sync>) -> Self {
        self.id_source = source;
        self
    }
}

impl DirectoryStore for MemoryDirectoryStore {
    fn principal(&self, id: &PrincipalId) -> Result<Option<Principal>, DirectoryStoreError> {
        Ok(self
            .principals
            .read()
            .expect("directory store")
            .get(id)
            .cloned())
    }

    fn principal_for(
        &self,
        authenticator: &AuthenticatorId,
    ) -> Result<Option<PrincipalId>, DirectoryStoreError> {
        Ok(self
            .links
            .read()
            .expect("directory store")
            .get(authenticator)
            .cloned())
    }

    fn link_count(&self, principal: &PrincipalId) -> Result<usize, DirectoryStoreError> {
        Ok(self
            .links
            .read()
            .expect("directory store")
            .values()
            .filter(|p| *p == principal)
            .count())
    }

    fn authenticators_of(
        &self,
        principal: &PrincipalId,
    ) -> Result<Vec<AuthenticatorId>, DirectoryStoreError> {
        // The map is ordered by authenticator id, so the answer is sorted.
        Ok(self
            .links
            .read()
            .expect("directory store")
            .iter()
            .filter(|(_, p)| *p == principal)
            .map(|(a, _)| a.clone())
            .collect())
    }

    fn create_principal(
        &self,
        kind: PrincipalKind,
        display: &str,
        at: Timestamp,
    ) -> Result<PrincipalId, DirectoryStoreError> {
        let mut principals = self.principals.write().expect("directory store");
        for _ in 0..MINT_ATTEMPTS {
            let id = PrincipalId(format!("{}_{}", kind.id_prefix(), (self.id_source)()));
            if principals.contains_key(&id) {
                continue;
            }
            principals.insert(
                id.clone(),
                Principal {
                    id: id.clone(),
                    kind,
                    display: display.to_string(),
                    created: at,
                },
            );
            return Ok(id);
        }
        Err(DirectoryStoreError(
            "could not mint a principal id that does not already exist".into(),
        ))
    }

    fn insert_link(
        &self,
        authenticator: &AuthenticatorId,
        principal: &Principal,
        at: Timestamp,
    ) -> Result<(), DirectoryStoreError> {
        self.links
            .write()
            .expect("directory store")
            .insert(authenticator.clone(), principal.id.clone());
        self.meta.write().expect("directory store").insert(
            authenticator.clone(),
            LinkMeta {
                linked_at: at.0,
                ..LinkMeta::default()
            },
        );
        Ok(())
    }

    fn remove_link(&self, authenticator: &AuthenticatorId) -> Result<(), DirectoryStoreError> {
        self.links
            .write()
            .expect("directory store")
            .remove(authenticator);
        self.meta
            .write()
            .expect("directory store")
            .remove(authenticator);
        Ok(())
    }

    fn link_meta(
        &self,
        authenticator: &AuthenticatorId,
    ) -> Result<Option<LinkMeta>, DirectoryStoreError> {
        if !self
            .links
            .read()
            .expect("directory store")
            .contains_key(authenticator)
        {
            return Ok(None);
        }
        Ok(Some(
            self.meta
                .read()
                .expect("directory store")
                .get(authenticator)
                .cloned()
                .unwrap_or_default(),
        ))
    }

    fn insert_link_marked(
        &self,
        authenticator: &AuthenticatorId,
        principal: &Principal,
        at: Timestamp,
        presence_not_before: u64,
        case: &str,
    ) -> Result<(), DirectoryStoreError> {
        self.insert_link(authenticator, principal, at)?;
        self.meta.write().expect("directory store").insert(
            authenticator.clone(),
            LinkMeta {
                presence_not_before,
                recovered_case: Some(case.to_string()),
                retired: None,
                linked_at: at.0,
            },
        );
        Ok(())
    }

    fn set_retired(
        &self,
        authenticator: &AuthenticatorId,
        retired: Option<&Retired>,
    ) -> Result<(), DirectoryStoreError> {
        let mut meta = self.meta.write().expect("directory store");
        meta.entry(authenticator.clone()).or_default().retired = retired.cloned();
        Ok(())
    }

    fn retired_by(
        &self,
        now: u64,
    ) -> Result<Vec<(AuthenticatorId, PrincipalId, Retired)>, DirectoryStoreError> {
        let links = self.links.read().expect("directory store");
        let meta = self.meta.read().expect("directory store");
        Ok(meta
            .iter()
            .filter_map(|(a, m)| {
                let r = m.retired.as_ref()?;
                if r.until > now {
                    return None;
                }
                Some((a.clone(), links.get(a).cloned()?, r.clone()))
            })
            .collect())
    }

    fn deleted_at(&self, principal: &PrincipalId) -> Result<Option<u64>, DirectoryStoreError> {
        Ok(self
            .deleted
            .read()
            .expect("directory store")
            .get(principal)
            .copied())
    }

    fn mark_deleted(
        &self,
        principal: &PrincipalId,
        at: Timestamp,
    ) -> Result<(), DirectoryStoreError> {
        self.deleted
            .write()
            .expect("directory store")
            .entry(principal.clone())
            .or_insert(at.0);
        Ok(())
    }
}

/// The principal directory and account linking, over a [`DirectoryStore`].
pub struct Directory {
    provisioning: Provisioning,
    store: Arc<dyn DirectoryStore>,
    /// How fresh a [`LinkCeremony`]'s presence assertion must be. Defaults to
    /// the access engine's step-up window — linking *is* a step-up op, so it
    /// rides the same window rather than inventing a second one.
    link_presence_max_age_secs: u64,
}

impl Directory {
    /// A directory over the process-local store. **For tests, and for
    /// nothing a deployment runs** — see [`Directory::with_store`].
    pub fn new(provisioning: Provisioning) -> Self {
        Directory::with_store(provisioning, Arc::new(MemoryDirectoryStore::new()))
    }

    /// A directory over a durable store — what a deployment builds,
    /// matching `LocalPasswordProvider::with_store`'s shape.
    pub fn with_store(provisioning: Provisioning, store: Arc<dyn DirectoryStore>) -> Self {
        Directory {
            provisioning,
            store,
            link_presence_max_age_secs: DEFAULT_STEP_UP_MAX_AGE_SECS,
        }
    }

    /// The deployment's first-login policy. An emailed code is sent to an
    /// address nobody has linked only when a sign-in by it could be onboarded
    /// (`plan-auth-federated.md` §7.5).
    pub fn provisioning(&self) -> Provisioning {
        self.provisioning
    }

    /// Narrow (or widen) the link ceremony's freshness window.
    pub fn with_link_presence_max_age(mut self, secs: u64) -> Self {
        self.link_presence_max_age_secs = secs;
        self
    }

    pub fn link_presence_max_age_secs(&self) -> u64 {
        self.link_presence_max_age_secs
    }

    /// Mint a principal. Server-minted id, typed prefix per kind, drawn and
    /// committed by the store (see [`DirectoryStore::create_principal`]).
    pub fn create_principal(
        &mut self,
        kind: PrincipalKind,
        display: &str,
        at: Timestamp,
    ) -> Result<PrincipalId, DirectoryError> {
        Ok(self.store.create_principal(kind, display, at)?)
    }

    pub fn principal(&self, id: &PrincipalId) -> Result<Option<Principal>, DirectoryError> {
        Ok(self.store.principal(id)?)
    }

    /// Which principal an authenticator belongs to, if any.
    pub fn principal_for(
        &self,
        authenticator: &AuthenticatorId,
    ) -> Result<Option<PrincipalId>, DirectoryError> {
        Ok(self.store.principal_for(authenticator)?)
    }

    /// Record a completed sign-in. See [`DirectoryStore::note_sign_in`].
    pub fn note_sign_in(
        &self,
        principal: &PrincipalId,
        at: Timestamp,
    ) -> Result<(), DirectoryError> {
        Ok(self.store.note_sign_in(principal, at)?)
    }

    /// Mark the account deleted. See [`DirectoryStore::mark_deleted`].
    pub fn mark_deleted(
        &self,
        principal: &PrincipalId,
        at: Timestamp,
    ) -> Result<(), DirectoryError> {
        Ok(self.store.mark_deleted(principal, at)?)
    }

    /// Refuse a principal whose account was deleted, with
    /// [`DirectoryError::Deleted`]. Every path that hands a person a session,
    /// a link or a step-up asks this first.
    pub fn refuse_deleted(&self, principal: &PrincipalId) -> Result<(), DirectoryError> {
        match self.store.deleted_at(principal)? {
            Some(_) => Err(DirectoryError::Deleted(principal.clone())),
            None => Ok(()),
        }
    }

    /// Every authenticator linked to `principal`, sorted by id.
    pub fn authenticators_of(
        &self,
        principal: &PrincipalId,
    ) -> Result<Vec<AuthenticatorId>, DirectoryError> {
        Ok(self.store.authenticators_of(principal)?)
    }

    /// Link an authenticator to a principal — **step-up gated** and audited
    /// (Z-006).
    ///
    /// The ceremony carries a fresh presence assertion made on an
    /// already-linked authenticator of the same principal. Everything it
    /// proves is re-checked here against live state and the current clock, so
    /// a ceremony value cannot outlive its window or survive the vouching
    /// factor being unlinked.
    ///
    /// Order of refusals is deliberate: the ceremony's own bindings first
    /// (principal identity, freshness, the vouching factor), then the
    /// directory's ordinary conflicts. A caller who cannot authorize a link at
    /// all learns nothing about which authenticators exist.
    pub fn link(
        &mut self,
        principal: &PrincipalId,
        authenticator: &AuthenticatorId,
        ceremony: &LinkCeremony,
        at: Timestamp,
    ) -> Result<AuditEvent, DirectoryError> {
        if ceremony.principal != *principal {
            return Err(DirectoryError::CeremonyPrincipalMismatch {
                ceremony: ceremony.principal.clone(),
                requested: principal.clone(),
            });
        }
        // Presence is a moment, not a bit: a future stamp is a clock lie, not
        // freshness, so the subtraction is checked rather than saturated.
        let age = at.0.checked_sub(ceremony.presence_at.0).unwrap_or(u64::MAX);
        if age > self.link_presence_max_age_secs {
            return Err(DirectoryError::StepUpStale {
                age_secs: age,
                max_age_secs: self.link_presence_max_age_secs,
            });
        }
        // The dual proof's other half: the vouching factor must still belong
        // to this principal *now*.
        if self.store.principal_for(&ceremony.existing)?.as_ref() != Some(principal) {
            return Err(DirectoryError::ExistingAuthenticatorNotLinked {
                authenticator: ceremony.existing.clone(),
                principal: principal.clone(),
            });
        }
        // A retired authenticator may contest a recovery and nothing else,
        // vouching included.
        if let Some(meta) = self.store.link_meta(&ceremony.existing)? {
            if let Some(r) = meta.retired {
                return Err(DirectoryError::AuthenticatorRetired {
                    authenticator: ceremony.existing.clone(),
                    case: r.case,
                    contest_until: r.until,
                });
            }
        }
        self.insert_link(
            principal,
            authenticator,
            Some(ceremony.existing.clone()),
            at,
        )
    }

    /// Does `first` vouch for `principal` now? It must be linked to it, not
    /// retired by a recovery, and the account must not be deleted. Asked
    /// when a claim's window opens and again when the claim lands
    /// ([`Directory::claim`]).
    pub fn vouches(
        &self,
        principal: &PrincipalId,
        first: &AuthenticatorId,
    ) -> Result<(), DirectoryError> {
        self.refuse_deleted(principal)?;
        if self.store.principal_for(first)?.as_ref() != Some(principal) {
            return Err(DirectoryError::ExistingAuthenticatorNotLinked {
                authenticator: first.clone(),
                principal: principal.clone(),
            });
        }
        if let Some(r) = self.store.link_meta(first)?.and_then(|m| m.retired) {
            return Err(DirectoryError::AuthenticatorRetired {
                authenticator: first.clone(),
                case: r.case,
                contest_until: r.until,
            });
        }
        Ok(())
    }

    /// **Link by claim** (`plan-auth-federated.md` §2.5, step 7, owner
    /// ruling R12: *"Existing account logs in and claims another
    /// account."*).
    ///
    /// Two fresh ceremonies prove it. `first` is the authenticator the
    /// person signed in with again when the claim's window opened, at
    /// `first_at`; `claimed` is the one whose ceremony they completed now,
    /// at `at`. The caller ran both ceremonies. This re-checks everything
    /// against live state:
    ///
    /// - the window: `at` is no more than `window_secs` after `first_at`,
    ///   and not before it;
    /// - `first` still vouches for `principal` ([`Directory::vouches`]);
    /// - `claimed` may be a new passkey, registered for this account by the
    ///   caller; any sign-in the account holds may vouch for it (owner:
    ///   *"a login method is a login method"*);
    /// - `claimed` is not linked anywhere. Linked to a deleted account is
    ///   [`DirectoryError::Deleted`] naming that account (D5); linked to a
    ///   live one is [`DirectoryError::AlreadyLinked`]. Nothing is moved
    ///   between accounts.
    ///
    /// **Shares nothing with an attribute.** No email, verified or not, and
    /// no name is read. Two ceremonies are the only proof.
    pub fn claim(
        &mut self,
        principal: &PrincipalId,
        first: &AuthenticatorId,
        first_at: Timestamp,
        claimed: &AuthenticatorId,
        at: Timestamp,
        window_secs: u64,
    ) -> Result<AuditEvent, DirectoryError> {
        let age = at.0.checked_sub(first_at.0).unwrap_or(u64::MAX);
        if age > window_secs {
            return Err(DirectoryError::ClaimRefused("window-closed"));
        }
        self.vouches(principal, first)?;
        if let Some(owner) = self.store.principal_for(claimed)? {
            if &owner != principal {
                self.refuse_deleted(&owner)?;
            }
            return Err(DirectoryError::AlreadyLinked {
                authenticator: claimed.clone(),
                to: owner,
            });
        }
        self.insert_link(principal, claimed, Some(first.clone()), at)
    }

    /// The bootstrap link: an invitation being accepted, or an auto-onboard.
    ///
    /// The principal has **no** authenticator yet, so there is no existing
    /// factor that could vouch and no [`LinkCeremony`] is constructible. This
    /// is the one path that bypasses the ceremony, and it is guarded by
    /// exactly that condition: an account with any linked authenticator is
    /// refused with [`DirectoryError::StepUpRequired`] and must go through
    /// [`Directory::link`].
    ///
    /// That guard is what keeps it from being the takeover path again. A
    /// stolen session belongs to an account that logged in, which means that
    /// account already holds a factor, which means this refuses.
    pub fn link_first(
        &mut self,
        principal: &PrincipalId,
        authenticator: &AuthenticatorId,
        at: Timestamp,
    ) -> Result<AuditEvent, DirectoryError> {
        if self.store.link_count(principal)? > 0 {
            return Err(DirectoryError::StepUpRequired {
                principal: principal.clone(),
            });
        }
        self.insert_link(principal, authenticator, None, at)
    }

    /// The shared tail: existence, non-duplication, insert, audit.
    fn insert_link(
        &mut self,
        principal: &PrincipalId,
        authenticator: &AuthenticatorId,
        existing: Option<AuthenticatorId>,
        at: Timestamp,
    ) -> Result<AuditEvent, DirectoryError> {
        let Some(p) = self.store.principal(principal)? else {
            return Err(DirectoryError::UnknownPrincipal(principal.clone()));
        };
        // A deleted account gains no way back in: not a first link by
        // invitation (a ticket that names it), not a second one vouched for
        // by a factor it still holds (audit 2026-10-06).
        self.refuse_deleted(principal)?;
        if let Some(owner) = self.store.principal_for(authenticator)? {
            return Err(DirectoryError::AlreadyLinked {
                authenticator: authenticator.clone(),
                to: owner,
            });
        }
        self.store.insert_link(authenticator, &p, at)?;
        Ok(AuditEvent {
            kind: AuditEventKind::AuthenticatorLinked,
            principal: principal.clone(),
            authenticator: Some(authenticator.clone()),
            existing_authenticator: existing,
            at,
        })
    }

    /// Unlink an authenticator. Audited. Refuses to strand a human with no
    /// way in ([`DirectoryError::LastAuthenticator`]).
    pub fn unlink(
        &mut self,
        authenticator: &AuthenticatorId,
        at: Timestamp,
    ) -> Result<AuditEvent, DirectoryError> {
        let principal = self
            .store
            .principal_for(authenticator)?
            .ok_or_else(|| DirectoryError::NotLinked(authenticator.clone()))?;
        let is_human = self
            .store
            .principal(&principal)?
            .is_some_and(|p| p.kind == PrincipalKind::Human);
        if is_human {
            // Retired links are held for a contest and do not count as a
            // way in: removing the one active link would leave a person
            // who can do nothing but contest.
            let active_others = self
                .store
                .links_of(&principal)?
                .into_iter()
                .filter(|(a, m)| a != authenticator && m.retired.is_none())
                .count();
            if active_others == 0 {
                return Err(DirectoryError::LastAuthenticator { principal });
            }
        }
        self.store.remove_link(authenticator)?;
        Ok(AuditEvent {
            kind: AuditEventKind::AuthenticatorUnlinked,
            principal,
            authenticator: Some(authenticator.clone()),
            existing_authenticator: None,
            at,
        })
    }

    /// What the directory knows about one link: whether a recovery retired
    /// it, and from when its presence counts. `None` when it is not linked.
    pub fn link_meta(
        &self,
        authenticator: &AuthenticatorId,
    ) -> Result<Option<LinkMeta>, DirectoryError> {
        Ok(self.store.link_meta(authenticator)?)
    }

    /// Does this person sign in **only** through outside providers? True when
    /// they hold at least one link and every link is an outside provider's.
    /// Such a person is recovered at their provider, not here.
    pub fn only_outside_provider(&self, principal: &PrincipalId) -> Result<bool, DirectoryError> {
        only_outside_provider(self.store.as_ref(), principal)
    }

    /// Every link of a principal with its meta, sorted by id.
    pub fn links_of(
        &self,
        principal: &PrincipalId,
    ) -> Result<Vec<(AuthenticatorId, LinkMeta)>, DirectoryError> {
        Ok(self.store.links_of(principal)?)
    }

    /// **Recover a principal** who can no longer sign in.
    ///
    /// Adds `new`, marked as a recovery's and quarantined, and retires
    /// `lost` (every link the principal holds when it is empty) for the
    /// contest window. Nothing is deleted: the retired links are kept until
    /// the window closes so the real owner can contest.
    ///
    /// **Shares nothing with [`Directory::link`].** It takes a
    /// [`RecoveryTicket`] and asks nothing of any authenticator the person
    /// holds: not a presence assertion, not a vouching factor, not a fresh
    /// window. That is the point. A person recovering has none of them. It is
    /// also why it is a different function and not a flag on `link`: a flag
    /// is a condition somebody can be argued into setting.
    ///
    /// The three writes are ordered so that a failure between them leaves
    /// nothing the person cannot recover from, and a failure undoes what it
    /// wrote: the new link first, then each retirement, and a refusal after
    /// the first takes the new link back and restores what was retired.
    pub fn recover(
        &mut self,
        ticket: &RecoveryTicket,
        new: &AuthenticatorId,
        lost: &[AuthenticatorId],
        at: Timestamp,
    ) -> Result<Recovered, DirectoryError> {
        let principal = &ticket.principal;
        let Some(p) = self.store.principal(principal)? else {
            return Err(DirectoryError::UnknownPrincipal(principal.clone()));
        };
        // A deleted account is not recovered: a recovery is a way back in.
        self.refuse_deleted(principal)?;
        if let Some(owner) = self.store.principal_for(new)? {
            return Err(DirectoryError::AlreadyLinked {
                authenticator: new.clone(),
                to: owner,
            });
        }
        let held = self.store.links_of(principal)?;
        // A person whose only sign-ins belong to outside providers is
        // recovered by those providers, and a recovery here would give them
        // a way in the provider never granted.
        if !held.is_empty() && held.iter().all(|(a, _)| a.is_outside_provider()) {
            return Err(DirectoryError::RecoveryRefused(OUTSIDE_PROVIDER));
        }
        let retire: Vec<AuthenticatorId> = if lost.is_empty() {
            held.iter()
                // An outside provider's link is never retired: the provider
                // manages it.
                .filter(|(a, m)| m.retired.is_none() && !a.is_outside_provider())
                .map(|(a, _)| a.clone())
                .collect()
        } else {
            for a in lost {
                if a.is_outside_provider() {
                    return Err(DirectoryError::RecoveryRefused(OUTSIDE_PROVIDER));
                }
                if !held.iter().any(|(h, _)| h == a) {
                    return Err(DirectoryError::RecoveryRefused(
                        "a lost authenticator is not this person's",
                    ));
                }
            }
            lost.to_vec()
        };
        let presence_from = Timestamp(at.0.saturating_add(ticket.quarantine_secs));
        let contest_until = Timestamp(at.0.saturating_add(ticket.contest_secs));
        self.store
            .insert_link_marked(new, &p, at, presence_from.0, &ticket.case)?;
        let mut retired = Vec::new();
        for a in &retire {
            let r = Retired {
                case: ticket.case.clone(),
                until: contest_until.0,
            };
            if let Err(e) = self.store.set_retired(a, Some(&r)) {
                for done in &retired {
                    let _ = self.store.set_retired(done, None);
                }
                let _ = self.store.remove_link(new);
                return Err(e.into());
            }
            retired.push(a.clone());
        }
        Ok(Recovered {
            principal: principal.clone(),
            case: ticket.case.clone(),
            added: new.clone(),
            retired,
            presence_from,
            contest_until,
        })
    }

    /// Take a recovery back before anyone has relied on it: remove the link
    /// it added and restore the links it retired. For the host, when a step
    /// after [`Directory::recover`] fails. It is not a contest, and asks
    /// nothing of any authenticator.
    pub fn undo_recovery(&mut self, r: &Recovered) -> Result<(), DirectoryError> {
        for a in &r.retired {
            self.store.set_retired(a, None)?;
        }
        self.store.remove_link(&r.added)?;
        Ok(())
    }

    /// **Contest a recovery** with an authenticator it retired, and do
    /// nothing else with it.
    ///
    /// Removes what the recovery added and restores what it retired. The
    /// caller has already verified an assertion from `by`; this checks that
    /// `by` is retired and its window is open. A retired authenticator that
    /// reaches any other function of the directory is refused there.
    ///
    /// **Every other link made at or after `opened`, the instant the case
    /// opened, goes too** (owner ruling 2026-10-25, "what a contest undoes"):
    /// whoever held the recovered passkey could add more, and the owner must
    /// not be left facing a sign-in only the thief holds. They are listed in
    /// [`Contested::enrolled_since`] as well as in `removed`. The owner keeps
    /// at least `by`, which is restored.
    pub fn contest(
        &mut self,
        by: &AuthenticatorId,
        at: Timestamp,
        opened: Timestamp,
    ) -> Result<Contested, DirectoryError> {
        let principal = self
            .store
            .principal_for(by)?
            .ok_or_else(|| DirectoryError::NotRetired(by.clone()))?;
        let Some(Retired { case, until }) = self.store.link_meta(by)?.and_then(|m| m.retired)
        else {
            return Err(DirectoryError::NotRetired(by.clone()));
        };
        if until <= at.0 {
            return Err(DirectoryError::NotRetired(by.clone()));
        }
        let mut removed = Vec::new();
        let mut restored = Vec::new();
        let mut enrolled_since = Vec::new();
        for (a, m) in self.store.links_of(&principal)? {
            if m.recovered_case.as_deref() == Some(case.as_str()) {
                self.store.remove_link(&a)?;
                removed.push(a);
            } else if m.retired.as_ref().is_some_and(|r| r.case == case) {
                self.store.set_retired(&a, None)?;
                restored.push(a);
            } else if m.linked_at >= opened.0 && m.retired.is_none() {
                self.store.remove_link(&a)?;
                enrolled_since.push(a.clone());
                removed.push(a);
            }
        }
        Ok(Contested {
            principal,
            case,
            removed,
            restored,
            enrolled_since,
        })
    }

    /// Remove every retired link whose contest window has closed. Returns
    /// them, so the caller can remove the public keys behind them.
    pub fn purge_retired(
        &mut self,
        now: Timestamp,
    ) -> Result<Vec<(AuthenticatorId, PrincipalId, String)>, DirectoryError> {
        let mut gone = Vec::new();
        for (a, p, r) in self.store.retired_by(now.0)? {
            self.store.remove_link(&a)?;
            gone.push((a, p, r.case));
        }
        Ok(gone)
    }

    /// The post-`complete` step of every login: resolve the authenticated
    /// subject to a principal. Under `AutoOnboard`, an unknown authenticator
    /// mints a human principal and links it — both audited (two events).
    /// Under `InviteOnly`, unknown = [`DirectoryError::NotInvited`].
    ///
    /// **Idempotent across a restart.** The link this checks first is the
    /// same durable row an earlier onboard wrote, so a second login from an
    /// authenticator this deployment has already seen resolves to the same
    /// principal whether or not the process in between restarted — minting
    /// is only ever reached the first time an authenticator is seen.
    pub fn resolve_or_onboard(
        &mut self,
        subject: &AuthenticatedSubject,
        display: &str,
        at: Timestamp,
    ) -> Result<(PrincipalId, Vec<AuditEvent>), DirectoryError> {
        self.resolve_or_onboard_as(subject, display, at, Onboarding::ByPolicy)
    }

    /// [`Directory::resolve_or_onboard`], told whether the caller has spent
    /// an invitation for this person.
    ///
    /// [`Onboarding::Invited`] lets an unknown authenticator be minted under
    /// `InviteOnly` too (owner ruling 2026-10-09, federated ask 9). Nothing
    /// else changes: a known authenticator resolves, and a deleted or retired
    /// one is refused, before the invitation counts for anything.
    pub fn resolve_or_onboard_as(
        &mut self,
        subject: &AuthenticatedSubject,
        display: &str,
        at: Timestamp,
        onboarding: Onboarding,
    ) -> Result<(PrincipalId, Vec<AuditEvent>), DirectoryError> {
        if let Some(existing) = self.store.principal_for(&subject.authenticator)? {
            // A deleted account keeps its links, so its identifiers name it
            // and nobody else; none of them signs in. Asked before anything
            // that could onboard the authenticator as a stranger.
            self.refuse_deleted(&existing)?;
            // A retired authenticator is refused by name, before anything
            // that could onboard it as a stranger: it is not unknown, it is
            // one a recovery took out of use.
            if let Some(r) = self
                .store
                .link_meta(&subject.authenticator)?
                .and_then(|m| m.retired)
            {
                return Err(DirectoryError::AuthenticatorRetired {
                    authenticator: subject.authenticator.clone(),
                    case: r.case,
                    contest_until: r.until,
                });
            }
            return Ok((existing, Vec::new()));
        }
        match (self.provisioning, onboarding) {
            (Provisioning::InviteOnly, Onboarding::ByPolicy) => {
                Err(DirectoryError::NotInvited(subject.authenticator.clone()))
            }
            (Provisioning::AutoOnboard, _) | (_, Onboarding::Invited) => {
                let id = self.create_principal(PrincipalKind::Human, display, at)?;
                let onboarded = AuditEvent {
                    kind: AuditEventKind::PrincipalOnboarded,
                    principal: id.clone(),
                    authenticator: None,
                    existing_authenticator: None,
                    at,
                };
                // Bootstrap, not a link ceremony: the account was minted one
                // line ago and holds no factor that could vouch.
                let linked = self.link_first(&id, &subject.authenticator, at)?;
                Ok((id, vec![onboarded, linked]))
            }
        }
    }
}

#[cfg(test)]
mod claim_tests {
    use super::*;

    const AT: Timestamp = Timestamp(1_800_000_000);

    fn a(id: &str) -> AuthenticatorId {
        AuthenticatorId(id.into())
    }

    /// A directory holding Kim, signed in by `email:kim@example.org`.
    fn kim() -> (Directory, PrincipalId) {
        let mut dir = Directory::new(Provisioning::InviteOnly);
        let p = dir
            .create_principal(PrincipalKind::Human, "Kim", AT)
            .unwrap();
        dir.link_first(&p, &a("email:kim@example.org"), AT).unwrap();
        (dir, p)
    }

    #[test]
    fn a_claim_inside_its_window_links_and_names_the_sign_in_that_vouched() {
        let (mut dir, kim) = kim();
        let event = dir
            .claim(
                &kim,
                &a("email:kim@example.org"),
                AT,
                &a("oauth2:discord:1"),
                Timestamp(AT.0 + 300),
                300,
            )
            .unwrap();
        assert_eq!(event.kind, AuditEventKind::AuthenticatorLinked);
        assert_eq!(event.authenticator, Some(a("oauth2:discord:1")));
        assert_eq!(
            event.existing_authenticator,
            Some(a("email:kim@example.org"))
        );
        assert_eq!(
            dir.principal_for(&a("oauth2:discord:1")).unwrap(),
            Some(kim)
        );
    }

    #[test]
    fn a_claim_outside_its_window_is_refused() {
        let (mut dir, kim) = kim();
        let first = a("email:kim@example.org");
        let claimed = a("oauth2:discord:1");
        let late = dir.claim(&kim, &first, AT, &claimed, Timestamp(AT.0 + 301), 300);
        assert_eq!(late, Err(DirectoryError::ClaimRefused("window-closed")));
        // A first ceremony stamped after the claim is a clock lie.
        let early = dir.claim(&kim, &first, Timestamp(AT.0 + 10), &claimed, AT, 300);
        assert_eq!(early, Err(DirectoryError::ClaimRefused("window-closed")));
        assert_eq!(dir.principal_for(&claimed).unwrap(), None);
    }

    #[test]
    fn a_sign_in_of_another_account_vouches_for_nothing() {
        let (mut dir, kim) = kim();
        let other = dir
            .create_principal(PrincipalKind::Human, "Mallory", AT)
            .unwrap();
        dir.link_first(&other, &a("email:m@example.net"), AT)
            .unwrap();
        let r = dir.claim(
            &kim,
            &a("email:m@example.net"),
            AT,
            &a("oauth2:discord:1"),
            AT,
            300,
        );
        assert!(matches!(
            r,
            Err(DirectoryError::ExistingAuthenticatorNotLinked { .. })
        ));
    }

    #[test]
    fn any_sign_in_the_account_holds_vouches_for_a_new_passkey() {
        let (mut dir, kim) = kim();
        let event = dir
            .claim(
                &kim,
                &a("email:kim@example.org"),
                AT,
                &a("passkey:AAAA"),
                AT,
                300,
            )
            .unwrap();
        assert_eq!(
            event.existing_authenticator,
            Some(a("email:kim@example.org"))
        );
        assert_eq!(dir.principal_for(&a("passkey:AAAA")).unwrap(), Some(kim));
    }

    #[test]
    fn a_claim_moves_nothing_between_accounts_and_revives_nothing() {
        let (mut dir, kim) = kim();
        let other = dir
            .create_principal(PrincipalKind::Human, "Nelly", AT)
            .unwrap();
        let discord = a("oauth2:discord:1");
        dir.link_first(&other, &discord, AT).unwrap();
        let first = a("email:kim@example.org");
        let live = dir.claim(&kim, &first, AT, &discord, AT, 300);
        assert!(matches!(live, Err(DirectoryError::AlreadyLinked { ref to, .. }) if *to == other));
        // Claiming a deleted account's sign-in out of it (D5).
        dir.mark_deleted(&other, AT).unwrap();
        let gone = dir.claim(&kim, &first, AT, &discord, AT, 300);
        assert_eq!(gone, Err(DirectoryError::Deleted(other.clone())));
        assert_eq!(dir.principal_for(&discord).unwrap(), Some(other));
        // Claiming into a deleted account (D5).
        dir.mark_deleted(&kim, AT).unwrap();
        let into = dir.claim(&kim, &first, AT, &a("oauth2:discord:2"), AT, 300);
        assert_eq!(into, Err(DirectoryError::Deleted(kim)));
    }
}
