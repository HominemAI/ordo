//! 27d, the constraint half: a foreign key or a check onto a table that
//! exists is added `NOT VALID` inside the transaction and validated as its
//! own step after it (`plan-evolution-plans.md` §3.7, board
//! `apply-lock-timeout`). A brand-new table keeps its constraints inline.
//!
//! Before this, a constraint over existing rows was refused
//! (`constraint_over_existing_rows`) with the two statements printed for a
//! person to run by hand. Now the apply runs them: the add takes its
//! `ACCESS EXCLUSIVE` lock for a moment and scans nothing, and the validate
//! scans the rows under a lock that lets writes continue.
//!
//! When validation fails, the constraint is dropped again, so the table
//! holds what the transaction committed and nothing of the constraint, and
//! the next apply plans it afresh. The failure names the constraint and
//! the table.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::{Duration, Instant};

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::{self as sa, Applied, ExecFailure, LockPolicy, Phase, Program, Step};

/// Two entities and one op: enough to apply, and rows to put in `a`.
const V1: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
}

op listA unbounded =
  a[name, n]
"#;

/// V1 plus an optional ref onto `b` (A2 with its foreign key) and a row
/// invariant on `a` (a check), both onto a table that holds rows.
const V2: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
  owner -> b?
  check n >= 0
}

op listA unbounded =
  a[name, n]
"#;

/// V1 plus a new entity with a foreign key and a check of its own.
const V3: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
}

entity c {
  owner -> a
  qty:   int
  check qty >= 0
}

op listA unbounded =
  a[name, n]
"#;

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.into(),
        size: 3,
        ..PoolConfig::from_env()
    }
}

async fn empty_db(name: &str) -> (Pg, Pool) {
    let pg = Pg::scoped(name);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(cfg(&pg.db)).await.expect("pool boots");
    (pg, pool)
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

fn validated(pg: &Pg, constraint: &str) -> String {
    one(
        pg,
        &format!("SELECT convalidated FROM pg_constraint WHERE conname = '{constraint}'"),
    )
}

/// A `widgets` table with rows, for the hand-built steps.
async fn widgets_db(name: &str, ns: &[i64]) -> (Pg, Pool) {
    let (pg, pool) = empty_db(name).await;
    pg.exec(
        "CREATE TABLE \"widgets\" (\"$space\" text NOT NULL, \"id\" text NOT NULL, \
         \"rev\" bigint NOT NULL, \"n\" bigint NOT NULL, PRIMARY KEY (\"$space\", \"id\"))",
    )
    .expect("a table");
    for (i, n) in ns.iter().enumerate() {
        pg.exec(&format!(
            "INSERT INTO \"widgets\" VALUES ('s1', 'w_{i}', 1, {n})"
        ))
        .expect("a row");
    }
    (pg, pool)
}

fn add_column() -> Step {
    Step {
        code: "A2",
        phase: Phase::Alter,
        sql: "ALTER TABLE \"widgets\" ADD COLUMN \"note\" text;\n".into(),
        index: None,
        constraint: None,
        table: "widgets".into(),
    }
}

fn add_check_not_valid() -> Step {
    Step {
        code: "C9",
        phase: Phase::Alter,
        sql: "ALTER TABLE \"widgets\" ADD CONSTRAINT \"widgets$n$chk\" CHECK (\"n\" >= 0) NOT VALID;\n"
            .into(),
        index: None,
        constraint: None,
        table: "widgets".into(),
    }
}

fn validate_check() -> Step {
    Step {
        code: "C9",
        phase: Phase::Validate,
        sql: "ALTER TABLE \"widgets\" VALIDATE CONSTRAINT \"widgets$n$chk\";\n".into(),
        index: None,
        constraint: Some("widgets$n$chk".into()),
        table: "widgets".into(),
    }
}

fn quick() -> LockPolicy {
    LockPolicy {
        lock_timeout: Duration::from_millis(100),
        retries: 2,
        backoff: Duration::from_millis(50),
    }
}

/// The whole path through `apply`: a foreign key and a check onto a table
/// with rows are two steps each, the add `NOT VALID` in the transaction
/// and the validate after it, and both end validated and enforced.
#[tokio::test(flavor = "multi_thread")]
async fn a_constraint_onto_a_populated_table_is_added_not_valid_then_validated() {
    let (pg, pool) = empty_db("ordo_apply_nv_populated").await;
    let v1 = Program::compile(V1).expect("v1 compiles");
    let v2 = Program::compile(V2).expect("v2 compiles");
    let (_, created) = sa::apply(&pool, &v1, "test", "test").await;
    assert!(matches!(created, Ok(Applied::Created(_))), "{created:?}");
    pg.exec(
        "INSERT INTO b (\"$space\", id, rev, title) VALUES ('s1','b_1',1,'t'); \
         INSERT INTO a (\"$space\", id, rev, n) VALUES ('s1','a_1',1,1), ('s1','a_2',1,2);",
    )
    .expect("rows");

    let (plan, outcome) = sa::apply(&pool, &v2, "test", "test").await;
    let plan = plan.expect("the plan reads");
    let Ok(Applied::Additive {
        steps, executed, ..
    }) = outcome
    else {
        panic!(
            "a foreign key and a check onto rows apply, refusals {:?}: {outcome:?}",
            plan.refusals
        );
    };

    let not_valid: Vec<&Step> = steps
        .iter()
        .filter(|s| s.phase == Phase::Alter && s.sql.contains("ADD CONSTRAINT"))
        .collect();
    assert_eq!(not_valid.len(), 2, "one add per constraint: {steps:#?}");
    for s in &not_valid {
        assert!(
            s.sql.trim_end().ends_with("NOT VALID;"),
            "the add skips the scan under the lock: {}",
            s.sql
        );
    }
    let validates: Vec<&Step> = steps
        .iter()
        .filter(|s| s.phase == Phase::Validate)
        .collect();
    assert_eq!(
        validates.len(),
        2,
        "and one validate per constraint: {steps:#?}"
    );
    let names: Vec<&str> = validates
        .iter()
        .map(|s| {
            s.constraint
                .as_deref()
                .expect("a validate names its constraint")
        })
        .collect();
    assert!(
        names.contains(&"a$owner$fk"),
        "the foreign key is one of them: {names:?}"
    );
    for s in &validates {
        assert!(
            s.sql.contains("VALIDATE CONSTRAINT"),
            "a validate step validates: {}",
            s.sql
        );
        assert!(
            !s.sql.contains("ADD"),
            "and adds nothing of its own: {}",
            s.sql
        );
    }
    // Phases in order: the adds before every validate.
    let last_alter = steps.iter().rposition(|s| s.phase == Phase::Alter).unwrap();
    let first_validate = steps
        .iter()
        .position(|s| s.phase == Phase::Validate)
        .unwrap();
    assert!(last_alter < first_validate, "the transaction commits first");

    assert_eq!(executed.validated, 2, "{executed:?}");
    for name in &names {
        assert_eq!(validated(&pg, name), "t", "`{name}` ended validated");
    }
    assert!(
        pg.exec("INSERT INTO a (\"$space\", id, rev, n) VALUES ('s1','a_9',1,-1)")
            .is_err(),
        "the check is enforced"
    );
    assert!(
        pg.exec("INSERT INTO a (\"$space\", id, rev, n, owner) VALUES ('s1','a_8',1,1,'b_nope')")
            .is_err(),
        "and so is the foreign key"
    );
    assert_eq!(one(&pg, "SELECT count(*) FROM a"), "2", "the rows survived");
    assert_eq!(
        one(
            &pg,
            "SELECT phase FROM \"$migrations\" ORDER BY seq DESC LIMIT 1"
        ),
        "applied"
    );
}

/// A brand-new table has nothing to scan and nobody to lock out: its
/// foreign key and check ride inline, in the transaction, with no
/// validate step.
#[tokio::test(flavor = "multi_thread")]
async fn a_brand_new_table_keeps_its_constraints_inline() {
    let (pg, pool) = empty_db("ordo_apply_nv_new_table").await;
    let v1 = Program::compile(V1).expect("v1 compiles");
    let v3 = Program::compile(V3).expect("v3 compiles");
    let (_, created) = sa::apply(&pool, &v1, "test", "test").await;
    assert!(matches!(created, Ok(Applied::Created(_))), "{created:?}");
    pg.exec("INSERT INTO a (\"$space\", id, rev, n) VALUES ('s1','a_1',1,1)")
        .expect("rows in the table it references");

    let (_, outcome) = sa::apply(&pool, &v3, "test", "test").await;
    let Ok(Applied::Additive {
        steps, executed, ..
    }) = outcome
    else {
        panic!("a new entity applies: {outcome:?}");
    };
    assert!(
        steps.iter().all(|s| s.phase != Phase::Validate),
        "no validate step for a new table: {steps:#?}"
    );
    assert!(
        steps.iter().all(|s| !s.sql.contains("NOT VALID")),
        "and nothing added NOT VALID: {steps:#?}"
    );
    assert_eq!(executed.validated, 0);
    assert_eq!(
        validated(&pg, "c$owner$fk"),
        "t",
        "the foreign key is live and valid"
    );
    assert!(
        pg.exec("INSERT INTO c (\"$space\", id, rev, owner, qty) VALUES ('s1','c_1',1,'a_1',-1)")
            .is_err(),
        "the check is enforced"
    );
}

/// A row that breaks the constraint was written between the probe and the
/// add. The transaction has committed; the validate fails naming the
/// constraint and the table; the constraint is dropped again so the next
/// apply plans it afresh; the other changes stay.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_that_breaks_the_constraint_fails_validation_and_the_constraint_is_dropped_again() {
    let (pg, pool) = widgets_db("ordo_apply_nv_violation", &[1, -5, 3]).await;
    let steps = [add_column(), add_check_not_valid(), validate_check()];
    let err = sa::execute_with(&pool, &steps, &quick())
        .await
        .expect_err("a row breaks the check");

    match &err {
        ExecFailure::Validate {
            table,
            constraint,
            lock_attempts,
            transactional,
            validated_before,
            dropped,
            error,
            ..
        } => {
            assert_eq!(&**table, "widgets");
            assert_eq!(&**constraint, "widgets$n$chk");
            assert_eq!(*lock_attempts, None, "the rows failed it, not a lock");
            assert_eq!(*transactional, 2, "the transaction committed first");
            assert_eq!(*validated_before, 0);
            assert_eq!(*dropped, Ok(()), "and the constraint was dropped again");
            assert!(
                error.contains("widgets$n$chk"),
                "the database's own error names it: {error}"
            );
        }
        other => panic!("a violating row is a Validate failure, not {other:?}"),
    }
    let text = err.to_string();
    assert!(
        text.contains("`widgets$n$chk`") && text.contains("`widgets`"),
        "the message names the constraint and the table: {text}"
    );
    assert!(
        text.contains("failed validation") && text.contains("dropped again"),
        "and says what happened and what state it left: {text}"
    );

    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_constraint WHERE conname = 'widgets$n$chk'"
        ),
        "0",
        "nothing of the constraint remains"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_name = 'widgets' AND column_name = 'note'"
        ),
        "1",
        "the transaction's other change is live"
    );
    assert_eq!(
        one(&pg, "SELECT count(*) FROM widgets"),
        "3",
        "the rows are untouched"
    );

    // The rows fixed, the same two steps go through.
    pg.exec("UPDATE widgets SET n = 5 WHERE n < 0")
        .expect("fix the row");
    let done = sa::execute_with(&pool, &[add_check_not_valid(), validate_check()], &quick())
        .await
        .expect("the rows pass now");
    assert_eq!(done.validated, 1);
    assert_eq!(validated(&pg, "widgets$n$chk"), "t");
}

/// The validate runs under `lock_timeout` like every other statement, and
/// is retried with backoff. When the lock never comes, the apply fails
/// naming the table and the constraint. The drop that would clean up
/// needs the same lock, so it fails too, and the message says the
/// constraint is in place `NOT VALID` and how to remove it.
#[tokio::test(flavor = "multi_thread")]
async fn a_validate_waits_out_its_lock_and_is_retried_like_every_other_step() {
    let (pg, pool) = widgets_db("ordo_apply_nv_lock", &[1, 2]).await;
    pg.exec(
        "ALTER TABLE \"widgets\" ADD CONSTRAINT \"widgets$n$chk\" CHECK (\"n\" >= 0) NOT VALID",
    )
    .expect("the add has landed");
    let holder = pool.begin().await.expect("the holder's transaction");
    holder
        .rows("LOCK TABLE \"widgets\" IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("the holder takes the lock");

    let started = Instant::now();
    let err = sa::execute_with(&pool, &[validate_check()], &quick())
        .await
        .expect_err("another session holds the lock");
    let elapsed = started.elapsed();
    match &err {
        ExecFailure::Validate {
            table,
            constraint,
            lock_attempts,
            dropped,
            ..
        } => {
            assert_eq!(&**table, "widgets");
            assert_eq!(&**constraint, "widgets$n$chk");
            assert_eq!(*lock_attempts, Some(3), "one attempt plus two retries");
            assert!(
                dropped.is_err(),
                "the drop needs the same lock and did not get it either: {dropped:?}"
            );
        }
        other => panic!("a held lock on the validate is a Validate failure, not {other:?}"),
    }
    let text = err.to_string();
    assert!(
        text.contains("could not lock table `widgets`") && text.contains("3 attempts"),
        "{text}"
    );
    assert!(
        text.contains("NOT VALID") && text.contains("DROP CONSTRAINT \"widgets$n$chk\""),
        "it says the constraint stands NOT VALID and how to remove it: {text}"
    );
    // Three waits of 100 ms and pauses of 50 ms then 100 ms, a lower bound.
    assert!(
        elapsed >= Duration::from_millis(450),
        "three attempts and two backoffs take at least 450 ms, took {elapsed:?}"
    );
    assert_eq!(
        validated(&pg, "widgets$n$chk"),
        "f",
        "still there, still NOT VALID"
    );

    // Lock released: the validate goes through and the constraint is valid.
    holder.rollback().await.expect("release");
    let done = sa::execute_with(&pool, &[validate_check()], &quick())
        .await
        .expect("the table is free");
    assert_eq!(done.validated, 1);
    assert_eq!(done.retries, 0);
    assert_eq!(validated(&pg, "widgets$n$chk"), "t");
}

/// A lock that goes away during the backoff lets the validate through.
#[tokio::test(flavor = "multi_thread")]
async fn a_lock_released_during_the_retries_lets_the_validate_through() {
    let (pg, pool) = widgets_db("ordo_apply_nv_release", &[1]).await;
    let holder = pool.begin().await.expect("the holder's transaction");
    holder
        .rows("LOCK TABLE \"widgets\" IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("the holder takes the lock");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(350)).await;
        let _ = holder.rollback().await;
    });
    let patient = LockPolicy {
        lock_timeout: Duration::from_millis(100),
        retries: 20,
        backoff: Duration::from_millis(50),
    };
    let done = sa::execute_with(&pool, &[add_check_not_valid(), validate_check()], &patient)
        .await
        .expect("the lock was released during the retries");
    assert_eq!(done.transactional, 1);
    assert_eq!(done.validated, 1);
    assert!(
        done.retries >= 1,
        "at least one attempt waited the lock out: {done:?}"
    );
    assert_eq!(validated(&pg, "widgets$n$chk"), "t");
}

/// CI's toolchain refuses an `Err` of 128 bytes or more (`result_large_err`),
/// and an older local toolchain did not say so: the failure reached CI only.
/// Pin the size here so a new field shows up in `cargo test` on any machine.
#[test]
fn exec_failure_stays_under_clippys_large_error_threshold() {
    assert!(
        std::mem::size_of::<ExecFailure>() < 128,
        "ExecFailure is {} bytes; box a field",
        std::mem::size_of::<ExecFailure>()
    );
}
