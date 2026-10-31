//! 7c — the drainer, against a real database.
//!
//! Five properties, and each is one that could not be asserted while the
//! stream had no drainer on any host path:
//!
//! - a denial observed at the door **reaches Postgres** without anybody
//!   calling an append, and survives the pool that wrote it being dropped;
//! - a **shutdown drain** writes the window that has not elapsed, so a
//!   planned restart loses nothing;
//! - an **unreachable store** does not lose the backlog and does not grow
//!   without bound — the outbox holds, and what it cannot hold becomes a
//!   declared `EventsDropped` link in the stream rather than silence;
//! - **retention** cuts at a verified checkpoint, refuses to empty a
//!   stream, and leaves a suffix that still verifies;
//! - the drain's state is **visible** on `/health/deep`.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{CorrelationId, SpaceId, Timestamp};
use ordo_conformance::Pg;
use ordo_exec::secevent::PgSecurityStore;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::metrics::Metrics;
use ordo_host::secdrain::{DrainConfig, SecurityDrain};
use ordo_secevent::{
    Collapser, EventKind, RecordedDecision, Recorder, Relationship, Retention, Scope,
    SecurityStore, Stamp, StreamCheckpoint, StreamEntry, Subject,
};

/// A database of this suite's own. `Pg::named` creates it; every suite in
/// the cluster writes to `$secevents`, so a shared one would make another
/// suite's entries this test's tamper.
async fn fresh(db: &str) -> (Pool, PoolConfig) {
    let _ = Pg::named(db);
    let cfg = PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    };
    let pool = Pool::connect(cfg.clone()).await.expect("boot");
    pool.ensure_platform_tables()
        .await
        .expect("platform tables");
    // `TRUNCATE`, not `DELETE`. `the_cost_of_one_recorded_decision_is_what_
    // the_audit_says` measures `pg_total_relation_size` over a thousand
    // rows, and a `DELETE` leaves the dead tuples and the pages holding
    // them — so on a persistent cluster the measurement grew by one run's
    // corpses every run and eventually broke the audited range while
    // nothing about the row had changed. `TRUNCATE` reclaims the relation,
    // which is what makes the number mean what the test says it means.
    for t in ["$secevents", "$seccheckpoints", "$schedules"] {
        pool.rows(&format!("TRUNCATE TABLE \"{t}\"")).await.unwrap();
    }
    (pool, cfg)
}

fn space() -> SpaceId {
    SpaceId("spc_alpha".into())
}

fn scope() -> Scope {
    Scope::Space(space())
}

/// One denial at the door, from a member of the space — so §6.1's
/// members-only rule routes it to that space's own stream.
fn observe(recorder: &Recorder, at: u64, n: u32) {
    recorder.deny(
        Some(&space()),
        Subject::member("usr_1"),
        "openTasks",
        RecordedDecision::OpNotInRole,
        &Stamp::new(Timestamp(at), CorrelationId(format!("cor_{n:08x}"))),
        Relationship::Member,
    );
}

/// A drain whose tick is short enough for a test to wait on. Retention off
/// unless a test asks for it: it needs a signing key from the environment,
/// and the environment is shared with every other test in this binary.
fn quick() -> DrainConfig {
    DrainConfig {
        tick: Duration::from_millis(50),
        retain: false,
        ..DrainConfig::default()
    }
}

async fn stored(pool: &Pool) -> i64 {
    pool.rows("SELECT count(*) FROM \"$secevents\"")
        .await
        .unwrap()[0][0]
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Wait for a condition, or give up. A drain is a background task, so a
/// test that asserts immediately after observing is asserting about
/// scheduling rather than about the drain.
async fn until(mut f: impl FnMut() -> bool, what: &str) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

// ── durability ───────────────────────────────────────────────────────────

/// **The property this whole item exists for.** A denial recorded at the
/// door reaches durable storage with nobody calling `append`, and it is
/// still there after the pool that wrote it is gone.
///
/// Proved the way the rest of this workspace proves durability: write,
/// drop the pool, reconnect on a *new* pool, read it back and verify the
/// chain. An assertion made through the same connection that wrote proves
/// only that the session remembers.
#[tokio::test(flavor = "multi_thread")]
async fn an_observed_denial_reaches_postgres_and_survives_the_pool() {
    let (pool, cfg) = fresh("ordo_sec_durable").await;
    let recorder = Arc::new(Recorder::default());
    let metrics = Arc::new(Metrics::new());

    let drain = SecurityDrain::start(recorder.clone(), pool.clone(), metrics.clone(), quick())
        .await
        .expect("drain starts");

    for n in 0..25u32 {
        observe(&recorder, 1_000 + u64::from(n), n);
    }
    // Nothing has been appended yet: the collapse window is open, and §6.1
    // pins that nothing is hashed while it is.
    assert_eq!(stored(&pool).await, 0, "an open window must not append");

    // The shutdown drain closes it. This is what makes a planned restart
    // lossless.
    let written = drain.shutdown().await.expect("shutdown flushes");
    assert_eq!(written, 1, "twenty-five denials collapse to one entry");

    // Drop every connection that has seen this data.
    drop(pool);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // A brand-new pool, and the entry is there.
    let pool2 = Pool::connect(cfg).await.expect("reconnect");
    assert_eq!(stored(&pool2).await, 1);

    let sec = PgSecurityStore::new(pool2.clone(), tokio::runtime::Handle::current());
    let entries = sec.try_entries(&scope(), 0, 8).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].entry.count, 25,
        "the collapsed count is inside the hashed body, not a column"
    );
    assert_eq!(entries[0].entry.subject, Subject::member("usr_1"));

    // And it verifies as a chain, read back from storage rather than from
    // the memory that produced it.
    let sec2 = PgSecurityStore::new(pool2, tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || sec2.verify(&scope()).expect("the stream verifies"))
        .await
        .unwrap();
}

/// The drain writes on its own tick, with nobody calling anything. This is
/// the difference between "a drainer exists" and "a drainer runs".
#[tokio::test(flavor = "multi_thread")]
async fn the_drain_writes_on_its_own_without_a_shutdown() {
    let (pool, _cfg) = fresh("ordo_sec_tick").await;
    // A one-second collapse window, so a tick can find a closed one.
    let recorder = Arc::new(Recorder::new(Collapser::new(1, 256)));
    let metrics = Arc::new(Metrics::new());
    let drain = SecurityDrain::start(recorder.clone(), pool.clone(), metrics.clone(), quick())
        .await
        .expect("drain starts");

    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for n in 0..10u32 {
        observe(&recorder, at, n);
    }

    let health = drain.health();
    until(|| health.appended() > 0, "the drain to write on its tick").await;
    assert!(stored(&pool).await >= 1);
    assert_eq!(health.failures(), 0, "{:?}", health.last_error());

    // The counters an operator alerts on are published.
    let text = metrics.render_prometheus();
    assert!(
        text.contains(ordo_host::secdrain::APPENDED),
        "no append counter:\n{text}"
    );
    assert!(
        text.contains(ordo_host::secdrain::OBSERVED),
        "no observation counter — without it the drop counter has no \
         denominator:\n{text}"
    );
}

// ── the store is unreachable ─────────────────────────────────────────────

/// The decision, asserted rather than described: an unreachable store loses
/// **nothing** while the outbox holds, and the drain reports the fault.
///
/// The failure is injected the way a real one arrives — the database goes
/// away underneath a running pool — by pointing the drain at a database
/// that is then dropped.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_store_keeps_the_backlog_and_reports_the_fault() {
    let (pool, cfg) = fresh("ordo_sec_unreachable").await;
    let recorder = Arc::new(Recorder::new(Collapser::new(1, 256)));
    let metrics = Arc::new(Metrics::new());
    let drain = SecurityDrain::start(recorder.clone(), pool.clone(), metrics.clone(), quick())
        .await
        .expect("drain starts");
    let health = drain.health();

    // Take the table away. Every append now fails on a real Postgres error,
    // which is what a store outage looks like from inside the drain.
    pool.rows("ALTER TABLE \"$secevents\" RENAME TO \"$secevents_hidden\"")
        .await
        .expect("hide the table");

    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for n in 0..10u32 {
        observe(&recorder, at, n);
    }

    until(|| health.failures() > 0, "an append failure to be recorded").await;
    assert!(
        health.last_error().is_some_and(|e| !e.is_empty()),
        "a failing drain must name its fault, not merely count it"
    );
    assert!(
        health.failing_for(at + 1).is_some(),
        "a failing drain must report how long it has been failing"
    );

    // Nothing was lost: the entry went back on the outbox rather than
    // vanishing with the failed append.
    assert!(
        recorder.backlog() > 0,
        "a failed append must return its entries to the recorder"
    );
    assert_eq!(
        recorder.dropped(),
        0,
        "nothing may be dropped while the outbox still has room"
    );

    // Bring the store back. The backlog lands, unaided.
    pool.rows("ALTER TABLE \"$secevents_hidden\" RENAME TO \"$secevents\"")
        .await
        .expect("restore the table");
    until(
        || health.appended() > 0,
        "the backlog to drain once the store returns",
    )
    .await;
    assert_eq!(recorder.backlog(), 0);

    drop(drain);
    let pool2 = Pool::connect(cfg).await.expect("reconnect");
    assert!(
        stored(&pool2).await >= 1,
        "the entries buffered during the outage must be durable afterwards"
    );
}

/// Past the bound, the loss is **declared inside the record** — a hashed
/// `EventsDropped` link in the stream a reviewer is reading, not a counter
/// in a process that is about to restart.
#[tokio::test(flavor = "multi_thread")]
async fn a_full_outbox_declares_its_loss_in_the_stream_itself() {
    let (pool, _cfg) = fresh("ordo_sec_dropped").await;
    // A four-entry outbox and a zero-second collapse window, so ten
    // distinct principals overflow it immediately.
    let recorder = Arc::new(Recorder::with_outbox(Collapser::new(0, 256), 4));
    let metrics = Arc::new(Metrics::new());

    // Non-collapsing events go straight to the outbox, which is what the
    // bound guards. Ten of them into four slots.
    for n in 0..10u32 {
        recorder.observe(
            ordo_secevent::Observation {
                space: Some(space()),
                subject: Subject::member(&format!("usr_{n}")),
                op: "openTasks".into(),
                kind: EventKind::Authenticated,
                at: Timestamp(1_000 + u64::from(n)),
                correlation: CorrelationId(format!("cor_{n}")),
            },
            Relationship::Member,
        );
    }
    assert_eq!(recorder.backlog(), 4, "the bound holds");
    assert_eq!(recorder.dropped(), 6, "and counts what it refused");
    assert_eq!(
        recorder.undeclared_loss(),
        6,
        "the loss is tallied and waiting to be declared"
    );

    let drain = SecurityDrain::start(recorder.clone(), pool.clone(), metrics.clone(), quick())
        .await
        .expect("drain starts");
    let health = drain.health();
    until(|| health.appended() >= 5, "the drain to write five entries").await;
    drop(drain);

    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let entries = sec.try_entries(&scope(), 0, 32).await.unwrap();

    // The marker is FIRST: it is a statement about what would have come
    // before the entries behind it.
    assert_eq!(
        entries[0].entry.kind,
        EventKind::EventsDropped { lost: 6 },
        "the record must declare its own gap, in the record"
    );
    assert_eq!(
        entries[0].entry.subject,
        Subject::platform("secevent"),
        "a statement about the record is attributed to the platform, never \
         to a principal"
    );
    assert_eq!(entries.len(), 5, "the marker plus the four that fitted");
    // The four that survived are the OLDEST four — the onset of the burst,
    // which is the part with evidentiary value.
    assert_eq!(entries[1].entry.subject, Subject::member("usr_0"));
    assert_eq!(entries[4].entry.subject, Subject::member("usr_3"));

    // And the declaration is a link like any other: it chains, and the
    // stream still verifies with it in place.
    let sec2 = PgSecurityStore::new(pool, tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        sec2.verify(&scope())
            .expect("a declared gap does not break the chain")
    })
    .await
    .unwrap();

    // Declared once, not once per drain.
    assert_eq!(recorder.undeclared_loss(), 0);
    assert_eq!(
        recorder.dropped(),
        6,
        "the lifetime total stays, so a health surface can still report it"
    );
}

/// A marker that could not be appended is **never** the entry the bound
/// discards. Losing the declaration that events were lost is the one loss
/// nothing downstream could notice.
#[tokio::test(flavor = "multi_thread")]
async fn a_drop_marker_is_never_itself_dropped() {
    let recorder = Recorder::with_outbox(Collapser::new(0, 256), 2);
    for n in 0..6u32 {
        recorder.observe(
            ordo_secevent::Observation {
                space: Some(space()),
                subject: Subject::member(&format!("usr_{n}")),
                op: "openTasks".into(),
                kind: EventKind::Authenticated,
                at: Timestamp(1_000),
                correlation: CorrelationId(format!("cor_{n}")),
            },
            Relationship::Member,
        );
    }
    assert_eq!(recorder.dropped(), 4);

    // Drain, then hand the whole batch back as a failed append would. The
    // outbox is two deep and the batch is three (marker + two entries).
    let batch = recorder.drain_ready(Timestamp(2_000));
    assert_eq!(batch.len(), 3);
    recorder.requeue(batch);

    // The marker survived — as a tally, which the next drain regenerates —
    // and nothing about the declared total was lost.
    assert_eq!(
        recorder.undeclared_loss(),
        4,
        "a requeued marker folds back into the tally it came from"
    );
    let again = recorder.drain_ready(Timestamp(3_000));
    assert_eq!(
        again[0].kind,
        EventKind::EventsDropped { lost: 4 },
        "the declaration is regenerated, never lost: {again:?}"
    );
}

// ── retention ────────────────────────────────────────────────────────────

/// Retention cuts at a **verified** checkpoint, and never to nothing.
///
/// Three things this pins, in order:
///
/// - a pass with no signed anchor prunes nothing and **says so**, rather
///   than cutting on an unvouched-for position;
/// - a checkpoint signed at the head is not a cut point *yet* — the tail
///   bound puts the floor one short of the end, so a checkpoint becomes
///   usable when the stream grows past it. A dormant scope therefore keeps
///   its last checkpoint's prefix, which is the price of never emptying a
///   stream and is bounded by what that scope had already accumulated;
/// - the last entry always survives. See `StreamError::RetainedTail`: an
///   emptied table does not merely lose history, it makes the next append
///   fork a second chain from genesis.
#[tokio::test(flavor = "multi_thread")]
async fn retention_cuts_at_a_checkpoint_and_never_empties_the_stream() {
    let (pool, _cfg) = fresh("ordo_sec_retention").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let signer = ordo_commandlog::Ed25519Signer::from_seed("sec-retain", &[3u8; 32]);
    let verifier = signer.verifier();
    let entry = |n: u32| {
        StreamEntry::single(
            scope(),
            Subject::member("usr_1"),
            "openTasks",
            EventKind::denied(RecordedDecision::NoMembership),
            Timestamp(1_000 + u64::from(n)),
            CorrelationId(format!("cor_{n}")),
        )
    };

    // Eight entries, all far older than the policy's window.
    for n in 0..8u32 {
        sec.try_append(&entry(n)).await.unwrap();
    }
    let keep = Retention {
        keep_secs: 10,
        deployment_keep_secs: 10,
    };
    let now = Timestamp(1_000_000);

    // Pass 1: there is no signed checkpoint, so there is no lawful cut
    // point — and the pass says so rather than pruning anyway.
    let first = ordo_exec::secevent::retain_scope(&sec, &scope(), &signer, &verifier, keep, now)
        .await
        .expect("a pass with no anchor is not an error");
    assert_eq!(first.pruned, 0);
    assert!(
        first
            .refused
            .as_deref()
            .unwrap_or("")
            .contains("checkpoint"),
        "the refusal must name the missing anchor: {first:?}"
    );
    // But it signed one, at the head — which is why checkpointing comes
    // first in a pass: today's head is what makes today's entries prunable
    // later.
    assert_eq!(first.checkpointed, Some(8));

    // Pass 2: the only checkpoint is AT the head, and the tail bound puts
    // the floor at 7. Still nothing to cut, and still not an error.
    let second = ordo_exec::secevent::retain_scope(&sec, &scope(), &signer, &verifier, keep, now)
        .await
        .expect("second pass");
    assert_eq!(
        second.pruned, 0,
        "a checkpoint at the head is not yet a cut point: {second:?}"
    );
    assert_eq!(stored(&pool).await, 8);

    // The stream grows. Now the checkpoint at 8 is below the floor.
    for n in 8..11u32 {
        sec.try_append(&entry(n)).await.unwrap();
    }
    let third = ordo_exec::secevent::retain_scope(&sec, &scope(), &signer, &verifier, keep, now)
        .await
        .expect("third pass");
    assert_eq!(
        third.pruned, 8,
        "the prefix the checkpoint covers: {third:?}"
    );
    assert_eq!(third.verifiable_from, 8);
    assert_eq!(stored(&pool).await, 3);

    // The surviving suffix verifies from the checkpoint it was cut at, and
    // verifying it from genesis is refused rather than mistaken for
    // tampering at index 0.
    let sec2 = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let anchor = sec
        .try_checkpoint_at_or_below(&scope(), 8)
        .await
        .unwrap()
        .expect("the anchor is still stored");
    tokio::task::spawn_blocking(move || {
        sec2.verify_from(&scope(), anchor.head, 8)
            .expect("the retained suffix verifies from its checkpoint");
        assert_eq!(
            sec2.verify(&scope())
                .expect_err("a pruned stream is not a genesis stream")
                .kind,
            ordo_commandlog::TamperKind::Unreadable
        );
    })
    .await
    .unwrap();

    // And a scope whose every entry has aged out still keeps one, forever.
    for _ in 0..4 {
        let _ = ordo_exec::secevent::retain_scope(&sec, &scope(), &signer, &verifier, keep, now)
            .await
            .expect("further passes");
    }
    assert!(
        stored(&pool).await >= 1,
        "retention must never empty a stream — the head is what the next \
         append chains from"
    );
}

/// A checkpoint that does not verify is not a cut point, whatever it says
/// about where the chain was. Otherwise "prune" is a way to delete evidence
/// and blame retention.
#[tokio::test(flavor = "multi_thread")]
async fn an_unverified_checkpoint_is_refused_as_a_cut_point() {
    let (pool, _cfg) = fresh("ordo_sec_forged").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    for n in 0..4u32 {
        sec.try_append(&StreamEntry::single(
            scope(),
            Subject::member("usr_1"),
            "openTasks",
            EventKind::denied(RecordedDecision::NoMembership),
            Timestamp(1_000 + u64::from(n)),
            CorrelationId(format!("cor_{n}")),
        ))
        .await
        .unwrap();
    }
    // A checkpoint signed by a key nobody published, inserted straight into
    // the table — the shape of "an operator with database access wants a
    // prefix gone".
    let forger = ordo_commandlog::Ed25519Signer::from_seed("not-the-deployment", &[9u8; 32]);
    let head = sec.try_entries(&scope(), 2, 1).await.unwrap()[0].hash;
    let bytes = StreamCheckpoint::signed_bytes(&scope(), 3, head, Timestamp(5_000));
    let cp = StreamCheckpoint {
        scope: scope(),
        upto: 3,
        head,
        at: Timestamp(5_000),
        key_id: ordo_commandlog::Signer::key_id(&forger).to_string(),
        signature: ordo_commandlog::Signer::sign(&forger, &bytes),
    };
    let tx = pool.begin().await.unwrap();
    tx.put_security_checkpoint(&cp).await.unwrap();
    tx.commit().await.unwrap();

    let real = ordo_commandlog::Ed25519Signer::from_seed("the-deployment", &[3u8; 32]);
    let out = ordo_exec::secevent::retain_scope(
        &sec,
        &scope(),
        &real,
        &real.verifier(),
        Retention {
            keep_secs: 10,
            deployment_keep_secs: 10,
        },
        Timestamp(1_000_000),
    )
    .await
    .expect("a refused cut is a report, not an error");
    assert_eq!(out.pruned, 0, "a forged anchor must not authorize a cut");
    assert_eq!(stored(&pool).await, 4);
}

// ── the checkpoint pass: the marker that makes check 4 reachable ──────────
//
// `audit_stream`'s fourth check is *nothing was removed from the end of the
// stream*, and it is the only one a chain walk structurally cannot perform:
// deleting the last N entries leaves a perfectly self-consistent chain, so
// only a signed marker at a higher position knows they were ever there.
//
// Until this section, the only thing in a running deployment that ever
// signed one was step 1 of `retain_scope`. So the check was reachable only
// when retention was on, at the retention period, and over a head nothing
// had verified. Everything below asserts one of those three being fixed.

/// A signer standing in for `ORDO_SECURITY_CHECKPOINT_KEY`, which this
/// workspace cannot set without `unsafe`. Its own seed, distinct from
/// `sec-retain`'s, so a test can tell a checkpoint this key signed from one
/// the retention tests' key signed.
fn cp_signer() -> ordo_commandlog::Ed25519Signer {
    ordo_commandlog::Ed25519Signer::from_seed("sec-checkpoint", &[7u8; 32])
}

async fn seed_entries(sec: &PgSecurityStore, scope: &Scope, range: std::ops::Range<u32>) {
    for n in range {
        let mut e = StreamEntry::single(
            scope.clone(),
            Subject::member("usr_1"),
            "openTasks",
            EventKind::denied(RecordedDecision::NoMembership),
            Timestamp(1_000 + u64::from(n)),
            CorrelationId(format!("cor_{n}")),
        );
        e.scope = scope.clone();
        sec.try_append(&e).await.expect("append");
    }
}

async fn checkpoint_rows(pool: &Pool) -> Vec<i64> {
    pool.rows("SELECT \"upto\" FROM \"$seccheckpoints\" ORDER BY \"upto\"")
        .await
        .unwrap()
        .into_iter()
        .filter_map(|r| r.first()?.as_deref()?.parse().ok())
        .collect()
}

/// **The scheduled signer signs, on its own schedule row, once per period.**
///
/// Three properties in one pass, because they are the three halves of "a
/// fleet signs once an hour":
///
/// - a marker appears at the head with nobody building one by hand;
/// - the row claimed is `secevent.checkpoint` and **not**
///   `secevent.retention` — two artifacts' cadences through one row is the
///   bug `ordo_exec::cadence`'s module doc exists to make inexpressible, and
///   this is the assertion that the security stream did not reintroduce it;
/// - a second pass inside the period claims nothing, which is the whole of
///   the fleet property: `claim_schedule` is fires-once by `FOR UPDATE SKIP
///   LOCKED`, so a hundred replicas asking at the same instant produce one
///   signature.
///
/// Note what [`ordo_host::secdrain::Checkpointer::new`] does **not** take: a
/// `Retention`. The pass has no retention input at all, which is how "a
/// deployment that keeps everything still gets its head signed" is
/// guaranteed rather than tested.
#[tokio::test(flavor = "multi_thread")]
async fn the_checkpoint_pass_signs_the_head_on_its_own_schedule_once_per_period() {
    let (pool, _cfg) = fresh("ordo_sec_cp_schedule").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    seed_entries(&sec, &scope(), 0..4).await;

    let period = Duration::from_secs(3600);
    let now = 2_000_000u64;
    // Registered two periods ago, so it is due now — the same setup the
    // supervisor's own retention test uses.
    ordo_exec::secevent::register_checkpoint_schedule(&pool, period, now as i64 - 7200)
        .await
        .expect("schedule");

    let checkpointer = ordo_host::secdrain::Checkpointer::new(
        pool.clone(),
        Arc::new(Metrics::new()),
        Arc::new(ordo_host::secdrain::DrainHealth::default()),
        Arc::new(cp_signer()),
        period,
    );

    assert_eq!(
        checkpointer.pass(now).await.expect("a due pass"),
        Some(1),
        "one scope, one marker"
    );
    assert_eq!(checkpoint_rows(&pool).await, vec![4]);

    // Its own row, and nothing else's.
    let names: Vec<String> = pool
        .rows("SELECT \"name\" FROM \"$schedules\" ORDER BY \"name\"")
        .await
        .unwrap()
        .into_iter()
        .filter_map(|r| r.first()?.clone())
        .collect();
    assert_eq!(
        names,
        vec!["secevent.checkpoint".to_string()],
        "the checkpoint pass must claim its own row and must not touch \
         `secevent.retention`: two cadences through one row is the bug \
         `cadence.rs` was written to prevent"
    );

    // Inside the period, a second replica's pass finds nothing to claim.
    seed_entries(&sec, &scope(), 4..6).await;
    assert_eq!(
        checkpointer.pass(now + 60).await.expect("a pass"),
        None,
        "a fleet signs once per period, not once per replica"
    );
    assert_eq!(
        checkpoint_rows(&pool).await,
        vec![4],
        "the head moved and the period had not elapsed, so nothing was signed"
    );

    // A period later it is due again, and now the head has moved.
    assert_eq!(
        checkpointer
            .pass(now + period.as_secs())
            .await
            .expect("a due pass"),
        Some(1)
    );
    assert_eq!(checkpoint_rows(&pool).await, vec![4, 6]);
}

/// **A head that has not moved is not signed again.**
///
/// `$seccheckpoints` has `PRIMARY KEY (scope, upto)` and
/// `put_security_checkpoint` is `ON CONFLICT DO NOTHING`, so an
/// unconditional re-sign would not have corrupted anything — it would have
/// reported a write it did not perform, every period, forever. The decision
/// is made where it can be *named* instead.
#[tokio::test(flavor = "multi_thread")]
async fn an_unmoved_head_is_reported_rather_than_signed_again() {
    use ordo_exec::secevent::StreamCheckpointed;
    let (pool, _cfg) = fresh("ordo_sec_cp_unmoved").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let signer = cp_signer();
    seed_entries(&sec, &scope(), 0..3).await;

    let first = ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(5_000))
        .await
        .expect("a pass");
    assert!(
        matches!(&first, StreamCheckpointed::Signed(cp) if cp.upto == 3),
        "{first:?}"
    );
    let second = ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(6_000))
        .await
        .expect("a pass");
    assert_eq!(
        second,
        StreamCheckpointed::AlreadyAt(3),
        "signing an identical marker every tick is noise, and the second \
         write would have been a silent no-op reported as a success"
    );
    assert_eq!(checkpoint_rows(&pool).await, vec![3]);

    // And when the head does move, it is signed.
    seed_entries(&sec, &scope(), 3..5).await;
    let third = ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(7_000))
        .await
        .expect("a pass");
    assert!(
        matches!(&third, StreamCheckpointed::Signed(cp) if cp.upto == 5),
        "{third:?}"
    );
    assert_eq!(checkpoint_rows(&pool).await, vec![3, 5]);
}

/// **A checkpoint over nothing anchors nothing, so none is signed.**
///
/// `ordo_secevent::checkpoint` returns `None` for an empty stream and that
/// discipline survives the move to the pooled path — as an answer with a
/// name rather than as a silent nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_over_nothing_is_not_signed() {
    use ordo_exec::secevent::StreamCheckpointed;
    let (pool, _cfg) = fresh("ordo_sec_cp_empty").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let out = ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &cp_signer(), Timestamp(5_000))
        .await
        .expect("a pass");
    assert_eq!(out, StreamCheckpointed::EmptyStream);
    assert!(checkpoint_rows(&pool).await.is_empty());
}

/// **Nothing signs the head of a stream that does not recompute.**
///
/// The command chain's signer has refused this since it was written
/// (`CheckpointFailure::WouldAnchorTamperedHistory`); the security stream's
/// signing, which lived inside `retain_scope`, did not. A signature here is
/// worse than none: a checkpoint is the only lawful cut point, so the marker
/// this would have minted is a licence to delete the prefix that proves the
/// stream was edited.
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_that_does_not_recompute_is_not_anchored() {
    use ordo_exec::secevent::{CheckpointRefusal, StreamCheckpointed};
    let (pool, _cfg) = fresh("ordo_sec_cp_tampered").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    seed_entries(&sec, &scope(), 0..4).await;
    // One entry's body doctored, its stored hash left alone.
    pool.rows("UPDATE \"$secevents\" SET \"body\" = '\\xdeadbeef'::bytea WHERE \"idx\" = 1")
        .await
        .unwrap();

    let out = ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &cp_signer(), Timestamp(5_000))
        .await
        .expect("a refusal is a report, not an error");
    match out {
        StreamCheckpointed::Refused(CheckpointRefusal::WouldAnchorTamperedHistory(t)) => {
            assert_eq!(t.index, 1, "the refusal names the entry, not the window");
        }
        other => panic!("a doctored stream was anchored: {other:?}"),
    }
    assert!(
        checkpoint_rows(&pool).await.is_empty(),
        "no marker may exist over history that does not verify"
    );
}

/// **Nothing countersigns a stream entries have already been removed from
/// the end of.**
///
/// The state `audit_stream`'s check 4 has just detected. A second marker at
/// the shorter position would be a true statement about the surviving
/// entries, and that is exactly why it must not be written: it is this
/// deployment putting its own signature on the shortened history, beside the
/// one that disproves it.
#[tokio::test(flavor = "multi_thread")]
async fn a_truncated_stream_is_not_countersigned() {
    use ordo_exec::secevent::{CheckpointRefusal, StreamCheckpointed};
    let (pool, _cfg) = fresh("ordo_sec_cp_truncated").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let signer = cp_signer();
    seed_entries(&sec, &scope(), 0..5).await;
    ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(5_000))
        .await
        .expect("signed");

    pool.rows("DELETE FROM \"$secevents\" WHERE \"idx\" >= 3")
        .await
        .unwrap();

    let out = ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(6_000))
        .await
        .expect("a refusal is a report, not an error");
    assert_eq!(
        out,
        StreamCheckpointed::Refused(CheckpointRefusal::TailRemoved {
            signed: 5,
            stored: 3
        })
    );
    assert_eq!(
        checkpoint_rows(&pool).await,
        vec![5],
        "the marker that disproves the truncation stands alone"
    );
}

/// **A `$seccheckpoints` row this deployment could not have signed is not
/// signed over; it is reported.**
///
/// `put_security_checkpoint` is `ON CONFLICT (scope, upto) DO NOTHING`, so an
/// insert at a taken position returns success and stores nothing. A row
/// written by hand at the position the pass would sign could therefore
/// **suppress** checkpointing there indefinitely, with the write reporting a
/// success it did not perform — and the only visible symptom would be a
/// counter that did not move. Refusing is what surfaces it, and it agrees
/// with `audit_stream`, which already fails a stream whose newest checkpoint
/// does not verify.
#[tokio::test(flavor = "multi_thread")]
async fn a_marker_this_deployment_did_not_sign_is_reported_rather_than_signed_over() {
    use ordo_exec::secevent::{CheckpointRefusal, StreamCheckpointed};
    let (pool, _cfg) = fresh("ordo_sec_cp_foreign").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    seed_entries(&sec, &scope(), 0..3).await;

    // A marker at the head, signed by a key nobody published — the shape of
    // "an operator with database access wants checkpointing to stop here".
    let forger = ordo_commandlog::Ed25519Signer::from_seed("not-the-deployment", &[9u8; 32]);
    let head = sec.try_head(&scope()).await.unwrap().expect("a head");
    let cp = ordo_secevent::checkpoint_at(&scope(), 3, head, &forger, Timestamp(4_000));
    let tx = pool.begin().await.unwrap();
    tx.put_security_checkpoint(&cp).await.unwrap();
    tx.commit().await.unwrap();

    let out = ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &cp_signer(), Timestamp(5_000))
        .await
        .expect("a refusal is a report, not an error");
    assert_eq!(
        out,
        StreamCheckpointed::Refused(CheckpointRefusal::ForeignMarker {
            upto: 3,
            key_id: "not-the-deployment".to_string(),
        }),
        "a foreign row at the head must not be mistaken for this deployment's own work"
    );
}

/// **A marker and a stream that describe two histories is the loudest thing
/// this pass can say, and it says it.**
///
/// The state `StreamError::RetainedTail`'s doc warns about, reached the other
/// way round: emptying a scope's table makes the next append start again at
/// index 0 from genesis, so the stream silently forks a second chain through
/// the positions the first one held. The marker at position 3 still verifies
/// — it was signed by this key over the *old* chain — and the entry now
/// stored at index 2 is a different entry. Neither the signature nor the
/// chain walk can see it alone; only comparing them can.
#[tokio::test(flavor = "multi_thread")]
async fn a_marker_and_a_stream_that_describe_two_histories_is_refused() {
    use ordo_exec::secevent::{CheckpointRefusal, StreamCheckpointed};
    let (pool, _cfg) = fresh("ordo_sec_cp_two_histories").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let signer = cp_signer();
    seed_entries(&sec, &scope(), 0..3).await;
    let signed =
        match ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(4_000))
            .await
            .expect("signed")
        {
            StreamCheckpointed::Signed(cp) => cp,
            other => panic!("{other:?}"),
        };

    // The whole scope emptied and rewritten to the same length. The next
    // append restarted at index 0 from genesis, which is exactly the fork
    // `RetainedTail` exists to keep retention from manufacturing.
    pool.rows("DELETE FROM \"$secevents\"").await.unwrap();
    seed_entries(&sec, &scope(), 100..103).await;

    let out = ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(5_000))
        .await
        .expect("a refusal is a report, not an error");
    match out {
        StreamCheckpointed::Refused(CheckpointRefusal::TwoHistories {
            upto,
            signed: named,
            stored,
        }) => {
            assert_eq!(upto, 3);
            assert_eq!(named, signed.head, "the marker's own head");
            assert_ne!(stored, signed.head, "and the head storage now has");
        }
        other => panic!("a forked stream was signed over: {other:?}"),
    }
    assert_eq!(checkpoint_rows(&pool).await, vec![3]);
}

/// **A retained tail nothing signed vouches for is not extended.**
///
/// `audit_stream`'s check 2: a pruned stream anchors only at a verified
/// marker sitting exactly at its base, and without one the honest answer is
/// that it cannot be verified. Signing a *new* marker over such a tail would
/// hand the verifier a second signature over a history it has already
/// declined to believe — the deployment vouching for a suffix whose start it
/// cannot account for.
#[tokio::test(flavor = "multi_thread")]
async fn an_unanchored_retained_tail_is_not_extended() {
    use ordo_exec::secevent::{CheckpointRefusal, StreamCheckpointed};
    let (pool, _cfg) = fresh("ordo_sec_cp_unanchored").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let signer = cp_signer();
    seed_entries(&sec, &scope(), 0..6).await;
    // A lawful prune to 3, and then its anchor deleted — the state a restore
    // from a partial backup, or a hand-run `DELETE`, leaves behind.
    ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(4_000))
        .await
        .expect("signed at 6");
    pool.rows("DELETE FROM \"$secevents\" WHERE \"idx\" < 3")
        .await
        .unwrap();
    pool.rows("DELETE FROM \"$seccheckpoints\"").await.unwrap();

    let out = ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(5_000))
        .await
        .expect("a refusal is a report, not an error");
    assert_eq!(
        out,
        StreamCheckpointed::Refused(CheckpointRefusal::Unanchored {
            base: 3,
            nearest: None
        })
    );
    assert!(checkpoint_rows(&pool).await.is_empty());
}

/// **A refusal to anchor is a refusal to cut.**
///
/// The consequence of the old order, spelled out: read the head, sign it
/// unverified, then prune the prefix underneath it, and a doctored stream
/// comes out the far side verifying green under the deployment's own
/// signature — with the evidence of the edit destroyed by retention, on
/// schedule.
///
/// The cut here would have been *lawful* by `check_prune`'s four rules: the
/// anchor at 4 verifies and the entry stored at index 3 carries its head.
/// `check_prune` compares the cut point against storage and cannot see a
/// tamper anywhere else in the prefix it is about to destroy, which is why
/// the refusal has to come from the signer and not from the cut.
#[tokio::test(flavor = "multi_thread")]
async fn retention_does_not_prune_a_stream_the_signer_refused_to_anchor() {
    let (pool, _cfg) = fresh("ordo_sec_cp_no_laundering").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let signer = cp_signer();
    let verifier = signer.verifier();

    // A lawful anchor at 4, then more history, all of it older than the
    // policy's window.
    seed_entries(&sec, &scope(), 0..4).await;
    ordo_exec::secevent::checkpoint_scope(&sec, &scope(), &signer, Timestamp(5_000))
        .await
        .expect("signed");
    seed_entries(&sec, &scope(), 4..8).await;

    // Now an edit inside the prefix the checkpoint at 4 covers — and not at
    // the cut point itself, which is the only entry `check_prune` inspects.
    pool.rows("UPDATE \"$secevents\" SET \"body\" = '\\xdeadbeef'::bytea WHERE \"idx\" = 1")
        .await
        .unwrap();

    let keep = Retention {
        keep_secs: 10,
        deployment_keep_secs: 10,
    };
    let out = ordo_exec::secevent::retain_scope(
        &sec,
        &scope(),
        &signer,
        &verifier,
        keep,
        Timestamp(1_000_000),
    )
    .await
    .expect("a refused pass is a report, not an error");
    assert_eq!(
        out.pruned, 0,
        "retention destroyed the evidence of an edit it had already detected: {out:?}"
    );
    assert_eq!(out.checkpointed, None);
    assert!(
        out.refused.as_deref().unwrap_or("").contains("entry 1"),
        "the pass must say which entry stopped it: {out:?}"
    );
    assert_eq!(stored(&pool).await, 8);
}

// ── the cost audit, measured rather than estimated ───────────────────────

/// The storage and per-request numbers the checklist's audit quotes, taken
/// from the real encoder and a real table rather than from arithmetic on a
/// struct definition.
///
/// It asserts loosely — the point is that the quoted figures cannot drift
/// silently, not that a byte count is frozen — and it **prints** what it
/// measured, so re-running it is how the audit is refreshed.
#[tokio::test(flavor = "multi_thread")]
async fn the_cost_of_one_recorded_decision_is_what_the_audit_says() {
    let (pool, _cfg) = fresh("ordo_sec_cost").await;
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());

    // A representative collapsed denial: one member, one op, four sampled
    // correlations — the shape the door produces under any real load.
    let mut entry = StreamEntry::single(
        scope(),
        Subject::member("usr_01HQ8XKPZV3M2N4R6T8W0Y1B5C"),
        "openTasks",
        EventKind::denied(RecordedDecision::OpNotInRole),
        Timestamp(1_700_000_000),
        CorrelationId("cor_01HQ8XKPZV3M2N4R6T8W0Y1B5C".into()),
    );
    entry.count = 512;
    entry.correlations = (0..4)
        .map(|n| CorrelationId(format!("cor_01HQ8XKPZV3M2N4R6T8W0Y1B{n}")))
        .collect();

    let body = ordo_secevent::encode_entry(&entry);
    println!("hashed body: {} bytes", body.len());
    assert!(
        body.len() < 400,
        "the hashed body grew past the audited size: {} bytes",
        body.len()
    );

    // What a row actually costs on disk, including the two 32-byte hashes,
    // the denormalized filter columns, and Postgres's own per-tuple
    // overhead. Measured on a table of a thousand of them.
    for n in 0..1_000u32 {
        let mut e = entry.clone();
        e.first_at = Timestamp(1_700_000_000 + u64::from(n));
        e.last_at = e.first_at;
        sec.try_append(&e).await.unwrap();
    }
    let bytes: i64 = pool
        .rows("SELECT pg_total_relation_size('\"$secevents\"')")
        .await
        .unwrap()[0][0]
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let per_row = bytes / 1_000;
    println!("stored: {bytes} bytes for 1000 entries = {per_row} bytes/entry (with index)");
    assert!(
        (200..2_000).contains(&per_row),
        "stored cost per entry is outside the audited range: {per_row} bytes"
    );

    // The per-request cost: what `observe` does on a door thread. No I/O,
    // no allocation of the entry (a fold into an open window), one mutex.
    let recorder = Recorder::default();
    let start = std::time::Instant::now();
    const N: u32 = 100_000;
    for n in 0..N {
        observe(&recorder, 1_700_000_000, n);
    }
    let each = start.elapsed() / N;
    println!(
        "observe(): {each:?} per denial, {} windows open",
        recorder.open_windows()
    );
    assert_eq!(
        recorder.open_windows(),
        1,
        "a hundred thousand denials from one principal on one op must \
         collapse to ONE window, or the audit's storage numbers are wrong"
    );
    // Deliberately generous: this runs on shared CI hardware in a debug
    // build. It is here to catch an order-of-magnitude regression — a
    // round trip or an O(n) scan sneaking onto the request path — not to
    // police nanoseconds.
    assert!(
        each < std::time::Duration::from_micros(50),
        "recording a denial got expensive enough to matter on the request \
         path: {each:?} each"
    );
}
