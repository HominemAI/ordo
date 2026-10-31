//! The console on the identity service (`plan-identity-service.md` 5.4,
//! step 6, session 1): a bearer token from the service on every console
//! call, verified with the service's published keys; calls to the door made
//! as the person, with their own token; a stale touch answered
//! `step_up_required` so the page touches at the service and runs the call
//! again; and the console's own sign-in and console key routes closed.
//!
//! The identity service is a fake issuer: an Ed25519 signer and a JWKS
//! route on a real socket. The door is the real check
//! (`ordo_host::IdentityAccess` with the identity keys and its home space)
//! in front of a stand-in that answers each op it allows with who called.
//!
//! Needs the conformance cluster for the console's own database
//! (`scripts/conformance-pg.sh start`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use ed25519_dalek::SigningKey;
use ordo_api::{AccessChecker, AccessDecision};
use ordo_console::config::{
    DATABASE_URL_ENV, DATA_DIR_ENV, DOOR_SPACE_ENV, DOOR_URL_ENV, MODE_ENV, ORIGIN_ENV,
    PAGE_DIR_ENV,
};
use ordo_console::identity::{ISSUER_ENV, URL_ENV};
use ordo_console::{BootOptions, Clock, Console, ConsoleConfig};
use ordo_exec::PoolConfig;
use ordo_host::identity_keys::{IdentityKeyStore, IdentityKeys, IdentitySettings, ServiceUrl};
use ordo_identity::deployment_token::{self, DeploymentClaims, Kind, ROLE_ADMIN};
use ordo_identity::software_authenticator::DEFAULT_ORIGIN;
use ordo_identity::{AccessEngine, Argon2idHasher, SpaceId, TokenKey};
use serde_json::{json, Value};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;
const ISS: &str = "https://console.example.com:8082";
const HOME: &str = "system";
const TENANT: &str = "t1";
const PAUSE: &str = "std.deployment.pauseTenant";
const SECRET: &[u8] = b"console-identity-test-secret";

// ════════════════════════════ the identity service ═══════════════════════

#[derive(Clone)]
struct Issuer {
    published: Arc<Mutex<Value>>,
    hits: Arc<AtomicUsize>,
    url: String,
    /// Every management call it took: method, path, bearer, body.
    calls: Calls,
    /// Answer `presence_required` to the next write, as the service does
    /// to a person whose touch is stale there.
    stale: Arc<std::sync::atomic::AtomicBool>,
}

/// The management calls the fake service took: method, path, bearer, body.
type Calls = Arc<Mutex<Vec<(String, String, String, Value)>>>;

/// What the fake service answers a management call.
fn service_answer(method: &str, path: &str, stale: bool) -> (StatusCode, Value) {
    if method == "POST" && stale {
        return (
            StatusCode::FORBIDDEN,
            json!({"error": "presence_required", "message": "touch your passkey"}),
        );
    }
    match (method, path) {
        ("GET", "/people") => (
            StatusCode::OK,
            json!({"people": [{"id": "p_ada", "displayName": "Ada", "role": "admin",
                "createdAt": START, "endedAt": null}]}),
        ),
        ("GET", "/invites") => (StatusCode::OK, json!({"invites": []})),
        ("GET", "/machine-keys") => (StatusCode::OK, json!({"machineKeys": []})),
        ("GET", p) if p.starts_with("/audit") => (
            StatusCode::OK,
            json!({"entries": [{"seq": 2, "action": "invite.issued"}],
                "verified": {"ok": true, "rows": 2, "checkpoints": 0}}),
        ),
        ("POST", "/invites") => (
            StatusCode::OK,
            json!({"id": "i_1", "link": "https://id.example.com/join#s3cret", "expiresAt": START + 60}),
        ),
        ("POST", "/people/p_last/end") => (
            StatusCode::CONFLICT,
            json!({"error": "last_admin", "message": "that is the last admin"}),
        ),
        ("POST", p) if p.starts_with("/people/") => (StatusCode::OK, json!({"ended": "p_eli"})),
        ("POST", "/machine-keys") => (
            StatusCode::OK,
            json!({"id": "m_1", "secret": "ordo-mk-v1:m_1:abc"}),
        ),
        ("POST", "/machine-keys/m_1/end") => {
            (StatusCode::OK, json!({"ended": "m_1", "sessions": 0}))
        }
        _ => (
            StatusCode::NOT_FOUND,
            json!({"error": "not_found", "message": path}),
        ),
    }
}

impl Issuer {
    async fn start(keys: &[&SigningKey]) -> Issuer {
        let published = Arc::new(Mutex::new(Value::Null));
        let hits = Arc::new(AtomicUsize::new(0));
        let (p, h) = (published.clone(), hits.clone());
        let calls: Calls = Arc::default();
        let stale = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (log, st) = (calls.clone(), stale.clone());
        let app = Router::new()
            .route(
                "/.well-known/jwks.json",
                get(move || {
                    let (p, h) = (p.clone(), h.clone());
                    async move {
                        h.fetch_add(1, Ordering::SeqCst);
                        (StatusCode::OK, p.lock().unwrap().to_string())
                    }
                }),
            )
            .fallback(move |req: Request<Body>| {
                let (log, st) = (log.clone(), st.clone());
                async move {
                    let method = req.method().to_string();
                    let path = req
                        .uri()
                        .path_and_query()
                        .map(|p| p.to_string())
                        .unwrap_or_default();
                    let bearer = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                        .await
                        .unwrap();
                    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock()
                        .unwrap()
                        .push((method.clone(), path.clone(), bearer, body));
                    let (s, v) = service_answer(&method, &path, st.swap(false, Ordering::SeqCst));
                    (s, Json(v))
                }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let i = Issuer {
            published,
            hits,
            url,
            calls,
            stale,
        };
        i.publish(keys);
        i
    }

    fn publish(&self, keys: &[&SigningKey]) {
        let keys: Vec<Value> = keys
            .iter()
            .map(|k| deployment_token::public_jwk(&kid(k), &k.verifying_key()))
            .collect();
        *self.published.lock().unwrap() = json!({ "keys": keys });
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn kid(k: &SigningKey) -> String {
    deployment_token::key_id(&k.verifying_key())
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn claims(kind: Kind, touched_at: Option<u64>) -> DeploymentClaims {
    let sub = match kind {
        Kind::Person => "p_ada",
        Kind::Machine => "m_ci",
        Kind::Breakglass => "breakglass:root",
    };
    DeploymentClaims {
        iss: ISS.into(),
        aud: deployment_token::audience("main"),
        sub: sub.into(),
        kind,
        role: ROLE_ADMIN.into(),
        client: "console".into(),
        sid: "s_1".into(),
        touched_at,
        iat: START,
        exp: START + 600,
        jti: "j_1".into(),
    }
}

fn sign(c: &DeploymentClaims, k: &SigningKey) -> String {
    deployment_token::sign(c, &kid(k), k)
}

// ════════════════════════════ the deployment ═════════════════════════════

/// The door: the real check, then an answer naming who called.
struct Door {
    access: ordo_host::IdentityAccess,
    keys: Arc<IdentityKeys>,
    /// Every bearer an op call carried, in order.
    bearers: Mutex<Vec<String>>,
}

fn door_error(status: StatusCode, code: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": code}})),
    )
        .into_response()
}

async fn op(State(d): State<Arc<Door>>, Path(name): Path<String>, headers: HeaderMap) -> Response {
    let header = |k: &str| {
        headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let bearer = header("authorization")
        .strip_prefix("Bearer ")
        .unwrap_or_default()
        .to_string();
    d.bearers.lock().unwrap().push(bearer.clone());
    let space = header("x-ordo-space");
    d.keys.prepare(&bearer).await;
    match d.access.check(&bearer, &space, &name) {
        AccessDecision::Allow { principal, .. } => {
            Json(json!({"rows": [{"op": name, "space": space, "principal": principal}]}))
                .into_response()
        }
        AccessDecision::Deny if d.access.presence_required(&bearer, &space, &name) => {
            door_error(StatusCode::FORBIDDEN, "presence_required")
        }
        AccessDecision::Deny => door_error(StatusCode::FORBIDDEN, "access_denied"),
    }
}

async fn serve_door(issuer: &Issuer, clock: Arc<AtomicU64>) -> (Arc<Door>, String) {
    let mut engine = AccessEngine::new();
    engine.require_step_up(PAUSE);
    let _ = SpaceId(HOME.into());
    let read = clock.clone();
    let keys = Arc::new(IdentityKeys::new(
        IdentitySettings {
            url: ServiceUrl::parse(&issuer.url, false).unwrap(),
            issuer: ISS.into(),
            audience: deployment_token::audience("main"),
        },
        IdentityKeyStore::memory(),
        Arc::new(move || read.load(Ordering::SeqCst)),
    ));
    keys.boot().await.expect("door keys");
    let read = clock.clone();
    let access = ordo_host::IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(move || read.load(Ordering::SeqCst)),
        Arc::new(ordo_host::metrics::Metrics::new()),
    )
    .with_console_home(HOME)
    .with_identity_keys(keys.clone());
    let door = Arc::new(Door {
        access,
        keys,
        bearers: Mutex::new(Vec::new()),
    });
    let app = Router::new()
        .route("/op/{name}", any(op))
        .route(
            "/health/ready",
            get(|| async { Json(json!({"status": "ready", "quarantined": []})) }),
        )
        .route(
            "/health/live",
            get(|| async { Json(json!({"status": "live"})) }),
        )
        .with_state(door.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (door, url)
}

// ═════════════════════════════ the console ═══════════════════════════════

fn url_for(db: &str) -> String {
    let c = PoolConfig::from_env();
    let mut url = format!(
        "host={} port={} user={} dbname={db}",
        c.host, c.port, c.user
    );
    if let Some(p) = &c.password {
        url.push_str(&format!(" password={p}"));
    }
    url
}

struct H {
    console: Console,
    router: Router,
    door: Arc<Door>,
    issuer: Issuer,
    clock: Clock,
    door_clock: Arc<AtomicU64>,
    key: SigningKey,
    /// The settings the console booted with, to boot it again.
    vars: HashMap<String, String>,
}

struct Answer {
    status: StatusCode,
    body: Value,
    headers: HeaderMap,
}

async fn boot(name: &str) -> H {
    let db = format!("ordo_console_t_id_{name}");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let dir = std::env::temp_dir().join(format!(
        "ordo-console-identity-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let k = key(1);
    let issuer = Issuer::start(&[&k]).await;
    let door_clock = Arc::new(AtomicU64::new(START));
    let (door, door_url) = serve_door(&issuer, door_clock.clone()).await;
    let vars = HashMap::from([
        (DATABASE_URL_ENV.to_string(), url_for(&db)),
        (ORIGIN_ENV.to_string(), DEFAULT_ORIGIN.to_string()),
        ("ORDO_CONSOLE_RP_ID".to_string(), "example.com".to_string()),
        (DATA_DIR_ENV.to_string(), dir.display().to_string()),
        (
            PAGE_DIR_ENV.to_string(),
            dir.join("page").display().to_string(),
        ),
        (MODE_ENV.to_string(), "door-only".to_string()),
        (DOOR_URL_ENV.to_string(), door_url),
        (DOOR_SPACE_ENV.to_string(), HOME.to_string()),
        (URL_ENV.to_string(), issuer.url.clone()),
        (ISSUER_ENV.to_string(), ISS.to_string()),
    ]);
    let cfg = ConsoleConfig::from_vars(&|k| vars.get(k).cloned()).expect("config");
    let clock = Clock::manual(START);
    let console = Console::boot(
        cfg,
        BootOptions {
            clock: clock.clone(),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("boot");
    H {
        router: console.router(),
        console,
        door,
        issuer,
        clock,
        door_clock,
        key: k,
        vars,
    }
}

impl H {
    async fn send(&self, req: Request<Body>) -> Answer {
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        Answer {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            headers,
        }
    }

    async fn get(&self, path: &str, token: Option<&str>) -> Answer {
        let mut req = Request::get(format!("/d/main/api/{path}"));
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        self.send(req.body(Body::empty()).unwrap()).await
    }

    async fn post_from(
        &self,
        origin: &str,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> Answer {
        let mut req = Request::post(format!("/d/main/api/{path}"))
            .header("origin", origin)
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        self.send(req.body(Body::from(body.to_string())).unwrap())
            .await
    }

    async fn post(&self, path: &str, token: Option<&str>, body: Value) -> Answer {
        self.post_from(DEFAULT_ORIGIN, path, token, body).await
    }

    fn person(&self, touched_ago: Option<u64>) -> String {
        sign(
            &claims(Kind::Person, touched_ago.map(|a| START - a)),
            &self.key,
        )
    }

    fn bearers(&self) -> Vec<String> {
        self.door.bearers.lock().unwrap().clone()
    }

    async fn actors(&self, action: &str) -> Vec<(String, String)> {
        self.console
            .store()
            .audit_page("main", None, Some(action), 20)
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.actor, r.outcome))
            .collect()
    }
}

// ═════════════════════════════════ cases ═════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn the_page_learns_where_to_sign_in_and_may_call_the_service() {
    let h = boot("status").await;
    let a = h.get("status", None).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["identity"]["origin"], ISS);
    assert_eq!(a.body["identity"]["audience"], "ordo-deployment:main");
    let csp = a.headers["content-security-policy"].to_str().unwrap();
    assert!(csp.contains(&format!("connect-src 'self' {ISS}")), "{csp}");
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    // The keys were fetched once, at boot.
    assert_eq!(
        h.issuer.hits(),
        2,
        "one fetch by the door, one by the console"
    );
}

/// Step 7 (board `upgrading-with-an-old-compose-file`, ask 1): with no
/// address of its own the identity service is reached under the console's
/// origin, at `ORDO_IDENTITY_BASE_PATH`. The page is sent there; the
/// console passes each request through as it came, with the visitor's
/// address added to `X-Forwarded-For`, and the service's answer back with
/// its own headers, not the console's.
#[tokio::test(flavor = "multi_thread")]
async fn under_the_consoles_origin_the_service_is_passed_through() {
    use ordo_console::identity::BASE_PATH_ENV;
    // A service that answers with what it was sent.
    let app = Router::new().fallback(|req: Request<Body>| async move {
        let h = |k: &str| {
            req.headers()
                .get(k)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string()
        };
        let seen = json!({
            "method": req.method().as_str(),
            "path": req.uri().path_and_query().map(|p| p.as_str()).unwrap_or(""),
            "cookie": h("cookie"),
            "origin": h("origin"),
            "forwardedFor": h("x-forwarded-for"),
            "host": h("host"),
        });
        (
            StatusCode::CREATED,
            [
                ("set-cookie", "ordo-identity=s1; Path=/identity; HttpOnly"),
                ("content-security-policy", "default-src 'none'"),
            ],
            Json(seen),
        )
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let svc_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let h = boot("passthrough").await;
    let mut vars = h.vars.clone();
    vars.insert(BASE_PATH_ENV.to_string(), "/identity".to_string());
    vars.insert(ISSUER_ENV.to_string(), DEFAULT_ORIGIN.to_string());
    let cfg = ConsoleConfig::from_vars(&|k| vars.get(k).cloned()).expect("config");
    let console = Console::boot(
        cfg,
        BootOptions {
            clock: h.clock.clone(),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("boot");
    let router = console.router();

    let status = router
        .clone()
        .oneshot(
            Request::get("/d/main/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(status.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        v["identity"]["origin"],
        format!("{DEFAULT_ORIGIN}/identity"),
        "the page sends people under the console's origin"
    );

    // The JWKS fetch went to the issuer stand-in; the passthrough goes to
    // ORDO_IDENTITY_URL, here the echoing one.
    drop(console);
    vars.insert(URL_ENV.to_string(), svc_url);
    let cfg = ConsoleConfig::from_vars(&|k| vars.get(k).cloned()).expect("config");
    let console = Console::boot(
        cfg,
        BootOptions {
            clock: h.clock.clone(),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("boot with an unanswering key set is still a boot");
    let mut req = Request::post("/identity/sign-in/begin?x=1")
        .header("cookie", "ordo-identity=s0")
        .header("origin", DEFAULT_ORIGIN)
        .header("x-forwarded-for", "203.0.113.9")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    req.extensions_mut().insert(axum::extract::ConnectInfo(
        "198.51.100.7:40000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let res = console.router().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let headers = res.headers().clone();
    assert_eq!(
        headers["set-cookie"],
        "ordo-identity=s1; Path=/identity; HttpOnly"
    );
    assert_eq!(
        headers["content-security-policy"], "default-src 'none'",
        "the service's own policy, not the console's"
    );
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let seen: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(seen["method"], "POST");
    assert_eq!(seen["path"], "/identity/sign-in/begin?x=1");
    assert_eq!(seen["cookie"], "ordo-identity=s0");
    assert_eq!(seen["origin"], DEFAULT_ORIGIN);
    assert_eq!(seen["forwardedFor"], "203.0.113.9, 198.51.100.7");
    // Outside the base path, nothing is passed through.
    let res = console
        .router()
        .oneshot(Request::get("/identityx").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_with_a_token_is_signed_in_and_nobody_else_is() {
    let h = boot("session").await;
    let a = h.get("session", None).await;
    assert_eq!(
        (a.status, a.body["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("not_signed_in"))
    );
    let t = h.person(Some(10));
    let a = h.get("session", Some(&t)).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["person"], "p_ada");
    assert_eq!(a.body["mustEnrol"], false);
    assert_eq!(a.body["endsAt"], START + 600);
    assert_eq!(a.body["touchFreshUntil"], START - 10 + 300);

    // A machine key's token is a valid token, and still not for the console.
    let m = sign(&claims(Kind::Machine, None), &h.key);
    let a = h.get("session", Some(&m)).await;
    assert_eq!(
        (a.status, a.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("person_only"))
    );
    // Nor is a break-glass token, which is for the host's own CLI (plan
    // 5.8), even with a fresh touch.
    let b = sign(&claims(Kind::Breakglass, Some(START)), &h.key);
    for path in ["session", "runtime"] {
        let a = h.get(path, Some(&b)).await;
        assert_eq!(
            (a.status, a.body["error"].as_str()),
            (StatusCode::FORBIDDEN, Some("person_only")),
            "{path}"
        );
    }
    // A console cookie means nothing here.
    let req = Request::get("/d/main/api/session")
        .header("cookie", "ordo-console=anything")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_bad_token_is_refused_and_names_the_setting_to_fix() {
    let h = boot("refusals").await;
    let refused = |a: &Answer| {
        assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{}", a.body);
        assert_eq!(a.body["error"], "token_refused", "{}", a.body);
        a.body["setting"].as_str().map(str::to_string)
    };

    let mut c = claims(Kind::Person, Some(START));
    c.aud = deployment_token::audience("other");
    let a = h.get("session", Some(&sign(&c, &h.key))).await;
    assert_eq!(refused(&a).as_deref(), Some("ORDO_CONSOLE_DEPLOYMENT"));

    let mut c = claims(Kind::Person, Some(START));
    c.iss = "https://elsewhere.example.com".into();
    let a = h.get("session", Some(&sign(&c, &h.key))).await;
    assert_eq!(refused(&a).as_deref(), Some(ISSUER_ENV));

    // Signed by a key the service never published.
    let forged = sign(&claims(Kind::Person, Some(START)), &key(66));
    let a = h.get("session", Some(&forged)).await;
    assert_eq!(refused(&a), None);

    // Not a deployment token at all.
    let a = h.get("session", Some("abc.def")).await;
    assert_eq!(refused(&a), None);

    // Expired: the page asks the service for a new one; nothing to set.
    let t = h.person(Some(0));
    h.clock.advance(600);
    let a = h.get("session", Some(&t)).await;
    assert_eq!(refused(&a), None);
    assert!(a.body["message"].as_str().unwrap().contains("expired"));

    // None of these reached the door.
    assert!(h.bearers().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_key_is_fetched_once_and_a_flood_of_them_fetches_no_more() {
    let h = boot("rotation").await;
    let before = h.issuer.hits();
    // The service rotates: a token under the new key verifies after one
    // fetch.
    let k2 = key(2);
    h.issuer.publish(&[&h.key, &k2]);
    let t = sign(&claims(Kind::Person, Some(START)), &k2);
    assert_eq!(h.get("session", Some(&t)).await.status, StatusCode::OK);
    assert_eq!(h.issuer.hits(), before + 1);
    // A minute on, a forged kid causes one fetch, and the ones after it
    // inside the next minute cause none.
    h.clock.advance(61);
    for seed in 70..75 {
        let forged = sign(&claims(Kind::Person, Some(START)), &key(seed));
        assert_eq!(
            h.get("session", Some(&forged)).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(h.issuer.hits(), before + 2, "one for the first forged kid");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_console_calls_the_door_as_the_person_with_their_own_token() {
    let h = boot("as_person").await;
    let t = h.person(Some(10));
    let a = h.get("tenants", Some(&t)).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["tenants"][0]["principal"], "p_ada");
    assert_eq!(a.body["tenants"][0]["space"], HOME);
    assert_eq!(
        h.bearers(),
        vec![t.clone()],
        "the person's token, as it came"
    );
    // The Deployment screen's check: no link, the door answers.
    let a = h.get("door", Some(&t)).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["linked"], false);
    assert_eq!(a.body["reachable"], true);
    assert_eq!(a.body["via"], "identity-service");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_touch_asks_for_a_step_up_and_a_fresh_one_goes_through() {
    let h = boot("step_up").await;
    // The console's own gate: a write with a touch older than 300 seconds
    // never reaches the door.
    let stale = h.person(Some(301));
    let a = h
        .post("tenants/pause", Some(&stale), json!({"tenant": TENANT}))
        .await;
    assert_eq!(
        (a.status, a.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("step_up_required")),
        "{}",
        a.body
    );
    assert_eq!(a.body["windowSecs"], 300);
    assert!(h.bearers().is_empty());
    // No touch at all is stale too.
    let never = h.person(None);
    let a = h
        .post("tenants/pause", Some(&never), json!({"tenant": TENANT}))
        .await;
    assert_eq!(a.body["error"], "step_up_required");
    // A touch "from the future" past the leeway is no touch.
    let ahead = sign(&claims(Kind::Person, Some(START + 600)), &h.key);
    let a = h
        .post("tenants/pause", Some(&ahead), json!({"tenant": TENANT}))
        .await;
    assert_eq!(a.body["error"], "step_up_required");

    // After a touch at the service, the same call goes through as them.
    let fresh = h.person(Some(5));
    let a = h
        .post("tenants/pause", Some(&fresh), json!({"tenant": TENANT}))
        .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["paused"]["principal"], "p_ada");
    assert_eq!(h.bearers(), vec![fresh]);
    let acts = h.actors("tenants.pause").await;
    assert_eq!(acts[0], ("person:p_ada".into(), "ok".into()));
    assert!(acts[1..]
        .iter()
        .all(|(who, outcome)| who == "person:p_ada" && outcome == "refused:step_up_required"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_doors_presence_required_comes_back_as_the_same_step_up() {
    let h = boot("door_presence").await;
    // A tenant read asks no touch of the console, and the door asks one of
    // any deployment caller in a tenant: the person's 301 second old touch
    // is refused there, and the page hears the one step-up word.
    let stale = h.person(Some(301));
    let a = h
        .get(&format!("people?tenant={TENANT}"), Some(&stale))
        .await;
    assert_eq!(
        (a.status, a.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("step_up_required")),
        "{}",
        a.body
    );
    assert_eq!(h.bearers(), vec![stale]);
    let fresh = h.person(Some(5));
    let a = h
        .get(&format!("people?tenant={TENANT}"), Some(&fresh))
        .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["members"][0]["space"], TENANT);
    // The door's own clock moves too: a touch the console still calls fresh
    // but the door does not is refused there, never let through.
    h.door_clock.store(START + 400, Ordering::SeqCst);
    let a = h
        .get(&format!("people?tenant={TENANT}"), Some(&fresh))
        .await;
    assert_eq!(a.body["error"], "step_up_required", "{}", a.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_with_a_good_token_from_another_origin_is_refused() {
    let h = boot("origin").await;
    let t = h.person(Some(5));
    let a = h
        .post_from(
            "https://evil.example",
            "tenants/pause",
            Some(&t),
            json!({"tenant": TENANT}),
        )
        .await;
    assert_eq!(
        (a.status, a.body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("origin_refused"))
    );
    assert!(h.bearers().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_consoles_own_sign_in_and_console_key_routes_are_closed() {
    let h = boot("closed").await;
    let t = h.person(Some(5));
    for path in [
        "setup/begin",
        "signin/begin",
        "stepup/begin",
        "recovery",
        "passkeys/begin",
        "recovery-codes",
        "owners/invite",
        "owners/remove",
        "signout",
        "door/link/begin",
        "door/stepup/begin",
        "door/rotate",
        "door/unlink",
        "door/keys/revoke",
        "door/forget",
    ] {
        let a = h.post(path, Some(&t), json!({})).await;
        assert_eq!(
            (a.status, a.body["error"].as_str()),
            (StatusCode::GONE, Some("moved_to_identity_service")),
            "{path}: {}",
            a.body
        );
        assert_eq!(a.body["setting"], URL_ENV, "{path}");
        assert!(a.body["message"].as_str().unwrap().contains(ISS), "{path}");
    }
    for path in ["owners", "door/keys"] {
        let a = h.get(path, Some(&t)).await;
        assert_eq!(a.status, StatusCode::GONE, "{path}");
    }
    // No setup code is waiting either: the first admin comes from the
    // identity service, never from this console.
    assert!(!h.console.setup_code_path().exists());
    assert!(h.bearers().is_empty());
}

/// A console with `ORDO_IDENTITY_URL` set refuses to boot while it holds a
/// console key for the door (G14), naming the setting and the command
/// that forgets it; the command forgets it, audited as its unix user, and
/// the console boots.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_on_the_identity_service_will_not_boot_holding_a_console_key() {
    let h = boot("held_key").await;
    let store = h.console.store().clone();
    store
        .put_credential(
            "main",
            &ordo_console::store::CredentialRow {
                key_id: "ck-old".into(),
                name: "before the upgrade".into(),
                space: HOME.into(),
                sealed: "00".into(),
                created_at: START,
                rotated_at: None,
            },
            START,
        )
        .await
        .unwrap();
    let again = || async {
        let cfg = ConsoleConfig::from_vars(&|k| h.vars.get(k).cloned()).expect("config");
        Console::boot(
            cfg,
            BootOptions {
                clock: h.clock.clone(),
                hasher: Arc::new(Argon2idHasher::weak_for_tests()),
            },
        )
        .await
    };
    let refused = again().await.err().expect("refused").to_string();
    assert!(refused.contains(URL_ENV), "{refused}");
    assert!(refused.contains("forget-door-key"), "{refused}");
    assert!(refused.contains("ck-old"), "{refused}");

    let db = h.console.config().database.clone();
    // The delete and its audit row are one transaction: with the audit
    // write failing, the key is still held and nothing is half done.
    store
        .pool()
        .rows("ALTER TABLE audit RENAME TO audit_away")
        .await
        .unwrap();
    let failed = ordo_console::recover::forget_door_key(&db, "main", "ada", &h.clock).await;
    assert!(failed.is_err(), "{failed:?}");
    store
        .pool()
        .rows("ALTER TABLE audit_away RENAME TO audit")
        .await
        .unwrap();
    assert!(store.credential("main").await.unwrap().is_some());
    let got = ordo_console::recover::forget_door_key(&db, "main", "ada", &h.clock)
        .await
        .unwrap();
    assert_eq!(got, ordo_console::recover::Forgotten::Key("ck-old".into()));
    let audit = store
        .audit_page("main", None, Some("console.forget-door-key"), 10)
        .await
        .unwrap();
    assert_eq!(
        (
            audit[0].actor.as_str(),
            audit[0].outcome.as_str(),
            audit[0].detail.as_str()
        ),
        ("unix:ada", "ok", "key=ck-old")
    );
    let booted = again().await.expect("boots once the key is forgotten");
    booted.shutdown().await;
}

/// The one image's upgrade (`plan-identity-service.md` 3.1, step 7): the
/// supervisor runs the `ordo-console forget-door-key` binary as the
/// console, with the console's own settings and `USER=ordo-supervisor`,
/// before the console's first start. It forgets the held key, the
/// console's audit names the supervisor, and the console then boots.
#[tokio::test(flavor = "multi_thread")]
async fn the_supervisor_forgets_a_held_console_key_with_the_console_binary() {
    let h = boot("held_key_bin").await;
    h.console
        .store()
        .put_credential(
            "main",
            &ordo_console::store::CredentialRow {
                key_id: "ck-before".into(),
                name: "before the upgrade".into(),
                space: HOME.into(),
                sealed: "00".into(),
                created_at: START,
                rotated_at: None,
            },
            START,
        )
        .await
        .unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_ordo-console"))
            .arg("forget-door-key")
            .env_clear()
            .envs(&h.vars)
            .env("USER", "ordo-supervisor")
            .output()
            .unwrap()
    };
    let out = run();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("ck-before"));
    // A second run finds nothing, and says so: the supervisor runs it at
    // every boot.
    let again = run();
    assert!(again.status.success());
    let audit = h
        .console
        .store()
        .audit_page("main", None, Some("console.forget-door-key"), 10)
        .await
        .unwrap();
    let rows: Vec<(&str, &str, &str)> = audit
        .iter()
        .map(|r| (r.actor.as_str(), r.outcome.as_str(), r.detail.as_str()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("unix:ordo-supervisor", "nothing", ""),
            ("unix:ordo-supervisor", "ok", "key=ck-before"),
        ]
    );
    let cfg = ConsoleConfig::from_vars(&|k| h.vars.get(k).cloned()).expect("config");
    let booted = Console::boot(
        cfg,
        BootOptions {
            clock: h.clock.clone(),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("boots once the key is forgotten");
    booted.shutdown().await;
}

/// The Deployment people screen (plan 5.6, 5.7, 7): the console relays a
/// person's management calls to the identity service with their own token,
/// a write needs a fresh touch at the console and at the service, the
/// service's refusals come back in its own words, and each write is in
/// the console's audit log under the person, with no secret in it.
#[tokio::test(flavor = "multi_thread")]
async fn deployment_people_are_managed_at_the_identity_service_as_the_person() {
    let h = boot("admins").await;
    let fresh = h.person(Some(10));
    let a = h.get("admins", Some(&fresh)).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["people"][0]["id"], "p_ada");
    assert_eq!(a.body["machineKeys"], json!([]));
    let a = h.get("admins/audit?limit=5", Some(&fresh)).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["verified"]["ok"], true);
    let a = h.get("admins/audit?limit=x", Some(&fresh)).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);
    {
        let calls = h.issuer.calls.lock().unwrap();
        assert!(calls
            .iter()
            .all(|(_, _, bearer, _)| *bearer == format!("Bearer {fresh}")));
        assert!(calls.iter().any(|(_, p, _, _)| p == "/audit?limit=5"));
    }

    // A write with a stale touch is asked for one before the service.
    let stale = h.person(Some(400));
    let before = h.issuer.calls.lock().unwrap().len();
    let a = h
        .post("admins/invite", Some(&stale), json!({"label": "Eli"}))
        .await;
    assert_eq!(a.body["error"], "step_up_required", "{}", a.body);
    assert_eq!(h.issuer.calls.lock().unwrap().len(), before);

    // Fresh: the invite is made there, as the person, as an admin.
    let a = h
        .post("admins/invite", Some(&fresh), json!({"label": "Eli"}))
        .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["link"], "https://id.example.com/join#s3cret");
    let (_, path, _, body) = h.issuer.calls.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        (path.as_str(), body),
        ("/invites", json!({"role": "admin", "label": "Eli"}))
    );

    // The service's own `presence_required` is the same step-up.
    h.issuer.stale.store(true, Ordering::SeqCst);
    let a = h
        .post("admins/end", Some(&fresh), json!({"id": "p_eli"}))
        .await;
    assert_eq!(a.body["error"], "step_up_required", "{}", a.body);
    let a = h
        .post("admins/end", Some(&fresh), json!({"id": "p_eli"}))
        .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let a = h
        .post("admins/end", Some(&fresh), json!({"id": "p_last"}))
        .await;
    assert_eq!(
        (
            a.status,
            a.body["error"].as_str(),
            a.body["identityError"].as_str()
        ),
        (
            StatusCode::CONFLICT,
            Some("identity_refused"),
            Some("last_admin")
        )
    );
    let a = h
        .post("admins/end", Some(&fresh), json!({"id": "../x"}))
        .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);

    let a = h
        .post(
            "admins/machine-keys/add",
            Some(&fresh),
            json!({"name": "ci-main"}),
        )
        .await;
    assert_eq!(a.body["secret"], "ordo-mk-v1:m_1:abc", "{}", a.body);
    let a = h
        .post(
            "admins/machine-keys/end",
            Some(&fresh),
            json!({"id": "m_1"}),
        )
        .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);

    // The console's own audit: the person, and never a secret.
    let rows = h
        .console
        .store()
        .audit_page("main", None, Some("admins."), 50)
        .await
        .unwrap();
    assert!(rows.iter().all(|r| r.actor == "person:p_ada"), "{rows:?}");
    assert!(rows
        .iter()
        .all(|r| !r.detail.contains("s3cret") && !r.detail.contains("ordo-mk-v1")));
    assert!(rows
        .iter()
        .any(|r| r.action == "admins.end" && r.outcome == "refused:identity_refused"));
    assert!(rows
        .iter()
        .any(|r| r.action == "admins.invite" && r.detail == "invite=i_1"));
    // A machine key's token does not reach it.
    let m = sign(&claims(Kind::Machine, None), &h.key);
    assert_eq!(h.get("admins", Some(&m)).await.body["error"], "person_only");
}
