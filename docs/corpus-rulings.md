# Language rulings from the conformance corpus

The wants corpus (`crates/ordo-conformance/corpus/`) surfaced five language
questions the spec had not answered. All five are now ruled; this file records
the rulings, and the corpus files carry the same text next to the evidence.
Implementation of rulings 1 and 3 is Stage-1 work (`reduce`, multi-source) —
their corpus entries stay pinned `inexpressible` until the constructs land,
and the pinned test forces deliberate reclassification when they do.

---

## 1 · W039 — counting a distinct set · **RULED 2026-08-11**

`reduce` (already reserved: a `group` with no keys) runs after `distinct`.
No fused `count distinct` aggregate; counting composes from existing steps:

```
# how many distinct tags are in use — one number, not three rows
taskTags[tag]
distinct
reduce { n: count }
```

The structure to notice: each step narrows the *shape* of the result —
rows → distinct rows → one row of aggregates — using only concepts the
grammar already has. This is the pattern for every "turn a set into a
number" question, and the reason no fused aggregate is needed.

Complementary, ruled in the same breath: the API envelope carries a `total`
for any op (pagination needs it anyway), so a caller who only wants the
number of results never writes a query at all. Envelope totals are not
composable — inside another op, `reduce` is the spelling.

## 2 · W049 — avg's result type · **RULED 2026-08-11**

`avg` returns exact decimal. There is no arithmetic over aggregates
(`sum x / count` is not a spelling). The integer mean is inexpressible **by
design**; its pinned corpus entry says so permanently.

## 3 · W050 — empty groups · **RULED 2026-08-11** (adversarial review)

The multi-source form is already outer-preserving in the spec, with no
keyword. The settled spelling for "every team, including empty ones":

```
teams[]  people[]
group teams.id, name { n: count }
```

Adopted with the review's four amendments, each owed a conformance witness:

1. **`match any` keeps its one meaning** — semi-join, everywhere. It is
   never a preservation marker; the candidate spelling that used it that way
   would have deleted the empty team it existed to keep.
2. **Bare `count` counts the deepest many-entity**, and the null-extension
   row is masked from **every** aggregate, including `count(cond)`.
   Witnesses on the empty group: `count = 0`, `count(nickname is null) = 0`,
   `sum salary = 0`, `avg age = null`. (A naive `COUNT(*)` gives 1; a naive
   filtered count conjures a phantom person.)
3. **Preservation is per-edge for ≥3 sources** — an org with no teams and a
   team with no people each survive their own edge. This closes latent
   W052's empty-org gap.
4. **Guards inline inside the source, never the joined WHERE** — a guard
   that hides the last member of a team yields that team with `count 0`,
   never a vanished team. (WHERE-side inlining collapses the outer join.)

Rejected: the projection-position collection aggregate (`count people`) —
a second spelling for `group` that repeals two settled rules; and the
maintained counter — an unguardable read path (a caller-invariant number
discloses rows a guard hides) with a bootstrap paradox (backfilling it
requires the query it replaces).

**Disclosure rule** (answers judged want W044's third question): a `group`
whose key dots into a parent entity that is not a listed source gets a
compiler note — *"teams with no people will not appear; list `teams` as a
source to keep them"* — because the keep-empty and drop-empty spellings are
one source-token apart and both compile.

## 4 · W047 — step keys over unprojected fields · **RULED 2026-08-11**

A key does not need to be projected: steps resolve against outputs first,
then the pre-projection **source scope** — `products[name] sort price` is
legal and adds nothing to the output. The fallback dies with row identity
(group / distinct / settled callees), never tunnels past a callee's outputs,
and never reaches a sealed field (`sealed_unsupported` — sorting by a secret
is an ordering oracle). Errors state what is available, not what was
dropped. Implemented in `ordo-sema` (`Scope::Layered`), regression-tested.

## 5 · Latent wording gaps · **RESOLVED 2026-08-11** (by rulings 3 and 4)

- **Empty-bucket aggregates** (latent on W049/W052): pinned by ruling 3,
  amendment 2 — `count` 0, `sum` 0, `avg` null.
- **Empty-parent preservation** (latent on W052): pinned by ruling 3,
  amendment 3 — per-edge, explicit in the source list.
- **W053 stored-vs-derived depth**: `depth` is a **stored column the write
  path maintains** — the executor updates it when a tree edge changes, and
  it is read like any field. It is never derived at query time; a recursive
  read would make tree queries' cost depend on tree shape, which the cost
  classes cannot admit. Divergence between the stored value and the parent
  chain is corruption, and detecting it is the runtime shape-verification
  job (F6), not the query planner's.

---

## Post-Stage-1 closures (2026-08-12)

- **`month()` was never an open ruling** — DL ~617 pins date parts as
  BUCKETS (`month(d)` → `'2026-07'`, sortable scalars). The EXTRACT
  lowering was a bug that would have merged Februaries across years;
  C051 executes correctly now. Removed from the undecided list.
- **Minimal-connecting-subgraph inference** landed for N sources with
  compiler-inserted bridges; the fan-out guard counts many-edges at
  their one-side node (C083 caught the anchor-side hole).
- **`whole`** implemented per its settled section: owned closure down,
  one-hop signposts up, incoming associations never expand.
- Still F0-gated: C070–C074 executions, C081/C082 (giftOrders filters
  the sealed memo). Still open: the near-miss compiler note awaits a
  notes channel on BindResult.

- **The venue gate — now live** (owed by the audit of 2026-08-13, delivered
  with the F5 read executor, which classifies every plan and refuses a
  non-servable venue before a connection is used): DL ~1816 pins that `take` bounds RETURNED rows, not
  EXAMINED ones — `entity(sealed == $x) … take 50` needs the
  acknowledgment even though its row bound is 50. Today that shape
  cannot bind (sealed filters refuse), so the registration gate's
  self-bounded skip is sound by unreachability; the day host classes
  become emittable, an ack requirement keyed on venue class must land
  beside them or the skip silently generalizes what DL rejects.

---

## Stage 5 — the write protocol's executed slice (2026-08-14)

Write ops landed as bodies of `set`/`del` statements; modes are **derived,
never spelled**: no condition is a create, `id ==` is addressed, any other
condition ranges (bulk). Guard verbs become preconditions with a fixed
disclosure order — outside the READ view fails `not_found`, byte-class-equal
with a row that does not exist (G12, asserted); visible but failing the
write guard fails `access_guard` (G10); a bulk statement's range is
read-view ∩ condition ∩ write-view, and unwritable rows are silently
outside it (G11), exactly like rows a guard hides from a read.

Creates prove the write guard **against the would-be row**: atoms over
supplied scalars fold host-side, walks through a supplied ref pin the
parent by literal id and continue in SQL, and a reverse step rooted at the
new row is FALSE in its disjunct. When *every* caller-path is such a
reverse step the guard is statically create-exempt (G10a) — and the acl
bootstrap answers for it: the creator's top-level access row rides the
same command group as the root it governs.

Every accepted op appends one `CommandGroup` — attributed actor, one
transaction, before/after images per row, hash-chained and
`verify_chain`-checked in conformance. Ids mint from the entity key
(`task_w7`), `rev` starts at 1 and bumps per converge.

Gaps as of that stage, all since closed except the last: preconditions and
mutations were not one snapshot (**closed** — see AA-004 below), the log was
in-memory (**closed**), sealed-field writes were refused (**closed by F4**).
Tree writes ($path maintenance) are still refused by class, never faked.

## F4 — sealed writes land (2026-08-14)

`ordo-crypto` now seals: XChaCha20-Poly1305, envelope
`[epoch:1][nonce:24][ct||tag]`, AAD-bound to `entity:field` (a payload
moved to another column refuses to open), fresh random nonce per seal, no
dedup — all per implementation.md's settled custody design. The DEK is
per-epoch behind `KeyProvider` (test provider derives; real providers
unwrap); blind-index keys stay SMK-rooted and epoch-independent, so the
uniqueness-across-rotation story is unchanged.

The write path seals sealed fields (payload AS WRITTEN; only the index
normalizes), maintains `field$bidx` digests, and images sealed values as
opaque `SealedRef` handles. Conformance proves the full loop against
Postgres: mixed-case write → envelope in the column (epoch byte leading,
opened back to the original casing) → found by the lowercase blind-equality
read → converge re-seals with a changed envelope for the changed value.
Reads of sealed columns stay refused by name. Still absent, still refused
honestly: chunked STREAM blobs, and every read that would open a payload
(the F5 executor's venue-gated territory).

## Stage 6 — the F5 executor's first half (2026-08-14)

`ordo-exec` replaces the psql shell-out on the read path: `tokio-postgres`
with an **explicit, deliberately sized pool** (tokio-postgres ships none,
and an automanaged one would hide the platform's real concurrency bound).
The sizing decisions, all written down beside the code:

- **Fixed size**, `available_parallelism` clamped to [2, 8]. A backend is a
  process; more connections than cores buys queueing inside the server
  instead of in front of it where it is visible. The pool IS the DB-side
  concurrency bound, and the API layer inherits it because no request
  executes without a checkout.
- **Bounded acquire** (2s default) — waiters queue FIFO on a semaphore and
  time out into `PoolError::Saturated`, a fast typed failure the door can
  render retryable. Never an unbounded internal queue.
- **Eager dial at boot** (a bad config fails at start, not first request),
  **health at checkout** (a closed connection is replaced there), one
  spawned socket-driver task per connection — no second thread pool exists
  to tune, tokio's runtime threads are the pool's threads.
- **Transactions own their connection**, and **dropping an uncommitted
  guard discards the connection** — the socket closes, the server aborts,
  and no rollback path can be forgotten or half-run. Tested: uncommitted
  work is invisible outside the transaction and stays gone after the drop.

The read executor puts the **venue gate at the one point a plan becomes
work**: a non-servable cost class is refused before a connection is used,
so no caller path bypasses it. Rows decode canonically by declared output
type (money/int as JSON integers, never floats). `ordo-host` now serves
HTTP reads through it.

**AA-004 is closed.** The write protocol now runs against a `WriteDb`
seam rather than a psql handle, and the executor supplies an open
transaction as that seam: `FOR UPDATE` locks the addressed row (and each
selected bulk row) BEFORE its guards are evaluated, so nothing slips
between "the guard held" and "the update ran"; every statement of the op
shares one snapshot; success commits and any refusal rolls the whole
group back. Conformance proves it: a refused write leaves the row
untouched AND appends no command group, and an acl create commits with
its bootstrap access row or not at all.

The protocol is sequential by nature (lock → judge → image → mutate), so
rather than colouring it async it runs on a `spawn_blocking` thread with
each query bridged back to the runtime — `block_on` is sound there and
nowhere else, which is why `Executor::write_as` is the only constructor
of the bridge. The psql path keeps its own BEGIN/COMMIT wrapper so the
conformance suites are unchanged, but only the transactional path can
claim the atomicity.

## Pagination — keyset over the language's own total order (2026-08-14)

The API layer's cursor machinery was complete (opaque, MAC'd, bound to
space/op/principal); what was missing was an executor that seeks. It now
does, by **keyset**, never OFFSET: every Ordo plan already carries a TOTAL
order (declared keys, then the tiebreak, NULLS LAST throughout), which is
precisely what makes "strictly after this row" well-defined and
index-friendly, and what stops a paging client from skipping or repeating
rows when the table changes underneath it. OFFSET has neither property.

Two details the language forced:

- **Sort keys need not be projected** (ruling W047), so a key the
  projection does not carry rides as a hidden trailing column. The cursor
  is minted from it server-side; the client's output shape never mentions
  it. `plan_paged` and the executor derive that decision from one shared
  function rather than agreeing by convention.
- **The seek predicate is spelled out**, not a row-value comparison:
  `(a, b) > (x, y)` is wrong as soon as two keys disagree in direction, and
  wrong again for NULLs. With NULLS LAST in both directions, "after" for a
  NULL key value is FALSE (nothing follows the last position) and equality
  is `IS NULL`.

A cursor is minted only when the page came back FULL — the alternative is
a second query per page to say "no more". Page size is deployment-wide,
not a caller's choice: page size is a cost decision and the cost gate is
the platform's. Conformance walks projected, descending, unprojected, and
nullable key orders and demands the pages reassemble the unpaged answer
exactly; over HTTP it also proves the token is opaque and refuses to seek
for a different principal.

## Stage 8 — ops truth: the manifest as derived data (2026-08-14)

`ordo_host::manifest(op)` and `catalog()` publish what the platform knows
about an op: kind and derived write mode, cost class and row bound (from
the same classifier the registration gate enforces), params, output shape,
the entities the whole reachable body touches — **through composition**,
so a caller sees the entity a callee sourced and they never typed — and
the effective guard set with **provenance**: own, `inherited from <entity>`
(rendered rebased through the owning path), or `acl-derived`.

Nothing here is restated by hand. Every field comes from the artifacts the
executor runs, which is what makes manifest/behaviour drift impossible
rather than merely discouraged — the disclosure that "a guard that inlines
but does not disclose is half-built" (engine-plan, Track P) now has the
surface it was written for.

R-G8's disclosure closes with it: an `unguarded` op publishes
`bypassed_guards` naming each guard it strips, so the flag never travels
alone in the catalog any more than it does in the source.

## Adversarial sweep of stages 3c–8 (2026-08-15)

Two independent reviewers attacked the guard/write path and the paging
machinery with instructions to break them. Four real defects, all fixed
with a regression apiece; several areas came back genuinely clean.

### Authorization — two escalations, both by DESIGN rather than plumbing

**Re-pointing a ref walked rows across the boundary.** The write guard was
proven against the *pre-image* only, so a caller could move a row they
legitimately controlled into a parent they did not: a project member
re-pointed into someone else's project carried its member with it, and an
acl access row re-pointed onto another board made its holder an admin
there. Proven live, both of them.

The fix is a rule, not a patch. Re-pointing a **plain** reference needs
only that the target be readable — you are naming someone, not modifying
them. Re-pointing an **owned** ref needs the caller to be able to WRITE
the target, because moving a child into a parent changes what that parent
contains. A post-image guard obligation runs beside it as defence in
depth, but the target check is the load-bearing one: the acl case is
*self-satisfying*, so a post-image check alone passes because of the very
row that was just moved.

**`create_exempt` swallowed satisfiable disjuncts.** The test flattened
the guard's disjuncts and treated a bare bool as exempt-compatible, so
`shares(member == $caller) or open` looked create-exempt though its second
disjunct is an independent, perfectly satisfiable authorization path. Now
a guard is exempt only when EVERY disjunct contains a reverse step rooted
at the row being created — the property that actually makes it
unsatisfiable before the row exists.

Also corrected: the create parent-visibility check keyed off a **heuristic**
(does the string contain `_`, is it under 40 characters) rather than the
field's declared type, so a caller could opt out of it by choosing an id
that did not look like one. Refs are refs because the schema says so.

### Paging — two row losses, both proven

**A union's order was total only up to row identity.** `$arm` plus the
outputs cannot distinguish two identical rows in the same arm, and a
keyset cursor is *strictly* after its key, so paging dropped every
duplicate past the first — seven rows became five on the stock fixture.
Each arm now carries `$uid`, a row number over its own ordering. Which
physical row of a run gets which number is not stable across queries and
does not need to be: the rows are byte-identical, so any assignment yields
the same values in the same quantity. What it buys is the per-row-unique
final key keyset paging actually requires.

**A repeated sort key shifted every hidden key column after it.**
`order_terms` de-duplicates, but `plan_paged` then re-derived each hidden
column's expression by index into the *un*-deduplicated key list, so
`sort status, status` replaced the anchor-id tiebreak with a copy of an
earlier key — twelve rows became three. The expression now travels with
its term instead of being re-derived.

Two smaller ones: a keyless `reduce` (the one legitimately empty tiebreak)
failed at the door because every read routes through paging, and now
answers whole; and a plan whose own `take n` bounded the result no longer
costs an extra empty request to discover it is finished.

### Honesty repairs

The conformance harness's psql entry point wrapped writes in
`BEGIN`/`COMMIT` — through *separate psql processes*, so the transaction
ended the instant each exited and the `FOR UPDATE` locks released with it.
That is now stated rather than pretended: atomicity lives on the executor
path, which is where AA-004 was closed and where the suite proves it.
`standard_conforming_strings` is pinned on both connection paths, so quote
escaping cannot depend on a server GUC.

### Attacked and clean

The seek predicate's NULL logic in both directions; type coercion across
all eight scalar types (with hostile text, quotes, unicode and extreme
dates); hidden key columns never reaching a `DISTINCT`/`GROUP BY` or the
client; guards surviving paging (cursors spliced between principals
*below* the MAC returned nothing outside the caller's own view); the
addressed-vs-bulk mode derivation (`id == $t and 1 == 1` falls to bulk and
bulk intersects the write view, so it escalates nothing); `unguarded`
never leaking through composition; and the bulk `IN (…)` list, which is
built from ids read back from the database rather than from caller text.

## F4 completes — sealed reads, host-side (2026-08-23)

Sealed values can now be **projected**. The envelope leaves the database
as bytes and the host opens it under the entity and field it was sealed
with, so a payload that arrived from another column refuses rather than
decrypting into the wrong name. A failed open is a hard error, never a
null: an empty sealed value reads as "no data" when it means "we could not
open your data".

Every other position still refuses. The database cannot compare, sort,
group, or compute from a value it cannot read, and doing that work
host-side would mean EXAMINING rows the caller never asked for. Equality
on a `sealed indexed` field remains the exception, answered in SQL by the
blind index — and the digest column is never itself projectable, because
serving it would hand out a stable equality oracle.

`[*]` still yields the open skeleton. Naming the field is how a caller
opts into the cost, which keeps the disclosure a choice rather than a
default.

**The venue gate is live, and this is what it was ledgered for.** DL ~1816
pins that `take` bounds RETURNED rows, not EXAMINED ones. A plan that
opens payloads does work per row the database hands over, so the
acknowledgment is keyed on the **venue class**: `unbounded-host` demands
`unbounded` however small the row bound is, while `bounded-host` (a plan
whose own limit bounds the payload count) does not. The self-bounded skip
in the row gate can no longer generalize to the shape DL rejects.

One hazard the work exposed: the conformance harness decoded rows without
opening them, so a sealed projection would have rendered `\x01dd91…` in a
`memo` column — ciphertext served as if it were the value, which is
precisely what the old blanket refusal existed to prevent. The harness now
opens on every read path it has.

## F5 completes; F6's core lands (2026-08-23)

**AA-005 fencing.** A claim takes a generation; a takeover bumps it;
completion is conditional on the generation the holder claimed. A
superseded holder cannot finish, finds out, and rolls back rather than
committing work nobody could attribute under a key someone else now owns.

**Triggers fire only on commit, and it needed no machinery.** The enqueue
happens inside the write transaction, so an aborted write un-enqueues
itself, and `NOTIFY` — delivered at commit, discarded on rollback — agrees
by construction rather than by protocol. No outbox, no delivery
bookkeeping. The queue claims with `FOR UPDATE SKIP LOCKED`, so replicas
compete without a broker and a dead worker's claim returns to the queue
instead of stranding.

**Clock leadership and fires-once stayed separate**, as the platform's own
review insisted. An advisory lock bounds who tries; the schedule row's own
claim-and-advance in one transaction is what makes firing exactly once. A
crash before firing leaves the work owed, a crash after leaves it
advanced, and no ordering fires twice.

**Shape verification is the load gate now.** Every registered op's planned
SQL is described against an empty shadow schema, and the compiler's
inferred shape must match what Postgres reports. The comparison is a type
mapping, not string equality, so it catches divergence rather than
spelling. Zero rows are read, which is what makes it affordable on every
boot and hot swap.

**The catalog is scoped to the asker.** It runs the door's own check per
op and omits rather than marks — a listed-but-forbidden op still discloses
that it exists. A capability token's catalog is exactly its allowlist.

### Hazards the tests found, worth recording

- `CREATE TABLE IF NOT EXISTS` takes a lock even as a no-op, so running it
  inside a transaction deadlocks every other transaction running the same
  guard. Schema setup belongs at boot, not on the hot path.
- That guard is also not race-free: replicas booting together can both
  pass the existence check and one then fails on the catalog's unique
  index. Boot retries once and re-checks.
- The pool reported `db error` for every failure, because tokio-postgres
  keeps the server's message in the error's `source`. Swallowing it turned
  every diagnosis into a shrug.
- `CREATE OR REPLACE VIEW` may not change a view's column list, so the
  shape probe drops and recreates.

---

## The parse/bind boundary, and error classes at it (2026-08-30)

Checklist 9r, 10f, 9p. `ParseError` was `{ message, span }` — no class —
while both conformance suites open with the same sentence: *the harness
asserts the class, not the message text*. With nothing to assert, every
reject case matched message TEXT, so a reworded message silently broke an
assertion and a wrong-but-similar message kept one green. Two of the
guards sweep's eleven divergences were exactly that failure, in opposite
directions.

### The ruling: shape is the parser's, meaning is the guard grammar's

The line is **not** "which pass noticed". A class names what is wrong with
the input; the compiler noticing it earlier is an implementation detail,
and a class that moves when a lookahead moves is not a contract.

- **The parser owns the SHAPE of an `access` clause**: which of the two
  forms it is (the reserved word `open`, or a walk), and whether the walk
  is a well-formed chain of path / collection-step / `==` atoms joined by
  `and`/`or`. A failure here is `parse_error` — the parser can say what it
  expected and nothing about what the author meant.
- **R-G3 owns what a well-formed shape is allowed to MEAN**: walk steps
  over declared refs, `==` against `$caller` or a literal, bare `bool`
  fields, at least one `$caller`-referencing disjunct. A violation is
  `invalid_guard_walk` **wherever it is noticed** — mostly in
  `ordo-schema`, once the walk resolves against the graph; in the parser
  where the grammar alone can decide it.

`ParseError` therefore carries a class, drawn from the closed inventories
and never invented: `parse_error`, `invalid_guard_walk`, and
`bare_field_in_group` (which had been smuggled into the message text as
its first word, and matched there).

**GC077** — `length(title) > 3`. The *document* was right; the tree was
wrong. The atom is readable — a path, a parenthesised step, a comparison —
and what it says is what R-G3 excludes, so the walk can be named:
`invalid_guard_walk`.

**GC100 (partial-open)** — `… or open`. The *document* was right; the tree
was wrong. Choosing between `open` and a walk is a shape decision taken
before any walk is read, so `open` is a keyword inside an `access` clause
and cannot be an operand: `parse_error`, exactly as the withdrawn
`access write true` already was. The old answer — the schema builder's
"`announcements3` has no field `open`" — was a message about a field the
author never meant, and one that would have gone on being right the day
somebody declared a `bool` called `open`.

The two only looked like they pulled in opposite directions. Under
shape-versus-meaning they are the same rule applied twice.

**GC088** — a projection of a missing field. The *document* was right. The
class of a miss is decided by the POSITION that named the field, not by
the lookup that failed: `projection_unknown_field` for a projection,
`not_output_field` for a condition, group/take key or sort key. The
binder's `unknown_field` is internal vocabulary that appears in **no**
closed inventory, so it must never be what a conformance case sees.

### Creates through multi-hop inherited guards (9p)

Guard inheritance walks owned refs to a fixed point, and `rebase` records
the hops. Reads always honoured the whole chain; creates refused past the
first hop, which made a grandchild under a guarded root **uncreatable** —
not forbidden to some callers, uncreatable by everyone including the
root's administrator. A refusal nobody can satisfy is not a guard.

The generalisation turns on one asymmetry. `rebase[0]` is a ref on the row
being created, and that row does not exist, so hop zero cannot be *read* —
it can only be the value the caller **supplied**. Every hop above it is an
ordinary ref chase off the ancestor the previous hop pinned. So the create
gate is: pin hop zero by the supplied literal, nest one `EXISTS` per hop
after it, evaluate the ancestor's walk at the far end, all under the
caller's own `members` row. A supplied parent that does not exist (or
lives in another space) makes the outermost `EXISTS` empty, so the guard
fails closed rather than opening.

### Left ambiguous, and not invented here

- ~~**Parenthesised guard groups.**~~ **BUILT (9e).** The entry read: R-G3
  admits parens when mixing `and`/`or`; the parser refuses them ("not in
  this slice"); that is a legal construct that is unbuilt, and neither
  inventory has a class meaning "legal but unimplemented" at parse time,
  so it is `parse_error` today — the wrong shape of answer for the right
  refusal. The ambiguity is resolved by building it rather than by naming
  a class. **A group is distributed**, so `GuardExpr` stays in
  disjunctive normal form and "a disjunct" keeps meaning *one independent
  authorization path* — the unit G2, the create-exemption rule and R-G6's
  semi-join union are all stated over. Distribution is sound because the
  guard grammar has no `not`. Two engine-pinned consequences, recorded
  here because neither is in the spec: the normalized form is capped at
  64 paths (`parse_error`, by this document's own `take count out of
  range` ruling — a limit on what the compiler can represent, not on what
  a guard may mean), and the guard parser's silent precedence table
  (`a and b or c` read as `(a and b) or c`) is gone, since principle 2
  has no exception for guards.
- **`unknown_field`.** In no closed inventory. `not_output_field` is
  defined only for a *callee's output shape*, so a condition or sort key
  naming a field an ENTITY does not have has no inventory class at all.
  Confined to non-projection positions here rather than renamed.
- **`take count out of range`.** A literal outside its representable
  range is not "text outside the closed grammar", and `bound_violation` is
  a runtime wire class. Left as `parse_error`.

---

## Five places a document and the compiler disagreed (2026-08-30)

Checklist 9e′ and 10g. Five constructs, one question in five costumes:
*is the document wrong, or is the compiler wrong?* The 9r/10f discipline
applies — decide on the merits, then make the **losing side** match, and
never split the difference by weakening both. A document fix is a real
fix: three of these five are closed by editing a document, and each of
those closes as completely as a code change would have.

Outcome: the compiler wins three, the document wins one, and one is
neither side's fault.

### 9e′ · Three constructs the block-parse harness found

**1 · `list of { … }` parameter types — NEITHER side is wrong. The
allow-list was.**

`definition-language.md` §1 marks list params and `each` **[lean]**, in
those words, and the two `syntax-examples.md` blocks that use them mark
themselves `# [lean]` in their own comments. So the parser refusing the
construct is the parser being right, and the document showing it is the
document being right — it is publishing a sketch and saying that is what
it is. What was wrong is the block harness, which filed both blocks under
*"settled features with no parser yet"* and thereby promoted an unsettled
sketch into a work item.

**A [lean] construct is not owed.** The allow-list now carries three
categories rather than two — settled-and-unbuilt (a debt, 9e's), [lean]-
and-not-owed (nothing is owed), not-Ordo-source — because they are owed
differently and a list that cannot tell the first two apart manufactures
work. This is 9c's `allocate` finding arrived at from the other end: 9c
declined to build the distribution algorithm because its only legal home
is `each` over a list parameter and building that home would mean
inventing the shape the spec declined to fix. Same construct, same answer.

**2 · `let p = set projects { … }` — the DOCUMENT wins; the debt is the
parser's, and it is 9e's.**

DL §3 *"Naming a freshly created row in the same op"* is headed
**[settled]**; R-G19 restates it; `traps.md`, `modeling-guidance.md`,
`tutorial.md` and `sql-translation-corpus-2.md` all spell it. There is no
write-valued `let` production at all — `LetDecl` binds a read `Pipeline` —
which is why `unbindable_write` sits in the closed inventory as
"unreachable, and the feature it guards is unbuilt".

This is a **debt, not a divergence**, so it moves onto 9e's list of
settled-but-unparseable constructs where the parser's other arrears live,
and 9e stays open. Deliberately **not** built here: a production that
parsed `let x = set …` without the write-mode derivation behind it would
make `unbindable_write` raisable in name only, and an author would learn
that a converge cannot be bound somewhere far downstream of the line that
bound it — a worse answer than today's honest parse error.

**3 · A projection wrapped onto the next line — G6 wins.
`syntax-examples.md` §expiringSoon is fixed.**

```
leases(status == "active" and end is not null and end <= 2026-12-31)
  [start, end, rent, unit.name, unit.property.name]      # ✗ not in the grammar
```

The mechanical argument is **not available** to either side: no step in
the language begins with `[`, so a parser could decide this with one token
of lookahead and no ambiguity. The design argument decides it instead.

Delete the wrapped line and what remains is **not** an error — it is a
complete step with a **different meaning**: `leases(…)` with no projection
reads the whole row. So the continuation would let the next line silently
*narrow what the line above it disclosed*, and in this language a
projection is exactly how a caller opts into cost and disclosure (`[*]`
yields the open skeleton; naming a sealed field is how you accept its
price). G6's "a step ends at end of line" is not a parsing convenience —
it is the promise that a step's meaning is decidable from its own line.
Relaxing it buys one prettier example and spends that promise everywhere.

The example now breaks **inside the parens**, which G6 explicitly permits,
so the block demonstrates the rule instead of violating it. That is the
general repair, and it is written into the block: break inside a
delimiter, never between a source and its projection.

### 10g · Two corpus texts that did not compile as written

Both share one shape. The expected class was **right**; the source carried
a **second, unrelated defect** that an earlier gate answered first, so the
case never reached the gate it was written to test. Neither expectation
moved. Both texts did.

**4 · GC033 — the COMPILER wins.**

`op leak unguarded = openTasks[title]` expects `unguarded_superfluous`.
`openTasks` is declared `unbounded` in §3.4 and composition-R9 puts the
acknowledgment on the **caller**, so the op is refused
`unacknowledged_unbounded` at the registration gate before that gate asks
what it bypasses. Nothing about the ordering is wrong: an op with two
registration defects gets one class, and no rule ranks a superfluous flag
above an unacknowledged cost. The text predates R9. `unbounded` added; the
expected class unchanged.

**The finding, worth more than the ruling: GC034 and GC035 have exactly
the same defect and only GC033 was ever recorded.** The suite had quietly
supplied the missing marker for the other two and said nothing — one
divergence noticed, two identical ones absorbed. All three are corrected,
and the gate ordering is now held by an assertion (`op leak unguarded = …`
→ `unacknowledged_unbounded`) rather than by prose, so the corrected text
cannot drift back into looking unnecessary.

**5 · GC074–GC078 — the COMPILER wins, and R-G21 does say what the
checklist claims, with one correction to the attribution.**

Verified rather than taken on trust. The **rule** the corpus text violates
is spec **G9** — an `access` declaration is *both verbs or neither*, so
`access read` alone is `incomplete_access_clause` at the declaration, not
a permissive default. The sentence prescribing the **repair** — *"the
symmetric case (`access write` with no `access read`) is the same error
and the same fix (`access read open`)"* — lives in G9's own paragraph in
`definition-language.md` and is restated verbatim in R-G21's territory at
GC100. R-G21 proper pins the spelling and the keyword-ness of `open`. So
the checklist's reading holds: G9 condemns the text, R-G21 supplies the
words, and the corpus is the stale side.

All five entities declare `access read` alone, so all five are refused
before the walk is examined — including GC077, whose `invalid_guard_walk`
had been promoted out of `DIVERGENT` days earlier and was, until now,
being asserted against a text that could not reach it. Each case now
spells `access write open`, which adds nothing to what it asserts. GC079
needs no repair: `access admin` fails at the verb, before completeness is
asked. A sweep for the same shape across every doc found no other
half-guarded entity except two labelled *does not compile* on purpose.

### What the five have in common

Rulings **4 and 5 are one shape applied twice** — a stale corpus text
whose expected class is right and whose source has an unrelated second
defect that a gate answers first. The lesson is procedural: when a case
does not reach its own assertion, suspect the case's *other* line before
suspecting the class.

Rulings **1 and 2 are the two halves of one question the harness had
collapsed** — *is this construct owed?* **[lean]** means no and
**[settled]** means yes, and until now the allow-list could not say which
it was holding. Ruling 3 is the only one of the five where the merits
could plausibly have gone the other way, and the thing that settles it is
not the parser's capability but what a reader is entitled to conclude from
one line.

## P0's last two gaps: text collation and `hash(x)` (2026-08-30)

Checklist 9t. P0's only deliverable is rulings, and engine-plan **G11** and
**G12** carried ⚠ "needs a spec ruling" — G11 with a live code consequence
(9i emitted `COLLATE "C"` everywhere), G12 with a live code consequence in
the other direction (9f″ REMOVED `hash()` from the language). Both are
ruled below. The ⚠ marks come off.

The shape of both rulings is the same, and it is worth naming before either
one: **a determinism promise is a promise not to own the thing you are
promising about.** An order or a digest you can change is an order or a
digest that will change; the way to hold "the same answer on every
deployment, forever" is to define it by something external and frozen, and
then to say plainly what that costs.

---

### G11 · Text ordering — **RULED**: codepoint order over NFC, and it is the language's order, not the engine's

**The order.** Ordo sorts text by **Unicode codepoint order** over the
stored value's NFC form. `C` collation over UTF-8 is exactly that, which is
why `ordo_plan::TEXT_COLLATION` is right — but the ruling is not "because
the engine emits `C`". It is the other way round: codepoint order is the
only order the *language* can define, because it is the only one that is a
function of the value alone. Every alternative is a function of the value
**and** a table that ships with somebody else's software, on a release
cadence the platform does not control. glibc has changed `en_US.UTF-8`'s
ordering across releases; ICU changes weights with CLDR. A language whose
answers must be reproducible across deployments and across time cannot put
such a table inside its semantics — and keyset pagination makes the
consequence concrete rather than theoretical, because a cursor means
"strictly after this row **in the plan's order**", so an order that changes
under a package upgrade silently skips or repeats rows on the next page.

**What it costs, stated plainly because it is user-visible.** `C` puts `Z`
before `a`. So `sort name` over human names returns `Zoë`, `Åsa`, `ana` in
that order, and that reads wrong to every author who has not read this
paragraph. This is a **commitment**, not an implementation detail: Ordo
does not sort text the way a phone book does, and by default it never will.
The trade is determinism over legibility, and the reason it is the right
trade at this layer is that legibility is a property of the *reader* — it
depends on a locale the server does not know and the op does not carry —
while determinism is a property of the *answer*, which is the thing an op
is for.

**Three questions G11 said the spec still owed. All three, answered:**

1. **Is the language's order codepoint order at all?** Yes, ruled above.
   Not "the engine's choice pending something better" — the language's
   order, for the reason above.

2. **Where does NFC normalization happen?** *This was never an open
   question.* `definition-language.md` §2, "Text canonicalization
   [settled]", already says: **stored text is Unicode-normalized (NFC)
   platform-wide**, on write. So the language's order has always been
   codepoint order over NFC; the two halves were in two documents and
   nobody put them together. What G11 actually found is therefore not a gap
   in the spec but a **gap in the engine**: nothing normalizes anything
   today. `é` (U+00E9) and `e` + U+0301 are the same text by DL's settled
   rule, and they sort apart, compare unequal, and can both occupy a
   `unique` column. `ordo-commandlog` even documents its strings as
   "NFC-normalized upstream" — there is no upstream doing it. That is now
   recorded as owed *engine* work (checklist 9u) rather than as an open
   ruling, and it is the more serious half of G11: `COLLATE "C"` makes the
   order deterministic *given the bytes*, and NFC-on-write is what makes
   the bytes deterministic *given the text*.

3. **May an author ask for a locale-aware order per sort key?** **No, and
   not later either.** A `sort name collate "en_US"` would put an external
   versioned table back inside the language's semantics — undoing the whole
   ruling — and it would do it *per caller*, so two callers of one
   registered op would get two orders and one caller's cursor would not
   seek in the other's. It also breaks the cost model: the plan would need
   one `COLLATE` expression index per (key, locale), so an op's cost class
   would depend on which locale a caller passed, and cost classes are fixed
   at registration by design.

   The refusal has a real cost, and pretending otherwise would be
   dishonest: a client can re-sort a *page* into a human order, but it
   cannot re-sort page 3 of 40 — a paginated list in a legible order is
   genuinely not expressible by sorting at the client. The spelling that
   stays inside the language is **data**: an author who needs a phone-book
   order stores the fold they want in an ordinary `text` field
   (`sortName`) and sorts by that. It is deterministic because it is a
   value; it is reviewable because it is in the schema; it indexes like any
   other column; and the locale decision is made once, by the author, at
   write time, instead of once per caller at read time. Whether the
   platform should offer a `normalize(...)` form that maintains such a
   field is **[lean]**, not ruled here.

**Two consequences of the ruling the engine already implements, now blessed
rather than merely pinned:**

- **Equality is not collated.** `=`, `DISTINCT`, `GROUP BY`, `UNIQUE` are
  byte equality under every *deterministic* collation, and Postgres refuses
  a nondeterministic one as a database default, so grouping and
  de-duplication do not depend on the pin at all.
- **The collation belongs to the ORDERING, not to the column.** Nothing in
  the generated DDL is collated, and that is correct rather than merely
  convenient: pinning a *column* to `C` also repoints `lower()`/`upper()`
  at `C`'s ctype, which stops folding non-ASCII case and would trade a
  deterministic order for a wrong `normalize(lower)`. The price is that a
  default-collation btree cannot serve `ORDER BY x COLLATE "C"`; the fix is
  a `COLLATE "C"` expression index per sort key, and that is owed work, not
  a reason to move the pin into the columns.

**Did the engine pick wrong?** No — and the check was worth running, because
the engine picked `C` for an *engine* reason ("the collation every Postgres
installation has, no ICU build required") and that reason does not survive
contact with a language ruling on its own. What makes `C` right is not its
availability; it is that codepoint order is the only order definable
without importing someone else's release cycle. The availability argument
is a happy accident that makes the right answer also the cheap one.

---

### G12 · `hash(x)`'s algorithm — **RULED**: SHA-256, truncated; the SipHash pin is **withdrawn**

DL §3 names `hash(x)` — "stable 64-bit non-cryptographic hash → int, for
bucketing/rollout striping; **stability across platform versions is a
compatibility promise**" — and left the algorithm unruled, which is why 9f″
correctly removed the function rather than shipping a promise it could not
keep.

**The ruling.** `hash(x)` is:

> the **SHA-256** digest of the value's **UTF-8 bytes**, of which the
> **first eight bytes, big-endian**, are taken as a signed 64-bit integer
> with the **top bit cleared**. `null` hashes to `null`.

Reference implementation and lowering live together in
`ordo_plan::hash64` / `hash64_sql`, and conformance runs both against a
real Postgres over the same inputs
(`hash_is_bit_stable_and_agrees_across_venues`), because two
implementations of one rule is the shape that drifts.

**Why this algorithm can be held stable across versions.** Because it is
not ours. SHA-256 is an external frozen standard (FIPS 180-4) with
published test vectors, implemented identically by Postgres core —
`sha256(bytea)`, in core since 11, **no extension** — and by every host
language. "Stable" here means "we did not invent it and therefore cannot
drift from it", which is the only kind of stability a promise like this can
rest on. Two alternatives were considered and rejected:

- **Postgres's own `hashtext`/`hashint8`** are explicitly documented as not
  stable across server versions. Shipping one would make the compatibility
  promise and break it in the same commit — 9f″'s finding, unchanged.
- **`md5()`** is also in core and also externally specified, and its
  cryptographic weakness genuinely does not matter for bucketing. It loses
  on availability: under an OpenSSL FIPS provider Postgres's `md5()` fails
  outright, so the function would be absent on exactly the deployments that
  care most about being predictable.

**Why the engine's SipHash-2-4 pin is withdrawn.** Two independent reasons,
and the second is the interesting one.

1. **It is not in Postgres.** `hash(x)` appears in conditions and
   projections, so the *database* has to compute it. SipHash-2-4 in core
   Postgres does not exist; it would mean an extension the operator must
   install, or a PL/pgSQL transcription. That contradicts the platform's
   own stance three sections up, where `C` was chosen over ICU precisely
   because it "needs no ICU build".
2. **The key was ceremony.** SipHash is keyed to defend hash tables against
   collision flooding. There is no hash table here, and the pin specified a
   *fixed published* key — which removes the only property the key provides
   while adding the key itself to the compatibility surface. A key that
   must be published in order to be stable buys nothing and can be got
   wrong. (A *secret* per-deployment key would buy something — an outsider
   could not compute which bucket a given id lands in — but it would
   destroy cross-deployment determinism, which is the promise. So:
   unkeyed.)

**The three details that are ours, and are exactly where a second
implementation goes wrong:**

- **Which bytes, and the sign.** First eight, big-endian, top bit cleared.
  Ordo's `int` is signed 64-bit, and `mod` follows the dividend's sign (DL
  §3), so an untruncated digest makes `mod(hash(id), 100)` land in
  `(-100, 100)`: every rollout stripe an author writes would quietly be
  half the width they intended, over a randomly chosen half of the
  population. Masking costs one bit of a hash used for bucketing and
  removes a trap the obvious use walks straight into. `abs()` is not the
  fix — `abs(i64::MIN)` overflows.
- **The input is UTF-8 text, converted and not cast.** The lowering uses
  `convert_to(x, 'UTF8')`, **never** `x::bytea`. The cast goes through
  `byteain`, so a value beginning `\x` is reinterpreted as a hex escape and
  the six-character string `\x4142` hashes as the two bytes `AB` — two
  distinct ids colliding into one bucket, from the tidier-looking spelling.
  Asserted in conformance.
- **The domain is `text` and `id`.** A bit-stability promise is a promise
  about *bytes*, and those are the two types whose bytes the language pins
  (UTF-8, NFC by G11). Promising stability over the rendering of a `date`
  or a `money` would mean freezing a rendering that is written down
  nowhere, which is how such promises actually get broken. Bucketing and
  rollout striping key on an id, which is what the function is for.

**What it costs to change later, and therefore what the freeze means.** The
value is baked into consequences the platform cannot see: cohorts, stripes,
bucket columns an author computed once and stored. Changing the algorithm
reshuffles every one of them silently — a 5% rollout stays 5% and becomes a
*different* 5%, with no error anywhere. So `hash` is **frozen at first
release and may only ever be added to**: a future need for a different
digest is a differently-named function, never a new meaning for this one.
That is the whole cost of the promise, and it is why the algorithm had to
be ruled before the function shipped rather than after.

**`hash()` is restored.** The parser table entry, the `int` result type and
the lowering land with this ruling, as the checklist's 9f″ entry said they
would. Two notes on what did *not* land with it:

- **The domain is not enforced by the compiler**, because the compiler
  type-checks **no** expression function's arguments — `lower(price)` and
  `length(created)` are equally unchecked — and no closed inventory carries
  a class meaning "argument of the wrong type to an expression function".
  What catches `hash(price)` is F6's shape verification at registration,
  which is what catches `lower(price)` too: the op fails to load rather
  than failing a caller. Inventing a class for one function would have been
  a producer with no inventory. The general gap is recorded in the
  checklist (9v) as its own finding.
- **`hash(x)` cannot carry an expression index as written**, because
  `convert_to` is STABLE rather than IMMUTABLE (its result depends on the
  server encoding). An IMMUTABLE platform-owned SQL wrapper is sound — the
  platform requires a UTF-8 database — and is owed work, in the same drawer
  as G11's owed `COLLATE "C"` expression indexes.

## P0's last gap: the arithmetic class (2026-08-30)

Checklist 9w, carried out of 9t where it was a footnote to a correction
and easy to lose. `definition-language.md` §2 settles the *behaviour*
twice — "`/ 0` is a typed runtime error, uniform with `mod(a, 0)`.
Division is total on its domain and loud off it", and "Overflow on
`int`/`money` is a typed runtime error, never wraparound" — and names no
wire class for either. `api-engine-design.md`'s closed inventory named
neither. 9c pinned `arithmetic_error` (400) in the engine pending this.

The mechanism was already whole and is unchanged by this ruling: SQLSTATE
22012/22003 → `PoolError::Arithmetic` → `classify` → `ErrorCode::
ArithmeticError` → 400, with both failure modes reached against a real
Postgres. What was missing was the document's blessing, and the class's
absence from the closed inventory — which is the part that mattered, since
an inventory that does not name a class the engine emits is not closed.

### RULED: one class, `arithmetic_error`, 400, for both

**One class, not two.** DL §2 gives `/ 0`, `mod(a, 0)` and overflow one
description — "a typed runtime error" — and the inventory does not mint
classes a caller cannot act on differently. There is no repair for an
overflow that is not also the repair for a division by zero: ask a
different question. The *message* still names which occurred, in the
server's own words, because the caller already knows the expression they
asked for and withholding it buys nothing.

**400, and the reason is not fault.** The tempting split is by provenance
— a caller-supplied divisor is the caller's mistake (400), an overflow
accumulated out of stored rows is not (500) — and the inventory does
reason this way elsewhere: `null_key_address` is *"request-shaped, so 400:
no stored data decided it"*, and `sealed_open_failed` is 500 because *"the
caller can do nothing about it"*. So the split has real precedent and has
to be refused on its merits rather than waved away.

It is refused because **the platform cannot tell the two apart.** Postgres
reports 22012 with no account of whether the zero came from a literal, a
bound parameter, or a column; to split, the engine would have to
re-derive, from a plan that has already failed, the provenance of every
operand feeding the operation — an analysis it does not have and would be
wrong about in exactly the interesting cases (a parameter that was
previously *written* into the row now being read back, a column whose
value this caller set a moment ago). A status code that is sometimes
wrong about fault is worse than one that is consistently modest.

What 400 asserts here is not "you erred" but **"this request, as posed,
has no answer — pose a different one"**, which is true in both cases and
is actionable by the only party who can act. 500 would page an operator
for a data condition that is not a defect, and re-create the failure mode
9c fixed: the arithmetic rules that ARE built becoming indistinguishable
from a server bug.

**Not retryable.** `is_exec_conflict()` is false: nothing was applied, and
a retry over unchanged data fails identically. It is not a conflict class
and must never be auto-retried.

### The disclosure question, stated at its real size

A secondary argument for a single code is that a varying one would be a
one-bit channel about data the caller cannot read. That argument is
**weaker than it first looks and is not what this ruling rests on**: a
read's guard is inside the statement, so an aggregate only ever sums rows
the caller may read, and the same holds for a division over a projected
column. The channel would need arithmetic over a value the caller cannot
read, which the built surface does not currently offer.

What *does* survive, and is worth stating so nobody re-derives it as a
finding: the **existence** of the error is a signal, and no choice of
class or code removes it. A caller who asks for a sum and gets a refusal
instead of a number has learned that the sum has no representable answer.
That is inherent in answering at all, not something the inventory can
close, and it is bounded by the read view like every other answer.

### P0

With G11, G12 and this, P0's rulings are complete. G16's two tracks remain
open, but those need owners and scheduling, not rulings.

## Expression-function argument types (2026-08-30)

Checklist 9v, found while restoring `hash()` under G12. `lower(price)`,
`length(created)`, `trim(count)` and `hash(price)` all bound, planned, and
emitted SQL Postgres would refuse.

### RULED: the closed inventory gains `argument_type_mismatch`

Adding to a closed set is supposed to cost a written justification, so:
the compile inventory's **`param_type_mismatch`** is *"binding expression's
type ≠ declared parameter type"* — a **caller binding a value into a
callee's declared parameter**, which is composition. **`invalid_binding_expr`**
is a **write's right-hand side**. Applying a function to a value is neither,
and stretching either name would make it stop predicting where to look —
which is the only thing a class name is for.

### Why catch it at bind when the load gate already did

It genuinely did: F6's shape verification poses each registered op to the
database and fails the boot when the row type disagrees, so none of these
ever reached a caller. That is why `hash()` was restorable without
inventing anything, and the gate is not weakened here.

But it was **decent by accident**. What the operator saw was *"the database
refused to describe this op's plan"* — true, and about a statement rather
than about an author applying `lower` to a money. Worse, load is the wrong
moment: the op that fails to boot may be one nobody has touched in months,
while the edit that broke it was somewhere else entirely. Binding is where
the author's line is still in hand.

### The domains, and the one that was wrong first

Temporal parts (`year`, `month`, `week`, `day`, `hour`, `dow`) take
`date`/`datetime`. `abs`, `round`, `floor`, `ceil` take the whole numeric
tower, because `decimal` is expression-only and flows into `int`/`money` by
DL §2 — widening inside the tower is not a mistake. `lower`, `trim`,
`length` take `text`. `hash` takes `text`/`id`, which **G12 pinned
explicitly**: bit-stability is a promise about bytes, and those are the only
types whose bytes the language fixes.

**`mod` was wrong in the first draft, and the error is worth keeping.** DL
§3 says *"`mod(a, b)` (integer; truncated division, sign follows the
dividend)"*, and reading "integer" as a type excluded `money` — which broke
`decimal.rs`, a test that asks for `mod(amount, qty - 1)`. That sentence
describes the **division's semantics**, not its domain, and `money` **is**
an integer count of minor units in this platform, so `mod(price, 100)` is
the cents part and is meaningful. `decimal` stays out: truncated integer
division over a fractional value is not what the sentence describes.

The general lesson is the one a type checker always owes: **a false positive
is worse than the gap it closes.** A missing check fails an op at load with
an unhelpful message; a wrong check refuses a program that was correct. So
`least`, `greatest` and `coalesce` constrain only **agreement** — one type
in both arguments, a rule about the relationship that a per-position table
cannot express — and the numeric tower counts as agreeing, because
`least(price, 0)` is exactly the comparison the author meant.

---

## `allocate` is two things, and only one of them was unsettled (2026-08-31)

Checklist 9f′. The item read *"`allocate()` still does not exist"*, and two
earlier pieces of work had each concluded it could not be built:

- **9c** declined the distribution algorithm because *"its only legal home is
  `each` over a list parameter, and building that home would mean inventing
  the shape the spec declined to fix."*
- **9e′** ruled that `list of { … }` parameters and `each` are marked
  **`[lean]`** in DL §1 *in those words*, and that **a `[lean]` construct is
  not owed** — the allow-list that filed them as "settled features with no
  parser yet" was itself the bug.

Both are right. Neither answers 9f′, because 9f′ names **two** deliverables
and both prior findings are about the second one.

### RULED: the semantics are owed and buildable; the spelling is not owed

The document says so itself, and the sentence had been read as one clause
when it is two. DL §3's Allocation entry is headed **`[settled]`**, and the
parenthetical that marks the syntax lean reads:

> (`each` over a list parameter is **[lean]**, not settled — §2, Types. **The
> allocation semantics above are settled**; the spelling lands with that
> construct, and if `each` changes shape `allocate` moves with it.)

"The spelling lands with that construct" scopes the `[lean]` mark to the
*site*. The rule is settled on its own, and three documents state it in the
same words (DL §3, `glossary.md`, `traps.md`): floor each exact share, hand
the leftover minor units out one apiece by **largest remainder**, ties by
weight order, and the parts sum to the total by construction. It also has a
**printed value** — `allocate(100.00, (1, 1, 1))` is 33.34 / 33.33 / 33.33 —
which is an executable assertion, not prose.

So the correct outcome is **neither** "build it" nor "close it as not owed".
It is a split, and the reason the split was not seen earlier is instructive:
9c asked *is this buildable?*, got "no" for the site, and stopped — the site
was the last thing it looked at, so the answer for the site became the answer
for the item. 9e′ arrived from the other end and confirmed the same half.
Nobody asked the question 9f′ actually poses, which is whether the *rule*
depends on the *site*. It does not.

### Where each half now lives

`ordo_plan::allocate` holds the rule, next to `hash64` and for the same
stated reason — a reference implementation of something that must not drift
— with one difference worth naming: `hash64` sits beside its lowering, and
`allocate` has none. `crates/ordo-conformance/tests/allocate.rs` pins the
printed example, the sum-to-total guarantee, largest-remainder **as such**,
and host/SQL agreement against a real server. That last one is new work
rather than a restatement: DL breaks ties "by weight order (deterministic, so
both venues agree)" and `glossary.md` names the venues as host and SQL, and
until now there was no second venue to check the claim against.

The site stays refused. DL §3 is exhaustive by construction — "anywhere else
it is `invalid_allocate`" — and the refusal's own claim that no other legal
site exists was **verified rather than trusted**: every expression context in
the tree was mapped, and no path lets a function call reach generated SQL.

### The ambiguity the printed example resolves

**"Ties broken by weight order" means positional order, not a sort by weight
value.** In `allocate(100.00, (1,1,1))` all three remainders tie *and* all
three weights are equal, so the value reading leaves the extra cent's home
undetermined — and the clause exists to be deterministic. Only the positional
reading gives the printed example one answer. Recorded because the phrase
will read ambiguously to the next person too, and the case that settles it is
the same case everyone starts from.

### Three gaps named rather than filled

- **Negative weights** are not typed by any document beyond the example's
  `weight: int`, and "in proportion to" has no evident meaning for one. The
  kernel computes the stated formula — the sum-to-total invariant survives
  either sign — but no refusal was minted, because minting one is a ruling
  and this is not the place for it. It is owed with the surface syntax.
- **`Σw == 0`, the empty weight list included**, is a division by zero and
  therefore DL §2's own rule ("a typed runtime error, uniform with
  `mod(a, 0)`") with 9w's class, `arithmetic_error`. Stated here because the
  documents do not: splitting among nobody has no answer that sums to a
  non-zero total, so it is the ordinary `/ 0` and not a special case.
- **The lowering is not an expression-function lowering.** `allocate` reads
  the whole weight vector to answer for one element, so it is a windowed
  computation over the list relation, not a scalar expression over the
  current row. It can never be rendered by `Planner::function`, whatever
  shape `each` takes; that arm is a backstop, not a slot waiting to be
  filled. Worth pinning, because filling it in is the obvious wrong way to
  finish this item.

### The general lesson

**A checklist entry can name more than one deliverable, and "not buildable"
answers only the one it was asked about.** 9c's and 9e′'s findings were both
correct and both partial, and two correct partial findings in a row look
exactly like a settled closure. The tell was available in the spec the whole
time: a paragraph that marks itself `[settled]` and then marks *part* of
itself `[lean]` is telling you the item has a seam.

## The std library — deriving the half that can be bounded (2026-08-31)

Checklist 9k. `std-library.md` specifies twelve derived verbs per entity;
seven of them now derive (`ordo_schema::std_ops`). The five that do not are
the interesting half, because they fail for four different reasons and only
one of those reasons is "nobody wrote it yet".

The mechanism is `acl_admin_ops`'s, deliberately and without variation:
`OpDecl` nodes built from the schema, `Registry::extend_from` at the same
two call sites, then the same `ordo_plan::cost::check_registration`. A
derivation that needed its own gate would be a derivation that had escaped
one.

### 1. `std.E.list` does not derive, and the gate is what refuses it

**Not a scoping decision — the row-bound prover rejects it, and every way
around the rejection breaks something [settled].**

`std-library.md` §5.7 lists the bound each derivation carries and says
`list` and `under` are bounded *because they are keyset-paged*.
`ordo_plan::cost::row_bound` does not accept that: its evidence is a `take`
with a provable count, a keyless `reduce`, or a unique pin, and nothing
else. So a derived `list` is `Unbounded` and fails registration.

The three ways out, and why each is closed:

- **Acknowledge it.** §5.7 rule 7 forbids std the `unbounded` marker in
  terms — *"a derivation cannot acknowledge on anyone's behalf"* — and it is
  right to: the marker exists so a human ships O(space) work on purpose, and
  a derivation would be taking that decision once per entity, silently.
- **Add a `take`.** This looks obvious and is wrong, for a reason that is a
  fact about this engine rather than about the spec. `plan_paged` wraps the
  bound query and comments that *"the inner query keeps its own ordering and
  any `take` limit; paging ranges over that settled result, never around
  it"* — the seek predicate is applied **outside** the take. So `take n`
  caps the listing at n rows *total*; row n+1 is unreachable by any cursor.
  That would break §1's promise that iterating a refined `list` to
  exhaustion yields the matching set, and would put `list` (capped) and
  `count` (`reduce`-shaped, unbounded in rows examined) permanently out of
  the agreement §2.1 requires of them.
- **Teach `row_bound` that keyset paging is a bound.** This is the honest
  fix and it is **not a derivation's to make**. `row_bound` is one shared,
  security-bearing gate; relaxing it for std relaxes it for every authored
  op in the workspace at the same instant. Whether "the door will page this"
  is a *bound* or merely a *window* is a language ruling, and it wants the
  same owner who ruled that `take` bounds returned rows rather than examined
  ones.

Recorded rather than worked around, because the shape of the mistake — a
document asserting a bound the prover cannot see, and an implementer
quietly adding a `take` to make the red go away — is the one that would
have shipped a silently truncated listing under a spec sentence promising
exhaustion.

**Consequence worth stating:** `count` derives and `list` does not, which
reads backwards. It is what the gate says. `critique-consolidated-4.md`'s
**AA-018** (Critical, open) argues `count` should not pass either —
*"exact count over 100M uncapped rows examines 100M entries"* — with the
ruling line *"examined-stage proof wins"*. That ruling, if taken, retracts
`std.E.count` too. It is not taken here: the rule this derivation follows is
**derive exactly what the registration gate admits**, and the gate admits a
keyless reduce today for authored ops (`specCount`) and derived ones alike.
Deciding otherwise would be pre-empting an owner ruling in the opposite
direction from the one above, using the same authority I declined to use.

### 2. `whole`, `under`, `rollup`, `history` — four different noes

- **`whole` has nowhere to live.** It is not an `OpBody` variant; it is a
  standalone read (`ordo_syntax::parse_whole`) with no spelling inside an
  op. Its row bound also rests on a per-collection cap whose value §10 marks
  `[open]` and explicitly declines to guess, and `AA-019` (Major, open)
  records that §2's `{ items, count, next }` envelope and §5.7's
  `truncated` marker are two incompatible result types for the same op.
- **`under` and `rollup` are `[lean]`, so nothing is owed** — the 9e′
  ruling, applied. `under` would hit `list`'s blocker anyway. `rollup`
  additionally derives only over a *"declared relationship cap"* that no
  version of the language has ever had a syntax for, so it would derive for
  no entity even if built; and its `[settled]`-looking "never associations"
  proof rests on the one-owner rule that `definition-language.md` §Ownership
  **withdrew as false**, with an explicit instruction that it must not
  survive in `std-library.md` either. It has.
- **`history` is platform-derived, not language-compiled** — §2.3 says so —
  and there is no log source to compile. Field list `[lean]`.

### 3. `members` derives nothing, and the spec does not say so

Not in `std-library.md`; taken here. §2's *"derived entities are entities"*
licenses std over the `acl`-derived link entities, which carry the link's
inherited write guard and so cannot undercut `grantBoardsAccess` (asserted).
It says nothing about the platform's own `members`, which is **unguarded**.
Derived over it, `std.members.get`/`count` is a whole-space principal
directory and `std.members.create`/`update`/`delete` is an unattributed path
to minting, re-roling and deleting principals — the thing `std.space.*`
exists to do *as attributed, chained, revertible commands*. The enumeration
oracle everyone watches for in `callersOf` arrives here first and more
cheaply, through 9k rather than 8r.

### 4. Two places `std-library.md` says both things

- **`update` and sealed params.** §2.4's table says write params are
  *"unchanged — sealed fields stay writable"* and, one row later, that
  `update`'s converge braces are *"the open skeleton only"*. Both cannot
  hold: a param no brace consumes is one the manifest advertises and the op
  ignores. **Braces win** — that row is the specific one — so sealed fields
  leave `update`'s params along with its braces. Nothing is lost: writing a
  sealed value is `create`/`put`, where insert-completeness is the actual
  reason for the rule.
- **`create` "always".** §2 derives `create` for every entity, but DL §6
  rule 8 refuses any statement covering two unique keys (`claimCoupon` is in
  the fixture to be refused). On a two-key entity `create` is therefore
  unregistrable for the same reason `put` is undecidable, and it is
  **skipped** — applying the language's rule rather than overriding it.
  `create` also carries `rev: null`, so its derived mode is `creates` as §2
  says rather than silently becoming `creates-or-converges` on every entity
  that happens to declare a unique field; `put` is the same braces without
  it, which is what makes them two ops rather than one op named twice.

### 5. The namespace is reserved by the lexer, for free

§6 reserves `std.` wholesale and the acl precedent (`reserved_derived_name`)
suggests a check. None is needed: an authored op name is one identifier and
the lexer admits no dot in one, so `op std.tasks.get = …` does not parse.
The **other** half of §7 did need building — a qualified name in *source*
position, which is how a std op is called and composed — and is one
lookahead loop in `source()`. It takes nothing from any program that parses
today, because a dot there is currently an error.

### 6. Deriving an op and exposing it are different acts

`std-library.md` §5 rules 2 and 3 are the difference between the std library
and the largest privilege escalation in the platform. `ordo_host::
deployment_engine` grants **every op to every role** it makes sense for — so
the instant std derived, every viewer in the fixture held all 85 derived
ops, i.e. read-and-write access to every open field of every entity, guards
filtering rows but nothing filtering fields. The template now carves std out
to owner and manager (rule 3), and `deployment.rs` asserts it, including the
negative control that the authored surface still reaches a viewer. Worth
recording because the fusion was invisible until the derivation existed: no
test could have caught it a day earlier, and nothing about deriving an op
makes a reader look at a profile template.

---

## A section preamble can be a status marker, and §8's is (2026-08-31)

Checklist 9e's remainder. The item listed twelve constructs as one debt —
`check`, `unique (a, b)`, `index (…)`, a write-valued `let`, and declaration
forms for `routine`/`when`/`every`/`trigger`/`principal`/`app`/`client`/
`policy`/`channel` — and they are not one thing. Three were owed and
buildable, one is owed and blocked, and **seven are not owed at all**.

### RULED: DL §8's opening line is the deciding sentence for five of them

`definition-language.md` §8 begins:

> ## 8. Platform definition blocks
>
> All illustrative syntax **[lean]** — concepts settled, file format not final.

Below it, `routine`, `principal`, `client`, `policy` and `channel` each carry
their own `[settled]` heading. Both are true and they are about different
things: the **semantics** are settled, the **file format** is explicitly not.
A declaration form *is* the file format. So building one means inventing the
shape the spec declined to fix, and 9e′'s ruling applies without amendment:
**a `[lean]` construct is not owed.**

This is the `allocate` seam read from the other side, and the symmetry is
what makes it worth recording. There, a `[settled]` entry marked a sliver of
itself lean — *"the allocation semantics above are settled; the spelling
lands with that construct"* — and the owed half was the **semantics**. Here a
section preamble marks all the spelling lean while each subsection pins its
own semantics, so the owed half is again the semantics and the **spelling is
the not-owed half**. Same seam, opposite side, opposite answer — and the
answer flips because the deliverable named in the checklist was *"declaration
forms"*, which is the spelling.

The general form: **a status marker's scope is whatever the sentence carrying
it is about, and that can be a whole section.** Reading `### Principals
[settled]` and stopping there gets the wrong answer, because the marker four
paragraphs above it is the one that governs the grammar.

### `app` is the corpus's clearest instance, and its document says so

DL has no `app` declaration at all. The grammar lives in `ui-gen.md` §5,
headed **"[mixed: binding grammar normative, everything else illustrative]"**,
which splits itself explicitly and then closes the question:

> a manifest cannot be compiled against this section until they land.

A document that partitions its own section into buildable and not is doing
the reader's hardest job for them. Nothing was inferred here.

### A settled construct whose only operand is not-owed

`when` and `every` are the exception that proves the rule. They live in
`## 7. Triggers [settled]`, which ends *before* §8's lean pin and qualifies
nothing — the cleanest `[settled]` of the twelve, with real spelling pins
inside it (`every day 08:00Z`: "UTC is explicit at the keyboard, never
silently implied"). And they are still not buildable, for a reason that is
not about them: **`-> run <name>` names a routine**, and routines are the
lean half. Two further obstacles compound it — the spec gives the same fact
two unreconciled spellings (§7's top-level `when … -> run x` and §8's nested
`trigger when …` inside a routine block, with nothing saying which is the
declaration), and there is no routine mechanism to attach either to.

That last point corrects a premise this item was started on. Checklist 5d is
titled "triggers and the job queue", and it built **primitives**, not a
runtime: `enqueue`, `claim_jobs`, `finish_job`, `claim_due`, `schedule` and
`try_lead` all exist in `ordo-exec/src/jobs.rs`, and **only `enqueue` has a
production caller** (cache invalidation). Nothing drains the queue, nothing
writes `$schedules`, there is no worker loop and no clock loop, and no
`routine:` principal ever runs anything. "Declaration syntax over an existing
mechanism" was the reasonable guess and it is not the situation.

**A settled construct whose sole argument is not-owed is a category**, and it
needs its own answer: not "build it", not "close it", but "blocked, and here
is the thing it is blocked on".

### `check` is owed, and blocked on one line of taxonomy

Three separate `[settled]` bullets in DL §3 — the construct, `check` may walk
refs, and both sides are re-checked. Nothing lean. What stops it is the
sentence *"violation = typed rejection aborting the whole atomic group"*
meeting a closed inventory that has no class for it. `critique-consolidated.md`
X-016 found exactly this and named the fix — *"add a `check_violation` code to
the taxonomy"* — and it was never taken.

Building the lowering first would put a **producer behind no class**, so a
settled rule would surface as `internal` 500: indistinguishable from a server
bug, which is the failure mode the write path's typed refusals were added to
end. `arithmetic_error` is the one precedent for engine-pinning a class where
the inventory is silent, and it was taken by the item that *owned* that
ruling. This one does not.

Two things the next attempt should not rediscover: a **ref-walking `check`
cannot be a Postgres CHECK constraint** (CHECK cannot subquery), so the second
and third settled bullets need triggers on every entity in the walk plus a
parent-side error naming the child rows — a second enforcement mechanism, not
more DDL. And even the row-local half must refuse `today`/`now`, because a
CHECK constraint must be IMMUTABLE and those are the first things an author
will reach for.

### The bug the item's own wording was causing

The doc-block harness's allow-list had all seven `[lean]` declaration forms
filed under **"settled features with no parser yet"** — the same mis-filing
9e′ found and corrected for `list of { … }`, re-made one document further out.
It was re-made because the checklist entry said "build these twelve". A list
that names constructs without their marks is an instruction to build them,
and the next reader will follow it.

## "Documented limitation" is not a translation (2026-09-05)

The 2026-09-04 sweep closed five defects in the reverse compiler and filed
three more under **Recorded, not fixed**: the clock pin, `INNER JOIN`
becoming an outer-preserving dot-path, and `group` always projecting its
keys. The last two were disclosed in notes and changed the answer anyway —
one the row set, one the column set — on the surface whose entire purpose is
making a translation visible. This sweep decides that the earlier one
declined to.

### The rule

> A decompilation may differ from its SQL only in ways that **cannot change
> which values come back**. Anything that changes the row set, the column
> set, or a row order the SQL had actually defined is either translated
> faithfully or **refused by name**. A note is not a licence to return
> different rows; a reader trusts the page over the docs.

Two carve-outs, both stated rather than assumed:

- Where SQL's own answer is undefined — `LIMIT` with no `ORDER BY`, ties at
  a `LIMIT` boundary — there is nothing to be faithful to, and the note
  saying Ordo's answer is *defined* where the SQL's was not is the honest
  disclosure.
- Where the values are equal and only their rendering differs — `avg`'s
  scale — a note is the proportionate answer. Refusing `avg` would be
  refusing arithmetic.

And **one standing exception**, which is not a carve-out and should not be
dressed as one: the text-collation pin can change which rows come back under
a `LIMIT`, and it stays a note anyway. The reasoning is under [Two that stay
notes](#two-that-stay-notes-and-why) below and it is a judgement about cost,
not a case where the rule does not apply. This file breaks its own rule in
exactly one place, on purpose, and says so.

### The two limitations, closed

**`INNER JOIN` is a filter, and can be spelled as one.** The v0 subset only
accepts `fk = joined.id` against a *declared ref*, which means the foreign
key is doing work the emitter was not using: a non-null `fk` matches exactly
one row of the target, so an inner join drops precisely the null-`fk` rows,
duplicates nothing, and reorders nothing. `sponsor is not null` is therefore
not an approximation of the join — it *is* the join. It is folded into the
`WHERE` as a conjunct before rendering, so it goes through the same
precedence-correct printer as everything else (`(a or b) and sponsor is not
null`, parenthesised because Ordo has no precedence table). An inner join on
a *required* ref drops nothing and gets no conjunct: claiming a divergence
that is not there is its own kind of lie. Before: three rows against two,
under a note.

**`group` always projects its keys — so put the projection somewhere else.**
Ordo has no post-`group` projection step; the pipeline steps are `filter`,
`sort`, `take`, `group`, `reduce`, `distinct`, `match`, `append`. But `let`
binds a whole pipeline and the *outer* source line can project a subset of
what it produced. `SELECT count(*) AS n … GROUP BY team` becomes

```
let grouped = people
group team { n: count }

grouped[n]
```

which returns one column, not two. The same mechanism fixes a second thing
the row oracle could never have seen: `group` projects its keys **first**, so
`SELECT count(*) AS n, team` came back as `(team, n)`. The `let` form is used
whenever the SELECT's output list differs from `keys ++ aggregates` — by
membership or by order — and the plain form stays everywhere else, because it
is the readable one and the `let` is machinery for a mismatch.

`sort` and `take` go on the OUTER line, not inside the `let`. The outer line
may sort by a column it does not project, which is what lets `ORDER BY team`
survive a SELECT that does not want `team`.

*The precise reason, because the first draft of this ruling overstated it and
a reader would have been misled.* A `sort` inside a `let` is **not**
discarded — it still decides which rows an inner `take` keeps. What does not
survive is its ORDER, once the outer line reads the binding:

```
let s = people
sort -salary
take 2

s[name]              → Cy, Eli      (the top two by salary — in ID order)

people[name]
sort -salary
take 2               → Eli, Cy      (the top two by salary — in SALARY order)
```

Same two rows, different sequence, no diagnostic either way. So the emitter's
rule is right and its reason is narrower than "discarded": an ordering placed
inside a `let` is silently lost, and only an outer `sort` is an ordering the
caller gets.

*Owed elsewhere:* that silence is a forward-compiler hazard in its own right,
independent of the decompiler. `let s = … sort x` followed by `s[…]` returns
an unordered answer, and nothing says so. Recorded here rather than worked
around; it is not this item's to fix.

### What the sweep found that nobody had named

Ranked by whether a user would be misled.

**1. Null placement in an ordering — silently wrong, and under `LIMIT` a
different set of rows.** Postgres sorts nulls FIRST under `DESC` and LAST
under `ASC`. Ordo's planner emits `NULLS LAST` unconditionally. And the
parser *read* an explicit `NULLS FIRST`/`NULLS LAST` and threw it away
(`if p.kw("nulls") { p.kw("last"); p.kw("first"); }`), so `ORDER BY x ASC
NULLS FIRST` and `ORDER BY x ASC` decompiled to the same Ordo. On the shipped
seed, `SELECT name FROM people ORDER BY nickname DESC, id LIMIT 3` is
Ada/Cy/Eli in Postgres and was Di/Bo/Ada in its decompilation — three rows
each, none of them the same three.

This was invisible to every oracle in the crate, because `row_multiset` sorts
both sides before comparing and the no-`LIMIT` form returns the same six
people in a different sequence. There is now an ordering oracle that compares
sequences.

Translated faithfully rather than refused: a leading `cond(k is null -> 0, 1)`
sort key puts the nulls in one bucket ahead of everything, and the real key
orders the rest. Verified on Postgres against all four combinations of
direction and placement, over text, int, date and a dot-path through an
optional ref.

**2. Two projected paths that share a leaf name — a column lost in
silence.** `SELECT p.name, sp.name` is two columns in SQL, both called
`name`. It decompiled to `projects[name, sponsor.name]`, which the forward
compiler accepts and plans as two `AS "name"` columns — and the reader gets
ONE, the sponsor's, standing where the project's was. Now refused, with the
fix (`AS` one of them) in the message. The same collision through two `GROUP
BY` keys with the same last segment is refused too.

*Owed elsewhere:* `ordo-sema` does not refuse a projection with two outputs
of the same name. That is not the decompiler's bug to fix and is recorded
here rather than silently worked around.

**3. `SELECT *` — nineteen columns against twelve.** It became the bare
source with a note listing what had been dropped. `SELECT * FROM people`
returns every physical column: `$space`, `$rev`, `id`, the `email$bidx`
blind-index digest, and the raw ciphertext of both sealed fields. Ordo's
whole-row form is the declared *open* fields. That is not a translation with
a caveat; the reverse pane printed `ROWS DIVERGE` under it every single time.
Now refused — and the refusal carries the explicit projection that does
translate, so it is one paste from working.

**4. String literals were being re-lexed, not quoted.** The emitter was
`format!("\"{s}\"")` with no escaping. Ordo's string lexer has a closed
escape set (`\" \\ \n \t`), so a SQL literal containing a backslash changed
value in transit: `'a\nb'` — a backslash and an `n` under
`standard_conforming_strings` — became a NEWLINE, compared in silence. A
literal containing `"` produced `"x"y"`, which is not a string at all, and
the refusal arrived from the forward compiler two steps later, worded about
source the user never wrote — the exact shape 4d-2 had just finished fixing
for `AS` aliases. Now escaped, with control characters that have no Ordo
spelling refused by name.

**5. Temporal literals were emitted verbatim.** Postgres accepts
`TIMESTAMPTZ '2026-05-01 09:00:00+00'`; Ordo's datetime is
`YYYY-MM-DDThh:mm:ssZ` and the `Z` is mandatory. Same class as 4: a downstream
parse error about text the user did not write. Now shape-checked here.

**6. Column identifiers were emitted verbatim.** `SELECT "$space"` became the
path segment `$space`; `SELECT "my col"` became two words. Refused here now.

**7. `ORDER BY` after a `GROUP BY` rendered the key's PATH.** `group
assignee.team` produces a column called `team`, so `sort assignee.team` is a
bind error — "no field `assignee` here". Sort keys are now resolved against
the grouped row's own names, and an `ORDER BY` naming something the grouped
rows do not have is refused with the list of what they do have.

### Two that stay notes, and why

**Text ordering collation.** Ordo pins `COLLATE "C"` on every text ordering
and on `min`/`max` over text, deliberately, so the answer cannot move between
deployments; the SQL side takes the database's own collation. The test
cluster's is `en_US.UTF-8`, where `'a' < 'B'` is TRUE and under `C` it is
FALSE. There is no Ordo spelling for another collation, so this cannot be
translated — and refusing every text ordering would refuse a third of the
corpus for a divergence that needs mixed case or punctuation to appear at
all. It is disclosed on every text ordering key, the note says a `LIMIT`
turns it into a row difference, and a test asserts the two collations still
*disagree* on this cluster — so the note stays backed by something rather
than becoming folklore.

**`avg`'s scale.** Ordo pins scale 12 (DL §2, the division ruling); Postgres
derives its scale from the operand and prints `36.0000000000000000` where
Ordo prints `36.000000000000`. The same number. A note, and it is now the
crate's demonstration that the `ROWS DIVERGE` banner still works — the
grouped case that used to play that role is faithful now.

### What the tests prove, and what they do not

`tests/differential.rs` grew from two oracles to five, and 22 statements to
46:

- **the row set** — 46 statements on real Postgres, both sides run, multisets
  compared. The count is asserted (`checked == DIFFERENTIAL.len()`), so a
  case that silently starts being refused fails rather than vanishing.
- **the row order** — ten single-column statements compared as *sequences*,
  because `row_multiset` sorts and cannot see an ordering defect. Every one
  carries an explicit trailing `id`: ties on a nullable key are ordered
  arbitrarily by SQL, and an oracle cannot compare against an arbitrary
  answer.
- **the refusals** — four of them: `SELECT *` on two entities, the
  duplicate-name collision, and ungrouped `count(col)`. For each, the
  translation the emitter *used* to produce is written out by hand and run
  against its SQL, and required to DISAGREE. A refusal is only honest if the
  thing refused really would have returned different rows; when one of these
  starts agreeing, the refusal has become over-caution and should go.
- **the premises** — twelve assertions that the seed still contains the nulls
  these cases turn on (covering `nickname`, `sponsor`, `team`, `assignee`,
  `age`, `ends`, `due`, and both sides where both sides matter), and that the
  cluster's collation still disagrees with `C`.

Each fix was reverted in turn against the full suite: every one is caught by
the ROW oracle on real Postgres, not only by the shape oracle.

Deliberately not proven:

- **The backslash half of the escaping fix has no row-level test.** The
  double-quote half does (before the fix the emitted Ordo does not compile at
  all). Demonstrating that `'a\nb'` matches a different row would need a seed
  value containing a backslash, and the `wide` seed is shared with four other
  suites. It is asserted on the emitted text instead, and this paragraph is
  the disclosure that a shape assertion is what it is.
- **The collation divergence is demonstrated on scalars, not on rows.** Every
  text value in the `wide` seed is capitalised the same way, so `C` and
  `en_US.UTF-8` agree on all of them. The test proves the two collations
  differ on this cluster; it does not prove a query returns different rows,
  because on this seed none does.
- **`SELECT *` after the refusal.** Nothing asserts what the platform columns
  of every entity are; the refusal quotes the schema rather than a list
  maintained here.
- **The browser.** Unchanged from the last sweep: `index.html` is asserted on
  by nobody.

### Two the sweep itself got wrong (found on review)

The agent that wrote everything above died before reviewing any of it, and
the two below were found by reading the emitter rather than by any oracle in
it. Both are the same shape as what the sweep was hunting: a translation that
looks right and returns a different answer.

**A joined side's `id` was eaten as the implicit tiebreak.** Ordo appends an
`id` tiebreak of its own — the SOURCE entity's — so the emitter drops a
trailing `ORDER BY …, t.id` rather than restating it. The test for "is this
the implicit tiebreak" was `col == "id"`, with no look at the alias. So
`ORDER BY a.name, a.id` on a joined alias dropped the ASSIGNEE's id and
emitted a note saying "Ordo appends it itself" — when what Ordo appends is
the task's. A different tiebreak, and under a `LIMIT` a different set of
rows. Now only the FROM table's own trailing ascending `id` is dropped, and a
joined side's id is kept as the ordinary sort key it is.

This one is asserted on the emitted TEXT, not on rows, and the reason is
worth stating: in the `wide` seed every assigned task's id runs in the same
direction as its assignee's (`tsk_1`→`ppl_1`, `tsk_2`→`ppl_2`, …), so the two
tiebreaks agree on every row and no row oracle can tell them apart. Same
judgement as the backslash case above — the seed is shared with four other
suites and is not worth editing for this. The ordering oracle gains the
positive case (a joined id as a sort key, compared as a sequence); the
discriminating case is the shape assertion.

**Ungrouped `count(col)` returned `null` where SQL says `0`.** `count(x)`
counts non-null values, and Ordo spells that `sum cond(x is null -> 0, 1)`.
That is exact inside a `group`: every bucket holds at least one row and
`cond` yields 0 or 1 and never null, so the sum is always a number. Ungrouped
it is not, because the input can be EMPTY — `sum` of no rows is null and
SQL's `count` of no rows is 0. `SELECT count(nickname) AS n FROM people WHERE
age > 1000` was `[[0]]` in Postgres and `[{"n":null}]` in the decompilation.
A different value, not a rendering difference, and it had no oracle.

There is no bridge to write. Ordo's aggregate braces admit aggregates only —
`coalesce` is not one of them (`bare_field_in_group: coalesce is not an
aggregate`) — so `coalesce(sum …, 0)` does not compile. Refused, with
`count(*)` and "add a GROUP BY" in the message, and added to the refusal
oracle so the refusal has to keep earning itself. W015 is grouped, so the
round-trip ratchet is unmoved at 29 of 37.

**And one refusal that named the wrong token.** `SELECT name FROM people
OFFSET 2` came back as *trailing tokens outside the v0 subset:
`Num("2")`* — because `is_clause_kw` did not list `offset`, so `OFFSET` was
swallowed as the table's ALIAS and the digit was all that was left to
complain about. Still a refusal, which is the safe direction, but it named
the wrong word. `offset` is listed now, along with the join-type and
set-operation words that were being eaten the same way (`union`, `cross`,
`right`, `full`, …), and `OFFSET` is refused as itself with the reason: Ordo
pages with a CURSOR, and `page` resuming from a key is a different question
from discarding the first n rows once anything is written between pages.
There is no emitter spelling to reach for, so this stays a refusal.

*Also corrected here:* the paragraph above claiming "48 statements" and
"thirteen assertions" was claiming more than the file contained (46 and 12).
A count in a ruling is a claim like any other.

## The playground was lying about the one thing it exists to show (2026-09-04)

`ordo-playground` had 1267 lines of source and one test — the round-trip
suite, which runs the reverse compiler over the stage-two SQL corpus and
compares rows. That suite is a good oracle for the queries it contains and
proved nothing at all about the queries it does not, which is where the
defects were. Four of the five below change ROWS, silently, on the page
built to make translation visible.

### 1. `<>` and `=` decompiled as if SQL's null logic were Ordo's

**Wrong rows, no note, no refusal — the worst shape a defect can take here.**

SQL's `=` and `<>` are three-valued: a null operand makes the predicate
unknown and `WHERE` drops the row. Ordo's `==`/`!=` are two-valued — they
compile to `IS [NOT] DISTINCT FROM`, so `null != 'B'` is TRUE. The
decompiler rendered `<>` as a bare `!=`. On the shipped `wide` seed,
`SELECT name FROM people WHERE nickname <> 'B'` is **one** row in Postgres
and **five** in its decompilation.

Two-valued equality is recent; the decompiler predates it and nothing
connected the two. The corpus contains no `<>` against a nullable column,
so the one existing test was green throughout.

Fixed by emitting the bridge where — and only where — the operand can
actually be null: `nickname != "B" and nickname is not null`, with a note
saying why. Nullability is read from the schema (through the ref for a
dot-path), so a required column still decompiles to a bare comparison. The
same reasoning covers `NOT col` on a nullable bool: `col` lowers to `col IS
TRUE` and agrees with SQL, but `not col` lowers to `NOT (col IS TRUE)` and
does not. Ordering comparisons and `IN` already lower through `COALESCE(…,
false)` and need nothing.

### 2. `a AND b OR c` decompiled as `a and (b or c)`

**Wrong rows, silently.** The condition parser read a flat AND-list and
folded a trailing OR into whichever conjunct preceded it. SQL binds `AND`
tighter than `OR`; that reading inverts it. On the `wide` seed the two
predicates differ by a row.

Compounding it, `(a OR b) AND c` — the only way to spell the other grouping
in SQL — was refused outright, because the condition parser had no rule for
`(`. So the one predicate the parser could express was the one SQL does not
mean.

Fixed with a two-level grammar (`or_expr := and_expr ('OR' and_expr)*`) and
a parenthesised factor. Ordo has **no precedence table** and refuses mixed
`and`/`or` unparenthesised (spec §1), so the emitter parenthesises every
mixed nesting — which is what preserves SQL's reading by construction.

### 3. A page size turned a working query into an error

`paged_rows` bound and planned but never called `resolve_blind`, the step
`Fixture::compile` does for the unpaged path. Any query touching a `sealed
indexed` field ran fine unpaged and refused with `unresolved_blind_index`
the moment a page size was set. Same query, two answers, decided by a
dropdown. One line.

### 4. The reverse pane showed one side of a comparison

`main.rs` claimed the page "displays the round-trip SQL next to it so a
divergence is visible, not hidden". It displayed the decompilation's rows
and never ran the submitted SQL, so a row divergence was exactly as visible
as an agreement. Defects 1 and 2 were therefore invisible on the surface
built to expose them. The pane now runs both sides and prints a
`ROWS DIVERGE` block with both counts.

### 5. Read panes served from the conformance suites' databases

`ordo_conformance` and `ordo_wide`, which the playground never seeds. On a
tree where `cargo test` had not run, every read answered `relation "orders"
does not exist`; run beside `cargo test`, reads raced a `DROP SCHEMA public
CASCADE`. All three databases are now the playground's own and rebuilt at
boot.

### Recorded, not fixed

- **The execution clock is pinned to `Clock::CORPUS` (2026-05-01).** With a
  real clock every `since` window on this seed is empty, so the pin is the
  right call — but the page said nothing, and a date-dependent feature
  answering about a date nobody chose is the bug `Clock`'s required
  argument exists to prevent, moved one layer out. The pin stays; the
  forward response now carries `clock` and the page prints it.
- ~~**An `INNER JOIN` becomes an outer-preserving dot-path.** Disclosed in a
  note since before this sweep, and the rows do differ. Left as a
  documented limitation rather than a refusal: the note names it and the
  reverse pane now shows both row sets.~~ **Superseded 2026-09-05** — both
  of these were wrongly decided. A note over a changed answer is not a
  disclosure a reader of the page can act on, and both turned out to be
  translatable: the join is `fk is not null`, the group goes inside a `let`.
  See ["Documented limitation" is not a
  translation](#documented-limitation-is-not-a-translation-2026-09-05).
- ~~**Ordo's `group` always projects its keys**, so `SELECT count(*) … GROUP
  BY team` decompiles to something with one more column. Noted rather than
  refused — the grouping is right, and the divergence is now displayed.~~
  Superseded, as above.

### Silent drops that are now refusals

A group key with an `AS` alias (`GROUP BY p.team` selected as `t`) had its
alias discarded and came back named `team`; a selected non-key column was
dropped entirely; a snake_case alias produced Ordo that does not parse, and
the refusal arrived from the forward compiler two steps later, worded about
a source the user never wrote. All three are named refusals at the
decompiler now.

### What the tests cover, and what they do not

One test became 51, across three suites with different oracles:

- `tests/decompile.rs` (27) — the shape oracle. No database. What the
  emitter produces for a decided translation, and the exact wording of each
  refusal. Every emission is checked to compile forward.
- `tests/differential.rs` (2) — the row oracle for the queries the corpus
  does not contain: 22 statements chosen for the places the two languages
  could disagree, each run twice on real Postgres and required to return
  the same rows. A companion asserts the seed still *contains* the nulls
  those cases turn on, so a seed edit cannot disarm eleven cases quietly.
- `tests/server.rs` (21) — the handlers, which had no tests because they
  lived in `main.rs` and a binary has no library surface. They now live in
  `ordo_playground::server` behind synchronous functions returning
  `serde_json::Value`; the axum layer is a wrapper per route.

Deliberately not covered: the browser. `index.html` is asserted on by
nobody, and its JavaScript is the one place a field can be renamed in the
API and silently stop rendering. The API-shape tests in `server.rs` are the
closest thing to a guard on it.

---

## Groups get a writer, and Y-013 is ruled (2026-09-05)

Checklist 10i landed the half of groups that **reads**: `groupMembers` as a
platform entity, the acl derivation emitting its second disjunct, the walk
disclosed in the scope manifest. Nothing wrote the table. A group could be
granted access and could not be given members, so the disjunct the
derivation emits was a semi-join against an empty relation — correct,
disclosed, and doing nothing. `definition-language.md` §3 also named a
refusal class, `group_nesting`, that no code had ever produced.

### 1 · Y-013 — the more permissive level wins · **RULED**

**The question.** A principal holds `read` on a row **directly** and `admin`
on the same row **through a group**. Which level applies?

**The ruling.** The **maximum**. Effective level is the highest level among
{the caller's own acl row} ∪ {acl rows naming a group the caller belongs to},
and it applies whichever path carries it. The audit note for the call names
**both** paths, not just the one that admitted — a decision recorded as "Dov
wrote via `boardsAccess`" when he also held a direct row is a record that
cannot answer "what happens if we remove him from Contractors".

**The reasoning, which is the part that matters.** The alternatives are the
minimum, or a refusal on disagreement. Both make **adding someone to a
group** able to **reduce** their access. That is not a corner case to be
documented; it inverts what the act means. An administrator putting Dana in
*Contractors* is granting, and if Dana already held `admin` directly, a
minimum rule silently demotes her to whatever *Contractors* has — a
revocation nobody performed, with no event that says so, discovered when she
cannot do her job. A refusal-on-disagreement is the same failure with a
louder failure mode: the administrator is told the grant is invalid and has
no way to express "both", which is the *normal* case under any per-project
group pattern.

It is also the only reading consistent with the documents that already exist.
`definition-language.md` §acl states it as settled — *"A caller's effective
level is the MAXIMUM of every acl row that applies to them … There is no
'most specific wins', no 'direct beats group', and no subtraction: a group
grant can raise a caller above their direct row, which is the point of
granting to a group"* — and `critique-consolidated-4.md`'s Y-013 entry
records the defect as a **glossary contradiction**, not an open design
question. So the ruling here is a ratification and a removal: the glossary's
*"avoid granting the same principal both directly and through a group on the
same row"* is gone, replaced by the rule. Advice standing in for a rule is
worse than either, because it reads as a limitation of the platform and is
in fact a limitation of the documentation.

**What the ruling cost to implement: nothing.** Checked rather than assumed,
which was the point. The derived guard is two outer disjuncts —
`X(member == $caller and <levels>) or X(member.groupMembers.group(member ==
$caller) and <levels>)` — and each disjunct asks *"is there a qualifying row
reachable this way"*. A union of existence tests is the maximum by
construction: it cannot subtract, and it has no notion of which path was
"more specific". No SQL changed. The work was the ruling, the test, and the
glossary.

**How it is proven** (`crates/ordo-conformance/tests/group_membership.rs`,
real Postgres, `ordo_grp_*`): one principal, one row, `acl (read, write,
admin)` whose write rung is `write`.

| direct | via group | writes? |
|---|---|---|
| `read` | `admin` | yes — the group raises him |
| `admin` | `read` | yes — the group does not lower him |
| `read` | `read` | **no** |

Both orderings, because one is not the rule. If only the first passed, the
outcome would be an artifact of which disjunct the planner reached first; if
only the second, the implementation could be taking the direct path and
ignoring the group entirely. The third row is what makes the first two mean
anything — without a case where the union *refuses*, "he could write" is
equally explained by a group disjunct that admits anyone it names at any
level, which is a real and much worse bug that a two-row test would pass.

### 2 · The membership surface, and why it is not an op

`members` and `groupMembers` are platform entities, refused to authored
statements at registration and again at the write path's dispatch. That
refusal is the rule, not an obstacle: DL says *"there is no domain-authored
path to minting one"*, and `security-hardening-review-2.md` §B6 extends it
to groups — *"group-membership edits are platform commands with their own
door, not domain-authored ops over a members-shaped entity"*. So membership
is written by `ordo_conformance::membership` (`Fixture::group_command_on` /
`group_command_as`), on the same footing `members` rows are: by the platform,
never by a program. Three commands, because DL names three — create a group,
add a member, remove one — each appending exactly one attributed
`CommandGroup`, staged until the effect commits, exactly as an op's group is.

**`memberCount` is maintained there**, because there is the only place it can
be: DL calls it *"platform-maintained alongside the membership commands (same
machinery as a tree's `depth`)"*. Every add and remove rewrites the group's
count in the **same command group** as the link row — two rows changed, two
commands — so a revert cannot restore the link and leave the count behind, a
state the platform never had. `count(*)` over the link rows **is** the
principal count precisely because nesting is refused; the two rulings hold
each other up.

It stays the *cheap upper bound* DL describes, and the suite proves the gap
is real rather than theoretical: on a board where one principal holds a
direct row and a group row, `sum member.memberCount` answers 6 and the
`distinct` pipeline answers 5. That is the documented limitation executing,
not a defect — the honesty is in showing it.

**Deliberately not ruled here.** *Which role* may administer groups, and the
no-escalation invariant `security-hardening-review-2.md` proposes (a writer's
own effective grant set must cover the authority they confer). Both are
`[proposed]`. The surface enforces the boundary that *is* settled — the actor
is attributed, is a member of this space, and is not a guest (bootstrap rule
4's null-`role` row) — and leaves the authority bar to the door, which is
where rule 6 already puts owner administration. A surface is the wrong place
to settle an authorization question, and settling one by implementing it is
how a `[proposed]` becomes load-bearing without anyone deciding it.

### 3 · `group_nesting` gets a producer

DL: *"Groups may not contain groups in v1 — nesting is rejected at write time
(`group_nesting`)."* Documented since groups existed; produced by nothing.

This is not a scope note. Depth 1 is the **premise** that makes a single
expansion sufficient everywhere: the acl-derived guards emit exactly one
`member.groupMembers.group(member == $caller)` hop, `callersOf` enumerates
one level, and `memberCount` counts link rows. A nested group would not fail
loudly — it would quietly under-expand all three, in the safe direction and
inaudibly, which is the same silent-omission shape 10i was opened to fix.
Refused at the add, with the class DL names, asserted to leave no row and no
log entry behind.

The neighbouring refusals answer with the classes that already exist rather
than minting new ones. An id that names no group, one that names a `members`
row which is not a group, and a member with no row in this space are all the
single `not_found` message, byte-identical: telling a caller *"that id exists
but is a person"* is an existence answer about a row they never saw (R-G10).
Re-adding an existing member is `duplicate` — the unique key on
`(group, member)` is what makes "remove Sam from Finance" delete a known
number of rows. An anonymous or guest actor is `access_guard`, checked before
any probe runs, so a refusal cannot answer an existence question on the way
out.

### 4 · Found on the way: the fixture's `members` rows were wrong

`seed.sql` seeded `kind` as `'person'` and `memberCount` as `0` for every
principal. DL §3's table says `kind` is `principal` or `group` and
`memberCount` is *"`1` for a principal"*. Both had been wrong for as long as
the fixture has existed, and nothing noticed because nothing read either
column: `memberCount` had no maintainer and `kind` had no consumer until a
surface arrived that has to tell a group row from a principal row.

Corrected in the seed rather than worked around in the code that reads it. No
golden moved — the DDL and artifact goldens carry the schema, not the data —
which is itself the reason it survived: a value no assertion covers is not
protected by having goldens.

**Still divergent, recorded not fixed:** DL says `members.principal` is
*"`unique` within the space"* and `ordo_schema::platform_members` builds it
with `unique: false`, so no constraint enforces one members row per principal
per space. Out of this item's scope; named here so it is a finding rather
than a silence.

---

## `allocate` gets modes: the odd penny is a business rule (2026-09-05)

Checklist 9f⁗. The owner's ruling, in his words: **"A dev decides in which way
to allocate and when that happens, not the spec."**

DL §3 pinned exactly one distribution — floor each exact share, hand the
leftover minor units out by largest remainder, ties by weight order — and
marked the paragraph `[settled]`. That settles the *arithmetic*. What it also
did, without meaning to, was settle a **business decision**: which party ends
up a penny up. Contracts name a residual party. Accounting policies name
largest-remainder. Settlement agreements name the largest counterparty. Those
are three different right answers to the same maths, and a platform that
hard-codes one of them is a platform you cannot write the other two in.

So this widens a closed grammar, which needs its reasoning on a page.

### RULED: a named-mode third argument, four modes, default unchanged

`allocate(total, weights [, mode])`. The mode is one bare name from a closed
set; omitted, it is `largestRemainder`.

| mode | queue | the case for it |
| --- | --- | --- |
| `largestRemainder` | largest fractional remainder, ties by position | **default** — what accounting texts and regulators assume, and it minimises the largest deviation from an exact share |
| `first` | position ascending | a contract naming a residual party first |
| `last` | position descending | the ledger habit: plug the difference into the final line |
| `largestShare` | \|weight\| descending, ties by position | the rounding lands where it is proportionally smallest |

**Named modes rather than a family of functions** (`allocateFirst`,
`allocateLast`, …) for two reasons. One name to learn and one place to look
when the answer surprises you — a family scatters the decision across four
entries in a closed function table that a reader has to notice are related.
And the failure modes differ: an unknown *function* is already a parse error,
but a family invites `allocateLargest`, which is a different function name and
would be refused with no idea that a mode was meant. A mode is a value in an
argument position that means one thing, so the refusal can list the four
answers.

### The framing that makes the invariants free: a mode is a queue

The important design move is that **a mode is not a second algorithm**. Every
mode runs the same two steps — floor every exact share, then hand out the
leftover minor units **one apiece** — and chooses only the *order of the
queue*. Two properties then hold for every mode by construction rather than by
four separate promises:

1. **the parts sum to the total exactly**, because the leftover is `total`
   minus the floors, which is in `[0, n)` for either sign of `total`, so
   handing out exactly that many `+1`s closes the gap whoever receives them;
2. **no part is more than one minor unit from its exact share**, because each
   starts at its floor and a queue position is served at most once.

The second one is not decoration. Without it, "the odd unit to the first
party" would naturally generalise to *all* the leftover units to the first
party when there are several — which still sums to the total, and would move a
party ten minor units off its share in a ten-way split. That is a
redistribution wearing a mode's name. `allocate` exists for the sum guarantee,
and the one-apiece rule is what stops the mode argument from being a hole in
it.

Both are asserted for all four modes over the same hostile sweep the default
was already held to (negative totals, zero weights, one huge weight against
many tiny, totals smaller than the party count), and each mode is separately
checked to be the queue it claims, against an independently derived one. A
further test asserts the four are **pairwise distinct as functions** — every
mode passing its own characterization is still consistent with two of them
being wired to the same comparator, because the expectation and the
implementation would be making the same mistake.

### Determinism is the boundary, and it is what excludes rotation

**A mode may depend only on `(total, weights)`.** That is the whole admission
test for a fifth mode, and it is not a style preference: DL §3's tie-break is
pinned "deterministic, so both venues agree", and a mode that consulted
anything else would make the host and SQL answers diverge and make the same
call answer differently on a retry — which the write protocol's idempotency
rests on not happening.

**Rotation is therefore not a mode and will not become one.** Spreading the
odd penny across successive invoices so that it evens out over time is the
mode users ask for first, and it is not a function of `(total, weights)`: it
has to remember what it did last time. That memory is application state, and
an expression function that carries state is not an expression function. The
blessed shape is the one the platform already has for this: the application
keeps the counter in its own data and passes the weights in the order that
counter dictates. The rotation is then visible to an auditor in the
application's own rows, rather than hidden in the compiler where nobody can
see whose turn it was.

The refusal says this rather than only saying "unknown mode", because the
reader's next move otherwise is to look for a typo.

### Why the default stays largest-remainder

Three reasons, in order of how much they bind.

1. **It is a printed value, not a preference.** Four documents print
   `allocate(100.00, (1, 1, 1))` = 33.34 / 33.33 / 33.33, and they print it
   under this mode. `first` happens to give the same answer there (the
   tie-break *is* positional order), but `last` gives 33.33 / 33.33 / 33.34
   and would falsify the page. Changing a default is changing every existing
   call, and here it would also change the specification.
2. **It is the profession's default.** Accounting texts and regulators mean
   largest-remainder by "allocate"; a caller who does not name a mode has not
   made a decision, and the answer they get should be the one their auditor
   expects.
3. **It is the only mode with a mathematical argument for being the default.**
   It minimises the largest deviation from an exact share. `first`, `last` and
   `largestShare` are chosen because a document says so, which is exactly why
   they must be *named* rather than defaulted to.

### An unknown mode is a compile error, and where that is decided

In the **parser**, and the class is `parse_error` — already in the closed
inventory, nothing minted. The reasoning is the 9r boundary rule: the third
argument position of `allocate` is a mode and nothing else, so a word that is
not a mode is text outside the closed grammar, which is what that class means.
Deciding it in the parser also puts it where it survives the site landing —
`each` is still `[lean]`, and a rule living in the site's binder would be
written by whoever lands the site, i.e. later and by someone else.

The consequence worth stating: a mode name **cannot be read as a field**. An
entity with a field called `first` does not create an ambiguity, because the
position admits modes only. A misspelled mode that silently allocated by
largest-remainder would be a money bug wearing a typo's clothes — nothing
would fail, and the parts would still sum.

A legal mode at an illegal site is still `invalid_allocate`. The mode check is
the parser's and the site refusal is the binder's; neither substitutes for the
other, and a mode does not make a home legal.

### The site is still not owed

Unchanged from 9f′, and worth repeating because this item widened the rule
without touching it. `allocate` is legal only inside an `each` emission over a
`list of { … }` parameter; DL §1 marks both `[lean]`; 9e′ ruled a `[lean]`
construct is not owed. The mode argument parses today and every call is still
refused at its site. That is the same arrangement the rule itself has been in
since 9f′: the settled half held executable and pinned, so that it cannot
drift while its home is being decided.

### Two things found on the way that nobody was looking for

- **`allocate` panicked on input no document forbids.** The kernel closed with
  `i64::try_from(part).expect("a part is bounded by the total")`, and the
  comment beside it was right about the case everyone had in mind and wrong in
  general: `|wᵢ| ≤ |Σw|` bounds every part by `total` **only when the weights
  share a sign**. A mixed-sign vector that nearly cancels — `Σw` tiny, the
  weights enormous — makes individual parts far larger than the total they
  still sum to. `allocate(i64::MAX, (1_000_000, -999_999))` is the small case,
  and it panicked. 9f′ had named negative weights as unruled and written that
  "the kernel computes the stated formula and the invariant survives"; the
  invariant does survive, and the *function* did not.

  Negative weights stay unruled — inventing a refusal for them is a ruling and
  this is not the place for it — but "unruled" cannot mean "panics". The
  overflow now returns the class an `int`/`money` overflow already has:
  `arithmetic_error`, which `api-engine-design.md` defines as "`/ 0`,
  `mod(a, 0)` **or an `int`/`money` overflow**". A reuse, not a mint. The
  neighbouring cases still answer, so this is not a blanket ban on mixed signs.

- **The mode names the literature uses do not lex.** `largest_remainder` is
  how every accounting text and every other implementation spells it, and DL
  §1 *Identifiers* — `[settled]`, "one rule for every declared name …
  `[a-z][A-Za-z0-9]*`, lowerCamel, no underscores" — means this language does
  not have that spelling. The first draft shipped four modes nobody could
  type, and the lexer caught it. They are `largestRemainder` and
  `largestShare`, and the test that pins the mode set now also asserts every
  name matches §1's rule and actually lexes, because the next person to add a
  mode will reach for the literature's spelling too.

### The general lesson

**A settled rule can be settled about the wrong question.** DL §3 was right
about the arithmetic and did not notice it was also deciding a policy. The
tell was available: the paragraph explains *why* largest-remainder is exact
and never says why largest-remainder rather than another exact rule — because
there is no mathematical reason, only a conventional one. Where a spec
justifies a choice on grounds that do not distinguish it from the
alternatives, the choice is probably the caller's.

---

## Grant pinning: DL §8 wins, and the escalation it warned about is now a test (2026-09-05)

Checklist 8s. Three documents specified grant pinning in three forms, the
code implemented a fourth, and one of the specified forms was a silent
privilege escalation. The owner's instruction: *"Implement recommendation —
make sure the escalation is not possible."*

### The ruling

**DL §8's form is the one built.** A capability grant pins, per op, the
manifest hash, the output field set, **and the row-scope shape**: per
reached entity, the read and write guard walks, the `$caller` restriction,
the tree flag — plus the op-level `unguarded` acknowledgment. Comparison is
shape against shape, not hash against hash:

- **Narrowing** — fewer columns, a gained guard, a gained `$caller`
  restriction, an entity no longer reached — **applies silently and is
  audit-noted.** This is what §5 wanted and the old combined hash denied;
  a consent screen people are made to re-click after every deploy is a
  consent screen they stop reading.
- **Expansion** — a new column, a new entity, a lost or *changed* guard
  walk, a lost `$caller` restriction, `unguarded` newly set — kills the
  grant **for the whole space until re-consent**, and kills the *whole*
  grant, not the one op: consent was one act over a set of ops, and a grant
  alive in pieces is a state nobody consented to.

### Why §5's form lost

`security-logging-design.md` §5 and `api-engine-design.md` §10 pinned the
manifest hash and the **ordered output field set**, with subset = narrowing.
DL §8 calls that decidable and **wrong**, with a counterexample that is the
most ordinary edit in the language: delete `member == $caller` from an op
body, or add `unguarded` to its header, and the output field set is
**byte-identical** — same columns, same types, same order. A field-set
comparison reads that as "not an expansion" and applies it silently. *"This
app may read my expenses"* becomes *"this app may read everyone's expenses"*
with no re-consent and no audit flag. Field sets answer which columns; they
never answered which rows, and rows are what the consent was about.

That counterexample is executed, not cited: `ordo-host/tests/pinning.rs`
deletes the `$caller` restriction while moving no column and asserts the
grant dies; adds `unguarded` while the field set *shrinks* and asserts the
grant dies; and runs a genuine narrowing through to success with the audit
note saying what narrowed.

### The edges, all failing closed

- A **changed** guard walk is fatal, not weighed: "a different walk is a
  different set of rows," and the comparator does not pretend to decide
  whether two walks denote comparable sets.
- An **undeployed** op is expansion-equivalent — an absent manifest is not
  a smaller one.
- The comparison is decidable from the two manifests alone, with no
  registry history — the pre-ruling formulation quietly required history,
  since it promised narrowing applied silently *and* that any hash change
  killed the grant, and narrowing changes the hash.
- **The door does not read the grant registry.** The token carries its own
  pins; `GrantRecord` stores the encoded pin so *"what did I agree to?"*
  has an answer, but a registry outage must not become an authorization
  outage.

### The boundary recorded rather than closed

A loosened `take $n` bound reaches **more rows of the same shape**, and
neither the field set nor the row-scope shape can see it. DL has no clause
for volume; inventing one inside the comparator would be legislating in
code. Noted in `PinDrift`'s documentation as not covered, so the next
reader finds a stated limit instead of a discovered one.

---

## Keyset paging counts as a row bound (2026-09-05)

Checklist 9k′, and the blocker on 9k's last derived op. The owner's
decision: proceed as recommended.

### The disagreement

`std-library.md` §5.7 asserts a std listing needs no `unbounded`
acknowledgment *because it is keyset-paged*. `ordo_plan::cost::row_bound`
accepted three kinds of evidence — a `take`, a keyless `reduce`, a unique
pin — and paging was not among them, so the sentence in the spec was
unimplementable and `std.E.list` could not derive. 85 ops derived; the
listing did not.

### Why `take` could not substitute

The tempting dodge was to derive `list` with a large `take`. It is wrong in
a way worse than refusal: `plan_paged` wraps the bound query with the take
**inside** the paged subquery, so `take n` caps the listing at n rows *in
total* rather than per page — row n+1 is unreachable by any cursor, and the
walk ends looking complete. A listing that silently stops is worse than one
that refuses, because nothing tells the caller anything is missing. This is
now a test (`a_take_cannot_stand_in_for_paging`).

### The ruling, stated tightly because it relaxes a shared gate

**A plan is row-bounded if it is served as a keyset page over a total
order.** `ordo_plan::cost::keyset_paged` decides it, and the definition is
deliberately narrow:

- The plan's tiebreak must be non-empty — the language's guaranteed total
  order, which is what makes a cursor exact rather than approximate. An
  order that is total only up to identity would let paging skip or repeat
  rows at ties, and a gate that accepted it would stop meaning anything.
- Nothing below the order may multiply a row: a many-side source join
  disqualifies. A group's keys and a `distinct`'s outputs are unique per
  emitted row by construction, so they restore totality whatever sits
  beneath them; `append`'s tiebreak ends in a per-arm row number for
  exactly this reason, and a subquery or CTE is paged iff its inner query
  is.
- The page size itself is bounded by the door, not by the plan — the gate's
  question is only whether a cursor can walk the order without loss.

The gate's refusal message now names the missing half precisely: *"no
provable row bound and cannot be keyset-paged — its order is not total, so
paging would skip rows."*

**This is not a std-only privilege.** The evidence the gate accepts for
`std.E.list` is the evidence it accepts for any authored listing with a
total order — a derived op is exempt from nothing, in either direction.

### What it unblocked, and what held

`std.E.list` derives (16 entities on the market fixture, 149 → 165 ops in
the artifact golden — **pure addition**: audited op-by-op, zero existing
manifests changed, none removed). The suite holds it to the standard the
other derived ops meet, and past it: a caller sees only their own rows
through the listing, **the guard travels into every page rather than just
the first**, and `deriving_list_does_not_loosen_the_pre_reduce_gate_on_count`
pins §2.1 rule 4's pairing — `count` still refuses a sealed refinement
before its reduce, because the effective-cost gate's whole argument rests
on that pairing staying tight.

---

## `check` builds its row-local half, and refuses the rest by name (2026-09-05)

Checklist 9e, second half — the declaration forms landed earlier; this is
the semantics. The owner approved both halves of the recommendation:
mint `check_violation` (X-016 named it two audits ago and it was never
taken), and scope `check` to the row-local half.

### The scope, and why it is a refusal rather than an approximation

DL §3 settles three forms of `check`. One is built and two are refused
**by name**, because each is impossible to build honestly today:

- **Row-local** (`check used <= seats`) — **built**. Lowered to a real
  Postgres CHECK constraint, enforced inside every write's transaction; a
  violation aborts the whole atomic group, which is DL's
  seat-never-oversells argument executed rather than promised.
- **Ref-walking** (`check assignee.org == customer.org`) — refused,
  `check_beyond_row`. A CHECK admits no subqueries, so this form is
  trigger work — on the declaring write *and* on writes to every entity
  the walk passes through, per DL's own "an invariant that only guards one
  of its own edges is not an invariant". The trigger substrate has only
  just landed; wiring half an invariant would be worse than refusing.
- **Time-dependent** (`check due >= today`) — refused,
  `check_time_dependent`. A CHECK expression must be IMMUTABLE. A rule
  that consults the clock passes at write and silently becomes false as
  time moves — a lie wearing an invariant's name.

Two more refusals fell out of building it, neither in the original brief:

- **`check_sealed_field`** — the column stores ciphertext; the database
  cannot evaluate a predicate over a value it cannot see, and a blind
  index answers equality-to-a-known-digest only.
- **`check_unknown_field`** — a typo'd field gets an authored,
  case-visible class rather than `unknown_field`, which ruling 10f made
  internal.
- **`check_unsupported`** — constructs outside the row-local slice
  (params, expression functions, `cond`, division, fractional money
  literals, duration literals), each message naming the construct and the
  spelling to use instead.

### Two-valued logic is load-bearing twice in a CHECK

Postgres treats a NULL check result as **pass**. The language's table says
an ordering predicate against null is **false**. So the rendered SQL uses
the planner's exact shapes — `IS [NOT] DISTINCT FROM` for equality,
`COALESCE(…, false)` for ordering and `in` — and the difference is
observable: `check slack <= 100` over a null `slack` *refuses the write*,
where naive rendering would let it land silently unenforced for exactly
the rows nobody looks at. Executed in
`check_invariants::null_fails_an_ordering_check_because_the_language_says_so`.

### The constraint name is the diagnostic

SQLSTATE 23514 plus a constraint name is everything Postgres reports, so
the name carries the condition: `teams$chk0$used<=seats` — stable
`{entity}$chk{i}` prefix, condensed source suffix, truncated to the
63-byte identifier limit (the prefix alone is unique, so truncation cannot
collide). The wire class is read **once, by SQLSTATE** — at the pool for
the runtime (beside 22012/22003), and at the harness's one psql seam
(`VERBOSITY=verbose` puts the SQLSTATE on the error line) — never by any
layer matching English.

### Found while building

- ~~**DL's own example does not compile yet.**~~ **BUILT (17f,
  2026-09-07).** The entry read: §3's `set teams { id: $t, used: used + 1 }`
  needs a computed RHS, and the write protocol accepts literals, params and
  `$caller` only. `check_invariants.rs` now executes the constraint against
  the increment spelling as well as the param-bound one, which is what that
  pairing was always meant to be. See *Computed write right-hand sides*
  below for the rules the spec left to this engine.
- **`unique (a, b)` and `index (…)` never validate their column names.**
  A typo'd column in either clause survives to DDL and fails at CREATE
  TABLE as a raw database error. Found while deciding what class a typo'd
  check field gets; recorded rather than silently fixed, because the fix
  deserves its own tests.

---

## `idempotency_applied` — a class may not promise a future it does not have (2026-09-06)

Checklist 8q's last window, ruled with `docs/plan-idempotency.md`. The
plan's own recommendation was *stop and document*: the correctness hazard
it was written to close turned out to be closed already, and the audit's
verdict was to keep the response store where it is. What was left was one
wire honesty problem, and it is the kind worth recording.

### The fact, and the two names it could wear

A retry arrives for a key whose effect **already committed**. The `$replay`
journal rides the write's own transaction, so the executor's
in-transaction read says so, the attempt rolls back untouched, and nothing
re-executes. But the verbatim response body was stored *after* that commit,
outside the transaction, and the process that was going to store it died in
between. There is no body to replay.

That answer was returned as `idempotency_in_flight`, with a message that
said the true thing in prose. It is the wrong class, and the reason is not
aesthetic:

- `idempotency_in_flight` means **somebody is still working; ask again**. A
  client that switches on the class name and polls is behaving correctly.
- Here nobody is working, and nobody ever will be again. The write landed;
  the body is gone. A client polling on that name polls forever.

A message is prose for a human reading a log. The class is the field a
machine branches on, and it was telling the machine to wait for an event
that cannot happen. So: **`idempotency_applied`** — 409, terminal, *the
effect committed, its response was lost with the process that ran it, stop
retrying*. One producer (the router's `AlreadyApplied` arm), one row in the
closed inventory, both directions asserted — the crash-window retry gets
it, an ordinary verbatim replay still gets its 200 and never sees it.

### Terminal, and therefore not cached — which is the counter-intuitive half

The class is stable: ask again and the journal answers the same way. That
makes it look like a natural member of the cacheable-for-replay family
(`is_exec_conflict`), and it is deliberately not one.

It is not an *outcome*. Every member of that family is what the op did
against the data at that moment; this is the absence of a result, a report
about what the response store does not contain. And storing it would be
actively wrong: the holder whose body went missing may be **stalled rather
than dead**. Its reservation ages out, this retry answers, and the original
then completes its `put` with the real 200. Writing a terminal 409 into
that same slot lets the order of those two decide what every later retry
sees — and the losing order answers *"your body is gone"* to a caller whose
body is sitting in the store. The journal is the durable authority and
answers identically every time it is asked; caching the answer buys nothing
and adds a way to be wrong.

### Found while ruling it

The plan's audit called the journal's own `complete_lease` UPDATE a small
cost *inside* the append lock's critical section. Measuring it (5f,
`ordo-host/tests/lock_wait.rs`) found both halves off: it is ~29% of the
section, which is not small, and it is not in the section at all — the lock
is transaction-scoped and taken as late as possible, so the completion
record is already written when the append acquires it. The conclusion
survives in a stronger form. The section has room for nothing, and the one
platform statement already there is free only because of where it sits.

---

## Delegation: the record, the door, and the one refusal the wire is allowed to distinguish (2026-09-06)

`audit-board-coverage.md` row 4: `definition-language.md` §8 settles that
**delegation is itself a grant** — user → agent, expiring, revocable,
command-logged, *distinct from the capability token's op allowlist* — and
`bootstrap.md` item 10 makes a live one **required** on the agent
provisioning path. The code column read *"none — no match for `delegat` in
`ordo-identity`. `PrincipalKind::Agent` exists; the grant that makes it
lawful does not."*

The owner's decision this was built under, verbatim: *"Yes this product
will offer that as a feature for the builders for ease of
use/implementation. MCP by default — though we'll abstract it."* So the
core is built and the protocol surface is planned behind a seam
(`plan-delegation-surface.md`).

Five rulings, each of which had a cheaper alternative.

### 1 · The delegation is its own record, its own table, its own id

DL says *distinct from the token's op allowlist*, and folding the two would
have saved a table. What it would have cost is the rule itself. The two
records answer different questions, are consented by different people, and
are revoked by different people:

| | capability grant | delegation |
|---|---|---|
| asks | may this token call these ops? | may this principal act as that person? |
| consented by | whoever administers the agent | **the delegator, and nobody else** |
| revoked by | the same | **the delegator** |
| scope | an op allowlist, pinned per op | a person |

One record carrying both means a manager editing an allowlist is editing
the reach of somebody else's consent — *"a manager granting an agent ops
never grants it someone else's face"*, defeated by a data-model shortcut.
`DelegationId` is likewise not a `GrantId`: one id type would let a
revocation aimed at "let this thing be me" land on "let this thing call
these ops".

### 2 · This registry IS consulted at the door — the one place that boundary moves

`ordo_identity::grants` states in terms that the grant registry is *"not
consulted to admit a request… a registry outage cannot become an
authorization outage"*, and names the trade-off: a revoked grant is refused
only where the registry is reached, and a deployment wanting immediate
revocation must put the registry in the request path and accept it.

For delegation, it is put there, and the asymmetry is the ruling:

> A capability grant is the agent's **own** authority; whoever can revoke it
> is whoever suffers if it lingers, and "revocation lands at expiry" is a
> trade-off they made. A delegation is **someone else's face**. The user
> withdrawing it does not hold the token, gets no say in how long it lives,
> and would be told the thing acting as them stops *eventually*. That is not
> revocation, and DL calls delegation revocable.

The cost is stated rather than hidden: a delegation-registry outage is an
authorization outage **for delegated calls and for nothing else**. A
deployment that cannot confirm a delegation confirms none, and serves every
ordinary call untouched. That is why `IdentityAccess.delegations` being
`None` refuses every delegated token rather than admitting it — a delegation
nothing can confirm is one nobody can revoke.

### 3 · The two consents INTERSECT, and the bound is the delegator's live reach

`security-logging-design.md` §6 rule 4, in passing: *"a delegation carries
only the delegator's authority."* Built as `DenyReason::
DelegationBeyondDelegator`, evaluated with `membership_reaches` — the same
predicate `callersOf` reports with, so the door and the audit surface
cannot come to disagree.

Without it the two consents **multiply**: whoever administers the agent
lists `space.delete` on its allowlist, Ada consents to "act as me", and the
agent reaches something Ada — an editor — never could. A delegation would be
a privilege-escalation primitive wearing a convenience's clothes.

Two consequences that fell out and are worth naming:

- **Suspending the delegator stops the delegation at the next call.**
  `membership_reaches` refuses a suspended member, so Z-005's *"an operator
  stops a compromised member NOW"* is not defeated by the thing wearing
  their face. That would have been the suspension's one loophole.
- **The bound is live, not pinned.** A demoted delegator's agent loses the
  reach immediately, which is right. A *promoted* delegator's agent silently
  gains it, which is the half a live check cannot catch;
  `plan-delegation-surface.md` §5.2 recommends recording the delegator's
  reach at consent time and raising an audit note — never a denial — when it
  has grown.

### 4 · `delegation_required` — the door's one distinguishable refusal, and why it is safe

This repo's rule is that the door has **one** answer: an unregistered op and
a forbidden op are byte-identical, because telling them apart tells a prober
what exists (C7 / X-038), and `ordo_host::platform_ops` says of `callersOf`
that *"the number of new refusal classes is zero, on purpose"*. So a new
403 needed an argument, not a preference.

The argument is **positional**. The refusal is decided by a **token gate**
that runs *before the op registry is consulted and before the space is
bound*. Consequences:

- Every name the same token could send — a real op, an op it may not call, a
  name nobody ever registered — gets the identical status, code and message.
  The class therefore correlates with nothing about the deployment. This is
  asserted directly (`the_gate_answers_identically_for_every_name_a_token_
  could_send`), because it is the property the whole carve-out rests on.
- What it discloses is the state of the caller's **own** delegation, and the
  caller is its holder.

And the repair is genuinely different, which is why folding it in would be a
disservice rather than a discipline: `access_denied` means *ask whoever
administers you for the op*; this means *the person you were acting for has
to consent again* — a different act, by a different person, that no retry
and no wider scope reaches. An agent that cannot tell them apart retries
forever or escalates to a human with the wrong question.

**The ordering is the security argument, not an implementation detail.** Had
the check lived inside `AccessChecker::check`, an unregistered name — which
the router answers without ever reaching the checker — and a registered one
would have given a dead-delegation caller two different codes, and the
difference between them would have been a map of the deployment's op
surface. That is why it is a second trait method with a `Pass` default
rather than a widened `check`.

Everything else about delegation stays under the uniform answer: a
space mismatch, a lapsed claim, and the delegator-authority bound all depend
on what the caller sent, so all three are `access_denied` on the wire and
four distinct `RecordedDecision`s in the space's own stream.

### 5 · The actor taxonomy was NOT extended; the delegation id lives in the registry

`Actor::AgentOnBehalfOf { agent, user }` has existed since the command log
was written. What did not exist was anything able to **construct** one —
nothing knew a call was delegated — so the variant had no producer and only
tests referenced it. `ordo_host::attribution` is that producer.

The delegation **id** is deliberately not in the rendering. DL §8 pins the
attribution string verbatim as `agent:<name> on behalf of user:<id>`; the
chain hashes the *rendering*, not the enum; a third component would
contradict a settled spec and change a frozen encoding. So the id lives in
the registry, and a chain entry joins to it on `(space, agent, delegator,
at)`.

**That join is exact only because at most one live delegation per that
triple is representable** — `DelegationRegistry::record` refuses a second
(`AlreadyLive`), and re-consent is revoke-then-record: two audited acts
rather than one silent overwrite. The uniqueness rule is not tidiness. It is
the thing that makes *"which consent authorised this write"* answerable at
all, and without it the honest answer would have been "we cannot tell".

Recording the **same** id twice is not a conflict — a retried ceremony must
be idempotent, or one dropped response becomes a permanent refusal.

### 6 · `callersOf` grows a fifth via, and it is conjunctive

Rule 4's four words (`role`, `group`, `override`, `token`) are closed *"on
purpose"*, and the argument for the closure is the argument for extending
it: *"a via is the thing a manager acts on to remove access, and a fifth
kind of access with no fifth via would be access nobody can find."* A
delegated agent is a fifth kind: it is removed neither by editing the
profile nor by revoking the token, but by the **delegator** withdrawing
consent — a different act by a different person. Indexed under `token` it
would send a manager to revoke a grant and find the caller still there.

It is emitted **only** for an agent that also appears via `token` for the
same op, because both consents are required at the door and a row for either
alone would name a caller the door refuses — rule 4's own *"listing a caller
the door refuses is the same class of wrong as omitting one it admits"*. So
a delegated agent appears **twice**, which is "every via, not the first"
read honestly: two ways to stop it, two people's to perform, two rows.

### Found on the way, that nobody was looking for

- **`refresh_presence` would have refreshed presence into a delegated
  session.** The step-up refresh carries *"everything else about the session
  through unchanged"*, and it refused capability tokens only. A token that
  claimed a delegation and no capability grant — a perfectly constructible
  shape — could have had a fresh user-presence assertion minted into it,
  making a delegation satisfy the one gate the platform has for *not
  delegable*. Now refused, with a test.
- **The token payload had no room to grow safely.** The format is positional
  with a fixed field count, and adding a seventh field while keeping the
  `v1` word would have made a truncated or forged six-field token read as
  *"claims no delegation"* — a silent downgrade to acting-as-itself, which
  is the dangerous direction. It is `v2`, and `v1` is refused outright; a
  token is a short-lived bearer credential, so a format bump is a re-login
  rather than a migration.
- **A dead delegation is refused rather than downgraded.** DL says an agent
  *without* a delegation acts only as itself. Presenting one that is not
  live is a different fact: the caller asked to be somebody. Silently
  downgrading it would land writes in the chain attributed to the agent that
  the caller believed were the user's. Refuse, and say what the repair is.
- **A partial unique index cannot express the uniqueness rule.** "At most one
  *live* delegation per pair" needs `expires > now()`, and a unique index
  predicate must be immutable. Enforcing it with `WHERE revoked_at IS NULL`
  alone would be wrong in the ordinary direction: a delegation that lapsed a
  year ago and was never explicitly withdrawn would lock the delegator out
  of ever consenting again. It is a conditional `INSERT … SELECT … WHERE NOT
  EXISTS … RETURNING`, and the read-committed window is stated in the
  module doc rather than papered over.

### The remainder that decides whether this is useful

**`$caller` inside a delegated call is still the agent.** The door
authorizes against the delegator's reach; the executor's row guards do not.
Most guards in this language are `$caller`-shaped, so a delegated agent
calling an op its delegator can call, over rows guarded by that delegator's
membership, currently sees **nothing** — lawful, and quietly empty, which is
the worst failure shape available.

It was not changed here because `$caller` is reserved and router-inserted
(R-G13), the pinning machinery records `caller_restricted` per entity, and
the idempotency cache and page cursor are both keyed on the door's resolved
principal (SH-17) — four separate arguments, each owed its own conformance
case. `plan-delegation-surface.md` §7 lays out the three candidate answers
and recommends `$caller` = the delegator, and makes it **increment 2** —
before the ceremony, before the consent page, before MCP — because
everything else is polish on a feature that otherwise returns empty results.
The door's bound was built first precisely so that the row layer has
something consistent to move toward.

---

## The exit ops: what a bundle discloses, and where the record goes (2026-09-06)

Building `std.space.export` and `std.space.delete` (`plan-export.md`) put
two questions in front of the code that `definition-language.md` §8 and
`migration.md` §6 had answered only in part.

### Sealed fields in an export — §6 had already ruled, and it is followed

`migration.md` §6: *"Data — per-entity JSONL (and SQL loadable against
`schema.sql`), **decrypted under the owner's authority through the normal
key-release path**; client-first spaces export from the device, where
plaintext lives."*

That is a complete ruling and the implementation follows it rather than
re-opening it. Both halves are modes:

- **Opened** (the default) — one `KeyProvider::release` for
  `Purpose::Read` over the whole export, then `open` per value. Plaintext
  lands in the JSONL. This is *why* the op is step-up gated: export is
  total disclosure, so the disclosure is as authenticated as the door can
  make it.
- **Ciphertext** — §6's second clause. A deployment holding no key for a
  space cannot open its envelopes; pretending otherwise produces a bundle
  of nulls that reads as *this field was empty*. The envelopes go out as
  hex in the frozen layout, and the manifest says `"sealed": "ciphertext"`.

Three consequences the sentence did not spell out, ruled here:

**An unopenable value in opened mode is a refusal, not a null.** A space
with a destroyed epoch holds data the platform has and cannot hand over.
Leg 1 of `licensing.md`'s trust argument is *"a documented op, not a
support ticket"*; a bundle that silently under-delivers while looking
complete is worse than one that says what it cannot do. Re-run in
ciphertext mode and the state is disclosed for what it is.

**Blind-index columns are omitted, and the manifest says so.** A `$bidx`
value is `HMAC(k_field, plaintext)` under the *source* space's key. It is
not data: it carries nothing the plaintext beside it does not, it cannot be
reproduced under a destination space's key, and shipping it hands the
reader a working **equality oracle** over the values it sits next to — the
same leak §7 rejects the content hash for. Re-deriving digests belongs to
import, under the destination's key.

**Export is therefore a disclosure format, not a backup format.** An opened
value cannot go back into the `bytea` column `schema.sql` declares without
being re-sealed under the destination space's key, and re-sealing is
`ordo import`'s job (§3, genesis import mode), which is unbuilt. The
round-trip is exact for the open portion and declared-only for the sealed
one; saying so is the point.

### "Command-logged" means the audit spine, and the spine has two halves

§8 calls both exit ops *"owner-only, step-up gated, command-logged ops"*.
The first two are literal. The third cannot be, and the reason is
structural rather than an implementation gap.

The **command log records writes to entity rows**. Its taxonomy is closed —
`Create` / `Converge` / `Delete` — and every command carries an entity, a
row id and before/after images. Neither exit op is a write to a row:

- **Export mutates nothing.** Forging a `Converge` with equal images to
  record a read would put a lie in a chain whose entire value is that a
  reader may take it literally. A revert of that command would be a revert
  of nothing.
- **Deletion destroys the chain.** A final group appended to a log that is
  deleted in the same act records nothing, and a log that survives its
  space is a space that was not deleted.

**Ruled:** both exit ops record `EventKind::CustodyAction` in the
**security-event stream**, at space scope — beside epoch rotation, epoch
destruction, custody conversion and break-glass, which are the acts these
two are actually siblings of. The requirement §8 is making, *the owner
cannot do this without leaving a record*, is met by the half of the spine
that can outlive the space. The command log is not weakened, and no forged
command enters a chain.

This is why `ordo_exec::erase::RETAINED` keeps `$secevents` and
`$seccheckpoints` through a deletion: the one record that must outlive a
space is the record that it was deleted. That retention has a stated cost —
principal identifiers and op names survive the space — and it is
deliberate, because an audit spine a subject can erase is not an audit
spine. Where a jurisdiction disagrees, the answer is `policy retention`,
not a hole in the spine.

### Crypto-erasure ends the SMK, not only the DEKs

`destroy_epoch` was the narrow primitive and stays narrow: it refuses the
epoch new writes seal under, and no sequence of rotations reaches "no live
epoch" through it, because rotating to escape an epoch mints another.

**Ruled:** space deletion destroys **every DEK and the SMK under them**.
Dropping the DEKs alone is what crypto-erasure usually means and it does
not finish the job — blind-index keys are SMK-derived and
epoch-independent by design (that is what lets rotation leave a digest
where it is), so a surviving SMK leaves anyone holding a backup able to
recompute `HMAC(k_field, guess)` against the `$bidx` columns in it. Erasing
only the DEKs answers *what was the value* and leaves *was the value X*
answerable, which is the weaker half of the same question.

Keys are destroyed **before** rows. The failure modes are asymmetrical:
keys gone with rows surviving is a space whose sealed data is already
unrecoverable and whose rows a re-run removes; rows gone with keys
surviving is a space that looks deleted while every backup of it stays
openable. A partial failure should land on the stronger guarantee.

### The undo horizon is disclosed, not enforced

`prune` refuses to cut inside the undo horizon because the platform
guarantees the **owner** a fine-grained revert for that window. Deletion is
the owner spending that guarantee.

**Ruled:** deletion reports how many groups are still inside the horizon
and proceeds on the confirmation; it does not refuse. Refusing would make
the platform the owner of their data's end. What *does* refuse is a
**non-evictable consumer behind the head** — an archive that is the copy of
record, a client-first device that is the authoritative store, a listener
with at-least-once deliveries outstanding — which is the same
`ConsumerKind::evictable` split `prune` already respects, read once more. A
stalled evictable mirror blocks neither, because it is a cache.

The confirmation is the **space's own id typed back**: not a boolean, which
a retried request supplies for free, and not a token the platform issues,
which would make the platform the party that decided.

### Found while building

- **`Pool::build_shadow` leaked `search_path` onto a pooled connection.**
  `SET` is session state; the connection went straight back into the pool,
  and the next unrelated query on it would have resolved `orders` to the
  shadow schema's empty copy — a read returning nothing, correctly typed,
  with no error anywhere. The only existing caller dials a throwaway pool,
  which hid the hazard rather than removing it. Fixed with `RESET
  search_path` in the same simple query.
- **A bundle was not loadable from itself.** Members are in name order so a
  bundle is diffable, but every ref is a foreign key on `($space, id)`, so
  an importer inserting in name order hits `orders` before `vendors`. The
  manifest now carries a topological `load.order` and a `load.cyclic` list
  for entities that reference each other and have no sequential order at
  all. Found by the round-trip test, which follows the manifest and nothing
  else.
- **`fixtures/seed.sql` writes `'\x00'::bytea` into every sealed column** —
  a one-byte placeholder, not an envelope, left from before anything in the
  tree could encrypt. An opened export refuses against it, correctly. The
  comment above those rows still says *"nothing in the tree can encrypt
  (F0)"*, which stopped being true when F0 landed. Recorded rather than
  silently reseeded: real envelopes in the fixture would change every
  golden that reads those columns, and that deserves its own change.
- **`legalHold` has no producer anywhere in `crates/`.** `policy
  compliance` marks it `[lean]`, so deletion cannot respect a legal hold
  and does not pretend to.

---

## The interface descriptor: what a projection may say, and what it may not (2026-09-07)

B1 (`tooling-and-embedding-plan.md`) is the one data contract the generated
SDK, the LSP and generated UI share, and `ui-gen.md` §1–§4 settle that
descriptors are shape-verified, catalog-scoped, carry typed-id action
discovery and commit CAS-by-default. Building it forced a handful of
judgment calls that neither document rules on. They are recorded here with
the reasoning, because each of them will look arbitrary later.

### The descriptor is a projection of the catalog manifest, not of the compiler

The obvious build is a second walk over the compiler output shaped for a UI.
It is also the mistake 9m already made once: the catalog manifest and the
plan artifact answered the same questions from two walks, and *two walks are
two answers waiting to differ* — which they did, the artifact growing a
field-scope manifest the manifest would not have. 9l collapsed them.

**Ruled:** the descriptor is a third projection of the same producer, and
`ordo_plan::interface` takes **JSON in and JSON out** so it cannot reach a
schema, a registry or a binder even by accident. Where it needed a fact the
producer did not carry — which entity an id parameter addresses, whether an
output is refinable, which entity an id column derefs to, whether the result
is statically one row — **the producer grew the fact**. That is the same rule
stated from the other side, and it is what makes "derived truth" checkable:
`ordo-cli/tests/interface.rs` walks every emitted value back to the manifest
key it came from.

The consequence is that `catalog_view` moved from `ordo-host` down to
`ordo_plan::artifact`. It had been in the wiring crate because that is where
the catalog *route* is — but every field it selects is compiler output, which
is `artifact.rs`'s own layering argument for why the artifact producer is not
in `ordo-host`. `ordo-host` keeps the half it actually owns: **who may see
which op**.

### Scoping is one filter, applied before everything derived

`ui-gen.md` §1 settles that descriptors are catalog-scoped. The tempting
implementation is a scoping rule inside the descriptor builder, beside the
catalog route's own — two rules, one of which will eventually be loosened
without the other.

**Ruled:** the serving route filters the manifests with the same check the
door runs and *then* describes them. There is one filter, so "the descriptor
a caller receives contains exactly the ops their catalog shows" is true by
construction rather than by agreement.

That ordering is load-bearing for the derived halves, and this is the part
worth writing down. The typed-id action index is computed **after** the
filter: an index built over the whole program would tell a viewer that
`proj_` ids have actions and name them, which is the enumeration the catalog
refuses one op at a time. So would an entity list built from the schema.

### Entities are described when a visible op reaches them; id types are named more freely

Two different disclosures, ruled differently, and the difference is the point.

**An entity's field list** is described only when a visible op reaches it
(the manifest's own `entities`, which is the field-scope manifest's
projection). A descriptor is metadata and grants nothing, but an enumeration
of every shape in the deployment is still an enumeration.

**A typed-id entry** — the prefix, the entity name it stands for, and which
visible ops accept it — is emitted for every entity a visible op's ids point
at, *including one no visible op reaches*. This discloses nothing new: the op
already returns `proj_01J8…` values, so the caller holds the prefix and the
name it stands for already. Withholding the mapping would only stop them
linking a column they can see, and would not stop them reading it.

### Refusal classes are affordances, and the list is a subset by design

Each op carries the refusal classes its **own declaration** makes reachable.
The honest framing matters, because the useful list and the exhaustive list
are different lists and conflating them would be dangerous:

- It is a **subset** of what the door may answer. Every caller still handles
  the closed inventory — `internal` and `serialization_conflict` can meet
  anyone, and a generated SDK types the whole enum. This list exists so a UI
  knows which forms need a conflict view, which need a duplicate message
  beside a field, which need a confirm.
- Every entry is derived from a **stated declaration**: a required parameter,
  a declared `<=` bound, a unique key, a `check`, a tree edge, an inbound
  unowned ref, a key release. Nothing is listed because it seemed likely — an
  affordance rendered on an entity that cannot raise it teaches its author
  that the affordance means nothing.
- It mints **no class**. The empty-allow-list rule applies here too: every
  name is asserted against `ErrorCode::ALL`, and every derivation *rule* has
  a witness. Two of them (`check_violation`, `bound_violation`) have none in
  the market fixture, which declares no `check` and no parameter bound, so a
  four-line program witnesses them — an untested derivation rule is a claim.

### Only the settled half of the metric rule is implemented

`ui-gen.md` §5 marks *"reduce-ended ops are metrics"* `[settled]` and offers
two more single-row cases — a `group` over a constant key set, and `take 1`
over a totally-ordered pipeline — as `[proposed]`.

**Ruled:** `result.single_row` implements the settled case only (a `reduce`
is a keyless `group`, and a keyless group is provably one row). A descriptor
that implements a proposal is a descriptor that has to change when the
proposal is ruled on, and every consumer with it. Nothing is blocked in the
meantime: the row bound is on the same document, so a consumer that wants to
treat `<= 1` as a card can, and say so in its own code rather than reading it
as a platform guarantee.

### The op-level refinement tier is `none`, and saying so is not a placeholder

`refine.tier` is `none` for every op, because `refinable` is a **field** mark
(`sealed indexed refinable`) and §B2's per-op Tier-1 opt-in has no spelling
in `definition-language.md`. The door therefore refuses `$refine` outright
and a renderer must draw sort-only chrome.

**Ruled:** state the tier anyway, and carry the per-field list beside it. The
tier is a fact about the language, not a stub; and the field list is the
other half of the same rule (`std-library.md` §2.1), so the day the op-level
mark lands a UI can grey the right columns instead of discovering them from a
rejection.

### The UI annotations are not in this document

`ui-gen.md` §1 says surface descriptors are a projection of B1 **plus** the
reserved annotations (`label`, `desc`, `order`, `choices`, `danger`).
`definition-language.md` §10.3 marks them inert, and the lexer does not carry
one of them.

**Ruled:** B1 emits none of them. An always-null `danger` would be a
producer-less field, and a renderer would learn to trust it — which is how a
"this is destructive" mark comes to mean "nobody wrote one".

### Found while building

- **`check` was the sixth silently-omitted declaration.**
  `plan-schema-evolution.md` §1.1(a) found five (`default`, `normalize`,
  `enum_values`, `unique_keys`, `indexes`) and fixed them; `check` is the
  same family and was missed. `check used <= seats` reaches `ddl_for` as a
  real CHECK constraint, enforced inside every write's transaction, and
  reached the artifact document nowhere — so a deploy that tightened,
  loosened or removed a row invariant showed a reviewer no line.
  `program_version` moved (it hashes the DDL too, which was §1.1(a)'s
  *general* fix doing its job), but the diff a human reads was silent about
  what moved. The document now carries `{name, source}` per rule. It is also
  a contract fact by the same test the other five pass: a check is a refusal
  class a caller can hit, and the constraint name is what the 409 reports.

  **Left open, and owed:** `ordo_schema::diff` has no taxonomy code for a
  `check` in either direction, so `ordo plan --against <db>` cannot classify
  adding, tightening or dropping one. Increment 2's "class A only" apply will
  need one before a check can be added to a live deployment.

- **`ordo_api::FieldSpec` carries no type.** The wire's own output manifest
  has `name`, `sealed`, `refinable` and `refine_class`, and nothing that says
  what a column *is*. Compiled ops are unaffected — their types come off the
  bound query and reach the artifact — but a **platform op** (`std.space.*`)
  is the one kind whose result columns nothing in the tree can name a type
  for. Its descriptor entries carry `"type": null`, stated in
  `platform_ops::manifest` beside the null rather than guessed as `text`,
  and the descriptor marks the op `platform: true` so a generator can tell
  "the compiler proved this" from "the platform asserts this".

- **Every op version moved, and that is the correct outcome.** The artifact
  gained `params[].id_entity`, `outputs[].refinable`, `outputs[].id_entity`
  and `result`, so 165 op version hashes moved and every capability grant
  pinned to an old manifest needs re-consent. The audit is the point: the
  golden diff is exactly those four keys plus 18 entities' `checks`, with no
  `plan.sql`, `scope`, `row_scope`, `guards` or `cost` line touched. What an
  op discloses genuinely grew — a caller can now see which of its output
  fields accept a refinement condition — so a pin that survived the change
  would be a pin that had stopped meaning anything.

---

## Two 409s that look alike: `cas_miss` refetches a row, `version_skew` restarts a listing (2026-09-07)

Building `@ordo/react` (`ui-gen.md` §2, `clients/ordo-react/`) forced a
distinction the settled docs make separately and no reader had yet had to make
at once. `ui-gen.md` §1 settles the commit path as **CAS-by-default**: the
fetched row's `rev` rides the write, and a miss is *"a typed conflict the
runtime resolves by refetch + conflict presentation, never a silent
overwrite."* `api-engine-design.md` §8 settles that a page token minted under
a different op version answers `version_skew`, *"because keyset columns may no
longer mean the same thing. The client restarts from page one; the token is
not migrated."*

Both are 409. Both are "the world moved under you". A runtime brief written
from §1 alone reads naturally as *updates carry the row's `rev`; a
`version_skew` refusal surfaces as a typed conflict the consumer must handle
with a refetch-and-remerge affordance* — and that sentence, followed
literally, produces a merge dialog in front of a user who was scrolling a
table.

**Ruled: they are two conflict kinds, surfaced on two hooks, with opposite
repairs, and the runtime never folds one into the other.**

- **`cas_miss`** is a *data* conflict. Stored data refused the write; the
  identical request lands once that data changes. Repair: refetch the
  addressed row through the catalog's own detail op, remerge, retry against
  the new `rev`. It surfaces on `useCommit` as `conflict`.
- **`version_skew`** — and `invalid_cursor` with it — is a *cursor* conflict.
  No row disagreed with anything; the op's version moved under an outstanding
  token. Repair: drop the cursor, re-page from the top, keep the rows.
  It surfaces on `useOpPages` as `restarted`, and the state handed back is a
  fresh one, because continuing from the old cursor is precisely what the
  server just refused.

The general form, which the package encodes as a `Disposition` over the closed
inventory: a refusal's class says what happened, and what a surface must *do*
about it is a second fact that has to be stated once rather than re-derived at
each call site. `conflict` (refetch and remerge), `restart` (drop the cursor),
`retry` (ask again, safely, under the same key), `terminal` (asking again will
never change the answer), `denied`, `request`, `fault`. Three of those are
load-bearing and the rest are noise reduction; the important property is that
`retry` is the *only* disposition under which an automatic retry is correct,
and `cas_miss` is not in it.

Two corollaries the same build forced, recorded here because both are places
where the easy implementation is the wrong one:

- **A CAS op with no `rev` is refused before the wire, not sent.** "CAS by
  default" cannot mean "CAS if the caller remembers to pass a rev": that makes
  last-write-wins the shorter code path, and the shorter path is the one an
  ejected component copies. The runtime raises the door's own `missing_param`
  naming `rev` and makes no request. An op that declares no `rev` refuses a
  supplied one just as loudly, as `unknown_param`.
- **A retry that re-asserts the losing `rev` is refused locally too.** Without
  that, `retry()` is a spin: the same write, the same rev, the same 409,
  forever, and the loop looks like a network problem rather than a
  programming one. It is refused as `cas_miss` with a message naming
  `refetch()` — the same class, because it is the same fact, learned earlier.

Nothing in the settled text is changed by this; it is two settled rules being
told apart at the one layer that has to hold both.

---

## What an editor grammar is allowed to assert (2026-09-07)

Building `editors/ordo.tmLanguage.json` (`plan-editor-tooling.md` increment
0) raised a question no settled document had needed to answer, because until
now nothing outside the compiler made a claim about Ordo source. A
highlighter is not a parser and cannot be one. So: **when the grammar and the
compiler must disagree, which way does the grammar err?**

Three cases forced the question, and they do not all resolve the same way,
which is why the rule below is about *kinds* of disagreement rather than a
single direction.

**Ruled: a highlighter may under-refuse but never over-refuse, and it may
render a spelling the parser has not yet reached.**

### 1 · A partial check is worth more than no check, and its gap must be named

A date literal must name a day that exists — `lexer.rs::is_real_date` applies
month length *and* the Gregorian leap rule, because "a literal that cannot
occur should not survive the lexer" (Z-010). A regex can carry the month
lengths: `2026-02-31`, `2026-04-31`, `2026-11-31`, `2026-13-01` and
`2026-01-00` are all refusable lexically, and the grammar refuses them. It
cannot carry the leap rule, which needs arithmetic on the year.

The alternatives were to check nothing (colour every `\d{4}-\d{2}-\d{2}` as a
date) or to check everything by generating a year-by-year alternation. The
first throws away a real refusal the reader benefits from on four of the five
impossible shapes; the second is a regex that has to be regenerated on a
calendar.

So the grammar checks what it can and **says in its own comment which case it
gets wrong** — `2026-02-29` colours as a date and the compiler is the one
that refuses it. The general form: a partial lexical check is legitimate when
the gap is *silence*, not a false accusation. Under-refusing costs a reader
one deferred error message. Over-refusing — underlining something legal —
costs them trust in every underline the tool draws, which is the whole asset.

This is also why there is no rule for `lease-expiry`. §2 settles that the
hyphen is not an identifier character and that `lease-expiry` is genuinely
`lease` minus `expiry`. A rule flagging it would over-refuse legal
subtraction. Silence is correct.

### 2 · A word is coloured by position, and the rare position loses

`count`, `read`, `open`, `run` and `day` are contextual: they stay `Ident` so
they remain usable as field names, and position disambiguates them (G3). The
grammar colours them only in the position where they mean the vocabulary word
— aggregates only inside `group`/`reduce` braces and only in value position,
`open` only inside an `access` clause, `read|write|admin` only after `access`
or inside `acl (…)`.

A field genuinely named `count`, used in a group brace's value position, will
therefore be coloured as an aggregate. That is accepted. The trade is
explicit: the common reading is right, the rare one is *mis-coloured and
never mis-parsed*, and the alternative — colouring these words nowhere —
would hide the language's most-used vocabulary to protect a field name almost
nobody writes. The exact answer is semantic tokens from the language server,
which resolve the name rather than guessing at it; the grammar's job is to be
right instantly and dependency-free, not to be right always.

### 3 · The grammar renders the settled spelling, not the parser's current reach

`every day 08:00Z` is the settled schedule spelling — §7 pins that "schedule
times must be written with the `Z` suffix. UTC is explicit at the keyboard,
never silently implied" — and it **does not lex**. It reaches the lexer as
`Int Colon Int` and then a capital `Z`, which is the identifier refusal.
Schedules are parsed from a standalone string by `trigger::Schedule::parse`,
and `parse_program` accepts no declaration that could contain one, because
§7 and §8 spell the trigger declaration two irreconcilable ways and
`trigger.rs` declines to invent a third.

The grammar colours it anyway, as a valid schedule, and refuses `08:00`
without the `Z` as `invalid.illegal.schedule-time`.

The reasoning: a highlighter's subject is **the language**, not the parser's
current front. Every trigger and routine block in `syntax-examples.md` is
settled prose that authors are told to copy; colouring them as errors would
teach that the documented spelling is wrong, which is false, and would
underline a third of the corpus. The line this does *not* cross is inventing
a spelling: the grammar renders `every day HH:MMZ` because §7 pins it in
prose above the lean line, and renders nothing for `route … fallback …` or
an `app`/`page` body, where the form itself is unsettled — those get their
declaration head coloured and their contents left as ordinary expressions.

The corollary is the one that binds the *language server* rather than the
highlighter, and it is the opposite: a server may not diagnose a form the
parser cannot accept, which is why `plan-editor-tooling.md` gates increment 2
on the trigger/routine declaration form settling rather than on effort.

### Found while building

Tokenizing every fence in `definition-language.md` with the real TextMate
engine surfaced four spellings that violate §2's one identifier rule —
`match transfers.from_account`, `append reminders(due)[title, at: due_date]`,
`$GOOGLE_CLIENT`, `$DISCORD_CLIENT`. `docs_parse.rs` covers
`syntax-examples.md`, `tutorial.md` and two how-to guides, but not
`definition-language.md`, so nothing had read that file's code blocks
mechanically before. Recorded in the checklist (14e) rather than fixed here:
adding the file to `docs_parse.rs` has allow-list consequences of its own,
and it is a corpus decision, not a grammar one.

---

## `ordo.interface/2`: what a renderer may not be asked to reconstruct (2026-09-07)

The first consumer of the interface descriptor (`@ordo/react`, `ui-gen.md` §2,
checklist 14b) was built against `ordo.interface/1` and reported seven facts a
renderer needed and could not read. Six of the seven had a workaround, every
workaround was correct, and that is precisely why they were worth closing: the
package's whole argument is *"there are no op-name heuristics anywhere in the
source"*, and six correct heuristics is still a document that made a consumer
guess. This ruling settles what moved, what the version number did, and the
one place where describing the deployment honestly meant contradicting a
settled doc.

### One version, and the argument for it

`interface.rs` states the rule: the number moves on **a removed key, a changed
meaning, a narrowed value set**, and adding a key is not breaking. Applied
per-gap, four of the seven are additions:

- `id_types[].list` / `id_types[].get` — the missing index (below).
- `params[].field_ref` and `params[].enum_values` — the link from a parameter
  to the field it fills, and that field's value set.
- `params[].min` — the derived floor on a `take` count.
- `result.shape` and `statements[].summary` — what a response actually holds.

Three are not, and each is the *changed meaning* case exactly:

- **`call.cas`** was `true` for every write and delete. It meant "is a
  mutation" — which `kind` already says — and now means "asserts a revision".
  A `/1` reader that branched on it now takes the other branch.
- **`call.refinable`** was `true` for every read while `refine.tier` was
  `none` for every op. It is now a projection of the tier, so it is `false`
  wherever the door refuses. Same key, opposite value.
- **`call.terminal` / `call.paged`** advertised `pages` and `true` on
  single-row ops. The value set narrowed.

**Ruled: one document, `ordo.interface/2`.** The additive four ride with the
breaking three rather than shipping first at `/1`, because a document cannot
be `/1` for some keys and `/2` for others — a version names a shape, not a
per-key compatibility matrix. And the consumer's own gate proved the rule
works before anything was rewritten: pointed at the new golden it refused with
`SchemaVersionError` and named both versions, which is what a loud major is
for.

The corollary is that **no fallback path was kept**. A `/1`-shaped document
cannot reach this package any more — `parseInterface` refuses it — so keeping
the heuristics "for older descriptors" would have kept code that provably
never runs. They were deleted with the reason recorded here.

### The index keyed on what an op RETURNS

`id_types` indexed ops by the id types they **accept** (`actions` for writes,
`detail` for reads) and nothing indexed what an op **returns**. So the
commonest question a form asks — *an `assignee` parameter addresses `members`;
what stocks its picker?* — had no answer, and the consumer reconstructed one
from `result.row_identity` plus "no required parameters".

Two things were wrong with that, and only the second is fatal. It ranked
`std.E.list` first **by name**, in the package that exists to need no names.
And it could not distinguish *"nothing lists this entity"* from *"my rule
missed one"* — which mattered immediately, because in the market fixture the
answer for `members` is genuinely nothing (it is platform-provided, `std_ops`
skips platform entities by flag, and no authored op returns its rows). An
empty list that was **stated** is a different object from an empty list that
fell out of a heuristic, because only the first can be trusted.

**Ruled: the index carries both halves, and the listing half is derived from
`ui-gen.md` §1's row identity** — an op lists `E` iff its rows carry `E`'s
typed id, which is `result.anchor` as the binder proved it. Split on
addressing, never on name:

- **`get`** — anchored on `E` *and* addressed by a required `E` id: the ops
  that read one row back.
- **`list`** — anchored on `E` and not so addressed.

`std.E.list` and `std.E.get` are what make the rule well-defined; `myProjects`
and `openTasks` qualify by exactly the same test, and the golden shows 39
listing entries of which the majority are authored. Whether a *particular*
listing can stock a **blind** picker is then a question about
`params[].required` — a fact a consumer already holds — so the index owes the
candidates and not the policy: `cheapActive($cap)` lists products and is not a
cold picker source, and both halves of that are now readable.

**`get` is not `detail`, and the difference is a bug this found.** `detail`
holds every read that accepts an `E` id *whatever it returns* — an op taking a
category id and returning products is in it. The consumer's `cas_miss` repair
refetched through `detail[0]`, which in the market fixture happened to be the
right op by alphabetical luck. Presenting another entity's row as "the current
value" after a conflict is the exact failure the conflict state exists to
prevent, and `get` is the fact that makes it impossible.

### A parameter's value set: the link belongs to the producer

`entities[].fields[].enum_values` existed, and `interface.rs` said of it that
*"a dropdown is derivable from this and nothing else"* — while the parameter
that fills such a field is spelled `text`, with nothing pointing back. The
consumer bridged it with `statements[].entity` joined to a field of the same
**name**, refusing when two written entities both had one.

**Ruled: the producer grows `params[].field_ref` — `{entity, field}` — and the
descriptor resolves it into `params[].enum_values`.** Three reasons the join
could not stay in the consumer:

- **A name join is not the same question.** The AST knows which field a
  parameter fills, through ref paths and through a read's source conditions.
  `recentOrders($since)` filters `orders(placed >= $since)`: the parameter is
  an `orders.placed` value, and `statements[]` is empty on a read, so the
  consumer's join could never have produced it.
- **The ambiguity refusal belongs where the ambiguity is.** One parameter
  landing in two different fields is a property of the op, resolved once to
  `null` by the walk that saw both sites — not re-litigated by every reader.
- **It came free.** `param_id_entities` was already walking exactly these
  sites and throwing the field away to keep the ref target. It is now
  `param_sites`: one walk, two facts, so `id_entity` and `field_ref` cannot
  disagree about which site they read.

`field_ref` names **declared fields only**. `id`, `rev` and `depth` are the
platform's and are not in `entities[].fields`, so naming one would point a
reader at a field the document does not carry. `enum_values` is `null` and
never `[]`, because an empty list reads as a dropdown with no options.

### CAS is a revision assertion, not a mutation flag

`call.cas` was `true` for every write and delete — including
`std.E.delete($id)`, which has no `rev` anywhere. So the flag meant "is a
mutation", the consumer found the actual revision parameter by matching
`name === 'rev' && type === 'int'`, and `cas_miss` sat in the refusal list of
71 ops that cannot raise it.

**Ruled: `statements[].cas_param` names the parameter each statement asserts
as the row's revision, and `call.cas` and the `cas_miss` affordance both
derive from it.** Read off the braces, which is where DL §6 rule 9 puts the
assertion: `split_rev` hands `rev: $r` to `check_cas` and `rev: null` to the
create-once test, and only the first is a compare-and-swap. In the market
fixture 18 ops keep the flag and 71 lose it; every one that loses it is a
create, a put, a delete, or an addressed converge that never named a `rev`.

CAS-by-default is **not weakened by this**. Every op that can compare-and-swap
still does, by declaration; what changed is that the document stopped claiming
it of ops that could not. The refusal list is the sharper half of the point:
`refusals` is documented as *"every entry derived from a stated declaration …
a class the declaration cannot prove reachable is omitted, because a UI that
renders a 'duplicate' affordance on an entity with no unique key teaches its
author that the affordance means nothing"*. A refetch-and-remerge dialog
wired to a `cas_miss` that cannot happen teaches exactly that.

Two boundaries recorded rather than guessed at. A `rev: <int literal>` would
evaluate and is `cas_param: null`, deliberately — the question the key answers
is *which value must a runtime carry over from the fetched row*, and a
statement pinned to a constant asks the caller for nothing; it is also a form
no document spells. And a `del` has no braces to put a `rev` in, so no delete
is ever a CAS.

### A single-row op has no second page, and now says so

`single_row` ops advertised `terminal: [fetch, live, pages]` and
`paged: true`, so the call contract and the result contract disagreed about
whether another page exists.

**Ruled: the executor decides, and it already had.** `Executor::read_page_as`
finds a keyless `reduce` unpageable (no ordering, so no cursor to seek by),
falls back to an unpaged read, answers `next: null`, and refuses a cursor
presented against one as `invalid_cursor`. So a single-row op is `fetch`/`live`
with `paged: false` — 17 ops in the market fixture — and `.pages()` on a
metric is refused from the descriptor rather than at the door.

`invalid_cursor` **stays** in those ops' refusal list, and that is not an
inconsistency: the op does not page, and presenting it a cursor is still a
client error the door answers. What the descriptor stopped doing is inviting
one.

### `refinable` was two answers to one question

`call.refinable` was `true` for every read (the structural fact: a read
accepts the envelope's shape) while `refine.tier` was `none` for every op (the
permission). Both defensible alone; together they disagreed on every op in
every emitted document, and the consumer had to ship a README paragraph saying
which to believe.

**Ruled: `call.refinable` is `refine.tier != "none"`.** One fact, projected,
so the two cannot disagree. The structural half was never a key and does not
need to be: it is `kind == "read"`, which is right there, and that is what
decides whether to draw sort chrome.

### Bounds: `max` had a producer, `min` had one too and nobody carried it

The reported gap was *"`params[].max` never populated and no `min`"*. Both
halves turned out to be about producers, in opposite directions.

**`max` was never dead.** `ParamDecl::max` is produced by
`declparse::param_bound` (`<= <literal>`, admitted on `int`/`money`/`date`
only) and rendered by the artifact. It read as dead because the market fixture
declares no bound, so every one of the 165 ops in the golden showed `null` —
which is also why the consumer's `typeof max === 'number'` check was dead code
that looked live. The emitter canonicalises every bound to **exact digits as a
string** (`"100"`, `"12.50"`, `"2026-03-01"`), because a `money` ceiling
through a JSON float is a rounded ceiling; the type is narrowed to
`string | null` and the bounded-program test now asserts the value rather than
only the affordance it implies.

**`min` had a producer, and it was in the wrong crate.** `ordo_host::
param_bounds` derived `min: "0"` for a `take` count and handed it to the door,
which enforces it — `LIMIT -1` is a Postgres error and `take[keys] -1` is a
silently empty page. Nothing else knew. So the artifact and the descriptor
carried a ceiling and were silent about a floor the deployment was already
refusing below.

**Ruled: `take_count_param` moves down to `ordo_plan::artifact` and
`ordo-host` calls it.** One producer, two readers, the rule the field-scope
manifest follows. `min` is emitted as a canonical digit string beside `max`,
and `bound_violation` now derives from either.

There is **no declared lower bound and this document does not invent one**:
`param_bound` refuses `>=` by name (*"the language has no `>=` and no range
form"*; X-077's range form is proposed, not settled), so `min` is
derivation-only. The witness test asserts that `>=` still does not parse —
the day it does, `min` stops being derivation-only and the document owes the
declared form too.

Two things found here and **not** fixed, recorded as owed:

- **`text(n)` does not parse.** DL §2 writes `$payee: text(64)` and
  `param_bound`'s own error message says *"a `text` parameter's bound is its
  length, `text(64)`"* — but `scalar_ty` reads the bare word and returns
  `ScalarTy::Text`, which carries no length, so the form is a parse error at
  the following token. The spec's example does not compile.
- **A platform op's text length reached no document.** `ParamTy::Text {
  max_len }` is enforced at the door and `platform_ops::manifest` emitted
  `"max": null` regardless, so two of the three platform ops claimed unbounded
  about a parameter the door measures. Fixed here (the manifest now states the
  length); it is unambiguous because `<=` on `text` is refused, so a `max` on a
  `text` parameter can only ever be a character count.

### The one place honesty meant contradicting a settled doc

`api-engine-design.md` §3 says, and `std-library.md` §2 and
`definition-language.md` §4 repeat: *"Addressed writes return the written row
(post-defaults, post-`rev`) as `rows[0]`; bulk writes return affected-id
descriptors."* The consumer reported the descriptor's write `result` as empty
and asked for the row's shape, so a form could know it gets back the `rev` the
next CAS write needs.

**The executor does not do that.** Every write and every delete — addressed or
ranged, create or converge — answers with one summary object per statement, in
statement order: `{entity}` plus exactly one of `created`/`converged` (the row
id) or `updated`/`deleted` (a count). The mutation carries no `RETURNING` that
reaches the caller; the post-image *is* re-read, but only into the command
log's `after` field.

**Ruled: the descriptor states the executor's answer, and the contradiction is
filed against the engine rather than smoothed over here.** `result.shape` is
`write_summary` for a compiled write, `rows` for a read — and `rows` for a
*platform* write, which does not go through the write path and does return
declared columns, which is why the shape is a producer's fact and not
something derived from `kind`. `statements[].summary` states the keys and
whether the value is an id or a count, per statement, because a transfer that
creates one row and converges another puts two different shapes in one
response.

The general rule this is an instance of: **a descriptor is derived truth about
what the deployment does, not about what the spec wishes it did.** A document
that promised §3's row because §3 promises it would be a document that lies in
the flattering direction, and the renderer would find out by reading
`rows[0].rev` and getting `undefined`. Consequence for the runtime, stated
plainly: a commit does not hand back a `rev`, so a form that commits twice
re-reads through `get` in between.

**Owed, and it is an engine change, not a descriptor one:** either the
executor returns the addressed row as §3 says, or §3 (and `std-library.md`
§2, and `definition-language.md` §4) is amended to describe the summary. The two documents may not both stand.

### Found on the way, that nobody was looking for

- **A write refused by rule 8 reads as callable.** `claimCoupon` covers two
  unique keys, the fixture comments it *"Refused at compile time"*, its mode
  label is `ambiguous-identity` — and its artifact `error` is `null`, because
  the artifact records only read-bind failures. So the descriptor tells a
  renderer the op is fine and the door refuses it. `renumberEntryOnce` is the
  same shape. Left as a finding: a runtime cannot pre-empt it, and closing it
  means surfacing write-registration refusals into `error`, which moves the
  golden well outside this work.
- **Every op's version hash moved, and that is correct.** `result.shape` is a
  new disclosed fact on all 165, so all 165 moved. What did **not** move:
  `plan.sql`, `scope`, `row_scope`, `guards`, `cost`, or any entity. A
  capability grant pinned to an old manifest must be re-consented, which is
  what the pin is for.
- **The `id_types` name set closure is currently inert.** The index now names
  an entity whose rows a visible op returns even if no visible op takes its
  id — necessary for the listing half to be total, and in the market fixture
  it adds nothing, because every listed entity already had an id-typed
  reference. Recorded so the next reader knows it was deliberate rather than
  untested.

---

## A frozen encoding grows a field: the owner-administration flag, by presence (2026-09-07)

DL §3 bootstrap rule 6 `[settled]` says an owner's acl administration is
*"logged with the owner as actor and **flagged as owner administration**"*.
The mechanism shipped with `8z`; the flag did not, because it "had nowhere to
live in `CommandGroup`". The owner approved adding the field on 2026-09-06,
and the question that had to be ruled before it could be added was not where
to put it but **how to encode it**.

The constraint is real and it is the reason this sat unbuilt. `chain.rs`
carries a frozen, deliberately unversioned canonical layout, and the chain
hashes *the rendering*. A field encoded the obvious way — one byte, `0x00` or
`0x01`, always present — adds a byte to the rendering of every group ever
written, including the ones written before anybody thought of the field.
Every chain hash moves. Every signed checkpoint over those hashes stops
verifying. Every golden that pins one has to be "updated", which is the
sentence that turns an audit spine into a set of numbers somebody edits when
they stop matching.

**Ruled: the flag is encoded by *presence*, not by value.**

```text
group   := str correlation ‖ str actor ‖ u64 count ‖ command* ‖ trailer?
trailer := 0x01                          # owner administration, and nothing else
```

An unflagged group renders exactly the bytes it rendered before the field
existed, so it hashes exactly where it did. A flagged group renders one more.
There is no format version, no migration, and no fallback path, because the
old bytes are not an old format — they are a lawful current encoding of a
group whose flag is false.

Three properties make it safe rather than merely convenient:

- **The compatibility is proved by a number, not by an argument.** The chain
  hash of an ordinary group was computed with the encoder that had never
  heard of a trailer, written down, and pinned as a constant
  (`PRE_FLAG_GOLDEN`) *before* the field was added. A derived golden would
  have proved nothing about the previous code. It still matches, along with
  the byte length, which is the more legible failure: a hash that moved says
  "something changed", a length that moved says "you added a byte".
- **Canonicity survives.** Exactly one byte names a trailer. An explicit
  `0x00` "no flags" trailer is refused as `TrailingBytes` — "no flags" keeps
  its single spelling, silence — so one group still has exactly one
  rendering, which is the property a hash chain cannot lose. Every other
  padding byte gets the answer it always got; only the one byte that gained a
  meaning changed one.
- **The flag is inside the hash.** That is the whole point of putting it in
  the encoding rather than in a column beside it. An owner-administration
  record that whoever holds the database could quietly clear, or forge, would
  answer the auditor's question with whatever the last writer preferred.

**No column, and not as an economy.** `$commandlog.body` *is* the canonical
encoding, so a field inside it needs no DDL. A denormalized column would have
been a second, unhashed answer to a question the body already answers under
the hash — and it would not have worked: the platform tables are created with
`CREATE TABLE IF NOT EXISTS`, which does nothing to a table that already
exists, so the column would be absent on every deployment that has ever
booted and every insert against one would fail.

**The corollary the ruling forced, found rather than designed.**
`plan-export.md` §2.3 promises that a reader holding nothing but
`commandlog.jsonl` can recompute `SHA-256(prev ‖ body)` and check the links
*without the platform*. That promise is only kept if every byte of the body
is recoverable from the exported line — so the flag had to travel with the
export, or a flagged group would re-encode a byte short and an outside
verifier would read a lawful group as tampering. It travels by presence
there too, which has a second use: `grep ownerAdministration` over an
exported chain is the list of every rule-6 command in it.

**What the flag is not.** It is not an authorization input, and nothing reads
it as one. Rule 6 is decided at the door, once per call, and the call then
runs against a schema view whose acl entities carry no guards — so the flag
records a decision already made rather than participating in one. It is
produced in exactly one place, the door's rule-6 arm, and threaded to the
single `CommandGroup` constructor as a parameter, so a rule-6 group is
*constructed* flagged rather than flagged afterwards.

Two absences are deliberate and both are asserted. A **revert never inherits
it**: the flag says how *this* call was authorized, not what it undid, and a
revert is a separate call by a separate actor that rule 6 did not authorize.
A **platform membership command is never flagged**, whoever issues it,
because rule 6 is not what let them.

The general form, worth keeping for the next field somebody wants in a frozen
layout: **an optional, presence-encoded suffix is the only extension a hashed
rendering admits for free**, and it is free only while "absent" is the value
every existing record would have had. A field whose default differs from what
history implies cannot be added this way at all — it has to be a new record
type, because the alternative is rewriting what the old records say.

---

## The decompiler grows `append`, and where a union's subset ends (2026-09-07)

The owner pasted a many-branch feed at the reverse pane:

```sql
SELECT 'order' AS kind, o.number AS title, o.placed AS at FROM orders o WHERE o.customer = $1
UNION ALL
SELECT 'refund', r.reason, r.issued FROM refunds r WHERE r.customer = $1
ORDER BY at DESC LIMIT 100
```

and got `outside the v0 subset: expected an identifier, found Str("order")`
— a Rust `Debug` of a token, printed on the surface whose entire job is
making a translation visible. It named neither of the two constructs it
could not translate, and both of them have exact Ordo spellings.

### The two, translated

**A literal select column is a literal projection entry.** `'order' AS kind`
is `kind: "order"`; the forward compiler takes it and plans `'order' AS
"kind"`, the same constant column the SQL emits. The escaping goes through
the same closed set (`\" \\ \n \t`) the condition literals already used —
SQL doubles a quote to escape it and Ordo's strings are double-quoted, so
`'it''s'` is `"it's"` and `'x"y'` is `"x\"y"` — and this is the one path
where a literal the reader typed becomes a VALUE RETURNED rather than a
filter, so a re-lexing bug here is a wrong answer, not a wrong row set.

Which literal KINDS are supported was decided by asking the forward compiler
and then Postgres, not by taste. String, integer, decimal, boolean and
`NULL` are all accepted in a projection and all agree row-for-row; each has
a differential case. A literal with **no `AS` name** is refused by name in
the first arm — SQL calls it `?column?` and an Ordo projection entry is
`name: value` with no unnamed form, so any name the emitter picked would be
a column the SQL did not produce — and expected to have none in a later arm,
where SQL takes the name from the first SELECT.

**`UNION ALL` is `append`.** Arm 1 becomes the pipeline's source line and
each later arm an `append <source>[…]` line; the trailing `ORDER BY` and
`LIMIT` become `sort` and `take` after the last one, which is where SQL puts
them too — a trailing ORDER BY after a set operation applies to the whole
union in both languages.

The interesting half is the **name matching**, because the two languages use
opposite rules and this is exactly the shape where "close enough" would
silently swap two columns:

> SQL names a union's columns from the FIRST SELECT and matches every later
> arm by POSITION. Ordo's `append` matches by NAME (DL §5).

So each later arm is emitted projecting position *k* under arm 1's name for
*k*: `SELECT 'refund', r.reason, r.issued` becomes `append refunds[kind:
"refund", title: reason, at: issued]`. That is not an approximation of SQL's
rule — it *is* SQL's rule, written in the language that only has the other
one, and the value SQL would have put in each column is the value Ordo puts
under that name. A later arm that gave a column its own, different `AS` name
is disclosed in a note, because SQL discards that name and a reader watching
`heading` come back as `label` deserves the sentence.

Ordo then adds one demand SQL does not: **the types must agree exactly**, no
coercion and no null-filling. SQL resolves a union column's type across the
arms (`int` against `numeric` is `numeric`; a bare `NULL` takes the other
arm's type) and `append` refuses. That check runs here, where the message can
name the column and both types, rather than arriving from the forward
compiler as `append_misalignment` at line 2 of source the reader never wrote.

### Plain `UNION`: supported, and here is the evidence

`UNION` without `ALL` dedupes. Ordo spells that `append … distinct`, and the
question was whether the two dedupe the *same* rows. Three things had to hold
and all three were run on Postgres rather than argued:

1. **The same columns.** SQL's `UNION` dedupes over all output columns;
   Ordo's `distinct` dedupes over the projected outputs, which after an
   `append` are exactly those columns. R-C16 (refs compare and sort by their
   id) makes a ref column the same value on both sides.
2. **Nulls compare equal.** SQL's set operations treat two nulls as the same
   value for deduplication, and so does `SELECT DISTINCT`, which is what the
   planner emits. The case that proves it dedupes a nullable ref column that
   is null in both arms.
3. **The collation does not change the partition.** Ordo's `distinct` pins
   `COLLATE "C"`; SQL uses the database's. Under a **deterministic**
   collation — Postgres's default, and this database's — equality is byte
   equality either way, so the two partition text identically. Under a
   nondeterministic collation they would not, and there is no Ordo spelling
   for one; that is disclosed in a note on every plain `UNION`.

Four differential cases dedupe within one arm, across two arms, and on a
key that is null on both sides. All four agree, so `UNION` is **supported**.
Had any disagreed the ruling would have been the other way — a refusal by
name — and the note would have been the wrong instrument, because a dedupe
that keeps a row SQL dropped is a different row set.

`INTERSECT` and `EXCEPT` stay **refused by name**. Ordo's only set operation
is `append`; the semijoin/antijoin that would stand in for them is `match
any` / `match none` over a *declared ref*, and a two-SELECT `INTERSECT` does
not carry enough information to reconstruct which ref that would be.

### Where the union subset ends, each refused by name

- **An entity twice.** DL §5: an entity may appear ONCE as a source per
  pipeline. A `UNION` of a table with itself has no Ordo spelling.

  *This is the one refusal in the file that is not a divergence refusal, and
  it says so in its own message.* The forward compiler currently ACCEPTS
  `orders[…] append orders[…]` and the rows agree — the rule is in the DL and
  not in `ordo-sema`. The refusal stands because the emitted text would not
  be Ordo, and pretending it were about rows would be the same species of lie
  the 2026-09-05 sweep closed. A test
  (`the_self_union_refusal_is_a_language_rule_not_a_divergence`) pins that
  reading and fails if the rows ever start differing, at which point the
  message needs rewriting rather than the refusal.
- **A per-arm `ORDER BY` or `LIMIT`.** The parentheses in `(SELECT … ORDER BY
  x LIMIT 2) UNION ALL …` exist precisely so an arm can carry them, and an
  `append` arm is ONE SOURCE FORM (DL §1: "never a nested pipeline") with no
  steps at all. A branch that needs steps is a `let`, which is a different
  pipeline rather than a rewriting of this one. In the refusal oracle: the
  translation that ignores the parentheses returns the rows the LIMIT existed
  to exclude.
- **A per-arm `GROUP BY`, aggregate, or `SELECT DISTINCT`**, for the same
  reason. Moving a per-arm `DISTINCT` to the whole pipeline also collapses
  rows the SQL keeps — likewise in the oracle.
- **A mixed `UNION` / `UNION ALL` chain.** SQL's set operators are
  left-associative, so the dedupe applies to everything left of it; Ordo's
  `distinct` is a step and can only dedupe the whole pipeline. There is no
  place to put it that means the same thing.
- **Arm width and type mismatches**, named here rather than by Postgres or by
  the forward compiler.
- **A qualified or unknown `ORDER BY` key** after a union. SQL does not allow
  a qualified one there either; the message lists the names the union does
  have.

One note had to be *rewritten* rather than reused. The single-source `LIMIT`
note says "Ordo appends an implicit `id` tiebreak". After an `append` it does
not — the arms have different ids — and the real tiebreak is the arm, then
the projected columns, then a per-arm row number. Restating the id version
would have been a false claim about which rows a tied `LIMIT` boundary keeps.

### The tokenizer-soup class, audited

`expected an identifier, found {token:?}` had one site and a surprising
number of well-formed-SQL routes into it. The audit turned up six more
besides the owner's literal, each now a named refusal:

| What a real query wrote | What it used to say |
| --- | --- |
| `WHERE x = $1` (a bind parameter) | `not a legal Ordo path segment` |
| `WHERE x = ?` / `:name` | `character '?' is outside the v0 subset` |
| `ORDER BY 1` / `GROUP BY 1` (an ordinal) | `expected an identifier, found Num("1")` |
| `SELECT t.*` | `expected an identifier, found Sym("*")` |
| `SELECT upper(x) AS u` | `expected FROM` — naming the *next* clause |
| `x::text` | `character ':' is outside the v0 subset` |
| `CASE WHEN …` | `expected FROM` |

`$1` is worth its own sentence: it is what every *pasted application query*
contains, and the pane decompiles to a standalone pipeline, which has no
parameter list — Ordo's parameters are declared on an `op`. The refusal says
that, and says the `op` form is what the query would look like with one.

`SELECT t.*` is routed to the existing `SELECT *` refusal, which already
argues the case (every physical column against the declared open fields) and
hands back the explicit projection. Only a `*` qualified to a JOINED alias
gets a separate message, because the far side of a hop has no whole-row form
at all.

### What the sweep found that nobody was looking for

**`WHERE x = NULL` was emitting `x == null`, which is the opposite
predicate.** The condition parser's column-to-column branch accepted any word
that was not `true`/`false`/`DATE`/`TIMESTAMPTZ`, so `NULL` was read as a
*column name* and rendered as an Ordo path. SQL's `= NULL` is three-valued:
unknown for every row, null ones included, so it matches NOTHING. Ordo's `==`
is `IS NOT DISTINCT FROM`, so `x == null` matches EXACTLY the null rows. Not
a near miss in either direction — zero rows against four on the shipped seed,
emitted in silence, with `<> NULL` wrong the same way. Refused by name now,
with the working spelling (`IS NULL` / `IS NOT NULL`) in the message, and in
the refusal oracle.

It is the same species as the defects the 2026-09-05 sweep closed, and it
survived that sweep because nothing in the corpus or the differential set
compares against a null literal — which is the point the sweep itself made
about `a AND b OR c`: a divergence the corpus does not reach is invisible to
a row oracle until somebody writes the query.

**`ordo-sema` does not enforce DL §5's once-per-pipeline rule.** Recorded
above and owed elsewhere; the decompiler refuses the shape, the forward
compiler compiles and runs it. This is the second entry on that list, after
"`ordo-sema` still accepts a projection with two outputs of the same name".

## Two things the frozen chain encoding was thought to forbid, and only one of them does (2026-09-08)

Planning the production-operations trio (`plan-production-ops.md`) turned up
two places where a document had reasoned from the chain's encoding to a
conclusion the encoding does not support. One of them was wrong and the
mechanism it blocked is buildable; the other was right, and the thing it
blocks needs a different shape than the document assumed. Both are recorded
here because in each case the *sentence* is true and the *inference* is what
drifted — which is the failure mode a grep does not catch.

### 1 · A historical payload can be re-sealed without touching the chain

`security-logging-design.md` §7 pins, under *"Epoch keys survive at least
the undo horizon"*:

> A `SealedRef` resolves only while the epoch key that sealed the payload
> still exists, and the chain hashes the ref, so a re-seal pass **cannot**
> rewrite historical before/after images: rewriting one would change the
> group's canonical encoding and break the chain from that position forward.

`implementation.md`'s key-custody section repeats the inference in its own
words, and both then conclude that a rotation's post-compromise recovery is
necessarily **delayed by the undo horizon**, because the historical images
sealed under the old epoch cannot be moved.

Every clause quoted above is true. The conclusion does not follow.

The chain hashes the **ref** — sixteen bytes, and only those:

```rust
Value::SealedRef(h) => out.extend_from_slice(h),   // chain.rs:291
```

The payload bytes are not in the canonical encoding, have never been in it,
and cannot be: the encoding's `value` production gives tag `5` exactly
sixteen raw bytes (`chain.rs:19-25`). Re-sealing a `$sealedpayload` row **in
place** — same `("$space","ref")` primary key, a new envelope in `payload`,
a new `epoch` — rewrites no image, changes no group's canonical encoding,
and moves no hash. It is not a rewrite of history; it is a rewrite of what
history *points at*, and the handle is a stable indirection precisely so that
distinction exists.

**Ruled:** a re-seal pass covers historical payloads as well as live rows.

The pinned rule itself — *an epoch key may not be destroyed while any
command image within the space's `undoHorizon` is tagged with it* — **stands
unchanged**, and this ruling does not weaken it. What changes is which kind
of thing satisfies it. "Tagged with it" is a property of `$sealedpayload`,
and a worker can drive that count to zero; so the gate is a state the
deployment can *reach*, not only a clock it waits out. `epoch_count_sql`
(`ordo-crypto/src/sealed.rs:208-217`) already exists to ask the question,
and the `("$space","epoch")` index was put there for exactly this reader
(`sealed.rs:60-67`).

Two constraints came with it, and neither is optional:

- **The verification digest must not be over plaintext.** F7's conversion
  job is *"verified by count plus content hash per entity"*, and a durable
  hash of a sealed value is the unsalted permanent equality index §7 rejects
  the content-addressed `SealedRef` for — it would survive crypto-erasure
  for the same reason. So correctness is checked in memory per row (open
  under `n`, seal under `n+1`, open, compare, discard) and the durable chunk
  checkpoint digests `(ref ‖ new-envelope-bytes)`. Every seal draws a fresh
  nonce, so two rows holding equal plaintext contribute unrelated bytes: the
  digest proves a chunk was processed and proves nothing about what was in
  it.
- **A re-seal needs no write barrier**, unlike every other backfill in the
  system, and for a reason worth keeping: writes always seal under the
  newest epoch, so a row written during the pass is already in the state the
  pass wanted. The only hazard is clobbering it, and a compare-and-set on
  the old envelope bytes closes that exactly — the fresh nonce makes those
  bytes unique to one write, so zero rows updated means "the row moved,
  skip it". A rotation therefore does not freeze writes to sealed fields,
  which matters because rotation is mandatory on every membership
  revocation.

What the ruling does **not** reach, stated so it is not read as more than it
is: a backup or an archived log bundle. Re-sealing the live database cannot
touch either, so `implementation.md`'s *"rotate then retire = post-compromise
recovery, at the cost of old archives"* survives intact. The destruction
confirmation prints it as a date.

### 2 · `system:restore` cannot be a command group — and must still be a chain entry

`implementation.md` says the platform *"appends a `system:restore` command
group as the first group after the restored head"*. Taken literally that is
unbuildable, three times over: `CommandGroup::validate` refuses an empty
group (`GroupShapeError::EmptyGroup`, `command.rs:189-195`); `CommandKind`
is closed to `Create | Converge | Delete` and every command carries an
entity, a row id and images whose shape is enforced (`command.rs:100-104`,
`:130-146`); and forging a `Converge` with equal images to record a non-write
is the move `exit_ops.rs` already refused in as many words — *"would put a
lie in the chain — and the chain's whole value is that a reader may take what
it says literally."*

Here the inference is right and the exit ops' **answer** is what does not
transfer. Export and deletion went to the security-event stream because the
chain could not hold them and nothing needed it to. A restore is the opposite
case: the entire requirement is that `verify` — which walks the *command*
chain — understands the seam. A marker in another stream leaves `verify`
reporting `Unreadable` at the rewind, which is today's behaviour and the
thing being fixed.

**Ruled:** the restore marker is a chain entry, and it grows the frozen
encoding **by presence**, on the precedent set eight days earlier by the
owner-administration trailer (`chain.rs:39-63`). A second trailer byte, on a
group whose command list is empty; `EmptyGroup` is relaxed **only** in that
trailer's presence, so a write that produced nothing is still refused, which
is the failure `EmptyGroup` exists to catch. Unflagged groups re-encode
byte-identically, so every existing chain and every signed checkpoint over it
is untouched — the same identity the first trailer holds by construction and
pins with a pre-change golden.

Two consequences fall out:

- **The operator is a field, not the actor.** Workers attribute as
  `system:<component>` `[pinned]`, so the marker's actor is
  `Actor::System("restore")` — the first production producer of that variant
  for any group — and the human who ran the restore travels in the payload
  beside the step-up correlation. This is the same split the delegation
  design already made when it kept the delegation id out of the actor
  rendering.
- **Detection was never the missing half.** `verify` already catches the
  rewind: a stored head below a signed checkpoint's `upto` is
  `TamperKind::Unreadable` at `last + 1` (`undo.rs:499-503`). What is
  missing is a *lawful spelling* for what it found. So the marker does not
  get a `TamperKind` variant — a documented event must not share a type with
  an alarm, given exit code 3 pages the security team by design — and
  `verify` instead learns never to anchor above a marker's restored-from
  position. Without that rule the truncation check fires on every lawful
  restore, which is the "documented event read as an alarm" the spec is
  trying to prevent.

### What both have in common

Neither was a wrong fact. Both were a true sentence about the encoding
carried one step further than the encoding licenses, and then repeated in a
second document in slightly different words — which is how it stopped being
checkable. The general rule the two suggest: **when a design argues from the
canonical encoding, quote the production.** `value := tag:u8 ‖ payload` with
`5 SealedRef  16 raw bytes` settles the first question in one line, and the
trailer's by-presence rule settles the second; neither needed a judgement
call once the bytes were on the page.

---

## Two seams that looked like injection and were not (2026-09-08)

Both surfaced while planning the server binary (`plan-runtime.md`). They
share a shape: an interface that *has* the seam a design asks for, and a
construction site one layer away that quietly supplies the wrong thing —
so every review of the interface passes and the property is still absent.

### 1 · A program's custody and its space are inputs to loading it, never properties of its compilation

`KeyProvider` was built so that key material never leaves the trait and so
that a provider is **injected** rather than constructed at call sites. The
module doc names the three failures the old `fn smk(&self) -> &[u8]` shape
caused, the first of which is *"No injection seam. Callers constructed
`TestProvider` inline, so there was nothing to swap a real provider into"*
(`crates/ordo-crypto/src/custody.rs:9-13`). `Fixture::keys` is the seam that
answered it, and it is documented as such: *"Key custody, INJECTED rather
than constructed at call sites. This is the seam F0's done-when asks for"*
(`crates/ordo-conformance/src/lib.rs:227-231`).

The seam exists. Nothing on a production path sets it.

`deploy::verify_candidate` — the only way a deployment loads a program —
takes `keys: Arc<dyn KeyProvider>` and threads it into the
`ordo_exec::Executor` it builds (`crates/ordo-host/src/deploy.rs:346-352`).
That covers reads. It does not cover **writes**, because the write path does
not go through `Executor::keys`: `write_leased` hands the *fixture* to the
Stage-5 protocol (`crates/ordo-exec/src/lib.rs:928` —
`fixture.run_write_on(&db, …)`), whose `impl` block is `impl Fixture`
(`crates/ordo-conformance/src/write.rs:333`), and every seal, blind-index
digest and sealed-handle mint in it reads `self.keys`.

And the constructor `verify_candidate` reaches — `Fixture::compile_source` —
hardcodes that field:

```rust
// crates/ordo-conformance/src/lib.rs:318
keys: std::sync::Arc::new(ordo_crypto::TestProvider::with_space(FIXTURE_SPACE)),
```

So a deployment holding a real provider reads through the operator's custody
and **writes sealed values under `b"ordo-test-smk-00"`** — a constant
published in this repository at `custody.rs:737`. The same line pins
`space: fixture_space()`, so tenancy arrives from the compiler too.

Nothing here is a wrong fact and no reviewer of either file could have seen
it. The trait is right; the field is right; the argument threading is right
as far as it goes. What is wrong is a **constructor that mints custody**, and
the failure mode it produces is the worst available: not a refusal to boot,
but a deployment that works, round-trips correctly, and hands its ciphertext
to anyone who has cloned the repository.

**Ruled:** custody and tenancy are **inputs to loading a program**, never
outputs of compiling one. A compiler is a pure function of source text; a
constructor that mints key material makes the same source text mean
different key material depending on who called it, which is precisely how a
test key reached a production write path with nobody having made a decision.
`verify_candidate` assigns both from its arguments before the fixture is
shared, and asserts `Arc::ptr_eq` on the way out.

`TestProvider` keeps its constant. Its own doc is right that *"nothing it
seals is secret, and looking secret would be worse than being plain"*
(`custody.rs:723-725`) — that sentence is true exactly while nothing
production can reach it, and it was not.

**The general rule the two halves suggest:** *an injection seam is only a
seam if the construction site is on the other side of it.* A `pub` field
with a doc comment saying "injected" is a seam a caller **may** use. The
property is held by the code that constructs the value, and if the only
constructor supplies a default, the seam is documentation.

Two independent mechanisms now hold it, the arrangement the sealed-handle
design already uses for scoping (`custody.rs:290-296`): the assertion inside
`verify_candidate`, and the server binary's own check at boot **and on every
swap** — a swap compiles a fresh fixture, so a boot-only check would catch
the first program and no other.

### 2 · A session-scoped Postgres lock is held on a connection the pool does not own

`TxConn::try_lead(realm)` runs `pg_try_advisory_lock` and its doc is exactly
right about the lock: *"A session advisory lock is held for the life of this
connection and released when it closes, so a dead leader's claim does not
outlive its process"* (`crates/ordo-exec/src/jobs.rs:210-217`). Every word
is true. The hazard is in where the connection goes next.

`TxConn::commit` pushes its client back onto the pool's idle list
(`crates/ordo-exec/src/lib.rs:525-532`), and `rollback` does the same. So a
leader elected through a pooled `TxConn` leaves a session lock on a
connection that immediately returns to serving unrelated queries, and the
lock is released only when that connection dies. Within one process the
effects compound in the confusing direction: advisory locks are re-entrant
per session, so a re-election that happens to borrow the *same* connection
succeeds while one that borrows a different one fails — permanently, for the
life of the process, with no error anywhere.

Nothing calls `try_lead` outside its own test
(`crates/ordo-exec/tests/jobs.rs:119-126`), which is why it has never bitten.
It would bite on the first supervisor tick of the first replica.

The neighbouring module had already reasoned this out for a sibling case and
the conclusion did not travel. `jobs::listen`'s doc explains that `LISTEN` is
**session state** and that *"a pooled connection goes back to the idle list
the moment a query finishes, so a `LISTEN` issued on one would follow an
unrelated request around the pool or vanish with a re-dial"*
(`jobs.rs:262-268`) — and it therefore *"owns exactly one dedicated
connection for the life of the process."* A session advisory lock is session
state in the same sense and wants the same answer.

**Ruled:** session-scoped Postgres state — `LISTEN`, `pg_advisory_lock`, and
anything else whose lifetime is the connection rather than the transaction —
is held on a **dedicated connection the pool does not own**. A leadership
handle is a struct holding its own `tokio_postgres::Client`; dropping it
releases the lock, which also makes the release an explicit step at shutdown
rather than a TCP timeout the replacement replica waits out.

`try_lead` is not changed: the primitive is correct and tested, and it is the
*caller* that owes the connection. It gains a doc note saying so.

**What keeps this from being serious**, stated because the reading is easy to
get wrong in the alarming direction: leadership is not load-bearing for
correctness. `claim_schedule`'s doc pins it — *"This is the fires-once
property, and it does not come from leadership"* (`jobs.rs:130-136`) —
`FOR UPDATE SKIP LOCKED` plus the committed `next_due` advance is what makes
firing exactly-once, and `fire_clock` repeats the distinction at its own call
site (`crates/ordo-exec/src/triggers.rs:798-804`). A supervisor whose
leadership is broken is wasteful, not wrong. That is the right way round for
a mechanism nobody has run yet.

### What both have in common

Neither is a bug in the thing being reviewed. In both cases the interface
holds the property, its documentation states the property correctly, and the
property is lost at a construction site in a different crate that no reader
of the interface has any reason to open. The first was found by asking *which
`keys` does a write actually use*; the second by asking *where does this
connection go after commit*. Both questions are of the same shape: **follow
the value past the boundary that documents it.**

---

---

## Generator-first: two TypeScript packages, one producer (2026-09-07)

Checklist **14d** (the generated TS SDK) and **14f** (`@ordo/cdk`, the typed
builder) landed together, out of one crate — `ordo-gen` — because the owner
ruled the architecture before either was built: both outputs are **generated
from the implementation's own canonical tables, never hand-maintained**, so
language evolution is regeneration and never rewriting. *"Just regenerated
from the source of truth — being our implementation — and every iteration
it's just a quick gen button."*

This records what that costs, what it buys, and the four places where
building it forced a decision the plan had not taken.

### One generator, because the alternative was two walks

The SDK's honesty rule is that it is *a pure function of the interface
descriptor*. The CDK's, from Track C, is that *"the builder API is generated
from the same grammar definition the parser uses — it cannot drift and cannot
express more than the language."* Those are two different sources of truth (a
deployment's `interface.json`; `ordo-syntax`'s closed tables), and the
tempting reading is that they are therefore two projects.

They are not, and the reason is the one `ordo_plan::interface` already gives
about itself: **two walks over the same compiler output are two answers
waiting to differ.** The CDK's *schema* layer — which entities and ops may be
sources, which fields `f.` completes, which refs it follows — is a function of
exactly the descriptor the SDK consumes. Two crates would each have grown a
descriptor reader, and the day `id_entity` or `enum_values` changed meaning,
one of them would have been updated.

So `ordo-gen` takes **JSON in and text out**. It depends on `ordo-syntax` and
`serde_json` and nothing else: it cannot reach a schema, a registry, a binder
or a database even by accident, which is the property that makes "derived
truth" checkable rather than asserted. `ordo gen ts` emits both packages'
generated halves in one run, because a package whose two halves came from two
commands is a package that ships with one of them stale.

### The boundary is *shape versus word*, not *hard versus easy*

A builder cannot be entirely generated. Somebody has to decide that
`q.source(…)` is followed by steps, that `group` takes keys and then an
aggregate object, that a write body returns statements. The line drawn is:

> **A shape is the skeleton's; a word is the generator's.**

That a comparison has a left, an operator and a right is a shape. That the
operators are `== != < <= > >=`, and that they are spelled `.eq() .ne() .lt()
.le() .gt() .ge()`, is vocabulary.

The line is *there* because of what each side costs when it drifts. A word
that drifts is a builder that can express something the language cannot, or
cannot express something it can — caught by isomorphism, and prevented by
generating it. A shape that drifts is an API redesign, which is a human
decision and must not arrive as a diff on a generated file.

Two seams cross it, both in `clients/ordo-cdk/src/core/value.ts`. `Value` is
declared as an interface with **no members** and the generated vocabulary
merges the operators into it, so the skeleton types parameters, paths, sort
keys and write values without naming one operator; and `installValue()` is
the run-time half, so a value typed as having `.eq()` really has it. One
mutable module-level binding, and it is the only one in the package — the
alternative was threading a constructor through forty skeleton signatures,
which would have put the vocabulary's name in every one of them.

### Absence is a decision, and it is published

`spell.rs` matches every closed language enum with **no wildcard arm**, so a
new keyword fails `cargo build` — the generator does not compile, `ordo gen
ts` cannot run, and neither TypeScript package can be regenerated until
somebody writes down what the new word is called. The `ScalarTy` precedent in
`ordo-exec/src/export.rs` states the argument; this is the same argument one
layer out, where the consequence of a silent default is that a word reaches
TypeScript as *nothing at all* and the author of it never learns two client
packages exist.

**"It is not offered, and here is why" is a legal answer**, and it is the one
most declaration keywords get. But it is two answers, not one, and they are
owed differently — the same split `docs_parse.rs` draws between "settled and
unbuilt" and "[lean], not settled":

- **`does-not-parse`** — the *language* refuses the construct, so the builder
  must too. An absent method is the language's refusal arriving one layer
  earlier. `when`, `every`, `trigger`, `routine`, `principal`, `app` and the
  rest are here. Nothing is owed until the grammar moves, and a builder that
  offered one could express what no `.ordo` file can — the isomorphism
  property inverted.
- **`out-of-scope`** — a real debt. Exactly one word is here (`whole`), and a
  test asserts it is the only one, so a second cannot appear without somebody
  deciding whether the README still tells the truth.

Both are emitted into `vocabulary.ts` with their reasons, and
`clients/ordo-cdk/test/vocabulary.test.ts` walks that list and asserts every
*offered* name resolves on the package's surface. That is what carries the
ratchet across the language boundary: `cargo build` fails if a word has no
decision, `npm run check` fails if a decision has no implementation, and the
corpus fails if an implementation says something the parser refuses. Three
gates, and a new word passes all three or none.

### Isomorphism is checked against the parser, not against a formatter

Track C's bar is that isomorphism is *"a conformance-tested property, not an
aspiration"*, and that `synth` emits canonical `.ordo` **via `ordo fmt`**.
There is no `ordo fmt` — Track A1 is unbuilt — so the property was restated
rather than approximated: **`parse(original)` and `parse(synth(…))` must be
the same AST**, spans excepted. Weaker than byte equality, stronger than
nothing, and it is the claim the test actually makes. When `ordo fmt` lands it
becomes the normalizer on both sides and the claim tightens to bytes.

The translation from AST to builder calls is **machine-written**
(`ordo_gen::emit`), and that is the honesty of it. A hand-written set of
builder programs would exercise only the nodes whoever wrote it thought of; a
walk that matches every AST enum exhaustively cannot skip one quietly — a node
kind with no builder spelling either fails the build or is reported by name in
the score. **31 of 31 corpus programs round-trip, with no misses**, over both
conformance fixtures and every `syntax-examples.md` block `parse_program`
accepts.

The loop crosses a language boundary, and `cargo test --workspace` has to stay
green on a machine with no node. The two ways to reconcile that are a Rust
test that shells out and *skips* when node is absent — which leaves the
property unverified on exactly the machines that skip, silently — or an
intermediate artifact both sides check. The second was taken: the synthesized
`.ordo` is checked in, vitest asserts `synth()` still produces it, and cargo
asserts it parses back to the original tree. A stale intermediate cannot hide,
because the emitted TypeScript beside it is a pure function of the same parse
and moves first.

### Found while building

Four, and three of them were the corpus doing its job.

**A subtree delete must be printed back in the brace form.** The parser lowers
`del E { id: $id }` into the condition `id == $id` it means — "one
representation is one behaviour, two could drift" — so the *spelling* is
unrecoverable from the tree, and printing the parenthesized form is ordinarily
correct, because the spec calls both spellings addressed with identical
semantics. It is not correct here: rule 6 refuses `subtree` after a **ranged**
delete, and "ranged" is decided by which spelling was written, not by what the
condition happens to say. The builder printed `del E(id == $id) subtree` and
the parser refused text the builder had produced from a program the parser
accepted. The printer now reconstructs the braces when the condition is
exactly `id == <expr>`, and refuses to print `subtree` at all otherwise.

**A distributed guard walk needs its parentheses.** The builder distributes
guards into disjunctive normal form, as `decl.rs` does and for the same three
reasons. Printing the result flat produces `a and b or c and d`, which the
guard grammar refuses — boolean mixing is parenthesized. Found on
`syntax-examples.md`'s property manager, whose `access write` clause is
exactly that shape.

**`allocate`'s third argument is not an expression, and no arity table could
say so.** `functions()` reports `("allocate", 2, 3)`; the third argument is a
bare mode word the parser checks against `ALLOCATE_MODES` at the written form.
A generator reading only the arity would have typed it as a value and produced
a builder whose output does not parse. The CDK types it as the closed union
instead, and `ordo-syntax/tests/closed_tables.rs` records the exception where
the probe that found it lives.

**`$caller` is a parameter reference that no op declares.** It is an
`Expr::Param` like any other, so a translator that spells parameters
`p.<name>` from the op's own list emits `p.caller` and finds nothing. Caught
by `tsc` rather than silently, and the emitter now spells any undeclared
parameter `param('name')` — but the next ambient parameter will look exactly
as surprising, which is why the reason is written beside the field that holds
the declared set.

*Two producer changes this needed, both made rather than worked around.*
`function_names()` gained `functions()` — the same table with its arities —
and `AGGREGATE_NAMES` gained `AGGREGATE_SHAPES` through an `aggregate_table!`
macro, two projections of one list in the `word_enum!` style. The old note on
the arities said they stayed private because "nothing outside needs them, and
a public tuple would invite a second arity check somewhere else". Something
outside needs them now, and the alternative was for the generator to carry its
own copy — which is *exactly* the second arity check the note feared. A
projection of the one table cannot be that.

---

## The backfill audit encoding: summarised groups, with a per-creator escape to full images (2026-09-08)

The question `plan-schema-evolution.md` §9 left open, now closed by the
owner: **option 2 — summarised batch groups — is the default**, and an app
creator may switch a backfill to **option 1 — full per-row images** when
their domain needs per-row revertibility and they accept the log cost.

The reasoning that makes the switch honest rather than a hedge: the two
options price the same guarantee differently. A summarised group attests
*data before = data now, in a different structure* — an integrity claim
over the transformation, verifiable, small. Full images buy back per-row
`revert` at the price of a log larger than the data it describes. Which
side of that trade is right is a fact about the application (a ledger
wants images; a rename of `name` into `first`/`last` does not), so the
platform ships the cheap-correct default and the creator owns the upgrade.

Mechanically: the choice is **per backfill, declared in the migration**,
and lands in the chain as part of the group encoding — so it can never be
ambiguous later which kind a segment is. The Z-014 collision resolves the
same way: summarised groups extend the settled create-summarisation to
converges *for option-2 backfills only*, and the entry declares itself.
13c builds against this.

---

## Break-glass: no default, and the grant key is the only door (2026-09-07)

Finding 26 of the 2026-09-07 coverage audit: `dashboard-plan.md` settles
break-glass *as shape* — an explicit, time-boxed grant, step-up gated so a
capability token can never satisfy it, command-logged, never a default — but
nothing implements it, and five comments across four modules
(`ordo-identity/src/access.rs:29` and `:552`, `ordo-exec/src/erase.rs:73`,
`ordo-secevent/src/event.rs:401`, `ordo-host/src/exit_ops.rs:26`) list it
beside epoch rotation as an existing `CustodyAction`, so the code reads as
though the capability already existed.

**Ruled: over sealed data there is no break-glass.** The platform offers the
operation only against an explicit, user-provided grant key; that default is
non-negotiable at the platform level. An app author who wants an *implicit*
form — one that does not wait on a human to hand over a key — may build it in
their own domain, on their own custody model; that is their trade, not the
platform's.

The sharpening that keeps "non-negotiable" honest rather than rhetorical:
for a **managed-custody** space the platform holds the keys, so the refusal
is **policy** — the operation is physically possible and refused anyway
without the grant, and the space's consent surface should say so. For a
**client-first** space it is **physics** — the platform never held a key
capable of the act, so there is nothing to refuse in the first place.

This resolves 11-26's "either build it or strike it from those comments":
the five comments described a capability that does not exist in the form
they named, and are rewritten to describe the grant-gated form instead.
Building the grant-gated operation itself remains open work; only its shape
and the platform's refusal to default it are ruled here.

---

## Deployment-scoped ops: the space-free set is four, closed (2026-09-07)

Finding 16: `bootstrap.md` §1.1 settles the `scope deployment` op class — it
binds no space, `$space` renders `unknown_param`, and it authorizes against a
deployment capability rather than a per-space profile — but §8 left the
shape of that capability `[open]`, waiting on a ruling that had never
actually been asked.

**Ruled: the space-free set is exactly four kinds**, and anything new must
argue its way in:

1. **Operator/system tenant management.** Precedent already in code:
   `/health/deep` and `/metrics` are deployment-scoped platform ops,
   authorized with empty entity pins.
2. **Genesis — `std.space.create`.** Space-free by logical necessity:
   there is no space yet to bind.
3. **The cross-space personal surface** — "list my spaces." A principal's
   memberships span spaces, so the query cannot live inside one.
4. **Deployment usage export** (metering M3) — per-deployment rollups for
   the operator.

A third-party app is not a fifth kind: an app executes inside spaces and its
tables live in spaces, so a developer wanting their own custody is a client
of the API, external to the platform, not a deployment-scoped principal.

This closes 11-16's open ruling; the `CapabilityGrant` shape for a
deployment-scoped grant and its wire rendering can now be built against a
fixed list instead of an open question.

---

## Attestation layers 2–4, and the egress kernel underneath them (2026-09-08)

The owner ruled *execute* on attestation layers 2–4 and, in the same breath,
moved the general egress rail to a post-deployment roadmap. Those two
decisions collide, and the collision is a sequencing fact rather than a
question: the attestation webhook **is** an outbound HTTP POST, and 5f's own
note says shipping that POST before §7b's pins exist is the SSRF-into-key-
custody path §7b was written to close. The standing rule — *"egress ships as
the first increment of its first consumer"* — resolves it: attestation is now
that first consumer, so the minimal egress substrate ships with it, scoped to
what attestation needs. The full rail (per-space allowlist UI, notification
sinks, feeds) stays post-deploy, still homed at 17k.

### `ordo-egress` is a crate, and the reason is pin 5

The six pins are cheap to state and easy to erode. Five of them can be
enforced inside a module; the fifth cannot.

> **Egress runs under an identity that holds nothing** — no instance-credential
> access and no `KeyProvider` reachability, *"so a bypass of everything above
> still lands somewhere with nothing to steal. This is the control that
> survives a bug in the other four."*

That is a **process** property, and this platform ships as one process, so the
full sentence is a deployment obligation and is recorded as one in
`deploying.md`. What can be enforced statically is narrower and still worth
paying a crate for: the code that opens a socket to a customer-named host
**cannot name** `ordo_crypto::KeyProvider`, cannot name `ordo_exec::Pool`, and
therefore cannot reach key material or database credentials, because it does
not depend on the crates that define them. `ordo-egress` has **no `ordo-*`
dependency at all**, and `tests/holds_nothing.rs` asserts it by parsing the
manifest.

Parsing the manifest rather than grepping for a `use` is the deliberate half.
The failure mode being guarded is somebody adding
`ordo-crypto = { path = "../ordo-crypto" }` to make one thing convenient, and a
test that reads the manifest fails on exactly that line — where a test looking
for an import would pass until the second commit. The asymmetry is the pin:
`ordo-host` may depend on `ordo-egress`; the reverse is what the test forbids.

The second argument is `implementation.md`'s own: *"one client, one policy, one
place to audit… feed delivery, import/sync bridges, and any future callback
share the egress path rather than each growing their own HTTP client."* A
module inside `ordo-exec` would be reachable only from `ordo-exec`, and the
second consumer would grow its own.

### The pins are constructors and moves, not checks

| § | Pin | Enforced by |
|---|---|---|
| 1 | `https` only, certs validated, TLS 1.2 floor | `HttpsUrl` has one constructor and it refuses every other scheme; `rustls` has no code for TLS below 1.2, so the floor is the library's shape; the `dangerous_configuration` feature is off, so `ClientConfig::dangerous()` does not exist in this build |
| 2 | Resolve-then-pin, connect to the resolved address | `PinnedDestination` is minted only by `resolve_and_pin` and **consumed by value** by `deliver`; not `Clone`, no public constructor, no serialisation |
| 3 | No redirect following | the response reader has no loop and never looks at a `Location` |
| 4 | Signed deliveries | `deliver` takes a `SinkSecret` and there is no overload without one |
| 5 | An egress identity that holds nothing | the dependency graph, above |
| 6 | Per-space consented allowlist | `resolve_and_pin` takes an `Allowlist`; a destination outside it cannot be constructed |

Three of those are worth the second reading.

**Pin 2 is a move.** The usual implementation resolves, checks, and hands a
*URL* to a client that resolves again — which is the rebind window §7b names
and calls *"the half most implementations omit"*. Here the only thing `deliver`
accepts is a `PinnedDestination`, whose only field the dialler reads is a
`SocketAddr`, and the call consumes it. *"Re-checked at connect time"* is not
a discipline the worker observes; it is the only way to get a second delivery,
because the first one ate the value.

**Pin 6 runs before resolution, not after.** A DNS lookup is itself an outbound
request to a name the tenant chose, and a resolver that logs queries is a
channel. An allowlist consulted after the lookup has already performed the
leak it exists to prevent.

**Pin 3 is an absence.** A general HTTP client is built to be *accommodating* —
it follows redirects, retries, pools connections, exposes response bodies — and
several of those are features this crate exists to not have. Turning them off
is a configuration, and a configuration can be changed, defaulted wrong on a
version bump, or overridden by a second caller. The HTTP/1.1 request is
written out by hand (about a hundred lines) so there is no redirect-following
code to disable: a reviewer checking pin 3 greps for `Location` and finds
nothing, which is a stronger review than confirming `redirect(Policy::none())`
is still in a builder chain twelve lines away. TLS is the opposite call and for
the same reason `argon2id` is not hand-rolled: nobody should write a TLS stack,
and `rustls` is a small audited one whose *shape* enforces the pin. The
dependency bill is 7 crates, against the 223 that `security-logging-design.md`
§2.1 declined for `webauthn-rs`.

**And the response does not come back.** §7b: *"sinks deliver payloads and log
deliveries, so the response or its metadata comes back to the tenant."* That is
the exfiltration half of SSRF, and it is closed by the return type rather than
by a rule about logging: `Accepted` carries a status code and nothing else.
Headers are consumed only to find the blank line that ends them; the body is
read to a cap and dropped without being decoded. There is no path by which a
byte the destination sent reaches a caller. A receiver that wants to say
something says it with a status code.

### Two widenings of `[pinned]` text, made rather than done quietly

**`fc00::/7`, not `fd00::/8`.** §7b and `implementation.md` both list
`fd00::/8` among the link-local/unique-local denials. The standard is
`fc00::/7`, of which `fd00::/8` is the half in actual use — so the document's
letter leaves `fc00::/8` reachable, and *"the spec said fd00"* is not a thing
to tell an incident review. The wider prefix is what `deny.rs` checks. This is
a widening in the direction the pin argues for, which is why it is recorded
here and the two documents now say `fc00::/7`.

**The destination is inside the MAC.** §7b pins `X-Ordo-Signature` as *"HMAC
over `timestamp ‖ body`… with a replay window and the timestamp inside the
signed material"*. The implementation signs `version ‖ timestamp ‖
destination ‖ body`, each part length-prefixed with a u64 big-endian length.
Two additions, both strict strengthenings:

- *the destination*, because without it a delivery captured on the wire is
  valid at **any** path on any host sharing the secret — and a receiver who
  reuses one secret across two sinks, which nothing prevents, has handed an
  attacker a cross-sink replay;
- *a version tag inside the MAC*, not only in the header, because a scheme
  whose version lives outside the MAC is a scheme whose version an attacker
  chooses, which is how every downgrade works.

Length-prefixed rather than concatenated for the reason every canonical
encoding in this repo gives — `("ab","c")` and `("a","bc")` must not collide —
and u64 big-endian because that is `ordo-commandlog`'s chain encoding and one
convention beats two. Both documents now carry the wider material.

### Two signatures, two keys, and the one that must never be shared

An attestation delivery carries two independent signatures answering different
questions, and the separation is the security argument:

| | signed material | key | claim |
|---|---|---|---|
| payload | `Checkpoint::signed_bytes(space, upto, head, at)` | the deployment's Ed25519 checkpoint signer | *this space's chain was at this head at this time* |
| transport | `v1 ‖ ts ‖ destination ‖ body` | a per-sink HMAC secret, shown once at grant | *this request came from this deployment, for you, and is not a replay* |

The payload signature is computed by `cadence::attest_due` when the record is
minted, and the delivery path copies it out of `$attestations` verbatim. The
egress crate never sees the Ed25519 key and could not use one if handed it.
That is not tidiness: **the egress identity is the one built to hold nothing,
and the checkpoint signer is the key that authors this deployment's claims
about its own history** — the single worst key for it to hold. A design that
signed deliveries with the checkpoint key would put history-authoring
authority in the process whose entire job is opening sockets to customer-named
hosts. So: never the same key, and the crate boundary is what makes it never.

It also splits cleanly by *reader*: the transport check is a webhook endpoint
authenticating a header at ingest with a secret it already has (no key
discovery), and the payload check is anyone auditing the stored record months
later with a public key.

### Layer 2: what "cannot redirect" buys, and where it stops

The sink row is `(destination, owner_key, destination_sig)`, and
`AttestSink::verify_destination` re-checks the owner's signature over the
canonical destination **before every send**, not only at setup. That second
check is the one that matters: it makes a direct edit of the `destination`
column a *refused* delivery rather than a *redirected* one, and it costs one
Ed25519 verification per attestation, which arrives at most once per
`attest_every` commands. The threat it closes is the realistic one — a
compromised operator account, a SQL injection, a backup restored from
somewhere else.

It does **not** stop a platform that rewrites the destination *and* the owner
key together, because a signature checked against a key the attacker also
chose proves nothing. No scheme with the verifying key inside the perimeter
can close that, and claiming otherwise would be the kind of promise this
corpus keeps striking out. Two things make the remaining gap visible rather
than silent, and both are built: the owner key's **fingerprint travels in
every payload** (`destinationKeyId`), so a receiver who recorded it once sees
a swap on the next delivery — layer 1's trust-on-first-use shape, applied to
the destination; and setting a destination is owner-gated and step-up gated,
so it lands in the security-event stream as a `CustodyAction`, and a platform
that changes it without the record has to have edited two chained artifacts.

**The space is inside `consent_bytes`.** `str("ordo.attest-destination/1") ‖
str(space) ‖ str(canonical-url)`. Without the space, an owner who owns two
spaces has signed a destination for both, and space B's attestations go
quietly to space A's receiver. The secret is deliberately **outside** it: the
platform mints it, so the owner cannot sign it at the moment they sign the
URL, and pretending otherwise means either a two-round ceremony or a secret
the owner chose, which is a worse secret.

**A new sink starts at the current head, not at genesis.** §7's second stated
limit is that *"you can only verify from the point you started watching"*.
Backfilling a thousand historical attestations into a receiver's log would
hand them signed heads for a window they were not watching — which anchors
nothing and reads like it does. `register_sink` sets the cursor to the latest
attestation and the op reports it as `watchingFrom`.

**The outbox is `$triggerruns`-shaped, not `$jobs`-shaped**, because `$jobs`
has no `attempts` and no `next_attempt` and therefore cannot back off or
dead-letter. It reuses `triggers::backoff_secs` and `MAX_ATTEMPTS` rather than
growing a second backoff — R3's *"one shape of backoff in the platform, not
two"*. Attempts increment at **claim**, copied from `triggers::claim_runs`, so
a destination that reliably kills the worker still reaches the dead-letter cut
instead of looping forever. And a **non-retryable** refusal dead-letters on its
first attempt: re-resolving a name that answers `127.0.0.1` answers
`127.0.0.1` again, and five retries are five more DNS queries to a name an
attacker chose.

### Layer 3: one format shipped, four owed, and why this one

§7's layer 3 is not a new mechanism — *"witnesses = wherever the owner points
that webhook. No special protocol and no cross-deployment federation"* — it is
the generic signed-checkpoint POST plus documented targets. The generic POST is
shipped. The one concrete format **implemented** is §7's own cheapest-first
entry: **the owner's own HTTPS endpoint**. That is a real witness and not a
dodge, because the property that makes something a witness is *a copy exists
outside the platform's reach at a time it cannot retroactively influence*, and
a signed POST to an endpoint the platform does not control achieves exactly
that.

The four owed, each with the specific reason it is not first:

- **WORM / object-lock bucket.** The strongest property in the list — §7 calls
  write-once *"the property that matters"* — and the nearest to shipping: it is
  a request builder plus SigV4 (HMAC-SHA256, already a dependency) on the same
  pinned client. It is not shipped because *the request has the right headers*
  is not the property; *S3 accepts it and applies object lock* is, and
  asserting the first against a listener I wrote myself would be a test of my
  own fixture. It needs a real bucket in CI, and then it is small.
- **Sigstore Rekor.** Best candidate for second, because the Merkle
  inclusion-proof verification it needs generalises — the offline verifier
  would gain a capability rather than a special case.
- **OpenTimestamps.** Free, and per-attestation cost effectively zero, which is
  the argument §7 makes for it. Its *verification* story is the weakest of the
  set: checking an anchor honestly needs a Bitcoin node, and anything less is
  trusting a calendar server, which is another platform to trust.
- **git over HTTPS.** The most code (smart-HTTP plus packfile writing) for the
  least incremental property, since the ordering a commit chain proves is
  already proved by the Ed25519 signature over the head.

The **public transparency log (layer 4 in §7's own numbering)** stays declined,
which is what the `attestation` decisions row recommended. What this build
calls layer 4 is `chainProof` and its verifier — the piece §7's layer 4 rests
on and the piece `licensing.md`'s trust argument assumes.

### `chainProof` carries digests, not bodies

`implementation.md` specifies *"head hashes, signed checkpoints, and per-group
hashes"*. Per-group **hashes**, and that is the design rather than an economy:

- it is what a third party needs. The question is *"is the head I pinned at
  time T still what this chain says at that position"*, which is a question
  about links and signatures;
- the bodies are already exportable — `std.space.export` ships
  `commandlog.jsonl` with every group's content, under the owner's authority
  and with a disclosure record. A second full-content path with a different
  gate would be the weaker of two doors to the same room;
- so a chain proof discloses **no space content**, which is what makes it safe
  to hand to the third party it exists for.

The limit that follows is stated on every run rather than left to be
discovered: a verifier checking this artifact confirms the links and the
signatures; it does not confirm that a hash corresponds to any particular
content, because it has not been given the content.

**Roles, argued rather than defaulted.** `platform_ops.rs` already sets the
rule: the exit ops are the owner's *"because ending a space and disclosing all
of it are not administration"*, and `callersOf` is manager-and-owner because
*"an audit surface its own administrator cannot read pushes deployments to
hand out owner"*. The two new ops land on opposite sides. `attestTo` **decides
where a space's history is published** — a custody decision of the export
class, naming a third party who will hold signed statements about the chain
forever — so it is owner-only. `chainProof` **discloses digests**, strictly
less than `callersOf`, which names principals by id; refusing it to the
manager would mean the role that administers a space cannot check whether its
history is intact. Both are step-up gated by the registration loop, which has
a consequence worth naming: `step_up_decision` denies outright when
`claims.capability` is present, so **a capability token can never nominate an
attestation destination.**

**`Write` for one, `Read` for the other**, and the same argument runs both
ways. `attestTo` mints a secret, stores a destination and writes a
`CustodyAction`; a `GET` that does those things is a `GET` with side effects,
which every cache and prefetcher is entitled to repeat — and repeating this one
rotates the secret out from under a receiver already using it. `chainProof`
writes nothing, mints nothing, records nothing, and discloses no content, so
there is no disclosure whose record is owed; it is a pure function of
`(state, from, to)`, which is exactly what a cache is entitled to assume.

**`MAX_LINKS` is a bound, not a page.** A proof is a single artifact a third
party verifies as a unit, so paging it would hand somebody two files and the
job of stitching them — the job the verifier exists to do correctly. A caller
who needs more asks for two ranges and verifies each against its own anchor,
which the verifier supports and a stitched proof would not.

**The platform verifies its own segment before publishing it.** That proves
nothing to a third party — the whole point of the exercise — and it stops the
deployment handing out a confident schema over a chain that does not recompute
locally, which would be a proof of the wrong thing.

**The encoding agrees with the export bundle.** Each link is
`{"idx","prev","hash"}` with lowercase-hex digests: the same three keys and
the same strings `export.rs::group_json` writes on each `commandlog.jsonl`
line. A proof and a bundle taken from one deployment at one position are
comparable field by field with no conversion, which is what
`plan-production-ops.md`'s R-track wants where a restore marker and a chain
proof overlap. Asserted in `attest_webhook.rs`, not in prose.

### The verifier is a separate binary, and `ordo verify --proof` calls it

`ordo verify` exists and is the right shape, and it is the wrong *home* for
this — for a reason `ordo-cli`'s own module doc supplies: *"`ordo-cli` now
depends on `ordo-exec`, so building the `ordo` binary builds the driver, the
pool."* A verifier is handed to somebody whose entire reason for running it is
that they **do not trust the deployment**, and a binary that links a Postgres
driver is a binary a careful recipient has to audit before running.
`ordo-verify-proof` links `ordo-commandlog` and `serde_json`. It has no socket,
no driver, no environment variable, and `tests/offline.rs` asserts that by
reading the manifest — so a recipient confirming *"this cannot phone home"*
reads one dependency list instead of auditing a call graph.

`ordo verify --proof` is offered anyway and calls straight into the same
routine, because an operator looks for verification beside the other
verification commands. One routine, two front doors: a second implementation of
a verifier is the one place a disagreement between two correct-looking programs
would read as tampering. The same rule produced
`the_two_consent_encodings_agree`, which compares the platform's
`consent_bytes` against the verifier's byte for byte.

Three things the verifier says that most verifiers do not:

- **without `--key` it prints `UNVERIFIED`** and says why — the key inside a
  proof is the platform's claim about its own key, and checking a signature
  against a key the signer chose is a tautology;
- **the caveats print on success too.** A verifier that mentions its limits
  only when it fails has taught its reader to skip them;
- **a mismatched preimage is a different finding from a bad signature.** A
  receiver who gets the u64 length prefix wrong sees a *failed signature*,
  which is indistinguishable from tampering and is the one error nobody debugs
  correctly. `signedBytes` is published in the payload for exactly this
  comparison, and it is why that redundancy is not redundancy.

Exit codes are `ordo verify`'s: **3** for a failed verification, **1** for "I
could not read it". A cron job that only knows `!= 0` treats a bad download as
tampering, cries wolf once, and gets switched off.

### Two refusal classes, and why not zero and not six

`platform_ops.rs` sets the default at zero new classes — *"a class minted for
'you are not a manager' would itself be the oracle"*. These two ops need
refusals nothing existing says, and each has exactly one producer and one doc
row:

- **`egress_refused`** ← `attest_ops::egress_refusal`. Not `invalid_param` (the
  value bound correctly as text), not `bound_violation` (it is not a length),
  not `access_denied` (which an executor may not raise). **One class for all
  six pins**: `egress_denied_pod_cidr` as a distinct wire word would let anyone
  who can name a hostname map the deployment's internal address plan one grant
  at a time. What *is* disclosed is the **category**, because §7b requires the
  deny list to be on the consent page and a refusal that will not say which
  promise it kept is one nobody can act on. The resolved address goes to the
  deployment's own log and never to the customer, which is the
  `disclosable()` / `operational()` split.
- **`proof_unavailable`** ← `chain_proof`. Deliberately **not** `not_found`: a
  range whose entries were lawfully pruned is a range the deployment is telling
  the truth about, and collapsing it into "no such thing" would let a
  deployment answer a tampering question with a shrug — the one answer
  `security-logging-design.md` forbids, since *"I could not check" and "it
  checks out" must not be the same value*. 409 with the state-decided family,
  because a narrower range lands.

An owner signature that does not verify reuses **`not_confirmed`**, whose
inventory row already reads *"a refusal after the value bound correctly, made
by the op because what the value says is not what the op requires it to say"*.
That is what an unverifiable signature is, and a new class would be a second
word for one idea.

### Found while building

Five, and the first is the one that matters.

**A correct IPv4 range check does not see three of the classic bypasses,
because none of them is an IPv4 address at the moment it runs.**

- `::ffff:127.0.0.1` is an `IpAddr::V6`, and `Ipv6Addr::is_loopback` answers
  **false** for it — correctly, since `::1` is the IPv6 loopback and this is
  not it — after which the socket layer connects to 127.0.0.1 anyway. The same
  form of `169.254.169.254` walks straight past a deny list that checks the
  five categories §7b names.
- `64:ff9b::7f00:1` is 127.0.0.1 behind a NAT64 gateway, which is most mobile
  networks and a growing number of clusters.
- `0.0.0.0/8` reaches localhost on Linux and is in none of the five categories.

Every address is now canonicalised — IPv4-mapped, IPv4-compatible and NAT64
forms unmapped — *before any predicate sees it*, and `6to4` (`2002::/16`) is
**refused** rather than unmapped, because a 6to4 relay is not something this
platform should be posting through at all. `is_global` is not stable on
`IpAddr`, so the check is an explicit enumeration, and an explicit enumeration
has to be complete or it is worse than useless; the additions beyond §7b's five
are named in `deny.rs` with a reason each.

**`allocation` contains `location`.** The grep-as-a-test for pin 3 — *no code
in this crate reads a `Location` header* — failed on the first run against a
file that legitimately uses the word "allocation". A whole-word check is the
difference between a test and a tripwire, and a tripwire that fires on honest
code is a test somebody deletes.

**A sink registered after an attestation delivers nothing**, and every delivery
test in the suite got zero units the first time it ran. That is the
start-at-the-head rule working exactly as designed — but it means the tests had
to be written in the order a deployment actually does it, and the helper now
says so in its own doc. Worth recording because the failure was silent: a green
`enqueue_due` returning `Ok(0)` looks identical to a working one.

**A health field only some callers update lies to the others.**
`DeliveryHealth::sent` was incremented by the loop and not by the pass, so a
caller driving `run_pass` directly — a test today, R3's supervisor calling
`Worker::run` tomorrow — read zero after a delivery that demonstrably landed.
The counters moved to where the deliveries happen; the clock stayed with the
loop.

**`webpki` refuses `CaUsedAsEndEntity`**, which broke the first test fixture: a
self-signed certificate with `CA:TRUE` presented as a server certificate is
exactly the shape a lazy test wants, and a correct verifier rejects it. The
fixture is now a real CA → leaf chain, which means the suite exercises chain
building rather than a single-certificate shortcut. A small vote of confidence
in the stack, arrived at the hard way.

**`lock_wait.rs` is wall-clock sensitive, and this suite made it flakier
without touching it.** `a_contended_append_lock_is_visible_in_the_histogram_and_an_uncontended_one_is_not`
asserts `work_max < 0.1s` for the critical section — a head read, a duplicate
check and an insert. That is a generous bound on an idle machine and a
reachable one on a loaded one, and adding three crates' worth of compilation
and a second Postgres-backed suite to `cargo test --workspace` is exactly the
load that reaches it. It failed once in a full-workspace run and passed nine
times out of nine in isolation, before and after this change, so it is a
**pre-existing timing sensitivity surfaced by more parallel work**, not a
regression — recorded rather than fixed, because the fix is a judgement about
what the bound is protecting (a lock held across something it should not be)
and widening it to silence a flake would remove the only thing it detects.
The honest options when it next bites: measure CPU time rather than wall
clock, or gate the assertion on a load check. Filed here so the next person to
see it red does not go looking for a lock bug that is not there.

---

## Custody and space are inputs to loading a program (2026-09-07)

`plan-runtime.md` R2a. The runtime plan's survey found a defect that reordered
the whole plan, and it is worth recording as a ruling rather than as a bug fix,
because the shape of the mistake is general.

**What was true.** `deploy::verify_candidate` took `keys: Arc<dyn KeyProvider>`
and a `SpaceId` and threaded both into the `ordo_exec::Executor` it built. That
covers the **read** path: `Executor::read_page_as` resolves blind indexes and
opens sealed payloads through `self.keys`. The **write** path does not go
through the executor's copy. `FixtureExecutor::execute`'s write arm calls
`exec.write_leased(live.fixture.clone(), …)`, and every seal, every
blind-index digest and every sealed-handle mint in the Stage-5 protocol is
`Fixture::keys` — while `Fixture::compile_source` hardcoded that field to
`TestProvider::with_space(FIXTURE_SPACE)`, whose master key is the constant
`b"ordo-test-smk-00"` published in this repository.

So a deployment holding a real `KeyProvider` read through the operator's
custody and **wrote under the repository's constant**. Not a refusal to boot: a
working round trip, a green health check, and ciphertext openable by anyone who
has cloned the repo.

**Ruled: a program's custody and its space are inputs to loading it, never
properties of its compilation.** The argument in one line: the compiler is a
pure function of source text, and a constructor that mints custody makes the
same source text mean different key material depending on who called it —
which is exactly how a test key reached a production write path with nobody
having made a decision. `verify_candidate` now assigns both, before the fixture
is shared, and asserts what it assigned.

**The assertion is by identity, never by type name.** `Arc::ptr_eq` between the
provider the caller supplied and `Loaded.fixture.keys`. A `provider_name`
comparison would pass for a second, differently-configured `LocalKeyProvider`,
and that is precisely the shape of the bug being guarded against. Two
independent mechanisms hold it: the check inside `verify_candidate`, and the
binary's own check on every boot *and every swap* — a swapped-in candidate
carries a freshly compiled fixture, so a boot-time-only check would not catch a
regression that reached the tree after the first load.

`TestProvider` keeps its constructor and its constant. Nothing about it was
wrong; what was wrong was that it was the *default* on a path a deployment
takes. Its own doc's *"nothing it seals is secret, and looking secret would be
worse than being plain"* stays true only while nothing production reaches it.

**Found while proving it.** The test harness had been compensating for this the
whole time and said so: `metered_fixture` assigns `f.keys` *after*
construction, with a comment reading *"the executor reads through `f.keys`, and
the Stage-5 write protocol seals through the very same handle."* The seam was
already known to be the fixture field. What was missing was anything on the
production path that set it — which is the general lesson: a seam that only
tests use is a seam whose absence in production nothing detects.

---

## Blob & content custody: five rulings the build forced (2026-09-08)

`plan-blob.md` B1–B5. The plan's §2 ruling — keyed-random handles, never
content-hash — was settled before the build; these five are what the build
turned up, and each was a fork the code could not leave open.

**1 · A blob field must admit null (`blob_required`).** Not a style rule: the
only producer of a blob value is an upload, and an upload addresses an
existing row (`PUT /blob/<entity>/<field>/<id>`). So the field is empty
between the row's creation and the upload, and a required blob field would be
a field no `create` could ever fill.

Found by the derived CRUD ops, which name every declared field and therefore
declared a `blob` parameter the compiler refuses — every entity holding a file
failed registration on the first blob field. Excluding content fields from
`std.E.create` is only *sound* because of this ruling; without it the derived
create would stop being insert-complete. Excluding them from `std.E.update` is
additionally the correct semantics: a full-row CAS converge sets every field
in its braces, so including one would make every unrelated edit either
re-send the handle (the caller naming an object it did not upload) or null the
file.

**2 · `inline` is a storage tier, `sealed` is custody, and they are
orthogonal — but `inline` never reaches the blob door.** DL §2 says the three
custody tiers *"compose with `inline` orthogonally"*, and the column types
follow: external `blob` is `text` (the hex handle), inline is `bytea` (the
payload handle), sealed is `bytea` either way.

What the build forced is narrower and was not obvious: an inline blob's bytes
go into `$sealedpayload`, which is written **by the write protocol inside a
command group** — and the blob door has no group to attribute them to. So the
door refuses an inline field by name, with that reason. It is a seam, not a
stub: `ordo_crypto::inline` exists, the cap is in `BlobPolicy`, and
`sealed::collect_pruned_in` already collects it. What is owed is the write
protocol's side, not a second door.

**3 · §5.1's retained-image reference set is subsumed by §5.2's floor.** The
plan names two sources for GC's reference set — live rows, and retained
command images. The second cannot be queried without *opening payloads*,
which would make garbage collection a key-using operation, and would be
weaker than what replaces it: a payload whose epoch was destroyed cannot be
opened at all.

It is unnecessary. Phase 1 stamps `unreferenced_since` at the instant no live
row names the handle; any image that could restore it is at or before that
instant; phase 2 deletes only once the floor has passed. So by deletion time
every image naming the object is already prunable. Enumerating them would be
a second, weaker statement of the same guarantee. What keeps it airtight is
the re-check under the tombstone's row lock — a revert that restores the row
before the floor passes cancels the tombstone rather than racing it.

**4 · The bucket's object key and the export bundle's layout are different
layouts, deliberately.** `migration.md` §6 says *"blob bytes, laid out by
entity/field/id"* and the bundle does exactly that. The **bucket** must not:
an object key carrying an entity and a field would enumerate the schema to
anyone who can list it, and an epoch in a key would print a revocation
timeline to someone holding no keys. So the key is `<space>/<hex handle>` and
nothing else, and the export can produce the other layout because it holds
the `$blobref` rows, which carry the entity and the field.

The space *is* in the key, and hashing it was declined: it would leave an
operator holding a bucket and an incident unable to answer "which objects are
this tenant's" without the database that is currently down. What the prefix
must never do is escape, so `space_prefix` **refuses** a hostile space id
rather than sanitising one — a sanitiser can collapse two ids onto one prefix,
which is the cross-tenant read the composite primary key exists to prevent.

**5 · A BYO-bucket attachment names a credential, never carries one.** DL §2
pin (1) already says the platform's credential *"lives behind the same
resolver pattern as keys (`KeyProvider`-style, never in definitions)"*. The
build makes the consequence concrete: `std.space.attachBucket` takes a
credential **label** the operator installed out of band, because a secret in
an op parameter is a secret in the request envelope, in the idempotency cache,
and — the one that decides it — in the **plaintext command log**, which would
make "the log is readable" an incident rather than a property.

The same op closes an inherited gap rather than inheriting it. `exit_ops`
records that `PlatformExecutor` drops the durable `ReplayLease`, *"safe here
only because both exit ops are idempotent by construction"*. Attaching a
bucket is not — run twice with different arguments it changes storage custody
twice. So it is keyed on the target's identity: the same attachment is a
no-op that reports success and writes **no custody event** (a retry is not a
custody change), and a different one is a genuine change with its own record.

---

### Three ratchets `plan-blob.md` §3 did not list, found by firing

The plan enumerated fourteen sites the `blob`/`uri` type-add would trip. Three
more fired that it had not named, and all three are the good kind — a
compiler or a test refusing to let a decision be made silently.

- **`ordo_exec::shape::accepts`** — the F6 shape gate's declared-versus-
  observed type mapping. Exhaustive already, so it broke. `blob` accepts
  `text|bytea` because the function is handed the *type* and not the field
  and cannot see the tier; both are shapes `ddl::column_ty` genuinely emits.
- **`ordo-conformance`'s `no_new_refusal_class_arrives_undocumented`** — five
  new classes had to be *specified*, not merely raised. They are now a table
  in `definition-language.md` §2, which is where a reader implementing from
  the spec will meet them.
- **The derived `std.E.create` / `.update`** — ruling 1 above.

`ScalarTy::ALL` closed a fourth that pre-dated this work: five inventories in
`editors/ordo.tmLanguage.json` were pinned to their producers and the *type
list* was not, because no `ALL` existed to pin it to. The recorded reason —
that `Duration` carries a unit — was true of the `word_enum!` macro and
mistaken for a fact about the enum. So a type added to the language and
forgotten in the grammar left it uncoloured, with nothing failing.

---

## Three rulings from the owner's answers of 2026-09-08

Recorded from `decisions.json` saved 2026-09-08. Each one closes a question
that had been carried on the decisions page across more than one audit.

### GC048: the case follows GC080, and the document is edited

`guards-conformance.md` lists `level` among `cardList`'s guard reads, and
`GC080` in the same document states the post-pruning rule that drops it:
`level` is tested only by the *write* guard, and `cardList` is a read. The
engine follows `GC080`. The two cases could not both be right, and the
owner's answer ("implement the fix, I will take your suggestion") settles
which one moves: **the `GC048` case is edited to match the engine**, since
the engine is doing what the document's own rule says. Editing the engine
instead would have meant a read op paying for a predicate that only a
write can fail. In the same answer, `GC007` is fixed in code (a dot-path
target's read guard must inline, and the fixture hid that it does not), and
`GC083` is left as vocabulary, since forcing it churns the manifest for a
difference in words.

### The deployment operational security log is its own chained artifact, on by default, behind a toggle

Finding 22 (`11-22`). The owner's ruling: "it should be a toggle, but its
own artifact makes sense to me as a default." Recorded as: the deployment
log exists as a hash-chained artifact separate from every space chain;
it records **metadata about refusals only** (principal, time, op name,
refusal class), never a space's contents, so it is a record of doors that
did not open rather than a door of its own; its head is exposed the same
way a space chain's is, so the operator who can read it can also be shown
to have not edited it; and it is **on by default**. The toggle turns the
artifact off, and when it is off a cross-space sweep is recorded nowhere.
That consequence is disclosed in the operator documentation rather than
softened, because the alternative (dropping denials into the application
log) removes nothing from the operator's visibility and only removes the
proof.

### The worker loops run, by default, inside the server process

Triggers and routines (`§7`, `§12`). Every loop step is built and tested
(`claim_runs`, `fail_run`, `fire_due`, `fire_clock`, `drain_listener`), and
no long-running task calls one. The owner's answer, "yes, wire it," is
recorded as: the loops are driven by the worker supervisor (`plan-runtime.md`
R3), behind an explicit flag on `ordo serve` that **defaults to on** for a
single-node deployment. The flag exists because clock leadership is an
advisory lock and a multi-pod operator may want the loops on a dedicated
pod. The default is on because a subsystem that is fully built and never
runs decays silently: the per-step tests keep passing while the composition
rots, and a trigger a developer wrote and tested would be documented,
shipped and inert. The wiring ships with the runtime build, since the
process it runs inside does not exist before R1.

Pending from the same batch, recorded here so it is not lost: the owner's
reading of the default blob store (deployment configuration at boot,
pointing at an S3 gate; an internal volume-backed store later) matches the
recommendation on the decisions page and awaits his flipping the row to
*decided*.

---

## Blob uploads take the write protocol's path, and the chain carries the digest (2026-09-08)

`plan-blob.md` §8.1 and §8.2, ruled by the owner on 2026-09-08 after reading
`docs/blob-chain-explained.html`. The build follows a plan
(`plan-blob-chain.md`); nothing is built from the ruling alone.

**§8.1: candidate (c).** An upload is a row change and is recorded like one,
by taking the same path. The blob door hands the write to the write protocol,
so the upload becomes a converge the ordinary executor runs, with the handle
and the digest bound by the platform. The owner's framing, which is the
ruling in plain words: *"I don't understand how this is different than any
other row change."* It is not, and (c) makes the code say so.

Chosen over (a), where the door builds and appends the group itself, for
one reason: (a) produces a discipline (every door that writes a row must
remember to append) and (c) produces an invariant (only the write protocol
writes entity rows). An invariant can be enforced by a test and checked by
a reviewer; a discipline can only be remembered. The audit spine is the
claim this platform makes, so the version that makes "every row write goes
through one path" a checkable fact is worth its extra cost.

The seam (c) opens, a value entering the write path with no caller-written
source expression, is closed the way the egress kernel closed pin 2: a type
only the blob door can mint, consumed by value by the write protocol, with
no public constructor and no serialisation. The build also consolidates the
two full-row image builders that already exist (the write protocol's and
undo's) into one, since a third would have been the hazard §8.1 named and
two is already one too many.

**§8.2: carry the digest.** The image's blob column carries the handle and
the digest, landed in the same commit as §8.1 because that is the only
moment the encoding is free. The owner's definition of verification is
recorded as the rule: *"verification is comparing the audit log hash against
what exists; that is proof of what was and what is."* The chain is the record
of what was; the store is what is; verification is the comparison.

Three consequences follow from that definition:

- A **mismatch** (the object exists and its digest differs from the chain's)
  is proof of corruption or tampering. A revert refuses, consistent with
  undo's posture of refusing before any write.
- An **absent or unreachable object** proves nothing about the object. A
  revert restores the handle and emits a security event, so the undo
  horizon never depends on object-store availability. This is the case the
  plan called the sharp edge, and the owner's definition resolves it:
  absence is not a failed comparison, because there is nothing to compare.
- An object absent because the **undo horizon has passed** is the horizon's
  own refusal, already in force for every kind of data. Retention does not
  grow: bytes live until the horizon, and the handle and digest live in the
  chain permanently, at under fifty bytes per change.

### Addendum, same day: what the digest is over, and what control of the bucket has to do with proof

Two clarifications the owner put as questions and confirmed, recorded
with the ruling so the plan builds against them.

**The digest is over the original bytes, not the ciphertext.** "Plaintext"
in the blob pipeline means the file's own bytes as the uploader sent them,
before sealing; a PDF or an image is plaintext in this sense. The digest is
computed over those bytes as they stream past, the same bytes are sealed
under the space key (chunked STREAM, fresh nonce per upload) and the sealed
form is what the bucket holds, and the digest is itself sealed on the file
row and, under §8.2, in the chain image. This was B3's choice and it is the
correct one: the same file seals to different ciphertext every time, and a
re-seal after key rotation or a move to another bucket would break a
ciphertext digest. A digest of the original bytes stays true across all
three.

**Control of the bucket is required neither for proof nor for
availability.** Criticism: "the chain promises revert, but the bytes live in
a bucket you do not control." The answer splits in two. *Proof* needs a
commitment, and the digest in the image is one: whoever presents bytes for a
handle, from our bucket, a customer's BYO bucket, or a hostile one, either
produces the recorded digest or does not, so the platform can prove what
the file was and whether what it has now is that file, regardless of who
runs the store. This is the principle the rest of the spine already uses
(sealed values as handles, `chainProof` as digests). *Availability* is the
horizon floor: the sweep keeps an object for as long as the chain promises
a revert of it, so inside the horizon the bytes exist by construction, and
past it every revert refuses anyway. An unreachable bucket inside the
horizon restores the handle and says so in a security event; the record of
what was is untouched.

**The key boundary, stated for critics.** The comparison (fetch, unseal with
the space key, hash, compare) is available to whoever holds that key. An
outside auditor with only the exported chain verifies its links and hashes
and cannot check a file's content without being given the key. That is the
boundary every sealed value already has: the chain proves history to
everyone and content only to those entitled to read it.

## Three answers on the blob-chain plan (2026-09-09)

`plan-blob-chain.md` §7 raised four questions. Three are settled here; the
fourth (verification cost) is still open and is recorded in the decisions
file with the argument that changed it.

**The seam crosses on a trait, and a test holds the property the type
system cannot.** `PinnedUpload` stays mint-once: private type, private
constructor, no `Clone`, no serialisation, consumed by value, carrying its
own target row. It lives in the host crate and the write protocol lives
three crates below, so the crossing is a `PlatformValue` trait declared in
the lower crate, because a concrete type cannot travel downwards while the
browser build forbids the dependency that would let it. A trait is
implementable by anyone, so the one-implementor property is held by a
source-scanning test, in the manner of `PinnedDestination` in
`ordo-egress`. Ruled on the recommendation, owner's answer of 2026-09-09.

**An image with no recorded digest restores, and the skip is recorded
rather than alarmed.** Blob columns are already imaged today, because a
delete images every column, so images without a digest exist in every
deployed chain. Reverting one restores the handle without verification and
raises no security event: absence of a digest is not a failed comparison,
since there is nothing to compare. The owner accepted this on the condition
that the fact is logged, and drew the line himself at a security event
because of the noise it would make. The condition is right and is not free.
The revert's own command group already records that the restore happened,
which is the sense in which every change is audited, but nothing in it says
verification was skipped, and a reader would have to notice a missing key
and know what its absence means. The recommendation put back to him is an
ops-log line naming the correlation, entity, field and handle, with the
security event reserved for the two cases that mean something is wrong: a
digest that does not match, and an object that is not there.

**A deployment is pointed at a store, and refuses to serve blobs without
one.** Ordo is a platform others deploy on, so a deployment names an
S3-compatible endpoint at start, which is what `ORDO_BLOB_S3_*` reads and
what `production_ready` already refuses a deployment for lacking. Tenants
bring their own bucket or use the one the system provides. A store backed
by whatever volume the process happens to have is a later addition and
never the default, because a default that silently works on one node is the
failure this refusal exists to prevent. Owner's answer of 2026-09-09.

### The in-object hash, and why it cannot be the witness

The owner asked whether the object could carry a sealed digest of itself in
its header, so verification reads only the surface. The machinery exists:
every sealed object already carries a 21-byte header
(`crates/ordo-crypto/src/stream.rs:105`) fetched by its own range request,
deliberately separate from the body (`pipeline.rs:374`). What stops it is
what the comparison would prove. A digest inside the object was written by
whoever wrote the object, in the same act, under the same key, so it agrees
with its own body by construction. Against an attacker holding no keys it
adds nothing, because the streaming AEAD already refuses every chunk they
could mint. Against a key holder, which is the threat an audit chain exists
for, the false body and the false header are written together. In both
cases the object verifies itself. The chain is the only independent
witness, and reaching it means the bytes.

Stopping the stream early works only if the digest covers a prefix, which
turns proof of the object into proof of its first N bytes.

Two things cut the cost without weakening the claim. A size fast-fail:
`$blobref` records the plaintext length and `verify` is already handed it,
so a length the store disagrees with refuses before anything transfers,
while a length that agrees proves nothing and the full hash still runs.
And a defect to fix first: `verify` (`crates/ordo-blob/src/pipeline.rs:445`)
buffers the whole object before hashing it, contradicting the promise the
crate pins in `a_hundred_megabyte_upload_never_holds_the_object`. Hashing
as it streams is constant memory, and that is the real cost here.

A digest over the ciphertext would need no keys to check, and dies at the
first key rotation: rotation re-seals the object while the plaintext is
unchanged, and an append-only chain cannot rewrite what it recorded.

### What refusing `blob sealed` implies

Nothing that works is lost. `column_ty` returns `bytea` for any sealed
field before reaching the blob arm (`ordo-schema/src/ddl.rs:90`) while the
door writes the hex handle as text; the image builder takes the sealed
branch and asks for an epoch it does not have; and the collector's
reference query compares that `bytea` column against `encode(b."ref",
'hex')` (`ordo-exec/src/blobgc.rs:176`), which Postgres has no operator
for, so a sweep over any schema declaring one errors. No deployment can be
relying on it.

Nor would the tier buy secrecy of the bytes, which every external blob
already has. It hides which object a row points at: the linkage rather than
the content. That is a real property, and it is a design rather than a
missing branch, because the collector matches live references by comparing
the column to the handle and holds no keys on purpose. Sealed handles carry
a fresh nonce, so equal handles do not produce equal envelopes and the
comparison cannot be made to work at all. Serving the tier needs a keyed
collector or a deterministic blind index derived from the handle.

Refusing it at the door by name is the third of a pattern rather than a new
kind of refusal: `private` and `inline` are already refused there with
their reasons stated (`ordo-host/src/blob.rs:194`, `:207`). The declaration
stays legal in the language, so a later build serves it without a syntax
change, and the refusal names the collector conflict so the next reader
does not rediscover it.

## The last three blob-chain answers (2026-09-09)

**No size cap on verification. The application bounds the object instead.**
Verification always runs: a revert of a blob converge reads and hashes the
whole object. An application that does not want unbounded verification cost
limits object size at upload, which is where a limit belongs, because the
application knows what its files are for. A cap at revert time was
considered and dropped: it would have bought a rare saving at the price of
a fifth outcome, a record for the skipped case, and a per-space policy home
that does not exist in the tree. The audit path keeps one behaviour, which
is that it verifies. Owner's answer of 2026-09-09, overriding an
intermediate proposal for a per-space cap defaulting to unbounded.

Two things from the same exchange stand on their own, because neither was
about capping. A **length fast-fail**: `$blobref` records the plaintext
length, so a length the store disagrees with refuses before a byte moves,
while a length that agrees proves nothing and the hash still runs. Cheap to
fail and full price to pass. And a **defect**: `BlobPipeline::verify`
(`crates/ordo-blob/src/pipeline.rs:445`) calls `get(..)` and holds the whole
object in memory before hashing it, contradicting the promise the crate pins
in `a_hundred_megabyte_upload_never_holds_the_object`. It hashes as chunks
arrive instead, which is constant memory and is the real cost of
verification.

**`blob sealed` is refused at the door by name.** The third of a pattern
beside `private` and `inline` (`crates/ordo-host/src/blob.rs:194`, `:207`),
with the reason stated, and sealed blob fields leave the collector's
reference query. The declaration stays legal in the language so a later
build serves it without a syntax change. The refusal names why the tier is
not merely unfinished: the collector matches live references by comparing
the column against the handle and holds no keys on purpose, and sealed
handles carry a fresh nonce, so equal handles never produce equal
envelopes. Serving the tier needs a keyed collector or a deterministic
blind index derived from the handle. Owner's answer of 2026-09-09.

**An unverified restore is recorded on the ops log.** One line naming the
correlation, the entity, the field, the handle, and that the image carried
no digest so nothing was compared. No security event. This satisfies the
condition attached to the ruling above on old images, and keeps the two
streams saying what they already say: the security stream is for what means
something is wrong, which is a digest that does not match or an object that
is not there, and a revert that had nothing to compare is neither. Owner's
answer of 2026-09-09.

## Four answers on the ops log and the blob chain (2026-09-10)

**The unverified restore rides the revert's own command group, not an ops
log.** The owner ruled on 2026-09-09 that the record goes to "the ops log"
and not to the security event stream, which would be noise. There is no ops
log: no logging framework in the workspace, and `implementation.md`'s
structured logs are still `[proposed]`. On 2026-09-10 he deferred the
choice of home, and it is decided here as `CommandGroup::restored_unverified`,
a second chain trailer under tag `0x02`.

The argument is his own readership ruling applied to this record. An
unverified restore is a fact about a tenant's data, made by one of their
members, in their space. The deployment artifact is the operator's, admits
entries by a rule this record fails (it is not a denial, its subject is a
member, and it has a space), and can be switched off by a toggle that would
take this record with it. The command group is the same hash-chained
artifact as the act it qualifies, so the statement and the thing it
describes cannot be separated, cannot disagree, and cannot be retained
differently. Its reader is the space owner, in the pane that already renders
group detail.

Adding a second trailer tag required a canonicity rule, and the one taken
generalises rather than enumerating: present tags render in ascending order,
and a decoder accepts a byte only if it is a known tag strictly greater than
the last one seen. That refuses an explicit `0x00`, a repeat, the known tags
out of order, and any unknown tag, all as `TrailingBytes`. A third flag takes
`0x03` and needs no new case.

**Who may read the deployment operational security log.** The backend's own
admin, and no one else. Refused: space owners, and the subject of an entry.

A per-space filtered feed is refused because the entries name principals who
are strangers to that space and are often another tenant's users, so the
filter would disclose one tenant's people to another, and every filter bug
would be a tenancy breach in the artifact where a breach is worst. A space
owner's real question, whether someone is probing them, is answered by a
derived count that names nobody. The subject is refused because reading
entries about oneself is a live oracle on what the platform saw and when,
which is what a prober needs to pace the next sweep.

The platform reading its own artifact was considered and found to have
nothing to do: the platform acts on these events as they occur and counts
them, which is how the deny-volume alert already works, so reading its own
record back afterwards has no caller. Not built, and recorded here so it is
not rediscovered as an omission.

**The concentration this leaves, filed rather than papered over.** The
backend admin reads the artifact, prunes it, and can switch it off. The rule
that every read appends an entry to the artifact helps and does not close
it, since whoever can prune can prune the read records too. The honest fix
is a role that reads and cannot prune or configure, which has nothing to
build on today because roles resolve per space and there is no
deployment-level role concept. It is an item, not a solved problem.

**The collapse key is a bug and is being fixed.** `Scope::Deployment` is a
unit variant carrying no space, so the deployment stream's collapse key is
`(principal, op)`, which is the version `security-logging-design.md` §6.1
explicitly rejected. A sweep across ten thousand spaces collapses to one
entry with a count. The artifact that exists because a sweep is invisible in
any one space's stream cannot presently see a sweep either.

**The platform surfaces stop authorizing against a caller-named space.**
Approved on 2026-09-10, and a prerequisite rather than a tidy-up, since the
reader ruled above is reachable over HTTP. Two defects, both since fixed by
D0 (`bbcf0d0`, 2026-09-11) and stated here in the past tense because a
ruling written while something was broken reads as a live bug forever
otherwise. `authorize` resolved the check space from `x-ordo-space`, so the
owner of any space could read a deployment-wide disclosure, confirmed at
200 where it owed 403. And `platform.security.alerts` was mounted with its
op deployed nowhere, so it answered 403 to everyone. Both now carry
regression tests.

## The tenancy topology, and the scale that shaped it (2026-09-12)

**One tenant, one key, with encryption at the database rather than the
cluster.** The owner: "encryption at database not cluster and one tenant
per database, that allows the distinction without the overhead of many
instances." The reasoning is sound and the mechanism is specific: Postgres
makes a database the unit you connect to, and `pg_tde` keys the unit you
connect to, so the key boundary and the query boundary coincide. A volume
key cannot do it, because a volume covers the whole cluster; per-instance
customer-managed keys fail for the same reason.

**Then the scale arrived and reshaped it.** The target is tens of
thousands of spaces per deployment. A database per tenant at that size is
roughly 75 GB of system catalog before any data, an autovacuum launcher
with three workers cycling ten thousand databases, and migrations measured
in hours. Connections were not the constraint, and an earlier version of
this argument said they were: a transaction-mode pooler on the cluster
host opens backends only for concurrently active tenants, so the ceiling
is concurrency rather than tenancy.

**The resolution is that routing is per space and data-driven**, so the
long tail points at one shared target and a few point elsewhere. Every
cost above scales with the number of distinct targets, not with the number
of spaces. A dedicated database becomes a tier a tenant is promoted into,
not a default everyone pays for, and true BYO-DB is the same mechanism
pointed off-cluster. Planned in `plan-space-routing.md`.

**What carries the per-tenant key at that scale is what already exists.**
Application-level sealing gives one key per tenant across tens of
thousands of them with no storage overhead, because the boundary is in the
data rather than in the storage. Crypto-erasure stays per tenant.
Cluster-wide volume encryption then covers the plaintext skeleton, and one
key for the cluster stops being a defect because the per-tenant boundary
is handled above it. The four software defences (composite primary key,
composite foreign keys, the planner's table-and-predicate, compiled
guards) remain the only isolation almost every tenant gets, which is why
the isolation suite of checklist 11-27 matters more under this topology
rather than less.

**Where each log lives.** Owner-confirmed: "both makes sense." A tenant's
database holds `$commandlog` and its checkpoints, the space-scope security
stream and its checkpoints, and the blob tables, because they travel with
the data on export and die with it on erasure. The control database holds
the deployment-scope security stream, identity, access and grants. The
deployment stream cannot move into a tenant's database for two reasons: it
holds denials by non-members and the cross-space sweep, which belong to no
space, and a caller can name a space that does not exist at all, so it must
be writable without opening any target.

**One consequence that is easy to miss.** Cross-pod coordination rides
Postgres with no broker, and a `NOTIFY` reaches only listeners on the same
database. A space on an attached target therefore needs its own
invalidation listener, or that space's `?live` subscriptions go silently
stale. The failure mode is silence rather than an error, which is why
`plan-space-routing.md` gives it its own increment.

**And one limit no topology fixes.** The WAL is cluster-wide: one Postgres
instance is one write-ahead stream carrying every database's writes, and
it feeds every standby and every archive. So per-tenant keys cover tables
and indexes and never the write stream. Application-level sealing is the
only thing that protects a value there, because a sealed value is
ciphertext before Postgres ever sees it. That is the sharpest argument for
sealing in this corpus, and the topology is what creates it.

## The third home, and what checking it turned up (2026-09-12)

**Ordo names two homes for data and needs three.** The control plane holds
identity, access and grants: infrastructure, not modelled as entities, with
no chain of its own. Packages hold code, distributed as signed git
artifacts and installed rather than queried. Tenant spaces hold customer
data. Nothing holds platform-owned **runtime content**: a tool catalogue,
system agents, reference data, a published listing.

The catalogue forced the question. The owner ruled it must not be
replicated, on the ground that a catalogue is the one thing that should be
singular and fifty copies give "what is tool X" fifty answers.

**The fill is a system space**, platform-owned, with a universal read
window. It takes a chain, guards, versioning, deploys and export unchanged,
and needs no entity kind and no placement syntax. Its only distinguishing
properties are that nobody erases it and everybody reads it.

**Third-party content does not go there.** A third party is itself a
tenant, so what they publish lives in their own space and is shared
outward: their chain records the change, their erasure removes it, their
consent governs the disclosure. A platform-owned space holding their
listing would make the platform the author of record for something it did
not write, which is the attribution failure the chain exists to prevent.
The test for placement is therefore authorship: is the platform genuinely
the author of this row.

**Discovery across publishers is the one genuinely new artifact.** Windows
and feeds both require a relationship, and searching across parties you
have no relationship with has neither. A derived projection in the control
plane, rebuilt from what publishers consented to expose, never a source of
truth, and pruned when a publisher is erased. That last property is why
projection beats replication here: copies in ten thousand tenant databases
cannot be recalled, and the corpus already states that revocation stops a
feed while already-copied data stays.

### A tenant cannot install an app

Found while checking the above, and it is structural. `$artifacts` and
`$activations` carry no space column, and the registry states that
activations record "which version **the deployment** points at, one". So
every space in a deployment runs the same schema, and **installing is a
deployment act**. A marketplace of installable third-party apps is not
expressible: tenant A cannot have an app that tenant B does not.

The decision that has to come first is what an app is allowed to be. If it
may define its own shapes, per-space activation is the only honest answer
and it is a large build touching the artifact registry, the planner's
cache, deploy propagation and every assumption that one schema describes a
deployment. If an app is configuration over shapes the platform already
defines, installing is a row and costs almost nothing. The denizens case is
the second kind: a "denizen" is configuration over a fixed shape.

### Sharing inverts under the routing topology

A window resolves its grant in the control plane and then reads the
**owner's** target, so under routing it is a second connection to a second
database and can never be joined against the guest's own rows. A feed
copies into the receiver's space, after which every read is local and
joinable. In a single-database deployment the window is the cheaper
mechanism; under routing it is the more expensive one. The modelling
guidance has to say so, because an author following today's advice would
choose the wrong one.

### Bulk data belongs behind a reference, not on the chain

An earlier note in this session claimed Ordo lacks an unchained tier for
high-volume data. It does not. The blob tier is that tier, and the
discipline it requires is batching: since C4 an upload is an ordinary
converge, so a thousand records in one sealed object cost **one** chain
append rather than a thousand. The object is sealed under the space key
before it leaves the process, so an S3-compatible store holds ciphertext
and crypto-erasure reaches it, and it is swept when unreferenced.
Aggregates stay as rows because they are small, rarely written, and are
what reporting reads. What is given up is ad-hoc query over the detail,
which becomes fetch-and-scan.

## The default database stays the control database (2026-09-13)

Owner, answering `plan-space-routing.md` Q-1: "Continue with S4 and the
recommendation." The recommendation was to keep the default target and the
control database the same database.

**What it settles.** A deployment that attaches nothing is one database, as
it was before routing existed: the control tables and every unattached
space's rows live together, and S1's `SinglePool` changes nothing on the day
it lands. Separating them would have made even the shared tier a target,
which is tidier and costs every small deployment a second database from its
first day.

**What it costs, stated so it is not rediscovered.** The control database
also holds the rows of every tenant that has not been promoted to its own
database, so its size, its vacuum load and its backup scope grow with the
shared tier rather than staying proportional to the deployment's own state.
A deployment that outgrows that separates them by attaching, which is the
mechanism routing already provides, rather than by changing this ruling.

**What it means for S4.** The control database owes both table sets: its own,
and the space-scoped set, because it is the default target. An attached
target owes the space-scoped set only.

## Who attaches a database to a space: the system (2026-09-13)

Owner, answering `plan-space-routing.md` Q-3 (`db-attach-gate`): "during
provisioning the system decides, and for changes that would require an
agreement - you cannot have an agreement for a space that does not exist and
you cannot have a space without a place for storage. Given this - I think the
system should own it." And on the customer side: "dev decides how that is
interfaced to the customers - we provide the architecture for provenance it
doesn't mean we own the relationship - but 2 party agreement makes sense."

**The ruling, in three parts.**

1. **At provisioning, the system chooses the target.** There is no owner to
   consent yet: consent needs a party, and the party does not exist until
   the space does. A space also cannot exist without somewhere to store it,
   so the choice of database is part of creating the space rather than an act
   performed on one. It is made by the deployment, not requested from a
   tenant.
2. **A change after creation needs both parties.** Moving an existing space
   between databases changes where a tenant's data, chain and security stream
   live, which is the two-party case the recommendation described. That is
   the live move already ruled post-release (`routing-migration`), and until
   it exists `spacedb::attach` refuses any change with `WouldMove`.
3. **How it is presented to customers is the app developer's.** Ordo provides
   the mechanism and the provenance: who chose, when, and under which
   custody, recorded where it cannot be changed silently. It does not own the
   relationship between the developer and their customers, so it does not
   dictate the consent screen.

**Where this departs from the recommendation.** The recommendation was the
bucket's gate (space-owner consent, step-up) plus the operator's grant, for
attaching at any time. The owner narrowed the question: at provisioning,
owner consent is not a coherent requirement, so the gate there is the
system's authority alone. The bucket attachment keeps its owner-consent gate,
because a bucket is attached to a space that already exists.

**What it means for S5.** Provisioning onto a database is a system action
with no consent step: the deployment decides the target, creates the
database, and records the attachment with `attached_by` naming the system
actor that made the choice. The record of it belongs in the deployment's
security stream as a custody action, since the space's own stream does not
exist until the space does.

## A space's members live in its own database (2026-09-14)

Owner, answering `plan-space-routing.md` Q-4 (`membership-home`): "In its own
database."

**What it settles.** `members` rows are entity rows: other entities reference
them through composite foreign keys, and a foreign key cannot cross
databases. So they have to sit with the rows that reference them. Once a
space has a database of its own, its members are there and nowhere else.
Export and erasure carry membership with the space because it is in the
space, and there is no second copy to drift.

**What it costs, and where.** The door's access engine was built by reading
every `members` row from one database. It now reads each space's members from
the database that space lives in: unattached spaces from the control
database, attached spaces from their target. A copy of an attached space's
members left on the default database is ignored rather than honored, so a
space's membership has exactly one source. That is one query per distinct
target when the engine is built, and almost every space shares the default.

**What it does not change.** Refreshing a live engine when membership changes
was an open item before routing and remains one.

## Routing a space to its own database: what the build settled (2026-09-14)

`plan-space-routing.md` S1 to S7 are built. Four owner rulings shaped it:
the default database stays the control database (Q-1, 2026-09-13); moving a
live space is post-release and routing must be able to carry it
(`routing-migration`, 2026-09-12, filed as 18e); the system chooses a new
space's database at provisioning and a later change needs both parties
(Q-3, 2026-09-13); and a space's members live in its own database (Q-4,
2026-09-14).

**What the build found that the plan did not know.**

- **The host's audit chain had never reached the database** (11-28). Both
  write paths built command groups in memory only, and the counter that
  minted row ids, correlations and command times restarted with every
  process. The chain is now durable and a deployment's ids and times no
  longer repeat across restarts.
- **Every target owes `$jobs`**, because a write enqueues its invalidation in
  its own transaction. The "two DDL sets" the plan proposed were not
  needed: the split is which database a row is sent to, not which tables a
  database has.
- **A space's own attachment row is not its data.** Provisioning's emptiness
  check first counted `$spacedb` and refused every retry of itself.
- **Two unit bugs in blob collection surfaced once command times became
  real.** The chain's `Timestamp` is seconds; the collector compared it with
  milliseconds twice. Nothing inside the undo horizon was collected early,
  but the report named the wrong holder and lagging mirrors did not hold the
  sweep back.
- **Three loops the plan meant to route have no runner in the host:** blob
  collection, job claiming, and the attestation writer. The supervisor that
  runs them must iterate `targets()`.
- **A schema change never reaches an attached database.** Provisioning
  applies the schema once and the load gate checks only the default. This is
  18d's "migrations applied N times", and it has to land before a second
  schema version meets an attached space.
- **Two tests assumed `localhost` answers `127.0.0.1` first.** The resolver
  does not promise an order, and this machine began answering `::1` first.
  Both tests were fixed to accept either loopback rather than retried.

**What is open.** S6b (the attestation delivery drain reading each space's
chain from its own database), migrations across every database, the
supervisor's loops, and 18e.

## The local cache is a replica, and IndexedDB is the default (2026-09-14)

Owner, on `plan-client-cache.md` Q-1 (`cache-shape`): "This is supposed to
function like jazz - so yes replication - that's the whole point of the
indexdb and fast responsive usage - its pretty critical." And on Q-2
(`cache-home`): "Cacheing in indexdb should be the default check up on our
design docs."

**What it settles.** 5g-pre builds a replica of rows on the device, not a
cache of answers. The device holds rows and answers queries from them,
including queries it has never sent to the server, which is the Jazz-style
benefit. The recommendation to cache answers first is overruled. Caching is
on by default and lives in IndexedDB; it is not an opt-in.

**What it costs, stated now so it is planned rather than discovered.**
Three things that do not exist yet: a change feed from the server (20d); a
device engine that evaluates queries (`f7-vault-plan.md` Q10 recommends
Postgres compiled to WASM); and a feed scoped to each principal. The third
is the hard one. Guards are evaluated on the server and a device may hold
only rows its principal may read, so a membership or acl change has to be
able to hide rows a device already holds. The plan is being revised against
the design documents before any of it is built.

**Still open:** Q-3 (`cache-key`), the at-rest key for v1.

## Key releases are recorded in the space's own stream (2026-09-14)

Owner, on `key-release-home`: "Take the reccomendation." The recommendation
was option (a): route a key release about a space into that space's
security stream. An earlier answer on the same row leaned toward
dual-writing with reconciliation; this answer supersedes it.

**What it settles.** A tenant auditing their own custody is the party who
should see that the platform opened their key, for which purpose, and when.
The routing rule that keeps other parties out of a tenant's stream exists to
stop a stranger writing into it, and the platform is not a stranger to a
space whose key it holds. So custody acts about a space are recorded in that
space's stream: one record, with the reader who cares about it.

**What it costs.** Key releases are the largest share of security events
and do not collapse, so that volume moves from the deployment stream into
tenant streams. And the deployment stream no longer holds every platform
custody act: an operator reviewing custody reads the deployment stream and
each space's.

**Built** the same day: a `Custodian` relationship, passed only by the key
provider's metering wrapper, routes a key release to the space whose key was
opened. An unverified subject is still checked first and reaches no tenant
stream.

## The replica's feed, engine and key (2026-09-14)

Owner, on the three open questions of `plan-client-cache.md`: "Take all
reccomendations and update doc."

**What a device may receive (Q-4, `replica-feed`): op-scoped sync.** A
device keeps named ops in sync, and for each the server sends row-level
changes to that op's result for the caller, as upserts and deletes at a
position. Guards are applied by the server exactly as for any read, so a
device can only ever hold what an ordinary call would have returned, and an
access change removes rows because it already invalidates the ops it
touches. A principal whose reads are unguarded may sync whole entities,
which is the whole-space feed as a special case rather than a second
mechanism. A per-principal filtered command feed is not built.

**How a device answers new questions (Q-5, `replica-engine`): local
refinement now, Postgres in WASM with F7.** The device filters, picks and
sorts synced rows with the Tier-0 refinement operators Ordo already defines.
Full local pipelines arrive with F7, on the engine `f7-vault-plan.md` Q10
recommends. Evaluating Ordo pipelines in JavaScript, a second implementation
of the language, is the option ruled out.

**The at-rest key for v1 (Q-3, `cache-key`): a device-local key.** A
non-extractable key held on the device, which protects a copied disk or
browser profile and not a script running in the app's origin while
unlocked. The store is shaped so the passkey-PRF wrap `implementation.md`
proposes and the server-released key of 5g-rev slot in without a change of
format.

**What followed from them, done the same day.** `definition-language.md`'s
mirror sentence said "whole-space command feed", which is only safe for a
principal who may read the whole space; it now describes op-scoped sync, and
the neighbouring sentence no longer limits delta-push to high-churn feeds.
`bootstrap.md` input 33 defaulted the mirror off for managed spaces; it is
now on by default, per the 2026-09-14 ruling that caching is the default.

## The replica feed's state, id and size (2026-09-15)

Owner, on the three questions of `plan-replica-feed.md`: "1 - take
reccomendation", "2 - take reccomendation", and on the cap: "I think a cap
like that would be arbitrary - instead it should paginate over time and be
honest that over 10k will take a while - add in some waits to not overload
the db (first loads)".

**State (Q-1, `feed-state`): the subscription remembers its last result.**
For the life of a connection the server keeps each synced row's key and hash
and sends only differences. A reconnect loses that memory and reloads each
synced op, paced like a first load, while the device keeps answering from its
store and no row moves backwards. A completed load is the whole result, so
rows it did not include are released.

**The id (Q-2, `feed-id`): required in the projection.** An op can be synced
only if it returns its anchor entity's `id`, so a feed never sends anything
an ordinary call to the same op would not.

**Size (Q-3, `feed-cap`): no cap.** A fixed row limit would be arbitrary.
The load arrives in pages with a wait between them, so a large result never
arrives as one read and never holds a long snapshot open, and every page says
how many rows have arrived and whether the load is complete. An answer taken
from a still-loading op says it may be partial, which keeps the concern the
cap was for without the cap. The same pacing and coalescing apply to the
re-read each change causes on a large op.

**Raised by the owner's question "on reconnect the client gets everything?
Only for live?"**: yes, a reconnect reloads, and only synced ops, which ride
the live connection. Resuming without a reload was Q-4, and was ruled on
2026-09-16: build it, on a hash the two sides compare. See below.

## Local refinement, resume, and generated UI (2026-09-16)

Three answers off the board, all in the owner's own words.

**The device may answer refinements the door refuses (`refine-tier-local`):
"Take suggestion."** The suggestion was A now, C when the refinement feature
is scheduled. So C6 stands as built: the device validates a filter against
`refine.fields` and does not consult `refine.tier`. The reasoning is that the
tier governs work the *server* does, since a refinement lands after the op's
`take` and after the page window and the door charges for it; rows already on
the device cost the server nothing, and every op ships `tier: "none"`, so
honouring it would have refused the whole catalog and shipped C6 inert.

The cost is recorded rather than hidden: the same envelope answers on the
device and earns `invalid_refinement` at the door. That ends when C lands —
the executor applying `$refine` and a spelling for the per-op opt-in — which
is now scheduled with the refinement feature rather than left implicit. The
device's evaluator is already the same rules, so the gate becomes one clause
when the server catches up.

**A reconnect may resume instead of reloading (`feed-resume`): "Can it share
a diff hash or version hash and if they disagree on reconnect reconcile. Add
it to C4 work."** This reverses the recommendation to leave resume for later,
and the reversal is the cheaper answer, because the material already exists:
under the ruled state model the subscription keeps each row's `(entity, id)`
and sixteen bytes of SHA-256 of the row it last sent (`ordo-api/src/sync.rs`).
A reconnect can therefore carry a hash of what the device holds; if the two
agree, the load is skipped and the feed resumes from the device's position,
and if they disagree the server reconciles by reloading, which is exactly
today's behaviour. So the reload is the floor, never the norm.

One thing the build must not get wrong: the digest is taken over the server's
own serialization of a row, so a device that recomputed it would be inventing
a canonical form and would disagree for reasons that have nothing to do with
staleness. The device therefore returns the digests it was given rather than
computing new ones. Filed as **C4d** in `plan-replica-feed.md`.

**Generated UI waits for the deployment (`uigen`): "Lets build this later on
- add it to the post-deploy roadmap. At that point all the system bits will be
settled."** 14a and 14b stay as built. 14c, the surface descriptors that need
five annotations lexed, moves to the post-deploy roadmap rather than staying
an open question, and the lexing question goes with it. The earlier answer on
this row ("generated UI should be a feature we deploy") is not contradicted:
it is the same yes, with the timing now stated.

## The synced id follows ui-gen §1, and the ruling it replaces was wrong (2026-09-16)

**Ruled (`feed-id-uigen`): "Take reccomendation but launch fable to go through
it and confirm its the right choice and reconcile the conflicts."** The review
ran and confirmed the switch, with two amendments. So: **any anchored,
ungrouped read can be synced, and the anchor's id rides every row**, as
`ui-gen.md` §1 has said all along. This supersedes the `feed-id` ruling of
2026-09-15 ("a synced op must return its id"), which stands in the record
above as what was decided then.

**The premise of that ruling was false, and the correction matters more than
the ruling.** It argued that requiring the id in the projection "keeps one
promise exact: a synced op sends nothing an ordinary call would not return",
because the cursor carries the id "inside an authenticated token the client
cannot read". The token is **authenticated, not encrypted**:
`crates/ordo-api/src/page.rs` states the format as
`base64url(JSON{v,k}) . base64url(HMAC)`, and `encode` base64url-encodes the
plain payload. The MAC stops forgery; it does not stop reading. Any caller can
decode the last row's id from every page they already receive. The entitlement
class the ruling protected — "ids are withheld from this caller" — does not
exist anywhere in the system: `id` is reserved and implicit, so no author can
seal it or blind-index it, and it is the tiebreak of every row plan, where a
sealed value is never an order key (W047).

Two settled rules already said so and were in conflict with the ruling:
`ui-gen.md` §1 (the anchor's typed id "rides every list result row even when
unprojected"), and the std-index ruling that "an op lists `E` iff its rows
carry `E`'s typed id, which is `result.anchor`". The `feed-id` ruling was the
outlier, not ui-gen.

**Amendment (a): the predicate is stated positively.** "Anchored and
ungrouped" is not enough, because two shapes slip through it. `distinct`
replaces the tiebreak with the outputs, so `customers[name] distinct` has rows
that are not stored rows; today it is refused only by the accident of
`id_not_projected`, and under the new rule it would be admitted. A many-side
source join keeps the anchor at `q.from` while the rows are the far entity's,
so several rows share one anchor id. Both get named refusals rather than
accidental ones.

**Amendment (b): the id is attached to every paged read row, not only to feed
frames.** Otherwise the promise the original ruling was protecting breaks in
the other direction: the feed would send something an ordinary call does not.
Making the ordinary call honest to ui-gen §1 is what keeps the two the same.

**The cost was understated when the switch was recommended.** "One clause of
the rule, plus the id in C4b's frames" was wrong: the id has no channel to
travel on today, because `canonical()` builds each row object from
`plan.outputs` alone, so the attach point is in the executor and the change
crosses `ordo-plan`, `ordo-exec` and `ordo-api`, re-versions every read op,
and moves 23 ops in the market golden out of `id_not_projected`. The riskiest
part is locating the anchor id at the attach point: it must come from the
order-key outputs, handling a wrapped callee's hidden tiebreak and never
colliding with an author's own output named `id`. Key a synced op by a sort
value instead and the difference pass treats it as churn rather than failing.

**Found by the same review, and not caused by the switch:** `syncability`
never inspects `source_joins`, so a many-side join op that projects its id is
syncable **today**, and the subscription keys rows by that id, so sibling rows
overwrite each other. That is a defect in C4 as shipped and is being fixed on
its own, not as part of this switch.

## The policy blocks are documentation, and the docs will say so (2026-09-16)

**Ruled (`policy-grammar`): "Take reccomendation."** The recommendation was
**C now, A later**: mark the documented policy blocks as proposed so nobody
reads them as shipped, and schedule the grammar as its own increment when
policies are actually wanted.

**What was found, building C7.** `policy` is lexed — `lexer.rs` and
`token.rs` both know the word — and the trail ends there. There is **no
parser arm, no AST node, and no consumer** in `ordo-schema` or `ordo-plan`.
So every policy block `definition-language.md` documents is prose: `policy
spaces`, `policy commandlog`, `policy vault`, `policy llm-egress`, `policy
retention`, `policy compliance`. `security-hardening-review-2.md` had already
noticed one corner of it, recording that `policy mirror` and `policy keys`
are "referenced but absent from the policy grammar". The gap is wider than
that note: **none of them parse.**

It surfaced because C7's fourth item is `policy mirror { keep 90d | keep <n>
rows | full }` scope and eviction. The other three items of C7 are built. That
one could not be, and building it alone would have been the worst of the
options: a spelling invented for one policy, with five more arriving later to
find the grammar already shaped around the narrowest case.

**Why C rather than A now.** Marking them proposed costs an afternoon and
removes a class of false expectation — a reader of the language doc currently
cannot tell which of these six do anything. A grammar is a real language
change, and every keyword is forever, so it gets scheduled deliberately
rather than smuggled in under a cache increment. Nothing is thrown away by
waiting: the six blocks keep their spellings, and the increment that builds
the grammar makes them consumers of it.

**What this does not change.** `policy retention`'s enforcement — the
platform's nightly sweep as attributed system deletes — is likewise unbuilt,
and stays that way until the grammar exists to describe it. Mirror scope and
eviction remain C7's open half, now blocked on a named increment rather than
on nothing.

## What C4e settled on the way (2026-09-17)

The id switch was ruled on 2026-09-16 and built in two pushes. Three
questions fell out of building it that the ruling did not cover, recorded
here because each is a decision rather than an implementation detail.

**`$id` is the attached name, and it is refinable.** The anchor's id, when
the author did not project it, arrives under `$id`. `$` is the platform's
reserved prefix everywhere else (`$space`, `$refine`, `$caller`, and the
planner's own `$k{i}` and `$tb{i}`), and the lexer routes a leading `$` to
the parameter rule, so no author can declare a column that collides with it.
Because the id is a **genuine projected output** rather than a value spliced
onto a row in flight, it reaches `refine.fields` and the door's own field
manifest through the same list, and the two agree with no special case.
Filtering a synced list by row id is a reasonable thing to permit, and
excluding it would have meant teaching two producers to hide the same column.

**One decision feeds both halves of the contract.** `syncability` answers the
door with the key's name and `project_row_key` puts the output in place
before planning, and both read one function. Computed separately they could
return different names, and the descriptor would then say rows are keyed by
one thing while the wire keyed them by another. Nothing would detect that
until a device filed a row under a key the server never sends.

**`row_identity` derives from `sync`.** The two were computed independently
and answered the same question differently: `row_identity` was claimed for
every ungrouped read, including `distinct` results and many-side joins, which
`sync` refuses by name because their rows are output tuples and fanned-out
duplicates. A document that answers one question twice will eventually answer
it two ways, and this one already did.

**A blind-index op keeps its secret and gains its identity.** `customerByEmail`
still withholds its statement, because publishing a per-space digest would be
unstable and a membership oracle. It now carries `$id` like every other
anchored read. Withholding the statement and withholding row identity are
separate things, and only the first was ever intended.

## Computed write right-hand sides: four rulings the spec left open (2026-09-07)

DL settles that expressions are legal in a *"write RHS (per-row values in
bulk writes and rules)"* (§3) and gives the flagship spelling with its
guarantee — *"`{ n: n + 1 }` — computed server-side inside the row's
transaction; atomic increments without read-modify-write races"* (§6). It
does not say what happens where there is no row, nor what happens to the
things the write protocol does with a value *before* it writes it. Those are
this engine's, recorded here.

### 1 · The distinction is WHEN the value exists, not what it looks like

The obvious reading is "literals go one way, expressions another". That
reading is wrong, and the reason is that the write protocol is not a
statement builder — it holds values and reasons over them. Before a create
runs it proves the write guard against the *would-be* row from those values
(G10a). A sealed field seals plaintext; an indexed one digests it. A unique
key addresses a row with it. A closed value set is checked against its
ladder. A ref is proved visible to the caller (G12). The command log images
what landed.

So the line is drawn at whether the value has an answer *before* the
statement runs:

- **Row-free** — `"thirsty " + $plant`, `round($t / $n, 0)`, `hash($k)`. It
  has an answer, so it is given one and then it is an ordinary value.
- **Row-reading** — `used + 1`. It has no answer until the row is in hand,
  so every host-side use of it is off the table.

Reading the line as literal-versus-expression would have refused
`"thirsty " + $plant` in a create for no reason at all, or admitted
`memo + "!"` into a sealed column, which is a plaintext write.

### 2 · Row-free values are folded by the DATABASE, not by the host

The tempting implementation is a small host-side interpreter: it is a dozen
functions and it removes a round trip. It is refused. It would be a second
implementation of `+` over money, of `/`'s fixed scale, of `round`'s tie
rule, of `+`-is-concatenation-when-the-binder-says-text — agreeing with
`ordo-plan`'s renderer on the day it was written and drifting after, and
drifting *silently*, because both halves would go on producing numbers.

The value is obtained instead with one `SELECT` of the rendered expressions,
on the write's own connection, inside the write's own transaction; the
statement is then rewritten to carry the answer, exactly as
`substitute_bound` rewrites a `let` descriptor before planning. Expressions
are pure and deterministic (DL §3), so asking has no consequence beyond the
trip. There is one evaluator for the language's expressions and it is
Postgres, reached through one renderer.

### 3 · A statement that can create refuses a field reference — by name, and converge-or-create is not an exception

DL's guarantee is stated *"inside the row's transaction"*. Every mode whose
compiler label can be `creates` has a branch with no such row.

The plain create is obvious. **Converge-or-create is the one worth stating**,
because it is the case where a lenient answer is available and wrong: the
branch is `INSERT … ON CONFLICT`, decided by the database against a unique
index, and Postgres would happily let the `DO UPDATE` half read the existing
row. Admitting it would mean one statement whose right-hand side means
"increment" on one branch and something invented on the other — null, or the
field's default, neither of which the author wrote. Both available defaults
are answers nobody chose, so there is no answer and the refusal is by name,
quoting the mode label back (`creates-or-converges`) so the author can see
which of their statement's properties caused it.

The mirror rule is what makes this cheap to obey: **the value the platform
must hold**. A row-reading value is refused in five field kinds — sealed
(sealing needs plaintext), unique-keyed (rule 8 must know which row the
value identifies), ref (G12 must prove the target visible), closed-set (the
ladder is checked before the write, and a value outside it is a grant that
reads nothing and writes nothing), and content (a blob handle is minted by
an upload, a uri is validated at the door). None of these is a renderer
limitation and each names its own reason.

Refused **twice**: at registration, so the op does not deploy, and at plan
time, because the harness can plan an unregistered statement. Both read the
same `stmt_label` — the single authority §6 rule 1 already routes every
renderer through — so they cannot disagree about which statements have a
row.

### 4 · A misspelled column in a right-hand side is `projection_unknown_field`

GC088's rule applied to a new position: *"The class of a miss is decided by
the POSITION that named the field, not by the lookup that failed"*, and the
binder's `unknown_field` *"is internal vocabulary that appears in no closed
inventory, so it must never be what a conformance case sees"*. The position
here is a `set`'s braces, and the write path already answers
`projection_unknown_field` for a miss on the LEFT of the colon. The right of
the colon is the same braces and the same mistake, so it is the same class.
Minting a new one would have named where the mistake was noticed rather than
what is wrong.

### Found while building

- **The executor deadlocks on its own command log.** Six concurrent writes
  to ONE row that share an `Arc<Mutex<WriteLog>>` never return.
  `Executor::write_attempt` takes that `std::sync::Mutex` inside its
  `spawn_blocking` and holds it for the whole protocol — the
  `SELECT … FOR UPDATE` included — while also taking it on the async side,
  where the continuation that ends a transaction and releases its row locks
  lives. Lock order is log-mutex → row-lock in one place and the reverse in
  the other. **It is not the computed RHS's**: the identical hang reproduces
  with six plain `set teams { id: $t, used: $n }` converges, which contain
  no expression at all — the control was run before the finding was written
  down. `ordo_host::FixtureExecutor` shares exactly one such log across
  requests, so it is reachable at the served door under ordinary contention.
  Recorded with an executable `#[ignore]`d reproduction rather than fixed in
  passing, because the fix is a decision about where the in-memory chain
  sits relative to the transaction. Owed; checklist §6g.
- **`syntax-examples.md`'s money example did not work, and nothing could
  tell.** §4 carried `set leases { …, rent: round($annual / 12, 2) }` as the
  *working* contrast to the `implicit_rounding` beside it. It is the same
  error: `rounds_to_integral` says only `round(x, 0)`, `floor` and `ceil`
  convert, because *"money is minor units here, so `round(x, 2)` asks for
  two digits finer than a minor unit and there is no integral slot for
  that."* Nothing on the write path had ever type-checked a right-hand side,
  so the example was not working — it was unchecked. Corrected to
  `round($annual / 12, 0)`, with a test
  (`round_to_two_places_still_does_not_reach_a_money_slot`) so the doc and
  the engine cannot drift apart again. The doc was the wrong one: the
  engine's rule is argued at the site and follows from money being minor
  units, which DL §2 settles.
- **Two literal types are unwritable, and always were.** `eval_expr` has no
  arm for `DatetimeLit` or `DecimalLit`, so `set e { at: 2026-01-01T00:00:00Z }`
  is `invalid_binding_expr` today — not by any ruling, by omission. `decimal`
  is expression-only (DL §2, never a column) so that half is correct by
  accident; `datetime` is a real gap, and the fold path refuses to invent a
  literal for one rather than guessing at Postgres's timestamp text. Named
  here so the omission stops being invisible.

## The server stack's lean mark is overtaken by a GO'd plan (2026-09-18)

**Ruled (`lean-overtaken`): "Yes and document/update docs."** `roadmap-checklist.md` §19 (19m) flagged `implementation.md`'s "Server stack" section as `[lean]` over a subsystem that had, by the time of the flag, an owner-approved GO: `plan-runtime.md` (1,915 lines) specifies the binary, key custody and the worker supervisor against the code that already exists, each claim carrying its file and line, and closes with a done-when per increment (§6). A lean mark, "leaning this way, not settled," over a plan in that state misleads a reader into thinking the mechanism is still undecided.

The promotion states the mechanism `plan-runtime.md` settled, rather than only flipping the mark: `ordo-host` assembles around the load path that already exists (`deploy::load_active`, `crates/ordo-host/src/deploy.rs:357`); key custody runs behind the existing `KeyProvider` trait, with the production path wrapping `LocalKeyProvider` over a Postgres-backed `PgKeyStore` sourced from an operator-supplied master key rather than `TestProvider`'s constant; the worker supervisor runs the already-built loop steps in `crates/ordo-exec/src/triggers.rs` under a `Worker` trait with backoff and quarantine. `implementation.md` now points at `plan-runtime.md` §6 for the done-when of each.

**One fact stops this from reading as "done": the binary is not merged.** `crates/ordo-host/Cargo.toml` has no `[[bin]]` and there is no `crates/ordo-host/src/main.rs` (verified by grep against this worktree, matching `plan-runtime.md` §0.1's own check); the only `TcpListener::bind` in the tree is the playground's. The rewrite states that in the same breath as the mechanism, so "settled design" is not misread as "shipped."

The rewrite also corrects an adjacent claim the plan itself flagged (`plan-runtime.md` §7): `implementation.md`'s old text said "single static `ordo` binary," but the workspace ships two, `ordo` (the operator tool) and `ordo-host` (the server), as separate images with different bases (`plan-ci-images.md` §3.1). The *role* half of the old sentence is honoured (`ORDO_WORKERS` selects api/worker/all-in-one on `ordo-host`); the *single-binary* half was not, and left uncorrected here it would have kept contradicting the tree from inside a section this same ruling was promoting to settled.

## Skins are a package over the headless runtime, and the default four stay illustrative (2026-09-18)

**Ruled (`lean-overtaken`): "Yes and document/update docs."** `roadmap-checklist.md` §19 (19o) flagged `ui-gen.md` §3's "Skins" heading as `[lean]` over a shape the owner has since approved and put into build: the Watermelon skin and its shadcn registry emission are homed at 17p, on "the approved build order" (`roadmap-checklist.md` 17p). The general claim a skin makes, that it is a package over the already-settled headless runtime (`ui-gen.md` §2, "settled as shape"), was never itself in doubt; what carried the lean mark was the whole section, including the specific default components, undifferentiated.

The promotion separates the two. §3 is retitled "Skins [settled as shape] and static renderers [settled]" and states the mechanism plainly: a skin takes the descriptor and state `useSurface` returns plus a theme, and renders a surface; nothing else crosses the boundary, so any design system can write one and the runtime cannot tell which skin is drawing it. That is the part 17p's approval carries.

**The specific block set does not get the same promotion.** `<OrdoTable>`, `<OrdoDetail>`, `<OrdoForm>`, `<OrdoMetric>` are marked `[illustrative]`: they are one design's answer to "what does a default skin look like," not a commitment the shape depends on. Nothing in 17p's homing or in the code (`clients/ordo-react/` ships the headless hooks; no default-skin package exists yet in `clients/`) settles that these four, specifically, are the shipped default rather than a placeholder for it.

## Form generation is a shipped mechanism, and layout is a separate, still-open question (2026-09-18)

**Ruled (`lean-overtaken`): "Yes and document/update docs."** `roadmap-checklist.md` §19 (19p) flagged `ui-gen.md` §10's "Form generation from write-op params" bullet as `[lean]`, but the mechanism it describes had already shipped: `useOpForm` (`clients/ordo-react/src/hooks/useOpForm.ts`, field derivation in `src/form.ts`) derives a field model from any write op's `params[]`, tested against all 165 ops of the golden descriptor with no server.

The old text was wrong about more than the mark. It said forms derive from `std.E.create` params plus annotations, `choices` for dropdowns, `enum` fields the same way, `danger` for confirm-and-step-up, and none of that is what shipped. Annotations were never the path: `definition-language.md` §10.3's `label`/`desc`/`order`/`choices`/`danger` are still lexer-inert (`ui-gen.md` §9.1), so a form built on them could not exist. What shipped instead reads the descriptor's declared facts directly: reference fields (`id_entity`) resolve to candidate-picker sources through `id_types[].list`/`.get`, dropdown options come from `field_ref` + `enum_values` (the parameter's declared field and that field's own enum set, resolved by the emitter), and the mechanism runs against any op with `params[]`, not only `create`. `danger` has no shipped source to read, so `deriveForm` proxies it from the op's `kind` (a delete is destructive by derivation, `src/form.ts`'s own comment says so) rather than leaving an always-null field a renderer would learn to trust.

**What stays lean, and why it is a different claim.** A field model (what fields exist, their types, bounds, and candidates) is not a form layout (how those fields are arranged, grouped, and presented on a page). The mechanism settles the first; the second was never addressed by it and is not addressed by this ruling. `ui-gen.md` §10 now states the two as separate lines so a reader cannot infer a shipped layout from a shipped field model.
## The server binary's configuration, and when a deployment must have a blob store (2026-09-07)

`plan-runtime.md` R1, plus the config decision `plan-blob.md` deliberately left
to whoever wrote the entrypoint (checklist 17g: *"somebody must rule where a
deployment's default store comes from"*). Recorded together because they are
one act — the binary is the first thing in the tree that has to answer either.

**Refusal postures, and what is never available.** Three: refuse to boot (the
deployment is unsafe or incoherent without it — every secret is here), refuse
the *feature* and say so (the platform is coherent without it, and pretending
otherwise turns a missing option into an outage — the checkpoint keys), and
default-and-publish (a tuning knob with one obvious right answer). Never
available: silently defaulting a secret, and silently disabling a mandatory
control. `Metrics::default`'s own doc drew that line for the deny-volume alert
— *"a mandatory control that a missing variable silences is not a control"* —
and the binary applies it everywhere.

**Every environment refusal precedes every dial.** Not tidiness: a deployment
missing its token key should fail in milliseconds with one message, not after a
pool timeout against a database it should never have touched, and a
misconfigured replica in a rolling deploy should die before it takes a
connection slot from a healthy one. `RuntimeConfig::from_env` does no I/O, and
the R1 suite proves the property by pointing the refusals at an unroutable
address: a binary that dialled first would sit in a connect timeout instead of
answering in milliseconds.

**A database nobody has deployed to is not a boot failure.** `Deployment::empty`
is a first-class state, distinct from `Deployment::pinned` (which carries a
program with no gate behind it and must never appear in a binary). Refusing on
`NoActiveVersion` would make the ordinary first `ordo apply` impossible: you
would need a program in the database before you could start the server the
program is deployed to. Every surface reads the empty state correctly without
being told — liveness is 200 because the process is alive, readiness is 503
naming `registry` and `shape` because there is no canary op and no report.

**Found while building it:** a no-program deployment has no `members` table
either, because the directory comes from a program's DDL — so the access engine
refused to build and the binary would not start, defeating the whole posture.
Asking the *schema* does not answer it (`members` is a platform entity
`ordo_schema` injects into every program, the empty one included), so the
question goes where the answer is: `to_regclass`, not the error text. A
directory table that does not exist is an un-applied deployment; a directory
table that exists and will not read is a failure, and stays one.

---

### The blob store: `ORDO_BLOB_STORE`, and the rule for when one is required

**Ruled: a deployment must configure a blob store exactly when its loaded
program declares a `blob` field that is not `inline`.** Three cases:

- **No blob field.** Every blob route addresses `entity.field` and
  `PlatformBlobs` refuses a field that is not a blob, so no upload is
  reachable. A required store would be a dependency nothing can touch, and it
  would make every deployment of every program written before blobs need a
  bucket.
- **`blob inline` only.** Those bytes ride the row's sealed payload and never
  reach the object store — that is what makes the tier GC-free by inheritance
  rather than by assertion. Requiring a bucket for bytes that never leave
  Postgres would be requiring the wrong thing.
- **A stored `blob` field.** The declaration is a promise the schema has
  already made: the column exists, `std.E.create`/`.update` accept the field,
  the interface descriptor publishes it, and a generated client renders a file
  input. With no store the first upload is a 500 in production, on the path the
  program was written for. So it is not "refuse the feature and say so" — the
  platform is not coherent without it, because the program has already said the
  field is there. It is a refusal, and the refusal names the field.

**The one refusal that follows the dial, and the exception is forced.** Whether
a store is *needed* is a property of the deployed program, and the program
lives in the database. So `ORDO_BLOB_STORE`'s syntax and the S3 binding's own
required variables are validated in `from_env`, before any dial — a typo still
refuses in milliseconds — and only *necessity* waits for the load. A swap
applies the same rule and **refuses rather than installs**: the node keeps
serving the version it has, which is the deploy channel's stated policy, and
the alternative is a door that publishes a file field and 500s on the first
upload.

**`fs:` needs its own dev flag.** `FsStore`'s module doc asked for exactly this
and gave the reason — *"a deployment configuration naming it outside dev mode
refuses to boot with the reason, exactly as `LocalKeyProvider::from_env`
refuses a default master key."* The authority is
`BlobStore::production_ready()` rather than the scheme string, so a future
binding that answers `false` inherits the rule without anybody remembering to
add it. And it is `ORDO_DEV_UNSAFE_BLOB_STORE`, **not** a reuse of
`ORDO_DEV_UNSAFE_TEST_KEYS`: the two unsafe different things, and a deployment
can legitimately want one without the other — a self-host laptop running a real
local master key is not a deployment that agreed to put customer files on a
filesystem. One flag unlocking both would hand an operator who asked for the
first the second in silence, which is the failure the explicit selector exists
to prevent.

**One resolver, held by the runtime.** `SingleStore` is the resolver, honest
about the absence it stands in for; per-space BYO buckets arrive through
`std.space.attachBucket` and a resolver that reads what it attached. It is held
on the `Runtime` rather than constructed per consumer, because the sweep and
the exit ops both need it and two resolvers over one bucket would be two
answers to "where are this space's bytes".

---

### The `psql` door degrades under load, and now there is evidence

`plan-runtime.md` §0.5 argued from first principles that `MemberAccess` — which
answers the door's access question by forking the `psql` **binary**, once per
request — is not a deployable door: a distroless image cannot carry it, and it
is a blocking process spawn inside an async request path.

Adding the R1 suite (which spawns real `ordo-host` processes, each holding a
pool against the same cluster) made `deployment.rs`'s
`the_deployment_surface_is_mounted` fail once in a full-workspace run and pass
in isolation and on every rerun. The assertion that broke compares the catalog's
op list against the interface descriptor's — **two surfaces the same
`AccessChecker` scopes, one op at a time**. A `psql` fork that fails under
process pressure drops an op from one list and not the other, which is exactly
the shape observed. Recorded rather than fixed, in the same spirit as
`lock_wait.rs`: the fix is the one R1 already made for the *deployable*
(`deployment_engine_pooled` + `IdentityAccess`, no fork anywhere), and moving
the conformance harness onto it is a separate change to suites this increment
does not own. Filed so the next person to see it red does not go looking for a
catalog bug that is not there.

---

## The sealed-master file, and two presence checks that were not custody checks (2026-09-07)

`plan-runtime.md` R2b. Two rulings and one defect the build found.

**Ruled: the master key gets a source, and the environment is one of them
rather than the only one.** `LocalKeyProvider::new(secret, store)` already took
bytes, so `MasterKeySource` is a *source* for those bytes and not a fifth
implementation of a fifteen-method trait — which is the whole reason R2b stayed
small. `ORDO_MASTER_KEY` is kept and stays first-class for a laptop; what is
refused is it being the **only** way, for five mechanical reasons:
`/proc/<pid>/environ`, `docker inspect`, crash reporters that dump the
environment, children inheriting it (the operator tool forks `psql`), and — the
one that decides the design — **the stretch is one HMAC**, with a
sixteen-character floor, so a sixteen-character passphrase behind one HMAC is a
sixteen-character passphrase.

**The format is the crate's own envelope, deliberately.**
`ordo-master-v1 ‖ key_id ‖ salt(16) ‖ seal_with(kek, 0, "master", key_id,
master32)`, with `kek = argon2id(passphrase, salt)` at the OWASP profile
`ordo-identity` already pinned. No new crypto is designed: a key file whose
format was invented for the occasion is a key file nobody can audit. The
`key_id` is the envelope's AAD field, so an id edited in the file does not open
— an id that could be changed without invalidating the envelope would be a
label rather than a binding. The **profile is not in the file**: reading cost
parameters out of a file the attacker also holds means an attacker who wants a
cheap cracking target can hand you one.

**Why the file source ships before any KMS.** A KMS at boot makes a KMS outage a
boot outage, and boot is exactly when a fleet is rolling; the file is testable
in CI with no secret, no cloud account and no credential; self-host is
first-class by ruling, and a custody story whose only non-plaintext option is a
cloud KMS makes it second-class; and it is the same file a KMS source later
opens, so the move is a provider swap plus a re-wrap, never a migration. The
disclosure that KMS will owe is written now rather than later by somebody with
a reason to soften it: **a KMS buys "the key is not on the disk and not in the
environment"; it does not buy "the key never exists in this host's RAM."**

**Two dev flags, not one.** `ORDO_KEY_PROVIDER=test` needs
`ORDO_DEV_UNSAFE_TEST_KEYS`; `ORDO_BLOB_STORE=fs:` needs
`ORDO_DEV_UNSAFE_BLOB_STORE`. Each names the thing it unsafes, because a
deployment can legitimately want one without the other and one flag unlocking
both would hand an operator who asked for the first the second in silence.

---

### The defect: `release` and `current_epoch` are presence checks, not custody checks

Found while writing the R2b tests, and it reached further than R2b.

`LocalKeyProvider::release` asks whether an SMK **row exists**.
`current_epoch` takes the maximum epoch of the **DEK rows**. Neither unwraps
anything. So a deployment holding the *wrong* master key — a mistyped
`ORDO_MASTER_KEY`, the wrong passphrase file, a sealed file fed to `new`
instead of `from_master` — passes both and fails at the first read of a sealed
field.

That was `/health/ready`'s KeyProvider probe. A replica booted with the wrong
master **reported ready**, took traffic from the load balancer, and then failed
every sealed read. The worst available shape: the failure arrives after the
routing decision rather than before it.

`blind_index` is the cheapest question that actually unwraps — it derives the
per-field index key from the space's SMK — and it takes no lease and releases no
DEK, which is exactly what readiness needs. It is now what `/health/ready` and
the binary's boot check both ask, over a reserved `$custody`/`probe` name pair
whose digest addresses nothing.
`ordo-crypto`'s `a_wrong_master_is_invisible_to_the_presence_checks` pins the
distinction so it cannot quietly regress.

**The adjacent trap, closed by construction.** `LocalKeyProvider::new` *hashes*
what it is given; a sealed file holds the master **itself**. Feeding one to the
other produces a provider whose master is `HMAC(master)` — opens nothing, no
error until the first read, and indistinguishable from a wrong passphrase. So
`from_master` exists, `derive_from_secret` is the one spelling of the stretch,
and a test pins that the two constructors are not interchangeable.

---

### The ceremony

`ordo keys seal-master | init | rewrap-master`. Three notes worth keeping:

- **The passphrase comes from a file**, not a prompt and not a flag. A flag is
  in the shell history and in `ps`; a prompt cannot be scripted, and a ceremony
  that cannot be scripted is one somebody performs by hand differently each
  time. A file is what the orchestrator's secret store already mounts.
- **`seal-master` refuses to overwrite**, through `create_new` rather than a
  check-then-write window. Clobbering a master key file erases every space's
  custody, and there is no escrow and no recovery.
- **`rewrap-master` is not SMK rotation**, and the output says so in as many
  words. A re-wrap moves the *wrapping* of each space's SMK — one row per
  space, no envelope and **no blind index** moves, because BIKs are
  SMK-derived and the SMK is unchanged. An SMK rotation is a full re-index of
  every `sealed indexed` column. The two differ by orders of magnitude, and the
  test asserts the digest is byte-identical across a re-wrap precisely so the
  cost model in the docs cannot silently become wrong. `--dry-run` does every
  unwrap and re-wrap and rolls back, so an operator finds out the old
  passphrase is wrong before anything moves.

## The security drain stays outside the supervisor, and its retention pass moves inside (2026-09-18)

**Decided while finishing R3 (checklist 17d); open to the owner's review.**
`plan-runtime.md` §3.4 says R3 wraps `SecurityDrain` in the `Worker` shape
"so it appears in the census and the metrics with everything else, and
changes none of its behaviour". Those two halves cannot both hold, because
adopting it as a supervised worker changes three of its guarantees:

1. **When it starts.** The drain starts in `assemble`, before the door, so a
   denial recorded by the first request is already on its way to the
   database. Supervised workers start in `serve`, after `assemble` has
   returned.
2. **When and how it stops.** `SecurityDrain::shutdown()` flushes at §1.5
   step 4, after the workers have stopped, and returns what it wrote or the
   fault. A worker's `on_shutdown` runs at step 3, inside the supervisor's
   grace, and its error becomes a log line.
3. **Whether it can be stopped for failing.** A supervised worker is
   quarantined after ten consecutive failures. Ten drain failures in a row
   is a database outage, and quarantining the drain then would turn a
   bounded, declared loss (the recorder's outbox bound and the
   `EventsDropped` link) into an unbounded one nobody is told about. The
   drain already answers a failing store its own way: `failing_since`, and
   `degraded` on `/health/deep` after ten minutes.

**Decided:** `secevent.drain` keeps its own task and lifecycle, and the
supervisor reports it as an `External`: it appears in the `workers` census
with `enabled: true`, `supervised: false`, and its own `appended`,
`failures` and `pruned` counters. The census half of §3.4 holds and the
behaviour half is untouched.

**Retention is the part that does move.** The drain used to run the
security-stream retention pass from its own loop. That pass is a cadence
over a schedule row with nothing to do with the flush guarantees above, and
§3.4 lists it as its own leader-only worker, `secevent.retain`. The pass is
now a `Retainer` the drain builds and hands out
(`DrainConfig::supervised_retention`), and the worker drives it. It is the
same claim (`claim_retention_tick`) and the same sweep, and it writes the
same `DrainHealth` counters. A drain started on its own, as every drain test
starts one, still retains by itself, so nothing that relied on the old
behaviour lost it.

**A related mechanism, recorded because the obvious version is wrong: how a
leader learns it lost leadership.** It cannot find out by calling
`Leadership::acquire` again. Advisory locks are re-entrant per session and a
new `acquire` dials a new session, so the answer says whether *another*
session could take the lock, not whether this one still holds it. The only
way a session-scoped lock is lost is that the session ends, so the leader
asks its own session (`Leadership::is_held`, a `SELECT 1` on the lock's
connection) on every slow tick, bounded by the tick so a half-open socket
cannot hang the overseer, and treats a timeout as a loss. A follower asks
the other question, `acquire`, on the same tick. A loss is counted before
any retake, so a lock that was lost and retaken inside one tick shows as two
transitions on `ordo_supervisor_leadership_changes_total` instead of none.

## The binary serves the platform ops, and which ones it registers (2026-09-18)

Checklist 17a named this as owed by R1: the binary booted with
`Deployment::boot`, so the access engine granted `std.space.*` and the
registry held none of it. `runtime::assemble` now boots with
`boot_with_platform_ops` (or `empty_with_platform_ops` on a database with
no program) and mounts `PlatformExecutor` over `MeteredExecutor`. Four
decisions came with it.

**The audit surface reads the live engine.** The binary rebuilds its door
on a program swap, and the rebuilt `IdentityAccess` carries a new
`AccessEngine`, because the engine pins a manifest per op. `PlatformOps`
held an `Arc<AccessEngine>` taken once, so after the first swap
`callersOf` would have answered from the previous version's pins. Rebuilding
`PlatformOps` on the swap was not available: `PlatformExecutor` holds it
behind an immutable `Arc` inside `ApiState`. So `PlatformOps` gained an
optional engine source, and the binary's reads `SwappableAccess::current`
on every call. `callers_of` takes one snapshot for both its gate and its
answer, so a swap between the two cannot have them read different engines.
Assemblies that never swap keep the fixed `Arc` and change nothing.

**Registered only when wired.** `PlatformOps`'s docs require that an op
with no state behind it is absent from the registry rather than present
and failing. `callersOf` is always registered (engine, a `PgGrantRegistry`
over the pool, and the door's own delegation registry). The exit ops are
registered when `ORDO_EXIT_BUNDLE_DIR` is set; the attestation ops when the
checkpoint signer is, the attestation worker's own condition.
`ORDO_EXIT_BUNDLE_DIR` refuses the feature when unset, like the checkpoint
key: an operator who has not chosen where a whole-space bundle may land
has not asked for exports. A value that is relative or cannot be created
refuses the boot, because an operator who set it meant the feature on.

**`std.space.attachBucket` is registered when this deployment will honour
an attachment** (18p U6, 2026-09-24): `ORDO_BLOB_ROUTING=attached`, an
`ORDO_BLOB_STORE`, and a credential provider, all three. On `single` the
blob tier resolves every space to one `SingleStore`, which never reads
attachments, so registering the op there would accept an owner's custody
decision and ignore it — worse than a 404, because the op prints a consent
sentence saying the objects are in the tenant's bucket. One condition,
`runtime::buckets_wired`, read by the registration, the census and the
wiring, so the three cannot disagree; the census names which of the three
parts is missing.

**The reservation covers every platform name, and a swap re-checks it.**
`vet_platform` used to check the program against the ops this deployment
registers. `PlatformExecutor` routes on `is_platform_op`, which is every
name the build defines, so a program's own op under an unregistered
platform name would have been taken from it at call time; and a program
that loads must not start refusing the day an operator sets a variable.
The check now covers every reserved name. `Deployment::swap` had no check
at all, so a deploy could install what the boot would refuse; it now
returns `Swapped::Refused` and keeps the old version. Today this is
defence in depth: `space` is a keyword, so no program can spell a
`std.space.*` name, and `tests/runtime.rs` pins that so the binary-level
test gets written the day it changes.

---

## Two confirmations from the owner's answers of 2026-09-19

**CI image cadence (R4, 17c): option B.** Every push to `main` builds, smoke
tests and pushes the `amd64` image of each deployable; both `amd64` and
`arm64` are built only on a `v*` release tag. The repository is private, so
Actions minutes are a budget: emulated `arm64` builds cost roughly 20 to 40
minutes a push and prove nothing a release does not, while the `amd64` smoke
proves on every push that the image boots. Switching to every-push
multi-arch later is one line in `images.yml`.

**The security drain stays outside the supervisor (17d).** Confirmed as
built on 2026-09-18: it appears in the supervisor's census with
`supervised: false` and its own counters, and is never quarantined. The
supervisor's quarantine exists for loops where stopping is the safe
default. For security events a stopped drain turns a bounded, reported loss
into an unbounded one, so retrying forever is the safer failure.

---

## Restore and metering: what the re-read of 2026-09-19 settled

`plan-production-ops.md` Tracks R and M were written on 2026-09-08 against
`89b1d1d`. Re-reading them against `a2cdfe6` found four assumptions overtaken
by the runtime, the export digests, the opslog trailer and space routing,
and two mechanisms that would have counted the wrong thing. Each ruling
below says what the code does and why the plan now follows it. None of
these is the owner's call; the two that are appear as questions at the end.

### The restore marker is trailer `0x03`, and it carries a payload

The plan assigned the marker tag `0x02`. That tag went to
`CommandGroup::restored_unverified` on 2026-09-10 (`plan-opslog.md` D7):
the bit a revert's own group carries when it restored a blob handle with
no recorded digest. The two share a word and nothing else. The decoder now
reads trailer tags in ascending order and refuses a repeat, an unknown tag
or two known tags out of order, which is what keeps the encoding a
bijection. A third tag therefore takes the next value, `0x03`, and because
it is the highest tag the same rule refuses anything after its payload
without a new special case.

It is the first trailer with a payload, and the payload carries the
marker's `space`. `CommandGroup::validate` reads a group's space from its
first command; a marker has none, so the space rides the marker and is
hashed with it. `EmptyGroup` is relaxed only when the marker is present,
and a marker group with commands is refused (`MarkerWithCommands`): the
marker says history moved, and a group that also wrote rows would be two
facts in one entry. The golden-first sequence D1a and D7 used applies:
`PRE_THIRD_FLAG_GOLDEN` is committed before the encoder changes.

### `ordo restore --record` attributes and does not authenticate

The plan gave the marker a `step_up: CorrelationId` joinable to
`StepUpSatisfied`. A step-up is a door ceremony, and the door is closed
exactly when the marker is needed: the boot assertion refuses to serve the
space until the marker exists. So the marker is recorded by the operator
CLI, which holds database credentials, and the posture is the one
`ordo keys init --as` already states in its own comment: *"`--as` is
asserted, never authenticated: this tool holds database credentials, so it
attributes rather than proves."* The operator is a string field, and the
join to the security stream is the group's own correlation, adopted by the
`CustodyAction { action: "space_restored" }` entry the same transaction
writes. That entry is what `erase::RETAINED` keeps, so the record of a
restore outlives a later prune of the marker.

### A marker is found by the actor column, not by an index table

`$commandlog` stores the actor rendering as a plaintext column beside the
hashed body. `WHERE "actor" = 'system:restore'` finds a marker, and
decoding the row confirms it; the actor is hashed through the body, so the
column and the body cannot disagree on an untampered chain, and a row whose
column claims a marker its body lacks is a refusal that names the index. A
`$restores` table would be a stored copy of a fact the chain already holds,
and one an operator could edit, which is the objection the plan already
raised against a stored `orphaned_at` column.

### The boot assertion has two sites, and only a passing verdict is cached

*The two sites became one on 2026-09-21; see 17aa below.*

The plan put the assertion on `PlatformState::new`, "per space". The binary
binds `ORDO_SPACE` in `assemble` and every other space lazily in
`Spaces::resolve`, on a request. So `assemble` refused the boot for
`ORDO_SPACE` (`BootRefusal::Rewound`) and `Spaces::resolve` refuses the
space (`SpaceRefusal::Rewound`) on its first use in a process, on that
space's own pool, since its chain is in its own database. Under 17aa the
boot no longer refuses: it calls the same per-space function and
quarantines what it finds, so there is one site and one wording.
`Spaces` caches
the spaces that passed; a refusal is never cached, so the request after
`ordo restore --record` re-checks and serves without a restart. The rule
that fires is arithmetic over two rows: the chain's head is below the
newest checkpoint's `upto` and no marker's `restored_from` sits below that
checkpoint. A space with no checkpoint is never refused, because there is
no proof the chain was ever longer.

### The key-material assertion, in the form the binary can check

`implementation.md` asks the binary to refuse "a local master key
configured inside the backed-up database". A process cannot know where an
environment string came from. What it can check is the one table custody
owns: no `$keycustody` row holds the master's raw bytes or the key
`derive_from_secret` produces from it. `PgKeyStore::open` already loads
every row into its cache, so the check is a scan of memory in
`build_custody` and in `ordo keys init`. Everything outside that table is
the operator's rule, stated in the howto: the master and the sealed-master
passphrase live outside every backed-up database and outside the Postgres
data directory.

### The bundle digest is `sha256(manifest.json)`, reported by the export

The member digests the plan asked for were built with the export's blob
members; the manifest names every member's SHA-256 and does not digest
itself. The marker names a bundle by one string, and that string is the
SHA-256 of the manifest's exact bytes, which fixes every other member
through the map inside it. No sidecar file: the export's own doc rules that
something outside attests the manifest, and the two things outside that
need the digest are the export's response (`manifest_sha256` on
`std.space.export`'s result row and on `ExportReport`) and the operator's
`--digest` flag.

### The drill rewinds by template copy

A cargo test cannot run `pg_basebackup` and a WAL restore against the
cluster the suites share. `CREATE DATABASE "x_backup" TEMPLATE "x"` is a
real point-in-time copy of the whole database, which is the property the
drill's five assertions need, and restoring is the same statement the
other way. PITR proper is Postgres's instrument, configured by the operator
and described in the howto; the drill proves the platform's half runs end
to end: marker, verifier, boot assertion, bundle.

### Writes are read from the chain at rollup, not observed at the choke point

The plan added `AppendObserver::appended(space, commands)` inside
`append_group`. That call runs before the transaction commits, so it counts
a group whose transaction later rolls back (a serialization conflict after
the append is the ordinary case), and it lands in a per-process counter a
crash discards. `$commandlog` between two positions is the exact,
committed, replica-independent record of every write, and its body carries
the command count at a fixed offset, so the leader reads writes and rows
written from the chain once per period and carries `chain_from` and
`chain_to` on the row. The observer seam keeps its two timings and gains
nothing.

### Worker runs are counted at the runner, because `finish_run` deletes the row

The plan counted worker executions from `$triggerruns`, "already
per-space". `finish_run` deletes the row it finishes, so at rollup time a
finished run leaves nothing to count. `TriggersRunner` holds the `Run` and
its space at the moment it finishes or fails one, and counts there, into
the same in-process ledger as requests.

### Two workers and two tables, because in-process counters live in every replica

A schedule-claimed worker is leader-only and an in-process counter lives in
every replica; the leader cannot read a follower's memory. So the ledger
splits along durability. `usage.flush` runs on every replica on a tick, not
leader-only, and writes rows keyed `(space, period_start, replica)` to
`$usage_replica`; a row still `open` after its period ended is a replica
that died before it could close it, recorded rather than smoothed.
`usage.rollup` is the leader-only schedule the R3 slot reserved, and writes
the per-space quantities (writes from the chain, bytes stored sampled per
target database) to `$usage`. Both tables live in the control database,
because they answer for the deployment. Blob bytes are exact:
`$blobref.bytes` is a plaintext column, so the plan's `unbuilt` label for
them is retired.

### Two questions for the owner, with recommendations

- **The usage period.** Recommend one hour for `usage.rollup` and sixty
  seconds for `usage.flush`. An hour is the smallest period a billing
  product plausibly wants and keeps `$usage` at 24 rows per space per day;
  a minute bounds a crashed replica's loss to what its last row already
  discloses.
- **A quota on writes.** `writes` is a leader-rolled quantity and lags a
  rollup period; v1 quotas cover `requests` and `rows_returned`, which the
  door can read from a snapshot. Recommend deferring a writes quota until
  someone needs it, with the lag stated when it lands, rather than
  counting writes a second way to make it instant.

## Building the restore marker and the seam (R2a, R2b, 2026-09-19)

Building R2a and R2b against the plan turned up four places where the
code decided something the plan left loose, and one question the plan
does not answer. The four are recorded here with their reasons; the
question is at the end with a recommendation.

### The goldens are taken over a new correlation, and there are two

`PRE_THIRD_FLAG_GOLDEN` pins `create_group("grp-r2a")`, 167 bytes, hashed
by the encoder that knew tags `0x01` and `0x02` and nothing else, and it
was committed before the encoder changed. The plan said "over the same
fixture group" as `PRE_SECOND_FLAG_GOLDEN`; the same correlation would
have produced the same number as that golden, so the new pin would have
proved nothing the old one does not. A second golden,
`PRE_THIRD_FLAG_BOTH_TAGS_GOLDEN`, pins a group carrying both older tags,
because the claim that has to hold is that every existing chain keeps its
hash, and revert groups and rule-6 groups carry trailers today. Both
tests also decode the old bytes and re-encode them to themselves.

### One existing test moved its "future tag", and still refuses

`an_unknown_trailer_tag_still_refuses` pushed `0x03` after both older tags
and expected `TrailingBytes`, as the example of a tag no decoder knows.
R2a makes `0x03` a known tag followed by a payload, so that exact input
now fails as a payload cut short (`Truncated`). The test was amended
rather than deleted: its unknown-tag claim moved to `0x04`, the next
future tag, with the same assertion, and it also asserts that a bare
`0x03` still refuses. The property it guards, that an older reader
refuses a trailer it does not know, is unchanged.

### A marker must be attributed to `system:restore`

The verifier finds markers by the plaintext `actor` column. The plan
refuses a row whose column claims a marker its body lacks, which covers
one direction. The other direction, a marker body under another actor,
would be a seam the anchor rule never sees: the walk would pass it, but
the anchor would already have been chosen as if it did not exist.
`CommandGroup::validate` therefore refuses a marker group whose actor is
anything but `Actor::System("restore")` (`GroupShapeError::MarkerActor`),
so every marker the append gate admits is one the column finds. It is a
shape rule, like `MarkerWithCommands`, and it changes no encoding.

### The seam bound is the lowest over every marker

The plan's anchor rule reads "when a marker exists, the anchor is the
newest verifiable checkpoint with `upto <= marker.restored_from.0 + 1`".
With one marker that is unambiguous. With two it has to be the lowest
bound over all of them, which is also what the plan's own orphan rule
("greater than `restored_from.0 + 1` of any marker it passed") implies.
The case that forces it: a first restore resumes at 3 and orphans a
checkpoint at 6; the chain grows, and a second restore resumes at 6. The
second marker's bound is 7, so a rule reading only the newest marker
anchors on the checkpoint at 6, whose head is from history the first
restore discarded, and the walk reports `BrokenLink` at 6 on a chain
with two lawful restores. `two_restores_are_two_discontinuities_in_order`
builds exactly that case.

### `RestoreSource::Pitr { at }` is in seconds

The plan's field table says the PITR target is in milliseconds, and its
R2a signature says `at: Timestamp`. `Timestamp` is seconds since the
epoch everywhere in the code, including the chain's own command
timestamps and checkpoint times, so the marker uses seconds and the
chain keeps one time unit. R2c's `--at` flag takes seconds accordingly.

### Open: what a checkpoint signed after a marker is

The plan's rule classes every checkpoint above the seam as orphaned and
never anchors on one. It does not distinguish a checkpoint signed over
discarded history from one the cadence worker signs over the new history
after the marker, since both have `upto` above the seam. Once R2c writes
real markers, that has five consequences: a post-restore checkpoint is
listed as orphaned; verification replays from below the seam forever; a
prune cut above the seam leaves the anchor below the first stored entry,
so `verify` fails; `continuity` reads a later unannounced rewind below a
post-restore checkpoint as `Explained`; and `sign_checkpoint` answers
`AlreadyAt` when the new head reaches an orphan's `upto`, where the
insert's `ON CONFLICT ("space", "upto") DO NOTHING` would drop it anyway.

Recommendation, needing no change to the frozen encoding: classify a
checkpoint above the seam by the stored chain. One whose head equals the
stored hash at `upto - 1` (or the stored `prev` at `upto`, after a prune)
was signed over current history, so it is an anchor candidate and not an
orphan; one the stored chain contradicts is an orphan. For a checkpoint
above the head, where there is no stored hash to compare, have R2c
default `pre_restore_head` to the newest stored checkpoint, so that such
a checkpoint above `pre_restore_head` is a fresh rewind. This should be
settled before R2c ships, because the marker payload freezes the first
time a real marker is written.

A related point for R2d and R3: `$checkpoints` lives in the space's own
database (`plan-space-routing.md`), so a whole-database PITR rewinds the
checkpoints with the chain. After such a restore the stored checkpoints
agree with the stored chain, and `continuity` reads `Intact`. The
rewinds R2b detects are the ones a checkpoint row survived. R3's drill,
which restores a template copy of the whole database, will need a
checkpoint that lives outside the copy to see `Rewound`.

---

## Four rulings from the owner's answers of 2026-09-19 (restore and metering)

**Usage is flushed every 60 seconds and rolled up hourly, by default, and
both are configuration** (`usage-period`, 17n). The owner: "Take
recommendation as default - making sure it is configurable - document the
feature / configuration and its implication." An hour is the billing grain
and keeps `$usage` small; a minute bounds what a replica crash can lose to
one flush interval of counts. The two intervals are settings with these
defaults, and `howto-operating.md` states the trade: a shorter flush costs a
write per replica per interval and loses less on a crash; a shorter rollup
makes quotas and dashboards fresher at the cost of more `$usage` rows.

**v1 quotas gate requests and rows returned only** (`write-quota`, 17n M2).
Both are counted at the door as they happen, so the quota is exact. Writes
are counted at rollup, so a write quota would bite up to a period late,
which is a quota in name only. Write quotas are 18i on the post-deployment
roadmap.

**The boot check reads checkpoint heads that a space restore cannot roll
back** (`restore-witness`, 17j R2d, option C). Signed checkpoints live in
`$checkpoints` in the space's own database, so a point-in-time restore of
that database rolls them back with the chain, and a boot check that read
them would report a silent restore as intact. Every signed checkpoint head
is therefore also recorded in the control-plane database, and the boot check
compares against that record; a space restore does not touch it. The
offline verifier additionally compares against the heads the attestation
receivers hold, which catches the rarer case of the control plane being
restored too. The plan is updated before R2d is built.

**A checkpoint counts if the stored chain agrees with it**
(`orphan-checkpoints`, 17j R2b). A checkpoint whose signed head equals the
stored hash at its position is current history and may anchor
verification; one that does not is orphaned by a restore. The plan's first
rule (every checkpoint signed after a marker is orphaned) broke
verification across a prune, let a later unannounced rewind read as
explained, and confused checkpoint signing at an orphan's position. The
restore command records the newest stored checkpoint as the pre-restore
head by default. No format change.

## Key rotation and the re-seal pass: rulings from building K1–K3 (2026-09-19)

The owner ruled on 2026-09-07: envelope encryption, with a built-in
ability to force re-encrypt after an exposure. Building it against the
tree of 2026-09-19 forced five rulings the 2026-09-08 text did not
contain. Each is stated with its mechanism, because the outcome alone
would read as a shortcut.

### 1 · An object cannot keep its handle across a re-seal, so the object population clears by the blobgc floor

The K text said a re-sealed object "mints a new handle, writes the new
object, updates the `$blobref` row and the entity column in one
transaction". Updating the row's `ref` breaks every retained command
image: a revert resolves a blob column through `$blobref` by the handle
the image carries, and a handle with no row is refused as corruption
(`undo.rs::blob_digests`). Keeping the handle is not available either:
the handle is the object's key, and `BlobStore::put` refuses an existing
key on purpose.

**Ruled:** an object moves to a new handle, a **new** `$blobref` row is
inserted, the entity column is moved by compare-and-set, and the old row
and object are left exactly where they are. `blobgc` collects the old
pair once no live row and no retained image names it, which is the
undo-horizon floor. So for external objects the gate is discharged by the
horizon clock, not by fact, and the pass says so: a sweep that leaves only
old pairs behind reports `blocked` rather than re-opening every tick, the
gate keeps refusing by name with the count, and `ordo keys reseal`
re-opens the row for another sweep after `blobgc` has run. Payloads,
columns and `$blobref` envelopes still clear by fact, in place.

### 2 · `$blobref.epoch` is the object's epoch; the envelopes carry their own

`mime`, `size` and `integrity` are envelopes with their own leading epoch
byte, opened by that byte. The row's `epoch` column is the epoch of the
**object** in the store, which is the one thing a worker cannot read
without fetching the object. **Ruled:** the envelope population's
predicate is the envelopes' own bytes, re-sealed in place; the row's
`epoch` column belongs to the object population and moves only when the
object does. An epoch is clear only when both are zero.

### 3 · Completion is a re-count, never a cursor

A row sealed under the old epoch inside a transaction that committed
after the chunk passed its keyset position is rare and real. **Ruled:**
when every population's cursor is exhausted the pass counts again, and a
non-zero population re-opens its cursor for a second sweep; `Completed`
is returned once, at zero. The done-when's `count == 0` is what the
state machine checks, not what a test hopes. The one exception is ruling
1's: objects are never re-opened by the pass itself.

### 4 · The gate is the count, and the horizon date is the re-seal date

With every population re-sealed, §7's rule ("no command image within the
undo horizon is tagged with the epoch") is discharged by the count: an
image is tagged with an epoch through the payload row its handle names,
and a zero count means no such row exists, inside the horizon or out of
it. **Ruled:** `destroy_gate` consults the four counts and nothing else,
and the consequence the confirmation prints is the one thing the pass
cannot reach: *backups and archived log bundles taken before <the date
the pass finished> hold this epoch's ciphertext and will not open for
sealed fields in this space once it is destroyed.* The `--confirm` token
is `<space>:<epoch>`, so a command copied from another space refuses.

### 5 · Two things the plan named do not exist, and the build says so rather than faking them

There is no suspension op in the host (`AccessEngine::suspend` has no
production caller), so "a membership suspension enqueues a rotation" has
no caller to wire. **Ruled:** K1 ships `reseal::enqueue_rotation`, the
function that op will call, and `ordo keys rotate` is its production
caller now. There is no `std.E.history` op, so K2's test asserts through
`sealed::resolve` returning `Resolved::Destroyed` and the revert's typed
refusal, the two surfaces a history view would render from.

One mechanical correction rides with these: a sealed column can hold zero
bytes, and `get_byte` on an empty `bytea` is an error rather than false,
so every "under epoch `n`" predicate is `length(c) > 0 AND get_byte(c, 0)
= n`.

## `ordo dev` (17o, `plan-dev-loop.md`) — decisions from building D1–D3

**`ordo dev`'s custody is `CustodyChoice::Test` (`TestProvider`), unconditionally.**
Production gates this provider behind `ORDO_DEV_UNSAFE_TEST_KEYS` precisely
because it seals under a key published in this repository. `ordo dev` is
the caller that gate exists to let through: §4.3 already rules that dev's
whole database is regenerated seed data, never production's, so there is
no material this provider's weakness could expose that the loop does not
already treat as disposable. Composed directly on the `RuntimeConfig`
struct (never through the environment gate), since `ordo dev` is not a
server reading `ORDO_KEY_PROVIDER`.

**A save's version label is a content digest, not a constant.** The first
draft of `ordo_host::deploy::verify_embedded` hardcoded `"dev"` as every
candidate's version. `Deployment::swap` compares versions to decide
`Installed` vs `AlreadyServing`; a constant label made every save after the
first report `AlreadyServing` and silently keep serving the old program.
Fixed by deriving the label from a digest of the compiled source
(`dev_version` in `ordo-cli`'s `dev.rs`) and threading it through
`verify_embedded` as a parameter rather than hard-coding it — `assemble_embedded`'s
own first boot uses the constant `"dev-boot"` instead, chosen because a hex
digest can never collide with it (`o`/`t` are not hex digits).

**A swap must rebuild the access engine, or the new op is dispatchable and
unreachable.** Found by writing D3's own test: a save that added an op
compiled, gated and swapped cleanly, and the door then answered
`access_denied` for every token calling it — including one that could call
every pre-existing op. `Deployment::swap` moves one pointer (the dispatch
registry); `state.access` (`SwappableAccess`, wrapping `ordo_identity::AccessEngine`)
is a **separate** structure built once at boot from the fixture that was
live *then*, and nothing before this fix rebuilt it on `ordo dev`'s own
swap path. `spawn_deploy_listener` already does this correctly for a
second process's `ordo apply` (`door_for` + `SwappableAccess::install`,
inline in its `Installed` arm) — that code was the proof the property is
real and the model for the fix. `ordo_host::runtime::refresh_access` is now
a named, public function both callers use: `ordo dev`'s save loop calls it
immediately after every `Swapped::Installed`, in the same journal phase as
the swap, because "swap" only means what the plan needs it to mean
(the op is actually callable) when both run together.

**The concurrent-describe optimisation (§6 D3's first-named lever) was
attempted and reverted.** `shape::verify_on`'s per-op `DESCRIBE` loop was
split into a planning pass and a second pass running every describe
concurrently (`futures::stream::buffer_unordered`) over a pool sized to
match. Against the market fixture it reproducibly corrupted results —
mismatches reporting one op's expected columns against a database report
that was the union of several unrelated ops' columns, meaning describes on
different concurrent connections were resolving against the wrong
statement. The module's own doc already names the likely mechanism:
`build_shadow`/`observed_shape` both `SET search_path`, which is
*connection* state, and the existing code's comment that verification
"takes a short-lived pool of its own" was written for the *serial* case.
Making this safe under concurrency needs a real look at `ordo_exec::Pool`'s
connection-checkout guarantees, which is `ordo-exec`'s code, not this
plan's, and reverting was the only responsible move inside this task's
time. **It is not needed to hit the bar**: measured serial, the gate over
the market fixture (378 lines, 165 ops, 76 reads) is ~330ms, and swap +
access-engine rebuild is ~170ms more — ~500ms total, comfortably under
the 1s bar with real but not enormous margin. If CI hardware proves
slower, this optimisation is still there to pick back up, with the
connection-safety question named rather than papered over.

**Corrected 2026-09-19, the same day: the pool was not at fault.** The
probe was. `observed_shape` looked its session-temporary `$shape` view up
by `pg_class.relname` alone, which matches every open session's `$shape`,
so concurrent probes read the union of each other's columns. It also left
`search_path` pointing at the shadow schema on the connection it used.
Both are fixed in `ordo_exec::shape` (a `pg_temp` lookup, and `SET LOCAL`
scoped to the probe's own implicit transaction), and
`concurrent_probes_each_see_their_own_statement` reproduced the union
before the fix. The concurrent-describe optimisation is therefore safe to
pick back up if the dev loop's budget ever needs it.

**`ordo migrate --apply` (schema-evolution increment 2) still does not
exist**, so D2's interim rule (§4.4: every non-empty delta resets rather
than migrates, additive or not) is not a simplification `ordo dev` chose —
it is the only correct behaviour available, and stays that way until that
increment lands.

---

## Three rulings from the owner's answers of 2026-09-20

**`$checkpoints` admits two checkpoints at one position** (`checkpoint-key`,
17j R2b′). After a restore the chain grows again from an earlier point, so a
current checkpoint can be signed at a position an orphaned one already
holds. The key becomes (space, position, head) through an additive `ALTER`.
Without it the first restore leaves a space with no current anchor until it
writes past the orphan.

**Rotation is an operator act, and exposure is an event** (`rotation-policy`,
11-11 / 17i). Nothing rotates on a clock: a schedule would spend a full
re-seal pass per tenant on a date nobody chose, while the thing that makes
rotation urgent is an event. Operators are told to rotate on any event that
may have put a data key in front of a party who should no longer hold it: a
departing administrator, a leaked backup, a compromised host. `reseal`
repairs an interrupted pass; `rotate` answers exposure. The worker exists,
so a schedule can be added if a compliance regime ever demands one.

**Old blob objects clear on the sweep, and the docs say so**
(`rotation-objects`, 11-11 / 17i). A re-sealed blob is a new object under a
new handle, because the handle is the object's key in the store; the old
object is left for the garbage sweep's horizon. The owner accepted this with
disclosure. The consequence to state plainly, because it is stronger than
"some bytes linger": `destroy` refuses while an epoch still has objects, so
**the retired DEK stays alive until `blobgc` collects the old objects**. A
rotation therefore does not end the old key's life on its own; the sweep
does. `howto-custody.md` says so where an operator will meet it. Revisit if
an object-key indirection on `$blobref` is ever wanted, which would let the
bytes be replaced in place and the key die with the pass.

---

## What a backfill's revert means: the diff, not a clone (2026-09-20)

The owner, asked again about `plan-schema-evolution.md` §9's open question:
"some operations cannot be fully reverted in some way, the transaction
should include summaries. Multi million row clones are expensive, only
diffs should be revertible."

This confirms the 2026-09-08 ruling above (summarised groups by default,
full per-row images as a per-backfill escape an app creator declares) and
settles the part that was still loose, which is what `revert` promises
over a summarised backfill:

- **The migration's transaction carries a summary**, not a copy of the
  data: what changed, how many rows, the declared mapping, and the content
  hash that makes "data before = data now, in a different structure"
  checkable.
- **Revert is defined over the diff the migration made**, which is the
  schema change and the transformation that produced it. A backfill's
  revert undoes the structural change by the inverse the migration
  declared; it does not reconstruct each row's prior value from the chain,
  because the chain deliberately does not hold them.
- **Copying every row to make a backfill revertible is refused as the
  default.** A per-row image set for a multi-million row entity is a log
  larger than the data it describes, paid on every deploy, to buy a
  recovery that a point-in-time restore already provides.
- **An application that needs per-row revert declares it** on that
  backfill, and pays the log cost knowingly. Unchanged from 2026-09-08.
- **What this costs, stated plainly so nobody discovers it during an
  incident:** for a summarised backfill, the prior values are not
  recoverable from the command log. The recovery path is the deployment's
  backup or point-in-time restore, and the restore marker work (17j) is
  what makes such a restore visible rather than silent. The migration's
  own report must say which class it is before it runs.

This closes §9. Increment 3 (the barrier and the plaintext backfill) builds
against it.

---

## The apply belongs to the service (2026-09-20)

The owner: "We need to make sure that the apply belongs to the service as
opposed to a run from the client to the database - we can build the tool to
confirm logic and test - but it will belong to the server."

So the apply path is a library the server owns: classify, order, execute,
record in `$migrations`, emit the security event. `ordo migrate --apply`
and `ordo apply`'s first-apply path are thin callers of it, kept because
they are how the logic is confirmed and tested. Nothing in the library may
assume an operator's shell: no environment read inside it, no prompts, no
`psql`. It takes a pool and a program and returns a structured outcome.

The consequence is that 18l, applying a schema over the wire as an
owner-gated platform op, is a wrapper rather than a second implementation,
and a hosted control plane and a self-hoster's CLI cannot drift into two
different migrators.


---

## Schema apply (13b, `plan-schema-evolution.md` increment 2): rulings from building it (2026-09-20)

### 1 · The apply belongs to the service

Owner constraint, stated during the build: *"We need to make sure that the
apply belongs to the service as opposed to a run from the client to the
database - we can build the tool to confirm logic and test - but it will
belong to the server."* So the plan, the probes, the triage, the execution
and the record live in `ordo_host::schema_apply`, over a `Pool` and a
compiled `Program`, returning structured outcomes. The module reads no
environment, prints nothing and shells out to nothing. `ordo migrate
--apply` and `ordo apply` parse arguments, render and set an exit code.
Filed as 18l: the owner-gated platform op that puts a door in front of
the same functions for a hosted control plane.

### 2 · A first apply creates; a registry over no tables is not empty

`ordo apply` on a database holding none of the program's tables and no
serving version creates the whole schema through
`ordo_exec::spacedb::apply_schema`, the path `provision::prepare_target`
already ran for a space's own database, and records it as a `create`
migration. A database holding none of the tables while `$activations`
names a serving version is refused (`serving_schema_drifted`): creating
over it would leave a registry describing a database that never existed.
The operator restores the database or clears the registry, and the
refusal says which.

### 3 · `ordo apply` refuses to activate what the gate would refuse

The load gate is one-way: every object the artifact asserts must exist
live; extra live objects are tolerated. `Change::gate_blocking` encodes
that direction. When the diff holds a blocking change, `ordo apply`
stores the artifact (the staged case) and refuses activation by name,
pointing at `ordo migrate --apply`. When it holds only drops, the schema
is a match and activation proceeds, because drops sequence artifact
first. Refusing at the apply rather than at the node is the difference
between an operator reading the reason and a fleet logging it.

### 4 · A constraint that constrains no existing data applies with class A

Increment 2 is class A only. One extension is taken: a class-C constraint
is applied when the table is empty, or when every column it names arrives
in the same apply and its probe is clean by construction. These are the
column's own declaration, and `CREATE TABLE` would have carried them; a
new ref field that arrived without its foreign key would be a column the
artifact asserts and the gate refuses. A constraint over existing rows is
refused (`constraint_over_existing_rows`) and the refusal carries the
`NOT VALID` then `VALIDATE CONSTRAINT` statement for an operator who has
read the probe. A `unique` has no online form and is carried plain.

### 5 · Nothing is sent when anything is refused

An apply that landed half its changes would leave a database that
matches no version. The triage returns every step or every refusal, and
the executor is not called until the refusals are empty.

### 6 · Drops wait, including the data-free `A7`, and the contract needs the previous version

Dropping an index the serving artifact still declares makes it drift on
its next load, so `A7` is refused as `artifact_first` with the statement
carried. Found while building: once the artifact that stops declaring the
index is activated, the diff has no prior that declares it and the live
object is invisible, so it is never proposed for dropping. Increment 5's
contract phase needs the previously active version as its prior, which
`$activations` holds. Named now so it is a decision rather than a
discovery.

### 7 · `check` joins the taxonomy as `C9` and `C10`

16a recorded that the diff had no code for a row invariant, and that 13b
needed one before a check could be added to a live deployment. `C9`
adds one, probed by `migrate::check_violations`, which negates the
rendered expression; `check.rs` renders it total, so a null counts as
the constraint would count it. `C10` drops one, the same one-way door
as `C6`. A changed condition reaches the diff as both, because the
constraint name carries the condensed condition.

### 8 · `ordo dev` applies class A through the production path and resets for the rest

§4.3's rule made literal: `dev_db::classify` runs the production
`assess` and `triage`; when nothing is refused the loop executes the
same steps `ordo migrate --apply` sends, skipping the registry and the
`$migrations` row (the bookkeeping §4.3 keeps as dev's one relaxation).
A refused delta resets and names what forced it. An empty database
resets, because the reset is the path that seeds.

### 9 · A `sealed indexed` field onto a populated table is refused by name

`<f>$bidx` is `bytea NOT NULL` with no default (a null HMACs the null
sentinel), so it cannot land on rows without a digest for each under
every space's key. Refused as `blind_index_backfill_required`; increment
4 builds the backfill. On an empty table it applies.

## Four rulings from closing 20b's credential and session gaps (2026-09-20)

**A session is a claim in the token, not a row in a table**
(`session-shape`, 20b). Everything a live session asserts (who, since
when, under which client, with what presence) rides in the token under
the MAC, so the only durable state is the deny list of sessions somebody
ended. That list is bounded by logouts rather than by logins and prunes
itself at each session's ceiling, which is what lets the door answer from
memory: one lock read and one set lookup per request, no I/O. A session
table would have made the door's question a database round trip on every
call the deployment serves, which is the cost the whole door is built to
avoid.

The consequence to state plainly, because it is a capability people will
ask for: **nothing can list a principal's live sessions**, so "sign out
everywhere" and an operator session pane are not merely unbuilt but
unbuildable in this shape. Both want a per-principal epoch, which is the
same deny-list mechanism keyed on the principal and is a much smaller
increment than a session table. Revisit if a compliance regime demands an
inventory of live sessions.

**Refresh does not rotate the session id** (`refresh-rotation`, 20b).
Rotating on every refresh is the textbook hardening and it buys detection
of a stolen token: the thief and the owner both refresh, one presents a
superseded id, the deployment kills the session. It costs a durable write
per refresh and a record of which id supersedes which, which is the
session table the ruling above rejects, and it turns any lost race, any
retry and any second browser tab into a logout for somebody who did
nothing wrong. Not rotating keeps the deny list bounded by logouts. **The
trade is the owner's and this is the default rather than the ruling.**

**A refusal must cost the same as a refusal** (`auth-timing`, 20b). Every
authentication failure was already one answer, and that was not enough: an
unknown identifier returned in microseconds while a real one paid a full
argon2id verification first, which is tens of milliseconds and is
measurable over the network. The wording being identical bought nothing
against a stopwatch. Both paths now run exactly one verification, the
unknown one against a decoy hash minted per process. The general rule this
settles: **wherever a refusal is deliberately indistinguishable, the work
it does is part of what must be indistinguishable.**

**A store that cannot answer is not a wrong credential**
(`store-unavailable`, 20b). An unreachable credential store is its own
answer at the HTTP surface (`credential_store_unavailable`), and it
discloses nothing, because whether the database is reachable does not
depend on which identifier was offered. Conflating it with
`authentication_failed` would tell every user their password was wrong
during a database incident, and the first thing a user does about that is
change a password they remembered correctly. The same reasoning makes a
failed logout a `503` rather than the usual `204`: a logout that was not
recorded must not be reported as done, because the client discards the
token and believes it is safe while the door goes on honouring it.

## One ruling from closing 20b's directory gap (2026-09-20)

**Principal-id collision is a database constraint, not a property of the
id source** (`principal-id-collision`, 20b). The old code minted a
principal id from a counter, which cannot collide but also cannot
survive a restart. The durable replacement draws 128 bits from the OS
CSPRNG instead — the same shape `SessionId::mint` already uses — and the
question that raises is what happens on the astronomically unlikely
collision. The answer is not "trust the odds": `$principal` has `id` as
its primary key, `create_principal` commits a draw with `INSERT ... ON
CONFLICT ("id") DO NOTHING`, and a draw that did not insert a row is
known to have collided and is retried with a fresh one, bounded so a
broken id source refuses loudly rather than spinning. This is why the
directory is two tables (`$principal` for identity, `$authenticator` for
the authenticator-to-principal link) rather than one denormalized table
carrying a principal's kind and display on every authenticator row: a
denormalized table has no row that exists independently of a link, so
nothing would ever ask the database whether a freshly drawn id already
names somebody, and two different humans could silently end up sharing
one principal id. The general rule this settles: **wherever an id must
never collide, minting it has to be a write the store can refuse, not a
calculation the caller trusts.**

## Two rulings from the bulk import build's own defects (2026-09-20)

Both found in code nothing had executed before it, per checklist **11-10a**
(`plan-import.md` §9). Neither is import's to keep quiet about — each is a
gap in the compiler or the derivation, argued here so the next caller of
either surface finds the reasoning next to the fix rather than re-deriving
it.

**A negative numeric literal binds as the value it names, at every literal
site, not just the write path's** (`negative-call-arg`, 11-10a). The
grammar reads a leading `-` as `unary := '-' unary` uniformly
(`ordo-syntax/src/parser.rs`), so `-2500` is `Expr::Neg { IntLit(2500) }`
regardless of where it appears. That is right for a general expression —
`price - $delta` stays a subtraction — and wrong for a literal parameter
value, because a **parameter's value is a constant looked up by shape**
(`lit_ty`), and `Neg` was not one of the shapes it recognized. So `$total:
-2500` refused, unconditionally, as *"bound to something that is not a
value"* — reachable from a case's own call syntax, from a routine's
parameters, from a harness's top-level `params:` line, and from
`ordo_host::FixtureExecutor::call_source`, which renders a caller's bound
negative amount into exactly that source shape before re-parsing it.

The fix is one function, `ordo_sema::fold_neg_lit`, called wherever a
literal binds a parameter value (`const_expr`'s fallback — the read path's
op-call arguments — plus `bind_routine`, `bind_as`, `bind_with_params`, and
`write_value_expr`'s own environment loop). It folds `Neg` over an `int` or
`decimal` literal into the literal it spells and nothing else: `-$x` and
`-(a + b)` still bind as `Neg` wherever a general expression is legal
(`Binder::expr` already handled that arm correctly), so the ruling widens
what a **constant** can be spelled as, never what an expression can
compute. `ordo_conformance::write` had already found and fixed this for
its own call-argument vector (`fold_negation`, write path only); that
function is now a call to the shared one, kept as its own step in
`write.rs` rather than folded away entirely, because that vector is also
read directly by `is_held_value`'s literal-shape match, which has the same
blind spot `lit_ty` did and would otherwise misclassify a negative literal
as a computed right-hand side instead of a held one.

**`rev`'s create-once belief needs a row to be about, so it needs a key**
(`keyless-create-belief`, 11-10a). DL §6 rule 9 already said this —
"`rev` states a belief about the addressed row, and is legal alongside
`id` *or* a unique key" — but `ordo_schema::std_ops` put `rev: null` in
every entity's derived `create` braces regardless, including entities with
no unique key at all. A keyless entity's create braces address no row by
construction (`ordo_plan::writes::set_mode` reads zero covered unique keys
as `SetMode::Create`), so the belief had nothing to be about, and the
write protocol refused it by name on every single call
(`invalid_binding_expr`, `ordo_conformance::write::plan_write_stmt`) — not
a registration-time failure, which is why the interface-shape tests that
already asserted `std.vendors.create` exists never caught it.

The ruling: the belief is conditional on the derivation having a key to
put it against. `std_ops`'s create arm now includes `rev: null` only when
`unique_keys == 1`; a keyless entity's derived `create` carries no `rev`
and is an ordinary insert every call. A keyed entity's `create` is
unchanged — it still refuses a second row under the same key with
`expected_absent`, which is the entire reason `create` and `put` are
different derived ops rather than one op named twice. This does **not**
retire `ordo_exec::import`'s own `import.<entity>.create` derivation:
import omits `rev` for every entity, keyed or not, which now differs from
the fixed `std.<entity>.create` on a keyed entity's within-bundle duplicate
key — converge silently versus refuse. That is a duplicate-key policy
question for import, already tracked separately (`plan-import.md` §7,
"import into a non-empty space" and its sibling, an import bundle with an
internal duplicate), not a consequence this ruling settles by itself.

---

## Three rulings on what a platform must do before anything is deployed to it (2026-09-20)

**Readiness is about accepting deployments, not about having one.** The
owner: *"Being a baas in a way, a 503 is not an appropriate status.
Readiness is not a measure of is a deployment running, it is a measure of
are we ready to apply and run deployments."* The binary's `/health/ready`
was written from the perspective of a server that exists to serve one
program, so it reports 503 until that program is active. A platform exists
to receive programs, so a correctly configured deployment with none is
ready by definition, and a readiness probe pointed at the old meaning keeps
such a pod out of service permanently. Readiness now asks: database
reachable, custody present and openable, required configuration and
credentials present and well formed, required workers running, deploy path
able to accept a program. Whether a program is active is reported in the
body. Filed as 17t.

**Implemented 2026-09-20, two design calls worth recording.** First:
"required configuration and credentials" turned out to need exactly one
live check rather than five. `runtime::assemble` already refuses to boot
without the token key, the page key, the master key and `ORDO_PG_*`, so
none of those can be missing from a *running* process — checking them again
on every probe would be checking a boot-time invariant at request time.
The one requirement that is not a boot-time invariant is a blob store:
`blob_needed`'s rule (2026-09-07, above) is evaluated against whichever
program is *live*, and a **swap** can install a program that declares a
non-inline `blob` field after boot, past the point the original
`ORDO_BLOB_STORE` check ran. So `config` asks that one question, cheaply
(an in-memory schema scan), and names the `entity.field` when it fails.
Second: "deploy path able to accept a program" is read literally as
"can `ordo apply` create tables here", checked with
`has_database_privilege(current_user, current_database(), 'CREATE')` —
one more pooled round trip, skipped when the `database` check already
failed. Both are judgment calls in the absence of a more specific ruling;
`docs/roadmap-checklist.md`'s 17t entry has the full account and the
tests.

**The CLI talks to the deployment.** The owner: *"CLI should interact with
the endpoint, should be adjusted with a flag."* This extends the same day's
ruling that the apply belongs to the service. The operator surface was
written when there was no server, so it reaches into Postgres directly;
with a server, the ordinary path is the door, and database credentials stop
being something every operator needs. The direct path stays for the acts
that must work when the door does not: custody init, chain verification,
revert. Filed as 17u, with 18l as its server half. The default host is
`ordo.inf.smg.icu`, confirmed the same day.

**The system space exists at boot.** The owner: *"System space should exist
at boot because nothing can really exist without it, it is where ordo
configurations live."* This does not contradict `build_custody`'s rule that
provisioning is never a boot side effect. That rule protects against a typo
in a *tenant* name minting a tenant. The system space is not a tenant: its
name is the platform's, not the operator's, and it is where the platform
keeps what it knows about itself. A deployment without one has nowhere to
put its own configuration, which is why it cannot be created later by a
command nobody has run yet. Boot creates it when absent and still refuses
an unknown tenant. Filed as 17v.

---

## Three rulings from building 17v, the system space at boot (2026-09-20)

Each one was forced by the first cluster deployment's single line of
output, `custody: key store: $keycustody does not exist`, and each is
argued here because the next person to meet the surface will otherwise
re-derive it.

**A platform's own tables are the deployment's to create, and creating a
table is not provisioning custody** (`custody-table-at-boot`, 17v). The
custody table was the operator's act on an argument that reads well and
is wrong in one step: a binary that created `$keycustody` on the way up
would be one step from provisioning custody on the way up. The step is
the whole distance. An empty `$keycustody` grants nobody anything;
`LocalKeyProvider::release` refuses a space with no SMK in it whether the
table is there or not, and the boot still refuses an `ORDO_SPACE` naming
a tenant with no keys. What the old rule actually bought was a crash loop
on the one database every deployment starts with, which is the same
defect the artifact registry had until `registry::ensure_tables` moved
into the boot on 2026-09-19. The general form: **a rule that protects a
property should be tested against the property, not against the thing
that is one step away from it.**

**What tells the platform's own space from a tenant is where its name
came from** (`system-space-name`, 17v). The rule "provisioning is never a
boot side effect" existed so a typo in `ORDO_SPACE` could not mint a
tenant with brand-new keys that looks perfectly healthy and holds
nothing. That rule survives 17v intact, because the space the boot
creates is not named by any input: `SYSTEM_SPACE` is a constant compiled
into the binary, read from no variable, no header and no command line, so
no value anybody can mistype selects it. A tenant's id arrives from
outside, every time, and is still refused when it has no custody. This is
what makes "the platform picks the name, not the operator" a mechanism
rather than a convention, and it is why the refusal for a tenant now says
so in its own words: the platform's own home exists and this master key
opens it, so what is missing is a tenant. An operator told "no keys
exist" by a process that just minted keys goes and checks the master key,
which is the wrong thing to check.

**A space the door has to bind cannot carry the platform's reserved
prefix** (`system-space-prefix`, 17v). `ordo_exec::erase` had already
reserved `$` for platform-owned spaces, on the good argument that a
deployment may hold several of them and `$` is the character the platform
reserves everywhere it needs a name a caller cannot produce. The system
space every deployment actually has cannot use it:
`ordo_api::well_formed_space` binds only alphanumerics, `_` and `-`, so a
`$` id cannot be provisioned, cannot arrive in a request header, and
therefore cannot have a founding member row, an access engine built from
that row, or the universal read window the third-home ruling promises. So
the id is `system`, and `is_system_space` answers for the name as well as
the prefix. The reason it is one function and not two: the erase gate is
the only thing standing between the platform's own home and an owner who
confirms a deletion correctly, and a second predicate somewhere else is
how that gate comes to cover one of the two and not the other.

## Seven rulings from building the deploy door (17u and 18l, 2026-09-20)

Both items are the owner's, both from 2026-09-20: *"CLI should interact
with the endpoint, should be adjusted with a flag"*, and *"we can build the
tool to confirm logic and test, but it will belong to the server."* These
are the calls made while building them.

### 1 · Being the owner of a space you were given is not owning the deployment

`std.deployment.apply`'s gate is three checks stacked, and only the first
two are mechanisms the platform already had. `owner_only` grants it to
`Role::Owner` alone; `require_step_up` refuses every capability token. A
**tenant space's owner passes both**, because an access profile is
deployment-wide policy installed in every space — so on a shared deployment
every tenant owner would have been able to replace the program every space
runs.

The third check is the handler's own: the bound space must be the
deployment's own, the one the binary was configured with. Stated as the
rule it is: *applying a program is the deployment's act, not a space's.*
The `std.deployment.*` namespace exists to say the same thing in the op's
name, because every other platform op acts on the space it is bound to and
a `std.space.*` name would have invited exactly the reading this check
refuses.

### 2 · A `$`-prefixed space cannot be bound at the door, so 17v's prefix is not yet a gate

`ordo_exec::erase::is_system_space` is the one answer to "is this space the
platform's own", and asking it in check 1 above is the obvious spelling. It
does not work: `ordo_api::router::well_formed_space`'s alphabet is ASCII
alphanumerics, `_` and `-`, so a `$`-prefixed space id is refused with
`missing_space` (400) **before any op is reached**. An op gated on the
prefix would be an op nobody could call.

So the check compares the configured space's name, which is exact today and
is the narrower statement anyway — one deployment has one home, while the
prefix question would admit any system space on a deployment that grows a
second. Recorded as a constraint on 17v: giving the platform's own space a
`$` name and making it reachable over HTTP are two changes, and the second
is an amendment to the door's space alphabet.

### 3 · An executor may not say `access_denied`, and `not_found` is the better answer anyway

The wrong-space refusal was written as `access_denied` and reached callers
as `internal` — a 500 for a refusal that is nobody's failure — because
`ErrorCode::executor_producible` does not list it. `access_denied` is the
**door's** word, answered before an op is reached, and that list is what
enforces the boundary.

The fix is `not_found`, and the mechanical constraint turned out to be
pointing at the honest answer. The op genuinely does not exist in a tenant
space. It is not a permission the caller might one day be granted, and no
membership change inside a tenant will ever make the call succeed, which is
exactly the difference between *there is nothing here* and *not for you*.

### 4 · The lock guards the decision, not the DDL

Two applies serialize on an advisory lock keyed by an FNV-1a of the string
`"$migrations"` — named by the thing it guards rather than chosen, computed
in Rust because asking Postgres for the table's OID fails on a first apply,
where the table is created by the apply.

It is taken **before the first read of the schema**. Guarding only the
execution would leave the failure intact: two applies that both read an
empty target and both call `create_schema` are two *correct* executions of
a sequence that raced at its first read.

Transaction scope on a dedicated connection, not a session lock and not a
durable row. A session lock left by a cancelled apply is a lock nothing
releases until the process restarts. An "in progress" row in `$migrations`
is durable, which sounds like the virtue and is the defect: a crashed apply
would leave one and wedge every later apply until a human cleared it. The
advisory lock's failure mode is that a crashed apply releases.

The wait is bounded at thirty seconds and then refuses. Unbounded would
hold a request open behind an apply that is itself stuck; immediate would
turn two deploys a second apart into a failed pipeline.

### 5 · `apply_in_progress` is not `serialization_conflict`

`serialization_conflict` is a transaction the engine retries for the
caller. An apply must **not** be retried automatically: the apply holding
the lock may be changing the very schema this one planned against, so the
retry has to re-plan, and only the caller can decide to do that. Its own
class, and the same argument makes `not_activated` distinct from
`apply_refused` — there the artifact *is* stored, so the next step is a
migration rather than a recompile, which is the one difference an operator
acts on.

`apply_refused` is deliberately **one** code for the whole triage taxonomy,
with the class as the first word of the message in
`schema_apply::Refused::class`'s own spelling. The opposite of
`egress_refused`'s reason: not secrecy, but that the taxonomy belongs to
the apply path and increments 3 through 5 extend it, and a wire code per
class would make every new refusal an amendment to a closed inventory two
tests enforce.

### 6 · One renderer, because two format strings drift

`ordo apply` and `ordo apply --host` print the same text, and the
mechanism is that both render the same `apply_op::Report` through
`operator::render_apply`. The sequence one apply *is* moved out of the CLI
command and into `ordo_host::apply_op` to make that possible — which is the
2026-09-20 ruling's own consequence read one step further: if the apply
belongs to the service, so does the order its pieces go in.

The test compares the two reports **as text**, not field by field, and that
is the whole point of it: a field comparison would pass for a build where
the two renderers had drifted, which is the only way this can go wrong.

What stays client-side is what the server does not know: the remedies that
name the operator's own file path and database name are built in the CLI.

### 7 · No HTTP client was added, and a bare `--host` gets https

`ordo_blob::http` is a minimal HTTP/1.1 client over `tokio-rustls`,
hand-rolled for the S3 surface, and the deploy client is its second caller.
So the `ordo` binary and the three Docker images gain no new third-party
crate and no native TLS, which is what `plan-ci-images.md` §1.5 asks for.
`reqwest` would have brought connection pooling, redirect following, cookie
jars, proxy discovery and a middleware stack to send one POST.

`Endpoint::parse` refuses a schemeless endpoint and `--host` defaults one to
`https`, and the two are right for the same reason read from opposite
sides: who typed the string. A blob endpoint is configuration, and the case
that matters is a config that meant `http` for a local gateway, where a
silent upgrade produces a TLS handshake against a plaintext port. A
`--host` is typed at a shell against a bare public default
(`ordo.inf.smg.icu`), and plaintext by default is the one wrong answer for
a request carrying a token that can replace the deployment's program. So
`http` must be spelled — which is exactly what addressing a local `ordo
dev` looks like.

**And the door is opt-in.** With neither `--host` nor `ORDO_HOST` the
command talks to the database as it always did. The default host is what
`--host` means when it is given, not what the command does when nobody
mentioned a host at all.

## Five rulings from closing 17w, the dev-loop token wall (2026-09-20)

Found while writing the quickstart, by trying to call an op straight after
`ordo dev` came up clean and hitting `access_denied` with no documented way
past it. These are the calls made closing it.

### 1 · A seeded principal, not a login route or a new command

Three shapes were on the table: wire `ORDO_AUTH_METHODS` into `ordo dev`,
add an `ordo dev token` command, or seed a principal and print its token at
boot. The third is the only one that answers without a developer first
learning what a token, a principal or membership is — the login route asks
for a password `ordo dev` has nowhere to check, and a second command is one
more thing to know exists before the very first call works. Printing wins
by needing nothing learned.

### 2 · Seeded by `reset`, not by a new hook, and after `seed.sql` rather than instead of it

The insert lives in `dev_db::reset`, the same function that already
creates the schema and runs a developer's `seed.sql` on every full reset —
not a new pass over the database, and not a special case in `dev.rs`'s own
loop. **After** `seed.sql` rather than before: a developer's own rows are
never contingent on this row existing, and a project with no `seed.sql` at
all still gets one caller who can reach anything. The id (`mem_dev`) is
chosen to read as reserved rather than collide with the `mem_1`, `mem_2`
a seed file's own inserts are likely to use.

### 3 · Dev-only is a call-site argument, not a runtime flag

Nothing checks an environment variable to decide whether to seed the
principal or print the token — the safety argument is that `dev_db::reset`
has exactly one caller in the whole binary, `ordo_cli::dev::settle_delta`,
itself reachable only from `ordo dev`'s own loop. `ordo apply`, `ordo
migrate` and the `ordo-host` server binary never call it, so there is no
code path by which a real deployment's database receives this row or a
real deployment's token secret signs a token naming it. A flag would be a
second thing to get right; a function with one caller is checked by
`grep`.

### 4 · `ORDO_TOKEN_KEY` malformed is a refusal, not a fallback

Read the same way `ordo_identity::TokenKey::from_env` reads it for a real
server: unset is the ordinary case and falls back to the ephemeral key;
set and well-formed overrides it. Set and **malformed** was the one
decision worth naming — silently falling back to the ephemeral key would
hide that a developer's `ORDO_TOKEN_KEY` did not do what they typed it
for, on a page where the answer is what you see is what you get.
Refusing, by name, is the same posture the production path already takes
and costs nothing extra to reuse.

### 5 · The test is a real socket, in-process

The done-when is "call an op over real HTTP", so the test opens a real
`TcpListener`-bound door (`ordo_host::runtime::assemble_embedded`/`serve`)
and speaks to it with `ordo_blob::http::send` — the same client `ordo
apply --host` uses — never `tower::ServiceExt::oneshot`, which proves the
router but not the wire the quickstart's `curl` line actually uses. It
runs in-process rather than as a backgrounded `ordo dev` binary because a
backgrounded child is killed quickly in this sandbox (confirmed again
while building this); the substitution costs nothing the mechanism under
test cares about, since every function it calls — `dev_db::reset`,
`dev_runtime_config`, `assemble_embedded`, `serve` — is the real one, not
a parallel implementation standing in for it. Confirmed to fail first:
with the seeding insert disabled by hand, the same test gets 403
`access_denied`.

## Five rulings from dividing the connection pool (20a, 2026-09-20)

`audit-topology-gaps.md` §2.7 found one semaphore serving every space, no
bound on a statement, and no series naming the space that saturated the
pool. These are the decisions taken while closing it.

### 1 · A per-space cap, and the alternatives it beat

The mechanism is a semaphore of `size - reserve` permits per space, taken
before the pool's own permit. Two alternatives were weighed.

A **fair queue** across tenants, round-robin over per-space wait lists, is
the answer that would bound k noisy tenants rather than one. It needs a
notify loop, a re-check on every wake, and a policy for a space whose held
count exceeds a share that just shrank, and every one of those is a place a
request can be lost or spun. A **dynamic share**, `size / active_spaces`
recomputed per acquire, is smaller to write and flaps: a space holding four
when a second arrives is over a cap it can only leave by finishing, so the
cap has to be advisory, and an advisory cap under load is a suggestion.

The fixed cap ends the failure the audit describes outright, which is one
tenant holding **every** connection, and it does it with one `try_acquire`
on the healthy path. What it does not do is stated where an operator will
read it (`deploying.md` §2a): two spaces at their caps can still reach the
pool's bound, and the reserve is the dial.

### 2 · The space's share is taken first, and out of one budget

A checkout that took the pool's permit and then waited for its space's
share would hold a connection it cannot use while a space that has a share
waits behind it. That is the same starvation in a different shape, so the
order is the mechanism rather than a detail of it.

Both waits come out of one `acquire_timeout`, because the timeout is a
promise made to a caller. A request that queued in two places and waited
twice as long as the deployment declared has been told a number that is not
true.

### 3 · The cap floors at `min(size, 2)`

A pool of two cannot both serve a tenant and reserve a connection. Rather
than halve a single-tenant deployment's concurrency to buy an isolation
guarantee the pool is too small to make, the share stops being a bound at
that size and says so. The guarantee returns as the pool grows, which is
also when a deployment has more than one tenant on it.

### 4 · The statement bound rides the dial, not the checkout

`statement_timeout` is set in the connection's session options rather than
by a `SET` at checkout. A connection then carries it from the moment it
exists, including the re-dial that silently replaces a dead connection
mid-request, and no checkout can forget to apply it. It costs no round trip
on the path whose whole job is to be cheap.

`PoolConfig::connect_config` stays bare. A `LISTEN` session and the
supervisor's leadership lock are session-scoped things the bound would
cancel, and those two are exactly why that method exists.

`statement_timeout` (503) is the only 503 in the wire inventory. The
request is well formed, so the 4xx family is wrong; nothing broke, so
`internal` is wrong; and no stored row decided it, so the 409 family's
"change what you collided with" does not describe the repair. What the
caller is told is that this deployment bounds a statement and theirs did
not fit.

### 5 · Two refusal counters are exempt from the per-space opt-in

`implementation.md` makes per-space series opt-in and the argument is sound
for a family with a series per tenant. `ordo_pool_saturated_total{space}`
and `ordo_pool_space_capped_total{space}` have a series per tenant **that
was refused**, which is the incident population: a deployment where nothing
saturates carries no series at all.

The cardinality the opt-in refuses to pay is the cost of watching
everybody. The cost here is the cost of naming whoever is causing an
incident, and §2.7's disclosure is precisely that a deployment which never
set the variable "sees a slow deployment and no name". The pool's
healthy-path histogram is the family that would cost a series per tenant,
and it stays behind the opt-in with the rest.

---

## Building the stored-chain rule for checkpoints (R2b′, 2026-09-20)

`orphan-checkpoints` (2026-09-19) and `checkpoint-key` (2026-09-20) replace
the seam bound with a question the chain itself answers: does the stored
chain hold this head at this position? Building it settled five things the
rulings left to the code.

### 1 · The markers left the anchor, and the refusal moved with them

Under the old rule `choose_anchor` read every marker, so a `$commandlog`
row whose plaintext `actor` claimed `system:restore` while its body carried
no marker was refused there. The anchor no longer depends on a marker, so
`choose_anchor` no longer reads one.

The refusal moved to `verify`, which reads the markers on every run to
explain orphans, and reads them **whether or not a checkpoint is stored**.
That last clause is deliberate: a chain with no checkpoint yet is exactly
where such a row would otherwise sit unexamined, and one query against an
indexed column is a small price for a property that would otherwise have a
hole in it. `ordo verify` runs both functions, so the binary's answer is
unchanged: exit 3, naming the index.
`a_row_whose_actor_claims_restore_without_a_marker_is_refused` asserts the
new site and the binary's exit code together.

### 2 · An unexplained orphan fails from genesis too, which flips one old assertion

The old truncation check compared the *anchor's* `upto` against the head, so
from genesis, which has no `upto`, a truncated chain verified green. That
was stated as the point of
`a_truncated_tail_is_caught_by_the_checkpoint_that_covered_it`'s first
assertion. Orphans are now judged from `$checkpoints` on every run and from
any anchor, so genesis catches it as well. The assertion flipped; the test's
real claim, that the answer is the first position that went missing, is
unchanged and still holds. Judging from the table rather than from the
anchor is also what makes a forged checkpoint row fail verification naming
itself instead of quietly becoming the anchor.

### 3 · `AlreadyAt` means the head is signed, not that the position is taken

`sign_checkpoint` compared positions. After a restore the chain grows back
through positions an orphan already holds, so that comparison reported "no
work to do" over a head nobody had signed, and the insert's old
`ON CONFLICT ("space", "upto")` would have dropped the row if it had got
that far. The test compares `(upto, head)` against the stored rows, which
is the same question `agreement` asks and one row read cheaper: a stored row
at `upto` whose head is the current head *is* current by construction.

### 4 · The prune needs a fourth bound, and it is temporary by construction

A cut at `F` deletes entries below `F` and leaves `$checkpoints` alone, so
an orphan at `upto >= F` is still classified after the cut while the marker
that explains it may be gone. That turns a properly recorded restore into
`UnexplainedOrphan` on the next verify. `floor_in` therefore drops the cut
to the greatest current checkpoint at or below that marker's index, reported
as `FloorHolder::RestoreMarker { index, orphan_upto }`.

The clamp cannot pin history open indefinitely, and the reason is worth
stating rather than trusting. It applies only to orphans at or above the
cut; once the live bound passes the orphan's position the orphan is below
the floor too, `agreement` answers `BelowFloor`, nothing has to explain it,
and the marker and the orphans it explained leave the chain in one pass.
What is retained past the horizon is bounded by the history the restore
discarded, not by the age of the marker.

### 5 · One plan observation does not reproduce, and the test is kept anyway

The plan records the pre-change failure of
`verify_survives_a_prune_across_the_seam` as `PrefixMissing
{ first_stored: 10 }`, on the reasoning that the seam bound is 4 and no
checkpoint sits at or below it. It does not reproduce: the cut that removes
the prefix removes the marker at index 4 with it, so the old rule has no
seam bound left and falls back to the newest checkpoint, which is the cut
point. The test earns its place on what it does pin — that a pruned marker
does not disturb verification, and that an orphan below the floor is
reported as neither current nor orphaned — so it is kept with the
observation corrected rather than dropped. The two old-rule failures that do
reproduce are `a_later_unannounced_rewind_is_rewound_not_explained`
(`Intact`, then `Explained`, where the rule reads `Rewound`) and
`a_checkpoint_at_an_orphans_position_is_signed_and_both_rows_are_kept`
(`AlreadyAt(6)` on the first call). The clamp's own test fails with the
clamp removed, which is the third observation.

---

## Building the witness, the record and the drill (R2c to R3, 2026-09-20)

`restore-witness` (2026-09-19) settles where the record of a signed head
lives; R2c to R3 build it, the command that writes a restore into the chain,
the two refusals, the verifier's reading and the drill. Six things the
rulings left to the code.

### 1 · `AlreadyRecorded` needs a second reading, or it never fires

The plan refuses a second marker "when the newest marker has the same
`restored_from`". On an ordinary re-run that never matches: the first marker
moved the head, so the second call reads a different point from the chain
and writes a second marker at the head. The case the refusal exists for is
the one it cannot see.

A second reading catches it, and it is the simpler sentence: **the newest
marker is the head**, so nothing has happened since it was written. Either
reading is enough, and the plan's still fires when the operator names the
point with `--restored-from`.

### 2 · The gap needs the operator, because the chain cannot know

§2.4 defines `restored_from` as the last entry that survived the restore and
says it differs from the marker's `prev` when ordinary writes land first,
with the command printing the gap. The R2c build note says `head_of` supplies
`restored_from`, which makes the two equal by construction and the gap
always zero.

They cannot both hold. Nothing in the chain distinguishes a write that landed
after a restore from any other write, so the only source for "where history
broke" is the operator: `--restored-from <idx>`, defaulting to the head, and
an index the chain does not hold is refused rather than recorded as a claim
about nothing.

### 3 · A witnessed checkpoint keeps the bound rule until a restore is recorded after it

R2d-ii gives a witness row one explanation (a `restore` row with a greater
`seq`) and a stored row another (the bound rule over the markers). Taken
strictly, a checkpoint that is *both* stored and witnessed loses the bound
rule, and R2b′'s own continuity tests fail — which R2d-ii's done-when says
they must not.

The rule that satisfies both: a witnessed checkpoint may fall back to the
bound rule **while no `restore` row precedes it in `seq`**. Once a restore
has been recorded after this head was witnessed, no recorded restore accounts
for it, because it was signed over the history that existed after that
restore. That is exactly the case
`a_checkpoint_witnessed_after_the_restore_is_a_fresh_rewind_even_below_the_attested_head`
pins, and it is the reading `seq` gives that a bound over the markers cannot.

### 4 · The cursor is still the window; what R2e changes is what is inside it

The plan describes `enqueue_due` fanning out "every attestation that is
`Current` and has neither a delivery row nor a dead letter". A landed
delivery leaves neither — the receiver holds the record — so that rule
re-sends every delivered head on every tick, which is what
`the_cursor_is_at_least_once_and_never_re_delivers_a_landed_position` exists
to forbid.

The cursor stays as the window and as the at-least-once guarantee. R2e adds
two filters inside it: a head the chain no longer holds is not sent, and a
dead letter is not resurrected. What reaches a receiver at a *reused*
position is the cursor **going back**, which `record_restore` does and which
the plan already specifies; the `(space, upto, head)` key is what lets the
new head land beside the old row rather than instead of it.

### 5 · The refusal at `resolve` caches passes only, and an unreadable witness is not a pass

A cached refusal would keep refusing after `ordo restore --record` had fixed
it, and every replica would need restarting to serve again: a recorded
restore would become an outage. So the set holds passing verdicts only, and
the next request after the record re-asks and is served.

The staleness that leaves is stated where the check runs rather than left to
be discovered: a space restored under a running process is caught by the
checkpoint worker's next tick for `ORDO_SPACE`, by `ordo verify
--control-db` for any space, and at the next boot. A continuity read that
*errors* is refused as unroutable, not admitted as a pass: "I could not
check" is not "it checks out".

### 6 · The drill's step 2 is `Spaces::resolve`, because `assemble` cannot route

R3 is amended to copy an attached target so the witness survives, and then
asks `assemble` for the refusal. `assemble` boots with the control pool and
the host binary does not construct an `AttachedPools`, a gap the plan records
in R2d-iii itself, so `assemble` cannot reach an attached space's chain — it
would read the control database and refuse for the wrong reason, which is a
worse test than none.

The boot site that covers an attached space is `Spaces::resolve`, and the
drill exercises that. `assemble`'s own refusal was proved on a single
database by `a_rewound_space_refuses_the_boot_until_its_restore_is_recorded`,
whose own observation is worth recording: with the refusal removed the binary
does not crash, it **serves**, so the test waited on the child under a bound
and failed rather than hanging.

That observation became the design on 2026-09-21. 17aa deleted the
process-wide refusal, and the test that replaced it,
`a_rewound_space_is_quarantined_and_the_door_stays_up`, asserts that the
binary boots and that the space does not serve: the behaviour the old test
treated as the regression is now half of the requirement, and the other half
is the part that never changed.

---

## Two rulings on where work happens, and on signing (2026-09-20)

**The CLI triggers actions on the server, not locally.** The owner: "The
CLI should trigger actions on the server not locally." This generalises
the same day's ruling that the apply belongs to the service. The operator
surface was written when there was no server, so it reaches into Postgres
and does the work in the operator's own process. With a server, that is
backwards: the deployment holds the custody, the pool, the locks and the
audit trail, and a laptop holding database credentials is a second place
the platform can be operated from, with none of those.

So the door is the default for every command that acts on a deployment,
and the CLI is a client: it parses, sends, renders and exits. The
database path does not disappear, because a tool that only works when the
deployment works cannot diagnose a deployment that does not. It becomes
**break-glass**: named as such in the help text, for the acts that must
work when the door is down (custody init before there is a door to call,
chain verification when the server is suspect, revert when it will not
start). That is a narrower set than the one the 2026-09-20 ruling left,
which kept the database path for those acts as their ordinary home rather
than as a fallback.

**What this changes in practice:** `import` goes over the wire rather
than waiting for the CLI to grow its own custody selection, since the
server already holds the master key (11-10a, 17y gap 1). `rollback`,
`deploys`, `migrate --apply`, `quota`, `usage` and `restore --record`
follow. Each needs an owner-gated platform op beside
`std.deployment.apply`, and each keeps its database form behind the
break-glass label.

**Artifact attestations stay off, for now.** The owner: "I'll leave it
off for now. Maybe I'll just do some manual signing at some point, but it
will end up public anyways at some point." Signed build provenance
requires GitHub Enterprise Cloud for a private repository, and is free
once a repository is public. The release workflow keeps the step, made
non-fatal on 2026-09-20 after it failed a release whose binaries had all
built, and prints whether provenance was attached, so a green release
never implies a signature it does not have. Tarballs ship with SHA-256
files meanwhile, which catch corruption and not substitution. Revisit
when the repository goes public, which costs nothing, or if a consumer
outside this organisation appears first.

---

## The control plane boots without the deployment under it (2026-09-21)

The owner, answering how a restore is recorded when the deployment will
not start, and how a first tenant gets custody:

> "The CLI is just a way to work with the server... If the system isn't up
> there's no way to record it. There must be some reconciliation process on
> startup if it was broken and needed manual intervention to fix. But the
> system should not be dependent on the rest of the data designs,
> deployment space, or tenants. The system is the control plane and should
> function without teeth into those. Dependence is on the control plane,
> not the deployment underneath."

And: *"everything happens within the server. But a first tenant
(non-system) can't exist until something is applied."*

**This dissolves the circularity rather than working around it.** The
refusal that made recovery impossible was process-wide: `assemble` returns
`BootRefusal::Rewound` when `ORDO_SPACE`'s chain sits below a head the
witness remembers, so one tenant's rewound chain stops the whole
deployment, including the door through which the restore would be
recorded. Under this ruling that is the wrong shape twice over: it makes
the control plane depend on a tenant's data state, and it removes the only
surface an operator has.

**The rule.** The system space is the control plane. It boots on its own
custody, its own tables and nothing else. A tenant's state, including a
chain that has been rewound, a schema that will not load, or custody that
is missing, is a fact *about that tenant*: it is refused at the tenant's
boundary, reported, and never allowed to take the control plane down.
Dependence runs one way, from a tenant to the control plane, never back.

**What follows, and each is work:**

1. **The rewind refusal moves from the process to the space.** A rewound
   tenant is quarantined and says so, with the same force it refuses with
   today; the deployment stays up and keeps serving every other tenant and
   the control plane. Without this, one restored tenant is an outage for
   all of them.
2. **Startup reconciliation.** A deployment that was repaired by hand
   reconciles at boot: it reads what it finds, compares it against the
   witness, and either explains the difference or holds the affected space
   closed and names what an operator must do. A marker written afterwards
   through the door is then possible, because the door is up.
3. **A tenant comes into being through an apply, not before it.** Nothing
   needs to write custody into a database for a tenant that does not yet
   exist. The act that creates a tenant is an apply through the door,
   which also keeps the property that a typo cannot mint one, because the
   name is in a program someone deployed rather than in a shell command.

**What this does not loosen.** A rewound tenant still does not serve. The
control plane staying up is not permission to serve data whose history is
in question; it is what makes the repair possible.

---

## The control plane is a service above deployments (2026-09-21)

The owner, answering the multi-deployment evaluation:

> "The control plane/system exists out of the concept of the deployment /
> tenants. It is there to serve and manage the system and should in theory
> be able to serve more. I intend to host this as a paid service... 1
> deployment, 1 separately managed master key... they should be globally
> ID'd and that's managed by the control plane. The deployment should have
> no concept of the control plane, just that it gets to live somewhere.
> Think of docker service abstraction."

**Four rulings, and a naming correction.**

1. **Serving many deployments is a product requirement**, not a
   hypothetical: the platform is to be hosted as a paid service. The
   evaluation's shape (c) is the shape: a control plane **above**
   deployments that registers, names, provisions and bills them.
2. **One deployment, one separately managed master key.** This forecloses
   the one-process-many-deployments shape permanently, which is the point:
   one process stays one custody perimeter, and a process compromise
   cannot become every customer.
3. **Identities are global and the control plane owns them.** Space names
   and deployment names are minted and kept unique there, not by whoever
   types a name. Three tables already key on space alone, so a collision
   would be two tenants on one chain rather than an error.
4. **A deployment has no concept of the control plane.** It is handed its
   configuration and told where to live, the way a container is. It does
   not call home, does not discover, and cannot reach across.

**The naming correction this forces.** "Control plane" now means two
different things: the **system space inside a deployment** (the
2026-09-21 ruling that it boots without the deployment under it), and the
**service above deployments** ruled here. They are different scopes with
different failure domains. The service above is named **the platform
service** from here; the system space keeps "control plane" where it
already appears in the code and the earlier ruling.

**The tension this leaves, named rather than glossed.** The restore
witness has a deployment write every signed checkpoint head somewhere a
restore of its own database cannot roll back. Under ruling 4 the
deployment must not know that place is shared, and under the earlier
ruling it may depend on the control plane. Both hold if the deployment is
simply configured with a control database and never told what is on the
other side, exactly as it is configured with a Postgres today. What must
not happen is the deployment learning to discover the platform service, or
the platform service reaching into a deployment's spaces. Dependence runs
one way and stays configuration, not discovery.

---

## The console, the operator, and where deployments run (2026-09-21)

Refining the same day's ruling, after the owner read the definitions back:

> "Control plane term can belong to the deployment system space. Platform,
> this is the Ordo Console, we can call it the console. I want
> multideployment to specifically exist for me so it is easier to manage
> multiple deployments. There will be an operator at some point... the Ordo
> Server should be able to manage a deployment and the deployment should
> not care what comes above it. I think the server and console should exist
> together within one pod for general release. For my release it should be
> bundled separately."

**Names, settled.** "Control plane" means a deployment's **system space**
and nothing else. The surface above is the **console**. The software that
creates and configures deployments is the **operator**, in the Kubernetes
sense. "Platform service" is retired as a term.

**A deployment is not a process.** It is one database, one program, one
master key and one space of its own, served by one or more **replicas**.
Several replicas are ordinary, which is why the supervisor elects a leader.
Scaling out adds replicas, not deployments. And an Ordo deployment is not a
Kubernetes Deployment; the latter is one way to run the former's replicas.

**One console, two modes.** Bundled with a deployment it manages that one;
run by an operator it manages many. The same surface, and in both modes it
reaches a deployment **through the door**, as an owner with a token, never
into its database. A console that read the database would be a privileged
back door around the rule that the system is the source of truth, and it
would have to be built twice.

**Multi-deployment belongs to the operator, not to the server.** The Ordo
server manages one deployment and does not know what is above it. Running
several is the operator's job: it provisions them, gives each its own
master key, and places them. That keeps one deployment equal to one custody
perimeter however they are packed.

**On packing them to save cost**, which was the owner's reason for wanting
several in one pod: several deployments may share a pod, a node or a
Postgres cluster, as long as each keeps its own database, its own master
key and its own process. What must not happen is one process serving
several deployments, which is the shape that trades away the custody
perimeter and requires the engine changes 18n found. The cost of a
deployment is its database and its idle memory, not its pod.


### What was built, 2026-09-21: pieces 1 and 2

**The refusal moved, and nothing about it loosened.** `BootRefusal::Rewound`
no longer exists. `Spaces::continuity_of` is the single function that asks
the continuity question, and both callers reach it: `Spaces::resolve`, on the
first request to name a space, and `runtime::reconcile_at_boot`, which reads
the system space and `ORDO_SPACE` at startup. A rewound tenant is refused by
`SpaceRefusal::Rewound` with the class it was refused with before, and the
deployment keeps serving every other tenant and the control plane.

**What readiness says.** `Spaces` holds a quarantine register that nothing
refuses from: `resolve` re-asks the question every time for a space that has
not passed, which is what makes a recorded restore lift the quarantine with
no restart, and the register exists so an operator hears about a closed
tenant from the deployment rather than from that tenant's customers.
`/health/ready` reports it as `quarantined`, an array of `{space, reason}`
beside `program_active`, and gains one check, `continuity`, which answers for
the **system space alone**. A tenant's history never decides whether the
deployment is ready, because readiness that failed on it would be the
process-wide refusal wearing a status code. The system space is the
exception for the reason it exists: a control plane that cannot account for
its own history is not ready to apply and run deployments.

**What reconciliation settles, and what it refuses to.** It settles a
difference somebody already wrote down: a chain that contradicts a recorded
head, where a restore marker attests the difference and the chain reaches
that marker, is `Explained` and the space serves. That is the loop a recorded
restore opens and the next boot closes, and it is why recording one through
the door works at all. It will not write a marker, infer one, or wave a
rewind through because it looks plausible; that act is the one the restore
mechanism exists to prevent. A continuity read that errors holds the space
closed too. It repairs nothing in the database: missing rows stay missing,
and the operator's acts stay the operator's.

**The startup report** prints one line per space read, and under a closed one
the two lawful remedies and the fact that every other space is served.

## Two answers on packing and on the fork (2026-09-21)

**Idle memory is the cost, and a replica is not free.** The owner: *"Each
pod adds idle memory. RAM is expensive, I'm trying to budget, but
horizontal scaling within a pod space makes a certain amount of sense,
adding one more layer of availability, but maybe it does add increased
complexity."* Both halves are right, and the shape that follows is:

- **A replica costs its own pool, its own supervisor and its own caches.**
  Two replicas of a deployment are not two halves of one; each dials the
  database, each builds the access engine at boot and on every swap, and
  each holds the live program and its schema graph. The saving from
  packing two *deployments* into one pod is the pod's own overhead only,
  which is small; what costs is the second set of everything above.
- **So the first lever is fewer replicas, not smaller pods.** One replica
  per deployment is the honest default for a deployment nobody is paying
  for availability on. The supervisor already tolerates one: leadership is
  taken by whoever is there.
- **The second lever is sharing the database server, not the process.**
  Ten deployments against one Postgres cluster, each with its own
  database, cost one server's baseline rather than ten.
- **Availability is a per-deployment choice, not a platform default.**
  Two replicas is a thing an operator turns on for a deployment that has
  earned it, and the complexity the owner suspects is real: a second
  replica means leadership handover, a second set of connections against
  the same database, and a quarantine or a swap that must be true on both.
  Worth it for a paying tenant, not for all of them.

**The fork, and why it is not a branch.** The owner: *"It wouldn't be a
branch, it would be a forked repo, occasionally pulling in upstream. The
seams are worth that separation. Main repo will be open source, my private
repo will not."* A fork that pulls from upstream is a different thing from
a long-lived feature branch, and the objection to branches does not apply
to it: the private repository carries what upstream must not have, and
takes upstream's changes on its own cadence. What makes it work is that
the seam is small and stable. So the obligation this ruling creates is on
**upstream**, not on the fork: the operator must attach through interfaces
the open repository already has (the door, the configuration surface, the
usage ledger), and nothing in the private repository may need a patch to
an open file. If the fork ever has to edit engine code to do its job, the
seam is in the wrong place and the interface is the thing to fix.

**Tenant-level scaling is foreseen and not designed.** The owner expects a
tenant busy enough to need dedicated resources. Ordo already has the
pieces that make that possible without new concepts: a space can have its
own database (18c/18d, `AttachedPools`), and the pool is bounded per space
(20a). What is missing is placement: which replicas serve which spaces.
Recorded so that the next person who needs it does not invent a second
tenancy model.

## Topology: what runs where, and what points at which database (2026-09-21)

The owner, settling the shape:

> "Each deployment can be pointed to any particular Postgres cluster, so
> the deployment requires a Postgres connection but is not tied to a
> specific one, since it has no concept of the console. Given that, it
> follows that the console can have its own Postgres cluster connection for
> its own data... One deployment can be within a pod, or more than one
> deployment can run in that pod. It's just instances of software, it
> shouldn't care. And tenants exist under that... Public release = one Ordo
> console pod, runs a deployment within itself, and the deployment can
> point to a database and the console to another."

**Three layers, each pointed at a database independently.**

1. **A deployment** takes a Postgres connection as configuration and does
   not care whose it is. It has no concept of the console, so nothing about
   the console's storage can constrain it.
2. **The console** keeps its own data in its own database, for the same
   reason and in the same way. Its storage is not the deployment's
   business, and the deployment's is not its.
3. **A tenant** may sit in the deployment's database or in one of its own.
   The design is indifferent: what matters is that the data is reachable,
   not where it lives.

**One correction, because the third layer is designed and not wired.**
`$spacedb` exists, `ordo_exec::route::AttachedPools` exists, the directory
reads attachments, and the tests exercise them. **The shipped binary
constructs `SinglePool`** (`runtime.rs`), so today every space resolves to
the one configured database. Per-tenant databases are a seam that is
built and not connected, filed under 18c/18d. A tenant that must have its
own database cannot have one yet, whatever the design says.

**Packing is an operational choice, not an architectural one.** One pod
may run one deployment or several; they are processes and should not care.
If a pod runs out of room, the deployments spread across pods. Availability
within a deployment is replicas, and it is a per-deployment choice rather
than a default. None of this reaches the engine: a deployment is
configured, told where to live, and never learns what else is on the node.

**The public release shape, ruled:** one pod, the console and one
deployment inside it, each pointed at its own database. That is what a
self-hoster gets and what the documentation should describe.

**The one constraint that must survive co-location.** A console in the
same pod as a deployment still reaches it **through the door**, as an
owner with a token, over localhost. Sharing a pod is not a reason to share
a process or to reach into a database. The moment the console reads the
deployment's database directly it becomes a second, unaudited way to
operate the platform, and the rule that the system is the source of truth
stops being true.

**The console as operator** follows from the same shape: it places
deployments, and later assigns workloads to pods. That is the private
fork's job and needs nothing from the engine beyond configuration, which
is the test of whether the seam is in the right place.

## A tenant is an id, and its name is an attribute (2026-09-21)

The owner, resolving how a tenant comes into being: *"Names are irrelevant,
everything should be by id, and there can be attributes under the id that
are not global. Only the ID is globally unique for a tenant."*

**What this changes.** Today a space's id *is* its name: an operator types
`acme`, and that string is the identity in the header, in every table's
space column, in custody, in the chain and in the append lock. Nothing
mints one, and `well_formed_space` only checks the character set. So
uniqueness has been a thing people were trusted to arrange.

**The rule.** A tenant is identified by a **minted, globally unique id**.
Everything the platform keys on uses that id and nothing else. **The id carries the
uniqueness. A name exists alongside it purely for human reference**, and
carries nothing: it is not identity, nothing keys on it, and it is not
required to be unique anywhere.

**An id is not a secret, and "opaque" was the wrong word for it** (owner,
2026-09-21: *"ids don't have to be sealed"*). A space id travels in a
request header, appears in metric labels, in logs, in refusal messages and
in the operator surfaces, so it is public by construction and must never
be treated as though it were not. What it must be is *minted and unique*,
not unguessable. It may carry a readable prefix or fragment if that helps
a human place it, as long as nothing keys on that fragment and nothing
sensitive is in it. Authority never comes from knowing an id: every call
is authorised by a token and a membership, and an id in the wrong hands
opens nothing.

**Three consequences worth stating, because they are why this is the right
answer and not merely a preference:**

1. **Uniqueness stops being a promise and becomes a property.** Mint with
   enough entropy and no registry has to allocate ids, no two deployments
   can collide, and the 18n hazard (three tables keyed on space alone, a
   collision being two tenants on one chain rather than an error) cannot
   arise. The console records ids; it does not have to hand them out, and
   a self-hosted deployment with no console mints its own safely.
2. **A rename stops being a migration.** With the name as identity,
   changing it means rewriting every row's space column, the chain's
   lock key and custody. With the name as an attribute, it is an update.
3. **It closes piece 3 of 17aa.** A program cannot name a tenant, which
   the compiler already enforces three times over, and now nothing wants
   to: there is no name to compile in, only an id that did not exist when
   the program was written. A tenant is created by an owner-gated call
   that mints the id and records the caller's chosen name beside it. The
   owner's sentence, *"a first tenant cannot exist until something is
   applied"*, is about order and remains true: a program's DDL is what
   gives the tenant its tables.

**What does not change, and this is why it is cheap.** The door still
takes a space in a header, the character set the door accepts is unchanged,
and every existing deployment keeps working, because a minted id is a
well-formed space string. Well-known reserved ids stay as they are: the
system space is a constant compiled into the binary and is not minted.
What changes is how a *new* tenant's id comes into being, and that names
stop being load-bearing anywhere.

**What this obliges, filed as work:** minting on creation with a stated
shape and enough entropy that two deployments never collide; a place for
attributes below the id, which is where the display name lives; and the
operator-facing surfaces (`/metrics`, `ordo usage`, the console) able to
show that name beside the id, since the id identifies and the name is what
reads.

## Two answers on per-tenant databases (2026-09-21)

**Occupancy is the application developer's to disclose.** The owner: *"Since
it is configurable that belongs to the app developer to disclose."* So the
platform's obligation is to **record and expose the fact**, not to write
the sentence: which target a space sits on, and whether anything else is on
it, readable by the operator and by the developer whose application it is.
What that means for the person whose data it is, the developer says, in the
words their own product uses, because they are the ones who chose the
configuration and who have a relationship with that person. Ordo does not
invent a disclosure on their behalf, and it does not let the fact be
unavailable.

**A gap, never a zero, and the owner's question about what is even being
counted was the right one.** He asked: *"If it is unreachable transactions
cannot happen, unless we mean the local ledger?"* Both, and they fail
differently, which is why "unreachable" needed splitting:

- **Door-side counters** (requests by class, rows returned) are counted in
  the replica's memory as calls arrive and flushed to `$usage_replica` in
  the **control database**. A tenant whose own database is unreachable
  still generates these: its callers arrive and are refused. Those refusals
  are real activity and are recorded, so there is no gap here, and none
  should be invented.
- **Database-side quantities** (writes and rows written from the command
  log, bytes from the sealed payloads, the blob references and the live
  schema) are a **measurement of the tenant's database**, taken by the
  leader at rollup. If that database cannot be reached at rollup time, the
  measurement did not happen, and the row for that period must say so
  rather than carry a zero.

A zero asserts that nothing happened, which the deployment cannot claim
about a database it could not read. It is also a silent discount on the
bill and hides the outage. A gap is the honest record and is the signal an
operator wants. When the target comes back, the period can be measured
again, because the command log it is measuring is still there.

## Thirteen rulings on per-tenant databases and buckets (2026-09-21)

Answers to the decisions filed from `plan-space-routing.md` §11 and
`plan-tenant-storage.md`. Where an answer moves a recommendation, the move
is recorded rather than smoothed over.

**D-A, the switch ships now** (*"now. its critical."*), which overrides the
recommendation to hold T6 to T8. So the jobs table and schema-everywhere
are in scope rather than deferred, because a switch that lands without T6
leaves an attached tenant's background work claimed by nobody, and the
owner has asked for the switch rather than for half of it.

**D-C, the reserve stays unconditional and configurable** (*"configurable
makes more sense to me"*). Per-target defaults, no derivation from live
occupancy.

**D-E and D-S6, one allowlist for both tiers, configured at the
deployment** (*"per tenant configurations are still configurations,
configured at the deployment level. Egress filtering... if I defined it on
my platform it is allowed"*). A database host and a bucket endpoint are
the same kind of fact: an address the deployment will dial, chosen by
data. One mechanism, one refusal reason, and it belongs beside the
existing outbound egress filtering rather than as a second list. The
loopback exception moves behind a dev flag.

**D-F, the attachment does not go in the tenant's chain** (*"there's no
provability unless it's BYO DB... I think this belongs to the app not
Ordo"*), which reverses the recommendation. The reasoning holds: a chain
entry saying where data was put is a claim the deployment makes about
itself, and a tenant who genuinely holds their own database can see where
their data is without being told. The disclosure belongs to the
application, which is the same answer the owner gave for occupancy.

**D-H, credentials are KMS first with optional environment or file**
(*"KMS with optional envs, configurable will allow easier deployment but
only if it makes sense, otherwise KMS only"*), which moves off the
directory-only recommendation. The provider abstraction is unchanged; what
changes is which implementation ships first. **This raises a question the
plans do not answer: which KMS.** The deployment already reads its secrets
from GCP Secret Manager in the cluster, and the master key already
supports a sealed file, so a KMS-only rule for tenant credentials while
the master key still opens from a file is an inconsistency worth settling
deliberately.

**D-S1 two switches, D-S3 one deployment-wide sweep cadence (also closing
17g's open cadence), D-S4 key pairs only in v1 with roles on the
post-deployment roadmap, D-S7 publish the least-privilege bucket policy,
and D-S8 refuse a second tenant inside an owned bucket and expose
occupancy**, all as recommended.

**D-S2, attaching a bucket that already holds objects is refused, with an
exception the owner named:** *"yes unless everything matches, i.e. a 1 for
1 migration would be indistinguishable."* The exception is right and it
collides with D-S7: proving that everything matches would ordinarily mean
listing the bucket, and the store interface has no listing and the
published policy grants none. **The shape that satisfies both:** verify
from the references rather than from the bucket. For every row in
`$blobref`, ask the target for that object with a head request. If every
reference resolves, the migration is one for one as far as Ordo can ever
know, and the attach proceeds. Objects in the bucket that no reference
names cannot be enumerated, are not Ordo's, and stay the tenant's. That
keeps the credential unable to list, which is the point of D-S7, and turns
the exception into N head requests bounded by the reference count.

## Erasure proof, and a secret provider the service does not know about (2026-09-21)

**D-S5 confirmed.** Under a tenant-owned perimeter, erasure destroys the
keys, leaves the objects, and writes what it left into the chain as a
custody action: how many objects remained, which store, and the epoch
whose destruction made them unopenable. Ordo asserts only its own act and
never that the bytes are gone.

**Erasure proof is a transaction between the application and its user, and
the platform's job is to make it provable.** The owner: *"There should be
the ability to match in some zero trust chain. The decision in these cases
would be a transaction between app dev and client, but we should enable
them with tools to do it."*

The distinction that makes this buildable: Ordo cannot prove bytes are
gone, and nobody can, because absence is not observable from outside the
store that holds them. What Ordo can prove is **unopenability**, which is
the claim cryptographic erasure actually makes, and it can prove it
without being trusted. The pieces already exist and are not yet pointed at
this:

- **The chain records the act**, and the checkpoint signer signs the chain.
  A destroyed epoch is already a custody action.
- **The offline verifier** (`ordo-verify-proof`) checks a proof against a
  published verifying key with no access to the deployment, which is what
  makes it zero trust: the application hands it over, and the user checks
  it without asking either the application or Ordo to be honest.
- **The negative test is the strongest half, and it costs nothing.** A
  user holding a sealed value can attempt to open it after the erasure and
  fail. That is a check they run themselves, against the running system,
  and no signature is required for it to mean something.

So the tool is a **receipt**: the signed chain entry for the erasure, the
epoch it destroyed, the count and location of what was left, and the proof
that entry is in a chain whose head was signed. The application hands that
to its user, and the user verifies it offline. Ordo never speaks to the
user and never makes a claim about a perimeter it does not hold.

**The same shape answers the attach match.** A one-for-one migration
produces a manifest (every reference, and the digest the target returned
for it), and signing that manifest into the chain gives both sides the
same verifiable set without anyone listing the bucket.

**Secrets: the service does not know what a KMS is.** The owner: *"Secret
manager should be agnostic, the service shouldn't care, KMS comes from
above."* That is the same rule as the console: the deployment is
configured and told where to live. So the provider reads from a **mount or
an environment**, and whatever fills that mount, a KMS, a secret operator,
a human with a file, is above the deployment and invisible to it. This
resolves the KMS question by dissolving it: there is no KMS binding to
choose, and no inconsistency with the master key, which already opens
from a file the same way.

**Rotation, which the owner flagged as unclear, and which has two
different answers.** They must not be confused:

1. **A credential rotating** (a database password, a bucket key pair) is
   the ordinary case. The value in the mount changes. The deployment must
   pick it up without a restart, which today it would not: the pool reads
   its password once at boot. The requirement is to read through the
   provider, cache, and **invalidate on an authentication failure and
   retry once**, so a rotation is at worst one failed request. The
   rotator's obligation is the overlap: leave the old credential valid
   until the new one is in the mount, which every secret manager supports
   and which is the operator's to arrange.
2. **The master key rotating** is not that. It is `rewrap-master`, which
   is built, transactional, and proved not to move a blind-index digest.
   Nothing about it changes here.

A tenant's data keys are a third thing again, and they are K1's re-seal
pass, also built.


## A tenant whose database is behind the program is a per-tenant contract (2026-09-21)

**The question, 18d · D-B.** A tenant's database can lag the activated
artifact: the deploy applied everywhere it could reach, and one target was
unreachable, or one tenant was attached from a cluster still at the older
shape. That tenant's program references tables that are not there. The
choice was quarantine the tenant, or serve it the older program its
database actually matches.

**The recommendation was quarantine, unconditionally** (R-S4). The
argument was that serving a lagging tenant the old artifact means one
deployment holding two artifacts at once, which is a different product.

**The owner ruled otherwise.** *"It should be configurable per tenant.
This would be a deployment or possible maintenance window contract."*

So the behaviour is a per-tenant setting rather than a platform-wide law,
and the setting is a **contract**: the tenant's deployment window, or
their maintenance window, says when their database may be moved and what
happens in the interval where it has not been. A tenant who has bought a
window where nothing changes under them gets the version their database
holds until that window opens. A tenant with no such contract is
quarantined, which keeps the recommendation as the default and makes the
tolerance the thing you opt into.

**The cost this accepts, stated plainly, because the ruling moves onto
it.** A deployment that can serve a lagging tenant the older artifact is
a deployment that holds more than one artifact. `Deployment` today is one
`RwLock<Arc<Loaded>>`, swapped whole, and everything downstream of it
assumes the artifact a request resolves is the artifact every other
request resolves. Retaining a previous version means the previous
`Verified` stays loaded, the resolver picks an artifact per tenant rather
than per deployment, and the platform ops that are deliberately outside
`Loaded` have to keep being outside it for both. That is the multi-version
build, bought deliberately rather than refused.

**What bounds it, proposed and not ruled.** The ruling settles that the
behaviour is per-tenant. It does not settle how far back a tenant may
lag, and the difference between one version and arbitrary history is the
difference between a bounded build and an open one. The shape that seems
to follow from calling it a *window*:

1. **One version back, never two.** A tenant at N-1 is served N-1 under
   contract. A tenant at N-2 is quarantined regardless of contract,
   because a window that has been missed is not a window.
2. **The contract carries an expiry.** It names when the tenant's
   database will be moved, and when that time passes without the move,
   the tenant quarantines. An indefinite contract is a tenant who never
   upgrades, and a deployment that can never drop an artifact.
3. **A change the old artifact cannot survive refuses at deploy time,
   not at request time.** A destructive change, a removed field, a
   changed authorization rule: the apply reports up front which
   contracted tenants it will break, and the operator decides before the
   activation rather than discovering it per request.

Those three are the open follow-up (D-B2), not a ruling.

## The lag bound, and what "contract" meant (2026-09-21)

**Two corrections and a partial answer**, from the owner on D-B2: *"I
think one version is fine, I'd rather it be flexible, and contract I mean
app-dev to customer. But yeah one version makes sense ideally but there
could be broken situations where you may want to revert down the chain.
That may be a business requirement and that is what I am considering."*

**Correction 1: the contract is between the application developer and
their customer.** D-B was recorded as a deployment or maintenance-window
contract, which reads as something the platform offers a tenant. It is
not. The party promising a customer that nothing will change under them
is the **application developer**, who owns the program and runs the
deployment; the customer is the tenant. So Ordo's job here is to supply
the knob and enforce it, never to define the policy, which is the same
position the platform already takes on custody disclosure: state what is
true and let the parties contract on it. It also settles who sets the
value: the deployment owner, because that is the app developer, and it is
already the only principal `std.space.create` and `std.deployment.apply`
answer to.

**Correction 2, and it is the one that matters: lag and revert are not
the same mechanism, and only one of them is expensive.**

- **Reverting the deployment down the chain is already unbounded and
  already built.** `$artifacts` is content-addressed and *never deleted
  from*, and the foreign key from `$activations.version` makes deleting
  an artifact that was ever active impossible, so "old artifacts are
  retained" is enforced by the schema rather than promised. A rollback is
  an ordinary activation naming an older version. There is no depth limit
  today and none is proposed.
- **Lag is different: it is two versions serving at the same instant**,
  one tenant on the current artifact and another on an older one. That is
  what needs a second `Loaded` alive and a resolver that picks per tenant,
  and that is what the bound is about.

So the broken situation the owner is weighing — *you may want to revert
down the chain* — does not need the lag bound relaxed. Reverting the
whole deployment three versions is available now and stays available. A
one-version lag bound costs nothing in that scenario, because in it every
tenant moves together.

**What actually limits a deep revert is the schema, not the bound.** The
load gate compares the candidate against the live database, so reverting
to a program whose shape no longer matches a migrated database is refused
there. That is the real constraint, it is unchanged by anything here, and
it is worth naming because it is the one a business requirement would run
into.

**So the partial answer recorded is: one version back**, with the
flexibility the owner wants supplied by the revert path rather than by
widening the lag window. **Still open**: whether that is accepted, since
the owner's own words are *"that is what I am considering."* Nothing is
built either way until it is.

**Added by the owner the same day, and it is a third answer to the
"broken situation":** *"It may make sense to be able to create a snapshot
the user can offload and keep somewhere. This will also require migration
instructions, which should function cross-versions, but the concern is
data destruction, but that is no different than any migration where a
schema change could invalidate or destroy data as part of that
migration."*

Most of this is built. `std.space.export` produces the bundle,
`ordo_exec::import::import_space` loads it back through the write
protocol, and the loader is schema-driven by construction: *"Nothing here
knows an entity name, a field name or a type."*

**One deliberate refusal blocks it.** The import compares the bundle's
`artifact_version` against the deployed one and refuses any difference
(`ImportError::ArtifactMismatch`). So a snapshot restores only into the
exact program it was taken under, which is the version most likely to be
gone when somebody reaches for it. Filed as **11-10b**: narrow that
refusal from *any version difference* to *the changes the schema diff
cannot carry*, using the diff engine 13a–13e already has.

**The owner's data-destruction point is correct and not a reason to
stop.** A cross-version restore can lose a dropped field's values exactly
as a live migration can. The asymmetry favours the snapshot: the bundle
still exists afterwards, so the loss is confined to the restored space
rather than being the only copy, which is what a migrated live database's
dropped column is.

**And it strengthens one version as the lag bound.** If the safety net
for *we need to go back* is a bundle the customer holds, the platform
does not need to be the safety net by keeping N programs serving at once.
That is cheaper for the deployment and worth more to the customer, whose
bundle outlives the deployment.

## D-B2 settled: one version back, with a snapshot behind it (2026-09-21)

The owner: *"One version back + ability for snapshots (for requesting
customers on the app dev side) and ensure step by step downgrading in
schema."*

**One version back is the lag bound.** A tenant under contract may be
served the program its database matches for exactly one version. Two
behind quarantines regardless, because a window that has been missed is
not a window. So a deployment holds two artifacts at most, which is the
bounded build rather than the open one.

**Snapshots are requested, not universal, and requested on the app
developer's side.** This follows from D-B's own correction: the contract
is app-developer to customer, so it is the app developer who asks for a
customer's snapshot, and the platform's job is to make one available
rather than to decide who gets one. A snapshot that every tenant
generated by default would be a storage cost and a disclosure surface
the platform imposed on people who never asked.

**Step by step downgrading in schema**, and this is the sharpest of the
three because it constrains 11-10b's shape rather than its existence. A
restore across versions walks the chain **one version at a time**, not as
a single diff computed between the ends. Three reasons it has to be that
way:

1. **The diff engine is per-step by construction.** 13a to 13e compute
   what changed between two adjacent program versions and classify each
   change. A diff across five versions is not a thing the engine
   produces, and synthesising one would mean a second classifier
   disagreeing with the first.
2. **A step that cannot be carried has to name itself.** Walking
   step by step, the refusal is *"version 7 to 6 drops `x` and nothing
   restores it"*. Computed end to end, the refusal can only say the ends
   do not match, which tells an operator nothing about where to stop.
3. **Downgrade is the direction that loses**, and it loses at a specific
   step. A field added at version 5 has no values before version 5, so
   coming down through that step drops it. Naming the step is what lets
   an application decide whether that step is acceptable before it runs.

**What this obliges that is not built.** The diff engine plans forward.
A step-by-step downgrade needs each step to state its own down path, or
to state that it has none, which is a property no artifact carries today.
That is 11-10b's real work and it is larger than lifting the version
check.

## Found while building the blob chain's last increment (2026-09-21)

C7 finished `plan-blob-chain.md`, and three of its findings are worth
keeping because they are one mistake made three times: **the plan
described revert-time behaviour and did not notice that an offline audit
walk cannot inherit it.**

1. **A digest that is recorded and will not open was not in the
   taxonomy.** §4.3 names four outcomes, and at revert time a fifth
   cannot arise, because `resolve_sealed` refuses the whole image before
   the comparison is reached. For a walk over the entire chain that
   all-or-nothing posture is wrong: hard-stopping over one unrelated
   sealed column on some other row, or over a digest whose epoch was
   crypto-erased, is an audit tool that stops auditing at the first
   erased tenant. It is bucketed under "nothing was compared" rather than
   given a fifth outcome, and presence is now tested against the raw
   image rather than the resolved plaintext so that *never recorded* and
   *recorded and unreadable* stay distinguishable.
2. **Nobody had said which images the walk reads.** A revert checks only
   the image it is about to write, because that is the only thing it
   acts on. An audit has no such restriction, so the walk checks `before`
   and `after`, and the cost that follows is now written down: a handle
   that is one command's `after` and the next command's `before` is read
   twice.
3. **The mode needs a program and real key custody, and the plan said
   neither.** Both follow from decisions already taken, since §4.1
   refuses a plaintext digest and only the schema says which columns are
   blob columns. The signal was a comment in `revert` rather than
   anything in the plan.

**The general lesson, stated because it will recur.** Every one of these
came from generalising a mechanism from the path that writes to the path
that only reads. A writer may refuse everything when one thing is wrong,
because refusing costs the caller a retry. A reader that refuses
everything when one thing is wrong reports nothing about the parts that
are fine, which is the opposite of what a reader is for. The next tool
that walks the chain offline should be checked against this before it is
designed, not after.

## The upload tombstone, and why the plan's version deleted live bytes (2026-09-22)

`plan-tenant-storage.md` §5 B-F3 specified U2 as: the door writes a
`$blobtomb` intent row before the `put`, and phase 2's re-check under the
row lock finds the live reference and cancels the intent when the upload
commits. The first half is right. **The second half loses data**, and the
loss is silent.

**Two properties of the existing sweep, both verified in the code rather
than assumed:**

1. `mark` inserts a tombstone only `AND NOT EXISTS (SELECT 1 FROM
   $blobtomb t WHERE t.$space = b.$space AND t.ref = b.ref)`. A handle
   that already has a tombstone row is skipped.
2. The sweep selects `WHERE deleted_at IS NULL AND unreferenced_since <
   cutoff`. A tombstone that is not yet due is never examined and never
   re-stamped.

**Together they price a deletion against the upload's clock.** An intent
stamped at `t0` survives a committed upload, because phase 2 only ever
looks at due tombstones and this one is not due. Ninety-one days later
the row is deleted: phase 1 skips the handle, because a tombstone already
exists. The sweep then finds a tombstone stamped `t0`, which is well past
the cutoff, and deletes the object, while the deletion is seconds old and
revertible for another ninety days.

That is not an edge case. **Every upload whose row is deleted inside the
undo horizon would lose its bytes early**, which turns a rare leak into
the common path. Demonstrated by
`a_delete_long_after_an_upload_is_floored_from_the_delete`, which reports
`deleted: 1` against the plan's design and `0` against what was built,
and which was run both ways rather than reasoned about.

**What was built instead.** The door still declares the intent before the
`put`, and the `$blobref` insert cancels it **in the same statement**, as
a leading data-modifying CTE. An intent and a committed reference for one
handle cannot coexist, so there is no window to reason about and none to
crash inside. Phase 2's re-check is unchanged and becomes the backstop
for reverts alone.

**The floor argument holds for a different reason than it does for a
mark-phase tombstone, and the difference is worth keeping.** The retained
image rule turns on *"any image that could restore the handle is at or
before `unreferenced_since`"*, which is plainly false for an intent,
since the converge group is appended afterwards. What rescues it: the
group and the `$blobref` row commit in one transaction, so either both
landed (and the intent did not) or neither did, and in the second case no
retained image anywhere names the handle. A surviving intent is a handle
nothing can ever refer to, which makes the floor pure conservatism there.
It is applied anyway, because one rule is worth more than a saved day.

**The general lesson, which is the same one C7 produced.** A mechanism
designed for one phase was assumed to serve another. `mark`'s
skip-if-tombstoned and the sweep's due-filter are both correct for the
lifecycle they were written for, and neither is correct once a second
writer puts rows in that table with a different meaning. The question to
ask of any shared table is not "does this still work" but "what does this
column mean now that two writers disagree about it".

## The stale tombstone clock, found twice and fixed once (2026-09-23)

U2's build found that the plan's own upload-intent design deleted live
bytes. Closing that surfaced the **same defect in code that has nothing
to do with intents**, reachable today by an ordinary sequence:

1. A row holding a blob is deleted. Phase 1 tombstones the handle at `t`.
2. The deletion is reverted inside the undo horizon, so a live row names
   the handle again. The tombstone stays **open**, because phase 2 only
   examines tombstones that are **due** and this one is not.
3. The row is deleted again, much later. Phase 1's `INSERT` skips the
   handle, because a tombstone already exists for it.
4. The sweep finds a tombstone stamped `t`, long past the cutoff, and
   deletes the object. The second deletion is days old and revertible for
   the rest of the horizon.

**Measured rather than reasoned about**: with the fix disabled the sweep
reports `deleted: 1` for an object whose second deletion is twenty days
old and stays revertible for seventy more.

**Phase 2's re-check does not save it**, and that is the part worth
keeping. The re-check runs only once a tombstone is due, and by then the
handle is genuinely unreferenced again, so the re-check *agrees* with the
deletion. Every individual step is correct. The defect lives entirely in
a timestamp that describes one deletion while being used to price
another, and no single point of inspection sees anything wrong.

**The fix is an eager cancel in phase 1**: before marking, delete open
tombstones whose handle a live row names again. The next deletion then
inserts a fresh row with its own clock. Cancelling rather than
re-stamping is deliberate, so that `unreferenced_since` keeps meaning
*the moment this handle stopped being referenced* rather than spanning
two unrelated lifetimes. Closed tombstones are left alone: a closed row
is the record that an object was deleted, which is phase 2's
crash-recovery evidence and not a claim about the present.

**It moved a counter, and the counter had to follow.** An existing test
asserted that a revert cancels a tombstone by reading phase 2's
`cancelled`. With the eager cancel the tombstone is still cancelled, one
phase earlier, so that number went to zero while the behaviour improved.
Phase 1's cancels now join phase 2's in one `cancelled`, because an
operator counting cancelled tombstones is counting **reverts**, and which
phase noticed is the module's business rather than theirs. A test that
fails because a fix moved where something is counted is a test doing its
job; the wrong response would have been to relax it.

**Two lessons, and the second is the reusable one.** A shared table with
two writers needs its columns asked not *does this still work* but *what
does this mean now that two writers disagree about it*. And a guard of
the form "skip if a record already exists" silently inherits whatever
that record says, including a timestamp from an era that has ended.

---

## Where each log lives, confirmed: 18d · S4 (2026-09-13)

Owner, confirming the split `plan-space-routing.md` recommended: *"Control
plane / data plane splot makes sense."*

**What it confirms.** A space's own security stream, its `$commandlog` and
their checkpoints, and the blob tables live in that space's database,
because they travel with the data on export and die with it on erasure.
The deployment-scope stream — identity, access, grants, denials by
non-members, the cross-space sweep — stays in the control database,
because it is a fact about more than one tenant and has to be writable
before a named space is known to exist at all. This is the split S4 built
the same day, 2026-09-13: the drain sends deployment-scope entries to the
control database without asking the resolver, and each space's entries to
its own database, with `$jobs` owed by every target rather than split by
DDL set. Nothing about the build changed as a result of this row; it is
the board's formal record of a confirmation the build had already acted
on informally (2026-09-12: "both makes sense").

## Four more rulings on per-tenant databases (18d, 2026-09-21)

Owner, answering the `plan-space-routing.md` §11 rows the tenancy-topology
ruling opened, the same day as the thirteen rulings recorded above.

**D-C, the reserve stays unconditional and configurable** (`db-reserve`):
*"ill take your recomendation but configurable makes more sense to me."*
**What it settles.** 20a's connection reserve does not resize itself
against how many tenants currently share a target — a live resize under
running checkouts is a real hazard bought for a configuration default. It
is set once per target, and an operator with a database holding a single
tenant configures whatever reserve they want rather than the platform
inferring zero is safe.

**D-E, an allowlist for the database hosts a deployment will dial**
(`db-allowlist`): *"theres no reason it cant still be configured. per
tenant confgurations are still configurations - configured at the
deployment level. egress filtering usually belongs on the infra level, but
we can just add a easy allow list."* **What it settles.** An attach-time
allowlist ships for v1: a host that came from data (a tenant's own
database or bucket target) must be on a list the deployment's operator
configured, or the attach is refused before any dial, with one refusal
reason only — `ordo-egress`'s own rule that a distinguishable reason is an
oracle applies here too. **D-S6 asked the same question for bucket
endpoints and the owner tied the two together**, so this did not end as
two allowlists; see D-S6 below for the mechanism.

**D-F, the attachment does not go in the tenant's own chain**
(`db-chain-attach`): *"theres no provability unless its byo db - they can
see where there data is. I think this belongs to the app not ordo."* This
reverses the plan's own recommendation, which was to record the attachment
in the tenant's founding group so their own export would carry it
(`plan-space-routing.md` R-X4). **What it cancels.** R-X4's founding-group
half was never built: `plan-space-routing.md` still lists
`the_attachment_fact_is_in_the_tenants_founding_group` as a done-when that
does not exist, and `bucket_ops.rs` carries the identical ruling on the
storage side ("belongs to the app not ordo... there is no founding-group
entry"). **What carries the fact instead.** The deployment's own security
stream, as a custody action recording the attachment — which is where
Q-3's ruling (2026-09-13, above) already put it, since the system chooses
the target at provisioning and the deployment's stream is the one place
writable before the space exists to have a chain of its own. A tenant who
genuinely holds their own database can already see where their data is
without Ordo telling them; one who doesn't isn't owed a claim Ordo can't
make honestly.

**D-H, credentials are KMS first with environment or file optional**
(`db-credential-provider`): *"KMS with optional envs - configurable will
allow easier deployment but only if it makes sense. otherise kms only"*
This moves off the plan's directory-only recommendation for v1. **What it
left open.** Which KMS — the deployment already reads its own secrets from
GCP Secret Manager in the cluster, and the master key already opens from a
sealed file, so a KMS-only rule for tenant credentials while the master
key still opens from a file would have been an inconsistency worth
settling rather than leaving implicit. **Resolved the same day, by
dissolving the question rather than answering it** — see "Erasure proof,
and a secret provider the service does not know about," above: the
provider reads from a mount or an environment, and whatever fills that
mount — a KMS, a secret operator, a human with a file — is above the
deployment and invisible to it, the same rule the console's own storage
already follows.

## D-D settled: one tenant, one database, broader than the question asked (2026-09-25)

Owner, answering `plan-space-routing.md` §11 (`db-occupancy-disclosure`,
18d · D-D): *"1 tenant 1 database. same for buckets. database/schema not
cluster."*

**The answer is broader than the question.** 18d · D-D asked whether a
second tenant inside a database someone had *declared* their own
perimeter is refused. The owner answered the general rule instead: a
database holds one tenant, full stop, and which side (if either) had
called it "their own perimeter" does not enter into it.

**What was built for the narrow reading collapsed into the general one.**
Two refusals existed — a second tenant arriving into a database already
claimed as someone's own perimeter, and an incoming own-perimeter claim
over a database someone else already occupied — distinguished by which
side declared `OwnPerimeter`. Both directions end in the same state, one
database holding two tenants with one of them told the perimeter is
theirs, and arrival order was never a difference worth two outcomes. They
collapsed into one refusal, `ProvisionRefusal::DatabaseOccupied { space,
holder }`, which does not consult `DbCustody` at all.

**A test was inverted rather than deleted, because the behaviour it named
is the behaviour that changed.** `two_platform_managed_tenants_share_one_database`
had asserted the shared tier worked, and it was not wrong about the code
it tested — R-X1 describes that tier in as many words, "a hundred tenants
attached to one database share one pool and one credential." The ruling
removes sharing from the *attach* path specifically, so the test became
`two_tenants_do_not_share_one_database_whatever_their_custody`, and it
also checks that a **second database on the same server** still works —
without that half, the test would pass equally if attaching had simply
stopped working at all.

**"database/schema not cluster" is the half that keeps this affordable.**
Many tenants on one Postgres server stays the ordinary case; only the
database may not be shared. `ORDO_PG_ATTACH_HOSTS` stays a list of hosts
rather than a list of tenants, and a tenant still costs a database rather
than a server.

**"Same for buckets" was read as bucket-not-endpoint, and that reading is
an implementer's judgement the owner has not separately confirmed.** U6
built the storage side to refuse a shared bucket with a per-tenant
`keyPrefix`, on the argument that a Postgres schema is a boundary the
*server* draws through its own privilege system (`GRANT`), while an S3 key
prefix is enforced only by a policy on the tenant's own side that D-S7
already keeps the platform's credential from being able to check by
listing — a prefix says where an object is named, and grants or withholds
nothing on its own. The argument is sound and is why `bucket::occupants`
keys on the endpoint host and the bucket rather than on `keyPrefix`, but it
is a reading of "same for buckets," not a sentence the owner wrote, and it
is worth flagging back to him as a place he could still narrow or widen.

**The ruling does not reach the default path.** A tenant created without
`$db` still shares the control database with every other unattached
tenant; that gap is its own board row, `db-default-tier`, raised rather
than answered, because closing it means provisioning a database at
creation for every tenant and touches the cost model, the usage ledger,
and both T9 and T10.

**Occupancy stays a fact, not a sentence.** `spacedb::occupants` is
unchanged in kind — it still enforces the refusal and still feeds R-S2's
fleet view — it just cannot return more than one attached tenant for a
database any longer, which is what makes the ambiguity the original
disclosure ruling worried about (a consent page printing "your own
database" to someone sharing one) structurally unable to arise.
`DbCustody::disclosure` itself is untouched.

## Nine rulings on per-tenant buckets (18p, 2026-09-21)

Owner, answering `plan-tenant-storage.md`'s board rows, filed the same day
as the thirteen database-and-bucket rulings recorded above.

**18p, the plan's requirements and four found defects** (`tenant-storage-req`):
*"recomendation + above."* **What it approves.** The storage side is built
the way the database side was: as libraries, with the binary wiring only
the single-store variant, and the plan's four found defects are accepted
as found — nothing recording which store an object went to; the attach op
returning a credential label to a tenant; `S3Config` deriving `Debug` over
its access key and secret (fixed 2026-09-21, with a test); and plaintext
HTTP admitted to a fixed local list. **"+ above"** folds in every D-S*
ruling below, plus D-D and D-F, which the storage plan shares with the
database plan.

**D-S1, two switches, one per tier** (`st-two-switches`): *"recomendation."*
A single combined switch would make the tier that's ready (database) wait
on the one that isn't (storage, which needed a resolver in the blob
pipeline before anything could route). All four combinations — own
database and shared bucket, and the reverse — are real configurations, and
the cost of two switches is one more fact in the disclosure, which
occupancy already answers by making both facts visible in one place.

**D-S2, attaching a bucket that already holds objects is refused, with a
named exception not yet built** (`st-attach-existing`): *"yes unless
everything matches i.e. a 1 for 1 migration would be indistubgushable."*
**Built 2026-09-24.** A space that already holds `$blobref` rows cannot be
pointed at a new bucket, because nothing records which store an object
went to, so the sweep could delete against the wrong bucket, succeed (a
missing key deletes without error), retire the reference, and strand the
bytes with erasure then reporting a deletion that never reached them. A
retry of an attachment that is already standing is not refused by this.
**Not built: the exception the owner named.** The store interface
deliberately has no listing (D-S7), so the only way to prove a one-for-one
migration is one existence check per `$blobref` row against the new
bucket — bounded by the reference count, but unspecified for what happens
on a partial match, since most objects present with a few missing is
neither an empty space nor a clean migration. Verifying from the
references rather than from the bucket is the shape that satisfies both
this exception and D-S7's no-listing rule; it is recorded here but not
implemented.

**D-S3, one sweep cadence for the whole deployment** (`st-gc-cadence`):
*"recomendation."* Closes 17g's cadence question, open since the sweep
itself was built: a cadence per attachment would be many schedules to
reason about, and would let a tenant defer their own deletions past
whatever retention their own disclosure promised. What genuinely varies
per tenant is the cost of a sweep, which the existing floor and batch size
already bound.

**D-S4, bucket credentials are a key pair only in v1** (`st-roles`): *"yes
for v1 add enhancement for post deploy roadmap."* An IAM role would add a
token call, another header in the signature, and a credential that can
expire mid-request; all three live inside the S3 binding, so nothing about
the attachment table, the resolver, or the audit trail changes when a role
is added later. Filed as a post-deployment roadmap enhancement rather than
v1 scope, as asked.

**D-S5, erasure proof — settled** (`st-delete-objects`): *"erasure proof
belongs to the app not ordo byt we can automatically collect something for
the audit log."* Confirmed 2026-09-25 when the contradiction below was put
to the owner: *"This was settled. We don't own that contractual relation
between app dev and end user."*

**A correction to what T10 wrote here.** This entry first recorded the
ruling as **open, not settled**, because `docs/decisions.json` carried the
row's status as `discuss` while the board row sat under *Ruled*. T10
declined to pick between the two records and wrote down the more cautious
one. The cautious reading was the wrong one: the flag was stale, and the
question had been answered. Recorded rather than quietly edited, because
the useful part is the rule it produced — **a status flag is not a
ruling**, and where a flag and a written answer disagree, the answer is
what was said.

What it settles: the proof that erasure happened belongs to the
**application's** relationship with its own user, not to Ordo. Ordo
collects the fact for the audit log and does not own the contract. From
the fuller exchange the same day ("Erasure proof, and a secret provider
the service does not know about," above): under a tenant-owned perimeter,
erasure destroys the keys and leaves the objects, writing what it left
into the chain as a custody action. What *"we can automatically collect
something for the audit log"* is meant to become — the signed erasure
receipt an application can hand to its own user for offline, zero-trust
verification — is open build work (`17ab`) and is not built.

**D-S6, an allowlist for bucket endpoints, tied to the database allowlist
as asked** (`st-endpoint-allowlist`): *"yes but should be tied in with the
db allowlist setup. implication really is if i defined it on my platform
it is allowed."* **Built 2026-09-24, as one mechanism with D-E rather than
two.** `provision::HostAllowlist::from_env` reads both
`ORDO_TENANT_ATTACH_HOSTS` and `ORDO_PG_ATTACH_HOSTS`, comma separated,
and **unions** them; both the database tier (`prepare_target`) and the
bucket tier (`bucket_ops::BucketOps::attach`) check the same list through
the same type. **Union, not rename and not precedence — both alternatives
were considered and rejected.** A rename was the first instinct:
`ORDO_PG_ATTACH_HOSTS` is already on the decisions board as "one new
setting an operator must know about," so renaming it would make a board
entry the owner has read false. A precedence is worse: an operator who set
the new variable would silently turn the old one off, and the hosts that
vanished would be the ones their existing database tenants are already
attached to. A union only ever grows, which is the one direction safe to
do behind an operator's back. The loopback exception moved behind its own
dev flag, `ORDO_DEV_UNSAFE_BUCKET_ENDPOINT`, kept separate from the
store's own dev flag because the two unsafe different things.

**D-S7, publish the least-privilege bucket policy, with no list
permission** (`st-bucket-policy`): *"yes."* Turns a leaked attachment
credential into one that cannot even enumerate the tenant's own objects,
and shapes the attach-time probe into a `head` request rather than a
`list`. It is also the fact D-D's storage reading leans on: the platform's
credential cannot enforce a key-prefix boundary because it is explicitly
forbidden from listing the bucket it would need to enforce one against.

**D-S8, a second tenant inside a bucket one tenant was told was theirs is
refused** (`st-shared-own-bucket`): *"agreed."* **Built 2026-09-24, and
widened the next day.** The question asked was narrow — a second tenant
inside a bucket *declared* as one tenant's own — but D-D's later general
ruling ("1 tenant 1 database. same for buckets," above) makes it
unconditional: a bucket holds one tenant regardless of which side's
custody was declared, and `bucket::occupants` keys on the endpoint host
and the bucket, deliberately not on `keyPrefix`. Unlike the database side,
which built the narrow reading first and had to collapse it, the bucket
side built the general rule from the start. What this does not touch: a
tenant who attaches nothing still shares the deployment's own default
bucket, separated only by the `<space>/` key prefix — the exact twin of
`db-default-tier`, and not settled either way here.

## Every tenant gets a database, and the setting picks the cluster (2026-09-25)

Owner, answering the board row `db-default-tier`, which was raised after D-D
left the default path untouched: *"okay lets reframe this - there exist a
default cluster which databases are created in. 1 tenant one database - in
addition a tenant COULD have their own BYO DB. But the settings are simply to
decide what DB cluster to store the database in. Either whatever the
deployment provides as default, or what the deployment provides for that
tenant."*

**What it settles.** Every tenant has a database of its own. The question put
to the owner offered two readings — one database per tenant everywhere, or one
only where a tenant was given one — and the answer took neither, because the
question had the wrong shape. It assumed a tenant might or might not be
*given* a database. What the routing setting chooses is the **cluster** the
tenant's database is created in, never whether there is one.

**Four clusters, clarified the same day:** *"It doesnt neccesarily need to be
the control plane database. The console has its own it can define, the
deplyment can define its own, it can define one as a default for tenants, and
it can define individually per tenant."*

1. The console's own, which belongs to the console rather than to any
   deployment it manages. `18o` already puts the console and the operator
   outside the deployment.
2. The deployment's own: the control plane, holding identity, grants,
   `$artifacts`, `$spacedb` and the witness.
3. A default cluster for tenants, where a tenant's database is created when
   nothing else is said.
4. A per-tenant cluster, which is the BYO case.

**They are configuration levels and not necessarily separate servers:**
*"someone deploying could use the same cluster for all 3 if they chose."* Each
level therefore defaults to the one above it rather than being required, and a
deployment that configures nothing runs the console, the control plane and
every tenant database on one cluster. The ordinary small deployment must stay
a no-op to set up.

**How this squares with D-D.** D-D's *"database/schema not cluster"* and this
ruling say the same thing from two directions: the **cluster** may be shared,
the **database** may not. `provision::same_database` already compares host,
port *and* database, so a tenant database sitting beside the control database
on one server passes while the control database itself is refused. That check
needs no change.

**What it changes in the build.** T8 shipped `$db` as *name the database this
tenant's rows go in*, with `$dbHost`, `$dbUser`, `$dbCredential` and
`$dbOwnPerimeter` beside it, refusing the call unless every one was given.
Under the ruling the database is neither optional nor the caller's to name, so
the parameter becomes which cluster and the database name is minted the way
the space id already is. Level 3 is the setting the build does not have:
`runtime.rs` hands `TenantOps` a `PoolConfig::from_env()` template, which is
the control database's own connection, so a tenant with no cluster named
currently lands on the control plane's cluster by inheritance rather than by
configuration. Queued as `db-always-a-database`.

**What survives untouched:** one tenant per database, the control database
refused as a target, the host allowlist, the routing lock, the fleet view and
the custody record. None of them depend on who chose the name. **What gets
heavier:** the reaper and the usage ledger, because a thousand trial tenants
is a thousand databases before any of them writes a row.

**The storage twin, settled the same day.** See the entry below.

## One tenant one bucket, where a bucket answers to a cluster (2026-09-25)

Owner, answering `st-default-tier`: *"one tenant one bucket. Same principal. I
dont know how that got lost."* Then, clarifying what the principle means:
*"1 bucket can have folders so 1 bucket defined per - but it could be the same
and bucketname/tenant would be the option. The deployment can also have its
own bucket defined."*

**What it settles.** The storage tier mirrors the database tier one level
lower than it had been read. A **bucket** is the analogue of a **cluster** and
may be shared; a **folder** inside it is the analogue of a **database** and is
what one tenant has to itself. Every tenant has a bucket defined for it, and
that may be the same physical bucket another tenant uses, separated by
`bucketname/tenant`. The four levels are the database tier's: the console's
own bucket, the deployment's own, a default for tenants, and a per-tenant
bucket, each defaulting to the one above.

**The layout already did this.** `ordo_blob::object_key` keys every object
`<space>/<handle>`, so two tenants in one bucket could never have collided
whatever prefix either stated, and `key_prefix` is an additional prefix on top
of that. Nothing about the object layout changed.

**What it corrects.** D-D says *"same for buckets"*, and this session read
that twice as something narrower or wider than it is. U6 was told to build the
general rule and did: a second tenant was refused from a bucket whatever
prefix they stated, on the argument that an S3 prefix grants nothing because
it is enforced by an IAM policy the platform cannot set. That argument is true
and answers a question about **collision**, which `object_key` had already
answered. The refusal is reversed.

**What survives is D-S8**, ruled *"agreed"* on 2026-09-21, which asks a
different question: a bucket declared as one tenant's **own perimeter** is
theirs, and the platform is not the party who may put somebody else inside it.
A folder does not answer that, because it is a disclosure rather than a
collision. `bucket_ops` now refuses in that case alone, in both directions —
a tenant arriving into someone's declared perimeter, and a tenant claiming a
perimeter over a bucket somebody is already in.

**How it got lost, recorded because the pattern matters more than the
instance.** The owner's *"same for buckets"* was scoped to the attach path
rather than read as the parallel principle, and the part scoped off was then
put back as a fresh question. The rule it argues for: when a ruling states a
principle, apply it everywhere it reaches before raising a follow-up about the
part left out.

## Nothing has deployed, so nothing is owed a migration (2026-09-25)

Owner, closing `db-always-a-database`, which asked what should happen to
tenants already sitting in a running deployment's control database:
*"left alone or moved under what circumstance? Nothing has deployed in terms
of ordo so we make - expectation of recoverability for anything at the
moment."*

**What it settles, and it reaches much further than the question.** There are
no deployments in the field. So a change to the shape of anything — a table,
a column, where a tenant's rows live, what an op's parameters mean — owes **no
migration and no recovery path**, because there is no state anywhere that a
path would recover. `db-always-a-database` closed on this: the question
assumed tenants existing in deployed control databases, and there are none.

**What this does not license.** It is about *existing deployed state*, not
about care. The command log's hash chain, `$migrations`, the artifact version
and the witness all still have to be correct, because they are what a
deployment will rely on the day one exists. A shape change that made a chain
unverifiable would be wrong today for the same reason it is wrong later.

**What flips it.** The first real deployment. From that moment a shape change
owes a path from the shape before it, and the question
`db-always-a-database` asked becomes live rather than moot. That is worth
knowing in advance, because the cheap window for changing a decision closes
without anybody sending a notice.

**Recorded because it was asked the wrong way round.** The question was put to
the owner twice — once badly enough to draw *"I don't understand the ask"* —
before anybody checked whether the situation it described could exist. The
rule it argues for: before asking what should happen to existing state,
establish that any exists.

## A dot-path guard nulls the field, it does not drop the row (GC007, 2026-09-26)

**The gap first, because it is a real one.** When an op reads through a
dot-path — `task.project.name` — the target entity's own read guard is
**never evaluated**. `ordo-sema`'s `entity_source` pushes the base entity's
guard into the query's filters; `walk_refs`, which handles each dot-path hop,
pushes a bare `Join`, and the `Join` IR carries no field for a guard at all.
So `tasks`' guard runs and `projects`' does not, which is the opposite of what
`guards-conformance.md` R-G6 says in as many words: *"every entity the
flattened plan reads contributes its read guard as a conjunctive semi-join,
dot-path targets included."*

GC007 has recorded this divergence since the corpus was written. The fixture
hides it because the test caller happens to satisfy both guards — which is the
corpus doing exactly its job, and the reason a case asserted *as the
implementation behaves* fails the moment anyone fixes the code.

**The question, which is not an implementation detail.** A dot-path is a
**LEFT JOIN**. Putting the target's guard in `WHERE`, as the base entity's is,
turns it into an inner join in effect: a task whose project you may not see
**disappears from the result entirely**. Putting the same condition in the
join's `ON` clause leaves the task standing with a **null** project name.

**Ruled: the guard goes in the `ON` clause. The field comes back null and the
row stays.** The owner, 2026-09-26: *"take reccomendaton on dotpath."*

The rule it keeps is the one the rest of Ordo already follows: **a guard
decides what you may read, not whether a row you already may read exists.**
The caller passed `tasks`' guard, so that task is theirs to see; a guard on a
*different* entity reaching back to delete it from their list is that entity's
guard deciding something outside its own scope.

It is also the answer that fails visibly. A blank field is a question a user
asks support about. A row that silently is not there is a bug report about lost
data, and the support cost lands on an application author who did nothing
wrong.

**The cost, recorded rather than glossed.** A null is an existence disclosure:
a blank project name tells the caller a project is there that they cannot see,
and with a sharp enough query that is countable. That is real, and it is the
same disclosure the platform already accepts elsewhere — a space id is treated
as leakable, which is why grants are space-bound. Existence is not the secret;
contents are.

**What implementing it owes**, none of it optional:

1. The guard compiled into the **join condition**, not `q.filters`. The `Join`
   IR gains a guard, because it currently cannot express one.
2. `taskSheet`'s scope manifest changes: `projects.owner` becomes a genuine
   guard read and must appear. The GC007 compile-half assertions move with it.
3. **The falsifying case the fixture has never had**: a caller who satisfies
   the task guard and fails the project guard, proving the stacking filters the
   *field* and keeps the *row*. Without it the fix is unverified, which is the
   state GC007 is a record of.
4. R-G8's `unguarded` bypass applies to dot-path targets as it does to the op's
   own source — a dot-path is part of the op's own body, which is what both
   R-G6 and R-G8 already say.
5. The catalog should name which entities a dot-path pulls a guard from, so an
   author reading their own op can see that `taskSheet` now depends on
   `projects`' guard.


---

## 2026-09-28 · Seven rulings after the round-6 security review

Answered in `decisions.json` the evening the review landed. Recorded with
their reasons, because three of them were answers to questions the owner
asked back.

### 1 · A row the compiler brings in is guarded like a row the author listed

*"Fix (see if there are any other places where a similar issue could come
up, as it seems something similar was previously found already)."*

The instruction to look further is the ruling's substance. The defect had
been found once (GC007, the dot-hop) and treated as one site. It is a
**class**: any row a query reads that its author did not list. Three
members were found (the dot-hop, the inferred bridge, the whole-read's
signpost) and one decision now covers all of them. The rule for the next
compiler-added node is that it is not finished until it calls the same
decision.

### 2 · A duplicate unique key in a bundle is refused

*"Refuse."* A loader for somebody else's data that cannot reject a duplicate
loses rows quietly. Built with `std_ops`' create-once belief under
`std_ops`' own condition.

### 3 · `ORDO_KEY_PROVIDER=local` refuses outside development

*"Take recommendation."* The sealed-master path exists and is the one the
operator guide uses, so the environment-variable path is a laptop
convenience and says so by not starting anywhere else.

### 4 · CORS is an allowlist

Ruled with two requirements attached: documented so it is not missed, and
configurable from the console. See checklist `14g`.

### 5 · The first deployed program is a notes app with passkey sign-in

See checklist `17ae` for the two items the choice makes blocking.

### 6 · A retired listener is deregistered by an operator command

*"Take Recommendation."* `ordo triggers deregister <listener> --as <who>`,
through the door, because the act destroys a guarantee and must record who
did it.

### 7 · Suspension is to be built to match the specification

*"I don't know what a member is here, but yes this should be fixed and meet
to match spec."* The specification is `definition-language.md`'s `members`
table, where `status` is `active` / `suspended` and *"suspension is a
command, not a delete"*. What remains the owner's is **how fresh** the door
must be, which is `sec6-membership-snapshot` on the board.

### Not ruled: binding a sealed value to its row

The owner's objection was cost at run time. The answer is on the board row:
the change lengthens the associated data the cipher already authenticates
by about thirty bytes and adds no query, no storage and no round trip. The
expensive half of the original proposal, verifying a stored file's digest on
every read, is withdrawn as unnecessary once the handle is bound.


---

## 2026-09-29 · Who owns a credential, and who decides who gets in

Given by the owner in conversation, after the finding that the served
authentication could not be used (checklist 20b). Six principles, recorded
with the sentences they came from, because `plan-enrolment.md`,
`plan-auth-federated.md` and `plan-passkey-recovery.md` are all built on
them.

**P1 · Credentials and identities belong to the deployment.** *"Service owns
the credentialing."* A tenant never stores one.

**P2 · Admission to a tenant belongs to the tenant.** Whether a tenant is
open, invitation-only or fed by an outside provider is that tenant's choice,
under a ceiling the deployment sets. This changes `ORDO_AUTH_PROVISIONING`
from the policy into the ceiling.

**P3 · A sign-in is an enrolment by default.** *"Each new sign in is an
enrollment (auto enrollment) unless they want to manually provision and
invite."* Anything stricter is a choice somebody made.

**P4 · No flow is compulsory.** *"It does not make sense to enforce a
particular flow, in that way it's not easy to use and therefore makes a bad
app."* And: *"Invitation does make sense, but it should not stop signups
from existing if a tenant chooses it."* This overrules the recommendation on
`auth-bootstrap`, which proposed a one-time code as the only way the first
person registers.

**P5 · A tenant has power over credentials it created, and every use is
recorded.** *"Before the user exists the tenant creates that credential for
the user, then the user can own it. If the user needs to reset a password
that's a little different but the tenant should have the power for it and
should be logged appropriately."*

**P6 · One person, one identity, many tenants.** *"Credentialing can't
explicitly stay scoped to just a tenant but needs some control or management
from the system above."*

**Where P5 and P6 collide, and how the plan settles it.** A credential
unlocks every tenant its holder belongs to, so one tenant resetting it
reaches into the others. `plan-enrolment.md` §4.2 settles it by who created
the identity: a tenant controls the credential of an identity it created,
for as long as that tenant is the only place the identity is used, and loses
that control, on the record, the moment the person is admitted anywhere
else. That is a proposal and is on the board for the owner to confirm.

**First come, first served** is ruled sensible for a single-tenant
deployment, with *"something more"* wanted for multi-user settings. The plan
proposes an optional claim code for a deployment on a public address.

## 2026-09-29 · Three rulings the board audit found unrecorded here

The audit of the board (`audit-board-2026-09-29.md`) found three rulings
that were on the board and missing from this record.

**The bootstrap inventory's items close when their blockers do**
(2026-09-26, `bootstrap-inventory-split`). The owner: *"They'll close after
their blockers complete."* So 11-15 stays open past the provisioning
lifecycle, and no item of the 46 is moved elsewhere to make it closable.

**An import takes a people map** (2026-09-26, `platform-ref-import`). Rows
that name a person are imported through a map the operator supplies, and an
import refuses any name the map does not cover. The owner took the
recommendation. The mechanism is in `plan-import.md`.

**The board's files stay its store** (2026-09-29, `board-store`). Left to
Claude by the owner: *"okay then proceed as you see fit."* `docs/board.jsonl`
and `docs/board-answers.jsonl` are the record; the database holds vectors
and locking and is rebuilt from the files.

## 2026-09-29 · The owner's answers on sign-in, enrolment, recovery and the audit

Given on the board, one answer per ask. The owner's words are in
`docs/board-answers.jsonl`. Where an answer moves off the recommendation it
is marked.

### Enrolment (`enrolment-questions`, all five)

1. **A tenant resets a password only for a person it created who belongs
   nowhere else.** *"Agreed."* This confirms `plan-enrolment.md` §4.2.
2. **A stranger admitted by an open tenant is a viewer by default**, and the
   app developer may change the default: *"This is app dev territory but
   default should be viewer."*
3. **The first-person claim code is a setting**, reachable three ways:
   *"This should be managed via console / api / envs."* The owner did not
   name a default. It stays off by default, as recommended, until he says
   otherwise.
4. **A typed temporary password is a recorded option, off by default.**
   Recommendation taken.
5. **When a tenant is deleted, the people it created become independent if
   they ever signed in, and are removed if they never did.** Recommendation
   taken, with one addition from the owner: people who came through the
   tenant's own registered provider are destroyed with it, and a platform
   sign-in remains and *"thats an app dev's decision"* what it then reaches.

### Sign-in through outside providers and email (`federated-auth-questions`, all six)

1. **Outside providers first, then emailed codes.**
2. **The app developer decides whether a provider's two-step claim counts
   as presence. Moves off the recommendation**, which was never. The owner:
   *"2FA from a third party is often a standard... If a company was using
   external auth, forcing internal auth in addition would be a nuisace."*
   The risk the recommendation named stays true and shapes the build: the
   setting belongs to the deployment, per provider, and a tenant that adds
   its own provider does not get to switch it on for itself.
3. **A registration is kept for a set time, then removed if no tenant
   admits the person. Moves off both plans.** The owner: *"it should be
   registration and they stay there for some x amount of time and then get
   nuked... this is really up to the app dev."* The time is a deployment
   setting.
4. **Mail goes by secure SMTP, and by API through Resend and the five
   largest API mail providers.** Wider than the recommendation, which was
   SMTP alone. Which five is not yet named.
5. **No automatic joining of accounts. Linking is an option a person
   chooses**: *"Existing account logs in and claims another account."*
6. **A removal at the company's provider is noticed when the session token
   expires**: *"if that's 12 hours then it is 12 hours."* Anything faster is
   post-deployment (18w).

### Passkey recovery (`passkey-recovery-questions`, four of five)

1. **A recovery code works at once.**
2. **A day's block on export after recovery, adjustable.**
3. Open. The owner asked who the operator is. It is the deployment owner,
   and the ask is put again in those words.
4. **A recovered person is not locked out of each tenant.**
5. **Email alone is not enough by default**, and the option is documented
   well, because *"someone may want it"*.

### The board audit (`audit-decisions`, all five)

1. **Tenants choosing between app versions is filed** (18u). This settles
   the disagreement between the owner's earlier answer and 20f's "not work
   at all".
2. **One connection reserve for the deployment.** A reserve per database is
   post-deployment (18v, which replaces 22e).
3. **A bucket that holds the tenant's own objects may be attached when every
   object matches a record** (22c, now decided).
4. **A bucket attach is not written to the tenant's security stream**, for
   the reason D-F gave for databases: disclosure belongs to the application
   (22k).
5. **Images are built for x64 only.** *"we can always add it later if it is
   requested."* This replaces cadence B of 2026-09-19. Done in
   `.github/workflows/images.yml`.

### SH-2, traced at the owner's word

Blind-index keys are derived from a space's own master key
(`crates/ordo-crypto/src/custody.rs`, `blind_index`), and each space is
given a random one at creation, so one value has a different digest in each
space. The consented feed that SH-2 describes is not built. No test compares
two spaces, and nothing yet tells the feed to write through the receiver
instead of copying digest columns. Both are owed as 22l.

### Later the same day: recovery closed, four security findings, and an amendment

**Passkey recovery, ask 3.** *"Yes the deployment owner."* The deployment
owner restores a tenant owner who has lost their only passkey, and nobody
inside the tenant does. All five recovery questions are now answered.

**Two-step claims, amended in conversation.** The owner: *"A tenant should
also decide what it is what they need for their tenant."* So there are two
levels. The app developer sets, per provider, whether a two-step claim
counts as presence, and that is the default. Each tenant then decides for
its own tenant. A tenant's choice reaches that tenant's data and no further:
acts on the deployment itself (deploying, deleting a tenant) follow the
deployment's setting whatever a tenant chose. That boundary is how the risk
the recommendation named is kept, and it is this document's reading of the
ruling, not the owner's words.

**The open security findings** (`security-trace`):

- **SH-18, tokens are not tied to a device: filed** for after the first
  deployment (18x). Short expiry and logout are the limits until then.
- **SH-12, the log shows who changed which record and when, in a space whose
  contents are sealed: accepted.** *"Agree."* Stated in `howto-custody.md`.
- **SH-7, data kept in the browser: checked at the owner's word.** Built:
  the replica is sealed on the device (`clients/ordo-sdk/src/replica/seal.ts`),
  kept per space and principal, purged and poisoned on revocation
  (`replica.ts`), and `std.space.*` is never copied (`sync.ts`). Owed:
  wrapping the device keys under a passkey, locking when idle, and a
  retention policy. Filed as 22m.
- **SH-11, the running server holds the master key: open.** The owner asked
  whether the fix is an outside key service like AWS KMS. It is, and the
  question of filing one is back with him.

**SH-11, the key service: ruled, and planned for after the first
deployment.** The owner: *"I thought we already worked around this - yes
easy deployment allows this to be the way to do it - but ideally it is
indeed in a key management service and we need to implement that. Make sure
it is not already implemented, if it is not then plan it for post-deploy."*
Checked: it is not implemented. The easy deployment, a key from a file or
the environment, stays. The plan is `plan-key-service.md` (18y).

One correction to what the board told the owner. It said that with a key
service "the key never leaves the service". That is true of the master key
under the plan's design B, and false under design A, where the service only
wraps a master key the server still holds in memory. `master.rs` had already
said so. The plan states both and recommends B.

### A second factor and backup codes (`second-factor`, all five)

Given in conversation, 2026-09-29.

1. **Hardware tokens: both** security keys and code-generating tokens.
2. **A second step applies to password sign-in and code sign-in**, and
   optionally to passkeys, as a person's setting or a tenant's requirement.
   Outside providers were not named, so they follow the two-level rule on
   provider two-step claims above.
3. **Hardware is offered without proof.** Proof (attestation) is added if
   people ask for it; the settled rule against verifying it stands until then.
4. **Who turns it on:** a person turns it on for themselves, a tenant can
   require it, and the app developer can enable it or require it.
5. **The same backup codes stand in for a lost second factor**, with the
   owner's condition: *"if theres an industry standard way to add more
   validity to the One time backup coes then lets add that"*, so that a code
   is not simply a leakable password. The plan (`plan-second-factor.md`)
   states which standard and how it applies.

**Build order, owner 2026-09-29:** row-bound seals and CORS in parallel;
the local key refusal once the seals land; passkey sign-in in parallel
with enrolment and the membership commands; recovery then the second
factor; the notes app built last and planned now.

### CORS: the wildcard is refused (built 2026-09-29, 14g)

Decided in the build, within the owner's ruling of 2026-09-28. `*` in
`ORDO_CORS_ORIGINS` refuses the boot, alone and not only with credentials.
Every Ordo call carries a bearer token in `Authorization`, so a wildcard
would let a script on any site use a token its page holds. A deployment that
wants many origins lists them, which keeps the choice visible in review.

### The local key provider, as built (21e, 2026-09-29)

Built to the ruling of 2026-09-28. The development signal is the existing
`ORDO_DEV_UNSAFE_TEST_KEYS`, reused so that there is one flag for unsafe
development custody. The `ordo keys` commands follow the same rule as the
server, because they mint the wrapped keys that reach backups: `ordo keys
init` under a short env master would create exactly the crackable rows the
ruling is about. A side effect, recorded: no command moves an env-wrapped
key store onto a sealed master, since `rewrap-master` takes only files.
Nothing has deployed, so none is owed; one can be added with a
`--from-env` option if a deployment ever needs it.

### Two sign-in details, in conversation (2026-09-29, `signin-details` asks 4 and 5)

**Managing members with a password is the app developer's call.** The
owner: *"THis is not ours to decide - that is for the developer."* So the
presence requirement on the membership and admission ops becomes a setting
the app developer holds. Its default is today's behaviour, a passkey
required; that default is this document's reading, not the owner's words,
and he can move it.

**Passkeys may be used on more than one address.** The owner: *"Passkeys
can potentially be used on more than one address - potentially."* Read as
the answer to ask 4: the addresses allowed to use passkeys are their own
setting, apart from the CORS allowlist, and hold several addresses. Today
`RelyingParty::new` accepts only the relying party's own domain and its
subdomains. An address on another domain needs WebAuthn Related Origin
Requests, where the relying party's domain publishes the list at
`/.well-known/webauthn`; the build supports it as an explicit option.

**Which acts need a passkey is app design, for every platform op.** The
owner, the same day, on the reading above that deploying and deleting would
keep requiring a passkey: *"What is enforced is app design ... not our
design."* So the presence requirement on every platform op (deploying,
deleting, exporting, importing, creating a tenant, the membership and
admission ops) is a setting the app developer holds. This overrules the
fixed rule in `security-logging-design.md` §2 that every platform op is
step-up gated. The platform still provides step-up and still decides how
presence is proved; the app developer decides where it is required. The
default stays today's behaviour, presence required on every platform op;
that default is this document's reading and the owner can move it.

### Sign-in details and the key service, answered on the board (2026-09-29)

- **A sign-up no tenant admits is kept 90 days by default**, configurable.
- **A person may see the list of their own tenants.** Nobody else sees it.
- **Mail:** SendGrid, Mailgun, Postmark, Brevo and Mailjet beside Resend, with
  SES after.
- The owner's comment on the task: *"We do not enforce an app developer's
  choices but we can have it as default."* This confirms the reading above:
  presence required is a default, and the setting is the app developer's.
- **The key service (18y):** both designs, B the default; GCP KMS, AWS KMS,
  Cloudflare if it offers one, HashiCorp Vault, and the large and popular
  self-hosted others; built self-hosted first, because most deployments will
  be a small Kubernetes cluster or docker-compose. Five minutes in memory,
  adjustable. `plan-key-service.md` §6.

**Mail, narrowed the same day:** *"Leave just resend, sendgrid as the main
ones and leave the rest for post deploy."* v1: secure SMTP, Resend, SendGrid.
The rest are 18z.

**Keys for the easy deployment, agreed in conversation.** The owner: *"Agree
with this setup - set bao for post deploy."* docker-compose first, then a
small cluster; the sealed key file by default; a cloud key service when the
machine runs in GCP or AWS; OpenBao in the compose file as an option after
the first deployment. Recorded in `plan-key-service.md` §6.

### The second factor, and the notes app (answered on the board, 2026-09-29)

**Second factor (all five):** 24-character backup codes; a second step
available to anyone by default; a reset or recovery removes every second
factor, *"but only after the reset"*, so the removal happens when the reset
completes and not when it is asked for; after a passkey, a second step only
when a person or tenant turns it on; 20 wrong codes in a row locks a factor,
the default, configurable.

**The notes app (17ae):**

- **Anyone can sign up.**
- **No claim code, and setup belongs on the server.** The owner: *"setup
  belongs on the server side end of story. Also in this app 1 user 1 tenant.
  User should be able to sign up via a form, press their passkey button, and
  bam theyre signed in."* So a new person gets a tenant of their own at sign-up,
  and the deployment's own owner is established on the server side, never by
  being the first to arrive. This also governs enrolment's first-comer (E5):
  it is not a default anywhere.
- **The page lives at `notes.inf.smg.icu`**, so the passkey's relying party
  is `inf.smg.icu`, which also covers the live deployment at
  `ordo.inf.smg.icu`.
- **A person can share a note with another person.** With one tenant per
  person, that is sharing between tenants, which is inter-space sharing
  (11-2): ruled 2026-09-08 and not yet registered. It becomes a dependency of
  the notes app.
- **The app belongs to the deployment and is deployed to many tenants.** The
  owner found the question about its own space confusing: in Ordo a space is
  a tenant, and the app is one program the deployment serves to every tenant.
- **Measure before changing anything about Cloudflare.**

**A tenant per person is built-in provisioning.** The owner, the same day:
*"this is simply how provisioning works - an app dev needs built in
provisioning."* Creating a person's tenant when they sign up is a platform
feature any app developer can turn on, part of provisioning (11-15), with the
notes app as its first user.

### Built-in provisioning, two limits (`provisioning-signup`, 2026-09-29)

- **Sign-up limits:** *"sounds good"*: 60 an hour for the deployment and 5
  per address, the defaults, configurable.
- **Tenants of people who never come back are kept.** *"if deletion is
  required thats completely an app dev decision."* The platform deletes none
  on its own; an app developer who wants a clean-up builds or configures it.

### Sharing, answered (`sharing`, 2026-09-29)

1. **A guest's acceptance is recorded in the sharer's log and in the
   deployment's security log.** *"Yes."*
2. **Share links are on by default, with 7 days as the default expiry, and
   the platform sets no maximum**: *"no max (not enforced by us)"*. The app
   developer sets the lifetime. This document's reading: a link still carries
   an expiry, of any length the app developer chooses, because the settled
   spec makes expiry what bounds a bearer secret; an app developer who wants a
   link that lasts years sets years.
3. **A guest writing is planned for after the first deployment** (18ab).
   Shares are read-only until then.

**Share link lifetime is layered, not one system setting.** The owner,
the same day: *"it should be possible for a tenant to choose how long it can
be if the developer exposes it - so it should not be a hardcoded system
setting."* Three levels: the deployment's default (7 days), a tenant's own
choice when the app developer exposes it, and the lifetime a link is minted
with, within what its tenant allows. The platform sets no ceiling of its own.

### Recovery details (`recovery-details`, 2026-09-29, in conversation)

1. **A recovery the deployment owner runs on the server skips the one-day
   wait.** *"thats fine."* Built this way, as a setting.
2. **A tenant manager may recover a member "if the app dev sets it that
   way".** A setting the app developer holds, off unless set. Two limits
   from the owner: a single-person tenant is an exception (there is nobody
   else in it to recover anyone), and **the sign-in method matters**: *"third
   party is managed elsewhere"*, so a person who signs in through an outside
   provider is recovered by that provider, and Ordo's recovery paths do not
   apply to that sign-in.
3. **Recovery does not cancel the person's grants or delegations.** *"no."*
4. **No platform objection to a last-owner recovery.** *"deployment owns
   this. There's no enforceable objection - that becomes dev coustomer
   contract."* The deployment owner decides; any right to object is between
   the app developer and their customer, not a platform mechanism.
5. **A tenant with no recoverable owner and no successor:** the same. The
   deployment owner decides what happens; the platform builds nothing for it.

## 2026-09-29 · Deleting a row takes the rows it owns with it (`owned-delete`, 25b)

The owner, asked whether this was the notes app's question or the
language's: it is the language's, and the answer is *"yes"*. A plain `del`
of a row deletes every row that references it through an `owned`
reference, including the access rows Ordo derives for an entity with an
`acl`. A plain (association) reference still refuses the delete
(refuse-with-referrers).

This settles a contradiction in `definition-language.md`: the ownership
section (~line 859, "owned cascades (part-of), association restricts") wins
over write rule 6 (~line 2325), which said owned subtrees need the explicit
`subtree` form. `subtree` stays the form for tree entities, where a delete
can remove a large hierarchy. Rule 6 is rewritten to match. Every program
with an `acl` entity could not delete its rows with a plain `del` until
this lands.

## 2026-09-29 · A new version goes live on the deployment's own database (R-S3 changed)

`plan-space-routing.md` R-S3 gated activation on every database taking the
program. Building 25i changed it: activation is gated on the deployment's
own database, and a tenant database that cannot follow is closed alone as
schema-behind and brought forward by the catch-up. The owner: *"that makes
sense."* With one tenant per person, the old gate let one unreachable
person block every deploy.


## 2026-10-01 · The deployment owner signs in separately (`deployment-owner-identity`, 18o)

The owner: *"Yes separately through the backend. The console is simply a
tool to interact with the deployment, but a different credentialing
system."* A deployment owner is not a customer of the app, so their sign-in
does not share the app's list of people or its passkeys. Until the console's
credential system exists, `ORDO_DEPLOYMENT_OWNERS` and the app's own sign-in
remain the model, now written down in `glossary.md` (**deployment owner**)
and `deploying.md` §4c.

## 2026-10-01 · Who may delete a note's owned rows (`owned-delete` ask 2, 27m)

The recommendation, accepted: whether deleting a row requires permission to
delete its owned rows too is the **app developer's setting**, and the default
is that permission to delete the row is enough. Owned rows, including
derived access rows, are part of the row; asking for separate permission on
Ordo's own bookkeeping would surprise most apps. The owner's reminder: the
notes app is an example, though what it teaches is real.

## 2026-10-01 · Offline writes and device totals (`offline-plan`, 18ad)

All six recommendations accepted: queue offline writes by default where the
device copy is on, with the app developer able to turn it off per operation;
drop a queued write after 24 hours by default, as a setting; a pending write
shows only what the person typed, marked pending; counts and totals for a
small fixed set of calculations run on the device, with a parity test against
the server; counts come before offline writes; offline, the last server total
may be shown labelled with its time, as the app developer's setting. The
owner's reminder: *"this is all a library so app dev decides what
information they display, we make that information available."* Every
display choice above is therefore something the library makes possible, not
something it imposes.

## 2026-10-01 · Live hooks: pacing, order, and the coordinator (`react-live-device` asks 2 to 4)

Refetch pacing follows the recommendation: group bursts and wait at least a
quarter second between refetches, as a setting (27n). The device copy
returns rows in each read's own order, with the order stated in the
interface, *"as long as the language allows it"* (27o): a read whose order
the language cannot state keeps id order and says so. The page-level
coordinator is after first deployment; the owner suggests consolidating
connectors, one stream per page instead of one per hook, which is the likely
shape (board `live-coordinator`). Ask 1 was answered with a question and is
put again.

## 2026-10-01 · The erasure receipt is built with the attach work (`erasure-receipt`, 17ab)

*"Take recommendation."* The manifest the attach needs and the receipt the
erasure needs are one mechanism pointed at two moments, so they are built
together.

## 2026-10-01 · Writes from outside Ordo are an access question (grief note g27)

On another system writing straight into an Ordo table, the owner: *"Direct
DB access is discouraged. It seems expensive to monitor integrity of data
source. This should be handled at the permission and access level,
security. If someone needs some special FedRAMP level then we can address
it."* So Ordo's database role is the only one with write access to Ordo's
tables, and detecting outside writes is not built until a customer needs it.
`plan-external-entities.md` §6.

## 2026-10-01 · The dev loop does not wipe what is not its own (grief note g24, 27p)

The owner: *"If things exist that aren't in schema don't wipe what's not
yours. Just make a note of it."* `ordo dev`'s reset drops only the program's
own tables, and lists anything else as present but not Ordo's. The same
principle as the shape gate's: extra live objects are not drift, and are
never Ordo's to remove.

## 2026-10-01 · Evolution plans (`evolution-plans` asks 1, 2, 4, 5; 27a to 27c)

**Render, then apply.** `ordo plan` renders and stores the migration and
`ordo apply` runs a stored plan: *"yes, errors are rejected."* A plan whose
checks fail is never stored.

**Keep everything.** *"No need to get rid of artifacts at all unless app dev
wants to then."* Pruning is the app developer's setting; Ordo never prunes
on its own.

**Seamless.** *"Old version should not stop serving until new version is
functional. So rollback should be possible but not required because failed
migration would not impact running tenant."* Each tenant expands, is
checked, and only then switches; a failure never switched. This replaces
holding a schema-behind tenant closed (25i's behaviour), which shut its
people out. The owner noted it *"may require more in depth planning"*; the
class B write freeze is where it is hardest.

**Choices live in the plan, with a receipt.** *"In the plan, but there
should be some record of it. At least a receipt that can be committed so
there's a code revision of the changes."* The `.ordo` stays pure state; the
receipt of each step's choices goes in the repository beside it.

**Ask 3 is open, and in tension with D-B2.** The owner asked what serving
many versions really costs, and said it is *"really up to the app dev"*. That
would replace D-B2's one-version-back bound (2026-09-21), so it is put to him
again with the costs rather than recorded as a reversal.

## 2026-10-01 · Environments (`environments`, all five asks; 27e to 27h, 27v)

`ordo apply` defaults to dev, and refuses staging until dev runs the version
and production until staging does. A dev branch is a tenant in the dev
deployment, removed only by command; promoting a branch to serve special
tenants (*"public tests of beta builds"*) is wanted later and needs more than
one served version (`tenant-builds`). Staging clones mask personal data by
default: *"that's a dev to end user contract"*, so the app developer owns the
setting. Staging is treated as production for access.

**Rollout**: *"Default to rollout every 5 minutes over the course of an
hour, all at once is also an option. Also provide custom blue green / custom
query blue green / canary."* Selection by query is a first-class strategy,
with the owner's examples: least usage, most rows of an entity, a match on
name or metadata, fewest members, every tenant except those on a version.

## 2026-10-01 · External entities (`external-entities`, all four asks; 27i to 27l, 27u)

Other systems' tables are read as **external entities**, generated read-only
translations that Ordo serves as ordinary entities. Ordo's own tables move
from `public` to a schema named by a prefix plus a random identifier (read as
a schema name). Other systems reach Ordo through **the door**; grant-based
views *"could be possible, maybe"*, not planned. **Not deferred**: *"they're
independent. If there is an issue at first-apply Ordo will error and will
also show existing untracked tables as well."*

## 2026-10-01 · One live stream per client (`react-live-device` ask 1, 27s)

On whether every read hook opens a `?live=1` stream by default: *"1 per
entity seems like it would be pricey among tons of clients. Would it not
make sense to keep 1 per client and share it? I'll take recommendation either
way."* Both stand: live is off by default, and when it is on a client opens
one stream that its hooks share. That needs the door to carry several reads
on one stream, which is the same consolidation the page coordinator wants
(`live-coordinator`).

## 2026-10-01 · Dual writes, and one-way steps (`evolution-plans`, in conversation)

The owner chose Stripe's pattern for seamless migration: *"Stripe's makes
the most sense."* During a step's soak the write path writes both the new
shape and the previous one, so a revert switches the tenant back to a
previous shape that stayed current; nothing reverse-migrates. This replaces
increment 3's write freeze. Reverse mappings are therefore generated with
the forward ones, at render time.

A step whose reverse cannot be generated (a merge, a narrowing) is **not**
a rejected error: *"it's a different kind of error. In this case it is
'accept this one way change - revert requires this or manual
intervention'."* The plan renders, names what a revert would need, and
promoting that step requires the confirmation.

## 2026-10-01 · What a plan cannot generate forward (`forward-gaps`, 16 of 22 asks)

**Refuse, and put the options in the error.** *"Let's keep as close to IaC
as possible: decisions should happen on paper."* No case a generator cannot
decide is resolved silently.

**Pre-checks run everything, everywhere.** Every check runs even after one
fails, so the developer gets the whole list first (ask 13), against every
tenant, since each tenant's data differs (ask 12). Each issue names the fix
in Ordo's terms: a value to map, or a query or op that finds the missing
data.

**Unknown is a value.** Old rows given a field they never had get the
developer's value or a clear "system doesn't know" (ask 2); times default to
creation time where one can be derived.

**`acl` on existing rows** grants the creator from the command log; the
system owns the schema. Flagged for a security audit (ask 4, checklist 27z).

**Same rules as compilation.** A change that would not compile fresh fails
the migration the same way, with the failure on the receipt (ask 5).

**`prev`, and orphan instead of drop** (asks 6, 8, 18, 20). Renames and
correspondences are declared: `entity derpderp (prev blabla) {}`, `field
macaroni (prev c)`, reaching one version back only. A field or entity no
longer declared is **orphaned**, kept and reported, never dropped by
default; pruning is an explicit step for exposure risk or a customer
requirement. This replaces class D1/D2 as the default and helps reverts.
Where `prev` lives, source or plan, is put to the owner (ask 23), since it
narrows the earlier "choices in the plan" ruling.

**Others:** moving a field to a related entity takes a developer-named rule
(7); narrowing takes developer-given precision, from a mapped set of options
(15); text to enum refuses with an explicit correction (16); text to date or
number accepts the formats the compiler knows, a list that grows (17);
foreign changes reach Ordo only through a declared external translation
(21); the by-design refusals stay unless real value is found (22).

**The owner's comment, a design principle for the syntax:** *"This area
quickly becomes non-deterministic for non-standard changes. We'll need to
consider the agentic focus of developing in Ordo. Keep reasoning stupid
simple, do as much provable magic under the hood as possible, and work at
producing keywords that have a low probability of hallucination."*

## 2026-10-03 · forward-gaps, the rest; and tenants that need not move

`forward-gaps` asks 9, 10, 11, 14 and 19 took the recommendation. On 14, an
op that fixes orphans is an op that runs a query modifying data. On 19, the
plan offers basic diagnostics of each collision. Ask 23 (where `prev`
lives) is deferred to the syntax design: *"it's hard to decide until we go
through the syntax"*, and receipts matter for reversibility. Ask 3 is put
again with an example.

On `tenant-schema-updates` (25i, which brings every tenant forward): *"This
leaves out deployment dependencies, but unchanged schema may be compatible.
That's developer choice."* Confirmed by the owner: a new
version is a new compilation, and it need not change the schema. New or
changed ops alone are not a schema change. If a new version changes only ops
and leaves the tables alone, a tenant's database needs no migration, and
whether that tenant moves to the new version is the app developer's choice,
like which versions stay served. The general rule, in the owner's words:
*"we want to force as little as possible but make the default behavior
secure, and comprehensive for a full production deployment."*

## 2026-10-03 · Public data, first answers (`public-data` asks 1 to 14)

Most answers were the owner thinking aloud; they became
`plan-public-data.md` and seven new asks. Ruled: public reads are **not
logged or audited**, and counts belong to the web layer (ask 10); unpublished
or erased content stops being served at once and caches are told to drop it,
with copies already taken out of reach (ask 12); an API key is a credential,
a capability token with an allowlist and a quota, not anonymous access (ask
13); allowed origins are configurable per tenant (ask 14). The direction, in
his words: public data belongs to the deployment's space (*"it belongs to the
App, i.e. the deployment"*); a tenant rarely has public data outside its
boundary *"aside from sharing things by choice"*; and public load is the
biggest load, so it moves to *"something dedicated and horizontally
scalable"*.

Also 2026-10-03: `tenant-schema-updates` ask 2, yes: a new version's plan
checks every still-served version's ops against the new schema and refuses a
break unless the developer confirms. `forward-gaps` ask 3, agreed: the
developer gives old rows their value, with the migration day as one option.

## 2026-10-03 · The public path: cached, unaudited, checked at compile time (`public-data` 18, 19, in conversation)

A site key that ships in the page secures nothing, since anyone can read it;
the owner agreed it is at most an optional label for telling sites apart.
Anonymous access is the reserved anonymous principal, so every call still
carries a credential. On the path itself: *"cache layer and no standard Ordo
audit and permission overhead."* Public reads are served from a cache layer
and skip per-call audit, guard walks and membership lookups. That is safe
because public is decided at compile time: an op declared public reads only
public fields. Public writes stay ordinary writes, checked and logged.
`plan-public-data.md` §5a. This replaces the entry first written for the
key, which overstated it.

## 2026-10-03 · Public data, the model (`public-data` asks 15, 16, 17, 20, 21)

The owner took the model the first answers pointed to.

- **Public data lives in the deployment's space** (ask 15, *"yes"*). A tenant
  stays private and contributes to the app's public face only by an explicit
  act: a submission, or a grant to the deployment.
- **Public is a declared qualifier** (ask 16, *"yes"*): `public` on an entity
  or field, `public*` cascading to children, refused by the compiler on a
  sealed field. Nothing is public unless declared, so a forgotten policy
  cannot open a table.
- **A public op may write into a private entity, write-only** (ask 17). The
  owner widened it: *"it could be possible the system has a grant to allow
  this - like a form or some upload."* So the write can come from a grant
  the deployment holds, a form or an upload, as well as an op the developer
  opens. These writes are ordinary writes, checked and logged.
- **A build-time credential** that runs public ops while a static site
  builds is a backup option on the future roadmap (ask 20).
- **API keys for an app's own customers** (ask 21): yes, each naming its ops,
  and **default quotas are unlimited**. That is the layered-settings rule
  again: Ordo ships no ceiling, and the developer sets one if the app wants
  it.

**A conflict, put back to the owner.** Ask 15's model places a forum post in
the deployment's space as a submission. The 2026-09-12 ruling on the system
space says third-party content does not go there, by the authorship test:
what a tenant writes lives in its own space and is shared outward, and
discovery across publishers is a derived projection. A marketplace grant
fits both. A submission does not. On the board as `public-data` ask 22.

**The conflict, settled** (asks 22, 23). The owner took the recommendation:
a post stays in its author's space and is shown through a feed and the
derived index, so the 2026-09-12 authorship test stands. His reasoning: a
public post read across tenants costs extra to gather, which the index and
the cache layer absorb, and *"what about single tenant deployments -
everyone exists within that same deployment - I guess it everything stays
within that space. I guess this makes sense."* Submissions remain for
content the app owns. And on how a tenant reads the deployment's space,
*"Agreed"*: three levels, public, any signed-in person by default, or
narrowed by a guard the developer writes. Ordo supplies the default and the
guard; the app decides. `plan-public-data.md` §3, §7a.

## 2026-10-03 · The syntax for the new language additions (`syntax-additions`)

The owner ruled most of `plan-syntax-additions.md`.

- **`public` is a reserved word** (ask 2). **A `deployment` line** marks an
  entity whose rows live only in the deployment's space, and is required
  before `public` (ask 3).
- **A guard may test the caller's own row and tenant** (`$caller.role`,
  `$tenant.plan`), compiled to one constant per call (ask 5). Today a guard
  sees only `$caller` and literals, so this is new grammar.
- **One `settings { }` block** replaces the unparsed `policy` blocks (ask 6),
  with names from Ordo's closed catalogue this round (ask 7). Per-op
  settings wait for 18ac's build, and the owner asked that this be written
  down (ask 13): it is, in 18ac.
- **API keys are issued at runtime against an `apikey` profile** the program
  declares (ask 8, *"correct"*), and a tenant owner or manager may issue keys
  for the tenant's own customers by default (ask 9).
- **A public write op is create only** (ask 10).
- **Pruning an orphan is a plan choice, never a source line** (ask 11):
  *"maybe prune is better with an extra confirmation."* Spelled `ordo plan
  --prune`, with a second confirmation at apply naming each pruned field and
  its row count. `ordo prune` stays the command-log command.
- **External entities are read at the same three levels** as the
  deployment's space (ask 14, marked done with no text, so the
  recommendation stands).
- **The language's reserved-word list is generated from the lexer's
  `Keyword` enum** (ask 15), ending a disagreement found while designing
  (checklist 27ag).

Open: `prev` after it lands (ask 1), whether ACLs are implicit (ask 4), and
the `<-` mark for external columns (ask 12).


## 2026-10-03 · The public path at runtime (`public-path-design`), and three smaller rulings

The owner took most of `design-public-path.md`, and changed three defaults.

- **No separate read-layer mode** (ask 5): *"it being there should not be a
  separate mode unless there would be meaningfully extra overhead. It
  should just be available to use out of the box no extra configuration
  steps."* Every node serves public requests from its cache with no setup.
  A node without a request pool stays possible, never required.
- **Public writes are unlimited per address by default** (ask 4): *"this
  logic can be complex and things like turnstile can stop it. There could
  be valid reasons for many writes."* The window is a setting the app
  developer or tenant turns on. This is the layered-settings rule again:
  Ordo ships no ceiling.
- **Both API key kinds ship together** (ask 6): personal keys that act as
  their issuer, and service keys with a seat of their own.
- **Taken as recommended** (asks 1, 2, 7 to 11): declared parameters only on
  a public read, no open `$refine`; `s-maxage` 60 seconds; a key is the
  bearer on every call, resolved in memory; `public` on an entity may carry
  a row condition; a public live stream pushes the fresh result; a public
  write is recorded as `$anonymous` plus the site label, with the address
  kept out of the chain; a deploy drops the whole public cache.
- **Shared rate windows across nodes** go on the post-deploy roadmap (ask
  12; checklist 18ae). Per node in v1.
- **Open:** which human checks (ask 3). The owner's go-to is Cloudflare
  Turnstile, with reCAPTCHA and another large provider; he asked whether
  the server needs support at all, since the widgets are client code. It
  does: only the server's check of the token with the provider stops a bot
  that skips the widget. Put back as ask 3.

Also ruled the same day: **`ordo check` exits non-zero when an op does not
compile** (`check-exit-code`, *"ok"*). **ACLs are opt-in**, and `public` stays
off guarded or acl entities (`syntax-additions` ask 4). **`<-`** maps an
external entity's columns (ask 12). For the 18 places the language reference
and the compiler disagree (`dl-spec-gaps`): settled syntax that does not
compile is a compiler debt, lean syntax that is unbuilt is marked so in the
reference, and the 3 contradicting how-tos are corrected now.

**The human check, settled** (`public-path-design` ask 3, *"ok"*). The server
verifies each widget token with its provider, because a bot can skip the
widget. Cloudflare Turnstile, reCAPTCHA and hCaptcha are supported, each one
HTTP call, set per deployment or tenant, and off until a key is set.

## 2026-10-04 · Money declares its places, and a storable decimal (`dl-spec-gaps` ask 3, in conversation)

**How the old rule came about.** The first spec (2026-08-10) said money is
stored as whole minor units and written in major units, with the server
converting. The conversion was never built. A review on 2026-08-12 (finding
6) found `29.99` compared raw against stored cents, and the fix made the
compiler refuse `29.99` ("write 2999") instead of building the conversion.
`howto-values.md` then followed the compiler. No ruling of the owner's chose
minor units; the owner asked *"why did we do it the other way? I don't
remember writing that in spec."*

**The reasons weighed.** For whole minor units: they match what can be paid,
splitting is explicit through `allocate`, payment providers use them, they are
fixed-size and fast, the chain's frozen encoding has no decimal tag, and the
device engine computes integers trivially. Against: no rates, unit prices or
exchange rates; a "minor unit" depends on a currency the type does not carry;
source and wire disagree (`2999` against `"29.99"`).

**The ruling:** *"okay the middle ground seems fine."* `money` stays whole
minor units, and each field declares its currency places (`money(2)`,
`money(0)`). Source writes `29.99` and the compiler converts. A new storable
`decimal(p)` holds rates and unit prices at any precision and becomes money
only through an explicit `round`. Payable amounts stay whole and auditable;
precision is the app's choice where it needs it. Checklist 27ak.

**As built (2026-10-04, commits 3933a89 to a1c0efb).** The chain's frozen
encoding gains `6 Decimal  str`, added by presence so every older entry hashes
as before (pinned by `PRE_DECIMAL_TAG_GOLDEN`). A decimal is hashed in one
canonical spelling: trailing zeros stripped, no leading zeros, no `+`, no
`-0`, so `0.0800` and `0.08` hash the same and a column's scale can widen
without moving a hash; the scale stays a schema fact in the artifact. Any
other spelling is refused on decode (`BadDecimal`). Results now spell money
as a string with exactly the field's places (`"29.99"`), as `api-engine-design.md`
§5 always said; the executor had sent the integer of minor units. Bare
`money` was built as `money(0)` so existing programs keep their meaning; that
default is put to the owner (`money-types` ask 1).

## 2026-10-04 · `ordo evolve`, and cleaning up `prev` (`syntax-additions` ask 1)

The owner first ruled `prev` belongs in the source (*"We do IaC style work ...
a user needs to define the state"*), then asked how a `prev` line goes away once
its rename has landed, whether versions should be separate files, and whether
*"ordo evolve"* should name migrations in general. Terraform's `moved` blocks
and Pulumi's `aliases` keep the same kind of line and never edit the source.
Ruled (*"ok"*): one source tree, since every applied version is already kept
as a stored artifact; **`ordo evolve`** renders and stores the plan, which also
frees `ordo plan` for the compiler's artifact; and a clean `ordo apply` ends by
offering to remove `prev` lines that have landed in every environment.

## 2026-10-05 · The audit's eight decisions, and bare money (`audit-2026-10-04`, `money-types` ask 1)

All taken as recommended.

- The playground spells money as the door does (`"29.99"`); the conformance
  decoder keeps stored units for its own checks (ask 1, *"yes"*).
- Two outputs with one name in a multi-source projection (`a[note] b[note]`)
  are refused now, as `projection_unknown_field`, before host-side opening
  lands; the owner asked that this be documented (ask 2).
- The artifact's cost class uses the schema, so it agrees with the door and
  the venue gate (ask 3).
- The storage sweep records its deletions in the security stream, one entry
  per pass per tenant (ask 4).
- `money(0) * money(0)` is refused like any other places, typed `decimal`
  (ask 5).
- 12 places is the ceiling for `money(p)` and `decimal(p)`, a language fact
  recorded in the reference (ask 6).
- `ordo dev` keeps refusing a reset when a foreign table depends on one of
  Ordo's (ask 7).
- When 27m lands, one setting decides who may delete owned rows, for a
  delete and an undo alike (ask 8).
- Bare `money` stays `money(0)`, so existing programs keep their meaning; new
  code is warned to say its places (`money-types` ask 1).

## 2026-10-05 · Refinement is a standard feature (`dl-spec-gaps` ask 4, in conversation)

Asked how an op opts in to refinement, the owner asked what refinement is,
then: *"this should be a standard feature...Imagine having to write an op
for every specific thing you could display for any tenant."* Ruled: every
ordinary read accepts refinement (filter and pick on its own outputs) by
default. It stays safe because it only narrows: the op's own rows and
fields, every guard applied, no condition or sort on a sealed field, and the
op's cost plus a condition. Public reads keep declared parameters only
(`public-path-design` ask 1). Whether an op may turn it off, and with what
word, is `dl-spec-gaps` ask 6. Today the server's envelope is built
(`crates/ordo-api/src/refine.rs`) but every op registers tier `None`, and the
SDK has no builder that sends one; checklist 27am.

## 2026-10-05 · filled, a migration hint, and the second audit's decisions

- **`filled` marks only `date` and `datetime`** (`dl-spec-gaps` ask 5, *"ok"*).
  Those are the values a client can produce alone; on any other type it is
  refused with a clear class. The owner first asked whether `filled` was for
  migrations; it is a call parameter the client fills, which led to the next
  ruling.
- **A migration refusal suggests a text field's 3 most used values**
  (`forward-gaps` ask 24, the owner's idea, *"ok"*): never for a sealed field;
  in dev and staging by default, in production only when the deployment owner
  turns it on, since it shows tenant data in a plan's output.
- **A group or reduce over a sealed field is plain SQL** (`audit-2026-10-05`
  ask 1, *"yes"*): nothing sealed survives a fold, so no key is released and
  no `unbounded` is demanded. This reverses the 2026-10-05 tightening in
  `host_work_bound` for folds.
- **A sweep pass that storage refused entirely still writes a security
  entry**, with `deleted=0` (`audit-2026-10-05` ask 2, marked done, the
  recommendation stands).
- **Refinement's off switch** is put again (`dl-spec-gaps` ask 6): the owner
  read refinement as parameters; parameters choose what an op answers, and
  refinement only filters its returned rows and fields.

**Refinement has no off switch** (`dl-spec-gaps` ask 6, *"ok"*). Parameters
choose what an op answers; refinement only filters its returned rows and
fields, guards apply, and the cost gate refuses an expensive one. The owner
added: *"Make sure we have docs on the cost gate. That will be an important
feature that could affect devs later unexpectedly."* A developer-facing guide
to cost classes and the gate is owed with 27am.

## 2026-10-05 · Refinement, the build's decisions (`refinement-default` asks 1 to 6)

From `plan-refinement.md`.

- **Conditions on open and blind-index fields compile into the plan** (ask 1,
  *"ok"*), so pages come back full. This replaces `api-engine-design.md` §7's
  post-page-window sentence; host-side sealed conditions, which cannot run
  today, would keep the after-window rule.
- **A sync accepts `where` and refuses `pick`** (ask 2): the device stores
  whole rows keyed by id.
- **Until host-side sealed filtering is built, the door refuses a sealed
  unindexed condition, naming the field** (ask 3).
- **Count over a filtered list is built this round** (ask 5, *"This round"*),
  against the recommendation to defer it: `std.E.count` takes `list`'s
  conditions, validated against `list`'s manifest and applied before the
  count.
- **A refinement's size is a setting with defaults** (ask 6): at most 16
  conditions and 200 values in one `in`. The owner: *"document it so its not a
  surprise or a debugging mission."* Both limits go in `howto-cost.md` and
  `docs/settings.md`, and the refusal names the limit and the setting.
- **Open:** whether `ordo check` prints each read's refinable fields (ask 4,
  put again with an example).

**Two more, 2026-10-05.** `ordo check` prints one line per read naming its
refinable fields and what each allows (`refinement-default` ask 4, *"Yes"*),
computed at compile time from the program alone; runtime refusals (size
limits, the cost gate) are `howto-cost.md`'s to explain. A tenant whose
storage could not be reached at all gets no sweep security entry
(`audit-2026-10-05` ask 3, *"ok"* to the recommendation): the entry records
what a pass did to objects, and an unreachable store stays a metric and a
log line.

## 2026-10-05 · Serve reads that return a sealed field, next (`serve-sealed-reads`)

The R8 audit found that the door returns no sealed value at all: every read
whose result holds one is a 501. Ruled: *"Okay do this next."* Each page (at
most 50 rows) takes one read lease for the tenant and request, opens that
page's envelopes, and returns them; the refusal names the field until it
lands. The owner added: *"On a separate cheaper model - try to find other
inconsistensies where we would have access to something but only in a one
way direction like this. Sealing but with no means to decrypt. After it has
a list - fable should check to see if those findings are contrary to our
goals or intended. Anything that is not intended should be flagged for
review."*

## 2026-10-05 · The one-way gaps: what the owner ruled

From `docs/audit-one-way-gaps.md` (a Sonnet sweep, reviewed by Fable).

- **Decline** (`person-can-leave` ask 1, *"ok."*): an invitee may decline an
  invitation and a guest a share ticket; a decline spends the ticket and is
  recorded.
- **Leaving a tenant is a mechanism, and the app decides** (ask 2): *"The
  ability should exist but that's an app choice."*
- **Deleting one's own account is a mechanism, gated by the tenants the
  person belongs to** (ask 3). The owner: many systems do not let a person
  delete themselves at all; a departure can strand a tenant (the only owner
  or manager) or the data that references the person. His design: *"a tenant
  could have a user-release that can remove a user under whatever app
  conditions are being met. if that clears then membership from the tenant
  could be removed ... but if a tenant locks a release it cannot be removed.
  Technically there could be a always true release or always false release.
  App choice."* So a tenant carries a release rule the app developer writes;
  a membership ends only when its release clears, and an account is deleted
  only when every membership has. Needs a plan (checklist 27aq).
- **Detaching a bucket** (`bucket-detach`, *"Yes - should be an option"*),
  under the same refusal as a change while objects are held. The owner
  added: *"Make sure we're documented all the methods of BYO custody
  items"*: one guide to every bring-your-own custody option.
- **Attestation** (`attest-sink-readback`, both *"ok"*): the destination can
  be switched off and read back; webhook dead letters are readable by the
  tenant owner and the deployment owner; no retry yet.
- **Pausing a tenant** (`tenant-pause`, *"agreed"*), with the owner's
  addition: *"as well as a layer that exists outside of that paused space -
  the inbetween space that allows a tenant to still login and do some
  actions - but that may actually be a deployment layer thing."* Needs a plan
  (checklist 27ar).
- **Read-backs** (`small-readbacks`, both *"ok"*): a tenant owner lists its
  outgoing guest shares; the deployment's self-serve mode and whether a
  claim code is set are readable, never the code.
- **Open:** `money-places-change`, put again with an example of what places
  mean.

## 2026-10-05 · Member release and tenant pause: the owner's answers

**Money places** (`money-places-change`, *"Agreed"*): a change of places on a
stored money or decimal field is refused unless it says whether to rescale
the stored amounts.

**Member release** (`person-can-leave` asks 4 to 10):
- With no release line, members are locked; the owner asked that this be
  documented *"so if a dev is trying to figure out why users can't leave he's
  not hunting for it"* (ask 4).
- The rule lives in the program, and the mode is also a layered setting
  (ask 5, *"agreed"*).
- What happens to rows that point at a leaver is the app's choice: they stay
  by default, and the app may remove them or reassign them, for example to
  the person's manager (ask 6: *"Think of o365, or any corporate app where
  sometimes ownership of resources goes to the person's manager or delegated
  relief"*).
- There is no tenant override built in (ask 7): *"we provide the
  functionality - and that function is the rule which the app dev owns. If
  they decide their rule includes a button the tenant can press- that is
  theirs to use."* A tenant can still remove a member, and an outside
  identity provider may end memberships.
- Deleting an account while memberships remain is refused; the API's refusal
  lists each tenant and what stands in the way (ask 8; Ordo produces no
  client app, so "list" means the response body).
- A deployment owner may delete an account for someone who cannot sign in, as
  a recovery-style case (ask 9). A random id with nothing behind it in the
  history is the erasure promised (ask 10).

**Tenant pause** (`tenant-pause`):
- Pause is a deployment operation; exposing it to tenants is the app's
  choice (ask 1).
- The layer outside a paused tenant is the deployment's own space (ask 3).
- Scheduled work during a pause is the app's choice, default not run; the
  owner noted jobs are not yet defined or built (ask 5).
- A notice for members is the app's to build (ask 6), so `pause.notice` is
  dropped from the plan.
- Open: a word on an op to keep answering while paused (ask 2), and platform
  ops using the door's own role and allowlist rules for who may call them
  (ask 4).

**Maintenance mode** (asked 2026-10-05): a deployment-wide pause of every
tenant, planned with tenant pause (checklist 27as).

**Tenant pause, asks 2 and 4 (2026-10-05).** An op the app marks keeps
answering while its tenant is paused; the owner wants a word on the op, and
named candidates: *"freespace, homespace, or maybe just lockfree or
nopause"* (the word is chosen with the plan). Platform ops such as pause
take their callers from the door's own role, profile and allowlist rules,
with defaults (*"ok"*).

## 2026-10-05 · The pause word, a status code, and the detach race

- **`nopause`** marks an op that keeps answering while its tenant is paused
  (`tenant-pause` ask 7, *"perfect"*): `op billing nopause = ...`, beside
  `unguarded` and `unbounded`.
- **`sealed_unsupported` answers 422**, no longer 501, now that sealed reads
  are served (`audit-2026-10-05c` ask 2, *"ok"*): what remains are op shapes
  the app developer fixes.
- **No re-check of the bucket on every upload** (`detach-upload-race` ask 1):
  *"In an async service? I don't think so - just if it fails because
  something took that name before it did. I'd expect constant race
  conditions otherwise."* Put again as ask 2: each stored file records the
  bucket it was written to, so nothing is checked at upload.


## 2026-10-05 · Where an upload lands, and stopping attestation

**Each stored file records its bucket** (detach-upload-race ask 2, owner: "ok").
An upload is not re-checked against the tenant's bucket before it commits.
The file's row names the bucket it was written to. Reads and the sweep follow
that name, so a file written while a bucket is being detached or replaced stays
readable and collectable where it landed. This replaces the per-upload re-check
offered in ask 1.

**Stopping attestation needs no signature** (audit-2026-10-05c ask 3, owner:
"ok"). The owner key's signature on a destination proves the owner chose that
address. Removing a destination has no address to prove, so the check is role
plus a fresh sign-in by the owner.

## 2026-10-06 · What a paused tenant answers, and a revert past a rescale

**What a paused tenant answers is the app developer's** (tenant-pause ask 8,
owner: *"It technically could but what it actually is/responds should be up to
the app dev. The API makes sense but a message as to way should be overrideable
dev. For instance if it is paused due to billing, security issues, breach of
contract etc"*). `space_paused` stays the class and 423 stays the default
status. The app developer may set, per reason word (`billing`, `abuse`,
`legal`, `maintenance`, `owner`, `other`), the status and the message a caller
sees. A layered setting with a default, as every setting is.

**A revert past a rescale converts the old amount** (revert-past-rescale ask 1,
owner: *"is this for money? If so the answer should be yes but precision etc
should be up to the dev to allow/agree to."*). It applies to money only: a
decimal is stored as its value, so a change of places leaves nothing to
convert. The revert converts the before-image's minor units to the field's
current places. A conversion that would lose digits is refused unless the app
developer has allowed rounding.

## 2026-10-06 · A leaver's name, a deleted account, and changing a bucket

**A leaver keeps their name; a deleted account is marked, not removed**
(person-can-leave ask 11, owner: *"Default should be to leave it. Honestly
This is a dev choice - maybe dev wants to show [Display Name (Deactivated)]
Which is not uncommon. There should be flexibility for that - or it should be
interfaced. It seems like theres too many variables to be able to preempt all
use cases ... it should be left, account should be marked deleted but not
removed ... and then dev can decide if they get removed automatically, and
they'll have the ability to decide how that information is presented
always."*).
- A leave keeps the seat's `displayName`. The seat reads `status = left`, and
  the app decides how to show it. Replacing the name on leave becomes an app
  developer setting, off by default (`member.leftName` stops being a default
  placeholder).
- History keeps the name, as every members change already does.
- Deleting an account marks it deleted and keeps it. Whether and when its
  personal data is removed is an app developer setting, default never. The
  erasure path built in 27aq R6 stays as what that removal runs.
- This replaces ask 10's *"a random id with nothing behind it"* as the
  default; that is now one choice the app developer may make.

**Changing or detaching a bucket that holds files is allowed** (st-attach-existing
ask 2, *"yes"*). Each file records its bucket (25ac), so files written before
the change stay readable where they are and are cleaned up there. Nothing is
copied. The owner asked (ask 1) that the design choice and its reasoning be
documented so users are not later confused: the guide says where each file
lives, why old files stay put, and that moving them is a separate step.

## 2026-10-06 · Share-link misses, the client's address, and retrying a busy server

**The share-link miss limit is not Ordo's to impose** (first-program ask 7,
owner: *"this should not be hard coded ... frankly it's not something we
should impose. We don't decide how a share link its used - its just access to
data which we know nothing about and the feature is there to be used. IPs are
just not something we should rely on. First thing is first - this needs to be
dragged out of being hard coded. There should be some mode (in general) that
lets it know it is behind a proxy -- but this shouldnt neccesarily be
required. We can't knowingly provide for every header for every proxy."*).
- `MISS_LIMIT` (5 a minute per address, `share_links.rs`) becomes an app
  developer setting, default off. A link secret is 256 random bits, so the
  limit was never what stopped guessing.
- Ordo does not rely on client addresses for protection by default.
- A general, optional "behind a proxy" mode is wanted, without Ordo naming
  every proxy's header. How it works is put to the owner as a new ask.

**The SDK retries a busy server, and the app developer sets how** (ask 8,
*"reccomended is fine - do not hardcode"*): on `server_busy` the SDK retries
honouring `Retry-After`, default 2 retries, set in the client's options and
able to be turned off.

## 2026-10-06 · The proxy mode, retry timing, and no single-database mode

**A proxy mode, off by default and documented** (first-program ask 10, owner:
*"Okay we can just do the proxy and have setting that is off by default.
(documented)"*). The deployment names the header that carries the visitor's
address and lists the proxy addresses it trusts that header from. Off, the
socket's address is used, as today.

**Retry timing is the app developer's, and grows** (ask 8, added: *"make sure
it is not hardcoded numbers and possibly a increasing time scale for the
retries"*). The number of retries, the first delay, how the delay grows and
its ceiling are all client options with defaults; `Retry-After` is honoured
as a floor.

**There is no single-database mode** (ask 9, owner: *"No. This is not what we
decided. a PG DbCluster can be selected as the default location. Within that
cluster each tenant gets 1 database (the database resource not a cluster),
optionally each tenant can be provided a different cluster to put their
database on."*). This restates the 2026-09-25 ruling (`db-always-a-database`,
`deploying.md` §2d). `ORDO_PG_ROUTING=single`, still the default in code, puts
every tenant in the deployment's own database and contradicts it. The switch
goes: every tenant always gets its own database, on the default tenant cluster
(`ORDO_PG_TENANT_HOST`, falling back to the deployment's own server) unless
one is named for it. Configuring nothing must still work, so the credentials
directory is optional when only `$deployment` is used.

**The proxy mode waits until after the first deployment** (2026-10-06, owner:
*"Just simply strip out proxy mode and disable the related ip code that would
depend on it. Add it to post deploy roadmap"*). Until it is built, nothing
decides by a client's address: the per-address sign-up limit and the
share-link miss limit are removed. The deployment-wide sign-up ceiling stays.
Checklist 28a, board `proxy-mode`.

## 2026-10-06 · The removal setting's reach, and where the client packages go

**Accounts already deleted follow the current removal setting**
(person-can-leave ask 12, *"ok"*). The due time is worked out at each sweep
from `deleted_at` and the setting in force, so shortening it, or setting
`now`, applies to everyone waiting, and `never` stops all removal.

**The client packages are published privately to GitHub Packages under
HominemAI** (client-packages ask 1, *"That sounds fine. Private."*), by CI.
GitHub Packages requires the npm scope to match the organization, so the
package names are put to the owner as ask 2.

**The client packages are `@hominemai/ordo-sdk` and `@hominemai/ordo-react`**
(client-packages ask 2, *"ok"*), published privately to GitHub Packages by CI.

**`@ordo/cdk` becomes `@hominemai/ordo-cdk`** and is published the same way
(client-packages ask 3, owner in conversation: *"yes"*).

## 2026-10-08 · Onboarding order, React gaps, and branch serving

**The order for onboarding ellanor and denizens** (ellanor-readiness ask 1,
*"Execute."*): JSON fields, delegation for workers, triggers on, a plan for
backend functions, then emailed-code and OAuth sign-in. Ellanor-only gaps
follow. This supersedes "no new features until the notes app is live" for
these items.

**React building blocks** (react-gaps): fix the five small gaps now (ask 1,
*"Yes"*), and give platform ops (share, links, revoke) hooks and guest windows
a hooks client now as well (ask 2, *"yes now"*).

**Branch tenants need multi-version serving, built for dev first**
(environments ask 6, *"okay sounds good."*), with the owner's comment: *"I
want to make sure that there will actually be a served point for the
environments."* Dev and staging are each reachable at a real address a team
can use, not only a design.

## 2026-10-08 · Backend functions wait; mail and functions are the app developer's; sign-in plan answers

**Backend functions move to the post-deploy roadmap** (backend-functions
ask 1 and comment): *"we will continue relying on cloudflare for instance -
but we will come back to backend functions ... we're not ready as it
requires a much deeper design concerning safety, sandboxing, and the ability
for backend services as code. The truest BAAS anyone has ever delivered."*
The plan (`docs/plan-backend-functions.md`) stays as the starting point.
Rulings kept for when it resumes:
- API keys sealed under the deployment's keys, per environment, never read
  back (ask 3, *"agreed."*).
- A page-called function runs as the signed-in person through a one-call
  delegation (ask 4, *"yes."*).
- Increments 1, 2 and 4 first (ask 5, *"Probably makes sense?"*).
- Connected accounts (a person's Google or GitHub) are built in as an
  option, and an app may still use its own (ask 6).
- Languages (ask 2, under discussion): TS/JS first, then Python, Rust, Java
  and others; the owner adds *"our sdks will allow the passing in of
  functions."*

**Mail and functions are services the app developer uses as they choose**
(federated ask 2): *"Mailing itself is a deployment service - not bound down
by normal credential flows. The same goes for functions - there are a
million reasons why a function may be called and we cannot make that
decision. The app dev decides what and when to run it. It is however
possible that we provide a system to pass along grants for that function.
Remember we have public grants as well now."* Ordo's own mail setting sends
sign-in codes when the app uses the built-in sign-in; an app may run its own
SMTP or anything else. Neither depends on the other.

**What an app exposes is the app's choice** (federated ask 3): guest sign-in
is a mechanism Ordo provides; whether and how it is exposed belongs to the
app developer.

**Sign-in plan order** (federated ask 1, *"ok"*): OAuth2 (Discord, GitHub),
then emailed codes, then OIDC. Passkeys are already built. Re-signing up
after removal is a new, empty person (ask 5, *"ok"*). Importing Supabase
users keeps an existing email-plus-Discord join (ask 6), and an importer for
Supabase users, and other systems, is a design of its own to consider
carefully.

**Worker credentials wait for the post-deploy roadmap** (delegation ask 4).
Platform writes stay closed to workers until they record the worker (ask 3,
*"ok"*).

**Routines** (triggers asks 1 and 2, *"ok"*): build `each` bodies next; a
newly deployed routine acts only on new changes.

**JSON defaults** (json-field asks 1 and 3, *"ok"*): 256 KiB and 64 levels;
text inside a document is NFC like every text field.

## 2026-10-08: delegation, sign-in and self-serve, JSON filters

**Workers cache per worker** (delegation ask 1, *"Yes"*).

**A worker uses listed ops like anything else** (delegation ask 2):
*"There generally would be no case where anything could not use a listed
op? So yes but same for anything because ops are the only normal way to
transact."* A worker may delete only through an op that lists the delete,
the same as every caller.

**Ops reading a person's own linked account** (federated ask 4, *"ok"*):
only their own, every read recorded.

**Self-serve does not wait for a passkey touch** (federated ask 7):
*"Passkey requirements are up to the appdev ... A login method is a login
method. The app dev decides if it grants access or creation of the space."*
Any sign-in method the app developer serves can give a person their own
tenant when self-serve is on. Refined the same day: *"any login method should
have no bearing on auto-provisioning."* Self-serve checks no presence, and no
setting brings it back. Whether people get a tenant at all stays the app
developer's choice: self-serve is off by default, and admission decides
membership.

**JSON fields may become filterable** (json-field ask 2): *"if pg supports
the filtering we can do that."* Postgres does. When to build it is asked
again on the board.

**Filter inside JSON now** (json-field ask 2, 2026-10-08): *"Add filtering
now."* Built before go-live. Asks 4 to 6, *"ok"* each: a missing key and JSON null
both satisfy `is null`; a value of the wrong type does not match and raises no
error; arrays are filterable (`tags contains "red"`).

**Mail ports are configurable** (2026-10-08): *"why are we blocking other
ports? It should be configurable."* Any port; the TLS mode is a setting
(`implicit` by default, or `starttls`, which is required once chosen and never
falls back). The outbox's retry and batch numbers become settings. Whether a
plaintext mode should exist is asked on `mail-relay-gaps`.

**Plaintext SMTP is allowed, off by default** (mail-relay-gaps ask 1):
*"On the one hand security by default - on the other hand flexibility for
quick setups and mockups - more configuration = bad dev experience."* Then:
*"Allow it, disabled by default but whatever errors says where in settings,
or what setting to change."* `ORDO_MAIL_TLS=none` exists; `implicit` stays the
default. **A refusal a setting can fix names that setting and the value to
use.** For quick local setups, `ORDO_EGRESS_PRIVATE_HOSTS` lets named private
hosts (a local Mailpit) through, empty by default.

**Splitting an app into several deployments is post-deploy** (2026-10-09):
*"Add splitting to post deploy roadmap."* Shared sign-in across separate Ordo
deployments (`shared-identity`) moves to the future roadmap. One deployment
with several front ends on subdomains (`subdomain-signin`) stays current.

## 2026-10-09: board answers

**Totals over sealed fields wait** (sealed-folds, *"ok"*): sum, min and avg
over a sealed value, opened and folded on the server, come after go-live.

**Email code limits keep clock windows** (federated ask 8, *"ok"*).

**An invitation admits a newcomer by every method** (federated ask 9,
*"yes"*): under `invite-only`, a valid invitation lets an unknown person sign
up with a passkey, password, emailed code or outside provider, then admits
them.

**One sign-in across subdomains** (subdomain-signin asks 1 to 3, *"ok"*): a
hand-off where the core site signs in and returns the person with a one-time
code the subdomain swaps for its own session; return addresses match by
origin with any path; wildcard origins such as `*.example.com` are allowed
when switched on, off by default.

**Private hosts reach every outbound call** (audit-leftovers, 2026-10-09):
ask 1 *"ok"*: `ORDO_EGRESS_PRIVATE_HOSTS` applies to mail, OIDC, OAuth2 and
Turnstile alike, and is checked at every boot. Ask 2, *"cidrs should be okay
too"*: entries may be exact host names, exact IPs, or CIDR ranges. Link-local
and cloud metadata addresses stay refused even inside a listed range.

**Tenants may bring their own provider** (tenant-providers, 2026-10-09):
*"A tenant given the opportunity should have the ability to setup their own
provider as long as it falls within the oauth / oidc standard protocol. URIs
callbacks client secret client ID etc. And from this the App Dev may want
tenants to assign OIDC/oauth attributes(groups etc..) to app level (tenant
level? Depending on how the App Dev defined it) roles etc.."* This brings plan
L1 (tenant-owned providers) forward and adds claim-to-role mapping. The
details are asked on the task.

**Tenants choose which providers' two-step counts** (federated ask 10,
*"Open"*): a tenant may accept any connection's two-step claim for its own
data, without the app developer first opening that connection to tenants.
The deployment-level acceptance stays the app developer's. Read as "open it";
to be confirmed if meant otherwise.

**An accepted two-step login skips Ordo's own second step** (federated ask 11,
*"yes"*), under the same per-connection setting.

**Tenant-owned providers** (tenant-providers asks 1 to 4, 2026-10-09):
- Off by default; the app developer switches it on; a tenant's owner adds
  providers. *"Make sure it is documented in our user docs."*
- The client secret lives in the tenant's own database, sealed with the
  tenant's key. *"There should always be an originating admin account -
  someone has to be able to login to setup that OIDC. It is a common pattern
  accross the industry - admin account + IDP."* A tenant always keeps an
  admin account that signs in without the tenant's provider.
- Addresses: *"If they decide their provider is at [any domain] then that's
  where it is ... in general it should be allowed and definitely the default
  assuming add your own OIDC is enabled."* Any address is allowed by default.
- A tenant's provider admits new people into that tenant only (*"Yes."*).
- A tenant's provider address may be any domain, but a name resolving to the
  cloud metadata address is refused (tenant-providers ask 5, *"yes"*).
- Loopback (the Ordo server itself) stays refused for a tenant's provider
  (tenant-providers ask 6, *"ok"*). The app developer's
  `ORDO_EGRESS_DENY_CIDRS` also applies to tenant providers.

**Build the public path before the first deployment** (public-data ask 24,
2026-10-10): *"This is an important feature before we can deploy because most
apps have a public side."* The `public` keyword, the anonymous door, the cache
and the read layer are built now from `docs/design-public-path.md` and
`docs/plan-public-data.md`, read-only first.

## 2026-10-10: board answers

**The consent ceremony is built before the first deployment** (delegation
ask 5, *"yes"*): a person grants an assistant or agent delegation through the
door (11-4b).

**A public route lists the sign-in methods** (signin-methods-route, *"yes"*):
what a deployment, and a named tenant, serves, so pages build their sign-in
screen from it. It reveals only what sign-in buttons already show.

**Limiting ops to roles** (op-role-limit ask 1): *"ok - but where does the
granularity lie since tenants could define their own internal roles as
well?"* Put again as asks 2 and 3, since the spec rules that who may call
what is runtime state, not syntax.

**Who may call which op: a default per op, owned by each tenant**
(op-role-limit ask 2, *"ok"*): the app developer ships a default (the
domain's profile template), and each tenant's owner may change it for their
tenant. Ask 3, on tenant-defined roles: *"yes this was in the original spec.
Please confirm."* The spec fixes roles at four (glossary, "profile / role /
override"; security-logging-design §8 puts custom roles out of v1) and gives
tenants groups with their own op lists (§6, DL "Groups"). Put as ask 4.

**Tenants' own roles over resources** (op-role-limit ask 4): *"an app dev can
make it so tenants can have their own permissions internally to different
resources - similar to how ellanor can define roles to people and then lock
projects to those roles or people etc."* This is the spec's groups plus `acl`:
a tenant names groups, and an `acl` row on a record names a person or a group;
the acl-derived guards expand groups (DL "Groups", built and tested in
`guards_corpus`).

**A manager grants only what they hold** (op-role-limit ask 5, *"ok"*): a
manager may change the tenant's profile only to hand out ops they can call
themselves; only the owner widens past that.

**Ellanor is server-managed; client-first stays the pinnacle**
(ellanor-readiness ask 3, 2026-10-10): *"server managed I suppose - but
client first is very important as well to have that as an option. So it
stays on the future board but I want to compete with jazz and that will be
pinnacle."* Ellanor's offline needs `plan-offline.md` §1 only. The vault
(F7) and vault sync stay on the future roadmap, named as the feature that
competes with Jazz.

## 2026-10-11: board answers

**Slow a busy visitor by address behind a CDN** (public-data ask 28,
*"Yes. Though CDNs usually handle this through cacheing and CFs page
protections"*): proxy mode (28a) comes before the first deployment, as a
slow-down only, off until the deployment names its proxy. The CDN's own
cache and page protection stay the first line.

**Functions run as WebAssembly in V1** (backend-functions ask 7, *"Yes for
V1. We may look at other options later ... a local pod or local semi
sandboxed process ... through some hook"*). Other runners are filed for after
the first deployment as `functions-other-runners`.

**Each call gets a fresh instance** (ask 8, *"We can do fresh and test
latency and if it is a problem we can look at a shared pool"*). Latency is
measured in the build; a warm pool is filed as `functions-warm-pool`.

**A function may hold its own grant** (ask 9, *"ok"*), declared in the
program and recorded on every use.

**Inline functions and a register call** (ask 10, *"yes. And there should be
a method to register?"*). Registering a function object by a call is the
common shape (Firebase `onRequest`, the AWS CDK `new Function`), and the SDK
offers it beside functions in their own file.

**Public functions and the cache** (ask 11, *"if this is about the public
op cacheing service - it should be default"*). Put again: a reading public
function is cached by default like a public op, and a writing one never is.

**Services get identity, a route and egress first** (ask 12, *"yes"*).

**A public function has no signed-in person** (comment on asks 4 and 9: *"a
public person isn't exactly signed in ... Forms might be a function instead
of an op"*). Put as ask 13: the visitor's public grants plus the function's
own grant.

**Design the Supabase import now** (ellanor-readiness ask 5, *"design"*).
The build follows the design.

**Audit 2026-10-11, in conversation.** *"Managers should not be able to give
themselves more access"*: the code already refused every route; a test now
guards it and the roles guide is corrected. *"client calls should not freeze
the page. This should be a render now - fill after thing"*: the two calls
without a deadline get one, and `more()` no longer copies earlier pages.
Finding 26, *"feels innocuous but we can patch it"*: a tokenless caller gets
one answer for a real op and an unknown one.

## 2026-10-12: board answers

**Public write cap** (public-data ask 29, *"10000 default"*):
`ORDO_PUBLIC_WRITES_PER_HOUR` defaults to 10000 per op per hour.

**Tenant origin lists stay off until the deployment owner allows them** (ask
30, *"yes"*).

**A duplicate on a keyed public create answers like a new one** (ask 31,
*"yes"*), so a newsletter form never tells a visitor whose email is listed.

**The human check runs before the cap counts a write** (ask 32, *"yes"*).

**Public writes into a tenant's own space** (ask 33, *"depends on the setup
... a shop with publicly acessible data would write to the deployment ...
seems like a niche use case"*). Public writes land in the deployment's space;
the tenant case is filed for later as `public-writes-tenant-space`.

**Supabase import design** (supabase-import asks 1 to 10). Ellanor is a
reference only, *"just the idea of what ellanor WOULD use"*; no Jazz reader is
planned. Bcrypt hashes opt-in and rehashed (2), unused ones deleted after a
setting (3), people imported as one recorded act (4), big-bang move after 2
rehearsals (6), guests left behind (8, *"there's only 1 anyway"*), RLS drafted
as commented-out grants (9), exported files only (10). The 3 hominem apps as
one deployment: *"we're not doing that yet - but in the end it likely would
be"* (5). Keeping Supabase read-only after the move: *"That's mine to do"*
(7), so Ordo only documents it.

## 2026-10-16: the import method comes from trying it

**Migrations are learned, then written down** (in conversation): *"This
isn't straight forward ETL - And we'd need to make a lot of assumptions about
the system. I think our best bet will be going through some of the design
work and implementig some of the parts, then attempting to migrate some apps
and using the results and observations (recording each step) To then produce
a methedology and create a skill from it"*. The Supabase import design is a
starting point. Parts are built as a try needs them, each try is logged in
`docs/migrations/`, and the method and a skill are written from the logs.

## 2026-10-18: the notes app is deployed

The owner, on `first-program`: *"We already did the notes app. We had
deployed it already and it worked."* The board still read go-live as next,
because nobody had recorded it when it happened. Checklist 17ae is closed on
his word. What "deployed" covers is now its own task (`what-deployed-means`),
because the board used the word for 4 different steps.

## 2026-10-19: what "deployed" means, and notes next

**"Deployed" means 4 things are true at once** (`what-deployed-means`): the
server runs, the program is applied to it, the app's page is served at its
address, and a real person signs up from a phone and writes. The 4-step smoke
test checks it. When only part is done, the step is named on its own ("server
up", "program applied"). **"Live" means deployed, with its data kept across
upgrades.** Reason: the board used one word for 4 steps and read notes as
undeployed after it had run.

Correcting the 2026-10-18 entry: the notes app ran successfully once, about
2026-10-05, before the page moved to the `ordo-react` components. It is not
deployed today in the sense above.

**Notes v2 over v1, with its data kept** (`end-to-end-tests-through-the-notes-app`,
all 5 asks yes). The new use cases ship as version 2. The upgrade is itself a
test. A throwaway stack runs on every push, "as long as the throw away can
provide valuable feedback"; a nightly run hits staging with real credentials.
Discord and other OAuth sign-in are tested against a local stand-in. A few
flows drive a real browser with Playwright.

**The SDK retries a lost write race** (`six-writes-to-one-row-one`, "yes"), as
it retries a busy server: nothing ran, so a retry is safe.

**Supabase import paused** (`supabase-import` asks 11 and 12): "I think the
first app will be my Ordo which will host the other Ordos", and the notes app
comes first: "do some runs. I'd like to test it throughly and add more
features to it."

## 2026-10-21: a slow tenant setup is acceptable

On `slow-passkey-signup`: *"I think generally the concept of provisioning
taking some amount of time is not unprecedented. It definitley seems like a
fix it if someone yells situation."* So no pre-made tenants, tenant pools or
background setup for now. What gets fixed is only what makes a slow sign-up
look broken: the call that makes a tenant gets its own, longer wait; a
timeout is reported as a timeout and never as an unreachable server; the
page says it is setting up the account; the end-to-end tests wait as long.

**Then, the same day:** *"The process of provisioning should not be a waiting
call."* Sign-up answers at once. Making the tenant is a job of its own, and
the client learns it is ready from a status or a stream. The SDK gives the
app developer one way to wait for ready, so no app writes a polling loop. A
restart part way through resumes the job, and one person never gets two
tenants. This replaces "its own, longer wait" above.

## 2026-10-21: board answers (SDK ease, screenshots, tenant setup)

- **The 4 small SDK improvements are built now, and the 3 medium ones are
  planned now** (`ease-what-the-sdk-asks-of`): "We're basically working on v2
  now so lets go ahead and plan this work out."
- **Screenshot comparison starts** (`should-the-notes-end-to-end-tests-compare`),
  with the owner's concern first: "my biggest concern is false flags - these
  could stop the pipeline." So it starts as a report that never blocks; whether
  it blocks after 20 clean runs is asked.
- **Tenant setup** (`tenant-setup-without-waiting-3-choices`), discussed, not
  yet ruled. The owner: what to show while a tenant is made is "up to the
  developer", and deployment-space things can be shown; "nothing can be
  written to the tenant"; a component could show the sync or loading status.
  On a stalled setup: "just saying something is going to fail after 15
  minutes doesn't really make sense to me ... it should never really stop
  trying", and a stop must not block, and the feedback must reach the person
  and the app developer "from both angles". Put back as 3 asks.

## 2026-10-21: tenant setup, the device plan, share windows, screenshots

- **Tenant setup** (`tenant-setup-without-waiting-3-choices`, all yes): Ordo
  gives app developers a tenant status hook and a gate component, and refuses
  tenant writes before ready with a retryable "tenant not ready". Any running
  server keeps retrying a stalled setup with no time limit and reports it as
  stuck after a setting's time, to the person, the app developer and the
  deployment owner. If a tenant can never be made, the account stays.
- **The device plan** (`plan-a-ready-made-device-dropping-unused`, all 7 yes,
  `docs/plan-sdk-device.md`): `openDevice` in the SDK; the SDK stops reloading
  feeds on focus and every 30 s, "Document this behavior"; an unused sync
  closes 15 s after its last screen, "Document this behavior"; rows held only
  by a sync unused for 7 days are erased; the leader tab first; deploying.md
  recommends HTTP/2 at the proxy; the device registry keyed by space and person.
- **Closing a share window** (`closing-a-whole-share-window-no`, yes): the
  server ends every open feed on a window the moment it is closed.
- **Screenshots** (ask 2, "Ok"): report only, blocking after 20 clean runs in
  a row.

## 2026-10-21: tenant setup, two follow-ups

- **Where "stuck" is reported** (`tenant-setup-without-waiting-3-choices`
  ask 4, "ok"): on the security-event stream, as built. It already reaches
  the app developer, so a second feed would only be one more thing to watch.
- **Deep health** (ask 5, "ok"): a stuck setup turns `/health/deep` to
  `degraded`. A stuck setup is a person who cannot use the app, and alerts
  should see it without reading a new field. `/health/ready` is unchanged,
  for the same reason as the audit drain: a logging-style fault must not
  pull a serving node out of rotation.

## 2026-10-21: the device plan, two follow-ups

- **Older syncs age out too** (`plan-a-ready-made-device-dropping-unused`
  ask 8, "ok"): a sync saved before last-used times existed is stamped
  "used now" the first time the device opens, then erased after 7 unused
  days like the rest. Leaving them alone would mean old devices never shrink.
- **Tabs and an app's own storage** (ask 9, "ok"): when the app passes its
  own storage backend, tabs do not share feeds by default, because Ordo
  cannot know other tabs see that backend. The app developer turns it on
  with `openDevice({ tabs: 'share' })`, which 7a already built.

## 2026-10-21: notes v1 is tested before v2

The owner tests notes v1 on his own machine first (`examples/notes/dev/serve.sh`
and the page on `http://localhost:5173`), "to catch any bugs in v1 before
hitting v2". Each bug he finds becomes its own task under Reported fixes. v2
waits until v1 is clean. His local data then serves as the upgrade check: v2
is applied over it and his notes must survive.

## 2026-10-21: staying signed in is standard

Owner, on notes v1: "Your session persisting after a refresh or browser
restart should be standard expected behavior. Not just 'looks like a bug'."
Ordo keeps a person signed in across a reload and a browser restart by
default. The token itself stays in page memory; a renewal cookie that scripts
cannot read (HttpOnly, Secure off loopback, SameSite=Strict, path
/auth/refresh) brings it back. How long an unused device stays signed in is
a setting (ask 3, recommended 30 days, renewed by use). Turning it off is a
setting too, for an app developer who wants a touch on every visit.

## 2026-10-21: session settings are layered to the tenant

Owner: "There should however be session settings the app dev can set. And
for certain applications it is forseeable that tenants would want their own
settings for session duration because different companies different
compliance." Every session setting (token lifetime, ceiling, staying signed
in, its idle lifetime) has three layers: the deployment's secure defaults,
the app developer's defaults and allowed range, and a tenant's own value
inside that range. The narrowest layer that is set wins. Whether a tenant
may go longer than the app's default is ask 4 on the task; built as "within
the app developer's range".

Ask 4, same day: "It is there space so yes - they own their logins if they
have their own auth as well (if enabled like OIDC etc)". A tenant may set its
session values longer or shorter than the app's default. The app developer's
range is optional; with none, the deployment's absolute limits bound it. A
tenant with its own sign-in connections owns the session settings for logins
through them too.

Same day: "But the behavior to make it changeable is blockable by the app dev
anyways. For instance the app dev coud provide them an interface that shows
set values." The tenant's setter and the read are separate platform ops under
the existing op-access controls, so an app developer can deny the setter and
still show tenants the values.

## 2026-10-22: notes v1 findings, answers

- **Safe schema changes apply themselves** (`notes-v1-would-not-start-on`
  ask 1, "Yes"): `ordo apply` makes additive changes that cannot lose or
  reject data (a new field with a default, a new index) itself, under a
  setting that is on by default. Changes with data risk stay refusals.
- **The refusal is pasteable** (ask 2, "Yes"): it prints the exact
  `ordo migrate` command with this deployment's database filled in.
- **Staying signed in** (`refreshing-the-page-signs-you-out`): the renewal
  cookie as built (ask 2, "ok"); 30 days idle by default (ask 3); an overall
  limit is a setting, none by default, settable by app and tenant (ask 5).
- **Signed in means authenticated** (ask 6): "yes... imagine having to touch
  your passkey 30 times everytime you refresh the page. If you are signed in
  = that passkey was your credentials - you are authenticated." Applied as a
  principle: a platform op that only reads asks for no passkey touch. Changes
  that the step-up rules cover still ask, as before.

## 2026-10-22: an overall limit on a remembered sign-in

Ask 5 on the staying-signed-in task. A remembered sign-in gets an optional
overall limit: N seconds after the original sign-in, the person signs in
again, however often it was used. The default is none. It is layered like
the idle lifetime: `ORDO_REMEMBER_MAX_SECS` with the absolute cap
`ORDO_REMEMBER_MAX_SECS_LIMIT` at the deployment, `rememberMaxSecs` from the
app developer (`std.deployment.setSessionPolicy`) and from a tenant
(`std.space.setSessionPolicy`). The original sign-in time travels with the
renewal family through every rotation. The cookie, the token and a resumed
session never outlive it. Reasoning: big consumer apps keep you in while you
use them; a company with compliance rules sets, say, 30 days.

## 2026-10-22: signed in means authenticated

Ask 6, owner: "imagine having to touch your passkey 30 times everytime you
refresh the page. If you are signed in = that passkey was your credentials -
you are authenticated." A platform op that only reads asks for no passkey
touch by default. Read or change is the op's own declaration, its `OpKind`,
the field that also makes it a `GET`. A change keeps the step-up rules of
2026-09-29. `ORDO_PRESENCE_ON_READS=on` restores presence on reads for a
strict deployment. A read freed this way stays closed to delegated tokens,
because the touch was what kept a capability token off reads such as
`std.space.callersOf`. A read the app developer names in
`ORDO_STEP_UP_EXEMPT_OPS` is open to them, as before.

## 2026-10-22: notes v2 features

`notes-v2-which-features-it-adds`, all 5 asks "Yes": attachments, reminders
(in the app and by email), pinned and archived notes, export of all a
person's notes, and search across a person's notes and those shared with
them. Search was recommended for v3; the owner put it in v2.

**Password-gated share links** (owner comment, same day): "someone needs a
password in addition to the link. So they go to the link and a password is
requested before a user can actually access it." Ordo has no such gate
today: a link is a bearer secret. It is built in Ordo, not in the app: the
sharer may set a password when minting a link or later; the door stores only
a slow hash; opening the link needs the password before any grant is
issued; wrong tries are limited and then the link locks for a while, both
as layered settings with secure defaults. Delivering the password by
another channel is the app developer's choice.

## 2026-10-22: what notes v2 found hard, answers

`what-notes-v2-found-hard-in`:
- **A person's own download** (ask 1): "Yes only if allowed by the app dev.
  There could also be filtering to determine what 'all' is." Off unless the
  app developer turns it on, and the app developer says which entities and
  fields it holds.
- **File helpers in the SDK and ordo-react** (ask 2): "ok".
- **Link password lockout** (ask 3): "Off by default. But per link. We should
  also overall add a max usage counter (after n accesses it is unavailable).
  And expiration dates which I think we already added." The wrong-try lock
  stays per link and is off by default; a link gains an optional maximum
  number of opens; expiry already exists.
- **A verified email for passkey and Discord users** (ask 4): "yes - but app
  dev could make it required." Optional by default; the app developer can
  require it.
- **Tenant catch-up several at a time** (ask 5): "ok", bounded by the dial cap.

## 2026-10-22: email, download and skipped tests

- **Resend after a crash** (`an-email-could-be-sent-twice`): "Resent with a
  possible duplicate ack." Ordo resends, accepting a rare duplicate, and keeps
  one Message-ID per email across tries so mail clients can fold copies.
- **Skipped tests run on CI** (`a-closed-share-window-stayed-open`, "ok"):
  the S3 storage contract against a local S3 stand-in, and `ordo dev`'s own
  Postgres cluster.
- **Changing or removing an email** (`what-notes-v2-found-hard-in` ask 6):
  "Yes by default... But this is the app devs choice." A passkey touch by
  default; the app developer may turn it off.
- **Email table unreadable** (ask 7, "ok"): an op that requires an email is
  refused, retryable.
- **Own download** (ask 8, "ok"): refuses delegated tokens and guests by
  default; the app developer may open it to delegated tokens by name.

## 2026-10-22: custom settings, for Ordo v2

Owner: "a way to define custom settings ... if an app dev needs something
configurable across tenants or across the whole system, the settings are
there and we'll provide a dynamic interface in our ... console ... editable
via code and possibly via ordo itself at apply time." Then: "we should have a
second kind" (secrets), "agree on renaming or removing for safety", "Feature
flags, definitely. feature flag part might in part have to integrate maybe
with our green blue system when deploying different branches or trying to
roll out different versions to different tenants."

Folded into `settings-layers` (24a to 24d), marked for Ordo v2: one layered
mechanism for Ordo's own settings and for app-defined ones, declared in the
program, edited at apply, in the console or by code, with secrets as a
sealed write-only kind, renames and removals classed like schema changes, and
feature flags as settings. Whether flags and tenant builds (27t) share one
targeting model is ask 1.

## 2026-10-22: the console is required for launch

Owner: "It is required for release launch as it is hwo people will be able to
read and maange their instances for when they are less technical and is part
of the open source release." The console (18o) moves from later work to the
launch path. The 2026-09-21 split stands: the open repository carries the
server and the console, the private fork the operator. The launch scope is
asked on `console-operator` (asks 2 to 4) before a plan is written.

## 2026-10-23: the console's launch scope

`console-operator`:
- **One deployment in open source** (ask 2): "Yep open source will have only
  1 deployment managed. Ordo will host ordo asa multi deployment deployment."
- **What it covers** (ask 3): "Yes all of those. All settings, any built in
  observability we have exposed. And if we've implemented the query
  performance tools / a dashboard for that. Ideally a page to see all caps -
  and if any caps have been triggered (maybe v2 here)". Health and deploys,
  tenants and people, sign-in settings, shares and links, the security log,
  usage, every setting, all exposed observability, query performance if
  built; a caps page (which limits exist, which were hit) possibly in v2.
- **Separate from the deployment** (ask 4): "N o - this was previously ruled
  console is serpate from the deployment - it is only a way to deploy,
  observe, and manage the deployment(s) itself." My recommendation
  contradicted the 2026-09-21 ruling and is withdrawn. The console is its own
  application; it reaches a deployment only through the door, as an owner
  with a token, and it is how a deployment is deployed, observed and managed.

`a-tenant-s-pool-could-fill` (ask 1, "yes"): the wait for the dial cap gets a
time limit, as a setting.

## 2026-10-23: query performance variants

Reviewed: `docs/plan-tiers.md` (proposed 2026-08-10, never ruled) designs
several proven-equal SQL variants per op, chosen per tenant with a human's
approval. None of it is built; each op compiles to one SQL shape.
`plan-tiers-cheap`: the 2 cheap steps land before launch ("yes"): reserve a
`tier` field in each op's artifact, and record each op's observed cost,
because adding the field after launch would move every op version and end
the grants pinned to them. The full design is scheduled for Ordo v2 ("yes").

## 2026-10-23: the console's implementation decisions

`console-operator`:
- **Tenant view** (ask 5, "yes."): the deployment owner sees every tenant's
  name, status, counts and usage, not its members or shares.
- **Packaging** (ask 7): "for the public deployment I was thinking everything
  in one image. But I guess a side container is probably fine."
- **Addresses** (ask 8): "We don't own that. That's up for the dev to decide
  ... The console needs to run even if the deployment is broken." The console
  must not depend on the deployment running, and must not assume a shared
  site.
- **Settings and redeploys** (ask 9): "the mechanism should be that the
  console can redeploy the deployment as often as it needs with whatever
  variables and settings it needs." So the console manages the deployment's
  runtime too (its image, environment and restarts), not only what the door
  exposes. This reaches past the 2026-10-23 plan, which went only through the
  door; the plan is revised.
- **Caps page in v2, slow ops table at launch, rollback and apply preview at
  launch** (asks 10 to 12): "ok", "ok", "yes".
- **Stack** (ask 6): asked again. The owner's concern: the console must not
  depend on the deployment, and has its own database.

`plan-tiers-cheap` ask 3 ("yes"): usage metering counts the cost class an op
actually ran at.

## 2026-10-23: the console runs inside the one image

Owner, on the revised console plan:
- **One image, the console supervising the deployment inside it** (asks 13,
  16): "within the image we deploy the console ideally is within the same
  image - not how standard pods are run - but it is for the simplicity.
  People can work their deployments as they see fit." "we can spawn new
  processes within the image itself as part of its workload. It's not
  industry practice but it checks all the boxes and is not unheard of." "I do
  not want to provide a public driver because there are so many different
  ways to deal with scaling. Lets just do the 1 image for now. And we can
  provide independent image builds that they can work with however they see
  fit." So: no Docker or Kubernetes driver; the console redeploys by
  restarting the deployment process it runs; separate deployment-only and
  console-only images are offered for people who run them their own way.
- **The console's database** (ask 14): "There was not supposed to be a shared
  database cluster for the console. The console has its own cluster it can be
  connected to ... It lives within its own db instance within a cluster."
  Postgres, its own database; SQLite is not the default, because persistence
  and moving data between nodes are unreliable for it.
- **Console key plus your touch** (ask 15): accepted; "My biggest concern is
  having a failsafe if people get locked out". Asked as ask 17.
- **ordo-react** (ask 6): "Ordo react implies needing backend ordo
  components otherwise why are we using it if not using Ordo data?" Asked
  again with that in view.

Same day, `console-operator`: the console page uses ordo-sdk for its calls
through the console server, plain React otherwise (ask 6, "ok sounds good");
the lockout failsafe is `ordo console recover` inside the image, plus
recovery codes and a second console owner, and it is documented (ask 17,
"yes document it").

Same day: "several copies is not v2 - it is for internal ordo (cloud)
service." Running and rolling out several copies of a deployment is not on
the open-source console's roadmap at all; it belongs to Ordo's own cloud
service, in the private operator. Removed from the plan's v2 list.

## 2026-10-23: busy versus broken, through one interface

`sign-in-answered-store-unavailable-when-the` ask 1: "Yes - but this should be
done through some interface rather than individual spots." Every sign-in path,
and every SDK sign-in call, tells a busy server from a broken store and
retries busy; done through one shared error type and one answer function on
the server, and one shared request helper in the SDK, never spot by spot.

Same day, the owner on the above: "This should be a general practice and
aligns with DRY principles - which is in very much a way exactly what Ordo is
solving". Made a standing rule (AGENTS.md rule 9): a fix or rule that applies
in many places goes through one shared interface, with a guard test so no new
place bypasses it.

Correction, same day: "That's the wrong lesson to add. In general any
parotuclar service we interact with should be through the same interface as
opposed to indiviudal layers of interaction and code and custom error
handling." AGENTS.md rule 9 now says it that way: each service (database,
mail, file storage, outside sign-in providers, the door seen from the SDK) is
reached through one client with one typed error that keeps its cause and says
what a caller can do; one place at the edge turns it into an HTTP answer.

`audit-one-interface-and-one-typed` (2026-10-23): an audit of every service
boundary against rule 9 runs before any Ordo v2 work, report first ("yes"),
fixes built after the owner rules on them.

`audit-one-interface-and-one-typed` (2026-10-23): the 6 bugs fixed now ("yes
now. It seems that mail send should be queued."); the structural order as
proposed ("sure. Document.", written into docs/audit-service-interfaces.md);
`#[non_exhaustive]` on every public error enum and ordo-react importing
ordo-sdk ("Yes").

## 2026-10-23: decrypt failures, and runner credentials later

- **A decrypt failure is not a destroyed key** (`audit-one-interface-and-one-typed`
  ask 5): "Yes. Having a way to see that last succesful decrypt could be
  useful for debug." A sealed value that fails to decrypt while its key is
  present is reported as unreadable, possibly tampered, and alerts the
  deployment owner; "destroyed" means only a deliberately erased key. Ordo
  keeps the time of the last successful decrypt, per key epoch, for
  debugging.
- **CI runners on a GitHub App** (`move-the-ci-runners-from-a`): "In the
  future. Not today. File this for v2 roadmap." Filed for Ordo v2.

Same day, on the last-successful-decrypt time: the owner asked what happens
after a container restart. It is kept per space and key epoch, flushed to
`$decrypt_seen` every `ORDO_DECRYPT_SEEN_SECS` (default 60) and at shutdown,
loaded at boot, with the first failure since the last success; shown on
`/health/deep` under `sealed_decrypts`.

## 2026-10-24: console recovery, succession, runner disk

Owner, on `console-operator` and the runner eviction:
- **A console recovery code retires the old console passkeys, only once a
  new one is enrolled** (ask 18). Refined the same day: "they shouldn't be
  removed unless there's a new way to sign in. This needs to be explicit
  otherwise you end up with a locked account." Retiring happens in the same
  transaction as the new passkey's enrolment, never before it, and never to
  zero passkeys. First wording:
  "I believe we already answered this previously." It had been, for the
  app: a recovery retires the old passkeys, which cannot sign in, and the
  sweep removes them after the contest window (`howto-operating.md`,
  "Contesting a recovery"). The console follows the same rule.
- **Succession is held** (ask 19). The owner's follow-up: "The real
  question here is what would the app dev have without the console. How
  does that work with grants, how would it affect privacy and sovereignty.
  Visibility? Auditability?" The console leaves succession out until
  `owner-succession-what-the-app-developer` is answered. First wording:
  "it is only accessible by the app dev - so tenants where you are the
  owner of doesn't really make sense." The console offers it as such, in
  the deployment's own space, needing no role in the tenant.
- **CI runners' `docker-lib` is 25Gi** (`ci-test-job-evicted-runner-docker-lib`):
  "25GiB". Set in `smg_infrastructure` `gha-runner-values/runner.yaml`,
  shared by every pool.

### Owner succession (`owner-succession-what-the-app-developer`, all three)

Owner, 2026-10-24, each "Ok" on the recommendation:
1. **The tenant's managers are told of a succession and may object during
   the delay**, and this is built before the console offers succession.
   Without it a tenant learns of a new owner only after the fact; the rule
   was already planned (`plan-passkey-recovery.md` ask 8) and never built.
2. **The console's succession form takes the successor's id from the
   deployment owner, with no list of the tenant's members.** The deployment
   owner holds no role in most tenants, and a member list would show who is
   inside.
3. **Completing a succession ends the departed owner's grants to apps and
   their delegations**, an option on by default. The record names each one
   ended; the new owner can grant again.

## 2026-10-24: a person's own download, past the cap, goes to v2

Owner, on `a-person-s-own-download-past`: "Move all of those including the
asks to v2." Parts past `ORDO_OWN_DOWNLOAD_MAX_BYTES`, compressing the rows,
writing rows a page at a time, resuming a part by byte range, and counting
the hourly limit across nodes are all Ordo v2. The five asks stay open for
then. Until then the download is one stored zip of at most the cap, a file
past it listed missing and the manifest saying `"complete": false`.

## 2026-10-24: files served safely, linked by signed links, rendered by choice

Owner, on serving stored files:
- **No second domain is expected of a deployment owner**: "exepcting
  newcomers to have multiple domains is not yet a likely solution for them
  - so they short lived signed links makes sense - it was the original
  method I was using for denizens.ellanor storage in r2." Signed links are
  the direction (`short-lived-signed-links-for-files`); when they come is
  asked there.
- **Render or download is the app developer's choice**: "Render vs provide
  is up the to the app dev and theres good cases for either situation."
  The door's safe list is only the default when the app has not chosen;
  the per-field choice joins the v2 upload rules
  (`upload-rules-set-per-file-field`). The sandbox CSP applies to every
  rendered file whatever the choice, so a rendered HTML file runs no
  script.
- **Per-field size limits are v2**: "So for V2 100% we need to make that
  configurable by field."

### Signed file links (`short-lived-signed-links-for-files`, all three)

Owner, 2026-10-24:
1. **Ordo v2**: "V2". Until then files are shown through the SDK with a
   token.
2. **An unsealed file's link is the storage provider's own presigned link
   (R2, S3), and a sealed file's is the door's**: "Yes". The provider's link
   serves from a separate domain at no cost to the deployment owner; a
   sealed file is ciphertext in the bucket and must pass through the door.
3. **5 minutes by default, a setting the app developer may change within a
   ceiling**: "Sounds good."

## 2026-10-25: rulings from the security round

- **Guessing limits are the app developer's, and a tenant's where tenants
  run their own sign-in; Ordo sets a default** (`guessing-a-password-or-a-link`
  ask 1): "This should belong to the app dev, and even more so - a tenant
  choice if multitenant environment is in use and these tenants have their
  own sign in requirements, but we can set a default." The default is an
  always-on slowdown after wrong passwords and wrong link passwords, per
  account or per link, across nodes. The 2026-10-22 lockout ruling stands:
  the lockout stays off by default.
- **A link password is at least 8 characters** (ask 2): "Yes".
- **Strangers get the health status alone; the deployment owner's token
  gets the body** (`what-health-checks-tell-a-stranger`): "Yes". This
  narrows who reads 17t's body; what the body reports is unchanged.
- **The matrix's three open answers** (`three-answers-the-matrix-found-no`):
  a private file another person may not read answers `not_found`, the same
  as a missing handle ("ok"); link, guest and agent tokens may not list a
  person's tenants ("agreed."); a dead token is refused before the door
  says whether a live stream exists ("ok").
- **A contest removes every second factor and passkey enrolled since the
  case opened** (`what-a-contest-undoes-after-a`): "ok". The record names
  each one removed.
- **After a contest** (`after-a-contest-what-comes-back`, 2026-10-25): a
  contest removes every sign-in method linked since the case opened, not
  only passkeys and second factors ("ok"), and resets sign-in settings the
  thief changed during the case to their value when it opened ("ok"). The
  owner's own second factors from before the case are enrolled again; their
  secrets are gone.

## 2026-10-26: the index fix goes first

Owner, on the usability and performance audit: "Let's put this on the board
and begin executing. Put the rest of the findings on the board too." The
guard equality fix (`guard-equality-uses-no-index`) goes first. Its rule,
settled in conversation the same night: plain `=` only where it gives the
same answer as `IS NOT DISTINCT FROM`, proven per expression; one never-null
side is enough in a positive check; negated checks spell out the null case;
anything unproven keeps the old form. The proof is checked by tests on CI,
not at runtime, and a deployment refuses to boot if a `NOT NULL` it relies on
is gone.

## 2026-10-27: deployment people, from the back end

Owner, in conversation, refining `deployment-owner-identity` (2026-10-01):

- **Everything is set up from the back end.** *"I will be deploying ordo for
  ordo. I'll need to be able to set up ordo instances for people
  programmatically without their need to do anything."* No one claims a
  deployment from its front page: *"This isn't 1999."* The first-person claim
  code goes.
- **Deployment people live in the deployment's own space**, apart from the
  app's tenants and users, with their own credentials. There may be several:
  *"there could be multiple people that are authenticated to apply ordo, test
  new branches"*. The door on the Ordo server is the authority; the console
  and the CLI are both clients of it.
- **One admin role for now.** *"We'll have to get into permission breakdown
  later."*
- **Every act through the door is recorded with who triggered it**, *"an
  ordo apply, an ordo whatever"*, in an audit log that is *"unchangeable and
  set in stone"*, though *"it doesn't have to be super strict"*.
- **CLI tokens are issued by the Ordo server**; the console is a good place
  to mint them.
- **Open:** how deployment people sign in (passkey only, an outside identity
  provider, an identity provider inside the deployment). The CLI follows
  from that. Asked on `deployment-people-sign-in-machines-cli-and`.

## 2026-10-27: Ordo Cloud v1 is plain Ordo, hosted

Owner, in conversation:

- **v1 is "basically just ordo, but we host it for them"**: the separate
  images, a full console for the customer, one admin role. Settings tied to
  the hosting (domain and origins, database, ports and TLS, version) live in
  the container's environment, which a door-only console cannot change.
  Pinned settings and a custom cloud interface stay later options.
- **The host can read what it hosts**: *"if I'm hosting the database,
  technically I could read whatever I want. Unless it's encrypted by their
  keys, but if I manage in the keys anyways, well, that's just what having a
  hosting provider is."* Ordo does not promise customers otherwise for v1.
- **No support account for now.** A support window (a super admin the
  provider can use) was weighed: easier support, but it *"would also make
  them uncomfortable"*. *"we'll just leave it as admin for now and see where
  things go."* Filed for later as `support-window`.

## 2026-10-27: deployment accounts, for now

Owner, after weighing a built-in identity service shared by the console, the
deployment and the CLI: *"as Ordo develops ... it will quickly open the light
into how we might want to reframe this part of the system. And so I think we
should keep it as two separate accounts for now until I actually know what
direction it makes sense to go in."*

- **Two account lists.** The console keeps its own sign-in, independent of the
  deployment. Deployment accounts (what the door and the CLI act as) are
  separate. A shared identity service, or two realms in one directory, stays
  an option for later.
- **The console creates deployment accounts**, by invite link: *"You go to the
  link and you sign up. The user decides where to paste it."* The link works
  once, expires, can be revoked from the console, and records who made it and
  who used it.
- **Email later.** *"Ideally, I'd like to send an email, but ... before an
  email is set up, you need to actually set up the SMTP settings."* So the
  console copies the link; sending it by mail can be offered once mail is
  configured.

## 2026-10-27 (later): one identity service, in the one image

This replaces "two account lists for now", earlier the same night.

- **The thing that manages must not depend on the thing it manages.** *"if
  the thing that manages the thing depends on the thing its managing its no
  bueno."* And the console is *"assumed but not guaranteed"*: a deployment can
  run without it. So neither the console nor the deployment holds people for
  the other.
- **A third service holds deployment people**: *"3rd service that manages the
  connectivity makes the most sense and that would also allow the cli login
  through the browser (session)."* The console and the deployment both trust
  it. The CLI signs in through the browser to it and calls the deployment's
  door directly. The first admin of an instance is made there, on the server.
- **It is not a full identity provider**: *"just something that satisfies the
  needs of ordo"*, built into the image, *"no extra hosting needed"*, and
  usable from the CLI.
- **One image, everything in it**: *"What we provide for now is everything in
  one. The binaries and everything else will be available if someone wants to
  make a different docker image."* A small supervisor runs the deployment, the
  console and the identity service as siblings, so no one of them takes
  another down. The separate deployment-only and console-only images are no
  longer something Ordo provides.
- Machine keys and a signed audit log by default: "ok" (same task).
- **Ordo builds its own identity service, the bare minimum** (after weighing
  Dex and Ory Hydra): *"Lets just make our own - the bare minimum."* Dex lacks
  passkeys for local accounts and duplicates the outside-provider code Ordo
  already has; Hydra would add a Go binary to own the one missing part. The
  minimum: passkey accounts for deployment people, invite links with expiry,
  access that can end, machine keys, the first admin from the provisioning
  call or first boot, browser sign-in for the CLI with a renewable session,
  short-lived signed tokens the deployment and the console verify with its
  public key, a host-only break-glass path, and every act in its audit log.
  Outside providers can come later through the client code Ordo has.
- **It is v1, not v2**: *"we need it to be able to address the hosting and
  the CLI login"*. Ordo Cloud's back-end provisioning and `ordo login` both
  depend on it.

## 2026-10-27: the identity service plan, 10 answers

On `docs/plan-identity-service.md` (`identity-service-plan-10-questions`):
Q1 the identity database lives on the console's Postgres cluster, as its own
database ("yes"). Q2 a machine key may do platform writes that need a touch, in
the deployment's own space ("yes"). Q3 ending access takes effect at the
10-minute token expiry, no live revoke check in v1 ("yes"). Q4 deployment
admins keep the read-only tenant admin views ("ok"). Q5 `ordo login` by
browser and local callback, device code later ("ok"). Q6 the claim code goes;
app tenants keep their own first-comer setting ("yes"). Q8 the service has its
own address and port ("yes"). Q10 the console restarts only the deployment
("ok").

- **Q7, upgrades affect nothing**: *"they shouldnt need to re-enroll??.
  Upgrade should affect nothing"*. Existing console owners and their passkeys
  move to the identity service on upgrade, with no action from them. The plan
  must keep the passkeys' relying party the same so they still work.
- **Q9, CLI session lifetimes**: *"7 day idle 30 days most default -
  configurable ( I figure 24 hours / 12hours will be the most common). Invites
  7 days default."* Tokens stay 10 minutes.

## 2026-10-27: Ordo mass-scale single-tenant application, a v2 design

Owner: a plain app where tenants make no sense *"is supposed to be a big case
as well"*. It need not do *"the most intensive things at every layer for every
number for every app"*, but must be able to reach the scale. Most users only
read, and much of that should be cached at the edge, beyond the public path
(*"more than not public path for unchanging data"*). Filed as one v2 design
ticket, *"we'll call it ordo mass scale single tenant application"*: the walls
of a single big space, and edge caching for reads that are not public.
`horizontal-scaling-before-v1` stays before v1.

## 2026-10-27: tenant database grouping, a v2 ticket

Owner, after the per-database cost at a million tenants was raised: *"new
ticket for v2 joint tenant database grouping which allows grouping tenants
into a singular database on some cluster and the grouping is at the
discretion of the deployment and they can have as many groupings as they
want. As long as one tenant to one database remains and then you know they can
have the table names whatever makes sense there."* A tenant is never split
across databases; a database may hold a group of tenants. This amends the
2026-09-25 "no single-database mode" ruling for v2 only, as the deployment's
choice per group. v1 is unchanged: one database per tenant.

## 2026-10-27: identity service step 1 reviewed; door keeps the last good member list

- **Token as issued** (owner: *"ok"*): `iss`, `aud: ordo-deployment:<handle>`,
  `sub`, `kind`, `role`, `client`, `sid`, `touched_at` (people only), `iat`,
  `exp` 10 minutes, `jti`; no name or email. It carries only what the door
  decides on.
- **Audit keys** published at `/.well-known/ordo-audit-keys.json`, apart from
  the sign-in keys, so rotating sign-in keys never breaks old audit checks.
- **`ORDO_IDENTITY_DATA_DIR`** (default `/var/lib/ordo-identity`) holds the
  key file and the first invite. Owner: *"ok- reminder to mount it properly in
  the docs"*: the how-to (step 9) and the compose files (step 7) mount it as a
  persistent volume and name it in backups.
- **Machine sessions** and **a cookie with no Origin refused**: ok.
- **Console import** as built: ok (read-only on the console, one transaction,
  nobody re-enrols).
- **Guessing limits**: count per source address plus a high shared ceiling,
  so one stranger cannot shut a kind of sign-in for everyone. The secrets are
  long random codes; the ceiling only stops a flood.
- **Door rebuild on a passing failure** (deadlock, timeout, a full pool):
  keep serving the last good member list for a bounded time, default 60 s, a
  setting; retry meanwhile; deny past the bound or on a lasting error. A
  removed member keeps access at most that long, the same order as the
  session refresh. The deadlock behind it is fixed on its own.

## 2026-10-29: deployment admins keep the console's tenant operations

Asked: which tenant admin operations may a deployment admin call once the
console signs in through the identity service? Owner: *"yes"* to the
recommendation.

- Deployment admins keep the full list the console key has today
  (`TENANT_OPS_OPEN`, 35 operations, reads and writes).
- Each one needs a fresh passkey touch, as today.
- Why: moving sign-in to the service should change how people prove who they
  are, and nothing about what they may do. Narrowing the list would break
  console pages owners already use.

## 2026-10-29: the token-efficiency benchmark, first answers

Plan: `docs/plan-token-benchmark.md`. Board: `a-token-efficiency-benchmark-ordo-against-conven`.

- **Stacks:** *"All 3"*. Ordo against Supabase, plain TypeScript on
  Postgres, and Convex.
- **Win:** *"sure"*. Ordo claims a win only at no more than half the best
  baseline's cost per passing app, with a permission pass rate no lower.
- **Vendor AI docs:** *"Yep"*. Baselines run with and without them; Ordo is
  compared against the better.
- **Publishing:** *"Save all results - but we will publish - lets log and
  save as much data/telemetry as possible for provability if neeed"*. Every
  transcript, usage record and test result is kept. This replaces the plan's
  "raw transcripts stay internal".
- **Budget and models:** *"we'll do what I can afford which is as close to
  free as possible. So just using claude code for all 3."* The runs go
  through Claude Code on the owner's plan, so they spend its usage allowance
  rather than API dollars.
- **Scope:** *"We still need the test case - will be building a functional
  app. Complexity should be close to allowing me to have free. We can add
  mroe tests later if I have funding."* One small, working app first; the
  10-task set waits for funding.
- Why: the claim needs a number before it is published, and a small honest
  run on one real app beats a large plan nobody can pay for. Asked again on
  the board: which app, which model, and how to pace the runs.

## 2026-10-29 (later): the token benchmark's first run

- **App:** *"Agreed"*: a small team task tracker (teams, members, tasks
  with an owner, one team never seeing another's tasks).
- **Model:** *"Each run gets the ability to run 1 large plan with fable.
  using their stack's guidance. Once the plan is complete then
  implementation is with sonnet."* Each trial is two phases: one Fable
  planning pass with the stack's own AI guidance, then Sonnet builds from
  that plan. Both phases' tokens count.
- **Pacing and stacks:** *"I need this out asap like - so for now lets just
  compare against supabase. If we can slightly reduce the scope of the
  project that would help."* The first run is Ordo against Supabase only,
  with the app trimmed. Plain TypeScript and Convex follow later. This
  narrows the earlier "All 3" for the first run; it does not drop them.
- Why: a first honest number soon is worth more than a full comparison
  later, and a plan-then-build split matches how agents are used in
  practice.
- **Interface:** *"comparabiltiy of the interface matters."* Both stacks
  build the same user-facing web app from one spec (React, same screens,
  same labels), and one shared browser test suite checks both, alongside
  the permission and isolation probes. Ordo's SDK and React pieces may be
  used, as Supabase's client may.
- **Supabase locally:** the owner installed Docker Desktop (2026-10-29), so
  Supabase runs on this Mac with its CLI.

## 2026-10-30: the token benchmark's win is decided by cost

- Asked: which number decides the win, raw token count or list-price cost?
  Owner: *"cost is fine but we should keep track of both."*
- List-price cost per passing app decides the win. Raw tokens (input,
  output, cache read, cache write) are recorded and reported beside it for
  every trial.
- Why: cache reads are most of the tokens but cost a tenth of fresh input,
  so raw counts overstate whatever rereads the most; showing both keeps the
  result checkable either way.

## 2026-10-30: more than one copy, and high availability, move to v2

- Owner: *"for more than one copy lets move that for v2. (I assume this is
  for HA)"*. Yes: running more than one copy of the door is both how a
  deployment scales out and how it survives one copy dying.
- v1 is published as one copy per deployment, and the docs say so. The
  known limits stay recorded on `horizontal-scaling-before-v1` for v2: key
  custody read once at boot (a tenant made after a copy starts is not served
  by it), leader failover untested, the guessing slowdown tested on share
  links only, and no total connection budget.
- Why: it is not needed to publish, and it is the largest of the remaining
  items.

## 2026-10-30: two step 7 and door answers

- **A door behind its database:** while a tenant cannot be read, any
  signed-in caller gets `503`, so a non-member can tell the tenant exists.
  Owner: *"We'll accept 503 - but for the future in v2 we want a better
  solution"*. Accepted for v1; a v2 item to answer without that hint.
- **The sign-in service's address after an upgrade:** *"ok"*. With no
  separate address set, the console serves the service under its own origin.
- **The sign-in service's database:** *"no. Database needs to be explicit,
  or there is an optional ENV / setting that enables using the same db
  server."* The service never makes a database by itself. The owner names
  one, or turns on a setting that lets it use the console's database server.
  Read with the upgrade rule: an upgraded deployment that has neither keeps
  running as before (the console's own sign-in), and the console and boot log
  name both settings, until the owner chooses. This holds until step 8.

## 2026-10-30: a fair start for the benchmark, and an Ordo skill with op lookup

- Asked in conversation after round 1: how do we level the field for a
  stack the model was never trained on? Owner: *"we start at the same place.
  A skill and everything already installed. And for ordo a running server
  since supabase also gets a running service."*
- Each stack starts with its vendor's published skill installed, its
  packages installed and its backend running. Supabase gets its official
  skills (`supabase/agent-skills`), pinned to one commit; we do not write
  one for it (owner: *"But we're not making a supabase skill right"*).
- Owner: *"Lets go ahead and try implementing this, Worst case scenario we
  change what we render."* Build an Ordo skill that ships with the product,
  plus an op index written from the definition and a lookup command.
- The lookup's rendered output is not a compatibility promise. An app's
  own ops are the app developer's choice (owner: *"If so that's the App
  Devs choice"*), so what `ordo ops` shows follows whatever they define.
- Why: round 1's extra Ordo cost was the planner reading the SDK and the
  generated client for the first time, at Fable's highest token rate. A
  derived, exact lookup answers from the definition in a few lines and
  stays the same size as the app grows.

## 2026-10-30: step 8 keeps a deployment that has not moved working

- Asked: after step 8 removes the old sign-in, what does an upgraded
  deployment with no identity database do, and does the door keep reading
  `ORDO_DEPLOYMENT_OWNERS`? Owner: *"ok"* to both recommendations.
- A deployment with no identity database keeps the console's old sign-in,
  with a banner and a boot-log line naming the 2 settings. Step 8 removes
  the old pieces from new deployments and from any deployment that has set
  the database. A later, announced release removes them for good.
- The door keeps reading `ORDO_DEPLOYMENT_OWNERS` until that deployment has
  the identity service. Then the setting is ignored with a log line naming
  it, never a refusal at boot.
- Why: the upgrade rule. An upgraded deployment must start and sign in as
  before, with nothing to run by hand.

## 2026-10-30: benchmark round 2, and op descriptions from comments

- Owner: *"ok"* to all four benchmark asks. Round 2 measures a change
  request on the finished app, both stacks. Clearing an optional reference
  is made easier in the SDK and said in the skill. Round 2's rules are
  committed before any run: frozen skill, a holdout app, alternating runs,
  5 or more trials, one price table, every run reported.
- The no-plan arm: *"ok. The cost doesn't seem high in general but only if
  it makes Ordo's comparison more fair."* Read as: it runs on both stacks
  and is reported beside the result; the win is still decided on the
  Fable-plan arm, so the arm can inform and cannot tilt the verdict.
- Op descriptions: *"ok"*. A `#` comment written just above an op becomes
  its description in `ordo ops`, `OPS.md` and the generated client. No new
  syntax.

## 2026-10-30: the claim code goes now

- Asked: keep the claim code until the announced release, building only
  the home space's refusal of first-comer sign-up now? Owner: *"No Remove
  it now."*
- Step 8 removes the claim code (`ORDO_AUTH_CLAIM_CODE`,
  `std.deployment.setClaimCode`, the claim store) on every deployment, as
  the plan's §6 table says. First-comer admission for app tenants stays as
  the app developer's setting, off by default, and the home space refuses
  it (plan Q6, ruled earlier).
- A sign-up the claim code used to admit is refused by a message naming the
  first-comer setting, so a deployment owner who wants it back knows what
  to set.

## 2026-10-30: the benchmark is not a release blocker

- Owner: *"We'll skip the benchmark as a release blocker. We'll do that
  after."*
- Publishing waits on identity step 9 and the fresh-install check from the
  docs. Benchmark round 2 runs after the release, under the round 2 rules
  already ruled.
- Why: the release is what goes public now; the benchmark result is
  published when it is measured, and nothing in the product waits on it.

## 2026-10-30: the repository goes public as it is

- Owner: *"As soon as we're cleaned up, git ignore updated, and good enough
  - make the repo public"*, after leaving what to ignore *"at your
  discretion"*.
- The existing repository is made public with its history. The audit found
  no real credentials in the tree or the history (only test fixtures); the
  history holds scratch files and local paths now untracked, which are
  harmless. Keeping the history shows how the work was done.
- The board files stay. A skim of the answer lines the audit flagged found
  product reasoning only, and they are visible in the history anyway.
- The CLI loses its default host (the owner's live deployment); a command
  that needs one names `--host` and `ORDO_HOST`.
- CI's self-hosted runners never run for a pull request from a fork: a
  job-level guard in each workflow, and GitHub's approval for all outside
  contributors once the repository is public.

## 2026-10-30: work lands on bleeding, the owner merges to main

Owner, on the branch ruleset that began blocking every push once the repo
went public: "I can do a bypass when needed - Let's make a bleeding branch."

- The owner narrowed the ruleset, now "Protect", to the default branch. On
  `main` it keeps: pull request, 1 approval, linear history, no delete, no
  force-push.
- Agents push to `bleeding`, which no ruleset covers.
- CI, Notes, Console and Identity e2e run on pushes to `bleeding`. Images do
  not, so `latest` on GHCR still comes from `main` alone.
- The owner then gave the repository admin role a bypass on that ruleset.
  When all 4 runs on `bleeding` are green, `main` is fast-forwarded to it
  by that bypass. Nothing reaches `main` without a green run.

Why: the ruleset is the owner's control on what reaches the public `main`
and the published image. A second branch keeps that control intact and lets
work flow without a second reviewer.

## 2026-10-31: a localhost default host, the skill's licence, no live rehearsal, the counter write

- **Default host.** Owner: *"yes. For now make it a localhost one. I need
  to still purchase and choose a domain."* An empty `--host` selects
  `http://127.0.0.1:4000`, where `ordo dev` serves. The database path stays
  the CLI's default; a door command still needs `--host` or `ORDO_HOST` to be
  given. Why: a local default can never send a command to someone else's
  machine, and a public domain replaces it once one is chosen.
- **Skill licence.** Owner: *"ok"*. `skills/ordo/` is Apache-2.0, like the
  docs and the SDKs, since `ordo skill install` copies it into app repos.
  NOTICE and the SKILL.md frontmatter say so.
- **No live-copy rehearsal.** Owner: *"Nothing has been truly deployed so
  anything existing in smg_infra can be replaced/updated without care."*
  Identity step 9 is complete without it.
- **Create-or-increment on a unique key.** Owner: *"ok"*. The compiler
  accepts `set counters { key: $k, n: n + 1 }`; a create reads each field
  it uses as that field's default. Why: it is SQL's `ON CONFLICT DO UPDATE`
  counter, the language doc already promised it, and refusing it leaves
  agents writing a read and then a write.
- **The fresh export.** Asks 1 and 2 on `making-the-repository-public` were
  answered after the repository had gone public with its history and its
  board files. That conflict is back on the board as asks 7 and 8; nothing
  is changed until it is answered.

## 2026-10-31: the public repository restarts from one commit

- Owner, on ask 7: *"It's not cloned Can we just orphan the existing branch
  and force push - if that work enough we can do a 2 repo swap"*. On ask 8,
  whether the board's files leave the tree: *"No"*.
- An orphaned, force-pushed `main` is not enough. GitHub keeps
  `refs/pull/1` to `refs/pull/4` for good, so the whole old history stays
  fetchable, and the closed pull requests show commits on no branch.
- So the swap: the repository is renamed `ordo-history` and made private,
  keeping its history, pull requests and runs. A new public `HominemAI/ordo`
  starts from one commit holding today's tree, board files included.
- No repository secrets exist, so only settings move: fork approval, the
  `main` ruleset with the admin bypass, and package access for the images.

## 2026-10-31: backend functions are v1

- Owner: *"I'm not looking for the backend service deifnitions - that was
  planned for v2. For V1 we had backend function calls using WASM rust for
  microsecond execution time."*
- The 2026-10-11 answer to ask 7 (*"Yes for V1"*) was recorded as the first
  version of functions, still after go-live. The owner meant Ordo's v1. So
  function calls (wasmtime in the Ordo binary, a fresh instance per call)
  are v1 work, in the ruled order: increments 1, 2 and 4 first.
- Long-running services (`design-backend-functions.html` §6) stay v2.
- Nothing was built before this correction; the plan and design stand.

## 2026-10-31: the new repository's packages and upgrade baselines

- **Packages.** Owner, on giving the new repository access to the 3 GHCR
  packages: *"can we just delete them and push new?"* Yes: the owner deletes
  `ordo`, `ordo-cli` and `ordo-playground`, and the first push of `main`
  makes them again, linked to the new repository. The live deployment loses
  its images until then, which the owner ruled acceptable (nothing has truly
  deployed).
- **Upgrade baseline.** Owner: *"ok"*. `scripts/upgrade-continuity.sh`
  upgrades from the repository's first commit, the first public release,
  until release tags exist. The old pins (`ef37d9de`, `42010b4e`) live only
  in `ordo-history`.
- The identity upgrade e2e starts from a console that predates the identity
  service, which only `ordo-history` holds. It now runs only when started by
  hand with an `old_ref`; whether to retire it is ask 12. Owner: *"ok"*.
  The job, its spec, its Playwright config and the 3 rehearsal scripts it
  shared are removed.

## 2026-10-31: staging is always its own deployment

- Owner, on backend-functions ask 15: *"yes"*. Secrets are kept per
  deployment (`$secrets`), so each environment is its own deployment, and
  staging is never run inside production's. There is no `--env` flag.
- Why: staging's secret can then never be production's, and ruling 5 of
  `plan-environments.md` already gives staging its own keys.
- When environments are built (27e to 27h), dev, staging and production are
  three deployments, and `ordo apply` moves a version between them.

## 2026-10-31: functions at /fn/<name>, and Boa as the JS engine for v1

- **Route.** Owner, ask 16: *"ok"*. Functions are served at `/fn/<name>`,
  beside `/op/<name>`. Versioning lives in each function's code hash.
- **Engine.** Owner, ask 17: *"ok no wasm? i thought it was faster"*. It is
  WebAssembly: Boa, a JS engine in Rust, is compiled to Wasm and runs in a
  fresh wasmtime instance per call (about 80 µs to make one, 0.2 to 0.3 ms
  for a small call). StarlingMonkey is SpiderMonkey compiled to Wasm; it
  interprets heavy JS faster but needs Node, jco and a C toolchain at
  apply. Boa suits short glue code; a swap later changes no declaration or
  call. Code compiled straight to Wasm (Rust, increment 10) is the fastest
  kind of function.
