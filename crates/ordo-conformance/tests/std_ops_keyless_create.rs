//! Checklist 11-10a, the first defect the bulk import build found:
//! **`std.<entity>.create` refused every row of a keyless entity.**
//!
//! `std_ops` (`ordo-schema/src/lib.rs`) derives the create-once belief
//! (`rev: null`) into every entity's `create` braces, unconditionally. DL §6
//! rule 9 makes `rev` a statement about the ONE row the braces address by a
//! unique key, and a keyless entity's create braces address none — so
//! `ordo_plan::writes::set_mode` reads them as `SetMode::Create`, and the
//! write protocol (`ordo_conformance::write::plan_write_stmt`) refuses `rev`
//! there by name: `invalid_binding_expr`. Every call to a keyless entity's
//! derived `create` failed, unconditionally — `std.vendors.create` in this
//! fixture among them (`std_library.rs::the_derived_surface_is_exactly_what_the_shapes_imply`
//! already asserts `std.vendors.create` is REGISTERED; nothing before this
//! ever CALLED it).
//!
//! Two entities in the shipped fixture cover both cases without a bespoke
//! schema: `vendors` (no unique field, no `unique (...)` block — keyless)
//! and `categories` (`name: text unique` — keyed, and used by
//! `std_library.rs` to assert `std.categories.put` exists).

use ordo_conformance::{Fixture, Pg, WriteLog};

fn seeded(name: &str) -> (Fixture, Pg) {
    let f = Fixture::load().expect("market fixture compiles");
    let pg = Pg::named(name);
    f.reset(&pg).expect("fresh seed");
    (f, pg)
}

/// The fix: a keyless entity's derived `create` carries no `rev` at all, so
/// it is an ordinary insert — `SetMode::Create` with nothing for the write
/// protocol to refuse — every time it is called.
#[test]
fn a_keyless_entitys_derived_create_creates_a_row_every_call() {
    let (f, pg) = seeded("ordo_11_10a_keyless_create");
    let mut log = WriteLog::new();

    let r1 = f
        .run_write_as(
            &pg,
            &mut log,
            r#"std.vendors.create($name: "Acme Tools", $region: "west")"#,
            Some("mem_1"),
        )
        .unwrap_or_else(|e| {
            panic!("`std.vendors.create` on a keyless entity must create a row, not refuse: {e}")
        });
    assert_eq!(r1[0]["entity"], "vendors", "{r1}");
    assert!(r1[0]["created"].as_str().is_some(), "{r1}");

    // A second, identical call creates a SECOND row rather than refusing:
    // there is no key for a create-once belief to be about, so nothing
    // distinguishes this call from the first one to converge onto or
    // reject. This is the behavioural difference from the keyed case
    // below, not just a compile-time one.
    let r2 = f
        .run_write_as(
            &pg,
            &mut log,
            r#"std.vendors.create($name: "Acme Tools", $region: "west")"#,
            Some("mem_1"),
        )
        .expect("a second identical call still creates: nothing addresses the first row");
    assert_ne!(
        r1[0]["created"], r2[0]["created"],
        "two distinct rows, two distinct minted ids"
    );
}

/// The fix must not touch the keyed case: `categories.name` is unique, so
/// `std.categories.create` keeps its `rev: null` belief and a second call
/// under the same key is still `expected_absent`, not a silent converge —
/// which is the whole reason `create` and `put` are two different derived
/// ops (`std_ops`'s module doc).
#[test]
fn a_keyed_entitys_derived_create_still_refuses_a_duplicate() {
    let (f, pg) = seeded("ordo_11_10a_keyed_create_still_refuses");
    let mut log = WriteLog::new();

    f.run_write_as(
        &pg,
        &mut log,
        r#"std.categories.create($name: "gizmos")"#,
        Some("mem_1"),
    )
    .expect("the first row under this key creates");

    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"std.categories.create($name: "gizmos")"#,
            Some("mem_1"),
        )
        .expect_err("the create-once belief must survive this fix");
    assert!(
        e.starts_with("expected_absent:"),
        "a keyed entity's derived `create` must still refuse a duplicate \
         under its unique key, by the same class it always did: {e}"
    );
}
