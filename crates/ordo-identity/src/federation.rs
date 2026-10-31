//! What every sign-in through an outside provider shares: the handoff, the
//! binder, and the one way such a provider reaches the network
//! (`plan-auth-federated.md` §3.1 to §3.4).
//!
//! # The handoff, and why there is one
//!
//! An outside provider returns the browser to Ordo's callback, not to the
//! application. Ordo verifies what the provider says there, then writes a
//! **handoff**: a row in `$authceremony`, phase `handoff`, sixty seconds
//! long, holding the subject the provider vouched for. The browser goes back
//! to the application's `return_to` with the handoff code in the URL, and the
//! application redeems it at `/auth/complete` together with the **binder**
//! `begin` gave it.
//!
//! The binder is 256 random bits that never travel in a URL. The `begun` row
//! and the `handoff` row are both filed under its SHA-256, and
//! [`CeremonyStore::take_bound`] redeems a handoff only with it. A callback
//! URL somebody planted in another person's browser therefore produces a
//! handoff that browser cannot redeem: login CSRF is closed without a cookie,
//! which matters because Ordo is an API on another origin from the page.
//!
//! # Resolution happens at redemption
//!
//! The handoff carries the subject, not a principal. `/auth/complete` resolves
//! it through `resolve_or_onboard` exactly as a passkey sign-in does, so the
//! directory's deleted-account refusal (§2.7 D1, D3) is asked at the moment a
//! session is minted, and a deletion between the callback and the redemption
//! wins.
//!
//! # The network
//!
//! [`FederationTransport`] is the whole of a provider's reach. `ordo-host`
//! implements it over `ordo-egress`'s `fetch`, which keeps the kernel's pins
//! (https only, resolve then pin, no redirects, an allowlist of exactly the
//! connection's hosts). This crate opens no socket, so every parser in it is
//! a function of bytes, testable without a network.
//!
//! **A client secret is passed per call and never held by the transport**,
//! the way the egress kernel takes a sink secret. Nothing here derives
//! `Debug` over a secret, and no error carries a byte a provider sent: the
//! words in [`ProviderError`] and in [`AuthError::UpstreamRefused`] are
//! fixed strings.

use crate::ceremony::{CeremonyRow, CeremonyStore};
use crate::principal::{AuthMethodKind, Timestamp};
use crate::provider::{
    AuthError, AuthenticatedSubject, AuthenticatorId, EmailEvidence, SubjectAttributes,
    UpstreamAssurance, VerifiedEmail,
};
use crate::CredentialStoreError;

/// The phase a redirect ceremony is filed under at `begin`.
pub const PHASE_BEGUN: &str = "begun";
/// The phase a callback files the verified subject under.
pub const PHASE_HANDOFF: &str = "handoff";
/// How long a handoff is redeemable by default
/// (`ORDO_AUTH_PROVIDER_HANDOFF_SECS`). Long enough for a browser to load the
/// application's page and post it; short enough that a code in a history
/// file or a referrer is worthless by the time anybody reads it.
pub const HANDOFF_TTL_SECS: u64 = 60;

static HANDOFF_TTL: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(HANDOFF_TTL_SECS);

/// Set the handoff life from the deployment's setting. Called once at boot,
/// before any sign-in is served.
pub fn set_handoff_ttl_secs(secs: u64) {
    HANDOFF_TTL.store(secs, std::sync::atomic::Ordering::Relaxed);
}

/// The handoff life in force: the setting, or [`HANDOFF_TTL_SECS`].
pub fn handoff_ttl_secs() -> u64 {
    HANDOFF_TTL.load(std::sync::atomic::Ordering::Relaxed)
}

/// 256 bits from the OS CSPRNG, base64url. A `state`, a binder, a PKCE
/// verifier or a handoff code.
pub fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .expect("the operating system CSPRNG is unavailable, refusing to mint a ceremony");
    crate::webauthn::b64url_encode(&bytes)
}

/// The stored form of a secret: SHA-256, base64url. A reader of
/// `$authceremony` cannot go from a handle back to the value a browser holds.
pub fn handle_of(secret: &str) -> String {
    use sha2::{Digest as _, Sha256};
    crate::webauthn::b64url_encode(&Sha256::digest(secret.as_bytes()))
}

/// A client secret. **No `Debug`, no `Display`, no `Clone`, no serialiser**:
/// the type cannot reach a log line, an error or the chain by accident. It is
/// read from a file at boot and handed to the transport per call.
pub struct ClientSecret(String);

impl ClientSecret {
    pub fn new(secret: String) -> ClientSecret {
        ClientSecret(secret.trim().to_string())
    }

    /// The bytes, for the one place they go: the body of a token request.
    /// Named so its use anywhere else is visible in a review.
    pub fn reveal_to_wire(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What a provider asks the transport to do.
#[non_exhaustive]
pub enum FederationMethod {
    /// `GET`, with an optional bearer token: a userinfo call.
    Get,
    /// `POST` of `application/x-www-form-urlencoded`: a token exchange.
    PostForm,
}

/// One request to an outside provider. **Not `Debug`**: it can hold a client
/// secret and an access token.
pub struct FederationRequest<'a> {
    pub method: FederationMethod,
    /// An `https` URL whose host is one of the connection's configured hosts.
    pub url: &'a str,
    /// Form fields, for [`FederationMethod::PostForm`].
    pub form: Vec<(&'a str, &'a str)>,
    /// Appended to the form as `client_secret` by the transport.
    pub client_secret: Option<&'a ClientSecret>,
    /// Sent as `Authorization: Bearer`, for a userinfo call.
    pub bearer: Option<&'a str>,
    /// The largest body accepted. Over it is a refusal, not a truncation.
    pub cap: usize,
}

/// A provider's answer body. **Not `Debug`, not `Display`**: it is read once,
/// by the parser it was fetched for, and never shown.
pub struct FederationResponse(Vec<u8>, Option<u64>);

impl FederationResponse {
    pub fn new(body: Vec<u8>) -> FederationResponse {
        FederationResponse(body, None)
    }

    /// With the answer's `Cache-Control` freshness, in seconds.
    pub fn with_max_age(mut self, max_age: Option<u64>) -> FederationResponse {
        self.1 = max_age;
        self
    }

    /// The bytes, for the parser.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    /// How long the provider said this answer may be cached. `None` when it
    /// said nothing.
    pub fn max_age(&self) -> Option<u64> {
        self.1
    }
}

/// How a provider call ended, when it did not produce a body. Sorted by
/// [`ProviderError::class`], never by text: `kind` is a fixed word safe for
/// the security stream and for nothing else to branch on.
///
/// It keeps the cause: `source()` is the egress refusal (or whatever the
/// transport met), `status` is the provider's HTTP status when it answered
/// one, `retryable` is the egress kernel's own verdict
/// (`Refusal::retryable`), and `retry_after` is the provider's `Retry-After`
/// when it gave one. A 5xx, 408, 425 or 429 is the provider asking for a
/// later attempt, not refusing the person.
#[derive(Debug, Clone)]
pub struct ProviderError {
    kind: &'static str,
    status: Option<u16>,
    retryable: bool,
    bad_response: bool,
    retry_after: Option<std::time::Duration>,
    source: Option<std::sync::Arc<dyn std::error::Error + Send + Sync + 'static>>,
}

/// What a provider call that failed means for the sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProviderClass {
    /// Not now: the provider is down, rate limiting, or not reached. Try
    /// again later. Says nothing about the person.
    Busy,
    /// The provider answered a final no (a 4xx other than 408, 425, 429).
    Refused,
    /// The provider answered, but not with a usable document.
    BadResponse,
    /// The call cannot succeed as configured: a pin, an address the
    /// deployment denies, a URL that is not https. Retrying changes nothing.
    Broken,
}

impl ProviderError {
    /// The provider was not reached: no route, no TLS, a timeout.
    pub fn unreachable(kind: &'static str) -> ProviderError {
        ProviderError {
            kind,
            status: None,
            retryable: true,
            bad_response: false,
            retry_after: None,
            source: None,
        }
    }

    /// The provider answered `status`, which was not 2xx.
    pub fn answered(status: u16, retryable: bool) -> ProviderError {
        ProviderError {
            kind: "rejected",
            status: Some(status),
            retryable,
            bad_response: false,
            retry_after: None,
            source: None,
        }
    }

    /// The provider answered 2xx with a document that cannot be used. `why`
    /// is a fixed word.
    pub fn bad_response(why: &'static str) -> ProviderError {
        ProviderError {
            kind: why,
            status: None,
            retryable: true,
            bad_response: true,
            retry_after: None,
            source: None,
        }
    }

    /// A call that cannot succeed as configured (not retryable, no status).
    pub fn broken(kind: &'static str) -> ProviderError {
        ProviderError {
            kind,
            status: None,
            retryable: false,
            bad_response: false,
            retry_after: None,
            source: None,
        }
    }

    /// With the underlying cause, for `source()`.
    pub fn with_source(
        mut self,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> ProviderError {
        self.source = Some(std::sync::Arc::new(source));
        self
    }

    /// With the provider's `Retry-After`.
    pub fn with_retry_after(mut self, after: Option<std::time::Duration>) -> ProviderError {
        self.retry_after = after;
        self
    }

    /// The class. The one question every caller asks.
    pub fn class(&self) -> ProviderClass {
        if self.bad_response {
            ProviderClass::BadResponse
        } else if self.status.is_some() && !self.retryable {
            ProviderClass::Refused
        } else if self.retryable {
            ProviderClass::Busy
        } else {
            ProviderClass::Broken
        }
    }

    /// Is this the provider asking for a later attempt, or unreachable?
    pub fn is_busy(&self) -> bool {
        self.class() == ProviderClass::Busy
    }

    /// Would another attempt be worth making?
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    /// Did the provider answer, and answer no? Only then is the person's
    /// sign-in refused.
    pub fn refused_by_provider(&self) -> bool {
        self.class() == ProviderClass::Refused
    }

    /// The provider's HTTP status, when it answered one.
    pub fn status(&self) -> Option<u16> {
        self.status
    }

    /// How long the provider asked to be left alone.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        self.retry_after
    }

    /// The fixed word for the security stream: `provider-busy` for a
    /// provider that answered "not now" (5xx, 429), else the egress kind.
    pub fn reason(&self) -> &'static str {
        if self.status.is_some() && self.retryable {
            "provider-busy"
        } else {
            self.kind
        }
    }

    /// The one mapping from a provider failure to a sign-in answer.
    /// `refused` names the step whose final no refuses the person (a token
    /// exchange, a userinfo call); `None` for a call about the provider
    /// itself (discovery, keys), where no answer is a refusal of anyone.
    /// Everything else is [`AuthError::UpstreamUnavailable`], which the
    /// callback answers `identity_provider_unavailable`: try again later.
    pub fn into_auth_error(self, refused: Option<&'static str>) -> AuthError {
        match (self.class(), refused) {
            (ProviderClass::Refused, Some(step)) => AuthError::UpstreamRefused(step),
            _ => AuthError::UpstreamUnavailable(self.reason()),
        }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "identity provider call failed: {}", self.reason())?;
        if let Some(s) = self.status {
            write!(f, " (status {s})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

/// How a provider reaches the network. Synchronous: providers run inside
/// `spawn_blocking`, and the host's implementation blocks on the egress
/// kernel's future there. Only [`ProviderClient`] calls it.
pub trait FederationTransport: Send + Sync {
    fn fetch(&self, req: &FederationRequest<'_>) -> Result<FederationResponse, ProviderError>;
}

/// The one client every call to an outside identity provider goes through:
/// discovery, the token exchange, userinfo and the key set. It builds the
/// request, so no provider code assembles a [`FederationRequest`] itself,
/// and it answers one typed error, [`ProviderError`].
#[derive(Clone)]
pub struct ProviderClient {
    transport: std::sync::Arc<dyn FederationTransport>,
}

impl ProviderClient {
    pub fn new(transport: std::sync::Arc<dyn FederationTransport>) -> ProviderClient {
        ProviderClient { transport }
    }

    fn get(
        &self,
        url: &str,
        bearer: Option<&str>,
        cap: usize,
    ) -> Result<FederationResponse, ProviderError> {
        self.transport.fetch(&FederationRequest {
            method: FederationMethod::Get,
            url,
            form: Vec::new(),
            client_secret: None,
            bearer,
            cap,
        })
    }

    /// The discovery document.
    pub fn discovery(&self, url: &str, cap: usize) -> Result<FederationResponse, ProviderError> {
        self.get(url, None, cap)
    }

    /// The key set.
    pub fn jwks(&self, url: &str, cap: usize) -> Result<FederationResponse, ProviderError> {
        self.get(url, None, cap)
    }

    /// The userinfo call, with the access token.
    pub fn userinfo(&self, url: &str, access: &str) -> Result<FederationResponse, ProviderError> {
        self.get(url, Some(access), USERINFO_CAP)
    }

    /// The token exchange; the client secret joins the form per call.
    pub fn token_exchange(
        &self,
        url: &str,
        form: Vec<(&str, &str)>,
        secret: &ClientSecret,
    ) -> Result<FederationResponse, ProviderError> {
        self.transport.fetch(&FederationRequest {
            method: FederationMethod::PostForm,
            url,
            form,
            client_secret: Some(secret),
            bearer: None,
            cap: TOKEN_RESPONSE_CAP,
        })
    }
}

/// Body caps per purpose (§3.4).
pub const TOKEN_RESPONSE_CAP: usize = 64 * 1024;
pub const USERINFO_CAP: usize = 64 * 1024;

/// File the subject a callback verified as a handoff, bound to `binder_hash`,
/// and answer the code the browser carries back.
pub fn write_handoff(
    store: &dyn CeremonyStore,
    connection: &str,
    binder_hash: &str,
    subject: &AuthenticatedSubject,
    now: Timestamp,
) -> Result<String, AuthError> {
    let code = random_secret();
    let a = &subject.attributes;
    let payload = serde_json::json!({
        "a": subject.authenticator.0,
        "m": subject.method.as_str(),
        "d": a.display,
        "v": a.avatar,
        "e": a.email.as_ref().map(|e| e.address().to_string()),
        "ec": a.email.as_ref().map(|e| match e.evidence() {
            EmailEvidence::ProviderClaim { claim } => claim.clone(),
            EmailEvidence::EmailedCode => "ordo:emailed-code".to_string(),
        }),
        // What the provider said about how the person signed in there, kept
        // so the sign-in records it (§8.2), and the two-step claim when it
        // counted under the connection's setting.
        "amr": a.assurance.as_ref().map(|x| x.amr.clone()),
        "acr": a.assurance.as_ref().and_then(|x| x.acr.clone()),
        "at": a.assurance.as_ref().and_then(|x| x.auth_time),
        "up": subject.upstream_presence.as_ref().map(|u| serde_json::json!([u.at().0, u.via()])),
    })
    .to_string();
    store
        .put(&CeremonyRow {
            handle: handle_of(&code),
            connection: connection.to_string(),
            phase: PHASE_HANDOFF.to_string(),
            binder: binder_hash.to_string(),
            payload,
            minted_at: now,
            expires_at: Timestamp(now.0.saturating_add(handoff_ttl_secs())),
        })
        .map_err(|e| AuthError::StoreUnavailable(CredentialStoreError::from(e)))?;
    Ok(code)
}

/// Redeem a handoff with the binder the page holds. Unknown, expired, spent,
/// for another connection, or presented without its binder are one answer.
pub fn redeem_handoff(
    store: &dyn CeremonyStore,
    connection: &str,
    code: &str,
    binder: &str,
    now: Timestamp,
) -> Result<AuthenticatedSubject, AuthError> {
    if code.is_empty() || binder.is_empty() {
        return Err(AuthError::BadCredentials);
    }
    let payload = store
        .take_bound(
            &handle_of(code),
            connection,
            PHASE_HANDOFF,
            &handle_of(binder),
            now,
        )
        .map_err(|e| AuthError::StoreUnavailable(CredentialStoreError::from(e)))?
        .ok_or(AuthError::BadCredentials)?;
    subject_from_payload(&payload).ok_or(AuthError::BadCredentials)
}

fn subject_from_payload(payload: &str) -> Option<AuthenticatedSubject> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let method = match text("m")?.as_str() {
        "oauth2" => AuthMethodKind::OAuth2,
        "oidc" => AuthMethodKind::Oidc,
        "saml" => AuthMethodKind::Saml,
        _ => return None,
    };
    let email = match (text("e"), text("ec")) {
        (Some(address), Some(claim)) => {
            VerifiedEmail::new(&address, EmailEvidence::ProviderClaim { claim })
        }
        _ => None,
    };
    let assurance = v
        .get("amr")
        .and_then(|x| x.as_array())
        .map(|amr| UpstreamAssurance {
            amr: amr
                .iter()
                .filter_map(|m| m.as_str().map(str::to_string))
                .collect(),
            acr: text("acr"),
            auth_time: v.get("at").and_then(|x| x.as_u64()),
        });
    // The claim is both halves or none. A row this code did not write
    // carries nothing.
    let upstream = v.get("up").and_then(|x| x.as_array()).and_then(|pair| {
        match (
            pair.first().and_then(|x| x.as_u64()),
            pair.get(1).and_then(|x| x.as_str()),
        ) {
            (Some(at), Some(via)) => crate::upstream::UpstreamPresence::new(Timestamp(at), via),
            _ => None,
        }
    });
    Some(
        AuthenticatedSubject::without_presence(AuthenticatorId(text("a")?), method)
            .with_attributes(SubjectAttributes {
                email,
                display: text("d"),
                avatar: text("v"),
                assurance,
            })
            .with_upstream_presence(upstream),
    )
}

/// `application/x-www-form-urlencoded`, for a query string or a form body.
pub fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent(k), percent(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode everything but RFC 3986's unreserved set.
pub fn percent(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode one `application/x-www-form-urlencoded` value. `None` on a
/// malformed escape or bytes that are not UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ceremony::MemoryCeremonyStore;

    #[test]
    fn a_provider_error_is_sorted_by_class_and_keeps_its_cause() {
        use std::error::Error as _;
        let busy = ProviderError::answered(503, true);
        assert_eq!(busy.class(), ProviderClass::Busy);
        assert!(busy.is_busy() && busy.is_retryable() && !busy.refused_by_provider());
        assert_eq!(busy.status(), Some(503));
        assert_eq!(
            busy.into_auth_error(Some("x")),
            AuthError::UpstreamUnavailable("provider-busy")
        );
        let no = ProviderError::answered(401, false);
        assert_eq!(no.class(), ProviderClass::Refused);
        assert_eq!(
            no.clone().into_auth_error(Some("step")),
            AuthError::UpstreamRefused("step")
        );
        // A call about the provider itself never refuses a person.
        assert_eq!(
            no.into_auth_error(None),
            AuthError::UpstreamUnavailable("rejected")
        );
        assert_eq!(
            ProviderError::bad_response("discovery-issuer").class(),
            ProviderClass::BadResponse
        );
        let broken = ProviderError::broken("denied_address");
        assert_eq!(broken.class(), ProviderClass::Broken);
        assert!(!broken.is_retryable());
        let cause = std::io::Error::other("boom");
        let e = ProviderError::unreachable("connect_failed")
            .with_source(cause)
            .with_retry_after(Some(std::time::Duration::from_secs(7)));
        assert_eq!(e.retry_after(), Some(std::time::Duration::from_secs(7)));
        assert_eq!(e.source().expect("source").to_string(), "boom");
    }

    fn subject() -> AuthenticatedSubject {
        AuthenticatedSubject::without_presence(
            AuthenticatorId("oauth2:discord:80351110224678912".into()),
            AuthMethodKind::OAuth2,
        )
        .with_attributes(SubjectAttributes {
            email: VerifiedEmail::new(
                "nelly@example.com",
                EmailEvidence::ProviderClaim {
                    claim: "verified".into(),
                },
            ),
            display: Some("Nelly".into()),
            avatar: None,
            assurance: None,
        })
    }

    #[test]
    fn a_handoff_round_trips_the_subject_with_its_binder() {
        let store = MemoryCeremonyStore::new();
        let binder = random_secret();
        let code = write_handoff(
            &store,
            "discord",
            &handle_of(&binder),
            &subject(),
            Timestamp(100),
        )
        .expect("filed");
        let back =
            redeem_handoff(&store, "discord", &code, &binder, Timestamp(101)).expect("redeemed");
        assert_eq!(back, subject());
        assert_eq!(
            redeem_handoff(&store, "discord", &code, &binder, Timestamp(101)),
            Err(AuthError::BadCredentials),
            "one-shot"
        );
    }

    #[test]
    fn a_handoff_without_its_binder_is_refused_and_not_burned() {
        let store = MemoryCeremonyStore::new();
        let binder = random_secret();
        let code = write_handoff(
            &store,
            "discord",
            &handle_of(&binder),
            &subject(),
            Timestamp(100),
        )
        .expect("filed");
        for wrong in ["", "another-browsers-binder"] {
            assert_eq!(
                redeem_handoff(&store, "discord", &code, wrong, Timestamp(101)),
                Err(AuthError::BadCredentials)
            );
        }
        assert_eq!(
            redeem_handoff(&store, "github", &code, &binder, Timestamp(101)),
            Err(AuthError::BadCredentials),
            "another connection's route cannot redeem it"
        );
        assert!(redeem_handoff(&store, "discord", &code, &binder, Timestamp(101)).is_ok());
    }

    #[test]
    fn a_handoff_lives_sixty_seconds() {
        let store = MemoryCeremonyStore::new();
        let binder = random_secret();
        let code = write_handoff(
            &store,
            "discord",
            &handle_of(&binder),
            &subject(),
            Timestamp(100),
        )
        .expect("filed");
        assert_eq!(
            redeem_handoff(
                &store,
                "discord",
                &code,
                &binder,
                Timestamp(100 + HANDOFF_TTL_SECS)
            ),
            Err(AuthError::BadCredentials)
        );
    }

    #[test]
    fn form_encoding_round_trips() {
        let raw = "a b&c=d/é+%";
        let enc = percent(raw);
        assert!(!enc.contains(' ') && !enc.contains('&') && !enc.contains('='));
        assert_eq!(percent_decode(&enc).as_deref(), Some(raw));
        assert_eq!(percent_decode("%zz"), None);
        assert_eq!(form_encode(&[("a", "1 2"), ("b", "x")]), "a=1%202&b=x");
    }
}
