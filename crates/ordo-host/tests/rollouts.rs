//! Increment 10 of `plan-versions.md`: **rollouts, the core** (§3.5, §3.7;
//! owner rulings of 2026-11-23, Q3, Q4, Q9, Q10, Q15).
//!
//! What these pin, over real Postgres:
//!
//! - a rolling rollout moves a batch per interval, the deployment's own
//!   space first, and finishes; each tenant's move is a `$pins` row naming
//!   the rollout; `ordo apply` starts one when `rollout.strategy` is set;
//! - one deployment-wide rollout at a time: a second, and any activation,
//!   is refused naming `ordo rollout status` and `abort`; one tenant still
//!   moves on its own to a hotfix version, and the rollout skips it;
//! - a tenant outside its maintenance window waits, and a held one;
//! - a failed batch pauses at `rollout.failure_threshold`, and an
//!   unreachable tenant database waits and never fails;
//! - the leader dies and a second node finishes the rollout;
//! - the deployment's own space last, a tenant made mid-rollout, abort,
//!   finish by hand, the stranded-tenant refusal;
//! - the door: the ops, their refusals, no reauth, the acts on the stream;
//!   retry of a refused move with no rollout.
//!
//! A regression looks like a tenant moved outside its window or while
//! held, a failure that does not pause, an unreachable database counted as
//! failed, two rollouts open, or a hotfix undone by the rollout.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_api::BoundParams;
use ordo_conformance::Pg;
use ordo_crypto::SpaceId;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver, SinglePool};
use ordo_exec::spacedb::{DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, Activation, ApplyOps};
use ordo_host::deploy_ops;
use ordo_host::rollouts::{
    self, DeploymentSpace, RolloutRefusal, RolloutState, Rollouts, StartOptions, Strategy,
    TenantState,
};
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::Versions;
use ordo_host::windows::{Schedule, Target, WindowSpec, Windows};

const V1: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

const V2: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]
";

const V3: &str = "domain tasks @ 0.3.0

entity task {
  title: text
  done:  bool
  due:   int?
  note:  text?
}

op openTasks unbounded =
  task[title, done]
";

/// V1 and a unique title: refused over a database that holds duplicates.
const V2_UNIQUE: &str = "domain tasks @ 0.2.0

entity task {
  title: text unique
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

/// The test's clock: 2026-11-05 12:00 UTC.
const NOW: u64 = 1_762_344_000;
const OWNER: &str = "deployment-owner:ada";

/// Every own-database test builds several databases.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 8,
        ..PoolConfig::from_env()
    }
}

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(cfg(&pg.db)).await.expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    pool
}

fn ops_on(
    pool: &Pool,
    pools: Arc<dyn PoolResolver>,
) -> (Arc<ApplyOps>, Arc<ordo_secevent::Recorder>) {
    let stream = Arc::new(ordo_secevent::Recorder::default());
    (
        ApplyOps::recording(pool.clone(), pools, "home", Some(stream.clone())),
        stream,
    )
}

fn shared_ops(pool: &Pool) -> (Arc<ApplyOps>, Arc<ordo_secevent::Recorder>) {
    ops_on(pool, Arc::new(SinglePool::new(pool.clone())))
}

async fn apply(pool: &Pool, source: &str) -> apply_op::Report {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        true,
    )
    .await
    .expect("applies")
}

/// Store `source` and bring the deployment's own database to it, with no
/// activation (`ordo apply --no-activate`).
async fn stage(pool: &Pool, source: &str) {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        false,
    )
    .await
    .expect("stored");
}

async fn tenant(pool: &Pool, id: &str) {
    assert!(tenants::reserve(pool, id, id, "dev:ada", 10, None)
        .await
        .expect("reserved"));
    tenants::activate(pool, id, 11).await.expect("active");
}

async fn on(pool: &Pool, id: &str) -> Option<String> {
    Versions::on(pool)
        .pin_of(id)
        .await
        .expect("read")
        .expect("a tenant")
        .version
}

async fn state_of(pool: &Pool, id: i64, t: &str) -> TenantState {
    Rollouts::on(pool)
        .tenant(id, t)
        .await
        .expect("read")
        .expect("in the rollout")
        .state
}

/// Three tenants on 0.1.0, and 0.2.0 stored.
async fn three(stem: &str) -> Pool {
    let pool = fresh(stem).await;
    for t in ["spc_a", "spc_b", "spc_c"] {
        tenant(&pool, t).await;
    }
    apply(&pool, V1).await;
    stage(&pool, V2).await;
    pool
}

fn rolling(interval: i64, size: i64) -> StartOptions {
    StartOptions {
        strategy: Some(Strategy::Rolling),
        batch_interval: Some(interval),
        batch_size: Some(size),
        ..Default::default()
    }
}

fn acts(stream: &ordo_secevent::Recorder) -> Vec<String> {
    stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect()
}

// ── rolling ─────────────────────────────────────────────────────────────

/// **A rolling rollout moves one batch per interval**, the deployment's own
/// space first (Q4), and finishes. `ordo apply` starts it when
/// `rollout.strategy` is set; each move is a `$pins` row naming the
/// rollout.
#[tokio::test(flavor = "multi_thread")]
async fn a_rolling_rollout_moves_a_batch_per_interval_and_the_deployment_first() {
    let pool = fresh("ordo_ro_rolling").await;
    for t in ["spc_a", "spc_b", "spc_c"] {
        tenant(&pool, t).await;
    }
    apply(&pool, V1).await;
    let r = Rollouts::on(&pool);
    // Unset, an apply moves every tenant at once, as before rollouts.
    assert_eq!(r.settings().await.unwrap().strategy, None);
    r.set_setting("rollout.strategy", Some("rolling"), OWNER, "batches", NOW)
        .await
        .expect("set");
    r.set_setting("batch-interval", Some("1m"), OWNER, "fast", NOW)
        .await
        .expect("set");
    r.set_setting("rollout.window", Some("3m"), OWNER, "fast", NOW)
        .await
        .expect("set");
    let s = r.settings().await.unwrap();
    assert_eq!((s.batch_interval, s.window), (60, 180));

    let report = apply(&pool, V2).await;
    let Activation::RolloutStarted {
        id,
        strategy,
        from,
        to,
    } = report.activation.clone()
    else {
        panic!("the apply started a rollout: {report:?}");
    };
    assert_eq!(
        (strategy.as_str(), from.as_deref(), to.as_str()),
        ("rolling", Some("0.1.0"), "0.2.0")
    );
    // Nothing moved yet: the apply returns when the rollout starts.
    let v = Versions::on(&pool);
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
    assert_eq!(on(&pool, "spc_a").await.as_deref(), Some("0.1.0"));
    let started = r.get(id).await.unwrap().unwrap();
    assert_eq!(started.batch_size, 1, "3 tenants over 3 batches");

    let (ops, stream) = shared_ops(&pool);
    let t0 = started.started_at;
    let t = rollouts::tick(&ops, "node-a", t0).await.expect("tick");
    assert!(t.deployment_moved, "{t:?}");
    assert_eq!(t.moved, ["spc_a"]);
    assert_eq!(on(&pool, "spc_a").await.as_deref(), Some("0.2.0"));
    assert_eq!(on(&pool, "spc_b").await.as_deref(), Some("0.1.0"));

    // **Mid-rollout** (Q4, Q5): the deployment's own space serves 0.2.0,
    // and every node reads that from the pins. "Active" is still 0.1.0, so
    // sign-in (`AuthApp`, built from the active version) and a new tenant
    // stay on 0.1.0 until the rollout finishes.
    assert_eq!(
        v.own_pin().await.unwrap().map(|(s, v, _)| (s, v)),
        Some(("home".to_string(), "0.2.0".to_string()))
    );
    let (all, _) = v.all_pins().await.unwrap();
    assert!(
        all.iter().any(|(s, v, _)| s == "home" && v == "0.2.0"),
        "{all:?}"
    );
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
    assert_eq!(
        v.new_tenant_version().await.unwrap().as_deref(),
        Some("0.1.0")
    );
    tenant(&pool, "spc_new").await;
    v.pin_new_tenant("spc_new", "t", t0 + 1).await.unwrap();
    assert_eq!(on(&pool, "spc_new").await.as_deref(), Some("0.1.0"));

    // Not due again until the interval has passed.
    let t = rollouts::tick(&ops, "node-a", t0 + 30).await.expect("tick");
    assert!(t.moved.is_empty(), "{t:?}");
    let t = rollouts::tick(&ops, "node-a", t0 + 61).await.expect("tick");
    assert_eq!(t.moved, ["spc_b"]);
    let t = rollouts::tick(&ops, "node-a", t0 + 122)
        .await
        .expect("tick");
    assert_eq!(t.moved, ["spc_c"]);
    assert!(!t.finished, "the tenant made mid-rollout is still to move");
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
    let t = rollouts::tick(&ops, "node-a", t0 + 183)
        .await
        .expect("tick");
    assert_eq!(t.moved, ["spc_new"]);
    assert!(t.finished, "{t:?}");
    // After the finish, all are on 0.2.0: "active", new tenants, the own
    // space and every tenant.
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.2.0"));
    assert_eq!(
        v.new_tenant_version().await.unwrap().as_deref(),
        Some("0.2.0")
    );
    assert_eq!(v.own_pin().await.unwrap().unwrap().1, "0.2.0");
    for t in ["spc_a", "spc_b", "spc_c", "spc_new"] {
        assert_eq!(on(&pool, t).await.as_deref(), Some("0.2.0"), "{t}");
    }

    let status = r.status(Some(id)).await.unwrap().unwrap();
    assert_eq!(status.rollout.state, RolloutState::Finished);
    assert_eq!(status.counts["moved"], 4);
    assert_eq!(status.pending(), 0);
    assert_eq!(status.rollout.leader.as_deref(), Some("node-a"));
    let h = v.pin_history("spc_c").await.unwrap();
    let last = h.last().unwrap();
    assert_eq!(
        (last.cause.as_str(), last.to.as_str()),
        ("rollout", "0.2.0")
    );
    let tagged = pool
        .rows(&format!(
            "SELECT count(*) FROM \"$pins\" WHERE \"rollout\" = {id} AND \"cause\" = 'rollout'"
        ))
        .await
        .unwrap();
    // 4 tenants and the own space.
    assert_eq!(tagged[0][0].as_deref(), Some("5"));
    assert_eq!(acts(&stream), ["rollout-finished"]);
    // A new rollout may start now the first is closed.
    stage(&pool, V3).await;
    r.start("0.3.0", &rolling(60, 3), OWNER, t0 + 200)
        .await
        .expect("a second, after the first finished");
}

/// **`all` moves every tenant in one batch.**
#[tokio::test(flavor = "multi_thread")]
async fn all_moves_every_tenant_in_one_turn() {
    let pool = three("ordo_ro_all").await;
    let r = Rollouts::on(&pool);
    let opts = StartOptions {
        strategy: Some(Strategy::All),
        ..Default::default()
    };
    let started = r.start("#2", &opts, OWNER, NOW).await.expect("started");
    let (ops, _) = shared_ops(&pool);
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert_eq!(t.moved.len(), 3, "{t:?}");
    assert!(t.finished);
    assert_eq!(
        r.get(started.id).await.unwrap().unwrap().state,
        RolloutState::Finished
    );
}

// ── one at a time, and the hotfix ───────────────────────────────────────

/// **One deployment-wide rollout at a time** (Q3). A second is refused,
/// and so is an activation, each naming `ordo rollout status` and `abort`.
/// One tenant moves on its own to a hotfix version, and the rollout skips
/// it.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_rollout_is_refused_and_a_hotfix_tenant_is_skipped() {
    let pool = three("ordo_ro_one").await;
    stage(&pool, V3).await;
    let r = Rollouts::on(&pool);
    let first = r
        .start("0.2.0", &rolling(600, 1), OWNER, NOW)
        .await
        .expect("started");

    let e = r
        .start("0.3.0", &rolling(60, 1), OWNER, NOW)
        .await
        .unwrap_err();
    assert!(matches!(e, RolloutRefusal::Open { .. }), "{e}");
    let text = e.to_string();
    assert!(
        text.contains("ordo rollout status") && text.contains("ordo rollout abort"),
        "{text}"
    );
    let e = apply_op::activate(&pool, "0.3.0", OWNER).await.unwrap_err();
    assert_eq!(e.class(), "rollout_open", "{e}");
    assert!(e.to_string().contains("ordo rollout abort"), "{e}");

    // The hotfix: spc_c moves to 0.3.0 on its own while the rollout runs.
    Versions::on(&pool)
        .pin("spc_c", "0.3.0", OWNER, NOW + 1)
        .await
        .expect("one tenant moves on its own");
    let (ops, _) = shared_ops(&pool);
    let t = rollouts::tick(&ops, "n", NOW + 2).await.unwrap();
    assert_eq!(t.moved, ["spc_a"]);
    assert_eq!(
        state_of(&pool, first.id, "spc_c").await,
        TenantState::Skipped
    );
    let reason = r.tenant(first.id, "spc_c").await.unwrap().unwrap().reason;
    assert!(reason.contains("moved on its own to 0.3.0"), "{reason}");
    for at in [NOW + 700, NOW + 1400] {
        rollouts::tick(&ops, "n", at).await.unwrap();
    }
    assert_eq!(on(&pool, "spc_b").await.as_deref(), Some("0.2.0"));
    // The rollout never undid the hotfix.
    assert_eq!(on(&pool, "spc_c").await.as_deref(), Some("0.3.0"));
    assert_eq!(
        r.get(first.id).await.unwrap().unwrap().state,
        RolloutState::Finished
    );
}

// ── windows and holds ───────────────────────────────────────────────────

/// **A tenant outside its window waits** (§3.5) and moves when its window
/// opens. A tenant no window covers moves in its batch.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_outside_its_window_waits() {
    let pool = three("ordo_ro_window").await;
    let start = NOW as i64 + 3_600;
    Windows::on(&pool)
        .set(
            &WindowSpec {
                id: None,
                target: Target::Only(["spc_b".to_string()].into_iter().collect()),
                schedule: Schedule::Range {
                    start,
                    end: start + 3_600,
                },
                message: "upgrades".into(),
            },
            OWNER,
            NOW as i64,
        )
        .await
        .expect("a window");
    let r = Rollouts::on(&pool);
    let opts = StartOptions {
        strategy: Some(Strategy::All),
        ..Default::default()
    };
    let started = r.start("0.2.0", &opts, OWNER, NOW).await.unwrap();
    let (ops, _) = shared_ops(&pool);
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert_eq!(t.moved, ["spc_a", "spc_c"]);
    assert_eq!(t.waiting.len(), 1, "{t:?}");
    assert!(
        t.waiting[0].1.contains("outside its maintenance window"),
        "{t:?}"
    );
    assert!(!t.finished);
    assert_eq!(
        state_of(&pool, started.id, "spc_b").await,
        TenantState::Waiting
    );
    assert_eq!(on(&pool, "spc_b").await.as_deref(), Some("0.1.0"));

    // Inside the window it moves, and the rollout finishes.
    let t = rollouts::tick(&ops, "n", start as u64 + 60).await.unwrap();
    assert_eq!(t.moved, ["spc_b"]);
    assert!(t.finished);
}

/// **A held tenant waits until the hold is lifted.**
#[tokio::test(flavor = "multi_thread")]
async fn a_held_tenant_waits_until_the_hold_is_lifted() {
    let pool = three("ordo_ro_hold").await;
    let r = Rollouts::on(&pool);
    r.hold("spc_a", Some("their launch is today"), OWNER, NOW)
        .await
        .expect("held");
    let e = r
        .hold("spc_nobody", Some("x"), OWNER, NOW)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no_tenant"), "{e}");
    let opts = StartOptions {
        strategy: Some(Strategy::All),
        ..Default::default()
    };
    r.start("0.2.0", &opts, OWNER, NOW).await.unwrap();
    let (ops, _) = shared_ops(&pool);
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert_eq!(t.moved, ["spc_b", "spc_c"]);
    assert!(t.waiting[0].1.contains("their launch is today"), "{t:?}");
    assert!(t.waiting[0].1.contains("--clear"), "{t:?}");
    r.hold("spc_a", None, OWNER, NOW + 1).await.unwrap();
    let t = rollouts::tick(&ops, "n", NOW + 2).await.unwrap();
    assert_eq!(t.moved, ["spc_a"]);
    assert!(t.finished);
}

// ── tenants with their own database: failure and unreachable ────────────

fn drop_database(name: &str) {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
}

async fn empty_db(db: &str) -> Pool {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("an empty database");
    Pool::connect(cfg(db)).await.expect("a pool")
}

async fn tenant_db_at(db: &str, program: &Program) -> Pool {
    let pool = empty_db(db).await;
    ordo_exec::spacedb::apply_schema_at_version(
        &pool,
        &program.ddl,
        &program.version,
        "test",
        "A1 — new entity `task`",
        1_000,
    )
    .await
    .expect("the schema and its version row");
    pool
}

fn attach_row(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.to_string(),
        credential: "tf".into(),
        custody: DbCustody::PlatformManaged,
        attached_at: 1,
        attached_by: "test".into(),
    }
}

/// **A failed batch pauses** at `rollout.failure_threshold`, and the
/// failed tenant serves its old version. **An unreachable database waits,
/// never fails** (Q10). Then skip, resume, and the rollout finishes.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_batch_pauses_and_an_unreachable_database_waits() {
    let _serial = SERIAL.lock().await;
    let control = empty_db("ordo_ro_own_control").await;
    tenants::ensure_table(&control).await.unwrap();
    ordo_exec::spacedb::ensure_table(&control).await.unwrap();
    let v1 = Program::compile(V1).unwrap();
    let dup = tenant_db_at("ordo_ro_own_dup", &v1).await;
    let _fine = tenant_db_at("ordo_ro_own_fine", &v1).await;
    drop_database("ordo_ro_own_gone");
    dup.rows(
        r#"INSERT INTO "task" ("$space", "id", "rev", "title", "done") VALUES
           ('s', 't_1', 1, 'same', false), ('s', 't_2', 1, 'same', false)"#,
    )
    .await
    .expect("duplicate titles");
    for (space, db) in [
        ("spc_dup", "ordo_ro_own_dup"),
        ("spc_fine", "ordo_ro_own_fine"),
        ("spc_gone", "ordo_ro_own_gone"),
    ] {
        tenant(&control, space).await;
        ordo_exec::spacedb::attach(&control, &SpaceId(space.into()), &attach_row(db))
            .await
            .expect("attached");
    }
    tenant(&control, "spc_shared").await;
    let pools: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        cfg("unused"),
        Arc::new(Installed::new().as_configured("tf")),
    ));
    apply_op::run_program(&control, pools.as_ref(), &v1, vec![], "t", "t", true)
        .await
        .expect("v1");
    let v2 = Program::compile(V2_UNIQUE).unwrap();
    apply_op::run_program(&control, pools.as_ref(), &v2, vec![], "t", "t", false)
        .await
        .expect("v2 stored");
    // A unique constraint is not a change an apply makes for you: the
    // deployment's own database takes it the way `ordo migrate` gives it.
    let (_, applied) = ordo_host::schema_apply::apply(&control, &v2, "test", "migrate").await;
    applied.expect("the deployment's own database takes the change");

    let r = Rollouts::on(&control);
    let opts = StartOptions {
        strategy: Some(Strategy::All),
        failure_threshold: Some(1),
        ..Default::default()
    };
    let started = r.start("0.2.0", &opts, OWNER, NOW).await.expect("started");
    let (ops, stream) = ops_on(&control, pools.clone());
    let t = rollouts::tick(&ops, "n", NOW).await.expect("tick");
    assert!(t.paused, "{t:?}");
    assert_eq!(t.failed.len(), 1, "{t:?}");
    assert_eq!(t.failed[0].0, "spc_dup");
    assert!(t.moved.contains(&"spc_fine".to_string()), "{t:?}");
    assert!(t.moved.contains(&"spc_shared".to_string()), "{t:?}");
    let gone = t
        .waiting
        .iter()
        .find(|(s, _)| s == "spc_gone")
        .expect("waits");
    assert!(gone.1.contains("cannot be reached"), "{gone:?}");
    assert_eq!(
        state_of(&control, started.id, "spc_gone").await,
        TenantState::Waiting
    );
    assert_eq!(on(&control, "spc_dup").await.as_deref(), Some("0.1.0"));
    let paused = r.get(started.id).await.unwrap().unwrap();
    assert_eq!(paused.state, RolloutState::Paused);
    assert!(
        paused.reason.contains("ordo rollout resume"),
        "{}",
        paused.reason
    );
    // Paused: a tick moves nothing.
    let t = rollouts::tick(&ops, "n", NOW + 10).await.unwrap();
    assert!(t.moved.is_empty() && t.failed.is_empty());

    // Skip the failed tenant, resume. The unreachable one still waits.
    r.skip("spc_dup", OWNER, Some("data to fix by hand"), NOW + 20)
        .await
        .expect("skipped");
    r.resume(NOW + 21).await.expect("resumed");
    let t = rollouts::tick(&ops, "n", NOW + 22).await.unwrap();
    assert!(!t.paused && !t.finished, "{t:?}");
    assert_eq!(t.waiting.len(), 1, "{t:?}");

    // Its database comes back: it moves, and the rollout finishes.
    tenant_db_at("ordo_ro_own_gone", &v1).await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let t = rollouts::tick(&ops, "n", NOW + 30).await.unwrap();
    assert_eq!(t.moved, ["spc_gone"], "{t:?}");
    assert!(t.finished, "{t:?}");
    assert_eq!(on(&control, "spc_dup").await.as_deref(), Some("0.1.0"));
    assert_eq!(acts(&stream), ["rollout-paused", "rollout-finished"]);
}

// ── the leader ──────────────────────────────────────────────────────────

/// **The leader dies and a second node finishes.** While a node holds the
/// rollout lock, another node's turn does nothing. When its connection
/// ends, the lock is free, and the second node moves the tenant the first
/// left mid-move and the rest.
#[tokio::test(flavor = "multi_thread")]
async fn the_leader_dies_and_another_node_finishes() {
    let pool = three("ordo_ro_leader").await;
    let r = Rollouts::on(&pool);
    let started = r.start("0.2.0", &rolling(60, 1), OWNER, NOW).await.unwrap();
    let (ops, _) = shared_ops(&pool);
    let t = rollouts::tick(&ops, "node-a", NOW).await.unwrap();
    assert_eq!(t.moved, ["spc_a"]);

    // Node A is mid-turn: it holds the lock, and has marked spc_b moving.
    let a = Pool::connect(PoolConfig {
        size: 1,
        ..cfg(&pool_db(&pool).await)
    })
    .await
    .unwrap();
    let held = a.begin().await.unwrap();
    held.rows(&format!(
        "SELECT pg_advisory_xact_lock({}, 0)",
        rollouts::ROLLOUT_LOCK_NS
    ))
    .await
    .unwrap();
    pool.rows(&format!(
        "UPDATE \"$rollout_tenants\" SET \"state\" = 'moving' WHERE \"rollout\" = {} \
         AND \"space\" = 'spc_b'",
        started.id
    ))
    .await
    .unwrap();
    let t = rollouts::tick(&ops, "node-b", NOW + 61).await.unwrap();
    assert!(t.not_leader, "{t:?}");
    assert!(t.moved.is_empty());

    // Node A dies: its connection closes and the lock goes with it.
    drop(held);
    drop(a);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let t = rollouts::tick(&ops, "node-b", NOW + 62).await.unwrap();
    assert!(!t.not_leader);
    assert_eq!(t.moved, ["spc_b"], "the half-done move first");
    let t = rollouts::tick(&ops, "node-b", NOW + 123).await.unwrap();
    assert_eq!(t.moved, ["spc_c"]);
    assert!(t.finished);
    let done = r.get(started.id).await.unwrap().unwrap();
    assert_eq!(done.leader.as_deref(), Some("node-b"));
}

async fn pool_db(pool: &Pool) -> String {
    pool.rows("SELECT current_database()").await.unwrap()[0][0]
        .clone()
        .unwrap()
}

// ── the deployment's own space last, and new tenants ────────────────────

/// **`rollout.deployment_space = last`**: the active version flips when
/// the last tenant has moved. A tenant made mid-rollout starts on the old
/// active version and is moved too (§1.3).
#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_last_and_a_tenant_made_mid_rollout_moves() {
    let pool = three("ordo_ro_last").await;
    let r = Rollouts::on(&pool);
    r.set_setting(
        "deployment_space",
        Some("last"),
        OWNER,
        "flip at the end",
        NOW,
    )
    .await
    .unwrap();
    let started = r.start("0.2.0", &rolling(60, 2), OWNER, NOW).await.unwrap();
    assert_eq!(started.deployment_space, DeploymentSpace::Last);
    let (ops, _) = shared_ops(&pool);
    let v = Versions::on(&pool);
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert!(!t.deployment_moved);
    assert_eq!(t.moved, ["spc_a", "spc_b"]);
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));

    // A tenant made now starts on the active version, 0.1.0.
    tenant(&pool, "spc_new").await;
    v.pin_new_tenant("spc_new", "t", NOW + 5).await.unwrap();
    assert_eq!(on(&pool, "spc_new").await.as_deref(), Some("0.1.0"));
    let t = rollouts::tick(&ops, "n", NOW + 61).await.unwrap();
    assert_eq!(t.moved, ["spc_c", "spc_new"]);
    assert!(t.deployment_moved && t.finished, "{t:?}");
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.2.0"));
    let s = r.status(Some(started.id)).await.unwrap().unwrap();
    assert_eq!(s.deployment.unwrap().state, TenantState::Moved);
}

// ── abort, finish, stranded ─────────────────────────────────────────────

/// **Abort stops new moves; moved tenants stay. Finish by hand skips the
/// rest and moves the deployment.**
#[tokio::test(flavor = "multi_thread")]
async fn abort_finish_and_the_stranded_refusal() {
    let pool = three("ordo_ro_abort").await;
    let r = Rollouts::on(&pool);
    let v = Versions::on(&pool);
    let started = r.start("0.2.0", &rolling(60, 1), OWNER, NOW).await.unwrap();
    let (ops, _) = shared_ops(&pool);
    rollouts::tick(&ops, "n", NOW).await.unwrap();
    let e = r.abort("", NOW + 1).await.unwrap_err();
    assert!(e.to_string().contains("reason"), "{e}");
    let aborted = r.abort("wrong build", NOW + 1).await.unwrap();
    assert_eq!(aborted.state, RolloutState::Aborted);
    let t = rollouts::tick(&ops, "n", NOW + 120).await.unwrap();
    assert!(t.moved.is_empty());
    assert_eq!(on(&pool, "spc_a").await.as_deref(), Some("0.2.0"));
    assert_eq!(on(&pool, "spc_b").await.as_deref(), Some("0.1.0"));
    let e = r.resume(NOW + 2).await.unwrap_err();
    assert!(matches!(e, RolloutRefusal::NoneOpen { .. }), "{e}");
    assert_eq!(started.id, aborted.id);

    stage(&pool, V3).await;
    let second = r
        .start("0.3.0", &rolling(60, 1), OWNER, NOW + 3)
        .await
        .unwrap();
    rollouts::tick(&ops, "n", NOW + 3).await.unwrap();
    let finished = r
        .finish("home", OWNER, Some("the rest can wait"), NOW + 4)
        .await
        .unwrap();
    assert_eq!(finished.state, RolloutState::Finished);
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.3.0"));
    let s = r.status(Some(second.id)).await.unwrap().unwrap();
    assert_eq!(s.counts["skipped"], 2, "{:?}", s.counts);
    assert!(s
        .tenants
        .iter()
        .any(|t| t.reason.contains("the rest can wait")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rollout_that_would_strand_tenants_is_refused() {
    let pool = three("ordo_ro_strand").await;
    stage(&pool, V3).await;
    // 0.1.0 is active and every tenant is on it. A list without it cannot
    // be stored; one is written straight, as an older binary might have.
    pool.rows(
        "INSERT INTO \"$version_settings\" (\"setting\", \"value\", \"actor\", \"reason\", \"at\") \
         VALUES ('versions.served', '0.3.0', 'x', 'x', 1)",
    )
    .await
    .unwrap();
    let e = Rollouts::on(&pool)
        .start("0.3.0", &rolling(60, 1), OWNER, NOW)
        .await
        .unwrap_err();
    assert!(matches!(e, RolloutRefusal::Stranded { .. }), "{e}");
    assert!(
        e.to_string()
            .contains("ordo versions set served 0.3.0,0.1.0"),
        "{e}"
    );

    // Below versions.minimum, and nothing stored under the name.
    let pool = three("ordo_ro_strand_min").await;
    stage(&pool, V3).await;
    Versions::on(&pool)
        .set_setting(
            ordo_host::versions::VersionSetting::Minimum,
            Some("0.1.0"),
            OWNER,
            "x",
            NOW,
        )
        .await
        .unwrap();
    let e = Rollouts::on(&pool)
        .start("9.9.9", &rolling(60, 1), OWNER, NOW)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no version is stored"), "{e}");
}

/// **A version the deployment's own database does not hold is refused**,
/// naming `ordo evolve`; one already active with every tenant on it has
/// nothing to move.
#[tokio::test(flavor = "multi_thread")]
async fn a_rollout_with_nothing_to_move_or_a_database_behind_is_refused() {
    let pool = fresh("ordo_ro_nothing").await;
    tenant(&pool, "spc_a").await;
    apply(&pool, V1).await;
    let e = Rollouts::on(&pool)
        .start("0.1.0", &rolling(60, 1), OWNER, NOW)
        .await
        .unwrap_err();
    assert!(matches!(e, RolloutRefusal::Nothing { .. }), "{e}");
    // Stored without its tables on this database.
    stage(&pool, V2).await;
    pool.rows("ALTER TABLE \"task\" DROP COLUMN \"due\"")
        .await
        .unwrap();
    let e = Rollouts::on(&pool)
        .start("0.2.0", &rolling(60, 1), OWNER, NOW)
        .await
        .unwrap_err();
    assert!(matches!(e, RolloutRefusal::DeploymentBehind { .. }), "{e}");
    assert!(
        e.to_string()
            .contains("ordo evolve --from 0.1.0 --to 0.2.0 --run"),
        "{e}"
    );
}

// ── the door ────────────────────────────────────────────────────────────

/// **The backfill settings are stored in Ordo** (Q15): set and cleared
/// through `setVersionSetting` with a reason, read back by
/// `versionSettings`, refused by name when a value is not one, and what a
/// stored step's backfill runs with.
#[tokio::test(flavor = "multi_thread")]
async fn the_backfill_settings_are_stored_and_a_step_runs_with_them() {
    let pool = three("ordo_ro_backfill_settings").await;
    let (ops, _stream) = shared_ops(&pool);
    let run = |name: &'static str, v: serde_json::Value| {
        let ops = &ops;
        let b = BoundParams(v.as_object().unwrap().clone());
        async move { deploy_ops::execute(ops, name, "home", &b, "sys:updater").await }
    };
    let defaults = ops.step_context().await.expect("reads").settings;
    assert_eq!(defaults, ordo_host::backfill::BackfillSettings::default());

    let set = run(
        deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({"setting": "backfill.chunk_rows", "value": "250", "reason": "small"}),
    )
    .await
    .expect("a backfill setting through the door");
    assert_eq!(set.rows[0]["backfill"]["chunkRows"], 250);
    assert_eq!(set.rows[0]["changed"], true);
    run(
        deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({"setting": "backfill.chunk-timeout", "value": "2m", "reason": "slow"}),
    )
    .await
    .expect("the short form of a name");
    run(
        deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({"setting": "backfill.budget", "value": "40", "reason": "pace"}),
    )
    .await
    .expect("a budget");
    let again = run(
        deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({"setting": "backfill.chunk_rows", "value": "250", "reason": "same"}),
    )
    .await
    .unwrap();
    assert_eq!(again.rows[0]["changed"], false);

    // Refusals name the setting and the command that sets it.
    for (setting, value) in [
        ("backfill.chunk_rows", "0"),
        ("backfill.chunk_rows", "many"),
        ("backfill.chunk_timeout", "2d"),
        ("backfill.max_attempts", "0"),
        ("backfill.nope", "1"),
    ] {
        let e = run(
            deploy_ops::SET_VERSION_SETTING,
            serde_json::json!({"setting": setting, "value": value, "reason": "x"}),
        )
        .await
        .unwrap_err();
        assert_eq!(
            e.code,
            ordo_api::ErrorCode::InvalidParam,
            "{setting}={value}"
        );
        assert!(e.message.contains("ordo versions set"), "{}", e.message);
    }
    let e = run(
        deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({"setting": "backfill.chunk_rows", "value": "9", "reason": ""}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::InvalidParam);

    let read = run(deploy_ops::VERSION_SETTINGS, serde_json::json!({}))
        .await
        .unwrap();
    let b = &read.rows[0]["backfill"];
    assert_eq!(b["chunkRows"], 250);
    assert_eq!(b["chunkTimeout"], 120);
    assert_eq!(b["maxAttempts"], 5);
    assert_eq!(b["budget"], 40);
    let history = read.rows[0]["history"].as_array().unwrap();
    assert!(
        history
            .iter()
            .any(|c| c["setting"] == "backfill.chunk_rows" && c["reason"] == "small"),
        "{history:?}"
    );

    // A stored step runs its backfill with them.
    let ctx = ops.step_context().await.expect("reads").settings;
    assert_eq!(ctx.chunk_rows, 250);
    assert_eq!(ctx.chunk_timeout, std::time::Duration::from_secs(120));
    assert_eq!(ctx.budget, Some(40));

    // Cleared, a setting reads as its default again.
    run(
        deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({"setting": "backfill.budget", "reason": "no pause"}),
    )
    .await
    .expect("cleared");
    assert_eq!(ops.step_context().await.unwrap().settings.budget, None);
}

/// **The door**: start, status, skip, retry, hold, abort, the settings
/// through `setVersionSetting`, refusals as their codes, no reauth, and one
/// stream entry per act by a person (Q9).
#[tokio::test(flavor = "multi_thread")]
async fn the_door_starts_reads_and_controls_a_rollout() {
    let pool = three("ordo_ro_door").await;
    let (ops, stream) = shared_ops(&pool);
    let run = |name: &'static str, space: &'static str, v: serde_json::Value| {
        let ops = &ops;
        let b = BoundParams(v.as_object().unwrap().clone());
        async move { deploy_ops::execute(ops, name, space, &b, "sys:updater").await }
    };
    let set = run(
        deploy_ops::SET_VERSION_SETTING,
        "home",
        serde_json::json!({"setting": "rollout.failure_threshold", "value": "2", "reason": "two"}),
    )
    .await
    .expect("a rollout setting through the door");
    assert_eq!(set.rows[0]["rollout"]["failureThreshold"], 2);
    assert_eq!(set.rows[0]["changed"], true);
    let e = run(
        deploy_ops::SET_VERSION_SETTING,
        "home",
        serde_json::json!({"setting": "rollout.strategy", "value": "blue-green", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::InvalidParam);
    let read = run(deploy_ops::VERSION_SETTINGS, "home", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(read.rows[0]["rollout"]["failureThreshold"], 2);
    assert_eq!(read.rows[0]["rollout"]["strategy"], serde_json::Value::Null);

    let none = run(deploy_ops::ROLLOUT_STATUS, "home", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(none.rows[0]["id"], serde_json::Value::Null);

    let started = run(
        deploy_ops::ROLLOUT_START,
        "home",
        serde_json::json!({"version": "0.2.0", "strategy": "rolling", "batchInterval": "10m",
                           "batchSize": 1}),
    )
    .await
    .expect("started");
    let row = &started.rows[0];
    assert_eq!(row["state"], "running");
    assert_eq!(row["batchInterval"], 600);
    assert_eq!(row["failureThreshold"], 2);
    assert_eq!(row["counts"]["queued"], 3);
    assert_eq!(row["deploymentStep"]["state"], "queued");
    let e = run(
        deploy_ops::ROLLOUT_START,
        "home",
        serde_json::json!({"version": "0.2.0"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::ApplyRefused);
    assert!(e.message.contains("ordo rollout status"), "{}", e.message);

    run(
        deploy_ops::ROLLOUT_SKIP,
        "home",
        serde_json::json!({"tenant": "spc_b", "reason": "later"}),
    )
    .await
    .expect("skipped");
    let e = run(
        deploy_ops::ROLLOUT_SKIP,
        "home",
        serde_json::json!({"tenant": "spc_zz"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::NotFound);
    let e = run(
        deploy_ops::ROLLOUT_RETRY,
        "home",
        serde_json::json!({"tenant": "spc_a"}),
    )
    .await
    .unwrap_err();
    assert!(e.message.contains("only a failed tenant"), "{}", e.message);
    run(
        deploy_ops::ROLLOUT_HOLD,
        "home",
        serde_json::json!({"tenant": "spc_c", "reason": "busy week"}),
    )
    .await
    .expect("held");
    let e = run(
        deploy_ops::ROLLOUT_HOLD,
        "home",
        serde_json::json!({"tenant": "spc_c"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::InvalidParam);
    let s = run(deploy_ops::ROLLOUT_STATUS, "home", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(s.rows[0]["counts"]["skipped"], 1);
    let e = run(deploy_ops::ROLLOUT_RESUME, "home", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(e.message.contains("not_paused"), "{}", e.message);
    let aborted = run(
        deploy_ops::ROLLOUT_ABORT,
        "home",
        serde_json::json!({"reason": "test over"}),
    )
    .await
    .unwrap();
    assert_eq!(aborted.rows[0]["state"], "aborted");

    // Bound to a tenant, none is there.
    for op in rollouts::OPS {
        let e = run(op, "spc_a", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.code, ordo_api::ErrorCode::NotFound, "{op}");
    }
    // No reauth, at any level, for the controls (owner, 2026-11-05).
    use ordo_host::security_level::SecurityLevel;
    let presence = ordo_host::presence::PresencePolicy::standard();
    for op in rollouts::OPS.into_iter().filter(|o| *o != rollouts::STATUS) {
        assert!(deploy_ops::NO_PRESENCE.contains(&op), "{op}");
        assert!(
            !presence
                .clone()
                .with_level(SecurityLevel::High)
                .requires(op, ordo_api::OpKind::Write),
            "{op}"
        );
    }
    assert_eq!(
        acts(&stream),
        [
            "version-setting",
            "rollout-started",
            "rollout-skipped",
            "rollout-held",
            "rollout-aborted"
        ]
    );
}

/// **Retry a refused move with no rollout** (increment 7 left it retried
/// only by a new version): the move opens again for the catch-up.
#[tokio::test(flavor = "multi_thread")]
async fn retry_reopens_a_refused_move() {
    let pool = three("ordo_ro_retry").await;
    Versions::on(&pool).ensure().await.unwrap();
    pool.rows(
        "INSERT INTO \"$moves\" (\"space\", \"from\", \"to\", \"phase\", \"reason\", \
           \"soak_until\", \"started_at\") \
         VALUES ('spc_a', '0.1.0', '0.2.0', 'refused', 'duplicate titles', 0, 1)",
    )
    .await
    .unwrap();
    let r = Rollouts::on(&pool);
    let done = r.retry("spc_a", NOW).await.expect("reopened");
    assert_eq!(done, rollouts::Retried::Move { to: "0.2.0".into() });
    let m = Versions::on(&pool).moves().await.unwrap();
    assert_eq!(m[0].phase, ordo_host::versions::MovePhase::Expanding);
    assert_eq!(m[0].reason, "");
    let e = r.retry("spc_a", NOW).await.unwrap_err();
    assert!(e.to_string().contains("no refused move"), "{e}");
}
