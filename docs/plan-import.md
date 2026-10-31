# Ordo bulk import: loading an application's rows into a space

Companion to `plan-export.md` (the bundle this reads), `migration.md` §3
(*Bulk import mechanics*, `[proposed]`), `corpus-rulings.md`
(2026-09-08, the backfill audit encoding) and `roadmap-checklist.md`
**11-10a**.

Markers: **[settled]** restated from a settled document · **[v1]** built
in this plan's first slice · **[deferred]** named, scheduled, not built.

Status: **v1 built**, in the shape below.
`crates/ordo-exec/src/import.rs`, `crates/ordo-exec/tests/import.rs`, and
the batch on `ordo_conformance::WriteLog`. Every claim about the code was
checked against the code, and §9 records the two defects the build found.

**The blob leg is built (2026-09-26)**, which is §10. A space holding an
attachment now round-trips: export it, import it into an empty space, read
the file back, and the bytes, the mime and the plaintext digest are the
same ones under a different key. `crates/ordo-host/tests/import_blobs.rs`
is that round trip, and it lives in `ordo-host` because there is one upload
path and the door is there.

---

## 0 · Why this exists

`export_space` is real and one-directional. `plan-export.md` §3.1 says so
in as many words:

> Export is a disclosure format, not a backup format. […] re-sealing under
> a destination key is `ordo import`'s job, which is unbuilt.

Rows can only enter a space through the write protocol, one op call at a
time, and nothing in the tree drives that in bulk. There is no `COPY`
path, no loader, no `ordo import`. A review found this the single biggest
obstacle to migrating a real application onto the platform, and it is the
missing second leg of the trust argument `licensing.md` makes: a customer
can get their data out and cannot get it in.

## 1 · The constraint that shapes everything

**A write is not an `INSERT`.** `Fixture::run_write_body`
(`crates/ordo-conformance/src/write.rs`) proves preconditions, takes row
locks, evaluates guards against the would-be row, seals sealed fields
under the space's epoch key, derives `HMAC(k_field, plaintext)` into each
blind-index column, mints the row id and `rev`, images the result, and
hands back a `CommandGroup` that `Executor::attempt`
(`crates/ordo-exec/src/lib.rs`) appends to the space's hash chain inside
the same transaction that wrote the rows.

Bypassing that produces rows the platform cannot read (no blind index, so
every indexed lookup misses), cannot open (no envelope), cannot verify
(no chain entry) and cannot revert (no images). So **an import goes
through the protocol.** Everything below is about making that bearable
for a million rows rather than about avoiding it.

---

## 2 · Input shape: the export bundle **[v1]**

**v1 reads an `ordo.space-export.v1` bundle.** CSV/JSONL from a foreign
system is **[deferred]**.

Three reasons, in order of weight.

**The bundle is self-describing and the alternative is not.** The
manifest already carries everything a loader has to be told: the
**load order** (`load.order`, Kahn over `Schema::ref_edges`, with
`load.cyclic` naming what has no sequential order), the **per-entity
column list with its encoding** (`text`/`integer`/`bool`/`decimal`/
`sealed`/`blob`), the **row counts** to check against, the per-member
`sha256` digests, and the artifact's identity. `plan-export.md` §2.2
records that the load order was added *because the round-trip test
followed the manifest and nothing else*, which is the same reader this
plan writes. A CSV has none of that and needs a mapping file to supply
it.

**Export and import become inverses, which is a checkable property.** A
round trip is a test that can fail. "Import reads some CSV" is not.

**The foreign-system path is not blocked by this; it is blocked by the
mapping file.** `migration.md` §1 and §2 put CSV/JSONL behind
`ordo introspect` producing an **import mapping** (source table to entity,
column to field, type coercions, per-item disposition). Neither
`introspect` nor the mapping format exists. Building a CSV loader first
would mean inventing the mapping format in the loader, which is the wrong
place to decide it. When the mapping lands, it becomes a second
`RowSource` in front of the same batch driver: the driver takes entity
name plus a field map plus a source id, and does not care which producer
built it.

### 2.1 What v1 does with each bundle member

| member | v1 |
|---|---|
| `manifest.json` | read: format tag, artifact identity, `sealed`, load order, per-entity columns and counts |
| `artifact.json`, `schema.sql` | **checked**, not applied. The destination's deployed artifact must already match (§6, `artifact_mismatch`). Applying DDL is `ordo migrate`'s job and another agent's file. |
| `data/<entity>.jsonl` | the rows, read in file order |
| `commandlog.jsonl` | **not replayed**, and §3 says why |
| `blobs/…` | the bytes, bound to their rows through the deployment's blob door after every row exists (§10) |

---

## 3 · Chain shape: the batch group **[v1]**

### 3.1 What is already settled, checked before inventing anything

A batch group needs **no new ruling and no encoding change**.
`CommandGroup.commands` is a `Vec<Command>`
(`crates/ordo-commandlog/src/command.rs`), and `CommandGroup::validate`
asks only two things of that vector: it is non-empty (`EmptyGroup`) and
every command names one space (`MixedSpaces`). A group holding a thousand
commands is the shape the type was written for. `migration.md` §3 states
the same conclusion from the other side: *"Batch = one command group […]
Atomicity, ordering-per-space, and the single-space rule are all
inherited, not re-invented."*

So one group per row is honest and so is one group per batch. v1 writes
**one group per batch**, and the chain gets one entry per batch rather
than one per row.

### 3.2 What v1 does **not** do, and why that is the right cut

`migration.md` §3 asks for more: *"Images are summarized, not per-row"*,
a group carrying a count, an id set and a content hash instead of a
thousand after-images. The 2026-09-08 ruling (*the backfill audit
encoding*) settles the same question for migration backfills and makes
summarised groups the **default**, with an escape: *"an app creator may
switch a backfill to option 1 (full per-row images) when their domain
needs per-row revertibility and they accept the log cost."*

**v1 ships option 1.** Not as a hedge: a summarised group is a **growth
of the frozen chain encoding**, and the precedent for growing it is the
restore marker and the owner-administration trailer, both of which came
with a ruling, a by-presence trailer and a pre-change golden
(`corpus-rulings.md`, 2026-09-08 §2). Writing a summarised encoding is
therefore a separate, ruling-bearing piece of work, and doing it inside a
first import slice would decide the encoding by accident. Option 1 needs
none of it, is what the code already validates, and buys back per-row
`revert` in exchange for a log the size of the data.

The cost is stated rather than hidden: **a million imported rows produce
a million `Create` commands**, and at batch size 1000 they produce **one
thousand chain entries**. The chain is cheap; the log is not. Summarised
groups are **[deferred]** to 11-10a and are the first thing to build
after v1.

### 3.2a The 2026-09-20 ruling, and why it makes summarising import easier

A second ruling landed the day this was built (*What a backfill's revert
means: the diff, not a clone*). It confirms 2026-09-08 and sharpens it:
*"copying every row to make a backfill revertible is refused as the
default"*, and for a summarised backfill *"the prior values are not
recoverable from the command log"*, so the recovery path is the
deployment's restore.

Its scope is a **backfill**, which transforms rows that already exist.
Import creates rows that did not. That difference is what makes the cost it
names small here, and it is worth writing down before somebody builds the
summarised encoding: **a create has no prior value to lose.**
`migration.md` §3 already reaches the same place from the other side,
*"creates have no before, and the after is the database […] the group
inverse is delete-by-minted-ids, so `revert_group` works from the id set
without per-row images."*

So a summarised import group gives up strictly less than a summarised
backfill does, and the ruling's direction of travel is the one v1's
deferral should be read against: **summarised is where import should end
up**, and option 1 here is the honest first slice rather than a preference.

### 3.3 Why the exported command log is not replayed

A bundle's `commandlog.jsonl` is the *source* space's chain: its
correlations, its actors, its `SealedRef` handles, its hashes. Replaying
it into a destination space would append groups asserting writes that did
not happen here, under handles that resolve to nothing, and would break
the destination's own chain links. The bundle's chain is evidence about
where the data came from, verifiable offline by whoever holds it. It is
not a script.

What v1 writes instead is the truth: the rows arrived in this space, on
this date, in these batches, attributed to the principal who ran the
import. `migration.md` §3 already names the boundary and its meaning:
*"history before this point is the source system's problem; history after
is fully attributed Ordo history."*

---

## 4 · Ids: minted, and mapped **[v1]**

**Server-minted, always** `[settled — no exception for imports]`
(`migration.md` §3, and the table in §2 sends old serial keys to the same
place). v1 does not negotiate this: rows enter through
`std.<entity>.create`, which mints the id like every other write.

So the source key has to go somewhere, and references between entities
have to be rewritten. v1 adds one platform table:

```
"$importmap" (
  "$space"    text not null,
  "import"    text not null,   -- the import's declared name
  "entity"    text not null,
  "source_id" text not null,   -- the id the bundle carried
  "row_id"    text not null,   -- what this space minted
  primary key ("$space","import","entity","source_id")
)
```
with a unique index on `("$space","import","entity","row_id")`, because a
minted id belongs to exactly one source row and a mapping that let two
source rows claim one minted row would make the rewrite ambiguous in the
direction nobody checks.

**References.** Every ref column in the JSONL carries the *source*
target's id. Before the call is built, each ref value is looked up in
`$importmap` under the same import name and the target entity, and
replaced by the minted id. A miss is a refusal (`unresolved_ref`), never
a guess and never a null.

**This is what makes the manifest's load order load-bearing**, not
advisory: a referencing entity can only be rewritten after the referenced
entity has been imported and mapped. v1 follows `load.order` exactly and
refuses a non-empty `load.cyclic` (§6).

**A `tree` self-reference orders rows inside its own file**, which
`plan-export.md` §2.2 explicitly hands to import: *"Row-level ordering
within a self-referencing table is import's problem, not the manifest's."*
v1 does a Kahn pass over the entity's own rows (roots first, then
children) before batching, and refuses a cycle (`tree_cycle`).

**The mapping is kept, not dropped.** `migration.md` §3: *"queryable
during the transition (redirects, foreign systems still holding old keys,
the dual-write bridge), deletable after."* v1 writes it and leaves it.
Deleting it as an attributed command is **[deferred]**.

---

## 5 · Idempotency and resume **[v1]**

A large import will be interrupted. Two mechanisms make a re-run safe,
and both are already built.

**The `$replay` journal, per batch.** `plan-idempotency.md` records what
the write path gained: `TxConn::claim_lease` claims an idempotency key
**inside the transaction that will write the effect**, with a fencing
generation, and `complete_lease` marks it done in that same transaction,
so *"commit publishes both or neither."* A claim that finds
`LeaseState::Applied` rolls back untouched and the caller is told the
effect already committed.

v1 gives every batch a **deterministic** key:

```
import/<import-name>/<entity>/<batch-index>
```

with the fingerprint `sha256` of the batch's source ids in order. Batch
`k` is rows `[k·N, (k+1)·N)` of that entity's JSONL **in file order**,
and file order is deterministic because the bundle is
(`plan-export.md` §2.1: rows by `id` under the `C` collation, no clock
anywhere). So a re-run of the same bundle produces the same keys over the
same rows, every already-committed batch answers `Applied`, and the
import resumes at the first batch that did not commit. Nothing is
re-written and nothing is duplicated.

**A changed batch under a used key is a refusal, not an overwrite.** The
fingerprint is what makes that true: a corrected bundle re-run under the
same import name hits `idempotency_mismatch` on the first batch whose
rows changed. Re-running a corrected bundle is a new import name, or a
converge pass, which is **[deferred]** (§7).

**`$importmap` is the second half.** The mapping rows ride the batch's own
transaction, so a batch that committed left its mapping behind, and a
later entity's ref rewrite still resolves after a crash and restart. A
batch that did not commit left nothing.

**The batch is the unit of atomicity.** One transaction, one lease, one
command group, one chain append. A refusal anywhere in the batch rolls
the whole batch back: no partial group, no orphan mapping row, no
completion record for work that did not happen. This is
`Executor::attempt`'s existing contract and v1 inherits it rather than
restating it.

---

## 6 · Refusals: what it must not guess **[v1]**

Every refusal names the remedy. An import that guesses produces a space
that looks migrated and is not, which is discovered at cutover.

| class | when |
|---|---|
| `format_unknown` | `manifest.format` is not `ordo.space-export.v1` |
| `bundle_incomplete` | a member the manifest names is missing, or its `sha256` does not match |
| `artifact_mismatch` | the destination's deployed artifact version or domain differs from the bundle's. Rows shaped by one schema loaded against another is the silent wrongness `plan-export.md` §1 is written against |
| `unknown_field` | the JSONL carries a column the destination schema does not declare |
| `missing_field` | the destination declares a required field the bundle does not carry |
| `sealed_ciphertext_unsupported` | `manifest.sealed == "ciphertext"`. The envelopes are sealed under the **source** space's epoch key; the write protocol seals under the destination's. Re-keying one to the other is a custody act nobody has ruled, and guessing it would either fail to open later or land plaintext the caller never authorised |
| `space_not_empty` | the destination holds rows for an entity being imported that **this import did not write**. Stated against `$importmap` under this import's name rather than against the table being empty, because a flat emptiness rule would refuse the interrupted run that resume exists for. Merging into somebody else's rows needs a conflict policy (§7) |
| `cyclic_entities` | `manifest.load.cyclic` is non-empty |
| `tree_cycle` | a tree entity's parent links do not form a forest |
| `unresolved_ref` | a ref's source id is not in `$importmap` for this import |
| `row_refused` | the write protocol refused the row: a guard, a check, a unique collision, a closed ladder. The message carries the protocol's own class, the entity and the **source** id, because the minted id does not exist |
| `blob_door_missing` | the bundle carries objects and no blob door was passed. The mirror of `ExportRequest::blobs: None`, which means *this deployment has no blob tier wired*: loading the rows and leaving the files would produce a space whose every attachment resolves to nothing |
| `blob_unplaced` | an object names an entity or a field this schema does not have as a blob field, or a source row this import never mapped. An object is attached to a row that exists, so a miss is a refusal rather than a file on some other row |
| `blob_tier_unsupported` | the field is `blob private`, `blob inline` or `blob sealed`. The three the door refuses for a client's own upload too, refused here **before** a byte is sealed and each with its own reason (§10) |
| `blob_refused` | the door refused the object. The message carries the door's own error class, as `row_refused` carries the write protocol's |
| `idempotency_mismatch` | §5 |

`bundle_incomplete` also covers the objects, and in one more direction than
it covers a data file: a `blobs/` member the manifest does not describe, a
described object the bundle does not hold, a `blobs.objects` count that
disagrees with `blobs.files`, an object with no recorded `sha256`, and an
object the manifest names no mime for. The digest is **required** rather
than checked-if-present, because an object has no row count and no second
description: any byte string is a valid file, so the manifest's `sha256` is
the only thing that says these are its bytes.

`blob_unsupported` is **gone**, and it was three refusals: one on a
blob-encoded column in the manifest, one on `blobs.carried` with a non-zero
count, and one on any bundle member under `blobs/`. Each is replaced by
behaviour with a test rather than by silence, and §10.4 says which by name.

Two of these deserve their reasoning stated rather than listed.

**A row failing a check is a refusal of the batch, not a skipped row.**
`migration.md` §4 wants rejections collected into a verify manifest
instead of aborting, and that is the right end state. It needs the
manifest, the disposition vocabulary and a way to say "accepted with 12
rejections", none of which exists. Until it does, a loader that silently
dropped rows would be the failure mode this whole document exists to
avoid. v1 stops, names the row, and the operator fixes the bundle or the
schema. Continue-on-reject is **[deferred]**.

**A sealed field with no key released is not a null.** This is the same
ruling `plan-export.md` §3 made in the other direction: *"an unopenable
value in `Open` mode is a refusal, not a null."* On the import side the
key release is the ordinary write-purpose release the protocol already
takes, so the failure surfaces as the protocol's own refusal and the
batch aborts.

---

## 7 · v1 and what is deferred

**v1 builds:**

1. `crates/ordo-exec/src/import.rs`: `BundleSource` (the read-side mirror
   of `BundleSink`), manifest parsing, the checks in §6, `$importmap`,
   the tree pass, the ref rewrite, the batch driver, `ImportReport`.
2. A **batch mode** on `WriteLog` (`crates/ordo-conformance/src/write.rs`,
   about forty lines): while a batch is open, `run_write_body` takes the
   group correlation from the batch instead of minting one, and **extends**
   the pending group instead of replacing it. The shared correlation is
   not cosmetic: `$sealedpayload` rows are stamped with the group's
   correlation and collected by the anti-join *"payload rows whose
   correlation is no longer in the chain"* (`ordo-crypto/src/sealed.rs`),
   so a group whose correlation did not match the payloads written under
   it would have every sealed value in the batch garbage-collected.
3. Tests: a round trip, an interrupted import that resumes, a refusal, and
   an `#[ignore]`d throughput measurement (§8).

**The CLI verb is deferred, and for the reason the CLI already gives.**
`ordo revert` passes `keys: None` and refuses sealed entities, with the
cause written on the line: *"what is missing today is the operator tool's
custody selection — which master key, from where, with which refusal when
it is absent — and wiring it before that surface exists would mean this
command inventing its own answer to a question the deployment is about to
answer once"* (`crates/ordo-cli/src/operator.rs`). An import that cannot
seal is an import that cannot write a sealed field, so `ordo import` waits
on the same `ORDO_KEY_PROVIDER` surface. `import_space` is a library entry
point and the host, which does have custody, can call it today.

**Deferred, filed under 11-10a:**

- **Summarised batch groups** (§3.2). The frozen-encoding change, its
  ruling, its trailer and its golden.
- **`source:import-<name>` attribution.** `migration.md` §3 wants the
  actor to be a source principal. `run_write_body` hardcodes
  `Actor::User(caller)`, and a source principal that is not a space member
  cannot pass a guard, so honest attribution needs the write-only source
  principal itself, which is its own piece of work. v1 attributes to the
  principal who ran the import, which is true.
- **The genesis checkpoint** that closes an import and states the history
  boundary (`migration.md` §3).
- **The verify pass and the didn't-migrate manifest** (`migration.md` §4),
  and with it continue-on-reject.
- **CSV/JSONL plus the mapping file** (§2), behind `ordo introspect`.
- ~~**Blob objects.**~~ **Built 2026-09-26**, in §10. What is still deferred
  under it is named there: `blob private`, `blob inline` and `blob sealed`
  objects (each refused by name), a **changed** object under a used import
  name, and objects in a ciphertext bundle.
- **An entity that references a platform entity.** Not a blob gap, found by
  the blob build, and it blocks more than blobs do: `members` is a platform
  entity, import skips platform entities on purpose (a bundle that could
  create access rows would be a bundle that grants itself membership), and
  nothing maps their ids, so `owner -> members`, which almost every real
  application declares, refuses with `unresolved_ref` on every row. Filed
  on the decisions board (`platform-ref-import`) because the answer is a
  ruling about identity across spaces rather than a mechanism.
- **Ciphertext-mode import** into a space holding the same keys.
- **Import into a non-empty space**, which needs a conflict policy.
- **Quiet triggers.** `migration.md` §3 wants listener triggers not to
  fire per batch. v1 lets the ordinary invalidation enqueue happen, which
  is correct but noisy for a very large import.
- **Deleting `$importmap`** as an attributed command after cutover.

---

## 8 · Throughput, measured

`crates/ordo-exec/tests/import.rs::how_fast_one_import_is`, `#[ignore]`d
because it is a measurement and a threshold would fail on a loaded laptop
while proving nothing. 2000 single-field rows, debug build, local Postgres
over a unix socket:

| batch | time | rows/s | chain entries |
|---|---|---|---|
| 1 | 10.06s | 199 | 2000 |
| 100 | 0.62s | 3220 | 20 |
| 1000 | 0.54s | 3705 | 2 |

**The batch group is worth 18x**, and the shape of the curve says where the
cost is: batch 1 pays one transaction, one advisory lock acquisition and
one chain append per row, and batch 100 has already bought almost all of
what batching can buy. At batch 1000 a million rows is about four and a
half minutes, in a debug build.

What is left is per-row and is the op-call compile (parse, bind, plan),
which runs once per row because the call text differs per row only in its
literals. Caching the bound plan per entity is the obvious next move and is
**[deferred]**: there is now a number to improve on, which there was not
when v1 was planned.

---

## 9 · Two defects the build found, both unexecuted before it, both now fixed

Recorded here because each one is a live gap outside import, and neither
was import's to keep quiet about. **Both fixed 2026-09-20** — see
`corpus-rulings.md`'s "Two rulings from the bulk import build's own
defects" for the full reasoning; this section keeps the original account
of what the build found.

**`std.<entity>.create` refuses every row of a keyless entity.** `std_ops`
puts `rev: null` in the braces, which is the create-once belief. With one
unique key that is right. With **no** unique key the braces address no row,
and DL §6 rule 9 refuses a belief about a row the statement does not
address, so the derived op refuses with `invalid_binding_expr`. Nothing had
executed a derived `create` before: the tests that name it are
interface-shape tests. v1 derived `import.<entity>.create` in its own file,
through the same registration gate, with the belief dropped, and said in
the module doc that it becomes a call to `std.<entity>.create` the day the
derivation is fixed.

**Fixed**: `std_ops`'s create arm now carries `rev: null` only when
`unique_keys == 1`. `import.<entity>.create` is **not** retired by this —
see the ruling for why the keyed case still needs its own derivation — but
a keyless entity's derived `create` and `import`'s own are now identical.

**A negative `int`, `money` or `decimal` could not be bound as a call
argument.** The grammar reads a leading `-` as `unary := '-' unary` and
builds `Expr::Neg`; `lit_ty` answers only for the literal productions, so
`$total: -2500` refused as *"bound to something that is not a value"*. This
reaches further than import: `ordo_host`'s own `call_source` renders a
negative money exactly that way, so no client could send one either.
`fold_negation` folded the two numeric cases at the one place a call's
named arguments become the parameter environment, which is a strict
widening because `Neg` never bound at all. The **read** path had the same
gap and was not fixed there.

**Fixed**: the fold now lives in `ordo_sema` as `fold_neg_lit`, called
wherever a literal binds a parameter — the read path's op-call arguments
included — so both paths meet at one function instead of two. `write.rs`'s
`fold_negation` is gone; it calls the shared one.

---

## 10 · The blob leg **[built 2026-09-26]**

Export has carried blob bytes since 17g, at `blobs/<entity>/<field>/<id>`,
which is `migration.md` §6's layout. Import refused them in three places. So
a space with attachments could leave and could not come back, which is
exactly half of the argument §0 makes.

### 10.1 The constraint that shapes it: a file is not a column

`Sema::check_content_params` refuses a blob-typed op parameter, because the
only producer of a blob's value is an upload that happens after the row
exists. `import.<entity>.create` therefore cannot carry a file, and
`literal()`'s `ScalarTy::Blob` arm is a refusal rather than a spelling.

The bundle's blob **column** is no help either: it holds the *source*
space's handle, which names an object in the source's bucket sealed under
the source's key. Nothing in the destination could make that value mean
anything. So the column's value is dropped, and the bytes arrive the other
way: each object rides the bundle as its own member and is bound to its row
after the row exists.

### 10.2 One upload path, so the seam points up

`plan-blob-chain.md` C1–C8 ruled that an upload is a converge: bytes become
a `Stored` in `ordo-blob`, a `Stored` becomes a `PinnedUpload` at **one mint
site** in `ordo_host::blob`, and the write protocol converges on that token.
`crates/ordo-host/tests/upload_seam.rs` asserts all of it over the
workspace's own source: one mint site, one implementor of `PlatformValue`,
one caller of `attach_write`, and no path from a parsed op to the platform
converge.

`ordo_exec::import` is three crates below the door, so it cannot reach the
mint site, and building a second one is the thing that was ruled out. The
seam therefore points up: import declares `import::BlobDoor` and
`ordo_host::blob::PlatformBlobs` implements it by calling its own
`BlobPort::write`. Import decides **which bytes belong on which row**; the
door decides **what an upload is**, and every property of one is inherited
rather than restated: the tier from the schema, the intent row before the
bytes, the seal under the destination's key, the row lock, the write guard
before and after the mutation, the digest in the image, and one command
group in the destination's chain.

`ImportRequest::blobs: Option<&dyn BlobDoor>` mirrors
`ExportRequest::blobs: Option<&BlobPipeline>`: `None` means *this deployment
has no blob tier wired*, and a bundle carrying objects is then refused by
name.

### 10.3 Where the attach sits, and why the batch is untouched

**After every row of every entity, one pass, in member order.** An object is
bound to a row that exists, so it cannot precede its own row, and no object
references another object, so nothing else constrains the order.

**It is not inside a batch, and it cannot be.** A batch's closure is
re-executed on a serialization conflict, so it is `Fn + Clone`; an upload
token is minted from bytes that were actually stored and is consumed by
value, so it is neither. The bytes also have to reach the store *before* the
transaction that names them opens, which is the door's own ordering and the
reason an interrupted upload costs money rather than a guarantee.

**The batch's shared correlation is not weakened by that**, and it is worth
being exact because §3 calls it load-bearing. What makes it load-bearing is
that `$sealedpayload` rows are collected by an anti-join over correlations
no longer in the chain, so a payload written under a correlation no group
carries is collectable. Each attach is its own group with its own
correlation, appended to this space's chain by the door, so any payload it
writes is named by a group that *is* in the chain. The property is "every
payload's correlation is in the chain", not "one correlation per import".
And the tier whose bytes are a `$sealedpayload` row is `blob inline`, which
is refused by name before any of this.

`ImportReport` gains `objects_attached` and `objects_skipped`, counted apart
from `groups` so the one-group-per-batch property the report exists to show
is not folded into the attaches' own groups.

### 10.4 Resume: the marker is the effect, not a record of it

`$importmap` stays the mechanism in the sense that matters: it is what turns
an object's **source** row id into the row this import minted, and without it
an object has nowhere to go (`blob_unplaced`). It is **not** the marker for
whether the attach happened, and the reason is atomicity.

A mapping row rides the batch's own transaction, so a batch that committed
left its mapping behind and a batch that did not left nothing. There is no
window. An attach commits in the **door's** transaction, which import does
not open and cannot join, because the bytes must reach the store before it
begins. A mapping row for an object could therefore only be written after
that commit, and the gap between the two is a crash that leaves the object
attached and the record saying otherwise. The next run would upload a second
object over the first: a second `rev`, a second group, and an unreferenced
object for the sweep.

The **column** has no such gap, because it is what the attach produced, in
the transaction that produced it. `ordo_exec::blob::owning_row` already
rests on the same sentence for the other direction (*"the column IS the
reference, and that is what makes the GC anti-join sound"*), so reading it
is the existing authority answering the existing question. What makes the
reading safe is the enclosing rule: `space_not_empty` has already refused
unless every row of every imported entity is one this import minted, so a
non-null blob column on one of them was written by this import's own attach.

What the column does not distinguish is *which* object is attached. That is
named rather than hidden: the skip is counted in
`ImportReport::objects_skipped`, and re-importing a **changed** object under
a used import name is deferred (§7). It is the same question one space over
as "import into a non-empty space", which is also deferred: the row batches'
fingerprint catches a changed **row** under a used name, and no fingerprint
covers a changed file.

### 10.5 The digest, on the way in and on the way back

The bundle carries a `sha256` per member, so every object is read through
the same `member()` that checks it, **on every run including a resumed
one**: a resumed import verifies the bundle it is resuming over rather than
trusting the run that read it first. An object with no recorded digest is a
refusal rather than an unchecked load, for the reason §6 gives.

The round trip's other digest is the platform's own. `BlobPipeline::put`
records `SHA-256` over the plaintext, sealed and AAD-bound to `(entity,
field)`, as `$blobref.integrity`, and `plan-blob-chain.md` C5 puts it in the
converge's image as `"<field>$digest"`. So a destination that re-sealed the
same file agrees with the source about the digest while holding an envelope
that cannot be compared with the source's. That is what the round-trip
test asserts, opening the destination's envelope under the **destination's**
key and comparing against a digest taken from the bytes the test itself
wrote.

### 10.6 The mime, and the one thing the export had to add

An object's mime is a sealed envelope on the source's `$blobref` row. An
importer cannot open it and cannot derive it from the bytes, so before this
build it was nowhere in the bundle, and an import would have had to default
every file to `application/octet-stream`, a value the customer never
stored.

So the export's manifest grew `blobs.files`: one entry per object with its
member, entity, field, id, and its **mime when the member holds plaintext**.
The entity, field and id are there so that no reader recovers them by
splitting a member name, which would be parsing a path the exporter
formatted and would place an object on the wrong row the first time an id
held a `/`. The mime is absent in `ciphertext` mode and for a `blob private`
field, for two reasons that both matter: a bundle the platform did not
decrypt must not disclose through the manifest what it refused to disclose
through the member, and a mime describing bytes that are not the file's
bytes is a false description. Import refuses an object with no mime, so a
bundle exported before this change refuses by name rather than importing
with a wrong one.

### 10.7 The tiers that stay refused, by name

Each is refused **before a byte is sealed**, and each reason is the door's
own rather than a new one:

- **`blob private`**: client-sealed custody, where the platform holds no
  key. Such an object leaves the source space *as stored*, so what the
  bundle holds is the source client's ciphertext under a key nobody in the
  destination has, and there is nothing an import could do with it but copy
  it into a column and call it a file. The presigned direct-to-bucket path
  that makes the tier work is F7 Phase 6's (`f7-vault-plan.md`), and until it
  exists a `private` object cannot leave one space and arrive in another.
- **`blob inline`**: its bytes ride the row's sealed payload rather than the
  object store, so they are written by the write protocol inside the group
  that creates the row. There is no upload to make, and the create that
  could carry them cannot name a blob parameter.
- **`blob sealed`**: declarable and not servable in either direction. The
  collector proves a reference live by comparing the column against the
  handle and holds no keys, and a sealed handle carries a fresh nonce, so
  equal handles never produce equal envelopes.

### 10.8 What is tested, and where

| property | test |
|---|---|
| the round trip: bytes, mime, digest, the digest in the chain, and a re-run that attaches nothing | `ordo-host/tests/import_blobs.rs::a_space_with_an_attachment_round_trips_into_an_empty_one` |
| every refusal made before a row is written, and that nothing was written | `ordo-exec/tests/import.rs::an_object_the_import_cannot_place_is_refused_before_a_row_is_written` |
| the door is handed the **minted** row, and a second run skips | `…::an_object_lands_on_the_minted_row_and_is_not_attached_twice` |
| an object with no row, bytes that do not match the digest, and a door that refuses | `…::an_object_with_no_row_is_refused_and_so_is_one_the_door_declines` |
| the manifest names each object and its mime, and omits the mime in ciphertext mode | `ordo-exec/tests/export.rs::an_export_reads_the_spaces_own_store` |

### 10.9 What the blob build found outside itself

**An entity that references a platform entity cannot be imported at all.**
Not a blob gap, and it blocks far more than blobs do. `members` is a platform
entity; import skips platform entities deliberately, because a bundle that
could create access rows would be a bundle that grants itself membership; and
nothing maps their ids. So `owner -> members`, which almost every real
application declares, refuses with `unresolved_ref` on its first row. Filed
on the decisions board as `platform-ref-import` with a recommendation,
because the answer is a ruling about identity across spaces rather than a
mechanism. The round-trip fixture declares no such ref, and says in the
source why.

**A dead refusal.** The refusal on a blob-encoded column keyed on
`encoding == "blob"`, and the export writes that encoding as `blob_ref`, so
the arm had never fired for any bundle the export produced. The refusal
doing the work was the one over the member list. Recorded here because it is
the second refusal in this module found to be unreachable, and both were
found by building the thing they were standing in for.

---

## 11 · The door **[built 2026-09-26]**

Status: **built**, as `std.space.import`
(`crates/ordo-host/src/exit_ops.rs`), with `ordo import` in front of it
(`crates/ordo-cli/src/wire.rs`). Every claim below was checked against the
code.

### 11.1 · Why the engine sat unreachable for six days

`import_space` landed with 11-10a and, until this, **every caller of it in
the workspace was a test.** 17y finding 1 recorded the reason and it was
right: the CLI has no custody selection surface, and *an import that cannot
seal cannot write a sealed field*. A tool holding a database password can
insert rows. It cannot re-seal a sealed column under the destination space's
key, because it has no key and no legitimate way to get one.

So the fix is not a CLI feature. It is the 2026-09-20 ruling applied to the
first command that had no alternative: **the deployment holds custody, so the
deployment runs the import**, and the operator asks it to.

### 11.2 · The custody assignment, which is the whole of the handler's risk

`Fixture::compile_source` mints a `TestProvider` over the conformance space,
because a compiler is a pure function of source text and has no business
deciding whose keys a program seals under (R2a, ruled 2026-09-07). The
Stage-5 write protocol seals, digests and mints handles through
`Fixture::keys`. **Threading custody into the `Executor` alone covers every
read and none of the writes**, which is exactly the shape a reviewer would
approve: the reads work, the rows arrive, the chain is intact.

What that costs is visible only from outside: `ExitOps::deployed_fixture`
does `in_space(space)` and then `fixture.keys = self.keys.clone()`, in that
order, copying `deploy::verify_candidate` deliberately. Remove the second
line and `tests/import_op.rs` fails with

```
sealed_open_failed: `notes.memo` did not open
```

which is the honest shape of the defect: the import **succeeds**, the rows
are in the destination and in its chain, and the sealed values are
unopenable by the space that now owns them. The test uses two distinct
`TestProvider` instances, one per side, because a single shared provider
makes the assertion pass either way. That vacuous version was written first
and is recorded here rather than quietly replaced.

### 11.3 · Which bundle a caller may name, and why the path carries the space

A bundle is **named, never pathed**: `bundle: text` resolves to
`<ORDO_IMPORT_BUNDLE_DIR>/<bound space>/<bundle>`, with both components
restricted to ASCII alphanumerics, `_` and `-`. No `.` is admitted at all, so
`..` is unspellable rather than filtered.

**The space component is an authorization boundary rather than a layout.**
One staging root serves a deployment with many tenants, and every tenant's
owner passes every gate this op has. Without that component, an operator who
staged one tenant's bundle so that tenant could reload it would have made it
importable by *any* other tenant's owner, who would then hold the whole of
somebody else's space, opened, in a space whose keys they control. The caller
never supplies the component: the door does, from the space it bound.

`bundle_unknown` is **one refusal for an illegal name and for a name nothing
is standing under**, on `bucket_refused`'s argument about its allowlist: a
caller who can tell those apart can map the staging directory a guess at a
time, and what an operator has staged is the operator's business.

### 11.4 · The wire class

`import_refused`, 409, with the class as the first word of the message in
`ImportError`'s own spelling, so an import refused over the door reads
identically to one refused in-process. One code for fourteen classes on
`apply_refused`'s reasoning, since §7's deferred work extends the set and a
wire class per refusal would make each increment an amendment to a closed
inventory. Two refusals are deliberately other codes: a ciphertext bundle is
`sealed_unsupported`, and a bundle the destination's blob door refused keeps
the door's own class, as `row_refused` keeps the write protocol's.

409 rather than `tenant_refused`'s 400 because the bundle is named rather
than supplied inline, so every class, including a malformed bundle, names
state the caller repairs and retries the identical call over.

### 11.5 · Idempotency: the parameter, not the header

`requires_idempotency_key: false`, unlike `std.space.create`. The import's
`name` **is** the idempotency key and a stronger one: the batch leases in
`$replay` are keyed by it and fingerprinted by their source ids, so two runs
under one name resume each other.

**What that fingerprint does not cover, and this section used to say it
did.** It read *"a changed bundle under a used name refuses instead of
skipping."* That is true only when the **ids** change. `batch_fingerprint`
hashes `source_id` and nothing else, so a bundle with the same ids and
different values resumes cleanly: batches already committed keep the old
values, batches not yet committed take the new ones, and the import reports
success over the mixture (`security-review-6.md` S6-006). The manifest's
per-member digests do not help, because a changed bundle with a consistent
manifest passes them. A header would add a second, weaker answer to a
question the parameter already settles.

### 11.6 · What the door does not close

- **Delivering a bundle to the deployment.** An operator stages it on the
  deployment's disk. That is the same asymmetry `plan-export.md` records for
  export, one tier over: a self-hoster has the filesystem and a hosted
  tenant does not. An upload surface is unbuilt and is the obvious next
  increment.
- **The platform-ref gap** (§10, `platform-ref-import` on the board). The
  door cannot help: no caller can import an entity that references
  `members`, and that is most real applications.
- **`ordo import` end to end through a real deployment.** The command's
  refusals are exercised; the success path is proved at the handler, which
  is where the custody question lives. The dev loop registers no import op
  by design, so the end-to-end walk needs a host with a staging directory.

---

## 12 · The people map **[built 2026-09-26, owner-ruled]**

`platform-ref-import`, ruled the day it was filed: **a supplied map, and a
refusal for any name it does not cover.** §10's finding was that an entity
declaring `owner -> members` could not be imported at all, which is most real
applications.

### 12.1 · Why it is pre-seeded rather than resolved

The map is applied by putting `("members", source_id) -> destination_id` rows
into `$importmap` **before the load order runs**, so the ordinary ref rewrite
resolves a person exactly as it resolves anything else. Nothing in
`render_call` learned about people.

That is deliberate, and the reason is that the ruling's requirement and the
existing behaviour are the same sentence. `unresolved_ref` already refuses a
ref it cannot resolve, and the ruling asks for *a refusal for any name the map
does not cover*. Teaching the rewrite about people would have meant a second
resolution path, and a second path is a second place for the refusal to go
missing.

### 12.2 · What was refused, and why each alternative is worse

- **Pointing every row at one member** (candidate (b)) rewrites authorship
  silently. Every document in the imported space would look like it was
  created by the same person, and no reader afterwards could tell that from
  the truth.
- **Creating the members the bundle names** (candidate (c)) was refused
  outright, and the map enforces that refusal rather than merely declining to
  implement it: `people_unknown` fires when the map names anybody who is not
  already a member of the destination. A file that could admit a principal
  would be a file that grants access to the space it is loaded into.
- **A merge**, two source people onto one destination member, is refused with
  `people_ambiguous`. `$importmap`'s minted-id index exists because one
  minted row answers to exactly one source row, and two source ids resolving
  to one row make every reference to either resolve to the same row with
  nothing to show it. An operator who means to merge two identities can say
  so in the source data, where a reader can see it.

### 12.3 · Where the map lives, and why it is beside the bundle

`<ORDO_IMPORT_BUNDLE_DIR>/<space>/<bundle>.people`, a JSON object of source
member id to destination member id.

**Beside the bundle, not inside it.** The manifest carries a digest per member
and the importer refuses a member the manifest does not describe, so a file
added inside would make the bundle incomplete. And the map is not part of the
export: it is the operator's statement about who the people in the destination
are, and the same bundle honestly takes a different map in a different
destination.

**Absent is no map**, which is the behaviour before this existed: the first
row naming a person is refused and nothing is written. A *malformed* map is
`people_unreadable` rather than absent, and the distinction is the point. A
typo in a staged file that read as "nobody is mapped" would refuse for a
reason that names the wrong thing, and an operator would go looking at their
bundle.

### 12.4 · It is stored, not just applied

Each pair becomes an `$importmap` row under the import's name, so **the
mapping an import used is queryable in the destination space afterwards**,
which is what makes attributed authorship answerable rather than only
assertable at the time. A re-run under the same name supplying a *different*
map is `people_changed`: the rows already loaded were attributed under the
stored mapping, so honouring a new one would split one import's authorship
between two answers. A new import name is the way to change the answer.

### 12.5 · Still open

- **Nothing maps `groupMembers`**, and nothing should. A membership is granted
  by the destination's owner, and the whole of this ruling is that a bundle
  cannot grant one. A bundle's group structure therefore does not survive the
  import, and reconstructing it is an act in the destination.
- **The map is per import, not per deployment.** An operator migrating twenty
  spaces writes twenty maps. A directory of people known to the deployment
  would be the obvious next increment and needs a ruling of its own, because
  it is a standing statement rather than one about a single load.
