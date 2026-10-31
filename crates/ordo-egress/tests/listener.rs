//! The six pins, each proved by an attempt that was refused — plus one round
//! trip against a real TLS listener so the refusals are not the only thing
//! the client can do.
//!
//! The listener is real: a `rustls` server on loopback with a self-signed
//! certificate for `egress.test.invalid`, which the client is pointed at with
//! [`TlsRoots::custom`]. That combination is what makes pin 1 testable
//! without a public host — certificate validation stays *on* and the test
//! supplies the anchor, rather than the test turning validation off, which is
//! the shape of every "we tested TLS" claim that turns out not to have.
//!
//! `pin_for_test` mints the destination directly, because the deny list quite
//! correctly refuses to resolve anything to 127.0.0.1 — the constructor that
//! skips the check exists only behind the `test-pin` feature, and the pins it
//! skips are proved separately in `dest.rs`'s and `deny.rs`'s unit tests.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ordo_egress::dest::pin_for_test;
use ordo_egress::{deliver, sign, Delivery, SinkSecret, TlsRoots};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

/// The trust anchor the client is given. A real CA certificate, not the leaf:
/// `webpki` refuses `CaUsedAsEndEntity`, which is itself a small vote of
/// confidence in the stack — a self-signed CA masquerading as a server is
/// exactly the shape a lazy test wants and a correct verifier rejects.
const CA: &[u8] = include_bytes!("fixture/ca.der");
/// What the listener presents: a leaf for `egress.test.invalid`, signed by
/// that CA, `CA:FALSE`, `serverAuth`.
const LEAF: &[u8] = include_bytes!("fixture/leaf.der");
const LEAF_KEY: &[u8] = include_bytes!("fixture/leaf-key.der");
const NAME: &str = "egress.test.invalid";

/// What the listener saw: the raw request head and body.
#[derive(Debug)]
struct Seen {
    head: String,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<String> {
        self.head
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")))
            .map(str::to_string)
    }
}

fn acceptor() -> TlsAcceptor {
    let cert = rustls::pki_types::CertificateDer::from(LEAF.to_vec());
    let key = rustls::pki_types::PrivateKeyDer::try_from(LEAF_KEY.to_vec()).expect("pkcs8 key");
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config");
    TlsAcceptor::from(Arc::new(cfg))
}

/// Start a listener that answers `response` to one request, and reports what
/// it received. Returns the bound address and the receiver.
async fn listener(response: &'static str) -> (SocketAddr, mpsc::Receiver<Seen>) {
    let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = tcp.local_addr().unwrap();
    let (tx, rx) = mpsc::channel(4);
    let acceptor = acceptor();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = tcp.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(sock).await else {
                    return;
                };
                if let Ok(seen) = read_request(&mut tls).await {
                    let _ = tx.send(seen).await;
                }
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.flush().await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, rx)
}

async fn read_request<S>(s: &mut S) -> io::Result<Seen>
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
    Ok(Seen { head, body })
}

fn roots() -> TlsRoots {
    TlsRoots::custom(&[CA.to_vec()]).expect("anchor")
}

fn secret() -> SinkSecret {
    SinkSecret::from_bytes(b"a per-sink secret, shown once at grant")
}

const BODY: &[u8] = br#"{"ordo.attestation/1":true}"#;

async fn post(addr: SocketAddr, path: &str) -> Result<ordo_egress::Accepted, ordo_egress::Refusal> {
    let dest = pin_for_test(NAME, addr, path);
    let secret = secret();
    deliver(
        dest,
        &roots(),
        Delivery {
            body: BODY,
            content_type: "application/json",
            timestamp: 1_757_000_000,
            headers: &[("x-ordo-space", "acme".to_string())],
            secret: &secret,
        },
        Duration::from_secs(5),
    )
    .await
}

// ── the round trip ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_round_trip_over_real_tls() {
    let (addr, mut rx) = listener("HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n").await;
    let out = post(addr, "/attest").await.expect("accepted");
    assert_eq!(out.status, 204);

    let seen = rx.recv().await.expect("the listener saw a request");
    assert!(
        seen.head.starts_with("POST /attest HTTP/1.1"),
        "{}",
        seen.head
    );
    // The `Host` header carries the **name**, with the non-default port —
    // never the address the socket went to. That split is pin 2's whole
    // trick: the connection is pinned and the authentication is not weakened.
    assert_eq!(seen.header("host"), Some(format!("{NAME}:{}", addr.port())));
    assert_eq!(seen.body, BODY);
    assert_eq!(seen.header("x-ordo-space").as_deref(), Some("acme"));

    // Pin 4, from the receiver's side: the header verifies under the secret,
    // over the timestamp, the canonical destination, and the body — using the
    // published routine, not a re-implementation.
    let sig = seen.header("x-ordo-signature").expect("signature header");
    let ts: u64 = seen
        .header("x-ordo-timestamp")
        .expect("timestamp header")
        .parse()
        .unwrap();
    let canonical = format!("https://{NAME}:{}/attest", addr.port());
    assert!(
        sign::verify(&secret(), &sig, ts, &canonical, &seen.body),
        "the receiver can authenticate the platform"
    );
    // And a body one byte different does not verify.
    assert!(!sign::verify(&secret(), &sig, ts, &canonical, b"{}"));
}

// ── pin 3: no redirect following, ever ────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_is_answered_and_refused_rather_than_followed() {
    // The listener answers a 302 pointing at the metadata endpoint. A client
    // that follows redirects lands on 169.254.169.254 with the deny list
    // already satisfied — which is the exact "one `Location:` header away
    // from irrelevant" §7b names.
    let (addr, mut rx) = listener(
        "HTTP/1.1 302 Found\r\nlocation: http://169.254.169.254/latest/meta-data/\r\n\
         content-length: 0\r\n\r\n",
    )
    .await;
    let e = post(addr, "/attest").await.unwrap_err();
    assert_eq!(e.kind(), "redirect_answered");
    assert!(
        !e.retryable(),
        "a 3xx is a configuration fact, not a transient"
    );
    // The request was made exactly once. If the client had followed, there
    // would have been a second.
    assert!(rx.recv().await.is_some());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(rx.try_recv().is_err(), "exactly one request was made");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_three_hundred_is_a_delivery_failure() {
    for (code, phrase) in [
        (301, "Moved Permanently"),
        (302, "Found"),
        (303, "See Other"),
        (307, "Temporary Redirect"),
        (308, "Permanent Redirect"),
    ] {
        let leaked: &'static str = Box::leak(
            format!("HTTP/1.1 {code} {phrase}\r\nlocation: https://elsewhere.invalid/\r\ncontent-length: 0\r\n\r\n")
                .into_boxed_str(),
        );
        let (addr, _rx) = listener(leaked).await;
        let e = post(addr, "/attest").await.unwrap_err();
        assert_eq!(
            e,
            ordo_egress::Refusal::RedirectAnswered { status: code },
            "{code}"
        );
    }
}

// ── pin 1: TLS is validated, and there is no way to ask it not to be ──────

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_certificate_is_a_refusal_not_a_warning() {
    let (addr, _rx) = listener("HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n").await;
    // The public root program, against a certificate chaining to a test CA
    // that is not in it.
    let dest = pin_for_test(NAME, addr, "/attest");
    let secret = secret();
    let e = deliver(
        dest,
        &TlsRoots::webpki(),
        Delivery {
            body: BODY,
            content_type: "application/json",
            timestamp: 1,
            headers: &[],
            secret: &secret,
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert_eq!(e.kind(), "tls_failed");
    assert_eq!(
        e.disclosable(),
        "TLS handshake failed",
        "a prober is not told which half of validation they beat"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_for_another_name_is_refused() {
    let (addr, _rx) = listener("HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n").await;
    // The anchor is trusted and the chain is valid — but the certificate is
    // presented for a name it does not carry. Pinning the *address* must not
    // weaken name checking, and this is the assertion that says so.
    let dest = pin_for_test("other.test.invalid", addr, "/attest");
    let secret = secret();
    let e = deliver(
        dest,
        &roots(),
        Delivery {
            body: BODY,
            content_type: "application/json",
            timestamp: 1,
            headers: &[],
            secret: &secret,
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert_eq!(e.kind(), "tls_failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plaintext_listener_never_gets_a_request() {
    // No TLS on the far end at all. The handshake fails; the POST — which
    // carries the space's payload — is never written.
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let (tx, mut rx) = mpsc::channel(1);
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = tcp.accept().await {
            let mut buf = [0u8; 4096];
            if let Ok(n) = sock.read(&mut buf).await {
                let _ = tx.send(buf[..n].to_vec()).await;
            }
        }
    });
    let e = post(addr, "/attest").await.unwrap_err();
    assert_eq!(e.kind(), "tls_failed");
    let first = rx.recv().await.expect("something arrived");
    assert_ne!(&first[..1], b"P", "a ClientHello, never a POST");
    assert!(
        !first.windows(4).any(|w| w == b"ordo"),
        "no payload byte reached a plaintext listener"
    );
}

// ── what does not come back ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_response_body_does_not_reach_the_caller() {
    // §7b: "sinks deliver payloads and log deliveries, so the response or its
    // metadata comes back to the tenant". `Accepted` has one field and it is
    // a number, so there is nowhere for this to go.
    let (addr, _rx) = listener(
        "HTTP/1.1 200 OK\r\ncontent-length: 43\r\nx-secret: ssh-rsa AAAA\r\n\r\n\
         AKIAIOSFODNN7EXAMPLE/wJalrXUtnFEMI/K7MDENG",
    )
    .await;
    let out = post(addr, "/attest").await.expect("accepted");
    assert_eq!(out, ordo_egress::Accepted { status: 200 });
    // The type has no other field. This line is the assertion; it would not
    // compile if a body or a header map were added.
    let ordo_egress::Accepted { status } = out;
    assert_eq!(status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejection_carries_its_status_and_nothing_else() {
    // 408, 425 and 429 ask for a later attempt, as every 5xx does; the rest
    // of the 4xx are a no.
    for (code, retryable) in [
        (400u16, false),
        (403, false),
        (404, false),
        (408, true),
        (425, true),
        (429, true),
        (500, true),
        (503, true),
    ] {
        let leaked: &'static str = Box::leak(
            format!("HTTP/1.1 {code} X\r\ncontent-length: 5\r\n\r\nsorry").into_boxed_str(),
        );
        let (addr, _rx) = listener(leaked).await;
        let e = post(addr, "/attest").await.unwrap_err();
        assert_eq!(
            e,
            ordo_egress::Refusal::Rejected {
                status: code,
                retry_after_secs: None
            }
        );
        assert_eq!(e.retryable(), retryable, "{code}");
        assert!(!e.disclosable().contains("sorry"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rate_limit_carries_its_retry_after_in_seconds_and_nothing_else() {
    let (addr, _rx) =
        listener("HTTP/1.1 429 Too Many\r\nretry-after: 30\r\ncontent-length: 5\r\n\r\nslow!")
            .await;
    let e = post(addr, "/attest").await.unwrap_err();
    assert!(e.retryable(), "a 429 is tried again");
    assert_eq!(e.retry_after(), Some(std::time::Duration::from_secs(30)));

    // An HTTP-date is not read: the caller's own backoff applies.
    let (addr, _rx) = listener(
        "HTTP/1.1 503 X\r\nRetry-After: Wed, 21 Oct 2015 07:28:00 GMT\r\ncontent-length: 0\r\n\r\n",
    )
    .await;
    let e = post(addr, "/attest").await.unwrap_err();
    assert!(e.retryable());
    assert_eq!(e.retry_after(), None);

    // A huge value is clamped to one day.
    let (addr, _rx) =
        listener("HTTP/1.1 429 X\r\nretry-after: 99999999\r\ncontent-length: 0\r\n\r\n").await;
    let e = post(addr, "/attest").await.unwrap_err();
    assert_eq!(
        e.retry_after(),
        Some(std::time::Duration::from_secs(86_400))
    );

    // A final refusal has no later attempt to schedule, whatever it says.
    let (addr, _rx) =
        listener("HTTP/1.1 400 X\r\nretry-after: 5\r\ncontent-length: 0\r\n\r\n").await;
    let e = post(addr, "/attest").await.unwrap_err();
    assert!(!e.retryable());
    assert_eq!(e.retry_after(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_destination_that_answers_gibberish_is_bounded() {
    let (addr, _rx) = listener("this is not HTTP\r\n\r\n").await;
    let e = post(addr, "/attest").await.unwrap_err();
    assert_eq!(e.kind(), "malformed_response");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_destination_that_never_answers_times_out() {
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let acceptor = acceptor();
    tokio::spawn(async move {
        if let Ok((sock, _)) = tcp.accept().await {
            if let Ok(mut tls) = acceptor.accept(sock).await {
                let mut buf = [0u8; 4096];
                let _ = tls.read(&mut buf).await;
                // and then nothing, forever.
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
        }
    });
    let dest = pin_for_test(NAME, addr, "/attest");
    let secret = secret();
    let e = deliver(
        dest,
        &roots(),
        Delivery {
            body: BODY,
            content_type: "application/json",
            timestamp: 1,
            headers: &[],
            secret: &secret,
        },
        Duration::from_millis(300),
    )
    .await
    .unwrap_err();
    assert_eq!(e.kind(), "timed_out");
    assert!(e.retryable());
}

// ── pin 2, the half that needs a socket ───────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn the_socket_goes_to_the_pinned_address_not_the_name() {
    // `egress.test.invalid` does not resolve — `.invalid` is reserved by
    // RFC 2606 precisely so that it cannot. The delivery lands anyway,
    // because the client dials `dest.addr()` and uses the name only for TLS.
    // If the client resolved the hostname at connect time, this would fail,
    // which is the rebind window §7b says most implementations leave open.
    assert!(
        tokio::net::lookup_host(format!("{NAME}:443"))
            .await
            .is_err(),
        "the fixture name must not resolve, or this test proves nothing"
    );
    let (addr, mut rx) = listener("HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\n\r\n").await;
    let out = post(addr, "/attest").await.expect("accepted");
    assert_eq!(out.status, 202);
    assert!(rx.recv().await.is_some());
}
