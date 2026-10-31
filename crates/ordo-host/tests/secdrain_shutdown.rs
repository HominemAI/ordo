//! `SecurityDrain::shutdown` loses nothing, even when it lands in the middle
//! of a pass.
//!
//! The drain loop takes entries out of the recorder and then awaits the
//! resolver and the store. Before this test the shutdown aborted the loop,
//! and an abort inside one of those awaits dropped the entries in hand:
//! neither written nor requeued. It showed as a flake in
//! `attached_tenant.rs` ("the twin's denial must be recorded in the twin's
//! own database") under full-suite load, when a slow target widened the
//! window. The resolver here is made slow on purpose so the window is
//! always open when the shutdown arrives.
//!
//! Databases are prefixed `ordo_secshut_`. Needs the conformance cluster
//! (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{CorrelationId, SpaceId, Timestamp};
use ordo_conformance::Pg;
use ordo_exec::route::{PoolResolver, Resolving, SinglePool, Targets};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::metrics::Metrics;
use ordo_host::secdrain::{DrainConfig, SecurityDrain};
use ordo_secevent::{RecordedDecision, Recorder, Relationship, Stamp, Subject};

const ALPHA: &str = "spc_alpha";

/// A resolver that takes its time. The drain loop holds the drained entries
/// across this wait, which is exactly where an abort would lose them.
struct Slow {
    inner: SinglePool,
    delay: Duration,
}

impl PoolResolver for Slow {
    fn resolve<'a>(&'a self, space: &'a ordo_crypto::SpaceId) -> Resolving<'a> {
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            self.inner.resolve(space).await
        })
    }

    fn targets<'a>(&'a self) -> Targets<'a> {
        self.inner.targets()
    }
}

async fn fresh(db: &str) -> Pool {
    let _ = Pg::named(db);
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    for t in ["$secevents", "$seccheckpoints", "$schedules"] {
        pool.rows(&format!("TRUNCATE TABLE \"{t}\"")).await.unwrap();
    }
    pool
}

async fn rows_in(pool: &Pool, scope: &str) -> i64 {
    pool.rows(&format!(
        "SELECT count(*) FROM \"$secevents\" WHERE \"scope\" = '{scope}'"
    ))
    .await
    .unwrap()[0][0]
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shutdown_during_a_pass_writes_the_entries_the_pass_held() {
    let control = fresh("ordo_secshut_control").await;
    let recorder = Arc::new(Recorder::default());
    let delay = Duration::from_millis(400);
    let drain = SecurityDrain::start_routed(
        recorder.clone(),
        control.clone(),
        Arc::new(Slow {
            inner: SinglePool::new(control.clone()),
            delay,
        }),
        Arc::new(Metrics::new()),
        DrainConfig {
            tick: Duration::from_millis(20),
            retain: false,
            ..DrainConfig::default()
        },
    )
    .await
    .expect("drain");

    // One denial, stamped in the past so its collapse window has elapsed
    // and the very next tick drains it into the pass.
    recorder.deny(
        Some(&SpaceId(ALPHA.into())),
        Subject::member("usr_1"),
        "openTasks",
        RecordedDecision::OpNotInRole,
        &Stamp::new(Timestamp(1_000), CorrelationId("cor_00000001".into())),
        Relationship::Member,
    );
    // Long enough for the tick to fire and the pass to be inside the slow
    // resolve, holding the entry; well short of the resolve finishing.
    tokio::time::sleep(Duration::from_millis(120)).await;

    let written = drain.shutdown().await.expect("the shutdown flush commits");
    assert_eq!(
        rows_in(&control, ALPHA).await,
        1,
        "the entry the pass held when the shutdown arrived must reach the \
         stream; an aborted pass drops it (written by shutdown: {written})"
    );
}
