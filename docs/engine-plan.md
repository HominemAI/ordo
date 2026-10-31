# Ordo Engine — Gap Review & Build Plan

Companion to `definition-language.md` (the spec), `implementation.md`, and
`composition-conformance.md`. This document is (1) the last-once-over gap list
found before building, with the rulings the engine pins to proceed, and (2) the
build plan for the language engine itself.

Rulings below are marked **[engine-pinned]** — adopted to unblock the build,
each a one-line change if vetoed. Everything else defers to the spec.

---

## Part 1 — Gap review (last once-over)

The spec is conformance-ready on semantics; the gaps are almost all **lexical
formalization** — things every example agrees on but no section states.

### G1. Comments [engine-pinned]
Every example uses `#`. Never declared. Pinned: `#` starts a comment to end of
line, anywhere outside a string literal. No block comments (one spelling).

### G2. Identifiers [engine-pinned]
Never stated. Pinned: entity, field, query, routine, and domain names match
`[a-z][A-Za-z0-9]*` (lowerCamel; no underscores — one spelling, and `_` stays
reserved for typed-id literals). Params are `$` + the same rule. Max 64 chars.

### G3. Reserved words [engine-pinned — regenerated]
The previous list was labelled *closed* and was not: it omitted **every
authorization keyword** and every platform-block keyword, which made
`entity x { read: int }` legal and ambiguous and let a parser pass its own
tests while accepting files the spec forbids. Two tiers, and one rule for
deciding which tier a word is in:

**Rule.** A word is **fully reserved** iff it can *begin a declaration or a
clause* in top-level or shape scope. Everything else is **contextual** — legal
as a field name, because position disambiguates.

- **Fully reserved** — logic and pipeline: `and or not in is null true false
  match any none on group reduce sort take append let distinct under above
  since contains starts matches set del subtree each cond`.
- **Fully reserved** — declarations and clauses: `entity op routine domain
  trigger when every check index unique sealed indexed owned tree normalize
  whole acl access app page principal client auth policy compliance channel
  space group` … the registration flags `unguarded unbounded reproducible`,
  which are part of G9's wrapper and must be reserved or an op named
  `unbounded` parses, and the parameter marker `filled`, which occupies a
  trailing grammar slot in the parameter list and would otherwise lex as an
  identifier in type position.
- **Contextual** — legal as field names, recognized only in one position:
  `read write admin` (after `access`, and inside `acl (…)`), `run method
  methods share rollup live table metric detail route` (inside `routine`,
  `auth`, `policy`, and `app`/`page` blocks), and the aggregate/function names
  (`count sum avg min max median percentile joined coalesce round …`).
  Reserving `read`, `write`, `live`, or `table` globally would steal four of
  the most natural field names in the language for no parsing benefit.

**The list must be generated from the grammar, not hand-maintained.** This is
the second time it drifted. P1 carries a test that walks the parser's keyword
table and asserts it equals this document's list — a hand-kept list is a list
that will be wrong again.

### G4. String literals [engine-pinned]
Double quotes; escapes exactly `\" \\ \n \t`. No other escapes, no single
quotes, no interpolation (concatenation is `+`).

### G5. Numeric & money literals [engine-pinned]
`int`: decimal digits. Decimal literals (`10.50`) are **money/decimal**
literals — exact decimal, never float. Money fields accept decimal literals in
major units (`10.50`) — storage in minor units is mechanics the server owns
(the spec's "minor-units integer semantics" is storage, not spelling). Unary
minus is legal in expressions (`-1050`, `-(a + b)`); after `sort` /
projection-`*`, a leading `-` means descending/exclusion — position
disambiguates, as with brackets.

### G6. Line structure [engine-pinned]
The grammar is line-oriented ("one transform per line"). Pinned: a **step ends
at end-of-line unless a bracket/paren/brace is open** — open delimiters
continue the line (this is what every multi-line `group { … }` and `cond(…)`
example already does). No explicit continuation character. Blank lines and
comment-only lines are insignificant. Indentation is style, not syntax.

### G7. Bare boolean fields as conditions [engine-pinned]
`alerts(active)` appears in examples. Pinned: a bare field of type `bool` is a
complete condition (≡ `active == true`); any other bare expression in condition
position is a compile error.

### G8. `append` arity [engine-pinned]
`append <source>` takes one source form (entity/name with parens/projection) —
not a nested pipeline. Multiple appends chain (`append a … append b`). A
branch needing its own steps is a `let` first. (Matches every example; keeps
the pipeline linear.)

### G9. Registration wrapper — settled at spec level
`op name($params) flags = <body>` for **every** registration; kind (read /
write / delete) is derived from the body, never asserted. `query` is not a
keyword (reserved-for-teaching error in the parser: "every registration is
`op`"). REST enforcement of kind-as-method is the API layer's job.

### G10. Duration comparability — **ruled at spec level; this entry's pin is withdrawn**
The request was answered, and with a *stronger* rule than the one pinned here.
`definition-language.md` §1 (Duration semantics [settled]) states it:

- A duration is a **two-component value `(months, milliseconds)`** — the one
  part of the old pin that survives unchanged. The components never convert
  into each other.
- **`==` / `!=` are structural component equality**: total, defined on every
  pair, never an error. `1mo != 30d`; `1y == 12mo`; `7d == 168h`.
- **Ordering is a property of the type, not of a proof.** A declaration site
  names a unit class: `duration(ms)` is the **exact** class (zero months by
  construction), bare `duration` is the **calendar** class. `< <= > >=` and
  `sort` are legal only on the exact class — otherwise a **compile error**,
  class `incomparable_duration`, at the point of use. Admitting a
  months-bearing value into a `duration(ms)` field or parameter is a typed
  rejection **at the door**, class `duration_unit_class` — the one check that
  cannot be static, because the value arrives at call time.

**What the engine must stop planning for.** This entry's "compile error where
statically detectable, typed runtime error otherwise" is **withdrawn**, and so
is the analysis it implied: the spec explicitly withdrew the
prove-every-duration-is-single-component rule as *not provable* (any registered
op may write `1mo` into a duration field, and a parameter's value exists only
at call time). There is **no runtime comparability error class** to implement.
The engine implements two much cheaper things instead: a class check at the
declaration site, and a door-level bound check that reuses the machinery every
other parameter bound already uses.

**Still owed by the spec, and named here rather than pinned:** no result-type
rule for `date ± duration` exists anywhere (§1 blesses the operator, and the
type table pins only `date - date -> duration`), and no doc states which
component of a two-component duration applies first. The old pin's `date + 1mo`
clamps-to-month-end behaviour has no spec home. The engine cannot invent an
operator matrix that the host and SQL venues must agree on digit-for-digit;
this is findings X-008 / Z-007 and it is a spec ruling.

### G11. Text ordering/collation — **RULED 2026-08-30; this entry's ⚠ is withdrawn**
Determinism requires one answer. Engine-pinned: text sorts by **NFC codepoint
order** (locale-free, stable everywhere, matches the canonicalization section's
spirit). Locale-aware presentation ordering is a client concern.

**The ruling is in [`corpus-rulings.md`](corpus-rulings.md)** ("P0's last two
gaps"), and it confirms the pin — on the *language's* grounds, not the
engine's. Codepoint order is the only order that is a function of the value
alone; every alternative is a function of the value and a collation table that
ships on somebody else's release cadence. All three questions this section
listed as owed are answered there: yes it is codepoint order; NFC-on-write was
never open (DL §2 settles it — the gap is that **the engine does not do it**,
now owed as checklist 9u); and a per-sort-key locale order is refused, with its
cost stated and the in-language spelling (a stored sort-key field) named.

**Emitted, as of 9i.** The pin had no code behind it — zero `COLLATE`
anywhere — so `sort name` and the keyset seek predicate took the
*deployment's* `datcollate`. Two deployments, one plan, two orders; and
because keyset pagination's seek means "strictly after this row **in the
plan's order**", pages could skip or repeat rows across them. The planner
now emits `COLLATE "C"` on every text ordering (`ordo_plan::TEXT_COLLATION`):
`ORDER BY`, the keyset `seek` predicate and the outer page ordering, a
`SELECT DISTINCT`'s projection (Postgres requires the ORDER BY expression to
appear in the select list *verbatim*), `joined`'s ordered `string_agg`,
`ROW_NUMBER`'s window order, `min`/`max` over text — which are orderings —
and `whole`'s `json_agg` over the owned closure.

`C` is byte order over UTF-8, which for valid UTF-8 **is** codepoint order —
the same answer this section already pins, spelled in the one collation every
Postgres installation has. It needs no ICU build and no agreement about which
`en_US` a libc version means; glibc has changed that ordering across releases.

Not collated, deliberately: text **equality** — `=`, `DISTINCT`, `GROUP BY`,
`UNIQUE` — is byte equality under every *deterministic* collation, and
Postgres refuses a nondeterministic one as a database default. And nothing in
the generated DDL: pinning a *column's* collation would also repoint
`lower()`/`upper()` at `C`'s ctype and stop folding non-ASCII case, trading a
deterministic order for a wrong `normalize(lower)`. The cost of keeping it out
of the DDL is that a default-collation btree index cannot serve
`ORDER BY x COLLATE "C"`; the fix is a `COLLATE "C"` expression index per sort
key, and that is **owed work**, not a reason to move the pin into the columns.

**What the ruling changed here.** The engine chose the order it could defend
(`C` is present everywhere, no ICU build), and the ruling reaches the same
answer by a different route: codepoint order is the only order definable
without importing a third party's release cycle. So the pin stands and the
availability argument is demoted to a happy accident. `C` still puts `Z`
before `a` — the ruling states that as a **user-visible commitment** rather
than an engine detail, and refuses a per-sort-key locale order outright,
naming the stored sort-key field as the in-language spelling.

**One thing the ruling turned from a question into owed engine work.** DL §2
already settles NFC-on-write platform-wide, so the language's order has always
been codepoint order *over NFC* — and **nothing in this engine normalizes
anything**. `é` and `e´` are one text by the settled rule and sort apart,
compare unequal, and can both occupy a `unique` column. `COLLATE "C"` makes
the order deterministic given the bytes; NFC-on-write is what makes the bytes
deterministic given the text, and it is missing. Checklist 9u.

### G12. `hash(x)` algorithm — **RULED 2026-08-30; this entry's pin is withdrawn**
Stability across versions is promised; algorithm unspecified. Engine-pinned:
**SipHash-2-4 with a fixed published key**, truncated to i64. Frozen at first
release.

**The pin is withdrawn**, and the ruling is in
[`corpus-rulings.md`](corpus-rulings.md) ("P0's last two gaps"). `hash(x)` is
the **SHA-256** digest of the value's UTF-8 bytes, first eight bytes
big-endian, top bit cleared; `null` hashes to `null`. Two reasons the SipHash
pin does not survive: it is not in core Postgres (an extension, or a PL/pgSQL
transcription, for a function the *database* must compute — contradicting
G11's own "needs no ICU build" reasoning), and its *fixed published* key
removes the only property a key provides while adding the key to the
compatibility surface. SHA-256 can be held stable because it is not ours to
change: an external frozen standard with published vectors, in core since PG
11, and — unlike `md5()` — available under an OpenSSL FIPS provider.

`hash()` is back in the language with this ruling (9f″'s removal is retired);
`ordo_plan::hash64` holds the reference implementation next to its lowering,
and conformance proves the two venues agree digit-for-digit against a real
server.

### G13. `since` anchor precision [engine-pinned]
`since` on `date` fields compares against the execution-time UTC **date**; on
`datetime` fields against the execution-time UTC instant. (Conformance clock
pin: the harness executes at a fixed instant.)

### G14. Not gaps — confirmed intentionally absent
No `OFFSET`, no client-minted ids, no user-defined functions, no free-form
recursion, no windows (§11), no `now` in queries, no `HAVING`/`ON`/aliases.
The parser must *reject* these loudly, and the conformance suite tests the
rejections. Registration wrapper flags (`reproducible`, `unbounded`) parse as
part of G9's wrapper.

### G15. The gap in *this* document: authorization was absent from the build plan

Not a gap in the spec — a gap here, and the most serious one found. As
originally written, P0–P7 never mentioned access guards, `acl`, `$caller`, or
groups. The only "guard" named anywhere in the plan was the **fan-out** guard,
which is a cost rule and has nothing to do with authorization. There was no
phase in which guard binding, guard inlining, semi-join generation, or acl
derivation got built, and the definition of done could be met **with no
authorization system at all**.

That is not a scheduling oversight; it is the failure mode authorization has:
it is the one subsystem that fails *silently*. An app with no guards runs
perfectly and returns everyone's rows. Nothing in a green test suite catches
it, and a reviewer skimming a schema full of `access read` lines sees a
guarded app whether or not the compiler ever enforced one.

The correction runs through every phase below — P1 (lexing and parsing the
grammar), P2 (resolution and acl derivation), P3 (inlining, inheritance,
manifests), P5 (write preconditions), P6 (the guards corpus as a **release
gate**) — and through the definition of done, which is rewritten so that v1
cannot ship without it.

**Dependency, stated plainly**: guard *semantics* — inheritance and
replacement, link-entity non-inheritance, bulk-write scoping, anonymous
evaluation, acl level ordering, guest exposure, group resolution, and the
error-code rules — are currently normative-by-corpus in
`guards-conformance.md` §4 (R-G1…R-G18) rather than in the spec body. The
engine implements those rules either way; the fold-into-spec pass is the
spec's work, not the engine's, and it is tracked there. Where a rule is
contested (notably the addressed-write-on-an-unreadable-row error, which
`api-engine-design.md` §4.1 resolves toward non-enumerability), the engine
implements §4.1 and the corpus is updated to match.

### G16. The same gap, again: the platform substrate had no build home

G15 found that authorization was absent from the build plan and threaded it
through P1–P6. The correction was right and it was **partial**, because it
treated the omission as a missing *feature* rather than as a symptom of the
plan's *shape*. This document is a compiler plan — lex, resolve, bind, plan,
write, conform, CLI — and a compiler plan has no place to put a subsystem that
is not a compiler pass. So every settled capability that is neither a pass nor
a pass's output fell out of it. Two of them are named by the coverage gate,
and three more are not named by anything:

- **Blind index** (`definition-language.md`, Sealed fields;
  `implementation.md`, Key custody) — appeared here only as the *name of a cost
  class*, not as a thing anyone builds.
- **Command log** (`security-logging-design.md` §7) — appeared here only as
  "command-record emission shape", i.e. as an obligation the engine hands to a
  recipient this plan does not say exists.
- **The custody substrate** underneath both — `KeyProvider`, the space master
  key, DEK epochs, epoch tags, AEAD sealing (`implementation.md`, Server stack
  & Key custody).
- **The security-event stream** (`security-logging-design.md` §6.1).
- **Runtime shape verification and the capability catalog**
  (`implementation.md`, Runtime shape verification) — settled, load-bearing for
  the SDK and the LLM-facing surface, and named in no plan at all.

**Three pieces of evidence that this is structural and not a scheduling slip.**

1. **The plan does not describe the workspace that exists.** "Shape of the
   engine" below listed seven crates. Four crates exist on disk, and **three of
   them are not on that list** — `ordo-identity`, `ordo-commandlog`,
   `ordo-api`, roughly 6,000 lines, already written and already tested. Six of
   the seven planned crates do not exist. A build plan that cannot see three
   quarters of the build is not sequencing it. `README.md` ("Current status")
   describes the real tree correctly; this document was the stale one.
2. **A settled subsystem was built from a superseded reading, and no phase
   owned catching it.** `crates/ordo-commandlog/src/value.rs` defines
   `SealedRef([u8; 32])`, documented as "SHA-256 of the sealed ciphertext".
   `security-logging-design.md` §7 **pins the opposite** — "[pinned — corrects
   the earlier 'SHA-256 of the ciphertext']", a **random 128-bit handle** minted
   per write — because a content hash turns the command log into an unsalted,
   permanent equality index over every sealed field that survives
   crypto-erasure. The crate implements the rejected design, inside the
   canonical encoding — which §7 pins and the crate's own comment calls
   frozen. That is exactly what "no build home" produces: code
   written against whichever reading was current when someone reached for it,
   with no phase whose definition of done says *check this*.
3. **The engine's own definition of done is unsatisfiable without the
   substrate.** Criterion 3 requires C001–C118 to pass **in the execution
   tier**. The corpus fixture (`composition-conformance.md` §2.1) declares
   `customers.email: text? sealed indexed unique normalize(lower)` and
   `orders.memo: text = "" sealed`, and §K (C084–C091) executes against them —
   C084 resolves a blind-index equality to one row, C088 evaluates a host-side
   predicate over a decrypted sealed field, and `cust_3` carries the null email
   that exercises the sentinel. **There is no ordering of P0–P7 in which
   criterion 3 passes without a real key provider, real AEAD sealing, and a
   real HMAC blind index.** The substrate was not a follow-on to this plan;
   it was a prerequisite inside it, unwritten.

**The ruling.** This document becomes the build plan for the **system**, not
for the compiler alone. It carries two tracks — **Track P**, the compiler,
whose phase names `P0`–`P7` are unchanged because other documents cite them
(`security-logging-design.md` §6, `eval-harness-plan.md`, `adoption-dx-plan.md`,
`README.md`), and **Track F**, the foundation the compiler's output runs on —
plus an explicit dependency map and joint milestones where the two must meet.
Every phase in both tracks carries a definition of done, and where a corpus
exists it is the definition of done rather than a description of one.

**And a note on the gate that found this.** `review/checks.config.json`'s
`plan-coverage` check reported `blind index` and `command log` absent. Both
terms were two hyphens away from present — this document already said
`blind-index` and `command-log`. Editing the hyphens would have turned the gate
green and changed nothing true. The gate is a **proxy for the question "does
somebody build this"**, and the honest answer was no; that is what the tracks
below fix. Recorded here so that the next person to see a coverage failure
reaches for a phase and not for a find-and-replace.

---

## Part 2 — Build plan

### Shape of the system [rewritten per G16]

Per `implementation.md`: Rust, server-only compiler, compile-once artifacts,
and — the part this section used to omit — **one static `ordo` binary with a
role flag (api / worker / all-in-one) whose only mandatory stateful dependency
is Postgres** (`implementation.md`, Deployment topology & Server stack). The
compiler is a library inside that binary, not a separate product. That is why
the two tracks share one plan: they ship as one artifact.

One Cargo workspace at the repo root (`~/DevWorkspace/ordo`), library-first so
the compiler embeds in the platform binary and compiles to WASM later.
**`now` marks what exists on disk today; everything else is owed by the phase
named beside it.**

```
ordo/
  Cargo.toml            # workspace
  crates/
    # ── Track P: the compiler ────────────────────────────────────────────
    ordo-syntax         # now  · lexer, AST, hand-written recursive-descent parser
    ordo-schema         # P2   · entity graph: refs, ownership, trees, sealed, indexes,
                        #        acl derivation, per-entity effective guard table
    ordo-sema           # P3   · binder + all compile rules (the spec's teeth), guard inlining
    ordo-plan           # P4   · SQL planner: pipeline -> PostgreSQL, DDL gen, cost classes
    ordo-artifact       # P4/P5· registration metadata: mode labels, cost class,
                        #        scope manifest, effective guard set
    ordo-cli            # P7   · `ordo` binary: check / plan / (later) apply / dev
    ordo-conformance    # P6   · corpus harness: golden SQL + seeded-Postgres execution
    # ── Track F: the foundation ──────────────────────────────────────────
    ordo-crypto         # F0   · KeyProvider trait, SMK/DEK epochs, AEAD seal/open,
                        #        SealedRef minting, blind-index key derivation (F1)
    ordo-commandlog     # now  · command types, canonical encoding, per-space chain,
                        #        Store trait, checkpoints, revert planning        (F2, F3)
    ordo-identity       # now  · principals, providers, directory, access engine  (F4)
    ordo-api            # now  · HTTP surface generated from the artifact registry(F4)
    ordo-exec           # F5 (partial, built) · pool, transactions, read executor,
                        #        sealed stages, chain append, triggers, job queue
    ordo-catalog        # F6   · runtime shape verification, capability catalog
    ordo-vault          # F7   · client-first: proposals + receipts, artifact-pinned
                        #        folding, sealed snapshots, compaction floor
  docs/                 # (existing)
```

Two crate-map notes, both consequences of G16:

- `ordo-crypto` is **new to this plan and old to the design** —
  `implementation.md` (Server stack) settles RustCrypto,
  XChaCha20-Poly1305, chunked STREAM for blobs, HMAC blind indexes, and
  "**key custody behind a `KeyProvider` trait from day one**". Day one had no
  phase; it does now (F0).
- `ordo-commandlog` and `ordo-identity` are listed in
  `security-logging-design.md` §9 as **"(now)"** with their contents
  enumerated. They were built to that list. The gap was not that nobody knew
  what to build — it was that nothing sequenced it against the compiler or
  gated it on a corpus.

**Parser ruling**: hand-written recursive descent, not chumsky/pest
(`implementation.md` left the pick to build time). Reasons: the grammar is
closed and line-oriented (G6), "fails loudly with the right words" is a core
product property — hand-rolled gives exact control of every error message —
and zero parser deps keeps the WASM story trivial. *(`implementation.md`'s
Compiler section still reads "Parser: chumsky or pest (final pick at build
time)"; the pick is made here and that line is owed an update — spec-side
housekeeping, not engine work.)*

### Two tracks, one plan [structure]

**Track P — the compiler.** P0–P7, unchanged in name and largely unchanged in
content: source text in, artifacts out. Its output is data (planned SQL, mode
label, cost class, scope manifest, effective guard set) and it runs at deploy
time, not on the hot path.

**Track F — the foundation.** F0–F7: the things the artifacts run *on*, and
the things that must be true before an artifact can be trusted. Track F is not
"phase two". Three of its phases block Track P's own definition of done.

**Why one document and not two.** Splitting the compiler plan from a platform
plan was considered and rejected — see "Options considered" at the end. The
short version: the hole this restructure fixes was created by a boundary
exactly like that one, and the coverage gate reads a single plan file, so a
second document would be a second place for a pillar to go missing unwatched.

#### Dependency map

The only edges that matter are the ones that cross tracks. Read "→" as
"must land before".

| Edge | Why | Evidence |
|---|---|---|
| **F0 → P6** | The composition fixture declares a sealed field and a sealed-indexed field; the execution tier must actually seal and open them | `composition-conformance.md` §2.1, §K |
| **F1 → P4** | `blind-index` is a cost class the planner *assigns*; assigning it to a plan nothing can execute is an unverified label | `definition-language.md`, Cost classes |
| **F1 → P6** | C084, C085, C089 execute blind-index equality against `customers.email`, including the null sentinel on `cust_3` | `composition-conformance.md` §K |
| **F0 → F2** | The command log's redaction rule stores a `SealedRef`; the ref's shape is F0's to mint and F2's to hash | `security-logging-design.md` §7 |
| **P5 → F2** | The engine derives the command-record shape (before/after images, group boundaries); the log appends it | `security-logging-design.md` §7 |
| **P5 → F3** | The engine emits the row-level **decision**; the stream records it. A bare wire code reaching F3 has already destroyed the distinction | `api-engine-design.md` §4.1, `security-logging-design.md` §6.1 |
| **P2/P3 → F4** | The door decides *whether the op runs*; guards decide *which rows it sees*. Groups appear in both and must mean the same thing in both | `security-logging-design.md` §6 |
| **P4/P5 → F5** | The executor runs prepared plans; there is nothing to execute before the planner emits them | `implementation.md`, Compiler |
| **F5 → P6** | The execution tier of the guards corpus asserts per-principal **row sets**, which requires running plans under a bound principal and space, not just diffing SQL | `guards-conformance.md`, DoD criterion 4 |
| **P4 → F6** | Shape verification compares the compiler's *inferred* shape against Postgres's *observed* shape; both sides must exist | `implementation.md`, Runtime shape verification |

Two edges deserve to be stated as prose because they are the ones that were
being got wrong:

- **F0 and F1 are prerequisites of Track P's definition of done, not
  successors to it.** This is the whole finding of G16.3.
- **F2 and F3 are downstream of P5 and upstream of nothing in Track P.** They
  can be built in parallel with P2–P4 and must not wait for P7.

### Phases

#### Track P — the compiler

**P0 — Formalization (this doc).** Gap rulings above; fold pinned items into
the spec when confirmed.

*Done when:* G10, G11 and G12 are ruled rather than pinned here, and G16's two
tracks have owners.

*Status (2026-08-30, checklist 9t):* **the rulings are complete.** G10 was
ruled at spec level in `definition-language.md` §1; G11 and G12 are ruled in
[`corpus-rulings.md`](corpus-rulings.md), which is where this project's
rulings live — the done-when said `definition-language.md` because that was
the only rulings home when it was written, and a ruling recorded in the file
the corpus and the compiler both cite is the same commitment in a better
place. No ⚠ remains in this section.

What is left of P0 is **G16's two tracks having owners**, which is a
scheduling fact and not a ruling, plus the two owed *engine* items the
rulings surfaced (NFC-on-write, checklist 9u; the `COLLATE "C"` and `hash`
expression indexes). Those are implementation debts with an answer, not open
questions — which is the difference P0 is measured by.

**P1 — Syntax.** Lexer (tokens, literals incl. dates/durations/decimals,
line-aware; keyword table generated per G3, with the drift test). AST as real
ADTs. Parser for: `entity` decls (fields, refs,
`owned`/`tree`/`sealed`/`indexed`/`unique`/`normalize`/`check`/`index`),
`query` wrapper + read pipelines (source forms, conditions, projections,
`group`/`reduce`, `sort`, `take`/`take[keys]`, `append`, `let`, `match`,
`distinct`), write statements (`set`/`del`/`subtree`/`each`), `routine`/trigger
headers, `cond`, the expression grammar. Every spec rejection = a distinct
error with the offending span. Unit tests per construct, accept + reject.

**Authorization is P1 work, not a later layer** (G15):

- **`access read` / `access write` clauses** inside `entity` bodies: the
  `$caller`-anchored relationship walk, its `or`-combinations, and the closed
  condition grammar it admits — with a *specific* error for the arbitrary
  predicate the grammar does not admit, because that is the error an author
  hits first. Includes the **explicit-open write spelling `access write open`**
  and the paired-clause rule: an `access` declaration is **both verbs or
  neither**, so `access read` without `access write` is
  `incomplete_access_clause` at parse/bind time, not a permissive default.
- **`acl (level, level, …)`** declarations, including level order (it is
  significant) and the derived-name implications.
- **`$caller`** as a distinguished parameter token — lexed and resolved
  differently from an ordinary `$param`, since it is never bound by the caller
  and must be rejected as an explicit argument at a call site.
- **Group references** inside guard walks (the access model's group layer),
  parsed as an ordinary relationship hop so that group support costs the parser
  nothing beyond recognizing the shape.
- **Registration flags** `unguarded` / `unbounded` / `reproducible` on G9's
  wrapper, each with its own reserved word and its own rejection message.
- Accept/reject unit tests for every one of these, including the negative that
  matters most: `entity x { read: int }` and friends must now parse
  unambiguously per G3's two-tier rule.

*Done when:* the keyword-drift test (G3) passes against `token.rs`; every
`.ordo` block in `syntax-examples.md`, `tutorial.md` and the two conformance
corpora lexes and parses; every §-listed illegal form is rejected with a
spanned error naming the rule. The compile tier of the eval harness
(`eval-harness-plan.md`, "P1–P4 land") becomes runnable here.

**P2 — Schema graph.** Entity table from decls: ref edges, one-owner rule +
link-entity + tree-root exceptions, tree marks, implicit `id`/`rev`/`depth`,
unique sets, sealed/blind-index marks, default classification. DDL generation
(CREATE TABLE, CHECK, indexes, FKs) — golden-tested.

**`check` may walk refs, and that is not a plain `CHECK`** [settled at spec
level, `definition-language.md` §3]. An invariant may be about a row *and the
rows it points at* — `check assignee.org == customer.org` on `tickets` — and
the walk follows **outgoing refs from the row being written** (many→one, the
same direction dot-paths always go), at **any depth**. The spec's obligation on
the engine is specific: it "compiles to a constraint the database enforces
**inside the write's transaction**", and a violation is a typed rejection that
aborts the whole command group like any other `check`.

- A dot-path `check` therefore **cannot** be emitted as a Postgres `CHECK`
  constraint, which is single-table by definition. P2 must pick and golden-pin
  one mechanism — a `CONSTRAINT TRIGGER` on the child (fired on insert and on
  update of the participating columns), or a generated/denormalized column kept
  in step by the same trigger and covered by an ordinary `CHECK`. The choice is
  an implementation detail; *having* the choice recorded is not, because the
  two differ in what happens when the **parent** row changes after the fact.
- **The parent-side obligation is the sharp edge.** `assignee.org` can be
  falsified by a write to `members`, not only by a write to `tickets`. Whether
  the constraint is re-checked on parent updates is a decision this plan owes
  a golden case for; naming it here so it is chosen rather than discovered.
- **The bound is the useful half and the engine enforces it at compile time**:
  the walk may not aggregate, count, or test existence over a collection. Those
  are questions about *other rows*, stay rule-tier (a maintained counter field
  feeding a row-scoped `check`), and a `check` that reaches for them is a
  compile error at the shape, not a slow constraint at runtime.

**Guard resolution and acl derivation live here** (G15) — they are schema-graph
facts, computed once, not per-op:

- **`acl` derivation**: mint the link entity (`projectsAccess`-shaped) as a
  real entity in the graph with its refs, level enum, and unique key, so it
  receives DDL, std ops, and guards like anything else. Golden-test the derived
  DDL — a derived entity that differs from a hand-written one is a bug.
- **Guard resolution per entity**: for every entity, compute its *effective*
  guard pair (read, write) from (a) its own `access` clauses, (b) the ACL
  inherited through the **ownership graph** from an `acl`-bearing root, and (c)
  the replacement rule — an explicit clause **replaces** the inherited guard,
  with no composition algebra. **The per-verb-vs-whole-clause question is
  closed** [settled — this plan previously carried it as an owed ruling]: since
  an `access` declaration is both verbs or neither, whole-clause replacement and
  per-verb replacement are the *same* rule and there is no partial clause to
  disagree about. The ambiguous case the engine was told to fail loudly on no
  longer exists; what the engine enforces instead is the completeness error at
  the declaration (`incomplete_access_clause`), which is a better error at a
  better place.
- **Non-inheritance rules**: link entities and tree-root exceptions per the
  guards corpus.
- **Guard reachability check**: every guard walk must resolve to declared refs;
  an unresolvable walk is a compile error at the shape, not a runtime false.
- **Group resolution**: a group hop resolves to its membership entity in the
  same graph, so the semi-join is generated identically whether membership is
  direct or via a group.
- Output: a per-entity guard table the binder consumes. It is data, and it is
  dumpable — `ordo check` can print the effective guard for every entity, which
  is the cheapest possible defence against "looks guarded, isn't".

**The sealed/blind-index half of the schema graph is P2's too** (G16), because
a **blind index** is a column and a constraint before it is anything else:

- A `sealed indexed` field contributes a **plaintext digest column** alongside
  the sealed payload; a `sealed indexed unique` field contributes
  `UNIQUE (space, hmac)` (`definition-language.md`, uniqueness — the HMAC key
  is already per-space, so the space column is belt-and-braces and is
  golden-pinned either way). The digest column and its index are ordinary DDL
  output and are golden-tested like every other table.
- The **null sentinel** is schema-graph data, not planner data: a nullable
  `sealed indexed unique` field HMACs a reserved sentinel so uniqueness holds
  over nulls, and the shape summary must carry the disclosure that follows —
  **the blind index conceals values, never nullness**
  (`implementation.md`, Key custody). The compiler emits that disclosure; it is
  not documentation anyone remembers to write.
- The graph records, per sealed field, which **key** its index derives under —
  `BIK = KDF(SMK, "blind-index", entity, field)`, domain-separated per entity
  and field and **independent of the DEK epoch** (`implementation.md`, Key
  custody). The derivation is F0/F1's to implement; the *entity and field
  names that feed it* are P2's, and they are frozen by that derivation: a
  rename is a re-index, and the schema graph is the only place that can say so.
- Sealed fields also constrain the derived std surface: entities containing
  sealed fields derive their std read surface over the **open skeleton only**
  unless the shape carries **`std sealed`**, the one greppable word that opts
  the whole entity's derived ops into the sealed-inclusive manifest
  (`std-library.md` §2.4, `definition-language.md`, Sealed fields). Write
  params are unaffected — sealed fields stay writable, because writing a value
  is not disclosing it. That is a schema-graph fact, computed once.
- **Refinability is a separate opt-in with a separate spelling**:
  `sealed indexed refinable`, defaulting to not-refinable, and where enabled
  the operator set is closed to `eq` and `in` (`std-library.md` §2.1). The
  marker does not imply refinability and refinability does not imply the
  marker; P2 records both flags and P4/F4 enforce their consequences.

*Done when:* golden DDL for the `market` fixture (`composition-conformance.md`
§2.1) is byte-stable, including `categories`' tree columns, `orderItems`'
ownership FK, and `customers.email`'s digest column and unique constraint; the
`acl`-derived link entity's DDL is byte-identical to the hand-written
equivalent; `ordo check` prints an effective read/write guard for every entity
with its provenance; and a guard walk that does not resolve to declared refs is
a compile error at the shape.

**P3 — Binding & rules.** Resolve pipelines against the graph: dot-path→join
chains, **match inference over the settled path rule** — simple paths in the
**undirected** ref multigraph, transitive iff exactly one path exists, a unique
**direct** (length-1) edge winning over any longer path, parallel edges counted
as distinct paths, tree self-loops excluded, and genuine ambiguity raising the
enumerate-the-paths error (this plan previously pinned inference to direct refs
only, which the spec's own bridged example `projects ← tasks ← receipts`
contradicts) — fan-out guard,
entity-once, let-scalar rule, group-brace aggregate-only rule, null truth
table typing, cond totality/type-agreement, projection/rename rules,
field-scope manifest derivation. Named-query composition: registry, binding
($ flow-through), output-field conditions, inlining with post-take semantics,
cycle rejection (per the formal section + R1–R17 rulings in
`composition-conformance.md`).

**Guard inlining is the other half of P3** (G15), and it is what makes the
whole design's central claim true:

- **Inline the effective read guard into every op touching the entity** — as a
  semi-join, in every source position, including inside `let`, inside composed
  ops, inside `append` branches, and inside `whole`/subtree traversals. "With
  no exception path" is a property the binder either has or does not; there is
  no partial version.
- **The one exception is explicit**: an op carrying `unguarded` skips
  inlining, and an op *composing* an `unguarded` op must carry the flag itself
  or fail (the unacknowledged-bypass error). Greppability is the point.
- **Inherited guards appear in the composing op's scope manifest**, marked as
  inherited and naming the entity they came from — the manifest is what the
  consent page and the catalog render, so a guard that inlines but does not
  disclose is half-built.
- **Anonymous / no-membership evaluation**: a principal holding no rows makes
  the walk false, which is correct for humans and *silently wrong* for
  platform-born callers (routines have no membership row). The binder therefore
  knows a caller *kind*, and **the routine case is now settled by trigger
  provenance, not by the principal**: a **clock-woken** routine runs as
  `routine:` with guards passing, while a **listener-woken** routine is scoped
  to `$trigger.ids` — the rows its own trigger delivered — unless it carries
  `unguarded`. The engine implements the bound as a **source-set restriction**
  at bind time, not as a runtime filter, so a listener routine that reads
  outside its trigger set without the flag is a compile error naming the source.
  Any explanation of routine authority that appeals to "the principal is
  trusted" alone is stale; the trigger is what bounds it
  (`definition-language.md` §8).
- **Guest and cross-space evaluation**: a guard evaluated for a guest principal
  resolves against the granting space's rows only.
- **Cost**: guard semi-joins are `sql` class and must stay so — a guard that
  forces an op host-side is a compile error, not a slow op. The sharp edge of
  that rule is sealed data: **a guard predicate referencing a sealed field,
  anywhere in the walk or its inner conditions, is a compile error, class
  `guard_on_sealed`** (`guards-conformance.md` R-G1, cases GC074/GC075). A
  guard that had to decrypt to decide would be a host-side authorization
  check, which is the thing this design does not have.

*Done when:* every C001–C118 and GC001–GC087 case reaches a bound IR or its
named rejection in the **compile** tier; the scope manifest of a composed op
lists inherited guards with the entity they came from; `guard_on_sealed`,
`unacknowledged_unbounded` and the unacknowledged-bypass error each have a
corpus case that produces them and no other.

**P4 — Planner.** Pipeline → PostgreSQL SELECT (joins from dot-paths,
DISTINCT ON-free take[keys] via ROW_NUMBER or LATERAL — planner's choice,
golden-pinned), group/reduce, append as UNION ALL, let as CTEs, tree
under/above via materialized-path columns, id tiebreaker on every ordering,
param binding slots. Cost-class classification (sql / blind-index /
bounded-host / unbounded-host + the `unbounded` gate, which now covers **row
cardinality** as well as venue — every op needs a provable row bound or the
`unbounded` acknowledgment).

**Exact-decimal arithmetic is P4's other pinned surface**, and it is where the
division ruling lands:

- All arithmetic maps to `NUMERIC`; no float appears anywhere in a plan.
- **Division yields `decimal` at a fixed fractional scale of 12, with no
  implicit rounding** — the scale is a property of the result type, not of the
  operands, so the same expression produces the same scale in every plan and in
  every deployment. Nothing rounds unless the author writes a rounding step;
  silent rounding at an unnamed scale is how money code becomes untrustworthy.
- **`allocate()` is a planner primitive, not a library function**: splitting an
  amount into parts that sum *exactly* to the original is the operation
  everybody hand-rolls and everybody gets wrong at the last cent. It plans as a
  deterministic remainder distribution, golden-tested per case, and it is the
  reason division may be honest about its scale rather than pretending to
  reconcile.
- Golden tests cover the scale of every arithmetic result type and the
  `allocate()` remainder cases; a scale drift is a golden diff, not a rounding
  surprise found in production.

**Sealed staging is P4's third surface** (G16), because the planner is what
decides where a sealed value is touched:

- A plan over an entity with sealed fields **splits into an SQL stage and a
  host stage**. The SQL stage may reference the plaintext skeleton and the
  **blind-index digest column**; it may never reference a sealed value.
  Equality and `in` over a `sealed indexed` field push into SQL as digest
  comparisons and classify **`blind-index`**; ranges, `contains`, sorting and
  aggregation over sealed fields stay host-side after decryption
  (`definition-language.md`, Sealed fields).
- **Sealed sort keys with keyset pagination are prohibited** — the op still
  registers, and requesting a page token against it is the runtime rejection
  `sealed_keyset_pagination` (`composition-conformance.md` R12, C090).
- The plan records, per host stage, **which fields it will need released**.
  Key release is derived from op access and nothing else — "a caller allowed
  to invoke an op whose manifest includes sealed fields thereby (and only
  thereby) triggers release for exactly those fields, for that run"
  (`definition-language.md` §8) — so the release set is an **artifact field
  the planner computes**, not a runtime decision the executor improvises.
- **The cost class is a claim the planner makes about machinery F0/F1 owns.**
  Labelling an op `blind-index` when no blind index can be computed is the
  precise failure G16 is about; the label is verified by C084/C085/C089 in the
  execution tier, which is why F1 → P4 is an edge in the dependency map.

*Done when:* golden SQL + artifact metadata (mode label, cost class, row bound,
scope manifest, effective guard set, release set) is byte-stable for every
C001–C118 case in the **compile** tier; the `market` fixture's blind-index ops
emit a digest comparison and never a sealed column reference; every arithmetic
result type's scale is golden-pinned.

**P5 — Writes.** Mode derivation (identity precedence rules 1–10 §6), SQL for
each mode (INSERT / UPDATE by id / CAS with rev / INSERT ON CONFLICT for the
one blessed upsert / bulk UPDATE), insert-completeness check, command-record
emission shape (before/after capture plan), atomic group boundaries.

**Write guards are preconditions, and they are P5's hardest case** (G15):

- **Addressed writes**: the write guard compiles to a precondition evaluated
  against the row, failing with a loud typed rejection — never a silent
  no-op, never a zero-row `UPDATE` reported as success. The error depends on
  visibility: a row outside the caller's **read** view answers `not_found`, a
  visible-but-unwritable row answers `access_guard`
  (`api-engine-design.md` §4.1).
- **Creating writes** evaluate the guard against the **would-be row** — the
  values being written, not an existing row — which is what makes "can this
  caller create this?" answerable at all, and which is also the mechanism most
  likely to produce a bootstrap deadlock (nobody can create the first row that
  would satisfy the guard). The engine must make that case a **specific,
  named** compile-or-runtime error that says *which* guard could never be
  satisfied by a creating write, because the alternative is an author staring
  at a permission system that derives everything except its own first row.
  - **G10a, the create-exemption, is part of the rule, not an escape from it**
    [settled]. **Correction:** an earlier revision of this entry attached the
    exemption to creating writes on `acl`-derived **link** rows. That is the
    wrong entity and contradicts `definition-language.md` §3 (G10a and acl
    bootstrap rules 2–3). The exemption is on the **acl root** — the entity
    carrying the `acl` clause — not on the derived link:
    - The test is **static and per-entity**, decided at registration: a write
      guard is create-exempt iff **every** `$caller`-referencing disjunct of
      its walk begins with a **reverse collection step rooted at the entity
      being created**. On an acl root the derived guard is
      `<entity>Access(member == $caller …)` — a reverse step into the derived
      link, whose rows cannot exist before the root row does. Exempt.
    - The **derived link entity is not exempt**. Its own write guard (top level
      on the parent row) is rooted at the **parent**, which already exists, so
      it is a real question with a real subject and the engine evaluates it
      normally. The creator's auto-grant lands not by exemption but because it
      is minted **in the same command group** as the create, after the create,
      under the platform's own bootstrap rule.
    - There is **one rule, not one for `acl` and one for hand-rolled clauses**:
      an explicitly declared `access write documentsAccess(member == $caller
      and canWrite)` on `documents` is exempt by exactly the same test. The
      engine implements the test, not a special case keyed on the `acl`
      keyword.
    - A guard **mixing** self-rooted and parent-rooted disjuncts is **not**
      exempt — the parent-rooted disjunct is answerable, so it is answered.
    - The artifact must **disclose** the outcome per entity
      (`create: guard-exempt (self-rooted guard)`), because what bounds an
      exempt create is the door plus the op body, and a reviewer needs to see
      which creates are door-gated only.
    The **space-owner override is checked at the door**, not inside the guard —
    so the exemption is a door decision the engine consumes, never a row-level
    bypass the engine invents. P6 carries the negative twin: a non-owner,
    non-admin creating a row in a **derived link** entity must still be
    refused, and a **guest** (members row, null `role`) creating an acl-root
    row must receive **no** auto-grant — auto-grant follows space membership,
    not authorship.
- **Bulk writes** scope to the guard's row set: the guard is a filter on the
  target set *and* a precondition, so a bulk `set` never touches a row the
  caller could not have addressed individually. Conformance must include the
  case where those two readings would differ.
- **`del … subtree`** evaluates guards over the whole subtree, not the anchor
  alone.
- **acl admin ops** (grant / revoke / level change) derive with the
  highest-declared-level gate, and their generated SQL is golden-tested like
  any other write.
- **Command-log emission carries the guard outcome context** the platform needs
  for attribution. Denials are not state changes and **never enter the command
  chain**; they are recorded in the space's separate, hash-chained
  **security-event stream** (`security-logging-design.md` §6.1), which has its
  own head, retention, and lock and is platform work rather than engine work.
  The engine's obligation is the half only it can supply: emit the **decision**
  behind each row-level rejection — guard-failed distinctly from row-absent —
  so the stream can record what the wire deliberately will not say
  (`api-engine-design.md` §4.1). A rejection that reaches the platform as a bare
  wire code has already destroyed the distinction the stream exists to keep.
  The recipient is no longer hypothetical: the chain is **F2**, the stream is
  **F3**, and both are in this plan.

**What P5 owes the command log, stated as an interface** (G16 — this used to
read "command-record emission shape (before/after capture plan)", which named
an obligation without naming who receives it):

- **Per-row, resolved before/after images** — never the token `default`.
  Create records `after` only, delete records `before` only, converge records
  both; a shape violation is a typed error **at append**, so the engine is
  responsible for never producing one (`security-logging-design.md` §7).
- **Group boundaries.** All commands from one invocation share a correlation
  id and commit in one database transaction; a group is **single-space**. The
  engine decides the boundary; F5 holds the transaction; F2 hashes the group.
- **Sealed fields never appear in an image.** The engine emits a `SealedRef`
  placeholder and F0 mints the actual handle. The engine must not be the
  component that decides what a ref *is* — that is exactly how the current
  `ordo-commandlog` came to hold a content hash (G16.2).
- **Attribution is supplied, not inferred by the log**: `user:`, `routine:`,
  `agent:… on behalf of user:`, `source:`, `system:`. The routine case is the
  one with a compile-time component — clock-woken versus listener-woken
  changes both the guard bound (P3) and the attributed actor.

*Done when:* every write construct in the corpora derives its mode label per
§6's precedence rules 1–10; addressed writes on unreadable rows answer
`not_found` and on unwritable-but-visible rows answer `access_guard`
(`api-engine-design.md` §4.1); the `acl`-derived creating-write exemption and
its non-owner negative twin both have corpus cases; and every generated write
emits a command-group description that F2 accepts without a shape error.

**P6 — Conformance harness.** Runner over `composition-conformance.md` C001–
C118 plus a language corpus grown from the two SQL-translation corpora and
§12's priority list: per case parse → bind → golden artifact (SQL + metadata)
→ execute against a seeded dockerized Postgres → canonical-JSON compare.
Compile-only tier runs without a database; execution tier gates releases.

**`guards-conformance.md` GC001–GC087 is a first-class P6 target, not a
follow-on** (G15). It is the only corpus that can prove the authorization
system exists, and it has properties the composition corpus does not:

- **Every case runs in the execution tier with seeded fixtures** — multiple
  principals, membership rows, acl rows, group rows, guest grants — and asserts
  the **row set** each principal sees, not just the SQL. Golden SQL cannot
  catch a guard that inlines correctly and resolves to the wrong rows.
- **Negative cases are the point.** Each positive case has an adversarial twin:
  the principal who must see nothing, the write that must be refused, the
  composition that must fail to compile without `unguarded`.
- **The oracle assertions run here too**: absent-vs-forbidden answer the same
  code, and their cost profiles stay comparable (`api-engine-design.md` §4.1).
- **A "no guards at all" canary**: a fixture domain with `access` clauses whose
  enforcement is deliberately disabled must make the corpus **fail loudly**. If
  a broken authorization system can pass P6, P6 is not testing it — and that
  canary is the only test that proves otherwise.

**P6 is where Track F stops being optional** (G16.3). The execution tier is not
"the compile tier plus a database": the `market` fixture declares
`customers.email: text? sealed indexed unique normalize(lower)` and
`orders.memo: text = "" sealed` (`composition-conformance.md` §2.1), and §K
executes against them — C084 resolves a blind-index equality to one row, C088
evaluates a host-side predicate over a decrypted value, and `cust_3`'s null
email exercises the sentinel. **Running C084 requires a key provider, a derived
`BIK`, and a real HMAC.** The harness therefore takes F0 and F1 as
dependencies, and it runs them for real rather than against a stub:

- **No mock key provider in the execution tier.** A local-master-key
  `KeyProvider` is a legitimate implementation (`implementation.md`, Server
  stack); a test double that returns plaintext is not, because it makes exactly
  the cases that prove the sealed substrate works pass without it.
- **The harness owns the two corpora Track F is missing** — see "Owed corpora"
  below. P6 is the runner; the cases are documents, and they do not exist yet.

*Done when:* C001–C118 and GC001–GC087 pass in the **execution** tier against
seeded dockerized Postgres with a real local-key `KeyProvider`; the no-guards
canary fails when enforcement is disabled; compile-tier runs green with no
database.

**P7 — CLI.** `ordo check <dir>` (parse+bind+cost, human errors), `ordo plan`
(diff-shaped output; DDL for now), artifact JSON emission. `ordo apply`, the
registry, hot swap and `ordo dev` are **F6**, not "the platform phase, not the
engine" — they are in this plan now, one track over.

*Done when:* `ordo check` accepts every legal construct in the spec's worked
examples, `syntax-examples.md` and `tutorial.md`, rejects every §-listed
illegal form with a spanned error naming the rule, and prints the per-entity
effective guard table (DoD criterion 5); `ordo plan` emits byte-stable artifact
JSON.

#### Track F — the foundation

Track F is the substrate Track P's artifacts run on. It is sequenced **early
and in parallel**, not after P7, for the reason G16 gives: F0 and F1 are
prerequisites of Track P's own definition of done, and F2/F3 are the recipients
of obligations P5 emits into the void today.

Track F's phases are smaller than Track P's because most of their content is
already settled prose — `implementation.md` (Server stack, Key custody,
Deployment topology, Write path, Runtime shape verification) and
`security-logging-design.md` (§6, §6.1, §7, §9). The work is building them and
**gating them on a corpus**, which is the half still missing.

---

**F0 — Custody substrate.** `ordo-crypto`. The `KeyProvider` trait "from day
one" (`implementation.md`, Server stack) with a local-master-key implementation
first and KMS/Vault behind the same trait; per-space **space master key
(SMK)**; the SMK-wrapped series of per-epoch **DEKs**; AEAD sealing
(XChaCha20-Poly1305, chunked STREAM for blobs) with **a fresh random nonce per
seal** and a sealed payload store that **never deduplicates**; and the
**epoch tag** on every sealed row payload, blob chunk header, command image and
archive bundle.

Two rules land here and nowhere else, because they are the ones the rest of the
system assumes without being able to check:

- **`SealedRef` is a random 128-bit handle minted per sealed image write**,
  salted per space and per epoch in its derivation, stored alongside the payload
  it names — **never a content hash** (`security-logging-design.md` §7,
  `[pinned]`). `crates/ordo-commandlog/src/value.rs` currently holds
  `SealedRef([u8; 32])` documented as "SHA-256 of the sealed ciphertext", and
  `chain.rs` encodes it as 32 raw bytes under a tag the crate calls frozen. **F0
  corrects both**, and the correction is a canonical-encoding change, so it
  must land before any chain exists that anyone would be sad to invalidate.
- **Epoch tags ship before first customer data** even if rotation itself lands
  later (`implementation.md`, Key custody). Retrofitting a tag onto unversioned
  ciphertext is an unwelcome migration; one byte now, or a re-tag job later.

Also F0: `Signer` (implementations ride `KeyProvider`), the **startup assertion
that no key material can reach a database backup** (`implementation.md`,
Backup), and the deployment invariant that the egress identity has **no
`KeyProvider` reachability** (`implementation.md`, Outbound egress).

*Done when:* the **sealed-custody corpus** (owed — see below) passes:
same-plaintext-twice yields two different `SealedRef`s; a golden test asserts
the ref is not derivable from the payload bytes; every sealed artifact carries a
resolvable epoch tag; the backup assertion fires on a seeded violation; and a
`KeyProvider` that returns plaintext cannot be selected outside unit tests.

---

**F1 — Blind index.** Derivation, computation, constraint, and rotation.

- **`BIK = KDF(SMK, "blind-index", entity, field)`** — domain-separated per
  entity and field and **independent of the DEK epoch**
  (`implementation.md`, Key custody, `[settled — this replaces "blind-index
  keys derive per epoch"]`). The consequence is deliberate and must be built as
  stated: **a DEK rotation touches no blind index at all** — no re-salt, no
  re-index, no cross-epoch fan-out — which is what keeps `sealed indexed
  unique` from admitting duplicates during a rotation, and therefore keeps
  converge-or-create from creating a second row on a webhook redelivery.
- **HMAC computation is server-side, inside the host stage**, after the access
  check and behind key release (`implementation.md`, Observability — the span
  tree is `plan execute → host stages → key release → chain append`). The one
  exception is client-first mode, where blind-index lookups exist only if **the
  client computes and ships the index values** (`implementation.md`,
  Client-first) — a stated capability loss, not a second implementation.
- **The null sentinel**, and its disclosure. A nullable `sealed indexed unique`
  field HMACs a reserved sentinel so `HMAC(null)` is never undefined and
  uniqueness needs no special case — and the sentinel is therefore the modal
  value in that column, so **whoever holds the database learns which rows are
  null without any key**. The shape summary says so, computed rather than
  written.
- **SMK rotation is the index's rotation**, and it is a real re-index: build
  the new digest column **offline and swap it atomically**. An in-place
  re-index shows an observer with continuous database access the old and new
  digest for the same row — a linkage pair per row, which is the map the
  re-salt existed to destroy. If an online build is ever unavoidable, randomize
  row order and batch, and downgrade the claim in the shape summary to
  best-effort, because that is the honest claim.
  **"Offline" means a write barrier, and F1 builds it as one** (C4-015): writes
  to the entity's blind-indexed fields are quiesced from scan start through
  verified swap, rejected with a typed retryable error naming the rotation.
  "Old and new never both queryable" is a read invariant and does not exclude a
  write landing behind the scan cursor, which is how a logical duplicate
  survives the swap.

*Done when:* C084, C085 and C089 pass in the **execution** tier over the
`market` fixture; a `sealed indexed unique` field rejects a duplicate through
`UNIQUE (space, hmac)` and converges rather than creates on redelivery;
`cust_3`'s null email round-trips through the sentinel; a simulated DEK
rotation leaves every digest column **byte-identical**; an SMK rotation
completes via offline build and atomic swap with the old and new columns never
both queryable; and **a write attempted against a scanned-past row during that
rotation is rejected rather than accepted, with a duplicate-insert retry after
the swap still colliding**.

---

**F2 — Command log.** Finish `ordo-commandlog` against
`security-logging-design.md` §7 and §9's `(now)`/`later` split, and gate it.

- **Now, and already partly built**: actor/command/group types, canonical
  encoding (tagged values, big-endian u64 lengths, image fields in key order —
  no JSON, no serde), per-space `hash = SHA-256(prev ‖ canonical-encoding(group))`
  with a 32-zero-byte genesis, `Store` trait + in-memory store, signed-shape
  checkpoints behind `Signer`, `revert_group` planning.
- **Owed**: the **Postgres store**, archive bundling, checkpoint scheduling,
  consumer cursors — plus the `SealedRef` correction from F0, which is an
  encoding change and must precede any durable chain.
- **Attestation is a second, independent scheduler — do not hang it off the
  checkpoint timer** [settled, `security-logging-design.md`]. A checkpoint is
  the heavyweight pruning/verification anchor and `checkpoint 1mo` is right for
  it; an attestation is a few hundred bytes and one signature. Wiring them
  together leaves a month of unattested history, which is a long time for a
  rewrite to sit undetected. The spelling is
  `policy commandlog { checkpoint 1mo; attest every 1000 commands or 1h }` —
  **volume or time, whichever lands first** — and a space may request a tighter
  cadence than the deployment default. Two more facts the engine has to get
  right at build time rather than retrofit:
  - **Layer 1, client pinning, is always on and free, and needs no config.**
    Responses already carry a command-log position; they must also carry the
    **signed head** at that position, so every request is an attestation to that
    client and detection latency for anyone actually using the system is one
    request. Clients persist the last signed head and check continuity on next
    contact — trust-on-first-use with local detection.
  - **Signature cost is bounded by writes, not reads.** The head only moves on
    commit, so sign at most once per commit — in practice once per short
    batching window (`attest.signWindow`, default a few seconds) — cache that
    signature and attach the cached one to every response in between. This
    matters because the `Signer` may ride a cloud KMS where each signature is a
    billable call with real latency. One signature per write burst, never one
    per read.
  - The **attestation webhook** is layer 2 and is **opt-in per space**, with an
    owner-signed destination so the platform stores it but cannot silently
    redirect it. It is not the thing that makes history verifiable — layer 1 is
    — it is what makes verification survive a client that stops calling.
- **The append is a per-space serialization point**, taken as
  `pg_advisory_xact_lock` **inside** the appending transaction (never a
  session-scoped lock — under a transaction pooler a session lock outlives the
  work it protects). Parsing, planning, binding, access checks and statement
  execution all happen outside it. **Per-space write throughput is bounded by
  chain-append rate**, and `implementation.md` (Write path) calls the
  measurement **release-blocking**: it is F6's benchmark, and it is one of the
  three numbers that do not exist yet.
- **Epoch keys survive at least the undo horizon**: an epoch key may not be
  destroyed while any command image within the space's `undoHorizon` is tagged
  with it. Beyond the horizon, images whose key is gone render as an explicit
  **unresolvable** marker in `std.E.history` and refuse `revert_group` with a
  typed rejection — never a silent empty value.
- **A restore is a fact in the chain, not a silent rewind**: a `system:restore`
  group is appended as the first group after the restored head, so
  `verify(space)` reports "one restore discontinuity at position N" rather than
  "tampered at N" (`implementation.md`, Backup).
- **Genesis import mode** (`migration.md` §3) is a command-log shape, not a
  bypass: one group per batch, summarized images, a content hash over the
  batch's post-seal payloads, and a genesis checkpoint that closes the import.

*Done when:* the **command-log corpus** (owed) passes: `verify(space)` names
the first tampered index; a checkpoint lets verification start mid-chain; group
shape violations are typed errors at append; `revert_group` inverts
create→delete, converge→converge-with-before-image, delete→create and applies
**through the normal gate** as new history; a restore produces exactly one
discontinuity marker; **the pruning floor never passes the slowest registered
consumer, where a stalled mirror is evictable from that set after
`consumerStall` and a listener cursor or archive sink never is** (V-002); and
the sealed-redaction test asserts the ref is not derivable from the payload
bytes.

---

**F3 — Security-event stream — BUILT** (`ordo-secevent`, plus
`ordo_exec::secevent::PgSecurityStore` for the durable half; checklist 7a/7b).
The same machinery, **separately instantiated**
(`security-logging-design.md` §6.1, §9): its own head, its own checkpoints,
its own retention, its own lock and its own append path, sharing
`ordo-commandlog`'s chain arithmetic, `Signer` and actor taxonomy rather than
forking them — the body layout is its own, domain-tagged, so the two
encodings are disjoint and neither stream's rows can be grafted into the
other's table.

Still owed after 7a/7b: the read surfaces. A space owner reading their
stream, a single `verify()` command covering both artifacts, the
dashboard's explorer and burst view (`dashboard-plan.md` §3), and the job
that drains the recorder on a tick are all reads or wiring over what now
exists — the record is built, the window onto it is not.

The four rules that are easy to build wrong and are each fatal alone:

- **Members only.** A denial appends to a space's stream only when the denied
  principal has a relationship with that space. A non-member, an anonymous
  caller, or a token naming a space it has no relationship with is a
  **deployment** event. Without this, any stranger grows a victim space's audit
  spine at will.
- **No interaction with the command chain's lock**, and appends run **off the
  request path** through the job queue. A denial must never contend with a
  legitimate writer, and a stream append failure is an operational alert, never
  a failed request.
- **The decision is recorded, not the wire code.**
  `not_found`-because-guard is a distinct recorded decision from
  `not_found`-because-absent. This is the half P5 supplies and the reason the
  two surfaces exist.
- **Collapse key `(space, principal, op)`**, one entry emitted at window close
  with the count inside the hashed body, and a per-space per-window cap that
  overflows to a single suppression marker. Nothing is mutated after hashing;
  the open window is ordinary mutable state.

*Done when:* a non-member denial never touches any space's stream; a denial
never acquires the command chain's lock (asserted, not reviewed);
`verify()` verifies both streams independently; retention drops whole prefixes
ending at a checkpoint and verification resumes from it; and the recorded
decision differs from the wire code in the cases where §4.1 makes the wire
uniform.

---

**F4 — The door and the wire.** `ordo-identity` + `ordo-api`, both partly
built, neither gated on anything today.

- **One access check at the door** (`security-logging-design.md` §6): token →
  principal → (membership role + group memberships → per-space access profile)
  **or** capability allowlist → is the op included? **Group resolution is
  union-then-overrides**, per space, non-transitive, with the higher of a direct
  and a group-conferred role winning and deny-by-override beating allow-by-group.
- **The space is bound before the check and carried into the executor as its
  own argument**, which the executor asserts. A re-derived space is the
  cross-space read the check exists to prevent (`api-engine-design.md` §12).
- **Non-enumerability** (`api-engine-design.md` §4.1): an op the principal may
  not call and an op that does not exist produce the identical door answer.
  `access_denied` is the door's word; `access_guard` is the executor's; they are
  never interchanged.
- **The refinement envelope and its cost gate** (§7, §7.1):
  `effective = registered ⊔ class_of(refinement)`, sealed refinement raising to
  `blind-index` or host-side, `refinement_unbounded` when the effective class is
  unbounded and the op carried no acknowledgment. The declaration-time half
  lives in the shape (`sealed indexed refinable`) and in std's derivation
  (`std-library.md` §2.1); **implementing one half leaves the hole open in one
  direction**, so both halves are F4's done-when.
- **Idempotency replay in Postgres**, sealed at rest under the space key,
  committed in the same transaction as the group. The in-memory store is a test
  double and a multi-node deployment on it is a correctness defect.

*Done when:* the error-parity gate is green against
`crates/ordo-api/src/error.rs`; the oracle-parity assertions of
`api-engine-design.md` §4.1 hold (absent and forbidden answer the same code
with comparable cost); **cross-space denial tests exist** — §12 names them as
the coverage obligation nothing currently meets; and `std.space.callersOf`
enumerates principals reached via group with the group named as the *via*.

---

**F5 — Executor and write path.** `ordo-exec`. **Status: partially built.**
The pooled `tokio-postgres` connection layer, the transaction seam, the read
executor with the venue gate at the plan, keyset pagination, and the
single-transaction write path (row locks before guard evaluation, chain
append inside the same transaction) all execute. Still owed: triggers and
invalidations on commit, LISTEN/NOTIFY, the job queue, clock leadership,
host stages with key release, and the replay-finalization leg described
below. Run P4/P5's artifacts:
prepared-plan execution, host stages with key release, transaction boundaries
per command group, **isolation level per operation class**, chain append inside
the write transaction, triggers and invalidations firing **only on commit**,
the LISTEN/NOTIFY invalidation bus, the `FOR UPDATE SKIP LOCKED` job queue, and
clock triggers via a leadership claim **plus** the schedule row's own claim
(the lock stops stampedes; the claimed row committing in the firing transaction
is what delivers *fires once*).

The one write recipe worth naming here because it is the sealed substrate
leaking into the write path: **converge-or-create whose converge RHS reads a
sealed field cannot be one statement** — `INSERT … ON CONFLICT DO NOTHING`,
then `SELECT … FOR UPDATE` on the unique key, decrypt, `UPDATE`, all inside the
group's transaction (`implementation.md`, Isolation level per operation class).

**Two of the three legs now commit together** (this passage previously said
the interfaces could not express it; the seam was replaced rather than
hidden behind — C4-004, C4-005, SH4-002). The requirement is that the domain
write, the chain append and the replay finalization commit together.

**Done (AA-004 closed).** The write path talks to a `WriteDb` seam rather
than a connection it owns, and the executor supplies an open transaction as
that seam: `SELECT … FOR UPDATE` locks the addressed row *before* its guards
are evaluated, every statement of the op shares one snapshot, and a refusal
rolls the whole group back — leaving no partial effect and appending no
command group. The chain append rides that same transaction.

**Still owed.** `ReplayCache::put` remains a separate post-return call
(`ordo-api/src/idempotency.rs`, driven from `router.rs` *after* `execute`
returns). A process that dies between the two leaves a claimed key with a
committed effect and no recorded outcome; moving `put` earlier only inverts
the failure. Closing it means threading the replay lease into the same
transactional unit — `execute_claimed(…, ReplayLease)` or a
transaction-context trait — and removing the post-return call rather than
leaving it as a second path.

The same unit of work carries a **fencing generation**. A replay lease that
ages out and is taken over by a successor must render the original holder's
writes unacceptable — today a stale holder that wakes up can still finalize and
destroy its successor's outcome, which is the takeover half of the same defect.

*Done when:* GC001–GC087's execution tier runs **through the executor** under a
bound principal and space, asserting row sets rather than SQL; a retry
re-executes the whole group from scratch with the idempotency record and the
chain append committing exactly once; **a kill injected between any two of
domain write / chain append / replay finalize leaves no committed effect
without its recorded outcome, and a fenced stale holder's finalize is
rejected**; a trigger never fires on a rolled-back group; and the executor
rejects a plan whose space disagrees with the door's with `internal` (500)
rather than proceeding.

---

**F6 — Deploy, catalog, operate.** `ordo-catalog` plus the operational
surface that turns the binary into a deployment.

- **`ordo apply`, the artifact registry in Postgres, and hot swap**: new
  versions written, nodes nudged over LISTEN/NOTIFY, each swapping atomically
  with in-flight requests finishing on their version; **rollback is re-pointing
  the registry at the prior version hash**, restart-free. `ordo dev` is the
  same path with destructive migrations auto-confirmed.
- **Runtime shape verification** (`implementation.md`): each op's planned SQL
  runs once against an **empty shadow schema**; Postgres reports the exact
  result columns and types; the compiler's inferred shape must match exactly or
  the load fails loudly. Host-side stages verify the SQL portion this way and
  the host portion against the typed IR.
- **The capability catalog**: verified shapes joined with registration metadata,
  rendered twice — machine JSON for SDK generators and an LLM-ready document at
  a well-known endpoint — **access-checked like everything else**, so an
  agent's catalog is its consent page in machine form.
- **Archive, pruning, checkpoint scheduling, and the *separate* attestation
  cadence** (`attest every N commands or T`, never derived from `checkpoint`);
  the three-artifact
  observability split (command chain / security-event stream / deployment
  security log, one correlation id joining them) and the **mandatory** deny-
  volume-per-principal-per-space alert; outbound egress under §7b's six pins.
- **The three release-blocking measurements that do not exist**: single-space
  write throughput under the chain-append lock, p99 op latency, and restore wall
  time. `implementation.md` calls them release-blocking; nothing scheduled them
  until now.

*Done when:* a hot swap changes no in-flight request's version and a rollback is
a registry re-point; every registered op's inferred shape matches its observed
shape or the load fails; the catalog served to a token lists exactly the ops
that token may call; a quarterly restore drill runs `verify(space)` over sampled
spaces and finds exactly one discontinuity marker; and the three numbers are
published rather than promised.

---

**F7 — Client-first: the vault protocol [new].** The custody dial's top stop
ships in v1, and until now it had **no deliverable in this plan at all** —
`implementation.md` specified it and nothing here built it (V-008, C4-001,
C4-002, C4-003). It is F-track because it is substrate, not language: the
compiled artifacts, the ops and the manifests are identical to every other
stop; only the execution venue and the trust boundary move.

- **The two signed records** (`implementation.md`, merge rule part 1). The
  device-signed proposal `P = (payload_digest, device_id, counter,
  believed_prev, artifact_hash, schema_hash)` and the server-signed receipt
  `R = (hash(P), canonical_position, canonical_prev)`. Both immutable; `P` is
  signed once at authorship and never re-signed. The chain commits to both, and
  `verify(space)` checks each link's receipt against its proposal. The server's
  sequencing path is **content-blind by construction**, not by discipline: it
  is handed a header and a ciphertext blob and has no code path that opens one.
- **Artifact-pinned folding.** A receiver fetches and verifies the retained
  artifact `P` names before folding, and folds under that one. Sequencing
  refuses a proposal naming a non-retained artifact — a transport fact, so a
  blind server can enforce it — and artifact retirement is a **deprecation
  window announced to devices**, never a silent swap, or an offline device's
  proposals start bouncing with no warning it could have received.
- **The deterministic evaluator.** The same compiled artifact, running in WASM
  on the device (the embedded-mode engine from `tooling-and-embedding-plan.md`
  §B4). Every invariant class — `rev`, guards, `check`s, uniqueness, referential
  integrity, bounds — resolves to a **canonical no-op with a recorded outcome**
  when it fails, identically on every key-holding device. This is the same
  evaluator P4/P5 emit for the server; a second implementation would be a second
  source of divergence.
- **Canonical-derived inverses.** `revert_group` in a client-first space
  recomputes the before-image from the canonical prefix and treats the
  proposal-supplied image as a *claim*. A mismatch is signed evidence of a false
  image and is surfaced as such, not silently preferred either way.
- **Snapshots and compaction** (`implementation.md`, "Snapshots and
  compaction"): snapshot production on the `policy vault` cadence, sealed and
  chained; verification of a snapshot against the chain head it claims;
  cold-start selection of the newest openable snapshot; and the discard floor at
  the newest snapshot every enrolled device has acknowledged, with an enrolled
  device non-evictable and un-enrolment as the remedy.

*Done when:* a device that has been offline across an artifact change folds the
canonical order to **byte-identical state** with a device that was online
throughout; a proposal carrying a **false before-image** is sequenced, applies
nothing if it violates an invariant, and — where it is valid but false — reverts
to the true canonical predecessor with the mismatch reported; a device killed
between upload and acknowledgment leaves **no** sequenced link lacking a valid
device signature; a cold start over a space with 100k commands and a snapshot
every 5k reads **one snapshot plus at most 5k segments**, measured, not
asserted; and discarding runs only below the acknowledged floor, with the
dashboard naming the device that holds it.

### Joint milestones

Phases say what gets built. Milestones say when the two tracks have to meet,
and each one ends in something that runs end to end — the discipline the
layered plan was missing.

**M1 — Walking skeleton.** One domain, one entity carrying one open field and
one sealed field, one guarded read op, one write op. The path runs whole:
request → door binds the space and checks access (F4) → prepared plan executes
(F5) → host stage releases the key and opens the sealed value (F0) →
blind-index equality resolves in SQL (F1) → the write's command group appends
and `verify(space)` passes (F2). Compiler-side it needs only P1–P5 for that one
shape; the point is not coverage but **seams**. Every integration mistake in
this design is at a seam — the space that gets re-derived, the ref that turns
out to be a hash, the guard that inlines but does not disclose — and a
milestone that crosses all of them once, early, is worth more than any of the
phases individually.

**M2 — Authorization gate.** GC001–GC087 in the execution tier through the
executor, with per-principal row-set assertions, negative twins, oracle parity
and the no-guards canary. Needs P1–P6 and F0/F1/F4/F5. This is DoD criterion 4
and it is sequenced **before** feature completeness, not after.

**M3 — Engine v1.** The full definition of done below.

### Order of attack inside P1 (starting now)
1. Workspace + crate scaffolding, CI-less `cargo test` green.
2. Lexer with full token inventory + literal parsing + tests.
3. AST types for expressions/conditions/pipeline/entity.
4. Expression + condition parser (closed grammar, paren-mixing rule) + tests.
5. Entity declaration parser + tests — **including `access` clauses and `acl`
   declarations**, in the same step as fields and refs, because they are part
   of a shape rather than an extension to one.
6. Query pipeline parser + tests.
7. Registration wrapper flags (`unguarded` / `unbounded` / `reproducible`) and
   `$caller` resolution + tests.

### Order of attack inside Track F (in parallel, starting now)
1. **The `SealedRef` correction first.** `ordo-commandlog` holds a content
   hash where §7 pins a random handle, in a frozen encoding (G16.2). It is a
   small change today and a chain migration the moment anything durable exists.
2. `ordo-crypto` scaffold: `KeyProvider` trait + local-master-key
   implementation, SMK, DEK epoch series, epoch tag in the wire format.
3. AEAD seal/open with fresh-nonce-per-write; the no-dedup invariant asserted
   in the payload store's own tests, not in prose.
4. `BIK` derivation and HMAC computation; the null sentinel; the
   `UNIQUE (space, hmac)` constraint shape agreed with P2's DDL golden.
5. The **sealed-custody corpus** and the **command-log corpus** as documents
   (below) — before the code that would otherwise define its own bar.
6. `ordo-commandlog`'s Postgres store and transaction-scoped append lock.
7. M1's thin slice, wired end to end.

### Owed corpora [new]

The definition of done leans on corpora everywhere it can. For all five pillars
G16 names, **there is no corpus to lean on** — two new documents would cover
four of them, and the fifth (runtime shape verification) is covered by
criterion 9 instead, because its assertion is structural rather than
case-by-case. This is work, and it is listed here so it is scheduled rather
than assumed:

| Corpus | Covers | Status |
|---|---|---|
| `guards-conformance.md` GC001–GC087 | authorization, row-level | exists |
| `composition-conformance.md` C001–C118 | composition, cost classes, and — §K, C084–C091 — the **compiler's classification** of sealed access | exists |
| `sql-translation-corpus.md` / `-2.md` | expressibility gauntlet, ~200 cases | exists |
| **sealed-custody corpus** | `BIK` domain separation, HMAC and the null sentinel, `UNIQUE (space, hmac)`, epoch tagging, DEK rotation leaving indexes untouched, SMK rotation via offline swap, `SealedRef` non-derivability, crypto-erasure, key release derived from op access | **owed — F0/F1** |
| **command-log corpus** | canonical encoding, chain recomputation, `verify(space)` naming the first tampered index, checkpoint sign/verify, `revert_group` inversion, group atomicity and single-space, archive/prune floor, restore discontinuity, genesis-import batch hashing, and the security-event stream's members-only and collapse rules | **partly owed — F2**; the stream's half (members-only routing, collapse, the cap, separate keys, prefix pruning, and a domain revert leaving it untouched) landed with 7a |

The existing sealed coverage is worth being precise about, because it is easy
to mistake for more than it is: C084–C091 and GC074/GC075 test **what the
compiler decides about sealed data** — the cost class, the manifest, the
`guard_on_sealed` rejection. Not one of them tests a key, a digest, or a
ciphertext. The crypto has not been under test.

### Definition of done (engine v1) [rewritten]

The previous definition — *"`ordo check` accepts every legal construct in the
spec's worked examples and `syntax-examples.md`"* — could be met in full by an
engine with **no authorization system whatsoever**, because
`syntax-examples.md` contains zero authorization examples. An engine could
ship, conformant to its own stated bar, and return everyone's rows to everyone.
That bar is replaced.

**And it is replaced a second time** [G16]. The six criteria below were written
against Track P alone. Criterion 3 already required an execution tier the
sealed substrate makes possible, so it was not satisfiable as written — and
criteria 7–9 name the parts that had no criterion at all. The pattern is the
same one G15 found and is worth stating once more: **a definition of done that
covers only what the plan mentions inherits the plan's blind spots.**

Engine v1 is done when **all nine** hold. Any one failing means not done; there
is no partial credit on 3, 4 or 7.

1. **Language acceptance.** `ordo check` accepts every legal construct in the
   spec's worked examples, `syntax-examples.md`, and the guarded examples the
   onboarding docs gain, and rejects every §-listed illegal form with a
   spanned, specific error naming the rule.
2. **Determinism of output.** Golden-stable SQL and artifact metadata (mode
   label, cost class, scope manifest, **effective guard set**) for every
   corpus case.
3. **Composition conformance.** C001–C118 pass in the **execution** tier
   against seeded Postgres.
4. **Authorization conformance — the gate.** `guards-conformance.md`
   GC001–GC087 pass in the **execution** tier, with per-principal row-set
   assertions, the negative twins, the oracle-parity assertions, and the
   no-guards canary failing when enforcement is disabled. **v1 does not ship
   without this**, and it is the criterion most likely to slip, so it is
   sequenced early rather than last.
5. **Guards are inspectable.** `ordo check` can print, per entity, the
   effective read and write guard and where each came from (own clause /
   inherited from which acl root / none) — and every op's artifact carries its
   inlined guard set and its `unguarded` flag. "It has guards" must be
   verifiable by reading output, not by trusting the build.
6. **Bypass is greppable and complete.** Every `unguarded` op is listed in the
   artifact output; every op composing one carries its own acknowledgment or
   fails to compile; and no code path inlines guards *conditionally*.
7. **Custody conformance — the second gate.** The **sealed-custody corpus**
   passes against a real `KeyProvider`: `SealedRef`s are unique per write and
   not derivable from the payload; every sealed artifact carries an epoch tag;
   a DEK rotation leaves every blind index byte-identical; a `sealed indexed
   unique` field converges rather than duplicates across a rotation; and the
   null sentinel round-trips with its disclosure surfaced on the shape summary.
   **The crypto is either under test or it is a claim** — and this design's
   central promises (per-space sealing, crypto-erasure, equality-without-keys)
   are all claims about the crypto.
8. **The record survives its own audit.** The **command-log corpus** passes:
   `verify(space)` recomputes the chain and names the first tampered index; a
   restore produces exactly one discontinuity marker and no other break; a
   group shape violation is a typed error at append; `revert_group` produces new
   history rather than rewriting old; denials appear in the security-event
   stream and **never** in the command chain, never for a non-member, and never
   under the chain's append lock.
9. **The system runs, and says what it is.** M1's end-to-end path executes
   through the door, the executor and the log against real Postgres; every
   registered op's inferred shape matches its Postgres-observed shape or the
   load fails; and the capability catalog served to a token lists exactly the
   ops that token may call. An engine whose output nothing executes has not been
   shown to be an engine.

**Two of the nine are measurements, not features, and are called out because
they are the ones that get deferred**: `implementation.md` names single-space
write throughput under the chain-append lock, p99 op latency, and restore wall
time as **release-blocking**, and `README.md` states plainly that no
performance numbers and no executed restore drill exist. They are F6's, and
"release-blocking" is a claim this plan now has to schedule rather than repeat.

Four things explicitly **not** in engine v1's scope, so they are not mistaken
for it: the fold of the guard rules from `guards-conformance.md` §4 into the
spec body (spec work); the runtime access check at the door, including the
space binding and the space-scoped capability path (Track F, `F4`); the
per-space **security-event stream** that records denials (Track F, `F3` —
the engine supplies the decision, the stream stores it); and the `std.space.*`
group administration ops (`std-library.md` §3.1), as against **group resolution
inside guards**, which is P2/P3 engine work and is tested by criterion 4. The
engine's job is the **row-level** half — inlining, resolution, preconditions,
disclosure — and that half is what criterion 4 tests. Those four are out of
*Track P's* scope; three of them are now **in this document**, in Track F,
because "not the engine's job" was being read as "not anybody's job" (G16).

---

## Part 3 — How this restructure was chosen

Recorded because a plan's shape is a decision, and the last time it was made
implicitly it produced G15 and then G16.

### Prior art consulted

Six sources. The first four are projects that are a compiler *and* a storage
platform and that documented what their sequencing cost; they do not agree with
each other, which is the useful part — what they agree on is narrower and
sharper than "layers versus slices". The last two are a security platform and a
language project, chosen for the two failure modes this plan is trying to avoid.

- **CockroachDB built storage, Raft and the KV layer for roughly eighteen
  months before serious SQL work began**, and shipped. Its published regrets
  are not about that order. The costliest redo in the SIGMOD 2020 paper's
  "Lessons Learned" is §7.4: early versions replicated *un-evaluated* requests
  and evaluated them on each peer, so any change to evaluation risked replica
  divergence across a mixed-version cluster; the fix was to **move evaluation
  ahead of replication** — a change to a layer *boundary*, not to a layer's
  contents. **The failure mode of a long horizontal build-out is that you
  cannot tell you drew a boundary wrong until the whole stack runs under real
  conditions**, and for them "real conditions" meant a rolling upgrade, years
  in. That is the argument for M1 and it is why M1 crosses every seam rather
  than covering many features.
  <https://www.cockroachlabs.com/pdf/cockroachdb-the-resilient-geo-distributed-sql-database-sigmod-2020.pdf>
- **SQLite draws its stability line at the durable format and says so in
  writing.** The on-disk B-tree/pager format has been backwards compatible
  since 3.0.0 in 2004 and is committed through 2050
  (<https://www.sqlite.org/lts.html>, <https://www.sqlite.org/formatchng.html>),
  while the layer immediately above it is declared permanently renegotiable:
  *"The VDBE opcodes are not part of the interface definition for SQLite. The
  number of opcodes and their names and meanings change from one release of
  SQLite to the next."* (<https://www.sqlite.org/opcode.html>). Its documented
  regrets are all cases where **semantics leaked into the frozen contract
  before anyone noticed** — NULLs permitted in a PRIMARY KEY is *"a bug"* they
  must now support forever (<https://www.sqlite.org/quirks.html>).
  **Ordo has exactly one durable format and it is not SQL**: the command log's
  canonical byte encoding and the sealed envelope's epoch tag. That is the
  direct source of F0's ordering rule — the `SealedRef` correction is a change
  to the frozen layer, so it lands before anything durable exists, and the
  epoch tag ships before first customer data even though rotation does not.
- **DuckDB is the purest vertical slice in the set**: parser → binder →
  logical plan → physical plan → vectorized execution, runnable end to end
  **eight days after the first commit**; durable storage arrived five months
  later and deliberately incomplete. They then carried a published policy —
  *"The DuckDB internal storage format is currently in flux, and is expected to
  change with each release until we reach v1.0.0"* — for five and a half years,
  and the v1.0 announcement names format compatibility as *"one of the major
  blockers to releasing 1.0.0"*, paid for in per-release manual user migrations
  (<https://duckdb.org/2024/06/03/announcing-duckdb-100>). **The deferral cost
  was anticipated and stated rather than discovered**, which is the half most
  projects skip. `README.md`'s "Current status" is Ordo's version of that
  declaration; this plan's job is to make it converge.
- **TiDB shipped the SQL layer first, on borrowed storage — and users rejected
  it.** PingCAP's own account: *"In September 2015, we released TiDB to the
  open source community. It didn't have a storage layer and ran on top of
  HBase"*, and prospects refused a prototype that required Hadoop, Zookeeper,
  HDFS and HBase to run. TiKV began four months later and the driver landed in
  TiDB seven months after that
  (<https://pingcap.medium.com/five-principles-that-guide-tidb-and-pingcap-part-i-71bcb1f84c0>).
  **A vertical slice whose bottom layer is a stand-in proves less than it
  appears to.** That is why P6's execution tier and M1 both forbid a mock key
  provider: a slice that fakes the substrate tests the slice, not the system.

**What those four agree on**, and the one sentence this restructure is built
around: *the layer whose churn cost is unbounded is the durable format, and
every project either froze it early or paid publicly for not doing so.* Ordo's
durable format is the command-log encoding and the sealed envelope — F0 and F2
— which is precisely the pair G16 found had no build home.

Two more, on the security substrate and on why plans like this one fail:

- **Vault states the foundation-first rule more plainly than any design doc
  here does.** *"When a Vault server is started, it begins in a sealed state.
  Before any operation can be performed on Vault, it must be unsealed"*, and
  *"Prior to unsealing, the only possible Vault operations are to unseal the
  Vault and check the status of the server"*
  (<https://developer.hashicorp.com/vault/docs/concepts/seal>). The barrier
  shipped in Vault 0.1 — in the same release that said *"we can't recommend
  production usage"*. **A security boundary is the one thing that cannot be
  vertically sliced**: a half-built barrier is not a partial barrier, it is a
  broken one. That is why F0 is F0 and not F3.
  What Vault *did* defer, for about three and a half years, is the
  **ergonomics** of that substrate: Shamir unseal at 0.1, HSM auto-unseal in
  Enterprise, cloud-KMS auto-unseal in open source only at Vault 1.0 in
  December 2018
  (<https://www.hashicorp.com/en/blog/enabling-cloud-based-auto-unseal-in-vault-open-source>).
  F0 copies that split exactly — the `KeyProvider` trait and a local master key
  now, KMS and Vault behind the same trait later — and it is the reason the
  trait is required "from day one" while its implementations are not.
- **Rust's Project Goals process (RFC 3614) exists because per-team roadmaps
  produced accepted designs nobody shipped.** The RFC's own diagnosis of the
  old process is that it *"lacked mechanisms for sourcing goals from teams"*,
  had no owner assignment and no progress tracking; the replacement is an
  explicitly cross-team unit of work that is *"a **contract** between the owner
  and project teams"*
  (<https://rust-lang.github.io/rfcs/3614-project-goals.html>). Rust's RFC
  README says the quiet part directly: *"the fact that a given RFC has been
  accepted and is 'active' implies nothing about what priority is assigned to
  its implementation, nor does it imply anything about whether a Rust developer
  has been assigned the task of implementing the feature."*
  **That is this project's failure verbatim.** `security-logging-design.md` §9
  marks `ordo-commandlog` and `ordo-identity` **"(now)"** with their contents
  enumerated — a settled design, with no owner, no sequence, and no gate. The
  result was code built from a superseded reading. Rust's fix and this
  restructure's fix are the same one: give the cross-cutting work a named phase
  with a done-when, and keep the per-layer view as a *capacity* plan rather than
  a delivery plan. Tracks P and F are the capacity view; the dependency map and
  M1–M3 are the delivery view.

**The methodology term for M1 is Alistair Cockburn's "walking skeleton"**: a
tiny implementation that performs a small end-to-end function and **links
together the main architectural components**, so that architecture and
functionality evolve together and boundary problems surface while little has
been invested in them
(<https://www.oreilly.com/library/view/97-things-every/9780596800611/ch60.html>;
Cockburn's own page for it is dead, so the book chapter is the citable text).
It is the named remedy for the CockroachDB §7.4 failure mode, and Fowler states
the risk it addresses in general form: building one side last means *"the
back-end code may be designed in a way that doesn't work with the UI once it's
built"*
(<https://martinfowler.com/bliki/KeystoneInterface.html>).

**The honest cost of M1, since prior art supplies one.** Gojko Adzic's critique
is that linking all the main architectural components up front **delays
time-to-first-value**, and that a thinner slice with temporary crutches often
beats a complete skeleton
(<https://gojko.net/2014/06/09/forget-the-walking-skeleton-put-it-on-crutches/>).
The LeSS literature names the symmetric cost on the other side: layer-shaped
teams produce *"sequential development … many queues … many handoffs"*, while
feature-shaped work is *"seemingly difficult to implement"* and demands
continuous integration and cross-specialty competence
(<https://less.works/less/structure/feature-teams>). M1 is deliberately placed
where the crutches argument does **not** apply: its components are not
scaffolding to be replaced but the seams — the space binding, the ref shape,
the guard disclosure — where crutches would hide exactly the defects it exists
to find.

### Options considered

**(a) Add the missing phases to the end of the layered plan** — a P8 for the
sealed substrate and a P9 for the command log.
*Buys:* the smallest possible edit, and no disturbance to anything that cites
P0–P7.
*Costs:* nothing up front.
*Risks:* it is refuted by the plan's own definition of done. Criterion 3
requires C001–C118 in the execution tier, and C084/C088 execute against sealed
and sealed-indexed fields — so a P8 after P6 is a prerequisite scheduled after
the thing it is prerequisite to. It also repeats G15's exact error at a larger
scale: scheduling the silent-failure subsystem last, where slipping it is
invisible until it is expensive. **Rejected on the dependency, not on taste.**

**(b) Restructure into pure vertical slices**, each slice one narrow feature
carried from source text to executed row, replacing the phase list entirely.
*Buys:* something runnable at every boundary, and integration risk paid down
continuously instead of at the end.
*Costs:* the definition of done is corpus-shaped, and the corpora are organized
by feature area — C001–C118 by composition concern, GC001–GC087 by
authorization dimension, ~200 SQL-translation cases by query family. Slicing
the plan without re-cutting the corpora leaves every slice's done-when
referring to a fraction of a document; re-cutting them is a large edit to
settled, reviewed assets that are normative in places the spec is not.
*Risks:* rework, and the loss of the one thing this project has that most
pre-alpha projects do not — an executable definition of done that was written
before the code. **Rejected for the corpora**, not for the idea; M1 keeps the
part of it that was load-bearing.

**(c) Split into two documents — a compiler plan and a platform plan — with a
dependency map between them.**
*Buys:* clean ownership; each document stays the size one person holds in
their head.
*Costs:* a boundary.
*Risks:* the boundary is precisely what produced this failure. This document
already said the security-event stream was "platform work rather than engine
work" — deferring it to a platform plan **that does not exist** — and the
result was five settled pillars with no owner. A second document also splits
the coverage gate: `plan-coverage` reads a single `plan_file`, so the pillars
would have to be partitioned between two lists by hand, and a pillar that
migrated across the boundary would go unwatched in both. **Rejected because it
re-creates the failure mode under better lighting.**

**(d) Two tracks in one document, with a dependency map and joint
milestones** — adopted.
*Buys:* every pillar has a phase; the cross-track edges that were being got
wrong are written down rather than assumed; P0–P7 keep their names, so
`security-logging-design.md` §6, `eval-harness-plan.md`, `adoption-dx-plan.md`
and `README.md` keep working; the corpora stay as they are and become
per-phase done-whens; and the coverage gate keeps reading one file.
*Costs:* one long document, and a plan that is honest about being larger than
"build a compiler".
*Risks:* two tracks in one document can still be read as "do Track P, then
Track F". The dependency map and M1 exist to make that reading fail fast — if
Track F has not started by the time P4 is labelling ops `blind-index`, the plan
is being read wrong.

### Why this was implemented rather than presented as an open choice

The owner's standing instruction is to converge when one answer is clearly
right and to present options when it is genuinely a judgment call. Two facts
moved this out of judgment-call territory:

1. **The dependency is checkable.** Track P's criterion 3 cannot pass without
   F0 and F1. That is not a preference about plan shape; it is a property of
   the fixture in `composition-conformance.md` §2.1, and it eliminates (a) and
   any variant that sequences the substrate after the engine.
2. **The failure already happened once, in code.** `SealedRef` was built as a
   content hash against a `[pinned]` ruling that says the opposite, inside a
   canonical encoding both the ruling and the crate treat as frozen. A subsystem with no build home does
   not wait politely to be scheduled — it gets built from whichever reading was
   nearest. That eliminates (c), which would leave the next such subsystem in
   the same position.

What remains genuinely open is not the structure but three sequencing calls
inside it, listed under "Owner decisions" below.

### Owner decisions still open

- **How much of Track F runs before P6.** The dependency map says F0 and F1
  must; it does not say whether F2–F5 should be built alongside P2–P4 (earlier
  integration, more context-switching) or batched after P5. M1 is written to
  need only a thin slice of each, which makes the earlier reading cheap, but it
  is a resourcing call.
- **Whether the two owed corpora are written by this project or by review.**
  Every existing corpus was authored as a review artifact before the code. The
  sealed-custody and command-log corpora have no author assigned, and they are
  criterion 7 and 8's entire content.
- **Whether `ordo-crypto` is one crate or two.** `security-logging-design.md`
  §9 leaves the analogous question open for the security-event stream and calls
  it a packaging choice; the same answer probably applies to splitting key
  custody from sealing, but it is not this document's ruling to make.

### Owed elsewhere — found while writing this, owned by other documents

Listed rather than fixed, because each lives in a document this one does not
own. Every item is a place where a reader would build the wrong thing.

- **`glossary.md`, "DEK epoch / rotation" — repaired (C4-016), pin retained.**
  The entry briefly stated both sides of the ruling: its main text had already
  been corrected to SMK-derived, DEK-independent blind-index keys, while a
  trailing "open gaps" paragraph still said they derive per epoch. The stale
  paragraph is gone; what remains there is the genuinely open **master-key**
  rotation window (C4-015), which is a different problem. Restating the pin
  here anyway, because this document is where the engine reads it: the engine derives
  `BIK = KDF(SMK, "blind-index", entity, field)` — domain-separated per entity
  and field, **independent of the DEK epoch**. A DEK rotation touches no blind
  index: no re-salting, no re-index, no cross-epoch fan-out. F1 builds it that
  way regardless of what the glossary currently says.
- **`implementation.md` (Compiler) still says "Parser: chumsky or pest (final
  pick at build time)".** The pick is hand-written recursive descent, made in
  this document's "Shape of the system" and already implemented in
  `crates/ordo-syntax`.
- **`api-engine-design.md` §12's trait table has no `KeyProvider`.** It lists
  `OpRegistry`, `OpExecutor`, `AccessChecker`, `InvalidationBus`,
  `ReplayCache`; key custody is implicit inside `OpExecutor`. Since key release
  is derived from op access and every release is logged, the boundary deserves
  to be named where the other five are.
- **`crates/ordo-commandlog` implements `SealedRef` as a content hash**
  (G16.2). Code, not docs — F0's first task, and listed again here because it is
  the one item on this list that is currently shipping behaviour rather than
  prose.
