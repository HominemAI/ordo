//! 17n M2 — the quota gate, through the real door and real Postgres.
//!
//! `plan-production-ops.md` M2. The non-negotiable this whole file exists to
//! prove: [`ordo_api::router::gate`] checks quotas **after**
//! `AccessChecker::check` and never before, so a caller with no membership
//! in an over-quota space is answered `access_denied` — never
//! `quota_exceeded`, which would disclose that the space exists and is busy
//! (`corpus-rulings.md`, `write-quota`, 2026-09-19).
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{
    router, ApiState, MemoryBus, MemoryReplayCache, NotSyncable, OpCost, OpKind, OpSpec, PageKey,
    QuotaDecision, QuotaGate, RefineTier, Syncability,
};
use ordo_commandlog::SpaceId as ChainSpace;
use ordo_conformance::{Fixture, Pg};
use ordo_exec::usage::{self, ReplicaRow, ReplicaState};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::metrics::{MeteredExecutor, Metrics};
use ordo_host::quotas::{self, PgQuotas};
use ordo_host::{build_specs, FixtureExecutor, MemberAccess, DEFAULT_SPACE};
use serde_json::Value;
use tower::util::ServiceExt;

/// A deployment over the market fixture, wired the way `runtime.rs` wires a
/// real one for 17n M2: `MeteredExecutor` over `Metrics`, and `quota`
/// whichever gate this test wants exercised. Returns the pool too, since
/// every test here has to seed `$quotas`/`$usage_replica` directly — these
/// tests are about `PgQuotas`, not about `usage.flush`, which M1b already
/// covers.
async fn deployment(
    db: &str,
    metrics: Arc<Metrics>,
    quota: Arc<dyn ordo_api::QuotaGate>,
) -> (ApiState, Pool) {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("DDL applies and the fixture seeds");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    quotas::ensure_table(&pool).await.expect("$quotas");
    let deployment = ordo_host::deploy::Deployment::pinned(
        "quotas-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let _specs = build_specs(&f);
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(MeteredExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: pg.clone(),
                page_size: 20,
            }),
            metrics,
        )),
        access: Arc::new(MemberAccess {
            fixture: f.clone(),
            pg,
        }),
        quota,
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"ordo-quotas-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    (state, pool)
}

/// A bare pool over a fresh database — no fixture schema, just the platform
/// tables `Pool::connect` and [`quotas::ensure_table`] create. Enough for
/// [`the_gate_reads_a_snapshot_and_never_the_database`], which never calls
/// through the door.
async fn raw_pool(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    quotas::ensure_table(&pool).await.expect("$quotas");
    pool
}

/// `top3Orders` (`fixtures/market.ordo`), the same plain SQL-class read
/// `tests/usage.rs` calls through — a caller that reaches the executor at
/// all is proof the door let the request through.
async fn call_as(state: &ApiState, bearer: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri("/op/top3Orders")
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn class(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or("")
}

/// Every stale `$usage_replica`/`$quotas` row a previous run against the
/// same database name left behind — these tests reuse database names
/// across runs, and `Fixture::reset` rebuilds the fixture schema but not
/// these control-plane tables.
async fn cleanup(pool: &Pool, space: &str) {
    for table in ["$usage_replica", "$quotas"] {
        pool.rows(&format!(
            "DELETE FROM \"{table}\" WHERE \"space\" = {}",
            ordo_exec::sql_lit(space)
        ))
        .await
        .expect("cleanup");
    }
}

/// One durable `$usage_replica` row for `space` — the level `usage.flush`
/// writes at, without needing a running worker: these tests are about
/// `PgQuotas::refresh`/`check`, not the flush loop M1b already covers.
async fn seed_usage(pool: &Pool, space: &str, requests: u64, rows_returned: u64) {
    let now = now_secs();
    let row = ReplicaRow {
        space: ChainSpace(space.to_string()),
        period_start: now,
        period_end: now + 1,
        replica: "r-quota-test".to_string(),
        requests: [("sql".to_string(), requests)].into_iter().collect(),
        rows_returned,
        worker_runs: 0,
        state: ReplicaState::Open,
        method_version: usage::METHOD_VERSION.to_string(),
    };
    usage::add_replica(pool, &row).await.expect("seed usage");
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A placeholder `OpSpec`, for the tests that call [`PgQuotas::check`]
/// directly rather than through the door — `check`'s own signature never
/// reads the op beyond passing it through, so any read-shaped value proves
/// the point.
fn placeholder_op() -> OpSpec {
    OpSpec {
        name: "placeholder".to_string(),
        version_hash: "v1".to_string(),
        kind: OpKind::Read,
        params: vec![],
        output_fields: vec![],
        live_capable: false,
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
    }
}

/// The non-negotiable: over quota plus grace refuses a member with
/// `quota_exceeded`, and never reaches the executor — but a non-member of
/// the very same over-quota space still gets `access_denied`, learning
/// nothing about the quota. Proves `gate()`'s ordering, not just
/// `PgQuotas`'s arithmetic.
#[tokio::test(flavor = "multi_thread")]
async fn over_quota_plus_grace_is_refused_at_the_door_and_a_non_member_still_gets_access_denied() {
    const DB: &str = "ordo_quotas_over_limit";
    let metrics = Arc::new(Metrics::new().with_usage_ledger(true));
    let quota = Arc::new(PgQuotas::new(metrics.clone()));
    let (state, pool) = deployment(DB, metrics.clone(), quota.clone()).await;
    cleanup(&pool, DEFAULT_SPACE).await;

    quotas::set_limit(&pool, DEFAULT_SPACE, quotas::QUANTITY_REQUESTS, 1, 0, 3600)
        .await
        .expect("set limit");
    // 2 >= limit(1) + grace(0): exceeded.
    seed_usage(&pool, DEFAULT_SPACE, 2, 0).await;
    quota.refresh(&pool, now_secs()).await.expect("refresh");

    // A member of the space: the quota refuses, not access — one op, one
    // caller, and the executor never runs.
    let (status, body) = call_as(&state, "member:mem_1").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(class(&body), "quota_exceeded", "{body}");
    // Audit 31: the refusal names the command that raises the limit.
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("ordo quota set"),
        "{body}"
    );
    // The refusal never reached `MeteredExecutor`: nothing landed in the
    // live ledger this call would have added to.
    let live = metrics.usage_take();
    assert!(
        !live.contains_key(DEFAULT_SPACE),
        "a quota refusal reached the executor and was counted: {live:?}"
    );

    // A non-member of the SAME, still over-quota space: `access_denied`,
    // never `quota_exceeded` — a caller with no membership must learn
    // nothing about the quota.
    let (status, body) = call_as(&state, "member:mem_nonexistent").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(class(&body), "access_denied", "{body}");
}

/// Over the limit, under limit **plus** grace: served. A quota is a soft
/// limit by default, not a hard stop at the boundary.
#[tokio::test(flavor = "multi_thread")]
async fn within_grace_warns_and_serves() {
    const DB: &str = "ordo_quotas_within_grace";
    let metrics = Arc::new(Metrics::new().with_usage_ledger(true));
    let quota = Arc::new(PgQuotas::new(metrics.clone()));
    let (state, pool) = deployment(DB, metrics.clone(), quota.clone()).await;
    cleanup(&pool, DEFAULT_SPACE).await;

    quotas::set_limit(&pool, DEFAULT_SPACE, quotas::QUANTITY_REQUESTS, 1, 5, 3600)
        .await
        .expect("set limit");
    // 2 is over limit(1), under limit + grace(6): a warning, not a refusal.
    seed_usage(&pool, DEFAULT_SPACE, 2, 0).await;
    quota.refresh(&pool, now_secs()).await.expect("refresh");

    let (status, body) = call_as(&state, "member:mem_1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A space with no `$quotas` row at all is never refused, however much it
/// has used — `PgQuotas` starts empty and gates only what `ordo quota set`
/// has actually populated.
#[tokio::test(flavor = "multi_thread")]
async fn no_quota_row_never_refuses() {
    const DB: &str = "ordo_quotas_no_row";
    let metrics = Arc::new(Metrics::new().with_usage_ledger(true));
    let quota = Arc::new(PgQuotas::new(metrics.clone()));
    let (state, pool) = deployment(DB, metrics.clone(), quota.clone()).await;
    cleanup(&pool, DEFAULT_SPACE).await;
    // No `set_limit` call: `$quotas` has nothing for this space.
    quota.refresh(&pool, now_secs()).await.expect("refresh");

    let (status, body) = call_as(&state, "member:mem_1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// `PgQuotas::check` never touches the database: its signature is `(&self,
/// space, op)`, no pool anywhere in it. Refresh once from a real pool, close
/// that pool, and the snapshot still answers correctly from memory — a
/// database outage cannot become a door outage.
#[tokio::test(flavor = "multi_thread")]
async fn the_gate_reads_a_snapshot_and_never_the_database() {
    const DB: &str = "ordo_quotas_snapshot";
    const SPACE: &str = "spc_quotas_snapshot";
    let pool = raw_pool(DB).await;
    cleanup(&pool, SPACE).await;

    let metrics = Arc::new(Metrics::new().with_usage_ledger(true));
    let quota = PgQuotas::new(metrics);
    quotas::set_limit(&pool, SPACE, quotas::QUANTITY_REQUESTS, 1, 0, 3600)
        .await
        .expect("set limit");
    seed_usage(&pool, SPACE, 5, 0).await;
    quota.refresh(&pool, now_secs()).await.expect("refresh");

    // Close the only pool this snapshot could have come from.
    drop(pool);

    // Whatever the snapshot says, it says it from memory — `check` takes
    // no pool at all.
    let decision = quota.check(SPACE, &placeholder_op());
    assert!(
        matches!(decision, QuotaDecision::Exceeded { .. }),
        "{decision:?}"
    );
}
