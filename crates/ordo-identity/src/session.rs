//! The join between a finished ceremony and a platform token — the step the
//! crate was missing.
//!
//! Every piece of the step-up story existed except this one. A provider could
//! complete a ceremony; [`crate::token::mint`] could put a
//! `user_presence_at` in a token; [`crate::access::AccessEngine`] could gate
//! an op on the freshness of that stamp. Nothing carried the presence from
//! the first to the second, so the third could only ever deny. This module is
//! that carry, and it is deliberately small enough to read in one sitting,
//! because it is the point where "a human was there" becomes an authorization
//! fact.
//!
//! Two operations, and the difference between them matters:
//!
//! - [`sign_in`] — a ceremony that *establishes* a session. The subject is
//!   resolved to a principal by the directory, which is where invite-only and
//!   auto-onboard policy lives.
//! - [`refresh_presence`] — a ceremony that *re-proves presence* inside a
//!   session that already exists. This is what a step-up prompt does, and its
//!   defining obligation is the binding check: the authenticator that just
//!   asserted must belong to **the session's** principal. Without that check a
//!   step-up prompt would accept any passkey in the deployment, and the
//!   attacker with a stolen bearer token supplies their own.
//!
//! # The rest of a session's life
//!
//! Sign-in was the whole of it. A token was minted with a fixed one-hour
//! expiry and then nothing could touch it: there was no way to end a session
//! early and no way to continue one without running the ceremony again. The
//! module doc on [`crate::token`] said so plainly: "no refresh-token
//! machinery (the session store's, when sessions exist), and no revocation
//! list". Sessions exist now, and three operations complete them:
//!
//! - [`logout`]: this session stops being accepted, before its token
//!   expires.
//! - [`refresh`]: this session continues without re-authenticating, up to
//!   an absolute ceiling it cannot extend.
//! - [`SessionRevocations::is_revoked`]: the question the door asks on
//!   every request, and the one whose cost had to be argued for.
//!
//! **What makes all three possible is one field**: a token now names its
//! session ([`SessionClaim`]). Before it, two tokens from two logins by one
//! person were indistinguishable to anything downstream, so "log this one
//! out" had no referent short of suspending the principal, which logs out
//! every device they own and is a membership change rather than a session
//! one.
//!
//! # What is deliberately not here
//!
//! A session *table*. There is no row per live session, and nothing counts
//! or lists them. Everything a live session asserts (who, since when, under
//! which client, with what presence) is in the token and authenticated by
//! its MAC, so the only durable state is the **deny list**: the sessions
//! somebody ended early. That list is bounded by logouts rather than by
//! logins, it prunes itself at each session's ceiling, and it is what lets
//! the door answer from memory. A session table would make the door's
//! question a database round trip per request, which is the cost this design
//! exists to avoid.
//!
//! The consequence, stated rather than discovered: an operator cannot list
//! the live sessions of a principal, because nothing knows them. "Sign out
//! everywhere" is therefore not built either; it wants a per-principal
//! epoch, which is the same deny-list mechanism keyed on the principal and
//! is a smaller increment than a session table.

use crate::access::TokenClaims;
use crate::directory::{AuditEvent, Directory, DirectoryError, Onboarding};
use crate::passkey::UserPresence;
use crate::principal::{AuthMethodKind, ClientId, PrincipalId, Timestamp};
use crate::provider::{AuthenticatedSubject, AuthenticatorId};
use crate::token::TokenSpec;

/// How long a freshly minted session token is good for.
///
/// An hour, and short on purpose: membership and presence are re-read at
/// every check, but a token's own claims are frozen at mint, so its lifetime
/// is how long a stale claim can live. [`refresh`] is what makes a short
/// token survivable.
pub const DEFAULT_SESSION_SECS: u64 = 3_600;

/// The ceiling a session cannot refresh past, counted from the sign-in that
/// started it.
///
/// Twelve hours: a working day plus the evening, so an ordinary day needs
/// one ceremony and an abandoned session is gone by morning. Without a
/// ceiling, refresh is a perpetual session and the one-hour expiry is
/// decoration: whoever holds a token holds it forever by asking hourly,
/// including whoever stole it.
///
/// **The number is the owner's**, and this constant is the default rather
/// than the ruling.
pub const DEFAULT_SESSION_MAX_SECS: u64 = 12 * 3_600;

/// One sign-in, named.
///
/// Opaque, random, and meaningless on its own: it indexes nothing but the
/// deny list, so an id observed in a log discloses no principal, no time and
/// no client. 128 bits from the OS CSPRNG, hex, which cannot contain the
/// `|` or `~` the token encoding splits on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub String);

impl SessionId {
    /// Mint a fresh one.
    pub fn mint() -> SessionId {
        let mut b = [0u8; 16];
        getrandom::getrandom(&mut b)
            .expect("the operating system CSPRNG is unavailable, refusing to mint a session");
        SessionId(b.iter().map(|x| format!("{x:02x}")).collect())
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a token says about the session it belongs to: which one, and when
/// that session began.
///
/// `started_at` rides in the token rather than in a table because it is what
/// [`refresh`] needs to enforce the ceiling, and a claim the MAC covers
/// cannot be moved by the holder. Putting it in a row instead would buy
/// nothing and cost a read on the one path that is allowed to be slow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionClaim {
    pub id: SessionId,
    pub started_at: Timestamp,
    /// Which second factor proved this sign-in, when one did
    /// (`plan-second-factor.md` §5.5). It is a claim about the sign-in, so it
    /// lives beside the session it belongs to: `refresh` and
    /// `refresh_presence` carry the whole claim through and cannot drop it.
    /// `None` is a session that took no second step, and a tenant that
    /// requires one refuses it at the door.
    pub second_step: Option<crate::second_step::SecondStepKind>,
    /// **Which way in** this session was authenticated: the connection for
    /// an outside provider (`oauth2:discord`, `oidc:google`), the method word
    /// otherwise (`passkey`, `password`). `plan-auth-federated.md` §2.4's
    /// `via` claim. Under the MAC like every other claim, carried through
    /// `refresh` and `refresh_presence` with the rest of the session, and read
    /// by a tenant's provider admission rule.
    pub via: Option<String>,
    /// An outside provider's two-step claim that counted when this session
    /// began (`plan-auth-federated.md` §8.2, `crate::upstream`). Separate
    /// from the token's `user_presence_at` and never read as it: the access
    /// engine asks its [`crate::upstream::UpstreamAcceptance`] whether the
    /// connection counts for the op and the space, and checks the age at
    /// that moment. Carried through `refresh` like the rest of the session,
    /// so it keeps ageing and a refresh cannot make it fresh.
    pub upstream_presence: Option<crate::upstream::UpstreamPresence>,
}

impl SessionClaim {
    /// A session beginning now.
    pub fn starting_at(at: Timestamp) -> SessionClaim {
        SessionClaim {
            id: SessionId::mint(),
            started_at: at,
            second_step: None,
            via: None,
            upstream_presence: None,
        }
    }

    /// This session, authenticated by way of `via`.
    pub fn with_via(mut self, via: String) -> SessionClaim {
        self.via = Some(via);
        self
    }

    /// This session, after a second step of `kind` proved it.
    pub fn with_second_step(mut self, kind: crate::second_step::SecondStepKind) -> SessionClaim {
        self.second_step = Some(kind);
        self
    }

    /// This session, with a provider's two-step claim that counted.
    pub fn with_upstream_presence(
        mut self,
        upstream: Option<crate::upstream::UpstreamPresence>,
    ) -> SessionClaim {
        self.upstream_presence = upstream;
        self
    }

    /// The instant past which no refresh may reach.
    pub fn ceiling(&self, max_secs: u64) -> Timestamp {
        Timestamp(self.started_at.0.saturating_add(max_secs))
    }
}

/// The deny list could not be written. Reads never fail; see
/// [`SessionRevocations::is_revoked`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationStoreError(pub crate::StoreFault);

impl std::fmt::Display for RevocationStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "session revocation store unavailable: {}", self.0)
    }
}

impl RevocationStoreError {
    /// Busy or unavailable, as the store's client said.
    pub fn class(&self) -> crate::FaultClass {
        self.0.class()
    }
}

impl std::error::Error for RevocationStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// The sessions that have been ended early.
///
/// # Why the read cannot fail and the write can
///
/// [`SessionRevocations::is_revoked`] is called by the door on **every
/// request**, and it returns `bool` rather than `Result` because there is no
/// answer a failure could give. `true` on an unreachable store shuts the
/// deployment down for every caller; `false` accepts tokens somebody
/// revoked. Neither is acceptable per request, so the failure is moved to
/// where it can be handled: an implementation holds the list in memory,
/// loads it once when it is built, and **refuses to be built** if it cannot
/// That becomes a refused boot, with the operator in front of it, rather
/// than an outage or a hole discovered later.
///
/// [`SessionRevocations::revoke`] is a write on the logout path, which is an
/// act somebody is waiting on and can be told about. It returns a `Result`
/// and a failed logout is reported as a failure rather than swallowed, since
/// a logout that silently did nothing is the worst outcome of the three.
///
/// `Send + Sync` because the door holds one for the life of the process.
pub trait SessionRevocations: Send + Sync {
    /// Has this session been ended? **No I/O**: see the trait doc.
    fn is_revoked(&self, id: &SessionId) -> bool;

    /// End this session. `until` is the instant past which the entry may be
    /// dropped, which is the session's ceiling: after it, every token naming
    /// this session is refused by its own expiry and the entry proves
    /// nothing.
    ///
    /// Idempotent. A second logout of the same session is success.
    fn revoke(&self, id: &SessionId, until: Timestamp) -> Result<(), RevocationStoreError>;

    /// Has every session this principal started at or before some instant
    /// been ended, and did **this** one start in that range? **No I/O**,
    /// like [`SessionRevocations::is_revoked`], and asked beside it on every
    /// request.
    ///
    /// This is "sign out everywhere": a per-principal epoch, which is the
    /// same deny-list mechanism keyed on the principal
    /// (`plan-enrolment.md` §4.3, `plan-passkey-recovery.md` §4.3). A store
    /// that cannot answer says `false`, which is what every store before
    /// this method said.
    fn principal_ended(&self, _principal: &PrincipalId, _started_at: Timestamp) -> bool {
        false
    }

    /// End every session `principal` has that started at or before `at`.
    /// `until` is the instant past which the entry may be dropped: `at` plus
    /// the session ceiling, after which every older token is dead by its own
    /// expiry.
    ///
    /// Sessions that start **after** `at` are not touched, so the person can
    /// sign in again the moment whatever prompted this is dealt with. Tokens
    /// that name no session (capability tokens, delegated tokens) are not
    /// sessions and are ended by withdrawing the grant or the delegation.
    ///
    /// Monotonic: a later call moves the epoch forward and an earlier one
    /// never moves it back. A store that cannot end every session says so
    /// rather than pretending.
    fn end_all(
        &self,
        _principal: &PrincipalId,
        _at: Timestamp,
        _until: Timestamp,
    ) -> Result<(), RevocationStoreError> {
        Err(RevocationStoreError(
            "this store cannot end every session a principal has".into(),
        ))
    }
}

/// The process-local deny list.
///
/// A deployment that runs one process and is never restarted would be
/// correct with this. Neither is true of a deployment, so it is for tests,
/// and `ordo_host::sessions::PgSessionRevocations` is what a binary boots:
/// a logout that a restart forgets is a logout that did not happen.
#[derive(Debug, Default)]
pub struct MemorySessionRevocations {
    rows: std::sync::RwLock<std::collections::BTreeMap<SessionId, Timestamp>>,
    /// principal -> (not_before, until). A session that started before
    /// `not_before` is ended.
    epochs: std::sync::RwLock<std::collections::BTreeMap<PrincipalId, (u64, Timestamp)>>,
}

impl MemorySessionRevocations {
    pub fn new() -> MemorySessionRevocations {
        MemorySessionRevocations::default()
    }

    /// Drop entries whose `until` has passed. The token naming such a
    /// session is refused by its own expiry, so the entry is no longer load
    /// bearing.
    pub fn prune(&self, now: Timestamp) {
        self.rows
            .write()
            .expect("revocations")
            .retain(|_, until| until.0 > now.0);
    }

    pub fn len(&self) -> usize {
        self.rows.read().expect("revocations").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SessionRevocations for MemorySessionRevocations {
    fn is_revoked(&self, id: &SessionId) -> bool {
        self.rows.read().expect("revocations").contains_key(id)
    }

    fn revoke(&self, id: &SessionId, until: Timestamp) -> Result<(), RevocationStoreError> {
        self.rows
            .write()
            .expect("revocations")
            .insert(id.clone(), until);
        Ok(())
    }

    fn principal_ended(&self, principal: &PrincipalId, started_at: Timestamp) -> bool {
        self.epochs
            .read()
            .expect("revocations")
            .get(principal)
            .is_some_and(|(not_before, _)| started_at.0 < *not_before)
    }

    fn end_all(
        &self,
        principal: &PrincipalId,
        at: Timestamp,
        until: Timestamp,
    ) -> Result<(), RevocationStoreError> {
        let mut epochs = self.epochs.write().expect("revocations");
        let e = epochs.entry(principal.clone()).or_insert((0, Timestamp(0)));
        // `at + 1`: a session that started in the very second this was
        // called is ended too, so a tie goes to the safe side.
        e.0 = e.0.max(at.0.saturating_add(1));
        e.1 = Timestamp(e.1 .0.max(until.0));
        Ok(())
    }
}

/// Why a session operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LifetimeError {
    /// The token names no session, so there is nothing to end or continue.
    /// A capability token and a delegated token are the ordinary cases: both
    /// are minted against a recorded grant or consent, and both are revoked
    /// by withdrawing *that*, not by logging out.
    NotASession,
    /// This session was ended. Presented after a [`logout`].
    Revoked,
    /// The session has reached the ceiling it was started under. The holder
    /// must run a ceremony again; there is nothing they can present that
    /// moves this.
    CeilingReached {
        started_at: Timestamp,
        ceiling: Timestamp,
    },
    /// The deny list could not be written, so a logout cannot be reported as
    /// having happened.
    Store(RevocationStoreError),
}

impl std::fmt::Display for LifetimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifetimeError::NotASession => f.write_str(
                "this token names no session; it is revoked by withdrawing the \
                             grant or the delegation it was minted against",
            ),
            LifetimeError::Revoked => f.write_str("this session was ended"),
            LifetimeError::CeilingReached { ceiling, .. } => write!(
                f,
                "this session reached its ceiling at {}; authenticate again",
                ceiling.0
            ),
            LifetimeError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LifetimeError {}

/// End a session: every token naming it is refused from here on, whatever
/// its expiry says.
///
/// Takes already-verified [`TokenClaims`], so the caller has proved the
/// presenter holds a token this deployment minted. That is the whole
/// authorization: holding the token **is** being the session, so a session
/// can always end itself and needs no further proof to do it. Nothing here
/// consults the directory, which is what lets logout keep working for a
/// principal whose membership has since changed.
///
/// Idempotent: logging out twice succeeds twice.
pub fn logout(
    revocations: &dyn SessionRevocations,
    claims: &TokenClaims,
    max_secs: u64,
) -> Result<SessionId, LifetimeError> {
    let session = claims
        .session
        .as_ref()
        .ok_or(LifetimeError::NotASession)?
        .clone();
    revocations
        .revoke(&session.id, session.ceiling(max_secs))
        .map_err(LifetimeError::Store)?;
    Ok(session.id)
}

/// Continue a session without re-authenticating.
///
/// Takes already-verified claims, so the token's signature and its expiry
/// have been checked by [`crate::token::verify`] before this runs: an
/// expired token cannot be refreshed, which is what keeps the one-hour
/// expiry meaningful. Four things are then true of what comes back:
///
/// 1. **It names the same session**, with the same `started_at`. The id is
///    deliberately *not* rotated, see the note below.
/// 2. **It cannot reach past the ceiling.** The new expiry is the sooner of
///    `now + session_secs` and the session's ceiling, so a session ends at
///    its ceiling whatever the refresh cadence.
/// 3. **It is no wider.** Same principal, same client. Presence is carried
///    across unchanged, so it keeps ageing against
///    [`crate::access::AccessEngine::step_up_max_age_secs`] from the
///    ceremony that produced it and a refresh cannot launder a stale
///    presence into a fresh one: the same rule
///    [`refresh_presence`] enforces from the other side.
/// 4. **It carries no capability and no delegation**, because a token that
///    carries either is not a session and is refused as
///    [`LifetimeError::NotASession`] above.
///
/// # Rotation, and why not
///
/// Rotating the session id on every refresh is the textbook refresh-token
/// hardening, and it buys detection of a stolen token: the thief and the
/// owner both refresh, one of them presents a superseded id, and the
/// deployment kills the session. It costs a durable write per refresh and a
/// record of *which* id supersedes which, which is the session table this
/// design does not have, and it turns any lost race, any retry, any second
/// tab into a logout for a user who did nothing wrong. Not rotating keeps
/// the deny list bounded by logouts and the door's answer in memory. The
/// trade is the owner's; this is the default, and it is the one the current
/// shape supports.
pub fn refresh(
    revocations: &dyn SessionRevocations,
    claims: &TokenClaims,
    now: Timestamp,
    session_secs: u64,
    max_secs: u64,
) -> Result<TokenSpec, LifetimeError> {
    let session = claims
        .session
        .as_ref()
        .ok_or(LifetimeError::NotASession)?
        .clone();
    // Revoked first, and before the ceiling: a session somebody ended is
    // ended, and the answer must not depend on how long ago it started.
    if revocations.is_revoked(&session.id)
        || revocations.principal_ended(&claims.principal, session.started_at)
    {
        return Err(LifetimeError::Revoked);
    }
    let ceiling = session.ceiling(max_secs);
    if now.0 >= ceiling.0 {
        return Err(LifetimeError::CeilingReached {
            started_at: session.started_at,
            ceiling,
        });
    }
    let expires = Timestamp(now.0.saturating_add(session_secs).min(ceiling.0));
    Ok(TokenSpec {
        principal: claims.principal.clone(),
        expires,
        client: claims.client.clone(),
        user_presence_at: claims.user_presence_at,
        capability: None,
        delegation: None,
        session: Some(session),
    })
}

/// A completed sign-in: who it is, what proved it, and whether presence came
/// with it. The audit events belong to the caller, who records them through
/// the gate — the directory never writes the log itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    pub principal: PrincipalId,
    pub authenticator: AuthenticatorId,
    pub method: AuthMethodKind,
    pub presence: Option<UserPresence>,
    /// Set when the ceremony proved presence and the authenticator is
    /// quarantined (`plan-passkey-recovery.md` §7): the presence is dropped
    /// from `presence`, and this says when it would count.
    pub presence_available_at: Option<Timestamp>,
    /// The provider's two-step claim that counted, if any. Copied into the
    /// session claim by [`SignIn::token_spec`].
    pub upstream_presence: Option<crate::upstream::UpstreamPresence>,
    pub audit: Vec<AuditEvent>,
}

impl SignIn {
    /// The token this session should be minted with.
    ///
    /// `user_presence_at` is filled from the ceremony and from nowhere else.
    /// A password or OIDC sign-in therefore produces a token that is valid,
    /// useful, and structurally incapable of satisfying a step-up op — which
    /// is `security-logging-design.md` §5's rule expressed as a data flow
    /// rather than as a check somebody has to remember.
    ///
    /// `session` is required rather than optional, and that is the point: a
    /// human sign-in that minted no session id produced a token nothing
    /// could log out, and the only way to make that impossible is to leave
    /// no way to not pass one. Callers mint it with
    /// [`SessionClaim::starting_at`].
    pub fn token_spec(
        &self,
        session: SessionClaim,
        expires: Timestamp,
        client: Option<ClientId>,
    ) -> TokenSpec {
        TokenSpec {
            principal: self.principal.clone(),
            expires,
            client,
            user_presence_at: self.presence.map(|p| p.at()),
            // The way in is a fact about this sign-in, so it is stated here
            // rather than remembered by each route that mints.
            session: Some({
                let session = match session.via {
                    Some(_) => session,
                    None => session.with_via(self.authenticator.via(self.method)),
                };
                // A claim the caller set on the session stands; otherwise the
                // ceremony's, and nothing else.
                if session.upstream_presence.is_some() {
                    session
                } else {
                    session.with_upstream_presence(self.upstream_presence.clone())
                }
            }),
            // A human session is never a capability token. The two are
            // exclusive by token shape in the engine, and this is the end that
            // has to keep them apart.
            capability: None,
            // A sign-in proves who you are; it does not hand you anybody
            // else. A delegated token is minted by the delegation consent
            // ceremony against a recorded delegation, never by logging in.
            delegation: None,
        }
    }
}

/// Resolve a completed ceremony to a principal and carry its presence forward.
///
/// `display` is used only when the deployment's policy is `AutoOnboard` and
/// this authenticator is new; under `InviteOnly` an unknown authenticator is
/// [`DirectoryError::NotInvited`] and nothing is created.
pub fn sign_in(
    directory: &mut Directory,
    subject: &AuthenticatedSubject,
    display: &str,
    at: Timestamp,
) -> Result<SignIn, DirectoryError> {
    sign_in_as(directory, subject, display, at, Onboarding::ByPolicy)
}

/// [`sign_in`], for a caller that has spent a valid invitation for this
/// person ([`Onboarding::Invited`]): an unknown authenticator is minted a
/// principal under `InviteOnly` as well.
pub fn sign_in_as(
    directory: &mut Directory,
    subject: &AuthenticatedSubject,
    display: &str,
    at: Timestamp,
    onboarding: Onboarding,
) -> Result<SignIn, DirectoryError> {
    let (principal, audit) = directory.resolve_or_onboard_as(subject, display, at, onboarding)?;
    // Recorded before the sign-in is handed back, so a token is never minted
    // for an identity the deployment has no record of using (E7 reads this
    // to tell a used identity from one nobody ever held).
    directory.note_sign_in(&principal, at)?;
    // A passkey a recovery added does not satisfy step-up until its
    // quarantine ends. The token is then valid for ordinary calls and
    // structurally unable to pass step-up, by the data flow that already
    // governs a password session: no presence in the claims.
    let quarantined_until = directory
        .link_meta(&subject.authenticator)?
        .map(|m| m.presence_not_before)
        .filter(|from| *from > at.0);
    let (presence, presence_available_at) = match (subject.presence, quarantined_until) {
        (Some(_), Some(from)) => (None, Some(Timestamp(from))),
        (presence, _) => (presence, None),
    };
    Ok(SignIn {
        principal,
        authenticator: subject.authenticator.clone(),
        method: subject.method,
        presence,
        presence_available_at,
        upstream_presence: subject.upstream_presence.clone(),
        audit,
    })
}

/// Why a presence refresh was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionError {
    /// The session presenting itself is a capability token. Presence is not
    /// delegable (`security-logging-design.md` §5), so an agent cannot step
    /// up on its delegator's behalf however good the assertion it carries —
    /// the refusal is structural, before the assertion is even looked at.
    PresenceNotDelegable { principal: PrincipalId },
    /// The ceremony that just completed proves identity but not presence — a
    /// password re-prompt, an OIDC round trip. Re-authenticating is not the
    /// same claim as being there, and this is where the two stop being
    /// interchangeable.
    MethodCannotProvePresence { method: AuthMethodKind },
    /// The authenticator that asserted is not linked to the session's
    /// principal — or is not linked at all.
    ///
    /// **This is the check that makes a step-up prompt mean anything.** An
    /// attacker holding a stolen bearer token can always run a perfectly
    /// valid passkey ceremony with their *own* authenticator; the assertion
    /// verifies, the presence is genuine, and it belongs to somebody else.
    /// Neither the provider nor the token layer can catch that: the provider
    /// knows credentials and not principals, and the token was already minted.
    /// It has to be caught here.
    PresenceForAnotherPrincipal {
        session: PrincipalId,
        authenticator: AuthenticatorId,
    },
    /// The session's account was deleted. Its sessions were ended when it
    /// was; this refuses a step-up that raced the deletion.
    AccountDeleted { principal: PrincipalId },
    /// The directory's store could not answer. Distinct on purpose: it does
    /// not depend on which authenticator or session was offered.
    StoreUnavailable(crate::directory::DirectoryStoreError),
}

impl From<crate::directory::DirectoryError> for SessionError {
    fn from(e: crate::directory::DirectoryError) -> SessionError {
        match e {
            crate::directory::DirectoryError::StoreUnavailable(s) => {
                SessionError::StoreUnavailable(s)
            }
            crate::directory::DirectoryError::Deleted(principal) => {
                SessionError::AccountDeleted { principal }
            }
            // `principal_for`, `link_meta` and `refuse_deleted` are the only
            // directory calls this module makes, and they return no other
            // `DirectoryError` variant.
            other => SessionError::StoreUnavailable(crate::directory::DirectoryStoreError(
                other.to_string().into(),
            )),
        }
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::PresenceNotDelegable { principal } => write!(
                f,
                "a capability token acting as `{principal}` cannot step up — user \
                 presence is not delegable"
            ),
            SessionError::MethodCannotProvePresence { method } => write!(
                f,
                "a {} ceremony proves identity, not presence",
                method.as_str()
            ),
            SessionError::PresenceForAnotherPrincipal {
                session,
                authenticator,
            } => write!(
                f,
                "authenticator `{authenticator}` is not linked to `{session}`, so its \
                 presence proves nothing about this session"
            ),
            SessionError::AccountDeleted { principal } => {
                write!(f, "the account `{principal}` was deleted")
            }
            SessionError::StoreUnavailable(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// Re-mint an existing session's token with a fresh presence stamp.
///
/// The order of the three refusals is the order of cost and of consequence:
/// the token's *shape* first (a capability token is stopped before any
/// ceremony is inspected), then the ceremony's *method*, then the binding
/// between the authenticator and the session.
///
/// Everything else about the session is carried through unchanged — same
/// principal, same client. A step-up must not be able to widen a session; it
/// exists to make one operation possible for a few minutes, not to hand out a
/// better token.
pub fn refresh_presence(
    directory: &Directory,
    claims: &TokenClaims,
    subject: &AuthenticatedSubject,
    expires: Timestamp,
) -> Result<TokenSpec, SessionError> {
    // Presence is not delegable (§5). A capability token has never been able
    // to carry it; a token acting **on somebody's behalf** is the same rule
    // with the name written on it — refreshing presence into one would let a
    // delegation satisfy a step-up gate, and step-up is the platform's only
    // mechanism for "not delegable". A second way past it would be a second
    // thing to get right.
    if claims.capability.is_some() || claims.delegation.is_some() {
        return Err(SessionError::PresenceNotDelegable {
            principal: claims.principal.clone(),
        });
    }
    let Some(presence) = subject.presence else {
        return Err(SessionError::MethodCannotProvePresence {
            method: subject.method,
        });
    };
    directory.refuse_deleted(&claims.principal)?;
    if directory.principal_for(&subject.authenticator)? != Some(claims.principal.clone()) {
        return Err(SessionError::PresenceForAnotherPrincipal {
            session: claims.principal.clone(),
            authenticator: subject.authenticator.clone(),
        });
    }
    // A retired authenticator may contest a recovery and do nothing else. A
    // quarantined one is a live way in whose presence does not count yet:
    // the session keeps the presence it had and gains none.
    let meta = directory
        .link_meta(&subject.authenticator)?
        .unwrap_or_default();
    if meta.retired.is_some() {
        return Err(SessionError::PresenceForAnotherPrincipal {
            session: claims.principal.clone(),
            authenticator: subject.authenticator.clone(),
        });
    }
    let quarantined = meta.presence_not_before > presence.at().0;
    Ok(TokenSpec {
        principal: claims.principal.clone(),
        expires,
        client: claims.client.clone(),
        user_presence_at: if quarantined {
            claims.user_presence_at
        } else {
            Some(presence.at())
        },
        capability: None,
        delegation: None,
        // The same session, carried through. A step-up re-mints a token; it
        // does not start a session, so a logout recorded against this id
        // must still refuse what comes back, otherwise stepping up would
        // be a way to escape a revocation.
        session: claims.session.clone(),
    })
}
