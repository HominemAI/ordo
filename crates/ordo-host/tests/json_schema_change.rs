//! 28b: a stored field changing between `text` and `json`.
//!
//! `text` to `json` applies in place when every stored value parses as a
//! document, and is refused with the probe's rows when one does not.
//! `json` to `text` always applies: a document becomes its JSON text. Both
//! move `rev`, so a client caching by `rev` refetches the new shape.
//!
//! A regression looks like a conversion that loses a row, a `text` value
//! that is not JSON reaching a failed cast, or `rev` left where it was.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_jsonchg_`.

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::{self as sa, Applied, ApplyRefusal, Program};

fn src(ty: &str) -> String {
    format!(
        "entity notes {{\n  label: text\n  body: {ty}?\n}}\n\n\
         op listNotes unbounded =\n  notes[label, body]\n"
    )
}

async fn serving(name: &str, ty: &str, bodies: &[&str]) -> (Pg, Pool) {
    let pg = Pg::scoped(name);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    let v1 = Program::compile(&src(ty)).expect("v1 compiles");
    let (_, created) = sa::apply(&pool, &v1, "test", "test").await;
    assert!(matches!(created, Ok(Applied::Created(_))), "{created:?}");
    for (i, b) in bodies.iter().enumerate() {
        pg.exec(&format!(
            "INSERT INTO notes (\"$space\", id, rev, label, body) \
             VALUES ('s1', 'note_{i}', 1, 'n{i}', {})",
            ordo_exec::sql_lit(b)
        ))
        .expect("a row");
    }
    (pg, pool)
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn text_to_json_applies_when_every_stored_value_is_a_document() {
    let (pg, pool) = serving("ordo_jsonchg_clean", "text", &[r#"{"a":1}"#, "[1, 2]", "3"]).await;
    let v2 = Program::compile(&src("json")).expect("v2 compiles");
    let (plan, outcome) = sa::apply(&pool, &v2, "test", "test").await;
    assert!(
        matches!(outcome, Ok(Applied::Additive { .. })),
        "{:?}: {outcome:?}",
        plan.map(|p| p.refusals)
    );
    assert_eq!(
        one(
            &pg,
            "SELECT data_type FROM information_schema.columns \
             WHERE table_name = 'notes' AND column_name = 'body'"
        ),
        "jsonb"
    );
    assert_eq!(
        one(&pg, "SELECT body->>'a' FROM notes WHERE id = 'note_0'"),
        "1"
    );
    assert_eq!(
        one(&pg, "SELECT rev FROM notes WHERE id = 'note_0'"),
        "2",
        "rev moves"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn text_to_json_is_refused_naming_the_rows_that_are_not_documents() {
    let (pg, pool) = serving("ordo_jsonchg_dirty", "text", &[r#"{"a":1}"#, "hello"]).await;
    let v2 = Program::compile(&src("json")).expect("v2 compiles");
    let (plan, outcome) = sa::apply(&pool, &v2, "test", "test").await;
    let plan = plan.expect("the plan reads");
    assert!(
        matches!(outcome, Err(ApplyRefusal::Plan)),
        "refused by the plan: {outcome:?}"
    );
    let shown = format!("{:?}", plan.assessed);
    assert!(shown.contains("not JSON"), "{shown}");
    assert!(shown.contains("note_1"), "the row is named: {shown}");
    assert_eq!(
        one(
            &pg,
            "SELECT data_type FROM information_schema.columns \
             WHERE table_name = 'notes' AND column_name = 'body'"
        ),
        "text",
        "nothing changed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn json_to_text_applies_and_keeps_each_document_as_its_text() {
    let (pg, pool) = serving("ordo_jsonchg_back", "json", &[r#"{"a": [1, 2]}"#]).await;
    let v2 = Program::compile(&src("text")).expect("v2 compiles");
    let (plan, outcome) = sa::apply(&pool, &v2, "test", "test").await;
    assert!(
        matches!(outcome, Ok(Applied::Additive { .. })),
        "{:?}: {outcome:?}",
        plan.map(|p| p.refusals)
    );
    assert_eq!(
        one(&pg, "SELECT body FROM notes WHERE id = 'note_0'"),
        r#"{"a": [1, 2]}"#
    );
    assert_eq!(one(&pg, "SELECT rev FROM notes WHERE id = 'note_0'"), "2");
}
