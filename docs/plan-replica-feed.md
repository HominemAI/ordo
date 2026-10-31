# The replica's feed: design pass for C4

**`plan-client-cache.md` C4. For a synced op, the server sends the device the row-level changes to that op's result for the caller, at positions, over the live transport.**

Ruled, and not re-opened here (2026-09-14): the replica is a replica of rows; a device may receive only what an ordinary call would return, through **op-scoped sync**, with the whole-space feed as the special case for unguarded reads; positions are read in the rows' own snapshot (C1, built); the device store orders rows by position and keeps removals at theirs (C3, built).

---

## 0 · The starting state, verified

### 0.1 The live stream keeps nothing but a subscription [settled]

`GET /op/{name}?live=1` (`crates/ordo-api/src/router.rs`, `live_inner`) subscribes to the op's invalidation channel before its first read, sends the ordinary response envelope as the first frame, and then sends `event: invalidate` with an empty payload for every bus signal. It refuses paging on a subscription. Since R6 (`plan-refinement.md` section 7) it accepts `$refine`: the first frame is the refined read, and a sync takes `where` and refuses `pick`. It holds no copy of what it sent.

### 0.2 The manifest already says which results have row identity [settled]

Every read op's manifest carries `result.anchor`, "the base table every row of this result came from, the entity whose implicit typed id underwrites the keyset cursor", `null` for a union or a CTE; `result.grouped`, true when the rows are synthetic and "have no identity, no detail link and no row actions"; and `result.single_row` for a measure (`crates/ordo-plan/src/artifact.rs`). So the platform can already tell, per op, whether each result row is a stored row of one entity.

### 0.3 Each op's cost facts are on its spec [settled]

`OpSpec.cost` carries the cost class, `unbounded_ack` (the author's acknowledgment that the op's work grows with the space) and `refine_candidate_bounded` (whether the result set is provably bounded). `OpSpec.aggregate_only` marks a measure.

### 0.4 A read reports the position its rows were read at [settled, C1]

`ExecResult.position` is the space's command-log position, taken in the same snapshot as the rows.

---

## 1 · Which ops can be synced

An op can be synced when its result is made of stored rows of one entity that the device can key: **the manifest names an anchor, the result is not grouped, the op is not a measure, and the projection includes the anchor's `id`.** Everything else stays an ordinary call or an ordinary live subscription.

**Superseded 2026-09-16 (`feed-id-uigen`), and the reasoning below was wrong.** It read: "Requiring `id` in the projection rather than having the server add it keeps one promise exact: a synced op sends nothing an ordinary call to the same op would not return. The paging cursor carries the id today, but inside an authenticated token the client cannot read." The cursor is authenticated, **not encrypted** (`ordo-api/src/page.rs`: `base64url(JSON{v,k}) . base64url(HMAC)`), so every caller can already read the last row's id on every page. `id` is reserved and implicit, can never be sealed or blind-indexed, and is the tiebreak of every row plan, so no caller is entitled to a row but not to its id. The rule is now `ui-gen.md` §1's: any anchored, ungrouped read can be synced and the anchor's id rides every row, attached by the executor to ordinary reads as well as to feed frames. Two shapes need named refusals under it, which `IdNotProjected` was refusing by accident: `distinct` (its rows are not stored rows) and a many-side source join (several rows share one anchor id).

---

## 2 · The frames

A sync is the live subscription in a second mode, `GET /op/{name}?live=1&sync=1`, under the same door, the same guards and the same principal. Every row a frame carries is keyed `(entity, id)`.

- **The load.** The op's result arrives in pages, in the order of the anchor's id, each page read at its own position and carrying its signed pin (C1). Every page frame says how many rows have arrived so far and whether the load is complete. The server waits between pages (section 4), so a large result arrives over time and the frames say how far along it is.
- **Changes.** After the load, on each invalidation the server reads the op again at a new position and sends only the difference: `upserts` for rows that are new or changed, and `deletes` for rows that were in the previous result and are not in this one.
- **A delete means "no longer in this op's result for you"**, which is the same message whether the row was deleted, changed so it no longer matches, or hidden by a guard change. A guard change already invalidates every op whose guard reads the changed entity (G16), so rows leave the device without any mechanism of their own.

The device applies each frame to its store at the frame's position (C3): a lower position never replaces a higher one, and a delete is kept at its position. A row the device holds through two synced ops stays until neither holds it.

---

## 3 · How the server knows what the device holds [ruled 2026-09-15: (A)]

**The subscription remembers its last result.** For the life of the connection the server keeps each row's `(entity, id)` and a hash of the row it last sent, and diffs each new read against that. Memory grows with the result, at roughly fifty bytes a row: about 5 MB per open subscription for 100,000 rows.

**Reconnect.** A dropped connection loses the server's memory, so on reconnect the server runs the load again, paced like a first load, for each op the device syncs. Only synced ops are affected; an ordinary call has no connection to lose. The device keeps answering from its store throughout: the rows it holds stay readable, no row moves to a lower position (C3), and the op reads as loading until the reload completes. When it completes, the rows that op held on the device and the reload did not include are released at the completion position, because a complete load is the whole result.

**Not taken, and the path if reconnects cost too much.** (B) the device describes what it holds and the server sends only what changed; (C) incremental maintenance from the command log, which needs each guard's dependency set, because one access change can hide rows no command named. Either can sit behind the same frames (Q-4).

---

## 4 · Large results [ruled 2026-09-15: no cap, paced loads]

The owner ruled out a fixed row cap as arbitrary. A synced op may be as large as its result; what changes with size is how long the load takes, and the device is told.

- **Paced pages.** The load reads a page, sends it, and waits before reading the next. Page size and the wait are per-deployment settings, with defaults set from a measurement in C4b. Each page is an ordinary bounded read, so a large load never holds a long transaction or a long-lived snapshot open against the database.
- **Honest progress.** Every page frame carries the rows loaded so far and `complete: false` until the last page. The SDK exposes that state, so an app can show that a result is still loading, and an answer taken from a still-loading op says it may be partial. That covers the concern the cap was for, a device answering from a partial result without knowing it.
- **Changes during a load.** Pages are read at successive positions, so a row can change after its page was sent. Invalidations that arrive during a load are held, and when any arrived, one difference pass runs after the last page, paced the same way, so the device ends at a single position.
- **Steady-state cost.** Under (A) each invalidation re-reads the op's whole result. For a large op the re-read is paced like a load, and invalidations are coalesced, so a subscription has at most one re-read running and one pending. The re-read still grows with the result and its rate of change, which is the cost (C) would remove.

---

## 5 · What this does not do

Writes (the ruled v1: writes go to the server and the device applies the committed rows). Syncing a result with no row identity (grouped, unions, measures), which stays an ordinary call or live subscription. Resuming after a reconnect without a reload (Q-4).

---

## 6 · Increments

**C4a · Syncability.** Each read op's spec says whether it can be synced (section 1), from the manifest's anchor, grouping and projection. *Done when* a grouped op, a measure and an op without `id` in its projection are refused with a reason that says which rule.

*Built 2026-09-15.* `ordo_plan::artifact::syncability` decides from the bound plan and returns the entity and the output that carries each row's id, or the rule the op fails (`no_anchor`, `grouped`, `measure`, `id_not_projected`). It follows the id through a callee wrapped as a subquery, and it refuses a group at any depth, which is stricter than the manifest's `anchor`: that recurses into a grouped callee and names a table whose rows the caller never sees. The artifact carries the answer as `result.sync`, `interface.json` passes it on (a write says `not_a_read`), and the door's `OpSpec.sync` holds it through one crossing in `ordo_host::syncability`, where both spellings are checked against each other. Because `result.sync` is part of each op's contract, the artifact digest moves once and every read op gets a new version on the next deploy. Nothing serves `?sync=1` yet; that is C4b. Across the fixture, only the derived `std.E.list` and `std.E.get` ops project `id`, so they are the only ones syncable under the ruling as recorded (Q-5).

**C4b · The server feed.** `?live=1&sync=1`: the paced load with progress, then differences per invalidation from the subscription's remembered result, coalesced and paced for large results. *Done when* an insert, an update, a delete and a guard change each arrive as the right frame; a change made during a load is on the device once the load completes; and a load of several pages spaces its reads by the configured wait.

*Built 2026-09-15* in `crates/ordo-api/src/sync.rs`. A sync of an op that cannot be synced is refused as `not_live`, with the rule's code and wording in the message: a new refusal code would ripple through the closed inventory and the SDK's mirror of it for no gain a client can act on. The first page is read before the response, so a refusal there is an ordinary HTTP error; a task sends the rest and stops when the client leaves. The frames:

- `event: load`: `{entity, key, position, pin, rows, loaded, complete}`, one per page.
- `event: change`: `{entity, key, position, pin, upserts, deletes, result}`, only when a pass found a difference. `result` is the C4d hash of the whole result after the change (added 2026-10-20).
- `event: error`: the error envelope, once; the stream then ends.

`key` names the output that carries each row's id, and `pin` is the signed head at the frame's position in the SDK's `Pin` shape (C2). The wait between pages is `ApiState.sync.page_wait`: 50 ms by default, not yet measured. The page size is the executor's own. A difference pass re-reads the whole result with the same wait and carries its last page's position. The server holds 16 bytes of SHA-256 per row id. Tests: five in `ordo-api/tests/api.rs` (pages and progress; an update, a delete and an insert as one change; an empty pass sending nothing; the wait between pages; a change during a load; queued signals answered by one pass; a refusal naming its rule), and one on real Postgres in `ordo-host/tests/deployment.rs`, where removing a membership sends the tasks it hid as deletes through the LISTEN bus.

**C4c · The device side.** The SDK consumes the frames into `LocalReplica` (C3), tracks which synced ops hold each row, and exposes each op's loading state. *Done when* a reconnect cannot move a row backwards, a completed reload releases rows the server no longer returns, a row held by two ops survives leaving one of them, and a guard change removes rows from the device.

*Built 2026-09-15* in `@ordo/sdk`. `syncOp(client, replica, spec, params)` opens the feed through `OrdoClient.openFeed`, which sends the same headers as every other call (they now come from one builder), and applies each frame: a `load` puts its rows at its position and records that the op holds them, and when a load completes, the rows the op held before it and the load did not include are released; a `change` puts its upserts and releases its deletes. `LocalReplica` gained `hold`, `release` and `holdings`: which ops hold a row is kept in sealed records under keyed names like every other record, and a row is removed only when no op holds it. `onFrame` reports `{loaded, complete, position, pin}` after each frame, so an app can show that a result is still loading. The generated op table carries `sync: {entity, key} | null` from the descriptor's `result.sync`, and `syncOp` refuses an op without it before the wire. A guard change needs nothing on the device: it arrives as deletes (C4b's Postgres test). Tests: ten in `test/sync.test.ts`, on a fake door that answers with scripted event streams, including a frame split across chunks, a reconnect, and the generated table's `sync` for a syncable list, an unprojected one, a measure and a write.

This is the answer the SDK README said was missing for `.live()`: the read path now carries positions (C1), so frames can be ordered against each other. `@ordo/react` still has its own response type without a pin, which C8 takes up.

---

**C4d · Resume on reconnect.** *Ruled 2026-09-16 (`feed-resume`).* A reconnect says what it already has, and an unchanged result costs no rows.

*Revised 2026-09-16, before building.* The first sketch had the device send `(id, digest)` pairs for every row it held. Two facts killed it. The feed is opened with **GET** (`OrdoClient.openFeed`), so thousands of pairs cannot ride the request without moving the feed to POST. And the device has no digests to send: `digest()` is computed server-side and C4b's frames carry rows, not digests, so "the device returns the digests it was given" described something the device was never given. The owner's words were narrower and are what gets built: *"Can it share a diff hash or version hash and if they disagree on reconnect reconcile."* One hash, not a set.

*The shape.* The completing `load` frame carries one more field, `result`: a hash over the whole result the server just sent, computed from the same per-row digests the subscription already keeps. The device stores it against that op. On reconnect it returns the hash in a header, beside the `x-ordo-*` headers the door already reads. The server runs the op's read, hashes the result the same way, and compares:

- **Equal.** Nothing changed while the device was away. The server sends a `load` frame with no rows and `complete: true` at the new position, and the device keeps every row it holds, at the positions it holds them. No rows cross the wire.
- **Unequal, absent, or unreadable.** A full paced load, exactly as today. The reload is the fallback, never the norm.

*Revised 2026-10-20, after the notes e2e (test 7).* The device used to keep the hash of its last load through every later `change` frame. Its rows moved on and its hash did not, so a result that changed back to the last load's set resumed onto rows the device no longer matched: a note that lost its tag stayed in the tagged list. Each `change` frame now carries `result` as well, and the device stores it in the same write as the change. A `change` without one makes the device forget its hash, so the next connection loads in full. Tests: `a_change_frame_names_the_result_after_it_so_a_resume_matches_what_the_device_holds` in `ordo-api/tests/api.rs`, and the two hash cases in `clients/ordo-sdk/test/manager.test.ts` ("a filtered live read keeps its membership").

The same defect had a second cause, in the SDK's manager. A manager keeps an observed handle's rows in memory from its own frames and one fill from the store, but the store has other writers: a manager being replaced, or another tab on the same replica. A row another writer released after the fill stayed in memory. A completing `load` frame now hands the manager the whole set the holder holds (`SyncDelta.members`), and the manager sets memory to exactly that set.

*Why a whole-result hash and not a per-row one.* Because the win is in the common case: a device that reconnects after a short drop with nothing changed pays one read and no rows. When something did change, the server cannot know *which* rows the device is missing without the device telling it, and telling it is the POST-shaped design above. That trade is worth taking only if reloads after a change turn out to dominate, which is a measurement nobody has yet.

*What it does not do.* It does not remove the re-read: the server still evaluates the op's result to hash it, which is (A)'s cost and what (C) would remove. It removes the **send**, which is what a device on a slow link pays for.

*Done when* a reconnect with an unchanged result transfers no rows and leaves every held row at its position, a reconnect after any change falls back to a full load, a device that sends no hash is loaded in full, and a stale hash from a previous op version is refused rather than trusted.

*Built 2026-09-16.* The completing `load` frame carries `result`, sixteen bytes of hex folded from the per-row digests the subscription already keeps, sorted by id and length-prefixed so it names a **set** of rows rather than an order. The device stores it against the op **and its version**, returns it in `x-ordo-resume` on the next connection (a header, beside the eight the door already reads: it is data about the device, not addressing), and the server runs `scan_result`, which pages the result without sending it. Equal means one frame with no rows, `complete: true` and `resumed: true`; unequal means the rows are read again and sent as an ordinary paced load.

*The cost, stated rather than discovered.* A resume that matches costs **one read and no rows**. A resume that does not match costs **two reads**, because the scan discards rows rather than buffering a whole result in memory. That trade is only worth it while reconnects that find nothing changed are the common case, which is the bet the ruling makes; if changed reconnects come to dominate, the answer is (C), incremental maintenance from the command log, not a bigger buffer.

*The trap this design sets, and the guard.* A resumed frame is a **completing load with no rows**, and C4c's rule is that a completed load is the whole result, so the rows it left out are released. Applied to a resume that rule would release everything the op holds, every time a resume succeeded — the device would be wiped by the mechanism meant to spare it. `resumed: true` is what stops it, and a test pins it on both halves: the device keeps its rows at their positions on a resume, and still releases what a genuine reload left out.

---

**C4e · The anchor's id rides every row.** *Ruled 2026-09-16 (`feed-id-uigen`), and built in two halves so the wire delivers before the document declares.*

*First half, built 2026-09-17: the wire.* `ordo_plan::artifact::project_row_key` puts the anchor's id into the projection when the author did not, under the reserved name `$id`, and says what the rows are keyed by. Both read paths call it after `blind::resolve` and before planning.

*Why it projects rather than reads a cell.* The first plan attached the id at `read_page_as`, reading it out of the hidden cursor columns on the way past. The review that the owner asked for recommended projecting it instead, and that is what got built: from the push onward the id is an output like any other, so the paged path, the unpaged path, the SQL, the manifest and the shape gate all agree with no special cases, and `Hidden(n)` becomes a cross-check rather than arithmetic on the hot path. The expression is **cloned from the order term** rather than rebuilt, because `output_named` compares whole expressions including their type, and a rebuilt one would fail to match its own term and ride both projected and hidden.

*The gate.* Attaching is refused wherever `syncability` refuses: a grouped result, a measure, a `distinct`, a many-side join, a result with no anchor. The locator alone would have answered for some of those, because the id is still an order term there, and `$id` would have ridden fan-out rows and synthetic rows. Two predicates would drift, and the one that drifted would put an id on rows that do not have one.

*The wrapped callee.* A callee that did not project its own id carries its tiebreak out as the hidden `$tb0` the binder already adds, so the id does ride and the column is `$tb0` rather than a guess. Without this the ruling's "any anchored, ungrouped read" was not delivered, and `IdNotProjected` could not be retired.

*What it cost an old ruling.* W047's pin in `ordo-exec/tests/pool.rs` said the hidden key never reaches the client. That still holds for the **sort** key and is still asserted. Row identity now does reach the client, which is `ui-gen.md` §1's rule, so the pin was amended with its reason rather than deleted.

*Proven against Postgres, not a fake:* paged equals unpaged on both read paths, and every `$id` carries its entity's prefix and is unique across pages. That is the failure the review called silent, because reading the wrong cell yields a sort value, and the difference pass would read a sort value as churn rather than fail.

*Second half, built 2026-09-17: the document.* `syncability` now answers with the name instead of refusing, and both halves of the contract read **one** decision (`row_key`): the door gets the name, the executor puts the output in place before planning. Two functions deciding separately could return different names, and the descriptor would then say rows are keyed by one thing while the wire keyed them by another, which nothing would detect until a device filed a row under a key the server never sends. `id_not_projected` is retired from the planner, the door and the crossing, because nothing can fail that way any more.

*What moved, exactly.* **55 of 165 ops are syncable, up from 32.** Twenty-three gained a `$id` output and re-versioned; **no other op's version moved**, because a per-op version hashes that op's own manifest and the other 142 manifests are unchanged byte for byte. Each attached output is described like any other, `type: "id"` with `id_entity` naming the anchor, so a renderer can link a row without parsing a prefix out of a value it never fetched; the generated row types carry `readonly $id: Ids.<E>Id`, the typed id rather than a bare string.

*Two answers became one.* `row_identity` was computed from `(read, grouped)` while `sync` was computed separately, so the document claimed identity for `distinct` results and many-side joins that `sync` refused by name. It now derives from `sync`, and the golden confirms no op claims identity where syncing is refused.

*`$id` is refinable, and that is a decision rather than an accident.* Because the id is a genuine projected output, it appears in `refine.fields` and in the door's own field manifest through the same list, so the two agree with no special case. Filtering a synced list by row id is a reasonable thing to permit, and excluding it would have meant teaching two producers to hide the same column.

*A blind-index op keeps its secret and gains its identity.* `customerByEmail` still withholds its statement, because publishing a per-space digest would be unstable and a membership oracle, and it now carries `$id` like every other anchored read. Withholding the statement and withholding row identity are separate things, and only the first was ever intended.

---

## 7 · Questions for the owner

**Q-1 · How the server knows what the device holds.** *Ruled 2026-09-15: (A)*, the subscription remembers its last result and reloads on reconnect.

**Q-2 · Must a synced op project its `id`?** *Ruled 2026-09-15: yes. **Superseded 2026-09-16** by Q-5: no, the anchor's id rides every row.*

**Q-3 · The size cap.** *Ruled 2026-09-15: no cap.* Large loads arrive in paced pages with honest progress (section 4).

**Q-5 · Q-2 against `ui-gen.md` §1.** **Ruled 2026-09-16: follow `ui-gen.md` §1**, after the review the owner asked for confirmed the switch. Any anchored, ungrouped read can be synced; the anchor's id is attached to every paged read row, not only to feed frames, so an ordinary call and a feed still return the same thing. Q-2's premise was false: the page cursor is authenticated, not encrypted, so the id it carries is already readable. Two amendments: the predicate names `distinct` and many-side source joins as refusals rather than catching them by accident, and the attach point must take the id from the order-key outputs, since keying a synced op by a sort value would look like churn rather than fail. The reconciliation crosses `ordo-plan`, `ordo-exec` and `ordo-api`, re-versions every read op, and moves 23 of the market golden's ops out of `id_not_projected`. Filed as **C4e**.

**Q-4 · Resuming without a reload.** **Ruled 2026-09-16:** build it. The owner: "Can it share a diff hash or version hash and if they disagree on reconnect reconcile. Add it to C4 work." So a reload stops being what a reconnect does and becomes what it falls back to. The material already exists: under (A) the subscription keeps each row's `(entity, id)` and sixteen bytes of SHA-256 of the row it last sent, which is precisely a per-row version. What was missing was a way for the device to say what it holds, and that is one frame. Filed as C4d. The earlier recommendation, to leave resume for later and measure first, is superseded.
