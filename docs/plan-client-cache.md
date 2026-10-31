# The client runtime and its local replica

**Checklist 5g-pre. A device that holds a replica of the rows it may read, answers from it at once and offline, stays current from a feed, and never shows an answer older than one it already showed.**

**Ruled, and not re-opened here.** Local caching is a staple of the system (2026-09-12). It is built before F7, because "cache system needs to be built before f7 as it will affect how f7 functions" (2026-09-12). It is a **replica of rows**, not a cache of answers: "This is supposed to function like jazz - so yes replication - that's the whole point of the indexdb and fast responsive usage" (2026-09-14). And it is **on by default, in IndexedDB** (2026-09-14). This document was first written recommending a cache of answers; that recommendation was overruled and this revision replaces it. **All five questions in §8 are ruled** (2026-09-14).

Companion documents: `implementation.md` "The local store is a database: client storage rules" and "Client delivery" (the mirror's design, most of which this plan builds), `definition-language.md` "Live queries", `api-engine-design.md` §6 (the live transport), `f7-vault-plan.md` §7 Q10 (the device engine), roadmap **5g-rev** (revocable caching, built on this) and **20d** (no change feed).

---

## 0 · The starting state, verified

### 0.1 Nothing in the tree holds a replica [settled]

No crate and no client mentions IndexedDB, OPFS or local storage. `@ordo/sdk` (`clients/ordo-sdk`) is a generated request builder that stores nothing; `@ordo/react` is a headless runtime over the interface descriptor and stores nothing either.

### 0.2 The design documents already specify most of the mirror [settled in the documents, unbuilt]

- **Log-positioned reads** (`implementation.md`, Client delivery, `[settled]`): every read result carries the per-space command-log position it was computed at; multi-query screens and the mirror advance their visible snapshot only when every refetch has reached a new position; the mirror stores its high-water position and applies feed batches atomically per group.
- **Client storage rules** (`implementation.md`, `[proposed]`): sealed at rest under a device key wrapped by a passkey-PRF-derived key; auto-lock with the unwrapped key in memory only (default 15 minutes idle); per-space mirrors, separately keyed, never a cross-space store; mirror scope as a disclosed policy, `policy mirror { keep 90d | keep <n> rows | full }`, on the consent page; `std.space.*` results and command-log slices never mirrored; revocation marks the space poisoned at next contact and the client purges before serving any read.
- **Live queries** (`definition-language.md`, `[settled]`): liveness is a call-time choice; invalidation is a staleness signal and the client refetches under its own scope; "delta-push can replace refetch behind the same subscription interface later"; and "the IndexedDB mirror is the same pattern one level up (whole-space command feed); per-query subscriptions serve unmirrored screens."
- **The consumer machinery exists server-side.** `$logconsumers` already has `Mirror` and `ClientFirstMirror` kinds with positions and eviction rules (`crates/ordo-exec/src/prune.rs`): a managed mirror is evictable because it is a cache; a client-first mirror is not, because the device is the authoritative store.

### 0.3 Two places where the documents and the ruling disagree [amended 2026-09-14]

- **`bootstrap.md` input 33** defaults the mirror `off` for managed spaces. The owner's ruling makes it on by default. The ruling stands, and input 33 has to be amended (increment C9).
- **"Whole-space command feed"** (`definition-language.md`, Live queries) is only safe for a principal who may read the whole space, because guards are evaluated on the server and a command group carries the images of every row it wrote. Section 2 settles what a device may receive instead, and the sentence has to be amended to match (C9).

### 0.4 The read position is taken after the query, not inside it [fixed 2026-09-14, C1]

Every read response carries `x-ordo-position` and a signed head (layer-1 pinning, 8q), and the SDK ignores them. The router reads the position with a separate query after the call (`run_call`, then `attest.head`), so it is an upper bound on the rows' version rather than the version the settled rule promises. Increment C1.

### 0.5 There is no change feed [settled]

Roadmap 20d: nothing reads the chain out to a client.

---

## 1 · What the replica is

A per-space, per-principal store on the device, in IndexedDB, **on by default**. It holds rows the principal may read, keyed by entity and id, each with the position it was read at. It answers the app's reads from local rows at once, including reads the device has never sent to the server; stays current from a feed; and keeps answering offline. Writes still go to the server in v1 (section 6).

---

## 2 · What a device may receive [ruled 2026-09-14: (c)]

The whole problem in one sentence: **guards are evaluated on the server, and a device must only ever hold what its principal may read.** A membership or acl write can change which rows a principal may read without touching those rows, so the feed also has to be able to take rows away.

**(a) A whole-space command feed, for principals who may read the whole space.** The literal reading of `definition-language.md`. Simple, and safe only where no guard restricts the principal's reads; everyone else gets no replica, only per-query subscriptions. In most apps with guards that is most members.

**(b) A per-principal filtered command feed.** Every command group is filtered on the server to the rows the principal may read after it, and every guard-input change sends retractions for rows that stopped being visible. The fullest Jazz-like behavior, and the hardest: a single acl write can change visibility for many rows and many principals, and the server has to compute each principal's difference.

**(c) Op-scoped sync.** The app declares which ops a device keeps in sync (a list screen, a detail view, a lookup table). For each synced op the server sends row-level changes to that op's result **for this principal**: upserts and deletes keyed by entity and id, at a position. The device stores the rows once, whichever ops brought them, and answers new questions over the union. Guards are applied by the server exactly as for any read, so the device can only ever receive what an ordinary call would have returned. Visibility changes arrive for free, because a guard-input change invalidates the op (G16) and its next delta deletes the rows that left it. This is the "delta-push can replace refetch" path the definition language already anticipates, and it is how the two nearest industry systems (ElectricSQL's shapes, PowerSync's sync rules) solve the same problem.

**Ruled 2026-09-14: (c), with (a) as its special case.** For a principal whose reads are unguarded over what is synced, "every row of these entities" is itself a valid synced op, which is (a) without a separate mechanism. (c) is safe by construction, reuses the op, invalidation and position machinery that exists, and lets an app sync exactly the screens it wants offline. (b) is worth revisiting only if an app needs a replica of rows no op ever reads, which is a strange thing to need.

---

## 3 · Never showing an older answer

The settled rule (0.2) applied to rows: **a row is replaced only by a version at an equal or higher position**, and a screen advances only when every query it shows has reached the new position. Positions compare within a space, since each space has its own chain. This needs C1: with the position read after the query, "higher position" does not yet mean "newer rows".

---

## 4 · Answering new questions on the device [ruled 2026-09-14: level 1 now, level 3 with F7]

A replica earns its keep when the device answers questions without the server. Three levels:

1. **Local refinement over synced rows.** Filter, pick and sort synced rows with the Tier-0 operators the refinement envelope already defines (`eq ne lt le gt ge in is_null`, combined with AND; `api-engine-design.md` §7). Small, needs no second query language, and covers the common interaction: typing into a filter, toggling a column, re-sorting.
2. **Ordo pipelines evaluated in JavaScript.** Full local queries, and a second implementation of the language, which `f7-vault-plan.md` warns is a second evaluator with drift on the critical path.
3. **Postgres in WASM (PGlite).** The same SQL the server runs, no second dialect; a multi-megabyte payload and a slower cold start. `f7-vault-plan.md` Q10 recommends it for the vault.

**Ruled 2026-09-14: level 1 in 5g-pre, level 3 with F7.** Level 1 makes the replica immediately useful; level 3 arrives with the build that needs it anyway. Level 2 is the option to avoid.

---

## 5 · Storage, sealing and the default

- **IndexedDB, on by default.** One store per space and principal, never a cross-space store.
- **Sealed at rest** under a device key (`implementation.md` proposes a passkey-PRF-wrapped key, the mechanism `blob private` already uses). v1 uses a device-local, non-extractable key (Q-3, ruled 2026-09-14), shaped so the passkey-PRF wrap and the server-released key of 5g-rev slot in.
- **Auto-lock** with the key in memory only, as proposed: locked means ciphertext on disk.
- **What sealing protects, stated so nobody over-claims it.** A copied disk or profile holds ciphertext. A script running in the app's origin while unlocked can read what the app can. Taking back what a device already holds is 5g-rev.

---

## 6 · Isolation, purging and writes

- **Never mirrored:** `std.space.*` results and command-log slices.
- **Poisoned at next contact** when access is revoked, and purged before any read is served.
- **Sign-out purges** the principal's stores. **A refusal purges** what that op synced.
- **Mirror scope** is `policy mirror { … }`, disclosed on the consent page, with on-by-default as the deployment default.
- **Writes in v1 go to the server.** When a commit returns its rows, the device applies them at the returned position at once, so the screen changes without waiting for the feed. Writing offline and optimistic local writes need the deterministic evaluator (F7 Phase 4) and stay there.

---

## 7 · Increments

**C1 · The position inside the read. [built 2026-09-14]** The server reads the chain position in the read's own snapshot, as the settled rule says. *Done when* a write committed between a read's rows and its response cannot raise the response's position. *Small, server-side.*

*As built.* A new `OpClass::SnapshotRead` begins `REPEATABLE READ READ ONLY`; `read_page_as` runs its page query and the position query in one such transaction, so both see one snapshot, and a read-only transaction at that level cannot fail to serialize. The position rides out on `Page`, `ExecResult` and the router's `CallOutcome`, and the door pins the signed head at that position through a new `AttestationSource::head_at`. `PgAttestation` answers from its cache when nothing moved and signs the head read back from the chain when a write landed in between; the default answers only when the current head is at the requested position, and otherwise leaves the response unattested rather than signing a head the rows predate. Pinned by `a_snapshot_read_does_not_see_a_write_committed_after_it_began` (`crates/ordo-exec/tests/snapshot_read.rs`), which also shows READ COMMITTED seeing the write, and `head_at_signs_the_head_the_rows_were_read_at` (`crates/ordo-host/tests/attest.rs`).

**C2 · The SDK carries positions. [built 2026-09-14]** `x-ordo-position` and the head onto every response. *Small.*

*As built* (`clients/ordo-sdk/src/core/client.ts`): every `OpResponse` has `pin: Pin | null`, carrying the position, the signed head, its key id and signature, and the position the chain can still be verified from. A position must be a plain count the client can hold exactly, so a malformed or unsafe one gives no pin rather than a rounded number, since the replica orders rows by it. `@ordo/react` has its own response type and does not carry the pin yet; that is C8.

**C3 · The store. [built 2026-09-14]** Per space and principal, IndexedDB with an in-memory implementation for tests, sealed, rows keyed by entity and id with positions. *Done when* rows round-trip and a raw dump holds no plaintext row or parameter.

*As built* (`clients/ordo-sdk/src/replica`): `LocalReplica` enforces the ordering rule (a lower position never replaces a higher one; a removal is kept at its position so an older version cannot bring the row back) and serializes its operations so versions arriving together still leave the newer one. Records are sealed with AES-GCM under a non-extractable device key and named by an HMAC of their parts, so a raw dump shows no row, id, entity, space or principal (asserted). Two backends behind one small key-value interface: IndexedDB, which also holds each replica's non-extractable keys, and memory. `purge` removes a replica and its keys. Tests in `clients/ordo-sdk/test/replica.test.ts`, the last of them on the IndexedDB backend through `fake-indexeddb`, a dev-only dependency; the shipped SDK still depends on nothing. What the store does not do yet: know which ops brought a row (C5), or receive rows from the server (C4).

**C4 · The delta feed.** For a synced op, the server sends upserts and deletes of that op's guarded result for the principal, at positions, over the live transport. *The largest increment, and it gets its own design pass first* (written 2026-09-14, `plan-replica-feed.md`; its first three questions ruled 2026-09-15): how the server knows what a device already has (a client-sent position and row-version summary, or a server-kept result per subscription), and the bound on a delta's size.

**C5 · The sync manager. [built 2026-09-15]** On by default: declares synced ops, applies deltas atomically per position, advances screens only at a common position, handles reconnect. *Done when* a reconnect can never move a row backwards and a guard change removes rows from the device.

*As built* (`clients/ordo-sdk/src/replica/manager.ts`). `SyncManager.start(client, replica, declarations)` declares the ops a device syncs and keeps each one connected through `syncOp`, reopening a feed that ends. It publishes one view. Its `position` is the lowest position any live op has reached, so a screen drawn from two synced ops never mixes one op's newer rows with another's older ones, and it never decreases, which is at the screen what C3 already enforces on rows: a reconnect whose reload answers at a lower position leaves both the row and the view where they were. A frame's rows all reach the replica before the position they arrived at is published, so nothing reads half a delta. A refusal the door names is a standing answer to the same request, so it stops that op and rides on its `ManagedOp.stopped` instead of being asked again; `internal` and `serialization_conflict` reconnect, and a connection that delivered no frames counts as a failure, or a door that closes every stream at once would be reopened in a tight loop. A stopped op is left out of the common position rather than freezing every screen. `drop` releases what one op held, so rows no other synced op holds leave the device, and `syncOp` gained a `holder` so one op synced at two sets of parameters holds its rows apart. Twelve tests in `test/manager.test.ts`, on a door that keeps each feed open so a test pushes frames and closes streams itself.

*Found building it.* `@ordo/sdk` decoded the door's refusal envelope from a `class` field while the door writes `code` (`ApiError::envelope`, `api-engine-design.md` §4, and `@ordo/react` all agree on `code`), so **every refusal the door has ever sent decoded as `internal`** with its message intact, which is why nothing looked wrong. Three of this package's own tests had been written to the invented shape and passed against it. The manager made it matter, because its retry policy reads the class and `internal` is the one class it retries. Fixed here: `decodeRefusal` reads `code`, and the fixtures now send what the door sends.

**C6 · Local refinement. [built 2026-09-15]** Tier-0 filter, pick and sort over synced rows, answering without a request.

*As built* (`clients/ordo-sdk/src/replica/refine.ts`). `refineOp(replica, spec, query)` answers from the rows a synced op holds, and `refineRows` is the same evaluation as a pure function, so a keystroke can refilter without touching the store. The operator set, the AND-only combination, the value shapes and the unknown-field refusal are the door's own rules (`api-engine-design.md` §7), kept identical so one envelope means one thing wherever it is evaluated. Values compare by the type `canonical()` gave them: `int`, `money` and `duration` arrive as JSON numbers and compare numerically, everything else as strings and compares by code point, which is the order Ordo's ISO dates need. Two values of different types never order, because a column has one type and the server would never have been asked. Nulls sort last in both directions, and ties fall to `id`.

*Parity with the server, fixed 2026-09-30 (checklist 25k and 25l).* The first build got three rules wrong, and a shared test now holds them. Null logic is two-valued, as the language says: `null == null` is true, `x != y` is exactly `not (x == y)` so `null != 5` is true, an ordering test with a null operand is false, and a null in an `in` list matches a null value. Text compares by code point, as `COLLATE "C"` does, and not by UTF-16 code unit, which put an emoji below U+E000 to U+FFFF. A filter value is NFC before it is compared, as the door makes it. The guard is `clients/ordo-sdk/test/refine-parity.test.ts`. It reads the case table `crates/ordo-conformance/fixtures/refine-parity.json`, which `crates/ordo-conformance/tests/refine_parity.rs` runs through the real read path on Postgres, so the expected answers are proved on the server. Building it found two server defects, fixed in `ordo-plan`: a text `<`, `<=`, `>` or `>=` in a condition used the database's default collation and not `COLLATE "C"`, and `x in (..., null)` did not match a null `x`, against the settled rule that `in` is a chain of `==`. The generated table now carries `refine: { tier, fields }`, projected from the descriptor's `ops[].refine`, which is what makes a local unknown-field refusal possible at all.

**Three deliberate differences from the door**, each because the reason for the server's rule does not reach the device. **Sorting is local only**: the envelope has no sort key and never has, because ordering belongs to the op's compiled query and `ordo-api/src/page.rs` depends on refinement not touching it, since a cursor addresses a row by sort key. Sorting rows already on the device moves no cursor. **Sealed-field rules do not apply**: they exist to stop an oracle over values the server holds encrypted, and the device only ever receives values the executor already opened. **`refine.tier` is not consulted.** Since 2026-10-05 that is no longer a disagreement: every program read is tier `basic`, so the device and the door accept refinement on the same ops.

*What building it found.* Tier-0 refinement is **specified, validated, cost-gated and served by nothing.** `interface.rs:550` hardcodes `tier: "none"` for every op, so the door refuses `$refine` outright across the whole catalog; `@ordo/react` can build an envelope and will never send one; and `ordo-host`'s executor takes a validated refinement and ignores it, which its own test says in as many words (`crates/ordo-host/tests/cost_class.rs:460`: "the executor does not yet APPLY `$refine`"). None of that is new breakage, and all of it was known in pieces. What is new is that C6 is now the **first working implementation of Tier 0 anywhere in the system**, and it runs on the device. That is why the tier question went on the board as `refine-tier-local`.

*Answered 2026-10-05.* The owner ruled refinement a standard feature: every program read is refinable, with no off switch, and the door caught up with the device (`plan-refinement.md`, R1 to R6). The door now applies `$refine` in the plan, `cost_class.rs` proves the filter, and `@ordo/react` sends refinements with no local tier refusal. The paragraph above is kept as what the build found at the time.

**C7 · Purge and revocation. [device half built 2026-09-16]** Poisoned at next contact, sign-out purge, refusal purge, `policy mirror` scope and eviction.

*As built* (`clients/ordo-sdk`). Three of the four are done, and the fourth has no producer to build against.

**Poisoned at next contact.** `LocalReplica.poison()` purges and latches the instance: every read afterwards answers empty and every write is refused. The latch is on the instance because `purge()` deletes the keys as well as the records, so a purged replica is dead rather than empty — without the latch a later read would open a freshly keyed store and look like an ordinary miss. `implementation.md` says the client "purges before serving any read", and the latch is where *before* is enforced. **A test caught the half of this the design missed:** gating reads alone left a poisoned replica answering empty while still sealing new records to disk under keys held in memory, so the latch covers `put`, `remove`, `hold` and `release` too. Poisoned means it serves nothing **and** stores nothing.

**A refusal purges what that op synced.** A refusal in `access_denied`, `access_guard` or `delegation_required` releases everything that op held, at its last position, through the path `drop` already used. Rows another synced op still holds survive, because a row leaves only when no op holds it. The set is deliberately narrow: `not_found` and `unknown_op` are excluded, or an op renamed in a deploy would throw away rows the caller is still entitled to.

**Never mirrored.** `syncOp` refuses `std.space.*` before the wire. The permission map is syncable *by shape* — stored rows of one entity, with an id — so nothing upstream stops it, and a stolen device must not yield it. Command-log slices are the rule's other half and have no client op to name today.

**Sign-out purge** was already `purge()` from C3, and stays distinct from poisoning: one is the person leaving, the other is access being taken away.

*Not built, because it has nothing to enforce.* **`policy mirror { keep 90d | keep <n> rows | full }` scope and eviction.** The keyword `policy` is lexed and never parsed: there is no parser arm, no AST node, and no consumer in `ordo-schema` or `ordo-plan`. That is true of **every** policy block the language documents — `spaces`, `commandlog`, `vault`, `llm-egress`, `retention`, `compliance` — not only `mirror`. So mirror scope would be the first policy Ordo ever parses, which is a language increment rather than a device one, and eviction built against a policy nobody can write would be a dial no deployment can set. On the board as `policy-grammar`.

*Also not C7, by the plan's own boundary:* the server-released key, leases and grace windows are 5g-rev. C7 is the honest-online-client half, which G16 already delivers free, because an acl write invalidates every guarded live query touching it.

**C8 · `@ordo/react` over the replica.** Hooks that render local rows at once and mark them stale while the feed catches up.

**C9 · The record.** The two amendments are done (2026-09-14): `bootstrap.md` input 33 is on by default, and `definition-language.md`'s mirror sentence describes op-scoped sync. The rulings are in `corpus-rulings.md` and the roadmap. Remaining: a note in `f7-vault-plan.md` that the vault's store inherits this one's shape.

---

## 8 · Questions for the owner

**Q-1 · Cache answers, or replicate rows?** Ruled 2026-09-14: replicate rows.

**Q-2 · Where the cache lives, and whether it is on by default.** Ruled 2026-09-14: IndexedDB, on by default.

**Q-3 · The at-rest key for v1.** **Ruled 2026-09-14:** a device-local key now, shaped so the passkey-PRF wrap (`implementation.md`) and the server-released key (5g-rev) slot in.

**Q-4 · What a device may receive: (a), (b) or (c) of section 2?** **Ruled 2026-09-14: (c), op-scoped sync, with (a) as its special case.**

**Q-5 · How the device answers new questions in v1: level 1, 2 or 3 of section 4?** **Ruled 2026-09-14: level 1 now, level 3 (PGlite) with F7.**
