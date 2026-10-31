//! Routines, end to end (checklist 12a; board task `triggers`).
//!
//! Each case deploys a program that declares routines, assembles the
//! runtime the binary assembles, and starts its workers. Nothing is wired
//! by hand: the listeners, the clock routines and the run handler all come
//! from the program, which is the thing this increment switched on.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_crypto::KeyProvider as _;
use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::Actor;
use ordo_conformance::{Fixture, Pg};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::runtime::{self, CustodyChoice, RuntimeConfig};

const SPACE: &str = "acme";

fn pg_cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 6,
        ..PoolConfig::from_env()
    }
}

fn config(db: &str) -> RuntimeConfig {
    RuntimeConfig {
        blob_routing: ordo_host::runtime::BlobRouting::Single,
        pg: pg_cfg(db),
        addr: "127.0.0.1:0".parse().expect("addr"),
        shutdown_grace: Duration::from_secs(5),
        space: ordo_crypto::SpaceId(SPACE.into()),
        custody: CustodyChoice::SealedMaster {
            path: key_file(),
            passphrase: pass_file(),
        },
        token_secret: [7u8; 32],
        page_key: ordo_api::PageKey::from_secret(b"ordo-routines-test"),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "routines-test".into(),
        usage_flush: Duration::from_secs(1),
        usage_rollup: Duration::from_secs(3600),
        sync: Default::default(),
        auth: None,
        credentials: None,
        presence: Default::default(),
        pausing: Default::default(),
        delegable: Default::default(),
        access_template: Default::default(),
        deployment_owners: Vec::new(),
        provisioning: Default::default(),
        recovery: Default::default(),
        second_factor: Default::default(),
        mail: None,
    }
}

const PASSPHRASE: &str = "the routines suite's passphrase";

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ordo-routines-{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("tmp");
    d
}

fn key_file() -> std::path::PathBuf {
    dir().join("master.key")
}

/// Written once per test binary. Tests run in parallel, and a rewrite
/// truncates the file before it fills it, so a second test could read it
/// empty while the first was writing.
fn pass_file() -> std::path::PathBuf {
    static ONCE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let p = dir().join("pass");
        std::fs::write(&p, PASSPHRASE).expect("write");
        p
    })
    .clone()
}

fn master() -> [u8; 32] {
    use std::sync::OnceLock;
    static ONCE: OnceLock<[u8; 32]> = OnceLock::new();
    *ONCE.get_or_init(|| {
        let path = key_file();
        let _ = std::fs::remove_file(&path);
        ordo_crypto::master::seal_master(&path, "routines-suite", PASSPHRASE.as_bytes())
            .expect("sealed");
        ordo_crypto::master::open_sealed_master(&path, PASSPHRASE.as_bytes()).expect("opens")
    })
}

/// The shapes every case shares. Each case adds its own routines.
const SHAPES: &str = "domain shop @ 1.0.0

entity orders {
  placed:   datetime
  status:   text = \"new\"
  reviewed: bool = false
  archived: bool = false
  extra:    bool = false
}

entity lines {
  order -> orders owned
  qty:    int
  stale:  bool = true
}

entity counters {
  n: int
  d: int
}

op placeOrder($placed: datetime) =
  set orders { placed: $placed }

op addLine($order: id, $qty: int) =
  set lines { order: $order, qty: $qty }

op addCounter($n: int, $d: int) =
  set counters { n: $n, d: $d }
";

/// A deployment of `SHAPES` plus `routines`, applied and activated the way
/// `ordo apply` does it, under a master key this suite holds.
async fn deploy(db: &str, routines: &str) -> Pool {
    let source = format!("{SHAPES}\n{routines}");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("clean");
    let f = Fixture::from_source(&source, "").expect("the program compiles");
    pg.exec(&f.ddl).expect("ddl");
    pg.exec(ordo_crypto::SEALED_PAYLOAD_DDL).expect("payloads");

    let pool = Pool::connect(pg_cfg(db)).await.expect("pool");
    ordo_exec::keystore::ensure_tables(&pool)
        .await
        .expect("custody table");
    let provider = ordo_crypto::LocalKeyProvider::from_master(
        master(),
        Box::new(ordo_exec::keystore::PgKeyStore::open(pg_cfg(db)).expect("store")),
    );
    provider
        .create_space(&ordo_crypto::SpaceId(SPACE.into()))
        .expect("custody");

    ordo_exec::registry::ensure_tables(&pool)
        .await
        .expect("registry");
    let doc = ordo_plan::artifact::program_artifact(&f.schema, &f.registry).expect("artifact");
    let ddl = f.ddl.clone();
    let art = ordo_exec::registry::Artifact {
        version: ordo_plan::artifact::program_version(&doc, &ddl),
        artifact: ordo_plan::artifact::to_bytes(&doc),
        source,
        ddl,
        applied_at: ordo_commandlog::Timestamp(1),
        actor: "operator:routines".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:routines",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
    pool
}

/// Run one op in the tenant, as a person would through the door.
async fn call(rt: &runtime::Runtime, op: &str) -> serde_json::Value {
    let bound = rt.spaces.resolve(SPACE).await.expect("the tenant resolves");
    let f = bound.view.fixture.clone();
    let op = op.to_string();
    bound
        .view
        .exec
        .platform_write(bound.log.clone(), move |db, log| {
            f.run_write_on(db, log, &op, Some("mem_1"))
        })
        .await
        .expect("the op writes")
}

/// Every command group in the tenant's chain whose actor is the routine.
async fn routine_groups(pool: &Pool, routine: &str) -> Vec<ordo_commandlog::CommandGroup> {
    let store =
        ordo_exec::commandlog::PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    store
        .try_entries(&ordo_commandlog::SpaceId(SPACE.into()), 0, 10_000)
        .await
        .expect("the chain reads")
        .into_iter()
        .map(|e| e.group)
        .filter(|g| matches!(&g.actor, Actor::Routine(r) if r.starts_with(&format!("{routine}@"))))
        .collect()
}

async fn column(pool: &Pool, sql: &str) -> Vec<String> {
    pool.rows(sql)
        .await
        .expect("query")
        .into_iter()
        .map(|r| r.first().cloned().flatten().unwrap_or_default())
        .collect()
}

async fn wait_until<F, Fut>(limit: Duration, mut done: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if done().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    done().await
}

async fn stop(sup: Arc<ordo_host::supervisor::Supervisor>) {
    Arc::into_inner(sup)
        .expect("sole owner")
        .shutdown(Duration::from_secs(5))
        .await;
}

// ── the cases ────────────────────────────────────────────────────────────

/// A routine on `every 2s` fires on its schedule, writes through the write
/// protocol, and the chain names it as the actor.
#[tokio::test(flavor = "multi_thread")]
async fn a_scheduled_routine_runs_on_time_and_records_its_act() {
    const DB: &str = "ordo_rt_routine_clock";
    let pool = deploy(
        DB,
        "routine sweep {
  trigger every 2s
  run($now: datetime):
    set orders(status == \"new\" and placed < $now) { status: \"seen\" }
}",
    )
    .await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    assert_eq!(
        rt.triggers.clock.len(),
        1,
        "the program's schedule is wired"
    );
    assert!(
        rt.triggers.handler.is_some(),
        "the program's runner is wired"
    );
    call(&rt, "placeOrder($placed: \"2026-01-01T00:00:00Z\")").await;

    let started = std::time::Instant::now();
    let sup = rt.start_workers().await.expect("starts");
    let ran = wait_until(Duration::from_secs(20), || async {
        column(&pool, "SELECT \"status\" FROM \"orders\"").await == ["seen"]
    })
    .await;
    assert!(ran, "the routine never ran; census: {}", sup.health());
    // `every 2s` registers its first firing two seconds out. On time means
    // within a few ticks of that, not whenever.
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the routine ran late: {:?}",
        started.elapsed()
    );

    let groups = routine_groups(&pool, "sweep").await;
    assert!(!groups.is_empty(), "the chain does not name the routine");
    let g = &groups[0];
    assert!(
        g.commands.iter().all(|c| c.entity == "orders"),
        "the routine's group holds its own writes only: {g:?}"
    );
    assert_eq!(
        g.actor.to_string(),
        format!("routine:sweep@{}", rt.deployment.current().version),
        "attributed routine:<name>@<version>"
    );
    let health = sup.health();
    assert_eq!(health["triggers.clock"]["enabled"], serde_json::json!(true));
    assert_eq!(
        health["triggers.runner"]["enabled"],
        serde_json::json!(true)
    );
    stop(sup).await;
}

/// A routine woken by a change runs once for each change, and touches only
/// the changed rows: an order that existed before the routine did is left
/// as it was, though the body's condition matches it too.
#[tokio::test(flavor = "multi_thread")]
async fn an_on_change_routine_runs_once_per_change() {
    const DB: &str = "ordo_rt_routine_change";
    let pool = deploy(
        DB,
        "routine reviewNew {
  trigger when shop.orders.created
  run:
    set orders(reviewed == false) { reviewed: true }
}",
    )
    .await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    assert_eq!(rt.triggers.listeners.len(), 1);
    // Before the workers start: the cursor registers at the head, so this
    // order is history the routine does not react to.
    call(&rt, "placeOrder($placed: \"2026-01-01T00:00:00Z\")").await;
    let sup = rt.start_workers().await.expect("starts");

    call(&rt, "placeOrder($placed: \"2026-01-02T00:00:00Z\")").await;
    let reviewed = "SELECT \"placed\" FROM \"orders\" WHERE \"reviewed\" ORDER BY \"placed\"";
    let ran = wait_until(Duration::from_secs(20), || async {
        column(&pool, reviewed).await.len() == 1
    })
    .await;
    assert!(ran, "the routine never ran; census: {}", sup.health());
    // Settle: a second run for the same change would land here.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(routine_groups(&pool, "reviewNew").await.len(), 1);
    let rows = column(&pool, reviewed).await;
    assert_eq!(rows.len(), 1, "only the new order was reviewed: {rows:?}");
    assert!(rows[0].starts_with("2026-01-02"), "{rows:?}");

    call(&rt, "placeOrder($placed: \"2026-01-03T00:00:00Z\")").await;
    let again = wait_until(Duration::from_secs(20), || async {
        routine_groups(&pool, "reviewNew").await.len() == 2
    })
    .await;
    assert!(again, "the second change did not run the routine once more");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(routine_groups(&pool, "reviewNew").await.len(), 2);
    assert_eq!(column(&pool, reviewed).await.len(), 2);
    stop(sup).await;
}

/// A paused tenant's runs are skipped, finished rather than retried, and
/// each skip is a row. A routine marked `nopause` runs anyway.
#[tokio::test(flavor = "multi_thread")]
async fn a_paused_tenants_routine_is_skipped_and_the_skip_recorded() {
    const DB: &str = "ordo_rt_routine_paused";
    let pool = deploy(
        DB,
        "routine sweep {
  trigger every 1s
  run:
    set orders(status == \"new\") { status: \"seen\" }
}

routine sweepAnyway {
  trigger every 1s
  run nopause:
    set orders(extra == false) { extra: true }
}",
    )
    .await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    call(&rt, "placeOrder($placed: \"2026-01-01T00:00:00Z\")").await;
    // Paused in the registry, which every replica's register is reloaded
    // from, and held here at once, as a pause on this replica would be.
    use ordo_host::tenants;
    tenants::ensure_table(&pool).await.expect("registry");
    tenants::reserve(&pool, SPACE, "acme", "operator:routines", 1, None)
        .await
        .expect("reserve");
    tenants::activate(&pool, SPACE, 2).await.expect("activate");
    tenants::pause(
        &pool,
        SPACE,
        tenants::PAUSED_BY_DEPLOYMENT,
        "maintenance",
        3,
    )
    .await
    .expect("pause");
    rt.spaces.hold_paused(ordo_host::pause::PauseRecord {
        space: SPACE.into(),
        at: 3,
        by: tenants::PAUSED_BY_DEPLOYMENT.into(),
        note: "maintenance".into(),
    });
    let sup = rt.start_workers().await.expect("starts");

    let skipped = wait_until(Duration::from_secs(20), || async {
        ordo_host::routines::skips(&pool, SPACE)
            .await
            .map(|s| {
                s.iter()
                    .any(|k| k.routine == "sweep" && k.reason == "paused")
            })
            .unwrap_or(false)
    })
    .await;
    assert!(skipped, "no skip was recorded; census: {}", sup.health());
    let anyway = wait_until(Duration::from_secs(20), || async {
        column(&pool, "SELECT \"extra\" FROM \"orders\"").await == ["t"]
    })
    .await;
    assert!(
        anyway,
        "the nopause routine did not run in the paused tenant"
    );

    assert_eq!(
        column(&pool, "SELECT \"status\" FROM \"orders\"").await,
        ["new"],
        "the paused tenant's routine wrote"
    );
    assert!(routine_groups(&pool, "sweep").await.is_empty());
    let skips = ordo_host::routines::skips(&pool, SPACE)
        .await
        .expect("skips");
    assert!(
        skips.iter().all(|k| k.routine == "sweep"),
        "only the routine without nopause is skipped: {skips:?}"
    );
    // Finished, not retried and not dead-lettered: a pause is not a fault.
    let dead = ordo_exec::triggers::dead_letters(&pool, &ordo_commandlog::SpaceId(SPACE.into()))
        .await
        .expect("dead letters");
    assert!(dead.is_empty(), "{dead:?}");
    stop(sup).await;
}

/// A routine whose body fails at run time is retried with backoff, the
/// runner keeps running, and the door keeps answering.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_routine_backs_off_and_does_not_take_down_the_server() {
    const DB: &str = "ordo_rt_routine_failing";
    let pool = deploy(
        DB,
        "routine divide {
  trigger every 1s
  run:
    set counters(true) { n: floor(n / d) }
}",
    )
    .await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    // `d` is zero, so every run fails in the database.
    call(&rt, "addCounter($n: 10, $d: 0)").await;
    let sup = rt.start_workers().await.expect("starts");

    let retried = wait_until(Duration::from_secs(25), || async {
        column(&pool, "SELECT max(\"attempts\") FROM \"$triggerruns\"")
            .await
            .first()
            .and_then(|a| a.parse::<i64>().ok())
            .unwrap_or(0)
            >= 2
    })
    .await;
    assert!(
        retried,
        "the failing run was not retried; census: {}",
        sup.health()
    );
    // Backoff: the next attempt is scheduled in the future, not now.
    let waiting = column(
        &pool,
        "SELECT count(*) FROM \"$triggerruns\" WHERE \"attempts\" >= 1 \
         AND NOT \"claimed\" AND \"next_attempt\" > extract(epoch from now())::bigint",
    )
    .await;
    assert_ne!(waiting, ["0"], "no run is waiting out a backoff");

    // The runner is still enabled and not quarantined: a failing body is
    // the run's failure, not the worker's.
    let health = sup.health();
    assert_eq!(
        health["triggers.runner"]["enabled"],
        serde_json::json!(true)
    );
    assert_eq!(
        health["triggers.runner"]["quarantined"],
        serde_json::json!(false),
        "{health}"
    );
    // And the server still serves: an op lands while the routine fails.
    call(&rt, "addCounter($n: 1, $d: 1)").await;
    assert_eq!(
        column(&pool, "SELECT \"n\" FROM \"counters\" ORDER BY \"n\"").await,
        ["1", "10"]
    );
    stop(sup).await;
}

/// An `each` routine on a clock reads every matching row and writes each
/// one, in one command group the chain names the routine for. A row its
/// pipeline does not match is left alone.
#[tokio::test(flavor = "multi_thread")]
async fn an_each_routine_on_a_clock_writes_every_matching_row_as_one_group() {
    const DB: &str = "ordo_rt_routine_each_clock";
    let pool = deploy(
        DB,
        "routine seeAll {
  trigger every 2s
  run($now: datetime):
    orders(status == \"new\" and placed < $now)[id]
    each ->
      set orders { id: id, status: \"seen\" }
}",
    )
    .await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    for day in ["01", "02", "03"] {
        call(
            &rt,
            &format!("placeOrder($placed: \"2026-01-{day}T00:00:00Z\")"),
        )
        .await;
    }
    // In the future, so the pipeline does not match it.
    call(&rt, "placeOrder($placed: \"2099-01-01T00:00:00Z\")").await;
    let sup = rt.start_workers().await.expect("starts");
    let seen = "SELECT \"status\" FROM \"orders\" ORDER BY \"placed\"";
    let ran = wait_until(Duration::from_secs(20), || async {
        column(&pool, seen).await == ["seen", "seen", "seen", "new"]
    })
    .await;
    assert!(ran, "the routine never ran; census: {}", sup.health());
    let groups = routine_groups(&pool, "seeAll").await;
    assert_eq!(groups.len(), 1, "one group for the run: {groups:?}");
    assert_eq!(groups[0].commands.len(), 3, "{:?}", groups[0]);
    assert_eq!(
        groups[0].actor.to_string(),
        format!("routine:seeAll@{}", rt.deployment.current().version)
    );
    stop(sup).await;
}

/// An `each` routine woken by a change reads only the rows the change
/// reaches. The earlier line of the same order matches the pipeline's
/// condition too, and is two refs from the new line, so it is left alone.
#[tokio::test(flavor = "multi_thread")]
async fn an_each_routine_woken_by_a_change_reaches_only_the_changed_rows() {
    const DB: &str = "ordo_rt_routine_each_change";
    let pool = deploy(
        DB,
        "routine freshen {
  trigger when shop.lines.created
  run:
    lines(stale == true)[id, qty]
    each ->
      set lines { id: id, stale: false, qty: qty + 1 }
}",
    )
    .await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    let order = call(&rt, "placeOrder($placed: \"2026-01-01T00:00:00Z\")").await;
    let order = order[0]["created"].as_str().expect("an order").to_string();
    // History: the cursor registers at the head, so this line is not a
    // change the routine reacts to.
    call(&rt, &format!("addLine($order: \"{order}\", $qty: 10)")).await;
    let sup = rt.start_workers().await.expect("starts");

    call(&rt, &format!("addLine($order: \"{order}\", $qty: 20)")).await;
    let lines = "SELECT \"qty\" || ':' || \"stale\" FROM \"lines\" ORDER BY \"qty\"";
    let ran = wait_until(Duration::from_secs(20), || async {
        column(&pool, lines).await == ["10:true", "21:false"]
    })
    .await;
    assert!(
        ran,
        "the routine did not freshen the new line alone: {:?}; census: {}",
        column(&pool, lines).await,
        sup.health()
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(column(&pool, lines).await, ["10:true", "21:false"]);
    assert_eq!(routine_groups(&pool, "freshen").await.len(), 1);
    stop(sup).await;
}

/// Store and activate `routines` over the same shapes, as `ordo apply`
/// does for a version that changes only its routines. Returns the version.
async fn activate_next(pool: &Pool, routines: &str) -> String {
    let source = format!("{SHAPES}\n{routines}");
    let f = Fixture::from_source(&source, "").expect("the program compiles");
    let doc = ordo_plan::artifact::program_artifact(&f.schema, &f.registry).expect("artifact");
    let art = ordo_exec::registry::Artifact {
        version: ordo_plan::artifact::program_version(&doc, &f.ddl),
        artifact: ordo_plan::artifact::to_bytes(&doc),
        source,
        ddl: f.ddl.clone(),
        applied_at: ordo_commandlog::Timestamp(3),
        actor: "operator:routines".into(),
    };
    ordo_exec::registry::store(pool, &art).await.expect("store");
    ordo_exec::registry::activate(
        pool,
        &art.version,
        "operator:routines",
        ordo_commandlog::Timestamp(4),
    )
    .await
    .expect("activate");
    art.version
}

/// The deploy listener's two acts for `version`: the swap, then the
/// routines switch.
async fn switch_to(rt: &runtime::Runtime, version: &str) -> String {
    let loaded = ordo_host::deploy::load_version(
        &rt.pool,
        &rt.pg,
        version,
        rt.keys.clone(),
        rt.space.clone(),
    )
    .await
    .expect("the version loads");
    let _ = rt.deployment.swap(loaded);
    rt.routines
        .switch(&rt.pool, &rt.deployment.current().fixture.schema, version)
        .await
        .expect("switched")
}

/// A program that declares no routine wires nothing, and the trigger
/// workers run idle. A version switch that adds a routine starts it, and
/// one that drops it stops it, with no restart (notes v2 gap 26,
/// 2026-10-22).
#[tokio::test(flavor = "multi_thread")]
async fn a_routine_added_by_a_version_switch_runs_with_no_restart() {
    const DB: &str = "ordo_rt_routine_none";
    let pool = deploy(DB, "").await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    assert!(rt.triggers.handler.is_none());
    assert!(rt.triggers.listeners.is_empty() && rt.triggers.clock.is_empty());
    assert!(rt.routines.follows_program());
    call(&rt, "placeOrder($placed: \"2026-01-01T00:00:00Z\")").await;
    let sup = rt.start_workers().await.expect("starts");
    let health = sup.health();
    for w in [
        "triggers.drain",
        "triggers.batch",
        "triggers.runner",
        "triggers.clock",
    ] {
        assert_eq!(
            health[w]["enabled"],
            serde_json::json!(true),
            "{w}: {health}"
        );
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        column(&pool, "SELECT \"status\" FROM \"orders\"").await,
        ["new"]
    );

    // v2 adds a clock routine.
    let v2 = activate_next(
        &pool,
        "routine sweep {
  trigger every 1s
  run($now: datetime):
    set orders(status == \"new\" and placed < $now) { status: \"seen\" }
}",
    )
    .await;
    let line = switch_to(&rt, &v2).await;
    assert!(line.contains("1 declared"), "{line}");
    let ran = wait_until(Duration::from_secs(20), || async {
        column(&pool, "SELECT \"status\" FROM \"orders\"").await == ["seen"]
    })
    .await;
    assert!(
        ran,
        "the routine v2 added never ran; census: {}",
        sup.health()
    );

    // v3 drops it: an order placed after the switch stays as it was.
    let v3 = activate_next(&pool, "").await;
    switch_to(&rt, &v3).await;
    assert!(rt.routines.current().clock.is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;
    call(&rt, "placeOrder($placed: \"2026-01-02T00:00:00Z\")").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut statuses = column(&pool, "SELECT \"status\" FROM \"orders\"").await;
    statuses.sort();
    assert_eq!(statuses, ["new", "seen"], "the dropped routine still ran");
    stop(sup).await;
}

/// Audit 2026-10-08 finding 8. A tenant whose database could not be
/// reached at boot is first registered by the drain. It existed before the
/// version was activated, so its cursor starts at the head of its chain and
/// the routine does not run over its history. A tenant created after the
/// activation starts at 0, since its whole chain is new.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_unreachable_at_boot_does_not_replay_its_history_when_reached() {
    const DB: &str = "ordo_rt_routine_unreached";
    let pool = deploy(
        DB,
        "routine reviewNew {
  trigger when shop.orders.created
  run:
    set orders(reviewed == false) { reviewed: true }
}",
    )
    .await;
    // The tenant was created at 1 and the version activated at 2.
    use ordo_host::tenants;
    tenants::ensure_table(&pool).await.expect("registry");
    tenants::reserve(&pool, SPACE, "acme", "operator:routines", 1, None)
        .await
        .expect("reserve");
    tenants::activate(&pool, SPACE, 1).await.expect("activate");
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    // History the routine must not react to. No boot registration runs:
    // this is the tenant boot could not reach.
    call(&rt, "placeOrder($placed: \"2026-01-01T00:00:00Z\")").await;
    call(&rt, "placeOrder($placed: \"2026-01-02T00:00:00Z\")").await;

    let t = rt.triggers.tenants.clone().expect("the program's tenants");
    let reached = t
        .list_created(&rt.pool)
        .await
        .into_iter()
        .find(|x| x.id == SPACE)
        .expect("the tenant is listed");
    assert_eq!(reached.created_at, Some(1));
    let listeners = rt.triggers.listeners.clone();
    let sp = ordo_commandlog::SpaceId(SPACE.into());
    t.register_on_reach(&rt.pool, &pool, &reached, &listeners, 10)
        .await
        .expect("registers");
    let store =
        ordo_exec::commandlog::PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let head = store.try_len(&sp).await.expect("chain length") as u64;
    assert!(head >= 2, "the tenant has history: {head}");
    let at = ordo_exec::triggers::cursor(&pool, &sp, &listeners[0])
        .await
        .expect("cursor");
    assert_eq!(at, head, "the cursor starts at the head, not at 0");

    // The drain finds nothing to hand the routine.
    let sup = rt.start_workers().await.expect("starts");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        routine_groups(&pool, "reviewNew").await.is_empty(),
        "the routine ran over the tenant's history"
    );
    assert!(column(&pool, "SELECT 1 FROM \"orders\" WHERE \"reviewed\"")
        .await
        .is_empty());

    // A change after it was reached runs the routine once.
    call(&rt, "placeOrder($placed: \"2026-01-03T00:00:00Z\")").await;
    let ran = wait_until(Duration::from_secs(20), || async {
        routine_groups(&pool, "reviewNew").await.len() == 1
    })
    .await;
    assert!(
        ran,
        "the new change did not run the routine; {}",
        sup.health()
    );
    stop(sup).await;

    // The rule itself: only a tenant created after the activation starts
    // at 0.
    use ordo_host::routines::drain_starts_at_head;
    assert!(!drain_starts_at_head(Some(3), 2));
    assert!(drain_starts_at_head(Some(2), 2));
    assert!(drain_starts_at_head(Some(1), 2));
    assert!(drain_starts_at_head(None, 2));
}

const REVIEW_ON_CREATE: &str = "routine reviewNew {
  trigger when shop.orders.created
  run:
    set orders(reviewed == false) { reviewed: true }
}";

/// Audit 2026-10-08 finding 7(a). A tenant whose chain has not moved is
/// visited after 1, 2, 4 and then 8 seconds, the ceiling set here. A change
/// is drained at the next visit, and the tenant is visited every second
/// again.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_tenant_is_visited_less_often_and_a_change_restores_the_pace() {
    const DB: &str = "ordo_rt_routine_idle";
    deploy(DB, REVIEW_ON_CREATE).await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    let version = rt.deployment.current().version.clone();
    let listeners = rt.triggers.listeners.clone();
    let t = ordo_host::routines::Tenants::new(rt.spaces.clone(), SPACE.into(), version.clone())
        .with_idle_max(8);
    let tenant_pool = rt.spaces.pool_of(SPACE).await.expect("tenant pool");
    t.register(&tenant_pool, SPACE, &listeners, true, 0)
        .await
        .expect("registers");

    let mut visits = Vec::new();
    for now in 100..=123 {
        let pass = t
            .drain_pass(&rt.pool, &listeners, &version, 256, now)
            .await
            .expect("drains");
        if pass.visited.iter().any(|s| s == SPACE) {
            visits.push(now);
        }
    }
    assert_eq!(
        visits,
        [100, 101, 103, 107, 115, 123],
        "an idle tenant's visits spread out to the ceiling"
    );

    call(&rt, "placeOrder($placed: \"2026-01-01T00:00:00Z\")").await;
    let mut visits = Vec::new();
    let mut read = 0;
    for now in 124..=134 {
        let pass = t
            .drain_pass(&rt.pool, &listeners, &version, 256, now)
            .await
            .expect("drains");
        if pass.visited.iter().any(|s| s == SPACE) {
            visits.push(now);
            read += pass.units;
        }
    }
    assert!(read >= 1, "the change was drained");
    assert_eq!(
        visits,
        [131, 132, 134],
        "the change is seen at the next visit, and the pace returns to 1 s"
    );
}

/// Audit 2026-10-08 finding 7(b). A routine whose trigger changed has a
/// cursor under its earlier key. The new version's cursor starts where the
/// old one stopped, so nothing the old one had not read is lost, and the
/// old one is retired, with a record, once the new one has passed it. A
/// replica not on the new version retires nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_retires_the_previous_versions_cursor_once_caught_up() {
    const DB: &str = "ordo_rt_routine_retire";
    let pool = deploy(DB, REVIEW_ON_CREATE).await;
    let rt = runtime::assemble(config(DB)).await.expect("assembles");
    let sp = ordo_commandlog::SpaceId(SPACE.into());
    let v1 = rt.deployment.current().version.clone();
    let old_listeners = rt.triggers.listeners.clone();
    let old_key = old_listeners[0].key();

    // Version 1 registers at the head of a chain with history.
    call(&rt, "placeOrder($placed: \"2026-01-01T00:00:00Z\")").await;
    let v1_tenants = ordo_host::routines::Tenants::new(rt.spaces.clone(), SPACE.into(), v1.clone());
    v1_tenants
        .register(&pool, SPACE, &old_listeners, true, 0)
        .await
        .expect("registers");
    let old_at = ordo_exec::triggers::cursor(&pool, &sp, &old_listeners[0])
        .await
        .expect("cursor");
    // Three changes the old cursor has not read.
    for d in ["02", "03", "04"] {
        call(
            &rt,
            &format!("placeOrder($placed: \"2026-01-{d}T00:00:00Z\")"),
        )
        .await;
    }

    // Version 2 changes the routine's trigger.
    let new = ordo_exec::triggers::Listener {
        domain: "shop".into(),
        pattern: ordo_syntax::trigger::ListenerPattern::parse("shop.orders.written")
            .expect("pattern"),
        routine: "reviewNew".into(),
        version: "v2".into(),
        modifiers: Default::default(),
    };
    assert_ne!(new.key(), old_key);
    let v2 = vec![new.clone()];
    let v2_tenants =
        ordo_host::routines::Tenants::new(rt.spaces.clone(), SPACE.into(), "v2".into());

    // A replica whose live version is not v2 drains but retires nothing.
    let pass = v2_tenants
        .drain_pass(&rt.pool, &v2, &v1, 1, 10)
        .await
        .expect("drains");
    assert!(pass.retired.is_empty(), "{pass:?}");
    let new_at = ordo_exec::triggers::cursor(&pool, &sp, &new)
        .await
        .expect("cursor");
    assert_eq!(
        new_at,
        old_at + 1,
        "the new cursor started where the old one stopped, not at the head"
    );

    // The old cursor is moved ahead, as a replica still on version 1 would.
    ordo_exec::prune::advance_consumer(&pool, &sp, &old_key, old_at + 3, 11)
        .await
        .expect("advance");
    let pass = v2_tenants
        .drain_pass(&rt.pool, &v2, "v2", 1, 12)
        .await
        .expect("drains");
    assert!(
        pass.retired.is_empty(),
        "an old cursor ahead of the new one stays: {pass:?}"
    );
    let held = ordo_exec::prune::consumers(&pool, &sp)
        .await
        .expect("consumers");
    assert!(held.iter().any(|c| c.name == old_key));

    // Once the new cursor reaches it, the old one is retired.
    let pass = v2_tenants
        .drain_pass(&rt.pool, &v2, "v2", 1, 13)
        .await
        .expect("drains");
    assert_eq!(pass.retired, [(SPACE.to_string(), old_key.clone())]);
    let held = ordo_exec::prune::consumers(&pool, &sp)
        .await
        .expect("consumers");
    assert!(
        !held.iter().any(|c| c.name == old_key),
        "the old cursor no longer holds the floor"
    );
    assert!(held.iter().any(|c| c.name == new.key()));

    // The events the old cursor had not read reached the new listener.
    let ids = column(
        &pool,
        &format!(
            "SELECT \"ids\" FROM \"$triggerbatches\" WHERE \"listener\" = {}",
            ordo_exec::sql_lit(&new.key())
        ),
    )
    .await;
    assert_eq!(ids.len(), 1, "one open batch for the new listener");
    let orders = column(&pool, "SELECT \"id\" FROM \"orders\" ORDER BY \"placed\"").await;
    for o in &orders[1..] {
        assert!(
            ids[0].contains(o.as_str()),
            "{o} was not handed over: {ids:?}"
        );
    }
    assert!(!ids[0].contains(orders[0].as_str()), "history was replayed");

    // The act is recorded in the tenant's security stream.
    let store =
        ordo_exec::secevent::PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let actions: Vec<String> = store
        .try_entries(&ordo_secevent::Scope::Space(sp.clone()), 0, 1000)
        .await
        .expect("security stream")
        .into_iter()
        .filter_map(|c| match c.entry.kind {
            ordo_secevent::EventKind::CustodyAction { action } => Some(action),
            _ => None,
        })
        .collect();
    assert!(
        actions
            .iter()
            .any(|a| a.starts_with("listener_deregistered ") && a.ends_with(&old_key)),
        "{actions:?}"
    );
}
