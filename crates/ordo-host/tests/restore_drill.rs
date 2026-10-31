//! **`implementation.md`'s five-step disaster-recovery drill, executable**
//! (`plan-production-ops.md` R3).
//!
//! In `ordo-host` because step 2 exercises the boot assertion, and the marker
//! is written through `undo::record_restore`, the library half of R2c, since
//! `ordo-host` does not depend on `ordo-cli` outside its tests.
//!
//! # The rewind mechanism, ruled
//!
//! A cargo test cannot run `pg_basebackup` and a WAL restore against the
//! shared cluster the suites use. The drill takes its backup as a Postgres
//! **template copy** — `CREATE DATABASE "<db>_backup" TEMPLATE "<db>"`, with
//! the pool dropped first, because a template copy needs no other connection
//! to the source — and restores by dropping the original and copying the
//! backup back under its name. That is a real point-in-time copy of a whole
//! database, which is the property the assertions need. PITR proper is the
//! deployment's instrument and the howto's procedure; this proves the
//! platform's half runs end to end.
//!
//! # Why the space is attached, and what that changes
//!
//! Amended 2026-09-19 (`restore-witness`). In a single database the template
//! copy would restore `$chainwitness` with the chain, step 2 would read
//! `Intact`, and the drill would prove nothing about the boot assertion. So
//! the space is provisioned onto an **attached target**, the copy is of that
//! target, and the control database is left alone. Step 2's refusal then
//! comes from the witness, which the drill asserts.
//!
//! **Plan-vs-build note.** The plan asks step 2 to call `assemble`. It
//! cannot, for a reason the plan itself records: the host binary does not
//! construct an `AttachedPools`, so `assemble` boots with the control pool
//! and cannot route to an attached space — it would read that space's chain
//! in the wrong database and refuse for the wrong reason. The boot site that
//! covers an attached space is `Spaces::resolve`, and that is the one the
//! drill exercises. Since 17aa that is the *only* site the question is
//! answered at: `assemble` no longer refuses a boot over one space's
//! history, it reconciles and quarantines, which `runtime.rs`'s
//! `a_rewound_space_is_quarantined_and_the_door_stays_up` proves on a
//! single database.
//!
//! # The wall time is not a gate
//!
//! Step 1 prints how long the copy and the restore took, and says in the
//! line that it is not a gate. The same discipline `plan-production-ops.md`
//! 5f states for the other two measurements: a laptop cluster's number is
//! not a release criterion, and recording it as one would make the first
//! slow morning look like a regression. The test proves the drill *runs*; a
//! deployment produces the number.

use ordo_host::provision::HostAllowlist;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ordo_commandlog::{
    Actor, ChainHash, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Ed25519Signer,
    Image, RestoreSource, SpaceId as LogSpace, Timestamp, Value,
};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::export::{export_space, ExportRequest, MemoryBundle, SealedMode};
use ordo_exec::route::{AttachedPools, Installed, PoolResolver};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::undo::{self, Anchor, Checkpointed, Db, RecordSource, RestoreRecord};
use ordo_exec::witness::Witness;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::provision::{prepare_target, Target, SYSTEM_ACTOR};
use ordo_host::spaces::{SpaceRefusal, Spaces};

const SPACE: &str = "spc_drill";
const NOW: u64 = 1_700_000_000;
/// Groups written before the backup. Also the marker's index after the
/// restore, which is what step 2 pins.
const N: usize = 4;
/// Groups written after the backup, which the restore must discard.
const M: usize = 3;

fn admin() -> Pg {
    Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
}

fn drop_database(name: &str) {
    let _ = admin().exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
}

async fn connect(db: &str) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .unwrap_or_else(|e| panic!("{db}: {e}"))
}

fn attachment(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.into(),
        credential: "local".into(),
        custody: DbCustody::PlatformManaged,
        attached_at: NOW as i64,
        attached_by: SYSTEM_ACTOR.into(),
    }
}

fn group(n: usize) -> CommandGroup {
    let mut after = Image::new();
    after.insert("id".into(), Value::Id(format!("ven_{n}")));
    after.insert("name".into(), Value::Text(format!("vendor {n}")));
    CommandGroup {
        correlation: CorrelationId(format!("drill_cg_{n}")),
        actor: Actor::User("usr_drill".into()),
        commands: vec![Command {
            id: CommandId(format!("drill_cmd_{n}")),
            space: LogSpace(SPACE.into()),
            entity: "vendors".into(),
            row_id: format!("ven_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(after),
            at: Timestamp(1_000 + n as u64),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

/// One group, and the row it describes, so step 4 has a row set to compare.
async fn write(pool: &Pool, n: usize) {
    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(n)).await.expect("append");
    tx.commit().await.unwrap();
    pool.rows(&format!(
        "INSERT INTO \"vendors\" (\"$space\", \"id\", \"rev\", \"name\", \"region\", \"active\") \
         VALUES ('{SPACE}', 'ven_{n}', 1, 'vendor {n}', 'eu', true)"
    ))
    .await
    .expect("the row lands");
}

async fn vendors(pool: &Pool) -> Vec<String> {
    pool.rows(&format!(
        "SELECT \"id\" FROM \"vendors\" WHERE \"$space\" = '{SPACE}' ORDER BY \"id\""
    ))
    .await
    .unwrap()
    .iter()
    .map(|r| r[0].clone().unwrap())
    .collect()
}

async fn chain_len(pool: &Pool) -> i64 {
    pool.rows(&format!(
        "SELECT count(*) FROM \"$commandlog\" WHERE \"space\" = '{SPACE}'"
    ))
    .await
    .unwrap()[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

async fn sealed_rows(pool: &Pool) -> i64 {
    pool.rows("SELECT count(*) FROM \"$sealedpayload\"")
        .await
        .map(|r| r[0][0].as_deref().unwrap_or("0").parse().unwrap_or(0))
        .unwrap_or(0)
}

fn signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("drill-key", &[93u8; 32])
}

/// What one run of the drill's setup leaves behind.
struct Drilled {
    control: Pool,
    target: Pool,
    witness: Witness,
    spaces: Arc<Spaces>,
    fixture: Arc<Fixture>,
    /// The `(name, sha256)` map the manifest published, and the bundle it
    /// describes.
    bundle: MemoryBundle,
    manifest: String,
    manifest_sha256: String,
    /// The head signed **after** the backup, which the restore discards.
    /// Step 5's receiver document is cut over this.
    discarded: (u64, ChainHash),
    /// The row set as it stood when the backup was taken.
    snapshot: Vec<String>,
    control_db: String,
    target_db: String,
}

/// Steps 1 and the setup halves of the rest, parameterised by the database
/// names so the four tests do not share a cluster database.
async fn drill_to_the_restore(tag: &str) -> Drilled {
    // `target_db`/`backup_db` never go through `Pg::named`, so they are
    // namespaced explicitly, once, here — `test_db_name` is deterministic
    // within this process, so the pre-clean drop below and every later use
    // agree on the same name. `control_db` goes through `Pg::named`
    // instead, which namespaces it internally; `control_pg.db` is read back
    // from that call rather than re-namespaced, so the two never disagree.
    let control_db = format!("ordo_drill_{tag}_control");
    let target_db = ordo_conformance::test_db_name(&format!("ordo_drill_{tag}_target"));
    let backup_db = format!("{target_db}_backup");
    drop_database(&target_db);
    drop_database(&backup_db);

    let f = Arc::new(Fixture::load().expect("fixture"));
    let control_pg = Pg::scoped(&control_db);
    f.reset(&control_pg).expect("seeded");
    let control = connect(&control_pg.db).await;
    spacedb::ensure_table(&control).await.expect("$spacedb");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    let witness = Witness::new(control.clone());
    witness.ensure_table().await.expect("$chainwitness");
    control
        .rows(&format!(
            "DELETE FROM \"$chainwitness\" WHERE \"space\" = '{SPACE}'"
        ))
        .await
        .unwrap();

    let credentials = Installed::new().as_configured("local");
    let template = PoolConfig {
        size: 2,
        ..PoolConfig::from_env()
    };
    // R-S1: `prepare_target` reads the schema from the deployment that is
    // serving, so this test stands one up rather than handing it a DDL.
    let serving = ordo_host::deploy::Deployment::pinned(
        "restore-drill-v1",
        f.clone(),
        ordo_exec::Executor {
            pool: control.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    prepare_target(
        &control,
        f.keys.as_ref(),
        &serving,
        SPACE,
        &Target {
            attachment: &attachment(&target_db),
            credentials: &credentials,
            template: &template,
            hosts: &HostAllowlist::of(&PoolConfig::from_env().host),
        },
        None,
        NOW,
    )
    .await
    .expect("the target is prepared");

    let space = LogSpace(SPACE.into());
    let target = connect(&target_db).await;

    // ── step 1 ──────────────────────────────────────────────────────────
    for n in 0..N {
        write(&target, n).await;
    }
    undo::sign_checkpoint(&target, &space, &signer(), Timestamp(400), &witness)
        .await
        .expect("the checkpoint over the history the backup will hold");
    let snapshot = vendors(&target).await;

    // The bundle, with R1a's digest in hand.
    let artifact = ordo_exec::registry::Artifact {
        version: "drill".into(),
        artifact: "{}\n".into(),
        source: String::new(),
        ddl: f.ddl.clone(),
        applied_at: Timestamp(0),
        actor: "drill".into(),
    };
    let mut bundle = MemoryBundle::new();
    let report = export_space(
        &target,
        &ExportRequest {
            space: &ordo_crypto::SpaceId(SPACE.into()),
            schema: &f.schema,
            artifact: &artifact,
            keys: f.keys.as_ref(),
            blobs: None,
            sealed: SealedMode::Open,
            correlation: "drill_export",
        },
        &mut bundle,
    )
    .await
    .expect("the export runs against the target");
    let manifest = bundle.get("manifest.json").expect("a manifest").to_string();
    assert_eq!(
        report.manifest_sha256,
        ordo_exec::export::bundle_digest(manifest.as_bytes()),
        "R1a: the report's digest is the manifest's own"
    );

    // The backup.
    let started = Instant::now();
    drop(target);
    tokio::time::sleep(Duration::from_millis(200)).await;
    admin()
        .exec(&format!("CREATE DATABASE {backup_db} TEMPLATE {target_db}"))
        .expect("the template copy");
    let copied = started.elapsed();

    // M more groups, and a second checkpoint. This is the history the
    // restore discards, and the head a receiver was sent.
    let target = connect(&target_db).await;
    for n in N..N + M {
        write(&target, n).await;
    }
    let Checkpointed::Signed(discarded_cp) =
        undo::sign_checkpoint(&target, &space, &signer(), Timestamp(800), &witness)
            .await
            .expect("the checkpoint the restore will discard")
    else {
        panic!("expected a fresh signature")
    };
    assert_eq!(discarded_cp.upto as usize, N + M);

    // The restore.
    let restore_started = Instant::now();
    drop(target);
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop_database(&target_db);
    admin()
        .exec(&format!("CREATE DATABASE {target_db} TEMPLATE {backup_db}"))
        .expect("the restore");
    let restored = restore_started.elapsed();
    let target = connect(&target_db).await;

    eprintln!(
        "drill wall time (NOT A GATE — a laptop cluster's number, never a release criterion): \
         backup {copied:?}, restore {restored:?}"
    );

    let pools: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        template.clone(),
        Arc::new(credentials),
    ));
    let deployment = ordo_host::deploy::Deployment::empty(
        ordo_crypto::SpaceId(SPACE.into()),
        control.clone(),
        f.keys.clone(),
    )
    .expect("an empty deployment");
    let spaces = Spaces::routed(deployment, pools, witness.clone());

    drop_database(&backup_db);
    Drilled {
        control,
        target,
        witness,
        spaces,
        fixture: f,
        bundle,
        manifest,
        manifest_sha256: report.manifest_sha256,
        discarded: (discarded_cp.upto, discarded_cp.head),
        snapshot,
        control_db: control_pg.db.clone(),
        target_db,
    }
}

/// Record the restore the way an operator does.
async fn record(d: &Drilled, again: bool) -> undo::Recorded {
    undo::record_restore(
        &d.target,
        &d.witness,
        &LogSpace(SPACE.into()),
        RestoreRecord {
            operator: "ops@example.test".into(),
            source: RestoreSource::Bundle {
                manifest_sha256: {
                    let mut out = [0u8; 32];
                    for (i, b) in out.iter_mut().enumerate() {
                        *b = u8::from_str_radix(&d.manifest_sha256[2 * i..2 * i + 2], 16).unwrap();
                    }
                    out
                },
            },
            pre_restore_head: None,
            again,
            restored_from: None,
        },
    )
    .await
    .expect("recorded")
}

// ── the drill ────────────────────────────────────────────────────────────

/// **All five steps.**
#[tokio::test(flavor = "multi_thread")]
async fn the_restore_drill_runs_all_five_steps() {
    let d = drill_to_the_restore("main").await;
    let space = LogSpace(SPACE.into());

    // ── step 2: the refusal, the record, and the verification ──────────
    let Err(refused) = d.spaces.resolve(SPACE).await else {
        panic!("a restored target was served");
    };
    let SpaceRefusal::Rewound { continuity, .. } = &refused else {
        panic!("expected Rewound, got {refused}")
    };
    assert!(
        matches!(
            continuity,
            undo::Continuity::Rewound {
                source: RecordSource::Witness,
                ..
            }
        ),
        "the refusal comes from the record the restore could not reach: {continuity:?}"
    );

    let r = record(&d, false).await;
    assert_eq!(r.idx, N, "the marker lands at the restored head plus one");
    assert!(r.witnessed);
    d.spaces
        .resolve(SPACE)
        .await
        .expect("a recorded restore is served");

    let db = Db::Pool(&d.target);
    let verifier = ordo_commandlog::CheckpointVerifier::from_public_hex(
        "drill-key",
        &signer().verifier().public_hex(),
    )
    .unwrap();
    let (anchor, hash) = undo::choose_anchor(db, &space, Some(&verifier))
        .await
        .expect("an anchor");
    let v = undo::verify(db, &space, anchor, hash)
        .await
        .expect("the restored chain verifies");
    assert_eq!(
        v.discontinuities.len(),
        1,
        "exactly one discontinuity, and nothing else"
    );
    assert_eq!(v.discontinuities[0].idx, N, "at exactly N");
    assert!(
        v.orphaned_checkpoints.is_empty(),
        "the discarded checkpoint went with the database it was in: {:?}",
        v.orphaned_checkpoints
    );
    let g = undo::verify(db, &space, Anchor::Genesis, ChainHash::GENESIS)
        .await
        .expect("and from genesis too");
    assert_eq!(g.discontinuities, v.discontinuities, "the two agree");

    // ── step 3: the restored schema is the shape the program says ──────
    // The same comparison `export_space` makes before it reads a row: a
    // shadow schema built from the program's DDL, compared against what the
    // restored database actually holds.
    const SHADOW: &str = "ordo_drill_shadow";
    d.target
        .build_shadow_in(
            SHADOW,
            &ordo_schema::ddl_for(&d.fixture.schema).expect("ddl"),
        )
        .await
        .expect("the shadow builds");
    let drift = d.target.live_drift(SHADOW, "public").await;
    let _ = d
        .target
        .rows(&format!("DROP SCHEMA IF EXISTS {SHADOW} CASCADE"))
        .await;
    assert!(
        drift.as_ref().map(|d| d.is_empty()).unwrap_or(false),
        "the restored database drifted from the program: {drift:?}"
    );

    // ── step 4: the rows are the pre-backup rows, exactly ──────────────
    assert_eq!(
        vendors(&d.target).await,
        d.snapshot,
        "one read per entity against a known row set"
    );
    assert_eq!(
        chain_len(&d.target).await,
        N as i64 + 1,
        "the M post-backup groups are gone, and the marker is the only addition"
    );
    for n in N..N + M {
        assert!(
            d.target
                .rows(&format!(
                    "SELECT 1 FROM \"$commandlog\" WHERE \"space\" = '{SPACE}' \
                     AND \"correlation\" = 'drill_cg_{n}'"
                ))
                .await
                .unwrap()
                .is_empty(),
            "group {n} was written after the backup and must be gone"
        );
    }
    assert_eq!(
        sealed_rows(&d.target).await,
        0,
        "the anti-join's other half: no payload outlived the groups that named it"
    );

    // ── step 5: the bundle stands alone ────────────────────────────────
    let manifest: serde_json::Value = serde_json::from_str(&d.manifest).expect("a manifest");
    let digests = manifest["digests"].as_array().expect("digests");
    assert!(!digests.is_empty());
    for entry in digests {
        let name = entry["member"].as_str().expect("member");
        assert_ne!(name, "manifest.json", "the manifest cannot digest itself");
        let body = d
            .bundle
            .get(name)
            .unwrap_or_else(|| panic!("the bundle holds {name}"));
        assert_eq!(
            sha256_hex(body.as_bytes()),
            entry["sha256"].as_str().expect("sha256"),
            "member {name} re-hashes to what the manifest published"
        );
        assert_eq!(
            body.len() as u64,
            entry["bytes"].as_u64().expect("bytes"),
            "member {name} is the length the manifest published"
        );
    }

    // And the exported chain's prefix is the prefix the restored chain
    // holds, link by link and below the marker.
    let exported = d
        .bundle
        .get("commandlog.jsonl")
        .expect("the exported chain");
    let mut checked = 0usize;
    for line in exported.lines().filter(|l| !l.trim().is_empty()) {
        let e: serde_json::Value = serde_json::from_str(line).expect("a chain line");
        let idx = e["idx"].as_u64().expect("idx") as usize;
        assert!(idx < N, "the bundle was cut before the restore point");
        let stored = d
            .target
            .rows(&format!(
                "SELECT encode(\"prev\", 'hex'), encode(\"hash\", 'hex') \
                 FROM \"$commandlog\" WHERE \"space\" = '{SPACE}' AND \"idx\" = {idx}"
            ))
            .await
            .unwrap();
        assert_eq!(
            (
                stored[0][0].as_deref().unwrap(),
                stored[0][1].as_deref().unwrap()
            ),
            (
                e["prev"].as_str().expect("prev"),
                e["hash"].as_str().expect("hash")
            ),
            "link {idx} in the bundle is the link the restored chain holds"
        );
        checked += 1;
    }
    assert_eq!(checked, N, "every link below the marker");

    // R2d-iv, over the head a receiver was sent before the restore: the
    // chain has moved away from it, and the marker the operator recorded
    // attests a pre-restore head at or above it, so it is accounted for.
    let verdicts = undo::classify_records(
        db,
        Some(&d.witness),
        &space,
        &[(d.discarded.0, d.discarded.1)],
    )
    .await
    .expect("classified");
    let receiver = verdicts
        .iter()
        .find(|v| matches!(v.origin, undo::RecordOrigin::Receiver { .. }))
        .expect("the receiver's record");
    assert_eq!(receiver.agreement, undo::Agreement::Orphaned);
    assert!(
        receiver.explained_by.is_some(),
        "the receiver's head is accounted for by the recorded restore"
    );
    assert!(
        verdicts
            .iter()
            .all(|v| v.agreement != undo::Agreement::Orphaned || v.explained_by.is_some()),
        "no record of any origin is left unexplained: {verdicts:?}"
    );

    let _ = (&d.control, &d.fixture, &d.control_db, &d.target_db);
}

// ── step 2 fails the build when the marker is wrong ─────────────────────

/// No marker: the space is not served, and that is the whole point.
#[tokio::test(flavor = "multi_thread")]
async fn the_drill_fails_when_the_marker_is_missing() {
    let d = drill_to_the_restore("missing").await;
    assert!(
        matches!(
            d.spaces.resolve(SPACE).await,
            Err(SpaceRefusal::Rewound { .. })
        ),
        "a restore nobody recorded must not be served"
    );
    let db = Db::Pool(&d.target);
    let v = undo::verify(
        db,
        &LogSpace(SPACE.into()),
        Anchor::Genesis,
        ChainHash::GENESIS,
    )
    .await
    .expect("the chain itself is intact; what is missing is the disclosure");
    assert!(
        v.discontinuities.is_empty(),
        "and step 2's claim of one discontinuity at N does not hold"
    );
}

/// Two markers for one restore: step 2's "exactly one discontinuity" is
/// false, and it is false loudly rather than by looking the same.
#[tokio::test(flavor = "multi_thread")]
async fn the_drill_fails_when_the_marker_is_doubled() {
    let d = drill_to_the_restore("doubled").await;
    record(&d, false).await;
    let second = record(&d, true).await;
    assert_eq!(second.idx, N + 1);

    let db = Db::Pool(&d.target);
    let space = LogSpace(SPACE.into());
    let verifier = ordo_commandlog::CheckpointVerifier::from_public_hex(
        "drill-key",
        &signer().verifier().public_hex(),
    )
    .unwrap();
    let (anchor, hash) = undo::choose_anchor(db, &space, Some(&verifier))
        .await
        .expect("an anchor");
    let v = undo::verify(db, &space, anchor, hash)
        .await
        .expect("verifies");
    assert_eq!(
        v.discontinuities.len(),
        2,
        "step 2 asserts exactly one; a doubled marker gives two"
    );
}

/// A marker at the wrong index: ordinary writes landed before the operator
/// recorded, so the marker is not at N. The chain is still lawful — that is
/// what `restored_from` and the gap are for — but step 2's claim that the
/// discontinuity is at exactly N is false.
#[tokio::test(flavor = "multi_thread")]
async fn the_drill_fails_when_the_marker_is_misplaced() {
    let d = drill_to_the_restore("misplaced").await;
    write(&d.target, 900).await; // an ordinary write at index N
    let r = record(&d, false).await;
    assert_eq!(r.idx, N + 1, "the marker landed above N");

    let db = Db::Pool(&d.target);
    let space = LogSpace(SPACE.into());
    let v = undo::verify(db, &space, Anchor::Genesis, ChainHash::GENESIS)
        .await
        .expect("verifies");
    assert_eq!(v.discontinuities.len(), 1);
    assert_ne!(
        v.discontinuities[0].idx, N,
        "step 2 asserts the discontinuity is at exactly N"
    );
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
