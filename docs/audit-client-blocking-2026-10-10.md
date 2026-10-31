# Audit: what can block a page in Ordo's browser clients, 2026-10-10

Scope: `clients/ordo-sdk`, `clients/ordo-react`, and the TypeScript that
`ordo gen` emits. The server only where it shapes what a client receives.

The premise is the owner's ruling of 2026-10-10 (board `staged-load`): Ordo
must never block a page's main thread by default. Staged loading is the app
developer's design, and Ordo documents how to do it (`docs/howto-loading.md`).

No product code was changed. Every finding names its fix; a later task builds
them.

## Summary

Every finding below is fixed or bounded as of 2026-10-11; each names its
commit. The numbers in this summary are the ones measured before the fixes.

| Severity | Count | Meaning |
|---|---|---|
| High | 4 | Grows with the data and freezes the page or holds a screen for seconds at 10k rows |
| Medium | 6 | Holds a screen or a button, or grows badly, under conditions that will happen |
| Low | 7 | Bounded today, or a cost per call rather than per row |

Two kinds of harm show up, and they are kept apart below:

- **Freeze.** One synchronous step holds the thread. Taps and scrolls do
  nothing until it ends. Anything over 50 ms is a long task.
- **Wait.** The thread stays free, but a screen has no rows until a long async
  chain finishes.

**The top five:**

1. **H1. A device-copy screen re-reads every row on every sync frame.**
   `clients/ordo-react/src/device.ts:489-501`, `hooks/useDevice.ts:83-90`.
   Quadratic in the load. Measured: 3,000 rows take 7.2 s instead of about 0.6 s.
2. **H2. Local sort and filter run as one synchronous task.**
   `clients/ordo-sdk/src/replica/refine.ts:239-255`. Measured at 100k rows:
   180 ms by text, 761 ms by a money field.
3. **H3. Every IndexedDB key scan reads every key in the database, then sorts.**
   `clients/ordo-sdk/src/replica/backend.ts:123-126`. Measured: 285 ms of
   synchronous filter and sort for 100k synced rows.
4. **H4. The device store writes one row at a time through one queue.**
   `clients/ordo-sdk/src/replica/replica.ts:85-118`, `sync.ts:147-152`. About
   6 IndexedDB transactions per row, and reads wait behind the whole load.
5. **M3. A live read does not fetch until the live stream opens, with no
   deadline.** `clients/ordo-react/src/hooks/useLive.ts:106`, `live.ts:296-330`.
   A stream that hangs leaves every live screen loading forever.

**What is already right.** The network path never walks rows: no per-row
decode, no reviver, no deep copy (`ordo-react/src/client.ts:394-413`). Live
updates are tiny `invalidate` frames, paced to one refetch per 250 ms
(`live.ts:421-438`). `useCommit` does not wait on a refetch
(`useCommit.ts:119`). All crypto is async WebCrypto. A device-copy hook shows
rows as soon as it has any and says `complete: false` until the load ends.

## How the numbers were taken

`clients/ordo-sdk/test/blocking.bench.test.ts`, run with:

```
ORDO_BENCH=1 npx vitest run test/blocking.bench.test.ts
```

Node 22 on an Apple laptop, real WebCrypto, the in-memory backend. The
in-memory backend has no IndexedDB latency, so every async figure below is a
floor; a browser adds the cost of each IndexedDB transaction. Without
`ORDO_BENCH=1` the file runs one tiny case of each, so the suite stays fast.

| Measure | 1k rows | 3k rows | 10k rows | 100k rows |
|---|---|---|---|---|
| `refineRows`, sort by text (sync) | | | 12 ms | 180 ms |
| `refineRows`, sort by money (sync) | | | 52 ms | 761 ms |
| `refineRows`, filter on money (sync) | | | 9 ms | 62 ms |
| `keys()` filter and sort, 3 keys per row (sync) | | | 8 ms | 285 ms |
| Sync load, `syncOp` (async) | 210 ms | | 1,756 ms | about 18 s, estimated |
| `scan` of one entity (async) | 23 ms | | 208 ms | |
| `refineOp`, read back and sort (async) | 85 ms | | 762 ms | |
| Load with one screen re-reading per frame (async) | 1,013 ms | 7,249 ms | about 80 s, estimated | hours, estimated |

The longest stall of the event loop during the async runs was 15 ms. The
async paths keep the page responsive; what they cost is time to rows.

---

## High

### H1. A device-copy screen re-reads every row on every sync frame

**Fixed** in `df8cfcea` (I1), `ec803c49` (I4) and `3b4d2fa7` (I6). A screen keeps its rows in memory and merges each frame. One screen settles at 0.9x of the load at 3k rows, 1.1x at 10k and 1.1x at 100k, with no store row reads.

- **Where.** `clients/ordo-react/src/device.ts:396-423` (`notify` on every
  manager view), `device.ts:489-501` (`read`), `hooks/useDevice.ts:83-90`
  (`device.watch(() => void refresh())`).
- **What happens.** The sync manager publishes a view after every frame
  (`replica/manager.ts:296-333`), which is every 50 rows. Each mounted
  `useDevice` (so each `useOpQuery` and `useOpPages` on the device) then runs
  `read`: list every row the op holds, copy and sort that list, `get` each row
  one at a time, then `refineRows` when the op states an order.
- **How it scales.** A load of n rows sends n/50 frames, and each read
  touches every row held so far: about n²/100 row reads per screen. The
  `latest` counter drops stale answers, but every read still runs. They share
  the replica's one queue with the load, so the load slows too.
- **Measured.** 1,000 rows: 1,013 ms against 210 ms for the load alone.
  3,000 rows: 7,249 ms. Tripling the rows multiplied the time by 7.
- **Estimate.** 10k rows: about 80 s. 100k rows: hours. Each finished read
  also runs H2's sort on the main thread: 2,000 sorts of up to 180 ms at 100k.
- **Fix.**
  - Coalesce: one read in flight and at most one pending per hook, as
    `useOpPages` already does with `busy` and `dirty`.
  - Skip a read when this handle's `loaded`, `complete` and `position` did not
    change. The manager notifies for every op; most notices are not this one's.
  - Keep a per-handle row map in memory, filled from the frames, so a read
    does not go back to the sealed store.
  - Read in one IndexedDB transaction (`getMany`), not one per row.

### H2. Local sort and filter run as one synchronous task

**Fixed** in `38188c63` (I5). 100k rows sort by money in 58 ms and by text in 83 ms. A sliced refine of 100k rows stalls at most 4.5 ms.

- **Where.** `clients/ordo-sdk/src/replica/refine.ts:220-266` (`refineRows`),
  with `compareDecimal` at `clients/ordo-sdk/src/core/client.ts:510-522` and
  `asDecimal` at `core/refine.ts:95-99`.
- **What blocks.** The whole set is filtered, then sorted once per key. Each
  key wraps every row in a new object, sorts, and unwraps. A money or decimal
  key runs a regex, a `split` and two `BigInt` builds inside every comparison,
  so about 1.7 million times for 100k rows.
- **When.** Every device read of an op that states an order (`device.ts:501`),
  every `sort` or `refine` on a device-copy hook, and `refineOp`. Through H1,
  once per frame during a load.
- **Measured.** 10k rows: 12 ms by text, 52 ms by money. 100k rows: 180 ms by
  text, 761 ms by money, 62 ms to filter on money. Over 50 ms is a long task.
- **Fix.**
  - Decorate once: parse each decimal to a scaled `BigInt` once per row, then
    compare cached values. This removes most of the money cost.
  - One composite comparator in one sort, not one sort per key.
  - Skip the sort when rows already come in the stated order.
  - Above a threshold (about 5,000 rows), run the refine in a Web Worker, or
    filter in chunks with `scheduler.yield()` and sort in the worker.
  - Cache the answer by handle, query and position, so an unchanged result is
    never sorted twice.

### H3. Every IndexedDB key scan reads every key in the database, then sorts

**Fixed** in `75bdf0f8` (I2). Key scans ask for the replica's range: 5.8 ms beside 300k other keys, against 506 ms unranged.

- **Where.** `clients/ordo-sdk/src/replica/backend.ts:123-126`:
  `getAllKeys()` with no range, then `.map(String).filter(...).sort()`.
- **What blocks.** It reads every key of every replica on the device: every
  space, every person, every entity, and the hold records. The browser
  structure-clones the whole key array onto the main thread, then the filter
  and sort run there.
- **Callers.** `scan`, `holdings` (every device read, every sync start, every
  `refineOp`, every drop), and `purge`.
- **Measured.** Each synced row has 3 records (the row, its holders, one
  hold). For 100k rows that is 300k keys of about 200 characters. The filter
  and sort alone took 285 ms; 8 ms at 10k rows. The clone of the array comes
  on top.
- **Fix.** `getAllKeys(IDBKeyRange.bound(prefix, prefix + '￿'))`. Keys are
  hex and `/`, so the range is exact, and IndexedDB returns keys sorted, so
  the sort goes. Better still, use `getAll` over the same range, so rows come
  back with their keys in one request (H4, M1).

### H4. The device store writes one row at a time through one queue

**Fixed** in `49886f3e` (I3). One write transaction per frame. A 10k load takes 716 ms against 1,815 ms.

- **Where.** `clients/ordo-sdk/src/replica/sync.ts:147-152` and `171-176`
  call `put` then `hold` per row. `replica.ts:85-97`, `148-173`, `250-263`.
  `backend.ts:96-121` opens one transaction per request.
- **What it costs per row.** About 5 HMACs to name keys (the entity prefix is
  recomputed for every row), 1 to 2 decrypts, 3 encrypts, and about 6
  IndexedDB transactions. Each step is its own `await`.
- **How it scales.** Linear, but with a large constant. Measured on the
  in-memory backend: 1,756 ms for 10k rows. Estimated 100k: about 18 s before
  IndexedDB latency, plus about 600,000 transactions.
- **Who waits.** Everything goes through one `#serial` queue
  (`replica.ts:273-277`). A screen's read waits behind every row still to be
  written. The thread stays free (longest stall 6.5 ms).
- **Fix.**
  - Apply a whole frame in one IndexedDB transaction (`putMany`).
  - Memoize names: the entity prefix per entity, and row names per frame.
  - Keep the holders index in memory, loaded once, written per frame.
  - Run HMAC and AES in a Worker for big frames, so the main thread only hands
    over bytes.
  - Let reads use their own read-only transactions, not the write queue. A
    read sees what is committed; the manager's position rule already keeps
    screens consistent.

## Medium

### M1. A device-copy screen shows nothing until its whole first read ends

**Fixed** in `49886f3e` (I3) and `ec803c49` (I4). The first fill is one batched read, then frames apply in memory.

- **Where.** `clients/ordo-react/src/device.ts:489-496`,
  `hooks/useDevice.ts:85-87` and `110`.
- **What waits.** The first answer is set only after every held row is read,
  one `get` at a time. `loading` stays true until then. A reload of the page
  with 10k rows on the device: 208 ms to scan and 762 ms through `refineOp` on
  the in-memory backend. A browser pays one IndexedDB transaction per row on
  top.
- **Fix.** Read in one transaction (H3's ranged `getAll`). Answer the first
  screenful first: `read(handle, { limit })`, then the rest.

### M2. Sign-out and revocation delete one record per transaction

**Fixed** in `75bdf0f8` (I2). Purge is one ranged delete in one transaction.

- **Where.** `clients/ordo-sdk/src/replica/replica.ts:121-126` (`purge`).
- **What waits.** A loop of `await backend.delete(key)`, after H3's full key
  scan. 100k synced rows means 300k transactions. Safety holds: `poison`
  latches before the purge (`replica.ts:137-140`). Sign-out waits.
- **Fix.** One `delete(IDBKeyRange.bound(ns + '/', ns + '/￿'))` in one
  transaction.

### M3. A live read does not fetch until the live stream opens, with no deadline

**Fixed** in `441a0262` (I8). A live read fetches at once and once more on open. A stream silent for `connectMs` (3 s) is retried.

- **Where.** `clients/ordo-react/src/hooks/useLive.ts:104-106` (`ready` is
  false while `connecting`), `hooks/useOpQuery.ts:133-143`, `live.ts:296-330`.
- **What waits.** The door subscribes before it reads, so a fetch after
  subscribing cannot miss a change. That is correct. The first fetch of a
  live read waits for the stream to say `ready`, then for the `POST` that adds
  the read: at least two round trips before the read itself.
- **The risk.** Nothing times out `connecting`. A stream whose response never
  arrives, or arrives without a `ready` frame, holds every live screen in
  `loading` forever. A failed connection is fine: it moves to `reconnecting`
  and the fetch goes ahead.
- **Fix.** Fetch at once, in parallel with the subscribe, and refetch once
  when the read opens if an invalidate could have been missed. Or give
  `connecting` a deadline (about 3 s), after which the read fetches anyway.

### M4. Paging copies every earlier row per page, and `exhaust` shows nothing until the end

**Fixed** in `8e83b2ce` (I9a). Paging appends into one array. `exhaust` shows rows every 10 pages, refuses to run beside another fetch, and takes a signal. The live reload still re-reads the pages on screen, now linearly: a first-page merge by id would miss rows that moved between pages.

- **Where.** `clients/ordo-react/src/pages.ts:50` (`[...state.rows,
  ...res.rows]`), `pages.ts:102-115` (`collectAll`, default 10,000 pages),
  `hooks/useOpPages.ts:207-228` (`exhaust`), `useOpPages.ts:176-198` (live
  reload).
- **How it scales.** O(pages × rows). At 50 rows a page, 100k rows is 2,000
  pages and about 100 million element copies. `exhaust` sets state once, after
  the last page. A live reload of a person scrolled K pages down refetches K
  pages one after another.
- **A correctness note.** `exhaust` sets `busy` without checking it
  (`useOpPages.ts:210`), so it can run at the same time as `more` or a reload.
- **Fix.** Accumulate into a mutable array and publish per page, or every few
  pages. Check `busy` in `exhaust`. Take an `AbortSignal`. For a live reload,
  refetch the first page and merge by id, or reload only what is on screen.

### M5. Sign-in calls have no deadline

**Fixed** in `78f8efe9` (I9b). Every sign-in call gives up after `callTimeoutMs` (15 s) as unavailable. The delegation poll stops at `expiresAt`.

- **Where.** `clients/ordo-sdk/src/auth/passkey.ts:240-244` (`call`, shared by
  passkey, email, guest, hand-off, recovery, claim and second step),
  `auth/access.ts:164`, `auth/connections.ts:201`, and the delegation poll
  `auth/delegation.ts:324-338`, which has no default timeout.
- **What waits.** The thread is free. A door that never answers leaves the
  promise pending, so a sign-in button the app disabled stays disabled.
- **Fix.** A default `AbortSignal.timeout(15_000)`, combined with the caller's
  signal, mapped to `PasskeyError('unavailable')`. Default the delegation
  poll's timeout to the delegation's `expiresAt`.

### M6. A synced op loads its whole result, at a pace nobody can set

**Fixed** in `da820db8` (I7) and `bd44360c` (I9d). `ORDO_SYNC_PAGE_ROWS` and `ORDO_SYNC_PAGE_WAIT_MS` are deployment settings, and `useOpQuery` takes `syncWhere`. The `settings.md` citations are corrected.

- **Where.** `crates/ordo-api/src/sync.rs:44-60` (`SyncPacing`, 50 ms between
  pages, "not measured yet"), the page size of 50 at
  `crates/ordo-host/src/runtime.rs:3713` and `:3903`, `docs/plan-replica-feed.md` §4.
- **What it means.** There is no cap (ruled 2026-09-15). A device-copy hook
  on an op with 100k rows makes the device load 2,000 frames. At 50 ms each
  that is at least 100 s of server waits alone, before H4's writes.
- **Not settable.** `docs/plan-replica-feed.md:63` says page size and the
  wait are per-deployment settings. Neither is: no env var, no settings row
  for `page_wait`. `docs/settings.md:56` and `:376` list page size 50 as
  hardcoded and cite `host/runtime.rs:2791`, which is stale; the literal is at
  `runtime.rs:3713` and `:3903`.
- **Also.** The hooks declare a sync with no refinement (`device.ts:456-463`),
  so the SDK's narrowed sync (R6, `manager.ts:163`) cannot be reached from
  React.
- **Fix.** Make the sync page size and `page_wait` deployment settings, as the
  plan says. Let `useOpQuery` pass a `syncRefine` so a screen can hold a slice
  of a large op. Fix the stale line in `docs/settings.md`.

## Low

### L1. A whole response body is parsed in one `JSON.parse`

**Bounded** in `da820db8` (I7). A sync frame is at most 1,000 rows (`ORDO_SYNC_PAGE_ROWS`).

`clients/ordo-react/src/client.ts:405-413`, `clients/ordo-sdk/src/core/client.ts:349-357`.
Linear in bytes, on the main thread. A page is 50 rows, so this is well under
a millisecond today. It becomes a finding if page size becomes a setting:
then cap it, or parse large bodies in a Worker.

### L2. Stream parsers concatenate and slice a string buffer

**Fixed** in `da820db8` (I7). Both parsers search only what arrived since their last look.

`clients/ordo-sdk/src/replica/sync.ts:205-221`, `clients/ordo-react/src/sse.ts:26-35`.
Fine for 50-row frames and tiny `invalidate` frames. Quadratic in one frame's
size if frames grow. Scan from the last position instead of from 0.

### L3. `rows()` spreads each page into `push`

**Fixed** in `78f8efe9` (I9b). A loop.

`clients/ordo-sdk/src/core/call.ts:171`. `out.push(...p.rows)` is fine at 50
rows. A page of more than about 100k rows would overflow the argument limit.
Use a loop.

### L4. Every live refetch replaces the rows array

**Fixed** in `8e83b2ce` (I9a). A refetch with the same rows, field for field, keeps the array.

`clients/ordo-react/src/hooks/useOpQuery.ts:157`. Every invalidate re-renders
the list even when nothing changed. Paced, so bounded. Keep the old array
when ids and revs are equal.

### L5. The generated spec table is built at import

**Fixed** in `116ccab2` (I9c). One `export const` per spec; `specList()` builds the list on first call.

`clients/ordo-sdk/src/generated/specs.ts:3083`, emitted by
`crates/ordo-gen/src/sdk.rs:450`. One literal of about 148 KB and
`Object.values(SPECS)` at load. A few milliseconds to parse, and it cannot be
tree-shaken per op. `generated/ops.ts` is types and one-line functions, and
costs nothing. Emit one `export const` per spec and build `SPEC_LIST` lazily.

### L6. Small per-call and per-render costs

**Fixed** in `3b4d2fa7` (I6) and `bd44360c` (I9d). Parameters are checked once per call, `useCommit` passes its form model, and `useDevice` builds its key only when an input is a new object.

All O(params), never O(rows):
- `checkParams` runs twice per call (`call.ts:63` and in `request`), and a
  `json` parameter is walked both times (`core/client.ts:375`).
- `deriveForm` runs on every commit (`ordo-react/src/commit.ts:127`), though
  `useCommit` already holds the model (`useCommit.ts:80`).
- `JSON.stringify` of params and refine on every render
  (`useOpQuery.ts:113`, `useLive.ts:69-72`, `useDevice.ts:64`).

### L7. Busy retries wait silently

**Fixed** in `bd44360c` (I9d). `onRetry` on fetch options, and `retrying` on `useOpQuery`.

`clients/ordo-react/src/busy.ts:131-172`, the same in
`clients/ordo-sdk/src/core/busy.ts`. Two retries of `server_busy`, up to about
24 s in the worst case. Capped, jittered and abortable. Nothing tells the
screen it is retrying; expose a `retrying` state so the app can say so.

---

## Checked and clean

- **No serial loading of independent ops in the library.** Each hook fetches
  on its own, and the live hub adds reads in parallel (`live.ts:317`). The only
  serial chains are dependent ones: a cursor needs the page before it, and
  WebAuthn allows one ceremony at a time (`auth/recovery.ts:115-116`).
- **`useCommit` returns when the server answers.** The device reload after a
  write runs in the background (`useCommit.ts:119`).
- **Sign-in.** WebCrypto is async (`auth/handoff.ts:64-70`). Base64url loops
  run over a few KB at most (`passkey.ts:187`, `205-215`). WebAuthn waits are
  the browser's own sheet. The hand-off stores 43 characters in
  `sessionStorage`. There is no global busy overlay.
- **Turnstile.** The clients load no Turnstile script. They carry the token
  only (`auth/methods.ts:37-87`). How the widget loads is the app's.
- **Live.** One shared stream per client. Frames carry a key, not data, and a
  pacer coalesces bursts (`live.ts:421-438`, `useLive.ts:87`).
- **The device answer is honest.** `useOpQuery` returns `complete: false`
  while a load is arriving, and rows as soon as any are held
  (`useDevice.ts:105-115`).
- **No `structuredClone` or deep copy** of rows anywhere in the clients.

## What needs the owner

- **M6, the settings.** The plan says sync page size and pacing are
  per-deployment settings. They are hardcoded. Whether to expose them, and to
  whom (app developer, deployment, tenant), is a ruling.
- **The guarantee in the guide.** `docs/howto-loading.md` says what never
  blocks today and what does until H1 to H4 are fixed. Once they are, the
  guide's "today" section shrinks to nothing.
