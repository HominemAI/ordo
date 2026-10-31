//! Pools booting together on a fresh database all come up.
//!
//! `CREATE TABLE IF NOT EXISTS` races on Postgres's catalog: two sessions
//! both pass the existence check and one fails on the unique index of the
//! table's row type. CI b15227ea (2026-10-24) lost two tests to it on
//! `$commandlog` when one retry was the only guard.
//! `Pool::ensure_platform_tables` now takes an advisory lock, so boots take
//! turns.

use std::time::Duration;

use ordo_exec::{Pool, PoolConfig, PoolLimits};

const DB: &str = "ordo_platform_tables_race";

fn cfg() -> PoolConfig {
    PoolConfig {
        db: DB.into(),
        size: 1,
        acquire_timeout: Duration::from_secs(30),
        ..PoolConfig::from_env()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn pools_booting_together_on_a_fresh_database_all_come_up() {
    for round in 0..10 {
        ordo_exec::drop_database(&cfg(), DB).await.expect("drop");
        ordo_exec::create_database(&cfg(), DB)
            .await
            .expect("create");
        let boots: Vec<_> = (0..24)
            .map(|_| tokio::spawn(Pool::connect_with(cfg(), PoolLimits::default())))
            .collect();
        for boot in boots {
            let pool = boot
                .await
                .expect("joined")
                .unwrap_or_else(|e| panic!("round {round}: a boot lost the race: {e}"));
            drop(pool);
        }
    }
}

/// A dial on a database that already has its tables takes no table lock.
///
/// The write path's first connection to a tenant runs
/// `ensure_platform_tables`. It used to alter `$sealedpayload` and then
/// `$blobref` in one transaction, each at `ACCESS EXCLUSIVE` even with
/// nothing to add, while a blob write holds `$blobref` and then wants
/// `$sealedpayload`: a deadlock, and Postgres killed one of the two.
///
/// The write is stood in for by `LOCK TABLE ... IN ROW EXCLUSIVE MODE`, the
/// lock its `INSERT`s take, in the same order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dial_on_a_made_database_does_not_deadlock_with_a_blob_write() {
    const DB2: &str = "ordo_platform_tables_dial";
    let cfg2 = || PoolConfig {
        db: DB2.into(),
        size: 3,
        acquire_timeout: Duration::from_secs(30),
        ..PoolConfig::from_env()
    };
    ordo_exec::drop_database(&cfg2(), DB2).await.expect("drop");
    ordo_exec::create_database(&cfg2(), DB2)
        .await
        .expect("create");
    let pool = Pool::connect_with(cfg2(), PoolLimits::default())
        .await
        .expect("the tables are made");

    let write = pool.begin().await.expect("a write begins");
    write
        .rows(r#"LOCK TABLE "$blobref" IN ROW EXCLUSIVE MODE"#)
        .await
        .expect("the write holds its blob reference");

    let dial = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(10),
            Pool::connect_with(cfg2(), PoolLimits::default()),
        )
        .await
    });
    // Long enough for the old dial to take `$sealedpayload` and queue on
    // `$blobref`.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let second = write
        .rows(r#"LOCK TABLE "$sealedpayload" IN ROW EXCLUSIVE MODE"#)
        .await;
    write.rollback().await.ok();

    assert!(
        second.is_ok(),
        "the write's second insert was not killed by a deadlock: {second:?}"
    );
    dial.await
        .expect("the dial task")
        .expect("the dial did not wait on the write")
        .expect("the dial succeeded");
}
