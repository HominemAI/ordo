//! 20a: one space must not be able to hold every connection.
//!
//! The failure `audit-topology-gaps.md` §2.7 describes is a *deployment*
//! failure rather than a query failure: one tenant's slow work takes the
//! whole pool, and every other tenant's request then fails `Saturated`
//! having done nothing wrong. So the test is written as the comparison,
//! with the reserve at zero standing in for the pool as it was: the same
//! body, the same load, and the only difference is whether a space has a
//! share it cannot exceed.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_crypto::SpaceId;
use ordo_exec::{Pool, PoolConfig, PoolError, PoolLimits};

const DB: &str = "ordo_noisy_neighbour";

fn cfg(size: usize) -> PoolConfig {
    PoolConfig {
        db: DB.into(),
        size,
        // Short, because every assertion below is about a checkout that
        // does not come back, and a two second wait per refusal would make
        // this test slower than the thing it proves.
        acquire_timeout: Duration::from_millis(300),
        ..PoolConfig::from_env()
    }
}

async fn pool(size: usize, reserve: usize) -> Pool {
    let cfg = cfg(size);
    ordo_exec::create_database(&cfg, DB)
        .await
        .expect("database");
    Pool::connect_with(
        cfg,
        PoolLimits {
            space_reserve: Some(reserve),
            ..PoolLimits::default()
        },
    )
    .await
    .expect("boot")
}

/// How far a noisy space gets, and whether a quiet one still reads a row
/// while it holds what it got.
async fn starve(reserve: usize) -> (usize, Result<(), PoolError>) {
    let pool = pool(4, reserve).await;
    let noisy = pool.for_space(&SpaceId("space_noisy".into()));
    let quiet = pool.for_space(&SpaceId("space_quiet".into()));

    // The noisy tenant takes everything it is allowed to take and holds it
    // in open transactions, which is what a slow scan looks like to the
    // pool.
    let mut held = Vec::new();
    for _ in 0..4 {
        match noisy.begin().await {
            Ok(tx) => held.push(tx),
            Err(PoolError::Saturated) => break,
            Err(e) => panic!("unexpected: {e}"),
        }
    }
    let progress = quiet.rows("SELECT 1").await.map(|_| ());
    (held.len(), progress)
}

/// The starvation itself, and the share that ends it.
#[tokio::test(flavor = "multi_thread")]
async fn one_space_cannot_hold_every_connection() {
    // Reserve zero is the pool as it was before 20a: no share, so the
    // first space to ask takes all four connections and the second gets
    // the deployment's concurrency bound in the face.
    let (held, progress) = starve(0).await;
    assert_eq!(held, 4, "with no share, one space takes the whole pool");
    assert!(
        matches!(progress, Err(PoolError::Saturated)),
        "this is the incident: the quiet space is refused, got {progress:?}"
    );

    // With one connection reserved, the same load leaves the quiet space a
    // connection, and it reads a row while the noisy space still holds
    // three.
    let (held, progress) = starve(1).await;
    assert_eq!(held, 3, "the noisy space is capped at `size - reserve`");
    progress.expect("the quiet space still makes progress");
}

/// The cap is a cap on **concurrency**, not a quota: the connections come
/// back and the space is served again.
#[tokio::test(flavor = "multi_thread")]
async fn a_capped_space_is_served_again_once_it_lets_go() {
    let pool = pool(4, 1).await;
    let noisy = pool.for_space(&SpaceId("space_recovers".into()));

    let held: Vec<_> = {
        let mut v = Vec::new();
        for _ in 0..3 {
            v.push(noisy.begin().await.expect("up to the cap"));
        }
        v
    };
    assert!(
        matches!(noisy.rows("SELECT 1").await, Err(PoolError::Saturated)),
        "the fourth checkout is refused while three are held"
    );
    for tx in held {
        tx.commit().await.expect("commit");
    }
    noisy
        .rows("SELECT 1")
        .await
        .expect("and it is served again");
}

/// A handle charged to no space keeps the bound it always had: the pool's
/// size, and nothing else. Boot-time migrations, operator commands and the
/// loops that walk every target run on one, and a share they did not ask
/// for would be a new way for them to fail.
#[tokio::test(flavor = "multi_thread")]
async fn an_untagged_handle_is_bounded_by_the_pool_alone() {
    let pool = pool(4, 1).await;
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(pool.begin().await.expect("untagged checkouts are uncapped"));
    }
    assert_eq!(held.len(), 4);
    assert!(matches!(
        pool.rows("SELECT 1").await,
        Err(PoolError::Saturated)
    ));
}

// ── the statement bound ─────────────────────────────────────────────────

async fn bounded_pool(statement_timeout: Option<Duration>) -> Pool {
    let cfg = cfg(2);
    ordo_exec::create_database(&cfg, DB)
        .await
        .expect("database");
    Pool::connect_with(
        cfg,
        PoolLimits {
            space_reserve: None,
            statement_timeout,
            ..PoolLimits::default()
        },
    )
    .await
    .expect("boot")
}

/// The other half of 20a: a statement that runs forever is refused rather
/// than held, and refused as itself rather than as a 500.
#[tokio::test(flavor = "multi_thread")]
async fn a_statement_past_the_bound_is_refused_not_hung() {
    let pool = bounded_pool(Some(Duration::from_millis(200))).await;

    let started = std::time::Instant::now();
    let e = pool
        .rows("SELECT pg_sleep(5)")
        .await
        .expect_err("five seconds does not fit in two hundred milliseconds");
    assert!(
        matches!(e, PoolError::StatementTimeout(_)),
        "the SQLSTATE must be read, or this arrives as `db:` and then as a 500: {e:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "it refused at the bound rather than running to completion"
    );
    // The class prefix is the whole seam to the wire inventory: spelled
    // any other way it is `internal`, which is the failure
    // `ordo-host/tests/http.rs::every_raised_class_prefix_is_a_wire_class`
    // exists to catch.
    assert!(format!("{e}").starts_with("statement_timeout: "), "{e}");

    // A cancelled statement costs the pool nothing: the connection comes
    // back and serves the next caller.
    pool.rows("SELECT 1").await.expect("the pool is intact");
}

/// Inside a transaction too, and the transaction is dead afterwards, which
/// is Postgres's own rule rather than ours. The pool's answer to a dead
/// transaction is the one it always had: the guard is dropped, the socket
/// closes, and the connection is re-dialled.
#[tokio::test(flavor = "multi_thread")]
async fn the_bound_reaches_a_statement_inside_a_transaction() {
    let pool = bounded_pool(Some(Duration::from_millis(200))).await;
    let tx = pool.begin().await.expect("begin");
    let e = tx.rows("SELECT pg_sleep(5)").await.expect_err("bounded");
    assert!(matches!(e, PoolError::StatementTimeout(_)), "{e:?}");
    drop(tx);
    pool.rows("SELECT 1").await.expect("the pool re-dials");
}

/// The bound is a policy, not a law: an operator who needs a migration
/// window turns it off, and the same statement runs.
#[tokio::test(flavor = "multi_thread")]
async fn the_bound_can_be_turned_off() {
    let pool = bounded_pool(None).await;
    pool.rows("SELECT pg_sleep(0.4)")
        .await
        .expect("no bound, no cancel");
}

/// The arithmetic of the share, at the sizes a deployment actually runs.
#[test]
fn the_share_leaves_a_small_pool_alone_and_bounds_a_large_one() {
    let computed = PoolLimits::default();
    assert_eq!(computed.space_max(8), 6, "a quarter of eight is reserved");
    assert_eq!(computed.space_max(4), 3);
    // Two connections cannot both serve a tenant and reserve one, so the
    // floor wins and the cap stops being a bound rather than halving a
    // single-tenant deployment.
    assert_eq!(computed.space_max(2), 2);
    let stated = |r| PoolLimits {
        space_reserve: Some(r),
        ..PoolLimits::default()
    };
    assert_eq!(stated(0).space_max(8), 8, "reserve zero is the share off");
    assert_eq!(stated(4).space_max(8), 4);
    assert_eq!(
        stated(99).space_max(8),
        2,
        "a reserve past the pool's size floors rather than refusing service"
    );
}
