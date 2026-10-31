//! The lexer never panics on any input (audit 2026-10-11, finding 29).
//!
//! `op thirsty($today: date filled) = …` once panicked: the two-byte
//! operator lookahead sliced through the three-byte `…`.

use ordo_syntax::{lex, parse_program};

#[test]
fn ellipsis_after_equals_is_a_parse_error_naming_the_character() {
    let src = "op thirsty($today: date filled) = …\n";
    let err = lex(src).unwrap_err();
    assert!(
        err.message.contains('…') && err.message.contains("U+2026"),
        "the error names the character: {}",
        err.message
    );
    assert!(parse_program(src).is_err());
}

/// A sample of characters of every UTF-8 width, with the edges of each
/// width, plus every ASCII character.
fn sample_chars() -> Vec<char> {
    let mut out: Vec<char> = (0u32..0x80).filter_map(char::from_u32).collect();
    let picks = [
        0x80, 0xA0, 0xE9, 0x7FF, // 2 bytes
        0x800, 0x2026, 0x2014, 0x20AC, 0x3000, 0xFEFF, 0xFFFD, 0xFFFF, // 3 bytes
        0x10000, 0x1F600, 0x10FFFF, // 4 bytes
    ];
    out.extend(picks.iter().filter_map(|&c| char::from_u32(c)));
    // A stride over the whole range catches what the picks miss.
    out.extend((0x80u32..0x11_0000).step_by(997).filter_map(char::from_u32));
    out
}

#[test]
fn every_character_after_an_operator_lexes_without_panicking() {
    let operators = [
        "=",
        "<",
        ">",
        "!",
        "-",
        "+",
        "*",
        "/",
        "?",
        "@",
        "|",
        ":",
        ",",
        ".",
        "(",
        "[",
        "{",
        "==",
        "!=",
        "<=",
        ">=",
        "->",
        "$",
        "\"",
        "#",
        "1",
        "2026-01-0",
    ];
    for op in operators {
        for c in sample_chars() {
            for src in [
                format!("{op}{c}"),
                format!("x {op}{c}"),
                format!("x {op}{c}\n"),
                format!("{c}{op}"),
            ] {
                let got = std::panic::catch_unwind(|| {
                    let _ = lex(&src);
                    let _ = parse_program(&src);
                });
                assert!(got.is_ok(), "panicked on {src:?}");
            }
        }
    }
}
