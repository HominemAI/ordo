//! The request-time half of the cost gate, in the planner's own vocabulary:
//! `api-engine-design.md` §7.1 and `std-library.md` §2.1 rule 4, both
//! `[settled]`.
//!
//! > A registered cost class describes the op **as written**. A refinement
//! > changes the work the op actually does — a `where` on a sealed field
//! > turns a pushed-down `sql` op into a per-row host decrypt. So the
//! > registered class is not the served class, and nothing recomputed it.
//!
//! Three functions do the recomputing, and this suite is their arithmetic.
//! It lives beside `emit.rs`'s registration-gate tests deliberately: the
//! request-time gate reuses `CostClass`, `RowBound` and `row_bound` rather
//! than a parallel model, and the surest way to keep that true is to test
//! both against the same fixture.
//!
//! The sharp case, which §7.1 states and this suite pins:
//!
//! > Where the gate still bites is the shape that has no page window: an op
//! > whose result is statically one row — a `reduce`, and `std.E.count`
//! > under the list-manifest pairing — refines *before* the reduce, over the
//! > op's whole result set.

use ordo_plan::cost::{
    effective_class, refine_candidate_bound, refine_field_class, row_bound, CostClass, RowBound,
};
use ordo_schema::build;
use ordo_sema::{bind, Registry};
use ordo_syntax::{parse_pipeline, parse_program};

const SRC: &str = r#"
entity customers {
  name:  text
  email: text?  sealed indexed refinable unique normalize(lower)
  memo:  text = ""  sealed refinable
  spend: money
}

op recent =
  customers[name, spend]
  sort name
  take 5
"#;

fn q(src: &str) -> ordo_sema::ir::Query {
    let p = parse_program(SRC).expect("fixture parses");
    let schema = build(&p).expect("schema builds");
    let registry = Registry::from_program(&p);
    bind(
        &schema,
        &registry,
        &parse_pipeline(src).expect("parses"),
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds")
}

fn schema() -> ordo_schema::Schema {
    build(&parse_program(SRC).expect("parses")).expect("builds")
}

// ── the candidate set: what a post-everything refinement filters ─────────

/// A row-shaped plan's refinement lands after the `take` and after the page
/// window, so what it filters is the plan's own result — and the bound is
/// the plan's own row bound, read by the same prover registration used.
#[test]
fn a_row_shaped_plan_refines_over_its_own_result() {
    let s = schema();

    let bounded = q("customers[name, spend]\nsort name\ntake 5");
    assert_eq!(row_bound(&bounded, &s), RowBound::AtMost(5));
    assert_eq!(refine_candidate_bound(&bounded, &s), RowBound::AtMost(5));

    let addressed = q("customers(id == \"cus_1\")[name, memo]");
    assert_eq!(refine_candidate_bound(&addressed, &s), RowBound::AtMost(1));

    // A pageable plan is unbounded by the same prover. Such an op cannot
    // register without the `unbounded` acknowledgment, which is exactly why
    // the gate's exemption is not a hole: it is that acknowledgment.
    let pageable = q("customers[name, memo]");
    assert_eq!(refine_candidate_bound(&pageable, &s), RowBound::Unbounded);
}

/// **The one that matters.** A keyless reduce returns one row, so
/// `row_bound` proves `<= 1` and the op registers with no acknowledgment.
/// Its refinement is applied *before* the reduce, over everything the reduce
/// examines — and that set is bounded by nothing anybody checked.
///
/// One returned row over an unbounded examined set is the same confusion
/// `check_venue` calls out for `take` ("a `take` bounds what is RETURNED,
/// not what is EXAMINED"). Asking the bound question at the reduce's input
/// is what makes the request-time gate able to see it.
#[test]
fn a_reduce_refines_over_what_it_examines_not_over_its_one_row() {
    let s = schema();

    let counted = q("customers[spend]\nreduce { n: count }");
    assert_eq!(
        row_bound(&counted, &s),
        RowBound::AtMost(1),
        "the reduce returns one row, and registration is right to say so"
    );
    assert_eq!(
        refine_candidate_bound(&counted, &s),
        RowBound::Unbounded,
        "…but a pre-reduce condition is applied to every customer in the space"
    );

    // The input's own evidence still counts: a reduce over a bounded set is
    // bounded, and refusing it would be a false positive.
    let capped = q("recent[spend]\nreduce { n: count }");
    assert_eq!(refine_candidate_bound(&capped, &s), RowBound::AtMost(5));

    // A *keyed* group is not the one-row shape: the prover already answers
    // Unbounded for it, so registration already demanded the acknowledgment.
    let grouped = q("customers[name, spend]\ngroup name { total: sum spend }");
    assert_eq!(refine_candidate_bound(&grouped, &s), RowBound::Unbounded);
}

// ── the class one condition implies ──────────────────────────────────────

/// §7.1: "Sealed refinement fields raise the class, to `blind-index` for
/// equality that rides the index, to host-side for anything else." An open
/// field raises nothing, and that half is not decoration: a false positive
/// here refuses every refining caller in the deployment.
#[test]
fn only_sealed_fields_raise_the_class() {
    for candidate in [RowBound::AtMost(50), RowBound::Unbounded] {
        assert_eq!(
            refine_field_class(false, false, true, candidate),
            CostClass::Sql,
            "an open field is an ordinary predicate over rows the plan already produced"
        );
        assert_eq!(
            refine_field_class(true, true, true, candidate),
            CostClass::BlindIndex,
            "`sealed indexed` equality is one digest compare, in the database"
        );
    }

    // No index: every candidate payload is opened host-side, and how many
    // there are is the candidate set's question.
    assert_eq!(
        refine_field_class(true, false, true, RowBound::AtMost(50)),
        CostClass::BoundedHost
    );
    assert_eq!(
        refine_field_class(true, false, true, RowBound::Unbounded),
        CostClass::UnboundedHost
    );

    // An ordering comparison over sealed values is host work whatever the
    // index says — the index answers "same digest?" and nothing else. The
    // door refuses these outright (§2.1 rule 2); the cost model does not
    // rely on that to be honest.
    assert_eq!(
        refine_field_class(true, true, false, RowBound::Unbounded),
        CostClass::UnboundedHost
    );
}

// ── the join ─────────────────────────────────────────────────────────────

/// `effective = registered ⊔ class_of(…)`. The lattice is a chain, so the
/// join is the maximum — commutative, associative, idempotent, and never
/// cheaper than either side. Asserted over the whole cross product rather
/// than at three points, because "the join went the wrong way once" is the
/// bug that would serve unacknowledged host work under a `sql` label.
#[test]
fn the_join_is_the_maximum_of_the_lattice() {
    for &a in CostClass::ALL {
        assert_eq!(a.join(a), a);
        assert_eq!(CostClass::from_class(a.as_str()), Some(a));
        for &b in CostClass::ALL {
            assert_eq!(a.join(b), b.join(a));
            assert!(a.join(b).rank() >= a.rank());
            assert!(a.join(b).rank() >= b.rank());
            for &c in CostClass::ALL {
                assert_eq!(a.join(b).join(c), a.join(b.join(c)));
            }
        }
    }
    assert_eq!(
        effective_class(CostClass::Sql, []),
        CostClass::Sql,
        "no conditions, no change: an unrefined request is served at its registered class"
    );
}

// ── the two halves of the settled sentence ───────────────────────────────

/// §7.1 end to end, in the vocabulary the planner owns: the same sealed
/// condition on the same field is `bounded-host` under a `take` and
/// `unbounded-host` under a `count`, and only the second one is a class the
/// op never registered for.
#[test]
fn the_count_shape_is_what_the_rejection_protects() {
    let s = schema();

    let counted = q("customers[spend]\nreduce { n: count }");
    let escalated = refine_field_class(true, false, true, refine_candidate_bound(&counted, &s));
    assert_eq!(
        effective_class(CostClass::Sql, [escalated]),
        CostClass::UnboundedHost,
        "a sealed condition under a reduce is whole-entity host decryption"
    );

    // The same condition on the row-shaped op it pairs with: refinement is
    // post-take, so the venue rises and the cardinality does not. Serving
    // this is correct, and refusing it would break every refining caller.
    let listed = q("customers[name, memo]\nsort name\ntake 5");
    let ordinary = refine_field_class(true, false, true, refine_candidate_bound(&listed, &s));
    assert_eq!(
        effective_class(CostClass::BoundedHost, [ordinary]),
        CostClass::BoundedHost
    );

    // And an open condition on the *reduce* stays `sql`. This is the false
    // positive that would have been easiest to ship: the candidate set is
    // unbounded, but an open predicate pushes down into the same statement
    // and costs the venue nothing.
    let open = refine_field_class(false, false, true, refine_candidate_bound(&counted, &s));
    assert_eq!(effective_class(CostClass::Sql, [open]), CostClass::Sql);
}
