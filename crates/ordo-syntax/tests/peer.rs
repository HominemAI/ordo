//! `peer <name>` (owner, 2026-11-17, `example-apps-notes-v2-plus-note_viewer`
//! asks 5 and 10): a local nickname for another Ordo deployment, and the
//! refusals the parser can decide on its own.

use ordo_syntax::{parse_program, PARSE_ERROR, PEER_NAME};

const VIEWER: &str = r#"
peer journal

external entries from journal.notes shared {
  id: text <- id
  title: text <- title
  owner: text <- owner
  access read owner == $caller
}
"#;

#[test]
fn a_peer_line_declares_a_nickname() {
    let p = parse_program(VIEWER).expect("parses");
    assert_eq!(p.peers.len(), 1);
    assert_eq!(p.peers[0].name.name, "journal");
    assert_eq!(p.externals.len(), 1);
    assert_eq!(p.externals[0].schema.name, "journal");
    assert_eq!(p.externals[0].table.name, "notes");
}

#[test]
fn without_a_peer_line_the_prefix_is_a_plain_schema() {
    let p = parse_program(
        "external customers from crm.customers shared {\n  id <- \"customer_id\"\n}\n",
    )
    .expect("parses");
    assert!(p.peers.is_empty());
    assert_eq!(p.externals[0].schema.name, "crm");
}

#[test]
fn peer_stays_a_name_elsewhere() {
    // Contextual: a field and an op may still be called `peer`.
    parse_program("entity links {\n  peer: text\n}\nop peer = links[peer]\n").expect("parses");
}

#[test]
fn a_peer_name_is_lowercase_letters_and_digits() {
    for bad in ["peer Journal", "peer notesV2", "peer \"my_peer\""] {
        let e = parse_program(&format!("{bad}\n")).expect_err(bad);
        assert!(
            e.code == PEER_NAME || e.code == PARSE_ERROR,
            "{bad}: {} {}",
            e.code,
            e.message
        );
    }
    let e = parse_program("peer notesV2\n").expect_err("camel case");
    assert_eq!(e.code, PEER_NAME, "{}", e.message);
    assert!(
        e.message.contains("ORDO_PEER_<NAME>_SCHEMA"),
        "{}",
        e.message
    );
    parse_program("peer notes2\n").expect("digits are fine");
}

#[test]
fn a_peer_line_is_declared_once_and_holds_no_schema() {
    let e = parse_program("peer journal\npeer journal\n").expect_err("twice");
    assert_eq!(e.code, PEER_NAME, "{}", e.message);

    let e = parse_program("peer journal schema \"notes\"\n").expect_err("a schema in the file");
    assert!(
        e.message.contains("ORDO_PEER_JOURNAL_SCHEMA"),
        "the refusal names the setting: {}",
        e.message
    );
}
