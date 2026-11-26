//! Increment 16 of `plan-versions.md`: **tenant builds** (§3.8, 27t). A
//! named build serves one stored version to the tenants named, while the
//! rest follow the active version.
//!
//! What these pin, over real Postgres:
//!
//! - a beta tenant serves the build's version while the rest serve main;
//!   an activation, a rollback and a rollout leave it there; setting the
//!   build moves it; leaving returns it to the active version; each pin is
//!   a `$pins` row with cause `build`;
//! - `ordo versions pin` and a move off a revoked version are refused for
//!   a tenant a build holds, naming `ordo tenant-builds leave`;
//! - a tenant with a database of its own has it brought to the build's
//!   version before its pin moves;
//! - the refusals: an unknown build, a second build, a bad name, a build
//!   that still holds tenants, a version the deployment's own database
//!   does not hold, a revoked version, a version that skipped the gate,
//!   and the ops bound to a tenant's space.
//!
//! A regression looks like a beta tenant moved back by an activation or a
//! rollout, or a pin moved to a version its database does not hold.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::Pg;
use ordo_crypto::SpaceId;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver, SinglePool};
use ordo_exec::spacedb::{DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::ApplyOps;
use ordo_host::environments::Environments;
use ordo_host::rollouts::{self, Rollouts, StartOptions, Strategy};
use ordo_host::schema_apply::Program;
use ordo_host::tenant_builds::{self as tb, TenantBuilds};
use ordo_host::tenants;
use ordo_host::versions::{PinCause, Versions};

const V1: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

/// The branch: V1 and a field.
const BETA: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]

op dueTasks unbounded =
  task[title, due]
";

/// Main moves on: the branch's field and one more.
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

const NOW: u64 = 1_762_344_000;
const OWNER: &str = "deployment-owner:ada";

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

async fn run(pool: &Pool, pools: &dyn PoolResolver, source: &str, activate: bool) {
    let program = Program::compile(source).expect("compiles");
    run_all_at_once(pool, pools, &program, vec![], "dev:ada", "t", activate)
        .await
        .expect("applies");
}

async fn apply(pool: &Pool, source: &str, activate: bool) {
    run(pool, &SinglePool::new(pool.clone()), source, activate).await
}

async fn tenant(pool: &Pool, id: &str) {
    assert!(tenants::reserve(pool, id, id, "dev:ada", 10, None)
        .await
        .expect("reserved"));
    tenants::activate(pool, id, 11).await.expect("active");
}

async fn on(pool: &Pool, id: &str) -> String {
    Versions::on(pool)
        .pin_of(id)
        .await
        .expect("read")
        .expect("a tenant")
        .version
        .expect("pinned")
}

fn ops_on(pool: &Pool, pools: Arc<dyn PoolResolver>) -> Arc<ApplyOps> {
    ApplyOps::recording(
        pool.clone(),
        pools,
        "home",
        Some(Arc::new(ordo_secevent::Recorder::default())),
    )
}

fn shared(pool: &Pool) -> Arc<ApplyOps> {
    ops_on(pool, Arc::new(SinglePool::new(pool.clone())))
}

/// Three tenants on main (0.1.0), and the branch (0.2.0) stored beside it,
/// not activated, with the deployment's own database brought to it.
async fn three(stem: &str) -> Pool {
    let pool = fresh(stem).await;
    for t in ["spc_beta", "spc_b", "spc_c"] {
        tenant(&pool, t).await;
    }
    apply(&pool, V1, true).await;
    apply(&pool, BETA, false).await;
    pool
}

/// **A beta tenant serves the branch while the rest serve main**, and
/// stays there through an activation, a rollback and a rollout; the build
/// moves it, and leaving returns it to main.
#[tokio::test(flavor = "multi_thread")]
async fn a_beta_tenant_serves_the_branch_while_the_rest_serve_main() {
    let pool = three("ordo_tb_beta").await;
    let ops = shared(&pool);
    let b = TenantBuilds::on(&ops);

    b.set("beta", "0.2.0", "public test of due dates", OWNER, NOW)
        .await
        .expect("a build");
    let joined = b.join("beta", "spc_beta", OWNER, NOW + 1).await.unwrap();
    assert_eq!(joined.version, "0.2.0");
    assert_eq!(on(&pool, "spc_beta").await, "0.2.0");
    assert_eq!(on(&pool, "spc_b").await, "0.1.0");
    assert_eq!(on(&pool, "spc_c").await, "0.1.0");
    let last = Versions::on(&pool).pin_history("spc_beta").await.unwrap();
    let last = last.last().unwrap();
    assert_eq!(
        (last.cause.as_str(), last.actor.as_str()),
        (PinCause::Build.as_str(), OWNER)
    );

    // Main moves on: every tenant but the beta one.
    apply(&pool, V3, true).await;
    assert_eq!(on(&pool, "spc_beta").await, "0.2.0");
    assert_eq!(on(&pool, "spc_b").await, "0.3.0");
    // A rollback to 0.1.0 leaves it too.
    Versions::on(&pool)
        .activate("0.1.0", OWNER, ordo_commandlog::Timestamp(NOW + 2))
        .await
        .unwrap();
    assert_eq!(on(&pool, "spc_beta").await, "0.2.0");
    assert_eq!(on(&pool, "spc_c").await, "0.1.0");

    // A rollout to 0.3.0 moves the rest and never queues the beta tenant.
    let r = Rollouts::on(&pool);
    let opts = StartOptions {
        strategy: Some(Strategy::All),
        ..Default::default()
    };
    let started = r.start("0.3.0", &opts, OWNER, NOW + 3).await.unwrap();
    let t = rollouts::tick(&ops, "n", NOW + 3).await.unwrap();
    assert!(t.finished, "{t:?}");
    assert!(!t.moved.contains(&"spc_beta".to_string()), "{t:?}");
    assert!(r.tenant(started.id, "spc_beta").await.unwrap().is_none());
    assert_eq!(on(&pool, "spc_b").await, "0.3.0");
    assert_eq!(on(&pool, "spc_beta").await, "0.2.0");

    // Only the build moves it: a pin by hand names `leave`.
    let e = Versions::on(&pool)
        .pin("spc_beta", "0.3.0", OWNER, NOW + 4)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("in_tenant_build"), "{e}");
    assert!(e.contains("ordo tenant-builds leave spc_beta"), "{e}");

    // The build is set to main's version: its tenant moves with it.
    let moved = b
        .set("beta", "0.3.0", "due dates shipped", OWNER, NOW + 5)
        .await
        .unwrap();
    assert_eq!(moved.tenants, ["spc_beta"]);
    assert_eq!(on(&pool, "spc_beta").await, "0.3.0");
    b.set("beta", "0.2.0", "back to the branch", OWNER, NOW + 6)
        .await
        .unwrap();
    assert_eq!(on(&pool, "spc_beta").await, "0.2.0");

    // A build with a tenant is not removed; leaving returns it to main.
    let e = b.remove("beta").await.unwrap_err().to_string();
    assert!(
        e.contains("build_has_tenants") && e.contains("spc_beta"),
        "{e}"
    );
    let left = b.leave("spc_beta", OWNER, NOW + 7).await.unwrap();
    assert_eq!(left.version, "0.3.0");
    assert_eq!(on(&pool, "spc_beta").await, "0.3.0");
    b.remove("beta").await.expect("removed");
    assert!(b.list().await.unwrap().is_empty());
}

/// A move off a revoked version is refused for a tenant a build holds,
/// and the window sweep passes it over. A revoked version is no build's.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_version_does_not_move_a_build_tenant_and_is_no_builds() {
    let pool = three("ordo_tb_revoked").await;
    let ops = shared(&pool);
    let b = TenantBuilds::on(&ops);
    b.set("beta", "0.2.0", "beta", OWNER, NOW).await.unwrap();
    b.join("beta", "spc_beta", OWNER, NOW).await.unwrap();
    let v = Versions::on(&pool);
    v.revoke("0.2.0", "a flaw", None, OWNER, NOW + 1)
        .await
        .expect("revoked");
    let e = v
        .move_off_revoked("spc_beta", OWNER, PinCause::Demand, NOW + 2)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("ordo tenant-builds set beta"), "{e}");
    let e = b
        .set("beta2", "0.2.0", "again", OWNER, NOW + 3)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("revoked"), "{e}");
    // Setting the build to another version is how it moves off.
    b.set("beta", "0.1.0", "off the flaw", OWNER, NOW + 4)
        .await
        .unwrap();
    assert_eq!(on(&pool, "spc_beta").await, "0.1.0");
}

/// The refusals, each naming its fix.
#[tokio::test(flavor = "multi_thread")]
async fn each_refusal_names_its_fix() {
    let pool = three("ordo_tb_refusals").await;
    let ops = shared(&pool);
    let b = TenantBuilds::on(&ops);
    let e = b
        .join("beta", "spc_b", OWNER, NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("ordo tenant-builds set beta <version>"), "{e}");
    let e = b
        .set("Beta Build", "0.2.0", "x", OWNER, NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("like `beta`"), "{e}");
    let e = b
        .set("beta", "0.9.0", "x", OWNER, NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("0.9.0"), "{e}");
    b.set("beta", "0.2.0", "x", OWNER, NOW).await.unwrap();
    b.set("canary", "0.2.0", "x", OWNER, NOW).await.unwrap();
    b.join("beta", "spc_b", OWNER, NOW).await.unwrap();
    let e = b
        .join("canary", "spc_b", OWNER, NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("ordo tenant-builds leave spc_b"), "{e}");
    let e = b.leave("spc_c", OWNER, NOW).await.unwrap_err().to_string();
    assert!(e.contains("not_in_a_build"), "{e}");
    let e = b
        .join("beta", "spc_nobody", OWNER, NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("no_tenant") || e.contains("spc_nobody"), "{e}");
    assert!(b.build_of("spc_nobody").await.unwrap().is_none());

    // A version the deployment's own database does not hold (its column
    // gone, as on a database restored from before it): a tenant on that
    // database cannot be served it, and no pin moves.
    apply(&pool, V3, false).await;
    pool.rows(r#"ALTER TABLE "task" DROP COLUMN "note""#)
        .await
        .expect("behind");
    let e = b
        .set("beta", "0.3.0", "x", OWNER, NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("deployment_behind"), "{e}");
    assert!(e.contains("--no-activate"), "{e}");
    assert_eq!(on(&pool, "spc_b").await, "0.2.0");
}

/// Past the first step of a chain, a build takes only a version that came
/// through the promotion gate (V-13).
#[tokio::test(flavor = "multi_thread")]
async fn a_build_takes_only_a_version_that_came_through_the_gate() {
    let pool = fresh("ordo_tb_gate").await;
    tenant(&pool, "spc_beta").await;
    apply(&pool, V1, true).await;
    Environments::on(&pool)
        .register("dev", None, OWNER, "where work starts", 1)
        .await
        .expect("dev");
    apply(&pool, BETA, false).await;
    let ops = shared(&pool);
    let e = TenantBuilds::on(&ops)
        .set("beta", "0.2.0", "x", OWNER, NOW)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("not_promoted"), "{e}");
    assert!(e.contains("ordo promote 0.2.0"), "{e}");
}

/// **The door**: the ops answer as the client does, bound to the
/// deployment's own space only, and write their acts to the stream.
#[tokio::test(flavor = "multi_thread")]
async fn the_door_lists_sets_joins_and_leaves() {
    let pool = three("ordo_tb_door").await;
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = ApplyOps::recording(
        pool.clone(),
        Arc::new(SinglePool::new(pool.clone())),
        "home",
        Some(stream.clone()),
    );
    let call = |op: &'static str, space: &'static str, v: serde_json::Value| {
        let ops = ops.clone();
        async move {
            tb::execute(
                &ops,
                op,
                space,
                &BoundParams(v.as_object().unwrap().clone()),
                OWNER,
            )
            .await
        }
    };
    let set = call(
        tb::SET_BUILD,
        "home",
        serde_json::json!({"name": "beta", "version": "#2", "reason": "beta"}),
    )
    .await
    .unwrap();
    assert_eq!(set.rows[0]["version"], "0.2.0");
    assert_eq!(set.rows[0]["ordinal"], 2);
    let joined = call(
        tb::JOIN_BUILD,
        "home",
        serde_json::json!({"name": "beta", "tenant": "spc_beta"}),
    )
    .await
    .unwrap();
    assert_eq!(joined.rows[0]["tenants"], serde_json::json!(["spc_beta"]));
    let list = call(tb::BUILDS, "home", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(list.rows[0]["builds"][0]["name"], "beta");
    assert_eq!(
        list.rows[0]["builds"][0]["tenants"][0]["tenant"],
        "spc_beta"
    );
    let e = call(
        tb::JOIN_BUILD,
        "home",
        serde_json::json!({"name": "nope", "tenant": "spc_b"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let e = call(tb::BUILDS, "spc_b", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let e = call(
        tb::REMOVE_BUILD,
        "home",
        serde_json::json!({"name": "beta", "reason": "done"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::ApplyRefused);
    call(
        tb::LEAVE_BUILD,
        "home",
        serde_json::json!({"tenant": "spc_beta"}),
    )
    .await
    .unwrap();
    call(
        tb::REMOVE_BUILD,
        "home",
        serde_json::json!({"name": "beta", "reason": "done"}),
    )
    .await
    .unwrap();
    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(
        acts,
        [
            "tenant-build-set",
            "tenant-build-joined",
            "tenant-build-left",
            "tenant-build-removed"
        ]
    );
}

// ── a tenant with a database of its own ─────────────────────────────────

async fn empty_db(db: &str) -> Pool {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("an empty database");
    Pool::connect(cfg(db)).await.expect("a pool")
}

fn attach_row(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.to_string(),
        credential: "tb".into(),
        custody: DbCustody::PlatformManaged,
        attached_at: 1,
        attached_by: "test".into(),
    }
}

/// **A tenant with a database of its own** has it brought to the build's
/// version before its pin moves.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_database_is_brought_to_the_build_before_its_pin_moves() {
    let control = empty_db("ordo_tb_own_control").await;
    tenants::ensure_table(&control).await.unwrap();
    ordo_exec::spacedb::ensure_table(&control).await.unwrap();
    let own = empty_db("ordo_tb_own_tenant").await;
    let v1 = Program::compile(V1).unwrap();
    ordo_exec::spacedb::apply_schema_at_version(
        &own,
        &v1.ddl,
        &v1.version,
        "test",
        "A1 — new entity `task`",
        1_000,
    )
    .await
    .unwrap();
    tenant(&control, "spc_own").await;
    ordo_exec::spacedb::attach(
        &control,
        &SpaceId("spc_own".into()),
        &attach_row("ordo_tb_own_tenant"),
    )
    .await
    .unwrap();
    tenant(&control, "spc_shared").await;
    let pools: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        cfg("unused"),
        Arc::new(Installed::new().as_configured("tb")),
    ));
    run(&control, pools.as_ref(), V1, true).await;
    // Stored beside main, on the deployment's own database only.
    apply(&control, BETA, false).await;
    let ops = ops_on(&control, pools.clone());
    let b = TenantBuilds::on(&ops);
    b.set("beta", "0.2.0", "beta", OWNER, NOW).await.unwrap();
    b.join("beta", "spc_own", OWNER, NOW).await.unwrap();
    assert_eq!(on(&control, "spc_own").await, "0.2.0");
    assert_eq!(on(&control, "spc_shared").await, "0.1.0");
    let due = own
        .rows(
            "SELECT 1 FROM information_schema.columns WHERE table_name = 'task' \
             AND column_name = 'due'",
        )
        .await
        .unwrap();
    assert_eq!(due.len(), 1, "the tenant's database took the branch");
    let recorded = ordo_exec::spacedb::recorded_version(&own).await.unwrap();
    assert!(
        matches!(&recorded, ordo_exec::spacedb::TargetVersion::At(v) if v == "0.2.0"),
        "{recorded:?}"
    );
}

/// `apply_op::run_program` with `rollout.strategy` set to `all` first. With
/// it unset, a deployment that is not dev rolls an apply out a batch at a
/// time (ruled 2026-11-25); these tests apply and expect every tenant on
/// the new version at once.
async fn run_all_at_once(
    pool: &ordo_exec::Pool,
    pools: &dyn ordo_exec::route::PoolResolver,
    program: &ordo_host::schema_apply::Program,
    notes: Vec<String>,
    actor: &str,
    op: &str,
    activate: bool,
) -> Result<ordo_host::apply_op::Report, ordo_host::apply_op::ApplyRefused> {
    ordo_host::rollouts::Rollouts::on(pool)
        .set_setting(
            "rollout.strategy",
            Some("all"),
            "test",
            "every tenant at once",
            0,
        )
        .await
        .expect("rollout.strategy all");
    ordo_host::apply_op::run_program(pool, pools, program, notes, actor, op, activate).await
}
