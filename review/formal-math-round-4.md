# Formal mathematics review, round 4

Date: 2026-08-09  
Reviewer posture: adversarial. A settled claim survives only if every admitted input has one typed result, every claimed static proof has a finite reproducible procedure, and every cross-venue promise names operations that implement the same function.

## Scope and method

I read the requested documents in the prescribed order, then inspected every current source file and inline test in `crates/ordo-syntax`. I ran `cargo test -p ordo-syntax`: all 36 tests passed. That result establishes the implemented token/lexer/expression-condition parser subset only; I do not infer an evaluator, binder, pipeline parser, planner, SQL generator, or conformance runner from it.

I do not repeat the supplied deterministic failures (36 rule-coverage items, 17 markers, 646 absolute registrations, or four site external requests). Keyword parity and plan coverage currently pass. Absence of an implementation is not itself a finding below; source code is cited only where implemented behavior contradicts a settled surface rule.

Severity summary: **4 Critical, 11 Major, 1 Minor**.

## Findings

### M4-001 — The replacement division pin still overflows in SQL for valid arbitrary-precision expressions

**Severity:** Critical  
**Exact claim citation:** `docs/definition-language.md:149-151` says `+ - *` evaluate in “exact decimal (arbitrary precision)”; `:177-185` says `/` and `avg` have fractional scale 12 and that SQL uses `NUMERIC(38, 12)` at every division/average to produce the “same expression, same rows, same answer” as the host.

**Hole:** `NUMERIC(38,12)` has only 26 integer digits. A fixed typmod therefore imposes a magnitude bound that the arbitrary-precision input algebra does not impose. Fractional scale 12 is implementable, but this particular cast is not an implementation of arbitrary-magnitude scale-12 decimal. The host can return a valid value where SQL must overflow.

**Concrete counterexample:**

```ordo
(99999999999999999999999999 * 10) / 1
```

The literal fits the current `i128` lexer domain. Exact multiplication produces a 27-digit integer, and division by one produces that integer at scale 12. A host arbitrary-precision decimal returns it; PostgreSQL cannot cast it to `NUMERIC(38,12)`, whose integer capacity is 26 digits.

**Smallest deterministic fix:** Retain fractional scale 12, but use unconstrained PostgreSQL `numeric` plus an explicit `round(value, 12)` at the division/average boundary, mirrored by the host. Alternatively publish one shared maximum magnitude and require the same typed overflow before either venue runs. Add cross-venue cases at 26/27 integer digits and around signed half ties.

**Prior disposition:** **Relocated.** Y-006's original “38 significant digits via fixed `NUMERIC`” defect is genuinely withdrawn, but the replacement fixed cast recreates the magnitude half of the same impossibility.

### M4-002 — The author is required to choose a rounding mode that the language cannot spell

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:163-169` says no correct default spans half-even and half-up and therefore “must make the author write it”; `docs/sql-translation-corpus.md:121-123` likewise says “the mode is the author's to state.” The closed function list at `docs/definition-language.md:593-595` offers only `round(x)` and `round(x,n)`. The implemented closed set agrees: `crates/ordo-syntax/src/parser.rs:39-55` gives `round` arity 1–2, and its test at `:921-925` requires a three-argument call to fail.

**Hole:** Neither legal call contains a mode. Choosing the scale is not choosing how ties round. Thus a mandatory author decision has no representation and two conforming venues may select different answers.

**Concrete counterexample:** `round(2.5, 0)` is `2` under half-even and `3` under half-up. Both are expressly admitted policies, but the source text cannot distinguish them; `round(2.5, 0, halfEven)` is rejected by the current parser.

**Smallest deterministic fix:** Make the mode a required closed argument, for example `round(x, n, halfEven|halfUp)`, and add both modes to the AST and cross-venue corpus. If only one mode is desired, pin it globally and delete every claim that the author chooses it.

**Prior disposition:** **New hole exposed by the Y-006 repair.** The fixed-scale division mechanic can stay half-up; this finding concerns the separate typed-boundary conversion.

### M4-003 — Z-007 remains open: mixed durations have neither an application order nor a total date result type

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:135-146` blesses `date ± duration`; `:221-224` defines a duration as `(months, milliseconds)`; `:264-278` specifies month clamping and exact millisecond arithmetic. The batch pins at `:1490-1491` type subtraction but do not type forward date/duration arithmetic.

**Hole:** A pair representation does not define the order in which its components act on a calendar point, and clamping makes the two orders noncommutative. Separately, a `date` has no time of day, while the admitted millisecond component can be sub-day. The result cannot change static type according to a stored duration's runtime value.

**Concrete counterexamples:**

- From `2026-01-30T00:00:00Z`, applying `(1mo, 2d)` months-first yields `2026-03-02`; milliseconds-first yields `2026-03-01`.
- `2026-01-01 + 12h` could be a `date`, a `datetime`, or a typed rejection. Each is consistent with the present text.

**Smallest deterministic fix:** Publish an operator matrix and an ordered action. For example: apply signed months first with clamping, then signed milliseconds; `datetime ± duration -> datetime`; `date ± duration -> date` only when the exact component is statically an integral number of days, otherwise reject and require conversion to `datetime`. Define subtraction and negative mixed durations by the same ordering.

**Prior disposition:** **Open.** Z-007 remains accurately open. Structural equality, exact-class ordering, and month clamping do not close the action/type problem.

### M4-004 — The closed type algebra has no result types for settled month/week buckets

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:135-143` calls the field/parameter inventory closed and singles out only `decimal` as an expression-only type. `:590-592` declares `year`, `month`, `week`, `day`, and `hour` to return sortable scalars, while `:1490-1491` explicitly states `day(ts) -> date` and `month(ts) -> month`. No `month` or `week` type exists in the inventory or wire rules.

**Hole:** A closed expression algebra must assign a type to every expression, including a representation and comparison order. `month` is used as a type name without being defined; `week` has the same representational problem because ISO week-years do not map to a bare integer week number. A projected or grouped bucket therefore has no reproducible descriptor/SDK type.

**Concrete counterexample:**

```ordo
op monthly = orders[] group month(placed) { n: count }
```

The output key is promised to be sortable and composable, but an implementation may encode it as text `"2026-07"`, a first-of-month `date`, `(year,month)`, or an undeclared scalar. Those choices differ in typing, JSON shape, and which caller parameters can compare to it.

**Smallest deterministic fix:** Either map every function to existing types (`year/dow/hour -> int`, `day -> date`, `month -> date` at month start, `week -> date` at ISO Monday), or add explicit expression-only `month`/`week` types with canonical ordering and wire encodings. Put the full function signature table in the closed algebra.

**Prior disposition:** **New.** This is a type-closure defect, not an implementation-coverage complaint.

### M4-005 — Y-016's static type repair is contradicted by the duration trap

**Severity:** Minor  
**Exact claim citation:** Normative `docs/definition-language.md:232-248` permits ordering only for `duration(ms)` and rejects bare calendar `duration`. But `docs/traps.md:107-111` says ordering is legal when both operands are the same class, “either both calendar (`mo`, `y`) or both elapsed”; its own table at `:125-128` then says bare calendar duration is a compile error.

**Hole:** The guidance gives two opposite answers for calendar/calendar ordering. This is not the old proof-calculus problem—the normative type rule solved that—but the obsolete rule survived in a downstream document.

**Concrete counterexample:** `1mo < 2mo` is legal under `traps.md:109-111` and `incomparable_duration` under both the normative text and `traps.md:125-128`.

**Smallest deterministic fix:** Replace “same unit class” and the calendar/elapsed sentence with “both operands have exact class `duration(ms)`.” Preserve the later table and examples.

**Prior disposition:** **Relocated.** Y-016 is closed in the normative type system and reopened only as a contradictory guidance rule; therefore this relocated fix is still a finding.

### M4-006 — Z-008's nullable/empty `in` repair is contradicted by the SQL corpus and rejected by the parser

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:1437-1442` settles `in` as exact sugar for an `==` chain, says null matches null, and says an empty list is false. `docs/sql-translation-corpus.md:76-89` instead says nullable and empty membership remain “unruled” and names Z-008 as open. The parser grammar at `crates/ordo-syntax/src/parser.rs:323-324` requires `expr,+`, and `:345-358` unconditionally parses a first expression, so `x in ()` cannot reach the settled empty case.

**Hole:** The normative semantics are now sufficient, but the translation oracle still forbids asserting them and the implemented syntax excludes one member of the stated domain. This is a relocated conformance/surface contradiction, not mere missing implementation.

**Concrete counterexamples:**

- For `x = null`, `x in (null)` is `true` under the normative equality expansion, while corpus A4 says the behavior is unruled.
- `x in ()` must be `false` for every row, but the current parser errors at `)` because it requires a first expression.

**Smallest deterministic fix:** Update A4 to require an OR-chain of `IS NOT DISTINCT FROM` (or an equivalent null-safe semi-join), with empty membership compiled to `FALSE`. Change the grammar to `expr,*`, permit `)` immediately, and add nullable member/list and empty-list parser/SQL goldens.

**Prior disposition:** **Relocated, not closed.** The main Z-008 semantic rule is repaired; the current ledger's `closed` status overlooks the stale corpus and impossible empty syntax.

### M4-007 — Null arithmetic remains an explicitly admitted partial algebra

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:1451-1482` settles null comparisons and their SQL mapping, but no arithmetic rule follows. `docs/traps.md:366-370` explicitly says “what `null * 2` means is not yet pinned in the spec.” Outer-preserving joins deliberately produce nullable child values at `docs/traps.md:354-365`.

**Hole:** Legal expressions over ordinary pipeline output do not have one result. This affects projections, conditions over arithmetic, `cond` arms, aggregate arguments, and write RHS. SQL null propagation, a host typed error, and implicit zero are observationally different.

**Concrete counterexample:** In a parent-preserving join with no child, `finished = null`. The legal expression `finished * 2 < readers` may evaluate false via null propagation, abort on multiplication, or treat missing work as zero. A write `{ score: finished * 2 }` additionally changes transaction behavior depending on the choice.

**Smallest deterministic fix:** Make arithmetic and numeric functions strict by default: any null operand yields null, except explicitly null-eliding `coalesce`, `least`, `greatest`, and separately specified aggregates. A null crossing a nonnullable typed boundary is one named typed runtime error. Mirror the rule in SQL and host cases.

**Prior disposition:** **Open.** X-012 remains open; the repository itself acknowledges the gap.

### M4-008 — Appending only the anchor id does not make multi-source row order total

**Severity:** Critical  
**Exact claim citation:** `docs/definition-language.md:1698-1700` says the typed id is appended to every ordering, yielding “total order everywhere, ties impossible.” `docs/composition-conformance.md:373-376` specifies unsorted ungrouped order by anchor id, and `:383-386` says composed ungrouped output carries only the callee anchor id as its tiebreak.

**Hole:** A legal one-to-many pipeline has multiple result rows per anchor. Those rows share the anchor id, so adding it cannot break their tie. A cursor made only from the advertised key and anchor id is non-injective over result rows.

**Concrete counterexample:**

```ordo
customers[name] orders[placed] sort name
```

One customer with two orders having the same `placed` (or with `placed` absent from the sort) produces two rows with identical `(name, customer.id)` order keys. Their relative order and keyset boundary are undefined even with static data.

**Smallest deterministic fix:** Define a stable row-identity tuple for every pipeline grain and append all necessary source ids in a fixed source order—at least the anchor id plus the id of the admitted many-side. For composed outputs, carry that tuple invisibly rather than only the anchor id. Reject pagination if the compiler cannot prove the final tuple injective.

**Prior disposition:** **New counterexample to a newly repeated closed claim.** The id tiebreak survives for single-source rows and unique grouped keys, not for all ungrouped pipelines.

### M4-009 — X-059 is still open: `cursor: stable` promises an unconditional theorem after excluding real writers

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:1702-1705` states the iff condition in terms of write-stable sort keys. `:1707-1720` excludes `std.E.update` even though it can move every field. `:1724-1727` nevertheless says the no-skip/no-duplicate guarantee “holds unconditionally.” `docs/composition-conformance.md:413-423` repeats that unconditional guarantee. The more candid `docs/traps.md:225-235` says the label does not mean “no command in the universe can” move the key.

**Hole:** An analysis cannot exclude an executable writer and still prove an unconditional property against writes. It also examines only sort-key movement, while filter/guard membership can change between pages without moving a key. Without snapshot semantics, the pages cannot unconditionally partition either the initial or final eligible set.

**Concrete counterexamples:**

- With no authored writer for `status`, `orders sort status` is labeled stable. Grant `std.orders.update`; after page 1, move an already returned row's status behind the cursor. It is returned again on a later page.
- C120's `orders(status == "done")` has stable id order. After page 1, change an as-yet-unseen done row to open. It is skipped from the initial eligible set although no sort key moved.

**Smallest deterministic fix:** Rename the present fact `cursor: authored-key-stable` and remove “unconditionally.” Reserve exact `cursor: stable` for a proof covering all callable writes, creates/deletes, filter/guard membership dependencies, and deployment/grant changes; otherwise specify snapshot pagination or scope the theorem explicitly to rows continuously eligible whose keys no executable command moves.

**Prior disposition:** **Partly repaired, still open.** C114/C119's move-ahead and move-behind witnesses are now correct and the old “no duplicates ever” oracle is gone. X-059's remaining proof scope is not closed by those corpus repairs.

### M4-010 — X-068 remains open because `take` is still admitted as row-work evidence

**Severity:** Critical  
**Exact claim citation:** `docs/definition-language.md:1782-1793` correctly says `take` bounds returned, not examined, rows for host filters/sorts. But `:1778-1780` and `:1806-1812` still list `take` as evidence for the row-cardinality bound that decides whether O(space) work needs `unbounded`; `:1816-1817` concludes the claim is true for every field.

**Hole:** The local host-stage qualification does not repair the global cardinality proof. A downstream global take may still require an unbounded SQL scan in the no-match/worst-order case. A partitioned take may not even bound total output because the number of groups is unbounded.

**Concrete counterexamples:**

- `orders(openText contains "impossible")[id] take 1` can examine every row before returning zero, despite the global `take 1`.
- `orders[] take[customer] 1` returns one row per customer; without a customer bound, both group count and total output grow with the space.

**Smallest deterministic fix:** Track separately (1) final output rows, (2) number of partitions/groups, and (3) rows examined at every stage. A global `take N` proves only (1); it proves examined work only with a static access-path/early-stop proof. A partitioned take additionally requires a bound on partitions. Gate `unbounded` on the worst examined-stage bound promised by the O(space) theorem.

**Prior disposition:** **Relocated, still open.** X-068's host filter/sort wording was repaired, but the same unsound evidence remains in the global row gate.

### M4-011 — Automatically derived `std.E.count` is the O(space) operation the gate says cannot register silently

**Severity:** Critical  
**Exact claim citation:** `docs/definition-language.md:1798-1817` requires `unbounded` for any op with no provable row-work bound, independently of venue. `docs/std-library.md:55-60` derives `std.E.count` for every entity. `:427-436` says no std op needs the acknowledgment because `count` is “reduce-shaped,” and `:478-483` treats a full SQL scan as bounded from std's side for the same reason.

**Hole:** Reduction bounds output cardinality, not examined cardinality. Exact count over an uncapped entity must inspect all qualifying rows or equivalent index entries in the worst case. Calling the result one row does not supply the row-work bound required by the normative gate.

**Concrete counterexample:** An uncapped entity with 100 million rows automatically receives `std.E.count`. Its exact `COUNT(*)` may scan 100 million visibility/index entries, yet no human writes `unbounded`; an authored identical reduce must carry the marker under the normative rule.

**Smallest deterministic fix:** Apply one rule to authored and derived operations. Derive count only with a declared entity/cardinality cap, or require an explicit deployment-policy opt-in that is recorded exactly like `unbounded`. If std count is deliberately exempt, retract the universal “never accidental O(space)” theorem and name the exemption in artifacts.

**Prior disposition:** **New direct contradiction created by the strengthened X-068 gate.** It is not the known absolute-registration count; it is a counterexample to the gate's proof.

### M4-012 — `std.E.whole` has two incompatible collection contracts and no settled bound

**Severity:** Major  
**Exact claim citation:** The settled derivation table at `docs/std-library.md:55-60` says every collection is “paged, not truncated,” shaped `{items,count,next}`, with default 100. Rule 7 at `:438-450` instead says each collection is capped and carries a `truncated` marker. The open-items section at `:587-591` says the cap value is “deliberately not guessed,” although that cap is the proof of registrability.

**Hole:** For the same collection, `next` means omitted rows can be fetched through the same derived result, while a truncation marker and “write an op that pages it” describe a terminal capped display. The exact numeric bound is simultaneously 100 and open. Therefore neither the result type nor the static bound is settled.

**Concrete counterexample:** An invoice owns 101 lines. The table requires `{items: first 100, count: 101, next: token}` and a second request yielding line 101. Rule 7 permits `{items: first cap, truncated: true}` and directs the author to another op. Generated SDKs and the row-bound proof cannot satisfy both contracts.

**Smallest deterministic fix:** Choose one settled envelope. The smallest is the already specified per-collection page `{items,count,next}` with a fixed policy constant recorded in the artifact; remove `truncated`. If truncation is intended, remove `next`/“paged,” pin the cap before settlement, and make `truncated` part of the static type.

**Prior disposition:** **New cross-section contradiction in the std closure repair.**

### M4-013 — The std rollup owner still uses the display closure that the normative proof excludes

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:711-731` says rollup is `whole` minus the double-owned link's shallow far-side hop and proves no double count only per anchor; `:733-743` explicitly withdraws “every row has at most one root” and says the false proof must not survive std. Yet `docs/std-library.md:347-358` says rollup walks “the same closure `whole` walks” and that the one-owner rule makes double count structurally impossible.

**Hole:** The platform-derived operation has mutually exclusive closure definitions, and its owner document retains exactly the proof the normative document declares false. A link row can have two owners, and a display closure may reach the same far-side row more than once from one anchor.

**Concrete counterexample:** A project owns two link rows both pointing to the same receipt of amount 100. `whole(project)` may display that receipt twice. Under std's “same closure” rule, rollup includes 200 of receipt amount; under the normative strict closure, far-side receipt scalars contribute zero and only link-row scalars can contribute.

**Smallest deterministic fix:** Copy the strict rollup-closure rule into std §4: anchor plus owned closure, but double-owned links contribute link scalars only, never the shallow far side. Replace the one-root proof with the normative per-anchor route proof and add the duplicate-far-side case to conformance.

**Prior disposition:** **Open/relocated.** X-010 correctly remains open. Any closed disposition for the related Y-019 repair is premature while the std owner document retains the withdrawn theorem.

### M4-014 — The settled match rules disagree on what is unique and assert false monotonicity

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:1544-1550` says transitive inference occurs iff exactly one path exists. `:1567-1577` says one direct path wins even when longer simple paths exist. `:1594-1598` instead identifies the two-source rule with a unique *shortest* path. Finally `:1599-1604` says listing an extra bridge source “can only remove ambiguity.”

**Hole:** “Exactly one simple path,” “unique direct edge wins,” and “unique shortest path” are three different predicates. The first rejects graphs accepted by the latter two. Adding a terminal to a minimum connecting-subgraph problem can create multiple minimum trees, so the claimed monotonicity is false.

**Concrete counterexamples:**

- Graph `A-B` plus `A-C-B` has two simple A/B paths. The opening rule rejects; direct-edge-wins and unique-shortest accept `A-B`.
- Start with sources `A,B` and unique edge `A-B`. Add bridge source `C` with edges `A-C` and `B-C`. There are now three minimum two-edge connecting trees: `{AB,AC}`, `{AB,BC}`, and `{AC,BC}`. Listing C created ambiguity.

**Smallest deterministic fix:** State only the minimum-edge rule: for two sources, a unique shortest path wins; delete “iff exactly one path.” Replace the monotonic claim with “adding a source may resolve or create ambiguity.” If listing a bridge is intended to select edges rather than merely add a terminal, give that selection an explicit `match <ref/path>` semantics.

**Prior disposition:** **Relocated/open.** Z-009's old engine-plan contradiction is closed, but the normative match section now contradicts itself and contains a false derived theorem.

### M4-015 — Match inference is finite but has uncapped exponential diagnostics and an NP-hard general case

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:1575-1578` requires an error that enumerates two or more simple paths. `:1580-1593` requires considering every connecting subgraph for three or more sources, minimizing edge count, and enumerating all minimum alternatives.

**Hole:** Finiteness establishes decidability, not a usable compile-time procedure. A chain of diamonds has exponentially many shortest simple paths, so the mandatory diagnostic itself can be exponentially large. For a variable number of listed sources, finding a minimum-edge connecting subgraph is the unweighted Steiner-tree problem; the prescribed general inference is NP-hard before exhaustive alternative enumeration.

**Concrete counterexample:** Forty binary diamond layers between A and B yield `2^40` equal shortest paths, all mandated in one rejection. For three or more terminals, ordinary Steiner instances encode directly as entity ref graphs; “consider every subgraph” is exponential in the number of refs.

**Smallest deterministic fix:** For two sources, count shortest paths only up to two and report two witnesses plus “more omitted.” For three or more, require explicitly listed bridges/refs rather than solve Steiner inference, or impose small hard limits on terminals and graph size and make limit failure a specified diagnostic. Never require enumeration beyond two witnesses.

**Prior disposition:** **Open and expanded.** Z-009's uncapped path diagnostic remains open; the new three-source settled rule adds a stronger complexity defect.

### M4-016 — `allocate` promises total conservation over a weight domain containing undefined cases

**Severity:** Major  
**Exact claim citation:** `docs/definition-language.md:600-607` guarantees allocation parts sum exactly to total by proportional largest remainder. Its settled example surface at `:617-623` types every weight merely as `int`. No positivity, nonzero-sum, or nonempty-list precondition is stated.

**Hole:** Proportional shares require division by total weight. The admitted domain includes an empty list, all-zero weights, and positive/negative weights summing to zero. No exact shares or largest remainders exist in those cases, so the function is not total on its declared inputs and the conservation guarantee cannot always be fulfilled.

**Concrete counterexample:** `allocate(100, (0,0))` requires shares `100*0/0`. `allocate(100, (1,-1))` has the same zero denominator. An empty weights list cannot return parts summing to a nonzero total.

**Smallest deterministic fix:** At the bounded-list door, require a nonempty list, every weight `>= 0`, and total weight `> 0`; reject otherwise with one named typed error. If negative totals are admitted, allocate their absolute minor units by the same positive weights and restore the sign. Add zero, negative, empty, and tie cases to both venue corpora.

**Prior disposition:** **New.** The deterministic tie break survives; the missing input-domain predicate does not.

## Prior-finding retest ledger

| Prior | Round-4 disposition | Evidence |
|---|---|---|
| Y-006 — significant-digit division pin | **Closed in its original form; repair relocated** | “38 significant digits” is withdrawn. M4-001 shows `NUMERIC(38,12)` now imposes an incompatible magnitude bound; M4-002 shows the separate author-selected mode is unspellable. |
| Y-016 — duration proof calculus | **Closed normatively; relocated in guidance** | Unit class is now a declaration type, a finite reproducible rule. `traps.md` still admits calendar/calendar ordering (M4-005). |
| Z-007 — duration application/type | **Open** | Neither mixed-component order nor `date ± duration` result typing is supplied (M4-003). |
| Z-008 — nullable/empty `in` | **Relocated, not closed** | Main semantics are now exact, but the SQL corpus calls them unruled and the parser rejects the empty case (M4-006). |
| Z-009 — match contradiction/complexity | **Partial/open** | The earlier engine-plan contradiction is gone. Normative uniqueness predicates conflict, the bridge monotonicity theorem is false, and diagnostics/general inference remain unbounded (M4-014/015). |
| Z-010 — Gregorian literals | **Closed** | `lexer.rs:333-380` validates actual month length/leap years for dates and datetimes; tests at `:532-578` cover 1900/2000, bad month ends, and datetime inheritance. |
| X-059 — cursor behavior/proof | **Partly repaired; open** | C114/C119 now correctly demonstrate both drift directions. Excluding std writes while claiming an unconditional theorem, and ignoring membership drift, remains unsound (M4-009). |
| X-068 — row-work gate | **Relocated, open** | The returned-versus-examined distinction is now stated, but `take` remains evidence for the global row bound and std count treats reduce output as work-bounded (M4-010/011). |

## Reviewer disagreements

1. **Fixed fractional scale is a valid owner decision.** I agree with withdrawing significant digits and do not require rational arithmetic. I disagree only with encoding scale 12 as `NUMERIC(38,12)` while retaining arbitrary-magnitude arithmetic (M4-001).
2. **Duration type classes are the right repair.** Y-016's old dataflow-proof obligation is genuinely closed by `duration(ms)` versus bare `duration`; I would not reopen it. The remaining defect is the stale calendar/calendar sentence in `traps.md` (M4-005).
3. **A cursor label can honestly be weaker than snapshot pagination.** `cursor: drifting` is useful and the repaired C114/C119 witnesses should remain. I reject calling the current authored-only analysis “unconditional stable,” because the same spec identifies executable writes it deliberately ignores (M4-009).
4. **Finite does not mean acceptable compile-time magic.** The match rule is mathematically decidable on a finite schema. That does not rescue mandatory exponential output or an unrestricted Steiner-tree search during registration (M4-015).
5. **Derived operations do not receive a mathematical exemption.** A one-row `count` result is still an O(space) computation. If std is product-policy exempt from human acknowledgment, the absolute cost theorem must say so rather than relabeling reduction as a bound (M4-011).

## Claims that survived attack

- **Null equality itself is coherent.** `==`/`!=` form a two-valued partition, and the `IS [NOT] DISTINCT FROM` obligation covers the named locations. The remaining null defects are membership-document relocation and arithmetic, not explicit equality.
- **`in` now has a sound normative algebra.** Expanding to Ordo equality gives `null in (null) = true`, strict negation, and empty membership false. M4-006 concerns stale corpus/parser surface, not the repaired rule.
- **Duration equality and exact-class ordering are decidable.** Structural equality on `(months,milliseconds)` is total; declaration-site `duration(ms)` makes ordering admissibility finite and reproducible.
- **Month clamping examples are correct.** The stated Jan-end non-associativity follows from clamp-per-application. It does not determine the order of mixed components.
- **Gregorian literal validation is genuinely repaired.** The current lexer rejects nonexistent dates before constructing date/datetime tokens, and its tests cover leap-century boundaries.
- **The repaired cursor-drift witnesses are valid.** Moving a key ahead can skip and moving it behind can duplicate. Those cases accurately explain `cursor: drifting`.
- **The strict normative rollup closure proves its scoped claim.** Excluding the shallow far-side hop gives at most one visit per row within one anchor's owned tree. The defect is that std still specifies another closure.
- **Match inference is decidable on any fixed finite ref graph.** Simple paths and finite connecting subgraphs eliminate infinite walks. The surviving failure is consistency and complexity, not undecidability.
- **The id tiebreak is sufficient on a one-row-per-id grain.** Single-source ungrouped output and groups with a unique full group key can have total order. M4-008 refutes only the universal extension to multi-row-per-anchor grains.
- **The syntax subset passes its current suite.** All 36 `ordo-syntax` tests pass, including Gregorian dates, precedence, nonchained comparisons, function arities, duration tokenization, and `cond` shape. No stronger semantic conclusion follows.

## Bottom line

Round 4 contains real closures: Y-006's significant-digit rule is withdrawn, Y-016's proof calculus is replaced by a reproducible type class, Z-010 is implemented and tested, nullable `in` is normatively defined, and the drift corpus now demonstrates both failure directions. The release is not yet formally closed. The most serious surviving defects are cross-venue numeric overflow under a valid expression (M4-001), non-total multi-source ordering (M4-008), and two independent ways the O(space) gate silently admits unbounded work (M4-010/011). The settled match, pagination, std closure, and rounding-mode claims also contradict their own downstream contracts. Each has a bounded repair above; none requires widening the language's core design.
