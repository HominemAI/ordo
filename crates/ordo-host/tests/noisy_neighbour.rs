//! 20a at the door's edge: the pool's two refusals, as a caller sees them.
//!
//! The mechanisms live in `ordo-exec` and are proved there
//! (`ordo-exec/tests/noisy_neighbour.rs`). What can only be asserted here
//! is the part that crosses the string seam: a refusal the pool types has
//! to arrive as its own wire class, because the alternative is a 500 that
//! a caller cannot tell from a crash.

use std::sync::Arc;
use std::time::Duration;

use ordo_crypto::SpaceId;
use ordo_exec::{Pool, PoolConfig, PoolLimits};
use ordo_host::metrics::Metrics;

/// The audit's third question: **which customer is causing this?**
///
/// A deployment that has not set `ORDO_METRICS_PER_SPACE` used to see a
/// slow deployment and no name. It now sees the name of the space holding
/// the connections, because the two series that carry it are refusal
/// series and a space that never saturates never gets one.
#[tokio::test(flavor = "multi_thread")]
async fn the_space_consuming_the_pool_is_named_with_the_opt_in_off() {
    let cfg = PoolConfig {
        db: "ordo_host_noisy_neighbour".into(),
        size: 4,
        acquire_timeout: Duration::from_millis(200),
        ..PoolConfig::from_env()
    };
    ordo_exec::create_database(&cfg, &cfg.db)
        .await
        .expect("database");
    let pool = Pool::connect_with(
        cfg,
        PoolLimits {
            space_reserve: Some(1),
            ..PoolLimits::default()
        },
    )
    .await
    .expect("boot");

    // Per-space series OFF, which is the deployment §2.7 describes.
    let metrics = Arc::new(Metrics::new().with_per_space_series(false));
    let observed = pool.observing(metrics.clone());
    let noisy = observed.for_space(&SpaceId("spc_noisy".into()));
    let quiet = observed.for_space(&SpaceId("spc_quiet".into()));

    // The noisy space takes its whole share and then asks for more.
    let held: Vec<_> = {
        let mut v = Vec::new();
        for _ in 0..3 {
            v.push(noisy.begin().await.expect("up to the cap"));
        }
        v
    };
    assert!(
        noisy.rows("SELECT 1").await.is_err(),
        "the checkout past the cap is refused"
    );
    // The quiet space is served throughout, which is M1's guarantee and is
    // also why it must not appear in either refusal series.
    quiet.rows("SELECT 1").await.expect("still served");

    let rendered = metrics.render_prometheus();
    assert!(
        rendered.contains("ordo_pool_space_capped_total{space=\"spc_noisy\"} 1"),
        "the space holding the connections is named:\n{rendered}"
    );
    assert!(
        rendered.contains("ordo_pool_saturated_total{space=\"spc_noisy\"} 1"),
        "and so is the refusal it earned:\n{rendered}"
    );
    assert!(
        !rendered.contains("spc_quiet"),
        "a space that was served carries no refusal series, which is what \
         keeps this family the size of an incident rather than a tenant \
         list:\n{rendered}"
    );
    // The healthy-path histogram is the one that would cost a series per
    // tenant, so with the opt-in off it carries no space at all.
    assert!(
        rendered.contains("ordo_pool_wait_seconds"),
        "the wait is still measured:\n{rendered}"
    );
    assert!(
        !rendered.contains("ordo_pool_wait_seconds_sum{space="),
        "and it is measured without a per-tenant label:\n{rendered}"
    );

    for tx in held {
        tx.commit().await.expect("commit");
    }
}

/// With the opt-in on, the wait itself is per space too: the question
/// stops being *"is the pool queueing"* and becomes *"for whom"*.
#[tokio::test(flavor = "multi_thread")]
async fn the_opt_in_adds_the_wait_per_space() {
    let cfg = PoolConfig {
        db: "ordo_host_noisy_neighbour".into(),
        size: 2,
        acquire_timeout: Duration::from_millis(200),
        ..PoolConfig::from_env()
    };
    ordo_exec::create_database(&cfg, &cfg.db)
        .await
        .expect("database");
    let pool = Pool::connect(cfg).await.expect("boot");
    let metrics = Arc::new(Metrics::new().with_per_space_series(true));
    let observed = pool.observing(metrics.clone());
    observed
        .for_space(&SpaceId("spc_labelled".into()))
        .rows("SELECT 1")
        .await
        .expect("a checkout");

    let rendered = metrics.render_prometheus();
    assert!(
        rendered.contains("ordo_pool_wait_seconds_count{space=\"spc_labelled\"}"),
        "{rendered}"
    );
}

/// The statement bound is a class the caller can act on, not an
/// `internal`.
#[test]
fn a_statement_timeout_reaches_the_caller_as_itself() {
    let raised = ordo_exec::PoolError::StatementTimeout(
        "canceling statement due to statement timeout".into(),
    )
    .to_string();
    let classified = ordo_host::classify(raised);
    assert_eq!(classified.code.as_str(), "statement_timeout");
    assert_eq!(
        classified.code.status().as_u16(),
        503,
        "well formed, nothing broke, and no stored row decided it"
    );
    assert!(
        classified.message.contains("statement timeout"),
        "the server's own words survive: {}",
        classified.message
    );
}

/// And the class is in the inventory in both directions, which is what
/// keeps the door from having to know about the pool.
#[test]
fn the_class_is_in_the_wire_inventory() {
    let code = ordo_api::ErrorCode::from_class("statement_timeout")
        .expect("the inventory knows the class");
    assert!(
        code.executor_raisable(),
        "the pool is below the door, so the executor must be allowed to say it"
    );
    assert!(
        !code.is_exec_conflict(),
        "nothing executed, so a replay must really run"
    );
}
