# Plan: a ready-made device, dropping unused syncs, one stream per tab

**Ruled 2026-10-21 (`ease-what-the-sdk-asks-of`, ask 2): plan now, build as part
of notes v2.** Three SDK improvements, each taking away work the SDK leaves to
the app developer today:

- **5**: a ready-made device.
- **6**: dropping syncs nobody uses.
- **7**: one stream per tab.

Nothing here changes what a device may hold. The rules in `plan-replica-feed.md`
and `plan-client-cache.md` stay: op-scoped sync; a row leaves only when no
holder holds it; resume on one result hash (C4d); `members` on the completing
load.

Board task: `plan-a-ready-made-device-dropping-unused` (7 asks).

---

## 0 · Where things stand

- **The device is app code.** `examples/notes/web/src/offline.ts` holds a
  `Device` class of about 100 lines. It opens the IndexedDB backend under
  `'ordo-notes'`, opens `LocalReplica`, starts a `SyncManager`, hands
  `deviceCopy` a function that returns the current manager, restarts by
  starting a new manager before stopping the old one (`#restart`), and purges
  and closes.
- **Devices are tracked by hand.** `App.tsx` keeps a `devices` ref and a
  `purgeAll`. `SharedView.tsx` opens one `Device` per shared space.
- **The app polls.** `keepFresh` reloads on focus, on `online` and every 30 s.
- **The swap is the defect.** Starting the new manager before the old one
  stops let a stale row stay in the live `notesTagged` list. `f504c26f`
  (`SyncDelta.members`) repairs it on the SDK side; the swap is still there.
- **The polling's reason may be out of date.** `offline.ts` says a tenant made
  after boot is not pushed to. The host's bus has listened per database on
  demand since 25g (`crates/ordo-host/src/invalidation.rs`, `subscribe_in`).
  Not re-checked since.
- **Declarations are never given back.** `deviceCopy`
  (`clients/ordo-react/src/device.ts`) keeps a `declared` map for life, and
  `rebind()` declares every entry again on each new manager. `useDevice` has
  nothing to call on unmount. Every tag a person clicks is a feed that stays
  open, is reopened on every `keepFresh` tick, and keeps its rows on disk until
  sign-out.
- **Each synced op holds one HTTP connection.** `OrdoClient.openFeed` opens
  `GET /op/{name}?live=1&sync=1` per op and parameter set
  (`crates/ordo-api/src/sync.rs`). The shared `/live` stream
  (`crates/ordo-api/src/live.rs`) carries invalidations only.
- **One notes tab holds:** a feed for `myNotes`, one per tag clicked, one per
  shared space on screen, and `/live` while a screen follows a network read.
  Two tabs reach the browser's 6 connections per host over HTTP/1.1. The door
  serves plain HTTP behind the deployment owner's proxy (`docs/deploying.md`),
  so HTTP/2 depends on that proxy.
- **Every tab repeats the work.** Each tab runs its own manager and feeds, so
  the server re-reads each result once per tab on every change.

### The 4 small items this plan builds on

Being built now; this plan assumes they land first.

| Item | Used here for |
|---|---|
| `navigator.locks` around replica writes (`ordo-replica:<ns>`) | Keeps two writers safe while 5 and 6 land. In 7 only the leader writes frames; the lock stays for purge and for tabs that lead themselves. |
| Reconnect on `online` and `nudge()` | `nudge()` cuts a backoff short. 5 adds `reconnect()`, which reopens healthy feeds too. `openDevice` wires `online` to `nudge()`. |
| Typed `OrdoNetworkError` | The manager counts it as retryable, never as a refusal. A device op can report `offline` plainly. |
| ordo-react device registry keyed by space | The registry owns lifetimes; `openDevice` is the factory it calls. `App.tsx`'s `devices` ref becomes `registry.purgeAll()`. |

---

## 5 · A ready-made device

### Today

The app developer copies `offline.ts`: a backend and a replica, a manager,
`deviceCopy` with a manager function, three refiners and a `reload`, a restart
that swaps managers, `purge`, `close`, and `keepFresh` with three listeners and
a timer. They also track every device so sign-out can purge them all. The
restart order is theirs to get wrong, and the notes app got it wrong.

### Design

**`SyncManager.reconnect()`** (`clients/ordo-sdk/src/replica/manager.ts`)

- Aborts each live entry's controller with a `reconnect` reason.
- `#run` catches that reason, gives the entry a new controller, sets
  `attempts` to 0, marks it loading, and loops with no backoff. Because `#run`
  awaits `syncOp`, the new feed opens only after the old one ended, so one key
  never has two feeds.
- A `stopped` entry (a standing refusal) stays stopped.
- Overlapping calls share one promise, resolved when every entry has reopened.
- The manager, its memory and its watchers stay the same objects, so screens
  do not restart or flash.
- The revoked latch stays set until a new device is opened.

**`openDevice`** (new `clients/ordo-sdk/src/replica/device.ts`)

```ts
interface OpenDeviceOptions {
  client: OrdoClient; space: string; principal: string;
  backend?: Backend;          // default: IndexedDbBackend.open(store)
  store?: string;             // IndexedDB name, default 'ordo'
  manager?: SyncManagerOptions;
  idleMs?: number;            // item 6
  tabs?: 'share' | 'own';     // item 7
}
interface OrdoDevice {
  readonly space: string; readonly principal: string;
  readonly replica: LocalReplica; readonly manager: SyncManager;
  readonly refineRows; readonly refineMerge; readonly refineRowsAsync;
  reconnect(): Promise<void>;
  nudge(): void;
  purge(): Promise<void>;     // stop, then erase
  close(): Promise<void>;     // stop, keep rows
}
function openDevice(o: OpenDeviceOptions): Promise<OrdoDevice>;
```

It listens to `online` and calls `nudge()`. It has no focus polling and no
timer (decision 2).

**ordo-react** keeps React as its only dependency. `deviceCopy` accepts an
`OrdoDevice`-shaped object, and when it has `reconnect`, `deviceCopy.reload()`
calls it. `OrdoProvider device={...}` accepts either a `DeviceCopy` or that
object and wraps it once. The app writes:

```tsx
<OrdoProvider client={hooks} device={await openDevice({ client, space, principal })}>
```

**Notes:** `offline.ts` is deleted; `App.tsx` and `SharedView.tsx` use the
registry; `keepFresh` goes if decision 2 is yes. **No server or wire change.**

### What could go wrong

- **Membership.** Reconnecting in place removes the second writer in a tab. A
  frame being applied when the abort lands commits whole. Another tab is still
  a second writer until 7, so `members` stays the guard.
- **Result hashes.** A reconnect mid-load resumes on the hash of the last
  complete load or change. Equal means the partial rows are a subset; unequal
  means a full load. No new case.
- **Revocation.** A reconnect must not clear `revoked` or reopen a stopped op.
- **Purge during a reconnect.** `purge` stops, waits for every task, then
  erases.
- **Store name.** An app moving to `openDevice` without passing its old store
  name re-syncs from empty. Nothing is lost, because writes are never queued.
  The notes migration passes `'ordo-notes'`.
- **Back-compat.** Additive. `deviceCopy({ manager: () => m, reload })` keeps
  working. SDK and ordo-react each take a minor version.

### Tests

- SDK `test/manager.test.ts`: `reconnect` opens a new feed per key only after
  the old one ended; an edit between abort and reopen leaves a filtered handle
  exactly matching (the stale-row case, with no swap); a stopped op stays
  stopped and `revoked` holds; overlapping calls open one feed per key; a
  reconnect after a complete load sends `X-Ordo-Resume` and moves no rows when
  unchanged.
- SDK `test/device.test.ts`: the namespace, `purge`, `close`, `online` calls
  `nudge`.
- ordo-react `test/device.test.ts`: the provider takes an `OrdoDevice`;
  `reload` calls `reconnect`; a screen's `version` keeps counting across a
  reconnect.
- Notes e2e: the existing tag, edit, delete, offline, revocation and sign-out
  cases, plus a new "a tenant made after the server booted gets a second tab's
  edit without a reload", which proves decision 2 before `keepFresh` goes.

**Size: M.** About 1 session and 1 to 2 CI rounds. The pace is set by the
e2e on CI, and by the new-tenant push check: if it fails, `keepFresh` stays
(calling `nudge`) and the listener fix is its own task.

---

## 6 · Dropping syncs nobody uses

### Today

There is no way to give a declaration back. A tag list mounted once keeps its
feed open until the device closes, each `keepFresh` tick reopens it, and its
rows stay on disk until sign-out. `revokeOp` is the only way out, and it is for
revocation.

### Design

**SDK manager: `idle(key)`.** A step between running and `drop`: abort the
feed, remove the entry from `view.ops` (so `complete` and the common
`position` stop waiting on it), and clear the handle's memory if nobody
observes it. It **keeps the holdings and the stored result hash**, so a later
`sync()` for the same key resumes, costing one server read and no rows when
nothing changed. `drop` still releases everything.

**ordo-react `deviceCopy`: counts per handle.** A new `use(op, params,
syncWhere)` returns `{ handle, release }`. `declare` stays as one use never
released. At 0 uses a timer of `idleMs` starts (decision 3); if still unused
when it fires, `deviceCopy` calls `manager.idle(handle)` and removes it from
`declared`. A new use cancels the timer. A manager without `idle` keeps
today's behaviour.

**`useDevice`** calls `use` in its subscribe and effect, and `release` in
their cleanups. StrictMode's mount, unmount, mount lands inside the delay.

**6b, optional (decision 4): forget long-unused rows.** The replica keeps a
sealed "last used" time per holder, written on `use` and refreshed about
hourly while in use. On `openDevice`, under the replica lock, holders unused
past the retention period are released with `drop` semantics.

**No server or wire change.**

### What could go wrong

- **Shared holder keys across tabs.** Every tab on one replica uses the same
  holder key. If `idle` released rows, tab A going idle would empty tab B's
  list. That is why `idle` keeps holdings. The 6b sweep touches only holders
  unused for days, and an open tab refreshes its time hourly.
- **Kept rows of an idle holder.** A remounted list shows its kept rows at
  once with `complete: false` until the resume or load finishes. Offline it
  can show a note deleted elsewhere since; that is the same promise as any
  offline read.
- **Result hash while idle** is the one stored with the last frame applied, so
  a resume cannot land on a set the device does not hold (C4d, unchanged).
- **Position** can rise when an idle entry leaves the view, and never falls.
- **Back-compat.** Direct `declare` callers keep every sync. Hook users get
  dropping by default, and `idleMs: Infinity` keeps the old behaviour.

### Tests

- SDK: `idle` ends the feed, leaves the view, keeps holdings and the hash;
  declaring again resumes and moves no rows; an edit while idle that took a
  row out gives an unequal resume, a full load, and the row leaves; two
  managers on one backend, A idles and B is untouched.
- ordo-react, fake timers: two hooks on one handle, one unmounts, feed stays;
  last unmounts and remounts within `idleMs`, one feed throughout; after
  `idleMs` the feed is closed and `reload` does not reopen it; StrictMode
  double mount, one feed.
- 6b: the sweep releases an old holder and keeps today's, under the lock.
- Notes e2e: "a person clicks through five tags; after the idle delay the page
  holds sync feeds only for the lists on screen", counting open `?sync=1`
  requests, then back to the first tag shows its notes at once.

**Size: M** for 6a, **S** for 6b. The pace is set by one CI round for the new
e2e and the owner's answers on the delay and retention.

---

## 7 · One stream per tab

### Today

Every synced op, parameter set and shared space holds an HTTP connection.
Browsers allow 6 per host over HTTP/1.1, shared across tabs, so a second or
third notes tab stalls, writes included. Over HTTP/2 the limit does not apply,
but each tab still repeats every feed on the server.

### 7a · A leader tab (client only, no wire change)

- **Election** per replica namespace (space and person) inside `openDevice`.
  The leader holds `navigator.locks.request('ordo-sync-leader:<ns>')`. Tabs
  talk on `BroadcastChannel('ordo-sync:<ns>')`. Naming both by namespace keeps
  one person's rows off another's channel on the same origin.
- **The leader** runs the only real `SyncManager` and is the only tab writing
  frames. After each frame it posts `{key, state, delta}` (the same
  `SyncDelta`, `members` included), plus `view` changes and `poisoned`.
- **A follower** runs a follower manager with the same surface. `sync`,
  `idle`, `reconnect` and `nudge` become messages to the leader with a tab id.
  `observe` fills from the store once, then applies the leader's deltas; the
  fill buffer and `members` cover the race.
- **Counting tabs.** The leader counts which tabs use each key and idles a key
  only when none do (item 6 at the second level). Each tab holds an
  `ordo-tab:<id>` lock; when a tab dies, the leader sees its lock gone and
  drops its counts.
- **Handover.** When the leader closes, a waiting tab wins, posts `leader`,
  every tab posts its keys again, and the new leader resumes on stored hashes.
  A hidden leader gives way when a visible tab asks, because browsers slow
  hidden tabs' timers.
- **Fallback.** Without `BroadcastChannel` or `navigator.locks`, or with
  `tabs: 'own'`, each tab leads itself, as today.
- **Result.** One set of feeds per person and space, however many tabs.

### 7b · Sync on the shared `/live` stream (server and wire change)

- **Wire.** `POST /live/{stream}` takes `"sync": true` and `"resume"` in the
  body. The stream gains `load`, `change` and `error` frames tagged by key.
  `DELETE` ends one read.
- **Server.** `live.rs` carries row frames; its unbounded signal channel
  becomes bounded for rows, so a slow client holds its tasks back. `sync.rs`
  sends into the shared sender. A new cap `ORDO_LIVE_MAX_SYNCS` beside
  `ORDO_LIVE_MAX_READS`.
- **Result.** Two connections per tab (the hook's `/live` and the manager's).
  It does not stop tabs repeating the server's work.

### Recommendation

Build **7a** first: it fixes the case that runs out (several tabs) and the
server's repeated work, with no wire change. Build **7b** only if a deployment
must serve HTTP/1.1. Separately, `deploying.md` should tell the deployment
owner to serve the door over HTTP/2 at the proxy (decision 6).

### What could go wrong

- **Membership.** A follower never applies a delta to a handle it fills later
  without the buffer; `members` stays the final word. A delta after the
  follower stopped observing is ignored. The leader decides idles only on its
  own counts, after the delay.
- **Two leaders for a moment.** The old leader may still be applying a frame
  when it loses the lock. The replica lock serialises writes, positions never
  go backwards (C3), and `members` fixes memory.
- **Sign-out in one tab** purges the shared replica as today; the leader posts
  `poisoned` and every tab empties at once (C7). The registry should key by
  space and person (decision 7).
- **Shared spaces.** Each window token's device elects on its own namespace.
  Revocation reaches every tab through `poisoned`.
- **Background tabs** are measured in Chrome and WebKit.
- **Back-compat.** `tabs: 'own'` keeps today's behaviour.

### Tests

- SDK, with an in-memory `BroadcastChannel` and lock manager: two devices,
  one opens feeds, both screens match; the leader closes, the follower leads
  and resumes with no rows moved; a follower's new op is opened by the leader;
  one tab releases while another uses the key, feed stays; a dead tab's count
  is dropped; poison in one tab empties every tab; no `BroadcastChannel`, each
  leads itself; a filtered handle in a follower keeps its exact set across an
  edit and a handover.
- 7b Rust (`crates/ordo-api/tests/api.rs`): tagged frames, one refused read
  leaves others running, `resume` equal and unequal, `DELETE`, the cap, a slow
  reader's memory. Postgres (`crates/ordo-host/tests/deployment.rs`): a
  membership removal arrives as deletes.
- Notes e2e on the HTTP/1.1 stack: "seven tabs open for one person: every tab
  shows the notes and saves"; "an edit in tab 1 shows in tab 3 while tab 2
  leads, and keeps showing after tab 2 closes".

**Size: 7a L** (2 to 3 sessions with a review between). The pace is set by
getting multi-tab Playwright tests steady in Chromium and WebKit (expect 2 or
3 tries) and 12-minute CI rounds. **7b L**, paced by Rust CI rounds, a review
of the bounded channel, and a check of whether the deployed door already
speaks HTTP/2.

---

## Build order

1. The 4 small items land.
2. **5a:** `SyncManager.reconnect()`; `deviceCopy.reload` calls it.
3. **5b:** `openDevice`; the provider accepts it; notes moves onto it and the
   registry; the new-tenant push e2e; `keepFresh` goes if decision 2 is yes.
4. **6a:** `idle()`, counts in `deviceCopy`, `useDevice` releases.
5. **6b** if decision 4 is yes.
6. **7a:** the leader tab inside `openDevice`, on by default.
7. **7b** only if decision 5 asks for it.
8. Docs in the same push as each step: `howto-loading.md` §6 and §7,
   `howto-sign-in.md`, the React and SDK READMEs, `deploying.md` on HTTP/2.

Steps 2 and 4 touch different methods of `manager.ts`; doing them in order
avoids a merge in the busiest file.

## Decisions for the owner

On the board as `plan-a-ready-made-device-dropping-unused`:

1. `openDevice` in the SDK, with `OrdoProvider` taking what it returns.
2. Drop `keepFresh` once an e2e proves new tenants get pushes.
3. An unused sync stays open 15 s after its last hook unmounts.
4. Rows held only by a sync unused for 7 days are erased on next open.
5. Leader tab first; the shared sync stream only for HTTP/1.1.
6. `deploying.md` recommends HTTP/2 at the proxy.
7. The device registry is keyed by space and person.
