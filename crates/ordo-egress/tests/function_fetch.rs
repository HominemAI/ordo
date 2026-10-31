//! `FnEgress`, a backend function's `fetch`, against a real TLS listener.
//!
//! The listener is the one `fetch.rs` uses: a `rustls` server on loopback
//! with a leaf for `egress.test.invalid` signed by a test CA the client is
//! given as its only anchor. Names are answered by
//! `FnEgress::resolving_for_tests`, so the allowlist, the secret bindings and
//! the deny list all still run. What is proved here: a secret reaches only a
//! host it is bound to, every status comes back with its body, both bodies
//! are capped, a request has a time limit, and a redirect is never followed.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ordo_egress::{
    BodySide, DenyList, FetchError, FnEgress, FnFetch, FnFetchRequest, FnLimits, HeaderValue,
    SecretBytes, SecretSource, TlsRoots,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

const CA: &[u8] = include_bytes!("fixture/ca.der");
const LEAF: &[u8] = include_bytes!("fixture/leaf.der");
const LEAF_KEY: &[u8] = include_bytes!("fixture/leaf-key.der");
const NAME: &str = "egress.test.invalid";
/// Another allowed host, which the secret is not bound to.
const OTHER: &str = "other.test.invalid";
const VALUE: &str = "sk-test-0123456789";

fn acceptor() -> TlsAcceptor {
    let cert = rustls::pki_types::CertificateDer::from(LEAF.to_vec());
    let key = rustls::pki_types::PrivateKeyDer::try_from(LEAF_KEY.to_vec()).expect("pkcs8 key");
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config");
    TlsAcceptor::from(Arc::new(cfg))
}

/// A listener that answers `response` (or nothing, when `None`) to every
/// request, counts connections, and reports each request it read.
async fn listener(
    response: Option<Vec<u8>>,
) -> (SocketAddr, Arc<AtomicUsize>, mpsc::Receiver<String>) {
    let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = tcp.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel(8);
    let acceptor = acceptor();
    let seen = count.clone();
    let response = Arc::new(response);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = tcp.accept().await else {
                return;
            };
            seen.fetch_add(1, Ordering::SeqCst);
            let (acceptor, tx, response) = (acceptor.clone(), tx.clone(), response.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(sock).await else {
                    return;
                };
                if let Ok(req) = read_request(&mut tls).await {
                    let _ = tx.send(req).await;
                }
                match response.as_ref() {
                    Some(r) => {
                        let _ = tls.write_all(r).await;
                        let _ = tls.flush().await;
                        let _ = tls.shutdown().await;
                    }
                    None => tokio::time::sleep(Duration::from_secs(30)).await,
                }
            });
        }
    });
    (addr, count, rx)
}

async fn read_request<S>(s: &mut S) -> io::Result<String>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let end = loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::other("closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let want: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[end + 4..].to_vec();
    while body.len() < want {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Ok(format!("{head}\r\n\r\n{}", String::from_utf8_lossy(&body)))
}

/// One secret, `apiKey`, bound to [`NAME`] only.
struct Store;

#[async_trait]
impl SecretSource for Store {
    fn bound_hosts(&self, name: &str) -> Option<Vec<String>> {
        (name == "apiKey").then(|| vec![NAME.to_string()])
    }
    async fn open(&self, _space: &str, name: &str) -> Result<Option<SecretBytes>, FetchError> {
        Ok((name == "apiKey").then(|| SecretBytes::new(VALUE.as_bytes().to_vec())))
    }
}

fn egress(addr: SocketAddr, limits: FnLimits) -> FnEgress {
    FnEgress::new(
        Arc::new(Store),
        TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
        DenyList::categories_only().permitting_loopback_for_tests(),
    )
    .with_limits(limits)
    .resolving_for_tests(NAME, addr)
    .resolving_for_tests(OTHER, addr)
}

fn request(host: &str, addr: SocketAddr, with_secret: bool) -> FnFetchRequest {
    let mut headers = vec![(
        "content-type".to_string(),
        HeaderValue::Plain("application/json".into()),
    )];
    if with_secret {
        headers.push((
            "authorization".into(),
            HeaderValue::Secret {
                name: "apiKey".into(),
                template: "Bearer {}".into(),
            },
        ));
    }
    FnFetchRequest {
        method: "POST".into(),
        url: format!("https://{host}:{}/v1/classify", addr.port()),
        headers,
        body: br#"{"notes":["a"]}"#.to_vec(),
        egress: vec![NAME.into(), OTHER.into()],
        secrets: vec!["apiKey".into()],
    }
}

fn ok(body: &[u8]) -> Vec<u8> {
    let mut r = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    r.extend_from_slice(body);
    r
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_is_filled_in_for_its_bound_host_and_the_answer_comes_back() {
    let (addr, _, mut rx) = listener(Some(ok(br#"{"label":"work"}"#))).await;
    let got = egress(addr, FnLimits::default())
        .fetch("acme", request(NAME, addr, true))
        .await
        .expect("fetched");
    assert_eq!(got.status, 200);
    assert_eq!(got.body, br#"{"label":"work"}"#);
    assert!(got
        .headers
        .contains(&("content-type".into(), "application/json".into())));
    let req = rx.recv().await.expect("a request");
    assert!(req.starts_with("POST /v1/classify HTTP/1.1\r\n"), "{req}");
    assert!(
        req.contains(&format!("authorization: Bearer {VALUE}\r\n")),
        "{req}"
    );
    assert!(req.ends_with(r#"{"notes":["a"]}"#), "{req}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_for_another_allowed_host_is_refused_before_any_connection() {
    let (addr, count, _rx) = listener(Some(ok(b"{}"))).await;
    let e = egress(addr, FnLimits::default())
        .fetch("acme", request(OTHER, addr, true))
        .await
        .expect_err("refused");
    assert_eq!(
        e,
        FetchError::SecretNotBound {
            secret: "apiKey".into(),
            host: OTHER.into()
        }
    );
    let text = e.to_string();
    assert!(text.contains("apiKey") && text.contains(OTHER), "{text}");
    assert!(!text.contains(VALUE), "{text}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(count.load(Ordering::SeqCst), 0, "nothing was dialled");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_status_comes_back_with_its_body() {
    let answer = br#"{"error":"rate limited"}"#;
    let mut r = format!(
        "HTTP/1.1 429 Too Many Requests\r\nretry-after: 3\r\ncontent-length: {}\r\n\r\n",
        answer.len()
    )
    .into_bytes();
    r.extend_from_slice(answer);
    let (addr, _, _rx) = listener(Some(r)).await;
    let got = egress(addr, FnLimits::default())
        .fetch("acme", request(NAME, addr, false))
        .await
        .expect("an answer");
    assert_eq!(got.status, 429);
    assert_eq!(got.body, answer);
    assert!(got.headers.contains(&("retry-after".into(), "3".into())));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_is_returned_and_never_followed() {
    let (addr, count, _rx) = listener(Some(
        b"HTTP/1.1 302 Found\r\nlocation: https://169.254.169.254/latest\r\ncontent-length: 0\r\n\r\n"
            .to_vec(),
    ))
    .await;
    let got = egress(addr, FnLimits::default())
        .fetch("acme", request(NAME, addr, true))
        .await
        .expect("an answer");
    assert_eq!(got.status, 302);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(count.load(Ordering::SeqCst), 1, "one connection, no hop");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_echoed_secret_is_scrubbed_from_the_answer() {
    let echo = format!(r#"{{"seen":"Bearer {VALUE}"}}"#);
    let (addr, _, _rx) = listener(Some(ok(echo.as_bytes()))).await;
    let got = egress(addr, FnLimits::default())
        .fetch("acme", request(NAME, addr, true))
        .await
        .expect("fetched");
    let body = String::from_utf8(got.body).unwrap();
    assert_eq!(body, r#"{"seen":"Bearer [secret]"}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn both_bodies_are_capped() {
    let (addr, count, _rx) = listener(Some(ok(&[b'x'; 2048]))).await;
    let limits = FnLimits {
        request_body: 8,
        response_body: 1024,
        ..FnLimits::default()
    };
    let e = egress(addr, limits)
        .fetch("acme", request(NAME, addr, false))
        .await
        .expect_err("request too big");
    assert_eq!(
        e,
        FetchError::BodyTooLarge {
            limit: 8,
            side: BodySide::Request
        }
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);

    let limits = FnLimits {
        response_body: 1024,
        ..FnLimits::default()
    };
    let e = egress(addr, limits)
        .fetch("acme", request(NAME, addr, false))
        .await
        .expect_err("response too big");
    assert_eq!(
        e,
        FetchError::BodyTooLarge {
            limit: 1024,
            side: BodySide::Response
        }
    );

    // Chunked and too big: refused at the chunk that passes the cap.
    let mut chunks = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
    for _ in 0..3 {
        chunks.extend_from_slice(b"200\r\n");
        chunks.extend(std::iter::repeat_n(b'y', 0x200));
        chunks.extend_from_slice(b"\r\n");
    }
    chunks.extend_from_slice(b"0\r\n\r\n");
    let (addr, _, _rx) = listener(Some(chunks)).await;
    let e = egress(addr, limits)
        .fetch("acme", request(NAME, addr, false))
        .await
        .expect_err("chunked too big");
    assert_eq!(e.kind(), "body_too_large", "{e:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_far_end_that_never_answers_times_out() {
    let (addr, _, _rx) = listener(None).await;
    let limits = FnLimits {
        timeout: Duration::from_millis(300),
        ..FnLimits::default()
    };
    let started = std::time::Instant::now();
    let e = egress(addr, limits)
        .fetch("acme", request(NAME, addr, true))
        .await
        .expect_err("timed out");
    assert_eq!(e, FetchError::Timeout { after_ms: 300 });
    assert!(e.retryable());
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test(flavor = "multi_thread")]
async fn loopback_is_denied_without_the_test_escape() {
    let (addr, count, _rx) = listener(Some(ok(b"{}"))).await;
    let e = FnEgress::new(
        Arc::new(Store),
        TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
        DenyList::categories_only(),
    )
    .resolving_for_tests(NAME, addr)
    .fetch("acme", request(NAME, addr, true))
    .await
    .expect_err("denied");
    assert_eq!(e.kind(), "host_denied", "{e:?}");
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
