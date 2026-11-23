//! `reader <name> reads <table>, …` in the schema (owner, 2026-11-18,
//! `example-apps-notes-v2-plus-note_viewer` ask 17): every table it names is
//! one of this program's own entities, checked before anything runs.

use ordo_schema::{build, Reader};
use ordo_syntax::parse_program;

const NOTES: &str = r#"
reader note_viewer reads notes, notesAccess, members

entity notes {
  title: text
}

entity notesAccess {
  note -> notes
}

external customers from crm.customers shared {
  id <- "customer_id"
}
"#;

#[test]
fn a_reader_line_lists_its_tables() {
    let s = build(&parse_program(NOTES).expect("parses")).expect("builds");
    assert_eq!(
        s.readers,
        [Reader {
            name: "note_viewer".into(),
            tables: vec!["notes".into(), "notesAccess".into(), "members".into()],
        }]
    );
}

#[test]
fn an_unknown_table_is_refused_naming_it() {
    let src = NOTES.replace("notesAccess, members", "notesAccess, notez");
    let e = build(&parse_program(&src).expect("parses")).expect_err("unknown");
    assert_eq!(e.code, "reader_unknown_table");
    assert!(
        e.message.contains("`notez`") && e.message.contains("not an entity"),
        "{}",
        e.message
    );
}

#[test]
fn an_external_is_not_this_deployments_to_grant() {
    let src = NOTES.replace("notesAccess, members", "customers");
    let e = build(&parse_program(&src).expect("parses")).expect_err("external");
    assert_eq!(e.code, "reader_unknown_table");
    assert!(
        e.message.contains("`customers`, an external"),
        "{}",
        e.message
    );
}
