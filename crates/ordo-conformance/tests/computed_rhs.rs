//! Checklist 17f — **the computed write right-hand side**, executed.
//!
//! DL §6 spells the flagship case itself: *"**RHS field references**:
//! `{ n: n + 1 }` — computed server-side inside the row's transaction;
//! atomic increments without read-modify-write races."* DL §3 puts the whole
//! expression language behind it: *"Legal anywhere expressions are:
//! projections, **write RHS (per-row values in bulk writes and rules)**,
//! aggregate arguments, sort keys."* Until this landed, a write's right-hand
//! side was literals, parameters and `$caller`, and `set teams { id: $t,
//! used: used + 1 }` — the language's own quota pattern — did not compile.
//!
//! # What only an executing test can show
//!
//! 1. **The expression is the read's expression.** `+` over text is
//!    concatenation and over money is addition; `/` evaluates in `numeric`
//!    at a fixed scale rather than truncating two `bigint`s. Those are
//!    `ordo-plan`'s rendering rules, and the only way to show a write got
//!    them is to write a row and read the number back.
//! 2. **The increment reads the row's CURRENT value.** A test that supplied
//!    the new value as a parameter would pass against a broken
//!    implementation; only `used + 1` against a stored `used` distinguishes
//!    them.
//! 3. **Division by zero is `arithmetic_error`, inside a write.** The class
//!    arrives by SQLSTATE at the pool seam, and a write is a different code
//!    path from the read that first proved it.
//!
//! The concurrency half — two racing increments landing as `+2` — needs a
//! real transaction per caller and therefore the pooled executor: it lives
//! in `ordo-exec/tests/computed_rhs.rs`.
//!
//! Real Postgres, this suite's own databases (`ordo_rhs_*`).

mod gc_support;

use gc_support::{class_of, db, ANA};
use ordo_conformance::{Fixture, WriteLog, FIXTURE, SEED};

/// DL §3's own `check` example — the seat counter — plus the field families
/// a computed value has to reach: money, text, a closed value set, a unique
/// key, a ref, and a sealed column.
const CLUSTER: &str = r#"
entity teams (key: team) {
  name:  text
  seats: int
  used:  int = 0
  check used <= seats
  check used >= 0
}

entity meters (key: mtr) {
  label:  text
  budget: money
  spent:  money = 0
  heads:  int = 1
  slug:   text unique
}

entity plants (key: plt) {
  name: text
  note: text?
}

entity counters (key: ctr) {
  name: text unique
  n:    int = 0
}

entity wallets (key: wal) {
  name:    text unique
  balance: money(2) = 1.50
}
"#;

const OPS: &str = r#"
op addTeam($name: text, $seats: int) =
  set teams { name: $name, seats: $seats }

op claimSeat($team: id) =
  set teams { id: $team, used: used + 1 }

op releaseSeats($n: int) =
  set teams(used > 0) { used: used - $n }

op addMeter($label: text, $budget: money, $slug: text) =
  set meters { label: $label, budget: $budget, slug: $slug }

op spend($m: id, $amount: money) =
  set meters { id: $m, spent: spent + $amount }

op perHead($m: id) =
  set meters { id: $m, spent: round(budget / heads, 0) }

op setHeads($m: id, $n: int) =
  set meters { id: $m, heads: $n }

op addPlant($name: text) =
  set plants { name: $name }

op tagPlant($plant: id, $kind: text) =
  set plants { id: $plant, note: $kind + ": " + name }

op namePlant($plant: id, $prefix: text) =
  set plants(id == $plant) { name: $prefix + name }

op rankPlant($plant: id, $short: text, $long: text) =
  set plants { id: $plant, note: cond(length(name) < 4 -> $short, $long) }

# Converge-or-create on a unique key, reading the row's own field: the
# create branch reads `n` as its default (owner, 2026-10-31).
op bump($k: text) =
  set counters { name: $k, n: n + 1 }

op credit($k: text, $amount: money) =
  set wallets { name: $k, balance: balance + $amount }

op stampPlant($prefix: text) =
  set plants { name: $prefix + "x" }

# Registers, and refuses at plan time: whether a value's TYPE reaches the
# column is a question about the bound expression, and the write path has
# no bound parameters until the call arrives.
op badTy($t: id) =
  set teams(id == $t) { used: name + "1" }

# Registers (the structural gate sees a `round`, which is a conversion) and
# refuses at plan time, where the RESULT type is known: `round(x, 2)` is
# still a decimal.
op badScale($m: id) =
  set meters { id: $m, spent: round(budget / heads, 2) }
"#;

fn fixture() -> Fixture {
    let src = format!("{FIXTURE}\n{CLUSTER}\n{OPS}");
    Fixture::from_source(&src, SEED).expect("the computed-RHS cluster compiles")
}

/// Compile one extra op against the fixture and hand back its refusal.
fn refusal(extra: &str) -> String {
    match Fixture::from_source(&format!("{FIXTURE}\n{CLUSTER}\n{OPS}\n{extra}"), SEED) {
        Ok(_) => panic!("compiled, and must not have:\n{extra}"),
        Err(e) => e.to_string(),
    }
}

/// A registration refusal reads `register: [class] message`, so the runtime
/// `class_of` (split on the first colon) would answer `register` for every
/// one of them. The class is the thing in brackets.
fn reg_class(e: &str) -> &str {
    e.split('[')
        .nth(1)
        .and_then(|r| r.split(']').next())
        .unwrap_or(e)
}

fn col(pg: &ordo_conformance::Pg, entity: &str, id: &str, column: &str) -> String {
    pg.query(&format!(
        r#"SELECT "{column}" FROM "{entity}" WHERE "id" = '{id}'"#
    ))
    .expect("read back")[0][0]
        .clone()
        .unwrap_or_else(|| "NULL".into())
}

// ── the flagship: DL's own increment ─────────────────────────────────────

/// **`set teams { id: $t, used: used + 1 }`, verbatim**, under the very
/// `check used <= seats` DL pairs it with.
///
/// Three seats, three claims, and the fourth refuses — with the constraint
/// name that says which invariant, because the counter pattern is only a
/// quota if the cap is enforced by something other than the caller's
/// arithmetic. The refused claim leaves `used` where it was: the abort is
/// the whole statement, not a partial increment.
#[test]
fn dls_own_increment_counts_and_the_check_caps_it() {
    let f = fixture();
    let pg = db("ordo_rhs_seats");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let r = f
        .run_write_as(&pg, &mut log, r#"addTeam($name: "core", $seats: 3)"#, ANA)
        .expect("a team with three seats");
    let team = r[0]["created"].as_str().expect("created id").to_string();
    assert_eq!(col(&pg, "teams", &team, "used"), "0", "the default applies");

    for want in 1..=3 {
        f.run_write_as(
            &pg,
            &mut log,
            &format!(r#"claimSeat($team: "{team}")"#),
            ANA,
        )
        .expect("a seat is free");
        assert_eq!(
            col(&pg, "teams", &team, "used"),
            want.to_string(),
            "the increment read the CURRENT value, not a parameter"
        );
    }

    let err = f
        .run_write_as(
            &pg,
            &mut log,
            &format!(r#"claimSeat($team: "{team}")"#),
            ANA,
        )
        .expect_err("the fourth claim oversells");
    assert_eq!(class_of(&err), "check_violation", "{err}");
    assert!(
        err.contains("teams$chk0$used<=seats"),
        "the refusal names WHICH check: {err}"
    );
    assert_eq!(
        col(&pg, "teams", &team, "used"),
        "3",
        "the refused claim incremented nothing"
    );
}

/// **The one-seam proof.** `used + 1` in a write and `used + 1` in a read
/// are rendered by the same function, and this asserts the output rather
/// than the intention.
///
/// The write's fragment names the row's own column, qualified by the table
/// (`"teams"."used" + 1`) — which is what makes it the row's PRE-write value
/// in an `UPDATE "teams" SET …`, whatever order the assignments are in. The
/// read's plan differs in exactly one character sequence, the alias, because
/// a read gives its source one and a write's source is the table it is
/// updating. Strip that and the two are the same string.
///
/// A second renderer for writes would pass every behavioural test in this
/// file on the day it was written. This is the test it would fail.
#[test]
fn a_write_expression_renders_exactly_as_the_same_read_expression() {
    use ordo_syntax::{OpBody, ScalarTy, WriteStmt};

    let f = fixture();

    // The write's `used + 1`, taken from the registered op rather than
    // re-typed, so this cannot drift from what `claimSeat` actually says.
    let op = f.registry.get("claimSeat").expect("registered");
    let OpBody::Write(stmts) = &op.body else {
        panic!("claimSeat is a write")
    };
    let WriteStmt::Set { fields, .. } = &stmts[0] else {
        panic!("one set")
    };
    let (_, value) = fields
        .iter()
        .find(|(n, _)| n.name == "used")
        .expect("`used:` is the computed field");

    let bound = ordo_sema::write_value_expr(
        &f.schema,
        "teams",
        "teams",
        value,
        &[],
        Some("mem_1"),
        ScalarTy::Int,
    )
    .expect("binds");
    assert!(bound.reads_row, "`used + 1` reads the row");
    let write_sql = ordo_plan::render_write_value(&bound.expr, &f.plan_space()).expect("renders");
    assert_eq!(write_sql, r#"("teams"."used" + 1)"#);

    // The same expression, in a read.
    let read = ordo_plan::plan(
        &f.bind_case("teams[bump: used + 1]")
            .expect("binds as a read"),
        &f.plan_space(),
    )
    .expect("plans");
    let tail = write_sql
        .split_once("\"teams\".")
        .expect("the write qualifies by the table")
        .1;
    assert!(
        read.sql.contains(tail),
        "the read renders the same expression: wanted `…{tail}` in\n{}",
        read.sql
    );
}

/// A computed converge is a converge: `rev` still bumps, the command is a
/// `Converge`, and the after image carries the value the database computed.
#[test]
fn a_computed_converge_is_an_ordinary_converge() {
    let f = fixture();
    let pg = db("ordo_rhs_sql");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let r = f
        .run_write_as(&pg, &mut log, r#"addTeam($name: "sql", $seats: 9)"#, ANA)
        .expect("a team");
    let team = r[0]["created"].as_str().unwrap().to_string();

    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"claimSeat($team: "{team}")"#),
        ANA,
    )
    .expect("claimed");
    assert_eq!(col(&pg, "teams", &team, "used"), "1");
    assert_eq!(
        col(&pg, "teams", &team, "rev"),
        "2",
        "a computed converge is a converge: rev still bumps"
    );

    // The images are the audit spine, and they must record what LANDED, not
    // what was asked for — a revert reads them and puts the old value back.
    let group = log.last().expect("one command group");
    let cmd = &group.commands[0];
    assert_eq!(
        cmd.before
            .as_ref()
            .expect("a converge images both")
            .get("used"),
        Some(&ordo_commandlog::Value::Int(0)),
        "the before image is the pre-increment value"
    );
    assert_eq!(
        cmd.after
            .as_ref()
            .expect("a converge images both")
            .get("used"),
        Some(&ordo_commandlog::Value::Int(1)),
        "the after image is the value the DATABASE computed, read back post-batch"
    );
}

/// A **bulk** computed write. DL §3 names exactly this position — *"per-row
/// values in bulk writes"* — and the point is that the value is per-row:
/// two teams at different counts both come down by the same amount from
/// wherever they were, not to a shared number.
#[test]
fn a_bulk_write_computes_per_row() {
    let f = fixture();
    let pg = db("ordo_rhs_bulk");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let a = f
        .run_write_as(&pg, &mut log, r#"addTeam($name: "a", $seats: 9)"#, ANA)
        .expect("a")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();
    let b = f
        .run_write_as(&pg, &mut log, r#"addTeam($name: "b", $seats: 9)"#, ANA)
        .expect("b")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..5 {
        f.run_write_as(&pg, &mut log, &format!(r#"claimSeat($team: "{a}")"#), ANA)
            .expect("a claims");
    }
    for _ in 0..2 {
        f.run_write_as(&pg, &mut log, &format!(r#"claimSeat($team: "{b}")"#), ANA)
            .expect("b claims");
    }

    let out = f
        .run_write_as(&pg, &mut log, r#"releaseSeats($n: 2)"#, ANA)
        .expect("both teams have used > 0");
    assert_eq!(out[0]["updated"], 2);
    assert_eq!(col(&pg, "teams", &a, "used"), "3");
    assert_eq!(col(&pg, "teams", &b, "used"), "0");
}

// ── the expression language, in a write ──────────────────────────────────

/// Money arithmetic. `money` is an integer count of minor units (DL §2), so
/// `spent + $amount` is `bigint + bigint` and the total is exact — no float
/// exists anywhere on this path.
#[test]
fn money_adds_in_minor_units() {
    let f = fixture();
    let pg = db("ordo_rhs_money");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let m = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addMeter($label: "ops", $budget: 100000, $slug: "ops")"#,
            ANA,
        )
        .expect("a meter")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();

    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"spend($m: "{m}", $amount: 2999)"#),
        ANA,
    )
    .expect("spend");
    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"spend($m: "{m}", $amount: 1)"#),
        ANA,
    )
    .expect("spend again");
    assert_eq!(
        col(&pg, "meters", &m, "spent"),
        "3000",
        "29.99 + 0.01 is 30.00, to the minor unit"
    );
}

/// `+` over text is **concatenation** — the same operator, disambiguated by
/// the type the binder inferred, exactly as in a read. Both operands here
/// come from the row and from a parameter, so the value is per-row.
#[test]
fn plus_over_text_concatenates() {
    let f = fixture();
    let pg = db("ordo_rhs_text");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let p = f
        .run_write_as(&pg, &mut log, r#"addPlant($name: "fern")"#, ANA)
        .expect("a plant")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();

    // Row-reading: `name` is the stored value.
    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"tagPlant($plant: "{p}", $kind: "thirsty")"#),
        ANA,
    )
    .expect("tagged");
    assert_eq!(col(&pg, "plants", &p, "note"), "thirsty: fern");

    // And the same operator through a ranged write, prefixing in place.
    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"namePlant($plant: "{p}", $prefix: "the ")"#),
        ANA,
    )
    .expect("renamed");
    assert_eq!(col(&pg, "plants", &p, "name"), "the fern");
}

/// A **row-free** computed value — no field reference — is legal in a
/// CREATE, because it has an answer before the row exists. It is obtained
/// from the database and then behaves in every way like a value the author
/// wrote out: it lands in the row and it lands in the command log's after
/// image, which is what a revert would put back.
#[test]
fn a_row_free_expression_creates_and_images_honestly() {
    let f = fixture();
    let pg = db("ordo_rhs_create");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let p = f
        .run_write_as(&pg, &mut log, r#"stampPlant($prefix: "seedling-")"#, ANA)
        .expect("a create whose name is computed")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(col(&pg, "plants", &p, "name"), "seedling-x");

    let group = log.last().expect("one command group");
    let after = group.commands[0]
        .after
        .as_ref()
        .expect("a create images the whole row");
    assert_eq!(
        after.get("name"),
        Some(&ordo_commandlog::Value::Text("seedling-x".into())),
        "the after image records the value the database computed, not the expression"
    );
}

/// `cond` in a write right-hand side — DL names the position explicitly
/// (*"Legal anywhere expressions are: projections, write RHS …"*) — with an
/// arm condition that reads the row.
#[test]
fn cond_chooses_by_the_rows_own_value() {
    let f = fixture();
    let pg = db("ordo_rhs_cond");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let short = f
        .run_write_as(&pg, &mut log, r#"addPlant($name: "oak")"#, ANA)
        .expect("oak")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();
    let long = f
        .run_write_as(&pg, &mut log, r#"addPlant($name: "monstera")"#, ANA)
        .expect("monstera")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();

    for id in [&short, &long] {
        f.run_write_as(
            &pg,
            &mut log,
            &format!(r#"rankPlant($plant: "{id}", $short: "brief", $long: "wordy")"#),
            ANA,
        )
        .expect("ranked");
    }
    assert_eq!(col(&pg, "plants", &short, "note"), "brief");
    assert_eq!(col(&pg, "plants", &long, "note"), "wordy");
}

/// **`/ 0` inside a write is `arithmetic_error`**, not a 500 and not a null.
///
/// DL §2: *"`/ 0` is a typed runtime error, uniform with `mod(a, 0)`.
/// Division is total on its domain and loud off it; there is no
/// null-on-divide-by-zero folklore."* The class arrives by SQLSTATE 22012 at
/// the pool seam; on the harness's `psql` path it arrives as the server's
/// own words, and either way the write applies nothing.
///
/// The op divides `budget` by `heads`, so the zero comes from the ROW —
/// which is the case a compile-time check cannot catch and the reason the
/// runtime class has to exist.
#[test]
fn dividing_by_a_zero_in_the_row_is_a_typed_arithmetic_error() {
    let f = fixture();
    let pg = db("ordo_rhs_divzero");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let m = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addMeter($label: "split", $budget: 9500, $slug: "split")"#,
            ANA,
        )
        .expect("a meter")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();

    // Two heads: the division has an answer, and `round(x, 0)` is the
    // explicit conversion DL §2 demands before a decimal reaches a money
    // slot.
    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"setHeads($m: "{m}", $n: 2)"#),
        ANA,
    )
    .expect("two heads");
    f.run_write_as(&pg, &mut log, &format!(r#"perHead($m: "{m}")"#), ANA)
        .expect("9500 / 2 = 4750");
    assert_eq!(col(&pg, "meters", &m, "spent"), "4750");

    // Zero heads: no answer, loudly.
    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"setHeads($m: "{m}", $n: 0)"#),
        ANA,
    )
    .expect("zero heads is a legal count");
    let err = f
        .run_write_as(&pg, &mut log, &format!(r#"perHead($m: "{m}")"#), ANA)
        .expect_err("dividing by zero has no answer");
    assert!(
        err.to_lowercase().contains("division by zero"),
        "the server's own words survive: {err}"
    );
    assert_eq!(
        col(&pg, "meters", &m, "spent"),
        "4750",
        "the failed write changed nothing"
    );
}

// ── the refusals, each by its own class and its own reason ───────────────

/// **A create has no current row.** DL's increment is spelled *"inside the
/// row's transaction"*, and a statement whose mode can create has a branch
/// on which there is no such row. Refused at REGISTRATION, so the op never
/// deploys — the alternative is discovering it on the first call that
/// happens to take the create branch.
#[test]
fn a_field_reference_in_a_creating_statement_is_refused_by_name() {
    // Plain create.
    let e = refusal("op badCreate($n: text) =\n  set plants { name: $n + name }");
    assert_eq!(reg_class(&e), "invalid_binding_expr", "{e}");
    assert!(e.contains("creates"), "the mode label is the reason: {e}");
    assert!(
        e.contains("no current row"),
        "the message says why, not merely that: {e}"
    );

    // Create-once (`rev: null`) on a key only ever creates, so it has no
    // row to read either.
    let e = refusal("op badOnce($k: text) =\n  set counters { name: $k, rev: null, n: n + 1 }");
    assert_eq!(reg_class(&e), "invalid_binding_expr", "{e}");
    assert!(e.contains("no current row"), "{e}");
}

// ── converge-or-create reads the default when it creates (2026-10-31) ────

/// **`set counters { name: $k, n: n + 1 }`**, SQL's `INSERT … ON CONFLICT
/// DO UPDATE` counter. The first call creates the row and reads `n` as its
/// default `0`, so it stores `1`; each later call converges and adds one to
/// the stored value. One row, whatever the number of calls.
#[test]
fn a_converging_counter_creates_at_one_and_then_counts() {
    let f = fixture();
    let pg = db("ordo_rhs_upsert_counter");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let r = f
        .run_write_as(&pg, &mut log, r#"bump($k: "hits")"#, ANA)
        .expect("the first bump creates");
    let id = r[0]["created"].as_str().expect("created id").to_string();
    assert_eq!(
        col(&pg, "counters", &id, "n"),
        "1",
        "created as default + 1"
    );

    for want in 2..=3 {
        f.run_write_as(&pg, &mut log, r#"bump($k: "hits")"#, ANA)
            .expect("a later bump converges");
        assert_eq!(col(&pg, "counters", &id, "n"), want.to_string());
    }
    let rows = pg
        .query(r#"SELECT count(*) FROM "counters""#)
        .expect("count")[0][0]
        .clone();
    assert_eq!(rows.as_deref(), Some("1"), "converged, never a second row");

    // Another key is another row, starting again at one.
    let r = f
        .run_write_as(&pg, &mut log, r#"bump($k: "misses")"#, ANA)
        .expect("a new key creates");
    let other = r[0]["created"].as_str().expect("created id").to_string();
    assert_eq!(col(&pg, "counters", &other, "n"), "1");
}

/// The create branch's default is read in the units the column stores:
/// `balance: money(2) = 1.50` is `150` minor units, so a first credit of `1.00`
/// creates at `250` and the converge adds `100` more minor units.
#[test]
fn a_converging_money_counter_reads_its_default_in_stored_units() {
    let f = fixture();
    let pg = db("ordo_rhs_upsert_money");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let call = r#"credit($k: "ops", $amount: 1.00)"#;
    let r = f
        .run_write_as(&pg, &mut log, call, ANA)
        .expect("the first credit creates");
    let id = r[0]["created"].as_str().expect("created id").to_string();
    assert_eq!(col(&pg, "wallets", &id, "balance"), "250", "1.50 + 1.00");
    f.run_write_as(&pg, &mut log, call, ANA)
        .expect("the second credit converges");
    assert_eq!(col(&pg, "wallets", &id, "balance"), "350");
}

/// **A read field with no default is refused at registration, by name.**
/// The create branch reads each field as its default, so a field without
/// one has no value there. An optional field is not read as null: `n + 1`
/// over a null is null, an answer nobody wrote. The message names the field
/// and the fix.
#[test]
fn a_converging_read_of_a_field_with_no_default_is_refused_by_name() {
    let e = refusal(
        "entity tallies (key: tly) {\n  name: text unique\n  n: int\n  note: text?\n}\n\
         op badTally($k: text) =\n  set tallies { name: $k, n: n + 1 }",
    );
    assert_eq!(reg_class(&e), "invalid_binding_expr", "{e}");
    assert!(
        e.contains("creates-or-converges"),
        "the mode is the reason: {e}"
    );
    assert!(e.contains("Give `n` a default"), "the fix is named: {e}");

    let e = refusal(
        "entity tallies (key: tly) {\n  name: text unique\n  n: int = 0\n  note: text?\n}\n\
         op badNote($k: text) =\n  set tallies { name: $k, note: note + \"!\" }",
    );
    assert_eq!(reg_class(&e), "invalid_binding_expr", "{e}");
    assert!(
        e.contains("Give `note` a default"),
        "optional is not a default: {e}"
    );
}

/// A right-hand side sees the row it writes and **no other row**. A
/// dot-path would need a join an UPDATE has nowhere to put, and DL §11
/// declines inter-row computation outright.
#[test]
fn a_dot_path_in_a_right_hand_side_is_refused_by_name() {
    let e = refusal("op badDot($t: id) =\n  set tasks(id == $t) { title: project.name }");
    assert_eq!(reg_class(&e), "invalid_binding_expr", "{e}");
}

/// A misspelled column is caught at registration rather than reaching the
/// database as a statement it refuses to describe.
#[test]
fn a_right_hand_side_naming_no_field_is_refused_at_registration() {
    let e = refusal("op badField($t: id) =\n  set teams(id == $t) { used: usde + 1 }");
    assert_eq!(reg_class(&e), "projection_unknown_field", "{e}");
    assert!(e.contains("usde"), "{e}");
}

/// The fields whose value the platform must **hold** before the write, each
/// refused for its own reason. These are not renderer limitations: every one
/// of them names a host-side check that stands between the statement and the
/// database, and a value the database computes arrives after all of them.
#[test]
fn a_computed_value_is_refused_where_the_platform_must_hold_it() {
    // A unique key: rule 8 has to know which row the value identifies.
    let e = refusal("op badKey($m: id) =\n  set meters { id: $m, slug: slug + \"-2\" }");
    assert_eq!(reg_class(&e), "invalid_binding_expr", "{e}");
    assert!(e.contains("unique key"), "{e}");

    // A ref: G12 has to prove the target is visible to the caller.
    let e = refusal("op badRef($t: id) =\n  set tasks(id == $t) { project: project }");
    assert_eq!(reg_class(&e), "invalid_binding_expr", "{e}");
    assert!(e.contains("ref"), "{e}");

    // A sealed column: sealing needs the plaintext.
    let e = refusal("op badSealed($o: id) =\n  set orders(id == $o) { memo: memo + \"!\" }");
    assert_eq!(reg_class(&e), "invalid_binding_expr", "{e}");
    assert!(e.contains("sealed"), "{e}");

    // A closed value set: the ladder is checked before the write, and a
    // value outside it is a grant that reads nothing and writes nothing.
    let e =
        refusal("op badLadder($a: id) =\n  set boardsAccess(id == $a) { level: level + \"x\" }");
    assert_eq!(reg_class(&e), "invalid_binding_expr", "{e}");
    assert!(e.contains("closed set"), "{e}");
}

/// A division landing in an integral slot is still `implicit_rounding` — the
/// computed form does not become a way around DL §2's "no implicit rounding,
/// anywhere". `perHead` above passes only because it writes `round(x, 0)`.
#[test]
fn an_unconverted_division_still_refuses() {
    let e = refusal("op badRound($m: id) =\n  set meters { id: $m, spent: budget / heads }");
    assert_eq!(reg_class(&e), "implicit_rounding", "{e}");
}

/// **`round(x, 2)` does not convert a decimal into a `money` slot**, and
/// this is the first path that can say so.
///
/// `ordo_sema::rounds_to_integral` states the reason: *"money is minor units
/// here, so `round(x, 2)` asks for two digits finer than a minor unit and
/// there is no integral slot for that — calling it converted would put the
/// truncation back one layer down."* Only `round(x, 0)`, `floor` and `ceil`
/// convert.
///
/// Before a write RHS was type-checked, nothing on the write path consulted
/// that rule, and `syntax-examples.md` §4 carried
/// `rent: round($annual / 12, 2)` as a *working* example beside the
/// `implicit_rounding` it contrasts with. It was not working; it was
/// unchecked. The example is corrected and this is the test that keeps it
/// honest.
#[test]
fn round_to_two_places_still_does_not_reach_a_money_slot() {
    let f = fixture();
    let pg = db("ordo_rhs_round");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let m = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addMeter($label: "r", $budget: 120000, $slug: "r")"#,
            ANA,
        )
        .expect("a meter")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();

    let e = f
        .run_write_as(&pg, &mut log, &format!(r#"badScale($m: "{m}")"#), ANA)
        .expect_err("two digits finer than a minor unit is not a conversion");
    assert_eq!(class_of(&e), "implicit_rounding", "{e}");

    // `round(x, 0)` is the conversion, and it lands.
    f.run_write_as(&pg, &mut log, &format!(r#"perHead($m: "{m}")"#), ANA)
        .expect("round(budget / heads, 0) converts");
    assert_eq!(col(&pg, "meters", &m, "spent"), "120000", "one head");
}

/// A computed value whose type cannot reach the column is
/// `param_type_mismatch` — the same class and the same check a parameter
/// binding goes through, because it is the same question.
#[test]
fn a_computed_value_of_the_wrong_type_is_refused() {
    let f = fixture();
    let pg = db("ordo_rhs_types");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let r = f
        .run_write_as(&pg, &mut log, r#"addTeam($name: "t", $seats: 1)"#, ANA)
        .expect("a team");
    let team = r[0]["created"].as_str().unwrap().to_string();
    let e = f
        .run_write_as(&pg, &mut log, &format!(r#"badTy($t: "{team}")"#), ANA)
        .expect_err("a text cannot land in an int column");
    assert_eq!(class_of(&e), "param_type_mismatch", "{e}");
}
