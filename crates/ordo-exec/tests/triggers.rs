//! §7's operational semantics, executed against real Postgres: at-least-once
//! with a dead-letter path, `debounce` trailing-edge batching, `coalesce`,
//! the clock's fires-once, and the listener cursor that holds the archive
//! floor.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).
//!
//! Every test owns its database. These suites run in parallel and each one
//! empties the trigger tables on entry, so a shared database has one test
//! deleting another's committed work — the failure `jobs.rs`'s own suite
//! found the first time it was written.

use std::time::Duration;

use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Image, SpaceId, Timestamp,
};
use ordo_exec::commandlog::PgStore;
use ordo_exec::prune::{self, ConsumerKind};
use ordo_exec::triggers::{self, ClockRoutine, Failed, Listener, MAX_ATTEMPTS, TRIGGERS_DDL};
use ordo_exec::{Pool, PoolConfig};
use ordo_syntax::trigger::{ListenerPattern, Modifiers, Schedule};

async fn pool(db: &str) -> Pool {
    // `Pg::named` creates the database; the pool then connects to it and
    // `ensure_platform_tables` puts the trigger tables in.
    let _ = ordo_conformance::Pg::named(db);
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    // Idempotent, and asserts the DDL is reachable from a test that did not
    // go through `Pool::connect`'s startup path.
    pool.rows(TRIGGERS_DDL).await.unwrap();
    // `$logconsumers` belongs to the pruning subsystem and is created by
    // its own `ensure_tables`, the way `$artifacts` and `$attestations`
    // are — not by `ensure_platform_tables`. The trigger tables are in the
    // platform set because accumulation rides the WRITE transaction and so
    // must exist before the first write; a consumer cursor is registered by
    // whoever starts the listener.
    prune::ensure_tables(&pool).await.unwrap();
    for t in [
        "$triggerbatches",
        "$triggerruns",
        "$deadletters",
        "$schedules",
        "$logconsumers",
        "$commandlog",
    ] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }
    pool
}

fn space() -> SpaceId {
    SpaceId("sp_trig".into())
}

fn listener(pattern: &str, modifiers: &str) -> Listener {
    Listener {
        domain: "market".into(),
        pattern: ListenerPattern::parse(pattern).unwrap(),
        routine: "seedTaskNotes".into(),
        version: "1".into(),
        modifiers: Modifiers::parse(modifiers).unwrap(),
    }
}

fn event(entity: &str, row: &str, kind: CommandKind, actor: Actor, corr: &str) -> triggers::Event {
    let g = group(entity, &[(row, kind)], actor, corr);
    triggers::events_of(&g).remove(0)
}

fn group(entity: &str, rows: &[(&str, CommandKind)], actor: Actor, corr: &str) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(corr.into()),
        actor,
        commands: rows
            .iter()
            .map(|(row, kind)| Command {
                id: CommandId(format!("cmd_{corr}_{row}")),
                space: space(),
                entity: entity.into(),
                row_id: (*row).into(),
                kind: *kind,
                before: match kind {
                    CommandKind::Create => None,
                    _ => Some(Image::new()),
                },
                after: match kind {
                    CommandKind::Delete => None,
                    _ => Some(Image::new()),
                },
                at: Timestamp(10),
            })
            .collect(),
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

async fn batches(pool: &Pool) -> Vec<(String, String)> {
    pool.rows("SELECT \"listener\", \"ids\" FROM \"$triggerbatches\" ORDER BY \"listener\"")
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            (
                r[0].clone().unwrap_or_default(),
                r[1].clone().unwrap_or_default(),
            )
        })
        .collect()
}

async fn run_count(pool: &Pool) -> usize {
    pool.rows("SELECT \"run_key\" FROM \"$triggerruns\"")
        .await
        .unwrap()
        .len()
}

/// The property everything else rests on, restated for the trigger layer:
/// a write that does not commit produces no events.
///
/// `jobs.rs` proved it for `enqueue`; accumulation into a batch rides the
/// same transaction for the same reason, so it must hold here too or the
/// two halves of "fires only on commit" would disagree.
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_write_accumulates_nothing() {
    let pool = pool("ordo_trig_commit").await;
    let l = listener("market.tasks.ingested", "");
    let e = event(
        "tasks",
        "task_6",
        CommandKind::Create,
        Actor::Source("hub".into()),
        "cg_1",
    );

    let tx = pool.begin().await.unwrap();
    triggers::accumulate(&tx, &l, &e, 100).await.unwrap();
    drop(tx); // the crash-shaped ending: no commit, no rollback call
    assert!(
        batches(&pool).await.is_empty(),
        "an aborted write must leave no consequences behind"
    );

    let tx = pool.begin().await.unwrap();
    triggers::accumulate(&tx, &l, &e, 100).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(batches(&pool).await.len(), 1);
}

/// §7: "a run is keyed by **batch id × routine version** — a debounced batch
/// is one identity; redelivery is a no-op".
///
/// Both halves. Ten events collapse into one run because they are one batch,
/// and firing the same batch twice inserts once because the run key is a
/// primary key and the second insert is a no-op in the database rather than
/// in a check above it.
#[tokio::test(flavor = "multi_thread")]
async fn a_debounced_batch_is_one_run_and_redelivery_is_a_no_op() {
    let pool = pool("ordo_trig_batch").await;
    let l = listener("market.tasks.written", "debounce 10s");

    let tx = pool.begin().await.unwrap();
    for i in 0..10 {
        let e = event(
            "tasks",
            &format!("task_{i}"),
            CommandKind::Converge,
            Actor::User("mem_1".into()),
            &format!("cg_{i}"),
        );
        triggers::accumulate(&tx, &l, &e, 100 + i as u64)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    assert_eq!(batches(&pool).await.len(), 1, "ten events, one open batch");

    // Inside the window: nothing fires. Trailing edge means the burst has to
    // stop before the run starts.
    let tx = pool.begin().await.unwrap();
    assert!(triggers::fire_due(&tx, std::slice::from_ref(&l), 112)
        .await
        .unwrap()
        .is_empty());
    tx.commit().await.unwrap();
    assert_eq!(run_count(&pool).await, 0);

    // Ten seconds after the last event, one run.
    let tx = pool.begin().await.unwrap();
    let fired = triggers::fire_due(&tx, std::slice::from_ref(&l), 119)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(run_count(&pool).await, 1);

    // The run carries every row id the batch accumulated, deduplicated,
    // which is what `$trigger.ids` is for.
    let tx = pool.begin().await.unwrap();
    let runs = triggers::claim_runs(&tx, 200, 10).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].ids.len(), 10);
    assert_eq!(runs[0].events.len(), 10);

    // Redelivery: re-enqueueing the same batch is a no-op, not a second run.
    let tx = pool.begin().await.unwrap();
    pool.rows(&format!(
        "INSERT INTO \"$triggerbatches\" \
           (\"space\", \"listener\", \"batch\", \"first_at\", \"last_at\", \"ids\", \"events\") \
         VALUES ('{}', '{}', '{}', 100, 100, '', '')",
        space().0,
        l.key(),
        runs[0].batch,
    ))
    .await
    .unwrap();
    let again = triggers::fire_due(&tx, std::slice::from_ref(&l), 300)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(again.len(), 1, "it did fire again");
    assert_eq!(
        run_count(&pool).await,
        1,
        "…and the run key already existed, so nothing was added"
    );
}

/// §7: "`coalesce` caps pending runs at one".
///
/// And the half that matters more: a coalesced batch that cannot fire is
/// **kept**, not dropped. At-least-once is not negotiable against a
/// modifier, so events that arrive while a run is in flight wait for it.
#[tokio::test(flavor = "multi_thread")]
async fn coalesce_caps_pending_runs_at_one_without_losing_events() {
    let pool = pool("ordo_trig_coalesce").await;
    let l = listener("market.tasks.written", "coalesce");

    let tx = pool.begin().await.unwrap();
    let e = event(
        "tasks",
        "task_1",
        CommandKind::Converge,
        Actor::User("mem_1".into()),
        "cg_1",
    );
    triggers::accumulate(&tx, &l, &e, 100).await.unwrap();
    triggers::fire_due(&tx, std::slice::from_ref(&l), 100)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(run_count(&pool).await, 1);

    // A second burst while the first run is still pending.
    let tx = pool.begin().await.unwrap();
    let e2 = event(
        "tasks",
        "task_2",
        CommandKind::Converge,
        Actor::User("mem_1".into()),
        "cg_2",
    );
    triggers::accumulate(&tx, &l, &e2, 200).await.unwrap();
    let fired = triggers::fire_due(&tx, std::slice::from_ref(&l), 200)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(fired.is_empty(), "at most one pending run");
    assert_eq!(run_count(&pool).await, 1);
    assert_eq!(
        batches(&pool).await.len(),
        1,
        "the batch is KEPT — dropping it would lose task_2"
    );

    // The first run finishes; the held batch is now free to fire.
    let tx = pool.begin().await.unwrap();
    let runs = triggers::claim_runs(&tx, 200, 10).await.unwrap();
    triggers::finish_run(&tx, &runs[0]).await.unwrap();
    let fired = triggers::fire_due(&tx, std::slice::from_ref(&l), 300)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(fired.len(), 1);
    let tx = pool.begin().await.unwrap();
    let runs = triggers::claim_runs(&tx, 300, 10).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(runs[0].ids, vec!["task_2"], "the held event ran");
}

/// §7: "failures retry with backoff, then **dead-letter as an operator
/// alert**".
///
/// The whole ladder, and the thing at the bottom of it: a dead letter is a
/// row an operator can enumerate, carrying the payload, so the batch's ids
/// survive for whoever replays it. Neither an infinite retry (a listener
/// pinning the floor forever while doing nothing) nor a silent drop
/// (at-least-once quietly becoming zero-times).
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_run_backs_off_then_dead_letters() {
    let pool = pool("ordo_trig_deadletter").await;
    let l = listener("market.tasks.written", "");

    let tx = pool.begin().await.unwrap();
    let e = event(
        "tasks",
        "task_1",
        CommandKind::Converge,
        Actor::User("mem_1".into()),
        "cg_1",
    );
    triggers::accumulate(&tx, &l, &e, 100).await.unwrap();
    triggers::fire_due(&tx, std::slice::from_ref(&l), 100)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut now = 100u64;
    let mut last = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let tx = pool.begin().await.unwrap();
        let runs = triggers::claim_runs(&tx, now, 10).await.unwrap();
        assert_eq!(runs.len(), 1, "attempt {attempt} must be claimable");
        assert_eq!(runs[0].attempts, attempt);
        let outcome = triggers::fail_run(&tx, &runs[0], "the routine blew up", now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        last = Some(outcome);
        if let Failed::Retrying { at } = outcome {
            // A claim before the backoff elapses finds nothing — that is the
            // backoff being real rather than advisory.
            let tx = pool.begin().await.unwrap();
            assert!(
                triggers::claim_runs(&tx, at - 1, 10)
                    .await
                    .unwrap()
                    .is_empty(),
                "still backing off"
            );
            tx.commit().await.unwrap();
            now = at;
        }
    }
    assert_eq!(last, Some(Failed::DeadLettered));
    assert_eq!(run_count(&pool).await, 0, "no longer in the queue");

    let dl = triggers::dead_letters(&pool, &space()).await.unwrap();
    assert_eq!(dl.len(), 1);
    assert_eq!(dl[0].attempts, MAX_ATTEMPTS);
    assert_eq!(dl[0].listener, l.key());
    assert_eq!(dl[0].reason, "the routine blew up");
}

/// §7: "the archive floor never passes the slowest registered consumer, and
/// **a listener cursor is never evicted** from that calculation … A stalled
/// *mirror* may be evicted so one abandoned device cannot pin a space's
/// history forever; a listener may not, because evicting one and resuming it
/// at the current floor would silently turn at-least-once into zero-times."
///
/// Two consumers, both stalled well past the policy's window, and only one
/// of them leaves.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_listener_holds_the_floor_and_a_stalled_mirror_does_not() {
    let pool = pool("ordo_trig_floor").await;
    let sp = space();
    let l = listener("market.tasks.written", "");

    triggers::register_listener(&pool, &sp, &l, 3, 0)
        .await
        .unwrap();
    prune::register_consumer(&pool, &sp, "phone", ConsumerKind::Mirror, 1, 0)
        .await
        .unwrap();

    // Long past any stall window — both are abandoned by the clock's
    // reckoning. The KIND is what decides, not the idleness.
    let policy = ordo_exec::cadence::LogPolicy::DEFAULT;
    let much_later = 10 * policy.consumer_stall.as_secs();
    let f = prune::floor(&pool, &sp, &policy, much_later).await.unwrap();
    let evicted: Vec<&str> = f.evicted.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        evicted,
        vec!["phone"],
        "the mirror leaves, the listener stays"
    );
    assert!(
        f.live_bound <= 3,
        "the floor cannot pass the listener at position 3, it is at {}",
        f.live_bound
    );

    // And the supported way out is explicit: deregistration, not a timeout.
    assert!(prune::deregister_consumer(&pool, &sp, &l.key())
        .await
        .unwrap());
    let after = prune::consumers(&pool, &sp).await.unwrap();
    assert!(after.iter().all(|c| c.name != l.key()));
}

/// Reading history forward: a listener drains the chain from its cursor,
/// accumulates only what its pattern matches, and advances only after the
/// batch has committed.
///
/// Ordering is **per space only**, which is §7's promise and the only one a
/// per-space chain can keep.
#[tokio::test(flavor = "multi_thread")]
async fn draining_the_log_wakes_only_matching_listeners() {
    let pool = pool("ordo_trig_drain").await;
    let sp = space();
    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());

    // Three groups: an ingest on tasks, an ordinary user write on tasks, and
    // an ingest on a different entity.
    for g in [
        group(
            "tasks",
            &[("task_6", CommandKind::Create)],
            Actor::Source("hub".into()),
            "cg_1",
        ),
        group(
            "tasks",
            &[("task_1", CommandKind::Converge)],
            Actor::User("mem_1".into()),
            "cg_2",
        ),
        group(
            "taskNotes",
            &[("note_9", CommandKind::Create)],
            Actor::Source("hub".into()),
            "cg_3",
        ),
    ] {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&g).await.unwrap();
        tx.commit().await.unwrap();
    }

    let ingests = listener("market.tasks.ingested", "");
    triggers::register_listener(&pool, &sp, &ingests, 0, 0)
        .await
        .unwrap();
    let read = triggers::drain_listener(&pool, &store, &sp, &ingests, 100, 500)
        .await
        .unwrap();
    assert_eq!(read, 3, "the drain read the whole chain");
    assert_eq!(triggers::cursor(&pool, &sp, &ingests).await.unwrap(), 3);

    let b = batches(&pool).await;
    assert_eq!(b.len(), 1);
    assert!(b[0].1.contains("task_6"), "the ingest woke it");
    assert!(!b[0].1.contains("task_1"), "a user write is not an ingest");
    assert!(
        !b[0].1.contains("note_9"),
        "a different entity is not this pattern"
    );

    // Draining again reads nothing: the cursor is where it left off.
    assert_eq!(
        triggers::drain_listener(&pool, &store, &sp, &ingests, 100, 600)
            .await
            .unwrap(),
        0
    );

    // A second listener, registered later, starts where it says it starts —
    // registration takes a position, and `ON CONFLICT DO NOTHING` means
    // re-registering never moves an existing cursor in either direction.
    let writes = Listener {
        routine: "auditAll".into(),
        ..listener("market.*.written", "")
    };
    triggers::register_listener(&pool, &sp, &writes, 0, 0)
        .await
        .unwrap();
    triggers::drain_listener(&pool, &store, &sp, &writes, 100, 700)
        .await
        .unwrap();
    let b = batches(&pool).await;
    let audit = b.iter().find(|(k, _)| k == &writes.key()).unwrap();
    for row in ["task_6", "task_1", "note_9"] {
        assert!(audit.1.contains(row), "`*.written` sees {row}");
    }
}

/// The clock: fires-once per schedule, per its **own** period.
///
/// `claim_schedule` rather than `claim_due`, for the reason `jobs.rs`
/// records — `claim_due` advances every due schedule by the one period it
/// was handed, so two routines with different cadences would have the loser
/// silently adopt the winner's. Two cadences is the ordinary case for
/// routines, not the exotic one.
#[tokio::test(flavor = "multi_thread")]
async fn the_clock_fires_once_per_schedule_at_its_own_cadence() {
    let pool = pool("ordo_trig_clock").await;
    let sp = space();

    let nightly = ClockRoutine {
        routine: "nightlyStale".into(),
        version: "1".into(),
        schedule: Schedule::parse("every day 02:00Z").unwrap(),
    };
    let heartbeat = ClockRoutine {
        routine: "heartbeat".into(),
        version: "1".into(),
        schedule: Schedule::parse("every 5m").unwrap(),
    };
    let routines = [nightly.clone(), heartbeat.clone()];

    // Registered just after midnight: the nightly is due at 02:00, the
    // heartbeat five minutes from now.
    let now = 60;
    triggers::register_clock(&pool, &routines, now)
        .await
        .unwrap();
    assert!(triggers::fire_clock(&pool, &sp, &routines, now)
        .await
        .unwrap()
        .is_empty());

    // 05:00 minute: the heartbeat is due, the nightly is not.
    let at = 60 + 300;
    let fired = triggers::fire_clock(&pool, &sp, &routines, at)
        .await
        .unwrap();
    assert_eq!(fired.len(), 1);
    assert!(fired[0].contains("heartbeat"), "{fired:?}");

    // Fires-once: a second replica at the same instant claims nothing,
    // because the `next_due` advance committed with the claim.
    assert!(triggers::fire_clock(&pool, &sp, &routines, at)
        .await
        .unwrap()
        .is_empty());

    // 02:00: the nightly. And the heartbeat did NOT inherit a day's period
    // when the nightly fired, nor the nightly a five-minute one.
    let at = 2 * 3600;
    let fired = triggers::fire_clock(&pool, &sp, &routines, at)
        .await
        .unwrap();
    assert_eq!(fired.len(), 2, "both are due by now: {fired:?}");

    // §7: "Clock-fired runs get an empty id set."
    let tx = pool.begin().await.unwrap();
    let runs = triggers::claim_runs(&tx, at, 10).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(runs.len(), 3);
    for r in &runs {
        assert!(r.ids.is_empty(), "{}: a clock run names no rows", r.run_key);
        assert!(r.context(at).ids.is_empty());
    }
}
