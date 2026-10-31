# Formal mathematics review, round 2

Date: 2026-08-03  
Reviewer posture: adversarial; a claim survives only if all admitted inputs have one result and every claimed static decision has a finite, specified proof procedure.

## Scope and method

I read the requested documents in the prescribed order, then inspected `ordo-syntax`'s token inventory, lexer, expression/condition AST, parser, and their unit tests. I ran `cargo test -p ordo-syntax`: all 29 tests passed. That result establishes only the implemented lexical/expression-parser subset; there is no semantic evaluator, binder, pipeline parser, planner, SQL generator, or conformance harness to test the mathematical claims below.

I do not repeat the supplied deterministic-gate counts (keyword parity, plan coverage, unreferenced rulings, missing markers, or absolute-claim registrations). This review is about semantic counterexamples. Absence of implementation is not itself reported as a new finding.

Severity summary: 2 Critical, 8 Major, 2 Minor.

## Findings

### M2-001 — “38 significant digits” cannot be implemented by a fixed `NUMERIC(p,s)` cast

**Severity:** Critical  
**Exact claim:** `definition-language.md` §2, lines 151–156: “38 significant digits, round-half-even” and “generated SQL carries explicit `NUMERIC(p,s)` casts at every division and average,” producing the same result host-side and in SQL. Lines 137–143 separately promise arbitrary-precision exact decimal for `+ - *`.

**Hole:** PostgreSQL `NUMERIC(p,s)` fixes total decimal digits `p` and fractional digits `s`; it does not express a fixed number of significant digits across arbitrary magnitudes. No single `(p,s)` both retains 38 significant digits for values near zero and admits arbitrary-size integer parts. Thus the specified implementation mechanism cannot realize the specified rounding rule. “Maximum scale” is also being used for “significant digits,” two different quantities.

**Concrete counterexample:** Compare `1 / 3` with `10^38 / 3`. A cast that gives the first 38 significant digits of `1/3` needs at least 38 fractional places. `NUMERIC(38,38)` then has zero integer places and cannot represent `10^38 / 3`; `NUMERIC(38,0)` admits an integer part but rounds `1/3` to zero. Raising `p` merely moves the failure because `+` and `*` are declared arbitrary precision. Host code following significant-digit rounding and SQL following any fixed cast therefore diverge or SQL raises overflow.

**Smallest closed fix:** Choose one implementable rule. The smallest is a fixed **fractional** scale (for example 38 digits after the decimal), specify `round_half_even(x, 38)` as a semantic operation, use unconstrained PostgreSQL `numeric` operands followed by the same explicit rounding operation, and define a maximum numeric exponent/precision if storage or wire formats need one. If significant digits are essential, specify the magnitude-dependent algorithm (including zero and exponent bounds) and its exact PostgreSQL expression; do not claim a fixed `NUMERIC(p,s)` cast implements it. Add cross-venue goldens around powers of ten and half-even ties.

**Prior disposition:** X-007 was **merely relocated**. `/0` is now closed and choosing venue-independent rounding was a valid owner decision, but the selected rule is not executable as written.

### M2-002 — Applying a mixed duration to a date/datetime has no component order

**Severity:** Major  
**Exact claim:** `definition-language.md` §2, lines 181–184 defines every duration as `(months, milliseconds)`; lines 201–215 define month clamping and say millisecond arithmetic is exact; lines 132–134 bless `date ± duration` generally.

**Hole:** Addition can construct a duration with both nonzero components (`1mo + 2d`), but the spec never says whether the calendar component or fixed component is applied first. Clamping makes the choices observably different. Structural representation alone does not define the action of that representation on a calendar point.

**Concrete counterexample:** Let `d = 2026-01-30` and `u = 1mo + 2d`. Months-first gives `2026-02-28 + 2d = 2026-03-02`. Milliseconds-first gives `2026-02-01 + 1mo = 2026-03-01`. Both follow every stated local rule; Ordo has two answers.

**Smallest closed fix:** Define duration application as an ordered pair action, preferably calendar months first, then milliseconds, for both `+` and `-`; define multiplication componentwise and negation before application. Add Jan 29–31 cases for positive and negative mixed durations on both `date` and `datetime`.

**Prior disposition:** X-008 **remains open in part**. Structural equality, incomparable ordering, clamping, and non-associativity were closed; mixed-component application was not.

### M2-003 — `date ± duration` has no total result type for sub-day durations

**Severity:** Major  
**Exact claim:** `definition-language.md` §2, lines 129–135: closed types include `date`, `datetime`, `duration`, and “`date ± duration`” is blessed. §4 lines 985–986 pins `date - date → duration` and `datetime - datetime → duration`, but supplies no forward-arithmetic matrix.

**Hole:** A date has no time of day, while a duration may be `1ms`, `12h`, or `36h`. The result cannot sometimes be `date` and sometimes `datetime` based on a runtime duration value without destroying static typing. Rounding/truncating back to a date is also unspecified.

**Concrete counterexample:** `2026-01-01 + 12h` could be the date `2026-01-01`, the datetime `2026-01-01T12:00:00Z`, or a type error. For a field `delta: duration`, the compiler cannot choose a result type from the row value.

**Smallest closed fix:** Publish an operator typing table. Minimal conservative rule: `date ± duration → date` only when static duration analysis proves the millisecond component is an integral number of days; otherwise reject and require an explicit `datetime` operand. Define `datetime ± duration → datetime` in UTC, using M2-002's component order. Define whether `date - date` yields a millisecond-only whole-day duration.

**Prior disposition:** X-008 **remains open** for the date/datetime action even though its ordering subproblem was partly closed.

### M2-004 — “Statically single-component” has no specified proof calculus

**Severity:** Major  
**Exact claim:** `definition-language.md` §2, lines 192–200: `< <= > >=` and sorting are legal only where “the compiler proves that every duration reaching the comparison” is single-component and both sides use the same component; failure is always compile-time.

**Hole:** The admissibility judgment is normative but undefined. Zero belongs to both component subspaces; `cond`, arithmetic, parameters, fields, and multiplication all require join/transfer rules. Different sound compilers can accept different programs, contradicting one language. Merely saying “proves” does not specify which true propositions are provable.

**Concrete counterexample:** `sort cond(monthly -> 1mo, 0d)`. Every runtime result is month-only or zero, so it is orderable as months if zero is polymorphic. A syntactic unit checker sees both `mo` and `d` and rejects. Conversely, treating every `duration` field as single-component because its default is `1mo` is unsound: a later write can store `1mo + 1d`.

**Smallest closed fix:** Specify a finite abstract domain and transfer rules, e.g. `{Zero, Months, Milliseconds, MixedOrUnknown}` with `Zero` compatible with either side; define rules for literals, fields/params, `+`, `-`, integer multiplication, `cond`, and `coalesce`. Unconstrained stored fields/params are `MixedOrUnknown`; optional declared component refinements can narrow them. Add accept/reject conformance cases.

**Prior disposition:** X-008's runtime-error disagreement was **relocated** into an unspecified static proof obligation. The static-only direction is preferable, but not yet a language rule an independent implementation can reproduce.

### M2-005 — Null arithmetic is still explicitly unruled

**Severity:** Major  
**Exact claim:** `definition-language.md` §4, lines 946–977 settles only comparisons and their SQL mapping. `traps.md` lines 291–295 admits: “what `null * 2` means is not yet pinned in the spec.” Outer-preserving joins deliberately produce such nulls.

**Hole:** The expression algebra is partial or multivalued on ordinary query output. This infects projections, write RHS, aggregate arguments, `cond` values, and comparisons of arithmetic results. `coalesce` suggests propagation but implication is not semantics.

**Concrete counterexample:** After `customers[name] perCust[spent]`, customer Cy has `spent = null`. The legal projection `[double: spent * 2]` may return null, zero, or abort. A write RHS `{ score: spent * 2 }` has the same ambiguity with atomicity consequences.

**Smallest closed fix:** State one global rule: all arithmetic operators and numeric functions are strict (`null` operand ⇒ `null`) except the explicitly null-eliding functions (`coalesce`, `least`, `greatest`) and aggregates with separately specified empty/null-input behavior. A null result crossing a nonnullable typed boundary is a typed runtime rejection. Add SQL/host goldens.

**Prior disposition:** X-012 **remains open**. The repository itself says so; no new finding is inferred from absence.

### M2-006 — `in` has no null truth table and is omitted from the SQL compilation obligation

**Severity:** Major  
**Exact claim:** `definition-language.md` §4 lines 936–944 lists `in` among comparisons; lines 946–963 declare two-valued logic and one reflexive equality; lines 965–977 prescribe distinctness compilation for `==`, `!=`, and orderings, but not membership.

**Hole:** If `in` is membership under Ordo equality, `null in (null)` must be true and `5 in (null)` false. Native PostgreSQL `IN` returns `UNKNOWN` in both relevant null cases. The “every comparison” prose does not state whether `in` expands into Ordo equality, and the prescribed SQL mapping does not cover it.

**Concrete counterexample:** A nullable `category` with `category in ($a)` and `$a = null`. Ordo's one-equality model implies the null row matches; SQL `category IN (NULL)` drops every row. A host evaluator using existential structural equality returns a different result.

**Smallest closed fix:** Define `x in (a, …, n)` as `x == a or … or x == n`, including an empty-list ruling if empty lists ever become expressible. Compile each term with `IS NOT DISTINCT FROM` (or an equivalent null-safe semi-join), never native nullable `IN`. Add null list/member conformance cases.

**Prior disposition:** X-011 was **closed for explicit equality/inequality but remains open at the membership operator**. X-006's uniqueness repair survives and is not implicated.

### M2-007 — `take` does not prove a bound on rows examined

**Severity:** Critical  
**Exact claim:** `definition-language.md` §5 lines 1208–1217 says a `take` proves a maximum candidate count for bounded host work. Lines 1219–1238 reuse `take` as evidence for the row-cardinality gate and conclude an O(space) dashboard is “never accidental.”

**Hole:** `take N` bounds output cardinality, not candidate rows processed before the take. Any unpushed predicate, sort, aggregate, or decrypt before `take` may inspect the entire space to determine the first N results. The proof confuses a bound on the result with a bound on work.

**Concrete counterexample:** `op firstGift = orders(memo == "gift")[id] take 1` where `memo` is sealed and unindexed. If the only gift is the last of ten million rows—or no gift exists—the host must decrypt and test all ten million. The stated rule can classify it bounded because `take 1`, exactly the accidental O(space) operation the gate claims impossible. `sort sealedScore; take 10` has the same defect even without a filter.

**Smallest closed fix:** Track two bounds: rows **produced** and rows **examined at each stage**. A downstream `take` may bound only stages after it; it may bound an upstream scan only when the planner proves a pushed-down key/order permits early termination before every host-side or aggregate stage. Registration gating must use the examined-row bound. Add counterexamples with no match, last match, host sort, and aggregate-before-take.

**Prior disposition:** X-068 was **merely relocated**. Extending the gate from venue to cardinality was correct, but treating `take` as universal evidence reopens the same absolute claim.

### M2-008 — The pagination conformance corpus still asserts the disproved theorem

**Severity:** Major  
**Exact claim:** `composition-conformance.md` lines 120–126 says pagination is stable under concurrent writes with “no duplicates”; C114 lines 1281–1286 says “no duplicates, ever.” The normative spec now says the opposite at `definition-language.md` lines 1151–1171 and gives the duplicate counterexample itself.

**Hole:** The test oracle contradicts the repaired semantics. Worse, C113's key is the derived aggregate `spent`, which is demonstrably mutable, so the op should be labeled `cursor: drifting`; C114 calls its behavior “stability” and asserts only the move-ahead/skipped branch. A conforming implementation of the normative spec and a conforming implementation of the corpus cannot be the same program.

**Concrete counterexample:** Page 1 ends at `(cust_1, spent=16000)`. A write reduces cust_1 to 5000. Page 2's descending keyset predicate includes cust_1 again, producing a duplicate—the exact example in the normative spec. C114 has no assertion for the `cursor: drifting` metadata and its universal sentence forbids this valid outcome.

**Smallest closed fix:** Rename C114 as one drifting-cursor scenario, assert `cursor: drifting`, delete “no duplicates, ever,” and add the move-behind duplicate scenario. C113 should also assert drifting. C115 must assert drifting because the spec declares composed post-`take` pagination always drifting.

**Prior disposition:** X-059 was **fixed in the main spec but merely relocated in the executable oracle**.

### M2-009 — Cursor stability is not decidable from write brace fields alone

**Severity:** Major  
**Exact claim:** `definition-language.md` lines 1151–1155: stability is provable because the compiler knows every registered write's braces and therefore “the closed set of fields any op can move.”

**Hole:** A sort key may be a derived expression, aggregate, dot-path, group membership, or guard-dependent result. Its value or presence can change without any write assigning the output key as a brace field. Creates and deletes have no prior aggregate output field to “move.” Guard-row changes can alter which contributing rows exist for a caller. Brace membership is therefore not a decision procedure for semantic write-stability.

**Concrete counterexample:** `spendRanked` sorts by `spent = sum orders.total`. `set orders { customer: ..., total: 9000, ... }` creates a row; no brace names `spent`, yet the key moves (C114 demonstrates it). Likewise `sort customer.name` moves when a write braces `customers.name`, not a field of the anchor row. A membership grant can add rows to a guarded aggregate without touching any data sort field.

**Smallest closed fix:** Define dependency sets for every key over flattened base fields **and row-set dependencies**. A key is stable only if no registered create/delete can change its contributing set, no write can change a dependent base field/ref, and no guard dependency can change visibility. Conservatively label all aggregates, post-`take` compositions, and guard-varying row sets drifting unless an immutability proof discharges every dependency. “Decidable” then means a specified finite dependency analysis, not brace lookup.

**Prior disposition:** X-059 **remains open at the proof-rule level** even though the stable/drifting distinction itself is correct.

### M2-010 — The rollup fix exists in the language spec and is contradicted by the std contract

**Severity:** Major  
**Exact claim:** `definition-language.md` lines 545–559 correctly says rollup uses `whole` minus the shallow far-side hop. `std-library.md` §4 lines 265–276 instead says rollup uses “the same closure `whole` walks” and that one-owner makes double count structurally impossible.

**Hole:** These are mutually exclusive definitions of the platform-derived op. The std document is the document that owns derivation, so an implementer can legitimately follow the stale, false definition. Link entities are the explicit exception to one-owner.

**Concrete counterexample:** Root `portfolio` owns two link rows, each linking the same `transaction` whose amount is 100. `whole(portfolio)` displays both link rows and shallow-hops to the transaction twice. Under std's stated same-closure rollup, transaction amount contributes 200; under the language spec's corrected closure it contributes zero (only numeric scalars on the links themselves contribute).

**Smallest closed fix:** Replace std §4's first paragraph and proof with the exact smaller-closure rule from the language spec; say “at most once,” not “exactly once”; add a link entity with duplicate far-side refs to rollup conformance.

**Prior disposition:** X-010 was **merely relocated**. One owner document was repaired; the std owner document was not.

### M2-011 — Match ambiguity diagnostics require exponentially large output

**Severity:** Minor  
**Exact claim:** `definition-language.md` lines 1039–1073 defines simple paths correctly but requires an ambiguity error that “enumerates the paths.”

**Hole:** Existence/uniqueness of a path in a finite graph is decidable efficiently enough for compilation; enumerating every simple path is not a bounded diagnostic obligation. A graph with a chain of binary diamonds has exponentially many simple paths. The compiler can be forced to spend exponential time and produce exponential output merely to reject one source pair.

**Concrete counterexample:** Construct 40 successive diamond layers between entities A and B, each layer offering two bridge entities. There are `2^40` simple A–B paths. `A B` must reject, but the mandated diagnostic cannot enumerate approximately one trillion paths in any practical build.

**Smallest closed fix:** Decide ambiguity by finding at most two witness paths. Report those two, the total only if cheaply known, and “additional paths omitted.” Cap path length/diagnostic work or use a polynomial bridge/articulation analysis. The semantic definition of path and direct-edge-wins rule need not change.

**Prior disposition:** X-009 is **closed semantically**. This is a new diagnostic-complexity defect, not a reopening of path meaning.

### M2-012 — The lexer accepts nonexistent ISO dates

**Severity:** Minor  
**Exact claim:** `definition-language.md` lines 53–55 calls date literals bare ISO dates. `lexer.rs` lines 303–314 calls malformed dates invalid but checks only month `1..=12` and day `1..=31`; its tests reject month 13 but have no Gregorian validity cases.

**Hole:** Lexical acceptance admits values outside the declared date domain. The AST then carries them as `DateLit`; no current semantic layer exists to repair the token. This also makes month-clamping rules nonsensical on an invalid starting point.

**Concrete counterexample:** `parse_expr("2026-02-31")`, `2025-02-29`, and `2026-04-31` lex successfully. `2024-02-29` should succeed. Datetime literals inherit the same date check, so `2026-02-31T12:00:00Z` also succeeds.

**Smallest closed fix:** Validate Gregorian month length and leap years in the lexer (or a shared date constructor called by it), and add all four cases above plus century boundaries 1900/2000. Continue rejecting before AST construction.

**Prior disposition:** No prior X-ID directly covers this finding.

## Prior-finding retest ledger

| Prior | Disposition after attack | Evidence |
|---|---|---|
| X-006 null equality/uniqueness | **Closed** | Language equality now governs uniqueness; `UNIQUE NULLS NOT DISTINCT`, blind-index null sentinel, and `null_key_address` are all specified together. |
| X-007 division | **Merely relocated** | `/0` and the policy choice are closed; M2-001 shows the selected SQL realization cannot implement 38 significant digits. |
| X-008 duration algebra | **Partly closed; remains open** | Structural equality, static rejection direction, clamping, and non-associativity survive. M2-002–004 show application order, date typing, and proof calculus remain. |
| X-009 path inference | **Closed semantically** | Undirected simple multigraph paths, parallel edges, self-loop exclusion, and direct-edge-wins are precise. M2-011 concerns only unbounded enumeration. |
| X-010 rollup closure | **Merely relocated** | Corrected in `definition-language.md`; contradicted in `std-library.md` (M2-010). |
| X-011 SQL two-valued equality | **Closed narrowly** | Explicit `==`/`!=` mapping is now correct and broad. Nullable `in` remains outside it (M2-006). |
| X-012 null arithmetic | **Remains open** | `traps.md` explicitly acknowledges the missing rule (M2-005). |
| X-059 pagination | **Merely relocated / proof incomplete** | Main spec retracts the universal guarantee; composition corpus still asserts it and dependency analysis is underspecified (M2-008/009). |
| X-068 O(space) acknowledgment | **Merely relocated** | Cardinality was added, but `take` is unsound evidence for examined-row bounds (M2-007). |

## Reviewer disagreements

1. **Division policy:** The earlier review presented rationals versus pinned rounding as an owner decision. I agree with choosing venue independence and do not demand rationals. I disagree that “38 significant digits via `NUMERIC(p,s)`” completes that choice; it confuses precision with scale and fails by magnitude (M2-001).
2. **Duration comparison:** The earlier engine ruling allowed runtime incomparability where static detection failed; the revised spec requires compile-time rejection. I side with the revised spec: a filter must not start failing when one stored row changes units. I nevertheless reject an unspecified proof oracle as conformance-ready (M2-004).
3. **Null arithmetic severity:** One could infer ordinary SQL null propagation from `coalesce`. I reject inference as a language rule, especially because host execution is a first-class venue and `traps.md` explicitly says the meaning is not pinned. Major is warranted.
4. **Pagination:** The stable/drifting disclosure is the right repair and should remain non-fatal. The conformance corpus's “no duplicates, ever” is not a harmless stale phrase: it is an asserted behavioral oracle and must be changed (M2-008).
5. **Path enumeration:** The path definition itself now survives. I rate exhaustive diagnostic enumeration Minor, not Major, because it does not make valid programs ambiguous; it creates a compiler resource-exhaustion surface that a two-witness diagnostic removes without language change.

## Claims that survived attack

- **One equality for null and uniqueness:** `null == null`, nullable uniqueness, blind-index null handling, and null-key upsert rejection now form one coherent algebra. The deliberate departure from ordinary SQL is explicit.
- **Explicit equality SQL mapping:** `IS NOT DISTINCT FROM` / `IS DISTINCT FROM` is required across conditions, checks, joins, guards, and writes. That closes the original 3VL-negation hole for the operators it names.
- **Duration equality:** Structural equality on `(months, milliseconds)` is total; `1mo != 30d`, `1y == 12mo`, and `7d == 168h` are coherent. Rejecting cross-component ordering at compile time is the correct direction.
- **Month clamping:** Jan-end behavior and non-associativity are explicitly disclosed; `since 1mo` shares the same clamp. The stated examples are internally consistent.
- **Path meaning:** Simple paths in the undirected ref multigraph, parallel edges distinct, tree self-loops excluded, and a unique direct edge winning is a complete semantic resolution of X-009.
- **Total ordering within a page:** Appending typed id as the final key and fixing nulls last yields a total row order. This does not imply cross-page stability under writes, and the main spec now correctly separates those properties.
- **Conditional totality:** `cond` has a mandatory else and ordered first-match arms. The parser structurally enforces an arm plus final else, and its unit tests cover missing-else and missing-arrow failures.
- **Division by zero:** `/0` is now a loud typed runtime error, aligned with `mod(a,0)`.
- **Rollup's corrected mathematical closure:** The definition-language version—exclude a double-owned link's shallow far-side hop—does restore the no-double-count argument. The defect is cross-document contradiction, not the corrected proof itself.
- **Current parser subset:** All 29 `ordo-syntax` tests pass. Arithmetic precedence, nonchained comparisons, boolean mixing rejection, closed function arity, duration token spelling, and `cond` surface shape behave as their tests state. No stronger semantic inference is justified.

## Bottom line

The prior round produced real repairs, not cosmetic edits: X-006 and X-009 are closed, and the main pagination and duration-ordering directions are substantially better. The remaining fatal mathematical issue is that the exact-decimal venue rule names an implementation mechanism that cannot implement its own significant-digit semantics. The other high-risk defect is the bound analysis: `take` bounds returned rows, not work, so the renewed “never accidental O(space)” theorem is false. Until M2-001 and M2-007 are corrected, Ordo's two central deterministic promises—same numeric answer across venues and statically honest cost—do not hold.
