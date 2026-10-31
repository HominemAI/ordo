//! `ordo plan --against` and `ordo migrate --apply` with a change of places
//! on a stored money field (25aa). Unstated, the report names the field,
//! both places and both choices, and exits 1. Stated with `--places`, the
//! report says what was stated, and the apply moves the amounts.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_cli::migrate::{
    apply_against_database_with, plan_against_database, plan_against_database_with,
};
use ordo_cli::Sources;
use ordo_commandlog::Timestamp;
use ordo_conformance::Pg;
use ordo_exec::registry::{self, Artifact};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::{Choices, Program};

fn src(total: &str) -> String {
    format!(
        "entity invoices {{\n  label: text\n  total: {total}\n}}\n\n\
         op listInvoices unbounded =\n  invoices[label, total]\n"
    )
}

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("p.ordo".into(), text.to_string())])
}

async fn deployment(db: &str) -> Pg {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let v1 = Program::compile(&src("money(0)")).expect("compiles");
    pg.exec(&v1.ddl).expect("the generated DDL applies");
    pg.exec(
        "INSERT INTO invoices (\"$space\", id, rev, label, total) \
         VALUES ('s1', 'inv_1', 1, 'a', 30)",
    )
    .expect("a row");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    registry::ensure_tables(&pool).await.expect("registry");
    registry::store(
        &pool,
        &Artifact {
            version: v1.version.clone(),
            artifact: ordo_plan::artifact::to_bytes(&v1.doc),
            source: v1.source.clone(),
            ddl: v1.ddl.clone(),
            applied_at: Timestamp(1),
            actor: "test".into(),
        },
    )
    .await
    .expect("stored");
    registry::activate(&pool, &v1.version, "test", Timestamp(1))
        .await
        .expect("activated");
    pg
}

#[tokio::test(flavor = "multi_thread")]
async fn the_plan_refuses_an_unstated_change_of_places_and_takes_a_stated_one() {
    let pg = deployment("ordo_mig_places_cli").await;
    let v2 = one(&src("money(2)"));

    let out = plan_against_database(&v2, "p.ordo", "ordo_mig_places_cli").await;
    assert_eq!(out.code, 1, "{}", out.report);
    for needle in [
        "B11",
        "`invoices.total` changes places",
        "REFUSED  places_choice_required",
        "--places invoices.total=rescale",
        "--places invoices.total=keep",
    ] {
        assert!(
            out.report.contains(needle),
            "`{needle}` missing:\n{}",
            out.report
        );
    }

    let choices = Choices::parse_places("invoices.total=rescale").expect("parses");
    let out = plan_against_database_with(&v2, "p.ordo", "ordo_mig_places_cli", &choices).await;
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("stated   rescale"), "{}", out.report);

    let out =
        apply_against_database_with(&v2, "p.ordo", "ordo_mig_places_cli", "test", &choices).await;
    assert_eq!(out.code, 0, "{}", out.report);
    let total = pg
        .query("SELECT total::text FROM invoices WHERE id = 'inv_1'")
        .expect("query");
    assert_eq!(
        total[0][0].as_deref(),
        Some("3000"),
        "30 at money(0) is 3000 at money(2)"
    );
}

#[test]
fn a_places_choice_must_name_a_field_and_a_known_choice() {
    assert!(Choices::parse_places("invoices.total=rescale,fees.rate=keep").is_ok());
    for bad in ["invoices.total", "total=rescale", "invoices.total=round"] {
        assert!(Choices::parse_places(bad).is_err(), "`{bad}` parsed");
    }
}
