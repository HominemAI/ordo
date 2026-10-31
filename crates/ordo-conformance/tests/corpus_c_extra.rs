//! Composition corpus, the cases the executed set was missing
//! (`docs/composition-conformance.md` §4).
//!
//! `corpus.rs` runs C001–C010 and the C099–C110 rejects; `corpus_p0.rs`
//! runs the P0 harvest. This file adds the compile-tier cases neither
//! covered and that the compiler can already answer, keyed by their ids.
//!
//! The composition cases still missing are listed in `NOT_RUN` at the
//! bottom with the reason each is unreachable — several of them for a
//! reason worth stating once here: **`orders.memo` is sealed and the seed
//! carries empty payloads**, so every case built on `giftOrders`
//! (`orders(memo == "gift")`) has no rows to return and, more decisively,
//! no plan at all — a filter on a sealed field with no blind index does
//! not bind. That is `giftOrders`' whole job in the fixture.

use ordo_conformance::Fixture;
use ordo_plan::cost::classify;
use ordo_sema::bind_op_generic;

fn fixture() -> Fixture {
    Fixture::load().expect("the market fixture compiles")
}

/// **C052 — compile.** Single-plan assertion for C050's registration
/// artifact: composition is inlining, not a view and not a runtime lookup.
#[test]
fn c052_composition_compiles_to_exactly_one_statement() {
    let f = fixture();
    let p = f
        .compile_params(
            "toolDeals($cap: $cap, $root: $root)",
            &[("cap", "10000"), ("root", "\"cat_2\"")],
        )
        .expect("C052: toolDeals plans");

    // Exactly one statement: one terminator, at the end.
    assert_eq!(
        p.sql.matches(';').count(),
        1,
        "C052: one planned SQL statement, not a script:\n{}",
        p.sql
    );
    assert!(
        p.sql.trim_end().ends_with(';'),
        "C052: and the terminator is the end of it:\n{}",
        p.sql
    );
    // No view references and no runtime lookup — the callee names do not
    // survive into the plan at all, because there is nothing left of them
    // to reference.
    for callee in ["toolDeals", "cheapActive", "activeProducts"] {
        assert!(
            !p.sql.contains(callee),
            "C052: `{callee}` is inlined, so its NAME must not appear in the plan:\n{}",
            p.sql
        );
    }
    assert!(
        !p.sql.to_uppercase().contains("CREATE VIEW"),
        "C052: composition is inlining, never a view"
    );
    // Depth 3 really was traversed: the base entities of all three levels
    // are in the one statement.
    for base in ["\"products\"", "\"categories\""] {
        assert!(p.sql.contains(base), "C052: {base} is reached:\n{}", p.sql);
    }
}

/// **C089 — the half that can be asserted.** The document's case pairs a
/// blind-index classification against an `unbounded-host` one; only the
/// first half exists, because the second half's op does not bind (see
/// `NOT_RUN`). The half that does exist is worth its own assertion: the
/// digest comparison written near the base is an index probe, and the
/// class says so.
#[test]
fn c089_partial_a_digest_comparison_near_the_base_is_an_index_probe() {
    let f = fixture();
    let near_base = bind_op_generic(
        &f.schema,
        &f.registry,
        "customerByEmail",
        ordo_sema::Clock::CORPUS,
    )
    .expect("C089");
    assert_eq!(
        classify(&near_base).as_str(),
        "blind-index",
        "C089: written near the base, the digest comparison is an index probe"
    );
}

/// **C118 — compile + reject.** Composition cannot launder scope: a
/// callee's unprojected field is not reachable from the caller.
///
/// The document writes `op pub = customers[name]`; `customers` is
/// row-unbounded, so the registration gate wants the acknowledgment
/// before it looks at anything else. Added here, and recorded as a
/// corpus-text divergence in `NOT_RUN`'s sibling note.
#[test]
fn c118_composition_cannot_launder_scope() {
    let mut f = fixture();
    let prog = ordo_syntax::parse_program(
        "op pub unbounded = customers[name]\n\nop pubNames unbounded = pub[name]",
    )
    .expect("C118: parses");
    f.registry.extend_from(&prog);
    ordo_plan::cost::check_registration(&f.schema, &f.registry).expect("C118: registers");

    // Both accept, and neither can see `email`: the callee's output shape
    // is all composition sees.
    let pub_q = bind_op_generic(&f.schema, &f.registry, "pub", ordo_sema::Clock::CORPUS)
        .expect("C118: `pub` binds");
    let names: Vec<&str> = pub_q.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["name"], "C118: `pub`'s shape excludes `email`");
    let names_q = bind_op_generic(&f.schema, &f.registry, "pubNames", ordo_sema::Clock::CORPUS)
        .expect("C118: binds");
    let names: Vec<&str> = names_q.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["name"], "C118: and so does `pubNames`'");

    // And the final query is refused: a condition on a field the callee
    // did not output is not a condition the caller may write.
    let e = f
        .compile("pub(email == \"ada@example.com\")[name]")
        .expect_err("C118: `email` is not in `pub`'s output shape");
    assert!(e.contains("not_output_field"), "C118: {e}");
}

/// **C023, C025, C026, C028, C029 — reject.** Binding rules: a required
/// parameter must be bound, a bound value must have the declared type, a
/// parameter may be bound once, and a binding is row-free (R-C3).
#[test]
fn c023_c025_c026_c028_c029_binding_rejects() {
    let f = fixture();
    for (case, src, class) in [
        // C023 — required param unbound (`$from`).
        (
            "C023",
            "doneOrders(total > 0)[total]",
            "unbound_required_param",
        ),
        // C025 — int literal bound to a date param.
        (
            "C025",
            "recentOrders($since: 5)[placed]",
            "param_type_mismatch",
        ),
        // C026 — a string is not a date literal (a bare ISO date is).
        (
            "C026",
            "doneOrders($from: \"2026-01-01\")[total]",
            "param_type_mismatch",
        ),
        // C028 — the same param bound twice.
        (
            "C028",
            "doneOrders($from: 2026-01-01, $from: 2026-02-01)[total]",
            "duplicate_binding",
        ),
        // C029 — a field reference in a binding: bindings are row-free.
        (
            "C029",
            "doneOrders($from: placed)[total]",
            "invalid_binding_expr",
        ),
    ] {
        let e = f
            .compile(src)
            .expect_err(&format!("{case}: `{src}` must be rejected"));
        assert!(
            e.contains(class),
            "{case}: `{src}` was rejected as:\n  {e}\nexpected class: {class}"
        );
    }
}

/// **C040, C041 — reject.** A condition may only name the callee's output
/// fields: a field the callee filtered on internally (C040) and a base
/// field it did not project (C041) are both `not_output_field`.
#[test]
fn c040_and_c041_conditions_reach_only_output_fields() {
    let f = fixture();
    for (case, src) in [
        (
            "C040",
            "doneOrders($from: 2026-01-01, status == \"done\")[total]",
        ),
        ("C041", "activeProducts(active == true)[name]"),
    ] {
        let e = f
            .compile(src)
            .expect_err(&format!("{case}: `{src}` must be rejected"));
        assert!(e.contains("not_output_field"), "{case}: {e}");
    }
}

/// **C085 — compile (cost half).** Composite cost: a caller over a
/// blind-index callee inherits the callee's class.
///
/// The manifest half (`customers.{email, tier}`, `name` pruned) has no
/// surface to assert against — see `NOT_RUN`'s C116 entry.
#[test]
fn c085_partial_composite_cost_inherits_the_callee_class() {
    let mut f = fixture();
    let prog = ordo_syntax::parse_program("op tierOf($e: text) = customerByEmail($e: $e)[tier]")
        .expect("C085: parses");
    f.registry.extend_from(&prog);
    ordo_plan::cost::check_registration(&f.schema, &f.registry).expect("C085: registers");
    let q = bind_op_generic(&f.schema, &f.registry, "tierOf", ordo_sema::Clock::CORPUS)
        .expect("C085: binds");
    assert_eq!(classify(&q).as_str(), "blind-index", "C085");
}

/// The cases this file runs in full — every assertion the document states
/// for them. `corpus.rs`'s coverage report reads this list.
const EXECUTED: &[&str] = &[
    "C023", "C025", "C026", "C028", "C029", "C040", "C041", "C052", "C118",
];

/// Cases where one half of the document's assertion is reachable and the
/// other is not. Counted apart from `EXECUTED`, because a half-run case is
/// not a covered case.
const PARTIAL: &[(&str, &str)] = &[
    (
        "C085",
        "the cost class is asserted; the pruned per-field manifest has no surface",
    ),
    (
        "C089",
        "the `blind-index` half is asserted; the `unbounded-host` contrast does not bind",
    ),
];

/// The composition cases still out of reach, each with its reason.
/// Printed, not silent: a corpus whose gaps are unstated reads as complete.
const NOT_RUN: &[(&str, &str)] = &[
    (
        "C089",
        "the contrast half does not bind: `custEmails(email == $e)` compares a sealed \
              OUTPUT field host-side and is refused `sealed_unsupported`, so there is no \
              `unbounded-host` classification to set against `customerByEmail`'s \
              `blind-index` one. The blind-index half is asserted above",
    ),
    (
        "C070",
        "`giftOrders` filters `orders.memo`, a sealed field with no blind index — \
              the op does not bind, by design (the fixture ships it as the refusal case)",
    ),
    ("C073", "same: the append's left side is `giftOrders`"),
    ("C074", "same"),
    (
        "C081",
        "same: the second composed many-side is `giftOrders`",
    ),
    ("C082", "same: the right-hand `let` sources `giftOrders`"),
    ("C087", "same"),
    (
        "C088",
        "same shape: `orderMemos(memo == \"rush\")` compares a sealed field; and the \
              seed carries empty sealed payloads, so there is no `rush` order to return",
    ),
    (
        "C091",
        "same: `memo contains \"ru\"` over empty sealed payloads",
    ),
    (
        "C090",
        "`sealed_keyset_pagination` has no producer — `OpSpec` carries no sort key and \
              the host marks every output field open (checklist 9q)",
    ),
    (
        "C095",
        "incremental registration and the deploy diff are unbuilt (9m): the registry is \
              built from one source text in one shot, so nothing is a previously-registered \
              dependent",
    ),
    (
        "C096",
        "same — the version-hash cascade needs two registrations",
    ),
    ("C097", "same"),
    (
        "C111",
        "`?live` invalidation is `ordo-host`'s surface; this crate cannot depend on it",
    ),
    ("C112", "same"),
    (
        "C113",
        "the `cursor: drifting` / `cursor: stable` label (R-C18) exists in no crate — \
              nothing in the artifact, the catalog entry or the descriptor carries it",
    ),
    (
        "C114",
        "same, plus the behavioral drift sequence needs the live/paging runtime",
    ),
    ("C115", "same"),
    ("C119", "same"),
    ("C120", "same"),
    (
        "C116",
        "the per-field scope manifest (`products.{active, price, name, category}`) does \
              not exist: `ordo_plan::op_artifact` and `ordo_host::manifest` both list \
              ENTITIES, never the fields touched",
    ),
    (
        "C117",
        "same, and the `categories.(tree structure)` entry with it",
    ),
];

#[test]
fn composition_gaps_are_named() {
    let src = include_str!("corpus_c_extra.rs");
    for id in EXECUTED {
        assert!(
            src.matches(id).count() >= 2,
            "{id} is claimed as executed but appears in no case here"
        );
    }
    for (id, reason) in PARTIAL.iter().chain(NOT_RUN.iter()) {
        assert!(
            !EXECUTED.contains(id),
            "{id} cannot be both executed and not"
        );
        assert!(
            !reason.trim().is_empty(),
            "{id}: the claim carries its reason"
        );
    }
    println!(
        "this file: {} composition cases executed, {} partial, {} unreachable",
        EXECUTED.len(),
        PARTIAL.len(),
        NOT_RUN.len()
    );
    for (id, reason) in PARTIAL.iter().chain(NOT_RUN.iter()) {
        println!("  {id}: {reason}");
    }
}
