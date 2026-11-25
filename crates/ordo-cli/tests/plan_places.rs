//! The plan report and `ordo evolve` with a change of places on a stored
//! money field (25aa). Unstated, the report names the field, both places
//! and both choices, and exits 1. Stated with `--places`, the report says
//! what was stated, and `ordo evolve --run` moves the amounts through the
//! door. (`ordo migrate --apply` did the last part until 2026-11-23.)
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

mod support;

use ordo_cli::migrate::{plan_against_database, plan_against_database_with};
use ordo_cli::Sources;
use ordo_commandlog::Timestamp;
use ordo_conformance::Pg;
use ordo_exec::registry::{self, Artifact};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::{Choices, Program};
use support::door::{door, runtime, serve, token, DEP_OWNER};
use support::operator;

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
    drop(pg);
}

/// The program written to its own directory, as a developer gives it.
fn program(db: &str, tag: &str, text: &str) -> String {
    let dir = std::env::temp_dir().join(format!("ordo-places-{db}-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("p.ordo");
    std::fs::write(&path, text).expect("write program");
    path.display().to_string()
}

/// `ordo evolve` through the door: the step that changes places is refused
/// until the choice is stated, and with `--places ... --run` the amounts
/// move.
#[test]
fn evolve_refuses_an_unstated_change_of_places_and_runs_a_stated_one() {
    let db = "ordo_mig_places_evolve";
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let v1 = program(db, "v1", &src("money(0)"));
    let v2 = program(db, "v2", &src("money(2)"));
    let first = operator::run(
        "apply",
        &[v1, "--db".into(), db.into(), "--as".into(), "test".into()],
    );
    assert_eq!(first.code, 0, "{}", first.report);
    pg.exec(
        "INSERT INTO invoices (\"$space\", id, rev, label, total) \
         VALUES ('s1', 'inv_1', 1, 'a', 30)",
    )
    .expect("a row");
    let staged = operator::run(
        "apply",
        &[
            v2,
            "--db".into(),
            db.into(),
            "--as".into(),
            "test".into(),
            "--no-activate".into(),
        ],
    );
    assert_eq!(staged.code, 0, "{}", staged.report);

    let from = Program::compile(&src("money(0)"))
        .expect("compiles")
        .version;
    let to = Program::compile(&src("money(2)"))
        .expect("compiles")
        .version;
    let rt = runtime();
    let addr = rt.block_on(serve(db));
    let evolve = |extra: &[&str]| {
        let mut args: Vec<String> = ["--from", from.as_str(), "--to", to.as_str()]
            .iter()
            .chain(extra.iter())
            .map(|a| a.to_string())
            .collect();
        args.extend(door(addr, &token(DEP_OWNER)));
        operator::run("evolve", &args)
    };

    let out = evolve(&[]);
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("invoices.total"), "{}", out.report);
    assert!(out.report.contains("--places"), "{}", out.report);

    let out = evolve(&["--places", "invoices.total=rescale", "--run"]);
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
