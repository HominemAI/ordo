//! Checklist 9f′ — `allocate`'s distribution rule, executed.
//!
//! **Read this before reading the tests.** `allocate` is two things, and only
//! one of them is owed:
//!
//! * its **semantics** — DL §3 *Allocation* is headed `[settled]`, and the
//!   paragraph that marks the syntax lean says so in as many words: *"The
//!   allocation semantics above are settled; the spelling lands with that
//!   construct."* The same rule is restated in `glossary.md` and `traps.md`.
//!   That is what [`ordo_plan::allocate`] implements and what this file pins.
//! * its **home** — `allocate` is legal only inside an `each` emission over a
//!   `list of { … }` parameter, and DL §1 marks both **`[lean]`**. Checklist
//!   9e′ ruled that a `[lean]` construct is **not owed**. So there is no Ordo
//!   program that can call this, by design, and nothing here writes one.
//!
//! Splitting the item that way is the whole finding. 9c declined to build the
//! algorithm because its home is unsettled; the correction is that the *rule*
//! is settled independently of the *site*, and holding an unbuilt rule
//! hostage to an unbuilt site loses the half that was pinned.
//!
//! ### What each test is for
//!
//! Nothing below asserts a number this author chose. Every case is either a
//! value the specification prints, a property the specification states, or an
//! identity between two independent implementations:
//!
//! 1. `the_spec_s_own_worked_example` — the one value three documents print.
//! 2. `parts_always_sum_to_the_total` — the guarantee the primitive exists
//!    for, over a wide sweep including negative totals and lopsided weights.
//! 3. `remainders_go_to_the_largest_fractions_ties_by_position` — that the
//!    distribution is *largest-remainder* and not merely *some* distribution
//!    that happens to sum, which is the bug a sum-only test cannot see.
//! 4. `agrees_with_postgres_over_the_same_inputs` — determinism across venues,
//!    against a real server. DL's determinism clause is "so both venues
//!    agree"; until now nothing checked that, because there was no second
//!    venue to check against.
//! 5. `has_no_legal_site_in_the_surface_language` — the other half, asserted
//!    rather than assumed.
//!
//! ### And the modes (9f⁗)
//!
//! `allocate(total, weights, <mode>)` names **which party absorbs the odd
//! minor unit** — a business rule, not a mathematical one. The tests below
//! treat a mode as what it is: a **priority order** over the parties, applied
//! to the same floor-then-hand-out-one-apiece algorithm. That framing is why
//! the guarantees are checked *for every mode* rather than for the default
//! and then assumed:
//!
//! 6. `every_mode_keeps_the_sum_and_moves_each_part_at_most_one_unit` — both
//!    structural invariants, the whole sweep, every mode. A mode that broke
//!    either would not be a mode.
//! 7. `each_mode_hands_the_units_to_the_queue_it_names` — that each mode is
//!    the order it claims, checked against an independently derived queue.
//! 8. `no_two_modes_are_the_same_function` — that four names are four
//!    functions, which a per-mode test passing cannot tell you.
//! 9. `agrees_with_postgres_over_the_same_inputs` — grown to run every mode,
//!    so the determinism clause is checked per mode against a real server.
//! 10. `the_mode_names_are_one_closed_set` — `ordo_syntax::ALLOCATE_MODES`
//!     against `ordo_plan::AllocateMode`, both directions.
//! 11. `an_unknown_mode_is_refused_at_compile_time` — including `rotation`,
//!     the one that is excluded by ruling rather than by omission.
//! 12. `a_part_too_large_for_the_type_is_the_ruled_arithmetic_error` — a
//!     panic found while widening this, on input no document forbids.

use ordo_conformance::{Fixture, Pg};
use ordo_plan::{allocate, allocate_at_places, allocate_with, AllocateMode};

/// The weight vectors and totals every property below is checked over.
///
/// One sweep, four modes, so a mode cannot be introduced with a friendlier
/// set of cases than the default was held to. Deliberately hostile to a
/// floor-plus-fixup that was only ever tried on friendly input: negative
/// totals (a refund split, where floor and truncation-toward-zero disagree),
/// zero weights, one huge weight against many tiny ones, and a total smaller
/// than the number of parties.
const WEIGHTINGS: &[&[i64]] = &[
    &[1, 1, 1],
    &[1, 2, 3],
    &[3, 1],
    &[0, 1, 0],
    &[1, 0, 0, 0, 0, 0, 1],
    &[1; 10],
    &[1, 1, 1, 1, 1, 1, 1],
    &[1_000_000, 1, 1],
    &[7, 11, 13, 17, 19, 23],
    &[5],
];

const TOTALS: &[i64] = &[
    0,
    1,
    2,
    7,
    99,
    100,
    10_000,
    10_001,
    -1,
    -7,
    -10_000,
    -10_001,
    999_999_999_999,
];

/// The floors and the normalized fractional numerators, **derived here** and
/// not reused from the implementation — every characterization below is an
/// identity between two independent statements of the rule, which is the
/// only kind of assertion that can catch a wrong one.
///
/// Returns `(floors, fracs, extra)`: `fracs` are normalized so that "largest"
/// compares fractional parts rather than signed leftovers when the weights
/// sum negative, and `extra` is how many minor units are left to hand out.
fn floors_and_fracs(total: i64, ws: &[i64]) -> (Vec<i128>, Vec<i128>, usize) {
    let sum: i128 = ws.iter().copied().map(i128::from).sum();
    let sign = if sum < 0 { -1i128 } else { 1 };
    let mut floors = Vec::new();
    let mut fracs = Vec::new();
    for w in ws {
        let num = i128::from(total) * i128::from(*w);
        let mut q = num / sum;
        if num % sum != 0 && ((num % sum < 0) != (sum < 0)) {
            q -= 1;
        }
        floors.push(q);
        fracs.push((num - q * sum) * sign);
    }
    let extra = (i128::from(total) - floors.iter().sum::<i128>()) as usize;
    (floors, fracs, extra)
}

/// Who a mode hands the leftover units to, stated as the **queue** the
/// documentation says it is, and then truncated to `extra`.
///
/// Every queue ends in the position itself: the tie-break DL §3 already
/// states ("by weight order", positional) generalised across the modes
/// rather than replaced by four of them.
fn expected_recipients(
    total: i64,
    ws: &[i64],
    mode: AllocateMode,
) -> std::collections::BTreeSet<usize> {
    let (_, fracs, extra) = floors_and_fracs(total, ws);
    let mut q: Vec<usize> = (0..ws.len()).collect();
    match mode {
        AllocateMode::LargestRemainder => {
            q.sort_by(|a, b| fracs[*b].cmp(&fracs[*a]).then(a.cmp(b)))
        }
        AllocateMode::First => {}
        AllocateMode::Last => q.reverse(),
        AllocateMode::LargestShare => {
            let mag = |i: usize| i128::from(ws[i]).abs();
            q.sort_by(|a, b| mag(*b).cmp(&mag(*a)).then(a.cmp(b)));
        }
    }
    q.into_iter().take(extra).collect()
}

// ── 1 · the value the documents print ────────────────────────────────────

/// `allocate(100.00, (1, 1, 1))` is 33.34 / 33.33 / 33.33 — DL §2 (*Splitting
/// money is `allocate`, not `/`*), DL §3 (*Allocation*), `glossary.md` and
/// `traps.md`, four printings of the same three numbers.
///
/// It is also the case that **disambiguates the rule's one ambiguous
/// phrase**. "Ties broken by weight order" could mean positional order or a
/// sort by weight value; here all three weights are equal *and* all three
/// remainders tie, so a sort by value leaves the extra cent's home
/// undetermined — and the clause exists to be deterministic. Only the
/// positional reading gives this example one answer, and it is the answer
/// printed. If the implementation ever handed the cent to the last element
/// instead, everything else in this file would still pass.
#[test]
fn the_spec_s_own_worked_example() {
    // Minor units: 100.00 is 10_000 cents.
    assert_eq!(
        allocate(10_000, &[1, 1, 1]).unwrap(),
        vec![3_334, 3_333, 3_333]
    );
    // …and the failure it exists to prevent, stated so the point is on the
    // page: three independent roundings lose a cent.
    assert_eq!(3_333 * 3, 9_999);
}

// ── 2 · the guarantee ────────────────────────────────────────────────────

/// **Parts sum to the total exactly** — "guarantees the parts sum to exactly
/// `total`" (DL §3), "sum to `total` by construction" (`traps.md`).
///
/// The sweep is deliberately hostile to a floor-plus-fixup that was only ever
/// tried on friendly input: negative totals (a refund split, where floor and
/// truncation-toward-zero disagree and a `/`-based implementation silently
/// loses units), zero weights, one huge weight against many tiny ones, and a
/// total smaller than the number of parties.
#[test]
fn parts_always_sum_to_the_total() {
    for ws in WEIGHTINGS {
        for total in TOTALS.iter().copied() {
            let parts = allocate(total, ws).unwrap_or_else(|e| panic!("{total} / {ws:?}: {e}"));
            assert_eq!(parts.len(), ws.len(), "one part per weight");
            assert_eq!(
                parts.iter().sum::<i64>(),
                total,
                "allocate({total}, {ws:?}) = {parts:?} does not sum to its total"
            );
        }
    }

    // `int` and `money` are both a bigint count of minor units, so the
    // extremes of the type are ordinary input and not an edge case to be
    // excluded. Every intermediate is i128 precisely so this holds.
    for total in [i64::MAX, i64::MIN, i64::MAX - 1] {
        let parts = allocate(total, &[1, 1, 1]).expect("the type's own extremes");
        assert_eq!(
            parts.iter().copied().map(i128::from).sum::<i128>(),
            i128::from(total)
        );
    }
}

/// A weight vector summing to zero — the empty list included — is a division
/// by zero, and DL §2 rules that "a typed runtime error, uniform with
/// `mod(a, 0)`". Checklist 9w ruled the class: `arithmetic_error`.
///
/// No class is minted for it. Asserted here so the refusal cannot quietly
/// become a panic, a wrapped value, or an all-zero split that silently fails
/// the sum-to-total guarantee this file's other test asserts.
#[test]
fn a_zero_weight_sum_is_the_ruled_division_by_zero() {
    for ws in [vec![], vec![0], vec![0, 0, 0], vec![2, -2]] {
        let e = allocate(100, &ws).expect_err(&format!("{ws:?} sums to zero"));
        assert!(
            e.starts_with("arithmetic_error:"),
            "the class is 9w's, not a new one: {e}"
        );
    }
}

// ── 3 · largest-remainder, not merely "some" distribution ────────────────

/// The distribution is **largest-remainder with ties by position**, which is
/// a stronger claim than "the parts sum".
///
/// Checked against an independent statement of the rule: compute the exact
/// share of each element as a rational, take the floor, and confirm that the
/// elements which received an extra unit are exactly the ones with the
/// largest fractional parts, earliest position winning a tie. An
/// implementation that distributed the shortfall round-robin, or to the
/// largest *weights*, or to the last elements, sums correctly and fails here.
#[test]
fn remainders_go_to_the_largest_fractions_ties_by_position() {
    let cases: &[(i64, &[i64])] = &[
        (10_000, &[1, 1, 1]),
        (10_000, &[1, 2, 3]),
        (10, &[3, 1]),
        (7, &[1; 10]),
        (100, &[17, 31, 52]),
        (-10_000, &[1, 1, 1]),
        (5, &[1, 1, 1, 1]),
    ];
    for (total, ws) in cases {
        let parts = allocate(*total, ws).unwrap();
        let sum: i128 = ws.iter().copied().map(i128::from).sum();
        let sign = if sum < 0 { -1i128 } else { 1 };
        let (floors, fracs, _) = floors_and_fracs(*total, ws);
        // Who *should* have received a unit: sort positions by fraction
        // descending, position ascending, and take the first `extra`.
        let expected = expected_recipients(*total, ws, AllocateMode::LargestRemainder);

        for (i, part) in parts.iter().enumerate() {
            let bumped = i128::from(*part) - floors[i];
            assert!(
                bumped == 0 || bumped == 1,
                "allocate({total}, {ws:?}): element {i} moved {bumped} units off its floor — \
                 the rule hands out ONE minor unit apiece"
            );
            assert_eq!(
                bumped == 1,
                expected.contains(&i),
                "allocate({total}, {ws:?}) = {parts:?}: element {i} (fraction {}/{}) \
                 got the wrong answer; the units belong to {expected:?}",
                fracs[i],
                sum * sign
            );
        }
    }
}

// ── 3b · the modes ───────────────────────────────────────────────────────

/// **The default is largest-remainder, and it is not a taste question.**
///
/// `allocate(100.00, (1, 1, 1))` = 33.34 / 33.33 / 33.33 is printed by four
/// documents, and it is printed under this mode: `first` gives the same
/// answer here (the tie-break IS positional order), but `last` gives
/// 33.33 / 33.33 / 33.34 and would falsify the page. Asserted over the whole
/// sweep so that the two-argument form can never quietly become a different
/// mode's alias.
#[test]
fn the_default_mode_is_largest_remainder() {
    assert_eq!(AllocateMode::default(), AllocateMode::LargestRemainder);
    for ws in WEIGHTINGS {
        for total in TOTALS.iter().copied() {
            assert_eq!(
                allocate(total, ws).unwrap(),
                allocate_with(total, ws, AllocateMode::LargestRemainder).unwrap(),
                "the omitted mode is largestRemainder: allocate({total}, {ws:?})"
            );
        }
    }
    // And the printed value under the mode that would replace it.
    assert_eq!(
        allocate_with(10_000, &[1, 1, 1], AllocateMode::Last).unwrap(),
        vec![3_333, 3_333, 3_334],
        "if `last` were the default the specification's worked example would be wrong"
    );
}

/// **Both structural invariants, for every mode.**
///
/// A mode is a *priority order*, not a second algorithm: floor every exact
/// share, then hand the leftover units out one apiece down the queue the mode
/// names. Two properties follow from that construction, and they are the
/// reason the construction was chosen — so they are asserted for all four
/// modes over the same sweep the default is held to, rather than for the
/// default and then assumed:
///
/// 1. **the parts sum to the total exactly** — the guarantee `allocate`
///    exists for, and a mode that broke it would not be a mode;
/// 2. **no part is more than one minor unit from its floor** — which is what
///    stops "mode" from being a way to smuggle an arbitrary redistribution in
///    behind a name. `allocate(x, ws, first)` handing *all* the leftover to
///    element 0 would still sum, and would still be wrong.
#[test]
fn every_mode_keeps_the_sum_and_moves_each_part_at_most_one_unit() {
    for mode in AllocateMode::ALL.iter().copied() {
        for ws in WEIGHTINGS {
            for total in TOTALS.iter().copied() {
                let parts = allocate_with(total, ws, mode)
                    .unwrap_or_else(|e| panic!("{}({total}, {ws:?}): {e}", mode.name()));
                assert_eq!(parts.len(), ws.len(), "one part per weight");
                assert_eq!(
                    parts.iter().sum::<i64>(),
                    total,
                    "allocate({total}, {ws:?}, {}) = {parts:?} does not sum to its total",
                    mode.name()
                );
                let (floors, _, extra) = floors_and_fracs(total, ws);
                let mut bumped = 0usize;
                for (i, part) in parts.iter().enumerate() {
                    let d = i128::from(*part) - floors[i];
                    assert!(
                        d == 0 || d == 1,
                        "allocate({total}, {ws:?}, {}): element {i} moved {d} units off its \
                         floor — every mode hands the leftover out ONE apiece and differs \
                         only in the order of the queue",
                        mode.name()
                    );
                    bumped += d as usize;
                }
                assert_eq!(
                    bumped,
                    extra,
                    "allocate({total}, {ws:?}, {}) handed out {bumped} units, not {extra}",
                    mode.name()
                );
            }
        }
    }
}

/// **Each mode is the queue it names** — checked against an independently
/// derived one, the same way the default's largest-remainder claim is.
///
/// A sum-and-bound test cannot tell `first` from `last`; both pass it. This
/// is the test that says *which* parties got the units, and it is the one
/// that would fail if a mode were wired to the wrong comparator.
///
/// `largestShare` is ordered by **|weight|**, which is the same ordering as
/// "largest part" without depending on the sign of the total — the negative
/// totals in the sweep are what make that a real assertion rather than a
/// restatement.
#[test]
fn each_mode_hands_the_units_to_the_queue_it_names() {
    for mode in AllocateMode::ALL.iter().copied() {
        for ws in WEIGHTINGS {
            for total in TOTALS.iter().copied() {
                let parts = allocate_with(total, ws, mode).unwrap();
                let (floors, _, _) = floors_and_fracs(total, ws);
                let expected = expected_recipients(total, ws, mode);
                for (i, part) in parts.iter().enumerate() {
                    assert_eq!(
                        i128::from(*part) - floors[i] == 1,
                        expected.contains(&i),
                        "allocate({total}, {ws:?}, {}) = {parts:?}: element {i} got the wrong \
                         answer; the units belong to {expected:?}",
                        mode.name()
                    );
                }
            }
        }
    }
}

/// **Four names are four functions.**
///
/// Every mode passing its own characterization would still be consistent with
/// two of them being wired to the same comparator — the per-mode tests would
/// agree with each other and with the code, because the expectation and the
/// implementation would be making the same mistake. This asserts the modes
/// are pairwise distinct *as functions*: for every pair, some case in the
/// sweep separates them.
///
/// It also documents the shape of the disagreements. `first` and
/// `largestRemainder` coincide often, because the tie-break in
/// largest-remainder IS positional order and equal weights make every
/// remainder tie — which is exactly why `allocate(100.00, (1,1,1))` cannot be
/// used to tell them apart, and why this sweep carries lopsided weights.
#[test]
fn no_two_modes_are_the_same_function() {
    let modes = AllocateMode::ALL;
    for (x, a) in modes.iter().copied().enumerate() {
        for b in modes.iter().copied().skip(x + 1) {
            let mut separated = None;
            'search: for ws in WEIGHTINGS {
                for total in TOTALS.iter().copied() {
                    let pa = allocate_with(total, ws, a).unwrap();
                    let pb = allocate_with(total, ws, b).unwrap();
                    if pa != pb {
                        separated = Some((total, *ws, pa, pb));
                        break 'search;
                    }
                }
            }
            assert!(
                separated.is_some(),
                "`{}` and `{}` agree on every case in the sweep — either they are the same \
                 comparator wearing two names, or the sweep cannot tell them apart and is \
                 not proving what the per-mode tests claim",
                a.name(),
                b.name()
            );
        }
    }

    // The separations, written out, so the file states what the modes
    // actually do rather than only that they differ. `100` over `(3, 1)`
    // is 75 / 25 exactly, so it takes an inexact split to see anything.
    let ws = &[3i64, 1];
    assert_eq!(allocate_with(10, ws, AllocateMode::First).unwrap(), [8, 2]);
    assert_eq!(allocate_with(10, ws, AllocateMode::Last).unwrap(), [7, 3]);
    // Largest remainder: 7.5 and 2.5 tie, so position decides — the same
    // answer as `first`, and this is the case that shows why `first` is
    // still a distinct mode further down the sweep.
    assert_eq!(
        allocate_with(10, ws, AllocateMode::LargestRemainder).unwrap(),
        [8, 2]
    );
    // …and here it is not: 1/3 against 2/3 sends the unit to the second
    // element, where `first` sends it to the first and `largestShare`
    // follows the bigger weight.
    assert_eq!(
        allocate_with(7, &[1, 2], AllocateMode::LargestRemainder).unwrap(),
        [2, 5]
    );
    assert_eq!(
        allocate_with(7, &[1, 2], AllocateMode::First).unwrap(),
        [3, 4]
    );
    assert_eq!(
        allocate_with(7, &[1, 2], AllocateMode::LargestShare).unwrap(),
        [2, 5]
    );
    // `largestShare` against `largestRemainder`: the big weight has the
    // smaller fraction here, so the two part company.
    assert_eq!(
        allocate_with(11, &[10, 1, 1], AllocateMode::LargestRemainder).unwrap(),
        [9, 1, 1]
    );
    assert_eq!(
        allocate_with(11, &[10, 1, 1], AllocateMode::LargestShare).unwrap(),
        [10, 1, 0]
    );
}

/// **One closed set of mode names, checked in both directions.**
///
/// `ordo_syntax::ALLOCATE_MODES` is what the parser admits;
/// `ordo_plan::AllocateMode` is what the kernel computes. Two lists about
/// money that can drift apart will: a mode the parser accepts and the kernel
/// does not know is an unreachable arm, and a mode the kernel implements and
/// the parser refuses is a feature nobody can spell.
#[test]
fn the_mode_names_are_one_closed_set() {
    let spelled: Vec<&str> = ordo_syntax::ALLOCATE_MODES.to_vec();
    let computed: Vec<&str> = AllocateMode::ALL.iter().map(|m| m.name()).collect();
    assert_eq!(
        spelled, computed,
        "the surface mode names and the kernel's modes have drifted"
    );
    for name in &spelled {
        assert_eq!(
            AllocateMode::from_name(name).map(|m| m.name()),
            Some(*name),
            "`{name}` parses but does not resolve"
        );
    }
    assert_eq!(AllocateMode::from_name("rotation"), None);
    assert_eq!(AllocateMode::from_name("largest"), None);
    assert_eq!(AllocateMode::from_name(""), None);

    // **Every mode name is a legal Ordo name.** DL §1 *Identifiers* is
    // `[settled]` — "one rule for every declared name … `[a-z][A-Za-z0-9]*`
    // — lowerCamel, no underscores" — and the lexer enforces it. The
    // literature spells these `largest_remainder`; that spelling does not
    // lex in this language, and the first draft of this work discovered it
    // by writing four modes nobody could type.
    for name in &spelled {
        let mut cs = name.chars();
        assert!(
            cs.next().is_some_and(|c| c.is_ascii_lowercase())
                && cs.all(|c| c.is_ascii_alphanumeric()),
            "`{name}` is not `[a-z][A-Za-z0-9]*`, so DL §1 says it is not a name"
        );
        assert!(
            ordo_syntax::parse_expr(&format!("allocate($t, item.w, {name})")).is_ok(),
            "`{name}` does not lex"
        );
    }
}

/// Determinism, in the plain sense: the same input is the same output, and
/// the answer depends on the weights' *positions*, not on their multiset.
#[test]
fn the_same_input_is_the_same_output() {
    let ws = [5, 5, 3, 1];
    let once = allocate(1_000, &ws).unwrap();
    for _ in 0..8 {
        assert_eq!(allocate(1_000, &ws).unwrap(), once);
    }
    // Permuting the weights permutes the parts — the tie-break reads
    // position, so this is a real assertion and not a tautology.
    let flipped = allocate(1_000, &[1, 3, 5, 5]).unwrap();
    let mut a = once.clone();
    let mut b = flipped.clone();
    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b, "the same shares, differently ordered");
    assert_ne!(once, flipped, "and the order is the weights' own");
}

// ── 4 · the two venues ───────────────────────────────────────────────────

/// **The determinism clause, checked.** DL §3 breaks the tie "by weight order
/// (deterministic, so both venues agree)"; `glossary.md` names the venues —
/// "so host and SQL agree". Two venues that never ran the same input past
/// each other agree only by assertion.
///
/// The SQL below is an **oracle, not a lowering**, and the distinction
/// matters enough to state twice. `allocate` reads the whole weight vector to
/// answer for one element, so it is a windowed computation over the list
/// relation — it can never lower through `Planner::function`, which renders a
/// scalar expression over the current row. What the real lowering looks like
/// depends on how a `list of { … }` parameter arrives, and that is the
/// **`[lean]`** shape this work is under orders not to invent. So the
/// relation here is a `VALUES` list belonging to this test, exactly as
/// `aggregates.rs` builds relations to assert identities the language states.
///
/// It is still worth running, because it catches the hazard a host-only test
/// cannot see: **Postgres integer division truncates toward zero and the
/// specification says floor.** The two agree for a non-negative total and
/// part company on a refund split, so the negative cases below are the point
/// of the exercise rather than padding.
#[test]
fn agrees_with_postgres_over_the_same_inputs() {
    let pg = Pg::named("ordo_alloc_venues");

    // Ordinality is the tie-break, so it is part of the oracle, not
    // scaffolding: `row_number() OVER (ORDER BY r DESC, i)` is the SQL
    // spelling of "largest remainder, ties by weight order".
    pg.exec(
        r#"
        CREATE OR REPLACE FUNCTION ordo_allocate(total bigint, ws bigint[], mode text)
        RETURNS bigint[] AS $fn$
        WITH w(i, weight) AS (
          SELECT ordinality, x FROM unnest(ws) WITH ORDINALITY AS t(x, ordinality)
        ),
        s AS (SELECT sum(weight) AS sw FROM w),
        -- Exact integer arithmetic in `numeric`, and true floor rather than
        -- `/`: `(a - ((a % sw) + sw) % sw) / sw` is `floor(a/sw)` for either
        -- sign, where plain `/` would truncate toward zero.
        f AS (
          SELECT i, weight,
                 ((total::numeric * weight)
                   - (((total::numeric * weight) % sw) + sw) % sw) / sw AS q,
                 -- Normalized so "largest" compares fractions, not signed
                 -- leftovers, when the weights sum negative.
                 ((((total::numeric * weight) % sw) + sw) % sw) * sign(sw) AS r
          FROM w, s
        ),
        e AS (SELECT total - sum(q) AS extras FROM f),
        -- The MODE is the ORDER BY, and nothing else. That is the whole
        -- claim the modes make on the host side too: one algorithm, four
        -- queues. If the venues can disagree about a mode, this is where
        -- it shows — the primary key is what the mode ranks by, and the
        -- secondary is always the weights' own order.
        n AS (
          SELECT i, q, row_number() OVER (
                   ORDER BY CASE mode
                              WHEN 'largestRemainder' THEN r
                              WHEN 'largestShare'     THEN abs(weight)::numeric
                              ELSE 0
                            END DESC,
                            CASE WHEN mode = 'last' THEN -i ELSE i END ASC
                 ) AS rn
          FROM f
        )
        SELECT array_agg(
                 (q + CASE WHEN rn <= (SELECT extras FROM e) THEN 1 ELSE 0 END)::bigint
                 ORDER BY i)
        FROM n;
        $fn$ LANGUAGE sql IMMUTABLE;
        "#,
    )
    .expect("the oracle installs");

    let cases: &[(i64, &[i64])] = &[
        (10_000, &[1, 1, 1]),
        (10_000, &[1, 2, 3]),
        (10, &[3, 1]),
        (7, &[1; 10]),
        (100, &[0, 1, 0]),
        (100, &[17, 31, 52]),
        (1, &[1, 1]),
        (0, &[4, 9]),
        // The ones that separate floor from truncation. A `/`-based host
        // implementation and a `/`-based SQL one would agree with each
        // other here and both be wrong; these are checked against the
        // documented value too, in `parts_always_sum_to_the_total`.
        (-10_000, &[1, 1, 1]),
        (-7, &[1, 2, 3]),
        (-1, &[1, 1]),
    ];

    // **Every mode, not just the default.** A mode that computes differently
    // in SQL than on the host is a defect, not a variant — the whole point of
    // naming modes rather than leaving the odd penny to the caller is that
    // the answer is the same wherever it is computed.
    for (total, ws) in cases {
        for mode in AllocateMode::ALL.iter().copied() {
            let arr = ws
                .iter()
                .map(|w| w.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let got = pg
                .query(&format!(
                    "SELECT ordo_allocate({total}, ARRAY[{arr}]::bigint[], '{}')",
                    mode.name()
                ))
                .expect("the oracle runs")[0][0]
                .clone()
                .expect("an array, not null");
            let sql: Vec<i64> = got
                .trim_matches(|c| c == '{' || c == '}')
                .split(',')
                .map(|s| s.parse().expect("a bigint"))
                .collect();
            let host = allocate_with(*total, ws, mode).expect("the host venue answers");
            assert_eq!(
                host,
                sql,
                "the two venues disagree on allocate({total}, {ws:?}, {}) — \
                 host {host:?}, SQL {sql:?}",
                mode.name()
            );
            assert_eq!(sql.iter().sum::<i64>(), *total, "and SQL sums too");
        }
    }

    // The oracle must be able to tell the modes apart, or the loop above is
    // four copies of one assertion. Checked in SQL, on the server's own
    // answers, rather than inferred from the host's.
    let split = pg
        .query(
            "SELECT ordo_allocate(10, ARRAY[3,1]::bigint[], 'first'), \
                    ordo_allocate(10, ARRAY[3,1]::bigint[], 'last'), \
                    ordo_allocate(7, ARRAY[7,11,13,17,19,23]::bigint[], 'largestRemainder'), \
                    ordo_allocate(7, ARRAY[7,11,13,17,19,23]::bigint[], 'largestShare')",
        )
        .expect("runs");
    assert_eq!(
        split[0],
        vec![
            Some("{8,2}".to_string()),
            Some("{7,3}".to_string()),
            Some("{1,1,1,1,1,2}".to_string()),
            Some("{0,0,1,2,2,2}".to_string()),
        ],
        "the SQL oracle does not distinguish the modes, so agreeing with the host \
         about them proves nothing"
    );

    // The hazard, stated as its own assertion rather than left to be
    // inferred from the loop above: Postgres's own `/` on a negative
    // quotient is not floor, so an oracle that used it would have agreed
    // with a host implementation that made the same mistake.
    let trunc = pg
        .query("SELECT (-10000::numeric * 1) / 3, floor((-10000::numeric * 1) / 3)")
        .expect("runs");
    assert_ne!(
        trunc[0][0], trunc[0][1],
        "if these were equal the negative cases above would be testing nothing"
    );
}

// ── 5 · the half that is not owed ────────────────────────────────────────

/// **There is no legal site, and this asserts it rather than trusting the
/// refusal's own message.**
///
/// DL §3 is exhaustive by construction — "inside an `each` emission over a
/// list parameter … Anywhere else it is `invalid_allocate`" — and `each` has
/// no grammar while `list of { … }` has no `ParamTy`. So *every* site the
/// surface language can currently express is an illegal one, which is why
/// [`ordo_plan::allocate`] has no caller and is not going to acquire one by
/// anything short of `each` being settled.
#[test]
fn has_no_legal_site_in_the_surface_language() {
    let f = Fixture::load().expect("fixture");
    for src in [
        "orderItems[part: allocate(amount, qty)]",
        "orderItems[amount, qty] | allocate(amount, qty) > 3",
        "orderItems[amount, qty]\nsort allocate(amount, qty)",
        "orderItems[amount, qty]\nreduce { t: sum allocate(amount, qty) }",
    ] {
        let e = f
            .compile(src)
            .expect_err("no expression position is a legal home");
        assert!(e.contains("invalid_allocate"), "{src}\n{e}");
    }

    // And the construct it WOULD be legal in does not parse — asserted so
    // that the day `each` lands, this test fails and sends whoever landed
    // it back to this file.
    assert!(
        ordo_syntax::parse_program(
            "op splitBill($shares: list of { member: id, weight: int }, $total: money) =\n\
             set billShares each $shares { member: item.member, \
             amount: allocate($total, item.weight) }\n"
        )
        .is_err(),
        "`each` over a `list of {{ … }}` parameter parses now — DL §1 marks both \
         [lean], 9e′ ruled a [lean] construct is not owed, and `ordo_plan::allocate` \
         has been waiting for exactly this. Wire it up."
    );
}

// ── 6 · the mode is a closed grammar, and it fails closed ────────────────

/// **An unknown mode is a compile error, not a fall back to the default.**
///
/// This is the whole argument for *named modes on one function* over *a
/// family of functions*: one name to learn, and a name outside the set is
/// refused by the grammar rather than absorbed. A misspelled mode that
/// quietly allocated by largest-remainder would be a money bug wearing a
/// typo's clothes — nothing would fail, and the parts would still sum.
///
/// The refusal is a **parse error** (`parse_error`, already in the closed
/// inventory) and not a new class: the third argument position is a mode
/// name, so a word that is not a mode is text outside the closed grammar,
/// which is what that class means. Deciding it in the parser also puts it
/// where it survives the site landing — `each` is still `[lean]`, and a rule
/// living in the site's binder would be written by whoever lands the site.
///
/// `rotation` gets its own case because it is the mode people reach for and
/// the reason it is absent is a **ruling**, not an oversight: spreading the
/// odd penny across successive invoices has to remember what it did last
/// time, so it is not a function of `(total, weights)`. The message says so,
/// because "unknown mode" would send the reader looking for a typo.
#[test]
fn an_unknown_mode_is_refused_at_compile_time() {
    let expr = |s: &str| ordo_syntax::parse_expr(s).map_err(|e| e.to_string());

    // Every mode in the set parses. It then fails at the *site*, which is
    // the other half of this file — a mode does not make an illegal home
    // legal, and this is asserted rather than assumed just below.
    for m in ordo_syntax::ALLOCATE_MODES {
        assert!(
            expr(&format!("allocate($total, item.weight, {m})")).is_ok(),
            "`{m}` is in ALLOCATE_MODES and does not parse"
        );
    }

    let e = expr("allocate($total, item.weight, rotation)").expect_err("not a mode");
    assert!(e.contains("unknown allocation mode `rotation`"), "{e}");
    assert!(
        e.contains("remember what it did last time"),
        "the message should say WHY rotation is absent, not just that it is: {e}"
    );
    assert!(e.contains("parse_error"), "the class is not a new one: {e}");

    let e = expr("allocate($total, item.weight, largest)").expect_err("not a mode");
    assert!(e.contains("unknown allocation mode `largest`"), "{e}");
    for m in ordo_syntax::ALLOCATE_MODES {
        assert!(e.contains(m), "the refusal lists the legal modes: {e}");
    }

    // A mode is a bare NAME, not an expression — so a string, a number or a
    // dotted path is refused with the shape rather than accepted and
    // reinterpreted.
    for bad in [
        r#"allocate($total, item.weight, "first")"#,
        "allocate($total, item.weight, 1)",
        "allocate($total, item.weight, item.mode)",
    ] {
        let e = expr(bad).expect_err("a mode is a bare name");
        assert!(e.contains("the mode"), "{bad}\n{e}");
    }

    // Arity still closes at three.
    assert!(expr("allocate($total, item.weight, first, last)")
        .expect_err("four arguments")
        .contains("2 or 3 arguments"));

    // And a legal mode at an illegal site is still `invalid_allocate` — the
    // site refusal is the binder's and the mode check is the parser's, and
    // neither substitutes for the other.
    let f = Fixture::load().expect("fixture");
    let e = f
        .compile("orderItems[part: allocate(amount, qty, largestShare)]")
        .expect_err("a mode does not make the site legal");
    assert!(e.contains("invalid_allocate"), "{e}");
}

// ── 7 · a part that does not fit the type ────────────────────────────────

/// **Found while widening this: `allocate` panicked on input no document
/// forbids.**
///
/// The old kernel closed with `i64::try_from(part).expect("a part is bounded
/// by the total")`, and the comment beside it was right about the case
/// everyone had in mind and wrong in general. `|wᵢ| ≤ |Σw|` bounds every part
/// by `total` **only when the weights share a sign**. A mixed-sign vector
/// that nearly cancels — `Σw` tiny, the weights enormous — makes individual
/// parts far larger than the total they still sum to, and the `expect` fired:
/// a panic in library code, on the exact input the earlier ruling had
/// declared "computes the stated formula, the invariant survives".
///
/// Negative weights remain **unruled** — no document types the vector beyond
/// `weight: int`, and inventing a refusal for them is a ruling this is not
/// the place for. But "unruled" cannot mean "panics", so the overflow now
/// returns the class the platform already gives an `int`/`money` overflow:
/// `arithmetic_error` (`api-engine-design.md`, "`/ 0`, `mod(a, 0)` or an
/// `int`/`money` overflow"). A reuse, not a mint.
#[test]
fn a_part_too_large_for_the_type_is_the_ruled_arithmetic_error() {
    // Σw = 1, so element 0's exact share is total × 1_000_000 — outside i64
    // while the two parts still sum to i64::MAX exactly.
    for mode in AllocateMode::ALL.iter().copied() {
        let e = allocate_with(i64::MAX, &[1_000_000, -999_999], mode)
            .expect_err("a part outside the type is not an answer");
        assert!(
            e.starts_with("arithmetic_error:"),
            "the class is the one an overflow already has, not a new one: {e}"
        );
    }

    // The neighbouring case still answers, so the check is not a blanket ban
    // on mixed signs: same shape, a total small enough that the parts fit.
    let parts = allocate_with(100, &[1_000_000, -999_999], AllocateMode::LargestRemainder)
        .expect("the parts fit here");
    assert_eq!(parts.iter().sum::<i64>(), 100);

    // And `i64::MIN` is reachable as a weight, where an `i64` `.abs()` in the
    // `largestShare` comparator would panic. It does not, because the
    // magnitude is taken in i128.
    let parts = allocate_with(10, &[i64::MIN, 1, 1], AllocateMode::LargestShare)
        .expect("i64::MIN is an ordinary weight to a comparator that widens");
    assert_eq!(parts.iter().sum::<i64>(), 10);
}

/// `allocate` reads the money's places (27ak): the total is written in major
/// units, and the places decide what the odd minor unit is.
#[test]
fn allocate_splits_in_the_minor_units_of_the_declared_places() {
    let lr = AllocateMode::LargestRemainder;
    // DL's own example, now in the units DL writes it in.
    assert_eq!(
        allocate_at_places("100.00", 2, &[1, 1, 1], lr).unwrap(),
        ["33.34", "33.33", "33.33"]
    );
    // Whole units at places 0: the odd unit is a dollar, not a cent.
    assert_eq!(
        allocate_at_places("100", 0, &[1, 1, 1], lr).unwrap(),
        ["34", "33", "33"]
    );
    // Yen-like and mill-like places split their own smallest unit.
    assert_eq!(
        allocate_at_places("1", 3, &[1, 1, 1], lr).unwrap(),
        ["0.334", "0.333", "0.333"]
    );
    // A total finer than the places has no value to split.
    let e = allocate_at_places("100.005", 2, &[1, 1], lr).unwrap_err();
    assert!(e.starts_with("implicit_rounding"), "{e}");
}
