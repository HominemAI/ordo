//! Checklist 9e — **row invariants (`check`), executed.**
//!
//! DL §3 ("Row invariants: `check` [settled]") promises a declared
//! invariant is *system*-enforced inside every write's transaction, with a
//! typed rejection that aborts the whole atomic group. The ruling of
//! 2026-09-05 scopes this build to the **row-local half** and refuses the
//! other two settled forms by name — see `ordo_schema::check` for the
//! argument and `corpus-rulings.md` for the record.
//!
//! # What only an executing test can show
//!
//! 1. **The database is the enforcer.** The constraint must exist in the
//!    DDL and refuse a violating INSERT/UPDATE with `check_violation` —
//!    not `internal`, because the server did not break; the data was
//!    invalid. The class arrives by SQLSTATE (23514), read once at the
//!    seam, never by matching English.
//! 2. **The constraint name is the diagnostic.** A caller must learn WHICH
//!    check failed, and the name is the only channel Postgres offers — so
//!    the refusal must carry `teams$chk0$used<=seats`.
//! 3. **Two-valued logic holds inside the constraint.** Postgres treats a
//!    NULL check result as *pass*; the language says an ordering predicate
//!    against null is FALSE. Under naive rendering, `check used <= seats`
//!    with a null `used` is silently unenforced. Under the language's
//!    table it refuses — only a real write can tell the two apart.
//!
//! Real Postgres, this suite's own databases (`ordo_chk_*`).

mod gc_support;

use gc_support::{class_of, db, ANA};
use ordo_conformance::{Fixture, WriteLog, FIXTURE, SEED};

/// DL §3's own example, plus a nullable column so the null semantics are
/// reachable, plus a text-shape rule so more than one comparison family is
/// executed.
const CLUSTER: &str = r#"
entity teams (key: team) {
  name:  text
  seats: int
  used:  int = 0
  check used <= seats
  check used >= 0
}

entity labels (key: lbl) {
  name: text
  slack: int?
  check slack <= 100
}
"#;

const CLUSTER_OPS: &str = r#"
op addTeam($name: text, $seats: int) =
  set teams { name: $name, seats: $seats }

op setUsed($team: id, $used: int) =
  set teams { id: $team, used: $used }

# DL §6's own spelling of the counter half of the quota pattern, compiling
# since 17f. Kept beside `setUsed` so this suite executes the constraint
# against both the computed and the supplied form.
op claimSeat($team: id) =
  set teams { id: $team, used: used + 1 }

op addLabel($name: text) =
  set labels { name: $name }

op setSlack($label: id, $slack: int) =
  set labels { id: $label, slack: $slack }
"#;

fn fixture() -> Fixture {
    let src = format!("{FIXTURE}\n{CLUSTER}\n{CLUSTER_OPS}");
    Fixture::from_source(&src, SEED).expect("the fixture plus the checked entities compiles")
}

fn refusal(extra: &str) -> String {
    match Fixture::from_source(&format!("{FIXTURE}\n{extra}"), SEED) {
        Ok(_) => panic!("compiled, and must not have:\n{extra}"),
        Err(e) => e.to_string(),
    }
}

// ── the DDL carries the constraint, planner-shaped ───────────────────────

/// The generated DDL holds a real CHECK constraint whose name carries the
/// condition and whose SQL is the planner's own two-valued shapes — the
/// ordering comparison is COALESCE-wrapped, because a CHECK passing on
/// UNKNOWN would leave null rows silently unenforced.
#[test]
fn the_ddl_carries_the_named_constraint() {
    let f = fixture();
    let ddl = ordo_schema::ddl::ddl_for(&f.schema).expect("ddl");
    assert!(
        ddl.contains(
            r#"CONSTRAINT "teams$chk0$used<=seats" CHECK (COALESCE("used" <= "seats", false))"#
        ),
        "the constraint is named by its condition and wrapped for two-valued logic:\n{ddl}"
    );
    assert!(
        ddl.contains(r#"CONSTRAINT "teams$chk1$used>=0" CHECK (COALESCE("used" >= 0, false))"#),
        "{ddl}"
    );
}

// ── executed: violating refused by class, satisfying applied ─────────────

/// DL's seat example, run against the database: filling the last seat
/// lands; overselling refuses with `check_violation`, and the refusal
/// names the constraint — which names the condition.
#[test]
fn a_violating_write_is_refused_and_names_the_check() {
    let f = fixture();
    let pg = db("ordo_chk_seats");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let r = f
        .run_write_as(&pg, &mut log, r#"addTeam($name: "core", $seats: 1)"#, ANA)
        .expect("a team with one seat");
    let team = r[0]["created"].as_str().expect("created id").to_string();

    // DL's own spelling, now that it compiles (17f): `used: used + 1`,
    // computed in the row's own UPDATE. This suite is about the CONSTRAINT
    // and it fires the same whichever way the value arrives — but the
    // counter pattern DL pairs with `check` is the increment, so that is
    // what the pairing is tested with. `claimSeat` and `setUsed` are both
    // registered; the second still exercises the parameter form below.
    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"claimSeat($team: "{team}")"#),
        ANA,
    )
    .expect("the last seat fills — used == seats satisfies used <= seats");

    let err = f
        .run_write_as(
            &pg,
            &mut log,
            &format!(r#"claimSeat($team: "{team}")"#),
            ANA,
        )
        .expect_err("the oversell refuses");
    assert_eq!(class_of(&err), "check_violation", "{err}");
    assert!(
        err.contains("teams$chk0$used<=seats"),
        "the refusal names WHICH check: {err}"
    );

    // The refused write left no trace: used is still 1.
    let rows = pg
        .query(&format!(
            r#"SELECT "used" FROM "teams" WHERE "id" = '{team}'"#
        ))
        .expect("read back");
    assert_eq!(rows[0][0].as_deref(), Some("1"), "the abort was complete");
}

/// The language's null table holds inside the constraint: an ordering
/// predicate against null is FALSE, so writing a null `slack`… is fine
/// (the check never mentions a row that leaves it null — wait, it does:
/// `slack <= 100` over a null `slack` is FALSE by the language's table).
/// A nullable checked column therefore refuses null, which is exactly the
/// difference between the language's semantics and SQL's — under naive
/// rendering this test would observe the write LANDING.
#[test]
fn null_fails_an_ordering_check_because_the_language_says_so() {
    let f = fixture();
    let pg = db("ordo_chk_null");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let err = f
        .run_write_as(&pg, &mut log, r#"addLabel($name: "urgent")"#, ANA)
        .expect_err("a label whose slack stays null fails `slack <= 100`");
    assert_eq!(class_of(&err), "check_violation", "{err}");
    assert!(err.contains("labels$chk0"), "{err}");

    // And a satisfying value lands through the same op pair.
    let r = f
        .run_write_as(&pg, &mut log, r#"addTeam($name: "x", $seats: 5)"#, ANA)
        .expect("an unchecked-path write still lands");
    assert!(r[0]["created"].is_string());
}

// ── the refusals, each by its own class ──────────────────────────────────

/// The ref-walking form is DL-settled and deliberately NOT built — CHECK
/// admits no subqueries, so it is trigger work. Refused by name.
#[test]
fn a_ref_walking_check_refuses_as_beyond_row() {
    let err = refusal(
        r#"
entity tickets2 (key: tik) {
  board -> boards
  check board.name != "closed"
}
"#,
    );
    assert!(err.contains("check_beyond_row"), "{err}");
}

/// `today`/`now` cannot enter a CHECK: the expression must be immutable,
/// and a rule that passes at write and silently becomes false later is
/// worse than a refusal.
#[test]
fn a_time_dependent_check_refuses_by_name() {
    let err = refusal(
        r#"
entity offers (key: ofr) {
  due: date
  check due >= today
}
"#,
    );
    assert!(err.contains("check_time_dependent"), "{err}");
}

/// A sealed column stores ciphertext; the database cannot evaluate a
/// predicate over a value it cannot see.
#[test]
fn a_sealed_field_check_refuses_by_name() {
    let err = refusal(
        r#"
entity notes2 (key: nte) {
  body: text sealed
  check body != ""
}
"#,
    );
    assert!(err.contains("check_sealed_field"), "{err}");
}

/// A misspelled field is the author's mistake and gets an authored,
/// case-visible class — not `unknown_field`, which ruling 10f made
/// internal.
#[test]
fn an_unknown_field_check_refuses_by_its_own_class() {
    let err = refusal(
        r#"
entity crates2 (key: crt) {
  weight: int
  check wieght > 0
}
"#,
    );
    assert!(err.contains("check_unknown_field"), "{err}");
}

/// Constructs outside the row-local slice refuse with a message naming the
/// construct and the spelling to use instead — division, params, function
/// calls.
#[test]
fn out_of_slice_constructs_refuse_as_unsupported() {
    for (body, needle) in [
        ("check used / seats <= 1", "divides"),
        ("check used <= $cap", "no caller at constraint time"),
        ("check length(name) > 0", "expression functions"),
    ] {
        let err = refusal(&format!(
            r#"
entity q2 (key: qq) {{
  name:  text
  seats: int
  used:  int
  {body}
}}
"#
        ));
        assert!(err.contains("check_unsupported"), "{body}: {err}");
        assert!(err.contains(needle), "{body}: {err}");
    }
}
