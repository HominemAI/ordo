//! `ordo peers key|rotate-key` refuse what they cannot do, by name
//! (`plan-federation-grants.md` G9). That `ordo apply` registers the key and
//! a rotation moves it is proved against a real identity service and 2
//! deployments in `ordo-host/tests/peer_consumer.rs`, which drives the ops
//! these commands call.
//!
//! A regression looks like a command that runs with no host, or accepts a
//! flag it does not read.

mod support;
use support::operator;

#[test]
fn peers_refuses_what_it_cannot_do_and_names_the_fix() {
    std::env::remove_var("ORDO_HOST");

    let out = operator::run("peers", &[]);
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("ordo peers rotate-key"),
        "{}",
        out.report
    );

    let out = operator::run("peers", &["key".into()]);
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("--host <h>"), "{}", out.report);

    let out = operator::run("peers", &["rotate-key".into()]);
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("--host <h>"), "{}", out.report);

    let out = operator::run(
        "peers",
        &[
            "rotate-key".into(),
            "--host".into(),
            "http://127.0.0.1:1".into(),
            "--overlap-secs".into(),
            "soon".into(),
        ],
    );
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("--overlap-secs"), "{}", out.report);

    let out = operator::run("peers", &["rotate".into()]);
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("`key` and `rotate-key`"),
        "{}",
        out.report
    );
}
