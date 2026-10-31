//! Checklist 27ak — **money declares its places**, executed against Postgres.
//!
//! Ruled 2026-10-04: `price: money(2)` stores whole cents and source writes
//! dollars. The binder's half is proved on the bound IR in
//! `ordo-sema/tests/money_places.rs`; this half writes rows and reads the
//! column back, because "the literal became 2999" is only true if 2999 is
//! what the database holds and what a filter compares with.
//!
//! Antagonist finding 6 (2026-08-12) was a raw `29.99` compared against
//! stored cents. The fix then refused the literal; this suite pins the fix
//! the first spec promised: `price > 29.99` against a `money(2)` field
//! compares with 2999.
//!
//! Real Postgres, this suite's own databases (`ordo_money_*`).

mod gc_support;

use gc_support::{db, ANA};
use ordo_conformance::{Fixture, WriteLog, FIXTURE, SEED};

const CLUSTER: &str = r#"
entity items (key: itm) {
  name:  text
  price: money(2)
  fee:   money(2) = 1.50
  yen:   money = 0
  rate:  decimal(4) = 0.0825
  check price <= 1000.00
}
"#;

const OPS: &str = r#"
op addItem($name: text, $price: money(2)) =
  set items { name: $name, price: $price }

op addPlain($name: text) =
  set items { name: $name, price: 29.99 }

op bump($i: id) =
  set items { id: $i, price: price + 0.01 }

op taxOf($i: id) =
  set items { id: $i, fee: round(price * 0.08, 2) }

op dearer($floor: money(2)) =
  items(price > $floor)[name]

op setRate($i: id, $r: decimal(4)) =
  set items { id: $i, rate: $r }

op taxed($i: id) =
  set items { id: $i, fee: round(price * rate, 2) }
"#;

fn fixture() -> Fixture {
    let src = format!("{FIXTURE}\n{CLUSTER}\n{OPS}");
    Fixture::from_source(&src, SEED).expect("the money cluster compiles")
}

fn col(pg: &ordo_conformance::Pg, id: &str, column: &str) -> String {
    pg.query(&format!(
        r#"SELECT "{column}" FROM "items" WHERE "id" = '{id}'"#
    ))
    .expect("read back")[0][0]
        .clone()
        .unwrap_or_else(|| "NULL".into())
}

fn names(v: &serde_json::Value) -> Vec<String> {
    let mut n: Vec<String> = v
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["name"].as_str().unwrap_or_default().to_string())
        .collect();
    n.sort();
    n
}

fn add(f: &Fixture, pg: &ordo_conformance::Pg, log: &mut WriteLog, call: &str) -> String {
    f.run_write_as(pg, log, call, ANA).expect("written")[0]["created"]
        .as_str()
        .expect("created id")
        .to_string()
}

#[test]
fn major_units_are_written_and_minor_units_are_stored() {
    let f = fixture();
    let pg = db("ordo_money_store");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let a = add(&f, &pg, &mut log, r#"addItem($name: "a", $price: 29.99)"#);
    assert_eq!(col(&pg, &a, "price"), "2999", "29.99 dollars is 2999 cents");
    assert_eq!(col(&pg, &a, "fee"), "150", "the default 1.50 is 150 cents");
    assert_eq!(col(&pg, &a, "yen"), "0");

    let b = add(&f, &pg, &mut log, r#"addItem($name: "b", $price: 30)"#);
    assert_eq!(col(&pg, &b, "price"), "3000", "an int is dollars too");

    let c = add(&f, &pg, &mut log, r#"addPlain($name: "c")"#);
    assert_eq!(
        col(&pg, &c, "price"),
        "2999",
        "a literal in the op body converts"
    );

    // The harness has no door, so this reaches the write path's backstop,
    // which refuses rather than rounds. Through HTTP the door answers
    // first with `invalid_param` (`ordo-api`).
    let e = f
        .run_write_as(&pg, &mut log, r#"addItem($name: "d", $price: 29.999)"#, ANA)
        .expect_err("finer than a cent");
    assert!(e.contains("`money(2)` holds 2"), "{e}");
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "items""#).expect("count")[0][0].as_deref(),
        Some("3"),
        "nothing was written"
    );
}

/// Finding 6: the filter compares in cents.
#[test]
fn a_filter_in_dollars_compares_in_cents() {
    let f = fixture();
    let pg = db("ordo_money_filter");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    add(
        &f,
        &pg,
        &mut log,
        r#"addItem($name: "cheap", $price: 29.99)"#,
    );
    add(&f, &pg, &mut log, r#"addItem($name: "dear", $price: 30)"#);

    let rows = f
        .run_case(&pg, "items(price > 29.99)[name]")
        .expect("reads");
    assert_eq!(
        names(&rows),
        ["dear"],
        "29.99 is 2999 cents, not 29.99 cents"
    );
    let rows = f
        .run_case(&pg, "items(price == 29.99)[name]")
        .expect("reads");
    assert_eq!(names(&rows), ["cheap"]);
    let rows = f
        .run_case(&pg, "items(price > 29.989)[name]")
        .expect("reads");
    assert_eq!(
        names(&rows),
        ["cheap", "dear"],
        "a finer literal compares exactly in dollars"
    );
    let rows = f
        .run_case(&pg, "dearer($floor: 29.99)[name]")
        .expect("reads");
    assert_eq!(names(&rows), ["dear"], "a parameter takes dollars");
}

#[test]
fn computed_money_stays_in_cents() {
    let f = fixture();
    let pg = db("ordo_money_computed");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let a = add(&f, &pg, &mut log, r#"addItem($name: "a", $price: 29.99)"#);

    f.run_write_as(&pg, &mut log, &format!(r#"bump($i: "{a}")"#), ANA)
        .expect("a cent more");
    assert_eq!(col(&pg, &a, "price"), "3000", "29.99 + 0.01 is 30.00");

    f.run_write_as(&pg, &mut log, &format!(r#"taxOf($i: "{a}")"#), ANA)
        .expect("8% of 30.00");
    assert_eq!(col(&pg, &a, "fee"), "240", "round(30.00 * 0.08, 2) is 2.40");
}

/// A `check` is written in dollars and reads the column in dollars.
#[test]
fn a_check_reads_dollars() {
    let f = fixture();
    let pg = db("ordo_money_check");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    add(&f, &pg, &mut log, r#"addItem($name: "ok", $price: 1000)"#);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addItem($name: "no", $price: 1000.01)"#,
            ANA,
        )
        .expect_err("over the cap");
    assert!(e.contains("check_violation"), "{e}");
}

/// The places are checked when the program registers, not on the first call.
#[test]
fn a_literal_finer_than_the_places_does_not_register() {
    let src =
        format!("{FIXTURE}\n{CLUSTER}\nop bad =\n  set items {{ name: \"x\", price: 1.005 }}\n");
    let e = Fixture::from_source(&src, SEED)
        .map(|_| ())
        .expect_err("1.005 dollars has no cent value");
    assert!(e.to_string().contains("implicit_rounding"), "{e}");

    let src = format!("{FIXTURE}\nentity bad {{ p: money(2) = 1.005 }}\n");
    let e = Fixture::from_source(&src, SEED)
        .map(|_| ())
        .expect_err("a default that would round");
    assert!(e.to_string().contains("implicit_rounding"), "{e}");
}

// ── decimal(p): a storable exact decimal (27ak item 4) ───────────────────

/// `rate: decimal(4)` is a `numeric` at scale 4. It is stored exactly,
/// compared exactly, and becomes money only through an explicit `round`.
#[test]
fn a_decimal_field_stores_exactly_and_becomes_money_by_round() {
    let f = fixture();
    let pg = db("ordo_money_decimal");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    let a = add(&f, &pg, &mut log, r#"addItem($name: "a", $price: 100)"#);
    assert_eq!(col(&pg, &a, "rate"), "0.0825", "the default, at scale 4");

    f.run_write_as(&pg, &mut log, &format!(r#"taxed($i: "{a}")"#), ANA)
        .expect("8.25% of 100.00");
    assert_eq!(
        col(&pg, &a, "fee"),
        "825",
        "round(100.00 * 0.0825, 2) is 8.25"
    );

    f.run_write_as(
        &pg,
        &mut log,
        &format!(r#"setRate($i: "{a}", $r: 0.1)"#),
        ANA,
    )
    .expect("a rate with fewer places");
    assert_eq!(col(&pg, &a, "rate"), "0.1000");
    // The chain records the value in its one canonical spelling, under the
    // decimal tag: `0.1`, whatever scale the column prints it at.
    let group = log.last().expect("a group");
    let after = group.commands[0].after.as_ref().expect("an after image");
    assert_eq!(
        after.get("rate"),
        Some(&ordo_commandlog::Value::Decimal(
            ordo_commandlog::value::Decimal::parse("0.1").expect("a decimal")
        )),
        "{after:?}"
    );
    let rows = f
        .run_case(&pg, "items(rate > 0.0999)[name]")
        .expect("reads");
    assert_eq!(names(&rows), ["a"]);
    let rows = f.run_case(&pg, "items(rate > 0.1)[name]").expect("reads");
    assert!(names(&rows).is_empty(), "0.1000 is not more than 0.1");

    let e = f
        .run_write_as(
            &pg,
            &mut log,
            &format!(r#"setRate($i: "{a}", $r: 0.12345)"#),
            ANA,
        )
        .expect_err("five places into decimal(4)");
    assert!(e.contains("`decimal(4)` holds 4"), "{e}");
    assert_eq!(col(&pg, &a, "rate"), "0.1000", "nothing was written");
}

/// A decimal does not land in a money field on its own.
#[test]
fn a_decimal_into_money_needs_round() {
    let src =
        format!("{FIXTURE}\n{CLUSTER}\nop bad($i: id) =\n  set items {{ id: $i, fee: rate }}\n");
    let e = Fixture::from_source(&src, SEED)
        .map(|_| ())
        .expect_err("a rate is not an amount of money");
    assert!(e.to_string().contains("implicit_rounding"), "{e}");
    let src = format!(
        "{FIXTURE}\n{CLUSTER}\nop bad($i: id) =\n  set items {{ id: $i, fee: price * 0.5 }}\n"
    );
    let e = Fixture::from_source(&src, SEED)
        .map(|_| ())
        .expect_err("half a price may not be whole cents");
    assert!(e.to_string().contains("implicit_rounding"), "{e}");
}
