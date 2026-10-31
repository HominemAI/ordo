# Ordo Migration: Into and Out

Companion to `definition-language.md` (§3 shapes, §6 writes, §9 deployment
model), `adoption-dx-plan.md` (§7 migration paths, which this doc expands),
`security-logging-design.md` (§7 command log),
`implementation.md` (custody, BYO-DB), and `std-library.md`.

Scope: **arriving from another system, and leaving.** The deployment model
(spec §9) already handles evolving a deployed Ordo schema:
additive/transforming/destructive classification, declared mappings,
backfill jobs, per-target replay. Nothing there is duplicated here. This
doc covers the step *before* §9 has anything to diff against, where an
existing system's schema and data become an Ordo domain and a populated
space. It also covers the documented exit.

Markers: **[settled]** restated from other docs · **[proposed]** adopted
here, pending owner review.

---

## 1. The migration pipeline [proposed]

One shape for every source, five inspectable stages:

```
introspect → draft shapes + mapping → review → apply (DDL) → import → verify → cutover
```

**What of this pipeline exists, as of 2026-09-26.** **Step 4 is built and
callable**: `ordo_exec::import::import_space` loads an export bundle through
the write protocol, so every row gets its seals, its blind indexes and its
place in the chain (11-10a), and `std.space.import` with `ordo import` in
front of it is how a deployment owner reaches it (`plan-import.md` §11). The
custody objection that held it up for six days is answered: an import that
cannot seal cannot write a sealed field, so the
import runs **on the deployment**, which holds custody, and the command asks
it to. The bundle is staged on the deployment under the destination space's
own directory. Delivering one to a hosted deployment is still unbuilt.

**Rows that name people** resolve through a map the deployment owner stages beside
the bundle (`plan-import.md` §12, owner-ruled 2026-09-26). The map may only
name people who are already members of the destination, and any source person
it does not cover stops the import by name. Nothing about a membership travels
in a bundle, because a bundle that could create one would be a file that
grants access to the space it is loaded into.

**The tenant's access profile** travels in `profile.json`: its op lines, per-member
overrides, groups and group links (`howto-roles.md` §8). Import remakes the groups,
places every person through the same map, and refuses a line for an op the program no
longer has. It admits nobody.

**Step 1 has no CLI verb**: `ordo introspect` is unbuilt, and by the owner's
2026-09-20 ruling (18k) it is not wanted before the first deployment. So
migration today is the sentence that ruling gives: stand an Ordo database up
beside the old one, create the schema from the `.ordo` definition, import the
rows through the write protocol, cut over. The bundle for step 4 is
produced by whatever reads the source system, which is the piece a first
migration writes for itself.

1. **`ordo introspect <source>`**: connect, emit DRAFT `.ordo` shapes plus
   an **import mapping** file (§2). Both are text artifacts in the repo.
2. **Review**: a human or LLM edits the drafts. Same posture as index
   advisors [settled]: suggestions land in files for approval, never
   runtime magic. Nothing introspect emits is authoritative until a person
   commits it.
3. **`ordo apply`**: the ordinary deploy. It compiles the reviewed shapes,
   runs DDL, registers std and authored ops. From the deployment model's
   view this is a first deploy, all-additive by construction.
4. **`ordo import <mapping>`**: bulk data transfer under genesis import
   mode (§3).
5. **Verify**: count parity, hash sampling, ref integrity, the
   didn't-migrate manifest (§4). The import is not done until the manifest
   is empty or explicitly accepted.
6. **Cutover**: per one of the patterns in §5.

The rule the pipeline enforces: **nothing goes source→production in one
opaque step.** Every stage's artifact is a file or a report a reviewer can
read, diff, and re-run. A migration is a sequence of reviewable decisions.

### The mapping file [proposed: mechanism pinned, format not final]

Introspect emits, next to the draft shapes, a declarative mapping:
source table/collection → entity, source column/path → field, the declared
**external-id unique key** per entity (§3), per-field transforms where the
type mapping demands one, and per-item disposition (`import` / `skip` /
`review`). `ordo import` consumes it; the verify stage reports against it.
It is the migration's contract: greppable, diffable, and the artifact a
re-run is idempotent over.

---

## 2. `ordo introspect` [proposed]

Part of the spec'd toolchain (`ordo introspect` + `ordo import`), not a
wiki page. Adoption-dx §8 items 13–14 name the build slots. Output is a
*draft for human editing* [settled posture, restated]: obvious `owned`
candidates are flagged, never auto-marked (ownership is a claim, spec §3);
everything outside the closed type inventory lands on an explicit worklist.

### 2a. Raw Postgres

The deepest introspector, deliberately: it is most of the work for
Supabase too (§7).

**Structure:**

- tables → entities; names canonicalized `snake_case` → `lowerCamel`
  (identifiers admit no underscores [settled]); the original name is
  recorded in the mapping so the import can find its source.
- FKs → refs (`field -> entity`). FK direction already matches ref
  direction (many side holds the column), so this is mechanical.
- unique constraints → `unique` / `unique (a, b)`, **with a scope change
  worth stating in the report**: Ordo's `unique` is always **space-scoped**,
  never deployment-wide. Importing a single-tenant database into one space is
  a faithful translation; splitting one source database across several spaces
  turns a table-wide constraint into a per-space one, which is usually what a
  tenant-per-space model wants and is occasionally a silent weakening. There
  is no deployment-wide uniqueness spelling to fall back on, so any source
  constraint that genuinely spanned tenants goes on the worklist.
- CHECK constraints → `check` where the expression fits the closed
  grammar; otherwise worklist with the original SQL quoted.
- indexes carried as `index (…)`; redundant ones (ref auto-indexes)
  dropped silently, noted in the mapping.

**Type mapping:**

| Postgres | Ordo | Notes |
|---|---|---|
| `text`, `varchar(n)` | `text` / `text(n)` | |
| `integer`, `bigint`, `smallint` | `int` | |
| `boolean` | `bool` | |
| `date` | `date` | |
| `timestamptz`, `timestamp` | `datetime` | bare `timestamp` flagged: zone assumed UTC, stated in the mapping |
| `numeric(p,s)` | `money` | exact decimal either side, so lossless; flagged when the column is plainly not currency, since `money` is Ordo's exact-decimal storage type. **Note for the queries that follow the data:** `+ - *` on `money` stay exact `money`, but `/` and `avg` yield `decimal` at a fixed fractional scale of 12 and **nothing rounds implicitly**. Every place the source SQL divided into a currency column becomes an explicit `round`/`floor`/`ceil` (or `allocate`, where shares must sum to a total) in the drafted op |
| `real`, `double precision` | `money`/`int` per declared scale | **stated-loss warning, always**: binary floats have no exact decimal identity. Introspect emits nearest-decimal at a declared scale and flags every float column for review. Round-trip is not identity |
| `interval` | `duration` | worklist when the value mixes units the literal grammar can't spell |
| `uuid` PK | typed `id` | old value preserved in the id-mapping table (§3); optionally also as a declared `externalId` field |
| `serial`/`bigserial` PK | typed `id` | ids are server-minted, always [settled]. Old serials go to the id-mapping table, kept during import, queryable during transition, deletable after |
| pg `enum` | `enum(…)` | [lean type, carried as such] |
| `bytea` | `blob inline` suggestion | policy cap applies; oversize columns worklist |
| `jsonb`, arrays, ranges, custom types | worklist | suggestion where structure is evident: array-of-FK → link entity, embedded object → owned child entity |

**What does not translate (reported, never dropped silently):**

- **Views** → named-op suggestions. A view is a query with a name; where
  the SQL fits the pipeline grammar, introspect drafts the op body; where
  it doesn't (window functions, recursive CTEs), the worklist entry quotes
  the SQL and names the blessed pattern (spec §11).
- **Triggers** → routine suggestions, **flagged, never auto-emitted as
  runnable**. A trigger's body is imperative SQL; the draft states what
  the trigger appears to do and sketches the rule-tier shape
  (`when … -> run …` + a query-`each ->` body). Human writes the routine.
- **RLS policies** → guard **suggestions**, with the honest note printed
  in the draft itself: **policy semantics ≠ guard semantics.** RLS
  composes permissively across policies; guards are single-expression,
  replace-not-compose, and inline into op manifests. A mechanically
  converted policy can be *wrong while parsing cleanly*. Every suggested
  guard is emitted commented-out and requires individual human review.
  Role-based policies map to access profiles (runtime state, not syntax)
  and are reported as profile-template suggestions, not shape text.
- Stored procedures/functions → worklist (op or tier-2 effector, human
  decision). Sequences, extensions, publications → reported, no mapping.

### 2b. Supabase

The Postgres path plus platform semantics:

- **`auth.users` → principals.** Rows become human principals in the
  directory (provisioning path, audited as `system:identity` [settled
  taxonomy]). Password hashes do not transfer: users re-establish
  credentials via the deployment's auth methods (email-OTP first login is
  the credible path). OAuth-linked identities map to `FederatedProvider`
  authenticator links where the provider is configured.
- **Storage buckets → blob fields.** Objects referenced from rows become
  `blob` on the referencing entity (bytes re-uploaded through the sealer;
  custody begins at entry, §3); unreferenced objects worklist. Public
  buckets noted: Ordo has no anonymous serving; share links are the
  replacement, stated in the manifest.
- **RLS → guards + profiles**, per the concept map (adoption-dx §7.2):
  row-ownership policies suggest `access` guards / `$caller` conditions;
  role-tier policies suggest profile templates. Same commented-out,
  review-each posture as raw Postgres.
- Edge functions → tier-2 effector or routine worklist entries.

### 2c. Firebase / Firestore

Document → relational reshaping, the most transformative path:

- **Subcollections → owned entities.** A subcollection's existential
  dependence on its parent document is exactly the `owned` test, the one
  case where introspect flags `owned` with high confidence (still never
  auto-marked).
- **Denormalized duplicates → refs + a dedup pass.** Repeated embedded
  objects (the same user summary in a thousand documents) draft as one
  entity plus a ref; the mapping declares the dedup key; the import's
  converge-or-create collapses duplicates wholesale (§3). Conflicting
  duplicates (same key, different field values) land in the verify
  manifest, not silently last-write.
- **No schema → shape inference from sampled documents, with confidence
  notes.** Introspect samples (policy-tunable N per collection) and emits
  per-field confidence in the draft: `seen in 100% of 5,000 sampled` vs
  `seen in 3%`. The latter drafts as nullable with a review flag. Fields
  with mixed types worklist. The inference is a starting point, stated as
  such in the file header it generates.
- Security rules → guard suggestions, same honesty note as RLS but
  louder: Firestore rules are imperative and per-path; expect rewrites,
  not translations.

### 2d. Convex

The best structural fit:

- **`schema.ts` → shapes.** Convex validators map well: `v.string()` →
  `text`, `v.number()` → the float warning (Convex numbers are doubles),
  `v.boolean()` → `bool`, `v.id("t")` → ref, `v.optional` → `?`,
  `v.union` of literals → `enum`. Nested objects → owned-entity
  suggestions.
- **Functions → op suggestions; bodies need rewrite (the report says so).** Queries
  and mutations are TypeScript; introspect emits the op *signature*
  (name, params, an inferred result shape where the return type is
  declared) and a stub body with the original source quoted. The pipeline
  grammar expresses most Convex queries; nobody should expect a
  transpiler. Actions → tier-2 effector worklist entries.
- **`_id` → typed-id mapping**, using the same id-mapping table as serial PKs.
  `_creationTime` → a declared `datetime` field where referenced,
  otherwise dropped (UUIDv7 ids carry creation order).

### 2e. Generic CSV / JSON

The floor: no source connection, just files. Column/key inference with the
Firestore confidence machinery (sampled, typed by observed values, stated
in the draft), one entity per file unless the mapping declares joins,
external-id key declared by the human (a CSV has no opinion). Exists so
"migrate into Ordo" never has a source it simply cannot start from.

---

## 3. Bulk import mechanics [proposed]

The real design decision. The command log's contract is per-row attributed
commands with before/after images [settled]. Importing 10M rows as
10M per-row image-bearing entries is untenable: the log would dwarf the
data, and every entry would attribute to the same actor saying the same
thing. The answer is a distinguished mode that goes through the gate.

### Genesis import mode

- **One actor:** `source:import-<name>`, a source principal in the
  settled taxonomy (write-only, IaC-declared, attributed). `<name>` is the
  mapping file's declared name; two imports are two actors.
- **Batch = one command group.** Rows land in per-batch groups (size
  policy-tunable, default in the thousands); each group commits in one
  transaction like any group [settled]. Atomicity, ordering-per-space, and
  the single-space rule are all inherited.
- **Images are summarized, not per-row.** A genesis batch group's
  canonical encoding carries: row **count**, the **minted-id range/set**
  (plaintext skeleton, as ids always are), and a **content hash** over the
  batch's row payloads (post-seal ciphertext for sealed fields, consistent
  with the `SealedRef` redaction rule). There are no per-row before/after
  images: creates have no before, and the after *is* the database.
- **The chain records batch hashes.** Each batch group enters the
  per-space hash chain exactly like any group
  (`SHA-256(prev ‖ canonical-encoding(group))`), so the import is
  **tamper-evident without 10M entries**: altering any imported row breaks
  its batch's content hash; altering the log breaks the chain. Ten million
  rows cost thousands of chain entries.
- **A genesis checkpoint closes the import.** A signed checkpoint (the
  settled `Signer`-trait machinery) marking the boundary: space, chain
  position, head hash, the import's total counts per entity, the verify
  report hash (§4). The boundary's meaning is stated plainly: **history
  before this point is the source system's problem; history after is
  fully attributed Ordo history.** `std.E.history` on an imported row
  shows one honest entry ("arrived in import `<name>`, batch N") and
  everything after in full fidelity.

### Semantics that follow

- **Ids are server-minted, always** [settled, with no exception for imports].
  A persisted **old-id → new-id mapping table** is written during import:
  queryable during the transition (redirects, foreign systems still
  holding old keys, the dual-write bridge §5), deletable after. Its
  deletion is an ordinary attributed command.
- **Idempotent re-runs** ride the spec's own ingestion pattern, applied
  wholesale: every imported entity declares an **external-id unique key**
  in the mapping, and batch rows are converge-or-create statements on that
  key [settled semantics, §6]. A crashed import re-runs from the top and
  converges; a corrected mapping re-runs and updates; `duplicate` and
  `conflicting_identity` rejections surface in the verify manifest instead
  of aborting the batch.
- **Revert:** genesis batches are create-only (converge on re-run), so the
  group inverse is delete-by-minted-ids, and `revert_group` works from the id
  set without per-row images. Whole-import abort before the genesis
  checkpoint = revert the batches in reverse order, through the gate, as
  always. After the checkpoint is accepted, batches age out of
  `undoHorizon` like any history.
- **Triggers are quiet during genesis.** Batch envelopes carry an import
  flag; listener triggers do not fire per batch (a 10M-row import must not
  wake every routine thousands of times). The genesis checkpoint emits
  **one** event, so routines baseline once over final state. Clock
  triggers unaffected. Policy-tunable for the rare feed that wants
  per-batch wake-ups.
  **One consequence to design for:** a routine woken by that checkpoint is
  **listener**-woken, and a listener-woken run may touch only rows reachable
  from `$trigger.ids` (§spec-8, trigger provenance). A baseline rule that must
  range over the whole imported set therefore has to say so (`run …
  unguarded:` on its header) or be written as a clock routine that runs after
  the import instead. Decide this while writing the import plan, so cutover
  holds no surprise.
- **Custody is respected at import.** Sealed fields seal **on entry**.
  Batch payloads pass the sealer before the database, and the batch
  content hash covers ciphertext. Plaintext never lands "temporarily."
  Blob values upload through the blob pipeline under the field's declared
  custody. **Client-first spaces import on-device**: the import runs
  through the embedded engine against the local store, seals under
  client-held keys, and syncs sealed segments up. The server never sees
  the migration's plaintext, same as it never sees the app's. BYO-DB
  spaces import into their own database via the resolver, nothing special.

---

## 4. Verification [proposed]

`ordo import` ends with a verify pass; its output is a file, and the
genesis checkpoint embeds its hash, so the import's closure is itself
tamper-evident.

- **Count parity per entity**: source count vs imported count vs mapping
  expectation, three columns, any mismatch itemized.
- **Content-hash sampling**: per entity, a policy-tunable sample re-read
  through `std.E.get` and hashed against the source row under the
  mapping's transforms. This catches transform drift as well as row loss.
  (Full re-hash available for the paranoid; sampling is the default
  because the batch hashes already pin bulk integrity.)
- **Ref-integrity report**: every ref resolved; dangling source FKs
  (rows referencing deleted targets, which are common in the wild) itemized with
  disposition options (skip row / null the ref / create tombstone target),
  chosen in the mapping, re-run to converge.
- **The didn't-migrate manifest**, diffable and the pipeline's honest core:
  rows dropped and why, types narrowed (every float column, every
  truncated `text(n)`), constraints not carried, views/triggers/policies
  still on the worklist, guard suggestions still commented out.

**The import is not done until the manifest is empty or accepted.**
Acceptance is explicit: a signed-off manifest hash in the genesis
checkpoint. "We knew about those 40 rows" is recorded in the log.

---

## 5. Cutover patterns

Three patterns, with guidance by size. All three end the same way: the
genesis checkpoint (plus, for the bridge, a bridge-retired marker) is the
moment Ordo history begins.

- **Big-bang.** Stop the source, import, verify, point clients at Ordo.
  Right for small apps and anything with a tolerable maintenance window.
  It is the default recommendation, because the dual-write window is
  complexity most migrations do not need. The pipeline's re-run
  idempotency makes rehearsal cheap: import into a staging space twice
  before doing it live once.
- **Dual-write window.** The source keeps running. A **sync bridge**
  (a feed-shaped, one-way replicator in the consented-feed mold) replays
  source changes as ordinary import batches (same `source:import-<name>`
  actor, same converge-or-create on the external-id key, same quiet-flag
  rules) until cutover. The bridge is **bounded and then dies**: it is
  declared with an expiry, its retirement is a logged command, and it is
  one direction in one window. Right for
  apps that cannot take a window. It is a poor way to avoid deciding.
- **Read-only-source archive.** Import once, freeze the source read-only,
  keep it as its own historical record instead of migrating deep history.
  Right when old data is referenced rarely and the mapping cost of full
  history exceeds its value. The id-mapping table plus the source's own
  backup is the archive.

---

## 6. Migration out [settled, restated as a trust signal]

The door out exists. **Exit ops are settled** (spec §8):
full space export and space deletion always exist, owner-only, step-up
gated, command-logged. BYO-DB spaces are included, with the same gating.

What export yields, in standard formats another system's introspector can
read:

- **`schema.sql`**: the deployed shapes as plain DDL (the compiler
  already owns the DDL; export renders it without Ordo-isms).
- **Data**: per-entity JSONL (and SQL loadable against `schema.sql`),
  decrypted under the owner's authority through the normal key-release
  path. Client-first spaces export from the device, where plaintext lives.
- **Files**: blob bytes, laid out by entity/field/id.
- **Command-log archive**: the settled JSONL bundle format, chain
  hashes included, so the history's integrity survives the platform.

Leaving Ordo is a documented path. Ordo makes no lock-in claim, and
the exit is enforced by the same machinery as everything
else: an op with a manifest.

**Built [2026-09-06].** `std.space.export` and `std.space.delete` exist,
owner-only and step-up gated. `docs/plan-export.md` is the build plan and
records what the two ops rule that this section left open, chiefly that
"command-logged" means the audit spine's *security-event* half for these
two (export mutates no row, and deletion destroys the chain), and that an
unopenable sealed value refuses rather than exporting a null. ~~**Blob bytes
are the one member of the list above that is not produced**~~ (**they are
produced now**, checklist 17g). The bundle carries them at
`blobs/<entity>/<field>/<id>`, which is this section's own layout, and the
manifest's `unbuilt` list is empty for the first time. Note the bucket does
**not** use that layout and must not: an object key carrying an entity and
a field would enumerate the schema to anyone who can list it, so the store
keys objects `<space>/<hex handle>` and the export builds the readable
layout from the `$blobref` rows, which carry the entity and the field.
Delivering the bundle to a caller (a presigned URL, a stream, a CLI
`--into`) also remains: the op writes to a deployment-configured directory
and returns the path.

---

## 7. Coverage roadmap [proposed]

Migration generation is part of the spec'd toolchain. `ordo introspect`
and `ordo import` are build items (adoption-dx §8 rows 13–14), and their
outputs (draft shapes, mapping files, verify manifests) are conformance
surface like any compiler output: golden-file tested per source fixture.

Order of shipment:

1. **Postgres** first. It is ~80% of Supabase, the BYO-DB deployment
   model already speaks it, and it is the introspector every other SQL
   source degrades to.
2. **Generic CSV/JSON**: small, unblocks every source without a
   connector, and exercises the mapping/import/verify spine independently of
   any introspector.
3. **Supabase**: the Postgres path plus auth/storage/RLS layers (§2b).
4. **Convex**: schema.ts is a clean parse. It ships with the
   function-stub posture (§2d).
5. **Firestore** last, because it is inference-heavy. The confidence machinery
   it needs is shared with CSV/JSON and matures there first.

Genesis import mode (§3) precedes all introspectors: `ordo import` against
a hand-written mapping is useful on day one. The chain semantics
(batch hashing, the genesis checkpoint, quiet triggers) are commandlog
work and need no connector.
