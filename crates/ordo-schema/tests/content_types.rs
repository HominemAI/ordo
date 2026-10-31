//! `blob` and `uri` — the physical shape, and every refusal by name.
//!
//! `plan-blob.md` B2's closing condition is that
//! `entity doc (key: doc) { file: blob sealed  avatar: blob inline
//! link: uri }` parses, binds and emits DDL. This is that, plus the
//! refusals — because the refusals are the part a wildcard would have
//! swallowed, and a type-add that only proves the happy path has proved
//! the half that was never in doubt.
//!
//! Asserted here rather than through the conformance harness for
//! `graph.rs`'s reason: a failure should point at the schema layer rather
//! than at whichever corpus case happened to notice.

use ordo_schema::{build, ddl_for};
use ordo_syntax::parse_program;

fn schema_of(src: &str) -> ordo_schema::Schema {
    build(&parse_program(src).expect("parses")).expect("builds")
}

fn ddl_of(src: &str) -> String {
    ddl_for(&schema_of(src)).expect("ddl")
}

fn err_code(src: &str) -> &'static str {
    match parse_program(src) {
        Ok(p) => build(&p).expect_err("expected a schema error").code,
        Err(e) => panic!("expected a schema error, got a parse error: {}", e.message),
    }
}

const DOC: &str = "\
domain d @ 1.0.0
entity doc (key: doc) {
  file:   blob?  sealed
  avatar: blob?  inline
  scan:   blob?
  link:   uri?
  canon:  uri?   sealed indexed unique normalize(lower)
}
";

/// The plan's own acceptance sentence, and the four column shapes it
/// produces. **One field, one column** — never a `(handle, mime, size,
/// hash)` family, because `Change::AddOptionalField` names a field and
/// `live_drift` compares columns one for one, so four siblings would make
/// one field add read as four changes on the class-A additive path.
#[test]
fn the_four_content_tiers_each_get_exactly_one_column() {
    let ddl = ddl_of(DOC);
    // External blob: the hex handle, in `text`. Not `bytea`, so an
    // operator reading the schema can tell a sealed blob from an open one.
    assert!(ddl.contains("\"scan\" text"), "{ddl}");
    // Sealed blob: the handle inside an envelope.
    assert!(ddl.contains("\"file\" bytea"), "{ddl}");
    // Inline blob: the payload-store handle, raw, so the join to
    // `$sealedpayload."ref"` is bytes to bytes with nothing to decode.
    assert!(ddl.contains("\"avatar\" bytea"), "{ddl}");
    // An address is text.
    assert!(ddl.contains("\"link\" text"), "{ddl}");
    // A sealed address is an envelope, and — the load-bearing one — it
    // gets a blind index, which is the whole difference between an address
    // and a handle.
    assert!(ddl.contains("\"canon\" bytea"), "{ddl}");
    assert!(ddl.contains("\"canon$bidx\""), "{ddl}");

    // And no sibling columns crept in under any spelling.
    for ghost in [
        "\"file$mime\"",
        "\"file$size\"",
        "\"file$hash\"",
        "\"scan$mime\"",
    ] {
        assert!(
            !ddl.contains(ghost),
            "a metadata sibling column exists: {ghost}"
        );
    }
}

/// A `uri` is an ADDRESS, and an address is a value. It seals, it
/// blind-indexes, it normalizes, it takes a uniqueness constraint. Every
/// one of those is refused for a `blob`, and that contrast is the type
/// distinction rather than a coincidence of implementation.
#[test]
fn a_uri_is_a_value_and_a_blob_is_not() {
    let s = schema_of(DOC);
    let e = s.entity("doc").expect("doc");
    let canon = e.field("canon").expect("canon");
    assert!(canon.sealed && canon.indexed && canon.unique);
    assert!(!canon.is_content(), "an address is not content");
    assert!(e.field("scan").expect("scan").is_content());
    assert!(e.field("avatar").expect("avatar").inline);
    assert!(!e.field("file").expect("file").inline);
}

/// Each refusal, by class. The messages take the `rule:` / `fix:` shape
/// `adoption-dx-plan.md` §3.3 commits to, and each class is in
/// `definition-language.md` §2's table — which is not decoration:
/// `ordo-conformance`'s `no_new_refusal_class_arrives_undocumented`
/// enforces it.
#[test]
fn every_blob_refusal_has_its_own_class() {
    let entity = |body: &str| format!("domain d @ 1.0.0\nentity doc (key: doc) {{\n{body}\n}}\n");

    assert_eq!(
        err_code(&entity("  f: blob? unique")),
        "blob_not_indexable",
        "a blind index over a keyed-random handle indexes randomness"
    );
    assert_eq!(
        err_code(&entity("  f: blob? sealed indexed")),
        "blob_not_indexable"
    );
    assert_eq!(
        err_code(&entity("  f: blob? = \"deadbeef\"")),
        "blob_default",
        "a literal here names an object nobody uploaded"
    );
    assert_eq!(
        err_code(&entity("  f: blob? normalize(lower)")),
        "blob_not_normalizable"
    );
    assert_eq!(
        err_code(&entity("  f: text inline")),
        "inline_without_blob",
        "`inline` chooses where BYTES live, and nothing else has bytes"
    );
    assert_eq!(
        err_code(&entity("  f: blob")),
        "blob_required",
        "an upload addresses an existing row, so the field is empty until it lands"
    );
    assert_eq!(
        err_code(&entity("  f: text? private")),
        "private_without_blob",
        "`private` is custody over BYTES; `sealed` is the space-key form for a value"
    );
    assert_eq!(
        err_code(&entity("  f: blob? sealed private")),
        "conflicting_custody",
        "two custody positions, not a stronger one"
    );
}

/// **`blob private` is declarable.** A language that cannot spell the tier
/// cannot refuse it by name, and "unknown modifier `private`" is a worse
/// answer than "the key ceremony has not landed". So it parses, binds, and
/// gets a column; every runtime path that would have to touch the
/// plaintext refuses with the reason (`ordo_host::blob`, `ordo_exec::export`).
#[test]
fn a_private_blob_declares_and_carries_its_tier() {
    let s = schema_of("domain d @ 1.0.0\nentity doc (key: doc) { vault: blob? private }\n");
    let f = s
        .entity("doc")
        .and_then(|e| e.field("vault"))
        .expect("vault");
    assert!(f.private);
    assert!(!f.sealed, "`private` is not the space key wearing a hat");
    assert!(f.is_content());
    // The column is the hex handle, exactly as an external `blob`'s is:
    // the platform stores an object it cannot open, and the reference to
    // it is not itself a secret.
    let ddl = ddl_of("domain d @ 1.0.0\nentity doc (key: doc) { vault: blob? private }\n");
    assert!(ddl.contains("\"vault\" text"), "{ddl}");
}

/// The `uri` half of the same table: none of it applies.
#[test]
fn a_uri_refuses_nothing_a_text_accepts() {
    let src = "\
domain d @ 1.0.0
entity doc (key: doc) {
  a: uri unique
  b: uri? sealed indexed normalize(lower)
  c: uri = \"https://example.com/\"
}
";
    let ddl = ddl_of(src);
    assert!(ddl.contains("\"a\" text"), "{ddl}");
    assert!(ddl.contains("\"b\" bytea"), "{ddl}");
    assert!(ddl.contains("'https://example.com/'"), "{ddl}");
}

/// The derived CRUD ops skip content fields, and the exclusion is only
/// sound because `blob_required` forces every blob field to admit null —
/// otherwise `std.doc.create` would be missing a required field and stop
/// being insert-complete.
///
/// Including a blob in `std.doc.update` would be worse than merely
/// refusing to register: a full-row CAS converge sets every field in the
/// braces, so every unrelated edit would either re-send the handle (the
/// caller naming an object it did not upload) or null the file.
#[test]
fn derived_writes_do_not_name_a_blob_field() {
    let s = schema_of(DOC);
    let ops = ordo_schema::std_ops(&s).ops;
    let names: Vec<&str> = ops.iter().map(|o| o.name.name.as_str()).collect();
    assert!(names.contains(&"std.doc.create"), "{names:?}");

    for op in &ops {
        for p in &op.params {
            assert_ne!(
                p.ty,
                ordo_syntax::ScalarTy::Blob,
                "`{}` declares `${}: blob`, which the compiler refuses",
                op.name.name,
                p.name.name
            );
        }
    }
    // The `uri` fields DO appear: an address is a value, so a derived
    // create takes one.
    let create = ops
        .iter()
        .find(|o| o.name.name == "std.doc.create")
        .expect("create");
    assert!(
        create.params.iter().any(|p| p.name.name == "link"),
        "a `uri` is an ordinary parameter and must be settable"
    );
    assert!(
        !create.params.iter().any(|p| p.name.name == "scan"),
        "a blob field must not reach a derived write"
    );
}
