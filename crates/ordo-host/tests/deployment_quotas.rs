//! Console step 7 (`plan-console.md` 4.9 and 7), through the real door
//! against real Postgres:
//!
//! - **G9**, `std.deployment.quotas` and `std.deployment.setQuota`: `ordo
//!   quota` through the door, on the same engine (`quotas::set_limit`).
//!   The deployment owner's, in the deployment's own space, a write with a
//!   touch.
//! - **G12, the quotas part**: every `setQuota` leaves a record, stored or
//!   refused.
//! - `/usage` answers the deployment owner, which it did not before: its
//!   op was never granted (`platform.usage`).
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{
    router, AccessChecker, AccessDecision, ApiState, MemoryBus, MemoryReplayCache, PageKey,
};
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::pause::PausePolicy;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::FixtureExecutor;
use ordo_host::{quotas, tenants};
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use ordo_secevent::Recorder;
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"deployment-quotas-test-secret-32";
/// The conformance seed's owner of the tenant `conformance`.
const TENANT_OWNER: &str = "mem_o";
/// The deployment owner, an owner of the deployment's own space.
const DEPLOYMENT_OWNER: &str = "dep_owner";
/// A manager of the deployment's own space: not the owner.
const MANAGER: &str = "dep_manager";
const HOME: &str = "home";

struct Harness {
    state: ApiState,
    access: Arc<ordo_host::IdentityAccess>,
    pool: ordo_exec::Pool,
    stream: Arc<Recorder>,
    /// The conformance tenant's id.
    space: String,
}

async fn harness(db: &str) -> Harness {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    pool.ensure_platform_tables()
        .await
        .expect("platform tables");
    tenants::ensure_table(&pool).await.expect("registry");
    quotas::ensure_table(&pool).await.expect("quotas");
    let space = ordo_conformance::fixture_space().0;
    tenants::reserve(&pool, &space, "Conformance", TENANT_OWNER, NOW, None)
        .await
        .expect("reserved");
    assert!(tenants::activate(&pool, &space, NOW).await.expect("active"));

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");
    let mut rows = pool.rows(ordo_host::MEMBERS_SQL).await.expect("members");
    for (who, role) in [(DEPLOYMENT_OWNER, "owner"), (MANAGER, "manager")] {
        rows.push(vec![
            Some(HOME.into()),
            Some(who.into()),
            Some(role.into()),
            Some("active".into()),
            Some(who.into()),
        ]);
    }
    let engine = Arc::new(
        ordo_host::engine_from_members_with(
            &f,
            &rows,
            &ordo_host::presence::PresencePolicy::standard(),
            &PausePolicy::deployment_only(),
        )
        .expect("engine"),
    );
    let access = Arc::new(ordo_host::IdentityAccess::sharing(
        engine.clone(),
        TokenKey::from_secret(&SECRET),
        Box::new(ordo_host::runtime::now_secs),
        Arc::new(ordo_host::metrics::Metrics::new()),
    ));
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "deployment-quotas-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("the fixture compiles no platform-op name");
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let stream = Arc::new(Recorder::default());
    let ops = Arc::new(
        PlatformOps::new(engine, grants, Arc::new(|| NOW)).with_quotas(Arc::new(
            quotas::QuotaOps {
                control: pool.clone(),
                home: HOME.into(),
                enforced: false,
                stream: Some(stream.clone()),
                now: Arc::new(|| NOW),
            },
        )),
    );
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces,
                pg: pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"deployment-quotas-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        access,
        pool,
        stream,
        space,
    }
}

/// A person's token; `touched` when they gave a touch just now.
fn token(principal: &str, touched: bool) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(9_000_000_000),
            client: None,
            user_presence_at: touched.then(|| IStamp(ordo_host::runtime::now_secs())),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn send(h: &Harness, req: Request<Body>) -> (StatusCode, Value) {
    let resp = router(h.state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `std.deployment.quotas` as `who`, bound to `space`.
async fn read(h: &Harness, space: &str, who: &str, query: &str) -> (StatusCode, Value) {
    send(
        h,
        Request::get(format!("/op/{}{query}", quotas::QUOTAS))
            .header("x-ordo-space", space)
            .header("authorization", format!("Bearer {}", token(who, false)))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

/// `std.deployment.setQuota` as `who`, bound to `space`.
async fn set(
    h: &Harness,
    space: &str,
    who: &str,
    touched: bool,
    body: Value,
) -> (StatusCode, Value) {
    let mut body = body;
    body["$space"] = json!(space);
    send(
        h,
        Request::post(format!("/op/{}", quotas::SET_QUOTA))
            .header("x-ordo-space", space)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", token(who, touched)))
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

fn records(h: &Harness) -> String {
    format!(
        "{:?}",
        h.stream
            .drain_all(ordo_commandlog::Timestamp(NOW + 10_000_000_000))
    )
}

fn quota(space: &str, quantity: &str, limit: u64) -> Value {
    json!({"space": space, "quantity": quantity, "limit": limit, "grace": 10,
           "periodSecs": 3600})
}

// ── G9 and G12 ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_owner_sets_and_reads_a_quota_and_each_change_is_recorded() {
    let h = harness("ordo_dq_set").await;
    // Requests this tenant made in the current hour, flushed by a replica.
    let (period_start, period_end) = ordo_exec::usage::period_of(NOW, 3600);
    ordo_exec::usage::add_replica(
        &h.pool,
        &ordo_exec::usage::ReplicaRow {
            space: ordo_commandlog::SpaceId(h.space.clone()),
            period_start,
            period_end,
            replica: "r1".into(),
            requests: [("sql".to_string(), 7u64)].into_iter().collect(),
            rows_returned: 3,
            worker_runs: 0,
            state: ordo_exec::usage::ReplicaState::Open,
            method_version: ordo_exec::usage::METHOD_VERSION.into(),
        },
    )
    .await
    .unwrap();

    let (status, body) = set(
        &h,
        HOME,
        DEPLOYMENT_OWNER,
        true,
        quota(&h.space, "requests", 100),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["limit"], 100);
    assert_eq!(row["grace"], 10);
    assert_eq!(row["periodSecs"], 3600);
    assert_eq!(row["used"], 7);
    assert_eq!(row["enforced"], false);
    assert!(
        row["note"].as_str().unwrap().contains("ORDO_USAGE_LEDGER"),
        "off, the answer names the setting: {row}"
    );

    // The same row `ordo quota show` reads, from the same engine.
    let stored = quotas::limits_for(&h.pool, &h.space).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].1.limit, 100);

    // A second quota, and a change to the first: an upsert.
    set(
        &h,
        HOME,
        DEPLOYMENT_OWNER,
        true,
        quota(HOME, "rows_returned", 5000),
    )
    .await;
    let (status, _) = set(
        &h,
        HOME,
        DEPLOYMENT_OWNER,
        true,
        quota(&h.space, "requests", 200),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The read: every quota, or one space's. A read asks for no touch.
    let (status, body) = read(&h, HOME, DEPLOYMENT_OWNER, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 2, "{body}");
    let (_, body) = read(&h, HOME, DEPLOYMENT_OWNER, &format!("?space={}", h.space)).await;
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(rows[0]["limit"], 200);
    assert_eq!(rows[0]["used"], 7);
    assert_eq!(rows[0]["periodStart"], period_start);

    // G12: each change left a record, naming who and what.
    let text = records(&h);
    assert!(text.contains("quota-set"), "{text}");
    assert!(
        text.contains(&format!(
            "space={} quantity=requests limit=100 grace=10 period=3600s result=set",
            h.space
        )),
        "{text}"
    );
    assert!(text.contains("limit=200"), "{text}");
    assert!(text.contains(DEPLOYMENT_OWNER), "who did it: {text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn set_quota_refuses_what_would_gate_nothing_and_records_the_refusal() {
    let h = harness("ordo_dq_refuse").await;
    // `writes` is not a quota quantity in v1.
    let (status, body) = set(
        &h,
        HOME,
        DEPLOYMENT_OWNER,
        true,
        quota(&h.space, "writes", 10),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(body["error"]["code"], "tenant_refused", "{body}");
    assert!(body.to_string().contains("writes"), "{body}");
    // A period of nothing.
    let mut zero = quota(&h.space, "requests", 10);
    zero["periodSecs"] = json!(0);
    let (status, body) = set(&h, HOME, DEPLOYMENT_OWNER, true, zero).await;
    assert_eq!(body["error"]["code"], "tenant_refused", "{status} {body}");
    assert!(body.to_string().contains("period"), "{body}");
    // A tenant that does not exist.
    let (status, body) = set(
        &h,
        HOME,
        DEPLOYMENT_OWNER,
        true,
        quota("spc_nobody", "requests", 10),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("spc_nobody"), "{body}");

    assert!(quotas::limits_for(&h.pool, &h.space)
        .await
        .unwrap()
        .is_empty());
    assert!(quotas::limits_for(&h.pool, "spc_nobody")
        .await
        .unwrap()
        .is_empty());
    let text = records(&h);
    assert!(text.contains("quantity=writes"), "{text}");
    assert!(text.contains("result=refused"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn quotas_answer_only_the_deployment_owner_and_a_change_needs_a_touch() {
    let h = harness("ordo_dq_scope").await;
    // No touch: the change is refused before the op runs.
    let (status, body) = set(
        &h,
        HOME,
        DEPLOYMENT_OWNER,
        false,
        quota(&h.space, "requests", 1),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    // The door answers as it answers any op this token cannot call.
    assert_eq!(body["error"]["code"], "access_denied", "{body}");
    // A manager of the home space: the wrong role, touch or not.
    let (status, body) = set(&h, HOME, MANAGER, true, quota(&h.space, "requests", 1)).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    let (status, body) = read(&h, HOME, MANAGER, "").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    // A tenant's owner, in their own tenant.
    let space = h.space.clone();
    let (status, body) = set(&h, &space, TENANT_OWNER, true, quota(&space, "requests", 1)).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    let (status, body) = read(&h, &space, TENANT_OWNER, "").await;
    assert_ne!(status, StatusCode::OK, "{body}");

    assert!(quotas::limits_for(&h.pool, &h.space)
        .await
        .unwrap()
        .is_empty());
    assert!(
        !records(&h).contains("quota-set"),
        "a refusal at the door is not an act"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_owner_may_read_usage_and_nobody_else_may() {
    let h = harness("ordo_dq_usage").await;
    let owner = h.access.check(
        &token(DEPLOYMENT_OWNER, false),
        HOME,
        ordo_host::serve::USAGE_OP,
    );
    assert!(matches!(owner, AccessDecision::Allow { .. }), "{owner:?}");
    let manager = h
        .access
        .check(&token(MANAGER, false), HOME, ordo_host::serve::USAGE_OP);
    assert!(matches!(manager, AccessDecision::Deny), "{manager:?}");
}
