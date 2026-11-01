//! `fetch` for backend functions (`plan-backend-functions.md` §5, §6).
//!
//! A function's outbound call. It shares the dialler and the destination
//! pins with [`crate::deliver`] and [`crate::fetch`]:
//!
//! | Pin | Here |
//! |---|---|
//! | 1 · https, TLS 1.2 floor | [`HttpsUrl::parse`], the shared dialler |
//! | 2 · resolve, then connect to the pinned address | [`resolve_and_pin`] |
//! | 3 · no redirects | a 3xx is returned to the function as an answer; no code here reads where it points |
//! | 4 · signed | replaced by secret injection |
//! | 5 · holds nothing | the secret's bytes arrive per call through [`SecretSource`], which the host implements; this crate still names no key provider and no pool |
//! | 6 · allowlist, before DNS | the function's `egress` list, intersected with the deployment's list when it sets one |
//!
//! # Secret injection
//!
//! A function never holds a secret's value. It puts a handle in a header
//! ([`HeaderValue::Secret`]). [`FnEgress`] puts the value in as the request
//! is built, and only when:
//!
//! 1. the function lists the secret (`secrets openaiKey`),
//! 2. the program declares it (`secret openaiKey for api.openai.com`), and
//! 3. the destination host is one of the hosts it is bound to.
//!
//! Any other host is refused with [`FetchError::SecretNotBound`], which
//! names the secret and the host. This is checked before any DNS lookup, so
//! a prompt injection that steers the code to another allowed host still
//! cannot carry the key there.
//!
//! As a second guard, every injected value is replaced by `[secret]` in the
//! answer's headers and body, so a host that echoes a request back cannot
//! hand the value to the function either.
//!
//! # HMAC with a secret as the key
//!
//! A function signs or verifies an HMAC with a secret by its name
//! ([`FnFetch::mac`], owner ruling 2026-10-31). The value is opened here,
//! used as the key, and dropped; only the MAC bytes or a yes or no go back.
//! The function must list the secret and the program must declare it, as
//! for a header. A secret declared with no host (`secret hookKey`) is a
//! signing key: it is used here and sent nowhere.
//!
//! # What comes back
//!
//! Unlike the sink doors, the answer comes back whole: status, headers and a
//! bounded body, for every status. A function's hosts are named in the
//! program and approved by the deployment, so data can choose a path, never
//! a host (`plan-backend-functions.md` §6).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use crate::client::{self, BodyFault, Reader, TlsRoots};
use crate::deny::{DeniedCategory, DenyList};
use crate::dest::{resolve_and_pin, Allowlist, HttpsUrl, PinnedDestination};
use crate::refusal::Refusal;

/// Request body, by default: 32 MiB. Plan §6 said 10 MB; files in
/// (increment 2) raised it so a file of the whole default
/// `ORDO_FN_BLOB_BYTES` (20 MiB) goes out as base64 inside JSON.
pub const DEFAULT_REQUEST_BODY_CAP: usize = 32 * 1024 * 1024;
/// The setting that sets [`FnLimits::request_body`]. Named here because the
/// refusal names it.
pub const REQUEST_BODY_ENV: &str = "ORDO_FN_FETCH_BODY_BYTES";
/// Response body, by default: 25 MB (§6).
pub const DEFAULT_RESPONSE_BODY_CAP: usize = 25 * 1024 * 1024;
/// One request's time, by default: 60 s (§6). Covers DNS, connect, the
/// handshake, the write and the whole read.
pub const DEFAULT_FN_TIMEOUT: Duration = Duration::from_secs(60);
/// The text an injected value is replaced with in an answer.
pub const SCRUBBED: &str = "[secret]";

/// The verbs a function may send.
pub const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// Headers the kernel writes itself. A function that sets one is refused,
/// because each one is framing or routing: a second `host` or
/// `content-length` is request smuggling.
pub const RESERVED_HEADERS: [&str; 11] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
    "proxy-connection",
    "proxy-authorization",
    "accept-encoding",
];

/// One outbound request from a function.
///
/// **Not `Debug`.** A plain header can hold a token the function was given.
pub struct FnFetchRequest {
    /// `GET`, `POST`, ... (any case). See [`METHODS`].
    pub method: String,
    /// `https://host[:port]/path[?query]`.
    pub url: String,
    pub headers: Vec<(String, HeaderValue)>,
    pub body: Vec<u8>,
    /// The function's `egress` list. A host not on it is refused before
    /// any DNS lookup. Empty refuses every host.
    pub egress: Vec<String>,
    /// The function's `secrets` list. A handle for a secret not on it is
    /// refused.
    pub secrets: Vec<String>,
}

/// A header's value: text, or a secret handle the kernel fills in.
pub enum HeaderValue {
    Plain(String),
    /// `template` holds `{}` exactly once, where the value goes:
    /// `"Bearer {}"`, or `"{}"` for the value alone.
    Secret {
        name: String,
        template: String,
    },
}

/// What the destination answered. Every status comes back, 3xx included:
/// a redirect is an answer, and the function may fetch again, checked again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnFetchResponse {
    pub status: u16,
    /// Lowercased names, in the order received. Framing headers
    /// (`content-length`, `transfer-encoding`, `connection`, `keep-alive`)
    /// are dropped, because the body is already decoded.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A hash for [`FnFetch::mac`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacHash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl MacHash {
    /// `SHA-1`, `SHA-256`, `SHA-384` or `SHA-512`, as Web Crypto names them.
    pub fn named(name: &str) -> Option<MacHash> {
        match name {
            "SHA-1" => Some(MacHash::Sha1),
            "SHA-256" => Some(MacHash::Sha256),
            "SHA-384" => Some(MacHash::Sha384),
            "SHA-512" => Some(MacHash::Sha512),
            _ => None,
        }
    }
}

/// One HMAC with a secret as the key.
///
/// **Not `Debug`.** The data can be a request body the function was given.
pub struct FnMacRequest {
    /// The secret whose value is the key.
    pub secret: String,
    pub hash: MacHash,
    pub data: Vec<u8>,
    /// `None` signs. `Some` verifies these bytes against the MAC.
    pub signature: Option<Vec<u8>>,
    /// The function's `secrets` list. A secret not on it is refused.
    pub secrets: Vec<String>,
}

/// What [`FnFetch::mac`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FnMacAnswer {
    /// The MAC, for a sign.
    Mac(Vec<u8>),
    /// Whether the signature matched, for a verify. Compared in constant
    /// time.
    Verified(bool),
}

/// What a function reaches outside its instance through the host: an
/// outbound call, and an HMAC keyed by a secret. Both live here because
/// the egress kernel is the one holder of a secret's value. Part of the
/// seam with the function runtime, which holds an `Arc<dyn FnFetch>`.
#[async_trait]
pub trait FnFetch: Send + Sync {
    /// Send `req` for a function running in `space`.
    async fn fetch(&self, space: &str, req: FnFetchRequest) -> Result<FnFetchResponse, FetchError>;

    /// Sign or verify `req.data` with the value of `req.secret` as the HMAC
    /// key, for a function running in `space`. The value never leaves.
    async fn mac(&self, space: &str, req: FnMacRequest) -> Result<FnMacAnswer, FetchError>;
}

/// A secret's value, as the host hands it to the kernel.
///
/// **No `Debug`, no `Display`, no `Clone`, and no reader outside this
/// crate.** The host builds one from the bytes it unsealed; only the
/// request writer here reads it.
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> SecretBytes {
        SecretBytes(bytes)
    }

    fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }
}

/// Where the kernel gets a secret's binding and value. The host implements
/// it over the deployment's sealed store and the serving program.
#[async_trait]
pub trait SecretSource: Send + Sync {
    /// The hosts the serving program binds `name` to. `None` when the
    /// program declares no secret by that name.
    fn bound_hosts(&self, name: &str) -> Option<Vec<String>>;

    /// The value, unsealed. `Ok(None)` when no value is set. Called only
    /// after every host check has passed. `space` is the space the
    /// function runs in, for the key release's correlation.
    async fn open(&self, space: &str, name: &str) -> Result<Option<SecretBytes>, FetchError>;
}

/// What a caller can do about a [`FetchError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchAction {
    /// Try again later: the network, the far end or the store.
    Retry,
    /// Change the request: the URL, a header, the body.
    FixInput,
    /// Change the program or the deployment's settings: a host or a secret
    /// is not allowed, bound or set. Retrying gives the same answer.
    Refuse,
}

/// Which body passed its cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodySide {
    Request,
    Response,
}

/// Why a function's fetch did not get an answer. One typed error for the
/// whole door (AGENTS.md rule 9): [`FetchError::action`] says what a caller
/// can do, and [`FetchError::kind`] is the one reason word the function is
/// given beside `egress_refused`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FetchError {
    /// The URL is not `https`, or is not a usable URL.
    BadUrl { detail: String },
    /// The method, a header or a template cannot be sent.
    BadRequest { detail: String },
    /// The host is not on the function's `egress` list, or not on the
    /// deployment's list.
    HostNotAllowed { host: String },
    /// The host resolves inside the deployment. The category is
    /// disclosable; the address is not.
    HostDenied {
        host: String,
        category: DeniedCategory,
    },
    /// The function does not list this secret in its `secrets`.
    SecretNotListed { name: String },
    /// The program declares no secret by this name.
    SecretUnknown { name: String },
    /// The secret is declared but no value is set in this deployment.
    SecretNotSet { name: String },
    /// The secret is not bound to this host. Plan word
    /// `secret_host_mismatch`.
    SecretNotBound { secret: String, host: String },
    /// A body passed its cap.
    BodyTooLarge { limit: usize, side: BodySide },
    /// The whole request took longer than its time.
    Timeout { after_ms: u64 },
    /// The secret store could not answer.
    Store { detail: String },
    /// The connection, the handshake or the answer failed.
    Upstream(Refusal),
}

impl FetchError {
    /// The reason word the function sees.
    pub fn kind(&self) -> &'static str {
        match self {
            FetchError::BadUrl { .. } => "bad_url",
            FetchError::BadRequest { .. } => "bad_request",
            FetchError::HostNotAllowed { .. } => "host_not_allowed",
            FetchError::HostDenied { .. } => "host_denied",
            FetchError::SecretNotListed { .. } => "secret_not_listed",
            FetchError::SecretUnknown { .. } => "secret_unknown",
            FetchError::SecretNotSet { .. } => "secret_not_set",
            FetchError::SecretNotBound { .. } => "secret_host_mismatch",
            FetchError::BodyTooLarge { .. } => "body_too_large",
            FetchError::Timeout { .. } => "timed_out",
            FetchError::Store { .. } => "secret_store_unavailable",
            FetchError::Upstream(r) => r.kind(),
        }
    }

    /// What a caller can do.
    pub fn action(&self) -> FetchAction {
        match self {
            FetchError::BadUrl { .. }
            | FetchError::BadRequest { .. }
            | FetchError::BodyTooLarge { .. } => FetchAction::FixInput,
            FetchError::HostNotAllowed { .. }
            | FetchError::HostDenied { .. }
            | FetchError::SecretNotListed { .. }
            | FetchError::SecretUnknown { .. }
            | FetchError::SecretNotSet { .. }
            | FetchError::SecretNotBound { .. } => FetchAction::Refuse,
            FetchError::Timeout { .. } | FetchError::Store { .. } => FetchAction::Retry,
            FetchError::Upstream(r) if r.retryable() => FetchAction::Retry,
            FetchError::Upstream(_) => FetchAction::Refuse,
        }
    }

    /// Is another attempt worth making?
    pub fn retryable(&self) -> bool {
        self.action() == FetchAction::Retry
    }

    fn of_refusal(r: Refusal) -> FetchError {
        match r {
            Refusal::NotHttps { detail } | Refusal::MalformedUrl { detail } => {
                FetchError::BadUrl { detail }
            }
            Refusal::NotAllowlisted { host } => FetchError::HostNotAllowed { host },
            Refusal::DeniedAddress { host, category, .. } => {
                FetchError::HostDenied { host, category }
            }
            Refusal::TimedOut { after_ms } => FetchError::Timeout { after_ms },
            other => FetchError::Upstream(other),
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::BadUrl { detail } => write!(f, "the URL cannot be fetched: {detail}"),
            FetchError::BadRequest { detail } => write!(f, "the request cannot be sent: {detail}"),
            FetchError::HostNotAllowed { host } => write!(
                f,
                "`{host}` is not on this function's egress list; add it to the function's \
                 `egress` line"
            ),
            FetchError::HostDenied { host, category } => write!(
                f,
                "`{host}` resolves to a {} address, and a function may not reach inside the \
                 deployment",
                category.as_str()
            ),
            FetchError::SecretNotListed { name } => write!(
                f,
                "this function does not list secret `{name}`; add it to the function's \
                 `secrets` line"
            ),
            FetchError::SecretUnknown { name } => write!(
                f,
                "the program declares no secret `{name}`; declare it with \
                 `secret {name} for <host>`, or `secret {name}` for a signing key"
            ),
            FetchError::SecretNotSet { name } => write!(
                f,
                "secret `{name}` has no value in this deployment; the deployment owner sets it \
                 with `ordo secret set {name}`"
            ),
            FetchError::SecretNotBound { secret, host } => write!(
                f,
                "secret `{secret}` is not bound to `{host}`, so it is not sent there; the \
                 program binds it with `secret {secret} for <host>`"
            ),
            FetchError::BodyTooLarge {
                limit,
                side: BodySide::Request,
            } => write!(
                f,
                "the request body is over its limit of {limit} bytes; send less, or raise \
                 {REQUEST_BODY_ENV}"
            ),
            FetchError::BodyTooLarge {
                limit,
                side: BodySide::Response,
            } => write!(f, "the response body is over its limit of {limit} bytes"),
            FetchError::Timeout { after_ms } => write!(f, "timed out after {after_ms}ms"),
            FetchError::Store { detail } => {
                write!(f, "the secret store did not answer: {detail}; try again")
            }
            FetchError::Upstream(r) => f.write_str(&r.disclosable()),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FetchError::Upstream(r) => Some(r),
            _ => None,
        }
    }
}

/// The deployment's ceilings for one request. Settings with defaults; a
/// function may only lower them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnLimits {
    pub request_body: usize,
    pub response_body: usize,
    pub timeout: Duration,
}

impl Default for FnLimits {
    fn default() -> FnLimits {
        FnLimits {
            request_body: DEFAULT_REQUEST_BODY_CAP,
            response_body: DEFAULT_RESPONSE_BODY_CAP,
            timeout: DEFAULT_FN_TIMEOUT,
        }
    }
}

/// The one [`FnFetch`]: the egress kernel with host-bound secret injection.
pub struct FnEgress {
    secrets: Arc<dyn SecretSource>,
    roots: TlsRoots,
    deny: DenyList,
    limits: FnLimits,
    /// The deployment's own list. `None`: the program's lists stand, as
    /// approved by applying the program.
    deployment_hosts: Option<Allowlist>,
    /// Test only: names answered without DNS. Each address still passes
    /// the deny list.
    #[cfg(any(test, feature = "test-pin"))]
    fixed: HashMap<String, std::net::SocketAddr>,
}

impl FnEgress {
    pub fn new(secrets: Arc<dyn SecretSource>, roots: TlsRoots, deny: DenyList) -> FnEgress {
        FnEgress {
            secrets,
            roots,
            deny,
            limits: FnLimits::default(),
            deployment_hosts: None,
            #[cfg(any(test, feature = "test-pin"))]
            fixed: HashMap::new(),
        }
    }

    pub fn with_limits(mut self, limits: FnLimits) -> FnEgress {
        self.limits = limits;
        self
    }

    /// Intersect every function's `egress` list with this one.
    pub fn with_deployment_hosts(mut self, hosts: &[String]) -> FnEgress {
        self.deployment_hosts = Some(Allowlist::of(hosts));
        self
    }

    pub fn limits(&self) -> FnLimits {
        self.limits
    }

    /// **Tests only.** Answer `host` with `addr` instead of asking DNS.
    /// The allowlist, the secret bindings and the deny list still apply.
    #[cfg(any(test, feature = "test-pin"))]
    pub fn resolving_for_tests(mut self, host: &str, addr: std::net::SocketAddr) -> FnEgress {
        self.fixed.insert(host.to_ascii_lowercase(), addr);
        self
    }

    async fn pin(&self, url: &HttpsUrl, allow: &Allowlist) -> Result<PinnedDestination, Refusal> {
        #[cfg(any(test, feature = "test-pin"))]
        if let Some(addr) = self.fixed.get(url.host()) {
            if !allow.permits(url.host()) {
                return Err(Refusal::NotAllowlisted {
                    host: url.host().to_string(),
                });
            }
            if let Err(category) = self.deny.check_host(url.host(), addr.ip()) {
                return Err(Refusal::DeniedAddress {
                    host: url.host().to_string(),
                    address: addr.ip(),
                    category,
                });
            }
            let mut a = *addr;
            a.set_port(url.port());
            return Ok(crate::dest::pin_for_test(url.host(), a, url.path()));
        }
        resolve_and_pin(url, allow, &self.deny).await
    }

    /// Every check that needs no network, in order: the body cap, the URL,
    /// the method, the allowlist, then each secret handle.
    fn check<'r>(&self, req: &'r FnFetchRequest) -> Result<Checked<'r>, FetchError> {
        if req.body.len() > self.limits.request_body {
            return Err(FetchError::BodyTooLarge {
                limit: self.limits.request_body,
                side: BodySide::Request,
            });
        }
        let url = HttpsUrl::parse(&req.url).map_err(FetchError::of_refusal)?;
        let method = req.method.to_ascii_uppercase();
        if !METHODS.contains(&method.as_str()) {
            return Err(FetchError::BadRequest {
                detail: format!(
                    "method `{}` is not one of {}",
                    req.method,
                    METHODS.join(", ")
                ),
            });
        }
        let host = url.host().to_string();
        let listed = Allowlist::of(&req.egress);
        let deployment_ok = self
            .deployment_hosts
            .as_ref()
            .is_none_or(|d| d.permits(&host));
        if !listed.permits(&host) || !deployment_ok {
            return Err(FetchError::HostNotAllowed { host });
        }
        let mut handles = Vec::new();
        for (name, value) in &req.headers {
            let lower = name.to_ascii_lowercase();
            if !client::header_token_ok(&lower) {
                return Err(FetchError::BadRequest {
                    detail: format!("header name `{name}` is not representable"),
                });
            }
            if RESERVED_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-") {
                return Err(FetchError::BadRequest {
                    detail: format!("header `{lower}` is written by Ordo and cannot be set"),
                });
            }
            match value {
                HeaderValue::Plain(v) => {
                    if !client::header_value_ok(v) {
                        return Err(FetchError::BadRequest {
                            detail: format!(
                                "header `{lower}` has a value that is not representable"
                            ),
                        });
                    }
                }
                HeaderValue::Secret {
                    name: secret,
                    template,
                } => {
                    if template.matches("{}").count() != 1 || !client::header_value_ok(template) {
                        return Err(FetchError::BadRequest {
                            detail: format!(
                                "header `{lower}`: a secret's template holds `{{}}` exactly once, \
                                 such as \"Bearer {{}}\""
                            ),
                        });
                    }
                    if !req.secrets.iter().any(|s| s == secret) {
                        return Err(FetchError::SecretNotListed {
                            name: secret.clone(),
                        });
                    }
                    let Some(bound) = self.secrets.bound_hosts(secret) else {
                        return Err(FetchError::SecretUnknown {
                            name: secret.clone(),
                        });
                    };
                    if !Allowlist::of(&bound).permits(&host) {
                        return Err(FetchError::SecretNotBound {
                            secret: secret.clone(),
                            host,
                        });
                    }
                    handles.push(secret.as_str());
                }
            }
        }
        Ok(Checked {
            url,
            method,
            handles,
        })
    }

    async fn send(
        &self,
        req: &FnFetchRequest,
        checked: Checked<'_>,
        values: &HashMap<&str, SecretBytes>,
    ) -> Result<FnFetchResponse, FetchError> {
        let dest = self
            .pin(&checked.url, &checked.allow_for_pin())
            .await
            .map_err(FetchError::of_refusal)?;
        let request = build(&dest, &checked.method, req, values)?;
        let mut tls = client::dial(&dest, &self.roots)
            .await
            .map_err(FetchError::of_refusal)?;
        client::send(&mut tls, &request)
            .await
            .map_err(FetchError::of_refusal)?;
        drop(request);
        let mut reader = Reader::new(&mut tls);
        let head = reader.head().await.map_err(FetchError::of_refusal)?;
        let status = client::parse_status_line(&head).map_err(FetchError::of_refusal)?;
        let headers = headers_of(&head);
        let bodiless = checked.method == "HEAD" || matches!(status, 100..=199 | 204 | 304);
        let body = if bodiless {
            Vec::new()
        } else {
            let framing = client::framing_of(&head).map_err(FetchError::of_refusal)?;
            let cap = self.limits.response_body;
            let max_chunks = client::MAX_CHUNKS.max(cap / 256);
            reader
                .body(framing, cap, max_chunks)
                .await
                .map_err(|f| match f {
                    BodyFault::OverCap => FetchError::BodyTooLarge {
                        limit: cap,
                        side: BodySide::Response,
                    },
                    BodyFault::Refused(r) => FetchError::of_refusal(r),
                })?
        };
        let _ = tls.shutdown().await;
        let mut out = FnFetchResponse {
            status,
            headers,
            body,
        };
        scrub(&mut out, values);
        Ok(out)
    }
}

/// A request that passed every check that needs no network.
struct Checked<'r> {
    url: HttpsUrl,
    method: String,
    /// The secrets the headers name, each already checked against the host.
    handles: Vec<&'r str>,
}

impl Checked<'_> {
    /// The allowlist has already been applied in [`FnEgress::check`]; the
    /// pin applies it again, to the one host, as every door does.
    fn allow_for_pin(&self) -> Allowlist {
        Allowlist::exactly(self.url.host())
    }
}

#[async_trait]
impl FnFetch for FnEgress {
    async fn fetch(&self, space: &str, req: FnFetchRequest) -> Result<FnFetchResponse, FetchError> {
        let checked = self.check(&req)?;
        let ms = self.limits.timeout.as_millis() as u64;
        let work = async {
            let mut values: HashMap<&str, SecretBytes> = HashMap::new();
            for name in checked.handles.iter().copied() {
                if values.contains_key(name) {
                    continue;
                }
                let Some(v) = self.secrets.open(space, name).await? else {
                    return Err(FetchError::SecretNotSet {
                        name: name.to_string(),
                    });
                };
                values.insert(name, v);
            }
            self.send(&req, checked, &values).await
        };
        match tokio::time::timeout(self.limits.timeout, work).await {
            Ok(r) => r,
            Err(_) => Err(FetchError::Timeout { after_ms: ms }),
        }
    }

    async fn mac(&self, space: &str, req: FnMacRequest) -> Result<FnMacAnswer, FetchError> {
        let name = req.secret.as_str();
        if !req.secrets.iter().any(|s| s == name) {
            return Err(FetchError::SecretNotListed {
                name: name.to_string(),
            });
        }
        if self.secrets.bound_hosts(name).is_none() {
            return Err(FetchError::SecretUnknown {
                name: name.to_string(),
            });
        }
        let ms = self.limits.timeout.as_millis() as u64;
        let key =
            match tokio::time::timeout(self.limits.timeout, self.secrets.open(space, name)).await {
                Ok(r) => r?,
                Err(_) => return Err(FetchError::Timeout { after_ms: ms }),
            };
        let Some(key) = key else {
            return Err(FetchError::SecretNotSet {
                name: name.to_string(),
            });
        };
        Ok(hmac_with(
            req.hash,
            &key.0,
            &req.data,
            req.signature.as_deref(),
        ))
    }
}

/// The HMAC of `data` under `key`, or whether `signature` is it. The
/// comparison is the `hmac` crate's, which takes the same time wherever
/// the bytes differ.
fn hmac_with(hash: MacHash, key: &[u8], data: &[u8], signature: Option<&[u8]>) -> FnMacAnswer {
    use hmac::{Hmac, Mac};
    macro_rules! run {
        ($t:ty) => {{
            // HMAC takes a key of any length, so this cannot fail.
            let mut m = <Hmac<$t>>::new_from_slice(key).expect("HMAC takes any key length");
            m.update(data);
            match signature {
                None => FnMacAnswer::Mac(m.finalize().into_bytes().to_vec()),
                Some(sig) => FnMacAnswer::Verified(m.verify_slice(sig).is_ok()),
            }
        }};
    }
    match hash {
        MacHash::Sha1 => run!(sha1::Sha1),
        MacHash::Sha256 => run!(sha2::Sha256),
        MacHash::Sha384 => run!(sha2::Sha384),
        MacHash::Sha512 => run!(sha2::Sha512),
    }
}

/// The request bytes, with each handle filled in.
fn build(
    dest: &PinnedDestination,
    method: &str,
    req: &FnFetchRequest,
    values: &HashMap<&str, SecretBytes>,
) -> Result<Vec<u8>, FetchError> {
    let mut head = String::with_capacity(512);
    head.push_str(method);
    head.push(' ');
    head.push_str(dest.path());
    head.push_str(" HTTP/1.1\r\n");
    let mut put = |name: &str, value: &str| {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    };
    put("host", &client::host_header(dest));
    put("connection", "close");
    put("accept-encoding", "identity");
    let sends_body = !req.body.is_empty() || matches!(method, "POST" | "PUT" | "PATCH");
    if sends_body {
        put("content-length", &req.body.len().to_string());
    }
    let mut agent = true;
    for (name, value) in &req.headers {
        let name = name.to_ascii_lowercase();
        agent &= name != "user-agent";
        match value {
            HeaderValue::Plain(v) => put(&name, v),
            HeaderValue::Secret {
                name: secret,
                template,
            } => {
                let text = values
                    .get(secret.as_str())
                    .and_then(SecretBytes::text)
                    .filter(|t| client::header_value_ok(t))
                    .ok_or_else(|| FetchError::BadRequest {
                        detail: format!(
                            "secret `{secret}` cannot be written into header `{name}`: its \
                             value is not printable ASCII"
                        ),
                    })?;
                put(&name, &template.replacen("{}", text, 1));
            }
        }
    }
    if agent {
        put("user-agent", "ordo-egress/1");
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if sends_body {
        out.extend_from_slice(&req.body);
    }
    Ok(out)
}

/// The answer's headers, lowercased, without the framing ones.
fn headers_of(head: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(head);
    text.split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
        .filter(|(n, _)| {
            !n.is_empty()
                && !matches!(
                    n.as_str(),
                    "content-length" | "transfer-encoding" | "connection" | "keep-alive"
                )
        })
        .collect()
}

/// Replace every injected value in the answer with [`SCRUBBED`].
fn scrub(out: &mut FnFetchResponse, values: &HashMap<&str, SecretBytes>) {
    for v in values.values() {
        if v.0.is_empty() {
            continue;
        }
        out.body = replace_all(&out.body, &v.0, SCRUBBED.as_bytes());
        if let Some(t) = v.text() {
            for (_, value) in out.headers.iter_mut() {
                if value.contains(t) {
                    *value = value.replace(t, SCRUBBED);
                }
            }
        }
    }
}

fn replace_all(hay: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    if needle.len() > hay.len() {
        return hay.to_vec();
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(needle) {
            out.extend_from_slice(with);
            i += needle.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dest::pin_for_test;

    struct Fixed(Vec<(&'static str, Vec<String>, &'static str)>);

    #[async_trait]
    impl SecretSource for Fixed {
        fn bound_hosts(&self, name: &str) -> Option<Vec<String>> {
            self.0
                .iter()
                .find(|(n, _, _)| *n == name)
                .map(|(_, h, _)| h.clone())
        }
        async fn open(&self, _: &str, name: &str) -> Result<Option<SecretBytes>, FetchError> {
            Ok(self
                .0
                .iter()
                .find(|(n, _, _)| *n == name)
                .map(|(_, _, v)| SecretBytes::new(v.as_bytes().to_vec())))
        }
    }

    fn egress() -> FnEgress {
        FnEgress::new(
            Arc::new(Fixed(vec![(
                "openaiKey",
                vec!["api.openai.com".into()],
                "sk-live-123",
            )])),
            TlsRoots::webpki(),
            DenyList::categories_only(),
        )
    }

    fn req(url: &str, headers: Vec<(String, HeaderValue)>) -> FnFetchRequest {
        FnFetchRequest {
            method: "post".into(),
            url: url.into(),
            headers,
            body: b"{}".to_vec(),
            egress: vec!["api.openai.com".into(), "evil.example.com".into()],
            secrets: vec!["openaiKey".into()],
        }
    }

    fn bearer() -> (String, HeaderValue) {
        (
            "Authorization".into(),
            HeaderValue::Secret {
                name: "openaiKey".into(),
                template: "Bearer {}".into(),
            },
        )
    }

    #[test]
    fn a_secret_goes_only_to_a_bound_host_and_the_refusal_names_both() {
        let e = egress();
        assert!(e
            .check(&req("https://api.openai.com/v1/x", vec![bearer()]))
            .is_ok());
        let err = e
            .check(&req("https://evil.example.com/steal", vec![bearer()]))
            .err()
            .unwrap();
        assert_eq!(
            err,
            FetchError::SecretNotBound {
                secret: "openaiKey".into(),
                host: "evil.example.com".into()
            }
        );
        assert_eq!(err.kind(), "secret_host_mismatch");
        assert_eq!(err.action(), FetchAction::Refuse);
        let text = err.to_string();
        assert!(
            text.contains("openaiKey") && text.contains("evil.example.com"),
            "{text}"
        );
    }

    #[test]
    fn a_handle_needs_the_function_to_list_it_and_the_program_to_declare_it() {
        let e = egress();
        let mut r = req("https://api.openai.com/", vec![bearer()]);
        r.secrets.clear();
        assert!(matches!(
            e.check(&r).err().unwrap(),
            FetchError::SecretNotListed { .. }
        ));
        let mut r = req(
            "https://api.openai.com/",
            vec![(
                "x-key".into(),
                HeaderValue::Secret {
                    name: "other".into(),
                    template: "{}".into(),
                },
            )],
        );
        r.secrets.push("other".into());
        assert_eq!(
            e.check(&r).err().unwrap(),
            FetchError::SecretUnknown {
                name: "other".into()
            }
        );
    }

    #[test]
    fn hosts_off_the_egress_list_are_refused_before_dns() {
        let e = egress();
        let err = e
            .check(&req("https://api.anthropic.com/", vec![]))
            .err()
            .unwrap();
        assert_eq!(
            err,
            FetchError::HostNotAllowed {
                host: "api.anthropic.com".into()
            }
        );
        let e = egress().with_deployment_hosts(&["api.openai.com".into()]);
        assert!(e.check(&req("https://evil.example.com/", vec![])).is_err());
        assert!(e.check(&req("https://api.openai.com/", vec![])).is_ok());
        let err = e
            .check(&req("http://api.openai.com/", vec![]))
            .err()
            .unwrap();
        assert_eq!(err.kind(), "bad_url");
    }

    #[test]
    fn reserved_headers_bad_templates_and_injection_are_refused() {
        let e = egress();
        for name in ["Host", "content-length", "Transfer-Encoding", "proxy-foo"] {
            let r = req(
                "https://api.openai.com/",
                vec![(name.into(), HeaderValue::Plain("x".into()))],
            );
            assert_eq!(e.check(&r).err().unwrap().kind(), "bad_request", "{name}");
        }
        let r = req(
            "https://api.openai.com/",
            vec![(
                "x-a".into(),
                HeaderValue::Plain("a\r\nX-Injected: 1".into()),
            )],
        );
        assert_eq!(e.check(&r).err().unwrap().kind(), "bad_request");
        for template in ["Bearer", "{} {}", "Bearer {}\r\n"] {
            let r = req(
                "https://api.openai.com/",
                vec![(
                    "authorization".into(),
                    HeaderValue::Secret {
                        name: "openaiKey".into(),
                        template: template.into(),
                    },
                )],
            );
            assert_eq!(
                e.check(&r).err().unwrap().kind(),
                "bad_request",
                "{template}"
            );
        }
        let mut r = req("https://api.openai.com/", vec![]);
        r.method = "CONNECT".into();
        assert_eq!(e.check(&r).err().unwrap().kind(), "bad_request");
    }

    #[test]
    fn the_request_body_cap_is_checked_first() {
        let e = egress().with_limits(FnLimits {
            request_body: 4,
            ..FnLimits::default()
        });
        let mut r = req("https://nowhere.invalid/", vec![]);
        r.body = vec![0; 5];
        let err = e.check(&r).err().unwrap();
        assert_eq!(
            err,
            FetchError::BodyTooLarge {
                limit: 4,
                side: BodySide::Request
            }
        );
        assert!(
            err.to_string().contains("ORDO_FN_FETCH_BODY_BYTES"),
            "the refusal names the setting: {err}"
        );
        // At the cap exactly, the body passes the cap check.
        r.body = vec![0; 4];
        assert_ne!(e.check(&r).err().map(|e| e.kind()), Some("body_too_large"));
    }

    #[test]
    fn the_value_is_written_only_into_its_header() {
        let dest = pin_for_test("api.openai.com", "203.0.113.7:443".parse().unwrap(), "/v1");
        let r = req("https://api.openai.com/v1", vec![bearer()]);
        let mut values = HashMap::new();
        values.insert("openaiKey", SecretBytes::new(b"sk-live-123".to_vec()));
        let text = String::from_utf8(build(&dest, "POST", &r, &values).unwrap()).unwrap();
        assert!(
            text.starts_with("POST /v1 HTTP/1.1\r\nhost: api.openai.com\r\n"),
            "{text}"
        );
        assert!(
            text.contains("authorization: Bearer sk-live-123\r\n"),
            "{text}"
        );
        assert_eq!(text.matches("sk-live-123").count(), 1, "{text}");
        assert!(text.ends_with("\r\n\r\n{}"), "{text}");
        values.insert("openaiKey", SecretBytes::new(b"bad\nvalue".to_vec()));
        let err = build(&dest, "POST", &r, &values).unwrap_err();
        assert!(!err.to_string().contains("bad\nvalue"), "{err}");
    }

    #[test]
    fn an_echoed_value_is_scrubbed_from_the_answer() {
        let mut values = HashMap::new();
        values.insert("k", SecretBytes::new(b"sk-live-123".to_vec()));
        let mut out = FnFetchResponse {
            status: 200,
            headers: vec![("x-echo".into(), "Bearer sk-live-123".into())],
            body: b"{\"auth\":\"Bearer sk-live-123\"}".to_vec(),
        };
        scrub(&mut out, &values);
        assert_eq!(out.body, b"{\"auth\":\"Bearer [secret]\"}");
        assert_eq!(out.headers[0].1, "Bearer [secret]");
    }

    #[test]
    fn framing_headers_are_dropped_from_the_answer() {
        let h = headers_of(
            b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\nX-Request-Id: a\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(h, vec![("x-request-id".to_string(), "a".to_string())]);
    }

    #[test]
    fn no_code_here_reads_where_a_redirect_points() {
        // Pin 3, as `client.rs` asserts it for itself.
        let src = include_str!("function.rs");
        let code = src
            .split("#[cfg(test)]")
            .next()
            .unwrap()
            .to_ascii_lowercase();
        let hits = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains("\"location\""))
            .count();
        assert_eq!(hits, 0);
    }

    /// `hookKey` is declared with no host and set; `unsetKey` is declared
    /// and has no value.
    struct Signing;

    #[async_trait]
    impl SecretSource for Signing {
        fn bound_hosts(&self, name: &str) -> Option<Vec<String>> {
            matches!(name, "hookKey" | "unsetKey").then(Vec::new)
        }
        async fn open(&self, _: &str, name: &str) -> Result<Option<SecretBytes>, FetchError> {
            Ok((name == "hookKey").then(|| SecretBytes::new(b"Jefe".to_vec())))
        }
    }

    fn mac_req(secret: &str, signature: Option<Vec<u8>>) -> FnMacRequest {
        FnMacRequest {
            secret: secret.into(),
            hash: MacHash::Sha256,
            data: b"what do ya want for nothing?".to_vec(),
            signature,
            secrets: vec!["hookKey".into(), "unsetKey".into(), "strayKey".into()],
        }
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn a_mac_is_keyed_by_the_secret_and_its_value_stays_here() {
        let e = FnEgress::new(
            Arc::new(Signing),
            TlsRoots::webpki(),
            DenyList::categories_only(),
        );
        // RFC 4231 test case 2.
        let want = unhex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(
            e.mac("acme", mac_req("hookKey", None)).await.unwrap(),
            FnMacAnswer::Mac(want.clone())
        );
        assert_eq!(
            e.mac("acme", mac_req("hookKey", Some(want.clone())))
                .await
                .unwrap(),
            FnMacAnswer::Verified(true)
        );
        let mut bad = want.clone();
        bad[31] ^= 1;
        assert_eq!(
            e.mac("acme", mac_req("hookKey", Some(bad))).await.unwrap(),
            FnMacAnswer::Verified(false)
        );
        assert_eq!(
            e.mac("acme", mac_req("hookKey", Some(want[..16].to_vec())))
                .await
                .unwrap(),
            FnMacAnswer::Verified(false)
        );

        let mut r = mac_req("hookKey", None);
        r.secrets = vec![];
        let err = e.mac("acme", r).await.unwrap_err();
        assert_eq!(err.kind(), "secret_not_listed");
        let err = e.mac("acme", mac_req("strayKey", None)).await.unwrap_err();
        assert_eq!(err.kind(), "secret_unknown");
        let err = e.mac("acme", mac_req("unsetKey", None)).await.unwrap_err();
        assert_eq!(err.kind(), "secret_not_set");
        assert!(
            err.to_string().contains("ordo secret set unsetKey"),
            "{err}"
        );
        assert_eq!(err.action(), FetchAction::Refuse);
    }
}
