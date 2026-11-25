//! `ordo promote` and where `ordo apply` goes, against two deployments
//! running in this process: dev and staging, each a door on a loopback
//! port over its own database (`plan-versions.md` §3.1, row 4b; owner
//! rulings 2026-11-24).
//!
//! The person signs in to both with one principal. Staging reads the version
//! from dev's door itself, with the person's token for dev; nothing the CLI
//! read from its own files reaches it.
//!
//! - `ordo apply` with no `--env` goes to dev: on staging it is refused,
//!   naming dev's address; on dev it applies.
//! - `ordo promote` moves the version dev activated to staging.
//! - With no `--from-token` and no handle recorded for dev, it is refused
//!   naming the command that records one.
//! - `ordo environments door` and `set skip-staging` are recorded and shown.
//!
//! A regression looks like an apply landing on staging, or a promotion the
//! target did not read from dev.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

mod support;
use ordo_conformance::Pg;
use support::door::{runtime, serve, token, DEP_OWNER, HOME};
use support::operator;

const V1: &str = r#"
domain tasks @ 0.1.0

entity task {
  title: text
}

op openTasks unbounded =
  task[title]
"#;

fn program(tag: &str, text: &str) -> String {
    let dir = std::env::temp_dir().join(format!("ordo-promote-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("p.ordo");
    std::fs::write(&path, text).expect("write program");
    path.display().to_string()
}

fn at(addr: std::net::SocketAddr, rest: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
    args.extend([
        "--host".into(),
        format!("http://{addr}"),
        "--token".into(),
        token(DEP_OWNER),
        "--space".into(),
        HOME.into(),
    ]);
    args
}

fn ok(cmd: &str, args: Vec<String>) -> String {
    let out = operator::run(cmd, &args);
    assert_eq!(out.code, 0, "{cmd} {args:?}: {}", out.report);
    out.report
}

#[test]
fn the_person_applies_to_dev_and_promotes_to_staging() {
    let dev_pg = Pg::scoped("ordo_cli_promo_dev");
    let staging_pg = Pg::scoped("ordo_cli_promo_stg");
    let rt = runtime();
    let dev = rt.block_on(serve(&dev_pg.db));
    let staging = rt.block_on(serve(&staging_pg.db));
    let path = program("chain", V1);

    // The chain, on each deployment.
    ok(
        "environments",
        at(
            dev,
            &["register", "dev", "--position", "1", "--reason", "chain"],
        ),
    );
    ok(
        "environments",
        at(dev, &["set-own", "dev", "--reason", "this is dev"]),
    );
    for (name, pos) in [("dev", "1"), ("staging", "2")] {
        ok(
            "environments",
            at(
                staging,
                &["register", name, "--position", pos, "--reason", "chain"],
            ),
        );
    }
    ok(
        "environments",
        at(
            staging,
            &["set-own", "staging", "--reason", "this is staging"],
        ),
    );
    let dev_url = format!("http://{dev}");
    let shown = ok(
        "environments",
        at(
            staging,
            &[
                "door",
                "dev",
                &dev_url,
                HOME,
                "--reason",
                "where dev answers",
            ],
        ),
    );
    assert!(
        shown.contains(&format!("door        dev at {dev_url}")),
        "{shown}"
    );
    let listed = ok("environments", at(staging, &["list"]));
    assert!(
        listed.contains(&format!("door {dev_url}, space {HOME}")),
        "{listed}"
    );

    // `ordo apply` goes to dev: staging refuses it, naming dev's address.
    let refused = operator::run("apply", &at(staging, &[&path, "--no-wait"]));
    assert_ne!(refused.code, 0, "{}", refused.report);
    assert!(
        refused.report.contains("first step of the chain, `dev`"),
        "{}",
        refused.report
    );
    assert!(
        refused.report.contains(&format!("--host {dev_url}")),
        "{}",
        refused.report
    );
    assert!(
        refused.report.contains("ordo promote"),
        "{}",
        refused.report
    );

    ok("apply", at(dev, &[&path, "--no-wait"]));

    // No token for dev and no handle to ask the sign-in for one.
    let refused = operator::run(
        "promote",
        &at(staging, &["0.1.0", "--reason", "ready for staging"]),
    );
    assert_ne!(refused.code, 0, "{}", refused.report);
    assert!(
        refused.report.contains("ordo environments door dev"),
        "{}",
        refused.report
    );
    assert!(
        refused.report.contains("--from-token"),
        "{}",
        refused.report
    );

    // The person's own token for dev.
    let done = ok(
        "promote",
        at(
            staging,
            &[
                "0.1.0",
                "--reason",
                "ready for staging",
                "--from-token",
                &token(DEP_OWNER),
            ],
        ),
    );
    assert!(
        done.contains("promoted    0.1.0 from dev to staging"),
        "{done}"
    );

    // Promoting into dev, the first step, is refused naming apply.
    let refused = operator::run(
        "promote",
        &at(
            dev,
            &["0.1.0", "--reason", "x", "--from-token", &token(DEP_OWNER)],
        ),
    );
    assert_ne!(refused.code, 0, "{}", refused.report);
    assert!(refused.report.contains("first step"), "{}", refused.report);

    // `promotion.skip_staging`, set and shown.
    let set = ok(
        "environments",
        at(
            staging,
            &[
                "set",
                "skip-staging",
                "on",
                "--reason",
                "no staging host yet",
            ],
        ),
    );
    assert!(set.contains("promotion.skip_staging on"), "{set}");
    let listed = ok("environments", at(staging, &["list"]));
    assert!(listed.contains("promotion.skip_staging on"), "{listed}");
}

#[test]
fn a_deployment_that_registered_nothing_still_applies_with_no_env() {
    let pg = Pg::scoped("ordo_cli_promo_alone");
    let rt = runtime();
    let addr = rt.block_on(serve(&pg.db));
    let path = program("alone", V1);
    ok("apply", at(addr, &[&path, "--no-wait"]));
}
