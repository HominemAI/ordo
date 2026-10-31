//! Step 3a, the console's side of the link (`plan-console.md` 8): linking
//! by sign-in, the console key, the owner's touch relayed, rotation and
//! unlink, all through the console's one client to the door. The claim
//! step was removed (owner, 2026-10-30); a console without the identity
//! service links by sign-in, as an owner the deployment named.
//!
//! The door here is the deployment's own: `ordo_host`'s sign-in routes and
//! console key ops, served in-process on a loopback port, over memory
//! stores. The one stand-in is admission: the principal the test names
//! is the deployment's owner, as `ORDO_DEPLOYMENT_OWNERS` makes one at a
//! real deployment.
//!
//! Needs the conformance cluster for the console's own database
//! (`scripts/conformance-pg.sh start`).

mod support {
    pub mod link_owner;
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::{Json, Router};
use ordo_api::{AccessChecker, AccessDecision, BoundParams};
use ordo_console::config::{
    DATABASE_URL_ENV, DATA_DIR_ENV, DOOR_SPACE_ENV, DOOR_URL_ENV, MODE_ENV, ORIGIN_ENV,
    PAGE_DIR_ENV,
};
use ordo_console::{BootOptions, Clock, Console, ConsoleConfig};
use ordo_exec::PoolConfig;
use ordo_host::auth::AuthState;
use ordo_host::console_keys::{ConsoleKeyOps, ConsoleKeyStore, ConsoleKeys, ADD, LIST, REVOKE};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey, DEFAULT_ORIGIN};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{
    AccessEngine, AccessProfile, Argon2idHasher, CompleteRequest, MemorySessionRevocations,
    PasskeyProvider, PrincipalId, ProviderRegistry, RelyingParty, Role, SpaceId, TokenKey,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;
const HOME: &str = "home";
const SECRET: &[u8] = b"door-link-test-secret";
const HEALTH_DEEP: &str = "platform.health.deep";
const APPLY: &str = ordo_host::apply_op::APPLY;
use ordo_host::deploy_ops::{ACTIVATE, PLAN, VERSIONS};

/// Where the console and the deployment sit. The default is one site, the
/// passkey library's test origin; the cross-site rehearsal puts them on two.
#[derive(Clone)]
struct Site {
    /// The console's own origin, where the page runs every ceremony.
    console_origin: String,
    /// The console's own relying party.
    console_rp: String,
    /// The deployment's relying party.
    door_rp: String,
    /// Origins the deployment takes on its own domain.
    door_origins: Vec<String>,
    /// Origins on other domains (Related Origin Requests).
    door_related: Vec<String>,
    /// The hostname the console reaches the door by.
    door_host: String,
}

impl Default for Site {
    fn default() -> Site {
        Site {
            console_origin: DEFAULT_ORIGIN.into(),
            console_rp: "example.com".into(),
            door_rp: "example.com".into(),
            door_origins: vec![DEFAULT_ORIGIN.into()],
            door_related: Vec::new(),
            door_host: "127.0.0.1".into(),
        }
    }
}

// ════════════════════════════ the deployment ═════════════════════════════

struct Door {
    auth: Arc<AuthState>,
    ops: ConsoleKeyOps,
    /// The deploy ops (step 4), over a database of the test's own.
    deploys: Arc<ordo_host::apply_op::ApplyOps>,
    /// One node, which serves whatever is active when asked.
    node: Arc<ordo_host::node_status::NodeStatus>,
    revocations: Arc<MemorySessionRevocations>,
    owner: Mutex<Option<PrincipalId>>,
    clock: Arc<AtomicU64>,
    auth_router: Router,
}

impl Door {
    fn access(&self) -> IdentityAccessHolder {
        let space = SpaceId(HOME.into());
        let mut engine = AccessEngine::new();
        engine.set_profile(
            &space,
            AccessProfile::new()
                .allow(Role::Owner, ADD)
                .allow(Role::Owner, LIST)
                .allow(Role::Owner, REVOKE)
                .allow(Role::Owner, HEALTH_DEEP)
                .allow(Role::Owner, APPLY)
                .allow(Role::Owner, VERSIONS)
                .allow(Role::Owner, ACTIVATE)
                .allow(Role::Owner, PLAN),
        );
        if let Some(o) = self.owner.lock().unwrap().clone() {
            engine.set_membership(&space, &o, Role::Owner);
        }
        // The deployment's default: every write needs a touch.
        engine.require_step_up(ADD);
        engine.require_step_up(REVOKE);
        for change in [APPLY, ACTIVATE, PLAN] {
            engine.require_step_up(change);
        }
        let at = self.clock.load(Ordering::SeqCst);
        IdentityAccessHolder(
            ordo_host::IdentityAccess::new(
                engine,
                TokenKey::from_secret(SECRET),
                Box::new(move || at),
                Arc::new(ordo_host::metrics::Metrics::new()),
            )
            .with_revocations(self.revocations.clone())
            .with_console_home(HOME),
        )
    }
}

struct IdentityAccessHolder(ordo_host::IdentityAccess);

fn door_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// `/op/{name}`: the door's check, then the console key ops.
async fn op(
    State(d): State<Arc<Door>>,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    let space = headers
        .get("x-ordo-space")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let principal = match d.access().0.check(&bearer, &space, &name) {
        AccessDecision::Allow { principal, .. } => principal,
        _ => return door_error(StatusCode::FORBIDDEN, "forbidden", "not allowed"),
    };
    let params: serde_json::Map<String, Value> = if body.is_empty() {
        query.into_iter().map(|(k, v)| (k, json!(v))).collect()
    } else {
        serde_json::from_slice(&body).unwrap_or_default()
    };
    let bound = BoundParams(params);
    let done = if name == APPLY {
        d.deploys.apply(&space, &bound, &principal).await
    } else if ordo_host::deploy_ops::OPS.contains(&name.as_str()) {
        ordo_host::deploy_ops::execute(&d.deploys, &name, &space, &bound, &principal).await
    } else {
        d.ops.execute(&name, &space, &bound, &principal).await
    };
    match done {
        Ok(r) => Json(json!({"rows": r.rows})).into_response(),
        Err(e) => door_error(StatusCode::CONFLICT, e.code.as_str(), &e.message),
    }
}

/// `/health/deep`: as the deployment answers it, to its owner only.
async fn health_deep(State(d): State<Arc<Door>>, headers: HeaderMap) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    match d.access().0.check(&bearer, HOME, HEALTH_DEEP) {
        AccessDecision::Allow { .. } => {
            Json(json!({"status": "ok", "boot": {"revision": "7"}})).into_response()
        }
        _ => door_error(StatusCode::FORBIDDEN, "access_denied", "not permitted"),
    }
}

/// `/auth/complete`: the deployment's own, plus the claim's admission.
async fn complete(State(d): State<Arc<Door>>, Json(body): Json<Value>) -> Response {
    let res = d
        .auth_router
        .clone()
        .oneshot(
            Request::post("/auth/complete")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut answer: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if status == StatusCode::OK && body.get("space").is_some() {
        let token = answer["token"].as_str().unwrap_or_default();
        let at = d.clock.load(Ordering::SeqCst);
        let who = ordo_identity::verify(
            &TokenKey::from_secret(SECRET),
            token,
            ordo_identity::Timestamp(at),
        )
        .unwrap()
        .principal;
        let owner = d.owner.lock().unwrap();
        let outcome = if owner.as_ref() == Some(&who) {
            "already-member"
        } else {
            "refused"
        };
        answer["admission"] = json!({"space": HOME, "outcome": outcome});
    }
    (status, Json(answer)).into_response()
}

/// `/health/deploy`, as the deployment answers it, with its one node
/// serving whatever is active.
async fn health_deploy(State(d): State<Arc<Door>>) -> Response {
    let pool = &d.deploys.pool;
    let active = ordo_exec::registry::active_version(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    d.node.serving(&active).await;
    d.node.swept(&active, Vec::new(), false, 0).await;
    let nodes = ordo_exec::nodes::live(pool).await.unwrap_or_default();
    Json(ordo_host::serve::deploy_status(&active, &nodes)).into_response()
}

async fn serve_door(site: &Site, name: &str) -> (Arc<Door>, String) {
    // The app's own database, which the deploy ops apply to.
    let app_db = format!("ordo_console_t_{name}_app");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &app_db).await.expect("drop");
    ordo_exec::create_database(&c, &app_db)
        .await
        .expect("create");
    let app = ordo_exec::Pool::connect(PoolConfig {
        db: app_db,
        // An apply holds one connection for its lock and works on another.
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("app pool");
    let deploys = ordo_host::apply_op::ApplyOps::new(
        app.clone(),
        Arc::new(ordo_exec::route::SinglePool::new(app.clone())),
        HOME,
    );
    let node = ordo_host::node_status::NodeStatus::new(
        app,
        "node-1".into(),
        std::time::Duration::from_secs(5),
    );
    let origins: Vec<&str> = site.door_origins.iter().map(String::as_str).collect();
    let mut rp = RelyingParty::new(&site.door_rp, &origins).unwrap();
    if !site.door_related.is_empty() {
        let related: Vec<&str> = site.door_related.iter().map(String::as_str).collect();
        rp = rp.with_related_origins(&related).unwrap();
    }
    let ceremonies = Arc::new(ordo_identity::MemoryCeremonyStore::new());
    let passkey = Arc::new(PasskeyProvider::with_stores(
        rp,
        Arc::new(ordo_identity::MemoryPasskeyStore::new()),
        ceremonies.clone(),
    ));
    let mut registry = ProviderRegistry::new();
    registry
        .register("passkey", Box::new(passkey.clone()))
        .unwrap();
    let revocations = Arc::new(MemorySessionRevocations::new());
    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let mut state = AuthState::new(
        registry,
        Directory::with_store(
            Provisioning::AutoOnboard,
            Arc::new(ordo_identity::MemoryDirectoryStore::new()),
        ),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(recorder.clone())
    .with_passkey(passkey);
    let clock = Arc::new(AtomicU64::new(START));
    let read = clock.clone();
    state.now = Box::new(move || read.load(Ordering::SeqCst));
    let keys = Arc::new(ConsoleKeys::new(
        ConsoleKeyStore::memory(),
        ConsoleKeys::challenge_key_from(SECRET),
        900,
        HOME,
    ));
    state.wire_console_keys(keys.clone());
    let auth = Arc::new(state);
    let read = clock.clone();
    let door = Arc::new(Door {
        auth_router: ordo_host::auth::router(Some(auth.clone())),
        auth,
        ops: ConsoleKeyOps {
            keys,
            revocations: Some(revocations.clone()),
            now: Arc::new(move || read.load(Ordering::SeqCst)),
            stream: Some(recorder),
        },
        revocations,
        owner: Mutex::new(None),
        clock,
        deploys,
        node,
    });
    let app = Router::new()
        .route("/auth/complete", post(complete))
        .route("/op/{name}", any(op))
        .route(
            "/health/ready",
            axum::routing::get(|| async { Json(json!({"status": "ready"})) }),
        )
        .route("/health/deep", axum::routing::get(health_deep))
        .route("/health/deploy", axum::routing::get(health_deploy))
        .with_state(door.clone())
        .fallback_service(door.auth_router.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}:{}",
        site.door_host,
        listener.local_addr().unwrap().port()
    );
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
    cookie: String,
    owner_key: TestAuthenticator,
    site: Site,
}

struct Answer {
    status: StatusCode,
    body: Value,
}

async fn boot(name: &str, door_url: Option<&str>) -> H {
    boot_at(name, door_url, Site::default()).await
}

async fn boot_at(name: &str, door_url: Option<&str>, site: Site) -> H {
    let db = format!("ordo_console_t_{name}");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let dir = std::env::temp_dir().join(format!("ordo-console-door-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (door, url) = serve_door(&site, name).await;
    let vars = HashMap::from([
        (DATABASE_URL_ENV.to_string(), url_for(&db)),
        (ORIGIN_ENV.to_string(), site.console_origin.clone()),
        ("ORDO_CONSOLE_RP_ID".to_string(), site.console_rp.clone()),
        (DATA_DIR_ENV.to_string(), dir.display().to_string()),
        (
            PAGE_DIR_ENV.to_string(),
            dir.join("page").display().to_string(),
        ),
        (MODE_ENV.to_string(), "door-only".to_string()),
        (
            DOOR_URL_ENV.to_string(),
            door_url.unwrap_or(&url).to_string(),
        ),
        (DOOR_SPACE_ENV.to_string(), HOME.to_string()),
    ]);
    let cfg = ConsoleConfig::from_vars(&|k| vars.get(k).cloned()).expect("config");
    let console = Console::boot(
        cfg,
        BootOptions {
            clock: Clock::manual(START),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("boot");
    let mut h = H {
        router: console.router(),
        console,
        door,
        cookie: String::new(),
        owner_key: TestAuthenticator {
            key: TestKey::es256(21),
            credential_id: b"console-owner".to_vec(),
            rp_id: site.console_rp.clone(),
            origin: site.console_origin.clone(),
            ..TestAuthenticator::es256()
        },
        site,
    };
    // The console owner, enrolled with the setup code and touched.
    let code = std::fs::read_to_string(dir.join("setup-code"))
        .unwrap()
        .trim()
        .to_string();
    let begun = h.post("setup/begin", json!({"code": code})).await;
    let ch = begun.body["publicKey"]["challenge"]
        .as_str()
        .unwrap()
        .to_string();
    let CompleteRequest::PasskeyRegistration {
        client_data_json,
        attestation_object,
        ..
    } = h.owner_key.register(&ch)
    else {
        unreachable!()
    };
    let res = h
        .router
        .clone()
        .oneshot(
            Request::post("/d/main/api/setup/finish")
                .header("origin", &h.site.console_origin)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"code": code, "challenge": ch,
                           "clientDataJSON": b64url_encode(&client_data_json),
                           "attestationObject": b64url_encode(&attestation_object)})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    h.cookie = res
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap()
        .to_string();
    h
}

impl H {
    async fn send(&self, req: Request<Body>) -> Answer {
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        Answer {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    async fn post(&self, path: &str, body: Value) -> Answer {
        let mut req = Request::post(format!("/d/main/api/{path}"))
            .header("origin", &self.site.console_origin)
            .header("content-type", "application/json");
        if !self.cookie.is_empty() {
            req = req.header("cookie", &self.cookie);
        }
        self.send(req.body(Body::from(body.to_string())).unwrap())
            .await
    }

    async fn get(&self, path: &str) -> Answer {
        self.send(
            Request::get(format!("/d/main/api/{path}"))
                .header("cookie", &self.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    /// The console owner's own touch, at the console.
    async fn touch_console(&self) {
        let begun = self.post("stepup/begin", json!({})).await;
        let ch = begun.body["publicKey"]["challenge"]
            .as_str()
            .unwrap()
            .to_string();
        let CompleteRequest::PasskeyAssertion {
            credential_id,
            client_data_json,
            authenticator_data,
            signature,
            ..
        } = self.owner_key.assert(&ch)
        else {
            unreachable!()
        };
        let done = self
            .post(
                "stepup/finish",
                json!({"challenge": ch, "credentialId": b64url_encode(&credential_id),
                       "clientDataJSON": b64url_encode(&client_data_json),
                       "authenticatorData": b64url_encode(&authenticator_data),
                       "signature": b64url_encode(&signature)}),
            )
            .await;
        assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    }

    /// The deployment's console keys, as the door holds them.
    async fn door_keys(&self) -> Vec<Value> {
        let all = self.door.ops.keys.store.list().await.unwrap();
        all.iter()
            .map(|k| json!({"id": k.id, "revoked": k.revoked_at.is_some()}))
            .collect()
    }
}

/// Boot the console again on the same database and settings, as a
/// container restart does. The console owner's session survives it.
async fn restart(h: &mut H) {
    let cfg = h.console.config().clone();
    let now = h.console.clock().now();
    let console = Console::boot(
        cfg,
        BootOptions {
            clock: Clock::manual(now),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("boot again");
    h.router = console.router();
    h.console = console;
}

/// The ruling (2026-10-30): a deployment with no identity database keeps
/// working as before. A console linked before the upgrade never needs a
/// link step again: not after a restart, not after a rotation, not after
/// both.
#[tokio::test(flavor = "multi_thread")]
async fn a_linked_console_stays_linked_across_restarts_and_rotation_with_no_link_step() {
    let mut h = boot("door_kept", None).await;
    let owner = h.link(71).await;
    let first = h.get("door").await.body["key"]
        .as_str()
        .unwrap()
        .to_string();

    restart(&mut h).await;
    let status = h.get("door").await.body;
    assert_eq!(status["linked"], true, "{status}");
    assert_eq!(status["key"], first.as_str());
    let keys = h.get("door/keys").await;
    assert_eq!(keys.status, StatusCode::OK, "{}", keys.body);

    h.touch_console().await;
    let touched = touch_deployment(&h, &owner).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.body);
    let rotated = h.post("door/rotate", json!({"name": "rotated"})).await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.body);
    let second = rotated.body["key"].as_str().unwrap().to_string();

    restart(&mut h).await;
    h.advance(901);
    let status = h.get("door").await.body;
    assert_eq!(status["linked"], true, "{status}");
    assert_eq!(status["key"], second.as_str());
    let keys = h.get("door/keys").await;
    assert_eq!(keys.status, StatusCode::OK, "{}", keys.body);
    assert_eq!(
        h.door_keys().await,
        vec![
            json!({"id": first, "revoked": true}),
            json!({"id": second, "revoked": false})
        ]
    );
}

/// A console with no link and no identity service (a fresh install
/// without the identity database) is told plainly how to go on: by
/// sign-in as an owner the deployment named, or by the identity service's
/// 2 settings. The removed claim step is refused with the same words.
#[tokio::test(flavor = "multi_thread")]
async fn an_unlinked_console_without_the_identity_service_names_the_settings() {
    let h = boot("door_unlinked", None).await;
    let status = h.get("door").await.body;
    assert_eq!(status["linked"], false, "{status}");
    let how = status["howToLink"].as_str().expect("how to link");
    for setting in [
        "ORDO_DEPLOYMENT_OWNERS",
        "ORDO_IDENTITY_DATABASE_URL",
        "ORDO_IDENTITY_DATABASE_URL_FILE",
        "ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true",
    ] {
        assert!(how.contains(setting), "{setting}: {how}");
    }
    h.touch_console().await;
    let claim = h.post("door/link/begin", json!({"kind": "claim"})).await;
    assert_eq!(claim.status, StatusCode::BAD_REQUEST, "{}", claim.body);
    let said = claim.body["message"].as_str().unwrap_or_default();
    assert!(said.contains("claim step was removed"), "{said}");
    assert!(said.contains("ORDO_IDENTITY_DATABASE_URL"), "{said}");
    assert!(
        said.contains("ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true"),
        "{said}"
    );
    assert_eq!(h.get("door").await.body["linked"], false);
}

fn door_assertion(device: &TestAuthenticator, challenge: &str) -> Value {
    let CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = device.assert(challenge)
    else {
        unreachable!()
    };
    json!({"kind": "passkey_assertion", "challenge": challenge.0,
           "credential_id": b64url_encode(&credential_id),
           "client_data_json": b64url_encode(&client_data_json),
           "authenticator_data": b64url_encode(&authenticator_data),
           "signature": b64url_encode(&signature)})
}

/// Relay the deployment owner's touch for the console's session there.
async fn touch_deployment(h: &H, device: &TestAuthenticator) -> Answer {
    let begun = h.post("door/stepup/begin", json!({})).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    h.post(
        "door/stepup/finish",
        json!({"credential": door_assertion(device, &ch), "session": begun.body["session"]}),
    )
    .await
}

fn deployment_key(seed: u8) -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::es256(seed),
        credential_id: format!("deployment-{seed}").into_bytes(),
        ..TestAuthenticator::es256()
    }
}

// ═════════════════════════════════ tests ═════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn a_console_links_by_sign_in_and_signs_in_with_its_own_key() {
    let h = boot("door_claim", None).await;
    let status = h.get("door").await;
    assert_eq!(status.body["linked"], false, "{}", status.body);

    // Linking hands the console the deployment: the console owner's own
    // touch comes first.
    h.console.clock().advance(400);
    let refused = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["error"], "step_up_required");
    h.touch_console().await;

    // Somebody the deployment did not name: not admitted, nothing linked,
    // and the refusal says how an owner is named.
    let stranger = deployment_key(31);
    h.register_at_door(&stranger).await;
    let begun = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    let wrong = h
        .post(
            "door/link/finish",
            json!({"kind": "sign-in", "credential": door_assertion(&stranger, &ch)}),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN, "{}", wrong.body);
    assert_eq!(wrong.body["error"], "deployment_refused");
    assert_eq!(wrong.body["doorError"], "not_the_owner");
    let said = wrong.body.to_string();
    assert!(said.contains("ORDO_DEPLOYMENT_OWNERS"), "{said}");
    assert!(
        said.contains("ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true"),
        "{said}"
    );
    assert_eq!(h.get("door").await.body["linked"], false);

    // The owner the deployment named: admitted, the key enrolled with the
    // owner's touch, and the owner's own session signed out.
    let owner = deployment_key(32);
    let named = h.register_at_door(&owner).await;
    *h.door.owner.lock().unwrap() = Some(named);
    let begun = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    let linked = h
        .post(
            "door/link/finish",
            json!({"kind": "sign-in", "name": "test console",
                   "credential": door_assertion(&owner, &ch)}),
        )
        .await;
    assert_eq!(linked.status, StatusCode::OK, "{}", linked.body);
    let key = linked.body["key"].as_str().unwrap().to_string();
    assert_eq!(
        h.door_keys().await,
        vec![json!({"id": key, "revoked": false})]
    );
    let status = h.get("door").await.body;
    assert_eq!(status["linked"], true);
    assert_eq!(status["key"], key.as_str());
    assert_eq!(status["name"], "test console");
    assert_eq!(status["space"], HOME);
    assert!(status["sessionUntil"].as_u64().is_some(), "{status}");
    assert!(
        status["presentUntil"].is_null(),
        "a console key carries no presence"
    );

    // The private half is sealed: the row holds no key in the clear.
    let row = h
        .console
        .store()
        .credential("main")
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(row.key_id, key);
    assert!(row.sealed.len() > 64);

    // A second link is refused: rotate instead.
    let again = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    assert_eq!(again.body["error"], "already_linked", "{}", again.body);

    // The key is due for rotation after 30 days (ORDO_CONSOLE_KEY_ROTATE_DAYS).
    let link = h.console.link().expect("a door");
    let due = link.status().await.unwrap();
    assert_eq!(due["rotateDays"], 30);
    assert_eq!(due["rotateDue"], false);
    let at = due["rotateDueAt"].as_u64().unwrap();
    h.console.clock().advance(at - h.console.clock().now());
    assert_eq!(link.status().await.unwrap()["rotateDue"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_need_the_owners_relayed_touch_and_rotation_retires_the_old_key() {
    let h = boot("door_touch", None).await;
    let owner = h.link(41).await;
    let old = h.get("door").await.body["key"]
        .as_str()
        .unwrap()
        .to_string();

    // The console key alone cannot rotate: the deployment needs a touch.
    let refused = h.post("door/rotate", json!({})).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["error"], "deployment_touch_required");

    // A touch by a passkey the deployment does not know is refused there.
    let begun = h.post("door/stepup/begin", json!({})).await;
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    let stranger = deployment_key(42);
    let bad = h
        .post(
            "door/stepup/finish",
            json!({"credential": door_assertion(&stranger, &ch), "session": begun.body["session"]}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::FORBIDDEN, "{}", bad.body);
    assert_eq!(bad.body["error"], "deployment_refused");

    // The owner's touch, relayed: the session is present for 300 seconds.
    let touched = touch_deployment(&h, &owner).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.body);
    assert_eq!(touched.body["presentUntil"], START + 300);

    let rotated = h.post("door/rotate", json!({"name": "rotated"})).await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.body);
    let new = rotated.body["key"].as_str().unwrap().to_string();
    assert_ne!(new, old);
    assert_eq!(
        h.door_keys().await,
        vec![
            json!({"id": old, "revoked": true}),
            json!({"id": new, "revoked": false})
        ]
    );
    // The new key signs in: the status shows a session under it.
    let status = h.get("door").await.body;
    assert_eq!(status["key"], new.as_str());

    // Unlinking needs a touch for the new session, then revokes and forgets.
    let refused = h.post("door/unlink", json!({})).await;
    assert_eq!(
        refused.body["error"], "deployment_touch_required",
        "{}",
        refused.body
    );
    let touched = touch_deployment(&h, &owner).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.body);
    let gone = h.post("door/unlink", json!({})).await;
    assert_eq!(gone.status, StatusCode::OK, "{}", gone.body);
    assert_eq!(h.get("door").await.body["linked"], false);
    assert!(h.door_keys().await.iter().all(|k| k["revoked"] == true));
    let audit: Vec<String> = h
        .console
        .store()
        .pool()
        .rows("SELECT action || ' ' || outcome FROM audit ORDER BY id")
        .await
        .unwrap()
        .into_iter()
        .filter_map(|r| r[0].clone())
        .filter(|a| a.starts_with("door."))
        .collect();
    assert!(
        audit.contains(&"door.link.finish ok".to_string()),
        "{audit:?}"
    );
    assert!(audit.contains(&"door.rotate ok".to_string()), "{audit:?}");
    assert!(audit.contains(&"door.unlink ok".to_string()), "{audit:?}");
}

impl H {
    /// Move the console's clock and the deployment's together.
    fn advance(&self, secs: u64) {
        self.console.clock().advance(secs);
        self.door.clock.fetch_add(secs, Ordering::SeqCst);
    }

    /// Register a passkey at the deployment, as anyone does on its page;
    /// answers the principal it made.
    async fn register_at_door(&self, device: &TestAuthenticator) -> PrincipalId {
        let at = self.door.clock.load(Ordering::SeqCst);
        support::link_owner::register(&self.door.auth_router, device, SECRET, at).await
    }

    /// Name a new owner at the deployment and link by sign-in; answers
    /// the owner's passkey.
    async fn link(&self, seed: u8) -> TestAuthenticator {
        self.touch_console().await;
        let owner = deployment_key(seed);
        let named = self.register_at_door(&owner).await;
        *self.door.owner.lock().unwrap() = Some(named);
        let begun = self
            .post("door/link/begin", json!({"kind": "sign-in"}))
            .await;
        let ch = begun.body["challenge"].as_str().unwrap().to_string();
        let linked = self
            .post(
                "door/link/finish",
                json!({"kind": "sign-in", "credential": door_assertion(&owner, &ch)}),
            )
            .await;
        assert_eq!(linked.status, StatusCode::OK, "{}", linked.body);
        owner
    }
}

/// Security review of step 3a (finding 6): a renewal near a session's end
/// dropped the owner's touch, and a renewal between a touch's begin and
/// finish sent the assertion on a session it was not made for. A touched
/// session is now kept to its end, and the page carries the session from
/// begin to finish.
#[tokio::test(flavor = "multi_thread")]
async fn a_touch_survives_a_renewal_and_finishes_on_the_session_it_began_for() {
    let h = boot("door_lease", None).await;
    let owner = h.link(51).await;
    let link = h.console.link().expect("a door");

    // Touched 100 s before the session's end, then a read 50 s before it:
    // the touched session is kept, so the write that follows still counts.
    h.advance(800);
    let touched = touch_deployment(&h, &owner).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.body);
    h.advance(50);
    link.token().await.expect("a session");
    h.touch_console().await;
    let rotated = h.post("door/rotate", json!({"name": "kept"})).await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.body);

    // The new key's session has 70 s left when a touch begins; a renewal
    // replaces it before the finish. The finish goes to the session begun.
    h.advance(830);
    let begun = h.post("door/stepup/begin", json!({})).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    h.advance(15);
    link.token().await.expect("renewed");
    let done = h
        .post(
            "door/stepup/finish",
            json!({"credential": door_assertion(&owner, &ch), "session": begun.body["session"]}),
        )
        .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    // An unknown session asks for a touch again.
    let begun = h.post("door/stepup/begin", json!({})).await;
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    let stale = h
        .post(
            "door/stepup/finish",
            json!({"credential": door_assertion(&owner, &ch), "session": "gone"}),
        )
        .await;
    assert_eq!(
        stale.body["error"], "deployment_touch_required",
        "{}",
        stale.body
    );
}

/// The cross-site rehearsal (`plan-console.md` 12, step 3): the console on
/// one site (`https://console.ops.test`), the deployment on another
/// (relying party `deploy.test`), reached by a second loopback hostname
/// (`localhost`). Every deployment ceremony runs in the console's page, so
/// the browser reports the console's origin; the deployment takes it only
/// because it lists it in `ORDO_AUTH_RELATED_ORIGINS` and publishes it at
/// `/.well-known/webauthn`. Covers link, a read, a write with the relayed
/// touch, rotation, and revocation from either side.
#[tokio::test(flavor = "multi_thread")]
async fn cross_site_link_read_write_rotate_and_revoke() {
    const CONSOLE: &str = "https://console.ops.test";
    let site = Site {
        console_origin: CONSOLE.into(),
        console_rp: "ops.test".into(),
        door_rp: "deploy.test".into(),
        door_origins: vec!["https://app.deploy.test".into()],
        door_related: vec![CONSOLE.into()],
        door_host: "localhost".into(),
    };
    let h = boot_at("door_cross_site", None, site).await;
    let door_url = h.get("door").await.body["door"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(door_url.starts_with("http://localhost:"), "{door_url}");

    // The deployment publishes the console's origin for the browser.
    let doc = h
        .door
        .auth_router
        .clone()
        .oneshot(
            Request::get("/.well-known/webauthn")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let doc = axum::body::to_bytes(doc.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&doc).contains(CONSOLE));

    // The owner's deployment passkey, used in the console's page.
    let owner = TestAuthenticator {
        rp_id: "deploy.test".into(),
        origin: CONSOLE.into(),
        ..deployment_key(61)
    };
    // A page on a site the deployment does not list is refused.
    h.touch_console().await;
    let stray = TestAuthenticator {
        origin: "https://elsewhere.test".into(),
        ..deployment_key(62)
    };
    let begun = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    let refused = h
        .post(
            "door/link/finish",
            json!({"kind": "sign-in", "credential": door_assertion(&stray, &ch)}),
        )
        .await;
    assert_eq!(
        refused.body["error"], "deployment_refused",
        "{}",
        refused.body
    );

    // Link: the owner named at the deployment signs in from the page.
    let named = h.register_at_door(&owner).await;
    *h.door.owner.lock().unwrap() = Some(named);
    let begun = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    let linked = h
        .post(
            "door/link/finish",
            json!({"kind": "sign-in", "name": "ops console",
                   "credential": door_assertion(&owner, &ch)}),
        )
        .await;
    assert_eq!(linked.status, StatusCode::OK, "{}", linked.body);
    let first = linked.body["key"].as_str().unwrap().to_string();

    // A read: the keys, and Health from the door.
    let keys = h.get("door/keys").await;
    assert_eq!(keys.status, StatusCode::OK, "{}", keys.body);
    assert_eq!(keys.body["keys"][0]["id"], first.as_str());
    assert_eq!(keys.body["keys"][0]["thisConsole"], true);
    let health = h.get("door/health").await.body;
    assert_eq!(health["ready"]["status"], "ready", "{health}");
    assert_eq!(health["deep"]["boot"]["revision"], "7", "{health}");
    assert_eq!(health["link"]["rotateDue"], false, "{health}");

    // A write with the relayed touch: revoke another console's key.
    let other = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let owner_id = h.door.owner.lock().unwrap().clone().unwrap();
    let added = h
        .door
        .ops
        .execute(
            ADD,
            HOME,
            &BoundParams(
                json!({"publicKey": b64url_encode(other.verifying_key().as_bytes()),
                       "name": "old laptop"})
                .as_object()
                .unwrap()
                .clone(),
            ),
            &owner_id.0,
        )
        .await
        .unwrap();
    let other_id = added.rows[0]["id"].as_str().unwrap().to_string();
    let untouched = h.post("door/keys/revoke", json!({"id": other_id})).await;
    assert_eq!(untouched.body["error"], "deployment_touch_required");
    let touched = touch_deployment(&h, &owner).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.body);
    let revoked = h.post("door/keys/revoke", json!({"id": other_id})).await;
    assert_eq!(revoked.status, StatusCode::OK, "{}", revoked.body);
    let own = h.post("door/keys/revoke", json!({"id": first})).await;
    assert_eq!(own.body["error"], "bad_request", "{}", own.body);

    // Rotation.
    let rotated = h.post("door/rotate", json!({"name": "ops console"})).await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.body);
    let second = rotated.body["key"].as_str().unwrap().to_string();
    assert_ne!(second, first);

    // Revocation from the deployment's side: the console's next sign-in is
    // refused, it says so, and the owner forgets the link here.
    h.door
        .ops
        .execute(
            REVOKE,
            HOME,
            &BoundParams(json!({"id": second}).as_object().unwrap().clone()),
            &owner_id.0,
        )
        .await
        .unwrap();
    h.advance(901);
    h.touch_console().await;
    let after = h.get("door/keys").await;
    assert_eq!(after.body["error"], "console_key_refused", "{}", after.body);
    let forgot = h.post("door/forget", json!({})).await;
    assert_eq!(forgot.status, StatusCode::OK, "{}", forgot.body);
    assert_eq!(h.get("door").await.body["linked"], false);

    // Revocation from the console's side: link again, then unlink.
    let begun = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    let relinked = h
        .post(
            "door/link/finish",
            json!({"kind": "sign-in", "credential": door_assertion(&owner, &ch)}),
        )
        .await;
    assert_eq!(relinked.status, StatusCode::OK, "{}", relinked.body);
    let forget_live = h.post("door/forget", json!({})).await;
    assert_eq!(
        forget_live.body["error"], "bad_request",
        "{}",
        forget_live.body
    );
    let touched = touch_deployment(&h, &owner).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.body);
    let gone = h.post("door/unlink", json!({})).await;
    assert_eq!(gone.status, StatusCode::OK, "{}", gone.body);
    assert!(h.door_keys().await.iter().all(|k| k["revoked"] == true));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_that_does_not_answer_is_said_plainly_and_may_be_retried() {
    // A port nothing listens on.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let h = boot("door_down", Some(&format!("http://{closed}"))).await;
    h.touch_console().await;
    let down = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    assert_eq!(
        down.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        down.body
    );
    assert_eq!(down.body["error"], "deployment_not_answering");
    assert_eq!(down.body["retry"], true);
    let _ = h.door.auth.console_keys();
}

// ═══════════════════════════ step 4: deploys ═════════════════════════════

const APP_V1: &str = r#"
entity note {
  title: text
}

op listNotes unbounded =
  note[title]
"#;

/// V1 plus an optional field: a safe change the apply makes itself.
const APP_V2: &str = r#"
entity note {
  title: text
  body:  text?
}

op listNotes unbounded =
  note[title, body]
"#;

/// The Deploys screen's routes: read the history, preview (nothing
/// stored), apply with both touches, wait until the node serves it, and
/// roll the app back. Every change is in the console's audit.
#[tokio::test(flavor = "multi_thread")]
async fn deploys_preview_apply_wait_and_roll_back_the_app() {
    let h = boot("door_deploys", None).await;
    let owner = h.link(51).await;

    // Nothing applied yet: an empty history, read with the console key.
    let empty = h.get("deploys").await;
    assert_eq!(empty.status, StatusCode::OK, "{}", empty.body);
    assert_eq!(empty.body["active"], Value::Null);

    // A preview is a POST at the deployment, so it asks for the touch.
    let refused = h.post("deploys/preview", json!({"program": APP_V1})).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["error"], "deployment_touch_required");
    let touched = touch_deployment(&h, &owner).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.body);

    let preview = h.post("deploys/preview", json!({"program": APP_V1})).await;
    assert_eq!(preview.status, StatusCode::OK, "{}", preview.body);
    assert_eq!(preview.body["would"]["state"], "create");
    assert_eq!(preview.body["alreadyStored"], false);
    assert_eq!(
        h.get("deploys").await.body["stored"],
        json!([]),
        "a preview stores nothing"
    );

    // A broken program: the deployment's refusal, with its own word.
    let broken = h
        .post("deploys/preview", json!({"program": "entity {"}))
        .await;
    assert_eq!(broken.status, StatusCode::FORBIDDEN, "{}", broken.body);
    assert_eq!(broken.body["error"], "deployment_refused");
    assert_eq!(broken.body["doorError"], "apply_refused");

    // Apply V1, then V2.
    let v1 = h
        .post(
            "deploys/apply",
            json!({"program": APP_V1, "activate": true}),
        )
        .await;
    assert_eq!(v1.status, StatusCode::OK, "{}", v1.body);
    assert_eq!(v1.body["activation"]["state"], "switched");
    let v1 = v1.body["version"].as_str().unwrap().to_string();
    let preview = h.post("deploys/preview", json!({"program": APP_V2})).await;
    assert_eq!(
        preview.body["would"]["state"], "migrate",
        "{}",
        preview.body
    );
    let v2 = h.post("deploys/apply", json!({"program": APP_V2})).await;
    assert_eq!(v2.status, StatusCode::OK, "{}", v2.body);
    let v2 = v2.body["version"].as_str().unwrap().to_string();

    // The wait: the node serves the active version and has caught up.
    let serving = h.get("deploys/serving").await;
    assert_eq!(serving.status, StatusCode::OK, "{}", serving.body);
    assert_eq!(serving.body["version"], v2.as_str());
    assert_eq!(serving.body["pending"], json!([]), "{}", serving.body);

    let history = h.get("deploys").await.body;
    assert_eq!(history["active"], v2.as_str());
    assert_eq!(history["stored"].as_array().unwrap().len(), 2, "{history}");

    // Roll the app back to V1.
    let back = h.post("deploys/activate", json!({"version": v1})).await;
    assert_eq!(back.status, StatusCode::OK, "{}", back.body);
    assert_eq!(back.body["state"], "switched");
    assert_eq!(back.body["from"], v2.as_str());
    assert_eq!(h.get("deploys").await.body["active"], v1.as_str());

    // An unknown version is the deployment's `not_found`.
    let unknown = h
        .post("deploys/activate", json!({"version": "v-none"}))
        .await;
    assert_eq!(unknown.body["doorError"], "not_found", "{}", unknown.body);

    let audit: Vec<String> = h
        .console
        .store()
        .pool()
        .rows("SELECT action || ' ' || outcome FROM audit ORDER BY id")
        .await
        .unwrap()
        .into_iter()
        .filter_map(|r| r[0].clone())
        .filter(|a| a.starts_with("deploys."))
        .collect();
    assert!(
        audit.contains(&"deploys.preview ok".to_string()),
        "{audit:?}"
    );
    assert!(audit.contains(&"deploys.apply ok".to_string()), "{audit:?}");
    assert!(
        audit.contains(&"deploys.activate ok".to_string()),
        "{audit:?}"
    );
}

/// A change to the app's program needs the console owner's own touch as
/// well, and a page from another origin is refused before anything.
#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_needs_the_console_owners_touch_too() {
    let h = boot("door_deploys_touch", None).await;
    let owner = h.link(52).await;
    let touched = touch_deployment(&h, &owner).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.body);
    // The console's own touch lapses after 300 seconds; the deployment's
    // was taken just now on its own clock.
    h.console.clock().advance(400);
    let refused = h.post("deploys/apply", json!({"program": APP_V1})).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["error"], "step_up_required");
    let refused = h.post("deploys/activate", json!({"version": "x"})).await;
    assert_eq!(refused.body["error"], "step_up_required");

    let foreign = h
        .send(
            Request::post("/d/main/api/deploys/apply")
                .header("origin", "https://elsewhere.test")
                .header("cookie", &h.cookie)
                .header("content-type", "application/json")
                .body(Body::from(json!({"program": APP_V1}).to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(foreign.body["error"], "origin_refused", "{}", foreign.body);
    assert_eq!(h.get("deploys").await.body["stored"], json!([]));
}
