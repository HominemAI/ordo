//! `ordo release-check` (`plan-versions.md` §2.8, ruled 2026-11-23, Q8):
//! the mode of the new image an upgrade runs before its tag changes.
//!
//! - Without `--direct` it is refused, naming `--direct`; with `--host` it
//!   is refused too, since the door of the old release cannot compile
//!   under the new one.
//! - With `--direct` it records the use in `$direct_uses` before it runs,
//!   writes one `$release_checks` row, and exits 0 when every version
//!   passes.
//! - A version this release does not compile (the test build's flag) is
//!   named with its tenant, and the exit is 1. `--override` without a
//!   reason is refused naming `--reason`; with one it is recorded and
//!   exits 0.
//!
//! One test: the flag is the process's environment. Needs the conformance
//! cluster (`scripts/conformance-pg.sh start`).

use ordo_cli::operator;
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::Program;

fn run(args: &[&str]) -> operator::Outcome {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    operator::run("release-check", &args)
}

const V1: &str = "domain board @ 0.1.0

entity cards {
  title: text
}

op cardList =
  cards[title]
  sort title
  take 50
";

async fn rows(pool: &Pool, sql: &str) -> Vec<Vec<Option<String>>> {
    pool.rows(sql).await.expect("reads")
}

#[test]
fn release_check_runs_direct_names_a_failing_version_and_records_each_run() {
    std::env::remove_var(ordo_host::release_check::TEST_UNCOMPILABLE);

    // Refusals before anything is opened.
    let out = run(&[]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--direct"), "{}", out.report);
    let out = run(&["--host", "http://127.0.0.1:9"]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--direct"), "{}", out.report);

    let pg = Pg::scoped("ordo_cli_relcheck");
    let db = pg.db.clone();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pool = rt.block_on(async {
        let pool = Pool::connect(PoolConfig {
            db: db.clone(),
            size: 2,
            ..PoolConfig::from_env()
        })
        .await
        .expect("pool");
        ordo_host::tenants::ensure_table(&pool)
            .await
            .expect("$spaces");
        assert!(
            ordo_host::tenants::reserve(&pool, "spc_a", "Acme", "dev:ada", 10, None)
                .await
                .expect("reserved")
        );
        ordo_host::tenants::activate(&pool, "spc_a", 11)
            .await
            .expect("active");
        let program = Program::compile(V1).expect("compiles");
        ordo_host::apply_op::run_program(
            &pool,
            &SinglePool::new(pool.clone()),
            &program,
            vec![],
            "dev:ada",
            "t",
            true,
        )
        .await
        .expect("applies");
        pool
    });

    let out = run(&["--direct", "--db", &db, "--reason", "why"]);
    assert_eq!(out.code, 2, "--reason alone: {}", out.report);
    assert!(out.report.contains("--override"), "{}", out.report);

    // Every version passes.
    let out = run(&["--direct", "--db", &db]);
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("ok    0.1.0"), "{}", out.report);
    assert!(out.report.contains("passed"), "{}", out.report);

    // A release that no longer compiles 0.1.0.
    std::env::set_var(ordo_host::release_check::TEST_UNCOMPILABLE, "0.1.0");
    let out = run(&["--direct", "--db", &db]);
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("FAILS 0.1.0"), "{}", out.report);
    assert!(out.report.contains("spc_a (Acme)"), "{}", out.report);
    let out = run(&["--direct", "--db", &db, "--json"]);
    assert_eq!(out.code, 1, "{}", out.report);
    let json: serde_json::Value = serde_json::from_str(out.report.trim()).expect("json");
    assert_eq!(json["result"], "fail");
    assert_eq!(json["versions"][0]["tenants"][0]["space"], "spc_a");

    let out = run(&["--direct", "--db", &db, "--override"]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--reason"), "{}", out.report);
    let out = run(&[
        "--direct",
        "--db",
        &db,
        "--override",
        "--reason",
        "security fix",
    ]);
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("override recorded"), "{}", out.report);
    std::env::remove_var(ordo_host::release_check::TEST_UNCOMPILABLE);

    // Each run is on record: the direct uses for the security log, and the
    // checks with their results.
    rt.block_on(async {
        let uses = rows(
            &pool,
            "SELECT \"command\" FROM \"$direct_uses\" ORDER BY \"id\"",
        )
        .await;
        assert!(uses.len() >= 4, "{uses:?}");
        assert!(uses
            .iter()
            .all(|r| r[0].as_deref() == Some("ordo_release-check")));
        let checks = rows(
            &pool,
            "SELECT \"act\", \"result\", \"reason\" FROM \"$release_checks\" ORDER BY \"seq\"",
        )
        .await;
        let got: Vec<(String, String, String)> = checks
            .into_iter()
            .map(|r| {
                (
                    r[0].clone().unwrap(),
                    r[1].clone().unwrap(),
                    r[2].clone().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("check".into(), "pass".into(), "".into()),
                ("check".into(), "fail".into(), "".into()),
                ("check".into(), "fail".into(), "".into()),
                ("override".into(), "fail".into(), "security fix".into()),
            ]
        );
    });
}
