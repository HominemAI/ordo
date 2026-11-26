//! `peer <name> { op … }` (`plan-federation-grants.md` §4): a local nickname
//! for another Ordo deployment and the holder's ops this one calls through
//! its door, and the refusals the parser can decide on its own.
//!
//! The one-line `peer <name>` and the `reader` line were the shared-database
//! path, removed in G8 (owner, board `federation-ordo-deployments-share-
//! data-through`, ask 2: "Remove"). Each is refused by name, with what to
//! write instead.
//!
//! A regression looks like the one-line form or a `reader` line parsing, or
//! a refusal that does not say what to write.

use ordo_syntax::{parse_program, PARSE_ERROR, PEER_BLOCK, PEER_NAME, READER_LINE};

const VIEWER: &str = r#"
peer journal {
  std.notes.list
}

external entries from journal.notes shared {
  id: text <- id
  title: text <- title
  owner: text <- owner
  access read owner == $caller
}
"#;

#[test]
fn a_peer_block_declares_a_nickname() {
    let p = parse_program(VIEWER).expect("parses");
    assert_eq!(p.peers.len(), 1);
    assert_eq!(p.peers[0].name.name, "journal");
    assert_eq!(p.externals.len(), 1);
    assert_eq!(p.externals[0].schema.name, "journal");
    assert_eq!(p.externals[0].table.name, "notes");
}

#[test]
fn without_a_peer_the_prefix_is_a_plain_schema() {
    let p = parse_program(
        "external customers from crm.customers shared {\n  id <- \"customer_id\"\n}\n",
    )
    .expect("parses");
    assert!(p.peers.is_empty());
    assert_eq!(p.externals[0].schema.name, "crm");
}

#[test]
fn peer_and_reader_stay_names_elsewhere() {
    // Contextual: a field and an op may still be called `peer` or `reader`.
    parse_program("entity links {\n  peer: text\n  reader: text\n}\nop peer = links[peer]\n")
        .expect("parses");
}

#[test]
fn a_peer_name_is_lowercase_letters_and_digits() {
    for bad in [
        "peer Journal { a }",
        "peer notesV2 { a }",
        "peer \"my_peer\" { a }",
    ] {
        let e = parse_program(&format!("{bad}\n")).expect_err(bad);
        assert!(
            e.code == PEER_NAME || e.code == PARSE_ERROR,
            "{bad}: {} {}",
            e.code,
            e.message
        );
    }
    let e = parse_program("peer notesV2 {\n  a\n}\n").expect_err("camel case");
    assert_eq!(e.code, PEER_NAME, "{}", e.message);
    assert!(e.message.contains("ORDO_PEER_<NAME>_URL"), "{}", e.message);
    parse_program("peer notes2 {\n  a\n}\n").expect("digits are fine");
}

#[test]
fn a_peer_block_lists_the_holder_ops_it_calls() {
    let p = parse_program("peer notes {\n  std.notes.list\n  addComment\n}\n").expect("parses");
    let ops: Vec<&str> = p.peers[0].uses.iter().map(|u| u.op.as_str()).collect();
    assert_eq!(ops, ["std.notes.list", "addComment"]);
}

#[test]
fn a_peer_block_refuses_what_is_not_one_exact_op_per_line() {
    let cases = [
        ("peer notes {\n}\n", "names no op"),
        ("peer notes {\n  addComment addNote\n}\n", "one op per line"),
        ("peer notes {\n  addComment\n  addComment\n}\n", "twice"),
        ("peer notes {\n  /add.*/\n}\n", "patterns and `!` belong"),
        ("peer notes {\n  *\n}\n", "patterns and `!` belong"),
    ];
    for (src, want) in cases {
        let e = parse_program(src).expect_err(src);
        assert_eq!(e.code, PEER_BLOCK, "{src}: {}", e.message);
        assert!(e.message.contains(want), "{src}: {}", e.message);
    }
    // `!` outside a grant is the lexer's refusal.
    parse_program("peer notes {\n  !addComment\n}\n").expect_err("a negation");
    let e = parse_program("peer notes {\n  addComment\n").expect_err("unclosed");
    assert!(e.message.contains("not closed"), "{}", e.message);
}

#[test]
fn a_peer_is_declared_once_and_holds_no_address() {
    let e = parse_program("peer journal {\n  a\n}\npeer journal {\n  b\n}\n").expect_err("twice");
    assert_eq!(e.code, PEER_NAME, "{}", e.message);

    let e = parse_program("peer journal url \"https://x\"\n").expect_err("an address in the file");
    assert!(
        e.message.contains("ORDO_PEER_JOURNAL_URL"),
        "the refusal names the setting: {}",
        e.message
    );
}

/// G8: the one-line form read another deployment's tables. Refused by name,
/// with the block to write and the setting that replaces its schema.
#[test]
fn the_one_line_peer_is_refused_naming_the_block_to_write() {
    let e = parse_program("peer journal\n").expect_err("the shared-database form");
    assert_eq!(e.code, PEER_NAME, "{}", e.message);
    for want in [
        "that path was removed",
        "only through its door",
        "`peer journal { std.notes.list }`",
        "ORDO_PEER_JOURNAL_URL",
        "in place of ORDO_PEER_JOURNAL_SCHEMA",
    ] {
        assert!(e.message.contains(want), "{want}: {}", e.message);
    }
}

/// G8: a `reader` line granted a database role the tables. Refused by name,
/// with the `grant` block that opens the same tables through the door.
#[test]
fn a_reader_line_is_refused_with_the_grant_to_write() {
    let src = "reader note_viewer reads notes, notesAccess\n\nentity notes {\n  title: text\n}\n";
    let e = parse_program(src).expect_err("the shared-database consent line");
    assert_eq!(e.code, READER_LINE, "{}", e.message);
    for want in [
        "that path was removed",
        "grant note_viewer {\n  std.notes.list\n  std.notesAccess.list\n}",
        "ORDO_CONSUMER_NOTE_VIEWER_CLIENT",
        "in place of ORDO_READER_NOTE_VIEWER_ROLE",
    ] {
        assert!(e.message.contains(want), "{want}: {}", e.message);
    }
    // A line with no table still says what to write.
    let e = parse_program("reader viewer\n").expect_err("no table");
    assert_eq!(e.code, READER_LINE, "{}", e.message);
    assert!(e.message.contains("std.<table>.list"), "{}", e.message);
}
