# Ordo production operations — rotation, restore & metering: design & build plan

Three tracks, one document. They are the operational trio a hosted Ordo
cannot ship without, and they are grouped here because each one is a
*mechanism the spec already settled and nothing built*:

| Track | The gap, in one line | Spec that settled it |
|---|---|---|
| **K** — key rotation & the re-seal worker | `rotate()` mints a new epoch and nothing ever re-seals, so a rotation forced by a revocation rotates to a key that still opens everything | `implementation.md`, "Key custody & DEK rotation" `[settled]` |
| **R** — restore & DR | nothing appends `system:restore`, and `verify` has no concept of a discontinuity | `implementation.md`, "Backup, restore & disaster recovery" `[proposed]` |
| **M** — metering & billing substrate | nothing anywhere counts usage, per space or otherwise | nothing — this is the one track with no prior spec |

Numbering is `K1–K3`, `R1–R3`, `M1–M3` for `roadmap-checklist.md` §17.
This document adds nothing to the checklist itself.

Every code claim below is grep-verified against the tree at `89b1d1d`, and
every claim carries its file and line. Where a document and the code
disagree, the code is quoted and the document is named. **Tracks R and M
were re-verified against `a2cdfe6` on 2026-09-19**; each opens with a table
of what moved, and their line numbers are `a2cdfe6`'s. Track K and §0's
table still cite `89b1d1d` except where a bullet says otherwise.

---

## 0 · The starting state, verified

What is already built and can be ridden, before any of the three tracks
proposes machinery:

| Already there | Where |
|---|---|
| `Purpose::Reseal` — the lease variant a re-seal worker takes, named separately because it needs two epochs live at once | `crates/ordo-crypto/src/custody.rs:135-142` |
| `KeyProvider::rotate` / `destroy_epoch` / `destroy_space`, and a `LocalKeyProvider` behind them | `crates/ordo-crypto/src/custody.rs:341-397` (trait), `:686-717` (impl) |
| `$sealedpayload` with an `("$space","epoch")` index, put there *for* the horizon count | `crates/ordo-crypto/src/sealed.rs:96-107`, argued at `:60-67` |
| `sealed::epoch_count_sql` and `exec::sealed::epoch_payload_count` — the number `destroy_epoch`'s caller owes | `crates/ordo-crypto/src/sealed.rs:208-217`, `crates/ordo-exec/src/sealed.rs:163-178` |
| `EventKind::CustodyAction { action }` — where a custody decision is recorded, with the exit ops as the worked precedent | `crates/ordo-secevent/src/event.rs:402`, `crates/ordo-host/src/exit_ops.rs:24-36` |
| `verify`'s anchor machinery, and `VerifyFailure::PrefixMissing` — the one existing "this break may be lawful" concept | `crates/ordo-exec/src/undo.rs:329-341`, `:384-386` |
| Truncation detection: a stored head below a signed checkpoint's `upto` is already caught | `crates/ordo-exec/src/undo.rs:499-503` |
| The export bundle, deterministic and byte-identical across runs | `crates/ordo-exec/src/export.rs:50-62` |
| `AppendObserver` — a per-space observation seam on the write choke point, with no dependency inversion cost | `crates/ordo-exec/src/metrics.rs:38-55`, attached at `crates/ordo-exec/src/lib.rs:336-344` |
| `claim_schedule` — a *named*-schedule claim, built in 5d precisely so a second cadence cannot steal the first's period | `crates/ordo-exec/src/jobs.rs:170` |
| The frozen chain encoding's growth precedent: a trailer emitted **by presence** | `crates/ordo-commandlog/src/chain.rs:39-63`, `:240` |

And what is *not* there, stated once so no increment below quietly assumes it:

- **No production caller of `rotate()`.** Grep for `rotate` outside
  `custody.rs` finds tests (`crates/ordo-exec/tests/sealed.rs:620`,
  `crates/ordo-crypto/src/lib.rs:282`) and one forwarding decorator
  (`crates/ordo-host/src/metrics.rs:954-960`). `AccessEngine::suspend`
  (`crates/ordo-identity/src/access.rs:479`) flips a status and says in its
  own doc that opening a new epoch "is not reachable from here". So
  "rotation is mandatory on every membership revocation" is not merely
  ineffective today — it does not fire.
- ~~**No blob storage.**~~ **Built since this section was written**
  (`plan-blob.md` B1–B5, checklist 17g): the field types, the store
  abstraction with two bindings, and the chunked seal
  (`ordo_crypto::stream`). The export manifest's `unbuilt` list — cited
  here as the disclosure shape the tracks below reuse — is now **empty**,
  which does not weaken the citation: the shape is the point, and it is
  still the machine-readable place a gap gets declared.

  What this section's own K1 gains from it is a **fourth population**; see
  §1.3, which was written as three and is now four.
- ~~**No worker supervisor.**~~ **Built since this section was written**
  (`plan-runtime.md` R3, checklist 17d, finished 2026-09-18):
  `crates/ordo-host/src/supervisor.rs` and `workers.rs`. K1 and M1 ride it,
  and each has a registered `Slot` (`workers.rs:449-473`) whose census
  reason names this plan; neither track builds a scheduler.
- ~~**No host config struct and no server binary.**~~ **Built since this
  section was written** (`plan-runtime.md` R1 and R2, checklist 17a and
  17b): `ordo-host` is a binary (`crates/ordo-host/src/main.rs`),
  `RuntimeConfig::from_env` is its one environment reader, `assemble` is
  the assembly and `BootRefusal` its refusal type (`runtime.rs`). Custody
  is `PgKeyStore` under a master from `ORDO_MASTER_KEY` or a sealed file.
  Any toggle proposed below still takes the shape
  `Metrics::with_per_space_series` set (`crates/ordo-host/src/metrics.rs:336`):
  a builder for deployment control, an env default, never a label a missing
  variable adds.

---

## 1 · Track K — key rotation and the re-seal worker

**Owner ruling (2026-09-07).** Envelope encryption it is — with built-in
ability to force re-encrypt after exposure. The owner confirmed this
document's envelope architecture (wrapped DEKs under an SMK, not derived
keys) as the settled shape, and put the forced re-seal command — the
exposure-response operation this track exists to build — in scope for K3's
operator surface below, alongside `status`, `rotate` and `destroy`.

### 1.1 What rotation does today

`LocalKeyProvider::rotate` (`crates/ordo-crypto/src/custody.rs:686-700`)
mints a random `DEK_{n+1}`, wraps it under the SMK, stores it, and returns
`n+1`. `current_epoch` is `max(epoch)` over the stored DEKs
(`:677-684`), so the new key becomes the write epoch by arithmetic. Reads
open under whatever epoch the envelope's leading byte names
(`open`, `:645-655`). Nothing else happens.

That is correct as far as it goes — `rotate` is documented as not
re-sealing (`custody.rs:344-349`) — but it means the state after a
revocation-forced rotation is: **every existing payload is still openable
under the key the revoked principal may hold.** Post-compromise recovery is
the whole reason the epoch scheme is wrapped-not-derived
(`custody.rs:27-40`), and it is not reachable without the pass.

### 1.2 The ruling this reading forced

`security-logging-design.md` §7 pins:

> A `SealedRef` resolves only while the epoch key that sealed the payload
> still exists, and the chain hashes the ref, so a re-seal pass **cannot**
> rewrite historical before/after images: rewriting one would change the
> group's canonical encoding and break the chain from that position
> forward.

Both halves of that sentence are true, and the conclusion drawn from them
across two documents is not. The chain hashes the ref — sixteen bytes,
`Value::SealedRef(h) => out.extend_from_slice(h)`
(`crates/ordo-commandlog/src/chain.rs:291`). The payload bytes are never in
the canonical encoding. So re-sealing a `$sealedpayload` row **in place** —
same `("$space","ref")` primary key, new envelope in `payload`, new `epoch`
— rewrites no image, changes no group encoding, and moves no hash.

The handle exists to be a stable indirection. Using it as one is not a
loophole; it is the property the random-handle design was chosen for.

**Ruling (2026-09-08):** the re-seal pass covers historical payloads as
well as live rows. The pinned rule — *an epoch key may not be destroyed
while any command image within the space's `undoHorizon` is tagged with it*
— **stands unchanged**. What changes is that "tagged with it" is a property
of `$sealedpayload`, which a worker can drive to zero, so the gate becomes a
state the deployment can *reach* rather than only a clock it waits out.
Recorded in `corpus-rulings.md`.

The honest cost, and it is the one the confirmation must print: re-sealing
the live database cannot reach a **backup or an archived log bundle**.
`implementation.md` already says destroying a retired epoch key stops those
opening. That stays true, and K2's confirmation states it as a date and a
consequence rather than as prose.

### 1.3 The four populations

A re-seal pass over epoch `n → n+1` touches exactly these, and the plan
enumerates them because a pass that misses one leaves the gate permanently
unsatisfiable:

1. **Live entity-table columns.** A sealed field is `bytea`
   (`crates/ordo-schema/src/ddl.rs:78-81`) holding
   `[epoch][nonce][ct‖tag]`. The predicate for "still under the old epoch"
   is exact and index-friendly: `get_byte("field", 0) = n`.
2. **`$sealedpayload` rows** — the command images' payloads, one per sealed
   image write, never deduplicated, two per converge
   (`crates/ordo-crypto/src/sealed.rs:24-43`). Scanned on the existing
   `$sealedpayload_epoch` index.
3. **Blob chunks** — ~~none exist. Named, counted as zero, and declared in
   the pass report's `unbuilt` list the way the export manifest declares
   it.~~ **They exist now** (`plan-blob.md` B1–B4), and there are two of
   them rather than one, which is why the count in this heading is wrong
   and is corrected below.

   An external blob's object is a chunked STREAM envelope
   (`ordo_crypto::stream`) whose **header carries the epoch byte** in the
   same leading position the single-shot envelope uses — deliberately, so
   the re-seal worker can find an object's epoch without opening it and
   without a listing. Re-sealing one is a read-decrypt-encrypt-write of a
   whole object, not a column update, so unlike populations 1 and 2 it is
   **not** CAS-able: the object key is keyed-random and never reused, so a
   re-seal mints a **new** handle, writes the new object, updates the
   `$blobref` row and the entity column in one transaction, and leaves the
   old object to `blobgc`'s ordinary floor. That is the correct shape
   rather than a workaround — an in-place object overwrite is exactly what
   `BlobStore::put`'s `AlreadyExists` refusal exists to prevent.

   A `blob inline` payload is **not** a fourth thing: its bytes are a
   `$sealedpayload` row, so it is already inside population 2 and is
   re-sealed by the same pass with no extra machinery.

4. **`$blobref` metadata envelopes** — `mime`, `size` and `integrity`, one
   set per stored object, sealed under the same DEK and carrying the same
   leading epoch byte. Scanned on the existing `$blobref_epoch` index,
   which is there for this. They are small, they are CAS-able exactly as
   population 1 is (every seal draws a fresh nonce, so the old envelope
   bytes are unique), and they must not be forgotten: an epoch with zero
   objects left and a live `mime` envelope is an epoch the gate would let
   an operator destroy, and destroying it would make a blob's *metadata*
   permanently unopenable while its bytes still read.

   `ordo_crypto::blobref::epoch_count_sql` is the counter, and its doc
   names this section.

So the gate counts **four** populations, and a pass that misses either of
the two new ones leaves it permanently unsatisfiable in the way this
section exists to prevent.

Blind-index columns are **not** a population. `BIK` is SMK-derived and
epoch-independent (`custody.rs:657-675`), which is the settled rule that
makes a DEK rotation cheap. A re-seal moves no digest, so `sealed indexed
unique` holds continuously across the pass — the exact failure
`implementation.md` deleted the per-epoch derivation to avoid.

### 1.4 Concurrency: why this needs no write barrier

A transforming schema backfill needs a freeze because a concurrent write to
a row behind the cursor lands only in the old shape. A re-seal does not,
and the reason is worth stating rather than discovering:

**Writes always seal under the newest epoch** (`seal`,
`custody.rs:628-643`). So a row a user writes during the pass is *already*
in the state the pass wanted. The only hazard is the pass clobbering that
newer write with an older value it re-sealed from a stale read.

That is closed by a compare-and-set on the ciphertext itself:

```sql
UPDATE "<entity>" SET "<field>" = <new envelope>
 WHERE "$space" = … AND "id" = … AND "<field>" = <old envelope bytes>
```

Every seal draws a fresh random nonce
(`security-logging-design.md` §7, and `crate::seal_with`), so the old
envelope bytes are unique to that one write and the CAS is exact. Zero rows
updated means the row moved; the pass skips it and counts it, because the
row is already current. No version column, no advisory lock, no freeze.

`$sealedpayload` rows are insert-only — the write path never updates one
(`sealed::insert_sql` is "an INSERT, never an upsert",
`crates/ordo-crypto/src/sealed.rs:152-159`) — so there is no race there at
all.

### 1.5 What "verified" means for a chunk, and the digest that would be an oracle

`f7-vault-plan.md` §5.1 pins the shape of a conversion job: *"chunked
streaming re-encryption, progress checkpointed per chunk, verified by count
plus content hash per entity, nothing deleted until the target copy
verifies."* Re-seal is that machinery, with one substitution that is not
optional.

**A content hash over plaintext must not be persisted.** A durable digest
of a sealed value is an unsalted, permanent equality index over that field
— precisely the oracle §7 rejects the content-addressed `SealedRef` for,
and it survives crypto-erasure for the same reason. So:

- **Correctness is checked in memory, per row, and discarded**: open under
  `n`, seal under `n+1`, open the result, compare bytes, drop both. A
  mismatch aborts the chunk.
- **The durable chunk checkpoint records** the row count and a SHA-256 over
  the concatenated `(ref ‖ new-envelope-bytes)` of the chunk. Because every
  new envelope carries a fresh nonce, two rows holding equal plaintext
  contribute unrelated bytes; the digest proves a chunk was processed and
  proves nothing about what it contained. That is the property that makes it
  storable.

### K1 — the re-seal worker

A per-space, chunked, checkpointed, resumable pass, running on the worker
supervisor `plan-runtime.md` R3 defines. Not a new scheduler.

- **State table `$reseal`**: `(space, from_epoch, to_epoch, population,
  cursor, rows_done, rows_skipped, chunk_digest, attempts, state, started_at,
  finished_at)`, `PRIMARY KEY (space, from_epoch, population)`. It carries
  the cursor, attempt count and dead-letter the `$jobs` table does not
  (`$jobs` is `(id, kind, payload, claimed)` — `crates/ordo-exec/src/jobs.rs:104-115`).
- **One key lease per chunk, not per pass**:
  `release(space, Purpose::Reseal, correlation)` where the correlation names
  the `$reseal` row. A lease per chunk bounds how long two epochs are
  resident, which is the exposure `implementation.md` names as the reason
  the re-seal purpose is separate at all.
- **Populations in order**: `$sealedpayload` first (it is the population the
  horizon gate counts), then live columns per entity, per sealed field, keyset-paged on
  `("$space","id")` — never `OFFSET`, for the reason `export.rs:116-119`
  gives.
- **Throttled**: a chunk size and an inter-chunk delay from the schedule
  row, so a rotation cannot starve the deployment. `implementation.md`
  requires this in the same breath as "resumable".
- **Recorded** as `EventKind::CustodyAction { action: "epoch_rotated" }` at
  the rotation and `"reseal_completed"` at the end, in the security-event
  stream — the half of the spine that records decisions about a space, per
  the exit-ops ruling (`crates/ordo-host/src/exit_ops.rs:24-36`). A re-seal
  writes no command-log group: it changes no row's value, and forging a
  `Converge` with equal images to say so would put a lie in the chain.
- **Wired to the trigger that mandates it**: a membership suspension enqueues
  a rotation for the space. `AccessEngine::suspend`'s doc already owes this
  (`crates/ordo-identity/src/access.rs:466-471`).

**Done when** a revocation on a seeded space enqueues a rotation, the pass
runs to completion across a kill-and-resume, and
`epoch_payload_count(space, old) == 0` with no live sealed column carrying
`get_byte(col,0) = old`.
And when, after `destroy_epoch(old)`, **every** payload written before the
revocation refuses to open — asserted per row against the seeded fixture,
not sampled — while every value read through the ordinary path is byte-identical
to what it was before the pass.

### K2 — retirement, destruction, and the horizon gate

Retirement and destruction stay two events, as §7 pins.

- **Retired** = the K1 pass reports zero remaining references in all three
  populations. A retired epoch is no longer offered for `seal`; it is still
  accepted by `open`, because a backup restored onto this deployment may
  hand it one.
- **Destroyable** = retired **and** the §7 gate holds. With history
  re-sealed the gate is discharged by fact rather than by clock, but it is
  still *checked*, and checked in the provider's caller as the trait
  requires (`custody.rs:352-359`): count `$sealedpayload` under the epoch,
  count live columns under the epoch, and resolve the space's
  `undo_horizon` (`crates/ordo-exec/src/cadence.rs:96`,
  `LogPolicy::DEFAULT` = 90 days at `:117`) to a position the way
  `prune::horizon_position` already does.
- **The confirmation prints the consequence, not a warning.** Three numbers
  and one sentence: payloads still under the epoch, the horizon date,
  and — the one nothing can undo — *"backups and archived log bundles taken
  before <date> will not open for sealed fields in this space."* This is
  `implementation.md`'s "rotate then retire = post-compromise recovery, at
  the cost of old archives", printed as the date it actually is.
- **Cross-track hazard, named:** a restore (Track R) from a backup older
  than a completed destruction returns payloads under a destroyed epoch.
  `Resolved::Destroyed` already renders that honestly
  (`crates/ordo-exec/src/sealed.rs:41-63`), and R1's backup contract states
  the ordering rule: **destroy an epoch only after a backup taken since the
  re-seal completed.**

**Done when** `destroy_epoch` is reachable only through a command that has
counted all four populations and resolved the horizon to a position, and
refuses by name — with the number that refused it — otherwise.
And when a test destroys an epoch on a seeded space and asserts
`std.E.history` renders the explicit unresolvable marker and `revert_group`
refuses with a typed rejection, never a silent empty value.

### K3 — the operator surface

`ordo keys` beside the existing `verify` / `checkpoint` / `revert` /
`apply` / `rollback` / `deploys` / `prune`
(`crates/ordo-cli/src/operator.rs:220-227`), same flag discipline, same exit
codes (`:30-47`).

- `ordo keys status --space` — current epoch, every live epoch with its
  three population counts, whether a pass is running and where its cursor
  is, and for each retired epoch the date the horizon clears.
- `ordo keys rotate --space --reason` — step-up gated like every other
  custody op, mints the epoch, enqueues the pass.
- `ordo keys destroy --space --epoch` — the K2 gate, with the confirmation
  above and an explicit `--confirm` token, mirroring the destructive-migration
  discipline.
- **The metric.** `ordo_reseal_rows_remaining{space, population}` — a gauge,
  the first in the registry (`metrics.rs` has counters and histograms and
  says so at `:57-88`), so K3 owes the gauge type. Plus
  `ordo_reseal_stuck_seconds{space}`: wall time since the cursor last
  advanced on a non-idle pass. **Stuck detection is the cursor, not the
  queue** — a job that is claimed and making no progress is invisible to a
  depth gauge, and a rotation that silently stops is a space whose gate never
  clears.
- **Per-space labels.** These two carry `space` unconditionally, unlike
  `ordo_op_duration_seconds`. That is defensible where the op histograms are
  not: the series count is bounded by spaces *with a live pass*, which is a
  handful, not by spaces × ops. The `MAX_SERIES` cap (`metrics.rs:169`)
  still applies and `ordo_metrics_series_dropped_total` still reports.

**Done when** an operator can answer "is this rotation finished, and if not,
where is it stuck" from `ordo keys status` and from the two metrics, without
reading a table.
And when a pass whose cursor has not moved for longer than its threshold
raises the stuck series while `ordo_reseal_rows_remaining` is non-zero —
proved against a deliberately wedged worker, not against a mock.

### Ruled out (K)

- **Re-seal as a command-log group.** It changes no row's value. The exit-ops
  ruling settles the shape: a decision about a space goes to the
  security-event stream.
- **A write barrier during the pass.** §1.4 — the CAS on the envelope makes
  one unnecessary, and a freeze on sealed fields during every revocation is
  the availability outage `implementation.md` already rejected in a
  neighbouring design.
- **A persisted plaintext content hash for verification.** §1.5 — it is the
  equality oracle §7 exists to prevent.
- **Re-seal on read ("lazy migration").** It never terminates, so the gate
  never clears, and the pass would hold a `Reseal` lease on the request
  path. Reads open under any live epoch; that is the whole point.
- **SMK rotation.** A different and rarer operation, and a real re-index with
  a real write barrier (`implementation.md`, "SMK rotation is therefore the
  index's rotation too"). Out of scope here; K makes DEK rotation work.

### 1.6 The build plan, verified against the tree (2026-09-19)

The section above was written against `89b1d1d`. Five of its claims moved
before anything was built, and each is corrected here rather than
discovered by the increment that trips on it.

| Claim above | State on 2026-09-19 |
|---|---|
| K1 is "wired to the trigger that mandates it: a membership suspension enqueues a rotation" | **There is no suspension op.** `AccessEngine::suspend` (`crates/ordo-identity/src/access.rs`) is an in-memory engine method with no production caller; grep for `suspend(` in `ordo-host` and `ordo-api` finds nothing. The trigger's *caller* does not exist, so K1 ships the function it will call (`reseal::enqueue_rotation`) and the operator command that calls it now (`ordo keys rotate`). Wiring the suspension op is that op's increment, and its doc already owes it. |
| K2's test asserts `std.E.history` renders the unresolvable marker | **There is no `history` op.** What exists is `sealed::resolve` returning `Resolved::Destroyed` and the revert refusing on it (`crates/ordo-exec/tests/sealed.rs::a_pruned_payload_and_a_destroyed_epoch_refuse_differently`). K2's test asserts through those two, which are the surfaces a history op would render from. |
| Population 3, blob objects: "mints a new handle, writes the new object, updates the `$blobref` row and the entity column in one transaction" | **Updating the row's `ref` breaks every retained image.** A revert resolves a blob column through `$blobref` by the handle the image carries, and a handle with no row is refused as corruption (`undo.rs::blob_digests`: *"names a blob handle with no `$blobref` row"*). The handle is the object's key, and `BlobStore::put` refuses an existing key, so an object cannot be re-sealed under its own handle either. The only shape that keeps every image revertable is: new handle, new object, a **new** `$blobref` row, the entity column moved by CAS, and the **old row and object left for `blobgc`** to collect once no live row and no retained image names them. Under that shape the object population clears by the blobgc floor, which is the horizon clock, not by fact. That is honest, it is what the plan already accepts for archives, and it is built last (K1d) because populations 1, 2 and 3 are what every space has and population 4 is what spaces with external blobs have. |
| "`$blobref.epoch`" is a population count | It is the **object's** epoch and stays so: `mime`/`size`/`integrity` carry their own leading epoch byte and are opened by it (`KeyProvider::open` reads the envelope), and nothing reads `BlobRef.epoch` except the counter. So the envelope population's predicate is the envelopes' own bytes, and the row's `epoch` column is the object population's. An epoch clears only when both are zero. |
| `get_byte("field", 0) = n` is the column predicate | A sealed column can hold **zero bytes** (`orders.memo` with the fixture's default; `tests/sealed.rs::memo_of` treats empty as none), and `get_byte` on an empty `bytea` is an error, not false. The predicate is `length(col) > 0 AND get_byte(col, 0) = n`. |

Two shapes the section left open, decided here:

- **The chunk's throttle is the worker's tick.** `Worker::run` "never loops
  internally", so a chunk is one `run`, `more` is always false, and the
  inter-chunk delay is `Cadence::Tick(pause)`. `ORDO_RESEAL_CHUNK` (default
  256 rows) and `ORDO_RESEAL_PAUSE_MS` (default 250) are the two knobs; an
  idle tick is one indexed `SELECT` on `$reseal`, leader-only.
- **Completion is a count, not a cursor.** A row sealed under `n` inside a
  transaction that committed after the chunk passed its keyset position is
  rare and real. When every population's cursor is exhausted, the pass
  re-counts; a non-zero population re-opens its cursor for a second sweep
  rather than declaring completion. The done-when's `count == 0` is
  therefore what the state machine checks, not what a test hopes.

The four populations, restated with their predicates and their per-row act:

| # | Population | Predicate for "under `n`" | Act | Cursor |
|---|---|---|---|---|
| 1 | `$sealedpayload` rows | `"epoch" = n` on `$sealedpayload_epoch` | open, seal, open, compare, `UPDATE … SET payload, epoch WHERE ref = … AND payload = <old>` | `ref` |
| 2 | live sealed columns (every `sealed` field that is not content) | `length(c) > 0 AND get_byte(c,0) = n` | same, `UPDATE "<entity>" SET c WHERE "$space" = … AND "id" = … AND c = <old>` | `(entity, field, id)` |
| 3 | `$blobref` envelopes | any of the three columns non-empty with leading byte `n` | open and re-seal all three, one `UPDATE` with a CAS on all three | `ref` |
| 4 | blob objects (`$blobref."epoch" = n`) | the row's `epoch` | K1d: new handle, new object, new row, column CAS; old row and object retire through `blobgc` | `ref` |

Blind-index columns are untouched by every one of these statements, and
K1's test asserts every `$bidx` value byte-identical across the pass.

#### K1a — the engine and the state table (`ordo-exec`)

Files: new `crates/ordo-exec/src/reseal.rs`; `lib.rs` adds `pub mod reseal`.

```rust
pub const RESEAL_DDL: &str;                       // "$reseal"
pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError>;
pub enum Population { Payloads, Columns, BlobRefs, Objects }
pub struct PassRow { space, from_epoch, to_epoch, population, cursor: Option<String>,
                     rows_done, rows_skipped, chunk_digest: Option<String>, attempts,
                     state: PassState, reason, started_at, advanced_at, finished_at }
pub enum PassState { Pending, Running, Done }
pub async fn enqueue_rotation(pool, keys: &dyn KeyProvider, space, reason, now)
    -> Result<Epoch, ResealError>;                // rotate() + four Pending rows
pub async fn enqueue_pass(pool, space, from: Epoch, to: Epoch, reason, now)
    -> Result<bool, ResealError>;                 // rows for an epoch already minted
pub struct Reseal<'a> { pool, keys, schema, space, chunk: usize }
pub async fn step(&self, now) -> Result<Step, ResealError>;   // one chunk
pub enum Step { Idle, Moved { population, done, skipped },
                Completed { from_epoch } }
pub async fn remaining(pool, schema, space, epoch) -> Result<Remaining, PoolError>;
pub struct Remaining { payloads, columns, blobrefs, objects }  // impl total()
pub async fn passes(pool, space) -> Result<Vec<PassRow>, PoolError>;
```

`$reseal`: `("space", "from_epoch", "to_epoch", "population", "cursor",
"rows_done", "rows_skipped", "chunk_digest", "attempts", "state", "reason",
"started_at", "advanced_at", "finished_at")`, `PRIMARY KEY ("space",
"from_epoch", "population")`. `chunk_digest` is SHA-256 over
`(ref ‖ new-envelope)` per §1.5; nothing else about a row is persisted.

`step` picks the lowest `(from_epoch, population)` row not `Done`, takes
`release(space, Purpose::Reseal, "reseal:<space>:<from>:<population>")`,
reads one chunk after the cursor, and for each row: open under `n`, seal
under the current epoch, open the result, compare, drop both. A mismatch
returns `ResealError::Verify` and the chunk is not committed. The CAS
update, the cursor, the counts and the digest commit in one transaction. A
zero-row `UPDATE` is `rows_skipped`. When a population is exhausted its
state is `Done`; when all four are `Done`, `remaining()` is re-checked and a
non-zero population is re-opened. `Completed` is returned once at zero.

Tests (`crates/ordo-exec/tests/reseal.rs`, market fixture, own database):

- `a_pass_moves_every_population_and_no_digest`: seed memos and emails,
  snapshot every `$bidx`, every plaintext, every image handle and every
  envelope; `enqueue_rotation`; `step` to `Completed`; assert
  `epoch_payload_count(old) == 0`, `remaining(old).total() == 0`, every
  `$bidx` byte-identical, every plaintext byte-identical through `open`
  and through `sealed::resolve`, every envelope changed.
- `a_killed_pass_resumes_from_its_cursor`: chunk of 1, `step` three times,
  drop the engine, build a new one over the same pool, `step` to
  `Completed`; the `$reseal` rows show `rows_done` summing to the row count
  and no row processed twice.
- `a_row_written_during_the_pass_is_skipped_not_clobbered`: write a memo
  under `n+1` between two steps of a pass over `n`; the pass finishes with
  the row holding the new value, and the stale envelope the test kept is
  never written back.
- `a_verification_mismatch_aborts_the_chunk`: a provider wrapper whose
  `seal` corrupts one byte; `step` returns `ResealError::Verify` and the
  `$reseal` row's cursor has not advanced.

**Done when** the four tests pass and nothing in `ordo-exec` outside
`reseal.rs` changed except `lib.rs`'s module line.

#### K1b — the worker fills the slot (`ordo-host`)

Files: `crates/ordo-host/src/workers.rs` (`KeysReseal`, and `Slot::all`
loses `keys.reseal`), `runtime.rs::start_supervisor` registers it,
`supervisor.rs::bootstrap` calls `reseal::ensure_tables`.

```rust
pub struct KeysReseal { pub chunk: usize, pub pause: Duration,
                        pub stream: Option<Arc<ordo_secevent::Recorder>> }
impl Worker for KeysReseal { name "keys.reseal"; Cadence::Tick(pause);
                             requires_leadership true }
```

`run` builds a `Reseal` over `ctx.pool`, `ctx.keys`,
`ctx.deployment.current().fixture.schema`, calls `step`, and maps `Moved`
to `Progress::did(done)`, `Idle` to `Progress::idle()`, `Completed` to a
`CustodyAction { action: "reseal_completed" }` observation on the stream
(`Subject::platform("keys.reseal")`, `Relationship::Custodian`, the
metered-keys precedent). Before the step it sets the two K3 gauges from
`remaining()` and the row's `advanced_at`, so a wedged worker still
reports.

Test (`crates/ordo-host/tests/reseal.rs`):
`a_rotation_reseals_under_the_supervisor`: `deployment_ready`, `assemble`,
`start_workers`; write memos through the door; `enqueue_rotation` with
`rt.keys`; wait for `remaining(old).total() == 0`; assert the census shows
`keys.reseal` enabled with `units > 0`, the memos read back unchanged
through the door, and the security stream holds `reseal_completed`.

**Done when** that test passes and
`without_a_run_handler_the_runner_does_not_start` is updated to expect
`keys.reseal` enabled (the slot is filled).

#### K2 — the gate (`ordo-exec`), then `ordo keys destroy` (`ordo-cli`)

Files: `crates/ordo-exec/src/reseal.rs` (`destroy_gate`),
`crates/ordo-cli/src/keys.rs` (`destroy`).

```rust
pub enum Gate { Clear { resealed_at: Option<u64> },
                Refused { population: Population, count: i64 },
                Current }
pub async fn destroy_gate(pool, schema, keys, space, epoch) -> Result<Gate, ResealError>;
```

`Current` for the write epoch (the provider refuses it too; the gate
names it first). `Refused` names the first population with a non-zero
count and the count. `Clear` carries the pass's `finished_at`, which is
the date the confirmation prints: *backups and archived log bundles taken
before <date> hold this epoch's ciphertext and will not open for sealed
fields in this space once it is destroyed.*

`ordo keys destroy --space <s> --epoch <n> --as <who> --confirm <s>:<n>`:
the gate, the confirmation text, the `--confirm` token spelled as
`<space>:<epoch>` so a copied command for another space refuses, then
`destroy_epoch`, then a `CustodyAction { action: "epoch_destroyed" }`
entry. Without `--confirm` it prints the consequence and exits 1.

Tests. `crates/ordo-exec/tests/reseal.rs::the_gate_refuses_by_name_with_the_count`
is **written first and observed failing** against a `destroy_gate` that
returns `Clear` unconditionally, then made to pass: with payloads still
under `n` the gate is `Refused { Payloads, k }` with `k` the exact count;
after the pass it is `Clear`; `destroy_epoch(n)`; every envelope captured
before the pass refuses to open (`KeyError::UnknownEpoch(n)`, asserted per
row), every value read through the ordinary path is byte-identical, and a
payload row deliberately re-tagged under `n` by the test after the pass
resolves `Destroyed(n)` and the revert refuses with the typed rejection.
`crates/ordo-cli/tests/keys.rs::destroy_refuses_without_the_gate_and_the_token`
drives the command.

**Done when** `destroy_epoch` has one production caller in the workspace:
the CLI command behind the gate.

#### K3 — the operator surface and the two gauges

Files: `crates/ordo-cli/src/keys.rs` (`status`, `rotate`, `reseal`),
`crates/ordo-host/src/metrics.rs` (`gauge`, rendered `# TYPE … gauge`),
`crates/ordo-host/src/workers.rs` (the worker sets both).

- `ordo keys status --space <s>`: current epoch; every live epoch with its
  four counts; each `$reseal` row's state, cursor, `rows_done`,
  `rows_skipped`, `attempts`, and seconds since `advanced_at` for a
  running row; for an epoch at zero, the `finished_at` date.
- `ordo keys rotate --space <s> --reason <r> --as <who>`:
  `enqueue_rotation`, prints the new epoch and the four counts the pass
  will move, records `CustodyAction { action: "epoch_rotated" }`.
- `ordo keys reseal --space <s> --as <who>`: the forced re-encrypt. For
  every live epoch below the current one with a non-zero count and no
  open pass, `enqueue_pass`. This is what an operator runs after an
  exposure when the rotation already happened, and it is idempotent.
- `Metrics::gauge(name, labels, value)`: a third map, rendered after the
  histograms under `# TYPE <name> gauge`, charged against `MAX_SERIES`.
  `ordo_reseal_rows_remaining{space,population}` and
  `ordo_reseal_stuck_seconds{space}` (0 when no pass is running).

Tests: `metrics.rs::a_gauge_can_go_down_and_renders_as_a_gauge`;
`crates/ordo-host/tests/reseal.rs::a_wedged_pass_raises_the_stuck_gauge`:
a bare supervisor over a provider wrapper that refuses `Purpose::Reseal`
leases, a pass enqueued, and after two seconds
`ordo_reseal_stuck_seconds{space}` is above zero while
`ordo_reseal_rows_remaining{…,population="payloads"}` is non-zero;
`crates/ordo-cli/tests/keys.rs::rotate_then_status_reports_the_pass`.

**Done when** an operator can answer "is this rotation finished, and if
not, where is it stuck" from `ordo keys status` and from the two gauges.

#### K1d — blob objects (built last, after K3)

`Reseal` takes an `Option<&dyn StoreResolver>`; the worker hands in the
deployment's, the CLI has none, and with none the population is counted
and reported blocked. Per row with a live reference: fetch the object,
open it chunk by chunk under `n`, seal under the current epoch with a
fresh stream header and a handle minted for the new epoch, verify every
chunk in memory, `put`, insert a new `$blobref` row with the three
envelopes carried or re-sealed, CAS the entity column from the old hex
handle to the new, commit. A row no live column names is skipped: it is
`blobgc`'s to retire, and moving it would mint a second orphan. The old
row and object are left for `blobgc::sweep`.

**Built 2026-09-19.** Two things the increment settled beyond the plan:
the object population is never re-opened by the pass itself (what a sweep
leaves behind is old pairs only `blobgc` can retire, and re-opening would
scan them once per tick forever), so it reports `blocked` and `ordo keys
reseal` re-opens it; and the whole object is held in memory for the move,
bounded by the blob policy's cap, which is the shape §1.3 names. Test:
`crates/ordo-exec/tests/reseal.rs::an_external_object_is_resealed_to_a_new_handle`
over `FsStore`: the column moves, the new object reads and verifies under
the new epoch, the old row and object still read, an orphan is untouched,
the gate refuses on two objects until the old rows go, and the re-opened
pass then completes.

---

## 2 · Track R — restore & DR

**Re-verified 2026-09-19 against `a2cdfe6`.** The section below was written
on 2026-09-08 against `89b1d1d`, and four things it proposed or assumed have
since landed or moved. Each is corrected in place; the reasoning that still
holds is kept. Line numbers cited in this section are those of `a2cdfe6`.

### 2.1 The starting state, re-read

`implementation.md`'s "Backup, restore & disaster recovery" (`:421-475`) is
still `[proposed]`, and in code the *discontinuity* half is still zero:
`grep discontinuity crates/` finds one comment in `export.rs`. What has
changed around it:

| Claim as written on 2026-09-08 | State on 2026-09-19 |
|---|---|
| "a bundle has no digest" (R1's headline gap) | **Built.** `DigestingSink` (`crates/ordo-exec/src/export.rs:562-625`) hashes every member as it is written; `manifest_json` (`:1262-1340`) writes a `digests` map, member name to `{bytes, sha256}`, lowercase hex. The manifest does not digest itself and its module doc says so (`:117-123`). Nothing yet emits the bundle's *own* digest anywhere a marker could read it, and no test flips a byte and checks the map; that is what remains of R1. |
| "the restore marker takes a **second** trailer byte, `0x02`" | **`0x02` is taken.** `TRAILER_RESTORED_UNVERIFIED = 0x02` (`crates/ordo-commandlog/src/chain.rs:271`) landed 2026-09-10 as `plan-opslog.md` D7: the flag a revert's own group carries when it restored a blob handle with no recorded digest. Unrelated to a database restore; the word collides, the mechanism does not. The restore marker takes **`0x03`**, and it is the first trailer that carries a payload. The decoder now reads tags in ascending order and refuses a repeat, an unknown tag, or two known tags out of order (`:380-418`); two goldens pin the pre-flag encodings (`tests.rs:654`, `:1095`). |
| "there is no server binary to start; the assertion lands on `PlatformState::new`" | **The binary exists** (`crates/ordo-host/src/main.rs`; `RuntimeConfig::from_env`, `assemble`, `BootRefusal` in `runtime.rs`). `build_custody` (`runtime.rs:1149-1189`) already runs two custody assertions before the door opens: the space has keys, and this master opens them. The restore-track assertions join those, in `assemble`, and refuse with `BootRefusal`. |
| "key material lives in the `KeyProvider`" and the backup holds none of it | **Half right.** `PgKeyStore` (`crates/ordo-exec/src/keystore.rs`) stores SMKs wrapped under the master and DEKs wrapped under their SMK in `$keycustody`, which is in the database and therefore in the backup. Its module doc states the rule this track needs: *"wrapped keys in the backup are the design; the master in the backup is the misconfiguration."* The master comes from `ORDO_MASTER_KEY` or a sealed file (`ordo_crypto::master::MasterKeySource`). |
| one deployment, one space | **Many spaces per process.** `Spaces::resolve` (`crates/ordo-host/src/spaces.rs:201-224`) binds a space per request, lazily, and `ordo_exec::route::PoolResolver` says which database its rows are in. "Per space at boot" therefore has two sites: `assemble` for `ORDO_SPACE`, and `Spaces::resolve` on a space's first use in a process. |
| `verify` has no concept of a discontinuity | **Still true.** `undo::verify` (`crates/ordo-exec/src/undo.rs:504-604`), the truncation check (`:544-548`), `choose_anchor` (`:462-497`), `Verified` (`:403-411`) and `VerifyFailure` (`:414-433`) are as described. |
| `Actor::System(...)` has no production producer of a command group | **Still true** (`crates/ordo-secevent/src/event.rs:184` renders one for a security event; nothing appends one to the chain). |
| `docs/howto-operating.md` has no backup or restore section | **Still true.** |

One fact the original section did not use and the design below rests on:
`$commandlog` stores the actor rendering as a plaintext column beside the
hashed body (`crates/ordo-exec/src/commandlog.rs:91`). A marker is found by
`WHERE "actor" = 'system:restore'` and confirmed by decoding the row, so no
index table is needed to locate one.

### 2.2 The backup contract — two backups, two jobs

**Recommendation, unchanged: both, with distinct and non-overlapping roles.
Neither substitutes for the other, and a deployment that runs only one
should be told which job it is not doing.**

| | Postgres PITR (base backup + WAL) | The export bundle |
|---|---|---|
| **Unit** | the deployment (the control database and every attached space database) | one space |
| **Answers** | "the disk is gone / a bad migration ran at 14:03" | "give me my data" / "reconstruct this one tenant" |
| **Granularity** | any point in time, all spaces at once | the state at export, one space |
| **Contains keys?** | **wrapped keys only**: `$keycustody` holds SMKs and DEKs wrapped under the master, and the master is never a row. A master that *is* a row is the misconfiguration R1b refuses | no; `SealedMode::Open` exports plaintext under owner authority, `Ciphertext` exports envelopes |
| **Verifiable standalone?** | no; it is a database | **yes**: every group carries `prev` and `hash`, so a reader recomputes `SHA-256(prev ‖ body)` with no platform, and every member carries a digest in the manifest |
| **Restores to** | this deployment, this shape | anywhere, including a different deployment |
| **Cost** | continuous | a scan per export |

PITR is operational recovery: it restores a deployment to a moment, and it is
the only thing that can, because a per-space bundle has no notion of "14:03".
The bundle is evidential and portable recovery: it survives the deployment
entirely, it is the exit promise `licensing.md` leg 1 makes, and it is the
only artifact a third party can check. A deployment with only PITR cannot hand
a tenant their data or prove anything to anyone; a deployment with only
bundles cannot recover from a bad Tuesday.

**Roles, stated as policy:** PITR is the RPO/RTO instrument and is
continuous. Bundles are cut on a cadence and **before every destructive
migration and every epoch destruction**, the two acts PITR cannot reverse
either, because the first drops columns and the second destroys the key the
backup's ciphertext needs. With per-space databases (`plan-space-routing.md`),
PITR covers every target the resolver lists (`PoolResolver::targets`), and
the howto says so, because a WAL archive for the control database alone
restores routing to spaces whose rows were not restored with it.

**The bundle digest, now that members have one.** The marker names a bundle
by one string. That string is `sha256(manifest.json)` over the manifest's
exact bytes: the manifest already names every other member's digest, so the
digest of the manifest fixes the whole bundle. No sidecar file is written;
`export.rs`'s own doc rules that *"something outside attests the manifest"*,
and the two things outside that need it are the export's response and the
operator's hand. R1a puts it in the response of `std.space.export` and in
`ExportReport`, and `ordo restore --record --source bundle --digest <hex>`
takes it.

**The encoding, one rule:** lowercase hex, SHA-256, no prefix. The manifest's
member digests, `std.space.chainProof`'s links (`attest_ops.rs:281`) and the
marker's `manifest_sha256` all render the same way, so an operator comparing
two artifacts never does string surgery.

### 2.3 `system:restore` cannot be a command group

`implementation.md` says the platform "appends a `system:restore` command
group". Taken literally that is unbuildable, for three independently
sufficient reasons:

1. **`CommandGroup::validate` refuses an empty group**:
   `GroupShapeError::EmptyGroup` (`crates/ordo-commandlog/src/command.rs:217-222`).
2. **The command taxonomy is closed to writes.** `CommandKind` is
   `Create | Converge | Delete` (`command.rs:100-104`) and every command
   carries an entity, a row id and images whose shape `validate` enforces.
   A restore is none of those.
3. **Forging one is the move `exit_ops.rs` already refused.** Its module doc
   (`:8-22`): recording a non-write as a `Converge` with equal images "would
   put a lie in the chain — and the chain's whole value is that a reader may
   take what it says literally."

The exit ops' answer, the security-event stream, is **not available here**,
and this is the asymmetry that decides the design. The whole requirement is
that `verify`, which walks the *command* chain, understands the seam. A
marker in another stream leaves `verify` reporting `Unreadable` at the rewind
point, which is exactly today's behaviour.

**Ruling (2026-09-08, encoding corrected 2026-09-19): the restore marker is a
chain entry, and it grows the frozen encoding by presence.** The precedent is
the owner-administration trailer and now the restored-unverified trailer
(`chain.rs:39-63`): a group that does not carry a flag renders the bytes it
rendered before the field existed, so every existing chain and every signed
checkpoint over it is untouched. The restore marker takes tag **`0x03`**,
the next tag in ascending order, and is the first trailer with a payload.
`EmptyGroup` is relaxed **only** in the presence of that trailer, so a write
that produced nothing is still refused, which is the failure `EmptyGroup`
exists to catch. A marker group with commands is refused too: the marker
records that history moved, and a group that also wrote rows would be two
facts in one entry.

The actor is `Actor::System("restore")`, which the pinned taxonomy already
renders (`command.rs:82-93`). This becomes its first producer of a chain
entry.

### 2.4 What the marker attests

Six fields. Each has a reason it is present and a stated behaviour when it
is absent:

| Field | What it says | If unknown |
|---|---|---|
| `space: SpaceId` | which chain this marker belongs to. `validate` reads a group's space from its first command; a marker has none, so the marker carries it, hashed | never absent |
| `restored_from: (idx, hash)` | the last entry that survived the restore | never absent; it is read from the restored database when the marker is recorded |
| `pre_restore_head: Option<(idx, hash)>` | what the chain said before, from the last signed checkpoint or an owner-held pin | **absent is a legal, disclosed answer**: after a total loss nobody can say. Absent means "unattested", never "nothing was lost" |
| `source` | `Pitr { at }` (milliseconds) or `Bundle { manifest_sha256 }` | never absent; a restore from an unidentifiable source is refused |
| `operator: String` | the human, as `--as` names them. **Not** the actor: workers attribute as `system:<component>` `[pinned]`, so the operator is a field, not a rendering | never absent |
| *(the group's own `correlation`)* | shared with the `CustodyAction { action: "space_restored" }` entry the same command writes to the security-event stream, so the two records join on one id | never absent; it is the group's correlation, so it is not a seventh field |

**Two corrections to the 2026-09-08 field list.** It had `step_up:
CorrelationId`, joinable to `StepUpSatisfied`. A step-up is a door ceremony,
and the marker cannot be recorded through the door: the boot assertion below
refuses to serve the space until the marker exists, so the door is closed
exactly when the marker is needed. The marker is recorded by the operator
CLI, which holds database credentials and **attributes rather than
authenticates**, the posture `ordo keys init --as` already takes and states
(`crates/ordo-cli/src/keys.rs:190-194`). The join to the security stream is
the correlation. It also had `operator: PrincipalId`; there is no such type
in `ordo-commandlog`, and the operator is a string the way `Actor::User`
carries one.

**Amended 2026-09-19 (`orphan-checkpoints`):** `pre_restore_head` is what
an orphaned checkpoint is explained by (R2b′), so `ordo restore --record`
defaults it to the newest signed head the deployment still holds, the
witnessed one first (R2d), and `None` is written only when nothing can be
defaulted from or the operator says `--unattested`.

**Why `restored_from` is a field when `prev` already links to it.** In the
common case the marker lands at `restored_from.idx + 1` and `prev` equals
`restored_from.hash`. The two differ when a space with no checkpoint above
the lost suffix keeps serving after the restore (nothing refuses it: the
boot assertion fires only when a checkpoint proves the chain was longer) and
ordinary writes land before the operator records the marker. Then
`restored_from` says where history broke and `prev` says where the marker
was written, and both are true. `ordo restore --record` prints the gap when
there is one.

The marker is written under the ordinary append lock through
`TxConn::append_group`, links to the head like any entry, and everything
after it verifies normally. Everything before it verifies against the bundle
or against a checkpoint at or below `restored_from`, which is why the anchor
rule in R2b exists and why `verify --from-genesis` remains the command that
proves the pre-restore prefix.

### R1 · the bundle digest in hand, and the key-material assertion

Three increments. All three are Sonnet-tier: none touches the chain
encoding or the verifier.

**R1a · The bundle digest is reported.** *Sonnet.*

- `crates/ordo-exec/src/export.rs`: `pub fn bundle_digest(manifest: &[u8]) -> String`
  (SHA-256, lowercase hex). `ExportReport` gains `pub manifest_sha256: String`,
  set where the manifest member is written (`:877-885`, the bytes are in
  hand there).
- `crates/ordo-host/src/exit_ops.rs` `export` (`:225-280`): the result row
  gains `"manifest_sha256": report.manifest_sha256`.
- Tests, `crates/ordo-exec/tests/export.rs`:
  - `a_flipped_byte_in_any_member_is_caught_by_the_manifest`: export to a
    `MemoryBundle`; for every name in the manifest's `digests`, flip one
    byte of that member and assert its SHA-256 no longer equals the
    manifest's entry; assert the manifest names every member except
    itself.
  - `the_bundle_digest_is_the_manifests_own_and_is_stable`: two exports of
    an unchanged space report equal `manifest_sha256`, and it equals
    `bundle_digest(manifest bytes)`.
- Test, `crates/ordo-host/tests/exit_ops.rs`
  `an_owners_export_reports_the_bundle_digest`: the response's
  `manifest_sha256` equals the SHA-256 of `manifest.json` on disk.
- **Done when** those three pass and `two_exports_of_one_state_are_byte_identical`
  still passes.

**R1b · The master is not in the database.** *Sonnet.*

`implementation.md` asks for a startup refusal when "a local master key is
configured inside the backed-up database". The binary cannot know where an
environment string came from, so the buildable form is a check of the one
table custody owns: no `$keycustody` row may hold the master's raw bytes or
the key `derive_from_secret(master)` produces (`ordo_crypto::master::derive_from_secret`).
Everything outside that table is the operator's rule, stated in R1c.

- `crates/ordo-exec/src/keystore.rs`: `impl PgKeyStore { pub fn holds_bytes(&self, needle: &[u8]) -> bool }`
  over the cache `open` already loads (every `wrapped` value, both kinds).
- `crates/ordo-host/src/runtime.rs` `build_custody`: after `PgKeyStore::open`
  and before `from_master`, refuse with
  `BootRefusal::Custody(KeyError::Store(..))` when `store.holds_bytes(&master)`
  or `store.holds_bytes(&derive_from_secret(&master))`, naming `$keycustody`
  and the remedy (remove the row; rotate the master with
  `ordo keys rewrap-master`).
- `crates/ordo-cli/src/keys.rs` `init`: the same check, same message, exit 1.
- Test, `crates/ordo-host/tests/runtime.rs`
  `a_master_stored_in_the_database_refuses_the_boot`: provision under a
  known master (`deployment_under`), `INSERT INTO "$keycustody"` a row whose
  `wrapped` is the master bytes, `assemble` → `Err(BootRefusal::Custody(_))`
  whose text contains `$keycustody`. **Observed failing first:** before the
  change `assemble` returns `Ok`.
- **Done when** that test passes and `the_binary_boots_under_a_sealed_master_file`
  still passes.

**R1c · The contract, written down.** *Sonnet.*

`docs/howto-operating.md` grows a "Backup and Restore" section between
"Metrics" and "Verifying the Audit Chain": what each backup is for (the
table in §2.2); that PITR must cover every database `PoolResolver::targets`
lists; the two mandatory pre-cut points (destructive migration, epoch
destruction); the ordering rule K2 owes; that the master key and the
sealed-master passphrase live outside every backed-up database and outside
the Postgres data directory; and the restore procedure R2c and R2d define
(restore, boot, read the refusal, `ordo restore --record`, `ordo verify`).
**Done when** the section exists and every command it names exists.

**Built 2026-09-19, note on sequencing.** R1c landed with R1a and R1b, and
R2 (the marker, the verifier, `ordo restore --record`, the boot assertion)
had not: it is Opus-tier and out of scope for a Sonnet pass. The done-when
as written cannot hold at that point, since the restore procedure it asks
the section to name is not yet a command that exists. The section built
instead documents everything this increment makes true (the backup
contract, the PITR coverage rule, the two pre-cut points, K2's ordering
rule, the master-outside-every-backup rule) and states plainly, rather than
silently, that Track R2 is not built yet and points at checklist 17j. The
restore procedure itself is still owed once R2c and R2d land.

### R2 · the marker, the verifier, and the boot assertion

Four increments, in order. R2a and R2b are **Opus-tier**: R2a changes the
frozen chain encoding, R2b changes what the verifier accepts. R2c and R2d
are Sonnet-tier plumbing over them.

**R2a · The marker type and the trailer.** *Opus. Depends on nothing.*

- `crates/ordo-commandlog/src/command.rs`:

  ```rust
  pub struct RestoreMarker {
      pub space: SpaceId,
      pub restored_from: (u64, ChainHash),
      pub pre_restore_head: Option<(u64, ChainHash)>,
      pub source: RestoreSource,
      pub operator: String,
  }
  pub enum RestoreSource {
      Pitr { at: Timestamp },
      Bundle { manifest_sha256: [u8; 32] },
  }
  // on CommandGroup:
  pub restore: Option<RestoreMarker>,
  // on GroupShapeError:
  MarkerWithCommands { correlation: CorrelationId },
  ```

  `CommandGroup::validate`: when `restore` is `Some`, require
  `commands.is_empty()` (else `MarkerWithCommands`) and return
  `&marker.space`; when `None`, exactly today's rule. Every
  `CommandGroup { .. }` literal in the tree gains `restore: None` (the
  `restored_unverified: false` grep above lists all of them: 24 sites).
- `crates/ordo-commandlog/src/chain.rs`: `const TRAILER_RESTORE: u8 = 0x03;`
  Grammar, appended to the module doc's block:

  ```text
  trailer    := 0x01 | 0x02 | 0x03 ‖ restore
  restore    := str space ‖ u64-BE idx ‖ hash32
              ‖ opt-head ‖ source ‖ str operator
  opt-head   := 0x00 | 0x01 ‖ u64-BE idx ‖ hash32
  source     := 0x00 ‖ u64-BE at | 0x01 ‖ digest32
  ```

  `encode_group` pushes `0x03` and the payload after the two flag tags.
  `decode_group`'s loop: on `0x03`, read the payload with the existing
  `Reader` (a short read is `DecodeError::Truncated`, whatever the reader
  already raises), set `restore`, and continue; the ascending-order rule
  then refuses anything after it, since `0x03` is the highest tag. Explicit
  `0x00` stays `TrailingBytes`.
- `crates/ordo-commandlog/src/store.rs` `invert_group`: a group with
  `restore.is_some()` returns `Err(RevertError::RestoreMarker(correlation))`,
  a new variant with a `Display` that says there is nothing to undo.
- Tests, `crates/ordo-commandlog/src/tests.rs`, in this order:
  1. `PRE_THIRD_FLAG_GOLDEN`: **computed and committed before the encoder
     changes**, the way `PRE_SECOND_FLAG_GOLDEN` (`:1095`) was, over the
     same fixture group. `an_unflagged_group_hashes_where_it_did_before_the_restore_trailer`
     asserts it; this test must pass both before and after.
  2. `a_restore_marker_round_trips`: encode, decode, equal; the hash
     differs from the unflagged hash.
  3. `a_restore_marker_with_commands_is_refused` (`MarkerWithCommands`).
  4. `an_empty_group_without_a_marker_is_still_refused` (`EmptyGroup`).
  5. `a_truncated_marker_payload_refuses` (cut the payload at three
     places).
  6. `the_restore_tag_before_a_lower_tag_refuses` (`0x03 ‖ payload ‖ 0x01`).
  7. `a_marker_group_renders_all_three_tags_in_order` (owner
     administration, restored unverified and restore on one group;
     decodes to all three).
  8. `a_marker_cannot_be_reverted` (`RevertError::RestoreMarker`).
- **Done when** all eight pass, every existing chain test passes unchanged,
  and `cargo test -p ordo-commandlog` is green.

**R2b · The verifier understands the seam.** *Opus. Depends on R2a.*

- `crates/ordo-exec/src/undo.rs`:

  ```rust
  pub struct Discontinuity { pub idx: usize, pub marker: RestoreMarker }
  // on Verified:
  pub discontinuities: Vec<Discontinuity>,
  pub orphaned_checkpoints: Vec<u64>,     // `upto` values, kept, listed
  pub enum Continuity {
      Intact,
      Explained { marker_idx: usize, checkpoint_upto: u64 },
      Rewound { head: Option<usize>, checkpoint_upto: u64 },
  }
  pub async fn latest_restore_marker(db: Db<'_>, space: &SpaceId)
      -> Result<Option<(usize, RestoreMarker)>, PoolError>;
  pub async fn continuity(db: Db<'_>, space: &SpaceId)
      -> Result<Continuity, PoolError>;
  ```

  `latest_restore_marker`: `SELECT idx, body FROM "$commandlog" WHERE space
  = ? AND actor = 'system:restore' ORDER BY idx DESC LIMIT 1`, decode, and
  require `restore.is_some()`; a row whose actor column claims the marker
  and whose body carries none is `PoolError::Db(..)` naming the index (the
  actor column is plaintext and hashed through the body, so the two cannot
  disagree on an untampered chain).

  `continuity`: with `cp = latest_checkpoint` and `head = head_of`:
  `Intact` when there is no checkpoint or `head.idx + 1 >= cp.upto`;
  otherwise `Explained` when a marker exists with
  `marker.restored_from.0 + 1 < cp.upto`; otherwise `Rewound`.

  **The anchor rule**, in `choose_anchor`: when a marker exists, the anchor
  is the newest verifiable checkpoint with `upto <= marker.restored_from.0 + 1`
  (new helper `checkpoint_at_or_below(db, space, upto)`), and a checkpoint
  above that bound is never the anchor. Without this the truncation check
  (`:544-548`) fires on every lawful restore. `verify` collects
  `Discontinuity` for every decoded group whose `restore` is `Some`, and
  fills `orphaned_checkpoints` with every `$checkpoints.upto` greater than
  `restored_from.0 + 1` of any marker it passed. Orphans are derived by
  arithmetic, kept, and listed; no column stores the verdict.
- `crates/ordo-cli/src/operator.rs` `verify`: prints
  `VERIFIED n groups · 1 restore discontinuity at index N`, then one line
  per orphaned checkpoint (`orphaned  checkpoint upto 1300, kept`), exit
  **0**. A rewind with no marker still exits 3 exactly as today.
- Tests, `crates/ordo-cli/tests/operator.rs` (its harness has `seeded`,
  `append`, `chain_len`, `undo::sign_checkpoint`; a marker group is built
  by hand with `restore: Some(..)` and appended through `append`):
  - `a_recorded_restore_verifies_with_one_discontinuity_at_its_index`.
    **Observed failing first:** after R2a and before R2b, the same setup
    returns `Tamper(Unreadable)` at `head + 1`, because the anchor is the
    orphaned checkpoint.
  - `verify_anchors_at_or_below_the_restored_head_never_above`.
  - `orphaned_checkpoints_are_listed_and_kept` (the rows are still in
    `$checkpoints` after verify).
  - `continuity_reports_rewound_then_explained` (rewind → `Rewound`;
    append marker → `Explained`; a fresh chain → `Intact`).
  - `two_restores_are_two_discontinuities_in_order`.
  - `a_row_whose_actor_claims_restore_without_a_marker_is_refused`
    (doctor the actor column of an ordinary row; `latest_restore_marker`
    errors and names the index).
  - `a_truncated_tail_is_caught_by_the_checkpoint_that_covered_it` (exists,
    `:364`) keeps passing: no marker, still exit 3.
- **Done when** all of the above pass and `docs/howto-operating.md`'s
  "Verifying the Audit Chain" shows the discontinuity line.

**Built 2026-09-19, R2a and R2b, with five differences from the text
above.** Reasons in `corpus-rulings.md`, "Building the restore marker and
the seam".

1. `PRE_THIRD_FLAG_GOLDEN` is taken over `create_group("grp-r2a")`, not
   the fixture `PRE_SECOND_FLAG_GOLDEN` used (same bytes, same number), and
   a second golden pins a group carrying both older tags.
2. `an_unknown_trailer_tag_still_refuses` used `0x03` as its unknown tag.
   It now uses `0x04`, and also asserts a bare `0x03` refuses
   (`Truncated`). This is the one existing chain test that could not pass
   unchanged.
3. `validate` also refuses a marker whose actor is not `system:restore`
   (`GroupShapeError::MarkerActor`), so the actor column finds every
   marker the gate admits.
4. The anchor bound is the lowest `restored_from + 1` over every marker,
   not the newest marker's; with the newest only, a second restore
   anchors on the first restore's orphan.
5. `RestoreSource::Pitr { at }` is a `Timestamp`, which is seconds in the
   code; §2.4 said milliseconds. R2c's `--at` takes seconds.

**Ruled 2026-09-19 (`orphan-checkpoints`):** a checkpoint counts when the
stored chain agrees with its head. One whose signed head equals the stored
hash at its position is current history and may anchor; one the stored
chain contradicts is orphaned by a restore. The rule above (every
checkpoint above a marker's seam is orphaned) broke verification across a
prune, let a later unannounced rewind read as `Explained`, and confused
signing at an orphan's position. R2b′ changes the built code to the new
rule; the marker payload does not change. R2d reads its last paragraph:
`$checkpoints` rewinds with a whole-database PITR, and the record a space
restore cannot roll back is the witness R2d builds.

**R2b′ · The stored chain classifies a checkpoint.** *Opus: it changes
what the verifier accepts and how a checkpoint is stored. Depends on R2b;
before R2c, since R2c's default reads the rule.*

The rule, in one function, used by every reader that picks a checkpoint:

- `crates/ordo-exec/src/undo.rs`:

  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Agreement {
      /// The chain holds the record's head at its position: current history.
      Current,
      /// The chain holds another hash there, or nothing that far: orphaned by a restore.
      Orphaned,
      /// Both entries that could confirm it are pruned; the chain cannot say.
      BelowFloor,
  }
  pub async fn agreement(db: Db<'_>, space: &SpaceId, upto: u64, head: ChainHash)
      -> Result<Agreement, PoolError>;
  ```

  At most two row reads. The entry at `upto - 1` with hash `head` is
  `Current`; that entry with another hash is `Orphaned`; when it is
  missing, the entry at `upto` decides by its `prev` (the case after a
  prune that cut exactly at the checkpoint); when both are missing, the
  record is `Orphaned` if `upto - 1` is above the head or the chain is
  empty, and `BelowFloor` if `upto` is below the first stored index.
- **The anchor.** `choose_anchor` takes the newest stored checkpoint that
  is `Current`: `ORDER BY "upto" DESC`, walked until one agrees, which is
  the first row on any chain that has not been restored. `seam_bound` and
  `undo::checkpoint_at_or_below` go, and the markers are not read here. An
  agreeing checkpoint whose signature does not verify is still
  `UnverifiedCheckpoint`.
- **Orphans and their explanation.** `Verified.orphaned_checkpoints`
  becomes `Vec<(u64, ChainHash)>`, every stored checkpoint that is
  `Orphaned`, since two rows can now share a position. An orphan is
  **explained** when a stored marker attests a pre-restore head at or
  above it: `upto <= marker.pre_restore_head.idx + 1`, over every marker
  `restore_markers` returns, not only the ones the walk passed, because
  the anchor can sit above a marker. A marker whose `pre_restore_head` is
  `None` explains nothing: absent means unattested, and an orphan it
  cannot bound is an orphan nobody vouched for. An **unexplained** orphan
  fails verification, from any anchor: above the head it is the truncation
  finding as today, `Tamper(TamperAt { index: head + 1, kind: Unreadable
  })`; at or below the head it is a new `VerifyFailure::UnexplainedOrphan
  { upto, head }`, whose `Display` says a checkpoint was signed over a head
  the chain no longer holds at that position and no restore marker attests
  a head at or above it. Judgement reads `$checkpoints` as data, so a
  forged row fails verification naming itself rather than passing quietly.
  `BelowFloor` rows are listed as neither.
- **`continuity`** (space database only here; R2d-ii adds the witness):
  every stored checkpoint classified; each orphan explained by the bound
  rule above; the highest unexplained orphan is `Rewound { head,
  checkpoint_upto }`; otherwise `Explained { marker_idx, checkpoint_upto }`
  for the highest explained one, or `Intact`. The old rule read the newest
  checkpoint only, which is how a later rewind below a post-restore
  checkpoint passed as explained.
- **Signing at an orphan's position.** `sign_checkpoint` answers
  `AlreadyAt(upto)` only when the stored row at `upto` is `Current`; a row
  there with another head is an orphan, and the new checkpoint is stored
  beside it. That needs the key to admit two rows at one position:
  `crates/ordo-exec/src/commandlog.rs` `COMMANDLOG_DDL` drops the
  `PRIMARY KEY ("space", "upto")` from the `CREATE`, and appends `ALTER
  TABLE "$checkpoints" DROP CONSTRAINT IF EXISTS "$checkpoints_pkey"` and
  `CREATE UNIQUE INDEX IF NOT EXISTS "$checkpoints_position_head" ON
  "$checkpoints" ("space", "upto", "head")`, both idempotent, in the shape
  `secevent.rs:109` uses for `ADD COLUMN IF NOT EXISTS`; `put_checkpoint`
  conflicts on the three columns. The security stream's own checkpoint
  table is untouched.
- **Every reader that picks a checkpoint by position gains the filter.**
  `PgStore::try_latest_checkpoint` (`attest_ops::build_proof:441`: a
  proof's anchor must be current, or a third party is handed an orphan);
  `prune::checkpoint_at_or_below` and `checkpoint_exactly_at`
  (`prune.rs:643-660`: the cut point and the base are `Current`, and an
  orphan is never a cut point); `undo::latest_checkpoint` stays as the
  CLI's no-key note.
- **The prune keeps a marker while an orphan above the cut needs it.** A
  cut at `F` deletes entries below `F`; an orphan with `upto >= F` is still
  judged afterwards, and the marker that explains it is gone if its index
  is below `F`. `prune::floor_in` therefore lowers the cut to the greatest
  `Current` checkpoint at or below that marker's index, reported as
  `FloorHolder::RestoreMarker { index, orphan_upto }`, and the clamp lifts
  once the live bound passes the orphan, so the marker and the orphans it
  explains fall below the floor in one cut. What is retained past the
  horizon is bounded by the history the restore discarded.
- `crates/ordo-cli/src/operator.rs` `verify`: `orphaned  checkpoint upto 6
  head 4b1e…, explained by restore at index 4, kept`; `prune`'s floor
  report prints the new holder. `docs/howto-operating.md` "Verifying the
  Audit Chain" states the rule in the paragraph that today states the seam
  bound.
- `crates/ordo-exec/src/undo.rs` `record_restore` (R2c, when it is built):
  `pre_restore_head` defaults to the newest stored checkpoint, whatever its
  agreement, when the operator gives none; R2d-i prefers the witness.
- Tests, `crates/ordo-cli/tests/operator.rs`, the three failures first:
  - `verify_survives_a_prune_across_the_seam`: six entries, checkpoint at
    6, rewind to 4, marker at 4 attesting `(5, H6)`, regrow to index 9,
    checkpoint at 10, two more entries; `prune::prune` with `now` past the
    horizon (the retained tail holds the bound at 11, the cut is the
    checkpoint at 10, and the clamp does not fire because the orphan at 6
    falls below the floor in the same cut); `choose_anchor` → 10;
    `verify` → `Ok` with no discontinuity (the marker is pruned) and no
    orphan listed (6 is `BelowFloor`). **Observed failing first:** the
    seam bound is 4, no checkpoint sits at or below it, the anchor falls to
    genesis, and `verify` answers `PrefixMissing { first_stored: 10 }`.
  - `a_later_unannounced_rewind_is_rewound_not_explained`: the same
    restore, regrow to 20, checkpoint at 20, delete from 10 with no
    marker; `continuity` → `Rewound { head: Some(9), checkpoint_upto: 20
    }` and `verify` fails with the truncation finding at 10. **Observed
    failing first:** `Explained { marker_idx: 4, checkpoint_upto: 20 }`.
  - `a_checkpoint_at_an_orphans_position_is_signed_and_both_rows_are_kept`:
    the same restore, regrow to index 5, `sign_checkpoint` → `Signed` at
    `upto` 6 with the new head; `$checkpoints` holds two rows at 6;
    `choose_anchor` → 6 with the new head; `verify` → `Ok`, orphans `[(6,
    H6_old)]`. A second call answers `AlreadyAt(6)`. **Observed failing
    first:** `AlreadyAt(6)` on the first call, and the insert would have
    been dropped by the old conflict clause.
  - `a_prune_keeps_a_marker_while_an_orphan_above_the_cut_needs_it`:
    checkpoint at 12 before the restore, restore to 3 with the marker at 4
    attesting `(11, H12)`, regrow to index 14 with checkpoints at 10 and
    15; the retained tail holds the bound at 14, the cut would be the
    checkpoint at 10, and that would leave the orphan at 12 judged with
    its marker gone; the floor reports `RestoreMarker { index: 4,
    orphan_upto: 12 }` and the cut is the checkpoint at or below 4 (none,
    so `NothingToPrune`); append one more entry so the tail permits a cut
    at 15, and the cut is 15, with the marker and the orphan below the
    floor together.
  - `an_unexplained_orphan_fails_verification_naming_the_checkpoint`:
    checkpoints at 3 and 6, rewind to 4, regrow to 8 with no marker;
    `choose_anchor` → 3; `verify` → `Err(UnexplainedOrphan { upto: 6, .. })`;
    exit 3 from the binary.
  - `an_unattested_marker_explains_nothing`: the restore recorded with
    `pre_restore_head: None`; `continuity` → `Rewound` naming 6; re-record
    with `Some((5, H6))` → `Explained`.
- Existing tests that change, each with its reason in the test's doc
  comment: `a_truncated_tail_is_caught_by_the_checkpoint_that_covered_it`
  flips its first assertion (from genesis the truncation is now caught
  too, because orphans are judged from `$checkpoints` rather than from the
  anchor) and keeps `(3, Unreadable)` for the second;
  `continuity_reports_rewound_then_explained` records its marker with
  `Some((5, hash6))`; `orphaned_checkpoints_are_listed_and_kept` asserts
  `[(6, cp6.head)]`; `verify_anchors_at_or_below_the_restored_head_never_above`
  becomes `verify_anchors_on_the_newest_checkpoint_the_chain_agrees_with`,
  with its markers attesting `(5, H6)`; `two_restores_are_two_discontinuities_in_order`
  keeps its assertions with the orphan's head added. Every other R2b test
  passes unchanged.
- **Done when** all of the above pass, `cargo test -p ordo-exec --test
  cadence_prune` is green with the clamp, and the howto states the rule.

**R2c · `ordo restore --record`.** *Sonnet. Depends on R2b.*

- `crates/ordo-exec/src/undo.rs`:

  ```rust
  pub struct RestoreRecord { pub operator: String, pub source: RestoreSource,
                             pub pre_restore_head: Option<(u64, ChainHash)>,
                             pub again: bool }
  pub struct Recorded { pub idx: usize, pub hash: ChainHash,
                        pub restored_from: (u64, ChainHash),
                        pub gap: usize, pub audit: String }
  pub async fn record_restore(pool: &Pool, space: &SpaceId, rec: RestoreRecord)
      -> Result<Recorded, RestoreError>;
  ```

  One transaction: `head_of` (make it `pub`) gives `restored_from`;
  refuse `RestoreError::NoHistory` on an empty chain; refuse
  `RestoreError::AlreadyRecorded { idx }` when the newest marker has the same
  `restored_from` unless `rec.again` is set (a second marker on the same
  restore point is almost always a re-run, and a real second restore to the
  same point is what `--again` says); build the group with
  `Actor::System("restore")`, a minted correlation, `commands: vec![]`,
  `restore: Some(marker)`; `tx.append_group`; then
  `tx.append_security_entry` for a `CustodyAction { action: "space_restored" }`
  observed through a `Recorder` exactly as `keys.rs::record_provisioned`
  does (`:217-258`), with the group's correlation adopted via
  `ordo_secevent::correlation::adopt`; commit. `gap` is
  `idx - (restored_from.0 + 1)`.
- `crates/ordo-cli/src/operator.rs`: `"restore"` with `--record`, flags
  `--space --db --as --source pitr|bundle --at <ms> --digest <hex>
  --pre-restore-head <idx>:<hex> --again`. Output block: `space`,
  `continuity before`, `RECORDED marker at index N · restored from
  (idx, hash) · source … · operator …`, `gap` when non-zero, `audit`
  (recorded or `NOT RECORDED (..)`), `continuity after`. Exit 0 recorded,
  1 refused, 2 usage. `crates/ordo-cli/src/main.rs`: add `"restore"` to the
  dispatch list (`:131`) and to `usage()`, with the line *"restores are
  recorded by the operator who ran them; this tool attributes, it does not
  authenticate"*.
- Tests, `crates/ordo-cli/tests/operator.rs` (`ordo(args)` at `:958` runs
  the binary):
  - `restore_record_writes_the_marker_at_head_plus_one_and_the_custody_event`
    (the chain gains one entry with `actor = 'system:restore'`, and
    `$secevents` gains one `custody-action` whose correlation equals the
    group's).
  - `restore_record_refuses_an_unidentifiable_source` (no `--at` and no
    `--digest`: exit 2).
  - `restore_record_without_a_pre_restore_head_prints_unattested`.
  - `restore_record_twice_on_one_point_needs_again`.
  - `restore_record_reports_the_gap_when_writes_landed_first`.
- **Done when** those pass and `verify` after `restore --record` exits 0
  with one discontinuity.

**Amended 2026-09-19 (`orphan-checkpoints`, `restore-witness`), before it
is built.** Five changes to the text above, each from one of the two
rulings:

1. `record_restore(pool, witness: &Witness, space, rec)`; the CLI takes
   `--control-db <name>`, defaulting to `--db`.
2. `pre_restore_head` defaults when `--pre-restore-head` is absent: the
   newest witnessed checkpoint of the space
   (`Witness::latest_checkpoint`), else the newest stored checkpoint
   whatever its agreement, else `None`; `--unattested` forces `None`. The
   output line says which of the three it took. The test
   `restore_record_without_a_pre_restore_head_prints_unattested` becomes
   `restore_record_with_nothing_to_default_from_prints_unattested`, and
   `restore_record_defaults_the_pre_restore_head_to_the_witnessed_checkpoint`
   is added: after a `DELETE` of the chain and the checkpoints, the marker
   attests the head the witness holds.
3. After the marker commits, `witness.record_restore(space, idx, hash,
   operator, now)`; `Recorded` gains `witnessed: bool`, and a witness
   write that fails is reported as `NOT WITNESSED (..)` with exit 1 while
   the marker stands. A re-run on the same point completes the witness
   without `--again` (`AlreadyRecorded` is answered only when the restore
   row exists too). Test: `restore_record_completes_a_missing_witness_row_on_rerun`.
4. `--at` takes seconds (the built-note item 5 above).
5. The sink cursor reset R2e names, once R2e is built; until then the
   command prints `attest deliveries: cursor not reset (R2e)`.

**R2d · The witness and the boot assertion.** Ruled 2026-09-19
(`restore-witness`, option C, `corpus-rulings.md` "Four rulings from the
owner's answers of 2026-09-19"): every signed checkpoint head is also
recorded in the control-plane database, the boot check compares the space's
chain against that record, and the offline verifier also compares against
the heads the attestation receivers hold. The design below answers the five
questions the ruling leaves to the code, then splits into four increments,
R2d-i to R2d-iv. The classification it leans on (`agreement`, the bound
rule) is R2b′'s.

**Why `$checkpoints` alone cannot witness a restore.** `$checkpoints` is a
boot-time platform table (`COMMANDLOG_DDL`, `crates/ordo-exec/src/commandlog.rs:101`)
and lives in the space's own database (`plan-space-routing.md` §3;
`Pool::connect` creates it on every pool it opens). A point-in-time restore
of that database rolls the checkpoints back with the chain, the stored
checkpoints then agree with the stored chain, and `continuity` reads
`Intact`. What R2b detects today is a rewind that a checkpoint row survived:
a `DELETE`, a bundle re-import, a per-table `pg_restore`. The
whole-database case is the one PITR produces, and it is the one the boot
check has to see.

**Where the record lives.** A new table, `$chainwitness`, in the control
database, created on demand by its own module
(`crates/ordo-exec/src/witness.rs`, `Witness::ensure_table`) against the
control pool only, the way `$attestsinks` and `$spacedb` are, and never by
`ensure_platform_tables`, so a target database never gets one.

- *Single database.* The control pool is the pool the binary booted with
  (`assemble`'s `pool`, held as `Runtime.pool`), and it is the same
  database the space's `$checkpoints` are in. The witness then survives
  every restore that acts on rows or tables (a bundle re-import, a
  `DELETE`, a `pg_restore` of the space's tables) and does not survive a
  PITR of the whole database, because there is only one. That PITR is the
  control plane being restored, which the ruling assigns to the receivers
  (R2d-iv). `howto-operating.md` states this in the restore procedure.
- *Routed.* `AttachedPools.control` is the control pool
  (`crates/ordo-exec/src/route.rs:218`; `$spacedb` is read there), the
  default target is the control database (`plan-space-routing.md` Q-1),
  and an attached space's chain and checkpoints are on its target. The
  witness stays on the control pool, so a PITR of the target alone rolls
  `$commandlog` and `$checkpoints` back together and leaves the witness
  standing. That is the case the last test in R2d-iii builds. The host
  binary does not construct `AttachedPools` yet (`grep AttachedPools
  crates/ordo-host/src` finds nothing; every routed test builds one by
  hand), so `Spaces` takes the witness as a handle rather than deriving it
  from the resolver.

```sql
CREATE TABLE IF NOT EXISTS "$chainwitness" (
  "seq"       bigserial PRIMARY KEY,
  "space"     text   NOT NULL,
  "kind"      text   NOT NULL,   -- 'checkpoint' | 'restore'
  "upto"      bigint NOT NULL,
  "head"      bytea  NOT NULL,
  "at"        bigint NOT NULL,
  "key_id"    text   NOT NULL,
  "signature" bytea  NOT NULL,
  UNIQUE ("space", "kind", "upto", "head")
)
```

Every row is one statement: *this space's chain reached position `upto`
with head `head`.* A `checkpoint` row is a copy of a `$checkpoints` row,
signature included, so a reader holding the public key verifies it the way
it verifies the original (`CheckpointVerifier::verify_checkpoint` over the
same five fields). A `restore` row is written by `ordo restore --record`
(R2c) for the marker itself: `upto` is the marker's index plus one, `head`
is the marker entry's chain hash, `key_id` is the operator string,
`signature` is empty, `at` is the wall clock. `seq` orders the two kinds
against each other, and that order is what the boot check reads: a
checkpoint witnessed before the newest `restore` row was signed over
history that restore may have discarded; one witnessed after it was signed
over the history the chain holds now. The two writers never race for a
position, because the checkpoint worker is leader-only and a restore is
recorded while the space is refused.

**Who writes it, and what a failed write does.** `undo::sign_checkpoint`
writes it, **before** the checkpoint row, inside the append-lock critical
section but on the control pool: the two writes are ordered and not atomic,
since a routed deployment has no transaction across two databases. The
order is chosen for the invariant it gives: **no stored checkpoint without
a witness row**, unless the control database was itself restored. If the
witness write fails, `sign_checkpoint` returns
`CheckpointFailure::Unwitnessed(PoolError)`, the transaction rolls back and
nothing is signed; the worker reports the error and the next tick tries
again. If the checkpoint commit fails after the witness landed, the witness
names a head the chain holds (it was read under the lock from committed
rows), the boot check finds it `Current`, the next signing lands the same
or a later position, and the unique key absorbs a repeat. `sign_checkpoint`
also runs `continuity` (R2d-ii) before it signs and refuses with
`CheckpointFailure::WouldWitnessARewind(Continuity)` on `Rewound`, so a
chain restored under a running process is refused a checkpoint at the next
tick rather than witnessed as current.

**How the boot check reads it.** `undo::continuity(db, witness, space)`
(R2d-ii) reads three things: every `$checkpoints` row for the space from
the space's database, every `$chainwitness` row for the space from the
control pool, and the markers. Each checkpoint row from either table is
classified against the stored chain by R2b′'s `agreement`: `Current`,
`Orphaned` or `BelowFloor`. `Intact` when no row is orphaned; otherwise
every orphan needs an explanation, and the answer is `Rewound` when any
lacks one. A witness row is explained by a `restore` row with a greater
`seq` whose own `(upto, head)` is `Current`, which says the marker it names
is in the chain. A `$checkpoints` row is explained through its witness copy
when one exists (matched on `(upto, head)`), and otherwise by R2b′'s bound
rule over the markers, which is the rule a control database restored to
before the row was witnessed leaves available. At `assemble` the space is
`ORDO_SPACE` and the space pool and the control pool are both `pool`. At
`Spaces::resolve` the space pool is `pool_of(space)` and the witness is the
handle `Spaces` was built with.

**How a legitimate restore reconciles it.** `ordo restore --record` (R2c,
amended below) appends the marker, commits, and then writes the `restore`
row through the same `Witness` handle. A witness write that fails leaves
the marker in the chain and the space still refused, and the operator's
re-run completes it: `record_restore` on a point already recorded writes
the missing `restore` row without `--again` and reports `witnessed: true`.
The marker's `pre_restore_head` defaults to the newest witnessed checkpoint
of the space, which after a whole-database PITR is exactly the head the
restore discarded; only when the witness holds no checkpoint does the
default fall to the newest stored checkpoint, and only when neither exists
is it `None`.

**How the verifier reaches the receivers' heads.** It does not dial them.
`ordo-verify-proof` links no driver and no socket by design, outbound HTTP
belongs to `ordo-egress`, and a receiver is a party outside the deployment.
What a receiver holds is an `ordo.attestation/1` document
(`webhook::payload`, `crates/ordo-exec/src/webhook.rs:645`: `space`,
`upto`, `head`, `at`, `keyId`, `signature`, `signedBytes`), and
`ordo_verify_proof::verify_attestation` already checks one under a supplied
key. R2d-iv lets `ordo verify` take those documents as files
(`--attestation <file>`, repeatable) and classifies each head with the same
`agreement` the stored records get; `--control-db` lets it read the witness
too. The deployment's own copy of what it sent is not a substitute:
`$attestations` is in the space's database and rolls back with it, and
`$attestdeliveries` holds positions only and deletes its row on success.

**R2d-i · The witness table and its writer.** *Opus: the verifier reads
this table in R2d-iv, and its shape is fixed here. Depends on R2b′.*

- `crates/ordo-exec/src/witness.rs` (new):

  ```rust
  pub const WITNESS_TABLE: &str = "$chainwitness";
  pub const WITNESS_DDL: &str = /* the DDL above */;
  #[derive(Clone)]
  pub struct Witness { control: Pool }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum WitnessKind { Checkpoint, Restore }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Witnessed {
      pub seq: u64, pub kind: WitnessKind, pub upto: u64, pub head: ChainHash,
      pub at: Timestamp, pub key_id: String, pub signature: Vec<u8>,
  }
  impl Witness {
      pub fn new(control: Pool) -> Witness;
      pub async fn ensure_table(&self) -> Result<(), PoolError>;
      pub async fn record_checkpoint(&self, cp: &Checkpoint) -> Result<(), PoolError>;
      pub async fn record_restore(&self, space: &SpaceId, marker_idx: u64,
                                  marker_hash: ChainHash, operator: &str, at: Timestamp)
          -> Result<(), PoolError>;
      pub async fn rows(&self, space: &SpaceId) -> Result<Vec<Witnessed>, PoolError>; // by seq
      pub async fn latest_checkpoint(&self, space: &SpaceId)
          -> Result<Option<Witnessed>, PoolError>;
  }
  ```

  `record_*` run `ensure_table` first, in the retry-once shape
  `cadence::ensure_tables` uses; inserts are `ON CONFLICT DO NOTHING` on
  the unique key.
- `crates/ordo-exec/src/undo.rs`: `sign_checkpoint(pool, space, signer, at,
  witness: &Witness)`. The parameter is not optional, since an unwitnessed
  checkpoint is the thing the ruling removes. Order inside the transaction:
  lock; `continuity` (refuse `Rewound`); `choose_anchor`; `verify`;
  `head_of`; the `AlreadyAt` test; build `cp`;
  `witness.record_checkpoint(&cp)` (refuse `Unwitnessed`);
  `tx.put_checkpoint`; commit. `CheckpointFailure` gains
  `Unwitnessed(PoolError)` and `WouldWitnessARewind(Continuity)`, each with
  a `Display` that names the remedy.
- `crates/ordo-exec/src/cadence.rs` `checkpoint_due(pool, space, signer,
  at, witness)`; `crates/ordo-host/src/workers.rs` `CommandlogCheckpoint`
  gains `pub witness: Witness`, built where the worker is registered from
  `Runtime.pool`, which is the control pool.
- `crates/ordo-cli/src/operator.rs` `checkpoint`: flag `--control-db
  <name>`, defaulting to the value of `--db`; the witness is
  `Witness::new(control pool)`. `usage()` states the flag and the default.
- Every test that calls `sign_checkpoint` passes
  `&Witness::new(pool.clone())` (single database); `grep -rn
  'sign_checkpoint(' crates` lists them (`crates/ordo-cli/tests/operator.rs`,
  `crates/ordo-exec/tests/cadence_prune.rs`, the attestation tests in
  `crates/ordo-host/tests`).
- Tests, `crates/ordo-exec/tests/witness.rs` (new), against the shared
  cluster the way `cadence_prune.rs` runs:
  - `every_signed_checkpoint_has_a_witness_row_with_the_same_five_fields`:
    sign three checkpoints; `$chainwitness` holds three `checkpoint` rows
    equal to the `$checkpoints` rows field for field, in `seq` order, and
    `verify_checkpoint` passes on each row read back as a `Checkpoint`.
  - `a_witness_that_cannot_be_written_signs_nothing`: a `Witness` over a
    pool whose database has `$chainwitness` replaced by a view of the same
    name (an insert into it fails, and `CREATE TABLE IF NOT EXISTS` sees a
    relation and stands down); `sign_checkpoint` returns
    `Err(Unwitnessed(_))` and `$checkpoints` is unchanged. **Observed
    failing first:** before the change the checkpoint is stored.
  - `a_witness_row_whose_checkpoint_never_committed_is_current_and_harmless`:
    write a `checkpoint` witness row by hand for the real head;
    `continuity` reads `Intact`; the next `sign_checkpoint` stores the
    checkpoint and the unique key absorbs the repeat.
  - `signing_refuses_a_chain_the_witness_contradicts`: sign at 6; `DELETE`
    the chain and the checkpoints from index 4; `sign_checkpoint` returns
    `Err(WouldWitnessARewind(Rewound { .. }))`; record the restore (witness
    row included); signing succeeds.
- **Done when** the four pass, every existing `sign_checkpoint` caller
  compiles with a witness, and `cargo test -p ordo-exec --test
  cadence_prune` is green.

**R2d-ii · `continuity` reads the witness.** *Opus. Depends on R2d-i.
Small.*

- `crates/ordo-exec/src/undo.rs`:

  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum RecordSource { Stored, Witness }
  pub enum Continuity {
      Intact,
      Explained { marker_idx: usize, checkpoint_upto: u64, source: RecordSource },
      Rewound   { head: Option<usize>, checkpoint_upto: u64,
                  checkpoint_head: ChainHash, source: RecordSource },
  }
  pub async fn continuity(db: Db<'_>, witness: &Witness, space: &SpaceId)
      -> Result<Continuity, PoolError>;
  ```

  The rule, in order: (1) classify every `$checkpoints` row and every
  witness `checkpoint` row with `agreement`; (2) for each `Orphaned`
  witness row, look for a `restore` row with a greater `seq` whose
  `(upto, head)` is `Current`; (3) for each `Orphaned` stored row with no
  witness copy, apply R2b′'s bound rule over the markers; (4) the first
  unexplained orphan, highest `upto` first, is the `Rewound` answer, naming
  its source; (5) otherwise `Explained` names the highest explained orphan
  and the marker or restore row that explains it, or `Intact` when nothing
  was orphaned. `BelowFloor` rows are neither. No signature is checked
  here, as before.
- `Display` for `Rewound` says which database holds the record: *"the
  control database witnessed a checkpoint at 1300 (head c07a…) that space
  `s` no longer reaches; the head is 999. Record the restore (`ordo restore
  --record`) or restore the missing suffix."*
- Tests, `crates/ordo-cli/tests/operator.rs`, beside the R2b harness:
  - `a_witnessed_checkpoint_the_chain_no_longer_reaches_is_rewound_even_when_the_checkpoints_agree`:
    sign at 6; `DELETE FROM "$commandlog"` **and** `DELETE FROM
    "$checkpoints"` from 4, the shape a whole-database restore leaves;
    `continuity` over the space alone would read `Intact`, and this test
    asserts `Rewound { source: Witness, checkpoint_upto: 6, .. }`.
    **Observed failing first:** `Intact`.
  - `a_restore_row_after_the_witnessed_checkpoint_explains_it`: the same,
    then the marker plus `witness.record_restore`; `Explained { source:
    Witness, .. }`.
  - `a_checkpoint_witnessed_after_the_restore_is_a_fresh_rewind_even_below_the_attested_head`:
    the case the bound rule cannot see. A head at 100 witnessed; restore to
    3, marker attesting `(99, H100)`, restore row; regrow to 50, sign at
    50 (witnessed, `seq` above the restore row); rewind to 20 unannounced;
    `Rewound { checkpoint_upto: 50, source: Witness, .. }`. Under the bound
    rule alone `50 <= 100` reads as explained; the `seq` order says
    otherwise.
  - `a_restore_row_whose_marker_is_gone_is_itself_a_rewind`: record a
    restore, then delete the marker's row; `Rewound` naming the restore
    row's position.
- **Done when** those pass and R2b′'s `continuity` tests still pass with a
  `Witness` over the same pool.

**R2d-iii · The two boot sites.** *Sonnet, with one rule reviewed by
whoever built R2d-ii: the cache holds passing verdicts only. Depends on
R2d-ii; R2c is what lifts the refusal.*

Nothing can stop someone with `psql`. What the platform can do is refuse to
serve a chain that has moved backwards without saying so. This moves the
finding from a command an operator may never run to the path every request
takes.

> **Superseded in part, 2026-09-21 (17aa).** The per-space refusal below is
> exactly what shipped and is unchanged. The *process-wide* one is gone:
> `BootRefusal::Rewound` stopped the whole deployment for one tenant's
> history, which made the control plane depend on a tenant and removed the
> door through which `ordo restore --record` explains the rewind. `assemble`
> now calls `reconcile_at_boot`, which asks `Spaces` the same question and
> quarantines what it finds instead of refusing. Read the first bullet as
> history.

- `crates/ordo-host/src/runtime.rs`: `BootRefusal::Rewound(Continuity)`,
  `Display` naming the record, its source and the two lawful remedies
  (`ordo restore --record`, or restore the missing suffix). In `assemble`,
  after `ensure_platform_tables` and before the drain starts:
  `undo::continuity(Db::Pool(&pool), &Witness::new(pool.clone()), &space)`;
  refuse on `Rewound`. The handle is kept as `Runtime.witness` for the
  worker registration and for `Spaces`.
- `crates/ordo-host/src/spaces.rs`: `SpaceRefusal::Rewound { space: String,
  continuity: Continuity }`; `Spaces::routed(deployment, pools, witness:
  Witness)`; `Spaces::new` builds
  `Witness::new(deployment.current().exec.pool.clone())`; `Spaces` gains
  `continuous: Mutex<HashSet<String>>`. In `resolve`, after `pool_of` and
  before `Bound` is built: if the space is not in the set, run
  `continuity(Db::Pool(&pool), &self.witness, space)` on **that space's
  pool**; insert on `Intact` or `Explained`; refuse on `Rewound` without
  inserting, so the next request after `ordo restore --record` re-checks
  and serves. The refusal reaches the wire as `internal` through the
  existing mapping (`lib.rs:680-685`), and the message names the space.
  The check runs once per process for a serving space; a space restored
  under a running process is caught by the checkpoint worker's next tick
  (R2d-i) for `ORDO_SPACE`, by `ordo verify --control-db` for any space,
  and at the next boot.
- Tests:
  - `crates/ordo-host/tests/runtime.rs`
    `a_rewound_space_refuses_the_boot_until_its_restore_is_recorded`: seed
    `ORDO_SPACE`, sign a checkpoint (witnessed), `DELETE` the chain and the
    checkpoints from index N, `assemble` → `Err(BootRefusal::Rewound(_))`
    whose text names the control database and the position;
    `undo::record_restore` with the witness; `assemble` → `Ok`. **Observed
    failing first:** `assemble` returns `Ok` on the rewound database.
  - `crates/ordo-host/tests/spaces.rs`
    `a_rewound_tenant_is_refused_at_resolve_and_served_after_the_record`,
    on a space other than `ORDO_SPACE`, asserting the check ran once per
    process for a serving space (a counter on `Spaces` exposed for tests).
  - `a_space_with_no_record_is_never_refused`: no checkpoint and no witness
    row means nothing to fire on.
  - `crates/ordo-host/tests/restore_witness.rs` (new)
    `a_whole_database_restore_of_an_attached_space_is_caught_at_resolve`,
    the test the ruling asks for. Harness from `provision_target.rs`
    (`control`, `on`, `prepare`, `target_pool`, `drop_database`) and
    `spaces.rs`'s `harness_with` over `AttachedPools::new(control, control,
    template, Installed::new().as_configured(label))`: provision a space
    onto its own target database; append N groups; `sign_checkpoint` with
    the witness on the control pool; drop the target pool and `CREATE
    DATABASE "<target>_backup" TEMPLATE "<target>"`; reconnect, append M
    more, sign again; drop the target and `CREATE DATABASE "<target>"
    TEMPLATE "<target>_backup"`. Assert, in order: `continuity` over the
    target with an empty `Witness` reads `Intact` (the failure this table
    exists to close, asserted so it stays visible); `Spaces::resolve` →
    `Err(SpaceRefusal::Rewound { .. })` whose continuity has `source:
    Witness` and `checkpoint_upto: N + M`; `record_restore` through the
    witness → `resolve` → `Ok`, and the marker's `pre_restore_head` equals
    the second checkpoint's `(N + M - 1, head)` without the test naming it.
    **Observed failing first:** with the witness read removed from
    `continuity`, `resolve` returns `Ok` on the restored target.
- **Done when** those pass and every existing `runtime.rs`, `spaces.rs` and
  `provision_target.rs` test still passes.

**R2d-iv · The verifier reads the witness and the receivers' heads.**
*Opus. Depends on R2d-ii.*

- `crates/ordo-exec/src/undo.rs`:

  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum RecordOrigin { Stored, Witness { seq: u64 }, Receiver { index: usize } }
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct RecordVerdict {
      pub origin: RecordOrigin, pub upto: u64, pub head: ChainHash,
      pub agreement: Agreement,
      /// The marker index, or the restore row's `seq`, that explains an orphan.
      pub explained_by: Option<Explanation>,
  }
  pub async fn classify_records(db: Db<'_>, witness: Option<&Witness>, space: &SpaceId,
                                receivers: &[(u64, ChainHash)])
      -> Result<Vec<RecordVerdict>, PoolError>;
  ```

  A receiver's record is explained by the bound rule only: no `seq` exists
  outside the deployment.
- `crates/ordo-cli/src/operator.rs` `verify`: flags `--control-db <name>`
  (the witness; with no value the output says `witness  not read
  (--control-db)`) and `--attestation <file>` (repeatable; each parsed by
  `ordo_verify_proof::verify_attestation` under the supplied key, structure
  only without one and the line then says `unverified`; a document naming
  another space is a usage error, exit 2). Output, after the `restore` and
  `orphaned` lines: one `witness` line per witnessed checkpoint that is not
  `Current` (`witness   checkpoint upto 1300 head c07a… orphaned, explained
  by restore at index 1000`) and one `receiver` line per file (`receiver
  attest-1290.json: upto 1290 head 8e1f… current`, or `orphaned, explained
  by …`, or `below the floor`). An unexplained orphan from any origin
  prints `FAILED  the chain no longer reaches a head that <origin>
  recorded: upto N head H` and exits 3. `docs/howto-operating.md`
  "Verifying the Audit Chain" shows the lines and states the reading: a
  receiver's head the chain has moved away from, with no marker attesting a
  pre-restore head at or above it, is a restore nobody recorded, whatever
  the deployment's own records say about each other.
- Tests, `crates/ordo-cli/tests/operator.rs`:
  - `verify_reads_the_witness_and_fails_on_an_unrecorded_whole_database_restore`:
    sign at 6; delete the chain and the checkpoints from 4; `ordo verify
    --control-db` exits 3 and names the witnessed head; append the marker
    and the restore row; exit 0 with the `witness` line reading
    `explained`.
  - `verify_compares_a_receivers_attestation_against_the_chain`: build a
    payload with `webhook::payload` for a signed head at 6 and write it to
    a file; rewind to 4 with a marker attesting `(3, H4)`, below the
    receiver's head; exit 3 naming the file; re-record with `--again
    --pre-restore-head 5:<H6>`; exit 0 with the line reading `orphaned,
    explained`.
  - `a_receivers_attestation_below_the_floor_is_reported_not_judged`: prune
    past its position; the line reads `below the floor` and the exit code
    is unchanged.
  - `an_attestation_for_another_space_is_a_usage_error` (exit 2).
- **Done when** those pass, `ordo verify` without the new flags prints
  exactly what it printed before, and the howto shows the lines.

**R2e · Attestation delivery after a restore.** *Opus. Small. Depends on
R2b′ and R2c.* Found while designing R2d-iv. `$attestations` is keyed
`("space", "upto")` and `attest_due` inserts with `ON CONFLICT DO NOTHING`
(`crates/ordo-exec/src/cadence.rs:423`), so a head the chain reaches again
after a restore, at a position an orphaned attestation already holds, is
never recorded; and `enqueue_due` (`webhook.rs:409`) fans out only rows
with `upto > sink.cursor`, so even a recorded one at a position the cursor
has passed is never delivered. A receiver therefore never hears the
post-restore history at reused positions, and R2d-iv's comparison would run
over heads the receiver was never sent.

- `crates/ordo-exec/src/cadence.rs`: the `$attestations` key becomes
  `("space", "upto", "head")`, by the idempotent `ALTER` shape R2b′ uses
  for `$checkpoints`; `attest_due` classifies the stored attestation at
  `upto` with `agreement`, and an orphan there does not make the head
  `NotDue`: the new one is stored beside it.
- `crates/ordo-exec/src/webhook.rs`: `$attestdeliveries` and
  `$attestdeadletters` gain `"head" bytea NOT NULL DEFAULT ''` (`ADD COLUMN
  IF NOT EXISTS`) and a unique index over `(space, upto, head)`;
  `enqueue_due` fans out every attestation that is `Current` against the
  chain and has neither a delivery row nor a dead letter under its
  `(upto, head)`; the sink's `cursor` stays as the ops surface's number,
  and `record_restore` (R2c) sets it to `restored_from + 1` so the change
  is visible to an operator reading the row.
- Tests, `crates/ordo-host/tests/attest_webhook.rs`:
  `a_post_restore_head_at_a_reused_position_is_delivered` (attest at 6;
  restore to 3; regrow to 6; attest; the outbox holds a second delivery for
  position 6 with the new head, and the receiver's second payload verifies)
  and `an_orphaned_attestation_is_kept_and_not_redelivered`.
- **Done when** those pass and `attest_webhook.rs`'s existing tests pass
  unchanged.

### R3 · the drill, as a test

*Sonnet. Depends on R1a, R2b, R2c.* `implementation.md`'s five-step drill,
executable, in `crates/ordo-host/tests/restore_drill.rs` (in `ordo-host`
because step 2 exercises `assemble`'s refusal; the marker is written through
`undo::record_restore`, the library half of R2c, since `ordo-host` does not
depend on `ordo-cli`).

**The rewind mechanism, ruled.** A cargo test cannot run `pg_basebackup` and
a WAL restore against the shared cluster the suites use. The drill takes its
backup as a Postgres template copy, `CREATE DATABASE "<db>_backup" TEMPLATE
"<db>"`, with the pool dropped first (a template copy needs no other
connection to the source), and restores by dropping the original and copying
the backup back under its name. That is a real point-in-time copy of the
whole database, which is the property the drill's assertions need. PITR
proper is the deployment's instrument and the howto's procedure; the drill
proves the platform's half (marker, verifier, assertion, bundle) runs
end to end.

**Amended 2026-09-19 (`restore-witness`).** The copy has to be of the
space's database while the witness stays where it is. In a single database
the template copy restores `$chainwitness` with the chain, step 2 reads
`Intact`, and the drill would prove nothing about the boot assertion. The
drill therefore provisions its space onto an attached target
(`provision_target.rs`'s harness, the shape R2d-iii's
`restore_witness.rs` test uses), copies and restores that target, and
leaves the control database alone. Step 2's `Err(Rewound)` then comes from
the witness (`source: Witness`), which the drill asserts; step 5 additionally
runs `ordo verify --control-db --attestation` over a payload cut at step 1
(R2d-iv) and expects exit 0 with the receiver line reading `orphaned,
explained`. The drill depends on R2d-iii and R2d-iv as well as on R1a, R2b
and R2c.

1. Seed a space, write N groups, sign a checkpoint, export a bundle (R1a's
   digest in hand), take the template copy. Write M more groups. Restore.
   **Record wall time** and print it with `eprintln!` labelled *not a
   gate*.
2. `assemble` → `Err(Rewound)`. `record_restore`. `assemble` → `Ok`.
   `undo::verify` from `choose_anchor` → exit-0 shape with **exactly one**
   discontinuity at exactly `N` (the marker's index) and no other finding;
   `verify` from `Anchor::Genesis` agrees.
3. `Pool::live_drift` against the restored schema (`shape.rs:260`), empty.
4. One read per entity against a known row set: rows equal the pre-backup
   snapshot exactly; the M post-backup groups are gone from `$commandlog`
   **and** from `$sealedpayload` (the anti-join's other half).
5. The step-1 bundle verifies standalone against its manifest digests
   (re-hash every member), and its `commandlog.jsonl` prefix recomputes to
   the hashes the restored chain holds below the marker, link by link.

The wall time from a laptop cluster is **not** the release gate and the test
must not record it as one; the same discipline 5f states for the other two
measurements. The test proves the drill *runs*; a deployment produces the
number.

**Done when** the drill runs as one `cargo test` against a real Postgres and
passes all five steps, and step 2 fails the build if the marker is missing,
doubled, or at the wrong index: three variants of the test
(`the_drill_fails_when_the_marker_is_missing`, `..._doubled`,
`..._misplaced`) each assert the specific failure.

### Ruled out (R)

- **A silent restore.** R2d. This is the one thing the audit spine must
  never allow, and the mechanism is refusal-to-serve, not a log line.
- **`system:restore` as a real command group.** §2.3: it would be a forged
  write, and `EmptyGroup` refuses the alternative.
- **A `TamperKind::Restored` variant.** A lawful event must not share a type
  with an alarm; the CLI's exit code 3 pages the security team by design
  (`operator.rs:31-47`).
- **A `$restores` index table.** The actor column already locates a marker
  and the body confirms it; a second table would be a stored copy of a fact
  the chain holds, and one an operator could edit.
- **Deleting orphaned checkpoints.** Named in the spec as exactly what a
  dishonest operator would do.
- **Signing the bundle.** A digest is enough for the marker to name one, and
  a signature raises the key-custody question `8h` settled for checkpoints
  and would need its own answer here. Deferred with the reason, not omitted.
- **The witness in the space's database.** A second copy of `$checkpoints`
  beside the first restores with it; the record has to sit in a database a
  space restore does not touch, and in a single-database deployment that
  database does not exist, which the howto says rather than hides.
- **The verifier dialling the receivers.** `ordo-verify-proof` links no
  driver and no socket, outbound HTTP is `ordo-egress`'s alone, and a
  receiver's copy is evidence exactly because the deployment cannot reach
  it. The receiver's document comes in as a file (R2d-iv).
- **Witnessing attestations.** The witness holds what anchors, which is a
  checkpoint; the receivers hold the attestations, and R2e keeps their
  copies complete across a restore.
- **A step-up on `ordo restore --record`.** §2.4: the door is closed when
  the marker is needed; the CLI attributes, and the correlation joins the
  marker to the retained security stream.
- **A restore that replays the missing suffix.** Out of scope: the M groups
  after the restore point are gone, and pretending otherwise is the rewind
  the marker exists to disclose.

---

## 3 · Track M — metering & billing substrate

**Re-verified 2026-09-19 against `a2cdfe6`: nothing of M1 to M3 was built
then. Built 2026-09-20, all of it** (17n): the in-process ledger, the
per-replica flush, the leader rollup, the quota gate that runs after the
access check, and `ordo usage` with its `/usage` route. The survey below
is kept because it is what the build was made against.
What the plan of 2026-09-08 leaned on has moved in four places, and two of
its mechanisms turn out to count the wrong thing; both are corrected below
with the reason.

### 3.1 The starting state, re-read

Nothing counts usage. `grep -rn 'quota\|billing\|rate.limit' crates/` finds
the definition language's own `used + 1` pattern in two conformance tests,
the `usage.rollup` slot, and prose.

| Claim as written on 2026-09-08 | State on 2026-09-19 |
|---|---|
| "both ride the supervisor `plan-runtime.md` R3 defines" | **Built** (17d, finished 2026-09-18). `Worker`, `Cadence::Schedule { name, period }`, `WorkerCtx`, `Progress::more`, `on_shutdown` in `crates/ordo-host/src/supervisor.rs:118-260`; the loop claims a schedule with the worker's own period (`:905-919`). `usage.rollup` is a registered **`Slot`** (`workers.rs:449-473`) whose census reason names this plan; `tests/supervisor.rs:423` asserts it is `enabled: false`, and that assertion is what M1 flips. |
| "requests by cost class and rows returned come from the door and the executor decorator" | Both come from **`MeteredExecutor`** alone (`crates/ordo-host/src/metrics.rs:810-856`): it sees `op.cost.class`, the bound `space`, and the result's `rows.len()`. The router (`gate`, `router.rs:593-626`) needs no change for M1. |
| "writes come from one new `AppendObserver` method" | **Withdrawn.** See §3.2a: writes are read from the chain at rollup time. |
| "worker executions come from `$triggerruns`, which is already per-space" | **Wrong.** `finish_run` deletes the row (`crates/ordo-exec/src/triggers.rs:579-587`), so at rollup time a finished run leaves nothing to count. Runs are counted in process by `TriggersRunner` (`workers.rs:113-165`), which has the `Run` and its space in hand at the moment it finishes or fails one. |
| "blob bytes are `unbuilt` and reported as a named zero" | **Retired.** `$blobref.bytes` is a plaintext `bigint` (`crates/ordo-crypto/src/blobref.rs:112`; only `mime`, `size` and `integrity` are sealed), so `SUM("bytes")` per space is exact and free. |
| "exact `SUM(octet_length(..))` over `$sealedpayload` and `$commandlog`, grouped by space" | Still the method, with one addition: a space's rows can be in an attached database, so the rollup walks every target `PoolResolver::targets` lists (`crates/ordo-exec/src/route.rs:82-93`) rather than one pool. |
| `MAX_SERIES = 10_000` (`metrics.rs:169`), the space label opt-in (`with_per_space_series`), "a deployment that wants no metering simply does not wrap" (`:781`, `:859`) | All still true; line numbers updated. |
| `$jobs` has no space column | Still true (`jobs.rs:45-50`). |

### 3.2 Why the ledger is not a stream

The tempting move is a third append-only hash-chained stream beside the
command log and the security-event stream. It is wrong, for three reasons
that are each disqualifying:

1. **A chain would take the per-space append lock on every request.** That
   lock is the deployment's known serialization point, and 5f built two
   histograms specifically to watch it. Putting a usage record behind it
   makes metering a throughput tax on the exact axis the platform already
   worries about.
2. **Usage is not evidence about anyone's rights.** The two existing streams
   are tamper-evident because someone will later dispute what they say about
   a person's actions. A usage total is an accounting quantity: it gets
   recomputed, corrected, back-filled after a replica crash, and aggregated
   across replicas. In a hash chain, a billing correction is a rewrite.
3. **The counts are already visible where they happen.** The executor
   decorator sees requests, the chain holds every write, the runner sees
   every trigger run.

**So: usage is a periodic per-space rollup, fed by two sources with
different durability, drained by two supervised workers.** No stream, no new
bus, no new scheduler.

### 3.2a Two sources, two workers, two tables

The 2026-09-08 text had one worker on `claim_schedule("usage.rollup")`
draining one in-process accumulator. That does not work with the supervisor
as built: a schedule-claimed worker is **leader-only**, and an in-process
counter lives in **every** replica. The leader cannot read the followers'
memory. So the design splits along durability:

- **Per-replica quantities** (requests by class, rows returned, worker
  runs) exist only in a process's memory until written. Every replica
  flushes its own, on a tick, into rows keyed by replica. A crashed replica
  loses at most one flush interval, and its last row says so.
- **Per-space quantities** (writes, rows written, bytes stored) are
  durable already or are a measurement of the database. Writes are the
  chain: `$commandlog` between two positions is the exact, committed,
  replica-independent record of every write, and its body carries the
  command count at a fixed offset. The leader reads them once per period.

**Why writes moved from the choke point to the chain.** An
`AppendObserver::appended` call inside `append_group` runs before the
transaction commits, so it counts a group whose transaction later rolls
back (a serialization conflict after the append is the ordinary case), and
it lives in a per-process counter that a crash discards. The chain has
neither problem and already exists. The observer seam stays what it is:
two timings.

### M1 · the metering ledger

Three increments. All Sonnet-tier; the only judgement is in §3.2a, which is
settled.

**M1a · The in-process ledger.** *Sonnet. Depends on nothing.*

- `crates/ordo-host/src/metrics.rs`:

  ```rust
  #[derive(Default, Clone, Debug, PartialEq, Eq)]
  pub struct UsageCounters {
      pub requests: BTreeMap<&'static str, u64>,   // by CostClass::as_str()
      pub rows_returned: u64,
      pub worker_runs: u64,
  }
  // on Metrics: None when the ledger is off, so an unwrapped deployment
  // allocates nothing per space.
  usage: Option<Mutex<BTreeMap<String, UsageCounters>>>,
  pub fn with_usage_ledger(mut self, on: bool) -> Metrics;   // env: ORDO_USAGE_LEDGER=1
  pub fn usage_enabled(&self) -> bool;
  pub fn usage_request(&self, space: &str, class: CostClass, rows: Option<usize>);
  pub fn usage_worker_run(&self, space: &str);
  pub fn usage_take(&self) -> BTreeMap<String, UsageCounters>;   // swap with empty
  ```

  `Default` reads `ORDO_USAGE_LEDGER` the way `space_labels` reads
  `ORDO_METRICS_PER_SPACE` (`:280-283`): off unless set. Every method is a
  `Mutex` and a map insert, within the budget `ordo_exec::metrics` states.
- `MeteredExecutor::execute` (`:830-856`): after timing, call
  `usage_request(space, op.cost.class, rows)` where `rows` is
  `Some(r.rows.len())` on `Ran`, `None` otherwise. Every executor outcome
  counts as a request; only `Ran` adds rows. Door refusals never reach the
  executor, which is the rule "counting denials prices the act of being
  attacked" needs and gets for free.
- `TriggersRunner::run` (`workers.rs:127-165`): `ctx.metrics.usage_worker_run(&run.space.0)`
  once per run, after `finish_run` or `fail_run` (an attempt executed a
  body either way).
- Tests, unit tests in `metrics.rs` beside `MAX_SERIES`'s:
  `usage_is_off_by_default_and_take_returns_empty`,
  `usage_counts_requests_by_class_and_rows_only_on_ran`,
  `usage_take_drains_and_a_second_take_is_empty`.
  Integration, `crates/ordo-host/tests/usage.rs` (new; copy the harness of
  `tests/exit_ops.rs`): `a_read_through_the_door_is_counted_by_class_and_rows`.
- **Done when** those pass with the ledger on and every existing metrics
  test passes with it off.

**M1b · The tables and the per-replica flush.** *Sonnet. Depends on M1a.*

- `crates/ordo-exec/src/usage.rs` (new module, `pub mod usage` in
  `lib.rs`; its DDL joins `ensure_platform_tables`, `lib.rs:450-464`):

  ```sql
  CREATE TABLE IF NOT EXISTS "$usage_replica" (
    "space" text NOT NULL, "period_start" bigint NOT NULL,
    "period_end" bigint NOT NULL, "replica" text NOT NULL,
    "requests" jsonb NOT NULL,          -- {"sql": n, "blind_index": n, ...}
    "rows_returned" bigint NOT NULL, "worker_runs" bigint NOT NULL,
    "state" text NOT NULL,              -- 'open' | 'closed'
    "method_version" text NOT NULL,
    PRIMARY KEY ("space", "period_start", "replica"));
  CREATE TABLE IF NOT EXISTS "$usage" (
    "space" text NOT NULL, "period_start" bigint NOT NULL,
    "period_end" bigint NOT NULL,
    "chain_from" bigint NOT NULL, "chain_to" bigint NOT NULL,
    "writes" bigint NOT NULL, "rows_written" bigint NOT NULL,
    "bytes_sealed" bigint NOT NULL, "bytes_chain" bigint NOT NULL,
    "bytes_blob" bigint NOT NULL, "bytes_rows_estimated" bigint NOT NULL,
    "method_version" text NOT NULL,
    PRIMARY KEY ("space", "period_start"));
  ```

  ```rust
  pub const METHOD_VERSION: &str = "ordo.usage/1";
  pub fn period_of(now_secs: u64, period_secs: u64) -> (u64, u64);
  pub struct ReplicaRow { .. }   // one field per column
  pub struct SpaceRow { .. }
  pub async fn add_replica(pool: &Pool, row: &ReplicaRow) -> Result<(), PoolError>;
      // INSERT .. ON CONFLICT (space, period_start, replica) DO UPDATE
      // SET requests = merged, rows_returned = +, worker_runs = +, state = EXCLUDED.state
  pub async fn close_replica_periods(pool: &Pool, replica: &str, before: u64) -> Result<u64, PoolError>;
  pub async fn upsert_space(pool: &Pool, row: &SpaceRow) -> Result<(), PoolError>;
  pub async fn last_space_row(pool: &Pool, space: &str) -> Result<Option<SpaceRow>, PoolError>;
  pub async fn between(pool: &Pool, space: &str, from: u64, to: u64)
      -> Result<(Vec<SpaceRow>, Vec<ReplicaRow>), PoolError>;
  pub fn command_count(body: &[u8]) -> Option<u64>;
      // the u64 after two length-prefixed strings; tested against decode_group
  ```

  Both tables live in the **control database** (`ctx.pool`): they are the
  answer to "what did this deployment serve", so they cannot live inside a
  tenant's database.
- `crates/ordo-host/src/runtime.rs`: `RuntimeConfig.replica: String` from
  `ORDO_REPLICA_ID`, default eight random hex characters per process
  (`ordo_crypto::fill_random`), printed in the startup log. A replica that
  restarts is a new replica; sums still add.
- `crates/ordo-host/src/workers.rs`: `UsageFlush { replica: String, period:
  Duration }`, `Cadence::Tick(60 s)`, `requires_leadership: false`. `run`:
  `usage_take()`, one `add_replica` per space for the current period with
  `state = 'open'`, then `close_replica_periods(replica, period_start)` so
  this replica's rows for earlier periods read `closed`. `on_shutdown`: the
  same, then close the current period too. A row still `open` after its
  period ended is a replica that died before it could say so, following
  `EventKind::EventsDropped { lost }`'s posture of recording the loss rather
  than smoothing it.
- Registration in `start_supervisor` (`runtime.rs:1553-1700`): when
  `rt.metrics.usage_enabled()` push `UsageFlush`; otherwise the census
  reads `("usage.flush", "ORDO_USAGE_LEDGER is not set")`.
- Tests, `crates/ordo-host/tests/usage.rs`:
  `a_flush_writes_one_open_row_per_space_and_closes_the_previous_period`,
  `two_replicas_in_one_period_are_two_rows_that_sum` (two `Metrics`
  registries, two replica ids, one pool),
  `shutdown_closes_the_current_period`,
  `command_count_agrees_with_decode_group_on_every_stored_body`.
- **Done when** those pass and `without_a_run_handler_the_runner_does_not_start`
  (`tests/supervisor.rs:404`) is updated to expect `usage.flush` and
  `usage.rollup` disabled with the ledger reason.

**M1c · The leader rollup.** *Sonnet. Depends on M1b.*

- `crates/ordo-host/src/runtime.rs`: `Runtime` gains `pub spaces:
  Arc<crate::spaces::Spaces>` (it is a local of `assemble` today, `:960`);
  the worker needs its `pools()`.
- `crates/ordo-host/src/workers.rs`: `UsageRollup { spaces: Arc<Spaces>,
  period: Duration }`, `Cadence::Schedule { name: "usage.rollup", period }`,
  `requires_leadership: true`. `run`, per target in
  `spaces.pools().targets()` (an unreachable target is skipped and named in
  the returned `WorkerError`, never silently): `SELECT "space", max("idx")
  FROM "$commandlog" GROUP BY "space"`; per space, `chain_from` is the last
  `$usage` row's `chain_to` (or the smallest stored `idx`), `chain_to` is
  the head; `writes` and `rows_written` are counted over `("chain_from",
  "chain_to"]` reading bodies in windows of `VERIFY_BATCH` with
  `command_count`; `bytes_sealed = SUM(octet_length("payload"))` over
  `$sealedpayload`, `bytes_chain = SUM(octet_length("body"))` over
  `$commandlog`, `bytes_blob = SUM("bytes")` over `$blobref`, all `WHERE
  space = ?`; `bytes_rows_estimated` is, per entity table the live schema
  names, `count(*) WHERE "$space" = ?` times the table's mean row width
  (`pg_relation_size / greatest(reltuples, 1)` from `pg_class`), summed,
  and it is the only estimated column. `Progress::more` when a target had
  more than 64 spaces, so one pass is bounded.
- Remove `usage.rollup` from `Slot::all()` (`workers.rs:455-473`); register
  `UsageRollup` when the ledger is on, otherwise the census reason above.
- Tests, `crates/ordo-host/tests/usage.rs`:
  `a_periods_rows_reconstruct_what_the_test_issued` (N reads across two
  cost classes, M writes carrying K commands in total, J trigger runs
  through a test `RunHandler`; after flush and rollup, the sums over
  `$usage_replica` and the `$usage` row equal N, K, M, J exactly);
  `a_second_rollup_continues_from_chain_to_and_counts_nothing_twice`;
  `bytes_stored_names_its_method_and_the_estimated_column_alone_is_estimated`;
  `the_rollup_walks_every_attached_target` (harness from
  `tests/provision_target.rs`); `usage_rollup_is_no_longer_a_slot`.
- **Done when** those pass, across a replica restart mid-period (the test
  builds a second `Metrics` under a second replica id and asserts the
  period's sums).

### M2 · quotas

**The class decision: `quota_exceeded` is added, and only in the increment
that ships its producer.** The wire inventory is closed in both directions
and the enforcing test says why: *"a class with no producer at all is the
recurring defect this whole file exists to catch"*
(`crates/ordo-api/tests/inventory_is_closed.rs:29-39`). `SPECIFIED_NOT_BUILT`
is empty (`:39`) and this plan does not put anything on it. The precedent is
`delegation_budget`, which SH-20 deliberately left unminted because nothing
counted (`docs/plan-delegation-surface.md:414-419`).

Shipping it means all three: the variant in `ErrorCode` + `ALL` + `as_str`
+ status (429, `StatusCode::TOO_MANY_REQUESTS`, the first use of it in
`crates/ordo-api/src/error.rs`), the row in `docs/api-engine-design.md` §4,
and the three inventory tests green (`inventory_is_closed.rs`,
`crates/ordo-cli/tests/interface.rs:450`,
`crates/ordo-host/tests/interface_surface.rs:346`).

It is **one class, distinct from `delegation_budget`**: a space over its
plan and a delegation over its grant are different limits with different
remedies, and collapsing them would send an operator to the wrong page.

- **Soft limits.** A quota has a limit and a grace: over the limit raises a
  warning on the deployment surface and continues; over limit + grace
  refuses. A hosted plan that hard-stops at the boundary is a product
  decision the platform should not make for every deployment.
- **Enforcement at the door, before planning.** In `gate()`
  (`crates/ordo-api/src/router.rs:593-626`), immediately **after**
  `check_access` (`:615`) and never before. A quota refusal handed to a
  non-member discloses that the space exists and is busy; an
  `access_denied` discloses nothing. Order matters and this is the reason.
- **The seam.** `ordo-api` depends on no sibling crate by design
  (`router.rs:381`), so `QuotaGate` is a trait declared in `spec.rs` beside
  `AccessChecker` (`:607`) and implemented in `ordo-host` over M1's rows.
  Same shape, no new dependency edge. Like `AccessChecker::check`, it is
  synchronous and pure: it reads a snapshot a worker refreshes, so a
  database outage cannot become a door outage.
- **Quantities in v1:** `requests` and `rows_returned` per period, from
  `$usage_replica` sums plus this process's live counters. `writes` is a
  leader-rolled quantity and lags a rollup period; a quota on it is
  deferred, with the reason, until someone needs it.
- **The unlock, worth naming:** SH-20's rate and volume caps become
  buildable on this substrate. They were deferred for exactly one reason,
  the metrics could not answer "rows affected per hour per delegation", and
  M1's `rows_written` from the chain is that number, keyed by the
  delegation the actor rendering already carries.

**M2 · One increment, two halves in one change.** *Sonnet. Depends on M1b.*

- `crates/ordo-api/src/spec.rs`:

  ```rust
  pub enum QuotaDecision {
      Within,
      Warn { quantity: &'static str, used: u64, limit: u64 },
      Exceeded { quantity: &'static str, used: u64, limit: u64, grace: u64 },
  }
  pub trait QuotaGate: Send + Sync {
      fn check(&self, space: &str, op: &OpSpec) -> QuotaDecision;
  }
  pub struct NoQuota;   // always Within
  ```

  `ApiState.quota: Arc<dyn QuotaGate>` (14 construction sites in the tree,
  `grep -rn 'ApiState {' crates`; each gains `quota: Arc::new(NoQuota)`).
  In `gate()`, after `check_access`: `Exceeded` is
  `ApiError::new(ErrorCode::QuotaExceeded, "space is over its quota for
  <quantity>").with_op(..)`; `Warn` and `Within` pass. The message names
  the quantity and nothing about other spaces.
- `crates/ordo-api/src/error.rs`: `QuotaExceeded`, `"quota_exceeded"`,
  429, in `ALL`. `docs/api-engine-design.md` §4: the row, in the table's
  form, with the sentence *"raised only after the access check, so a caller
  who is not a member of the space is answered `access_denied` and learns
  nothing about the quota"*.
- `crates/ordo-host/src/quotas.rs` (new): DDL `$quotas ("space",
  "quantity", "limit", "grace", "period_secs", PRIMARY KEY (space,
  quantity))` in the control database; `PgQuotas { limits: RwLock<..>,
  used: RwLock<..>, metrics: Arc<Metrics> }` implementing `QuotaGate`; a
  `refresh(&self, pool)` the `UsageFlush` worker calls on every tick after
  writing its rows (`$quotas` rows plus `SUM` over `$usage_replica` for the
  current period per space); `check` adds the live counter from
  `Metrics` to the snapshot, records `ordo_quota_over_limit_total{space,
  quantity}` on `Warn`. `ordo quota set --space --quantity --limit --grace
  --period-secs` and `ordo quota show --space` in `operator.rs`, dispatch
  list and usage in `main.rs`.
- Wiring: `ApiState.quota` is `PgQuotas` when the ledger is on and
  `NoQuota` otherwise; a deployment with no `$quotas` row is never refused.
- Tests: `crates/ordo-host/tests/quotas.rs`
  `over_quota_plus_grace_is_refused_at_the_door_and_a_non_member_still_gets_access_denied`
  (one op, two callers, two different refusals, no plan compiled: assert
  through the executor counter that nothing ran);
  `within_grace_warns_and_serves`; `no_quota_row_never_refuses`;
  `the_gate_reads_a_snapshot_and_never_the_database` (drop the pool, the
  check still answers). The three inventory tests, green.
- **Done when** all of those pass with `SPECIFIED_NOT_BUILT` still empty.

### M3 · the billing boundary

**The platform emits usage records. It does not invoice.**

- `ordo usage --space <s> --from <secs> --to <secs> [--db]` emits JSONL: one
  record per `(space, period_start)`, keys sorted, `"schema":
  "ordo.usage/1"`, `method_version` on every record, the space row's
  columns, `replicas: [{replica, requests, rows_returned, worker_runs,
  state}]` sorted by replica, and `totals` summed across replicas. No clock
  beyond the period bounds. A record is a fact about a period, and two runs
  over the same closed period are byte-identical.
- Also served at `/usage?space=&from=&to=` on `platform_router`
  (`serve.rs:182`), gated by `authorize(state, headers, USAGE_OP)` with
  `pub const USAGE_OP: &str = "platform.usage"` beside `METRICS_OP`
  (`:70`), same body, `application/x-ndjson`.
- **Explicitly out of scope, and it is a boundary rather than a backlog:**
  pricing, plans, invoices, payment, dunning, tax, proration, credits,
  refunds, currency. Those are a product built *on* usage records, by
  whoever is doing the billing, and they carry regulatory and financial
  obligations no backend platform should absorb on its users' behalf. This
  plan builds the meter. Somebody else reads it.

**M3 · One increment.** *Sonnet. Depends on M1c.*

- `crates/ordo-exec/src/usage.rs`: `pub fn render(space, rows: (Vec<SpaceRow>,
  Vec<ReplicaRow>)) -> String` (JSONL, sorted keys via `BTreeMap`); the CLI
  and the route both call it.
- `crates/ordo-cli/src/operator.rs` `"usage"`; `main.rs` dispatch and
  `usage()` text ending *"Ordo meters. It does not invoice."*
- `crates/ordo-host/src/serve.rs`: the route and the op constant; the
  reserved-name vet already covers `platform.*`.
- Tests: `crates/ordo-cli/tests/operator.rs`
  `two_usage_runs_over_a_closed_period_are_byte_identical`,
  `usage_output_reconstructs_the_tables_exactly`;
  `crates/ordo-host/tests/usage.rs`
  `the_usage_route_answers_the_operator_and_refuses_a_tenant_owner` (the
  platform-op rule: checked against the deployment's own space,
  `serve.rs:592-616`).
- **Done when** those pass and `docs/howto-operating.md` gains a "Usage"
  section that says invoicing is not here in the CLI's words.

### 3.3 The self-host posture

- **Off by default.** The established pattern is already stated twice in the
  code: *"a deployment that wants no metering simply does not wrap"*
  (`crates/ordo-host/src/metrics.rs:781`, `:859`) and *"the observation
  compiles down to an `Option` that is always `None`"*
  (`crates/ordo-exec/src/metrics.rs:16-17`). The ledger is an `Option` on
  the registry; off allocates nothing and both workers report themselves
  disabled with the variable named.
- **On by config, and by a builder**, `with_usage_ledger(true)`, with an
  env default `ORDO_USAGE_LEDGER=1`, following `with_per_space_series`
  (`metrics.rs:336`) and its stated reason: a builder is deployment
  control, while a label a missing variable adds is a surprise.
- **It never phones home.** Usage leaves the deployment only when the
  operator pulls it. There is no outbound POST, and that is not a
  preference: any egress would fall under `security-logging-design.md` §7b,
  which pins outbound sinks six ways, and `ordo-egress` is the one client
  that honours them. A telemetry POST that skipped those is the
  SSRF-into-key-custody path §7b was written to close. The record ships;
  the delivery does not exist.
- **Quotas are unset by default.** A self-hoster who never configures one is
  never refused, and `quota_exceeded` never fires. The class exists because
  the hosted deployment produces it, and a class with a producer is legal
  even where no deployment configures a limit.

### Ruled out (M)

- **A third hash-chained stream.** §3.2: the append lock, and the fact that
  a billing correction would be a chain rewrite.
- **Per-space Prometheus series for op latency.** Cardinality: spaces × ops
  against a 10,000 series cap. The ledger accumulates per space and exports
  aggregates.
- **Counting writes at the append choke point.** §3.2a: it counts before
  commit and forgets on crash; the chain does neither.
- **Per-request storage measurement.** `pg_total_relation_size` is
  per-relation, not per-space, and a per-space `pg_column_size` sum is a
  full scan. Sampled on the rollup cadence, with the method disclosed.
- **`quota_exceeded` ahead of its producer.** The inventory rule, and the
  `delegation_budget` precedent.
- **Metering the security-event stream or the audit spine.** Counting
  denials as billable usage prices the act of being attacked.
- **Invoicing.** M3.

---

## 4 · Sequencing

The tracks are independent except at three joints, and all run one way:

- **K2 before R1's cadence rule is meaningful.** Epoch destruction is one of
  the two events that must be preceded by a fresh backup; until destruction
  is reachable, the rule has nothing to fire on.
- **R2d's boot assertion before K1 runs on a real deployment.** A re-seal
  pass rewrites a large fraction of a space's payload rows. Doing that on a
  database whose chain silently moved backwards is the worst available
  ordering.
- **M1c before M2's `writes` quota, if one is ever wanted.** The leader
  rollup is the only source of that number.

Within the tracks: R2a → R2b → R2b′ → R2c → R2d-i → R2d-ii → R2d-iii →
R2d-iv, with R2e off R2c and R3 after R2d-iv; R1a is needed by R3, and
R1b, R1c are free-standing. M1a → M1b → M1c → M3, with M2 off M1b. R1 and
M1a can start today.

Model tiers, for the dispatcher: **Opus** for R2a, R2b, R2b′, R2d-i,
R2d-ii, R2d-iv and R2e (the chain encoding, what the verifier accepts, and
the tables it reads). **Sonnet** for R2c, R2d-iii, R3 and everything
else, with R2d-iii's caching rule read once by whoever built R2d-ii.

## 5 · What this plan does not close

- **SMK rotation**, and therefore blind-index re-salting. A real re-index
  with a real write barrier; `implementation.md` states the design and this
  plan does not build it.
- **`ordo.chainproof/2` carrying the marker.** A witness holding a
  pre-restore signed head already sees a restore through `chainProof`,
  because the marker changes every link after it. Carrying the marker's
  fields inside the proof would hand the explanation to the third party in
  the same artifact as the anomaly, and it is a schema bump
  (`attest_ops.rs:83`, `ordo-verify-proof`). Recorded here so R2 stays
  bounded; it is one increment when someone asks for it.
- **Archive delivery and layer-2 attestation.** Still blocked on §7b's six
  pins, as 5f says. R1's bundle cadence is a local cut, not a shipped one.
- **RPO/RTO numbers.** R3 makes the drill runnable. A number still requires
  a deployment under a load shape somebody committed to, and a laptop figure
  must not be recorded as a gate.
- **PITR itself.** Postgres's instrument, configured by the operator; the
  howto states the procedure and the platform proves the half it owns.
- **`$jobs` per-space attribution.** M1 counts worker runs at the runner,
  which has the space; the generic job queue is not per space, and giving
  it a space column is `plan-runtime.md`'s call, not this one.
- **A quota on `writes`.** Deferred with its reason in M2.
