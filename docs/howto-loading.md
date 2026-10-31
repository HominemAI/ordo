# Loading a page in stages

This guide is for an app developer who has not used Ordo before. It says what
Ordo promises about loading, and how to build a page that a person can use
before all of its data has arrived.

You design how your page loads. Ordo gives you the pieces: hooks that answer
at once with what they have, a flag that says whether that is everything, and
settings for how much comes at a time. Ordo's part is to never hold your page's
main thread while it works (owner ruling, 2026-10-10).

The examples use `@hominemai/ordo-react` and the notes app in
`examples/notes/`. Op names are your program's.

---

## 1. What Ordo promises

### What never blocks

- **Network reads.** `useOpQuery` and `useOpPages` fetch with `fetch` and hand
  rows to you as they are. There is no per-row decoding, no deep copy, and no
  sort of a server answer.
- **Live updates.** A live read gets a small "this changed" signal, not the
  data. Bursts are paced to one refetch per 250 ms for each hook, so 5 live hooks can make 5 refetches in that time. Rows already on screen
  stay there until the new ones arrive.
- **Writes.** `useCommit` resolves when the server answers. It does not wait
  for any read to refetch.
- **Sign-in.** All crypto is the browser's async WebCrypto. A passkey prompt
  is the browser's own sheet.
- **Honest answers.** A hook says when its rows may be partial (`complete`)
  and when it has nothing yet (`loading`). It never pretends to be done.

- **A device-copy screen during a load.** A screen keeps its rows in
  memory and merges each sync frame into them. It never reads the store
  again during a load. One screen settles within 1.1x of the load alone at
  3k, 10k and 100k rows, and the rows and `complete` always come from the
  same frames.
- **Local sort and filter.** Each sort key is parsed once per row. 100k rows
  sort by money in about 60 ms and by text in about 85 ms. A first answer or
  a new query over more than 5,000 rows is refined in slices when the device
  copy is given `refineRowsAsync` (section 6), and the longest stall measured
  at 100k rows is under 5 ms.
- **The device store.** Key scans read only the replica's own range, and a
  sync frame is one write transaction. A read mid-load answers at once.
- **A live read.** It fetches at once, beside the subscribe, and fetches once
  more when its read opens on the stream. A stream that says nothing for
  `connectMs` (3 s by default) is treated as failed and retried.
- **Sign-in.** Every sign-in call gives up after `callTimeoutMs` (15 s by
  default) with an `OrdoNetworkError` of kind `timeout`. An op call gives up
  after 60 s, set by the same option on `OrdoClient`. A delegation wait stops
  at the request's own expiry.
- **Staying current, with no polling.** A feed is pushed to, so a page needs
  no timer. The SDK does not reload feeds when a tab regains focus, and it
  runs no timer of its own (owner, 2026-10-21). The sync manager opens its
  feeds again when the browser comes back online, and `device.reconnect()`
  does it on demand. A tenant made after the server booted is pushed to as
  well, because the server listens when a feed opens.
- **No answer.** A call that never got one fails with `OrdoNetworkError`:
  `blocked` (the server is up and refuses the page's origin), `unreachable`,
  or `timeout`. Each names its fix. A page does not guess from a `TypeError`.
- **Long lists.** `exhaust()` shows rows every 10 pages as they arrive, takes
  a signal to stop, and copies each row once.

### What can still block

All of these are bounded:

| What | When it bites | What to do |
|---|---|---|
| A device copy loads its op's whole result, at 50 rows a frame and 50 ms between frames by default | An op with very many rows | Narrow it with `syncWhere`, or ask the deployment owner for larger frames (section 7) |
| A busy server is retried twice, up to about 24 s | The database is saturated | Show `useOpQuery`'s `retrying` |
| A sync frame's body is one `JSON.parse` | Frames of 1,000 rows, the ceiling | Nothing: a frame stays well under a long task |

The record is `docs/audit-client-blocking-2026-10-10.md`, where each finding
names the commit that fixed it.

---

## 2. Show what you have at once

Every read hook returns at once. Draw from what it gives you.

```tsx
function Notes() {
  const { rows, loading, complete } = useOpQuery('listNotes');

  if (loading && rows.length === 0) return <NotesSkeleton />;
  return (
    <>
      <NoteList notes={rows} />
      {!complete && <p className="hint">Still loading…</p>}
    </>
  );
}
```

- `loading` is true only while the hook has nothing to say yet.
- `complete` is false while a device copy is still arriving. The rows you
  have are real; more may follow. From the network it is always true.
- Never hide rows you already have behind `complete`. Show them, and say more
  are coming.

---

## 3. Load the first screen first

A page usually needs a few ops to draw what a person sees first, and more for
what they open later. Ask for the first ones first.

**Parallel, not serial.** Two hooks in one component fetch at the same time.
Do not make one wait for another unless it needs its answer.

```tsx
function Dashboard() {
  // These three start together.
  const notes = useOpQuery('listNotes');
  const tags = useOpQuery('listTags');
  const me = useOpQuery('myProfile');
  // ...
}
```

**A dependent read waits on purpose.** Use `enabled` so it starts as soon as
its input exists, and not before.

```tsx
const note = useOpQuery('getNote', { params: { id } });
const comments = useOpQuery('listComments', {
  params: { note: id },
  enabled: note.row !== undefined,
});
```

**The rest after the first screen.** Mount the second tier once the first has
drawn, or when the person asks for it.

```tsx
function NotePage({ id }: { id: string }) {
  const note = useOpQuery('getNote', { params: { id } });
  return (
    <>
      <NoteBody note={note.row} />
      {note.row && <Suspended>{() => <History id={id} />}</Suspended>}
    </>
  );
}

// Mounts its child after the browser has painted.
function Suspended({ children }: { children: () => React.ReactNode }) {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const id = requestIdleCallback(() => setReady(true), { timeout: 500 });
    return () => cancelIdleCallback(id);
  }, []);
  return ready ? <>{children()}</> : null;
}
```

A tab, a drawer, or a "show more" link does the same job with no helper: a
hook inside a closed tab is not mounted, so it fetches nothing.

---

## 4. Long lists: pages, not everything

A read returns pages of 50 rows. `useOpQuery` gives you the first page.
`useOpPages` gives you a page at a time.

```tsx
function AllNotes() {
  const { rows, hasMore, more, loading } = useOpPages('listNotes');
  return (
    <>
      <NoteList notes={rows} />
      {hasMore && (
        <button onClick={more} disabled={loading}>
          Show more
        </button>
      )}
    </>
  );
}
```

- Call `more()` from a button, or when the last row scrolls into view.
- `more()` adds each page to the same `rows` array, so no page copies the
  ones before it. Key a `useMemo` on `version`, which changes every page.
- Filter on the server with `refine`, so the pages hold only what you show.
- `exhaust()` shows the rows so far every 10 pages (`publishEvery`) and stops
  on a signal. For a large list, `more()` still asks the server for less.
- Render long lists with a virtualized list, so the DOM holds what is on
  screen and not every row.

---

## 5. Live updates

Turn live on where the data changes under a person's eyes.

```tsx
<OrdoProvider client={client} live={{ paceMs: 250 }}>
```

Or per hook:

```tsx
const inbox = useOpQuery('listInbox', { live: true, livePaceMs: 1000 });
```

- A hook refetches at most once per `paceMs`. Raise it for a busy list.
- The old rows stay on screen during a refetch. `loading` does not flip back.
- Each live read holds a connection at the door while it is mounted. Live on
  every hook costs more than live on the screens that need it.
- A live read fetches at once and does not wait for the stream to open. If
  the stream says nothing for `connectMs`, it is retried:

```tsx
<OrdoProvider client={client} live={{ paceMs: 250, connectMs: 3000 }}>
```

---

## 6. The device copy

With a device copy, a syncable read is mirrored onto the device, sealed at
rest, and answered from there. The page draws from the device at once, works
offline, and filters and sorts locally without asking the server.

```tsx
import { openDevice } from '@hominemai/ordo-sdk';

const device = await openDevice({ client: sdkClient, space, principal });

<OrdoProvider client={client} device={device}>
```

`openDevice` opens the replica in IndexedDB (database `'ordo'`, or `store`),
starts a sync manager over it, and hands the hooks all three refiners.
`refineMerge` lets a screen merge each frame into its answer instead of
refining every row again. `refineRowsAsync` refines a large first answer in
slices of `refineChunk` rows (5,000 by default).

- `device.reconnect()` reopens every feed on the same manager and resolves
  when each has answered. Screens keep their rows meanwhile.
- `device.purge()` stops every feed, then erases what is held. Sign-out
  calls it.
- `device.close()` stops every feed and keeps the rows.
- **No polling.** The device reloads nothing when a tab regains focus and
  runs no timer. The server pushes each change down the open feed. When the
  browser fires `online`, every feed reopens at once.
- **A sync nobody uses closes after 15 s.** When the last screen using an op
  at some parameters unmounts, its feed stays open for `idleMs` (15 s by
  default, owner 2026-10-21), then closes. Its rows stay on the device. The
  next screen that asks shows them at once with `complete: false`, and the
  feed resumes on the stored result hash: one server read, and no rows moved
  when nothing changed. A screen back within 15 s keeps the same feed.
  `idleMs: Infinity` on `openDevice` keeps every sync open until sign-out.
- **Rows nobody used for 7 days are erased.** The device notes when each
  sync was last used: when a screen asks for it, when it goes idle, and every
  hour while it runs. When a device opens, a sync unused for `eraseAfterMs`
  (7 days by default, owner 2026-10-21) is let go as if dropped. A row no
  other sync holds leaves the device. This runs under the replica lock, so a
  tab using the sync at that moment is seen. A sync from before these notes
  were kept is never erased this way.
- **One set of feeds however many tabs.** The tabs of one person in one
  space elect a leader with a Web Lock. The leader runs the only feeds and
  writes the frames; it passes each frame to the other tabs on a
  `BroadcastChannel`, and their screens follow. When the leader closes,
  another tab takes over and resumes each feed on its stored hash. A hidden
  leader gives way to a visible tab. Sign-out in one tab empties every tab.
  This is on by default when `openDevice` opens IndexedDB itself;
  `tabs: 'own'` turns it off. Serve the door over HTTP/2 as well
  (`docs/deploying.md`), because over HTTP/1.1 the browser allows 6
  connections per host across all tabs.
- An app that shows several spaces keeps one device per space and person in
  a `DeviceRegistry` (`@hominemai/ordo-react`). `registry.purgeAll(person)`
  erases that person's devices at sign-out.

An app that builds its own manager still can: `deviceCopy({ manager,
replica, refineRows, refineMerge, refineRowsAsync })` is what the provider
builds from a device.

Without `refineRowsAsync`, a first answer or a new query over `refineChunk`
rows is refined in one task. Over many rows that holds the page for a moment.
The device warns once in the console when it happens.

How it loads:

- The first time a hook asks for a syncable op, the device syncs that op's
  **whole result**. There is no cap. A large op takes a while, and `complete`
  is false until it is done.
- Rows appear as they arrive. Show them (section 2).
- On the next visit the rows are already on the device. The page draws from
  them, and the sync catches up behind.
- `source` says where the rows came from, and `deviceFallback` says why a read
  went to the network.
- A synced read with parameters, such as `notesTagged({ tag })`, holds its
  result and nothing else. A row an edit takes out of it leaves the screen,
  and a row an edit brings into it joins. You do not filter the rows again on
  the page. This holds with several tabs open.
- Feeds never pass through the browser's HTTP cache, and the door tells the
  browser not to store any answer it gives a member.
- **`syncWhere`** narrows the sync itself. The server sends only the rows that
  pass it, so a screen on a large op holds a slice:

```tsx
const open = useOpQuery('listTasks', { syncWhere: [{ field: 'status', op: 'eq', value: 'open' }] });
```

**`device: false`** sends one read to the network although it could be
synced:

```tsx
// A big archive: page it from the server, do not mirror it.
const archive = useOpPages('listArchivedNotes', { device: false });
```

Use it for:

- large lists a person pages through and rarely revisits;
- reads that want the server's own sorting and filtering over many rows.

Keep on the device the ops a person works in every day: their notes, their
tasks, their settings. Those are the ones offline matters for.

---

## 7. Settings that shape loading

| Setting | Default | Who sets it today |
|---|---|---|
| Rows per page of a read | 50 | Nobody: hardcoded in the host |
| Rows per sync frame | 50 | The deployment owner, `ORDO_SYNC_PAGE_ROWS` (1 to 1,000) |
| Wait between sync pages | 50 ms | The deployment owner, `ORDO_SYNC_PAGE_WAIT_MS` (0 to 10,000) |
| Live refetch pace, app-wide | 250 ms | You, `live={{ paceMs }}` on `OrdoProvider` |
| Live refetch pace, one hook | the app's pace | You, `livePaceMs` (it can only be slower) |
| Live stream idle close | 60 s | The deployment, `ORDO_LIVE_IDLE_CLOSE_SECS` |
| Reads per live stream | 256 | The deployment, `ORDO_LIVE_MAX_READS` |
| Wait for a live stream to open | 3 s | You, `live={{ connectMs }}` on `OrdoProvider` |
| Device snapshots during a load | one per 100 ms per screen | You, `manager.publishMs` on `openDevice` |
| Device database name | `'ordo'` | You, `store` on `openDevice` |
| A sync with no screen closes after | 15 s | You, `idleMs` on `openDevice` (`Infinity` never closes it) |
| Rows of a sync unused this long are erased on open | 7 days | You, `eraseAfterMs` on `openDevice` (`Infinity` keeps them) |
| Tabs share one set of feeds | on, when `openDevice` opens IndexedDB | You, `tabs: 'own'` on `openDevice` |
| A running sync's last-used time is written every | 1 hour | You, `manager.touchEveryMs` on `openDevice` |
| Rows above which a device refine runs in slices | 5,000 | You, `refineChunk` on `deviceCopy` |
| Pages between `exhaust()` updates | 10 | You, `exhaust(max, { publishEvery })` |
| Wait for a sign-in call | 15 s | You, `callTimeoutMs` on the sign-in endpoint |
| Wait for an op call | 60 s | You, `callTimeoutMs` on `OrdoClient` |
| Probe that tells `blocked` from `unreachable` | `/health/live`, answer kept 5 s, waits 5 s | You, `network.probePath`, `probeCacheMs`, `probeTimeoutMs` |
| Reconnect on the `online` event | on | You, `manager.online` on `openDevice` (`null` turns it off) |
| Reload on focus, or on a timer | never | Nobody: the server pushes changes |

The deployment owner sets the two sync numbers at boot. A value outside its
bounds refuses the boot and names the setting and its default. Larger frames
load a device copy in fewer round trips; each frame is still one parse on the
page, which is why 1,000 is the ceiling. A wait of 0 loads as fast as the
database answers, at the cost of other callers.

---

## 8. A checklist for a page

1. Draw from `rows` at once. Show a hint while `complete` is false.
2. Start the first screen's reads together. Gate only true dependencies with
   `enabled`.
3. Mount the second tier after the first paints, or when asked.
4. Page long lists with `useOpPages` and `more()`. Filter with `refine`.
5. Turn live on where data changes under the person's eyes.
6. Put daily ops on the device. Send big archives to the network with
   `device: false`.
7. Do your own heavy work off the main thread: a Web Worker for big
   computations, or `scheduler.yield()` between chunks.

See also `docs/howto-sign-in.md` for the provider and the device copy set up
end to end, and `clients/ordo-react/README.md` for every hook option.
