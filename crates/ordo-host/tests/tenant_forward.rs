//! 25i — **a program update reaches every tenant's own database.**
//!
//! Seen 2026-09-29: the notes program changed, `serve.sh` applied it, the
//! deployment's database moved and the owner's tenant did not. The tenant was
//! held closed as schema-behind, and the person was told its database could
//! not be reached. With one tenant per person every update would have locked
//! out every person.
//!
//! What these pin, over real Postgres databases and a real `AttachedPools`:
//!
//! - an apply through the door brings every attached tenant database to the
//!   new version, and both serve at once;
//! - an apply made straight against the deployment's database (`ordo apply`)
//!   reaches no tenant, and the catch-up does;
//! - a database that was unreachable during the apply is brought forward
//!   when it returns;
//! - a change refused for one tenant keeps that tenant closed, with the
//!   refusal as its reason, and serves the others, and is not asked again;
//! - a dropped field is handled as the deployment's own database handles it:
//!   the column stays and the version moves.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::SpaceId;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver, SinglePool};
use ordo_exec::spacedb::{DbAttachment, DbCustody, TargetVersion};
use ordo_exec::witness::Witness;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::deploy::Deployment;
use ordo_host::schema_apply::Program;
use ordo_host::spaces::{SpaceRefusal, Spaces};
use ordo_host::tenant_forward::{CatchUp, Caught};

/// Every test builds several databases and pools.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const V1: &str = "entity task {\n  title: text\n  done:  bool\n}\n\n\
                  op openTasks unbounded =\n  task[title, done]\n";
/// V1 and an optional field: the additive change.
const V2: &str = "entity task {\n  title: text\n  done:  bool\n  note:  text?\n}\n\n\
                  op openTasks unbounded =\n  task[title, done]\n";
/// V1 and a unique title: refused over a database that holds duplicates.
const V2_UNIQUE: &str = "entity task {\n  title: text unique\n  done:  bool\n}\n\n\
                         op openTasks unbounded =\n  task[title, done]\n";
/// A program that still declares `extra`, and the same program without it.
const WITH_EXTRA: &str = "entity task {\n  title: text\n  done:  bool\n  extra: text?\n}\n\n\
                          op openTasks unbounded =\n  task[title, done]\n";

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 4,
        ..PoolConfig::from_env()
    }
}

fn drop_database(name: &str) {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
}

/// An empty database and a pool over it.
async fn empty_db(db: &str) -> Pool {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("an empty database");
    Pool::connect(cfg(db)).await.expect("a pool")
}

/// A tenant's database as provisioning leaves it: the program's schema and
/// a `$migrations` row naming its version.
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

struct World {
    control: Pool,
    pools: Arc<AttachedPools>,
    spaces: Arc<Spaces>,
    catchup: Arc<CatchUp>,
    keys: Arc<dyn ordo_crypto::KeyProvider>,
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

impl World {
    /// The deployment's own database at `v1`, applied and active, and a
    /// resolver that serves `serving` (which may be a later version than
    /// anything applied yet, as a node is between a swap and the catch-up).
    async fn new(stem: &str, v1: &Program, serving: &Program, serving_src: &str) -> World {
        World::capped(stem, v1, serving, serving_src, None).await
    }

    /// As `new`, with the tenant dial cap set to `cap` when given.
    async fn capped(
        stem: &str,
        v1: &Program,
        serving: &Program,
        serving_src: &str,
        cap: Option<usize>,
    ) -> World {
        let control = empty_db(&format!("ordo_tf_{stem}_control")).await;
        ordo_host::apply_op::run_program(
            &control,
            &SinglePool::new(control.clone()),
            v1,
            vec![],
            "test",
            "test.apply",
            true,
        )
        .await
        .expect("the first version applies to the deployment's own database");
        ordo_exec::spacedb::ensure_table(&control)
            .await
            .expect("attachments");
        let pools = AttachedPools::new(
            control.clone(),
            control.clone(),
            cfg("unused"),
            Arc::new(Installed::new().as_configured("tf")),
        );
        let pools = Arc::new(match cap {
            Some(n) => pools.with_dial_concurrency(n),
            None => pools,
        });
        let f = Arc::new(Fixture::from_source(serving_src, "").expect("compiles"));
        let deployment = Deployment::pinned(
            &serving.version,
            f.clone(),
            ordo_exec::Executor {
                pool: control.clone(),
                schema: f.schema.clone(),
                registry: f.registry.clone(),
                keys: f.keys.clone(),
                space: ordo_conformance::fixture_space(),
            },
        );
        let spaces = Spaces::routed(
            deployment,
            pools.clone() as Arc<dyn PoolResolver>,
            Witness::new(control.clone()),
        );
        let catchup = CatchUp::new(spaces.clone(), control.clone());
        World {
            control,
            pools,
            spaces,
            catchup,
            keys: f.keys.clone(),
        }
    }

    /// A tenant whose database is `db`, at whatever `pool` was made at.
    async fn attach(&self, space: &str, db: &str) {
        self.keys
            .create_space(&SpaceId(space.to_string()))
            .expect("custody");
        ordo_exec::spacedb::attach(&self.control, &SpaceId(space.to_string()), &attach_row(db))
            .await
            .expect("attached");
    }

    async fn serves(&self, space: &str) -> bool {
        self.spaces.resolve(space).await.is_ok()
    }
}

/// The version a database says it is at.
async fn version_of(pool: &Pool) -> String {
    match ordo_exec::spacedb::recorded_version(pool)
        .await
        .expect("a version")
    {
        TargetVersion::At(v) => v,
        other => panic!("no version recorded: {other:?}"),
    }
}

/// The deployment's own database takes a change the way `ordo migrate
/// --apply` does before an `ordo apply`.
async fn migrate(control: &Pool, program: &Program) {
    let (_, applied) = ordo_host::schema_apply::apply(control, program, "test", "migrate").await;
    applied.expect("the deployment's own database takes the change");
}

async fn column_exists(pool: &Pool, table: &str, column: &str) -> bool {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_name = '{table}' AND column_name = '{column}'"
        ))
        .await
        .expect("columns");
    rows[0][0].as_deref() == Some("1")
}

// ── an apply through the door reaches every tenant ──────────────────────

/// **Two tenants, an additive change, both serve at the new version.**
///
/// Before the change was applied the deployment served a version neither
/// database had, so both were closed as behind. The apply, through the same
/// `run_program` the door and the CLI's `--host` call, brings both forward,
/// and both resolve with no request in between having lifted anything.
#[tokio::test(flavor = "multi_thread")]
async fn an_apply_brings_every_attached_tenant_to_the_new_version() {
    let _serial = SERIAL.lock().await;
    let v1 = Program::compile(V1).expect("v1");
    let v2 = Program::compile(V2).expect("v2");
    let w = World::new("two", &v1, &v2, V2).await;
    let a = tenant_db_at("ordo_tf_two_a", &v1).await;
    let b = tenant_db_at("ordo_tf_two_b", &v1).await;
    w.attach("spc_a", "ordo_tf_two_a").await;
    w.attach("spc_b", "ordo_tf_two_b").await;

    // The deployment serves v2 and neither tenant has it.
    for s in ["spc_a", "spc_b"] {
        let Err(e) = w.spaces.resolve(s).await else {
            panic!("{s} resolved while its database was behind")
        };
        assert!(matches!(e, SpaceRefusal::SchemaBehind { .. }), "{e}");
    }
    assert_eq!(w.spaces.awaiting_schema(), ["spc_a", "spc_b"]);

    migrate(&w.control, &v2).await;
    let report = ordo_host::apply_op::run_program(
        &w.control,
        w.pools.as_ref(),
        &v2,
        vec![],
        "test",
        "test.apply",
        true,
    )
    .await
    .expect("the apply");
    assert_eq!(report.tenants.advanced, 2, "{report:?}");
    assert!(report.tenants.held.is_empty(), "{report:?}");
    assert_eq!(version_of(&a).await, v2.version);
    assert_eq!(version_of(&b).await, v2.version);
    assert!(column_exists(&a, "task", "note").await);
    assert!(column_exists(&b, "task", "note").await);

    // Served at once: the resolver asks the database, and the register lifts.
    assert!(w.serves("spc_a").await);
    assert!(w.serves("spc_b").await);
    assert!(w.spaces.quarantined().is_empty());
    assert!(w.spaces.awaiting_schema().is_empty());

    // The same apply again writes nothing to a database already there.
    let before = version_of(&a).await;
    let again = ordo_host::apply_op::run_program(
        &w.control,
        w.pools.as_ref(),
        &v2,
        vec![],
        "test",
        "test.apply",
        true,
    )
    .await
    .expect("the apply again");
    assert_eq!((again.tenants.advanced, again.tenants.current), (0, 2));
    assert_eq!(version_of(&a).await, before);
}

// ── a program applied straight to the deployment's database ─────────────

/// **`ordo apply` reaches no tenant, and the catch-up does.**
///
/// The owner's case. The apply is made against one database, as `ordo apply
/// --db` is; the node swaps to the version and holds each tenant closed. The
/// tick brings each forward, and the tenants serve.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_applied_to_the_deployments_own_database_reaches_tenants_by_the_catch_up() {
    let _serial = SERIAL.lock().await;
    let v1 = Program::compile(V1).expect("v1");
    let v2 = Program::compile(V2).expect("v2");
    let w = World::new("direct", &v1, &v2, V2).await;
    let a = tenant_db_at("ordo_tf_direct_a", &v1).await;
    let b = tenant_db_at("ordo_tf_direct_b", &v1).await;
    w.attach("spc_a", "ordo_tf_direct_a").await;
    w.attach("spc_b", "ordo_tf_direct_b").await;

    migrate(&w.control, &v2).await;
    let report = ordo_host::apply_op::run_program(
        &w.control,
        &SinglePool::new(w.control.clone()),
        &v2,
        vec![],
        "test",
        "ordo apply",
        true,
    )
    .await
    .expect("the direct apply");
    assert_eq!(
        report.tenants.advanced, 0,
        "one database is what --db means"
    );
    assert_eq!(version_of(&a).await, v1.version);

    // Each tenant is asked for, found behind, and held.
    assert!(!w.serves("spc_a").await);
    assert!(!w.serves("spc_b").await);
    assert_eq!(w.spaces.awaiting_schema().len(), 2);

    // One tick: a sweep at start, then each space held as behind.
    let moved = w.catchup.tick().await;
    assert_eq!(moved, 2, "both databases moved");
    assert_eq!(version_of(&a).await, v2.version);
    assert_eq!(version_of(&b).await, v2.version);
    assert!(w.serves("spc_a").await);
    assert!(w.serves("spc_b").await);
    assert!(w.spaces.quarantined().is_empty());
}

/// **A sweep needs no request.** A tenant nobody has asked for is brought
/// forward too, so the first person to come back after an update finds it
/// done.
#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_brings_a_tenant_nobody_has_asked_for() {
    let _serial = SERIAL.lock().await;
    let v1 = Program::compile(V1).expect("v1");
    let v2 = Program::compile(V2).expect("v2");
    let w = World::new("sweep", &v1, &v2, V2).await;
    let a = tenant_db_at("ordo_tf_sweep_a", &v1).await;
    w.attach("spc_a", "ordo_tf_sweep_a").await;
    migrate(&w.control, &v2).await;
    ordo_host::apply_op::run_program(
        &w.control,
        &SinglePool::new(w.control.clone()),
        &v2,
        vec![],
        "test",
        "ordo apply",
        true,
    )
    .await
    .expect("the direct apply");

    let swept = w.catchup.sweep().await;
    assert_eq!((swept.advanced, swept.held.len()), (1, 0), "{swept:?}");
    assert_eq!(version_of(&a).await, v2.version);
    assert!(w.serves("spc_a").await);
}

/// V2 with a backend function and its secret.
const V2_FN: &str = "domain tf @ 0.1.0\n\n\
                     entity task {\n  title: text\n  done:  bool\n  note:  text?\n}\n\n\
                     op openTasks unbounded =\n  task[title, done]\n\n\
                     secret k for api.example.com\n\n\
                     function hello in tf {\n  call($text: text(100)) -> json by member\n  \
                     code \"fn/hello.js\"\n  egress api.example.com\n  secrets k\n}\n";

/// **A program with a function reaches tenants by the catch-up.**
///
/// Seen 2026-10-31 in the notes upgrade test: the catch-up compiled the
/// serving artifact from its source alone, which has no code hash, so the
/// version it got was not the one served and no tenant was ever brought
/// forward. `ordo apply` waited 240 s on "the tenant catch-up is running".
#[tokio::test(flavor = "multi_thread")]
async fn a_program_with_a_function_reaches_tenants_by_the_catch_up() {
    let _serial = SERIAL.lock().await;
    let v1 = Program::compile(V1).expect("v1");
    let mut v2 = Program::compile(V2_FN).expect("v2");
    let source_only = v2.version.clone();
    let code = b"export default { async call() { return {}; } };\n";
    let hashes = [(
        "fn/hello.js".to_string(),
        ordo_plan::artifact::code_hash(code),
    )]
    .into_iter()
    .collect();
    v2.bind_code(&hashes).expect("the code binds");
    assert_ne!(v2.version, source_only, "the code hash is in the version");

    let w = World::new("withfn", &v1, &v2, V2_FN).await;
    let a = tenant_db_at("ordo_tf_withfn_a", &v1).await;
    w.attach("spc_a", "ordo_tf_withfn_a").await;
    migrate(&w.control, &v2).await;
    ordo_host::apply_op::run_program(
        &w.control,
        &SinglePool::new(w.control.clone()),
        &v2,
        vec![],
        "test",
        "ordo apply",
        true,
    )
    .await
    .expect("the direct apply");

    let swept = w.catchup.sweep().await;
    assert_eq!((swept.advanced, swept.held.len()), (1, 0), "{swept:?}");
    assert_eq!(version_of(&a).await, v2.version);
    assert!(w.serves("spc_a").await);
}

// ── several at once, under the dial cap ─────────────────────────────────

/// **A sweep moves several databases at once and never more than the cap.**
/// Each is brought forward once, the one that is refused is held, and the
/// others still move.
#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_moves_several_at_once_and_the_cap_holds() {
    let _serial = SERIAL.lock().await;
    let v1 = Program::compile(V1).expect("v1");
    let v2 = Program::compile(V2_UNIQUE).expect("v2");
    let w = World::capped("cap3", &v1, &v2, V2_UNIQUE, Some(3)).await;
    let mut dbs = Vec::new();
    for i in 0..7 {
        let db = format!("ordo_tf_cap3_{i}");
        dbs.push(tenant_db_at(&db, &v1).await);
        w.attach(&format!("spc_cap3_{i}"), &db).await;
    }
    dbs[2]
        .rows(
            r#"INSERT INTO "task" ("$space", "id", "rev", "title", "done") VALUES
               ('s', 't_1', 1, 'same', false), ('s', 't_2', 1, 'same', false)"#,
        )
        .await
        .expect("duplicate titles");
    migrate(&w.control, &v2).await;
    ordo_host::apply_op::run_program(
        &w.control,
        &SinglePool::new(w.control.clone()),
        &v2,
        vec![],
        "test",
        "ordo apply",
        true,
    )
    .await
    .expect("the direct apply");

    let swept = w.catchup.sweep().await;
    assert_eq!((swept.advanced, swept.held.len()), (6, 1), "{swept:?}");
    for (i, db) in dbs.iter().enumerate() {
        let want = if i == 2 { &v1.version } else { &v2.version };
        assert_eq!(&version_of(db).await, want, "database {i}");
    }
    let peak = w.catchup.peak_concurrent();
    assert!(peak >= 2, "several moved together, peak {peak}");
    assert!(peak <= 3, "the cap of 3 held, peak {peak}");
}

/// A cap of 1 walks them one by one, as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_cap_of_one_moves_them_one_at_a_time() {
    let _serial = SERIAL.lock().await;
    let v1 = Program::compile(V1).expect("v1");
    let v2 = Program::compile(V2).expect("v2");
    let w = World::capped("cap1", &v1, &v2, V2, Some(1)).await;
    for i in 0..3 {
        let db = format!("ordo_tf_cap1_{i}");
        tenant_db_at(&db, &v1).await;
        w.attach(&format!("spc_cap1_{i}"), &db).await;
    }
    migrate(&w.control, &v2).await;
    ordo_host::apply_op::run_program(
        &w.control,
        &SinglePool::new(w.control.clone()),
        &v2,
        vec![],
        "test",
        "ordo apply",
        true,
    )
    .await
    .expect("the direct apply");
    let swept = w.catchup.sweep().await;
    assert_eq!((swept.advanced, swept.held.len()), (3, 0), "{swept:?}");
    assert_eq!(w.catchup.peak_concurrent(), 1);
}

// ── a database that was down during the apply ───────────────────────────

/// **A tenant database that is unreachable during the apply is brought
/// forward when it returns.** The apply answers without it and names it, the
/// others are not held up, and the next sweep finds it.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_that_was_down_during_the_apply_is_brought_forward_when_it_returns() {
    let _serial = SERIAL.lock().await;
    let v1 = Program::compile(V1).expect("v1");
    let v2 = Program::compile(V2).expect("v2");
    let w = World::new("down", &v1, &v2, V2).await;
    let up = tenant_db_at("ordo_tf_down_up", &v1).await;
    drop_database("ordo_tf_down_gone");
    w.attach("spc_up", "ordo_tf_down_up").await;
    w.attach("spc_gone", "ordo_tf_down_gone").await;

    migrate(&w.control, &v2).await;
    let report = ordo_host::apply_op::run_program(
        &w.control,
        w.pools.as_ref(),
        &v2,
        vec![],
        "test",
        "test.apply",
        true,
    )
    .await
    .expect("one database being down does not fail the apply");
    assert_eq!(report.tenants.advanced, 1, "{report:?}");
    assert_eq!(report.tenants.held.len(), 1, "{report:?}");
    assert!(
        report.tenants.held[0].0.ends_with("ordo_tf_down_gone"),
        "{report:?}"
    );
    assert_eq!(version_of(&up).await, v2.version);
    assert!(w.serves("spc_up").await, "the others serve");
    assert!(!w.serves("spc_gone").await);

    // The database comes back, at the old version.
    let gone = tenant_db_at("ordo_tf_down_gone", &v1).await;
    // A target that failed to dial is not dialled again until its backoff
    // has run out.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    let swept = w.catchup.sweep().await;
    assert_eq!(swept.advanced, 1, "{swept:?}");
    assert_eq!(version_of(&gone).await, v2.version);
    assert!(w.serves("spc_gone").await);
    assert!(w.spaces.quarantined().is_empty());
}

// ── a change that is refused ────────────────────────────────────────────

/// **A refused change keeps that tenant closed, with the reason, and serves
/// the others.** One database holds duplicate titles a unique constraint
/// cannot be added over.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_change_keeps_that_tenant_closed_with_the_reason_and_serves_the_others() {
    let _serial = SERIAL.lock().await;
    let v1 = Program::compile(V1).expect("v1");
    let v2 = Program::compile(V2_UNIQUE).expect("v2");
    let w = World::new("refused", &v1, &v2, V2_UNIQUE).await;
    let dup = tenant_db_at("ordo_tf_refused_dup", &v1).await;
    let fine = tenant_db_at("ordo_tf_refused_fine", &v1).await;
    dup.rows(
        r#"INSERT INTO "task" ("$space", "id", "rev", "title", "done") VALUES
           ('s', 't_1', 1, 'same', false), ('s', 't_2', 1, 'same', false)"#,
    )
    .await
    .expect("duplicate titles");
    w.attach("spc_dup", "ordo_tf_refused_dup").await;
    w.attach("spc_fine", "ordo_tf_refused_fine").await;

    migrate(&w.control, &v2).await;
    let report = ordo_host::apply_op::run_program(
        &w.control,
        w.pools.as_ref(),
        &v2,
        vec![],
        "test",
        "test.apply",
        true,
    )
    .await
    .expect("the apply answers");
    assert_eq!(report.tenants.advanced, 1, "{report:?}");
    assert_eq!(report.tenants.held.len(), 1, "{report:?}");

    // The others serve.
    assert!(w.serves("spc_fine").await);
    assert_eq!(version_of(&fine).await, v2.version);

    // The refused one is closed as schema-behind, and the tick learns why.
    let Err(refusal) = w.spaces.resolve("spc_dup").await else {
        panic!("a tenant whose change was refused must not serve")
    };
    assert!(
        matches!(refusal, SpaceRefusal::SchemaBehind { .. }),
        "{refusal}"
    );
    let caught = w.catchup.forward_space("spc_dup").await;
    let Caught::Refused(why) = caught else {
        panic!("expected a refusal, got {caught:?}")
    };
    assert!(
        why.contains("probe_not_empty") || why.contains("plan_refused"),
        "{why}"
    );
    let held = w.spaces.quarantined();
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].0, "spc_dup");
    assert!(
        held[0].1.contains("schema behind") && held[0].1.contains("refused"),
        "the reason names both the comparison and the refusal: {}",
        held[0].1
    );
    assert_eq!(version_of(&dup).await, v1.version, "nothing was sent");

    // Not asked again for this version: the database is not written to.
    let rows_before = dup
        .rows(r#"SELECT count(*) FROM "$migrations""#)
        .await
        .expect("migrations");
    assert!(matches!(
        w.catchup.forward_space("spc_dup").await,
        Caught::Refused(_)
    ));
    w.catchup.tick().await;
    let rows_after = dup
        .rows(r#"SELECT count(*) FROM "$migrations""#)
        .await
        .expect("migrations");
    assert_eq!(rows_before, rows_after);
}

// ── a dropped field ─────────────────────────────────────────────────────

/// **A dropped field is handled as the deployment's own database handles
/// it: the column stays and the version moves.** This is what the notes
/// program's `gone` field did. No DDL runs, so no data is lost, and the
/// tenant serves at the new version.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_field_leaves_the_column_and_moves_the_version() {
    let _serial = SERIAL.lock().await;
    let before = Program::compile(WITH_EXTRA).expect("with extra");
    let after = Program::compile(V1).expect("without extra");
    let w = World::new("drop", &before, &after, V1).await;
    let t = tenant_db_at("ordo_tf_drop_t", &before).await;
    t.rows(
        r#"INSERT INTO "task" ("$space", "id", "rev", "title", "done", "extra")
           VALUES ('s', 't_1', 1, 'kept', false, 'still here')"#,
    )
    .await
    .expect("a row that uses the field");
    w.attach("spc_t", "ordo_tf_drop_t").await;
    assert!(
        !w.serves("spc_t").await,
        "behind until the apply reaches it"
    );

    let report = ordo_host::apply_op::run_program(
        &w.control,
        w.pools.as_ref(),
        &after,
        vec![],
        "test",
        "test.apply",
        true,
    )
    .await
    .expect("the apply");
    assert_eq!(report.tenants.advanced, 1, "{report:?}");
    assert!(report.tenants.held.is_empty(), "{report:?}");

    // The deployment's own database and the tenant's are treated alike.
    assert_eq!(version_of(&w.control).await, after.version);
    assert_eq!(version_of(&t).await, after.version);
    assert!(
        column_exists(&t, "task", "extra").await,
        "no column is dropped"
    );
    let kept = t.rows(r#"SELECT "extra" FROM "task""#).await.expect("rows");
    assert_eq!(kept[0][0].as_deref(), Some("still here"));
    let kind = t
        .rows(r#"SELECT "kind" FROM "$migrations" ORDER BY "seq" DESC LIMIT 1"#)
        .await
        .expect("kind");
    assert_eq!(kind[0][0].as_deref(), Some("version"));
    assert!(w.serves("spc_t").await);
}
