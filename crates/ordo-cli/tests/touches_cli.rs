//! `ordo touches` (owner, 2026-11-05): which passkey touches tenants may
//! choose is a deployment setting stored in Ordo, read and changed through
//! the door only. These are the refusals made before anything is sent:
//! an act on the deployment is named as never a tenant's, a direction
//! that is not one is named, a change needs a reason, and `--direct` is
//! refused naming `--host`.

use ordo_cli::operator;

fn run(args: &[&str]) -> operator::Outcome {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    operator::run("touches", &args)
}

#[test]
fn refusals_before_anything_is_sent() {
    let out = run(&[
        "expose",
        "deploying",
        "lower",
        "--reason",
        "x",
        "--host",
        "http://127.0.0.1:9",
    ]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("never choose it"), "{}", out.report);

    let out = run(&[
        "expose",
        "membership",
        "sideways",
        "--reason",
        "x",
        "--host",
        "http://127.0.0.1:9",
    ]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("raise"), "{}", out.report);

    let out = run(&[
        "expose",
        "membership",
        "both",
        "--host",
        "http://127.0.0.1:9",
    ]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--reason"), "{}", out.report);

    let out = run(&["--direct"]);
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("--host"), "{}", out.report);
}
