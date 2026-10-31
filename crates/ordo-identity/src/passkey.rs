//! Passkeys (WebAuthn) — and, because of that, **the only source of user
//! presence in the platform**.
//!
//! `security-logging-design.md` §2 makes this provider "the step-up source —
//! user-presence assertions come only from here", and §5 makes presence
//! non-delegable. Until this module existed those two sentences described a
//! gate with no key: [`crate::access::StepUpRequirement`] and
//! [`crate::directory::LinkCeremony`] both demand a presence timestamp and
//! nothing in the crate could produce one, so every step-up op and every
//! authenticator link was unreachable in a real deployment.
//!
//! # What is settled, and what this leaves to the deployment
//!
//! Settled by the design and implemented here: the RP id is the deployment's
//! registrable apex domain; the authenticator id is `passkey:<credential-id>`;
//! challenges are one-shot; `begin` issues one whether or not the identifier
//! exists.
//!
//! Left to the deployment, deliberately: **the ceremony itself**. The browser
//! call, the user gesture, the platform authenticator — none of that is
//! Ordo's to run and none of it can be proven in a test without a real
//! authenticator. What Ordo owns is the *verification*, which is entirely
//! here and entirely tested: challenge binding, RP-id binding, origin
//! binding, the presence flag, the signature, and the clone counter. The
//! deployment supplies [`RelyingParty`] (its apex domain and origins, the
//! `ORDO_APEX_DOMAIN` configuration `security-logging-design.md` §2 proposes)
//! and transports the four opaque byte strings between browser and server.
//!
//! # The presence type
//!
//! [`UserPresence`] has private fields and **no public constructor**. The
//! only code in the workspace that can mint one is
//! [`PasskeyProvider::complete`], a few hundred lines below, which does so
//! only after a signature verified over an `authenticatorData` whose UP flag
//! was set. That is not a convention; it is what the module boundary
//! enforces. Every other provider returns
//! [`crate::provider::AuthenticatedSubject::presence`] as `None` because it
//! has nothing to put there.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::ceremony::{CeremonyRow, CeremonyStore, CeremonyStoreError, MemoryCeremonyStore};
use crate::credentials::CredentialStoreError;
use crate::principal::{AuthMethodKind, Timestamp};
use crate::provider::{
    AuthError, AuthProvider, AuthenticatedSubject, AuthenticatorId, BeginRequest, ChallengeId,
    ChallengeOrRedirect, CompleteRequest,
};
use crate::webauthn::{
    b64url_decode, b64url_encode, rp_id_hash, signed_message, AttestationObject, AuthenticatorData,
    ClientData, CoseKey, WebAuthnError, ALG_EDDSA, ALG_ES256, CLIENT_DATA_CREATE, CLIENT_DATA_GET,
};

/// How long a WebAuthn challenge stays answerable. Pinned to the same five
/// minutes as [`crate::access::DEFAULT_STEP_UP_MAX_AGE_SECS`] on purpose: a
/// challenge that outlives the presence window it exists to produce is a
/// presence assertion with a longer life than the policy allows.
pub const CHALLENGE_TTL_SECS: u64 = 300;

// ─────────────────────────── user presence ───────────────────────────────

/// Proof that a human touched an authenticator at a known instant.
///
/// Private fields, private constructor, no `Default`, no `serde`. It can be
/// read and copied but not *made* outside this module, so no future caller
/// can conjure one from a password login, an OIDC callback, a configuration
/// flag, or a struct literal — which is the whole reason the type exists
/// rather than a bare `Option<Timestamp>` on the subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserPresence {
    at: Timestamp,
    user_verified: bool,
}

impl UserPresence {
    /// When the assertion was verified — the server's clock, not the client's.
    /// An authenticator signs no timestamp, so the only honest stamp is the
    /// instant the RP accepted the signature.
    pub fn at(&self) -> Timestamp {
        self.at
    }

    /// Whether the authenticator also performed **user verification** (a PIN
    /// or a biometric), not merely registered a touch. Recorded because "a
    /// finger was on the key" and "the owner unlocked it" are different
    /// claims and a deployment may want to demand the second.
    pub fn user_verified(&self) -> bool {
        self.user_verified
    }
}

/// How hard the authenticator must work to prove *who* is present.
///
/// The default is [`UserVerification::Required`]. Presence alone means a
/// finger touched a key that was already plugged in; for a platform passkey
/// UV costs nothing (the device unlock already happened), and demanding it is
/// what separates a step-up proof from a proximity proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserVerification {
    #[default]
    Required,
    /// Ask for it, accept without. For deployments carrying UV-incapable
    /// security keys; the resulting [`UserPresence::user_verified`] says which
    /// happened, so the weaker proof is visible rather than silent.
    Preferred,
}

impl UserVerification {
    pub fn as_str(self) -> &'static str {
        match self {
            UserVerification::Required => "required",
            UserVerification::Preferred => "preferred",
        }
    }
}

// ────────────────────────── relying party ────────────────────────────────

/// The deployment's WebAuthn identity: one RP id and the origins that may run
/// a ceremony for it.
///
/// The RP id is the registrable apex domain (`security-logging-design.md` §2,
/// `[settled]`) and **changing it strands every enrolled authenticator** —
/// credentials are scoped to it by the authenticator, not by us, so there is
/// no migration. That is why it is validated loudly at construction instead of
/// being taken on trust from configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelyingParty {
    rp_id: String,
    /// Origins under the RP id: the page and its subdomains.
    origins: Vec<String>,
    /// `https://*.<suffix>` entries of `ORDO_AUTH_ORIGINS`, each under the
    /// RP id, present only when `ORDO_WILDCARD_ORIGINS=on`. See
    /// [`RelyingParty::with_wildcards`].
    wildcards: Vec<crate::origin::OriginPattern>,
    /// Origins on **other domains**, allowed through WebAuthn Related Origin
    /// Requests. Empty unless the deployment asked. See
    /// [`RelyingParty::with_related_origins`].
    related: Vec<String>,
    /// The name an authenticator shows (`rp.name`). Defaults to the RP id.
    name: String,
    user_verification: UserVerification,
}

/// Refusals from [`RelyingParty::new`]. Every one of them is a deployment
/// that would either not work at all or, worse, work while accepting
/// assertions made on somebody else's page.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RelyingPartyError {
    /// Empty, non-ASCII, uppercase, or otherwise not a bare domain. Browsers
    /// lowercase the RP id before hashing it, so an uppercase configuration
    /// silently never matches.
    InvalidRpId { rp_id: String },
    /// No origins configured — a relying party that trusts no page cannot
    /// complete a ceremony, and an empty allowlist is far more likely to be a
    /// missing config value than an intention.
    NoOrigins,
    /// An origin that is not `scheme://host[:port]`.
    InvalidOrigin { origin: String },
    /// A plaintext origin that is not a loopback development host. WebAuthn
    /// requires a secure context; accepting `http://` for a real domain means
    /// accepting assertions from a page any network can rewrite.
    InsecureOrigin { origin: String },
    /// The origin's host is neither the RP id nor a subdomain of it. This is
    /// the misconfiguration that makes a phishing origin *authoritative*: the
    /// browser would refuse it anyway, so an allowlist entry like this can
    /// only ever be a mistake, and a silent one.
    OriginNotUnderRpId { origin: String, rp_id: String },
    /// A wildcard origin while `ORDO_WILDCARD_ORIGINS` is off, or one that is
    /// not `https://*.<domain under the RP id>`. The text says which.
    InvalidWildcard { origin: String, detail: String },
    /// A related origin that is not `https://host[:port]`. Related Origin
    /// Requests are https only, with no loopback exemption, because the
    /// origin list is published for anybody to read and a browser fetches it
    /// over TLS.
    InvalidRelatedOrigin { origin: String },
}

impl std::fmt::Display for RelyingPartyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelyingPartyError::InvalidRpId { rp_id } => write!(
                f,
                "`{rp_id}` is not a usable WebAuthn RP id — it must be a bare, \
                 lowercase, ASCII registrable domain with no scheme, port or path"
            ),
            RelyingPartyError::NoOrigins => {
                f.write_str("a relying party must list at least one allowed origin")
            }
            RelyingPartyError::InvalidOrigin { origin } => {
                write!(f, "`{origin}` is not a `scheme://host[:port]` origin")
            }
            RelyingPartyError::InsecureOrigin { origin } => write!(
                f,
                "`{origin}` is not a secure context — only https, or http on a \
                 loopback host for development"
            ),
            RelyingPartyError::InvalidRelatedOrigin { origin } => write!(
                f,
                "`{origin}` is not an `https://host[:port]` origin, which is all a \
                 related origin can be"
            ),
            RelyingPartyError::InvalidWildcard { detail, .. } => f.write_str(detail),
            RelyingPartyError::OriginNotUnderRpId { origin, rp_id } => write!(
                f,
                "`{origin}` is not `{rp_id}` or a subdomain of it, so no browser \
                 would ever send an assertion for it"
            ),
        }
    }
}

impl std::error::Error for RelyingPartyError {}

impl RelyingParty {
    /// Build and validate. `rp_id` is the apex domain (`example.com`);
    /// `origins` are the full origins clients load the app from
    /// (`https://app.example.com`).
    pub fn new(rp_id: &str, origins: &[&str]) -> Result<RelyingParty, RelyingPartyError> {
        RelyingParty::with_wildcards(rp_id, origins, false)
    }

    /// [`RelyingParty::new`], where an origin may also be
    /// `https://*.<domain>` when `wildcards` is on (`ORDO_WILDCARD_ORIGINS`,
    /// owner ruling 2026-10-09). The `*` stands for one label, the domain
    /// must be the RP id or under it, and it is https only. Safe for the
    /// same reason a subdomain is: the browser binds the assertion to the RP
    /// id, and every origin under it may already use these passkeys.
    pub fn with_wildcards(
        rp_id: &str,
        origins: &[&str],
        wildcards: bool,
    ) -> Result<RelyingParty, RelyingPartyError> {
        let mut patterns = Vec::new();
        let mut exact = Vec::new();
        for origin in origins {
            if !origin.contains('*') {
                exact.push(*origin);
                continue;
            }
            let invalid = |detail: String| RelyingPartyError::InvalidWildcard {
                origin: (*origin).to_string(),
                detail,
            };
            let p =
                crate::origin::OriginPattern::parse(origin, false, wildcards).map_err(invalid)?;
            if let crate::origin::OriginPattern::OneLabelUnder { suffix, .. } = &p {
                if suffix != rp_id && !suffix.ends_with(&format!(".{rp_id}")) {
                    return Err(RelyingPartyError::OriginNotUnderRpId {
                        origin: (*origin).to_string(),
                        rp_id: rp_id.to_string(),
                    });
                }
            }
            patterns.push(p);
        }
        if exact.is_empty() && !patterns.is_empty() {
            let mut rp = RelyingParty::exact(rp_id, &[&format!("https://{rp_id}")])?;
            rp.origins.clear();
            rp.wildcards = patterns;
            return Ok(rp);
        }
        let mut rp = RelyingParty::exact(rp_id, &exact)?;
        rp.wildcards = patterns;
        Ok(rp)
    }

    fn exact(rp_id: &str, origins: &[&str]) -> Result<RelyingParty, RelyingPartyError> {
        if !valid_rp_id(rp_id) {
            return Err(RelyingPartyError::InvalidRpId {
                rp_id: rp_id.to_string(),
            });
        }
        if origins.is_empty() {
            return Err(RelyingPartyError::NoOrigins);
        }
        for origin in origins {
            let (scheme, hostport) =
                origin
                    .split_once("://")
                    .ok_or_else(|| RelyingPartyError::InvalidOrigin {
                        origin: (*origin).to_string(),
                    })?;
            if hostport.is_empty() || hostport.contains('/') {
                return Err(RelyingPartyError::InvalidOrigin {
                    origin: (*origin).to_string(),
                });
            }
            let host = hostport.split(':').next().unwrap_or_default();
            let loopback = crate::origin::is_loopback_host(host);
            let subdomain = crate::origin::is_localhost_subdomain(host);
            match scheme {
                "https" => {}
                "http" if loopback => {}
                "http" => {
                    return Err(RelyingPartyError::InsecureOrigin {
                        origin: (*origin).to_string(),
                    })
                }
                _ => {
                    return Err(RelyingPartyError::InvalidOrigin {
                        origin: (*origin).to_string(),
                    })
                }
            }
            // The browser enforces this too; enforcing it here turns a
            // ceremony that would fail in the field into a deploy-time refusal.
            let under = host == rp_id || host.ends_with(&format!(".{rp_id}"));
            if !under && (!loopback || subdomain) {
                return Err(RelyingPartyError::OriginNotUnderRpId {
                    origin: (*origin).to_string(),
                    rp_id: rp_id.to_string(),
                });
            }
        }
        Ok(RelyingParty {
            rp_id: rp_id.to_string(),
            origins: origins.iter().map(|o| (*o).to_string()).collect(),
            wildcards: Vec::new(),
            related: Vec::new(),
            name: rp_id.to_string(),
            user_verification: UserVerification::default(),
        })
    }

    /// Allow pages on **other domains** to run a ceremony for this RP id
    /// (WebAuthn Related Origin Requests, Level 3).
    ///
    /// A browser accepts a ceremony for `rp_id` from a page on another
    /// domain only if `https://<rp_id>/.well-known/webauthn` lists that
    /// page's origin. **This method makes the server accept the origin; it
    /// does not make the browser.** The deployment owner must publish the
    /// document ([`RelyingParty::well_known_document`]), and browsers limit
    /// how many distinct domains one document may name (five labels in the
    /// current draft), so a long list stops working in the browser while
    /// still passing here.
    ///
    /// Each origin must be `https://host[:port]` with no path. An origin
    /// already under the RP id is allowed and needs no document.
    pub fn with_related_origins(
        mut self,
        origins: &[&str],
    ) -> Result<RelyingParty, RelyingPartyError> {
        for origin in origins {
            let bad = || RelyingPartyError::InvalidRelatedOrigin {
                origin: (*origin).to_string(),
            };
            let hostport = origin.strip_prefix("https://").ok_or_else(bad)?;
            let host = hostport.split(':').next().unwrap_or_default();
            if hostport.is_empty() || hostport.contains('/') || !valid_rp_id(host) {
                return Err(bad());
            }
            if let Some(port) = hostport.split_once(':').map(|(_, p)| p) {
                if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad());
                }
            }
            if !self.related.iter().any(|o| o == origin) {
                self.related.push((*origin).to_string());
            }
        }
        Ok(self)
    }

    /// The name an authenticator shows for this relying party.
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The related origins, on other domains. Empty for a relying party that
    /// did not ask.
    pub fn related_origins(&self) -> &[String] {
        &self.related
    }

    /// Every origin that may run a ceremony: those under the RP id, then the
    /// related ones. What `clientDataJSON.origin` is checked against.
    pub fn allowed_origins(&self) -> Vec<String> {
        self.origins
            .iter()
            .chain(self.related.iter())
            .cloned()
            .collect()
    }

    /// May a ceremony from `origin` complete? An exact or related origin,
    /// or one a wildcard entry matches.
    pub fn allows_origin(&self, origin: &str) -> bool {
        self.origins
            .iter()
            .chain(self.related.iter())
            .any(|o| o == origin)
            || (!self.wildcards.is_empty()
                && crate::origin::Origin::parse(origin)
                    .is_some_and(|o| self.wildcards.iter().any(|p| p.matches(&o))))
    }

    /// The document to publish at `https://<rp_id>/.well-known/webauthn`
    /// (media type `application/json`), or `None` when there are no related
    /// origins and nothing needs publishing.
    pub fn well_known_document(&self) -> Option<String> {
        if self.related.is_empty() {
            return None;
        }
        Some(serde_json::json!({ "origins": self.related }).to_string())
    }

    /// Accept authenticators that cannot verify the user. See
    /// [`UserVerification::Preferred`] for what that costs.
    pub fn with_user_verification(mut self, uv: UserVerification) -> Self {
        self.user_verification = uv;
        self
    }

    pub fn rp_id(&self) -> &str {
        &self.rp_id
    }

    /// The origins under the RP id. [`RelyingParty::allowed_origins`] adds
    /// the related ones.
    pub fn origins(&self) -> &[String] {
        &self.origins
    }

    pub fn user_verification(&self) -> UserVerification {
        self.user_verification
    }
}

/// A bare, lowercase, ASCII domain: labels of `[a-z0-9-]` joined by dots,
/// no leading or trailing dot, no empty label, no hyphen at a label edge.
fn valid_rp_id(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 || !s.is_ascii() {
        return false;
    }
    s.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

// ───────────────────────── ceremony options ──────────────────────────────

/// What the deployment hands the browser to run `navigator.credentials.create`.
/// Plain data with no serializer: the HTTP shape belongs to whoever grows the
/// login endpoint, and inventing one here would pin it early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationOptions {
    pub rp_id: String,
    pub challenge: String,
    /// COSE algorithm identifiers, most preferred first. An authenticator
    /// chooses from this list and nothing else, which is why the verifier
    /// supports exactly these two.
    pub algorithms: Vec<i64>,
    pub user_verification: &'static str,
    /// Credential ids already enrolled for this human, so an authenticator
    /// refuses to make a second credential on the same device.
    pub exclude_credentials: Vec<String>,
    /// The user handle (`user.id`) minted for this ceremony, base64url, 32
    /// bytes. The authenticator stores it beside the credential and the
    /// credential row keeps it. Never a principal id.
    pub user_handle: String,
}

/// What the deployment hands the browser to run `navigator.credentials.get`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOptions {
    pub rp_id: String,
    pub challenge: String,
    pub user_verification: &'static str,
    /// Credential ids the ceremony will accept. **Empty means any enrolled
    /// credential** — correct for a sign-in page, and wrong for a step-up,
    /// where the caller must narrow it to the session principal's own
    /// credentials. See [`crate::session::refresh_presence`], which re-checks
    /// the binding server-side rather than trusting this hint.
    pub allow_credentials: Vec<String>,
}

/// `{"publicKey": …}` for `navigator.credentials.create`: a discoverable
/// credential, no attestation. The console and the identity service both
/// answer with this shape, so one page script reads either.
pub fn creation_json(o: &CreationOptions, rp_name: &str, user_name: &str) -> serde_json::Value {
    serde_json::json!({ "publicKey": {
        "rp": { "id": o.rp_id, "name": rp_name },
        "user": { "id": o.user_handle, "name": user_name, "displayName": user_name },
        "challenge": o.challenge,
        "pubKeyCredParams": o.algorithms.iter()
            .map(|a| serde_json::json!({ "type": "public-key", "alg": a }))
            .collect::<Vec<_>>(),
        "authenticatorSelection": {
            "residentKey": "required",
            "userVerification": o.user_verification,
        },
        "excludeCredentials": o.exclude_credentials.iter()
            .map(|c| serde_json::json!({ "type": "public-key", "id": c }))
            .collect::<Vec<_>>(),
        "attestation": "none",
        "timeout": 300_000,
    }})
}

/// `{"publicKey": …}` for `navigator.credentials.get`.
pub fn request_json(o: &RequestOptions) -> serde_json::Value {
    serde_json::json!({ "publicKey": {
        "rpId": o.rp_id,
        "challenge": o.challenge,
        "userVerification": o.user_verification,
        "allowCredentials": o.allow_credentials.iter()
            .map(|c| serde_json::json!({ "type": "public-key", "id": c }))
            .collect::<Vec<_>>(),
        "timeout": 300_000,
    }})
}

// ──────────────────────────── credentials ────────────────────────────────

/// One enrolled passkey's key material. The sign count is the only mutable
/// part, and it moves in one direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasskeyCredential {
    id: Vec<u8>,
    key: CoseKey,
    sign_count: u32,
}

impl PasskeyCredential {
    /// Assemble a credential from its parts, as a store does when it loads a
    /// row. Holding one proves nothing: presence is minted only by the
    /// provider, after a signature verified.
    pub fn from_parts(id: Vec<u8>, key: CoseKey, sign_count: u32) -> PasskeyCredential {
        PasskeyCredential {
            id,
            key,
            sign_count,
        }
    }

    /// `passkey:<base64url credential id>` — the directory's link key, per
    /// `security-logging-design.md` §3.
    pub fn authenticator_id(&self) -> AuthenticatorId {
        AuthenticatorId(format!("passkey:{}", b64url_encode(&self.id)))
    }

    pub fn id(&self) -> &[u8] {
        &self.id
    }

    pub fn key(&self) -> &CoseKey {
        &self.key
    }

    pub fn algorithm(&self) -> i64 {
        self.key.algorithm()
    }

    pub fn sign_count(&self) -> u32 {
        self.sign_count
    }
}

/// What a stored credential is for (`plan-second-factor.md` §4).
///
/// A **passkey** is a first factor: it signs in alone and its assertion
/// carries presence. A **second-factor** key is a WebAuthn credential
/// registered as a second step. It shares the table and the verifier, and
/// nothing else: it cannot sign in alone, and no assertion of it can carry
/// presence, because presence is what a stolen password plus an attacker's own
/// key must never be able to buy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyRole {
    #[default]
    Passkey,
    SecondFactor,
}

impl KeyRole {
    pub fn as_str(self) -> &'static str {
        match self {
            KeyRole::Passkey => "passkey",
            KeyRole::SecondFactor => "second-factor",
        }
    }

    pub fn parse(s: &str) -> Option<KeyRole> {
        match s {
            "passkey" => Some(KeyRole::Passkey),
            "second-factor" => Some(KeyRole::SecondFactor),
            _ => None,
        }
    }
}

/// A credential as the store keeps it: the key material plus what a later
/// ceremony and a person's list of passkeys need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPasskey {
    pub credential: PasskeyCredential,
    /// A first-factor passkey or a second-factor key. See [`KeyRole`].
    pub role: KeyRole,
    /// The 32 random bytes the authenticator was told at registration
    /// (`user.id`). Never a principal id.
    pub user_handle: Vec<u8>,
    /// BE at registration: the authenticator says the credential can sync.
    pub backup_eligible: bool,
    /// BS at registration: it was already synced.
    pub backed_up: bool,
    pub registered_at: Timestamp,
    /// The last accepted assertion; zero before the first.
    pub last_used_at: Timestamp,
}

/// Why a passkey store did not answer as asked.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PasskeyStoreError {
    /// `insert` found the credential id already stored. Not an outage.
    AlreadyExists,
    /// The store could not be read or written. Safe to keep distinct from a
    /// refusal because it does not depend on which credential was offered.
    Unavailable(crate::StoreFault),
}

impl std::fmt::Display for PasskeyStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasskeyStoreError::AlreadyExists => write!(f, "that passkey is already stored"),
            PasskeyStoreError::Unavailable(why) => write!(f, "passkey store unavailable: {why}"),
        }
    }
}

impl std::error::Error for PasskeyStoreError {}

/// Where enrolled public keys are kept.
///
/// Synchronous for the reason [`crate::credentials::CredentialStore`] is:
/// the provider surface is. A store over an async pool owns the bridge.
pub trait PasskeyStore: Send + Sync {
    fn get(&self, id: &[u8]) -> Result<Option<StoredPasskey>, PasskeyStoreError>;

    /// Store a new credential. **Refuses an id already stored**
    /// ([`PasskeyStoreError::AlreadyExists`]) and never replaces it: a live
    /// public key overwritten under an existing authenticator id would hand
    /// the account to whoever asked. The store decides between two racing
    /// registrations of one id.
    fn insert(&self, cred: &StoredPasskey) -> Result<(), PasskeyStoreError>;

    /// Record an accepted assertion: set `last_used_at`, and advance the
    /// sign count to `count` **only if `count` is greater than the stored
    /// one** (a `count` of zero is an authenticator that keeps no counter,
    /// and only records the use).
    ///
    /// Returns `false`, changing nothing, when a non-zero `count` did not
    /// advance the stored one. The provider read the row before verifying;
    /// this is the write that notices a second assertion with the same count
    /// got in between, on this replica or another.
    fn set_sign_count(
        &self,
        id: &[u8],
        count: u32,
        at: Timestamp,
    ) -> Result<bool, PasskeyStoreError>;

    /// Remove a credential. Removing one that is not there is success.
    fn remove(&self, id: &[u8]) -> Result<(), PasskeyStoreError>;
}

/// The process-local store. **For tests, and for nothing a deployment
/// runs.** It is what the provider held inline before the trait existed.
#[derive(Debug, Default)]
pub struct MemoryPasskeyStore {
    rows: Mutex<BTreeMap<Vec<u8>, StoredPasskey>>,
}

impl MemoryPasskeyStore {
    pub fn new() -> MemoryPasskeyStore {
        MemoryPasskeyStore::default()
    }

    /// How many passkeys are stored. Test observation only.
    pub fn len(&self) -> usize {
        self.rows.lock().expect("passkey store").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl PasskeyStore for MemoryPasskeyStore {
    fn get(&self, id: &[u8]) -> Result<Option<StoredPasskey>, PasskeyStoreError> {
        Ok(self.rows.lock().expect("passkey store").get(id).cloned())
    }

    fn insert(&self, cred: &StoredPasskey) -> Result<(), PasskeyStoreError> {
        let mut rows = self.rows.lock().expect("passkey store");
        if rows.contains_key(cred.credential.id()) {
            return Err(PasskeyStoreError::AlreadyExists);
        }
        rows.insert(cred.credential.id().to_vec(), cred.clone());
        Ok(())
    }

    fn set_sign_count(
        &self,
        id: &[u8],
        count: u32,
        at: Timestamp,
    ) -> Result<bool, PasskeyStoreError> {
        let mut rows = self.rows.lock().expect("passkey store");
        let Some(row) = rows.get_mut(id) else {
            return Ok(false);
        };
        if count != 0 && count <= row.credential.sign_count {
            return Ok(false);
        }
        row.credential.sign_count = row.credential.sign_count.max(count);
        row.last_used_at = at;
        Ok(true)
    }

    fn remove(&self, id: &[u8]) -> Result<(), PasskeyStoreError> {
        self.rows.lock().expect("passkey store").remove(id);
        Ok(())
    }
}

// ────────────────────────────── provider ─────────────────────────────────

/// The ceremony connection name this provider files its rows under.
pub const CEREMONY_CONNECTION: &str = "passkey";

const PHASE_REGISTER: &str = "register";
const PHASE_ASSERT: &str = "assert";
const PHASE_STEP_UP: &str = "step-up";
const PHASE_REGISTER_SECOND: &str = "register-second-factor";
const PHASE_SECOND_STEP: &str = "second-step";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Register,
    Assert,
    /// An assertion inside a session, bound to that session's principal.
    StepUp,
    /// A registration of a key as a second factor, bound to the person who
    /// began it.
    RegisterSecondFactor,
    /// An assertion by a second-factor key, after a first factor succeeded,
    /// bound to that person.
    SecondStep,
}

impl Purpose {
    fn phase(self) -> &'static str {
        match self {
            Purpose::Register => PHASE_REGISTER,
            Purpose::Assert => PHASE_ASSERT,
            Purpose::StepUp => PHASE_STEP_UP,
            Purpose::RegisterSecondFactor => PHASE_REGISTER_SECOND,
            Purpose::SecondStep => PHASE_SECOND_STEP,
        }
    }

    /// The role a credential must hold to answer this ceremony.
    fn role(self) -> KeyRole {
        match self {
            Purpose::Register | Purpose::Assert | Purpose::StepUp => KeyRole::Passkey,
            Purpose::RegisterSecondFactor | Purpose::SecondStep => KeyRole::SecondFactor,
        }
    }
}

/// A registration that verified and has **not been stored**.
///
/// Splitting verification from storage is what lets the door refuse a
/// registration the deployment's policy does not admit without leaving a
/// public key behind: verify, then `sign_in`, then store. A key stored first
/// and refused after is a credential nobody can sign in with and nobody
/// removes.
#[derive(Debug, Clone)]
pub struct PendingRegistration {
    stored: StoredPasskey,
    subject: AuthenticatedSubject,
    binding: Option<String>,
    user_verified: bool,
}

impl PendingRegistration {
    /// The subject the registration proves, with presence.
    pub fn subject(&self) -> &AuthenticatedSubject {
        &self.subject
    }

    /// The binding the ceremony began with, if it had one.
    pub fn binding(&self) -> Option<&str> {
        self.binding.as_deref()
    }

    /// Whether the authenticator performed user verification at registration.
    /// Recorded beside a second-factor key, since many keys have no PIN.
    pub fn user_verified(&self) -> bool {
        self.user_verified
    }

    /// The row that [`PasskeyProvider::store_registration`] will insert.
    pub fn stored(&self) -> &StoredPasskey {
        &self.stored
    }
}

/// What a consumed challenge hands back: the bytes the response must sign
/// over, and the provider's payload from `begin`.
struct Taken {
    challenge: Vec<u8>,
    payload: String,
}

/// The handle a ceremony is filed under: SHA-256 of the challenge bytes,
/// base64url. The challenge itself is never stored.
fn ceremony_handle(challenge: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    b64url_encode(&Sha256::digest(challenge))
}

fn keys_unavailable(e: PasskeyStoreError) -> AuthError {
    AuthError::StoreUnavailable(CredentialStoreError::from(e))
}

fn ceremonies_unavailable(e: CeremonyStoreError) -> AuthError {
    AuthError::StoreUnavailable(CredentialStoreError::from(e))
}

/// The passkey provider.
///
/// Its two tables are behind traits: enrolled public keys
/// ([`PasskeyStore`]) and open ceremonies ([`CeremonyStore`]). With the
/// memory doubles it behaves as it always did; with the Postgres stores a
/// registered passkey and an open challenge survive a restart, and a
/// challenge minted on one replica completes on another.
pub struct PasskeyProvider {
    rp: RelyingParty,
    keys: Arc<dyn PasskeyStore>,
    ceremonies: Arc<dyn CeremonyStore>,
    challenge_ttl_secs: u64,
    /// User verification for a key used as a second factor. `Preferred`
    /// because many security keys have no PIN, and the record says whether it
    /// happened rather than refusing the key.
    second_factor_uv: UserVerification,
}

impl PasskeyProvider {
    /// A provider over the memory doubles. **For tests.**
    pub fn new(rp: RelyingParty) -> Self {
        PasskeyProvider::with_stores(
            rp,
            Arc::new(MemoryPasskeyStore::new()),
            Arc::new(MemoryCeremonyStore::new()),
        )
    }

    /// A provider over durable stores.
    pub fn with_stores(
        rp: RelyingParty,
        keys: Arc<dyn PasskeyStore>,
        ceremonies: Arc<dyn CeremonyStore>,
    ) -> Self {
        PasskeyProvider {
            rp,
            keys,
            ceremonies,
            challenge_ttl_secs: CHALLENGE_TTL_SECS,
            second_factor_uv: UserVerification::Preferred,
        }
    }

    /// Set the user verification a second-factor key is asked for.
    pub fn with_second_factor_uv(mut self, uv: UserVerification) -> Self {
        self.second_factor_uv = uv;
        self
    }

    pub fn with_challenge_ttl(mut self, secs: u64) -> Self {
        self.challenge_ttl_secs = secs;
        self
    }

    pub fn relying_party(&self) -> &RelyingParty {
        &self.rp
    }

    pub fn challenge_ttl_secs(&self) -> u64 {
        self.challenge_ttl_secs
    }

    /// The algorithms this deployment advertises, most preferred first.
    pub fn algorithms() -> [i64; 2] {
        [ALG_ES256, ALG_EDDSA]
    }

    /// An enrolled credential, by its raw id. A store that cannot answer is
    /// `None` here; [`PasskeyProvider::stored`] tells the two apart.
    pub fn credential(&self, id: &[u8]) -> Option<PasskeyCredential> {
        self.stored(id).ok().flatten().map(|s| s.credential)
    }

    /// An enrolled credential with everything the store keeps about it.
    pub fn stored(&self, id: &[u8]) -> Result<Option<StoredPasskey>, PasskeyStoreError> {
        self.keys.get(id)
    }

    /// Remove an enrolled public key. The other half of an unlink: the
    /// directory's link says whose the credential is, and this row is what
    /// verifies its assertions, so revoking a passkey removes both. Removing
    /// one that is not there is success.
    pub fn remove_credential(&self, id: &[u8]) -> Result<(), AuthError> {
        self.keys.remove(id).map_err(keys_unavailable)
    }

    /// Start an **enrollment**. Separate from [`AuthProvider::begin`] because
    /// registration and assertion are different ceremonies with different
    /// client calls, and a single entry point would have to be told which —
    /// at which point it is two entry points wearing one name.
    ///
    /// `exclude` is the caller's list of the human's existing credential ids;
    /// it is a client-side convenience (stop a second key on the same device)
    /// and is not a security boundary — the boundary is
    /// [`crate::directory::Directory::link`]'s dual proof.
    ///
    /// Mints the **user handle** the authenticator will be told: 32 random
    /// bytes, held in the ceremony row and written to the credential row at
    /// completion. It is never a principal id, because the principal may not
    /// exist until the registration completes.
    pub fn begin_registration(
        &self,
        now: Timestamp,
        exclude: &[Vec<u8>],
    ) -> Result<CreationOptions, AuthError> {
        self.begin_registration_bound(now, exclude, None)
    }

    /// The same, with a **binding**: a word the caller needs back at
    /// completion, held in the ceremony row and never shown to the browser
    /// (the door binds a registration to the hash of the ticket it began
    /// with, so a ceremony begun with a ticket cannot finish without it).
    pub fn begin_registration_bound(
        &self,
        now: Timestamp,
        exclude: &[Vec<u8>],
        binding: Option<&str>,
    ) -> Result<CreationOptions, AuthError> {
        let mut handle = [0u8; 32];
        getrandom::getrandom(&mut handle)
            .expect("the operating system CSPRNG is unavailable — refusing to mint a user handle");
        let user_handle = b64url_encode(&handle);
        let payload =
            serde_json::json!({ "user_handle": user_handle, "binding": binding }).to_string();
        let (id, _) = self.mint(Purpose::Register, payload, now)?;
        Ok(CreationOptions {
            rp_id: self.rp.rp_id.clone(),
            challenge: id.0,
            algorithms: Self::algorithms().to_vec(),
            user_verification: self.rp.user_verification.as_str(),
            exclude_credentials: exclude.iter().map(|c| b64url_encode(c)).collect(),
            user_handle,
        })
    }

    /// Start an **assertion**, optionally narrowed to a set of credentials.
    ///
    /// Anti-enumeration (`security-logging-design.md` §2): a challenge is
    /// issued whether or not those credentials exist, and an empty `allow`
    /// list is the ordinary sign-in case rather than an error.
    pub fn begin_assertion(
        &self,
        now: Timestamp,
        allow: &[Vec<u8>],
    ) -> Result<RequestOptions, AuthError> {
        let (id, _) = self.mint(Purpose::Assert, "{}".to_string(), now)?;
        Ok(RequestOptions {
            rp_id: self.rp.rp_id.clone(),
            challenge: id.0,
            user_verification: self.rp.user_verification.as_str(),
            allow_credentials: allow.iter().map(|c| b64url_encode(c)).collect(),
        })
    }

    /// Mint a one-shot challenge and file it in the ceremony store.
    ///
    /// Expired rows are not swept here: the store's own sweep does it (the
    /// host runs one every minute), and an expired row answers nothing in
    /// the meantime because `take` checks the expiry.
    fn mint(
        &self,
        purpose: Purpose,
        payload: String,
        now: Timestamp,
    ) -> Result<(ChallengeId, Vec<u8>), AuthError> {
        let mut bytes = [0u8; 32];
        // A relying party that cannot obtain randomness must not fall back to
        // anything: a predictable challenge makes every assertion replayable.
        getrandom::getrandom(&mut bytes)
            .expect("the operating system CSPRNG is unavailable — refusing to mint a challenge");
        let encoded = b64url_encode(&bytes);
        self.ceremonies
            .put(&CeremonyRow {
                handle: ceremony_handle(&bytes),
                connection: CEREMONY_CONNECTION.to_string(),
                phase: purpose.phase().to_string(),
                binder: String::new(),
                payload,
                minted_at: now,
                expires_at: Timestamp(now.0.saturating_add(self.challenge_ttl_secs)),
            })
            .map_err(ceremonies_unavailable)?;
        Ok((ChallengeId(encoded), bytes.to_vec()))
    }

    /// Consume a challenge. One-shot in the strong sense: the row is deleted
    /// before anything is verified, so a failed attempt burns it and a replay
    /// of a *successful* response finds nothing to match. The delete is the
    /// store's, so two replicas racing on one challenge cannot both win.
    ///
    /// A challenge minted for an enrollment must not answer a sign-in: the
    /// phase is part of the delete's condition, and the `clientData.type`
    /// check catches the same crossing from the other side. Two independent
    /// checks is the point. A crossed attempt finds nothing and leaves the
    /// other ceremony's row where it was.
    fn consume(
        &self,
        id: &ChallengeId,
        purpose: Purpose,
        now: Timestamp,
    ) -> Result<Taken, AuthError> {
        let unknown = || AuthError::UnknownChallenge(id.clone());
        let challenge = b64url_decode(&id.0).map_err(|_| unknown())?;
        let payload = self
            .ceremonies
            .take(
                &ceremony_handle(&challenge),
                CEREMONY_CONNECTION,
                purpose.phase(),
                now,
            )
            .map_err(ceremonies_unavailable)?
            .ok_or_else(unknown)?;
        Ok(Taken { challenge, payload })
    }

    /// The user verification a ceremony of `purpose` asks for.
    fn uv_for(&self, purpose: Purpose) -> UserVerification {
        match purpose.role() {
            KeyRole::Passkey => self.rp.user_verification,
            KeyRole::SecondFactor => self.second_factor_uv,
        }
    }

    fn check_flags(
        &self,
        data: &AuthenticatorData,
        uv: UserVerification,
    ) -> Result<bool, WebAuthnError> {
        // Presence first: it is the claim the whole step-up gate rests on.
        if !data.user_present() {
            return Err(WebAuthnError::UserPresenceMissing);
        }
        let verified = data.user_verified();
        if uv == UserVerification::Required && !verified {
            return Err(WebAuthnError::UserVerificationMissing);
        }
        Ok(verified)
    }

    /// Enroll a credential from a registration response: verify it, then
    /// store it.
    fn complete_registration(
        &self,
        challenge: &ChallengeId,
        client_data_json: &[u8],
        attestation_object: &[u8],
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        let pending =
            self.verify_registration(challenge, client_data_json, attestation_object, now)?;
        self.store_registration(&pending)?;
        Ok(pending.subject)
    }

    /// Verify a registration response and consume its challenge, **without
    /// storing anything**. Refuses a credential id already stored, so the
    /// common duplicate is caught before the directory is touched;
    /// [`PasskeyProvider::store_registration`] refuses the race the check
    /// cannot see.
    pub fn verify_registration(
        &self,
        challenge: &ChallengeId,
        client_data_json: &[u8],
        attestation_object: &[u8],
        now: Timestamp,
    ) -> Result<PendingRegistration, AuthError> {
        self.verify_registration_as(
            Purpose::Register,
            challenge,
            client_data_json,
            attestation_object,
            now,
        )
        .map(|(pending, _)| pending)
    }

    /// The registration verifier both roles share. Answers the ceremony's
    /// payload too, which a second-factor registration reads its principal
    /// from.
    fn verify_registration_as(
        &self,
        purpose: Purpose,
        challenge: &ChallengeId,
        client_data_json: &[u8],
        attestation_object: &[u8],
        now: Timestamp,
    ) -> Result<(PendingRegistration, String), AuthError> {
        let taken = self.consume(challenge, purpose, now)?;
        let client = ClientData::parse(client_data_json)?;
        client.check(CLIENT_DATA_CREATE, &taken.challenge, &|o| {
            self.rp.allows_origin(o)
        })?;
        let att = AttestationObject::parse(attestation_object)?;
        if att.auth_data.rp_id_hash != rp_id_hash(&self.rp.rp_id) {
            return Err(WebAuthnError::RpIdMismatch.into());
        }
        let user_verified = self.check_flags(&att.auth_data, self.uv_for(purpose))?;
        let attested = att
            .auth_data
            .credential
            .clone()
            .ok_or(WebAuthnError::NoAttestedCredential)?;
        // The handle minted at begin. A row without one is not one this
        // provider wrote, so it answers nothing.
        let user_handle = serde_json::from_str::<serde_json::Value>(&taken.payload)
            .ok()
            .and_then(|v| v.get("user_handle")?.as_str().map(str::to_string))
            .and_then(|h| b64url_decode(&h).ok())
            .ok_or_else(|| AuthError::UnknownChallenge(challenge.clone()))?;
        let credential = PasskeyCredential {
            id: attested.id,
            key: attested.key,
            sign_count: att.auth_data.sign_count,
        };
        let authenticator = credential.authenticator_id();
        // Re-registering an id we already hold would overwrite a live public
        // key with an attacker-chosen one — the directory would keep the same
        // link and the credential behind it would have changed hands.
        if self
            .keys
            .get(credential.id())
            .map_err(keys_unavailable)?
            .is_some()
        {
            return Err(AuthError::CredentialAlreadyEnrolled);
        }
        let binding = serde_json::from_str::<serde_json::Value>(&taken.payload)
            .ok()
            .and_then(|v| v.get("binding")?.as_str().map(str::to_string));
        // A first-factor passkey's registration proves presence. A second
        // factor's proves none: the subject carries `presence: None`, so
        // nothing downstream can mint presence from it.
        let presence = match purpose.role() {
            KeyRole::Passkey => Some(UserPresence {
                at: now,
                user_verified,
            }),
            KeyRole::SecondFactor => None,
        };
        let pending = PendingRegistration {
            binding,
            user_verified,
            stored: StoredPasskey {
                credential,
                role: purpose.role(),
                user_handle,
                backup_eligible: att.auth_data.backup_eligible(),
                backed_up: att.auth_data.backed_up(),
                registered_at: now,
                last_used_at: Timestamp(0),
            },
            subject: AuthenticatedSubject {
                authenticator,
                method: AuthMethodKind::Passkey,
                attributes: crate::provider::SubjectAttributes::default(),
                presence,
                upstream_presence: None,
            },
        };
        Ok((pending, taken.payload))
    }

    /// Store a verified registration. The store refuses an id already held
    /// and decides between two racing registrations of one id.
    pub fn store_registration(&self, pending: &PendingRegistration) -> Result<(), AuthError> {
        self.keys.insert(&pending.stored).map_err(|e| match e {
            PasskeyStoreError::AlreadyExists => AuthError::CredentialAlreadyEnrolled,
            other => keys_unavailable(other),
        })
    }

    /// Verify a sign-in assertion. This is the function that mints presence.
    fn complete_assertion(
        &self,
        challenge: &ChallengeId,
        credential_id: &[u8],
        client_data_json: &[u8],
        authenticator_data: &[u8],
        signature: &[u8],
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        self.verify_assertion(
            Purpose::Assert,
            challenge,
            credential_id,
            client_data_json,
            authenticator_data,
            signature,
            now,
        )
        .map(|(subject, _)| subject)
    }

    /// Start a **step-up** inside a session: an assertion challenge bound to
    /// the session's principal, in its own phase so it cannot be answered at
    /// the sign-in route and a sign-in challenge cannot be answered here.
    /// `allow` narrows the request to that principal's own credentials; the
    /// server re-checks the binding at completion whatever the browser does
    /// with the list.
    pub fn begin_step_up(
        &self,
        now: Timestamp,
        principal: &crate::principal::PrincipalId,
        allow: &[Vec<u8>],
    ) -> Result<RequestOptions, AuthError> {
        self.begin_step_up_for(now, principal, None, allow)
    }

    /// [`PasskeyProvider::begin_step_up`], bound also to **one session**
    /// (`plan-console.md` 8.3, G15). The door's `/auth/step-up` refuses a
    /// challenge minted for another session of the same principal, so a
    /// touch relayed for one session cannot be spent on another.
    pub fn begin_step_up_for(
        &self,
        now: Timestamp,
        principal: &crate::principal::PrincipalId,
        session: Option<&str>,
        allow: &[Vec<u8>],
    ) -> Result<RequestOptions, AuthError> {
        let payload = match session {
            Some(s) => serde_json::json!({ "principal": principal.0, "session": s }),
            None => serde_json::json!({ "principal": principal.0 }),
        }
        .to_string();
        let (id, _) = self.mint(Purpose::StepUp, payload, now)?;
        Ok(RequestOptions {
            rp_id: self.rp.rp_id.clone(),
            challenge: id.0,
            user_verification: self.rp.user_verification.as_str(),
            allow_credentials: allow.iter().map(|c| b64url_encode(c)).collect(),
        })
    }

    /// Verify a step-up assertion and return the subject with **the
    /// principal the challenge was minted for**. The caller compares it to
    /// the session's principal; `refresh_presence` checks the asserting
    /// passkey's link independently.
    pub fn complete_step_up(
        &self,
        challenge: &ChallengeId,
        credential_id: &[u8],
        client_data_json: &[u8],
        authenticator_data: &[u8],
        signature: &[u8],
        now: Timestamp,
    ) -> Result<(AuthenticatedSubject, crate::principal::PrincipalId), AuthError> {
        self.complete_step_up_for(
            challenge,
            credential_id,
            client_data_json,
            authenticator_data,
            signature,
            now,
        )
        .map(|(subject, principal, _)| (subject, principal))
    }

    /// [`PasskeyProvider::complete_step_up`], answering also the session the
    /// challenge was bound to, if it was bound to one.
    pub fn complete_step_up_for(
        &self,
        challenge: &ChallengeId,
        credential_id: &[u8],
        client_data_json: &[u8],
        authenticator_data: &[u8],
        signature: &[u8],
        now: Timestamp,
    ) -> Result<
        (
            AuthenticatedSubject,
            crate::principal::PrincipalId,
            Option<String>,
        ),
        AuthError,
    > {
        let (subject, payload) = self.verify_assertion(
            Purpose::StepUp,
            challenge,
            credential_id,
            client_data_json,
            authenticator_data,
            signature,
            now,
        )?;
        let parsed = serde_json::from_str::<serde_json::Value>(&payload).ok();
        let principal = parsed
            .as_ref()
            .and_then(|v| v.get("principal")?.as_str().map(str::to_string))
            .map(crate::principal::PrincipalId)
            .ok_or_else(|| AuthError::UnknownChallenge(challenge.clone()))?;
        let session = parsed
            .as_ref()
            .and_then(|v| v.get("session")?.as_str().map(str::to_string));
        Ok((subject, principal, session))
    }

    /// Start registering a key as **a second factor** for `principal`. Its own
    /// phase, so a first-factor registration challenge cannot finish it and it
    /// cannot finish a first-factor registration, and its own user
    /// verification (`Preferred` unless the deployment says otherwise).
    pub fn begin_second_factor_registration(
        &self,
        now: Timestamp,
        principal: &crate::principal::PrincipalId,
        exclude: &[Vec<u8>],
    ) -> Result<CreationOptions, AuthError> {
        let mut handle = [0u8; 32];
        getrandom::getrandom(&mut handle)
            .expect("the operating system CSPRNG is unavailable — refusing to mint a user handle");
        let user_handle = b64url_encode(&handle);
        let payload = serde_json::json!({
            "user_handle": user_handle, "binding": null, "principal": principal.0,
        })
        .to_string();
        let (id, _) = self.mint(Purpose::RegisterSecondFactor, payload, now)?;
        Ok(CreationOptions {
            rp_id: self.rp.rp_id.clone(),
            challenge: id.0,
            algorithms: Self::algorithms().to_vec(),
            user_verification: self.second_factor_uv.as_str(),
            exclude_credentials: exclude.iter().map(|c| b64url_encode(c)).collect(),
            user_handle,
        })
    }

    /// Verify a second-factor registration. Answers the pending row and the
    /// principal the ceremony was begun for; nothing is stored.
    pub fn verify_second_factor_registration(
        &self,
        challenge: &ChallengeId,
        client_data_json: &[u8],
        attestation_object: &[u8],
        now: Timestamp,
    ) -> Result<(PendingRegistration, crate::principal::PrincipalId), AuthError> {
        let (pending, payload) = self.verify_registration_as(
            Purpose::RegisterSecondFactor,
            challenge,
            client_data_json,
            attestation_object,
            now,
        )?;
        let principal = serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .and_then(|v| v.get("principal")?.as_str().map(str::to_string))
            .map(crate::principal::PrincipalId)
            .ok_or_else(|| AuthError::UnknownChallenge(challenge.clone()))?;
        Ok((pending, principal))
    }

    /// Start the second-step assertion for `principal`, narrowed to their
    /// second-factor keys (`allow`). A non-discoverable credential cannot be
    /// found without its id, so the list is not optional here.
    pub fn begin_second_step(
        &self,
        now: Timestamp,
        principal: &crate::principal::PrincipalId,
        allow: &[Vec<u8>],
    ) -> Result<RequestOptions, AuthError> {
        let payload = serde_json::json!({ "principal": principal.0 }).to_string();
        let (id, _) = self.mint(Purpose::SecondStep, payload, now)?;
        Ok(RequestOptions {
            rp_id: self.rp.rp_id.clone(),
            challenge: id.0,
            user_verification: self.second_factor_uv.as_str(),
            allow_credentials: allow.iter().map(|c| b64url_encode(c)).collect(),
        })
    }

    /// Verify a second-step assertion. What comes back is **not** an
    /// [`AuthenticatedSubject`] and carries no [`UserPresence`]: a second
    /// factor proves possession of a key for this person and nothing that a
    /// step-up could read. The caller compares the principal to the one the
    /// first factor named.
    pub fn complete_second_step(
        &self,
        challenge: &ChallengeId,
        credential_id: &[u8],
        client_data_json: &[u8],
        authenticator_data: &[u8],
        signature: &[u8],
        now: Timestamp,
    ) -> Result<SecondStepProof, AuthError> {
        let (subject, payload) = self.verify_assertion(
            Purpose::SecondStep,
            challenge,
            credential_id,
            client_data_json,
            authenticator_data,
            signature,
            now,
        )?;
        let principal = serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .and_then(|v| v.get("principal")?.as_str().map(str::to_string))
            .map(crate::principal::PrincipalId)
            .ok_or_else(|| AuthError::UnknownChallenge(challenge.clone()))?;
        Ok(SecondStepProof {
            principal,
            authenticator: subject.authenticator,
            user_verified: subject.presence.is_some_and(|p| p.user_verified()),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_assertion(
        &self,
        purpose: Purpose,
        challenge: &ChallengeId,
        credential_id: &[u8],
        client_data_json: &[u8],
        authenticator_data: &[u8],
        signature: &[u8],
        now: Timestamp,
    ) -> Result<(AuthenticatedSubject, String), AuthError> {
        let taken = self.consume(challenge, purpose, now)?;
        let client = ClientData::parse(client_data_json)?;
        client.check(CLIENT_DATA_GET, &taken.challenge, &|o| {
            self.rp.allows_origin(o)
        })?;
        let data = AuthenticatorData::parse(authenticator_data)?;
        if data.rp_id_hash != rp_id_hash(&self.rp.rp_id) {
            return Err(WebAuthnError::RpIdMismatch.into());
        }
        let user_verified = self.check_flags(&data, self.uv_for(purpose))?;

        // Anti-enumeration: an unknown credential is the same refusal as a
        // signature that did not verify, so probing ids tells a caller nothing.
        let row = self
            .keys
            .get(credential_id)
            .map_err(keys_unavailable)?
            .ok_or(AuthError::BadCredentials)?;
        // A second-factor key cannot sign in or step up, and a first-factor
        // passkey cannot answer a second step. The same refusal, so probing
        // which is which tells a caller nothing either.
        if row.role != purpose.role() {
            return Err(AuthError::BadCredentials);
        }
        let stored = row.credential;
        stored
            .key
            .verify(
                &signed_message(authenticator_data, client_data_json),
                signature,
            )
            .map_err(|_| AuthError::BadCredentials)?;
        // The counter is checked *after* the signature: a caller who cannot
        // sign must not be able to provoke a clone alert on somebody else's
        // credential, which would be a denial-of-service on an account.
        //
        // A stored and presented count of zero means the authenticator does
        // not keep one (the common case for platform passkeys) — nothing to
        // compare, and pretending otherwise would refuse every Apple device.
        if (data.sign_count != 0 || stored.sign_count != 0) && data.sign_count <= stored.sign_count
        {
            return Err(WebAuthnError::SignCountRegression {
                stored: stored.sign_count,
                presented: data.sign_count,
            }
            .into());
        }
        // **Written before the subject is returned.** A counter that
        // advanced only in memory is not a clone detector after a restart,
        // and one that advanced on this replica only is not one on the next.
        // If the write fails the assertion is refused, so a store outage
        // cannot make a replayed assertion succeed. A write that finds the
        // count already at or past this one lost a race with a second
        // assertion, and is the regression it looks like.
        let advanced = self
            .keys
            .set_sign_count(credential_id, data.sign_count, now)
            .map_err(keys_unavailable)?;
        if !advanced {
            return Err(WebAuthnError::SignCountRegression {
                stored: stored.sign_count,
                presented: data.sign_count,
            }
            .into());
        }
        Ok((
            AuthenticatedSubject {
                authenticator: stored.authenticator_id(),
                method: AuthMethodKind::Passkey,
                attributes: crate::provider::SubjectAttributes::default(),
                presence: Some(UserPresence {
                    at: now,
                    user_verified,
                }),
                upstream_presence: None,
            },
            taken.payload,
        ))
    }
}

/// What a verified second-step assertion proves: which person the challenge
/// was for, which credential answered, and whether the key verified the user.
/// **No presence**, by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondStepProof {
    pub principal: crate::principal::PrincipalId,
    pub authenticator: AuthenticatorId,
    pub user_verified: bool,
}

/// A shared provider is a provider, so the registry can hold one handle and
/// the routes that need the concrete type (registration, step-up) another.
impl AuthProvider for Arc<PasskeyProvider> {
    fn kind(&self) -> AuthMethodKind {
        (**self).kind()
    }

    fn begin(&self, req: &BeginRequest, now: Timestamp) -> Result<ChallengeOrRedirect, AuthError> {
        (**self).begin(req, now)
    }

    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        (**self).complete(resp, now)
    }
}

impl AuthProvider for PasskeyProvider {
    fn kind(&self) -> AuthMethodKind {
        AuthMethodKind::Passkey
    }

    /// The sign-in ceremony. Registration has its own entry point
    /// ([`PasskeyProvider::begin_registration`]) because enrolling a factor is
    /// not a login and the registry routes logins.
    fn begin(&self, _req: &BeginRequest, now: Timestamp) -> Result<ChallengeOrRedirect, AuthError> {
        // A store that could not file the challenge is a refusal, not a
        // challenge nobody can complete.
        let opts = self.begin_assertion(now, &[])?;
        Ok(ChallengeOrRedirect::Challenge {
            id: ChallengeId(opts.challenge),
            method: AuthMethodKind::Passkey,
        })
    }

    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        match resp {
            CompleteRequest::PasskeyRegistration {
                challenge,
                client_data_json,
                attestation_object,
            } => self.complete_registration(challenge, client_data_json, attestation_object, now),
            CompleteRequest::PasskeyAssertion {
                challenge,
                credential_id,
                client_data_json,
                authenticator_data,
                signature,
            } => self.complete_assertion(
                challenge,
                credential_id,
                client_data_json,
                authenticator_data,
                signature,
                now,
            ),
            _ => Err(AuthError::MethodMismatch {
                provider: AuthMethodKind::Passkey,
            }),
        }
    }
}

/// Decode a base64url credential id as the browser sends it.
pub fn credential_id_from_b64url(s: &str) -> Result<Vec<u8>, WebAuthnError> {
    b64url_decode(s)
}
