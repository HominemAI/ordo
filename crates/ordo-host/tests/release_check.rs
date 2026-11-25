//! Increment 6 of `plan-versions.md`: the release check (§2.6, §2.8).
//!
//! A stored version that this release no longer compiles is named with
//! its tenants, by `ordo release-check` before the upgrade and by the boot
//! after it. The boot never stops: that version's tenants are closed, and
//! every other tenant serves (owner, 2026-11-06). The release is made to
//! refuse a version with the test build's flag `ORDO_TEST_UNCOMPILABLE`,
//! as a release that changed the language under it would.
//!
//! A regression looks like a boot that refuses, a tenant on a passing
//! version closed, a failing version not named, or its tenants missing
//! from the report.
//!
//! One test per scenario, each setting the flag for itself. nextest runs
//! each test in its own process; under `cargo test` they share one, and
//! take turns on `SERIAL`.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeMap;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::release_check::{self, TEST_UNCOMPILABLE};
use ordo_host::runtime::{self, CustodyChoice, RuntimeConfig};
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::Versions;
use serde_json::Value;
use tower::util::ServiceExt;

const SECRET: [u8; 32] = [21u8; 32];
const SPC_A: &str = "spc_a";
const SPC_B: &str = "spc_b";

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const V010: &str = "domain board @ 0.1.0

entity cards {
  title: text
}

op addCard($title: text) =
  set cards { title: $title }

op cardList =
  cards[title]
  sort title
  take 50

function stamp in board {
  call($text: text(200)) -> json by member
  code \"fn/stamp.js\"
}
";

const V020: &str = "domain board @ 0.2.0

entity cards {
  title: text
  due:   int?
}

op addCard($title: text) =
  set cards { title: $title }

op cardList =
  cards[title]
  sort title
  take 50

function stamp in board {
  call($text: text(200)) -> json by member
  code \"fn/stamp.js\"
}
";

fn code(said: &str) -> String {
    format!("export default {{ async call(input, ctx) {{ return {{ from: \"{said}\" }}; }} }}")
}

fn pg_cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 8,
        ..PoolConfig::from_env()
    }
}

async fn apply(pool: &Pool, source: &str, said: &str) {
    let mut program = Program::compile(source).expect("compiles");
    let h = ordo_host::functions::store_code(pool, code(said).as_bytes())
        .await
        .expect("code stored");
    program
        .bind_code(&BTreeMap::from([("fn/stamp.js".to_string(), h)]))
        .expect("code bound");
    ordo_host::apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        true,
    )
    .await
    .expect("applies");
}

fn free_addr() -> std::net::SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn config(db: &str) -> RuntimeConfig {
    RuntimeConfig {
        blob_routing: ordo_host::runtime::BlobRouting::Single,
        pg: pg_cfg(db),
        addr: free_addr(),
        shutdown_grace: Duration::from_secs(1),
        space: ordo_crypto::SpaceId("system".into()),
        tightened: Vec::new(),
        custody: CustodyChoice::Test,
        token_secret: SECRET,
        page_key: ordo_api::PageKey::from_secret(&[22u8; 32]),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "release-check-test".into(),
        usage_flush: Duration::from_secs(60),
        usage_rollup: Duration::from_secs(3600),
        sync: Default::default(),
        auth: None,
        credentials: None,
        presence: Default::default(),
        pausing: Default::default(),
        version_moves: Default::default(),
        delegable: Default::default(),
        access_template: Default::default(),
        deployment_owners: Vec::new(),
        provisioning: Default::default(),
        recovery: Default::default(),
        second_factor: Default::default(),
        mail: None,
    }
}

fn mint(principal: &str) -> String {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&SECRET),
        &ordo_identity::TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(u64::MAX / 2),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// 0.1.0 then 0.2.0 applied, 0.2.0 active, `spc_a` pinned to 0.1.0 and
/// `spc_b` on 0.2.0, each with its owner. Nothing serves yet: this is the
/// deployment the old release left behind.
async fn deployment(stem: &str) -> (Pool, String) {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(pg_cfg(&pg.db)).await.expect("pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    for id in [SPC_A, SPC_B] {
        assert!(tenants::reserve(&pool, id, id, "dev:ada", 10, None)
            .await
            .expect("reserved"));
        tenants::activate(&pool, id, 11).await.expect("active");
    }
    apply(&pool, V010, "0.1.0").await;
    apply(&pool, V020, "0.2.0").await;
    let keys = Fixture::from_source("", "").expect("empty").keys.clone();
    for (space, who) in [(SPC_A, "ada"), (SPC_B, "bo")] {
        ordo_host::provision::provision_space(
            &pool,
            keys.as_ref(),
            space,
            &ordo_host::provision::FoundingOwner {
                member_id: who,
                principal: who,
                display_name: who,
            },
        )
        .await
        .expect("provisions");
    }
    Versions::on(&pool)
        .pin(SPC_A, "0.1.0", "deployment-owner:ada", 50)
        .await
        .expect("pinned");
    (pool, pg.db.clone())
}

struct Node {
    app: axum::Router,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Node {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

/// This release boots on the deployment. `assemble` refusing is the
/// failure this file guards against.
async fn boot(db: &str) -> Node {
    let rt = runtime::assemble(config(db))
        .await
        .unwrap_or_else(|e| panic!("the boot stopped: {e}"));
    for space in [SPC_A, SPC_B] {
        let _ = rt.keys.create_space(&ordo_crypto::SpaceId(space.into()));
    }
    let app = runtime::door(&rt);
    let serving = tokio::spawn(async move {
        let _ = runtime::serve(rt).await;
    });
    Node { app, serving }
}

async fn card_list(node: &Node, space: &str, who: &str) -> (u16, Value) {
    let req = Request::builder()
        .method("GET")
        .uri("/op/cardList")
        .header("x-ordo-space", space)
        .header("authorization", format!("Bearer {}", mint(who)))
        .body(Body::empty())
        .unwrap();
    let resp = node.app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The answer a tenant gets once the node has read its pin and loaded what
/// it can: polled, since both happen just after the boot.
async fn settled(node: &Node, space: &str, who: &str, want: u16) -> (u16, Value) {
    let mut last = (0, Value::Null);
    for _ in 0..100 {
        last = card_list(node, space, who).await;
        if last.0 == want {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    last
}

/// The boot's `$release_checks` row, once its background run has written
/// it.
async fn boot_row(pool: &Pool) -> release_check::Recorded {
    for _ in 0..300 {
        let h = release_check::history(pool, release_check::release())
            .await
            .expect("history");
        if let Some(r) = h.into_iter().find(|r| r.act == "boot") {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the boot recorded no release check");
}

fn failing_versions(report: &Value) -> Vec<String> {
    report["versions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["passed"] == false)
        .map(|v| v["version"].as_str().unwrap().to_string())
        .collect()
}

/// **The done-when of increment 6.** Before the upgrade the check passes.
/// Under a release that no longer compiles 0.1.0, it names 0.1.0 and
/// `spc_a`, and the override is refused without a reason. The boot goes
/// on: `spc_a` is closed, `spc_b` serves, and the boot's own run is
/// recorded once.
#[tokio::test(flavor = "multi_thread")]
async fn an_uncompilable_pinned_version_is_named_with_its_tenants_and_every_other_tenant_serves() {
    let _turn = SERIAL.lock().await;
    std::env::remove_var(TEST_UNCOMPILABLE);
    let (pool, db) = deployment("ordo_relcheck_pinned").await;
    let cfg = pg_cfg(&db);

    // This release, as the old one left it: every version passes.
    let report = release_check::run(&pool, &cfg).await.expect("runs");
    assert!(report.passed(), "{}", report.lines().join("\n"));
    let names: Vec<&str> = report.versions.iter().map(|c| c.version.as_str()).collect();
    assert_eq!(names, vec!["0.1.0", "0.2.0"]);
    let v010 = &report.versions[0];
    assert_eq!(v010.reasons, vec!["pinned".to_string()]);
    assert_eq!(v010.tenants.len(), 1);
    assert_eq!(v010.tenants[0].space, SPC_A);
    assert!(report.versions[1].reasons.contains(&"active".to_string()));
    assert_eq!(report.versions[1].tenants[0].space, SPC_B);

    // A release that no longer compiles 0.1.0.
    std::env::set_var(TEST_UNCOMPILABLE, "0.1.0");
    let report = release_check::run(&pool, &cfg).await.expect("runs");
    assert!(!report.passed());
    let failing = report.failing();
    assert_eq!(failing.len(), 1);
    assert_eq!(failing[0].version, "0.1.0");
    assert_eq!(failing[0].failures[0].check, "compile");
    assert_eq!(
        report.closing(),
        vec![("0.1.0".to_string(), failing[0].tenants.clone())]
    );
    let text = report.lines().join("\n");
    assert!(text.contains("FAILS 0.1.0"), "{text}");
    assert!(text.contains(SPC_A), "{text}");
    assert!(text.contains("ordo versions pin"), "{text}");
    assert!(text.contains("--override --reason"), "{text}");

    // The check wrote nothing: no pin moved and no table was made.
    let (pins, _) = Versions::on(&pool).all_pins().await.expect("pins");
    assert!(pins.iter().any(|(s, v, _)| s == SPC_A && v == "0.1.0"));
    assert!(release_check::history(&pool, release_check::release())
        .await
        .expect("history")
        .is_empty());

    // The override names its reason, or is refused naming the flag.
    let refused = release_check::record_override(&pool, &report, "cli:adam", " ", 60)
        .await
        .expect_err("a reason is required");
    assert!(refused.contains("--reason"), "{refused}");
    release_check::record_override(&pool, &report, "cli:adam", "security fix", 60)
        .await
        .expect("recorded");

    // The new release boots. It does not stop.
    let node = boot(&db).await;
    let (status, body) = settled(&node, SPC_B, "bo", 200).await;
    assert_eq!(status, 200, "spc_b serves: {body}");
    let (status, body) = settled(&node, SPC_A, "ada", 503).await;
    assert_eq!(status, 503, "spc_a is closed: {body}");

    let row = boot_row(&pool).await;
    assert_eq!(row.result, "fail");
    assert_eq!(failing_versions(&row.report), vec!["0.1.0".to_string()]);
    assert_eq!(
        row.report["versions"][0]["tenants"][0]["space"], SPC_A,
        "{}",
        row.report
    );
    // Once per release: a second boot's run finds it and checks nothing.
    assert!(release_check::at_boot(&pool, &cfg, 70).await.is_empty());
    let acts: Vec<String> = release_check::history(&pool, release_check::release())
        .await
        .expect("history")
        .into_iter()
        .map(|r| r.act)
        .collect();
    assert_eq!(acts, vec!["override".to_string(), "boot".to_string()]);
    std::env::remove_var(TEST_UNCOMPILABLE);
}

/// **The active version fails under the new release.** The boot used to
/// stop here. Now it goes on with no program serving: `spc_b`, on the
/// active version, is closed, `spc_a`, pinned to 0.1.0, serves, the node
/// answers its health check, and the boot's run names 0.2.0 with `spc_b`.
#[tokio::test(flavor = "multi_thread")]
async fn an_uncompilable_active_version_closes_its_tenants_and_the_boot_goes_on() {
    let _turn = SERIAL.lock().await;
    let (pool, db) = deployment("ordo_relcheck_active").await;
    std::env::set_var(TEST_UNCOMPILABLE, "0.2.0");
    let node = boot(&db).await;
    // `spc_a`, on 0.1.0, is served it.
    let (status, body) = settled(&node, SPC_A, "ada", 200).await;
    assert_eq!(status, 200, "spc_a serves: {body}");
    // `spc_b` is closed, and told so rather than denied.
    let (status, body) = settled(&node, SPC_B, "bo", 503).await;
    assert_eq!(status, 503, "spc_b is closed: {body}");
    assert_ne!(body["error"]["code"], "access_denied", "{body}");

    let req = Request::builder()
        .uri("/health/live")
        .body(Body::empty())
        .unwrap();
    let live = node.app.clone().oneshot(req).await.unwrap();
    assert_eq!(live.status().as_u16(), 200);

    let row = boot_row(&pool).await;
    assert_eq!(row.result, "fail");
    assert_eq!(failing_versions(&row.report), vec!["0.2.0".to_string()]);
    let tenants = row.report["versions"][1]["tenants"].to_string();
    assert!(tenants.contains(SPC_B), "{}", row.report);
    std::env::remove_var(TEST_UNCOMPILABLE);
}

/// **A stored plan that renders to another digest** under this release
/// fails its target version (§2.8 check 6), naming `ordo evolve`.
#[tokio::test(flavor = "multi_thread")]
async fn a_stored_plan_whose_digest_moved_fails_its_target_version() {
    let _turn = SERIAL.lock().await;
    std::env::remove_var(TEST_UNCOMPILABLE);
    let (pool, db) = deployment("ordo_relcheck_plan").await;
    let cfg = pg_cfg(&db);
    ordo_host::evolve::evolve(
        &pool,
        "0.1.0",
        "0.2.0",
        &Default::default(),
        None,
        "dev:ada",
        40,
    )
    .await
    .expect("a plan is stored");
    let report = release_check::run(&pool, &cfg).await.expect("runs");
    assert!(report.passed(), "{}", report.lines().join("\n"));

    pool.rows("UPDATE \"$plans\" SET \"digest\" = 'sha256:00'")
        .await
        .expect("changed");
    let report = release_check::run(&pool, &cfg).await.expect("runs");
    let failing = report.failing();
    assert_eq!(failing.len(), 1, "{}", report.lines().join("\n"));
    assert_eq!(failing[0].version, "0.2.0");
    assert_eq!(failing[0].failures[0].check, "plan");
    assert!(
        failing[0].failures[0]
            .why
            .contains("ordo evolve --from 0.1.0 --to 0.2.0"),
        "{}",
        failing[0].failures[0].why
    );
}
