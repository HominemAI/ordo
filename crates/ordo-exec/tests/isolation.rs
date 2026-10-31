//! 8m: the isolation level per operation class, a real `40001`, and the
//! retry that makes SERIALIZABLE usable.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ordo_exec::{Isolation, OpClass, Pool, PoolConfig, PoolError};

async fn fresh(db: &str, size: usize) -> Pool {
    let admin = ordo_conformance::Pg {
        db: "postgres".into(),
        ..ordo_conformance::Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("CREATE TABLE IF NOT EXISTS \"ssi\" (\"class\" int, \"value\" int)")
        .await
        .unwrap();
    pool.rows("DELETE FROM \"ssi\"").await.unwrap();
    pool
}

async fn setting(tx: &ordo_exec::TxConn, name: &str) -> String {
    tx.rows(&format!("SELECT current_setting('{name}')"))
        .await
        .unwrap()[0][0]
        .clone()
        .unwrap()
}

/// The table in `implementation.md` §"Isolation level per operation class",
/// asserted against the server rather than against the comment that claims
/// it. `Pool::begin` used to issue a bare `BEGIN`, which inherits
/// `default_transaction_isolation` — so the pin was a sentence, not a
/// setting.
#[tokio::test(flavor = "multi_thread")]
async fn every_operation_class_gets_the_level_the_spec_pins() {
    let pool = fresh("ordo_iso_levels", 8).await;

    for (class, want) in [
        (OpClass::Read, "read committed"),
        (OpClass::AddressedWrite, "read committed"),
        (OpClass::ConvergeOrCreate, "read committed"),
        (OpClass::BulkConditional, "serializable"),
        (OpClass::MultiStatementGroup, "serializable"),
    ] {
        let tx = pool.begin_for(class).await.unwrap();
        assert_eq!(
            setting(&tx, "transaction_isolation").await,
            want,
            "{class:?} must run at {want}"
        );
        // Reads are read-only transactions — the property that makes them
        // replica-eligible, and the one that keeps the read path from ever
        // paying for a serializable snapshot.
        assert_eq!(
            setting(&tx, "transaction_read_only").await,
            if class.read_only() { "on" } else { "off" },
            "{class:?} read-only flag"
        );
        tx.rollback().await.unwrap();
    }

    // The class table and the isolation table agree.
    assert_eq!(OpClass::Read.isolation(), Isolation::ReadCommitted);
    assert_eq!(
        OpClass::BulkConditional.isolation(),
        Isolation::Serializable
    );
    assert!(OpClass::MultiStatementGroup.retryable());
    assert!(!OpClass::AddressedWrite.retryable());

    // And a read-only transaction refuses to write, which is what makes
    // "replica-eligible" a fact rather than an intention.
    let tx = pool.begin_for(OpClass::Read).await.unwrap();
    let e = tx
        .rows("INSERT INTO \"ssi\" VALUES (1, 1)")
        .await
        .unwrap_err();
    assert!(format!("{e}").contains("read-only"), "{e}");
    drop(tx);

    // `begin()` states its level too, rather than inheriting the server's.
    let tx = pool.begin().await.unwrap();
    assert_eq!(
        setting(&tx, "transaction_isolation").await,
        "read committed"
    );
    tx.rollback().await.unwrap();
}

/// A **real** `40001`, forced rather than simulated.
///
/// The canonical SSI write skew: two transactions each read the rows of one
/// class and insert into the other. Neither sees the other's insert, so
/// each commits a state that is impossible in any serial order — and
/// PostgreSQL's SSI detects the read-write dependency cycle at the second
/// commit and aborts it with `serialization_failure`.
///
/// Under READ COMMITTED (the level everything used to run at) both commit
/// happily, which is exactly the anomaly the SERIALIZABLE pin exists to
/// exclude — so this test also asserts that, to show the level is doing the
/// work rather than the retry.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_serializable_transactions_produce_a_typed_40001() {
    let pool = fresh("ordo_iso_conflict", 6).await;
    pool.rows("INSERT INTO \"ssi\" VALUES (1, 10), (1, 20), (2, 100), (2, 200)")
        .await
        .unwrap();

    let a = pool.begin_for(OpClass::BulkConditional).await.unwrap();
    let b = pool.begin_for(OpClass::BulkConditional).await.unwrap();

    // Each reads one class…
    let sum_a = a
        .rows("SELECT sum(\"value\") FROM \"ssi\" WHERE \"class\" = 1")
        .await
        .unwrap();
    let sum_b = b
        .rows("SELECT sum(\"value\") FROM \"ssi\" WHERE \"class\" = 2")
        .await
        .unwrap();
    assert_eq!(sum_a[0][0].as_deref(), Some("30"));
    assert_eq!(sum_b[0][0].as_deref(), Some("300"));

    // …and writes into the other, on the strength of what it read.
    a.rows("INSERT INTO \"ssi\" VALUES (2, 30)").await.unwrap();
    b.rows("INSERT INTO \"ssi\" VALUES (1, 200)").await.unwrap();

    // One of them cannot be serialized against the other.
    let first = a.commit().await;
    let second = b.commit().await;
    let failures: Vec<&PoolError> = [&first, &second]
        .into_iter()
        .filter_map(|r| r.as_ref().err())
        .collect();
    assert_eq!(
        failures.len(),
        1,
        "exactly one of a serialization conflict's two sides may commit: \
         {first:?} / {second:?}"
    );
    let e = failures[0];
    assert!(
        matches!(e, PoolError::Serialization(_)),
        "a 40001 must be typed as retryable, not as `db:` — got {e:?}"
    );
    assert!(e.is_retryable());
    assert!(format!("{e}").starts_with("serialization_conflict:"), "{e}");

    // The counterfactual: the same interleaving at READ COMMITTED commits
    // both sides. That is the anomaly, and it is why the pin is the pin.
    pool.rows("DELETE FROM \"ssi\"").await.unwrap();
    pool.rows("INSERT INTO \"ssi\" VALUES (1, 10), (2, 100)")
        .await
        .unwrap();
    let a = pool.begin_for(OpClass::AddressedWrite).await.unwrap();
    let b = pool.begin_for(OpClass::AddressedWrite).await.unwrap();
    a.rows("SELECT sum(\"value\") FROM \"ssi\" WHERE \"class\" = 1")
        .await
        .unwrap();
    b.rows("SELECT sum(\"value\") FROM \"ssi\" WHERE \"class\" = 2")
        .await
        .unwrap();
    a.rows("INSERT INTO \"ssi\" VALUES (2, 10)").await.unwrap();
    b.rows("INSERT INTO \"ssi\" VALUES (1, 100)").await.unwrap();
    assert!(a.commit().await.is_ok());
    assert!(
        b.commit().await.is_ok(),
        "READ COMMITTED admits the write skew — this is the anomaly, recorded"
    );
}

/// The retry, driven by a real conflict rather than a mocked one.
///
/// Two units of work run through [`Pool::retrying`] with the same write
/// skew as above. One of them loses a serialization race, is re-executed
/// from scratch by the loop, and succeeds — so both callers get an answer,
/// and the attempt counter proves the second attempt actually happened.
#[tokio::test(flavor = "multi_thread")]
async fn a_serialization_failure_is_retried_and_the_work_completes() {
    let pool = fresh("ordo_iso_retry", 8).await;
    pool.rows("INSERT INTO \"ssi\" VALUES (1, 10), (2, 100)")
        .await
        .unwrap();

    // A gate that holds both attempts inside their transactions at the same
    // moment, so their first attempts genuinely overlap. Without it the
    // conflict is a coin flip on scheduling.
    let gate = Arc::new(tokio::sync::Barrier::new(2));
    let attempts = Arc::new(AtomicU32::new(0));

    async fn skew(
        pool: Pool,
        attempts: Arc<AtomicU32>,
        gate: Arc<tokio::sync::Barrier>,
        read: i32,
        write: i32,
    ) -> Result<i64, PoolError> {
        let first = Arc::new(AtomicU32::new(0));
        pool.retrying(OpClass::BulkConditional, move |tx| {
            let attempts = attempts.clone();
            let gate = gate.clone();
            let first = first.clone();
            async move {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                let _ = n;
                let rows = tx
                    .rows(&format!(
                        "SELECT coalesce(sum(\"value\"), 0) FROM \"ssi\" WHERE \"class\" = {read}"
                    ))
                    .await?;
                let sum: i64 = rows[0][0].as_deref().unwrap_or("0").parse().unwrap_or(0);
                // Only the first attempt waits for its rival; a retry must
                // not deadlock on a barrier the other side has left.
                if first.fetch_add(1, Ordering::SeqCst) == 0 {
                    gate.wait().await;
                }
                tx.rows(&format!("INSERT INTO \"ssi\" VALUES ({write}, {sum})"))
                    .await?;
                Ok((sum, tx))
            }
        })
        .await
    }

    let a = tokio::spawn(skew(pool.clone(), attempts.clone(), gate.clone(), 1, 2));
    let b = tokio::spawn(skew(pool.clone(), attempts.clone(), gate.clone(), 2, 1));
    let ra = a.await.unwrap();
    let rb = b.await.unwrap();

    assert!(ra.is_ok(), "the retry loop must land the work: {ra:?}");
    assert!(rb.is_ok(), "the retry loop must land the work: {rb:?}");
    assert!(
        attempts.load(Ordering::SeqCst) >= 3,
        "one side must have been re-executed from scratch, not just retried \
         at the statement level (attempts = {})",
        attempts.load(Ordering::SeqCst)
    );

    // Both units of work landed exactly one row each — a retry re-executes,
    // it does not double-apply, because the losing attempt's insert rolled
    // back with its transaction.
    let rows = pool.rows("SELECT count(*) FROM \"ssi\"").await.unwrap();
    assert_eq!(
        rows[0][0].as_deref(),
        Some("4"),
        "two seeded rows plus one row per unit of work"
    );
}

/// The budget is finite, and exhausting it is a *retryable conflict* rather
/// than an internal error — `implementation.md`'s `serialization_conflict`
/// [proposed]. Forced by a body that conflicts on every attempt: it reads a
/// row a permanently-open rival keeps writing.
#[tokio::test(flavor = "multi_thread")]
async fn an_exhausted_retry_budget_is_a_typed_conflict_not_an_internal_error() {
    let pool = fresh("ordo_iso_exhaust", 8).await;
    pool.rows("INSERT INTO \"ssi\" VALUES (1, 1)")
        .await
        .unwrap();

    // A body that always fails with a serialization error, so the loop runs
    // its whole budget. `raise` with SQLSTATE 40001 is the same wire answer
    // the server gives for a real conflict, which is the thing under test
    // here — the loop's arithmetic, not SSI's detection (which the test
    // above forces for real).
    let attempts = Arc::new(AtomicU32::new(0));
    let seen = attempts.clone();
    let out: Result<(), PoolError> = pool
        .retrying(OpClass::MultiStatementGroup, move |tx| {
            let attempts = attempts.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                tx.rows(
                    "DO $$ BEGIN RAISE EXCEPTION USING ERRCODE = '40001', \
                     MESSAGE = 'could not serialize access'; END $$",
                )
                .await?;
                Ok(((), tx))
            }
        })
        .await;

    let e = out.unwrap_err();
    assert!(
        matches!(e, PoolError::RetryExhausted { attempts, .. } if attempts == pool.write_attempts()),
        "{e:?}"
    );
    assert!(
        format!("{e}").starts_with("serialization_conflict:"),
        "an exhausted budget is a retryable conflict, not `internal`: {e}"
    );
    assert_eq!(
        seen.load(Ordering::SeqCst),
        pool.write_attempts(),
        "the budget is spent, and no more than spent"
    );
}

/// A failure that is NOT a serialization failure must not be retried: the
/// budget exists for conflicts, and burning it on a syntax error turns one
/// bad request into five.
#[tokio::test(flavor = "multi_thread")]
async fn a_non_conflict_failure_is_not_retried() {
    let pool = fresh("ordo_iso_noretry", 4).await;
    let attempts = Arc::new(AtomicU32::new(0));
    let seen = attempts.clone();
    let out: Result<(), PoolError> = pool
        .retrying(OpClass::MultiStatementGroup, move |tx| {
            let attempts = attempts.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                tx.rows("SELECT * FROM \"no_such_table\"").await?;
                Ok(((), tx))
            }
        })
        .await;
    assert!(matches!(out, Err(PoolError::Db(_))), "{out:?}");
    assert_eq!(seen.load(Ordering::SeqCst), 1, "one attempt, one answer");
}

/// C1 of `plan-client-cache.md`: a read that reports its chain position takes
/// the rows and the position in one snapshot, and at that level a read-only
/// transaction cannot fail to serialize, so it is not retried.
#[test]
fn a_snapshot_read_is_repeatable_read_and_read_only() {
    assert_eq!(OpClass::SnapshotRead.isolation(), Isolation::RepeatableRead);
    assert!(OpClass::SnapshotRead.read_only());
    assert!(!OpClass::SnapshotRead.retryable());
}
