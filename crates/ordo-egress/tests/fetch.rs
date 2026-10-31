//! `fetch`, the verifier's door, against a real TLS listener.
//!
//! The listener is the one `listener.rs` uses: a `rustls` server on loopback
//! with a leaf for `egress.test.invalid` signed by a test CA the client is
//! given as its only anchor. Certificate validation stays on. What is proved
//! here is the half `deliver` never had to have: a body comes back, whole
//! when it is chunked, refused when it is too big, and never when the answer
//! is a redirect.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ordo_egress::dest::pin_for_test;
use ordo_egress::{fetch, FetchMethod, FetchRequest, Refusal, TlsRoots};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

const CA: &[u8] = include_bytes!("fixture/ca.der");
const LEAF: &[u8] = include_bytes!("fixture/leaf.der");
const LEAF_KEY: &[u8] = include_bytes!("fixture/leaf-key.der");
const NAME: &str = "egress.test.invalid";

fn acceptor() -> TlsAcceptor {
    let cert = rustls::pki_types::CertificateDer::from(LEAF.to_vec());
    let key = rustls::pki_types::PrivateKeyDer::try_from(LEAF_KEY.to_vec()).expect("pkcs8 key");
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config");
    TlsAcceptor::from(Arc::new(cfg))
}

/// A listener that answers `response` to every request, counts the
/// connections it accepted, and reports each request head it read.
async fn listener(response: Vec<u8>) -> (SocketAddr, Arc<AtomicUsize>, mpsc::Receiver<String>) {
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
            let acceptor = acceptor.clone();
            let tx = tx.clone();
            let response = response.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(sock).await else {
                    return;
                };
                if let Ok(head) = read_request(&mut tls).await {
                    let _ = tx.send(head).await;
                }
                let _ = tls.write_all(&response).await;
                let _ = tls.flush().await;
                let _ = tls.shutdown().await;
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

fn roots() -> TlsRoots {
    TlsRoots::custom(&[CA.to_vec()]).expect("anchor")
}

async fn get(addr: SocketAddr, cap: usize) -> Result<Vec<u8>, Refusal> {
    fetch(
        pin_for_test(NAME, addr, "/userinfo"),
        &roots(),
        FetchRequest {
            method: FetchMethod::Get,
            body: &[],
            accept: "application/json",
            headers: &[("authorization", "Bearer at-1".to_string())],
        },
        cap,
        Duration::from_secs(5),
    )
    .await
    .map(|f| f.into_verifier_input())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_is_a_refusal_and_no_second_request_is_made() {
    let (addr, count, _rx) = listener(
        b"HTTP/1.1 302 Found\r\nlocation: https://169.254.169.254/latest\r\ncontent-length: 0\r\n\r\n"
            .to_vec(),
    )
    .await;
    let e = get(addr, 1024).await.expect_err("refused");
    assert_eq!(e, Refusal::RedirectAnswered { status: 302 });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(count.load(Ordering::SeqCst), 1, "one connection, no hop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_over_its_cap_is_refused_before_it_is_held() {
    // Declared too big: refused on the declaration. The listener sends only
    // a few bytes of the claimed megabyte, so a reader that tried to hold
    // it would wait for bytes that never come; this one answers at once.
    let (addr, _, _rx) =
        listener(b"HTTP/1.1 200 OK\r\ncontent-length: 1048576\r\n\r\n{\"a\":".to_vec()).await;
    let started = std::time::Instant::now();
    let e = get(addr, 64 * 1024).await.expect_err("refused");
    assert_eq!(e.kind(), "malformed_response", "{e:?}");
    assert!(started.elapsed() < Duration::from_secs(2));

    // Undeclared and too big: refused as it passes the cap.
    let mut big = b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n".to_vec();
    big.extend(std::iter::repeat_n(b'x', 70 * 1024));
    let (addr, _, _rx) = listener(big).await;
    assert_eq!(
        get(addr, 64 * 1024).await.err().map(|e| e.kind()),
        Some("malformed_response")
    );

    // Chunked and too big: refused at the chunk that passes the cap.
    let mut chunks = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
    for _ in 0..3 {
        chunks.extend_from_slice(b"8000\r\n");
        chunks.extend(std::iter::repeat_n(b'y', 0x8000));
        chunks.extend_from_slice(b"\r\n");
    }
    chunks.extend_from_slice(b"0\r\n\r\n");
    let (addr, _, _rx) = listener(chunks).await;
    assert_eq!(
        get(addr, 64 * 1024).await.err().map(|e| e.kind()),
        Some("malformed_response")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chunked_body_is_read_whole() {
    let (addr, _, mut rx) = listener(
        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n\
          5\r\n{\"id\"\r\n4;ext=1\r\n:\"80\r\n9\r\n35111022\"\r\n1\r\n}\r\n0\r\nx-trailer: 1\r\n\r\n"
            .to_vec(),
    )
    .await;
    let body = get(addr, 1024).await.expect("fetched");
    assert_eq!(body, br#"{"id":"8035111022"}"#);
    let head = rx.recv().await.expect("a request");
    assert!(head.starts_with("GET /userinfo HTTP/1.1\r\n"), "{head}");
    assert!(head.contains("accept-encoding: identity\r\n"), "{head}");
    assert!(head.contains("authorization: Bearer at-1\r\n"), "{head}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_form_post_carries_its_body_and_a_length_framed_answer_comes_back() {
    let answer = br#"{"access_token":"t","token_type":"Bearer"}"#;
    let mut response = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
        answer.len()
    )
    .into_bytes();
    response.extend_from_slice(answer);
    let (addr, _, mut rx) = listener(response).await;
    let form = b"grant_type=authorization_code&code=c1";
    let got = fetch(
        pin_for_test(NAME, addr, "/token"),
        &roots(),
        FetchRequest {
            method: FetchMethod::PostForm,
            body: form,
            accept: "application/json",
            headers: &[],
        },
        64 * 1024,
        Duration::from_secs(5),
    )
    .await
    .expect("fetched");
    assert_eq!(got.status(), 200);
    assert_eq!(
        got.into_verifier_input(),
        br#"{"access_token":"t","token_type":"Bearer"}"#
    );
    let seen = rx.recv().await.expect("a request");
    assert!(seen.starts_with("POST /token HTTP/1.1\r\n"), "{seen}");
    assert!(
        seen.contains("content-type: application/x-www-form-urlencoded\r\n"),
        "{seen}"
    );
    assert!(
        seen.ends_with("grant_type=authorization_code&code=c1"),
        "{seen}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_status_returns_no_body_and_an_encoded_body_is_refused() {
    let (addr, _, _rx) = listener(
        b"HTTP/1.1 400 Bad Request\r\ncontent-length: 27\r\n\r\n{\"error\":\"invalid_grant\"}  "
            .to_vec(),
    )
    .await;
    assert_eq!(
        get(addr, 1024).await.err(),
        Some(Refusal::Rejected {
            status: 400,
            retry_after_secs: None
        })
    );
    let (addr, _, _rx) = listener(
        b"HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\ncontent-length: 2\r\n\r\nxx".to_vec(),
    )
    .await;
    assert_eq!(
        get(addr, 1024).await.err().map(|e| e.kind()),
        Some("malformed_response")
    );
    let (addr, _, _rx) = listener(
        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n"
            .to_vec(),
    )
    .await;
    assert_eq!(
        get(addr, 1024).await.err().map(|e| e.kind()),
        Some("malformed_response"),
        "both framings is the smuggling shape"
    );
}
