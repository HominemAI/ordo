# Offline writes and local answers: a top-level plan

**Filed for after the first deployment.** The owner, 2026-09-29: "Yeah let's do a brief top level plan and file it for post deploy." Nothing here is queued to build. Three things the device does not do today, and what it would take:

| # | The gap today | Where it is ruled today |
|---|---|---|
| 1 | A write made offline in a server-managed space is refused, never queued | `plan-client-cache.md` §6; `clients/ordo-react/src/hooks/useCommit.ts:66-82` |
| 2 | A write in a client-first (vault) space has no device evaluator to apply it | `f7-vault-plan.md` Phase 4; checklist 5g, 18h |
| 3 | A count, total or grouped read always goes to the network | `crates/ordo-plan/src/artifact.rs:1190-1236` (`SyncRule::Measure`, `Grouped`) |

The owner's words on 1: in mixed and client-first sovereignty, offline writing "would be a good feature and at times expected". Two principles from the same day govern every choice below: "What is enforced is app design ... not our design" and "We do not enforce an app developer's choices but we can have it as default" (`corpus-rulings.md:9456`, `9472`), and limits are layered, never one hardcoded system value (`corpus-rulings.md:9549`, checklist 24).

Custody names, so the sections read exactly: `managed` and `byo-storage` are server-managed, and the owner's "mixed" is a managed space with sealed fields. `client-first` is the vault (`definition-language.md:3037-3041`).

## 0 · Where it stands, 2026-10-10

Read again when ellanor's readiness was reviewed. Nothing in §1 to §3 is built. What is built is the ground they stand on.

**Built: the device copy** (checklist 5g-pre, `plan-client-cache.md`):

- Syncable reads are mirrored onto the device, sealed at rest, kept current, and answered from there. `useOpQuery` says where rows came from (`source`, `deviceFallback`, `complete`).
- Refinement runs on the device over synced rows with the server's operators (C6, db6e1ca).
- A revocation purges the device copy and poisons the replica; sign-out clears it (C7, d4e756a).
- The permission map, `std.space.*` reads and acl access lists are never mirrored. `device: false` opts a read out.
- A write that loses a compare-and-set lands in the conflict state (`useCommit`), which §1 reuses for a drained outbox.

**Not built, and what each waits on:**

| Piece | Waits on |
|---|---|
| §1, offline writes in server-managed spaces | Nothing but a go. It does not need the vault. |
| §2, offline writes in client-first spaces | F7 (checklist 5g): not one line built, nine phases, Phase 4 (the deterministic evaluator) a third of the work. |
| §3, counts and grouped results on the device | Nothing but a go for the server-managed case. |
| Multi-device sync for vault spaces (18h) | F7. No mechanism tells one device that another changed something. |
| PGlite, the same SQL on the device | F7, as `f7-vault-plan.md` recommends. |
| Revocable caching for a shared window (5g-rev) | The server-side content-key release, which waits on share registration (11-2, still owed). |
| Staged load: the first screen fast, the rest after | Ruled 2026-10-10 the app developer's design, not Ordo's. Ordo owes a guarantee that the client never blocks the main thread, an audit of where it could, and a guide (board `staged-load`). |

**The question this raises.** Whether an app's data must be client-first decides which track it takes. A server-managed app gets offline writes from §1 alone, without waiting for the vault. A client-first app waits for F7 (§2). For ellanor this is asked on the board (`ellanor-readiness`).

---

## 1 · Offline writes in server-managed spaces

**What the person sees.** They edit a row or add one with no network. The screen shows what they typed, marked pending. When the network returns the write goes to the server on its own. It lands, or it comes back as a conflict they resolve, or as a typed refusal. Nothing is lost silently and nothing is merged silently.

**The mechanism.** An outbox on the device. Each queued write holds the op, the params, the `rev` it read, and the idempotency key minted for it. On reconnect the outbox drains in order through the ordinary door: same op, same CAS, same key. The server stays the only evaluator. A `cas_miss` becomes the conflict state the runtime already has; a refusal stays a refusal. The replica never holds a pending row: pending values live beside it as an overlay the hooks show, and the server's row replaces the overlay when the write lands and the device reloads.

**What exists and is reused.**

| Part | Where |
|---|---|
| Every write carries the `rev` it read; a stale one is `cas_miss`, never an overwrite | `clients/ordo-react/src/commit.ts` (three rules), `crates/ordo-conformance/src/write.rs` `check_cas` |
| `cas_miss` as a conflict state with refetch and retry | `clients/ordo-react/src/hooks/useCommit.ts` |
| An idempotency key per attempt, a durable replay cache, `idempotency_mismatch` on a changed payload | `clients/ordo-sdk/src/core/client.ts:136`, `crates/ordo-api/src/router.rs:860-1089`, `crates/ordo-host/src/replay.rs` |
| Synced rows sealed at rest, per space and principal, purged and poisoned on revocation | `clients/ordo-sdk/src/replica/replica.ts`, `seal.ts` |
| After a commit the device asks the server again | `clients/ordo-react/src/device.ts` `afterCommit`, `reload` |

**What is new.** The outbox store, sealed like replica records. The drain and its per-item outcome. The pending overlay in `useOpQuery` and `useOpPages` (`source: 'pending'`). A local handle for a row created offline, re-keyed by the server's id when the create lands (ids are server-minted in managed spaces). A setting that turns the queue on or off.

**The hard part.** Without an evaluator the device cannot compute the row a write produces: guards, `check`, computed fields, converge-or-create all run on the server, and a commit returns a `write_summary` without the row (`crates/ordo-plan/src/artifact.rs:638-668`). So the overlay is only what the person typed, never a derived value, and that is what keeps this section clear of F7 Phase 4. Two limits follow. A queued CAS write is stale by construction, so a queue held long is a queue of conflicts. And the replay window is 24 hours (`crates/ordo-api/src/idempotency.rs:136`), after which a drained item is a new request; the queue's lifetime must sit inside it.

**Increments.**

| | Increment | Done when |
|---|---|---|
| W1 | The outbox store | A write made offline is on disk, sealed, survives a reload, and a raw dump shows no op, param or key |
| W2 | The drain | On reconnect items go in order with their original keys; each ends `committed`, `conflict` or `refused`; one conflict stops nothing else |
| W3 | The pending overlay | A list shows the typed values marked pending; the server's row replaces them on commit; a refused item leaves the overlay with its refusal |
| W4 | Creates offline | A create shows under a local handle and is re-keyed by the server's id when it lands |
| W5 | The setting | `offline: queue \| refuse` per provider and per op, layered (deployment default, app developer sets, exposable to a tenant); with `refuse` the hook behaves exactly as today |
| W6 | The record | `plan-client-cache.md` §6 and the `@ordo/react` README say what a device does with an offline write |

---

## 2 · Offline writes in client-first (vault) spaces

**What the person sees.** The same four states as §1: pending, committed, conflict, refused. The difference is underneath: the device applies its own write at once, because the device is the store.

**The mechanism, as F7 already designs it.** A write is a device-signed proposal `P` carrying `believed_prev`; the platform orders it without reading it and signs a receipt `R`; every device folds the canonical order with the deterministic evaluator, and a conflict is a canonical no-op with a recorded outcome (`f7-vault-plan.md` §2.2, Phase 4). No CRDT, no automatic merge: conflicts resolve by rejection and replay (§9). Multi-device liveness is 18h (form 2, ruled). **This plan does not redesign any of it.**

**What this plan adds.** One vocabulary and one store shared with §1, so a hook cannot tell the custodies apart:

| §1, managed | §2, vault |
|---|---|
| The `rev` the write read | `believed_prev` in `P`, and the fold's CAS precondition |
| The idempotency key | `(device_id, counter)` in `P` |
| `cas_miss` from the server | The fold's recorded outcome for that proposal |
| The outbox of unsent writes | The queue of unsent proposals |
| The pending overlay | Unneeded: the device folds its own proposal and shows the result |

**What exists.** The signed record shapes and the build order (`f7-vault-plan.md` §2, §6); the replica the vault store inherits (`plan-client-cache.md` C9); a device-local at-rest key shaped for the passkey wrap (C3).

**What is new.** Nothing beyond F7's phases and 18h. The outbox (W1) is built once and becomes the proposal queue.

**The hard part.** Phase 4, the deterministic evaluator, 6 to 10 weeks and all of F7's schedule risk. Nothing in this plan shortens it, and nothing in §1 or §3 waits on it.

**Increments.**

| | Increment | Done when |
|---|---|---|
| V1 | The shared commit contract | `useCommit` state and the outbox types cover both custodies with no custody branch in a hook |
| V2 | The outbox as the proposal queue | After F7 Phase 3, an unsent proposal is the same sealed record as W1 with `counter` monotonic per device |
| V3 | The device folds its own proposal | After Phase 4, a write shows its folded result at once and the receipt confirms its position |
| V4 | Rejection and replay | A proposal that folded to a no-op is shown as a conflict with the recorded outcome, and replay needs the person, never a retry loop |
| V5 | Other devices | 18h: a second enrolled device sees the write when the platform announces the new position |

---

## 3 · Counts and grouped results on the device

**What the person sees.** A count tile or a totals row answers offline and changes the moment the device's rows change. Today 17 of the 76 reads in the market fixture are `measure` and 3 are `grouped`, and all 20 go to the network (`crates/ordo-cli/tests/golden/market.interface.json`; 55 reads are syncable).

**The mechanism.** Coverage. A measure `M` is answered locally when a synced read `R` holds every row `M` covers: same anchor, `R` completely loaded, `R`'s filter implied by `M`'s. The device then evaluates `M`'s aggregate over `R`'s rows with the language's own semantics. When coverage fails the hook goes to the network and says so (`deviceFallback: 'uncovered'`). Counts first; grouped reads are the same evaluation with group keys producing synthetic rows.

**What exists and is reused.**

| Part | Where |
|---|---|
| The planner already classifies each read: `measure`, `grouped`, the anchor, `single_row` | `crates/ordo-plan/src/artifact.rs:509-542`, `1190-1236` |
| The device evaluates Tier-0 conditions over held rows, and knows when a load is complete | `clients/ordo-sdk/src/replica/refine.ts`, `manager.ts` (`complete`, `position`) |
| The semantics to match, stated in one place | `howto-values.md`: two-valued nulls, `COLLATE "C"`, NFC on write, decimal scale 12 half-up |
| The constants | `crates/ordo-plan/src/lib.rs:52` (`DECIMAL_SCALE = 12`), `:111` (`TEXT_COLLATION = "C"`), `crates/ordo-exec/src/nfc.rs` |
| Rows arrive typed: `int` and `money` as integers, text and dates as strings, NULL as null | `crates/ordo-exec/src/lib.rs:1426` `canonical` |
| Exact decimal comparison on the client, BigInt-based | `clients/ordo-sdk/src/core/client.ts:417-430` |

**What is new.**

- **A local recipe on the artifact and the descriptor.** Neither carries a measure's filter or aggregate in a form a client can evaluate; the only form is the planned SQL (`artifact.rs:476-499`), and `interface.json` carries none. The recipe is the anchor, the filter as a closed predicate (the Tier-0 set, AND-only, over the anchor's fields and the op's params), the aggregate (`count`, `sum`, `min`, `max`, `avg` over one field) and, for a grouped op, the keys. A measure outside that closed set gets `local: null` with the rule it fails, the way `syncability` refuses today. The gate is the honest part.
- **The coverage check** on the device: `covers(R, M)`. v1 accepts two cases: `R` is the anchor's unfiltered list, or `R`'s recipe conjuncts are a subset of `M`'s.
- **The aggregate evaluator**, with the language's rules and a mandatory parity test.
- **Grouped rows** from the same evaluator, ordered by key in `"C"` order.

**The hard part.** This is a second evaluator, the thing `f7-vault-plan.md` §1.2 and `plan-client-cache.md` §4 warn against, and the owner ruled level 1 now and level 3 (PGlite) with F7. The case for a bounded exception: the recipe set is closed and small, anything outside it is refused before evaluation, and the managed executor is the oracle in a test that runs every measure in the fixture both ways over the same rows. Two divergences are already visible and show why the test is mandatory. The device refiner's `ne` keeps SQL's three-valued null logic (`refine.ts:142-144`) while the language is two-valued (`null != 5` is true); a count must use the language's rule. And `refine.ts:160-169` orders text by UTF-16 code unit while `"C"` orders bytes; they disagree outside the Basic Multilingual Plane. `avg` needs scale-12 half-up division in BigInt on the device.

**Increments.**

| | Increment | Done when |
|---|---|---|
| A1 | The recipe in the artifact and `interface.json` | Every measure and grouped read in the market fixture carries a recipe or names the rule it fails; the artifact digest moves once |
| A2 | Coverage on the device | A count over a completely loaded `std.E.list` answers locally; a partial load or an unsynced anchor goes to the network with `deviceFallback` |
| A3 | `count`, with the oracle test | Every count-class measure in the fixture agrees with the server over rows that include nulls and NFC-equivalent text |
| A4 | `sum`, `min`, `max`, `avg` | Same test; `avg` agrees to twelve places; `min`/`max` on text agree in `"C"` order |
| A5 | Grouped reads | The fixture's grouped reads agree, rows in key order, and stay synthetic (no id, no actions) |
| A6 | Live | A local count re-evaluates on every replica change, with no network, through the manager's `watch` |

---

## 4 · How the three depend on each other

| Section | Needs | Gives |
|---|---|---|
| §1 managed offline writes | 5g-pre (built), 14b (built). Nothing from F7 | The outbox and the commit contract to §2; a pending write can raise a local count in §3 by one |
| §2 vault offline writes | F7 Phases 1 to 4, then 18h; §1's outbox | Nothing back; it consumes |
| §3 local counts | 5g-pre (built); A1 is a planner change. Nothing from F7 | Counts that reflect §1's pending rows; retired in vault spaces once PGlite runs the same SQL on the device |

Recommended order: A1 to A3, then W1 to W3, then A4 to A6 and W4 to W6, then V1 to V5 as F7 lands. §3 comes first because A1 changes the artifact, which is cheapest before any deployment holds one.

---

## 5 · What must not break

- **Row-bound seals.** The device holds only values the executor already opened. The outbox holds what the person typed, sealed under the device key like every replica record, never in plain storage.
- **CAS.** An outbox item carries the `rev` it read. The drain never turns a `cas_miss` into a retry, and there is still no last-write-wins path.
- **Revocation purges the replica.** `poison()` also empties the outbox: a revoked person's queued writes are dropped and the hook says so. A refusal that releases an op's rows also drops its pending overlay.
- **The permission map is never mirrored.** No recipe covers `std.space.*`; a count over members stays on the network, and `syncOp`'s refusal keeps standing.
- **Never an older answer.** A pending overlay is not a replica row. It carries no position and never moves one.
- **One truth per value.** The server is the oracle for every aggregate the device answers; the parity test is part of each increment, not a follow-up.
- **Layered settings.** The queue switch, the queue's lifetime and the replay window are settings at the deployment layer with app developer and tenant layers above them, never a constant (`settings.md`, checklist 24a).
- **The artifact is custody-independent.** The recipe is the same bytes at every stop on the dial (`f7-vault-plan.md` §1.3).

---

## 6 · Ruled 2026-10-01

All six asks on the board (`offline-plan`) took the recommendation, as
listed below, plus a sixth: offline, the last total the server gave may be
shown, labelled with its time, as the app developer's setting. The owner's
reminder: this is a library, so the app developer decides what to display
and Ordo makes the information available. Recorded in `corpus-rulings.md`
(2026-10-01). Still after first deployment (checklist 18ad).

The questions as they were put:

1. **Queue offline writes by default wherever the device copy is on, or refuse by default with the queue as a switch?** Recommend: **on where the device copy is on**, off per op by the app developer; the device copy is already on by default, and the owner said offline writing is "at times expected".
2. **How long does a queued write live before it is dropped and the person told?** Recommend: **the replay window, 24 hours by default**, as one layered setting; past it a drained write is a new request.
3. **What a pending write shows: the typed values marked pending, or nothing until it lands?** Recommend: **the typed values, marked**; nothing derived, because the server is the only evaluator.
4. **Accept a bounded second evaluator for counts (closed recipe set, mandatory parity test), or wait for PGlite with F7?** Recommend: **accept, bounded**; managed spaces will never carry the PGlite payload, so without it counts never answer offline there.
5. **Order: counts before offline writes?** Recommend: **yes, A1 to A3 first**, because A1 changes the artifact and that is cheapest before anything is deployed.
