//! The one full-row image builder (`docs/plan-blob-chain.md` §3, increment
//! C2), consolidating what were two independent decoders:
//! `Fixture::row_image` (`ordo-conformance`) and `undo::row_image`
//! (`ordo-exec`), which iterated the same fields, selected the same
//! columns, and decoded the same scalar types, and agreed on the bytes of
//! an image by coincidence rather than by construction.
//!
//! What actually differed between the two was never the decoding rules: it
//! was the I/O. One builds its select and runs it synchronously over a
//! blocking `psql` harness and stages a sealed field's payload for a later
//! flush; the other runs asynchronously over a pooled connection and writes
//! a sealed field's payload immediately. Both of those are the caller's
//! business. A database handle and a key release are not things this crate
//! can hold, since `ordo-schema` has no I/O and no crypto (it depends on
//! `ordo-syntax` alone, plus, as of this module, `ordo-commandlog` for
//! [`Image`] and [`Value`], a pure crate with no `tokio`, so the `wasm`
//! build this crate ships into is unaffected).
//!
//! So the split is: this module knows how to turn a schema entity and one
//! row of raw column text into an [`Image`], and it knows how to build the
//! `SELECT` that row came from, always scoped to a space, because a select
//! with no way to omit the scope is a select that cannot repeat undo's old
//! bug (`docs/plan-blob-chain.md` §0.4 item 1, fixed by hand in C1, fixed by
//! construction here). What it does NOT know is how a sealed field's
//! envelope becomes a handle: that is a key release and a write to the
//! payload store, and it crosses the [`SealSink`] seam into the caller.
//!
//! C5 adds a second thing this module does not know how to do: read
//! `$blobref`. An external blob column's digest (`"{field}$digest"`, §4.1)
//! comes from that table, and `ordo-schema` cannot issue SQL any more than
//! it can release a key, so [`image_of_row`] takes the answer as a plain
//! `BTreeMap<String, Vec<u8>>`, a caller's completed lookup rather than a
//! seam this module drives. See [`is_external_blob`] and [`image_of_row`]'s
//! own doc for the shape of that contract.

use std::collections::BTreeMap;

use ordo_commandlog::{Image, Value};
use ordo_syntax::decl::ScalarTy;

use crate::ddl::q;
use crate::{Entity, Field, SPACE_COLUMN};

/// Why [`image_of_row`] could not finish an image.
///
/// The first two variants are facts about the stored bytes, checked here
/// because decoding them is exactly this module's job. [`ImageError::Sink`]
/// is not: it is whatever the caller's [`SealSink`] refused to do (no key
/// custody, a failed mint, a failed write to the payload store), carried
/// through as text because the two implementors' own error types
/// (`String` in `ordo-conformance`, `RevertFailure` in `ordo-exec`) are not
/// visible from a crate three layers below either of them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageError {
    /// `entity.field` is declared `sealed` and the stored cell is not a
    /// well-formed hex `bytea` rendering at all.
    NotSealed { entity: String, field: String },
    /// `entity.field`'s stored text did not parse as its declared scalar
    /// type.
    BadValue {
        entity: String,
        field: String,
        reason: &'static str,
    },
    /// The sink refused a sealed field. Its message.
    Sink(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::NotSealed { entity, field } => {
                write!(f, "internal: `{entity}.{field}` is not a sealed envelope")
            }
            ImageError::BadValue {
                entity,
                field,
                reason,
            } => write!(f, "internal: `{entity}.{field}` {reason}"),
            ImageError::Sink(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ImageError {}

/// Where a sealed field's envelope goes, and where the handle that names it
/// in the image comes from.
///
/// [`image_of_row`] decodes; it never touches a payload store, releases a
/// key, or mints a handle. That is entirely I/O, and entirely the caller's.
/// The write protocol's sink stages the payload into its own write log,
/// flushed once the enclosing group has a correlation; the revert
/// applier's sink inserts it immediately, under the revert's own
/// correlation. Both obey the one rule `ordo_crypto::sealed` states: the
/// I/O is two shapes, the rule is one.
pub trait SealSink {
    /// Seal `envelope` (the field's stored bytes, already decoded out of
    /// Postgres's hex `bytea` rendering) for `entity.field`, and return
    /// the handle the image records in its place.
    ///
    /// `anchor` is the label of what the envelope was sealed at, in the
    /// form `ordo_crypto::Anchor::label` produces: `row:<id>` for a column
    /// value. The payload keeps it, because the envelope opens only at the
    /// anchor it was sealed at. This crate cannot name `ordo_crypto`, so the
    /// label crosses as text.
    fn seal_ref(
        &mut self,
        entity: &str,
        field: &str,
        anchor: &str,
        envelope: &[u8],
    ) -> Result<Value, ImageError>;
}

/// The columns an image reads: every field the schema declares for `e`, in
/// declaration order. `id` and `rev`, being implicit fields, are already in
/// this list: they are not physical-column afterthoughts the way `$space`
/// and `$path` are.
pub fn image_columns(e: &Entity) -> Vec<&Field> {
    e.fields.iter().collect()
}

/// Does `f`'s column hold an external blob handle, one this space's
/// `$blobref` names and therefore one [`image_of_row`] can be handed a
/// digest for?
///
/// `blob sealed` is excluded even though it is content: `ddl::column_ty`
/// gives it `bytea` and images it through the sealed branch instead
/// (checked first, `f.sealed`), because a sealed handle is minted fresh
/// per call and carries no `$blobref` row at all (`docs/plan-blob-
/// chain.md` §7.4: it is refused at the door and modelled by nothing).
/// `inline` and `private` are excluded for the same reason: neither tier
/// ever produces a `$blobref` row (`inline`'s bytes are a `$sealedpayload`
/// row instead, and `private` never reaches a platform-held reference at
/// all), so there is nothing here for a caller to look up.
pub fn is_external_blob(f: &Field) -> bool {
    f.is_content() && !f.sealed && !f.inline && !f.private
}

/// Decode Postgres's text rendering of a `bytea` (`\x0a1b…`).
///
/// A duplicate of `ordo_crypto::sealed::unhex` by design, not by oversight:
/// this is Postgres's wire format for a byte string, not a cryptographic
/// operation, and importing `ordo-crypto` for ten lines of hex parsing
/// would reattach the exact dependency this module's doc comment says the
/// `wasm` build cannot carry. The two copies decode the same bytes the same
/// way because the format is Postgres's, not either crate's.
fn unhex(text: &str) -> Option<Vec<u8>> {
    let body = text.strip_prefix("\\x")?;
    if body.len() % 2 != 0 {
        return None;
    }
    (0..body.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(body.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The full field image of one row, decoded from `row`: one cell per
/// [`image_columns`] entry, in that order, exactly as `image_select`'s
/// column list produces them.
///
/// A sealed field's non-empty cell is handed to `sink` for sealing; an
/// empty `bytea` (a sealed field that has never held a value) images as
/// [`Value::Null`] without calling the sink at all, the same rule both
/// predecessor builders applied. There is no envelope to name, so there is
/// no handle to mint.
///
/// # The digest sibling key (`docs/plan-blob-chain.md` §4.1, C5)
///
/// A non-null [`is_external_blob`] column also gets a second image entry,
/// `"{field}$digest"`, holding `$blobref.integrity` for the handle the
/// column names: the **verbatim** envelope, sealed once at upload time
/// and never re-sealed here, exactly as a `sealed` field's own envelope is
/// copied rather than re-encrypted. `sink.seal_ref` is what does that
/// copying (it already exists for exactly this: "the image names these
/// exact bytes"), called with the field's **real** name, not the `$digest`
/// key, because the envelope is AAD-bound to `(entity, field)` at the
/// pipeline that sealed it (`ordo-blob/src/pipeline.rs`) and opening it
/// later needs that same pair. The `$digest` suffix exists only as a map
/// key, never as an AAD input.
///
/// `blob_digests` is the caller's answer to "what does `$blobref` say", one
/// entry per field name for which a lookup found a row. See the module
/// doc for why this crate cannot run that lookup itself. A non-null
/// external blob column absent from the map images with no digest key at
/// all rather than refusing: this builder trusts the map it was handed and
/// makes no claim about why an entry is missing. Every caller in this tree
/// populates it completely (a lookup miss is the caller's own refusal,
/// upstream of this function), so the only way a live column reaches here
/// digest-less is a caller that has not done its half of the work. That
/// is a caller bug, not a case this function can distinguish from "no
/// digest was ever recorded" (§4.2's pre-C5 images, which look identical
/// and are meant to).
pub fn image_of_row(
    e: &Entity,
    row: &[Option<String>],
    blob_digests: &BTreeMap<String, Vec<u8>>,
    sink: &mut dyn SealSink,
) -> Result<Image, ImageError> {
    let cols = image_columns(e);
    let mut img: Image = BTreeMap::new();
    // The row's own id, which a sealed value is bound to. Read from the
    // row rather than passed in, so no caller can image one row under
    // another's label.
    let row_id = cols
        .iter()
        .zip(row)
        .find(|(f, _)| f.name == "id")
        .and_then(|(_, cell)| cell.clone())
        .unwrap_or_default();
    let row_anchor = format!("row:{row_id}");
    for (f, cell) in cols.iter().zip(row) {
        let v = match cell {
            None => Value::Null,
            Some(raw) if f.sealed => {
                let bytes = unhex(raw).ok_or_else(|| ImageError::NotSealed {
                    entity: e.name.clone(),
                    field: f.name.clone(),
                })?;
                if bytes.is_empty() {
                    Value::Null
                } else {
                    sink.seal_ref(&e.name, &f.name, &row_anchor, &bytes)?
                }
            }
            Some(raw) => match f.ty.value_ty() {
                ScalarTy::Id => Value::Id(raw.clone()),
                ScalarTy::Bool => Value::Bool(raw == "t" || raw == "true"),
                ScalarTy::Int => Value::Int(raw.parse().map_err(|_| ImageError::BadValue {
                    entity: e.name.clone(),
                    field: f.name.clone(),
                    reason: "is not an integer",
                })?),
                // A `decimal(p)` rides the chain's decimal tag, in its one
                // canonical spelling: Postgres prints `0.0800` at scale 4 and
                // the chain records `0.08` (27ak).
                ScalarTy::Decimal(_) => match ordo_commandlog::value::Decimal::parse(raw) {
                    Some(d) => Value::Decimal(d),
                    None => {
                        return Err(ImageError::BadValue {
                            entity: e.name.clone(),
                            field: f.name.clone(),
                            reason: "is not a decimal",
                        })
                    }
                },
                // Dates/money ride as text in the conformance images; the
                // executor's epoch/minor-unit encoding is F5's, unchanged
                // by this consolidation.
                _ => Value::Text(raw.clone()),
            },
        };
        if cell.is_some() && is_external_blob(f) {
            if let (Some(envelope), Some(handle)) = (blob_digests.get(&f.name), cell) {
                // The digest is the reference row's `integrity`, which is
                // sealed at the object it describes, so its payload keeps
                // that object's label. The handle is the column's own
                // text, in the lowercase hex the platform writes it in.
                let anchor = format!("obj:{}", handle.to_ascii_lowercase());
                let digest = sink.seal_ref(&e.name, &f.name, &anchor, envelope)?;
                img.insert(format!("{}$digest", f.name), digest);
            }
        }
        img.insert(f.name.clone(), v);
    }
    Ok(img)
}

/// A SQL string literal, NFC-normalised before quoting.
///
/// `ordo-conformance`'s own literal quoter does this ("the last gate before
/// a text value becomes part of a statement", `write.rs`'s `sql_str`); the
/// undo applier's did not. Both `id` and a space's name are minted ASCII
/// (`format!("{}_w{}", …)`, a space's own literal), so this is a no-op for
/// every value either caller has ever passed, and defence in depth for the
/// caller: `image_select` gains a guarantee it did not have. The select
/// this module builds is the one funnel both callers now share, and a
/// value that reached it un-normalised would have no other gate
/// downstream.
fn lit(s: &str) -> String {
    format!("'{}'", ordo_syntax::nfc(s).replace('\'', "''"))
}

/// The `SELECT` an image is read from: every declared column of `e`, from
/// this space's row at `id`, and only this space's.
///
/// The space predicate is inside the string on purpose: there is no
/// signature by which a caller can build this select without naming a
/// space. That is undo's old bug (`docs/plan-blob-chain.md` §0.4 item 1),
/// closed by construction instead of by a call-site convention to
/// remember.
pub fn image_select(e: &Entity, id: &str, space: &str) -> String {
    let col_sql = image_columns(e)
        .iter()
        .map(|f| q(&f.column()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT {col_sql} FROM {} WHERE {} = {} AND {} = {}",
        q(&e.name),
        q(SPACE_COLUMN),
        lit(space),
        q("id"),
        lit(id),
    )
}
