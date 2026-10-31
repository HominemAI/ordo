//! `ordo triggers ls` and `ordo triggers deregister`, against a real
//! Postgres (`scripts/conformance-pg.sh start`).
//!
//! Each test owns its database. The running deployment's orphan census is a
//! canned `/health/deep` answer served from a loopback socket, because the
//! census is the deployment's to give and the command only reads it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

mod support;
use ordo_commandlog::SpaceId;
use ordo_conformance::Pg;
use ordo_exec::prune::{self, ConsumerKind};
use ordo_exec::secevent::PgSecurityStore;
use ordo_exec::{Pool, PoolConfig};
use ordo_secevent::{EventKind, Scope};
use support::operator;

const SPACE: &str = "spc_triggers_dereg";
const LIVE: &str = "digest:*.tasks.created";
const DEAD: &str = "oldname:*.tasks.created";

fn space() -> SpaceId {
    SpaceId(SPACE.into())
}

/// A database of this test's own, with one dead and one live listener
/// cursor and a mirror, and an empty security stream for the space.
fn seed(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let pool = Pool::connect(PoolConfig {
            db: db.to_string(),
            size: 4,
            acquire_timeout: Duration::from_secs(5),
            ..PoolConfig::from_env()
        })
        .await
        .expect("boot");
        prune::ensure_tables(&pool).await.unwrap();
        pool.rows("DELETE FROM \"$logconsumers\"").await.unwrap();
        pool.rows("DELETE FROM \"$secevents\"").await.unwrap();
        let s = space();
        prune::register_consumer(&pool, &s, DEAD, ConsumerKind::Listener, 7, 100)
            .await
            .unwrap();
        prune::register_consumer(&pool, &s, LIVE, ConsumerKind::Listener, 12, 100)
            .await
            .unwrap();
        prune::register_consumer(&pool, &s, "mirror:phone", ConsumerKind::Mirror, 3, 100)
            .await
            .unwrap();
        pool
    })
}

fn names(pool: &Pool) -> Vec<String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        prune::consumers(pool, &space())
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect()
    })
}

/// The `CustodyAction` records in the space's security stream, as
/// `(subject, op, action)`.
fn records(pool: &Pool) -> Vec<(String, String, String)> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let store = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
        store
            .try_entries(&Scope::Space(space()), 0, 100)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|c| match c.entry.kind {
                EventKind::CustodyAction { action } => {
                    Some((c.entry.subject.render(), c.entry.op, action))
                }
                _ => None,
            })
            .collect()
    })
}

/// A one-shot deployment: answers one `GET /health/deep` with a census
/// naming `orphans`, then stops. Returns its `--host` value.
fn deployment(orphans: &[&str]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let body = serde_json::json!({
        "space": SPACE,
        "supervisor": {
            "orphaned_listeners": orphans.iter()
                .map(|o| serde_json::json!({ "consumer": o, "position": 0 }))
                .collect::<Vec<_>>()
        }
    })
    .to_string();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    format!("http://{addr}")
}

fn args(db: &str, rest: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = ["--space", SPACE, "--db", db]
        .iter()
        .map(|s| s.to_string())
        .collect();
    v.extend(rest.iter().map(|s| s.to_string()));
    v
}

fn run(sub: &str, a: Vec<String>) -> operator::Outcome {
    let mut all = vec![sub.to_string()];
    all.extend(a);
    operator::run("triggers", &all)
}

/// The command's whole point: an orphan is retired, its cursor is gone, the
/// others are untouched, and the record says who, what and where it stood.
#[test]
fn an_orphaned_listener_is_deregistered_and_the_act_is_recorded() {
    let db = "ordo_triggers_dereg_orphan";
    let pool = seed(db);
    let host = deployment(&[DEAD]);
    let out = run(
        "deregister",
        args(
            db,
            &[
                "--listener",
                DEAD,
                "--as",
                "usr_adam",
                "--host",
                &host,
                "--token",
                "t",
            ],
        ),
    );
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("position 7"), "{}", out.report);
    assert_eq!(names(&pool), vec![LIVE.to_string(), "mirror:phone".into()]);
    let rec = records(&pool);
    assert_eq!(rec.len(), 1, "{rec:?}");
    assert_eq!(rec[0].0, "user:usr_adam");
    assert_eq!(rec[0].1, "ordo.triggers.deregister");
    assert_eq!(
        rec[0].2,
        format!("listener_deregistered position=7 forced=false listener={DEAD}")
    );
}

/// An unknown listener is an error and changes nothing, not a silent no-op.
#[test]
fn an_unknown_listener_is_an_error_and_nothing_is_recorded() {
    let db = "ordo_triggers_dereg_unknown";
    let pool = seed(db);
    let host = deployment(&[DEAD]);
    let out = run(
        "deregister",
        args(
            db,
            &["--listener", "typo:*.*.*", "--host", &host, "--token", "t"],
        ),
    );
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(
        out.report.contains("no consumer is registered"),
        "{}",
        out.report
    );
    assert_eq!(names(&pool).len(), 3);
    assert!(records(&pool).is_empty());
}

/// A listener the running deployment still serves is refused without
/// `--force`, with the reason, and goes with `--force`, recorded as forced.
#[test]
fn a_live_listener_is_refused_without_force_and_goes_with_it() {
    let db = "ordo_triggers_dereg_live";
    let pool = seed(db);

    let host = deployment(&[DEAD]);
    let out = run(
        "deregister",
        args(db, &["--listener", LIVE, "--host", &host, "--token", "t"]),
    );
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("still serves"), "{}", out.report);
    assert!(out.report.contains("at-least-once"), "{}", out.report);
    assert!(names(&pool).contains(&LIVE.to_string()));
    assert!(records(&pool).is_empty());

    let host = deployment(&[DEAD]);
    let out = run(
        "deregister",
        args(
            db,
            &[
                "--listener",
                LIVE,
                "--force",
                "--as",
                "usr_adam",
                "--host",
                &host,
                "--token",
                "t",
            ],
        ),
    );
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(!names(&pool).contains(&LIVE.to_string()));
    let rec = records(&pool);
    assert_eq!(rec.len(), 1, "{rec:?}");
    assert!(rec[0].2.contains("forced=true"), "{rec:?}");
    assert!(rec[0].2.contains("position=12"), "{rec:?}");
}

/// With no deployment asked, standing is unknown and that is refused like a
/// live listener. A non-listener cursor is refused outright.
#[test]
fn without_a_census_it_refuses_and_a_mirror_is_not_a_listener() {
    let db = "ordo_triggers_dereg_nocensus";
    let pool = seed(db);
    // `ORDO_HOST` would make this ask a deployment; the test means none.
    std::env::remove_var("ORDO_HOST");
    let out = run("deregister", args(db, &["--listener", DEAD]));
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("unknown"), "{}", out.report);
    let out = run("deregister", args(db, &["--listener", "mirror:phone"]));
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(
        out.report.contains("not a trigger listener"),
        "{}",
        out.report
    );
    assert_eq!(names(&pool).len(), 3);
    assert!(records(&pool).is_empty());
}

/// `ls` shows each cursor with kind, position, idle time and standing.
#[test]
fn ls_lists_each_cursor_with_its_standing() {
    let db = "ordo_triggers_dereg_ls";
    let _pool = seed(db);
    let host = deployment(&[DEAD]);
    let out = run("ls", args(db, &["--host", &host, "--token", "t"]));
    assert_eq!(out.code, 0, "{}", out.report);
    let line = |name: &str| {
        out.report
            .lines()
            .find(|l| l.starts_with(name))
            .unwrap_or_else(|| panic!("no line for {name}: {}", out.report))
            .to_string()
    };
    assert!(line(DEAD).contains("listener  7"), "{}", out.report);
    assert!(line(DEAD).ends_with("orphaned"), "{}", out.report);
    assert!(line(LIVE).ends_with("live"), "{}", out.report);
    assert!(line("mirror:phone").ends_with('-'), "{}", out.report);
}

/// The action name is what alerting keys off, so it does not drift.
#[test]
fn the_recorded_action_name_is_stable() {
    assert_eq!(ordo_cli::triggers::ACTION, "listener_deregistered");
    assert_eq!(ordo_cli::triggers::OP, "ordo.triggers.deregister");
}
