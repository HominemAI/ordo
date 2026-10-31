//! DL §2's reserved-word list is `ordo_syntax::token::Keyword`, written down
//! (checklist 27ag).
//!
//! DL said the list was "closed and mechanically regenerated from the
//! grammar". Nothing regenerated it, and the two drifted: the document named
//! `run read write open route std`, which the lexer deliberately keeps
//! contextual, and left out `auth compliance space`, which it reserves. This
//! test reads both and fails when they differ in either direction, so the
//! document can only say what the lexer does.
//!
//! The document is the source of truth for *prose*; the enum is the source
//! of truth for the *set*. A word that should change tiers changes in
//! `token.rs` first, and this test then says which line of DL to rewrite.

use std::collections::BTreeSet;

use ordo_syntax::token::Keyword;

const DL: &str = include_str!("../../../docs/definition-language.md");

/// The backtick span that follows "cannot name fields/entities:" in the
/// "Reserved words" bullet, split on whitespace. The list wraps across
/// lines, so the span is read to its closing backtick rather than to the end
/// of the line.
fn documented_reserved_words() -> Vec<String> {
    let bullet_start = DL
        .find("- **Reserved words**:")
        .expect("DL §2 has a `- **Reserved words**:` bullet");
    let after = &DL[bullet_start..];
    let anchor = "cannot name fields/entities:";
    let list_start = after
        .find(anchor)
        .expect("the bullet says what reserved words cannot name")
        + anchor.len();
    let rest = &after[list_start..];
    let open = rest.find('`').expect("the list opens with a backtick");
    let body = &rest[open + 1..];
    let close = body.find('`').expect("the list closes with a backtick");
    body[..close]
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[test]
fn dl_section_2_lists_exactly_the_keyword_enum() {
    let documented = documented_reserved_words();
    let documented_set: BTreeSet<&str> = documented.iter().map(String::as_str).collect();
    assert_eq!(
        documented_set.len(),
        documented.len(),
        "DL §2 lists a reserved word twice: {documented:?}"
    );

    let lexed: BTreeSet<&str> = Keyword::ALL.iter().map(|k| k.as_str()).collect();

    let only_in_dl: Vec<&&str> = documented_set.difference(&lexed).collect();
    let only_in_lexer: Vec<&&str> = lexed.difference(&documented_set).collect();
    assert!(
        only_in_dl.is_empty() && only_in_lexer.is_empty(),
        "DL §2 and `Keyword` disagree.\n  in DL and not reserved by the lexer: {only_in_dl:?}\n  \
         reserved by the lexer and not in DL: {only_in_lexer:?}\n  Fix `token.rs` if the tier is \
         wrong, else rewrite the list in docs/definition-language.md §2 from `Keyword::ALL`."
    );
}

/// The document lists the words in the enum's declaration order, so that a
/// reader of either sees the same sequence and a regeneration is a copy.
#[test]
fn dl_section_2_lists_them_in_declaration_order() {
    let documented = documented_reserved_words();
    let lexed: Vec<&str> = Keyword::ALL.iter().map(|k| k.as_str()).collect();
    assert_eq!(
        documented, lexed,
        "DL §2's reserved-word list is not `Keyword::ALL` in declaration order"
    );
}

/// Non-vacuity: every documented word really lexes as a keyword, and the
/// contextual words the bullet names as legal field names really do not.
#[test]
fn documented_words_lex_as_keywords_and_named_contextual_words_do_not() {
    for w in documented_reserved_words() {
        assert!(
            Keyword::parse(&w).is_some(),
            "`{w}` is in DL §2's list and does not lex as a keyword"
        );
    }
    for w in [
        "read", "write", "open", "std", "run", "route", "count", "round",
    ] {
        assert!(
            Keyword::parse(w).is_none(),
            "`{w}` is contextual in DL §2 and the lexer reserves it"
        );
    }
}

/// The reject and pass cases DL §2 promises: every reserved word fails as a
/// field name, and every contextual word the bullet names parses as one.
#[test]
fn reserved_words_cannot_name_a_field_and_contextual_words_can() {
    for k in Keyword::ALL {
        let src = format!("entity x {{ {}: int }}", k.as_str());
        assert!(
            ordo_syntax::parse_program(&src).is_err(),
            "`{}` is reserved and named a field: {src}",
            k.as_str()
        );
    }
    for w in [
        "read",
        "write",
        "open",
        "std",
        "run",
        "method",
        "methods",
        "share",
        "rollup",
        "live",
        "table",
        "metric",
        "detail",
        "route",
        "count",
        "sum",
        "avg",
        "min",
        "max",
        "median",
        "percentile",
        "joined",
        "coalesce",
        "round",
    ] {
        let src = format!("entity x {{ {w}: int }}");
        assert!(
            ordo_syntax::parse_program(&src).is_ok(),
            "`{w}` is contextual in DL §2 and could not name a field"
        );
    }
}
