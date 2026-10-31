# Plan tiers — adaptive optimization with a human in the loop

Status: **[proposed]** — the design for "queries get faster over time, but only
when someone says so." Owner's idea; this document is the worked version.

## Why Ordo can do this and most systems cannot

An ordinary database re-plans continuously and invisibly: the optimizer picks a
plan per execution, and yesterday's fast query is today's table scan because a
statistic moved. That is a support burden dressed as a feature — nobody
approved it, nobody can diff it, and the only evidence is a latency graph.

Ordo already has the four things that make the honest version possible:

1. **Ops are compiled artifacts stored as data**, keyed by version hash. A
   different plan is a different artifact, not a different mood.
2. **Hot swap already exists.** New version, atomic pointer move, in-flight
   requests finish on the old one, rollback is re-pointing. Re-planning needs
   no new deployment machinery.
3. **Cost is already predicted statically** — every op carries a cost class.
   So there is a *prediction on record* to compare reality against, which is
   the signal this whole feature runs on.
4. **The conformance corpus exists.** A re-planned artifact can be *proven* to
   return the same rows as the one it replaces. That is the difference between
   "trust the optimizer" and "the suite says it is the same query."

## The signal: predicted versus observed

Every execution records observed cost against the artifact that ran: rows
examined, wall time, host-side decrypt count, page-fill depth (how far a keyset
walked to fill a page), and how often the op ran. The platform compares that to
the artifact's static cost class.

**The finding is the gap, not the absolute.** An `unbounded-host` op taking
four seconds is behaving as documented; a `sql`-class op taking four seconds is
a lie on the artifact. Drift is what surfaces — the same thing deploy diffs
already surface for cost-class changes, now measured instead of predicted.

## Variants are peers, not rungs

The framing that matters most, because it is easy to get backwards: these are
**not** a ladder you climb to go faster. If one plan were universally best the
compiler would emit it always and none of this would need to exist. **The
mechanism exists precisely because no plan wins everywhere** — each variant has
a *regime*, a set of conditions under which it is the right answer and outside
which it is the wrong one.

- `DISTINCT ON` is not a lower rung than `LATERAL`: it wins when groups are few
  and loses when they are many.
- Materialized is not strictly better than direct: it buys reads with write
  amplification, so it *loses* on a write-heavy space with modest reads.
- Client venue is not better than `sql`: it wins for sealed ranges and loses
  for anything the mirror does not hold.

Three consequences:

1. **The default is not "start low and escalate."** It is the variant the
   compile-time rule picks from the schema alone — for a get-by-id there is
   only one; for a top-N it is whichever the declared indexes imply.
   Per-space statistics *refine* that choice; they do not climb from it.
2. **The prompt is "wrong regime," not "upgrade."** The dashboard says *this
   op is running a variant outside its regime, and here is the measurement
   showing which regime this space is actually in* — which can also say **move
   back**, something an escalation model cannot express.
3. **Every variant publishes its regime** in the artifact: the conditions under
   which it is correct. That is what makes selection auditable instead of
   magical — you can read why a space is on `LATERAL` and check whether it is
   still true.

## Selection is `(space, op)`, resolved in the lookup that already happens

Compilation emits every legal variant for an op **at deploy time**, so all of
them are corpus-verified together, once. Nothing is ever recompiled in
production, and switching is a pointer move rather than a build.

The selection is a stored per-space row keyed `(space, op)` → variant — **per
op, not per space**, because one space will want different variants for
different routes, and every route is a different op. The door already resolves
op → artifact on every call; it resolves `(space, op)` → artifact instead. No
new hot-path work, no decision made per request: the *choice* is made by a
background job when statistics drift, and the *request* only reads it.

## The variant kinds

Same op, same name, same contract, same results. Only the plan differs.

| Tier | What it is | What it needs | What it costs |
|---|---|---|---|
| **T0 — direct** | The obvious compilation. What every op is today. | nothing | — |
| **T1 — index-assisted** | The same plan with an index that makes it cheap. | a DDL change | index maintenance on write |
| **T2 — materialized** | A routine maintains derived state (rollup, closure table, counter); the op reads it. | a generated routine + a table | **staleness**, which must be disclosed |
| **T3 — restructure** | Cannot be fixed by planning. The op needs different authoring. | a human editing `.ordo` | the advisor explains; it does not act |

**Long traversals get their own answer.** Deep `under`/`above` work is the
canonical T2 case, and the platform already maintains materialized paths for
tree entities — so the T2 plan for a subtree aggregate is a closure table
maintained incrementally by a generated routine, not a recursive walk per call.
This is the "different way to do it from the standard" the tier system exists
to reach: the standard plan is right for a shallow tree and wrong for a deep
one, and only observation can tell you which you have.

## Where formulation genuinely matters (and where it does not)

**Not join order.** That is what Postgres's planner does well, with live
statistics we do not have. Pinning an order would usually lose. We do not
compete there.

What matters are formulations Postgres **cannot rewrite between**, where the
choice is ours because we emit the SQL:

| Construct | Variants | Regime that decides |
|---|---|---|
| `take[keys] N` (top-N per group) | `DISTINCT ON` · `LATERAL` · `ROW_NUMBER()` | few groups → `DISTINCT ON`; many groups **and** an index on `(groupKey, sortKey)` → `LATERAL`; otherwise `ROW_NUMBER` |
| `match any` / `match none` | `EXISTS` · `IN` · `JOIN`+`DISTINCT` | selectivity and whether the child side is indexed; `JOIN`+`DISTINCT` is usually the worst and is emitted only where the others cannot express the shape |
| `group` over a join chain | aggregate-then-join · join-then-aggregate | how far the many-side collapses; pre-aggregation wins when the child count per parent is large |
| `let` | `MATERIALIZED` · `NOT MATERIALIZED` | reference count and whether the body is selective — PG's default heuristic is frequently wrong for these shapes |
| wide row + selective filter | one join · two round trips (ids, then rows by id) | row width × selectivity; this is the "avoid the join" case |
| sealed staging | batch size before host decrypt | candidate count and decrypt cost |

**What the compiler knows statically**: cardinality direction from refs, which
fields are indexed, whether the sort key is indexed, the fan-out shape. **What
it cannot know**: row counts, group cardinality, selectivity — and those are
per-space facts. That split is exactly why variants compile once and select per
space.

**The selection rule is written down, not inferred.** Each variant ships with
its regime as a stated condition over a handful of cheap statistics, so an
operator can read *why* a space is on `LATERAL` and check whether it still
holds. A planner that explains itself is worth more than one that is slightly
cleverer and silent — and it is only possible because we control the
formulation, which nobody writing SQL by hand does.

**Variant explosion is the real limit.** Artifacts are ops × versions ×
variants, and every variant must pass the corpus. So variants are enumerated
only where the *shape* admits them: a top-N op gets three, a get-by-id gets
one. The set stays small and closed, and an op with one variant costs nothing.

## The second axis: *where* the work happens

The tiers above answer *how* to plan. The other half of the question is
**venue**, and the cost classes are already a venue ladder that stops one rung
short:

| Venue | What it means | Where it wins |
|---|---|---|
| `sql` | Postgres does it | almost always, when it can |
| `blind-index` | equality on sealed data, at index speed | sealed identity lookups |
| `bounded-host` / `unbounded-host` | the platform decrypts and filters | sealed ranges, `contains`, anything the index cannot answer |
| **`client`** | rows ship; the device filters against its mirror | **exactly where the host is worst** |

**The missing rung is the client, and it inverts the expensive case.** A range
or `contains` predicate on a sealed field is the most costly thing the server
can do — decrypt every candidate, `unbounded-host`, the class that fails
registration without an acknowledgment. On the device that same predicate is
free, because the IndexedDB mirror already holds those rows **in plaintext**.
The spec already blesses this for search ("where plaintext legitimately lives,
zero key releases, instant"); this makes it a venue the planner can choose
rather than a special case one feature enjoys.

Client venue is selectable when all of these hold, and refused otherwise:

- the transferred row set is **provably bounded** (it is a shipped window, not
  a stream), and the caller already has rights to every row in it;
- the predicate is a **filter, not an aggregate over data the client lacks**;
- the op does not depend on keyset pagination for the filtered result — you are
  paging the *transferred* set, not the *displayed* one;
- the mirror's freshness is acceptable, and the result says which it was.

**The honesty rule this needs.** If an op ships 5,000 rows so the device can
display 20, the scope manifest describes **what was transferred, never what was
displayed**. The caller had rights to all 5,000 — that is why they shipped — so
the manifest is already correct, and narrowing it to the visible subset would
turn a legitimate strategy into a laundering story. Transferred is the number
that goes on the consent page.

## Normalization is a plan, not a modelling decision

Ordo pushes hard toward normalized form: refs declared once, one owner, no
duplicated values. That is right semantically and expensive relationally — a
five-hop read is five joins. The resolution keeps the source clean: **the
author keeps writing normalized `.ordo`, and T2 maintains a denormalized
shadow** — a flattened read model the op reads instead of the join. The
developer never hand-denormalizes, never keeps two copies in sync, and never
inherits somebody's 2019 performance workaround as permanent schema. The
duplication is derived, disclosed, and reversible, which is the difference
between a plan and a regret.

## Two mechanical wins that need no approval at all

Not everything is a tier. Two are pure compiler work, always safe, and should
simply happen:

1. **Order predicates by cost class.** A query mixing sealed equality
   (blind-index) with a sealed range (host-side) must run the index filter
   *first*, so the expensive stage decrypts the smallest possible candidate
   set. Same results, strictly less work, no human in the loop.
2. **Suggest `indexed` where it would move a class.** "Adding `indexed` to
   `customers.email` moves `findCustomer` from `unbounded-host` to
   `blind-index`" is a T3 — a human edits the shape — but the advisor can
   compute it exactly, because it already knows the manifest and the class.

## The compiler emits the decision, not just the SQL

The architectural commitment this whole design rests on: in an ordinary
database the cost model is **code**, buried inside the optimizer, and the only
way to learn why it chose something is to read `EXPLAIN` and infer. Here the
cost model is **data, emitted at compile time and shipped with the artifact** —
which is what makes selection explainable, auditable, and serveable over the
API instead of being a black box.

Each variant therefore carries three things besides its SQL:

| Part | What it is | Used for |
|---|---|---|
| **guard** | static preconditions that must hold or the variant is *illegal* — e.g. `LATERAL` requires an index on `(groupKey, sortKey)` | narrowing the legal set at compile time, with no statistics at all |
| **score** | a formula over a closed, named statistic set — e.g. `distinct(groupKey) / rows(entity) < 0.05` | picking among the survivors, deterministically |
| **regime** | the same condition in one sentence | so the dashboard can say *why* without rendering algebra |

**Selection = evaluate guards → score survivors → take the max.** Same
statistics plus same artifacts always yield the same choice, so a selection can
be replayed and explained rather than merely observed.

### The statistic set is closed

The same handful for every op, or this becomes unbounded: `rows(entity)`,
`distinct(entity.field)`, `avgChildren(ref)`, `nullFrac(field)`.

**Postgres's own statistics will not serve.** Spaces share tables under RLS, so
`pg_stats.n_distinct` describes the *deployment*, not the space, and selecting
per space on deployment-wide distributions would be worse than not selecting at
all. The platform therefore keeps its own per-space numbers, and the cheap ones
are nearly free: **every write already passes through the command gate**, so
per-space row counts per entity are a counter increment on a path that exists.
Only the distribution-shaped statistics need periodic sampling, and their
staleness is published alongside them.

### Three stages, in decreasing certainty

1. **Static.** Guards eliminate illegal variants with no data at all. Often one
   survives and there is nothing to decide.
2. **Modeled.** The score picks among survivors from per-space statistics.
   Deterministic, explainable, right most of the time.
3. **Measured.** When the model's prediction disagrees with observed cost, that
   is the drift signal — and it is worth more than one space's choice, because
   it is evidence *the formula is wrong*. Feed it back into the formula rather
   than patching the space.

### Shadow execution: measuring without guessing

For the cases where two variants score close to a tie, there is an honest
alternative to both guess-and-check and a nondeterministic oracle: **run the
other one and time it.** The corpus already proves the variants return
identical rows, so executing a candidate in the background, or against a
sample, is safe by construction and cannot change what the caller receives.
That is measurement rather than modelling — still deterministic in the sense
that matters (reproducible, evidence-based, explainable) — and it is offered as
an operator-triggered action, never a background habit.

**Split trials** are the same idea one step further: serve *both* arms to real
traffic at a configured ratio for a bounded time and compare. Safe for the same
reason — the corpus proved the arms equivalent, so a caller cannot tell which
answered except by timing it — and deterministic because assignment is
`hash(principal ‖ op ‖ trialId) mod 100 < split` using the platform's stable
`hash`, so arms are stateless, sticky per caller, and recomputable when the
results are read. Eligibility rule: a variant that cannot accept its peer's
keyset cursor cannot be split-tested. Configuration, guardrails, and the
operator surface live in `dashboard-plan.md` §5d.

Explicitly out of scope for now: asking a model to choose. It may become cheap
enough to be worth it, and the door is not closed, but a selection nobody can
replay is not a selection this system should make.

### Surfacing it: the catalog already is the API

The capability catalog is already "what can this deployment do," already
access-scoped, already version-hashed. Variant metadata belongs there rather
than in a new surface: **per op** — the variants with their guards, scores and
regimes; **per space** — the current selection, the statistics that produced
it, and when those were last sampled. The dashboard then renders the
explanation for free, and an agent reading the catalog gets exactly the same
account a human does.

## The loop

1. **Observe.** Telemetry accumulates per artifact.
2. **Detect.** The advisor flags drift: predicted class versus measured
   behaviour, with the threshold as operator policy.
3. **Propose.** The advisor produces a candidate plan at a higher tier and a
   diff a human can read: what changes (a new index; a maintained table and the
   routine that maintains it; a staleness window), what does **not** change
   (result rows, scope manifest, op name and parameters), and what the new
   predicted class is.
4. **Prove.** The candidate runs the op's conformance cases. **Identical
   outputs or the proposal is discarded** — this is the gate that makes the
   whole thing safe, and it exists already.
5. **Approve.** A human accepts on the dashboard. Never automatic; never a
   background rewrite.
6. **Swap.** New artifact, new version hash, ordinary hot swap, old artifact
   retained. Rollback is re-pointing, like any other deploy.
7. **Record.** The re-plan is an attributed command: who made this op faster,
   when, from which tier to which, with the measurement that justified it.

## Granularity: the artifact is global, the choice is per space

The two halves separate cleanly, and the split is what keeps this affordable.

- **The candidate plan is deployment-wide.** One artifact per
  `(op, version, tier)`, compiled once and proven against the corpus once. The
  proof is about *semantics*, not data, so it holds for every space. The
  registry stays bounded — ops × versions × tiers, never multiplied by tenants.
- **The selection is per-space runtime state.** A row: "this space runs
  `projectCosts` at T2," default T0. The door already resolves op → artifact;
  it resolves `(op, space)` → artifact instead. One dimension on a lookup that
  already happens, and the same rule-in-code / facts-in-data split used
  everywhere else.

**Per-space selection is not a convenience, it is required by what the tiers
cost.** The skew lives in the space: one tenant has twelve projects, another
four hundred thousand, and the right plan genuinely differs. T2 in particular
buys reads with **write amplification** — a maintained table taxes every write
in that space — so applying it deployment-wide to help one tenant is a bill
nine thousand others never agreed to. The maintained table also holds *that
space's* derived data, which makes it custody-relevant and therefore the
owner's to approve.

The tier's nature settles its granularity; none of this is arbitrary:

| Tier | Granularity | Because |
|---|---|---|
| T1 index | per **storage target** | on shared Postgres an index is physically shared, so it is a deployment decision; on BYO-DB it is DDL on someone else's database, so it is theirs |
| T2 materialized | per **space** | it is data, in that space, costing that space's writes |
| T3 restructure | per **op** | it is source; there is one body |

**Promotion closes the loop.** The advisor observes per space, but when the
same proposal wins across most spaces that is evidence about the *default*, not
about a tenant — so an operator gets a "promote to default for new spaces"
action. New tenants are then born fast instead of each rediscovering the same
problem one prompt at a time.

## Propose at the workload level, not per op

A single maintained table often serves several ops: three ops that each walk
`projects → tasks` are one closure table's worth of work, not three. The
advisor therefore proposes over the **workload** — "these three ops share this
traversal; one maintained table makes all three cheap, and here is the combined
write cost" — rather than firing three prompts that each look marginal alone
and collectively rebuild the same structure three times. Per-op proposals are
the degenerate case of the same mechanism, not a different one.

## Rules that keep it honest

- **A tier may never change results.** If a candidate would, it is not a tier —
  it is T3, and a human writes it. The corpus enforces this rather than a
  promise.
- **A tier may never widen scope.** The field manifest is derived from the body,
  and the body is unchanged, so a re-plan cannot touch what an op discloses.
  Consents pinned to that manifest survive a re-plan untouched — which is the
  point: making a query faster must never re-prompt every agent that uses it.
- **Staleness is disclosed, never absorbed.** A T2 op carries a freshness marker
  in its result envelope and in the catalog. "Fast" and "current" are different
  claims and the caller gets to see which one it received.
- **Contract-affecting changes are refused, not smuggled.** If a candidate plan
  would change cursor stability (`cursor: drifting`), pagination order, or
  liveness behaviour, it is rejected regardless of speed. Those are promises
  callers hold.
- **Approval is per op, and revocable.** Dropping back to T0 is the same
  mechanism backwards.

## How it degrades by custody, stated up front

This feature is measurement, and measurement is metadata.

- **Managed** — full: the platform sees rows examined and time spent.
- **BYO database** — full for time and plan shape; row-level detail depends on
  what the target exposes.
- **Client-first** — the server measures nothing, because it executes nothing.
  Telemetry is local, the advisor runs on-device, and any proposal is the
  owner's to accept locally. The feature does not silently reach into an E2E
  space to see how slow it is.

## Build placement

Not v1. But two things are cheap now and expensive later:

1. **Reserve a `tier` field in the artifact** so plan tiering is not a schema
   migration on the registry later.
2. **Record observed cost from the first executor**, even if nothing reads it
   yet. Telemetry you did not collect is telemetry you cannot backfill.

Both are built (owner ruling 2026-10-23). Every op artifact carries
`"tier": {"id": "T0", "kind": "direct"}`, inside the version hash
(`ordo_plan::artifact::Tier`). Observed cost is recorded per op and per
tenant by the metered executor (`crates/ordo-host/src/observed_cost.rs`;
`deploying.md` §2d). The version still covers `plan`, so a T1 variant with
its own SQL gets its own version. Whether a grant pins an op across its
tiers is a v2 question.

The feature itself belongs in Track F's deploy/catalog/operate phase, after the
executor and the catalog exist, and it should not start before the conformance
corpus can run on demand — since that gate is the only reason any of this is
safe.
