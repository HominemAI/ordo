//! The deployment's door for the console's end-to-end tests
//! (`clients/ordo-console-web/e2e`): `ordo-host`'s own sign-in routes and
//! console key ops over memory stores, on a loopback port, as
//! `tests/door_link.rs` serves them in-process, and the tenant ops of
//! step 5 over memory (`tests/support/tenants_door.rs`). The other stand-in is
//! admission: the last person to register a passkey here (with no `space`)
//! is the deployment's owner, as `ORDO_DEPLOYMENT_OWNERS` names one at a
//! real deployment. The console then links by sign-in; the claim step was
//! removed with the claim code (2026-10-30).
//!
//! A tenant's People ops (step 8) answer over memory too
//! (`tests/support/people_door.rs`).
//!
//! `std.deployment.settings` answers from the stand-in host's own
//! environment file (`support/settings_door.rs`).
//!
//! The deploy ops (`std.deployment.apply`, `versions`, `activate`, `plan`)
//! are `ordo-host`'s own over a real Postgres database, the one
//! `ORDO_PG_HOST`, `ORDO_PG_PORT`, `ORDO_PG_USER` and `ORDO_PG_DB` name
//! (stack.sh makes it fresh). `/health/deploy` answers for one node that
//! serves whatever is active when asked.
//!
//! With `ORDO_IDENTITY_URL`, `ORDO_IDENTITY_ORIGIN` and
//! `ORDO_IDENTITY_DEPLOYMENT` set, it also verifies the identity service's
//! tokens, as a deployment does (`ordo_host::identity_keys`), and answers
//! `presence_required` when a person's touch is too old for an op. For the
//! console on the identity service (`e2e/stack-identity.sh`).
//! `E2E_DOOR_TOUCH_SECS` shortens the touch window, so a flow sees the ask
//! without waiting 5 minutes.
//!
//! An example, not a binary, so it may use `ordo-host` (a dev-dependency):
//! the console itself never links it. Every value here is a test value.
//!
//! ```text
//! E2E_DOOR_ADDR=127.0.0.1:4682 E2E_DOOR_ORIGIN=http://localhost:4680 \
//!   cargo run -p ordo-console --example e2e_door
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use ordo_api::{AccessChecker, AccessDecision, BoundParams};
use ordo_host::apply_op::{ApplyOps, APPLY};
use ordo_host::auth::AuthState;
use ordo_host::console_keys::{ConsoleKeyOps, ConsoleKeyStore, ConsoleKeys, ADD, LIST, REVOKE};
use ordo_host::deploy_ops::{ACTIVATE, PLAN, VERSIONS};
use ordo_host::node_status::NodeStatus;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{
    AccessEngine, AccessProfile, MemorySessionRevocations, PasskeyProvider, PrincipalId,
    ProviderRegistry, RelyingParty, Role, SpaceId, TokenKey,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const HOME: &str = "home";
const SECRET: &[u8] = b"e2e-door-test-secret";
const HEALTH_DEEP: &str = "platform.health.deep";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The tenant ops behind the door's check, over memory (step 5). Shared
/// with `tests/tenants.rs`.
#[path = "../tests/support/tenants_door.rs"]
mod tenants_door;
use tenants_door::Tenants;

/// `std.deployment.settings` (G7, step 6): `ordo-host`'s own answer over
/// the environment the supervised stand-in host wrote at its last start
/// (`E2E_DOOR_CHILD_ENV`), with what `E2E_DOOR_STORED` (`key=value,...`)
/// says platform ops stored. Shared with `tests/settings.rs`.
#[path = "../tests/support/settings_door.rs"]
mod settings_door;
use settings_door::{SettingsDoor, SETTINGS};
/// Usage, quotas and observed cost behind the door's check, over memory
/// (step 7). Shared with `tests/usage.rs`.
#[path = "../tests/support/usage_door.rs"]
mod usage_door;
use usage_door::Usage;
/// A tenant's People ops behind the door's check, over memory (step 8).
/// Shared with `tests/people.rs`.
#[path = "../tests/support/people_door.rs"]
mod people_door;
use people_door::People;
/// Links, shares and the security records behind the door's check, over
/// memory (step 9). Shared with `tests/shares.rs`.
#[path = "../tests/support/shares_door.rs"]
mod shares_door;
use shares_door::Shares;

struct Door {
    ops: ConsoleKeyOps,
    deploys: Arc<ApplyOps>,
    node: Arc<NodeStatus>,
    tenants: Tenants,
    settings: SettingsDoor,
    usage: Usage,
    people: People,
    shares: Shares,
    revocations: Arc<MemorySessionRevocations>,
    owner: Mutex<Option<PrincipalId>>,
    auth_router: Router,
    /// The identity service's keys, when its settings are given.
    identity: Option<Arc<ordo_host::identity_keys::IdentityKeys>>,
    /// The touch window, `E2E_DOOR_TOUCH_SECS`, when given.
    touch_secs: Option<u64>,
}

impl Door {
    fn access(&self) -> ordo_host::IdentityAccess {
        let space = SpaceId(HOME.into());
        let mut engine = AccessEngine::new();
        engine.set_profile(
            &space,
            Shares::home_profile(Usage::home_profile(Tenants::home_profile(
                AccessProfile::new()
                    .allow(Role::Owner, ADD)
                    .allow(Role::Owner, LIST)
                    .allow(Role::Owner, REVOKE)
                    .allow(Role::Owner, HEALTH_DEEP)
                    .allow(Role::Owner, APPLY)
                    .allow(Role::Owner, VERSIONS)
                    .allow(Role::Owner, ACTIVATE)
                    .allow(Role::Owner, PLAN)
                    .allow(Role::Owner, SETTINGS),
            ))),
        );
        let owner = self.owner.lock().unwrap().clone();
        if let Some(o) = &owner {
            engine.set_membership(&space, o, Role::Owner);
        }
        for change in [ADD, REVOKE, APPLY, ACTIVATE, PLAN] {
            engine.require_step_up(change);
        }
        self.tenants.grant(&mut engine, owner.as_ref());
        People::grant(&mut engine, owner.as_ref());
        Usage::grant(&mut engine);
        Shares::grant(&mut engine, owner.as_ref());
        if let Some(secs) = self.touch_secs {
            engine = engine.with_step_up_max_age(secs);
        }
        let access = ordo_host::IdentityAccess::new(
            engine,
            TokenKey::from_secret(SECRET),
            Box::new(now),
            Arc::new(ordo_host::metrics::Metrics::new()),
        )
        .with_revocations(self.revocations.clone())
        .with_console_home(HOME);
        match &self.identity {
            Some(keys) => access.with_identity_keys(keys.clone()),
            None => access,
        }
    }
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string()
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
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let space = headers
        .get("x-ordo-space")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let access = d.access();
    let principal = match access.check(&bearer(&headers), &space, &name) {
        AccessDecision::Allow { principal, .. } => principal,
        _ if access.presence_required(&bearer(&headers), &space, &name) => {
            return door_error(
                StatusCode::FORBIDDEN,
                "presence_required",
                "this needs a fresh passkey touch",
            )
        }
        _ => return door_error(StatusCode::FORBIDDEN, "access_denied", "not permitted"),
    };
    let params: serde_json::Map<String, Value> = if body.is_empty() {
        query.into_iter().map(|(k, v)| (k, json!(v))).collect()
    } else {
        serde_json::from_slice(&body).unwrap_or_default()
    };
    if name == SETTINGS {
        return Json(json!({"rows": d.settings.rows()})).into_response();
    }
    let deploy_op = name == APPLY || ordo_host::deploy_ops::OPS.contains(&name.as_str());
    if shares_door::OPS.contains(&name.as_str()) {
        return match d.shares.execute(&name, &params) {
            Ok(rows) => Json(json!({"rows": rows})).into_response(),
            Err((code, message)) => door_error(StatusCode::CONFLICT, &code, &message),
        };
    }
    if usage_door::OPS.contains(&name.as_str()) {
        return match d.usage.execute(&name, &params) {
            Ok(rows) => Json(json!({"rows": rows})).into_response(),
            Err((code, message)) => door_error(StatusCode::CONFLICT, &code, &message),
        };
    }
    if people_door::answers(&name) {
        return match d.people.execute(&name, &space, &params) {
            Ok(rows) => Json(json!({"rows": rows})).into_response(),
            Err((code, message)) => door_error(StatusCode::CONFLICT, &code, &message),
        };
    }
    if !deploy_op && ![ADD, LIST, REVOKE].contains(&name.as_str()) {
        return match d.tenants.execute(&name, &space, &params, &principal) {
            Ok(rows) => Json(json!({"rows": rows})).into_response(),
            Err((code, message)) => door_error(StatusCode::CONFLICT, &code, &message),
        };
    }
    let bound = BoundParams(params);
    let done = if name == APPLY {
        d.deploys.apply(&space, &bound, &principal).await
    } else if deploy_op {
        ordo_host::deploy_ops::execute(&d.deploys, &name, &space, &bound, &principal).await
    } else {
        d.ops.execute(&name, &space, &bound, &principal).await
    };
    match done {
        Ok(r) => Json(json!({"rows": r.rows})).into_response(),
        Err(e) => door_error(StatusCode::CONFLICT, e.code.as_str(), &e.message),
    }
}

/// `GET /usage`: checked in the deployment's own space, whichever space it
/// names, as the real route is (step 7).
async fn usage(
    State(d): State<Arc<Door>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !matches!(
        d.access()
            .check(&bearer(&headers), HOME, usage_door::USAGE_OP),
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

/// `GET /security/events`: checked in the space it names, as the real
/// route is (step 9).
async fn security_events(
    State(d): State<Arc<Door>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let space = query.get("space").cloned().unwrap_or_default();
    if !matches!(
        d.access()
            .check(&bearer(&headers), &space, shares_door::SECEVENTS_OP),
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

async fn health_deep(State(d): State<Arc<Door>>, headers: HeaderMap) -> Response {
    match d.access().check(&bearer(&headers), HOME, HEALTH_DEEP) {
        AccessDecision::Allow { .. } => Json(json!({
            "status": "ok",
            "boot": {"revision": "1", "applied_by": "e2e"},
        }))
        .into_response(),
        _ => door_error(StatusCode::FORBIDDEN, "access_denied", "not permitted"),
    }
}

/// `/health/deploy`, with the one node serving whatever is active.
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
    // A registration on the deployment's own page: each spec file names
    // its own browser's passkey the owner, as ORDO_DEPLOYMENT_OWNERS and a
    // restart would.
    if status == StatusCode::OK
        && body.get("space").is_none()
        && body["kind"] == "passkey_registration"
    {
        let token = answer["token"].as_str().unwrap_or_default();
        if let Ok(claims) = ordo_identity::verify(
            &TokenKey::from_secret(SECRET),
            token,
            ordo_identity::Timestamp(now()),
        ) {
            *d.owner.lock().unwrap() = Some(claims.principal);
        }
    }
    if status == StatusCode::OK && body.get("space").is_some() {
        let token = answer["token"].as_str().unwrap_or_default();
        let Ok(claims) = ordo_identity::verify(
            &TokenKey::from_secret(SECRET),
            token,
            ordo_identity::Timestamp(now()),
        ) else {
            return door_error(StatusCode::UNAUTHORIZED, "authentication_failed", "no");
        };
        let owner = d.owner.lock().unwrap();
        let outcome = if owner.as_ref() == Some(&claims.principal) {
            "already-member"
        } else {
            "refused"
        };
        answer["admission"] = json!({"space": HOME, "outcome": outcome});
    }
    (status, Json(answer)).into_response()
}

#[tokio::main]
async fn main() {
    let addr = std::env::var("E2E_DOOR_ADDR").unwrap_or_else(|_| "127.0.0.1:4682".into());
    let origin =
        std::env::var("E2E_DOOR_ORIGIN").unwrap_or_else(|_| "http://localhost:4680".into());
    let rp_id = std::env::var("E2E_DOOR_RP_ID").unwrap_or_else(|_| "localhost".into());

    let rp = RelyingParty::new(&rp_id, &[origin.as_str()]).expect("relying party");
    let ceremonies = Arc::new(ordo_identity::MemoryCeremonyStore::new());
    let passkey = Arc::new(PasskeyProvider::with_stores(
        rp,
        Arc::new(ordo_identity::MemoryPasskeyStore::new()),
        ceremonies,
    ));
    let mut registry = ProviderRegistry::new();
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("passkey");
    let revocations = Arc::new(MemorySessionRevocations::new());
    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let state = AuthState::new(
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
    let keys = Arc::new(ConsoleKeys::new(
        ConsoleKeyStore::memory(),
        ConsoleKeys::challenge_key_from(SECRET),
        900,
        HOME,
    ));
    state.wire_console_keys(keys.clone());
    let auth = Arc::new(state);
    let app = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("e2e door: the app database (ORDO_PG_*)");
    let deploys = ApplyOps::new(
        app.clone(),
        Arc::new(ordo_exec::route::SinglePool::new(app.clone())),
        HOME,
    );
    let node = NodeStatus::new(app, "node-1".into(), std::time::Duration::from_secs(5));
    let identity = match ordo_host::identity_keys::IdentitySettings::from_env() {
        Ok(Some(settings)) => {
            let keys = Arc::new(ordo_host::identity_keys::IdentityKeys::new(
                settings,
                ordo_host::identity_keys::IdentityKeyStore::memory(),
                Arc::new(now),
            ));
            if let Err(e) = keys.boot().await {
                eprintln!("e2e door: the identity service's keys were not fetched yet: {e}");
            }
            keys.spawn_refresher();
            Some(keys)
        }
        Ok(None) => None,
        Err((var, why)) => panic!("e2e door: {var} {why}"),
    };
    let touch_secs = std::env::var("E2E_DOOR_TOUCH_SECS")
        .ok()
        .and_then(|v| v.parse().ok());
    let door = Arc::new(Door {
        deploys,
        node,
        auth_router: ordo_host::auth::router(Some(auth)),
        ops: ConsoleKeyOps {
            keys,
            revocations: Some(revocations.clone()),
            now: Arc::new(now),
            stream: Some(recorder),
        },
        tenants: Tenants::new(now()),
        settings: {
            let s = SettingsDoor::new(std::env::var("E2E_DOOR_CHILD_ENV").ok().map(Into::into));
            for pair in std::env::var("E2E_DOOR_STORED")
                .unwrap_or_default()
                .split(',')
            {
                if let Some((k, v)) = pair.split_once('=') {
                    s.store(k.trim(), v.trim(), "e2e", now());
                }
            }
            s
        },
        usage: Usage::new(now()),
        people: People::new(),
        shares: Shares::new(now()),
        revocations,
        owner: Mutex::new(None),
        identity,
        touch_secs,
    });
    let app = Router::new()
        .route("/auth/complete", post(complete))
        .route("/op/{name}", any(op))
        .route(
            "/health/ready",
            get(|d: State<Arc<Door>>| async move {
                Json(json!({"status": "ready", "quarantined": d.tenants.quarantined()}))
            }),
        )
        .route("/health/deep", get(health_deep))
        .route("/usage", get(usage))
        .route("/security/events", get(security_events))
        .route("/health/deploy", get(health_deploy))
        .with_state(door.clone())
        .fallback_service(door.auth_router.clone());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("e2e door: cannot listen on {addr}: {e}"));
    eprintln!("e2e door: listening on http://{addr} for {origin}");
    axum::serve(listener, app).await.expect("serve");
}
