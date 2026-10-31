//! F5's remainder, executed: commit-only firing, the competing job queue,
//! and clock leadership as two separate properties.

use std::time::Duration;

use ordo_exec::{Pool, PoolConfig};

/// Each test owns its database. These suites run in parallel and every
/// one of them starts by emptying the queue, so a shared database has one
/// test deleting another's committed work — which is exactly the failure
/// that showed up the first time this was written.
async fn fresh(db: &str, size: usize) -> Pool {
    let admin = ordo_conformance::Pg {
        db: "postgres".into(),
        ..ordo_conformance::Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size,
        acquire_timeout: Duration::from_millis(500),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$jobs\"").await.unwrap();
    pool.rows("DELETE FROM \"$schedules\"").await.unwrap();
    pool
}

/// The property everything else rests on: work enqueued by a write that
/// does not commit never becomes work.
#[tokio::test(flavor = "multi_thread")]
async fn triggers_fire_only_on_commit() {
    let pool = fresh("ordo_jobs_commit", 4).await;

    // An aborted write enqueues nothing — the insert rolls back with the
    // effect it belonged to.
    let tx = pool.begin().await.unwrap();
    tx.enqueue("reindex", "task_1").await.unwrap();
    drop(tx); // the crash-shaped ending: no commit, no rollback call
    let after = pool.begin().await.unwrap();
    assert!(
        after.claim_jobs(10).await.unwrap().is_empty(),
        "an aborted write must leave no consequences behind"
    );
    after.rollback().await.unwrap();

    // A committed one publishes work and effect in the same instant.
    let tx = pool.begin().await.unwrap();
    tx.enqueue("reindex", "task_2").await.unwrap();
    // Not visible to anyone else while the transaction is open.
    let peek = pool.begin().await.unwrap();
    assert!(peek.claim_jobs(10).await.unwrap().is_empty(), "not yet");
    peek.rollback().await.unwrap();
    tx.commit().await.unwrap();

    let worker = pool.begin().await.unwrap();
    let jobs = worker.claim_jobs(10).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].kind, "reindex");
    assert_eq!(jobs[0].payload, "task_2");
    worker.commit().await.unwrap();
}

/// `FOR UPDATE SKIP LOCKED` is what lets replicas compete without a
/// broker: no job is handed to two workers, and a worker that dies
/// releases its claim instead of stranding the job.
#[tokio::test(flavor = "multi_thread")]
async fn workers_compete_without_double_delivery() {
    let pool = fresh("ordo_jobs_compete", 6).await;
    let seed = pool.begin().await.unwrap();
    for i in 0..6 {
        seed.enqueue("send", &format!("msg_{i}")).await.unwrap();
    }
    seed.commit().await.unwrap();

    // Two workers claim concurrently; every job goes to exactly one.
    let a = pool.begin().await.unwrap();
    let b = pool.begin().await.unwrap();
    let got_a = a.claim_jobs(3).await.unwrap();
    let got_b = b.claim_jobs(3).await.unwrap();
    assert_eq!(got_a.len(), 3);
    assert_eq!(got_b.len(), 3);
    let mut ids: Vec<i64> = got_a.iter().chain(&got_b).map(|j| j.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 6, "no job was delivered twice");

    // A finishes; B dies mid-job. B's work returns to the queue.
    for j in &got_a {
        a.finish_job(j.id).await.unwrap();
    }
    a.commit().await.unwrap();
    drop(b);

    // Dropping B rolls it back in the background, so its row locks are
    // released a moment later, not at the drop. Under load (CI, 2026-10-05)
    // a claim made at once still saw them locked and skipped all 3. Claim
    // until they come back, bounded: the property is "released", not
    // "released before the next statement".
    let started = std::time::Instant::now();
    loop {
        let c = pool.begin().await.unwrap();
        let recovered = c.claim_jobs(10).await.unwrap();
        if recovered.len() == 3 || started.elapsed() > std::time::Duration::from_secs(10) {
            assert_eq!(
                recovered.len(),
                3,
                "a dead worker's claim is released, not stranded"
            );
            c.commit().await.unwrap();
            break;
        }
        drop(c);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Leadership and fires-once are separate properties, and the schedule
/// row's own claim is the one that actually delivers "once".
#[tokio::test(flavor = "multi_thread")]
async fn leadership_bounds_who_tries_and_the_claim_delivers_once() {
    let pool = fresh("ordo_jobs_clock", 6).await;
    let setup = pool.begin().await.unwrap();
    setup.schedule("nightly", 100).await.unwrap();
    setup.commit().await.unwrap();

    // At-most-one-holder: a second contender for the same realm fails to
    // take leadership while the first holds it.
    let leader = pool.begin().await.unwrap();
    assert!(leader.try_lead(42).await.unwrap(), "first claim wins");
    let contender = pool.begin().await.unwrap();
    assert!(
        !contender.try_lead(42).await.unwrap(),
        "a second replica does not also lead"
    );
    // ...and leadership is per realm, not global.
    assert!(contender.try_lead(43).await.unwrap());
    contender.rollback().await.unwrap();

    // Fires-once comes from the claim, NOT from leadership. Two firers
    // racing the same due schedule: one claims it, the other sees nothing.
    let x = pool.begin().await.unwrap();
    let y = pool.begin().await.unwrap();
    let due_x = x.claim_due(150, 100).await.unwrap();
    let due_y = y.claim_due(150, 100).await.unwrap();
    assert_eq!(
        due_x.len() + due_y.len(),
        1,
        "fired exactly once: {due_x:?} {due_y:?}"
    );
    x.commit().await.unwrap();
    y.commit().await.unwrap();

    // The advance committed with the claim, so it is not due again yet.
    let again = pool.begin().await.unwrap();
    assert!(again.claim_due(150, 100).await.unwrap().is_empty());
    // And it IS due once the clock passes the new time.
    assert_eq!(again.claim_due(260, 100).await.unwrap(), vec!["nightly"]);
    again.commit().await.unwrap();

    let count = pool
        .rows("SELECT \"fires\" FROM \"$schedules\" WHERE \"name\" = 'nightly'")
        .await
        .unwrap();
    assert_eq!(
        count[0][0].as_deref(),
        Some("2"),
        "two fires, no duplicates"
    );
    leader.rollback().await.unwrap();
}

/// A crash before firing leaves the schedule due; a crash after leaves it
/// advanced. There is no ordering in which it fires twice or vanishes.
#[tokio::test(flavor = "multi_thread")]
async fn a_crashed_firer_leaves_the_schedule_due() {
    let pool = fresh("ordo_jobs_crash", 4).await;
    let setup = pool.begin().await.unwrap();
    setup.schedule("hourly", 10).await.unwrap();
    setup.commit().await.unwrap();

    // Claim, then die without committing.
    let doomed = pool.begin().await.unwrap();
    assert_eq!(doomed.claim_due(20, 60).await.unwrap(), vec!["hourly"]);
    drop(doomed);

    // Still due — the fire never happened, so it must still be owed.
    let next = pool.begin().await.unwrap();
    assert_eq!(
        next.claim_due(20, 60).await.unwrap(),
        vec!["hourly"],
        "a crash before commit leaves the work owed"
    );
    next.commit().await.unwrap();
    let after = pool.begin().await.unwrap();
    assert!(after.claim_due(20, 60).await.unwrap().is_empty());
    after.rollback().await.unwrap();
}

/// The property joined to a real write: an accepted write publishes its
/// invalidation, and a REFUSED one publishes nothing — because the enqueue
/// lives in the transaction the refusal rolls back.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_publishes_its_invalidation_only_when_it_commits() {
    use std::sync::{Arc, Mutex};

    use ordo_conformance::{Fixture, Pg, WriteLog};
    use ordo_exec::Executor;

    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_jobs_write");
    let pg = Pg {
        db: "ordo_jobs_write".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = fresh("ordo_jobs_write", 4).await;
    let schema = f.schema.clone();
    let registry = f.registry.clone();
    let fixture = Arc::new(f);
    let log = Arc::new(Mutex::new(WriteLog::new()));
    let x = Executor {
        pool: pool.clone(),
        schema,
        registry,
        keys: fixture.keys.clone(),
        space: ordo_conformance::fixture_space(),
    };

    let drain = |pool: Pool| async move {
        let tx = pool.begin().await.unwrap();
        let jobs = tx.claim_jobs(50).await.unwrap();
        for j in &jobs {
            tx.finish_job(j.id).await.unwrap();
        }
        tx.commit().await.unwrap();
        jobs
    };

    // Bram cannot write in proj_1: the whole group rolls back, and the
    // invalidation rolls back with it.
    let e = x
        .write_leased(
            fixture.clone(),
            log.clone(),
            r#"renameTask($task: "task_1", $title: "refused")"#.into(),
            "mem_2".into(),
            None,
        )
        .await
        .expect_err("guard refuses");
    assert!(format!("{e}").contains("access_guard"), "{e}");
    assert!(
        drain(pool.clone()).await.is_empty(),
        "a refused write must not publish an invalidation"
    );

    // Ana's write commits, and the signal names the entity — not the rows,
    // which subscribers refetch under their own guards.
    x.write_leased(
        fixture.clone(),
        log.clone(),
        r#"renameTask($task: "task_1", $title: "published")"#.into(),
        "mem_1".into(),
        None,
    )
    .await
    .expect("Ana may write");
    let jobs = drain(pool.clone()).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0].kind, "invalidate");
    assert_eq!(jobs[0].payload, "tasks");

    // A multi-row group invalidates every entity it touched: the acl
    // create and its bootstrap access row are two entities, one commit.
    x.write_leased(
        fixture.clone(),
        log.clone(),
        r#"addBoard($name: "jobs board")"#.into(),
        "mem_3".into(),
        None,
    )
    .await
    .expect("exempt create");
    let mut kinds: Vec<String> = drain(pool.clone())
        .await
        .into_iter()
        .map(|j| j.payload)
        .collect();
    kinds.sort();
    assert_eq!(kinds, ["boards"], "the group's own entity, de-duplicated");
}

/// Reported fix `outbox-claimed-not-sent`, in the job queue: a claim of
/// `n` takes at most `n`, even when Postgres plans it as a nested loop that
/// runs the row choice again for each row it scans. The planner's own
/// switches pin that plan on the one connection the claim then uses.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_of_n_takes_at_most_n_whatever_the_plan() {
    let pool = fresh("ordo_jobs_claim_n", 1).await;
    let tx = pool.begin().await.unwrap();
    for i in 0..5 {
        tx.enqueue("reindex", &format!("task_{i}")).await.unwrap();
    }
    tx.commit().await.unwrap();
    pool.rows_in_session(&[
        "SET enable_hashjoin = off",
        "SET enable_mergejoin = off",
        "SET enable_hashagg = off",
        "SET enable_material = off",
        "SET enable_sort = off",
    ])
    .await
    .unwrap();
    let tx = pool.begin().await.unwrap();
    let claimed = tx.claim_jobs(2).await.unwrap();
    let marked = tx
        .rows("SELECT count(*) FROM \"$jobs\" WHERE \"claimed\"")
        .await
        .unwrap();
    drop(tx);
    assert_eq!(claimed.len(), 2, "the claim answered {}", claimed.len());
    assert_eq!(
        marked[0][0].as_deref(),
        Some("2"),
        "the claim marked more rows than it answered"
    );
}
