# Refinement: the build plan

Status: **R1 to R8 built** (owner's ruling 2026-10-05; checklist 27am; board
`refinement-default`). R8, the audit, is section 11's last row. This plan turned the ruling
into increments; section 11 says what each became, with its commits. Sections
1 to 8 are the plan as written, with notes where the build went otherwise.

**What changed from the plan, in the build:**

- **React live (R6).** Two hooks on one op with different refinements are
  two reads on the shared stream, keyed by `(op, params, refinement)`. The
  door checks each refinement when its read is added. Signals stay per op,
  so one write refreshes both. Sections 7 and 8 said a refined and an
  unrefined hook share one live read keyed by `(op, params)`.
- **`$id` is refinable (C4e, 2026-09-17).** R1 and R2 dropped it, because an
  op's body cannot name `$id`. Commit `877ce19` restored it: conditions on
  `$id` travel beside the call and apply to the bound query, after the take.
- **The count pairing was built this round** (ask 5, *"This round"*), against
  the recommendation to defer it.

**The ruling in one line.** Every ordinary read accepts a refinement, a filter
and a pick over its own outputs, by default and with no off switch. The owner:
*"this should be a standard feature...Imagine having to write an op for every
specific thing you could display for any tenant."*

**What a refinement is.** Structured data, never query text:

```
"$refine": { "where": [ { "field": "billed", "op": "gt", "value": "4000" } ],
             "pick":  [ "name", "billed" ] }
```

- 8 operators, closed: `eq ne lt le gt ge in is_null`.
- Conditions combine as AND. `pick` names a non-empty subset of outputs.
- It only narrows. Parameters choose what an op answers; a refinement filters
  the rows and fields that answer came back with.
- Public reads keep declared parameters only (`design-public-path.md` ask 1).

---

## 1 · Where it stood when planned

**The envelope was built end to end and served by nothing.** Each layer was
right on its own; the tier and the executor were the two gaps. Every **gap**
and **missing** row below is closed (section 11).

| Layer | State | Where |
|---|---|---|
| Parse and validate `$refine` | built | `crates/ordo-api/src/refine.rs:45-158` |
| Sealed-field rules (opt-in, equality only, none under an aggregate) | built | `refine.rs:91-122` |
| Cost gate, effective class per request | built | `refine.rs:182-262`, `crates/ordo-plan/src/cost.rs:552-620` |
| Router reads `$refine` from a POST body and a GET query | built | `crates/ordo-api/src/router.rs:1144`, `:1180-1187`, `:1244-1276` |
| Every compiled op registers tier `None` | **gap** | `crates/ordo-host/src/lib.rs:341` |
| Every platform op registers tier `None` | correct | `crates/ordo-host/src/platform_ops.rs:229` and 22 more sites |
| Descriptor hardcodes `refine.tier: "none"` | **gap** | `crates/ordo-plan/src/interface.rs:573-583` |
| The executor takes the validated refinement and ignores it | **gap** | `crates/ordo-host/src/lib.rs:761` (`_refine`), proved by `crates/ordo-host/tests/cost_class.rs:460-466` |
| Device engine: filter, pick, sort over synced rows | built | `clients/ordo-sdk/src/replica/refine.ts:287-333` |
| Parity table, 71 cases, proved on Postgres | built | `crates/ordo-conformance/fixtures/refine-parity.json`, `tests/refine_parity.rs` |
| React hooks take `refine` and send it | built, refused locally | `clients/ordo-react/src/hooks/useOpQuery.ts:30`, `src/refine.ts:103-109` |
| SDK builders `.where` / `.pick` | **missing** | `crates/ordo-gen/src/sdk.rs:37-41` says why |
| Live and sync refuse `$refine` | by design, until section 7 | `router.rs:1378-1384` |

Three board tasks already carry pieces of this: `refinement-default` (the
ruling), `effective-cost` ask 1 ("add the op-level marker or leave dormant?",
now answered: no marker, on by default) and `refine-tier-local` ask 1 (the
device answers what the door refuses; answered by the ruling: the door
catches up).

---

## 2 · The default

**Every compiled read is tier `Basic`. Everything else stays `None`.**

| Op | Tier | Why |
|---|---|---|
| A program's read op (`OpBody::Read`), std reads included | `Basic` | The ruling. The manifest is its own outputs, so a refinement can only narrow. |
| A write or delete | `None` | No rows to refine. The router refuses `$refine` on a non-read before the tier is read (`router.rs:1248-1253`). |
| A platform op (`platform_ops.rs`) | `None` | Their results are the platform's shapes, with a stated refusal at each site (`platform_ops.rs:225-237`). Unchanged. |
| A read that fails to bind at registration | `None` | It has no manifest (`lib.rs:305-309`), so there is nothing to validate a field against. |
| A public read, when `OpSpec.public` lands | `None` | Ruled: declared parameters only. Each refinement is a cache key, and open keys starve the cache (`design-public-path.md:166, :808`). `public.refine` is the later setting (`:780`). `OpSpec.public` does not exist yet; the public path's increment 1 owns it. |

**No off switch and no new word.** The op wrapper's flags stay `reproducible`,
`unbounded`, `unguarded` (`definition-language.md:3449-3453`). `refinable`
stays a field mark, and only on sealed fields (`sealed indexed refinable`).

**How the interface states it.** `interface.rs:582` stops hardcoding `"none"`
and derives the tier from the manifest: `kind == "read"` and not `platform`
and not `public` gives `basic`. `call.refinable` already follows the tier
(`interface.rs:473`), and the test at `crates/ordo-cli/tests/interface.rs:802-806`
holds the two together. The descriptor stays `ordo.interface/2`: the key's
meaning is unchanged, only its value moves, which the version rule at
`interface.rs:95-113` allows. The golden `crates/ordo-cli/tests/golden/market.interface.json`
re-records with `basic` on every program read.

**Refusals in the descriptor.** `interface.rs:761-800` lists each op's
possible refusals. A `basic` read gains `invalid_refinement` and
`refinement_unbounded`, so the generated client's typed refusal union carries
them.

**Aggregate ops.** `std.E.count` is derived (`crates/ordo-schema/src/lib.rs:1769-1790`)
and its outputs are `{ n }`. `std-library.md` §2.1 pairs its conditions with
`std.E.list`'s manifest; nothing in `crates/ordo-host/src/lib.rs` builds that
pairing (`aggregate_only` is derived from a `reduce` step alone, `lib.rs:311-315`).
Under this plan `count` is `Basic` over its own outputs, which is legal and
nearly useless. The pairing is its own increment, after the rest (ask 5).
**Ruled this round instead, and built** with R3 (`333de34`): the door's
`count_fields` validates a count's conditions against the list's manifest
and applies them before the count.

---

## 3 · The cost gate, as an app developer meets it

**The gate is about sealed fields. An open field never trips it.**

What `check_cost` (`refine.rs:218-262`) computes, per request:

```
effective = registered ⊔ class_of(each where condition)
```

- An **open** field's condition is `sql` (`cost.rs:594-596`). It never raises
  the class, whatever the operator.
- A **sealed indexed refinable** field with `eq` or `in` is `blind-index`
  (`cost.rs:597-599`). Any other operator on a sealed field is refused
  earlier, by `validate`, as `invalid_refinement` (`refine.rs:114-121`).
- A **sealed field with no blind index** is host work: `bounded-host` when the
  set the condition examines is provably bounded, `unbounded-host` otherwise
  (`cost.rs:600-603`). The set is the op's result for a row-shaped op, and the
  reduce's input for a keyless reduce such as `count` (`cost.rs:552-566`).
- The request is refused only when the effective class is `unbounded-host`
  and the op was not registered with `unbounded` (`refine.rs:219-222`).

**The exact refusal.** HTTP 400, class `refinement_unbounded`, `param` names
the field, `op` names the op. The message (`refine.rs:238-249`):

> refining `memo` on `ledgerCount` raises this request's effective cost class
> to `unbounded-host` (registered `sql`) — the condition cannot be answered in
> the database, so it opens sealed payloads for an unbounded number of rows.
> `ledgerCount` was not registered with the `unbounded` acknowledgment, and a
> refinement cannot acknowledge on its author's behalf

Three ways out, in the order to try them:

1. **Give the field a blind index**: `sealed indexed refinable`. Equality
   then rides the index at SQL speed, and `eq` and `in` are what a filter
   usually needs.
2. **Bound the op**: a `take n` makes the examined set provably finite, so the
   class is `bounded-host` and the gate passes.
3. **Say `unbounded` on the op**: a human's sentence that this op's work grows
   with the space. The gate then serves the request at `unbounded-host`.

**Gaps the gate had when planned, named so the increments close them.** The
first is closed (ask 3: the door refuses as `sealed_unsupported`, R2). The
second is answered by `docs/howto-cost.md` and `ordo check`'s refinable line
(R7). The third is still open: nothing logs the effective class.

- **`bounded-host` passes the gate and cannot run.** (Closed 2026-10-05 by
  27ao: the executor serves `bounded-host` reads, one key release per page.)
  The executor refused any host-class plan: `sealed_unsupported: ... host
  staging is not built`. So a sealed, unindexed condition
  on a bounded op clears `check_cost` and then fails in the executor. The
  door should refuse it itself, with the same class and a message that names
  the field and the fix (ask 3).
- **Open fields have no database cost gate at all.** A condition on an
  unindexed open text field is `sql` and is served; on a large entity it is a
  sequential scan. This is the same cost a parameterized op would have, and
  it is where the owner's warning lands: *"That will be an important feature
  that could affect devs later unexpectedly."* `docs/howto-cost.md` (section
  9) is the answer; a compile-time hint is ask 4.
- **The effective class is computed and dropped** (`router.rs:1262-1274`,
  `_effective`). Nothing logs it. When a tracing substrate exists it rides
  `ordo.cost_class`; until then a refined read's class is invisible to the
  deployment owner.

**Should `ordo check` suggest an index?** `ordo check` prints each read's
class and row bound (`crates/ordo-cli/src/lib.rs:414-421`) and warns on bare
`money` (`:225-236`). It cannot know which fields a tenant's screens will
filter on. What it can do without guessing: print each read's refinable
fields, mark a sealed unindexed one as `refused`, a sealed indexed one as
`equality only`, and leave open fields unmarked. The how-to tells developers
when to add `indexed` to an open field (ask 4).

**Built (R7).** One line per read, under its class and outputs:
`refinable: placed, $id (any operator); memo (refused)`. A sealed field
declared `sealed indexed refinable` prints as `equality only`. The how-to
found that `indexed` is refused on an open field (`indexed_without_sealed`),
so its advice for an open field is a two-column `index (field, other)`.

---

## 4 · Where the filter runs on the server

**Recommended: compile the refinement into the plan, the way written
composition is compiled.** This changes one settled sentence and needs the
owner's ruling (ask 1). **Ruled yes (2026-10-05) and built in R2**
(`crates/ordo-host/src/refine_call.rs`): pages come back full.

The language already has the exact grammar: `name(conditions)[projection]`
(`definition-language.md:2249-2300`). The compiler inlines a composed callee,
and when the callee is settled by a `take`, a `group` or `distinct` it wraps
it so the caller's conditions apply post-take (`crates/ordo-sema/src/lib.rs:2795-2800`).
A refinement is this call with the conditions coming from the envelope.

Two places disagree about what happens next, and both are marked settled:

- `api-engine-design.md` §7 (`:421-452`): the refinement applies **after the
  page window**. Pages come back short or empty; `next` alone terminates.
- `tooling-and-embedding-plan.md` §B2 (`:163-166`): the refinement compiles
  through **the same inliner as written composition**, "post-take,
  post-everything".

A wrapper query is paged like any read: the page window is on the outer query,
so the filter runs **before** the window and pages come back full. §7 rejected
that on cost grounds, "a caller-controlled filter that walks a whole entity to
find twenty rows". That argument holds for host-side conditions only. An
open-field condition is a SQL predicate under a `LIMIT`, which is what every
parameterized op already does, and §7 itself calls that "the right answer".

So the recommendation:

- **Open and blind-index conditions compile into the plan.** Full pages,
  `sql` or `blind-index` class, Postgres does the work.
- **Host-side sealed conditions stay post-window** in principle, and are
  refused at the door today because the executor cannot run them (section 3).
- **`pick` is the projection** `[fields]` of the wrapper. The anchor's `$id`
  rides anyway (`ordo-exec/src/lib.rs:2146-2151`), and the hidden sort key
  columns the cursor needs are already kept and trimmed (`:2179-2220`).

**Keyset paging stays correct either way.** The cursor is minted from the last
returned row's sort keys and seeks past them in the op's order
(`ordo-exec/src/lib.rs:2179-2213`). The envelope has no sort key, so the order
is the op's. "Rows after K that match P" is exactly the next page of the
refined set. The cursor's MAC does not cover the refinement (`page.rs:40-42`),
and that stays harmless for the reason `page.rs:52-54` gives: narrowing cannot
move the row a cursor addresses. A client that changes the refinement mid-
iteration starts the new filter after the same row, which is the behaviour a
parameter swap has today (`page.rs:47-51`).

**How the host applies it.** Not as text. `call_source` renders the call as
source and re-parses it (`crates/ordo-host/src/lib.rs:677-703`), and it strips
`"` from string values instead of escaping them (`:692`), which is already
lossy for a text parameter containing a quote. The refinement increment
builds the wrapper as an AST (`ordo_syntax::Pipeline`, a `Source` naming the
op with the bound parameters and the conditions, a projection step for
`pick`) and hands it to `ordo_sema::bind_as` (`crates/ordo-sema/src/lib.rs:1750-1757`),
so no value is ever spelled as source. Values become literals by the output's
type, exactly as the parity test writes them (`refine_parity.rs:100-139`):
money and decimal from the wire's decimal string at the field's places,
text NFC.

**What the door must check that it does not today**, because a `bind:` error
in the executor is a 500 (`lib.rs:704-716`, "parse:, bind:, plan: ...
internal"):

- **Type.** `FieldSpec` (`spec.rs:219-232`) has no type. A string against an
  `int` output reaches the binder and 500s. Add `ty` to `FieldSpec` from
  `manifest_fields` and check each value's shape against it in `validate`.
- **NFC.** `bind.rs:79` normalizes parameters; `refine.rs` normalizes nothing.
  The device doc assumes the door does (`refine.ts:65-66`). Normalize every
  string value in `parse`.
- **Decimals.** A money or decimal output takes a decimal string or an
  integer, never a float, as the device already enforces (`refine.ts:121-131`).

---

## 5 · SDK builders in the generated client

**Reads are lazy builders; `.where` and `.pick` narrow them; the terminal verb
is unchanged.** `tooling-and-embedding-plan.md` §B2 and §B3.

```ts
const rows = await vendorSpend(client, { from: '2026-01-01' })
  .where((v) => [v.billed.gt('4000'), v.status.in(['open', 'late'])])
  .pick('name', 'billed')
  .fetch();
// rows.rows: ReadonlyArray<Pick<VendorSpendRow, 'name' | 'billed'>>
```

- **Typed from the output manifest.** The generator (`crates/ordo-gen/src/sdk.rs`,
  `ops_file` at `:448`) already emits `<Op>Row` from `result.outputs`. It adds
  a per-op field proxy: each output is an object with the 8 operators, typed
  by the output's type. `money` and `decimal` take `string`; `int` and
  `duration` take `number`; `date`, `datetime`, `text` and ids take `string`;
  `bool` takes `boolean`. `is_null` takes nothing. Nullable outputs allow
  `is_null`; a `pick` key is `keyof Row`.
- **`.where` is AND.** A second `.where` appends. The callback returns one
  condition or an array. The builder is immutable: each call returns a new
  `ReadCall`, so a `.pages()` iteration holds one refinement for its life.
- **`.pick(...keys)` narrows the row type** to `Pick<Row, K>`. A later
  `.pick` replaces the earlier one. An empty pick is a compile error.
- **Serialization.** The builder holds `Refine { where, pick }` in the wire
  shape (`refine.ts:80-104` in the SDK already defines it). `OrdoClient.request`
  (`clients/ordo-sdk/src/core/client.ts:190-215`) gains `refine` in
  `RequestOptions` (`:85-89`) and puts it in the GET query as URL-encoded
  JSON under `$refine`, which is the router's form (`router.rs:1160-1187`).
  Reads are GET only, so no POST form is needed.
- **Local validation before the wire.** `checkQuery` (`replica/refine.ts:115-173`)
  moves to `core/refine.ts` and runs in the builder: unknown field, unknown
  operator, value shape, float against a decimal. It refuses with
  `OrdoLocalRefusal` class `invalid_refinement`, the door's own class, as the
  React copy does (`clients/ordo-react/src/refine.ts:87-140`). The React copy
  then imports the core one instead of carrying a second.
- **Paging.** `ReadCall.pages()` (`core/call.ts:77-86`) stops only when `next`
  is absent. That stays. Every page request carries the same refinement and
  the cursor from the last page. Under section 4's design pages are full;
  under §7's they may be short; the loop is correct under both.
- **Single-row reads** (`SingleReadCall`, `call.ts:103`) take `.where` and
  `.pick` too. A condition on a single row answers that row or nothing.
- **Errors as typed results.** The generated refusal union per op comes from
  the descriptor's `refusals` (`sdk.rs`, from `interface.rs:761`). A `basic`
  read's union gains `'invalid_refinement' | 'refinement_unbounded'`, and
  `'sealed_unsupported'` once a sealed condition can reach it. `OrdoRefusal`
  carries `param`, which names the field.
- **The spec table** (`generated/specs.ts`, emitted at `sdk.rs:306-344`)
  keeps `refine: { tier, fields, decimals }` and gains per-field types for
  the runtime check. The device reads the same table.

---

## 6 · The device copy

**One query, one answer, wherever it is evaluated.** The device engine
(`clients/ordo-sdk/src/replica/refine.ts`) already evaluates the envelope
over synced rows. This plan changes nothing in how it compares values and
adds proof that the server's `$refine` path agrees with it.

The parity rules, all already pinned by the shared table (`refine-parity.json`,
71 cases) and the two tests that read it:

| Rule | Device | Server |
|---|---|---|
| Two-valued null logic: `null == null`, `x != y` is `not (x == y)`, ordering with null is false, `in` is a chain of `==` | `refine.ts:176-211` (fixes 25k, 25l) | the rows of `IS NOT DISTINCT FROM` (spelled `=` where a side is proven never null, ruling 2026-10-26); `in` with a null member matches a null (fixes 25m, 25n) |
| Text by code point, `COLLATE "C"`, over NFC | `refine.ts:234-254` (fixes 25k, 25l) | `COLLATE "C"` on text ordering comparisons (fixes 25m, 25n) |
| Money and decimal as exact numbers; `"29.90"` equals `"29.9"`; a float is refused | `refine.ts:201-224`, `compareDecimal` | `numeric` (27ak) |
| Nulls last in both sort directions, then the id | `refine.ts:299-322` | `NULLS LAST`, the tiebreak |
| Unknown field refused by name, listing the manifest | `refine.ts:133-141` | `refine.rs:264-278` |

**Three deliberate differences stay** (`refine.ts:15-31`): sort is device-only
(the envelope has no sort key, and the cursor depends on that); sealed rules
do not apply on the device (it only holds opened values); and the tier is not
consulted. With `Basic` the default, the third difference stops being a
disagreement: the device and the door now say yes to the same ops.

**What the increment adds.** Today the server half of the parity test turns
each case into source text, `probes(n == 5)[id]` (`refine_parity.rs:141-172`),
and never sends `$refine`. A third test runs every case through the door as a
`$refine` on a registered op over Postgres, and requires the same ids. Then
three evaluators agree by construction: written composition, the envelope,
and the device. New cases: `pick` (column set and order), `in` over decimals
and over text needing NFC, `is_null` on each type, a datetime range, money at
0 places, `-0.00` against `0.00` (`bind.rs:431` normalizes a parameter's zero;
the refinement path must too).

---

## 7 · Live and sync with a refinement

**Live needs almost nothing. Sync needs a rule for `pick`. Neither blocks the
one-shot path, and a caller has a working answer meanwhile.**

**What is there.** Invalidation is per op: one broadcast channel per op name
(`crates/ordo-api/src/bus.rs:1-2, :35`), and a signal carries only the read's
key (`live.rs:9-16`). The shared stream `GET /live` carries signals only; a
read is added with `{key, op, params}` and `params` must be strings, numbers
or booleans (`router.rs:1512-1527`). The client keys a live read by
`(op, params)` (`clients/ordo-react/src/live.ts:348-351`) and refetches
through the one-shot op with its own refinement (`useOpQuery.ts:100-104`).
The single-read stream `GET /op/{name}?live=1` refuses `$refine`
(`router.rs:1378-1384`) because its first frame is the op's whole envelope.

**A refined live read.**

- Invalidation stays per op. A refinement does not change which writes can
  change the answer, only which rows the answer shows. A signal for a row
  outside the filter costs one refetch that returns the same rows. That is
  the over-invalidation the design already accepts for guards
  (`design-public-path.md:242-246`).
- The client refetches with its refinement. On the shared stream this is the
  case today, once the one-shot path serves `$refine`: nothing in `/live`
  changes.
- The single-read stream accepts `$refine` and applies it to the first frame,
  the one-shot read, then signals as before. The refusal at `router.rs:1378`
  goes; the paging refusal at `:1385` stays, since a subscription is the op
  at full scope.
- **Meanwhile:** a React app gets refined live reads the day the one-shot
  path lands, because its hooks already go through the shared stream and
  refetch with the refinement.
- **As built (R6), the shared stream changed after all.** A read is added
  with `{key, op, params, refine}`, and the door checks the refinement when
  the read is added, as the one-shot path does. The client keys a live read
  by `(op, params, refinement)`, so two hooks on one op with different
  refinements are two reads. Signals stay per op: one write refreshes both.

**A refined sync** (`?live=1&sync=1`, `crates/ordo-api/src/sync.rs`).

- The server remembers each row it sent and diffs each new read against it
  (`sync.rs:375-420`). A row that stops matching the filter leaves the result
  and is a `delete`; a row that starts matching is an `upsert`. Rows entering
  and leaving the refined set fall out of the diff with no new mechanism,
  because a delete already means "no longer in this op's result for you"
  (`plan-replica-feed.md:41`).
- `where` is accepted. The sync's reader pages the op with the refinement.
- **`pick` is refused on a sync** (ask 2). The device stores whole rows
  keyed `(entity, id)` and a row held through two synced ops stays until
  neither holds it (`plan-replica-feed.md:45`). A picked row would overwrite
  a whole one under the same key. `pick` is a presentation choice, and the
  device applies it locally in `refineRows` for free. `sync::check`
  (`sync.rs:102`) also needs the anchor's id, which a pick could drop.
- The device's holder key includes the refinement: the manager's handle is
  `op` plus sorted params (`clients/ordo-sdk/src/replica/manager.ts:323-326`),
  and `syncOp` resumes by `(holder, version)` (`sync.ts:127`). Two syncs of
  one op with different filters are two holders over one row store, which
  the store already supports.
- **Meanwhile:** the device syncs the whole op and refines locally
  (`refineOp`, `refine.ts:342-359`). A refined sync is a saving in rows on
  the device and on the wire; it adds no capability. So it is the last
  increment, and nothing waits on it.

---

## 8 · React hooks

**They already take `refine`. The local refusal goes, the types arrive.**

- `useOpQuery(op, { params, refine })` (`useOpQuery.ts:30`) and
  `useOpPages` (`useOpPages.ts:23`) send `refine` through `fetchOp`
  (`client.ts:269-308`). `encodeRefine` refuses on tier `none`
  (`refine.ts:103-109`); with `basic` it validates and passes. `encodeRefine`
  becomes a thin call to the core validator (section 5).
- `refinementSurface(op).enabled` (`refine.ts:60-80`) turns true for every
  program read, so a generated surface draws filter chrome. `sortOnly` stays
  the answer for a platform or public op.
- **Typed refinements.** The hooks accept the wire shape today. They gain a
  typed form: `refine: (v) => [v.billed.gt('4000')]` using the same per-op
  proxy the SDK builders use, and `pick: ['name', 'billed'] as const`
  narrowing the row type. The wire shape stays accepted for generated UIs
  that build envelopes from state (`ui-gen.md:98-100`).
- **Device first.** `useDevice` passes the refinement to `planDevice`
  (`useDevice.ts:57`), which answers locally when the op is synced and the
  device has a refiner, and falls back to the network with
  `refine_unavailable` otherwise (`device.ts:97-101`). Unchanged.
- **Pages.** `useOpPages` keeps the refinement in `call` (`useOpPages.ts:82`)
  and its `reload` refetches the same number of pages with it. Under section
  4 pages are full, so "the same number of pages" shows the same rows.
- **Live.** `useLive` follows by `(op, params)` (`useLive.ts:48-60`). A
  refined hook and an unrefined one on the same op share one signal.
  **As built (R6):** the hub keys a read by `(op, params, refinement)` and
  sends the refinement with the read, so the two hooks are two reads on the
  shared stream, each checked by the door when added. They still share the
  op's signal, so one write refreshes both.

---

## 9 · Documents to change

**The reference says "declare `refinable`" in 6 places. It must say "every
read, no flag".**

**Done in R7**, except the two rows the board's keeper writes
(`corpus-rulings.md` and `board-docs.jsonl`). `api-engine-design.md` was done
earlier, in `5d382ce`. The code comments were rewritten with R1 to R6, and R7
fixed the two left (`ordo-host/src/lib.rs` `manifest_fields`,
`ordo-conformance/tests/std_library.rs`). The sealed-fields example and
`examples/syntax/README.md` were also corrected.

| Document | Line | Change |
|---|---|---|
| `definition-language.md` | 2187-2192 | Drop "Not built yet: no op accepts refinement" and "which word opts an op in is undecided". State: every read is refinable; there is no op-level word; `refinable` marks a sealed field only. |
| `definition-language.md` | 2164-2185 | Keep the sealed rules. Add one line: an open field's condition is `sql` and never gated. |
| `api-engine-design.md` §7 | 418-420 | "Ops declare `refinable: none / basic` at registration" becomes "the tier is derived: `basic` for a program read, `none` for a write, a platform op or a public read". |
| `api-engine-design.md` §7 | 421-452 | If ask 1 is ruled as recommended: open and blind-index conditions compile into the plan and pages are full; the post-window reading stays for host-side conditions. If not, unchanged. |
| `api-engine-design.md` error table | 144 | Unchanged. Add `sealed_unsupported` as a refinement refusal when ask 3 is ruled. |
| `std-library.md` §2.1 | 136-139 | "`std.E.list` declares `refinable: basic`" becomes "like every read". The count pairing paragraph gains "not built" until ask 5 lands. |
| `tooling-and-embedding-plan.md` §B2 | 171-175 | "nothing is refinable that the op author didn't ship" becomes "every read ships Tier 0". Tier 1 stays a future opt-in. |
| `ui-gen.md` | 42, 98-100 | "ops declaring `refinable: none` render sort-only chrome" becomes "writes, platform and public ops render sort-only chrome". |
| `plan-client-cache.md` | 124-126 | Note that the tier question is answered and the device and door agree. |
| `corpus-rulings.md` | 2934-2945 | A note under the old ruling that 2026-10-05 superseded it. Written by the increment, not by this plan. |
| Code comments | `interface.rs:554-572`, `sdk.rs:37-41`, `ordo-react/src/refine.ts:1-24`, `ordo-sdk/src/replica/refine.ts:17-23`, `cost_class.rs:460-466` | Each says the tier is `none` everywhere. Each is rewritten with the increment that changes it. |
| `docs/board-docs.jsonl` | new row | `plan-refinement.md` as a plan, `howto-cost.md` as a guide, in the commits that add them. |

---

## 10 · `docs/howto-cost.md`: the outline

**A guide for app developers: what a read costs, what a refinement adds, and
what to do when the door says no.** The owner asked for it: *"That will be an
important feature that could affect devs later unexpectedly."*

1. **The four classes in one table.** `sql`, `blind-index`, `bounded-host`,
   `unbounded-host`: what each means, who does the work, and the one-line
   test for which you have. Read from `ordo check`.
2. **What registration refuses.** `unacknowledged_unbounded`,
   `unacknowledged_unguarded`, the venue gate. The word that clears each.
3. **What a refinement adds.** Open field: nothing. Sealed indexed field:
   equality only. Sealed unindexed field: refused or host work. The formula
   `effective = registered ⊔ conditions`, in words.
4. **The refusal, verbatim, and the three ways out.** Index the field, bound
   the op, or acknowledge `unbounded`. With the message from section 3 and
   one program before and after for each.
5. **Open fields on big entities.** A filter on an unindexed open field is a
   scan. When to write `indexed`, what it costs on writes, how to see the
   plan (`ordo plan`). This is the "unexpected" cost the owner meant.
6. **Pages under a refinement.** Full pages (section 4) or short pages (§7),
   whichever is ruled; `next` alone ends a listing; `count` and `list` under
   one refinement describe one set.
7. **On the device.** A synced op answers refinements locally and the server
   does no work. When to sync, when to call.
8. **Reading the class in production.** What is logged today (nothing yet),
   what `ordo.cost_class` will carry, and how a deployment owner can tell a
   refinement raised a class.
9. **A checklist** before shipping a screen with filters: 6 lines.

---

## 11 · Increments

**8 increments, each with its own tests. Opus 5.5 builds, Sonnet writes the
prose, Fable audits at the end** (owner, 2026-10-04). R1 to R3 are the
server; R4 and R5 the clients; R6 the streams; R7 the documents; R8 the
audit. R1, R2 and R4 can start in parallel after the asks are answered; R3
needs R2; R5 needs R4; R6 needs R2.

| # | Increment | Tier | Tests |
|---|---|---|---|
| **R1** | **The default and the interface.** `build_specs` registers `RefineTier::Basic` for every bound `OpBody::Read` (`lib.rs:341`); `interface.rs` derives the tier (`:582`) and adds the two refusal classes (`:761`); the golden descriptor re-records. The device's `refine.tier` note (`refine.ts:17-23`) is rewritten. | Opus | `crates/ordo-host/tests/interface_surface.rs` (every program read is `basic`; writes and platform ops `none`); `crates/ordo-cli/tests/interface.rs` (`refinable ⟺ tier != none` still holds; refusal list); `crates/ordo-api/tests/api.rs::refinement_rejected_when_not_refinable` keeps a hand spec at `None`; `clients/ordo-sdk/test/generated.test.ts` (spec table carries `basic`). |
| **R2** | **The executor applies `$refine`.** `FieldSpec` gains `ty`; `validate` checks type, NFC and decimal shape; the host builds the wrapper AST and binds it (section 4); `pick` is the projection; a host-class effective plan is refused at the door as `sealed_unsupported` naming the field (ask 3). | Opus | New `crates/ordo-conformance/tests/refine_door.rs`: all 71 parity cases through `$refine` on Postgres, plus the new cases of section 6; `cost_class.rs:460-466` flips from "served 3" to "filtered 1"; `api.rs` type-mismatch, non-NFC value, float against money, `pick` order, pick dropping and keeping `$id`; a sealed indexed `eq` through the blind index; a sealed unindexed condition refused at the door, never a 500. |
| **R3** | **Paging under a refinement, proved.** Page size 2 through a refined set equals the unpaged refined set; a cursor from page 1 with a changed refinement starts after the same row; a `take n` op refines post-take (the settled-wrapper rule, `sema/lib.rs:2795`); `std.E.count` over its own `n`. | Opus | `crates/ordo-conformance/tests/refine_paging.rs` on Postgres; `crates/ordo-api/tests/api.rs` for the cursor cases over the stub executor. |
| **R4** | **SDK builders.** `core/refine.ts` (validator shared with the device and React); `RequestOptions.refine`; `ReadCall.where/pick` and `SingleReadCall.where/pick`; the generator emits per-op field proxies, `Pick` row types and the refusal union. | Opus | `crates/ordo-gen/tests/{generated_files,isomorphism,ratchet}.rs`; `clients/ordo-sdk/test/generated.test.ts` (a refined GET carries `$refine` URL-encoded; `pages()` passes it on every page; local refusals); a type-level test file (`expectTypeOf`) for the narrowed row and the operator set per type. |
| **R5** | **React.** Typed `refine` and `pick` options; `encodeRefine` delegates to core; `refinementSurface` enabled for `basic`; hooks' tests. | Opus | `clients/ordo-react/test/{client,device,pages}.test.ts`: a refined query reaches the wire; a refined page reload fetches the same pages with the refinement; the device answers a refined synced op locally; a `pick` narrows the row. |
| **R6** | **Live and sync.** `?live=1` accepts `$refine` for the first frame; `?sync=1` accepts `where` and refuses `pick`; the manager's handle and `resultHash` key include the refinement. | Opus | `crates/ordo-api/tests/api.rs` (live first frame refined; sync with `pick` refused by class and message); `crates/ordo-host/tests` sync frames: a row entering the filter is an `upsert`, a row leaving is a `delete`; `clients/ordo-sdk/test/sync.test.ts` two holders of one op with two filters. |
| **R7** | **Documents.** Section 9's table; `docs/howto-cost.md` from section 10; `board-docs.jsonl` rows; `ordo check` prints refinable fields with their class (ask 4). | Sonnet (prose), Opus (`ordo check`) | `crates/ordo-cli/tests` snapshot of the op report; `node scripts/board-agrees.mjs`; a grep that no document still says "declares `refinable: basic`" or "tier is `none` for every op". |
| **R8** | **Audit.** Three-evaluator parity (composition, `$refine`, device); every door refusal is a class and never a 500; the cost gate against a `count` with a sealed condition and against an `in` with 10,000 values (ask 6); the documents against the code. | Fable | The audit's findings as reported fixes on the board, each with its guarding test. |

Not in this plan: Tier 1 (text operators, per-op opt-in), tracing the
effective class (needs a substrate), and the public path's `public.refine`
setting (the public path's plan). The `count` pairing was out of the plan
and was ruled in (ask 5).

**What each became:**

| # | State | Commits |
|---|---|---|
| R1 | built | `4adc91e` (with R2); `94bc161` regenerated the SDK and CDK |
| R2 | built | `4adc91e`; `5d382ce` the reference docs; `877ce19` restored `$id` (C4e) |
| R3 | built, with the count pairing | `333de34` |
| R4 | built | `ba846e5` (one validator for wire and device), `0a9d452` (builders) |
| R5 | built | `93881a3` |
| R6 | built | `fd4801d` (server), `e929942` (clients). React keys a live read by refinement too (top of this page) |
| R7 | built | `ordo check`'s refinable line, `docs/howto-cost.md`, and section 9's documents, in this branch |
| R8 | built | `e772523` the suite (`crates/ordo-host/tests/refine_audit.rs`); its fixes `3e5b4db` (door: calendar dates, NUL, repeated pick, oversized money), `924ca3b` (clients), `b15117e` (boot refusal names the default); `ac22ff6` reported fix 25w (`index` on a field) |

---

## 12 · Draft board asks

For the `refinement-default` task. Each is one question; each suggestion
starts with the answer. **All six are answered** (`corpus-rulings.md`,
2026-10-05): asks 1, 2, 3, 4 and 6 as recommended; ask 5 *"This round"*.

**Ask 1.** Should open-field and blind-index conditions compile into the plan,
so pages come back full, instead of filtering after the page window as §7
says?

> **Yes, compile them in.** They are SQL predicates under a LIMIT, the same
> cost as a parameterized op; only host-side sealed conditions need the
> post-window rule, and those cannot run today anyway.

**Ask 2.** Should a sync (`?live=1&sync=1`) accept `where` and refuse `pick`,
since the device stores whole rows keyed by id?

> **Yes.** A picked row would overwrite a whole row held by another sync of
> the same entity, and the device already applies `pick` locally at no cost.

**Ask 3.** When a sealed, unindexed condition is bounded-host, should the door
refuse it as `sealed_unsupported` naming the field, until host staging is
built?

> **Yes, refuse at the door.** Today it passes the cost gate and fails in the
> executor with a message that names no field and no fix.

**Ask 4.** Should `ordo check` print each read's refinable fields with their
class (open, equality only, refused), rather than guess which fields need an
index?

> **Yes, print, do not guess.** The compiler cannot know what a tenant's
> screens filter on; the how-to tells developers when to add `indexed` to an
> open field.

**Ask 5.** Should the `std.E.count` pairing (conditions validated against
`list`'s manifest, applied before the reduce) be built in this round or
after?

> **After.** It is the one refinement that can change the order of work, it
> has no code today, and bulk-confirm screens can read `count` and `list`
> through two calls meanwhile.

**Ask 6.** Should a refinement's size be a setting with defaults, such as at
most 16 conditions and 200 values in one `in`?

> **Yes, a setting with those defaults.** An unbounded `in` is a free scan
> the cost gate cannot see; a setting lets a deployment owner raise it where a
> screen needs more.
