//! The door's other side: where a bearer token comes from.
//!
//! # What was missing, and what was not
//!
//! Every authorization surface in this deployment verifies a bearer token
//! (`IdentityAccess::check`), and until this module **nothing could issue
//! one**: `ordo_identity::token::mint` had no caller outside tests, and
//! there was no route that began or completed a ceremony. The identity
//! crate was whole and unreachable. A deployment could refuse every
//! request correctly and admit none.
//!
//! What was already built is most of it, and this module invents none of
//! it. `AuthProvider::begin` and `complete` are the ceremony
//! (`ordo_identity::provider`); `sign_in` resolves a completed ceremony to
//! a principal and onboards under the deployment's policy
//! (`ordo_identity::session`); `SignIn::token_spec` states what the token
//! may do; `token::mint` signs it. Four existing pieces, in that order,
//! behind two routes.
//!
//! # The rest of a session's life
//!
//! Sign-in was all there was. A token was minted with a one-hour expiry and
//! then nothing could touch it: no way to end a session early, no way to
//! continue one without running the ceremony again, and the only revocation
//! the platform had was suspending the principal, which ends every session
//! they hold on every device and is a membership change rather than a
//! session one.
//!
//! Two more routes close that. [`logout`] puts the session on the deny list
//! the door consults; [`refresh`] continues a session up to a ceiling it
//! cannot move. Both take the session's own bearer token and nothing else,
//! because holding the token **is** being the session.
//!
//! The door's side of it, and what it costs per request, is
//! `IdentityAccess::revocations`.
//!
//! # One answer, for the same reason the door gives one
//!
//! `IdentityAccess`'s doc states the rule for authorization: every refusal
//! is the same `Deny`, because "a caller who could tell them apart could
//! enumerate memberships and op names by watching which complaint
//! changes". Authentication has the same property and a sharper version of
//! it, because its callers are strangers by definition. An unknown
//! identifier, a wrong password, an expired challenge, an authenticator
//! nobody invited and a signature that does not verify are **one answer**
//! here. Distinguishing them turns this route into an account-existence
//! oracle, which is the enumeration `not_found` is careful about
//! everywhere else in this tree.
//!
//! The reasons are not discarded. They go to the security stream through
//! the recorder the deployment already runs, where the operator reads them
//! and the caller does not.
//!
//! # Absence is a first-class answer
//!
//! [`PlatformState::auth`](crate::serve::PlatformState) is an `Option`, and
//! a deployment with no authentication configured answers these routes
//! with `auth_unconfigured` rather than 404. The same argument the drain's
//! `wired: false` makes: a surface that is missing because nobody wired it
//! must not look like a surface that does not exist, or the operator reads
//! a configuration mistake as a design.

use std::sync::{Arc, Mutex};

use axum::extract::{FromRef, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::directory::Directory;
use ordo_identity::provider::{BeginRequest, ChallengeOrRedirect, CompleteRequest};
use ordo_identity::Timestamp;
use ordo_identity::{token, ProviderRegistry, TokenKey};
use serde_json::{json, Value};

use crate::serve::PlatformState;

pub mod account;
pub mod authenticators;
pub mod claim;
pub mod console;
pub mod contact;
pub mod decline;
pub mod delegate;
pub mod factors;
pub mod federated;
pub mod guest;
pub mod handoff;
pub mod human;
pub mod link;
pub mod methods;
pub mod oidc;
pub mod recover;
pub mod second_step;
pub mod share;
pub mod signup;
pub mod spaces;
pub mod store_failure;
pub mod tenant;

pub use store_failure::StoreFailure;

/// Extracted rather than taking the whole [`PlatformState`], so these two
/// routes depend on the one thing they use. It also means a deployment can
/// mount them over any state that can produce an [`AuthState`], which is
/// what lets them be exercised without standing up a pool and a
/// deployment to answer a question about a password.
impl FromRef<PlatformState> for Option<SharedAuth> {
    fn from_ref(state: &PlatformState) -> Option<SharedAuth> {
        state.auth.clone()
    }
}

/// What enrolment adds to the sign-in routes: the tickets a person redeems,
/// the admission a sign-in that names a tenant runs, and the credential ops
/// a forced password change needs.
///
/// Set once, after the platform ops are built, because those ops are built
/// after the auth state and hold the same stores. Absent, the routes behave
/// as they did before enrolment existed: no ticket is honoured, no tenant is
/// joined, no password change is forced.
#[derive(Default)]
pub struct Enrolment {
    pub tickets: Option<Arc<crate::tickets::Tickets>>,
    pub admission: Option<Arc<crate::admission::Admission>>,
    pub credentials: Option<Arc<crate::credential_ops::CredentialOps>>,
}

/// Everything the two routes need, held together because a deployment that
/// has one of these and not the others cannot answer at all.
///
/// The directory is behind a `Mutex` because `sign_in` takes it by mutable
/// reference: onboarding writes. Take it with [`AuthState::lock_directory`],
/// never `directory.lock()`: a Postgres directory does its I/O while the lock
/// is held.
pub struct AuthState {
    pub registry: ProviderRegistry,
    pub directory: Mutex<Directory>,
    /// The deny list [`logout`] writes and [`refresh`] reads, **the same
    /// one the door consults** (`IdentityAccess::revocations`). Two stores
    /// would be a logout the door never hears about, which is a logout that
    /// did not happen.
    ///
    /// `None` is a deployment with no logout: the route answers
    /// `auth_unconfigured` rather than pretending to have ended a session.
    /// A session that cannot be recorded as ended must not be reported as
    /// ended, because the client discards the token and believes it is
    /// safe.
    pub revocations: Option<Arc<dyn ordo_identity::session::SessionRevocations>>,
    /// **The same key `IdentityAccess` verifies with.** A deployment that
    /// mints under one key and verifies under another authenticates
    /// nobody, and does it silently: every token is well formed and every
    /// check refuses it.
    pub token_key: TokenKey,
    /// How long a minted token is good for.
    pub session_secs: u64,
    /// How long a session may be continued by [`refresh`] before a ceremony
    /// is required again, counted from the sign-in.
    pub session_max_secs: u64,
    /// Staying signed in across a reload (`ORDO_REMEMBER`,
    /// [`crate::renewal`]). `None` sets no renewal cookie and honours none.
    pub remember: Option<Arc<crate::renewal::Remember>>,
    /// How long sessions last, in three layers ([`crate::session_policy`]).
    /// `None` is the deployment's values alone: `session_secs`,
    /// `session_max_secs` and the renewal setting.
    pub policy: Option<Arc<crate::session_policy::SessionPolicies>>,
    /// The always-on slowdown after wrong passwords, per account
    /// ([`crate::guess_limits`]). In memory with Ordo's defaults until a
    /// deployment gives it the shared one, so it is never off by omission.
    pub guess: Arc<crate::guess_limits::GuessLimits>,
    /// `ORDO_AUTH_METHODS_MAX_AGE`: the `max-age` of `GET /auth/methods`.
    pub methods_max_age: u64,
    /// Wall clock, injectable so a test can pin it, matching
    /// `IdentityAccess::now`. The clock is an input here for the same
    /// reason it is there: an expiry a caller could outrun by lying about
    /// the time is not an expiry.
    pub now: Box<dyn Fn() -> u64 + Send + Sync>,
    /// Where the routes write what they decide: the security stream the
    /// door already writes to. The caller is told one thing and the stream
    /// is told why (`security-logging-design.md` §3). `None` records
    /// nothing, which is a test harness and never a deployment.
    pub recorder: Option<Arc<ordo_secevent::Recorder>>,
    /// The open-ceremony table, held so the supervisor's sweep can delete
    /// what expired. The passkey provider files its challenges here.
    pub ceremonies: Option<Arc<dyn ordo_identity::CeremonyStore>>,
    /// The passkey provider, typed: registration, step-up and the
    /// `/.well-known/webauthn` document need the concrete type, which the
    /// registry's trait object cannot give. The **same** provider the
    /// registry holds. `None` when `passkey` is not a method.
    pub passkey: Option<Arc<ordo_identity::PasskeyProvider>>,
    /// The Postgres deny list, concrete, so the supervisor can re-read it on
    /// a timer. The **same** store `revocations` holds.
    pub session_store: Option<Arc<crate::sessions::PgSessionRevocations>>,
    /// Tickets, admission and credential ops. See [`Enrolment`].
    enrolment: std::sync::OnceLock<Enrolment>,
    /// Credential recovery. Set once, with the tickets it redeems.
    recovery: std::sync::OnceLock<Arc<crate::recovery::Recovery>>,
    /// Built-in provisioning on sign-up (P2): the setting, the limits and the
    /// tenant a sign-in is given. Set once. Absent, nothing is provisioned.
    signup: std::sync::OnceLock<Arc<crate::self_serve::Signup>>,
    /// Where `GET /auth/spaces` reads a person's tenants from (P4).
    spaces: std::sync::OnceLock<Arc<spaces::TenantLister>>,
    /// Sharing between tenants (G3, G4, G5): what accept, window and leave
    /// need. Set once. Absent, the share routes answer the one refusal.
    sharing: std::sync::OnceLock<Arc<crate::share_ops::ShareOps>>,
    /// Open share links (L2): what `POST /auth/link` opens with. Set once.
    links: std::sync::OnceLock<Arc<crate::share_links::ShareLinks>>,
    /// The second step and its methods. Set once. Absent, no sign-in owes one.
    second_factors: std::sync::OnceLock<Arc<crate::second_factors::SecondFactors>>,
    /// Deleting one's own account (27aq). Set once. Absent, the route
    /// answers the one refusal.
    account: std::sync::OnceLock<Arc<account::Accounts>>,
    /// The OAuth2 connections, typed, by name: the callback route needs the
    /// concrete provider, which the registry's trait object cannot give. The
    /// **same** providers the registry holds.
    oauth2: std::collections::BTreeMap<String, Arc<ordo_identity::oauth2::OAuth2Provider>>,
    /// The OIDC connections, typed, by name, for the same reason
    /// ([`oidc`]).
    oidc: std::collections::BTreeMap<String, Arc<ordo_identity::oidc::OidcProvider>>,
    /// Where an outside provider's profile is kept beside the link, for
    /// `GET /auth/identities`. `None` keeps nothing, and the route answers an
    /// empty list.
    pub identity_attrs: Option<Arc<dyn crate::federation::IdentityAttrStore>>,
    /// The optional human check every `begin` asks first (E15,
    /// [`human::HumanCheck`]). `None` is off, the default.
    pub human: Option<human::HumanCheck>,
    /// Sign-in with an emailed code, typed: `begin` hands the page a binder,
    /// which the registry's trait cannot, and the mail worker composes its
    /// messages. The **same** provider the registry holds under `email`.
    pub email: Option<Arc<crate::email_code::EmailCodeProvider>>,
    /// `ORDO_AUTH_RETURN_URLS`, read by origin: where a subdomain hand-off
    /// may send the browser ([`handoff`]). `None` refuses every hand-off.
    pub returns: Option<Arc<ordo_identity::origin::OriginList>>,
    /// `ORDO_AUTH_HANDOFF_SECS`: how long a hand-off code lives.
    pub handoff_secs: u64,
    /// `ORDO_AUTH_LINKING` and its window: linking by claim ([`claim`]).
    pub linking: claim::Linking,
    /// Guest sign-in's setting (`plan-auth-federated.md` §7A). `None` makes
    /// no guest, the default.
    guests: Option<guest::Guests>,
    /// The tenants' own providers ([`tenant`]). Set once. Absent, a `begin`
    /// that names a tenant is refused naming the setting.
    tenant_connections: std::sync::OnceLock<Arc<crate::tenant_providers::TenantConnections>>,
    /// The consent ceremony's agent side ([`delegate`]). Set once. Absent,
    /// the two routes answer `auth_unconfigured`.
    consent: std::sync::OnceLock<Arc<crate::consent::Ceremony>>,
    /// A person's verified email address ([`contact`]). Set once. Absent,
    /// the routes answer `auth_unconfigured` and nobody is required to have
    /// one.
    contact: std::sync::OnceLock<Arc<crate::contact_email::ContactEmails>>,
    /// Console keys (`plan-console.md` 8.2, G14). Set once. Absent, the
    /// `console` method is refused like any method this deployment lacks.
    console_keys: std::sync::OnceLock<Arc<crate::console_keys::ConsoleKeys>>,
}

impl AuthState {
    /// The directory, locked **without stopping the runtime** (reported fix
    /// slow-passkey-signup, 2026-10-21).
    ///
    /// A Postgres directory runs each statement on its store's thread and
    /// waits for the answer while this lock is held. A runtime worker that
    /// blocked on a plain `lock()` behind it kept its share of the runtime,
    /// and the task that would have carried that answer could sit in that
    /// share. The holder then waited on the waiter: every sign-in and every
    /// request stopped, seen in a local end-to-end run under load. Waiting
    /// through [`ordo_exec::bridge::in_place`] hands the worker's share to
    /// another thread first. On a thread outside the runtime it is a plain
    /// lock.
    pub fn lock_directory(&self) -> std::sync::MutexGuard<'_, Directory> {
        ordo_exec::bridge::in_place(|| self.directory.lock().expect("directory mutex poisoned"))
    }

    pub fn new(registry: ProviderRegistry, directory: Directory, token_key: TokenKey) -> AuthState {
        AuthState {
            registry,
            directory: Mutex::new(directory),
            revocations: None,
            token_key,
            // An hour, and short on purpose: presence and membership are
            // re-read at every check, but a token's own claims are frozen
            // at mint, so its lifetime is how long a stale claim can live.
            // `refresh` is what makes a short token survivable.
            session_secs: ordo_identity::session::DEFAULT_SESSION_SECS,
            session_max_secs: ordo_identity::session::DEFAULT_SESSION_MAX_SECS,
            remember: None,
            policy: None,
            guess: Arc::new(crate::guess_limits::GuessLimits::in_memory(
                crate::guess_limits::Deployment::default(),
            )),
            methods_max_age: methods::DEFAULT_MAX_AGE,
            now: Box::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            }),
            recorder: None,
            ceremonies: None,
            passkey: None,
            session_store: None,
            enrolment: std::sync::OnceLock::new(),
            recovery: std::sync::OnceLock::new(),
            signup: std::sync::OnceLock::new(),
            spaces: std::sync::OnceLock::new(),
            sharing: std::sync::OnceLock::new(),
            links: std::sync::OnceLock::new(),
            second_factors: std::sync::OnceLock::new(),
            account: std::sync::OnceLock::new(),
            oauth2: std::collections::BTreeMap::new(),
            oidc: std::collections::BTreeMap::new(),
            identity_attrs: None,
            human: None,
            email: None,
            returns: None,
            handoff_secs: handoff::DEFAULT_SECS,
            linking: claim::Linking::default(),
            guests: None,
            tenant_connections: std::sync::OnceLock::new(),
            consent: std::sync::OnceLock::new(),
            contact: std::sync::OnceLock::new(),
            console_keys: std::sync::OnceLock::new(),
        }
    }

    /// Give the routes a person's email address. Once.
    pub fn wire_contact(&self, contact: Arc<crate::contact_email::ContactEmails>) {
        let _ = self.contact.set(contact);
    }

    pub fn contact(&self) -> Option<&Arc<crate::contact_email::ContactEmails>> {
        self.contact.get()
    }

    /// Give the `console` method its keys. Once.
    pub fn wire_console_keys(&self, keys: Arc<crate::console_keys::ConsoleKeys>) {
        let _ = self.console_keys.set(keys);
    }

    pub fn console_keys(&self) -> Option<&Arc<crate::console_keys::ConsoleKeys>> {
        self.console_keys.get()
    }

    /// How long a console key session lasts, when console keys are wired.
    fn console_session_secs(&self) -> u64 {
        self.console_keys()
            .map(|k| k.session_secs)
            .unwrap_or(crate::console_keys::DEFAULT_SESSION_SECS)
    }

    /// Give the delegation routes the ceremony. Once.
    pub fn wire_consent(&self, consent: Arc<crate::consent::Ceremony>) {
        let _ = self.consent.set(consent);
    }

    pub fn consent(&self) -> Option<&Arc<crate::consent::Ceremony>> {
        self.consent.get()
    }

    /// Give the hand-off its return list and its code lifetime
    /// (`plan-subdomain-signin.md`).
    pub fn with_handoff(
        mut self,
        returns: Option<ordo_identity::origin::OriginList>,
        secs: u64,
    ) -> AuthState {
        self.returns = returns.map(Arc::new);
        self.handoff_secs = secs;
        self
    }

    /// Set linking by claim: on or off, and its window ([`claim::Linking`]).
    pub fn with_linking(mut self, linking: claim::Linking) -> AuthState {
        self.linking = linking;
        self
    }

    /// Give `/auth/complete` the guest setting ([`guest`]).
    pub fn with_guests(mut self, guests: guest::Guests) -> AuthState {
        self.guests = Some(guests);
        self
    }

    pub fn guests(&self) -> Option<&guest::Guests> {
        self.guests.as_ref()
    }

    /// Give `begin` the emailed-code provider. The same provider must be in
    /// the registry under [`crate::email_code::METHOD`].
    pub fn with_email(mut self, provider: Arc<crate::email_code::EmailCodeProvider>) -> AuthState {
        self.email = Some(provider);
        self
    }

    /// Turn the human check on.
    pub fn with_human_check(mut self, check: human::HumanCheck) -> AuthState {
        self.human = Some(check);
        self
    }

    /// Give the callback route a connection. The same provider must be in
    /// the registry under the same name, or `begin` cannot reach it.
    pub fn with_oauth2(
        mut self,
        provider: Arc<ordo_identity::oauth2::OAuth2Provider>,
    ) -> AuthState {
        self.oauth2.insert(provider.name().to_string(), provider);
        self
    }

    /// The connection named `name`, typed.
    pub fn oauth2(&self, name: &str) -> Option<Arc<ordo_identity::oauth2::OAuth2Provider>> {
        self.oauth2.get(name).cloned()
    }

    /// Keep outside providers' profiles in `store`.
    pub fn with_identity_attrs(
        mut self,
        store: Arc<dyn crate::federation::IdentityAttrStore>,
    ) -> AuthState {
        self.identity_attrs = Some(store);
        self
    }

    /// Give the routes the second step. Once, like [`AuthState::wire_recovery`].
    pub fn wire_second_factors(&self, second: Arc<crate::second_factors::SecondFactors>) {
        let _ = self.second_factors.set(second);
    }

    pub fn second_factors(&self) -> Option<&Arc<crate::second_factors::SecondFactors>> {
        self.second_factors.get()
    }

    /// Give the routes credential recovery. Once, like
    /// [`AuthState::wire_enrolment`]. Absent, the recovery routes answer the
    /// one refusal and sign-in is as it was.
    pub fn wire_recovery(&self, recovery: Arc<crate::recovery::Recovery>) {
        let _ = self.recovery.set(recovery);
    }

    pub fn recovery(&self) -> Option<&Arc<crate::recovery::Recovery>> {
        self.recovery.get()
    }

    /// Give the routes built-in provisioning. Once, like the others.
    pub fn wire_signup(&self, signup: Arc<crate::self_serve::Signup>) {
        let _ = self.signup.set(signup);
    }

    pub fn signup(&self) -> Option<&Arc<crate::self_serve::Signup>> {
        self.signup.get()
    }

    /// Give `POST /auth/account/delete` its state. Once.
    pub fn wire_account(&self, accounts: Arc<account::Accounts>) {
        let _ = self.account.set(accounts);
    }

    pub fn account(&self) -> Option<&Arc<account::Accounts>> {
        self.account.get()
    }

    /// Give `GET /auth/spaces` its source. Once.
    pub fn wire_spaces(&self, lister: Arc<spaces::TenantLister>) {
        let _ = self.spaces.set(lister);
    }

    pub fn spaces(&self) -> Option<&Arc<spaces::TenantLister>> {
        self.spaces.get()
    }

    /// Give the share routes their state. Once.
    pub fn wire_sharing(&self, sharing: Arc<crate::share_ops::ShareOps>) {
        let _ = self.sharing.set(sharing);
    }

    pub fn sharing(&self) -> Option<&Arc<crate::share_ops::ShareOps>> {
        self.sharing.get()
    }

    /// Give `POST /auth/link` its state. Once.
    pub fn wire_links(&self, links: Arc<crate::share_links::ShareLinks>) {
        let _ = self.links.set(links);
    }

    pub fn links(&self) -> Option<&Arc<crate::share_links::ShareLinks>> {
        self.links.get()
    }

    /// The browser's timeout for a passkey ceremony, in milliseconds.
    pub fn passkey_timeout_ms(&self) -> u64 {
        timeout_ms(
            self.passkey
                .as_ref()
                .map(|p| p.challenge_ttl_secs())
                .unwrap_or(ordo_identity::CHALLENGE_TTL_SECS),
        )
    }

    /// Give the routes what enrolment adds. Once: a second call is ignored,
    /// because the routes may already be serving with the first.
    pub fn wire_enrolment(&self, enrolment: Enrolment) {
        let _ = self.enrolment.set(enrolment);
    }

    /// Give the state the concrete deny list, for the refresh worker.
    pub fn with_session_store(
        mut self,
        store: Arc<crate::sessions::PgSessionRevocations>,
    ) -> AuthState {
        self.session_store = Some(store);
        self
    }

    /// The builder form of [`AuthState::wire_enrolment`].
    pub fn with_enrolment(self, enrolment: Enrolment) -> AuthState {
        self.wire_enrolment(enrolment);
        self
    }

    fn enrolment(&self) -> Option<&Enrolment> {
        self.enrolment.get()
    }

    /// Give the state the typed passkey provider.
    pub fn with_passkey(mut self, passkey: Arc<ordo_identity::PasskeyProvider>) -> AuthState {
        self.passkey = Some(passkey);
        self
    }

    /// Give the state the ceremony store, for the sweep.
    pub fn with_ceremonies(
        mut self,
        ceremonies: Arc<dyn ordo_identity::CeremonyStore>,
    ) -> AuthState {
        self.ceremonies = Some(ceremonies);
        self
    }

    /// Give the routes the recorder. The **same** `Arc` the door holds, so
    /// a refusal here and a denial there land in one stream.
    pub fn with_recorder(mut self, recorder: Arc<ordo_secevent::Recorder>) -> AuthState {
        self.recorder = Some(recorder);
        self
    }

    /// Record one event. Never blocks on I/O and never fails a request.
    /// `principal` is the verified principal the event is about, or `None`
    /// for a caller who proved nothing.
    fn record(
        &self,
        principal: Option<&ordo_identity::PrincipalId>,
        op: &str,
        kind: ordo_secevent::EventKind,
        at: Timestamp,
    ) {
        let Some(recorder) = &self.recorder else {
            return;
        };
        recorder.observe(
            ordo_secevent::Observation {
                space: None,
                subject: match principal {
                    Some(p) => ordo_secevent::Subject::member(&p.0),
                    None => ordo_secevent::Subject::Unverified,
                },
                op: op.to_string(),
                kind,
                at: ordo_commandlog::Timestamp(at.0),
                correlation: ordo_secevent::correlation::mint(),
            },
            ordo_secevent::Relationship::None,
        );
    }

    /// A try the guessing slowdown refused ([`crate::guess_limits`]): a
    /// denial, so a burst of them collapses into one entry with its count.
    pub(crate) fn record_throttled(&self, op: &str, at: Timestamp) {
        self.record(
            None,
            op,
            ordo_secevent::EventKind::denied(ordo_secevent::RecordedDecision::Throttled),
            at,
        );
    }

    /// One refusal, with the reason the caller is not told.
    fn record_refusal(&self, op: &str, reason: &str, at: Timestamp) {
        self.record(
            None,
            op,
            ordo_secevent::EventKind::AuthenticationFailed {
                reason: reason.to_string(),
            },
            at,
        );
    }

    /// Give the routes the deny list. The **same** `Arc` the door holds.
    pub fn with_revocations(
        mut self,
        revocations: Arc<dyn ordo_identity::session::SessionRevocations>,
    ) -> AuthState {
        self.revocations = Some(revocations);
        self
    }

    /// Give the routes renewals: the store and the deployment's setting.
    /// The deny list handed to [`AuthState::with_revocations`] should be a
    /// [`crate::renewal::RenewingRevocations`] over the same store, or a
    /// sign-out would leave the cookie live.
    pub fn with_remember(mut self, remember: crate::renewal::Remember) -> AuthState {
        self.remember = Some(Arc::new(remember));
        self
    }

    /// Give the routes the session policy layers.
    pub fn with_policy(mut self, policy: Arc<crate::session_policy::SessionPolicies>) -> AuthState {
        self.policy = Some(policy);
        self
    }

    /// Give the routes the password slowdown's layers and its shared count.
    pub fn with_guess_limits(mut self, guess: Arc<crate::guess_limits::GuessLimits>) -> AuthState {
        self.guess = guess;
        self
    }

    /// The values in force for a session for `space`: the tenant's, else
    /// the app developer's, else the deployment's ([`crate::session_policy`]).
    /// With no policy wired, the deployment's values on this state.
    pub(crate) async fn effective(&self, space: Option<&str>) -> crate::session_policy::Effective {
        match &self.policy {
            Some(p) => p.effective(space).await,
            None => {
                let (remember, remember_secs) = match &self.remember {
                    Some(r) => (r.config.on, r.config.idle_secs),
                    None => (false, crate::renewal::DEFAULT_SECS),
                };
                let d = crate::session_policy::Source::Deployment;
                crate::session_policy::Effective {
                    token_secs: self.session_secs.min(self.session_max_secs),
                    max_secs: self.session_max_secs,
                    remember,
                    remember_secs,
                    remember_max_secs: None,
                    sources: [d; 5],
                }
            }
        }
    }

    /// The last step of every sign-in, once the tenant it is for is known.
    ///
    /// 1. The session's lifetimes are the ones in force for that tenant: when
    ///    they differ from the deployment's, the token is minted again with
    ///    them and `answer` carries the new token and times.
    /// 2. The tenant is noted beside the session, so a refresh resolves the
    ///    same tenant's values.
    /// 3. A renewal family is started, and the `Set-Cookie` value given
    ///    back ([`crate::renewal`]). `None` when renewals are off for this
    ///    tenant, when the page asked not to be remembered
    ///    (`"remember": false`), or when the store could not be written: the
    ///    sign-in still stands, and the person is simply not remembered here.
    pub(crate) async fn finish_sign_in(
        &self,
        headers: &axum::http::HeaderMap,
        body: Option<&Value>,
        spec: &ordo_identity::TokenSpec,
        space: Option<String>,
        answer: &mut Value,
        now: Timestamp,
    ) -> Option<String> {
        let session = spec.session.as_ref()?;
        let principal = &spec.principal;
        let eff = self.effective(space.as_deref()).await;
        // The ceiling, or the overall limit when that is shorter.
        let ceiling = session.started_at.0.saturating_add(eff.ceiling_secs());
        if eff.token_secs != self.session_secs || eff.ceiling_secs() != self.session_max_secs {
            let mut again = spec.clone();
            // Never past the ceiling in force.
            let expires = now.0.saturating_add(eff.token_secs).min(ceiling);
            again.expires = Timestamp(expires);
            answer["token"] = json!(token::mint(&self.token_key, &again));
            answer["expires_at"] = json!(expires);
            answer["session_expires_at"] = json!(ceiling);
        }
        if let (Some(policy), Some(space)) = (&self.policy, space.as_deref()) {
            policy.note_session(&session.id.0, space, ceiling).await;
        }
        let remember = self.remember.clone()?;
        if !eff.remember {
            return None;
        }
        if body
            .and_then(|b| b.get("remember"))
            .and_then(|v| v.as_bool())
            == Some(false)
        {
            return None;
        }
        let family = crate::renewal::Family {
            family: session.id.0.clone(),
            principal: principal.0.clone(),
            via: session.via.clone(),
            second_step: session.second_step.map(|k| k.as_str().to_string()),
            space,
            signed_in_at: now.0,
        };
        let secret = crate::renewal::mint_secret();
        let hash = crate::renewal::hash_of(&secret);
        // Never past the overall limit (owner, 2026-10-22).
        let idle = eff
            .remember_max_secs
            .map_or(eff.remember_secs, |m| m.min(eff.remember_secs));
        let until = now.0.saturating_add(idle);
        let store = remember.store.clone();
        let done = tokio::task::spawn_blocking(move || store.issue(&family, &hash, until)).await;
        match done {
            Ok(Ok(())) => Some(crate::renewal::set_cookie(headers, &secret, idle)),
            _ => {
                self.record_refusal("auth.remember", "renewal-store-unavailable", now);
                None
            }
        }
    }

    /// Set the session lifetime and the ceiling a refresh cannot pass.
    pub fn with_session_lifetime(mut self, secs: u64, max_secs: u64) -> AuthState {
        self.session_secs = secs;
        self.session_max_secs = max_secs;
        self
    }
}

/// Start a passkey **registration**: `POST /auth/register`
/// `{"method": "passkey", "display"?: "..."}`.
///
/// Registration is a different ceremony from sign-in and answers different
/// options: the person's user handle, the algorithms, and the credential ids
/// to exclude. The registry's `begin` cannot express that, so this route
/// calls the concrete provider. `POST /auth/complete` with
/// `kind: passkey_registration` finishes it, and finishing it is also a
/// sign-in: the principal is resolved or minted under the deployment's
/// policy, and a token with presence comes back.
///
/// Everything refused is the one answer: no passkey method, a malformed
/// body, and a ticket this build does not yet honour.
pub async fn register(State(auth): State<Option<SharedAuth>>, Json(body): Json<Value>) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(passkey) = auth.passkey.clone() else {
        auth.record_refusal("auth.register", "unknown-method", now);
        return refused();
    };
    if body.get("method").and_then(|m| m.as_str()) != Some("passkey") {
        auth.record_refusal("auth.register", "malformed-request", now);
        return refused();
    }
    // The optional human check (E15) comes before any ceremony is minted:
    // this is the first leg of passkey sign-up.
    if let Err(refusal) = auth.check_human(&body, now).await {
        return *refusal;
    }
    let display = body
        .get("display")
        .and_then(|d| d.as_str())
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| d.chars().take(120).collect::<String>())
        .unwrap_or_else(|| passkey.relying_party().name().to_string());
    // A ticket lets a person register for an identity somebody created. It
    // is only looked at here, never spent: it is spent when the ceremony
    // completes, and this ceremony is bound to it so it cannot finish
    // without it.
    let mut exclude: Vec<Vec<u8>> = Vec::new();
    let mut binding: Option<String> = None;
    if let Some(ticket) = body.get("ticket").and_then(|t| t.as_str()) {
        let Some(tickets) = auth.enrolment().and_then(|e| e.tickets.clone()) else {
            auth.record_refusal("auth.register", "ticket-not-honoured", now);
            return refused();
        };
        let auth2 = auth.clone();
        let ticket2 = ticket.to_string();
        let looked = tokio::task::spawn_blocking(move || {
            let rec = tickets
                .peek(&ticket2, &passkey_ticket())
                .map_err(TicketRefusal::from)?;
            let held = holds_a_factor(&auth2, rec.principal.as_ref())?;
            if held.is_some_and(|factors| !factors.is_empty()) {
                return Err(TicketRefusal::Refused("principal-holds-a-factor"));
            }
            Ok(crate::tickets::hash_of(&ticket2))
        })
        .await;
        match looked {
            Ok(Ok(hash)) => binding = Some(hash),
            Ok(Err(e)) => return e.answer(&auth, "auth.register", now),
            Err(_) => {
                auth.record_refusal("auth.register", "ceremony-task-failed", now);
                return refused();
            }
        }
        // Nothing to exclude: a ticket is only good for a person who holds
        // no factor yet, so there is no existing credential to list. The
        // list is kept for the ticket that names a principal with a
        // password, when a second-factor plan lets that register a passkey.
        exclude.clear();
    }
    // A sign-up without a ticket is counted against the sign-up limit before
    // anything is minted (P3). A ticket is somebody in a tenant inviting.
    if binding.is_none() {
        if let Err(refusal) = signup::registration_allowed(&auth, now).await {
            return *refusal;
        }
    }
    let p2 = passkey.clone();
    let minted = tokio::task::spawn_blocking(move || {
        p2.begin_registration_bound(now, &exclude, binding.as_deref())
    })
    .await;
    match minted {
        Ok(Ok(options)) => (
            StatusCode::OK,
            Json(creation_options_json(
                &options,
                passkey.relying_party().name(),
                &display,
                auth.passkey_timeout_ms(),
            )),
        )
            .into_response(),
        Ok(Err(e)) => {
            auth.record_refusal("auth.register", "ceremony-store-unavailable", now);
            StoreFailure::from(e).into_response()
        }
        Err(_) => {
            auth.record_refusal("auth.register", "ceremony-store-unavailable", now);
            StoreFailure::task_failed().into_response()
        }
    }
}

/// What a step-up needs from the bearer token before any ceremony runs: it
/// verifies, it names a session, that session has not been ended, and it is
/// neither a capability nor a delegation (presence is not delegable). The
/// revocation list is consulted first, as `refresh` does, so a logged-out
/// session cannot step up.
fn step_up_claims(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
    op: &str,
    now: Timestamp,
) -> Result<ordo_identity::TokenClaims, Box<Response>> {
    let Some(bearer) = bearer(headers) else {
        auth.record_refusal(op, "no-token", now);
        return Err(Box::new(refused()));
    };
    let Ok(claims) = ordo_identity::verify(&auth.token_key, &bearer, now) else {
        auth.record_refusal(op, "token-unverified", now);
        return Err(Box::new(refused()));
    };
    if claims.capability.is_some() || claims.delegation.is_some() {
        auth.record_refusal(op, "presence-not-delegable", now);
        return Err(Box::new(refused()));
    }
    let Some(session) = claims.session.as_ref() else {
        auth.record_refusal(op, "not-a-session", now);
        return Err(Box::new(refused()));
    };
    let Some(revocations) = auth.revocations.as_ref() else {
        return Err(Box::new(unconfigured()));
    };
    if revocations.is_revoked(&session.id)
        || revocations.principal_ended(&claims.principal, session.started_at)
    {
        auth.record_refusal(op, "session-ended", now);
        return Err(Box::new(refused()));
    }
    Ok(claims)
}

/// Start a **step-up** inside a session: `POST /auth/step-up/begin` with the
/// session's bearer token.
///
/// Mints an assertion challenge in its own phase, with the session's
/// principal in the row, and answers request options whose
/// `allow_credentials` is that principal's own passkeys. The server checks
/// the binding again at completion, so a browser that ignores the list gains
/// nothing.
pub async fn step_up_begin(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(passkey) = auth.passkey.clone() else {
        auth.record_refusal("auth.step-up.begin", "unknown-method", now);
        return refused();
    };
    let claims = match step_up_claims(&auth, &headers, "auth.step-up.begin", now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let auth2 = auth.clone();
    let session = claims
        .session
        .as_ref()
        .map(|x| x.id.0.clone())
        .expect("checked by step_up_claims");
    let minted = tokio::task::spawn_blocking(move || {
        let listed = auth2
            .directory
            .lock()
            .expect("directory mutex poisoned")
            .authenticators_of(&claims.principal)
            .map_err(StoreFailure::from)?;
        let allow: Vec<Vec<u8>> = listed
            .iter()
            .filter_map(|a| a.0.strip_prefix("passkey:"))
            .filter_map(|id| ordo_identity::passkey::credential_id_from_b64url(id).ok())
            .collect();
        // Bound to this one session (G15): a touch relayed for it cannot be
        // spent on another session of the same person.
        passkey
            .begin_step_up_for(now, &claims.principal, Some(&session), &allow)
            .map_err(StoreFailure::from)
    })
    .await;
    match minted {
        Ok(Ok(options)) => (
            StatusCode::OK,
            Json(request_options_json(&options, auth.passkey_timeout_ms())),
        )
            .into_response(),
        Ok(Err(failure)) => {
            auth.record_refusal("auth.step-up.begin", "store-unavailable", now);
            failure.into_response()
        }
        Err(_) => {
            auth.record_refusal("auth.step-up.begin", "store-unavailable", now);
            StoreFailure::task_failed().into_response()
        }
    }
}

/// Finish a step-up: `POST /auth/step-up` with the session's bearer token
/// and a `passkey_assertion` body. Answers the **same session**, re-minted
/// with fresh presence, and nothing wider: same principal, same client, same
/// session id, so a logout still refuses what comes back.
///
/// Refused, always as the one answer: no bearer, a token that does not
/// verify, a capability or delegated token, a logged-out session, a
/// challenge minted for another principal or another phase, a passkey that
/// belongs to another principal, and anything wrong with the assertion.
pub async fn step_up(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now_secs = (auth.now)();
    let now = Timestamp(now_secs);
    let Some(passkey) = auth.passkey.clone() else {
        auth.record_refusal("auth.step-up", "unknown-method", now);
        return refused();
    };
    let claims = match step_up_claims(&auth, &headers, "auth.step-up", now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let Some(CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    }) = parse_complete(&body)
    else {
        auth.record_refusal("auth.step-up", "malformed-request", now);
        return refused();
    };
    let auth2 = auth.clone();
    let claims2 = claims.clone();
    let done = tokio::task::spawn_blocking(move || {
        let (subject, bound_to, bound_session) = passkey
            .complete_step_up_for(
                &challenge,
                &credential_id,
                &client_data_json,
                &authenticator_data,
                &signature,
                now,
            )
            .map_err(StepUpFailure::Ceremony)?;
        if bound_to != claims2.principal {
            return Err(StepUpFailure::OtherPrincipal);
        }
        let session = claims2.session.as_ref().expect("checked by step_up_claims");
        if bound_session.as_deref() != Some(session.id.0.as_str()) {
            return Err(StepUpFailure::OtherSession);
        }
        // A console key session keeps its own short end: a touch makes it
        // present, never longer (G15).
        let ceiling = if console::is_console_session(&claims2) {
            session.ceiling(auth2.console_session_secs())
        } else {
            session.ceiling(auth2.session_max_secs)
        };
        let expires = Timestamp(now.0.saturating_add(auth2.session_secs).min(ceiling.0));
        let dir = auth2.lock_directory();
        let spec = ordo_identity::refresh_presence(&dir, &claims2, &subject, expires)
            .map_err(StepUpFailure::Session)?;
        // A passkey a recovery added is quarantined: the session comes back
        // with the presence it had and none gained, and is told when that
        // changes.
        let held_until = dir
            .link_meta(&subject.authenticator)
            .ok()
            .flatten()
            .map(|m| m.presence_not_before)
            .filter(|from| *from > now.0);
        Ok((spec, ceiling, held_until))
    })
    .await;
    let (spec, ceiling, held_until) = match done {
        Ok(Ok(x)) => x,
        Ok(Err(StepUpFailure::Ceremony(e))) => {
            auth.record_refusal("auth.step-up", auth_error_reason(&e), now);
            return ceremony_refusal(&e);
        }
        Ok(Err(StepUpFailure::OtherPrincipal)) => {
            auth.record_refusal("auth.step-up", "challenge-for-another-principal", now);
            return refused();
        }
        Ok(Err(StepUpFailure::OtherSession)) => {
            auth.record_refusal("auth.step-up", "challenge-for-another-session", now);
            return refused();
        }
        Ok(Err(StepUpFailure::Session(e))) => {
            let reason = session_error_reason(&e);
            auth.record_refusal("auth.step-up", reason, now);
            return refused();
        }
        Err(_) => {
            auth.record_refusal("auth.step-up", "ceremony-task-failed", now);
            return refused();
        }
    };
    auth.record(
        Some(&claims.principal),
        "auth.step-up",
        ordo_secevent::EventKind::StepUpSatisfied,
        now,
    );
    let expires_at = spec.expires.0;
    let token = token::mint(&auth.token_key, &spec);
    let mut answer = json!({
        "token": token,
        "expires_at": expires_at,
        "session_expires_at": ceiling.0,
    });
    if let Some(from) = held_until {
        answer["presence_available_at"] = json!(from);
    }
    (StatusCode::OK, Json(answer)).into_response()
}

/// The word the stream carries for a session error.
fn session_error_reason(e: &ordo_identity::SessionError) -> &'static str {
    match e {
        ordo_identity::SessionError::PresenceForAnotherPrincipal { .. } => {
            "passkey-of-another-principal"
        }
        ordo_identity::SessionError::PresenceNotDelegable { .. } => "presence-not-delegable",
        ordo_identity::SessionError::MethodCannotProvePresence { .. } => {
            "method-cannot-prove-presence"
        }
        ordo_identity::SessionError::AccountDeleted { .. } => "identity-deleted",
        ordo_identity::SessionError::StoreUnavailable(_) => "directory-unavailable",
        _ => "session-refused",
    }
}

/// Why a step-up did not re-mint. Each is the one answer to the caller and a
/// different word in the stream.
enum StepUpFailure {
    Ceremony(ordo_identity::AuthError),
    OtherPrincipal,
    OtherSession,
    Session(ordo_identity::SessionError),
}

fn display_of(body: &Value) -> String {
    body.get("display")
        .and_then(|d| d.as_str())
        .unwrap_or_default()
        .to_string()
}

/// A ticket that lets a person register a passkey.
fn passkey_ticket() -> crate::tickets::Want<'static> {
    crate::tickets::Want {
        purpose: Some(crate::tickets::Purpose::Passkey),
        ..Default::default()
    }
}

/// The authenticators the directory holds for a principal a ticket names, or
/// `None` when it names nobody.
fn holds_a_factor(
    auth: &AuthState,
    principal: Option<&ordo_identity::PrincipalId>,
) -> Result<Option<Vec<ordo_identity::AuthenticatorId>>, TicketRefusal> {
    let Some(p) = principal else {
        return Ok(None);
    };
    auth.directory
        .lock()
        .expect("directory mutex poisoned")
        .authenticators_of(p)
        .map(Some)
        .map_err(|e| TicketRefusal::Unavailable("directory-unavailable", e.into()))
}

/// Why a ticket did not let a registration through. Always the one answer to
/// the caller, except that a store which could not be read says so.
enum TicketRefusal {
    Refused(&'static str),
    Unavailable(&'static str, StoreFailure),
}

impl From<crate::tickets::TicketError> for TicketRefusal {
    fn from(e: crate::tickets::TicketError) -> TicketRefusal {
        match e {
            crate::tickets::TicketError::Refused => TicketRefusal::Refused("ticket-refused"),
            e @ crate::tickets::TicketError::Unavailable(_) => {
                TicketRefusal::Unavailable("ticket-store-unavailable", e.into())
            }
        }
    }
}

impl TicketRefusal {
    fn answer(self, auth: &AuthState, op: &str, now: Timestamp) -> Response {
        match self {
            TicketRefusal::Refused(reason) => {
                auth.record_refusal(op, reason, now);
                refused()
            }
            TicketRefusal::Unavailable(reason, failure) => {
                auth.record_refusal(op, reason, now);
                failure.into_response()
            }
        }
    }
}

/// Complete a registration made with a ticket: the ticket is spent, the
/// credential's authenticator is linked to the person it names (or to a
/// person minted for a ticket that names none), and a sign-in comes back.
///
/// The order is the order of what cannot be undone. The ceremony must have
/// been begun with **this** ticket. It is looked at again, and the person it
/// names must still hold no factor. Only then is it spent, and only then is
/// the link written, by `link_first`, which refuses a person who has any
/// link, so a stolen session can never use this to add a factor.
fn register_with_ticket(
    auth: &AuthState,
    pending: &ordo_identity::PendingRegistration,
    ticket: &str,
    display: &str,
    now: Timestamp,
) -> Result<ordo_identity::SignIn, TicketRefusal> {
    let Some(tickets) = auth.enrolment().and_then(|e| e.tickets.clone()) else {
        return Err(TicketRefusal::Refused("ticket-not-honoured"));
    };
    if pending.binding() != Some(crate::tickets::hash_of(ticket).as_str()) {
        return Err(TicketRefusal::Refused("ticket-binding"));
    }
    let rec = tickets.peek(ticket, &passkey_ticket())?;
    // A ticket that names a deleted account is no way back in: refused
    // before it is spent, and recorded as the sign-in it would have been
    // (audit 2026-10-06). The directory refuses the link too, so a ticket
    // this check missed would still link nothing.
    if let Some(p) = rec.principal.as_ref() {
        let deleted = auth
            .directory
            .lock()
            .expect("directory mutex poisoned")
            .refuse_deleted(p);
        match deleted {
            Ok(()) => {}
            Err(ordo_identity::DirectoryError::Deleted(_)) => {
                return Err(TicketRefusal::Refused("identity-deleted"));
            }
            Err(e) => {
                return Err(TicketRefusal::Unavailable(
                    "directory-unavailable",
                    e.into(),
                ))
            }
        }
    }
    if holds_a_factor(auth, rec.principal.as_ref())?.is_some_and(|f| !f.is_empty()) {
        return Err(TicketRefusal::Refused("principal-holds-a-factor"));
    }
    let redeemed = tickets.redeem_for(ticket, &passkey_ticket())?;
    let subject = pending.subject();
    let mut dir = auth.lock_directory();
    let mut audit = Vec::new();
    let principal = match redeemed.principal {
        Some(p) => p,
        None => {
            let shown = if redeemed.display.is_empty() {
                display
            } else {
                redeemed.display.as_str()
            };
            let p = dir
                .create_principal(ordo_identity::PrincipalKind::Human, shown, now)
                .map_err(|e| TicketRefusal::Unavailable("directory-unavailable", e.into()))?;
            audit.push(ordo_identity::AuditEvent {
                kind: ordo_identity::AuditEventKind::PrincipalOnboarded,
                principal: p.clone(),
                authenticator: None,
                existing_authenticator: None,
                at: now,
            });
            p
        }
    };
    audit.push(
        dir.link_first(&principal, &subject.authenticator, now)
            .map_err(|_| TicketRefusal::Refused("link-refused"))?,
    );
    Ok(ordo_identity::SignIn {
        principal,
        authenticator: subject.authenticator.clone(),
        method: subject.method,
        presence: subject.presence,
        presence_available_at: None,
        upstream_presence: subject.upstream_presence.clone(),
        audit,
    })
}

/// The registration options, in the shape `navigator.credentials.create`
/// takes once the byte strings are decoded. `user.name` and
/// `user.display_name` are shown by the authenticator and by nothing else.
fn creation_options_json(
    options: &ordo_identity::CreationOptions,
    rp_name: &str,
    display: &str,
    timeout: u64,
) -> Value {
    json!({
        "challenge": options.challenge,
        "method": "passkey",
        "rp_id": options.rp_id,
        "rp_name": rp_name,
        "user": {"id": options.user_handle, "name": display, "display_name": display},
        "algorithms": options.algorithms,
        "user_verification": options.user_verification,
        "resident_key": "required",
        "attestation": "none",
        "exclude_credentials": options.exclude_credentials,
        "timeout_ms": timeout,
    })
}

/// How long a browser is told to wait for the person: the challenge's own
/// life, so the page gives up when the server would.
fn timeout_ms(secs: u64) -> u64 {
    secs.saturating_mul(1000)
}

/// The sign-in request options, in the shape `navigator.credentials.get`
/// takes once the byte strings are decoded. The shape is this route's:
/// `RequestOptions` is plain data with no serializer of its own.
fn request_options_json(options: &ordo_identity::RequestOptions, timeout: u64) -> Value {
    json!({
        "challenge": options.challenge,
        "method": "passkey",
        "rp_id": options.rp_id,
        "user_verification": options.user_verification,
        "allow_credentials": options.allow_credentials,
        "timeout_ms": timeout,
    })
}

/// The word the stream carries for a failed ceremony. A fixed vocabulary:
/// nothing the caller sent is copied into it.
fn auth_error_reason(e: &ordo_identity::AuthError) -> &'static str {
    use ordo_identity::AuthError as E;
    match e {
        E::UnknownChallenge(_) => "unknown-challenge",
        E::BadCredentials => "bad-credentials",
        E::StateMismatch(_) => "state-mismatch",
        E::NonceMismatch => "nonce-mismatch",
        E::EmptyAuthorizationCode => "empty-authorization-code",
        E::MissingSubject => "missing-subject",
        E::MethodMismatch { .. } => "method-mismatch",
        E::WebAuthn(_) => "webauthn-refused",
        E::CredentialAlreadyEnrolled => "credential-already-enrolled",
        E::StoreUnavailable(_) => "credential-store-unavailable",
        E::UpstreamUnavailable(_) => "identity-provider-unavailable",
        E::UpstreamRefused(why) => why,
        E::TooManyPending { .. } => "too-many-pending",
        E::ReturnNotAllowed => {
            "return-not-allowed: add the return origin to ORDO_AUTH_RETURN_URLS (any path on a listed origin is allowed)"
        }
        _ => "ceremony-failed",
    }
}

/// The one refusal. See the module doc for why there is only one.
fn refused() -> Response {
    EdgeError::new(EdgeCode::AuthenticationFailed, "that did not authenticate").into_response()
}

/// A ceremony's error as the route answers it: a store that did not answer
/// is its [`StoreFailure`] (busy or unavailable), anything else is the one
/// refusal.
fn ceremony_refusal(e: &ordo_identity::AuthError) -> Response {
    match StoreFailure::from_auth(e) {
        Some(f) => f.into_response(),
        None => refused(),
    }
}

/// The slowdown's answer ([`crate::guess_limits`]): `429
/// too_many_attempts` with `Retry-After`, naming the setting that changes
/// it. The same for an account that exists and one that does not.
pub(crate) fn throttled(what: &str, retry_after: u64, setting: &'static str) -> Response {
    let retry_after = retry_after.max(1);
    EdgeError::new(
        EdgeCode::TooManyAttempts,
        format!(
            "too many wrong passwords for {what} in a row: try again in {retry_after} seconds. \
             The wait is set with {setting}"
        ),
    )
    .with("retry_after", retry_after)
    .with("setting", setting)
    .retry_after(retry_after)
    .into_response()
}

fn unconfigured() -> Response {
    EdgeError::new(
        EdgeCode::AuthUnconfigured,
        "this deployment has no authentication provider wired",
    )
    .into_response()
}

/// Begin a ceremony: `{"method": "passkey", "identifier": "..."}`.
///
/// The identifier is optional and is the provider's business. It is never
/// used here to decide whether an account exists, which is the whole point
/// of the single refusal.
pub async fn begin(State(auth): State<Option<SharedAuth>>, Json(body): Json<Value>) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(method) = body.get("method").and_then(|m| m.as_str()) else {
        auth.record_refusal("auth.begin", "malformed-request", now);
        return refused();
    };
    // A console key (G14): a server's signed challenge, before the human
    // check and never for a tenant.
    if method == crate::console_keys::METHOD && body.get("tenant").is_none() {
        return console::begin(&auth, &body, now).await;
    }
    // A tenant's own provider (`plan-tenant-providers.md` §5).
    if let Some(t) = body.get("tenant").and_then(Value::as_str) {
        return tenant::begin(&auth, t, method, &body, now).await;
    }
    if auth.registry.get(method).is_err() {
        auth.record_refusal("auth.begin", "unknown-method", now);
        return refused();
    }
    // A sign-in that asks for a provider's two-step claim as presence
    // (§8.2). Only an OIDC connection carries one. Any connection may be
    // asked: a tenant may accept a claim the app developer left `off`
    // (ask 10), so where it counts is decided when it is used.
    let wants_presence = body
        .get("presence")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if wants_presence && auth.oidc(method).is_none() {
        auth.record_refusal("auth.begin", "presence-not-available", now);
        return oidc::presence_not_available(method);
    }
    // The optional human check (E15) comes before any ceremony is minted.
    if let Err(refusal) = auth.check_human(&body, now).await {
        return *refusal;
    }
    // An emailed code: one ceremony filed and one message queued, the same
    // work and the same answer for every address (§7.2, E11). The code is
    // minted later, by the worker that sends it.
    if method == crate::email_code::METHOD {
        if let Some(email) = auth.email.clone() {
            return email_begin(&auth, email, &body, now).await;
        }
    }
    // A passkey sign-in answers the request options a browser needs, and
    // asks for no identifier: the credential is discoverable, so the route
    // is not an existence oracle. The ceremony store is synchronous, so the
    // mint runs off the worker's thread.
    if method == "passkey" {
        if let Some(passkey) = auth.passkey.clone() {
            let minted =
                tokio::task::spawn_blocking(move || passkey.begin_assertion(now, &[])).await;
            return match minted {
                Ok(Ok(options)) => (
                    StatusCode::OK,
                    Json(request_options_json(&options, auth.passkey_timeout_ms())),
                )
                    .into_response(),
                Ok(Err(e)) => {
                    auth.record_refusal("auth.begin", "ceremony-store-unavailable", now);
                    StoreFailure::from(e).into_response()
                }
                Err(_) => {
                    auth.record_refusal("auth.begin", "ceremony-store-unavailable", now);
                    StoreFailure::task_failed().into_response()
                }
            };
        }
    }
    let req = BeginRequest {
        identifier: body
            .get("identifier")
            .and_then(|i| i.as_str())
            .map(str::to_string),
        return_to: body
            .get("return_to")
            .and_then(|r| r.as_str())
            .map(str::to_string),
        presence: wants_presence,
    };
    // A redirect method files its ceremony in the control database, so the
    // begin runs off the worker's thread like the passkey's.
    let auth2 = auth.clone();
    let method2 = method.to_string();
    let begun = tokio::task::spawn_blocking(move || {
        auth2
            .registry
            .get(&method2)
            .expect("the method resolved above and the registry is immutable at serve time")
            .begin(&req, now)
    })
    .await;
    match begun {
        Ok(Ok(ChallengeOrRedirect::Challenge { id, method })) => (
            StatusCode::OK,
            Json(json!({"challenge": id.0, "method": method.as_str()})),
        )
            .into_response(),
        // The binder goes to the page in the body and never in a URL. The
        // `state` is inside `redirect` and is not repeated: the page has no
        // use for it.
        Ok(Ok(ChallengeOrRedirect::Redirect { url, binder })) => (
            StatusCode::OK,
            Json(json!({"redirect": url, "binder": binder})),
        )
            .into_response(),
        Ok(Err(e)) => {
            auth.record_refusal("auth.begin", auth_error_reason(&e), now);
            match e {
                ordo_identity::AuthError::StoreUnavailable(_) => ceremony_refusal(&e),
                // Decided before any identity is known: an issuer whose
                // discovery could not be loaded.
                ordo_identity::AuthError::UpstreamUnavailable(why) => {
                    oidc::upstream_unavailable(&auth, "auth.begin", method, why, now)
                }
                // Too many challenges waiting: decided before any identifier
                // is read, so the answer names the setting that raises it.
                ordo_identity::AuthError::TooManyPending { .. } => {
                    EdgeError::new(EdgeCode::TooManyAttempts, e.to_string())
                        .retry_after(30)
                        .into_response()
                }
                _ => refused(),
            }
        }
        Err(_) => {
            auth.record_refusal("auth.begin", "ceremony-task-failed", now);
            refused()
        }
    }
}

/// Does an email `begin` carry an invitation a newcomer may sign up with?
/// Asked only under invite-only, where it decides whether an unknown address
/// is mailed. A store that cannot answer is `false`: the person is not
/// mailed, and the answer to `begin` does not change.
async fn email_begin_invited(auth: &AuthState, body: &Value) -> bool {
    let text = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::trim);
    let (Some(ticket), Some(space)) = (text("invitation"), text("space")) else {
        return false;
    };
    let invite_only = auth
        .directory
        .lock()
        .expect("directory mutex poisoned")
        .provisioning()
        == ordo_identity::Provisioning::InviteOnly;
    if !invite_only {
        return false;
    }
    let Some(admission) = auth.enrolment().and_then(|e| e.admission.clone()) else {
        return false;
    };
    matches!(
        admission.newcomer_invitation(space, ticket).await,
        Ok(Some(_))
    )
}

/// `begin` for an emailed code. See [`crate::email_code`].
async fn email_begin(
    auth: &AuthState,
    email: Arc<crate::email_code::EmailCodeProvider>,
    body: &Value,
    now: Timestamp,
) -> Response {
    let Some(identifier) = body
        .get("identifier")
        .and_then(|i| i.as_str())
        .map(str::to_string)
    else {
        auth.record_refusal("auth.begin", "malformed-request", now);
        return refused();
    };
    let device = body
        .get("device")
        .and_then(|d| d.as_str())
        .map(str::to_string);
    // An invitation a newcomer may sign up with lets the worker mail an
    // unknown address under invite-only. Only looked at, never spent: the
    // complete spends it. Whatever it is, the answer below is the same.
    // An open claim (step 7) does the same for the address being claimed.
    let invited = email_begin_invited(auth, body).await || claim::opens_mail(auth, body, now).await;
    let begun = tokio::task::spawn_blocking(move || {
        email.start_invited(&identifier, device.as_deref(), invited, now.0)
    })
    .await;
    match begun {
        Ok(Ok(b)) => (
            StatusCode::OK,
            Json(json!({
                "challenge": b.challenge,
                "binder": b.binder,
                "method": ordo_identity::AuthMethodKind::EmailOtp.as_str(),
            })),
        )
            .into_response(),
        Ok(Err(ordo_identity::AuthError::BadCredentials)) => {
            auth.record_refusal("auth.begin", "malformed-address", now);
            refused()
        }
        Ok(Err(e)) => {
            auth.record_refusal("auth.begin", auth_error_reason(&e), now);
            // The caller learns only "unavailable"; the owner's log says why
            // (a saturated pool, a missing table), so a 503 is diagnosable.
            eprintln!("auth: email begin answered 503: {e}");
            StoreFailure::from(e).into_response()
        }
        Err(_) => {
            auth.record_refusal("auth.begin", "ceremony-task-failed", now);
            refused()
        }
    }
}

/// Complete a ceremony and mint a bearer token.
///
/// The body is a [`CompleteRequest`] as JSON, tagged by `kind`. On success
/// the response carries the token and nothing else about the principal: a
/// caller learns who they are by presenting it, not by being told here.
pub async fn complete(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(mut body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now_secs = (auth.now)();
    let now = Timestamp(now_secs);
    let Some(method) = body
        .get("method")
        .and_then(|m| m.as_str())
        .map(str::to_string)
    else {
        auth.record_refusal("auth.complete", "malformed-request", now);
        return refused();
    };
    let method = method.as_str();
    // A console key (G14). Its own leg: no onboarding, no second step, no
    // renewal cookie, no presence.
    if method == crate::console_keys::METHOD && body.get("tenant").is_none() {
        return console::complete(&auth, &body, now).await;
    }
    // A tenant's own provider (`plan-tenant-providers.md` §6): resolved
    // from the tenant's database, and the arrival is at that tenant.
    let tenant_conn = match tenant::for_complete(&auth, &mut body, method, now).await {
        Ok(t) => t,
        Err(answer) => return *answer,
    };
    let tenant_provider = tenant_conn.as_ref().map(|t| t.live.provider());
    // Resolved here so an unknown method is refused before any work, and
    // resolved again inside the blocking task because a borrow cannot cross
    // it. The registry is immutable at serve time, so the second lookup
    // cannot disagree with the first.
    if tenant_conn.is_none() && auth.registry.get(method).is_err() {
        auth.record_refusal("auth.complete", "unknown-method", now);
        return refused();
    }
    // A new guest (§7A): the setting, the human check and the sign-up
    // ceiling are asked first, and the ceremony is the secret minted there.
    let guest_new = match guest::new_guest(&auth, &body, now).await {
        Ok(g) => g,
        Err(answer) => return *answer,
    };
    let Some(req) = guest::request(guest_new.as_deref()).or_else(|| parse_complete(&body)) else {
        auth.record_refusal("auth.complete", "malformed-request", now);
        return refused();
    };
    // The slowdown (owner, 2026-10-25): a password try is counted against
    // the identifier typed before it is checked, and a try inside its wait is
    // refused unchecked. Keyed by the account, never the address, and the
    // same for an identifier that exists and one that does not.
    let password_identifier = match (&req, &tenant_conn) {
        (CompleteRequest::Password { identifier, .. }, None) => Some(identifier.clone()),
        _ => None,
    };
    if let Some(identifier) = &password_identifier {
        let space = body.get("space").and_then(|s| s.as_str());
        let (attempt, eff) = auth
            .guess
            .password_attempt(identifier, space, now_secs)
            .await;
        if let crate::guess_limits::Attempt::Wait { retry_after } = attempt {
            auth.record_throttled("auth.complete", now);
            return throttled(
                "this sign-in",
                retry_after,
                crate::guess_limits::GuessLimits::setting_of(&eff),
            );
        }
    }

    // The ceremony. Every failure from here down is the same answer, with
    // one exception: a credential store that could not be read is not a
    // wrong password, it does not depend on the identifier, and telling
    // every user their password is wrong during a database incident sends
    // all of them to a password reset they do not need.
    //
    // Run on a blocking thread. The ceremony is synchronous by design and
    // spends tens of milliseconds in argon2id plus one database round trip;
    // doing that on an axum worker parks a thread that is serving every
    // other request on this process.
    let auth2 = auth.clone();
    let method2 = method.to_string();
    let completed = tokio::task::spawn_blocking(move || {
        // A passkey registration is verified here and **stored only after
        // the deployment's policy has admitted the person**, so a refused
        // registration leaves no public key behind.
        if let (Some(passkey), CompleteRequest::PasskeyRegistration { .. }) = (&auth2.passkey, &req)
        {
            if let CompleteRequest::PasskeyRegistration {
                challenge,
                client_data_json,
                attestation_object,
            } = &req
            {
                return passkey
                    .verify_registration(challenge, client_data_json, attestation_object, now)
                    .map(|p| (p.subject().clone(), Some(p)));
            }
        }
        if let Some(p) = &tenant_provider {
            return p.complete(&req, now).map(|s| (s, None));
        }
        let provider = auth2
            .registry
            .get(&method2)
            .expect("the method resolved above and the registry is immutable at serve time");
        provider.complete(&req, now).map(|s| (s, None))
    })
    .await;
    let (subject, pending) = match completed {
        Ok(Ok(s)) => {
            if let Some(identifier) = &password_identifier {
                auth.guess.password_right(identifier).await;
            }
            s
        }
        Ok(Err(e)) => {
            auth.record_refusal("auth.complete", auth_error_reason(&e), now);
            return ceremony_refusal(&e);
        }
        Err(_) => {
            auth.record_refusal("auth.complete", "ceremony-task-failed", now);
            return refused();
        }
    };
    // A first sign-in through an outside provider is a sign-up, and is known
    // to be one only now that the provider has answered. It is counted
    // against the deployment's sign-up ceiling here, before anything is
    // minted (§2.6). A returning person is never counted. An emailed code is
    // the same: the address is known to be new only once the code matched.
    let first_known_late = subject.authenticator.is_outside_provider()
        || subject.method == ordo_identity::AuthMethodKind::EmailOtp;
    if first_known_late {
        let known = auth
            .directory
            .lock()
            .expect("directory mutex poisoned")
            .principal_for(&subject.authenticator);
        match known {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(refusal) = signup::sign_up_allowed(&auth, "auth.complete", now).await {
                    return *refusal;
                }
            }
            Err(e) => {
                auth.record_refusal("auth.complete", "directory-unavailable", now);
                return StoreFailure::from(e).into_response();
            }
        }
    }
    // The name a new principal is minted with. For an outside provider it is
    // the profile the provider vouched for, and a `display` in the request is
    // not read: the person is who the provider says, under the name the
    // provider gives (§0.3 item 5). Password and passkey keep the name the
    // person typed, which is the only one there is.
    let display_owned = sign_in_display(&subject, &body);
    let display = display_owned.as_str();
    // A registration that presents a ticket is enrolment by invitation: the
    // ticket, not the deployment's policy, is what admits the person. A
    // ceremony begun with a ticket cannot finish without one, and the other
    // way round.
    let presented = body
        .get("ticket")
        .and_then(|t| t.as_str())
        .map(str::to_string);
    let bound = pending.as_ref().and_then(|p| p.binding()).is_some();
    if bound && presented.is_none() {
        auth.record_refusal("auth.complete", "ticket-required", now);
        return refused();
    }
    let ticketed = match (&pending, presented) {
        (Some(pending), Some(ticket)) => {
            let auth2 = auth.clone();
            let pending2 = pending.clone();
            let display2 = display_of(&body);
            let done = tokio::task::spawn_blocking(move || {
                register_with_ticket(&auth2, &pending2, &ticket, &display2, now)
            })
            .await;
            match done {
                Ok(Ok(signed)) => Some(signed),
                Ok(Err(e)) => return e.answer(&auth, "auth.complete", now),
                Err(_) => {
                    auth.record_refusal("auth.complete", "ceremony-task-failed", now);
                    return refused();
                }
            }
        }
        _ => None,
    };
    // An invitation lets a newcomer sign up under invite-only, with every
    // method (owner ruling 2026-10-09, federated ask 9). It is checked and
    // spent here, before anything is minted, so of two sign-ups racing with
    // one invitation only one is minted. Without a valid one the directory
    // refuses as it always did, with the one answer.
    let invited = if ticketed.is_none() {
        match newcomer_invitation(&auth, &subject, &body, now).await {
            Ok(role) => role,
            Err(answer) => return *answer,
        }
    } else {
        None
    };
    let signed = if let Some(signed) = ticketed {
        signed
    } else {
        let onboarding = match invited {
            Some(_) => ordo_identity::Onboarding::Invited,
            None => ordo_identity::Onboarding::ByPolicy,
        };
        let mut dir = auth.lock_directory();
        match ordo_identity::sign_in_as(&mut dir, &subject, display, now, onboarding) {
            Ok(s) => s,
            // Includes `NotInvited` under `InviteOnly`, which is exactly
            // the answer that must not differ from a bad signature: it
            // would otherwise tell a stranger whether an authenticator is
            // known to this deployment. The stream is told which.
            Err(e) => {
                drop(dir);
                // The person was not minted, so a spent invitation is
                // given back for them to try again.
                give_back(&auth, &body, invited.as_ref(), now);
                // A retired authenticator proved possession, so its holder
                // is told what happened and how to contest it. Nobody who
                // did not hold the key sees this: the assertion verified.
                if let ordo_identity::DirectoryError::AuthenticatorRetired {
                    case,
                    contest_until,
                    ..
                } = &e
                {
                    auth.record_refusal("auth.complete", "authenticator-retired", now);
                    return EdgeError::new(
                        EdgeCode::RecoveryContestable,
                        "a recovery retired this passkey; contest it with POST /auth/recover/contest or sign in another way",
                    )
                    .with("case", case)
                    .with("contest_until", contest_until)
                    .into_response();
                }
                // A deleted account is refused like any other sign-in that
                // does not go through: the answer tells a stranger nothing
                // about which identifiers this deployment knows.
                let reason = match e {
                    ordo_identity::DirectoryError::NotInvited(_) => "not-invited",
                    ordo_identity::DirectoryError::Deleted(_) => "identity-deleted",
                    _ => "sign-in-refused",
                };
                auth.record_refusal("auth.complete", reason, now);
                return refused();
            }
        }
    };
    // A password sign-in by a person whose password an administrator typed
    // does not get a session: they must choose their own first
    // (`ManagedStore::must_change`, A4). Asked only after the password has
    // proved who they are, so it tells no stranger anything, and it fails
    // closed: a table that cannot be read is a person who does not get in.
    // A passkey sign-in is not held up by it, since a passkey is the better
    // thing to have chosen.
    if signed.method == ordo_identity::AuthMethodKind::Password {
        if let Some(credentials) = auth.enrolment().and_then(|e| e.credentials.clone()) {
            match credentials.managed.must_change(&signed.principal.0).await {
                Ok(false) => {}
                Ok(true) => {
                    auth.record_refusal("auth.complete", "password-change-required", now);
                    return password_change_required();
                }
                Err(why) => {
                    auth.record_refusal("auth.complete", "credential-store-unavailable", now);
                    return StoreFailure::from(why).into_response();
                }
            }
        }
    }
    // The registration's key is stored now that the person is admitted. A
    // second registration of the same credential that got in between is the
    // one refusal, and the stream says why.
    if let (Some(pending), Some(passkey)) = (pending, auth.passkey.clone()) {
        let stored =
            tokio::task::spawn_blocking(move || passkey.store_registration(&pending)).await;
        if !matches!(stored, Ok(Ok(()))) {
            give_back(&auth, &body, invited.as_ref(), now);
        }
        match stored {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                auth.record_refusal("auth.complete", auth_error_reason(&e), now);
                return ceremony_refusal(&e);
            }
            Err(_) => {
                auth.record_refusal("auth.complete", "ceremony-task-failed", now);
                return refused();
            }
        }
    }
    // What the provider vouched for is kept beside the link, so the person
    // can read it back (`GET /auth/identities`). A store that cannot write
    // does not cost the person their sign-in; the stream says so. An emailed
    // code keeps the address it verified the same way.
    if first_known_late {
        if let Some(store) = auth.identity_attrs.clone() {
            let (who, s2) = (signed.principal.clone(), subject.clone());
            let noted = tokio::task::spawn_blocking(move || store.note(&who, &s2, now)).await;
            if !matches!(noted, Ok(Ok(()))) {
                auth.record_refusal("auth.complete", "identity-profile-unwritten", now);
            }
        }
    }
    // An invited newcomer is admitted now, at the role their spent invitation
    // named. Before the second-step gate, so a step owed later does not
    // leave them minted, the invitation spent, and no membership.
    let invited_outcome = match (&invited, body.get("space").and_then(|s| s.as_str())) {
        (Some(spent), Some(space)) => {
            let outcome = admit_invited(&auth, &signed.principal.0, space, &spent.role, now).await;
            // Not admitted after all: the invitation is given back. The
            // person is known now, so trying again with it is an ordinary
            // arrival that spends it then.
            if outcome == "refused" {
                give_back(&auth, &body, invited.as_ref(), now);
            }
            Some(outcome)
        }
        _ => None,
    };
    // The second-step gate: a first factor has succeeded and a session would
    // be minted. When a second step is owed no token is minted, and the answer
    // is the half-signed-in state (`auth::second_step`).
    if let Some(answer) = second_step::gate(&auth, &headers, &signed, &body, now).await {
        return answer;
    }
    // What the sign-in did to the directory is recorded before the token
    // exists: an onboarding is itself an audited event, and a token minted
    // with no record of the principal it created is the gap
    // `security-logging-design.md` §3 forbids.
    for event in &signed.audit {
        auth.record(
            Some(&event.principal),
            "auth.complete",
            ordo_secevent::EventKind::from(event),
            event.at,
        );
    }
    auth.record(
        Some(&signed.principal),
        "auth.complete",
        ordo_secevent::EventKind::Authenticated,
        now,
    );
    // What the provider said about how the person signed in there, for every
    // sign-in that carried it, and which connection vouched for presence
    // when its claim counted (§8.2).
    oidc::record_upstream_claim(&auth, &signed, &subject, now);
    // An address the outside provider marked verified is the person's, when
    // they have none yet (owner ruling 2026-10-22). It never replaces one
    // they chose, and a store that cannot write costs them nothing.
    contact::adopt_verified(&auth, &signed.principal.0, &subject, method, now).await;
    // Every sign-in names a session, so every token this route issues is
    // one a logout can refer to. `token_spec` requires it rather than
    // taking an `Option`, which is what makes that a property of the code
    // rather than a thing this handler remembers.
    let via = signed.authenticator.via(signed.method);
    let session = ordo_identity::SessionClaim::starting_at(now).with_via(via.clone());
    let spec = signed.token_spec(session, Timestamp(now_secs + auth.session_secs), None);
    let bearer = token::mint(&auth.token_key, &spec);
    let mut answer = json!({
        "token": bearer,
        // Which way in this session came: what a provider admission rule
        // reads, and what the page can show.
        "via": via,
        "expires_at": now_secs + auth.session_secs,
        // What the client needs to decide when to call `/auth/refresh`,
        // and when to stop trying. Publishing it is not a disclosure:
        // it is a property of the token the holder already has.
        "session_expires_at": now_secs + auth.session_max_secs,
    });
    // The person is told how many ways in they hold, and warned when it is
    // one. Information only: nothing is enforced on it (K2), and what an
    // application does with the notice is the app developer's.
    if let Some(from) = signed.presence_available_at {
        answer["presence_available_at"] = json!(from.0);
    }
    // A browser that signed in with an emailed code is given a marker for
    // the returning-device lane (E8). The page keeps it and sends it as
    // `device` with its next email `begin`.
    if signed.method == ordo_identity::AuthMethodKind::EmailOtp {
        if let (Some(email), Some(address)) = (auth.email.clone(), subject.attributes.email.clone())
        {
            let marked = tokio::task::spawn_blocking(move || {
                email.remember_device(address.address(), now_secs)
            })
            .await;
            if let Ok(Some(marker)) = marked {
                answer["device"] = json!(marker);
            }
        }
    }
    // What recovery has to tell this person: how many codes they hold, and
    // every recovery of theirs they have not yet acknowledged. The notice is
    // carried on every sign-in until they acknowledge it with presence
    // (`POST /auth/recover/ack`), so a recovery they did not expect cannot
    // pass unseen.
    // A new guest is given its secret once, here. The page keeps it.
    if let Some(secret) = guest_new {
        answer["guest_secret"] = json!(secret);
    }
    if let Some(recovery) = auth.recovery().cloned() {
        let who = signed.principal.0.clone();
        let looked = tokio::task::spawn_blocking(move || {
            (
                recovery.codes().and_then(|c| c.remaining(&who).ok()),
                recovery.notices(&who).ok(),
                recovery.manager_notices(&who).ok(),
            )
        })
        .await;
        if let Ok((left, notices, owner_cases)) = looked {
            if let Some(left) = left {
                answer["recovery_codes_remaining"] = json!(left);
            }
            if let Some(cases) = notices.filter(|c| !c.is_empty()) {
                answer["recoveries"] = crate::recovery::notices_json(&cases);
            }
            // A tenant's manager is told of every open case about an owner
            // of a tenant they manage, and may object (owner, 2026-10-24).
            if let Some(cases) = owner_cases.filter(|c| !c.is_empty()) {
                answer["owner_cases"] = crate::recovery::manager_notices_json(&cases);
            }
        }
    }
    let held = auth
        .directory
        .lock()
        .expect("directory mutex poisoned")
        .authenticators_of(&signed.principal)
        .map(|l| l.len());
    if let Ok(n) = held {
        answer["authenticators"] = json!(n);
        if n == 1 {
            answer["notices"] = json!(["single_authenticator"]);
        }
    }
    // A sign-in that names a tenant is an arrival there. The person is
    // signed in to the deployment whatever the tenant decides, so a refusal
    // is not a failed sign-in: it is `refused` and nothing more, whether the
    // tenant does not exist, does not admit strangers, or was handed a
    // ticket that is not for this person.
    if let Some(space) = body.get("space").and_then(|s| s.as_str()) {
        // The provider way of admission reads the connection this session
        // came by, and the domain of the address it vouched for, if any.
        let domain = subject.attributes.email.as_ref().map(|e| e.domain());
        let provider = first_known_late.then_some(crate::admission::Via {
            provider: via.as_str(),
            domain: domain.as_deref(),
        });
        let outcome = match (invited_outcome, &tenant_conn) {
            (Some(outcome), _) => outcome,
            (None, Some(t)) => tenant::arrive(&auth, t, &signed.principal.0).await,
            (None, None) => arrive(&auth, &signed.principal.0, space, &body, provider, now).await,
        };
        answer["admission"] = admission_answer(space, outcome, &body);
    }
    // Built-in provisioning (P2): after the session is minted, a person who
    // holds no tenant of their own is given one, when the app developer's
    // setting says so. The sign-in has succeeded whatever comes of it. A
    // tenant's own provider admits into that tenant only, so it gives none.
    if tenant_conn.is_none() {
        signup::given(&auth, &mut answer, &signed.principal.0, display, now).await;
    }
    // The tenant this session is for, whose session settings apply
    // ([`crate::session_policy`]) and whose renewal it is ([`crate::renewal`]):
    // a tenant's own sign-in connection, a tenant the person was admitted to
    // or already belongs to, or the tenant of their own. A tenant named in
    // the body and refused is not one: its settings must not be borrowed.
    let arrived = answer["admission"]["outcome"]
        .as_str()
        .is_some_and(|o| o == "admitted" || o == "already-member");
    let named = body.get("space").and_then(|s| s.as_str());
    let space = if tenant_conn.is_some() || arrived {
        named
    } else {
        answer.get("space").and_then(|s| s.as_str())
    }
    .map(str::to_string);
    let cookie = auth
        .finish_sign_in(&headers, Some(&body), &spec, space, &mut answer, now)
        .await;
    with_cookie((StatusCode::OK, Json(answer)).into_response(), cookie)
}

/// Add a `Set-Cookie` to a response, when there is one.
pub(crate) fn with_cookie(mut response: Response, cookie: Option<String>) -> Response {
    if let Some(c) = cookie.and_then(|c| axum::http::HeaderValue::from_str(&c).ok()) {
        response
            .headers_mut()
            .append(axum::http::header::SET_COOKIE, c);
    }
    response
}

/// The display name a sign-in uses. See the call in [`complete`].
fn sign_in_display(subject: &ordo_identity::AuthenticatedSubject, body: &Value) -> String {
    if subject.authenticator.is_outside_provider() {
        return subject.attributes.display.clone().unwrap_or_else(|| {
            // A provider that gave no name: the connection's name, so the
            // directory never holds text the caller chose.
            subject
                .authenticator
                .0
                .split(':')
                .nth(1)
                .unwrap_or_default()
                .to_string()
        });
    }
    body.get("display")
        .and_then(|d| d.as_str())
        .unwrap_or_default()
        .to_string()
}

/// The invitation a newcomer signs up with under invite-only, checked and
/// spent: the role it admits at, or `None`.
///
/// `None` when the deployment is not invite-only (the policy already
/// onboards, and admission reads the invitation as for anybody), when the
/// authenticator is already known (a returning or deleted person, whose
/// invitation admission reads, or whom the directory refuses), and when the
/// request names no invitation or tenant. An invitation that is spent,
/// expired, names a person, or is for another tenant is `None` too, and the
/// directory then gives the one refusal it gives any stranger.
async fn newcomer_invitation(
    auth: &SharedAuth,
    subject: &ordo_identity::AuthenticatedSubject,
    body: &Value,
    now: Timestamp,
) -> Result<Option<crate::admission::SpentInvitation>, Box<Response>> {
    let text = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::trim);
    let (Some(ticket), Some(space)) = (text("invitation"), text("space")) else {
        return Ok(None);
    };
    let known = {
        let dir = auth.lock_directory();
        if dir.provisioning() != ordo_identity::Provisioning::InviteOnly {
            return Ok(None);
        }
        dir.principal_for(&subject.authenticator)
    };
    match known {
        Ok(Some(_)) => return Ok(None),
        Ok(None) => {}
        Err(e) => {
            auth.record_refusal("auth.complete", "directory-unavailable", now);
            return Err(Box::new(StoreFailure::from(e).into_response()));
        }
    }
    let Some(admission) = auth.enrolment().and_then(|e| e.admission.clone()) else {
        return Ok(None);
    };
    match admission.take_newcomer_invitation(space, ticket).await {
        Ok(Some(spent)) => Ok(Some(spent)),
        Ok(None) => {
            auth.record_refusal("auth.complete", "invitation-refused", now);
            Ok(None)
        }
        Err(e) => {
            auth.record_refusal("auth.complete", "admission-unavailable", now);
            Err(Box::new(StoreFailure::from(e).into_response()))
        }
    }
}

/// Give back the invitation [`newcomer_invitation`] spent, when the newcomer
/// was not minted, their key not stored, or they were not admitted. Undoes
/// that spend only. Nothing to do when nothing was spent. A give-back that
/// fails is in the stream; the answer is whatever the failure already was.
fn give_back(
    auth: &AuthState,
    body: &Value,
    spent: Option<&crate::admission::SpentInvitation>,
    now: Timestamp,
) {
    let Some(spent) = spent else {
        return;
    };
    let Some(ticket) = body
        .get("invitation")
        .and_then(|v| v.as_str())
        .map(str::trim)
    else {
        return;
    };
    let Some(admission) = auth.enrolment().and_then(|e| e.admission.clone()) else {
        return;
    };
    match admission.give_back_invitation(ticket, spent) {
        Ok(true) => auth.record_refusal("auth.complete", "invitation-given-back", now),
        Ok(false) => {}
        Err(_) => auth.record_refusal("auth.complete", "invitation-not-given-back", now),
    }
}

/// Admit an invited newcomer at the role their spent invitation named.
async fn admit_invited(
    auth: &AuthState,
    principal: &str,
    space: &str,
    role: &str,
    now: Timestamp,
) -> &'static str {
    use crate::admission::Outcome;
    let Some(admission) = auth.enrolment().and_then(|e| e.admission.clone()) else {
        auth.record_refusal("auth.complete", "admission-not-wired", now);
        return "refused";
    };
    match admission.admit_invited(principal, space, role).await {
        Ok(Outcome::Admitted { .. }) => "admitted",
        Ok(Outcome::AlreadyMember { .. }) => "already-member",
        Ok(Outcome::Refused) => "refused",
        Err(_) => {
            auth.record_refusal("auth.complete", "admission-unavailable", now);
            "refused"
        }
    }
}

/// What a sign-in says about its arrival at a tenant. A refused arrival
/// that still carried a claim code is told the code is no longer read and
/// which setting admits a first-comer: the same words whatever the
/// tenant's policy, so they disclose nothing about it (owner, 2026-10-30).
pub fn admission_answer(space: &str, outcome: &str, body: &Value) -> Value {
    let mut a = json!({"space": space, "outcome": outcome});
    let carried_code = body
        .get("claim_code")
        .and_then(|v| v.as_str())
        .is_some_and(|c| !c.trim().is_empty());
    if outcome == "refused" && carried_code {
        a["message"] = json!(crate::admission::CLAIM_CODE_GONE);
        a["setting"] = json!(crate::admission::FIRST_COMER_SETTING);
    }
    a
}

/// Run the tenant's admission for a person who has just signed in.
///
/// Every failure is `refused`, and the stream keeps the reason: a person who
/// is signed in is not owed an account of which tenants exist.
async fn arrive(
    auth: &AuthState,
    principal: &str,
    space: &str,
    body: &Value,
    provider: Option<crate::admission::Via<'_>>,
    now: Timestamp,
) -> &'static str {
    use crate::admission::{Arrival, Outcome};
    let Some(admission) = auth.enrolment().and_then(|e| e.admission.clone()) else {
        auth.record_refusal("auth.complete", "admission-not-wired", now);
        return "refused";
    };
    let text = |k: &str| body.get(k).and_then(|v| v.as_str());
    let arrival = Arrival {
        ticket: text("invitation"),
        provider,
    };
    match admission.on_arrival(principal, space, &arrival).await {
        Ok(Outcome::Admitted { .. }) => "admitted",
        Ok(Outcome::AlreadyMember { .. }) => "already-member",
        Ok(Outcome::Refused) => "refused",
        Err(_) => {
            auth.record_refusal("auth.complete", "admission-unavailable", now);
            "refused"
        }
    }
}

/// The password was right and an administrator typed it: the person must
/// choose their own before they are given a session.
fn password_change_required() -> Response {
    EdgeError::new(
        EdgeCode::PasswordChangeRequired,
        "choose a new password with POST /auth/password/change, then sign in again",
    )
    .into_response()
}

/// Redeem a ticket for a first password: `POST /auth/password/redeem`
/// `{"ticket", "identifier", "password"}`.
///
/// This is how a person an administrator created (`std.space.createPerson`,
/// `std.space.resetCredential`) gets a password: the administrator hands
/// them a ticket and never a password. No token is needed and none is
/// returned; the person then signs in through `/auth/begin` and
/// `/auth/complete` as anyone does.
///
/// A ticket that is unknown, spent, expired, malformed or for something else
/// is the one refusal. An identifier that is already taken is told to the
/// holder of a valid ticket only, and does not spend it.
pub async fn password_redeem(
    State(auth): State<Option<SharedAuth>>,
    Json(body): Json<Value>,
) -> Response {
    use crate::credential_ops::RedeemError;
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(credentials) = auth.enrolment().and_then(|e| e.credentials.clone()) else {
        auth.record_refusal("auth.password.redeem", "not-wired", now);
        return refused();
    };
    let text = |k: &str| body.get(k).and_then(|v| v.as_str());
    let (Some(ticket), Some(identifier), Some(password)) =
        (text("ticket"), text("identifier"), text("password"))
    else {
        auth.record_refusal("auth.password.redeem", "malformed-request", now);
        return refused();
    };
    match credentials
        .redeem_password(ticket, identifier, password)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(RedeemError::IdentifierTaken) => {
            auth.record_refusal("auth.password.redeem", "identifier-taken", now);
            EdgeError::new(
                EdgeCode::IdentifierTaken,
                "that identifier is taken; choose another, the ticket is not spent",
            )
            .into_response()
        }
        Err(RedeemError::Refused) => {
            auth.record_refusal("auth.password.redeem", "ticket-refused", now);
            refused()
        }
        Err(RedeemError::Unavailable(why)) => {
            auth.record_refusal("auth.password.redeem", "store-unavailable", now);
            StoreFailure::from(why).into_response()
        }
    }
}

/// Choose a new password: `POST /auth/password/change`
/// `{"identifier", "password", "new_password"}`. What a temporary password's
/// forced change is, and what any person may do who knows their password.
///
/// The current password proves who is asking, so no token is needed, and
/// none is returned: every session the person had ends, and they sign in
/// again with the new one. Everything wrong is the one refusal.
pub async fn password_change(
    State(auth): State<Option<SharedAuth>>,
    Json(body): Json<Value>,
) -> Response {
    use crate::credential_ops::RedeemError;
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(credentials) = auth.enrolment().and_then(|e| e.credentials.clone()) else {
        auth.record_refusal("auth.password.change", "not-wired", now);
        return refused();
    };
    let text = |k: &str| body.get(k).and_then(|v| v.as_str());
    let (Some(identifier), Some(current), Some(new)) =
        (text("identifier"), text("password"), text("new_password"))
    else {
        auth.record_refusal("auth.password.change", "malformed-request", now);
        return refused();
    };
    match credentials.change_password(identifier, current, new).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(RedeemError::Unavailable(why)) => {
            auth.record_refusal("auth.password.change", "store-unavailable", now);
            StoreFailure::from(why).into_response()
        }
        Err(_) => {
            auth.record_refusal("auth.password.change", "bad-credentials", now);
            refused()
        }
    }
}

/// End this session: `POST /auth/logout` with the session's own bearer
/// token.
///
/// # Holding the token is the whole authorization
///
/// Nothing else is asked for and nothing else is consulted. Being able to
/// present the token **is** being the session, so a session can always end
/// itself, and a directory lookup here would mean a principal whose
/// membership changed could no longer log out, which is the one moment it
/// matters most.
///
/// # One answer, again
///
/// A token that does not verify, a token that verifies and names no
/// session, and a token that was already logged out are the same `204`. A
/// caller learns nothing from the shape of the response about a string they
/// presented, which is the same rule `/auth/complete` follows: a distinct
/// answer for "that was a real token" is an oracle for whoever is holding
/// a stolen one and wondering whether it is still worth anything.
///
/// A **failed write** is the exception and is a `503`. A logout that could
/// not be recorded must not be reported as done: the client discards the
/// token and believes it is safe while the door goes on honouring it.
pub async fn logout(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let Some(revocations) = auth.revocations.clone() else {
        // No deny list means no logout. Saying so is better than a 204 the
        // deployment cannot honour.
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(bearer) = bearer(&headers) else {
        auth.record_refusal("auth.logout", "no-token", now);
        return ended();
    };
    let Ok(claims) = ordo_identity::verify(&auth.token_key, &bearer, now) else {
        auth.record_refusal("auth.logout", "token-unverified", now);
        return ended();
    };
    // The deny-list write is a database round trip behind a synchronous
    // trait, so it runs on a blocking thread rather than on an axum worker.
    let max = auth.session_max_secs;
    let result = tokio::task::spawn_blocking(move || {
        ordo_identity::logout(revocations.as_ref(), &claims, max)
    })
    .await;
    match result {
        // Ended. One answer. The renewal cookie goes too: the deny list
        // ended its family ([`crate::renewal::RenewingRevocations`]), and
        // the browser need not keep a secret nothing honours.
        Ok(Ok(_)) => with_cookie(ended(), Some(crate::renewal::clear_cookie(&headers))),
        // There was no session to end. Same answer, and the stream says so.
        Ok(Err(ordo_identity::LifetimeError::NotASession)) => {
            auth.record_refusal("auth.logout", "not-a-session", now);
            ended()
        }
        // The write failed, or the blocking task did. Neither may be
        // reported as a logout.
        _ => {
            auth.record_refusal("auth.logout", "session-store-unavailable", now);
            store_unavailable()
        }
    }
}

/// End every session this person has: `POST /auth/logout-all` with a
/// session's own bearer token.
///
/// A stolen device that is still signed in is ended by the owner from
/// another one. The same call is what a recovery makes
/// (`plan-passkey-recovery.md` §4.3). It writes the per-principal epoch
/// (`SessionRevocations::end_all`): every session that started at or before
/// now is refused by the door and by `refresh`, this one included, and a
/// sign-in after this second starts a session that is not touched.
///
/// The token must be a session's, live and not ended, and neither a
/// capability nor a delegation. Anything else is the one refusal. A write
/// that failed is a 503, never a 204, for the reason `logout` gives.
pub async fn logout_all(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let Some(revocations) = auth.revocations.clone() else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let claims = match step_up_claims(&auth, &headers, "auth.logout-all", now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let until = Timestamp(now.0.saturating_add(auth.session_max_secs));
    let who = claims.principal.clone();
    let done = tokio::task::spawn_blocking(move || revocations.end_all(&who, now, until)).await;
    match done {
        Ok(Ok(())) => {
            auth.record(
                Some(&claims.principal),
                "auth.logout-all",
                ordo_secevent::EventKind::SessionsEnded {
                    reason: "logout-all".to_string(),
                },
                now,
            );
            ended()
        }
        _ => {
            auth.record_refusal("auth.logout-all", "session-store-unavailable", now);
            store_unavailable()
        }
    }
}

/// Continue this session: `POST /auth/refresh` with the session's own
/// bearer token, back with a new one.
///
/// # What it refuses, and why each refusal is the same answer
///
/// An expired token (checked by `verify`), a logged-out session, a session
/// past its ceiling, and a token that is not a session at all are one
/// `401`. They are distinguishable to the holder by other means, they know
/// when they logged in, and distinguishing them here would tell whoever
/// holds a stolen token *why* it stopped working, which is the difference
/// between "give up" and "the owner logged out, so try the password".
///
/// # What it does not do
///
/// It does not consult the directory, so it does not re-check membership: a
/// suspended principal can refresh, and every request they then make is
/// denied by the door, which re-reads membership on every check. Putting
/// the check here as well would be a second place for the answer to live
/// and a slower refresh for no gain.
///
/// It does not rotate the session id. See `ordo_identity::session::refresh`
/// for that argument, which is the owner's to settle.
pub async fn refresh(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let Some(revocations) = auth.revocations.clone() else {
        return unconfigured();
    };
    let now_secs = (auth.now)();
    let now = Timestamp(now_secs);
    let Some(bearer) = bearer(&headers) else {
        // No token: the page has just loaded. A renewal cookie, when the
        // browser sent one, continues the sign-in it came from.
        if let Some(secret) = crate::renewal::cookie_of(&headers) {
            return resume(&auth, revocations.as_ref(), &headers, &secret, now).await;
        }
        auth.record_refusal("auth.refresh", "no-token", now);
        return refused();
    };
    let Ok(claims) = ordo_identity::verify(&auth.token_key, &bearer, now) else {
        auth.record_refusal("auth.refresh", "token-unverified", now);
        return refused();
    };
    // A console key session is renewed only by signing again, so a revoked
    // key stops at its next sign-in (`plan-console.md` 8.2).
    if console::is_console_session(&claims) {
        auth.record_refusal("auth.refresh", "console-session-not-renewable", now);
        return refused();
    }
    // The lifetimes in force now for the tenant this session is for, so a
    // tenant's change reaches its sessions at their next refresh
    // ([`crate::session_policy`]).
    let space = match (&auth.policy, claims.session.as_ref()) {
        (Some(p), Some(x)) => p.session_space(&x.id.0).await,
        _ => None,
    };
    let eff = auth.effective(space.as_deref()).await;
    // The ceiling, or the overall limit when that is shorter. A resumed
    // session's start is set so this lands on the overall end (`resume`).
    let ceiling_secs = eff.ceiling_secs();
    let Ok(spec) = ordo_identity::refresh(
        revocations.as_ref(),
        &claims,
        now,
        eff.token_secs,
        ceiling_secs,
    ) else {
        auth.record_refusal("auth.refresh", "refresh-refused", now);
        return refused();
    };
    let expires_at = spec.expires.0;
    let session_expires_at = spec
        .session
        .as_ref()
        .map(|x| x.ceiling(ceiling_secs).0)
        .unwrap_or(expires_at);
    let token = token::mint(&auth.token_key, &spec);
    let mut answer = json!({
        "token": token,
        "expires_at": expires_at,
        "session_expires_at": session_expires_at,
    });
    // A manager whose page stays open is told at the next refresh, as at a
    // sign-in, of each open case about an owner of a tenant they manage.
    if let Some(recovery) = auth.recovery().cloned() {
        let who = claims.principal.0.clone();
        if let Ok(Ok(cases)) =
            tokio::task::spawn_blocking(move || recovery.manager_notices(&who)).await
        {
            if !cases.is_empty() {
                answer["owner_cases"] = crate::recovery::manager_notices_json(&cases);
            }
        }
    }
    (StatusCode::OK, Json(answer)).into_response()
}

/// `POST /auth/refresh` with a renewal cookie and no bearer token: the page
/// has loaded, and the browser still holds the sign-in ([`crate::renewal`]).
///
/// The secret is spent and a new one set. The token minted carries the
/// family's session id, so a later sign-out of it ends the whole family. It
/// carries no presence: nobody touched anything. An unknown, idle or reused
/// secret is the one refusal, and the cookie is cleared. Renewals switched
/// off is a refusal that names the setting, because the app developer can
/// fix it and the answer says nothing about any session.
async fn resume(
    auth: &SharedAuth,
    revocations: &dyn ordo_identity::session::SessionRevocations,
    headers: &axum::http::HeaderMap,
    secret: &str,
    now: Timestamp,
) -> Response {
    let remember = match auth.remember.clone() {
        Some(r) if r.config.on && auth.policy.as_ref().is_none_or(|p| p.deployment.remember) => r,
        _ => {
            auth.record_refusal("auth.refresh", "remember-off", now);
            return with_cookie(
                EdgeError::new(
                    EdgeCode::RememberOff,
                    format!(
                        "this deployment does not keep people signed in across a reload; \
                         the deployment owner sets {}=on to allow it",
                        crate::renewal::SETTING
                    ),
                )
                .into_response(),
                Some(crate::renewal::clear_cookie(headers)),
            );
        }
    };
    // Only an app's own page redeems the cookie (board
    // `blob-served-as-claimed-type`). Script on any other origin, the
    // door's own included, is refused before the secret is touched, and the
    // cookie is left as it is: clearing it here would let any page sign a
    // person out.
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if !remember.origin_allowed(origin) {
        auth.record_refusal("auth.refresh", "renewal-origin-not-allowed", now);
        return EdgeError::new(
            EdgeCode::RememberOff,
            format!(
                "this page's origin may not resume a sign-in; the deployment owner lists \
                 the app's origin in {}",
                crate::renewal::ORIGINS_SETTING
            ),
        )
        .into_response();
    }
    let gone = |reason: &str| {
        auth.record_refusal("auth.refresh", reason, now);
        with_cookie(refused(), Some(crate::renewal::clear_cookie(headers)))
    };
    let next = crate::renewal::mint_secret();
    let (hash, next_hash) = (
        crate::renewal::hash_of(secret),
        crate::renewal::hash_of(&next),
    );
    // The idle lifetime the next secret gets is the deployment's here; the
    // tenant's own is applied below, once the family names its tenant.
    let idle = remember.config.idle_secs;
    let store = remember.store.clone();
    let turned =
        tokio::task::spawn_blocking(move || store.turn(&hash, &next_hash, now.0, idle)).await;
    let (family, fresh) = match turned {
        Ok(Ok(crate::renewal::Turn::Fresh(f))) => (f, true),
        Ok(Ok(crate::renewal::Turn::Grace(f))) => (f, false),
        Ok(Ok(crate::renewal::Turn::Reused(f))) => {
            // Two browsers hold this sign-in. End it everywhere it reaches.
            let id = ordo_identity::SessionId(f.family.clone());
            let until = Timestamp(now.0.saturating_add(auth.session_max_secs));
            let _ = revocations.revoke(&id, until);
            auth.record(
                Some(&ordo_identity::PrincipalId(f.principal.clone())),
                "auth.refresh",
                ordo_secevent::EventKind::SessionsEnded {
                    reason: "renewal-reused".to_string(),
                },
                now,
            );
            return gone("renewal-reused");
        }
        Ok(Ok(crate::renewal::Turn::Unknown)) => return gone("renewal-unknown"),
        Ok(Err(e)) => {
            auth.record_refusal("auth.refresh", "renewal-store-unavailable", now);
            return store_unavailable_renewal(StoreFailure::from(e));
        }
        Err(_) => {
            auth.record_refusal("auth.refresh", "renewal-store-unavailable", now);
            return store_unavailable_renewal(StoreFailure::task_failed());
        }
    };
    let principal = ordo_identity::PrincipalId(family.principal.clone());
    let id = ordo_identity::SessionId(family.family.clone());
    if revocations.is_revoked(&id)
        || revocations.principal_ended(&principal, Timestamp(family.signed_in_at))
    {
        return gone("renewal-session-ended");
    }
    // The tenant's values. A tenant that turned renewals off ends them here.
    let eff = auth.effective(family.space.as_deref()).await;
    if !eff.remember {
        let store = remember.store.clone();
        let f = family.family.clone();
        let _ = tokio::task::spawn_blocking(move || store.end_family(&f)).await;
        auth.record_refusal("auth.refresh", "remember-off-for-tenant", now);
        return with_cookie(
            EdgeError::new(
                EdgeCode::RememberOff,
                format!(
                    "this tenant does not keep people signed in across a reload; its \
                     owner sets remember=on with {}",
                    crate::session_policy::SET_TENANT
                ),
            )
            .into_response(),
            Some(crate::renewal::clear_cookie(headers)),
        );
    }
    // **The overall limit** (owner, 2026-10-22): N seconds after the
    // original sign-in, sign in again however often it was used. The
    // family carries that time through every rotation.
    let overall_end = eff.overall_end(family.signed_in_at);
    if overall_end.is_some_and(|end| now.0 >= end) {
        let store = remember.store.clone();
        let f = family.family.clone();
        let _ = tokio::task::spawn_blocking(move || store.end_family(&f)).await;
        return gone("renewal-overall-limit");
    }
    let left = overall_end.map_or(u64::MAX, |end| end - now.0);
    let new_idle = eff.remember_secs.min(left);
    if fresh && new_idle != idle {
        // The secret just stored lives the tenant's idle lifetime, not the
        // deployment's, and never past the overall limit: store it again
        // under the same hash.
        let store = remember.store.clone();
        let (f, h) = (family.clone(), crate::renewal::hash_of(&next));
        let until = now.0.saturating_add(new_idle);
        let _ = tokio::task::spawn_blocking(move || {
            store.end_hash(&h)?;
            store.issue(&f, &h, until)
        })
        .await;
    }
    let idle = new_idle;
    // The session's start: now, or earlier when the overall limit would
    // otherwise fall inside its ceiling, so every refresh of it stops at the
    // overall end (`refresh` uses `ceiling_secs`). Never before the sign-in.
    let ceiling_secs = eff.ceiling_secs();
    let started_at = match overall_end {
        Some(end) => now
            .0
            .min(end.saturating_sub(ceiling_secs))
            .max(family.signed_in_at),
        None => now.0,
    };
    let session = ordo_identity::SessionClaim {
        id,
        started_at: Timestamp(started_at),
        second_step: family
            .second_step
            .as_deref()
            .and_then(ordo_identity::SecondStepKind::parse),
        via: family.via.clone(),
        upstream_presence: None,
    };
    let session_end = started_at.saturating_add(ceiling_secs);
    let expires_at = now.0.saturating_add(eff.token_secs).min(session_end);
    let spec = ordo_identity::TokenSpec {
        principal: principal.clone(),
        expires: Timestamp(expires_at),
        client: None,
        user_presence_at: None,
        capability: None,
        delegation: None,
        session: Some(session),
    };
    let token = token::mint(&auth.token_key, &spec);
    auth.record(
        Some(&principal),
        "auth.refresh",
        ordo_secevent::EventKind::Authenticated,
        now,
    );
    let mut answer = json!({
        "token": token,
        "expires_at": expires_at,
        "session_expires_at": session_end,
        "resumed": true,
    });
    if let Some(via) = &family.via {
        answer["via"] = json!(via);
    }
    if let Some(space) = &family.space {
        answer["space"] = json!(space);
        if let Some(policy) = &auth.policy {
            policy
                .note_session(&family.family, space, session_end)
                .await;
        }
    }
    let cookie = fresh.then(|| crate::renewal::set_cookie(headers, &next, idle));
    with_cookie(
        (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(answer),
        )
            .into_response(),
        cookie,
    )
}

/// The renewal store could not be read. Says nothing about the cookie.
fn store_unavailable_renewal(failure: StoreFailure) -> Response {
    failure.answer(
        ordo_api::EdgeCode::SessionStoreUnavailable,
        "the sign-in on this device cannot be checked right now; try again",
    )
}

/// `GET /.well-known/webauthn`: the related origins, for a browser deciding
/// whether a page on another domain may use this relying party's passkeys.
///
/// **This answers only when the deployment is reachable on the relying
/// party's own host.** The browser fetches `https://<rp id>/.well-known/webauthn`,
/// and the relying party's domain is usually the marketing site, not the
/// API. A deployment whose door is elsewhere has its owner publish the
/// document at the RP id's host (the same text this route answers, from
/// `RelyingParty::well_known_document`), or route that one path to the door.
/// With no related origins, and with no passkey, there is nothing to say and
/// the answer is a 404.
pub async fn well_known_webauthn(State(auth): State<Option<SharedAuth>>) -> Response {
    let doc = auth
        .and_then(|a| a.passkey.clone())
        .and_then(|p| p.relying_party().well_known_document());
    match doc {
        Some(body) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response(),
        None => ordo_api::error::no_document(),
    }
}

/// The one answer `/auth/logout` gives. No body: there is nothing to say
/// that the caller does not already know, and anything said here would be
/// something to compare against.
fn ended() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// The deny list could not be written. Distinct from a refusal on purpose,
/// and it discloses nothing: it does not depend on the token.
fn store_unavailable() -> Response {
    EdgeError::new(
        EdgeCode::SessionStoreUnavailable,
        "the logout could not be recorded; this session is still live",
    )
    .into_response()
}

/// The bearer token on a request, if there is one.
fn bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

/// Read a [`CompleteRequest`] out of JSON.
///
/// Byte strings arrive base64url, and `client_data_json` in particular is
/// **signed as bytes**, so it is decoded once here and passed through
/// untouched. Re-encoding it anywhere on the way in would break the
/// verification it exists for.
fn parse_complete(body: &Value) -> Option<CompleteRequest> {
    let s = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let b = |k: &str| body.get(k).and_then(|v| v.as_str()).and_then(b64);
    let challenge = || s("challenge").map(ordo_identity::ChallengeId);
    match body.get("kind").and_then(|k| k.as_str())? {
        "password" => Some(CompleteRequest::Password {
            challenge: challenge()?,
            identifier: s("identifier")?,
            password: s("password")?,
        }),
        "passkey_registration" => Some(CompleteRequest::PasskeyRegistration {
            challenge: challenge()?,
            client_data_json: b("client_data_json")?,
            attestation_object: b("attestation_object")?,
        }),
        "passkey_assertion" => Some(CompleteRequest::PasskeyAssertion {
            challenge: challenge()?,
            credential_id: b("credential_id")?,
            client_data_json: b("client_data_json")?,
            authenticator_data: b("authenticator_data")?,
            signature: b("signature")?,
        }),
        // What a browser brings back, and nothing about who the person is:
        // a `subject` or `nonce` in the body is not read (§0.3 item 3).
        "oidc_callback" => Some(CompleteRequest::OidcCallback {
            state: s("state")?,
            code: s("code")?,
            iss: s("iss"),
        }),
        // A redirect method's handoff, with the binder `begin` gave the page.
        "handoff" => Some(CompleteRequest::Handoff {
            handoff: s("handoff")?,
            binder: s("binder")?,
        }),
        // An emailed code, with the binder `begin` gave the page. The
        // address is not read from the body: the ceremony holds it.
        "email_code" => Some(CompleteRequest::EmailCode {
            challenge: challenge()?,
            binder: s("binder")?,
            code: s("code")?,
        }),
        // A guest coming back with its secret (§7A). A new guest is never
        // parsed from a body: `guest::new_guest` mints its secret.
        "resume" => Some(CompleteRequest::GuestResume {
            secret: s("secret")?,
        }),
        _ => None,
    }
}

/// base64url without padding, which is what WebAuthn puts on the wire.
fn b64(s: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = T.iter().position(|t| *t == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Shared so a deployment can hand the same `Arc` to the router.
pub type SharedAuth = Arc<AuthState>;

/// Every route under `/auth` and the WebAuthn well-known document, once.
///
/// Both the in-process [`router`] and the served `serve::platform_router`
/// call this, so a route added here is on both (defect 25ax: routes mounted on
/// one list and not the other answered 404 from the real binary).
///
/// Every route here sits under one layer, [`console::gate`], which keeps a
/// console key session to the relayed touch and its logout. `auth` is the
/// state that layer reads; it is the same the routes are given.
pub fn auth_routes<S>(auth: Option<SharedAuth>) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
    Option<SharedAuth>: FromRef<S>,
{
    axum::Router::new()
        .route("/auth/methods", axum::routing::get(methods::list))
        .route("/auth/begin", axum::routing::post(begin))
        .route("/auth/complete", axum::routing::post(complete))
        .route("/auth/logout", axum::routing::post(logout))
        .route("/auth/logout-all", axum::routing::post(logout_all))
        .route("/auth/refresh", axum::routing::post(refresh))
        .route("/auth/register", axum::routing::post(register))
        .route(
            "/auth/password/redeem",
            axum::routing::post(password_redeem),
        )
        .route(
            "/auth/password/change",
            axum::routing::post(password_change),
        )
        .route(
            "/auth/delegation/request",
            axum::routing::post(delegate::request),
        )
        .route(
            "/auth/delegation/redeem",
            axum::routing::post(delegate::redeem),
        )
        .route("/auth/step-up/begin", axum::routing::post(step_up_begin))
        .route("/auth/step-up", axum::routing::post(step_up))
        .route(
            "/auth/recovery-codes",
            axum::routing::post(recover::issue_codes),
        )
        .route(
            "/auth/recover/code",
            axum::routing::post(recover::redeem_code),
        )
        .route("/auth/recover/begin", axum::routing::post(recover::begin))
        .route(
            "/auth/recover/complete",
            axum::routing::post(recover::complete),
        )
        .route(
            "/auth/recover/contest",
            axum::routing::post(recover::contest),
        )
        .route(
            "/auth/recover/ack",
            axum::routing::post(recover::acknowledge),
        )
        .route(
            "/auth/authenticators",
            axum::routing::get(authenticators::list),
        )
        .route(
            "/auth/authenticators/link/begin",
            axum::routing::post(authenticators::link_begin),
        )
        .route(
            "/auth/authenticators/link",
            axum::routing::post(authenticators::link),
        )
        .route(
            "/auth/authenticators/unlink",
            axum::routing::post(authenticators::unlink),
        )
        .route(
            "/.well-known/webauthn",
            axum::routing::get(well_known_webauthn),
        )
        .route("/auth/spaces", axum::routing::get(spaces::list))
        .route(
            "/auth/provisioning",
            axum::routing::get(signup::provisioning).post(signup::retry_provisioning),
        )
        .route("/auth/account/delete", axum::routing::post(account::delete))
        .route("/auth/share/peek", axum::routing::post(share::peek))
        .route("/auth/share/accept", axum::routing::post(share::accept))
        .route("/auth/window", axum::routing::post(share::window))
        .route("/auth/share/leave", axum::routing::post(share::leave))
        .route(
            "/auth/share/decline",
            axum::routing::post(decline::share_decline),
        )
        .route(
            "/auth/invitation/peek",
            axum::routing::post(decline::invitation_peek),
        )
        .route(
            "/auth/invitation/decline",
            axum::routing::post(decline::invitation_decline),
        )
        .route("/auth/link", axum::routing::post(link::open))
        .route(
            "/auth/callback/{connection}",
            axum::routing::get(federated::callback),
        )
        .route(
            "/auth/callback/t/{tenant}/{connection}",
            axum::routing::get(tenant::callback),
        )
        .route(
            "/auth/identities",
            axum::routing::get(federated::identities),
        )
        .merge(second_step::routes())
        .merge(handoff::routes())
        .merge(claim::routes())
        .merge(contact::routes())
        // Last, so it covers every route above.
        .route_layer(axum::middleware::from_fn_with_state(auth, console::gate))
}

/// A router carrying only the auth routes.
///
/// Public because a test exercises them without building a pool and a
/// deployment.
pub fn router(auth: Option<SharedAuth>) -> axum::Router {
    auth_routes(auth.clone()).with_state(auth)
}

#[cfg(test)]
mod reason_tests {
    use super::auth_error_reason;

    /// Audit 34 (2026-10-09): the security stream's reason for a return
    /// address outside the list names the setting that holds the list.
    #[test]
    fn a_return_address_outside_the_list_names_its_setting_in_the_stream() {
        let reason = auth_error_reason(&ordo_identity::AuthError::ReturnNotAllowed);
        assert!(reason.starts_with("return-not-allowed"), "{reason}");
        assert!(reason.contains("ORDO_AUTH_RETURN_URLS"), "{reason}");
    }
}
