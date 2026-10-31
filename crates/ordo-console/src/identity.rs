//! The console's one client to the identity service
//! (`plan-identity-service.md` 4.2, 5.4, step 6).
//!
//! The identity service holds the deployment's people and signs their
//! tokens with Ed25519. The console page gets a token from the service and
//! sends it to the console as a bearer; the console verifies it here, with
//! the service's published keys, as the door does
//! (`ordo-host/src/identity_keys.rs`). The console holds no people of its
//! own in this mode.
//!
//! # What it does
//!
//! - Fetches `GET /.well-known/jwks.json` at boot and every
//!   [`REFRESH_SECS`].
//! - Fetches again when a token names a `kid` it does not hold, at most
//!   once per [`UNKNOWN_KID_GAP_SECS`], so forged `kid`s cannot make the
//!   console hammer the service.
//! - Keeps only Ed25519 keys whose `kid` is their RFC 7638 thumbprint.
//! - A fetched set replaces the one held whole, so a retired key verifies
//!   nothing after the next fetch.
//!
//! The set is held in memory only. The door also keeps it in its control
//! database so a restart with the service down still verifies; the console
//! has no such need, because a console restarted while the service is down
//! has nobody who can sign in anyway (plan 5.4, "the service down means no
//! new sign-in anywhere").
//!
//! # Settings
//!
//! The same names the door reads, so the one image sets them once:
//! `ORDO_IDENTITY_URL` (where the console reaches the service),
//! `ORDO_IDENTITY_ORIGIN` (its public origin: the `iss` every token carries
//! and where the page sends the browser to sign in),
//! `ORDO_IDENTITY_PLAIN_HTTP` and `ORDO_IDENTITY_DEPLOYMENT`. Unset
//! `ORDO_IDENTITY_URL` keeps the console's own sign-in, as before step 6.
//!
//! - Relays a deployment admin's management calls ([`IdentityClient::call`]):
//!   people, invites, machine keys and the audit log, with the person's
//!   own token as the bearer, so the service decides who may.
//!
//! # One error
//!
//! Every failure to reach the service, and every refusal it answers, is
//! [`IdentityError`]. A token that
//! does not verify is a [`DeploymentTokenError`], which the console's edge
//! answers as `token_refused`.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use ordo_identity::deployment_token::{self, DeploymentClaims, DeploymentTokenError, Expect};
use ordo_identity::jose::{self, Alg, JoseError, Jwks};
use ordo_identity::origin::Origin;
use serde_json::Value;

use crate::codes::Clock;
use crate::error::ConsoleError;

/// Where the console reaches the identity service.
pub const URL_ENV: &str = "ORDO_IDENTITY_URL";
/// The service's public origin: the `iss` the console requires, and where
/// the page sends the browser to sign in.
pub const ISSUER_ENV: &str = "ORDO_IDENTITY_ORIGIN";
/// The handle the service names in `aud`. When set it must be the
/// console's own handle, `ORDO_CONSOLE_DEPLOYMENT`.
pub const DEPLOYMENT_ENV: &str = "ORDO_IDENTITY_DEPLOYMENT";
/// `on` lets `ORDO_IDENTITY_URL` be plain `http://` to a host that is not
/// loopback.
pub const PLAIN_HTTP_ENV: &str = "ORDO_IDENTITY_PLAIN_HTTP";
/// Where browsers reach the service under its origin (step 7, board
/// `upgrading-with-an-old-compose-file`, ask 1). Set, and the service's
/// origin is the console's own, the console passes `<base>` and everything
/// under it through to the service ([`IdentityClient::forward`]), so no new
/// port is needed and the passkeys' relying party stays the console's host.
pub const BASE_PATH_ENV: &str = "ORDO_IDENTITY_BASE_PATH";
/// The largest request or answer the console passes through.
pub const MAX_PASS_BYTES: usize = 1024 * 1024;
/// How long one passed-through call may take.
pub const PASS_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the set is fetched again: the service's `max-age`.
pub const REFRESH_SECS: u64 = 600;
/// The least time between two fetches an unknown `kid` causes.
pub const UNKNOWN_KID_GAP_SECS: u64 = 60;
/// How long one fetch may take.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest set the console reads.
pub const MAX_SET_BYTES: usize = 64 * 1024;
/// Where the service publishes its token keys.
pub const JWKS_PATH: &str = "/.well-known/jwks.json";
/// The largest answer to a management call the console reads.
pub const MAX_CALL_BYTES: usize = 512 * 1024;

// ── settings ─────────────────────────────────────────────────────────────

/// Where the service is, as the console reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceUrl {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Empty, or a path starting `/` with no trailing `/`.
    pub prefix: String,
}

impl ServiceUrl {
    /// Read `ORDO_IDENTITY_URL`. `plain_ok` is `ORDO_IDENTITY_PLAIN_HTTP=on`.
    pub fn parse(text: &str, plain_ok: bool) -> Result<ServiceUrl, ConsoleError> {
        let bad = |why: &str| {
            ConsoleError::setting(
                URL_ENV,
                format!("is `{text}`, which {why}"),
                format!(
                    "Set {URL_ENV} to where the console reaches the identity service, such \
                     as http://127.0.0.1:8082 in the one image, or unset it to keep the \
                     console's own sign-in"
                ),
            )
        };
        let t = text.trim().trim_end_matches('/');
        let (https, rest) = if let Some(r) = t.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = t.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(bad("is not an http:// or https:// address"));
        };
        let (authority, prefix) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].to_string()),
            None => (rest, String::new()),
        };
        if authority.is_empty() || authority.contains('@') || prefix.contains(['?', '#']) {
            return Err(bad("is not scheme://host[:port][/path]"));
        }
        let (host, port_text) = if let Some(v6) = authority.strip_prefix('[') {
            match v6.split_once(']') {
                Some((h, "")) => (h, None),
                Some((h, p)) => match p.strip_prefix(':') {
                    Some(p) => (h, Some(p)),
                    None => return Err(bad("is not scheme://host[:port][/path]")),
                },
                None => return Err(bad("is not scheme://host[:port][/path]")),
            }
        } else {
            match authority.split_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            }
        };
        let port = match port_text {
            None if https => 443,
            None => 80,
            Some(p) => p
                .parse::<u16>()
                .map_err(|_| bad("has a port that is not a number"))?,
        };
        if host.is_empty() {
            return Err(bad("names no host"));
        }
        let host = host.to_string();
        if !https && !plain_ok && !is_loopback(&host) {
            return Err(ConsoleError::setting(
                URL_ENV,
                format!(
                    "is `{text}`: plain http:// to a host that is not loopback would let \
                     anyone on the way swap the keys the console trusts"
                ),
                format!(
                    "Use https://, or set {PLAIN_HTTP_ENV}=on if the console and the identity \
                     service share a private network you trust"
                ),
            ));
        }
        Ok(ServiceUrl {
            https,
            host,
            port,
            prefix,
        })
    }

    /// The address as text, for messages.
    pub fn display(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        format!(
            "{scheme}://{}:{}{}",
            self.host_text(),
            self.port,
            self.prefix
        )
    }

    fn host_text(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }

    fn host_header(&self) -> String {
        let default = if self.https { 443 } else { 80 };
        if self.port == default {
            self.host_text()
        } else {
            format!("{}:{}", self.host_text(), self.port)
        }
    }
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// What the console needs to send people to the service and verify the
/// tokens they come back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySettings {
    pub url: ServiceUrl,
    /// `ORDO_IDENTITY_ORIGIN`, as the service writes it in `iss`.
    pub issuer: String,
    /// [`BASE_PATH_ENV`], checked: empty, or `/` and plain segments.
    pub base_path: String,
    /// `ordo-deployment:<handle>`, the console's own handle.
    pub audience: String,
}

impl IdentitySettings {
    /// From the environment, through `get`. `handle` is the console's own
    /// deployment handle. `Ok(None)` when `ORDO_IDENTITY_URL` is unset: the
    /// console keeps its own sign-in.
    pub fn from_vars(
        get: &dyn Fn(&str) -> Option<String>,
        handle: &str,
    ) -> Result<Option<IdentitySettings>, ConsoleError> {
        let set = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(url_text) = set(URL_ENV) else {
            return Ok(None);
        };
        let plain_ok = match set(PLAIN_HTTP_ENV).as_deref() {
            None | Some("off") => false,
            Some("on") => true,
            Some(other) => {
                return Err(ConsoleError::setting(
                    PLAIN_HTTP_ENV,
                    format!("is `{other}`"),
                    format!("Set {PLAIN_HTTP_ENV} to on or off, or unset it for off"),
                ))
            }
        };
        let url = ServiceUrl::parse(&url_text, plain_ok)?;
        let Some(issuer_text) = set(ISSUER_ENV) else {
            return Err(ConsoleError::setting(
                ISSUER_ENV,
                format!("is unset, and {URL_ENV} is set"),
                format!(
                    "Set {ISSUER_ENV} to the identity service's public origin, the same value \
                     the service has, such as https://console.example.com:8082. The console \
                     sends people there to sign in, and its tokens name it in `iss`"
                ),
            ));
        };
        let issuer = Origin::parse(&issuer_text).ok_or_else(|| {
            ConsoleError::setting(
                ISSUER_ENV,
                format!("is `{issuer_text}`, which is not scheme://host[:port]"),
                format!(
                    "Set {ISSUER_ENV} to the identity service's public origin, such as \
                     https://console.example.com:8082"
                ),
            )
        })?;
        if let Some(named) = set(DEPLOYMENT_ENV) {
            if named != handle {
                return Err(ConsoleError::setting(
                    DEPLOYMENT_ENV,
                    format!(
                        "is `{named}`, and this console manages `{handle}` \
                         ({}), so no token the service issues would be for it",
                        crate::config::DEPLOYMENT_ENV
                    ),
                    format!(
                        "Set {DEPLOYMENT_ENV} and {} to the same handle, or unset \
                         {DEPLOYMENT_ENV}",
                        crate::config::DEPLOYMENT_ENV
                    ),
                ));
            }
        }
        let base_text = set(BASE_PATH_ENV).unwrap_or_default();
        let base_path = ordo_identity::origin::base_path(&base_text).map_err(|why| {
            ConsoleError::setting(
                BASE_PATH_ENV,
                format!("is `{base_text}`, and {why}"),
                format!(
                    "Set {BASE_PATH_ENV} to the same path the identity service has, such as \
                     /identity, or unset it"
                ),
            )
        })?;
        Ok(Some(IdentitySettings {
            url,
            issuer: issuer.to_string(),
            base_path,
            audience: deployment_token::audience(handle),
        }))
    }

    /// Where the page sends a browser: the service's origin and its base
    /// path.
    pub fn public_url(&self) -> String {
        format!("{}{}", self.issuer.trim_end_matches('/'), self.base_path)
    }
}

/// One request passed through to the service ([`IdentityClient::forward`]).
pub struct Passed {
    pub status: u16,
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: Vec<u8>,
}

// ── the one error ────────────────────────────────────────────────────────

/// Why the service's keys could not be fetched. Every variant names the
/// address, keeps its cause, and says whether a retry can help.
#[derive(Debug)]
#[non_exhaustive]
pub enum IdentityError {
    /// No connection: the service is down or the address is wrong.
    Unreachable { url: String, cause: std::io::Error },
    /// The service did not answer in [`FETCH_TIMEOUT`].
    TimedOut { url: String, secs: u64 },
    /// The TLS handshake failed.
    Tls {
        url: String,
        cause: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The HTTP exchange broke.
    Protocol { url: String, cause: hyper::Error },
    /// The service answered with a status other than 200.
    Answered { url: String, status: u16 },
    /// The answer is not a key set the console can use.
    Unusable { url: String, why: &'static str },
    /// The service refused a management call, in its own words: `code` is
    /// its one word (`presence_required`, `last_admin`, `name_taken`, ...).
    Refused {
        url: String,
        status: u16,
        code: String,
        message: String,
    },
}

impl IdentityError {
    /// Can the same fetch succeed later with nothing changed here?
    pub fn retryable(&self) -> bool {
        match self {
            IdentityError::Unreachable { .. }
            | IdentityError::TimedOut { .. }
            | IdentityError::Protocol { .. } => true,
            IdentityError::Answered { status, .. } => *status >= 500 || *status == 429,
            IdentityError::Tls { .. } | IdentityError::Unusable { .. } => false,
            IdentityError::Refused { status, .. } => *status >= 500 || *status == 429,
        }
    }

    /// The service's own word for a refusal, when it gave one.
    pub fn code(&self) -> Option<&str> {
        match self {
            IdentityError::Refused { code, .. } => Some(code),
            _ => None,
        }
    }
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityError::Unreachable { url, cause } => write!(
                f,
                "the identity service at {url} could not be reached ({cause}); check that it \
                 runs and that {URL_ENV} names it"
            ),
            IdentityError::TimedOut { url, secs } => {
                write!(
                    f,
                    "the identity service at {url} did not answer in {secs} s"
                )
            }
            IdentityError::Tls { url, cause } => write!(
                f,
                "TLS to the identity service at {url} failed ({cause}); check its certificate \
                 and {URL_ENV}"
            ),
            IdentityError::Protocol { url, cause } => {
                write!(f, "the exchange with {url} broke: {cause}")
            }
            IdentityError::Answered { url, status } => write!(
                f,
                "the identity service at {url} answered {status}; check that \
                 {URL_ENV} names the identity service"
            ),
            IdentityError::Unusable { url, why } => write!(
                f,
                "the identity service at {url} answered nothing the console can use ({why})"
            ),
            IdentityError::Refused { message, .. } => {
                write!(f, "the identity service refused: {message}")
            }
        }
    }
}

impl std::error::Error for IdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IdentityError::Unreachable { cause, .. } => Some(cause),
            IdentityError::Tls { cause, .. } => Some(cause.as_ref()),
            IdentityError::Protocol { cause, .. } => Some(cause),
            _ => None,
        }
    }
}

// ── the set ──────────────────────────────────────────────────────────────

/// Keep only the Ed25519 keys whose `kid` is their RFC 7638 thumbprint.
pub fn usable_set(bytes: &[u8]) -> Result<Jwks, &'static str> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "not JSON")?;
    let list = v
        .get("keys")
        .and_then(Value::as_array)
        .ok_or("no `keys` array")?;
    let kept: Vec<Value> = list
        .iter()
        .filter(|k| {
            let text = |n: &str| k.get(n).and_then(Value::as_str);
            let (Some("OKP"), Some("Ed25519"), Some(x), Some(kid)) =
                (text("kty"), text("crv"), text("x"), text("kid"))
            else {
                return false;
            };
            ordo_identity::webauthn::b64url_decode(x)
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
                .and_then(|b| ed25519_dalek::VerifyingKey::from_bytes(&b).ok())
                .is_some_and(|key| deployment_token::key_id(&key) == kid)
        })
        .cloned()
        .collect();
    if kept.is_empty() {
        return Err("no Ed25519 key whose kid is its thumbprint");
    }
    let text = serde_json::json!({ "keys": kept }).to_string();
    Jwks::parse(text.as_bytes()).map_err(|e| e.0)
}

#[derive(Default)]
struct Held {
    jwks: Option<Arc<Jwks>>,
    /// When an unknown `kid` last caused a fetch.
    last_kid_fetch: Option<u64>,
}

/// The one client. Share it as an `Arc`.
pub struct IdentityClient {
    settings: IdentitySettings,
    tls: Option<Arc<rustls::ClientConfig>>,
    held: RwLock<Held>,
    fetching: tokio::sync::Mutex<()>,
    fetches: AtomicU64,
    clock: Clock,
}

impl fmt::Debug for IdentityClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IdentityClient({})", self.settings.url.display())
    }
}

impl IdentityClient {
    pub fn new(settings: IdentitySettings, clock: Clock) -> IdentityClient {
        let tls = settings.url.https.then(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        });
        IdentityClient {
            settings,
            tls,
            held: RwLock::new(Held::default()),
            fetching: tokio::sync::Mutex::new(()),
            fetches: AtomicU64::new(0),
            clock,
        }
    }

    pub fn settings(&self) -> &IdentitySettings {
        &self.settings
    }

    /// How many keys the console holds now.
    pub fn held_keys(&self) -> usize {
        self.held
            .read()
            .expect("identity keys")
            .jwks
            .as_ref()
            .map_or(0, |j| j.len())
    }

    /// How many fetches were tried since boot, ok or not.
    pub fn fetches(&self) -> u64 {
        self.fetches.load(Ordering::SeqCst)
    }

    /// Fetch the set and hold it. One fetch at a time; a caller that waited
    /// for another's fetch does not fetch again.
    pub async fn refresh(&self) -> Result<(), IdentityError> {
        let seen = self.fetches.load(Ordering::SeqCst);
        let _one = self.fetching.lock().await;
        if self.fetches.load(Ordering::SeqCst) != seen {
            return Ok(());
        }
        let answer = self.fetch_and_keep().await;
        self.fetches.fetch_add(1, Ordering::SeqCst);
        answer
    }

    async fn fetch_and_keep(&self) -> Result<(), IdentityError> {
        let url = self.settings.url.display();
        let body = self.fetch().await?;
        let jwks = usable_set(&body).map_err(|why| IdentityError::Unusable { url, why })?;
        self.held.write().expect("identity keys").jwks = Some(Arc::new(jwks));
        Ok(())
    }

    /// Verify a bearer token: signature and `kid` against the set held,
    /// then `iss`, `aud`, `exp` and the shape of every claim. A token that
    /// names a `kid` the console does not hold causes one fetch, unless an
    /// unknown `kid` already caused one in the last
    /// [`UNKNOWN_KID_GAP_SECS`].
    pub async fn verify(&self, token: &str) -> Result<DeploymentClaims, DeploymentTokenError> {
        if !deployment_token::is_deployment_shape(token) {
            return Err(DeploymentTokenError::Unverifiable(JoseError(
                "not-compact-jws",
            )));
        }
        self.prepare(token).await;
        let jwks = self.held.read().expect("identity keys").jwks.clone();
        let Some(jwks) = jwks else {
            return Err(DeploymentTokenError::Unverifiable(JoseError("no-keys")));
        };
        deployment_token::verify(
            token,
            &jwks,
            &Expect {
                issuer: &self.settings.issuer,
                audience: &self.settings.audience,
                now: self.clock.now(),
                leeway_secs: deployment_token::DEFAULT_LEEWAY_SECS,
            },
        )
    }

    async fn prepare(&self, token: &str) {
        let Ok(header) = jose::peek_header(token, &[Alg::EdDSA]) else {
            return;
        };
        let Some(kid) = header.kid else {
            return;
        };
        let now = self.clock.now();
        {
            let mut held = self.held.write().expect("identity keys");
            if held.jwks.as_ref().is_some_and(|j| j.has(&kid)) {
                return;
            }
            if held
                .last_kid_fetch
                .is_some_and(|t| now < t.saturating_add(UNKNOWN_KID_GAP_SECS))
            {
                return;
            }
            held.last_kid_fetch = Some(now);
        }
        if let Err(e) = self.refresh().await {
            eprintln!("ordo-console: a token named an unknown key, and the fetch failed: {e}");
        }
    }

    /// Fetch every [`REFRESH_SECS`] for as long as the console runs.
    pub fn spawn_refresher(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let keys = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(REFRESH_SECS));
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = keys.refresh().await {
                    eprintln!("ordo-console: the scheduled fetch of the identity keys failed: {e}");
                }
            }
        })
    }

    async fn fetch(&self) -> Result<Vec<u8>, IdentityError> {
        let u = &self.settings.url;
        let shown = format!("{}{JWKS_PATH}", u.display());
        let req = hyper::Request::builder()
            .method("GET")
            .uri(format!("{}{JWKS_PATH}", u.prefix))
            .header(hyper::header::HOST, u.host_header())
            .header(hyper::header::ACCEPT, "application/json")
            .body(Full::new(Bytes::new()))
            .map_err(|_| IdentityError::Unusable {
                url: shown.clone(),
                why: "the request could not be built",
            })?;
        let (status, body) =
            match tokio::time::timeout(FETCH_TIMEOUT, self.exchange(&shown, req)).await {
                Ok(r) => r?,
                Err(_) => {
                    return Err(IdentityError::TimedOut {
                        url: shown,
                        secs: FETCH_TIMEOUT.as_secs(),
                    })
                }
            };
        if status != 200 {
            return Err(IdentityError::Answered { url: shown, status });
        }
        body.ok_or(IdentityError::Unusable {
            url: shown,
            why: "the answer is larger than 64 KiB",
        })
    }

    /// One management call to the service as the person whose `bearer`
    /// it is: `GET` or `POST` to `path` (with its query), with a JSON body
    /// for a `POST`. The answer's JSON, or the service's refusal as
    /// [`IdentityError::Refused`]. The token goes to the service only and
    /// is never logged.
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        bearer: &str,
        body: Option<&Value>,
    ) -> Result<Value, IdentityError> {
        let u = &self.settings.url;
        let shown = format!("{}{}", u.display(), path.split('?').next().unwrap_or(path));
        let bytes = body.map(|b| b.to_string()).unwrap_or_default();
        let mut req = hyper::Request::builder()
            .method(method)
            .uri(format!("{}{path}", u.prefix))
            .header(hyper::header::HOST, u.host_header())
            .header(hyper::header::ACCEPT, "application/json")
            .header(hyper::header::AUTHORIZATION, format!("Bearer {bearer}"));
        if body.is_some() {
            req = req.header(hyper::header::CONTENT_TYPE, "application/json");
        }
        let req = req
            .body(Full::new(Bytes::from(bytes)))
            .map_err(|_| IdentityError::Unusable {
                url: shown.clone(),
                why: "the request could not be built",
            })?;
        let (status, answer) = match tokio::time::timeout(
            FETCH_TIMEOUT,
            self.exchange_limited(&shown, req, MAX_CALL_BYTES),
        )
        .await
        {
            Ok(r) => r?,
            Err(_) => {
                return Err(IdentityError::TimedOut {
                    url: shown,
                    secs: FETCH_TIMEOUT.as_secs(),
                })
            }
        };
        let Some(answer) = answer else {
            return Err(IdentityError::Unusable {
                url: shown,
                why: "the answer is too large",
            });
        };
        let json: Option<Value> = serde_json::from_slice(&answer).ok();
        if (200..300).contains(&status) {
            return json.ok_or(IdentityError::Unusable {
                url: shown,
                why: "the answer is not JSON",
            });
        }
        let word = |k: &str| {
            json.as_ref()
                .and_then(|v| v.get(k))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        match word("error") {
            Some(code) => Err(IdentityError::Refused {
                url: shown,
                status,
                message: word("message").unwrap_or_else(|| code.clone()),
                code,
            }),
            None => Err(IdentityError::Answered { url: shown, status }),
        }
    }

    /// Pass a browser's request through to the service, as it came: the
    /// method, the path under the base path and its query, the headers but
    /// the hop-by-hop ones, and the body (at most [`MAX_PASS_BYTES`]).
    /// `X-Forwarded-For` gains `peer`, the address the console saw, so the
    /// service's guessing limits count each visitor and not the console.
    /// The answer comes back whole, its headers as they are.
    pub async fn forward(
        &self,
        method: &str,
        path_and_query: &str,
        headers: &[(String, Vec<u8>)],
        peer: Option<std::net::IpAddr>,
        body: Vec<u8>,
    ) -> Result<Passed, IdentityError> {
        let u = &self.settings.url;
        let shown = format!(
            "{}{}",
            u.display(),
            path_and_query.split('?').next().unwrap_or(path_and_query)
        );
        let mut req = hyper::Request::builder()
            .method(method)
            .uri(format!("{}{path_and_query}", u.prefix))
            .header(hyper::header::HOST, u.host_header());
        let mut forwarded_for: Option<String> = None;
        for (k, v) in headers {
            let name = k.to_ascii_lowercase();
            if HOP_BY_HOP.contains(&name.as_str()) || name == "host" {
                continue;
            }
            if name == "x-forwarded-for" {
                forwarded_for = std::str::from_utf8(v).ok().map(str::to_string);
                continue;
            }
            req = req.header(k.as_str(), v.as_slice());
        }
        let chain = match (forwarded_for, peer) {
            (Some(f), Some(p)) => Some(format!("{f}, {p}")),
            (None, Some(p)) => Some(p.to_string()),
            (f, None) => f,
        };
        if let Some(c) = chain {
            req = req.header("x-forwarded-for", c);
        }
        let req = req
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| IdentityError::Unusable {
                url: shown.clone(),
                why: "the request could not be passed on",
            })?;
        let u = &self.settings.url;
        let tcp = tokio::time::timeout(
            PASS_TIMEOUT,
            tokio::net::TcpStream::connect((u.host.as_str(), u.port)),
        )
        .await
        .map_err(|_| IdentityError::TimedOut {
            url: shown.clone(),
            secs: PASS_TIMEOUT.as_secs(),
        })?
        .map_err(|cause| IdentityError::Unreachable {
            url: shown.clone(),
            cause,
        })?;
        let passed = match &self.tls {
            None => tokio::time::timeout(PASS_TIMEOUT, pass(TokioIo::new(tcp), req)).await,
            Some(cfg) => {
                let name =
                    rustls::pki_types::ServerName::try_from(u.host.clone()).map_err(|e| {
                        IdentityError::Tls {
                            url: shown.clone(),
                            cause: Box::new(e),
                        }
                    })?;
                let tls = tokio_rustls::TlsConnector::from(cfg.clone())
                    .connect(name, tcp)
                    .await
                    .map_err(|e| IdentityError::Tls {
                        url: shown.clone(),
                        cause: Box::new(e),
                    })?;
                tokio::time::timeout(PASS_TIMEOUT, pass(TokioIo::new(tls), req)).await
            }
        };
        match passed {
            Err(_) => Err(IdentityError::TimedOut {
                url: shown,
                secs: PASS_TIMEOUT.as_secs(),
            }),
            Ok(Err(cause)) => Err(IdentityError::Protocol { url: shown, cause }),
            Ok(Ok(None)) => Err(IdentityError::Unusable {
                url: shown,
                why: "the answer is too large to pass on",
            }),
            Ok(Ok(Some(p))) => Ok(p),
        }
    }

    async fn exchange(
        &self,
        shown: &str,
        req: hyper::Request<Full<Bytes>>,
    ) -> Result<(u16, Option<Vec<u8>>), IdentityError> {
        self.exchange_limited(shown, req, MAX_SET_BYTES).await
    }

    async fn exchange_limited(
        &self,
        shown: &str,
        req: hyper::Request<Full<Bytes>>,
        limit: usize,
    ) -> Result<(u16, Option<Vec<u8>>), IdentityError> {
        let u = &self.settings.url;
        let tcp = tokio::net::TcpStream::connect((u.host.as_str(), u.port))
            .await
            .map_err(|cause| IdentityError::Unreachable {
                url: shown.to_string(),
                cause,
            })?;
        let protocol = |cause| IdentityError::Protocol {
            url: shown.to_string(),
            cause,
        };
        match &self.tls {
            None => drive(TokioIo::new(tcp), req, limit).await.map_err(protocol),
            Some(cfg) => {
                let tls_err = |e: Box<dyn std::error::Error + Send + Sync>| IdentityError::Tls {
                    url: shown.to_string(),
                    cause: e,
                };
                let name = rustls::pki_types::ServerName::try_from(u.host.clone())
                    .map_err(|e| tls_err(Box::new(e)))?;
                let tls = tokio_rustls::TlsConnector::from(cfg.clone())
                    .connect(name, tcp)
                    .await
                    .map_err(|e| tls_err(Box::new(e)))?;
                drive(TokioIo::new(tls), req, limit).await.map_err(protocol)
            }
        }
    }
}

/// Headers that belong to one connection, never passed on.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// One HTTP/1.1 exchange that keeps the answer's headers. `None` past
/// [`MAX_PASS_BYTES`].
async fn pass<T>(io: T, req: hyper::Request<Full<Bytes>>) -> Result<Option<Passed>, hyper::Error>
where
    T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    let driver = tokio::spawn(async move {
        let _ = conn.await;
    });
    let res = sender.send_request(req).await?;
    let status = res.status().as_u16();
    let headers: Vec<(String, Vec<u8>)> = res
        .headers()
        .iter()
        .filter(|(k, _)| !HOP_BY_HOP.contains(&k.as_str()) && k.as_str() != "content-length")
        .map(|(k, v)| (k.as_str().to_string(), v.as_bytes().to_vec()))
        .collect();
    let body = Limited::new(res.into_body(), MAX_PASS_BYTES)
        .collect()
        .await
        .ok()
        .map(|b| b.to_bytes().to_vec());
    driver.abort();
    Ok(body.map(|body| Passed {
        status,
        headers,
        body,
    }))
}

/// One HTTP/1.1 exchange. The body is `None` past `limit` bytes.
async fn drive<T>(
    io: T,
    req: hyper::Request<Full<Bytes>>,
    limit: usize,
) -> Result<(u16, Option<Vec<u8>>), hyper::Error>
where
    T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    let driver = tokio::spawn(async move {
        let _ = conn.await;
    });
    let res = sender.send_request(req).await?;
    let status = res.status().as_u16();
    let body = Limited::new(res.into_body(), limit)
        .collect()
        .await
        .ok()
        .map(|b| b.to_bytes().to_vec());
    driver.abort();
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn refused_setting(r: Result<Option<IdentitySettings>, ConsoleError>) -> &'static str {
        r.unwrap_err().setting_name().expect("names a setting")
    }

    #[test]
    fn unset_url_keeps_the_consoles_own_sign_in() {
        assert_eq!(
            IdentitySettings::from_vars(&vars(&[]), "main").unwrap(),
            None
        );
    }

    #[test]
    fn the_settings_read_and_each_refusal_names_its_setting() {
        let ok = IdentitySettings::from_vars(
            &vars(&[
                (URL_ENV, "http://127.0.0.1:8082/"),
                (ISSUER_ENV, "https://console.example.com:8082"),
            ]),
            "main",
        )
        .unwrap()
        .unwrap();
        assert_eq!(ok.url.display(), "http://127.0.0.1:8082");
        assert_eq!(ok.issuer, "https://console.example.com:8082");
        assert_eq!(ok.audience, "ordo-deployment:main");

        let r = IdentitySettings::from_vars(&vars(&[(URL_ENV, "http://127.0.0.1:8082")]), "main");
        assert_eq!(refused_setting(r), ISSUER_ENV);
        let r = IdentitySettings::from_vars(
            &vars(&[
                (URL_ENV, "http://identity:8082"),
                (ISSUER_ENV, "https://id.example.com"),
            ]),
            "main",
        );
        assert_eq!(refused_setting(r), URL_ENV);
        assert!(IdentitySettings::from_vars(
            &vars(&[
                (URL_ENV, "http://identity:8082"),
                (ISSUER_ENV, "https://id.example.com"),
                (PLAIN_HTTP_ENV, "on"),
            ]),
            "main",
        )
        .is_ok());
        let r = IdentitySettings::from_vars(
            &vars(&[
                (URL_ENV, "https://id.example.com"),
                (ISSUER_ENV, "https://id.example.com"),
                (PLAIN_HTTP_ENV, "yes"),
            ]),
            "main",
        );
        assert_eq!(refused_setting(r), PLAIN_HTTP_ENV);
        let r = IdentitySettings::from_vars(
            &vars(&[
                (URL_ENV, "https://id.example.com"),
                (ISSUER_ENV, "https://id.example.com/path"),
            ]),
            "main",
        );
        assert_eq!(refused_setting(r), ISSUER_ENV);
        let r = IdentitySettings::from_vars(
            &vars(&[
                (URL_ENV, "https://id.example.com"),
                (ISSUER_ENV, "https://id.example.com"),
                (DEPLOYMENT_ENV, "other"),
            ]),
            "main",
        );
        assert_eq!(refused_setting(r), DEPLOYMENT_ENV);
        assert!(IdentitySettings::from_vars(
            &vars(&[
                (URL_ENV, "https://id.example.com"),
                (ISSUER_ENV, "https://id.example.com"),
                (DEPLOYMENT_ENV, "main"),
            ]),
            "main",
        )
        .is_ok());
    }

    #[test]
    fn a_key_whose_kid_is_not_its_thumbprint_is_dropped() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let kid = deployment_token::key_id(&key.verifying_key());
        let good = deployment_token::public_jwk(&kid, &key.verifying_key());
        let liar = deployment_token::public_jwk("chosen-by-me", &key.verifying_key());
        let body = serde_json::json!({ "keys": [good, liar] }).to_string();
        let jwks = usable_set(body.as_bytes()).unwrap();
        assert_eq!(jwks.len(), 1);
        assert!(jwks.has(&kid) && !jwks.has("chosen-by-me"));
        let only_liar = serde_json::json!({
            "keys": [deployment_token::public_jwk("x", &key.verifying_key())]
        });
        assert!(usable_set(only_liar.to_string().as_bytes()).is_err());
    }
}
