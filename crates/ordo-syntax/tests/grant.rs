//! `grant` blocks (`plan-federation-grants.md` §3, step G1): what another
//! Ordo deployment may call through this one's door. The parser reads the
//! lines; what they match is decided at apply, against the program's ops.

use ordo_syntax::{parse_program, GrantPattern, GRANT_BLOCK};

const NOTES: &str = r#"
grant note_viewer {
  std.notes.get
  std.notes.list
  /std\.notesAccess\..*/
  /add.*/
  !addAdmin
}

grant billing as itself {
  markPaid
}

grant provider hominem {
  std.notes.get
}

grant everyone_2 {
  *
  !/a\/b/
}

entity notes {
  title: text
}
"#;

#[test]
fn the_three_headers_and_every_kind_of_line_parse() {
    let p = parse_program(NOTES).expect("parses");
    assert_eq!(p.grants.len(), 4);

    let v = &p.grants[0];
    assert_eq!(v.name.name, "note_viewer");
    assert!(!v.provider && !v.as_itself);
    let lines: Vec<String> = v.lines.iter().map(|l| l.text()).collect();
    assert_eq!(
        lines,
        [
            "std.notes.get",
            "std.notes.list",
            "/std\\.notesAccess\\..*/",
            "/add.*/",
            "!addAdmin"
        ]
    );
    assert_eq!(
        v.lines[2].pattern,
        GrantPattern::Regex("std\\.notesAccess\\..*".into())
    );
    assert!(v.lines[4].negated);
    assert_eq!(v.lines[4].pattern, GrantPattern::Op("addAdmin".into()));
    assert_eq!(v.header(), "grant note_viewer");

    let b = &p.grants[1];
    assert!(b.as_itself && !b.provider);
    assert_eq!(b.header(), "grant billing as itself");

    let h = &p.grants[2];
    assert!(h.provider && !h.as_itself);
    assert_eq!(h.name.name, "hominem");
    assert_eq!(h.header(), "grant provider hominem");

    let e = &p.grants[3];
    assert_eq!(e.lines[0].pattern, GrantPattern::All);
    // `\/` is a slash in the pattern, and prints back escaped.
    assert_eq!(e.lines[1].pattern, GrantPattern::Regex("a/b".into()));
    assert_eq!(e.lines[1].text(), "!/a\\/b/");

    // The rest of the file is untouched: the entity still parses.
    assert_eq!(p.entities.len(), 1);
}

#[test]
fn a_slash_outside_a_grant_block_is_still_division() {
    let src = "grant v {\n  /a.*/\n}\n\nop half = notes[title] | take 10 / 2\n\nentity notes {\n  title: text\n}\n";
    // Parses or fails on the op for its own reasons; never lexes `/ 2` as a
    // pattern. Lexing is what is checked.
    let toks = ordo_syntax::lex(src).expect("lexes");
    assert!(toks.iter().any(|t| t.kind == ordo_syntax::TokenKind::Slash));
    assert_eq!(
        toks.iter()
            .filter(|t| matches!(t.kind, ordo_syntax::TokenKind::Regex(_)))
            .count(),
        1
    );
}

#[test]
fn a_bad_name_is_refused_naming_the_setting() {
    let e = parse_program("grant noteViewer {\n  a\n}\n").expect_err("camel");
    assert_eq!(e.code, GRANT_BLOCK, "{}", e.message);
    assert!(
        e.message.contains("ORDO_CONSUMER_<NAME>_CLIENT"),
        "{}",
        e.message
    );
}

#[test]
fn a_block_is_declared_once_and_holds_a_line() {
    let e = parse_program("grant v {\n  a\n}\n\ngrant v {\n  b\n}\n").expect_err("twice");
    assert_eq!(e.code, GRANT_BLOCK);
    assert!(e.message.contains("declared twice"), "{}", e.message);

    let e = parse_program("grant v {\n}\n").expect_err("empty");
    assert_eq!(e.code, GRANT_BLOCK);
    assert!(e.message.contains("grants nothing"), "{}", e.message);
}

#[test]
fn one_pattern_per_line() {
    let e = parse_program("grant v {\n  a b\n}\n").expect_err("two on a line");
    assert_eq!(e.code, GRANT_BLOCK);
    assert!(e.message.contains("one pattern per line"), "{}", e.message);
}

#[test]
fn a_provider_grant_is_never_as_itself() {
    let e = parse_program("grant provider hominem as itself {\n  a\n}\n").expect_err("as itself");
    assert_eq!(e.code, GRANT_BLOCK);
    assert!(
        e.message.contains("on behalf of a person only"),
        "{}",
        e.message
    );
}

#[test]
fn a_grant_named_provider_is_a_plain_grant() {
    let p = parse_program("grant provider {\n  a\n}\n").expect("parses");
    assert!(!p.grants[0].provider);
    assert_eq!(p.grants[0].name.name, "provider");
}

#[test]
fn an_unclosed_pattern_and_a_stray_word_are_refused() {
    let e = parse_program("grant v {\n  /add.*\n}\n").expect_err("unclosed");
    assert!(e.message.contains("ends with `/`"), "{}", e.message);

    let e = parse_program("grant v {\n  //\n}\n").expect_err("empty pattern");
    assert!(e.message.contains("empty grant pattern"), "{}", e.message);

    let e = parse_program("grant v {\n  \"a\"\n}\n").expect_err("string");
    assert!(
        e.message.contains("an op name, a `/regex/` or `*`"),
        "{}",
        e.message
    );

    let e = parse_program("grant v as viewer {\n  a\n}\n").expect_err("as what");
    assert!(e.message.contains("expected `itself`"), "{}", e.message);
}

#[test]
fn bang_outside_a_grant_block_keeps_its_message() {
    let e = parse_program("op x = notes(not !true)\n").expect_err("bang");
    assert!(
        e.message.contains("negation is the word `not`"),
        "{}",
        e.message
    );
}
