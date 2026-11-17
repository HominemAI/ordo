//! `ORDO_PG_SCHEMA`: each deployment's tables live in a Postgres schema of
//! their own, named by a deployment setting (owner ruling 2026-11-16,
//! `example-apps-notes-v2-plus-note_viewer` ask 1). Two deployments can then
//! share one database without colliding (27k).
//!
//! What it proves:
//!
//! - Two programs, each with an entity named `widgets`, apply into one
//!   database under two schemas. A write through one is not seen by the
//!   other, and each has its own `$` system tables.
//! - Drift detection reads each deployment's own schema against its own
//!   shadow, and a later schema apply changes only its own schema.
//! - Notifications and leadership stay per deployment.
//! - Unset, the schema is `public`, and a pool dials as it always did.
//! - An `external ... from crm.customers` resolves when the deployment's own
//!   schema is not `public`.
//!
//! A regression looks like a table landing in `public` under a named
//! schema, one deployment reading the other's rows, a shadow both share, or
//! an external that stops resolving.
//!
//! Needs the conformance cluster. Its own databases, prefixed
//! `ordo_dschema_`.

use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::external_check;
use ordo_host::schema_apply::{self as sa, Program};

const ALPHA: &str = r#"
entity widgets {
  name: text
}

op listWidgets unbounded =
  widgets[name]
"#;

/// Another program with an entity of the same name and another shape.
const BETA: &str = r#"
entity widgets {
  label: text
  n:     int
}

op listWidgets unbounded =
  widgets[label, n]
"#;

/// ALPHA plus an optional field: a safe additive change.
const ALPHA_2: &str = r#"
entity widgets {
  name: text
  note: text?
}

op listWidgets unbounded =
  widgets[name, note]
"#;

fn cfg(db: &str, schema: &str) -> PoolConfig {
    PoolConfig {
        db: db.into(),
        size: 3,
        schema: schema.into(),
        ..PoolConfig::from_env()
    }
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

async fn count(pool: &Pool, sql: &str) -> i64 {
    pool.rows(sql)
        .await
        .expect("count")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|v| v.parse().ok())
        .expect("a number")
}

async fn applied(pool: &Pool, source: &str) -> Program {
    let program = Program::compile(source).expect("compiles");
    let (plan, outcome) = sa::apply(pool, &program, "test", "test").await;
    let plan = plan.expect("plans");
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
    assert!(outcome.is_ok(), "{outcome:?}");
    program
}

async fn drift(pool: &Pool, program: &Program) -> Vec<String> {
    pool.build_shadow(&program.ddl)
        .await
        .expect("shadow builds");
    pool.live_drift(&pool.shadow(), pool.schema())
        .await
        .expect("drift reads")
        .iter()
        .map(|d| d.to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn two_deployments_share_one_database_under_two_schemas() {
    let pg = Pg::scoped("ordo_dschema_two");
    pg.exec(
        "DROP SCHEMA IF EXISTS alpha CASCADE; DROP SCHEMA IF EXISTS beta CASCADE; \
         DROP SCHEMA public CASCADE; CREATE SCHEMA public;",
    )
    .expect("reset");

    // The schema is created when the deployment first starts.
    let a = Pool::connect(cfg(&pg.db, "alpha"))
        .await
        .expect("alpha boots");
    let b = Pool::connect(cfg(&pg.db, "beta"))
        .await
        .expect("beta boots");
    assert_eq!(a.schema(), "alpha");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_namespace WHERE nspname IN ('alpha', 'beta')"
        ),
        "2"
    );
    // Each has its own `$` system tables, and `public` has none.
    for s in ["alpha", "beta"] {
        assert_eq!(
            one(
                &pg,
                &format!("SELECT to_regclass('{s}.\"$commandlog\"') IS NOT NULL")
            ),
            "t",
            "{s} has its own command log"
        );
    }
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'"
        ),
        "0",
        "nothing lands in public"
    );

    let pa = applied(&a, ALPHA).await;
    let pb = applied(&b, BETA).await;
    assert_eq!(
        one(&pg, "SELECT to_regclass('alpha.widgets') IS NOT NULL"),
        "t"
    );
    assert_eq!(
        one(&pg, "SELECT to_regclass('beta.widgets') IS NOT NULL"),
        "t"
    );
    assert_eq!(
        one(&pg, "SELECT to_regclass('public.widgets') IS NOT NULL"),
        "f"
    );

    // A write through one deployment is not seen by the other.
    a.rows(
        "INSERT INTO \"widgets\" (\"$space\", \"id\", \"rev\", \"name\") \
         VALUES ('s1', 'wid_1', 1, 'from alpha')",
    )
    .await
    .expect("alpha writes");
    assert_eq!(count(&a, "SELECT count(*) FROM \"widgets\"").await, 1);
    assert_eq!(count(&b, "SELECT count(*) FROM \"widgets\"").await, 0);
    assert_eq!(one(&pg, "SELECT count(*) FROM alpha.widgets"), "1");
    assert_eq!(one(&pg, "SELECT count(*) FROM beta.widgets"), "0");

    // Each deployment's drift is read against its own shadow, and the two
    // shadows are two schemas.
    assert_ne!(a.shadow(), b.shadow());
    assert!(drift(&a, &pa).await.is_empty());
    assert!(drift(&b, &pb).await.is_empty());
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_namespace \
             WHERE nspname IN ('ordo_shadow_of_alpha', 'ordo_shadow_of_beta')"
        ),
        "2"
    );
    pg.exec("ALTER TABLE beta.widgets DROP COLUMN label")
        .expect("beta drifts");
    let db = drift(&b, &pb).await;
    assert!(
        db.iter().any(|d| d.contains("`widgets`.`label`")),
        "beta's drift names its column: {db:?}"
    );
    assert!(drift(&a, &pa).await.is_empty(), "alpha is untouched");
    pg.exec("ALTER TABLE beta.widgets ADD COLUMN label text NOT NULL DEFAULT ''")
        .expect("beta repaired");

    // A schema apply changes only its own deployment.
    applied(&a, ALPHA_2).await;
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_name = 'widgets' AND column_name = 'note'"
        ),
        "1",
        "the new column is in alpha's widgets only"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT table_schema FROM information_schema.columns \
             WHERE table_name = 'widgets' AND column_name = 'note'"
        ),
        "alpha"
    );
    let state = sa::schema_state(&b, &Program::compile(BETA).unwrap().schema)
        .await
        .expect("beta's state reads");
    assert!(
        matches!(state, sa::SchemaState::Matches { .. }),
        "beta still matches its program: {state:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn notifications_and_leadership_stay_with_their_deployment() {
    let pg = Pg::scoped("ordo_dschema_notify");
    let a_cfg = cfg(&pg.db, "alpha");
    let b_cfg = cfg(&pg.db, "beta");
    let a = Pool::connect(a_cfg.clone()).await.expect("alpha boots");
    let b = Pool::connect(b_cfg.clone()).await.expect("beta boots");

    let ch = ordo_exec::jobs::INVALIDATE_CHANNEL;
    let (_listener, mut heard) = ordo_exec::jobs::listen(&a_cfg, &[ch])
        .await
        .expect("alpha listens");
    let tx = b.begin().await.expect("beta's transaction");
    tx.notify(ch, "fromBeta").await.expect("beta notifies");
    tx.commit().await.expect("commits");
    let tx = a.begin().await.expect("alpha's transaction");
    tx.notify(ch, "fromAlpha").await.expect("alpha notifies");
    tx.commit().await.expect("commits");
    let first = tokio::time::timeout(Duration::from_secs(5), heard.recv())
        .await
        .expect("a notification arrives")
        .expect("the listener is live");
    assert_eq!(first.payload, "fromAlpha", "beta's signal is not heard");
    assert_eq!(first.channel, ch, "handed on under the plain name");

    // Each deployment elects its own leader for the same realm.
    let la = ordo_exec::jobs::Leadership::acquire(&a_cfg, "supervisor")
        .await
        .expect("dials");
    let lb = ordo_exec::jobs::Leadership::acquire(&b_cfg, "supervisor")
        .await
        .expect("dials");
    assert!(la.is_some() && lb.is_some(), "two deployments, two leaders");
}

#[tokio::test(flavor = "multi_thread")]
async fn unset_the_schema_is_public() {
    assert_eq!(ordo_exec::DEFAULT_SCHEMA, "public");
    if std::env::var(ordo_exec::SCHEMA_ENV).is_err() {
        assert_eq!(PoolConfig::from_env().schema, "public");
    }
    let pg = Pg::scoped("ordo_dschema_default");
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(cfg(&pg.db, ordo_exec::DEFAULT_SCHEMA))
        .await
        .expect("boots");
    assert_eq!(pool.shadow(), ordo_exec::shape::SHADOW);
    let p = applied(&pool, ALPHA).await;
    assert_eq!(
        one(&pg, "SELECT to_regclass('public.widgets') IS NOT NULL"),
        "t"
    );
    assert!(drift(&pool, &p).await.is_empty());
    // The server's own search_path, untouched.
    assert_eq!(
        count(&pool, "SELECT count(*) FROM (SELECT 1 WHERE current_setting('search_path') = '\"$user\", public') t").await,
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_external_resolves_when_the_deployment_schema_is_not_public() {
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

op customerByEmail($e: text) =
  patrons(email == $e)[id, name]
"#;
    let pg = Pg::scoped("ordo_dschema_external");
    pg.exec(
        "DROP SCHEMA IF EXISTS crm CASCADE; DROP SCHEMA IF EXISTS gamma CASCADE; \
         DROP SCHEMA public CASCADE; CREATE SCHEMA public; CREATE SCHEMA crm; \
         CREATE TABLE crm.customers (customer_id bigint PRIMARY KEY, \
           full_name text NOT NULL, email text); \
         INSERT INTO crm.customers VALUES (9, 'Ada', 'ada@example.com');",
    )
    .expect("the foreign table");
    let c = cfg(&pg.db, "gamma");
    let pool = Pool::connect(c.clone()).await.expect("boots");
    let program = applied(&pool, PROGRAM).await;
    assert_eq!(
        one(&pg, "SELECT to_regclass('gamma.tickets') IS NOT NULL"),
        "t"
    );

    let report = external_check::check(&pool, &program.schema)
        .await
        .expect("the check reads");
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert!(
        !report.present.iter().any(|l| l.contains("gamma")),
        "the deployment's own schema is not foreign: {:?}",
        report.present
    );
    // The op's SQL resolves the foreign table under the gate.
    let f = Arc::new(Fixture::from_source(PROGRAM, "").expect("fixture"));
    let shape = ordo_host::shape::verify_registry(&f, &c, &program.ddl).await;
    assert!(shape.is_ok(), "{shape:?}");
    assert!(shape.unwrap().drift.is_empty());
}

#[tokio::test]
async fn a_refused_schema_name_names_the_setting() {
    for bad in [
        "",
        "Notes",
        "1notes",
        "no-tes",
        "no tes",
        "pg_notes",
        "information_schema",
        "ordo_shadow",
        "ordo_shadow_of_x",
        &"n".repeat(ordo_exec::SCHEMA_MAX_LEN + 1),
    ] {
        let err = ordo_exec::check_schema_name(bad).expect_err(bad);
        assert!(
            err.contains("ORDO_PG_SCHEMA") && err.contains("Set ORDO_PG_SCHEMA"),
            "{bad}: {err}"
        );
        // The pool refuses it too, before any dial.
        let refused = Pool::connect(cfg("ordo_never_dialled", bad)).await;
        let text = refused.err().expect("refused").to_string();
        assert!(text.contains("ORDO_PG_SCHEMA"), "{bad}: {text}");
    }
    ordo_exec::check_schema_name("notes_v2").expect("an ordinary name");
    ordo_exec::check_schema_name(&"n".repeat(ordo_exec::SCHEMA_MAX_LEN)).expect("at the limit");
}
