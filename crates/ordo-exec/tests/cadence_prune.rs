//! 5f's log operations against a real database: the two cadences, and the
//! prune.
//!
//! The properties here are the ones a reviewer would not believe from
//! reading the code, because each of them is about something *not*
//! happening:
//!
//! - attestation does not inherit checkpoint's cadence, and an attestation
//!   never becomes a prune anchor;
//! - the floor does not pass a listener or an archive sink, however stale,
//!   and does pass a stalled mirror — loudly;
//! - a prune does not proceed over an unverified anchor or a tampered
//!   prefix;
//! - a lawful prune does not leave the sealed payloads it orphaned behind,
//!   and does not make itself look like tampering afterwards.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_commandlog::{
    Actor, ChainHash, Checkpoint, Command, CommandGroup, CommandId, CommandKind, CorrelationId,
    Ed25519Signer, Image, SpaceId, Store, Timestamp, Value,
};
use ordo_exec::cadence::{
    self, Attestation, Attested, LogPolicy, ATTEST_SCHEDULE, CHECKPOINT_SCHEDULE,
};
use ordo_exec::commandlog::PgStore;
use ordo_exec::prune::{self, ConsumerKind, Idle, PruneRefusal, Pruned};
use ordo_exec::undo::{self, Anchor, Db};
use ordo_exec::witness::Witness;
use ordo_exec::{Pool, PoolConfig};

/// Each test owns its database. Every suite in this cluster writes to
/// `$commandlog`, so sharing one would make another test's entries this
/// test's tamper — and this suite *deletes* from `$commandlog`, which makes
/// the sharing hazard worse than usual.
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
    cadence::ensure_tables(&pool).await.expect("$attestations");
    prune::ensure_tables(&pool).await.expect("$logconsumers");
    for t in [
        "$commandlog",
        "$checkpoints",
        "$attestations",
        "$logconsumers",
        "$schedules",
        "$sealedpayload",
    ] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }
    pool
}

fn space() -> SpaceId {
    SpaceId("spc_alpha".into())
}

fn signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("chain-key-2026", &[9u8; 32])
}

/// A different key, for the receiver who was handed the wrong one.
fn other_signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("chain-key-2026", &[11u8; 32])
}

fn img(fields: &[(&str, Value)]) -> Image {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// A one-command create group stamped at `at`.
fn group_at(n: usize, at: u64) -> CommandGroup {
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
            at: Timestamp(at),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

/// Append `n` groups, one second apart from `base`.
async fn append_n(pool: &Pool, n: usize, base: u64) {
    for i in 0..n {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&group_at(i, base + i as u64))
            .await
            .unwrap_or_else(|e| panic!("group {i}: {e:?}"));
        tx.commit().await.unwrap();
    }
}

async fn count(pool: &Pool, table: &str) -> i64 {
    pool.rows(&format!(
        "SELECT count(*) FROM \"{table}\" WHERE \"space\" = 'spc_alpha'"
    ))
    .await
    .unwrap()[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

/// Sign a checkpoint at the current head. The scheduled path, so the test
/// exercises the same call a cron job would.
async fn checkpoint(pool: &Pool, at: u64) -> Checkpoint {
    match cadence::checkpoint_due(
        pool,
        &space(),
        &signer(),
        Timestamp(at),
        &Witness::new(pool.clone()),
    )
    .await
    .expect("checkpoint")
    {
        undo::Checkpointed::Signed(cp) => cp,
        undo::Checkpointed::AlreadyAt(u) => panic!("already checkpointed at {u}"),
    }
}

// ── the two cadences are two cadences ────────────────────────────────────

/// The ruling `security-logging-design.md` §6 states and three other
/// documents repeat: **attestation must not inherit `checkpoint`'s
/// cadence.**
///
/// Both schedules are registered from one policy. An hour passes. The
/// attestation schedule is claimable and the checkpoint schedule is not,
/// because one period is an hour and the other is a month — and if the two
/// went through a single `claim_due` call, whichever period that call was
/// handed would have become both.
#[tokio::test(flavor = "multi_thread")]
async fn the_attestation_schedule_does_not_inherit_the_checkpoint_period() {
    let pool = fresh("ordo_5f_cadence_split").await;
    let policy = LogPolicy::DEFAULT;
    cadence::register_schedules(&pool, &policy, 0)
        .await
        .unwrap();

    let hour = 3_600;
    assert!(
        !cadence::claim_checkpoint_tick(&pool, &policy, hour)
            .await
            .unwrap(),
        "a month has not passed, so the checkpoint schedule is not due"
    );
    assert!(
        cadence::claim_attest_tick(&pool, &policy, hour)
            .await
            .unwrap(),
        "an hour has passed, so the attestation schedule is due"
    );

    // And the advance each one took is its own period, not the other's.
    let due = pool
        .rows("SELECT \"name\", \"next_due\" FROM \"$schedules\" ORDER BY \"name\"")
        .await
        .unwrap();
    let at = |name: &str| -> i64 {
        due.iter()
            .find(|r| r[0].as_deref() == Some(name))
            .and_then(|r| r[1].as_deref())
            .unwrap()
            .parse()
            .unwrap()
    };
    assert_eq!(
        at(ATTEST_SCHEDULE),
        hour + policy.attest_or.as_secs() as i64
    );
    assert_eq!(
        at(CHECKPOINT_SCHEDULE),
        policy.checkpoint.as_secs() as i64,
        "the checkpoint schedule was not touched"
    );
}

/// Volume with no time, and time with no volume. `every N commands or T`,
/// whichever lands first — so each half must fire on its own.
#[tokio::test(flavor = "multi_thread")]
async fn attestation_fires_on_volume_and_on_time_independently() {
    let pool = fresh("ordo_5f_cadence_or").await;
    let policy = LogPolicy {
        attest_every: 5,
        attest_or: Duration::from_secs(3_600),
        ..LogPolicy::DEFAULT
    };

    assert_eq!(
        cadence::attest_due(&pool, &space(), &signer(), &policy, Timestamp(0))
            .await
            .unwrap(),
        Attested::EmptyChain,
        "a space with no history has no head to attest to"
    );

    // The first attestation of a non-empty chain is always due: there is no
    // previous record for either half to measure against.
    append_n(&pool, 2, 1_000).await;
    let first = match cadence::attest_due(&pool, &space(), &signer(), &policy, Timestamp(1_010))
        .await
        .unwrap()
    {
        Attested::Signed(a) => a,
        other => panic!("expected the first attestation, got {other:?}"),
    };
    assert_eq!(first.upto, 2);

    // Two more groups: below the volume threshold, and no time has passed.
    append_n2(&pool, 2..4, 1_100).await;
    assert!(matches!(
        cadence::attest_due(&pool, &space(), &signer(), &policy, Timestamp(1_020))
            .await
            .unwrap(),
        Attested::NotDue { since: 2, .. }
    ));

    // Volume alone, at the same instant as the last attestation.
    append_n2(&pool, 4..7, 1_200).await;
    let by_volume = match cadence::attest_due(&pool, &space(), &signer(), &policy, Timestamp(1_010))
        .await
        .unwrap()
    {
        Attested::Signed(a) => a,
        other => panic!("volume should have fired it: {other:?}"),
    };
    assert_eq!(by_volume.upto, 7);

    // Time alone: one group, an hour later.
    append_n2(&pool, 7..8, 1_300).await;
    assert!(matches!(
        cadence::attest_due(&pool, &space(), &signer(), &policy, Timestamp(1_020))
            .await
            .unwrap(),
        Attested::NotDue { since: 1, .. },
    ));
    let by_time = match cadence::attest_due(
        &pool,
        &space(),
        &signer(),
        &policy,
        Timestamp(1_010 + 3_600),
    )
    .await
    .unwrap()
    {
        Attested::Signed(a) => a,
        other => panic!("time should have fired it: {other:?}"),
    };
    assert_eq!(by_time.upto, 8);

    // A quiet space costs nothing: the head has not moved, so no period
    // however long makes another attestation due. Without this the table of
    // a space nobody writes to fills with re-signings of identical bytes.
    assert!(matches!(
        cadence::attest_due(&pool, &space(), &signer(), &policy, Timestamp(9_000_000))
            .await
            .unwrap(),
        Attested::NotDue { since: 0, .. }
    ));

    // Every record verifies under the published key, with the same routine
    // a checkpoint is checked with.
    let v = signer().verifier();
    for a in cadence::attestations(&pool, &space()).await.unwrap() {
        assert!(a.verify(&v), "attestation at {} does not verify", a.upto);
        assert!(
            !a.verify(&other_signer().verifier()),
            "an attestation must not verify under a key that did not sign it"
        );
    }
}

/// Helper for appending a specific index range, so a test can extend a
/// chain without restarting correlations.
async fn append_n2(pool: &Pool, range: std::ops::Range<usize>, base: u64) {
    for (k, i) in range.enumerate() {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&group_at(i, base + k as u64))
            .await
            .unwrap_or_else(|e| panic!("group {i}: {e:?}"));
        tx.commit().await.unwrap();
    }
}

/// **An attestation is not a prune anchor.** The two artifacts live in two
/// tables for exactly this reason, and this asserts the separation from
/// both sides: the pruner finds nothing to cut to while only attestations
/// exist, and it starts working the moment a real checkpoint appears at a
/// *lower* position than the attestations it ignored.
#[tokio::test(flavor = "multi_thread")]
async fn a_prune_never_anchors_on_an_attestation() {
    let pool = fresh("ordo_5f_prune_anchor").await;
    let policy = LogPolicy {
        undo_horizon: Duration::from_secs(10),
        attest_every: 1,
        ..LogPolicy::DEFAULT
    };
    append_n(&pool, 8, 1_000).await;

    // Attest at the head. Nothing else exists to cut to.
    let a = match cadence::attest_due(&pool, &space(), &signer(), &policy, Timestamp(1_100))
        .await
        .unwrap()
    {
        Attested::Signed(a) => a,
        other => panic!("expected an attestation: {other:?}"),
    };
    assert_eq!(a.upto, 8);
    assert_eq!(count(&pool, "$attestations").await, 1);
    assert_eq!(count(&pool, "$checkpoints").await, 0);

    let now = 1_000 + 8 + 100;
    let f = prune::floor(&pool, &space(), &policy, now).await.unwrap();
    assert_eq!(
        f.position, 0,
        "an attestation at position 8 must not become a floor"
    );
    assert!(f.anchor.is_none());

    let out = prune::prune(&pool, &space(), &signer().verifier(), &policy, now)
        .await
        .expect("nothing to prune is not a refusal");
    assert!(matches!(
        out,
        Pruned::NothingToPrune {
            reason: Idle::NoAnchorBelowBound,
            ..
        }
    ));
    assert_eq!(count(&pool, "$commandlog").await, 8);

    // Now a real checkpoint, deliberately below the attestation's position.
    // If the pruner had been reading `$attestations` it would already have
    // cut at 8; it cuts at 4.
    let cp = Checkpoint {
        space: space(),
        upto: 4,
        head: head_at(&pool, 3).await,
        at: Timestamp(1_050),
        key_id: ordo_commandlog::Signer::key_id(&signer()).to_string(),
        signature: ordo_commandlog::Signer::sign(
            &signer(),
            &Checkpoint::signed_bytes(&space(), 4, head_at(&pool, 3).await, Timestamp(1_050)),
        ),
    };
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp).await.unwrap();
    tx.commit().await.unwrap();

    let out = prune::prune(&pool, &space(), &signer().verifier(), &policy, now)
        .await
        .expect("lawful prune");
    match out {
        Pruned::Removed {
            floor,
            entries,
            verifiable_from,
            ..
        } => {
            assert_eq!(floor.position, 4);
            assert_eq!(entries, 4);
            assert_eq!(verifiable_from, 4);
        }
        other => panic!("expected a prune to 4: {other:?}"),
    }
}

async fn head_at(pool: &Pool, idx: u64) -> ChainHash {
    let rows = pool
        .rows(&format!(
            "SELECT encode(\"hash\",'hex') FROM \"$commandlog\" \
             WHERE \"space\" = 'spc_alpha' AND \"idx\" = {idx}"
        ))
        .await
        .unwrap();
    let hex = rows[0][0].clone().unwrap();
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    ChainHash(out)
}

// ── the floor and who holds it ───────────────────────────────────────────

/// The floor never passes the slowest registered consumer — and for three
/// of the four kinds, "however stale" is the whole point. A listener that
/// stopped a year ago still pins, because at-least-once delivery *is* the
/// floor rule seen from the routine's side; an archive sink still pins,
/// because pruning past it destroys the copy of record.
#[tokio::test(flavor = "multi_thread")]
async fn a_listener_and_an_archive_pin_the_floor_however_stale() {
    let pool = fresh("ordo_5f_floor_pins").await;
    let policy = LogPolicy {
        undo_horizon: Duration::from_secs(10),
        consumer_stall: Duration::from_secs(30),
        ..LogPolicy::DEFAULT
    };
    append_n(&pool, 8, 1_000).await;
    let cp = checkpoint(&pool, 1_050).await;
    assert_eq!(cp.upto, 8);

    // `advanced_at = 0` against a clock this far along is an idle time no
    // stall window would tolerate — which is the point: for these three
    // kinds there is no stall window.
    let now = 9_999_999;
    for (name, kind, pos) in [
        ("routine:digest", ConsumerKind::Listener, 2u64),
        ("r2://bundles", ConsumerKind::Archive, 5),
        ("device:phone", ConsumerKind::ClientFirstMirror, 6),
    ] {
        prune::register_consumer(&pool, &space(), name, kind, pos, 0)
            .await
            .unwrap();
        let f = prune::floor(&pool, &space(), &policy, now).await.unwrap();
        assert_eq!(
            f.live_bound, pos,
            "`{name}` is the only registered consumer and must hold the bound"
        );
        assert!(
            f.evicted.is_empty(),
            "`{name}` is not an evictable kind, however long it has been idle"
        );
        assert_eq!(f.position, 0, "no checkpoint sits at or below {pos}");

        // Deregistration is the supported way to stop pinning, and for
        // these three it is the only way.
        assert!(prune::deregister_consumer(&pool, &space(), name)
            .await
            .unwrap());
    }

    // With nothing pinning it, the bound is the **retained tail**: the
    // checkpoint at the head is not a legal cut point, because cutting
    // there would take the whole table. See `a_prune_never_empties_the_chain`.
    let f = prune::floor(&pool, &space(), &policy, now).await.unwrap();
    assert!(
        matches!(f.holder, prune::FloorHolder::RetainedTail { last_index: 7 }),
        "{}",
        f.holder
    );
    assert_eq!(f.live_bound, 7);
    assert_eq!(f.position, 0, "the only checkpoint sits above the bound");

    // A checkpoint below the tail is a legal cut point, and now the floor
    // is that checkpoint.
    let cp4 = signed_checkpoint(&pool, 4).await;
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp4).await.unwrap();
    tx.commit().await.unwrap();
    let f = prune::floor(&pool, &space(), &policy, now).await.unwrap();
    assert_eq!(f.position, 4);
}

/// A stalled **mirror** — and only a mirror — leaves the calculation, and
/// the outcome names it so the operator alert has content.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_mirror_is_evicted_loudly() {
    let pool = fresh("ordo_5f_floor_evict").await;
    let policy = LogPolicy {
        undo_horizon: Duration::from_secs(10),
        consumer_stall: Duration::from_secs(30),
        ..LogPolicy::DEFAULT
    };
    append_n(&pool, 8, 1_000).await;
    // Mid-chain rather than at the head: a checkpoint at the head is never
    // a legal cut point, because the last entry is always retained.
    let cp4 = signed_checkpoint(&pool, 4).await;
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp4).await.unwrap();
    tx.commit().await.unwrap();

    // The groups are stamped 1000..1007, so a clock at 2_000 with a ten
    // second horizon puts all of them safely behind it and leaves the
    // consumer as the only thing that could bind the floor.
    prune::register_consumer(&pool, &space(), "mirror:a", ConsumerKind::Mirror, 2, 1_990)
        .await
        .unwrap();

    // Inside the stall window it pins like anything else.
    let f = prune::floor(&pool, &space(), &policy, 2_000).await.unwrap();
    assert_eq!(f.live_bound, 2);
    assert!(f.evicted.is_empty());

    // Past it, it is evicted — and named.
    let f = prune::floor(&pool, &space(), &policy, 2_100).await.unwrap();
    assert_eq!(f.evicted.len(), 1);
    assert_eq!(f.evicted[0].name, "mirror:a");
    assert_eq!(f.evicted[0].position, 2);
    assert_eq!(f.evicted[0].idle_for, 110);
    assert_eq!(
        f.live_bound, 7,
        "with the mirror out of the calculation the retained tail is the bound"
    );
    assert_eq!(f.position, 4, "the checkpoint at or below it");

    // Advancing it puts it back in the calculation, and a cursor never
    // moves backwards.
    prune::advance_consumer(&pool, &space(), "mirror:a", 5, 2_100)
        .await
        .unwrap();
    prune::advance_consumer(&pool, &space(), "mirror:a", 1, 2_101)
        .await
        .unwrap();
    let f = prune::floor(&pool, &space(), &policy, 2_102).await.unwrap();
    assert!(f.evicted.is_empty());
    assert_eq!(f.live_bound, 5, "GREATEST keeps the cursor from rewinding");
    match f.holder {
        prune::FloorHolder::Consumer { ref name, .. } => assert_eq!(name, "mirror:a"),
        ref other => panic!("the mirror should hold the bound: {other}"),
    }
}

/// The undo horizon is resolved to a position by reading the `at` inside
/// each body, and the resolution is exclusive of the first entry newer than
/// the cutoff.
#[tokio::test(flavor = "multi_thread")]
async fn the_undo_horizon_resolves_to_a_position_before_it_is_compared() {
    let pool = fresh("ordo_5f_horizon").await;
    append_n(&pool, 10, 1_000).await; // ats 1000..1009

    // cutoff 1004: entries 0..=4 are at or below it, entry 5 is not.
    assert_eq!(
        prune::horizon_position(&pool, &space(), 1_004)
            .await
            .unwrap(),
        5
    );
    // Everything is older than the cutoff: the horizon is past the end.
    assert_eq!(
        prune::horizon_position(&pool, &space(), 9_999)
            .await
            .unwrap(),
        10
    );
    // Nothing is: the horizon is the first stored index.
    assert_eq!(
        prune::horizon_position(&pool, &space(), 0).await.unwrap(),
        0
    );
}

// ── the prune refuses ────────────────────────────────────────────────────

/// A cut point nobody has vouched for is not a cut point. The refusal is
/// the same objection `verify` makes to anchoring on an unverifiable
/// checkpoint, with the difference that this one cannot be undone.
#[tokio::test(flavor = "multi_thread")]
async fn a_prune_refuses_an_anchor_that_does_not_verify() {
    let pool = fresh("ordo_5f_prune_badkey").await;
    let policy = LogPolicy {
        undo_horizon: Duration::from_secs(10),
        ..LogPolicy::DEFAULT
    };
    append_n(&pool, 6, 1_000).await;
    // Below the head: the last entry is always retained, so a checkpoint at
    // 6 would never be reached and there would be nothing to refuse.
    let cp3 = signed_checkpoint(&pool, 3).await;
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp3).await.unwrap();
    tx.commit().await.unwrap();

    let e = prune::prune(
        &pool,
        &space(),
        &other_signer().verifier(),
        &policy,
        9_999_999,
    )
    .await
    .expect_err("the verifier holds the wrong key");
    assert!(
        matches!(e, PruneRefusal::UnverifiedAnchor { upto: 3, .. }),
        "{e}"
    );
    assert_eq!(count(&pool, "$commandlog").await, 6, "nothing was deleted");
}

/// Deleting a tampered prefix destroys the only evidence it was tampered
/// with, so the prune verifies what it is about to remove and refuses.
#[tokio::test(flavor = "multi_thread")]
async fn a_prune_refuses_to_destroy_a_tampered_prefix() {
    let pool = fresh("ordo_5f_prune_tampered").await;
    let policy = LogPolicy {
        undo_horizon: Duration::from_secs(10),
        ..LogPolicy::DEFAULT
    };
    append_n(&pool, 6, 1_000).await;
    let cp3 = signed_checkpoint(&pool, 3).await;
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp3).await.unwrap();
    tx.commit().await.unwrap();

    // Doctor a body inside the prefix that is about to go.
    pool.rows(
        "UPDATE \"$commandlog\" SET \"body\" = \
         overlay(\"body\" placing '\\x00'::bytea from 9 for 1) \
         WHERE \"space\" = 'spc_alpha' AND \"idx\" = 1",
    )
    .await
    .unwrap();

    let e = prune::prune(&pool, &space(), &signer().verifier(), &policy, 9_999_999)
        .await
        .expect_err("the prefix does not verify");
    assert!(
        matches!(e, PruneRefusal::WouldDestroyTamperedPrefix(_)),
        "{e}"
    );
    assert_eq!(
        count(&pool, "$commandlog").await,
        6,
        "the evidence is still there"
    );
}

// ── the prune succeeds ───────────────────────────────────────────────────

/// The whole lawful path: rows gone from the table, the retained suffix
/// still verifying from its checkpoint, a from-genesis verification saying
/// **pruned** rather than **tampered**, the orphaned sealed payloads
/// collected in the same transaction, and `verifiable_from` moved so a
/// returning client can tell which of those two it is looking at.
#[tokio::test(flavor = "multi_thread")]
async fn a_lawful_prune_removes_the_prefix_its_payloads_and_publishes_the_floor() {
    let pool = fresh("ordo_5f_prune_lawful").await;
    let policy = LogPolicy {
        undo_horizon: Duration::from_secs(10),
        ..LogPolicy::DEFAULT
    };
    append_n(&pool, 8, 1_000).await;

    // One sealed payload per group, named by the group's correlation — the
    // only thing that makes a payload reachable.
    let cspace = ordo_crypto::SpaceId(space().0.clone());
    for i in 0..8u8 {
        let tx = pool.begin().await.unwrap();
        let handle = [i; 16];
        ordo_exec::sealed::put(
            &tx,
            &cspace,
            &handle,
            1,
            "task",
            "note",
            "row:t",
            &format!("cg_{i}"),
            &[i, i, i],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(
        ordo_exec::sealed::space_payload_count(&pool, &cspace)
            .await
            .unwrap(),
        8
    );

    // A checkpoint at 5, and a consumer that has only reached 5, so the
    // floor is a consumer's rather than the horizon's.
    let cp5 = Checkpoint {
        space: space(),
        upto: 5,
        head: head_at(&pool, 4).await,
        at: Timestamp(1_050),
        key_id: ordo_commandlog::Signer::key_id(&signer()).to_string(),
        signature: ordo_commandlog::Signer::sign(
            &signer(),
            &Checkpoint::signed_bytes(&space(), 5, head_at(&pool, 4).await, Timestamp(1_050)),
        ),
    };
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp5).await.unwrap();
    tx.commit().await.unwrap();
    prune::register_consumer(
        &pool,
        &space(),
        "mirror:a",
        ConsumerKind::Mirror,
        5,
        9_999_990,
    )
    .await
    .unwrap();

    assert_eq!(
        prune::verifiable_from(&pool, &space()).await.unwrap(),
        0,
        "before the prune, recomputation is possible from genesis"
    );

    let out = prune::prune(&pool, &space(), &signer().verifier(), &policy, 9_999_999)
        .await
        .expect("lawful prune");
    let (entries, payloads, verifiable_from) = match out {
        Pruned::Removed {
            floor,
            entries,
            payloads,
            verifiable_from,
        } => {
            assert_eq!(floor.position, 5);
            assert_eq!(floor.live_bound, 5);
            match floor.holder {
                prune::FloorHolder::Consumer { ref name, .. } => assert_eq!(name, "mirror:a"),
                ref other => panic!("the mirror holds the bound: {other}"),
            }
            (entries, payloads, verifiable_from)
        }
        other => panic!("expected a prune: {other:?}"),
    };

    // The rows are gone from the table, not merely hidden.
    assert_eq!(entries, 5);
    assert_eq!(count(&pool, "$commandlog").await, 3);

    // The payloads the prune orphaned went with it, in the same
    // transaction; the ones still named are untouched.
    assert_eq!(payloads, 5);
    assert_eq!(
        ordo_exec::sealed::space_payload_count(&pool, &cspace)
            .await
            .unwrap(),
        3
    );

    // The client-facing half: recomputation now starts at the floor.
    assert_eq!(verifiable_from, 5);
    assert_eq!(prune::verifiable_from(&pool, &space()).await.unwrap(), 5);

    // The retained suffix verifies from its checkpoint …
    let v = signer().verifier();
    let (anchor, hash) = undo::choose_anchor(Db::Pool(&pool), &space(), Some(&v))
        .await
        .expect("anchor");
    assert!(matches!(anchor, Anchor::Checkpoint { upto: 5, .. }));
    undo::verify(Db::Pool(&pool), &space(), anchor, hash)
        .await
        .expect("the retained suffix verifies from the checkpoint");

    // … and from genesis the answer is **pruned**, not **tampered**. That
    // distinction is the whole reason `PrefixMissing` exists.
    let e = undo::verify(
        Db::Pool(&pool),
        &space(),
        Anchor::Genesis,
        ChainHash::GENESIS,
    )
    .await
    .expect_err("the prefix is gone");
    assert!(
        matches!(e, undo::VerifyFailure::PrefixMissing { first_stored: 5 }),
        "a lawful prune must not read as tampering: {e}"
    );

    // A second prune with nothing new to cut to is idle, not a refusal.
    let again = prune::prune(&pool, &space(), &v, &policy, 9_999_999)
        .await
        .expect("idle");
    assert!(matches!(
        again,
        Pruned::NothingToPrune {
            reason: Idle::AlreadyAtFloor,
            ..
        }
    ));
}

/// After a prune the chain no longer begins at genesis, so the *next*
/// prune's prefix check needs a checkpoint at the new base. If there is
/// none, pruning again would compound an unverifiable state, and the
/// refusal says what to do about it.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_prune_refuses_when_the_retained_base_is_unanchored() {
    let pool = fresh("ordo_5f_prune_unanchored").await;
    let policy = LogPolicy {
        undo_horizon: Duration::from_secs(10),
        ..LogPolicy::DEFAULT
    };
    append_n(&pool, 9, 1_000).await;
    let cp3 = signed_checkpoint(&pool, 3).await;
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp3).await.unwrap();
    tx.commit().await.unwrap();

    let v = signer().verifier();
    assert!(matches!(
        prune::prune(&pool, &space(), &v, &policy, 9_999_999)
            .await
            .expect("first prune"),
        Pruned::Removed { .. }
    ));

    // A later checkpoint exists, but nothing anchors position 3 any more —
    // delete the marker the first prune left behind and the base is
    // unverifiable.
    let cp7 = signed_checkpoint(&pool, 7).await;
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp7).await.unwrap();
    tx.commit().await.unwrap();
    pool.rows("DELETE FROM \"$checkpoints\" WHERE \"upto\" = 3")
        .await
        .unwrap();

    let e = prune::prune(&pool, &space(), &v, &policy, 9_999_999)
        .await
        .expect_err("the retained base is unanchored");
    assert!(
        matches!(e, PruneRefusal::UnanchoredBase { first_stored: 3 }),
        "{e}"
    );
    assert_eq!(count(&pool, "$commandlog").await, 6, "nothing was deleted");
}

/// **A prune never removes the chain's last entry**, and the bound shows up
/// in the floor rather than as a silent clamp inside the delete.
///
/// The setup is the ordinary state of a quiet space: every entry is older
/// than the undo horizon, so `horizon_position` answers `last + 1` and there
/// is a checkpoint at the head to cut to. Without the retained-tail bound
/// the prune would take the whole table, and an empty chain is read two
/// different wrong ways — vacuously verified from genesis, and *tampered*
/// from its own checkpoint. Both are asserted here from the post-state,
/// because they are the reason the bound exists.
#[tokio::test(flavor = "multi_thread")]
async fn a_prune_never_empties_the_chain() {
    let pool = fresh("ordo_5f_prune_tail").await;
    let policy = LogPolicy {
        undo_horizon: Duration::from_secs(10),
        ..LogPolicy::DEFAULT
    };
    append_n(&pool, 8, 1_000).await;

    // Two checkpoints: one mid-chain, one at the head. Nothing is
    // registered as a consumer, and the clock is far past every entry, so
    // the only thing between the pruner and an empty table is the bound.
    for upto in [4u64, 8] {
        let cp = signed_checkpoint(&pool, upto).await;
        let tx = pool.begin().await.unwrap();
        tx.put_checkpoint(&cp).await.unwrap();
        tx.commit().await.unwrap();
    }

    let f = prune::floor(&pool, &space(), &policy, 9_999_999)
        .await
        .unwrap();
    assert_eq!(
        f.horizon_position, 8,
        "every entry is older than the horizon, so it lands past the end"
    );
    assert_eq!(f.live_bound, 7, "the bound is capped at the last entry");
    assert!(
        matches!(f.holder, prune::FloorHolder::RetainedTail { last_index: 7 }),
        "the cap must name itself in the floor: {}",
        f.holder
    );
    assert_eq!(
        f.position, 4,
        "the greatest checkpoint at or below the capped bound, not the one at the head"
    );

    let v = signer().verifier();
    match prune::prune(&pool, &space(), &v, &policy, 9_999_999)
        .await
        .expect("lawful prune")
    {
        Pruned::Removed {
            entries,
            verifiable_from,
            ..
        } => {
            assert_eq!(entries, 4);
            assert_eq!(verifiable_from, 4);
        }
        other => panic!("expected a prune to 4: {other:?}"),
    }
    assert_eq!(count(&pool, "$commandlog").await, 4);

    // The two answers the bound protects. From genesis: PREFIX MISSING —
    // an empty chain would have answered `Ok` with nothing checked, which
    // is what a space that was never written to answers.
    let e = undo::verify(
        Db::Pool(&pool),
        &space(),
        Anchor::Genesis,
        ChainHash::GENESIS,
    )
    .await
    .expect_err("the prefix is gone");
    assert!(
        matches!(e, undo::VerifyFailure::PrefixMissing { first_stored: 4 }),
        "{e}"
    );

    // From its own checkpoint: it VERIFIES — an empty chain would have
    // answered `Tamper`, which is a lawful prune reporting the one thing it
    // is not.
    let (anchor, hash) = undo::choose_anchor(Db::Pool(&pool), &space(), Some(&v))
        .await
        .expect("anchor");
    undo::verify(Db::Pool(&pool), &space(), anchor, hash)
        .await
        .expect("the retained suffix verifies");
}

/// A checkpoint at an arbitrary position, signed properly.
async fn signed_checkpoint(pool: &Pool, upto: u64) -> Checkpoint {
    let head = head_at(pool, upto - 1).await;
    let at = Timestamp(1_050);
    Checkpoint {
        space: space(),
        upto,
        head,
        at,
        key_id: ordo_commandlog::Signer::key_id(&signer()).to_string(),
        signature: ordo_commandlog::Signer::sign(
            &signer(),
            &Checkpoint::signed_bytes(&space(), upto, head, at),
        ),
    }
}

/// The two artifacts are separate tables, and this asserts it the blunt
/// way: the shapes match, the signed bytes match, and neither query can
/// reach the other's rows.
#[tokio::test(flavor = "multi_thread")]
async fn an_attestation_and_a_checkpoint_share_signed_bytes_and_nothing_else() {
    let pool = fresh("ordo_5f_two_tables").await;
    append_n(&pool, 3, 1_000).await;

    let cp = checkpoint(&pool, 1_050).await;
    let a = match cadence::attest_due(
        &pool,
        &space(),
        &signer(),
        &LogPolicy::DEFAULT,
        Timestamp(1_050),
    )
    .await
    .unwrap()
    {
        Attested::Signed(a) => a,
        other => panic!("expected an attestation: {other:?}"),
    };

    // Same position, same head, same instant, same key — so the signed
    // bytes are identical and one verifier checks both.
    assert_eq!(a.upto, cp.upto);
    assert_eq!(a.head, cp.head);
    assert_eq!(
        Attestation::signed_bytes(&space(), a.upto, a.head, a.at),
        Checkpoint::signed_bytes(&space(), cp.upto, cp.head, cp.at)
    );
    assert_eq!(a.signature, cp.signature);

    // And they are still two rows in two tables. A pruner reading
    // `$checkpoints` cannot be handed the attestation, whatever they have
    // in common.
    assert_eq!(count(&pool, "$checkpoints").await, 1);
    assert_eq!(count(&pool, "$attestations").await, 1);

    // The durable store's checkpoint reader sees exactly one, and it is the
    // checkpoint.
    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    assert_eq!(
        store.try_latest_checkpoint(&space()).await.unwrap(),
        Some(cp)
    );
    // `Store` is the blocking trait, so it may only be driven off a runtime
    // worker — the rule `PgStore`'s own doc states.
    tokio::task::spawn_blocking(move || store.verify(&space()).expect("the chain verifies"))
        .await
        .unwrap();
}
