//! Owner, 2026-10-23: the last successful decrypt per key epoch survives a
//! restart. A second ledger over the same database (the new process) shows
//! the earlier success and the failure that began after it, and a read
//! writes nothing: only `flush` does.
//!
//! Needs the conformance cluster. Its own database, `ordo_rt_decrypt_seen`.

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::decrypt_seen::{DecryptLedger, Seen, TABLE};

const DB: &str = "ordo_rt_decrypt_seen";

async fn fresh_pool() -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {DB}"));
    let pg = Pg {
        db: DB.to_string(),
        ..Pg::default()
    };
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("clean");
    Pool::connect(PoolConfig {
        db: DB.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_last_successful_decrypt_survives_a_restart() {
    let pool = fresh_pool().await;
    DecryptLedger::ensure_table(&pool).await.expect("table");

    let before = DecryptLedger::default();
    before.succeeded("acme", 1, 1_000);
    before.failed("acme", 1, 1_060);
    before.failed("acme", 1, 1_090);
    let count = || async {
        pool.rows(&format!("SELECT count(*) FROM \"{TABLE}\""))
            .await
            .unwrap()[0][0]
            .clone()
            .unwrap()
    };
    assert_eq!(count().await, "0", "a read writes nothing");
    before.flush(&pool).await;
    assert_eq!(count().await, "1");

    // The restart: a new process, an empty ledger, the same database.
    let after = DecryptLedger::default();
    assert!(after.snapshot().is_empty());
    after.load(&pool).await.expect("loads");
    assert_eq!(
        after.snapshot(),
        vec![(
            "acme".to_string(),
            1,
            Seen {
                last_success: 1_000,
                failing_since: 1_060,
                last_failure: 1_090,
            }
        )]
    );
    // And the report `/health/deep` serves carries it.
    let report = after.report();
    assert_eq!(report["epochs"][0]["last_success_at"], 1_000);
    assert_eq!(report["epochs"][0]["failing_since"], 1_060);

    // A success after the restart clears the failing mark, once flushed.
    after.succeeded("acme", 1, 2_000);
    after.flush(&pool).await;
    let third = DecryptLedger::default();
    third.load(&pool).await.expect("loads");
    let s = third.snapshot()[0].2;
    assert_eq!((s.last_success, s.failing_since), (2_000, 0));
}
