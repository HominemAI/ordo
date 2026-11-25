//! `prev` (`plan-syntax-additions.md` §5, `plan-versions.md` increment 15):
//! the spelling, and the 3 compile checks the parser decides alone.

use ordo_syntax::{
    parse_program, EXTERNAL_UNSUPPORTED, PREV_DUPLICATE, PREV_SELF, PREV_STILL_DECLARED,
};

fn refused(src: &str) -> (&'static str, String) {
    let e = parse_program(src).expect_err(src);
    (e.code, e.message)
}

#[test]
fn a_field_and_an_entity_declare_their_previous_names() {
    let p = parse_program(
        "entity customers (key: cust, prev: clients) {\n  fullName: text prev(name)\n  \
         note: text? prev(orders.shippingNote)\n  joined: text prev(first, last)\n  prev: int\n}\n",
    )
    .unwrap();
    let e = &p.entities[0];
    assert_eq!(e.key.as_ref().unwrap().name, "cust");
    assert_eq!(e.prev.as_ref().unwrap().name, "clients");
    let f = |n: &str| e.fields.iter().find(|f| f.name.name == n).unwrap();
    assert_eq!(f("fullName").prev[0].text(), "name");
    assert_eq!(f("note").prev[0].text(), "orders.shippingNote");
    assert!(f("note").optional);
    let merge: Vec<String> = f("joined").prev.iter().map(|n| n.text()).collect();
    assert_eq!(merge, ["first", "last"]);
    // A field named `prev` is still a field.
    assert!(f("prev").prev.is_empty());
}

#[test]
fn prev_may_come_first_in_the_header_and_stand_alone() {
    let p = parse_program("entity a (prev: b, key: aa) {\n  x: int\n}\n").unwrap();
    assert_eq!(p.entities[0].prev.as_ref().unwrap().name, "b");
    assert_eq!(p.entities[0].key.as_ref().unwrap().name, "aa");
    let p = parse_program("entity a (prev: b) {\n  x: int\n}\n").unwrap();
    assert!(p.entities[0].key.is_none());
}

#[test]
fn prev_naming_its_own_name_is_refused() {
    let (code, msg) = refused("entity a {\n  fullName: text prev(fullName)\n}\n");
    assert_eq!(code, PREV_SELF);
    assert!(msg.contains("says nothing"), "{msg}");
    let (code, _) = refused("entity a (prev: a) {\n  x: int\n}\n");
    assert_eq!(code, PREV_SELF);
}

#[test]
fn prev_naming_a_field_still_declared_is_refused() {
    let (code, msg) = refused("entity a {\n  name: text\n  fullName: text prev(name)\n}\n");
    assert_eq!(code, PREV_STILL_DECLARED);
    assert!(msg.contains("`name` still exists"), "{msg}");
}

#[test]
fn two_fields_each_naming_one_previous_name_are_a_split() {
    let p = parse_program("entity a {\n  first: text prev(name)\n  last: text prev(name)\n}\n")
        .unwrap();
    let f = &p.entities[0].fields;
    assert_eq!(f[0].prev[0].text(), "name");
    assert_eq!(f[1].prev[0].text(), "name");
}

#[test]
fn a_name_a_merge_also_claims_is_refused() {
    let (code, msg) =
        refused("entity a {\n  first: text prev(name)\n  fullName: text prev(name, title)\n}\n");
    assert_eq!(code, PREV_DUPLICATE);
    assert!(
        msg.contains("both `first` and `fullName` claim `name`"),
        "{msg}"
    );
    let (code, msg) = refused("entity a {\n  x: text prev(name, name)\n}\n");
    assert_eq!(code, PREV_DUPLICATE);
    assert!(msg.contains("names `name` twice"), "{msg}");
}

#[test]
fn prev_in_an_externals_header_is_refused_by_name() {
    let (code, msg) = refused("external c (prev: d) from crm.c shared {\n  id <- id\n}\n");
    assert_eq!(code, EXTERNAL_UNSUPPORTED);
    assert!(msg.contains("re-imported on every apply"), "{msg}");
}
