//! The `edition` line (`plan-versions.md` §2.6): one per program, a whole
//! number from 1. `edition` stays an ordinary name everywhere else.

use ordo_syntax::{parse_program, DUPLICATE_EDITION};

#[test]
fn a_program_names_its_edition_on_one_line() {
    let p = parse_program("domain shop @ 0.1.0\nedition 2\n\nentity items {\n  n: text\n}\n")
        .expect("parses");
    let e = p.edition.expect("an edition");
    assert_eq!(e.number, 2);
    assert_eq!((e.span.line, e.span.col), (2, 1));
    assert_eq!(e.span.start, "domain shop @ 0.1.0\n".len());
    assert_eq!(e.span.end, "domain shop @ 0.1.0\nedition 2".len());
}

#[test]
fn no_edition_line_is_none() {
    let p = parse_program("entity items {\n  n: text\n}\n").expect("parses");
    assert!(p.edition.is_none());
}

#[test]
fn a_second_edition_line_is_refused() {
    let e = parse_program("edition 1\nedition 2\n").expect_err("two lines");
    assert_eq!(e.code, DUPLICATE_EDITION);
}

#[test]
fn an_edition_must_be_a_whole_number_from_1() {
    for bad in ["edition\n", "edition 0\n", "edition two\n", "edition 1.5\n"] {
        let e = parse_program(bad).expect_err(bad);
        assert!(e.message.contains("edition"), "{bad}: {e}");
    }
}

#[test]
fn edition_is_still_a_field_name() {
    let p = parse_program("entity books {\n  edition: int\n}\n").expect("parses");
    assert!(p.edition.is_none());
    assert_eq!(p.entities[0].fields[0].name.name, "edition");
}
