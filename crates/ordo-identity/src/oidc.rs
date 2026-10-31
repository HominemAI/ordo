//! OIDC sign-in: Google, Microsoft, and any provider that publishes a
//! discovery document (`plan-auth-federated.md` §4, §5.3).
//!
//! The flow is OAuth2's ([`crate::oauth2`]) with three differences: `begin`
//! also mints a `nonce`, the scope carries `openid`, and the subject comes
//! from an **ID token verified here**, not from a userinfo call. The token's
//! signature is checked by the pure verifier ([`crate::jose`]) against the
//! issuer's published keys, which this provider fetches and caches.
//!
//! # The pins (§4.3)
//!
//! | # | Pin | Where |
//! |---|---|---|
//! | O1 | code flow, PKCE `S256`; a callback carrying `id_token` or `access_token` is refused | [`OidcProvider::begin`], [`OidcProvider::callback`] |
//! | O2 | `state` 256 bits, one-shot, five minutes, stored hashed, bound to the binder | the ceremony row, as OAuth2 |
//! | O3 | the ID token's `nonce` equals the ceremony's exactly | [`check_id_token`] |
//! | O4 | `iss` equals the configured issuer byte for byte | [`check_id_token`] |
//! | O5 | `aud` holds the client id; several audiences need `azp` equal to it | [`check_id_token`] |
//! | O6 | `exp` in the future, `iat` and `nbf` not ahead, within the skew (60 s, 0 to 120) | [`check_id_token`] |
//! | O7, O8, O17 | algorithm allow-list, key by `kid` from the JWKS only, one parse of verified bytes | [`crate::jose`] |
//! | O9 | keys cached for `ORDO_AUTH_OIDC_CACHE_SECS` (an hour), shortened by the provider's `max-age`; an unknown `kid` refetches at once, at most once a minute; a key gone from the set stops verifying at the next fetch | [`OidcProvider`]'s key cache |
//! | O10 | an unreachable JWKS with a warm cache verifies against it for 24 hours past expiry and says so; a cold cache is `identity_provider_unavailable` | the key cache, [`OidcOutcome::stale_keys`] |
//! | O11 | `email` only when `email_verified` is the JSON boolean `true` | [`attributes_of`] |
//! | O12 | no email links anything | the directory, unchanged |
//! | O13 | one redirect URI, built at boot, sent at `begin` and at the exchange | [`OidcConnection::redirect_uri`] |
//! | O14 | the callback path, the ceremony row and the token endpoint are one connection; the `iss` response parameter must match, and is required where discovery advertises it | [`OidcProvider::callback`] |
//! | O15 | discovery's `issuer` equals the configured one and every endpoint is `https`; the document is re-read on the cache's schedule, and a failed refresh keeps the last good copy and says so | [`parse_discovery`], [`OidcOutcome::stale_discovery`] |
//! | O16 | the client secret is in no log, refusal or event | [`crate::federation::ClientSecret`] |
//!
//! `at_hash` is not checked: the access token is never used (§4.1), so there
//! is nothing for it to bind.

use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use crate::ceremony::{CeremonyRow, CeremonyStore};
use crate::federation::{
    form_encode, handle_of, random_secret, redeem_handoff, write_handoff, ClientSecret,
    FederationTransport, ProviderClient, ProviderError, PHASE_BEGUN,
};
use crate::jose::{self, Alg, Jwks};
use crate::oauth2::{code_challenge, host_of, valid_connection_name};
use crate::principal::{AuthMethodKind, Timestamp};
use crate::provider::{
    AuthError, AuthProvider, AuthenticatedSubject, AuthenticatorId, BeginRequest,
    ChallengeOrRedirect, CompleteRequest, EmailEvidence, SubjectAttributes, UpstreamAssurance,
    VerifiedEmail, CEREMONY_TTL_SECS,
};
use crate::CredentialStoreError;

/// The default clock skew, each direction (O6).
pub const DEFAULT_SKEW_SECS: u64 = 60;
/// The largest skew a setting may ask for.
pub const MAX_SKEW_SECS: u64 = 120;
/// How long a fetched JWKS or discovery document is fresh, by default
/// (O9). `ORDO_AUTH_OIDC_CACHE_SECS`; a provider's `Cache-Control: max-age`
/// shortens it, never lengthens it.
pub const DEFAULT_CACHE_SECS: u64 = 3600;
/// The bounds of that setting. The floor is also the shortest freshness a
/// provider's `max-age` (or `no-store`) can impose, so a provider cannot
/// make every sign-in a fetch.
pub const MIN_CACHE_SECS: u64 = 300;
pub const MAX_CACHE_SECS: u64 = 86_400;
/// The least time between two fetches of one connection's JWKS, or of its
/// discovery document (O9). A token under a `kid` the cache lacks refetches
/// at once unless a fetch happened inside this minute. Fixed by the plan
/// (O9), not a setting: it bounds how hard a stream of unknown `kid`s can
/// drive the deployment at the issuer to one request a minute, and a key
/// rotation is seen within a minute of its first token.
pub const JWKS_REFETCH_FLOOR_SECS: u64 = 60;
/// How long past freshness a cached JWKS still verifies while the issuer
/// cannot be reached (O10), by default. `ORDO_AUTH_OIDC_STALE_GRACE_SECS`
/// sets it per deployment, 0 to [`MAX_STALE_GRACE_SECS`].
pub const JWKS_STALE_GRACE_SECS: u64 = 24 * 3600;
/// The longest stale grace a deployment may set: 7 days.
pub const MAX_STALE_GRACE_SECS: u64 = 7 * 24 * 3600;
/// Body caps (§3.4).
pub const DISCOVERY_CAP: usize = 256 * 1024;
pub const JWKS_CAP: usize = 256 * 1024;

/// The endpoints a connection uses. From discovery, or pinned by settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcEndpoints {
    pub authorize_url: String,
    pub token_url: String,
    pub jwks_url: String,
    /// RFC 9207: the provider sends `iss` with every authorization response,
    /// so a response without one is refused (O14).
    pub iss_parameter_required: bool,
}

impl OidcEndpoints {
    /// The hosts the transport may reach once the endpoints are known: the
    /// token endpoint and the JWKS. The authorize endpoint is the browser's.
    pub fn hosts(&self) -> Vec<String> {
        let mut out: Vec<String> = [&self.token_url, &self.jwks_url]
            .iter()
            .filter_map(|u| host_of(u))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// A connection's settings.
pub struct OidcConnection {
    /// `[a-z0-9-]{1,32}`, what a page passes to `begin`.
    pub name: String,
    /// Compared byte for byte with discovery's `issuer` and every token's
    /// `iss`. One concrete issuer: a template is refused.
    pub issuer: String,
    pub client_id: String,
    pub client_secret: ClientSecret,
    /// Space-separated; holds `openid`.
    pub scope: String,
    /// `Some` pins the endpoints and skips discovery.
    pub endpoints: Option<OidcEndpoints>,
    /// Keep a verified address (O11). Off for Microsoft, whose `email` is not
    /// verified for every account type.
    pub use_email: bool,
    /// The display name is the first of these that is a non-empty string.
    pub display_claims: Vec<String>,
    /// A claim holding an `https` avatar URL.
    pub avatar_claim: Option<String>,
    /// O6's skew, 0 to 120.
    pub skew_secs: u64,
    /// O7's allow-list.
    pub algs: Vec<Alg>,
    /// How long the JWKS and the discovery document are fresh:
    /// [`MIN_CACHE_SECS`] to [`MAX_CACHE_SECS`], default
    /// [`DEFAULT_CACHE_SECS`]. A shorter `max-age` from the provider wins.
    pub cache_secs: u64,
    /// How long past freshness the cached keys still verify while the
    /// issuer cannot be reached: 0 to [`MAX_STALE_GRACE_SECS`], default
    /// [`JWKS_STALE_GRACE_SECS`].
    pub stale_grace_secs: u64,
    /// `<ORDO_PUBLIC_URL>/auth/callback/<name>` (O13).
    pub redirect_uri: String,
    /// Where a callback may send the browser back to, by origin.
    pub returns: crate::origin::OriginList,
    /// Whether this connection's two-step claim may count as presence, and
    /// what counts (`plan-auth-federated.md` §8.2). Off by default.
    pub presence: crate::upstream::PresenceRule,
}

/// Why a connection was refused at boot. Names a setting, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OidcConnectionError {
    BadName,
    Missing(&'static str),
    NotHttps(&'static str),
    /// The issuer is a template or a multi-tenant endpoint (§4.2).
    TemplateIssuer,
    /// The scope does not ask for `openid`, so no ID token would come.
    ScopeWithoutOpenid,
    SkewOutOfRange,
    CacheOutOfRange,
    StaleGraceOutOfRange,
    NoAlgorithms,
}

impl std::fmt::Display for OidcConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadName => f.write_str("a connection name is 1 to 32 of a-z, 0-9 and -"),
            Self::Missing(what) => write!(f, "{what} is required"),
            Self::NotHttps(what) => write!(f, "{what} must be an https:// URL"),
            Self::TemplateIssuer => f.write_str(
                "the issuer must name one concrete issuer: a template (`{tenantid}`) or \
                 Microsoft's `common`, `organizations` or `consumers` would accept tokens \
                 from every tenant; name one tenant id",
            ),
            Self::ScopeWithoutOpenid => f.write_str("the scope must include `openid`"),
            Self::SkewOutOfRange => write!(f, "the clock skew is 0 to {MAX_SKEW_SECS} seconds"),
            Self::CacheOutOfRange => write!(
                f,
                "the key and discovery cache is {MIN_CACHE_SECS} to {MAX_CACHE_SECS} seconds"
            ),
            Self::StaleGraceOutOfRange => write!(
                f,
                "the stale key grace is 0 to {MAX_STALE_GRACE_SECS} seconds"
            ),
            Self::NoAlgorithms => f.write_str("the algorithm list is empty"),
        }
    }
}

impl std::error::Error for OidcConnectionError {}

fn https(url: &str) -> bool {
    url.len() > "https://".len()
        && url[..8].eq_ignore_ascii_case("https://")
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
        && !url.contains('#')
}

/// Is this issuer one concrete issuer? Refuses templates and Microsoft's
/// multi-tenant paths (§4.2).
pub fn concrete_issuer(issuer: &str) -> bool {
    if issuer.contains('{') || issuer.contains('}') || issuer.contains('?') {
        return false;
    }
    let microsoft = host_of(issuer).is_some_and(|h| h == "login.microsoftonline.com");
    let multi = ["common", "organizations", "consumers"];
    !(microsoft
        && issuer[8..]
            .split('/')
            .any(|seg| multi.iter().any(|m| seg.eq_ignore_ascii_case(m))))
}

impl OidcConnection {
    /// Refuse a connection that cannot be served safely. Called at boot.
    pub fn check(&self) -> Result<(), OidcConnectionError> {
        if !valid_connection_name(&self.name) {
            return Err(OidcConnectionError::BadName);
        }
        if self.client_id.trim().is_empty() {
            return Err(OidcConnectionError::Missing("the client id"));
        }
        if self.client_secret.is_empty() {
            return Err(OidcConnectionError::Missing("the client secret"));
        }
        if self.issuer.trim().is_empty() {
            return Err(OidcConnectionError::Missing("the issuer"));
        }
        if !https(&self.issuer) {
            return Err(OidcConnectionError::NotHttps("the issuer"));
        }
        if !concrete_issuer(&self.issuer) {
            return Err(OidcConnectionError::TemplateIssuer);
        }
        if !self.scope.split(' ').any(|s| s == "openid") {
            return Err(OidcConnectionError::ScopeWithoutOpenid);
        }
        if self.skew_secs > MAX_SKEW_SECS {
            return Err(OidcConnectionError::SkewOutOfRange);
        }
        if self.algs.is_empty() {
            return Err(OidcConnectionError::NoAlgorithms);
        }
        if !(MIN_CACHE_SECS..=MAX_CACHE_SECS).contains(&self.cache_secs) {
            return Err(OidcConnectionError::CacheOutOfRange);
        }
        if self.stale_grace_secs > MAX_STALE_GRACE_SECS {
            return Err(OidcConnectionError::StaleGraceOutOfRange);
        }
        if !https(&self.redirect_uri) {
            return Err(OidcConnectionError::NotHttps("the redirect URI"));
        }
        if let Some(e) = &self.endpoints {
            for (what, url) in [
                ("the authorization endpoint", &e.authorize_url),
                ("the token endpoint", &e.token_url),
                ("the JWKS endpoint", &e.jwks_url),
            ] {
                if !https(url) {
                    return Err(OidcConnectionError::NotHttps(what));
                }
            }
        }
        if self.returns.is_empty() {
            return Err(OidcConnectionError::Missing("ORDO_AUTH_RETURN_URLS"));
        }
        Ok(())
    }

    /// What identifies this connection's subject space: the issuer. A boot
    /// that finds another issuer under the same name refuses (§3.5).
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest as _, Sha256};
        let mut h = Sha256::new();
        for part in ["oidc", self.issuer.as_str()] {
            h.update((part.len() as u64).to_be_bytes());
            h.update(part.as_bytes());
        }
        crate::webauthn::b64url_encode(&h.finalize())
    }

    /// Where discovery is read: `<issuer>/.well-known/openid-configuration`.
    pub fn discovery_url(&self) -> String {
        format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        )
    }
}

/// What an OIDC preset fills: the claims rules, and Google's issuer.
/// Microsoft's issuer is built from the tenant id by [`microsoft_issuer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcPreset {
    pub issuer: Option<&'static str>,
    pub scope: &'static str,
    pub use_email: bool,
    pub display_claims: &'static [&'static str],
    pub avatar_claim: Option<&'static str>,
    /// The preset needs a tenant id (`_TENANT`).
    pub needs_tenant: bool,
}

/// The OIDC presets (§5.3).
pub fn oidc_preset(name: &str) -> Option<OidcPreset> {
    match name {
        "google" => Some(OidcPreset {
            issuer: Some("https://accounts.google.com"),
            scope: "openid email profile",
            use_email: true,
            display_claims: &["name"],
            avatar_claim: Some("picture"),
            needs_tenant: false,
        }),
        // Entra's `email` is not verified for every account type, so it is
        // not used. One tenant per connection.
        "microsoft" => Some(OidcPreset {
            issuer: None,
            scope: "openid profile",
            use_email: false,
            display_claims: &["name"],
            avatar_claim: None,
            needs_tenant: true,
        }),
        _ => None,
    }
}

/// The names [`oidc_preset`] knows.
pub const OIDC_PRESETS: [&str; 2] = ["google", "microsoft"];

/// The display claims of a company login with no preset.
pub const PLAIN_DISPLAY_CLAIMS: [&str; 2] = ["name", "preferred_username"];

/// Microsoft's v2 issuer for one tenant. The tenant must be its id, a GUID:
/// a domain name would be answered by a discovery document whose issuer is
/// the GUID, which O15 then refuses.
pub fn microsoft_issuer(tenant: &str) -> Option<String> {
    let t = tenant.trim().to_ascii_lowercase();
    let groups: Vec<&str> = t.split('-').collect();
    let guid = groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.bytes().all(|b| b.is_ascii_hexdigit()));
    guid.then(|| format!("https://login.microsoftonline.com/{t}/v2.0"))
}

/// O15: read a discovery document, and refuse it unless its `issuer` is the
/// configured one byte for byte and every endpoint is `https`.
pub fn parse_discovery(bytes: &[u8], issuer: &str) -> Result<OidcEndpoints, &'static str> {
    let v = jose::strict_json(bytes).map_err(|_| "discovery-not-json")?;
    let text = |k: &str| v.get(k).and_then(Value::as_str);
    if text("issuer") != Some(issuer) {
        return Err("discovery-issuer-mismatch");
    }
    let endpoint = |k: &str| -> Result<String, &'static str> {
        let url = text(k).ok_or("discovery-endpoint-missing")?;
        if !https(url) {
            return Err("discovery-endpoint-not-https");
        }
        Ok(url.to_string())
    };
    let listed = |k: &str, want: &str| -> bool {
        match v.get(k).and_then(Value::as_array) {
            None => true,
            Some(list) => list.iter().any(|x| x.as_str() == Some(want)),
        }
    };
    if !listed("response_types_supported", "code") {
        return Err("discovery-no-code-flow");
    }
    if !listed("code_challenge_methods_supported", "S256") {
        return Err("discovery-no-s256");
    }
    Ok(OidcEndpoints {
        authorize_url: endpoint("authorization_endpoint")?,
        token_url: endpoint("token_endpoint")?,
        jwks_url: endpoint("jwks_uri")?,
        iss_parameter_required: v.get("authorization_response_iss_parameter_supported")
            == Some(&Value::Bool(true)),
    })
}

/// What an ID token's claims must satisfy.
pub struct IdTokenRules<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub nonce: &'a str,
    pub skew_secs: u64,
}

/// A NumericDate: an integer, or a finite number taken at its floor.
fn date(v: Option<&Value>) -> Option<i128> {
    let n = v?.as_number()?;
    if let Some(i) = n.as_i64() {
        return Some(i as i128);
    }
    if let Some(u) = n.as_u64() {
        return Some(u as i128);
    }
    n.as_f64()
        .filter(|f| f.is_finite() && f.abs() < 1e15)
        .map(|f| f.floor() as i128)
}

/// O3 to O6: check an ID token's claims, answer its `sub`. Pure. Each
/// refusal is a fixed word.
pub fn check_id_token(
    claims: &Map<String, Value>,
    r: &IdTokenRules<'_>,
    now: Timestamp,
) -> Result<String, &'static str> {
    // O4: byte for byte, no normalising.
    if claims.get("iss").and_then(Value::as_str) != Some(r.issuer) {
        return Err("id-token-issuer");
    }
    // O5: `aud` names this client. With several audiences `azp` must be
    // present and be this client; wherever `azp` appears it must be.
    let azp = claims.get("azp");
    match claims.get("aud") {
        Some(Value::String(a)) if a == r.client_id => {}
        Some(Value::Array(list)) => {
            let ours = list.iter().any(|a| a.as_str() == Some(r.client_id));
            let all_text = list.iter().all(Value::is_string);
            if !ours || !all_text {
                return Err("id-token-audience");
            }
            if list.len() > 1 && azp.and_then(Value::as_str) != Some(r.client_id) {
                return Err("id-token-azp");
            }
        }
        _ => return Err("id-token-audience"),
    }
    if azp.is_some_and(|a| a.as_str() != Some(r.client_id)) {
        return Err("id-token-azp");
    }
    // O6: the skew, each way.
    let now = now.0 as i128;
    let skew = r.skew_secs as i128;
    let exp = date(claims.get("exp")).ok_or("id-token-exp-missing")?;
    if now >= exp + skew {
        return Err("id-token-expired");
    }
    let iat = date(claims.get("iat")).ok_or("id-token-iat-missing")?;
    if iat > now + skew {
        return Err("id-token-issued-in-the-future");
    }
    if claims.contains_key("nbf") {
        let nbf = date(claims.get("nbf")).ok_or("id-token-nbf")?;
        if nbf > now + skew {
            return Err("id-token-not-yet-valid");
        }
    }
    // O3: exactly the nonce this ceremony minted.
    if claims.get("nonce").and_then(Value::as_str) != Some(r.nonce) {
        return Err("id-token-nonce");
    }
    // `sub`: an exact, case-sensitive string (§4.2).
    match claims.get("sub") {
        Some(Value::String(s))
            if !s.is_empty() && s.len() <= 255 && !s.chars().any(char::is_control) =>
        {
            Ok(s.clone())
        }
        _ => Err("id-token-sub"),
    }
}

/// What the connection keeps from the claims. Nothing here decides who the
/// person is.
pub fn attributes_of(conn: &OidcConnection, claims: &Map<String, Value>) -> SubjectAttributes {
    let text = |k: &str| {
        claims
            .get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let display = conn.display_claims.iter().find_map(|c| text(c)).map(|d| {
        d.chars()
            .filter(|c| !c.is_control())
            .take(120)
            .collect::<String>()
    });
    let avatar = conn
        .avatar_claim
        .as_deref()
        .and_then(text)
        .filter(|u| https(u) && u.len() <= 2048)
        .map(str::to_string);
    // O11: the JSON boolean `true` and nothing else. Absent, `false` and the
    // string "true" are all unverified, and an unverified address is not kept.
    let email = if conn.use_email && claims.get("email_verified") == Some(&Value::Bool(true)) {
        text("email").and_then(|address| {
            VerifiedEmail::new(
                address,
                EmailEvidence::ProviderClaim {
                    claim: "email_verified".into(),
                },
            )
        })
    } else {
        None
    };
    let amr = claims
        .get("amr")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let assurance = UpstreamAssurance {
        amr,
        acr: text("acr").map(str::to_string),
        auth_time: claims.get("auth_time").and_then(Value::as_u64),
    };
    SubjectAttributes {
        email,
        display,
        avatar,
        assurance: Some(assurance),
    }
}

/// Builds a transport whose allowlist is exactly the hosts given. The host
/// implements it over the egress kernel; discovery gets the issuer's host,
/// and what discovery names gets its own transport.
pub trait TransportFactory: Send + Sync {
    fn for_hosts(&self, hosts: &[String]) -> Arc<dyn FederationTransport>;
}

/// The return leg's query, as far as this provider reads it.
#[derive(Debug, Clone, Default)]
pub struct OidcCallback<'a> {
    pub state: &'a str,
    pub code: Option<&'a str>,
    pub error: Option<&'a str>,
    /// RFC 9207's `iss`.
    pub iss: Option<&'a str>,
    /// The query carried `id_token` or `access_token` (O1).
    pub front_channel_token: bool,
}

/// What a callback came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcOutcome {
    /// `None` when no ceremony matched, so there is nowhere to send the
    /// browser.
    pub return_to: Option<String>,
    pub result: Result<String, AuthError>,
    /// The token verified against a JWKS past its freshness because the
    /// issuer could not be reached (O10). The route records it.
    pub stale_keys: bool,
    /// The endpoints in use are a discovery document past its freshness,
    /// kept because a refresh failed. The route records it.
    pub stale_discovery: bool,
    /// The sign-in asked for presence and the claim minted none: a fixed
    /// word from [`crate::upstream::PresenceRule::judge`]. The sign-in
    /// itself stands. The route records it.
    pub presence_not_minted: Option<&'static str>,
}

struct Begun {
    verifier: String,
    nonce: String,
    return_to: String,
    binder_hash: String,
    /// The authorize request carried `max_age=0`: the page asked for a
    /// two-step claim that may count as presence.
    presence: bool,
}

impl Begun {
    fn render(&self) -> String {
        serde_json::json!({"v": self.verifier, "n": self.nonce, "r": self.return_to,
                           "b": self.binder_hash, "p": self.presence})
        .to_string()
    }

    fn parse(payload: &str) -> Option<Begun> {
        let v: Value = serde_json::from_str(payload).ok()?;
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Begun {
            verifier: text("v")?,
            nonce: text("n")?,
            return_to: text("r")?,
            binder_hash: text("b")?,
            presence: v.get("p").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

struct KeyCache {
    keys: Jwks,
    fresh_until: u64,
}

struct Discovered {
    endpoints: OidcEndpoints,
    hosts: Vec<String>,
    transport: ProviderClient,
    fresh_until: u64,
}

#[derive(Default)]
struct Live {
    endpoints: Option<Discovered>,
    discovery_tried_at: Option<u64>,
    /// Why the last discovery attempt failed, while no copy is held.
    discovery_failed: Option<ProviderError>,
    keys: Option<KeyCache>,
    keys_tried_at: Option<u64>,
}

/// The OIDC provider for one connection.
pub struct OidcProvider {
    conn: OidcConnection,
    ceremonies: Arc<dyn CeremonyStore>,
    transports: Arc<dyn TransportFactory>,
    /// Endpoints and keys, per process. Held across a fetch, so two
    /// callbacks that both miss a `kid` make one request.
    live: Mutex<Live>,
}

/// Is a fetch due? Never twice inside [`JWKS_REFETCH_FLOOR_SECS`].
fn spaced(tried_at: Option<u64>, now: Timestamp) -> bool {
    tried_at.is_none_or(|at| now.0.saturating_sub(at) >= JWKS_REFETCH_FLOOR_SECS)
}

/// Endpoints in use: what they are, the transport that reaches them, and
/// whether they are past their freshness.
type Endpoints = (OidcEndpoints, ProviderClient, bool);

impl OidcProvider {
    pub fn new(
        conn: OidcConnection,
        ceremonies: Arc<dyn CeremonyStore>,
        transports: Arc<dyn TransportFactory>,
    ) -> Result<OidcProvider, OidcConnectionError> {
        conn.check()?;
        Ok(OidcProvider {
            conn,
            ceremonies,
            transports,
            live: Mutex::new(Live::default()),
        })
    }

    pub fn connection(&self) -> &OidcConnection {
        &self.conn
    }

    pub fn name(&self) -> &str {
        &self.conn.name
    }

    /// The provider client over a transport that reaches exactly `hosts`.
    fn client_for(&self, hosts: &[String]) -> ProviderClient {
        ProviderClient::new(self.transports.for_hosts(hosts))
    }

    /// How long an answer is fresh: the provider's `max-age` when it gave
    /// one, bounded by [`MIN_CACHE_SECS`] and the connection's setting.
    fn freshness(&self, max_age: Option<u64>) -> u64 {
        max_age
            .unwrap_or(self.conn.cache_secs)
            .clamp(MIN_CACHE_SECS, self.conn.cache_secs)
    }

    /// The endpoints. Pinned ones never change. Discovered ones are read on
    /// first use and **re-read on the cache's schedule**, as the JWKS is. A
    /// refresh that fails keeps the last good copy, which the callback
    /// reports as stale (O15: every copy is checked against the configured
    /// issuer before it is used).
    fn endpoints(&self, live: &mut Live, now: Timestamp) -> Result<Endpoints, AuthError> {
        if let Some(pinned) = &self.conn.endpoints {
            let d = live.endpoints.get_or_insert_with(|| Discovered {
                hosts: pinned.hosts(),
                transport: self.client_for(&pinned.hosts()),
                endpoints: pinned.clone(),
                fresh_until: u64::MAX,
            });
            return Ok((d.endpoints.clone(), d.transport.clone(), false));
        }
        let due = live
            .endpoints
            .as_ref()
            .is_none_or(|d| now.0 >= d.fresh_until);
        if due && spaced(live.discovery_tried_at, now) {
            live.discovery_tried_at = Some(now.0);
            match self.discover() {
                Ok((endpoints, max_age)) => {
                    let hosts = endpoints.hosts();
                    let transport = match &live.endpoints {
                        Some(d) if d.hosts == hosts => d.transport.clone(),
                        _ => self.client_for(&hosts),
                    };
                    live.discovery_failed = None;
                    live.endpoints = Some(Discovered {
                        endpoints,
                        hosts,
                        transport,
                        fresh_until: now.0.saturating_add(self.freshness(max_age)),
                    });
                }
                Err(why) => live.discovery_failed = Some(why),
            }
        }
        match &live.endpoints {
            Some(d) => Ok((
                d.endpoints.clone(),
                d.transport.clone(),
                now.0 >= d.fresh_until,
            )),
            None => Err(match &live.discovery_failed {
                Some(why) => why.clone().into_auth_error(None),
                None => AuthError::UpstreamUnavailable("discovery-not-loaded"),
            }),
        }
    }

    fn discover(&self) -> Result<(OidcEndpoints, Option<u64>), ProviderError> {
        let url = self.conn.discovery_url();
        let host = host_of(&url).ok_or(ProviderError::broken("discovery-url"))?;
        let client = self.client_for(&[host]);
        let got = client.discovery(&url, DISCOVERY_CAP)?;
        let endpoints =
            parse_discovery(got.bytes(), &self.conn.issuer).map_err(ProviderError::bad_response)?;
        Ok((endpoints, got.max_age()))
    }

    fn fetch_keys(
        &self,
        transport: &ProviderClient,
        url: &str,
    ) -> Result<(Jwks, Option<u64>), ProviderError> {
        let got = transport.jwks(url, JWKS_CAP)?;
        let keys = Jwks::parse(got.bytes()).map_err(|e| ProviderError::bad_response(e.0))?;
        Ok((keys, got.max_age()))
    }

    /// O9 and O10: the keys to verify a token whose header names `kid` (or
    /// none), and whether they are past their freshness. A `kid` the cache
    /// lacks refetches at once, subject to the one-a-minute floor.
    fn keys_for(
        &self,
        live: &mut Live,
        transport: &ProviderClient,
        jwks_url: &str,
        kid: Option<&str>,
        now: Timestamp,
    ) -> Result<(Jwks, bool), AuthError> {
        let want = match &live.keys {
            None => true,
            Some(c) => now.0 >= c.fresh_until || kid.is_some_and(|k| !c.keys.has(k)),
        };
        if want && spaced(live.keys_tried_at, now) {
            live.keys_tried_at = Some(now.0);
            match self.fetch_keys(transport, jwks_url) {
                Ok((keys, max_age)) => {
                    live.keys = Some(KeyCache {
                        keys,
                        fresh_until: now.0.saturating_add(self.freshness(max_age)),
                    })
                }
                Err(e) if live.keys.is_none() => return Err(e.into_auth_error(None)),
                // A warm cache rides out the outage, below.
                Err(_) => {}
            }
        }
        let Some(cache) = &live.keys else {
            return Err(AuthError::UpstreamUnavailable("jwks-not-loaded"));
        };
        if now.0 >= cache.fresh_until.saturating_add(self.conn.stale_grace_secs) {
            return Err(AuthError::UpstreamUnavailable("jwks-expired"));
        }
        Ok((cache.keys.clone(), now.0 >= cache.fresh_until))
    }

    /// The return leg: consume the ceremony, exchange the code, verify the ID
    /// token, file a handoff.
    pub fn callback(&self, q: &OidcCallback<'_>, now: Timestamp) -> OidcOutcome {
        let none = |result| OidcOutcome {
            return_to: None,
            result,
            stale_keys: false,
            stale_discovery: false,
            presence_not_minted: None,
        };
        let taken =
            match self
                .ceremonies
                .take(&handle_of(q.state), &self.conn.name, PHASE_BEGUN, now)
            {
                Ok(Some(p)) => p,
                Ok(None) => return none(Err(AuthError::StateMismatch(String::new()))),
                Err(e) => {
                    return none(Err(AuthError::StoreUnavailable(
                        CredentialStoreError::from(e),
                    )))
                }
            };
        let Some(begun) = Begun::parse(&taken) else {
            return none(Err(AuthError::StateMismatch(String::new())));
        };
        let mut stale = (false, false);
        let mut not_minted = None;
        let result = self.finish(&begun, q, now, &mut stale, &mut not_minted);
        OidcOutcome {
            return_to: Some(begun.return_to),
            result,
            stale_keys: stale.0,
            stale_discovery: stale.1,
            presence_not_minted: not_minted,
        }
    }

    fn finish(
        &self,
        begun: &Begun,
        q: &OidcCallback<'_>,
        now: Timestamp,
        stale: &mut (bool, bool),
        not_minted: &mut Option<&'static str>,
    ) -> Result<String, AuthError> {
        // O1: a token in the front channel means a flow this connection never
        // asked for.
        if q.front_channel_token {
            return Err(AuthError::UpstreamRefused("front-channel-token"));
        }
        if q.error.is_some() {
            return Err(AuthError::UpstreamRefused("provider-answered-an-error"));
        }
        let mut live = self.live.lock().expect("oidc live state");
        let (endpoints, transport, stale_discovery) = self.endpoints(&mut live, now)?;
        stale.1 = stale_discovery;
        // O14: RFC 9207. Wrong is always refused; absent is refused where the
        // provider said it would send it.
        match q.iss {
            Some(iss) if iss != self.conn.issuer => {
                return Err(AuthError::UpstreamRefused("iss-parameter"))
            }
            None if endpoints.iss_parameter_required => {
                return Err(AuthError::UpstreamRefused("iss-parameter-missing"))
            }
            _ => {}
        }
        let code = q
            .code
            .filter(|c| !c.is_empty())
            .ok_or(AuthError::EmptyAuthorizationCode)?;
        let id_token = self.exchange(&transport, &endpoints, code, &begun.verifier)?;
        let header = jose::peek_header(&id_token, &self.conn.algs)
            .map_err(|e| AuthError::UpstreamRefused(e.0))?;
        let (keys, stale_keys) = self.keys_for(
            &mut live,
            &transport,
            &endpoints.jwks_url,
            header.kid.as_deref(),
            now,
        )?;
        drop(live);
        stale.0 = stale_keys;
        let verified = jose::verify_with_jwks(&id_token, &keys, &self.conn.algs)
            .map_err(|e| AuthError::UpstreamRefused(e.0))?;
        let claims = verified
            .claims()
            .map_err(|e| AuthError::UpstreamRefused(e.0))?;
        let sub = check_id_token(
            &claims,
            &IdTokenRules {
                issuer: &self.conn.issuer,
                client_id: &self.conn.client_id,
                nonce: &begun.nonce,
                skew_secs: self.conn.skew_secs,
            },
            now,
        )
        .map_err(AuthError::UpstreamRefused)?;
        let attributes = attributes_of(&self.conn, &claims);
        // §8.2: a two-step claim is minted only when this sign-in asked for
        // one (`max_age=0`) and `auth_time` is fresh. Where it counts is the
        // acceptance's to say, so a connection set `off` mints one too. Asked and not met is recorded; the sign-in stands.
        let upstream = if begun.presence {
            match self.conn.presence.judge(
                &self.conn.name,
                true,
                attributes.assurance.as_ref(),
                now,
                self.conn.skew_secs,
            ) {
                Ok(p) => Some(p),
                Err(why) => {
                    *not_minted = Some(why);
                    None
                }
            }
        } else {
            None
        };
        let subject = AuthenticatedSubject::without_presence(
            AuthenticatorId(format!("oidc:{}:{sub}", self.conn.name)),
            AuthMethodKind::Oidc,
        )
        .with_attributes(attributes)
        .with_upstream_presence(upstream);
        write_handoff(
            self.ceremonies.as_ref(),
            &self.conn.name,
            &begun.binder_hash,
            &subject,
            now,
        )
    }

    /// The token exchange. Answers the ID token; the access and refresh
    /// tokens beside it are dropped here, unread (§4.1).
    fn exchange(
        &self,
        transport: &ProviderClient,
        endpoints: &OidcEndpoints,
        code: &str,
        verifier: &str,
    ) -> Result<String, AuthError> {
        let got = transport
            .token_exchange(
                &endpoints.token_url,
                vec![
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("redirect_uri", self.conn.redirect_uri.as_str()),
                    ("client_id", self.conn.client_id.as_str()),
                    ("code_verifier", verifier),
                ],
                &self.conn.client_secret,
            )
            .map_err(|f| f.into_auth_error(Some("token-exchange-rejected")))?;
        let v = jose::strict_json(got.bytes())
            .map_err(|_| AuthError::UpstreamRefused("token-response-not-json"))?;
        v.get("id_token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .ok_or(AuthError::UpstreamRefused(
                "token-response-without-id-token",
            ))
    }
}

impl AuthProvider for Arc<OidcProvider> {
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

impl AuthProvider for OidcProvider {
    fn kind(&self) -> AuthMethodKind {
        AuthMethodKind::Oidc
    }

    fn begin(&self, req: &BeginRequest, now: Timestamp) -> Result<ChallengeOrRedirect, AuthError> {
        let return_to = req
            .return_to
            .as_deref()
            .filter(|r| !r.contains('#') && self.conn.returns.allows_url(r))
            .ok_or(AuthError::ReturnNotAllowed)?;
        let (endpoints, _, _) = {
            let mut live = self.live.lock().expect("oidc live state");
            self.endpoints(&mut live, now)?
        };
        let state = random_secret();
        let binder = random_secret();
        let begun = Begun {
            verifier: random_secret(),
            nonce: random_secret(),
            return_to: return_to.to_string(),
            binder_hash: handle_of(&binder),
            presence: req.presence,
        };
        self.ceremonies
            .put(&CeremonyRow {
                handle: handle_of(&state),
                connection: self.conn.name.clone(),
                phase: PHASE_BEGUN.to_string(),
                binder: begun.binder_hash.clone(),
                payload: begun.render(),
                minted_at: now,
                expires_at: Timestamp(now.0.saturating_add(CEREMONY_TTL_SECS)),
            })
            .map_err(|e| AuthError::StoreUnavailable(CredentialStoreError::from(e)))?;
        // O1: `response_type` is the constant `code`, PKCE is `S256`.
        let challenge = code_challenge(&begun.verifier);
        let mut pairs = vec![
            ("response_type", "code"),
            ("client_id", self.conn.client_id.as_str()),
            ("redirect_uri", self.conn.redirect_uri.as_str()),
            ("scope", self.conn.scope.as_str()),
            ("state", state.as_str()),
            ("nonce", begun.nonce.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ];
        // §8.2: ask the provider to prompt the person now. Honouring it is
        // the provider's choice, so `auth_time` is checked on the way back.
        if begun.presence {
            pairs.push(("max_age", "0"));
        }
        let query = form_encode(&pairs);
        let joiner = if endpoints.authorize_url.contains('?') {
            '&'
        } else {
            '?'
        };
        Ok(ChallengeOrRedirect::Redirect {
            url: format!("{}{joiner}{query}", endpoints.authorize_url),
            binder,
        })
    }

    /// Redeem a handoff, the only completion this provider takes.
    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        match resp {
            CompleteRequest::Handoff { handoff, binder } => redeem_handoff(
                self.ceremonies.as_ref(),
                &self.conn.name,
                handoff,
                binder,
                now,
            ),
            _ => Err(AuthError::MethodMismatch {
                provider: AuthMethodKind::Oidc,
            }),
        }
    }
}

#[cfg(test)]
#[path = "oidc_tests.rs"]
mod tests;
