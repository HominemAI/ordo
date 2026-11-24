//! `ordo clones` against a deployment running in this process, a door on a
//! loopback port over its own database (`plan-versions.md` §3.4, row 12).
//! The whole clone, production to staging, is `ordo-host/tests/clones.rs`;
//! this is the command's own half:
//!
//! - `key` prints this deployment's clone key, the same each time, with the
//!   command that records it on production;
//! - `set mask` and `list` show `clone.mask`;
//! - each refusal names its fix: an environment not registered, no door for
//!   production.
//!
//! A regression looks like a command that reaches the database directly,
//! or a refusal that does not name the command that fixes it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

mod support;
use ordo_conformance::Pg;
use support::door::{runtime, serve, token, DEP_OWNER, HOME};
use support::operator;

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

fn refused(cmd: &str, args: Vec<String>) -> String {
    let out = operator::run(cmd, &args);
    assert_ne!(out.code, 0, "{cmd} {args:?}: {}", out.report);
    out.report
}

#[test]
fn the_clone_commands_go_through_the_door_and_name_each_fix() {
    let pg = Pg::scoped("ordo_cli_clones");
    let rt = runtime();
    let addr = rt.block_on(serve(&pg.db));

    // The clone key, made once and printed with the command that records it.
    let first = ok("clones", at(addr, &["key"]));
    assert!(first.contains("ordo clones set-key production"), "{first}");
    let key = first
        .lines()
        .find_map(|l| l.strip_prefix("clone key   "))
        .expect("a key line")
        .trim()
        .to_string();
    assert_eq!(key.len(), 64, "{first}");
    assert_eq!(ok("clones", at(addr, &["key"])), first, "the same key");

    // clone.mask, set and listed.
    let set = ok(
        "clones",
        at(
            addr,
            &["set", "mask", "notes.title", "--reason", "titles are safe"],
        ),
    );
    assert!(set.contains("clone.mask notes.title"), "{set}");
    let list = ok("clones", at(addr, &["list"]));
    assert!(list.contains("clone.mask notes.title"), "{list}");
    assert!(list.contains("clone.readers none"), "{list}");
    let bad = refused(
        "clones",
        at(addr, &["set", "mask", "everything", "--reason", "x"]),
    );
    assert!(bad.contains("entity.field"), "{bad}");

    // A key for an environment this deployment does not know.
    let e = refused(
        "clones",
        at(addr, &["set-key", "qa", &key, "--reason", "x"]),
    );
    assert!(e.contains("ordo environments register qa"), "{e}");

    // As staging, with no door recorded for production.
    ok(
        "environments",
        at(
            addr,
            &[
                "register",
                "staging",
                "--position",
                "1",
                "--reason",
                "chain",
            ],
        ),
    );
    ok(
        "environments",
        at(addr, &["set-own", "staging", "--reason", "this is staging"]),
    );
    let e = refused(
        "clones",
        at(
            addr,
            &["clone", "acme", "--from-token", "t", "--reason", "rehearse"],
        ),
    );
    assert!(e.contains("ordo environments door production"), "{e}");
    // Without --from-token, the same missing record is named first.
    let e = refused(
        "clones",
        at(addr, &["clone", "acme", "--reason", "rehearse"]),
    );
    assert!(e.contains("ordo environments door production"), "{e}");
}
