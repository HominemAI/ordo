//! Step 6, the settings screens' route (`plan-console.md` 4.2, 4.6 and
//! 4.10): every setting as the running deployment took it, beside what
//! the console's live revision gives it, with drift where they differ, and
//! a change through a runtime revision.
//!
//! The supervisor is a stand-in for its socket
//! (`support/fake_supervisor.rs`), which writes the environment the
//! deployment would start each live revision with to a file. The door is the
//! deployment's own sign-in routes, console key ops and door check, served
//! in-process; its `std.deployment.settings` is `ordo-host`'s own answer
//! over that file (`support/settings_door.rs`). So what the door says is
//! what the running child took.
//!
//! Needs the conformance cluster for the console's own database
//! (`scripts/conformance-pg.sh start`).

mod support {
    pub mod fake_supervisor;
    pub mod link_owner;
    pub mod settings_door;
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::{Json, Router};
use ordo_api::{AccessChecker, AccessDecision, BoundParams};
use ordo_console::config::{
    DATABASE_URL_ENV, DATA_DIR_ENV, DOOR_SPACE_ENV, DOOR_URL_ENV, LISTEN_ENV, ORIGIN_ENV,
    PAGE_DIR_ENV, SUPERVISOR_SOCKET_ENV,
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
use support::fake_supervisor::FakeSupervisor;
use support::settings_door::{SettingsDoor, SETTINGS};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;
const HOME: &str = "home";
const SECRET: &[u8] = b"console-settings-test-secret";
/// A secret the container hands the console, which must never be shown.
const TOKEN: &str = "ef56ef56ef56ef56ef56ef56ef56ef56ef56ef56ef56ef56ef56ef56ef56ef56";

// ════════════════════════════ the deployment ═════════════════════════════

struct Door {
    keys: ConsoleKeyOps,
    settings: SettingsDoor,
    revocations: Arc<MemorySessionRevocations>,
    owner: Mutex<Option<PrincipalId>>,
    auth_router: Router,
}

fn now() -> u64 {
    START
}

impl Door {
    fn access(&self) -> ordo_host::IdentityAccess {
        let space = SpaceId(HOME.into());
        let mut engine = AccessEngine::new();
        engine.set_profile(
            &space,
            AccessProfile::new()
                .allow(Role::Owner, ADD)
                .allow(Role::Owner, LIST)
                .allow(Role::Owner, REVOKE)
                .allow(Role::Owner, SETTINGS),
        );
        if let Some(o) = self.owner.lock().unwrap().as_ref() {
            engine.set_membership(&space, o, Role::Owner);
        }
        engine.require_step_up(ADD);
        engine.require_step_up(REVOKE);
        ordo_host::IdentityAccess::new(
            engine,
            TokenKey::from_secret(SECRET),
            Box::new(now),
            Arc::new(ordo_host::metrics::Metrics::new()),
        )
        .with_revocations(self.revocations.clone())
        .with_console_home(HOME)
    }
}

fn door_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

async fn op(
    State(d): State<Arc<Door>>,
    UrlPath(name): UrlPath<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
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
    let space = header("x-ordo-space");
    let principal = match d.access().check(&bearer, &space, &name) {
        AccessDecision::Allow { principal, .. } => principal,
        _ => return door_error(StatusCode::FORBIDDEN, "access_denied", "not permitted"),
    };
    if name == SETTINGS {
        return Json(json!({"rows": d.settings.rows()})).into_response();
    }
    let params: serde_json::Map<String, Value> = serde_json::from_slice(&body).unwrap_or_default();
    match d
        .keys
        .execute(&name, &space, &BoundParams(params), &principal)
        .await
    {
        Ok(r) => Json(json!({"rows": r.rows})).into_response(),
        Err(e) => door_error(StatusCode::CONFLICT, e.code.as_str(), &e.message),
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
        let who = ordo_identity::verify(
            &TokenKey::from_secret(SECRET),
            token,
            ordo_identity::Timestamp(START),
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

async fn serve_door(env_file: Option<PathBuf>) -> (Arc<Door>, String) {
    let rp = RelyingParty::new("example.com", &[DEFAULT_ORIGIN]).unwrap();
    let passkey = Arc::new(PasskeyProvider::with_stores(
        rp,
        Arc::new(ordo_identity::MemoryPasskeyStore::new()),
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
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
    state.now = Box::new(now);
    let keys = Arc::new(ConsoleKeys::new(
        ConsoleKeyStore::memory(),
        ConsoleKeys::challenge_key_from(SECRET),
        900,
        HOME,
    ));
    state.wire_console_keys(keys.clone());
    let auth = Arc::new(state);
    let door = Arc::new(Door {
        auth_router: ordo_host::auth::router(Some(auth)),
        keys: ConsoleKeyOps {
            keys,
            revocations: Some(revocations.clone()),
            now: Arc::new(now),
            stream: Some(recorder),
        },
        settings: SettingsDoor::new(env_file),
        revocations,
        owner: Mutex::new(None),
    });
    let app = Router::new()
        .route("/auth/complete", post(complete))
        .route("/op/{name}", any(op))
        .with_state(door.clone())
        .fallback_service(door.auth_router.clone());
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

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct H {
    console: Console,
    router: Router,
    door: Arc<Door>,
    dir: PathBuf,
    cookie: String,
}

struct Answer {
    status: StatusCode,
    body: Value,
}

/// The file the stand-in supervisor writes the deployment's environment to.
fn env_file(dir: &Path) -> PathBuf {
    dir.join("child.env")
}

/// A console in `mode`, its owner signed in and touched, linked to the
/// door with the deployment owner's passkey. In supervise mode it talks to
/// a stand-in supervisor whose revision 1 is built from `container`.
async fn boot(name: &str, mode: &str, container: &[(&str, &str)]) -> H {
    let db = format!("ordo_console_st_{name}");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let dir = std::env::temp_dir().join(format!(
        "ordo-console-settings-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("page")).unwrap();
    std::fs::write(
        dir.join("page/index.html"),
        "<!doctype html><title>t</title>",
    )
    .unwrap();
    let supervise = mode == "supervise";
    let (door, url) = serve_door(supervise.then(|| env_file(&dir))).await;
    let mut container_env = vec![(
        "ORDO_HOST_ADDR".to_string(),
        format!("127.0.0.1:{}", free_port()),
    )];
    for (k, v) in container {
        container_env.push((k.to_string(), v.to_string()));
    }
    let pairs: Vec<(&str, &str)> = container_env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let sup = FakeSupervisor::start(&format!("st-{name}"), &pairs, Some(env_file(&dir))).await;
    let mut vars = HashMap::from([
        (DATABASE_URL_ENV.to_string(), url_for(&db)),
        (ORIGIN_ENV.to_string(), DEFAULT_ORIGIN.to_string()),
        ("ORDO_CONSOLE_RP_ID".to_string(), "example.com".to_string()),
        (DATA_DIR_ENV.to_string(), dir.display().to_string()),
        (
            PAGE_DIR_ENV.to_string(),
            dir.join("page").display().to_string(),
        ),
        (DOOR_URL_ENV.to_string(), url),
        (DOOR_SPACE_ENV.to_string(), HOME.to_string()),
        (LISTEN_ENV.to_string(), format!("127.0.0.1:{}", free_port())),
    ]);
    if supervise {
        vars.insert(
            SUPERVISOR_SOCKET_ENV.to_string(),
            sup.socket.display().to_string(),
        );
    }
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
        dir: dir.clone(),
        cookie: String::new(),
    };
    if supervise {
        assert!(h.console.runtime().expect("supervise mode").synced());
    }
    let owner_key = TestAuthenticator {
        key: TestKey::es256(23),
        credential_id: b"console-owner".to_vec(),
        rp_id: "example.com".into(),
        origin: DEFAULT_ORIGIN.into(),
        ..TestAuthenticator::es256()
    };
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
    } = owner_key.register(&ch)
    else {
        unreachable!()
    };
    let res = h
        .router
        .clone()
        .oneshot(
            Request::post("/d/main/api/setup/finish")
                .header("origin", DEFAULT_ORIGIN)
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

    // Link by sign-in with the owner's passkey there. The clock
    // stands still, so the setup's touch is still fresh.
    let deployment_key = TestAuthenticator {
        key: TestKey::es256(54),
        credential_id: b"deployment-54".to_vec(),
        ..TestAuthenticator::es256()
    };
    // The owner registered at the deployment and was named its owner
    // there (`ORDO_DEPLOYMENT_OWNERS`): the claim step is removed.
    let named =
        support::link_owner::register(&h.door.auth_router, &deployment_key, SECRET, START).await;
    *h.door.owner.lock().unwrap() = Some(named);
    let begun = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    let linked = h
        .post(
            "door/link/finish",
            json!({"kind": "sign-in", "credential": support::link_owner::assertion(&deployment_key, &ch)}),
        )
        .await;
    assert_eq!(linked.status, StatusCode::OK, "{}", linked.body);
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
            .header("origin", DEFAULT_ORIGIN)
            .header("content-type", "application/json");
        if !self.cookie.is_empty() {
            req = req.header("cookie", &self.cookie);
        }
        self.send(req.body(Body::from(body.to_string())).unwrap())
            .await
    }

    async fn settings(&self) -> Value {
        let a = self
            .send(
                Request::get("/d/main/api/settings")
                    .header("cookie", &self.cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(a.status, StatusCode::OK, "{}", a.body);
        a.body
    }

    async fn done(self) {
        self.console.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn row<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("no row {name} in {body}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_show_value_source_and_drift_and_a_change_goes_through_a_revision() {
    let h = boot(
        "drift",
        "supervise",
        &[
            ("ORDO_PG_PORT", "6543"),
            ("ORDO_TOKEN_KEY", TOKEN),
            ("ORDO_AUTH_GUEST", "on"),
        ],
    )
    .await;
    // `std.deployment.setGuest` stored `off`, which wins over the variable.
    h.door
        .settings
        .store("auth_guest", "off", "dep_owner", START);

    let body = h.settings().await;
    assert_eq!(body["mode"], "supervise");
    assert_eq!(body["editable"], true);
    assert_eq!(body["revision"], 1);
    assert!(body["doorError"].is_null(), "{body}");
    let port = row(&body, "ORDO_PG_PORT");
    assert_eq!(port["value"], "6543");
    assert_eq!(port["source"], "environment");
    assert_eq!(port["default"], "5432");
    assert_eq!(port["console"]["value"], "6543");
    assert!(port["drift"].is_null(), "{port}");
    let token = row(&body, "ORDO_TOKEN_KEY");
    assert_eq!(token["source"], "file", "{token}");
    assert_eq!(token["console"]["secret"], "managed");
    assert!(token["drift"].is_null(), "{token}");
    let guest = row(&body, "ORDO_AUTH_GUEST");
    assert_eq!(guest["drift"]["kind"], "stored", "{guest}");
    let idle = row(&body, "ORDO_PG_IDLE_SECS");
    assert_eq!(idle["source"], "default");
    assert!(idle["drift"].is_null());
    // A secret is never shown, nor the file that holds it.
    let text = body.to_string();
    assert!(!text.contains(TOKEN), "{text}");
    assert!(!text.contains("/secrets/"), "{text}");

    // The deployment keeps running what it started with while the console
    // moves on: drift.
    let frozen = h.dir.join("frozen.env");
    std::fs::copy(env_file(&h.dir), &frozen).unwrap();
    h.door.settings.read_from(frozen);
    let applied = h
        .post(
            "runtime/apply",
            json!({"set": {"ORDO_PG_PORT": "7000"}, "reason": "move the port"}),
        )
        .await;
    assert_eq!(applied.status, StatusCode::OK, "{}", applied.body);
    assert_eq!(applied.body["revision"], 2);
    let body = h.settings().await;
    let port = row(&body, "ORDO_PG_PORT");
    assert_eq!(port["console"]["value"], "7000");
    assert_eq!(port["value"], "6543");
    assert_eq!(port["drift"]["kind"], "not-taken", "{port}");

    // The running child, as it really is: it took the change.
    h.door.settings.read_from(env_file(&h.dir));
    let body = h.settings().await;
    let port = row(&body, "ORDO_PG_PORT");
    assert_eq!(port["value"], "7000");
    assert!(port["drift"].is_null(), "{port}");
    assert_eq!(body["revision"], 2);
    h.done().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_claim_code_is_sealed_and_no_revision_row_holds_it() {
    let h = boot(
        "claim",
        "supervise",
        &[("ORDO_AUTH_CLAIM_CODE", "required")],
    )
    .await;
    let rows = h
        .console
        .store()
        .pool()
        .rows("SELECT settings::text, secret_refs::text FROM revisions")
        .await
        .unwrap();
    for r in &rows {
        let settings = r[0].clone().unwrap_or_default();
        assert!(!settings.contains("ORDO_AUTH_CLAIM_CODE"), "{settings}");
        assert!(
            r[1].clone()
                .unwrap_or_default()
                .contains("ORDO_AUTH_CLAIM_CODE"),
            "sealed and referenced: {r:?}"
        );
    }
    let env = std::fs::read_to_string(env_file(&h.dir)).unwrap();
    assert!(env.contains("ORDO_AUTH_CLAIM_CODE_FILE="), "{env}");
    assert!(!env.contains("ORDO_AUTH_CLAIM_CODE=required"), "{env}");
    let body = h.settings().await;
    let claim = row(&body, "ORDO_AUTH_CLAIM_CODE");
    assert_eq!(claim["secret"], true);
    assert_eq!(claim["source"], "file");
    assert!(claim["value"].is_null() && claim["console"].get("value").is_none());
    // Sent as a setting, it is refused, naming where it goes.
    let refused = h
        .post(
            "runtime/apply",
            json!({"set": {"ORDO_AUTH_CLAIM_CODE": "off"}}),
        )
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.body);
    assert!(refused.body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("secrets"));
    h.done().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn in_door_only_mode_the_settings_are_read_only() {
    let h = boot("door_only", "door-only", &[]).await;
    h.door
        .settings
        .store("auth_guest", "on", "dep_owner", START);
    let body = h.settings().await;
    assert_eq!(body["mode"], "door-only");
    assert_eq!(body["editable"], false);
    assert!(body["revision"].is_null());
    let guest = row(&body, "ORDO_AUTH_GUEST");
    assert_eq!(guest["value"], "on");
    assert!(guest.get("console").is_none() && guest.get("drift").is_none());
    let refused = h
        .post("runtime/apply", json!({"set": {"ORDO_PG_PORT": "1"}}))
        .await;
    assert_ne!(refused.status, StatusCode::OK, "{}", refused.body);
    h.done().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_settings_answer_only_a_signed_in_owner() {
    let h = boot("signed_out", "door-only", &[]).await;
    let a = h
        .send(
            Request::get("/d/main/api/settings")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{}", a.body);
    assert!(a.body.get("rows").is_none());
    h.done().await;
}
