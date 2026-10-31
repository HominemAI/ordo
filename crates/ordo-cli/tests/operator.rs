//! 8i: the operator surface for revert, checkpoint and verify, against a
//! real Postgres and the **durable** store — never `InMemoryStore`, which
//! is the reference implementation and not the thing an operator runs.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`). Every
//! test owns its own database: they run in parallel against one cluster and
//! all of them write `$commandlog`, so a shared database would make one
//! test's chain another test's tamper.
//!
//! What each of the three has to prove to count as reachable:
//!
//! - **verify** finds a tamper somebody actually made in the database, and
//!   names the index and the kind; and it can start from a checkpoint
//!   rather than replaying from genesis.
//! - **checkpoint** signs the head with the real Ed25519 signer, and a
//!   receiver holding only the published public key can check it.
//! - **revert** round-trips a write — the row back to its prior value, and
//!   the revert itself in the chain as new history rather than as a hole.

use std::sync::{Arc, Mutex};
use std::time::Duration;

mod support;
use ordo_commandlog::{
    Actor, ChainHash, CheckpointVerifier, Command, CommandGroup, CommandId, CommandKind,
    CorrelationId, Ed25519Signer, Image, RestoreMarker, RestoreSource, SpaceId, TamperKind,
    Timestamp, Value,
};
use ordo_conformance::{Fixture, Pg, WriteLog};
use ordo_exec::undo::{self, Anchor, Checkpointed, Db};
use ordo_exec::witness::{Witness, WitnessKind};
use ordo_exec::{Executor, LoggedExecutor, Pool, PoolConfig};
use ordo_schema::Schema;
use support::operator;

// ── plumbing ─────────────────────────────────────────────────────────────

/// Serialises the one test that sets process environment against every
/// other in-process `operator::run`.
///
/// `operator::run` reads `ORDO_CHECKPOINT_KEY_ID` and friends as fallbacks
/// (`verifier`), and the environment is shared by every test thread in this
/// binary. The end-to-end test must set the signing pair in-process, so a
/// verify running beside it saw a key id with no public key and exited 2.
/// Tests take the read side through `run_op` and run in parallel with each
/// other; the end-to-end test takes the write side for as long as the pair
/// is set, and clears it before letting go.
static PROCESS_ENV: std::sync::RwLock<()> = std::sync::RwLock::new(());

fn run_op(cmd: &str, args: &[String]) -> operator::Outcome {
    let _shared = PROCESS_ENV.read().unwrap_or_else(|e| e.into_inner());
    operator::run(cmd, args)
}

/// Holds the write side and unsets the checkpoint signing pair on drop, so
/// a failing assertion cannot leave it behind for the tests that follow.
struct CheckpointEnv(#[allow(dead_code)] std::sync::RwLockWriteGuard<'static, ()>);

impl CheckpointEnv {
    fn set(key_hex: &str, key_id: &str) -> Self {
        let guard = PROCESS_ENV.write().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ORDO_CHECKPOINT_KEY", key_hex);
        std::env::set_var("ORDO_CHECKPOINT_KEY_ID", key_id);
        CheckpointEnv(guard)
    }
}

impl Drop for CheckpointEnv {
    fn drop(&mut self) {
        std::env::remove_var("ORDO_CHECKPOINT_KEY");
        std::env::remove_var("ORDO_CHECKPOINT_KEY_ID");
    }
}

/// A database of this test's own, with an empty chain.
async fn fresh(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();
    pool.rows("DELETE FROM \"$checkpoints\"").await.unwrap();
    // The control-plane record too, since `sign_checkpoint` writes one for
    // every checkpoint and these tests run their own witness over this same
    // pool. A row left by a previous run of the same test is a checkpoint
    // this chain never reached, which is exactly what `continuity` refuses.
    let _ = pool.rows("DROP VIEW IF EXISTS \"$chainwitness\"").await;
    Witness::new(pool.clone()).ensure_table().await.unwrap();
    pool.rows("DELETE FROM \"$chainwitness\"").await.unwrap();
    pool
}

/// A database seeded with the market fixture AND an empty chain: the shape
/// a revert needs, because a revert writes rows.
async fn seeded(db: &str) -> (Pool, Pg, Arc<Fixture>, Schema) {
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();
    pool.rows("DELETE FROM \"$checkpoints\"").await.unwrap();
    let schema = f.schema.clone();
    (pool, pg, Arc::new(f), schema)
}

/// What `ordo revert` itself passes: no key custody.
///
/// The binary has none to give — there is no `WrappedKeyStore` over
/// Postgres for a `LocalKeyProvider` to unwrap per-space keys out of — so
/// these tests drive the operator surface the way the operator meets it.
/// The sealed path with real custody is proved in `ordo-exec`, which is
/// where a caller that HAS a provider lives.
const NO_KEYS: Option<&dyn undo::KeyProvider> = None;

/// The space the write protocol stamps its commands with.
fn space() -> SpaceId {
    SpaceId("conformance".into())
}

fn logged(pool: &Pool, f: &Arc<Fixture>) -> LoggedExecutor {
    LoggedExecutor::new(
        Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        "conformance",
    )
}

/// The correlation id of the group at `idx`.
async fn correlation_at(pool: &Pool, space: &SpaceId, idx: usize) -> CorrelationId {
    let rows = pool
        .rows(&format!(
            "SELECT \"correlation\" FROM \"$commandlog\" WHERE \"space\" = '{}' AND \"idx\" = {idx}",
            space.0
        ))
        .await
        .unwrap();
    CorrelationId(rows[0][0].clone().expect("a group at this index"))
}

async fn chain_len(pool: &Pool, space: &SpaceId) -> i64 {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM \"$commandlog\" WHERE \"space\" = '{}'",
            space.0
        ))
        .await
        .unwrap();
    rows[0][0].as_deref().unwrap().parse().unwrap()
}

async fn one(pool: &Pool, sql: &str) -> Option<String> {
    pool.rows(sql).await.unwrap().first()?.first()?.clone()
}

fn img(fields: &[(&str, Value)]) -> Image {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// A synthetic single-command group, for the chain tests that need a chain
/// and not a schema.
fn group(n: usize) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(format!("cg_{n}")),
        actor: Actor::User(format!("usr_{n}")),
        commands: vec![Command {
            id: CommandId(format!("cmd_{n}")),
            space: space(),
            entity: "tasks".into(),
            row_id: format!("task_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(img(&[
                ("id", Value::Id(format!("task_{n}"))),
                ("rev", Value::Int(1)),
                ("title", Value::Text(format!("row {n}"))),
            ])),
            at: Timestamp(1_000 + n as u64),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

async fn append(pool: &Pool, g: &CommandGroup) {
    let tx = pool.begin().await.unwrap();
    tx.append_group(g).await.expect("append");
    tx.commit().await.unwrap();
}

/// The stored hash of the entry at `idx`.
async fn hash_at(pool: &Pool, idx: usize) -> ChainHash {
    let rows = pool
        .rows(&format!(
            "SELECT encode(\"hash\", 'hex') FROM \"$commandlog\" \
             WHERE \"space\" = 'conformance' AND \"idx\" = {idx}"
        ))
        .await
        .unwrap();
    let hex = rows[0][0].clone().expect("an entry at this index");
    let mut h = [0u8; 32];
    for (i, b) in h.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    ChainHash(h)
}

/// The rewind a restore performs, as `$commandlog` sees it: every entry at
/// or above `from` is gone. `$checkpoints` is left alone: these tests model
/// a rewind the checkpoint rows survived, so a row still names a length
/// the chain once reached. That is the case the anchor rule and the orphan
/// list exist for. (A whole-database PITR rewinds `$checkpoints` with the
/// chain, since both live in the space's database.)
async fn rewind(pool: &Pool, from: usize) {
    pool.rows(&format!(
        "DELETE FROM \"$commandlog\" WHERE \"space\" = 'conformance' AND \"idx\" >= {from}"
    ))
    .await
    .unwrap();
}

/// A restore marker recorded by hand, resuming from the entry at
/// `restored_from`: the group `ordo restore --record` (R2c) will build.
async fn marker(
    pool: &Pool,
    n: usize,
    restored_from: usize,
    pre_restore_head: Option<(u64, ChainHash)>,
) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(format!("restore_{n}")),
        actor: Actor::System("restore".into()),
        commands: vec![],
        owner_administration: false,
        restored_unverified: false,
        restore: Some(RestoreMarker {
            space: space(),
            restored_from: (restored_from as u64, hash_at(pool, restored_from).await),
            pre_restore_head,
            source: RestoreSource::Pitr {
                at: Timestamp(1_757_000_000 + n as u64),
            },
            operator: "ops@example.test".into(),
        }),
    }
}

/// Rewrite a stored group's body without touching its stored hash — the
/// shape a quiet edit by whoever holds the database actually takes.
async fn doctor_body(pool: &Pool, idx: usize, replacement: &CommandGroup) {
    let hex: String = ordo_commandlog::encode_group(replacement)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    pool.rows(&format!(
        "UPDATE \"$commandlog\" SET \"body\" = '\\x{hex}'::bytea \
         WHERE \"space\" = 'conformance' AND \"idx\" = {idx}"
    ))
    .await
    .unwrap();
}

// ── verify ───────────────────────────────────────────────────────────────

/// The claim `verify` has to earn: it finds a tamper somebody made in the
/// database, and it says **where** and **what kind**. A verification that
/// cannot say where is not useful — an operator's next move is to read the
/// entry, and "somewhere in your history" is not a place.
#[tokio::test(flavor = "multi_thread")]
async fn verify_names_the_first_bad_index_and_its_kind() {
    let pool = fresh("ordo_op_verify").await;
    for n in 0..5 {
        append(&pool, &group(n)).await;
    }
    let db = Db::Pool(&pool);

    let clean = undo::verify(db, &space(), Anchor::Genesis, ChainHash::GENESIS)
        .await
        .expect("an untouched chain verifies");
    assert_eq!(clean.checked, 5);
    assert_eq!(clean.head.map(|(i, _)| i), Some(4));

    // (1) Content edited, stored hash left alone.
    let mut doctored = group(3);
    doctored.commands[0].after = Some(img(&[
        ("id", Value::Id("task_3".into())),
        ("rev", Value::Int(1)),
        ("title", Value::Text("something else entirely".into())),
    ]));
    doctor_body(&pool, 3, &doctored).await;
    let e = undo::verify(db, &space(), Anchor::Genesis, ChainHash::GENESIS)
        .await
        .expect_err("a doctored image must not verify");
    let undo::VerifyFailure::Tamper(t) = e else {
        panic!("expected a tamper, got {e}")
    };
    assert_eq!(t.index, 3, "the index named is the one that was edited");
    assert_eq!(t.kind, TamperKind::HashMismatch);

    // (2) A body that is not a decodable group at all, EARLIER in the
    // chain. The first failure is the one reported, so this now masks the
    // hash mismatch above — and it is reported as `Unreadable`, because
    // "I could not check" and "it checks out" must not be one value.
    pool.rows(
        "UPDATE \"$commandlog\" SET \"body\" = '\\xdeadbeef'::bytea \
         WHERE \"space\" = 'conformance' AND \"idx\" = 1",
    )
    .await
    .unwrap();
    let e = undo::verify(db, &space(), Anchor::Genesis, ChainHash::GENESIS)
        .await
        .expect_err("an undecodable body must not verify");
    let undo::VerifyFailure::Tamper(t) = e else {
        panic!("expected a tamper, got {e}")
    };
    assert_eq!(
        (t.index, t.kind),
        (1, TamperKind::Unreadable),
        "the corrupt entry names itself, not the 512-entry window it sat in"
    );
}

/// A link broken by deletion: the row at index 2 is removed outright, so
/// index 3 points at a predecessor that is not there. The reported index is
/// the stored `idx` — a verification that counted surviving rows would say
/// "2" here and send the operator to the wrong entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_entry_breaks_the_link_at_the_stored_index() {
    let pool = fresh("ordo_op_hole").await;
    for n in 0..5 {
        append(&pool, &group(n)).await;
    }
    pool.rows("DELETE FROM \"$commandlog\" WHERE \"space\" = 'conformance' AND \"idx\" = 2")
        .await
        .unwrap();

    let e = undo::verify(
        Db::Pool(&pool),
        &space(),
        Anchor::Genesis,
        ChainHash::GENESIS,
    )
    .await
    .expect_err("a hole is not a chain");
    let undo::VerifyFailure::Tamper(t) = e else {
        panic!("expected a tamper, got {e}")
    };
    assert_eq!(
        t.index, 2,
        "the missing position is what the operator needs"
    );
}

/// Verification from a checkpoint: the point of a checkpoint is that
/// verification (and pruning) can start mid-chain rather than replaying
/// everything, so this asserts the *arithmetic of that* — indices past the
/// anchor still name positions in the full chain — and the honest cost:
/// the prefix is taken on the signature, so a tamper below the anchor is
/// invisible to an anchored run and visible to `--from-genesis`.
#[tokio::test(flavor = "multi_thread")]
async fn verification_starts_from_a_signed_checkpoint_and_indices_stay_absolute() {
    let pool = fresh("ordo_op_anchor").await;
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    let signer = Ed25519Signer::from_seed("chk-op", &[21u8; 32]);
    let published = signer.verifier().public_hex();
    let Checkpointed::Signed(cp) = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(500),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint") else {
        panic!("expected a fresh signature")
    };
    assert_eq!(cp.upto, 3);
    for n in 3..6 {
        append(&pool, &group(n)).await;
    }

    // A receiver holding a key id and 64 hex characters — nothing that
    // could sign — picks the anchor.
    let receiver = CheckpointVerifier::from_public_hex("chk-op", &published).unwrap();
    let db = Db::Pool(&pool);
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .expect("the stored checkpoint verifies");
    assert_eq!(
        anchor,
        Anchor::Checkpoint {
            upto: 3,
            at: Timestamp(500),
            key_id: "chk-op".into()
        }
    );
    let v = undo::verify(db, &space(), anchor.clone(), head)
        .await
        .expect("the suffix verifies against the checkpoint's head");
    assert_eq!(v.checked, 3, "only the suffix was replayed, not all six");

    // Tamper ABOVE the anchor: reported at its absolute index, 4, not at
    // its offset from the checkpoint, 1.
    let mut doctored = group(4);
    doctored.commands[0].after = Some(img(&[("id", Value::Id("task_4".into()))]));
    doctor_body(&pool, 4, &doctored).await;
    let e = undo::verify(db, &space(), anchor.clone(), head)
        .await
        .expect_err("a doctored suffix entry must not verify");
    let undo::VerifyFailure::Tamper(t) = e else {
        panic!("expected a tamper, got {e}")
    };
    assert_eq!((t.index, t.kind), (4, TamperKind::HashMismatch));

    // Tamper BELOW the anchor: an anchored run does not look there — that
    // is what "start mid-chain" means, and the checkpoint's signature is
    // what stands in for the prefix. From genesis it is caught.
    let mut early = group(1);
    early.commands[0].after = Some(img(&[("id", Value::Id("task_1".into()))]));
    doctor_body(&pool, 1, &early).await;
    doctor_body(&pool, 4, &group(4)).await; // undo the later edit
    undo::verify(db, &space(), anchor, head)
        .await
        .expect("an anchored run verifies only the suffix");
    let e = undo::verify(db, &space(), Anchor::Genesis, ChainHash::GENESIS)
        .await
        .expect_err("from genesis the early edit is caught");
    let undo::VerifyFailure::Tamper(t) = e else {
        panic!("expected a tamper, got {e}")
    };
    assert_eq!((t.index, t.kind), (1, TamperKind::HashMismatch));
}

/// **Truncation** — the one rewrite a hash chain cannot notice on its own.
/// Delete the tail and every surviving link is still perfect; the chain has
/// no opinion about how long it was supposed to be. A stored checkpoint
/// does: it says the chain reached `upto`, so a chain that no longer does
/// is caught, and at the first position that went missing.
///
/// **Changed by R2b′ (`orphan-checkpoints`).** The first assertion used to
/// be that from genesis the truncation looks perfect, because the old rule
/// only compared the *anchor's* `upto` against the head and genesis has no
/// `upto` to compare. Orphans are now judged from `$checkpoints` on every
/// run, from any anchor, so genesis catches it too. The second assertion is
/// unchanged and is the point of the test: the answer is index 3, the first
/// position that went missing. It now arrives by a different route, because
/// a checkpoint the chain contradicts is no longer an anchor and
/// `choose_anchor` falls back to genesis.
#[tokio::test(flavor = "multi_thread")]
async fn a_truncated_tail_is_caught_by_the_checkpoint_that_covered_it() {
    let pool = fresh("ordo_op_truncate").await;
    for n in 0..5 {
        append(&pool, &group(n)).await;
    }
    let signer = Ed25519Signer::from_seed("chk-op", &[33u8; 32]);
    undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(700),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint at 5");

    // The tail is cut off. Indices 0..2 still chain perfectly.
    pool.rows("DELETE FROM \"$commandlog\" WHERE \"space\" = 'conformance' AND \"idx\" >= 3")
        .await
        .unwrap();
    let db = Db::Pool(&pool);
    let e = undo::verify(db, &space(), Anchor::Genesis, ChainHash::GENESIS)
        .await
        .expect_err("the checkpoint row outlasted the entries it covered");
    let undo::VerifyFailure::Tamper(t) = e else {
        panic!("expected a tamper, got {e}")
    };
    assert_eq!((t.index, t.kind), (3, TamperKind::Unreadable));

    let receiver =
        CheckpointVerifier::from_public_hex("chk-op", &signer.verifier().public_hex()).unwrap();
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .expect("an anchor");
    assert_eq!(
        anchor,
        Anchor::Genesis,
        "the checkpoint at 5 names a head the chain does not hold there, so it does not anchor"
    );
    let e = undo::verify(db, &space(), anchor, head)
        .await
        .expect_err("the checkpoint says this chain reached 5");
    let undo::VerifyFailure::Tamper(t) = e else {
        panic!("expected a tamper, got {e}")
    };
    assert_eq!(
        (t.index, t.kind),
        (3, TamperKind::Unreadable),
        "the first position that went missing"
    );
}

// ── restore discontinuities (plan-production-ops.md R2b) ────────────────

/// A chain of six with checkpoints at 3 and 6, rewound to index 3 and a
/// marker recorded at 4. Returns the signer's published verifier and the
/// checkpoint at 6's head (what the chain said before the restore).
async fn restored_once(pool: &Pool, seed: u8) -> (CheckpointVerifier, ChainHash) {
    for n in 0..3 {
        append(pool, &group(n)).await;
    }
    let signer = Ed25519Signer::from_seed("chk-op", &[seed; 32]);
    undo::sign_checkpoint(
        pool,
        &space(),
        &signer,
        Timestamp(500),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint at 3");
    for n in 3..6 {
        append(pool, &group(n)).await;
    }
    let Checkpointed::Signed(cp6) = undo::sign_checkpoint(
        pool,
        &space(),
        &signer,
        Timestamp(600),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint at 6") else {
        panic!("expected a fresh signature")
    };
    assert_eq!(cp6.upto, 6);
    rewind(pool, 4).await;
    let m = marker(pool, 1, 3, Some((5, cp6.head))).await;
    append(pool, &m).await;
    (
        CheckpointVerifier::from_public_hex("chk-op", &signer.verifier().public_hex()).unwrap(),
        cp6.head,
    )
}

/// **The seam, verified.** A restore rewound the chain below a signed
/// checkpoint and the marker says so; `verify` anchors below the seam,
/// replays through the marker, and reports the discontinuity rather than
/// a truncation. The binary prints it and exits 0.
///
/// Observed failing first (after R2a, before R2b): the same setup returned
/// `Tamper(Unreadable)` at index 5, head + 1, because the anchor was the
/// orphaned checkpoint at 6.
#[tokio::test(flavor = "multi_thread")]
async fn a_recorded_restore_verifies_with_one_discontinuity_at_its_index() {
    let pool = fresh("ordo_op_r2b_one").await;
    let (receiver, old_head) = restored_once(&pool, 41).await;
    let db = Db::Pool(&pool);
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .expect("an anchor");
    assert!(
        matches!(anchor, Anchor::Checkpoint { upto: 3, .. }),
        "{anchor}"
    );
    let v = undo::verify(db, &space(), anchor, head)
        .await
        .expect("a recorded restore verifies");
    assert_eq!(v.head.map(|(i, _)| i), Some(4));
    assert_eq!(v.checked, 2, "entries 3 and 4, the survivor and the marker");
    assert_eq!(v.discontinuities.len(), 1);
    let d = &v.discontinuities[0];
    assert_eq!(d.idx, 4);
    assert_eq!(d.marker.restored_from, (3, hash_at(&pool, 3).await));
    assert_eq!(d.marker.pre_restore_head, Some((5, old_head)));
    assert_eq!(v.orphaned_checkpoints, vec![(6, old_head)]);
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2b_one",
        "--key-id",
        "chk-op",
        "--public-key",
        &Ed25519Signer::from_seed("chk-op", &[41; 32])
            .verifier()
            .public_hex(),
    ]);
    assert_eq!(code, 0, "a recorded restore is lawful: {out}{err}");
    assert!(
        out.contains("VERIFIED 2 groups") && out.contains("· 1 restore discontinuity at index 4"),
        "{out}"
    );
    assert!(
        out.contains("restore   index 4 · restored from 3 = "),
        "{out}"
    );
    assert!(
        out.contains("orphaned  checkpoint upto 6 head ")
            && out.contains("explained by restore at index 4, kept"),
        "{out}"
    );
}

/// **The anchor rule** (`orphan-checkpoints`): the newest stored checkpoint
/// the chain agrees with, and never one it contradicts, even once the chain
/// has grown back past the orphan's `upto`.
///
/// Renamed from `verify_anchors_at_or_below_the_restored_head_never_above`.
/// The bound the old name states is gone: the anchor is decided by the
/// stored chain rather than by a marker's arithmetic, which is what keeps it
/// right across a prune that took the marker with it. The setup and every
/// assertion are the same, because on this chain the two rules agree.
#[tokio::test(flavor = "multi_thread")]
async fn verify_anchors_on_the_newest_checkpoint_the_chain_agrees_with() {
    let pool = fresh("ordo_op_r2b_anchor").await;
    let signer = Ed25519Signer::from_seed("chk-op", &[43u8; 32]);
    let mut head6 = ChainHash::GENESIS;
    for (upto, at) in [(2usize, 200u64), (4, 400), (6, 600)] {
        while (chain_len(&pool, &space()).await as usize) < upto {
            let n = chain_len(&pool, &space()).await as usize;
            append(&pool, &group(n)).await;
        }
        let Checkpointed::Signed(cp) = undo::sign_checkpoint(
            &pool,
            &space(),
            &signer,
            Timestamp(at),
            &Witness::new(pool.clone()),
        )
        .await
        .expect("checkpoint") else {
            panic!("expected a fresh signature")
        };
        head6 = cp.head;
    }
    // Restored to index 3. The chain still holds the checkpoint at 4's head
    // at position 3, and no longer holds the checkpoint at 6's at position 5.
    rewind(&pool, 4).await;
    append(&pool, &marker(&pool, 1, 3, Some((5, head6))).await).await;
    // Grow past 6, so the orphan at 6 names a position the chain has again
    // (with different history in it).
    for n in 10..14 {
        append(&pool, &group(n)).await;
    }
    let receiver =
        CheckpointVerifier::from_public_hex("chk-op", &signer.verifier().public_hex()).unwrap();
    let db = Db::Pool(&pool);
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .expect("an anchor");
    assert!(
        matches!(anchor, Anchor::Checkpoint { upto: 4, .. }),
        "at the bound, not below it and never above: {anchor}"
    );
    assert_eq!(head, hash_at(&pool, 3).await);
    let v = undo::verify(db, &space(), anchor, head)
        .await
        .expect("verifies from the checkpoint at 4");
    assert_eq!(v.head.map(|(i, _)| i), Some(8));
    assert_eq!(v.orphaned_checkpoints, vec![(6, head6)]);

    // What the orphan would have said as an anchor: its head is not the
    // hash the chain holds at 5, so the link check fails right there.
    let orphan_head = one(
        &pool,
        "SELECT encode(\"head\", 'hex') FROM \"$checkpoints\" WHERE \"upto\" = 6",
    )
    .await
    .unwrap();
    assert_ne!(orphan_head, hash_at(&pool, 5).await.to_string());
}

/// Orphans are derived and listed, never deleted: deleting them is exactly
/// what a dishonest operator would do.
#[tokio::test(flavor = "multi_thread")]
async fn orphaned_checkpoints_are_listed_and_kept() {
    let pool = fresh("ordo_op_r2b_orphans").await;
    let (receiver, old_head) = restored_once(&pool, 45).await;
    let db = Db::Pool(&pool);
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .unwrap();
    let v = undo::verify(db, &space(), anchor, head).await.unwrap();
    assert_eq!(v.orphaned_checkpoints, vec![(6, old_head)]);
    // From genesis the walk passes the same marker and lists the same
    // orphan.
    let g = undo::verify(db, &space(), Anchor::Genesis, ChainHash::GENESIS)
        .await
        .unwrap();
    assert_eq!(g.orphaned_checkpoints, vec![(6, old_head)]);
    assert_eq!(g.discontinuities, v.discontinuities);
    // And both rows are still there.
    assert_eq!(
        one(
            &pool,
            "SELECT string_agg(\"upto\"::text, ',' ORDER BY \"upto\") FROM \"$checkpoints\" \
             WHERE \"space\" = 'conformance'"
        )
        .await
        .as_deref(),
        Some("3,6")
    );
}

/// `continuity`: the arithmetic the boot assertion (R2d) will read.
#[tokio::test(flavor = "multi_thread")]
async fn continuity_reports_rewound_then_explained() {
    let pool = fresh("ordo_op_r2b_continuity").await;
    let db = Db::Pool(&pool);
    // A fresh chain: no checkpoint, so no proof it was ever longer.
    assert_eq!(
        undo::continuity(db, &Witness::new(pool.clone()), &space())
            .await
            .unwrap(),
        undo::Continuity::Intact
    );
    for n in 0..6 {
        append(&pool, &group(n)).await;
    }
    let signer = Ed25519Signer::from_seed("chk-op", &[47u8; 32]);
    let Checkpointed::Signed(cp6) = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(600),
        &Witness::new(pool.clone()),
    )
    .await
    .unwrap() else {
        panic!("expected a fresh signature")
    };
    assert_eq!(
        undo::continuity(db, &Witness::new(pool.clone()), &space())
            .await
            .unwrap(),
        undo::Continuity::Intact,
        "the chain agrees with the checkpoint"
    );

    // The rewind, unannounced.
    rewind(&pool, 4).await;
    assert_eq!(
        undo::continuity(db, &Witness::new(pool.clone()), &space())
            .await
            .unwrap(),
        undo::Continuity::Rewound {
            head: Some(3),
            checkpoint_upto: 6,
            checkpoint_head: cp6.head,
            source: undo::RecordSource::Witness,
        }
    );
    // Rewound all the way to nothing is still rewound.
    let empty = fresh("ordo_op_r2b_continuity_empty").await;
    append(&empty, &group(0)).await;
    let Checkpointed::Signed(cp1) = undo::sign_checkpoint(
        &empty,
        &space(),
        &signer,
        Timestamp(700),
        &Witness::new(empty.clone()),
    )
    .await
    .unwrap() else {
        panic!("expected a fresh signature")
    };
    rewind(&empty, 0).await;
    assert_eq!(
        undo::continuity(Db::Pool(&empty), &Witness::new(empty.clone()), &space())
            .await
            .unwrap(),
        undo::Continuity::Rewound {
            head: None,
            checkpoint_upto: 1,
            checkpoint_head: cp1.head,
            source: undo::RecordSource::Witness,
        }
    );

    // The marker explains it — because it attests a pre-restore head at or
    // above the orphan's position. A marker that attests nothing explains
    // nothing; that is `an_unattested_marker_explains_nothing`.
    append(&pool, &marker(&pool, 1, 3, Some((5, cp6.head))).await).await;
    assert_eq!(
        undo::continuity(db, &Witness::new(pool.clone()), &space())
            .await
            .unwrap(),
        undo::Continuity::Explained {
            marker_idx: 4,
            checkpoint_upto: 6,
            source: undo::RecordSource::Witness,
        }
    );
}

/// Two restores, two discontinuities, in chain order. The second restore
/// resumes above the first one's seam and below the first one's orphan, so a
/// rule that read only the newest marker's bound would anchor on the orphan
/// at 6. Under `orphan-checkpoints` the question never arises: the chain
/// holds another hash at position 5, so the checkpoint at 6 is not an
/// anchor, whatever any marker says.
#[tokio::test(flavor = "multi_thread")]
async fn two_restores_are_two_discontinuities_in_order() {
    let pool = fresh("ordo_op_r2b_two").await;
    let (receiver, old_head) = restored_once(&pool, 49).await;
    for n in 10..14 {
        append(&pool, &group(n)).await; // indices 5..8
    }
    rewind(&pool, 7).await;
    append(&pool, &marker(&pool, 2, 6, None).await).await; // index 7
    let db = Db::Pool(&pool);
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .unwrap();
    assert!(
        matches!(anchor, Anchor::Checkpoint { upto: 3, .. }),
        "{anchor}"
    );
    let v = undo::verify(db, &space(), anchor, head)
        .await
        .expect("two recorded restores verify");
    let at: Vec<usize> = v.discontinuities.iter().map(|d| d.idx).collect();
    assert_eq!(at, vec![4, 7]);
    assert_eq!(v.discontinuities[0].marker.restored_from.0, 3);
    assert_eq!(v.discontinuities[1].marker.restored_from.0, 6);
    assert_eq!(v.orphaned_checkpoints, vec![(6, old_head)]);
    let latest = undo::latest_restore_marker(db, &space())
        .await
        .unwrap()
        .expect("a marker");
    assert_eq!(latest.0, 7);
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2b_two",
        "--from-genesis",
    ]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("· 2 restore discontinuities at indices 4, 7"),
        "{out}"
    );
}

// ── R2b′: the stored chain classifies a checkpoint ──────────────────────
//
// The six tests below are the ones `orphan-checkpoints` (2026-09-19) was
// ruled for. The first three reproduce the three failures the old rule had
// and were each observed failing before the rule changed; the last three
// cover what the new rule adds.

/// **Failure 1 of the old rule: a prune across the seam.**
///
/// The old anchor rule bounded the anchor by `restored_from + 1` over every
/// marker. Prune past the marker and no checkpoint sits at or below that
/// bound any more, so the anchor fell to genesis on a chain whose prefix was
/// lawfully gone.
///
/// **Observed failing first:** `choose_anchor` answered `Genesis` and
/// `verify` answered `PrefixMissing { first_stored: 10 }` on a chain that
/// had done nothing wrong.
#[tokio::test(flavor = "multi_thread")]
async fn verify_survives_a_prune_across_the_seam() {
    let pool = fresh("ordo_op_r2bp_prune").await;
    ordo_exec::prune::ensure_tables(&pool).await.unwrap();
    let signer = Ed25519Signer::from_seed("chk-op", &[51u8; 32]);
    for n in 0..6 {
        append(&pool, &group(n)).await;
    }
    let Checkpointed::Signed(cp6) = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(600),
        &Witness::new(pool.clone()),
    )
    .await
    .unwrap() else {
        panic!("expected a fresh signature")
    };
    rewind(&pool, 4).await;
    append(&pool, &marker(&pool, 1, 3, Some((5, cp6.head))).await).await; // index 4
    for n in 10..15 {
        append(&pool, &group(n)).await; // indices 5..9
    }
    undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(1_000),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint at 10");
    for n in 20..22 {
        append(&pool, &group(n)).await; // indices 10, 11
    }

    // Past the horizon, so every entry is prunable and the retained tail is
    // what holds the bound. The orphan at 6 falls below the cut in the same
    // pass, so the marker that explained it may go with it.
    let receiver =
        CheckpointVerifier::from_public_hex("chk-op", &signer.verifier().public_hex()).unwrap();
    let out = ordo_exec::prune::prune(
        &pool,
        &space(),
        &receiver,
        &ordo_exec::cadence::LogPolicy::DEFAULT,
        10_000_000_000,
    )
    .await
    .expect("a lawful prune");
    let ordo_exec::prune::Pruned::Removed { floor, .. } = out else {
        panic!("expected entries removed, got {out:?}")
    };
    assert_eq!(floor.position, 10, "the cut is the checkpoint at 10");

    let db = Db::Pool(&pool);
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .expect("an anchor");
    assert!(
        matches!(anchor, Anchor::Checkpoint { upto: 10, .. }),
        "the chain holds the checkpoint at 10's head at position 9: {anchor}"
    );
    let v = undo::verify(db, &space(), anchor, head)
        .await
        .expect("a pruned chain with a pruned marker still verifies");
    assert_eq!(v.head.map(|(i, _)| i), Some(11));
    assert!(
        v.discontinuities.is_empty(),
        "the marker is below the floor"
    );
    assert!(
        v.orphaned_checkpoints.is_empty(),
        "the orphan at 6 is below the floor too: the chain has no opinion, so it is not listed"
    );
}

/// **Failure 2 of the old rule: a later unannounced rewind read as
/// explained.**
///
/// The old `continuity` compared the newest checkpoint against the newest
/// marker's seam. A restore long ago, a checkpoint signed over the history
/// since, and then a quiet rewind: the newest marker still sat below the
/// newest checkpoint, so the rewind came back `Explained`. Classification
/// against the stored chain plus the bound rule says otherwise, because the
/// marker attests a head at 100 and the checkpoint sits at 20 above it.
///
/// **Observed failing first:** `Explained { marker_idx: 4,
/// checkpoint_upto: 20 }`.
#[tokio::test(flavor = "multi_thread")]
async fn a_later_unannounced_rewind_is_rewound_not_explained() {
    let pool = fresh("ordo_op_r2bp_later").await;
    let (_, old_head) = restored_once(&pool, 53).await;
    let signer = Ed25519Signer::from_seed("chk-op", &[53u8; 32]);
    for n in 10..25 {
        append(&pool, &group(n)).await; // indices 5..19
    }
    let Checkpointed::Signed(cp20) = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(2_000),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint at 20") else {
        panic!("expected a fresh signature")
    };
    assert_eq!(cp20.upto, 20);
    assert_eq!(
        undo::continuity(Db::Pool(&pool), &Witness::new(pool.clone()), &space())
            .await
            .unwrap(),
        undo::Continuity::Explained {
            marker_idx: 4,
            checkpoint_upto: 6,
            source: undo::RecordSource::Witness,
        },
        "before the rewind the only orphan is the one the marker attests"
    );

    // The quiet rewind, with no marker.
    rewind(&pool, 10).await;
    let db = Db::Pool(&pool);
    assert_eq!(
        undo::continuity(db, &Witness::new(pool.clone()), &space())
            .await
            .unwrap(),
        undo::Continuity::Rewound {
            head: Some(9),
            checkpoint_upto: 20,
            checkpoint_head: cp20.head,
            source: undo::RecordSource::Witness,
        },
        "the marker attests a head at 5; it cannot vouch for a checkpoint at 20"
    );
    let (anchor, head) = undo::choose_anchor(db, &space(), None).await.unwrap();
    let e = undo::verify(db, &space(), anchor, head)
        .await
        .expect_err("the checkpoint at 20 says the chain reached 20");
    let undo::VerifyFailure::Tamper(t) = e else {
        panic!("expected the truncation finding, got {e}")
    };
    assert_eq!((t.index, t.kind), (10, TamperKind::Unreadable));
    // The first restore is still lawful and still explained.
    assert!(old_head != cp20.head);
}

/// **Failure 3 of the old rule: signing at an orphan's position.**
///
/// After a restore the chain grows back through positions an orphan already
/// holds. The old `AlreadyAt` test compared positions, so the checkpoint
/// worker reported "already checkpointed" over a head nobody had signed —
/// and the insert's `ON CONFLICT ("space", "upto")` would have dropped the
/// row anyway. The key is `(space, upto, head)` now (`checkpoint-key`), and
/// both rows are kept.
///
/// **Observed failing first:** `AlreadyAt(6)` on the first call.
#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_at_an_orphans_position_is_signed_and_both_rows_are_kept() {
    let pool = fresh("ordo_op_r2bp_signed").await;
    let (receiver, old_head) = restored_once(&pool, 55).await;
    let signer = Ed25519Signer::from_seed("chk-op", &[55u8; 32]);
    append(&pool, &group(99)).await; // index 5: the head is back at upto 6

    let Checkpointed::Signed(fresh_cp) = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(900),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("a head nobody has signed is signed") else {
        panic!("expected a fresh signature at a position an orphan holds")
    };
    assert_eq!(fresh_cp.upto, 6);
    assert_ne!(fresh_cp.head, old_head, "a different head at one position");
    assert_eq!(
        one(
            &pool,
            "SELECT count(*)::text FROM \"$checkpoints\" \
             WHERE \"space\" = 'conformance' AND \"upto\" = 6"
        )
        .await
        .as_deref(),
        Some("2"),
        "the orphan is kept beside the current one; deleting it is what a \
         dishonest operator would do"
    );

    let db = Db::Pool(&pool);
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .expect("an anchor");
    assert!(
        matches!(anchor, Anchor::Checkpoint { upto: 6, .. }),
        "{anchor}"
    );
    assert_eq!(head, fresh_cp.head, "the current row, not the orphan");
    let v = undo::verify(db, &space(), anchor, head)
        .await
        .expect("verifies from the new checkpoint");
    assert_eq!(v.orphaned_checkpoints, vec![(6, old_head)]);

    // Now the head *is* signed, so nothing is signed again.
    assert!(matches!(
        undo::sign_checkpoint(
            &pool,
            &space(),
            &signer,
            Timestamp(1_000),
            &Witness::new(pool.clone())
        )
        .await
        .unwrap(),
        Checkpointed::AlreadyAt(6)
    ));
}

/// The prune keeps a restore marker while an orphan above the cut still
/// needs it, and lets both go in the pass that takes the orphan below the
/// floor. Without the clamp the cut at 10 would delete the marker at index 4
/// and leave the orphan at 12 to be judged with nothing to explain it, which
/// is `UnexplainedOrphan` on a chain whose restore was properly recorded.
#[tokio::test(flavor = "multi_thread")]
async fn a_prune_keeps_a_marker_while_an_orphan_above_the_cut_needs_it() {
    let pool = fresh("ordo_op_r2bp_floor").await;
    ordo_exec::prune::ensure_tables(&pool).await.unwrap();
    let signer = Ed25519Signer::from_seed("chk-op", &[57u8; 32]);
    let receiver =
        CheckpointVerifier::from_public_hex("chk-op", &signer.verifier().public_hex()).unwrap();
    let policy = ordo_exec::cadence::LogPolicy::DEFAULT;
    let now = 10_000_000_000u64;

    for n in 0..12 {
        append(&pool, &group(n)).await;
    }
    let Checkpointed::Signed(cp12) = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(120),
        &Witness::new(pool.clone()),
    )
    .await
    .unwrap() else {
        panic!("expected a fresh signature")
    };
    rewind(&pool, 4).await;
    append(&pool, &marker(&pool, 1, 3, Some((11, cp12.head))).await).await; // index 4
    for n in 20..25 {
        append(&pool, &group(n)).await; // indices 5..9
    }
    undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(1_000),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint at 10");
    for n in 25..30 {
        append(&pool, &group(n)).await; // indices 10..14
    }
    undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(1_500),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint at 15");

    // The retained tail holds the bound at 14, so the cut would be the
    // checkpoint at 10 — which deletes the marker at index 4 and leaves the
    // orphan at 12 above the cut with nothing to explain it. The clamp drops
    // the cut to a current checkpoint at or below index 4; there is none, so
    // there is nothing to prune yet.
    let f = ordo_exec::prune::floor(&pool, &space(), &policy, now)
        .await
        .unwrap();
    assert_eq!(
        f.holder,
        ordo_exec::prune::FloorHolder::RestoreMarker {
            index: 4,
            orphan_upto: 12
        },
        "the marker at index 4 explains the orphan at 12, which is at or above the cut"
    );
    assert_eq!(f.position, 0, "no current checkpoint at or below index 4");
    assert!(matches!(
        ordo_exec::prune::prune(&pool, &space(), &receiver, &policy, now)
            .await
            .unwrap(),
        ordo_exec::prune::Pruned::NothingToPrune { .. }
    ));

    // One more entry lets the tail permit a cut at 15, which takes the
    // orphan at 12 below the floor — so the marker is free to go with it.
    append(&pool, &group(40)).await; // index 15
    let f = ordo_exec::prune::floor(&pool, &space(), &policy, now)
        .await
        .unwrap();
    assert_eq!(f.position, 15, "the clamp lifted: {:?}", f.holder);
    let out = ordo_exec::prune::prune(&pool, &space(), &receiver, &policy, now)
        .await
        .expect("a lawful prune");
    assert!(matches!(out, ordo_exec::prune::Pruned::Removed { .. }));

    let db = Db::Pool(&pool);
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .unwrap();
    let v = undo::verify(db, &space(), anchor, head)
        .await
        .expect("the marker and the orphans it explained fell below the floor together");
    assert!(v.orphaned_checkpoints.is_empty(), "{v:?}");
    assert!(v.discontinuities.is_empty());
}

/// An orphan nothing attests **fails** verification, naming the checkpoint.
/// It is at or below the head, so the truncation check cannot see it: every
/// stored link recomputes and the chain is long enough. The only evidence is
/// a signed row saying another head stood at that position.
#[tokio::test(flavor = "multi_thread")]
async fn an_unexplained_orphan_fails_verification_naming_the_checkpoint() {
    let pool = fresh("ordo_op_r2bp_unexplained").await;
    let signer = Ed25519Signer::from_seed("chk-op", &[59u8; 32]);
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(300),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint at 3");
    for n in 3..6 {
        append(&pool, &group(n)).await;
    }
    let Checkpointed::Signed(cp6) = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(600),
        &Witness::new(pool.clone()),
    )
    .await
    .unwrap() else {
        panic!("expected a fresh signature")
    };
    // A rewind with no marker, then enough new history to cover the orphan's
    // position, so the chain is long enough and only the head differs.
    rewind(&pool, 4).await;
    for n in 30..34 {
        append(&pool, &group(n)).await; // indices 4..7
    }

    let receiver =
        CheckpointVerifier::from_public_hex("chk-op", &signer.verifier().public_hex()).unwrap();
    let db = Db::Pool(&pool);
    let (anchor, head) = undo::choose_anchor(db, &space(), Some(&receiver))
        .await
        .unwrap();
    assert!(
        matches!(anchor, Anchor::Checkpoint { upto: 3, .. }),
        "{anchor}"
    );
    let e = undo::verify(db, &space(), anchor, head)
        .await
        .expect_err("a checkpoint the chain contradicts, with no restore recorded");
    let undo::VerifyFailure::UnexplainedOrphan { upto, head } = e else {
        panic!("expected UnexplainedOrphan, got {e}")
    };
    assert_eq!((upto, head), (6, cp6.head));
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2bp_unexplained",
        "--key-id",
        "chk-op",
        "--public-key",
        &Ed25519Signer::from_seed("chk-op", &[59; 32])
            .verifier()
            .public_hex(),
    ]);
    assert_eq!(code, 3, "an unrecorded restore is not lawful: {out}{err}");
    assert!(err.contains("position 6"), "{err}");
}

/// `pre_restore_head: None` is a legal, disclosed answer and it explains
/// **nothing**. Absent means unattested, and an orphan a marker cannot bound
/// is an orphan nobody vouched for.
#[tokio::test(flavor = "multi_thread")]
async fn an_unattested_marker_explains_nothing() {
    let pool = fresh("ordo_op_r2bp_unattested").await;
    let signer = Ed25519Signer::from_seed("chk-op", &[61u8; 32]);
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(300),
        &Witness::new(pool.clone()),
    )
    .await
    .unwrap();
    for n in 3..6 {
        append(&pool, &group(n)).await;
    }
    let Checkpointed::Signed(cp6) = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(600),
        &Witness::new(pool.clone()),
    )
    .await
    .unwrap() else {
        panic!("expected a fresh signature")
    };
    rewind(&pool, 4).await;
    append(&pool, &marker(&pool, 1, 3, None).await).await; // index 4, unattested

    let db = Db::Pool(&pool);
    assert_eq!(
        undo::continuity(db, &Witness::new(pool.clone()), &space())
            .await
            .unwrap(),
        undo::Continuity::Rewound {
            head: Some(4),
            checkpoint_upto: 6,
            checkpoint_head: cp6.head,
            source: undo::RecordSource::Witness,
        },
        "the restore is recorded, and what it discarded is not"
    );

    // Re-recorded with the head it discarded, the same orphan is explained.
    append(&pool, &marker(&pool, 2, 3, Some((5, cp6.head))).await).await; // index 5
    assert_eq!(
        undo::continuity(db, &Witness::new(pool.clone()), &space())
            .await
            .unwrap(),
        undo::Continuity::Explained {
            marker_idx: 5,
            checkpoint_upto: 6,
            source: undo::RecordSource::Witness,
        }
    );
}

// ── R2d-ii: `continuity` reads the witness ──────────────────────────────

/// **The failure the witness exists to close.** A point-in-time restore of
/// the space's whole database rolls `$checkpoints` back with the chain, so
/// afterwards the two agree and the space's own tables have nothing to say.
/// The control database was not restored, and it still holds the head.
///
/// **Observed failing first:** `Intact`.
#[tokio::test(flavor = "multi_thread")]
async fn a_witnessed_checkpoint_the_chain_no_longer_reaches_is_rewound_even_when_the_checkpoints_agree(
) {
    let pool = fresh("ordo_op_r2dii_whole").await;
    let w = Witness::new(pool.clone());
    let signer = Ed25519Signer::from_seed("chk-op", &[65u8; 32]);
    for n in 0..6 {
        append(&pool, &group(n)).await;
    }
    let Checkpointed::Signed(cp6) =
        undo::sign_checkpoint(&pool, &space(), &signer, Timestamp(600), &w)
            .await
            .unwrap()
    else {
        panic!("expected a fresh signature")
    };

    // The shape a whole-database restore leaves: the chain short and the
    // checkpoints gone with it.
    rewind(&pool, 4).await;
    pool.rows("DELETE FROM \"$checkpoints\" WHERE \"space\" = 'conformance' AND \"upto\" > 4")
        .await
        .unwrap();

    let db = Db::Pool(&pool);
    let empty_witness = Witness::new(pool.clone());
    // Asserted so it stays visible: over the space alone there is nothing
    // left that disagrees with anything.
    pool.rows("DELETE FROM \"$chainwitness\"").await.unwrap();
    assert_eq!(
        undo::continuity(db, &empty_witness, &space())
            .await
            .unwrap(),
        undo::Continuity::Intact,
        "this is the reading the witness table exists to replace"
    );

    // With the control database's copy, the loss is visible.
    w.record_checkpoint(&cp6).await.unwrap();
    assert_eq!(
        undo::continuity(db, &w, &space()).await.unwrap(),
        undo::Continuity::Rewound {
            head: Some(3),
            checkpoint_upto: 6,
            checkpoint_head: cp6.head,
            source: undo::RecordSource::Witness,
        }
    );
}

/// A `restore` row written after the witnessed checkpoint accounts for it,
/// and the space serves again.
#[tokio::test(flavor = "multi_thread")]
async fn a_restore_row_after_the_witnessed_checkpoint_explains_it() {
    let pool = fresh("ordo_op_r2dii_explained").await;
    let w = Witness::new(pool.clone());
    let signer = Ed25519Signer::from_seed("chk-op", &[67u8; 32]);
    for n in 0..6 {
        append(&pool, &group(n)).await;
    }
    undo::sign_checkpoint(&pool, &space(), &signer, Timestamp(600), &w)
        .await
        .unwrap();
    rewind(&pool, 4).await;
    pool.rows("DELETE FROM \"$checkpoints\" WHERE \"space\" = 'conformance' AND \"upto\" > 4")
        .await
        .unwrap();

    let db = Db::Pool(&pool);
    assert!(matches!(
        undo::continuity(db, &w, &space()).await.unwrap(),
        undo::Continuity::Rewound { .. }
    ));

    let r = undo::record_restore(
        &pool,
        &w,
        &space(),
        undo::RestoreRecord {
            operator: "ops@example.test".into(),
            source: pitr(1_757_000_000),
            pre_restore_head: None,
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("recorded");
    assert!(r.witnessed);
    assert_eq!(
        undo::continuity(db, &w, &space()).await.unwrap(),
        undo::Continuity::Explained {
            marker_idx: 4,
            checkpoint_upto: 6,
            source: undo::RecordSource::Witness,
        }
    );
}

/// **The case the bound rule cannot see.** A head at 100 attested by a
/// marker, a restore recorded, history regrown and a checkpoint at 50
/// witnessed *after* the restore row, then a quiet rewind to 20. Under the
/// bound rule alone `50 <= 100` reads as explained; `seq` says the
/// checkpoint at 50 was signed over the history that existed after the
/// restore, so no recorded restore accounts for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_witnessed_after_the_restore_is_a_fresh_rewind_even_below_the_attested_head() {
    let pool = fresh("ordo_op_r2dii_fresh").await;
    let w = Witness::new(pool.clone());
    let signer = Ed25519Signer::from_seed("chk-op", &[69u8; 32]);
    for n in 0..100 {
        append(&pool, &group(n)).await;
    }
    let Checkpointed::Signed(cp100) =
        undo::sign_checkpoint(&pool, &space(), &signer, Timestamp(1_000), &w)
            .await
            .unwrap()
    else {
        panic!("expected a fresh signature")
    };
    assert_eq!(cp100.upto, 100);

    // Restored to index 3, recorded, with the head it discarded attested.
    rewind(&pool, 4).await;
    pool.rows("DELETE FROM \"$checkpoints\" WHERE \"space\" = 'conformance'")
        .await
        .unwrap();
    let r = undo::record_restore(
        &pool,
        &w,
        &space(),
        undo::RestoreRecord {
            operator: "ops@example.test".into(),
            source: pitr(1_757_000_000),
            pre_restore_head: None,
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("recorded");
    assert!(r.witnessed);
    let db = Db::Pool(&pool);
    assert!(
        matches!(
            undo::continuity(db, &w, &space()).await.unwrap(),
            undo::Continuity::Explained { .. }
        ),
        "the restore is accounted for"
    );

    // Regrow to index 49 and sign at 50, which is witnessed above the
    // restore row.
    for n in 200..245 {
        append(&pool, &group(n)).await; // indices 5..49
    }
    let Checkpointed::Signed(cp50) =
        undo::sign_checkpoint(&pool, &space(), &signer, Timestamp(2_000), &w)
            .await
            .unwrap()
    else {
        panic!("expected a fresh signature")
    };
    assert_eq!(cp50.upto, 50);

    // The quiet rewind.
    rewind(&pool, 20).await;
    assert_eq!(
        undo::continuity(db, &w, &space()).await.unwrap(),
        undo::Continuity::Rewound {
            head: Some(19),
            checkpoint_upto: 50,
            checkpoint_head: cp50.head,
            source: undo::RecordSource::Witness,
        },
        "under the bound rule alone 50 <= 100 would read as explained"
    );
}

/// A `restore` row states that the chain reached the marker's position with
/// the marker's hash. Delete the marker and the chain contradicts the row,
/// which is a rewind that took a recorded restore with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_restore_row_whose_marker_is_gone_is_itself_a_rewind() {
    let pool = fresh("ordo_op_r2dii_marker_gone").await;
    let w = Witness::new(pool.clone());
    for n in 0..4 {
        append(&pool, &group(n)).await;
    }
    let r = undo::record_restore(
        &pool,
        &w,
        &space(),
        undo::RestoreRecord {
            operator: "ops@example.test".into(),
            source: pitr(1_757_000_000),
            pre_restore_head: None,
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("recorded");
    assert_eq!(r.idx, 4);
    let db = Db::Pool(&pool);
    assert_eq!(
        undo::continuity(db, &w, &space()).await.unwrap(),
        undo::Continuity::Intact
    );

    rewind(&pool, 4).await;
    assert_eq!(
        undo::continuity(db, &w, &space()).await.unwrap(),
        undo::Continuity::Rewound {
            head: Some(3),
            checkpoint_upto: 5,
            checkpoint_head: r.hash,
            source: undo::RecordSource::Witness,
        },
        "the restore row names position 5, which is the marker's index plus one"
    );
}

// ── R2c: `ordo restore --record` ────────────────────────────────────────

/// A database for the restore tests, with the two tables `fresh` does not
/// clear and these tests read.
async fn fresh_for_restore(db: &str) -> Pool {
    let pool = fresh(db).await;
    let _ = pool.rows("DELETE FROM \"$secevents\"").await;
    pool
}

fn pitr(at: u64) -> RestoreSource {
    RestoreSource::Pitr { at: Timestamp(at) }
}

/// The marker lands at head + 1, and the `custody-action` in the security
/// stream carries the group's own correlation, so the two records of one
/// act join on one id. The restore is then lawful history: `verify` exits 0
/// and names the discontinuity.
#[tokio::test(flavor = "multi_thread")]
async fn restore_record_writes_the_marker_at_head_plus_one_and_the_custody_event() {
    let pool = fresh_for_restore("ordo_op_r2c_marker").await;
    let w = Witness::new(pool.clone());
    for n in 0..4 {
        append(&pool, &group(n)).await;
    }
    let r = undo::record_restore(
        &pool,
        &w,
        &space(),
        undo::RestoreRecord {
            operator: "ops@example.test".into(),
            source: pitr(1_757_000_000),
            pre_restore_head: Some((9, hash_at(&pool, 3).await)),
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("recorded");
    assert_eq!(r.idx, 4, "head + 1");
    assert_eq!(r.gap, 0);
    assert!(r.witnessed, "the control database took the restore row");
    assert!(r.audit.starts_with("recorded"), "{}", r.audit);

    assert_eq!(
        one(
            &pool,
            "SELECT \"actor\" FROM \"$commandlog\" \
             WHERE \"space\" = 'conformance' AND \"idx\" = 4"
        )
        .await
        .as_deref(),
        Some("system:restore")
    );
    // The security entry carries its correlation inside the hashed body
    // rather than in a column, so the join is asserted where it lives.
    let group_corr = correlation_at(&pool, &space(), 4).await;
    assert_eq!(
        one(
            &pool,
            &format!(
                "SELECT count(*)::text FROM \"$secevents\" WHERE \"kind\" = 'custody-action' \
                 AND position('{}' in encode(\"body\", 'escape')) > 0",
                group_corr.0
            )
        )
        .await
        .as_deref(),
        Some("1"),
        "the two records of one act join on one correlation"
    );

    // The witness holds the marker, at the marker's index plus one.
    let rows = w.rows(&space()).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].kind, rows[0].upto, rows[0].head),
        (WitnessKind::Restore, 5, r.hash)
    );
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2c_marker",
        "--from-genesis",
    ]);
    assert_eq!(code, 0, "a recorded restore is lawful: {out}{err}");
    assert!(out.contains("1 restore discontinuity at index 4"), "{out}");
}

/// A restore from an unidentifiable source is refused. Usage, not failure:
/// nothing was attempted, and the operator's next move is to type the
/// source they restored from.
#[tokio::test(flavor = "multi_thread")]
async fn restore_record_refuses_an_unidentifiable_source() {
    let pool = fresh_for_restore("ordo_op_r2c_source").await;
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    drop(pool);

    let base = [
        "restore",
        "--record",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2c_source",
        "--as",
        "ops@example.test",
    ];
    let (_, err, code) = ordo(&base);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("`--source` is required"), "{err}");

    let mut pitr_no_at = base.to_vec();
    pitr_no_at.extend(["--source", "pitr"]);
    let (_, err, code) = ordo(&pitr_no_at);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--at"), "{err}");

    let mut bundle_no_digest = base.to_vec();
    bundle_no_digest.extend(["--source", "bundle"]);
    let (_, err, code) = ordo(&bundle_no_digest);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--digest"), "{err}");

    // And nothing was written by any of them.
    let (out, _, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2c_source",
    ]);
    assert_eq!(code, 0);
    assert!(!out.contains("restore discontinuity"), "{out}");
}

/// With no witness row and no stored checkpoint there is nothing to default
/// from, and the marker is written unattested — which the output says,
/// because "nobody could say how long the chain was" is a disclosure and
/// not a silence.
#[tokio::test(flavor = "multi_thread")]
async fn restore_record_with_nothing_to_default_from_prints_unattested() {
    let pool = fresh_for_restore("ordo_op_r2c_unattested").await;
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    drop(pool);

    let (out, err, code) = ordo(&[
        "restore",
        "--record",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2c_unattested",
        "--as",
        "ops@example.test",
        "--source",
        "pitr",
        "--at",
        "1757000000",
    ]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("pre-restore head defaulted from nothing — unattested"),
        "{out}"
    );
}

/// A credential directory `ordo_exec::creds::Dir` will accept: one label,
/// mode 0600 inside a mode-0700 directory (the module's own rule — see
/// `ordo-exec/src/creds.rs`). It holds `ORDO_PG_PASSWORD` when that is set
/// (CI's Postgres is over TCP and wants it) and is empty otherwise, which is
/// what makes a Postgres label passwordless on the local socket.
fn configured_credential_dir(label: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ordo_op_r9_creds_{}_{}", std::process::id(), label));
    std::fs::create_dir_all(&dir).expect("credential dir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("dir mode");
    }
    let file = dir.join(label);
    std::fs::write(&file, std::env::var("ORDO_PG_PASSWORD").unwrap_or_default())
        .expect("credential file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("mode");
    }
    dir
}

/// 18d R-O1: `ordo restore --record --tenant` resolves `--space`'s own
/// database through `$spacedb` instead of trusting `--db` to name it, and
/// prints which database it found.
///
/// The tenant is attached to a second real database on the same cluster.
/// The marker must land THERE, never in the control database this call
/// also opens for the lookup and the witness — a resolver that fell back
/// to the connection it already had would serve (and restore) the wrong
/// database exactly the way `spaces.rs`'s own unreachable-target test
/// checks the live path never falls back to the default.
#[tokio::test(flavor = "multi_thread")]
async fn restore_resolves_the_tenants_database_from_its_id() {
    const CONTROL_DB: &str = "ordo_op_r9_tenant_control";
    const TARGET_DB: &str = "ordo_op_r9_tenant_target";
    const LABEL: &str = "r9-tenant-local";

    let control = fresh_for_restore(CONTROL_DB).await;
    let target = fresh_for_restore(TARGET_DB).await;
    for n in 0..3 {
        append(&target, &group(n)).await;
    }
    drop(target);

    let env = PoolConfig::from_env();
    ordo_exec::spacedb::ensure_table(&control)
        .await
        .expect("table");
    ordo_exec::spacedb::attach(
        &control,
        &ordo_crypto::SpaceId("conformance".into()),
        &ordo_exec::spacedb::DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: TARGET_DB.into(),
            credential: LABEL.into(),
            custody: ordo_exec::spacedb::DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "mem_owner".into(),
        },
    )
    .await
    .expect("attached");
    drop(control);

    let creds = configured_credential_dir(LABEL);
    let out = support::ordo()
        .args([
            "restore",
            "--record",
            "--tenant",
            "--space",
            "conformance",
            "--control-db",
            CONTROL_DB,
            "--as",
            "ops@example.test",
            "--source",
            "pitr",
            "--at",
            "1757000000",
        ])
        .env("ORDO_PG_HOST", &Pg::default().host)
        .env("ORDO_PG_PORT", &Pg::default().port)
        .env("ORDO_PG_USER", &Pg::default().user)
        .env("ORDO_TENANT_CREDENTIALS", "dir")
        .env("ORDO_TENANT_CREDENTIALS_DIR", &creds)
        .env_remove("ORDO_CHECKPOINT_KEY_ID")
        .env_remove("ORDO_CHECKPOINT_PUBKEY")
        .output()
        .expect("run ordo");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let expected_label = format!("{}:{}/{TARGET_DB}", env.host, env.port);
    assert!(
        stdout.contains(&format!("database {expected_label}")),
        "the resolved database was not printed: {stdout}"
    );

    // Proof it actually wrote there: reconnect to the target directly (not
    // through the CLI) and find the marker. The control database has no
    // chain at all, so a marker found on the control connection instead
    // would mean the resolution silently fell back to it.
    let target_again = Pool::connect(PoolConfig {
        db: TARGET_DB.to_string(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("reconnect to the target");
    let markers = undo::restore_markers(Db::Pool(&target_again), &space())
        .await
        .expect("markers");
    assert_eq!(
        markers.len(),
        1,
        "the marker must be in the target: {markers:?}"
    );
}

/// 18d R-O1: `--tenant` and `--db` are two independent ways to name where a
/// tenant's rows are, and giving both is refused rather than guessed at —
/// the exact hazard R-O1 exists to remove. Usage error (exit 2): nothing
/// was attempted, and no database connection is even needed to see it.
#[tokio::test(flavor = "multi_thread")]
async fn restore_refuses_a_tenant_id_and_a_host_together() {
    let (out, err, code) = ordo(&[
        "restore",
        "--record",
        "--tenant",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r9_conflict",
        "--as",
        "ops@example.test",
        "--source",
        "pitr",
        "--at",
        "1757000000",
    ]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("--tenant"), "{err}");
    assert!(err.contains("--db"), "{err}");
}

// ── fleet ────────────────────────────────────────────────────────────────

/// 18d R-O6: the operator surface prints a tenant's **name** beside its
/// **id**, because the id (`spc_...`) is opaque by design (the 2026-09-21
/// ruling behind `ordo_host::tenants`) — a fleet report that named only
/// ids would tell an operator which target is unhealthy and nothing about
/// whose tenant that is.
#[tokio::test(flavor = "multi_thread")]
async fn the_operator_surface_prints_the_name_beside_the_id() {
    const CONTROL_DB: &str = "ordo_op_r9_fleet_control";
    const TENANT_ID: &str = "spc_r9fleettest0000000000000";
    const TENANT_NAME: &str = "Acme Corp (fleet test)";

    let control = fresh_for_restore(CONTROL_DB).await;
    ordo_host::tenants::ensure_table(&control)
        .await
        .expect("tenants table");
    ordo_exec::spacedb::ensure_table(&control)
        .await
        .expect("spacedb table");
    for (table, col) in [("$spaces", "$space"), ("$spacedb", "$space")] {
        control
            .rows(&format!(
                "DELETE FROM \"{table}\" WHERE \"{col}\" = {}",
                ordo_exec::sql_lit(TENANT_ID)
            ))
            .await
            .unwrap();
    }
    ordo_host::tenants::reserve(
        &control,
        TENANT_ID,
        TENANT_NAME,
        "mem_owner",
        1_700_000_000,
        None,
    )
    .await
    .expect("mint");
    let env = PoolConfig::from_env();
    ordo_exec::spacedb::attach(
        &control,
        &ordo_crypto::SpaceId(TENANT_ID.into()),
        &ordo_exec::spacedb::DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: "ordo_op_r9_fleet_target".into(),
            credential: "r9-fleet-local".into(),
            custody: ordo_exec::spacedb::DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "mem_owner".into(),
        },
    )
    .await
    .expect("attached");
    drop(control);

    let (out, err, code) = ordo(&["fleet", "--db", CONTROL_DB]);
    assert_eq!(code, 0, "{out}{err}");
    let tenant_line = out
        .lines()
        .find(|l| l.contains(TENANT_ID))
        .unwrap_or_else(|| panic!("the tenant's id was not printed at all: {out}"));
    assert!(
        tenant_line.contains(TENANT_NAME),
        "the id is printed without its name beside it: {tenant_line:?}\nfull output:\n{out}"
    );
    let expected_target = format!("{}:{}/ordo_op_r9_fleet_target", env.host, env.port);
    assert!(out.contains(&expected_target), "{out}");
}

/// The default `restore-witness` gives: the newest **witnessed** checkpoint
/// first, because after a whole-database restore that is the one record
/// still holding the head the restore discarded. The test deletes the chain
/// and the checkpoints, which is the shape such a restore leaves, and the
/// marker still attests the right head without the operator naming it.
#[tokio::test(flavor = "multi_thread")]
async fn restore_record_defaults_the_pre_restore_head_to_the_witnessed_checkpoint() {
    let pool = fresh_for_restore("ordo_op_r2c_default").await;
    let w = Witness::new(pool.clone());
    let signer = Ed25519Signer::from_seed("chk-op", &[63u8; 32]);
    for n in 0..6 {
        append(&pool, &group(n)).await;
    }
    let Checkpointed::Signed(cp6) =
        undo::sign_checkpoint(&pool, &space(), &signer, Timestamp(600), &w)
            .await
            .unwrap()
    else {
        panic!("expected a fresh signature")
    };

    // What a whole-database restore of the space leaves: the chain short
    // and the checkpoints gone with it. Only the control database still
    // knows the chain ever reached 6.
    rewind(&pool, 4).await;
    pool.rows("DELETE FROM \"$checkpoints\" WHERE \"space\" = 'conformance'")
        .await
        .unwrap();
    assert!(undo::latest_checkpoint(Db::Pool(&pool), &space())
        .await
        .unwrap()
        .is_none());

    let r = undo::record_restore(
        &pool,
        &w,
        &space(),
        undo::RestoreRecord {
            operator: "ops@example.test".into(),
            source: pitr(1_757_000_000),
            pre_restore_head: None,
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("recorded");
    assert_eq!(r.defaulted, Some(undo::PreHeadSource::Witness));
    let (_, marker) = undo::latest_restore_marker(Db::Pool(&pool), &space())
        .await
        .unwrap()
        .expect("a marker");
    assert_eq!(
        marker.pre_restore_head,
        Some((5, cp6.head)),
        "the head the witness holds, without the test naming it"
    );
}

/// A second marker on one restore point is almost always a re-run, and a
/// re-run must not write a second marker. Once the record is complete the
/// answer is `AlreadyRecorded`; `--again` is what a real second restore to
/// the same point says.
#[tokio::test(flavor = "multi_thread")]
async fn restore_record_twice_on_one_point_needs_again() {
    let pool = fresh_for_restore("ordo_op_r2c_again").await;
    let w = Witness::new(pool.clone());
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    let rec = || undo::RestoreRecord {
        operator: "ops@example.test".into(),
        source: pitr(1_757_000_000),
        pre_restore_head: None,
        again: false,
        restored_from: None,
    };
    let first = undo::record_restore(&pool, &w, &space(), rec())
        .await
        .expect("the first record");
    assert_eq!(first.idx, 3);

    // Straight away again, which is what an operator who is not sure the
    // first one worked actually does.
    let e = undo::record_restore(&pool, &w, &space(), rec())
        .await
        .expect_err("a re-run is not a second restore");
    assert!(
        matches!(e, undo::RestoreError::AlreadyRecorded { idx: 3 }),
        "{e}"
    );
    // And when the operator names the point, the plan's own reading of the
    // same question refuses it too.
    let mut named = rec();
    named.restored_from = Some(2);
    let e = undo::record_restore(&pool, &w, &space(), named)
        .await
        .expect_err("the same restored-from is the same restore");
    assert!(
        matches!(e, undo::RestoreError::AlreadyRecorded { idx: 3 }),
        "{e}"
    );

    let mut again = rec();
    again.again = true;
    let second = undo::record_restore(&pool, &w, &space(), again)
        .await
        .expect("--again says this really is a second restore");
    assert_eq!(second.idx, 4, "a second marker, above the first");
    assert_eq!(chain_len(&pool, &space()).await, 5);
}

/// A re-run **completes** a witness row the control database refused the
/// first time, without `--again` and without a second marker:
/// `AlreadyRecorded` is answered only when the restore row exists too.
#[tokio::test(flavor = "multi_thread")]
async fn restore_record_completes_a_missing_witness_row_on_rerun() {
    let pool = fresh_for_restore("ordo_op_r2c_complete").await;
    let w = Witness::new(pool.clone());
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    let rec = || undo::RestoreRecord {
        operator: "ops@example.test".into(),
        source: pitr(1_757_000_000),
        pre_restore_head: None,
        again: false,
        restored_from: None,
    };
    undo::record_restore(&pool, &w, &space(), rec())
        .await
        .expect("recorded");

    // The control database lost the row, which is what a failed witness
    // write leaves behind.
    pool.rows("DELETE FROM \"$chainwitness\"").await.unwrap();
    rewind(&pool, 3).await;

    let repair = undo::record_restore(&pool, &w, &space(), rec())
        .await
        .expect("the re-run repairs rather than refusing");
    assert_eq!(repair.idx, 3, "the marker that is already there");
    assert!(repair.witnessed);
    assert_eq!(
        chain_len(&pool, &space()).await,
        4,
        "no second marker was written"
    );
    assert_eq!(w.rows(&space()).await.unwrap().len(), 1);
}

/// Ordinary writes can land between the restore and the record: nothing
/// refuses them, because the boot assertion fires only when a record proves
/// the chain was longer. Then `restored_from` says where history broke and
/// the marker's `prev` says where it was written, and both are true. The
/// command prints the gap rather than leaving an operator to notice it.
#[tokio::test(flavor = "multi_thread")]
async fn restore_record_reports_the_gap_when_writes_landed_first() {
    let pool = fresh_for_restore("ordo_op_r2c_gap").await;
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    drop(pool);

    let (out, err, code) = ordo(&[
        "restore",
        "--record",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2c_gap",
        "--as",
        "ops@example.test",
        "--source",
        "bundle",
        "--digest",
        &"ab".repeat(32),
        "--pre-restore-head",
        &format!("9:{}", "cd".repeat(32)),
    ]);
    assert_eq!(code, 0, "{out}{err}");
    // Head is index 2, so the marker is at 3 and the gap is 0.
    assert!(!out.contains("gap "), "{out}");
    assert!(out.contains("RECORDED marker at index 3"), "{out}");

    let pool = Pool::connect(PoolConfig {
        db: "ordo_op_r2c_gap".into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .unwrap();
    // Now the shape the gap exists for: history broke at index 0 and two
    // ordinary writes landed before anybody recorded it. The chain cannot
    // tell those two from any other write, so `--restored-from` is how the
    // operator says where the break was.
    rewind(&pool, 1).await;
    for n in 20..22 {
        append(&pool, &group(n)).await; // indices 1, 2
    }
    drop(pool);

    let (out, err, code) = ordo(&[
        "restore",
        "--record",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2c_gap",
        "--as",
        "ops@example.test",
        "--source",
        "pitr",
        "--at",
        "1757000000",
        "--unattested",
        "--restored-from",
        "0",
    ]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("RECORDED marker at index 3"), "{out}");
    assert!(out.contains("restored from (0, "), "{out}");
    assert!(
        out.contains("gap     2 writes landed between the restore and this record"),
        "{out}"
    );

    // An index the chain does not hold is refused: a marker that named one
    // would be a claim about nothing.
    let (out, err, code) = ordo(&[
        "restore",
        "--record",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2c_gap",
        "--as",
        "ops@example.test",
        "--source",
        "pitr",
        "--at",
        "1757000000",
        "--unattested",
        "--restored-from",
        "99",
    ]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("no entry at index 99"), "{err}");
}

/// The actor column is plaintext and the actor is hashed through the body,
/// so on an untampered chain they agree. A row whose column claims a
/// marker its body lacks is refused, naming the index.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_whose_actor_claims_restore_without_a_marker_is_refused() {
    let pool = fresh("ordo_op_r2b_claims").await;
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    pool.rows(
        "UPDATE \"$commandlog\" SET \"actor\" = 'system:restore' \
         WHERE \"space\" = 'conformance' AND \"idx\" = 1",
    )
    .await
    .unwrap();
    let db = Db::Pool(&pool);
    let e = undo::latest_restore_marker(db, &space())
        .await
        .expect_err("a claimed marker with no marker in the body");
    assert!(e.to_string().contains("index 1"), "{e}");
    // **Where the refusal fires moved with R2b′, and it did not weaken.**
    // The anchor no longer depends on the markers, so `choose_anchor` no
    // longer reads them; `verify` does, on every run and whether or not a
    // checkpoint is stored, because a marker is what explains an orphan.
    // `ordo verify` runs both, so the binary still exits 3.
    let e = undo::verify(db, &space(), Anchor::Genesis, ChainHash::GENESIS)
        .await
        .expect_err("verification is not rendered over a lying row");
    assert!(e.to_string().contains("index 1"), "{e}");
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        "ordo_op_r2b_claims",
        "--from-genesis",
    ]);
    assert_eq!(code, 3, "{out}{err}");
    assert!(err.contains("index 1"), "{err}");
}

/// A checkpoint the operator cannot verify is not an anchor. Answering
/// "verified" against a marker signed by a key nobody vouches for would be
/// the whole property inverted: the suffix would check out against a head
/// an attacker chose.
#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_that_does_not_verify_is_refused_as_an_anchor() {
    let pool = fresh("ordo_op_forgery").await;
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    let attacker = Ed25519Signer::from_seed("chk-op", &[99u8; 32]);
    undo::sign_checkpoint(
        &pool,
        &space(),
        &attacker,
        Timestamp(500),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("the database is not the control — it stores this happily");

    let honest = Ed25519Signer::from_seed("chk-op", &[21u8; 32]);
    let receiver =
        CheckpointVerifier::from_public_hex("chk-op", &honest.verifier().public_hex()).unwrap();
    let e = undo::choose_anchor(Db::Pool(&pool), &space(), Some(&receiver))
        .await
        .expect_err("a forged checkpoint is not an anchor");
    assert!(
        matches!(e, undo::VerifyFailure::UnverifiedCheckpoint { upto: 3, .. }),
        "{e}"
    );
}

// ── checkpoint ───────────────────────────────────────────────────────────

/// The checkpoint is signed by the real signer, stored, and checkable by a
/// receiver who holds the published public key and nothing else — which is
/// the property the design requires, since a receiver that needed the
/// signing key to check a signature could forge one.
#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_signs_the_head_and_verifies_from_the_public_key_alone() {
    let pool = fresh("ordo_op_checkpoint").await;
    for n in 0..4 {
        append(&pool, &group(n)).await;
    }
    let signer = Ed25519Signer::from_seed("chk-live", &[7u8; 32]);
    let Checkpointed::Signed(cp) = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(900),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint") else {
        panic!("expected a fresh signature")
    };
    assert_eq!(cp.upto, 4, "`upto` is max(idx) + 1, not a row count");
    assert_eq!(cp.key_id, "chk-live");

    let receiver =
        CheckpointVerifier::from_public_hex("chk-live", &signer.verifier().public_hex()).unwrap();
    let stored = undo::latest_checkpoint(Db::Pool(&pool), &space())
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(stored, cp);
    assert!(
        receiver.verify_checkpoint(&stored),
        "the public key alone must be enough to check it"
    );

    // Asking again at the same head signs nothing and says so.
    assert!(matches!(
        undo::sign_checkpoint(
            &pool,
            &space(),
            &signer,
            Timestamp(950),
            &Witness::new(pool.clone())
        )
        .await,
        Ok(Checkpointed::AlreadyAt(4))
    ));
}

/// A checkpoint is a signature over history, so it is not given to history
/// that does not verify: a marker over a tampered chain is worse than no
/// marker, because every downstream receiver would then hold a signature
/// that makes the damage look vouched for.
///
/// This is a space with no prior checkpoint, deliberately. Once a verified
/// checkpoint exists, a later one inherits its prefix on the earlier
/// signature and re-verifies only the suffix — which is the same rule that
/// makes pruning legal, stated here so the limit is on the record rather
/// than discovered.
#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_is_refused_over_history_that_does_not_verify() {
    let pool = fresh("ordo_op_refuse_sign").await;
    for n in 0..4 {
        append(&pool, &group(n)).await;
    }
    let mut doctored = group(2);
    doctored.commands[0].after = Some(img(&[("id", Value::Id("task_2".into()))]));
    doctor_body(&pool, 2, &doctored).await;

    let signer = Ed25519Signer::from_seed("chk-live", &[7u8; 32]);
    let e = undo::sign_checkpoint(
        &pool,
        &space(),
        &signer,
        Timestamp(1_000),
        &Witness::new(pool.clone()),
    )
    .await
    .expect_err("signing over a broken chain is refused");
    assert!(format!("{e}").contains("index 2"), "{e}");
    assert!(
        undo::latest_checkpoint(Db::Pool(&pool), &space())
            .await
            .unwrap()
            .is_none(),
        "the refusal wrote nothing"
    );
}

// ── revert ───────────────────────────────────────────────────────────────

/// The round trip: a write, a revert, the row back to its prior value —
/// and the revert itself in the chain **as new history**, appended after
/// the group it undoes, rather than as a hole where that group used to be.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_round_trips_a_write_and_lands_in_the_chain_as_new_history() {
    let (pool, pg, f, schema) = seeded("ordo_op_revert").await;
    let x = logged(&pool, &f);
    let log = Arc::new(Mutex::new(WriteLog::new()));

    x.write_as(
        f.clone(),
        log.clone(),
        r#"renameTask($task: "task_1", $title: "renamed by ana")"#.into(),
        "mem_1".into(),
    )
    .await
    .expect("Ana may write");
    assert_eq!(
        pg.query(r#"SELECT "title", "rev" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0],
        vec![Some("renamed by ana".into()), Some("2".into())]
    );

    let written = correlation_at(&pool, &space(), 0).await;
    let r = undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &written,
        "mem_1",
        Timestamp(50_000),
    )
    .await
    .expect("Ana may undo her own write");

    // The row is back.
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("draft spec".into()),
        "the revert restored the pre-write value"
    );
    // And `rev` went FORWARD. A revert is a write; restoring the old
    // revision number would make the log claim the row had been written one
    // time fewer than it has, and would let a caller holding a stale `rev`
    // win a compare-and-set it should lose.
    assert_eq!(
        pg.query(r#"SELECT "rev" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("3".into())
    );

    // New history, not a rewrite: the original group is untouched at index
    // 0 and the inverse is a second link at index 1.
    assert_eq!(chain_len(&pool, &space()).await, 2);
    assert_eq!(r.index, 1);
    assert_eq!(correlation_at(&pool, &space(), 0).await, written);
    assert_eq!(r.group.correlation.0, format!("rev-{}", written.0));
    assert_eq!(
        r.group.actor,
        Actor::User("mem_1".into()),
        "the revert is attributed to whoever asked for it"
    );
    let cmd = &r.group.commands[0];
    assert_eq!(cmd.kind, CommandKind::Converge);
    assert_eq!(cmd.row_id, "task_1");
    // The recorded images are what the row actually was and became — read
    // back from the database, not copied from the plan.
    assert_eq!(
        cmd.before.as_ref().unwrap().get("title"),
        Some(&Value::Text("renamed by ana".into()))
    );
    assert_eq!(
        cmd.after.as_ref().unwrap().get("title"),
        Some(&Value::Text("draft spec".into()))
    );
    assert_eq!(cmd.after.as_ref().unwrap().get("rev"), Some(&Value::Int(3)));

    // The whole chain still verifies with the revert in it.
    undo::verify(
        Db::Pool(&pool),
        &space(),
        Anchor::Genesis,
        ChainHash::GENESIS,
    )
    .await
    .expect("a revert is history the chain accepts");

    // And it cannot be done twice: `(space, correlation)` is unique and the
    // inverse's id is derived, so the chain itself is what says "already
    // reverted".
    let again = undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &written,
        "mem_1",
        Timestamp(50_001),
    )
    .await
    .expect_err("a group is reverted once");
    assert!(
        matches!(again, undo::RevertFailure::AlreadyReverted(_)),
        "{again}"
    );
    assert_eq!(
        chain_len(&pool, &space()).await,
        2,
        "the refusal left no link"
    );
}

/// The other two inversions, against real rows: a create is undone by
/// deleting what it made, and a delete is undone by putting the row back
/// with the values the log recorded.
#[tokio::test(flavor = "multi_thread")]
async fn reverting_a_create_deletes_the_row_and_reverting_a_delete_restores_it() {
    let (pool, pg, f, schema) = seeded("ordo_op_create_delete").await;
    let x = logged(&pool, &f);
    let log = Arc::new(Mutex::new(WriteLog::new()));

    // ── a create, undone ────────────────────────────────────────────────
    x.write_as(
        f.clone(),
        log.clone(),
        r#"addTask($project: "proj_1", $title: "temporary")"#.into(),
        "mem_1".into(),
    )
    .await
    .expect("Ana may create");
    let made = one(
        &pool,
        r#"SELECT "id" FROM "tasks" WHERE "title" = 'temporary'"#,
    )
    .await
    .expect("the created row");

    let created_group = correlation_at(&pool, &space(), 0).await;
    let r = undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &created_group,
        "mem_1",
        Timestamp(60_000),
    )
    .await
    .expect("a create is undone by a delete");
    assert_eq!(r.group.commands[0].kind, CommandKind::Delete);
    assert_eq!(
        one(
            &pool,
            r#"SELECT "id" FROM "tasks" WHERE "title" = 'temporary'"#
        )
        .await,
        None,
        "the created row is gone"
    );
    // The delete's before-image is the row as it stood, so the history says
    // what was destroyed.
    assert_eq!(
        r.group.commands[0].before.as_ref().unwrap().get("id"),
        Some(&Value::Id(made))
    );

    // ── a delete, undone ────────────────────────────────────────────────
    // `purgeDone` removes task_2 (the only done task in proj_1). Nothing
    // references it, so refuse-with-referrers has nothing to say.
    x.write_as(
        f.clone(),
        log.clone(),
        r#"purgeDone($project: "proj_1")"#.into(),
        "mem_1".into(),
    )
    .await
    .expect("Ana may purge");
    assert_eq!(
        one(&pool, r#"SELECT "id" FROM "tasks" WHERE "id" = 'task_2'"#).await,
        None
    );

    let deleted_group = correlation_at(&pool, &space(), 2).await;
    let r = undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &deleted_group,
        "mem_1",
        Timestamp(61_000),
    )
    .await
    .expect("a delete is undone by a create");
    assert_eq!(r.group.commands[0].kind, CommandKind::Create);
    assert_eq!(
        pg.query(r#"SELECT "title", "status", "project" FROM "tasks" WHERE "id" = 'task_2'"#)
            .unwrap()[0],
        vec![
            Some("review spec".into()),
            Some("done".into()),
            Some("proj_1".into())
        ],
        "the row came back with the values the log recorded"
    );

    // Four links: two writes and two reverts, in that order, all verifying.
    assert_eq!(chain_len(&pool, &space()).await, 4);
    undo::verify(
        Db::Pool(&pool),
        &space(),
        Anchor::Genesis,
        ChainHash::GENESIS,
    )
    .await
    .expect("verifies");
}

/// **Who may revert.** A revert of somebody else's write is a write on
/// their rows, and there is no separate undo permission to appeal to — so
/// the reverter is put in the guard slot and refused exactly as an ordinary
/// write would refuse them.
///
/// Bram is a member of Ana's project (`pm_2`) with `canWrite: false`, so he
/// can *read* task_1 and cannot write it. He may not undo Ana's rename.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_is_refused_to_a_principal_who_could_not_have_made_the_write() {
    let (pool, pg, f, schema) = seeded("ordo_op_authz").await;
    let x = logged(&pool, &f);
    let log = Arc::new(Mutex::new(WriteLog::new()));

    x.write_as(
        f.clone(),
        log.clone(),
        r#"renameTask($task: "task_1", $title: "ana's title")"#.into(),
        "mem_1".into(),
    )
    .await
    .expect("Ana may write");
    let written = correlation_at(&pool, &space(), 0).await;

    let e = undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &written,
        "mem_2",
        Timestamp(70_000),
    )
    .await
    .expect_err("Bram may not write this row, so he may not unwrite it");
    assert!(format!("{e}").contains("access_guard"), "{e}");

    // Refused means refused: no effect, and no link claiming one.
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("ana's title".into())
    );
    assert_eq!(chain_len(&pool, &space()).await, 1);

    // A principal the guard does admit succeeds on the same group, which is
    // what makes the refusal above a decision rather than a breakage.
    undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &written,
        "mem_1",
        Timestamp(70_001),
    )
    .await
    .expect("Ana may");
}

/// A row somebody has written since is not something to overwrite quietly.
/// The inverse was computed against a snapshot; `rev` is the platform's own
/// belief-about-the-row mechanism (§6 rule 9), and this uses it.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_written_since_the_group_refuses_with_cas_miss() {
    let (pool, pg, f, schema) = seeded("ordo_op_casmiss").await;
    let x = logged(&pool, &f);
    let log = Arc::new(Mutex::new(WriteLog::new()));

    x.write_as(
        f.clone(),
        log.clone(),
        r#"renameTask($task: "task_1", $title: "first")"#.into(),
        "mem_1".into(),
    )
    .await
    .unwrap();
    let first = correlation_at(&pool, &space(), 0).await;
    x.write_as(
        f.clone(),
        log.clone(),
        r#"renameTask($task: "task_1", $title: "second")"#.into(),
        "mem_1".into(),
    )
    .await
    .unwrap();

    let e = undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &first,
        "mem_1",
        Timestamp(80_000),
    )
    .await
    .expect_err("the row moved on");
    assert!(format!("{e}").contains("cas_miss"), "{e}");
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("second".into()),
        "the later write survives"
    );
    // Undoing the LATEST group is the supported move, and it works.
    let latest = correlation_at(&pool, &space(), 1).await;
    undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &latest,
        "mem_1",
        Timestamp(80_001),
    )
    .await
    .expect("the most recent group reverts");
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("first".into())
    );
}

/// Two refusals that are about the *shape* of the request rather than the
/// rows: a group that belongs to another space, and a group that is not
/// logged at all. The first is loud and distinct on purpose — a cross-space
/// revert attempt is worth alerting on, and answering it identically to a
/// typo hides the one case that is.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_in_another_space_and_a_group_that_does_not_exist_answer_differently() {
    let (pool, _pg, f, schema) = seeded("ordo_op_scope").await;
    let other = SpaceId("spc_someone_else".into());
    let mut theirs = group(1);
    theirs.commands[0].space = other.clone();
    let tx = pool.begin().await.unwrap();
    tx.append_group(&theirs).await.unwrap();
    tx.commit().await.unwrap();
    drop(f);

    let e = undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &CorrelationId("cg_1".into()),
        "mem_1",
        Timestamp(90_000),
    )
    .await
    .expect_err("a group is single-space and reverts never cross spaces");
    assert!(format!("{e}").contains("single-space"), "{e}");

    let e = undo::revert(
        &pool,
        &schema,
        NO_KEYS,
        None,
        None,
        &space(),
        &CorrelationId("cg_nothing".into()),
        "mem_1",
        Timestamp(90_001),
    )
    .await
    .expect_err("no such group");
    assert!(format!("{e}").contains("no logged group"), "{e}");
}

/// `--dry-run`: the inverse computed and checked, nothing applied. An
/// operator about to undo somebody's afternoon should be able to read the
/// plan first.
#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_computes_the_inverse_and_touches_nothing() {
    let (pool, pg, f, schema) = seeded("ordo_op_dryrun").await;
    let x = logged(&pool, &f);
    let log = Arc::new(Mutex::new(WriteLog::new()));
    x.write_as(
        f.clone(),
        log.clone(),
        r#"renameTask($task: "task_1", $title: "planned")"#.into(),
        "mem_1".into(),
    )
    .await
    .unwrap();
    let written = correlation_at(&pool, &space(), 0).await;

    let plan = undo::plan_revert(
        &pool,
        &schema,
        NO_KEYS,
        &space(),
        &written,
        "mem_1",
        Timestamp(95_000),
    )
    .await
    .expect("the inverse computes");
    assert_eq!(plan.commands[0].kind, CommandKind::Converge);
    assert_eq!(
        plan.commands[0].after.as_ref().unwrap().get("title"),
        Some(&Value::Text("draft spec".into())),
        "the plan targets the pre-write value"
    );
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("planned".into()),
        "a plan is not an application"
    );
    assert_eq!(chain_len(&pool, &space()).await, 1);
}

// ── the binary itself ────────────────────────────────────────────────────

fn ordo(args: &[&str]) -> (String, String, i32) {
    let out = support::ordo()
        .args(args)
        .env("ORDO_PG_HOST", &Pg::default().host)
        .env("ORDO_PG_PORT", &Pg::default().port)
        .env("ORDO_PG_USER", &Pg::default().user)
        // The in-process suite sets these for its own end-to-end run; a
        // subprocess must not inherit a verifying key nobody asked it to
        // use, or the answer depends on which test ran first.
        .env_remove("ORDO_CHECKPOINT_KEY_ID")
        .env_remove("ORDO_CHECKPOINT_PUBKEY")
        .output()
        .expect("run ordo");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// The exit codes an operator's cron actually reads. Clean is 0; a broken
/// chain is 3, distinct from 1 — because "the chain at index 2 does not
/// recompute" and "I could not reach the database" must page different
/// people, and a cron job that only knows `!= 0` will treat a network blip
/// as evidence of tampering, cry wolf once, and be turned off.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_exits_0_on_a_clean_chain_and_3_on_a_broken_one() {
    let pool = fresh("ordo_op_exit").await;
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    drop(pool);

    let (out, _, code) = ordo(&["verify", "--space", "conformance", "--db", "ordo_op_exit"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("VERIFIED 3 groups"), "{out}");
    assert!(out.contains("anchor  genesis"), "{out}");

    let pool = Pool::connect(PoolConfig {
        db: "ordo_op_exit".into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .unwrap();
    let mut doctored = group(1);
    doctored.commands[0].after = Some(img(&[("id", Value::Id("task_1".into()))]));
    doctor_body(&pool, 1, &doctored).await;
    drop(pool);

    let (_, err, code) = ordo(&["verify", "--space", "conformance", "--db", "ordo_op_exit"]);
    assert_eq!(code, 3, "a broken chain is its own exit code: {err}");
    assert!(
        err.contains("content hash mismatch at index 1"),
        "the failure names where and what kind: {err}"
    );

    // A typo'd flag is usage (2), not a silently different question.
    let (_, err, code) = ordo(&["verify", "--spce", "conformance"]);
    assert_eq!(code, 2, "{err}");

    // And a missing subject is usage too, rather than a verification of
    // whatever the environment happened to point at.
    let (_, _, code) = ordo(&["verify", "--db", "ordo_op_exit"]);
    assert_eq!(code, 2);
}

/// The whole surface driven the way an operator drives it: checkpoint the
/// head with the real signer, verify from that checkpoint, revert a group,
/// and read the revert back out of the chain — all through
/// `ordo_cli::operator::run`, which is what `main` calls.
///
/// A plain `#[test]`: `operator::run` builds its own runtime, exactly as
/// `main` does, so it cannot be called from inside one. Setup gets a
/// runtime of its own and gives it back before the commands run — which is
/// itself a small proof that the surface is a *program*, not a library
/// call dressed up as one.
#[test]
fn the_three_commands_compose_the_way_an_operator_uses_them() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (pg, written) = rt.block_on(async {
        let (pool, pg, f, _schema) = seeded("ordo_op_endtoend").await;
        let x = logged(&pool, &f);
        let log = Arc::new(Mutex::new(WriteLog::new()));
        x.write_as(
            f.clone(),
            log.clone(),
            r#"renameTask($task: "task_1", $title: "operator test")"#.into(),
            "mem_1".into(),
        )
        .await
        .unwrap();
        let written = correlation_at(&pool, &space(), 0).await;
        (pg, written)
    });
    drop(rt);

    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ordo-conformance/fixtures/market.ordo"
    );
    let db = ["--db", "ordo_op_endtoend"];
    let space_args = ["--space", "conformance"];

    // These run in-process, so the signing key is set here rather than in
    // the child's environment. Unset would be a refusal, which is the
    // point: there is no default checkpoint key.
    let _env = CheckpointEnv::set(
        "0505050505050505050505050505050505050505050505050505050505050505",
        "chk-operator",
    );

    let args = |extra: &[&str]| -> Vec<String> {
        space_args
            .iter()
            .chain(db.iter())
            .chain(extra.iter())
            .map(|s| s.to_string())
            .collect()
    };

    let cp = operator::run("checkpoint", &args(&[]));
    assert_eq!(cp.code, 0, "{}", cp.report);
    assert!(
        cp.report.contains("SIGNED  checkpoint upto 1"),
        "{}",
        cp.report
    );
    let published = cp
        .report
        .lines()
        .find_map(|l| l.strip_prefix("verify  publish this to receivers: "))
        .expect("the verifying key is published in the report")
        .to_string();

    // Verification now anchors at that checkpoint, from the published key
    // alone.
    let v = operator::run(
        "verify",
        &args(&["--key-id", "chk-operator", "--public-key", &published]),
    );
    assert_eq!(v.code, 0, "{}", v.report);
    assert!(
        v.report.contains("anchor  checkpoint upto=1"),
        "{}",
        v.report
    );

    // The revert, through the same front door.
    let mut revert_args = vec![fixture_path.to_string()];
    revert_args.extend(args(&["--group", &written.0, "--as", "mem_1"]));
    let r = operator::run("revert", &revert_args);
    assert_eq!(r.code, 0, "{}", r.report);
    assert!(r.report.contains("REVERTED as new history"), "{}", r.report);
    assert!(
        r.report
            .contains(r#"title: "operator test" -> "draft spec""#),
        "the report shows the fields that moved: {}",
        r.report
    );
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("draft spec".into())
    );

    // A revert as somebody the guard refuses exits 1, not 0, and says why.
    let mut refused = vec![fixture_path.to_string()];
    refused.extend(args(&["--group", &written.0, "--as", "mem_2"]));
    let refused = operator::run("revert", &refused);
    assert_eq!(refused.code, 1, "{}", refused.report);

    // And the chain, with the revert in it, still verifies from the
    // checkpoint that predates it.
    let v = operator::run(
        "verify",
        &args(&["--key-id", "chk-operator", "--public-key", &published]),
    );
    assert_eq!(v.code, 0, "{}", v.report);
    assert!(v.report.contains("VERIFIED 1 group "), "{}", v.report);
}

/// `ordo prune` — 5f's operator surface for the one command that destroys
/// audit evidence.
///
/// Three things it has to get right, and none of them is the delete:
///
/// - **`--dry-run` needs no key and answers the question an operator has**:
///   where would you cut, and what is stopping you. It names the floor, the
///   bound, and the holder.
/// - **A real prune without a verifying key is refused as usage**, not run
///   with its checks skipped. Both of the prune's safety properties — the
///   cut point verifies, and the prefix about to be destroyed verifies —
///   need the key, so a keyless prune would be a prune with both of them
///   silently off.
/// - **A deployment that has never checkpointed has nothing to prune, and
///   says so.** A checkpoint is the only legal cut point, so with none
///   signed there is nowhere lawful to cut; that is the ordinary state of a
///   young deployment and it is reported, not refused.
///
/// The bound here is the **retained tail** rather than the horizon, because
/// the conformance write path stamps its groups from the harness clock and
/// they are all older than ninety days. That makes this the case the tail
/// bound exists for: without it the floor would sit past the last entry.
#[test]
fn prune_shows_the_floor_before_it_would_destroy_anything() {
    let db = "ordo_5f_cli_prune";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let (pool, _pg, f, _schema) = seeded(db).await;
        let x = logged(&pool, &f);
        let log = Arc::new(Mutex::new(WriteLog::new()));
        for title in ["one", "two", "three"] {
            x.write_as(
                f.clone(),
                log.clone(),
                format!(r#"renameTask($task: "task_1", $title: "{title}")"#),
                "mem_1".into(),
            )
            .await
            .unwrap();
        }
    });
    drop(rt);

    let (out, err, code) = ordo(&["prune", "--space", "conformance", "--db", db, "--dry-run"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("space   conformance"), "{out}");
    assert!(
        out.contains("policy  undoHorizon 90d"),
        "the horizon that produced the floor is printed, not assumed: {out}"
    );
    assert!(
        out.contains("horizon at position 3"),
        "the horizon is resolved to a position before anything is compared: {out}"
    );
    assert!(
        out.contains("bound   2 — held by the retained tail"),
        "the holder is named, or the report is one an operator cannot act on: {out}"
    );
    assert!(
        out.contains("floor   none"),
        "a checkpoint is the only legal cut point and there is none: {out}"
    );

    // The same state, pruned for real: unchanged, with the reason.
    let (out, err, code) = ordo(&[
        "prune",
        "--space",
        "conformance",
        "--db",
        db,
        "--key-id",
        "chk-nobody",
        "--public-key",
        &"00".repeat(32),
    ]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("UNCHANGED  no checkpoint at or below the live bound"),
        "{out}"
    );

    // No key, and not a dry run: usage, and the message says which of the
    // two the operator wanted.
    let (_, err, code) = ordo(&["prune", "--space", "conformance", "--db", db]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--dry-run` needs no key"), "{err}");

    // A typo'd flag is usage, never a prune of something else.
    let (_, err, code) = ordo(&["prune", "--spce", "conformance", "--db", db]);
    assert_eq!(code, 2, "{err}");
}

// ── R2d-iv: the verifier reads the witness and the receivers' heads ─────

/// Write a receiver's `ordo.attestation/1` document for a signed head, the
/// way `webhook::payload` builds the one a sink is sent.
fn attestation_file(path: &str, a: &ordo_exec::cadence::Attestation, owner_key_id: &str) -> String {
    let sink = ordo_exec::webhook::AttestSink {
        space: a.space.clone(),
        destination: "https://receiver.example/ordo".into(),
        owner_key_id: owner_key_id.to_string(),
        owner_key: String::new(),
        destination_sig: Vec::new(),
        secret: String::new(),
        created_at: 0,
        cursor: 0,
    };
    let bytes = ordo_exec::webhook::payload(a, &sink, &ordo_exec::cadence::LogPolicy::DEFAULT, 0);
    let dir = std::env::temp_dir().join(format!("ordo-r2div-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let full = dir.join(path);
    std::fs::write(&full, bytes).unwrap();
    full.to_string_lossy().into_owned()
}

/// An attestation for a head at `upto`, signed by `signer`.
fn attestation_at(
    space: &SpaceId,
    upto: u64,
    head: ChainHash,
    at: u64,
    signer: &Ed25519Signer,
) -> ordo_exec::cadence::Attestation {
    use ordo_commandlog::Signer as _;
    ordo_exec::cadence::Attestation {
        space: space.clone(),
        upto,
        head,
        at: Timestamp(at),
        key_id: signer.key_id().to_string(),
        signature: signer.sign(&ordo_commandlog::Checkpoint::signed_bytes(
            space,
            upto,
            head,
            Timestamp(at),
        )),
    }
}

/// **`ordo verify --control-db` catches a whole-database restore that the
/// space's own tables cannot see**, and stops catching it once the restore
/// is recorded.
#[tokio::test(flavor = "multi_thread")]
async fn verify_reads_the_witness_and_fails_on_an_unrecorded_whole_database_restore() {
    const DB: &str = "ordo_op_r2div_witness";
    let pool = fresh(DB).await;
    let w = Witness::new(pool.clone());
    let signer = Ed25519Signer::from_seed("chk-op", &[85u8; 32]);
    for n in 0..6 {
        append(&pool, &group(n)).await;
    }
    undo::sign_checkpoint(&pool, &space(), &signer, Timestamp(600), &w)
        .await
        .expect("a checkpoint at 6");

    // The shape a whole-database restore leaves.
    rewind(&pool, 4).await;
    pool.rows("DELETE FROM \"$checkpoints\" WHERE \"space\" = 'conformance'")
        .await
        .unwrap();
    drop(pool);

    // Without the flag, nothing outside this space's tables is read, and
    // the output says so rather than implying agreement.
    let (out, err, code) = ordo(&["verify", "--space", "conformance", "--db", DB]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("witness  not read (--control-db)"), "{out}");

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        DB,
        "--control-db",
        DB,
    ]);
    assert_eq!(code, 3, "an unrecorded restore is not lawful: {out}{err}");
    assert!(
        err.contains("the chain no longer reaches a head that the control database"),
        "{err}"
    );

    // Recorded, and the same command reads the witness line as explained.
    let pool = Pool::connect(PoolConfig {
        db: DB.into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .unwrap();
    let w = Witness::new(pool.clone());
    undo::record_restore(
        &pool,
        &w,
        &space(),
        undo::RestoreRecord {
            operator: "ops@example.test".into(),
            source: pitr(1_757_000_000),
            pre_restore_head: None,
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("recorded");
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        DB,
        "--control-db",
        DB,
    ]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("orphaned, explained by restore row"), "{out}");
}

/// **A receiver's head is compared against the chain**, and a marker that
/// does not reach it explains nothing. The deployment's own records say the
/// restore was recorded; the receiver's copy says it discarded more than the
/// marker admits.
#[tokio::test(flavor = "multi_thread")]
async fn verify_compares_a_receivers_attestation_against_the_chain() {
    const DB: &str = "ordo_op_r2div_receiver";
    let pool = fresh(DB).await;
    let w = Witness::new(pool.clone());
    let signer = Ed25519Signer::from_seed("chk-op", &[87u8; 32]);
    let public = signer.verifier().public_hex();
    for n in 0..6 {
        append(&pool, &group(n)).await;
    }
    let Checkpointed::Signed(cp6) =
        undo::sign_checkpoint(&pool, &space(), &signer, Timestamp(600), &w)
            .await
            .unwrap()
    else {
        panic!("expected a fresh signature")
    };
    // What the receiver was sent and still holds.
    let file = attestation_file(
        "attest-6.json",
        &attestation_at(&space(), 6, cp6.head, 600, &signer),
        "owner-key",
    );

    // A restore to index 3, recorded with a marker that attests only (3, H4)
    // — below the head the receiver holds.
    rewind(&pool, 4).await;
    pool.rows("DELETE FROM \"$checkpoints\" WHERE \"space\" = 'conformance'")
        .await
        .unwrap();
    pool.rows("DELETE FROM \"$chainwitness\"").await.unwrap();
    append(
        &pool,
        &marker(&pool, 1, 3, Some((3, hash_at(&pool, 3).await))).await,
    )
    .await;
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        DB,
        "--key-id",
        "chk-op",
        "--public-key",
        &public,
        "--attestation",
        &file,
    ]);
    assert_eq!(code, 3, "{out}{err}");
    assert!(
        err.contains("attest-6.json"),
        "the line names the file: {err}"
    );
    assert!(
        err.contains("orphaned, UNEXPLAINED") || err.contains("no longer reaches a head"),
        "{err}"
    );

    // Re-recorded with the head it really discarded, the same file reads as
    // explained and the run is green.
    let pool = Pool::connect(PoolConfig {
        db: DB.into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .unwrap();
    append(&pool, &marker(&pool, 2, 3, Some((5, cp6.head))).await).await;
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        DB,
        "--key-id",
        "chk-op",
        "--public-key",
        &public,
        "--attestation",
        &file,
    ]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("attest-6.json") && out.contains("orphaned, explained by restore at index"),
        "{out}"
    );
}

/// A receiver's head below the chain's first stored index is **reported,
/// not judged**: the chain has no opinion to offer about a position both of
/// whose confirming entries are pruned, and inventing one would turn a
/// lawful prune into an alarm.
#[tokio::test(flavor = "multi_thread")]
async fn a_receivers_attestation_below_the_floor_is_reported_not_judged() {
    const DB: &str = "ordo_op_r2div_floor";
    let pool = fresh(DB).await;
    ordo_exec::prune::ensure_tables(&pool).await.unwrap();
    let w = Witness::new(pool.clone());
    let signer = Ed25519Signer::from_seed("chk-op", &[89u8; 32]);
    let public = signer.verifier().public_hex();
    for n in 0..4 {
        append(&pool, &group(n)).await;
    }
    let head3 = hash_at(&pool, 3).await;
    let file = attestation_file(
        "attest-4.json",
        &attestation_at(&space(), 4, head3, 400, &signer),
        "owner-key",
    );
    for n in 10..14 {
        append(&pool, &group(n)).await; // indices 4..7
    }
    // A checkpoint at 8, below the retained tail, so there is a lawful cut
    // point above the receiver's position and below the head.
    undo::sign_checkpoint(&pool, &space(), &signer, Timestamp(800), &w)
        .await
        .expect("a checkpoint at 8");
    for n in 20..24 {
        append(&pool, &group(n)).await; // indices 8..11
    }

    let receiver =
        CheckpointVerifier::from_public_hex("chk-op", &signer.verifier().public_hex()).unwrap();
    let out = ordo_exec::prune::prune(
        &pool,
        &space(),
        &receiver,
        &ordo_exec::cadence::LogPolicy::DEFAULT,
        10_000_000_000,
    )
    .await
    .expect("a lawful prune");
    assert!(matches!(out, ordo_exec::prune::Pruned::Removed { .. }));
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        DB,
        "--key-id",
        "chk-op",
        "--public-key",
        &public,
        "--attestation",
        &file,
    ]);
    assert_eq!(code, 0, "a prune is not a restore: {out}{err}");
    assert!(
        out.contains("attest-4.json") && out.contains("below the floor"),
        "{out}"
    );
}

/// A document about another chain answers a question nobody asked. Usage,
/// so it cannot be mistaken for a verdict about this space.
#[tokio::test(flavor = "multi_thread")]
async fn an_attestation_for_another_space_is_a_usage_error() {
    const DB: &str = "ordo_op_r2div_other";
    let pool = fresh(DB).await;
    let signer = Ed25519Signer::from_seed("chk-op", &[91u8; 32]);
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    let head = hash_at(&pool, 2).await;
    let file = attestation_file(
        "attest-elsewhere.json",
        &attestation_at(&SpaceId("somewhere_else".into()), 3, head, 300, &signer),
        "owner-key",
    );
    drop(pool);

    let (out, err, code) = ordo(&[
        "verify",
        "--space",
        "conformance",
        "--db",
        DB,
        "--attestation",
        &file,
    ]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("somewhere_else"), "{err}");
}

/// The flags are additive: without them, the output is exactly what it was.
#[tokio::test(flavor = "multi_thread")]
async fn verify_without_the_new_flags_prints_what_it_printed_before() {
    const DB: &str = "ordo_op_r2div_unchanged";
    let pool = fresh(DB).await;
    for n in 0..3 {
        append(&pool, &group(n)).await;
    }
    drop(pool);

    let (out, err, code) = ordo(&["verify", "--space", "conformance", "--db", DB]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("VERIFIED 3 groups"), "{out}");
    assert!(
        !out.contains("receiver"),
        "no receiver was supplied, so no receiver line: {out}"
    );
}

// ── usage (17n M3) ───────────────────────────────────────────────────────

/// A closed period read twice through `ordo usage`, as two separate
/// process runs, prints byte-identical stdout both times — `render`'s
/// determinism (`ordo_exec::usage::render`), proved at the CLI rather than
/// only as a unit test.
#[test]
fn two_usage_runs_over_a_closed_period_are_byte_identical() {
    const DB: &str = "ordo_cli_usage_byte_identical";
    const SPACE: &str = "spc_cli_usage_byte_identical";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh(DB).await;
        pool.rows(&format!(
            "DELETE FROM \"$usage_replica\" WHERE \"space\" = {}",
            ordo_exec::sql_lit(SPACE)
        ))
        .await
        .unwrap();
        let (period_start, period_end) = ordo_exec::usage::period_of(1_700_000_000, 3_600);
        let row = ordo_exec::usage::ReplicaRow {
            space: SpaceId(SPACE.to_string()),
            period_start,
            period_end,
            replica: "r1".to_string(),
            requests: [("sql".to_string(), 3u64)].into_iter().collect(),
            rows_returned: 5,
            worker_runs: 1,
            state: ordo_exec::usage::ReplicaState::Closed,
            method_version: ordo_exec::usage::METHOD_VERSION.to_string(),
        };
        ordo_exec::usage::add_replica(&pool, &row).await.unwrap();
    });
    drop(rt);

    let args = [
        "usage",
        "--space",
        SPACE,
        "--db",
        DB,
        "--from",
        "1699990000",
        "--to",
        "1700003600",
    ];
    let (out_a, err_a, code_a) = ordo(&args);
    let (out_b, err_b, code_b) = ordo(&args);
    assert_eq!(code_a, 0, "{err_a}");
    assert_eq!(code_b, 0, "{err_b}");
    assert_eq!(out_a, out_b);
    assert!(out_a.contains(SPACE), "{out_a}");
}

/// `ordo usage` reconstructs exactly what was written to `$usage` and
/// `$usage_replica`: every space-row column, and the `totals` `render`
/// computes by summing the replica rows.
#[test]
fn usage_output_reconstructs_the_tables_exactly() {
    const DB: &str = "ordo_cli_usage_reconstruct";
    const SPACE: &str = "spc_cli_usage_reconstruct";
    let rt = tokio::runtime::Runtime::new().unwrap();
    let period_start = rt.block_on(async {
        let pool = fresh(DB).await;
        for table in ["$usage_replica", "$usage"] {
            pool.rows(&format!(
                "DELETE FROM \"{table}\" WHERE \"space\" = {}",
                ordo_exec::sql_lit(SPACE)
            ))
            .await
            .unwrap();
        }
        let (period_start, period_end) = ordo_exec::usage::period_of(1_700_000_000, 3_600);
        let space_row = ordo_exec::usage::SpaceRow {
            space: SpaceId(SPACE.to_string()),
            period_start,
            period_end,
            chain_from: 1,
            chain_to: 9,
            writes: 4,
            rows_written: 10,
            bytes_sealed: 111,
            bytes_chain: 222,
            bytes_blob: 333,
            bytes_rows_estimated: 444,
            method_version: ordo_exec::usage::METHOD_VERSION.to_string(),
        };
        ordo_exec::usage::upsert_space(&pool, &space_row)
            .await
            .unwrap();
        for (replica, sql, rows) in [("r1", 2u64, 3u64), ("r2", 5u64, 7u64)] {
            let row = ordo_exec::usage::ReplicaRow {
                space: SpaceId(SPACE.to_string()),
                period_start,
                period_end,
                replica: replica.to_string(),
                requests: [("sql".to_string(), sql)].into_iter().collect(),
                rows_returned: rows,
                worker_runs: 1,
                state: ordo_exec::usage::ReplicaState::Closed,
                method_version: ordo_exec::usage::METHOD_VERSION.to_string(),
            };
            ordo_exec::usage::add_replica(&pool, &row).await.unwrap();
        }
        period_start
    });
    drop(rt);

    let (out, err, code) = ordo(&[
        "usage",
        "--space",
        SPACE,
        "--db",
        DB,
        "--from",
        "1699990000",
        "--to",
        "1700003600",
    ]);
    assert_eq!(code, 0, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "{out}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).expect("one JSON record");
    assert_eq!(v["space"], SPACE);
    assert_eq!(v["period_start"], period_start);
    assert_eq!(v["chain_from"], 1);
    assert_eq!(v["chain_to"], 9);
    assert_eq!(v["writes"], 4);
    assert_eq!(v["rows_written"], 10);
    assert_eq!(v["bytes_sealed"], 111);
    assert_eq!(v["bytes_chain"], 222);
    assert_eq!(v["bytes_blob"], 333);
    assert_eq!(v["bytes_rows_estimated"], 444);
    assert_eq!(v["totals"]["requests"]["sql"], 7); // 2 + 5
    assert_eq!(v["totals"]["rows_returned"], 10); // 3 + 7
    assert_eq!(v["totals"]["worker_runs"], 2);
    let replicas = v["replicas"].as_array().expect("replicas array");
    assert_eq!(replicas.len(), 2, "{out}");
    assert_eq!(replicas[0]["replica"], "r1");
    assert_eq!(replicas[1]["replica"], "r2");
}

// ── 7d: the security stream gets a front door ────────────────────────────
//
// `ordo_secevent` has had a hash-chained stream, `verify_stream`, signed
// checkpoints under its **own** key with its own domain tag, and a prune
// that refuses an unverified cut — and until this section **nothing ever
// called any of it**. `store.rs`'s own argument for the second key is that
// one key would mean *"compromising the key that signs routine domain
// attestations is also the ability to forge the record of the compromise"*.
// That argument only pays out if something checks.
//
// The board's ruling on the shape (`secevent`, *"Execute your suggested
// rollout"*): one command, two answers, reported separately, because a
// merged verdict hides which artifact moved and which one moved is the
// whole diagnosis.

use ordo_exec::secevent::PgSecurityStore;
use ordo_secevent::{EventKind, RecordedDecision, Scope, StreamCheckpoint, StreamEntry, Subject};

fn sec_signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("sec-key-front-door", &[11u8; 32])
}

fn sec_scope() -> Scope {
    Scope::Space(space())
}

/// A denial entry in `space()`'s stream.
fn denial(n: u32) -> StreamEntry {
    StreamEntry::single(
        sec_scope(),
        Subject::member("usr_1"),
        "openTasks",
        EventKind::denied(RecordedDecision::NoMembership),
        Timestamp(2_000 + u64::from(n)),
        CorrelationId(format!("cor_{n:08x}")),
    )
}

/// `fresh`, and the **stream's** tables emptied too.
///
/// `fresh` clears `$commandlog`, `$checkpoints` and `$chainwitness` and stops
/// there, which was right while nothing in this file touched the security
/// stream. Each test's database outlives the run that made it, so without
/// this the second run of a test verifies the first run's entries as well as
/// its own — and the failure is a wrong *count* on a green line, which is
/// the shape of bug that sits in a suite for months. `secevent.rs`'s own
/// harness says the same thing in its own words: a shared database "would
/// make another test's entries this test's tamper".
async fn fresh_with_stream(db: &str) -> Pool {
    let pool = fresh(db).await;
    for t in ["$secevents", "$seccheckpoints"] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }
    pool
}

/// Reopen a database this test already created, **without** clearing
/// anything. `fresh_with_stream` empties the tables, which is right at the
/// start of a test and destroys its premise halfway through — a test that
/// signs a marker through a subprocess and then edits the stream needs a
/// second connection to the same state, not a second empty database.
async fn reopen(db: &str) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("reopen")
}

async fn append_denials(pool: &Pool, scope: &Scope, n: u32) {
    let store = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    for i in 0..n {
        let mut e = denial(i);
        e.scope = scope.clone();
        store.try_append(&e).await.expect("security append");
    }
}

/// The hex seed [`sec_signer`] is built from, as a deployment sets
/// `ORDO_SECURITY_CHECKPOINT_KEY`.
///
/// The same key both ways round on purpose: a test that signs through the
/// real binary and then verifies with [`sec_key_flags`] is checking that the
/// **published** half of the key the deployment signed with is what a
/// verifier holding nothing else needs, which is the whole contract of the
/// two variables being separate.
const SEC_SEED_HEX: &str = "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b";

/// `ordo checkpoint --security …`, through the **real binary** with the
/// signing key in its own environment.
///
/// A subprocess and not `operator::run`, for a reason specific to this key:
/// `security_verifier` falls back to `ORDO_SECURITY_CHECKPOINT_KEY_ID`, and
/// these tests run in one process in parallel, so setting the signing pair
/// in-process would hand a key to
/// `security_entries_removed_from_the_end_are_caught_only_by_the_signed_checkpoint`'s
/// deliberately keyless run and make it assert the opposite of its point.
/// The child also exercises `main.rs`'s dispatch and the real exit code,
/// which is what an operator's script sees.
fn sign_through_the_binary(db: &str, scope_args: &[&str]) -> (i32, String) {
    let pg = Pg::default();
    let mut args = vec!["checkpoint", "--security", "--db", db];
    args.extend_from_slice(scope_args);
    let out = support::ordo()
        .args(&args)
        .env("ORDO_PG_HOST", &pg.host)
        .env("ORDO_PG_PORT", &pg.port)
        .env("ORDO_PG_USER", &pg.user)
        .env("ORDO_SECURITY_CHECKPOINT_KEY", SEC_SEED_HEX)
        .env(
            "ORDO_SECURITY_CHECKPOINT_KEY_ID",
            ordo_commandlog::Signer::key_id(&sec_signer()),
        )
        .output()
        .expect("run ordo");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// Sign a marker at the stream's current head, under the security key, and
/// store it — what `ordo checkpoint` will do for this artifact and what an
/// operator has today only by hand.
async fn sign_stream_checkpoint(pool: &Pool, scope: &Scope) -> StreamCheckpoint {
    use ordo_commandlog::Signer;
    let store = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let upto =
        (store.try_base_index(scope).await.unwrap() + store.try_len(scope).await.unwrap()) as u64;
    let head = store.try_head(scope).await.unwrap().expect("a head");
    let signer = sec_signer();
    let cp = StreamCheckpoint {
        scope: scope.clone(),
        upto,
        head,
        at: Timestamp(9_000),
        key_id: signer.key_id().to_string(),
        signature: signer.sign(&StreamCheckpoint::signed_bytes(
            scope,
            upto,
            head,
            Timestamp(9_000),
        )),
    };
    let tx = pool.begin().await.unwrap();
    tx.put_security_checkpoint(&cp).await.unwrap();
    tx.commit().await.unwrap();
    cp
}

/// The published half of the security key, as an operator would pass it.
fn sec_key_flags() -> Vec<String> {
    use ordo_commandlog::Signer;
    let signer = sec_signer();
    vec![
        "--security-key-id".to_string(),
        signer.key_id().to_string(),
        "--security-public-key".to_string(),
        signer.verifier().public_hex(),
    ]
}

fn verify_args(db: &str, extra: &[String]) -> Vec<String> {
    let mut v = vec![
        "--space".to_string(),
        space().0.clone(),
        "--db".to_string(),
        db.to_string(),
    ];
    v.extend_from_slice(extra);
    v
}

/// **Both artifacts, one command, two verdicts.**
#[test]
fn verify_reports_the_security_stream_as_its_own_verdict() {
    const DB: &str = "ordo_op_sec_both";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        for n in 0..3 {
            append(&pool, &group(n)).await;
        }
        append_denials(&pool, &sec_scope(), 4).await;
    });
    drop(rt);

    let v = run_op("verify", &verify_args(DB, &sec_key_flags()));
    assert_eq!(v.code, 0, "{}", v.report);
    // The command chain's verdict, unchanged.
    assert!(
        v.report.contains("VERIFIED 3 groups"),
        "the chain's own line is missing or changed: {}",
        v.report
    );
    // And the stream's, as its own paragraph with its own word.
    assert!(
        v.report.contains("security  conformance"),
        "no security paragraph: {}",
        v.report
    );
    assert!(
        v.report.contains("VERIFIED  4 entries"),
        "the stream's verdict is missing: {}",
        v.report
    );
    // The two verdicts are on different lines. A single merged line is the
    // thing the ruling forbids, so this is checked rather than assumed.
    let verdicts: Vec<&str> = v
        .report
        .lines()
        .filter(|l| l.starts_with("VERIFIED"))
        .collect();
    assert_eq!(verdicts.len(), 2, "{}", v.report);
}

/// **A broken security stream does not hide behind a green chain.**
///
/// The failure mode a merged verdict would produce is not "less detail", it
/// is a green run: the command chain is untouched and much longer, so any
/// summary that folded the two would be reporting the chain.
#[test]
fn a_broken_security_stream_does_not_hide_behind_a_green_chain() {
    const DB: &str = "ordo_op_sec_broken";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        for n in 0..3 {
            append(&pool, &group(n)).await;
        }
        append_denials(&pool, &sec_scope(), 4).await;
        // Doctor one entry's body, leaving its stored hash alone: the
        // recomputation must disagree.
        pool.rows(
            "UPDATE \"$secevents\" SET \"body\" = '\\xdeadbeef'::bytea \
             WHERE \"idx\" = 2",
        )
        .await
        .unwrap();
    });
    drop(rt);

    let v = run_op("verify", &verify_args(DB, &sec_key_flags()));
    assert_ne!(v.code, 0, "a doctored stream exited 0: {}", v.report);
    assert!(
        v.report.contains("VERIFIED 3 groups"),
        "the chain is intact and must still say so: {}",
        v.report
    );
    assert!(
        v.report.contains("FAILED    entry 2"),
        "the stream's failure does not name the entry: {}",
        v.report
    );
}

/// **And the reverse: a broken chain does not stop the stream being
/// checked.**
///
/// This is the direction that matters most and the one an early return
/// would have broken. The security stream exists to be the artifact you can
/// still believe *after* the domain record is in doubt, so "the chain is
/// broken" is precisely the moment its verdict is worth the most.
#[test]
fn a_broken_chain_does_not_stop_the_security_stream_being_checked() {
    const DB: &str = "ordo_op_sec_chain_broken";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        for n in 0..3 {
            append(&pool, &group(n)).await;
        }
        append_denials(&pool, &sec_scope(), 4).await;
        pool.rows(
            "UPDATE \"$commandlog\" SET \"body\" = '\\xdeadbeef'::bytea \
             WHERE \"space\" = 'conformance' AND \"idx\" = 1",
        )
        .await
        .unwrap();
    });
    drop(rt);

    let v = run_op("verify", &verify_args(DB, &sec_key_flags()));
    assert_ne!(v.code, 0, "{}", v.report);
    assert!(
        v.report.contains("FAILED"),
        "the chain's failure is missing: {}",
        v.report
    );
    assert!(
        v.report.contains("VERIFIED  4 entries"),
        "the stream was never checked because the chain failed first: {}",
        v.report
    );
}

/// **Entries removed from the END are caught only by the signed
/// checkpoint**, and without a key the report says so instead of printing a
/// bare green line.
///
/// The most valuable test here, because it is the one property a chain walk
/// cannot have: deleting the last N entries leaves a perfectly
/// self-consistent chain. Only a marker signed at a higher position knows
/// they were ever there.
#[test]
fn security_entries_removed_from_the_end_are_caught_only_by_the_signed_checkpoint() {
    const DB: &str = "ordo_op_sec_truncated";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        append(&pool, &group(0)).await;
        append_denials(&pool, &sec_scope(), 5).await;
        let cp = sign_stream_checkpoint(&pool, &sec_scope()).await;
        assert_eq!(cp.upto, 5);
        // Remove the last two. What is left chains perfectly.
        pool.rows("DELETE FROM \"$secevents\" WHERE \"idx\" >= 3")
            .await
            .unwrap();
    });
    drop(rt);

    // With the key: caught, and the answer says how many went.
    let v = run_op("verify", &verify_args(DB, &sec_key_flags()));
    assert_ne!(v.code, 0, "a truncated stream exited 0: {}", v.report);
    assert!(
        v.report.contains("2 entries were removed from the end"),
        "the truncation is not named: {}",
        v.report
    );

    // Without it: the surviving entries do chain, so the walk passes — and
    // the report must not let that read as a clean bill of health.
    let bare = run_op("verify", &verify_args(DB, &[]));
    assert_eq!(
        bare.code, 0,
        "the retained entries do chain to each other: {}",
        bare.report
    );
    assert!(
        bare.report.contains("VERIFIED  3 entries"),
        "{}",
        bare.report
    );
    assert!(
        bare.report
            .contains("nothing proves none was removed from the end"),
        "a keyless run reported a green stream with no caveat: {}",
        bare.report
    );
    assert!(
        bare.report.contains("a checkpoint at position 5 is stored"),
        "the report does not mention the marker it declined to use: {}",
        bare.report
    );
}

/// **The deployment stream is its own mode**, because it belongs to no space
/// and has no command chain beside it.
#[test]
fn the_deployment_security_stream_is_verified_on_its_own() {
    const DB: &str = "ordo_op_sec_deployment";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        append_denials(&pool, &Scope::Deployment, 2).await;
    });
    drop(rt);

    let mut args = vec![
        "--deployment".to_string(),
        "--db".to_string(),
        DB.to_string(),
    ];
    args.extend(sec_key_flags());
    let v = run_op("verify", &args);
    assert_eq!(v.code, 0, "{}", v.report);
    assert!(v.report.contains("security  $deployment"), "{}", v.report);
    assert!(v.report.contains("VERIFIED  2 entries"), "{}", v.report);
    // No space's chain was walked: there is none for this scope, and a
    // report that showed one would be describing a different artifact.
    assert!(
        !v.report.contains("VERIFIED 0 groups"),
        "a command chain was reported for the deployment scope: {}",
        v.report
    );

    // And naming a space with it is a usage error rather than a quiet
    // choice between two different streams.
    let mut both = args.clone();
    both.extend(["--space".to_string(), space().0.clone()]);
    let e = run_op("verify", &both);
    assert_eq!(e.code, 2, "{}", e.report);
}

// ── 7d(b): something signs the marker, in a real deployment ───────────────
//
// 7d(a) built the verifier and left `audit_stream`'s fourth check — nothing
// was removed from the end of the stream — reachable only through
// `sign_stream_checkpoint` above, a marker this test file builds by hand.
// `ordo_secevent::checkpoint` had no caller outside tests anywhere in the
// workspace, and the one thing in a running deployment that ever wrote a
// `$seccheckpoints` row was step 1 of `retain_scope`: on the retention
// cadence, only when retention was enabled, and over a head nothing had
// verified.
//
// These tests use no hand-built marker. That is their point.

/// **THE test.** Entries deleted from the end of a security stream are
/// caught by `ordo verify`, with the marker signed by the shipped
/// `ordo checkpoint --security` command and nothing else.
///
/// Every earlier version of this property went through
/// `sign_stream_checkpoint`, which is a test helper reaching into the
/// database. Here the only thing that signs is the binary an operator runs,
/// with the key in its environment, and the truncation is caught end to end.
#[test]
fn a_truncation_is_caught_by_the_marker_the_checkpoint_command_signs() {
    const DB: &str = "ordo_op_sec_signed_end";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        append(&pool, &group(0)).await;
        append_denials(&pool, &sec_scope(), 5).await;
    });
    drop(rt);

    // No hand-built marker anywhere: the command signs it.
    let (code, report) = sign_through_the_binary(DB, &["--space", &space().0]);
    assert_eq!(code, 0, "{report}");
    assert!(
        report.contains("SIGNED    checkpoint upto 5"),
        "the command did not sign at the head: {report}"
    );
    // The published verifying key is in the report, because a marker nobody
    // can check is not evidence. It must be the same key `sec_key_flags`
    // hands the verifier below.
    let published = report
        .lines()
        .find_map(|l| l.strip_prefix("verify    give this to whoever verifies: "))
        .expect("the verifying key is published in the report")
        .to_string();
    assert_eq!(
        published,
        sec_signer().verifier().public_hex(),
        "the published half must be the half a verifier is given"
    );

    // A second run inside the same head: nothing to do, and it says so
    // rather than writing a row `ON CONFLICT DO NOTHING` would swallow.
    let (code, report) = sign_through_the_binary(DB, &["--space", &space().0]);
    assert_eq!(code, 0, "{report}");
    assert!(
        report.contains("UNCHANGED the head is already checkpointed at 5"),
        "{report}"
    );

    // Now remove the last two. What is left chains perfectly — this is the
    // edit no chain walk can see.
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = reopen(DB).await;
        pool.rows("DELETE FROM \"$secevents\" WHERE \"idx\" >= 3")
            .await
            .unwrap();
    });
    drop(rt);

    let v = run_op("verify", &verify_args(DB, &sec_key_flags()));
    assert_ne!(v.code, 0, "a truncated stream exited 0: {}", v.report);
    assert!(
        v.report.contains("2 entries were removed from the end"),
        "the truncation is not named: {}",
        v.report
    );
    // The command chain beside it is untouched and must still say so: the
    // whole ruling is that one verdict cannot stand for both artifacts.
    assert!(
        v.report.contains("VERIFIED 1 group"),
        "the chain's own verdict is missing: {}",
        v.report
    );
}

/// **The signing command refuses to countersign a stream it can already see
/// has been truncated.**
///
/// The one act an operator could perform here that a verifier could not
/// later undo. A marker at the shorter position would be a *true* statement
/// about the surviving entries, which is exactly why writing it is the
/// wrong answer: it puts the deployment's own signature beside the marker
/// that disproves the shortening.
#[test]
fn the_checkpoint_command_refuses_to_countersign_a_truncated_stream() {
    const DB: &str = "ordo_op_sec_no_countersign";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        append_denials(&pool, &sec_scope(), 5).await;
    });
    drop(rt);

    let (code, _) = sign_through_the_binary(DB, &["--space", &space().0]);
    assert_eq!(code, 0);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = reopen(DB).await;
        pool.rows("DELETE FROM \"$secevents\" WHERE \"idx\" >= 3")
            .await
            .unwrap();
    });
    drop(rt);

    let (code, report) = sign_through_the_binary(DB, &["--space", &space().0]);
    // Exit 3, the verification-failed code: the operator typed a signing
    // command and was handed a verification finding.
    assert_eq!(code, 3, "{report}");
    assert!(
        report.contains("REFUSED") && report.contains("2 were removed from the end of the stream"),
        "the refusal does not say what it found: {report}"
    );

    // And still exactly one marker: the one that disproves the truncation.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let rows = rt.block_on(async {
        let pool = reopen(DB).await;
        pool.rows("SELECT count(*) FROM \"$seccheckpoints\"")
            .await
            .unwrap()[0][0]
            .clone()
    });
    drop(rt);
    assert_eq!(rows.as_deref(), Some("1"), "a second marker was written");
}

/// **A signer that cannot reach its key is an error, never a skip.**
///
/// `Ed25519Signer::from_env`'s rule and `ordo checkpoint`'s, restated for
/// this artifact because the two halves of the work answer it differently
/// and both answers are deliberate: the scheduled pass *disables itself* and
/// says so on `/health/deep`, because a background loop failing every tick
/// forever is an alert nobody can clear; this command *fails*, because
/// signing is the only thing it was asked to do and a run that signed
/// nothing and exited 0 would report success for it.
#[test]
fn signing_the_security_stream_without_its_key_is_an_error() {
    const DB: &str = "ordo_op_sec_no_key";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        append_denials(&pool, &sec_scope(), 2).await;
    });
    drop(rt);

    let pg = Pg::default();
    let out = support::ordo()
        .args([
            "checkpoint",
            "--security",
            "--space",
            &space().0,
            "--db",
            DB,
        ])
        .env("ORDO_PG_HOST", &pg.host)
        .env("ORDO_PG_PORT", &pg.port)
        .env("ORDO_PG_USER", &pg.user)
        .env_remove("ORDO_SECURITY_CHECKPOINT_KEY")
        .env_remove("ORDO_SECURITY_CHECKPOINT_KEY_ID")
        .output()
        .expect("run ordo");
    assert_eq!(out.status.code(), Some(1), "unset must not exit 0");
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        report.contains(ordo_secevent::SECURITY_KEY_ENV),
        "the failure must name the variable that was not set: {report}"
    );
}

/// **The deployment stream is signable on its own**, and naming a space with
/// `--deployment` is a usage error rather than a quiet choice between two
/// streams — `verify --deployment`'s rule, and the same wording.
#[test]
fn the_deployment_security_stream_is_checkpointed_on_its_own() {
    const DB: &str = "ordo_op_sec_cp_deployment";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        append_denials(&pool, &Scope::Deployment, 3).await;
    });
    drop(rt);

    let (code, report) = sign_through_the_binary(DB, &["--deployment"]);
    assert_eq!(code, 0, "{report}");
    assert!(report.contains("security  $deployment"), "{report}");
    assert!(report.contains("SIGNED    checkpoint upto 3"), "{report}");

    let (code, report) = sign_through_the_binary(DB, &["--deployment", "--space", &space().0]);
    assert_eq!(code, 2, "{report}");

    // Neither flag is a usage error either way round, and neither is a
    // silent default: with no scope at all the command says which two
    // spellings it takes.
    let (code, report) = sign_through_the_binary(DB, &[]);
    assert_eq!(code, 2, "{report}");
    assert!(report.contains("--deployment"), "{report}");
}

// ── the rotation, ruled 2026-09-26 (`secevent-key-rotation`) ──────────────

/// The key a deployment rotates *to*. A different seed and a different id, so
/// a marker signed by one is unreadable to a run holding only the other.
fn rotated_sec_signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("sec-key-rotated", &[23u8; 32])
}

fn key_flags_of(signer: &Ed25519Signer) -> Vec<String> {
    use ordo_commandlog::Signer;
    vec![
        "--security-key-id".to_string(),
        signer.key_id().to_string(),
        "--security-public-key".to_string(),
        signer.verifier().public_hex(),
    ]
}

/// **A key rotation must not read as tampering.**
///
/// `audit_stream`'s fourth check verifies the *newest* checkpoint, so before
/// this ruling every marker signed under the old key failed under the new one
/// and the stream reported `FAILED` — not "unverifiable", not "anchored
/// earlier", but the word this artifact uses for a rewritten record.
///
/// That matters more than an ergonomics bug. A deployment rotates its security
/// key **because it suspects a compromise**, so the moment the record most
/// needs to be readable was the moment it started crying wolf, and an operator
/// who had just rotated could not tell that finding from the real thing.
#[test]
fn a_rotated_security_key_reads_as_unreadable_not_as_tampering() {
    const DB: &str = "ordo_op_sec_rotation";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = fresh_with_stream(DB).await;
        append_denials(&pool, &sec_scope(), 4).await;
        // A marker under the OLD key, which is what a rotation leaves behind.
        sign_stream_checkpoint(&pool, &sec_scope()).await;
    });
    drop(rt);

    // Holding **only the new key**: the marker is signed by a key we do not
    // have. The stream itself still recomputes, so this is a green run with a
    // narrower claim — and it must say which claim.
    let after = run_op(
        "verify",
        &verify_args(DB, &key_flags_of(&rotated_sec_signer())),
    );
    assert_eq!(
        after.code, 0,
        "a rotation was reported as a verification failure: {}",
        after.report
    );
    assert!(
        after.report.contains("VERIFIED  4 entries"),
        "{}",
        after.report
    );
    assert!(
        after.report.contains("was not supplied"),
        "the report does not say the marker was unreadable: {}",
        after.report
    );
    assert!(
        !after.report.contains("FAILED"),
        "an unreadable marker was reported as a failure: {}",
        after.report
    );

    // **Both keys, which is what an operator passes during the overlap.** Now
    // the marker is checked, so the end-of-stream property is back.
    let mut both = key_flags_of(&sec_signer());
    both.extend(key_flags_of(&rotated_sec_signer()));
    let overlap = run_op("verify", &verify_args(DB, &both));
    assert_eq!(overlap.code, 0, "{}", overlap.report);
    assert!(
        !overlap.report.contains("was not supplied"),
        "the marker was readable and the report still says otherwise: {}",
        overlap.report
    );

    // And it is genuinely checking, not merely quiet: truncate, and the run
    // holding both keys must catch it.
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let pool = Pool::connect(PoolConfig {
            db: DB.to_string(),
            size: 2,
            acquire_timeout: Duration::from_secs(5),
            ..PoolConfig::from_env()
        })
        .await
        .expect("pool");
        pool.rows("DELETE FROM \"$secevents\" WHERE \"idx\" >= 2")
            .await
            .unwrap();
    });
    drop(rt);
    let caught = run_op("verify", &verify_args(DB, &both));
    assert_ne!(caught.code, 0, "{}", caught.report);
    assert!(
        caught
            .report
            .contains("2 entries were removed from the end"),
        "{}",
        caught.report
    );
}

/// **The two flags pair up in order, and a mismatched count is a usage
/// error.**
///
/// Silently pairing the wrong id to the wrong key would verify nothing and
/// report tampering — the exact failure this ruling exists to remove, arriving
/// through the fix for it.
#[test]
fn security_key_flags_pair_in_order_and_refuse_a_mismatched_count() {
    use ordo_commandlog::Signer;
    const DB: &str = "ordo_op_sec_rotation_pairs";
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        fresh_with_stream(DB).await;
    });
    drop(rt);

    let mut lopsided = key_flags_of(&sec_signer());
    lopsided.push("--security-key-id".to_string());
    lopsided.push(rotated_sec_signer().key_id().to_string());
    let out = run_op("verify", &verify_args(DB, &lopsided));
    assert_eq!(
        out.code, 2,
        "a mismatched pair was accepted: {}",
        out.report
    );
    assert!(out.report.contains("pair up in order"), "{}", out.report);
}
