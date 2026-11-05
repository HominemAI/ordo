//! 17u — `ordo apply --host`, against a deployment running in this process.
//!
//! `ordo-host`'s `deploy_apply.rs` proves the door: who may call it, what
//! it records, that two applies queue. This file proves the client half,
//! and the one property 17u is actually a promise about:
//!
//! **the same program, applied over the wire and applied against the
//! database, prints the same report** — byte for byte, to two empty
//! databases that differ in nothing but their name. Not "equivalent", not
//! "the same fields": the same text, because both paths render the same
//! `ordo_host::apply_op::Report` through the same function. A test that
//! compared field by field would pass for a build where the two renderers
//! had drifted, which is the only way this can go wrong.
//!
//! And the refusals carry the same classes, which is the half an operator
//! meets on a bad day.
//!
//! # Why the CLI runs on the test's own thread
//!
//! `operator::run` builds a tokio runtime and `block_on`s it, because a
//! command with an exit code is what cron consumes. So these are plain
//! `#[test]`s: the server gets a runtime of its own, kept alive for the
//! length of the test, and the CLI is invoked from outside it exactly as a
//! shell would.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

mod support;
use ordo_conformance::Pg;
use support::door::{door, runtime, serve, token, DEP_MANAGER, DEP_OWNER, HOME};
use support::operator;

const V1: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
}

op listA unbounded =
  a[name, n]
"#;

/// V1 plus a required field with no default: a change `ordo apply` does not
/// make for you (B2), so the gate refuses an artifact of it, which is
/// `not_activated` on both paths. An optional field (A2) would now be made
/// for you (`ORDO_APPLY_SAFE_CHANGES`, owner ruling 2026-10-22).
const V2: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
  tag:   text
}

op listA unbounded =
  a[name, n, tag]
"#;

/// Write a program where an operator would keep one.
fn program(tag: &str, text: &str) -> String {
    let dir = std::env::temp_dir().join(format!("ordo-wire-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("p.ordo");
    std::fs::write(&path, text).expect("write program");
    path.display().to_string()
}

fn empty(db: &str) -> Pg {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    pg
}

fn over_the_wire(path: &str, addr: std::net::SocketAddr, tok: &str) -> operator::Outcome {
    operator::run(
        "apply",
        &[
            path.into(),
            // `http://`, spelled. A bare host would get https, which is
            // the right default for a public deployment and the wrong one
            // for a loopback port with no certificate — see `wire.rs`.
            "--host".into(),
            format!("http://{addr}"),
            "--token".into(),
            tok.into(),
            "--space".into(),
            HOME.into(),
            // These tests compare the apply's report on the two paths. The
            // wait after it differs by path and is apply_wait.rs's subject.
            "--no-wait".into(),
        ],
    )
}

fn against_the_database(path: &str, db: &str, actor: &str) -> operator::Outcome {
    operator::run(
        "apply",
        &[
            path.into(),
            // The direct path: the first apply of a database no host serves
            // yet. Named, and recorded for the next boot.
            "--direct".into(),
            "--db".into(),
            db.into(),
            "--as".into(),
            actor.into(),
            "--no-wait".into(),
        ],
    )
}

// ── the promise ──────────────────────────────────────────────────────────

/// The same program, two empty databases, two paths, one report.
#[test]
fn the_wire_and_the_database_print_the_same_report() {
    let wire_db = "ordo_wire_same_a";
    let direct_db = "ordo_wire_same_b";
    empty(wire_db);
    empty(direct_db);
    let path = program("same", V1);

    let rt = runtime();
    let addr = rt.block_on(serve(wire_db));

    let over = over_the_wire(&path, addr, &token(DEP_OWNER));
    assert_eq!(over.code, 0, "{}", over.report);

    // `--as dep_owner` on the direct path, because that is the principal
    // the token proved on the other one. The reports name no actor, so this
    // is about the `$migrations` row rather than the text — but a test that
    // let the two differ would be comparing the wrong two runs.
    let direct = against_the_database(&path, direct_db, DEP_OWNER);
    assert_eq!(direct.code, 0, "{}", direct.report);

    assert_eq!(
        over.report, direct.report,
        "one report, rendered once:\n--- over the wire ---\n{}\n--- against the database \
         ---\n{}",
        over.report, direct.report
    );
    // And it is the report of an apply that actually happened, not two
    // empty strings that trivially match.
    assert!(
        over.report.contains("schema     created"),
        "{}",
        over.report
    );
    assert!(over.report.contains("activated"), "{}", over.report);
}

/// A refusal over the wire carries the class the direct path prints.
///
/// V1 first on both, then V2, which the load gate would refuse over a
/// database holding V1's schema. Both say `not_activated`.
#[test]
fn a_refusal_over_the_wire_carries_the_direct_paths_class() {
    let wire_db = "ordo_wire_refuse_a";
    let direct_db = "ordo_wire_refuse_b";
    empty(wire_db);
    empty(direct_db);
    let v1 = program("refuse-v1", V1);
    let v2 = program("refuse-v2", V2);

    let rt = runtime();
    let addr = rt.block_on(serve(wire_db));
    let tok = token(DEP_OWNER);

    assert_eq!(over_the_wire(&v1, addr, &tok).code, 0);
    assert_eq!(against_the_database(&v1, direct_db, DEP_OWNER).code, 0);

    let over = over_the_wire(&v2, addr, &tok);
    let direct = against_the_database(&v2, direct_db, DEP_OWNER);
    assert_eq!(over.code, 1, "{}", over.report);
    assert_eq!(direct.code, 1, "{}", direct.report);

    // The direct path renders the refusal with the remedy that names the
    // operator's own path and database; over the wire the class and the
    // message are the door's envelope. Both name the same thing.
    assert!(
        direct.report.contains("not activated") && direct.report.contains("B2"),
        "{}",
        direct.report
    );
    assert!(
        over.report.contains("[not_activated]") && over.report.contains("B2"),
        "{}",
        over.report
    );
}

/// A program that does not compile is refused on both paths, with the same
/// class, and nothing reaches either database.
#[test]
fn a_broken_program_is_refused_on_both_paths() {
    let wire_db = "ordo_wire_broken_a";
    let direct_db = "ordo_wire_broken_b";
    let wire_pg = empty(wire_db);
    empty(direct_db);
    let path = program("broken", "entity {{{");

    let rt = runtime();
    let addr = rt.block_on(serve(wire_db));

    let over = over_the_wire(&path, addr, &token(DEP_OWNER));
    let direct = against_the_database(&path, direct_db, DEP_OWNER);
    assert_eq!(over.code, 1, "{}", over.report);
    assert_eq!(direct.code, 1, "{}", direct.report);
    assert!(
        over.report.contains("[apply_refused]"),
        "the wire class: {}",
        over.report
    );

    // The program's own entities. `$`-prefixed tables are the platform's
    // and are created by `Pool::connect` before any apply runs, so counting
    // every table would count the pool's own bookkeeping.
    let tables = wire_pg
        .query(
            "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND relkind = 'r' AND relname NOT LIKE '$%'",
        )
        .expect("catalog");
    assert_eq!(
        tables[0][0].as_deref(),
        Some("0"),
        "nothing was created by a program that does not compile"
    );
}

// ── what the client refuses before it sends anything ─────────────────────

/// A non-owner's token is refused by the door, and the CLI prints the
/// class rather than a stack of HTTP.
#[test]
fn a_manager_is_refused_over_the_wire_by_name() {
    let db = "ordo_wire_mgr";
    empty(db);
    let path = program("mgr", V1);

    let rt = runtime();
    let addr = rt.block_on(serve(db));

    let out = over_the_wire(&path, addr, &token(DEP_MANAGER));
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(
        out.report.contains("[access_denied]"),
        "the door's own class, printed: {}",
        out.report
    );
}

/// `--as` is an assertion, and this door does not take one. Refused as a
/// usage error, before a connection is opened.
#[test]
fn as_is_refused_together_with_host() {
    let path = program("as", V1);
    let out = operator::run(
        "apply",
        &[
            path,
            "--host".into(),
            "http://127.0.0.1:1".into(),
            "--token".into(),
            "t".into(),
            "--space".into(),
            HOME.into(),
            "--as".into(),
            "somebody".into(),
        ],
    );
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(
        out.report.contains("`--as` is an assertion"),
        "{}",
        out.report
    );
}

/// No token, no machine key and no session is a usage error that says how
/// to sign in, and opens no connection.
#[test]
fn a_missing_token_says_where_one_comes_from() {
    let path = program("notoken", V1);
    // The port is deliberately one nothing listens on: if this reached the
    // network the test would fail differently, which is the point.
    let out = operator::run(
        "apply",
        &[
            path,
            "--host".into(),
            "http://127.0.0.1:1".into(),
            "--space".into(),
            HOME.into(),
        ],
    );
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(out.report.contains("ordo login"), "{}", out.report);
    assert!(out.report.contains("ORDO_MACHINE_KEY"), "{}", out.report);
}

/// The database path is `--direct` only (owner, 2026-11-05): the first
/// apply, when no host serves the database yet. It writes the program, and
/// leaves a row for the next boot to carry to the security log.
#[test]
fn with_direct_the_command_talks_to_the_database_and_leaves_a_record() {
    let db = "ordo_wire_default";
    empty(db);
    let path = program("default", V1);
    let out = against_the_database(&path, db, "test");
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("schema     created"), "{}", out.report);

    let uses = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
                size: 1,
                ..ordo_exec::PoolConfig::from_env()
            })
            .await
            .expect("pool");
            ordo_host::direct_uses::pending(&pool).await.expect("rows")
        });
    assert!(
        uses.iter().any(|u| u.command == "ordo_apply"),
        "the use is on record for the next boot: {uses:?}"
    );
}

// ── backend functions ────────────────────────────────────────────────────

/// V1 with a function and its secret (`plan-backend-functions.md` §4.3).
const WITH_FN: &str = r#"
domain fx @ 0.1.0

entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
}

op listA unbounded =
  a[name, n]

secret k for api.example.com

function hello in fx {
  call($text: text(100)) -> json by member
  code "fn/hello.js"
  egress api.example.com
  secrets k
}
"#;

const HELLO_JS: &str = "export default { async call(input) { return { said: input.text }; } };\n";

/// A program with a function, beside its code file.
fn program_with_fn(tag: &str, js: &str) -> String {
    let path = program(tag, WITH_FN);
    let dir = std::path::Path::new(&path).parent().unwrap().join("fn");
    std::fs::create_dir_all(&dir).expect("fn dir");
    std::fs::write(dir.join("hello.js"), js).expect("write code");
    path
}

/// A program with a function applies over the door as it does against the
/// database: the same report, so the same version, and the code is stored
/// by its hash. The secret has no value, and that does not block the apply.
#[test]
fn a_function_applies_over_the_wire_with_its_code() {
    let wire_db = "ordo_wire_fn_a";
    let direct_db = "ordo_wire_fn_b";
    empty(wire_db);
    empty(direct_db);
    let path = program_with_fn("fn", HELLO_JS);

    let rt = runtime();
    let addr = rt.block_on(serve(wire_db));

    let over = over_the_wire(&path, addr, &token(DEP_OWNER));
    assert_eq!(over.code, 0, "{}", over.report);
    let direct = against_the_database(&path, direct_db, DEP_OWNER);
    assert_eq!(direct.code, 0, "{}", direct.report);
    assert_eq!(
        over.report, direct.report,
        "--- over the wire ---\n{}\n--- against the database ---\n{}",
        over.report, direct.report
    );

    let hash = ordo_plan::artifact::code_hash(HELLO_JS.as_bytes());
    let rows = Pg::named(wire_db)
        .query(&format!(
            "SELECT \"code\" FROM \"$fncode\" WHERE \"hash\" = '{hash}'"
        ))
        .expect("$fncode");
    // `psql` trims the trailing newline it prints; the hash above is of
    // the exact bytes.
    assert_eq!(
        rows,
        vec![vec![Some(HELLO_JS.trim_end().to_string())]],
        "the door stores the code by hash"
    );
}

/// A change to the code alone is a new version over the door too.
#[test]
fn a_code_change_alone_is_a_new_version_over_the_wire() {
    let db = "ordo_wire_fn_change";
    empty(db);
    let rt = runtime();
    let addr = rt.block_on(serve(db));
    let first = over_the_wire(&program_with_fn("fn_v1", HELLO_JS), addr, &token(DEP_OWNER));
    assert_eq!(first.code, 0, "{}", first.report);
    let changed = HELLO_JS.replace("said", "heard");
    let path = program_with_fn("fn_v2", &changed);
    // A real change, the code's included, takes a version of its own
    // (owner, 2026-11-03, ask 13): unraised, the door refuses it by name.
    let unraised = over_the_wire(&path, addr, &token(DEP_OWNER));
    assert!(
        unraised.report.contains("version_taken"),
        "{}",
        unraised.report
    );
    // `--bump` reads the number from the door's refusal, raises it in the
    // file, and applies again.
    let second = operator::run(
        "apply",
        &[
            path.clone(),
            "--bump".into(),
            "--host".into(),
            format!("http://{addr}"),
            "--token".into(),
            token(DEP_OWNER),
            "--space".into(),
            HOME.into(),
            "--no-wait".into(),
        ],
    );
    assert_eq!(second.code, 0, "{}", second.report);
    assert!(
        second.report.contains("@ 0.1.0 -> @ 0.1.1"),
        "{}",
        second.report
    );
    assert!(std::fs::read_to_string(&path)
        .expect("read")
        .contains("domain fx @ 0.1.1"));
    assert_ne!(
        first.report, second.report,
        "a change to the code alone stores a new version"
    );
}

/// `--label` and the source's git commit reach the door with the program,
/// and `ordo versions label|diff --host` print the deployment's answer
/// (`plan-versions.md` §4 row 2).
#[test]
fn a_label_and_a_commit_travel_and_versions_answer_over_the_wire() {
    let db = "ordo_wire_labels";
    empty(db);
    let path = program("labels", V1);
    let dir = std::path::Path::new(&path).parent().unwrap().to_path_buf();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .status()
            .expect("git")
            .success());
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "v1"]);

    let rt = runtime();
    let addr = rt.block_on(serve(db));
    let owner = token(DEP_OWNER);
    let with_door = |head: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = head.iter().map(|s| s.to_string()).collect();
        v.extend(door(addr, &owner));
        v
    };

    let mut args = with_door(&[&path, "--label", "over the wire", "--no-wait"]);
    let first = operator::run("apply", &args);
    assert_eq!(first.code, 0, "{}", first.report);
    assert!(
        first.report.contains("label      over the wire\n"),
        "{}",
        first.report
    );
    assert!(first.report.contains("commit     "), "{}", first.report);

    // V2 is refused activation over V1's schema, and stored all the same:
    // a second version to compare with.
    std::fs::write(&path, V2).expect("write");
    args.retain(|a| a != "--label" && a != "over the wire");
    let second = operator::run("apply", &args);
    assert!(second.report.contains("not_activated"), "{}", second.report);

    let label = operator::run("versions", &with_door(&["label", "#2", "staged"]));
    assert_eq!(label.code, 0, "{}", label.report);
    assert!(
        label.report.starts_with("labelled   #2 · "),
        "{}",
        label.report
    );
    assert!(label.report.contains("\"staged\""), "{}", label.report);

    let long = "x".repeat(81);
    let refused = operator::run("versions", &with_door(&["label", "#2", &long]));
    assert_ne!(refused.code, 0);
    assert!(refused.report.contains("at most 80"), "{}", refused.report);

    let over = operator::run("versions", &with_door(&["diff", "#1", "#2"]));
    assert_eq!(over.code, 0, "{}", over.report);
    assert!(over.report.contains("  B2  "), "{}", over.report);

    let source = operator::run("versions", &with_door(&["diff", "#1", "#2", "--source"]));
    assert!(!source.report.contains("SCHEMA"), "{}", source.report);
    let missing = operator::run("versions", &with_door(&["diff", "#1", "#9"]));
    assert_ne!(missing.code, 0);
    assert!(
        missing.report.contains("not_found") && missing.report.contains("#2 · "),
        "{}",
        missing.report
    );

    // A manager may not label the deployment's versions.
    let manager = token(DEP_MANAGER);
    let mut m = vec!["label".to_string(), "#1".into(), "nope".into()];
    m.extend(door(addr, &manager));
    let denied = operator::run("versions", &m);
    assert_ne!(denied.code, 0, "{}", denied.report);
}

/// `ordo versions revoke|unrevoke|revoked` and `ordo windows` over the door
/// print what the database path prints (`plan-versions.md` §4 row 2b).
#[test]
fn revokes_and_windows_answer_the_same_over_the_wire() {
    let db = "ordo_wire_revoke";
    empty(db);
    let path = program("revoke", V1);
    let rt = runtime();
    let addr = rt.block_on(serve(db));
    let owner = token(DEP_OWNER);
    let with_door = |head: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = head.iter().map(|s| s.to_string()).collect();
        v.extend(door(addr, &owner));
        v
    };
    let first = operator::run("apply", &with_door(&[&path, "--no-wait"]));
    assert_eq!(first.code, 0, "{}", first.report);
    std::fs::write(&path, V2).expect("write");
    let second = operator::run("apply", &with_door(&[&path, "--no-wait"]));
    assert!(second.report.contains("not_activated"), "{}", second.report);

    // #1 serves. Revoking it needs `--move-to`, and #2 cannot be loaded
    // over this database, so the move is refused by the load gate.
    let bare = operator::run("versions", &with_door(&["revoke", "#1", "--reason", "x"]));
    assert_ne!(bare.code, 0);
    assert!(bare.report.contains("--move-to"), "{}", bare.report);
    let gated = operator::run(
        "versions",
        &with_door(&["revoke", "#1", "--reason", "x", "--move-to", "#2"]),
    );
    assert!(gated.report.contains("not_activated"), "{}", gated.report);

    // #2 is only marked. Over the door, lifted, then again on the database.
    let over = operator::run(
        "versions",
        &with_door(&["revoke", "#2", "--reason", "drops a column"]),
    );
    assert_eq!(over.code, 0, "{}", over.report);
    let report_over = operator::run("versions", &with_door(&["revoked"]));
    assert_eq!(report_over.code, 0, "{}", report_over.report);
    assert!(report_over.report.contains("drops a column"));
    let lifted = operator::run(
        "versions",
        &with_door(&["unrevoke", "#2", "--reason", "ok"]),
    );
    assert_eq!(lifted.code, 0, "{}", lifted.report);
    let lifted_again = operator::run(
        "versions",
        &with_door(&["unrevoke", "#2", "--reason", "ok"]),
    );
    assert!(
        lifted_again.report.contains("not_revoked"),
        "{}",
        lifted_again.report
    );
    let again = operator::run(
        "versions",
        &with_door(&["revoke", "#2", "--reason", "drops a column"]),
    );
    assert_eq!(over.report, again.report, "the same words each time");
    let list = operator::run("versions", &with_door(&["list"]));
    assert!(
        list.report.contains("REVOKED \"drops a column\""),
        "{}",
        list.report
    );

    // A manager may not revoke.
    let manager = token(DEP_MANAGER);
    let mut m = vec![
        "unrevoke".to_string(),
        "#2".into(),
        "--reason".into(),
        "x".into(),
    ];
    m.extend(door(addr, &manager));
    assert_ne!(operator::run("versions", &m).code, 0);

    // Windows: a tenant to name, then the same window both ways.
    rt.block_on(async {
        let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
            db: db.to_string(),
            size: 1,
            ..ordo_exec::PoolConfig::from_env()
        })
        .await
        .expect("pool");
        ordo_host::tenants::ensure_table(&pool)
            .await
            .expect("$spaces");
        ordo_host::tenants::reserve(&pool, "spc_w", "w", "dev:ada", 1, None)
            .await
            .expect("tenant");
    });
    let set = [
        "set",
        "--id",
        "weekly",
        "--except",
        "spc_w",
        "--cron",
        "0 2 * * sun",
        "--length",
        "2h",
        "--message",
        "weekly upkeep",
    ];
    let over = operator::run("windows", &with_door(&set));
    assert_eq!(over.code, 0, "{}", over.report);
    let again = operator::run("windows", &with_door(&set));
    assert_eq!(over.report, again.report, "the same words each time");
    let skip = [
        "skip",
        "weekly",
        "--id",
        "freeze",
        "--starts",
        "2026-12-01T00:00Z",
        "--ends",
        "2026-12-31T00:00Z",
    ];
    let over = operator::run("windows", &with_door(&skip));
    assert_eq!(over.code, 0, "{}", over.report);
    let over = operator::run("windows", &with_door(&["list"]));
    assert_eq!(over.code, 0, "{}", over.report);
    assert!(over.report.contains("skipped by freeze"), "{}", over.report);
    // spc_w is left out of the window.
    let w = operator::run("windows", &with_door(&["list", "spc_w"]));
    assert!(w.report.contains("TIMES (0)"), "{}", w.report);
    let ghost = operator::run("windows", &with_door(&["list", "spc_ghost"]));
    assert_ne!(ghost.code, 0);
    assert!(ghost.report.contains("spc_ghost"), "{}", ghost.report);
    let gone = operator::run("windows", &with_door(&["unskip", "freeze"]));
    assert_eq!(gone.code, 0, "{}", gone.report);
    let gone = operator::run("windows", &with_door(&["delete", "weekly"]));
    assert_eq!(gone.code, 0, "{}", gone.report);
    let missing = operator::run("windows", &with_door(&["delete", "weekly"]));
    assert!(
        missing.report.contains("ordo windows list"),
        "{}",
        missing.report
    );
}

/// The commands that had a database path and now have only the door print
/// from the deployment's own answers: `versions list|show`, `rollback`,
/// `plan`, `fleet` and `quota` (owner, 2026-11-05).
#[test]
fn versions_rollback_plan_fleet_and_quota_answer_over_the_wire() {
    let db = "ordo_wire_reads";
    empty(db);
    let path = program("reads", V1);
    let rt = runtime();
    let addr = rt.block_on(serve(db));
    let owner = token(DEP_OWNER);
    let with_door = |head: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = head.iter().map(|s| s.to_string()).collect();
        v.extend(door(addr, &owner));
        v
    };
    let first = operator::run("apply", &with_door(&[&path, "--no-wait"]));
    assert_eq!(first.code, 0, "{}", first.report);

    let list = operator::run("versions", &with_door(&["list"]));
    assert_eq!(list.code, 0, "{}", list.report);
    assert!(list.report.starts_with("VERSIONS (1,"), "{}", list.report);
    assert!(list.report.contains("<- active"), "{}", list.report);
    let show = operator::run("versions", &with_door(&["show", "#1"]));
    assert_eq!(show.code, 0, "{}", show.report);
    assert!(
        show.report.starts_with("version    #1 · "),
        "{}",
        show.report
    );
    assert!(show.report.contains("ACTIVATIONS (1,"), "{}", show.report);
    let none = operator::run("versions", &with_door(&["show", "#9"]));
    assert_ne!(none.code, 0);
    assert!(
        none.report.contains("no version is stored"),
        "{}",
        none.report
    );

    // #1 already serves; a rollback to it changes nothing. A number that
    // names nothing is refused.
    let same = operator::run("rollback", &with_door(&["--version", "#1"]));
    assert_eq!(same.code, 0, "{}", same.report);
    assert!(same.report.contains("already active"), "{}", same.report);
    let ghost = operator::run("rollback", &with_door(&["--version", "#9"]));
    assert_ne!(ghost.code, 0);
    assert!(
        ghost.report.contains("#1 · "),
        "names what is stored: {}",
        ghost.report
    );

    // The deployment says what applying this program would do.
    let plan = operator::run("plan", &with_door(&[&path]));
    assert_eq!(plan.code, 0, "{}", plan.report);
    assert!(plan.report.contains("matches"), "{}", plan.report);

    // The fleet, from the registry on the deployment's database.
    rt.block_on(async {
        let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
            db: db.to_string(),
            size: 1,
            ..ordo_exec::PoolConfig::from_env()
        })
        .await
        .expect("pool");
        ordo_host::fleet::ensure_tables(&pool)
            .await
            .expect("tables");
        ordo_host::tenants::reserve(&pool, "spc_fleet", "Acme Corp", "dev:ada", 1, None)
            .await
            .expect("tenant");
        ordo_host::quotas::ensure_table(&pool)
            .await
            .expect("$quotas");
    });
    let fleet = operator::run("fleet", &with_door(&[]));
    assert_eq!(fleet.code, 0, "{}", fleet.report);
    let line = fleet
        .report
        .lines()
        .find(|l| l.contains("spc_fleet"))
        .unwrap_or_else(|| panic!("the tenant's id is not printed: {}", fleet.report));
    assert!(line.contains("Acme Corp"), "the name beside the id: {line}");
    assert!(
        fleet.report.starts_with("target  default"),
        "{}",
        fleet.report
    );

    // Quotas: `--space` is the tenant, `--home` the deployment's own space.
    let quota = |head: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = head.iter().map(|s| s.to_string()).collect();
        v.extend([
            "--host".to_string(),
            format!("http://{addr}"),
            "--token".to_string(),
            owner.clone(),
            "--home".to_string(),
            HOME.to_string(),
        ]);
        v
    };
    let set = operator::run(
        "quota",
        &quota(&[
            "set",
            "--space",
            "spc_fleet",
            "--quantity",
            "requests",
            "--limit",
            "100",
            "--grace",
            "10",
            "--period-secs",
            "3600",
        ]),
    );
    assert_eq!(set.code, 0, "{}", set.report);
    assert!(
        set.report.contains("SET     requests limit 100"),
        "{}",
        set.report
    );
    let shown = operator::run("quota", &quota(&["show", "--space", "spc_fleet"]));
    assert_eq!(shown.code, 0, "{}", shown.report);
    assert!(shown.report.contains("requests"), "{}", shown.report);
    assert!(shown.report.contains("limit 100"), "{}", shown.report);
}

/// A command that has a door and no `--host` is refused, and the refusal
/// names the flag to use. `--db` and `--direct` do not reopen a path that
/// was closed (owner, 2026-11-05: "the CLI goes through the door").
#[test]
fn a_door_command_with_no_host_is_refused_naming_the_flag() {
    for (cmd, args) in [
        ("versions", vec!["list"]),
        ("versions", vec!["revoke", "#1", "--reason", "x"]),
        ("windows", vec!["list"]),
        ("fleet", vec![]),
        ("quota", vec!["show", "--space", "spc_x"]),
        ("plan", vec!["p.ordo", "--against", "somedb"]),
    ] {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let out = operator::run(cmd, &args);
        assert_eq!(out.code, 2, "{cmd}: {}", out.report);
        assert!(out.report.contains("--host"), "{cmd}: {}", out.report);
        let mut with_db = args.clone();
        with_db.extend(["--db".to_string(), "somedb".to_string()]);
        let out = operator::run(cmd, &with_db);
        assert_eq!(out.code, 2, "{cmd} --db: {}", out.report);
        assert!(out.report.contains("--host"), "{cmd} --db: {}", out.report);
        let mut with_direct = args.clone();
        with_direct.push("--direct".to_string());
        let out = operator::run(cmd, &with_direct);
        assert_eq!(out.code, 2, "{cmd} --direct: {}", out.report);
        assert!(
            out.report.contains("no direct path") && out.report.contains("--host"),
            "{cmd} --direct: {}",
            out.report
        );
    }
}

/// `apply` and `rollback` take the door, or `--direct` when no host can
/// boot. With neither, both flags are named. With both, one must go.
#[test]
fn apply_and_rollback_name_both_flags_when_given_neither() {
    for cmd in ["apply", "rollback"] {
        let args: Vec<String> = if cmd == "apply" {
            vec!["p.ordo".into()]
        } else {
            vec!["--version".into(), "#1".into()]
        };
        let out = operator::run(cmd, &args);
        assert_eq!(out.code, 2, "{cmd}: {}", out.report);
        assert!(out.report.contains("--host"), "{cmd}: {}", out.report);
        assert!(out.report.contains("--direct"), "{cmd}: {}", out.report);
        let mut both = args.clone();
        both.extend([
            "--direct".to_string(),
            "--host".to_string(),
            "http://127.0.0.1:1".to_string(),
        ]);
        let out = operator::run(cmd, &both);
        assert_eq!(out.code, 2, "{cmd}: {}", out.report);
        assert!(out.report.contains("name one"), "{cmd}: {}", out.report);
    }
}
