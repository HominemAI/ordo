//! Money declares its places, and source writes major units (27ak, ruled
//! 2026-10-04).
//!
//! `price: money(2)` stores whole cents. Source says `29.99`, and the
//! binder turns it into the `2999` the column holds, wherever a literal
//! meets a money: a condition, a parameter, a write's right-hand side.
//! These tests read the bound IR, so they say what the compiler decided
//! and not what Postgres happened to answer.

use ordo_schema::{build, Schema};
use ordo_sema::ir::BExpr;
use ordo_sema::{bind, write_value_expr, Query, Registry, SemaError};
use ordo_syntax::{parse_expr, parse_pipeline, parse_program, ScalarTy};

const SRC: &str = r#"
entity items {
  name:  text
  price: money(2)
  yen:   money
  qty:   int
}

op cheap($cap: money(2)) =
  items(price < $cap)[name, price]
"#;

fn env() -> (Schema, Registry) {
    let p = parse_program(SRC).expect("fixture parses");
    (
        build(&p).expect("schema builds"),
        Registry::from_program(&p),
    )
}

fn bound(src: &str) -> Result<Query, SemaError> {
    let (s, r) = env();
    bind(
        &s,
        &r,
        &parse_pipeline(src).expect("pipeline parses"),
        ordo_sema::Clock::CORPUS,
    )
}

fn q(src: &str) -> Query {
    bound(src).expect("binds")
}

fn ir(src: &str) -> String {
    format!("{:?}", q(src))
}

fn rhs(value: &str, declared: ScalarTy) -> Result<BExpr, SemaError> {
    let (s, _) = env();
    let e = parse_expr(value).expect("expression parses");
    write_value_expr(&s, "items", "items", &e, &[], None, declared).map(|w| w.expr)
}

/// Antagonist finding 6, fixed by the conversion: `price > 29.99` on a
/// `money(2)` compares against 2999 minor units, never against `29.99`.
#[test]
fn a_major_unit_literal_compares_in_minor_units() {
    let s = ir("items(price > 29.99)[name]");
    assert!(s.contains("IntLit { value: 2999"), "{s}");
    assert!(
        !s.contains("29.99"),
        "the raw literal must not reach SQL: {s}"
    );
    let s = ir("items(price > 30)[name]");
    assert!(
        s.contains("IntLit { value: 3000"),
        "an int is dollars too: {s}"
    );
    let s = ir("items(price in (1.50, 2))[name]");
    assert!(
        s.contains("IntLit { value: 150") && s.contains("IntLit { value: 200"),
        "{s}"
    );
}

/// A comparison is not a typed slot (DL §2), so a literal finer than the
/// places is compared exactly in major units rather than refused.
#[test]
fn a_finer_literal_compares_exactly_in_major_units() {
    let s = ir("items(price > 29.999)[name]");
    assert!(s.contains("29.999"), "{s}");
    assert!(
        s.contains("\"0.01\""),
        "the column is read in dollars, `price * 0.01`: {s}"
    );
}

/// Bare `money` is `money(0)`: whole units, converted by the identity, so
/// a program written before places existed binds what it always bound.
#[test]
fn bare_money_is_whole_units() {
    let s = ir("items(yen > 2999)[name]");
    assert!(s.contains("IntLit { value: 2999"), "{s}");
    assert!(!s.contains("Mul"), "no scaling at places 0: {s}");
}

#[test]
fn a_parameter_takes_major_units() {
    let s = ir("cheap($cap: 29.99)[name]");
    assert!(s.contains("IntLit { value: 2999"), "{s}");
    let e = bound("cheap($cap: 29.999)[name]").expect_err("finer than a cent");
    assert_eq!(e.code, "implicit_rounding");
    assert!(e.message.contains("money(2)"), "{}", e.message);
}

#[test]
fn different_places_do_not_mix() {
    let e = bound("items(price > yen)[name]").expect_err("cents against yen");
    assert_eq!(e.code, "money_places_mismatch");
    let e = bound("items[t: price + yen]").expect_err("cents plus yen");
    assert_eq!(e.code, "money_places_mismatch");
}

/// `int` beside a money is dollars: `price > qty` scales `qty` by 100.
#[test]
fn an_int_beside_money_is_scaled_and_a_count_is_not() {
    let s = ir("items(price > qty)[name]");
    assert!(s.contains("IntLit { value: 100"), "{s}");
    let counted = q("items[t: price * qty]");
    assert_eq!(
        counted.outputs[0].expr.ty(),
        ScalarTy::Money(2),
        "a money times a count is a money"
    );
}

/// A money times a rate is a decimal in major units, and an average of a
/// money is a decimal in dollars.
#[test]
fn money_meets_decimals_in_major_units() {
    let rate = q("items[t: price * 0.08]");
    assert!(matches!(rate.outputs[0].expr.ty(), ScalarTy::Decimal(_)));
    let s = format!("{:?}", rate.outputs[0].expr);
    assert!(s.contains("\"0.01\""), "{s}");
    let q2 = q("items\ngroup name { a: avg price }");
    let s = format!("{:?}", q2.outputs);
    assert!(s.contains("\"0.01\""), "avg widens its argument: {s}");
}

#[test]
fn a_write_lands_money_through_an_explicit_round() {
    let m2 = ScalarTy::Money(2);
    let ok = rhs("round(price * 0.08, 2)", m2).expect("rounded to the cent");
    assert_eq!(ok.ty(), m2);
    let e = rhs("price * 0.08", m2).expect_err("unrounded");
    assert_eq!(e.code, "implicit_rounding");
    let e = rhs("round(price * 0.08, 3)", m2).expect_err("rounded finer than a cent");
    assert_eq!(e.code, "implicit_rounding");
    let plus = rhs("price + 1", m2).expect("a dollar more");
    assert!(
        format!("{plus:?}").contains("IntLit { value: 100"),
        "{plus:?}"
    );
    let lit = rhs("29.99", m2).expect("a literal");
    assert!(
        format!("{lit:?}").contains("IntLit { value: 2999"),
        "{lit:?}"
    );
    let e = rhs("29.999", m2).expect_err("finer than a cent");
    assert_eq!(e.code, "implicit_rounding");
    let floor = rhs("floor(price)", m2).expect("whole dollars");
    assert!(
        format!("{floor:?}").contains("IntLit { value: 100"),
        "floor works in dollars and scales back: {floor:?}"
    );
}

/// A bound or a default finer than the places is refused at registration.
#[test]
fn a_parameter_bound_must_fit_its_places() {
    let p = parse_program(
        "entity items { price: money(2) }\nop q($cap: money(2) <= 100.001) =\n  items[price]\n",
    )
    .expect("parses");
    let e = Registry::from_program(&p)
        .check_param_places()
        .expect_err("a bound the door could only keep by rounding");
    assert_eq!(e.code, "implicit_rounding");
    let p = parse_program(
        "entity items { price: money(2) }\nop q($cap: money(2) <= 100.50) =\n  items[price]\n",
    )
    .expect("parses");
    Registry::from_program(&p)
        .check_param_places()
        .expect("a bound to the cent fits");
}

/// Two moneys multiply to a decimal at every places value, bare `money`
/// (`money(0)`) included, and the product lands in no money slot without a
/// conversion the author writes (ruled 2026-10-05). Before, `yen * yen`
/// typed `money(0)` while `price * price` typed a decimal.
#[test]
fn a_money_times_a_money_is_a_decimal_at_every_places_value() {
    let cents = q("items[t: price * price]");
    assert_eq!(cents.outputs[0].expr.ty(), ScalarTy::Decimal(4));
    let yen = q("items[t: yen * yen]");
    assert_eq!(yen.outputs[0].expr.ty(), ScalarTy::Decimal(0));

    let e = rhs("price * price", ScalarTy::Money(2)).expect_err("cents squared");
    let f = rhs("yen * yen", ScalarTy::Money(0)).expect_err("yen squared");
    assert_eq!(e.code, f.code, "one refusal for every places value");
    // The author's conversion lands it, as it does at places 2.
    rhs("round(yen * yen)", ScalarTy::Money(0)).expect("rounded");
}
