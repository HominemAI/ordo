//! Step 9, the Shares and links screen and the Security log screen
//! (`plan-console.md` 4.7 and 4.8): a tenant's links with their state, its
//! shares, revoking either, the deployment's own security record (G8), a
//! tenant's record and the console's own audit log, all through the
//! console's one client to the door with the console key.
//!
//! The door is the deployment's own sign-in routes, console key ops and
//! door check (`ordo_host::IdentityAccess` with its home space, and its
//! rule that a console session leaves the home space only for the listed
//! ops with a fresh touch), served in-process. The answers behind the
//! check are stand-ins over memory (`support/tenants_door.rs`,
//! `support/shares_door.rs`); `ordo-host/tests/deployment_security.rs`
//! runs the real G8 against Postgres.
//!
//! Needs the conformance cluster for the console's own database
//! (`scripts/conformance-pg.sh start`).

mod support {
    pub mod link_owner;
    pub mod shares_door;
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
use support::shares_door::{Shares, GONE, LIVE, SEAT, SECEVENTS_OP, USED_UP};
use support::tenants_door::{Tenants, MINE, THEIRS};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;
const HOME: &str = "home";
const SECRET: &[u8] = b"console-shares-test-secret";

// ════════════════════════════ the deployment ═════════════════════════════

struct Door {
    keys: ConsoleKeyOps,
    tenants: Tenants,
    shares: Shares,
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
            Shares::home_profile(Tenants::home_profile(
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
        Shares::grant(&mut engine, owner.as_ref());
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
    let answer = if support::shares_door::OPS.contains(&name.as_str()) {
        d.shares.execute(&name, &params)
    } else {
        d.tenants.execute(&name, &space, &params, &principal)
    };
    match answer {
        Ok(rows) => Json(json!({"rows": rows})).into_response(),
        Err((code, message)) => door_error(StatusCode::CONFLICT, &code, &message),
    }
}

/// `GET /security/events`: checked in **the space it names**, as the real
/// route is, so a console session reaches it only with a fresh touch.
async fn events_route(
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
    let space = query.get("space").cloned().unwrap_or_default();
    if !matches!(
        d.access().check(&bearer, &space, SECEVENTS_OP),
        AccessDecision::Allow { .. }
    ) {
        return door_error(StatusCode::FORBIDDEN, "access_denied", "not permitted");
    }
    let n = |k: &str| query.get(k).and_then(|v| v.parse::<usize>().ok());
    Json(
        d.shares
            .tenant_events(&space, n("from").unwrap_or(0), n("limit").unwrap_or(200)),
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
        shares: Shares::new(START),
        revocations,
        owner: Mutex::new(None),
        clock,
    });
    let app = Router::new()
        .route("/auth/complete", post(complete))
        .route("/op/{name}", any(op))
        .route("/health/ready", axum::routing::get(ready))
        .route("/security/events", axum::routing::get(events_route))
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
    let db = format!("ordo_console_s_{name}");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let dir =
        std::env::temp_dir().join(format!("ordo-console-shares-{name}-{}", std::process::id()));
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

fn find<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {rows:?}"))
}

fn ids(rows: &Value) -> Vec<u64> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| r["index"].as_u64().unwrap())
        .collect()
}

// ═════════════════════════════════ tests ═════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn a_tenants_links_and_shares_show_with_a_touch_and_only_where_the_owner_holds_a_role() {
    let h = boot("list").await;
    let path = format!("shares?tenant={MINE}");
    // The console key alone stays in the deployment's own space.
    let refused = h.get(&path).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.body["error"], "deployment_touch_required");

    h.touch_deployment().await;
    let a = h.get(&path).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let links = a.body["links"].as_array().unwrap();
    assert_eq!(links.len(), 3);
    let live = find(links, LIVE);
    assert_eq!(live["state"], "live");
    assert_eq!(live["opens"], 5);
    assert_eq!(live["devices"], 3);
    assert_eq!(live["locked"], true);
    assert_eq!(live["hasPassword"], true);
    assert_eq!(find(links, USED_UP)["state"], "used-up");
    assert_eq!(find(links, GONE)["state"], "revoked");
    assert_eq!(a.body["shares"][0]["seat"], SEAT);
    // No secret and no hash has a field to arrive in.
    let text = a.body.to_string();
    assert!(!text.contains("hash") && !text.contains("secret"), "{text}");

    // A tenant the owner holds no role in stays closed, touch or not.
    let theirs = h.get(&format!("shares?tenant={THEIRS}")).await;
    assert_eq!(theirs.status.as_u16() / 100, 4, "{}", theirs.body);
    assert!(theirs.body.get("links").is_none());
    let bad = h.get("shares?tenant=a%20b").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_link_or_a_share_needs_both_touches_and_leaves_an_audit_row() {
    let h = boot("revoke").await;
    let body = json!({"tenant": MINE, "link": LIVE});
    let refused = h.post("shares/revoke-link", body.clone()).await;
    assert_eq!(refused.body["error"], "deployment_touch_required");
    assert_eq!(h.door.shares.link_state(LIVE).as_deref(), Some("live"));

    h.touch_deployment().await;
    let done = h.post("shares/revoke-link", body).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(h.door.shares.link_state(LIVE).as_deref(), Some("revoked"));
    let gone = h
        .post("shares/revoke-share", json!({"tenant": MINE, "seat": SEAT}))
        .await;
    assert_eq!(gone.status, StatusCode::OK, "{}", gone.body);
    assert_eq!(h.door.shares.share_count(), 0);
    // The door's own refusal comes back in its words.
    let again = h
        .post("shares/revoke-share", json!({"tenant": MINE, "seat": SEAT}))
        .await;
    assert_eq!(again.body["error"], "deployment_refused", "{}", again.body);

    // The console owner's own touch runs out: a revoke asks again.
    h.console.clock().advance(400);
    let stale = h
        .post(
            "shares/revoke-link",
            json!({"tenant": MINE, "link": USED_UP}),
        )
        .await;
    assert_eq!(stale.body["error"], "step_up_required", "{}", stale.body);

    let audit = h.audit().await;
    for (action, outcome) in [
        ("shares.revoke_link", "ok"),
        ("shares.revoke_share", "ok"),
        ("shares.revoke_link", "refused:deployment_touch_required"),
    ] {
        assert!(
            audit.iter().any(|(a, o)| a == action && o == outcome),
            "{action} {outcome}: {audit:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_security_log_reads_the_deployment_a_tenant_and_the_console() {
    let h = boot("log").await;
    // G8: the deployment's own record, newest first, with no touch.
    let a = h.get("security?source=deployment").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(ids(&a.body["events"]), vec![4, 3, 2, 1, 0]);
    assert!(a.body["before"].is_null(), "one page: {}", a.body);
    let a = h.get("security?source=deployment&limit=2").await;
    assert_eq!(ids(&a.body["events"]), vec![4, 3]);
    assert_eq!(a.body["before"], 3);
    let a = h.get("security?source=deployment&limit=2&before=3").await;
    assert_eq!(ids(&a.body["events"]), vec![2, 1]);
    let a = h.get("security?source=deployment&kind=enrolment").await;
    assert_eq!(ids(&a.body["events"]), vec![4, 2]);
    let a = h
        .get(&format!("security?source=deployment&tenant={MINE}"))
        .await;
    assert_eq!(ids(&a.body["events"]), vec![2, 1]);

    // A tenant's own record leaves the home space: it needs a touch.
    let path = format!("security?source=tenant&tenant={MINE}");
    let refused = h.get(&path).await;
    assert_eq!(refused.body["error"], "deployment_touch_required");
    h.touch_deployment().await;
    let a = h.get(&path).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["events"][0]["count"], 2);
    let theirs = h
        .get(&format!("security?source=tenant&tenant={THEIRS}"))
        .await;
    assert_eq!(theirs.status.as_u16() / 100, 4, "{}", theirs.body);

    // The console's own audit log, newest first, by action.
    let a = h.get("security?source=console").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let events = a.body["events"].as_array().unwrap();
    assert!(!events.is_empty());
    assert!(events
        .windows(2)
        .all(|w| w[0]["id"].as_u64() > w[1]["id"].as_u64()));
    let a = h.get("security?source=console&action=door.link").await;
    let events = a.body["events"].as_array().unwrap();
    assert!(!events.is_empty());
    assert!(events
        .iter()
        .all(|e| e["action"].as_str().unwrap().starts_with("door.link")));

    let bad = h.get("security?source=elsewhere").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}
