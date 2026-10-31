//! Pluggable authentication. Every login method — local password, email OTP,
//! passkey, federated OIDC/SAML/OAuth2 — sits behind [`AuthProvider`], so an
//! IdP is a *configuration*, never a fork. A provider's only job is to turn a
//! login ceremony into an [`AuthenticatedSubject`]; everything after that
//! instant (directory resolution, session, tokens) is core and method-blind.
//!
//! The trait is deliberately synchronous, and no socket exists in this crate.
//! A provider that has to reach an outside identity provider does it through
//! [`crate::federation::FederationTransport`], which `ordo-host` implements
//! over the egress kernel, so every verifier here stays a function of bytes,
//! keys and an instant. It is, however, **clock-taking**: both halves receive the server's `now`
//! rather than reading one. Every other decision in this crate is a pure
//! function of an explicit instant, and a ceremony is the one place where a
//! missing clock is not merely untestable but wrong — a challenge with no
//! notion of time never expires, and a `state` minted a year ago still opens
//! a session (see [`CEREMONY_TTL_SECS`]).
//!
//! Two implementations live here: [`LocalPasswordProvider`] (credential verify
//! behind a [`PasswordHasher`], [`Argon2idHasher`] in production) and, in this
//! crate's own tests only, `StubOidcProvider` (the redirect/callback dance with
//! real `state`/`nonce` semantics, no network). Outside providers are
//! [`crate::oauth2::OAuth2Provider`]. The third, [`crate::passkey::PasskeyProvider`], has
//! a module of its own because it is the only one that can mint user presence
//! — and keeping that constructor behind a module boundary is what makes
//! "presence comes only from passkeys" a fact about the code rather than a
//! rule somebody enforces.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::credentials::{CredentialStore, CredentialStoreError, MemoryCredentialStore};
use crate::passkey::UserPresence;
use crate::principal::{AuthMethodKind, Timestamp};
use crate::webauthn::WebAuthnError;

/// How long any provider's one-shot ceremony handle stays answerable —
/// challenge, OIDC `state`, SAML request id alike.
///
/// Five minutes, the same window as
/// [`crate::access::DEFAULT_STEP_UP_MAX_AGE_SECS`]. Before the trait took a
/// clock these maps had no expiry at all: they grew for the life of the
/// process, and a `state` value observed once (a referrer header, a proxy log,
/// a shoulder) stayed redeemable forever. One-shot was enforced; one-*time*
/// was not.
pub const CEREMONY_TTL_SECS: u64 = 300;

/// The setting that caps how many password challenges may wait at once.
pub const PASSWORD_PENDING_MAX_ENV: &str = "ORDO_AUTH_PASSWORD_PENDING_MAX";

/// Its default: password challenges one process holds unanswered at once.
/// Each is about 60 bytes, so the cap bounds the map near a megabyte.
pub const PASSWORD_PENDING_MAX: usize = 10_000;

/// One-shot challenge handle minted by `begin`. Consumed by `complete`;
/// replay fails loudly.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChallengeId(pub String);

/// Stable identifier of one authenticator, the key the directory links to a
/// principal. Canonical spellings per method family:
/// `password:<identifier>`, `passkey:<credential-id>`,
/// `oidc:<method>:<subject>`, `saml:<method>:<name-id>`,
/// `oauth2:<connection>:<subject>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AuthenticatorId(pub String);

impl AuthenticatorId {
    /// Does this sign-in belong to an **outside provider**: an OIDC, SAML or
    /// OAuth2 connection whose identity provider holds the person's
    /// credential, and not Ordo? The spelling is the marker
    /// (`plan-auth-federated.md` section 3, `oidc:<connection>:<sub>`), and
    /// it is set where the authenticator is minted, by the provider that
    /// names it, so nothing a person or a tenant supplies can make a link
    /// look like one or stop looking like one. **Recovery leaves such a link
    /// alone** (owner, 2026-09-29: *"third party is managed elsewhere"*).
    pub fn is_outside_provider(&self) -> bool {
        [
            AuthMethodKind::Oidc,
            AuthMethodKind::Saml,
            AuthMethodKind::OAuth2,
        ]
        .iter()
        .any(|k| {
            self.0
                .strip_prefix(k.as_str())
                .is_some_and(|rest| rest.starts_with(':'))
        })
    }
}

impl AuthenticatorId {
    /// The `via` word for a session this authenticator began: the
    /// connection (`oauth2:discord`) for an outside provider, the method
    /// word otherwise.
    pub fn via(&self, method: AuthMethodKind) -> String {
        if self.is_outside_provider() {
            let mut parts = self.0.splitn(3, ':');
            if let (Some(kind), Some(conn)) = (parts.next(), parts.next()) {
                return format!("{kind}:{conn}");
            }
        }
        method.as_str().to_string()
    }
}

impl std::fmt::Display for AuthenticatorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Input to `begin`. The identifier hint is optional: federated flows need
/// none, and local flows issue a challenge whether or not the identifier
/// exists (anti-enumeration — see [`AuthError::BadCredentials`]).
///
/// `return_to` is where a redirect method sends the browser back to, with a
/// one-time handoff code, once the outside provider has answered
/// (`plan-auth-federated.md` §3.2). A provider matches it exactly against the
/// deployment's list and refuses anything else at `begin`, before a redirect
/// exists, so the callback can never be used as an open redirector. A method
/// that redirects nowhere ignores it.
///
/// There is deliberately no field for the caller's network address: no limit
/// reads one until proxy mode exists (owner, 2026-10-06).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BeginRequest {
    pub identifier: Option<String>,
    pub return_to: Option<String>,
    /// The page asks the provider for a fresh sign-in whose two-step claim
    /// may count as presence (`plan-auth-federated.md` §8.2). OIDC sends
    /// `max_age=0`. Every other method ignores it.
    pub presence: bool,
}

/// What `begin` hands back: a challenge for the client to answer in place
/// (password, OTP, WebAuthn) or a redirect to follow (OIDC/SAML/OAuth2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeOrRedirect {
    Challenge {
        id: ChallengeId,
        method: AuthMethodKind,
    },
    /// Navigate the browser to `url`. `binder` is 256 random bits the page
    /// keeps in memory and presents with the handoff at `/auth/complete`.
    /// It never travels in a URL, so a callback planted in somebody else's
    /// browser produces a handoff that browser cannot redeem (login CSRF).
    Redirect { url: String, binder: String },
}

/// Closed set of completion payloads. A provider handed the wrong variant
/// fails with [`AuthError::MethodMismatch`] — never a silent ignore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompleteRequest {
    /// Answer to a local-password challenge.
    Password {
        challenge: ChallengeId,
        identifier: String,
        password: String,
    },
    /// The OIDC redirect-URI callback: what the browser brings back, and
    /// nothing about who the person is. The subject and the nonce come from
    /// the ID token the provider fetches with `code`; a caller that could
    /// post them could sign in as anyone (`plan-auth-federated.md` §0.3 item
    /// 3). `iss` is the RFC 9207 response parameter, when the IdP sends one.
    OidcCallback {
        state: String,
        code: String,
        iss: Option<String>,
    },
    /// Redeem a one-time handoff written by a redirect method's callback
    /// (`plan-auth-federated.md` §3.2 step 3). `binder` is the value `begin`
    /// handed the page; without it the handoff is refused.
    Handoff { handoff: String, binder: String },
    /// An emailed sign-in code (`plan-auth-federated.md` §7.2 step 3):
    /// the challenge `begin` answered, the binder it handed the page, and
    /// the code the person read in the message. The address is not in the
    /// request: the server recovers it from the ceremony.
    EmailCode {
        challenge: ChallengeId,
        binder: String,
        code: String,
    },
    /// A guest coming back (`plan-auth-federated.md` §7A): the secret Ordo
    /// returned once when the guest was made. The authenticator is the
    /// SHA-256 of it, so the secret itself is never stored.
    GuestResume { secret: String },
    /// A new guest: the secret the host just minted for it. Never read from
    /// a request body; the provider refuses one the directory already holds.
    GuestNew { secret: String },
    /// A WebAuthn **registration** response, exactly as the browser produced
    /// it. The byte strings are opaque here and stay opaque all the way to
    /// [`crate::webauthn`]: `client_data_json` in particular is *signed as
    /// bytes*, so re-encoding it anywhere on the way in would break the
    /// verification it exists for.
    PasskeyRegistration {
        challenge: ChallengeId,
        client_data_json: Vec<u8>,
        attestation_object: Vec<u8>,
    },
    /// A WebAuthn **assertion** response — the sign-in and step-up ceremony.
    PasskeyAssertion {
        challenge: ChallengeId,
        credential_id: Vec<u8>,
        client_data_json: Vec<u8>,
        authenticator_data: Vec<u8>,
        signature: Vec<u8>,
    },
}

/// An email address an outside provider vouched for, and how it vouched.
///
/// **One constructor, and it takes the evidence.** An address a provider
/// only reported, without saying it verified it, has no type to travel in,
/// so it cannot reach an invitation match, a domain rule or a stored
/// profile by accident (`plan-auth-federated.md` O11, A5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEmail {
    address: String,
    evidence: EmailEvidence,
}

/// Why an address counts as verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmailEvidence {
    /// The provider's profile carried this claim as the JSON boolean `true`.
    ProviderClaim { claim: String },
    /// The person typed a code that was mailed to this address, in the
    /// browser that asked for it (`plan-auth-federated.md` §7). Ordo itself
    /// verified the address.
    EmailedCode,
}

impl VerifiedEmail {
    /// An address and the evidence that it was verified. Refuses an address
    /// with no `@`, or with whitespace or a control character in it.
    pub fn new(address: &str, evidence: EmailEvidence) -> Option<VerifiedEmail> {
        let address = address.trim();
        let ok = address.len() <= 320
            && address
                .split_once('@')
                .is_some_and(|(l, d)| !l.is_empty() && !d.is_empty())
            && !address.chars().any(|c| c.is_whitespace() || c.is_control());
        ok.then(|| VerifiedEmail {
            address: address.to_string(),
            evidence,
        })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    /// The part after the `@`, lowercased: what a domain rule compares.
    pub fn domain(&self) -> String {
        self.address
            .rsplit_once('@')
            .map(|(_, d)| d.to_ascii_lowercase())
            .unwrap_or_default()
    }

    pub fn evidence(&self) -> &EmailEvidence {
        &self.evidence
    }
}

/// What an outside provider said about how the person signed in there
/// (`amr`, `acr`, `auth_time`). Recorded for every sign-in. Nothing here is
/// presence: only [`crate::upstream::PresenceRule::judge`] reads it, when the
/// connection's setting and the sign-in asked (`plan-auth-federated.md`
/// §8.2, step 10).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpstreamAssurance {
    pub amr: Vec<String>,
    pub acr: Option<String>,
    pub auth_time: Option<u64>,
}

/// What a ceremony learned about the person besides who they are, from the
/// provider's verified answer and never from the request.
///
/// Empty for password and passkey. For an outside provider it is the profile
/// the connection keeps: the name shown, an avatar address, a verified email.
/// The display name a sign-in mints a principal with comes from here when it
/// is present (`plan-auth-federated.md` §0.3 item 5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubjectAttributes {
    pub email: Option<VerifiedEmail>,
    pub display: Option<String>,
    pub avatar: Option<String>,
    pub assurance: Option<UpstreamAssurance>,
}

/// A completed ceremony: who proved themselves, how, and — only from a
/// passkey — that a human was physically there. This is the entire
/// provider→core interface; no upstream token ever crosses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedSubject {
    pub authenticator: AuthenticatorId,
    pub method: AuthMethodKind,
    /// The profile the provider vouched for. See [`SubjectAttributes`].
    pub attributes: SubjectAttributes,
    /// The fresh user-presence assertion this ceremony produced, if it could
    /// produce one. **`None` for every method but passkey**, and that is not a
    /// policy this struct enforces by checking: [`UserPresence`] has no
    /// constructor outside [`crate::passkey`], so a password or OIDC provider
    /// has nothing it could put here.
    ///
    /// This field is what closes the loop `security-logging-design.md` §5
    /// describes and the crate could not previously walk: it is copied into
    /// [`crate::token::TokenSpec::user_presence_at`] at sign-in, verified back
    /// out as [`crate::access::TokenClaims::user_presence_at`], and is the
    /// only thing that can satisfy
    /// [`crate::access::StepUpRequirement::Presence`].
    pub presence: Option<UserPresence>,
    /// An outside provider's two-step claim that counted under the
    /// connection's setting (`crate::upstream`). Never a [`UserPresence`]:
    /// it travels in the session claim and satisfies step-up only where the
    /// access engine's [`crate::upstream::UpstreamAcceptance`] says so.
    pub upstream_presence: Option<crate::upstream::UpstreamPresence>,
}

impl AuthenticatedSubject {
    /// A subject from a method that proves identity but not presence.
    pub fn without_presence(
        authenticator: AuthenticatorId,
        method: AuthMethodKind,
    ) -> AuthenticatedSubject {
        AuthenticatedSubject {
            authenticator,
            method,
            attributes: SubjectAttributes::default(),
            presence: None,
            upstream_presence: None,
        }
    }

    /// The same subject, with what the provider vouched for about it.
    pub fn with_attributes(mut self, attributes: SubjectAttributes) -> AuthenticatedSubject {
        self.attributes = attributes;
        self
    }

    /// The same subject, with a provider's two-step claim that counted.
    pub fn with_upstream_presence(
        mut self,
        upstream: Option<crate::upstream::UpstreamPresence>,
    ) -> AuthenticatedSubject {
        self.upstream_presence = upstream;
        self
    }
}

/// Authentication failures. Loud and typed; deliberately *not* more specific
/// where specificity would leak (unknown user vs wrong password are the same
/// `BadCredentials`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthError {
    /// Challenge id unknown or already consumed (challenges are one-shot).
    UnknownChallenge(ChallengeId),
    /// Credential did not verify — covers unknown identifier too.
    BadCredentials,
    /// Callback `state` unknown or already consumed.
    StateMismatch(String),
    /// ID-token nonce does not match the one minted at `begin`.
    NonceMismatch,
    /// Callback arrived without an authorization code.
    EmptyAuthorizationCode,
    /// Callback carried no subject claim.
    MissingSubject,
    /// This provider was handed a completion payload for another method.
    MethodMismatch { provider: AuthMethodKind },
    /// A WebAuthn response was structurally or cryptographically wrong. Kept
    /// specific because none of these are enumeration oracles and every one of
    /// them means something different to an operator — see [`WebAuthnError`].
    WebAuthn(WebAuthnError),
    /// A registration presented a credential id already enrolled here.
    /// Refused rather than overwritten: replacing a live public key under an
    /// existing authenticator id would hand the account to whoever asked, with
    /// the directory link unchanged and nothing in the audit trail to see.
    CredentialAlreadyEnrolled,
    /// The credential store could not be read. **Not a refusal of the
    /// credential**: it is "ask again later", and it is safe to keep
    /// distinct because reachability of the store does not depend on which
    /// identifier was offered. See [`CredentialStoreError`].
    StoreUnavailable(CredentialStoreError),
    /// The outside provider could not be reached, or the ceremony could not
    /// be filed. The federated counterpart of [`AuthError::StoreUnavailable`],
    /// decided before any identity is known, so it says nothing about an
    /// account. The detail is a fixed word for the security stream, never a
    /// byte the provider sent.
    UpstreamUnavailable(&'static str),
    /// The provider answered, and its answer does not name a person this
    /// connection accepts: no subject, a subject of the wrong type, a token
    /// response without a bearer token. A fixed word for the stream.
    UpstreamRefused(&'static str),
    /// `return_to` is not on the deployment's list, or is missing for a
    /// method that redirects. Refused at `begin`, before any redirect exists.
    ReturnNotAllowed,
    /// Too many ceremonies are waiting to be completed, so no new one is
    /// begun. `setting` is the variable that raises the cap, `cap` its value
    /// in force. Decided before any identifier is read, so it says nothing
    /// about an account.
    TooManyPending { setting: &'static str, cap: usize },
}

impl From<WebAuthnError> for AuthError {
    fn from(e: WebAuthnError) -> AuthError {
        AuthError::WebAuthn(e)
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::UnknownChallenge(c) => {
                write!(f, "unknown or already-used challenge `{}`", c.0)
            }
            AuthError::BadCredentials => write!(f, "credentials did not verify"),
            AuthError::StateMismatch(s) => {
                write!(f, "unknown or already-used state `{s}`")
            }
            AuthError::NonceMismatch => write!(f, "nonce does not match this ceremony"),
            AuthError::EmptyAuthorizationCode => {
                write!(f, "callback carried no authorization code")
            }
            AuthError::MissingSubject => write!(f, "callback carried no subject"),
            AuthError::MethodMismatch { provider } => {
                write!(
                    f,
                    "completion payload is not for a {} provider",
                    provider.as_str()
                )
            }
            AuthError::WebAuthn(e) => write!(f, "{e}"),
            AuthError::CredentialAlreadyEnrolled => {
                f.write_str("that credential is already enrolled")
            }
            AuthError::StoreUnavailable(e) => write!(f, "{e}"),
            AuthError::UpstreamUnavailable(why) => {
                write!(f, "the identity provider could not be used: {why}")
            }
            AuthError::UpstreamRefused(why) => {
                write!(f, "the identity provider's answer was refused: {why}")
            }
            AuthError::ReturnNotAllowed => f.write_str("that return address is not allowed"),
            AuthError::TooManyPending { setting, cap } => write!(
                f,
                "{cap} sign-ins are already waiting to be completed; wait, or raise {setting}"
            ),
        }
    }
}

impl std::error::Error for AuthError {}

/// The provider trait. `begin` starts a ceremony; `complete` finishes it or
/// fails loudly. Synchronous: a provider that needs the network reaches it
/// through [`crate::federation::FederationTransport`].
///
/// `begin` can fail: a redirect method refuses a `return_to` the deployment
/// does not list, and a ceremony that could not be filed is a refusal rather
/// than a challenge nobody can complete.
///
/// `now` is the **server's** clock and is a parameter rather than a read so
/// that ceremony expiry is a property the caller can test, and so that no
/// implementation is tempted to take the instant from the payload it is
/// verifying.
/// `Send + Sync` because a provider is shared across requests: the door
/// holds one registry for the life of the process and every ceremony runs
/// on whatever thread the server hands it. A provider that could not cross
/// a request boundary could not be used by a server, which is the only
/// thing that uses one.
pub trait AuthProvider: Send + Sync {
    fn kind(&self) -> AuthMethodKind;
    fn begin(&self, req: &BeginRequest, now: Timestamp) -> Result<ChallengeOrRedirect, AuthError>;
    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError>;
}

// ─────────────────────────── password hashing ───────────────────────────

/// Credential hashing behind a trait — the same resolver pattern as
/// `KeyProvider`. [`Argon2idHasher`] is what a deployment gets; nothing else
/// in the crate knows or cares which algorithm is behind it.
/// `Send + Sync` for [`AuthProvider`]'s reason: a hasher lives inside a
/// provider, and a provider is shared across requests.
pub trait PasswordHasher: Send + Sync {
    fn hash(&self, password: &str) -> String;
    fn verify(&self, password: &str, hash: &str) -> bool;
}

/// Transparent hasher for tests ONLY.
///
/// **`#[cfg(test)]`, and that is the fix.** It used to be a public export of
/// this crate, which meant a deployment could wire the platform's password
/// path to a function that stores the password. Nothing prevented it, nothing
/// warned about it, and the resulting "hash" column would have been a
/// plaintext password column with a prefix. It is now unreachable outside this
/// crate's own tests, so the only hasher a deployment can name is a real one.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct PlainTestHasher;

#[cfg(test)]
impl PasswordHasher for PlainTestHasher {
    fn hash(&self, password: &str) -> String {
        format!("plain-test${password}")
    }

    fn verify(&self, password: &str, hash: &str) -> bool {
        hash.strip_prefix("plain-test$") == Some(password)
    }
}

/// argon2id, with the cost pinned here where a reader can find it.
///
/// # The parameters, and why these
///
/// | | value | |
/// |---|---|---|
/// | memory | **19 456 KiB** (19 MiB) | OWASP's second recommended argon2id profile |
/// | iterations | **2** | the `t` that pairs with 19 MiB in that profile |
/// | parallelism | **1** | a server hashes many passwords at once; lanes buy nothing and cost a thread |
/// | output | **32 bytes** | |
/// | salt | **16 bytes, per hash, from the OS CSPRNG** | |
///
/// These are the `m=19456, t=2, p=1` line of OWASP's Password Storage Cheat
/// Sheet — the profile chosen for a server that also has to serve requests.
/// The stronger `m=47104, t=1` line trades three times the memory for one
/// pass; at a login rate a platform can actually see, memory is the scarcer
/// resource.
///
/// # Why the parameters are not a constructor argument
///
/// A tunable cost is a cost somebody turns down. The stored PHC string
/// records the parameters used, so [`Argon2idHasher::verify`] keeps working
/// across a future change to these constants and old hashes verify at their
/// old cost — but there is exactly one place to make that change, and it is
/// visible in a diff. The one escape hatch,
/// [`Argon2idHasher::weak_for_tests`], is named so that its appearance
/// anywhere outside a test is self-reporting.
#[derive(Debug, Clone, Copy)]
pub struct Argon2idHasher {
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
}

/// OWASP argon2id profile: memory in KiB.
pub const ARGON2ID_M_COST_KIB: u32 = 19_456;
/// OWASP argon2id profile: iterations.
pub const ARGON2ID_T_COST: u32 = 2;
/// OWASP argon2id profile: lanes.
pub const ARGON2ID_P_COST: u32 = 1;
/// Derived key length, in bytes.
pub const ARGON2ID_OUTPUT_LEN: usize = 32;

impl Default for Argon2idHasher {
    fn default() -> Self {
        Argon2idHasher::new()
    }
}

impl Argon2idHasher {
    pub fn new() -> Argon2idHasher {
        Argon2idHasher {
            m_cost: ARGON2ID_M_COST_KIB,
            t_cost: ARGON2ID_T_COST,
            p_cost: ARGON2ID_P_COST,
        }
    }

    /// The minimum argon2 will accept, so a test suite that enrolls a
    /// credential per case does not spend 19 MiB and tens of milliseconds on
    /// each one.
    ///
    /// **Never in a deployment.** The name is the warning, and the PHC string
    /// it produces carries `m=8` in plain sight, so a hash made with it is
    /// identifiable on sight in a database dump.
    pub fn weak_for_tests() -> Argon2idHasher {
        Argon2idHasher {
            m_cost: 8,
            t_cost: 1,
            p_cost: 1,
        }
    }

    fn argon2(&self) -> argon2::Argon2<'static> {
        let params = argon2::Params::new(
            self.m_cost,
            self.t_cost,
            self.p_cost,
            Some(ARGON2ID_OUTPUT_LEN),
        )
        .expect("argon2 parameters are constants, validated where they are defined");
        argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
    }
}

impl PasswordHasher for Argon2idHasher {
    /// Returns a PHC string — self-describing, so the parameters travel with
    /// the hash and a later change to the constants above does not invalidate
    /// what is already stored.
    fn hash(&self, password: &str) -> String {
        use argon2::password_hash::PasswordHasher as _;
        let mut salt = [0u8; 16];
        // A predictable salt makes one rainbow table serve the whole
        // deployment; there is no degraded mode worth having here.
        getrandom::getrandom(&mut salt)
            .expect("the operating system CSPRNG is unavailable — refusing to hash a password");
        let salt = argon2::password_hash::SaltString::encode_b64(&salt)
            .expect("16 bytes is a valid salt length");
        self.argon2()
            .hash_password(password.as_bytes(), &salt)
            .expect("argon2id cannot fail for validated parameters and a valid salt")
            .to_string()
    }

    /// Verification reads the cost out of the stored string, so a credential
    /// enrolled under older parameters still verifies. A malformed stored hash
    /// is `false`, never a panic: a corrupt row must refuse a login, not take
    /// the process down.
    fn verify(&self, password: &str, hash: &str) -> bool {
        use argon2::password_hash::PasswordVerifier as _;
        let Ok(parsed) = argon2::password_hash::PasswordHash::new(hash) else {
            return false;
        };
        // `argon2` compares the derived bytes in constant time.
        self.argon2()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }
}

// ─────────────────────── local password provider ────────────────────────

/// Username/password behind [`PasswordHasher`], over a
/// [`CredentialStore`].
///
/// The store is the durability seam. This provider used to own the
/// credential table as a `BTreeMap` field, which made every enrolment a fact
/// about one process: a restart, a deploy or a crash and the deployment had
/// no accounts. It now holds a store and knows nothing about where the rows
/// live: [`MemoryCredentialStore`] in a test, `PgCredentialStore` in a
/// deployment.
///
/// Anti-enumeration: `begin` issues a challenge whether or not the
/// identifier exists, and unknown-identifier and wrong-password both fail as
/// [`AuthError::BadCredentials`]. **And they cost the same**, which is the
/// half that used to be missing, see [`LocalPasswordProvider::decoy`].
///
/// **It cannot prove presence.** A password is a secret, not a human in the
/// room; a stolen one is indistinguishable from a typed one. So its
/// [`AuthenticatedSubject::presence`] is `None`, every step-up op stays shut
/// to a password-only session, and that is the design's rule
/// (`security-logging-design.md` §2) rather than this provider's opinion.
pub struct LocalPasswordProvider<H: PasswordHasher> {
    hasher: H,
    /// Where identifier → hash actually lives.
    store: Arc<dyn CredentialStore>,
    /// A hash of a password nobody has, verified against when the
    /// identifier is unknown.
    ///
    /// **This closes a timing oracle the single refusal did not.** Before
    /// it, an unknown identifier returned in microseconds and a known one
    /// paid a full argon2id verification first. That is tens of
    /// milliseconds, measurable over the network, and a reliable answer to
    /// "does this account exist" whatever the response body said. Both paths now run
    /// exactly one [`PasswordHasher::verify`] at the deployment's real cost,
    /// so the timing of a refusal carries the same information for every
    /// identifier: none.
    ///
    /// Built once at construction, because it is one argon2id hash at the
    /// pinned profile (about 50ms and 19 MiB) and doing it per refusal would
    /// make the refusal path twice the work of the success path, which is a
    /// timing signal of its own.
    decoy: String,
    /// Outstanding one-shot challenges, with the instant each was minted.
    /// Keyed by a 128-bit random id, so nobody can name another person's
    /// challenge and burn it.
    pending: Mutex<BTreeMap<String, Timestamp>>,
    /// The most challenges `pending` holds ([`PASSWORD_PENDING_MAX_ENV`]).
    pending_max: usize,
    ttl_secs: u64,
}

impl<H: PasswordHasher> LocalPasswordProvider<H> {
    /// A provider over a process-local store. **Tests**; a deployment calls
    /// [`LocalPasswordProvider::with_store`] with a durable one.
    pub fn new(hasher: H) -> Self {
        LocalPasswordProvider::with_store(hasher, Arc::new(MemoryCredentialStore::new()))
    }

    /// A provider over the store the deployment chose.
    pub fn with_store(hasher: H, store: Arc<dyn CredentialStore>) -> Self {
        // The decoy is a hash of random bytes rather than of a fixed
        // string: a constant would make every deployment's decoy the same
        // value, which is a thing an attacker who obtained one dump could
        // recognise in another.
        let mut filler = [0u8; 32];
        getrandom::getrandom(&mut filler)
            .expect("the operating system CSPRNG is unavailable, refusing to start a provider");
        let decoy = hasher.hash(&hex(&filler));
        LocalPasswordProvider {
            hasher,
            store,
            decoy,
            pending: Mutex::new(BTreeMap::new()),
            pending_max: PASSWORD_PENDING_MAX,
            ttl_secs: CEREMONY_TTL_SECS,
        }
    }

    /// Cap the challenges waiting at once ([`PASSWORD_PENDING_MAX_ENV`]).
    /// A `begin` past it is refused until some complete or expire.
    pub fn with_pending_max(mut self, max: usize) -> Self {
        self.pending_max = max.max(1);
        self
    }

    pub fn with_ceremony_ttl(mut self, secs: u64) -> Self {
        self.ttl_secs = secs;
        self
    }

    /// Enroll a credential (registration path; hashing happens here, the
    /// plaintext is never stored and never reaches the store).
    ///
    /// Takes `&self`, so a provider already boxed into a
    /// [`crate::registry::ProviderRegistry`] can still be enrolled into,
    /// which is what a durable store makes possible and a field could not.
    pub fn enroll(&self, identifier: &str, password: &str) -> Result<(), CredentialStoreError> {
        let hash = self.hasher.hash(password);
        self.store.put(identifier, &hash)
    }

    /// Remove a credential. Idempotent; see [`CredentialStore::remove`].
    pub fn withdraw(&self, identifier: &str) -> Result<(), CredentialStoreError> {
        self.store.remove(identifier)
    }
}

/// A challenge id: 128 bits from the operating system's generator, as the
/// passkey ceremonies mint theirs. A counter let anyone guess the id a
/// neighbour was handed and spend it with a wrong password.
fn challenge_id() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("the operating system CSPRNG is unavailable");
    format!("pwd-ch-{}", hex(&b))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl<H: PasswordHasher> AuthProvider for LocalPasswordProvider<H> {
    fn kind(&self) -> AuthMethodKind {
        AuthMethodKind::Password
    }

    fn begin(&self, _req: &BeginRequest, now: Timestamp) -> Result<ChallengeOrRedirect, AuthError> {
        // Unconditional: existence of the identifier must not be observable
        // from whether a challenge is issued.
        let id = challenge_id();
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        // Swept where the map grows; see `CEREMONY_TTL_SECS`.
        pending.retain(|_, at| now.0.saturating_sub(at.0) <= self.ttl_secs);
        // Bounded: anyone may begin, so without a cap the map is memory an
        // anonymous caller chooses the size of.
        if pending.len() >= self.pending_max {
            return Err(AuthError::TooManyPending {
                setting: PASSWORD_PENDING_MAX_ENV,
                cap: self.pending_max,
            });
        }
        pending.insert(id.clone(), now);
        Ok(ChallengeOrRedirect::Challenge {
            id: ChallengeId(id),
            method: AuthMethodKind::Password,
        })
    }

    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        let (challenge, identifier, password) = match resp {
            CompleteRequest::Password {
                challenge,
                identifier,
                password,
            } => (challenge, identifier, password),
            _ => {
                return Err(AuthError::MethodMismatch {
                    provider: AuthMethodKind::Password,
                })
            }
        };
        // One-shot: consume the challenge before verifying, so a failed
        // verify still burns it.
        let minted = self
            .pending
            .lock()
            .expect("pending mutex poisoned")
            .remove(&challenge.0);
        // Expired and unknown are one answer: a caller must not learn that a
        // challenge *was* real from the shape of the refusal.
        match minted {
            Some(at) if now.0.saturating_sub(at.0) <= self.ttl_secs => {}
            _ => return Err(AuthError::UnknownChallenge(challenge.clone())),
        }
        // A store that cannot answer is not a wrong password, and the
        // distinction discloses nothing: it does not depend on the
        // identifier. See `CredentialStoreError`.
        let stored = self
            .store
            .hash_for(identifier)
            .map_err(AuthError::StoreUnavailable)?;
        // One `verify` on both paths, at the same cost. The unknown
        // identifier is verified against the decoy and discarded: the point
        // is the work, not the answer.
        let (hash, known) = match &stored {
            Some(h) => (h.as_str(), true),
            None => (self.decoy.as_str(), false),
        };
        if !self.hasher.verify(password, hash) || !known {
            return Err(AuthError::BadCredentials);
        }
        Ok(AuthenticatedSubject::without_presence(
            AuthenticatorId(format!("password:{identifier}")),
            AuthMethodKind::Password,
        ))
    }
}

// ───────────────────────── stub OIDC provider ───────────────────────────

/// Models the OIDC authorization-code shape without HTTP: `begin` mints
/// `state` + `nonce` and returns the authorize redirect; `complete` plays the
/// redirect-URI callback — state must be known and one-shot, a code must be
/// present, and the nonce in the "ID token" the code is exchanged for must
/// match the ceremony.
///
/// **`#[cfg(test)]`, for the reason [`PlainTestHasher`] is.** It used to be
/// exported, with a callback payload that carried the subject and the nonce
/// from the caller: a sign-in-as-anyone the moment it was wired. The subject
/// now comes from what the stand-in IdP issued for a code
/// ([`StubOidcProvider::issue`]), never from the request, and the type is
/// unreachable outside this crate's tests.
#[cfg(test)]
pub struct StubOidcProvider {
    /// Method name from the auth block (`google`, `keycloak`, …) — part of
    /// the authenticator id, so subjects from different IdPs never collide.
    method_name: String,
    authorize_endpoint: String,
    client_id: String,
    /// state → (expected nonce, when it was minted), one-shot and time-boxed.
    pending: Mutex<BTreeMap<String, (String, Timestamp)>>,
    /// What the stand-in IdP would answer for a code: (subject, nonce in the
    /// ID token). One-shot, as a real code is.
    issued: Mutex<BTreeMap<String, (String, String)>>,
    /// Deterministic token counter — test shape only; production mints
    /// state/nonce from a CSPRNG.
    counter: Mutex<u64>,
    ttl_secs: u64,
}

#[cfg(test)]
impl StubOidcProvider {
    pub fn new(method_name: &str, authorize_endpoint: &str, client_id: &str) -> Self {
        StubOidcProvider {
            method_name: method_name.to_string(),
            authorize_endpoint: authorize_endpoint.to_string(),
            client_id: client_id.to_string(),
            pending: Mutex::new(BTreeMap::new()),
            issued: Mutex::new(BTreeMap::new()),
            counter: Mutex::new(0),
            ttl_secs: CEREMONY_TTL_SECS,
        }
    }

    pub fn with_ceremony_ttl(mut self, secs: u64) -> Self {
        self.ttl_secs = secs;
        self
    }

    /// Play the IdP: the person signed in as `subject`, and `code` now
    /// exchanges for an ID token carrying `subject` and `nonce`.
    pub fn issue(&self, code: &str, subject: &str, nonce: &str) {
        self.issued
            .lock()
            .expect("issued mutex poisoned")
            .insert(code.to_string(), (subject.to_string(), nonce.to_string()));
    }
}

#[cfg(test)]
impl AuthProvider for StubOidcProvider {
    fn kind(&self) -> AuthMethodKind {
        AuthMethodKind::Oidc
    }

    fn begin(&self, _req: &BeginRequest, now: Timestamp) -> Result<ChallengeOrRedirect, AuthError> {
        let n = {
            let mut c = self.counter.lock().expect("counter mutex poisoned");
            *c += 1;
            *c
        };
        let state = format!("st-{n}");
        let nonce = format!("nc-{n}");
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        pending.retain(|_, (_, at)| now.0.saturating_sub(at.0) <= self.ttl_secs);
        pending.insert(state.clone(), (nonce.clone(), now));
        let url = format!(
            "{}?response_type=code&client_id={}&state={}&nonce={}",
            self.authorize_endpoint, self.client_id, state, nonce
        );
        Ok(ChallengeOrRedirect::Redirect {
            url,
            binder: format!("bd-{n}"),
        })
    }

    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        let (state, code) = match resp {
            CompleteRequest::OidcCallback { state, code, .. } => (state, code),
            _ => {
                return Err(AuthError::MethodMismatch {
                    provider: AuthMethodKind::Oidc,
                })
            }
        };
        // One-shot: the state is consumed even if later checks fail.
        let entry = self
            .pending
            .lock()
            .expect("pending mutex poisoned")
            .remove(state);
        // Expired reads as unknown, for the same reason as the password path.
        let expected_nonce = match entry {
            Some((n, at)) if now.0.saturating_sub(at.0) <= self.ttl_secs => n,
            _ => return Err(AuthError::StateMismatch(state.clone())),
        };
        if code.is_empty() {
            return Err(AuthError::EmptyAuthorizationCode);
        }
        let issued = self
            .issued
            .lock()
            .expect("issued mutex poisoned")
            .remove(code);
        let Some((subject, nonce)) = issued else {
            return Err(AuthError::BadCredentials);
        };
        if nonce != expected_nonce {
            return Err(AuthError::NonceMismatch);
        }
        if subject.is_empty() {
            return Err(AuthError::MissingSubject);
        }
        // An upstream IdP may well have made the human touch a key; we did not
        // see it, cannot bound its freshness, and cannot re-demand it. Federated
        // presence would be presence on somebody else's word.
        Ok(AuthenticatedSubject::without_presence(
            AuthenticatorId(format!("oidc:{}:{subject}", self.method_name)),
            AuthMethodKind::Oidc,
        ))
    }
}
