//! 9f‴ — a guard comparison's literal is type-checked against the column.
//!
//! Found while verifying something else entirely: `guard_atom_cond` was the
//! one place in the binder that built a comparison **without ever consulting
//! what it compares against**. The literal was wrapped raw, with
//! `lit_ty(...).unwrap_or(Text)`, and the column's type sat unused on the
//! line above.
//!
//! The consequence was not cosmetic. `qty == "seven"` on an `int` field
//! registered, planned, and emitted `"t0"."qty" = 'seven'`, so a real server
//! answered `invalid input syntax for type bigint` on **every read** — a
//! registered op that looks healthy and fails at request time, on the
//! authorization path, which is the worst place for a guard that cannot be
//! evaluated.
//!
//! Compile-only: the whole point is that this is now refused before a
//! statement exists.

mod gc_support;

use gc_support::schema_with;

/// Build the schema, then BIND a read over the entity — because that is when
/// a guard is compiled. Schema construction alone does not touch
/// `guard_atom_cond`, which is part of why this survived: the entity looks
/// fine right up until somebody queries it.
fn bind_over(extra: &str, source: &str) -> Result<(), String> {
    let schema = schema_with(extra)?;
    let pl = ordo_syntax::parse_pipeline(source)
        .map_err(|e| format!("{}: {} at {}", e.code, e.message, e.span))?;
    ordo_sema::bind_as(
        &schema,
        &ordo_sema::Registry::default(),
        &pl,
        &[],
        Some("mem_1"),
        ordo_sema::Clock::CORPUS,
    )
    .map(|_| ())
    .map_err(|e| format!("{}: {}", e.code, e.message))
}

/// An entity whose guard compares an `int` column against a text literal.
///
/// The comparison sits beside a `$caller` term because G2 refuses a guard
/// that never consults the caller — a hidden filter is not authorization. So
/// the realistic shape of this bug was always a well-formed guard with one
/// ill-typed conjunct, which is exactly why it went unnoticed: the guard
/// looks like authorization and is.
fn bad_guard() -> String {
    r#"
entity widgets {
  qty:   int
  owner -> members
  access read   owner == $caller and qty == "seven"
  access write  owner == $caller
}
"#
    .to_string()
}

fn good_guard() -> String {
    r#"
entity widgets {
  qty:   int
  owner -> members
  access read   owner == $caller and qty == 7
  access write  owner == $caller
}
"#
    .to_string()
}

#[test]
fn a_text_literal_against_an_int_column_is_refused_by_class() {
    let e = bind_over(&bad_guard(), "widgets[qty]").expect_err(
        "a guard comparing an int column to a text literal must not build — \
         it used to, and then failed on every read",
    );
    assert!(
        e.contains("param_type_mismatch"),
        "refused by class, not by prose: {e}"
    );
    assert!(
        e.contains("qty"),
        "the message names the field, because on a guard the field is the \
         whole orientation: {e}"
    );
}

/// The refusal must be narrow. A well-typed guard is the overwhelmingly
/// common case and a false positive here would break every deployment's
/// authorization at once — strictly worse than the bug being fixed.
#[test]
fn a_well_typed_guard_still_builds() {
    bind_over(&good_guard(), "widgets[qty]").expect("an int literal against an int column is fine");
}

/// The conversions the language already admits are admitted here too, because
/// this reuses `fit_slot` rather than offering a second opinion. A
/// guard is not a place to be more permissive than a parameter slot, and two
/// type tables would eventually disagree about `money` — which is the one
/// every reader gets wrong.
#[test]
fn the_admitted_conversions_are_the_same_ones() {
    // `int` into `money`: money is integral minor units, and an int literal
    // is how you write one.
    bind_over(
        r#"
entity invoices {
  total: money
  owner -> members
  access read   owner == $caller and total == 2999
  access write  owner == $caller
}
"#,
        "invoices[total]",
    )
    .expect("an int literal against a money column is the admitted conversion");

    // …and a literal finer than the places, which would have to round to
    // fit. Bare `money` is `money(0)` (27ak), so `29.99` is two places too
    // many; on a `money(2)` column it is 2999 minor units, below.
    let e = bind_over(
        r#"
entity invoices {
  total: money
  owner -> members
  access read   owner == $caller and total == 29.99
  access write  owner == $caller
}
"#,
        "invoices[total]",
    )
    .expect_err("money(0) holds whole units — 29.99 would round");
    assert!(e.contains("implicit_rounding"), "{e}");
    assert!(
        e.contains("money(0)") && e.contains("places"),
        "the message names the places the slot holds: {e}"
    );

    // On a `money(2)` column the same literal is the price it looks like.
    bind_over(
        r#"
entity invoices {
  total: money(2)
  owner -> members
  access read   owner == $caller and total == 29.99
  access write  owner == $caller
}
"#,
        "invoices[total]",
    )
    .expect("29.99 against a money(2) column is 2999 cents");
}

/// The fixture's own guards must be unaffected — if this check were wrong
/// about any real guard, the whole corpus would stop building, which is the
/// loudest possible canary and the reason it is asserted rather than assumed.
#[test]
fn every_guard_in_the_fixture_still_builds() {
    bind_over("", "tasks[title]").expect("the market fixture's guards are all well typed");
}
