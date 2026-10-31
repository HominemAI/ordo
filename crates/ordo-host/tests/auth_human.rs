//! The optional human check at `begin` (`plan-auth-federated.md` step 6, E15):
//! the real `/auth/begin` route, the egress kernel, and a real TLS listener
//! standing in for Cloudflare's siteverify. Nothing here calls Cloudflare.
//!
//! What it proves:
//!
//! - with the check on, a `begin` without a token sends nothing out: no
//!   siteverify request and no request to the provider;
//! - with the check off, no request leaves the deployment;
//! - a token siteverify accepts begins the ceremony, and the request carries
//!   the secret and the token and no address of the visitor;
//! - a rejected token, a missing token and an unreachable siteverify are the
//!   one answer, and the secret is in no answer.

mod oauth2_support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use oauth2_support::{send, FakeDiscord, RETURN};
use ordo_egress::{DenyList, TlsRoots};
use ordo_host::auth::human::HumanCheck;
use ordo_host::auth::AuthState;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::federation::percent_decode;
use ordo_identity::{MemoryCeremonyStore, ProviderRegistry, TokenKey};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const CA: &[u8] = include_bytes!("fixture/ca.der");
const LEAF: &[u8] = include_bytes!("fixture/localhost.der");
const LEAF_KEY: &[u8] = include_bytes!("fixture/localhost-key.der");

const SECRET: &str = "turnstile-secret-do-not-print";
const GOOD: &str = "token-from-a-real-browser";

type Forms = Arc<Mutex<Vec<HashMap<String, String>>>>;

/// Cloudflare's siteverify, as far as `begin` uses it: `success` is true for
/// the one good token under the one secret.
struct FakeSiteverify {
    port: u16,
    seen: Forms,
}

impl FakeSiteverify {
    async fn start() -> FakeSiteverify {
        // Every address `localhost` resolves to, on one port: CI resolves to
        // `::1` first, a laptop to `127.0.0.1`, and the client pins the first.
        let answers: Vec<std::net::SocketAddr> = tokio::net::lookup_host("localhost:0")
            .await
            .expect("localhost resolves")
            .collect();
        let first = *answers.first().expect("an address");
        let tcp = TcpListener::bind((first.ip(), 0)).await.expect("bind");
        let port = tcp.local_addr().unwrap().port();
        let mut listeners = vec![tcp];
        for other in answers.iter().skip(1) {
            if other.ip() != first.ip() {
                if let Ok(l) = TcpListener::bind((other.ip(), port)).await {
                    listeners.push(l);
                }
            }
        }
        let seen: Forms = Arc::default();
        let cert = rustls::pki_types::CertificateDer::from(LEAF.to_vec());
        let key = rustls::pki_types::PrivateKeyDer::try_from(LEAF_KEY.to_vec()).expect("key");
        let cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .expect("server config");
        let acceptor = TlsAcceptor::from(Arc::new(cfg));
        for tcp in listeners {
            let (acceptor, seen) = (acceptor.clone(), seen.clone());
            tokio::spawn(async move {
                loop {
                    let Ok((sock, _)) = tcp.accept().await else {
                        return;
                    };
                    let (acceptor, seen) = (acceptor.clone(), seen.clone());
                    tokio::spawn(async move {
                        let Ok(mut tls) = acceptor.accept(sock).await else {
                            return;
                        };
                        let Some(form) = read_form(&mut tls).await else {
                            return;
                        };
                        let ok = form.get("secret").map(String::as_str) == Some(SECRET)
                            && form.get("response").map(String::as_str) == Some(GOOD);
                        seen.lock().unwrap().push(form);
                        let body = if ok {
                            json!({"success": true, "hostname": "app.example.com"})
                        } else {
                            json!({"success": false, "error-codes": ["invalid-input-response"]})
                        }
                        .to_string();
                        let res = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = tls.write_all(res.as_bytes()).await;
                        let _ = tls.flush().await;
                        let _ = tls.shutdown().await;
                    });
                }
            });
        }
        FakeSiteverify { port, seen }
    }

    fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn check(&self) -> HumanCheck {
        check_at(&format!(
            "https://localhost:{}/turnstile/v0/siteverify",
            self.port
        ))
    }
}

fn check_at(url: &str) -> HumanCheck {
    HumanCheck::new(
        SECRET.to_string(),
        url,
        TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
        DenyList::categories_only().permitting_loopback_for_tests(),
    )
    .expect("a valid check")
}

async fn read_form<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> Option<HashMap<String, String>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let end = loop {
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
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
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(
        String::from_utf8_lossy(&body)
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| {
                (
                    percent_decode(k).unwrap_or_default(),
                    percent_decode(v).unwrap_or_default(),
                )
            })
            .collect(),
    )
}

struct H {
    app: axum::Router,
    provider: FakeDiscord,
}

/// A deployment with `discord` as its method (its `begin` is a redirect and
/// makes no outside request) and the given check, if any.
async fn harness(check: Option<HumanCheck>) -> H {
    let provider = FakeDiscord::start().await;
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let discord = provider.provider("discord", ceremonies.clone());
    let mut registry = ProviderRegistry::new();
    registry
        .register("discord", Box::new(discord.clone()))
        .expect("registered");
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store),
        TokenKey::from_secret(b"auth-human-test-secret"),
    )
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_ceremonies(ceremonies)
    .with_oauth2(discord);
    if let Some(c) = check {
        state = state.with_human_check(c);
    }
    state.now = Box::new(|| 1_800_000_000);
    H {
        app: ordo_host::auth::router(Some(Arc::new(state))),
        provider,
    }
}

async fn begin(h: &H, human: Option<Value>) -> (StatusCode, Value) {
    let mut body = json!({"method": "discord", "return_to": RETURN});
    if let Some(v) = human {
        body["human"] = v;
    }
    let (status, _, answer) = send(&h.app, "POST", "/auth/begin", Some(body), None).await;
    (status, answer)
}

#[tokio::test]
async fn with_the_check_on_a_begin_without_a_token_sends_nothing() {
    let fake = FakeSiteverify::start().await;
    let h = harness(Some(fake.check())).await;
    for human in [None, Some(json!("")), Some(json!(7)), Some(json!(null))] {
        let (status, answer) = begin(&h, human).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
        assert_eq!(answer["error"]["code"], "authentication_failed");
    }
    assert_eq!(fake.requests(), 0, "no request to siteverify");
    assert!(
        h.provider.seen.lock().unwrap().is_empty(),
        "no request to the provider"
    );
}

#[tokio::test]
async fn with_the_check_off_no_request_leaves_the_deployment() {
    let fake = FakeSiteverify::start().await;
    let h = harness(None).await;
    // A token sent to a deployment with the check off is ignored.
    for human in [None, Some(json!(GOOD))] {
        let (status, answer) = begin(&h, human).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert!(answer["redirect"].is_string());
    }
    assert_eq!(fake.requests(), 0);
    assert!(h.provider.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_token_siteverify_accepts_begins_and_the_request_carries_no_address() {
    let fake = FakeSiteverify::start().await;
    let h = harness(Some(fake.check())).await;
    let (status, answer) = begin(&h, Some(json!(GOOD))).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(answer["redirect"].is_string() && answer["binder"].is_string());
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "one token, one verification");
    let mut keys: Vec<&str> = seen[0].keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["response", "secret"], "no remoteip, nothing else");
    assert_eq!(seen[0]["response"], GOOD);
}

#[tokio::test]
async fn a_token_siteverify_rejects_is_the_one_refusal_and_no_ceremony_begins() {
    let fake = FakeSiteverify::start().await;
    let h = harness(Some(fake.check())).await;
    let (status, answer) = begin(&h, Some(json!("forged"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
    assert_eq!(fake.requests(), 1);
    let (_, missing) = begin(&h, None).await;
    assert_eq!(answer, missing, "rejected and missing are one answer");
    let text = answer.to_string();
    assert!(
        !text.contains(SECRET) && !text.contains("invalid-input"),
        "{text}"
    );
}

#[tokio::test]
async fn an_unreachable_siteverify_refuses_and_leaks_nothing() {
    // A port nothing listens on.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let h = harness(Some(check_at(&format!(
        "https://localhost:{closed}/turnstile/v0/siteverify"
    ))))
    .await;
    let (status, answer) = begin(&h, Some(json!(GOOD))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
    let (_, missing) = begin(&h, None).await;
    assert_eq!(answer, missing);
}

#[tokio::test]
async fn an_oversized_token_is_refused_before_any_request() {
    let fake = FakeSiteverify::start().await;
    let h = harness(Some(fake.check())).await;
    let (status, _) = begin(&h, Some(json!("x".repeat(4096)))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(fake.requests(), 0);
}

/// A deployment with a passkey method, for the first leg of sign-up.
fn passkey_harness(check: HumanCheck) -> axum::Router {
    let passkey = Arc::new(ordo_identity::PasskeyProvider::with_stores(
        ordo_identity::RelyingParty::new("example.com", &["https://app.example.com"])
            .expect("a relying party"),
        Arc::new(ordo_identity::MemoryPasskeyStore::new()),
        Arc::new(MemoryCeremonyStore::new()),
    ));
    let mut registry = ProviderRegistry::new();
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("passkey");
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store),
        TokenKey::from_secret(b"auth-human-test-secret"),
    )
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_passkey(passkey)
    .with_human_check(check);
    state.now = Box::new(|| 1_800_000_000);
    ordo_host::auth::router(Some(Arc::new(state)))
}

#[tokio::test]
async fn with_the_check_on_a_register_without_a_token_mints_nothing() {
    let fake = FakeSiteverify::start().await;
    let app = passkey_harness(fake.check());
    for human in [None, Some(json!("")), Some(json!(7)), Some(json!("forged"))] {
        let mut body = json!({"method": "passkey"});
        if let Some(v) = human.clone() {
            body["human"] = v;
        }
        let (status, _, answer) = send(&app, "POST", "/auth/register", Some(body), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
        assert_eq!(answer["error"]["code"], "authentication_failed");
        assert!(answer.get("challenge").is_none() && answer.get("publicKey").is_none());
    }
    // Only the forged token reached siteverify.
    assert_eq!(fake.requests(), 1);
}

#[tokio::test]
async fn with_the_check_on_a_register_with_an_accepted_token_mints_the_ceremony() {
    let fake = FakeSiteverify::start().await;
    let app = passkey_harness(fake.check());
    let body = json!({"method": "passkey", "human": GOOD});
    let (status, _, answer) = send(&app, "POST", "/auth/register", Some(body), None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(answer.to_string().contains("challenge"), "{answer}");
    assert_eq!(fake.requests(), 1);
}
