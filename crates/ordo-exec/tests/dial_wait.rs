//! A walker waiting on the node's tenant dial gate gives up after
//! `ORDO_PG_TENANT_DIAL_WAIT_MS` with a distinct error naming the setting.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use ordo_exec::{Pool, PoolConfig, PoolError};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_walker_behind_a_stuck_dial_gives_up_after_the_limit() {
    let pool = Pool::connect(PoolConfig {
        db: "postgres".into(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    pool.close_idle().await;
    // A gate of 1, held by a dial that never finishes.
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    let _stuck = gate.clone().acquire_owned().await.unwrap();

    let limit = Duration::from_millis(300);
    let walker = pool.sweeping_for(gate.clone(), limit);
    let started = Instant::now();
    let err = walker
        .rows("SELECT 1")
        .await
        .expect_err("the second walker gives up");
    assert!(started.elapsed() >= limit, "it waited the whole limit");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "it did not wait forever"
    );
    assert!(matches!(err, PoolError::DialWaitExceeded(300)), "{err:?}");
    let said = err.to_string();
    assert!(said.contains("ORDO_PG_TENANT_DIAL_WAIT_MS=300"), "{said}");
    assert!(said.contains("can be tried again"), "{said}");

    // It held no slot of the pool, and once the gate frees a walker dials.
    pool.rows("SELECT 1").await.expect("requests are served");
    drop(_stuck);
    pool.sweeping_for(gate, limit)
        .rows("SELECT 1")
        .await
        .expect("a walker passes once the gate is free");
}
