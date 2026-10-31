//! `matches` against Postgres: a pattern the caller got wrong is
//! `invalid_param` with a clean sentence, never `internal` with the
//! database's words, and a pattern that could run to the statement timeout
//! (a backreference, or one past `ORDO_REGEX_MAX_LEN`) is refused before
//! the regex engine runs it.
//!
//! Each statement is the shape the planner emits for `x matches p`:
//! `(x ~ <guard(p)>)` ([`ordo_plan::regex_guard`]).
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::{Duration, Instant};

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

fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn matches_sql(subject: &str, pattern: &str) -> String {
    format!(
        "SELECT ({subject} ~ {})::text",
        ordo_plan::regex_guard(&lit(pattern))
    )
}

async fn refused(pool: &Pool, subject: &str, pattern: &str) -> PoolError {
    match pool.rows(&matches_sql(subject, pattern)).await {
        Err(e) => e,
        Ok(rows) => panic!("`{pattern}` ran: {rows:?}"),
    }
}

fn assert_clean_invalid_param(e: &PoolError, pattern: &str) {
    assert!(
        matches!(e, PoolError::InvalidInput(_)),
        "`{pattern}`: {e:?}"
    );
    let text = e.to_string();
    assert!(text.starts_with("invalid_param: "), "{text}");
    assert!(!text.contains("db:") && !text.contains("ERROR"), "{text}");
    assert!(text.contains("ORDO_REGEX_MAX_LEN"), "{text}");
    assert_eq!(e.class(), ordo_exec::FaultClass::Refused, "{e:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_valid_pattern_matches_as_before() {
    let pool = pool().await;
    let rows = pool.rows(&matches_sql("'abc'", "^a.c$")).await.unwrap();
    assert!(format!("{rows:?}").contains("true"), "{rows:?}");
    let rows = pool.rows(&matches_sql("'abc'", "^x")).await.unwrap();
    assert!(format!("{rows:?}").contains("false"), "{rows:?}");
    // A NULL pattern stays NULL: no refusal, no match.
    let rows = pool
        .rows(&format!(
            "SELECT ('abc' ~ {})::text",
            ordo_plan::regex_guard("NULL::text")
        ))
        .await
        .unwrap();
    assert!(!format!("{rows:?}").contains("true"), "{rows:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_pattern_is_invalid_param_not_internal() {
    let pool = pool().await;
    for p in [
        "a{1000000000}",
        ".*)|(select",
        "(((a{1,100}){1,100}){1,100}){1,100}$",
    ] {
        let e = refused(&pool, "'aaaa'", p).await;
        assert_clean_invalid_param(&e, p);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backreference_is_refused_before_it_runs() {
    let pool = pool().await;
    let long = "repeat('a', 100000)";
    for p in ["(\\1)", "^((a+)\\2)+$", "^(((a+)\\3)+)\\1$"] {
        let t = Instant::now();
        let e = refused(&pool, long, p).await;
        assert_clean_invalid_param(&e, p);
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "`{p}` took {:?}",
            t.elapsed()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pattern_past_the_cap_is_refused() {
    let pool = pool().await;
    let at_cap = "a".repeat(ordo_exec::regex_max_len());
    pool.rows(&matches_sql("'a'", &at_cap)).await.unwrap();
    let past = "a".repeat(ordo_exec::regex_max_len() + 1);
    let e = refused(&pool, "'a'", &past).await;
    assert_clean_invalid_param(&e, "past the cap");
}

#[tokio::test(flavor = "multi_thread")]
async fn other_data_exceptions_are_invalid_param_too() {
    let pool = pool().await;
    match pool.rows("SELECT 'not-a-uuid'::uuid").await {
        Err(e @ PoolError::InvalidInput(_)) => {
            let text = e.to_string();
            assert!(text.starts_with("invalid_param: "), "{text}");
            assert!(!text.contains("db:") && !text.contains("ERROR"), "{text}");
            assert!(text.contains("not-a-uuid"), "{text}");
        }
        other => panic!("expected invalid input, got {other:?}"),
    }
    // The arithmetic pair keeps its own ruled class.
    assert!(matches!(
        pool.rows("SELECT mod(1, 0)").await,
        Err(PoolError::Arithmetic(_))
    ));
}
