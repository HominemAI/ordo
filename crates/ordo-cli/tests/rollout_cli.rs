//! `ordo rollout` (`plan-versions.md` §3.7, increment 10) goes through the
//! door only. These are the refusals made before anything is sent: a
//! subcommand that needs a version or a tenant names it, an abort and a
//! hold say why, a setting is one of the five, and `--direct` is refused:
//! there is no direct path.
//!
//! What the door does with each call is proved against Postgres in
//! `ordo-host/tests/rollouts.rs`.

use ordo_cli::operator;

fn run(args: &[&str]) -> operator::Outcome {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    operator::run("rollout", &args)
}

const HOST: &str = "http://127.0.0.1:9";

#[test]
fn refusals_before_anything_is_sent() {
    let out = run(&["--host", HOST]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(
        out.report.contains("ordo rollout start <version>"),
        "{}",
        out.report
    );

    let out = run(&["start", "--host", HOST]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("name the version"), "{}", out.report);

    let out = run(&["abort", "--host", HOST]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--reason"), "{}", out.report);

    let out = run(&["hold", "spc_a", "--host", HOST]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--clear"), "{}", out.report);

    let out = run(&["set", "seats", "2", "--reason", "x", "--host", HOST]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(
        out.report.contains("rollout.batch_interval"),
        "{}",
        out.report
    );

    let out = run(&["start", "0.2.0", "--batch-size", "many", "--host", HOST]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--batch-size"), "{}", out.report);

    let out = run(&["status", "--direct"]);
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("no direct path"), "{}", out.report);

    let out = run(&["status", "--db", "x", "--host", HOST]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--db"), "{}", out.report);
}

/// With the call well formed and nothing listening, the call is made and
/// the unreachable door is named: nothing was opened locally.
#[test]
fn each_subcommand_goes_to_the_door() {
    for args in [
        vec!["start", "0.2.0", "--strategy", "rolling"],
        vec!["status"],
        vec!["skip", "spc_a"],
        vec!["retry", "spc_a"],
        vec!["abort", "--reason", "x"],
        vec!["finish"],
        vec!["hold", "spc_a", "--clear"],
        vec!["settings"],
        vec!["set", "strategy", "rolling", "--reason", "x"],
    ] {
        let mut all = args.clone();
        all.extend(["--host", HOST, "--token", "t", "--space", "system"]);
        let out = run(&all);
        assert_eq!(out.code, 1, "{args:?}: {}", out.report);
        assert!(
            out.report.contains("127.0.0.1:9"),
            "{args:?}: {}",
            out.report
        );
    }
}

/// The backfill settings are set with `ordo versions set` beside the
/// version settings (Q15). A name that is not one is refused before
/// anything is sent and lists the four; a good one goes to the door.
#[test]
fn ordo_versions_set_takes_the_backfill_settings() {
    let versions = |args: &[&str]| {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        operator::run("versions", &args)
    };
    let out = versions(&[
        "set",
        "backfill.chunks",
        "5",
        "--reason",
        "x",
        "--host",
        HOST,
    ]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(
        out.report.contains("backfill.chunk_rows") && out.report.contains("backfill.budget"),
        "{}",
        out.report
    );
    for args in [
        vec!["set", "backfill.chunk_rows", "500", "--reason", "x"],
        vec!["set", "backfill.budget", "--clear", "--reason", "x"],
    ] {
        let mut all = args.clone();
        all.extend(["--host", HOST, "--token", "t", "--space", "system"]);
        let out = versions(&all);
        assert_eq!(out.code, 1, "{args:?}: {}", out.report);
        assert!(
            out.report.contains("127.0.0.1:9"),
            "{args:?}: {}",
            out.report
        );
    }
}
