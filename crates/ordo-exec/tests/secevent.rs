//! 7a, durably: the security-event stream against a real database.
//!
//! Four properties the in-memory tests cannot reach, and each is one of the
//! reasons the stream is not more rows in the command chain:
//!
//! - a **domain revert** — the operation the command chain is built to
//!   support — does not alter one byte of the stream;
//! - the stream **verifies independently** of the command chain, in both
//!   directions: either artifact can be broken while the other still checks
//!   out, and each is checked under its own key;
//! - a security append **never takes the command chain's append lock**,
//!   asserted behaviourally rather than by reading a constant;
//! - retention drops a **whole prefix ending at a checkpoint** and
//!   verification resumes from it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Ed25519Signer, Image,
    SpaceId, Store, TamperKind, Timestamp, Value,
};
use ordo_exec::commandlog::{space_lock_key, PgStore, APPEND_LOCK_NS};
use ordo_exec::secevent::{PgSecurityStore, SECEVENT_LOCK_NS};
use ordo_exec::{Pool, PoolConfig};
use ordo_secevent::{
    checkpoint, prune_to_checkpoint, EventKind, RecordedDecision, Recorder, Relationship, Scope,
    SecurityStore, Stamp, StreamCheckpoint, StreamEntry, Subject,
};

/// Each test owns its database: every suite in this cluster writes to
/// `$secevents`, so sharing one would make another test's entries this
/// test's tamper.
async fn fresh(db: &str, size: usize) -> Pool {
    let admin = ordo_conformance::Pg {
        db: "postgres".into(),
        ..ordo_conformance::Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    for t in [
        "$secevents",
        "$seccheckpoints",
        "$commandlog",
        "$checkpoints",
    ] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }
    pool
}

fn space() -> SpaceId {
    SpaceId("spc_alpha".into())
}

fn scope() -> Scope {
    Scope::Space(space())
}

fn sec_signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("sec-key-2026", &[7u8; 32])
}

fn chain_signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("chain-key-2026", &[9u8; 32])
}

fn img(fields: &[(&str, Value)]) -> Image {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// A one-command create group in `space`.
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

fn deny(n: u32, decision: RecordedDecision) -> StreamEntry {
    StreamEntry::single(
        scope(),
        Subject::member("usr_1"),
        "openTasks",
        EventKind::denied(decision),
        Timestamp(2_000 + u64::from(n)),
        CorrelationId(format!("cor_{n:08x}")),
    )
}

/// Every stored row of the stream, as the database holds it. The comparison
/// unit for "not one byte changed".
async fn raw_rows(pool: &Pool) -> Vec<Vec<Option<String>>> {
    pool.rows(
        "SELECT \"scope\", \"idx\", \"subject\", \"op\", \"kind\", \"count\", \
         encode(\"prev\",'hex'), encode(\"hash\",'hex'), encode(\"body\",'hex') \
         FROM \"$secevents\" ORDER BY \"scope\", \"idx\"",
    )
    .await
    .unwrap()
}

// ───────────────────── D1b: `about` survives storage ─────────────────────

/// The durable half of the trailer: a deployment entry naming a target
/// round-trips through Postgres: the denormalized `about` column reads
/// back what the stream carries, and decoding the stored `body` (what
/// `verify` actually recomputes over) agrees with it. A space-scope entry
/// stores no target at all: the column is null, not an empty string,
/// because "no target" and "target that happened to be empty" are not the
/// same fact.
#[tokio::test(flavor = "multi_thread")]
async fn about_round_trips_through_the_durable_store() {
    let pool = fresh("ordo_secevent_about", 4).await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());

    let mut targeted = StreamEntry::single(
        Scope::Deployment,
        Subject::Unverified,
        "openTasks",
        EventKind::denied(RecordedDecision::TokenUnverifiable),
        Timestamp(1_000),
        CorrelationId("cor_about".into()),
    );
    targeted.about = Some(SpaceId("spc_probed".into()));
    sec.try_append(&targeted).await.expect("deployment append");
    sec.try_append(&deny(0, RecordedDecision::OpNotInRole))
        .await
        .expect("space append");

    // The denormalized column: queryable without decoding, and null for the
    // entry with no target.
    let rows = pool
        .rows("SELECT \"scope\", \"about\" FROM \"$secevents\" ORDER BY \"scope\", \"idx\"")
        .await
        .unwrap();
    let about_col = |scope_key: &str| -> Option<String> {
        rows.iter()
            .find(|r| r[0].as_deref() == Some(scope_key))
            .and_then(|r| r[1].clone())
    };
    assert_eq!(
        about_col(&Scope::Deployment.key().0),
        Some("spc_probed".to_string())
    );
    assert_eq!(
        about_col(&space().0),
        None,
        "a space-scope entry's `about` column must be null, not an empty \
         string standing in for absence"
    );

    // The record itself: decoding `body` (what `verify` recomputes over)
    // agrees with the column.
    let dep_entries = sec.try_entries(&Scope::Deployment, 0, 8).await.unwrap();
    assert_eq!(dep_entries.len(), 1);
    assert_eq!(
        dep_entries[0].entry.about,
        Some(SpaceId("spc_probed".into()))
    );
    let space_entries = sec.try_entries(&scope(), 0, 8).await.unwrap();
    assert_eq!(space_entries.len(), 1);
    assert_eq!(space_entries[0].entry.about, None);

    // And the stream still verifies with the trailer present.
    let sec2 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        sec2.verify(&Scope::Deployment)
            .expect("a trailer-bearing entry still verifies");
        sec2.verify(&scope())
            .expect("a trailer-free entry still verifies");
    })
    .await
    .unwrap();
}

// ───────────────── a domain revert does not touch the stream ─────────────

/// The sentence the whole design returns to: *a security record that a
/// domain revert can rewrite is not a security record.*
///
/// A revert is the command chain's own operation — `revert_group` inverts a
/// logged group and the inverse is applied through the normal gate as new
/// history. Here the space's chain is reverted while the space's security
/// stream is watched byte for byte.
#[tokio::test(flavor = "multi_thread")]
async fn a_domain_revert_does_not_alter_the_security_stream() {
    let pool = fresh("ordo_secevent_revert", 4).await;

    // Domain history, and a denial for the same space in the same window.
    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(0)).await.expect("group 0");
    tx.commit().await.unwrap();
    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(1)).await.expect("group 1");
    tx.commit().await.unwrap();

    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    for n in 0..4 {
        sec.try_append(&deny(n, RecordedDecision::OpNotInRole))
            .await
            .expect("stream append");
    }
    let before_rows = raw_rows(&pool).await;
    let before_head = sec.try_head(&scope()).await.unwrap().expect("head");
    let before_len = sec.try_len(&scope()).await.unwrap();
    assert_eq!(before_len, 4);

    // The revert. This is a domain operation with full write authority over
    // the space's command chain: it reads a logged group, inverts it, and
    // appends the inverse as new attributed history.
    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let inverse = tokio::task::spawn_blocking(move || {
        let plan = ordo_commandlog::revert_group(
            &store,
            &space(),
            &CorrelationId("cg_1".into()),
            &Actor::User("usr_owner".into()),
            Timestamp(3_000),
        )
        .expect("revert plans");
        assert_eq!(plan.commands[0].kind, CommandKind::Delete);
        plan
    })
    .await
    .unwrap();
    let tx = pool.begin().await.unwrap();
    tx.append_group(&inverse).await.expect("inverse appends");
    tx.commit().await.unwrap();

    // The command chain moved — three groups now, and a new head.
    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let (chain_len, chain_ok) =
        tokio::task::spawn_blocking(move || (store.len(&space()), store.verify(&space()).is_ok()))
            .await
            .unwrap();
    assert_eq!(chain_len, 3, "a revert is more history, not less");
    assert!(chain_ok);

    // The stream did not.
    assert_eq!(
        raw_rows(&pool).await,
        before_rows,
        "not one stored byte of the security stream may move when the \
         domain record is reverted"
    );
    assert_eq!(sec.try_head(&scope()).await.unwrap(), Some(before_head));
    assert_eq!(sec.try_len(&scope()).await.unwrap(), before_len);

    // And it still verifies on its own terms.
    let sec2 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        sec2.verify(&scope())
            .expect("the stream verifies after the revert");
    })
    .await
    .unwrap();

    // There is no revert path *into* the stream, and the shape of the
    // record is why: a `StreamEntry` has no before/after images to invert.
    // The nearest thing a caller could do is append a contradicting entry,
    // which leaves both entries in the chain — the record grows, it does
    // not change its mind.
    sec.try_append(&deny(9, RecordedDecision::PrincipalSuspended))
        .await
        .expect("appends");
    assert_eq!(sec.try_len(&scope()).await.unwrap(), 5);
    let kept = sec.try_entries(&scope(), 0, 8).await.unwrap();
    assert_eq!(
        kept[0].entry.kind,
        EventKind::denied(RecordedDecision::OpNotInRole),
        "the earlier entry is untouched by the later one"
    );
}

// ───────────────────── independent verification ─────────────────────────

/// `verify()` verifies both streams, independently — and each under its own
/// key. Broken in either direction, the other still checks out.
#[tokio::test(flavor = "multi_thread")]
async fn the_two_artifacts_verify_independently_and_under_different_keys() {
    let pool = fresh("ordo_secevent_verify", 4).await;

    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(0)).await.unwrap();
    tx.commit().await.unwrap();
    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(1)).await.unwrap();
    tx.commit().await.unwrap();

    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    for n in 0..4 {
        sec.try_append(&deny(n, RecordedDecision::NoMembership))
            .await
            .unwrap();
    }

    // Two checkpoints, two keys. Each verifies under its own and refuses
    // the other's — the separation is a fact about the artifacts, not a
    // convention about which variable was read.
    let sec_key = sec_signer();
    let sec_for_cp = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let stream_cp = tokio::task::spawn_blocking(move || {
        checkpoint(&sec_for_cp, &scope(), &sec_key, Timestamp(5_000)).expect("head")
    })
    .await
    .unwrap();
    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let chain_cp = tokio::task::spawn_blocking(move || {
        ordo_commandlog::checkpoint(&store, &space(), &chain_signer(), Timestamp(5_000))
            .expect("head")
    })
    .await
    .unwrap();

    assert!(stream_cp.verify(&sec_signer().verifier()));
    assert!(!stream_cp.verify(&chain_signer().verifier()));
    assert!(chain_cp.verify(&chain_signer()));
    assert!(!chain_cp.verify(&sec_signer()));

    // Both are persisted, in their own tables.
    let tx = pool.begin().await.unwrap();
    tx.put_security_checkpoint(&stream_cp).await.unwrap();
    tx.put_checkpoint(&chain_cp).await.unwrap();
    tx.commit().await.unwrap();
    let sec3 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let read_back = sec3.try_latest_checkpoint(&scope()).await.unwrap();
    assert_eq!(read_back.as_ref(), Some(&stream_cp));

    // Break the command chain: the stream is unaffected.
    pool.rows(
        "UPDATE \"$commandlog\" SET \"body\" = \
         overlay(\"body\" placing '\\x00'::bytea from 9 for 1) WHERE \"idx\" = 1",
    )
    .await
    .unwrap();
    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let sec4 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        assert!(
            store.verify(&space()).is_err(),
            "the command chain must notice its own tamper"
        );
        sec4.verify(&scope())
            .expect("the security stream is a separate artifact and still verifies");
    })
    .await
    .unwrap();

    // Break the stream: it reports the index, and the (already broken)
    // command chain's state is irrelevant to that answer.
    pool.rows("UPDATE \"$secevents\" SET \"count\" = 999 WHERE \"idx\" = 2")
        .await
        .unwrap();
    let sec5 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        // The denormalized column is a copy, not the record: verification
        // reads `body`, so this alone changes nothing.
        sec5.verify(&scope())
            .expect("a doctored query column is not the record");
    })
    .await
    .unwrap();
    pool.rows(
        "UPDATE \"$secevents\" SET \"body\" = \
         overlay(\"body\" placing '\\x00'::bytea from 27 for 1) WHERE \"idx\" = 2",
    )
    .await
    .unwrap();
    let sec6 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let bad = tokio::task::spawn_blocking(move || {
        sec6.verify(&scope())
            .expect_err("a doctored body is the record")
    })
    .await
    .unwrap();
    assert_eq!(bad.index, 2);
    assert_eq!(bad.kind, TamperKind::HashMismatch);
    assert_eq!(bad.space, space());
}

/// A `$commandlog` body stored in `$secevents` does not verify as anything:
/// the domain tag makes the two encodings disjoint, so the streams cannot be
/// cross-grafted even by someone with write access to both tables.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_body_grafted_into_the_stream_is_unreadable() {
    let pool = fresh("ordo_secevent_graft", 4).await;
    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(0)).await.unwrap();
    tx.commit().await.unwrap();
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    sec.try_append(&deny(0, RecordedDecision::NoMembership))
        .await
        .unwrap();

    pool.rows(
        "UPDATE \"$secevents\" SET \"body\" = \
         (SELECT \"body\" FROM \"$commandlog\" LIMIT 1) WHERE \"idx\" = 0",
    )
    .await
    .unwrap();
    let sec2 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let bad = tokio::task::spawn_blocking(move || {
        sec2.verify(&scope())
            .expect_err("a grafted body is a verification failure")
    })
    .await
    .unwrap();
    assert_eq!(
        bad.kind,
        TamperKind::Unreadable,
        "'I could not check' and 'it checks out' must not be the same value"
    );
}

// ─────────────────── no interaction with the chain's lock ────────────────

/// §6.1, pinned: **no interaction with the command chain's append lock**.
///
/// A denial that queued behind the write path would let a caller with no
/// authority in a space contend with every legitimate writer in it. The two
/// halves are asserted together, because either alone proves nothing:
/// the security append completes while the chain's lock is held, and a
/// *command* append in the same conditions demonstrably does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_security_append_never_takes_the_command_chains_lock() {
    let pool = fresh("ordo_secevent_lock", 6).await;
    assert_ne!(APPEND_LOCK_NS, SECEVENT_LOCK_NS);

    // A writer holds the command chain's per-space lock for `space()` and
    // stays open.
    let writer = pool.begin().await.unwrap();
    writer.append_group(&group(0)).await.expect("A appends");
    let held = pool
        .rows(&format!(
            "SELECT count(*) FROM pg_locks WHERE \"locktype\" = 'advisory' \
             AND \"classid\" = {APPEND_LOCK_NS} AND \"objid\" = {} AND \"granted\" \
             AND \"database\" = (SELECT oid FROM pg_database \
                                WHERE datname = current_database())",
            space_lock_key(&space()) as i64 & 0xFFFF_FFFF
        ))
        .await
        .unwrap();
    assert_eq!(
        held[0][0].as_deref(),
        Some("1"),
        "the command chain's lock for this space is held right now"
    );

    // The denial for that same space appends anyway, promptly.
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    tokio::time::timeout(
        Duration::from_secs(2),
        sec.try_append(&deny(0, RecordedDecision::NoMembership)),
    )
    .await
    .expect("a denial must never queue behind a legitimate writer")
    .expect("appends");

    // The control: a command append for the same space DOES queue. Without
    // this the test would pass on a build where nothing locks at all.
    let blocked = pool.begin().await.unwrap();
    let (tx_done, mut rx_done) = tokio::sync::oneshot::channel();
    let blocked_task = tokio::spawn(async move {
        blocked.append_group(&group(1)).await.expect("B appends");
        let _ = tx_done.send(());
        blocked.commit().await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        matches!(
            rx_done.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "a command append for the same space must wait on the lock the \
         security append sailed past"
    );
    writer.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), blocked_task)
        .await
        .expect("unblocks on commit")
        .unwrap();

    // And the stream's own lock does serialize the stream: two security
    // appends to one scope produce a single chain, not a fork.
    let sec2 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let mut tasks = Vec::new();
    for n in 1..7u32 {
        let s = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
        tasks.push(tokio::spawn(async move {
            s.try_append(&deny(n, RecordedDecision::OpNotInRole))
                .await
                .expect("racing append")
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    tokio::task::spawn_blocking(move || {
        assert_eq!(sec2.len(&scope()), 7);
        sec2.verify(&scope())
            .expect("six racing appends still make one verifiable chain");
    })
    .await
    .unwrap();
}

// ─────────────────────────── retention ──────────────────────────────────

/// Retention drops a whole prefix ending at a checkpoint, and verification
/// resumes from that checkpoint — the durable half of the in-memory test.
#[tokio::test(flavor = "multi_thread")]
async fn retention_drops_a_prefix_and_verification_resumes_durably() {
    let pool = fresh("ordo_secevent_retain", 4).await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    for n in 0..8 {
        sec.try_append(&deny(n, RecordedDecision::NoMembership))
            .await
            .unwrap();
    }
    let head5 = sec.try_entries(&scope(), 4, 1).await.unwrap()[0].hash;
    let signer = sec_signer();
    let cp = StreamCheckpoint {
        scope: scope(),
        upto: 5,
        head: head5,
        at: Timestamp(6_000),
        key_id: ordo_commandlog::Signer::key_id(&signer).to_string(),
        signature: ordo_commandlog::Signer::sign(
            &signer,
            &StreamCheckpoint::signed_bytes(&scope(), 5, head5, Timestamp(6_000)),
        ),
    };
    let tx = pool.begin().await.unwrap();
    tx.put_security_checkpoint(&cp).await.unwrap();
    tx.commit().await.unwrap();

    let mut sec2 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let anchor = tokio::task::spawn_blocking(move || {
        let anchor =
            prune_to_checkpoint(&mut sec2, &cp, &sec_signer().verifier()).expect("lawful prune");
        assert_eq!(sec2.len(&scope()), 3);
        assert_eq!(sec2.base_index(&scope()), 5);
        sec2.verify_from(&scope(), anchor, 5)
            .expect("the retained suffix verifies from the checkpoint");
        // Verifying a pruned stream as if it began at genesis is refused
        // rather than mistaken for tampering at index 0.
        assert_eq!(
            sec2.verify(&scope()).expect_err("pruned").kind,
            TamperKind::Unreadable
        );
        anchor
    })
    .await
    .unwrap();
    assert_eq!(anchor, head5);

    // The pruned rows are gone from the table, not merely hidden.
    let n = pool
        .rows("SELECT count(*) FROM \"$secevents\"")
        .await
        .unwrap();
    assert_eq!(n[0][0].as_deref(), Some("3"));
}

// ───────────────── the recorder end to end, off the path ─────────────────

/// The whole path a deployment runs: observations at the door, collapse in
/// memory, one drain, durable appends — and the members-only rule holding
/// across it.
#[tokio::test(flavor = "multi_thread")]
async fn a_drained_window_lands_as_one_entry_per_key_in_the_right_stream() {
    let pool = fresh("ordo_secevent_drain", 4).await;
    let recorder = Recorder::default();
    for n in 0..25u32 {
        // A member's denials: their space's stream.
        recorder.deny(
            Some(&space()),
            Subject::member("usr_1"),
            "openTasks",
            RecordedDecision::OpNotInRole,
            &Stamp::new(
                Timestamp(1_000 + u64::from(n)),
                CorrelationId(format!("cor_m{n}")),
            ),
            Relationship::Member,
        );
        // A stranger probing the same space: the deployment's.
        recorder.deny(
            Some(&space()),
            Subject::Unverified,
            "openTasks",
            RecordedDecision::TokenUnverifiable,
            &Stamp::new(
                Timestamp(1_000 + u64::from(n)),
                CorrelationId(format!("cor_u{n}")),
            ),
            Relationship::None,
        );
    }
    let entries = recorder.drain_all(Timestamp(2_500));
    assert_eq!(entries.len(), 2, "twenty-five requests each, two entries");

    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    sec.try_append_all(&entries).await.expect("batch appends");

    let space_entries = sec.try_entries(&scope(), 0, 16).await.unwrap();
    assert_eq!(space_entries.len(), 1);
    assert_eq!(space_entries[0].entry.count, 25);
    assert_eq!(space_entries[0].entry.subject, Subject::member("usr_1"));

    let dep = sec.try_entries(&Scope::Deployment, 0, 16).await.unwrap();
    assert_eq!(dep.len(), 1);
    assert_eq!(
        dep[0].entry.subject,
        Subject::Unverified,
        "an unverifiable presentation names no principal, and never grows a \
         space's spine"
    );

    let sec2 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        sec2.verify(&scope()).expect("space stream verifies");
        sec2.verify(&Scope::Deployment)
            .expect("deployment stream verifies, separately");
    })
    .await
    .unwrap();
}
