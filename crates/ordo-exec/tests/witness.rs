//! `$chainwitness`, the control-plane record of every signed head
//! (`plan-production-ops.md` R2d-i, ruling `restore-witness` 2026-09-19).
//!
//! The property under test is one sentence: **no stored checkpoint without
//! a witness row.** Everything here is a consequence of it, including the
//! two refusals, because a refusal is how an invariant survives a failure
//! rather than being quietly broken by one.
//!
//! These run against the shared cluster the way `cadence_prune.rs` does,
//! each test in a database of its own.

use std::time::Duration;

use ordo_commandlog::{
    Actor, ChainHash, CheckpointVerifier, Command, CommandGroup, CommandId, CommandKind,
    CorrelationId, Ed25519Signer, Image, RestoreMarker, RestoreSource, Signer, SpaceId, Timestamp,
    Value,
};
use ordo_exec::undo::{self, Checkpointed, Continuity, Db};
use ordo_exec::witness::{Witness, WitnessKind};
use ordo_exec::{Pool, PoolConfig};

async fn fresh(db: &str) -> Pool {
    let _ = ordo_conformance::Pg::named(db);
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    for t in ["$commandlog", "$checkpoints"] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }
    // A previous run of `a_witness_that_cannot_be_written_signs_nothing`
    // may have left its view behind if it panicked before its own cleanup,
    // and a view of that name is exactly what stops the table being made.
    let _ = pool.rows("DROP VIEW IF EXISTS \"$chainwitness\"").await;
    let w = Witness::new(pool.clone());
    w.ensure_table().await.expect("$chainwitness");
    pool.rows("DELETE FROM \"$chainwitness\"").await.unwrap();
    pool
}

fn space() -> SpaceId {
    SpaceId("spc_witness".into())
}

fn signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("witness-key", &[71u8; 32])
}

fn img(fields: &[(&str, Value)]) -> Image {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn group(n: usize) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(format!("cg_{n}")),
        actor: Actor::User(format!("usr_{n}")),
        commands: vec![Command {
            id: CommandId(format!("cmd_{n}")),
            space: space(),
            entity: "task".into(),
            row_id: format!("tsk_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(img(&[("title", Value::Text(format!("t{n}")))])),
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

async fn append_n(pool: &Pool, range: std::ops::Range<usize>) {
    for n in range {
        append(pool, &group(n)).await;
    }
}

async fn hash_at(pool: &Pool, idx: usize) -> ChainHash {
    let rows = pool
        .rows(&format!(
            "SELECT encode(\"hash\", 'hex') FROM \"$commandlog\" \
             WHERE \"space\" = 'spc_witness' AND \"idx\" = {idx}"
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

/// `$checkpoints` as the test reads it: `(upto, head)` per row, ascending.
async fn stored(pool: &Pool) -> Vec<(u64, String)> {
    pool.rows(
        "SELECT \"upto\", encode(\"head\", 'hex') FROM \"$checkpoints\" \
         WHERE \"space\" = 'spc_witness' ORDER BY \"upto\"",
    )
    .await
    .unwrap()
    .iter()
    .map(|r| {
        (
            r[0].as_deref().unwrap().parse().unwrap(),
            r[1].clone().unwrap(),
        )
    })
    .collect()
}

// ── the invariant ────────────────────────────────────────────────────────

/// Every signed checkpoint is in the control database too, field for field
/// and signature included, so a reader holding the public key checks the
/// copy with the routine that checks the original.
#[tokio::test(flavor = "multi_thread")]
async fn every_signed_checkpoint_has_a_witness_row_with_the_same_five_fields() {
    let pool = fresh("ordo_witness_same").await;
    let w = Witness::new(pool.clone());
    let mut signed = Vec::new();
    for (n, at) in [(2usize, 200u64), (4, 400), (6, 600)] {
        append_n(&pool, signed.len() * 2..n).await;
        let Checkpointed::Signed(cp) =
            undo::sign_checkpoint(&pool, &space(), &signer(), Timestamp(at), &w)
                .await
                .expect("checkpoint")
        else {
            panic!("expected a fresh signature")
        };
        signed.push(cp);
    }

    let rows = w.rows(&space()).await.expect("witness rows");
    assert_eq!(rows.len(), 3, "one row per signed checkpoint");
    let verifier =
        CheckpointVerifier::from_public_hex("witness-key", &signer().verifier().public_hex())
            .unwrap();
    for (row, cp) in rows.iter().zip(signed.iter()) {
        assert_eq!(row.kind, WitnessKind::Checkpoint);
        assert_eq!(
            (row.upto, row.head, row.at, row.key_id.as_str()),
            (cp.upto, cp.head, cp.at, cp.key_id.as_str())
        );
        assert_eq!(row.signature, cp.signature);
        assert!(
            verifier.verify_checkpoint(&row.as_checkpoint(&space())),
            "the copy verifies the way the original does"
        );
    }
    assert!(
        rows.windows(2).all(|p| p[0].seq < p[1].seq),
        "in seq order, which is the order the boot check reads"
    );
    assert_eq!(
        w.latest_checkpoint(&space()).await.unwrap().map(|r| r.upto),
        Some(6)
    );
}

/// **A witness that cannot be written signs nothing.**
///
/// The order inside `sign_checkpoint` is chosen for exactly this: the
/// witness lands first, so a control database that will not take the row
/// rolls the whole thing back instead of leaving a checkpoint no
/// control-plane record vouches for.
///
/// The failure is arranged the way a deployment would meet it — the name
/// taken by a relation that will not accept an insert — and a view is the
/// cheapest such relation. `CREATE TABLE IF NOT EXISTS` sees a relation of
/// that name and stands down, so `ensure_table` cannot paper over it.
///
/// **Observed failing first:** before the change the checkpoint is stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_witness_that_cannot_be_written_signs_nothing() {
    let pool = fresh("ordo_witness_refuses").await;
    pool.rows("DROP TABLE IF EXISTS \"$chainwitness\"")
        .await
        .unwrap();
    // Readable and not writable, which is the shape that isolates the
    // claim: `continuity` reads the witness before anything is signed, so a
    // relation that cannot even be *read* refuses earlier and for a
    // different reason. Here the read answers "nothing witnessed" and only
    // the insert fails.
    pool.rows(
        "CREATE VIEW \"$chainwitness\" AS SELECT 0::bigint AS \"seq\", \
         ''::text AS \"space\", ''::text AS \"kind\", 0::bigint AS \"upto\", \
         ''::bytea AS \"head\", 0::bigint AS \"at\", ''::text AS \"key_id\", \
         ''::bytea AS \"signature\" WHERE false",
    )
    .await
    .unwrap();
    append_n(&pool, 0..3).await;

    let w = Witness::new(pool.clone());
    let e = undo::sign_checkpoint(&pool, &space(), &signer(), Timestamp(300), &w)
        .await
        .expect_err("the witness cannot be written");
    assert!(
        matches!(e, undo::CheckpointFailure::Unwitnessed(_)),
        "{e:?}"
    );
    assert!(
        e.to_string().contains("$chainwitness"),
        "the message names the table: {e}"
    );
    assert!(
        stored(&pool).await.is_empty(),
        "nothing was signed, so there is no checkpoint for the witness to be missing from"
    );

    pool.rows("DROP VIEW \"$chainwitness\"").await.unwrap();
}

/// The other failure direction, which is the one the write order **allows**:
/// a witness row whose checkpoint never committed. It names a head the chain
/// holds, because it was read under the append lock from committed rows, so
/// it classifies as current and nothing is alarmed. The next signing stores
/// the checkpoint and the unique key absorbs the repeat.
#[tokio::test(flavor = "multi_thread")]
async fn a_witness_row_whose_checkpoint_never_committed_is_current_and_harmless() {
    let pool = fresh("ordo_witness_orphan_row").await;
    let w = Witness::new(pool.clone());
    append_n(&pool, 0..3).await;

    // The row `sign_checkpoint` would have written, by hand, with no
    // checkpoint behind it.
    let head = hash_at(&pool, 2).await;
    w.record_checkpoint(&ordo_commandlog::Checkpoint {
        space: space(),
        upto: 3,
        head,
        at: Timestamp(300),
        key_id: "witness-key".into(),
        signature: signer().sign(&ordo_commandlog::Checkpoint::signed_bytes(
            &space(),
            3,
            head,
            Timestamp(300),
        )),
    })
    .await
    .unwrap();
    assert!(stored(&pool).await.is_empty());
    assert_eq!(
        undo::continuity(Db::Pool(&pool), &w, &space())
            .await
            .unwrap(),
        Continuity::Intact,
        "the witness names the head the chain holds"
    );

    let Checkpointed::Signed(cp) =
        undo::sign_checkpoint(&pool, &space(), &signer(), Timestamp(300), &w)
            .await
            .expect("the next signing lands")
    else {
        panic!("expected a fresh signature")
    };
    assert_eq!((cp.upto, cp.head), (3, head));
    assert_eq!(stored(&pool).await.len(), 1);
    assert_eq!(
        w.rows(&space()).await.unwrap().len(),
        1,
        "the unique key absorbed the repeat"
    );
}

/// A chain restored under a running process is refused a checkpoint at the
/// next tick rather than witnessed as current. Without this, a rewind would
/// acquire the deployment's own signature over it, which is the one way an
/// unrecorded restore could come to look vouched for.
#[tokio::test(flavor = "multi_thread")]
async fn signing_refuses_a_chain_the_witness_contradicts() {
    let pool = fresh("ordo_witness_rewind").await;
    let w = Witness::new(pool.clone());
    append_n(&pool, 0..6).await;
    let Checkpointed::Signed(cp6) =
        undo::sign_checkpoint(&pool, &space(), &signer(), Timestamp(600), &w)
            .await
            .expect("checkpoint at 6")
    else {
        panic!("expected a fresh signature")
    };

    // The rewind a `DELETE` leaves: the chain is short and the checkpoint
    // row survived to say so.
    pool.rows("DELETE FROM \"$commandlog\" WHERE \"space\" = 'spc_witness' AND \"idx\" >= 4")
        .await
        .unwrap();
    let e = undo::sign_checkpoint(&pool, &space(), &signer(), Timestamp(700), &w)
        .await
        .expect_err("a rewind is not a head to sign");
    let undo::CheckpointFailure::WouldWitnessARewind(c) = &e else {
        panic!("expected WouldWitnessARewind, got {e:?}")
    };
    assert_eq!(
        c,
        &Continuity::Rewound {
            head: Some(3),
            checkpoint_upto: 6,
            checkpoint_head: cp6.head,
            // The `DELETE` left `$checkpoints` alone, so the stored row and
            // its witness copy both say 6; the copy is the one judged.
            source: undo::RecordSource::Witness,
        }
    );

    // Recorded, the restore is lawful history and signing resumes.
    let marker = CommandGroup {
        correlation: CorrelationId("restore_1".into()),
        actor: Actor::System("restore".into()),
        commands: vec![],
        owner_administration: false,
        restored_unverified: false,
        restore: Some(RestoreMarker {
            space: space(),
            restored_from: (3, hash_at(&pool, 3).await),
            pre_restore_head: Some((5, cp6.head)),
            source: RestoreSource::Pitr {
                at: Timestamp(1_757_000_000),
            },
            operator: "ops@example.test".into(),
        }),
    };
    append(&pool, &marker).await;
    w.record_restore(
        &space(),
        4,
        hash_at(&pool, 4).await,
        "ops@example.test",
        Timestamp(700),
    )
    .await
    .unwrap();

    let Checkpointed::Signed(cp) =
        undo::sign_checkpoint(&pool, &space(), &signer(), Timestamp(800), &w)
            .await
            .expect("a recorded restore is signable history")
    else {
        panic!("expected a fresh signature")
    };
    assert_eq!(cp.upto, 5, "the head is the marker, at index 4");
    let kinds: Vec<WitnessKind> = w
        .rows(&space())
        .await
        .unwrap()
        .iter()
        .map(|r| r.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            WitnessKind::Checkpoint,
            WitnessKind::Restore,
            WitnessKind::Checkpoint
        ],
        "seq orders the two kinds against each other"
    );
}
