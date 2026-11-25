//! Increment 6b of `plan-versions.md`: language editions and `ordo fix`
//! (§2.6, §2.8 step R4).
//!
//! An old edition loads while this release is inside its window
//! (`editions.load_window`), and the release check calls it out, marked
//! `soon` near the window's end. Outside the window the release check
//! names the version under `edition` with its tenants, the boot closes
//! only those tenants, and every other tenant serves. `ordo fix` rewrites
//! the source; applied, it is the same version, its row now holds the new
//! edition, and the version loads again.
//!
//! Only edition 1 is released, so the test build's flags make edition 2
//! current (`ORDO_TEST_EDITION_2_SINCE`) and date the release
//! (`ORDO_TEST_RELEASE_DATE`).
//!
//! A regression looks like an old edition refused inside its window, one
//! loaded outside it, a refusal that does not name `ordo fix`, a fixed
//! source applied as a new version, or a fixed version still refused.
//!
//! The tests take turns on `SERIAL`: they set process-wide flags.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::Stored;
use ordo_host::release_check;
use ordo_host::runtime::{self, CustodyChoice, RuntimeConfig};
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::Versions;
use ordo_plan::edition::{self, Editions, TEST_EDITION_2_SINCE, TEST_RELEASE_DATE};
use serde_json::Value;
use tower::util::ServiceExt;

const SECRET: [u8; 32] = [23u8; 32];
const SPC_A: &str = "spc_a";
const SPC_B: &str = "spc_b";

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Edition 1: no edition line, and a bare `money`.
const V010: &str = "domain shop @ 0.1.0

entity items {
  name:  text
  price: money
}

op itemList =
  items[name, price]
  sort name
  take 50
";

/// Edition 2, with one op more and the same schema.
const V020: &str = "domain shop @ 0.2.0
edition 2

entity items {
  name:  text
  price: money(0)
}

op itemList =
  items[name, price]
  sort name
  take 50

op addItem($name: text) =
  set items { name: $name, price: 0 }
";

/// Edition 2 current since 2026-01-01, so edition 1 loads in releases
/// dated up to 2027-01-01. This release is dated `release`.
fn editions_at(release: &str) {
    std::env::set_var(TEST_EDITION_2_SINCE, "2026-01-01");
    std::env::set_var(TEST_RELEASE_DATE, release);
}

fn released_editions_only() {
    std::env::remove_var(TEST_EDITION_2_SINCE);
    std::env::remove_var(TEST_RELEASE_DATE);
}

async fn apply(pool: &Pool, source: &str, activate: bool) -> ordo_host::apply_op::Report {
    let program = Program::compile(source).expect("compiles");
    ordo_host::apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        activate,
    )
    .await
    .expect("applies")
}

fn pg_cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 8,
        ..PoolConfig::from_env()
    }
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
        replica: "editions-test".into(),
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

/// 0.1.0 (edition 1) then 0.2.0 (edition 2) applied, 0.2.0 active,
/// `spc_a` pinned to 0.1.0 and `spc_b` on 0.2.0, each with its owner.
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
    apply(&pool, V010, true).await;
    apply(&pool, V020, true).await;
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

async fn item_list(node: &Node, space: &str, who: &str) -> (u16, Value) {
    let req = Request::builder()
        .method("GET")
        .uri("/op/itemList")
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
        last = item_list(node, space, who).await;
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

async fn stored_edition(pool: &Pool, version: &str) -> String {
    pool.rows(&format!(
        "SELECT \"edition\"::text FROM \"$artifacts\" WHERE \"version\" = '{version}'"
    ))
    .await
    .expect("read")[0][0]
        .clone()
        .unwrap()
}

/// **The done-when of increment 6b.** An old edition loads within its
/// window, the release check names it outside the window, and `ordo fix`
/// rewrites it to a source that is the same version and loads again.
#[tokio::test(flavor = "multi_thread")]
async fn an_old_edition_loads_within_its_window_and_ordo_fix_rewrites_it() {
    let _turn = SERIAL.lock().await;
    editions_at("2026-06-01");
    let (pool, db) = deployment("ordo_editions_window").await;
    let cfg = pg_cfg(&db);
    assert_eq!(stored_edition(&pool, "0.1.0").await, "1");
    assert_eq!(stored_edition(&pool, "0.2.0").await, "2");

    // Inside the window: it passes, with a note on the old edition.
    let report = release_check::run(&pool, &cfg).await.expect("runs");
    assert!(report.passed(), "{}", report.lines().join("\n"));
    let note = report.versions[0].notes.join("\n");
    assert!(note.contains("edition 1"), "{note}");
    assert!(note.contains("2027-01-01"), "{note}");
    assert!(note.contains("ordo fix"), "{note}");
    assert!(!note.contains("soon"), "{note}");
    assert!(report.versions[1]
        .notes
        .iter()
        .all(|n| !n.contains("edition")));
    {
        let node = boot(&db).await;
        let (status, body) = settled(&node, SPC_A, "ada", 200).await;
        assert_eq!(status, 200, "spc_a, on edition 1, serves: {body}");
        let (status, body) = settled(&node, SPC_B, "bo", 200).await;
        assert_eq!(status, 200, "spc_b serves: {body}");
        // The boot's own run, once per release, builds the same shadow
        // schema: wait for it before the next run.
        boot_row(&pool).await;
    }

    // Near the window's end: called out as soon.
    editions_at("2026-11-15");
    let report = release_check::run(&pool, &cfg).await.expect("runs");
    assert!(report.passed(), "{}", report.lines().join("\n"));
    let note = report.versions[0].notes.join("\n");
    assert!(note.starts_with("soon: "), "{note}");

    // Outside it: 0.1.0 fails under `edition`, naming `ordo fix` and spc_a.
    editions_at("2027-02-01");
    let report = release_check::run(&pool, &cfg).await.expect("runs");
    let failing = report.failing();
    assert_eq!(failing.len(), 1, "{}", report.lines().join("\n"));
    assert_eq!(failing[0].version, "0.1.0");
    assert_eq!(failing[0].failures[0].check, "edition");
    assert!(
        failing[0].failures[0].why.contains("ordo fix"),
        "{:?}",
        failing[0]
    );
    assert!(
        failing[0].failures[0].why.contains("2027-01-01"),
        "{:?}",
        failing[0]
    );
    assert_eq!(failing[0].tenants[0].space, SPC_A);
    {
        // The boot goes on: spc_a closes, spc_b serves.
        let node = boot(&db).await;
        let (status, body) = settled(&node, SPC_B, "bo", 200).await;
        assert_eq!(status, 200, "spc_b serves: {body}");
        let (status, body) = settled(&node, SPC_A, "ada", 503).await;
        assert_eq!(status, 503, "spc_a is closed: {body}");
    }

    // `ordo fix` rewrites 0.1.0's source to edition 2.
    let fixed = Editions::this_release()
        .fix(&[("shop.ordo".to_string(), V010.to_string())])
        .expect("fixes");
    assert_eq!((fixed.from, fixed.to), (1, 2));
    let source = fixed.files[0].2.clone();
    assert!(
        source.contains("domain shop @ 0.1.0\nedition 2\n"),
        "{source}"
    );
    assert!(source.contains("price: money(0)"), "{source}");
    assert!(
        fixed.diff().contains("-  price: money\n+  price: money(0)"),
        "{}",
        fixed.diff()
    );

    // Applied, it is the same version, holding the new source and edition.
    let report = apply(&pool, &source, false).await;
    assert_eq!(report.version, "0.1.0");
    assert_eq!(report.stored, Stored::EditionMoved);
    assert_eq!(stored_edition(&pool, "0.1.0").await, "2");
    let stored = Versions::on(&pool).get("0.1.0").await.unwrap().unwrap();
    assert_eq!(stored.source, source);
    assert_eq!(
        Versions::on(&pool)
            .active_version()
            .await
            .unwrap()
            .as_deref(),
        Some("0.2.0")
    );
    // Again: nothing more to move.
    assert_eq!(
        apply(&pool, &source, false).await.stored,
        Stored::AlreadyPresent
    );

    // It passes and loads under the release that refused it.
    let report = release_check::run(&pool, &cfg).await.expect("runs");
    assert!(report.passed(), "{}", report.lines().join("\n"));
    {
        let node = boot(&db).await;
        let (status, body) = settled(&node, SPC_A, "ada", 200).await;
        assert_eq!(status, 200, "spc_a serves again: {body}");
    }
    released_editions_only();
}

/// **Refusals name their fix.** An edition this release does not know, a
/// source outside its window, and an edition-2 source that breaks edition
/// 2's rule are each refused at compile, so at apply and at load.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_expired_or_rule_breaking_edition_is_refused_naming_the_fix() {
    let _turn = SERIAL.lock().await;
    released_editions_only();
    // A release build knows edition 1 alone.
    let e = Program::compile(V020).expect_err("edition 2 is not released");
    assert!(e.contains(edition::EDITION_UNKNOWN), "{e}");
    assert!(e.contains("write `edition 1`"), "{e}");
    Program::compile(V010).expect("edition 1 compiles");

    editions_at("2027-02-01");
    let e = Program::compile(V010).expect_err("outside the window");
    assert!(e.contains(edition::EDITION_EXPIRED), "{e}");
    assert!(e.contains("ordo fix"), "{e}");
    assert!(e.contains("editions.load_window"), "{e}");
    let e = Program::compile(&V020.replace("money(0)", "money")).expect_err("breaks edition 2");
    assert!(e.contains(edition::EDITION_RULE), "{e}");
    assert!(e.contains("money(0)"), "{e}");
    Program::compile(V020).expect("edition 2 compiles");
    released_editions_only();
}
