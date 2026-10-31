//! Step 7, the Usage screen's routes (`plan-console.md` 4.9 and 4.11):
//! usage per tenant and for the whole deployment over a range, the quotas
//! and a change to one, and the slow ops table, all through the console's
//! one client to the door with the console key.
//!
//! The door is the deployment's own sign-in routes, console key ops and
//! door check (`ordo_host::IdentityAccess` with its home space), served
//! in-process. The answers behind the check are stand-ins over memory
//! (`support/tenants_door.rs`, `support/usage_door.rs`);
//! `ordo-host/tests/deployment_quotas.rs` runs the real quota ops against
//! Postgres.
//!
//! Needs the conformance cluster for the console's own database
//! (`scripts/conformance-pg.sh start`).

mod support {
    pub mod link_owner;
    pub mod tenants_door;
    pub mod usage_door;
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
use support::tenants_door::{Tenants, HELD, MINE, THEIRS};
use support::usage_door::{Usage, PERIODS, USAGE_OP};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;
const HOME: &str = "home";
const SECRET: &[u8] = b"console-usage-test-secret";

// ════════════════════════════ the deployment ═════════════════════════════

struct Door {
    keys: ConsoleKeyOps,
    tenants: Tenants,
    usage: Usage,
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
            Usage::home_profile(Tenants::home_profile(
                AccessProfile::new()
                    .allow(Role::Owner, ADD)
                    .allow(Role::Owner, LIST)
                    .allow(Role::Owner, REVOKE),
            )),
        );
        let owner = self.owner.lock().unwrap().clone();
        if let Some(o) = &owner {
            engine.set_membership(&space, o, Role::Owner);
        }
        engine.require_step_up(ADD);
        engine.require_step_up(REVOKE);
        self.tenants.grant(&mut engine, owner.as_ref());
        Usage::grant(&mut engine);
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
    let answer = if support::usage_door::OPS.contains(&name.as_str()) {
        d.usage.execute(&name, &params)
    } else {
        d.tenants.execute(&name, &space, &params, &principal)
    };
    match answer {
        Ok(rows) => Json(json!({"rows": rows})).into_response(),
        Err((code, message)) => door_error(StatusCode::CONFLICT, &code, &message),
    }
}

/// `GET /usage`: checked in the deployment's own space, whichever space it
/// names, as the real route is.
async fn usage_route(
    State(d): State<Arc<Door>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    if !matches!(
        d.access().check(&bearer, HOME, USAGE_OP),
        AccessDecision::Allow { .. }
    ) {
        return door_error(StatusCode::FORBIDDEN, "access_denied", "not permitted");
    }
    let n = |k: &str| query.get(k).and_then(|v| v.parse::<u64>().ok());
    let (Some(space), Some(from), Some(to)) = (query.get("space"), n("from"), n("to")) else {
        return door_error(
            StatusCode::BAD_REQUEST,
            "invalid_param",
            "space, from and to",
        );
    };
    (
        StatusCode::OK,
        [("content-type", "application/x-ndjson")],
        d.usage.usage(space, from, to),
    )
        .into_response()
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
        usage: Usage::new(START),
        revocations,
        owner: Mutex::new(None),
        clock,
    });
    let app = Router::new()
        .route("/auth/complete", post(complete))
        .route("/op/{name}", any(op))
        .route("/health/ready", axum::routing::get(ready))
        .route("/usage", axum::routing::get(usage_route))
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
    let db = format!("ordo_console_u_{name}");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let dir =
        std::env::temp_dir().join(format!("ordo-console-usage-{name}-{}", std::process::id()));
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

    async fn quotas(&self) -> Vec<Value> {
        let a = self.get("usage/quotas").await;
        assert_eq!(a.status, StatusCode::OK, "{}", a.body);
        a.body["quotas"].as_array().unwrap().clone()
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

/// A day ending at the door's clock.
const DAY: &str = "from=1799913600&to=1800000000";

// ═════════════════════════════════ tests ═════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn usage_reads_one_tenant_and_the_whole_deployment_with_each_tenants_share() {
    let h = boot("usage_read").await;

    // One tenant: its periods, oldest first.
    let one = h.get(&format!("usage?{DAY}&tenant={MINE}")).await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.body);
    assert_eq!(one.body["scope"], "tenant");
    let periods = one.body["periods"].as_array().unwrap();
    assert_eq!(periods.len() as u64, PERIODS);
    assert!(periods[0]["start"].as_u64() < periods[1]["start"].as_u64());
    assert_eq!(periods[0]["requests"], 10);
    assert_eq!(periods[0]["rowsReturned"], 20);
    assert_eq!(one.body["total"]["requests"], 10 * PERIODS);

    // The deployment: its own space and every tenant, summed, with each
    // one's share, and the period a tenant's database did not answer.
    let all = h.get(&format!("usage?{DAY}")).await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.body);
    assert_eq!(all.body["scope"], "deployment");
    assert_eq!(all.body["home"], HOME);
    assert_eq!(all.body["total"]["requests"], (10 + 5 + 1) * PERIODS);
    let by = all.body["byTenant"].as_array().unwrap();
    assert_eq!(find(by, MINE)["requests"], 10 * PERIODS);
    assert_eq!(find(by, MINE)["name"], "Acme");
    assert_eq!(find(by, THEIRS)["requests"], 5 * PERIODS);
    assert_eq!(find(by, HOME)["requests"], PERIODS);
    let gaps = all.body["gaps"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert_eq!(gaps[0]["space"], HELD);
    assert!(all.body["missing"].as_array().unwrap().is_empty());

    // What the console would never send to the door.
    for bad in [
        "usage?from=10&to=5".to_string(),
        "usage?from=0&to=99999999".to_string(),
        format!("usage?{DAY}&tenant=a/b"),
        "usage".to_string(),
    ] {
        let a = h.get(&bad).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{bad}: {}", a.body);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_quota_change_needs_both_touches_and_leaves_an_audit_row() {
    let h = boot("usage_quota").await;
    let quotas = h.quotas().await;
    assert_eq!(quotas.len(), 1);
    assert_eq!(quotas[0]["space"], MINE);
    assert_eq!(quotas[0]["used"], 40);

    let change = json!({"space": MINE, "quantity": "rows_returned", "limit": 5000,
                        "grace": 500, "periodSecs": 86400});
    // The console key alone: the deployment asks for the owner's touch.
    let refused = h.post("usage/quotas/set", change.clone()).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["error"], "deployment_touch_required");
    assert!(h.door.usage.quota_of(MINE, "rows_returned").is_none());

    h.touch_deployment().await;
    let set = h.post("usage/quotas/set", change.clone()).await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    assert_eq!(set.body["quota"]["limit"], 5000);
    assert_eq!(h.quotas().await.len(), 2);
    assert_eq!(
        h.door.usage.quota_of(MINE, "rows_returned").unwrap()["periodSecs"],
        86400
    );

    // The door's own refusal comes back in its words.
    let writes = h
        .post(
            "usage/quotas/set",
            json!({"space": MINE, "quantity": "writes", "limit": 1, "grace": 0,
                   "periodSecs": 60}),
        )
        .await;
    assert_eq!(
        writes.body["error"], "deployment_refused",
        "{}",
        writes.body
    );
    // What the console checks itself.
    for bad in [
        json!({"space": "a b", "quantity": "requests", "limit": 1, "grace": 0, "periodSecs": 60}),
        json!({"space": MINE, "quantity": "requests", "limit": -1, "grace": 0, "periodSecs": 60}),
        json!({"space": MINE, "quantity": "requests"}),
    ] {
        let a = h.post("usage/quotas/set", bad.clone()).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{bad}: {}", a.body);
    }

    // The console owner's own touch runs out: a change asks again.
    h.console.clock().advance(400);
    let stale = h.post("usage/quotas/set", change).await;
    assert_eq!(stale.body["error"], "step_up_required", "{}", stale.body);

    let audit = h.audit().await;
    assert!(
        audit
            .iter()
            .any(|(a, o)| a == "usage.set_quota" && o == "ok"),
        "{audit:?}"
    );
    assert!(
        audit
            .iter()
            .any(|(a, o)| a == "usage.set_quota" && o == "refused:deployment_touch_required"),
        "{audit:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_ops_name_the_node_and_carry_mean_time_and_tier() {
    let h = boot("usage_slow").await;
    let a = h.get("usage/slow-ops").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["node"], "node-a");
    assert_eq!(a.body["since"], START - 7200);
    let ops = a.body["ops"].as_array().unwrap();
    assert_eq!(ops.len(), 3);
    let op = |name: &str| ops.iter().find(|o| o["op"] == name).unwrap().clone();
    assert_eq!(op("listTasks")["meanMicros"], 5_000);
    assert_eq!(op("listTasks")["tier"], "sql");
    assert_eq!(op("searchNotes")["meanMicros"], 200_000);
    assert_eq!(op("searchNotes")["tier"], "bounded-host");

    // With no session, nothing.
    let a = h
        .send(
            Request::get("/d/main/api/usage/slow-ops")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_ne!(a.status, StatusCode::OK, "{}", a.body);
}
