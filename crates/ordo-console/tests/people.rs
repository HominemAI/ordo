//! Step 8, the People screen's routes (`plan-console.md` 4.5): a
//! tenant's members, groups, grants, who can call an op and what a member
//! can call, and the writes (invite, change a role, remove, add to a
//! group), all through the console's one client to the door with the
//! console key, in the tenant's own space.
//!
//! The door is the deployment's own sign-in routes, console key ops and
//! door check (`ordo_host::IdentityAccess` with its home space and
//! `TENANT_OPS_OPEN`), served in-process. The People ops behind the check
//! are a stand-in over memory (`support/people_door.rs`);
//! `ordo-host/tests/people_reads.rs` runs the real ones against Postgres.
//!
//! Needs the conformance cluster for the console's own database
//! (`scripts/conformance-pg.sh start`).

mod support {
    pub mod link_owner;
    pub mod people_door;
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
use support::people_door::{self, People, EDITOR, GROUP, MANAGER, VIEWER, YOU};
use support::tenants_door::{Tenants, MINE, THEIRS};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;
const HOME: &str = "home";
const SECRET: &[u8] = b"console-people-test-secret";

// ════════════════════════════ the deployment ═════════════════════════════

struct Door {
    keys: ConsoleKeyOps,
    tenants: Tenants,
    people: People,
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
        People::grant(&mut engine, owner.as_ref());
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
    if people_door::answers(&name) {
        return match d.people.execute(&name, &space, &params) {
            Ok(rows) => Json(json!({"rows": rows})).into_response(),
            Err((code, message)) => door_error(StatusCode::CONFLICT, &code, &message),
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
        people: People::new(),
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
    let db = format!("ordo_console_p_{name}");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let dir =
        std::env::temp_dir().join(format!("ordo-console-people-{name}-{}", std::process::id()));
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

// ═════════════════════════════════ tests ═════════════════════════════════

fn ids(rows: &Value, field: &str) -> Vec<String> {
    rows.as_array()
        .unwrap_or_else(|| panic!("not a list: {rows}"))
        .iter()
        .filter_map(|r| r[field].as_str().map(str::to_string))
        .collect()
}

/// The reads need the deployment owner's relayed touch, as every call into
/// a tenant does; then they answer members, groups, grants, one group,
/// what a member can call and who can call an op. A tenant where the owner
/// holds no role is refused by the door. Past the door's 300 seconds the
/// door refuses again.
#[tokio::test(flavor = "multi_thread")]
async fn the_reads_answer_with_a_fresh_touch_where_the_owner_holds_a_role() {
    let h = boot("reads").await;
    let first = h.get(&format!("people?tenant={MINE}")).await;
    assert_eq!(first.status, StatusCode::FORBIDDEN, "{}", first.body);
    assert_eq!(first.body["error"], "deployment_touch_required");
    h.touch_deployment().await;

    let all = h.get(&format!("people?tenant={MINE}")).await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.body);
    assert_eq!(
        ids(&all.body["members"], "member"),
        [YOU, MANAGER, EDITOR, VIEWER]
    );
    assert_eq!(ids(&all.body["groups"], "group"), [GROUP]);
    assert_eq!(ids(&all.body["grants"], "grant"), ["grt_mail"]);

    let group = h
        .get(&format!("people/group?tenant={MINE}&group={GROUP}"))
        .await;
    assert_eq!(group.status, StatusCode::OK, "{}", group.body);
    assert_eq!(ids(&group.body["members"], "member"), [EDITOR]);

    let can = h
        .get(&format!("people/callable?tenant={MINE}&member={EDITOR}"))
        .await;
    assert_eq!(can.status, StatusCode::OK, "{}", can.body);
    assert!(ids(&can.body["ops"], "via").contains(&"token".to_string()));

    let who = h
        .get(&format!("people/callers?tenant={MINE}&op=notes.delete"))
        .await;
    assert_eq!(who.status, StatusCode::OK, "{}", who.body);
    assert_eq!(
        ids(&who.body["callers"], "principal"),
        ["usr_you", "usr_bea"]
    );

    // A tenant where the owner holds no role.
    let theirs = h.get(&format!("people?tenant={THEIRS}")).await;
    assert_eq!(theirs.status, StatusCode::FORBIDDEN, "{}", theirs.body);
    assert_eq!(theirs.body["doorError"], "access_denied");

    // What the door would not take never leaves the console.
    for bad in [
        "people?tenant=a/b",
        "people/callers?tenant=spc_mine&op=a%20b",
    ] {
        let a = h.get(bad).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{bad}: {}", a.body);
    }

    // The door's own window: 300 seconds after the touch, it refuses.
    h.door.clock.fetch_add(301, Ordering::SeqCst);
    let late = h.get(&format!("people?tenant={MINE}")).await;
    assert_eq!(late.status, StatusCode::FORBIDDEN, "{}", late.body);
    assert_eq!(late.body["doorError"], "access_denied");
}

/// Invite, change a role, add to a group and remove: each needs both
/// touches and leaves an audit row. The last owner cannot be demoted.
#[tokio::test(flavor = "multi_thread")]
async fn the_writes_need_both_touches_and_leave_audit_rows() {
    let h = boot("writes").await;
    let refused = h
        .post("people/invite", json!({"tenant": MINE, "role": "editor"}))
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["error"], "deployment_touch_required");
    h.touch_deployment().await;

    let invited = h
        .post(
            "people/invite",
            json!({"tenant": MINE, "role": "editor", "expires": "7d", "label": "Eve"}),
        )
        .await;
    assert_eq!(invited.status, StatusCode::OK, "{}", invited.body);
    assert!(invited.body["invited"]["ticket"]
        .as_str()
        .unwrap()
        .starts_with("inv_"));
    for bad in [
        json!({"tenant": MINE, "role": "guest"}),
        json!({"tenant": MINE, "expires": "soon"}),
    ] {
        let a = h.post("people/invite", bad.clone()).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{bad}: {}", a.body);
    }

    let changed = h
        .post(
            "people/set-role",
            json!({"tenant": MINE, "member": VIEWER, "role": "editor"}),
        )
        .await;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.body);
    assert_eq!(h.door.people.role_of(VIEWER).as_deref(), Some("editor"));
    let last = h
        .post(
            "people/set-role",
            json!({"tenant": MINE, "member": YOU, "role": "viewer"}),
        )
        .await;
    assert_eq!(last.body["error"], "deployment_refused", "{}", last.body);

    let added = h
        .post(
            "people/add-to-group",
            json!({"tenant": MINE, "group": GROUP, "member": VIEWER}),
        )
        .await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.body);
    assert_eq!(added.body["added"]["outcome"], "added");

    let removed = h
        .post("people/remove", json!({"tenant": MINE, "member": VIEWER}))
        .await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.body);
    assert_eq!(h.door.people.role_of(VIEWER), None);

    // The console owner's own touch runs out: writes ask again.
    h.console.clock().advance(400);
    let stale = h
        .post("people/remove", json!({"tenant": MINE, "member": MANAGER}))
        .await;
    assert_eq!(stale.body["error"], "step_up_required", "{}", stale.body);
    assert_eq!(h.door.people.role_of(MANAGER).as_deref(), Some("manager"));

    let audit = h.audit().await;
    for action in [
        "people.invite",
        "people.set_role",
        "people.add_to_group",
        "people.remove",
    ] {
        assert!(
            audit.iter().any(|(a, o)| a == action && o == "ok"),
            "{action}: {audit:?}"
        );
    }
    assert!(audit
        .iter()
        .any(|(a, o)| a == "people.invite" && o == "refused:deployment_touch_required"));
}

/// Step 8b: suspend and reinstate, make a group, take a member out, delete
/// it, an op's access and one member's, invitations listed and revoked,
/// admission read and set, a member's recovery and its approval, and the
/// tenant's recovery settings. Each write needs both touches and leaves an
/// audit row; a recovery ticket is answered once and never audited.
#[tokio::test(flavor = "multi_thread")]
async fn the_rest_of_the_people_screen_goes_through_the_door() {
    let h = boot("rest").await;
    let refused = h
        .post("people/suspend", json!({"tenant": MINE, "member": VIEWER}))
        .await;
    assert_eq!(refused.body["error"], "deployment_touch_required");
    h.touch_deployment().await;

    // Suspend and reinstate; the last owner is not suspended.
    let s = h
        .post("people/suspend", json!({"tenant": MINE, "member": VIEWER}))
        .await;
    assert_eq!(s.status, StatusCode::OK, "{}", s.body);
    assert_eq!(
        h.door.people.status_of(VIEWER).as_deref(),
        Some("suspended")
    );
    let r = h
        .post(
            "people/reinstate",
            json!({"tenant": MINE, "member": VIEWER}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(h.door.people.status_of(VIEWER).as_deref(), Some("active"));
    let last = h
        .post("people/suspend", json!({"tenant": MINE, "member": YOU}))
        .await;
    assert_eq!(last.body["error"], "deployment_refused", "{}", last.body);

    // Groups.
    let made = h
        .post(
            "people/create-group",
            json!({"tenant": MINE, "name": "Night shift"}),
        )
        .await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.body);
    let night = made.body["created"]["group"].as_str().unwrap().to_string();
    let added = h
        .post(
            "people/add-to-group",
            json!({"tenant": MINE, "group": night, "member": EDITOR}),
        )
        .await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.body);
    let out = h
        .post(
            "people/remove-from-group",
            json!({"tenant": MINE, "group": night, "member": EDITOR}),
        )
        .await;
    assert_eq!(out.status, StatusCode::OK, "{}", out.body);
    let gone = h
        .post(
            "people/delete-group",
            json!({"tenant": MINE, "group": night}),
        )
        .await;
    assert_eq!(gone.status, StatusCode::OK, "{}", gone.body);
    assert_eq!(h.door.people.group_names(), ["Support"]);
    let unnamed = h
        .post("people/create-group", json!({"tenant": MINE, "name": "  "}))
        .await;
    assert_eq!(unnamed.status, StatusCode::BAD_REQUEST);

    // Access: an op's roles and groups, back to default, one member's own.
    let set = h
        .post(
            "people/set-op-access",
            json!({"tenant": MINE, "op": "notes.delete", "roles": "owner", "groups": GROUP}),
        )
        .await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    let p = h
        .get(&format!("people/profile?tenant={MINE}&op=notes.delete"))
        .await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    assert_eq!(p.body["ops"][0]["roles"], json!(["owner"]));
    assert_eq!(p.body["ops"][0]["groups"], json!([GROUP]));
    let reset = h
        .post(
            "people/reset-op-access",
            json!({"tenant": MINE, "op": "notes.delete"}),
        )
        .await;
    assert_eq!(reset.status, StatusCode::OK, "{}", reset.body);
    let own = h
        .post(
            "people/set-member-access",
            json!({"tenant": MINE, "member": VIEWER, "op": "notes.create", "effect": "allow"}),
        )
        .await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.body);
    let p = h.get(&format!("people/profile?tenant={MINE}")).await;
    let create = p.body["ops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["op"] == "notes.create")
        .unwrap()
        .clone();
    assert_eq!(create["allow"], json!([VIEWER]));
    for bad in [
        json!({"tenant": MINE, "op": "notes.delete", "roles": "guest"}),
        json!({"tenant": MINE, "op": "a b"}),
    ] {
        let a = h.post("people/set-op-access", bad.clone()).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{bad}: {}", a.body);
    }
    let a = h
        .post(
            "people/set-member-access",
            json!({"tenant": MINE, "member": VIEWER, "op": "notes.list", "effect": "maybe"}),
        )
        .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);

    // Invitations: listed by id, never the ticket, and revoked.
    let invited = h
        .post(
            "people/invite",
            json!({"tenant": MINE, "role": "viewer", "label": "Quinn"}),
        )
        .await;
    let ticket = invited.body["invited"]["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    let list = h.get(&format!("people/invitations?tenant={MINE}")).await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    assert!(!list.body.to_string().contains(&ticket));
    let id = list.body["invitations"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let revoked = h
        .post(
            "people/revoke-invitation",
            json!({"tenant": MINE, "id": id}),
        )
        .await;
    assert_eq!(revoked.status, StatusCode::OK, "{}", revoked.body);
    let list = h.get(&format!("people/invitations?tenant={MINE}")).await;
    assert_eq!(list.body["invitations"], json!([]));

    // Admission: the four ways in.
    let adm = h.get(&format!("people/admission?tenant={MINE}")).await;
    assert_eq!(adm.status, StatusCode::OK, "{}", adm.body);
    assert_eq!(adm.body["admission"]["open"], false);
    let changed = h
        .post(
            "people/set-admission",
            json!({"tenant": MINE, "open": true, "role": "editor", "firstComer": true}),
        )
        .await;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.body);
    assert_eq!(h.door.people.admission()["open"], true);
    assert_eq!(h.door.people.admission()["role"], "editor");
    let nothing = h
        .post("people/set-admission", json!({"tenant": MINE}))
        .await;
    assert_eq!(nothing.status, StatusCode::BAD_REQUEST);

    // Recovery settings, then a member's recovery with two approvals.
    let pol = h
        .post(
            "people/recovery-policy",
            json!({"tenant": MINE, "approvals": 2, "codesEnabled": false}),
        )
        .await;
    assert_eq!(pol.status, StatusCode::OK, "{}", pol.body);
    assert_eq!(h.door.people.recovery_policy()["approvals"], 2);
    let none = h
        .post("people/recovery-policy", json!({"tenant": MINE}))
        .await;
    assert_eq!(none.status, StatusCode::BAD_REQUEST);
    let opened = h
        .post(
            "people/recover-member",
            json!({"tenant": MINE, "member": EDITOR}),
        )
        .await;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["recovery"]["state"], "pending");
    let case = opened.body["recovery"]["case"]
        .as_str()
        .unwrap()
        .to_string();
    let approved = h
        .post(
            "people/approve-recovery",
            json!({"tenant": MINE, "member": EDITOR, "case": case}),
        )
        .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.body);
    assert_eq!(approved.body["approval"]["outcome"], "approved");
    let collected = h
        .post(
            "people/recover-member",
            json!({"tenant": MINE, "member": EDITOR, "case": case}),
        )
        .await;
    assert_eq!(collected.status, StatusCode::OK, "{}", collected.body);
    let rticket = collected.body["recovery"]["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    let owner = h
        .post(
            "people/recover-member",
            json!({"tenant": MINE, "member": YOU}),
        )
        .await;
    assert_eq!(owner.body["error"], "deployment_refused", "{}", owner.body);

    let audit = h.audit().await;
    for action in [
        "people.suspend",
        "people.reinstate",
        "people.create_group",
        "people.remove_from_group",
        "people.delete_group",
        "people.set_op_access",
        "people.reset_op_access",
        "people.set_member_access",
        "people.revoke_invitation",
        "people.set_admission",
        "people.recovery_policy",
        "people.recover_member",
        "people.approve_recovery",
    ] {
        assert!(
            audit.iter().any(|(a, o)| a == action && o == "ok"),
            "{action}: {audit:?}"
        );
    }
    // Neither ticket is in the console's own record.
    let all: Vec<Option<String>> = h
        .console
        .store()
        .pool()
        .rows("SELECT detail FROM audit")
        .await
        .expect("audit")
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    let text = format!("{all:?}");
    assert!(
        !text.contains(&rticket) && !text.contains(&ticket),
        "{text}"
    );
}
