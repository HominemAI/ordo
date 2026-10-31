//! Checklist 9v — an expression function applied outside its domain is
//! refused at the line that caused it.
//!
//! Before this, `lower(price)` bound, planned, and emitted SQL Postgres would
//! refuse. What caught it was F6's shape verification at registration, which
//! is a real gate — it is why `hash()` was restorable without inventing
//! anything — but it is decent **by accident**: the operator is told the
//! database would not describe a statement, not that an author applied
//! `lower` to a money, and the op that fails to load may be one nobody just
//! edited.
//!
//! The class is `argument_type_mismatch`, a deliberate new member of the
//! compile inventory. It is not `param_type_mismatch` (that is about a caller
//! binding a value into a callee's declared parameter) and not
//! `invalid_binding_expr` (a write's right-hand side). Stretching either
//! would make the class name stop predicting where to look.
//!
//! These tests are compile-only: nothing here needs a row, and the point is
//! that the refusal happens before a statement is ever built.

use ordo_conformance::Fixture;

fn refusal(src: &str) -> String {
    let f = Fixture::load().expect("the market fixture compiles");
    f.compile(src)
        .map(|p| panic!("`{src}` should not compile, but planned:\n{}", p.sql))
        .unwrap_err()
}

fn accepts(src: &str) {
    let f = Fixture::load().expect("the market fixture compiles");
    if let Err(e) = f.compile(src) {
        panic!("`{src}` is well-typed and must still compile: {e}");
    }
}

/// The four the checklist names, each refused with the class and with a
/// message naming the function, the type it got, and what it takes.
#[test]
fn the_four_unchecked_applications_are_refused_by_class() {
    for (src, func, got) in [
        ("orderItems[x: lower(amount)]", "lower", "money"),
        ("orders[x: length(placed)]", "length", "date"),
        ("orderItems[x: trim(qty)]", "trim", "int"),
        ("orderItems[x: hash(amount)]", "hash", "money"),
    ] {
        let e = refusal(src);
        assert!(
            e.contains("argument_type_mismatch"),
            "`{src}` must be refused by class, got: {e}"
        );
        assert!(
            e.contains(func) && e.contains(got),
            "the message names the function and what it got: {e}"
        );
    }
}

/// The refusal has to be narrow, because a false positive breaks working
/// programs — which is worse than the gap it closes. Every one of these was
/// legal before and stays legal.
#[test]
fn well_typed_applications_are_untouched() {
    // The numeric tower agrees with itself: `round`/`floor`/`ceil` take it
    // whole, because `decimal` is expression-only and flows into `int` and
    // `money` by the rules DL §2 pins.
    accepts("orderItems[x: round(amount / qty, 2)]");
    accepts("orderItems[x: floor(amount)]");
    accepts("orderItems[x: abs(qty)]");

    // `mod` on money: "integer" in DL §3 describes the DIVISION's semantics,
    // not an exclusion of money — money is an integer count of minor units,
    // so this is the cents part and is meaningful. The first draft of the
    // domain table refused it and broke `decimal.rs`, which is the whole
    // reason this assertion is here rather than in a comment.
    accepts("orderItems[x: mod(amount, 100)]");

    // Date parts over a date, text functions over text.
    accepts("orders[x: year(placed)]");
    accepts("products[x: lower(name)]");
    accepts("products[x: length(name)]");
}

/// `least`/`greatest`/`coalesce` name no domain at all — they require one
/// type, which is a rule about the RELATIONSHIP between arguments and cannot
/// be said by a per-position table. So they are checked separately, and the
/// numeric tower still counts as agreeing.
#[test]
fn the_agreement_functions_constrain_the_pair_not_the_position() {
    let e = refusal("products[x: least(name, 1)]");
    assert!(e.contains("argument_type_mismatch"), "{e}");
    assert!(
        e.contains("one type in both"),
        "the message says what the rule is, not just that it broke: {e}"
    );

    // …and a mixed-numeric pair is exactly the comparison an author means.
    accepts("orderItems[x: least(amount, 0)]");
    accepts("orderItems[x: greatest(qty, 1)]");
}

/// The gate this replaces still exists and still runs. What changed is which
/// gate speaks first — so an op that WOULD have failed to load now fails to
/// bind, and the load gate's coverage does not quietly grow an
/// `undescribable` entry in its place.
#[test]
fn the_refusal_happens_at_bind_not_at_load() {
    let e = refusal("orderItems[x: lower(amount)]");
    assert!(
        !e.to_lowercase().contains("describe") && !e.to_lowercase().contains("shape"),
        "this must not be the load gate answering: {e}"
    );
}
