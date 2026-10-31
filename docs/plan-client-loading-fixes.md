# Plan: fixing what blocks a page in Ordo's clients

Written 2026-10-10 from the code at `a1b8e2e3`. Every increment, I0 to
I10, was built by 2026-10-11; each names its commit in section 5. The source is `docs/audit-client-blocking-2026-10-10.md` (17
findings) and its benchmark `clients/ordo-sdk/test/blocking.bench.test.ts`.
The rule it serves is the owner's of 2026-10-10 (board `staged-load`):
staged loading is the app developer's design, and Ordo never blocks the page
by default.

## On one screen

**What is wrong.**

- A device-copy screen reads every row again after every 50-row frame (H1).
  3,000 rows settle in 7.2 s against 0.6 s to load. It grows with the square
  of the rows.
- Local sort and filter run as one synchronous task (H2): 761 ms to sort
  100k rows by a money field.
- Each IndexedDB key scan reads every key on the device, then sorts in
  JavaScript (H3): 285 ms at 100k rows.
- The device store writes one row at a time, about 6 transactions a row,
  and reads wait behind the whole load (H4).
- A live read waits for the live stream to open, with no deadline (M3).

**What we will do.**

- A screen keeps its rows in memory and applies each frame's changes to
  them. It never re-reads the store during a load.
- One read in flight per screen, and a screen ignores notices about other
  ops.
- Sorting parses each money value once, uses one comparator, and above a
  size yields to the page between chunks.
- Key scans ask IndexedDB for a key range. A frame is written in one
  transaction. Reads stop queueing behind writes.
- A live read fetches at once and refetches once when its stream opens.
- The sync page size and the wait between pages become deployment settings,
  with today's 50 rows and 50 ms as defaults.

**In what order.** Measure first (I0). Then a quick win in React alone (I1).
Then the store (I2, I3), the in-memory rows (I4) and the faster sort (I5).
Then React moves onto them (I6). The settings (I7) and the live read (I8)
run in parallel with all of it. Section 5 has the list.

**How we will know.** Each fix lands with a budget test that fails today.

| Measure | Today | Target |
|---|---|---|
| Settle with one screen mounted, 3k rows | 7,249 ms, 12x the load | within 1.5x of the load |
| Same at 10k and 100k rows | about 80 s; hours | within 1.5x of the load |
| Store row reads per load with one screen mounted | about n²/100 | at most n, plus the first read |
| Sort 100k rows by money, pure | 761 ms | at most 150 ms |
| Longest stall during a device refine of 100k rows | 761 ms | at most 50 ms |
| Key scan's synchronous work at 300k keys | 285 ms | no filter and no sort in JavaScript |
| Write transactions per 50-row frame | about 300 | 1 |
| Sync load of 10k rows, in memory | 1,756 ms | at most 900 ms |
| First fetch of a live read | after the stream opens | at once |

---

## 1 · H1 in depth: a screen re-reads every row on every frame

### 1.1 What happens today, step by step

1. The app mounts `useOpQuery` on a syncable op. `useDevice`
   (`clients/ordo-react/src/hooks/useDevice.ts:75`) declares it on the device
   copy. `deviceCopy.declare` (`clients/ordo-react/src/device.ts:456-464`)
   calls `SyncManager.sync`, which opens the op's feed
   (`clients/ordo-sdk/src/replica/manager.ts:163-186`).
2. `useDevice` calls `refresh()` once, then watches the device copy for any
   change (`useDevice.ts:83-90`).
3. The feed sends a `load` frame of 50 rows. `syncOp` writes each row with
   `replica.put` and `replica.hold`, one awaited call after another
   (`clients/ordo-sdk/src/replica/sync.ts:147-152`). Each call goes through
   the replica's one queue (`replica.ts:273-277`).
4. After the frame, `syncOp` calls `onFrame` (`sync.ts:181`). The manager
   stores the state and publishes a view to every watcher
   (`manager.ts:296-300`, `330-333`).
5. The device copy's `onView` (`device.ts:420-423`) calls `notify`, which
   calls every listener (`device.ts:396-398`). Every mounted device hook is a
   listener, whatever op it shows. A frame for op A wakes the screen for op B.
6. Each listener runs `refresh()`. It bumps `latest` and calls `read`
   (`useDevice.ts:83-88`). Nothing stops a second read starting while the
   first runs; `latest` only throws the older answer away at the end.
7. `read` (`device.ts:478-502`) lists every row the op holds
   (`replica.holdings`, a full key scan under H3, then one `get` and one
   decrypt per hold record), sorts the ids, then calls `replica.get` once per
   row. Each call is one queued step and, on IndexedDB, one transaction.
8. When the op states an order, `read` calls `refineRows` over every row
   (`device.ts:497-501`). That is one synchronous sort (H2).
9. `setAnswer` stores a new array, so React renders the whole list again
   (`useDevice.ts:87`).

**Why it is quadratic.** A load of n rows is n/50 frames. Each frame starts a
read of every row held so far. That is about n²/100 row reads per screen,
each with a decrypt. The reads share the queue with the load, so the load
slows too.

**Found while planning: a read can see part of a frame.** `read`'s `get`
calls and the load's `put` calls take turns in the queue. A read that is
under way when the next frame arrives sees some of that frame's rows and not
others. The manager's promise that a screen never sees half a frame
(`manager.ts:11-13`) holds for publication only. The fix below closes it.

### 1.2 The design of the fix

Four parts. The first ships alone as a quick win (I1). The rest replace it.

**Part 1. One read in flight, and only for this screen's op.**

- Each hook keeps `busy` and `dirty`, as `useOpPages` already does
  (`useOpPages.ts:113-117`). A notice during a read sets `dirty`. When the
  read ends and `dirty` is set, one more read runs. At most one read runs and
  one waits.
- The device copy remembers each handle's last state: `loaded`, `complete`,
  `position`, `attempts` and `stopped`. A notice wakes a handle only when its
  own state changed, or when the copy itself changed (poisoned, revoked,
  manager replaced).
- Coalescing alone turns the load linear in reads: a read takes time
  proportional to n, frames keep arriving, and the number of reads stays
  about constant. It does not remove the queue contention. Parts 2 to 4 do.

**Part 2. The SDK hands over what each frame changed.**

- `syncOp`'s `onFrame` gains a second argument, the frame's delta:
  - `upserts`: each row the frame put, as the replica now holds it. When the
    replica already held a newer version (C3 refused the put), the delta
    carries the newer version, so memory matches the store.
  - `releases`: each `(entity, id)` this holder let go, and whether the row
    left the replica (no other holder) or stays.
  - `reset`: true on the first frame of a new load after a reconnect.
- The replica's frame write (H4, section 2.4) returns this delta, so the SDK
  computes it once with no extra reads.

**Part 3. The manager keeps rows in memory for screens that watch them.**

- A new `SyncManager.observe(key, listener)` returns a snapshot and calls
  the listener when it changes. Only handles with an observer keep rows in
  memory. A handle nobody watches costs nothing.
- One shared row map per manager: `(entity, id)` to `{ position, row }`. Per
  observed handle, a membership set of row refs.
- On the first `observe` of a handle, one bulk read fills it: the handle's
  holds and their rows in one read-only transaction (H3's ranged `getAll`).
  Deltas that arrive during that read are buffered and applied after it.
- Each delta is applied in the `onFrame` callback, synchronously, before the
  manager publishes the view:
  - An upsert replaces a row only when its position is equal or higher (C3's
    rule, the same comparison as `replica.ts:253`).
  - A release removes the ref from that handle's set. The row leaves the
    shared map when no observed handle holds it.
  - An upsert of a row another observed handle holds updates that handle too.
    Its listener is called.
- Only handles whose rows or state changed are told. That is the filter of
  Part 1, made exact.

**Part 4. Each snapshot is ordered and refined incrementally.**

- A snapshot is `{ rows, state }`, built together from one frame. The rows
  and the `complete` flag can no longer disagree, as they can today, where
  `state` is read after the rows (`useDevice.ts:87`).
- The ordered array is kept between frames. A delta of k rows is sorted
  (k log k), then merged into the kept array in one pass (n + k). Removed
  rows drop out in the same pass. Sort keys are decorated once per row (H2).
- `where` runs on the delta's rows only. A change of query recomputes in
  full, chunked so no step passes 50 ms (H2).
- `pick` results are cached per source row object (a `WeakMap`), so a row
  that did not change keeps its identity.
- A new array is published only when something changed. Unchanged rows keep
  their object identity, so `React.memo` rows skip rendering.
- **Publishing is paced during a load.** At most one snapshot per handle per
  `publishMs`, a client option on `deviceCopy`, default 100 ms. The first
  rows publish at once. The frame that completes a load, a change frame
  after the load, and a revocation publish at once. This bounds the copies
  of a 100k-row array to a few hundred over a whole load.

### 1.3 How it meets the device copy's rules

**C5, the common position.** The manager's rule: a frame's rows all reach
the replica before its position is published, and the view's position never
goes back (`manager.ts:9-20`).

- Per hook the fix makes this exact. A snapshot moves from one whole frame
  to the next. A read can no longer see part of a frame, because no read
  happens during a load.
- Across hooks, every observer touched by one frame is updated in the same
  call, before `notify`. With `useSyncExternalStore` (Part 5 below) React
  renders all of them from that one version. Two screens never show two
  different frames of one op.
- Two hooks on two ops can show two positions, as they can today. The
  adapter does not gate on the view's position, and the manager's position
  rises only when an op receives a frame. Strict gating would hold a screen
  forever behind an op whose result did not change. Question 1 asks the
  owner about this.

**C6, local refinement.** `refineRows` stays the one evaluation, kept
identical to the door by `refine-parity.test.ts`. The incremental path calls
the same comparator and filter. A parity test runs every case in
`crates/ordo-conformance/fixtures/refine-parity.json` both ways, full and
incremental, and expects the same rows in the same order.

**C7, purge and poisoning.**

- `poison()` clears the shared map and every membership set synchronously,
  before its purge starts, and publishes empty snapshots. Memory never
  outlives the latch.
- A revoking stop on a handle empties that handle's snapshot at once, as
  `read` does today (`device.ts:482-483`). Its releases then arrive as
  deltas and change nothing more.
- A replaced manager (`device.ts:431-449`, a new sign-in) drops every
  in-memory set. The new manager's observers fill from the store.
- Sign-out purge goes through the same clear.

**The `complete` flag.** Taken from the same frame as the rows. A reconnect
sets `complete: false` with the rows still shown (`manager.ts:278`), and a
reset delta keeps them until the new load completes. Rows the completed load
left out arrive as releases in the completing frame (`sync.ts:153-164`).

**`deviceFallback`.** Unchanged. `planDevice` decides from the descriptor
before any of this runs (`device.ts:100-137`). A hook on the network path
never observes.

**Sensitive lists.** Never declared, so never observed and never in memory.
`planDevice` and `syncOp` both refuse `std.space.*` and access maps first.

**React.**

- `useDevice` reads the snapshot through `useSyncExternalStore`. React 18 is
  the floor already (`clients/ordo-react/package.json`). The store's
  `getSnapshot` returns the same object until the next publish, so React
  sees no change between frames and renders all hooks from one version.
- State updates from one frame land in one render: React 18 batches updates
  made in one task, and the adapter makes them in one task.
- `useDevice`'s per-render `JSON.stringify` key (L6) becomes a memo keyed by
  the inputs' identities, with content compared only when they change.

**A side note.** Two ops over one entity that project different fields share
one row record (`replica.ts:269-271`, keyed by entity and id). The memory
layer copies that rule exactly, so it neither fixes nor worsens it. Today
only `std.E.list` and `std.E.get` are syncable, and they project the same
fields.

### 1.4 Invariants, and the tests that prove them

| Invariant | Test (new unless named) |
|---|---|
| No row moves to a lower position, in memory as on disk | `observe: a lower-position upsert leaves the snapshot alone` (`ordo-sdk/test/manager.test.ts`) |
| A snapshot never holds part of a frame | `observe: every snapshot is a whole number of frames`, frames of distinct marker rows |
| A frame for another op does not wake a screen | `device: a frame for op A runs no read for op B` (`ordo-react/test/device.test.ts`), counting `replica.get` |
| A row shared by two ops updates both screens | `observe: an upsert through op A reaches op B's snapshot` |
| Rows and `complete` come from one frame | `device: complete is never true with a partial row set` |
| Revocation empties the screen at once and memory holds nothing | the two `revocation` tests in `device.test.ts`, plus `poison clears every observer before the purge starts` |
| A replaced manager starts from the store | `device: a new sign-in drops every in-memory row` |
| Incremental refine equals full refine | `refine-parity.test.ts`, extended to run each case incrementally |
| Unchanged rows keep identity | `device: a change to one row keeps every other row object` |
| Store reads per load are linear | `device: a load of 2,000 rows with a screen mounted reads each row at most once` (CI, counting backend) |
| Settle within 1.5x of load | `device-load.bench.test.ts` at 3k, 10k, 100k, `ORDO_BENCH=1` |

**The benchmark.** A new `clients/ordo-react/test/device-load.bench.test.ts`
mounts a real `deviceCopy` and `useOpQuery` over the SDK, as
`device.test.ts` does. It times the load alone, then the load with one
screen mounted, until the screen's last snapshot shows every row with
`complete: true`. The budget is 1.5x at 3k, 10k and 100k rows. Today it
fails at 3k (about 12x). It also runs with two screens on one op and one
screen on each of two ops. The default run uses 200 rows and checks the
read count, so CI keeps it compiled and correct.

---

## 2 · H2 to H4 and M3

### 2.1 H2. Local sort and filter as one synchronous task

**The fix.**

- Decorate once. Parse each money or decimal value to a scaled `BigInt`
  once per row and compare the cached values. Today `compareDecimal`
  (`clients/ordo-sdk/src/core/client.ts:510-522`) splits and builds two
  `BigInt`s inside every comparison.
- One composite comparator in one stable sort, in place of one sort per key
  with a wrap and unwrap each time (`clients/ordo-sdk/src/replica/refine.ts:239-255`).
- Skip the sort when the rows already arrive in the stated order. One linear
  check decides.
- `refineRowsAsync`: the same answer, with no step over 50 ms. It filters in
  chunks of a few thousand rows and yields between them, then sorts chunks
  and merges them in yielding slices. Yield with `scheduler.yield()` where
  the browser has it, else `setTimeout(0)`. Used above a row count, a
  `deviceCopy` option `refineChunk` with default 5,000.
- A merge helper for the incremental path of Part 4: sorted array plus
  sorted delta, one pass.

**Why chunks and not a Worker.** A Worker needs its own script URL, which
every app's bundler would have to handle. Sending 100k rows to it is a
structured clone, which is itself a long task on the page. Chunks need
nothing from the app.

**Files.** `clients/ordo-sdk/src/replica/refine.ts`, `core/client.ts`
(`compareDecimal` stays for single comparisons), `core/refine.ts`,
`src/index.ts` exports.

**Tests.** `refine-parity.test.ts` runs every case through `refineRows`,
`refineRowsAsync` and the merge helper. New cases: negative decimals, mixed
scales (`1.5` against `1.50`), nulls in a composite sort, a presorted input.

**Benchmark targets** (`blocking.bench.test.ts`, 100k rows): sort by money at
most 150 ms (761 today), by text at most 100 ms (180 today), and a longest
stall during `refineRowsAsync` of at most 50 ms.

**Risks.** Parity: a decorated key must compare exactly as `compareDecimal`
does. The parity suite and the 27ak money tests guard it. A chunked refine
can be overtaken by a newer query; it takes an `AbortSignal` and the caller
drops stale answers.

### 2.2 H3. Every key scan reads every key on the device

**The fix.**

- `IndexedDbBackend.keys(prefix)` asks for
  `IDBKeyRange.bound(prefix, prefix + '￿')`. Keys are hex and `/`, so
  the range is exact. IndexedDB returns them sorted, so the JavaScript sort
  goes (`clients/ordo-sdk/src/replica/backend.ts:123-126`).
- A new `Backend.entries(prefix)`: keys and values over the same range in one
  read-only transaction (`getAllKeys` and `getAll` together). `holdings` and
  the first fill of an observed handle use it.
- `MemoryBackend` gets the same method. Its `keys` stays a filter, since it
  is for tests.
- M2 rides here: `purge` deletes the namespace's range in one transaction
  (`replica.ts:121-126`).

**Files.** `backend.ts`, `replica.ts` (`scan`, `holdings`, `purge`).

**Tests.** In `replica.test.ts`, on `fake-indexeddb`: two replicas on one
database, and `keys` of one returns only its own, sorted. `purge` of one
leaves the other whole. A counting backend shows `holdings` of n rows is one
transaction.

**Benchmark target.** The key scan's synchronous half has no filter and no
sort. The bench's `keys()` case is rewritten to time the ranged call on
`fake-indexeddb` with 300k keys of other replicas present, and asserts that
time does not grow with them.

**Risks.** A key outside the hex alphabet would fall outside the range. The
names come from `nameOf` (hex) and the namespace from `digestOf` (hex); a
test pins the alphabet.

### 2.3 H4. The device store writes one row at a time

**The fix.**

- `LocalReplica.applyFrame(holder, entity, rows, releases, position)`: one
  call per frame from `syncOp`, in place of `put` and `hold` per row
  (`sync.ts:147-152`, `171-176`).
- Names memoized: the entity prefix per entity and the hold prefix per
  holder, kept for the replica's life. Per row, three HMACs remain (row key,
  holders key, hold key), down from about five.
- An in-memory index of each row's position and holders, filled lazily: from
  what this session wrote, and on a miss from one batched read of the
  frame's missing keys. That removes the decrypts of the held row and of its
  holders list from each write.
- The frame's writes go in one read-write transaction. **The constraint:** an
  IndexedDB transaction commits as soon as it has no pending request, so it
  cannot wait on WebCrypto. The frame is therefore built in steps: name the
  keys, read what is missing in one read-only transaction, decrypt, decide
  and encrypt, then write everything in one read-write transaction.
- Writes stay on the one queue, so frames never interleave. Reads leave the
  queue: each read is its own read-only transaction and sees whole frames
  only, because each frame is one transaction.
- Not in this step: moving HMAC and AES to a Worker. The thread already stays
  free (longest stall 6.5 ms). Filed as a follow-up if a browser profile
  shows otherwise.

**Files.** `replica.ts`, `sync.ts`, `backend.ts` (`putMany`, `getMany`),
`seal.ts` if a batch helper is needed.

**Tests.** A counting backend: one write transaction per frame, plus one for
the resume hash on the completing frame. The C3 tests in
`replica.test.ts` pass unchanged. New: `a read issued mid-load answers
before the load ends`, and `a lower-position frame does not overwrite a row
in one batch`. `manager.test.ts` and `sync.test.ts` pass unchanged.

**Benchmark targets.** 10k-row load on the in-memory backend at most 900 ms
(1,756 today). 100k at most 9 s (about 18 s estimated today). A read issued
in the middle of a 10k load resolves within 100 ms.

**Risks.** A crash between the read step and the write step loses that frame
only. The next connection reloads, as a reconnect does today. The memory
index must be dropped on `poison` and `purge`, or a later write could use a
stale position; a test covers it.

### 2.4 M3. A live read waits for the stream, with no deadline

**The fix.**

- The first fetch of a live read starts at once, beside the subscribe
  (`clients/ordo-react/src/hooks/useOpQuery.ts:133-143`, `useLive.ts:106`).
- When the read opens on the stream, the hook refetches once if its first
  fetch finished before the open. A change made between the two would
  otherwise be missed. This keeps the guarantee that subscribe-before-read
  gave.
- A deadline on `connecting`: after `connectMs` with no `ready`, the stream is
  treated as failed and moves to `reconnecting` (`live.ts:296-330`). A
  provider option `live={{ connectMs }}`, default 3,000 ms.

**Files.** `useLive.ts`, `useOpQuery.ts`, `live.ts`, `useOpPages.ts` (it
waits the same way).

**Tests.** In `live.test.ts`: `a live read fetches before its stream is
ready`; `a stream that never says ready still shows rows, then reconnects
after the deadline`; `a change between the first fetch and the open is
shown`.

**Target.** The first rows of a live screen arrive after one round trip,
whatever the stream does.

**Risks.** One extra fetch per live screen on first open when the fetch wins
the race. Bounded by one, and the pacer still coalesces later invalidates.

---

## 3 · The two settings

`docs/plan-replica-feed.md` §4 says page size and the wait are
per-deployment settings. Today neither is: the wait is `SyncPacing::default`
(`crates/ordo-api/src/sync.rs:52-60`) and the page size is the executor's
`page_size: 50` (`crates/ordo-host/src/runtime.rs:3713` and `:3903`).

| Setting | What it does | Default | Bounds | Who sets it |
|---|---|---|---|---|
| `ORDO_SYNC_PAGE_ROWS` | Rows in each `load` frame of a sync feed, and in each page of its difference pass | 50 | 1 to 1,000 | Deployment |
| `ORDO_SYNC_PAGE_WAIT_MS` | The wait between one sync page read and the next | 50 | 0 to 10,000 | Deployment |

- Both are read once in `RuntimeConfig::from_env`, the binary's one
  environment reader. A value outside its bounds, or not a number, refuses
  the boot and names the setting, its bounds and its default, as
  `ORDO_REFINE_MAX_IN_VALUES` does.
- **The sync page size is its own setting.** The read page size stays
  interactive and stays 50. A sync is a background load, and a deployment
  may want larger frames there without changing what a screen pages. The
  sync's reader passes its page size to the executor; today it inherits
  `FixtureExecutor.page_size` (`crates/ordo-host/src/lib.rs:825`, `:1055`).
- **Why 1,000 rows at most.** Each frame is one `JSON.parse` and one stream
  buffer on the page (L1, L2). At 1,000 rows a frame stays well under a
  long task. The bound can rise once L1 and L2 are done.
- **0 ms is allowed.** It is the app developer's choice to load as fast
  as the database answers. The default keeps the database free for everyone
  else most of each second.
- `SyncPacing` gains `page_rows`. `ApiState.sync` carries both
  (`crates/ordo-api/src/router.rs:109`).

**`docs/settings.md` corrections.**

- Line 56 and line 376 cite `host/runtime.rs:2791` for the read page size.
  The literal is at `host/runtime.rs:3713` and `:3903`. Both rows are
  corrected.
- Two new rows in section 11, beside `ORDO_LIVE_IDLE_CLOSE_SECS`, for the
  two settings above: built, Deployment `sets`, App developer `could be
  exposed` (the `settings { }` block, once it is parsed).
- `docs/howto-loading.md` §7 says who sets them.

**Tests.** `crates/ordo-host` config tests: each setting parsed, each bound
refused with its name. `crates/ordo-api` sync test: a feed with
`page_rows = 7` sends frames of 7, and `page_wait = 0` sends with no sleep.

---

## 4 · The medium and low findings

**Ride with the high fixes.**

| Finding | One-line fix | Rides with |
|---|---|---|
| M1. A screen shows nothing until its first read ends | The first fill is one read-only transaction (`entries`), then deltas | I3, I4 |
| M2. Purge deletes one record per transaction | One ranged delete in one transaction | I2 |
| M6. Pacing nobody can set | The two settings of section 3 | I7 |
| L1. A whole body is one `JSON.parse` | Bounded by `ORDO_SYNC_PAGE_ROWS` at most 1,000 | I7 |
| L2. Stream parsers rescan the buffer from 0 | Scan from the last position checked (`sync.ts:205-221`, `sse.ts:26-35`) | I7 |
| L6. `JSON.stringify` per render in `useDevice` | Memo by identity, compare content on change | I6 |

**Follow after.**

| Finding | One-line fix |
|---|---|
| M4. Paging copies every earlier row per page; `exhaust` shows nothing until the end | Accumulate into one array and publish every few pages; `exhaust` checks `busy` and takes an `AbortSignal`; a live reload refetches page 1 and merges by id |
| M5. Sign-in calls have no deadline | A default `AbortSignal.timeout`, a client option `callTimeoutMs` with default 15,000 ms, mapped to `unavailable`; the delegation poll defaults to the delegation's `expiresAt` |
| M6, second half. React cannot narrow a sync | `useOpQuery` takes `syncWhere`, passed to `manager.sync` as its `refine` (`manager.ts:163`) |
| L3. `push(...p.rows)` | A loop (`core/call.ts:171`) |
| L4. Every live refetch replaces the array | Keep the old array when ids and revs are equal (`useOpQuery.ts:157`) |
| L5. The spec table is built at import | One `export const` per spec, `SPEC_LIST` built on first use (`crates/ordo-gen/src/sdk.rs:450`) |
| L6, the rest | `checkParams` once per call; `deriveForm` from the model `useCommit` holds |
| L7. Busy retries are silent | A `retrying` state on the hooks |

---

## 5 · Increments

Each is one agent's work. "Parallel" means it touches different files from
its neighbours and needs nothing they produce.

**I0 · The budgets.** Add `device-load.bench.test.ts` (ordo-react) and the
new cases in `blocking.bench.test.ts` (ordo-sdk), with the targets of the
summary table as assertions under `ORDO_BENCH=1`. Add the CI counters: store
reads per load, write transactions per frame. *Tests:* the files themselves.
*Done when* every budget fails today with a message naming its target, and
the default run passes in under 5 s. Small.

*Built, `015169fb`.* Before: 3k rows with one screen 12.7x the load;
10k and 100k not settled by 1.5x; 150 write transactions per frame;
10k load 1,815 ms.

**I1 · Coalesce and filter in React.** After I0. `device.ts` remembers each
handle's last state and wakes a handle only on its own change. `useDevice`
keeps `busy` and `dirty`. *Tests:* `a frame for op A runs no read for op B`;
`at most one read in flight per hook`; the existing `device.test.ts` and
`device-gaps.test.ts`. *Done when* the 3k bench drops from 12x to under 3x.
Small. Ships alone, so screens get better before the rest lands.

*Built, `df8cfcea`.* 3k bench 12.7x to 2.3x. A notice also wakes a
handle over the same entity, since another op's frame may write a row it
shows. Tests in `ordo-react/test/device.test.ts`, "a screen reads only when
its own op moves (H1)".

**I2 · Ranged keys, `entries`, ranged purge (H3, M2).** Parallel with I1, I5,
I7, I8. *Tests:* section 2.2. *Done when* the key scan does no JavaScript
filter or sort and purge is one transaction. Small.

*Built, `75bdf0f8`.* A 3,000-key scan beside 300k other keys: 506 ms
unranged, 5.8 ms ranged. `IndexedDbBackend.open` takes the runtime's
`IDBKeyRange`; with none it falls back to a filtered scan.

**I3 · One transaction per frame (H4).** After I2. `applyFrame`, memoized
names, the position and holders index, reads off the queue, the frame delta
returned. *Tests:* section 2.3. *Done when* a frame is one write transaction
and the 10k load meets 900 ms. Large; the transaction constraint needs care.

*Built, `49886f3e`.* 10k load 1,815 ms to 716 ms; 1 write transaction per
frame, the resume hash included. A read of 1,000 rows mid-load through
`held()` answers in 49 ms. Not built: the in-memory position and holders
index. Each frame reads its rows' records in one batched read instead,
which stays correct when two tabs write one store.

**I4 · Rows in memory per observed handle.** After I3. `syncOp` passes the
delta to `onFrame`; `SyncManager.observe`; the shared row map; buffering
during the first fill; the paced publish; the poison clear. *Tests:* the SDK
rows of section 1.4. *Done when* a load with an observer reads each row from
the store at most once. Large.

*Built, `ec803c49`.* `SyncManager.observe`; `publishMs` is a manager
option. A load with an observer settles in 1.01x of the load at 10k rows
and 1.04x at 100k, with no extra store reads. React still reads per row
until I6.

**I5 · The faster refine (H2).** Parallel with I2 to I4. Decorated keys, one
comparator, the presorted check, `refineRowsAsync`, the merge helper.
*Tests:* section 2.1. *Done when* the 100k money sort meets 150 ms and the
async path never stalls over 50 ms. Medium.

*Built, `38188c63`.* 100k rows by money 761 ms to 58 ms, by text 180 ms
to 83 ms. `refineRowsAsync` (slices of `DEFAULT_REFINE_CHUNK`, 5,000)
stalls at most 4.5 ms at 100k rows. Later keys are read only for rows whose
earlier keys tie. `refineMerge` is the merge helper.

**I6 · React on the snapshots.** After I4 and I5. The adapter uses
`observe` when the manager has it and keeps I1's path when it does not.
`useDevice` moves to `useSyncExternalStore`, with the pick cache and stable
rows. *Tests:* the React rows of section 1.4, and every existing device
test unchanged. *Done when* the H1 bench meets 1.5x at 3k, 10k and 100k.
Medium.

*Built, `3b4d2fa7`.* One screen 0.9x at 3k, 1.1x at 10k, 1.1x at 100k,
with 0 store row reads; two screens on one op 1.0x; a screen on each of two
ops 1.0x. `deviceCopy` takes `refineMerge`, `refineRowsAsync` and
`refineChunk`; snapshots carry `version` and `changes`.

**I7 · The two settings (M6, L1, L2).** Parallel with everything. Rust:
`SyncPacing`, `RuntimeConfig`, the sync reader's page size. Docs:
`settings.md`, `howto-loading.md`. TypeScript: the two stream parsers.
*Tests:* section 3. *Done when* both settings change a running feed and a
bad value refuses the boot by name. Small.

*Built, `da820db8`.* `SyncPacing.page_rows` reaches the executor through
`OpExecutor::execute_sized`; the read page stays 50.

**I8 · The live read fetches at once (M3).** Parallel with everything.
*Tests:* section 2.4. *Done when* a live screen shows rows with a stream that
never opens. Small.

*Built, `441a0262`.* `connectMs`, default `DEFAULT_LIVE_CONNECT_MS`
(3,000), on `LiveHub` and the provider's `live`.

**I9 · The follow-ups.** After I6. The "follow after" table of section 4,
split in two: M4 with L4 (paging and refetch), then M5, L3, L5, L6 and L7.
Small each.

*Built:* `8e83b2ce` (M4, L4), `78f8efe9` (M5, L3), `116ccab2` (L5),
`bd44360c` (L6, L7, and M6's `syncWhere`). The live reload of `useOpPages`
re-reads the pages on screen linearly instead of merging page 1 by id,
since rows can move between pages. `SPEC_LIST` became `specList()`.

**I10 · Close the record.** After I6 and I8. `docs/howto-loading.md`'s
"What can still block today" shrinks to what remains. The audit gets a line
per finding naming its fix commit. The board's `device-reread-quadratic`
(25bh) and `staged-load` get `work` and `where`, and `node
scripts/board-agrees.mjs` passes. Small.

*Built,* this commit: `howto-loading.md`, the audit's per-finding lines,
and these notes. The board's `work` and `where` are left to the session
that owns the board.

**Running in parallel.** Three tracks at once:

- Store: I2, then I3, then I4.
- Sort: I5.
- Edges: I1, then I7 and I8 (each small, in any order).

I6 joins the store and sort tracks. Opus for I3, I4 and I6. Sonnet for the
rest. A Fable audit after I6, per the batch rule.

---

## 6 · What must not break

These are the device copy's ruled properties. Every increment keeps their
tests green, and none is weakened to meet a budget.

- **No row moves to a lower position** (C3, `replica.ts:250-257`). Holds on
  disk in `applyFrame` and in memory in the shared map. Guards: the C3 tests
  in `replica.test.ts`, the reconnect tests in `manager.test.ts`, and the new
  lower-position test of section 1.4.
- **Revocation purges** (C7). A revoking refusal releases what the op held; a
  poisoned replica serves nothing and stores nothing; memory is cleared
  before the purge starts. Guards: the `revocation` tests in
  `device.test.ts`, `replica.test.ts`'s poison tests, and the new
  poison-clears-observers test.
- **Screens never mix positions** (C5). Per hook, a snapshot is a whole
  number of frames. Across hooks, one frame updates every observer in one
  call and one render. Question 1 covers the case across two ops.
- **Sensitive lists are never mirrored** (§6). `std.space.*` and access maps
  are refused before any declaration, so they never reach the store or
  memory. Guards: `never std.space.*` and the access-map tests in
  `device.test.ts`, and `syncOp`'s refusal in `sync.test.ts`.
- **The sealed store stays sealed.** Batching changes how records are
  written, never what is written. Guard: the raw-dump test in
  `replica.test.ts` (no plaintext row, id, entity, space or principal).
- **Honest progress.** `complete: false` until the load ends, with rows
  shown as they arrive. Guard: `an empty result is empty only once the load
  is complete`, plus the new rows-and-complete test.

---

## 7 · Open questions for the owner

The ruling settles the direction. One choice remains.

**Q1. Should two screens on two ops wait for each other?**

- Today, and after this plan, a screen moves when its own op's frame lands.
  Two screens showing two ops can briefly show two positions.
- Holding both until both reach a position needs the door to tell an op it
  is current when a change leaves its result unchanged. Today it sends
  nothing then, so a strict gate would freeze a screen.
- *Recommendation:* **keep per-op frames now, and file the strict gate for
  later.** Each screen stays exact and nothing is held back, which is what
  the ruling asks. The strict gate needs a server frame and has no reported
  case yet.
