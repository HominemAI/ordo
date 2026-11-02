//! 17n — the metering ledger (M1), through the real door, the real
//! supervisor, and real Postgres.
//!
//! `plan-production-ops.md` Track M. **M1a**, the in-process ledger, is
//! proved by the first two tests: a request through the real router, over a
//! real deployment, lands in `Metrics::usage_take()` keyed by the space it
//! was served in, with the effective cost class the compiler assigned and
//! the row count the read actually returned. **M1b**, `$usage_replica` and
//! the per-replica flush, is everything after it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, SpaceId as ChainSpace,
    Timestamp, Value as ChainValue,
};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::{AttachedPools, Installed, PoolResolver};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::usage::{self, ReplicaRow, ReplicaState};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::metrics::MeteredExecutor;
use ordo_host::metrics::Metrics;
use ordo_host::supervisor::{Plan, Supervisor, Worker};
use ordo_host::workers::{UsageFlush, UsageRollup};
use ordo_host::{build_specs, FixtureExecutor, MemberAccess, DEFAULT_SPACE};
use serde_json::Value;
use tower::util::ServiceExt;

/// A deployment over the shipped market fixture, its executor wrapped in
/// [`MeteredExecutor`] so every request through the door is timed AND
/// metered — the same wiring `runtime.rs::assemble` builds, minus the parts
/// this test does not need (blob tier, delegation, attestation).
async fn deployment(db: &str, metrics: Arc<Metrics>) -> ApiState {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("DDL applies and the fixture seeds");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    let deployment = ordo_host::deploy::Deployment::pinned(
        "usage-test",
        f.clone(),
        ordo_exec::Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let _specs = build_specs(&f);
    ApiState {
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
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"ordo-usage-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    }
}

async fn call(state: &ApiState, method: &str, path: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("authorization", "Bearer member:mem_1")
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

/// **M1a's own test.** A read through the real door, over a real deployment,
/// lands in the ledger keyed by space, by the op's effective cost class,
/// with the exact row count the read returned.
///
/// `top3Orders` (`fixtures/market.ordo`) is a plain SQL-class read —
/// `orders[placed, total, customer] sort -total take 3` — over seven seeded
/// orders (`fixtures/seed.sql`), so it returns exactly three rows every
/// time: no row-count assertion here depends on ordering or on the seed
/// growing.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_through_the_door_is_counted_by_class_and_rows() {
    let metrics = Arc::new(Metrics::new().with_usage_ledger(true));
    let state = deployment("ordo_usage_door_read", metrics.clone()).await;

    let (status, body) = call(&state, "GET", "/op/top3Orders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3, "take 3 over seven seeded orders");

    let taken = metrics.usage_take();
    let counters = taken
        .get(DEFAULT_SPACE)
        .unwrap_or_else(|| panic!("{DEFAULT_SPACE} recorded; got {taken:?}"));
    assert_eq!(
        counters.requests.get("sql"),
        Some(&1),
        "top3Orders is a plain SQL-class read"
    );
    assert_eq!(
        counters.rows_returned, 3,
        "the ledger's row count is the read's own row count, not a guess"
    );
    assert_eq!(counters.worker_runs, 0);

    // The ledger drains: a second take over the same registry, with no
    // further traffic, is empty.
    assert!(metrics.usage_take().is_empty());
}

/// The ledger is opt-in (`ORDO_USAGE_LEDGER`, or the builder here): a
/// deployment that never asked for metering pays nothing and reports
/// nothing, which is what lets `MeteredExecutor` call `usage_request`
/// unconditionally without a deployment-shaped branch at the call site.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_through_the_door_is_not_counted_when_the_ledger_is_off() {
    let metrics = Arc::new(Metrics::new());
    assert!(!metrics.usage_enabled());
    let state = deployment("ordo_usage_door_off", metrics.clone()).await;

    let (status, _body) = call(&state, "GET", "/op/top3Orders").await;
    assert_eq!(status, StatusCode::OK);

    assert!(metrics.usage_take().is_empty());
}

// ── M1b: `$usage_replica` and the per-replica flush ──────────────────────

/// A pool of its own database, created if it does not already exist. These
/// databases outlive a run (`lock_wait.rs`'s convention), so a caller that
/// reruns a synthetic-time test cleans its own space's rows first.
async fn raw_pool(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    Pool::connect(PoolConfig {
        db: db.to_string(),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots")
}

/// [`raw_pool`], plus the one table an empty-source `Fixture` still needs:
/// `members` is a platform entity every schema carries even with no domain
/// source (`ordo_schema::lib.rs:559`), and `UsageRollup`'s
/// `bytes_rows_estimated` pass queries every entity the live schema names —
/// `ordo_host::deploy::Deployment::empty` compiles that same empty source,
/// so a database backing it needs the same table or the rollup's own query
/// fails with `relation "members" does not exist`.
async fn raw_pool_with_empty_schema(db: &str) -> Pool {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("clean");
    let empty = Fixture::from_source("", "").expect("an empty source compiles");
    pg.exec(&empty.ddl).expect("the members table");
    raw_pool(db).await
}

/// A supervisor running exactly one worker, `UsageFlush`, over an empty
/// deployment — the lightest harness that exercises the real `Worker`
/// trait and the real cadence loop (`tests/supervisor.rs`'s
/// `a_panicking_worker_is_restarted...` uses the same shape).
async fn usage_flush_supervisor(
    db: &str,
    metrics: Arc<Metrics>,
    replica: &str,
    period: Duration,
) -> Supervisor {
    let pool = raw_pool(db).await;
    let space = ordo_crypto::SpaceId("spc_usage_worker_space".into());
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(&space.0));
    let deployment =
        ordo_host::deploy::Deployment::empty(space.clone(), pool.clone(), keys.clone())
            .expect("an empty deployment");
    let worker = Arc::new(UsageFlush {
        metrics: metrics.clone(),
        replica: replica.to_string(),
        period,
        quotas: None,
    });
    Supervisor::start(
        pool.clone(),
        keys,
        deployment,
        metrics,
        space,
        PoolConfig {
            db: db.to_string(),
            ..PoolConfig::from_env()
        },
        Plan {
            workers: vec![worker],
            ..Plan::default()
        },
    )
    .await
    .expect("the supervisor starts")
}

/// A supervisor over an arbitrary worker set (M1c's tests run `UsageFlush`
/// and `UsageRollup` together, or `UsageRollup` alone against a routed
/// resolver), sharing one control pool and one deployment with the caller
/// so the rows it writes land where the test already looks for them.
async fn usage_supervisor(
    db: &str,
    pool: Pool,
    deployment: Arc<ordo_host::deploy::Deployment>,
    metrics: Arc<Metrics>,
    workers: Vec<Arc<dyn Worker>>,
) -> Supervisor {
    let space = ordo_crypto::SpaceId("spc_usage_worker_space".into());
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(&space.0));
    Supervisor::start(
        pool,
        keys,
        deployment,
        metrics,
        space,
        PoolConfig {
            db: db.to_string(),
            ..PoolConfig::from_env()
        },
        Plan {
            workers,
            ..Plan::default()
        },
    )
    .await
    .expect("the supervisor starts")
}

/// A real deployment over the shipped market fixture — non-empty entities,
/// unlike [`usage_flush_supervisor`]'s [`ordo_host::deploy::Deployment::empty`]
/// — for the one test that needs `bytes_rows_estimated` to read something
/// other than zero.
async fn market_deployment(
    db: &str,
) -> (Arc<Fixture>, Pg, Arc<ordo_host::deploy::Deployment>, Pool) {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("DDL applies and the fixture seeds");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    let deployment = ordo_host::deploy::Deployment::pinned(
        "usage-rollup-bytes-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    (f, pg, deployment, pool)
}

/// How long a poll waits before failing. A poll returns as soon as its
/// condition holds, so this costs nothing on a pass. On 2026-11-01 (run
/// 41292306761) 6 and 8 s ran out on a loaded CI node.
const WAIT: Duration = Duration::from_secs(30);

/// Poll an async condition instead of sleeping a guessed duration — every
/// one of these tests waits on a real `Cadence::Tick` loop, and how long
/// that takes depends on the machine running it.
async fn poll<F, Fut, T>(deadline: Duration, mut f: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let start = std::time::Instant::now();
    loop {
        if let Some(v) = f().await {
            return Some(v);
        }
        if start.elapsed() > deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// **M1b's central test.** `Cadence::Tick` runs a worker immediately on
/// start, so the first flush writes one `'open'` row for the period
/// containing "now". A short period (1s) is then guaranteed to roll over
/// within a few ticks, and the roll must do two things at once: a new row
/// for the new period, and the old row moved to `'closed'`.
#[tokio::test(flavor = "multi_thread")]
async fn a_flush_writes_one_open_row_per_space_and_closes_the_previous_period() {
    const DB: &str = "ordo_usage_flush_periods";
    const SPACE: &str = "spc_usage_flush_periods";
    let pool = raw_pool(DB).await;
    pool.rows(&format!(
        "DELETE FROM \"$usage_replica\" WHERE \"space\" = {}",
        ordo_exec::sql_lit(SPACE)
    ))
    .await
    .unwrap();

    let metrics = Arc::new(Metrics::new().with_usage_ledger(true));
    metrics.usage_request(SPACE, ordo_api::CostClass::Sql, Some(2));

    let sup =
        usage_flush_supervisor(DB, metrics.clone(), "r-periods", Duration::from_secs(1)).await;

    let first = poll(WAIT, || async {
        let (_, rows) = usage::between(&pool, SPACE, 0, u64::MAX).await.unwrap();
        (!rows.is_empty()).then_some(rows)
    })
    .await
    .expect("no $usage_replica row appeared after the first tick");
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].requests.get("sql"), Some(&1));
    assert_eq!(first[0].rows_returned, 2);
    assert_eq!(first[0].state, ReplicaState::Open, "{first:?}");
    let first_period = first[0].period_start;

    // Feed the ledger again and wait for the period to roll over: a
    // second row appears, for a later period, and the first one closes.
    metrics.usage_request(SPACE, ordo_api::CostClass::Sql, Some(5));
    let rolled = poll(WAIT, || async {
        let (_, rows) = usage::between(&pool, SPACE, 0, u64::MAX).await.unwrap();
        // Wait for the close too: the flush writes the new period's row and
        // closes the old one in separate steps, and a loaded machine can
        // read between them.
        let rolled = rows.len() >= 2 && rows.iter().any(|r| r.period_start > first_period);
        let closed = rows
            .iter()
            .any(|r| r.period_start == first_period && r.state == ReplicaState::Closed);
        (rolled && closed).then_some(rows)
    })
    .await
    .expect("the period never rolled over and closed");
    let closed = rolled
        .iter()
        .find(|r| r.period_start == first_period)
        .expect("the first period's row is still there");
    assert_eq!(
        closed.state,
        ReplicaState::Closed,
        "a period this replica has moved past must close: {rolled:?}"
    );
    let open = rolled
        .iter()
        .find(|r| r.period_start > first_period)
        .expect("a new period's row");
    assert_eq!(open.state, ReplicaState::Open, "{rolled:?}");
    assert_eq!(open.requests.get("sql"), Some(&1));
    assert_eq!(open.rows_returned, 5);

    sup.shutdown(Duration::from_secs(5)).await;
}

/// Two replicas flushing the same space in the same period are two rows,
/// not a race over one — `PRIMARY KEY (space, period_start, replica)` says
/// so directly, and this is the sum an operator or `usage.rollup` reads
/// over them.
#[tokio::test(flavor = "multi_thread")]
async fn two_replicas_in_one_period_are_two_rows_that_sum() {
    const DB: &str = "ordo_usage_two_replicas";
    const SPACE: &str = "spc_usage_two_replicas";
    let pool = raw_pool(DB).await;
    pool.rows(&format!(
        "DELETE FROM \"$usage_replica\" WHERE \"space\" = {}",
        ordo_exec::sql_lit(SPACE)
    ))
    .await
    .unwrap();

    // A synthetic, fixed period rather than "now": deterministic across
    // reruns, and it exercises `add_replica`/`between` directly rather than
    // through a worker, which is the right level for a fact about the
    // table's key shape.
    let (period_start, period_end) = usage::period_of(1_700_000_000, 60);
    let row = |replica: &str, sql: u64, rows: u64| ReplicaRow {
        space: ChainSpace(SPACE.to_string()),
        period_start,
        period_end,
        replica: replica.to_string(),
        requests: [("sql".to_string(), sql)].into_iter().collect(),
        rows_returned: rows,
        worker_runs: 0,
        state: ReplicaState::Open,
        method_version: usage::METHOD_VERSION.to_string(),
    };
    usage::add_replica(&pool, &row("r1", 3, 10)).await.unwrap();
    usage::add_replica(&pool, &row("r2", 4, 20)).await.unwrap();

    let (_, rows) = usage::between(&pool, SPACE, period_start, period_start + 1)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let total_sql: u64 = rows
        .iter()
        .map(|r| *r.requests.get("sql").unwrap_or(&0))
        .sum();
    let total_rows: u64 = rows.iter().map(|r| r.rows_returned).sum();
    assert_eq!(total_sql, 7, "{rows:?}");
    assert_eq!(total_rows, 30, "{rows:?}");
}

/// `on_shutdown` closes the period it just flushed, unlike `run` — a
/// replica that is stopping will never write that period again either, so
/// there is nothing to leave `'open'` for.
///
/// The period is an hour, far longer than the test runs, which is what
/// isolates this from `a_flush_writes_one_open_row...`'s natural rollover:
/// the only thing that can close this row is the shutdown itself.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_closes_the_current_period() {
    const DB: &str = "ordo_usage_flush_shutdown";
    const SPACE: &str = "spc_usage_flush_shutdown";
    let pool = raw_pool(DB).await;
    pool.rows(&format!(
        "DELETE FROM \"$usage_replica\" WHERE \"space\" = {}",
        ordo_exec::sql_lit(SPACE)
    ))
    .await
    .unwrap();

    let metrics = Arc::new(Metrics::new().with_usage_ledger(true));
    metrics.usage_request(SPACE, ordo_api::CostClass::Sql, Some(1));

    let sup = usage_flush_supervisor(
        DB,
        metrics.clone(),
        "r-shutdown",
        Duration::from_secs(3_600),
    )
    .await;

    let rows = poll(WAIT, || async {
        let (_, rows) = usage::between(&pool, SPACE, 0, u64::MAX).await.unwrap();
        (!rows.is_empty()).then_some(rows)
    })
    .await
    .expect("the first tick did not write a row");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].state, ReplicaState::Open, "{rows:?}");

    sup.shutdown(Duration::from_secs(5)).await;

    let (_, after) = usage::between(&pool, SPACE, 0, u64::MAX).await.unwrap();
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(
        after[0].state,
        ReplicaState::Closed,
        "on_shutdown must close the current period even though it has not \
         ended on the clock: {after:?}"
    );
}

/// One command group, `n` commands, over its own space and entity — the
/// shape `lock_wait.rs`'s `group` helper uses, generalised to a variable
/// command count so this test can vary what it is checking against.
fn group(n: usize, commands: usize) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(format!("cg_usage_cc_{n}")),
        actor: Actor::User(format!("usr_{n}")),
        commands: (0..commands)
            .map(|i| Command {
                id: CommandId(format!("cmd_{n}_{i}")),
                space: ChainSpace("spc_usage_cc".into()),
                entity: "transactions".into(),
                row_id: format!("txn_{n}_{i}"),
                kind: CommandKind::Create,
                before: None,
                after: Some(
                    [("amount".to_string(), ChainValue::Int(i as i64))]
                        .into_iter()
                        .collect(),
                ),
                at: Timestamp(1_000 + n as u64),
            })
            .collect(),
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

/// [`group`], generalised to an arbitrary space and tag — M1c's tests write
/// several batches, sometimes into more than one space or one database,
/// and need each group's correlation and command ids to stay unique across
/// them.
fn rollup_group(space: &str, tag: usize, commands: usize) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(format!("cg_usage_rollup_{space}_{tag}")),
        actor: Actor::User(format!("usr_rollup_{tag}")),
        commands: (0..commands)
            .map(|i| Command {
                id: CommandId(format!("cmd_rollup_{space}_{tag}_{i}")),
                space: ChainSpace(space.into()),
                entity: "transactions".into(),
                row_id: format!("txn_rollup_{space}_{tag}_{i}"),
                kind: CommandKind::Create,
                before: None,
                after: Some(
                    [("amount".to_string(), ChainValue::Int(i as i64))]
                        .into_iter()
                        .collect(),
                ),
                at: Timestamp(2_000 + tag as u64),
            })
            .collect(),
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

/// **`usage.rs`'s own reading of the chain's encoding must agree with
/// `ordo-commandlog`'s.** Every stored body this test writes — one command,
/// a handful, and a dozen — is read back and checked both ways. (An empty
/// group is not among them: `CommandGroup::validate` refuses one before it
/// is ever appended, so no stored body is ever that shape.)
#[tokio::test(flavor = "multi_thread")]
async fn command_count_agrees_with_decode_group_on_every_stored_body() {
    const DB: &str = "ordo_usage_command_count";
    let pool = raw_pool(DB).await;
    pool.rows("DELETE FROM \"$commandlog\" WHERE \"space\" = 'spc_usage_cc'")
        .await
        .unwrap();

    let counts = [1usize, 2, 5, 12];
    for (n, commands) in counts.into_iter().enumerate() {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&group(n, commands)).await.expect("append");
        tx.commit().await.unwrap();
    }

    let rows = pool
        .rows("SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = 'spc_usage_cc'")
        .await
        .unwrap();
    assert_eq!(rows.len(), counts.len(), "{rows:?}");
    for r in rows {
        let body = ordo_exec::commandlog::bytea_of(r.first().and_then(|c| c.as_ref()))
            .expect("a stored body decodes from its bytea rendering");
        let decoded = ordo_commandlog::decode_group(&body).expect("a stored body always decodes");
        assert_eq!(
            usage::command_count(&body),
            Some(decoded.commands.len() as u64),
            "usage::command_count disagrees with decode_group on a stored body"
        );
    }
}

// ── M1c: usage.rollup, the leader-only worker ─────────────────────────────

/// **M1c's central test.** N reads across two cost classes go straight to
/// the in-process ledger; M writes carrying K commands in total go through
/// the real append path; J worker runs go through the same counter
/// `TriggersRunner` calls on a finished run
/// (`ordo_host::metrics::Metrics::usage_worker_run`, `workers.rs:164`).
/// `usage.flush` and `usage.rollup` run together over one space, and the
/// two tables they write must sum back to exactly what was issued: nothing
/// dropped, nothing doubled.
#[tokio::test(flavor = "multi_thread")]
async fn a_periods_rows_reconstruct_what_the_test_issued() {
    const DB: &str = "ordo_usage_rollup_reconstruct";
    const SPACE: &str = "spc_usage_rollup_reconstruct";
    let pool = raw_pool_with_empty_schema(DB).await;
    for table in ["$commandlog", "$usage_replica", "$usage"] {
        pool.rows(&format!(
            "DELETE FROM \"{table}\" WHERE \"space\" = {}",
            ordo_exec::sql_lit(SPACE)
        ))
        .await
        .unwrap();
    }

    // N = 5 reads across two cost classes; rows_returned sums to 11.
    const N: u64 = 5;
    const ROWS_RETURNED: u64 = 11;
    let metrics = Arc::new(Metrics::new().with_usage_ledger(true));
    for rows in [2, 3, 4] {
        metrics.usage_request(SPACE, ordo_api::CostClass::Sql, Some(rows));
    }
    for rows in [1, 1] {
        metrics.usage_request(SPACE, ordo_api::CostClass::BoundedHost, Some(rows));
    }

    // J = 4 trigger runs.
    const J: u64 = 4;
    for _ in 0..J {
        metrics.usage_worker_run(SPACE);
    }

    // M = 3 writes carrying K = 6 commands in total.
    const M: u64 = 3;
    const K: u64 = 6;
    for (i, commands) in [2usize, 3, 1].into_iter().enumerate() {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&rollup_group(SPACE, i, commands))
            .await
            .expect("append");
        tx.commit().await.unwrap();
    }

    let space_id = ordo_crypto::SpaceId(SPACE.into());
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(SPACE));
    let deployment = ordo_host::deploy::Deployment::empty(space_id, pool.clone(), keys)
        .expect("an empty deployment");
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());

    let flush = Arc::new(UsageFlush {
        metrics: metrics.clone(),
        replica: "r-rollup-reconstruct".into(),
        period: Duration::from_secs(1),
        quotas: None,
    });
    let rollup = Arc::new(UsageRollup {
        spaces,
        period: Duration::from_secs(1),
    });
    let sup = usage_supervisor(
        DB,
        pool.clone(),
        deployment,
        metrics.clone(),
        vec![flush, rollup],
    )
    .await;

    let (space_rows, replica_rows) = poll(WAIT, || async {
        let (s, r) = usage::between(&pool, SPACE, 0, u64::MAX).await.unwrap();
        (!s.is_empty() && !r.is_empty()).then_some((s, r))
    })
    .await
    .expect("neither the flush nor the rollup ever wrote a row");

    let requests_total: u64 = replica_rows
        .iter()
        .map(|r| r.requests.values().sum::<u64>())
        .sum();
    assert_eq!(requests_total, N, "{replica_rows:?}");
    let rows_returned_total: u64 = replica_rows.iter().map(|r| r.rows_returned).sum();
    assert_eq!(rows_returned_total, ROWS_RETURNED, "{replica_rows:?}");
    let worker_runs_total: u64 = replica_rows.iter().map(|r| r.worker_runs).sum();
    assert_eq!(worker_runs_total, J, "{replica_rows:?}");

    assert_eq!(space_rows.len(), 1, "{space_rows:?}");
    assert_eq!(space_rows[0].writes, M, "{space_rows:?}");
    assert_eq!(space_rows[0].rows_written, K, "{space_rows:?}");

    sup.shutdown(Duration::from_secs(5)).await;
}

/// A space's second rollup must continue from the first row's `chain_to`,
/// not from the space's earliest stored `idx` again — otherwise every
/// write before the most recent rollup is counted on every pass after it.
/// A short, shared period on both the schedule claim and the period
/// bucket (`usage::period_of`) is what lets the two passes land in
/// different periods without the test forcing the clock.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_rollup_continues_from_chain_to_and_counts_nothing_twice() {
    const DB: &str = "ordo_usage_rollup_continues";
    const SPACE: &str = "spc_usage_rollup_continues";
    let pool = raw_pool_with_empty_schema(DB).await;
    for table in ["$commandlog", "$usage"] {
        pool.rows(&format!(
            "DELETE FROM \"{table}\" WHERE \"space\" = {}",
            ordo_exec::sql_lit(SPACE)
        ))
        .await
        .unwrap();
    }

    // Batch 1: 2 writes, 5 commands.
    for (i, commands) in [3usize, 2].into_iter().enumerate() {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&rollup_group(SPACE, i, commands))
            .await
            .expect("append batch 1");
        tx.commit().await.unwrap();
    }

    let space_id = ordo_crypto::SpaceId(SPACE.into());
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(SPACE));
    let deployment = ordo_host::deploy::Deployment::empty(space_id, pool.clone(), keys)
        .expect("an empty deployment");
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let rollup = Arc::new(UsageRollup {
        spaces,
        period: Duration::from_secs(1),
    });
    let metrics = Arc::new(Metrics::new());
    let sup = usage_supervisor(DB, pool.clone(), deployment, metrics, vec![rollup]).await;

    let first = poll(WAIT, || async {
        let (rows, _) = usage::between(&pool, SPACE, 0, u64::MAX).await.unwrap();
        (!rows.is_empty()).then_some(rows)
    })
    .await
    .expect("the first rollup never wrote a row");
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].writes, 2, "{first:?}");
    assert_eq!(first[0].rows_written, 5, "{first:?}");
    let first_chain_to = first[0].chain_to;

    // Batch 2: 3 writes, 7 commands — appended only after the first rollup
    // is on the record, so the second pass has something new to find.
    for (i, commands) in [4usize, 2, 1].into_iter().enumerate() {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&rollup_group(SPACE, 10 + i, commands))
            .await
            .expect("append batch 2");
        tx.commit().await.unwrap();
    }

    let second = poll(WAIT, || async {
        let (rows, _) = usage::between(&pool, SPACE, 0, u64::MAX).await.unwrap();
        (rows.len() >= 2).then_some(rows)
    })
    .await
    .expect("the second rollup never wrote a row");
    assert_eq!(
        second.len(),
        2,
        "batch 2's rollup must add a row, not replace the first: {second:?}"
    );
    let row2 = second
        .iter()
        .find(|r| r.chain_to > first_chain_to)
        .expect("a row past the first rollup's chain_to");
    // `chain_from` on a continuing rollup is the PRIOR row's `chain_to`
    // itself, not `chain_to + 1` — the window `chain_window` actually
    // scans is `(chain_from, chain_to]`, exclusive on the low end, so the
    // entry at the old `chain_to` is correctly not recounted even though
    // the field that names it looks unchanged. Only a space's first-ever
    // rollup treats its lower bound as inclusive (`workers.rs`'s own
    // module doc, "why the lower bound has two shapes").
    assert_eq!(
        row2.chain_from, first_chain_to,
        "a continuing rollup's chain_from must be the prior row's chain_to: {row2:?}"
    );
    assert_eq!(
        row2.writes, 3,
        "batch 1's 2 writes must not be counted again: {row2:?}"
    );
    assert_eq!(
        row2.rows_written, 7,
        "batch 1's 5 commands must not be counted again: {row2:?}"
    );

    sup.shutdown(Duration::from_secs(5)).await;
}

/// The one estimated column. `bytes_sealed`, `bytes_chain` and
/// `bytes_blob` are exact `SUM`s over real rows this test controls;
/// `bytes_rows_estimated` comes from `pg_class` statistics
/// (`pg_relation_size`/`reltuples`) and is only asked to land in the right
/// order of magnitude, which is the whole reason it is the one column the
/// module doc calls out as estimated rather than measured.
#[tokio::test(flavor = "multi_thread")]
async fn bytes_stored_names_its_method_and_the_estimated_column_alone_is_estimated() {
    const DB: &str = "ordo_usage_rollup_bytes";
    let (f, pg, deployment, pool) = market_deployment(DB).await;
    let space = ordo_conformance::fixture_space();
    let space_str = space.0.clone();

    for (table, col) in [
        ("$commandlog", "space"),
        ("$usage", "space"),
        ("$sealedpayload", "$space"),
        ("$blobref", "$space"),
    ] {
        pool.rows(&format!(
            "DELETE FROM \"{table}\" WHERE \"{col}\" = {}",
            ordo_exec::sql_lit(&space_str)
        ))
        .await
        .unwrap();
    }

    // Exact, known bytes: two sealed payloads (5 and 11 bytes) and two
    // blob references (100 and 250 bytes).
    pool.rows(&format!(
        "INSERT INTO \"$sealedpayload\" \
           (\"$space\", \"ref\", \"epoch\", \"entity\", \"field\", \"anchor\", \
            \"correlation\", \"payload\") \
         VALUES \
           ({sp}, decode('aa', 'hex'), 1, 'e', 'f', 'row:r1', 'c1', decode('{p1}', 'hex')), \
           ({sp}, decode('bb', 'hex'), 1, 'e', 'f', 'row:r2', 'c2', decode('{p2}', 'hex'))",
        sp = ordo_exec::sql_lit(&space_str),
        p1 = "00".repeat(5),
        p2 = "00".repeat(11),
    ))
    .await
    .expect("sealed rows");
    const BYTES_SEALED: u64 = 16;

    pool.rows(&format!(
        "INSERT INTO \"$blobref\" \
           (\"$space\", \"ref\", \"epoch\", \"entity\", \"field\", \"mime\", \"size\", \
            \"integrity\", \"bytes\", \"created_at\") \
         VALUES \
           ({sp}, decode('cc', 'hex'), 1, 'e', 'f', decode('00','hex'), decode('00','hex'), \
            decode('00','hex'), 100, 1000), \
           ({sp}, decode('dd', 'hex'), 1, 'e', 'f', decode('00','hex'), decode('00','hex'), \
            decode('00','hex'), 250, 1000)",
        sp = ordo_exec::sql_lit(&space_str),
    ))
    .await
    .expect("blob rows");
    const BYTES_BLOB: u64 = 350;

    // A couple of command groups so `chain_heads` and `bytes_chain` have
    // something to read. The expected total comes from the database's own
    // count, not a hand-computed encoding size — `bytes_chain`'s query is
    // one `SUM(octet_length(body))`, so re-running it independently here
    // is a second, honest reading rather than a tautology.
    for (i, commands) in [3usize, 2].into_iter().enumerate() {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&rollup_group(&space_str, 900 + i, commands))
            .await
            .expect("append");
        tx.commit().await.unwrap();
    }
    let expected_bytes_chain: u64 = pool
        .rows(&format!(
            "SELECT COALESCE(SUM(octet_length(\"body\")), 0) FROM \"$commandlog\" \
             WHERE \"space\" = {}",
            ordo_exec::sql_lit(&space_str)
        ))
        .await
        .unwrap()[0][0]
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap();

    // Real statistics, so `bytes_rows_estimated` reads real row counts
    // rather than a freshly-loaded table's default zero `reltuples`.
    pg.exec("ANALYZE").expect("analyze");
    let entity_names: Vec<String> = f.schema.entities.iter().map(|e| e.name.clone()).collect();
    assert!(
        !entity_names.is_empty(),
        "the market fixture must declare at least one entity for this test to mean anything"
    );
    let entity_list = entity_names
        .iter()
        .map(|n| ordo_exec::sql_lit(n))
        .collect::<Vec<_>>()
        .join(",");
    let actual_total: f64 = pool
        .rows(&format!(
            "SELECT COALESCE(SUM(pg_relation_size(oid)), 0)::float8 FROM pg_class \
             WHERE relname IN ({entity_list})"
        ))
        .await
        .unwrap()[0][0]
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap();

    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let rollup = Arc::new(UsageRollup {
        spaces,
        period: Duration::from_secs(1),
    });
    let metrics = Arc::new(Metrics::new());
    let sup = usage_supervisor(DB, pool.clone(), deployment, metrics, vec![rollup]).await;

    let row = poll(WAIT, || async {
        let (rows, _) = usage::between(&pool, &space_str, 0, u64::MAX)
            .await
            .unwrap();
        rows.into_iter().next()
    })
    .await
    .expect("the rollup never wrote a row");

    assert_eq!(row.method_version, usage::METHOD_VERSION, "{row:?}");
    assert_eq!(row.bytes_sealed, BYTES_SEALED, "{row:?}");
    assert_eq!(row.bytes_blob, BYTES_BLOB, "{row:?}");
    assert_eq!(row.bytes_chain, expected_bytes_chain, "{row:?}");
    assert!(
        row.bytes_rows_estimated > 0,
        "the fixture space has real seeded rows; the estimate must not read zero: {row:?}"
    );
    assert!(
        (row.bytes_rows_estimated as f64) > actual_total * 0.1
            && (row.bytes_rows_estimated as f64) < actual_total * 10.0,
        "estimate {} is not within an order of magnitude of the real total {actual_total}",
        row.bytes_rows_estimated
    );

    sup.shutdown(Duration::from_secs(5)).await;
}

/// The rollup walks every database `PoolResolver::targets` names, not only
/// the one the process booted with: one space resolves to the default
/// (control) target, a second resolves to a database attached through
/// `$spacedb`, and both must land a `$usage` row — the control-database
/// table, regardless of which database either space's chain lives in.
#[tokio::test(flavor = "multi_thread")]
async fn the_rollup_walks_every_attached_target() {
    const CONTROL_DB: &str = "ordo_usage_rollup_targets_control";
    const ATTACHED_DB: &str = "ordo_usage_rollup_targets_attached";
    const DEFAULT_SPACE_NAME: &str = "spc_usage_rollup_default_side";
    const ATTACHED_SPACE_NAME: &str = "spc_usage_rollup_attached_side";

    let control = raw_pool_with_empty_schema(CONTROL_DB).await;
    let target = raw_pool_with_empty_schema(ATTACHED_DB).await;
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    for (pool, space) in [
        (&control, DEFAULT_SPACE_NAME),
        (&target, ATTACHED_SPACE_NAME),
    ] {
        pool.rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = {}",
            ordo_exec::sql_lit(space)
        ))
        .await
        .unwrap();
    }
    control
        .rows(&format!(
            "DELETE FROM \"$usage\" WHERE \"space\" IN ({}, {})",
            ordo_exec::sql_lit(DEFAULT_SPACE_NAME),
            ordo_exec::sql_lit(ATTACHED_SPACE_NAME)
        ))
        .await
        .unwrap();

    let env = PoolConfig::from_env();
    spacedb::attach(
        &control,
        &ordo_crypto::SpaceId(ATTACHED_SPACE_NAME.into()),
        &DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: ATTACHED_DB.into(),
            credential: "local".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "mem_owner".into(),
        },
    )
    .await
    .expect("attached");

    // Two writes on the default side, three on the attached side — through
    // the pools each one actually lives in, so a rollup reading only the
    // default target would report the default side complete and the
    // attached side untouched.
    for (i, commands) in [2usize, 1].into_iter().enumerate() {
        let tx = control.begin().await.unwrap();
        tx.append_group(&rollup_group(DEFAULT_SPACE_NAME, i, commands))
            .await
            .expect("default-side append");
        tx.commit().await.unwrap();
    }
    for (i, commands) in [4usize, 3, 2].into_iter().enumerate() {
        let tx = target.begin().await.unwrap();
        tx.append_group(&rollup_group(ATTACHED_SPACE_NAME, 20 + i, commands))
            .await
            .expect("attached-side append");
        tx.commit().await.unwrap();
    }

    let resolver: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        PoolConfig {
            size: 2,
            ..PoolConfig::from_env()
        },
        Arc::new(Installed::new().as_configured("local")),
    ));
    let keys: Arc<dyn ordo_crypto::KeyProvider> = Arc::new(ordo_crypto::TestProvider::with_space(
        "spc_usage_rollup_multi_target",
    ));
    let deployment = ordo_host::deploy::Deployment::empty(
        ordo_crypto::SpaceId("spc_usage_rollup_multi_target".into()),
        control.clone(),
        keys,
    )
    .expect("an empty deployment");
    let spaces = ordo_host::spaces::Spaces::routed(
        deployment.clone(),
        resolver,
        ordo_exec::witness::Witness::new(control.clone()),
    );
    let rollup = Arc::new(UsageRollup {
        spaces,
        period: Duration::from_secs(1),
    });
    let metrics = Arc::new(Metrics::new());
    let sup = usage_supervisor(
        CONTROL_DB,
        control.clone(),
        deployment,
        metrics,
        vec![rollup],
    )
    .await;

    let (default_row, attached_row) = poll(WAIT, || async {
        let (d, _) = usage::between(&control, DEFAULT_SPACE_NAME, 0, u64::MAX)
            .await
            .unwrap();
        let (a, _) = usage::between(&control, ATTACHED_SPACE_NAME, 0, u64::MAX)
            .await
            .unwrap();
        match (d.into_iter().next(), a.into_iter().next()) {
            (Some(d), Some(a)) => Some((d, a)),
            _ => None,
        }
    })
    .await
    .expect("the rollup never covered both targets");

    assert_eq!(default_row.writes, 2, "{default_row:?}");
    assert_eq!(default_row.rows_written, 3, "{default_row:?}");
    assert_eq!(attached_row.writes, 3, "{attached_row:?}");
    assert_eq!(attached_row.rows_written, 9, "{attached_row:?}");

    sup.shutdown(Duration::from_secs(5)).await;
}

/// **18d R-O5/D-G: an unreachable target leaves a gap in the usage ledger,
/// never a row of zeros — and the rest of the fleet still rolls up.**
///
/// One space stays on the default (control) database and writes two
/// groups; a second is attached to a port nothing listens on
/// (`127.0.0.1:1`, `tests/spaces.rs`'s own way of making a target
/// unreachable without a flaky network dependency). Before this increment
/// `usage.rollup` aborted the whole pass the moment any target came back
/// `Err` from `targets()`, so the default-side tenant's own usage would go
/// unmeasured too, over an outage that was never theirs. This asserts both
/// halves of the fix: the reachable side rolls up as if the down target
/// were not there, and the unreachable side gets an explicit
/// `$usage_gaps` row rather than a `$usage` row of zeros — the two are
/// distinguished by which table holds the period, not by a flag on one
/// row, because a caller that reads `$usage` for a gapped period must find
/// nothing rather than a number that looks measured.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_target_leaves_a_gap_not_a_zero() {
    const CONTROL_DB: &str = "ordo_usage_gap_control";
    const DEFAULT_SPACE_NAME: &str = "spc_usage_gap_default_side";
    const UNREACHABLE_SPACE_NAME: &str = "spc_usage_gap_unreachable_side";

    let control = raw_pool_with_empty_schema(CONTROL_DB).await;
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    control
        .rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = {}",
            ordo_exec::sql_lit(DEFAULT_SPACE_NAME)
        ))
        .await
        .unwrap();
    control
        .rows(&format!(
            "DELETE FROM \"$usage\" WHERE \"space\" IN ({}, {})",
            ordo_exec::sql_lit(DEFAULT_SPACE_NAME),
            ordo_exec::sql_lit(UNREACHABLE_SPACE_NAME)
        ))
        .await
        .unwrap();
    control
        .rows(&format!(
            "DELETE FROM \"$usage_gaps\" WHERE \"space\" = {}",
            ordo_exec::sql_lit(UNREACHABLE_SPACE_NAME)
        ))
        .await
        .unwrap();

    spacedb::attach(
        &control,
        &ordo_crypto::SpaceId(UNREACHABLE_SPACE_NAME.into()),
        &DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            user: "ordo".into(),
            db: "nowhere".into(),
            credential: "nowhere".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "mem_owner".into(),
        },
    )
    .await
    .expect("attached");

    let tx = control.begin().await.unwrap();
    tx.append_group(&rollup_group(DEFAULT_SPACE_NAME, 0, 2))
        .await
        .expect("default-side append");
    tx.commit().await.unwrap();

    let resolver: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        PoolConfig {
            size: 2,
            ..PoolConfig::from_env()
        },
        Arc::new(Installed::new().passwordless("nowhere")),
    ));
    let keys: Arc<dyn ordo_crypto::KeyProvider> = Arc::new(ordo_crypto::TestProvider::with_space(
        "spc_usage_gap_deploy",
    ));
    let deployment = ordo_host::deploy::Deployment::empty(
        ordo_crypto::SpaceId("spc_usage_gap_deploy".into()),
        control.clone(),
        keys,
    )
    .expect("an empty deployment");
    let spaces = ordo_host::spaces::Spaces::routed(
        deployment.clone(),
        resolver,
        ordo_exec::witness::Witness::new(control.clone()),
    );
    let rollup = Arc::new(UsageRollup {
        spaces,
        period: Duration::from_secs(1),
    });
    let metrics = Arc::new(Metrics::new());
    let sup = usage_supervisor(
        CONTROL_DB,
        control.clone(),
        deployment,
        metrics,
        vec![rollup],
    )
    .await;

    // The reachable side must roll up despite the other target being down
    // — this is the half `usage.rollup could not reach every target`
    // used to refuse outright.
    let default_row = poll(WAIT, || async {
        let (d, _) = usage::between(&control, DEFAULT_SPACE_NAME, 0, u64::MAX)
            .await
            .unwrap();
        d.into_iter().next()
    })
    .await
    .expect("the reachable side must still roll up despite the other target being down");
    assert_eq!(default_row.writes, 1, "{default_row:?}");
    assert_eq!(default_row.rows_written, 2, "{default_row:?}");

    // The unreachable side leaves a gap, and it must be a real, non-empty
    // reason string rather than a placeholder — an operator reading
    // `$usage_gaps` reads why, not just that.
    let gaps = poll(WAIT, || async {
        let g = usage::gaps_between(&control, UNREACHABLE_SPACE_NAME, 0, u64::MAX)
            .await
            .unwrap();
        (!g.is_empty()).then_some(g)
    })
    .await
    .expect("the unreachable side must leave a gap");
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(!gaps[0].reason.is_empty(), "{:?}", gaps[0]);
    assert_eq!(gaps[0].target, "127.0.0.1:1/nowhere", "{:?}", gaps[0]);

    // And a gap is not a zero-filled row: `$usage` must hold nothing for
    // this space, because nothing was measured.
    let (unreachable_rows, _) = usage::between(&control, UNREACHABLE_SPACE_NAME, 0, u64::MAX)
        .await
        .unwrap();
    assert!(
        unreachable_rows.is_empty(),
        "a gap is not a zero-filled row: {unreachable_rows:?}"
    );

    sup.shutdown(Duration::from_secs(5)).await;
}

// M1c's own done-when, that `usage.rollup` is a real [`UsageRollup`] worker
// and not a named empty slot, used to be asserted here against the slot
// list. U8 retired that list when `blob.gc`, its last entry, became a worker
// (`ordo_host::workers::BlobGc`), so there is no list left to assert
// against; the census half of the claim is still
// `without_a_run_handler_the_runner_does_not_start` in `tests/supervisor.rs`.

// ── the /usage route (17n M3) ─────────────────────────────────────────────

/// The deployment surface over one space's `$usage`/`$usage_replica` rows —
/// the same [`PlatformState`] shape `tests/spaces.rs` builds, minus the blob
/// tier this file has never needed.
async fn platform_state(
    db: &str,
    metrics: Arc<Metrics>,
) -> (ordo_host::serve::PlatformState, Pool) {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("DDL applies and the fixture seeds");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    let deployment = ordo_host::deploy::Deployment::pinned(
        "usage-route-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let access: Arc<dyn ordo_api::AccessChecker> = Arc::new(MemberAccess {
        fixture: f.clone(),
        pg: pg.clone(),
    });
    let state = ordo_host::serve::PlatformState::new(
        access,
        deployment,
        pool.clone(),
        f.keys.clone(),
        DEFAULT_SPACE.to_string(),
        metrics,
    );
    (state, pool)
}

async fn get_usage(
    state: &ordo_host::serve::PlatformState,
    bearer: Option<&str>,
    space: &str,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method("GET")
        .uri(format!("/usage?space={space}&from=0&to=9999999999"));
    if let Some(b) = bearer {
        req = req.header("authorization", format!("Bearer {b}"));
    }
    let resp = ordo_host::serve::platform_router(state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// `/usage` is the reserved-op pattern `/metrics` already has: gated on
/// [`ordo_host::serve::USAGE_OP`], checked against **the deployment's own
/// space**, never the `space` query parameter — an operator (a member of
/// the deployment's own space) reads it; a caller with no membership in
/// that space is refused, whatever space `space=` names.
#[tokio::test(flavor = "multi_thread")]
async fn the_usage_route_answers_the_operator_and_refuses_a_tenant_owner() {
    const DB: &str = "ordo_usage_route";
    let metrics = Arc::new(Metrics::new());
    let (state, pool) = platform_state(DB, metrics).await;
    pool.rows(&format!(
        "DELETE FROM \"$usage_replica\" WHERE \"space\" = {}",
        ordo_exec::sql_lit(DEFAULT_SPACE)
    ))
    .await
    .unwrap();
    let (period_start, period_end) = usage::period_of(1_000, 3_600);
    usage::add_replica(
        &pool,
        &ReplicaRow {
            space: ChainSpace(DEFAULT_SPACE.to_string()),
            period_start,
            period_end,
            replica: "r-usage-route".to_string(),
            requests: [("sql".to_string(), 2u64)].into_iter().collect(),
            rows_returned: 3,
            worker_runs: 0,
            state: ReplicaState::Closed,
            method_version: usage::METHOD_VERSION.to_string(),
        },
    )
    .await
    .unwrap();

    // An operator: a member of the deployment's own space.
    let (status, body) = get_usage(&state, Some("member:mem_1"), DEFAULT_SPACE).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(DEFAULT_SPACE), "{body}");

    // No membership in the deployment's own space at all — the property
    // `authorize` exists for: refused regardless of which space `space=`
    // asks about, tenant or otherwise.
    let (status, body) = get_usage(&state, Some("member:mem_nonexistent"), DEFAULT_SPACE).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // No token at all: refused the same way.
    let (status, _) = get_usage(&state, None, DEFAULT_SPACE).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
