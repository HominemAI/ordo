//! 25aa: a change of places on a stored money or decimal field is refused
//! unless the change says whether to rescale the stored amounts (ruled
//! 2026-10-05, board `money-places-change`).
//!
//! Every `money(p)` is `bigint` minor units, so `money(0)` to `money(2)`
//! changed no column type, and the diff and the shape gate saw nothing.
//! Stored amounts then read 100 times smaller. A regression looks like any
//! test below finding the amount moved without a stated choice, or a
//! stated rescale leaving 29.99 reading as anything else.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::{self as sa, Applied, ApplyRefusal, Choices, Program, SchemaState};
use ordo_schema::diff::Change;

fn src(total: &str) -> String {
    format!(
        "entity invoices {{\n  label: text\n  total: {total}\n}}\n\n\
         op listInvoices unbounded =\n  invoices[label, total]\n"
    )
}

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.into(),
        size: 2,
        ..PoolConfig::from_env()
    }
}

async fn empty_db(name: &str) -> (Pg, Pool) {
    let pg = Pg::scoped(name);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(cfg(&pg.db)).await.expect("pool boots");
    (pg, pool)
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

/// Store and activate a version, as `ordo apply` does once the schema
/// matches, so the next plan has a serving artifact to compare against.
async fn activate(pool: &Pool, p: &Program) {
    ordo_exec::registry::ensure_tables(pool)
        .await
        .expect("registry");
    ordo_exec::registry::store(
        pool,
        &ordo_exec::registry::Artifact {
            version: p.version.clone(),
            artifact: ordo_plan::artifact::to_bytes(&p.doc),
            source: p.source.clone(),
            ddl: p.ddl.clone(),
            applied_at: ordo_commandlog::Timestamp(0),
            actor: "test".into(),
        },
    )
    .await
    .expect("stored");
    ordo_exec::registry::activate(pool, &p.version, "test", ordo_commandlog::Timestamp(0))
        .await
        .expect("activated");
}

/// A deployment serving `from`, with rows holding `amounts`.
async fn serving(name: &str, from: &str, amounts: &[&str]) -> (Pg, Pool, Program) {
    let (pg, pool) = empty_db(name).await;
    let v1 = Program::compile(&src(from)).expect("v1 compiles");
    let (_, created) = sa::apply(&pool, &v1, "test", "test").await;
    assert!(matches!(created, Ok(Applied::Created(_))), "{created:?}");
    activate(&pool, &v1).await;
    for (i, a) in amounts.iter().enumerate() {
        pg.exec(&format!(
            "INSERT INTO invoices (\"$space\", id, rev, label, total) \
             VALUES ('s1', 'inv_{i}', 1, 'i{i}', {a})"
        ))
        .expect("a row");
    }
    (pg, pool, v1)
}

fn stated(spec: &str) -> Choices {
    Choices::parse_places(spec).expect("a choice parses")
}

fn total(pg: &Pg, id: &str) -> String {
    one(
        pg,
        &format!("SELECT total::text FROM invoices WHERE id = '{id}'"),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn money_0_to_2_unstated_is_refused_naming_the_field() {
    let (pg, pool, _) = serving("ordo_places_unstated", "money(0)", &["2999"]).await;
    let v2 = Program::compile(&src("money(2)")).expect("v2 compiles");

    // The activation gate (`ordo apply`) sees it: the serving version reads
    // the amounts at 0 places, so this version is behind until they move.
    match sa::schema_state(&pool, &v2.schema).await.expect("state") {
        SchemaState::Behind(changes) => assert!(
            changes.iter().any(|c| matches!(
                c,
                Change::ChangePlaces { field, from: 0, to: 2, money: true, .. } if field == "total"
            )),
            "{changes:?}"
        ),
        other => panic!("a change of places blocks the activation: {other:?}"),
    }

    let (plan, outcome) = sa::apply(&pool, &v2, "test", "test").await;
    let plan = plan.expect("the plan reads");
    assert!(
        matches!(outcome, Err(ApplyRefusal::Plan)),
        "refused by the plan: {outcome:?}"
    );
    assert!(
        plan.refusals
            .iter()
            .any(|r| r == "places_choice_required on `invoices`"),
        "{:?}",
        plan.refusals
    );
    let why = plan
        .assessed
        .iter()
        .flat_map(|a| a.refused.iter())
        .find(|r| r.starts_with("places_choice_required"))
        .expect("the refusal");
    for needle in [
        "`invoices.total`",
        "money(0)",
        "money(2)",
        "--places invoices.total=rescale",
        "--places invoices.total=keep",
        "multiplies every stored amount by 100",
    ] {
        assert!(why.contains(needle), "`{needle}` missing from: {why}");
    }
    assert_eq!(total(&pg, "inv_0"), "2999", "nothing was sent");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stated_rescale_keeps_29_99_as_29_99() {
    let (pg, pool, v1) = serving("ordo_places_rescale", "money(2)", &["2999", "5"]).await;
    let v2 = Program::compile(&src("money(4)")).expect("v2 compiles");

    let (plan, outcome) = sa::apply_with(
        &pool,
        &v2,
        "test",
        "test",
        &stated("invoices.total=rescale"),
    )
    .await;
    let plan = plan.expect("the plan reads");
    let Ok(Applied::Additive { steps, .. }) = outcome else {
        panic!(
            "a stated rescale applies ({:?}): {outcome:?}",
            plan.refusals
        );
    };
    assert!(
        steps
            .iter()
            .any(|s| s.code == "B11" && s.sql.contains("* 100")),
        "{steps:#?}"
    );
    // 29.99 at 2 places was 2999; at 4 places it is 299900, still 29.99.
    assert_eq!(total(&pg, "inv_0"), "299900");
    assert_eq!(total(&pg, "inv_1"), "500");
    // Every rewritten row moves `rev`, so a device copy refetches it.
    assert_eq!(
        one(
            &pg,
            "SELECT string_agg(rev::text, ',' ORDER BY id) FROM invoices"
        ),
        "2,2"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT (total::numeric / 10000)::numeric(20, 4)::text FROM invoices WHERE id = 'inv_0'"
        ),
        "29.9900"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT choice || ' ' || places FROM \"$places\" ORDER BY seq DESC LIMIT 1"
        ),
        "rescale 4"
    );
    assert!(
        one(
            &pg,
            "SELECT changes FROM \"$migrations\" ORDER BY seq DESC LIMIT 1"
        )
        .contains("stated: rescale"),
        "the record carries the choice"
    );

    // Applied once: the next plan sees the amounts at 4 places and proposes
    // nothing, so a second run can never rescale them twice.
    let again = sa::plan(&pool, &v2).await.expect("the plan reads");
    assert!(
        !again
            .changes()
            .any(|c| matches!(c, Change::ChangePlaces { .. })),
        "{:?}",
        again.changes().collect::<Vec<_>>()
    );
    assert!(
        matches!(
            sa::schema_state(&pool, &v2.schema).await.expect("state"),
            SchemaState::Matches { .. }
        ),
        "the new version may now be activated"
    );

    // And the load gate refuses the old version over the moved amounts.
    let held = ordo_exec::migrate::held_places(&pool).await.expect("held");
    let drift = ordo_host::deploy::places_drift(&v1.schema, &held);
    assert_eq!(drift.len(), 1, "{drift:?}");
    assert!(
        drift[0].detail.contains("money(2)") && drift[0].detail.contains("money(4)"),
        "{}",
        drift[0]
    );
    assert!(ordo_host::deploy::places_drift(&v2.schema, &held).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stated_keep_is_applied_as_written() {
    let (pg, pool, _) = serving("ordo_places_keep", "money(0)", &["2999"]).await;
    let v2 = Program::compile(&src("money(2)")).expect("v2 compiles");

    let (plan, outcome) =
        sa::apply_with(&pool, &v2, "test", "test", &stated("invoices.total=keep")).await;
    let plan = plan.expect("the plan reads");
    assert!(
        matches!(outcome, Ok(Applied::Additive { .. })),
        "{:?}: {outcome:?}",
        plan.refusals
    );
    assert_eq!(
        total(&pg, "inv_0"),
        "2999",
        "the stored number is untouched"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT choice || ' ' || places FROM \"$places\" ORDER BY seq DESC LIMIT 1"
        ),
        "keep 2"
    );
    let again = sa::plan(&pool, &v2).await.expect("the plan reads");
    assert!(again.assessed.is_empty(), "{:?}", again.refusals);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rescale_that_would_cut_digits_is_refused_naming_the_rows() {
    let (pg, pool, _) = serving("ordo_places_down", "money(2)", &["2999", "3000"]).await;
    let v2 = Program::compile(&src("money(0)")).expect("v2 compiles");

    let (plan, outcome) = sa::apply_with(
        &pool,
        &v2,
        "test",
        "test",
        &stated("invoices.total=rescale"),
    )
    .await;
    let plan = plan.expect("the plan reads");
    assert!(matches!(outcome, Err(ApplyRefusal::Plan)), "{outcome:?}");
    let probe = plan
        .assessed
        .iter()
        .find_map(|a| a.probe.clone())
        .expect("a probe")
        .expect("it ran");
    let sa::Probe::Dirty { what, lines } = probe else {
        panic!("the probe names the rows: {probe:?}");
    };
    assert!(what.contains("dividing by 100"), "{what}");
    assert!(lines.iter().any(|l| l.contains("inv_0")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("inv_1")), "{lines:?}");
    assert_eq!(total(&pg, "inv_0"), "2999", "nothing was sent");

    // With every amount whole, it divides.
    pg.exec("UPDATE invoices SET total = 2900 WHERE id = 'inv_0'")
        .expect("fix the row");
    let (_, outcome) = sa::apply_with(
        &pool,
        &v2,
        "test",
        "test",
        &stated("invoices.total=rescale"),
    )
    .await;
    assert!(
        matches!(outcome, Ok(Applied::Additive { .. })),
        "{outcome:?}"
    );
    assert_eq!(total(&pg, "inv_0"), "29");
    assert_eq!(total(&pg, "inv_1"), "30");
}

fn scale(pg: &Pg) -> String {
    one(
        pg,
        "SELECT numeric_scale::text FROM information_schema.columns \
         WHERE table_name = 'invoices' AND column_name = 'total'",
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decimal_widens_keeping_every_value() {
    let (pg, pool, _) = serving("ordo_places_widen", "decimal(2)", &["29.99"]).await;
    let v2 = Program::compile(&src("decimal(4)")).expect("v2 compiles");

    // Unstated, refused like money.
    let (plan, outcome) = sa::apply(&pool, &v2, "test", "test").await;
    assert!(matches!(outcome, Err(ApplyRefusal::Plan)), "{outcome:?}");
    assert!(plan
        .expect("plan")
        .refusals
        .iter()
        .any(|r| r.starts_with("places_choice_required")),);

    let (_, outcome) = sa::apply_with(
        &pool,
        &v2,
        "test",
        "test",
        &stated("invoices.total=rescale"),
    )
    .await;
    assert!(
        matches!(outcome, Ok(Applied::Additive { .. })),
        "{outcome:?}"
    );
    assert_eq!(scale(&pg), "4");
    assert_eq!(total(&pg, "inv_0"), "29.9900");
    let again = sa::plan(&pool, &v2).await.expect("the plan reads");
    assert!(again.assessed.is_empty(), "{:?}", again.refusals);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decimal_narrows_only_when_every_value_fits() {
    let (pg, pool, _) = serving("ordo_places_narrow", "decimal(4)", &["1.2345", "2.5"]).await;
    let v2 = Program::compile(&src("decimal(2)")).expect("v2 compiles");

    let (plan, outcome) =
        sa::apply_with(&pool, &v2, "test", "test", &stated("invoices.total=keep")).await;
    assert!(matches!(outcome, Err(ApplyRefusal::Plan)), "{outcome:?}");
    let probe = plan
        .expect("plan")
        .assessed
        .iter()
        .find_map(|a| a.probe.clone())
        .expect("a probe")
        .expect("it ran");
    let sa::Probe::Dirty { what, lines } = probe else {
        panic!("the probe names the rows: {probe:?}");
    };
    assert!(what.contains("more than 2 places"), "{what}");
    assert!(lines.iter().any(|l| l.contains("inv_0")), "{lines:?}");
    assert_eq!(scale(&pg), "4", "nothing was sent");

    pg.exec("UPDATE invoices SET total = 1.23 WHERE id = 'inv_0'")
        .expect("fix the row");
    let (_, outcome) =
        sa::apply_with(&pool, &v2, "test", "test", &stated("invoices.total=keep")).await;
    assert!(
        matches!(outcome, Ok(Applied::Additive { .. })),
        "{outcome:?}"
    );
    assert_eq!(scale(&pg), "2");
    assert_eq!(total(&pg, "inv_0"), "1.23");
    assert_eq!(total(&pg, "inv_1"), "2.50");
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_changes_when_the_places_stay() {
    let (pg, pool, _) = serving("ordo_places_same", "money(2)", &["2999"]).await;
    let same = format!("{}entity notes {{\n  body: text\n}}\n", src("money(2)"));
    let v2 = Program::compile(&same).expect("v2 compiles");
    let (plan, outcome) = sa::apply(&pool, &v2, "test", "test").await;
    let plan = plan.expect("the plan reads");
    assert!(
        !plan
            .changes()
            .any(|c| matches!(c, Change::ChangePlaces { .. })),
        "{:?}",
        plan.changes().collect::<Vec<_>>()
    );
    assert!(
        matches!(outcome, Ok(Applied::Additive { .. })),
        "{outcome:?}"
    );
    assert_eq!(total(&pg, "inv_0"), "2999");
    assert_eq!(
        one(&pg, "SELECT to_regclass('\"$places\"') IS NULL"),
        "t",
        "no places change, no record of one"
    );
}

/// A tenant's database holds no registry, so before 25aa its money places
/// were invisible to the walk that brings it forward. It now reads the
/// version it was written under, and the stated choice, from the
/// deployment's own database.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_is_rescaled_the_way_the_deployment_stated() {
    let (_cpg, control, _) = serving("ordo_places_control", "money(2)", &["100"]).await;
    let (tpg, tenant) = empty_db("ordo_places_tenant").await;
    let v1 = Program::compile(&src("money(2)")).expect("v1 compiles");
    let (_, created) = sa::apply(&tenant, &v1, "test", "test").await;
    assert!(matches!(created, Ok(Applied::Created(_))), "{created:?}");
    tpg.exec(
        "INSERT INTO invoices (\"$space\", id, rev, label, total) \
         VALUES ('t1', 'inv_0', 1, 'a', 2999)",
    )
    .expect("a tenant row");
    let v2 = Program::compile(&src("money(4)")).expect("v2 compiles");

    // Before the deployment states anything, the tenant is refused.
    let refused = sa::bring_forward_from(&tenant, Some(&control), &v2, "test", "test").await;
    let Err(r) = refused else {
        panic!("an unstated change of places on a tenant is refused: {refused:?}");
    };
    assert!(r.why.contains("places_choice_required"), "{r}");
    assert_eq!(total(&tpg, "inv_0"), "2999");

    let (_, outcome) = sa::apply_with(
        &control,
        &v2,
        "test",
        "test",
        &stated("invoices.total=rescale"),
    )
    .await;
    assert!(
        matches!(outcome, Ok(Applied::Additive { .. })),
        "{outcome:?}"
    );

    let forward = sa::bring_forward_from(&tenant, Some(&control), &v2, "test", "test").await;
    assert!(forward.is_ok(), "{forward:?}");
    assert_eq!(total(&tpg, "inv_0"), "299900", "29.99 stays 29.99");
    assert_eq!(
        one(
            &tpg,
            "SELECT choice || ' ' || places FROM \"$places\" ORDER BY seq DESC LIMIT 1"
        ),
        "rescale 4"
    );
}
