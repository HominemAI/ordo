//! Checklist 9c — `/ 0` is a **typed** runtime error, not a server fault.
//!
//! `definition-language.md` §2 pins the behaviour: "`/ 0` is a typed runtime
//! error, uniform with `mod(a, 0)`. Division is total on its domain and loud
//! off it; there is no null-on-divide-by-zero folklore." Overflow on
//! `int`/`money` is likewise "a typed runtime error, never wraparound."
//!
//! Loud is the easy half — Postgres does that on its own. **Typed** is the
//! half that needs code: without it, both arrive as an anonymous query
//! failure and reach the caller as `internal` (500), which says the server
//! broke when in fact the arithmetic simply has no answer. So the SQLSTATE
//! is read once, here, rather than by every layer above matching on English
//! error text.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_exec::{Pool, PoolConfig, PoolError};

async fn pool() -> Pool {
    Pool::connect(PoolConfig {
        db: "postgres".into(),
        size: 2,
        acquire_timeout: Duration::from_millis(500),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot")
}

#[tokio::test(flavor = "multi_thread")]
async fn division_by_zero_is_typed_arithmetic_not_an_anonymous_db_failure() {
    let pool = pool().await;

    // The shape the planner emits for `a / b`, with a zero divisor.
    match pool
        .rows("SELECT round((9500)::numeric / (0)::numeric, 12)")
        .await
    {
        Err(PoolError::Arithmetic(m)) => {
            assert!(
                m.to_lowercase().contains("division by zero"),
                "the message keeps the server's own words: {m}"
            );
        }
        other => panic!("expected a typed arithmetic error, got {other:?}"),
    }

    // `mod(a, 0)` answers the same way — DL §2 says "uniform with", and one
    // behaviour is what uniform means.
    match pool.rows("SELECT mod(9500, 0)").await {
        Err(PoolError::Arithmetic(_)) => {}
        other => panic!("expected a typed arithmetic error, got {other:?}"),
    }

    // Overflow, the other typed runtime error DL §2 names: never wraparound.
    match pool.rows("SELECT (9223372036854775807::bigint + 1)").await {
        Err(PoolError::Arithmetic(_)) => {}
        other => panic!("expected a typed arithmetic error, got {other:?}"),
    }
}

/// The class rides on the error's own Display, in the `<class>: <message>`
/// shape the write protocol already speaks — so the layer that maps engine
/// classes onto the wire inventory has something to match on that is not
/// Postgres's English.
#[tokio::test(flavor = "multi_thread")]
async fn the_typed_error_carries_its_class_prefix() {
    let pool = pool().await;
    let e = pool
        .rows("SELECT 1 / 0")
        .await
        .expect_err("dividing by zero must not answer");
    assert!(
        e.to_string().starts_with("arithmetic_error:"),
        "the class has to be readable without parsing prose: {e}"
    );
}

/// A genuine server or protocol failure must NOT be laundered into the
/// arithmetic class. The typing is a narrowing, not a catch-all.
#[tokio::test(flavor = "multi_thread")]
async fn other_failures_keep_their_own_class() {
    let pool = pool().await;
    match pool.rows("SELECT * FROM \"no_such_table_9c\"").await {
        Err(PoolError::Db(_)) => {}
        other => panic!("a missing table is not an arithmetic error: {other:?}"),
    }
}
