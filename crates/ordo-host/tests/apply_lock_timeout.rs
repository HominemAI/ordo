//! 27d: a schema apply sets `lock_timeout` on every statement, retries with
//! backoff a bounded number of times, and then fails naming the table it
//! could not lock (`plan-evolution-plans.md` §3.7, board `apply-lock-timeout`).
//!
//! Before this, `schema_apply::execute` sent its DDL with no lock bound. An
//! `ALTER TABLE` queues for an `ACCESS EXCLUSIVE` lock behind every open
//! transaction on the table, and every new reader queues behind the
//! `ALTER`, so one long report held the apply and the apply held the site
//! until `statement_timeout` (30 s) cut it off with a message that named
//! neither the table nor the cause.
//!
//! Each test holds a real conflicting lock in one connection and runs the
//! apply from the pool, with a `LockPolicy` tightened to tens of
//! milliseconds so the suite stays quick. The shipped defaults are checked
//! separately, as values.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::{Duration, Instant};

use ordo_conformance::Pg;
use ordo_exec::{migrate, Pool, PoolConfig};
use ordo_host::schema_apply::{self as sa, ExecFailure, Executed, LockPolicy, Phase, Step};

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.into(),
        // The lock holder's connection, the apply's, and one for the
        // validity read-back, all open at once.
        size: 3,
        ..PoolConfig::from_env()
    }
}

/// A database holding one entity table, shaped the way `ddl_for` shapes
/// one, with no program applied: these tests are about the executor's lock
/// handling, so the steps are hand-built over it.
async fn widgets_db(name: &str) -> Pool {
    let pg = Pg::scoped(name);
    pg.exec(
        "DROP SCHEMA public CASCADE; CREATE SCHEMA public; \
         CREATE TABLE \"widgets\" (\"$space\" text NOT NULL, \"id\" text NOT NULL, \
         \"rev\" bigint NOT NULL, \"name\" text, PRIMARY KEY (\"$space\", \"id\"))",
    )
    .expect("a table to lock");
    Pool::connect(cfg(&pg.db)).await.expect("pool boots")
}

/// The additive apply's most ordinary step: an optional field.
fn add_column() -> Step {
    Step {
        code: "A2",
        phase: Phase::Alter,
        sql: "ALTER TABLE \"widgets\" ADD COLUMN \"note\" text".into(),
        index: None,
        constraint: None,
        table: "widgets".into(),
    }
}

/// The step that runs outside the transaction, on its own connection.
fn concurrent_index() -> Step {
    Step {
        code: "A4",
        phase: Phase::ConcurrentIndex,
        sql: "CREATE INDEX CONCURRENTLY \"widgets$name$ix\" ON \"widgets\" (\"$space\", \"name\")"
            .into(),
        index: Some("widgets$name$ix".into()),
        constraint: None,
        table: "widgets".into(),
    }
}

/// Tens of milliseconds in place of seconds: three attempts of 100 ms,
/// with pauses of 50 ms and 100 ms between them.
fn quick() -> LockPolicy {
    LockPolicy {
        lock_timeout: Duration::from_millis(100),
        retries: 2,
        backoff: Duration::from_millis(50),
    }
}

/// `ACCESS EXCLUSIVE` conflicts with every lock a DDL statement takes,
/// `CREATE INDEX CONCURRENTLY`'s `SHARE UPDATE EXCLUSIVE` included. Held
/// until the returned transaction is rolled back or dropped.
async fn hold_lock(pool: &Pool) -> ordo_exec::TxConn {
    let holder = pool.begin().await.expect("the holder's transaction");
    holder
        .rows("LOCK TABLE \"widgets\" IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("the holder takes the lock");
    holder
}

/// The whole §3.7 path on the transactional steps: each attempt waits
/// `lock_timeout` and gives up, the apply backs off and tries again
/// `retries` times, and the failure names the table. Nothing changed, and
/// the same apply goes through once the lock is released.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_lock_is_waited_out_retried_and_then_named() {
    let pool = widgets_db("ordo_apply_lock_tx").await;
    let holder = hold_lock(&pool).await;

    let started = Instant::now();
    let err = sa::execute_with(&pool, &[add_column()], &quick())
        .await
        .expect_err("another session holds the lock");
    let elapsed = started.elapsed();

    match &err {
        ExecFailure::LockTimeout {
            table,
            attempts,
            lock_timeout,
            sql,
            ..
        } => {
            assert_eq!(table, "widgets", "the failure names the table it waited on");
            assert_eq!(*attempts, 3, "one attempt plus two retries");
            assert_eq!(*lock_timeout, Duration::from_millis(100));
            assert!(sql.contains("ADD COLUMN"), "and the statement: {sql}");
        }
        other => panic!("a held lock is a LockTimeout, not {other:?}"),
    }
    let text = err.to_string();
    assert!(
        text.contains("could not lock table `widgets`") && text.contains("3 attempts"),
        "the message is the diagnosis, for a reader with no access to the variant: {text}"
    );
    assert!(
        text.contains("Nothing changed"),
        "and says the database is as it was: {text}"
    );
    // Three waits of 100 ms and pauses of 50 ms then 100 ms. A lower bound
    // only: a loaded machine adds time, never removes it.
    assert!(
        elapsed >= Duration::from_millis(450),
        "three attempts and two backoffs take at least 450 ms, took {elapsed:?}"
    );

    // Nothing changed.
    holder.rollback().await.expect("release");
    let live = migrate::live_shape(&pool, sa::LIVE)
        .await
        .expect("live shape");
    assert!(
        !live
            .table("widgets")
            .expect("widgets")
            .columns
            .contains_key("note"),
        "no attempt's column landed"
    );

    // Lock released: the same steps apply, with no retry needed.
    let done = sa::execute_with(&pool, &[add_column()], &quick())
        .await
        .expect("the table is free");
    assert_eq!(
        done,
        Executed {
            transactional: 1,
            validated: 0,
            indexes: 0,
            retries: 0,
        }
    );
}

/// The point of retrying: a lock that goes away during the backoff lets
/// the apply through, and the result says how many attempts it took.
#[tokio::test(flavor = "multi_thread")]
async fn a_lock_released_during_the_retries_lets_the_apply_through() {
    let pool = widgets_db("ordo_apply_lock_release").await;
    let holder = hold_lock(&pool).await;
    // Release only after the apply has been seen waiting on the lock and has
    // had time to give up once (two `lock_timeout`s). A fixed timer raced the
    // apply under CI load: it released before the first attempt began, so
    // nothing ever retried (CI, 2026-10-05).
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            let waiting = holder
                .rows(
                    // `pg_locks` is read live; `pg_stat_activity` would be
                    // the snapshot taken when this transaction began.
                    "SELECT count(*) FROM pg_locks \
                     WHERE NOT granted AND relation = '\"widgets\"'::regclass",
                )
                .await
                .ok()
                .and_then(|r| r.first()?.first()?.clone())
                .and_then(|n| n.parse::<i64>().ok())
                .unwrap_or(0);
            if waiting > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        let _ = holder.rollback().await;
    });

    let patient = LockPolicy {
        lock_timeout: Duration::from_millis(100),
        retries: 20,
        backoff: Duration::from_millis(50),
    };
    let done = sa::execute_with(&pool, &[add_column()], &patient)
        .await
        .expect("the lock was released during the retries");
    assert_eq!(done.transactional, 1);
    assert!(
        done.retries >= 1,
        "at least one attempt waited the lock out first: {done:?}"
    );
    let live = migrate::live_shape(&pool, sa::LIVE)
        .await
        .expect("live shape");
    assert!(
        live.table("widgets")
            .expect("widgets")
            .columns
            .contains_key("note"),
        "and the column landed exactly once"
    );
}

/// A concurrent index runs outside the transaction on its own connection,
/// and its lock is bounded the same way. An attempt that waited the lock
/// out leaves no INVALID index behind by its name, so the retry (and the
/// next apply) does not fail on the name.
#[tokio::test(flavor = "multi_thread")]
async fn a_concurrent_index_waits_out_its_lock_and_leaves_no_invalid_index() {
    let pool = widgets_db("ordo_apply_lock_index").await;
    let holder = hold_lock(&pool).await;

    let err = sa::execute_with(&pool, &[concurrent_index()], &quick())
        .await
        .expect_err("another session holds the lock");
    match &err {
        ExecFailure::LockTimeout {
            table, attempts, ..
        } => {
            assert_eq!(table, "widgets");
            assert_eq!(*attempts, 3);
        }
        other => panic!("a held lock is a LockTimeout, not {other:?}"),
    }
    assert_eq!(
        migrate::index_valid(&pool, sa::LIVE, "widgets$name$ix")
            .await
            .expect("read back"),
        None,
        "no index by that name, valid or otherwise"
    );

    holder.rollback().await.expect("release");
    let done = sa::execute_with(&pool, &[concurrent_index()], &quick())
        .await
        .expect("the table is free");
    assert_eq!(done.indexes, 1);
    assert_eq!(
        migrate::index_valid(&pool, sa::LIVE, "widgets$name$ix")
            .await
            .expect("read back"),
        Some(true)
    );
}

/// The `lock_timeout` a pooled connection was given does not outlive the
/// apply: the next statement on the pool runs with the server default.
#[tokio::test(flavor = "multi_thread")]
async fn the_session_setting_does_not_outlive_the_apply() {
    let pool = widgets_db("ordo_apply_lock_reset").await;
    let done = sa::execute_with(&pool, &[concurrent_index()], &quick())
        .await
        .expect("an uncontended index");
    assert_eq!(done.indexes, 1);
    // Drain the pool's connections through the same query a few times:
    // whichever one the index ran on must answer with the default too.
    for _ in 0..6 {
        let rows = pool.rows("SHOW lock_timeout").await.expect("show");
        assert_eq!(
            rows,
            vec![vec![Some("0".to_string())]],
            "a pooled connection carries no lock_timeout after the apply"
        );
    }
}

/// The settings and their shipped defaults, read as `from_env` reads them.
/// A value that is not a number reads as unset in the library; the
/// server's `RuntimeConfig::from_env` refuses the boot on it first.
#[test]
fn the_policy_reads_its_settings_with_shipped_defaults() {
    let shipped = LockPolicy {
        lock_timeout: Duration::from_millis(2_000),
        retries: 5,
        backoff: Duration::from_millis(500),
    };
    assert_eq!(
        LockPolicy::default(),
        shipped,
        "§3.7: 2 s, retried, with backoff"
    );
    assert_eq!(LockPolicy::parse(None, None, None), shipped);
    assert_eq!(
        LockPolicy::parse(Some("250"), Some("1"), Some(" 10 ")),
        LockPolicy {
            lock_timeout: Duration::from_millis(250),
            retries: 1,
            backoff: Duration::from_millis(10),
        }
    );
    assert_eq!(
        LockPolicy::parse(Some("2s"), Some("many"), Some("")),
        shipped,
        "not a number: the default, in the library"
    );
    assert_eq!(
        LockPolicy::parse(None, Some("0"), None).retries,
        0,
        "0 retries is one attempt, not none"
    );
}
