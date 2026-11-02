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
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::{Duration, Instant};

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig, PoolLimits};

const DB: &str = "ordo_pool_idle";

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

async fn pool(size: usize, idle: Option<Duration>) -> Pool {
    let cfg = cfg(size);
    ordo_exec::create_database(&cfg, DB)
        .await
        .expect("database");
    Pool::connect_with(
        cfg,
        PoolLimits {
            idle_timeout: idle,
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
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while held() != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the last test's connections never left: {}",
            held()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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

/// A pool of eight holds one connection when it is made, and only as many as
/// were wanted at once when it is used.
#[tokio::test(flavor = "multi_thread")]
async fn a_pool_dials_on_demand() {
    let _serial = SERIAL.lock().await;
    settled().await;
    let p = pool(8, Some(Duration::from_secs(60))).await;
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
    let idle = Duration::from_secs(1);
    let p = pool(4, Some(idle)).await;
    let txs = [
        p.begin().await.expect("a"),
        p.begin().await.expect("b"),
        p.begin().await.expect("c"),
    ];
    for tx in txs {
        tx.commit().await.expect("commit");
    }
    assert_eq!(held(), 3);

    // The bound is 8 periods: a busy CI runner released them after 2.4 s
    // with a 1 s period (CI 7b974765, 2026-10-12), and after 4.0 s with
    // nextest running 8 tests beside it (run 41297909915). It still catches
    // a pool that never lets go.
    let (left, took) = settle(0, idle * 10).await;
    assert_eq!(left, 0, "still holding {left} connections after {took:?}");
    assert!(
        took <= idle * 8,
        "released after {took:?}, with an idle period of {idle:?}"
    );

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
    // Uses come every 200 ms. The idle period is 15 times that, so a CI
    // runner that stalls for a second does not close the connection
    // between two uses (CI c067487d, 2026-10-09).
    let idle = Duration::from_secs(3);
    let p = pool(4, Some(idle)).await;
    let until = Instant::now() + idle * 3;
    while Instant::now() < until {
        p.rows("SELECT 1").await.expect("query");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(held(), 1, "one connection, kept because it was in use");
    drop(p);
}

/// Zero seconds is the old behaviour: what was dialled is kept.
#[tokio::test(flavor = "multi_thread")]
async fn no_idle_period_keeps_every_connection() {
    let _serial = SERIAL.lock().await;
    settled().await;
    let p = pool(4, None).await;
    let a = p.begin().await.expect("a");
    let b = p.begin().await.expect("b");
    a.commit().await.expect("a");
    b.commit().await.expect("b");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(held(), 2);
    drop(p);
}
