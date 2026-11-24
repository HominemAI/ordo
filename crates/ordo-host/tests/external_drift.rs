//! External entities, slice C (ruled 2026-11-15, `external-entities`): the
//! check of each translation against its live table, at plan and apply, and
//! the watcher that holds and lifts the entity at run time.
//!
//! The table is made by raw SQL in its own schema, the way another system
//! would make it, then changed the way that system might. What it proves:
//!
//! - A mapped column dropped, retyped, or made nullable against the file
//!   refuses `external_changed`, naming the column and the fix.
//! - A column added, or an unmapped column dropped, is taken. The plan notes
//!   the columns the translation does not read.
//! - A table or schema that is gone refuses `external_missing`.
//! - A role with no `SELECT` refuses `external_no_grant`, naming the `GRANT`.
//! - The apply refuses on any of these, a first apply included, and the
//!   foreign table is not touched.
//! - The watcher holds the entity: every op that reads it answers
//!   `external_changed`, an op that does not read it still answers, and the
//!   first pass that finds the table as the program says serves it again.
//!
//! A regression looks like an apply that creates a schema over a missing
//! table, a refusal that names no column, a held entity that never lifts, or
//! a native op refused because an external is down.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_extd_`.

mod refine_support;

use std::sync::Arc;

use axum::http::StatusCode;
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::external_check::{self, ExternalGate, ExternalGated, ProblemKind};
use ordo_host::schema_apply::{self as sa, ApplyRefusal, Program};
use refine_support::{call, door_over};

const PROGRAM: &str = r#"
external patrons (key: pat) from crm.customers shared {
  id <- "customer_id"
  name: text <- "full_name"
  email: text? <- email
}

entity tickets {
  customer -> patrons
  subject: text
}

op patronByEmail($e: text) =
  patrons(email == $e)[id, name]

op mine =
  patrons(name == $caller)[id]
"#;

const FOREIGN: &str = r#"
DROP SCHEMA IF EXISTS crm CASCADE;
CREATE SCHEMA crm;
CREATE TABLE crm.customers (
  customer_id bigint PRIMARY KEY,
  full_name   text NOT NULL,
  email       text
);
INSERT INTO crm.customers VALUES (9, 'Ada', 'ada@example.com'), (42, 'Cy', NULL);
"#;

fn source() -> String {
    format!("{FIXTURE}\n{PROGRAM}")
}

fn fixture() -> Arc<Fixture> {
    let seed: &'static str = Box::leak(format!("{SEED}\n{FOREIGN}").into_boxed_str());
    Arc::new(Fixture::from_source(&source(), seed).expect("a program with an external compiles"))
}

/// A database with the program's tables and the foreign table, and a pool.
async fn setup(db: &str) -> (Arc<Fixture>, Pg, Pool, Program) {
    let f = fixture();
    let pg = Pg::named(db);
    f.reset(&pg).expect("DDL applies and the fixture seeds");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    let program = Program::compile(&source()).expect("the program compiles");
    (f, pg, pool, program)
}

async fn problems(pool: &Pool, f: &Fixture) -> Vec<(ProblemKind, String)> {
    external_check::check(pool, &f.schema)
        .await
        .expect("the check reads")
        .problems
        .into_iter()
        .map(|p| (p.kind, p.detail))
        .collect()
}

#[tokio::test]
async fn an_unchanged_table_has_no_problem_and_the_apply_goes_on() {
    let (f, _pg, pool, program) = setup("ordo_extd_same").await;
    assert!(problems(&pool, &f).await.is_empty());
    let (plan, applied) = sa::apply(&pool, &program, "test", "test").await;
    let plan = plan.expect("plans");
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
    assert!(applied.is_ok(), "{applied:?}");
}

#[tokio::test]
async fn a_dropped_mapped_column_refuses_naming_the_column_and_the_fix() {
    let (f, pg, pool, program) = setup("ordo_extd_drop").await;
    pg.exec("ALTER TABLE crm.customers DROP COLUMN full_name")
        .expect("the other system drops it");
    let found = problems(&pool, &f).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, ProblemKind::Changed);
    assert!(
        found[0].1.contains("column `full_name` is gone"),
        "{found:?}"
    );
    assert!(
        found[0]
            .1
            .contains("ordo introspect --external crm.customers"),
        "the fix is named: {found:?}"
    );

    let (plan, applied) = sa::apply(&pool, &program, "test", "test").await;
    let plan = plan.expect("plans");
    assert_eq!(plan.refusals.len(), 1, "{:?}", plan.refusals);
    assert!(
        plan.refusals[0].starts_with("external_changed on `patrons`: "),
        "{:?}",
        plan.refusals
    );
    assert!(matches!(applied, Err(ApplyRefusal::Plan)), "{applied:?}");
}

#[tokio::test]
async fn a_retyped_column_and_a_new_null_are_refused() {
    let (f, pg, pool, _program) = setup("ordo_extd_type").await;
    pg.exec("ALTER TABLE crm.customers ALTER COLUMN email TYPE integer USING 0")
        .expect("retypes");
    let found = problems(&pool, &f).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .1
            .contains("column `email` is `integer`, and the file reads it as text"),
        "{found:?}"
    );

    pg.exec(
        "ALTER TABLE crm.customers ALTER COLUMN email TYPE text USING NULL; \
         ALTER TABLE crm.customers ALTER COLUMN full_name DROP NOT NULL;",
    )
    .expect("restores one, loosens the other");
    let found = problems(&pool, &f).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].1.contains("column `full_name` admits null"),
        "{found:?}"
    );
}

#[tokio::test]
async fn an_added_column_is_taken_with_a_note_and_never_refused() {
    let (f, pg, pool, program) = setup("ordo_extd_add").await;
    pg.exec("ALTER TABLE crm.customers ADD COLUMN phone text, ADD COLUMN tier int")
        .expect("the other system adds columns");
    let report = external_check::check(&pool, &f.schema)
        .await
        .expect("reads");
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
    assert!(
        report.notes[0].contains("`phone`") && report.notes[0].contains("`tier`"),
        "{:?}",
        report.notes
    );
    let (plan, applied) = sa::apply(&pool, &program, "test", "test").await;
    let plan = plan.expect("plans");
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
    assert!(applied.is_ok(), "{applied:?}");

    // An unused column changing or going is taken too: nothing reads it.
    pg.exec("ALTER TABLE crm.customers ALTER COLUMN tier TYPE text; ALTER TABLE crm.customers DROP COLUMN phone;")
        .expect("changes unused columns");
    assert!(problems(&pool, &f).await.is_empty());
}

#[tokio::test]
async fn a_missing_table_or_schema_refuses_external_missing() {
    let (f, pg, pool, program) = setup("ordo_extd_missing").await;
    pg.exec("DROP TABLE crm.customers")
        .expect("drops the table");
    let found = problems(&pool, &f).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, ProblemKind::Missing);
    assert!(found[0]
        .1
        .contains("table `crm.customers` is not in this database"));

    pg.exec("DROP SCHEMA crm CASCADE")
        .expect("drops the schema");
    assert_eq!(problems(&pool, &f).await[0].0, ProblemKind::Missing);

    let (plan, applied) = sa::apply(&pool, &program, "test", "test").await;
    assert!(
        plan.expect("plans").refusals[0].starts_with("external_missing on `patrons`"),
        "named by class"
    );
    assert!(matches!(applied, Err(ApplyRefusal::Plan)), "{applied:?}");
}

/// A tenant's own database that has not got the table: the same program
/// applied there is refused, and the deployment's own database is fine.
#[tokio::test]
async fn a_database_without_the_table_refuses_and_its_neighbour_does_not() {
    let (f, _pg, pool, program) = setup("ordo_extd_home").await;
    let (_f2, pg2, pool2, _) = setup("ordo_extd_tenant").await;
    pg2.exec("DROP SCHEMA crm CASCADE").expect("no table here");
    assert!(problems(&pool, &f).await.is_empty());
    let (plan, applied) = sa::apply(&pool2, &program, "test", "test").await;
    assert_eq!(
        plan.expect("plans").refusals.len(),
        1,
        "the tenant's database is refused"
    );
    assert!(matches!(applied, Err(ApplyRefusal::Plan)));
    let (_, applied) = sa::apply(&pool, &program, "test", "test").await;
    assert!(applied.is_ok(), "{applied:?}");
}

/// A first apply over a missing table is refused too: no schema is created
/// for a program that reads a table that is not there.
#[tokio::test]
async fn a_first_apply_over_a_missing_table_creates_nothing() {
    let pg = Pg::named("ordo_extd_first");
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public; DROP SCHEMA IF EXISTS crm CASCADE;")
        .expect("empty");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    let program = Program::compile(&source()).expect("compiles");
    let (_plan, applied) = sa::apply(&pool, &program, "test", "test").await;
    assert!(matches!(applied, Err(ApplyRefusal::Plan)), "{applied:?}");
    let tables = pg
        .query("SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND relkind = 'r' AND relname NOT LIKE '$%'")
        .expect("catalog");
    assert_eq!(tables[0][0].as_deref(), Some("0"), "nothing was created");
}

/// A role that cannot read the table is refused, and the refusal prints the
/// `GRANT` that fixes it. The pool dials as that role.
#[tokio::test]
async fn a_role_with_no_select_refuses_external_no_grant_naming_the_grant() {
    let (f, pg, _pool, _program) = setup("ordo_extd_grant").await;
    let role = "ordo_extd_reader";
    pg.exec(&format!(
        "DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{role}') THEN \
           CREATE ROLE {role} LOGIN PASSWORD 'reader'; END IF; END $$; \
         ALTER ROLE {role} PASSWORD 'reader'; \
         GRANT ALL ON SCHEMA public TO {role}; \
         GRANT USAGE ON SCHEMA crm TO {role}; \
         GRANT SELECT ON crm.customers TO {role};"
    ))
    .expect("a reader role");
    let reader = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        user: role.into(),
        password: Some("reader".into()),
        ..PoolConfig::from_env()
    })
    .await
    .expect("the reader dials");
    assert!(problems(&reader, &f).await.is_empty(), "granted");

    pg.exec(&format!("REVOKE SELECT ON crm.customers FROM {role}"))
        .expect("revokes");
    let found = problems(&reader, &f).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, ProblemKind::NoGrant);
    assert!(
        found[0].1.contains(&format!(
            "GRANT SELECT ON \"crm\".\"customers\" TO \"{role}\";"
        )),
        "the GRANT is named: {found:?}"
    );
    assert!(!found[0].1.contains("GRANT USAGE"), "{found:?}");

    pg.exec(&format!("REVOKE USAGE ON SCHEMA crm FROM {role}"))
        .expect("revokes the schema too");
    let found = problems(&reader, &f).await;
    assert!(
        found[0].1.contains("GRANT USAGE ON SCHEMA \"crm\""),
        "{found:?}"
    );
}

// ── the watcher ──────────────────────────────────────────────────────────

fn gated(state: &mut ordo_api::ApiState, f: &Arc<Fixture>) -> Arc<ExternalGate> {
    gated_over(state, Arc::new(std::sync::Mutex::new(f.clone())))
}

/// [`gated`], over a fixture the test can swap, as an apply does.
fn gated_over(
    state: &mut ordo_api::ApiState,
    served: Arc<std::sync::Mutex<Arc<Fixture>>>,
) -> Arc<ExternalGate> {
    let gate = ExternalGate::new(Arc::new(move || served.lock().expect("lock").clone()));
    state.executor = Arc::new(ExternalGated::new(state.executor.clone(), gate.clone()));
    gate
}

#[tokio::test]
async fn the_watcher_holds_an_entity_and_lifts_it_when_the_table_is_back() {
    let f = fixture();
    let (mut state, f, pool) =
        door_over(f, "ordo_extd_watch", 50, PoolConfig::from_env(), |k| k).await;
    let pg = Pg::named("ordo_extd_watch");
    let gate = gated(&mut state, &f);

    // As served: the entity answers.
    assert!(gate.watch_once(&pool).await.expect("reads").is_empty());
    let (status, body) = call(&state, "GET", "/op/std.patrons.list", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The other system drops a mapped column. Before the next pass the door
    // does not know; the pass finds it and holds the entity.
    pg.exec("ALTER TABLE crm.customers DROP COLUMN full_name")
        .expect("drops");
    let lines = gate.watch_once(&pool).await.expect("reads");
    assert_eq!(lines.len(), 1, "{lines:?}");
    for op in [
        "std.patrons.list",
        "std.patrons.get?id=pat_9",
        "patronByEmail?e=x",
    ] {
        let (status, body) = call(&state, "GET", &format!("/op/{op}"), None).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{op}: {body}");
        let text = body.to_string();
        assert!(text.contains("external_changed"), "{op}: {text}");
        assert!(text.contains("full_name"), "the column is named: {text}");
        assert!(
            text.contains("ordo introspect --external crm.customers"),
            "{text}"
        );
    }
    // A read the generic bind cannot settle is held too: unknown is held.
    let (status, body) = call(&state, "GET", "/op/mine", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    // An op that does not read the entity answers.
    let (status, body) = call(&state, "GET", "/op/std.tickets.list", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &state,
        "POST",
        "/op/std.tickets.create",
        Some(serde_json::json!({ "customer": "pat_9", "subject": "w" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a write that only names a ref: {body}"
    );

    // The table comes back as the program says: the next pass serves again.
    pg.exec("ALTER TABLE crm.customers ADD COLUMN full_name text NOT NULL DEFAULT 'back'")
        .expect("restores");
    assert!(gate.watch_once(&pool).await.expect("reads").is_empty());
    let (status, body) = call(&state, "GET", "/op/std.patrons.list", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A missing table holds the same way.
    pg.exec("ALTER TABLE crm.customers RENAME TO gone")
        .expect("renames");
    assert_eq!(gate.watch_once(&pool).await.expect("reads").len(), 1);
    let (status, body) = call(&state, "GET", "/op/std.patrons.list", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(body.to_string().contains("not in this database"), "{body}");
    pg.exec("ALTER TABLE crm.gone RENAME TO customers")
        .expect("renames back");
    assert!(gate.watch_once(&pool).await.expect("reads").is_empty());
    let (status, _) = call(&state, "GET", "/op/std.patrons.list", None).await;
    assert_eq!(status, StatusCode::OK);
}

/// A column added is not a hold: the watcher takes it.
#[tokio::test]
async fn the_watcher_does_not_hold_for_an_added_column() {
    let f = fixture();
    let (mut state, f, pool) =
        door_over(f, "ordo_extd_watch_add", 50, PoolConfig::from_env(), |k| k).await;
    let gate = gated(&mut state, &f);
    Pg::named("ordo_extd_watch_add")
        .exec("ALTER TABLE crm.customers ADD COLUMN phone text")
        .expect("adds");
    assert!(gate.watch_once(&pool).await.expect("reads").is_empty());
    let (status, body) = call(&state, "GET", "/op/std.patrons.list", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[test]
fn the_interval_setting_is_checked_by_name() {
    // The default and the refusal are the pins reconcile's shape.
    std::env::remove_var(external_check::WATCH_ENV);
    assert_eq!(
        external_check::watch_interval_from_env().expect("default"),
        std::time::Duration::from_secs(300)
    );
}

/// **The timer reads each tenant's own database** (step F3,
/// `plan-evolution-federation.md` §4). A foreign table kept per tenant
/// database changes in one tenant's database: the pass over the tenants
/// holds that tenant's ops that read it, naming the column, and no other
/// tenant's. The deployment's own database is unchanged, so its pass holds
/// nothing. The table restored, the next pass lifts the hold.
///
/// A regression looks like a tenant reading a foreign table its database no
/// longer holds as the file says, or one tenant's drift holding another.
#[tokio::test]
async fn the_timer_reads_each_tenant_database_and_holds_that_tenant_only() {
    let (f, _pg, home, _) = setup("ordo_extd_f3_home").await;
    let (_, pg_a, tenant_a, _) = setup("ordo_extd_f3_a").await;
    let (_, _pg_b, tenant_b, _) = setup("ordo_extd_f3_b").await;
    let served = f.clone();
    let gate = ExternalGate::new(Arc::new(move || served.clone()));
    let tenants = vec![
        ("ten_a".to_string(), tenant_a),
        ("ten_b".to_string(), tenant_b),
    ];
    assert!(gate.watch_once(&home).await.expect("reads").is_empty());
    assert!(gate.watch_tenants(&tenants).await.is_empty());
    assert!(gate.refusal_in("patronByEmail", "ten_a").is_none());

    pg_a.exec("ALTER TABLE crm.customers DROP COLUMN full_name")
        .expect("drops in one tenant's database");
    assert!(gate.watch_once(&home).await.expect("reads").is_empty());
    let lines = gate.watch_tenants(&tenants).await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("tenant ten_a: external_changed"),
        "{lines:?}"
    );
    let held = gate
        .refusal_in("patronByEmail", "ten_a")
        .expect("ten_a is held");
    assert!(format!("{held:?}").contains("full_name"), "{held:?}");
    assert!(gate.refusal_in("patronByEmail", "ten_b").is_none());
    assert!(gate.refusal_in("patronByEmail", "system").is_none());

    pg_a.exec("ALTER TABLE crm.customers ADD COLUMN full_name text NOT NULL DEFAULT 'back'")
        .expect("restores");
    assert!(gate.watch_tenants(&tenants).await.is_empty());
    assert!(gate.refusal_in("patronByEmail", "ten_a").is_none());
}
