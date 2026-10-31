# Ordo exit ops — full space export and space deletion

Companion to `migration.md` §6 (*Migration out*, `[settled]`),
`definition-language.md` §8 (*Exit ops*, `[settled]`),
`licensing.md` ("Why there is no gate" — export is leg 1 of the three-leg
trust argument), `howto-custody.md`, and `audit-board-coverage.md` row 10,
which recorded this as **(b) missing — no export path; `space.delete` has
no producer**.

Markers: **[settled]** restated from a settled document · **[built]**
landed, with its file · **[open]** named and not answered.

Status: **built**, in the shape §7 describes. Every claim about the code
was checked against the code and carries its file.

---

## 0 · Why this exists at all

`licensing.md` makes a product argument the platform then has to keep:

> Ordo asks for an unusually large commitment. […] Nobody sane makes that
> bet against a vendor who can revoke the terms. So the exit story has to
> be real, and it has three legs: **Export.** Full space export yields
> plain DDL, per-entity JSONL and a chain-verifiable command-log archive —
> a documented op, not a support ticket.

Until this work there was no export path and `space.delete` had no
producer. Leg 1 of a three-leg argument was a paragraph.

---

## 1 · The architectural requirement, stated first because it shaped
everything else

The owner's instruction, verbatim:

> Put extra emphasis on making sure it is dynamic and flexible and the code
> can change with the flexibility of the data it is exporting — otherwise
> we'll have to modify the export code for every change. It should be
> dynamic.

Read as a hard requirement: **the export derives everything from the schema
and the artifact. No per-entity code, no hand-maintained column list.**

This is not tidiness. An export with a hand-maintained column list is
*silently wrong* one deploy after somebody adds a field, and the bundle
**still parses** — so nobody finds out until the day they need it, which is
the day they are leaving and least able to complain. The failure is
invisible by construction, which is why the requirement has to be
structural rather than a review habit.

### 1.1 How it is enforced, in four places [built]

**(a) One column list, shared by the printer and the reader.**
`ordo_schema::ddl::physical_columns` (`crates/ordo-schema/src/ddl.rs`) is
the single answer to *which columns does this entity's table have*, and
`ddl_for` now iterates it. The export reads the same function. Two tests in
that file check the two halves cannot drift, **in both directions**:
names-in-DDL-not-in-list is the omission (the export drops a column);
names-in-list-not-in-DDL is the invention (the export selects a column the
database has no reason to hold, and the query fails against a database
built from the `schema.sql` shipped beside it).

Columns carry a **role** — `Space`, `Field`, `BlindIndex`, `Path` — rather
than being classified by name. A reader that told `memo` from `memo$bidx`
by suffix would be reimplementing `Field::index_column` at a distance and
would mis-classify a declared field somebody happened to name `memo$bidx`.

**(b) The bundle's fixed members are the artifact's own bytes.**
`schema.sql` is `Artifact::ddl` verbatim and `artifact.json` is
`Artifact::artifact` verbatim (`crates/ordo-exec/src/export.rs`). Not
re-rendered, not assembled — the bytes the deploy applied. There is
nothing in either to keep in step with anything.

**(c) A new *type* is a decision; a new *field* is not.** Value encoding is
an exhaustive `match` on `ScalarTy` with **no wildcard arm**. A new entity,
field, constraint or declaration flows through with no code change. A new
scalar type in the language **fails to compile** in `export.rs` until
somebody decides what its JSON looks like — which is the right place to
make that decision and the wrong place to default it.

**(d) The requirement is a test.**
`crates/ordo-exec/tests/export.rs::a_new_entity_and_a_new_field_reach_the_bundle_with_no_export_code`
compiles a program, exports, then compiles the same program plus a new
entity and a new field and exports again, asserting the new entity's JSONL
appears and the new field is in every row of the existing entity. It
cannot fail today — dynamism is inherent to how the module is written — and
that is exactly why it is written down: it is a **specification of the
requirement**, and the day somebody hand-rolls a column list to fix an edge
case, it is what stops them.

The assertion is deliberately two-sided. An export that regenerated its
whole shape on every deploy would also pass a one-sided check while being
useless to anyone diffing two bundles, so the unchanged entities' members
must be byte-identical across the deploy.

---

## 2 · The bundle [built]

`ordo_exec::export::export_space`, into a `BundleSink`
(`MemoryBundle` for tests, `DirectoryBundle` for the op).

| member | contents |
|---|---|
| `manifest.json` | the bundle's self-description — format tag, space, artifact identity, sealed mode, per-entity column list and row count, the omitted columns and why, the command-log summary, the **load order**, and an `unbuilt` list |
| `schema.sql` | the deployed artifact's stored DDL, verbatim |
| `artifact.json` | the deployed artifact document, verbatim — identity travels with the data |
| `data/<entity>.jsonl` | one JSON object per row, space-scoped, one file per entity in the schema |
| `commandlog.jsonl` | one group per line in chain order, each carrying `prev` and `hash` |
| `profile.json` | the tenant's access profile: op lines, per-member overrides, groups and group links. Member ids are the source space's |

### 2.1 Determinism [built]

**A bundle is a pure function of the space's state and the artifact.**

- rows ordered by `id` under the **`C` collation** — byte order, so the
  database's `lc_collate` cannot reorder a bundle between two deployments;
- JSON object keys sorted (`serde_json::Map` is a `BTreeMap`);
- entities visited in name order, not source order, so moving an `entity`
  block up a file does not reorder a bundle;
- **no timestamp anywhere.**

Two exports of one unchanged space are byte-identical, which is what makes
a bundle diffable, cacheable and checkable by whoever receives it.
Provenance — who asked, and when — belongs to the security-event record the
op writes (§5), which is the artifact that is *supposed* to carry a clock.

### 2.2 The load order [built]

Every ref is a foreign key on `($space, id)`, so an importer must insert a
referenced entity before a referencing one. Bundle order is **name** order,
because a bundle should be diffable — so the manifest states the load order
separately: Kahn's algorithm over `Schema::ref_edges`, deterministic by name
among the ready set, plus a `cyclic` list naming entities that reference
each other and therefore have no sequential order at all (load those in one
statement, or with constraints deferred).

This was found by the round-trip test, which follows the manifest and
nothing else — it does not know that `orders` references `vendors`, only
that the bundle said so. Without it the bundle was not loadable from
itself, which would have made "a documented path, not a negotiation" false
in the most embarrassing possible way.

A `tree` self-reference orders **rows** within its own table (parents
before children) and appears in neither list. Row-level ordering within a
self-referencing table is import's problem, not the manifest's.

### 2.3 The command-log archive [built]

`migration.md` §6 asks for *"the settled JSONL bundle format, chain hashes
included, so the history's integrity survives the platform."* No emitter
existed; this defines one. Both hashes travel with every group, so a reader
holding nothing but `commandlog.jsonl` can recompute `SHA-256(prev || body)`
and check the links without the platform — which is the whole point of
exporting a chain rather than a list of events. `idx` is the chain position,
so a reader can anchor a checkpoint against it.

Sealed fields appear in images as `{"sealedRef": "<hex>"}` — an object
rather than a bare string, so a handle stays distinguishable from a `text`
value that happens to be 32 hex characters. The handle is random per write
and leaks nothing; that is why the chain carries it and not a value.

A group that is DL §3 rule-6 **owner administration** carries
`"ownerAdministration": true`; every other group carries no such key. That
is not decoration and the asymmetry is not sloppiness — it mirrors the chain
encoding, where the flag is a trailer that is present or absent rather than a
byte that is `1` or `0` (`corpus-rulings.md`, 2026-09-07). The verification
promise above is what makes it mandatory: a flagged group whose line omitted
the flag would re-encode one byte short of the body that was hashed, and a
reader doing exactly what this section invites would read a lawful group as
tampering. The by-presence spelling has a second use — `grep
ownerAdministration commandlog.jsonl` is the list of every command in which
the space owner exercised the rule-6 capability, which is the question an
auditor arrives with.

---

## 3 · Sealed fields — the ruling [settled, then built]

`migration.md` §6 had already ruled, and the implementation follows it
rather than re-opening it:

> **Data** — per-entity JSONL (and SQL loadable against `schema.sql`),
> **decrypted under the owner's authority through the normal key-release
> path**; client-first spaces export from the device, where plaintext
> lives.

So there are two modes and §6 names both.

**`SealedMode::Open` (the default).** One `KeyProvider::release` for
`Purpose::Read` over the whole export, attributed to the op's correlation,
then `KeyProvider::open` per value. Plaintext lands in the JSONL. This is
*why* the op is step-up gated: export is total disclosure, so the
disclosure must be as authenticated as the door can make it.

**`SealedMode::Ciphertext`.** §6's next sentence. A deployment holding no
key for a space cannot open its envelopes, and pretending otherwise would
produce a bundle full of nulls that reads as *this field was empty*.
Ciphertext mode hands over the envelopes as lowercase hex — the frozen
`[epoch:1][nonce:24][ciphertext||tag]` layout, so a holder of the same keys
can re-import — and the manifest says `"sealed": "ciphertext"`.

**An unopenable value in `Open` mode is a refusal, not a null.** A space
with a destroyed epoch holds data the platform has and cannot hand over.
The bundle must not quietly under-deliver on leg 1 of the trust argument
while looking complete; re-run in ciphertext mode and the state is
disclosed for what it is.

**Blind-index columns are omitted**, and the manifest records each one with
`"reason": "blind_index_derived"`. A `$bidx` value is
`HMAC(k_field, plaintext)` under the *source* space's key. It is not data:
it carries nothing the plaintext beside it does not, it cannot be
reproduced under a destination space's key, and shipping it hands a reader
a working equality oracle over the values it sits next to. Re-deriving
digests belongs to import, under the destination's key.

### 3.1 The consequence for round-tripping, stated honestly

**Export is a disclosure format, not a backup format.** An opened sealed
value cannot go back into the `bytea` column `schema.sql` declares without
being re-sealed under the *destination* space's key, and re-sealing is
`ordo import`'s job (`migration.md` §3, genesis import mode), which is
unbuilt. The round-trip test therefore covers the open portion exactly —
fresh database, the bundle's own `schema.sql`, the bundle's own load order,
counts and spot values including a negative money, a null, a date, an
embedded quote and the tenancy column — and the sealed portion is covered
by the manifest's `sealed` and `omitted` declarations plus the two mode
tests.

---

## 4 · Deletion [built]

`ordo_exec::erase::erase_space`. Two acts, in this order:

1. **Destroy the space's key material** — `KeyProvider::destroy_space`:
   every DEK *and the SMK under them*.
2. **Delete the rows** — every space-scoped row in every table.

### 4.1 Why both, and why in that order

Neither act alone is deletion. Row deletion answers the live database and
nothing else: every backup, WAL segment, replica and archive still holds
the bytes, and a deployment that promised erasure and delivered a `DELETE`
promised something it cannot keep. Key destruction answers all of those at
once — for the sealed fields — and answers the live database not at all,
because plaintext columns are plaintext.

The order is keys-first because the failure modes are asymmetrical. Keys
destroyed with rows surviving is a space whose sealed data is *already*
unrecoverable and whose remaining rows a re-run removes. Rows deleted with
keys surviving is a space that **looks** deleted while every backup of it
stays openable. When a partial failure is possible it should land on the
stronger guarantee. `crates/ordo-exec/tests/erase.rs` asserts it: a
provider whose `destroy_space` fails leaves every row where it was.

### 4.2 Why the SMK goes with the DEKs [built]

`destroy_epoch` already existed and is deliberately narrow — it refuses the
epoch new writes seal under, and no sequence of rotations reaches "no live
epoch" through it, because rotating to escape an epoch mints another one.
So the trait grew `destroy_space`.

Dropping the DEKs alone leaves every field envelope unopenable, which is
what crypto-erasure usually means. It does not finish the job: **blind-index
keys are SMK-derived and epoch-independent by design** (that is what lets
rotation leave a digest where it is), so a surviving SMK leaves anyone
holding a backup able to recompute `HMAC(k_field, guess)` against the
`$bidx` columns in it and confirm guesses about a space that was supposed
to be gone. Erasing only the DEKs answers *what was the value* and leaves
*was the value X* answerable.

`destroy_space` and `WrappedKeyStore::drop_space` are **required** trait
methods rather than defaulted ones: a default would silently give every
wrapping provider (metering, a KMS front) the wrong behaviour, and a wrapper
that forgets to forward is a deletion that reports success and erases
nothing. `MeteredKeys` forwards explicitly, with that reason on the method.

### 4.3 What refuses, and what merely discloses [built]

The prune machinery already knows what a space's chain owes, and deletion
respects the same list read once more.

**Refuses: a non-evictable consumer behind the head.**
`ConsumerKind::evictable` is true for `Mirror` and nothing else, and the
three that are not are not, for reasons that apply here verbatim:

- `Archive` — it is the copy of record; history it has not received dies
  with the space.
- `ClientFirstMirror` — the device *is* the authoritative store; erasing
  the keys strands an enrolled device rather than freeing it.
- `Listener` — at-least-once delivery holds only while the floor never
  passes the slowest consumer, and deletion would turn outstanding
  deliveries into zero-times.

The refusal names the remedy prune already documents: let it catch up, or
de-register it, which is an attributable act somebody performs on purpose.
Wire class `space_pinned`, 409 — the identical request lands once the
consumer moves.

**Discloses, and does not refuse: the undo horizon.** Prune refuses to cut
inside the horizon because the platform guarantees the *owner* undo for
that window. Deletion is the owner spending that guarantee, which is theirs
to spend; refusing would make the platform the owner of their data's end.
So `EraseDisclosure` carries `inside_undo_horizon` and the op puts the
number in its answer — the same shape `roadmap-checklist.md` 13e gives
destructive migration, "explicit confirmation and the undo-horizon
disclosure".

**Does not refuse: a stalled evictable mirror.** It does not block a prune
either, and for the identical reason: it is a cache. It is reported as
`strandedMirrors` so an operator sees what a deletion will strand.

**Legal hold is not implemented anywhere** (`policy compliance` is `[lean]`
and `legalHold-capable` has no producer in `crates/`), so deletion cannot
respect one. Named here rather than left to be discovered. **[open]**

### 4.4 The confirmation [built]

`confirm` must be the **space's own id typed back**. Not a boolean, which a
retried request supplies for free; not a token the platform issues, which
would make the platform the party that decided. Wire class `not_confirmed`,
400 — the request is malformed as written and no state will make this exact
call succeed. Deliberately not `missing_param`/`invalid_param`: those are
the door's type check, and this is a refusal *after* the value bound
correctly, made because what the value says is not what the op requires.

§8 also names a **confirmation window** — a delay between request and
destruction, with a cancel. That is unbuilt: it needs a pending-deletion
state in `$migrations`-shaped storage, a job to fire it, and a cancel op.
**[open]**

### 4.5 The sweep [built]

Every table holding rows for the space: the schema's entities, plus **every
platform table the catalog says has a space column**, discovered from
`information_schema` rather than listed. A hand-written list of platform
tables is the same fragility as a hand-written column list one layer up —
the day somebody adds a space-scoped platform table, a deletion that names
tables by hand starts leaving a tenant's rows behind and reports success.
The test creates a table this codebase has never heard of and asserts it is
swept anyway.

The delete is **one statement**, a data-modifying CTE per table. Every ref
is an FK on `($space, id)`, so a sequence of `DELETE`s would have to run
children-before-parents — which needs a topological sort and has no correct
answer at all when two entities reference each other. Inside one statement
the referential-integrity triggers fire once, at the end, by which time
every row of the space is gone from every table and nothing dangles. The
ordering problem stops existing rather than being solved. The transaction
holds the space's chain-append advisory lock, so a concurrent write cannot
append to a chain that is being deleted.

### 4.6 The one thing not erased [built]

`ordo_exec::erase::RETAINED` = `$secevents`, `$seccheckpoints` — the
security-event stream. Two reasons, both on the constant:

- **It is a hash chain.** Deleting a prefix of one is the act `prune`
  refuses as *"deleting it would destroy the evidence"*. Here the evidence
  would include the deletion itself, which is the one entry nobody may
  lose.
- **It is the operator's record, not the tenant's.** It carries decisions —
  who was denied what, which step-up was demanded, when a key was released
  — and never domain rows, never images, never a field value.

The honest cost, stated rather than hidden: the retained stream keeps
**principal identifiers and op names** associated with a deleted space.
That is a real residue and a deliberate one — an audit spine a subject can
erase is not an audit spine. Retention of the stream is governed by
`policy retention`, which is where a jurisdiction's answer belongs. A
deployment with a GDPR obligation that reaches this residue has a retention
question, not an erasure bug. **[open — needs the legal review
`policy compliance` is already waiting on]**

---

## 5 · Authorization and the record [built]

Both ops are registered in `deployment_engine` with:

- **`Role::Owner` only.** `platform_ops::owner_only` owns that decision so
  it sits beside the ops rather than in the registration loop, where a role
  list gets loosened by whoever edits the engine next. The loop previously
  blanket-listed every platform op for `Manager` — correct while
  `callersOf` was the only one, and it would have made `std.space.delete`
  manager-reachable the moment it was added, in silence.
- **`require_step_up`.** Which is also what makes both ops structurally
  **undelegable**: §5 pins that a capability token can never satisfy
  step-up. An owner may delegate the owner's authority over data; ending
  the space is not that. Reusing step-up rather than minting a "human-only"
  flag matters — the platform has exactly one mechanism for *not
  delegable*, and a second would be a second thing to get right.

### 5.1 What "command-logged" turns out to mean

§8's word is *command-logged*, and for these two ops it cannot be taken at
face value. The command log records **writes to entity rows**: a closed
`Create`/`Converge`/`Delete` taxonomy, every command carrying an entity, a
row id and before/after images. Neither exit op is a write to a row.

- **Export mutates nothing.** Forging a `Converge` with equal images to
  record a read would put a lie in the chain — and the chain's whole value
  is that a reader may take what it says literally. A revert of that
  command would be a revert of nothing.
- **Deletion destroys the chain.** A final group appended to a log deleted
  in the same act records nothing, and a log that survives its space is a
  space that was not deleted.

So both record `EventKind::CustodyAction` in the **security-event stream**
— beside epoch rotation, epoch destruction, custody conversion and
break-glass, which are the acts these two are actually siblings of. The
spine has two halves; the exit ops belong to the half that records
*decisions about a space* rather than *changes to its rows*. The
requirement §8 is making — **the owner cannot do this without leaving a
record** — is met, by the half of the spine that can outlive the space.

The record is written at **space scope** for both, which for the deletion
only works because §4.6 retains that stream: the one record that must
outlive a space is the record that it was deleted.

Recorded on success only. A refused export disclosed nothing, and recording
it as a disclosure would put a false total-disclosure event in the owner's
own stream.

See `corpus-rulings.md` for this ruling and the sealed-field one in their
canonical form.

### 5.2 The verbs, and the knob

`OpKind` fixes the HTTP verb. Export is `Write` (POST) rather than `Read`
(GET): it produces a durable artifact on the deployment's disk and a
disclosure record, and a `GET` that did both would be a `GET` every cache
and prefetcher in the world is entitled to repeat. Delete is `Delete`.

The export's one knob is `ciphertext: bool`, **not** `mode: text` with two
legal spellings, and the difference is which layer refuses a typo. Text
would put an enum check inside the handler — and an executor may not say
`invalid_param`, which is the door's word, so `ApiError::from_exec` would
rewrite a typo into `internal`. A bool has no invalid value: the door
type-checks it in its own vocabulary before the op is reached. Absent (or
false) is *opened*; the one thing this must never do is fall back to total
disclosure on a value it did not understand.

### 5.3 Idempotency, stated rather than hidden

`PlatformExecutor` does not carry the durable `ReplayLease` into these
handlers — a pre-existing gap for any non-read platform op. It is tolerable
here and would not be for a domain write, because **both ops are idempotent
by construction**: an export is a pure function of state (a re-run writes a
byte-identical bundle to a new directory), and an erasure of an
already-erased space destroys nothing that is there and deletes no rows
that remain. A crash between reservation and response costs a duplicate
bundle at worst. The in-memory replay cache still makes a retry verbatim;
what is skipped is the `$replay` journal. **[open — platform-op work, not
exit-op work]**

---

## 6 · Wire classes [built]

Three, each with a producer and a row in `api-engine-design.md`'s
inventory, which is checked in both directions with an empty allow-list.

| class | status | producer |
|---|---|---|
| `schema_drift` | 500 | `exit_ops::export_refusal` ← `ExportError::Drift` |
| `space_pinned` | 409 | `exit_ops::erase_refusal` ← `EraseRefusal::ConsumerPinned` |
| `not_confirmed` | 400 | `exit_ops::erase_refusal` ← `EraseRefusal::NotConfirmed` |

`schema_drift` was checked against the migration work first, as instructed.
That work's refusals (`unseal_refused`, `key_change_refused`,
`drop_read_by_active_op`, `probe_not_empty`, `probe_unavailable`,
`sealed_epochs_unverified`) are **migration-report** classes tabulated in
`plan-schema-evolution.md` §8.1 and owned by neither inventory, so none of
them was reusable. The live-drift refusal that *did* already exist —
`ordo_host::deploy::LoadRefusal::SchemaDrift` — is a Rust enum variant with
no wire name, because it refuses a **load** and never meets a caller. The
export is the first producer of that fact that does, so the class is minted
once and covers both.

`ExportError::SealedUnopened` and `EraseRefusal::Custody` reuse
`sealed_open_failed`, which already means exactly *a sealed value did not
open — key epoch, wrong column, or a tampered payload*, reached from the
other side.

### 6.1 Why the export refuses on a drifted database

The bundle ships `schema.sql` and the rows side by side, and a reader joins
them. If the live table has drifted from the artifact — a migration
half-applied, a restore onto an older shape, a column changed by hand —
then either the export's `SELECT` fails (a column the artifact declares and
the table lacks) or, worse, it succeeds and the bundle describes itself
wrongly.

So `export_space` builds a shadow from the artifact's own DDL and asks
`Pool::live_drift` **before the first row is read** — the same comparison
the load gate makes, asked by the other reader that has to trust it. It
inherits the constraint half of that gate for free, so a dropped `UNIQUE`
or `FOREIGN KEY` refuses an export too.

The shadow gets its own namespace (`ordo_shadow_export`). The load gate
drops and rebuilds `ordo_shadow` on every verification and drops it again
afterwards, so sharing one would let an unrelated boot delete the export's
schema mid-comparison.

---

## 7 · What is built, and what is not

**Built.**

| | file |
|---|---|
| `physical_columns` + the two anti-drift tests | `crates/ordo-schema/src/ddl.rs` |
| `KeyProvider::destroy_space`, `WrappedKeyStore::drop_space` | `crates/ordo-crypto/src/custody.rs` |
| the export engine | `crates/ordo-exec/src/export.rs` |
| the erasure engine | `crates/ordo-exec/src/erase.rs` |
| `build_shadow_in`, `EXPORT_SHADOW` | `crates/ordo-exec/src/shape.rs` |
| the two ops | `crates/ordo-host/src/exit_ops.rs`, `platform_ops.rs` |
| owner-only registration | `crates/ordo-host/src/lib.rs` |
| three wire classes | `crates/ordo-api/src/error.rs`, `docs/api-engine-design.md` |
| 10 export tests, 7 erasure tests, 6 door tests | `crates/ordo-exec/tests/{export,erase}.rs`, `crates/ordo-host/tests/exit_ops.rs` |

**Not built, named rather than left to be discovered.**

1. ~~**Blob bytes.**~~ **Built** — `plan-blob.md` B5, checklist 17g. The
   walk landed at **`blobs/`**, not the `files/` this sentence predicted,
   because `migration.md` §6 says "blob bytes" and the manifest's own key
   is `blobs`; one name, taken from the document that names it. It is
   driven by the `$blobref` rows rather than by a schema walk, which is
   what lets it file each object under the entity and field the bucket
   deliberately does not encode. `SealedMode::Ciphertext` exports the
   STREAM envelope unopened, the same rule the sealed columns beside it
   follow, and a reference no live row names is skipped — its bytes stopped
   being the space's data when the row went. The `unbuilt` list is now
   empty and stays in the manifest, because it is the machine-readable
   place the next gap gets declared.

   **The manifest describes each object, 2026-09-26** (`blobs.files`), and
   the addition was owed to import rather than chosen here. An object's
   **mime** is a sealed envelope on the `$blobref` row, so a reader cannot open
   it and cannot derive it from the bytes, so before this it was nowhere in
   the bundle and an import could only have defaulted every file to
   `application/octet-stream`, which is a value the customer never stored.
   Each entry carries its member, entity, field, id, and its mime **when the
   member holds plaintext**. It is absent in `ciphertext` mode and for a
   `blob private` field, because a bundle the platform did not decrypt must not
   disclose through the manifest what it refused to disclose through the
   member, and because a mime describing bytes that are not the file's bytes
   is a false description. The entity, field and id are stated rather than
   left to be recovered from the member's name: splitting the path would be
   parsing a string this module formatted, and the first id holding a `/`
   would place an object on the wrong row rather than refuse.
   `plan-import.md` §10.6 is the other half of this.

   *Superseded text, kept so the prediction can be read against the
   outcome:* the export was to gain a
   `files/<entity>/<field>/<id>` walk driven by the same
   `physical_columns` role mechanism (a `Blob` role), and `blob private`
   bytes take the ciphertext path for the same reason sealed fields do.
   **[open — blocked on blob custody]**

2. **Delivering the bundle.** The op writes to a deployment-configured
   directory and returns the path and the manifest. Handing the bytes to a
   caller (a presigned object-store URL, a streaming download, a CLI
   `ordo export <space> --into <dir>`) is unbuilt. A self-hoster has the
   directory; a hosted tenant does not yet have a way to fetch it.
   **[open]**

3. **BYO-DB specifics.** §8 pins *"BYO-DB spaces: the same ops, same
   gating — custody changes where the bytes live, never who may end them."*
   The ops run against `Pool`, which is whatever database the deployment
   resolved, so a BYO-DB space exports and erases through the identical
   path with no branch — that half is free and is why there is no BYO
   branch in this code. What is **not** covered: a BYO-DB deletion removes
   the *rows*, and the customer's own backups of their own database are
   outside the platform's reach entirely. Crypto-erasure is what makes that
   survivable — the DEKs are the platform's, and destroying them makes the
   customer's own backups of their own sealed columns unopenable — but a
   plaintext column in a customer-held backup is beyond any op. This
   belongs on the BYO-DB consent page and is not there. **[open]**

4. **The confirmation window** (§4.4), **legal hold** (§4.3), the
   **`$replay` journal for platform ops** (§5.3), and **`ordo import`**,
   without which a sealed field cannot be round-tripped (§3.1).

---

## 8 · One hazard found on the way, unrelated to exit ops

`Pool::build_shadow` set `search_path` on a **pooled** connection and never
reset it. `SET` is session state and the connection went straight back into
the pool, so the next unrelated query on it would have resolved `orders` to
the shadow schema's empty copy — a read returning **nothing, correctly
typed, with no error anywhere**. The only existing caller
(`shape::verify_registry`) dials a throwaway pool and drops it, which hid
the hazard rather than removing it; the export runs on the serving pool and
would have met it in production.

Fixed by ending the build statement with `RESET search_path`, in the same
simple query so it lands on the same connection. A failure mid-DDL rolls
the implicit transaction back, `SET` included, so the reset is not needed
on that path and is harmless on it.
