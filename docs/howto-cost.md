# What a read costs, and what a refinement adds

**Every read accepts a refinement. Open fields never trip the cost gate.
Sealed fields do, and a few size limits apply to every request. This page
lists each refusal word for word, why it happens, and how to avoid it.**

The owner asked for this page so the cost gate is never "a surprise or a
debugging mission". Read sections 1 and 4 first. The checklist is section 12.

Contents:

1. The four cost classes
2. What registration refuses
3. What a refinement is
4. What `ordo check` shows, and what only the door can refuse
5. What a refinement adds to the cost
6. Every refusal, and how to avoid it
7. Open fields on big entities
8. Pages under a refinement
9. Live and sync with a refinement
10. On the device
11. Reading the class in production
12. Before you ship a screen with filters

---

## 1 · The four cost classes

**Every read has one class: where its worst stage runs.** `ordo check` prints
it on the line under the op's name.

| Class | Who does the work | You have it when |
|---|---|---|
| `sql` | Postgres, all of it | Every filter, sort and projection is on open fields |
| `blind-index` | Postgres, through a keyed digest | An equality on a `sealed indexed` field |
| `bounded-host` | The host opens sealed values, a provably limited number | You project a sealed field, and a `take`, a key lookup or the page bounds the rows |
| `unbounded-host` | The host opens sealed values for every candidate row | A filter or sort on a sealed field with no blind index, and nothing bounds the rows examined |

- The classes are ordered. `sql` is cheapest, `unbounded-host` the most
  expensive.
- `take` bounds rows **returned**, not rows **examined**. A sealed filter
  examines every candidate before `take` drops any.
- A `group` or `reduce` over rows that carry a sealed field is plain `sql`
  when it only counts them. No sealed value survives the fold.
- An aggregate over the sealed value itself is refused as
  `sealed_unsupported`: `sum`, `min`, `avg`, `reduce { sum }`, and an
  expression such as `coalesce(total, 0)`. Postgres holds only the
  ciphertext and cannot compute on it. Host-side folding is not built.

**What the server serves (since 2026-10-05).** A read that returns a sealed
field is served when its class is `bounded-host`.

- Postgres returns the page's rows as stored. The guard has already decided
  which rows.
- The host takes **one key release per page**, for the request's tenant and
  under the request's correlation. It opens that page's envelopes and
  answers plaintext.
- A page that opens nothing releases nothing: no rows, a `pick` without the
  sealed field, or only empty values.
- Anonymous callers never open a sealed value.
- An `unbounded-host` read is refused as `sealed_unsupported` (section 6).
  The message names the sealed field and the fix: bound the op with
  `take n`, or keep each opened row a row of the page.

**Reading it in `ordo check`:**

```
  invoiceNotes
    class bounded-host   rows <= 20
    [customer: text, due: date, taxId: text (sealed), memo: text (sealed), $id: id]
    refinable: customer, due, $id (any operator); taxId (equality only); memo (refused)
```

- `class` is the registered class.
- `rows` is the proved row bound, or `unbounded (pageable)` for a listing
  served one page at a time.
- `(sealed)` marks an output the host opens.
- `refinable:` is section 4.

---

## 2 · What registration refuses

**These stop a program before it deploys. `ordo check` exits 1 and names the
op.** Each has one word that clears it.

**An op with no row bound that cannot be paged.**

```
`everyLine` has no provable row bound and cannot be keyset-paged — its order is not total, so paging would skip rows; acknowledge with `op everyLine unbounded = …` or bound it
```

Fix: add a `take n`, sort on a total order so it pages, or write `unbounded`
on the op.

**An op that opens sealed payloads for unbounded rows.** This is the venue
gate. It fires even when the op has a `take`, because `take` does not bound
what a sealed filter examines.

```
`memoSearch` opens sealed payloads for an unbounded number of rows — host work grows with the space, and a `take` bounds what is RETURNED, not what is EXAMINED; acknowledge with `op memoSearch unbounded = …`
```

Fix: give the sealed field a blind index (`sealed indexed`) and filter by
equality, filter on an open field instead, or write `unbounded`.

**An op that composes an unbounded op.**

```
`report` composes unbounded `everyLine` and must carry `unbounded` itself
```

Fix: write `unbounded` on the caller too. The word is viral on purpose.

**An op that composes a guard bypass.** All three above are
`unacknowledged_unbounded`. This one is `unacknowledged_unguarded`:

```
`summary` composes a callee whose guard bypass is live; carry the acknowledgment: `op summary unguarded = …` (R-G8)
```

**What `unbounded` means.** It is your sentence that this op's work grows with
the size of the space, and you mean it. It is greppable and it shows in a
deploy diff. It is also the only thing that lets a refinement reach
`unbounded-host` (section 6).

---

## 3 · What a refinement is

**A filter and a pick over a read's own outputs, sent as data.** Every
program read accepts one, with no flag and no off switch (ruled 2026-10-05).
Writes, platform ops and public reads do not.

```json
"$refine": {
  "where": [ { "field": "billed", "op": "gt", "value": "4000" },
             { "field": "status", "op": "in", "value": ["open", "late"] } ],
  "pick":  [ "customer", "billed" ]
}
```

- 8 operators: `eq ne lt le gt ge in is_null`.
- Conditions combine as AND.
- `pick` keeps a non-empty subset of the outputs. `$id` rides every row
  whatever `pick` says.
- It only narrows. Parameters choose what an op answers. A refinement filters
  the rows and fields that answer came back with.
- Every guard still applies. A refinement cannot reach a row the op could
  not.

**On the wire.** A read is a GET, and the GET carries it as URL-encoded JSON
in the query: `GET /op/openInvoices?from=2026-01-01&$refine=%7B%22where%22...`.
A read called with POST is `method_mismatch` (405) before anything else is
read. A write takes no refinement at all.

**From the generated SDK.** Reads are lazy builders:

```ts
const rows = await openInvoices(client, { from: '2026-01-01' })
  .where((v) => [v.billed.gt('4000'), v.status.in(['open', 'late'])])
  .pick('customer', 'billed')
  .fetch();
```

The builder checks the refinement before it is sent: unknown field, wrong
type, a float for money. It refuses with the door's own class,
`invalid_refinement`.

---

## 4 · What `ordo check` shows, and what only the door can refuse

**`ordo check` prints one `refinable:` line per read.** It is computed from
the program alone, by the same code that writes the interface descriptor's
`refine` block, so the two always agree. It never changes the exit code.

```
    refinable: customer, due, $id (any operator); taxId (equality only); memo (refused)
```

| Group | Fields in it | A condition may use |
|---|---|---|
| `any operator` | Open fields | `eq ne lt le gt ge in is_null` |
| `equality only` | Fields declared `sealed indexed refinable` | `eq`, `in` |
| `refused` | Any other sealed field; a sealed field under a count; a blob | Nothing |

- A count names its list's fields. `std.invoices.count` takes conditions on
  `std.invoices.list`'s outputs, never on its own `n`.
- Writes and deletes print no line.

**Known at compile time** (the `refinable:` line):

- which fields a condition may name;
- which operators each field takes;
- which fields are refused outright.

**Known only at run time** (the door refuses the request):

- a value of the wrong type, or a float for money;
- more conditions, or more `in` values, than the deployment allows;
- a sealed condition that would push the request to `unbounded-host` on an op
  without `unbounded`;
- a `pick` on a sync.

---

## 5 · What a refinement adds to the cost

**The door recomputes the class for each request.** In words: the request's
class is the op's registered class, or the most expensive class any one
condition needs, whichever is higher.

```
effective = registered ⊔ class_of(each where condition)
```

| A condition on | Its class | Effect |
|---|---|---|
| An open field, any operator | `sql` | Never raises the class |
| A `sealed indexed refinable` field, `eq` or `in` | `blind-index` | Raises a `sql` op to `blind-index`. Served |
| A sealed refinable field with no blind index | `bounded-host` or `unbounded-host` | Refused today (section 6) |

- `pick` adds nothing. It removes columns the op already produced.
- A condition on an open field compiles into the op's SQL. It runs where a
  parameter's condition would.
- That is why an open field is never gated. It can still be slow (section 7).

---

## 6 · Every refusal, and how to avoid it

**Every refusal is a JSON body with a class, a message, the op, and usually
the field.** None is a 500.

```json
{ "error": { "code": "invalid_refinement", "op": "openInvoices", "param": "colour",
             "message": "unknown field `colour` — fields a condition may name: customer, status, billed, due, $id" } }
```

### `invalid_refinement` (HTTP 400)

The refinement is malformed, names something it cannot, or is too big.

| Cause | Message |
|---|---|
| Unknown field in `where` | `` unknown field `colour` — fields a condition may name: customer, status, billed, due, $id `` |
| Unknown field in `pick` | `` unknown field `colour` — output fields: customer, status, billed, due, $id `` |
| Malformed envelope | `` malformed $refine envelope: <the parser's reason> `` |
| A GET whose `$refine` is not JSON | `` $refine query parameter must be URL-encoded JSON: <reason> `` |
| Refinement on a write | `refinement applies to read ops only` |
| An op that takes none (a platform or public read) | `` op `<name>` is not refinable `` |
| Sealed field, not opted in | `` `memo` is a sealed field and is not refinable — refining a sealed field is opt-in at its declaration, because the capability is a disclosure and belongs on the consent page `` |
| Sealed field, an operator other than `eq`/`in` | `` sealed field `taxId` accepts equality only (`eq`, `in`) — a sealed condition rides the blind index, and ordering comparisons over sealed values are an oracle `` |
| Sealed field under a count | `` `taxId` is a sealed field and `std.invoices.count` is an aggregate-only op — if you may not read the rows, you may not measure them `` |
| A blob | `` `scan` is a blob, and a blob has no order and no meaningful equality, so no condition may name it `` |
| Empty `in` on a sealed field | `` `in` on sealed field `taxId` takes at least one value `` |
| `in` without an array | `` `in` on `status` takes an array value `` |
| `is_null` with a value | `` `is_null` on `due` takes no value `` |
| Other operator with null, array or object | `` `due` needs one non-null scalar value (null tests are `is_null`) `` |
| Wrong type | `` the value of `due` must be a date string `YYYY-MM-DD` `` |
| Wrong type inside an `in` | `` every value of `in` on `due` must be a date string `YYYY-MM-DD`, or null `` |
| A float for money | `` the value of `billed` must be a decimal string like "-12.50" or an integer (money never rides as a float) `` |
| A money value past 18 digits | `` the value of `billed` must be a money value of at most 18 digits (the column holds whole minor units in a 64-bit integer) `` |
| A decimal past 38 digits | `` the value of `rate` must be a decimal of at most 38 digits `` |
| A NUL character in a string | `` the value of `customer` must be a string with no NUL character `` |
| Empty `pick` | `pick must name at least one field (omit it to keep all fields)` |
| A field named twice in `pick` | `` `pick` names `customer` twice `` |

**Types a value must have:**

| Field type | Send |
|---|---|
| `text`, `uri` | a string with no NUL character (made NFC by the door) |
| `id` | an id string |
| `int` | an integer |
| `money`, `decimal` | a decimal string like `"29.90"`, or an integer. Never a float. Money within 18 digits of minor units, a decimal within 38 digits |
| `bool` | `true` or `false` |
| `date` | `"YYYY-MM-DD"`, a day the month has |
| `datetime` | `"YYYY-MM-DDTHH:MM:SSZ"`, UTC, `Z` required |
| `duration` | a non-negative integer in the field's unit |

`"29.90"` equals `"29.9"`. Money compares as an exact number.

**The two size limits.** They are deployment settings with defaults
(`docs/settings.md`). The refusal names the limit and the setting.

| Setting | Default | Counts |
|---|---|---|
| `ORDO_REFINE_MAX_CONDITIONS` | 16 | conditions in one `where` |
| `ORDO_REFINE_MAX_IN_VALUES` | 200 | values in one `in` |

```
this refinement has 17 conditions and the limit is 16. The limit is the deployment setting `ORDO_REFINE_MAX_CONDITIONS` (default 16). Fold values of one field into a single `in`, or ask the deployment owner to raise it
```

```
`in` on `status` has 250 values and the limit is 200. The limit is the deployment setting `ORDO_REFINE_MAX_IN_VALUES` (default 200); ask the deployment owner to raise it, or narrow the set with another condition
```

- The interface descriptor states both as `refine.limits`, so the SDK can
  check before it sends.
- A deployment refuses to boot on `0` or a non-number for either.

How to avoid them:

- Fold `status eq a`, `status eq b` into one `status in [a, b]`.
- A screen that offers "pick from 500 customers" wants a parameter or a
  second condition, not a 500-value `in`.
- If a screen truly needs more, ask the deployment owner to raise the
  setting.

### `refinement_unbounded` (HTTP 400)

A condition on a sealed field with no blind index, on an op where nothing
bounds the rows it would examine, and the op does not say `unbounded`.

```
refining `memo` on `ledgerCount` raises this request's effective cost class to `unbounded-host` (registered `sql`) — the condition cannot be answered in the database, so it opens sealed payloads for an unbounded number of rows. `ledgerCount` was not registered with the `unbounded` acknowledgment, and a refinement cannot acknowledge on its author's behalf
```

`param` names the field. A count is the usual way to meet it: the count's
conditions run over every row it counts, before the count.

### `sealed_unsupported` (HTTP 501)

A condition only the host can answer. Host-side filtering is not built yet,
so the door refuses it, naming the field.

```
a condition on `memo` is answered by opening sealed payloads on the host, and host-side filtering is not built yet. Give the field a blind index (`sealed indexed refinable`) so `eq` and `in` run in the database, or filter on an open field
```

The executor answers with the same class for a read whose host work grows
with the data (`unbounded-host`). Returning a sealed field is served; opening
one for every candidate row is not. The message names the field:

```
`withMemo` returns sealed field `orders.memo` and plans as unbounded-host: the payloads it opens grow with the data; bound it with `take n`, or make each opened row a row of the page (no group, distinct or many-side join over it)
```

### How to avoid the last two

In the order to try them:

1. **Give the field a blind index.** Before: `memo: text sealed refinable`.
   After: `memo: text sealed indexed refinable`. `eq` and `in` then run in
   Postgres as `blind-index`.
2. **Filter on an open field instead.** If the field does not need to be
   sealed, unseal it. Every operator then works and costs `sql`.
3. **Bound the op, or say `unbounded`.** A `take n`, or `unbounded` on the op,
   turns `refinement_unbounded` into `sealed_unsupported` today. Both clear
   only once host-side filtering is built. Use 1 or 2 now.

A sealed field that should never be filtered needs nothing: leave it
`sealed`. The refusal is then `invalid_refinement`, which is the answer you
want.

### A sync with `pick` (`invalid_refinement`)

```
a sync takes a refinement's `where` and refuses its `pick`: the device stores whole rows keyed by id, so a picked row would overwrite a whole one. Sync with `where` alone and pick on the device
```

`param` is `pick`. Section 9 has the reason.

---

## 7 · Open fields on big entities

**A condition on an open field is never refused, and it can still be slow.**
This is the cost the owner meant by *"could affect devs later
unexpectedly"*.

- The condition compiles into the op's SQL, under the page's `LIMIT`.
- With no index on that column, Postgres scans the candidate rows to find a
  page of matches.
- On 1,000 rows nobody notices. On 10,000,000 rows a rare value can scan the
  whole entity for each page.
- The class stays `sql`. Nothing in `ordo check` or the door warns you,
  because the compiler cannot know which fields your screens filter on.

**What to do:**

- List the fields each filter screen offers. Those are the fields to index.
- Index one open field with the modifier `index`: `status: text index`.
- Index a pair with the clause `index (status, due)`, the filtered field
  first. It serves conditions on `status`, and on `status` with `due`.
- `unique` on a field also gives it an index.
- A ref field is indexed for you. You do not need `index` on it.
- `indexed` is the blind index of a sealed field. It serves `eq` and `in`
  only, and it is refused on an open field.
- An index costs every write to that entity a little. Index what screens
  filter on, not every field.
- Read the SQL an op runs with `ordo plan <dir> <op>`; it is under
  `plan.sql`.

---

## 8 · Pages under a refinement

**Pages come back full.** Conditions on open and blind-index fields compile
into the op's plan, before the page window (ruled 2026-10-05).

- A refinement applies after everything the op does, its own `take`
  included. `take 50` then a refinement gives at most 50 rows, filtered.
- Send the same refinement with every page request, and the cursor from the
  last page.
- `next` alone says whether more rows exist. Stop when it is absent.
- Changing the refinement mid-listing is allowed. The new filter starts after
  the same row, as a parameter change does. It does not restart the list.
- `std.E.count` and `std.E.list` under one refinement describe one set. A
  "showing 20 of 312" header is the count and the list with the same
  `where`.
- The SDK's `.pages()` carries the refinement on every page for you.

---

## 9 · Live and sync with a refinement

**Live follows a refined read. A sync takes `where` and refuses `pick`.**

**Live** (`GET /op/name?live=1`, or a read added to the shared `/live`
stream with `{key, op, params, refine}`):

- The first frame is the refined read.
- Invalidation stays per op. A write that cannot change your filtered rows
  still costs one refetch that returns the same rows.
- Each refetch carries the refinement again.
- A cursor does not apply to a subscription. It follows the whole result.
- In React, two hooks on one op with different refinements are two reads on
  the shared stream. The door checks each refinement when its read is added.
  One write refreshes both.

**Sync** (`?live=1&sync=1`):

- `where` is accepted. A row that starts matching arrives as an `upsert`; a
  row that stops matching arrives as a `delete`.
- `pick` is refused (section 6). The device stores whole rows keyed by id,
  and a row held through two syncs stays until neither holds it. Pick on the
  device instead, at no cost.
- Two syncs of one op with different filters are two holders over one row
  store.

---

## 10 · On the device

**A synced op answers refinements on the device, and the server does no
work.**

- The device evaluates the same 8 operators with the same rules: `null ==
  null` is true, text compares by code point after NFC, money compares as an
  exact number.
- Sort is device-only. The wire envelope has no sort key.
- Sealed rules do not apply on the device. It holds only values it has
  already opened.
- If the op is not synced, the call goes to the network and the door
  decides.

When to sync and when to call:

- **Sync** a list a screen filters over and over, such as a table with
  filter chips. Each filter change is free.
- **Call** for a one-off lookup, or an entity too large to hold on a device.
  Index the fields the call filters on (section 7).

---

## 11 · Reading the class in production

**Today nothing logs a request's effective class.** The door computes it and
drops it.

- A deployment owner cannot yet see that a refinement raised a request from
  `sql` to `blind-index`.
- When tracing lands, the effective class rides `ordo.cost_class` beside the
  registered one.
- Meanwhile, slow refined reads show as slow queries in Postgres. The fix is
  section 7.

---

## 12 · Before you ship a screen with filters

1. Run `ordo check` and read the op's `refinable:` line. Every field the
   screen filters on is in the first two groups.
2. A field the screen filters with a range or sort is open, not sealed.
3. Each sealed field the screen filters on is `sealed indexed refinable`, and
   the screen offers only `eq` or a pick-list (`in`).
4. Each open field a filter targets on a big entity leads an index.
5. No filter can build more than 16 conditions or an `in` of more than 200
   values, unless the deployment owner has raised the setting.
6. A synced screen sends `where` only and picks on the device.
