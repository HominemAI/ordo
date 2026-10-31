//! Checklist 9e — **`unique (a, b)` and `index (…)`**, executed.
//!
//! Both are `[settled]` in `definition-language.md` §3 ("Uniqueness
//! [settled]", "Indexes [settled]") and neither had a declaration form.
//! `unique (a, b)` is the unusual case where the *pin* arrived before the
//! *syntax*: the `acl` derivation minted `unique (<root>, member)` on its
//! link entity (10c), so `Entity::unique_keys` and every consumer of it —
//! the DDL, `SetMode`'s converge-or-create classifier, the write path's
//! `duplicate` refusal, `ordo check`'s shape summary — already existed
//! with exactly one producer that no author could reach. This suite proves
//! the author's spelling arrives at the same four places.
//!
//! # The three things a unit test cannot show
//!
//! 1. **`unique (a, b)` is an ADDRESS, not just a constraint.** Rule 8's
//!    identity precedence makes a `set` whose braces cover every column of
//!    a composite key a *converge-or-create* rather than a create. So the
//!    same op run twice must leave ONE row with the second call's values —
//!    and a partial cover (`{a}` alone) must still create, because it
//!    names no row.
//! 2. **`NULLS NOT DISTINCT`.** DL: the language has one equality,
//!    `null == null` is true in it, so "a nullable unique field admits **at
//!    most one null row**" and "Generated DDL is `UNIQUE NULLS NOT
//!    DISTINCT`". Postgres defaults to the opposite. The modifier was
//!    missing from every generated unique index, which is not a cosmetic
//!    gap: it is the difference between the constraint meaning what the
//!    language says and meaning what SQL says, and no test could see it
//!    because no test wrote two nulls.
//! 3. **`index (…)` reaches the catalog.** An index constrains nothing, so
//!    it can only be observed by asking Postgres what it built.
//!
//! Real Postgres, this suite's own database (`ordo_9ed_clauses`).

mod gc_support;

use gc_support::{class_of, db, ANA};
use ordo_conformance::{Fixture, Pg, WriteLog, FIXTURE, SEED};
use serde_json::json;

/// A cluster of its own so nothing here moves a number another suite
/// asserts about the shipped fixture.
///
/// `readings` is `syntax-examples.md`'s ingestion shape verbatim —
/// `unique (meter, at)`, described there as "ingestion identity" and the
/// reason the same reading arriving twice converges instead of doubling.
/// `tags` carries a NULLABLE unique column, which is the only shape that
/// can see `NULLS NOT DISTINCT`.
const CLUSTER: &str = r#"
entity meters (key: mtr) {
  kind: text
}

entity readings (key: rdng) {
  meter -> meters
  at:    date = 2026-01-01
  value: int
  unique (meter, at)
  index (meter, value)
}

entity tags (key: tg) {
  label: text
  slug:  text?  unique
}
"#;

const CLUSTER_OPS: &str = r#"
op ingestReading($meter: id, $at: date, $value: int) unguarded =
  set readings { meter: $meter, at: $at, value: $value }

op ingestValue($meter: id, $value: int) unguarded =
  set readings { meter: $meter, value: $value }

op addTag($label: text) unguarded =
  set tags { label: $label }
"#;

const CLUSTER_SEED: &str = r#"
INSERT INTO "meters" ("$space", "id", "kind") VALUES
  ('conformance', 'mtr_1', 'water'),
  ('conformance', 'mtr_2', 'power');
"#;

fn fixture() -> Fixture {
    let src = format!("{FIXTURE}\n{CLUSTER}\n{CLUSTER_OPS}");
    Fixture::from_source(&src, SEED).expect("the fixture plus the declared clauses compiles")
}

fn reseed(f: &Fixture, pg: &Pg) {
    f.reset(pg).expect("fresh seed");
    pg.exec(CLUSTER_SEED).expect("the meter rows seed");
}

/// Rule 8's identity precedence, reached through a **declared** key.
///
/// The braces cover both columns of `unique (meter, at)`, so the statement
/// is `creates-or-converges` and the second ingestion of the same reading
/// updates the row it already made. Without the declaration the same op is
/// a plain create and the second call mints a second row — which is the
/// bug `syntax-examples.md` calls "ingestion identity".
#[test]
fn a_declared_composite_key_addresses_a_row() {
    let f = fixture();
    let pg = db("ordo_9ed_clauses_key");
    reseed(&f, &pg);
    let mut log = WriteLog::new();

    // The static label is the compiler's own answer, read off the same
    // classifier the write path uses — so the behaviour below cannot drift
    // from what the artifact and the deploy diff say.
    let op = f.registry.get("ingestReading").expect("registered");
    let ordo_syntax::OpBody::Write(stmts) = &op.body else {
        panic!("a write op")
    };
    assert_eq!(
        ordo_plan::writes::stmt_label(&f.schema, &stmts[0]),
        "creates-or-converges",
        "covering every column of `unique (meter, at)` addresses"
    );

    let r = f
        .run_write_as(
            &pg,
            &mut log,
            r#"ingestReading($meter: "mtr_1", $at: 2026-03-01, $value: 10)"#,
            ANA,
        )
        .expect("first ingestion");
    assert_eq!(r, json!([{ "entity": "readings", "created": "rdng_w1" }]));

    let r = f
        .run_write_as(
            &pg,
            &mut log,
            r#"ingestReading($meter: "mtr_1", $at: 2026-03-01, $value: 11)"#,
            ANA,
        )
        .expect("the same reading again");
    assert_eq!(r, json!([{ "entity": "readings", "converged": "rdng_w1" }]));

    assert_eq!(
        pg.query(r#"SELECT count(*)::text, max("value")::text FROM "readings""#)
            .expect("count"),
        vec![vec![Some("1".into()), Some("11".into())]],
        "one row, converged onto the declared key"
    );

    // A different `at` is a different key: the pair is the identity, not
    // either column of it.
    f.run_write_as(
        &pg,
        &mut log,
        r#"ingestReading($meter: "mtr_1", $at: 2026-03-02, $value: 12)"#,
        ANA,
    )
    .expect("a different day");
    assert_eq!(
        pg.query(r#"SELECT count(*)::text FROM "readings""#)
            .expect("count"),
        vec![vec![Some("2".into())]]
    );

    // A PARTIAL cover addresses nothing. `{meter, value}` names no row —
    // "coverage is every column or none", the rule that stops a two-column
    // key converging onto whichever row happens to share one column — so
    // the statement is a plain create, and the compiler says so statically.
    let op = f.registry.get("ingestValue").expect("registered");
    let ordo_syntax::OpBody::Write(stmts) = &op.body else {
        panic!("a write op")
    };
    assert_eq!(
        ordo_plan::writes::stmt_label(&f.schema, &stmts[0]),
        "creates",
        "half of `unique (meter, at)` is not an address"
    );
    let r = f
        .run_write_as(
            &pg,
            &mut log,
            r#"ingestValue($meter: "mtr_2", $value: 1)"#,
            ANA,
        )
        .expect("a create");
    assert_eq!(r[0]["entity"], "readings");
    assert!(
        r[0].get("created").is_some(),
        "a create, not a converge: {r}"
    );
    // The constraint is real either way: the same partial-cover create
    // again claims the same (meter, at) pair, and the database refuses
    // it. A create the compiler did not read as an address is still a row
    // the unique index judges.
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"ingestValue($meter: "mtr_2", $value: 2)"#,
            ANA,
        )
        .expect_err("the key is taken");
    assert!(
        e.contains("duplicate key value") || class_of(&e) == "duplicate",
        "{e}"
    );
}

/// DL's one equality, in the database. `null == null` is TRUE in the
/// language, so a nullable unique field admits **at most one null row** —
/// "the deliberate departure from every SQL system", and the only rule in
/// the uniqueness section that a plain `UNIQUE` gets wrong.
///
/// Before `NULLS NOT DISTINCT` this passed silently in the wrong
/// direction: Postgres's default lets a nullable unique column hold
/// unlimited nulls, so `tags.slug` was not a unique claim at all for the
/// rows that left it out.
#[test]
fn a_nullable_unique_field_admits_one_null_row() {
    let f = fixture();
    let pg = db("ordo_9ed_clauses_null");
    reseed(&f, &pg);
    let mut log = WriteLog::new();

    f.run_write_as(&pg, &mut log, r#"addTag($label: "first")"#, ANA)
        .expect("the first slugless tag");
    let e = f
        .run_write_as(&pg, &mut log, r#"addTag($label: "second")"#, ANA)
        .expect_err("a second null slug is a second claim on the same key");
    assert!(
        e.contains("duplicate key value") || class_of(&e) == "duplicate",
        "the database refuses it rather than storing two: {e}"
    );
    assert_eq!(
        pg.query(r#"SELECT count(*)::text FROM "tags""#)
            .expect("count"),
        vec![vec![Some("1".into())]]
    );

    // The constraint says so itself, so a future DDL change that dropped
    // the modifier would fail here and not only in the behaviour above.
    assert!(
        f.ddl.contains(
            r#"ALTER TABLE "tags" ADD CONSTRAINT "tags$slug$uq" UNIQUE NULLS NOT DISTINCT ("$space", "slug");"#
        ),
        "{}",
        f.ddl
    );
}

/// `index (…)` constrains nothing, so the only honest assertion is that
/// Postgres built it, over the columns declared, with the space leading.
#[test]
fn a_declared_index_reaches_the_catalog() {
    let f = fixture();
    let pg = db("ordo_9ed_clauses_index");
    reseed(&f, &pg);

    let rows = pg
        .query(
            "SELECT indexdef FROM pg_indexes \
             WHERE tablename = 'readings' AND indexname = 'readings$meter$value$ix'",
        )
        .expect("catalog");
    let def = rows
        .first()
        .and_then(|r| r.first().cloned())
        .flatten()
        .expect("the declared index exists");
    assert!(
        def.contains(r#"("$space", meter, value)"#)
            || def.contains(r#"("$space", "meter", "value")"#),
        "the space leads, then the declared columns in order: {def}"
    );
    assert!(
        !def.starts_with("CREATE UNIQUE"),
        "an `index` claims nothing about uniqueness: {def}"
    );

    // The composite key's index is the one that DOES claim uniqueness, and
    // it is a different object with a different name.
    let rows = pg
        .query(
            "SELECT indexdef FROM pg_indexes \
             WHERE tablename = 'readings' AND indexname = 'readings$meter$at$uq'",
        )
        .expect("catalog");
    let def = rows
        .first()
        .and_then(|r| r.first().cloned())
        .flatten()
        .expect("the declared unique key exists");
    assert!(def.contains("UNIQUE"), "{def}");
    assert!(
        def.contains("NULLS NOT DISTINCT"),
        "and it carries the language's equality: {def}"
    );
}
