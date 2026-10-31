//! Bulk import — an export bundle's rows back into a space, through the
//! write protocol.
//!
//! The inverse of [`crate::export`], and the second leg of the trust
//! argument `licensing.md` makes: a customer who can get their data out and
//! cannot get it in has half a promise. `docs/plan-import.md` is the design
//! and every decision below is argued there; this module doc states only
//! what a reader of the code needs in front of them.
//!
//! # A write is not an `INSERT`, so this does not insert
//!
//! Every row enters through `import.<entity>.create` ([`import_ops`]) — the
//! same shape `ordo_schema::std_ops` derives as `std.<entity>.create`, and
//! for a keyless entity now identical to it (see `import_ops`'s doc for why
//! it is still its own derivation rather than a call to `std`). Either way
//! it is what seals the sealed fields under the **destination** space's epoch
//! key, derives each blind-index digest under the destination's field key,
//! mints the id and `rev`, proves the guard against the would-be row, and
//! produces the command this space's chain records. A loader that wrote the
//! columns directly would produce rows the platform cannot read (no
//! digest), cannot open (no envelope), cannot verify (no chain entry) and
//! cannot revert (no images).
//!
//! Nothing here knows an entity name, a field name or a type. The calls are
//! built from `Schema` and the bundle's own manifest, by exhaustive match,
//! for the reason [`crate::export`] gives at length: a hand-maintained
//! column list is silently wrong one deploy after somebody adds a field.
//!
//! # The batch is the unit of everything
//!
//! One batch is one transaction, one idempotency lease, one command group
//! and one chain append. `ordo_conformance::WriteLog::open_batch` is what
//! makes the group one rather than N: while a batch is open every call
//! extends the pending group instead of replacing it.
//!
//! A batch that refuses anywhere rolls back whole. There is no partial
//! group, no orphan mapping row, and no completion record for work that did
//! not happen.
//!
//! # A file is not a column, so a blob does not ride the create
//!
//! `Sema::check_content_params` refuses a blob-typed op parameter, because
//! the only producer of a blob's value is an upload that happens after the
//! row exists. So `import.<entity>.create` cannot carry a file and this
//! module does not pretend it can: the bundle's blob column is the
//! **source** space's handle, naming an object in the source's bucket under
//! the source's key, and it is dropped.
//!
//! The bytes arrive the other way. Each object rides the bundle as its own
//! member at `blobs/<entity>/<field>/<id>`, and once every row exists, each
//! one is bound to the row this import minted for it **through the
//! deployment's own blob door** ([`BlobDoor`]) — the same upload path a
//! client's own `PUT` takes. That is where the bytes are sealed under this
//! space's key, where the `$blobref` row is written, where the column is
//! set, and where the digest joins the image of the group that records it.
//! An import that wrote the column itself would be a second upload path,
//! and `docs/plan-blob-chain.md` C1–C8 ruled there is one.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use ordo_conformance::write::WriteDb;
use ordo_conformance::{Fixture, WriteLog};
use ordo_crypto::SpaceId;
use ordo_schema::{Field, FieldTy, Schema};
use ordo_syntax::decl::ScalarTy;
use serde_json::Value;

use crate::registry::Artifact;
use crate::{ExecFail, Executor, Pool, PoolError};

/// The only bundle format this reads. Matched against `manifest.format`
/// exactly: a bundle from a format nobody has written a reader for is a
/// refusal, never a best effort.
pub const FORMAT: &str = crate::export::FORMAT;

/// Rows per batch when the caller does not choose. Thousands, per
/// `migration.md` §3's *"size policy-tunable, default in the thousands"*;
/// one thousand keeps a single group's memory bounded while cutting the
/// chain entries for a million rows to a thousand.
pub const BATCH: usize = 1000;

/// The old-id to new-id mapping `migration.md` §3 requires.
///
/// *"A persisted old-id → new-id mapping table is written during import:
/// queryable during the transition (redirects, foreign systems still
/// holding old keys, the dual-write bridge), deletable after."*
///
/// It is a platform table with a `$space` column, so `erase_space`'s sweep
/// finds it from `information_schema` without being told, and the drift
/// gate ignores it because an extra live table is not drift
/// (`crate::shape`).
///
/// The second unique index is not decoration. A minted row belongs to
/// exactly one source row, and a mapping that let two source ids claim one
/// minted id would make the ref rewrite ambiguous in the direction nobody
/// checks: every reference to either source id would resolve, to the same
/// row, silently.
pub const IMPORTMAP_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$importmap\" ( \
  \"$space\"    text NOT NULL, \
  \"import\"    text NOT NULL, \
  \"entity\"    text NOT NULL, \
  \"source_id\" text NOT NULL, \
  \"row_id\"    text NOT NULL, \
  PRIMARY KEY (\"$space\", \"import\", \"entity\", \"source_id\") \
); \
CREATE UNIQUE INDEX IF NOT EXISTS \"$importmap_minted\" \
  ON \"$importmap\" (\"$space\", \"import\", \"entity\", \"row_id\"); ";

// ── the source ───────────────────────────────────────────────────────────

/// Where a bundle's members are read from: the read-side mirror of
/// [`crate::export::BundleSink`].
///
/// A trait for the sink's reason in reverse. A bundle is potentially larger
/// than memory, and the day a streaming reader is needed it should be a new
/// implementation rather than a new entry point.
/// **`Send + Sync`, and the bound is load-bearing.** `import_space` takes
/// `&dyn BundleSource`, and a platform op's handler returns a boxed future
/// the door requires to be `Send`. Without this, `std.space.import` could
/// not exist: the engine would be callable in a test and not through the
/// door, which is the state 17y finding 1 recorded. Both implementations
/// here satisfied it already.
pub trait BundleSource: Send + Sync {
    /// One member's exact bytes. `None` when the bundle does not have it.
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, ImportError>;
    /// Every member's name. Used to refuse a bundle carrying members this
    /// version does not understand rather than ignoring them.
    fn names(&self) -> Result<Vec<String>, ImportError>;
}

/// A bundle in memory, for tests and small spaces.
#[derive(Debug, Default)]
pub struct MemorySource {
    pub members: BTreeMap<String, Vec<u8>>,
}

impl MemorySource {
    pub fn new() -> MemorySource {
        MemorySource::default()
    }
    pub fn put(&mut self, name: &str, body: impl Into<Vec<u8>>) {
        self.members.insert(name.to_string(), body.into());
    }
}

impl From<&crate::export::MemoryBundle> for MemorySource {
    fn from(b: &crate::export::MemoryBundle) -> MemorySource {
        MemorySource {
            members: b.members.clone(),
        }
    }
}

impl BundleSource for MemorySource {
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, ImportError> {
        Ok(self.members.get(name).cloned())
    }
    fn names(&self) -> Result<Vec<String>, ImportError> {
        Ok(self.members.keys().cloned().collect())
    }
}

/// A bundle as files under a directory, as `DirectoryBundle` wrote it.
pub struct DirectorySource {
    root: std::path::PathBuf,
}

impl DirectorySource {
    pub fn new(root: impl Into<std::path::PathBuf>) -> DirectorySource {
        DirectorySource { root: root.into() }
    }
}

impl BundleSource for DirectorySource {
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, ImportError> {
        // A member name is a bundle-relative path the export chose; a
        // caller-supplied `..` in one would read outside the bundle. The
        // export never writes such a name, so refusing costs nothing and
        // closes the seam for a bundle that arrived from elsewhere.
        if name.split('/').any(|seg| seg == ".." || seg.is_empty()) {
            return Err(ImportError::Bundle(format!(
                "`{name}` is not a bundle member name"
            )));
        }
        match std::fs::read(self.root.join(name)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ImportError::Bundle(format!("reading `{name}`: {e}"))),
        }
    }
    fn names(&self) -> Result<Vec<String>, ImportError> {
        fn walk(
            root: &std::path::Path,
            dir: &std::path::Path,
            out: &mut Vec<String>,
        ) -> Result<(), ImportError> {
            let entries = std::fs::read_dir(dir)
                .map_err(|e| ImportError::Bundle(format!("listing `{}`: {e}", dir.display())))?;
            for entry in entries {
                let entry =
                    entry.map_err(|e| ImportError::Bundle(format!("listing a member: {e}")))?;
                let path = entry.path();
                if path.is_dir() {
                    walk(root, &path, out)?;
                } else if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        walk(&self.root, &self.root, &mut out)?;
        out.sort();
        Ok(out)
    }
}

// ── the blob door, as import needs it ────────────────────────────────────

/// One object, ready to be bound to a row that already exists.
pub struct ImportObject<'a> {
    pub space: &'a str,
    pub entity: &'a str,
    pub field: &'a str,
    /// The **minted** row id in the destination space, resolved through
    /// `$importmap`. Never the id the bundle carried.
    pub row_id: &'a str,
    /// The mime the source space held, from `blobs.files`. Not guessed:
    /// `plan_objects` refuses an object the manifest does not name one for.
    pub mime: &'a str,
    /// The member's exact bytes, already checked against the manifest's
    /// `sha256`.
    pub bytes: Vec<u8>,
    /// The principal the upload is attributed to, and whose write guard the
    /// protocol evaluates against the row.
    pub principal: &'a str,
    /// What the key release is attributed under. Deterministic in the
    /// import's name and the member, so a resumed import's custody audit
    /// reads the same as an uninterrupted one's.
    pub correlation: &'a str,
}

/// **The deployment's blob door, and the only way an import writes a blob
/// column.**
///
/// A trait because of where the door is: `ordo_host::blob::PlatformBlobs`
/// runs the sealer, mints the upload token and hands it to the write
/// protocol, and `ordo-host` is above this crate. Inverting that to let
/// import reach down would need a second mint site for the token and a
/// second caller of the platform converge, which is exactly what
/// `docs/plan-blob-chain.md` C1–C8 ruled against and what
/// `crates/ordo-host/tests/upload_seam.rs` asserts against the workspace's
/// own source. So the seam points the other way: import decides *which
/// bytes belong on which row*, and the door decides *what an upload is*.
///
/// **One upload path**, therefore, and every property it carries is
/// inherited rather than restated here: the tier comes from the schema and
/// not from the caller, the intent is declared before the bytes so an
/// interrupted upload is collectable, the bytes are sealed under the
/// **destination** space's key, the row is locked and its write guard
/// evaluated before and after, the digest rides the image, and the whole of
/// it is one command group in this space's chain. An import that wrote the
/// column itself would have none of that, and nothing about the resulting
/// row would look wrong.
#[async_trait::async_trait]
pub trait BlobDoor: Send + Sync {
    /// Store the bytes and bind them to the row, or refuse with the door's
    /// own error class.
    ///
    /// The object is taken **by value** because its bytes are moved into the
    /// upload's body stream, and because one object is one upload: an
    /// implementation that could be handed the same object twice would be
    /// one that could store the same file twice under two handles.
    async fn attach(&self, object: ImportObject<'_>) -> Result<(), String>;
}

// ── refusals ─────────────────────────────────────────────────────────────

/// Why an import did not happen, or did not finish.
///
/// Every variant names what the operator does next, and every one of them
/// is a case the loader could have guessed at instead. An import that
/// guesses produces a space that looks migrated and is not, and that is
/// discovered at cutover, which is the most expensive possible moment.
#[derive(Debug)]
#[non_exhaustive]
pub enum ImportError {
    /// `manifest.format` is not [`FORMAT`].
    FormatUnknown {
        found: String,
    },
    /// A member the manifest names is absent, or its bytes do not hash to
    /// the digest the manifest recorded.
    BundleIncomplete {
        member: String,
        detail: String,
    },
    /// The destination's deployed artifact is not the bundle's. Rows shaped
    /// by one schema loaded against another is the silent wrongness
    /// `plan-export.md` §1 is written against.
    ArtifactMismatch {
        bundle: String,
        deployed: String,
        detail: String,
    },
    /// The bundle carries a column this schema does not declare, or the
    /// schema declares a field the bundle does not carry.
    FieldMismatch {
        entity: String,
        field: String,
        detail: String,
    },
    /// The bundle's sealed values are envelopes under the **source**
    /// space's keys, and the write protocol seals under the destination's.
    SealedCiphertext,
    /// The destination already holds rows for an entity being imported.
    SpaceNotEmpty {
        entity: String,
        rows: i64,
    },
    /// `manifest.load.cyclic` is non-empty: entities that reference each
    /// other have no sequential load order.
    CyclicEntities {
        entities: Vec<String>,
    },
    /// A tree entity's parent links do not form a forest.
    TreeCycle {
        entity: String,
        rows: Vec<String>,
    },
    /// The people map names a destination member that is not a member of
    /// the destination space.
    ///
    /// Refused rather than created, which is the ruling: a map that could
    /// admit a principal would be the option where a file grants access.
    PeopleUnknown {
        source: String,
        destination: String,
    },
    /// The people map names one source person twice, or points two source
    /// people at one destination member.
    ///
    /// The first is a contradiction. The second is a **merge**, which is
    /// refused rather than performed: `$importmap`'s minted-id index exists
    /// because one minted row answers to exactly one source row, and two
    /// source ids claiming one row make every reference to either resolve to
    /// the same row silently. An operator who means to merge two source
    /// identities can say so in the source data, where it is visible, rather
    /// than in a map where it is not.
    PeopleAmbiguous {
        detail: String,
    },
    /// This import ran before under this name with a **different** people
    /// map.
    ///
    /// The stored mapping is what the rows already in the destination were
    /// written against, so a changed map would attribute the rest of the
    /// import to different people than the part that already landed. Refused
    /// for the same reason a changed bundle under a used name is.
    PeopleChanged {
        source: String,
        stored: String,
        supplied: String,
    },
    /// A ref names a source row that is not in `$importmap` for this
    /// import.
    UnresolvedRef {
        entity: String,
        field: String,
        source_id: String,
        target: String,
        target_id: String,
    },
    /// The write protocol refused the row. The detail carries its own error
    /// class, and the id is the **source** id because the minted one does
    /// not exist.
    RowRefused {
        entity: String,
        source_id: String,
        detail: String,
    },
    /// A value in the bundle has no literal spelling in the language.
    Unencodable {
        entity: String,
        field: String,
        source_id: String,
        detail: String,
    },
    /// The bundle carries objects and the caller passed no blob door.
    ///
    /// The mirror of `ExportRequest::blobs: None`, which means *this
    /// deployment has no blob tier wired* rather than *skip the blobs*. A
    /// loader that quietly loaded the rows and left the files behind would
    /// produce a space whose every attachment resolves to nothing, and the
    /// operator would find out when somebody opened a document.
    BlobDoorMissing {
        objects: u64,
    },
    /// An object the bundle carries has no row in this space to hang on.
    BlobUnplaced {
        member: String,
        detail: String,
    },
    /// The field's custody tier is declarable and not importable. The three
    /// tiers are the three the blob door itself refuses, refused here by
    /// name **before** a byte is sealed rather than discovered one upload
    /// in.
    BlobTierUnsupported {
        entity: String,
        field: String,
        tier: &'static str,
        detail: String,
    },
    /// The blob door refused the object. The detail carries the door's own
    /// error class, as [`ImportError::RowRefused`] carries the write
    /// protocol's.
    BlobRefused {
        member: String,
        detail: String,
    },
    /// The bundle's access profile cannot be restored faithfully. `class`
    /// names why (`profile_op_unknown`, `profile_member_unmapped`,
    /// `profile_invalid`, `profile_destination_not_empty`).
    ///
    /// Refused rather than dropped: a profile line that falls away on
    /// import leaves its op at the app developer's default, which can be
    /// wider than what the tenant set.
    ProfileRefused {
        class: &'static str,
        detail: String,
    },
    /// The bundle's JSON did not parse, or did not have the shape the
    /// format promises.
    Bundle(String),
    Store(PoolError),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::FormatUnknown { found } => write!(
                f,
                "format_unknown: this reads `{FORMAT}` bundles and the manifest says `{found}`"
            ),
            ImportError::BundleIncomplete { member, detail } => {
                write!(f, "bundle_incomplete: `{member}` — {detail}")
            }
            ImportError::ArtifactMismatch {
                bundle,
                deployed,
                detail,
            } => write!(
                f,
                "artifact_mismatch: the bundle was exported from `{bundle}` and this space \
                 runs `{deployed}` ({detail}). Deploy the bundle's artifact first: rows \
                 shaped by one schema loaded against another is the silent wrongness the \
                 export module is written against"
            ),
            ImportError::FieldMismatch {
                entity,
                field,
                detail,
            } => write!(f, "field_mismatch: `{entity}`.`{field}` — {detail}"),
            ImportError::SealedCiphertext => write!(
                f,
                "sealed_ciphertext_unsupported: this bundle's sealed values are envelopes \
                 under the SOURCE space's epoch key, and the write protocol seals under this \
                 space's. Re-keying one to the other is a custody act nobody has ruled, so \
                 it is refused rather than guessed. Re-export in `opened` mode"
            ),
            ImportError::SpaceNotEmpty { entity, rows } => write!(
                f,
                "space_not_empty: `{entity}` holds {rows} row(s) in this space that this \
                 import did not write. A run of this same import may resume over its own \
                 rows; merging into somebody else's needs a conflict policy that is not \
                 ruled. Import into an empty space, re-run under the name that wrote \
                 them, or erase this one first"
            ),
            ImportError::CyclicEntities { entities } => write!(
                f,
                "cyclic_entities: {} reference each other, so no sequential load order \
                 exists and the ref rewrite has nothing to go on. Deferred work, named in \
                 roadmap 11-10a",
                entities.join(", ")
            ),
            ImportError::TreeCycle { entity, rows } => write!(
                f,
                "tree_cycle: `{entity}`'s parent links do not form a forest — {} row(s) \
                 including `{}` have no root",
                rows.len(),
                rows.first().map(String::as_str).unwrap_or("?")
            ),
            ImportError::UnresolvedRef {
                entity,
                field,
                source_id,
                target,
                target_id,
            } => write!(
                f,
                "unresolved_ref: `{entity}`.`{field}` of source row `{source_id}` names \
                 `{target}` row `{target_id}`, which this import has not loaded. Every ref \
                 is a foreign key on ($space, id), so the manifest's load order is what has \
                 to be followed and a missing target is a refusal rather than a null"
            ),
            ImportError::RowRefused {
                entity,
                source_id,
                detail,
            } => write!(
                f,
                "row_refused: the write protocol refused `{entity}` source row \
                 `{source_id}`: {detail}. The batch rolled back whole — no row, no mapping, \
                 no chain entry"
            ),
            ImportError::Unencodable {
                entity,
                field,
                source_id,
                detail,
            } => write!(
                f,
                "unencodable_value: `{entity}`.`{field}` of source row `{source_id}` — \
                 {detail}"
            ),
            ImportError::BlobDoorMissing { objects } => write!(
                f,
                "blob_door_missing: the bundle carries {objects} object(s) and this import was \
                 given no blob door to put them through. An object is bound to a row by the \
                 same upload path a client's own upload takes, sealed under THIS space's key; \
                 there is no second door and no way to write a blob column without one. Pass \
                 the deployment's blob port, or import a bundle with no objects"
            ),
            ImportError::BlobUnplaced { member, detail } => write!(
                f,
                "blob_unplaced: `{member}` — {detail}. An object is attached to a row that \
                 already exists, so the row has to be one this import loaded and mapped"
            ),
            ImportError::BlobTierUnsupported {
                entity,
                field,
                tier,
                detail,
            } => write!(
                f,
                "blob_tier_unsupported: `{entity}`.`{field}` is `blob {tier}` — {detail}. The \
                 blob door refuses this tier on the way in for a client's upload too, so an \
                 import is refused here rather than one object later"
            ),
            ImportError::PeopleUnknown {
                source,
                destination,
            } => write!(
                f,
                "people_unknown: the map sends source person `{source}` to `{destination}`, \
                 which is not a member of this space. Add them to the destination first: an \
                 import may not admit anybody, because a bundle that could would be a file \
                 that grants access"
            ),
            ImportError::PeopleAmbiguous { detail } => write!(
                f,
                "people_ambiguous: {detail}. A merge is refused rather than performed, \
                 because two source ids resolving to one row make every reference to either \
                 resolve to the same row with nothing to show it"
            ),
            ImportError::PeopleChanged {
                source,
                stored,
                supplied,
            } => write!(
                f,
                "people_changed: this import already mapped source person `{source}` to \
                 `{stored}` and the map now says `{supplied}`. The rows already loaded were \
                 attributed under the stored mapping, so changing it would split this \
                 import's authorship between two answers. Use a new import name"
            ),
            ImportError::BlobRefused { member, detail } => write!(
                f,
                "blob_refused: the blob door refused `{member}`: {detail}. The object's bytes \
                 may have reached the store and the row's column did not move, which is the \
                 door's own asymmetry: an unreferenced object is collected by the sweep, and \
                 a column naming nothing has no such mechanism"
            ),
            ImportError::ProfileRefused { class, detail } => write!(f, "{class}: {detail}"),
            ImportError::Bundle(m) => write!(f, "bundle: {m}"),
            ImportError::Store(e) => write!(f, "store: {e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<PoolError> for ImportError {
    fn from(e: PoolError) -> ImportError {
        ImportError::Store(e)
    }
}

// ── what the caller chooses ──────────────────────────────────────────────

/// Everything one import needs that is not the executor.
pub struct ImportRequest<'a> {
    /// The import's declared name. It scopes `$importmap` and every
    /// idempotency key, so **two runs of the same bundle under the same
    /// name resume each other, and two different names are two imports**.
    pub name: &'a str,
    /// The compiled program, bound to the destination space, with its key
    /// provider. The registry must hold the derived `std` ops, which
    /// `Fixture::from_source` puts there.
    pub fixture: Arc<Fixture>,
    /// The deployed artifact, whose identity the bundle's must match.
    pub artifact: &'a Artifact,
    /// The principal the import runs as. Guards are evaluated against them
    /// like any other caller, and the chain attributes the commands to
    /// them.
    ///
    /// `migration.md` §3 wants `source:import-<name>`, a write-only source
    /// principal. That needs the source principal itself, which is deferred
    /// (`plan-import.md` §7). Attributing to the principal who actually ran
    /// the import is true in the meantime, which is the property that
    /// matters.
    pub caller: &'a str,
    /// Rows per batch. [`BATCH`] when the caller has no opinion.
    pub batch: usize,
    /// **Who the people in the destination are**: source member id to
    /// destination member id, supplied by the operator.
    ///
    /// Empty is *no map*, which is the behaviour before this existed: the
    /// first row that names a person is refused with `unresolved_ref` and the
    /// import writes nothing.
    ///
    /// The owner ruled the shape on 2026-09-26 (`platform-ref-import`): a
    /// supplied map, and **a refusal for any name it does not cover**. Two
    /// alternatives were rejected and the reasons are the design here.
    /// Pointing every row at one member was refused because authorship is
    /// part of the data and an import that quietly reassigns it produces a
    /// space whose history is wrong in a way nobody can see afterwards.
    /// Letting the bundle create the members it names was refused outright,
    /// because it makes a file a grant of access to the space it is loaded
    /// into.
    ///
    /// **It is stored, not just applied.** Each pair becomes an
    /// `$importmap` row under this import's name, so the mapping an import
    /// used is queryable in the destination space forever, and a re-run whose
    /// map disagrees with the stored one is refused rather than resumed.
    pub people: &'a [(String, String)],
    /// The deployment's blob door, when it has one.
    ///
    /// `None` is the mirror of `ExportRequest::blobs: None` — *this
    /// deployment has no blob tier wired* — and a bundle carrying objects is
    /// then refused by name (`blob_door_missing`) rather than loaded without
    /// its files.
    pub blobs: Option<&'a dyn BlobDoor>,
}

/// What an import produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub space: String,
    pub name: String,
    /// Per entity, in load order: name and rows the bundle carried.
    pub entities: Vec<(String, u64)>,
    /// Batches this run committed.
    pub batches_written: u64,
    /// Batches a previous run had already committed, found by their lease
    /// and skipped. Non-zero means this was a resume, and it is reported
    /// rather than hidden: an operator re-running an import should be able
    /// to see that it did.
    pub batches_skipped: u64,
    /// Command groups appended, which is [`ImportReport::batches_written`]
    /// and is stated separately because the one-group-per-batch property is
    /// the whole design and a report that did not show it could not be
    /// checked.
    pub groups: u64,
    /// Objects this run bound to a row, one upload each.
    ///
    /// Not added to [`ImportReport::groups`]: an attach is its own command
    /// group, appended by the blob door, and folding the two counts would
    /// hide the one-group-per-batch property the field above exists to
    /// show.
    pub objects_attached: u64,
    /// People this run added to `$importmap`, which is the map's size on a
    /// first run and 0 on a resume that supplied the same map.
    ///
    /// Reported because the map decides authorship: an operator who imports
    /// with the wrong map produces a space whose history names the wrong
    /// people, and the count is the cheapest thing that would have told them.
    pub people_mapped: u64,
    /// Objects a previous run had already bound, found by the row's own
    /// column and skipped.
    ///
    /// Reported for [`ImportReport::batches_skipped`]'s reason and one of
    /// its own: an operator re-running an import whose **objects** changed
    /// under the same name sees them counted here, which is the direction
    /// the row batches' fingerprint does not cover (`plan-import.md` §7).
    pub objects_skipped: u64,
    /// Access-profile rows this run restored: op lines, overrides, groups
    /// and group links. 0 when the bundle carried none, or on a resume.
    pub profile_rows: u64,
}

impl ImportReport {
    pub fn rows(&self) -> u64 {
        self.entities.iter().map(|(_, n)| n).sum()
    }
}

// ── the manifest, as much of it as import reads ──────────────────────────

struct Manifest {
    artifact_version: String,
    domain: String,
    sealed: String,
    order: Vec<String>,
    cyclic: Vec<String>,
    entities: BTreeMap<String, EntityEntry>,
    digests: BTreeMap<String, String>,
    /// `profile.file`, when the exporter wrote one.
    profile_file: Option<String>,
    blobs_carried: bool,
    /// `blobs.objects`, the manifest's own count, kept beside the list
    /// below so the two can be checked against each other.
    blob_objects: u64,
    /// `blobs.files`, one entry per object the bundle carries.
    blob_files: Vec<ManifestObject>,
}

/// One entry of `blobs.files`.
///
/// The entity, the field and the id come from **here** rather than from
/// splitting the member name, which is the reason `plan-export.md` gives for
/// writing them: recovering them from the path would be parsing a string the
/// exporter formatted, and the first id holding a `/` would place an object
/// on the wrong row rather than refuse.
struct ManifestObject {
    member: String,
    entity: String,
    field: String,
    /// The **source** row id. The destination's is whatever this import
    /// minted for it, which `$importmap` knows and the bundle cannot.
    source_id: String,
    /// Absent when the member does not hold plaintext. An import needs it
    /// and will not invent one: see `plan_objects`.
    mime: Option<String>,
}

struct EntityEntry {
    file: String,
    rows: u64,
    columns: Vec<(String, String)>,
}

fn field_str(v: &Value, key: &str) -> Result<String, ImportError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ImportError::Bundle(format!("the manifest has no string `{key}`")))
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, ImportError> {
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|e| ImportError::Bundle(format!("manifest.json does not parse: {e}")))?;
    let format = field_str(&v, "format")?;
    if format != FORMAT {
        return Err(ImportError::FormatUnknown { found: format });
    }
    let artifact = v
        .get("artifact")
        .ok_or_else(|| ImportError::Bundle("the manifest has no `artifact`".into()))?;
    let load = v
        .get("load")
        .ok_or_else(|| ImportError::Bundle("the manifest has no `load`".into()))?;
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut entities = BTreeMap::new();
    for e in v
        .get("entities")
        .and_then(Value::as_array)
        .ok_or_else(|| ImportError::Bundle("the manifest has no `entities` array".into()))?
    {
        let name = field_str(e, "entity")?;
        let columns = e
            .get("columns")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| {
                        Some((
                            c.get("name")?.as_str()?.to_string(),
                            c.get("encoding")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        entities.insert(
            name,
            EntityEntry {
                file: field_str(e, "file")?,
                rows: e.get("rows").and_then(Value::as_u64).unwrap_or(0),
                columns,
            },
        );
    }
    let digests = v
        .get("digests")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| {
                    Some((
                        d.get("member")?.as_str()?.to_string(),
                        d.get("sha256")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let blobs = v.get("blobs");
    Ok(Manifest {
        artifact_version: field_str(artifact, "version")?,
        // A program need not declare a domain, and the export writes the
        // absence as null. Absent on both sides is a match; the check below
        // is over what the two say rather than over whether they said
        // anything.
        domain: artifact
            .get("domain")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        sealed: field_str(&v, "sealed")?,
        order: strings(load.get("order")),
        cyclic: strings(load.get("cyclic")),
        entities,
        digests,
        profile_file: v
            .get("profile")
            .and_then(|p| p.get("file"))
            .and_then(Value::as_str)
            .map(str::to_string),
        blobs_carried: blobs
            .and_then(|b| b.get("carried"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        blob_objects: blobs
            .and_then(|b| b.get("objects"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        blob_files: blobs
            .and_then(|b| b.get("files"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|o| {
                        Some(ManifestObject {
                            member: o.get("member")?.as_str()?.to_string(),
                            entity: o.get("entity")?.as_str()?.to_string(),
                            field: o.get("field")?.as_str()?.to_string(),
                            source_id: o.get("id")?.as_str()?.to_string(),
                            mime: o.get("mime").and_then(Value::as_str).map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// One member, read and checked against the digest the manifest recorded.
///
/// The check is not optional politeness. The manifest is the only thing
/// that says what the bundle contains, and an import that loaded a
/// truncated `data/orders.jsonl` would report success over a short table.
fn member(
    src: &dyn BundleSource,
    mf: &Manifest,
    name: &str,
) -> Result<Option<Vec<u8>>, ImportError> {
    let Some(bytes) = src.read(name)? else {
        return Ok(None);
    };
    if let Some(want) = mf.digests.get(name) {
        let got = crate::export::bundle_digest(&bytes);
        if &got != want {
            return Err(ImportError::BundleIncomplete {
                member: name.to_string(),
                detail: format!("the manifest records sha256 {want}, the bytes hash to {got}"),
            });
        }
    }
    Ok(Some(bytes))
}

// ── value encoding, derived ──────────────────────────────────────────────

/// One JSON value from the bundle, as the source literal a call carries.
///
/// **The only place this module decides anything about a value**, and it
/// decides by exhaustive match on `ScalarTy`. Adding a scalar type to the
/// language breaks the build here, which is the right place to decide what
/// its literal spelling is and the wrong place to default it.
fn literal(f: &Field, v: &Value) -> Result<String, String> {
    if v.is_null() {
        return Ok("null".into());
    }
    let text = |v: &Value| -> Result<String, String> {
        let s = v
            .as_str()
            .ok_or_else(|| format!("expected a JSON string, the bundle has `{v}`"))?;
        str_literal(s)
    };
    match &f.ty {
        // A ref's value is the target's id, already rewritten to the
        // minted one by the caller. It is a string either way.
        FieldTy::Ref { .. } => text(v),
        FieldTy::Scalar(s) => match s {
            ScalarTy::Text | ScalarTy::Id | ScalarTy::Date | ScalarTy::Datetime | ScalarTy::Uri => {
                text(v)
            }
            ScalarTy::Int | ScalarTy::Money(_) | ScalarTy::Duration(_) => v
                .as_i64()
                .map(|n| n.to_string())
                .ok_or_else(|| format!("expected a JSON integer, the bundle has `{v}`")),
            ScalarTy::Bool => v
                .as_bool()
                .map(|b| b.to_string())
                .ok_or_else(|| format!("expected a JSON boolean, the bundle has `{v}`")),
            // `plan-export.md`: a JSON number when it round-trips through
            // `f64`'s grammar, the digit string otherwise. Both spellings
            // reach the language as a decimal literal, and the digit
            // string is the one that never lost anything.
            ScalarTy::Decimal(_) => match v {
                Value::Number(n) => Ok(n.to_string()),
                Value::String(s) => {
                    if s.chars()
                        .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+')
                        && s.chars().any(|c| c.is_ascii_digit())
                    {
                        Ok(s.clone())
                    } else {
                        Err(format!("`{s}` is not a decimal"))
                    }
                }
                other => Err(format!("expected a decimal, the bundle has `{other}`")),
            },
            // A blob value has no parameter form — `Sema::check_content_params`
            // refuses one — so `std.E.create` does not name a blob field and
            // this arm is unreachable through it. It is a refusal rather than
            // an `unreachable!` because the bundle is an outside input.
            // A document rides the source as its compact JSON text in a
            // string literal (28b). Compact JSON escapes every control
            // character itself, so the literal always has a spelling.
            ScalarTy::Json => serde_json::to_string(v)
                .map_err(|e| format!("the document does not serialize: {e}"))
                .and_then(|t| str_literal(&t)),
            ScalarTy::Blob => Err(
                "a blob field has no literal form: its only producer is an upload, which is \
                 deferred work"
                    .into(),
            ),
        },
    }
}

/// A source-language string literal.
///
/// The escapes are the closed set the lexer accepts (G4): `\"`, `\\`, `\n`,
/// `\t`. Anything else that cannot ride a source line raw is **refused**
/// rather than dropped or replaced. `ordo_host`'s `call_source` strips
/// embedded quotes instead, which silently changes the value; a loader that
/// did that would import data that is not the data.
fn str_literal(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c.is_control() && c != '\n' && c != '\t') => {
                return Err(format!(
                    "the value holds U+{:04X}, which has no escape in the language's closed \
                     set (\\\" \\\\ \\n \\t) and cannot ride a source line raw",
                    c as u32
                ));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

// ── one row, ready to be called ──────────────────────────────────────────

/// A bundle row, reduced to what the batch driver needs: its source id and
/// the fields `std.E.create` takes, in declaration order.
#[derive(Clone)]
struct Row {
    source_id: String,
    /// Field name, the raw JSON value, and whether it is a ref (and to
    /// what), which is what decides whether the value is rewritten.
    fields: Vec<(String, Value)>,
    /// The parent's source id, for a tree entity, so rows can be ordered
    /// parents-first inside their own file.
    parent: Option<String>,
}

/// The fields an import op declares: the declared fields minus the content
/// ones, exactly as `ordo_schema::std_ops` picks them for `std.E.create`.
///
/// A blob is excluded for that derivation's reason rather than for
/// convenience: `Sema::check_content_params` refuses a blob-typed op
/// parameter, because the only producer of a blob's value is an upload that
/// happens after the row exists. An op that named one would fail
/// registration for every entity holding a file.
fn creatable(e: &ordo_schema::Entity) -> Vec<&Field> {
    e.declared().filter(|f| !f.is_content()).collect()
}

fn import_op(entity: &str) -> String {
    format!("import.{entity}.create")
}

/// The op each entity's rows enter through: `import.<entity>.create`.
///
/// Derived from the schema, per entity, by the mechanism
/// `ordo_schema::acl_admin_ops` and `std_ops` already use: a `Program` of
/// ops registered beside the authored ones, through the same registration
/// gate.
///
/// # Why not `std.<entity>.create`
///
/// **11-10a's `std_ops` half is now fixed** (`ordo_schema::std_ops`'s create
/// arm carries `rev: null` only when the entity has a unique key), which
/// closes the defect this module used to work around: `std.E.create` no
/// longer refuses every row of a keyless entity. That makes the two
/// derivations identical for a **keyless** entity — same field set, no
/// `rev` either way — so `std.<entity>.create` would work there today.
///
/// For a **keyed** entity the two were different until 2026-09-28, and the
/// difference was a defect: `std.E.create` carried the create-once belief
/// (`rev: null`) and `import.E.create` did not, so two bundle rows under one
/// unique key converged silently onto one. The owner ruled **refuse**
/// (`sec6-import`), and the derivation below now carries the belief under
/// `std_ops`' own condition. This module still keeps its own derivation
/// rather than calling `std.<entity>.create`, because an import's op is
/// registered beside the deployed program's and must not depend on whether
/// that program exposes its `std` ops.
///
/// **Create** is the semantics an import wants either way. It loads into an
/// empty space, so there is nothing to converge onto for a source bundle
/// with no internal duplicates, which is the case this module's tests
/// cover today.
/// Unique keys an author declared, counted as `ordo_schema::std_ops` counts
/// them: `id` is unique and implicit and is not one of them.
fn declared_unique_keys(e: &ordo_schema::Entity) -> usize {
    e.declared().filter(|f| f.unique).count() + e.unique_keys.len()
}

fn import_ops(schema: &Schema) -> ordo_syntax::decl::Program {
    let sp = ordo_syntax::Span::new(0, 0, 1, 1);
    let ident = |n: &str| ordo_syntax::Ident {
        name: n.to_string(),
        span: sp,
    };
    let mut ops = Vec::new();
    for e in &schema.entities {
        // A platform entity has no derived writer and must not get one: the
        // membership surface writes `members` and `groupMembers` through
        // its own attributed path, and a bundle that could create access
        // rows would be a bundle that grants itself membership.
        if e.platform {
            continue;
        }
        let writable = creatable(e);
        let params: Vec<ordo_syntax::decl::ParamDecl> = writable
            .iter()
            .map(|f| ordo_syntax::decl::ParamDecl {
                name: ident(&f.name),
                ty: f.ty.value_ty(),
                // Mirrors the field, exactly as `std_ops` does. The bundle
                // carries every column so nothing relies on a default;
                // they are here so the op declares the shape the entity
                // declares rather than a stricter one.
                default: match (&f.default, f.optional) {
                    (Some(d), _) => Some(d.clone()),
                    (None, true) => Some(ordo_syntax::Expr::NullLit { span: sp }),
                    (None, false) => None,
                },
                max: None,
                // The field's own length bound, so an import bundle is
                // refused at the door exactly where a caller's value is.
                text_max: f.text_max,
                filled: false,
                span: sp,
            })
            .collect();
        ops.push(ordo_syntax::decl::OpDecl {
            name: ident(&import_op(&e.name)),
            params,
            // No flag, ever — the line `std_ops` draws, for its reason.
            // `unguarded` is the bypass a derivation may not take on an
            // author's behalf, and an import is not exempt: a row the
            // caller could not have written by hand is a row the import
            // must not write for them.
            flags: ordo_syntax::decl::OpFlags::default(),
            body: ordo_syntax::decl::OpBody::Write(vec![ordo_syntax::decl::WriteStmt::Set {
                binding: None,
                entity: ident(&e.name),
                cond: None,
                fields: writable
                    .iter()
                    .map(|f| {
                        (
                            ident(&f.name),
                            ordo_syntax::Expr::Param {
                                name: f.name.clone(),
                                span: sp,
                            },
                        )
                    })
                    // **Refuse a duplicate key** (owner-ruled 2026-09-28,
                    // `sec6-import`: "Refuse"). The create-once belief,
                    // `rev: null`, is what makes a second row under a key
                    // that already holds one answer `expected_absent`
                    // instead of converging onto the first. Without it two
                    // bundle rows sharing a unique key became one row, the
                    // import reported success, and the destination held
                    // fewer rows than the source with nothing to say so
                    // (`security-review-6.md` S6-007).
                    //
                    // Exactly `std_ops`' condition and for its reason: the
                    // belief is about the ONE row a unique key addresses,
                    // so a keyless entity cannot carry it (its braces
                    // address no row, and the protocol refuses `rev` there
                    // by name) and does not need it (with no key there is
                    // nothing for two rows to collide on).
                    .chain(
                        (declared_unique_keys(e) == 1)
                            .then(|| (ident("rev"), ordo_syntax::Expr::NullLit { span: sp })),
                    )
                    .collect(),
                span: sp,
            }]),
            span: sp,
        });
    }
    ordo_syntax::decl::Program {
        domain: None,
        entities: Vec::new(),
        ops,
        release: None,
        routines: Vec::new(),
        secrets: Vec::new(),
    }
}

/// Turn one JSONL member into rows, checking the bundle's columns against
/// the schema in **both directions**.
///
/// Both directions, because the two failures are different and both are
/// silent. A column the schema does not have is data that would be dropped;
/// a field the bundle does not carry is a column that would be defaulted,
/// which for a required field is a refusal at the protocol and for an
/// optional one is a null nobody asked for.
fn rows_of(
    schema: &Schema,
    entity: &ordo_schema::Entity,
    entry: &EntityEntry,
    body: &str,
) -> Result<Vec<Row>, ImportError> {
    let want: Vec<&Field> = creatable(entity);
    let carried: BTreeSet<&str> = entry.columns.iter().map(|(n, _)| n.as_str()).collect();
    for f in &want {
        if !carried.contains(f.name.as_str()) {
            return Err(ImportError::FieldMismatch {
                entity: entity.name.clone(),
                field: f.name.clone(),
                detail: "this schema declares it and the bundle does not carry it".into(),
            });
        }
    }
    // A column the bundle carries that is neither a field this schema
    // declares nor one of the two the bundle always carries (`$space`,
    // `id`) nor an implicit one (`rev`, `depth`) is a column with nowhere
    // to go.
    let declared: BTreeSet<&str> = entity.fields.iter().map(|f| f.name.as_str()).collect();
    for (name, _encoding) in &entry.columns {
        // **A blob column's value is dropped here, deliberately.** It is
        // the SOURCE space's handle: it names an object in the source's
        // bucket, sealed under the source's key, and no statement in the
        // destination could make it mean anything. The object rides the
        // bundle as its own member and is bound to the row by an upload
        // after the row exists (`attach_objects`), which is the only way a
        // blob column is ever written. So the column falls through to the
        // `declared` arm below like any other field this call does not
        // carry, and what makes that safe rather than silent is that a
        // member with no row, or a row with no member, is a refusal.
        //
        // There used to be a refusal on this line, keyed on
        // `encoding == "blob"`. It was unreachable: the export writes this
        // encoding as `blob_ref` (`export.rs`'s `encoding_name`), so the
        // arm had never fired for any bundle the export produced, and the
        // refusal that did the work was the one over the member list.
        //
        // `$space` is a fact about the rows the destination restamps, and
        // `$path` is a tree position the platform derives from the parent
        // chain rather than accepting from a caller (`write.rs`: "there is
        // no spelling by which a caller can claim a position in the tree it
        // did not earn through a real parent"). Both ride the bundle so it
        // is loadable by ordinary tooling against its own `schema.sql`, and
        // both are the destination's to write.
        if name == "$space" || name == "$path" || declared.contains(name.as_str()) {
            continue;
        }
        return Err(ImportError::FieldMismatch {
            entity: entity.name.clone(),
            field: name.clone(),
            detail: "the bundle carries it and this schema does not declare it".into(),
        });
    }
    // The tree self-reference, if any: the field whose target is this
    // entity. Read from the schema, never from a name.
    let tree_field = want.iter().find_map(|f| match &f.ty {
        FieldTy::Ref { target, tree, .. } if *tree && target == &entity.name => {
            Some(f.name.clone())
        }
        _ => None,
    });
    // Every ref's target must be an entity this schema knows, or the
    // rewrite has nowhere to look.
    for f in &want {
        if let FieldTy::Ref { target, .. } = &f.ty {
            if schema.entity(target).is_none() {
                return Err(ImportError::FieldMismatch {
                    entity: entity.name.clone(),
                    field: f.name.clone(),
                    detail: format!("it references `{target}`, which this schema does not have"),
                });
            }
        }
    }

    let mut out = Vec::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).map_err(|e| {
            ImportError::Bundle(format!(
                "`{}`: a JSONL line does not parse: {e}",
                entry.file
            ))
        })?;
        let source_id = v
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ImportError::Bundle(format!("`{}`: a row has no string `id`", entry.file))
            })?
            .to_string();
        let mut fields = Vec::with_capacity(want.len());
        for f in &want {
            let value = v.get(&f.name).cloned().unwrap_or(Value::Null);
            fields.push((f.name.clone(), value));
        }
        let parent = tree_field.as_ref().and_then(|tf| {
            v.get(tf)
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|p| !p.is_empty())
        });
        out.push(Row {
            source_id,
            fields,
            parent,
        });
    }
    if out.len() as u64 != entry.rows {
        return Err(ImportError::BundleIncomplete {
            member: entry.file.clone(),
            detail: format!(
                "the manifest records {} row(s) and the file holds {}",
                entry.rows,
                out.len()
            ),
        });
    }
    Ok(out)
}

/// Order a tree entity's rows parents-first.
///
/// `plan-export.md` §2.2 hands this to import by name: *"A `tree`
/// self-reference orders ROWS within its own table (parents before
/// children) and appears in neither list. Row-level ordering within a
/// self-referencing table is import's problem, not the manifest's."*
///
/// Kahn over the rows' own parent links, taking ready rows in file order so
/// the result is deterministic. A row whose parent is not in the file is a
/// root here and an `unresolved_ref` at call time, which is the right
/// place for it: this function orders, it does not validate.
fn parents_first(entity: &str, rows: Vec<Row>) -> Result<Vec<Row>, ImportError> {
    let present: BTreeSet<&str> = rows.iter().map(|r| r.source_id.as_str()).collect();
    let mut children: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut waiting: Vec<usize> = Vec::with_capacity(rows.len());
    let mut ready: Vec<usize> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        match r.parent.as_deref().filter(|p| present.contains(p)) {
            Some(p) => {
                children.entry(p).or_default().push(i);
                waiting.push(i);
            }
            None => ready.push(i),
        }
    }
    let mut remaining: HashMap<usize, usize> = waiting.iter().map(|i| (*i, 1usize)).collect();
    let mut order: Vec<usize> = Vec::with_capacity(rows.len());
    let mut queue: std::collections::VecDeque<usize> = ready.into_iter().collect();
    while let Some(i) = queue.pop_front() {
        order.push(i);
        if let Some(kids) = children.get(rows[i].source_id.as_str()) {
            for k in kids {
                if let Some(n) = remaining.get_mut(k) {
                    *n -= 1;
                    if *n == 0 {
                        queue.push_back(*k);
                    }
                }
            }
        }
    }
    if order.len() != rows.len() {
        let placed: BTreeSet<usize> = order.iter().copied().collect();
        return Err(ImportError::TreeCycle {
            entity: entity.to_string(),
            rows: rows
                .iter()
                .enumerate()
                .filter(|(i, _)| !placed.contains(i))
                .map(|(_, r)| r.source_id.clone())
                .collect(),
        });
    }
    let mut by_index: Vec<Option<Row>> = rows.into_iter().map(Some).collect();
    Ok(order
        .into_iter()
        .map(|i| by_index[i].take().expect("each row placed once"))
        .collect())
}

// ── the import ───────────────────────────────────────────────────────────

/// Import one bundle into `exec`'s space.
pub async fn import_space(
    exec: &Executor,
    req: &ImportRequest<'_>,
    src: &dyn BundleSource,
) -> Result<ImportReport, ImportError> {
    let space = exec.space.clone();
    let schema = &exec.schema;

    let manifest_bytes =
        src.read("manifest.json")?
            .ok_or_else(|| ImportError::BundleIncomplete {
                member: "manifest.json".into(),
                detail: "a bundle without its manifest describes nothing".into(),
            })?;
    let mf = parse_manifest(&manifest_bytes)?;

    // ── the checks, all of them before a single row is written ──────────
    if mf.sealed != crate::export::SealedMode::Open.as_str() {
        return Err(ImportError::SealedCiphertext);
    }
    if !mf.cyclic.is_empty() {
        return Err(ImportError::CyclicEntities {
            entities: mf.cyclic.clone(),
        });
    }
    // **The objects, checked in full before a row is written.** Everything
    // about them that does not need the mapping is decided here: the
    // manifest and the member list agree, every object names a blob field
    // of this schema, every tier is one the door can serve, and every
    // member has a mime. What is left for after the rows is which row each
    // one lands on, which nothing can know until the ids are minted.
    let objects = plan_objects(&mf, src, schema)?;
    if !objects.is_empty() && req.blobs.is_none() {
        return Err(ImportError::BlobDoorMissing {
            objects: objects.len() as u64,
        });
    }
    let deployed_domain = schema.domain.clone().unwrap_or_default();
    if mf.artifact_version != req.artifact.version || mf.domain != deployed_domain {
        return Err(ImportError::ArtifactMismatch {
            bundle: mf.artifact_version.clone(),
            deployed: req.artifact.version.clone(),
            detail: if mf.domain != deployed_domain {
                format!(
                    "the bundle's domain is `{}` and this space serves `{deployed_domain}`",
                    mf.domain
                )
            } else {
                "same domain, different artifact version".into()
            },
        });
    }

    // The order the manifest states, narrowed to the entities it carries a
    // file for. Following the manifest and nothing else is the property the
    // export's round-trip test was written to hold, and it is the reason
    // the ref rewrite can work at all.
    let order: Vec<String> = mf
        .order
        .iter()
        .filter(|e| mf.entities.contains_key(*e))
        .cloned()
        .collect();

    ensure_importmap(&exec.pool).await?;

    // Read every member and check it, then refuse an already-populated
    // space, then write. Nothing is written until everything is read,
    // because a refusal after the third of nine entities leaves a space
    // that is neither empty nor imported.
    let mut planned: Vec<(String, Vec<Row>)> = Vec::new();
    for name in &order {
        let entry = &mf.entities[name];
        let entity = schema.entity(name).ok_or_else(|| {
            ImportError::Bundle(format!(
                "the bundle carries `{name}` and this schema has no such entity, which the \
                     artifact check should have caught"
            ))
        })?;
        if entity.platform {
            // The membership surface writes `members` and `groupMembers`
            // through its own attributed path, and `std_ops` derives
            // nothing for a platform entity. Importing them through
            // `std.E.create` is impossible and pretending otherwise would
            // produce a space whose access rows came from a bundle.
            continue;
        }
        let body = member(src, &mf, &entry.file)?.ok_or_else(|| ImportError::BundleIncomplete {
            member: entry.file.clone(),
            detail: "the manifest names it and the bundle does not have it".into(),
        })?;
        let body = String::from_utf8(body)
            .map_err(|e| ImportError::Bundle(format!("`{}` is not UTF-8: {e}", entry.file)))?;
        let mut rows = rows_of(schema, entity, entry, &body)?;
        if rows.iter().any(|r| r.parent.is_some()) {
            rows = parents_first(name, rows)?;
        }
        planned.push((name.clone(), rows));
    }

    // **Empty, or holding nothing but this import's own residue.**
    //
    // A flat "the space must be empty" would refuse the case this whole
    // design is built for: an import interrupted after four batches leaves
    // rows behind, and a rule that would not let it resume would make
    // resume unreachable by the only route that needs it. So the rule is
    // stated against `$importmap` under **this import's name**: a row this
    // import minted is accounted for, and any row it did not is somebody
    // else's, which is the merge nobody has ruled a policy for.
    for (name, _) in &planned {
        let live = count_rows(&exec.pool, &space, name).await?;
        let mine = count_mapped(&exec.pool, &space, req.name, name).await?;
        if live > mine {
            return Err(ImportError::SpaceNotEmpty {
                entity: name.clone(),
                rows: live - mine,
            });
        }
    }

    // ── the run ─────────────────────────────────────────────────────────
    //
    // The caller's fixture with the derived import ops registered beside
    // its authored ones, through the same gate `Fixture::compile_source`
    // puts `std_ops` and `acl_admin_ops` through. Registration is not a
    // formality: it is where an op that ranges without an acknowledgment,
    // or names a parameter the compiler refuses, is caught — and a
    // derivation that skipped it would be the one derived op in the tree
    // with a private door.
    let mut fixture = req.fixture.in_space(&space.0);
    fixture.registry.extend_from(&import_ops(schema));
    ordo_plan::cost::check_registration(&fixture.schema, &fixture.registry)
        .map_err(|e| ImportError::Bundle(format!("the derived import ops do not register: {e}")))?;
    let fixture = Arc::new(fixture);

    let mut mapping = load_mapping(&exec.pool, &space, req.name).await?;
    // **Before the first row**, on `plan_objects`' reasoning: a map that
    // cannot be honoured should refuse while the destination is still empty,
    // not half way through the load order when the first `owner` column
    // arrives.
    let people_seeded = seed_people(&exec.pool, &space, req.name, req.people, &mut mapping).await?;
    // The access profile is read and checked here too, before the first row:
    // a line the program cannot honour, or a member the map cannot place,
    // refuses while the destination is still empty.
    let profile = plan_profile(exec, req, src, &mf, &fixture, &mapping).await?;
    let batch_size = req.batch.max(1);
    let mut report = ImportReport {
        space: space.0.clone(),
        name: req.name.to_string(),
        entities: Vec::new(),
        batches_written: 0,
        batches_skipped: 0,
        groups: 0,
        objects_attached: 0,
        objects_skipped: 0,
        profile_rows: 0,
        people_mapped: people_seeded,
    };

    for (name, rows) in &planned {
        let entity = schema.entity(name).expect("checked above");
        let fields: Vec<FieldPlan> = creatable(entity)
            .into_iter()
            .map(|f| FieldPlan {
                field: f.clone(),
                target: match &f.ty {
                    FieldTy::Ref { target, .. } => Some(target.clone()),
                    FieldTy::Scalar(_) => None,
                },
            })
            .collect();
        for (k, chunk) in rows.chunks(batch_size).enumerate() {
            let key = format!("import/{}/{}/{}", req.name, name, k);
            let fingerprint = batch_fingerprint(chunk);
            let applied = run_batch(
                exec,
                req,
                fixture.clone(),
                name,
                &fields,
                chunk.to_vec(),
                mapping.clone(),
                &key,
                &fingerprint,
            )
            .await?;
            if applied {
                report.batches_written += 1;
                report.groups += 1;
            } else {
                report.batches_skipped += 1;
            }
            // Re-read the mapping the batch wrote rather than trusting what
            // this process thinks it wrote. It is the same query on both
            // paths, which is what makes a resumed run and a fresh one
            // resolve refs identically: a skipped batch's mapping is in the
            // table and nowhere else.
            mapping = load_mapping(&exec.pool, &space, req.name).await?;
        }
        report.entities.push((name.clone(), rows.len() as u64));
    }

    // **After every row of every entity, and that is the whole of where the
    // attach sits.**
    //
    // An object is bound to a row that exists, so it cannot precede its own
    // row; and no object references another object, so there is no ordering
    // among them beyond that. One pass at the end is therefore the simplest
    // order the dependency admits, and it leaves the row batches exactly as
    // they were: the same keys, the same fingerprints, the same groups, the
    // same resume.
    //
    // **It is not inside a batch, and it cannot be.** A batch's closure is
    // re-run on a serialization conflict, so it is `Fn + Clone`; an upload
    // token is minted from bytes that were actually stored and is consumed
    // by value, so it is neither. The bytes also have to reach the store
    // *before* the transaction that names them opens, which is the door's
    // own ordering and the reason an interrupted upload leaks money rather
    // than a guarantee.
    //
    // **The batch's shared correlation is not weakened by that.** It is
    // load-bearing because `$sealedpayload` rows are collected by an
    // anti-join over correlations no longer in the chain, so a payload
    // written under a correlation no group carries is collectable. Each
    // attach is its own group with its own correlation, appended to this
    // space's chain by the door, so any payload it writes is named by a
    // group that is in the chain — which is the property, rather than
    // "one correlation per import". The tier whose bytes ARE a
    // `$sealedpayload` row is `blob inline`, and it is refused above by
    // name.
    if let Some(door) = req.blobs {
        attach_objects(exec, req, door, src, &mf, &objects, &mapping, &mut report).await?;
    }
    // The people are mapped and the rows are in. The profile goes last so
    // that everything it names exists.
    if let Some(plan) = profile {
        report.profile_rows = restore_profile(exec, req, fixture.clone(), plan, &mapping).await?;
    }
    Ok(report)
}

// ── the access profile ───────────────────────────────────────────────────

/// `$importmap` entity for the marker that says the profile has landed.
const PROFILE_MARKER: &str = "$profile";

/// A bundle's access profile, read and checked.
struct ProfilePlan {
    lines: Vec<(String, Vec<String>, Vec<String>)>,
    overrides: Vec<(String, String, String)>,
    /// `(source id, display name)`.
    groups: Vec<(String, String)>,
    /// `(source group, source member)`.
    links: Vec<(String, String)>,
    /// Digest of the member's bytes, for the batch's lease.
    fingerprint: String,
}

impl ProfilePlan {
    fn rows(&self) -> u64 {
        (self.lines.len() + self.overrides.len() + self.groups.len() + self.links.len()) as u64
    }
}

fn refuse(class: &'static str, detail: impl Into<String>) -> ImportError {
    ImportError::ProfileRefused {
        class,
        detail: detail.into(),
    }
}

fn strs(v: &Value, key: &str) -> Result<Vec<String>, ImportError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| refuse("profile_invalid", format!("`{key}` holds a non-string")))
            })
            .collect(),
        Some(_) => Err(refuse("profile_invalid", format!("`{key}` is not a list"))),
    }
}

fn str_of(v: &Value, key: &str) -> Result<String, ImportError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| refuse("profile_invalid", format!("a row has no string `{key}`")))
}

fn list_of<'a>(body: &'a Value, key: &str) -> Result<&'a [Value], ImportError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(refuse("profile_invalid", format!("`{key}` is not a list"))),
    }
}

/// Read the profile member, check every line against the program and every
/// member against the people map, and check the destination holds no
/// profile of its own. `None` when there is nothing to restore (no member,
/// an empty one, or one a previous run already restored).
async fn plan_profile(
    exec: &Executor,
    req: &ImportRequest<'_>,
    src: &dyn BundleSource,
    mf: &Manifest,
    fixture: &Fixture,
    mapping: &HashMap<(String, String), String>,
) -> Result<Option<ProfilePlan>, ImportError> {
    let Some(file) = &mf.profile_file else {
        return Ok(None);
    };
    let bytes = member(src, mf, file)?.ok_or_else(|| ImportError::BundleIncomplete {
        member: file.clone(),
        detail: "the manifest names it and the bundle does not have it".into(),
    })?;
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|e| refuse("profile_invalid", format!("`{file}` does not parse: {e}")))?;
    if body.get("format").and_then(Value::as_str) != Some(crate::export::PROFILE_FORMAT) {
        return Err(refuse(
            "profile_invalid",
            format!(
                "`{file}` is not a `{}` document",
                crate::export::PROFILE_FORMAT
            ),
        ));
    }

    let mut groups = Vec::new();
    for g in list_of(&body, "groups")? {
        groups.push((str_of(g, "id")?, str_of(g, "name")?));
    }
    let group_ids: BTreeSet<&str> = groups.iter().map(|(i, _)| i.as_str()).collect();
    let program: BTreeSet<&str> = fixture.registry.names().map(String::as_str).collect();
    let person = |id: &str| -> Result<(), ImportError> {
        if mapping.contains_key(&(PEOPLE_ENTITY.to_string(), id.to_string())) {
            Ok(())
        } else {
            Err(refuse(
                "profile_member_unmapped",
                format!(
                    "the profile names member `{id}` and the people map does not place them. \
                     Add them to the destination and to the map, so the profile never lands \
                     on the wrong person"
                ),
            ))
        }
    };
    let known_op = |op: &str| -> Result<(), ImportError> {
        if program.contains(op) {
            Ok(())
        } else {
            Err(refuse(
                "profile_op_unknown",
                format!(
                    "the profile sets a line for `{op}` and this program has no such op. \
                     Dropping the line would leave `{op}` at the app developer's default, \
                     which may be wider than what the tenant set. Remove it from the \
                     bundle's profile.json on purpose, or deploy the program that has the op"
                ),
            ))
        }
    };

    let mut lines = Vec::new();
    for l in list_of(&body, "lines")? {
        let op = str_of(l, "op")?;
        known_op(&op)?;
        let roles = strs(l, "roles")?;
        for r in &roles {
            if !["owner", "manager", "editor", "viewer"].contains(&r.as_str()) {
                return Err(refuse(
                    "profile_invalid",
                    format!("the line for `{op}` names `{r}`, which is not a role"),
                ));
            }
        }
        let gs = strs(l, "groups")?;
        for g in &gs {
            if !group_ids.contains(g.as_str()) {
                return Err(refuse(
                    "profile_invalid",
                    format!(
                        "the line for `{op}` names group `{g}`, which the bundle does not carry"
                    ),
                ));
            }
        }
        lines.push((op, roles, gs));
    }
    let mut overrides = Vec::new();
    for o in list_of(&body, "overrides")? {
        let (member, op, effect) = (str_of(o, "member")?, str_of(o, "op")?, str_of(o, "effect")?);
        known_op(&op)?;
        if effect != "allow" && effect != "deny" {
            return Err(refuse(
                "profile_invalid",
                format!("the override for `{member}` on `{op}` has effect `{effect}`"),
            ));
        }
        person(&member)?;
        overrides.push((member, op, effect));
    }
    let mut links = Vec::new();
    for l in list_of(&body, "links")? {
        let (group, member) = (str_of(l, "group")?, str_of(l, "member")?);
        if !group_ids.contains(group.as_str()) {
            return Err(refuse(
                "profile_invalid",
                format!("a link names group `{group}`, which the bundle does not carry"),
            ));
        }
        person(&member)?;
        links.push((group, member));
    }
    let plan = ProfilePlan {
        lines,
        overrides,
        groups,
        links,
        fingerprint: crate::export::bundle_digest(&bytes),
    };
    if plan.rows() == 0 {
        return Ok(None);
    }

    // Already restored by an earlier run of this import: the marker rides the
    // same transaction as the profile, so it is there exactly when the
    // profile is.
    let space = &exec.space;
    if count_mapped(&exec.pool, space, req.name, PROFILE_MARKER).await? > 0 {
        return Ok(None);
    }
    // Merging a bundle's profile into a tenant that already has one has no
    // ruled policy, and a silent merge could widen access.
    let sp = crate::sql_lit(&space.0);
    for (table, sql) in [
        (
            "$opaccess",
            format!(r#"SELECT count(*) FROM "$opaccess" WHERE "$space" = {sp}"#),
        ),
        (
            "$memberaccess",
            format!(r#"SELECT count(*) FROM "$memberaccess" WHERE "$space" = {sp}"#),
        ),
        (
            "members",
            format!(r#"SELECT count(*) FROM "members" WHERE "$space" = {sp} AND "kind" = 'group'"#),
        ),
    ] {
        let present = exec
            .pool
            .rows(&format!(r#"SELECT to_regclass('"{table}"') IS NOT NULL"#))
            .await?;
        let yes = matches!(
            present
                .first()
                .and_then(|r| r.first())
                .and_then(|c| c.as_deref()),
            Some("t") | Some("true")
        );
        if !yes {
            continue;
        }
        let n = exec
            .pool
            .rows(&sql)
            .await?
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        if n > 0 {
            return Err(refuse(
                "profile_destination_not_empty",
                format!(
                    "this space already holds {n} row(s) of `{table}` that this import did not \
                     write, and the bundle carries a profile. Import into a space with no \
                     groups or profile lines of its own"
                ),
            ));
        }
    }
    Ok(Some(plan))
}

/// Restore the profile as one batch: the groups, their links, the op lines
/// and the overrides, each through the same command the host's profile ops
/// run, so the destination's chain records them as commands by `req.caller`.
async fn restore_profile(
    exec: &Executor,
    req: &ImportRequest<'_>,
    fixture: Arc<Fixture>,
    plan: ProfilePlan,
    mapping: &HashMap<(String, String), String>,
) -> Result<u64, ImportError> {
    use ordo_conformance::access_profile::{ProfileCommand, MEMBERACCESS_DDL, OPACCESS_DDL};
    use ordo_conformance::membership::GroupCommand;

    let rows = plan.rows();
    let plan = Arc::new(plan);
    let mapping = mapping.clone();
    let caller = req.caller.to_string();
    let import = req.name.to_string();
    let space = exec.space.0.clone();
    let key = format!("import/{}/{}/0", req.name, PROFILE_MARKER);
    let correlation = format!("cg_import_{}", crate::export::bundle_digest(key.as_bytes()));
    let log = Arc::new(Mutex::new(import_log(&correlation)));
    let fingerprint = plan.fingerprint.clone();
    let outcome = exec
        .import_batch(
            log,
            Some((key, fingerprint)),
            correlation,
            move |db, wlog| {
                db.rows(OPACCESS_DDL)?;
                db.rows(MEMBERACCESS_DDL)?;
                let actor = Some(caller.as_str());
                let note = |entity: &str, source: &str, minted: &str| {
                    db.rows(&format!(
                        "INSERT INTO \"$importmap\" \
                         (\"$space\", \"import\", \"entity\", \"source_id\", \"row_id\") \
                         VALUES ({}, {}, {}, {}, {})",
                        crate::sql_lit(&space),
                        crate::sql_lit(&import),
                        crate::sql_lit(entity),
                        crate::sql_lit(source),
                        crate::sql_lit(minted),
                    ))
                };
                let person = |id: &str| -> Result<String, String> {
                    mapping
                        .get(&(PEOPLE_ENTITY.to_string(), id.to_string()))
                        .cloned()
                        .ok_or_else(|| format!("profile_member_unmapped: `{id}`"))
                };
                let mut group_of: BTreeMap<String, String> = BTreeMap::new();
                for (source, name) in &plan.groups {
                    let out = fixture.group_command_on(
                        db,
                        wlog,
                        GroupCommand::Create { display_name: name },
                        actor,
                    )?;
                    let minted = out["created"]
                        .as_str()
                        .ok_or_else(|| format!("the group command reported no id: {out}"))?
                        .to_string();
                    note(PEOPLE_ENTITY, source, &minted)?;
                    group_of.insert(source.clone(), minted);
                }
                for (group, member) in &plan.links {
                    fixture.group_command_on(
                        db,
                        wlog,
                        GroupCommand::Add {
                            group: &group_of[group],
                            member: &person(member)?,
                        },
                        actor,
                    )?;
                }
                for (op, roles, groups) in &plan.lines {
                    let mut mapped: Vec<String> =
                        groups.iter().map(|g| group_of[g].clone()).collect();
                    mapped.sort();
                    mapped.dedup();
                    fixture.profile_command_on(
                        db,
                        wlog,
                        ProfileCommand::SetOp {
                            op,
                            roles,
                            groups: &mapped,
                        },
                        actor,
                    )?;
                }
                for (member, op, effect) in &plan.overrides {
                    fixture.profile_command_on(
                        db,
                        wlog,
                        ProfileCommand::SetMember {
                            member: &person(member)?,
                            op,
                            effect: Some(effect),
                        },
                        actor,
                    )?;
                }
                note(PROFILE_MARKER, "profile", "restored")?;
                Ok(Value::Array(Vec::new()))
            },
        )
        .await;
    match outcome {
        Ok(Some(_)) => Ok(rows),
        Ok(None) => Ok(0),
        Err(ExecFail::Pool(e)) => Err(ImportError::Store(e)),
        Err(e) => Err(refuse("profile_invalid", e.to_string())),
    }
}

/// A field of the entity being imported, with the target it references.
#[derive(Clone)]
struct FieldPlan {
    field: Field,
    target: Option<String>,
}

/// The batch's identity, for the idempotency journal.
///
/// The source ids in order. It is what makes a re-run of a **changed**
/// bundle under the same import name answer `idempotency_mismatch` instead
/// of silently skipping a batch whose rows are no longer the rows that
/// committed.
fn batch_fingerprint(rows: &[Row]) -> String {
    let joined: Vec<&str> = rows.iter().map(|r| r.source_id.as_str()).collect();
    crate::export::bundle_digest(joined.join("\n").as_bytes())
}

/// One batch: one transaction, one lease, one command group, one append.
///
/// Returns `true` when this run committed it and `false` when a previous
/// run had, which is the whole of resume.
#[allow(clippy::too_many_arguments)]
async fn run_batch(
    exec: &Executor,
    req: &ImportRequest<'_>,
    fixture: Arc<Fixture>,
    entity: &str,
    fields: &[FieldPlan],
    rows: Vec<Row>,
    mapping: HashMap<(String, String), String>,
    key: &str,
    fingerprint: &str,
) -> Result<bool, ImportError> {
    let caller = req.caller.to_string();
    let entity_owned = entity.to_string();
    let fields = fields.to_vec();
    let space = exec.space.0.clone();
    let import = req.name.to_string();
    // The batch's correlation. Deterministic in the key, so the group a
    // re-run would build carries the same correlation as the one that
    // committed — which keeps a chain read of a resumed import indis-
    // tinguishable from an uninterrupted one.
    let correlation = format!("cg_import_{}", crate::export::bundle_digest(key.as_bytes()));

    // Kept for the refusal's sake: the closure takes the rows and the
    // error path still has to name the batch it was refusing.
    let first_id = rows
        .first()
        .map(|r| r.source_id.clone())
        .unwrap_or_default();
    let log = Arc::new(Mutex::new(import_log(&correlation)));
    let outcome = exec
        .import_batch(
            log,
            Some((key.to_string(), fingerprint.to_string())),
            correlation.clone(),
            move |db, wlog| {
                let mut map = mapping.clone();
                let mut summaries: Vec<Value> = Vec::new();
                for row in &rows {
                    let call = build_call(&entity_owned, &fields, row, &map)?;
                    let out = fixture.run_write_on(db, wlog, &call, Some(&caller))?;
                    let minted = created_id(&out).ok_or_else(|| {
                        format!(
                            "the protocol accepted `{}` source row `{}` and reported no created \
                             row: {out}",
                            entity_owned, row.source_id
                        )
                    })?;
                    db.rows(&format!(
                        "INSERT INTO \"$importmap\" \
                         (\"$space\", \"import\", \"entity\", \"source_id\", \"row_id\") \
                         VALUES ({}, {}, {}, {}, {})",
                        crate::sql_lit(&space),
                        crate::sql_lit(&import),
                        crate::sql_lit(&entity_owned),
                        crate::sql_lit(&row.source_id),
                        crate::sql_lit(&minted),
                    ))?;
                    map.insert(
                        (entity_owned.clone(), row.source_id.clone()),
                        minted.clone(),
                    );
                    summaries.extend(out.as_array().cloned().unwrap_or_default());
                }
                Ok(Value::Array(summaries))
            },
        )
        .await;

    match outcome {
        Ok(Some(_)) => Ok(true),
        // The lease's effect had already committed. The batch is done and
        // re-running it is exactly what the key exists to prevent.
        Ok(None) => Ok(false),
        Err(ExecFail::Pool(e)) => Err(ImportError::Store(e)),
        Err(e) => Err(ImportError::RowRefused {
            entity: entity.to_string(),
            source_id: first_id,
            detail: e.to_string(),
        }),
    }
}

/// A write log for one batch.
///
/// `WriteLog::serving` rather than `new`, because a counter that restarts
/// mints `cmd_1` for a chain that already holds it. The prefix is the
/// batch's own correlation digest, which is unique per batch by
/// construction and stable across a resume.
fn import_log(correlation: &str) -> WriteLog {
    let prefix = format!("{}_", &correlation[..correlation.len().min(20)]);
    WriteLog::serving(
        prefix,
        Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        }),
    )
}

/// The id the protocol minted, from its own summary.
fn created_id(out: &Value) -> Option<String> {
    out.as_array()?
        .iter()
        .find_map(|s| s.get("created").and_then(Value::as_str))
        .map(str::to_string)
}

/// `std.<entity>.create($f: <literal>, …)`, with every ref rewritten from
/// the source key to the minted one.
fn build_call(
    entity: &str,
    fields: &[FieldPlan],
    row: &Row,
    map: &HashMap<(String, String), String>,
) -> Result<String, String> {
    let mut args = Vec::with_capacity(fields.len());
    for fp in fields {
        let raw = row
            .fields
            .iter()
            .find(|(n, _)| n == &fp.field.name)
            .map(|(_, v)| v)
            .cloned()
            .unwrap_or(Value::Null);
        // A ref carries the SOURCE target's id. Rewriting it is what the
        // load order exists for, and a miss is a refusal: a ref is a
        // foreign key on ($space, id), so a guess would either fail at the
        // database or point at somebody else's row.
        let value = match (&fp.target, raw.as_str()) {
            (Some(target), Some(source_target_id)) => {
                let minted = map
                    .get(&(target.clone(), source_target_id.to_string()))
                    .ok_or_else(|| {
                        ImportError::UnresolvedRef {
                            entity: entity.to_string(),
                            field: fp.field.name.clone(),
                            source_id: row.source_id.clone(),
                            target: target.clone(),
                            target_id: source_target_id.to_string(),
                        }
                        .to_string()
                    })?;
                Value::String(minted.clone())
            }
            _ => raw,
        };
        let lit = literal(&fp.field, &value).map_err(|detail| {
            ImportError::Unencodable {
                entity: entity.to_string(),
                field: fp.field.name.clone(),
                source_id: row.source_id.clone(),
                detail,
            }
            .to_string()
        })?;
        args.push(format!("${}: {}", fp.field.name, lit));
    }
    Ok(format!("{}({})", import_op(entity), args.join(", ")))
}

// ── the objects ──────────────────────────────────────────────────────────

/// One object, checked against the schema and waiting for its row.
struct PlannedObject {
    member: String,
    entity: String,
    field: String,
    source_id: String,
    mime: String,
}

/// Everything about the bundle's objects that can be decided **before a row
/// is written**, for the reason the row planning above gives: a refusal
/// after the third of nine entities leaves a space that is neither empty nor
/// imported, and an object whose field does not exist is exactly the kind of
/// refusal that would otherwise arrive at the end of a million rows.
///
/// # The manifest and the member list are checked against each other
///
/// The old refusal here read the two separately and refused either way,
/// with the reason that *"they can only disagree if the bundle was assembled
/// by something other than the export, which is exactly when a loader that
/// trusted the manifest alone would drop files silently."* That reason
/// survives the feature: the two are now required to **agree**, so a member
/// the manifest does not describe is a refusal and a described object the
/// bundle does not hold is a refusal, and neither is a file quietly left on
/// the floor.
fn plan_objects(
    mf: &Manifest,
    src: &dyn BundleSource,
    schema: &Schema,
) -> Result<Vec<PlannedObject>, ImportError> {
    let mut carried: BTreeSet<String> = src
        .names()?
        .into_iter()
        .filter(|n| n.starts_with("blobs/"))
        .collect();

    if mf.blob_objects != mf.blob_files.len() as u64 {
        return Err(ImportError::BundleIncomplete {
            member: "manifest.json".into(),
            detail: format!(
                "`blobs.objects` says {} and `blobs.files` describes {}",
                mf.blob_objects,
                mf.blob_files.len()
            ),
        });
    }
    if !mf.blobs_carried && !mf.blob_files.is_empty() {
        return Err(ImportError::BundleIncomplete {
            member: "manifest.json".into(),
            detail: format!(
                "`blobs.carried` is false, which means no object store was consulted, and \
                 `blobs.files` describes {} object(s)",
                mf.blob_files.len()
            ),
        });
    }

    let mut out = Vec::with_capacity(mf.blob_files.len());
    for o in &mf.blob_files {
        if !carried.remove(&o.member) {
            return Err(ImportError::BundleIncomplete {
                member: o.member.clone(),
                detail: "the manifest's `blobs.files` describes it and the bundle does not \
                         hold it"
                    .into(),
            });
        }
        let entity = schema
            .entity(&o.entity)
            .ok_or_else(|| ImportError::BlobUnplaced {
                member: o.member.clone(),
                detail: format!(
                    "it names entity `{}`, which this schema does not have",
                    o.entity
                ),
            })?;
        let field = entity
            .field(&o.field)
            .filter(|f| f.is_content())
            .ok_or_else(|| ImportError::BlobUnplaced {
                member: o.member.clone(),
                detail: format!(
                    "`{}`.`{}` is not a blob field of this schema",
                    o.entity, o.field
                ),
            })?;
        // The three tiers the blob door declines, declined here first. Each
        // reason is the door's own and is not restated: this is the same
        // refusal arriving before the upload rather than during it.
        if field.private {
            return Err(ImportError::BlobTierUnsupported {
                entity: o.entity.clone(),
                field: o.field.clone(),
                tier: "private",
                detail: "client-sealed custody, where the platform holds no key. The bundle's \
                         bytes for such a field left the source space as stored, so they are \
                         the source client's ciphertext under a key nobody here has, and \
                         there is nothing an import could do with them but copy them into a \
                         column and call it a file. The presigned direct-to-bucket path that \
                         makes the tier work is F7 Phase 6's (`f7-vault-plan.md`), and until \
                         it exists a `private` object cannot leave one space and arrive in \
                         another"
                    .into(),
            });
        }
        if field.inline {
            return Err(ImportError::BlobTierUnsupported {
                entity: o.entity.clone(),
                field: o.field.clone(),
                tier: "inline",
                detail: "its bytes ride the row's sealed payload rather than the object \
                         store, so they are written by the write protocol inside the group \
                         that creates the row. The blob door refuses the tier for that \
                         reason and an import inherits the refusal: there is no upload to \
                         make, and the create that could carry the bytes cannot name a blob \
                         parameter (`Sema::check_content_params`)"
                    .into(),
            });
        }
        if field.sealed {
            return Err(ImportError::BlobTierUnsupported {
                entity: o.entity.clone(),
                field: o.field.clone(),
                tier: "sealed",
                detail: "declarable and not servable, on the way in or out: the collector \
                         proves a reference live by comparing the column against the handle \
                         and holds no keys, and a sealed handle carries a fresh nonce, so \
                         equal handles never produce equal envelopes. The door refuses both \
                         its write and its read for that reason"
                    .into(),
            });
        }
        // **An object must have a recorded digest.** Every other member has
        // a second description that catches a truncation — an entity file
        // has its row count, the manifest has its own bytes — and an object
        // has none: any byte string is a valid file. `member` checks the
        // digest when the manifest records one and silently accepts a member
        // it does not, which is right for a member whose absence is itself
        // the refusal and wrong here, so the presence is required rather
        // than hoped for.
        if !mf.digests.contains_key(&o.member) {
            return Err(ImportError::BundleIncomplete {
                member: o.member.clone(),
                detail: "the manifest records no `sha256` for it, and an object has no row \
                         count and no other description, so nothing would say these are its \
                         bytes"
                    .into(),
            });
        }
        let mime = o
            .mime
            .clone()
            .ok_or_else(|| ImportError::BundleIncomplete {
                member: o.member.clone(),
                detail: "the manifest does not name its mime. The mime is a sealed envelope on \
                     the source's `$blobref` row, so it is in the manifest or it is nowhere, \
                     and an import that defaulted it to `application/octet-stream` would \
                     store a value the customer never had. A `ciphertext` bundle omits it on \
                     purpose and is refused earlier"
                    .into(),
            })?;
        out.push(PlannedObject {
            member: o.member.clone(),
            entity: o.entity.clone(),
            field: o.field.clone(),
            source_id: o.source_id.clone(),
            mime,
        });
    }

    if let Some(stray) = carried.iter().next() {
        return Err(ImportError::BundleIncomplete {
            member: stray.clone(),
            detail: "the bundle holds it and the manifest's `blobs.files` does not describe \
                     it, so nothing says which field of which row it belongs to"
                .into(),
        });
    }
    Ok(out)
}

/// Bind every object to the row this import minted for it.
#[allow(clippy::too_many_arguments)]
async fn attach_objects(
    exec: &Executor,
    req: &ImportRequest<'_>,
    door: &dyn BlobDoor,
    src: &dyn BundleSource,
    mf: &Manifest,
    objects: &[PlannedObject],
    mapping: &HashMap<(String, String), String>,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    for o in objects {
        // **The bytes and their digest first, on every run.** `member`
        // checks the member against the `sha256` the manifest recorded, and
        // doing it before the skip below means a resumed run verifies the
        // bundle it is resuming over rather than trusting that the run which
        // read it first did. An import that did not check would be loading
        // bytes it was handed on the strength of a filename.
        let bytes = member(src, mf, &o.member)?.ok_or_else(|| ImportError::BundleIncomplete {
            member: o.member.clone(),
            detail: "the manifest describes it and reading it produced nothing".into(),
        })?;
        let row_id = mapping
            .get(&(o.entity.clone(), o.source_id.clone()))
            .ok_or_else(|| ImportError::BlobUnplaced {
                member: o.member.clone(),
                detail: format!(
                    "source row `{}` of `{}` is not in `$importmap` for this import, so no \
                     row in this space holds its file",
                    o.source_id, o.entity
                ),
            })?;
        if column_holds_object(&exec.pool, &exec.space, &o.entity, &o.field, row_id).await? {
            report.objects_skipped += 1;
            continue;
        }
        let correlation = format!(
            "cg_import_{}",
            crate::export::bundle_digest(format!("{}\n{}", req.name, o.member).as_bytes())
        );
        door.attach(ImportObject {
            space: &exec.space.0,
            entity: &o.entity,
            field: &o.field,
            row_id,
            mime: &o.mime,
            bytes,
            principal: req.caller,
            correlation: &correlation,
        })
        .await
        .map_err(|detail| ImportError::BlobRefused {
            member: o.member.clone(),
            detail,
        })?;
        report.objects_attached += 1;
    }
    Ok(())
}

/// Does this row's blob column already name an object?
///
/// **The resume marker for the blob leg, and it is the effect rather than a
/// record of the effect.**
///
/// `$importmap` is the mechanism for the rows and stays the mechanism here
/// in the sense that matters: it is what turns the member's source id into
/// the row this import minted, and without it an object has nowhere to go
/// (`blob_unplaced`). What it is **not** is the marker for whether the
/// attach happened, and the reason is atomicity. A mapping row rides the
/// batch's own transaction, so a batch that committed left its mapping
/// behind and a batch that did not left nothing; there is no window. An
/// attach commits in the **door's** transaction, which import does not open
/// and cannot join — the bytes must reach the store before it begins — so a
/// mapping row for an object could only be written after that commit, and
/// the gap between the two is a crash that leaves the object attached and
/// the record saying otherwise. The next run would then upload a second
/// object over the first: a second `rev`, a second group, and an
/// unreferenced object for the sweep.
///
/// The column has no such gap, because it *is* what the attach produced, in
/// the transaction that produced it. `crate::blob::owning_row` already rests
/// on the same sentence for the other direction — *"the column IS the
/// reference, and that is what makes the GC anti-join sound"* — so reading
/// it here is the existing authority answering the existing question.
///
/// What it does not distinguish is *which* object is attached, and that is
/// named rather than hidden: a skip is counted in
/// [`ImportReport::objects_skipped`], and re-importing a **changed** object
/// under a used import name is deferred work (`plan-import.md` §7). The
/// enclosing rule is what makes the reading safe in the first place:
/// `space_not_empty` has already refused unless every row of every imported
/// entity is one this import minted, so a non-null blob column on one of
/// them was written by this import's own attach.
async fn column_holds_object(
    pool: &Pool,
    space: &SpaceId,
    entity: &str,
    field: &str,
    row_id: &str,
) -> Result<bool, ImportError> {
    let rows = pool
        .rows(&format!(
            "SELECT 1 FROM {} WHERE {} = {} AND {} = {} AND {} IS NOT NULL",
            ordo_schema::ddl::q(entity),
            ordo_schema::ddl::q(ordo_schema::SPACE_COLUMN),
            crate::sql_lit(&space.0),
            ordo_schema::ddl::q("id"),
            crate::sql_lit(row_id),
            ordo_schema::ddl::q(field),
        ))
        .await?;
    Ok(!rows.is_empty())
}

// ── the mapping table ────────────────────────────────────────────────────

async fn ensure_importmap(pool: &Pool) -> Result<(), ImportError> {
    // The same benign-race retry `Pool::ensure_platform_tables` uses:
    // `CREATE TABLE IF NOT EXISTS` is not atomic against a concurrent
    // creator, and the loser's second look finds the table.
    if pool.rows(IMPORTMAP_DDL).await.is_err() {
        pool.rows(IMPORTMAP_DDL).await?;
    }
    Ok(())
}

/// The entity a people map writes into `$importmap`.
///
/// `members` and nothing else. `groupMembers` is the other platform entity
/// and mapping one would be mapping a *link row* rather than a person: a
/// membership in the destination is something the destination's owner grants,
/// and the whole point of the ruling is that a bundle cannot grant one.
pub const PEOPLE_ENTITY: &str = "members";

/// Pre-seed the people map into `$importmap`, so the ordinary ref rewrite
/// resolves a person exactly as it resolves anything else.
///
/// **No new resolution path**, which is the reason this is where it is.
/// `render_call`'s rewrite already refuses a ref it cannot resolve
/// (`unresolved_ref`), and the ruling's requirement is *a refusal for any
/// name the map does not cover*. Those are the same sentence, so the map is
/// applied by putting rows in front of a mechanism that already behaves
/// correctly, rather than by teaching the rewrite about people.
///
/// The four refusals below are ordered so that the cheapest and most local
/// comes first: a map that contradicts itself is wrong whatever the
/// destination holds.
async fn seed_people(
    pool: &Pool,
    space: &SpaceId,
    import: &str,
    people: &[(String, String)],
    mapping: &mut HashMap<(String, String), String>,
) -> Result<u64, ImportError> {
    if people.is_empty() {
        return Ok(0);
    }
    // 1. The map against itself.
    let mut seen_source: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen_dest: BTreeMap<&str, &str> = BTreeMap::new();
    for (source, destination) in people {
        if let Some(prior) = seen_source.insert(source, destination) {
            if prior != destination {
                return Err(ImportError::PeopleAmbiguous {
                    detail: format!(
                        "source person `{source}` is mapped to both `{prior}` and \
                         `{destination}`"
                    ),
                });
            }
        }
        if let Some(prior) = seen_dest.insert(destination, source) {
            if prior != source {
                return Err(ImportError::PeopleAmbiguous {
                    detail: format!(
                        "source people `{prior}` and `{source}` are both mapped to \
                         `{destination}`"
                    ),
                });
            }
        }
    }

    // 2. Every destination is a member of the destination space. One query
    // rather than one per person: the answer is a set, and a person-at-a-time
    // loop over a remote database is the kind of cost that only shows up in
    // the migration that is already too slow.
    let ids: Vec<String> = seen_dest.keys().map(|d| crate::sql_lit(d)).collect();
    let present: BTreeSet<String> = pool
        .rows(&format!(
            "SELECT \"id\" FROM \"members\" WHERE \"$space\" = {} AND \"id\" IN ({})",
            crate::sql_lit(&space.0),
            ids.join(", "),
        ))
        .await?
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    for (destination, source) in &seen_dest {
        if !present.contains(*destination) {
            return Err(ImportError::PeopleUnknown {
                source: (*source).to_string(),
                destination: (*destination).to_string(),
            });
        }
    }

    // 3. Against what this import already stored, which is what the rows
    // already in the destination were attributed under.
    for (source, destination) in &seen_source {
        if let Some(stored) = mapping.get(&(PEOPLE_ENTITY.to_string(), (*source).to_string())) {
            if stored != destination {
                return Err(ImportError::PeopleChanged {
                    source: (*source).to_string(),
                    stored: stored.clone(),
                    supplied: (*destination).to_string(),
                });
            }
        }
    }

    // 4. Store and apply. `ON CONFLICT DO NOTHING` because step 3 has
    // already refused every disagreement, so the only conflict left is a
    // resumed import re-supplying the identical map.
    let mut seeded = 0;
    for (source, destination) in &seen_source {
        pool.rows(&format!(
            "INSERT INTO \"$importmap\" \
             (\"$space\", \"import\", \"entity\", \"source_id\", \"row_id\") \
             VALUES ({}, {}, {}, {}, {}) ON CONFLICT DO NOTHING",
            crate::sql_lit(&space.0),
            crate::sql_lit(import),
            crate::sql_lit(PEOPLE_ENTITY),
            crate::sql_lit(source),
            crate::sql_lit(destination),
        ))
        .await?;
        if mapping
            .insert(
                (PEOPLE_ENTITY.to_string(), (*source).to_string()),
                (*destination).to_string(),
            )
            .is_none()
        {
            seeded += 1;
        }
    }
    Ok(seeded)
}

async fn load_mapping(
    pool: &Pool,
    space: &SpaceId,
    import: &str,
) -> Result<HashMap<(String, String), String>, ImportError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"entity\", \"source_id\", \"row_id\" FROM \"$importmap\" \
             WHERE \"$space\" = {} AND \"import\" = {}",
            crate::sql_lit(&space.0),
            crate::sql_lit(import),
        ))
        .await?;
    let mut out = HashMap::with_capacity(rows.len());
    for r in rows {
        let cell = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        out.insert((cell(0), cell(1)), cell(2));
    }
    Ok(out)
}

/// Rows of `entity` in this space that **this import** minted.
async fn count_mapped(
    pool: &Pool,
    space: &SpaceId,
    import: &str,
    entity: &str,
) -> Result<i64, ImportError> {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM \"$importmap\" WHERE \"$space\" = {} \
             AND \"import\" = {} AND \"entity\" = {}",
            crate::sql_lit(&space.0),
            crate::sql_lit(import),
            crate::sql_lit(entity),
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0))
}

async fn count_rows(pool: &Pool, space: &SpaceId, entity: &str) -> Result<i64, ImportError> {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM {} WHERE \"$space\" = {}",
            ordo_schema::ddl::q(entity),
            crate::sql_lit(&space.0),
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0))
}
