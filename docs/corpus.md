# The conformance corpus

The corpus is how this project answers *"does it actually work?"* without asking
anyone to take the answer on trust. It is a set of cases **transcribed from the
specification documents**, executed against a **real PostgreSQL server**, with
their **rows asserted**. A case is checked past compiling and planning.

If you are evaluating Ordo and only read one thing about its assurance, read
this. Everything else in `docs/` describes what the system is supposed to do;
this describes the mechanism that checks whether it does.

## The one rule

**The document is the authority, and the test transcribes it.**

Expected rows and error classes come from the specification. Nothing in the
corpus asserts what the compiler happens to produce. A test written from the
implementation passes forever and proves nothing, because it agrees with
whatever the code does, including whatever the code does wrong.

Two consequences follow:

- A failing case may or may not be a bug in the code. It is a
  disagreement, and either side can be the one that is wrong. Several have been
  settled by correcting the *document*.
- A case may **never be promoted by weakening its assertion**. A case that
  passes because it stopped checking something is worse than one honestly
  recorded as unresolved, because it looks like coverage.

## Where the cases come from

| Suite | Source document | What it covers |
|---|---|---|
| `corpus.rs` | `composition-conformance.md` | The executing composition corpus: named queries, pipelines, refusals |
| `corpus_c_extra.rs` | `composition-conformance.md` | Cases the executed set was missing |
| `corpus_p0.rs` | Stage 1, phase P0 | The corpus harvest. Row order is part of the contract and is asserted |
| `corpus_stage2.rs` | Stage 2 | Do the hand-written queries satisfy the frozen wants? |
| `corpus_stage3.rs` | Stage 3 | The wants expressed in Ordo, judged by the compiler and the seeded database |
| `corpus_stage5.rs` | Stage 5 | The write matrix, executed |
| `guards_corpus.rs` | `guards-conformance.md` §5 | Authorization: every `GC` case, run as real principals |

Cases are keyed by **the id the document gives them, spelled literally**, such as
`C042` or `GC083`. The audit that produced this discipline was
`rg GC0`, and a suite organised by theme cannot survive such an audit. If you cannot grep for a case id and find its assertion, the coverage
claim is unverifiable.

## The tiers

A case can be checked at three depths, and the depth is part of what the case
proves.

1. **Compile**: the source parses, binds, and passes the registration gates.
   Enough for a case about a *refusal*: if the program must not deploy, nothing
   further needs to run.
2. **Plan**: it emits SQL, and the SQL is compared against a golden. This catches
   changes in what is generated without needing a server.
3. **Execute**: the SQL runs against real PostgreSQL against a seeded database,
   and **the returned rows are asserted**. This is the tier that catches the
   defects the other two cannot: a query that compiles, plans, looks correct and
   returns the wrong rows.

The execute tier is why the harness needs a server. `scripts/conformance-pg.sh
start` brings up a throwaway cluster on its own port and socket.

**A green run that never reached Postgres would prove nothing**, so CI asserts
its own execution: it runs a single known case and greps for the pass line.

### Namespacing a test database per run

`Pg::scoped(stem)` creates `<stem>_<pid>_<nanos>` (`test_db_name` is the same
function if you need the name yourself), so two `cargo test` invocations, or
two agents working at once, never touch the same database. The stem stays the
readable part of any failure: `relation "tasks" does not exist in
ordo_rt_sup_e2e_84213_a91fb02c` still names the suite.

**`Pg::named` is the unnamespaced one and is still the majority.** Namespacing
is the right default but is not yet the default, because 95 call sites across
63 files spell the database name a *second* time (a `PoolConfig`, a `psql`
invocation, a helper taking `db: &str` and building a connection string). Each
of those opens the bare stem, a different database from the one the fixture
seeded. Switching `named` wholesale fails 69 tests across 12 suites at once,
so suites move to `scoped` one at a time. Converted so far:
`provision_target`, `health`, `restore_drill`, `restore_witness`, `dev_loop`.

**Converting a suite means spelling the name once.** Read it off the `Pg`
(`pg.db`) or off the pool (`pool.config().db`) and never re-type it. That is
the whole of the migration and also the whole of the hazard.

**This is a correctness fix that was diagnosed as a flake
four separate times.** `Fixture::reset` opens with `DROP SCHEMA public
CASCADE`, and test binaries run concurrently, so two suites on one database
tear down each other's tables mid-run. The failure surfaces in whichever suite lost the race
as `relation "tasks" does not exist`, which reads exactly like a bug in that
suite. Collisions across *runs* did real damage too: one agent's in-flight
migration sat in a database another agent's suite, sharing the fixed name,
then failed against.

Two rules to know before writing a test:

- **Never spell a database name twice.** A `PoolConfig` built on the bare stem
  opens a *different* database from the one the fixture seeded. Read it off
  `Pg::scoped`'s return (`pg.db`) or off the pool (`pool.config().db`). Salvaging
  this change turned up a test doing exactly that with the *control* database:
  it named the bare stem, the refusal it asserted stopped firing, and it passed
  for the wrong reason.
- **Nothing drops its own database on the way out**, so a failed run's state is
  still there to inspect. `Pg::scoped` reaps its own stem's dead-pid siblings on
  the next creation, and `scripts/reap-test-dbs.sh` sweeps the whole cluster for
  stems nothing asks for any more. Both check the pid with `ps` and never touch
  a database whose owning process is alive, so either is safe to run mid-suite.

`ORDO_TEST_DB_SUFFIX` pins the suffix, for a scripted before/after comparison
that wants the same name on both sides.

## The three states a case can be in

### EXECUTED

The case has a real assertion in the suite. The coverage test proves each listed
id **actually appears in the file**, so the list cannot outrun the suite. You
cannot claim a case by adding its id to a list.

### NOT_RUN

The case cannot run yet, and **each entry names the unbuilt construct it waits
on**. Each entry is a claim a reader can check. "GC064: share/exposure
registration does not exist" is falsifiable: go and look.

Reasons go stale, and a stale reason is a small lie. One entry was recently found
still claiming a feature was unbuilt after it had shipped. When you build
something, sweep this list.

### DIVERGENT

**The document and the implementation disagree.** Each one is a finding, and
the test is never bent to fit.

The divergence is asserted **as the implementation actually behaves**, and
recorded with a message telling the next reader to retire the test and promote
the case. So the entry fails the moment the implementation changes, including
when somebody fixes it. That stops a known disagreement from being
quietly forgotten.

An entry retires when the disagreement is **ruled**. Either side can lose:

- **The implementation was wrong**: 9r and 10f were settled this way. `GC077`
  and `GC100` looked like they pulled in opposite directions until a boundary
  ruling showed they were one rule applied twice.
- **The document was wrong**: 10g was settled this way, by editing the corpus
  text. That fix settles the case as completely as a code fix does.

Rulings are recorded in [`corpus-rulings.md`](corpus-rulings.md) with their
reasoning as well as their outcome. A future reader needs to know *why* a side
lost, or the ruling gets relitigated.

## Current state

**86 of 100 executing** (14 `NOT_RUN`, each naming its unbuilt construct;
verified against `EXECUTED`/`NOT_RUN` in `guards_corpus.rs`, 2026-09-07).
**Two** recorded divergences in the guards corpus, each a genuine finding:

| Case | The disagreement | Status |
|---|---|---|
| `GC007` | The document says a dot-path target's read guard inlines (R-G6 stacking). It does not: `taskSheet` LEFT JOINs `projects` with no guard on the join. The fixture hides it, because a project member satisfies both guards. | **A real gap, located, ruled, and blocked on the fixture.** `ordo-sema`'s `walk_refs` pushes a bare `Join` for each new hop; the `Join` IR carries no guard field at all, while `entity_source` pushes the base entity's guard into `q.filters`. Surfaced by the field-scope manifest, which shows `projects.{name}` read while `projects.owner` (which its own guard would test) is not. **Ruled 2026-09-26**: the guard belongs in the join's `ON`, so the field nulls and the row stays. See `corpus-rulings.md`. **The fixture cannot express the falsifying case**; see below. |
| `GC083` | The derived admin ops exist and carry their mode labels, but the manifest spells the gate as the link's write walk rather than as the case's `gatedBy`. | **Same facts, different vocabulary.** The narrower of the two, and ruled to be left alone, because forcing it churns the manifest for no behavioural gain. |

### GC007 is closed (2026-09-28)

Fixed with the two other instances of the same defect the security review
and the audit after it found (`security-review-6.md` S6-001 and §8): the
inferred bridge of a multi-source query, and the signpost of a whole-read.
The falsifying case lives in `tests/guards_compiler_joins.rs` with a program
of its own, which gets round the obstacle the section below describes.
The shared fixture still cannot hold the case, and no longer has to.

What follows is kept as the record of why it stood.

### Why GC007 stood open: the fixture cannot express its falsifying case

Checked entity by entity, 2026-09-26. "The fixture hides it" holds more
strongly than it sounds. **Every dot-path in `market.ordo` reaches a
target whose read guard is either implied by the source's or absent**, so no
caller exists who may read a row and may not read what it dots into:

| dot-path | why it cannot falsify |
|---|---|
| `tasks.project -> projects` | `tasks` read is `project.projectMembers(member == $caller)`; `projects` read is `owner == $caller or projectMembers(member == $caller)`. Task-readable ⟹ you are a project member ⟹ the second disjunct holds. **Implied.** |
| `projectMembers.project -> projects` | same implication, same reason |
| `taskNotes.task -> tasks` | `owned`, so the guard is inherited: one guard, not two |
| `cards.board -> boards` | `owned`, inherited from the `acl` |
| `tasks.assignee`, `projects.owner`, `projectMembers.member`, `folders.owner` | all `-> members`, and the platform's `members` carries `guards: None` |
| `folders.parent -> folders` | same entity, and its read is `open` |

Nobody can supply the missing test by writing a case.
It needs **a guarded entity that something unguarded-or-differently-guarded
dots into**, which this fixture does not contain. That is the honest reason
GC007 stood as a recorded divergence and was not fixed at once. The fix
itself is small, and there was no way to show it works.

**What the case needs**, for whoever builds it: an entity with a read guard
that is *independent* of its referrer's, reached by a non-`owned` ref. Adding
one to `market.ordo` is the natural move, and it has a cost. `gc_support`'s own
module doc explains that the guards corpus registers its ops separately
precisely to avoid "moving numbers that other suites assert about the shipped
fixture", and `ordo-host`'s catalog and load gate both count
`f.registry.names()`. An entity is not an op, so op counts should hold, but the
schema, the DDL and the seeded rows all move, and that needs checking.

`GC048` was **retired 2026-09-26** by fixing the document. It said
`guards-conformance.md` listed `level` among `cardList`'s guard reads; `level`
is tested only by the write guard and `cardList` is a read, so post-pruning
drops it, which is the discipline `GC080` states. The engine had always
followed `GC080` (the conformance assertion never included `level`), so the
document was the wrong half. See `corpus-rulings.md`.

`NOT_RUN` currently waits on router-tier assertions and the trigger cases
(`GC097`, `GC098`). The trigger cases are being built. `GC063` to `GC065`
(share registration) run in `crates/ordo-host/tests/sharing.rs`, and the
guards corpus lists them as executed elsewhere.

## Adding a case

1. **Find its id in the source document.** If it has no id, the document needs
   one first, because a case with no id cannot be audited.
2. **Transcribe the expectation from the document**, not from a run. If you find
   yourself running the code to discover what to assert, stop: that produces a
   test that agrees with the implementation by construction.
3. **Add the assertion to the suite that owns that document.**
4. **Add the id to `EXECUTED`.** The coverage test will fail if the assertion is
   not really there.
5. **If it fails, decide which side is wrong** before touching anything. If the
   implementation is wrong, fix it. If the document is wrong, fix the document
   and record the ruling. If you cannot resolve it now, add it to `DIVERGENT`
   asserting current behaviour. Never leave it failing, and never weaken it.

## Goldens

Some cases compare generated SQL, DDL or the plan artifact against a stored
golden file. Goldens are regenerated only with `ORDO_UPDATE_GOLDEN=1`.

**A golden that moves is a question to answer.** The diff says what changed
about the compiler's output; if you cannot explain why the new bytes are right,
the change is not ready. Several real defects in this project were caught by
someone reading a golden diff line by line after the golden was regenerated. At
least one was nearly missed because the regenerating came first and the looking after.

## Databases

Each suite owns its databases, created on demand via `Pg::named("…")`.

Suites run **concurrently** under `cargo test`, and the
fixture reset opens with `DROP SCHEMA public CASCADE`, so two suites sharing a
database tear down each other's tables mid-run. The failure surfaces in whichever
suite lost the race, reads as a bug in *that* suite, and passes on the isolated
rerun anyone would try next. It was diagnosed as a flake four separate times
before being fixed.

`Pg::named` gives a new suite isolation as soon as it asks for a database.

## Related

- [`corpus-rulings.md`](corpus-rulings.md): every ruling taken when a case
  disagreed, with the reasoning.
- [`roadmap-checklist.md`](roadmap-checklist.md): the work items, including
  every divergence's history.
- [`guards-conformance.md`](guards-conformance.md): the authorization document
  the `GC` cases transcribe.
- [`composition-conformance.md`](composition-conformance.md): the composition
  document the `C` cases transcribe, and the compile-class inventory.
