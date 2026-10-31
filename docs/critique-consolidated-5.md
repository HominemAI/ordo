# [review] Ordo critique round 5 — the usability findings, re-run against the docs

> **Status: REVIEW. Nothing here changes a specification.** This is the
> original consolidated critique (`critique-consolidated.md`, five independent
> expert reviews) re-run across today's documentation and today's binary, on
> the owner's instruction of 2026-09-27: *"run it across our documentation to
> confirm the usability of the system."*

Round 4 is dated **2026-09-05**. Every engine in this repository was built
after it: guards that execute, the door, custody, the supervisor, the import
path. Its carried list has therefore been stale for three weeks, which is the
reason for this pass.

**Method.** Each finding was checked against the tree as it stands, by grep,
by reading the code it names, or by running the shipping binary. Nothing below
is inferred from a status table. Where a finding is closed, the evidence is
named; where it is open, the failing command is pasted.

---

## 1 · Verdict

| | |
|---|---|
| Usability findings re-checked (§3f, X-074 to X-083) | **10** |
| Closed, with evidence | **8** |
| Closed in the documents, unbuilt in the engine | **1** (X-081) |
| Carried | **1** (X-078, and it is narrower than it was) |
| Round-4 `open` findings re-checked | **2** (X-001, X-068) |
| Of those, now closed | **1** (X-001) |
| **New findings this pass** | **1, Major** (W-001) — **fixed the same day** |

The documentation is in materially better shape than the audit that produced
these findings described. The one new finding is the same *class* the original
review kept identifying — a developer is stopped by what the docs say, not by
what the language is — and it is the sharpest single instance of it yet.

---

## 2 · New finding

### W-001 — The tutorial teaches a declaration the compiler cannot parse · **Major**

- **Claim.** `tutorial.md` §13 *"A routine, now that the data is guarded"*
  introduces a `routine` block as ordinary syntax, and walks the reader
  through what is right and wrong about it in detail.

- **What happens.** The shipping binary cannot parse the word:

  ```
  $ ordo check routine.ordo
  routine.ordo:1:1: parse error [parse_error]:
      expected a declaration (`domain`, `entity`, `op`), found `routine`
  ```

- **Why it is worse than an unimplemented feature.** The error names three
  declarations and stops. Nothing in it says the feature is deliberately
  unbuilt, so it reads as *you made a typo* or *you are on an old binary*. A
  reader who has followed twelve sections correctly now believes they mistyped
  something the document told them to write. `grep -c "not yet\|unbuilt\|does
  not compile" docs/tutorial.md` returns **0**: there is no caveat anywhere in
  the file.

- **Reach.** `routine` is taught as working syntax in `tutorial.md`,
  `syntax-examples.md`, `modeling-guidance.md`, `traps.md`,
  `definition-language.md` and `guards-conformance.md`.

- **This is not a language defect.** 12a ruled the declaration form **not
  owed**, on the ground that §7 and §8 spell the same fact two different ways
  and building either would decide it by accident. That ruling is correct and
  is not in question here. What it did not carry was a pass over the documents
  that teach the spelling it declined to pick.

- **Two fixes, and they are independent.**
  1. **The documents say so.** A marker at §13 and at every other site, in the
     form the corpus already uses for lean material. Cheap, and it removes the
     wall. [S]
  2. **The refusal says so.** `parse_error` for a word the grammar reserves
     and does not implement could name that state instead of listing three
     declarations. The compiler knows `routine` is a keyword; it is the parser
     that has nowhere to send it. [S]

- **Filed as** checklist `17af`, board row `routine-taught-unbuilt`.

- **FIXED 2026-09-27, both halves.**

  **The refusal now names the state.** `UNBUILT_DECLARATIONS` in
  `crates/ordo-syntax/src/declparse.rs` is a table of the nine
  reserved-and-unbuilt declarations — `routine`, `trigger`, `principal`,
  `client`, `policy`, `channel`, `route`, `app`, `page` — each with the reason
  and its checklist id, on the same argument the neighbouring
  `FOREIGN_SPELLINGS` table already makes: *one spelling per concept is only
  true if the other spellings fail, and fail saying so.* Reserved-and-unbuilt
  is a third state and the refusal has to say which of the three it is:

  ```
  $ ordo check routine.ordo
  routine.ordo:1:1: parse error [parse_error]: `routine` is reserved and not
      implemented: scheduled and reactive rules are specified in
      `definition-language.md` §7 and §8 and no compiler accepts one yet —
      the declaration's spelling is deliberately unsettled (checklist 12a, 19c)
  ```

  **The documents say so**: `tutorial.md` §13 carries a notice explaining that
  the semantics it teaches are settled and the declaration's spelling is not;
  `syntax-examples.md`, `traps.md` and `modeling-guidance.md` each carry a
  short form above their first `routine` block.

  **What the build itself found, and it is why the finding survived four
  rounds.** The fix was first written into `parse_program`'s `Ident` arm, on
  a check that reported these words as plain identifiers. They are all
  `Keyword`s. They reach the `other` arm, and `describe` renders a keyword
  with backticks in exactly the format the `Ident` arm uses for a word — so
  the two arms produced **byte-identical messages** and there was no way to
  tell from the output which one had answered. The first version of the fix
  therefore compiled, passed review by eye, and changed nothing. It was a unit
  test that caught it.

---

## 3 · The usability findings, re-checked

### X-074 — `let` appears in no onboarding document · **CLOSED**

Closed at round 3 and still closed. `let` now appears **12 times in
`tutorial.md`** and **13 times in `syntax-examples.md`**, which are the two
documents the finding named as silent.

### X-075 — The reserved-word list is incomplete and labeled closed · **CLOSED**

`engine-plan.md` §G3 no longer carries a hand-written list. It carries a
**rule**: *"A word is fully reserved iff it can begin a declaration or a
clause"*, with the words derived under it. That is the mechanical
regeneration the fix asked for.

### X-076 — Routine names in every example violate the identifier rule · **CLOSED**

Every hyphenated routine name is gone from the specification. The one
surviving occurrence of `lease-expiry` in `definition-language.md` is **the
rule that forbids it**, explaining that the hyphen lexes as minus and that
*"hyphenated names in older examples are wrong, not an exception"*. The
remaining hits across `docs/` are inside the critique documents themselves,
which are records of the review and must not be edited.

### X-077 — Parameter bounds are upper-bound-only · **CLOSED**

`ordo_api::spec::Bounds` carries **both** `min` and `max`. The finding's
sharpest consequence — one constraint raising two error classes at two
different times — no longer follows, because the door can express the whole
constraint.

### X-078 — The app manifest grammar is `[lean]` and under-specified · **CARRIED, narrower**

`ui-gen.md` §5 is no longer flat-lean. It now reads **"[mixed: binding grammar
normative, everything else illustrative]"**, with the section opening on
*"read the marker precisely, because the two halves of this section have
different status"*. The binding grammar — the half the failing builds actually
tripped on — was promoted. What remains illustrative is `metric`/`table`/
`detail` rendering, which is 14c's work and is carried with Z-021 and Z-022.

### X-079 — SDK-filled parameters have no declared mechanism · **CLOSED at round 4**

Not re-derived here; round 4 closed it and nothing in this pass contradicts
that.

### X-080 — The tutorial capstone does not compile · **CLOSED, and re-tested**

Closed at round 3 on a reading of the tutorial. This pass tested it instead.
All 24 blocks in `tutorial.md` that look like source were extracted and run
through `ordo check`:

- **8 compile standing alone.**
- **13 fail only because they are fragments** — `unknown_ref_target` on an
  entity an earlier block declares, `unknown_source` on an op defined earlier.
  That is the tutorial building up a program, which is what a tutorial does.
- **2 are grammar schemas, not programs** (`entity(conditions)[projection]`,
  `op name($params) flags = body`). They are notation and correctly do not
  compile.
- **1 fails on purpose**: §9.1's `incomplete_access_clause` block, which the
  section is titled *"The compile error that saves you"*.
- **1 is W-001**, above.

So the capstone defect is genuinely repaired, and the method that found it is
now cheap to re-run.

### X-081 — No syntax exists to emit a notify proposal · **CLOSED in the documents, UNBUILT**

`definition-language.md` names the mechanism: delivery is platform-owned and
domains *"emit `notify`"* rather than touching providers. The document gap is
closed. The engine is not: checklist **17r** records *"settled spec, zero
code"*. Stated as two facts rather than one verdict, because a reader of the
spec and a reader of the binary get different answers.

### X-082 — The trigger event-kind vocabulary is never enumerated · **CLOSED**

`definition-language.md` §7: *"Event kinds are a closed set [settled]"*, and
an unrecognized kind is a compile error (`unknown_event_kind`) *"not a pattern
that silently never fires"* — which is the enumeration plus the refusal the
finding asked for.

### X-083 — Docs/code ratio and spec surface · **INVERTED**

At the time of the review the documentation vastly outweighed the code. Today:
**71,108 lines of documentation against 228,646 lines of Rust**, roughly one
to three. The concern the note raised does not describe this repository any
more.

---

## 4 · Round 4's two open findings

### X-001 — Authorization is specified but does not execute · **CLOSED**

The headline finding of the original review, and of every round since.
Authorization now executes: `ordo_sema`'s `guard_cond` compiles a guard into
the query itself, and five conformance suites exercise it —
`guards_corpus.rs`, `guards_canary.rs`, `guards_multihop.rs`,
`guard_groups.rs`, `guard_literal_types.rs`.

### X-068 — Bare `take` admitted as evidence for an examined-row bound · **HALF CLOSED**

`ordo_plan::cost::row_bound` no longer takes a number from the plan. A
`take $n` bounds at **the parameter's declared bound**, and the proof has two
halves that are both built: `check_take_bounds` refuses an unbounded parameter
at registration (`unbounded_take`), and the door refuses a value above the
bound at call time (`bound_violation`). A parameter with no bound answers
`Unbounded` rather than being quietly bounded by whatever the caller chose.

What the finding also asked for — admitting `take` only where the planner
proves an access path can stop before unbounded work — is not done.
`take[keys]` is **deliberately absent** from the evidence list and is recorded
on the lean ledger rather than assumed covered. Half closed, honestly.

---

## 5 · What this pass did not cover

The carried list from round 4 also holds X-003, X-010, X-012, X-051, X-059,
X-070, X-071, X-072, X-073, X-084, X-085, X-086 and the Y/Z series. Those are
correctness, distributed-systems and operability findings rather than
usability ones, and the instruction for this pass was usability. They are
still carried and still three weeks stale, and a round 6 over them is worth
doing before the first deployment carries data that matters.

## Related

- `critique-consolidated.md` — the original five merged reviews
- `critique-consolidated-3.md`, `critique-consolidated-4.md` — prior re-runs
- `plan-usability-testing.md` — the *other* usability document: an automated
  walk of the quickstart plus a human protocol, which tests the happy path
  rather than re-auditing the corpus. Complementary, not this.
- `roadmap-checklist.md` `17af` — W-001
