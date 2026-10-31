# `@hominemai/ordo-react`

The headless React runtime for Ordo's generated UI. See `ui-gen.md` §2, step 2 of
the build order behind the interface descriptor (§9.1, landed) and ahead of the
default skins (§3).

It renders almost nothing. Every export is state, a verb, or a type, except
`TenantGate`, which shows the app developer's own content while a person's
tenant is set up (see "A person's own tenant"). What a table looks like is
not this layer's business; a skin maps these hooks onto its own components,
and any design system can.

**Breaking in 0.3.0.** `SPEC_LIST` (from `@hominemai/ordo-sdk`) became
`specList()`. Call the function.

**Breaking in 0.5.0.** The deprecated `OrdoClient`, `OrdoClientOptions`
and `CallOptions` aliases are gone; use `DescriptorClient`. `useOrdoAuth`
wraps the SDK's `createOrdoAuth`. `withPlatformOps` reaches `admit`,
`suspend`, `reinstate`, `remove`, `leave` and `members`. Needs
`@hominemai/ordo-sdk` 0.5.0.

**Changed in 0.4.0.** One request helper, one retry policy and one error
shape, the SDK's (needs `@hominemai/ordo-sdk` 0.4.0):

- `OrdoClient` is now `DescriptorClient`, because `OrdoClient` is the SDK's
  generated-op client. `OrdoClient`, `OrdoClientOptions` and `CallOptions`
  still work as deprecated aliases of `DescriptorClient`,
  `DescriptorClientOptions` and `DescriptorCallOptions`.
- Every call goes through the SDK's `doorRequest`. Each now has a deadline,
  `callTimeoutMs` (60 s by default), and a call that got no answer throws an
  `OrdoNetworkError` (`timeout`, `unreachable`, `blocked`). Wrapping `fetch`
  in `namedFetch` is no longer needed. The live hub honours a refusal's
  `Retry-After`.
- `OrdoRefusal` is the SDK's class, re-exported. `.refusal` is still there,
  the same object as `.detail`. A code this package does not know is now
  class `internal`, with the code kept in `.code`; it was kept as the class.
  The refusal also has `status`, `retryAfterMs` and `retryable`.
- `OpResponse` is the SDK's and carries `pin`.
- `AccountEmailError` and `TenantStatusError` are `OrdoRefusal`s. The
  account-email calls are the SDK's; this package's form, the token inside
  the endpoint, still works and is deprecated.
- `checkRefine` is the SDK's and takes a descriptor op. `encodeRefine(op,
  refine)` still works and is deprecated: call `checkRefine(op, refine,
  'door')`.
- The file calls are the SDK's, with nothing translated.

**`useOpPages` and memoising.** `rows` keeps a stable reference. `version`
changes on every page. Memoise on `version`, not on `rows`.

---

## What it is, and what it is not

**It is a runtime over a descriptor, not a generated client.** Nothing here is
emitted by a build. The package reads `interface.json` at run time and derives
its behaviour from it: the HTTP method comes from `call.method`, the legal
call-site verbs from `call.terminal`, the version pin from `op.version`, the
row actions from `id_types`, the form from `params[]`. There are no op-name
heuristics anywhere in the source, which is the property §B3 stamped the `call`
contract to buy.

That is also why it is not the generated TS SDK (14d). The SDK will be a *pure
function of* `interface.json` producing branded ids and per-op literal types at
the caller's compile time. This package is the thing an app holds at run time,
and where the two overlap (terminal verbs, typed refusals, the idempotency
envelope), the SDK will be the compile-time enforcement of what this package
enforces dynamically. They compose; neither replaces the other.

**Zero styling opinions.** `<OrdoProvider>` renders its children.
`<TenantGate>` renders the app developer's slots, and a plain unstyled
default for a slot left unset. Dependencies: React and `@hominemai/ordo-sdk` as peers, and nothing
else. The shared code (retry policy, refusal classes, refine types, file transfers, JSON check) is imported from `@hominemai/ordo-sdk`, never copied (owner, 2026-10-23); `test/no-sdk-copies.test.ts` fails if this package redeclares an SDK export. Install both packages in an app.

---

## Installing it in an app outside this repository

The package is private, published to GitHub Packages under the HominemAI
organization.

1. Add this line to the app's `.npmrc`:

   ```
   @hominemai:registry=https://npm.pkg.github.com
   ```

2. Create a personal access token (classic) with the `read:packages` scope.
   Keep it out of git: put `//npm.pkg.github.com/:_authToken=<your token>` in
   your user `~/.npmrc`, not the project's.

3. Install:

   ```
   npm install @hominemai/ordo-react @hominemai/ordo-sdk
   ```

A release is a version bump in `package.json` merged to `main`; the
`Packages` workflow publishes it.

---

## Running it

```
cd clients/ordo-react
npm install          # or pnpm install; the package is pnpm-compatible
npm run typecheck    # tsc --noEmit, strict, includes the type-level tests
npm test             # vitest run
npm run check        # both
```

The tests need no server and no DOM. They run against
`crates/ordo-cli/tests/golden/market.interface.json` (the emitter's own
golden) and a mock door. A hand-written fixture would be a second, drifting
statement of the contract, which is the thing `interface.rs` refuses for
itself.

> **Owed:** the JS build is not wired into cargo or CI. Nothing runs
> `npm test` on a Rust change today, so a descriptor shape change can break
> this package silently. Wiring it is a follow-up, deliberately not done here.

---

## The contract

**Settled, and this package implements it:** `ui-gen.md` §1–§4: surface
descriptors as derived truth, typed-id action discovery, CAS-by-default
commit, the headless-runtime shape, per-surface ejection. Plus
`api-engine-design.md`'s wire: route model, request envelope, error taxonomy,
parameter binding, keyset pagination, idempotency, version skew.

**Tracks the descriptor version.** The package speaks `ordo.interface/2`.

- An unknown **major** is refused loudly (`SchemaVersionError`) at
  `parseInterface`. Reading a document whose keys no longer mean what the
  package thinks is worse than not starting. `/1` is refused too, and not
  hypothetically: there `call.cas` meant *is a mutation*, so a `/1` document
  read by this package would put a CAS refusal in front of every create and
  every delete.
- An unknown **minor**, or an unknown key, is accepted. `interface.rs` states
  the rule: the version moves on a breaking change, and *adding a key is not
  breaking*. A consumer that refused additive changes would make every server
  addition a client outage.
- Per-op **version hashes** ride as `X-Ordo-Version` and skew comes back as a
  soft `stale` flag on every hook's result. The pin is per-op, never
  registry-wide (§10): a signal that fires after every unrelated deploy is
  one clients learn to ignore.

**Catalog scoping is honoured, never second-guessed.** A descriptor contains
exactly the ops the token may call. So an absent op is `undefined`, and the
package never distinguishes "no such op" from "not yours", the same
non-enumerability the door keeps (§4.1). An unknown typed-id prefix is
reported as unknown, not as invalid.

---

## The hooks

| Hook | Contract |
|---|---|
| `useOpForm(op, initial?)` | Field model derived from `params[]`: required/optional, `max`/`min` bounds, `id_entity` params as typed references whose candidates come from `id_types[].list`, enum params as option lists off `params[].enum_values`, plus values, typed per-param validation using the door's own classes, and the bound payload. A `json` param gets the form kind `json` and the value type `JsonValue`. |
| `useOpQuery(op, {params, refine, pick, enabled, live, livePaceMs, device, sort})` | One `.fetch()` over a read op: rows, `row` for a `single_row` result, loading, a typed `refusal`, the skew flag, and `refetch()`. With live on it refetches when the door says the data changed, paced at the provider's gap or the longer `livePaceMs`. With a device copy a syncable op is answered from the device, and `source`, `deviceFallback` and `complete` say where the rows came from. `revoked` is true when `refusal.class` is a revocation (`access_denied`, `access_guard`, `delegation_required`): the hook shows no rows, and a synced op's device rows are gone with the replica poisoned. **Risk: with a device copy, every syncable read is copied to the device, sealed at rest, until sign-out or a revocation. `device: false` is the opt-out, per hook.** A list that is sensitive on a shared or lost device (who holds access, an audit trail) should set it, or be one of the reads the package already refuses to mirror (`std.space.*` and an acl entity's `<Entity>Access` list). The package does not copy more by default than the descriptor marks syncable, and it keeps the permission map off the device without being asked. |
| `useOpPages(op, {params, refine, pick, live, livePaceMs, device})` | A keyset listing: accumulated rows, `hasMore` (which is `next !== undefined`, never `rows.length > 0`), `more()`, `exhaust()`, `restart()`, and a `restarted` refusal when the cursor went stale. With live on it reads the pages on screen again; on the device it answers whole and `hasMore` is false. |
| `useRow(id)` | Typed-id action discovery from an id alone, with no fetch. Resolves the prefix through `id_types` and exposes the actions and detail ops the descriptor grants, each pre-bound to the op's own param name. It reads nothing, so there is nothing for live or the device copy to follow; read the row with `useOpQuery` through the entity's `get` op. |
| `useCommit(op, {autoResolve})` | The CAS-by-default write path as a state machine: `idle → committing → committed \| conflict \| refused`, with `resolve()` to refetch and `retry(values, rev)` to remerge. A conflict's `currentIs` says what is known of the stored row: `not_fetched`, `fetching`, `found` (the row is in `current`), `deleted` (gone, so `current` stays `null`) or `unavailable` (the catalog grants no read for it, or the read failed). `current: null` alone says none of these. `autoResolve: true` reads the row back by itself. With a device copy, a committed write tells it to ask the server again. A write with no network is refused, never queued. |
| `useOpQuery` / `useCommit` on a platform op | `std.space.share`, `shareLink`, `revokeShare`, `revokeLink`, `links`, `shares`: same hooks, same states, rows typed by the op's name. They are not in `interface.json`, so the app names the ones it uses with `withPlatformOps(iface, ...)` (see Platform ops below). |
| `guestClient({baseUrl, iface, feed})` | Not a hook: the client a guest's `<OrdoProvider>` takes, built from the SDK's `windowFeed` or `linkFeed` so the token never reaches the page (see Guests below). |
| `useOrdo()` / `useInterface()` | The client and the loaded descriptor from context. |

Each of them exposes `refusal` typed to the closed inventory. For per-op
literal unions, `RefusalOf<typeof doc, 'std.tasks.update'>` narrows to that
op's declared set when the descriptor is imported `as const`, proved in
`test/types.test-d.ts`.

### What `/2` closed, and what this package stopped doing

The first build of this package reported seven facts a renderer needed and
`ordo.interface/1` did not carry. All seven are in `/2`, and the point of
naming them here is what came out of `src/` as a result. Every one of them
had a workaround, and a workaround in a package whose argument is *"it reads
facts"* is a liability even when it is correct.

| Gap | Now read from | Deleted from here |
|---|---|---|
| nothing indexed *the list op for entity E* | `id_types[].list` / `.get` | the `row_identity` + no-required-params reconstruction, and its `std.E.list`-first **name rank** |
| a param carried no `enum_values` | `params[].enum_values`, linked by `field_ref` | the `statements[].entity` × same-field-name join |
| a write's `result` was empty | `result.shape`, `statements[].summary` | none (there was no workaround; a form simply could not know) |
| `call.cas` meant *is a mutation* | `statements[].cas_param` | `name === 'rev' && type === 'int'`, the last name heuristic in the source |
| `single_row` ops advertised `pages` | `call.terminal` / `call.paged` | none (`.pages()` on a metric is now refused by `assertTerminal`, from the descriptor) |
| `params[].max` never populated, no `min` | both, as canonical digit strings | a dead `typeof max === 'number'` branch that could never fire |
| `call.refinable` disagreed with `refine.tier` | one fact, projected | the paragraph explaining which to believe |

Two of those are behaviour, not bookkeeping:

- **A picker's "blind" answer is now the descriptor's**, not this package's.
  `members` is platform-provided and no visible op returns its rows, so
  `id_types` for it carries an empty `list`, and an empty list that was
  *stated* is a different thing from an empty list that fell out of a
  heuristic, because only the first can be trusted.
- **A `cas_miss` refetch goes through `id_types[].get`**, not through
  `detail[0]`. `detail` is every read that *accepts* the row's id, including
  ops that return something else entirely; reading a row back through one of
  those and presenting it as "the current value" is exactly the failure the
  conflict state exists to prevent. Under `/1` there was nothing better to
  pick from.

### CAS by default means the default is not optional

`ui-gen.md` §1 settles that the fetched row's `rev` rides the write and
`cas_miss` is a typed conflict resolved by refetch, never a silent overwrite.
Three rules make that unavoidable:

1. **A CAS op cannot be committed without a `rev`.** Not "CAS if you pass one":
   the write is refused locally, before the wire, with the door's own
   `missing_param`. There is no last-write-wins path through this runtime.
2. **`cas_miss` never retries.** It becomes a `conflict` state with the
   attempted params, the rev that lost, the row's address, and a `refetch()`
   affordance that reads the row back through the catalog's own detail op.
   Where the address is ambiguous the runtime answers `null` and
   does not refetch the wrong row.
2b. **The `rev` always comes from a read, because a commit does not return
   one.** `result.shape` says `write_summary`: the response is one object per
   statement: `{entity}` plus `created`/`converged` (an id) or
   `updated`/`deleted` (a count). The written row is not in it.
   `api-engine-design.md` §3 says an addressed write returns the row
   post-`rev` as `rows[0]`; the executor does not, the descriptor describes
   the executor, and the divergence is filed against the engine. So a form
   that wants to commit twice re-reads through `get` between the two.
3. **A retry needs the refetched `rev`.** Re-submitting the stale one is
   refused locally, so "retry" cannot quietly mean "retry the losing write",
   and a fresh attempt mints a fresh idempotency key. The same key with a
   changed payload is `idempotency_mismatch`, permanently for a creating op.

### `cas_miss` and `version_skew` are different conflicts

Both are 409s and both are typed conflicts the consumer must handle, but the
repairs are opposites and the runtime keeps them apart:

- **`cas_miss`**: stored data disagreed. Refetch the row, remerge, retry
  against the new `rev`. Surfaces on `useCommit` as `conflict`.
- **`version_skew`** (and `invalid_cursor`): the cursor is stale. The
  data is fine. Keyset columns may no longer mean the same thing, so the token is
  dropped and the listing restarts from page one (§8: the token is not
  migrated). Surfaces on `useOpPages` as `restarted`.

Putting a refetch-and-remerge dialog in front of someone who was scrolling a
table is what conflating them looks like.

### Paging stops on a missing `next`, and on nothing else

Open-field conditions compile into the plan, so a refined page comes back
full (ruled 2026-10-05). A short or empty page with `next` set is still legal,
so a runtime must not stop on one. `hasMore` is therefore
`next !== undefined`, the empty state is `empty && !hasMore`. That is a fact about the
whole iteration, never about one page. `collectAll` keeps pulling through
empty pages. `maxPages` exists as a runaway guard and leaves `hasMore` true
when it trips: the runtime stopped, the listing did not end.

### Refinement: every program read, typed

Every program read takes a refinement, with no off switch (ruled
2026-10-05). The descriptor says so with `refine.tier: "basic"`; a write, a
platform op and a public read say `none`. The hooks do not refuse on the
tier. The door is the authority and its refusal is typed.

```tsx
import type { VendorSpendRow, VendorSpendFields } from './generated';
const vendorSpend = opRef<VendorSpendRow, VendorSpendFields>('vendorSpend');

const q = useOpQuery(vendorSpend, {
  refine: (v) => [v.billed.gt('4000'), v.status.in(['open', 'late'])],
  pick: ['name', 'billed'],
});
// q.rows: readonly Pick<VendorSpendRow, 'name' | 'billed'>[]
```

- `refine` is a callback over the field proxy, or the wire shape
  `{ where, pick }` for a UI that builds it from state.
- With an `opRef` carrying the SDK's generated types, a wrong field, a wrong
  value type or an ordering on a sealed field does not compile.
- `pick` narrows the row type. It replaces a pick inside `refine`.
- The envelope is checked before the wire with the SDK's rules and words.
  `test/refine-agreement.test.ts` runs both copies over the whole catalog.
- Each hook's answer is its own, keyed by op, params and refinement. Two
  hooks on one op share one live signal, since a refinement changes which
  rows show and never which writes can change them. Each refetches with its
  own refinement.

`refinementSurface(op).enabled` follows the tier. A skin draws filter chrome
on a `basic` read and sort-only chrome on a `none` read.

## Retrying a busy server

`DescriptorClient` takes the same options as `@hominemai/ordo-sdk`: `busyRetry`
(`retries` 2, `firstDelayMs` 500, `factor` 2, `maxDelayMs` 10000, `jitter`
on) and the shorthand `busyRetries`. A `503` with class `server_busy` is sent
again for `.fetch()`, `.pages()` and `.commit()`. Retry `n` waits
`min(maxDelayMs, firstDelayMs * factor^n)`, never less than the `Retry-After`
(held to `maxDelayMs`), plus up to 20% random jitter. `retries: 0` turns it
off, and a bad value throws `TypeError`. A write carries one `Idempotency-Key`
for every attempt, and no other 503 class is retried. When the retries run
out the refusal has class `server_busy`. The live streams are opened once
each; the live hub reconnects on the same classes, with the door's
`Retry-After` as a floor under its backoff. The policy and the backoff are
the SDK's (`doorRequest`, `reconnectDelayMs`).

A `503` with class `tenant_not_ready` (the person's own tenant is still being
set up) is retried the same way, with the same options, counted on its own.
When those retries run out the refusal keeps its class; `TenantGate` is what
keeps the person waiting.

```ts
new DescriptorClient({ baseUrl, iface, space, token, busyRetry: { retries: 4, firstDelayMs: 250 } });
new DescriptorClient({ baseUrl, iface, space, token, busyRetries: 0 }); // off
```

A `409` with class `serialization_conflict` (a write lost a race with other
writes to the same rows, and nothing was applied) is sent again too, with
the same options as the SDK: `conflictRetry` (`retries` 3, `firstDelayMs` 50,
`factor` 2, `maxDelayMs` 1000, `jitter` on) and the shorthand
`conflictRetries`. With jitter each wait is a random time from half the
computed wait to all of it. The write keeps its `Idempotency-Key`, no other
409 class is retried, and `retries: 0` turns it off. `onRetry` gets the
reason as its third argument.

```ts
new DescriptorClient({ baseUrl, iface, space, token, conflictRetry: { retries: 5 } });
new DescriptorClient({ baseUrl, iface, space, token, conflictRetries: 0 }); // off
```

---

## Ejection

`ui-gen.md` §4 settles that ejection is an **off-ramp, never the delivery
mechanism**: `ordo eject surface <op>` writes a copy-in component the app owns
outright, per surface, never all-or-nothing. The tooling belongs to step 4's
registry work and is **not built here**. What this package owes it is a clean
seam, and the seam is this:

**What a surface consumer owns after ejecting:**

- The component: markup, styling, layout, copy, empty and error presentation,
  the shape of the conflict dialog, how a picker looks.
- The composition: which ops a page calls and how their state is arranged.
- The pinned `op.version` hash the component was built from, and the decision
  about what to do when `X-Ordo-Version-Skew: stale` comes back. Leaving the
  runtime does not mean leaving drift detection: the headers ride regardless.

**What stays in the runtime, and what an ejected component should keep
calling instead of reimplementing:**

- `DescriptorClient`: method mapping, terminal-verb enforcement, the space/token/
  version/cursor headers, the idempotency envelope, error-envelope decoding.
- `commitOnce` / `retryAfterConflict`: the CAS rules above. An ejected
  component that reimplements the commit path is how a silent overwrite gets
  written, because the easy version is the wrong one.
- `discover`: action discovery. It is one lookup and it is derived after the
  catalog filter; a component that re-derived it from `params[]` could offer
  an action the token cannot call.
- `nextPage` / `collectAll`: the `next`-only terminator.
- `parseInterface`, `deriveForm`, `validateForm`, `refinementSurface`,
  `REFUSAL_CLASSES`: everything that reads the descriptor.

Stated as one rule: **eject the rendering, keep the protocol.** Every function
named above is exported from the package root precisely so an ejected
component can import it, and the ejection tooling's job is to emit a component
that does.

---

## Live and the device copy

Both are switched on in one place, `<OrdoProvider>`, and both are off until an
app does. Neither needs any wiring in a component.

```tsx
const device = await openDevice({ client: sdkClient, space, principal });   // @hominemai/ordo-sdk

<OrdoProvider
  client={client}
  live={{ paceMs: 250 }}                                  // the shipped default; `live` alone uses it
  device={device}
>
```

`device` takes what the SDK's `openDevice` answers, and wraps it once with
`deviceCopy`. It also takes a `DeviceCopy` the app built itself:
`deviceCopy({ manager, replica, refineRows, refineMerge, refineRowsAsync })`.

`deviceCopy` takes the SDK's own objects and wraps them in the small
interface the hooks use. This package does not import the SDK, so it keeps
React as its only dependency. The SDK's `SyncManager` and `LocalReplica` are
assignable to `ManagerLike` and `ReplicaLike` as they are, with no cast, and
`SyncSpec` is the SDK's `OpSpec` shape, so a refined, sealed or limited read
keeps its refine types, sealed fields, limits and parameter places.
`refineRows` is the SDK's, and lets the device answer `refine` and `sort`.
`refineMerge` and `refineRowsAsync` are the SDK's too. The first merges each
frame into an answer; the second refines a large first answer in slices.
Without `refineRowsAsync`, a refinement over `refineChunk` rows (5,000 by
default) runs in one task and the device warns once in the console.

**Staying current, with no polling.** An app does nothing for this. The SDK's
manager keeps each feed open, the server pushes a change (a tenant made after
the server booted included), and the manager reconnects by itself when the
browser comes back online. Nothing reloads when a tab regains focus, and no
timer runs (owner, 2026-10-21). `copy.reload()` reopens every feed on the same
manager (`reconnect()`) and resolves when each has answered; screens keep
their rows meanwhile. `copy.nudge()` reconnects every feed now without
waiting. Both do nothing on a revoked device. A committed write reopens
nothing: the open feed brings the change.

**A sync nobody uses closes after 15 s.** Each read hook holds a use of its
op while mounted (`copy.use()`), and gives it back when it unmounts. When an
op has had no use for `idleMs` (15 s by default, owner 2026-10-21), its feed
closes and its rows stay on the device. The next screen shows them at once
and the feed resumes. React's mount, unmount, mount in StrictMode lands
inside the delay, so it opens one feed. `copy.declare()` is a use never
given back. Set `idleMs` on the SDK's `openDevice`, or on `deviceCopy`;
`Infinity` keeps every sync open.

**Several tabs.** With the SDK's `openDevice`, the tabs of one person share
one set of feeds: a leader tab runs them and passes each frame to the others.
Nothing changes for the hooks. Serve the door over HTTP/2 too
(`docs/deploying.md`).

**A manager that is replaced.** `reload` is optional: an app that replaces its
manager on its own schedule passes `reload`, and `copy.reload()` runs it and
follows the result. `manager` may then be a function that answers the one now
current: `manager: () => device.manager`. The adapter looks at it on every
call. When it answers a different manager, the adapter declares everything the
hooks asked for on the new one and moves its watchers across, so the app keeps
no table of declarations.

### Live

A read hook follows the live feed when the provider says `live` or the hook
says `live: true`. `live: false` on a hook opts it out. A hook follows only if the
op's `call.live` is true.

- The door sends one `invalidate` per change. It is a staleness signal, never
  a data push, so the hook refetches through the op under its own scope.
  `useOpPages` reads the same number of pages again from the top and swaps
  them in whole, so a user ten pages down keeps their place.
- **One stream per client** (27s). The provider's client opens one stream,
  `GET /live`, and every hook shares it. Each distinct op and params is one
  read on it, and each `invalidate` names the read it is for
  (`api-engine-design.md` §6.1).
- The first fetch waits for the hook's read to be added. The door subscribes
  before it answers, so a fetch made after that cannot miss a change. If the
  door refuses the read, that hook's read goes ahead and `live.state` says
  `unsupported` or `failed`. The stream and the other hooks go on.
- A stream that ends is reopened with backoff, and the reopening counts as an
  invalidation, because signals may have been missed while it was down.
- Hooks that read the same op at the same params share one read. The last
  one to leave drops it at the door, and the last read leaving closes the
  stream.
- **Pacing is a setting.** Ruled 2026-10-01 (27n): group bursts and wait at
  least a quarter second between refetches, as a setting. A burst of
  invalidations is one refetch, and a stream of them is one refetch per gap.
  The gap is the provider's `live.paceMs`; unset, it is `DEFAULT_LIVE_PACE_MS`
  (250 ms), the one place the number lives. A hook's `livePaceMs` may lengthen
  the gap for itself and never shorten it: the provider's value is a floor
  (`effectivePaceMs`). The pace a program's `settings` block will state
  (`plan-syntax-additions.md` §9b) is not built yet; today the app passes it.

### The device copy

With `device` on the provider, a read hook answers from the device replica
when the descriptor says the op is syncable (`result.sync.syncable`, with the
row's id in `result.sync.id`). The SDK's `SyncManager` keeps the op current
over its own feed, so a synced hook does not also open a live stream. The
hook says where its rows came from:

| Field | Meaning |
|---|---|
| `source` | `'device'` or `'network'` |
| `deviceFallback` | why a read went to the network: `not_syncable`, `never_mirrored`, `opted_out`, `refine_unavailable`. `null` when the device answered, or when no device copy is configured |
| `deviceFallbackReason` | the rule in words, for example that the op is an aggregate |
| `complete` | false while a device load is still arriving, or when the rows are from an earlier session that nothing has confirmed yet |

- **Offline.** The device answers with no network. A device that was closed
  and reopened offline answers from what it stored, with `complete: false`.
- **Order.** The replica keeps rows by id and the descriptor does not carry
  the op's sort, so device rows come in id order. Pass `sort` for another
  order. It has no wire form and runs only on the device.
- **Refinement.** The device answers a refinement itself, because refining
  rows already held asks the server for nothing. This needs `refineRows`.
- **Commit.** After a committed write the hook calls `afterCommit`, which runs
  the app's `reload` when it passed one. The open feed carries the change, so
  most apps pass none. The write's own answer is a `write_summary`, not the row, so
  nothing is applied to the device from it. A refused write does not touch the
  device.
- **Revocation.** If the door refuses a synced op's feed with `access_denied`,
  `access_guard` or `delegation_required`, the SDK releases the rows that op
  held and the hook empties. The hook says `revoked: true` beside
  `refusal.class`, `copy.revoked` turns true, and the adapter poisons the
  replica so it holds nothing and serves nothing (turn it off with
  `poisonOnRevoke: false`). The door answers one `access_denied` for "not
  allowed" and "closed", so a feed it refuses for good is treated as a
  revocation. A network refusal of that kind on a syncable op empties the hook
  and drops the op's device declarations. A row another synced op still holds
  stays on the device until the replica is poisoned, and that op answers until
  then.

### What the device copy covers

The device copy is not a cache of all data. It holds the rows of the ops the
app has asked for, at the parameters it asked for them, for one person in one
space.

**Covered.** Only reads the descriptor marks syncable: anchored, ungrouped
reads whose rows carry the anchor's id.

**Never covered.**

- `std.space.*`. The permission map and the audit spine do not rest on a
  device.
- The access list of an acl entity: the reads of `<Entity>Access` when
  `<Entity>` declares `acl_levels` (`std.notesAccess.list`). It says who may
  read what, which is the permission map. The hook reports
  `deviceFallback: 'never_mirrored'`. This is the one default the package
  changed on the strength of being clearly safer: no app wants a list of who
  can read a note on a device the next person may hold, and an app that did
  would be holding the permission map by name.
- Aggregates (`measure`), grouped reads, `distinct`, many-side joins, and
  reads with no anchor. The descriptor says `syncable: false` with the rule.
- Writes. A write is always a network call.

These go to the network, and the hook says so in `deviceFallback`.

**Offline writes are refused, not queued.** A commit with no network ends in
`refused` with a message that says nothing was saved. Nothing is kept to send
later, and coming back online sends nothing by itself.

**Not built.** Optimistic local writes, and a write queue. Both need the
deterministic evaluator (`plan-client-cache.md` §6). The page-level snapshot
coordinator is also not built (below).

### One device per space and person

An app that shows more than one space (a person's own, and each one shared
with them) holds a device for each. `DeviceRegistry` opens each when it is
first used and closes it when nothing uses it. Devices are keyed by space
and person (owner, 2026-10-21), so two people on one origin never share one:

```tsx
const registry = new DeviceRegistry<{ device: OrdoDevice; hooks: DescriptorClient }>({
  open: async (space, person) => {
    const clients = await windowInto(session, space);
    return { device: await openDevice({ client: clients.sdk, space, principal: person }), hooks: clients.hooks };
  },
  close: ({ device }) => device.close(),   // optional: a device with its own close() needs none
  purge: ({ device }) => device.purge(),   // optional: a device with its own purge() needs none
});

<DeviceRegistryProvider registry={registry}>
  <Space space={space} />
</DeviceRegistryProvider>

function Space({ space }: { space: string }) {
  const { device, error } = useSpaceDevice<{ device: OrdoDevice; hooks: DescriptorClient }>(space, person);
  if (error) return <p>This space cannot be opened.</p>;
  if (device === null) return <p>Loading.</p>;
  return <OrdoProvider client={device.hooks} device={device.device}>...</OrdoProvider>;
}
```

- The first `useSpaceDevice(space, person)` calls `open(space, person)` once. Every later one,
  from any screen, shares the answer.
- When the last screen using a space unmounts, its device closes after
  `idleCloseMs` (default 0, the next turn of the event loop, so React's double
  effect in development does not open it twice). A screen that mounts before
  then keeps it.
- `DeviceRegistryProvider` calls `registry.closeAll()` when it unmounts. The
  registry can be used again afterwards.
- A failed `open` is reported as `error` and is not kept, so the next screen
  tries again.
- `registry.purgeAll(person)` closes and erases every device opened for that
  person, open now or closed since. Sign-out calls it. With no person it
  erases every device.
- `registry.opened()` lists the devices that are open now. `acquire(space, person)` is
  the same lease without React: it returns `{ device: Promise, release() }`.
- `useSpaceDevice(null)` holds nothing.

The registry does not import the SDK. It keeps whatever `open` returns.

## A person's own tenant

On a deployment that gives each person a tenant of their own
(`ORDO_PROVISION_SELF_SERVE=on-signup`), sign-up answers at once and the
tenant is made after it. Until it is ready, nothing can be written to it: a
call into it is refused `503 tenant_not_ready`, which the client retries like
`server_busy`. The server never stops trying a setup, and after the
deployment's `ORDO_PROVISION_STUCK_SECS` (default 300) it reports it stuck
(owner, 2026-10-21). `docs/howto-sign-in.md` section 4 has the whole story.

What the person sees meanwhile is the app developer's choice. `TenantGate`
takes a slot per phase and renders the app once the tenant is ready. It needs
no `DescriptorClient`, because a client names a space and the space is what it
waits for:

```tsx
<TenantGate
  baseUrl={DOOR}
  token={session.token}
  settingUp={<Spinner label="Setting up your account" />}
  stuck={(t) => <Notice>{t.status?.reason}</Notice>}
  failed={(t) => <Retry onClick={t.retry} />}
>
  {(space) => <OrdoProvider client={clientFor(space)}><App /></OrdoProvider>}
</TenantGate>
```

| Slot | Shown while | Default when unset |
|---|---|---|
| `settingUp` | the status is being read, or the tenant is being made | "Setting up your account." in a `role="status"` paragraph |
| `stuck` | the setup has taken longer than `ORDO_PROVISION_STUCK_SECS` | the door's reason for the person, in a `role="status"` paragraph |
| `failed` | it cannot be made now (`failed`, `none`, `off`), or the session was refused | the reason in a `role="alert"`, with a "Try again" button where trying again can help |

A slot is a node or a function of the hook's result. A function child gets
the tenant's space id.

`useTenantStatus({ baseUrl, token })` is the same thing without rendering:

- `phase`: `checking`, `setting-up`, `stuck`, `ready` or `failed`.
- `status`: the door's last answer, with `step`, `stuck`, `attempts` and
  `reason`.
- `space`: the tenant's id once ready.
- `problem`: the last read that failed. `unavailable` keeps reading;
  `refused`, `limit_reached` and `malformed` stop it.
- `retry()`: `POST /auth/provisioning`, then read on.

It reads with pauses that grow from 250 ms to 2 s, every 10 s once stuck
(`stuckPauseMs`), and backs off to 30 s through failed reads. It has no time
limit, because the server has none. `readTenantStatus` and `retryTenantSetup`
are the two calls on their own. They mirror the SDK's `tenantStatus`,
`retryTenant` and `watchTenant`.

---

## Platform ops

The sharing ops the door serves beside a program's own, `std.space.share`,
`shareLink`, `revokeShare`, `revokeLink`, `links` and `shares`, are not in
`interface.json`, and `GET /interface` lists only the program's manifests the
token may call. Showing them there would be a server change. So the package
keeps a small typed table (`PLATFORM_OPS`, mirroring `share_specs` in
`ordo-host`) and the hooks take them like any op:

```tsx
const iface = withPlatformOps(parseInterface(descriptor), 'std.space.share', 'std.space.links');
// ... <OrdoProvider client={new DescriptorClient({ ..., iface })} stepUp={() => session.stepUp()}>
const share = useCommit('std.space.share');          // rows typed: ticket, expiresAt, ...
const links = useOpQuery('std.space.links');         // rows typed: id, state, opens, ...
```

The table keeps the "only what the token may call" property in two ways. It is
a catalog of what exists, never evidence of what may be called: the door
authorises each call, and a guest's window, a share link or a member without
the role is refused `access_denied`, which the hook shows as a typed
`refusal`. And the app names the platform ops it uses, so an op it did not name
is absent from the interface and `useCommit` on it throws the same "not in this
catalog" error as any op the token cannot call. If a door ever carries a
platform op in its own descriptor, that descriptor wins.

Platform writes want a passkey touch no older than a few minutes, and the door
answers one `access_denied` for "too old" and "not allowed". `stepUp` on
`<OrdoProvider>` is how the app asks for a fresh touch: `useCommit` on a
platform op that is refused `access_denied` calls it once and tries the write
again, with a new idempotency key. A second refusal is believed. Program ops
never call it.

## Guests

`openWindow` and `openLink` in the SDK return an SDK client whose token is
private, which the hooks cannot use. The SDK now also has `windowFeed` and
`linkFeed`, which return `{ space, token }` where `token()` answers the
current token, minted again a little before it ends. That function is what an
`DescriptorClient` takes as its `token`, so:

```tsx
const feed = await windowFeed(endpoint, () => session.token(), space);   // or linkFeed(endpoint, secret)
const client = guestClient({ baseUrl: endpoint.baseUrl, iface, feed });
<OrdoProvider client={client} device={guestDevice}>...</OrdoProvider>
```

The page never holds the token string: it exists only as the answer of
`token()`, and no client has it as a property. A guest's device needs the SDK's
own client for its sync manager; build it from the same feed
(`new SdkClient({ baseUrl, space: feed.space, token: feed.token })`) so both
share one mint schedule.

## Files

Hooks over the blob door, with the client's own space and token.
`docs/howto-files.md` has the whole story.

```tsx
const up = useFileUpload();
await up.upload({ entity: 'attachments', field: 'file', id }, file);
// up.phase, up.progress.fraction, up.error, up.cancel()

const img = useFile(row.file);       // img.url for <img src>, img.type, img.size
const info = useFileInfo(row.file);  // { type, size } from the door, no bytes
const dl = useFileDownload();
await dl.download(row.file, row.name);
```

- `useFile` reads the bytes with the token and gives an object URL. It
  releases the URL when the handle changes or the component goes. A `null`
  handle leaves it `idle`.
- `upload` and `download` put a failure in `error` and throw it again.
- The plain functions (`uploadFile`, `fileUrl`, `downloadFile`, `fileInfo`,
  `readFile`) take the client and work outside a component.
- A failed call is an `OrdoRefusal`, or the `fetch` error as it came. Give
  the client the SDK's `namedFetch` to get an `OrdoNetworkError`.

## Deliberately not built here

- **Page-level snapshot coordination.** `ui-gen.md` §2 settles it as shape:
  a coordinator that holds one command-log position across every surface on a
  page and advances the visible snapshot only when every affected refetch
  reaches the new position. It needs log positions on the read path, and this
  package's responses carry none (the SDK's carry `pin`). Live and the device
  copy are built without it, so two hooks on a page can show a refetch at
  different moments. On the device the SDK's manager already holds the
  common position (`SyncView.position`); the hooks do not gate on it yet.
- **Optimistic apply and typed rollback.** §2 lists them. They are correct to
  build on top of the conflict state machine, not underneath it, and doing so
  before a skin exists would be guessing at the rollback presentation.
- **Skins.** Step 3. Nothing here has a class name, a token, or an opinion.
- **Ejection tooling.** Step 4/5, per §9. The seam is above.
- **Surface descriptors** (`surfaces.json`) and the reserved annotations
  `label`/`desc`/`order`/`choices`/`danger`. The grammar does not lex them
  (`definition-language.md` §10.3), so nothing here reads them; `deriveForm`
  reports `danger` derived from `kind === 'delete'` and says so.

## Layout

```
src/
  descriptor.ts   types for ordo.interface/2, transcribed from interface.rs
  load.ts         parseInterface, the schema-major gate, typed-id parsing, indexes
  refusal.ts      the SDK's refusals re-exported, and RefusalOf
  client.ts       DescriptorClient: methods, headers, terminal verbs, over the SDK's doorRequest
  refine.ts       the Tier-0 envelope: the SDK's checkRefine, refinementSurface, opRef
  form.ts         params[] -> field model, and the door's checks run locally
  actions.ts      ui-gen §2 typed-id action discovery
  commit.ts       CAS-by-default: the three rules
  pages.ts        keyset state, next-only termination, restart-on-stale-cursor
  sse.ts          the event-stream reader the live feed uses
  live.ts         LiveHub (one shared stream per client) and the pacer
  device.ts       the device copy: the plan, the sync spec, `deviceCopy`
  platform.ts     the platform ops' descriptors and `withPlatformOps`
  guest.ts        `guestClient`, a client for a window or a link
  hooks/          thin adapters over all of the above
```

The core is pure and the hooks are adapters, which is deliberate: it is what
lets the whole contract be tested against the real descriptor with no React
render. Only the live and device hooks, which are React state around streams,
are rendered in tests.

The live and device hooks are tested with `react-test-renderer` (a dev
dependency), against the SDK's real `LocalReplica` and `SyncManager` and a
door that holds streams open (`test/live-door.ts`). No DOM.
