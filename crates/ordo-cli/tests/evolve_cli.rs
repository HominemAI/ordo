//! `ordo evolve` (`plan-versions.md` §2.3, increment 8) goes through the
//! door only. These are the refusals made before anything is sent: a step
//! needs both ends, `--direct` is refused naming `--host`, a flag the
//! command does not take is named, and `--tenant` needs the tenant.
//!
//! What the door does with the call is proved against Postgres in
//! `ordo-host/tests/stored_plans.rs`.

use ordo_cli::operator;

fn run(args: &[&str]) -> operator::Outcome {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    operator::run("evolve", &args)
}

const HOST: &str = "http://127.0.0.1:9";

#[test]
fn refusals_before_anything_is_sent() {
    let out = run(&["--from", "1.0.0", "--host", HOST]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--to <version>"), "{}", out.report);

    let out = run(&["--from", "1.0.0", "--to", "1.1.0", "--direct"]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--host"), "{}", out.report);

    let out = run(&[
        "--from", "1.0.0", "--to", "1.1.0", "--db", "x", "--host", HOST,
    ]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--db"), "{}", out.report);

    let out = run(&["--tenant", "--host", HOST]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--tenant <id>"), "{}", out.report);
}

/// With the step named and nothing listening, the call is made and the
/// unreachable door is named: nothing was opened locally.
#[test]
fn the_step_goes_to_the_door() {
    let out = run(&[
        "--from", "1.0.0", "--to", "1.1.0", "--host", HOST, "--token", "t", "--space", "system",
    ]);
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("127.0.0.1:9"), "{}", out.report);
}
