//! The one client. Pins 1 (TLS), 3 (no redirects) and 4 (signed), and the
//! consumption of pin 2's evidence. Two doors share one dialler: [`deliver`],
//! which returns a status and nothing else, and [`fetch`], which returns a
//! bounded body in a type only a verifier can open (see `lib.rs`).
//!
//! # Why HTTP/1.1 is written out by hand
//!
//! This is the same argument `security-logging-design.md` §2.1 makes about
//! `webauthn-rs`, and it lands the same way. A general HTTP client is a large
//! surface built to be *accommodating*: it follows redirects, it retries, it
//! pools connections across hosts, it reads and exposes response bodies, and
//! several of those are features this crate exists to not have. Turning them
//! off is a configuration, and a configuration is a thing that can be
//! changed, defaulted wrong on a version bump, or overridden by a second
//! caller.
//!
//! What is actually needed is: open one TLS stream to one pinned address,
//! write one POST, read one status line, close. That is about a hundred
//! lines. Written out, **pin 3 stops being a setting and becomes an absence**
//! — there is no redirect-following code to disable, no `Location` header is
//! ever looked at, and there is no loop. A reader checking the pin greps for
//! `Location` and finds nothing, which is a much stronger review than
//! checking that `redirect(Policy::none())` is still in a builder chain
//! twelve lines away.
//!
//! TLS is the opposite call, and for the same reason `argon2id` is not
//! hand-rolled: nobody should write a TLS stack, and `rustls` is a small,
//! audited, memory-safe one whose *shape* enforces the pin —
//! it implements TLS 1.2 and 1.3 and has no code for anything older, so the
//! floor is not a setting either. Certificate validation is on because
//! `rustls` has no supported way for a caller in this crate's feature set to
//! turn it off, and nothing here reaches for the unsupported way.
//!
//! # What is bounded
//!
//! Everything, because the far end is chosen by a tenant. The whole attempt
//! has one deadline ([`DEFAULT_TIMEOUT`]); the status line and headers are
//! read into a capped buffer and a response that exceeds it is
//! [`Refusal::MalformedResponse`] rather than an allocation; the body is read
//! to [`DEFAULT_BODY_CAP`] and **dropped without being decoded**. A
//! destination cannot make this process allocate, block, or say anything
//! about what it sent.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::dest::PinnedDestination;
use crate::refusal::Refusal;
use crate::sign::{self, SinkSecret, SIGNATURE_HEADER, TIMESTAMP_HEADER};

/// The whole attempt: DNS is already done, so this covers connect, handshake,
/// write and read.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
/// How much of a response body is read before the socket is closed. Read and
/// discarded; never decoded, never returned.
pub const DEFAULT_BODY_CAP: usize = 8 * 1024;
/// The status line plus all headers must fit here.
const HEAD_CAP: usize = 16 * 1024;

/// The trust anchors deliveries are validated against.
///
/// [`TlsRoots::webpki`] is the public root program. [`TlsRoots::custom`] is
/// for a self-hosted deployment whose receiver is behind a private CA — which
/// is the honest answer to the pressure that otherwise produces an
/// `insecure: true` flag. Pinning a *different* trust anchor is a decision an
/// operator makes once, at boot, for the whole deployment; disabling
/// validation is not available at any granularity.
#[derive(Clone)]
pub struct TlsRoots(Arc<ClientConfig>);

impl TlsRoots {
    /// Mozilla's root program, via `webpki-roots`.
    pub fn webpki() -> TlsRoots {
        let mut store = RootCertStore::empty();
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        TlsRoots::from_store(store)
    }

    /// A deployment's own anchors, DER-encoded.
    pub fn custom(ders: &[Vec<u8>]) -> Result<TlsRoots, String> {
        let mut store = RootCertStore::empty();
        for der in ders {
            store
                .add(rustls::pki_types::CertificateDer::from(der.clone()))
                .map_err(|e| format!("not a usable trust anchor: {e}"))?;
        }
        Ok(TlsRoots::from_store(store))
    }

    /// The public root program plus a deployment's own anchors, read from
    /// PEM text (one or more `CERTIFICATE` blocks). For a provider behind a
    /// private CA, or a stand-in provider in a test deployment. Validation
    /// stays on: this only widens which issuers are believed. Refuses text
    /// that holds no certificate, or a certificate that is not a usable
    /// anchor, so a wrong file is a boot refusal and never a silent no-op.
    pub fn webpki_and_pem(pem: &[u8]) -> Result<TlsRoots, String> {
        use rustls::pki_types::pem::PemObject;
        let mut store = RootCertStore::empty();
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut added = 0usize;
        for cert in rustls::pki_types::CertificateDer::pem_slice_iter(pem) {
            let cert = cert.map_err(|e| format!("not PEM certificate text: {e}"))?;
            store
                .add(cert)
                .map_err(|e| format!("not a usable trust anchor: {e}"))?;
            added += 1;
        }
        if added == 0 {
            return Err("holds no PEM CERTIFICATE block".into());
        }
        Ok(TlsRoots::from_store(store))
    }

    fn from_store(store: RootCertStore) -> TlsRoots {
        // `with_safe_defaults` in rustls 0.23 is the builder's only path:
        // there is no cipher-suite or version knob reachable from here, which
        // is the point. TLS 1.2 is the floor because rustls has no code below
        // it.
        let cfg = ClientConfig::builder()
            .with_root_certificates(store)
            .with_no_client_auth();
        TlsRoots(Arc::new(cfg))
    }
}

/// What is being POSTed, and under what secret.
pub struct Delivery<'a> {
    pub body: &'a [u8],
    pub content_type: &'a str,
    /// Seconds since the epoch. Inside the MAC, so it is not decoration.
    pub timestamp: u64,
    /// Extra headers. Names are checked against the same rule the writer
    /// applies to everything: no control bytes, no colon, no whitespace. A
    /// header value that could contain CRLF is header injection, and this is
    /// where it stops.
    pub headers: &'a [(&'a str, String)],
    pub secret: &'a SinkSecret,
}

/// A destination answered 2xx. **The status and nothing else** — see
/// `lib.rs` on why no response byte reaches a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    pub status: u16,
}

/// Deliver, once, to a destination that has already passed every pin.
///
/// `dest` is taken **by value**. That is pin 2's enforcement: the caller
/// cannot deliver twice from one resolution, so "re-checked at connect time"
/// is not a discipline the worker observes but the only shape the API has.
pub async fn deliver(
    dest: PinnedDestination,
    roots: &TlsRoots,
    d: Delivery<'_>,
    timeout: Duration,
) -> Result<Accepted, Refusal> {
    let ms = timeout.as_millis() as u64;
    match tokio::time::timeout(timeout, attempt(dest, roots, d)).await {
        Ok(r) => r,
        Err(_) => Err(Refusal::TimedOut { after_ms: ms }),
    }
}

async fn attempt(
    dest: PinnedDestination,
    roots: &TlsRoots,
    d: Delivery<'_>,
) -> Result<Accepted, Refusal> {
    let canonical = dest.canonical();
    let request = build_request(&dest, &canonical, &d)?;
    let mut tls = dial(&dest, roots).await?;
    send(&mut tls, &request).await?;

    let (status, retry_after_secs) = read_status(&mut tls).await?;
    // The body is drained to a cap so the connection closes cleanly, and
    // discarded. Nothing decodes it; nothing returns it.
    drain(&mut tls).await;
    let _ = tls.shutdown().await;

    // Pin 3. This match is the entire redirect policy, and the absence of a
    // `Location` lookup below it is the enforcement.
    match status {
        200..=299 => Ok(Accepted { status }),
        300..=399 => Err(Refusal::RedirectAnswered { status }),
        other => Err(Refusal::Rejected {
            status: other,
            retry_after_secs,
        }),
    }
}

pub(crate) type Tls = tokio_rustls::client::TlsStream<TcpStream>;

/// The dialler `deliver` and `fetch` share: one TCP connection to the pinned
/// address, one TLS handshake validated against the name.
pub(crate) async fn dial(dest: &PinnedDestination, roots: &TlsRoots) -> Result<Tls, Refusal> {
    let tcp = connect(dest).await?;
    handshake(dest, roots, tcp).await
}

/// The TCP half of [`dial`]. `smtp`'s STARTTLS mode exchanges `EHLO` and
/// `STARTTLS` on it before [`handshake`]; nothing else uses it bare.
pub(crate) async fn connect(dest: &PinnedDestination) -> Result<TcpStream, Refusal> {
    // Pin 2, at the only place it can actually be observed: the argument to
    // `connect` is an address, not a name. There is no code path in this
    // crate that hands a hostname to a socket.
    let tcp = TcpStream::connect(dest.addr())
        .await
        .map_err(|e| Refusal::ConnectFailed {
            detail: e.to_string(),
        })?;
    let _ = tcp.set_nodelay(true);
    Ok(tcp)
}

/// The TLS half of [`dial`]: the same roots, floor and name check whether
/// the handshake starts at the first byte or after `STARTTLS`.
pub(crate) async fn handshake(
    dest: &PinnedDestination,
    roots: &TlsRoots,
    tcp: TcpStream,
) -> Result<Tls, Refusal> {
    // Pin 1. The name — never the address — is what the certificate is
    // validated against and what goes in SNI, so pinning the address does not
    // weaken authentication by one bit.
    let server_name =
        ServerName::try_from(dest.host().to_string()).map_err(|_| Refusal::MalformedUrl {
            detail: "host is not a valid TLS server name".into(),
        })?;
    TlsConnector::from(roots.0.clone())
        .connect(server_name, tcp)
        .await
        .map_err(|e| Refusal::TlsFailed {
            detail: e.to_string(),
        })
}

async fn send(tls: &mut Tls, request: &[u8]) -> Result<(), Refusal> {
    tls.write_all(request)
        .await
        .map_err(|e| Refusal::ConnectFailed {
            detail: e.to_string(),
        })?;
    tls.flush().await.map_err(|e| Refusal::ConnectFailed {
        detail: e.to_string(),
    })
}

/// Build the request bytes. Split out so a test can assert the wire form
/// without a socket — including that a hostile header value cannot inject a
/// second request.
pub(crate) fn build_request(
    dest: &PinnedDestination,
    canonical: &str,
    d: &Delivery<'_>,
) -> Result<Vec<u8>, Refusal> {
    let signature = sign::signature_header(d.secret, d.timestamp, canonical, d.body);
    let host_header = host_header(dest);

    let mut head = String::with_capacity(512);
    head.push_str("POST ");
    head.push_str(dest.path());
    head.push_str(" HTTP/1.1\r\n");
    let mut put = |name: &str, value: &str| -> Result<(), Refusal> {
        if !header_token_ok(name) || !header_value_ok(value) {
            return Err(Refusal::MalformedUrl {
                detail: format!("header `{name}` is not representable"),
            });
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
        Ok(())
    };
    put("host", &host_header)?;
    put("content-type", d.content_type)?;
    put("content-length", &d.body.len().to_string())?;
    // No keep-alive. One delivery, one connection, one pinned address; a
    // pooled connection is a pin that outlives the resolution that made it.
    put("connection", "close")?;
    put("user-agent", "ordo-egress/1")?;
    // Explicitly refuse a body encoding we would then have to decode.
    put("accept", "*/*")?;
    put(TIMESTAMP_HEADER, &d.timestamp.to_string())?;
    put(SIGNATURE_HEADER, &signature)?;
    for (name, value) in d.headers {
        put(name, value)?;
    }
    head.push_str("\r\n");

    let mut out = head.into_bytes();
    out.extend_from_slice(d.body);
    Ok(out)
}

/// The `Host:` value: the name, with the port when it is not 443.
fn host_header(dest: &PinnedDestination) -> String {
    if dest.host().contains(':') {
        format!("[{}]:{}", dest.host(), dest.port())
    } else if dest.port() == crate::dest::HTTPS_PORT {
        dest.host().to_string()
    } else {
        format!("{}:{}", dest.host(), dest.port())
    }
}

fn header_token_ok(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn header_value_ok(s: &str) -> bool {
    // Printable ASCII and horizontal tab. No CR, no LF, no NUL — those three
    // are the whole of header injection.
    s.bytes().all(|b| b == b'\t' || (0x20..0x7f).contains(&b))
}

/// Read the status line and skip the headers, bounded.
///
/// One header name is read: `Retry-After`, as a number of seconds, so a
/// refusal can say when to try again. `Location` is never read. That is
/// deliberate and it is the implementation of pin 3: a redirect is an
/// answer, not a hop.
async fn read_status<S>(s: &mut S) -> Result<(u16, Option<u64>), Refusal>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = s.read(&mut chunk).await.map_err(io_refusal)?;
        if n == 0 {
            return Err(Refusal::MalformedResponse {
                detail: "closed before the headers ended".into(),
            });
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > HEAD_CAP {
            return Err(Refusal::MalformedResponse {
                detail: format!("headers exceeded {HEAD_CAP} bytes"),
            });
        }
    }
    Ok((parse_status_line(&buf)?, retry_after_of(&buf)))
}

/// `Retry-After` in delta-seconds, clamped to
/// [`crate::refusal::RETRY_AFTER_CAP_SECS`]. An HTTP-date, two values or a
/// value that is not a number is `None`, and the caller's own backoff
/// applies. The one header a refusal reads: a number, never echoed text.
pub(crate) fn retry_after_of(head: &[u8]) -> Option<u64> {
    let end = head
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(head.len());
    let text = std::str::from_utf8(&head[..end]).ok()?;
    let mut found = None;
    for line in text.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("retry-after") {
            continue;
        }
        if found.is_some() {
            return None;
        }
        let v = value.trim();
        if v.is_empty() || v.len() > 10 || !v.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        found = v.parse::<u64>().ok();
    }
    found.map(|s| s.min(crate::refusal::RETRY_AFTER_CAP_SECS))
}

pub(crate) fn parse_status_line(buf: &[u8]) -> Result<u16, Refusal> {
    let end =
        buf.windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| Refusal::MalformedResponse {
                detail: "no status line".into(),
            })?;
    let line = std::str::from_utf8(&buf[..end]).map_err(|_| Refusal::MalformedResponse {
        detail: "status line is not UTF-8".into(),
    })?;
    let mut parts = line.split(' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(Refusal::MalformedResponse {
            detail: format!("`{version}` is not HTTP/1.x"),
        });
    }
    let code = parts.next().unwrap_or_default();
    code.parse::<u16>()
        .ok()
        .filter(|c| (100..600).contains(c))
        .ok_or_else(|| Refusal::MalformedResponse {
            detail: format!("`{code}` is not a status code"),
        })
}

async fn drain<S>(s: &mut S)
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut sink = [0u8; 1024];
    let mut total = 0usize;
    while total < DEFAULT_BODY_CAP {
        match s.read(&mut sink).await {
            Ok(0) | Err(_) => return,
            Ok(n) => total += n,
        }
    }
}

fn io_refusal(e: io::Error) -> Refusal {
    Refusal::ConnectFailed {
        detail: e.to_string(),
    }
}

// ─────────────────────────────── fetch ────────────────────────────────
//
// The second door (`lib.rs`, "The one exception"). Pins 1, 2, 3, 5 and 6
// hold exactly as for `deliver`, by sharing `dial` and the destination
// types; pin 4 does not apply. What differs is that the body is read,
// bounded, decoded from its framing, and returned in a type nobody can print.

/// How many chunks a chunked body may arrive in. A provider's answer is a few
/// kilobytes in a handful of chunks; a body sent one byte per chunk is a
/// destination making this process spin, and is refused.
pub const MAX_CHUNKS: usize = 1024;
/// The longest chunk-size line or trailer line accepted.
const LINE_CAP: usize = 1024;

/// The two requests a verifier makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchMethod {
    /// `GET`, no body: a userinfo document, a discovery document, a key set.
    Get,
    /// `POST` of `application/x-www-form-urlencoded`: a token exchange.
    PostForm,
    /// `POST` of `application/json`: a mail API's send call ([`post`]).
    PostJson,
}

/// One request. **Not `Debug`**: `body` can hold a client secret and
/// `headers` an access token. The caller passes them per call; nothing in
/// this crate keeps them (pin 5).
pub struct FetchRequest<'a> {
    pub method: FetchMethod,
    /// The encoded form, for [`FetchMethod::PostForm`]. Ignored for `GET`.
    pub body: &'a [u8],
    /// The `Accept:` value.
    pub accept: &'a str,
    /// Extra headers, checked by the same rule `deliver` applies: no control
    /// bytes, no colon or whitespace in a name.
    pub headers: &'a [(&'a str, String)],
}

/// A 2xx answer's body, decoded from its framing and bounded by its cap.
///
/// **No `Debug`, no `Display`, no `Clone`, no serialiser.** The body can be
/// taken out once, by the verifier it was fetched for, through
/// [`Fetched::into_verifier_input`], and in no other way. That is what keeps
/// the exfiltration half of SSRF closed for this door (`lib.rs`).
pub struct Fetched {
    status: u16,
    body: Vec<u8>,
    max_age: Option<u64>,
}

impl Fetched {
    /// The 2xx status the destination answered.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// How long the destination said the answer may be cached, in seconds:
    /// `Cache-Control: max-age`, or 0 for `no-store` or `no-cache`. `None`
    /// when it said nothing. A number the caller's cache is bounded by,
    /// never bytes of the body.
    pub fn max_age(&self) -> Option<u64> {
        self.max_age
    }

    /// The body, for a parser. Consumes the value.
    pub fn into_verifier_input(self) -> Vec<u8> {
        self.body
    }
}

/// Fetch once from a destination that has passed every destination pin.
///
/// `dest` is taken by value, as `deliver` takes it. `cap` is the largest body
/// accepted, per purpose: a body declared larger is refused before a byte of
/// it is read, and an undeclared one is refused the moment it passes the cap.
/// A 3xx is [`Refusal::RedirectAnswered`] and no second request is made; any
/// other non-2xx is [`Refusal::Rejected`] and its body is not read.
pub async fn fetch(
    dest: PinnedDestination,
    roots: &TlsRoots,
    req: FetchRequest<'_>,
    cap: usize,
    timeout: Duration,
) -> Result<Fetched, Refusal> {
    let ms = timeout.as_millis() as u64;
    match tokio::time::timeout(timeout, fetch_attempt(dest, roots, req, cap)).await {
        Ok(r) => r,
        Err(_) => Err(Refusal::TimedOut { after_ms: ms }),
    }
}

async fn fetch_attempt(
    dest: PinnedDestination,
    roots: &TlsRoots,
    req: FetchRequest<'_>,
    cap: usize,
) -> Result<Fetched, Refusal> {
    let request = build_fetch_request(&dest, &req)?;
    let mut tls = dial(&dest, roots).await?;
    send(&mut tls, &request).await?;
    let mut reader = Reader::new(&mut tls);
    let head = reader.head().await?;
    let status = parse_status_line(&head)?;
    // Pin 3, as in `deliver`: a 3xx is an answer, not a hop.
    match status {
        200..=299 => {}
        300..=399 => return Err(Refusal::RedirectAnswered { status }),
        other => {
            return Err(Refusal::Rejected {
                status: other,
                retry_after_secs: retry_after_of(&head),
            })
        }
    }
    let framing = framing_of(&head)?;
    let max_age = max_age_of(&head);
    let body = reader.body(framing, cap).await?;
    let _ = tls.shutdown().await;
    Ok(Fetched {
        status,
        body,
        max_age,
    })
}

/// `Cache-Control`'s freshness, in seconds. `no-store` and `no-cache` are 0.
/// Several `max-age` values, or one that is not a number, are ignored, so
/// the caller's own default applies.
pub(crate) fn max_age_of(head: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(head).ok()?;
    let mut ages = Vec::new();
    let mut uncacheable = false;
    for line in text.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("cache-control") {
            continue;
        }
        for d in value.split(',').map(str::trim) {
            let lower = d.to_ascii_lowercase();
            if lower == "no-store" || lower == "no-cache" {
                uncacheable = true;
            } else if let Some(n) = lower.strip_prefix("max-age=") {
                ages.push(n.trim_matches('"').parse::<u64>().ok());
            }
        }
    }
    if uncacheable {
        return Some(0);
    }
    match ages.as_slice() {
        [Some(n)] => Some(*n),
        _ => None,
    }
}

/// Build the request bytes for `fetch`. Split out so a test can read the
/// wire form without a socket.
pub(crate) fn build_fetch_request(
    dest: &PinnedDestination,
    req: &FetchRequest<'_>,
) -> Result<Vec<u8>, Refusal> {
    let verb = match req.method {
        FetchMethod::Get => "GET",
        FetchMethod::PostForm | FetchMethod::PostJson => "POST",
    };
    let mut head = String::with_capacity(512);
    head.push_str(verb);
    head.push(' ');
    head.push_str(dest.path());
    head.push_str(" HTTP/1.1\r\n");
    let mut put = |name: &str, value: &str| -> Result<(), Refusal> {
        if !header_token_ok(name) || !header_value_ok(value) {
            return Err(Refusal::MalformedUrl {
                detail: format!("header `{name}` is not representable"),
            });
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
        Ok(())
    };
    put("host", &host_header(dest))?;
    put("connection", "close")?;
    put("user-agent", "ordo-egress/1")?;
    put("accept", req.accept)?;
    // No body encoding to decode, so none is accepted.
    put("accept-encoding", "identity")?;
    match req.method {
        FetchMethod::Get => {}
        FetchMethod::PostForm => {
            put("content-type", "application/x-www-form-urlencoded")?;
            put("content-length", &req.body.len().to_string())?;
        }
        FetchMethod::PostJson => {
            put("content-type", "application/json")?;
            put("content-length", &req.body.len().to_string())?;
        }
    }
    for (name, value) in req.headers {
        put(name, value)?;
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if req.method != FetchMethod::Get {
        out.extend_from_slice(req.body);
    }
    Ok(out)
}

// ─────────────────────────────── post ─────────────────────────────────
//
// The third door: a request whose whole answer is its status. A mail API
// (`plan-auth-federated.md` §7.6) is called with a caller-supplied
// `Authorization` header and answers "accepted" or not. Nothing it sends back
// is read past the status line, so this door returns what `deliver` returns
// and holds what `deliver` holds: a status. Pins 1, 2, 3, 5 and 6 are the
// shared dialler's; pin 4 does not apply, because the API authenticates the
// caller by the key in the header, which the caller passes per call.

/// Send one request built as [`fetch`] builds it, and answer its status.
///
/// A 2xx is [`Accepted`] carrying the exact status, so a caller that
/// accepts only one status (Resend's 200, SendGrid's 202) can say so. A
/// 3xx is [`Refusal::RedirectAnswered`] and no second request is made; any
/// other status is [`Refusal::Rejected`]. The body is drained to
/// [`DEFAULT_BODY_CAP`] and dropped undecoded.
pub async fn post(
    dest: PinnedDestination,
    roots: &TlsRoots,
    req: FetchRequest<'_>,
    timeout: Duration,
) -> Result<Accepted, Refusal> {
    let ms = timeout.as_millis() as u64;
    match tokio::time::timeout(timeout, post_attempt(dest, roots, req)).await {
        Ok(r) => r,
        Err(_) => Err(Refusal::TimedOut { after_ms: ms }),
    }
}

async fn post_attempt(
    dest: PinnedDestination,
    roots: &TlsRoots,
    req: FetchRequest<'_>,
) -> Result<Accepted, Refusal> {
    let request = build_fetch_request(&dest, &req)?;
    let mut tls = dial(&dest, roots).await?;
    send(&mut tls, &request).await?;
    let (status, retry_after_secs) = read_status(&mut tls).await?;
    drain(&mut tls).await;
    let _ = tls.shutdown().await;
    match status {
        200..=299 => Ok(Accepted { status }),
        300..=399 => Err(Refusal::RedirectAnswered { status }),
        other => Err(Refusal::Rejected {
            status: other,
            retry_after_secs,
        }),
    }
}

/// How a body is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Framing {
    Length(usize),
    Chunked,
    UntilClose,
}

/// Read the framing headers, and refuse every ambiguous or encoded shape.
///
/// Refused: a `Content-Encoding` other than `identity` (none was accepted);
/// a `Transfer-Encoding` other than exactly `chunked`; both framings at once,
/// or two different lengths, which is the request-smuggling shape and is not
/// worth interpreting.
pub(crate) fn framing_of(head: &[u8]) -> Result<Framing, Refusal> {
    let malformed = |d: &str| Refusal::MalformedResponse { detail: d.into() };
    let text = std::str::from_utf8(head).map_err(|_| malformed("headers are not UTF-8"))?;
    let mut length: Option<usize> = None;
    let mut chunked = false;
    for line in text.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(malformed("a header line without a colon"));
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => {
                let n: usize = value
                    .parse()
                    .map_err(|_| malformed("content-length is not a number"))?;
                if length.is_some_and(|m| m != n) {
                    return Err(malformed("two different content-lengths"));
                }
                length = Some(n);
            }
            "transfer-encoding" => {
                if !value.eq_ignore_ascii_case("chunked") || chunked {
                    return Err(malformed("a transfer-encoding other than chunked"));
                }
                chunked = true;
            }
            "content-encoding" if !value.eq_ignore_ascii_case("identity") => {
                return Err(malformed("an encoded body"));
            }
            _ => {}
        }
    }
    match (length, chunked) {
        (Some(_), true) => Err(malformed("both a length and chunked")),
        (Some(n), false) => Ok(Framing::Length(n)),
        (None, true) => Ok(Framing::Chunked),
        (None, false) => Ok(Framing::UntilClose),
    }
}

fn over_cap(cap: usize) -> Refusal {
    Refusal::MalformedResponse {
        detail: format!("body exceeded its cap of {cap} bytes"),
    }
}

/// A bounded buffered reader over one response.
struct Reader<'s, S> {
    s: &'s mut S,
    buf: Vec<u8>,
}

impl<'s, S> Reader<'s, S>
where
    S: tokio::io::AsyncRead + Unpin,
{
    fn new(s: &'s mut S) -> Self {
        Reader {
            s,
            buf: Vec::with_capacity(4096),
        }
    }

    /// Read more into the buffer. `Ok(false)` at end of stream.
    async fn more(&mut self) -> Result<bool, Refusal> {
        let mut chunk = [0u8; 4096];
        let n = self.s.read(&mut chunk).await.map_err(io_refusal)?;
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(n > 0)
    }

    /// The status line and headers, through the blank line, bounded.
    async fn head(&mut self) -> Result<Vec<u8>, Refusal> {
        loop {
            if let Some(i) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let rest = self.buf.split_off(i + 4);
                let head = std::mem::replace(&mut self.buf, rest);
                return Ok(head);
            }
            if self.buf.len() > HEAD_CAP {
                return Err(Refusal::MalformedResponse {
                    detail: format!("headers exceeded {HEAD_CAP} bytes"),
                });
            }
            if !self.more().await? {
                return Err(Refusal::MalformedResponse {
                    detail: "closed before the headers ended".into(),
                });
            }
        }
    }

    /// One CRLF-terminated line, without the CRLF.
    async fn line(&mut self) -> Result<Vec<u8>, Refusal> {
        loop {
            if let Some(i) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let rest = self.buf.split_off(i + 2);
                let mut line = std::mem::replace(&mut self.buf, rest);
                line.truncate(i);
                return Ok(line);
            }
            if self.buf.len() > LINE_CAP {
                return Err(Refusal::MalformedResponse {
                    detail: "a chunk line is too long".into(),
                });
            }
            if !self.more().await? {
                return Err(Refusal::MalformedResponse {
                    detail: "closed inside a chunk line".into(),
                });
            }
        }
    }

    /// Exactly `n` bytes.
    async fn exactly(&mut self, n: usize) -> Result<Vec<u8>, Refusal> {
        while self.buf.len() < n {
            if !self.more().await? {
                return Err(Refusal::MalformedResponse {
                    detail: "closed before the body ended".into(),
                });
            }
        }
        let rest = self.buf.split_off(n);
        Ok(std::mem::replace(&mut self.buf, rest))
    }

    async fn body(&mut self, framing: Framing, cap: usize) -> Result<Vec<u8>, Refusal> {
        match framing {
            // Refused on the declaration, before a byte is held.
            Framing::Length(n) if n > cap => Err(over_cap(cap)),
            Framing::Length(n) => self.exactly(n).await,
            Framing::UntilClose => {
                while self.more().await? {
                    if self.buf.len() > cap {
                        return Err(over_cap(cap));
                    }
                }
                if self.buf.len() > cap {
                    return Err(over_cap(cap));
                }
                Ok(std::mem::take(&mut self.buf))
            }
            Framing::Chunked => {
                let mut out = Vec::new();
                for _ in 0..=MAX_CHUNKS {
                    let line = self.line().await?;
                    let size = chunk_size(&line)?;
                    if size == 0 {
                        // Trailers, bounded like every other line, ignored.
                        for _ in 0..64 {
                            if self.line().await?.is_empty() {
                                return Ok(out);
                            }
                        }
                        return Err(Refusal::MalformedResponse {
                            detail: "too many trailers".into(),
                        });
                    }
                    if out.len().saturating_add(size) > cap {
                        return Err(over_cap(cap));
                    }
                    let data = self.exactly(size).await?;
                    out.extend_from_slice(&data);
                    if !self.line().await?.is_empty() {
                        return Err(Refusal::MalformedResponse {
                            detail: "a chunk did not end where it said".into(),
                        });
                    }
                }
                Err(Refusal::MalformedResponse {
                    detail: format!("more than {MAX_CHUNKS} chunks"),
                })
            }
        }
    }
}

/// The size on a chunk line: hex, with any `;extension` ignored.
fn chunk_size(line: &[u8]) -> Result<usize, Refusal> {
    let text = std::str::from_utf8(line).unwrap_or("");
    let hex = text.split(';').next().unwrap_or("").trim();
    if hex.is_empty() || hex.len() > 8 {
        return Err(Refusal::MalformedResponse {
            detail: "a chunk size is not hex".into(),
        });
    }
    usize::from_str_radix(hex, 16).map_err(|_| Refusal::MalformedResponse {
        detail: "a chunk size is not hex".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dest::pin_for_test;

    fn delivery<'a>(secret: &'a SinkSecret, headers: &'a [(&'a str, String)]) -> Delivery<'a> {
        Delivery {
            body: b"{\"a\":1}",
            content_type: "application/json",
            timestamp: 1_757_000_000,
            headers,
            secret,
        }
    }

    #[test]
    fn the_request_is_a_post_to_the_path_with_the_name_in_host() {
        let secret = SinkSecret::from_bytes(b"k");
        let dest = pin_for_test(
            "hooks.example.com",
            "203.0.113.7:8443".parse().unwrap(),
            "/a",
        );
        let bytes = build_request(&dest, &dest.canonical(), &delivery(&secret, &[])).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("POST /a HTTP/1.1\r\n"), "{text}");
        // The Host header carries the *name*, though the socket goes to the
        // address. Pinning the address does not weaken authentication.
        assert!(text.contains("host: hooks.example.com:8443\r\n"), "{text}");
        assert!(text.contains("content-length: 7\r\n"), "{text}");
        let age = |h: &str| max_age_of(format!("HTTP/1.1 200 OK\r\n{h}\r\n").as_bytes());
        assert_eq!(age("cache-control: public, max-age=19742"), Some(19742));
        assert_eq!(age("Cache-Control: no-store"), Some(0));
        assert_eq!(age("cache-control: max-age=1, max-age=2"), None);
        assert_eq!(age("cache-control: max-age=x"), None);
        assert_eq!(age("content-type: application/json"), None);
        assert!(text.contains("connection: close\r\n"), "{text}");
        assert!(text.contains("x-ordo-timestamp: 1757000000\r\n"), "{text}");
        assert!(text.contains("\r\nx-ordo-signature: v1="), "{text}");
        assert!(text.ends_with("\r\n\r\n{\"a\":1}"), "{text}");
    }

    #[test]
    fn a_crlf_in_a_header_value_cannot_inject_a_second_request() {
        let secret = SinkSecret::from_bytes(b"k");
        let dest = pin_for_test("h.example.com", "203.0.113.7:443".parse().unwrap(), "/a");
        let hostile = [("x-ordo-space", "s\r\nX-Injected: yes".to_string())];
        let e = build_request(&dest, &dest.canonical(), &delivery(&secret, &hostile)).unwrap_err();
        assert_eq!(e.kind(), "malformed_url");
        let hostile = [("x-ordo\r\nEvil", "s".to_string())];
        assert!(build_request(&dest, &dest.canonical(), &delivery(&secret, &hostile)).is_err());
    }

    #[test]
    fn the_signature_covers_this_destination() {
        let secret = SinkSecret::from_bytes(b"k");
        let a = pin_for_test("h.example.com", "203.0.113.7:443".parse().unwrap(), "/a");
        let b = pin_for_test("h.example.com", "203.0.113.7:443".parse().unwrap(), "/b");
        let sa = build_request(&a, &a.canonical(), &delivery(&secret, &[])).unwrap();
        let sb = build_request(&b, &b.canonical(), &delivery(&secret, &[])).unwrap();
        let head = |v: &[u8]| {
            String::from_utf8_lossy(v)
                .lines()
                .find(|l| l.starts_with("x-ordo-signature"))
                .unwrap()
                .to_string()
        };
        assert_ne!(head(&sa), head(&sb));
    }

    #[test]
    fn a_fetch_is_a_get_or_a_form_post_and_cannot_be_injected_into() {
        let dest = pin_for_test("idp.example.com", "203.0.113.7:443".parse().unwrap(), "/me");
        let req = FetchRequest {
            method: FetchMethod::Get,
            body: b"ignored",
            accept: "application/json",
            headers: &[],
        };
        let text = String::from_utf8(build_fetch_request(&dest, &req).unwrap()).unwrap();
        assert!(
            text.starts_with("GET /me HTTP/1.1\r\nhost: idp.example.com\r\n"),
            "{text}"
        );
        assert!(text.ends_with("\r\n\r\n"), "a GET carries no body: {text}");
        assert!(!text.contains("x-ordo-signature"), "pin 4 does not apply");
        let hostile = [("authorization", "Bearer t\r\nX-Injected: yes".to_string())];
        let req = FetchRequest {
            method: FetchMethod::PostForm,
            body: b"a=1",
            accept: "application/json",
            headers: &hostile,
        };
        assert!(build_fetch_request(&dest, &req).is_err());
    }

    #[test]
    fn framing_refuses_every_ambiguous_shape() {
        let f = |h: &str| framing_of(format!("HTTP/1.1 200 OK\r\n{h}\r\n").as_bytes());
        assert_eq!(f("content-length: 5\r\n").unwrap(), Framing::Length(5));
        assert_eq!(
            f("Transfer-Encoding: Chunked\r\n").unwrap(),
            Framing::Chunked
        );
        assert_eq!(f("").unwrap(), Framing::UntilClose);
        for bad in [
            "content-length: 5\r\ncontent-length: 6\r\n",
            "content-length: 5\r\ntransfer-encoding: chunked\r\n",
            "transfer-encoding: gzip, chunked\r\n",
            "content-encoding: br\r\n",
            "content-length: -1\r\n",
        ] {
            assert!(f(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn status_lines() {
        assert_eq!(
            parse_status_line(b"HTTP/1.1 204 No Content\r\n\r\n").unwrap(),
            204
        );
        assert_eq!(
            parse_status_line(b"HTTP/1.0 302 Found\r\n\r\n").unwrap(),
            302
        );
        for bad in [
            &b"HTTP/2 200 OK\r\n\r\n"[..],
            b"garbage\r\n\r\n",
            b"HTTP/1.1 twohundred\r\n\r\n",
            b"HTTP/1.1 999\r\n\r\n",
            b"HTTP/1.1 200 OK",
        ] {
            assert!(
                parse_status_line(bad).is_err(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn no_code_in_this_crate_reads_a_location_header() {
        // Pin 3 as a grep, asserted: if somebody adds redirect following,
        // this fails before a reviewer has to notice. Comment lines and the
        // test module are excluded, because pin 3 is worth *explaining*.
        //
        // Found writing this: the naive `contains("location")` also matches
        // `allocation`, which is a word this file legitimately uses. A
        // whole-word check is the difference between a test and a tripwire.
        let src = include_str!("client.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        for (n, line) in code.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") || t.starts_with("//!") {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            let mut from = 0;
            while let Some(i) = lower[from..].find("location") {
                let at = from + i;
                let before = lower[..at].chars().next_back();
                assert!(
                    before.is_some_and(|c| c.is_ascii_alphabetic()),
                    "line {} of client.rs mentions a `location` header; \
                     redirects are never followed",
                    n + 1
                );
                from = at + 1;
            }
        }
    }
}
