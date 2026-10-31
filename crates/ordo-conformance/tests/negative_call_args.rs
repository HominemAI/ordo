//! Checklist 11-10a, the second defect the bulk import build found: **a
//! negative number could not be bound as a call argument.**
//!
//! The grammar reads a leading `-` as `unary := '-' unary` and builds
//! `Expr::Neg` uniformly (`ordo-syntax/src/parser.rs`), which is right for
//! an expression and wrong for a literal parameter value — the binder's
//! `lit_ty` answers only for the literal productions, so `topN($n: -1)`
//! refused as *"bound to something that is not a value"*.
//! `ordo_conformance::write`'s `fold_negation` (now `ordo_sema::fold_neg_lit`,
//! see its doc) closed this for the **write** path first; this file is the
//! **read** path's proof, exercised the way a caller actually reaches it —
//! an op call parsed from source text, exactly the shape
//! `ordo_host::FixtureExecutor::call_source` renders a bound negative
//! argument into (`$amt: -5`) — not a hand-built `Expr` fed straight to a
//! binder function.
//!
//! Run this suite against the pre-fix binder (revert `const_expr`'s
//! fallback arm in `ordo-sema/src/lib.rs` to call `lit_ty(other)` without
//! folding first) and every one of the first two tests below fails with
//! `bind: invalid_binding_expr` / `unsupported` — confirmed empirically
//! while writing this fix, not asserted from reading the code.

use ordo_conformance::{Fixture, Pg};

/// Two ops of this suite's own: one `int` parameter, one `money` parameter,
/// both used in an ordinary per-row filter so a wrong bind would show up as
/// wrong rows, not just a compile error.
const OPS: &str = r#"
op qtyAtLeast($min: int) unbounded =
  orderItems[qty]
  | qty >= $min
  sort qty

op priceAtLeast($floor: money) unbounded =
  products[name, price]
  | price >= $floor
  sort name
"#;

fn fixture() -> Fixture {
    Fixture::from_source(
        &format!("{}\n{OPS}", ordo_conformance::FIXTURE),
        ordo_conformance::SEED,
    )
    .expect("the extra ops register")
}

fn seeded() -> Pg {
    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let pg = Pg::named("ordo_11_10a_negative_args");
    ONCE.get_or_init(|| fixture().reset(&pg))
        .clone()
        .expect("fixture DDL applies and seeds");
    pg
}

/// The `int` case: every seeded `orderItems` row has `qty = 1`
/// (`fixtures/seed.sql`), so `qty >= -5` is true of all eight and a bind
/// that could not fold the negative literal would never get this far to be
/// wrong about it — it fails to compile before a row is read.
#[test]
fn a_negative_int_argument_binds_and_returns_the_right_rows() {
    let f = fixture();
    let pg = seeded();

    let got = f
        .run_case_as(&pg, "qtyAtLeast($min: -5)", &[], None)
        .unwrap_or_else(|e| panic!("`qtyAtLeast($min: -5)` did not run: {e}"));
    assert_eq!(
        got.as_array().expect("rows").len(),
        8,
        "qty >= -5 holds for every seeded row: {got}"
    );

    // Identity against a literal the old code path already bound: negating
    // the sign changes which rows qualify and nothing else about how the
    // parameter is read.
    let excludes_none = f
        .run_case_as(&pg, "qtyAtLeast($min: -5)", &[], None)
        .unwrap();
    let excludes_all = f
        .run_case_as(&pg, "qtyAtLeast($min: 2)", &[], None)
        .unwrap();
    assert_eq!(
        excludes_all.as_array().unwrap().len(),
        0,
        "no row has qty >= 2"
    );
    assert_ne!(excludes_none, excludes_all);
}

/// The `money` case, called the way a caller (or `call_source`) actually
/// spells a negative amount: minor units, no decimal point — `-100`, not
/// `-1.00`. Every seeded product is priced positive, so `price >= -100`
/// returns all six, by name.
#[test]
fn a_negative_money_argument_binds_and_returns_the_right_rows() {
    let f = fixture();
    let pg = seeded();

    let got = f
        .run_case_as(&pg, "priceAtLeast($floor: -100)", &[], None)
        .unwrap_or_else(|e| panic!("`priceAtLeast($floor: -100)` did not run: {e}"));
    let names: Vec<&str> = got
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["consult", "drill", "hammer", "saw", "tune-up", "wrench"],
        "price >= -100 holds for every seeded product, name-sorted"
    );

    // And the filter is doing real work, not accepting everything: a very
    // positive floor excludes the cheap end.
    let got = f
        .run_case_as(&pg, "priceAtLeast($floor: 5000)", &[], None)
        .unwrap();
    let names: Vec<&str> = got
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["consult", "drill", "tune-up"]);
}

/// The `decimal` production folds too (`fold_neg_lit` handles `IntLit` and
/// `DecimalLit` alike, matching `definition-language.md` §2's two exact
/// numeric literal forms) — but folding a negative decimal into
/// `DecimalLit("-12.50")` does not make it a legal `money` value. DL is
/// explicit that money is integral minor units, and `fit_slot`
/// refuses ANY decimal-shaped literal in a money slot, sign aside
/// (`"money is integral minor units — write 2999, not 29.99"`). This is
/// the regression this fix must not introduce: before it, `-12.50` failed
/// as the generic, unhelpful `"bound to something that is not a value"`;
/// after it, the fold succeeds and the specific, correct rule still fires.
#[test]
fn a_negative_decimal_argument_still_meets_moneys_integral_rule() {
    let f = fixture();

    let e = f
        .compile("priceAtLeast($floor: -12.50)")
        .map(|p| {
            panic!(
                "a fractional money literal must not compile, but planned:\n{}",
                p.sql
            )
        })
        .unwrap_err();
    // Since 27ak bare `money` is `money(0)`, and `12.50` has two places too
    // many: the specific refusal is `implicit_rounding`.
    assert!(
        e.contains("implicit_rounding") && e.contains("money(0)"),
        "the fold must produce the SPECIFIC refusal, not the generic \
         `invalid_binding_expr` that came before it: {e}"
    );

    // The positive spelling of the same mistake is refused identically —
    // sign was never what made this wrong.
    let e2 = f
        .compile("priceAtLeast($floor: 12.50)")
        .map(|p| panic!("planned:\n{}", p.sql))
        .unwrap_err();
    assert!(
        e2.contains("implicit_rounding") && e2.contains("money(0)"),
        "the rule does not care about sign, only places: {e2}"
    );
}
