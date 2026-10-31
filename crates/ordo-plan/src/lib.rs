//! P4 (slice) — the planner. Bound IR in, one PostgreSQL `SELECT` out.
//!
//! The planner does no name resolution: everything it prints was resolved by
//! the binder. Its whole job is to be deterministic and to be honest about
//! ordering.
//!
//! # Ordering
//!
//! Every plan ends with a total order. Declared sort keys come first, then
//! the implicit tiebreak (R-C4/R-C7): the anchor row id for row pipelines,
//! the group keys for grouped ones. Without that, two runs of the same query
//! may return the same rows in different orders and the corpus's byte-equal
//! comparison would be testing the query planner's mood.
//!
//! `NULLS LAST` is emitted explicitly on every key rather than inherited from
//! Postgres's direction-dependent default, so a descending sort does not
//! silently move nulls to the top.
//!
//! [`TEXT_COLLATION`] is emitted explicitly on every text ordering for the
//! same reason and with more at stake: without it, the deployment's
//! `datcollate` decides the answer.
//!
//! # Not implemented
//!
//! Cost-class classification and the `unbounded` gate — the other half of P4 —
//! are absent. This crate will happily plan an op that reads a whole table.
//! Nothing here decides whether an op is allowed to be registered.

use ordo_schema::ddl::{literal, q, SPACE_COLUMN};

pub mod artifact;
pub mod blind;
pub mod cost;
pub mod download;
pub mod each;
pub mod interface;
pub mod public;
pub mod whole;
pub mod writes;
use ordo_schema::SchemaError;
use ordo_sema::ir::{BCond, BExpr, From, Limit, OutField, Query, Sort};

/// The fixed fractional scale of every `decimal` the language can produce
/// (`definition-language.md` §2, "Division: determinism across venues"):
/// **twelve digits after the decimal point**, not a significant-digit
/// budget, not configurable, and the same number host-side and in SQL.
///
/// The earlier spellings the spec withdrew are worth remembering, because
/// both look right: `NUMERIC(38, 12)` bounds *magnitude* (leaving 26
/// integer digits) and so refuses expressions the arbitrary-precision
/// `+ - *` algebra answers; "38 significant digits" is not expressible as
/// a `NUMERIC` typmod at all. `round(value, 12)` over unconstrained
/// `numeric` fixes the fractional scale — the whole of what the pin
/// asserts — and bounds nothing else.
pub const DECIMAL_SCALE: u32 = 12;

/// The collation every text ordering is emitted under.
///
/// **Engine-pinned pending a G11 ruling** (`engine-plan.md` G11,
/// `definition-language.md`: "Text collation … engine-pinned pending
/// confirmation"). The spec owes a real ruling; what the engine owes
/// meanwhile is determinism and a statement of what it pinned, which is
/// this constant.
///
/// # Why a pin is not optional
///
/// Postgres orders text by the collation in scope, and an unqualified
/// `ORDER BY name` takes the *database's* — `en_US.UTF-8` on one
/// deployment, `C` on the next. Those disagree: `('a', 'a-b', 'ab', 'B')`
/// under `en_US.UTF-8` is `('B', 'a', 'a-b', 'ab')` under `C`. Ordering is
/// not cosmetic here. Keyset pagination's seek predicate is "strictly
/// after this row **in the plan's order**", so if the ORDER BY and the
/// seek were evaluated under different collations — or the same query ran
/// against two replicas provisioned with different locales — a page would
/// **skip or repeat rows**, which is the exact property keyset pagination
/// exists to provide.
///
/// # Why `C`
///
/// `C` is byte order over the UTF-8 encoding, which for valid UTF-8 *is*
/// codepoint order — so this is the same answer `engine-plan.md` G11
/// already pins ("text sorts by NFC codepoint order, locale-free"),
/// spelled in the one collation Postgres guarantees is present in every
/// installation. It needs no ICU build, no locale on the host, and no
/// agreement between the engine and the deployment about which `en_US` a
/// libc version means — glibc has changed `en_US.UTF-8`'s ordering across
/// releases, which is precisely the kind of drift a pinned engine must
/// not inherit.
///
/// It is deliberately **not** a human-friendly order: under `C`, `Z` sorts
/// before `a`. That is the trade the pin makes, and it is the right one to
/// make at this layer — locale-aware *presentation* ordering is a client
/// concern (G11 says so), while what the engine must guarantee is that the
/// same plan returns the same page everywhere.
///
/// # What is NOT collated, and why
///
/// Text **equality** — `=`, `IS NOT DISTINCT FROM`, `DISTINCT`, `GROUP
/// BY`, `UNIQUE` — is byte equality under every *deterministic* collation,
/// and Postgres refuses a nondeterministic collation as a database
/// default. So grouping and de-duplication do not depend on this pin and
/// are left uncollated; the one place `DISTINCT` sees it is that Postgres
/// requires a `SELECT DISTINCT`'s ORDER BY expressions to appear in the
/// select list, so the projection has to be spelled the same way the
/// ordering is.
///
/// Nothing is collated in the generated DDL. Pinning a *column*'s
/// collation would also repoint `lower()`/`upper()` at `C`'s ctype, which
/// stops folding non-ASCII case — trading a deterministic ordering for a
/// wrong `normalize(lower)`. The cost of keeping it out of the DDL is that
/// a default-collation btree index cannot serve `ORDER BY x COLLATE "C"`;
/// the fix is a `COLLATE "C"` expression index per sort key, and it is
/// owed work rather than a reason to move the pin into the columns.
pub const TEXT_COLLATION: &str = "C";

/// **`hash(x)`'s algorithm, frozen** (engine-plan G12, ruled 2026-08-30 in
/// `corpus-rulings.md`): SHA-256 of the value's UTF-8 bytes, first eight
/// bytes big-endian, top bit cleared.
///
/// DL §3 promises `hash(x)` is "stable across platform versions", which is a
/// compatibility promise about *bytes*: once a deployment has striped a
/// rollout on `mod(hash(id), 100)`, changing the algorithm reshuffles every
/// cohort silently. The ruling's whole argument is that the only way to hold
/// such a promise is to not own the thing being promised — SHA-256 is an
/// external frozen standard with published test vectors, implemented
/// identically by Postgres core (`sha256(bytea)`, in core since 11, no
/// extension, and unlike `md5()` it is available under an OpenSSL FIPS
/// provider) and by any host library. "Stable" here means "we did not invent
/// it and therefore cannot drift from it".
///
/// The three details that are ours, and are what a second implementation
/// gets wrong:
///
/// - **Which bytes.** The first eight of the digest, big-endian, which is
///   the order they are printed in.
/// - **Top bit cleared.** Ordo's `int` is signed 64-bit, so an untruncated
///   digest is negative half the time — and `mod` follows the dividend's
///   sign (DL §3), so `mod(hash(id), 100)` would land in `(-100, 100)` and
///   every rollout stripe would quietly be half the width its author
///   intended. Masking costs one bit of a hash used for bucketing and
///   removes the trap. `abs()` would not: `abs(i64::MIN)` overflows.
/// - **The input is UTF-8 text.** `convert_to(x, 'UTF8')` and NOT
///   `x::bytea` — the cast goes through `byteain`, so a value beginning
///   `\x` is reinterpreted as a hex escape and `'\x4142'` hashes as the two
///   bytes `AB`. Two distinct ids colliding into one bucket, from a cast
///   that looks tidier.
///
/// The cost of `convert_to` is that it is STABLE, not IMMUTABLE (its result
/// depends on the server encoding), so `hash(x)` cannot carry an expression
/// index as written. An IMMUTABLE platform-owned SQL wrapper is sound —
/// the platform requires a UTF-8 database — and is owed work, in the same
/// drawer as [`TEXT_COLLATION`]'s owed expression indexes.
pub fn hash64(s: &str) -> i64 {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(s.as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    (i64::from_be_bytes(b)) & i64::MAX
}

/// The SQL that computes [`hash64`] inside Postgres.
///
/// Kept beside the Rust so the two cannot drift unnoticed; `hash64_agrees_
/// with_its_sql` in the conformance suite runs both against a real server
/// over the same inputs and demands the same integers.
fn hash64_sql(arg: &str) -> String {
    format!(
        "(('x' || substr(encode(sha256(convert_to({arg}, 'UTF8')), 'hex'), 1, 16))\
         ::bit(64)::bigint & {})",
        i64::MAX
    )
}

/// **Which party absorbs the odd minor unit** — the caller's choice, not the
/// specification's.
///
/// `allocate` guarantees the parts sum to the total; it does **not** get to
/// decide who ends up a penny up. That is a business rule — a contract names
/// a residual party, an accounting policy names largest-remainder, a
/// settlement agreement names the largest counterparty — and hard-coding one
/// of them makes the primitive unusable for the applications the rest are
/// written for. So the rule is parameterised, and the parameter is a **closed
/// set of names** rather than a family of functions: one name to learn, and
/// an unrecognised mode is a compile error rather than a silent fall back to
/// the default.
///
/// ### A mode is a priority order, not a second algorithm
///
/// Every mode runs the same two steps: **floor every exact share**, then hand
/// the leftover minor units out **one apiece**. A mode chooses only the
/// *order of the queue*. Nothing else about it is variable, and that is what
/// makes the guarantees structural rather than four separate promises:
///
/// * the parts **sum to the total exactly** — the leftover is `total` minus
///   the floors, which is in `[0, n)` for either sign of `total`, so handing
///   out exactly that many units of `+1` closes the gap whoever gets them;
/// * **no part is more than one minor unit from its exact share** — each
///   starts at its floor and moves at most `+1`, because a queue position is
///   served at most once.
///
/// A "mode" that broke either of those would not be a mode; it would be a
/// different primitive wearing this one's name. `allocate` exists for the
/// first property, and the second is what stops a mode from being a way to
/// smuggle an arbitrary redistribution past the type system.
///
/// ### Determinism is the boundary
///
/// **A mode may depend only on `(total, weights)`.** That is the whole
/// admission test, and it is not a style preference: DL §3 pins the tie-break
/// "deterministic, so both venues agree", and a mode that consults anything
/// else — a clock, a counter, a row somewhere — makes the host and SQL
/// answers diverge and makes the same call return different answers on a
/// retry, which the write protocol's idempotency rests on not happening.
///
/// This is what excludes **rotation** — spreading the odd penny across
/// successive invoices so that it evens out over time — and the exclusion is
/// worth naming because it is the mode users ask for first. Rotation is not a
/// function of `(total, weights)`; it needs to remember what it did last
/// time. That memory is application state, and state in an expression
/// function is how a language stops being one. The honest home for rotation
/// is a stored counter the caller keeps and passes in as the weights' order —
/// which is a rotation the *application* owns, visible in its own data, and
/// auditable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AllocateMode {
    /// **The default, and the only one that is also a specification value.**
    /// The leftover units go to the largest fractional remainders, ties by
    /// weight order (positional — see [`allocate`]). This is what accounting
    /// texts and regulators mean by "allocate", it minimises the largest
    /// deviation from an exact share, and it is the mode under which
    /// `allocate(100.00, (1, 1, 1))` is the printed `33.34 / 33.33 / 33.33`.
    /// Changing the default would falsify a value four documents print, so
    /// the default is not a taste question.
    #[default]
    LargestRemainder,
    /// The odd units to the **earliest** parties, in positional order. The
    /// shape of a contract that names a residual party first ("the lead
    /// bears rounding differences").
    First,
    /// The odd units to the **latest** parties, in reverse positional order.
    /// The ledger habit — the difference is plugged into the final line — and
    /// the mirror of [`First`](AllocateMode::First) so that a caller whose
    /// residual party is at the other end does not have to reverse its list,
    /// which would also reverse the parts.
    Last,
    /// The odd units to the parties with the **largest shares**, ties by
    /// weight order. Puts the rounding where it is proportionally smallest,
    /// which is the argument for it: a penny on a £10,000 share is noise, and
    /// on a £0.03 share it is 33%.
    ///
    /// "Largest share" is ordered by **|weight|**, and that is the same
    /// ordering as "largest part" without depending on the sign of `total`.
    /// Every exact share is `total/Σw · wᵢ`, so the shares differ from the
    /// weights by one common factor; ordering by share *magnitude* is
    /// therefore ordering by weight magnitude, and it stays the largest
    /// *refund* as well as the largest charge. Ordering by the signed part
    /// would silently mean "the smallest refund" the moment the total went
    /// negative, which is the case this platform's `floor` rule exists for.
    LargestShare,
}

impl AllocateMode {
    /// Every mode, in the order they are documented and offered.
    ///
    /// One list, read by the name table, the refusal message and the tests,
    /// so that a fifth mode cannot be half-added.
    pub const ALL: &'static [AllocateMode] = &[
        AllocateMode::LargestRemainder,
        AllocateMode::First,
        AllocateMode::Last,
        AllocateMode::LargestShare,
    ];

    /// The mode's name in Ordo source — the third argument to `allocate`.
    pub const fn name(self) -> &'static str {
        match self {
            AllocateMode::LargestRemainder => "largestRemainder",
            AllocateMode::First => "first",
            AllocateMode::Last => "last",
            AllocateMode::LargestShare => "largestShare",
        }
    }

    /// Resolve a written mode name, or `None` — which callers must turn into
    /// a refusal rather than into the default. A misspelled mode that
    /// silently allocated by largest-remainder would be a money bug that
    /// looks like a typo.
    pub fn from_name(s: &str) -> Option<AllocateMode> {
        AllocateMode::ALL.iter().copied().find(|m| m.name() == s)
    }
}

/// **`allocate(total, weights)`'s distribution rule, executable** — the
/// settled half of the money-split primitive (DL §3 *Allocation*, restated
/// verbatim in `glossary.md` and `traps.md`).
///
/// This is [`allocate_with`] at the default mode. Read that function for the
/// algorithm and [`AllocateMode`] for why there is a choice at all; what is
/// here is the shape three documents print and the one an omitted mode means.
///
/// `total` is an amount in **minor units** (`money` and `int` are both a
/// `bigint` count of minor units in this platform, DL §1); what a minor
/// unit is comes from the money's places, and [`allocate_at_places`] is the
/// same split taking the total in major units (27ak). The result is
/// one part per weight, in the weights' own order, and the parts **sum to
/// `total` exactly** — that guarantee is the reason the primitive exists,
/// because `/` cannot make it at any scale and three independent roundings
/// of `100.00` drop a cent.
///
/// The rule, which all three documents state in the same words:
///
/// 1. each part is the **floor of its exact share**, `⌊total · wᵢ / Σw⌋`;
/// 2. the minor units left over are handed out **one apiece by largest
///    remainder**;
/// 3. **ties break by weight order**.
///
/// ### What "weight order" means, and how it is known
///
/// It is the weights' **positional** order, not a descending sort by weight
/// *value*. The spec's own worked example is what decides it:
/// `allocate(100.00, (1, 1, 1))` is `33.34 / 33.33 / 33.33`. All three
/// remainders tie *and* all three weights are equal, so ordering by weight
/// value would leave the extra cent's home undetermined — and the whole
/// clause exists to be deterministic ("so both venues agree"). The first
/// element gets it. That is the only reading under which the pinned example
/// has one answer.
///
/// ### There is no caller, and that is the point
///
/// `allocate` is legal **only** inside an `each` emission over a `list of
/// { … }` parameter, and both of those are marked **`[lean]`** in DL §1 — so
/// no Ordo program can call this, and every site the surface language *can*
/// express is refused `invalid_allocate` (`ordo-sema`, and the backstop in
/// [`Planner::function`]). This function is the settled arithmetic, held
/// executable and pinned against the spec's own example so that the rule
/// cannot drift while its home is being decided. It is the same arrangement
/// [`hash64`] is in, one step earlier: a reference implementation beside a
/// lowering, except that the lowering cannot exist yet.
///
/// Note also that `allocate` will never lower through [`Planner::function`]
/// even once `each` lands: it reads the **whole** weight vector to answer for
/// one element, so it is a windowed computation over the list relation and
/// not a scalar expression over the current row. The arm in `function` is a
/// correct backstop; it is not the slot the lowering will occupy.
pub fn allocate(total: i64, weights: &[i64]) -> Result<Vec<i64>, String> {
    allocate_with(total, weights, AllocateMode::LargestRemainder)
}

/// **`allocate` at a money's places** (27ak, ruled 2026-10-04): the total
/// written in major units, split in the minor units of `money(places)`,
/// the parts spelled back in major units.
///
/// The places decide what "the odd minor unit" is. `100.00` at places 2 is
/// 10,000 cents and splits three ways as `33.34 / 33.33 / 33.33`, DL's own
/// example; `100` at places 0 is 100 whole units and splits as
/// `34 / 33 / 33`. The arithmetic is [`allocate_with`]'s, unchanged: the
/// parts still sum to the total exactly, because they are whole minor units
/// of the declared places.
///
/// A total finer than the places is refused rather than rounded, class
/// `implicit_rounding`: `100.005` has no cent value to split.
///
/// Like [`allocate`] it has no caller in a program yet. Its home, an `each`
/// over a list parameter, is `[lean]`, and when it lands the total is a
/// `money(p)` value the binder has already put in minor units, so the
/// lowering reads `p` off the total's type. This is that rule held
/// executable, so the places cannot be forgotten when the site arrives.
pub fn allocate_at_places(
    total: &str,
    places: u8,
    weights: &[i64],
    mode: AllocateMode,
) -> Result<Vec<String>, String> {
    use ordo_syntax::numeric::{major_spelling, minor_units, Fit};
    let minor = minor_units(total, places).map_err(|e| match e {
        Fit::TooFine { written } => format!(
            "implicit_rounding: `allocate` total `{total}` has {written} places and \
             `money({places})` holds {places}"
        ),
        Fit::Overflow => format!(
            "arithmetic_error: `allocate` total `{total}` is too large for `money({places})`"
        ),
        Fit::NotNumeric => format!("`allocate` total `{total}` is not a decimal"),
    })?;
    Ok(allocate_with(minor, weights, mode)?
        .into_iter()
        .map(|part| major_spelling(i128::from(part), places))
        .collect())
}

/// [`allocate`] with the caller's choice of who absorbs the odd minor unit.
///
/// The algorithm is one algorithm, whatever the mode: **floor every exact
/// share, then hand the leftover units out one apiece down a queue that the
/// mode orders.** See [`AllocateMode`] for the queue each one builds, for the
/// two invariants that construction buys, and for why *rotation* is not on
/// the list.
///
/// ### Exactness
///
/// Every intermediate is `i128` over integers, so nothing rounds and nothing
/// overflows: `total · wᵢ` is at most 2¹²⁶ for `i64` inputs, and for weights
/// that share a sign each part is bounded in magnitude by `total`, so the
/// results fit `i64` whenever the input did. The division is **true floor**
/// (toward −∞) rather than Rust's or SQL's truncation-toward-zero, which
/// coincide only for a non-negative `total`; §2's word is "floor", and a
/// refund split is the ordinary case where the two part company. The
/// sum-to-total invariant holds for either sign: each floor is within one
/// unit below its exact share, so the shortfall is always in `[0, n)` and
/// always lands.
///
/// ### The refusals
///
/// `Σw == 0` is a division by zero, which DL §2 rules "a typed runtime error,
/// uniform with `mod(a, 0)`", and checklist 9w ruled its class:
/// **`arithmetic_error`**, 400. No class is minted here, and the error is
/// rendered with the same `class: message` shape `PoolError::Arithmetic`
/// uses. An **empty** weight list reaches the same refusal by the same route
/// — the sum of no weights is zero — which is worth stating because the
/// documents do not: splitting among nobody has no answer that sums to a
/// non-zero total, so it is a division by zero and not a special case.
///
/// A part that does not fit `i64` takes the **same class**, on the same
/// reading: `api-engine-design.md` gives `arithmetic_error` "`/ 0`,
/// `mod(a, 0)` **or an `int`/`money` overflow**", and this is that overflow.
/// It is unreachable for weights that share a sign — `|wᵢ| ≤ |Σw|` bounds
/// every part by `total` — and reachable only for a mixed-sign vector, where
/// a near-cancelling `Σw` blows the parts up past the type while they still
/// sum to `total`. `allocate(i64::MAX, (1_000_000, -999_999))` is the small
/// case. Returning the ruled class beats the alternative that was here
/// before, which was a panic in library code on input no document forbids.
///
/// ### Deliberately NOT ruled here
///
/// **Negative weights.** No document types the weight vector beyond the
/// example's `weight: int`, and "in proportion to" has no evident meaning
/// when a weight is negative. This function computes the stated formula for
/// whatever integers it is given — both invariants survive — but that is
/// arithmetic, not a ruling, and no refusal is invented for it. Whether
/// negative weights are refused is owed with the surface syntax.
pub fn allocate_with(total: i64, weights: &[i64], mode: AllocateMode) -> Result<Vec<i64>, String> {
    // Widened before summing: n weights of i64::MAX cannot overflow i128
    // short of 2^64 elements, so this needs no checked arithmetic.
    let sum: i128 = weights.iter().copied().map(i128::from).sum();
    if sum == 0 {
        return Err(
            "arithmetic_error: `allocate` weights sum to zero — nothing to divide among".into(),
        );
    }
    let total = i128::from(total);
    // `sign` normalizes the remainder so that "largest remainder" compares
    // the FRACTIONAL PART rather than a signed leftover: with r carrying
    // the divisor's sign, `r/sum == (r*sign)/|sum|`, and `r*sign` is in
    // `[0, |sum|)`. Comparing raw `r` would invert the ordering whenever
    // the weights sum negative.
    let sign = if sum < 0 { -1 } else { 1 };
    let mut parts: Vec<i128> = Vec::with_capacity(weights.len());
    let mut fracs: Vec<i128> = Vec::with_capacity(weights.len());
    for w in weights {
        let num = total * i128::from(*w);
        // True floor, not `/`'s truncation toward zero: they differ exactly
        // when the quotient is negative and inexact.
        let mut q = num / sum;
        let r = num % sum;
        if r != 0 && ((r < 0) != (sum < 0)) {
            q -= 1;
        }
        parts.push(q);
        fracs.push((num - q * sum) * sign);
    }
    // Each floor is within one unit below its exact share and the exact
    // shares sum to `total`, so this is in `[0, weights.len())` — it can
    // always be handed out, one unit per element, and never twice to one.
    let extra = total - parts.iter().sum::<i128>();
    // The queue. Every mode is a total order over positions, and every one
    // of them ends in the position itself, so no mode has a tie to break by
    // anything but the weights' own order — which is the tie-break DL §3
    // already states, generalised rather than replaced.
    let mut order: Vec<usize> = (0..weights.len()).collect();
    match mode {
        // `sort_by` is stable and `order` starts ascending, so equal keys
        // keep positional order without a second comparison. Written as an
        // explicit `.then(a.cmp(b))` anyway: the tie-break is a documented
        // rule, and a rule that survives only because of a library
        // guarantee reads like an accident to the next person.
        AllocateMode::LargestRemainder => {
            order.sort_by(|a, b| fracs[*b].cmp(&fracs[*a]).then(a.cmp(b)));
        }
        AllocateMode::First => {}
        AllocateMode::Last => order.reverse(),
        AllocateMode::LargestShare => {
            // |w| in i128 — `i64::MIN.abs()` would panic in i64, and a
            // weight vector is not typed against that.
            let mag = |i: usize| i128::from(weights[i]).abs();
            order.sort_by(|a, b| mag(*b).cmp(&mag(*a)).then(a.cmp(b)));
        }
    }
    for i in order.into_iter().take(extra as usize) {
        parts[i] += 1;
    }
    parts
        .into_iter()
        // For same-sign weights every part is bounded in magnitude by
        // `total`, an `i64`, and this cannot fire. A mixed-sign vector can
        // put a part outside the type while the parts still sum to `total`,
        // and that is an `int`/`money` overflow — `arithmetic_error`, the
        // class `api-engine-design.md` already gives it. This used to be an
        // `expect`, i.e. a panic on input no document forbids.
        .map(|p| {
            i64::try_from(p).map_err(|_| {
                "arithmetic_error: `allocate` part does not fit the type — weights of mixed \
                 sign that nearly cancel make parts larger than the total they sum to"
                    .to_string()
            })
        })
        .collect()
}

// ── paths into `json` documents (DL §2, JSON documents) ──────────────────
//
// Three behaviours are owner rulings (board `json-field`, asks 4 to 6,
// ruled 2026-10-08). Each lives in one function below with its own test, so
// a later change to one is a change to that function and nothing else.

/// The sub-document at `keys` inside `doc`, as `jsonb`. Postgres's `#>`
/// reads each step as an object key, or as an array position when the step
/// is digits and meets an array. A step that finds nothing is SQL `NULL`,
/// never an error.
///
/// Every key is a quoted SQL literal, its quotes doubled exactly as
/// `ordo_schema::ddl::literal` writes a text literal. A key never reaches
/// SQL any other way.
pub fn doc_at(doc: &str, keys: &[String]) -> String {
    if keys.is_empty() {
        return doc.to_string();
    }
    let steps: Vec<String> = keys
        .iter()
        .map(|k| format!("'{}'", k.replace('\'', "''")))
        .collect();
    format!("({doc} #> ARRAY[{}]::text[])", steps.join(", "))
}

/// **Ask 5: a value of the wrong JSON type does not match, and is no
/// error.** The value is read as the type its condition gave it only when
/// its JSON type is that type; otherwise it reads as absent (SQL `NULL`),
/// which every comparison already treats as a missing value. So
/// `prefs.size > 3` passes over `"size": "big"` without a cast error.
/// Numbers are read as `numeric`, which is exact: no float is ever made.
pub fn doc_read(d: &str, ty: ScalarTy) -> String {
    let (json_ty, cast) = match ty {
        ScalarTy::Text => ("string", ""),
        ScalarTy::Int | ScalarTy::Decimal(_) => ("number", "::numeric"),
        ScalarTy::Bool => ("boolean", "::boolean"),
        _ => return d.to_string(),
    };
    format!("(CASE WHEN jsonb_typeof({d}) = '{json_ty}' THEN ({d} #>> '{{}}'){cast} END)")
}

/// **Ask 4: `is null` is true when the key is missing or holds JSON
/// `null`.** Both are "no value here" to the app developer, and a stored
/// document cannot tell them apart by intent.
pub fn doc_missing(d: &str) -> String {
    format!("({d} IS NULL OR jsonb_typeof({d}) = 'null')")
}

/// **Ask 6: `contains` reaches into arrays.** The value's JSON type settles
/// which question is asked, so the two never overlap: an array contains an
/// element equal to the right side (`@>`), and a string contains the right
/// side as a substring. A number or a bool on the right asks the array
/// question only. Anything else, and a missing value, is false.
pub fn doc_contains(d: &str, r: &str, rty: ScalarTy) -> String {
    let (cast, string_arm) = match rty {
        ScalarTy::Text => (
            "::text",
            format!(" WHEN 'string' THEN strpos({d} #>> '{{}}', {r}) > 0"),
        ),
        ScalarTy::Bool => ("::boolean", String::new()),
        _ => ("::numeric", String::new()),
    };
    format!(
        "COALESCE(CASE jsonb_typeof({d}) WHEN 'array' THEN {d} @> jsonb_build_array(({r}){cast})\
         {string_arm} ELSE false END, false)"
    )
}

/// Append the pinned collation to a rendered text expression.
///
/// Only ever called through [`Planner::collated`], which decides whether
/// the expression is text at all — `bytea` (every sealed column) has no
/// collation and Postgres rejects `COLLATE` on it outright.
fn collate(sql: &str) -> String {
    format!("{sql} COLLATE \"{TEXT_COLLATION}\"")
}

/// Pin an unconstrained-`numeric` expression to [`DECIMAL_SCALE`].
///
/// Postgres reduces scale half-up (away from zero), which is why the spec
/// pins `round` to half-up too: one tie-breaking behaviour across the
/// whole numeric surface, and the two venues agree digit-for-digit
/// without reimplementing Postgres's arithmetic host-side.
fn at_scale(sql: &str) -> String {
    format!("round({sql}, {DECIMAL_SCALE})")
}

/// A `1 AS "$e"` output for EXISTS bodies.
struct OutFieldOne;
impl OutFieldOne {
    fn one() -> OutField {
        OutField {
            name: "$e".into(),
            expr: BExpr::Lit {
                value: ordo_syntax::Expr::IntLit {
                    value: 1,
                    span: ordo_syntax::Span::new(0, 0, 1, 1),
                },
                ty: ordo_syntax::decl::ScalarTy::Int,
            },
        }
    }
}
use ordo_syntax::decl::ScalarTy;
use ordo_syntax::{BinOp, CmpOp, TextOp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for PlanError {}

/// The reserved bind slot the space rides in when it is not folded to a
/// literal. It is spelled with the `$` **inside the name** so it cannot
/// collide with an op parameter: a parameter is `$name` where `name` is an
/// identifier, and no identifier starts with `$`. The binder refuses a
/// declared `$space` on top of that (`ordo_sema::check_reserved_params`),
/// so the door's space is unshadowable at the compiler as well as at the
/// envelope (SH2-1).
pub const SPACE_SLOT: &str = "$space";

/// Where a statement's space value comes from.
///
/// **Never from the caller.** `$space` is stripped from the parameter map
/// at the door and is not a name any op can declare; the two forms here are
/// the only two ways a space reaches SQL.
///
/// This mirrors how `$caller` is threaded — `bind_as` takes the principal
/// and folds it to a literal when it is known, leaving a prepared slot when
/// it is not — with one deliberate difference: the space is a **planner**
/// argument rather than a binder one. The bound IR carries no space at all,
/// so there is exactly one value per statement and exactly one place it can
/// enter the SQL. A space stored per `Query` node would be N places to get
/// right (guard bodies, CTEs, union arms, EXISTS bodies are all `Query`s),
/// and "the space that got re-derived" is the cross-space read the check
/// exists to prevent (`engine-plan.md`, F4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Space {
    /// The door bound this space for this request; fold it to a literal.
    Bound(String),
    /// A prepared statement's reserved slot, filled by the executor from
    /// the door's binding. Always `$1` (see [`Planner::new`]).
    Slot,
}

impl Space {
    /// A space named by string. Convenience for the many call sites that
    /// already hold one.
    pub fn bound(s: impl Into<String>) -> Space {
        Space::Bound(s.into())
    }
}

/// Every physical table source a statement names, and whether its space
/// predicate was emitted. See [`verify_scoped`].
#[derive(Debug, Default)]
struct Sources {
    /// Aliases bound to a base table: each owes a space predicate.
    physical: Vec<String>,
    /// Aliases bound to something already scoped — a subquery, a CTE
    /// reference, a union wrapper, the paging wrapper. Their scoping lives
    /// inside them.
    derived: Vec<String>,
}

impl Sources {
    fn physical(&mut self, alias: &str) {
        if !self.physical.iter().any(|a| a == alias) {
            self.physical.push(alias.to_string());
        }
    }
    fn derived(&mut self, alias: &str) {
        if !self.derived.iter().any(|a| a == alias) {
            self.derived.push(alias.to_string());
        }
    }
}

/// Every `"name" AS "alias"` in a source position, as (name, alias).
///
/// Source position is decided by what precedes the opening quote: a
/// whitespace character, `(` or `,`. That is exactly what separates
/// `FROM "orders" AS "t0"` from `SELECT "t0"."id" AS "id"`, whose quoted
/// name is preceded by a dot.
fn scan_sources(sql: &str) -> Vec<(String, String)> {
    let b: Vec<char> = sql.chars().collect();
    let quoted = |i: usize| -> Option<(String, usize)> {
        if b.get(i) != Some(&'"') {
            return None;
        }
        let mut j = i + 1;
        let mut s = String::new();
        while j < b.len() {
            if b[j] == '"' {
                if b.get(j + 1) == Some(&'"') {
                    s.push('"');
                    j += 2;
                    continue;
                }
                return Some((s, j + 1));
            }
            s.push(b[j]);
            j += 1;
        }
        None
    };
    let mut out = Vec::new();
    for i in 0..b.len() {
        if b[i] != '"' {
            continue;
        }
        let before = if i == 0 { ' ' } else { b[i - 1] };
        if !(before.is_whitespace() || before == '(' || before == ',') {
            continue;
        }
        // `x COLLATE "C" AS "name"` is a projection, not a source. The
        // collation pin is the one other place a quoted name follows a
        // space, and a `SELECT DISTINCT` spells its projection that way
        // (Postgres requires the ORDER BY expression verbatim in the
        // select list), so it is not a corner case.
        let head = i - b[..i]
            .iter()
            .rev()
            .take_while(|c| c.is_whitespace())
            .count();
        if head >= 7 && b[head - 7..head].iter().collect::<String>() == "COLLATE" {
            continue;
        }
        let Some((name, mut j)) = quoted(i) else {
            continue;
        };
        // ` AS `
        if !(b.get(j) == Some(&' ')
            && b.get(j + 1) == Some(&'A')
            && b.get(j + 2) == Some(&'S')
            && b.get(j + 3) == Some(&' '))
        {
            continue;
        }
        j += 4;
        let Some((alias, _)) = quoted(j) else {
            continue;
        };
        out.push((name, alias));
    }
    out
}

/// **The check that makes forgetting a space predicate impossible to ship.**
///
/// Every statement this crate emits runs through it before it is returned,
/// so a missing predicate is a hard planner refusal rather than a
/// cross-tenant read. Two obligations:
///
/// 1. Every alias in a source position was registered by the planner —
///    physical or derived. A new source site that renders a table name by
///    hand instead of going through [`Planner::table`] registers nothing
///    and fails here.
/// 2. Every physical alias is constrained: `"alias"."$space"` appears.
///
/// This is a belt over the braces of centralised emission, not a
/// substitute for it: the predicate is emitted by the same function that
/// renders the source, so the ordinary way to add a source is already the
/// correct one. What this catches is the extraordinary way.
fn verify_scoped(sql: &str, sources: &Sources) -> Result<(), PlanError> {
    for (name, alias) in scan_sources(sql) {
        if sources.physical.iter().any(|a| a == &alias) {
            continue;
        }
        if sources.derived.iter().any(|a| a == &alias) {
            continue;
        }
        return Err(PlanError {
            code: "unscoped_source",
            message: format!(
                "`{name}` is read as `{alias}` by a source site the planner never registered, \
                 so nothing confined it to a space — render table sources through \
                 `Planner::table`, which emits the source and its space predicate together"
            ),
        });
    }
    for alias in &sources.physical {
        if !sql.contains(&format!("{}.{}", q(alias), q(SPACE_COLUMN))) {
            return Err(PlanError {
                code: "unscoped_source",
                message: format!(
                    "the base table read as `{alias}` carries no space predicate — every \
                     physical source is confined to one space, and a source without one is a \
                     cross-tenant read"
                ),
            });
        }
    }
    Ok(())
}

impl core::convert::From<SchemaError> for PlanError {
    fn from(e: SchemaError) -> Self {
        PlanError {
            code: e.code,
            message: e.message,
        }
    }
}

/// The longest `matches` pattern a statement runs when the session sets no
/// [`REGEX_MAX_LEN_GUC`]: 256 characters.
pub const REGEX_MAX_LEN_DEFAULT: usize = 256;
/// The session setting the `matches` guard reads its length cap from. The
/// pool sets it on every connection it dials (`ORDO_REGEX_MAX_LEN`).
pub const REGEX_MAX_LEN_GUC: &str = "ordo.regex_max_len";

/// The pattern of a `matches`, guarded: a pattern longer than the cap, or
/// holding a backreference (`\1` to `\9`, the one construct that makes
/// Postgres's regex engine backtrack without bound), becomes `(`, an invalid
/// pattern, so the statement fails with SQLSTATE 2201B before any row is
/// tried. The pool maps 2201B to `invalid_param` naming these rules. A NULL
/// pattern stays NULL (the guard's condition is NULL, so the ELSE keeps it).
///
/// The backreference test is conservative: an escaped backslash before a
/// digit (`\\1`, a literal backslash then `1`) is refused as well.
pub fn regex_guard(r: &str) -> String {
    format!(
        "(CASE WHEN NOT (length({r}) <= COALESCE(NULLIF(current_setting('{REGEX_MAX_LEN_GUC}', \
         true), '')::int, {REGEX_MAX_LEN_DEFAULT}) AND {r} !~ '\\\\[1-9]') THEN '(' ELSE {r} END)"
    )
}

/// A planned operation: the statement, its parameter slots in `$n` order, and
/// the result shape the harness decodes with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub sql: String,
    pub params: Vec<String>,
    pub outputs: Vec<(String, ScalarTy)>,
    /// Outputs the host must open before the caller sees them:
    /// `(output name, entity, field)`. Carried from the bound query so the
    /// executor never has to re-derive which columns are envelopes.
    pub host_open: Vec<(String, String, String)>,
}

/// Split a sealed output cell the planner packed (`[row id][0x00]
/// [envelope]`) into the id of the row it was read from and the envelope.
///
/// The host opens the envelope at that row. `None` when the cell holds no
/// separator, which is a cell that did not come from a plan of this crate
/// and is never opened.
pub fn split_anchored(cell: &[u8]) -> Option<(&str, &[u8])> {
    let at = cell.iter().position(|b| *b == 0)?;
    let id = std::str::from_utf8(&cell[..at]).ok()?;
    Some((id, &cell[at + 1..]))
}

/// What makes an expression never null. See [`never_null`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeverNull {
    /// A column the database declares `NOT NULL`, on a row that is certain
    /// to exist where it is read.
    Column,
    /// A value that is not a column: a non-null literal, `$caller`, a digest,
    /// or an operation over never-null inputs that cannot answer null.
    Value,
}

/// Where [`never_null`] looks for its evidence.
pub struct NullScope<'a> {
    /// Table to its `NOT NULL` columns ([`Query::not_null`]). `None` proves
    /// no column.
    pub not_null:
        Option<&'a std::collections::BTreeMap<String, std::collections::BTreeSet<String>>>,
    /// `(alias, table)` of the sources whose row is certain to exist where
    /// the expression is evaluated, innermost last.
    pub present: &'a [(String, String)],
}

/// **The never-null rule**, the one place it is decided (owner ruling
/// 2026-10-26, `guard-equality-uses-no-index`). `Some` only with proof;
/// anything unproven is `None`, and a comparison with no proven side keeps
/// the null-safe form.
///
/// An expression is never null when it is:
///
/// - a column of a certain row (see `Planner::enter_level`: the `FROM`
///   table of an enclosing level, or a dot-hop through a `NOT NULL` ref
///   with no guard in its `ON`) that the database declares `NOT NULL`. A
///   field that is not optional, `id`, a blind-index digest column.
///   A hop through an optional ref, a guarded hop and a multi-source join
///   target are not certain, so their columns are not proven;
/// - a literal other than `null`, a resolved digest, or `$caller`;
/// - negation, arithmetic (`/` by zero raises, it does not answer null),
///   `lower`, `length`, `abs`, `trim` or `hash` over never-null inputs, or
///   `coalesce` with a never-null argument.
///
/// Parameters other than `$caller` are not proven: one bound at the call
/// site is already a literal here, and an unbound slot may be optional.
pub fn never_null(e: &BExpr, scope: &NullScope<'_>) -> Option<NeverNull> {
    let all = |xs: &[&BExpr]| xs.iter().all(|x| never_null(x, scope).is_some());
    match e {
        BExpr::Col { alias, column, .. } => {
            let (_, table) = scope.present.iter().rev().find(|(a, _)| a == alias)?;
            let declared = scope.not_null?.get(table)?.contains(column);
            declared.then_some(NeverNull::Column)
        }
        BExpr::Lit {
            value: ordo_syntax::Expr::NullLit { .. },
            ..
        } => None,
        BExpr::Lit { .. } | BExpr::ByteaLit { .. } | BExpr::BlindDigest { .. } => {
            Some(NeverNull::Value)
        }
        BExpr::Param { name, .. } if name == "caller" => Some(NeverNull::Value),
        BExpr::Neg { expr, .. } => all(&[expr]).then_some(NeverNull::Value),
        BExpr::Binary { lhs, rhs, .. } => all(&[lhs, rhs]).then_some(NeverNull::Value),
        BExpr::Fn { name, args, .. } => {
            let args: Vec<&BExpr> = args.iter().collect();
            let proven = match name.as_str() {
                "lower" | "length" | "abs" | "trim" | "hash" => !args.is_empty() && all(&args),
                "coalesce" => args.iter().any(|a| never_null(a, scope).is_some()),
                _ => false,
            };
            proven.then_some(NeverNull::Value)
        }
        _ => None,
    }
}

/// Render one bound condition on its own — for `whole`, which composes a
/// guard into a hand-built SELECT rather than going through `plan`.
///
/// The guard body can itself reach base tables (a `match any` semi-join),
/// so it takes the space like everything else, and `whole` verifies the
/// statement it splices this into.
pub fn render_cond(c: &BCond, space: &Space) -> Result<(String, Sourced), PlanError> {
    let mut p = Planner::new(space.clone());
    let sql = p.cond(c)?;
    Ok((sql, Sourced(p.sources)))
}

/// Render one bound expression on its own — **the computed write RHS**
/// (DL §6, "RHS field references"; §3, "Legal anywhere expressions are:
/// projections, write RHS …").
///
/// This is deliberately the same [`Planner::expr`] a read goes through and
/// not a second renderer. DL says an expression is one thing wherever it is
/// legal, so `used + 1` in a write has to be the SQL `used + 1` in a read:
/// the same `+`-is-concat-over-text rule, the same `/` → `round(numeric,
/// 12)` division rule that keeps `bigint / bigint` from silently
/// truncating, the same money-as-minor-units arithmetic. A copy of this
/// switch for writes would be a second dialect that agrees on the day it is
/// written and drifts thereafter — and the drift would be *silent*, because
/// both halves would keep producing numbers.
///
/// Unlike [`render_cond`] there is nothing to verify afterwards: a write's
/// right-hand side sees the row it is writing and nothing else, so it never
/// introduces a source (the binder refuses the dot-path that would), and the
/// statement it is spliced into carries its own space predicate. The
/// assertion is here rather than assumed: a fragment that DID name a table
/// would be an unscoped read inside a write, which is precisely what
/// `SpaceAudit` exists to make impossible.
pub fn render_write_value(e: &BExpr, space: &Space) -> Result<String, PlanError> {
    let mut p = Planner::new(space.clone());
    let before = p.params.len();
    let sql = p.expr(e)?;
    if !p.sources.physical.is_empty() || !p.sources.derived.is_empty() {
        return Err(PlanError {
            code: "invalid_binding_expr",
            message: "a write's right-hand side reads the row being written and nothing \
                      else — this one reached a table"
                .into(),
        });
    }
    if p.params.len() != before {
        return Err(PlanError {
            code: "invalid_binding_expr",
            message: "a write's right-hand side is rendered with its parameters already \
                      bound; an unbound slot has no value to write"
                .into(),
        });
    }
    Ok(sql)
}

/// Sources a rendered fragment introduced, handed back so the statement
/// that splices the fragment in can verify the whole of it.
pub struct Sourced(Sources);

impl Sourced {
    /// Base-table aliases the fragment reads, each of which owes a space
    /// predicate in the statement this is spliced into.
    pub fn physical_aliases(&self) -> Vec<String> {
        self.0.physical.clone()
    }
}

/// [`verify_scoped`] for a hand-built statement (`whole`): every alias it
/// names is physical, so there is nothing derived to exempt.
///
/// Public so a test can pose the negative directly — an unscoped statement
/// must be refused, and the only honest way to show that is to hand one
/// over. The checker is otherwise reached by every entry point in this
/// crate on its own way out.
pub fn verify_whole(sql: &str, physical: &[String]) -> Result<(), PlanError> {
    let mut s = Sources::default();
    for a in physical {
        s.physical(a);
    }
    verify_scoped(sql, &s)
}

pub fn plan(query: &Query, space: &Space) -> Result<Plan, PlanError> {
    plan_with(query, space, !PROVEN_EQUALITY)
}

/// Whether the planner compiles `==` by the never-null rule ([`never_null`])
/// or keeps every equality null-safe.
const PROVEN_EQUALITY: bool = true;

/// [`plan`] with equality compiled by the never-null rule, whatever the
/// default. For the parity test.
#[doc(hidden)]
pub fn plan_proven_equality(query: &Query, space: &Space) -> Result<Plan, PlanError> {
    plan_with(query, space, false)
}

/// [`plan`] with every equality in the null-safe `IS [NOT] DISTINCT FROM`
/// form, as before the never-null rule. For the parity test only, which
/// runs both and requires the same rows.
#[doc(hidden)]
pub fn plan_legacy_equality(query: &Query, space: &Space) -> Result<Plan, PlanError> {
    plan_with(query, space, true)
}

fn plan_with(query: &Query, space: &Space, legacy_eq: bool) -> Result<Plan, PlanError> {
    let mut p = Planner::new(space.clone());
    p.legacy_eq = legacy_eq;
    let sql = p.select(query, 0)?;
    let outputs = query
        .outputs
        .iter()
        .map(|o| (o.name.clone(), o.expr.ty()))
        .collect();
    let sql = format!("{sql};");
    verify_scoped(&sql, &p.sources)?;
    Ok(Plan {
        sql,
        params: p.params,
        outputs,
        host_open: query.host_open.clone(),
    })
}

/// A plan prepared to serve one page of a total order.
pub struct PagedPlan {
    pub sql: String,
    pub outputs: Vec<(String, ScalarTy)>,
    /// Trailing hidden columns carrying this row's order keys — the
    /// executor reads them off the last row to mint the next cursor, and
    /// strips them before decoding. Zero when every key is already an
    /// output (which is always the case after `group`/`distinct`).
    pub key_count: usize,
}

/// Keyset pagination. `after` is the previous page's last-row key values
/// (text, `None` for SQL NULL) in order-key order.
///
/// Keyset rather than OFFSET because the language already guarantees a
/// TOTAL order on every plan (declared keys, then the tiebreak, NULLS LAST
/// throughout): a total order is exactly what makes "strictly after this
/// row" a well-defined, index-friendly predicate that neither skips nor
/// repeats a row when the table changes under a paging client. OFFSET has
/// neither property.
///
/// Sort keys need not be projected (ruling W047), so any key that is not
/// already an output rides as a hidden trailing column — the client never
/// sees it, and the cursor is minted from it server-side.
pub fn plan_paged(
    query: &Query,
    space: &Space,
    page_size: i64,
    after: Option<&[Option<String>]>,
) -> Result<PagedPlan, PlanError> {
    let mut p = Planner::new(space.clone());
    p.sealed = Planner::sealed_of(query);
    let terms = p.order_terms(query)?;
    if terms.is_empty() {
        return Err(PlanError {
            code: "unpageable",
            message: "a plan with no ordering cannot be paged".into(),
        });
    }
    // Keys already projected are addressed by their output name; the rest
    // ride as hidden columns.
    //
    // Matched on the bound EXPRESSION, not on rendered SQL: the rendered
    // term now carries `COLLATE "C"` and the projection does not, so a
    // string comparison would miss every text key and push it into a
    // hidden column duplicating an output it already had.
    let mut inner = query.clone();
    let mut selectors: Vec<String> = Vec::new();
    let mut hidden = 0usize;
    for (i, (_, key, _)) in terms.iter().enumerate() {
        match output_named(query, key) {
            Some(name) => selectors.push(q(&name)),
            None => {
                let alias = format!("$k{i}");
                inner.outputs.push(ordo_sema::ir::OutField {
                    name: alias.clone(),
                    expr: key.clone(),
                });
                selectors.push(q(&alias));
                hidden += 1;
            }
        }
    }
    // The outer query re-orders over the subquery's columns, so the pin
    // has to be restated there: a bare `"name"` outside would take the
    // deployment's collation even though the inner ORDER BY took `C`. The
    // seek predicate and the ORDER BY use the SAME rendered key, because
    // "strictly after this row" is only true when both sides are asking
    // one ordering.
    let keys: Vec<String> = selectors
        .iter()
        .zip(&terms)
        .map(|(c, (_, key, _))| {
            if p.is_collatable(key) {
                collate(c)
            } else {
                c.clone()
            }
        })
        .collect();
    // The inner query keeps its own ordering and any `take` limit; paging
    // ranges over that settled result, never around it.
    let body = p.select(&inner, 0)?;
    // The page wrapper reads the settled inner result, which is already
    // confined to one space; nothing physical is reached here.
    p.sources.derived("$page");
    let mut sql = format!("SELECT * FROM (\n{body}\n) AS {}", q("$page"));
    if let Some(vals) = after {
        if vals.len() != terms.len() {
            return Err(PlanError {
                code: "invalid_cursor",
                message: format!(
                    "cursor carries {} keys, the order has {}",
                    vals.len(),
                    terms.len()
                ),
            });
        }
        sql.push_str(&format!("\nWHERE {}", seek(&keys, &terms, vals)));
    }
    let order: Vec<String> = keys
        .iter()
        .zip(&terms)
        .map(|(c, (_, _, desc))| format!("{c} {} NULLS LAST", if *desc { "DESC" } else { "ASC" }))
        .collect();
    sql.push_str(&format!(
        "\nORDER BY {}\nLIMIT {page_size};",
        order.join(", ")
    ));
    verify_scoped(&sql, &p.sources)?;
    Ok(PagedPlan {
        sql,
        outputs: query
            .outputs
            .iter()
            .map(|o| (o.name.clone(), o.expr.ty()))
            .collect(),
        key_count: hidden,
    })
}

/// For each order key in order, the OUTPUT NAME that already carries it,
/// or `None` when `plan_paged` gives it a hidden column. The executor
/// needs the same answer to read a cursor off a row, so both sides derive
/// it here rather than agreeing by convention.
/// The position of the column `alias.column` among this query's order
/// terms, if it is one of them.
///
/// Paired with [`order_key_outputs`], which says WHICH terms ride hidden;
/// this says WHERE a particular key sits among them. Both come from the
/// same `order_terms` walk, for the reason `output_named` is one
/// definition: the SQL, the cursor reader and anything attaching a key
/// back onto a row must never disagree about term order.
///
/// Matched on alias and column rather than on a whole `BExpr`, because the
/// caller reconstructing the expression would have to guess its `ScalarTy`
/// and a mismatch would read as "not an order key" rather than failing.
///
/// Hands back the term's own expression as well as its position, so a caller
/// that wants to PROJECT the key clones what the planner already bound.
/// `output_named` compares whole expressions, type included, so a rebuilt one
/// would fail to match its own order term and ride both projected and hidden.
pub fn order_key_of_col(query: &Query, alias: &str, column: &str) -> Option<(usize, BExpr)> {
    let mut p = Planner::new(Space::Slot);
    p.sealed = Planner::sealed_of(query);
    let terms = p.order_terms(query).ok()?;
    terms.iter().enumerate().find_map(|(i, (_, key, _))| {
        matches!(key, BExpr::Col { alias: a, column: c, .. } if a == alias && c == column)
            .then(|| (i, key.clone()))
    })
}

pub fn order_key_outputs(query: &Query) -> Vec<Option<String>> {
    // A rendering detail of the ORDER BY, not a statement: no source is
    // reached, so the space is irrelevant and a placeholder is honest.
    let mut p = Planner::new(Space::Slot);
    p.sealed = Planner::sealed_of(query);
    let Ok(terms) = p.order_terms(query) else {
        return Vec::new();
    };
    terms
        .iter()
        .map(|(_, key, _)| output_named(query, key))
        .collect()
}

/// The output that already projects this order key, if any. One
/// definition, used by both `plan_paged` and `order_key_outputs`, so the
/// executor's cursor reader and the SQL can never disagree about which
/// keys ride hidden.
fn output_named(query: &Query, key: &BExpr) -> Option<String> {
    query
        .outputs
        .iter()
        .find(|o| &o.expr == key)
        .map(|o| o.name.clone())
}

/// The lexicographic "strictly after" predicate, spelled out rather than
/// using row-value comparison: `(a, b) > (x, y)` is wrong the moment the
/// keys disagree in direction, and wrong again for NULLs. NULLS LAST is
/// the language's rule in both directions, so a NULL key sorts after every
/// value and ties with another NULL.
fn seek(cols: &[String], terms: &[(String, BExpr, bool)], vals: &[Option<String>]) -> String {
    let lit = |v: &Option<String>| match v {
        None => "NULL".to_string(),
        Some(s) => format!("'{}'", s.replace('\'', "''")),
    };
    let eq = |i: usize| match &vals[i] {
        None => format!("{} IS NULL", cols[i]),
        Some(_) => format!("{} = {}", cols[i], lit(&vals[i])),
    };
    let after = |i: usize| match &vals[i] {
        // The value is already last in the order; nothing follows it.
        None => "false".to_string(),
        Some(_) => format!(
            "({} {} {} OR {} IS NULL)",
            cols[i],
            if terms[i].2 { "<" } else { ">" },
            lit(&vals[i]),
            cols[i]
        ),
    };
    let mut disjuncts = Vec::new();
    for i in 0..cols.len() {
        let mut conj: Vec<String> = (0..i).map(eq).collect();
        conj.push(after(i));
        disjuncts.push(format!("({})", conj.join(" AND ")));
    }
    disjuncts.join("\n   OR ")
}

struct Planner {
    params: Vec<String>,
    /// Output expressions whose SQL type is `bytea`, not text: the sealed
    /// columns the host must open. They carry `ScalarTy::Text` in the IR
    /// because that is the LANGUAGE type of the value inside the envelope,
    /// so the type alone cannot be trusted to decide collatability —
    /// `COLLATE` on a `bytea` is a hard Postgres error. Set per select
    /// level from `Query::host_open`.
    sealed: Vec<BExpr>,
    /// The one space this statement is confined to.
    space: Space,
    /// Source aliases, so the emitted statement can be verified.
    sources: Sources,
    /// The database's `NOT NULL` columns, adopted from the outermost query
    /// (see [`Query::not_null`]). `None` proves nothing.
    not_null: Option<ordo_sema::ir::NotNullColumns>,
    /// `(alias, table)` for every source whose row is certain to exist
    /// wherever a condition at this point is evaluated: the `FROM` table of
    /// each enclosing level and the dot-hops that cannot come back empty.
    /// Innermost last. See [`never_null`].
    present: Vec<(String, String)>,
    /// Set under a `not`, inside a `NOT EXISTS`, and everything nested in
    /// either. There a comparison's null answer would be inverted rather
    /// than dropped, so only the forms that never answer null are used.
    strict: bool,
    /// Compile every equality to the null-safe distinctness form, as the
    /// planner did before the never-null rule. Only the parity test sets it.
    legacy_eq: bool,
}

impl Planner {
    fn new(space: Space) -> Self {
        let mut params = Vec::new();
        // **`$1` is always the space.** Claiming the slot before anything
        // else can take it means an executor binding parameters has one
        // rule to remember rather than a bind order to consult, and a
        // statement whose first bind is not the space is malformed on its
        // face.
        if space == Space::Slot {
            params.push(SPACE_SLOT.to_string());
        }
        Planner {
            params,
            sealed: Vec::new(),
            space,
            sources: Sources::default(),
            not_null: None,
            present: Vec::new(),
            strict: false,
            legacy_eq: !PROVEN_EQUALITY,
        }
    }

    /// The space value as SQL — a literal when the door bound one, the
    /// reserved `$1` slot otherwise.
    fn space_sql(&mut self) -> String {
        match self.space.clone() {
            Space::Bound(s) => format!("'{}'", s.replace('\'', "''")),
            Space::Slot => {
                let i = self
                    .params
                    .iter()
                    .position(|p| p == SPACE_SLOT)
                    .expect("the slot is claimed in `new`");
                format!("${}", i + 1)
            }
        }
    }

    /// **The only way to name a base table in emitted SQL.**
    ///
    /// It returns the source *and* the predicate that confines it, as one
    /// value, so the caller cannot take the first and forget the second —
    /// there is nothing to forget, because there is nothing to take
    /// separately. It also registers the alias, which is what lets
    /// [`verify_scoped`] find a source that came from anywhere else.
    fn table(&mut self, entity: &str, alias: &str) -> (String, String) {
        self.sources.physical(alias);
        let sp = self.space_sql();
        (
            format!("{} AS {}", q(entity), q(alias)),
            format!("{}.{} = {sp}", q(alias), q(SPACE_COLUMN)),
        )
    }

    /// The sealed outputs of one query level, by expression.
    fn sealed_of(qy: &Query) -> Vec<BExpr> {
        qy.host_open
            .iter()
            .filter_map(|(name, _, _)| {
                qy.outputs
                    .iter()
                    .find(|o| &o.name == name)
                    .map(|o| o.expr.clone())
            })
            .collect()
    }

    /// The alias of the stored row a sealed output is read from, when this
    /// level reads it from a table.
    ///
    /// A sealed value opens only at the row it was sealed for, so the row's
    /// id has to travel with the envelope. A level that reads the column
    /// from a table packs the two together ([`Self::anchored`]). A level
    /// that reads it through a subquery, a union or a CTE reads what that
    /// level already packed, and passes it on untouched.
    fn sealed_source(&self, qy: &Query, expr: &BExpr) -> Option<String> {
        if !self.sealed.contains(expr) {
            return None;
        }
        let BExpr::Col { alias, .. } = expr else {
            return None;
        };
        let from_table = matches!(&qy.from, From::Table { alias: a, .. } if a == alias)
            || qy.joins.iter().any(|j| &j.alias == alias)
            || qy
                .source_joins
                .iter()
                .any(|j| matches!(&j.from, From::Table { alias: a, .. } if a == alias));
        from_table.then(|| alias.clone())
    }

    /// `[row id][0x00][envelope]`, as one `bytea`. A null envelope stays
    /// null, because `||` over a null is null, and so does the outer side
    /// of a join that found no row. Row ids are identifiers and never hold
    /// a NUL, so the first zero byte ends the id.
    fn anchored(alias: &str, sealed_sql: &str) -> String {
        format!(
            "(convert_to({}.{}::text, 'UTF8') || '\\x00'::bytea || {sealed_sql})",
            q(alias),
            q("id"),
        )
    }

    /// Render an expression, pinning [`TEXT_COLLATION`] when it is text.
    ///
    /// Every ordering site goes through here. Non-text keys are returned
    /// untouched: numbers, dates and booleans have no collation, and a
    /// sealed `bytea` would be refused by Postgres if one were emitted.
    fn collated(&mut self, e: &BExpr) -> Result<String, PlanError> {
        let sql = self.expr(e)?;
        Ok(if self.is_collatable(e) {
            collate(&sql)
        } else {
            sql
        })
    }

    fn is_collatable(&self, e: &BExpr) -> bool {
        matches!(e.ty(), ScalarTy::Text | ScalarTy::Id) && !self.sealed.contains(e)
    }

    fn select(&mut self, qy: &Query, depth: usize) -> Result<String, PlanError> {
        if self.not_null.is_none() {
            self.not_null = qy.not_null.clone();
        }
        // Sealed outputs are a per-LEVEL fact; a nested select has its own
        // host_open, so save and restore rather than accumulating.
        let outer_sealed = std::mem::replace(&mut self.sealed, Self::sealed_of(qy));
        // So are the rows this level makes certain. They are in scope for
        // everything nested below (a correlated EXISTS reads the outer
        // row), and gone once the level is closed.
        let mark = self.present.len();
        self.enter_level(qy);
        let out = self.select_inner(qy, depth);
        self.present.truncate(mark);
        self.sealed = outer_sealed;
        out
    }

    /// Record the sources of one query level whose row is certain.
    ///
    /// - The `FROM` table: a condition at this level is evaluated against
    ///   one of its rows. A derived source (sub-select, CTE, union) is not
    ///   recorded: its columns are outputs this rule does not look inside.
    /// - A dot-hop, only when it has no read guard of its own and hops from
    ///   a certain row through a ref column the database declares `NOT
    ///   NULL`. The column's composite foreign key then names a row in the
    ///   same space, so the `LEFT JOIN` always finds it. A hop with a guard
    ///   in its `ON` is null whenever the caller may not read the target
    ///   (`Join::guard`), so it is never certain.
    /// - Never a multi-source join target: it is the right side of a `LEFT
    ///   JOIN` and null-extended when nothing matches.
    fn enter_level(&mut self, qy: &Query) {
        if let From::Table { entity, alias } = &qy.from {
            self.present.push((alias.clone(), entity.clone()));
        }
        for j in &qy.joins {
            if j.guard.is_some() {
                continue;
            }
            let via = BExpr::Col {
                alias: j.from_alias.clone(),
                column: j.from_column.clone(),
                ty: ScalarTy::Id,
            };
            if self.never_null(&via) == Some(NeverNull::Column) {
                self.present.push((j.alias.clone(), j.table.clone()));
            }
        }
    }

    /// [`never_null`] over this planner's evidence and scope.
    fn never_null(&self, e: &BExpr) -> Option<NeverNull> {
        never_null(
            e,
            &NullScope {
                not_null: self.not_null.as_deref(),
                present: &self.present,
            },
        )
    }

    /// Render `f` with `alias` (a `table` row) certain, for the `ON` of the
    /// join that introduces it: an `ON` is evaluated against candidate rows
    /// that exist, never against the null row an outer join pads with.
    fn with_present<T>(
        &mut self,
        alias: &str,
        table: &str,
        f: impl FnOnce(&mut Self) -> Result<T, PlanError>,
    ) -> Result<T, PlanError> {
        self.present.push((alias.to_string(), table.to_string()));
        let out = f(self);
        self.present.pop();
        out
    }

    /// Render `f` in strict mode (see [`Planner::strict`]).
    fn strictly<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, PlanError>,
    ) -> Result<T, PlanError> {
        let was = std::mem::replace(&mut self.strict, true);
        let out = f(self);
        self.strict = was;
        out
    }

    /// `l == r` (or `l != r` when `negated`), already rendered as `ls` and
    /// `rs`, in the cheapest spelling that returns exactly the rows `IS
    /// [NOT] DISTINCT FROM` returns. See the comment in [`Planner::cond`].
    fn equality(&self, (l, ls): (&BExpr, &str), (r, rs): (&BExpr, &str), negated: bool) -> String {
        let safe = || {
            if negated {
                format!("({ls} IS DISTINCT FROM {rs})")
            } else {
                format!("({ls} IS NOT DISTINCT FROM {rs})")
            }
        };
        if self.legacy_eq {
            return safe();
        }
        let (nl, nr) = (self.never_null(l), self.never_null(r));
        let plain = || {
            if negated {
                format!("({ls} <> {rs})")
            } else {
                format!("({ls} = {rs})")
            }
        };
        if self.strict {
            // Under a negation only a comparison that cannot answer null
            // is exact, and the owner's rule asks more than that here:
            // both sides backed by a database `NOT NULL`.
            return if nl == Some(NeverNull::Column) && nr == Some(NeverNull::Column) {
                plain()
            } else {
                safe()
            };
        }
        match (nl, nr, negated) {
            // Both sides are values: `=` and `<>` are already two-valued.
            (Some(_), Some(_), _) => plain(),
            // One value. `a = b` answers null only when `b` is null, where
            // `IS NOT DISTINCT FROM` answers false, and a filter drops the
            // row either way. This is the spelling an index serves.
            (Some(_), None, false) | (None, Some(_), false) => plain(),
            // `!=` with one side possibly null: true exactly when they
            // differ or that side is null, and never null itself.
            (Some(_), None, true) => format!("(NOT ({ls} = {rs}) OR {rs} IS NULL)"),
            (None, Some(_), true) => format!("(NOT ({ls} = {rs}) OR {ls} IS NULL)"),
            // Both possibly null, in a filter: equal, or both null. When
            // they differ and one is null this answers null where the old
            // form answers false, and a filter drops the row either way.
            // Measured 2026-10-26 inside an EXISTS over 50,000 rows: Postgres
            // serves it with a BitmapOr over the index (`= ` and `IS NULL`
            // arms), 1.0 s against 4.5 s for `IS NOT DISTINCT FROM`, which
            // it can only filter row by row.
            (None, None, false) => format!("({ls} = {rs} OR ({ls} IS NULL AND {rs} IS NULL))"),
            // `!=` with neither side proven: unchanged.
            (None, None, true) => safe(),
        }
    }

    fn select_inner(&mut self, qy: &Query, depth: usize) -> Result<String, PlanError> {
        let pad = "  ".repeat(depth);
        let mut s = String::new();

        if !qy.ctes.is_empty() {
            let parts: Result<Vec<String>, PlanError> = qy
                .ctes
                .iter()
                .map(|c| {
                    Ok(format!(
                        "{} AS (\n{}\n{pad})",
                        q(&c.name),
                        self.select(&c.query, depth + 1)?
                    ))
                })
                .collect();
            s.push_str(&format!("{pad}WITH {}\n", parts?.join(", ")));
        }

        s.push_str(&format!(
            "{pad}SELECT{}\n",
            if qy.distinct { " DISTINCT" } else { "" }
        ));
        if qy.outputs.is_empty() {
            return Err(PlanError {
                code: "empty_projection",
                message: "a query must produce at least one output field".into(),
            });
        }
        let cols: Result<Vec<String>, PlanError> = qy
            .outputs
            .iter()
            .map(|o| {
                // Under `SELECT DISTINCT`, Postgres requires every ORDER BY
                // expression to APPEAR IN THE SELECT LIST, and `x` and
                // `x COLLATE "C"` are not the same expression — so a
                // collated ordering over an uncollated projection is a hard
                // error ("for SELECT DISTINCT, ORDER BY expressions must
                // appear in select list"). The projection is spelled the
                // same way the ordering is. This changes no VALUE: DISTINCT
                // itself dedupes by byte equality either way (see
                // `TEXT_COLLATION`), so the collate here is syntax, not
                // semantics.
                let e = if qy.distinct {
                    self.collated(&o.expr)?
                } else {
                    self.expr(&o.expr)?
                };
                let e = match self.sealed_source(qy, &o.expr) {
                    Some(alias) => Self::anchored(&alias, &e),
                    None => e,
                };
                Ok(format!("{pad}  {e} AS {}", q(&o.name)))
            })
            .collect();
        s.push_str(&cols?.join(",\n"));
        s.push('\n');

        // Space predicates for the sources this level introduces, ahead of
        // the query's own filters. They are conjuncts of the WHERE for the
        // FROM source and conjuncts of the ON for every join target — an
        // outer join's predicate belongs in its ON or the join stops being
        // outer and the nullable-ref rows vanish.
        let mut scoping: Vec<String> = Vec::new();

        s.push_str(&format!("{pad}FROM "));
        match &qy.from {
            From::Table { entity, alias } => {
                let (src, pred) = self.table(entity, alias);
                s.push_str(&format!("{src}\n"));
                scoping.push(pred);
            }
            From::Sub { query, alias } => {
                self.sources.derived(alias);
                s.push_str("(\n");
                s.push_str(&self.select(query, depth + 1)?);
                s.push_str(&format!("\n{pad}) AS {}\n", q(alias)));
            }
            From::CteRef { name, alias } => {
                self.sources.derived(alias);
                s.push_str(&format!("{} AS {}\n", q(name), q(alias)));
            }
            From::Union { arms, alias } => {
                self.sources.derived(alias);
                let parts: Result<Vec<String>, PlanError> = arms
                    .iter()
                    .map(|a| Ok(format!("(\n{}\n{pad})", self.select(a, depth + 1)?)))
                    .collect();
                s.push_str(&format!(
                    "(\n{pad}{}\n{pad}) AS {}\n",
                    parts?.join(&format!("\n{pad}UNION ALL\n{pad}")),
                    q(alias)
                ));
            }
        }

        for j in &qy.source_joins {
            // Always LEFT: the multi-source form is outer-preserving.
            let mut extra: Vec<String> = Vec::new();
            let target = match &j.from {
                From::Table { entity, alias } => {
                    let (src, pred) = self.table(entity, alias);
                    extra.push(pred);
                    src
                }
                From::CteRef { name, alias } => {
                    self.sources.derived(alias);
                    format!("{} AS {}", q(name), q(alias))
                }
                From::Sub { query, alias } => {
                    self.sources.derived(alias);
                    format!(
                        "(\n{}\n{pad}) AS {}",
                        self.select(query, depth + 1)?,
                        q(alias)
                    )
                }
                From::Union { .. } => {
                    return Err(PlanError {
                        code: "unsupported",
                        message: "a union cannot be a join target".into(),
                    })
                }
            };
            let on: Result<Vec<String>, PlanError> =
                j.on.iter()
                    .map(|(a, b)| Ok(format!("{} = {}", self.expr(a)?, self.expr(b)?)))
                    .collect();
            let mut on = on?;
            on.extend(extra);
            // A bridge's read guard, in the ON for `Join`'s reason below: a
            // bridge row the caller may not read must look like a bridge row
            // that is not there, and a WHERE on the right-hand alias would
            // drop the near row the outer join exists to keep (S6-001).
            if let Some(g) = &j.guard {
                let g = match &j.from {
                    From::Table { entity, alias } => {
                        self.with_present(alias, entity, |p| p.cond(g))?
                    }
                    _ => self.cond(g)?,
                };
                on.push(g);
            }
            s.push_str(&format!(
                "{pad}LEFT JOIN {target} ON {}\n",
                on.join(" AND ")
            ));
        }

        for j in &qy.joins {
            // A dot-hop is a ref traversal, and a ref cannot cross spaces
            // (the foreign key is composite). The predicate is still
            // emitted: the plan must be correct on its own terms, not by
            // appeal to a constraint the reader has to go and check — and
            // it is what lets the join use the `(space, id)` primary key.
            //
            // In the ON, not the WHERE: this join is LEFT because dotting
            // through a nullable ref must keep the row, and a WHERE
            // predicate on the right-hand alias would discard exactly the
            // rows the outer join preserved.
            //
            // The target's READ GUARD goes in the same place for the same
            // reason (GC007 / S6-001): a target the caller may not read is
            // then indistinguishable from a ref that is null. The row stays
            // and every column reached through the hop is null.
            let (src, pred) = self.table(&j.table, &j.alias);
            let guard = match &j.guard {
                Some(g) => format!(
                    " AND {}",
                    self.with_present(&j.alias, &j.table, |p| p.cond(g))?
                ),
                None => String::new(),
            };
            s.push_str(&format!(
                "{pad}LEFT JOIN {src} ON {}.{} = {}.{} AND {pred}{guard}\n",
                q(&j.from_alias),
                q(&j.from_column),
                q(&j.alias),
                q("id"),
            ));
        }

        let filters: Result<Vec<String>, PlanError> =
            qy.filters.iter().map(|c| self.cond(c)).collect();
        scoping.extend(filters?);
        if !scoping.is_empty() {
            s.push_str(&format!("{pad}WHERE {}\n", scoping.join("\n  AND ")));
        }

        if let Some(g) = &qy.group {
            // A keyless group is `reduce`: aggregates with no GROUP BY
            // clause — SQL's one-row aggregation — not `GROUP BY ()`.
            if !g.keys.is_empty() {
                let keys: Result<Vec<String>, PlanError> =
                    g.keys.iter().map(|k| self.expr(k)).collect();
                s.push_str(&format!("{pad}GROUP BY {}\n", keys?.join(", ")));
            }
        }
        if !qy.having.is_empty() {
            let parts: Result<Vec<String>, PlanError> =
                qy.having.iter().map(|c| self.cond(c)).collect();
            s.push_str(&format!("{pad}HAVING {}\n", parts?.join("\n  AND ")));
        }

        let order = self.order_by(qy)?;
        if !order.is_empty() {
            s.push_str(&format!("{pad}ORDER BY {}\n", order.join(", ")));
        }
        if let Some(n) = &qy.limit {
            // `take $n` binds the count like any other parameter slot —
            // `LIMIT $3`, never an interpolated number. The door already
            // proved the value against the parameter's declared bound
            // (DL §2: "Bounds are enforced at the door, before planning"),
            // which is the same bound the registration gate used to prove
            // this op's row bound.
            let text = match n {
                Limit::Lit(v) => v.to_string(),
                Limit::Param { name, .. } => self.expr(&BExpr::Param {
                    name: name.clone(),
                    ty: ScalarTy::Int,
                })?,
            };
            s.push_str(&format!("{pad}LIMIT {text}\n"));
        }

        Ok(s.trim_end().to_string())
    }

    /// The final order as (expression SQL, its `BExpr`, descending) — what
    /// `order_by` renders and what keyset paging seeks over.
    ///
    /// The `BExpr` rides along deliberately. De-duplication here means the
    /// nth term is NOT the nth entry of `sort ++ tiebreak`, so a caller
    /// that re-derived the expression by index would silently pick the
    /// wrong column once any key repeated — and the one it lost is the
    /// anchor-id tiebreak, which is exactly the term that makes the order
    /// total. (Regression: `sort status, status` dropped rows when paged.)
    fn order_terms(&mut self, qy: &Query) -> Result<Vec<(String, BExpr, bool)>, PlanError> {
        let mut out: Vec<(String, BExpr, bool)> = Vec::new();
        for s in qy.sort.iter().cloned().chain(
            qy.tiebreak
                .iter()
                .cloned()
                .map(|expr| Sort { expr, desc: false }),
        ) {
            // The COLLATE rides on the rendered term, so the keyset seek
            // predicate compares under the same collation the ORDER BY
            // sorts under. They are the same string by construction, which
            // is the only way "strictly after this row" stays true.
            let e = self.collated(&s.expr)?;
            if out.iter().any(|(seen, _, _)| seen == &e) {
                continue;
            }
            out.push((e, s.expr.clone(), s.desc));
        }
        Ok(out)
    }

    /// Declared keys, then the implicit tiebreak. A key already present is not
    /// repeated — a duplicated ORDER BY term is noise in a golden file.
    fn order_by(&mut self, qy: &Query) -> Result<Vec<String>, PlanError> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();

        let mut push = |planner: &mut Self, s: &Sort| -> Result<(), PlanError> {
            let e = planner.collated(&s.expr)?;
            if seen.contains(&e) {
                return Ok(());
            }
            seen.push(e.clone());
            out.push(format!(
                "{e} {} NULLS LAST",
                if s.desc { "DESC" } else { "ASC" }
            ));
            Ok(())
        };

        for s in &qy.sort {
            push(self, s)?;
        }
        for k in qy.tiebreak.clone() {
            push(
                self,
                &Sort {
                    expr: k,
                    desc: false,
                },
            )?;
        }
        Ok(out)
    }

    /// A minimal SELECT 1 body for EXISTS — FROM, joins, WHERE only.
    fn select_exists(&mut self, qy: &Query) -> Result<String, PlanError> {
        let mut body = qy.clone();
        body.outputs = vec![];
        let mut s = String::from("  SELECT 1\n");
        let rest = self.select(
            &Query {
                outputs: vec![OutFieldOne::one()],
                ..body
            },
            1,
        )?;
        // Splice: drop the generated SELECT list line, keep FROM onward.
        let from = rest.find("FROM").ok_or(PlanError {
            code: "unsupported",
            message: "exists body has no FROM".into(),
        })?;
        s.push_str("  ");
        s.push_str(&rest[from..]);
        Ok(s.trim_end().to_string())
    }

    fn expr(&mut self, e: &BExpr) -> Result<String, PlanError> {
        Ok(match e {
            BExpr::Col { alias, column, .. } => format!("{}.{}", q(alias), q(column)),
            BExpr::Lit { value, .. } => literal(value)?,
            BExpr::Param { name, .. } => {
                if let Some(i) = self.params.iter().position(|p| p == name) {
                    format!("${}", i + 1)
                } else {
                    self.params.push(name.clone());
                    format!("${}", self.params.len())
                }
            }
            BExpr::Neg { expr, .. } => format!("(-{})", self.expr(expr)?),
            BExpr::DocPath { doc, keys, ty } => doc_read(&doc_at(&self.expr(doc)?, keys), *ty),
            BExpr::Binary { op, lhs, rhs, ty } => {
                let l = self.expr(lhs)?;
                let r = self.expr(rhs)?;
                if matches!(op, BinOp::Div) {
                    // `money` and `int` are both `bigint`, and `bigint /
                    // bigint` is TRUNCATING integer division — `total / qty`
                    // would answer with a wrong number rather than an error.
                    // DL §2 (Division: determinism across venues) pins the
                    // answer instead: evaluate in Postgres's UNCONSTRAINED
                    // `numeric` and apply an explicit `round(value, 12)`.
                    //
                    // The cast is on both operands so neither venue nor
                    // operand type can reintroduce integer division, and the
                    // scale is fixed at the division rather than inherited
                    // from the operands — the same expression therefore has
                    // the same scale in every plan and every deployment.
                    //
                    // Deliberately NO `nullif(r, 0)`: `/ 0` is a typed
                    // runtime error uniform with `mod(a, 0)`, and "there is
                    // no null-on-divide-by-zero folklore". Postgres raises
                    // SQLSTATE 22012, which `ordo-exec` types.
                    return Ok(at_scale(&format!("({l})::numeric / ({r})::numeric")));
                }
                // `+` doubles as concatenation; the type the binder inferred
                // is what decides which operator this actually is.
                let sym = match (op, ty) {
                    (BinOp::Add, ScalarTy::Text) => "||",
                    (BinOp::Add, _) => "+",
                    (BinOp::Sub, _) => "-",
                    (BinOp::Mul, _) => "*",
                    (BinOp::Div, _) => unreachable!("handled above"),
                };
                format!("({l} {sym} {r})")
            }
            BExpr::Fn { name, args, ty } => self.function(name, args, *ty)?,
            BExpr::Cond { arms, else_, .. } => {
                let mut s = String::from("CASE");
                for (c, v) in arms {
                    s.push_str(&format!(" WHEN {} THEN {}", self.cond(c)?, self.expr(v)?));
                }
                s.push_str(&format!(" ELSE {} END", self.expr(else_)?));
                s
            }
            BExpr::Agg {
                func,
                arg,
                arg2,
                order,
                mask,
                ..
            } => {
                let core = match (func.as_str(), arg) {
                    ("count", None) => "count(*)".to_string(),
                    ("median", Some(a)) => format!(
                        "percentile_cont(0.5) WITHIN GROUP (ORDER BY {})",
                        self.expr(a)?
                    ),
                    // The general form median is the 0.5 case of. The binder
                    // has already checked the fraction is a literal in (0,1).
                    ("percentile", Some(a)) => {
                        let p = arg2.as_ref().ok_or(PlanError {
                            code: "bad_aggregate",
                            message: "`percentile` needs a fraction".into(),
                        })?;
                        format!(
                            "percentile_cont({}) WITHIN GROUP (ORDER BY {})",
                            self.expr(p)?,
                            self.expr(a)?
                        )
                    }
                    // Count-distinct. NULLs are not a distinct value here,
                    // which is COUNT's own rule and the reason no mask is
                    // needed over an outer-preserved join.
                    ("distinct", Some(a)) => format!("count(DISTINCT {})", self.expr(a)?),
                    // Ordered string aggregation. The ORDER BY is inside the
                    // aggregate, so the concatenation order is the pipeline's
                    // and not whatever the executor happened to produce.
                    ("joined", Some(a)) => {
                        let sep = arg2.as_ref().ok_or(PlanError {
                            code: "bad_aggregate",
                            message: "`joined` needs a separator".into(),
                        })?;
                        // Ordered string aggregation is an ordering, so it
                        // takes the pin too: `joined(name, ", ")` must
                        // concatenate in the same sequence on every
                        // deployment or the STRING itself differs.
                        let mut ord = Vec::new();
                        for o in order {
                            ord.push(format!(
                                "{} {} NULLS LAST",
                                self.collated(&o.expr)?,
                                if o.desc { "DESC" } else { "ASC" }
                            ));
                        }
                        let inner = format!("{}, {}", self.expr(a)?, self.expr(sep)?);
                        if ord.is_empty() {
                            format!("string_agg({inner})")
                        } else {
                            format!("string_agg({inner} ORDER BY {})", ord.join(", "))
                        }
                    }
                    // `avg` is the other half of the division ruling: its
                    // result is `decimal`, so it evaluates in unconstrained
                    // `numeric` and is pinned to scale 12 below. The cast is
                    // on the argument because `avg(bigint)` picks its own
                    // scale from the operand — deployment-dependent output
                    // for a deployment-independent expression.
                    ("avg", Some(a)) => format!("avg(({})::numeric)", self.expr(a)?),
                    // `min`/`max` over text ARE an ordering — they return
                    // the first or last value in collation order, so an
                    // uncollated `max(name)` answers `B` on one deployment
                    // and `ab` on the next. Every other aggregate in the
                    // closed set is numeric or count-like and has no
                    // collation to take.
                    ("min" | "max", Some(a)) => format!("{func}({})", self.collated(a)?),
                    (f, Some(a)) => format!("{f}({})", self.expr(a)?),
                    (f, None) => {
                        return Err(PlanError {
                            code: "bad_aggregate",
                            message: format!("`{f}` needs an argument"),
                        })
                    }
                };
                let masked = match mask {
                    None => core,
                    Some(m) => {
                        // Ruling 3 amendment 2: the null-extension row is
                        // masked from every aggregate; sum also coalesces to
                        // 0 so an empty bucket sums to 0, not null.
                        let f = format!("{core} FILTER (WHERE {})", self.cond(m)?);
                        if func == "sum" {
                            format!("coalesce({f}, 0)")
                        } else {
                            f
                        }
                    }
                };
                // Outside the FILTER, so an empty set stays null rather than
                // rounding to 0 (`avg` of an empty set is null, DL §2).
                if func == "avg" {
                    at_scale(&masked)
                } else {
                    masked
                }
            }
            BExpr::RowNumber {
                partition, order, ..
            } => {
                let parts: Result<Vec<String>, PlanError> =
                    partition.iter().map(|e| self.expr(e)).collect();
                // `take[keys] n` picks the first n rows PER PARTITION by
                // this order. An uncollated text key here silently changes
                // WHICH rows survive, not merely their sequence.
                let mut ord: Vec<String> = Vec::new();
                for o in order {
                    ord.push(format!(
                        "{} {} NULLS LAST",
                        self.collated(&o.expr)?,
                        if o.desc { "DESC" } else { "ASC" }
                    ));
                }
                let parts = parts?;
                // No keys means one window over the whole arm; `PARTITION BY`
                // with nothing after it is a syntax error, not an empty
                // partition.
                if parts.is_empty() {
                    format!("ROW_NUMBER() OVER (ORDER BY {})", ord.join(", "))
                } else {
                    format!(
                        "ROW_NUMBER() OVER (PARTITION BY {} ORDER BY {})",
                        parts.join(", "),
                        ord.join(", ")
                    )
                }
            }
            BExpr::ByteaLit { hex, .. } => format!("'\\x{hex}'::bytea"),
            // Reaching the planner with an unresolved digest means the
            // access check and key release never ran. Emitting anything
            // here — a plaintext compare, a null — would be a silent
            // empty result; a refusal is the only honest lowering.
            BExpr::BlindDigest { entity, field, .. } => {
                return Err(PlanError {
                    code: "unresolved_blind_index",
                    message: format!(
                        "`{entity}.{field}`'s digest was never resolved —                          `ordo_plan::blind::resolve` runs after the access check,                          behind a key release, and before planning"
                    ),
                })
            }
            BExpr::ScalarSub { cte, column, .. } => {
                format!("(SELECT {} FROM {})", q(column), q(cte))
            }
        })
    }

    fn function(&mut self, name: &str, args: &[BExpr], ty: ScalarTy) -> Result<String, PlanError> {
        let a: Result<Vec<String>, PlanError> = args.iter().map(|x| self.expr(x)).collect();
        let a = a?;
        Ok(match name {
            // `allocate` is a planner primitive whose only legal home is an
            // `each` emission over a list parameter; the binder refuses it
            // everywhere else as `invalid_allocate`, and `each` does not
            // parse yet. Reaching here means the binder let one through.
            "allocate" => {
                return Err(PlanError {
                    code: "invalid_allocate",
                    message: "`allocate` is legal only inside an `each` emission over a list \
                              parameter (definition-language.md §3)"
                        .into(),
                })
            }
            // The decimal → storage conversions (DL §2, "No implicit
            // rounding"). The argument is cast to `numeric` because
            // `floor(bigint)` is resolved by Postgres's own overload rules
            // and `double precision` is one of the candidates — no float
            // may appear in a plan, so the operand type is stated rather
            // than inferred.
            //
            // What happens to the result depends on where it landed:
            //
            // - integral (`floor`, `ceil`, `round(x)`, `round(x, 0)`) →
            //   cast to `bigint`, because that is the whole point of
            //   writing one. `money` and `int` are minor units in `bigint`,
            //   and a `numeric` in that slot is a shape the load gate would
            //   (rightly) refuse.
            // - still decimal (`round(x, 2)`) → re-pinned to scale 12.
            //   `decimal` is scale 12 by definition, so `round(x, n)`
            //   rounds the VALUE without changing the type; letting the
            //   result carry scale `n` would make `decimal` mean a
            //   different thing depending on the expression that produced
            //   it, which is the venue-dependence the pin exists to kill.
            "floor" | "ceil" | "round" => {
                let mut cast = a.clone();
                cast[0] = format!("({})::numeric", a[0]);
                let call = format!("{name}({})", cast.join(", "));
                if matches!(ty, ScalarTy::Decimal(_)) {
                    at_scale(&call)
                } else {
                    format!("({call})::bigint")
                }
            }
            // Buckets, per DL ~617 — sortable scalars, never month numbers.
            "year" => format!("EXTRACT(YEAR FROM {})::bigint", a[0]),
            "month" => format!("to_char({}, 'YYYY-MM')", a[0]),
            "week" => format!("to_char({}, 'IYYY-\"W\"IW')", a[0]),
            "day" => format!("({})::date", a[0]),
            "hour" => format!("to_char({}, 'YYYY-MM-DD\"T\"HH24')", a[0]),
            "dow" => format!("EXTRACT(DOW FROM {})::bigint", a[0]),
            "abs" | "mod" | "least" | "greatest" | "coalesce" | "lower" | "length" => {
                format!("{name}({})", a.join(", "))
            }
            // G12, ruled. See `hash64` for the algorithm and for the two
            // spellings that look equivalent and are not.
            "hash" => hash64_sql(&a[0]),
            "trim" => format!("btrim({})", a[0]),
            other => {
                return Err(PlanError {
                    code: "unsupported",
                    message: format!("`{other}` has no lowering in this slice"),
                })
            }
        })
    }

    fn cond(&mut self, c: &BCond) -> Result<String, PlanError> {
        Ok(match c {
            // Two-valued logic is a COMPILATION OBLIGATION (DL §2, Null
            // semantics [settled]), and it does not survive naive SQL:
            // `NOT (a = b)` under SQL's 3VL yields UNKNOWN when either side
            // is null, and the row is silently dropped — the opposite of
            // `null != 5` = true. The language's table is fixed:
            // `null == null` is TRUE, `null != 5` is TRUE, every ordering
            // comparison against null is FALSE, and `x != y` is exactly
            // `not (x == y)` however spelled.
            //
            // The distinctness family, `IS [NOT] DISTINCT FROM`, is total
            // and keeps that table everywhere, but Postgres never serves it
            // from an index or a hash: every guard's EXISTS scanned its whole
            // access table once per row (audit 2026-10-25, item 1: 450 ms
            // for `myNotes` at 5,000 notes).
            //
            // So equality takes plain `=` wherever that returns exactly the
            // same rows, and keeps the distinctness form everywhere else
            // (owner ruling 2026-10-26, `guard-equality-uses-no-index`):
            //
            // - In a filter (WHERE, ON, HAVING, CASE WHEN, FILTER, through
            //   AND and OR), a null answer drops the row just as false does.
            //   One side proven never null (`never_null`) makes `a = b`
            //   answer null only where the old form answers false: `=`.
            //   Both sides possibly null: `a = b OR (a IS NULL AND b IS
            //   NULL)`. `!=` with one side possibly null spells out the
            //   null case and is total.
            // - Under `not` and inside `match none` a null answer would be
            //   inverted, not dropped (`Planner::strict`). There only `=`
            //   between two columns the database declares `NOT NULL` is
            //   used; anything else keeps the distinctness form.
            //
            // `Planner::equality` decides; `tests/equality_parity.rs` in
            // `ordo-conformance` runs both forms and requires the same rows.
            BCond::Cmp { op, lhs, rhs } => {
                let (l, r) = (self.expr(lhs)?, self.expr(rhs)?);
                match op {
                    CmpOp::Eq => self.equality((lhs, &l), (rhs, &r), false),
                    CmpOp::Ne => self.equality((lhs, &l), (rhs, &r), true),
                    // Ordering keeps its SQL form and is WRAPPED, because
                    // `a < b` is UNKNOWN on a null operand and `not (a < b)`
                    // would then drop the row instead of keeping it. The
                    // language says an ordering predicate against null is
                    // false — and false is what its negation must be able to
                    // invert.
                    _ => {
                        // Text ordering in a condition is ordering like any
                        // other, so it takes the pinned collation. Without
                        // it `<` on text used the database default, a locale
                        // order on most clusters, which disagreed with the
                        // `ORDER BY` beside it and with the device's local
                        // refinement (checklist 25l). Equality stays
                        // uncollated: it is byte equality under every
                        // deterministic collation.
                        let l = if self.is_collatable(lhs) {
                            collate(&l)
                        } else {
                            l
                        };
                        let r = if self.is_collatable(rhs) {
                            collate(&r)
                        } else {
                            r
                        };
                        let sym = match op {
                            CmpOp::Lt => "<",
                            CmpOp::Le => "<=",
                            CmpOp::Gt => ">",
                            CmpOp::Ge => ">=",
                            // Handled above; the compiler cannot see that.
                            CmpOp::Eq | CmpOp::Ne => unreachable!(),
                        };
                        format!("COALESCE({l} {sym} {r}, false)")
                    }
                }
            }
            // `x in (…)` is a disjunction of equalities by the language's own
            // reading, so it inherits equality's obligation: SQL's `IN`
            // yields UNKNOWN for a null left operand, and `not (x in …)`
            // would then drop the row rather than keep it. `COALESCE` is
            // enough here — the membership test's only null-valued outcome
            // is the one the language calls false.
            //
            // A `null` written in the list is the one member SQL's `IN`
            // cannot express: `null == null` is true, so it matches a null
            // value (DL §4, Comparisons), and `x IN (…, NULL)` never does.
            // It becomes `OR x IS NULL`; the other members stay an `IN`.
            BCond::In { expr, list } => {
                let has_null = list.iter().any(|i| {
                    matches!(
                        i,
                        BExpr::Lit {
                            value: ordo_syntax::Expr::NullLit { .. },
                            ..
                        }
                    )
                });
                let items: Result<Vec<String>, PlanError> =
                    list.iter().map(|i| self.expr(i)).collect();
                let x = self.expr(expr)?;
                let member = format!("COALESCE({x} IN ({}), false)", items?.join(", "));
                if has_null {
                    format!("({member} OR {x} IS NULL)")
                } else {
                    member
                }
            }
            BCond::Text {
                op: TextOp::Contains,
                lhs:
                    BExpr::DocPath {
                        doc,
                        keys,
                        ty: ScalarTy::Json,
                    },
                rhs,
            } => doc_contains(&doc_at(&self.expr(doc)?, keys), &self.expr(rhs)?, rhs.ty()),
            BCond::Text { op, lhs, rhs } => {
                let l = self.expr(lhs)?;
                let r = self.expr(rhs)?;
                match op {
                    // strpos/left rather than LIKE: the right side is a value,
                    // not a pattern, and LIKE would give `%` meaning it does
                    // not have in this language.
                    TextOp::Contains => format!("(strpos({l}, {r}) > 0)"),
                    TextOp::Starts => format!("(left({l}, length({r})) = {r})"),
                    TextOp::Matches => format!("({l} ~ {})", regex_guard(&r)),
                }
            }
            BCond::IsNull {
                expr:
                    BExpr::DocPath {
                        doc,
                        keys,
                        ty: ScalarTy::Json,
                    },
                negated,
            } => {
                let missing = doc_missing(&doc_at(&self.expr(doc)?, keys));
                if *negated {
                    format!("(NOT {missing})")
                } else {
                    missing
                }
            }
            BCond::IsNull { expr, negated } => format!(
                "({} IS {}NULL)",
                self.expr(expr)?,
                if *negated { "NOT " } else { "" }
            ),
            BCond::And(items) => {
                let parts: Result<Vec<String>, PlanError> =
                    items.iter().map(|i| self.cond(i)).collect();
                format!("({})", parts?.join(" AND "))
            }
            BCond::Or(items) => {
                let parts: Result<Vec<String>, PlanError> =
                    items.iter().map(|i| self.cond(i)).collect();
                format!("({})", parts?.join(" OR "))
            }
            BCond::Not(inner) => format!("(NOT {})", self.strictly(|p| p.cond(inner))?),
            BCond::Bare(e) => format!("({} IS TRUE)", self.expr(e)?),
            BCond::Exists { query, negated } => {
                // A semi/anti-join body: SELECT 1, never the outputs — this
                // deliberately bypasses the empty-projection rule.
                let mut body = Query {
                    outputs: vec![],
                    ..(**query).clone()
                };
                body.sort.clear();
                body.tiebreak.clear();
                let inner = if *negated {
                    self.strictly(|p| p.select_exists(&body))?
                } else {
                    self.select_exists(&body)?
                };
                format!(
                    "({}EXISTS (\n{inner}\n))",
                    if *negated { "NOT " } else { "" }
                )
            }
            BCond::TreeWalk {
                expr,
                tree,
                anchor,
                above,
            } => {
                // Stored materialized path (ruling 5): containment is a
                // prefix test on `$path`, never a recursive CTE. `left`,
                // not LIKE — the planner emits no pattern operators.
                //
                // BOTH ends of the walk are confined. Scoping only the
                // anchor would let a prefix match reach another space's
                // rows whose ids happen to spell the same path, and
                // scoping only the node would let a caller anchor on a row
                // they cannot see. The self-join is two independent
                // sources and each one owes its predicate.
                let node = self.expr(expr)?;
                let anch = self.expr(anchor)?;
                let (outer, prefix) = if *above { ("anc", "n") } else { ("n", "anc") };
                let (o_src, o_pred) = self.table(tree, outer);
                let (p_src, p_pred) = self.table(tree, prefix);
                format!(
                    "({node} IN (SELECT {o}.{id} FROM {o_src}, {p_src} \
                     WHERE {p}.{id} = {anch} AND {o_pred} AND {p_pred} \
                     AND left({o2}.{path}, length({p2}.{path})) = {p2}.{path}))",
                    o = q(outer),
                    p = q(prefix),
                    id = q("id"),
                    path = q("$path"),
                    o2 = q("n"),
                    p2 = q("anc"),
                )
            }
        })
    }
}
