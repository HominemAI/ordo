# Audit — where the documents disagree, and what the code decided

Read-only audit, 2026-09-02. Method: cross-compare the normative documents in
`docs/` against each other and against the tree, looking for the same rule
stated twice with different content, status marks that disagree for one thing,
rules whose stated consequence names a class or mechanism that does not exist,
and code fences the surrounding prose contradicts. `corpus-rulings.md` and
`roadmap-checklist.md` were read first; anything already ruled or already
recorded as owed is listed in §B rather than reported as a finding.

Ordered by consequence. **F1–F3 are security-relevant**; F1 is the one worth
acting on before anything else, because a checklist item currently instructs
the next implementer to build the losing side of it.

---

## F1 · Grant narrowing is specified two ways, and the recorded remediation implements the wrong one · **Critical · security**

**Three documents, two rules.**

`definition-language.md` §8, line 2542, heading its own paragraph `[settled]`:

> **A pinned manifest carries the field set AND the row-scope shape [settled].**
> Comparing field sets alone is decidable and **wrong**, and the counterexample is
> not exotic — it is the most ordinary edit in the language. Delete
> `member == $caller` from an op body, or add `unguarded` to its header, and the
> op's **output field set is byte-identical**: same columns, same types, same
> order. A field-set comparison reads that as "not an expansion", classifies it as
> narrowing, and **applies it silently**. A grant a person consented to as *"this
> app may read my expenses"* becomes *"this app may read everyone's expenses"* with
> no re-consent prompt, no audit flag, and no visible change anywhere.

and at line 2564: **"Narrowing = field-set subset AND row scope no weaker.
Both conditions, or it is not narrowing."**

`security-logging-design.md` §5, line 132, heading its own bullet `[pinned]`:

> **Per-op, not combined** [pinned — corrects an earlier self-contradiction] …
> The decidable form, needing no registry history:
> - Pin **per listed op**: its manifest hash **and** its ordered output field
>   set at consent time.
> - On call, compare per op: hash equal ⇒ unchanged. Hash differs ⇒ compare
>   field sets — **subset** = narrowing (applies silently, audit-noted);
>   **superset or disjoint** = expansion, and the grant is **dead until
>   re-consent**.

Row scope appears nowhere in that section. `api-engine-design.md` §10 (line
523), also `[settled]`, restates the same field-set-only form and cites §5 as
its authority:

> What *is* decidable without history is the grant-pinning comparison — an
> **ordered field-set** comparison per op: subset = narrowing, superset or
> disjoint = dead. See `security-logging-design.md` §5.

**What the code does.** Neither. `ordo_identity::access::grant_for` computes
`combined_manifest_hash(&ops, &self.deployed_manifests)` (`access.rs:426`), and
`CapabilityGrant` carries one `ManifestHash` and no field set at all
(`principal.rs:362-365`). `AccessEngine::check` compares that single hash and
denies `ManifestChanged` on any difference (`access.rs:469-472`). So today
every manifest change of any op in a grant kills the whole grant. That is
strictly more conservative than either document, and the escalation DL
describes is not reachable.

**Why this is urgent anyway.** `roadmap-checklist.md` 8s (line 977) is the one
open item covering this, and it quotes **only** `security-logging-design.md`
§5 — its stated fix is *"pin per listed op its manifest hash and its ordered
output field set at consent time"*. An implementer following 8s as written
would replace a fail-closed combined hash with exactly the field-set-only
comparison DL calls *"decidable and **wrong**"*, and would thereby build the
silent authorization widening DL spells out: delete `member == $caller`, the
field set is byte-identical, the grant survives, no prompt and no audit flag.
The item's own summary even says *"The consequence is not a bypass, it is the
opposite"* — true of today's code, and false of the fix it prescribes.

**Which side is right.** DL §8. Its argument is a counterexample, not a
preference, and it is corroborated externally: `security-hardening-review-4.md`
line 237 records *"Y-005 — open at SH4-001. **Row-scope pinning fixed the
guard-only counterexample** but not the complete authority lattice."* — i.e.
row-scope pinning is the accepted repair for the guard-only case and the
remaining dispute is only about how much further the lattice must go.
`security-logging-design.md` §5 and `api-engine-design.md` §10 are the stale
side; both predate the DL paragraph and neither cites it.

**Consequence.** As long as the two spellings coexist and the checklist points
at the weaker one, the next person to implement grant pinning has a
documentation-blessed path to a silent consent-widening. Severity is
**Critical** on that basis rather than on current behaviour.

**Owed.** Amend `security-logging-design.md` §5 and `api-engine-design.md` §10
to carry the row-scope half (or to defer to DL §8 by reference), and rewrite
8s's prescribed fix. Note also that neither §5 nor §10 mentions the trigger
class, which DL §8 requires in the shape for routines.

---

## F2 · `quota_exceeded` contradicts the non-enumerability rule at the deployment door · **Major · security**

`definition-language.md` §8, line 2928, on space-creation allowances:

> Enforcement sits at the deployment door, before the space registry row is
> reserved, and a refusal is an ordinary logged denial naming the attempted op —
> `quota_exceeded` when the allowance is the reason, **so a caller can tell "you
> may not" from "not right now."**

`api-engine-design.md` §4.1, line 197, `[settled]`, and its table row at line
128:

> **The door's answer is `access_denied` for every reason it refuses**
> [settled — this was specified three incompatible ways and is now one]. An
> unregistered name, a registered op this token may not call in this space, a
> missing or malformed token, and an unsatisfied step-up all produce the
> *identical* response: same status, same code, same message, same (absent)
> headers.

The two clauses are directly opposed: DL's stated *purpose* for the second
class is the discrimination §4.1 exists to remove. DL §8's own
`scope deployment` bullet (line 2868) says *"everything else about the door is
unchanged"*, which if taken literally makes `quota_exceeded` unreachable and
the sentence at 2928 self-cancelling.

`quota_exceeded` appears in no closed inventory — not
`api-engine-design.md` §4, not `composition-conformance.md` §0, not
`guards-conformance.md` §0.

**What the code does.** Nothing has decided. `std.space.create` is not built
(`roadmap-checklist.md` 8r: *"`std.space.*` needs a platform-op path"`), there
is no producer for `quota_exceeded`, and `AccessEngine::check` has no quota
input. The door renders one `access_denied` for every `DenyReason`
(`router.rs:332,443`); `AccessDecision` carries no reason across the API
boundary, which is correct under §4.1.

**Which side is right, and it is not a clean call.** §4.1's rule is right for
space-scoped ops and its reasoning does not fully transfer here: a deployment
capability holder who may call `std.space.create` has already been told the op
exists, so distinguishing "over quota" from "denied" discloses nothing new *to
that principal*. The leak is for a caller **without** the capability, who
learns from a `quota_exceeded` that the op is real and that they were inside
the allowlist. The narrow correct rule is therefore *"`quota_exceeded` only
after the capability check has already passed"*, which is a third statement
neither document makes. I would rule: §4.1 wins as written (one door answer),
and DL §8 is amended to say the allowance signal is delivered **after**
authorization succeeds, or is a `429`-shaped rate answer rather than an
authorization class. Left as a genuine gap rather than assumed.

---

## F3 · The compile-class inventory is not closed, and nothing checks it · **Major**

`composition-conformance.md` §0 line 14 heads itself *"Error classes (closed
inventory)"* and line 16 states *"Every reject case names one class. Classes
are part of the compiler contract — the harness asserts the class, not the
message text."* `guards-conformance.md` §0 extends it with eleven more. The
wire inventory is checked in two directions by
`crates/ordo-api/tests/inventory_is_closed.rs`, whose own doc comment states
the principle:

> An inventory that does not name a class the engine emits is not closed — it
> is a list.

**The compile inventory has no such test, and by that standard it is a list.**
Counted by hand across `ordo-syntax`, `ordo-schema`, `ordo-sema` and
`ordo-plan` (raise sites plus the three `const` class names in `parser.rs`):
**62 distinct compile classes have a producer. 34 of them appear in none of
the three inventories. 21 appear in no document at all.**

Producers with no inventory row and no document anywhere:

`ambiguous_field`, `ambiguous_match`, `bad_aggregate`, `duplicate_entity`,
`duplicate_field`, `empty_projection`, `indexed_without_sealed`,
`multiple_owners`, `no_match_path`, `non_boolean_condition`,
`reserved_field_name`, `tree_not_self_ref`, `type_error`,
`unknown_index_field`, `unknown_ref_target`, `unknown_unique_key_field`,
`unpageable`, `unresolved_blind_index`, `unsupported`, `unsupported_default`,
`unsupported_unique_key`.

`unsupported` is the striking one: **19 raise sites** across `ordo-sema` and
`ordo-plan`, carrying refusals as different as *"`{op}` is a write op — it is
called, never composed"* and *"`since` takes a duration literal"*. It is the
compile-side equivalent of the `internal` 500 the write path's typed refusals
were added to end.

Producers named in `definition-language.md` but in no inventory:
`ambiguous_reverse_step`, `bulk_identity_field`, `implicit_rounding`,
`incomplete_access_clause`, `insert_incomplete`, `invalid_allocate`; plus
`id_key_collision` (`deploying.md`), `snake_case` (`migration.md`),
`duration_unit_mismatch`, `reserved_param`, `unscoped_source`,
`unsupported_index`, `unknown_field` (checklist only).

**One case where the document and the code name different classes for the same
input.** `composition-conformance.md`:

- line 30: `` | `parse_error` | text outside the closed grammar: synonym
  spellings, empty parens, **non-boolean condition position**, … | ``
- line 91: `` | bare non-boolean field in condition position | `parse_error` |
  C105 | ``
- line 1288: **C105 — reject.** `activeProducts(name, price)` → reject:
  `parse_error`

The code raises `non_boolean_condition` (`ordo-sema/src/lib.rs:4342`), asserted
as such in `ordo-sema/tests/bind.rs:283`. The conformance harness does not
catch the disagreement because `corpus.rs::synonym_spellings_all_fail` asserts
only that no plan is produced — it never reads the class. That is precisely
the failure the 9r ruling was written to end, surviving in the one suite that
declares "every reject case names one class".

**Which side is right.** The code. Under the 2026-08-30 shape-versus-meaning
ruling (`corpus-rulings.md`), `activeProducts(name, price)` is *readable* —
the parser can see a condition position holding a path — and what is wrong
with it is its **type**, which only the binder knows. That is meaning, not
shape, so `parse_error` is the wrong class and the document is stale. But
`non_boolean_condition` then has to be **added to the inventory**; today it is
neither.

**Consequence.** Every one of the 34 is a class an author or operator can
receive and cannot look up, and the inventory's "closed" claim is unfalsifiable
without a test. This is 9w repeated one layer down, at roughly ten times the
scale.

**Owed.** A `compile_inventory_is_closed` test with the same two directions
and the same by-name allow-list as the wire one, then a triage pass: promote
the caller-facing classes into `composition-conformance.md` §0, and collapse or
rename `unsupported`.

---

## F4 · The engine implements a different `duration` type system than the one DL settles · **Major**

`definition-language.md` §1, line 274, `[settled]`:

> - **`==` / `!=` are structural component equality** — total, defined on every
>   pair, no errors. `1mo != 30d` and that is the whole story; `1y == 12mo`
>   (same months, same ms); **`7d == 168h`** (same ms). This keeps the partition
>   theorem (§4): every row is either equal or unequal, and a filter can never
>   abort an op on one row's stored value.

`ordo-sema/src/lib.rs:4292` runs `check_duration_units` on **every**
comparison, `==` and `!=` included, and `check_duration_units` (line 4434)
raises when the two `ScalarTy::Duration(unit)` operands differ:

> ```
> "duration_unit_mismatch",
> format!("comparing `duration({})` with `duration({})` — this slice does not
>          convert between units", …)
> ```

So `7d == 168h` — the spec's printed example of a true comparison — is a
compile error, and the partition theorem's stated support is gone.

Three further divergences from the same root:

1. **Bare `duration` does not parse.** DL §2 line 156 lists the type as
   *"`duration` (unit-classed — `duration(ms)` is the exact class, **bare
   `duration` the calendar class**…)"* and line 158 gives the example
   *"Durations are first-class (`everyDays: duration = 7d`)"*.
   `declparse.rs:817` is `p.expect(&TokenKind::LParen, "`(` — `duration`
   declares its unit")` — the parenthesis is mandatory. The **calendar class,
   the entire half of the model that may carry months, is undeclarable**.
   `traps.md`'s table row `` | `duration` (bare) | **calendar** … | **compile
   error** (`incomparable_duration`) | `` therefore describes a shape the
   parser rejects before the class check could run.
2. **The parenthesis means a different thing.** In the spec it selects a
   *class* and has exactly one member, `ms`. In the engine it is a storage
   unit: `duration(d)`, `duration(h)`, `duration(mo)`, `duration(y)` all parse,
   and `ordo-schema/src/ddl.rs::convert_duration` converts between them by
   millisecond factor. Ordering a `duration(mo)` field is legal in the engine;
   DL §2 line 291 makes it *"a **compile error** at the point of use, class
   `incomparable_duration`"*.
3. **The two classes the spec names have no producer; the class the engine
   raises has no spec home.** `incomparable_duration` and
   `duration_unit_class` appear in `definition-language.md`, `engine-plan.md`
   G10, `traps.md` and `tutorial.md`, and in zero raise sites.
   `duration_unit_mismatch` has three raise sites and appears only in
   `roadmap-checklist.md`.

**What is recorded, and what is not.** The checklist (line 1391) records the
*parameter-bound* consequence and describes the cross-unit refusal as
deliberate: *"a unit conversion it refuses everywhere else **by design**
(`duration_unit_mismatch`)"*. That framing is the problem — it presents a
divergence from a `[settled]` rule as a design choice, and nothing anywhere
records that `==` between durations is supposed to be total.

**Which side is right.** The spec, on the merits. DL's two-component model is
the one that keeps `==` total and therefore keeps §4's partition theorem, and
the partition theorem is load-bearing for guards and filters. The engine's
per-unit-storage model is defensible on its own terms and self-consistent, but
it buys nothing the spec model does not and costs the totality rule.

**Consequence.** Not security-bearing, but it is the largest silent
spec-versus-engine divergence found: three documents (`definition-language.md`,
`traps.md`, `tutorial.md`) teach a type system the compiler does not have.

---

## F5 · `since` anchors to a hard-coded date in the production path · **Major**

`definition-language.md` §5, line 1527, `[settled]`:

> `since <duration>` (dates: within the trailing window of **execution time**)

`engine-plan.md` G13 `[engine-pinned]`:

> `since` on `date` fields compares against the **execution-time** UTC date; on
> `datetime` fields against the execution-time UTC instant.

Every construction of `ordo_sema::Binder` sets `clock: (2026, 5, 1)` — the
conformance corpus's pinned instant — including `bind_as` (`lib.rs:1085`),
which is the constructor the **runtime** executor calls
(`ordo-exec/src/lib.rs:567` and `:1066`, once per request). The field's own doc
comment (line 379) says *"a real runtime will supply the request's execution
instant"*; there is no parameter, setter or seam through which one can.

`Condition::Since` folds to a literal `DateLit` at bind time (line 4361), so a
deployed `since 7d` means `>= 2026-04-24` permanently. As wall-clock time
advances the window does not slide — it **widens**, monotonically, forever. An
op written as "the trailing week" becomes "everything since April 2026".

`since` is also the language's only nondeterminism, which is what makes this
the one place a frozen clock is invisible: `reproducible` (F6) would have
flagged it and does not exist either.

**Which side is right.** The documents, unambiguously — a `since` that does not
move is not a trailing window. The code knows: its comment describes the
missing seam.

**Consequence.** Correctness for any op using `since`; mild disclosure risk
where `since` bounds what a caller may see, because the bound only ever
loosens. Note that `roadmap-checklist.md` 9j records a *different*, smaller
`since` gap (the `?live` re-evaluation tick) and nothing records this one.

---

## F6 · `reproducible` is parsed, published, and never enforced — and its violation has no class · **Major**

`definition-language.md` §13, line 3120, `[settled]`:

> Flags between name and `=`: **`reproducible` (forbids `since`)**, `unbounded`
> (cost-class acknowledgment), `unguarded` (guard bypass).

and §2, line 422, `[settled]`:

> Marked `filled` and also `reproducible` on the same op **is a compile
> error**: filling from a clock is the nondeterminism `reproducible` forbids.

`engine-plan.md` G14 lists it as part of G9's registration wrapper.

**What the code does.** `reproducible` lexes (`token.rs:187`), parses into
`OpFlags` (`declparse.rs:866`), and is published on the artifact
(`ordo-plan/src/artifact.rs:445`: `"reproducible": f.flags.reproducible`).
Nothing reads it. `grep -rn reproducible crates/` outside those four sites
returns only golden JSON. Both stated compile errors are unimplemented, and
**neither has a class name in any document** — unlike its two siblings, which
have `unacknowledged_unbounded` / `unbounded_take` and `unguarded_superfluous`
/ `unacknowledged_unguarded` respectively.

**Which side is right.** The spec. The asymmetry is the tell: two of the three
registration flags are enforced with named classes and one is decoration, and
the decorative one is the determinism promise. An artifact that advertises
`"reproducible": true` for an op containing `since` is a published false claim
— the artifact is the input to grant pinning (F1) and to version skew.

**Consequence.** A caller reading the manifest is told an op is reproducible
when it is not. Worse in combination with F5: the flag that would have caught
the frozen clock is the one that does nothing.

---

## F7 · Settled rules whose stated consequence names a class that exists nowhere · **Moderate**

Beyond F4's two and F6's unnamed pair, three more `[settled]` rules name a
refusal class that appears in no inventory and has no producer:

| Class | Named at | Inventory | Producer |
|---|---|---|---|
| `unknown_event_kind` | `definition-language.md` §7 line 2329 — *"an unrecognized kind is a compile error (`unknown_event_kind`), **not a pattern that silently never fires**"* | none | none |
| `group_nesting` | `definition-language.md` §3 line 966 — *"Groups may not contain groups in v1 — nesting is **rejected at write time** (`group_nesting`)"* | DL §3 | **`ordo_conformance::membership`** (closed 2026-09-05) |
| `check_violation` | `critique-consolidated.md` X-016 names it as the fix; `definition-language.md` §3's `check` rule says *"violation = typed rejection aborting the whole atomic group"* and names no class | none | none |

`check_violation` is already ruled: `corpus-rulings.md` (2026-08-31) records
that `check` is *"owed, and blocked on one line of taxonomy"* and explicitly
declines to mint the class. `group_nesting` is **closed** — the group
membership surface produces it at the add, and `corpus-rulings.md`
(2026-09-05) records why it is a premise rather than a scope note: depth 1 is
what makes a single expansion sufficient, so a nested group would have
under-expanded the guard walk, `callersOf` and `memberCount` silently.
`unknown_event_kind` is the survivor, and both it and `check_violation` sit
inside `[settled]` sections whose consequence sentences read as if the class
were live. `unknown_event_kind` is the more consequential of the two —
its own sentence advertises that it exists precisely so a listener pattern
cannot silently never fire, which is the failure mode it would currently have.

**Which side is right.** The documents; these are debts, not errors. But
`api-engine-design.md` §4 is `[settled]` and its codes are closed, so each of
these is a promise that cannot be kept without an inventory amendment, and
none is tracked.

---

## F8 · `api-engine-design.md` §4 claims a closure the code's own comment denies · **Moderate**

`api-engine-design.md` §4, closing paragraph:

> `ExecError` now carries an `ErrorCode` directly, so **an executor refusal
> whose class is not on this table cannot be constructed**, and `access_denied`
> — the door's word and the door's alone — is rejected at that boundary rather
> than merely discouraged.

`ordo_host::classify` (`ordo-host/src/lib.rs:380-412`), the function that
actually turns an executor refusal into an `ErrorCode`, documents itself:

> It is a **string** seam. `ordo-conformance`'s write protocol returns
> `Result<_, String>` and encodes its class in a prefix by convention; nothing
> in the type system requires a new refusal to carry one, or to spell it the
> way the inventory spells it. A typo (`tree_cyle:`) or a missing prefix is a
> **silent demotion to 500**, and no compiler will say so.

Both halves of the doc's sentence are conditionally true — the `ErrorCode`
enum is closed and `executor_raisable()` does reject `access_denied` — but the
"cannot be constructed" claim is about the *type system*, and the type system
is `String` up to `classify`. The code compensates with a static tripwire
(`tests/http.rs::every_raised_class_prefix_is_a_wire_class`) and says so.

**Which side is right.** The code comment. The document overstates a real
improvement into a guarantee, and the honest sentence is one clause longer:
"…cannot be *rendered*; the seam producing them is still a string, guarded by
a source-scanning test rather than by the type system."

**Consequence.** Small on its own; it matters because `classify`'s failure mode
is a silent 500 on a typed refusal, which is exactly the regression §4's
paragraph exists to say has been fixed.

---

## F9 · Contradictions that are ruled in `corpus-rulings.md` but still stand in the source document · **Moderate**

The 2026-08-31 std-library ruling (`corpus-rulings.md`, *"Two places
`std-library.md` says both things"*) settled both of these on the merits. The
document was never edited, so a reader of `std-library.md` alone still gets
both answers:

- `std-library.md:217` — *"`std.E.create` / `put` / `update` **write params** …
  **unchanged — sealed fields stay writable**"*
- `std-library.md:218` — *"`std.E.update` converge braces … the **open skeleton
  only** — sealed fields are *not touched*"*

Ruling: braces win; sealed fields leave `update`'s params with its braces. The
same section's §2 *"derives `create` for every entity"* against DL §6 rule 8 is
likewise ruled (skip on two-unique-key entities) and likewise unedited.

**Which side is right.** The ruling, and it is not in dispute. What is worth
recording is the pattern: `corpus-rulings.md` is authoritative for the ruling
and is not a substitute for fixing the document, and there is no mechanism
that notices the difference. Three of the five 2026-08-30 findings closed *by
editing a document*; these two closed by ruling only.

---

## F10 · Status marks that disagree across documents · **Minor**

- **`adoption-dx-plan.md`:257** — the MCP-server row's dependency is
  *"spec §8 principals **[open]**"*. `definition-language.md` §8's Principals
  subsection is headed `[settled]`; under the 2026-08-31 ruling (*"A section
  preamble can be a status marker, and §8's is"*), §8's semantics are
  `[settled]` and only its **spelling** is `[lean]`. `[open]` is neither, and
  it is the one mark that implies nothing has been decided. Right answer:
  `[lean] spelling over [settled] semantics`.

- **`bootstrap.md`:252, input 36** — the Status column reads *"not an input —
  **client-level**"* and Supplied-by reads `` client { stepUp … } ``, while the
  If-wrong cell in the same row reads *"Per Z-003 the minimum belongs on the
  **op**, and clients may only strengthen it."* The row contradicts itself, and
  `security-hardening-review-4.md`:221 explicitly asked for this repair — *"The
  stale 'client-level' label in `bootstrap.md:252` should be corrected without
  changing the rule"* — and it has not been made. `definition-language.md`'s
  own `client` block was corrected (its `stepUp` fence now carries `# ADDS to
  what these ops already require`), so this is the last residue.
  Right answer: op-level, clients may add.

- **`guards-conformance.md` GC048 vs GC080** — GC048 says the inherited acl
  read guard's scope manifest carries `boardsAccess.{board, member, level}`;
  GC080 pins *"post-pruning discipline"* and shows a guard-only field dropped
  for exactly this reason. `ordo-schema/src/lib.rs:1568` implements GC080: at
  the bottom rung the derived read guard emits `member == $caller` and **no
  level test at all**, because every declared level qualifies. So `level` is
  not read by the read guard and GC048's manifest is wrong. The engine has
  already picked a winner and nothing records it. Right answer: GC080; fix
  GC048's field list.

- **`definition-language.md` §7 vs §8, trigger declaration** — §7 spells a
  trigger `when finance.transactions.ingested -> run spendWatch`; §8 spells the
  same fact `trigger when …` nested inside a `routine` block, and no document
  says which is the declaration. Recorded only inside the doc-parse harness's
  allow-list (`ordo-syntax/tests/docs_parse.rs:165`) and in `corpus-rulings.md`
  as an obstacle, never as an owed ruling in any spec document. Genuinely
  balanced on the merits; it needs an owner, not a preference.

---

## F11 · The spec's own code fences are never parsed · **Minor, but it is how F4 survived**

`crates/ordo-syntax/tests/docs_parse.rs` extracts and parses every fenced block
in **two** documents:

```rust
const SYNTAX: &str = include_str!("../../../docs/syntax-examples.md");
const TUTORIAL: &str = include_str!("../../../docs/tutorial.md");
```

Its allow-list is checked in both directions and is genuinely excellent — it is
where the `[lean]`-versus-`[settled]` distinction is machine-enforced. But
`definition-language.md` — the normative document — is not among the files it
reads, nor are `traps.md`, `writing-data.md`, `modeling-guidance.md`,
`sql-translation-corpus*.md`, or either conformance suite. Two of the thirty-
eight documents are checked, and neither is the spec.

That is directly how F4's first symptom survives: DL's `everyDays: duration =
7d` is inline prose rather than a fence, and `traps.md`'s bare-`duration` block
is in a file the harness never opens.

**Owed.** Extend `docs_parse.rs`'s file list, expecting a large one-time
allow-list. The allow-list is the deliverable, not the green test — it is what
would have told someone that bare `duration` is unparseable.

---

# §B · Cross-checked and found consistent

These were compared because they are the places a contradiction would cost
most. Each came back clean, and several came back *already repaired*, which is
worth recording so the repair is not re-proposed.

**Already repaired since they were last written up:**

- **The lock-wait histogram.** `implementation.md` no longer claims
  `ordo_chain_append_lock_wait_seconds` exists. Line 265 now reads: *"it must
  exist before any tuning does. **It does not exist yet**, and that makes it a
  prerequisite of the measurement rather than a companion to it: it is a
  histogram, `ordo-host`'s metrics registry holds counters only (its module
  doc names the three absent histogram families)."* This agrees exactly with
  `ordo-host/src/metrics.rs`'s module doc. The metrics module is the most
  honest file in the tree about its own gaps and I found nothing it overstates.
- **The `client` block's `stepUp` fence.** `definition-language.md`:2680 now
  carries `# ADDS to what these ops already require —` inside the fence, and
  the prose beneath is headed *"[settled — corrects the earlier client-config
  reading]"*. Fence and prose agree. (`bootstrap.md`'s copy does not — F10.)
- **`hash(x)` and text collation.** `engine-plan.md` G11 and G12 both carry
  *"this entry's ⚠/pin is withdrawn"* and point at the rulings.
  `ordo_plan::hash64` implements SHA-256-truncated-top-bit-cleared as ruled;
  `TEXT_COLLATION` is `C`. No residue.

**Checked and consistent:**

- **The wire inventory, both directions.** `inventory_is_closed.rs` compares
  `api-engine-design.md` §4's table against `ErrorCode::ALL` including status
  codes, with `refinement_unbounded` as the sole by-name allow-list entry. I
  re-derived the table (42 rows) and found no drift.
- **The door's single answer.** `api-engine-design.md` §4.1,
  `guards-conformance.md` R-G13, `security-logging-design.md` §6 and
  `definition-language.md` G18 all say `access_denied` for every door refusal,
  and `router.rs:332/443` renders exactly one. `DenyReason` (including
  `PrincipalSuspended`) never crosses the API boundary — the security doc's
  *"its own reason (`principal_suspended`)"* is about the record, not the wire,
  and the code honours that distinction.
- **Step-up.** `security-logging-design.md` §5, `definition-language.md` §8 and
  Z-003's fix all say op-intrinsic minimum, clients may only strengthen.
  `StepUpRequirement::strongest` is a max with `None` as identity, the
  deployment window is a `min` ceiling, and capability tokens are refused
  structurally. Three documents and the code agree.
- **Suspension ordering.** *"denied at the door, before role, profile, group or
  capability resolution"* — `access.rs:455` checks
  `MembershipStatus::Suspended` before the token's shape is consulted. Agrees.
- **Capability grant space-binding.** `security-logging-design.md` §5's *"the
  space comparison is first [pinned]"* is `access.rs:463`, ahead of expiry and
  hash. Agrees.
- **`SealedRef` opacity.** §7's `[pinned]` correction (*"a random 128-bit
  handle … never a content hash"*) is `Value::SealedRef([u8; 16])` and
  `ordo-crypto/src/sealed.rs`, which additionally forbids deduplication with
  the reasoning spelled out. The one place a documented crypto correction was
  fully carried into the code.
- **acl level thresholds.** `definition-language.md` §3's *"the three
  thresholds are POSITIONS in the ladder, not names [settled]"*, including the
  `acl (read, comment, write, admin)` case, is implemented positionally at
  `ordo-schema/src/lib.rs:1533-1535`. (The code comment there says the spec
  *"leaves open"* ladders longer than three; DL settles it explicitly. Stale
  comment, correct code — not worth a finding.)
- **Non-enumerability at the row layer.** §4.1's table and R-G10 agree
  clause-for-clause, including the create split (parent → `not_found`,
  would-be row → `access_guard`) and the unguarded-entity exception. R-G10
  names itself the losing side of the earlier ruling; both documents say so.
- **The security-event stream.** §6.1's four reasons for keeping denials out of
  the command chain are consistent with `implementation.md`'s Observability
  section and with `engine-plan.md` P5, and §6.1 explicitly scopes the two
  earlier statements it supersedes rather than leaving them standing.
- **Combined-hash direction of failure.** Verified that today's
  `combined_manifest_hash` is strictly more conservative than either documented
  form — every change denies. F1 is about the prescribed fix, not about current
  behaviour.
- **Compile inventory, document → code direction.** Of the 22 classes in
  `composition-conformance.md` §0, only three lack a producer —
  `dependency_failed`, `breaking_dependents`, `sealed_keyset_pagination` — and
  §0 names all three with the feature each waits on. The four in
  `guards-conformance.md` §0 with no producer are likewise audited in place
  (`caller_supplied` and `caller_in_default` are since built). The doc-to-code
  direction is clean; only the code-to-doc direction (F3) is open.
