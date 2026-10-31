//! The sealed payload store: what a `SealedRef` resolves *through*.
//!
//! A command image never carries a sealed value. It carries a 16-byte
//! opaque handle ([`custody::KeyProvider::mint_sealed_ref`]), and the
//! bytes that handle names live here — one row per sealed image write, in
//! the space that wrote it, **never deduplicated**.
//!
//! # Why the shape lives in this crate
//!
//! Two crates create this table and neither can see the other. The
//! conformance harness builds its database through `psql` and has no
//! driver; `ordo-exec` owns the pool and creates the platform tables at
//! boot, and it *depends on* `ordo-conformance` rather than the reverse.
//! A table whose shape is spelled twice is a table that will be spelled
//! two different ways, and the difference would show up as a payload that
//! resolves on one path and not the other. `ordo-crypto` is the lowest
//! crate both can see and it already owns the handle, so it owns the
//! handle's storage shape too.
//!
//! It stays driver-free: everything here is a `String`. No `tokio`, no
//! `tokio-postgres`, no async — the same discipline that keeps
//! `ordo-commandlog` a pure crate.
//!
//! # Never deduplicate
//!
//! There is no unique index on `payload`, no content hash column, and no
//! upsert anywhere in this module. That is the point, not an omission.
//! `security-logging-design.md` §7 rejects a content-addressed ref because
//! it would make the command log "an unsalted, permanent, unbounded
//! equality index over every sealed field" — and a **deduplicating store**
//! reintroduces exactly that oracle through the back door: if two equal
//! plaintexts collapsed onto one row, then two images naming the same row
//! would prove the two writes were equal, whatever the handles looked
//! like. Every seal draws a fresh nonce (`ordo_crypto::seal_with`), so
//! equal plaintexts do not even produce equal bytes to deduplicate on;
//! this module additionally never asks.
//!
//! The cost is stated rather than hidden: a converge over a sealed field
//! stores the payload **twice** — once for the before-image, once for the
//! after-image — and a bulk write over N rows stores 2N. Sealed values are
//! small (they are fields, not blobs — a blob's bytes go to the object
//! store under [`crate::stream`], and only a `blob inline` payload lands
//! here, bounded by `BlobPolicy::inline_cap`), and the alternative is an
//! equality oracle nobody opted into.
//!
//! # Garbage
//!
//! A payload is reachable only through the image that names it, so it is
//! garbage exactly when that image is. Two ways that happens:
//!
//! - **The command group was pruned.** Pruning removes chain entries below
//!   a verified checkpoint; the handles in those images become unnameable
//!   at the same instant. That is why every row records the
//!   `correlation` of the group whose image minted it: collection is the
//!   anti-join "payload rows in this space whose correlation is no longer
//!   in this space's `$commandlog`", one index scan against
//!   `$sealedpayload_group`. Collection **is** scheduled now: pruning runs
//!   the anti-join in the same transaction as the entry delete
//!   (`ordo_exec::prune`), which is the ordering the alternative — running
//!   it behind the delete — cannot be trusted with, since a crash between
//!   the two leaves payloads this query can never find again.
//! - **The epoch key was destroyed.** The row stays. Destroying an epoch
//!   is the narrow crypto-erasure primitive: it makes the payload
//!   permanently unopenable, which is the whole intent, and deleting the
//!   row as well would only replace one unresolvable answer ("the key is
//!   gone") with a less honest one ("there was never a payload"). The
//!   `epoch` column is stored so the operator gating
//!   `KeyProvider::destroy_epoch` on the undo horizon can count what an
//!   epoch still holds before destroying it.
//!
//! Neither case is silent: an image whose payload cannot be produced
//! refuses the revert with a typed rejection, per §7.

use crate::custody::{Epoch, SealedHandle, SpaceId};

/// The table's name. `$`-prefixed like every platform table, so it cannot
/// collide with an entity (no schema can declare one).
pub const SEALED_PAYLOAD_TABLE: &str = "$sealedpayload";

/// The per-space tenancy column, spelled the way entity tables spell it.
///
/// Deliberately `"$space"` and not the `"space"` that `$commandlog` uses:
/// this table is written by the **write protocol**, whose every statement
/// passes an audit that fails any statement naming a table without
/// constraining `ordo_schema::SPACE_COLUMN`. Matching the entity tables'
/// spelling means a payload insert is confined by the same mechanism that
/// confines the row insert beside it, rather than by a reviewer noticing.
pub const SEALED_SPACE_COLUMN: &str = "$space";

/// The store, per space, never deduplicating.
///
/// `PRIMARY KEY ("$space", "ref")` is the per-space composite shape the
/// schema uses everywhere: a handle is unique *within a space*, and two
/// spaces cannot address each other's payloads even if a handle somehow
/// repeated. There is **no** index on `"payload"` — see the module doc.
pub const SEALED_PAYLOAD_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$sealedpayload\" ( \
  \"$space\" text NOT NULL, \
  \"ref\" bytea NOT NULL, \
  \"epoch\" smallint NOT NULL, \
  \"entity\" text NOT NULL, \
  \"field\" text NOT NULL, \
  \"anchor\" text NOT NULL, \
  \"correlation\" text NOT NULL, \
  \"payload\" bytea NOT NULL, \
  PRIMARY KEY (\"$space\", \"ref\") \
); \
ALTER TABLE \"$sealedpayload\" ADD COLUMN IF NOT EXISTS \"anchor\" text NOT NULL DEFAULT ''; \
CREATE INDEX IF NOT EXISTS \"$sealedpayload_group\" \
  ON \"$sealedpayload\" (\"$space\", \"correlation\"); \
CREATE INDEX IF NOT EXISTS \"$sealedpayload_epoch\" \
  ON \"$sealedpayload\" (\"$space\", \"epoch\")";

/// One stored payload, as the store hands it back.
///
/// `entity`, `field` and `anchor` are not decoration: the envelope is
/// AAD-bound to them, so opening it requires naming the column it was
/// sealed for and the row (or object) it was sealed at, and a payload that
/// has been moved to another column or another row refuses to open.
/// Keeping them beside the bytes means a resolver does not have to be
/// told. `anchor` is an [`crate::Anchor::label`]: the payload is a copy of
/// the envelope on the row it images, so it keeps that row's label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPayload {
    pub epoch: Epoch,
    pub entity: String,
    pub field: String,
    /// The label of the anchor the envelope was sealed at.
    pub anchor: String,
    /// The **envelope**, exactly as it was written to the row: the same
    /// `[epoch][nonce][ct||tag]` bytes, not a re-seal of them.
    pub bytes: Vec<u8>,
}

/// A single-quoted SQL string literal.
fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A `bytea` literal in the hex form Postgres emits and accepts.
pub fn bytea_lit(bytes: &[u8]) -> String {
    format!("'\\x{}'::bytea", crate::hex(bytes))
}

/// Decode Postgres's text rendering of a `bytea` (`\x0a1b…`).
///
/// Both venues render it the same way: `psql` and `tokio-postgres`'s
/// simple-query protocol both honour `bytea_output = hex`, which is the
/// default and which neither path overrides.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    let body = text.strip_prefix("\\x")?;
    if body.len() % 2 != 0 {
        return None;
    }
    (0..body.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(body.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Store one payload under the handle a command image will carry.
///
/// An `INSERT`, never an upsert: a handle that collided would be a
/// primary-key violation the caller sees, and that is the honest answer.
/// The handle is 128 random bits drawn per write, so a collision inside
/// one space is not a case anybody will meet; if it ever happened,
/// silently overwriting the other write's payload would be far worse than
/// a refused transaction.
#[allow(clippy::too_many_arguments)]
pub fn insert_sql(
    space: &SpaceId,
    handle: &SealedHandle,
    epoch: Epoch,
    entity: &str,
    field: &str,
    anchor: &str,
    correlation: &str,
    payload: &[u8],
) -> String {
    format!(
        "INSERT INTO \"{SEALED_PAYLOAD_TABLE}\" \
         (\"{SEALED_SPACE_COLUMN}\", \"ref\", \"epoch\", \"entity\", \"field\", \
          \"anchor\", \"correlation\", \"payload\") \
         VALUES ({}, {}, {epoch}, {}, {}, {}, {}, {})",
        lit(&space.0),
        bytea_lit(handle),
        lit(entity),
        lit(field),
        lit(anchor),
        lit(correlation),
        bytea_lit(payload),
    )
}

/// Look one handle up, confined to its space.
///
/// The space predicate is what makes a handle un-transferable between
/// tenants: even a caller who somehow learned another space's handle reads
/// nothing here, because the lookup is keyed on the pair.
pub fn lookup_sql(space: &SpaceId, handle: &SealedHandle) -> String {
    format!(
        "SELECT \"epoch\", \"entity\", \"field\", \"anchor\", \"payload\" \
         FROM \"{SEALED_PAYLOAD_TABLE}\" \
         WHERE \"{SEALED_SPACE_COLUMN}\" = {} AND \"ref\" = {}",
        lit(&space.0),
        bytea_lit(handle),
    )
}

/// Decode a [`lookup_sql`] row.
pub fn payload_of(row: &[Option<String>]) -> Option<SealedPayload> {
    let cell = |i: usize| row.get(i)?.as_deref();
    Some(SealedPayload {
        epoch: cell(0)?.parse().ok()?,
        entity: cell(1)?.to_string(),
        field: cell(2)?.to_string(),
        anchor: cell(3)?.to_string(),
        bytes: unhex(cell(4)?)?,
    })
}

/// How many payloads a space still holds under an epoch.
///
/// The question `destroy_epoch`'s caller owes an answer to before it
/// destroys anything: an epoch with live payloads inside the undo horizon
/// may not be destroyed (`security-logging-design.md` §7).
pub fn epoch_count_sql(space: &SpaceId, epoch: Epoch) -> String {
    format!(
        "SELECT count(*) FROM \"{SEALED_PAYLOAD_TABLE}\" \
         WHERE \"{SEALED_SPACE_COLUMN}\" = {} AND \"epoch\" = {epoch}",
        lit(&space.0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_through_postgres_text() {
        let bytes = vec![0u8, 1, 0x7f, 0x80, 0xff];
        let rendered = bytea_lit(&bytes);
        assert_eq!(rendered, "'\\x00017f80ff'::bytea");
        assert_eq!(unhex("\\x00017f80ff").unwrap(), bytes);
        assert_eq!(unhex("\\x").unwrap(), Vec::<u8>::new());
        assert!(unhex("00017f").is_none(), "a bare hex string is not bytea");
        assert!(unhex("\\x0").is_none(), "an odd nibble count is not bytea");
    }

    /// Every statement this module emits is confined to one space, and
    /// spells the tenancy column the way the write protocol's audit
    /// expects. The audit counts table mentions against `$space`
    /// mentions, so a statement that named the table without the column
    /// would be refused before it reached the database — this asserts it
    /// never has to be.
    #[test]
    fn every_statement_names_its_space() {
        let space = SpaceId("tenant-a".into());
        let h = [0u8; 16];
        for sql in [
            insert_sql(&space, &h, 1, "orders", "memo", "row:ord_1", "cg_1", b"x"),
            lookup_sql(&space, &h),
            epoch_count_sql(&space, 1),
        ] {
            assert!(sql.contains("\"$space\""), "unscoped: {sql}");
            assert!(sql.contains("'tenant-a'"), "not scoped to the space: {sql}");
        }
    }

    /// A quoted literal cannot end the string early.
    #[test]
    fn literals_escape() {
        let space = SpaceId("it's".into());
        assert!(lookup_sql(&space, &[0u8; 16]).contains("'it''s'"));
    }

    /// The DDL declares no uniqueness over the payload and no content
    /// column. Deduplication is the equality oracle the random handle
    /// exists to prevent, so its absence is asserted rather than reviewed.
    #[test]
    fn the_store_has_no_way_to_deduplicate() {
        assert!(!SEALED_PAYLOAD_DDL.contains("UNIQUE"));
        assert!(!SEALED_PAYLOAD_DDL.to_lowercase().contains("digest"));
        assert!(
            !SEALED_PAYLOAD_DDL.contains("(\"payload\")"),
            "an index on the payload is an equality index over sealed values"
        );
        assert!(SEALED_PAYLOAD_DDL.contains("PRIMARY KEY (\"$space\", \"ref\")"));
    }
}
