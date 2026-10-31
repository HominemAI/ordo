//! 28b: what the compiler refuses about a `json` field.
//!
//! A whole document is read and written whole. Every condition, sort,
//! group, aggregate, function and index over one is `json_opaque`; `is
//! null` is the one question it answers about the whole. Conditions on a
//! path inside one are `tests/json_filter.rs`. A `json` field takes a document and lands
//! nowhere else. Its defaults are `{}`, `[]`, and `null` where it admits null.
//!
//! A regression looks like any refused program below compiling, or the
//! accepted one failing. Nothing here touches a database.

use ordo_conformance::{Fixture, FIXTURE, SEED};

const SHAPE: &str = "entity pages (key: pg) {\n  title: text\n  doc: json = {}\n  list: json = []\n  draft: json?\n}\n";

fn compile(extra: &str) -> Result<(), String> {
    let src = format!("{FIXTURE}\n{SHAPE}\n{extra}\n");
    Fixture::from_source(&src, SEED)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A read pipeline's refusal, from binding it against the shape.
fn read_refused(pipeline: &str, class: &str) {
    let f = Fixture::from_source(&format!("{FIXTURE}\n{SHAPE}\n"), SEED).expect("the shape");
    let e = f.compile(pipeline).map(|_| ()).expect_err(pipeline);
    assert!(e.contains(class), "`{pipeline}` is {class}: {e}");
}

fn refused(extra: &str, class: &str) {
    let e = compile(extra).expect_err(extra);
    assert!(e.contains(class), "`{extra}` is {class}: {e}");
}

#[test]
fn a_json_field_with_its_defaults_and_an_is_null_condition_compiles() {
    compile(
        "op addPage($title: text, $doc: json) =\n  set pages { title: $title, doc: $doc }\n\
         op drafts unbounded =\n  pages(draft is not null)[title, draft]\n\
         op copyDraft($p: id) =\n  set pages { id: $p, draft: doc }\n\
         op clearDraft($p: id) =\n  set pages { id: $p, draft: null }\n",
    )
    .expect("compiles");
}

#[test]
fn a_condition_on_a_json_field_is_refused_as_json_opaque() {
    refused(
        "op a($d: json) =\n  pages(doc == $d)[title]\n",
        "json_opaque",
    );
    read_refused("pages(doc in (draft, list))[title]", "json_opaque");
    read_refused("pages(doc != draft)[title]", "json_opaque");
}

#[test]
fn a_sort_a_group_an_aggregate_or_a_function_over_json_is_refused() {
    read_refused("pages[title, doc] sort doc", "json_opaque");
    read_refused("pages[title] group doc { n: count }", "json_opaque");
    read_refused("pages[doc] reduce { m: max(doc) }", "json_opaque");
    read_refused("pages[title, d: coalesce(draft, doc)]", "json_opaque");
}

#[test]
fn a_json_field_cannot_be_unique_or_indexed_or_in_a_check() {
    let shape = |s: &str| {
        let src = format!("{FIXTURE}\n{s}\n");
        Fixture::from_source(&src, SEED)
            .map(|_| ())
            .map_err(|e| e.to_string())
            .expect_err(s)
    };
    assert!(shape("entity p (key: p) { d: json unique }").contains("json_opaque"));
    assert!(shape("entity p (key: p) { d: json index }").contains("json_opaque"));
    assert!(shape("entity p (key: p) { d: json  e: int  index (d, e) }").contains("json_opaque"));
    assert!(shape("entity p (key: p) { d: json  check d == 1 }").contains("json_opaque"));
    assert!(shape("entity p (key: p) { d: json = null }").contains("json_default"));
    assert!(shape("entity p (key: p) { d: json = [1] }").contains("`{}`, `[]` or `null`"));
}

#[test]
fn a_json_default_spelled_as_its_json_text_is_the_same_default() {
    let src =
        format!("{FIXTURE}\nentity p (key: p) {{ d: json = \"{{}}\"\n  e: json = \"[]\" }}\n");
    Fixture::from_source(&src, SEED).expect("`\"{}\"` and `\"[]\"` are `{}` and `[]`");
    let src = format!("{FIXTURE}\nentity p (key: p) {{ d: json = \"{{\\\"a\\\":1}}\" }}\n");
    let e = Fixture::from_source(&src, SEED)
        .map(|_| ())
        .expect_err("a document with contents");
    assert!(e.to_string().contains("json_default"), "{e}");
}

#[test]
fn a_text_never_lands_in_a_json_field_and_a_document_never_lands_in_a_text() {
    refused(
        "op a($t: text) =\n  set pages { title: \"x\", doc: $t }\n",
        "param_type_mismatch",
    );
    refused(
        "op a($d: json) =\n  set pages { title: $d }\n",
        "param_type_mismatch",
    );
    refused(
        "op a =\n  set pages { title: \"x\", doc: \"not json\" }\n",
        "param_type_mismatch",
    );
}
