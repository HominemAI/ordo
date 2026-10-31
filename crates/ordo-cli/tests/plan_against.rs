//! `ordo plan --against <db>` — the report, and its exit code.
//!
//! Increment 1's done-when clause is *"an operator can point `ordo plan
//! --against` at a live database with a changed `.ordo` and get a truthful,
//! classified, probed report"*. Truthful is the load-bearing word, and it
//! has an exit code attached: §5 says a non-empty probe is a **refusal, not
//! a warning**, and the module doc of `ordo_cli::migrate` says why the
//! refusal must reach the shell — *"a cron that reads `!= 0` must not be
//! able to gate a deploy on a report it did not read."*
//!
//! So every case here asserts both halves: what the operator reads, and what
//! the shell gets.
//!
//! The deployment is built the way a real one is — DDL applied, artifact
//! stored, version activated — because the report's baseline is the SERVING
//! artifact and half of what it can say depends on there being one. A test
//! that skipped the registry would be testing the no-prior-artifact path and
//! calling it the ordinary one.
//!
//! Databases are prefixed `ordo_mig_`, one per test function: these open
//! with `DROP SCHEMA public CASCADE`.

use ordo_cli::migrate::plan_against_database;
use ordo_cli::{load_sources, Sources};
use ordo_commandlog::Timestamp;
use ordo_conformance::Pg;
use ordo_exec::registry::{self, Artifact};
use ordo_exec::{Pool, PoolConfig};
use ordo_plan::artifact::{program_artifact, program_version, to_bytes};

/// What is deployed in every case below: two entities, some rows, and one op
/// that reads `a.name` — the last so a drop has something to collide with.
const DEPLOYED: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
  memo:  text?
}

op listA unbounded =
  a[name, n]
"#;

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("p.ordo".into(), text.to_string())])
}

/// Build the deployment: the generated DDL applied, the artifact stored, the
/// version activated.
async fn deployment(db: &str, seed: &str) -> Pool {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let (schema, registry_) = load_sources(&one(DEPLOYED)).expect("compiles");
    let doc = program_artifact(&schema, &registry_).expect("artifact");
    let ddl = ordo_schema::ddl_for(&schema).expect("ddl");
    pg.exec(&ddl).expect("the generated DDL applies");
    if !seed.is_empty() {
        pg.exec(seed).expect("seed");
    }

    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    registry::ensure_tables(&pool).await.expect("registry");
    let version = program_version(&doc, &ddl);
    registry::store(
        &pool,
        &Artifact {
            version: version.clone(),
            artifact: to_bytes(&doc),
            source: DEPLOYED.to_string(),
            ddl,
            applied_at: Timestamp(1),
            actor: "test".into(),
        },
    )
    .await
    .expect("stored");
    registry::activate(&pool, &version, "test", Timestamp(1))
        .await
        .expect("activated");
    pool
}

// ── the refusal ──────────────────────────────────────────────────────────

/// §5's rule for all of C, end to end: *the declaration is not the question;
/// the live data is.*
///
/// The program declares `name` unique. The database holds two rows sharing
/// `'dup'` and two more holding null — and under DL's `null == null` the
/// second null is a duplicate too, which is the half operators do not
/// expect. Nothing about the program says any of this; only the rows do.
#[tokio::test(flavor = "multi_thread")]
async fn a_probe_that_finds_a_violation_refuses_and_exits_nonzero() {
    let pool = deployment(
        "ordo_mig_plan_dirty",
        "INSERT INTO a (\"$space\", id, rev, name, n) VALUES \
           ('s1','a_1',1,'dup',1), ('s1','a_2',1,'dup',2), \
           ('s1','a_3',1,NULL, 3), ('s1','a_4',1,NULL, 4);",
    )
    .await;
    drop(pool);

    let out = plan_against_database(
        &one(r#"
entity b {
  title: text
}

entity a {
  name:  text?  unique
  n:     int
  memo:  text?
}

op listA unbounded =
  a[name, n]
"#),
        "p.ordo",
        "ordo_mig_plan_dirty",
    )
    .await;

    assert_eq!(
        out.code, 1,
        "a non-empty probe is a refusal, and the refusal has to reach the \
         shell. Report was:\n{}",
        out.report
    );
    assert!(
        out.report.contains("C1 ") && out.report.contains("becomes unique"),
        "the change should be classified and named:\n{}",
        out.report
    );
    assert!(
        out.report.contains("probe_not_empty"),
        "the refusal class should be greppable:\n{}",
        out.report
    );
    assert!(
        out.report.contains("dup"),
        "the offending value should be shown, because recognising it is how \
         an operator decides what to do:\n{}",
        out.report
    );
    assert!(
        out.report.contains("NULLS NOT"),
        "§5's C1 note asks for the null rule in these words, because a table \
         with nulls in a column cannot take a `unique` that every other SQL \
         system would accept:\n{}",
        out.report
    );
    assert!(
        !out.report.contains("PLANNABLE"),
        "no appliable plan may be printed when a probe came back non-empty:\n{}",
        out.report
    );
    assert!(
        out.report
            .contains("Every\n  tenant has a database of its own"),
        "§6's banner is unconditional — an operator who reads it after the \
         change list has already sized the outage wrong:\n{}",
        out.report
    );
}

// ── the plannable case ───────────────────────────────────────────────────

/// The same shape of change over a database that satisfies it, plus the two
/// free classes beside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_database_gets_a_classified_plan_and_exits_zero() {
    let pool = deployment(
        "ordo_mig_plan_clean",
        "INSERT INTO a (\"$space\", id, rev, name, n) VALUES \
           ('s1','a_1',1,'one',1), ('s1','a_2',1,'two',2), \
           ('s2','a_3',1,'one',3);",
    )
    .await;
    drop(pool);

    let out = plan_against_database(
        &one(r#"
entity b {
  title: text
}

entity a {
  name:  text?  unique
  n:     int
  memo:  text?
  tag:   text?
  index (name, n)
}

op listA unbounded =
  a[name, n]
"#),
        "p.ordo",
        "ordo_mig_plan_clean",
    )
    .await;

    assert_eq!(out.code, 0, "every probe is clean:\n{}", out.report);
    assert!(
        out.report.contains("PLANNABLE"),
        "a clean plan is appliable and must say so:\n{}",
        out.report
    );
    for expected in [
        "A2 ", // the new optional field
        "A4 ", // the declared index
        "C1 ", // the unique, probed and clean
        "class A · auto-applies",
        "class C · probe first",
        "clean",
    ] {
        assert!(
            out.report.contains(expected),
            "the report should carry `{expected}`:\n{}",
            out.report
        );
    }
    // `'one'` appears in two spaces and that is not a collision: the space
    // leads every generated unique index without exception. A probe that
    // forgot it would refuse this deploy.
    assert!(
        !out.report.contains("probe_not_empty"),
        "one row per space is not a duplicate:\n{}",
        out.report
    );
    assert!(
        out.report.contains("which holds 2 space(s)"),
        "§6: the plan says how many spaces the database it was read against holds:\n{}",
        out.report
    );
    assert!(
        !out.report.contains("EVERY tenant") && !out.report.contains("There is no per-space"),
        "the one-set-of-tables claim is gone; every tenant has its own database:\n{}",
        out.report
    );
    for expected in [
        "Every\n  tenant has a database of its own",
        "tenant catch-up",
        "Not built: a transforming (class B) migration that walks every tenant",
        "Other tenants are not\n  frozen",
    ] {
        assert!(
            out.report.contains(expected),
            "the TENANCY block should say `{expected}`:\n{}",
            out.report
        );
    }
}

// ── the drop, and the deploy-order violation it can be ───────────────────

/// §5 D3: dropping a field that an op in the **currently active** artifact
/// reads.
///
/// The gate would catch this afterwards, as drift on the serving artifact's
/// next load. Catching it here is the point of having a plan: *"a deploy
/// order violation… catching it at plan time is the point of a plan."*
#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_field_the_serving_artifact_reads_is_refused_by_name() {
    let pool = deployment("ordo_mig_plan_drop", "").await;
    drop(pool);

    let out = plan_against_database(
        &one(r#"
entity b {
  title: text
}

entity a {
  n:     int
  memo:  text?
}

op listA unbounded =
  a[n]
"#),
        "p.ordo",
        "ordo_mig_plan_drop",
    )
    .await;

    assert_eq!(
        out.code, 1,
        "a deploy-order violation refuses:\n{}",
        out.report
    );
    assert!(
        out.report.contains("D1 ") && out.report.contains("field `a.name` dropped"),
        "the drop should be classified D1 and named:\n{}",
        out.report
    );
    assert!(
        out.report.contains("drop_read_by_active_op"),
        "the refusal class should be greppable:\n{}",
        out.report
    );
    assert!(
        out.report.contains("listA"),
        "the ops still reading it must be named, or the operator cannot fix \
         it:\n{}",
        out.report
    );
    assert!(
        out.report.contains("class D · destructive"),
        "a drop is its own deploy and its own confirmation:\n{}",
        out.report
    );
    assert!(
        out.report.contains("undo "),
        "§7.2: a drop truncates the undo horizon for that entity at the \
         instant the column goes, and the disclosure is what was missing:\n{}",
        out.report
    );
}

// ── nothing to apply is not nothing to say ───────────────────────────────

/// A deploy that changes no schema still costs something, and the report has
/// to say so on that path too.
///
/// §5's A-note: an op's manifest is field-grain, and every capability grant
/// pins the combined hash of the manifests it consented to — so changing an
/// op body with no schema change kills grants and applies no DDL. The
/// GRANTS section is exactly the part of the report that matters here, and
/// it used to be skipped by an early return that printed "nothing to apply"
/// over a re-consent campaign.
#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_with_no_schema_change_still_reports_what_it_costs() {
    let pool = deployment("ordo_mig_plan_ops", "").await;
    drop(pool);

    let out = plan_against_database(
        &one(r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
  memo:  text?
}

op listA unbounded =
  a[name, n, memo]
"#),
        "p.ordo",
        "ordo_mig_plan_ops",
    )
    .await;

    assert_eq!(out.code, 0, "nothing is blocked:\n{}", out.report);
    assert!(
        out.report.contains("CHANGES  none"),
        "the schema did not move:\n{}",
        out.report
    );
    assert!(
        out.report.contains("GRANTS"),
        "the GRANTS section must print on this path — it is the only thing \
         this deploy costs:\n{}",
        out.report
    );
    assert!(
        out.report.contains("listA"),
        "`listA` now projects `memo`, so its manifest moved and every grant \
         pinning it dies:\n{}",
        out.report
    );
}

/// The report is read-only. Asserted because it is the property that makes
/// the command safe to point at production, and because it is the property
/// most easily lost by accident — one `CREATE TEMP TABLE` in a probe and it
/// is gone.
#[tokio::test(flavor = "multi_thread")]
async fn the_report_writes_nothing() {
    let db = "ordo_mig_plan_ro";
    let pool = deployment(
        db,
        "INSERT INTO a (\"$space\", id, rev, n) VALUES ('s1','a_1',1,1);",
    )
    .await;
    drop(pool);
    let pg = Pg::named(db);

    let fingerprint = || {
        pg.query(
            "SELECT string_agg(c.relname || ':' || c.relkind::text, ',' ORDER BY c.relname) \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public'",
        )
        .expect("catalog reads")
    };
    let counts = || {
        pg.query(
            "SELECT (SELECT count(*) FROM a) || '/' || (SELECT count(*) FROM \"$artifacts\") \
             || '/' || (SELECT count(*) FROM \"$activations\")",
        )
        .expect("counts read")
    };
    let before = (fingerprint(), counts());

    // The busiest possible report: every class, and a refusal.
    let out = plan_against_database(
        &one(r#"
entity b {
  title: text
}

entity a {
  name:  text   unique
  n:     int
  tag:   text?
  index (name, n)
}

op listA unbounded =
  a[name, n]
"#),
        "p.ordo",
        db,
    )
    .await;
    assert_eq!(
        out.code, 1,
        "this one refuses; the point is what it did not do"
    );

    assert_eq!(
        before,
        (fingerprint(), counts()),
        "`ordo plan --against` creates no table, applies no DDL, and stores no \
         artifact. It asserts what it needs by reading, never by writing a \
         marker first."
    );
}
