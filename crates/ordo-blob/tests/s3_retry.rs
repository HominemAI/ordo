//! Audit bug 6 (`docs/audit-service-interfaces.md`): an S3 429 counted as a
//! permanent refusal, and nothing retried a 5xx. The S3 binding now reads a
//! 5xx, 408, 425 or 429 as transient ([`BlobError::is_transient`]) and sends
//! a read, a HEAD or a delete again with a bounded backoff (`S3Retry`).
//!
//! The gateway is a plaintext listener on loopback that answers from a
//! script, which the binding allows for loopback only.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_blob::s3::S3Retry;
use ordo_blob::{collect, once, BlobStore, S3Config, S3Store};
use ordo_crypto::custody::SpaceId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A gateway that answers each request with the next status in its script,
/// then 200, and counts the requests.
struct Gateway {
    port: u16,
    script: Arc<Mutex<Vec<u16>>>,
    seen: Arc<Mutex<usize>>,
}

impl Gateway {
    async fn start(script: &[u16]) -> Gateway {
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = tcp.local_addr().unwrap().port();
        let script = Arc::new(Mutex::new(script.iter().rev().copied().collect::<Vec<_>>()));
        let seen = Arc::new(Mutex::new(0usize));
        let (s, n) = (script.clone(), seen.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = tcp.accept().await else {
                    return;
                };
                let (s, n) = (s.clone(), n.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(k) => buf.extend_from_slice(&chunk[..k]),
                        }
                    }
                    *n.lock().unwrap() += 1;
                    let status = s.lock().unwrap().pop().unwrap_or(200);
                    let body = if status == 200 { "hello" } else { "<Error/>" };
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(reply.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Gateway { port, script, seen }
    }

    fn store(&self, retries: u32) -> S3Store {
        let mut cfg = S3Config::new(
            &format!("http://127.0.0.1:{}", self.port),
            "us-east-1",
            "ordo-test",
            "AKIDEXAMPLE",
            "secret",
        )
        .expect("a loopback binding");
        cfg.retry = S3Retry {
            retries,
            base: Duration::from_millis(1),
            max: Duration::from_millis(5),
        };
        S3Store::new(cfg)
    }

    fn seen(&self) -> usize {
        *self.seen.lock().unwrap()
    }

    fn script(&self, statuses: &[u16]) {
        *self.script.lock().unwrap() = statuses.iter().rev().copied().collect();
        *self.seen.lock().unwrap() = 0;
    }
}

const HANDLE: [u8; 16] = [7u8; 16];

fn space() -> SpaceId {
    SpaceId("spc_retry".into())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transient_read_is_sent_again_and_a_final_refusal_is_not() {
    // Two 503s (S3's SlowDown) and a 429, then the object.
    let gw = Gateway::start(&[503, 429]).await;
    let store = gw.store(2);
    let bytes = collect(store.get(&space(), &HANDLE, None).await.expect("read"))
        .await
        .expect("body");
    assert_eq!(bytes, b"hello");
    assert_eq!(gw.seen(), 3, "two retries, then the answer");

    // Out of retries: the last transient failure comes back, typed.
    gw.script(&[500, 500, 500]);
    let err = store.head(&space(), &HANDLE).await.expect_err("down");
    assert!(err.is_transient(), "{err}");
    assert_eq!(gw.seen(), 3, "one try and two retries, bounded");

    // A 403 is a refusal: one request, permanent.
    gw.script(&[403]);
    let err = store.delete(&space(), &HANDLE).await.expect_err("refused");
    assert_eq!(err.http_status(), Some(403), "{err}");
    assert!(err.is_forbidden(), "{err}");
    assert!(!err.is_transient());
    assert_eq!(err.class(), ordo_fault::FaultClass::Broken);
    assert_eq!(gw.seen(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rate_limited_upload_is_transient_and_never_resent_by_the_binding() {
    // The body is a spent stream, so the binding does not resend it; the
    // caller is told to try again (the door answers `server_busy`).
    let gw = Gateway::start(&[429]).await;
    let err = gw
        .store(2)
        .put(&space(), &HANDLE, 5, once(b"hello".to_vec()))
        .await
        .expect_err("rate limited");
    assert!(
        err.is_transient(),
        "a 429 is not a permanent refusal: {err}"
    );
    assert_eq!(gw.seen(), 1);
}

#[test]
fn the_retry_settings_have_defaults_and_refuse_a_bad_value_by_name() {
    let none = |_: &str| None;
    assert_eq!(S3Retry::from_vars(&none).unwrap(), S3Retry::default());
    let set = |k: &str| match k {
        "ORDO_BLOB_S3_RETRIES" => Some("0".to_string()),
        "ORDO_BLOB_S3_RETRY_BASE_MS" => Some("50".to_string()),
        _ => None,
    };
    let r = S3Retry::from_vars(&set).unwrap();
    assert_eq!((r.retries, r.base), (0, Duration::from_millis(50)));
    let bad = |k: &str| (k == "ORDO_BLOB_S3_RETRIES").then(|| "99".to_string());
    let err = S3Retry::from_vars(&bad).unwrap_err();
    assert!(err.to_string().contains("ORDO_BLOB_S3_RETRIES"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_final_503_keeps_its_status_and_is_busy() {
    let gw = Gateway::start(&[503, 503, 503]).await;
    let err = gw.store(2).head(&space(), &HANDLE).await.expect_err("down");
    assert_eq!(err.http_status(), Some(503), "{err}");
    assert_eq!(err.class(), ordo_fault::FaultClass::Busy);
    assert!(err.is_busy() && err.is_transient());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_fails_keeps_its_cause_and_is_retried() {
    // Nothing listens on this port once the listener is dropped.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        l.local_addr().unwrap().port()
    };
    let mut cfg = S3Config::new(
        &format!("http://127.0.0.1:{port}"),
        "us-east-1",
        "ordo-test",
        "AKIDEXAMPLE",
        "secret",
    )
    .expect("a loopback binding");
    cfg.retry = S3Retry {
        retries: 1,
        base: Duration::from_millis(1),
        max: Duration::from_millis(5),
    };
    let err = S3Store::new(cfg)
        .head(&space(), &HANDLE)
        .await
        .expect_err("refused connection");
    assert!(err.is_transient(), "{err}");
    let cause = std::error::Error::source(&err).expect("the io error is kept");
    assert!(cause.downcast_ref::<std::io::Error>().is_some());
}
