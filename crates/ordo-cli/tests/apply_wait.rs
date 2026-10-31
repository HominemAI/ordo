//! `ordo apply` waits until every running node serves what it activated
//! and has caught its tenants up, read from `$nodes` (2026-10-22).
//!
//! The node here is a row written the way a running door writes it
//! (`ordo_exec::nodes::beat`, which `ordo_host::node_status` calls), so the
//! test drives the exact signal the apply reads and controls its timing.
//! The e2e (`examples/notes/e2e`) covers a real door writing it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).
//! Databases are prefixed `ordo_await_`.

use std::time::{Duration, Instant};

mod support;
use ordo_conformance::Pg;
use ordo_exec::nodes::{self, Node};
use ordo_exec::{Pool, PoolConfig};
use support::operator;

const V1: &str = "entity a {\n  name: text?\n}\n\nop listA unbounded =\n  a[name]\n";
const V2: &str =
    "entity a {\n  name: text?\n  tag: text?\n}\n\nop listA unbounded =\n  a[name, tag]\n";

fn program(db: &str, tag: &str, text: &str) -> String {
    let dir = std::env::temp_dir().join(format!("ordo-await-{db}-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("p.ordo");
    std::fs::write(&path, text).expect("write program");
    path.display().to_string()
}

fn apply(path: &str, db: &str, extra: &[&str]) -> operator::Outcome {
    let mut args: Vec<String> = vec![
        path.into(),
        "--db".into(),
        db.into(),
        "--as".into(),
        "test".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    operator::run("apply", &args)
}

fn empty(db: &str) {
    Pg::named(db)
        .exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
}

async fn pool(db: &str) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool")
}

fn node(serving: &str, caught_up: &str) -> Node {
    Node {
        node: "node-a".into(),
        serving: serving.into(),
        caught_up: caught_up.into(),
        beat_secs: 5,
        ..Node::default()
    }
}

/// The version an apply activated, from its report.
fn activated(report: &str) -> String {
    report
        .lines()
        .find_map(|l| l.strip_prefix("activated  "))
        .and_then(|l| l.split_whitespace().next())
        .expect("an activated line")
        .to_string()
}

/// Apply V1 with no node running, then set up a node serving it. Returns
/// the pool and the V1 version.
async fn deployed(db: &str) -> (Pool, String) {
    empty(db);
    let v1 = program(db, "v1", V1);
    let out = tokio::task::spawn_blocking({
        let db = db.to_string();
        move || apply(&v1, &db, &[])
    })
    .await
    .unwrap();
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("no node is running"),
        "with no node there is nothing to wait for:\n{}",
        out.report
    );
    let version = activated(&out.report);
    let pool = pool(db).await;
    nodes::ensure_table(&pool).await.unwrap();
    pool.rows("DELETE FROM \"$nodes\"").await.unwrap();
    nodes::beat(&pool, &node(&version, &version)).await.unwrap();
    (pool, version)
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_returns_only_after_the_node_serves_and_caught_up() {
    let db = "ordo_await_served";
    let (pool, v1) = deployed(db).await;
    let v2 = program(db, "v2", V2);

    let started = Instant::now();
    let cli = tokio::task::spawn_blocking({
        let db = db.to_string();
        move || apply(&v2, &db, &[])
    });

    // The node swaps after a second, and its catch-up finishes a second
    // later, the order a door says them in.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let version = nodes::live(&pool).await.unwrap();
    assert_eq!(
        version[0].serving, v1,
        "the row is the node's until it swaps"
    );
    let new = ordo_exec::registry::active_version(&pool)
        .await
        .unwrap()
        .expect("v2 is active");
    assert_ne!(new, v1);
    assert!(
        !cli.is_finished(),
        "apply returned before the node served {new}"
    );
    nodes::beat(&pool, &node(&new, &v1)).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !cli.is_finished(),
        "apply returned before the catch-up finished"
    );
    nodes::beat(&pool, &node(&new, &new)).await.unwrap();

    let out = cli.await.unwrap();
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert!(
        out.report
            .contains(&format!("served     {new} on 1 node(s)")),
        "{}",
        out.report
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_wait_returns_once_the_version_is_active() {
    let db = "ordo_await_nowait";
    let (_pool, _v1) = deployed(db).await;
    let v2 = program(db, "v2", V2);
    let started = Instant::now();
    let out = tokio::task::spawn_blocking({
        let db = db.to_string();
        move || apply(&v2, &db, &["--no-wait"])
    })
    .await
    .unwrap();
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!out.report.contains("served"), "{}", out.report);
    assert!(out.report.contains("nudged"), "{}", out.report);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timeout_names_the_node_and_what_it_is_doing() {
    let db = "ordo_await_timeout";
    let (pool, v1) = deployed(db).await;
    // A held database on a node that is still retrying it.
    let mut stuck = node(&v1, &v1);
    stuck.held = vec![("db-x".into(), "unreachable: refused".into())];
    nodes::beat(&pool, &stuck).await.unwrap();
    let v2 = program(db, "v2", V2);
    let out = tokio::task::spawn_blocking({
        let db = db.to_string();
        move || apply(&v2, &db, &["--wait-secs", "1"])
    })
    .await
    .unwrap();
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(
        out.report.contains("served     not yet: waited 1s"),
        "{}",
        out.report
    );
    assert!(
        out.report.contains(&format!("node-a: still serves {v1}")),
        "{}",
        out.report
    );
    assert!(out.report.contains("--no-wait"), "{}", out.report);
    // The version is active all the same.
    assert!(out.report.contains("activated  "), "{}", out.report);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_that_stopped_writing_is_not_waited_for() {
    let db = "ordo_await_stale";
    let (pool, v1) = deployed(db).await;
    pool.rows("UPDATE \"$nodes\" SET \"seen_at\" = \"seen_at\" - 60")
        .await
        .unwrap();
    let _ = v1;
    let v2 = program(db, "v2", V2);
    let out = tokio::task::spawn_blocking({
        let db = db.to_string();
        move || apply(&v2, &db, &["--wait-secs", "5"])
    })
    .await
    .unwrap();
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("no node is running"), "{}", out.report);
}

#[test]
fn no_wait_and_wait_secs_together_are_a_usage_error() {
    let out = apply(
        "/nonexistent",
        "ordo_await_usage",
        &["--no-wait", "--wait-secs", "3"],
    );
    assert_eq!(out.code, 2, "{}", out.report);
}
