//! The console's one client to the deployment's door (AGENTS.md rule 9,
//! `plan-console.md` 3 and 8).
//!
//! Every call the console makes to the deployment goes through
//! [`DoorClient`], over the door's own HTTP API: the sign-in routes and
//! `/op/<name>`. Every failure is one [`DoorError`], which keeps its cause
//! and says what a caller can do through [`DoorError::class`]: retry when
//! the deployment is busy or not answering, stop when it refused. The
//! console's edge (`crate::error`) turns it into an HTTP answer in one
//! place.
//!
//! The console never opens the deployment's database. This client is the
//! only way it reaches the deployment's state, and `tests/door_scan.rs`
//! fails if a socket to it is opened anywhere else.
//!
//! # Plain HTTP
//!
//! The console key session and the owner's relayed passkey assertion cross
//! this link, so `http://` is taken only to a loopback address, which is
//! what the one image uses. Any other `http://` address is refused at boot
//! naming `ORDO_CONSOLE_DOOR_URL`, unless the deployment owner sets
//! `ORDO_CONSOLE_DOOR_PLAIN_HTTP=on` for a private network they trust.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use ordo_fault::FaultClass;
use serde_json::Value;

use crate::config::{DOOR_PLAIN_HTTP_ENV, DOOR_URL_ENV};
use crate::error::ConsoleError;

/// How long one call may take, connect to last byte.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an op that changes the deployment's program may take: an apply
/// waits up to 30 seconds for another apply's lock, then runs its schema
/// changes. Only [`DoorClient::op_within`] callers use it.
pub const DEPLOY_CALL_TIMEOUT: Duration = Duration::from_secs(180);
/// The largest answer the console reads from the door.
pub const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;

/// The space header the door binds an op call to.
pub const SPACE_HEADER: &str = "x-ordo-space";

/// Every way a call to the door can fail.
#[derive(Debug)]
#[non_exhaustive]
pub enum DoorError {
    /// No connection: the deployment is down, restarting, or not at the
    /// address the console holds. Worth retrying.
    Unreachable { url: String, cause: std::io::Error },
    /// The call did not finish within its time, `secs`. Worth retrying.
    TimedOut { url: String, secs: u64 },
    /// TLS could not be set up with the deployment's address. A setting
    /// or a certificate to fix, not a retry.
    Tls {
        url: String,
        cause: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The door answered that it is busy (429 or 503). Worth retrying.
    Busy { status: u16 },
    /// The door answered with a refusal (another 4xx). `code` is the
    /// door's own word, `message` its sentence.
    Refused {
        status: u16,
        code: String,
        message: String,
    },
    /// The door answered with a failure of its own (5xx), or an answer the
    /// console cannot read.
    Failed { status: u16, message: String },
    /// The HTTP exchange broke part way.
    Protocol { url: String, cause: hyper::Error },
}

impl DoorError {
    /// What a caller can do about it, from the case and never the words.
    pub fn class(&self) -> FaultClass {
        match self {
            DoorError::Unreachable { .. } | DoorError::TimedOut { .. } => FaultClass::Retryable,
            DoorError::Busy { .. } => FaultClass::Busy,
            DoorError::Refused { .. } | DoorError::Tls { .. } => FaultClass::Refused,
            DoorError::Failed { .. } => FaultClass::Broken,
            DoorError::Protocol { .. } => FaultClass::Retryable,
        }
    }

    /// The door's own error word, when it answered with one.
    pub fn door_code(&self) -> Option<&str> {
        match self {
            DoorError::Refused { code, .. } => Some(code),
            _ => None,
        }
    }

    /// The deployment did not answer at all.
    pub fn not_answering(&self) -> bool {
        matches!(
            self,
            DoorError::Unreachable { .. } | DoorError::TimedOut { .. } | DoorError::Protocol { .. }
        )
    }
}

impl fmt::Display for DoorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DoorError::Unreachable { url, cause } => {
                write!(f, "the deployment is not answering at {url}: {cause}")
            }
            DoorError::TimedOut { url, secs } => write!(
                f,
                "the deployment at {url} did not answer within {secs} seconds"
            ),
            DoorError::Tls { url, cause } => write!(
                f,
                "a TLS connection to {url} could not be made ({cause}); check \
                 {DOOR_URL_ENV} and the deployment's certificate"
            ),
            DoorError::Busy { status } => {
                write!(
                    f,
                    "the deployment is busy (HTTP {status}); try again shortly"
                )
            }
            DoorError::Refused {
                status,
                code,
                message,
            } => write!(
                f,
                "the deployment refused (HTTP {status}, {code}): {message}"
            ),
            DoorError::Failed { status, message } => {
                write!(f, "the deployment failed (HTTP {status}): {message}")
            }
            DoorError::Protocol { url, cause } => {
                write!(f, "the exchange with {url} broke off: {cause}")
            }
        }
    }
}

impl std::error::Error for DoorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DoorError::Unreachable { cause, .. } => Some(cause),
            DoorError::Tls { cause, .. } => Some(cause.as_ref()),
            DoorError::Protocol { cause, .. } => Some(cause),
            _ => None,
        }
    }
}

impl From<DoorError> for ConsoleError {
    fn from(e: DoorError) -> ConsoleError {
        ConsoleError::Door(e)
    }
}

/// Where the door is: scheme, host, port and a path prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoorUrl {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Empty, or a path starting `/` with no trailing `/`.
    pub prefix: String,
}

impl DoorUrl {
    /// Read `ORDO_CONSOLE_DOOR_URL`. `plain_ok` is
    /// `ORDO_CONSOLE_DOOR_PLAIN_HTTP=on`.
    pub fn parse(text: &str, plain_ok: bool) -> Result<DoorUrl, ConsoleError> {
        let bad = |why: &str| {
            ConsoleError::setting(
                DOOR_URL_ENV,
                format!("is `{text}`, which {why}"),
                format!(
                    "Set {DOOR_URL_ENV} to the deployment's address, such as \
                     http://127.0.0.1:8080 in the one image or https://app.example.com"
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
        let default = if https { 443 } else { 80 };
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
            None => default,
            Some(p) => p
                .parse::<u16>()
                .map_err(|_| bad("has a port that is not a number"))?,
        };
        let host = host.to_string();
        if host.is_empty() {
            return Err(bad("names no host"));
        }
        if !https && !plain_ok && !is_loopback(&host) {
            return Err(ConsoleError::setting(
                DOOR_URL_ENV,
                format!(
                    "is `{text}`: plain http:// to an address that is not loopback would \
                     carry the console key's session and the owner's touch unencrypted"
                ),
                format!(
                    "Use https://, or set {DOOR_PLAIN_HTTP_ENV}=on if the console and the \
                     deployment share a private network you trust"
                ),
            ));
        }
        Ok(DoorUrl {
            https,
            host,
            port,
            prefix,
        })
    }

    /// The address as text, for messages.
    pub fn display(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("{scheme}://{host}:{}{}", self.port, self.prefix)
    }
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Percent-encode one query value.
pub(crate) fn encode(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The one client. Cheap to clone.
#[derive(Clone)]
pub struct DoorClient {
    url: DoorUrl,
    tls: Option<Arc<rustls::ClientConfig>>,
}

impl fmt::Debug for DoorClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DoorClient({})", self.url.display())
    }
}

impl DoorClient {
    pub fn new(url: DoorUrl) -> DoorClient {
        let tls = url.https.then(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        });
        DoorClient { url, tls }
    }

    pub fn url(&self) -> &DoorUrl {
        &self.url
    }

    /// `POST <path>` with a JSON body, and the bearer when there is one.
    pub async fn post(
        &self,
        path: &str,
        bearer: Option<&str>,
        space: Option<&str>,
        body: &Value,
    ) -> Result<Value, DoorError> {
        self.call("POST", path, bearer, space, Some(body), CALL_TIMEOUT)
            .await
    }

    /// `GET <path>`.
    pub async fn get(
        &self,
        path: &str,
        bearer: Option<&str>,
        space: Option<&str>,
    ) -> Result<Value, DoorError> {
        self.call("GET", path, bearer, space, None, CALL_TIMEOUT)
            .await
    }

    /// Call a platform op in `space`: a read as `GET` with its parameters
    /// in the query, anything else as `POST` with them as the body.
    /// Answers the op's rows.
    pub async fn op(
        &self,
        name: &str,
        space: &str,
        bearer: &str,
        read: bool,
        params: &Value,
    ) -> Result<Vec<Value>, DoorError> {
        self.op_within(name, space, bearer, read, params, CALL_TIMEOUT)
            .await
    }

    /// [`DoorClient::op`], allowed `timeout` rather than [`CALL_TIMEOUT`].
    pub async fn op_within(
        &self,
        name: &str,
        space: &str,
        bearer: &str,
        read: bool,
        params: &Value,
        timeout: Duration,
    ) -> Result<Vec<Value>, DoorError> {
        let answer = if read {
            let query: Vec<String> = params
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| {
                            let v = match v {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            format!("{}={}", encode(k), encode(&v))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let path = if query.is_empty() {
                format!("/op/{name}")
            } else {
                format!("/op/{name}?{}", query.join("&"))
            };
            self.call("GET", &path, Some(bearer), Some(space), None, timeout)
                .await?
        } else {
            self.call(
                "POST",
                &format!("/op/{name}"),
                Some(bearer),
                Some(space),
                Some(params),
                timeout,
            )
            .await?
        };
        match answer.get("rows").and_then(Value::as_array) {
            Some(rows) => Ok(rows.clone()),
            None => Err(DoorError::Failed {
                status: 200,
                message: format!("{name} answered without rows"),
            }),
        }
    }

    /// `GET <path>` for a route that answers one JSON record a line
    /// (`/usage`): the records, in order.
    pub async fn get_lines(
        &self,
        path: &str,
        bearer: Option<&str>,
        space: Option<&str>,
    ) -> Result<Vec<Value>, DoorError> {
        let answer = self
            .exchange_json("GET", path, bearer, space, None, CALL_TIMEOUT, true)
            .await?;
        Ok(match answer {
            Value::Array(lines) => lines,
            _ => Vec::new(),
        })
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        space: Option<&str>,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value, DoorError> {
        self.exchange_json(method, path, bearer, space, body, timeout, false)
            .await
    }

    /// One call. `lines`: a 2xx answer is one JSON record a line, gathered
    /// into an array; any other answer is the door's JSON error.
    #[allow(clippy::too_many_arguments)]
    async fn exchange_json(
        &self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        space: Option<&str>,
        body: Option<&Value>,
        timeout: Duration,
        lines: bool,
    ) -> Result<Value, DoorError> {
        let shown = format!("{}{path}", self.url.display());
        let accept = if lines {
            "application/x-ndjson, application/json"
        } else {
            "application/json"
        };
        let mut req = hyper::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.url.prefix))
            .header(hyper::header::HOST, self.host_header())
            .header(hyper::header::ACCEPT, accept);
        if let Some(t) = bearer {
            req = req.header(hyper::header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if let Some(s) = space {
            req = req.header(SPACE_HEADER, s);
        }
        let bytes = match body {
            Some(b) => {
                req = req.header(hyper::header::CONTENT_TYPE, "application/json");
                Bytes::from(b.to_string())
            }
            None => Bytes::new(),
        };
        let req = req.body(Full::new(bytes)).map_err(|e| DoorError::Failed {
            status: 0,
            message: format!("the request could not be built: {e}"),
        })?;
        let exchange = self.exchange(&shown, req);
        let (status, raw) = match tokio::time::timeout(timeout, exchange).await {
            Ok(r) => r?,
            Err(_) => {
                return Err(DoorError::TimedOut {
                    url: shown,
                    secs: timeout.as_secs(),
                })
            }
        };
        if lines && (200..=299).contains(&status) {
            let text = String::from_utf8_lossy(&raw);
            return text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| {
                    serde_json::from_str::<Value>(l).map_err(|e| DoorError::Failed {
                        status,
                        message: format!("{shown} answered a line that is not JSON: {e}"),
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array);
        }
        let json: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        match status {
            200..=299 => Ok(json),
            429 | 503 => Err(DoorError::Busy { status }),
            400..=499 => {
                let err = json.get("error").cloned().unwrap_or(Value::Null);
                // The door's two envelopes: `{"error": {"code", "message"}}`
                // and `{"error": "code", "message"}`.
                let (code, message) = match &err {
                    Value::Object(o) => (
                        o.get("code").and_then(Value::as_str).unwrap_or("refused"),
                        o.get("message").and_then(Value::as_str).unwrap_or(""),
                    ),
                    Value::String(s) => (
                        s.as_str(),
                        json.get("message").and_then(Value::as_str).unwrap_or(""),
                    ),
                    _ => ("refused", ""),
                };
                Err(DoorError::Refused {
                    status,
                    code: code.to_string(),
                    message: message.to_string(),
                })
            }
            _ => Err(DoorError::Failed {
                status,
                message: json
                    .pointer("/error/message")
                    .or_else(|| json.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("no message")
                    .to_string(),
            }),
        }
    }

    fn host_header(&self) -> String {
        let default = if self.url.https { 443 } else { 80 };
        let host = if self.url.host.contains(':') {
            format!("[{}]", self.url.host)
        } else {
            self.url.host.clone()
        };
        if self.url.port == default {
            host
        } else {
            format!("{host}:{}", self.url.port)
        }
    }

    async fn exchange(
        &self,
        shown: &str,
        req: hyper::Request<Full<Bytes>>,
    ) -> Result<(u16, Bytes), DoorError> {
        let tcp = tokio::net::TcpStream::connect((self.url.host.as_str(), self.url.port))
            .await
            .map_err(|cause| DoorError::Unreachable {
                url: shown.to_string(),
                cause,
            })?;
        let protocol = |cause| DoorError::Protocol {
            url: shown.to_string(),
            cause,
        };
        match &self.tls {
            None => drive(TokioIo::new(tcp), req).await.map_err(protocol),
            Some(cfg) => {
                let name = rustls::pki_types::ServerName::try_from(self.url.host.clone()).map_err(
                    |e| DoorError::Tls {
                        url: shown.to_string(),
                        cause: Box::new(e),
                    },
                )?;
                let tls = tokio_rustls::TlsConnector::from(cfg.clone())
                    .connect(name, tcp)
                    .await
                    .map_err(|e| DoorError::Tls {
                        url: shown.to_string(),
                        cause: Box::new(e),
                    })?;
                drive(TokioIo::new(tls), req).await.map_err(protocol)
            }
        }
    }
}

async fn drive<T>(io: T, req: hyper::Request<Full<Bytes>>) -> Result<(u16, Bytes), hyper::Error>
where
    T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    let driver = tokio::spawn(async move {
        let _ = conn.await;
    });
    let res = sender.send_request(req).await?;
    let status = res.status().as_u16();
    // An answer past the cap is cut off and read as no JSON at all.
    let body = match Limited::new(res.into_body(), MAX_ANSWER_BYTES)
        .collect()
        .await
    {
        Ok(b) => b.to_bytes(),
        Err(_) => Bytes::new(),
    };
    driver.abort();
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_is_taken_only_to_loopback_unless_opted_in() {
        let u = DoorUrl::parse("http://127.0.0.1:8080", false).unwrap();
        assert_eq!(
            (u.https, u.host.as_str(), u.port),
            (false, "127.0.0.1", 8080)
        );
        assert!(DoorUrl::parse("http://localhost:8080/", false).is_ok());
        assert!(DoorUrl::parse("http://[::1]:8080", false).is_ok());
        let e = DoorUrl::parse("http://ordo-host:8080", false).unwrap_err();
        assert_eq!(e.setting_name(), Some(DOOR_URL_ENV));
        assert!(e.to_string().contains(DOOR_PLAIN_HTTP_ENV), "{e}");
        assert!(DoorUrl::parse("http://ordo-host:8080", true).is_ok());
        let s = DoorUrl::parse("https://app.example.com/door", false).unwrap();
        assert_eq!((s.port, s.prefix.as_str()), (443, "/door"));
        for bad in [
            "ftp://x",
            "https://",
            "https://u@h",
            "https://h:port",
            "https://h/?q",
        ] {
            assert!(DoorUrl::parse(bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode("a b&c=d"), "a%20b%26c%3Dd");
    }
}
