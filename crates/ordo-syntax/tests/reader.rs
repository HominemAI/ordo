//! `reader <name> reads <table>, <table>` (owner, 2026-11-18,
//! `example-apps-notes-v2-plus-note_viewer` ask 17): another deployment that
//! may read these tables, and the refusals the parser can decide on its own.

use ordo_syntax::{parse_program, PARSE_ERROR, READER_LINE};

const NOTES: &str = r#"
reader note_viewer reads notes, notesAccess

entity notes {
  title: text
}

entity notesAccess {
  note -> notes
}
"#;

#[test]
fn a_reader_line_names_a_reader_and_its_tables() {
    let p = parse_program(NOTES).expect("parses");
    assert_eq!(p.readers.len(), 1);
    let r = &p.readers[0];
    assert_eq!(r.name.name, "note_viewer");
    let tables: Vec<&str> = r.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tables, ["notes", "notesAccess"]);
    assert_eq!(p.entities.len(), 2);
}

#[test]
fn reader_stays_a_name_elsewhere() {
    // Contextual: a field and an op may still be called `reader`.
    parse_program("entity links {\n  reader: text\n}\nop reader = links[reader]\n")
        .expect("parses");
}

#[test]
fn a_reader_name_is_lowercase_letters_digits_and_underscores() {
    let e = parse_program("reader noteViewer reads notes\n").expect_err("camel case");
    assert_eq!(e.code, READER_LINE, "{}", e.message);
    assert!(
        e.message.contains("ORDO_READER_<NAME>_ROLE"),
        "{}",
        e.message
    );
    parse_program("reader viewer2 reads notes\n").expect("digits are fine");
}

#[test]
fn a_reader_line_names_at_least_one_table_each_once() {
    for (src, why) in [
        ("reader viewer\n", "no reads"),
        ("reader viewer reads\n", "no table"),
        ("reader viewer reads notes, notes\n", "twice"),
        (
            "reader viewer reads notes\nreader viewer reads tags\n",
            "two lines",
        ),
    ] {
        let e = parse_program(src).expect_err(why);
        assert_eq!(e.code, READER_LINE, "{why}: {}", e.message);
        assert!(e.message.contains("viewer"), "{why}: {}", e.message);
    }
    let e =
        parse_program("reader viewer reads notes role \"x\"\n").expect_err("a role in the file");
    assert!(
        e.code == PARSE_ERROR && e.message.contains("ORDO_READER_VIEWER_ROLE"),
        "the refusal names the setting: {}",
        e.message
    );
}
