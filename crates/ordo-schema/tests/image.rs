//! `image_of_row`'s completeness invariant (`docs/plan-blob-chain.md` §3.3,
//! increment C2): an image's key set is exactly the schema's field set.
//!
//! `image_of_row` decodes by matching on `f.ty.value_ty()`, and its
//! fallback arm images anything it does not recognise as `Value::Text`
//! (`docs/plan-blob-chain.md` §0.4: "dates/money ride as text… the
//! executor's… encoding is F5's"). That wildcard is exactly what would let
//! a future scalar kind go through silently under-imaged if the loop
//! itself ever stopped inserting one key per field. This test pins the
//! loop's own guarantee, from the schema, so a regression there fails
//! loudly instead of shortening every image one field at a time.

use std::collections::{BTreeMap, BTreeSet};

use ordo_commandlog::chain::encode_image_bytes;
use ordo_commandlog::Value;
use ordo_schema::image::{image_of_row, ImageError, SealSink};
use ordo_schema::{build, Schema};
use ordo_syntax::parse_program;

fn schema_of(src: &str) -> Schema {
    build(&parse_program(src).expect("parses")).expect("builds")
}

/// A sink that mints a fixed, obviously-fake handle. This test is about
/// which keys an image carries, never about the handle's bytes: those are
/// random per call by design (`write.rs:2806-2817`) and covered by the
/// conformance and undo suites, not here.
struct FixedHandle;

impl SealSink for FixedHandle {
    fn seal_ref(
        &mut self,
        _entity: &str,
        _field: &str,
        _anchor: &str,
        _envelope: &[u8],
    ) -> Result<Value, ImageError> {
        Ok(Value::SealedRef([0xAB; 16]))
    }
}

/// One of each shape this crate's decode arms distinguish (`id`/`rev` come
/// for free, as implicit fields the schema always materialises): a plain
/// scalar of every kind the wildcard would swallow if it stopped being
/// exhaustive over insertion, a ref (which is an `Id` by value), a sealed
/// field with something to seal, and a sealed field with nothing in it.
/// Both must still leave a key behind.
const WIDE_FIELD: &str = "\
domain d @ 1.0.0
entity target (key: tgt) {
  name: text
}
entity widefield (key: wf) {
  who       -> target
  label:      text
  count:      int
  live:       bool
  born:       date
  secret:     text  sealed
  quiet:      text  sealed
}
";

#[test]
fn every_declared_field_is_imaged() {
    let schema = schema_of(WIDE_FIELD);
    let e = schema.entity("widefield").expect("declared above");

    // One cell per `image_columns(e)` entry, in field order: `id`, `rev`,
    // then the declared fields as written. `quiet` gets an empty `bytea`
    // ("sealed and never written"); everything else gets a real value.
    let row: Vec<Option<String>> = e
        .fields
        .iter()
        .map(|f| {
            Some(match f.name.as_str() {
                "id" => "wf_1".to_string(),
                "rev" => "1".to_string(),
                "who" => "tgt_1".to_string(),
                "label" => "hello".to_string(),
                "count" => "5".to_string(),
                "live" => "t".to_string(),
                "born" => "2020-01-01".to_string(),
                "secret" => "\\x00".to_string(),
                "quiet" => "\\x".to_string(),
                other => panic!("test schema grew a field `{other}` this row doesn't cover"),
            })
        })
        .collect();

    let img =
        image_of_row(e, &row, &BTreeMap::new(), &mut FixedHandle).expect("every cell decodes");

    let want: BTreeSet<&str> = e.fields.iter().map(|f| f.name.as_str()).collect();
    let have: BTreeSet<&str> = img.keys().map(String::as_str).collect();
    assert_eq!(
        have, want,
        "the image's key set must be exactly the schema's field set, no more and no fewer"
    );

    // The two sealed fields land on the two branches `image_of_row` gives
    // them: something to seal becomes a handle, nothing to seal becomes
    // `Null` without ever reaching the sink.
    assert_eq!(img.get("secret"), Some(&Value::SealedRef([0xAB; 16])));
    assert_eq!(img.get("quiet"), Some(&Value::Null));
    assert_eq!(img.get("who"), Some(&Value::Id("tgt_1".to_string())));
}

// ───────────────────────── the digest sibling key (C5) ──────────────────

/// One external blob field, and nothing else. The digest lookup
/// (`is_external_blob`, `docs/plan-blob-chain.md` §4.1) is the whole point
/// of this schema, so it stays narrow rather than reusing `WIDE_FIELD`,
/// which would make `every_declared_field_is_imaged`'s exact-key-set
/// assertion a second place this test's field would have to be accounted
/// for.
const BLOB_FIELD: &str = "\
domain d @ 1.0.0
entity withblob (key: wb) {
  scan: blob?
}
";

/// `id`, `rev`, then the external blob column's plain hex handle: the
/// column shape `ddl::column_ty` gives a non-inline, non-private,
/// non-sealed `blob` field, and the one `image_of_row` decodes through its
/// `Value::Text` fallback arm.
fn blob_row(handle_hex: &str) -> Vec<Option<String>> {
    vec![
        Some("wb_1".to_string()),
        Some("1".to_string()),
        Some(handle_hex.to_string()),
    ]
}

/// A sink whose returned handle is a function of the envelope it was
/// handed, so a test can tell "the envelope reached the sink" apart from
/// "the sink invented something" without needing real sealing, the same
/// trick `FixedHandle` skips because it never has to distinguish two
/// different envelopes in one test.
struct EnvelopeEchoSink;

impl SealSink for EnvelopeEchoSink {
    fn seal_ref(
        &mut self,
        _entity: &str,
        _field: &str,
        _anchor: &str,
        envelope: &[u8],
    ) -> Result<Value, ImageError> {
        let mut h = [0u8; 16];
        for (i, b) in envelope.iter().take(16).enumerate() {
            h[i] = *b;
        }
        Ok(Value::SealedRef(h))
    }
}

#[test]
fn a_blob_image_carries_a_sealed_digest() {
    let schema = schema_of(BLOB_FIELD);
    let e = schema.entity("withblob").expect("declared above");
    let handle_hex = "a1".repeat(16);
    let row = blob_row(&handle_hex);

    // Stands in for `$blobref.integrity`: the caller's lookup, already
    // done, handed in as `image_of_row`'s contract requires (§4.1: this
    // crate has no I/O to run that lookup itself).
    let envelope = vec![0x99u8; 40];
    let blob_digests = BTreeMap::from([("scan".to_string(), envelope.clone())]);

    let img = image_of_row(e, &row, &blob_digests, &mut EnvelopeEchoSink).expect("decodes");

    assert_eq!(
        img.get("scan"),
        Some(&Value::Text(handle_hex)),
        "the column's own value is unchanged: still the plain handle"
    );
    let mut want_handle = [0u8; 16];
    want_handle.copy_from_slice(&envelope[..16]);
    assert_eq!(
        img.get("scan$digest"),
        Some(&Value::SealedRef(want_handle)),
        "the digest rides as a SealedRef into $sealedpayload, never as the envelope itself"
    );
}

/// `docs/plan-blob-chain.md` §4.1: a hex digest as `Value::Text` would make
/// the plaintext command log an unsalted permanent equality index over
/// file content. This asserts the refusal over `encode_image_bytes`, the
/// actual chain bytes (the same ones `sealed_fields_never_carry_plaintext`
/// in `ordo-commandlog` checks for an ordinary sealed field), rather than
/// over the in-memory `Image`, because the in-memory `Image` is not what
/// gets hashed, stored, or ever leaves the process.
#[test]
fn the_digest_is_never_plaintext_in_the_encoded_image() {
    let schema = schema_of(BLOB_FIELD);
    let e = schema.entity("withblob").expect("declared above");
    let handle_hex = "b2".repeat(16);
    let row = blob_row(&handle_hex);

    // A distinctive envelope: not a real sealed integrity envelope's
    // shape, but that is not what this test is about. What matters is
    // that these exact bytes never appear in the encoded image, because
    // they are the thing the digest key must never carry in the clear.
    let envelope: Vec<u8> = (0u8..0xFA).step_by(7).collect();
    let blob_digests = BTreeMap::from([("scan".to_string(), envelope.clone())]);

    let img = image_of_row(e, &row, &blob_digests, &mut EnvelopeEchoSink).expect("decodes");
    let bytes = encode_image_bytes(&img);

    assert!(
        !bytes.windows(envelope.len()).any(|w| w == envelope),
        "the digest envelope leaked into the canonical encoding: it must ride behind a \
         SealedRef handle, never as bytes the chain stores directly"
    );
    // The sink's returned handle IS present. The log can name the
    // ciphertext, the same asymmetry `sealed_fields_never_carry_plaintext`
    // pins for an ordinary sealed field.
    let mut want_handle = [0u8; 16];
    want_handle.copy_from_slice(&envelope[..16]);
    assert!(bytes.windows(16).any(|w| w == want_handle));
}
