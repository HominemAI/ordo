//! Execution tier for the aggregates added in 9f: `percentile`, `distinct
//! <field>`, and `joined`.
//!
//! These are not corpus cases — the corpus states no rows for them — so
//! nothing here asserts a value this author chose. Each case instead asserts
//! an IDENTITY the definition language states, between two spellings the
//! language already has, evaluated by a real Postgres:
//!
//! * `median x` is `percentile(x, 0.5)` (DL: median is the p = 0.5 case);
//! * `distinct x` counts what W039's `distinct` + `reduce` composition counts;
//! * `joined`'s concatenation order is the pipeline's sort, so reversing the
//!   sort reverses the parts.
//!
//! An identity cannot be satisfied by a wrong-but-consistent answer in the
//! way a hand-written expected row can, and it needs no seed data pinned into
//! this file.

use ordo_conformance::{Fixture, Pg};
use serde_json::Value;

fn seeded() -> (Fixture, Pg) {
    let f = Fixture::load().expect("the market fixture compiles");
    let pg = Pg::named("ordo_conf_aggregates");
    f.reset_once(&pg).expect("fixture DDL applies and seeds");
    (f, pg)
}

fn run(f: &Fixture, pg: &Pg, src: &str) -> Value {
    f.run_case(pg, src).unwrap_or_else(|e| panic!("{src}\n{e}"))
}

/// `median` is `percentile(x, 0.5)`, per group and over the whole set. If the
/// new aggregate lowered to anything else — a discrete percentile, a wrong
/// fraction, a different ORDER BY — the two would part company here.
#[test]
fn percentile_at_one_half_is_median() {
    let (f, pg) = seeded();
    for (median, percentile) in [
        (
            "products[price, vendor]\ngroup vendor { m: median price }\nsort m",
            "products[price, vendor]\ngroup vendor { m: percentile(price, 0.5) }\nsort m",
        ),
        (
            "orders[total]\nreduce { m: median total }",
            "orders[total]\nreduce { m: percentile(total, 0.5) }",
        ),
    ] {
        assert_eq!(run(&f, &pg, median), run(&f, &pg, percentile), "{median}");
    }

    // A percentile that is not the median must not silently be one: the
    // fixture's prices are spread, so p10 and p90 differ from each other.
    let lo = run(
        &f,
        &pg,
        "products[price]\nreduce { p: percentile(price, 0.1) }",
    );
    let hi = run(
        &f,
        &pg,
        "products[price]\nreduce { p: percentile(price, 0.9) }",
    );
    assert_ne!(lo, hi, "every fraction produced the same value");
}

/// W039 rules that counting a distinct SET composes: `distinct` (the step)
/// then `reduce { n: count }`. `distinct <field>` is the aggregate spelling of
/// the same number, so the two must agree — on a column that genuinely
/// repeats, or the test would pass on any implementation.
#[test]
fn distinct_field_agrees_with_the_composed_count() {
    let (f, pg) = seeded();
    let composed = run(&f, &pg, "orders[customer]\ndistinct\nreduce { n: count }");
    let aggregate = run(&f, &pg, "orders[customer]\nreduce { n: distinct customer }");
    assert_eq!(composed, aggregate);

    // …and it is a smaller number than the row count, so the column really
    // does repeat and the equality above has content.
    let rows = run(&f, &pg, "orders[customer]\nreduce { n: count }");
    assert_ne!(
        rows, aggregate,
        "no duplicate customers in the fixture — this case proves nothing"
    );
}

/// DL pins `joined`'s order to the pipeline's current sort. Reversing the sort
/// must reverse the parts; an unordered `string_agg` would return whatever the
/// executor produced and would not reliably do that.
#[test]
fn joined_concatenates_in_the_pipelines_order() {
    let (f, pg) = seeded();
    let text = |src: &str| match run(&f, &pg, src) {
        Value::Array(rows) => rows
            .iter()
            .map(|r| r["names"].as_str().expect("text").to_string())
            .collect::<Vec<_>>(),
        other => panic!("expected rows, got {other}"),
    };

    let asc = text(
        "products[vendor, name]\nsort name\ngroup vendor { names: joined(name, \", \") }\nsort names",
    );
    let desc = text(
        "products[vendor, name]\nsort -name\ngroup vendor { names: joined(name, \", \") }\nsort names",
    );

    // Same buckets, same members, opposite order within each.
    let mut reversed: Vec<String> = asc
        .iter()
        .map(|s| {
            let mut parts: Vec<&str> = s.split(", ").collect();
            parts.reverse();
            parts.join(", ")
        })
        .collect();
    reversed.sort();
    let mut desc_sorted = desc.clone();
    desc_sorted.sort();
    assert_eq!(reversed, desc_sorted, "asc={asc:?} desc={desc:?}");
    assert!(
        asc.iter().any(|s| s.contains(", ")),
        "no bucket has two members — the ordering claim is untested"
    );
}
