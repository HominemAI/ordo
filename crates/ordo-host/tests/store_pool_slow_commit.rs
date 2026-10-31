//! The door's store pool survives one slow statement (board task
//! `store-pool-held-blocks-signin`).
//!
//! In CI one `INSERT INTO "$authceremony"` waited 2017 ms on its WAL fsync.
//! The pool was one connection with a 2 s wait, so every other sign-in store
//! queued behind it was refused `server_busy`. Here a second session holds a
//! row lock the ceremony store's write needs, which makes that write slow on
//! purpose and for a known time, and the passkey store, on the same pool,
//! must still answer.
//!
//! The pool is built by `ordo_host::runtime::store_pool_config`, the
//! function a server boots with, so a default that regresses fails here.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use ordo_exec::{Pool, PoolConfig};
use ordo_host::ceremonies::PgCeremonyStore;
use ordo_host::passkeys::PgPasskeyStore;
use ordo_identity::passkey::{PasskeyStore, PasskeyStoreError};
use ordo_identity::{CeremonyRow, CeremonyStore, FaultClass};

/// How long the other session holds the lock. Longer than the old 2 s wait,
/// shorter than the new 10 s one.
const HOLD: Duration = Duration::from_secs(3);

struct Fixture {
    /// Not the store pool: the session that holds the lock.
    other: Pool,
    ceremonies: Arc<PgCeremonyStore>,
    passkeys: Arc<PgPasskeyStore>,
}

async fn fixture(stem: &str, env: &[(&str, &str)]) -> Fixture {
    let pg = ordo_conformance::Pg::scoped(stem);
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    let base = PoolConfig {
        db: pg.db.clone(),
        ..PoolConfig::from_env()
    };
    let other = Pool::connect(PoolConfig {
        size: 2,
        ..base.clone()
    })
    .await
    .expect("other pool");
    ordo_host::ceremonies::ensure_table(&other)
        .await
        .expect("ceremonies");
    ordo_host::passkeys::ensure_table(&other)
        .await
        .expect("passkeys");
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let lookup = move |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let cfg = ordo_host::runtime::store_pool_config(&lookup, &base).expect("store pool config");
    let store_pool = Pool::connect(cfg).await.expect("store pool");
    let ceremonies = Arc::new(PgCeremonyStore::open_on(store_pool.clone()).expect("ceremonies"));
    let passkeys = Arc::new(PgPasskeyStore::open_on(store_pool).expect("passkeys"));
    Fixture {
        other,
        ceremonies,
        passkeys,
    }
}

fn row() -> CeremonyRow {
    CeremonyRow {
        handle: "h-slow".into(),
        connection: "passkey".into(),
        phase: "assert".into(),
        binder: String::new(),
        payload: "{}".into(),
        minted_at: ordo_identity::Timestamp(1_800_000_000),
        expires_at: ordo_identity::Timestamp(1_800_000_300),
    }
}

/// Start the ceremony store's write while another session holds the row it
/// writes, wait until Postgres shows it waiting on that lock, then time one
/// passkey store read on the same pool. The lock is let go after [`HOLD`].
async fn read_while_a_write_is_stuck(fx: &Fixture) -> (Duration, Result<(), PasskeyStoreError>) {
    // The same row, uncommitted, in the other session: the store's
    // `INSERT ... ON CONFLICT` waits for this transaction to end.
    let tx = fx.other.begin().await.expect("other session");
    let r = row();
    tx.rows(&format!(
        r#"INSERT INTO "$authceremony"
             ("handle", "connection", "phase", "binder", "payload", "minted_at", "expires_at")
           VALUES ('{}', 'passkey', 'assert', '', '{{}}', 1, 2)"#,
        r.handle
    ))
    .await
    .expect("held row");

    let ceremonies = fx.ceremonies.clone();
    let write = tokio::task::spawn_blocking(move || ceremonies.put(&row()));

    // Deterministic, not a sleep: wait until the write is in the database
    // and waiting on the lock.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let rows = fx
            .other
            .rows(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE datname = current_database() AND wait_event_type = 'Lock'",
            )
            .await
            .expect("activity");
        if rows[0][0].as_deref() == Some("1") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the write never reached the lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let release = tokio::spawn(async move {
        tokio::time::sleep(HOLD).await;
        tx.rollback().await.expect("released");
    });

    let passkeys = fx.passkeys.clone();
    let started = Instant::now();
    let read = tokio::task::spawn_blocking(move || passkeys.get(b"no-such-key").map(|_| ()))
        .await
        .expect("read thread");
    let took = started.elapsed();

    release.await.expect("release");
    write
        .await
        .expect("write thread")
        .expect("the slow write lands once the lock goes");
    (took, read)
}

fn is_busy(e: &PasskeyStoreError) -> bool {
    matches!(e, PasskeyStoreError::Unavailable(f) if f.class() == FaultClass::Busy)
}

#[tokio::test(flavor = "multi_thread")]
async fn another_store_is_served_while_one_statement_is_slow() {
    let fx = fixture("store_pool_slow_default", &[]).await;
    let (took, read) = read_while_a_write_is_stuck(&fx).await;
    read.expect("the passkey store answers on another connection");
    assert!(
        took < Duration::from_secs(1),
        "it did not wait behind the slow write: {took:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn one_connection_still_rides_out_a_slow_statement_within_its_wait() {
    // Every connection stuck at once is what a disk stall looks like; the
    // default wait outlasts one longer than the 2 s seen in CI.
    let fx = fixture("store_pool_slow_one", &[("ORDO_PG_STORE_POOL_SIZE", "1")]).await;
    let (took, read) = read_while_a_write_is_stuck(&fx).await;
    read.expect("the passkey store answers once the write lets go");
    assert!(
        took >= HOLD - Duration::from_millis(500),
        "it waited behind the write: {took:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_old_pool_turns_the_other_store_away() {
    // The configuration before the fix: one connection, a 2 s wait. Kept
    // so this file shows what the tests above guard against.
    let fx = fixture(
        "store_pool_slow_old",
        &[
            ("ORDO_PG_STORE_POOL_SIZE", "1"),
            ("ORDO_PG_STORE_ACQUIRE_TIMEOUT_MS", "2000"),
        ],
    )
    .await;
    let (_, read) = read_while_a_write_is_stuck(&fx).await;
    let e = read.expect_err("refused while the one connection is stuck");
    assert!(is_busy(&e), "refused as busy: {e}");
}

#[test]
fn a_bad_value_refuses_naming_the_setting() {
    let base = PoolConfig::from_env();
    for (var, bad) in [
        ("ORDO_PG_STORE_POOL_SIZE", "0"),
        ("ORDO_PG_STORE_POOL_SIZE", "four"),
        ("ORDO_PG_STORE_ACQUIRE_TIMEOUT_MS", "60001"),
    ] {
        let env = |k: &str| (k == var).then(|| bad.to_string());
        let e = ordo_host::runtime::store_pool_config(&env, &base)
            .expect_err("refused")
            .to_string();
        assert!(e.contains(var), "{e}");
    }
    let defaults = ordo_host::runtime::store_pool_config(&|_| None, &base).expect("defaults");
    assert_eq!(defaults.size, 4);
    assert_eq!(defaults.acquire_timeout, Duration::from_secs(10));
}
