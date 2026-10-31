//! The blob reference table: what a blob handle resolves *through*.
//!
//! A row never holds a blob's bytes. It holds a 16-byte opaque handle
//! ([`custody::KeyProvider::mint_blob_ref`]), and the metadata that handle
//! names lives here — one row per upload, in the space that wrote it,
//! **never deduplicated**. The bytes live in the space's resolved object
//! store, under `<prefix>/<hex handle>` and nothing else.
//!
//! # Why the shape lives in this crate
//!
//! [`crate::sealed`]'s reason, unchanged: two crates create this table and
//! neither can see the other. `ordo-conformance` builds its database
//! through `psql` and has no driver; `ordo-exec` owns the pool and creates
//! the platform tables at boot, and it *depends on* `ordo-conformance`
//! rather than the reverse. `ordo-crypto` is the lowest crate both can see
//! and it already owns the handle, so it owns the handle's storage shape
//! too. It stays driver-free: everything here is a `String`.
//!
//! It deliberately does **not** live in `ordo-blob`. That crate owns the
//! *bytes* tier — an HTTP client, a TLS stack, a SigV4 signer — and
//! nothing that creates a Postgres table should have to depend on any of
//! those. The split is the same one `KeyProvider` / `WrappedKeyStore`
//! draws: a deployment's storage choice and its custody choice are
//! independent, and so are the crates.
//!
//! # Never deduplicate, never content-address
//!
//! There is no unique index on any metadata column, no content-hash
//! column, and no upsert anywhere in this module. See
//! [`custody::KeyProvider::mint_blob_ref`] for the argument in full: a
//! content-derived handle is an unsalted permanent equality oracle over
//! sealed bytes that **survives crypto-erasure**, and it is strictly worse
//! for a document than for a column. Two uploads of the same file store it
//! twice. The cost is stated rather than hidden, and it is larger in
//! absolute terms than [`crate::sealed`]'s, which is why the shape summary
//! prices `inline` against external.
//!
//! # What is sealed and what is not
//!
//! `mime`, `size` and `integrity` are **envelopes** when the field is
//! sealed, plaintext otherwise. `size` is sealed for the same reason a
//! value is: file sizes fingerprint documents, and a plaintext size column
//! beside a sealed blob would answer "is this the standard NDA?" without a
//! key. `integrity` is the "sealed integrity hash"
//! `definition-language.md` §2 names — sealed is the operative word,
//! because a plaintext digest column *is* the content hash by another
//! name, complete with the cross-tenant equality it was rejected for.
//!
//! `bytes` is plaintext and is the **ciphertext** length, not the
//! plaintext's: the store needs it to serve a range, and it is already
//! observable to anyone who can list the bucket, so sealing it would buy
//! nothing and cost a range read. `epoch`, `entity` and `field` are
//! plaintext exactly as [`crate::sealed`] carries them, and for the same
//! two reasons: an operator gating `destroy_epoch` has to be able to count
//! what an epoch still holds, and the envelope's AAD is bound to
//! `(entity, field)` so a resolver must not have to be told which column
//! it is opening for.
//!
//! # The object key carries none of it
//!
//! `<space-key-prefix>/<hex handle>`. No entity, no field, no date, no
//! epoch. The epoch is left out for
//! [`custody::KeyProvider::mint_sealed_ref`]'s reason (3) — rotation is
//! mandatory on membership revocation, so an epoch visible in a bucket
//! listing prints a **revocation timeline** to someone holding no keys.
//! Entity and field are left out because a bucket listing would otherwise
//! enumerate the schema to anyone who can list it.
//!
//! # Each row names the store it was written to
//!
//! `"store"` on both tables is the name of the store the object went to,
//! as the resolver that wrote it spelled it (`ordo_exec::bucket` gives an
//! attached bucket's name; the empty string is the deployment's own store).
//! Reads, the sweep, erasure, export and the re-seal pass follow that name
//! rather than asking where the space's uploads go **now** (ruled
//! 2026-10-05, `detach-upload-race` ask 2). An upload that lands while the
//! tenant's bucket is being detached or replaced is written to the old
//! bucket, and its row says so, so it stays readable and collectable where
//! it landed. Nothing is checked or refused at upload.
//!
//! The tombstone carries the name too, because an upload's declared intent
//! is written before any `$blobref` row exists, and the sweep has to know
//! where to delete an interrupted upload's bytes from.
//!
//! `migration.md` §6's "blob bytes, laid out by entity/field/id" describes
//! the **export bundle's** layout, not the bucket's, and the export can
//! produce it because it holds these rows.

use crate::custody::{BlobHandle, Epoch, SpaceId};

/// The table's name. `$`-prefixed like every platform table, so it cannot
/// collide with an entity (no schema can declare one).
pub const BLOB_REF_TABLE: &str = "$blobref";

/// The tombstone table's name. See [`BLOB_TOMB_DDL`].
pub const BLOB_TOMB_TABLE: &str = "$blobtomb";

/// The per-space tenancy column, spelled the way entity tables spell it —
/// [`crate::sealed::SEALED_SPACE_COLUMN`]'s reason, unchanged: the write
/// protocol's audit fails any statement naming a table without
/// constraining `ordo_schema::SPACE_COLUMN`, and matching that spelling
/// means a blob reference insert is confined by the same mechanism that
/// confines the row insert beside it, rather than by a reviewer noticing.
pub const BLOB_SPACE_COLUMN: &str = "$space";

/// The reference store, per space, never deduplicating.
///
/// `PRIMARY KEY ("$space", "ref")` is the per-space composite shape the
/// schema uses everywhere, and it is the second of the two independent
/// mechanisms that scope a handle: the derivation is keyed per space, and
/// the key is composite. Two spaces cannot address each other's objects
/// even if a handle somehow repeated.
///
/// There is **no** index on `"integrity"`, `"mime"` or `"size"` — an index
/// over a sealed envelope is an equality index over the value inside it,
/// which is the oracle this whole design exists to refuse. The absence is
/// asserted by a test that reads this string.
pub const BLOB_REF_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$blobref\" ( \
  \"$space\" text NOT NULL, \
  \"ref\" bytea NOT NULL, \
  \"epoch\" smallint NOT NULL, \
  \"entity\" text NOT NULL, \
  \"field\" text NOT NULL, \
  \"mime\" bytea NOT NULL, \
  \"size\" bytea NOT NULL, \
  \"integrity\" bytea NOT NULL, \
  \"bytes\" bigint NOT NULL, \
  \"created_at\" bigint NOT NULL, \
  \"store\" text NOT NULL DEFAULT '', \
  PRIMARY KEY (\"$space\", \"ref\") \
); \
ALTER TABLE \"$blobref\" ADD COLUMN IF NOT EXISTS \"store\" text NOT NULL DEFAULT ''; \
CREATE INDEX IF NOT EXISTS \"$blobref_epoch\" \
  ON \"$blobref\" (\"$space\", \"epoch\"); \
CREATE INDEX IF NOT EXISTS \"$blobref_field\" \
  ON \"$blobref\" (\"$space\", \"entity\", \"field\")";

/// The two-phase sweep's tombstone table.
///
/// Collection of a `$sealedpayload` row happens *inside* the pruning
/// transaction, because deleting the log first would leave payloads
/// nothing can name (`ordo_exec::prune`). Bytes in a bucket cannot join
/// that transaction, so for blobs the safe order **inverts**: mark first,
/// delete the object, delete the reference last.
///
/// - Phase 1 is transactional and Postgres-only: the anti-join finds
///   handles with no live row and no retained command image and inserts a
///   tombstone. Nothing in the bucket is touched, so a crash here costs
///   nothing.
/// - Phase 2 is out of transaction and idempotent: for each tombstone
///   whose `unreferenced_since` is below the prune floor, re-check the
///   anti-join, delete the object, then stamp `deleted_at`. A crash
///   between the delete and the stamp leaves the tombstone open and the
///   sweep retries; bucket deletes are idempotent.
///
/// The asymmetry is deliberate and worth stating: this order can leak an
/// object, which costs money; the other order can break a revert, which
/// costs a guarantee.
pub const BLOB_TOMB_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$blobtomb\" ( \
  \"$space\" text NOT NULL, \
  \"ref\" bytea NOT NULL, \
  \"unreferenced_since\" bigint NOT NULL, \
  \"deleted_at\" bigint, \
  \"store\" text NOT NULL DEFAULT '', \
  PRIMARY KEY (\"$space\", \"ref\") \
); \
ALTER TABLE \"$blobtomb\" ADD COLUMN IF NOT EXISTS \"store\" text NOT NULL DEFAULT ''; \
CREATE INDEX IF NOT EXISTS \"$blobtomb_open\" \
  ON \"$blobtomb\" (\"$space\", \"unreferenced_since\") \
  WHERE \"deleted_at\" IS NULL";

/// One reference row, as the store hands it back.
///
/// `entity` and `field` are not decoration: every envelope on the row is
/// AAD-bound to them, so opening one requires naming the column it was
/// sealed for, and a reference that has been moved to another column
/// refuses to open. Keeping them beside the bytes means a resolver does
/// not have to be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRef {
    pub epoch: Epoch,
    pub entity: String,
    pub field: String,
    /// The mime type, as an envelope when the field is sealed and as
    /// plain UTF-8 bytes otherwise. Never interpreted here.
    pub mime: Vec<u8>,
    /// The **plaintext** byte length, same encoding rule as `mime`.
    pub size: Vec<u8>,
    /// The integrity digest over the plaintext, same encoding rule.
    pub integrity: Vec<u8>,
    /// The stored object's length — ciphertext, plaintext and observable.
    pub bytes: i64,
    pub created_at: i64,
    /// The name of the store the object was written to. Empty for the
    /// deployment's own store. See the module doc.
    pub store: String,
}

/// A single-quoted SQL string literal.
fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Store one reference under the handle a row's column will carry.
///
/// An `INSERT`, never an upsert — [`crate::sealed::insert_sql`]'s rule,
/// and here it carries a second meaning. `definition-language.md` §2 says
/// *"blobs are immutable (new content = new blob id → serving URLs are
/// permanently cacheable)"*, and that sentence reads as though content
/// addressing were the mechanism. It is not, and it does not need to be:
/// **immutability comes from never reusing a handle, not from deriving
/// one.** A keyed-random handle minted per upload, inserted and never
/// upserted, gives exactly the property the parenthesis claims.
///
/// # It cancels the upload's declared intent, in the same statement
///
/// The leading CTE deletes the `$blobtomb` row [`tomb_declare_sql`] wrote
/// before the bytes were stored. The two are one statement rather than two
/// because of what a surviving intent would mean: a tombstone whose
/// `unreferenced_since` is the instant the **upload started**, sitting
/// beside a live reference. `ordo_exec::blobgc`'s floor compares that
/// instant against the undo horizon, and its soundness rests on *"any
/// command image that could restore the handle is at or before that
/// instant"*. For an intent row that premise is false in the one direction
/// that costs a guarantee: the upload's own converge group is appended
/// **after** the intent, and so is every later delete. Leave the intent
/// standing and the sweep prices a delete made a year from now against a
/// timestamp from today, which deletes the bytes a reverted delete would
/// have needed.
///
/// Putting the cancel in a separate statement after the commit would leave
/// that window open for however long the gap is, and a crash inside the gap
/// would leave it open forever. In the CTE it cannot be open at all: either
/// the reference row exists and the intent does not, or neither does.
///
/// It is unconditional rather than a flag the caller passes, because the
/// property is about the *table*, not about the door: no path may create a
/// reference row and leave an older intent for the same handle behind. A
/// handle with no intent deletes nothing, which is every caller but the
/// door.
///
/// Closed tombstones (`deleted_at` not null) are left alone. Those are
/// phase 2's own crash-recovery record, they name an object already removed
/// from the store, and no write path has business erasing one.
#[allow(clippy::too_many_arguments)]
pub fn insert_sql(
    space: &SpaceId,
    handle: &BlobHandle,
    epoch: Epoch,
    entity: &str,
    field: &str,
    mime: &[u8],
    size: &[u8],
    integrity: &[u8],
    bytes: i64,
    created_at: i64,
    store: &str,
) -> String {
    format!(
        "WITH \"cancelled_intent\" AS ( \
           DELETE FROM \"{BLOB_TOMB_TABLE}\" \
           WHERE \"{BLOB_SPACE_COLUMN}\" = {sp} AND \"ref\" = {r} \
             AND \"deleted_at\" IS NULL \
           RETURNING 1 \
         ) \
         INSERT INTO \"{BLOB_REF_TABLE}\" \
         (\"{BLOB_SPACE_COLUMN}\", \"ref\", \"epoch\", \"entity\", \"field\", \
          \"mime\", \"size\", \"integrity\", \"bytes\", \"created_at\", \"store\") \
         VALUES ({sp}, {r}, {epoch}, {}, {}, {}, {}, {}, {bytes}, {created_at}, {})",
        lit(entity),
        lit(field),
        crate::sealed::bytea_lit(mime),
        crate::sealed::bytea_lit(size),
        crate::sealed::bytea_lit(integrity),
        lit(store),
        sp = lit(&space.0),
        r = crate::sealed::bytea_lit(handle),
    )
}

/// Declare the intent to write one object, **before a byte reaches the
/// store**.
///
/// # The leak this closes
///
/// `BlobPipeline::put` writes the object and writes nothing to the
/// database; the caller writes the reference row and the entity column in
/// one transaction, because a reference row and a column that disagree is
/// worse than either alone. So the order is object first, reference second,
/// and an upload interrupted between them leaves an object no `$blobref`
/// row names. [`crate::custody::BlobHandle`] is keyed-random per upload and
/// `BlobStore` has no `list` by design, so nothing in the deployment can
/// enumerate that object: not the sweep, not an export, not an operator. It
/// is unreachable for as long as the bucket exists, and on a tenant's own
/// bucket it is a line on the tenant's bill for bytes the platform will
/// never account for and cannot delete.
///
/// A row written here before the `put` makes it reachable, with no second
/// mechanism and no `list`: an interrupted upload leaves an ordinary
/// tombstone, and the ordinary sweep retires it on the ordinary floor. The
/// cost is one insert per upload.
///
/// # What distinguishes an intent from a sweep's tombstone, without a
/// column saying so
///
/// Phase 1 of the sweep only ever tombstones handles it read **out of**
/// `$blobref`, so its tombstones always have a reference row beside them.
/// An intent is written before the reference row exists, so it has none. A
/// `$blobtomb` row with no `$blobref` row is therefore exactly "an upload
/// that never committed", and phase 2 needs no new branch to serve it: the
/// live-reference re-check comes back false, the object is deleted, and the
/// `DELETE` of the reference row removes nothing because there was never
/// one. That is why `$blobtomb` needed no new column and no DDL change to
/// carry a second kind of row.
///
/// # Why the floor still binds an intent, though nothing could revert into
/// one
///
/// An intent that survives means no reference row committed, which means
/// no command group was appended (the group and the reference land in the
/// same transaction), which means no retained image anywhere names the
/// handle and no revert can restore it. The bytes could safely be deleted
/// at once. They are not, because the sweep has one rule and a second one
/// would have to be argued and tested separately for the sake of retiring
/// abandoned bytes ninety days sooner. The floor costs nothing here but
/// storage, and it is the conservative direction.
///
/// `store` names the store the bytes are about to go to, so an interrupted
/// upload is deleted from the bucket it landed in.
pub fn tomb_declare_sql(space: &SpaceId, handle: &BlobHandle, at: i64, store: &str) -> String {
    format!(
        "INSERT INTO \"{BLOB_TOMB_TABLE}\" \
         (\"{BLOB_SPACE_COLUMN}\", \"ref\", \"unreferenced_since\", \"store\") \
         VALUES ({}, {}, {at}, {})",
        lit(&space.0),
        crate::sealed::bytea_lit(handle),
        lit(store),
    )
}

/// Look one handle up, confined to its space.
///
/// The space predicate is what makes a handle un-transferable between
/// tenants: even a caller who somehow learned another space's handle reads
/// nothing here, because the lookup is keyed on the pair.
pub fn lookup_sql(space: &SpaceId, handle: &BlobHandle) -> String {
    format!(
        "SELECT \"epoch\", \"entity\", \"field\", \"mime\", \"size\", \"integrity\", \
                \"bytes\", \"created_at\", \"store\" \
         FROM \"{BLOB_REF_TABLE}\" \
         WHERE \"{BLOB_SPACE_COLUMN}\" = {} AND \"ref\" = {}",
        lit(&space.0),
        crate::sealed::bytea_lit(handle),
    )
}

/// Decode an external blob column's stored text back into the handle
/// [`lookup_sql`] takes.
///
/// `ddl::column_ty` gives an external (non-inline) blob field a `text`
/// column holding the handle's **plain** hex rendering, no `\x` prefix,
/// unlike Postgres's own `bytea` wire format (`crate::sealed::unhex`
/// decodes that one). This is the inverse of [`crate::hex`], which is what
/// wrote the column in the first place (`ordo-host`'s `mint`).
///
/// Used by every caller that images a row: a non-null external blob
/// column names a `$blobref` row, and finding it is the one extra read
/// `docs/plan-blob-chain.md` §4.1 asks for per image.
pub fn parse_handle(hex: &str) -> Option<BlobHandle> {
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Decode a [`lookup_sql`] row.
pub fn blob_ref_of(row: &[Option<String>]) -> Option<BlobRef> {
    let cell = |i: usize| row.get(i)?.as_deref();
    Some(BlobRef {
        epoch: cell(0)?.parse().ok()?,
        entity: cell(1)?.to_string(),
        field: cell(2)?.to_string(),
        mime: crate::sealed::unhex(cell(3)?)?,
        size: crate::sealed::unhex(cell(4)?)?,
        integrity: crate::sealed::unhex(cell(5)?)?,
        bytes: cell(6)?.parse().ok()?,
        created_at: cell(7)?.parse().ok()?,
        // Absent only from a row read with an older column list; read as
        // the deployment's own store.
        store: cell(8).unwrap_or_default().to_string(),
    })
}

/// How many references a space still holds under an epoch.
///
/// The blob population of the re-seal question. `plan-production-ops.md`
/// K1 counts three populations before an epoch may be destroyed — entity
/// columns, `$sealedpayload` rows, command images — and blob envelopes are
/// a **fourth**: `mime`, `size` and `integrity` here, plus the object's
/// own ciphertext in the bucket. An epoch with live blob references inside
/// the undo horizon may not be destroyed, for the same reason and by the
/// same gate.
pub fn epoch_count_sql(space: &SpaceId, epoch: Epoch) -> String {
    format!(
        "SELECT count(*) FROM \"{BLOB_REF_TABLE}\" \
         WHERE \"{BLOB_SPACE_COLUMN}\" = {} AND \"epoch\" = {epoch}",
        lit(&space.0)
    )
}

/// Where [`list_sql`] puts the `"store"` column.
pub const LIST_STORE_COLUMN: usize = 9;

/// Every reference a space holds, ascending by handle — what an export
/// walks and what a space erasure deletes objects by. The last column is
/// the store each object was written to ([`LIST_STORE_COLUMN`]).
pub fn list_sql(space: &SpaceId) -> String {
    format!(
        "SELECT encode(\"ref\", 'hex'), \"epoch\", \"entity\", \"field\", \
                \"mime\", \"size\", \"integrity\", \"bytes\", \"created_at\", \"store\" \
         FROM \"{BLOB_REF_TABLE}\" \
         WHERE \"{BLOB_SPACE_COLUMN}\" = {} ORDER BY \"ref\"",
        lit(&space.0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every statement this module emits is confined to one space, and
    /// spells the tenancy column the way the write protocol's audit
    /// expects — [`crate::sealed`]'s test, applied to the second store.
    #[test]
    fn every_statement_names_its_space() {
        let space = SpaceId("tenant-a".into());
        let h = [0u8; 16];
        for sql in [
            insert_sql(&space, &h, 1, "docs", "file", b"a", b"b", b"c", 9, 1, ""),
            tomb_declare_sql(&space, &h, 1, ""),
            lookup_sql(&space, &h),
            epoch_count_sql(&space, 1),
            list_sql(&space),
        ] {
            assert!(sql.contains("\"$space\""), "unscoped: {sql}");
            assert!(sql.contains("'tenant-a'"), "not scoped to the space: {sql}");
        }
    }

    #[test]
    fn literals_escape() {
        let space = SpaceId("it's".into());
        assert!(lookup_sql(&space, &[0u8; 16]).contains("'it''s'"));
    }

    /// The DDL declares no uniqueness over any metadata column and no
    /// content column. Deduplication and content addressing are the two
    /// spellings of the same equality oracle the keyed-random handle
    /// exists to prevent, so their absence is asserted rather than
    /// reviewed — the shape of `sealed.rs`'s
    /// `the_store_has_no_way_to_deduplicate`, one table over.
    #[test]
    fn the_reference_store_has_no_way_to_deduplicate() {
        assert!(!BLOB_REF_DDL.contains("UNIQUE"));
        let lower = BLOB_REF_DDL.to_lowercase();
        assert!(
            !lower.contains("digest") && !lower.contains("sha") && !lower.contains("content_hash"),
            "a content-derived column is the withdrawn ruling, re-broken"
        );
        for col in ["(\"integrity\")", "(\"mime\")", "(\"size\")"] {
            assert!(
                !BLOB_REF_DDL.contains(col),
                "an index on {col} is an equality index over sealed metadata"
            );
        }
        assert!(BLOB_REF_DDL.contains("PRIMARY KEY (\"$space\", \"ref\")"));
    }

    /// The tombstone table records *when* a reference stopped being
    /// referenced, because phase 2 compares that instant against the
    /// prune floor. A tombstone with no timestamp would make the sweep's
    /// floor unenforceable.
    #[test]
    fn the_tombstone_carries_the_instant_the_floor_is_compared_against() {
        assert!(BLOB_TOMB_DDL.contains("\"unreferenced_since\" bigint NOT NULL"));
        assert!(BLOB_TOMB_DDL.contains("\"deleted_at\" bigint"));
        assert!(BLOB_TOMB_DDL.contains("PRIMARY KEY (\"$space\", \"ref\")"));
    }

    /// **Writing a reference row cancels the upload's intent in the same
    /// statement.** Two statements would leave a window in which a
    /// tombstone stamped at the start of the upload sits beside a live
    /// reference, and that tombstone's `unreferenced_since` is the instant
    /// the sweep's floor is compared against. A crash inside the window
    /// would leave it there permanently, and the next delete of the row
    /// would be priced against a timestamp from the upload rather than
    /// from the delete.
    ///
    /// Asserted on the rendered SQL rather than on behaviour because the
    /// atomicity *is* the single statement: two `rows()` calls in one
    /// transaction would pass a behavioural test and still be the wrong
    /// shape for the write protocol, which executes each mutation on its
    /// own.
    #[test]
    fn a_reference_row_cancels_the_upload_intent_atomically() {
        let space = SpaceId("tenant-a".into());
        let sql = insert_sql(
            &space, &[7u8; 16], 1, "docs", "scan", b"a", b"b", b"c", 9, 1, "",
        );
        assert!(
            sql.starts_with("WITH "),
            "the cancel has to lead, or it is a second statement: {sql}"
        );
        assert!(sql.contains("DELETE FROM \"$blobtomb\""), "{sql}");
        assert!(
            sql.contains("INSERT INTO \"$blobref\""),
            "still the reference insert: {sql}"
        );
        assert!(
            !sql.contains(';'),
            "one statement, so nothing can commit between the two halves: {sql}"
        );
        assert!(
            sql.contains("\"deleted_at\" IS NULL"),
            "a closed tombstone is phase 2's crash-recovery record and is not a write \
             path's to erase: {sql}"
        );
    }

    /// **The intent names the handle and the instant, and nothing else.**
    /// It is an ordinary open tombstone, which is the whole point: the
    /// sweep needs no branch for it, because a `$blobtomb` row with no
    /// `$blobref` row beside it already means "an upload that never
    /// committed".
    #[test]
    fn a_declared_intent_is_an_ordinary_open_tombstone() {
        let sql = tomb_declare_sql(&SpaceId("tenant-a".into()), &[9u8; 16], 42, "");
        assert!(sql.contains("INSERT INTO \"$blobtomb\""), "{sql}");
        assert!(sql.contains("\"unreferenced_since\""), "{sql}");
        assert!(sql.contains("42"), "{sql}");
        assert!(
            !sql.contains("\"deleted_at\""),
            "an intent is open, or the sweep would skip it: {sql}"
        );
        assert!(
            !sql.to_uppercase().contains("ON CONFLICT"),
            "a handle that collides with a tombstone already in the table is a minting \
             failure, and refusing the upload is the answer that does not silently write \
             bytes under another object's name: {sql}"
        );
    }

    /// **Each row names the store its object was written to** (ruled
    /// 2026-10-05, `detach-upload-race` ask 2): the reference row and the
    /// upload's intent both carry it, quoted, and the lookup reads it back.
    #[test]
    fn the_reference_and_the_intent_both_record_the_store_the_bytes_went_to() {
        let space = SpaceId("tenant-a".into());
        let name = "{\"bucket\":\"it's\"}";
        let refs = insert_sql(
            &space, &[1u8; 16], 1, "docs", "scan", b"a", b"b", b"c", 9, 1, name,
        );
        assert!(refs.contains("\"store\")"), "{refs}");
        assert!(refs.contains("'{\"bucket\":\"it''s\"}'"), "{refs}");
        let intent = tomb_declare_sql(&space, &[1u8; 16], 1, name);
        assert!(intent.contains("\"store\")"), "{intent}");
        assert!(intent.contains("'{\"bucket\":\"it''s\"}'"), "{intent}");
        assert!(lookup_sql(&space, &[1u8; 16]).contains("\"store\""));
        assert!(list_sql(&space).contains("\"store\""));
        assert!(BLOB_REF_DDL.contains("\"store\" text NOT NULL DEFAULT ''"));
        assert!(BLOB_TOMB_DDL.contains("\"store\" text NOT NULL DEFAULT ''"));
    }

    /// A round trip through the row decoder, including the two envelope
    /// columns, which are bytea and therefore arrive hex-prefixed.
    #[test]
    fn a_reference_row_decodes() {
        let row: Vec<Option<String>> = vec![
            Some("3".into()),
            Some("docs".into()),
            Some("file".into()),
            Some("\\x0102".into()),
            Some("\\x03".into()),
            Some("\\x".into()),
            Some("4096".into()),
            Some("1700000000".into()),
            Some("{\"bucket\":\"old\"}".into()),
        ];
        let r = blob_ref_of(&row).expect("decodes");
        assert_eq!(r.store, "{\"bucket\":\"old\"}");
        assert_eq!(r.epoch, 3);
        assert_eq!(r.mime, vec![1, 2]);
        assert_eq!(r.size, vec![3]);
        assert_eq!(r.integrity, Vec::<u8>::new());
        assert_eq!(r.bytes, 4096);
    }
}
