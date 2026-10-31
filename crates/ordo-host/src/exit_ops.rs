//! The door out, and since 11-10a the door back in: `std.space.export`,
//! `std.space.delete` and `std.space.import`.
//!
//! `definition-language.md` §8 pins the two exit ops as **owner-only,
//! step-up gated, command-logged**. This module is the second and third of
//! those; the first is `deployment_engine`'s registration loop, which lists
//! only `Role::Owner` for these names.
//!
//! **Import is here rather than in a module of its own**, and the reason is
//! that it is the same act read backwards. It is gated identically, by the
//! same `owner_only` list. It resolves the same artifact. It is recorded in
//! the same stream, for the same reason. And it is the second leg of the one
//! argument the first leg was built for: `licensing.md` says a customer must
//! be able to get their data out, and a customer who cannot get it back in
//! has been given half of that. Splitting the legs across two modules would
//! have let one of them drift from the other's gate, which is the failure
//! `owner_only`'s own doc describes.
//!
//! # What "command-logged" turns out to mean, and why
//!
//! §8's word is *command-logged*, and for these two ops it cannot be taken
//! at face value. The command log records **writes to entity rows**: its
//! closed taxonomy is `Create`/`Converge`/`Delete`, and every command
//! carries an entity, a row id and a before/after image. Neither exit op is
//! a write to a row.
//!
//! - **Export mutates nothing.** Forging a `Converge` with equal images to
//!   record a read would put a lie in the chain — and the chain's whole
//!   value is that a reader may take what it says literally. A revert of
//!   that command would be a revert of nothing.
//! - **Deletion destroys the chain.** A final group appended to a log that
//!   is deleted in the same act records nothing, and a log that survives
//!   its space is a space that was not deleted.
//!
//! So both are recorded in the **security-event stream**, as
//! `EventKind::CustodyAction` — beside epoch rotation, epoch destruction and
//! custody conversion, which are the acts these two are actually siblings
//! of. (Break-glass will join them when it is built as the grant-gated
//! operation the owner ruled — corpus-rulings.md, 2026-09-07.) The spine
//! has two halves, and the exit ops belong
//! to the half that records *decisions about a space* rather than *changes
//! to its rows*. The requirement §8 is making — **the owner cannot do this
//! without leaving a record** — is met, and met by the half of the spine
//! that can outlive the space.
//!
//! The record is written at **space scope** for export and for deletion
//! alike. For deletion that is deliberate and only works because
//! `ordo_exec::erase::RETAINED` keeps the security stream: the one record
//! that must outlive a space is the record that it was deleted.
//!
//! # Idempotency
//!
//! `PlatformExecutor` does not carry the durable [`ReplayLease`] into these
//! handlers, and that is stated rather than hidden. It is tolerable here
//! and would not be for a domain write, because **both ops are idempotent
//! by construction**: an export is a pure function of state (re-running
//! writes a byte-identical bundle to a new directory), and an erasure of an
//! already-erased space destroys nothing that is there and deletes no rows
//! that remain. A crash between reservation and response therefore costs a
//! duplicate bundle at worst. The in-memory replay cache still makes a
//! retry verbatim; what is skipped is the `$replay` journal, and closing
//! that gap is platform-op work rather than exit-op work.

use std::path::PathBuf;
use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_crypto::{KeyProvider, SpaceId};
use ordo_exec::cadence::LogPolicy;
use ordo_exec::erase::{self, EraseRefusal};
use ordo_exec::export::{self, DirectoryBundle, ExportError, SealedMode};
use ordo_exec::route::PoolResolver;
use ordo_exec::{registry, Pool};
use serde_json::json;

/// `std.space.export` — the whole space as a bundle on disk.
pub const EXPORT: &str = "std.space.export";
/// `std.space.delete` — crypto-erasure, then the rows.
pub const DELETE: &str = "std.space.delete";
/// `std.space.import` — a staged bundle loaded through the write protocol.
///
/// **The door is what makes this callable at all.** `ordo_exec::import`
/// has been built since 11-10a and nothing in any shipped binary could
/// reach it: the CLI has no custody selection surface, and an import that
/// cannot seal cannot write a sealed field (17y finding 1). The deployment
/// holds custody, so the act belongs on this side of the wire, which is
/// also the 2026-09-20 ruling for every operator act.
pub const IMPORT: &str = "std.space.import";

/// Everything the exit ops need that the access engine does not have.
///
/// Optional on [`crate::platform_ops::PlatformOps`] for the reason its
/// module doc gives about the grant registry, applied once more: a
/// deployment either wires this and registers the two specs, or wires
/// neither. There is no third state in which the ops are callable and
/// answer partially — a `std.space.export` that returned an empty bundle
/// because nobody configured a directory would be the worst possible
/// answer to give an owner who is leaving.
pub struct ExitOps {
    /// The control database, where the artifact registry is. The registry
    /// is about the deployment rather than any one space, so it does not
    /// move when a space does.
    pub pool: Pool,
    /// Which database a space's rows are in. Export reads them and deletion
    /// erases them, so both go where the space is rather than where the
    /// registry is; with nothing attached the two are one database and the
    /// distinction costs nothing. A deployment hands over the resolver its
    /// op path holds (`Spaces::pools`), so the rows exported are the rows
    /// the space's requests read.
    pub pools: Arc<dyn PoolResolver>,
    pub keys: Arc<dyn KeyProvider>,
    /// The log policy whose `undoHorizon` the deletion discloses.
    pub policy: LogPolicy,
    /// Where bundles are written. One sub-directory per export; see
    /// [`ExitOps::bundle_dir`].
    pub bundles: PathBuf,
    /// Where staged bundles are **read** from, for `std.space.import`.
    ///
    /// A second root rather than the one above, and the reason is
    /// disclosure. Exports land under `<bundles>/<space>/<n>`, so letting an
    /// import name a path inside that tree would mean the caller naming
    /// another space's directory: on a hosted deployment that is a tenant
    /// reading which other tenants have exported, and the traversal surface
    /// is the caller's to probe. Under this root an operator stages a bundle
    /// in the **destination space's** own directory, and the caller names
    /// only the bundle: see [`ExitOps::staged`] for why that component is an
    /// authorization boundary rather than a layout.
    ///
    /// `None` is *this deployment does not serve imports*, and then the spec
    /// is not registered, on the same no-third-state rule as the struct
    /// itself: an import op that answered "no directory is configured"
    /// would be an op that exists and cannot work.
    pub imports: Option<PathBuf>,
    /// The blob door the op path holds, so an import's attachments take the
    /// **one** upload path (`plan-blob-chain.md` C1–C8, asserted against the
    /// workspace's source by `tests/upload_seam.rs`).
    ///
    /// Distinct from [`ExitOps::blobs`], which is the sealing pipeline that
    /// export reads and erasure sweeps through. An import does not read or
    /// sweep objects, it *uploads* them, and the only thing that may mint an
    /// upload is the door. `None` here is what
    /// `ordo_exec::import::ImportRequest::blobs: None` means: no blob tier
    /// is wired, and a bundle carrying objects is refused by name rather
    /// than loaded without its files.
    pub door: Option<Arc<dyn ordo_exec::import::BlobDoor>>,
    /// The security-event stream both ops record into. `None` in an
    /// embedding with no stream — and then the record is the one thing
    /// missing, so a deployment that means to serve these should have one.
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
    /// The blob pipeline, when the deployment serves blobs.
    ///
    /// `None` is *no blob tier is wired*, which the export manifest states
    /// as such — distinct from a schema with no blob fields, which is a
    /// different fact and reads the same way if nobody says which. The
    /// deletion needs it for the same reason: crypto-erasure would leave a
    /// space's objects unopenable but still listable, and an object's
    /// existence and size are observable without any key.
    pub blobs: Option<Arc<ordo_blob::BlobPipeline>>,
    /// Which store a space's bytes are in. Held beside the pipeline
    /// because erasure deletes through the resolver rather than through a
    /// single store — a space with a BYO bucket is erased in its own
    /// bucket, or it is not erased.
    pub stores: Option<Arc<dyn ordo_blob::StoreResolver>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl ExitOps {
    /// The database `space`'s rows are in.
    async fn space_pool(&self, space: &SpaceId) -> Result<Pool, ExecError> {
        self.pools.resolve(space).await.map_err(|e| {
            ExecError::of_fault(
                e.class(),
                ErrorCode::Internal,
                format!("the space's database: {e}"),
            )
        })
    }

    /// The deployed artifact, and the program compiled from its own stored
    /// source.
    ///
    /// Compiled here rather than borrowed from the live `Deployment`, and
    /// the difference matters: the bundle ships `artifact.json` and
    /// `schema.sql` as *this artifact's stored bytes*, so the schema that
    /// decides which columns to read must be the one those bytes describe.
    /// Reading the serving deployment's schema instead would be right
    /// almost always and wrong exactly during a swap, which is the moment
    /// nobody would be watching.
    async fn deployed(&self) -> Result<(registry::Artifact, ordo_schema::Schema), ExecError> {
        let stored = registry::active(&self.pool)
            .await
            .map_err(|e| {
                ExecError::of_fault(
                    e.class(),
                    ErrorCode::Internal,
                    format!("the artifact registry: {e}"),
                )
            })?
            .ok_or_else(|| {
                ExecError::internal(
                    "this deployment has no active artifact, so there is no schema to \
                     export against — run `ordo apply` against this database",
                )
            })?;
        let fixture = crate::Fixture::from_source(&stored.source, "").map_err(|e| {
            ExecError::internal(format!(
                "the active artifact `{}` no longer compiles under this binary, so its \
                 shape cannot be read: {e}",
                stored.version
            ))
        })?;
        Ok((stored, fixture.schema))
    }

    /// The deployed artifact and its **whole** program, bound to one space
    /// and to this deployment's custody.
    ///
    /// [`ExitOps::deployed`] keeps only the schema, because export reads
    /// columns and needs nothing else. An import writes through the Stage-5
    /// protocol, which seals, digests and mints handles through
    /// `Fixture::keys`, so the two assignments `deploy::verify_candidate`
    /// makes are made here too and for the same stated reason (R2a, ruled
    /// 2026-09-07): `Fixture::compile_source` mints a `TestProvider` over
    /// the conformance space, because a compiler has no business deciding
    /// whose keys a program seals under. Threading custody into the
    /// `Executor` alone would cover the read path and leave every seal this
    /// import writes under a key that is a constant in this repository.
    ///
    /// Tenancy first, then custody, in that order, so which of the two
    /// carries across cannot silently matter.
    async fn deployed_fixture(
        &self,
        space: &str,
    ) -> Result<(registry::Artifact, Arc<crate::Fixture>), ExecError> {
        let stored = registry::active(&self.pool)
            .await
            .map_err(|e| {
                ExecError::of_fault(
                    e.class(),
                    ErrorCode::Internal,
                    format!("the artifact registry: {e}"),
                )
            })?
            .ok_or_else(|| {
                ExecError::internal(
                    "this deployment has no active artifact, so there is no schema to \
                     import against — run `ordo apply` against this database",
                )
            })?;
        let compiled = crate::Fixture::from_source(&stored.source, "").map_err(|e| {
            ExecError::internal(format!(
                "the active artifact `{}` no longer compiles under this binary, so nothing \
                 can be imported against it: {e}",
                stored.version
            ))
        })?;
        let mut fixture = compiled.in_space(space);
        fixture.keys = self.keys.clone();
        Ok((stored, Arc::new(fixture)))
    }

    /// A fresh directory for one bundle: `<root>/<space>/<n>`, numbered
    /// from the directories already there.
    ///
    /// Never reused and never overwritten. Two exports of one unchanged
    /// space produce byte-identical bundles, so overwriting would be
    /// harmless in the common case and catastrophic in the one where the
    /// space *did* change and somebody was still reading the old bundle.
    fn bundle_dir(&self, space: &str) -> Result<PathBuf, ExecError> {
        // The space id comes from the door's bound space, never from a
        // request body — but it becomes a path component, so it is checked
        // rather than trusted. A space id is minted, not authored.
        if space.is_empty()
            || !space
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ExecError::internal(
                "the bound space id is not a name that can be a directory",
            ));
        }
        let root = self.bundles.join(space);
        std::fs::create_dir_all(&root)
            .map_err(|e| ExecError::internal(format!("{}: {e}", root.display())))?;
        let used: usize = std::fs::read_dir(&root)
            .map_err(|e| ExecError::internal(format!("{}: {e}", root.display())))?
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_string_lossy().parse::<usize>().ok())
            .max()
            .unwrap_or(0);
        let dir = root.join(format!("{:04}", used + 1));
        std::fs::create_dir(&dir)
            .map_err(|e| ExecError::internal(format!("{}: {e}", dir.display())))?;
        Ok(dir)
    }

    fn record(&self, space: &str, caller: &str, op: &str, action: &str, correlation: &str) {
        let Some(stream) = &self.stream else {
            return;
        };
        stream.observe(
            ordo_secevent::Observation {
                // Space scope, for both ops. The owner's own stream is
                // where an act on the owner's own space belongs — and for
                // the deletion it survives, because `erase::RETAINED`
                // keeps it.
                space: Some(ordo_commandlog::SpaceId(space.to_string())),
                subject: ordo_secevent::Subject::member(caller),
                op: op.to_string(),
                kind: ordo_secevent::EventKind::CustodyAction {
                    action: action.to_string(),
                },
                at: ordo_commandlog::Timestamp((self.now)()),
                correlation: ordo_secevent::correlation::adopt(correlation),
            },
            // The caller is the space's owner: a membership, by definition
            // of who reaches these ops at all.
            ordo_secevent::Relationship::Member,
        );
    }

    /// `std.space.export`.
    pub async fn export(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        // The one knob, and it is a bool so that a typo is the DOOR's
        // refusal rather than this handler's — see the spec's comment on
        // the parameter. Absent, or false, is the disclosure
        // `migration.md` §6 describes: opened under the owner's authority.
        // Ciphertext is the honest answer for a space whose keys this
        // deployment does not hold, and for one whose epochs are destroyed.
        let sealed = match bound.0.get("ciphertext").and_then(|v| v.as_bool()) {
            Some(true) => SealedMode::Ciphertext,
            _ => SealedMode::Open,
        };

        let (artifact, schema) = self.deployed().await?;
        let sid = SpaceId(space.to_string());
        let correlation = format!("exp_{space}_{}", artifact.version);
        let dir = self.bundle_dir(space)?;
        let mut sink = DirectoryBundle::new(&dir);
        let pool = self.space_pool(&sid).await?;

        let report = export::export_space(
            &pool,
            &export::ExportRequest {
                space: &sid,
                schema: &schema,
                artifact: &artifact,
                keys: self.keys.as_ref(),
                blobs: self.blobs.as_deref(),
                sealed,
                correlation: &correlation,
            },
            &mut sink,
        )
        .await
        .map_err(export_refusal)?;

        // Recorded on success only. A refused export disclosed nothing, and
        // recording it as a disclosure would put a false total-disclosure
        // event in the owner's own stream.
        self.record(space, caller, EXPORT, "space_exported", &correlation);

        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "space": report.space,
                "version": report.version,
                "sealed": report.sealed,
                "bundle": dir.display().to_string(),
                "members": report.members,
                "entities": report.entities.iter().map(|(n, c)| json!({
                    "entity": n, "rows": c,
                })).collect::<Vec<_>>(),
                "rows": report.rows(),
                "logGroups": report.log_groups,
                "manifest_sha256": report.manifest_sha256,
            })],
            next_cursor: None,
        })
    }

    /// A staged bundle's directory: `<root>/<destination space>/<bundle>`.
    ///
    /// **The space in the path is an authorization boundary, not a tidy
    /// layout.** Without it, one staging root would be a shared one: an
    /// operator who stages a tenant's bundle so that tenant can reload it
    /// would have made it importable by *every* other tenant's owner, each
    /// of whom is the owner of their own space and passes every gate this op
    /// has. They would then hold the whole of somebody else's space,
    /// opened, in a space whose keys they control. With the space in the
    /// path, a bundle staged under one space's directory cannot be named by
    /// any other caller, because the caller does not supply that component:
    /// the door does, from the space it bound.
    ///
    /// **One refusal for a name that is not a plain name and for a name
    /// nothing is standing under**, deliberately not two, on the same
    /// argument `bucket_refused`'s allowlist makes: a caller who can tell
    /// "no such bundle" from "that is not a legal name" can map the staging
    /// directory one guess at a time, and what an operator has staged is the
    /// operator's business.
    ///
    /// The alphabet is the one [`ExitOps::bundle_dir`] already trusts for a
    /// space id, applied to both components: ASCII alphanumerics, `_` and
    /// `-`. No `.` at all, so `..` is unspellable rather than filtered, and
    /// the resolved path is therefore always exactly two directories under
    /// the root.
    fn staged(
        &self,
        root: &std::path::Path,
        space: &str,
        bundle: &str,
    ) -> Result<PathBuf, ExecError> {
        let plain = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        let legal = plain(bundle) && plain(space);
        let dir = root.join(space).join(bundle);
        if !legal || !dir.is_dir() {
            return Err(ExecError::new(
                ErrorCode::ImportRefused,
                "bundle_unknown: no bundle of that name is staged for import".to_string(),
            ));
        }
        Ok(dir)
    }

    /// The people map staged beside a bundle, as
    /// `<root>/<space>/<bundle>.people`.
    ///
    /// **Beside the bundle and not inside it**, for two reasons that both
    /// matter. The bundle's manifest carries a digest per member and the
    /// importer refuses a member the manifest does not describe, so a file
    /// added inside would make the bundle incomplete. And the map is *not*
    /// part of the export: it is the operator's statement about who the
    /// people in the destination are, and the same bundle honestly takes a
    /// different map in a different destination.
    ///
    /// A JSON object, source member id to destination member id. Absent is
    /// **no map**, which is the behaviour before the map existed: the first
    /// row naming a person is refused and nothing is written. That is a
    /// deliberate default rather than an oversight, because the alternative
    /// to a map the operator wrote is a guess about authorship.
    fn people_for(
        &self,
        root: &std::path::Path,
        space: &str,
        bundle: &str,
    ) -> Result<Vec<(String, String)>, ExecError> {
        let path = root.join(space).join(format!("{bundle}.people"));
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            // Any read failure is *no map*, including a permission error, and
            // that is safe in the one direction that counts: without a map
            // the import refuses rather than attributing rows to anybody.
            Err(_) => return Ok(Vec::new()),
        };
        let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
            ExecError::new(
                ErrorCode::ImportRefused,
                format!(
                    "people_unreadable: `{bundle}.people` is not JSON ({e}). It is an object \
                     of source member id to destination member id"
                ),
            )
        })?;
        let object = parsed.as_object().ok_or_else(|| {
            ExecError::new(
                ErrorCode::ImportRefused,
                format!(
                    "people_unreadable: `{bundle}.people` is JSON but not an object of source \
                     member id to destination member id"
                ),
            )
        })?;
        let mut out = Vec::with_capacity(object.len());
        for (source, destination) in object {
            let destination = destination.as_str().ok_or_else(|| {
                ExecError::new(
                    ErrorCode::ImportRefused,
                    format!(
                        "people_unreadable: `{bundle}.people` maps `{source}` to something \
                         that is not a member id"
                    ),
                )
            })?;
            out.push((source.clone(), destination.to_string()));
        }
        Ok(out)
    }

    /// `std.space.import`.
    ///
    /// The half of the trust argument that has had an engine and no caller
    /// since 11-10a. Every property of a loaded row is inherited from the
    /// write protocol rather than restated here: the seals, the blind
    /// indexes, the guards, the ids, the images and the chain. What this
    /// function decides is *which* bundle, *whose* custody and *which*
    /// space, and then it gets out of the way.
    pub async fn import(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        // Not `import_refused`: a deployment that registered this op and
        // wired no directory is misconfigured, which is nothing the caller
        // can repair by changing the request. The registration invariant
        // means this is unreachable, and it is written rather than
        // `unwrap`ed because "unreachable" is a claim about wiring that a
        // future embedding can falsify.
        let Some(root) = self.imports.as_ref() else {
            return Err(ExecError::internal(
                "this deployment registered `std.space.import` and configured no staging \
                 directory, so there is nowhere for a bundle to be",
            ));
        };
        let bundle = bound
            .0
            .get("bundle")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        // The import's name, which scopes `$importmap` and every batch
        // lease, so it is what makes a re-run a resume rather than a second
        // import. Required at the door; an empty string would still bind, so
        // it is checked here and refused as the bundle name is, because an
        // unnamed import cannot be resumed and the operator has to know
        // that before the first row lands rather than after the last.
        let name = bound
            .0
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if name.is_empty()
            || name.len() > 64
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ExecError::new(
                ErrorCode::ImportRefused,
                "unnamable_import: an import's name scopes its resume, so it must be a \
                 plain name of at most 64 characters"
                    .to_string(),
            ));
        }
        let dir = self.staged(root, space, bundle)?;
        let people = self.people_for(root, space, bundle)?;

        let (artifact, fixture) = self.deployed_fixture(space).await?;
        let sid = SpaceId(space.to_string());
        let pool = self.space_pool(&sid).await?;
        let exec = ordo_exec::Executor {
            pool,
            schema: fixture.schema.clone(),
            registry: fixture.registry.clone(),
            keys: self.keys.clone(),
            space: sid.clone(),
        };
        // Deterministic in the import's name, so a resumed import's custody
        // audit and key-release records read as one act rather than as one
        // per attempt. The same property `ImportObject::correlation` states
        // for an object.
        let correlation = format!("imp_{space}_{name}");
        let source = ordo_exec::import::DirectorySource::new(&dir);

        let report = ordo_exec::import::import_space(
            &exec,
            &ordo_exec::import::ImportRequest {
                name,
                fixture,
                artifact: &artifact,
                caller,
                batch: ordo_exec::import::BATCH,
                people: &people,
                blobs: self.door.as_deref(),
            },
            &source,
        )
        .await
        .map_err(import_refusal)?;

        // Recorded on success only, on `export`'s reasoning read the other
        // way round. An export that refused disclosed nothing; an import
        // that refused **wrote nothing**, and the rows a partial import did
        // write are already in the chain as the commands that wrote them.
        // What this adds is the fact the chain cannot carry: that an owner
        // authorised a bulk load out of a foreign bundle, which otherwise
        // reads in the log as ordinary traffic from one principal.
        self.record(space, caller, IMPORT, "space_imported", &correlation);

        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "space": report.space,
                "name": report.name,
                "bundle": bundle,
                "entities": report.entities.iter().map(|(n, c)| json!({
                    "entity": n, "rows": c,
                })).collect::<Vec<_>>(),
                "rows": report.rows(),
                "groups": report.groups,
                "batchesWritten": report.batches_written,
                "batchesSkipped": report.batches_skipped,
                "objectsAttached": report.objects_attached,
                "objectsSkipped": report.objects_skipped,
                "peopleMapped": report.people_mapped,
                "profileRows": report.profile_rows,
            })],
            next_cursor: None,
        })
    }

    /// `std.space.delete`.
    pub async fn delete(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let confirm = bound
            .0
            .get("confirm")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let (_, schema) = self.deployed().await?;
        let sid = SpaceId(space.to_string());
        let now = (self.now)();

        // The blob store rides into `erase_space` rather than being swept
        // here afterwards, because **the ordering is the argument** and
        // that function is where the erasure's ordering lives: after the
        // confirmation and consumer gates, before `delete_rows` truncates
        // `$blobref` — which IS the list of objects to delete. See the
        // block comment at the deletion site for both halves.
        //
        // `None` is a deployment with no blob tier, and the answer says
        // which of the two that was: "there are none" and "we did not
        // look" are different facts that read identically if nobody states
        // one.
        let pool = self.space_pool(&sid).await?;
        let erased = erase::erase_space(
            &pool,
            &sid,
            &schema,
            self.keys.as_ref(),
            &self.policy,
            now,
            confirm,
            self.stores.as_deref(),
        )
        .await
        .map_err(erase_refusal)?;

        self.record(
            space,
            caller,
            DELETE,
            "space_erased",
            &format!("del_{space}_{now}"),
        );
        // D-S5: objects left in a bucket the tenant owns are recorded
        // beside the erasure itself, one entry per bucket. What it says is
        // Ordo's own act: these were not deleted, and the keys that open
        // them were destroyed. It never says the bytes are gone.
        for left in &erased.objects_left {
            self.record(
                space,
                caller,
                DELETE,
                &left.action(),
                &format!("del_{space}_{now}"),
            );
        }

        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "space": erased.space,
                "rowsDeleted": erased.total_deleted(),
                "deleted": erased.deleted.iter().map(|(t, n)| json!({
                    "table": t, "rows": n,
                })).collect::<Vec<_>>(),
                "retained": erased.retained,
                "logGroups": erased.disclosure.log_groups,
                "insideUndoHorizon": erased.disclosure.inside_undo_horizon,
                "sealedPayloads": erased.disclosure.sealed_payloads,
                "strandedMirrors": erased.disclosure.stranded_mirrors.iter()
                    .map(|c| c.name.clone()).collect::<Vec<_>>(),
                // Reported, not silent. An owner ending a space is owed the
                // count of files that went with it — and a deployment with
                // no blob tier reports `0` for a stated reason rather than
                // omitting the key, so a reader never has to decide whether
                // absence meant "none" or "not looked at".
                "objectsDeleted": erased.objects_deleted,
                // D-S5: files in a bucket the tenant owns are not deleted.
                // Reported with the same count, store and digest the custody
                // entry holds, so the app can hand it to its own user.
                "objectsLeft": erased.objects_left.iter().map(|l| json!({
                    "store": l.store,
                    "count": l.count,
                    "epochs": l.epochs,
                    "digest": l.digest,
                })).collect::<Vec<_>>(),
            })],
            next_cursor: None,
        })
    }
}

/// The engine's refusals, given their wire names.
fn export_refusal(e: ExportError) -> ExecError {
    let message = e.to_string();
    match e {
        // The class `LoadRefusal::SchemaDrift` never had. Same fact, first
        // producer that meets a caller.
        ExportError::Drift { .. } => ExecError::new(ErrorCode::SchemaDrift, message),
        // Custody could not open what the platform holds. The same fact
        // `sealed_open_failed` already names, reached from the other side.
        ExportError::SealedUnopened { .. } | ExportError::KeyRelease(_) => {
            ExecError::new(ErrorCode::SealedOpenFailed, message)
        }
        ExportError::Store(e) => ExecError::of_fault(e.class(), ErrorCode::Internal, message),
        ExportError::Bundle(_) => ExecError::internal(message),
        _ => ExecError::internal(message),
    }
}

/// Every import refusal onto the wire, with its class kept as the first word
/// of the message.
///
/// **One code for fourteen classes**, argued at [`ErrorCode::ImportRefused`]:
/// the taxonomy is `ordo_exec::import`'s to extend, `plan-import.md` §7's
/// deferred work extends it, and a wire class per refusal would make each
/// increment an amendment to an inventory two tests enforce. The `Display`
/// impl already leads with the class, so the mapping here is a status
/// decision and nothing else.
///
/// **Two are deliberately other codes**, and both are cases where a class
/// already names the same fact from another direction:
///
/// - a ciphertext bundle is `sealed_unsupported`, which is what the door
///   says about every other sealed value it cannot open;
/// - a bundle the destination's own blob door refused keeps **the door's**
///   class, exactly as `row_refused` keeps the write protocol's, because the
///   operator's next act is decided by the door's answer and not by the fact
///   that an import was what asked.
fn import_refusal(e: ordo_exec::import::ImportError) -> ExecError {
    use ordo_exec::import::ImportError as E;
    let message = e.to_string();
    match e {
        E::SealedCiphertext => ExecError::new(ErrorCode::SealedUnsupported, message),
        // The door already answered in the vocabulary an operator acts on.
        // `blob_refused: <class>: …` would bury it one level down.
        E::BlobRefused { .. } => ExecError::new(ErrorCode::AccessGuard, message),
        // Neither is the caller's to repair, and an operator has to be able
        // to tell a database or a store fault from a bundle defect.
        E::Store(e) => ExecError::of_fault(e.class(), ErrorCode::Internal, message),
        E::Bundle(_) => ExecError::internal(message),
        E::FormatUnknown { .. }
        | E::BundleIncomplete { .. }
        | E::ArtifactMismatch { .. }
        | E::FieldMismatch { .. }
        | E::SpaceNotEmpty { .. }
        | E::CyclicEntities { .. }
        | E::TreeCycle { .. }
        | E::UnresolvedRef { .. }
        | E::RowRefused { .. }
        | E::Unencodable { .. }
        | E::BlobDoorMissing { .. }
        | E::BlobUnplaced { .. }
        | E::BlobTierUnsupported { .. }
        // The people map's three, and they are `import_refused` for the
        // same reason as the rest: each names something outside the request
        // that an operator repairs before retrying the identical call. Adding
        // a member to the destination, correcting the map, or choosing a new
        // import name are all acts on the deployment rather than edits to a
        // parameter.
        | E::PeopleUnknown { .. }
        | E::PeopleAmbiguous { .. }
        | E::PeopleChanged { .. }
        // A profile that cannot be restored faithfully: an op the program no
        // longer has, a member the map cannot place, or a destination that
        // already has a profile. Each is repaired outside the request.
        | E::ProfileRefused { .. } => ExecError::new(ErrorCode::ImportRefused, message),
        _ => ExecError::internal(message),
    }
}

fn erase_refusal(e: EraseRefusal) -> ExecError {
    let message = e.to_string();
    match e {
        EraseRefusal::NotConfirmed { .. } => ExecError::new(ErrorCode::NotConfirmed, message),
        // `invalid_param`, not a new class, and not `not_found`. The
        // request named a space this route does not serve, which is a
        // property of the request, and that is the same argument the blob
        // door makes for refusing a tier by name. Not `not_found`,
        // because a system space is not hidden: every tenant reads it, so
        // pretending it does not exist would be a lie the caller can
        // already disprove.
        EraseRefusal::SystemSpace { .. } => ExecError::new(ErrorCode::InvalidParam, message),
        EraseRefusal::ConsumerPinned { .. } => ExecError::new(ErrorCode::SpacePinned, message),
        // Custody refused, so nothing was deleted. 500 with its own name
        // for `sealed_open_failed`'s reason: the caller can do nothing
        // about it, and an operator must be able to tell a custody fault
        // from an unknown panic.
        EraseRefusal::Custody(_) => ExecError::new(ErrorCode::SealedOpenFailed, message),
        EraseRefusal::Store(e) => ExecError::of_fault(e.class(), ErrorCode::Internal, message),
        _ => ExecError::internal(message),
    }
}
