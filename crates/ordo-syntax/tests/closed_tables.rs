//! The two closed tables that grew a second projection, pinned to the parser
//! that is their only other reader.
//!
//! `editor_grammar.rs` pins the *names* of five closed sets against their
//! producers, because a highlighter needs names and nothing else. `ordo-gen`
//! needs more than names: to generate a typed builder it has to know that
//! `round` takes one argument or two, that `count` takes none, and that
//! `percentile`'s second argument is a literal. Those facts already existed —
//! `FUNCTIONS`' arity columns and `agg_decl`'s match — and the honest way to
//! hand them to a generator is to project them from the one table rather than
//! let the generator carry a copy.
//!
//! A projection can still be a lie: `AGGREGATE_SHAPES` could name a shape
//! `agg_decl` does not parse, and `functions()` could report an arity the
//! parser does not enforce. So this file drives the *parser* with what each
//! table claims, in both directions — the arity the table gives must parse,
//! and one argument either side of it must not.

use ordo_syntax::{
    functions, parse_expr, parse_pipeline, AggShape, AGGREGATE_SHAPES, ALLOCATE_MODES,
};

/// The probe argument for position `i` of `name`.
///
/// `1` everywhere except `allocate`'s third, which is **not an expression**:
/// it is a bare mode word from `ALLOCATE_MODES`, checked by the parser at the
/// written form. That is a fact `(name, min, max)` cannot carry and a
/// generator therefore must not read off it — `ordo-gen` spells that argument
/// as a closed union of the mode names rather than an expression, and this is
/// where the exception is written down.
fn probe_arg(name: &str, i: usize) -> &'static str {
    if name == "allocate" && i == 2 {
        ALLOCATE_MODES[0]
    } else {
        "1"
    }
}

/// `functions()` is `FUNCTIONS`, arities included: the declared range parses
/// and everything outside it is refused by the parser's own arity check.
#[test]
fn function_arities_are_the_parser_s() {
    for (name, min, max) in functions() {
        for n in 0..=(max + 1) {
            let args = (0..n)
                .map(|i| probe_arg(name, i))
                .collect::<Vec<_>>()
                .join(", ");
            let src = format!("{name}({args})");
            let parsed = parse_expr(&src).is_ok();
            let in_range = n >= min && n <= max;
            assert_eq!(
                parsed,
                in_range,
                "`{src}`: the table says {name} takes {min}..={max} argument(s), \
                 but the parser {} it. One of the two is wrong, and the table is \
                 what `ordo-gen` generates the CDK's `fn.{name}(…)` signature from.",
                if parsed { "accepted" } else { "refused" },
            );
        }
    }
}

/// Every function name the table gives really is a function, and an invented
/// one is not — the non-vacuity half. Without it a table that had gone empty
/// would pass the test above by iterating nothing.
#[test]
fn the_function_table_is_not_empty_and_not_open() {
    assert!(functions().count() >= 10, "the function table went empty");
    assert!(
        parse_expr("notAFunction(1)").is_err(),
        "expression functions are a closed set; an unknown name must be refused"
    );
}

/// A pipeline with one `reduce { out: <agg …> }` — the smallest context an
/// aggregate is legal in.
fn reduce_with(agg: &str) -> bool {
    parse_pipeline(&format!("orders[total]\nreduce {{ out: {agg} }}")).is_ok()
}

/// `AGGREGATE_SHAPES` describes what `agg_decl` accepts, checked by making it
/// accept and refuse.
#[test]
fn aggregate_shapes_are_what_agg_decl_parses() {
    for &(name, shape) in AGGREGATE_SHAPES {
        // The shape's own spelling must parse.
        let legal: &[String] = &match shape {
            AggShape::NoneOrCondition => {
                vec![name.to_string(), format!("{name}(total > 5000)")]
            }
            AggShape::OneExpr => vec![format!("{name} total")],
            AggShape::ExprAndFraction => vec![format!("{name}(total, 0.5)")],
            AggShape::ExprAndSeparator => vec![format!("{name}(total, \", \")")],
        };
        for spelling in legal {
            assert!(
                reduce_with(spelling),
                "AGGREGATE_SHAPES says `{name}` is {shape:?}, so `{spelling}` must \
                 parse — the CDK generates its `agg.{name}(…)` signature from this"
            );
        }

        // And the shapes it is NOT must be refused, which is what stops the
        // table from being a description of nothing.
        let illegal: Vec<String> = match shape {
            // `count total` would read `total` as an argument it does not
            // take; the parser stops at the next token.
            AggShape::NoneOrCondition => vec![format!("{name}(total, 0.5)")],
            AggShape::OneExpr => vec![format!("{name}(total, 0.5)")],
            AggShape::ExprAndFraction => {
                // A fraction outside (0, 1) is the parse-time check DL pins.
                vec![format!("{name}(total, 1.0)"), format!("{name} total")]
            }
            AggShape::ExprAndSeparator => {
                // The separator is a literal, never a field.
                vec![format!("{name}(total, sep)"), format!("{name} total")]
            }
        };
        for spelling in &illegal {
            assert!(
                !reduce_with(spelling),
                "`{spelling}` parses, but AGGREGATE_SHAPES says `{name}` is \
                 {shape:?} — the table has drifted from `agg_decl`"
            );
        }
    }
}

/// The aggregate set is closed: a name outside the table is refused by name.
#[test]
fn an_unknown_aggregate_is_refused() {
    assert!(
        !reduce_with("stddev total"),
        "aggregates are a closed set; `stddev` is not in it"
    );
}
