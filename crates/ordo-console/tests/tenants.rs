//! Step 5, the Tenants screen's routes (`plan-console.md` 4.4): list,
//! create, reap, pause and resume in the home space, and a tenant's own
//! export, delete and bucket ops where the deployment owner owns it, all
//! through the console's one client to the door with the console key.
//!
//! The door is the deployment's own sign-in routes, console key ops and
//! door check (`ordo_host::IdentityAccess` with its home space), served
//! in-process. The tenant ops behind the check are a stand-in over memory
//! (`support/tenants_door.rs`); `ordo-host/tests/deployment_tenants.rs`
//! runs the real ones against Postgres.
//!
//! Needs the conformance cluster for the console's own database
//! (`scripts/conformance-pg.sh start`).

mod support {
    pub mod link_owner;
    pub mod tenants_door;
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
use support::tenants_door::{Tenants, CASE_MATURED, CASE_PAUSED, HELD, MINE, STUCK, THEIRS};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;
const HOME: &str = "home";
const SECRET: &[u8] = b"console-tenants-test-secret";

// ════════════════════════════ the deployment ═════════════════════════════

struct Door {
    keys: ConsoleKeyOps,
    tenants: Tenants,
    revocations: Arc<MemorySessionRevocations>,
    owner: Mutex<Option<PrincipalId>>,
    clock: Arc<AtomicU64>,
    auth_router: Router,
}

impl Door {
    fn access(&self) -> ordo_host::IdentityAccess {
        let space = SpaceId(HOME.into());
        let mut engine = AccessEngine::new();
        engine.set_profile(
            &space,
            Tenants::home_profile(
                AccessProfile::new()
                    .allow(Role::Owner, ADD)
                    .allow(Role::Owner, LIST)
                    .allow(Role::Owner, REVOKE),
            ),
        );
        let owner = self.owner.lock().unwrap().clone();
        if let Some(o) = &owner {
            engine.set_membership(&space, o, Role::Owner);
        }
        engine.require_step_up(ADD);
        engine.require_step_up(REVOKE);
        self.tenants.grant(&mut engine, owner.as_ref());
        let at = self.clock.load(Ordering::SeqCst);
        ordo_host::IdentityAccess::new(
            engine,
            TokenKey::from_secret(SECRET),
            Box::new(move || at),
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

/// `/op/{name}`: the door's check, then the console key ops or the tenant
/// ops.
async fn op(
    State(d): State<Arc<Door>>,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
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
    let params: serde_json::Map<String, Value> = if body.is_empty() {
        query.into_iter().map(|(k, v)| (k, json!(v))).collect()
    } else {
        serde_json::from_slice(&body).unwrap_or_default()
    };
    if [ADD, LIST, REVOKE].contains(&name.as_str()) {
        return match d
            .keys
            .execute(&name, &space, &BoundParams(params), &principal)
            .await
        {
            Ok(r) => Json(json!({"rows": r.rows})).into_response(),
            Err(e) => door_error(StatusCode::CONFLICT, e.code.as_str(), &e.message),
        };
    }
    match d.tenants.execute(&name, &space, &params, &principal) {
        Ok(rows) => Json(json!({"rows": rows})).into_response(),
        Err((code, message)) => door_error(StatusCode::CONFLICT, &code, &message),
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

async fn ready(State(d): State<Arc<Door>>) -> Response {
    Json(json!({"status": "ready", "quarantined": d.tenants.quarantined()})).into_response()
}

async fn serve_door() -> (Arc<Door>, String) {
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
        auth_router: ordo_host::auth::router(Some(auth)),
        keys: ConsoleKeyOps {
            keys,
            revocations: Some(revocations.clone()),
            now: Arc::new(move || read.load(Ordering::SeqCst)),
            stream: Some(recorder),
        },
        tenants: Tenants::new(START),
        revocations,
        owner: Mutex::new(None),
        clock,
    });
    let app = Router::new()
        .route("/auth/complete", post(complete))
        .route("/op/{name}", any(op))
        .route("/health/ready", axum::routing::get(ready))
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

struct H {
    console: Console,
    router: Router,
    door: Arc<Door>,
    cookie: String,
    owner_key: TestAuthenticator,
    deployment_key: TestAuthenticator,
}

struct Answer {
    status: StatusCode,
    body: Value,
}

/// A console, its owner signed in and touched, linked to the door with
/// the deployment owner's passkey.
async fn boot(name: &str) -> H {
    let db = format!("ordo_console_t_{name}");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let dir = std::env::temp_dir().join(format!(
        "ordo-console-tenants-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let (door, url) = serve_door().await;
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
        (DOOR_URL_ENV.to_string(), url),
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
            rp_id: "example.com".into(),
            origin: DEFAULT_ORIGIN.into(),
            ..TestAuthenticator::es256()
        },
        deployment_key: TestAuthenticator {
            key: TestKey::es256(52),
            credential_id: b"deployment-52".to_vec(),
            ..TestAuthenticator::es256()
        },
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
    } = h.owner_key.register(&ch)
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

    // Link by sign-in with the owner's passkey there.
    h.touch_console().await;
    // The owner registered at the deployment and was named its owner
    // there (`ORDO_DEPLOYMENT_OWNERS`): the claim step is removed.
    let named = support::link_owner::register(
        &h.door.auth_router,
        &h.deployment_key,
        SECRET,
        h.door.clock.load(Ordering::SeqCst),
    )
    .await;
    *h.door.owner.lock().unwrap() = Some(named);
    let begun = h.post("door/link/begin", json!({"kind": "sign-in"})).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    let ch = begun.body["challenge"].as_str().unwrap().to_string();
    let linked = h
        .post(
            "door/link/finish",
            json!({"kind": "sign-in", "credential": support::link_owner::assertion(&h.deployment_key, &ch)}),
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

    /// The deployment owner's touch, relayed for the console's session.
    async fn touch_deployment(&self) {
        let begun = self.post("door/stepup/begin", json!({})).await;
        assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
        let ch = begun.body["challenge"].as_str().unwrap().to_string();
        let CompleteRequest::PasskeyAssertion {
            challenge,
            credential_id,
            client_data_json,
            authenticator_data,
            signature,
        } = self.deployment_key.assert(&ch)
        else {
            unreachable!()
        };
        let done = self
            .post(
                "door/stepup/finish",
                json!({"session": begun.body["session"], "credential": {
                    "kind": "passkey_assertion", "challenge": challenge.0,
                    "credential_id": b64url_encode(&credential_id),
                    "client_data_json": b64url_encode(&client_data_json),
                    "authenticator_data": b64url_encode(&authenticator_data),
                    "signature": b64url_encode(&signature)}}),
            )
            .await;
        assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    }

    async fn tenants(&self) -> Vec<Value> {
        let a = self.get("tenants").await;
        assert_eq!(a.status, StatusCode::OK, "{}", a.body);
        a.body["tenants"].as_array().unwrap().clone()
    }

    async fn audit(&self) -> Vec<(String, String)> {
        self.console
            .store()
            .pool()
            .rows("SELECT action, outcome FROM audit ORDER BY id")
            .await
            .expect("audit")
            .into_iter()
            .map(|r| {
                (
                    r[0].clone().unwrap_or_default(),
                    r[1].clone().unwrap_or_default(),
                )
            })
            .collect()
    }
}

fn find<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {rows:?}"))
}

// ═════════════════════════════════ tests ═════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn the_list_shows_status_usage_and_the_tenants_held_closed() {
    let h = boot("tenants_list").await;
    let rows = h.tenants().await;
    assert_eq!(rows.len(), 4);
    let mine = find(&rows, MINE);
    assert_eq!(mine["status"], "active");
    assert_eq!(mine["yourRole"], "owner");
    assert_eq!(mine["usage"]["writes"], 42);
    assert_eq!(find(&rows, STUCK)["status"], "provisioning");
    // `/health/ready` names a tenant held closed, and the list says why.
    assert_eq!(
        find(&rows, HELD)["quarantined"],
        "the chain moved backwards"
    );
    assert!(find(&rows, THEIRS)["quarantined"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn create_pause_resume_and_reap_need_both_touches_and_leave_audit_rows() {
    let h = boot("tenants_writes").await;

    // The console key alone: the deployment asks for the owner's touch.
    let refused = h.post("tenants/create", json!({"name": "New Co"})).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["error"], "deployment_touch_required");
    h.touch_deployment().await;

    // Created, owned by the deployment owner when no owner is named.
    let made = h.post("tenants/create", json!({"name": "New Co"})).await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.body);
    let id = made.body["created"]["space"].as_str().unwrap().to_string();
    let rows = h.tenants().await;
    assert_eq!(find(&rows, &id)["name"], "New Co");
    assert_eq!(find(&rows, &id)["yourRole"], "owner");

    // A name of nothing is refused at the console.
    let bad = h.post("tenants/create", json!({"name": "  "})).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{}", bad.body);

    // Pause with a reason from the closed list, then resume.
    let paused = h
        .post("tenants/pause", json!({"tenant": MINE, "note": "billing"}))
        .await;
    assert_eq!(paused.status, StatusCode::OK, "{}", paused.body);
    let rows = h.tenants().await;
    assert_eq!(find(&rows, MINE)["status"], "paused");
    assert_eq!(find(&rows, MINE)["pausedNote"], "billing");
    let free_text = h
        .post("tenants/pause", json!({"tenant": THEIRS, "note": "late"}))
        .await;
    assert_eq!(
        free_text.status,
        StatusCode::FORBIDDEN,
        "{}",
        free_text.body
    );
    assert_eq!(free_text.body["error"], "deployment_refused");
    let resumed = h.post("tenants/resume", json!({"tenant": MINE})).await;
    assert_eq!(resumed.status, StatusCode::OK, "{}", resumed.body);
    assert_eq!(find(&h.tenants().await, MINE)["status"], "active");

    // Reap the one stuck in provisioning; a serving one is refused.
    let reaped = h.post("tenants/reap", json!({"tenant": STUCK})).await;
    assert_eq!(reaped.status, StatusCode::OK, "{}", reaped.body);
    assert!(h.tenants().await.iter().all(|r| r["id"] != STUCK));
    let serving = h.post("tenants/reap", json!({"tenant": MINE})).await;
    assert_eq!(
        serving.body["error"], "deployment_refused",
        "{}",
        serving.body
    );

    // An id the door would not take never leaves the console.
    let odd = h.post("tenants/pause", json!({"tenant": "a/b"})).await;
    assert_eq!(odd.status, StatusCode::BAD_REQUEST, "{}", odd.body);

    // The console owner's own touch runs out: writes ask again.
    h.console.clock().advance(400);
    let stale = h.post("tenants/resume", json!({"tenant": MINE})).await;
    assert_eq!(stale.body["error"], "step_up_required", "{}", stale.body);

    let audit = h.audit().await;
    for action in [
        "tenants.create",
        "tenants.pause",
        "tenants.resume",
        "tenants.reap",
    ] {
        assert!(
            audit.iter().any(|(a, o)| a == action && o == "ok"),
            "{action}: {audit:?}"
        );
    }
    assert!(audit
        .iter()
        .any(|(a, o)| a == "tenants.create" && o == "refused:deployment_touch_required"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenants_own_ops_reach_it_only_where_the_owner_holds_the_role() {
    let h = boot("tenants_own").await;
    // Without the deployment's touch, the console never calls the tenant.
    let refused = h.post("tenants/export", json!({"tenant": MINE})).await;
    assert_eq!(refused.body["error"], "deployment_touch_required");
    h.touch_deployment().await;

    let exported = h.post("tenants/export", json!({"tenant": MINE})).await;
    assert_eq!(exported.status, StatusCode::OK, "{}", exported.body);
    assert!(exported.body["exported"]["bundle"]
        .as_str()
        .unwrap()
        .contains(MINE));
    // A tenant the owner holds no role in: the door refuses.
    let theirs = h.post("tenants/export", json!({"tenant": THEIRS})).await;
    assert_eq!(theirs.status, StatusCode::FORBIDDEN, "{}", theirs.body);
    assert_eq!(theirs.body["doorError"], "access_denied");

    let attached = h
        .post(
            "tenants/attach-bucket",
            json!({"tenant": MINE, "endpoint": "https://s3.example.com", "region": "eu-1",
                   "bucket": "acme-files", "credential": "acme", "platformSealed": true}),
        )
        .await;
    assert_eq!(attached.status, StatusCode::OK, "{}", attached.body);
    assert_eq!(attached.body["attached"]["bucket"], "acme-files");
    let detached = h
        .post("tenants/detach-bucket", json!({"tenant": MINE}))
        .await;
    assert_eq!(detached.status, StatusCode::OK, "{}", detached.body);

    // Delete needs the id typed back.
    let wrong = h
        .post("tenants/delete", json!({"tenant": MINE, "confirm": "Acme"}))
        .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST, "{}", wrong.body);
    assert_eq!(h.door.tenants.status_of(MINE).as_deref(), Some("active"));
    let deleted = h
        .post("tenants/delete", json!({"tenant": MINE, "confirm": MINE}))
        .await;
    assert_eq!(deleted.status, StatusCode::OK, "{}", deleted.body);
    assert_eq!(h.door.tenants.status_of(MINE), None);
}

/// **Owner succession from the console** (owner, 2026-10-24): the open
/// cases with their maturity and objections, a succession opened from the
/// successor's typed id, a paused case resolved, a matured one completed.
/// Every write needs both touches and leaves an audit row; succession runs in
/// the home space, so a tenant the owner holds no role in is reached.
#[tokio::test(flavor = "multi_thread")]
async fn succession_lists_opens_resolves_and_completes_with_both_touches() {
    let h = boot("tenants_succession").await;
    let listed = h.get("tenants/successions").await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    let cases = listed.body["cases"].as_array().unwrap().clone();
    let paused = cases
        .iter()
        .find(|c| c["case"] == CASE_PAUSED)
        .expect("the paused case");
    assert_eq!(paused["paused"], true);
    assert_eq!(paused["objections"][0]["objector"], "prn_mia");
    assert_eq!(
        cases.iter().find(|c| c["case"] == CASE_MATURED).unwrap()["mature"],
        true
    );

    // Writes ask for the deployment owner's touch.
    let refused = h
        .post(
            "tenants/succession/open",
            json!({"tenant": THEIRS, "principal": "prn_new", "evidence": "contract-holder",
                   "departed": ["prn_gone"]}),
        )
        .await;
    assert_eq!(refused.body["error"], "deployment_touch_required");
    h.touch_deployment().await;

    // Open, from the successor's typed id; the door checks the evidence.
    let opened = h
        .post(
            "tenants/succession/open",
            json!({"tenant": THEIRS, "principal": "prn_new", "evidence": "contract-holder",
                   "reference": "MSA-2", "departed": ["prn_gone"]}),
        )
        .await;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["opened"]["outcome"], "opened");
    let bad = h
        .post(
            "tenants/succession/open",
            json!({"tenant": THEIRS, "principal": "prn_new", "evidence": "a hunch",
                   "departed": ["prn_gone"]}),
        )
        .await;
    assert_eq!(bad.body["error"], "deployment_refused", "{}", bad.body);
    let nobody = h
        .post(
            "tenants/succession/open",
            json!({"tenant": THEIRS, "principal": "prn_new", "evidence": "contract-holder",
                   "departed": []}),
        )
        .await;
    assert_eq!(nobody.status, StatusCode::BAD_REQUEST, "{}", nobody.body);
    let odd = h
        .post(
            "tenants/succession/open",
            json!({"tenant": THEIRS, "principal": "a b", "evidence": "contract-holder",
                   "departed": ["prn_gone"]}),
        )
        .await;
    assert_eq!(odd.status, StatusCode::BAD_REQUEST, "{}", odd.body);

    // A paused case cannot be completed until it is resolved.
    let blocked = h
        .post(
            "tenants/succession/complete",
            json!({"tenant": HELD, "case": CASE_PAUSED}),
        )
        .await;
    assert_eq!(
        blocked.body["error"], "deployment_refused",
        "{}",
        blocked.body
    );
    let resumed = h
        .post(
            "tenants/succession/resolve",
            json!({"case": CASE_PAUSED, "decision": "resume", "reason": "evidence-rechecked"}),
        )
        .await;
    assert_eq!(resumed.status, StatusCode::OK, "{}", resumed.body);
    assert_eq!(resumed.body["resolved"]["decision"], "resume");

    // The matured one completes, keeping grants when asked.
    let done = h
        .post(
            "tenants/succession/complete",
            json!({"tenant": THEIRS, "case": CASE_MATURED, "endGrants": false}),
        )
        .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(done.body["succeeded"]["outcome"], "succeeded");
    assert_eq!(done.body["succeeded"]["endsGrants"], false);
    let done = h
        .post(
            "tenants/succession/complete",
            json!({"tenant": HELD, "case": CASE_PAUSED}),
        )
        .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(
        done.body["succeeded"]["delegationsEnded"][0],
        "dlg_1@agent_x"
    );
    let left = h.get("tenants/successions").await.body["cases"]
        .as_array()
        .unwrap()
        .clone();
    assert!(left
        .iter()
        .all(|c| c["case"] != CASE_MATURED && c["case"] != CASE_PAUSED));

    let audit = h.audit().await;
    for action in [
        "tenants.succession_open",
        "tenants.succession_resolve",
        "tenants.succession_complete",
    ] {
        assert!(
            audit.iter().any(|(a, o)| a == action && o == "ok"),
            "{action}: {audit:?}"
        );
    }
}
