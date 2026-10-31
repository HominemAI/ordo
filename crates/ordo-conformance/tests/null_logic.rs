//! Two-valued logic, against a real server.
//!
//! DL §2 *"Null semantics [settled]"* fixes the truth table — `null == null`
//! is **true**, `null != 5` is **true**, every ordering comparison against
//! null is **false** — and then states the part that had no implementation:
//!
//! > **Planner note — two-valued logic is a compilation obligation
//! > [settled].** … every comparison compiles to the distinctness family —
//! > `IS NOT DISTINCT FROM` for `==`, `IS DISTINCT FROM` for `!=` and for
//! > `not (==)` — **everywhere**, without exception… **A plain `=` in
//! > generated SQL is a bug wherever a nullable operand can reach it.**
//!
//! The planner emitted `=` and `<>`. So every comparison against a nullable
//! column produced SQL's three-valued answer and the row was **silently
//! dropped** — no error, no warning, a shorter result set. Found by the
//! settled-versus-code audit; recorded once before as X-011 and closed as a
//! *document* fix, with the planner never changed.
//!
//! `categories.parent` is null on two of the five seeded rows — an unguarded
//! entity, so each assertion is purely about the logic and not about what a
//! caller may see. Every one is a result set that differs under the two
//! logics.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::{Fixture, Pg};
use serde_json::Value;

fn seeded() -> (Fixture, Pg) {
    let f = Fixture::load().expect("the market fixture compiles");
    let pg = Pg::named("ordo_null_logic");
    f.reset_once(&pg).expect("seeded");
    (f, pg)
}

fn ids(f: &Fixture, pg: &Pg, src: &str) -> Vec<String> {
    let v: Value = f.run_case(pg, src).unwrap_or_else(|e| panic!("{src}\n{e}"));
    v.as_array()
        .expect("rows")
        .iter()
        .map(|r| r["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `null == null` is **true**, so a comparison against a null literal finds
/// the null rows. Under SQL's `=` this returns nothing at all.
#[test]
fn equality_against_null_finds_the_null_rows() {
    let (f, pg) = seeded();
    let got = ids(&f, &pg, "categories(parent == null)[id]");
    assert_eq!(
        got,
        vec!["cat_1", "cat_4"],
        "`null == null` is true — under `=` this is silently empty"
    );
}

/// `null != 5` is **true**, so `!=` keeps the null rows. Under SQL's `<>`
/// they vanish — which is the case that makes the bug dangerous rather than
/// merely wrong: a filter meant to EXCLUDE something also drops every row
/// where the field was never set.
#[test]
fn inequality_keeps_the_null_rows() {
    let (f, pg) = seeded();
    let got = ids(&f, &pg, r#"categories(parent != "cat_1")[id]"#);
    assert_eq!(
        got,
        vec!["cat_1", "cat_3", "cat_4", "cat_5"],
        "everything whose parent is not `cat_1`, nulls included"
    );
}

/// `x != y` ≡ `not (x == y)`, *"with exactly one meaning however spelled"*.
/// This is the identity SQL breaks: `NOT (a = b)` is UNKNOWN on a null and
/// the row is dropped, so the two spellings disagreed.
#[test]
fn the_two_spellings_of_inequality_agree() {
    let (f, pg) = seeded();
    let direct = ids(&f, &pg, r#"categories(parent != "cat_1")[id]"#);
    let negated = ids(&f, &pg, r#"categories(not (parent == "cat_1"))[id]"#);
    assert_eq!(direct, negated, "one meaning, however spelled");
    assert!(
        direct.contains(&"cat_1".to_string()),
        "and both keep the null rows"
    );
}

/// `==` and `!=` **partition every row**: no row falls out of both, which is
/// exactly what three-valued logic allows and this language forbids.
#[test]
fn equality_and_inequality_partition_the_rows() {
    let (f, pg) = seeded();
    let yes = ids(&f, &pg, r#"categories(parent == "cat_1")[id]"#);
    let no = ids(&f, &pg, r#"categories(parent != "cat_1")[id]"#);
    let all = ids(&f, &pg, "categories[id]");
    let mut both: Vec<String> = yes.iter().chain(no.iter()).cloned().collect();
    both.sort();
    let mut expect = all.clone();
    expect.sort();
    assert_eq!(both, expect, "every row lands on exactly one side");
}

/// Ordering predicates *"always reject null"* — false, not unknown. The
/// difference only shows under negation, which is why the wrapper matters:
/// `not (a < b)` must be TRUE for a null operand, since `a < b` is false.
#[test]
fn a_negated_ordering_predicate_keeps_the_null_rows() {
    let (f, pg) = seeded();

    let kept = ids(&f, &pg, r#"categories(not (parent > "cat_1"))[id]"#);
    assert!(
        kept.contains(&"cat_1".to_string()) && kept.contains(&"cat_4".to_string()),
        "`parent > …` is FALSE for a null, so its negation is TRUE: {kept:?}"
    );
}

/// `in` is a disjunction of equalities, so it inherits the obligation. Under
/// SQL a null left operand makes `IN` unknown and `not in` drops the row.
#[test]
fn negated_membership_keeps_the_null_rows() {
    let (f, pg) = seeded();
    let got = ids(
        &f,
        &pg,
        r#"categories(not (parent in ("cat_1", "cat_2")))[id]"#,
    );
    assert_eq!(
        got,
        vec!["cat_1", "cat_4", "cat_5"],
        "a null parent is not in that list, so it survives the negation"
    );
}

/// The obligation is about the SQL, so assert the SQL. A plain `=` against
/// a nullable operand is the bug two-valued logic closes, except where it
/// cannot change the answer (owner ruling 2026-10-26,
/// `guard-equality-uses-no-index`): in a filter, with one side proven never
/// null, its null answer drops the row exactly as the old `false` did. A
/// future change that emits `=` anywhere else should fail here rather than
/// in a count nobody reads.
#[test]
fn the_generated_sql_keeps_two_valued_logic() {
    let f = Fixture::load().expect("fixture");
    let sql = |src: &str| f.compile(src).expect("plans").sql;

    // A filter, and the literal is never null: plain `=`, which an index
    // can serve.
    let eq = sql(r#"categories(parent == "cat_1")[id]"#);
    assert!(eq.contains(r#"("t0"."parent" = 'cat_1')"#), "{eq}");

    // `!=` with the nullable side spelled out: a null parent is not
    // "cat_1", so the row stays.
    let ne = sql(r#"categories(parent != "cat_1")[id]"#);
    assert!(
        ne.contains(r#"(NOT ("t0"."parent" = 'cat_1') OR "t0"."parent" IS NULL)"#),
        "{ne}"
    );

    // Under `not` a null answer would be inverted, so a nullable side keeps
    // the distinctness form.
    let not = sql(r#"categories(not (parent == "cat_1"))[id]"#);
    assert!(
        not.contains(r#"(NOT ("t0"."parent" IS NOT DISTINCT FROM 'cat_1'))"#),
        "{not}"
    );

    // Both sides possibly null, in a filter: equal, or both null.
    let both = sql(r#"tasks(assignee == project.owner)[id]"#);
    assert!(
        both.contains(r#"("t0"."assignee" = "t1"."owner" OR ("t0"."assignee" IS NULL AND "t1"."owner" IS NULL))"#),
        "a hop through a guarded ref is null when the guard refuses it: {both}"
    );

    // Under `not`, two `NOT NULL` columns of certain rows: plain `=`. The
    // same comparison through a guarded hop is not certain and keeps the
    // distinctness form.
    let own = sql(r#"tasks(not (title == status))[id]"#);
    assert!(
        own.contains(r#"(NOT ("t0"."title" = "t0"."status"))"#),
        "{own}"
    );
    let hop = sql(r#"tasks(not (project.name == title))[id]"#);
    assert!(
        hop.contains(r#"(NOT ("t1"."name" IS NOT DISTINCT FROM "t0"."title"))"#),
        "{hop}"
    );
}
