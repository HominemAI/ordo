//! **Full space export** — the door out, as an op.
//!
//! `licensing.md` prints a three-leg trust argument, and this is leg 1:
//! *"Full space export yields plain DDL, per-entity JSONL and a
//! chain-verifiable command-log archive — a documented op, not a support
//! ticket."* `migration.md` §6 says what the bundle holds and
//! `definition-language.md` §8 says who may ask for it. Neither had any
//! code under it before this module.
//!
//! # The one architectural requirement
//!
//! **Nothing here knows the name of a single entity, field or column.**
//! Everything the bundle contains is derived at run time from two things
//! the deployment already has: the compiled [`Schema`] and the stored
//! artifact. A new entity, a new field, a new constraint, a new
//! declaration — each flows into the export with no change to this file.
//!
//! That is not a style preference; it is the property that makes the exit
//! real. An export with a hand-maintained column list is an export that is
//! *silently wrong* one deploy after somebody adds a field, and the bundle
//! still parses, so nobody finds out until they need it. Concretely:
//!
//! - `schema.sql` is the artifact's own stored DDL, verbatim. Not
//!   re-rendered here, not assembled — the bytes the deploy applied.
//! - `artifact.json` is the stored artifact document, verbatim. Identity
//!   travels with the data.
//! - Each `data/<entity>.jsonl` selects
//!   [`ordo_schema::ddl::physical_columns`] — the same list `ddl_for`
//!   prints — never `SELECT *`. The column set comes from the schema, and
//!   the guarantee that the schema is what the database holds is
//!   [`Pool::live_drift`], asked before the first row is read (below).
//! - Value encoding is an exhaustive match on `ScalarTy`. A new scalar
//!   type does not silently fall through to a string: it **fails to
//!   compile** here until somebody decides what its JSON looks like. New
//!   *fields* need no decision; a new *type in the language* is a decision,
//!   and this is where it gets made rather than defaulted.
//!
//! # Why a drifted database refuses
//!
//! The bundle ships `schema.sql` and the rows side by side, and a reader
//! joins them. If the live table has drifted from the artifact — a
//! migration half-applied, a restore onto an older shape, a column added
//! by hand — then either the export's `SELECT` fails (a column the
//! artifact declares and the table lacks) or, worse, it succeeds and the
//! bundle describes itself wrongly. So the export asks
//! [`Pool::live_drift`] first, against a shadow built from the artifact's
//! own DDL, and refuses on any answer but "none". The same comparison the
//! load gate makes, asked by the one other reader that has to trust it.
//!
//! # Determinism
//!
//! A bundle is a **pure function of the space's state and the artifact**.
//! Rows are ordered by `id` under the `C` collation (byte order, so the
//! database's locale cannot reorder them); JSON object keys are sorted by
//! `serde_json::Map`'s `BTreeMap`; entities are visited in name order; and
//! **there is no timestamp anywhere in the bundle**. Two exports of one
//! unchanged space are byte-identical, which is what makes a bundle
//! diffable, cacheable, and checkable by whoever receives it. Provenance —
//! who asked, and when — belongs to the command the op appends to the
//! space's chain, which is the artifact that is *supposed* to carry a
//! clock.
//!
//! # Sealed fields
//!
//! `migration.md` §6 already ruled: data is *"decrypted under the owner's
//! authority through the normal key-release path"*. [`SealedMode::Open`]
//! does exactly that — an ordinary [`KeyProvider::release`] for
//! [`Purpose::Read`], then [`KeyProvider::open_at`] per value — and it is why
//! the op is step-up gated: export is total disclosure, so the disclosure
//! must be as authenticated as the door can make it.
//!
//! [`SealedMode::Ciphertext`] is the other half of the same sentence.
//! §6 continues: *"client-first spaces export from the device, where
//! plaintext lives."* A deployment holding no key for a space cannot open
//! its envelopes, and pretending otherwise would produce a bundle full of
//! nulls that reads as *this field was empty*. Ciphertext mode hands over
//! the envelopes as hex and says so in the manifest, which is an honest
//! partial answer rather than a dishonest complete one.
//!
//! In `Open` mode a value that will not open is a **refusal**, not a null.
//! A space with a destroyed epoch has data the platform holds and cannot
//! hand over; the bundle must not quietly under-deliver on leg 1 of the
//! trust argument. Re-run in ciphertext mode and the state is disclosed for
//! what it is.
//!
//! **Blind-index columns are omitted**, and the manifest records each one.
//! A `$bidx` value is `HMAC(k_field, plaintext)` under the *source* space's
//! key: it is not data, it carries nothing the plaintext does not, it
//! cannot be reproduced under a destination space's key, and shipping it
//! would export a working equality oracle over the plaintext beside it.
//! Re-deriving digests belongs to import, under the destination's key.
//!
//! # Blob bytes, and the layout that is not the bucket's
//!
//! `migration.md` §6 lists *"Files — blob bytes, laid out by
//! entity/field/id"*, and the bundle now carries them at
//! `blobs/<entity>/<field>/<id>`, one file per referenced object. The
//! `unbuilt` list — which named exactly this gap for the whole life of
//! this module — is empty.
//!
//! **The bucket does not use that layout and must not.** An object key
//! carrying an entity and a field would enumerate the schema to anyone who
//! can list the bucket, which is why `ordo_crypto::blobref` pins the key at
//! `<space>/<hex handle>` and nothing else. Two layouts, and the difference
//! between them is the difference between a store only the platform reads
//! and a bundle whose entire purpose is to be read by somebody else. The
//! export can produce the second because it holds the `$blobref` rows,
//! which carry the entity and the field.
//!
//! In `Open` mode the objects are decrypted through the same key release
//! the rows use; in `Ciphertext` mode they leave as stored — the chunked
//! STREAM envelope, unopened — which is the same rule the sealed columns
//! beside them follow. A reference no live row names is skipped: its bytes
//! stopped being this space's data when the row went, and it is already on
//! the sweep's list.
//!
//! # Member digests
//!
//! Every member is digested as it is written and the manifest names it —
//! `plan-production-ops.md` R1's finding, which was that the manifest had
//! none and *"a restore marker cannot attest a bundle it cannot name"*. The
//! manifest cannot digest itself and does not claim to: it attests the
//! bundle, and something outside attests the manifest.

use std::collections::BTreeMap;

use ordo_crypto::{KeyProvider, Purpose, SpaceId};
use ordo_schema::ddl::{ColumnRole, PhysicalColumn};
use ordo_schema::{FieldTy, Schema};
use ordo_syntax::ScalarTy;
use sha2::{Digest, Sha256};

use crate::registry::Artifact;
use crate::shape::{Drift, EXPORT_SHADOW};
use crate::{sql_lit, Pool, PoolError};

/// The bundle's format tag. A reader that does not recognise it should
/// stop rather than guess, so it is the first key of the manifest.
pub const FORMAT: &str = "ordo.space-export.v1";

/// Rows read per round trip. Keyset paging on `id`, never `OFFSET`: an
/// offset scan re-reads the prefix per page and, worse, silently skips rows
/// if anything is inserted between pages.
const PAGE: usize = 500;

// ── the sink ─────────────────────────────────────────────────────────────

/// Where a bundle's members are written.
///
/// A trait rather than a return value because a bundle is potentially
/// larger than memory and its members are produced in order. Members are
/// opened, written in chunks, and closed; none is ever reopened.
pub trait BundleSink: Send {
    fn open(&mut self, name: &str) -> Result<(), ExportError>;
    fn write(&mut self, chunk: &str) -> Result<(), ExportError>;
    /// A **binary** chunk. Blob bytes go through here.
    ///
    /// Not base64 inside `write`, and the reason is the same one that
    /// keeps bytes out of the per-entity JSONL: `plan-export.md` §2's
    /// property is that a bundle stays loadable by ordinary tooling, and
    /// a PDF that has to be un-base64ed before it is a PDF is not. It
    /// also inflates by a third, which for the one member class that is
    /// measured in megabytes is not a rounding error.
    ///
    /// Required rather than defaulted: a default would have to encode,
    /// and a sink that silently encoded would produce a bundle whose
    /// files are not the files.
    fn write_bytes(&mut self, chunk: &[u8]) -> Result<(), ExportError>;
    fn close(&mut self) -> Result<(), ExportError>;
}

/// A bundle in memory, for tests and small spaces.
#[derive(Debug, Default)]
pub struct MemoryBundle {
    /// Members as raw bytes. Text members are UTF-8 and read back through
    /// [`MemoryBundle::get`]; blob members are whatever was uploaded, and
    /// storing them as `String` would have made the in-memory sink refuse
    /// exactly the bundle this exists to test.
    pub members: BTreeMap<String, Vec<u8>>,
    open: Option<String>,
}

impl MemoryBundle {
    pub fn new() -> MemoryBundle {
        MemoryBundle::default()
    }
    /// A text member. `None` for an absent one **and** for a binary one —
    /// a blob is not text and must not be lossily rendered as some.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.members
            .get(name)
            .and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Any member, verbatim.
    pub fn get_bytes(&self, name: &str) -> Option<&[u8]> {
        self.members.get(name).map(Vec::as_slice)
    }
    /// Every member's name, in bundle order.
    pub fn names(&self) -> Vec<&str> {
        self.members.keys().map(String::as_str).collect()
    }
}

impl BundleSink for MemoryBundle {
    fn open(&mut self, name: &str) -> Result<(), ExportError> {
        if self.members.contains_key(name) {
            return Err(ExportError::Bundle(format!(
                "`{name}` was written twice — a bundle member is written once"
            )));
        }
        self.members.insert(name.to_string(), Vec::new());
        self.open = Some(name.to_string());
        Ok(())
    }
    fn write(&mut self, chunk: &str) -> Result<(), ExportError> {
        self.write_bytes(chunk.as_bytes())
    }
    fn write_bytes(&mut self, chunk: &[u8]) -> Result<(), ExportError> {
        let name = self
            .open
            .as_ref()
            .ok_or_else(|| ExportError::Bundle("write with no member open".into()))?;
        self.members
            .get_mut(name)
            .expect("the open member exists")
            .extend_from_slice(chunk);
        Ok(())
    }
    fn close(&mut self) -> Result<(), ExportError> {
        self.open = None;
        Ok(())
    }
}

/// A bundle as files under a directory.
pub struct DirectoryBundle {
    root: std::path::PathBuf,
    open: Option<std::io::BufWriter<std::fs::File>>,
}

impl DirectoryBundle {
    /// The directory must already exist and should be empty; the export
    /// creates sub-directories but never removes anything.
    pub fn new(root: impl Into<std::path::PathBuf>) -> DirectoryBundle {
        DirectoryBundle {
            root: root.into(),
            open: None,
        }
    }
}

impl BundleSink for DirectoryBundle {
    fn open(&mut self, name: &str) -> Result<(), ExportError> {
        let path = self.root.join(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| ExportError::Bundle(e.to_string()))?;
        }
        // `create_new`: a bundle member is written once, and refusing to
        // overwrite is what stops a second export into a used directory
        // from producing a half-old, half-new bundle.
        let file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| ExportError::Bundle(format!("{}: {e}", path.display())))?;
        self.open = Some(std::io::BufWriter::new(file));
        Ok(())
    }
    fn write(&mut self, chunk: &str) -> Result<(), ExportError> {
        self.write_bytes(chunk.as_bytes())
    }
    fn write_bytes(&mut self, chunk: &[u8]) -> Result<(), ExportError> {
        use std::io::Write;
        let w = self
            .open
            .as_mut()
            .ok_or_else(|| ExportError::Bundle("write with no member open".into()))?;
        w.write_all(chunk)
            .map_err(|e| ExportError::Bundle(e.to_string()))
    }
    fn close(&mut self) -> Result<(), ExportError> {
        use std::io::Write;
        if let Some(mut w) = self.open.take() {
            w.flush().map_err(|e| ExportError::Bundle(e.to_string()))?;
        }
        Ok(())
    }
}

// ── refusals ─────────────────────────────────────────────────────────────

/// Why an export did not happen, or did not finish.
///
/// Every variant names what the operator does next. A refusal on the exit
/// path that reads as "no" is a refusal somebody routes around, and the
/// route around a broken export is a support ticket — which is the thing
/// `licensing.md` promises this op is not.
#[derive(Debug)]
#[non_exhaustive]
pub enum ExportError {
    /// The live database is not the shape the artifact asserts. Exporting
    /// anyway would ship a `schema.sql` that does not describe the rows
    /// beside it.
    Drift {
        version: String,
        drift: Vec<Drift>,
    },
    /// A sealed value did not open under the space's keys, in a mode that
    /// promised plaintext.
    SealedUnopened {
        entity: String,
        field: String,
        row_id: String,
        detail: String,
    },
    /// The key provider would not release a read lease for the space.
    KeyRelease(String),
    /// The bundle's sink failed. Disk, permissions, a member written twice.
    Bundle(String),
    Store(PoolError),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Drift { version, drift } => write!(
                f,
                "refusing to export against a database that is not the shape version \
                 `{version}` asserts — the bundle's `schema.sql` would not describe the rows \
                 shipped beside it:\n{}",
                drift
                    .iter()
                    .map(|d| format!("  {d}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            ExportError::SealedUnopened {
                entity,
                field,
                row_id,
                detail,
            } => write!(
                f,
                "`{entity}`.`{field}` of row `{row_id}` did not open, so an export promising \
                 plaintext would hand over a null where a value is: {detail}. If this space \
                 has had an epoch destroyed, export it in ciphertext mode — that discloses \
                 the state for what it is instead of under-reporting it."
            ),
            ExportError::KeyRelease(e) => write!(
                f,
                "custody would not release a read lease for this space, so no sealed field \
                 can be opened: {e}"
            ),
            ExportError::Bundle(e) => write!(f, "the export bundle could not be written: {e}"),
            ExportError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<PoolError> for ExportError {
    fn from(e: PoolError) -> ExportError {
        ExportError::Store(e)
    }
}

// ── what the caller chooses ──────────────────────────────────────────────

/// What the bundle does with sealed fields. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedMode {
    /// Open every envelope under the space's keys and write plaintext.
    /// The disclosure `definition-language.md` §8 gates behind step-up.
    Open,
    /// Write the envelopes as lowercase hex and hand the reader the
    /// metadata to re-import them into a space with the same keys. What a
    /// deployment that holds no key for the space can honestly produce.
    Ciphertext,
}

impl SealedMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SealedMode::Open => "opened",
            SealedMode::Ciphertext => "ciphertext",
        }
    }
}

/// Everything one export needs that is not the pool.
pub struct ExportRequest<'a> {
    pub space: &'a SpaceId,
    /// The compiled program the artifact carries. Every column, every
    /// entity, every encoding decision comes from here.
    pub schema: &'a Schema,
    /// The deployed artifact — `schema.sql` and `artifact.json` are its
    /// stored bytes, and its version is what the drift check names.
    pub artifact: &'a Artifact,
    pub keys: &'a dyn KeyProvider,
    /// The blob pipeline, when the deployment serves blobs.
    ///
    /// `None` is not "skip the blobs" — it is *this deployment has no blob
    /// tier wired*, and the manifest says which of the two it was. A
    /// bundle that silently omitted files because a resolver was not
    /// passed would be exactly the "silently wrong export" the module doc
    /// exists to prevent.
    pub blobs: Option<&'a ordo_blob::BlobPipeline>,
    pub sealed: SealedMode,
    /// The correlation the key release is attributed under. The op passes
    /// its own command correlation, so a custody audit and the space's
    /// chain name the same act.
    pub correlation: &'a str,
}

/// What an export produced. Enough for the op's response and for a test to
/// assert on without re-parsing the bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    pub space: String,
    pub version: String,
    pub sealed: &'static str,
    /// Per entity, in bundle order: name and row count.
    pub entities: Vec<(String, u64)>,
    /// Command-log groups written to the archive.
    pub log_groups: u64,
    /// Bundle member names, in the order they were written.
    pub members: Vec<String>,
    /// `sha256(manifest.json)`, lowercase hex, no prefix — the bundle's own
    /// digest, as `plan-production-ops.md` R1a names it. The manifest
    /// already names every other member's digest, so this single string
    /// fixes the whole bundle: it is what a restore marker's
    /// `RestoreSource::Bundle` names.
    pub manifest_sha256: String,
}

impl ExportReport {
    pub fn rows(&self) -> u64 {
        self.entities.iter().map(|(_, n)| n).sum()
    }
}

// ── column encoding, derived ─────────────────────────────────────────────

/// How one column's text from Postgres becomes JSON.
///
/// Derived from the column's role and its field's declared type, never
/// from its name and never from the type Postgres reports. The mapping is
/// the same one [`ordo_schema::ddl::column_ty`] used to choose the physical
/// type, read in the other direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    /// Emitted as a JSON string.
    Text,
    /// Emitted as a JSON integer. `money` lands here as **minor units**,
    /// which is what `schema.sql` says the column is (`bigint`), and the
    /// bundle must describe itself.
    Integer,
    Bool,
    /// `numeric`. A JSON number when it round-trips through `f64`'s
    /// grammar, the digit string otherwise — precision is never silently
    /// lost to make the type prettier.
    Decimal,
    /// `jsonb`: the document itself, as JSON in the bundle (28b), never a
    /// string holding JSON.
    Json,
    /// A sealed envelope: opened, or handed over as hex.
    Sealed,
    /// A blob **reference**, in whichever spelling its tier stores: the hex
    /// handle for an external blob (a `text` column), the raw payload
    /// handle for an inline one (a `bytea` column, so hex again over the
    /// wire).
    ///
    /// The row carries the reference, and the **bytes** ride the bundle
    /// separately at `blobs/<entity>/<field>/<id>`, which is the layout
    /// `migration.md` §6 describes. That split is not an implementation
    /// convenience: `plan-export.md` §2's property is that the bundle stays
    /// loadable by ordinary tooling, and megabytes of hex per row in a
    /// JSONL file is the thing that ends it.
    BlobRef {
        inline: bool,
    },
}

/// One column as the export will read it.
struct ExportColumn<'a> {
    name: String,
    encoding: Encoding,
    /// The field it belongs to, for the AAD an `open` needs.
    field: Option<&'a str>,
    /// The field is `json`, so an opened sealed value is a document.
    json: bool,
}

/// The columns an entity's rows are made of, and the columns deliberately
/// left out.
///
/// **The only place this module decides anything about a column**, and it
/// decides by exhaustive match. Adding a `ScalarTy` breaks the build here.
fn columns_of<'a>(
    cols: &[PhysicalColumn<'a>],
) -> (Vec<ExportColumn<'a>>, Vec<(String, &'static str)>) {
    let mut take = Vec::with_capacity(cols.len());
    let mut omit = Vec::new();
    for c in cols {
        match c.role {
            // Kept. The bundle is loadable against its own `schema.sql`
            // with nothing supplied from outside, and the space is a fact
            // about the rows rather than a fact the reader must be told.
            ColumnRole::Space | ColumnRole::Path => take.push(ExportColumn {
                name: c.name.clone(),
                encoding: Encoding::Text,
                field: None,
                json: false,
            }),
            ColumnRole::BlindIndex(_) => {
                omit.push((c.name.clone(), "blind_index_derived"));
            }
            ColumnRole::Field(f) => take.push(ExportColumn {
                name: c.name.clone(),
                encoding: if f.sealed {
                    Encoding::Sealed
                } else {
                    match f.ty {
                        // A ref's value is the target's id: text.
                        FieldTy::Ref { .. } => Encoding::Text,
                        FieldTy::Scalar(s) => match s {
                            ScalarTy::Text | ScalarTy::Id | ScalarTy::Date | ScalarTy::Datetime => {
                                Encoding::Text
                            }
                            ScalarTy::Int | ScalarTy::Money(_) | ScalarTy::Duration(_) => {
                                Encoding::Integer
                            }
                            ScalarTy::Bool => Encoding::Bool,
                            ScalarTy::Decimal(_) => Encoding::Decimal,
                            // The row carries a reference; the bytes ride
                            // the bundle at `blobs/<entity>/<field>/<id>`.
                            ScalarTy::Blob => Encoding::BlobRef { inline: f.inline },
                            // An address is a string, and it is the whole
                            // of what `uri` exports: DL §2 says "no export
                            // of bytes (consent page states 'referenced,
                            // not held')", and this arm is where that
                            // sentence is true rather than promised.
                            ScalarTy::Uri => Encoding::Text,
                            ScalarTy::Json => Encoding::Json,
                            // No wildcard arm, deliberately. A scalar type
                            // added to the language must have its JSON
                            // shape decided here, and a `_ => Text` would
                            // decide it by accident and silently.
                        },
                    }
                },
                field: Some(&f.name),
                json: matches!(f.ty, FieldTy::Scalar(ScalarTy::Json)),
            }),
        }
    }
    (take, omit)
}

/// The SQL expression that reads one column as text.
fn select_expr(c: &ExportColumn<'_>) -> String {
    let q = ordo_schema::ddl::q(&c.name);
    match c.encoding {
        // `bytea` over the text protocol arrives as `\x…`; asking Postgres
        // for the hex directly skips a parse and a prefix.
        Encoding::Sealed => format!("encode({q}, 'hex')"),
        // An inline blob's column is `bytea` holding the raw payload
        // handle, so it needs the same hex treatment; an external one is
        // already the hex string.
        Encoding::BlobRef { inline: true } => format!("encode({q}, 'hex')"),
        _ => q,
    }
}

/// The bundle's own digest: `SHA-256` over `manifest.json`'s exact bytes,
/// lowercase hex, no prefix.
///
/// The manifest already names every other member's digest (see
/// [`DigestingSink`]), so the digest of the manifest fixes the whole
/// bundle to one string — the one `plan-production-ops.md` R1a's callers
/// report and the one a restore marker's `RestoreSource::Bundle` names.
pub fn bundle_digest(manifest: &[u8]) -> String {
    ordo_crypto::hex(&Sha256::digest(manifest))
}

/// A sink that digests every member as it passes.
///
/// # Why the manifest needs this at all
///
/// `plan-production-ops.md` R1 records the finding: the bundle manifest had
/// **no member digests**, *"and a restore marker cannot attest a bundle it
/// cannot name"*. A restore is a discontinuity in the chain, and the
/// trailer that records one has to say which bytes were restored — a
/// bundle identified only by a directory name is a bundle any later edit
/// silently redefines.
///
/// It is a wrapper rather than a field on each sink because the digest is
/// a property of the *export*, not of where the bytes went: the in-memory
/// bundle and the directory bundle must produce the same digests for the
/// same space, and two implementations would eventually not.
///
/// The manifest is written **through** this too but names only the members
/// written before it. That is not an omission — a manifest cannot contain
/// its own digest, and the honest structure is "the manifest attests the
/// bundle; something outside attests the manifest", which is exactly what
/// a restore marker is for.
struct DigestingSink<'a> {
    inner: &'a mut dyn BundleSink,
    open: Option<(String, Sha256, u64)>,
    /// Member name -> (byte length, sha256 hex).
    pub digests: BTreeMap<String, (u64, String)>,
}

impl<'a> DigestingSink<'a> {
    fn new(inner: &'a mut dyn BundleSink) -> Self {
        DigestingSink {
            inner,
            open: None,
            digests: BTreeMap::new(),
        }
    }
}

impl BundleSink for DigestingSink<'_> {
    fn open(&mut self, name: &str) -> Result<(), ExportError> {
        self.inner.open(name)?;
        self.open = Some((name.to_string(), Sha256::new(), 0));
        Ok(())
    }
    fn write(&mut self, chunk: &str) -> Result<(), ExportError> {
        self.write_bytes(chunk.as_bytes())
    }
    fn write_bytes(&mut self, chunk: &[u8]) -> Result<(), ExportError> {
        self.inner.write_bytes(chunk)?;
        if let Some((_, d, n)) = self.open.as_mut() {
            d.update(chunk);
            *n += chunk.len() as u64;
        }
        Ok(())
    }
    fn close(&mut self) -> Result<(), ExportError> {
        if let Some((name, d, n)) = self.open.take() {
            self.digests
                .insert(name, (n, ordo_crypto::hex(&d.finalize())));
        }
        self.inner.close()
    }
}

/// One object the bundle carries, as the manifest describes it.
///
/// The member **name** already spells `entity/field/id`, and this struct is
/// not a second copy of it for a reason: a reader that recovered the three
/// by splitting a path would be parsing a string the exporter formatted,
/// and the first id holding a `/` would place an object on the wrong row
/// rather than refuse. The manifest states them instead, so
/// `ordo_exec::import` never parses a member name.
///
/// `mime` is the one fact about an object that is **nowhere else in the
/// bundle**. It is a sealed envelope on the `$blobref` row, under the
/// source space's key, so an importer cannot open it and cannot re-derive
/// it from the bytes; without it an import would have to guess
/// `application/octet-stream` for every file, which is a value the customer
/// never stored.
pub struct ExportedObject {
    pub member: String,
    pub entity: String,
    pub field: String,
    pub row_id: String,
    /// Present exactly when the member holds **plaintext**, which is
    /// `SealedMode::Open` over a field that is not `blob private`.
    ///
    /// Two reasons it is conditional rather than always written. A
    /// ciphertext bundle is one the platform did not decrypt, and its
    /// object's mime is a sealed value like the columns beside it, so
    /// writing the mime in plaintext would disclose through the manifest
    /// exactly what the mode refused to disclose through the member. And a
    /// mime naming bytes that are not the file's bytes is a false
    /// description: a `private` object leaves as stored even in `Open`
    /// mode, so what the member holds is the client's ciphertext.
    pub mime: Option<String>,
}

/// Write `blobs/<entity>/<field>/<id>` for every blob a space holds.
///
/// The layout is `migration.md` §6's — *"blob bytes, laid out by
/// entity/field/id"* — and the export can produce it because it holds the
/// `$blobref` rows, which carry the entity and the field. The **bucket**
/// does not lay objects out this way and must not: an object key carrying
/// an entity and a field would enumerate the schema to anyone who can list
/// the bucket. Two layouts, one for a store nobody but the platform reads
/// and one for a bundle whose whole purpose is to be read by somebody
/// else.
///
/// # Ciphertext mode carries the object as it is stored
///
/// `SealedMode::Ciphertext` exports the object's stored bytes — the STREAM
/// envelope, unopened — for the same reason it exports sealed columns as
/// hex: the caller asked for a bundle the platform did not decrypt. Open
/// mode decrypts through the pipeline, under the one lease the export
/// already holds.
///
/// # A missing object is a refusal, not an omission
///
/// If a reference names an object the store does not have, the export
/// stops. Silently writing a bundle with a hole in it would produce an
/// artifact that looks complete and restores a row pointing at nothing —
/// which is the same failure the GC floor exists to prevent, arriving by
/// a different route.
async fn write_blobs(
    pool: &Pool,
    req: &ExportRequest<'_>,
    lease: Option<&ordo_crypto::KeyLease>,
    sink: &mut dyn BundleSink,
) -> Result<Vec<ExportedObject>, ExportError> {
    let Some(pipe) = req.blobs else {
        return Ok(Vec::new());
    };
    let mut written = Vec::new();
    let refs = pool
        .rows(&ordo_crypto::blobref::list_sql(req.space))
        .await
        .map_err(ExportError::Store)?;
    if refs.is_empty() {
        // Nothing to read, so nothing to resolve. A space with no
        // references must not need its store to be reachable to be
        // exported: under an attachment that would make an unreachable
        // bucket refuse a bundle that was never going to read a byte from
        // it.
        return Ok(written);
    }
    // **The space's own store, resolved once for the whole bundle.**
    // `plan-tenant-storage.md` §8 B-E1: the export was the second place
    // that reached through the pipeline to one store, and a bundle
    // assembled from two different stores is a bundle nobody can say which
    // deployment it is a copy of. Resolved once rather than per reference
    // so that every object in one bundle comes from one place even if an
    // attachment changed while the export ran.
    //
    // Only the as-stored reads below use it. `SealedMode::Open` goes
    // through `pipe.get`, which resolves the same space through the same
    // resolver, so the open path is resolver-aware without naming a store
    // at all.
    for row in &refs {
        let cell = |i: usize| row.get(i).and_then(|c| c.as_deref());
        let (Some(hex), Some(entity), Some(field)) = (cell(0), cell(2), cell(3)) else {
            continue;
        };
        // Each object is read from the store its row names (ruled
        // 2026-10-05, `detach-upload-race` ask 2). An object written while
        // the tenant's bucket was being detached or replaced is in the old
        // bucket, and an export that asked the space's current route would
        // miss it or, worse, find another object under the same key.
        let pipe = pipe.in_store(cell(ordo_crypto::blobref::LIST_STORE_COLUMN).unwrap_or_default());
        let store = pipe
            .store_for(req.space)
            .await
            .map_err(|e| ExportError::Bundle(format!("the space's object store: {e}")))?;
        let Some(handle) = parse_blob_handle(hex) else {
            continue;
        };
        let declared = req.schema.entity(entity).and_then(|e| e.field(field));
        let inline = declared.map(|f| f.inline).unwrap_or(false);
        let private = declared.map(|f| f.private).unwrap_or(false);
        // Which row holds it. The column IS the reference — that is what
        // makes the GC anti-join sound — so it is also the authority on
        // the `id` this member is filed under.
        let Some(id) = crate::blob::owning_row(pool, req.space, entity, field, inline, &handle)
            .await
            .map_err(ExportError::Store)?
        else {
            // A reference no live row names is on the sweep's list, and
            // its bytes stopped being this space's data when the row went.
            // Exporting it would put a deleted document into a bundle
            // somebody asked for as a copy of their space.
            continue;
        };

        let bytes = match req.sealed {
            // The object exactly as it is stored: the STREAM envelope,
            // unopened. Same reason sealed columns leave as hex — the
            // caller asked for a bundle the platform did not decrypt.
            SealedMode::Ciphertext => {
                let stream = store
                    .get(req.space, &handle, None)
                    .await
                    .map_err(|e| ExportError::Bundle(format!("blob {hex}: {e}")))?;
                ordo_blob::collect(stream)
                    .await
                    .map_err(|e| ExportError::Bundle(format!("blob {hex}: {e}")))?
            }
            // A `blob private` object is client-sealed and the platform
            // holds no key, so an "open" export of one is not a thing that
            // exists. The bytes leave **as stored**, exactly as ciphertext
            // mode does — which is the honest partial answer the module
            // doc already argues for a space whose envelopes the
            // deployment cannot open, rather than the dishonest complete
            // one of a file full of nulls.
            SealedMode::Open if private => {
                let stream = store
                    .get(req.space, &handle, None)
                    .await
                    .map_err(|e| ExportError::Bundle(format!("blob {hex}: {e}")))?;
                ordo_blob::collect(stream)
                    .await
                    .map_err(|e| ExportError::Bundle(format!("blob {hex}: {e}")))?
            }
            SealedMode::Open => {
                let lease = lease.ok_or_else(|| {
                    ExportError::Bundle("an open export needs a key release".into())
                })?;
                // The plaintext length lives in the sealed `size` column,
                // which is sealed for the reason a value is: file sizes
                // fingerprint documents.
                let sealed_size = cell(5)
                    .and_then(ordo_crypto::sealed::unhex)
                    .ok_or_else(|| ExportError::Bundle(format!("blob {hex}: no size")))?;
                let object = ordo_crypto::hex(&handle);
                let plain_len: u64 = req
                    .keys
                    .open_at(
                        lease,
                        entity,
                        field,
                        ordo_crypto::Anchor::Object(&object),
                        &sealed_size,
                    )
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| ExportError::SealedUnopened {
                        entity: entity.to_string(),
                        field: field.to_string(),
                        row_id: id.clone(),
                        detail: "the blob's sealed size did not open".into(),
                    })?;
                if plain_len == 0 {
                    Vec::new()
                } else {
                    pipe.get(
                        req.space,
                        entity,
                        field,
                        &handle,
                        plain_len,
                        0,
                        None,
                        req.correlation,
                    )
                    .await
                    .map_err(|e| ExportError::SealedUnopened {
                        entity: entity.to_string(),
                        field: field.to_string(),
                        row_id: id.clone(),
                        detail: e.to_string(),
                    })?
                }
            }
        };

        // The mime, opened only where the member holds plaintext — see
        // [`ExportedObject::mime`] for both halves of that condition. An
        // envelope that will not open in `Open` mode is a refusal rather
        // than an absent mime, which is §3's rule for every other sealed
        // value in this module: *"an unopenable value in `Open` mode is a
        // refusal, not a null."*
        let mime = match (req.sealed, private, lease) {
            (SealedMode::Open, false, Some(lease)) => {
                let sealed_mime = cell(4)
                    .and_then(ordo_crypto::sealed::unhex)
                    .ok_or_else(|| ExportError::Bundle(format!("blob {hex}: no mime")))?;
                let object = ordo_crypto::hex(&handle);
                Some(
                    req.keys
                        .open_at(
                            lease,
                            entity,
                            field,
                            ordo_crypto::Anchor::Object(&object),
                            &sealed_mime,
                        )
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                        .ok_or_else(|| ExportError::SealedUnopened {
                            entity: entity.to_string(),
                            field: field.to_string(),
                            row_id: id.clone(),
                            detail: "the blob's sealed mime did not open".into(),
                        })?,
                )
            }
            _ => None,
        };

        let name = format!("blobs/{entity}/{field}/{id}");
        sink.open(&name)?;
        sink.write_bytes(&bytes)?;
        sink.close()?;
        written.push(ExportedObject {
            member: name,
            entity: entity.to_string(),
            field: field.to_string(),
            row_id: id,
            mime,
        });
    }
    Ok(written)
}

fn parse_blob_handle(hex: &str) -> Option<[u8; 16]> {
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

// ── the export ───────────────────────────────────────────────────────────

/// Export one space into `sink`.
pub async fn export_space(
    pool: &Pool,
    req: &ExportRequest<'_>,
    sink: &mut dyn BundleSink,
) -> Result<ExportReport, ExportError> {
    // First, and before a single row is read: is this database the shape
    // the artifact says it is? See the module doc.
    assert_no_drift(pool, req.artifact).await?;

    // Every member passes through the digester on its way out, so the
    // manifest can name what it is attesting. See `DigestingSink`.
    let mut digesting = DigestingSink::new(sink);
    let sink: &mut dyn BundleSink = &mut digesting;

    let mut members = Vec::new();
    let put = |sink: &mut dyn BundleSink,
               members: &mut Vec<String>,
               name: &str,
               body: &str|
     -> Result<(), ExportError> {
        sink.open(name)?;
        sink.write(body)?;
        sink.close()?;
        members.push(name.to_string());
        Ok(())
    };

    // The deploy's own bytes, not a re-rendering of them. A `schema.sql`
    // regenerated here could differ from the one that built the database
    // — which is the exact disagreement the drift check just ruled out,
    // and re-introducing it in the bundle would be perverse.
    put(sink, &mut members, "schema.sql", &req.artifact.ddl)?;
    put(sink, &mut members, "artifact.json", &req.artifact.artifact)?;

    // Name order, not source order: a bundle should not reorder because
    // somebody moved an `entity` block up the file.
    let mut entities: Vec<&ordo_schema::Entity> = req.schema.entities.iter().collect();
    entities.sort_by(|a, b| a.name.cmp(&b.name));

    // One lease for the whole export, released once and attributed to the
    // op's correlation — not one per row, which would make a custody audit
    // read as a storm.
    let lease = match req.sealed {
        SealedMode::Open => Some(
            req.keys
                .release(req.space, Purpose::Read, req.correlation)
                .map_err(|e| ExportError::KeyRelease(e.to_string()))?,
        ),
        SealedMode::Ciphertext => None,
    };

    let mut counts = Vec::new();
    let mut omitted: BTreeMap<String, Vec<(String, &'static str)>> = BTreeMap::new();
    let mut shapes: BTreeMap<String, Vec<(String, &'static str)>> = BTreeMap::new();

    for e in &entities {
        let physical = ordo_schema::ddl::physical_columns(e);
        let (cols, omit) = columns_of(&physical);
        let name = format!("data/{}.jsonl", e.name);
        sink.open(&name)?;
        let rows = write_entity(pool, req, lease.as_ref(), e, &cols, sink).await?;
        sink.close()?;
        members.push(name);
        counts.push((e.name.clone(), rows));
        omitted.insert(e.name.clone(), omit);
        shapes.insert(
            e.name.clone(),
            cols.iter()
                .map(|c| (c.name.clone(), encoding_name(c.encoding)))
                .collect(),
        );
    }

    sink.open("commandlog.jsonl")?;
    let log_groups = write_commandlog(pool, req.space, sink).await?;
    sink.close()?;
    members.push("commandlog.jsonl".into());

    // The tenant's own access profile: its op lines, per-member overrides,
    // groups and group links. Without it a moved tenant would fall back to
    // the app developer's defaults, which can be wider than what the tenant
    // set.
    let profile = profile_json(pool, req.space).await?;
    let profile_counts = profile.counts();
    put(
        sink,
        &mut members,
        PROFILE_MEMBER,
        &(serde_json::to_string_pretty(&profile.body).expect("a Value always serialises") + "\n"),
    )?;

    // The bytes `migration.md` §6 has always listed and no bundle has ever
    // carried. Written before the manifest so the manifest can digest them.
    let objects = write_blobs(pool, req, lease.as_ref(), sink).await?;
    members.extend(objects.iter().map(|o| o.member.clone()));

    let digests = digesting.digests.clone();
    let sink: &mut dyn BundleSink = &mut digesting;
    let manifest = manifest_json(
        req,
        &counts,
        &shapes,
        &omitted,
        log_groups,
        &members,
        &digests,
        &objects,
        profile_counts,
    );
    let manifest_bytes =
        serde_json::to_string_pretty(&manifest).expect("a Value always serialises") + "\n";
    let manifest_sha256 = bundle_digest(manifest_bytes.as_bytes());
    put(sink, &mut members, "manifest.json", &manifest_bytes)?;

    Ok(ExportReport {
        space: req.space.0.clone(),
        version: req.artifact.version.clone(),
        sealed: req.sealed.as_str(),
        entities: counts,
        log_groups,
        members,
        manifest_sha256,
    })
}

/// Build the artifact's shadow and refuse if the live schema is not it.
///
/// The shadow is dropped afterwards whatever the answer: it is rebuilt from
/// the DDL on every call, so leaving one behind buys nothing and leaves a
/// schema in the operator's catalog that looks like state.
async fn assert_no_drift(pool: &Pool, artifact: &Artifact) -> Result<(), ExportError> {
    pool.build_shadow_in(EXPORT_SHADOW, &artifact.ddl).await?;
    let drift = pool.live_drift(EXPORT_SHADOW, "public").await;
    let _ = pool
        .rows(&format!("DROP SCHEMA IF EXISTS {EXPORT_SHADOW} CASCADE"))
        .await;
    let drift = drift?;
    if !drift.is_empty() {
        return Err(ExportError::Drift {
            version: artifact.version.clone(),
            drift,
        });
    }
    Ok(())
}

fn encoding_name(e: Encoding) -> &'static str {
    match e {
        Encoding::Text => "text",
        Encoding::Integer => "integer",
        Encoding::Bool => "boolean",
        Encoding::Decimal => "decimal",
        Encoding::Json => "json",
        Encoding::Sealed => "sealed",
        Encoding::BlobRef { .. } => "blob_ref",
    }
}

/// One entity's rows as JSONL, keyset-paged on `id` in byte order.
async fn write_entity(
    pool: &Pool,
    req: &ExportRequest<'_>,
    lease: Option<&ordo_crypto::KeyLease>,
    entity: &ordo_schema::Entity,
    cols: &[ExportColumn<'_>],
    sink: &mut dyn BundleSink,
) -> Result<u64, ExportError> {
    let list: Vec<String> = cols.iter().map(select_expr).collect();
    // `id` is an implicit field of every entity and the second half of
    // every table's primary key, so it is always selectable and always
    // unique within a space — which is what makes the keyset cursor total.
    let id_at = cols
        .iter()
        .position(|c| c.name == "id")
        .expect("every entity has an implicit `id`");
    let table = ordo_schema::ddl::q(&entity.name);
    let space_col = ordo_schema::ddl::q(ordo_schema::SPACE_COLUMN);
    let space = sql_lit(&req.space.0);

    let mut written = 0u64;
    let mut after: Option<String> = None;
    loop {
        // `COLLATE "C"` on both the predicate and the sort: byte order is
        // the one ordering that does not move when the database's locale
        // does, and a bundle whose row order depends on `lc_collate` is not
        // byte-comparable between two deployments.
        let cursor = match &after {
            Some(a) => format!(" AND \"id\" COLLATE \"C\" > {}", sql_lit(a)),
            None => String::new(),
        };
        let sql = format!(
            "SELECT {} FROM {table} WHERE {space_col} = {space}{cursor} \
             ORDER BY \"id\" COLLATE \"C\" LIMIT {PAGE}",
            list.join(", ")
        );
        let rows = pool.rows(&sql).await?;
        if rows.is_empty() {
            break;
        }
        let n = rows.len();
        let mut chunk = String::new();
        for r in &rows {
            let row_id = r
                .get(id_at)
                .and_then(|c| c.clone())
                .unwrap_or_else(|| "<null id>".into());
            let mut obj = serde_json::Map::new();
            for (i, c) in cols.iter().enumerate() {
                let cell = r.get(i).and_then(|c| c.clone());
                obj.insert(
                    c.name.clone(),
                    encode(req, lease, entity, c, cell, &row_id)?,
                );
            }
            chunk.push_str(
                &serde_json::to_string(&serde_json::Value::Object(obj))
                    .expect("a Value always serialises"),
            );
            chunk.push('\n');
        }
        sink.write(&chunk)?;
        written += n as u64;
        after = Some(row_id_of(&rows, id_at));
        if n < PAGE {
            break;
        }
    }
    Ok(written)
}

fn row_id_of(rows: &[crate::TextRow], id_at: usize) -> String {
    rows.last()
        .and_then(|r| r.get(id_at))
        .and_then(|c| c.clone())
        .unwrap_or_default()
}

/// One cell, as JSON. The whole type story of the export, in one function.
fn encode(
    req: &ExportRequest<'_>,
    lease: Option<&ordo_crypto::KeyLease>,
    entity: &ordo_schema::Entity,
    col: &ExportColumn<'_>,
    cell: Option<String>,
    row_id: &str,
) -> Result<serde_json::Value, ExportError> {
    use serde_json::Value;
    let Some(s) = cell else {
        return Ok(Value::Null);
    };
    Ok(match col.encoding {
        Encoding::Text => Value::String(s),
        // A `bigint` that will not parse as `i64` cannot come out of a
        // `bigint` column, but the string is kept rather than dropped if it
        // somehow does: an export never invents and never discards.
        Encoding::Integer => match s.parse::<i64>() {
            Ok(n) => Value::from(n),
            Err(_) => Value::String(s),
        },
        Encoding::Bool => Value::Bool(s == "t" || s == "true"),
        // The handle, verbatim, in whichever spelling the tier stores. It
        // is not opened and it is not resolved: opening it would put the
        // object's bytes in the JSONL, which is the property
        // `plan-export.md` §2 exists to keep. The bytes are a bundle
        // member of their own.
        Encoding::BlobRef { .. } => Value::String(s),
        Encoding::Decimal => match s.parse::<serde_json::Number>() {
            Ok(n) => Value::Number(n),
            // A `numeric` with more precision than a JSON number carries
            // stays a string. Silently rounding somebody's ledger on the
            // way out of the platform would be a remarkable parting gift.
            Err(_) => Value::String(s),
        },
        // Postgres printed the `jsonb`, so it parses; a cell that somehow
        // does not stays the string rather than being dropped.
        Encoding::Json => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        Encoding::Sealed => {
            let field = col.field.unwrap_or(&col.name);
            let hex = s;
            match lease {
                None => Value::String(hex),
                Some(lease) => {
                    let bytes =
                        ordo_crypto::sealed::unhex(&format!("\\x{hex}")).ok_or_else(|| {
                            ExportError::SealedUnopened {
                                entity: entity.name.clone(),
                                field: field.to_string(),
                                row_id: row_id.to_string(),
                                detail: "the stored bytes are not a readable envelope".into(),
                            }
                        })?;
                    // An empty `bytea` is what a row written before this
                    // field existed holds; it is an absent value, not a
                    // failed open.
                    if bytes.is_empty() {
                        return Ok(Value::Null);
                    }
                    let plain = req
                        .keys
                        .open_at(
                            lease,
                            &entity.name,
                            field,
                            ordo_crypto::Anchor::Row(row_id),
                            &bytes,
                        )
                        .map_err(|e| ExportError::SealedUnopened {
                            entity: entity.name.clone(),
                            field: field.to_string(),
                            row_id: row_id.to_string(),
                            detail: e.to_string(),
                        })?;
                    // Sealed fields take text-shaped values (the write
                    // path refuses anything else), so the plaintext is a
                    // string. A sealed `json` holds its compact JSON text
                    // and is handed over as the document it is (28b).
                    let text =
                        String::from_utf8(plain).map_err(|e| ExportError::SealedUnopened {
                            entity: entity.name.clone(),
                            field: field.to_string(),
                            row_id: row_id.to_string(),
                            detail: format!("the opened value is not text: {e}"),
                        })?;
                    if col.json {
                        serde_json::from_str(&text).unwrap_or(Value::String(text))
                    } else {
                        Value::String(text)
                    }
                }
            }
        }
    })
}

/// The bundle member that carries the tenant's access profile.
pub const PROFILE_MEMBER: &str = "profile.json";

/// The profile member's own format tag.
pub const PROFILE_FORMAT: &str = "ordo.space-profile.v1";

struct ProfileBody {
    body: serde_json::Value,
}

impl ProfileBody {
    fn counts(&self) -> [u64; 4] {
        let n = |k: &str| {
            self.body
                .get(k)
                .and_then(serde_json::Value::as_array)
                .map(|a| a.len() as u64)
                .unwrap_or(0)
        };
        [n("lines"), n("overrides"), n("groups"), n("links")]
    }
}

fn csv_words(csv: &str) -> Vec<serde_json::Value> {
    csv.split(',')
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(|w| serde_json::Value::String(w.to_string()))
        .collect()
}

/// One table's rows, or none when the table does not exist yet (no tenant
/// has changed its profile, or the deployment has no platform entities).
async fn rows_if_table(
    pool: &Pool,
    table: &str,
    sql: &str,
) -> Result<Vec<crate::TextRow>, PoolError> {
    let present = pool
        .rows(&format!(r#"SELECT to_regclass('"{table}"') IS NOT NULL"#))
        .await?;
    let yes = matches!(
        present
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref()),
        Some("t") | Some("true")
    );
    if yes {
        pool.rows(sql).await
    } else {
        Ok(Vec::new())
    }
}

/// The tenant's access profile as a bundle member: every row, in a fixed
/// order, so two exports of one unchanged space stay byte-identical.
async fn profile_json(pool: &Pool, space: &SpaceId) -> Result<ProfileBody, PoolError> {
    use serde_json::{json, Value};
    let sp = sql_lit(&space.0);
    let cell = |r: &crate::TextRow, i: usize| -> String {
        r.get(i).and_then(|c| c.clone()).unwrap_or_default()
    };
    let lines = rows_if_table(
        pool,
        "$opaccess",
        &format!(
            r#"SELECT "op", "roles", "groups" FROM "$opaccess" WHERE "$space" = {sp} ORDER BY "op" COLLATE "C""#
        ),
    )
    .await?
    .iter()
    .map(|r| {
        json!({
            "op": cell(r, 0),
            "roles": csv_words(&cell(r, 1)),
            "groups": csv_words(&cell(r, 2)),
        })
    })
    .collect::<Vec<Value>>();
    let overrides = rows_if_table(
        pool,
        "$memberaccess",
        &format!(
            r#"SELECT "member", "op", "effect" FROM "$memberaccess" WHERE "$space" = {sp} ORDER BY "member" COLLATE "C", "op" COLLATE "C""#
        ),
    )
    .await?
    .iter()
    .map(|r| json!({ "member": cell(r, 0), "op": cell(r, 1), "effect": cell(r, 2) }))
    .collect::<Vec<Value>>();
    let groups = rows_if_table(
        pool,
        "members",
        &format!(
            r#"SELECT "id", "displayName" FROM "members" WHERE "$space" = {sp} AND "kind" = 'group' ORDER BY "id" COLLATE "C""#
        ),
    )
    .await?
    .iter()
    .map(|r| json!({ "id": cell(r, 0), "name": cell(r, 1) }))
    .collect::<Vec<Value>>();
    let links = rows_if_table(
        pool,
        "groupMembers",
        &format!(
            r#"SELECT "group", "member" FROM "groupMembers" WHERE "$space" = {sp} ORDER BY "group" COLLATE "C", "member" COLLATE "C""#
        ),
    )
    .await?
    .iter()
    .map(|r| json!({ "group": cell(r, 0), "member": cell(r, 1) }))
    .collect::<Vec<Value>>();
    Ok(ProfileBody {
        body: json!({
            "format": PROFILE_FORMAT,
            "lines": lines,
            "overrides": overrides,
            "groups": groups,
            "links": links,
        }),
    })
}

/// The space's chain, one JSON object per group, in chain order.
///
/// This is the *"settled JSONL bundle format, chain hashes included, so the
/// history's integrity survives the platform"* of `migration.md` §6. Both
/// hashes travel with every group, so a reader with nothing but this file
/// can re-run `undo::verify`'s arithmetic — recompute
/// `SHA-256(prev || body)` and check the link — without the platform, which
/// is the whole point of exporting a chain rather than a list of events.
async fn write_commandlog(
    pool: &Pool,
    space: &SpaceId,
    sink: &mut dyn BundleSink,
) -> Result<u64, PoolError> {
    let sid = ordo_commandlog::SpaceId(space.0.clone());
    let store = crate::commandlog::PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let mut written = 0u64;
    let mut from = 0usize;
    loop {
        let entries = store.try_entries(&sid, from, PAGE).await?;
        if entries.is_empty() {
            break;
        }
        let n = entries.len();
        let mut chunk = String::new();
        for (i, e) in entries.iter().enumerate() {
            chunk.push_str(
                &serde_json::to_string(&group_json(from + i, e))
                    .expect("a Value always serialises"),
            );
            chunk.push('\n');
        }
        sink.write(&chunk)
            .map_err(|e| PoolError::db(e.to_string()))?;
        written += n as u64;
        from += n;
        if n < PAGE {
            break;
        }
    }
    Ok(written)
}

fn group_json(idx: usize, e: &ordo_commandlog::ChainedGroup) -> serde_json::Value {
    let mut out = serde_json::json!({
        "idx": idx,
        "prev": e.prev.to_string(),
        "hash": e.hash.to_string(),
        "correlation": e.group.correlation.0,
        "actor": e.group.actor.to_string(),
        "commands": e.group.commands.iter().map(command_json).collect::<Vec<_>>(),
    });
    // **Not optional decoration.** `plan-export.md` §2.3 promises that a
    // reader holding nothing but `commandlog.jsonl` can recompute
    // `SHA-256(prev ‖ body)` and check the links without the platform. That
    // is only true if every byte of `body` is recoverable from the line, and
    // the rule-6 flag is a byte of it. Omitted here, a flagged group would
    // re-encode a byte short and an outside verifier would read a lawful
    // group as tampering.
    //
    // Emitted by presence, exactly as the chain encodes it: absent means
    // false, so an unflagged group's line is unchanged and re-encodes to the
    // same bytes — and `grep ownerAdministration` over an exported chain is
    // the list of every rule-6 command in it, which is the question an
    // auditor came with.
    if e.group.owner_administration {
        out["ownerAdministration"] = serde_json::Value::Bool(true);
    }
    out
}

fn command_json(c: &ordo_commandlog::Command) -> serde_json::Value {
    serde_json::json!({
        "id": c.id.0,
        "entity": c.entity,
        "row": c.row_id,
        "kind": match c.kind {
            ordo_commandlog::CommandKind::Create => "create",
            ordo_commandlog::CommandKind::Converge => "converge",
            ordo_commandlog::CommandKind::Delete => "delete",
        },
        "at": c.at.0,
        "before": c.before.as_ref().map(image_json),
        "after": c.after.as_ref().map(image_json),
    })
}

/// One image. A sealed field appears as its opaque handle and **never as a
/// value**: the handle is random per write and leaks nothing, which is
/// exactly why the chain carries it. Rendering it as an object rather than
/// a bare string keeps it distinguishable from a `text` value that happens
/// to be 32 hex characters.
fn image_json(img: &ordo_commandlog::Image) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (k, v) in img {
        out.insert(
            k.clone(),
            match v {
                ordo_commandlog::Value::Null => serde_json::Value::Null,
                ordo_commandlog::Value::Bool(b) => serde_json::Value::Bool(*b),
                ordo_commandlog::Value::Int(n) => serde_json::Value::from(*n),
                ordo_commandlog::Value::Text(s) | ordo_commandlog::Value::Id(s) => {
                    serde_json::Value::String(s.clone())
                }
                ordo_commandlog::Value::SealedRef(h) => {
                    serde_json::json!({ "sealedRef": ordo_crypto::hex(h) })
                }
                // A decimal is exact, so a string: never a JSON number a reader
                // would parse into a float (27ak).
                ordo_commandlog::Value::Decimal(d) => serde_json::Value::String(d.to_string()),
            },
        );
    }
    serde_json::Value::Object(out)
}

/// The order an importer must insert entities in, and the entities for
/// which no such order exists.
///
/// Every ref is a foreign key `($space, field) REFERENCES target($space,
/// id)`, so a reader loading the JSONL back has to insert a referenced
/// entity before a referencing one. The **bundle order is not that order**
/// — it is name order, so a bundle is diffable — and leaving the importer
/// to work it out would mean shipping a bundle that cannot be loaded
/// without re-deriving the ref graph the `artifact.json` beside it already
/// describes. So the manifest states it.
///
/// Kahn's algorithm over `Schema::ref_edges`, which excludes tree
/// self-loops. Anything left when no node has zero in-degree is a
/// reference cycle: those entities have no valid sequential order at all
/// and must be loaded in one statement (or with constraints deferred),
/// which is what the manifest's second list says.
fn load_order(schema: &Schema) -> (Vec<String>, Vec<String>) {
    type Names = std::collections::BTreeSet<String>;
    let mut names: Vec<String> = schema.entities.iter().map(|e| e.name.clone()).collect();
    names.sort();
    // `waiting[x]` = the entities x must be loaded after.
    let mut waiting: BTreeMap<String, Names> =
        names.iter().map(|n| (n.clone(), Names::new())).collect();
    for edge in schema.ref_edges() {
        // A self-reference that is not a tree still orders rows within one
        // table, never tables against each other, so it is not an edge
        // here — it would make every such entity look cyclic.
        if edge.from == edge.to {
            continue;
        }
        if let Some(set) = waiting.get_mut(&edge.from) {
            set.insert(edge.to.clone());
        }
    }

    let mut ordered: Vec<String> = Vec::with_capacity(names.len());
    let mut placed = Names::new();
    loop {
        // Name order among the ready set, so the answer is deterministic
        // rather than whatever the map happened to yield first.
        let ready: Vec<String> = waiting
            .iter()
            .filter(|(n, deps)| !placed.contains(*n) && deps.iter().all(|d| placed.contains(d)))
            .map(|(n, _)| n.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        for n in ready {
            placed.insert(n.clone());
            ordered.push(n);
        }
    }
    let cyclic: Vec<String> = names.into_iter().filter(|n| !placed.contains(n)).collect();
    (ordered, cyclic)
}

/// The bundle's self-description. No clock: see the module doc on
/// determinism.
#[allow(clippy::too_many_arguments)]
fn manifest_json(
    req: &ExportRequest<'_>,
    counts: &[(String, u64)],
    shapes: &BTreeMap<String, Vec<(String, &'static str)>>,
    omitted: &BTreeMap<String, Vec<(String, &'static str)>>,
    log_groups: u64,
    members: &[String],
    // Member name -> (bytes, sha256 hex). See `DigestingSink`.
    digests: &BTreeMap<String, (u64, String)>,
    objects: &[ExportedObject],
    profile: [u64; 4],
) -> serde_json::Value {
    let entities: Vec<serde_json::Value> = counts
        .iter()
        .map(|(name, rows)| {
            serde_json::json!({
                "entity": name,
                "file": format!("data/{name}.jsonl"),
                "rows": rows,
                "columns": shapes.get(name).map(|cs| cs
                    .iter()
                    .map(|(n, e)| serde_json::json!({ "name": n, "encoding": e }))
                    .collect::<Vec<_>>())
                    .unwrap_or_default(),
                "omitted": omitted.get(name).map(|os| os
                    .iter()
                    .map(|(n, why)| serde_json::json!({ "name": n, "reason": why }))
                    .collect::<Vec<_>>())
                    .unwrap_or_default(),
            })
        })
        .collect();
    let (order, cyclic) = load_order(req.schema);

    serde_json::json!({
        "format": FORMAT,
        "space": req.space.0,
        "artifact": {
            "version": req.artifact.version,
            "domain": req.schema.domain,
            "file": "artifact.json",
            "schema": "schema.sql",
        },
        "sealed": req.sealed.as_str(),
        "ordering": "rows by `id` under the C collation; object keys sorted; no clock \
                     anywhere, so two exports of one unchanged space are byte-identical",
        "entities": entities,
        "load": {
            "order": order,
            "cyclic": cyclic,
            "note": "insert entities in `order`: every ref is a foreign key on \
                     ($space, id), so a referenced entity must land before a referencing \
                     one. `cyclic` names entities that reference each other and therefore \
                     have no sequential order — load those in one statement, or with \
                     constraints deferred. A `tree` self-reference orders ROWS within its \
                     own table (parents before children) and does not appear in either list.",
        },
        "commandlog": {
            "file": "commandlog.jsonl",
            "groups": log_groups,
            "note": "one group per line in chain order, each carrying `prev` and `hash`, \
                     so the chain verifies without the platform",
        },
        "profile": {
            "file": PROFILE_MEMBER,
            "format": PROFILE_FORMAT,
            "lines": profile[0],
            "overrides": profile[1],
            "groups": profile[2],
            "links": profile[3],
            "note": "the tenant's own access profile. Member ids are the source space's; \
                     an import maps them through its people map and refuses a member it \
                     cannot place, so a profile never lands on the wrong person",
        },
        "members": members,
        // **Per-member digests**, which `plan-production-ops.md` R1 found
        // missing: *"a restore marker cannot attest a bundle it cannot
        // name"*. `sha256` over the member's exact bytes, and `bytes` so a
        // reader can check the length before it reads. The manifest names
        // every member written before it and cannot name itself —
        // something outside attests the manifest, which is what a restore
        // marker is for.
        "digests": digests
            .iter()
            .map(|(name, (bytes, sha))| serde_json::json!({
                "member": name,
                "bytes": bytes,
                "sha256": sha,
            }))
            .collect::<Vec<_>>(),
        "blobs": {
            "carried": req.blobs.is_some(),
            "objects": objects.len() as u64,
            "layout": "blobs/<entity>/<field>/<id>",
            // **What the member name cannot be trusted to say.** The name
            // spells the same three values, and recovering them from it
            // would be parsing a path the exporter formatted — one id
            // holding a `/` and an importer would place an object on the
            // wrong row rather than refuse. `mime` is here because it is
            // nowhere else: it is a sealed envelope on `$blobref`, so a
            // reader cannot open it and cannot derive it from the bytes.
            // See `ExportedObject::mime` for why it is conditional.
            "files": objects
                .iter()
                .map(|o| {
                    let mut e = serde_json::Map::new();
                    e.insert("member".into(), serde_json::json!(o.member));
                    e.insert("entity".into(), serde_json::json!(o.entity));
                    e.insert("field".into(), serde_json::json!(o.field));
                    e.insert("id".into(), serde_json::json!(o.row_id));
                    if let Some(mime) = &o.mime {
                        e.insert("mime".into(), serde_json::json!(mime));
                    }
                    serde_json::Value::Object(e)
                })
                .collect::<Vec<_>>(),
            "note": if req.blobs.is_some() {
                match req.sealed {
                    SealedMode::Open =>
                        "one file per referenced blob, decrypted under the owner's key \
                         release — the same authority `migration.md` §6 grants the rows. \
                         The BUCKET does not lay objects out this way and must not: an \
                         object key carrying an entity and a field would enumerate the \
                         schema to anyone who can list it.",
                    SealedMode::Ciphertext =>
                        "one file per referenced blob, as stored: the chunked STREAM \
                         envelope, unopened. Same rule as the sealed columns beside it — \
                         the caller asked for a bundle the platform did not decrypt.",
                }
            } else {
                "this deployment has no blob tier wired, so no object store was consulted. \
                 Distinct from a schema with no blob fields, which carries an empty list \
                 for a different reason."
            },
        },
        // **Empty**, and it is the closing condition of `plan-blob.md`.
        //
        // This list carried exactly one entry for the whole life of the
        // export: `"blob bytes"`, with the reason *"blob custody is
        // unbuilt — there is no blob field type, no object-store resolver
        // and no inline payload"*. All three exist now, and the bundle
        // carries the layout `migration.md` §6 describes.
        //
        // The list stays rather than being deleted. It is the machine-
        // readable place a future gap is declared, and an export that
        // silently omitted something would be the failure the whole module
        // doc is written against.
        "unbuilt": serde_json::Value::Array(Vec::new()),
    })
}
