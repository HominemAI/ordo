//! 28b, filtering: conditions on a path into a `json` document
//! (owner ruling 2026-10-08, "Add filtering now").
//!
//! `prefs.theme == "dark"` walks into the document. The path has no type of
//! its own; the condition reads it as the type of its other side: text, a
//! number (exact `numeric`) or a bool. The three behaviours the owner ruled
//! on 2026-10-08 (`json-field` asks 4 to 6) each have a test here by name:
//!
//! - ask 4: `is null` is true for a missing key and for JSON `null`;
//! - ask 5: a value of the wrong JSON type does not match, and is no error;
//! - ask 6: `contains` asks an array for an element and a string for a
//!   substring, settled by the value's JSON type.
//!
//! A regression looks like a refused program below compiling, a filter
//! matching a different set of rows, or a cast error from Postgres on a
//! value of the wrong type. The filtering tests need the conformance
//! cluster; the refusals touch no database.

use ordo_conformance::{Fixture, Pg};
use serde_json::Value;

const PROGRAM: &str = "\
entity people (key: pers) {
  name: text
  card: json = {}
  access read   open
  access write  open
}

entity notes (key: nt) {
  title:  text
  prefs:  json?
  labels: json = []
  locked: json? sealed
  author -> people
  access read   open
  access write  open
}

op notesByTheme($t: text) unbounded =
  notes(prefs.theme == $t)[title]

op notesOverSize($n: int) unbounded =
  notes(prefs.size > $n)[title]

op notesTagged($t: text) unbounded =
  notes(prefs.tags contains $t)[title]
";

/// One row per note: its title and its document, as `jsonb` text.
const NOTES: &[(&str, &str, Option<&str>, &str)] = &[
    (
        "nt_1",
        "n1",
        Some(
            r#"{"theme": "dark", "size": 3, "ratio": 1.25, "on": true, "tags": ["red", "blue"],
                "my key": "x", "it's": "q", "nest": {"deep": {"v": "z"}}, "list": [{"a": 1}]}"#,
        ),
        r#"["urgent"]"#,
    ),
    (
        "nt_2",
        "n2",
        Some(r#"{"theme": "light", "size": "big", "on": false, "tags": ["green"]}"#),
        "[]",
    ),
    (
        "nt_3",
        "n3",
        Some(r#"{"theme": null, "size": 10, "tags": "reddish"}"#),
        r#""urgent care""#,
    ),
    ("nt_4", "n4", None, "[]"),
    (
        "nt_5",
        "n5",
        Some(r#"{"size": 2.50, "on": "yes", "tags": [1, 2, true]}"#),
        "[]",
    ),
];

fn fixture() -> Fixture {
    Fixture::from_source(PROGRAM, "").expect("the program compiles")
}

fn seeded(db: &str) -> (Fixture, Pg) {
    let f = fixture();
    let pg = Pg::scoped(db);
    f.reset(&pg).expect("the schema applies");
    let space = ordo_conformance::FIXTURE_SPACE;
    let mut sql = vec![
        format!(
            "INSERT INTO \"people\" (\"$space\",\"id\",\"rev\",\"name\",\"card\") \
             VALUES ('{space}','pers_1',1,'Ada','{{\"role\": \"admin\"}}')"
        ),
        format!(
            "INSERT INTO \"people\" (\"$space\",\"id\",\"rev\",\"name\",\"card\") \
             VALUES ('{space}','pers_2',1,'Bo','{{}}')"
        ),
    ];
    for (i, (id, title, prefs, labels)) in NOTES.iter().enumerate() {
        let prefs = prefs.map_or("NULL".to_string(), |p| {
            format!("'{}'", p.replace('\'', "''"))
        });
        let author = if i == 0 { "pers_1" } else { "pers_2" };
        sql.push(format!(
            "INSERT INTO \"notes\" (\"$space\",\"id\",\"rev\",\"title\",\"prefs\",\"labels\",\"author\") \
             VALUES ('{space}','{id}',1,'{title}',{prefs},'{labels}','{author}')"
        ));
    }
    for s in sql {
        pg.exec(&s).unwrap_or_else(|e| panic!("{s}: {e}"));
    }
    (f, pg)
}

/// The titles a condition keeps, in title order.
fn titles(f: &Fixture, pg: &Pg, cond: &str) -> Vec<String> {
    let src = format!("notes({cond})[title] sort title");
    let v = f
        .run_case(pg, &src)
        .unwrap_or_else(|e| panic!("`{src}`: {e}"));
    rows_titles(v)
}

fn rows_titles(v: Value) -> Vec<String> {
    v.as_array()
        .expect("rows")
        .iter()
        .map(|r| r["title"].as_str().expect("a title").to_string())
        .collect()
}

#[test]
fn a_path_compared_with_text_reads_the_value_as_text() {
    let (f, pg) = seeded("json_filter_text");
    assert_eq!(titles(&f, &pg, r#"prefs.theme == "dark""#), ["n1"]);
    assert_eq!(
        titles(&f, &pg, r#"prefs.theme in ("dark", "light")"#),
        ["n1", "n2"]
    );
    assert_eq!(titles(&f, &pg, r#"prefs.theme starts "da""#), ["n1"]);
    assert_eq!(titles(&f, &pg, r#"prefs.theme matches "^l""#), ["n2"]);
    assert_eq!(titles(&f, &pg, r#"prefs.theme > "e""#), ["n2"]);
    // `!=` is `not ==`: a missing value is not "dark", as a null field is
    // not. Ask 5's wrong-typed value reads as missing, so it is kept too.
    assert_eq!(
        titles(&f, &pg, r#"prefs.theme != "dark""#),
        ["n2", "n3", "n4", "n5"]
    );
}

#[test]
fn a_path_compared_with_a_number_compares_exactly() {
    let (f, pg) = seeded("json_filter_number");
    assert_eq!(titles(&f, &pg, "prefs.size > 2"), ["n1", "n3", "n5"]);
    assert_eq!(titles(&f, &pg, "prefs.size == 2.5"), ["n5"]);
    assert_eq!(titles(&f, &pg, "prefs.ratio == 1.25"), ["n1"]);
    assert_eq!(titles(&f, &pg, "prefs.ratio < 1.250001"), ["n1"]);
    assert_eq!(titles(&f, &pg, "prefs.size in (3, 10)"), ["n1", "n3"]);
}

#[test]
fn a_path_compared_with_a_bool_reads_the_value_as_a_bool() {
    let (f, pg) = seeded("json_filter_bool");
    assert_eq!(titles(&f, &pg, "prefs.on == true"), ["n1"]);
    assert_eq!(titles(&f, &pg, "prefs.on == false"), ["n2"]);
}

/// Board `json-field` ask 4.
#[test]
fn is_null_is_true_for_a_missing_key_and_for_json_null_ask_4() {
    let (f, pg) = seeded("json_filter_null");
    // n3 holds JSON null, n4 has no document, n5 has no `theme` key.
    assert_eq!(titles(&f, &pg, "prefs.theme is null"), ["n3", "n4", "n5"]);
    assert_eq!(titles(&f, &pg, "prefs.theme is not null"), ["n1", "n2"]);
    assert_eq!(titles(&f, &pg, "prefs is null"), ["n4"]);
}

/// Board `json-field` ask 5.
#[test]
fn a_value_of_the_wrong_json_type_does_not_match_and_is_no_error_ask_5() {
    let (f, pg) = seeded("json_filter_wrong_type");
    // n2's size is "big": no cast error, no match.
    assert_eq!(titles(&f, &pg, "prefs.size < 100"), ["n1", "n3", "n5"]);
    // n5's `on` is "yes", a string: not a bool, so neither true nor false.
    assert_eq!(titles(&f, &pg, "prefs.on == true"), ["n1"]);
    assert_eq!(titles(&f, &pg, r#"prefs.on == "yes""#), ["n5"]);
    // A number is not text: `size == "3"` does not match n1's 3.
    assert!(titles(&f, &pg, r#"prefs.size == "3""#).is_empty());
}

/// Board `json-field` ask 6.
#[test]
fn contains_asks_an_array_for_an_element_and_a_string_for_a_substring_ask_6() {
    let (f, pg) = seeded("json_filter_contains");
    // n1's tags is an array holding "red"; n3's is the string "reddish".
    assert_eq!(
        titles(&f, &pg, r#"prefs.tags contains "red""#),
        ["n1", "n3"]
    );
    // "re" is a substring of "reddish", and no element of n1's array.
    assert_eq!(titles(&f, &pg, r#"prefs.tags contains "re""#), ["n3"]);
    assert_eq!(titles(&f, &pg, "prefs.tags contains 1"), ["n5"]);
    assert_eq!(titles(&f, &pg, "prefs.tags contains true"), ["n5"]);
    // A whole field takes `contains` too.
    assert_eq!(titles(&f, &pg, r#"labels contains "urgent""#), ["n1", "n3"]);
    assert_eq!(
        titles(&f, &pg, r#"not (labels contains "urgent")"#),
        ["n2", "n4", "n5"]
    );
}

#[test]
fn keys_that_are_not_names_positions_and_nesting_reach_the_right_value() {
    let (f, pg) = seeded("json_filter_keys");
    assert_eq!(titles(&f, &pg, r#"prefs["my key"] == "x""#), ["n1"]);
    assert_eq!(titles(&f, &pg, r#"prefs["it's"] == "q""#), ["n1"]);
    assert_eq!(titles(&f, &pg, r#"prefs["theme"] == "dark""#), ["n1"]);
    assert_eq!(titles(&f, &pg, r#"prefs.nest.deep.v == "z""#), ["n1"]);
    assert_eq!(titles(&f, &pg, r#"prefs.nest["deep"].v == "z""#), ["n1"]);
    assert_eq!(titles(&f, &pg, r#"prefs.tags[1] == "blue""#), ["n1"]);
    assert_eq!(titles(&f, &pg, "prefs.list[0].a == 1"), ["n1"]);
    // A key with a quote reaches SQL as a doubled quote inside a literal,
    // never as text that ends the literal.
    assert!(titles(&f, &pg, r#"prefs["x'); DROP TABLE notes; --"] == "q""#).is_empty());
    assert_eq!(titles(&f, &pg, r#"prefs.theme == "dark""#), ["n1"]);
}

#[test]
fn a_path_through_a_ref_walks_into_the_document_at_its_end() {
    let (f, pg) = seeded("json_filter_hop");
    assert_eq!(titles(&f, &pg, r#"author.card.role == "admin""#), ["n1"]);
}

#[test]
fn a_registered_op_filters_on_its_parameters() {
    let (f, pg) = seeded("json_filter_params");
    let run = |src: &str, params: &[(&str, &str)]| {
        rows_titles(
            f.run_case_params(&pg, &format!("{src} sort title"), params)
                .unwrap_or_else(|e| panic!("`{src}`: {e}")),
        )
    };
    assert_eq!(run("notesByTheme($t: $t)", &[("t", "\"light\"")]), ["n2"]);
    assert_eq!(run("notesOverSize($n: $n)", &[("n", "5")]), ["n3"]);
    assert_eq!(run("notesTagged($t: $t)", &[("t", "\"green\"")]), ["n2"]);
}

#[test]
fn the_sql_carries_every_key_as_a_quoted_literal() {
    let f = fixture();
    let sql = f
        .compile(r#"notes(prefs["it's"].x == "q")[title]"#)
        .expect("compiles")
        .sql;
    assert!(sql.contains("ARRAY['it''s', 'x']::text[]"), "{sql}");
}

// ── what stays refused ───────────────────────────────────────────────────

fn refused(src: &str, class: &str) {
    let e = fixture().compile(src).map(|_| ()).expect_err(src);
    assert!(e.contains(class), "`{src}` is {class}: {e}");
}

#[test]
fn a_path_with_no_typed_other_side_is_refused() {
    refused("notes(prefs.a == prefs.b)[title]", "json_path_type");
    refused("notes(prefs.a == null)[title]", "json_path_type");
    refused("notes(prefs.a)[title]", "json_path_type");
    refused("notes(prefs.a == 2026-01-01)[title]", "json_path_type");
    refused(r#"notes(prefs.a in ("x", 1))[title]"#, "json_path_type");
    refused("notes(prefs.a in (null))[title]", "json_path_type");
    refused(r#"notes("abc" contains prefs.a)[title]"#, "json_path_type");
    refused("notes(prefs.a starts 1)[title]", "json_path_type");
    refused("notes(prefs.a == prefs)[title]", "json_opaque");
}

#[test]
fn sort_group_aggregate_arithmetic_and_projection_over_a_path_are_refused() {
    refused("notes[title] sort prefs.a", "json_opaque");
    refused("notes[title] group prefs.a { n: count }", "json_opaque");
    refused("notes[title] reduce { m: max(prefs.a) }", "json_opaque");
    refused("notes[title, a: prefs.a]", "json_opaque");
    refused("notes[title, prefs.a]", "json_opaque");
    refused("notes(prefs.a + 1 > 2)[title]", "json_opaque");
    refused("notes(coalesce(prefs.a, 1) > 2)[title]", "json_opaque");
}

#[test]
fn a_whole_document_compared_with_anything_is_still_refused() {
    refused(r#"notes(prefs == "x")[title]"#, "json_opaque");
    refused(r#"notes(prefs starts "x")[title]"#, "json_opaque");
    refused(r#"notes(prefs in ("x"))[title]"#, "json_opaque");
}

#[test]
fn a_path_into_a_sealed_document_is_refused() {
    let e = fixture()
        .compile(r#"notes(locked.a == "x")[title]"#)
        .map(|_| ())
        .expect_err("sealed");
    assert!(
        e.contains("sealed_unsupported") && e.contains("encrypted"),
        "{e}"
    );
}
