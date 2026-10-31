//! A minimal HTTP/1.1 client, hand-rolled, for the one caller that needs
//! one.
//!
//! # Why hand-rolled
//!
//! The house precedent is explicit: `ordo-identity`'s token MAC and
//! `ordo-commandlog`'s chain hashing are both hand-rolled canonical
//! constructions over `sha2` rather than a framework's, because the bytes
//! are a *specification* and a dependency that changes them changes the
//! spec. The S3 REST surface this crate speaks is five verbs on one
//! object key. A full HTTP client would bring connection pooling, redirect
//! following, cookie jars, proxy discovery, decompression and a middleware
//! stack — every one of which is a way for bytes to be transformed between
//! the sealer and the bucket, which is the one thing that must not happen.
//!
//! What is *not* hand-rolled is TLS. That line is drawn deliberately and
//! in the same place `ordo-crypto` draws it: primitives come from
//! RustCrypto, constructions are ours. A hand-rolled TLS is not a
//! construction, it is a second cryptographic library.
//!
//! # What it does not do, stated rather than discovered
//!
//! - **No connection reuse.** One connection per request, `Connection:
//!   close`. An upload is one request and a serve is one request, so the
//!   handshake is amortised over an object rather than over a header.
//!   Pooling is an optimisation with a correctness surface (a poisoned
//!   idle connection serving the next tenant's range read) and it is not
//!   bought until something measures the want.
//! - **No redirects.** An S3 endpoint that redirects is misconfigured —
//!   most often a region mismatch — and following it would send a
//!   signature scoped to one region to a host in another, which fails
//!   confusingly instead of at the point of the mistake.
//! - **No HTTP/2.** S3 speaks 1.1.
//! - **No decompression, ever.** `Accept-Encoding` is never sent. The
//!   bytes that come back must be the bytes that went in.

use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{BlobError, ByteStream};

/// Read/write half of a connection, TLS or not.
type Conn = Pin<Box<dyn Duplex + Send>>;

pub trait Duplex: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> Duplex for T {}

/// Where to dial. Parsed once from configuration so a malformed endpoint
/// is a boot refusal rather than a first-upload surprise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

impl Endpoint {
    /// Parse `https://host[:port]` or `http://host[:port]`.
    ///
    /// A bare host with no scheme is refused rather than defaulted to
    /// `https`. Defaulting looks friendly and is not: the one time it
    /// matters is a configuration that meant `http` for a local gateway,
    /// and silently upgrading it produces a TLS handshake failure against
    /// a plaintext port, which is the least legible error in the set.
    pub fn parse(url: &str) -> Result<Endpoint, BlobError> {
        let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(BlobError::Config(format!(
                "endpoint {url:?} must begin with http:// or https://"
            )));
        };
        let rest = rest.trim_end_matches('/');
        if rest.contains('/') {
            return Err(BlobError::Config(format!(
                "endpoint {url:?} must be a host and optional port, with no path — \
                 the bucket and the key are the store's, not the endpoint's"
            )));
        }
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>().map_err(|_| {
                    BlobError::Config(format!("endpoint {url:?} has no valid port"))
                })?,
            ),
            None => (rest.to_string(), if tls { 443 } else { 80 }),
        };
        if host.is_empty() {
            return Err(BlobError::Config(format!("endpoint {url:?} names no host")));
        }
        Ok(Endpoint { host, port, tls })
    }

    pub fn authority(&self) -> String {
        let default = if self.tls { 443 } else { 80 };
        if self.port == default {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

pub struct Request {
    pub method: &'static str,
    /// Already percent-encoded, beginning with `/`.
    pub path: String,
    /// `(name, value)`, lowercase names. `host` and `content-length` are
    /// added by [`send`]; everything else is the caller's, because the
    /// caller is the one that signed them.
    pub headers: Vec<(String, String)>,
    pub body: Option<(u64, ByteStream)>,
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: ByteStream,
}

impl Response {
    /// The store's `Retry-After` when it is a number of seconds. A date is
    /// ignored: the binding's own backoff applies.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        self.header("retry-after")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(std::time::Duration::from_secs)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn tls_config() -> Arc<tokio_rustls::rustls::ClientConfig> {
    use std::sync::OnceLock;
    static CFG: OnceLock<Arc<tokio_rustls::rustls::ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(
            tokio_rustls::rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    })
    .clone()
}

async fn connect(e: &Endpoint) -> Result<Conn, BlobError> {
    let tcp = tokio::net::TcpStream::connect((e.host.as_str(), e.port))
        .await
        .map_err(|err| BlobError::network(format!("connect {}", e.authority()), err))?;
    tcp.set_nodelay(true).ok();
    if !e.tls {
        return Ok(Box::pin(tcp));
    }
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(e.host.clone())
        .map_err(|_| BlobError::Config(format!("{:?} is not a TLS server name", e.host)))?;
    let stream = tokio_rustls::TlsConnector::from(tls_config())
        .connect(name, tcp)
        .await
        .map_err(|err| BlobError::network(format!("tls {}", e.host), err))?;
    Ok(Box::pin(stream))
}

/// Send one request and return the response with its body still on the
/// wire.
///
/// The body is a stream over the same connection, which is why there is no
/// pooling: the connection is owned by the response and closes when the
/// stream is dropped.
pub async fn send(endpoint: &Endpoint, req: Request) -> Result<Response, BlobError> {
    use futures::StreamExt;

    let mut conn = connect(endpoint).await?;

    let mut head = format!("{} {} HTTP/1.1\r\n", req.method, req.path);
    head.push_str(&format!("host: {}\r\n", endpoint.authority()));
    head.push_str("connection: close\r\n");
    for (k, v) in &req.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some((len, _)) = &req.body {
        head.push_str(&format!("content-length: {len}\r\n"));
    } else if req.method == "PUT" || req.method == "POST" {
        head.push_str("content-length: 0\r\n");
    }
    head.push_str("\r\n");
    conn.write_all(head.as_bytes())
        .await
        .map_err(|e| BlobError::network("write head", e))?;

    if let Some((len, mut body)) = req.body {
        let mut sent = 0u64;
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            sent += chunk.len() as u64;
            if sent > len {
                return Err(BlobError::Refused(
                    "the body carried more bytes than it declared".into(),
                ));
            }
            conn.write_all(&chunk)
                .await
                .map_err(|e| BlobError::network("write body", e))?;
        }
        if sent != len {
            // Abandoning the connection is the right move: the request is
            // already on the wire with a `content-length` this body did
            // not fill, so there is no way to withdraw it and no way for
            // the peer to know it was short except by timing out.
            return Err(BlobError::Refused(format!(
                "the body declared {len} bytes and carried {sent}"
            )));
        }
    }
    conn.flush()
        .await
        .map_err(|e| BlobError::network("flush", e))?;

    read_response(conn).await
}

async fn read_response(mut conn: Conn) -> Result<Response, BlobError> {
    // Read until the header terminator. Bounded: a store that sends more
    // than 64 KiB of headers is not a store this client talks to.
    let mut buf = Vec::with_capacity(4096);
    let mut split = None;
    while split.is_none() {
        let mut b = [0u8; 4096];
        let n = conn
            .read(&mut b)
            .await
            .map_err(|e| BlobError::network("read head", e))?;
        if n == 0 {
            return Err(BlobError::Transport("closed before a response head".into()));
        }
        buf.extend_from_slice(&b[..n]);
        if buf.len() > 64 * 1024 {
            return Err(BlobError::Transport("response head is too large".into()));
        }
        split = find(&buf, b"\r\n\r\n");
    }
    let at = split.expect("loop exits only when found");
    let head = String::from_utf8_lossy(&buf[..at]).into_owned();
    let leftover = buf[at + 4..].to_vec();

    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| BlobError::Transport(format!("unparsable status line {status_line:?}")))?;
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }

    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let content_length: Option<u64> = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok());

    let body: ByteStream = if chunked {
        chunked_body(conn, leftover)
    } else {
        length_body(conn, leftover, content_length)
    };

    Ok(Response {
        status,
        headers,
        body,
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A `content-length` body, or — when the header is absent — everything
/// up to the close. `connection: close` is always sent, so read-to-EOF is
/// well defined here in a way it is not for a pooled client.
fn length_body(conn: Conn, leftover: Vec<u8>, len: Option<u64>) -> ByteStream {
    struct S {
        conn: Conn,
        pending: Vec<u8>,
        left: Option<u64>,
        done: bool,
    }
    let s = S {
        conn,
        pending: leftover,
        left: len,
        done: false,
    };
    Box::pin(futures::stream::unfold(s, |mut s| async move {
        if s.done || s.left == Some(0) {
            return None;
        }
        if !s.pending.is_empty() {
            let mut out = std::mem::take(&mut s.pending);
            if let Some(l) = s.left.as_mut() {
                if out.len() as u64 > *l {
                    out.truncate(*l as usize);
                }
                *l -= out.len() as u64;
            }
            return Some((Ok(out), s));
        }
        let want = s
            .left
            .map(|l| l.min(64 * 1024) as usize)
            .unwrap_or(64 * 1024);
        let mut b = vec![0u8; want];
        match s.conn.read(&mut b).await {
            Err(e) => {
                s.done = true;
                Some((Err(BlobError::network("read body", e)), s))
            }
            Ok(0) => {
                s.done = true;
                match s.left {
                    // A short body under a declared length is a truncated
                    // object. Ending the stream cleanly would hand the
                    // caller a prefix of a document.
                    Some(l) if l > 0 => Some((
                        Err(BlobError::Transport(format!(
                            "the response ended {l} bytes short of its content-length"
                        ))),
                        s,
                    )),
                    _ => None,
                }
            }
            Ok(n) => {
                b.truncate(n);
                if let Some(l) = s.left.as_mut() {
                    *l -= n as u64;
                }
                Some((Ok(b), s))
            }
        }
    }))
}

/// `transfer-encoding: chunked`. MinIO and some gateways use it for error
/// documents even when S3 itself would not.
fn chunked_body(conn: Conn, leftover: Vec<u8>) -> ByteStream {
    struct S {
        conn: Conn,
        buf: Vec<u8>,
        done: bool,
    }
    let s = S {
        conn,
        buf: leftover,
        done: false,
    };
    Box::pin(futures::stream::unfold(s, |mut s| async move {
        loop {
            if s.done {
                return None;
            }
            if let Some(nl) = find(&s.buf, b"\r\n") {
                let size_line = String::from_utf8_lossy(&s.buf[..nl]).into_owned();
                let size_hex = size_line.split(';').next().unwrap_or("").trim();
                let Ok(size) = usize::from_str_radix(size_hex, 16) else {
                    s.done = true;
                    return Some((
                        Err(BlobError::Transport(format!(
                            "unparsable chunk size {size_hex:?}"
                        ))),
                        s,
                    ));
                };
                // The chunk, plus its trailing CRLF.
                if s.buf.len() >= nl + 2 + size + 2 {
                    let chunk = s.buf[nl + 2..nl + 2 + size].to_vec();
                    s.buf.drain(..nl + 2 + size + 2);
                    if size == 0 {
                        return None;
                    }
                    return Some((Ok(chunk), s));
                }
            }
            let mut b = [0u8; 64 * 1024];
            match s.conn.read(&mut b).await {
                Err(e) => {
                    s.done = true;
                    return Some((Err(BlobError::network("read chunk", e)), s));
                }
                Ok(0) => {
                    return Some((
                        Err(BlobError::Transport("closed mid-chunk".into())),
                        S { done: true, ..s },
                    ));
                }
                Ok(n) => s.buf.extend_from_slice(&b[..n]),
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_parses_or_refuses_by_name() {
        assert_eq!(
            Endpoint::parse("https://s3.eu-west-1.amazonaws.com").unwrap(),
            Endpoint {
                host: "s3.eu-west-1.amazonaws.com".into(),
                port: 443,
                tls: true
            }
        );
        assert_eq!(
            Endpoint::parse("http://127.0.0.1:9000").unwrap(),
            Endpoint {
                host: "127.0.0.1".into(),
                port: 9000,
                tls: false
            }
        );
        // No scheme is a refusal, never an https default.
        assert!(Endpoint::parse("s3.amazonaws.com").is_err());
        assert!(Endpoint::parse("https://host/bucket").is_err());
        assert!(Endpoint::parse("https://").is_err());
        assert!(Endpoint::parse("https://host:not-a-port").is_err());
    }

    #[test]
    fn the_authority_omits_a_default_port_and_keeps_a_custom_one() {
        assert_eq!(Endpoint::parse("https://h").unwrap().authority(), "h");
        assert_eq!(Endpoint::parse("http://h").unwrap().authority(), "h");
        assert_eq!(
            Endpoint::parse("http://h:9000").unwrap().authority(),
            "h:9000"
        );
    }
}
