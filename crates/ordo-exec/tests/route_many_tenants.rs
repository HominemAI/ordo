//! A server with many tenants walks all of them (boot, a sweep after new
//! tenants, a deploy) without holding a connection per tenant.
//!
//! Measured 2026-10-21: 121 tenants gave 122 connections in 1 second, and
//! against a Postgres with `max_connections` 100 the boot pass failed with
//! "too many clients". The connections stayed open until
//! `ORDO_PG_IDLE_SECS` passed.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::SpaceId;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};

const CRED: &str = "local";
const TENANTS: usize = 60;
const BOUND: usize = 4;
/// The boot pass, the catch-up sweep and a deploy.
const WALKERS: usize = 3;
/// Backends Postgres still lists after their client has closed them.
const EXITING: usize = 2;
const PREFIX: &str = "ordo_many_t";

fn admin() -> Pg {
    Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
}

fn on(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.into(),
        credential: CRED.into(),
        custody: DbCustody::PlatformManaged,
        attached_at: 1,
        attached_by: "mem_owner".into(),
    }
}

/// The connections open right now to the tenant databases of this test.
async fn tenant_connections(watch: &Pool) -> usize {
    let rows = watch
        .rows(&format!(
            "SELECT count(*) FROM pg_stat_activity WHERE datname LIKE '{PREFIX}%'"
        ))
        .await
        .expect("counted");
    rows[0][0].clone().unwrap().parse().unwrap()
}

/// **Walking 60 tenants holds a few connections at a time, never 60.**
/// Three walkers run together, each listing the tenants and touching every
/// one, as the boot pass, the catch-up sweep and a deploy do. Each walker
/// holds one connection at a time and closes it, so the peak is the number
/// of walkers, not the number of tenants, and nothing is left open when
/// they finish.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn walking_many_tenants_at_boot_holds_only_a_few_connections() {
    let _ = admin().exec("CREATE DATABASE ordo_many_control");
    Fixture::load()
        .expect("fixture")
        .reset(&Pg {
            db: "ordo_many_control".into(),
            ..Pg::default()
        })
        .expect("seeded");
    let control = Pool::connect(PoolConfig {
        db: "ordo_many_control".into(),
        size: 4,
        ..PoolConfig::from_env()
    })
    .await
    .expect("control");
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    for i in 0..TENANTS {
        let db = format!("{PREFIX}{i}");
        let _ = admin().exec(&format!("CREATE DATABASE {db}"));
        spacedb::attach(&control, &SpaceId(format!("spc_many_{i}")), &on(&db))
            .await
            .expect("attached");
    }
    let pools = AttachedPools::new(
        control.clone(),
        control.clone(),
        PoolConfig {
            // Fewer slots than walkers, so walkers on one tenant queue.
            size: 2,
            // What is measured here is connections, not the checkout budget.
            // With the default 2 s, the walker that queues on a tenant waits
            // for a sibling's dial and query, and a slow CI Postgres just
            // after 60 `CREATE DATABASE`s once took longer: `Saturated` (run
            // 39863717100). That is the budget doing its job, and a walker
            // takes it as a transient hold, so the test does not race it.
            acquire_timeout: Duration::from_secs(30),
            ..PoolConfig::from_env()
        },
        Arc::new(Installed::new().as_configured(CRED)),
    );
    let pools = Arc::new(with_bound(pools));

    let peak = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (peak, done, watch) = (peak.clone(), done.clone(), control.clone());
        tokio::spawn(async move {
            while !done.load(Ordering::Relaxed) {
                peak.fetch_max(tenant_connections(&watch).await, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    };

    let mut walkers = Vec::new();
    for _ in 0..WALKERS {
        let pools = pools.clone();
        walkers.push(tokio::spawn(async move {
            let targets = pools.targets().await.expect("listed");
            let mut reached = 0;
            for t in &targets {
                let pool = t.pool.as_ref().unwrap_or_else(|e| {
                    panic!("{} was not prepared: {e}", t.label);
                });
                pool.rows("SELECT 1").await.expect("touched");
                reached += 1;
            }
            reached
        }));
    }
    for w in walkers {
        assert_eq!(w.await.unwrap(), TENANTS + 1, "every tenant was reached");
    }
    done.store(true, Ordering::Relaxed);
    sampler.await.unwrap();

    let peak = peak.load(Ordering::Relaxed);
    println!("peak tenant connections: {peak} for {TENANTS} tenants");
    // What bounds the connections the walk holds is the walkers: each holds
    // one at a time. The dial gate bounds dials, and with 3 walkers under a
    // gate of 4 it is never the limit, so `BOUND` was the wrong ceiling.
    // On top of the live connections, Postgres lists a backend until it has
    // finished exiting, which lags the client's close. CI saw 5 once (run
    // 39713611832): 3 live and 2 still exiting. One per tenant would be 60.
    assert!(
        peak <= WALKERS + EXITING,
        "peak connections to tenant databases was {peak} for {TENANTS} tenants; \
         {WALKERS} walkers hold one each, plus {EXITING} backends still exiting"
    );
    // The same exit lag as above: a backend let go a moment ago may still be
    // listed. Every one must be gone within a few seconds (CI saw one still
    // listed right after the walk, run 39839422098).
    let mut left = tenant_connections(&control).await;
    for _ in 0..50 {
        if left == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        left = tenant_connections(&control).await;
    }
    assert_eq!(
        left, 0,
        "no connection is left open per tenant after the walk"
    );
}

fn with_bound(pools: AttachedPools) -> AttachedPools {
    pools.with_dial_concurrency(BOUND)
}

/// **A walker queued on the node's dial gate holds none of a tenant's
/// pool.** The gate is shared by every walker of the node, and it is
/// contended whenever the catch-up's batch and a boot pass or a deploy run
/// together, or when dials to a down cluster hold its permits until they
/// time out. A sweeping checkout that took a slot of the tenant's pool
/// first and then queued on the gate kept that slot while it waited, so
/// every other checkout on that tenant, a request's or another walker's,
/// waited behind a queue for other tenants and was refused `Saturated`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_walker_waiting_on_the_dial_gate_holds_no_pool_slot() {
    let pool = Pool::connect(PoolConfig {
        db: "postgres".into(),
        size: 1,
        acquire_timeout: Duration::from_millis(500),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    // No idle connection, so the walker's checkout must dial.
    pool.close_idle().await;
    // A gate with no permit free: the walker waits on it.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let walker = {
        let sweeping = pool.sweeping(gate.clone());
        tokio::spawn(async move { sweeping.rows("SELECT 1").await.map(|_| ()) })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    pool.rows("SELECT 1")
        .await
        .expect("a request on the same pool is served while a walker waits on the gate");
    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), walker)
        .await
        .expect("the walker finished once the gate let it dial")
        .unwrap()
        .expect("the walker's checkout succeeded");
}
