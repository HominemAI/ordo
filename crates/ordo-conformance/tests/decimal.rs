//! Checklist 9c — exact-decimal arithmetic, executed.
//!
//! This is a **wrong-answers** bug, not a compile bug: `money` and `int` are
//! both `bigint`, and `bigint / bigint` is truncating integer division in
//! Postgres, so `total / qty` used to return a plausible number that was
//! simply wrong. A test that asserted on the SQL text would not have caught
//! it — the old SQL was perfectly well-formed. So every case here runs
//! against a real server and asserts the **value**.
//!
//! The pins under test, all from `definition-language.md` §2 (Numeric
//! semantics / No implicit rounding / Division: determinism across venues):
//!
//! - `/` and `avg` produce `decimal` at a fixed FRACTIONAL scale of 12 —
//!   twelve digits after the point, the same number in every plan and every
//!   deployment, because the scale belongs to the result type and not to the
//!   operands.
//! - The SQL evaluates in Postgres's UNCONSTRAINED `numeric` with an
//!   explicit `round(value, 12)` at every division and average.
//! - Nothing rounds implicitly: a decimal landing in a typed slot is the
//!   compile error `implicit_rounding` until an explicit conversion says how.
//! - `/ 0` is a typed runtime error, loud — never null, never `internal`.
//! - No float appears anywhere in a plan.

use ordo_conformance::{Fixture, Pg};
use serde_json::json;

/// This suite's own database, created on demand — the shared
/// `ordo_conformance` database is reset by whichever test binary gets there
/// first, and these cases assert exact values over the seeded rows.
fn seeded() -> (Fixture, Pg) {
    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_decimal");
    let pg = Pg {
        db: "ordo_decimal".into(),
        ..Pg::default()
    };
    let f = Fixture::load().expect("market fixture compiles");
    ONCE.get_or_init(|| f.reset(&pg))
        .clone()
        .expect("seeded ordo_decimal");
    (f, pg)
}

/// Register extra ops against the fixture and run the gate every
/// registration path runs — the same helper `corpus_stage5` uses.
fn register(f: &Fixture, src: &str) -> Result<(), String> {
    let program = ordo_syntax::parse_program(src)
        .map_err(|e| format!("parse: {} at {}", e.message, e.span))?;
    let mut registry = f.registry.clone();
    registry.extend_from(&program);
    ordo_plan::cost::check_registration(&f.schema, &registry).map_err(|e| e.to_string())
}

// ── the regression itself ────────────────────────────────────────────────

/// **The bug.** `orderItems.amount` is `money` — minor-unit `bigint` — and
/// dividing it used to lower to `bigint / 3`, which truncates. 9500 minor
/// units split three ways is 3166.666…, and the old planner answered 3166:
/// two thirds of a cent, gone, silently, with no error anywhere.
///
/// The first assertion is the *before*: it asks Postgres directly what
/// `bigint / bigint` does, so the test carries its own evidence of what was
/// being emitted rather than asking the reader to take it on trust.
#[test]
fn truncating_integer_division_is_gone() {
    let (f, pg) = seeded();

    // BEFORE — what `BinOp::Div` used to lower to, straight from the server.
    let truncated = f
        .run_sql(&pg, "SELECT 9500 / 3, 2000 / 3, 52500 / 8")
        .expect("raw integer division runs");
    assert_eq!(
        truncated,
        json!([[3166, 666, 6562]]),
        "bigint / bigint truncates — this is the wrong answer the planner used to emit"
    );

    // AFTER — the same division through the planner.
    let got = f
        .run_case(&pg, "orderItems(amount == 9500)[amount, third: amount / 3]")
        .expect("the division plans and runs");
    assert_eq!(
        got,
        json!([{ "amount": 9500, "third": "3166.666666666667" }]),
        "the third of 9500 minor units is exact to scale 12, not 3166"
    );
}

/// Scale 12 is a property of the RESULT, not of the operands: an exact
/// division carries the same twelve digits an inexact one does. That is the
/// whole point of the pin — an expression means the same thing in every
/// plan, so a stage that moves between SQL and the host cannot change what
/// it says.
#[test]
fn division_holds_a_fixed_fractional_scale_of_twelve() {
    let (f, pg) = seeded();
    let got = f
        .run_case(&pg, "orderItems[amount, third: amount / 3]")
        .expect("runs");
    assert_eq!(
        got,
        json!([
            { "amount": 1500,  "third": "500.000000000000" },
            { "amount": 3000,  "third": "1000.000000000000" },
            { "amount": 9500,  "third": "3166.666666666667" },
            { "amount": 1500,  "third": "500.000000000000" },
            { "amount": 12000, "third": "4000.000000000000" },
            { "amount": 3000,  "third": "1000.000000000000" },
            { "amount": 20000, "third": "6666.666666666667" },
            { "amount": 2000,  "third": "666.666666666667" },
        ]),
        "every quotient carries twelve fractional digits, exact or not"
    );
}

/// `avg` is the other half of the ruling: it *is* a division — by the row
/// count — and it lands on the same fixed scale. Before, `avg(bigint)`
/// returned whatever scale Postgres chose from the operand (a
/// deployment-dependent answer to a deployment-independent question).
#[test]
fn avg_is_decimal_at_the_same_scale() {
    let (f, pg) = seeded();

    // 1500+3000+9500+1500+12000+3000+20000+2000 = 52500 over 8 rows.
    let got = f
        .run_case(&pg, "orderItems[amount]\nreduce { a: avg amount }")
        .expect("runs");
    assert_eq!(got, json!([{ "a": "6562.500000000000" }]));

    // Postgres's own unpinned answer, for contrast: same value, different
    // number of digits, chosen by the operand's type rather than by the
    // language.
    let bare = f
        .run_sql(&pg, "SELECT avg(\"amount\")::text FROM \"orderItems\"")
        .expect("runs");
    assert_eq!(bare, json!([["6562.5000000000000000"]]));

    // And the empty set is still null, not zero — `round(NULL, 12)` is
    // NULL, so pinning the scale did not quietly change what an empty
    // average means.
    let empty = f
        .run_case(
            &pg,
            "orderItems(amount > 999999)[amount]\nreduce { a: avg amount }",
        )
        .expect("runs");
    assert_eq!(empty, json!([{ "a": null }]));
}

/// Division over a COMPOSED, grouped source: the pin travels with the
/// expression, not with the entity it started on. `revenue` here is a
/// `sum` over `orderItems.amount` computed in a subquery, and dividing it
/// is the "revenue per unit sold" an author actually writes.
#[test]
fn division_survives_composition_and_grouping() {
    let (f, pg) = seeded();
    let got = f
        .run_case(
            &pg,
            "productRevenue[revenue, sold, per: revenue / sold, third: revenue / 3]\nsort -revenue",
        )
        .expect("runs");
    assert_eq!(
        got,
        json!([
            { "revenue": 20000, "sold": 1, "per": "20000.000000000000", "third": "6666.666666666667" },
            { "revenue": 12000, "sold": 1, "per": "12000.000000000000", "third": "4000.000000000000" },
            { "revenue": 9500,  "sold": 1, "per": "9500.000000000000",  "third": "3166.666666666667" },
            { "revenue": 6000,  "sold": 2, "per": "3000.000000000000",  "third": "2000.000000000000" },
            { "revenue": 3000,  "sold": 2, "per": "1500.000000000000",  "third": "1000.000000000000" },
            { "revenue": 2000,  "sold": 1, "per": "2000.000000000000",  "third": "666.666666666667" },
        ]),
    );
}

// ── the exactness the change must not have cost ──────────────────────────

/// `+ - *` on money stay exact and stay `money`: minor units in `bigint`,
/// decoded as JSON integers. If the decimal work had leaked a `numeric`
/// (or, worse, a float) into these, every money value in the system would
/// have started arriving as a string or a rounded double.
#[test]
fn money_addition_subtraction_and_multiplication_stay_exact_integers() {
    let (f, pg) = seeded();
    let got = f
        .run_case(
            &pg,
            "orderItems(amount == 9500)[plus: amount + 5, minus: amount - 5, times: amount * 3]",
        )
        .expect("runs");
    assert_eq!(
        got,
        json!([{ "plus": 9505, "minus": 9495, "times": 28500 }])
    );
}

/// No float, anywhere. The whole plan is grepped rather than one operator,
/// because the rule is about the plan and not about division: a `float8`
/// cast, a `double precision` cast, or Postgres picking the `float8`
/// overload of `floor` would each be a silent loss of exactness.
#[test]
fn no_plan_contains_a_float() {
    let f = Fixture::load().expect("fixture");
    for src in [
        "orderItems[amount, third: amount / 3]",
        "orderItems[amount]\nreduce { a: avg amount }",
        "orderItems[cents: round(amount / 3, 0), down: floor(amount / 3), up: ceil(amount / 3)]",
        "orderItems(amount / 3 > 1000)[amount]",
    ] {
        let sql = f.compile(src).expect("compiles").sql;
        let lower = sql.to_lowercase();
        for bad in ["float", "double precision", "real"] {
            assert!(
                !lower.contains(bad),
                "`{bad}` in the plan for `{src}`:\n{sql}"
            );
        }
        // Positively: the division is pinned where it happens.
        assert!(
            lower.contains("::numeric"),
            "no numeric cast in the plan for `{src}`:\n{sql}"
        );
    }
}

/// The conversions land on `bigint`, which is what makes them conversions.
/// `round(x, 0)`, `floor(x)` and `ceil(x)` over a decimal produce integral
/// minor units and decode as JSON integers again; `round(x, 2)` does not,
/// because two digits finer than a minor unit has no integral slot — it
/// stays a decimal rather than pretending to have converted.
#[test]
fn the_three_conversions_land_on_integral_minor_units() {
    let (f, pg) = seeded();
    let got = f
        .run_case(
            &pg,
            "orderItems(amount == 9500)[\
             cents: round(amount / 3, 0), \
             bare:  round(amount / 3), \
             down:  floor(amount / 3), \
             up:    ceil(amount / 3), \
             finer: round(amount / 3, 2)]",
        )
        .expect("runs");
    assert_eq!(
        got,
        json!([{
            // 3166.666… half-up (away from zero) is 3167.
            "cents": 3167,
            "bare":  3167,
            "down":  3166,
            "up":    3167,
            // Still a decimal, and `decimal` is scale 12 by definition —
            // `round(x, n)` rounds the value, it does not retype it.
            "finer": "3166.670000000000",
        }]),
    );
}

// ── `/ 0` ────────────────────────────────────────────────────────────────

/// `/ 0` is a typed runtime error, uniform with `mod(a, 0)` — loud, and
/// never null. The planner deliberately emits no `nullif`: answering NULL
/// for a division by zero is the folklore DL §2 refuses by name, and it is
/// the same class of failure as truncation (a plausible wrong answer with
/// no error attached).
#[test]
fn division_by_zero_is_loud_and_is_not_null() {
    let (f, pg) = seeded();

    // Every seeded qty is 1, so `qty - 1` is a real, data-derived zero.
    let e = f
        .run_case(&pg, "orderItems[amount, per: amount / (qty - 1)]")
        .expect_err("dividing by zero must not answer");
    assert!(
        e.to_lowercase().contains("division by zero"),
        "the server's refusal should say what happened: {e}"
    );

    // `mod(a, 0)` answers the same way — one behaviour, not two.
    let e = f
        .run_case(&pg, "orderItems[m: mod(amount, qty - 1)]")
        .expect_err("mod by zero must not answer");
    assert!(e.to_lowercase().contains("division by zero"), "{e}");

    // Structurally: nothing in the plan converts the zero into a null.
    let sql = f
        .compile("orderItems[per: amount / (qty - 1)]")
        .expect("compiles")
        .sql;
    assert!(!sql.to_lowercase().contains("nullif"), "{sql}");
}

// ── the compile-time half ────────────────────────────────────────────────

/// `implicit_rounding`, at the two typed slots this slice actually has: a
/// write RHS for a `money`/`int` field (`traps.md`'s canonical case) and a
/// `money`/`int` parameter. The message has to name the conversion, because
/// the whole point of the refusal is that the author decides.
#[test]
fn a_decimal_may_not_land_in_a_typed_slot_unconverted() {
    let f = Fixture::load().expect("fixture");

    // A write RHS. `traps.md`: `perHead: $total / $heads`.
    let e = register(
        &f,
        "op perHead($e: id, $n: int, $amt: money) =\n  set ledgerEntries(id == $e) { amount: $amt / $n }",
    )
    .expect_err("a division may not land in a money field");
    assert!(e.contains("implicit_rounding"), "{e}");
    assert!(
        e.contains("round(x, 0)"),
        "the refusal must name the fix: {e}"
    );
    assert!(
        e.contains("allocate"),
        "and must point at the primitive for the case that is really a split: {e}"
    );

    // Stating the conversion is accepted — the rule is "say how", not "no".
    register(
        &f,
        "op perHead($e: id, $n: int, $amt: money) =\n  set ledgerEntries(id == $e) { amount: round($amt / $n, 0) }",
    )
    .expect("the stated conversion registers");

    // A parameter slot: `cheapActive($cap: money)`. This one is the HOST
    // side of the same bug — the binder folds constant arithmetic at
    // compile time, and it used to fold `/` with Rust's truncating integer
    // division. `3000 / 7` folded to 428 while the identical expression in
    // SQL answers 428.571428571429: the same expression, two venues, two
    // answers.
    let e = f
        .compile("cheapActive($cap: 3000 / 7)[name]")
        .expect_err("a decimal cannot bind a money parameter");
    assert!(e.contains("implicit_rounding"), "{e}");
    assert!(
        !e.contains("428"),
        "and it must not have quietly folded to a truncated 428: {e}"
    );
    f.compile("cheapActive($cap: 3000)[name]")
        .expect("the value the author means binds");

    // Comparisons are NOT typed slots: nothing is stored, so nothing needs
    // a mode, and `value > total / 3` compares exactly at the pinned scale.
    f.compile("orderItems(amount > 10000 / 3)[amount]")
        .expect("a comparison against a decimal needs no conversion");
}

/// Prints the emitted SQL for the two lowerings 9c names. Not an assertion
/// — the assertions above are values, deliberately, because this was a
/// wrong-answers bug and SQL text would not have caught it. This exists so
/// a reviewer can read what changed without running the compiler in their
/// head: `cargo test -p ordo-conformance --test decimal sql -- --nocapture`.
#[test]
fn sql_for_the_two_lowerings() {
    let f = Fixture::load().expect("fixture");
    for src in [
        "orderItems[per: amount / qty]",
        "orderItems[amount]\nreduce { a: avg amount }",
    ] {
        println!("--- {src}\n{}\n", f.compile(src).expect("compiles").sql);
    }
}

/// `allocate` parses and is refused by name. It is the settled money-split
/// primitive, and `invalid_allocate` is the class the closed inventory
/// already named for every site outside an `each` emission over a list
/// parameter — which, since `each` does not parse, is every site the
/// language can currently express. Before this, `allocate(…)` was an
/// unknown function and the class had no producer at all.
#[test]
fn allocate_is_refused_by_name_rather_than_as_an_unknown_function() {
    let f = Fixture::load().expect("fixture");
    let e = f
        .compile("orderItems[part: allocate(amount, qty)]")
        .expect_err("allocate has no legal home in expression position");
    assert!(e.contains("invalid_allocate"), "{e}");
    assert!(
        e.contains("each"),
        "the refusal must say where it WOULD be legal: {e}"
    );

    // Arity is still the parser's: the closed function list carries it.
    let e = f
        .compile("orderItems[part: allocate(amount)]")
        .expect_err("allocate takes a total and a weight vector");
    assert!(e.contains("parse"), "{e}");
}
