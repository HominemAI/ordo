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

use std::sync::Arc;

use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
mod support;
use ordo_conformance::{Fixture, Pg};
use ordo_host::apply_op;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use support::operator;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"wire-apply-secret";
const HOME: &str = "ordo_home";
const DEP_OWNER: &str = "dep_owner";
const DEP_MANAGER: &str = "dep_manager";

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

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            // Present, so step-up is satisfied. Every platform op requires
            // it; see `wire.rs`'s module doc on where an operator's token
            // comes from.
            user_presence_at: Some(IStamp(NOW)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// The deployment, listening on a loopback port.
///
/// A real socket rather than `oneshot` on the router: the point of this
/// file is the **client**, and a client that was handed a `Service` would
/// prove nothing about the request it builds, the headers it sends or the
/// body it reads back off the wire.
async fn serve(target_db: &str) -> std::net::SocketAddr {
    let f = Arc::new(Fixture::load().expect("fixture"));

    let dir_db = format!("{target_db}_dir");
    let dir_pg = Pg::named(&dir_db);
    f.reset(&dir_pg).expect("seeded");
    let dir_pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: dir_db,
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("directory pool");

    let target = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: target_db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("target pool");

    let grants = Arc::new(PgGrantRegistry::new(dir_pool.clone()));
    grants.ensure_table().await.expect("platform table");

    // The same rows, through the same registration loop a deployment uses.
    let row = |id: &str, role: &str| {
        vec![
            Some(HOME.to_string()),
            Some(id.to_string()),
            Some(role.to_string()),
        ]
    };
    let engine = Arc::new(
        ordo_host::engine_from_members(&f, &[row(DEP_OWNER, "owner"), row(DEP_MANAGER, "manager")])
            .expect("engine"),
    );

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "wire-apply-test",
        f.clone(),
        ordo_exec::Executor {
            pool: dir_pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("platform specs");

    let ops = Arc::new(
        PlatformOps::new(engine.clone(), grants, Arc::new(|| NOW)).with_apply(
            apply_op::ApplyOps::new(
                target.clone(),
                std::sync::Arc::new(ordo_exec::route::SinglePool::new(target)),
                HOME,
            ),
        ),
    );
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: dir_pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access: Arc::new(ordo_host::IdentityAccess::sharing(
            engine,
            TokenKey::from_secret(SECRET),
            Box::new(|| NOW),
            Arc::new(ordo_host::metrics::Metrics::new()),
        )),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"wire-apply-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    addr
}

/// A runtime the server lives on, held for the length of the test. The CLI
/// runs outside it, on the test's own thread, and builds its own.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
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

/// The database path is still the default. No `--host`, no environment,
/// and the command talks to Postgres exactly as it always did.
#[test]
fn without_a_host_the_command_still_talks_to_the_database() {
    let db = "ordo_wire_default";
    empty(db);
    let path = program("default", V1);
    let out = against_the_database(&path, db, "test");
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("schema     created"), "{}", out.report);
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
    let second = over_the_wire(&program_with_fn("fn_v2", &changed), addr, &token(DEP_OWNER));
    assert_eq!(second.code, 0, "{}", second.report);
    assert_ne!(
        first.report, second.report,
        "a change to the code alone stores a new version"
    );
}
