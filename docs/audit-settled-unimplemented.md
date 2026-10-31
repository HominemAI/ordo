# Audit — settled rules with no implementation

**Date:** 2026-09-02 · **Method:** documents first, then the mechanism in the
code, then a throwaway probe against the conformance cluster wherever the gap
could be executed rather than argued.

Scope worked, in the order the brief set: `security-logging-design.md`,
`bootstrap.md`, `definition-language.md`, `api-engine-design.md`,
`composition-conformance.md`, `guards-conformance.md`, `writing-data.md`.
`docs/roadmap-checklist.md` was read first and every finding below was checked
against it; where an item is partly recorded the entry says so and says what
the record understates.

Seven findings. **Every one of them was confirmed by execution** except F6,
which is a reading of the code plus the absence of any caller. The probes were
deleted after they ran; each finding carries the exact source text to
reconstruct it.

Severity is about consequence, not effort. Ordered security-relevant first.

---

## F1 · `members` is writable by domain code — privilege escalation · **Critical**

**The rule.** `definition-language.md` §3, *"`members` — the principal entity
**[settled]**"*:

> **`members` is platform-provided, per space, and undeclarable** … Fields (all
> read-only to domain code, queryable like any field)

and, of the row itself:

> **Who creates the row [settled]:** the platform does, as an ordinary
> attributed command in the space's chain, at exactly four moments … **There is
> no domain-authored path to minting one**, and no path at all for `routine:`,
> `worker:`, `source:`, or `system:` principals

`displayName` is restated separately as *"**read-only to domain code**: a
domain refs `-> members` and reads `assignee.displayName`; **it cannot write
it**"*, and `memberCount` as *"int, read-only"*. `status` carries
`security-logging-design.md`'s *"Suspension is an authorization input
[settled]"* on top.

**What should implement it.** Nothing does. `ordo_schema::platform_members()`
(`crates/ordo-schema/src/lib.rs:421`) builds the entity with every field at
`implicit: false` — the flag that marks `id`/`rev`/`depth` as the platform's —
and with `guards: None`, i.e. **unguarded**, so no row-level check applies
either. `RESERVED` (`lib.rs:412`) is `&["id", "rev", "depth"]`; the
undeclarability check refuses `entity members { … }` by name only. Neither
`ordo_plan::writes::check_writes` nor `ordo_plan::cost::check_registration`
knows that `members` is a write target it should refuse.

**What it does instead.** A domain-authored `set`/`del` over `members`
compiles, registers, and executes.

**How it was confirmed.** Executed, against real Postgres
(`ordo_audit_members`), through `Fixture::from_source(FIXTURE + ops, SEED)` and
`Fixture::run_write_as(...)` — the same write protocol the conformance suite
runs. The ops:

```
op promote($id: id)     = set members { id: $id, role: "owner" }
op unsuspend($id: id)   = set members { id: $id, status: "active" }
op promoteAll($x: text) = set members(status == $x) { role: "owner" }
op wipe($id: id)        = del members { id: $id }
```

All four register. Run **as `mem_2`, an `editor`**:

| call | answer |
|---|---|
| `promote($id: "mem_2")` | `{"entity":"members","updated":1}` |
| `unsuspend($id: "mem_2")` | `{"entity":"members","updated":1}` |
| `promoteAll($x: "active")` | `{"entity":"members","updated":6}` |
| `wipe($id: "mem_4")` | `{"entity":"members","deleted":1}` |

`members` before: `mem_1 manager · mem_2 editor · mem_3 editor · mem_4 viewer ·
mem_g (null) · mem_o owner`. After: five rows, **every one of them `owner`**,
and `mem_4` gone.

Only one spelling was refused, and by accident rather than by rule: minting a
row outright needs `principal:` in the braces, and `principal` is a lexer
keyword, so `set members { principal: … }` is `parse_error`. Insert-completeness
then refuses the create because a required field cannot be named. Nothing about
that is the rule DL states; change the keyword list and the create lands too.

**The consequence.** `ordo_host::deployment_engine`
(`crates/ordo-host/src/lib.rs:1001`) builds the `AccessEngine`'s memberships
from `SELECT "id", "role" FROM "members"`. The role a domain op writes **is**
the role the door reads. So this is not "a domain can corrupt a display
column" — it is:

- **role assignment**: any caller who can reach such an op becomes `owner`, and
  owner is the role the same file grants `/health/deep` and `/metrics` to;
- **suspension defeat**: `status: "active"` undoes the one control
  `security-logging-design.md` calls *"how an operator stops a compromised
  member now"* — checked first in `AccessEngine::check`, and now writable by the
  principal it stopped;
- **membership deletion**: `del members` removes a principal's whole per-space
  identity, taking their acl rows' referent with it;
- **seat/quorum forgery**: `memberCount` is the number DL's quorum example sums.

The op-level access check is the only thing standing between an app author and
the role table, and the reference profile grants every authored write op to
`Editor` and up (`lib.rs:1045`) — which is exactly the caller the probe used.
DL's rule exists to make this unspellable rather than merely ungranted, and
`domain-packages.md` makes third-party domains a shipped concept, so "the
author would not write that op" is not a control.

**Not in the checklist.** No item covers writes to platform entities.

**The shape of the fix** (stated, not built): the same `implicit: true`/reserved
treatment `id`/`rev`/`depth` already get, extended to every `platform_members`
field, plus a registration-time refusal of any `set`/`del` naming a
platform-provided entity — the platform's own membership commands do not go
through the domain write path.

---

## F2 · A space owner cannot administer acl rows — the lockout DL says cannot happen · **High**

**The rule.** `definition-language.md` §3 `acl`, bootstrap rule 6 **[settled]**:

> **Space owners administer any acl row in their space, and the check is at the
> DOOR [settled].** A space owner may grant, revoke, and change levels on every
> acl-derived link entity in the space **without holding an access row on
> anything** … evaluated **at the door (G18), once per call, before planning**
> — not as an extra disjunct in each row's guard. When it applies, the derived
> link entity's guards are not consulted for that call, and the command is
> logged with the owner as actor and flagged as owner administration.

> It covers the routine/system-created rows of rule 5, the guest-created rows of
> rule 4, and the "everyone with admin left the company" case, and it means **no
> state of the data can ever lock a space's owner out of their own space.**

**What should implement it.** The door (`ordo_identity::AccessEngine::check`) or
`ordo_host::IdentityAccess`. Neither has any notion of it. `Role::Owner` appears
in exactly four places in non-test code — a level number, a string parse, a loop
over roles, and two reserved op names — and nowhere as a capability. `grep -rn
"owner administration|owner_admin|acl administration"` over `crates/` returns
nothing.

**What it does instead.** The derived link entity's own write guard —
`ordo_schema`'s acl derivation, `lib.rs:1600` — is `<back>.<link>(member ==
$caller and level == "admin")`, top level only, with no owner disjunct and no
door-level exemption. An owner holding no acl row fails it like anyone else.

**How it was confirmed.** Executed, against real Postgres
(`ordo_audit_owner`), on the shipped `market` fixture, where `mem_o` is the
space `owner` and holds no `boardsAccess` row anywhere:

| call | as `mem_o` (space owner) | as `mem_1` (holds `admin` on `board_1`) |
|---|---|---|
| `grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")` | `Err("not_found: no such row")` | `{"created":"boar-acce_w1"}` |
| `revokeBoardsAccess($board: "board_1", $member: "mem_3")` | `Err("not_found: no such row")` | `{"deleted":1}` |
| `setBoardsAccessLevel($board: "board_1", $member: "mem_2", $level: "admin")` | `{"entity":"boardsAccess","updated":0}` | `{"updated":1}` |

**The consequence.** Two, and the second is the nastier one.

1. The lockout DL rules out is reachable and permanent. Every row created by a
   routine, system or source principal (rule 5 — *no* auto-grant) and every row
   created by a guest (rule 4 — *no* auto-grant) is administrable by nobody,
   ever. So is any row whose admins all left. The rule exists to be the repair
   path for exactly rules 4 and 5, which the checklist records as **closed**
   (10b) on the strength of "the space owner is the repair path" — a repair path
   that does not exist.
2. `setBoardsAccessLevel` answers **`updated: 0` with a 200-shaped success
   envelope**. An owner who "fixes" a level is told it worked. Silent no-op on
   an authorization change is worse than the refusal the other two give.

Fail-closed, so not an escalation — but it is an availability and
recoverability guarantee stated in absolute terms with no implementation.

**Not in the checklist.** 10a/10c/10h build the ladder, the constraints and the
admin ops; none of them mentions rule 6.

---

## F3 · `since` is frozen at a hard-coded 2026-05-01 · **High**

**The rule.** `definition-language.md` §2, *"`since` anchor precision"*:
*"`since` on a `date` field compares against the **execution-time** UTC date."*
And §4, *"### `since` and reproducibility [settled]"*: *"`since` anchors to
execution time — the same op returns different rows tomorrow."*

**What should implement it.** `ordo_sema::Binder::clock`
(`crates/ordo-sema/src/lib.rs:384`), whose own doc comment says:

> The conformance corpus pins 2026-05-01T00:00:00Z (§2.4 stage 3); **a real
> runtime will supply the request's execution instant.** Folded at BIND time
> into an exact date literal — no `NOW()` ever reaches SQL.

**What it does instead.** No runtime supplies anything. `clock` is assigned the
literal `(2026, 5, 1)` at **all eight** `Binder` construction sites in
`ordo-sema` (lines 463, 505, 546, 721, 1086, 1137, 1168, 1211). There is no
constructor, builder or setter that takes a clock, and no caller in the
workspace — `ordo-exec`, `ordo-host`, `ordo-plan`, `ordo-cli` — passes one.
`Condition::Since` (`lib.rs:4352`) folds `shift_date(self.clock, …)` into a
`DateLit` at bind time, so the frozen value is baked into the SQL.

**How it was confirmed.** Executed. Compiling against the `market` fixture on
2026-09-02:

```
orders(placed since 30d)[placed, total]
  →  … AND ("t0"."placed" >= DATE '2026-04-01')
orders(placed since 1y)[placed, total]
  →  … AND ("t0"."placed" >= DATE '2025-05-01')
```

`since 30d` is a 154-day window and widening by one day per day.

**The consequence.** Every time-windowed op in the platform — every dashboard,
every trailing-window report, every routine condition that uses `since` — reads
a window anchored four months in the past and drifting. It is silent: the query
succeeds, returns rows, and is wrong by an amount nobody can see from the
answer. No test can catch it, because the conformance corpus pins the same
clock, so the corpus and the engine agree with each other and with nothing else.

The one honest complication worth stating: DL also pins *"no `NOW()` ever
reaches SQL, which is what keeps plans reproducible"*, so the fix is not a
`CURRENT_DATE`; it is threading the request's execution instant into the bind
that already happens per request (`ordo_exec::Executor::read_as`,
`crates/ordo-exec/src/lib.rs:567`, binds on every call).

**Not in the checklist.**

---

## F4 · Two-valued null logic is not compiled — every comparison is SQL's 3VL · **High**

**The rule.** `definition-language.md` §4, *"### Null semantics [settled]"*,
with a fixed truth table (`null == null` → true, `null != 5` → true,
`null != null` → false), and then, headed **[settled]** in its own right:

> **Planner note — two-valued logic is a compilation obligation [settled].** …
> Therefore **every comparison compiles to the distinctness family** — `IS NOT
> DISTINCT FROM` for `==`, `IS DISTINCT FROM` for `!=` and for `not (==)` —
> **everywhere**, without exception: pipeline conditions, source parens, `cond`
> arms, `check` constraints, guard semi-joins, join `ON` clauses, and the
> predicates behind bulk writes. A plain `=` in generated SQL is a bug wherever
> a nullable operand can reach it.

§4 also pins `in` as *"**exactly sugar for a chain of `==`**"*, so *"a null in
the list matches a null value"* and `not (x in …)` is the strict negation of
that chain. `sql-translation-corpus.md` §A4 restates it: a list that may contain
a null *"compiles to an OR-chain of `IS NOT DISTINCT FROM` … never to SQL's
`IN`."*

**What should implement it.** `ordo_plan::Printer::cond`,
`crates/ordo-plan/src/lib.rs:1465`.

**What it does instead** (`lib.rs:1469`):

```rust
CmpOp::Eq => "=",
CmpOp::Ne => "<>",
```

unconditionally, with `BCond::In` → SQL `IN` and `BCond::Not` → SQL `NOT`.
`IS NOT DISTINCT FROM` appears **nowhere** in generated SQL — the only two hits
in the whole workspace are doc comments, one of which
(`crates/ordo-sema/src/ir.rs:25`) asserts the mechanism exists: *"exactly as it
reaches every other column through `IS NOT DISTINCT FROM`."*

**How it was confirmed.** Executed against real Postgres (`ordo_audit_2vl`) on
the `wide` fixture, whose `people` rows are: `ppl_1` nickname NULL age 34 ·
`ppl_2` 'B' NULL · `ppl_3` NULL 45 · `ppl_4` 'Dee' 29 · `ppl_5` NULL 51 ·
`ppl_6` NULL NULL.

| source | emitted | rows | DL requires |
|---|---|---|---|
| `people(nickname == null)` | `"nickname" = NULL` | **0** | 4 (`null == null` true) |
| `people(nickname != null)` | `"nickname" <> NULL` | **0** | 2 |
| `people(nickname != "nope")` | `"nickname" <> 'nope'` | **2** | 6 (`null != 5` true) |
| `people(not (nickname == "nope"))` | `NOT ("nickname" = 'nope')` | **2** | 6 |
| `people(age != 30)` | `"age" <> 30` | **4** | 6 |
| `people(nickname in ("nope", null))` | `"nickname" IN ('nope', NULL)` | **0** | 4 |
| `people(not (nickname in ("nope")))` | `NOT ("nickname" IN ('nope'))` | **2** | 6 |

Every row is wrong, and wrong by silent omission.

**The consequence.** The language's single most-advertised semantic departure —
*"SQL's three-valued folklore is deleted"*, *"one equality"* — is not
implemented at all. Any predicate over a nullable column silently drops rows,
in reads, in `cond` arms, in guard semi-joins, and in the predicates behind bulk
writes. `x != y` and `not (x == y)` do **not** partition the rows, which is the
property `unique` and `null_key_address` are argued from elsewhere.

For guards the direction happens to be conservative (3VL hides rows the language
would show), so this is a correctness finding rather than a bypass. It is not
conservative for **bulk writes**: a bulk `set`/`del` whose condition names a
nullable column touches a different row set than the language defines, and the
caller is told `updated: n` for the wrong `n`.

**Recorded as CLOSED, wrongly.** `critique-consolidated.md` **X-011** is this
exact finding (*"Two-valued null logic must compile to `IS NOT DISTINCT FROM`;
the planner phase is silent"*), and `critique-consolidated-4.md` lists X-011 in
the **closed** row. It was closed as a *document* fix — the rule was written
into DL §4 — and the planner was never changed. `critique-consolidated.md`'s
own workplan row **D9** ("`IS NOT DISTINCT FROM` everywhere") has no checklist
item. That "closed" mark is why nobody looked again.

---

## F5 · Groups are unimplemented below the in-memory access engine · **Medium-High**

**The rule.** `definition-language.md` §3, *"### Groups [settled]"*:

> **`groupMembers` is the platform's group-membership link entity [settled]**:
> `groupMembers { group -> members owned, member -> members owned }`, per space,
> platform-maintained and undeclarable (same status as `members` itself), but
> **queryable and walkable like any entity** — which is what makes group-aware
> guards and headcounts writable instead of magical.

> **Counting people, not rows: `memberCount` [settled].** Every members row
> carries an implicit read-only `memberCount: int` … platform-maintained
> alongside the membership commands (same machinery as a tree's `depth`)

> **Groups may not contain groups in v1** — nesting is rejected at write time
> (`group_nesting`).

and, of the derived guards: *"The acl-derived guards carry exactly this second
disjunct"* — the disjunct being
`project.projectMembers(member.groupMembers.group(member == $caller))`.
`security-logging-design.md` §6 restates the door half as settled and adds
*"Row-level guards see groups too, as one more hop in the `$caller` walk."*

**What implements what.** The **door** half is built and correct:
`AccessProfile::ops_by_group` / `role_by_group` / two-stage overrides
(`crates/ordo-identity/src/principal.rs`) implement §6 rules 1 and 2 exactly,
including `Role::max_level`. Everything below the door is absent:

- **`groupMembers` does not exist.** `grep -rn "groupMembers"` over `crates/`
  returns one doc comment. `ordo_schema` provides exactly one platform entity,
  `members`. So the group disjunct DL prints verbatim cannot compile — its
  middle hop names an entity the schema does not have.
- **The acl derivation emits no group disjunct.** `crates/ordo-schema/src/lib.rs`
  `walk()` (≈1563) builds `<link>(member == $caller [and level == "…"])` and
  nothing else. Confirmed by reading the generator and by the guards corpus's
  own goldens.
- **`memberCount` is a plain writable column.** `platform_members()` builds it
  with `implicit: false` and nothing maintains it; the shipped seed stores `0`
  for every row, including the group-capable ones. DL calls it "implicit
  read-only". (It is also inside F1's blast radius.)
- **`group_nesting` has no producer**, and no error inventory row.

**How it was confirmed.** Reading plus one executed schema probe: building a
program and dumping `Schema::entities` shows `members` with fields
`[id, rev, principal, displayName, kind, memberCount, role, status, joined]` and
no `groupMembers` entity at all.

**The consequence.** An acl grant to a `kind: group` row creates a link row
naming the group and confers **nothing** at the row level, because `== $caller`
is the caller's own row by rule. Meanwhile the door *does* expand groups, so a
group grant opens the op and hides every row — the two layers disagree about
what a group grant means. DL's stated seat/quorum pipelines and the exact
headcount recipe cannot be written. It is fail-closed, which is why it rates
below F1/F2.

**Partly recorded, and the record understates it.** Checklist 10h contains the
clause *"it needs the group expansion DL §3 describes, and nothing expands
groups yet"*, framed as the reason one revoke report is not derived. The gap is
the whole feature — the platform link entity, the guard hop, the maintained
count, the nesting refusal — and it is load-bearing for
`security-logging-design.md` §6 rule 4's *"`callersOf` stays exhaustive"*,
which 8r already blocks on.

**Closed 2026-09-05** (checklist 10i; rulings in `corpus-rulings.md`). Every
bullet above is now false, and each was closed by the thing it named: the
platform entity and the guard hop landed first, then the membership surface
that writes the rows, `memberCount` maintained alongside the membership
commands, and `group_nesting` produced at the add. The two layers no longer
disagree — a group grant now confers at the row level exactly what it opens at
the door. **Still owed from this finding:** `callersOf` itself (8r), and
R-G17's surviving-grant-path report as a *derived op* — the query under it
exists and is asserted (`grant_paths`), which retires the reason the
derivation gave for not deriving it.

---

## F6 · The security-event stream has no drainer on any host path · **Medium**

**The rule.** `security-logging-design.md` §6.1, *"### The security-event stream
— where denials are recorded [settled]"*:

> **Every space has a second append-only, hash-chained stream — its
> security-event stream** … A space owner reads both; `verify()` verifies both,
> independently.

> **No interaction with the command chain's append lock** [pinned]. The stream
> has its own per-space lock, taken in its own transaction, and appends run
> **off the request path through the ordinary job queue.**

**What should implement it.** `ordo_secevent::Recorder::flush_into`, driven by
`ordo_exec::jobs`.

**What it does instead.** Nothing calls `flush_into` or `drain_ready` outside
`ordo-secevent`'s own unit tests. `ordo_exec::secevent::PgSecurityStore` — the
durable half, proved against Postgres — is constructed **only** in
`crates/ordo-exec/tests/secevent.rs`. In the host, `IdentityAccess.stream` is a
bare `Arc<Recorder>` whose only drain is `take_audit()`, and the field's own doc
says so: *"with no drainer wired, entries accumulate in memory and `take_audit`
is the drain."*

**How it was confirmed.** Reading, plus the absence of any caller —
`grep -rn "flush_into\|drain_ready\|PgSecurityStore" crates/` yields tests and
doc comments only. Not executed, because there is no server binary to run:
`find crates -name main.rs` returns `ordo-cli` and `ordo-playground` only, and
`ordo-host` is a library.

**The consequence.** In anything that mounts `ordo_host::serve::platform_router`,
the tamper-evident denial record §6.1 calls settled does not exist: denials
accumulate in an unbounded in-process `Vec<StreamEntry>`, are never hashed into
a chain, never checkpointed, never verifiable, and are lost on restart. An
unauthenticated caller can grow that `Vec` at request rate, which is a memory
sink as well as an evidence gap. §6.1's *"members only"* routing — the rule that
stops a stranger growing a victim space's spine — is computed correctly and then
has nowhere to write it.

**Honestly bounded**, which is why it is Medium and not higher: `ordo-secevent`'s
module doc lists the drainer under *"What a deployment must still supply"*, and
there is no deployment yet. But checklist **7a** records the stream as **BUILT**
and its honesty paragraph (under 7b) names only the *alert's* limits — one
process's counters, no exporter, no persistence across a restart — not the
stream's missing drainer. Assembling the drainer belongs on the list.

---

## F7 · Six DL-reserved words are not reserved in the grammar · **Low-Medium**

**The rule.** `definition-language.md` §2, *"Lexical formalization [settled]"*:

> **Reserved words**: keywords are reserved and cannot name fields/entities:
> `… run trigger check index … whole cond op access acl read write open
> unguarded … policy channel route app page std`. … The list is **closed and
> mechanically regenerated from the grammar** — a keyword that exists in the
> grammar and not in this list is a bug in the list, and every entry carries a
> conformance reject case (`entity x { read: int }` must fail).

**What it does instead.** Six entries of that closed list lex as ordinary
identifiers: **`run`, `read`, `write`, `open`, `route`, `std`**. The lexer's own
`fully_reserved_words_lex_as_keywords` test
(`crates/ordo-syntax/src/lexer.rs:757`) enumerates the list by hand and omits
exactly those six, so the assertion passes vacuously about them. Nothing is
mechanically regenerated from anything.

**How it was confirmed.** Executed: for each of the 64 words DL lists,
`entity x { <w>: int }` and `entity <w> { a: int }` were parsed. Both forms
were accepted for `run`, `read`, `write`, `open`, `route`, `std` and refused
for the other 58. Then, through the full compiler:

- `entity std { name: text }` builds, and `ordo_schema::std_ops` mints
  **`std.std.get`, `std.std.count`, `std.std.create`, `std.std.update`,
  `std.std.delete`** — a domain entity colliding with the reserved std op
  namespace by name.
- `entity things { read: bool, write: bool, run: text, route: text }` builds.
- `entity notes { open: bool … }` builds beside an ordinary `access` clause.

**The consequence.** Mostly forward-compatibility: any future grammar that
needs these words breaks every program that used them as names, which is the
cost DL's closed list exists to pay once. `std` is the sharpest today — a
domain entity named `std` produces op names in the platform's own namespace.
`open` is the one the corpus already worried about: checklist 10f rules the
current `open`-in-a-guard answer correct *"and one that would have gone on
being right the day somebody declared a `bool` called `open`"* — that day is
today, though the concrete hazard it feared does not materialise, because
`access read open == false` is still refused with the right message (*"`open`
is the whole access clause, never an operand inside a walk"*).

**Not in the checklist.**

---

## Checked and found correctly implemented

A clean bill, so the next reader knows where not to look again. Each of these
was traced from the settled sentence to the mechanism, and the ones marked
*(executed)* were also run.

**Schema and DDL**

- **`UNIQUE NULLS NOT DISTINCT` on every generated unique index** (DL,
  "Uniqueness uses language equality"). `ordo_schema::ddl::nulls_not_distinct`
  emits it for single-column and composite keys, and deliberately not for a
  blind index, whose digest column is `NOT NULL`. **Checklist 8l's trailing
  "still owed … the DDL still emits plain `UNIQUE`" is stale** and should be
  struck — it was fixed by 9e and the note survived.
- **The space column leads every unique index, without exception**; primary key
  is `(space, id)`; every ref's foreign key is `(space, col) → (space, id)`, so
  a cross-space edge is refused by Postgres rather than by nobody trying.
- **Blind-index null sentinel** and `$bidx NOT NULL` even when the base field is
  optional (8l), matching DL's at-most-one-null rule through the digest.
- **`COLLATE "C"` on every text ordering** (DL, "Text ordering") — present in
  the goldens and in every probe plan I generated. *(executed, incidentally)*
- **Month/year clamping**, `ordo_sema::shift_date`: `d.min(last_day_of_month(…))`
  — DL's 2026-03-31 − 1mo = 2026-02-28 rule. (The value it clamps is wrong;
  see F3. The arithmetic is right.)
- **`std_ops` does not derive over `members`** — verified in a probe dump.

**The door** (`ordo-identity`, `ordo-host`, `ordo-api`)

- **Suspension is checked before token shape, capability, role, group and
  override** (`AccessEngine::check`) — `security-logging-design.md`'s
  "Suspension is an authorization input [settled]".
- **Capability path ordering**: space equality **first**, then expiry, then
  manifest hash, then allowlist — exactly §6's pinned order, with the reason in
  the code. Capability tokens never fall through to roles or groups.
- **Step-up after inclusion**, refused structurally for capability tokens
  ("presence is not delegable"), freshness evaluated at check time, and
  `StepUpRequirement::strongest` making a client's declaration a max and never a
  replacement. `None` is the identity, so omitting an op is not an exemption.
- **Role combination is max-level; overrides are a second stage that can
  subtract; deny short-circuits** — §6 rules 1 and 2.
- **Every denial carries the attempted op**; the *decision* is recorded
  distinctly from the wire code (`RecordedDecision`), and §6.1's members-only
  routing (`ordo_secevent::route`) sends an unverified subject to the deployment
  scope whatever else is passed. (Where those records then go is F6.)
- **One door answer for every refusal** — unknown op, forbidden op, missing and
  malformed token all produce the identical `access_denied`, and no pre-door
  response carries version headers.
- **`$caller` in the request envelope is its own refusal**, raised before
  binding or planning, on both transports.
- **`$space` is bound before the access check, from either carrier, with
  disagreement an error rather than a precedence rule**, stripped before
  parameter binding, and re-verified at the executor boundary
  (`Gate::approved_space`).
- **Unknown `$`-envelope keys are `unknown_param`**; page tokens are MAC-bound to
  space, op and principal.
- **Idempotency**: reservation before execution, honoured on POST/DELETE only,
  `KeyBinding::Permanent` for creating ops, conflict outcomes stored so a retry
  cannot flip to re-execution.

**Refinement** (`ordo-api/src/refine.rs`) — all three declaration-time sealed
rules of `std-library.md` §2.1: not refinable by default, equality-only where
opted in, and no sealed condition at all on an aggregate-only op.

**Error inventory** — `ordo-api/tests/inventory_is_closed.rs` compares the
document's table against `ErrorCode` in **both** directions, statuses included,
with a shrink-only allow-list. This is the strongest doc↔code binding in the
repo and is why I did not find anything here.

**Already recorded as owed, and correctly so** (not findings): `check` row
invariants blocked on a missing error class (9e); grant pinning combined rather
than per-op (8s); `std.space.*` including `callersOf` (8r, 9k′);
`refinement_unbounded` (allow-listed, machine-checked); host-side sealed
filtering (5c); revoke's surviving-grant-path report (10h).

---

## Method note

Everything above began from a `[settled]` mark or a flat assertion about what
the platform does, and each was traced to a mechanism by grepping for the
mechanism rather than the words — `IS NOT DISTINCT FROM` rather than "null
semantics", `groupMembers` rather than "groups", `Role::Owner` rather than
"owner administers".

`[lean]` constructs were excluded throughout, per the standing ruling that a
lean construct is not owed. Two readings were taken deliberately:

- DL §2's `[lean] enum(…)` and `[lean] list params + each` sit in the same
  paragraph as the **[settled]** text-canonicalization rule; only the settled
  half was audited.
- `std-library.md`'s `std.E.under` and `std.E.rollup` carry `[lean]` on the
  *shape*, and were left alone.

The probes were written under `crates/ordo-conformance/tests/`, run against
`scripts/conformance-pg.sh`'s cluster in their own `ordo_audit_*` databases,
and deleted. Each finding's source text above is enough to rebuild its probe.
