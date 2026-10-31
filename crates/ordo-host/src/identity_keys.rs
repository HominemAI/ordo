//! The door's one client to the identity service's keys
//! (`plan-identity-service.md` 4.2, step 3).
//!
//! The identity service signs deployment tokens with Ed25519 and publishes
//! the public halves at `GET /.well-known/jwks.json`. The door verifies
//! those tokens with this client and holds no deployment people itself.
//!
//! # What it does
//!
//! - Fetches the set at boot, and again every [`REFRESH_SECS`].
//! - Fetches again when a token names a `kid` it does not hold, at most
//!   once per [`UNKNOWN_KID_GAP_SECS`], so a stream of forged `kid`s cannot
//!   turn the door into a client that hammers the service.
//! - Writes every set it fetched to `$identitykeys` in the control
//!   database, and reads it back at boot. So a door that restarts while the
//!   service is down still verifies the tokens already issued: the thing
//!   that manages the deployment must not be a dependency of it (owner,
//!   2026-10-27). Only new sign-ins wait for the service.
//! - A key is kept only when its `kid` is its RFC 7638 thumbprint, the id
//!   the service gives it ([`ordo_identity::deployment_token::key_id`]).
//! - A set fetched replaces the one held whole. A key the service retired
//!   is gone from the set, so it verifies nothing here after the next
//!   fetch.
//!
//! # Where it reaches
//!
//! `ORDO_IDENTITY_URL`: plain `http://` to loopback (the one image,
//! `http://127.0.0.1:8082`), `https://` anywhere, and plain `http://` to
//! another host only with `ORDO_IDENTITY_PLAIN_HTTP=on`, because a set
//! fetched in the clear can be swapped on the way. It is the deployment
//! owner's own sibling service, so it is reached directly and not through
//! the egress kernel, which is for hosts a customer names.
//!
//! # One error
//!
//! Every failure is [`IdentityKeysError`], which keeps its cause and says
//! whether a retry can help. The door never fails a request on it: a door
//! with no keys refuses deployment tokens, with the reason in the security
//! record.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use ordo_exec::{sql_lit, Pool};
use ordo_identity::deployment_token::{self, DeploymentClaims, DeploymentTokenError, Expect};
use ordo_identity::jose::{self, Alg, JoseError, Jwks};
use ordo_identity::origin::Origin;
use ordo_identity::StoreFault;
use serde_json::Value;

/// Where the door reaches the identity service.
pub const URL_ENV: &str = "ORDO_IDENTITY_URL";
/// The `iss` the door expects: the service's public origin.
pub const ISSUER_ENV: &str = "ORDO_IDENTITY_ORIGIN";
/// The handle the door's tokens name in `aud`.
pub const DEPLOYMENT_ENV: &str = "ORDO_IDENTITY_DEPLOYMENT";
/// `on` lets `ORDO_IDENTITY_URL` be plain `http://` to a host that is not
/// loopback.
pub const PLAIN_HTTP_ENV: &str = "ORDO_IDENTITY_PLAIN_HTTP";
/// The handle when [`DEPLOYMENT_ENV`] is unset: the console's and the
/// service's `main`.
pub const DEFAULT_DEPLOYMENT: &str = "main";

/// How often the set is fetched again: the service's `max-age`.
pub const REFRESH_SECS: u64 = 600;
/// The least time between two fetches an unknown `kid` causes.
pub const UNKNOWN_KID_GAP_SECS: u64 = 60;
/// How long one fetch may take.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest set the door reads. A real one is well under 2 KiB.
pub const MAX_SET_BYTES: usize = 64 * 1024;
/// The path the service publishes its token keys at.
pub const JWKS_PATH: &str = "/.well-known/jwks.json";

// ── settings ─────────────────────────────────────────────────────────────

/// Where the service is, as the door reaches it.
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
    pub fn parse(text: &str, plain_ok: bool) -> Result<ServiceUrl, String> {
        let bad = |why: &str| {
            format!(
                "is `{text}`, which {why}. Set it to where the door reaches the identity \
                 service, such as http://127.0.0.1:8082 in the one image"
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
            None => {
                if https {
                    443
                } else {
                    80
                }
            }
            Some(p) => p
                .parse::<u16>()
                .map_err(|_| bad("has a port that is not a number"))?,
        };
        if host.is_empty() {
            return Err(bad("names no host"));
        }
        let host = host.to_string();
        if !https && !plain_ok && !is_loopback(&host) {
            return Err(format!(
                "is `{text}`: plain http:// to a host that is not loopback would let anyone \
                 on the way swap the keys the door trusts. Use https://, or set \
                 {PLAIN_HTTP_ENV}=on if the door and the identity service share a private \
                 network you trust"
            ));
        }
        Ok(ServiceUrl {
            https,
            host,
            port,
            prefix,
        })
    }

    /// The address as text, for messages and as the stored row's key.
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

fn valid_handle(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 63
        && h.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What the door needs to verify deployment tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySettings {
    pub url: ServiceUrl,
    /// The `iss` every token must carry: `ORDO_IDENTITY_ORIGIN`, as the
    /// service writes it.
    pub issuer: String,
    /// `ordo-deployment:<handle>`.
    pub audience: String,
}

impl IdentitySettings {
    /// From the environment, through `get`. `Ok(None)` when
    /// `ORDO_IDENTITY_URL` is unset: the door then accepts no deployment
    /// token, and says so at boot. A bad value is the setting's name and
    /// what to do, for the boot refusal.
    pub fn from_vars(
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Option<IdentitySettings>, (&'static str, String)> {
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
                return Err((
                    PLAIN_HTTP_ENV,
                    format!("is `{other}`; set it to on or off, or unset it for off"),
                ))
            }
        };
        let url = ServiceUrl::parse(&url_text, plain_ok).map_err(|e| (URL_ENV, e))?;
        let Some(issuer_text) = set(ISSUER_ENV) else {
            return Err((
                ISSUER_ENV,
                format!(
                    "is unset, and {URL_ENV} is set. Set {ISSUER_ENV} to the identity \
                     service's public origin, the same value the service has, such as \
                     https://console.example.com:8082. Its tokens name it in `iss`"
                ),
            ));
        };
        let issuer = Origin::parse(&issuer_text).ok_or_else(|| {
            (
                ISSUER_ENV,
                format!(
                    "is `{issuer_text}`, which is not scheme://host[:port]. Set it to the \
                     identity service's public origin, such as https://console.example.com:8082"
                ),
            )
        })?;
        let handle = set(DEPLOYMENT_ENV).unwrap_or_else(|| DEFAULT_DEPLOYMENT.into());
        if !valid_handle(&handle) {
            return Err((
                DEPLOYMENT_ENV,
                format!(
                    "is `{handle}`, which is not a deployment handle. Set it to lowercase \
                     letters, digits and dashes, at most 63, the same value the identity \
                     service has, or unset it for `{DEFAULT_DEPLOYMENT}`"
                ),
            ));
        }
        Ok(Some(IdentitySettings {
            url,
            issuer: issuer.to_string(),
            audience: deployment_token::audience(&handle),
        }))
    }

    /// [`IdentitySettings::from_vars`] over the process environment.
    pub fn from_env() -> Result<Option<IdentitySettings>, (&'static str, String)> {
        IdentitySettings::from_vars(&|k: &str| std::env::var(k).ok())
    }
}

// ── the one error ────────────────────────────────────────────────────────

/// Why the keys could not be fetched or kept. Every variant names the
/// address, keeps its cause, and says whether a retry can help.
#[derive(Debug)]
pub enum IdentityKeysError {
    /// No connection: the service is down or the address is wrong.
    Unreachable { url: String, cause: std::io::Error },
    /// The service did not answer in [`FETCH_TIMEOUT`].
    TimedOut { url: String, secs: u64 },
    /// The TLS handshake failed: a certificate the door does not trust, or
    /// a host name it does not match.
    Tls {
        url: String,
        cause: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The HTTP exchange broke.
    Protocol { url: String, cause: hyper::Error },
    /// The service answered with a status other than 200.
    Answered { url: String, status: u16 },
    /// The answer is not a key set the door can use: not JSON, too large,
    /// or holding no Ed25519 key whose `kid` is its thumbprint.
    Unusable { url: String, why: &'static str },
    /// `$identitykeys` could not be read or written.
    Store(StoreFault),
}

impl IdentityKeysError {
    /// Can the same fetch succeed later with nothing changed here?
    pub fn retryable(&self) -> bool {
        match self {
            IdentityKeysError::Unreachable { .. }
            | IdentityKeysError::TimedOut { .. }
            | IdentityKeysError::Protocol { .. } => true,
            IdentityKeysError::Answered { status, .. } => *status >= 500 || *status == 429,
            IdentityKeysError::Store(f) => f.class().may_retry(),
            IdentityKeysError::Tls { .. } | IdentityKeysError::Unusable { .. } => false,
        }
    }
}

impl fmt::Display for IdentityKeysError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityKeysError::Unreachable { url, cause } => write!(
                f,
                "the identity service at {url} could not be reached ({cause}); check that it \
                 runs and that {URL_ENV} names it"
            ),
            IdentityKeysError::TimedOut { url, secs } => {
                write!(
                    f,
                    "the identity service at {url} did not answer in {secs} s"
                )
            }
            IdentityKeysError::Tls { url, cause } => write!(
                f,
                "TLS to the identity service at {url} failed ({cause}); check its certificate \
                 and {URL_ENV}"
            ),
            IdentityKeysError::Protocol { url, cause } => {
                write!(f, "the exchange with {url} broke: {cause}")
            }
            IdentityKeysError::Answered { url, status } => write!(
                f,
                "the identity service at {url} answered {status} for its keys; check that \
                 {URL_ENV} names the identity service"
            ),
            IdentityKeysError::Unusable { url, why } => write!(
                f,
                "the identity service at {url} answered no key set the door can use ({why})"
            ),
            IdentityKeysError::Store(fault) => {
                write!(
                    f,
                    "the identity service's keys could not be stored: {fault}"
                )
            }
        }
    }
}

impl std::error::Error for IdentityKeysError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IdentityKeysError::Unreachable { cause, .. } => Some(cause),
            IdentityKeysError::Tls { cause, .. } => Some(cause.as_ref()),
            IdentityKeysError::Protocol { cause, .. } => Some(cause),
            IdentityKeysError::Store(f) => Some(f),
            _ => None,
        }
    }
}

// ── the stored copy ──────────────────────────────────────────────────────

const DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$identitykeys" (
  "source"     text   PRIMARY KEY,
  "jwks"       text   NOT NULL,
  "fetched_at" bigint NOT NULL
)"#;

enum Backend {
    Pg(Pool),
    Memory(Mutex<Option<(String, u64)>>),
}

/// Where the last set fetched is kept: `$identitykeys` in the control
/// database, one row per service address.
#[derive(Clone)]
pub struct IdentityKeyStore(Arc<Backend>);

fn fault(e: ordo_exec::PoolError) -> StoreFault {
    crate::auth::store_failure::pool_fault(
        e,
        "the identity service's keys could not be read or written",
    )
}

impl IdentityKeyStore {
    /// Over the control database.
    pub fn pg(pool: Pool) -> IdentityKeyStore {
        IdentityKeyStore(Arc::new(Backend::Pg(pool)))
    }

    /// In memory. For tests.
    pub fn memory() -> IdentityKeyStore {
        IdentityKeyStore(Arc::new(Backend::Memory(Mutex::new(None))))
    }

    /// Make the table if it is missing.
    pub async fn ensure_table(&self) -> Result<(), StoreFault> {
        if let Backend::Pg(pool) = self.0.as_ref() {
            pool.rows(DDL).await.map_err(fault)?;
        }
        Ok(())
    }

    /// The set stored for `source`, and when it was fetched.
    pub async fn load(&self, source: &str) -> Result<Option<(String, u64)>, StoreFault> {
        match self.0.as_ref() {
            Backend::Memory(m) => Ok(m.lock().expect("identity keys memory").clone()),
            Backend::Pg(pool) => {
                let rows = pool
                    .rows(&format!(
                        r#"SELECT "jwks", "fetched_at" FROM "$identitykeys" WHERE "source" = {}"#,
                        sql_lit(source)
                    ))
                    .await
                    .map_err(fault)?;
                Ok(rows.first().and_then(|r| {
                    let jwks = r.first().cloned().flatten()?;
                    let at = r.get(1).cloned().flatten()?.parse().ok()?;
                    Some((jwks, at))
                }))
            }
        }
    }

    /// Keep `jwks` as the set for `source`.
    pub async fn save(&self, source: &str, jwks: &str, at: u64) -> Result<(), StoreFault> {
        match self.0.as_ref() {
            Backend::Memory(m) => {
                *m.lock().expect("identity keys memory") = Some((jwks.to_string(), at));
                Ok(())
            }
            Backend::Pg(pool) => {
                pool.rows(&format!(
                    r#"INSERT INTO "$identitykeys" ("source", "jwks", "fetched_at")
                       VALUES ({s}, {j}, {at})
                       ON CONFLICT ("source") DO UPDATE
                       SET "jwks" = EXCLUDED."jwks", "fetched_at" = EXCLUDED."fetched_at""#,
                    s = sql_lit(source),
                    j = sql_lit(jwks),
                ))
                .await
                .map_err(fault)?;
                Ok(())
            }
        }
    }
}

// ── the set ──────────────────────────────────────────────────────────────

/// Keep only the Ed25519 keys whose `kid` is their RFC 7638 thumbprint,
/// and parse what is left. The answer is the set and its canonical text,
/// which is what gets stored.
pub fn usable_set(bytes: &[u8]) -> Result<(Jwks, String), &'static str> {
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
    let jwks = Jwks::parse(text.as_bytes()).map_err(|e| e.0)?;
    Ok((jwks, text))
}

#[derive(Default)]
struct Held {
    jwks: Option<Arc<Jwks>>,
    /// When the set held was fetched, by the door's clock.
    fetched_at: u64,
    /// When an unknown `kid` last caused a fetch.
    last_kid_fetch: Option<u64>,
}

/// The one client. Share it as an `Arc`.
pub struct IdentityKeys {
    settings: IdentitySettings,
    store: IdentityKeyStore,
    tls: Option<Arc<rustls::ClientConfig>>,
    held: RwLock<Held>,
    /// One fetch at a time.
    fetching: tokio::sync::Mutex<()>,
    /// Fetches tried, so a caller that waited out another's fetch does not
    /// fetch again.
    fetches: AtomicU64,
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl fmt::Debug for IdentityKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IdentityKeys({})", self.settings.url.display())
    }
}

impl IdentityKeys {
    pub fn new(
        settings: IdentitySettings,
        store: IdentityKeyStore,
        now: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> IdentityKeys {
        let tls = settings.url.https.then(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        });
        IdentityKeys {
            settings,
            store,
            tls,
            held: RwLock::new(Held::default()),
            fetching: tokio::sync::Mutex::new(()),
            fetches: AtomicU64::new(0),
            now,
        }
    }

    pub fn settings(&self) -> &IdentitySettings {
        &self.settings
    }

    /// How many keys the door holds now.
    pub fn held_keys(&self) -> usize {
        self.held
            .read()
            .expect("identity keys")
            .jwks
            .as_ref()
            .map_or(0, |j| j.len())
    }

    /// At boot: the stored set first, so the door verifies even when the
    /// service is down, then a fetch. A failed fetch is answered, not
    /// fatal; the caller logs it. A failed read of the store is answered
    /// the same way, after the fetch was tried.
    pub async fn boot(&self) -> Result<(), IdentityKeysError> {
        let source = self.settings.url.display();
        let stored = self.store.load(&source).await;
        if let Ok(Some((text, at))) = &stored {
            if let Ok((jwks, _)) = usable_set(text.as_bytes()) {
                let mut held = self.held.write().expect("identity keys");
                held.jwks = Some(Arc::new(jwks));
                held.fetched_at = *at;
            }
        }
        self.refresh().await?;
        stored.map(|_| ()).map_err(IdentityKeysError::Store)
    }

    /// Fetch the set, hold it, and store it. One fetch at a time; a caller
    /// that waited for another's fetch does not fetch again.
    pub async fn refresh(&self) -> Result<(), IdentityKeysError> {
        let seen = self.fetches.load(Ordering::SeqCst);
        let _one = self.fetching.lock().await;
        if self.fetches.load(Ordering::SeqCst) != seen {
            return Ok(());
        }
        let answer = self.fetch_and_keep().await;
        self.fetches.fetch_add(1, Ordering::SeqCst);
        answer
    }

    /// How many fetches were tried since boot, ok or not.
    pub fn fetches(&self) -> u64 {
        self.fetches.load(Ordering::SeqCst)
    }

    async fn fetch_and_keep(&self) -> Result<(), IdentityKeysError> {
        let at = (self.now)();
        let url = self.settings.url.display();
        let body = self.fetch().await?;
        let (jwks, text) = usable_set(&body).map_err(|why| IdentityKeysError::Unusable {
            url: url.clone(),
            why,
        })?;
        {
            let mut held = self.held.write().expect("identity keys");
            held.jwks = Some(Arc::new(jwks));
            held.fetched_at = at;
        }
        self.store
            .save(&url, &text, at)
            .await
            .map_err(IdentityKeysError::Store)
    }

    /// Before a token is checked: if it names a `kid` the door does not
    /// hold, fetch once, unless an unknown `kid` already caused a fetch in
    /// the last [`UNKNOWN_KID_GAP_SECS`]. Never fails the request: a fetch
    /// that fails leaves the set as it was.
    pub async fn prepare(&self, token: &str) {
        let Ok(header) = jose::peek_header(token, &[Alg::EdDSA]) else {
            return;
        };
        let Some(kid) = header.kid else {
            return;
        };
        let now = (self.now)();
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
            eprintln!("identity keys: a token named an unknown key, and the fetch failed: {e}");
        }
    }

    /// Verify a deployment token against the set held: signature and
    /// `kid`, then `iss`, `aud`, `exp` (`deployment_token::verify`). No set
    /// held is a refusal, `no-keys`.
    pub fn verify(&self, token: &str, now: u64) -> Result<DeploymentClaims, DeploymentTokenError> {
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
                now,
                leeway_secs: deployment_token::DEFAULT_LEEWAY_SECS,
            },
        )
    }

    /// Fetch every [`REFRESH_SECS`] for as long as the door runs.
    pub fn spawn_refresher(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let keys = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(REFRESH_SECS));
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = keys.refresh().await {
                    eprintln!("identity keys: the scheduled fetch failed: {e}");
                }
            }
        })
    }

    async fn fetch(&self) -> Result<Vec<u8>, IdentityKeysError> {
        let u = &self.settings.url;
        let shown = format!("{}{JWKS_PATH}", u.display());
        let req = hyper::Request::builder()
            .method("GET")
            .uri(format!("{}{JWKS_PATH}", u.prefix))
            .header(hyper::header::HOST, u.host_header())
            .header(hyper::header::ACCEPT, "application/json")
            .body(Full::new(Bytes::new()))
            .map_err(|_| IdentityKeysError::Unusable {
                url: shown.clone(),
                why: "the request could not be built",
            })?;
        let exchange = self.exchange(&shown, req);
        let (status, body) = match tokio::time::timeout(FETCH_TIMEOUT, exchange).await {
            Ok(r) => r?,
            Err(_) => {
                return Err(IdentityKeysError::TimedOut {
                    url: shown,
                    secs: FETCH_TIMEOUT.as_secs(),
                })
            }
        };
        if status != 200 {
            return Err(IdentityKeysError::Answered { url: shown, status });
        }
        body.ok_or(IdentityKeysError::Unusable {
            url: shown,
            why: "the answer is larger than 64 KiB",
        })
    }

    async fn exchange(
        &self,
        shown: &str,
        req: hyper::Request<Full<Bytes>>,
    ) -> Result<(u16, Option<Vec<u8>>), IdentityKeysError> {
        let u = &self.settings.url;
        let tcp = tokio::net::TcpStream::connect((u.host.as_str(), u.port))
            .await
            .map_err(|cause| IdentityKeysError::Unreachable {
                url: shown.to_string(),
                cause,
            })?;
        let protocol = |cause| IdentityKeysError::Protocol {
            url: shown.to_string(),
            cause,
        };
        match &self.tls {
            None => drive(TokioIo::new(tcp), req).await.map_err(protocol),
            Some(cfg) => {
                let tls_err =
                    |e: Box<dyn std::error::Error + Send + Sync>| IdentityKeysError::Tls {
                        url: shown.to_string(),
                        cause: e,
                    };
                let name = rustls::pki_types::ServerName::try_from(u.host.clone())
                    .map_err(|e| tls_err(Box::new(e)))?;
                let tls = tokio_rustls::TlsConnector::from(cfg.clone())
                    .connect(name, tcp)
                    .await
                    .map_err(|e| tls_err(Box::new(e)))?;
                drive(TokioIo::new(tls), req).await.map_err(protocol)
            }
        }
    }
}

/// One HTTP/1.1 exchange. The body is `None` past [`MAX_SET_BYTES`].
async fn drive<T>(
    io: T,
    req: hyper::Request<Full<Bytes>>,
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
    let body = Limited::new(res.into_body(), MAX_SET_BYTES)
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

    #[test]
    fn unset_url_means_no_deployment_tokens() {
        assert_eq!(IdentitySettings::from_vars(&vars(&[])), Ok(None));
    }

    #[test]
    fn the_settings_read_and_each_refusal_names_its_setting() {
        let ok = IdentitySettings::from_vars(&vars(&[
            (URL_ENV, "http://127.0.0.1:8082/"),
            (ISSUER_ENV, "https://console.example.com:8082"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(ok.url.display(), "http://127.0.0.1:8082");
        assert_eq!(ok.issuer, "https://console.example.com:8082");
        assert_eq!(ok.audience, "ordo-deployment:main");

        let e = IdentitySettings::from_vars(&vars(&[(URL_ENV, "http://127.0.0.1:8082")]));
        assert_eq!(e.unwrap_err().0, ISSUER_ENV);
        let e = IdentitySettings::from_vars(&vars(&[
            (URL_ENV, "http://identity:8082"),
            (ISSUER_ENV, "https://id.example.com"),
        ]))
        .unwrap_err();
        assert_eq!(e.0, URL_ENV);
        assert!(e.1.contains(PLAIN_HTTP_ENV), "{}", e.1);
        assert!(IdentitySettings::from_vars(&vars(&[
            (URL_ENV, "http://identity:8082"),
            (ISSUER_ENV, "https://id.example.com"),
            (PLAIN_HTTP_ENV, "on"),
        ]))
        .is_ok());
        let e = IdentitySettings::from_vars(&vars(&[
            (URL_ENV, "https://id.example.com"),
            (ISSUER_ENV, "https://id.example.com"),
            (PLAIN_HTTP_ENV, "yes"),
        ]))
        .unwrap_err();
        assert_eq!(e.0, PLAIN_HTTP_ENV);
        let e = IdentitySettings::from_vars(&vars(&[
            (URL_ENV, "ftp://x"),
            (ISSUER_ENV, "https://id.example.com"),
        ]))
        .unwrap_err();
        assert_eq!(e.0, URL_ENV);
        let e = IdentitySettings::from_vars(&vars(&[
            (URL_ENV, "https://id.example.com"),
            (ISSUER_ENV, "https://id.example.com/path"),
        ]))
        .unwrap_err();
        assert_eq!(e.0, ISSUER_ENV);
        let e = IdentitySettings::from_vars(&vars(&[
            (URL_ENV, "https://id.example.com"),
            (ISSUER_ENV, "https://id.example.com"),
            (DEPLOYMENT_ENV, "Main!"),
        ]))
        .unwrap_err();
        assert_eq!(e.0, DEPLOYMENT_ENV);
    }

    #[test]
    fn a_key_whose_kid_is_not_its_thumbprint_is_dropped() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let kid = deployment_token::key_id(&key.verifying_key());
        let good = deployment_token::public_jwk(&kid, &key.verifying_key());
        let liar = deployment_token::public_jwk("chosen-by-me", &key.verifying_key());
        let body = serde_json::json!({ "keys": [good, liar] }).to_string();
        let (jwks, text) = usable_set(body.as_bytes()).unwrap();
        assert_eq!(jwks.len(), 1);
        assert!(jwks.has(&kid) && !jwks.has("chosen-by-me"));
        assert!(usable_set(text.as_bytes()).is_ok());
        let only_liar = serde_json::json!({
            "keys": [deployment_token::public_jwk("x", &key.verifying_key())]
        });
        assert!(usable_set(only_liar.to_string().as_bytes()).is_err());
    }
}
