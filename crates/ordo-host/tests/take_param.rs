//! Checklist 9e — **parameterized `take`** and **parameter bounds**, and
//! the join between them.
//!
//! DL "Other steps" [settled]:
//!
//! > `take $n` — legal: the count may be a parameter, and the parameter
//! > must carry a bound (`$n: int <= 200`) so the op's cost class is still
//! > statically provable. An unbounded parameter here is `unbounded_take`.
//!
//! # Why the two constructs are one test
//!
//! A bound is not a hint. `take $n` is the row-bound prover's evidence,
//! and with an unbounded parameter it proves exactly nothing — "the caller
//! receives as many rows as the caller asked for" is what *unbounded*
//! means. The declared bound restores the proof, and only because the DOOR
//! refuses a value above it before planning (DL §2: "Bounds are enforced at
//! the door, before planning").
//!
//! So there are two halves and neither stands alone:
//!
//! - **registration** — `unbounded_take` for a parameter with no bound, and
//!   `row_bound = <= the bound` for one with it;
//! - **the door** — `bound_violation` for a value above the bound.
//!
//! Half one without half two is a number nobody checks. Half two without
//! half one is a check on a number nobody used. This suite asserts both,
//! and asserts that the bound the gate proved against is the bound the door
//! enforces — the same declaration, read once.
//!
//! Real Postgres, this suite's own database (`ordo_9e_take`).

use ordo_api::spec::ParamTy;
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_host::build_specs;
use serde_json::json;

/// Ops of this suite's own, over the shipped fixture's `products`.
///
/// `topN` is the settled spelling; `topAny` is the refusal, kept out of
/// the registered set so the rest of the suite can register at all.
const OPS: &str = r#"
op topN($n: int <= 3) =
  products[name]
  sort name
  take $n

op topPerCategory($n: int <= 2) unbounded =
  products[name, category]
  sort name
  take[category] $n
"#;

fn fixture_with(extra: &str) -> Result<Fixture, String> {
    Fixture::from_source(&format!("{FIXTURE}\n{extra}"), SEED)
}

fn taking() -> Fixture {
    fixture_with(OPS).expect("a bounded `take $n` registers")
}

fn seeded() -> Pg {
    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_9e_take");
    let pg = Pg {
        db: "ordo_9e_take".into(),
        ..Pg::default()
    };
    ONCE.get_or_init(|| taking().reset(&pg))
        .clone()
        .expect("fixture DDL applies and seeds");
    pg
}

// ── registration: the gate ───────────────────────────────────────────────

/// `unbounded_take`, by the name DL gives it. Not `unacknowledged_unbounded`
/// — the op is not asking to scan the space, it is asking to return a
/// number nobody wrote down, and those are different mistakes with
/// different fixes.
#[test]
fn a_take_parameter_with_no_bound_is_unbounded_take() {
    let e = fixture_with("op topAny($n: int) =\n  products[name]\n  sort name\n  take $n\n")
        .err()
        .expect("an unbounded take parameter must not register");
    assert!(
        e.starts_with("unbounded_take:") || e.contains("unbounded_take"),
        "the class DL names, not a near miss: {e}"
    );
    assert!(
        e.contains("$n: int <= "),
        "and the message hands over the fix: {e}"
    );

    // The acknowledgment does NOT buy it: `unbounded` is the author saying
    // "this op's work grows with the space", and a take count nobody
    // bounded is not that claim — it is a missing number.
    let e =
        fixture_with("op topAny($n: int) unbounded =\n  products[name]\n  sort name\n  take $n\n")
            .err()
            .expect("`unbounded` acknowledges scanning, not an unwritten count");
    assert!(e.contains("unbounded_take"), "{e}");

    // The keyed spelling is the same construct and the same rule.
    let e = fixture_with(
        "op perCat($n: int) unbounded =\n  products[name, category]\n  sort name\n  take[category] $n\n",
    )
    .err()
    .expect("`take[keys] $n` is a take with a parameter count");
    assert!(e.contains("unbounded_take"), "{e}");
}

/// A take count counts rows. Any other declared type is a static mistake,
/// caught where the declaration is, not at the first call.
#[test]
fn a_take_parameter_must_be_an_int_and_must_be_declared() {
    let e = fixture_with("op topN($n: date <= 2026-01-01) =\n  products[name]\n  take $n\n")
        .err()
        .expect("a date does not count rows");
    assert!(e.contains("param_type_mismatch"), "{e}");

    let e = fixture_with("op topN($m: int <= 3) =\n  products[name]\n  take $n\n")
        .err()
        .expect("`$n` is not declared");
    assert!(e.contains("unknown_param"), "{e}");
}

/// The bound IS the row bound. This is the whole reason the bound is
/// mandatory, so it is asserted as a number rather than as "bounded".
#[test]
fn the_declared_bound_is_the_ops_row_bound() {
    let f = taking();
    let q = ordo_sema::bind_op_generic(&f.schema, &f.registry, "topN", ordo_sema::Clock::CORPUS)
        .expect("topN binds");
    assert_eq!(
        ordo_plan::cost::row_bound(&q, &f.schema),
        ordo_plan::cost::RowBound::AtMost(3),
        "`take $n` with `$n: int <= 3` proves three rows, not `take`'s absence"
    );

    // And the artifact says so, in digits, so loosening the bound shows up
    // in a deploy diff exactly as loosening a `take` would.
    let op = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, "topN")
        .expect("topN has an artifact");
    assert_eq!(op["cost"]["row_bound"], json!(3));
    assert_eq!(op["params"][0]["max"], json!("3"));

    // The count is a bind slot, never an interpolated number.
    let sql = ordo_plan::plan(&q, &f.plan_space())
        .expect("topN plans")
        .sql;
    assert!(
        sql.contains("LIMIT $"),
        "the count rides in a parameter slot:\n{sql}"
    );
}

// ── the door: the other half of the proof ────────────────────────────────

/// The declared bound reaches `ParamSpec`, which is what the door reads.
///
/// It was `None` there — `build_specs` hard-coded it — so `ParamSpec::bounds`
/// had **no producer at all** and every bound in the docs was a promise the
/// compiler could not make.
#[test]
fn the_declared_bound_reaches_the_door_and_is_enforced_there() {
    let f = taking();
    let specs = build_specs(&f);
    let top = specs.iter().find(|s| s.name == "topN").expect("topN");
    let n = top.params.iter().find(|p| p.name == "n").expect("$n");
    assert_eq!(n.ty, ParamTy::Int);
    let b = n
        .bounds
        .as_ref()
        .expect("the declared bound reached the door");
    assert_eq!(b.max.as_deref(), Some("3"));

    // Within the bound: bound.
    ordo_api::bind::bind_params(top, json!({ "n": 3 }).as_object().expect("obj"))
        .expect("the bound admits its own edge");

    // Above it: refused, before anything is planned.
    let e = ordo_api::bind::bind_params(top, json!({ "n": 4 }).as_object().expect("obj"))
        .expect_err("a value above the bound is refused at the door");
    assert_eq!(e.code, ordo_api::ErrorCode::BoundViolation);

    // A caller who could beat the bound could beat the row bound the gate
    // proved from it, so this is the assertion that makes
    // `the_declared_bound_is_the_ops_row_bound` mean anything.
    let e = ordo_api::bind::bind_params(top, json!({ "n": 1000000 }).as_object().expect("obj"))
        .expect_err("and not just by one");
    assert_eq!(e.code, ordo_api::ErrorCode::BoundViolation);
}

/// The implicit floor, and the ruling behind it (see `param_bounds` in
/// `ordo-host`): a parameter count must not be able to say what a literal
/// count cannot, and `take -1` does not parse.
#[test]
fn a_negative_take_count_is_refused_at_the_door() {
    let f = taking();
    let specs = build_specs(&f);
    let top = specs.iter().find(|s| s.name == "topN").expect("topN");

    let e = ordo_api::bind::bind_params(top, json!({ "n": -1 }).as_object().expect("obj"))
        .expect_err("`LIMIT -1` is a Postgres error, and the door is where it stops");
    assert_eq!(e.code, ordo_api::ErrorCode::BoundViolation);

    // Zero is legal — `take 0` parses, and "no rows" is an answer.
    ordo_api::bind::bind_params(top, json!({ "n": 0 }).as_object().expect("obj"))
        .expect("zero is a row count");

    // A parameter that is NOT a take count gets no floor invented for it:
    // the ruling is derived from the construct, not applied to every int.
    let cheap = specs
        .iter()
        .find(|s| s.name == "cheapActive")
        .expect("cheapActive");
    let cap = cheap.params.iter().find(|p| p.name == "cap").expect("$cap");
    assert!(
        cap.bounds.is_none(),
        "an ordinary money parameter carries no bound it never declared"
    );
}

// ── execution ────────────────────────────────────────────────────────────

/// Against real Postgres: the count the caller supplies is the count of
/// rows that come back, and `take $n` with `$n = k` answers exactly what
/// `take k` answers.
#[test]
fn the_supplied_count_is_the_count_of_rows_returned() {
    let f = taking();
    let pg = seeded();

    for n in [0, 1, 2, 3] {
        let got = f
            .run_case_as(&pg, &format!("topN($n: {n})"), &[], None)
            .unwrap_or_else(|e| panic!("topN($n: {n}) did not run: {e}"));
        assert_eq!(
            got.as_array().expect("rows").len(),
            n,
            "`take $n` returned the wrong number of rows for n = {n}"
        );
    }

    // Identity against the literal spelling, which is the claim that
    // matters: parameterizing the count changes who writes the number and
    // nothing else.
    let by_param = f
        .run_case_as(&pg, "topN($n: 2)", &[], None)
        .expect("the parameterized form runs");
    let by_literal = f
        .run_case_as(&pg, "products[name]\nsort name\ntake 2", &[], None)
        .expect("the literal form runs");
    assert_eq!(by_param, by_literal);
}

/// The slot form is valid SQL, proved by making Postgres parse it.
///
/// The executing path folds `take $n` to the literal the caller supplied,
/// exactly as it folds every other parameter, so `LIMIT $1` only ever
/// appears in the INSPECTION plan — `ordo plan`, the artifact, the shape
/// gate. That is precisely why it is worth handing to a real server: a
/// plan nobody executes is a plan nobody would notice was ill-formed.
#[test]
fn postgres_accepts_the_unfolded_slot_form() {
    let f = taking();
    let pg = seeded();

    for op in ["topN", "topPerCategory"] {
        let q = ordo_sema::bind_op_generic(&f.schema, &f.registry, op, ordo_sema::Clock::CORPUS)
            .expect("binds");
        let plan = ordo_plan::plan(&q, &f.plan_space()).expect("plans");
        assert_eq!(
            plan.params,
            vec!["n".to_string()],
            "{op}: the count is the only slot"
        );
        // One `psql` invocation is one session, so the PREPARE and the
        // EXECUTE have to travel together — which is just as well: this
        // proves the slot form parses AND runs with a value bound to it,
        // rather than only that Postgres could read it.
        let sql = plan.sql.trim_end_matches(';');
        pg.exec(&format!("PREPARE probe AS {sql}; EXECUTE probe(2);"))
            .unwrap_or_else(|e| panic!("{op}: Postgres refused the slot form:\n{sql}\n{e}"));
    }
}

/// The keyed spelling executes too: `take[category] $n` is `row_number()
/// <= $n` per bucket, and the count is the same parameter.
#[test]
fn a_keyed_take_takes_the_supplied_count_from_each_bucket() {
    let f = taking();
    let pg = seeded();

    let one = f
        .run_case_as(&pg, "topPerCategory($n: 1)", &[], None)
        .expect("the keyed form runs");
    let two = f
        .run_case_as(&pg, "topPerCategory($n: 2)", &[], None)
        .expect("the keyed form runs");

    let cats = |v: &serde_json::Value| -> Vec<String> {
        let mut c: Vec<String> = v
            .as_array()
            .expect("rows")
            .iter()
            .map(|r| r["category"].as_str().unwrap_or("").to_string())
            .collect();
        c.sort();
        c.dedup();
        c
    };
    assert_eq!(
        one.as_array().expect("rows").len(),
        cats(&one).len(),
        "one row per category at n = 1"
    );
    assert!(
        two.as_array().expect("rows").len() > one.as_array().expect("rows").len(),
        "and more at n = 2 — the parameter is what the window compares against"
    );
}
