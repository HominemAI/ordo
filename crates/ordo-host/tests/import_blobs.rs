//! **A space with an attachment leaves and comes back.**
//!
//! `plan-import.md` §0 and `licensing.md` make one argument in two legs: a
//! customer who can get their data out and cannot get it in has half a
//! promise. Until this file, blobs were the half of the half: export carried
//! the bytes at `blobs/<entity>/<field>/<id>` (17g) and import refused them
//! in three places, so a space holding a single attachment could be exported
//! and could not be loaded.
//!
//! # Why the round trip lives here and not beside the importer
//!
//! There is **one upload path** (`docs/plan-blob-chain.md` C1–C8, asserted
//! over this workspace's own source by `tests/upload_seam.rs`): bytes become
//! a `Stored` in `ordo-blob`, a `Stored` becomes a `PinnedUpload` at one mint
//! site in `ordo_host::blob`, and the write protocol converges on that token
//! and nothing else. `ordo_exec::import` is three crates below the door, so
//! it drives the door through a trait (`import::BlobDoor`) rather than
//! growing a second way to write a blob column. Which means the real round
//! trip — the real sealer, the real store, the real converge, the real chain
//! — can only be assembled where the real door is, which is here.
//!
//! `crates/ordo-exec/tests/import.rs` covers the other half: which bytes
//! import puts on which row, and every refusal it makes before a byte moves.
//!
//! # What this file will not let pass
//!
//! A round trip is the easiest test in the world to write so that it compares
//! nothing: an empty object set, a field the export skipped, a digest
//! computed twice from the same buffer. So each assertion below is paired
//! with the thing that would make it vacuous. The bundle is asserted to
//! actually carry the member. The destination's handle is asserted to
//! **differ** from the source's, because an import that copied the handle
//! would return the same bytes by reading the source's object. The digest is
//! opened from the destination's own `$blobref` row under the
//! **destination's** key and compared against a digest taken from the bytes
//! the test itself wrote, never against the source's envelope.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::path::PathBuf;
use std::sync::Arc;

use ordo_api::blob::{BlobBody, BlobPort, BlobRequest, BlobVerb};
use ordo_blob::{BlobPipeline, BlobPolicy, BlobStore, FsStore};
use ordo_commandlog::store::Store;
use ordo_conformance::{Fixture, Pg};
use ordo_crypto::{Purpose, SpaceId};
use ordo_exec::export::{export_space, ExportRequest, MemoryBundle, SealedMode};
use ordo_exec::import::{import_space, ImportRequest, MemorySource};
use ordo_exec::registry::Artifact;
use ordo_exec::{Executor, Pool, PoolConfig};
use ordo_host::blob::PlatformBlobs;
use ordo_host::deploy::Deployment;

/// The source: the deployment's own space, which the fixture's custody
/// covers.
const HERE: &str = ordo_host::DEFAULT_SPACE;
/// The destination, provisioned through the one entry point that mints
/// custody. Its keys are **different keys**, which is what makes a digest
/// that matches evidence of a re-seal rather than of a copy.
const TWIN: &str = "spc_import_twin";
/// `TWIN`'s founding owner, and the principal the import runs as.
const IMPORTER: &str = "mem_twin";
/// Who uploads into the source space. Any principal will do: the entity
/// below declares no guard, so what is under test is the blob leg rather
/// than the access story `tests/blob_door.rs` covers.
const AUTHOR: &str = "mem_author";

/// One entity, one external blob field, no guard and **no reference to
/// `members`**.
///
/// The missing ref is deliberate and is a finding rather than a
/// convenience: `members` is a platform entity, import skips platform
/// entities (a bundle that could create access rows would be a bundle that
/// grants itself membership), and nothing maps their ids — so an entity with
/// an `owner -> members` column cannot be imported at all today. That gap is
/// outside the blob leg and is reported rather than worked around here.
const PROGRAM: &str = "\
entity files (key: fil) {
  title: text
  scan:  blob?
}
";

const CONTRACT: &[u8] = b"a signed contract, sealed under one space's key and then another's";
const MIME: &str = "application/pdf";

struct Harness {
    pool: Pool,
    fixture: Arc<Fixture>,
    door: Arc<PlatformBlobs>,
    spaces: Arc<ordo_host::spaces::Spaces>,
    artifact: Artifact,
    dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

async fn harness(db: &str) -> Harness {
    let f = Arc::new(Fixture::from_source(PROGRAM, "").expect("the program compiles"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("the schema applies");

    // `Pool::connect` runs `ensure_platform_tables`, which is what creates
    // `$blobref` and `$blobtomb`; `Fixture::reset`'s DDL is schema-only.
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");

    let deployment = Deployment::pinned(
        "import-blobs-test",
        f.clone(),
        Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );

    // **The destination, provisioned before anything is exported.** Custody
    // and a founding owner, through `ordo_host::provision` rather than
    // assembled by hand, so the space the bundle lands in is a space the
    // resolver serves for the ordinary reason.
    ordo_host::provision::provision_space(
        &pool,
        f.keys.as_ref(),
        TWIN,
        &ordo_host::provision::FoundingOwner {
            member_id: IMPORTER,
            principal: IMPORTER,
            display_name: "The destination's owner",
        },
    )
    .await
    .expect("the destination provisions");

    let dir = std::env::temp_dir().join(format!("ordo-import-blobs-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    let pipeline = BlobPipeline::new(
        Arc::new(ordo_blob::SingleStore::new(store)),
        f.keys.clone(),
        BlobPolicy::DEFAULT,
    );
    // One resolver, shared by the door and by everything that reads a
    // space's chain back: two would be two chains for one tenant.
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let door = Arc::new(PlatformBlobs {
        spaces: spaces.clone(),
        pipeline: pipeline.clone(),
    });

    let artifact = Artifact {
        version: "v_files".into(),
        artifact: "{\n  \"stand-in\": \"the deployed artifact document\"\n}\n".into(),
        source: PROGRAM.to_string(),
        // The fixture's own rendering, which is what `reset` applied, so the
        // export's drift check compares the database against the DDL that
        // built it.
        ddl: f.ddl.clone(),
        applied_at: ordo_commandlog::Timestamp(0),
        actor: "test".into(),
    };

    Harness {
        pool,
        fixture: f,
        door,
        spaces,
        artifact,
        dir,
    }
}

impl Harness {
    fn request(&self, space: &str, principal: &str, row: &str, correlation: &str) -> BlobRequest {
        BlobRequest {
            space: space.to_string(),
            principal: principal.to_string(),
            agent: None,
            verb: BlobVerb::Write,
            entity: Some("files".into()),
            field: Some("scan".into()),
            row_id: Some(row.to_string()),
            handle_hex: None,
            correlation: correlation.to_string(),
        }
    }

    /// One cell, as text.
    async fn one(&self, sql: &str) -> Option<String> {
        self.pool
            .rows(sql)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next())
            .flatten()
    }

    async fn scan_of(&self, space: &str, row: &str) -> Option<String> {
        self.one(&format!(
            "SELECT \"scan\" FROM \"files\" WHERE \"$space\" = '{space}' AND \"id\" = '{row}'"
        ))
        .await
    }

    async fn rev_of(&self, space: &str, row: &str) -> Option<String> {
        self.one(&format!(
            "SELECT \"rev\" FROM \"files\" WHERE \"$space\" = '{space}' AND \"id\" = '{row}'"
        ))
        .await
    }

    /// The plaintext integrity digest on a space's own reference row, opened
    /// under **that space's** key, as lowercase hex.
    ///
    /// This is the digest `plan-blob.md` calls the sealed integrity hash:
    /// `SHA-256` over the plaintext, sealed and AAD-bound to `(entity,
    /// field)`. It is a property of the bytes, so two spaces holding the same
    /// file agree on it while holding envelopes that cannot be compared.
    async fn integrity(&self, space: &str, handle_hex: &str) -> String {
        let space = SpaceId(space.to_string());
        let handle = hex16(handle_hex);
        let reference = ordo_exec::blob::lookup(&self.pool, &space, &handle)
            .await
            .expect("the reference row reads")
            .expect("the handle names a reference row");
        let lease = self
            .fixture
            .keys
            .release(&space, Purpose::Read, "cg_test_integrity")
            .expect("a read release");
        let opened = self
            .fixture
            .keys
            .open_at(
                &lease,
                &reference.entity,
                &reference.field,
                ordo_crypto::Anchor::Object(handle_hex),
                &reference.integrity,
            )
            .expect("the integrity envelope opens under this space's key");
        ordo_crypto::hex(&opened)
    }

    /// Every command group on a space's chain, oldest first, read back off
    /// the resolver's own log for that space.
    fn groups(&self, space: &str) -> Vec<ordo_commandlog::CommandGroup> {
        let log = self.spaces.log_of(space);
        let log = log.lock().expect("the log");
        // `len_in`, never `len`: the bare one is the conformance space's
        // chain length, and this space is not that one.
        let id = ordo_commandlog::SpaceId(space.to_string());
        (0..log.len_in(&id))
            .map(|i| {
                log.store
                    .get(&ordo_commandlog::SpaceId(space.to_string()), i)
                    .expect("the group")
                    .group
            })
            .collect()
    }

    /// The correlations in a space's **durable** chain, in chain order.
    async fn chain_correlations(&self, space: &str) -> Vec<String> {
        self.pool
            .rows(&format!(
                "SELECT \"correlation\" FROM \"$commandlog\" WHERE \"space\" = '{space}' \
                 ORDER BY \"idx\""
            ))
            .await
            .expect("the chain reads")
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect()
    }
}

fn hex16(hex: &str) -> [u8; 16] {
    assert_eq!(hex.len(), 32, "a handle is sixteen bytes: `{hex}`");
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex");
    }
    out
}

fn body(bytes: &[u8]) -> BlobBody {
    let bytes = bytes.to_vec();
    Box::pin(futures::stream::once(async move {
        Ok::<Vec<u8>, String>(bytes)
    }))
}

/// **The whole of the blob leg, end to end.**
#[tokio::test(flavor = "multi_thread")]
async fn a_space_with_an_attachment_round_trips_into_an_empty_one() {
    let h = harness("ordo_host_import_blobs").await;

    // ── the source: one row, one real upload through the real door ───────
    h.pool
        .rows(&format!(
            "INSERT INTO \"files\" (\"$space\", \"id\", \"rev\", \"title\") \
             VALUES ('{HERE}', 'fil_1', 1, 'a contract')"
        ))
        .await
        .expect("the row the file hangs on");
    h.door
        .write(
            &h.request(HERE, AUTHOR, "fil_1", "cg_test_source_upload"),
            CONTRACT.len() as u64,
            MIME,
            body(CONTRACT),
        )
        .await
        .expect("the source's own upload");
    let source_handle = h
        .scan_of(HERE, "fil_1")
        .await
        .expect("the upload set the column");

    // ── the bundle ──────────────────────────────────────────────────────
    let mut bundle = MemoryBundle::new();
    export_space(
        &h.pool,
        &ExportRequest {
            space: &SpaceId(HERE.to_string()),
            schema: &h.fixture.schema,
            artifact: &h.artifact,
            keys: h.fixture.keys.as_ref(),
            blobs: Some(&h.door.pipeline),
            sealed: SealedMode::Open,
            correlation: "cg_test_export",
        },
        &mut bundle,
    )
    .await
    .expect("the source exports");

    // **The bundle carries the object.** Asserted before anything is
    // imported, because every assertion below would pass over a bundle with
    // no objects in it at all.
    assert_eq!(
        bundle.get_bytes("blobs/files/scan/fil_1"),
        Some(CONTRACT),
        "the member holds the plaintext, opened under the source's key: {:?}",
        bundle.names()
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(bundle.get_bytes("manifest.json").expect("the manifest")).unwrap();
    assert_eq!(manifest["blobs"]["objects"], 1);
    assert_eq!(manifest["blobs"]["files"][0]["mime"], MIME);
    assert_eq!(manifest["unbuilt"], serde_json::json!([]));

    // ── the import, into the provisioned twin ───────────────────────────
    let exec = Executor {
        pool: h.pool.clone(),
        schema: h.fixture.schema.clone(),
        registry: h.fixture.registry.clone(),
        keys: h.fixture.keys.clone(),
        space: SpaceId(TWIN.to_string()),
    };
    let request = ImportRequest {
        // This fixture declares no ref to `members`, which is what let it be
        // written before the people map existed.
        people: &[],
        name: "the-migration",
        fixture: Arc::new(h.fixture.in_space(TWIN)),
        artifact: &h.artifact,
        caller: IMPORTER,
        batch: 8,
        blobs: Some(h.door.as_ref()),
    };
    let source = MemorySource::from(&bundle);
    let report = import_space(&exec, &request, &source)
        .await
        .expect("the bundle imports");

    assert_eq!(report.rows(), 1, "the row landed");
    assert_eq!(
        (report.objects_attached, report.objects_skipped),
        (1, 0),
        "one object, attached once: {report:?}"
    );

    // The id the destination minted, from `$importmap` rather than guessed.
    let minted = h
        .one(&format!(
            "SELECT \"row_id\" FROM \"$importmap\" WHERE \"$space\" = '{TWIN}' \
             AND \"import\" = 'the-migration' AND \"entity\" = 'files' \
             AND \"source_id\" = 'fil_1'"
        ))
        .await
        .expect("the row was mapped");
    assert_ne!(minted, "fil_1", "the destination mints its own ids");

    // ── a fresh object, not a copied handle ─────────────────────────────
    let dest_handle = h
        .scan_of(TWIN, &minted)
        .await
        .expect("the destination's column names an object");
    assert_ne!(
        dest_handle, source_handle,
        "an import that copied the handle would read the SOURCE's object back and every \
         assertion below would pass while nothing had been sealed under this space's key"
    );

    // ── the bytes, read back through the door ───────────────────────────
    //
    // The whole path: the destination's `$blobref` row, its own epoch key,
    // its own object in the store, decrypted on the way out.
    let read = h
        .door
        .read(
            &BlobRequest {
                space: TWIN.to_string(),
                principal: IMPORTER.to_string(),
                agent: None,
                verb: BlobVerb::Read,
                entity: None,
                field: None,
                row_id: None,
                handle_hex: Some(dest_handle.clone()),
                correlation: "cg_test_read_back".into(),
            },
            0,
            None,
        )
        .await
        .expect("the destination serves the object");
    assert_eq!(read.bytes, CONTRACT, "the same bytes came back");
    assert_eq!(
        read.mime, MIME,
        "and the same mime, which is nowhere in the bundle but the manifest"
    );
    assert_eq!(read.plain_len, CONTRACT.len() as u64);

    // ── the same digest ─────────────────────────────────────────────────
    //
    // Taken three ways that must agree, and one that must not be the source
    // of either of the others: `SHA-256` of the bytes this test wrote, the
    // digest on the destination's reference row opened under the
    // **destination's** key, and the same on the source's under the
    // source's.
    let expected = ordo_exec::export::bundle_digest(CONTRACT);
    let destination_digest = h.integrity(TWIN, &dest_handle).await;
    let source_digest = h.integrity(HERE, &source_handle).await;
    assert_eq!(
        destination_digest, expected,
        "the destination's sealed integrity digest opens to SHA-256 over the file"
    );
    assert_eq!(
        source_digest, destination_digest,
        "and it is the same digest the source recorded, under a different key: the envelopes \
         cannot be equal (a fresh nonce each) and the plaintext digests must be"
    );

    // ── the chain carries the digest ────────────────────────────────────
    let groups = h.groups(TWIN);
    let converges: Vec<&ordo_commandlog::CommandGroup> = groups
        .iter()
        .filter(|g| {
            g.commands
                .iter()
                .any(|c| c.kind == ordo_commandlog::CommandKind::Converge)
        })
        .collect();
    assert_eq!(
        converges.len(),
        1,
        "one attach, one converge group: {groups:?}"
    );
    let command = &converges[0].commands[0];
    assert_eq!(command.entity, "files");
    assert_eq!(
        command.row_id, minted,
        "the converge lands on the row the import minted"
    );
    let after = command
        .after
        .as_ref()
        .expect("a converge images the row after");
    assert_eq!(
        after.get("scan"),
        Some(&ordo_commandlog::Value::Text(dest_handle.clone())),
        "the after-image names the object that was stored"
    );
    assert!(
        matches!(
            after.get("scan$digest"),
            Some(ordo_commandlog::Value::SealedRef(_))
        ),
        "the digest rides the image as a `SealedRef`, never as the plaintext digest: {after:?}"
    );
    assert!(
        h.chain_correlations(TWIN)
            .await
            .contains(&converges[0].correlation.0),
        "and the group is in the DURABLE chain, not only in this process's log"
    );

    // ── and re-running it attaches nothing a second time ────────────────
    let again = import_space(&exec, &request, &MemorySource::from(&bundle))
        .await
        .expect("the same import re-runs");
    assert_eq!(
        (again.objects_attached, again.objects_skipped),
        (0, 1),
        "a resumed import must not upload a second object for a file it already attached"
    );
    assert_eq!(
        h.scan_of(TWIN, &minted).await,
        Some(dest_handle),
        "the column still names the first object"
    );
    assert_eq!(
        h.rev_of(TWIN, &minted).await,
        Some("2".to_string()),
        "one create and one attach are two versions, and the second run is neither"
    );
}
