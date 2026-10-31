//! A stand-in for Discord: a real TLS listener on loopback that answers the
//! token exchange and the userinfo call the way Discord does, and the
//! harness that drives a sign-in through the real routes against it.
//!
//! **Nothing here calls Discord.** The listener presents a leaf for
//! `localhost` signed by the test CA (`fixture/`), the connection's endpoints
//! name `https://localhost:<port>/...`, and the transport is the deployment's
//! own [`EgressTransport`] with that CA as its only anchor and a deny list
//! that permits loopback. That permission exists only behind `ordo-egress`'s
//! `test-pin` feature, which `ordo-host` enables as a dev-dependency and
//! `holds_nothing.rs` forbids anywhere else. Certificate validation stays on.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_egress::{DenyList, TlsRoots};
use ordo_host::federation::EgressTransport;
use ordo_identity::federation::{percent_decode, ClientSecret};
use ordo_identity::oauth2::{preset, OAuth2Connection, OAuth2Provider};
use ordo_identity::CeremonyStore;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

const CA: &[u8] = include_bytes!("../fixture/ca.der");
const LEAF: &[u8] = include_bytes!("../fixture/localhost.der");
const LEAF_KEY: &[u8] = include_bytes!("../fixture/localhost-key.der");

/// The client secret the fake provider expects, as the app developer would
/// have put it in a file.
pub const CLIENT_SECRET: &str = "discord-client-secret-do-not-print";
pub const CLIENT_ID: &str = "1100220033";
pub const RETURN: &str = "https://app.example.com/signed-in";
pub const PUBLIC: &str = "https://api.example.com";

/// What the fake provider saw: one line per request, `METHOD path`, and the
/// form of a token request.
#[derive(Debug, Clone)]
pub struct Seen {
    pub line: String,
    pub form: HashMap<String, String>,
    pub authorization: Option<String>,
}

/// The fake. `authorize(code, profile)` plays the person signing in at
/// Discord: from then on `code` exchanges for a token that reads `profile`.
pub struct FakeDiscord {
    pub port: u16,
    codes: Arc<Mutex<HashMap<String, Value>>>,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeDiscord {
    pub async fn start() -> FakeDiscord {
        // Bound where the egress client will connect. The connection names
        // `localhost`, which answers both loopbacks in an order the resolver
        // does not promise, and the client pins the first address it is
        // given. A listener on `127.0.0.1` alone passed on a machine whose
        // resolver put `127.0.0.1` first and failed on CI's Linux runner,
        // which puts `::1` first: every exchange was refused at connect and
        // the callback answered `identity_provider_unavailable`. So the
        // listener binds the first address the same resolver answers, and
        // then every other one on the same port where it can, so a change of
        // order between two lookups cannot split them either. The production
        // rule (pin the first answer, every answer through the deny list) is
        // untouched.
        let answers: Vec<std::net::SocketAddr> = tokio::net::lookup_host("localhost:0")
            .await
            .expect("localhost resolves")
            .collect();
        let first = *answers.first().expect("localhost answers an address");
        let tcp = TcpListener::bind((first.ip(), 0)).await.expect("bind");
        let port = tcp.local_addr().unwrap().port();
        let mut listeners = vec![tcp];
        for other in answers.iter().skip(1) {
            if other.ip() == first.ip() {
                continue;
            }
            if let Ok(l) = TcpListener::bind((other.ip(), port)).await {
                listeners.push(l);
            }
        }
        let codes: Arc<Mutex<HashMap<String, Value>>> = Arc::default();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let acceptor = acceptor();
        for tcp in listeners {
            let (acceptor, c, s) = (acceptor.clone(), codes.clone(), seen.clone());
            tokio::spawn(async move {
                loop {
                    let Ok((sock, _)) = tcp.accept().await else {
                        return;
                    };
                    let (acceptor, c, s) = (acceptor.clone(), c.clone(), s.clone());
                    tokio::spawn(async move {
                        let Ok(mut tls) = acceptor.accept(sock).await else {
                            return;
                        };
                        let Ok((head, body)) = read_request(&mut tls).await else {
                            return;
                        };
                        let response = answer(&head, &body, &c, &s);
                        let _ = tls.write_all(response.as_bytes()).await;
                        let _ = tls.flush().await;
                        let _ = tls.shutdown().await;
                    });
                }
            });
        }
        FakeDiscord { port, codes, seen }
    }

    /// The person signed in at the provider as `profile`; `code` is what the
    /// provider puts in the redirect.
    pub fn authorize(&self, code: &str, profile: Value) {
        self.codes.lock().unwrap().insert(code.to_string(), profile);
    }

    fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.port)
    }

    /// The `discord` preset, pointed at this listener.
    pub fn connection(&self, name: &str) -> OAuth2Connection {
        let p = preset("discord").unwrap();
        OAuth2Connection {
            name: name.into(),
            client_id: CLIENT_ID.into(),
            client_secret: ClientSecret::new(CLIENT_SECRET.into()),
            authorize_url: self.url("/oauth2/authorize"),
            token_url: self.url("/api/oauth2/token"),
            userinfo_url: self.url("/api/users/@me"),
            scope: p.scope.into(),
            subject_claim: p.subject_claim.into(),
            email_claim: p.email_claim.map(str::to_string),
            email_verified_claim: p.email_verified_claim.map(str::to_string),
            display_claims: p.display_claims.iter().map(|s| s.to_string()).collect(),
            avatar: p.avatar.rule(),
            redirect_uri: format!("{PUBLIC}/auth/callback/{name}"),
            returns: ordo_host::federation::parse_return_urls(RETURN, false)
                .expect("a return list"),
            mutable_subject_acknowledged: false,
        }
    }

    /// The provider a deployment would build, over the egress kernel.
    pub fn provider(&self, name: &str, ceremonies: Arc<dyn CeremonyStore>) -> Arc<OAuth2Provider> {
        self.provider_with(
            name,
            ceremonies,
            DenyList::categories_only().permitting_loopback_for_tests(),
        )
    }

    /// The same, under a deny list of the caller's choosing.
    pub fn provider_with(
        &self,
        name: &str,
        ceremonies: Arc<dyn CeremonyStore>,
        deny: DenyList,
    ) -> Arc<OAuth2Provider> {
        let conn = self.connection(name);
        let transport = Arc::new(EgressTransport::new(
            TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
            deny,
            &conn.hosts(),
        ));
        Arc::new(OAuth2Provider::new(conn, ceremonies, transport).expect("a valid connection"))
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

async fn read_request<S>(s: &mut S) -> io::Result<(String, Vec<u8>)>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
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
    Ok((head, body))
}

fn form_of(body: &[u8]) -> HashMap<String, String> {
    String::from_utf8_lossy(body)
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| {
            (
                percent_decode(k).unwrap_or_default(),
                percent_decode(v).unwrap_or_default(),
            )
        })
        .collect()
}

fn json_response(status: &str, body: &Value) -> String {
    let text = body.to_string();
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    )
}

/// Discord's two endpoints, as far as a sign-in uses them.
fn answer(
    head: &str,
    body: &[u8],
    codes: &Mutex<HashMap<String, Value>>,
    seen: &Mutex<Vec<Seen>>,
) -> String {
    let line = head.lines().next().unwrap_or_default().to_string();
    let authorization = head
        .lines()
        .find_map(|l| l.strip_prefix("authorization: "))
        .map(str::to_string);
    let form = form_of(body);
    seen.lock().unwrap().push(Seen {
        line: line.clone(),
        form: form.clone(),
        authorization: authorization.clone(),
    });
    if line.starts_with("POST /api/oauth2/token ") {
        let ok = form.get("grant_type").map(String::as_str) == Some("authorization_code")
            && form.get("client_id").map(String::as_str) == Some(CLIENT_ID)
            && form.get("client_secret").map(String::as_str) == Some(CLIENT_SECRET)
            && form.get("code_verifier").is_some_and(|v| v.len() >= 43)
            && form
                .get("redirect_uri")
                .is_some_and(|r| r.starts_with(PUBLIC));
        let code = form.get("code").cloned().unwrap_or_default();
        // The provider having a bad day: a code named for a status answers
        // that status, as an outage or a rate limit would.
        match code.as_str() {
            "provider-503" => {
                return json_response("503 Service Unavailable", &json!({"error": "down"}))
            }
            "provider-429" => {
                return json_response("429 Too Many Requests", &json!({"error": "slow"}))
            }
            _ => {}
        }
        if !ok || !codes.lock().unwrap().contains_key(&code) {
            return json_response("400 Bad Request", &json!({"error": "invalid_grant"}));
        }
        return json_response(
            "200 OK",
            &json!({"access_token": format!("at-{code}"), "token_type": "Bearer",
                    "expires_in": 604800, "refresh_token": "rt-never-kept", "scope": "identify email"}),
        );
    }
    if line.starts_with("GET /api/users/@me ") {
        let code = authorization
            .as_deref()
            .and_then(|a| a.strip_prefix("Bearer at-"))
            .unwrap_or_default()
            .to_string();
        return match codes.lock().unwrap().get(&code) {
            Some(profile) => json_response("200 OK", profile),
            None => json_response("401 Unauthorized", &json!({"message": "401: Unauthorized"})),
        };
    }
    json_response("404 Not Found", &json!({}))
}

/// Nelly, as Discord describes her.
pub fn nelly() -> Value {
    json!({
        "id": "80351110224678912",
        "username": "nelly",
        "global_name": "Nelly",
        "avatar": "8342729096ea3675442027381ff50dfe",
        "email": "nelly@example.com",
        "verified": true
    })
}

// ── driving the routes, as a browser and a page would ──────────────────

pub async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Option<String>, Value) {
    let mut b = Request::builder().method(method).uri(path);
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let res = app
        .clone()
        .oneshot(
            b.body(match body {
                Some(v) => Body::from(v.to_string()),
                None => Body::empty(),
            })
            .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status();
    let location = res
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        location,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A query parameter of a URL, decoded.
pub fn param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| percent_decode(v))
}

/// The page's half of `begin`: `(state in the redirect, binder)`.
pub async fn begin(app: &axum::Router, method: &str) -> (String, String) {
    let (status, _, begun) = send(
        app,
        "POST",
        "/auth/begin",
        Some(json!({"method": method, "return_to": RETURN})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let redirect = begun["redirect"].as_str().expect("a redirect").to_string();
    assert!(
        redirect.contains("code_challenge_method=S256"),
        "{redirect}"
    );
    let state = param(&redirect, "state").expect("a state");
    let binder = begun["binder"].as_str().expect("a binder").to_string();
    assert!(
        !redirect.contains(&binder),
        "the binder never travels in a URL"
    );
    (state, binder)
}

/// The browser coming back from the provider: answers the `Location`.
pub async fn callback(app: &axum::Router, method: &str, state: &str, code: &str) -> String {
    let (status, location, body) = send(
        app,
        "GET",
        &format!(
            "/auth/callback/{method}?state={}&code={}",
            ordo_identity::federation::percent(state),
            ordo_identity::federation::percent(code)
        ),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    location.expect("a location")
}

/// The page redeeming the handoff.
pub async fn redeem(
    app: &axum::Router,
    method: &str,
    handoff: &str,
    binder: &str,
    extra: Value,
) -> (StatusCode, Value) {
    let mut body = json!({"method": method, "kind": "handoff", "handoff": handoff, "binder": binder,
                          "display": "a name the page made up"});
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    let (status, _, answer) = send(app, "POST", "/auth/complete", Some(body), None).await;
    (status, answer)
}

/// The whole sign-in: begin, the provider, the callback, the redemption.
pub async fn sign_in(
    app: &axum::Router,
    fake: &FakeDiscord,
    method: &str,
    code: &str,
    profile: Value,
) -> (StatusCode, Value) {
    let (state, binder) = begin(app, method).await;
    fake.authorize(code, profile);
    let location = callback(app, method, &state, code).await;
    assert!(location.starts_with(RETURN), "{location}");
    let Some(handoff) = param(&location, "handoff") else {
        panic!("no handoff: {location}");
    };
    redeem(app, method, &handoff, &binder, json!({})).await
}
