//! `since` anchors to execution time — and nothing supplied one.
//!
//! DL §5: *"`since` anchors to execution time — the same op returns different
//! rows tomorrow."* The engine folds the window at BIND time into an exact
//! date literal, so no `NOW()` reaches SQL and a plan stays reproducible.
//! That design was always right. What was missing is that **all eight
//! `Binder` construction sites hard-coded the conformance corpus's pinned
//! date**, including `bind_as` — the one the executor calls per request.
//!
//! So `since 30d` compiled to a window anchored to 2026-05-01 and widening by
//! a day, every day, in the one feature whose entire point is not being
//! stale. Invisible to the corpus, because the corpus pins that date on
//! purpose.
//!
//! The clock is now a **required argument**, not a defaulted field: a default
//! here is exactly the shape of the bug — the wrong answer, available without
//! anyone choosing it.

use ordo_conformance::Fixture;
use ordo_sema::Clock;

fn window_for(clock: Clock) -> String {
    let f = Fixture::load().expect("fixture");
    let pl = ordo_syntax::parse_pipeline("orders(placed since 30d)[total]").expect("parses");
    let q =
        ordo_sema::bind_as(&f.schema, &f.registry, &pl, &[], Some("mem_1"), clock).expect("binds");
    ordo_plan::plan(&q, &f.plan_space()).expect("plans").sql
}

/// The window moves with the clock. Under the frozen clock these two were the
/// same string, which is the whole bug in one assertion.
#[test]
fn the_window_follows_the_execution_clock() {
    let may = window_for(Clock::at(2026, 5, 1));
    let june = window_for(Clock::at(2026, 6, 1));
    assert_ne!(
        may, june,
        "a `since` window that does not move is not a window"
    );
    assert!(may.contains("2026-04-01"), "30 days before 1 May:\n{may}");
    assert!(
        june.contains("2026-05-02"),
        "30 days before 1 June:\n{june}"
    );
}

/// It is still folded to a literal — the reproducibility property that made
/// bind-time folding right in the first place. A `NOW()` in the plan would
/// make two runs of one op incomparable and defeat every golden here.
#[test]
fn the_window_is_a_literal_not_a_server_clock() {
    let sql = window_for(Clock::at(2026, 5, 1));
    let upper = sql.to_uppercase();
    assert!(
        !upper.contains("NOW()") && !upper.contains("CURRENT_DATE"),
        "the window must be folded, not delegated to the server:\n{sql}"
    );
    assert!(sql.contains("DATE '2026-04-01'"), "{sql}");
}

/// `Clock::now()` is a real date, not the pinned one. Asserted by shape
/// rather than by value — a test that hard-codes today's date fails tomorrow,
/// which is the same class of mistake as the bug being fixed.
#[test]
fn the_real_clock_is_not_the_corpus_pin() {
    let now = Clock::now();
    assert_ne!(
        now,
        Clock::CORPUS,
        "if these are equal the machine's clock is set to the corpus date, \
         which is possible but worth knowing about"
    );
    let (y, m, d) = now.0;
    assert!((2020..2200).contains(&y), "a plausible year: {y}");
    assert!((1..=12).contains(&m) && (1..=31).contains(&d), "{m}-{d}");
}

/// The corpus keeps its pin, and now says so at the call site instead of
/// inheriting it from a default nobody could see.
#[test]
fn the_corpus_pin_is_still_available_and_named() {
    assert_eq!(Clock::CORPUS, Clock::at(2026, 5, 1));
    assert!(window_for(Clock::CORPUS).contains("2026-04-01"));
}
