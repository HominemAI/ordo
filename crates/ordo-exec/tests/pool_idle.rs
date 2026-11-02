//! 25g: a pool holds connections in proportion to its use.
//!
//! A pool used to dial all of its connections when it was made and keep them
//! for good, so a database per tenant cost the server a pool per tenant
//! whether the tenant was there or not. It now dials one to prove the
//! database is reachable, dials more when a checkout finds none idle, and
//! closes any that have sat unused for `PoolLimits::idle_timeout`.
//!
//! The count is the server's own, from `pg_stat_activity`, which is the
//! number `max_connections` is enforced against.
//!
//! The idle period is measured on a manual clock (`PoolClock::manual`).
//! Each test moves it by hand, so "kept before one period, closed at it"
//! holds however loaded the machine is (owner ruling 2026-11-02, board task
//! `time-sensitive-tests-fail-when-other-tests`). Real time is waited on
//! only for the reaper's next wake and for the server to list a closed
//! backend as gone. Those waits fail only after a generous bound.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::{Duration, Instant};

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolClock, PoolConfig, PoolLimits};

const DB: &str = "ordo_pool_idle";

/// The idle period on the test's clock. The reaper wakes on the runtime's
/// timer at a quarter of it, so a short one keeps each test quick.
const IDLE: Duration = Duration::from_secs(1);

/// How long a real-time wait may take before it fails. It is reached only
/// when the pool never lets go, so it costs nothing on a pass.
const EVENTUALLY: Duration = Duration::from_secs(30);

/// Every test here counts the connections on one database.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg(size: usize) -> PoolConfig {
    PoolConfig {
        db: DB.into(),
        size,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    }
}

async fn pool(size: usize, idle: Option<Duration>, clock: PoolClock) -> Pool {
    let cfg = cfg(size);
    ordo_exec::create_database(&cfg, DB)
        .await
        .expect("database");
    Pool::connect_with(
        cfg,
        PoolLimits {
            idle_timeout: idle,
            clock,
            ..PoolLimits::default()
        },
    )
    .await
    .expect("boot")
}

/// Waits until the last test's connections have left the server. A dropped
/// pool closes its connections at once, but the server lists each backend
/// until it has exited, which on a busy runner can trail the drop.
async fn settled() {
    let (left, took) = settle(0, EVENTUALLY).await;
    assert_eq!(left, 0, "the last test's connections never left: {took:?}");
}

/// Server connections on the test database, not counting this one.
fn held() -> usize {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    admin
        .query(&format!(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE datname = '{DB}' AND pid <> pg_backend_pid()"
        ))
        .expect("pg_stat_activity")[0][0]
        .clone()
        .unwrap_or_default()
        .parse()
        .expect("a count")
}

/// Wait until `held()` is `want` or `limit` passes, and say how long it took.
async fn settle(want: usize, limit: Duration) -> (usize, Duration) {
    let started = Instant::now();
    loop {
        let now = held();
        if now == want || started.elapsed() >= limit {
            return (now, started.elapsed());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Give the reaper a few wakes on the runtime's timer. It waits for
/// something that must not happen, so a loaded machine can make it see
/// less but cannot make it fail.
async fn let_the_reaper_look() {
    tokio::time::sleep(IDLE).await;
}

/// The server process behind the connection a query was served on.
async fn backend(p: &Pool) -> String {
    p.rows("SELECT pg_backend_pid()").await.expect("query")[0][0]
        .clone()
        .expect("a pid")
}

/// A pool of eight holds one connection when it is made, and only as many as
/// were wanted at once when it is used.
#[tokio::test(flavor = "multi_thread")]
async fn a_pool_dials_on_demand() {
    let _serial = SERIAL.lock().await;
    settled().await;
    // A clock that never moves: nothing here may age out.
    let p = pool(8, Some(IDLE), PoolClock::manual()).await;
    assert_eq!(held(), 1, "a pool of eight is made with one connection");

    // Three checkouts at once are three connections, not eight.
    let a = p.begin().await.expect("a");
    let b = p.begin().await.expect("b");
    let c = p.begin().await.expect("c");
    assert_eq!(held(), 3);
    a.commit().await.expect("a");
    b.commit().await.expect("b");
    c.commit().await.expect("c");
    assert_eq!(held(), 3, "returned connections wait for the next checkout");
    drop(p);
}

/// Connections nobody has used for the idle period are closed, all of them,
/// and the pool dials again when it is asked.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_pool_holds_nothing_and_dials_again_when_asked() {
    let _serial = SERIAL.lock().await;
    settled().await;
    let clock = PoolClock::manual();
    let p = pool(4, Some(IDLE), clock).await;
    let txs = [
        p.begin().await.expect("a"),
        p.begin().await.expect("b"),
        p.begin().await.expect("c"),
    ];
    for tx in txs {
        tx.commit().await.expect("commit");
    }
    assert_eq!(held(), 3);

    // Just short of the period, all three are kept.
    clock.advance(IDLE - Duration::from_millis(1));
    let_the_reaper_look().await;
    assert_eq!(held(), 3, "closed before the idle period ran out");

    // At the period, all three are closed.
    clock.advance(Duration::from_millis(1));
    let (left, took) = settle(0, EVENTUALLY).await;
    assert_eq!(left, 0, "still holding {left} connections after {took:?}");

    let rows = p.rows("SELECT 1").await.expect("dials again");
    assert_eq!(rows[0][0].as_deref(), Some("1"));
    assert_eq!(held(), 1);
    drop(p);
}

/// A connection that is used is not closed under the caller: the period
/// counts from the last use, not from the dial.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_pool_keeps_the_connection_it_is_using() {
    let _serial = SERIAL.lock().await;
    settled().await;
    let clock = PoolClock::manual();
    let p = pool(4, Some(IDLE), clock).await;
    let first = backend(&p).await;

    // Nine periods pass in all, but never a whole one between two uses.
    for _ in 0..10 {
        clock.advance(IDLE * 9 / 10);
        let_the_reaper_look().await;
        assert_eq!(backend(&p).await, first, "the connection was closed");
    }
    assert_eq!(held(), 1, "one connection, kept because it was in use");

    // Left alone for the period, it goes.
    clock.advance(IDLE);
    let (left, took) = settle(0, EVENTUALLY).await;
    assert_eq!(left, 0, "still holding {left} connections after {took:?}");
    drop(p);
}

/// No idle period is the old behaviour: what was dialled is kept.
#[tokio::test(flavor = "multi_thread")]
async fn no_idle_period_keeps_every_connection() {
    let _serial = SERIAL.lock().await;
    settled().await;
    let clock = PoolClock::manual();
    let p = pool(4, None, clock).await;
    let a = p.begin().await.expect("a");
    let b = p.begin().await.expect("b");
    a.commit().await.expect("a");
    b.commit().await.expect("b");
    clock.advance(Duration::from_secs(3600));
    let_the_reaper_look().await;
    assert_eq!(held(), 2);
    drop(p);
}
