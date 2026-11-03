//! A version's two identities (`plan-versions.md` §1.1, increment 1).
//!
//! The source identity names a version and drives pins, plans and
//! promotion. The build identity records what one Ordo release compiled it
//! to. These tests hold the source identity to what was ruled on
//! 2026-11-02 and asked on the board's `program-versions-the-core-under-schema`
//! (asks 5 to 7).

use std::collections::BTreeMap;

use ordo_plan::artifact::{
    build_identity, build_identity_of, code_hashes, short, source_identity, to_bytes, FIRST_EDITION,
};
use serde_json::json;

const SOURCE: &str = "domain notes @ 1.0.0\n\nentity note {\n  body: text(200)\n}\n";

fn no_code() -> BTreeMap<String, String> {
    BTreeMap::new()
}

/// **The same source gives the same digest on every platform.** Pinned to a
/// literal, so macOS (where it is written) and Linux (where CI runs it) must
/// both produce these exact bytes. A change here is a change to every
/// version's name: it needs a new encoding tag, not a new literal.
/// The literal was checked against a separate Python computation of the
/// same length-prefixed encoding.
#[test]
fn the_digest_is_fixed() {
    assert_eq!(
        source_identity(SOURCE, FIRST_EDITION, &no_code()),
        "723bf4f210a67eedeac0d908d1a0fed7dca94605a0ab0e384db6dee2b0552ee6"
    );
}

#[test]
fn the_identity_is_the_full_digest() {
    let id = source_identity(SOURCE, FIRST_EDITION, &no_code());
    assert_eq!(id.len(), 64);
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(short(&id), &id[..16], "16 hex is for display");
    assert_eq!(short("abc"), "abc", "a short name shows whole");
}

/// A Windows checkout and a Linux one hold the same program.
#[test]
fn line_endings_and_a_byte_order_mark_are_evened_out() {
    let lf = source_identity(SOURCE, FIRST_EDITION, &no_code());
    let crlf = SOURCE.replace('\n', "\r\n");
    assert_eq!(source_identity(&crlf, FIRST_EDITION, &no_code()), lf);
    let bom = format!("\u{feff}{SOURCE}");
    assert_eq!(source_identity(&bom, FIRST_EDITION, &no_code()), lf);
}

/// Ask 6: the text is hashed as written, so a comment is a new version.
#[test]
fn a_comment_is_a_new_version() {
    let commented = format!("-- the notes app\n{SOURCE}");
    assert_ne!(
        source_identity(&commented, FIRST_EDITION, &no_code()),
        source_identity(SOURCE, FIRST_EDITION, &no_code())
    );
}

#[test]
fn the_edition_is_in_the_identity() {
    assert_ne!(
        source_identity(SOURCE, FIRST_EDITION, &no_code()),
        source_identity(SOURCE, FIRST_EDITION + 1, &no_code())
    );
}

/// A change to a function's code alone, or to where it lives, is a new
/// version (`plan-backend-functions.md` §4.3).
#[test]
fn function_code_and_its_path_are_in_the_identity() {
    let code = |path: &str, hash: &str| -> BTreeMap<String, String> {
        [(path.to_string(), hash.to_string())].into()
    };
    let base = source_identity(SOURCE, FIRST_EDITION, &code("fn/a.js", "sha256:01"));
    assert_ne!(base, source_identity(SOURCE, FIRST_EDITION, &no_code()));
    assert_ne!(
        base,
        source_identity(SOURCE, FIRST_EDITION, &code("fn/a.js", "sha256:02"))
    );
    assert_ne!(
        base,
        source_identity(SOURCE, FIRST_EDITION, &code("fn/b.js", "sha256:01"))
    );
}

/// Parts are length-prefixed: moving bytes from the text into a path does
/// not give the same identity.
#[test]
fn parts_cannot_be_re_cut() {
    let a: BTreeMap<String, String> = [("xy".to_string(), "h".to_string())].into();
    let b: BTreeMap<String, String> = [("y".to_string(), "h".to_string())].into();
    assert_ne!(
        source_identity("ax", FIRST_EDITION, &b),
        source_identity("a", FIRST_EDITION, &a)
    );
}

/// The code hashes a stored document carries are what the identity reads
/// back, so a stored version's identity can be taken without its files.
#[test]
fn code_hashes_read_back_from_a_document() {
    let doc = json!({"functions": [
        {"name": "hello", "code": "fn/hello.js", "code_hash": "sha256:aa"},
        {"name": "bye", "code": "fn/bye.js", "code_hash": "sha256:bb"},
    ]});
    let hashes = code_hashes(&doc);
    assert_eq!(hashes.len(), 2);
    assert_eq!(hashes["fn/hello.js"], "sha256:aa");
    assert!(code_hashes(&json!({})).is_empty());
}

/// An Ordo release moves the build identity and nothing else.
#[test]
fn the_release_moves_the_build_and_not_the_source() {
    let doc = json!({"ops": [], "entities": []});
    let a = build_identity(&doc, "CREATE TABLE t ();", "0.5.0");
    let b = build_identity(&doc, "CREATE TABLE t ();", "0.6.0");
    assert_ne!(a, b);
    assert_eq!(a.len(), 64);
    assert_eq!(
        a,
        build_identity_of(&to_bytes(&doc), "CREATE TABLE t ();", "0.5.0")
    );
}
