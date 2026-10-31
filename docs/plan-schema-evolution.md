# Ordo schema evolution — design & build plan

Companion to `definition-language.md` §9 (the settled deployment model),
`implementation.md` ("Migrations: failure modes, rollback & recovery",
"Backup, restore & disaster recovery"), `migration.md` (into and out — a
*different* problem, see §0.2), `deploying.md` §7, and
`audit-board-coverage.md` finding #9.

Markers: **[settled]** restated from a settled document · **[proposed]**
adopted here, pending owner review · **[open]** named and not answered.

**Extended 2026-10-01** by `plan-evolution-plans.md`: stored plans with a
reverse per step, options per change instead of refusals, retention from
the oldest tenant's version, and lock timeouts. The owner's notes on the
Grief Remediation tab asked for them. This document's classes stay.

Status: build plan. **Increments 1 and 2 have landed**: see §8.1 and
§8.2 for what is built, what moved in the taxonomy, and the corrections
this document owes. Increments 3–5 are unbuilt. Every claim about the
current code was checked against the code and carries its file.

---

## 0 · The problem, stated precisely

### 0.1 A deployed Ordo schema cannot change

`ordo_schema::ddl_for` (`crates/ordo-schema/src/ddl.rs:161`) emits
`CREATE TABLE` for a whole schema from scratch. There is no diff, no
`ALTER` beyond the `ADD CONSTRAINT` statements it emits at creation time,
no backfill, and no migration state anywhere in `crates/`. `ordo apply`
says so in its own doc comment
(`crates/ordo-cli/src/operator.rs:501`): *"It does not run DDL."*
Nothing else runs it either — the only callers of `ddl_for` outside its
own tests are `ordo apply` (which stores the text in a column) and the
conformance `Fixture`, which pipes it to `psql` to build a test database.

On its own that is a missing feature. What makes it a hard stop is the
**shape-verification gate**, which shipped and works: `Deployment::swap`
takes a `Verified` and nothing else, `verify_candidate` is its only
constructor, and `Verified::from_gate` refuses on any live-schema drift
(`crates/ordo-host/src/deploy.rs:136`). A deployment whose database
disagrees with its compiler does not serve. So the safety mechanism and
the missing mechanism compose: you cannot change the schema, and if you
change it by hand the deployment refuses to boot.

This is the largest gap in the system, and it blocks anyone past their
first `.ordo` file.

### 0.2 What this document is not

`migration.md` covers **arriving from another system and leaving** —
`ordo introspect`, `ordo import`, genesis import mode, exit ops. That is
the step *before* §9 has anything to diff against. This document is §9:
evolving a schema that is already deployed and already holds rows. The
two share one mechanism (genesis batch groups, §9.3 below) and nothing
else.

---

## 1 · The starting state, verified

Every increment in §8 is a delta on these facts. They were read, not
assumed.

**The DDL generator.** `ddl_for` emits, per entity, in a fixed order: the
`$space` tenancy column; one column per field (`bytea` if `sealed`,
`NOT NULL` unless optional, `DEFAULT <literal>` where the default lowers);
a `<field>$bidx bytea NOT NULL` column for every `sealed indexed` field; a
named `CHECK` per `enum_values` field; a `$path text NOT NULL` column for
tree entities; a `PRIMARY KEY ($space, id)`. Then, in three later passes:
composite `FOREIGN KEY ($space, <ref>) REFERENCES t ($space, id)`; single
and multi-column `UNIQUE`, always with `$space` leading, with
`NULLS NOT DISTINCT` except on blind-index columns; and `CREATE INDEX` for
declared `index (…)` keys. That list is the whole DDL surface a diff has to
cover.

**One set of tables holds every space.** There is no per-space schema and
no per-space table. Tenancy is the `$space` column
(`ddl.rs:69`), and the current host serves one hardcoded space
(`ordo_host::SPACE = "conformance"`, `crates/ordo-host/src/lib.rs:80`) with
`Fixture::in_space` as the multi-space test path. There is **no table
listing a deployment's spaces** — grep finds none. Only BYO-DB gives a
space its own database.

**The artifact registry.** `$artifacts` is content-addressed by version
hash and holds `artifact` (the byte-stable `ordo plan` document), `source`,
`ddl`, `applied_at`, `actor`. `$activations` is an append-only log of
re-points; rollback is an ordinary activation naming an older version
(`crates/ordo-exec/src/registry.rs`). The FK from `$activations.version`
makes "old artifacts are retained" a schema property.

**The shape gate, in two halves.**
`ordo_exec::shape::Pool::build_shadow` builds an empty `ordo_shadow`
schema *from the DDL the artifact carries*, and `observed_shape` reads a
row type out of Postgres for each op's planned SQL; `compare` matches it
against the compiler's inferred outputs. That half proves **planner
versus Postgres**, never that the live database is the one the program was
compiled for. The second half is `Pool::live_drift(SHADOW, "public")`
(`crates/ordo-exec/src/shape.rs:154`), and it is deliberately **one-way**:
every shadow column must exist live with the same type, length, nullability
and collation, and *extra live columns and extra live tables are not
drift*. The comment says why in as many words: `migration.md`'s
additive-then-swap sequence puts the new column in the database one deploy
before the artifact that reads it.

**That one-way rule is the single most load-bearing fact in this plan.**
It is what makes an in-place expand-contract migration legal without
touching the gate.

**Grants pin manifests.** A capability grant stores the combined hash of
the manifests it consented to (`crates/ordo-host/src/grants.rs:47`,
`crates/ordo-host/src/lib.rs:1091`), and a manifest carries `scope` at
field grain, `outputs`, `guards` and the per-op version
(`crates/ordo-host/src/lib.rs:559`). Changing what an op touches or
discloses kills the grant until a human re-consents — `deploying.md` §7
states it as an operator-facing promise.

**Reverts already refuse across a schema change.** `undo.rs` refuses with
`unrevertable_image` when *"the logged image names `entity.field`, which
the loaded schema no longer declares"* (`crates/ordo-exec/src/undo.rs:1584`
onward), refuses `unrevertable_tree` on tree entities, and re-seals
restored plaintext **under the current epoch** rather than writing the old
envelope back. All three are directly relevant to §7.

**The write funnel is single.** Every domain write goes through
`Executor::write_leased` → `write_retrying` → `write_attempt`
(`crates/ordo-exec/src/lib.rs:758`), wrapped by `LoggedExecutor`. There is
exactly one place to install a write barrier in-process.

**The job queue is thin.** `$jobs` is `(id, kind, payload, claimed)` with
`FOR UPDATE SKIP LOCKED` claiming (`crates/ordo-exec/src/jobs.rs:44`).
There is no cursor, no attempt count, no dead-letter, no progress column. A
resumable chunk-checkpointed backfill needs all four; the queue gives it a
claim and nothing else.

**The probe precedent exists and is the right one.**
`crates/ordo-exec/src/nfc.rs` is a migration probe for exactly this shape
of problem: it counts the rows a conversion would touch, names the values
that would **collide inside a unique column** before anything is written,
and *reports and never repairs* — "which row survives is the deployment's
decision, and a probe that took it would be making it for them." Every
transforming class in §5 gets a probe built on that model.

### 1.1 Two defects found while reading, which this work must fix first

**(a) A schema-only change is a silent no-op deploy.** `registry::store`
decides artifact identity on the `artifact` document alone
(`crates/ordo-exec/src/registry.rs:194`): *"Only the artifact document
decides identity… `source`, `ddl`, `applied_at` and `actor` are
provenance."* But `entity_artifact`
(`crates/ordo-plan/src/artifact.rs:466`) emits only `name`, `type`,
`optional`, `unique`, `sealed`, `indexed` per field, plus `key`, `tree`,
`acl_levels` and guards per entity. It carries **no field default, no
`normalize`, no `enum_values`, no composite `unique_keys`, and no declared
`indexes`** — grep for those four names in `artifact.rs` returns nothing.

So a program that adds `index (account, date)` produces byte-identical
artifact bytes, the same program version, `Applied::AlreadyStored`,
`Activated::AlreadyActive` — and the new DDL, which *does* differ, is
discarded. `ordo apply` prints "already present, byte-identical" and
"already active; nothing re-pointed", and the operator's change is gone.
The same holds for `unique (a, b)`, which is worse, because the operator
now believes a uniqueness constraint is deployed and it is not.

This must be fixed before any of §8, and it is small: either hash the DDL
into the program version, or make `store` treat a differing `ddl` under an
identical `artifact` as a `VersionCollision`. **[proposed]** — hash the
generated DDL into `program_version` alongside the document. It keeps one
version meaning one deployable state, which is what the version is for,
and `artifact.rs:105` already explains that the DDL is *deliberately* not
in the document (it is a rendering, and it diffs badly); hashing it does
not put it in the document.

**(b) The drift check cannot see constraints.** `live_drift` reads
`information_schema.columns` only (`shape.rs:213`). Adding or dropping a
`UNIQUE`, a `CHECK`, a `FOREIGN KEY`, an `INDEX`, or the
`NULLS NOT DISTINCT` modifier is **invisible** to the gate. Today that is
tolerable because the DDL is applied whole or not at all. The moment
migrations exist, half the taxonomy in §5 is constraint work, and "did the
migration run?" would have no truthful answer. Extending `live_drift` to
constraints is part of increment 1, not a later polish.

---

## 2 · What DL §9 settles, quoted

§9 is `## 9. Deployment model [settled]`, and it is short enough to quote

> **RESOLVED 2026-09-08 (owner ruling; see corpus-rulings.md):** option 2
> — summarised batch groups — is the default; a creator may declare a
> backfill option 1 (full per-row images) per migration, and the group
> encoding self-identifies. The recommendation below is superseded where
> it disagrees. 13c builds against this.

almost whole, because the whole of it is owed.

> One engine, Terraform-shaped, across **all** definition kinds:
>
> 1. `plan` — diff declared vs. deployed (shapes, ops, routines, clients, policies)
> 2. Classify each change:
>    - **additive** (new entity, defaulted field, new op) → auto-applies
>    - **transforming** (rename, type change, required-field addition) → requires a
>      declared mapping; runs as a backfill job. Sealed-field transforms route
>      through the worker (decrypt → transform → re-encrypt), never SQL.
>    - **destructive** (drops) → explicit confirmation; the apply itself is a
>      command with an actor. For BYO-DB spaces: **space owner** confirms drops;
>      the domain developer owns everything else.
> 3. `apply` — executes; every apply lands in the command log.

And the four sub-classes:

> Defaulted-field additions split into four classes: **metadata-only** (`= null` —
> truly additive), **constant backfill** (deterministic literal — additive, bulk
> write), **execution-time backfill** (`= today`, `= newid` — a *transforming* job:
> each existing row's resolved value is computed and recorded, consistent with
> resolved-value recording), and **mapping-required** (developer-supplied
> transform). Required-field addition is always transforming. Each space's database carries its shape
> version — the same diff engine replays per target on connect (BYO-DB).

**The write barrier is settled separately**, in `implementation.md`
(line 313), marked *[settled — X-071 / Z-011 / C4-008, open across three
review rounds]*:

> So a transforming migration **quiesces writes to the entity being
> transformed, from scan start through verified swap.** Reads are unaffected
> throughout; the source column stays authoritative and every op still serves
> the old shape. Only writes to that one entity block, and they block with a
> typed, retryable rejection naming the migration, not a hang.

With the disclosure obligation attached:

> `ordo plan` states the freeze, names the entity, and estimates the duration
> from live row count and measured chunk throughput before anything runs; an
> operator who finds the number unacceptable declines the plan.

And the upgrade path explicitly deferred:

> **The upgrade path, named so it is a decision rather than a discovery:**
> dual-write from a watermark … It is deliberately **not** in v1: it adds a
> second write path, a watermark protocol and a drain-completeness proof to a
> subsystem whose failure mode is silent data loss.

**So what the barrier bars, and when:** *writes to one entity*, from *scan
start through verified swap*, deployment-wide (see §6). Not reads. Not
writes to other entities. Not the destructive deploy that follows the soak,
which needs no barrier because nothing is being copied.

The remaining settled structure is `implementation.md`'s **three-deploy
invariant**, marked `[proposed]` at the section head but load-bearing
everywhere else:

> a deploy never destroys the readable shape in the same deploy that
> introduces the new one. Additive → backfill → verify → swap → soak →
> destructive is **three deploys**, and only the third is unrecoverable
> without a restore.

### 2.1 Where §9 is silent — and what this plan proposes

§9 says nothing about, and this document proposes rather than assumes:

| Silence | Proposal | §|
|---|---|---|
| Constraint changes (`unique`, `index`, `check`, `enum`) — §9's three classes name fields and entities, never constraints | A fourth axis: constraints classify by whether the live data satisfies them, decided by a probe, not by the declaration | §5 |
| Adding `sealed` to a plaintext field, and removing it | Sealing is transforming-through-the-worker; **unsealing is a refusal in v1** — it moves data permanently into plaintext columns and backups, and no re-seal undoes that | §5 D6 |
| Adding `indexed` to an existing sealed field | Transforming, and the hardest one in the taxonomy — see §7.3 | §7.3 |
| What "the apply itself is a command" means, given `CommandKind` is a closed three-variant enum with per-row images | Split it: DDL/activation is a **deployment-scoped security-stream event**; the backfill, which writes rows, is command-log traffic | §7.1 |
| What `revert` means for a schema change | Nothing. Reverting a schema change is a new migration. Backfill groups are marked non-revertible | §7.2 |
| Whether a migration is per-space | It cannot be, except under BYO-DB. Stated loudly rather than discovered | §6 |
| `check` invariants | DL §3 settles `check` including ref-walking; `ordo-schema::Field` has no `check` and `ddl_for` emits none. Out of scope here — when `check` is built, it joins class C2 | — |

---

## 3 · The shape: expand-contract, in place

**Recommendation: in-place `ALTER` following expand-contract, with the
"green" at column grain rather than schema grain.** This is what §9 and
`implementation.md` already describe; the contribution here is the argument
for it over blue/green, and the cost of the alternative.

### 3.1 Why not blue/green

Blue/green — copy into a new Postgres schema, cut over, drop the old — is
attractive because rollback is `DROP SCHEMA` and there are no partial
states in the serving schema. The space model does **not** make it simpler.
It makes it structurally unavailable, for four reasons, in descending
severity:

**1. The audit chain forks.** `$commandlog` is per-space and hash-chained:
`hash = SHA-256(prev ‖ canonical-encoding(group))`, one row per group,
appended under a per-space advisory lock (`commandlog.rs:87`,
`APPEND_LOCK_NS`). A green schema either shares the chain with blue — in
which case both schemas are appending to one strictly-ordered chain, and
"which schema was this group written against?" has no answer in the
encoding — or copies it, which produces two chains sharing a head and
diverging afterwards. Copying an append-only tamper-evident log is the one
operation it exists to make impossible. `implementation.md` already treats
a *restore* rewinding the chain as serious enough to need a
`system:restore` discontinuity marker; a routine deploy must not need one.

**2. The tables are shared across every space, so a copy is a
whole-deployment copy.** There is no per-space table and no space
directory. A blue/green migration therefore cannot cut a tenant over at a
time — it copies every space's rows, doubles the deployment's storage for
the duration, and cuts everyone over at one instant. The "copy one space,
see if it's fine" instinct that makes blue/green attractive in a
tenant-per-database system is not spellable here.

**3. Sealed payloads are not copyable in isolation.** `$sealedpayload` is
keyed `("$space", "ref")` and reached by opaque random handles held in
command-log images (`crates/ordo-crypto/src/sealed.rs:95`). A green schema
must either share that table — so green is not an independent database —
or copy it, which duplicates ciphertext under live epoch keys and makes
`Resolved::Pruned` mean two different things in two places.

**4. It does not solve Z-011, it relocates it.** Writes landing in blue
during the copy are exactly the "behind the cursor" problem the freeze
exists for. Blue/green needs the same change-capture or the same barrier,
so it buys no correctness — only a cleaner rollback — at the price of the
three above.

Two smaller costs, for completeness: `live_drift` compares the shadow
against the literal schema `"public"` (`crates/ordo-host/src/shape.rs:253`),
so a green schema would be invisible to the gate until the gate was
rewired — modifying the one mechanism that is currently doing its job. And
blue/green has no story under BYO-DB, where the target is a customer's
database and a second copy of their data is their disk bill.

**What blue/green would cost, stated as a number rather than a shrug:** a
chain-fork protocol (undesigned, and the closest analogue —
`system:restore` — is itself unbuilt), a space-directory table, a shared
or duplicated payload store, a rewired drift check, and 2× storage. It buys
an unambiguous rollback that expand-contract already gets more cheaply: the
old column is still there through the soak window, and rollback is a
registry re-point.

### 3.2 What expand-contract is, here

Per `implementation.md`, three deploys:

```
deploy 1  additive DDL (new column, nullable or constant-defaulted)
          + store the new artifact with --no-activate
          [old artifact still serving; new column is "extra live" → not drift]
   ↓
   backfill under the write barrier, chunk-checkpointed
   ↓
   verify (count + per-entity content hash)
   ↓
deploy 2  activate the new artifact  ← the swap; rollback = re-point backwards
   ↓
   soak
   ↓
deploy 3  destructive DDL (drop the old column) — its own confirmation
```

The "green" is a column, not a schema, and the swap is the artifact
registry's existing atomic re-point. Nothing new is invented for the
cutover.

---

## 4 · How a migration passes the shape gate legitimately

The gate must not be weakened, and it does not need to be. Trace each
deploy above through `verify_candidate`
(`crates/ordo-host/src/deploy.rs:300`):

- **Deploy 1, additive DDL applied, old artifact reloading.** The shadow is
  built from the *old* artifact's DDL. Every column it declares still
  exists live with the same type and nullability. The new column is extra,
  and extra live columns are not drift. **Passes.** ✅ Already true today.
- **Deploy 1, new artifact stored with `--no-activate`.** Nothing loads it.
  `deploying.md` §7 already documents this as the staged case.
- **During the backfill.** Nothing changed for either artifact. **Passes.**
- **Deploy 2, new artifact activated.** Its shadow includes the new column;
  live has it, with matching type and nullability (which is why the
  additive DDL must create it in its *final* shape wherever possible).
  **Passes.**
- **Rollback to the old artifact during the soak.** Old shadow ⊆ live.
  **Passes.**
- **Deploy 3, old column dropped.** The old artifact's shadow now declares
  a column live does not have → drift → `LoadRefusal::SchemaDrift`, which
  is exactly correct: after the drop, rolling back is a restore, and the
  gate says so by name. **Refuses, correctly.**

Three things must be **added** to the gate, none of them a relaxation:

1. **Constraint drift** (defect (b), §1.1). Extend `live_drift` to read
   `information_schema.table_constraints` / `pg_index` and compare
   uniques, checks, FKs, indexes and `NULLS NOT DISTINCT`, in the same
   one-way direction: every constraint the artifact asserts must exist
   live; extra live constraints are not drift (a not-yet-dropped one is
   the expand-contract state again).
2. **Migration-state refusal.** `verify_candidate` must refuse to activate
   an artifact whose migration is recorded in `$migrations` as
   `backfilling` or `failed` rather than `verified`. Today nothing knows a
   migration is in flight; the swap is precisely the moment that matters.
3. **The asymmetry of relaxations.** `live_drift` compares nullability,
   so `ALTER COLUMN DROP NOT NULL` — making a required field optional —
   makes the *currently serving* artifact drift on its next load, in the
   direction the one-way rule does not cover. Relaxations are therefore
   **not** additive: they must be sequenced as deploy-artifact-first, then
   DDL, which is the reverse of every other class. This is a real trap and
   `ordo plan` must print it.

---

## 5 · The change taxonomy

The core of this plan. One rule per class, decided against `ddl_for`'s
actual output. Classes: **A** auto-applies · **B** backfill (declared
mapping, barrier, verify) · **C** probe-then-constrain · **D** destructive
or refused.

### A — additive, auto-applies, no barrier

| # | Change | DDL | Notes |
|---|---|---|---|
| A1 | New entity | `CREATE TABLE` + its FKs, uniques, indexes | Extra live table is not drift. Free. |
| A2 | New optional field (`?` / `= null`) | `ADD COLUMN … NULL` | §9's *metadata-only*. Free. **Kills grants** on any op whose scope now includes it — see A-note. |
| A3 | New field with a **constant literal** default | `ADD COLUMN … NOT NULL DEFAULT <lit>` | §9 calls this *constant backfill — additive, bulk write*. **[proposed] correction:** Postgres ≥11 stores the missing value in `pg_attribute.atthasmissing`, so this is metadata, not a bulk write, and needs no job, no barrier, and no chunk cursor. Class A, not B. |
| A4 | New `index (a, b)` | `CREATE INDEX CONCURRENTLY` | `ddl.rs:396` states it constrains nothing. Non-transactional: the `implementation.md` invalid-index failure window applies. |
| A5 | New op, changed op body, changed guard | none | Artifact-only. Grant-killing. |
| A6 | New `acl (…)` on an entity | `CREATE TABLE` for the derived link entity + its composite unique | A1 in disguise. |

**A-note — the grant consequence, which is not a data question.** A2 and
A5 move an op's manifest (`scope` is field-grain,
`crates/ordo-plan/src/artifact.rs:157`), so every capability grant pinning
that combined hash dies. `ordo plan` must count them per op *before* the
apply: "this deploy revokes 412 grants over 3 ops." A field that no op
projects moves no manifest and kills nothing — that distinction is worth
printing, because it is the difference between a free deploy and a
re-consent campaign.

### B — transforming: declared mapping, write barrier, backfill, verify

| # | Change | Shape | Notes |
|---|---|---|---|
| B1 | New required field with an **execution-time** default (`= today`, `= newid`) | add nullable → backfill resolved values → `SET NOT NULL` | §9: *"each existing row's resolved value is computed and recorded"* — "recorded" is the audit-spine problem of §7.1. |
| B2 | New required field, no default | mapping required | Refuse the plan if no mapping is declared. |
| B3 | Type change | new column → transform → swap → drop | Never `ALTER COLUMN … TYPE USING`: it rewrites in place, breaking the old artifact instantly and giving no rollback. |
| B4 | Rename | new column → copy → swap → drop | Deliberately **not** `ALTER … RENAME`, for the same reason: a rename is atomic and leaves the serving artifact drifted the instant it lands. |
| B5 | `optional → required` | backfill nulls → `SET NOT NULL` | Probe first: a null count with no mapping is a refusal. |
| B6 | `required → optional` | `DROP NOT NULL` | Data-free, but **breaks rollback** — see §4.3. Sequenced artifact-first. |
| B7 | Plaintext → `sealed` | new `bytea` column → worker seals every row → swap → drop | §9: *"never SQL."* Per-space key lease; see §6. |
| B8 | `sealed` → `sealed indexed` | new `<f>$bidx bytea NOT NULL` computed from plaintext for **every row in every space** | The hardest change in the system. §7.3. |
| B9 | Entity becomes a `tree` | `ADD COLUMN "$path" text NOT NULL`, computed by walking the ref | Also makes every prior group on that entity unrevertable: `undo.rs` refuses `unrevertable_tree` outright. Disclose. |
| B10 | Add `normalize(lower)` to an indexed sealed field | recompute every `$bidx` | B8's cost with a collision risk on top (C4). |
| B11 | A stored `money(p)` or `decimal(p)` field changes places (25aa) | refused until stated with `--places <entity>.<field>=rescale\|keep`. Money rescale multiplies or divides by 10^n; a decimal's column is altered to the new scale. The change and a `$places` row commit together | Every `money(p)` is `bigint`, so the column type cannot show it. Found from the version the data was written under, or `$places`. A division or narrowing that would cut digits is refused, naming the rows. A sealed field cannot be rescaled in SQL. Since 2026-10-06 (25ad) `$places` also records `from_places`, `money` and the chain heads, and a revert past a money rescale converts the amount (see `howto-values.md`). |

### C — constraints: probe, then constrain

The rule for all of C: **the declaration is not the question; the live data
is.** Each gets a probe on the `nfc.rs` model — read-only, run at `ordo
plan` time, reported with counts and offending values, and a non-empty
result is a **refusal, not a warning**, because which row survives is the
deployment's decision.

| # | Change | Probe | Then |
|---|---|---|---|
| C1 | Add single-column `unique` | duplicate values; **and the null count**, because `ddl.rs:478` emits `NULLS NOT DISTINCT`, so at most **one** null row is admissible | `ADD CONSTRAINT … UNIQUE NULLS NOT DISTINCT` |
| C2 | Add composite `unique (a, b)` | same, over the tuple | as C1 |
| C3 | Add `enum(…)` pin to a `text` field | values outside the set | `ADD CONSTRAINT … CHECK … NOT VALID` then `VALIDATE CONSTRAINT` (online) |
| C4 | Any transform on a column carrying `unique` | **collisions**: does the transform map two distinct live values to one? | Refuse if non-empty. `nfc.rs::collisions` is this function, generalised from NFC to an arbitrary declared mapping. |
| C5 | Add a required ref field | orphan/dangling targets | `ADD CONSTRAINT … FOREIGN KEY … NOT VALID` then `VALIDATE` |
| C6 | Drop `unique` | none | Cheap, and a **one-way door**: duplicates accumulate immediately, so re-adding it later is a C1 that will usually fail. Print that. |

**The null rule in C1 is Ordo-specific and will surprise people.** DL's
`null == null` is TRUE, so a nullable unique field admits at most one null
row, and `ddl_for` emits `NULLS NOT DISTINCT` to enforce it. A table with
500 nulls in a column cannot take a `unique` that every other SQL system
would accept. The probe must say so in those words.

### D — destructive and refused

| # | Change | Ruling |
|---|---|---|
| D1 | Drop a field | Destructive; own deploy; own confirmation. **Truncates the undo horizon for that entity**: `undo.rs`'s `unrevertable_image` refuses any revert whose logged image names a field the schema no longer declares. `ordo plan` must state how many retained groups become unrevertable. |
| D2 | Drop an entity | D1 plus FK refusals from referrers. |
| D3 | Drop a field an op in the **currently active** artifact reads | **Refuse.** Deploy-order violation; the gate would catch it as drift, but catching it at plan time is the point of a plan. |
| D4 | Change an entity's typed-id `key` prefix | **Refuse, permanently.** `Entity::key` is *"a schema fact, fixed here so every log line ever emitted stays self-describing"* (`crates/ordo-schema/src/lib.rs:148`). Minted ids carry it and ref columns store them as text. |
| D5 | `sealed indexed unique` onto a populated plaintext field in one deploy | **Refuse.** It is B7 + B8 + C1, three barriers and two probes; it must be authored as three deploys. |
| D6 | `sealed` → plaintext (unseal) | **Refuse in v1 [proposed].** It moves values permanently into plaintext columns, WAL and backups, and re-sealing does not undo that. If it is ever allowed it needs its own consent surface, not a migration class. |
| D7 | Any C4 collision with a declared survivor rule | **Refuse in v1 [proposed].** The mapping file could name a survivor; v1 should not, because a silent merge under a `unique` is a data-loss path with a plausible-looking plan. |

---

## 6 · Every space at once

**There is no per-space migration, and the plan must say so on the first
page of its output.**

One set of tables holds every space's rows. A transforming migration
freezes writes to one entity — for every tenant simultaneously. A 40-minute
backfill of `transactions` is a 40-minute write outage on `transactions`
for every space in the deployment. `implementation.md`'s *"quiesces writes
to the entity being transformed"* is silent on tenancy; this is what it
means here.

Consequences to build for:

- **Chunk on `($space, id)`**, lexicographically. The primary key is
  `($space, id)` (`ddl.rs:262`), so this is an index-ordered scan and the
  cursor is an exact resume point.
- **Sealed backfills iterate spaces, under one freeze.** Sealing needs a
  key lease per space (`KeyProvider` derives per space). So B7/B8 take one
  lease at a time, ordered by space, while the entity stays frozen for all
  of them. A key release per space per migration is a burst on the
  key-release rate alert — expected, and it must be attributed
  (`Purpose::Reseal` exists; a migration purpose is needed).
- **The freeze is per-entity, the log lock is per-space.** A command group
  is one transaction and may touch several entities. A group touching a
  frozen entity is refused **whole**, not partially; it cannot commit its
  unfrozen half.
- **BYO-DB is the only per-space granularity that exists.** §9's *"Each
  space's database carries its shape version — the same diff engine replays
  per target on connect"* applies only there. Nothing implements shape
  versions; the plan defers them to increment 6.
- **`ordo plan` must print the freeze estimate before anything runs**, from
  the live row count and measured chunk throughput, per §9's disclosure
  obligation — and it should print the space count beside it, because the
  operator's real question is how many tenants are affected.

---

## 7 · The audit spine

### 7.1 Is a schema change a command-log entry?

§9 says two things: *"the apply itself is a command with an actor"* and
*"every apply lands in the command log."* Taken literally, neither is
implementable, and the reason is structural rather than incidental.

`CommandKind` is a closed three-variant enum — `Create`, `Converge`,
`Delete` — and a `Command` is per-row: it carries `entity`, `row_id`, and
before/after `Image`s whose shape is an invariant of the kind, validated at
the append gate (`crates/ordo-commandlog/src/command.rs:100`). The chain is
**per-space**; a `CommandGroup` is single-space by construction. A schema
change is neither per-row nor per-space. Encoding one would mean either a
fourth `CommandKind` — which changes the canonical encoding and therefore
every chain hash ever computed, a non-starter — or one group per space per
deploy, which makes "the apply is a command" mean N commands and puts a
row-shaped record with no row in every tenant's history.

**[proposed] — split the claim along the line the data already draws:**

- **The DDL and the activation** are deployment-scoped facts. They go to
  the **security-event stream**, which already has a `Scope::Deployment`
  for *"the platform's own custody and key-release events, which belong to
  the operator and not to any tenant"*
  (`crates/ordo-secevent/src/event.rs:36`), and which is independently
  hash-chained and checkpointed. Add one `EventKind` variant —
  `SchemaMigration { migration, phase }` — plus a `$migrations` table
  holding the durable state machine (`planned → applying → backfilling →
  verified → swapped → contracted`, with the chunk cursor and the probe
  results). `$activations` continues to record the re-point.
- **The backfill** writes rows, so it *is* command-log traffic, per space,
  actor `system:migration:<name>` — a legal `Actor::System`
  (`command.rs:42`). One group per space per chunk, `Converge` kind, real
  before/after images.

That satisfies §9's intent — the part of an apply that changes data lands
in the command log, attributed — without inventing a command kind. It is
**[proposed]** because §9's literal words say the apply itself is the
command, and this says the apply is a deployment event and its data effect
is the commands.

**The volume problem is real and is §9's open flank.** A B1 backfill of
`= newid` over 10M rows in 400 spaces writes 10M `Converge` commands with
full before/after images. `migration.md` §3 already solved the analogous
problem for *import* — genesis batch groups carrying a count, a minted-id
set and a content hash instead of per-row images — but that form is settled
for creates, and a backfill is all converges. See §9.

### 7.2 What does `revert` mean for a schema change?

**Nothing, and it must say so rather than half-work.**

- **A schema change is not revertible.** `implementation.md` is explicit:
  *"Transforming migrations are resumable forward, not reversible in flight
  — the reverse of a half-applied transform is a new transforming
  migration, authored and planned like any other."* What is reversible is
  the *swap*, and that is the registry re-point that already exists.
- **Backfill groups are marked non-revertible [proposed].** Reverting one
  chunk of a completed transform writes pre-transform values into a column
  whose meaning has moved on, and does it for an arbitrary slice of rows.
  `ordo revert --group` must refuse a group whose actor is
  `system:migration:*` with a typed reason naming the migration. This is a
  new refusal class beside the existing `unrevertable_image` /
  `unrevertable_sealed` / `unrevertable_tree` family in
  `crates/ordo-exec/src/undo.rs:955`.
- **A drop truncates the undo horizon, silently, today.** `undo.rs` already
  refuses `unrevertable_image` when a logged image names a field the schema
  no longer declares. So D1 makes every retained group touching that entity
  unrevertable at the instant the column drops. The mechanism is right; the
  *disclosure* is missing. `ordo plan` must count the affected groups
  within the undo horizon and print the number, and the destructive
  confirmation must name it.

**Update 2026-10-06 (25ad): a revert past a money rescale converts the amount.**
A group from before a `rescale` of a `money(p)` field is restored in the
field's current minor units, by the net factor of the rescales since. `$places`
records `from_places`, `money` and the chain heads for this. A `keep` converts
nothing, and a decimal has nothing to convert. A conversion that would lose
digits is refused (`places_revert_loses_digits`) unless the app developer allows
half-even rounding. The default is to refuse. See `howto-values.md`.

### 7.3 Sealed data under an epoch when a sealed field's shape changes

`undo.rs`'s revert applier already settles the rule for restoring a sealed
value, and a backfill should follow it verbatim: open the payload under
whatever epoch it names, and write back **a fresh seal of the plaintext
under the current epoch** — never the old envelope, because that *"would
quietly re-introduce a retired epoch into a live row and undo part of a
rotation that a revocation forced"* (`undo.rs:1584` onward). A sealed
backfill is therefore also a re-seal, which is a benefit: it advances rows
off retired epochs.

Three consequences:

1. **A destroyed epoch is a refusal, per row.** `sealed::Resolved` has
   three answers, not two — `Payload`, `Pruned`, `Destroyed(epoch)`
   (`crates/ordo-exec/src/sealed.rs:37`). A row whose envelope will not
   open under a destroyed epoch cannot be transformed. The probe must count
   them before the plan is offered; writing null in their place would be
   the platform deciding to lose data.
2. **History is untouched, and that is correct.** Logged `SealedRef`
   handles keep pointing at payloads of the *pre-transform* value. That is
   what history is for. A migration must not rewrite the payload store.
3. **B8 — adding `indexed` to an existing sealed field — is the hard
   one.** `<f>$bidx` is `bytea NOT NULL` unconditionally, including for
   nulls, which HMAC the reserved null sentinel (`ddl.rs:188`). So the
   column cannot be added with a default and cannot be added nullable in
   its final shape. Every row in every space must be opened, its plaintext
   HMACed under that space's blind-index key, and the digest written —
   which means a key release for every space in the deployment, inside one
   freeze window. And if the field is also `unique`, the blind index is
   where the constraint lives, so C1's duplicate probe has to run **over
   digests the probe must itself compute**, i.e. the probe is as expensive
   as the migration. There is no cheap version of this. Say so in `ordo
   plan` and let the operator decide.

---

## 8 · Build order

### Increment 1 — the plan, the probes, and the two defects (Monday)

Read-only against a live database. Weakens nothing. Produces the artifact
every later increment consumes.

1. **Fix defect (a)**: hash `ddl_for`'s output into `program_version`
   (`crates/ordo-plan/src/artifact.rs:140`). One-line change plus a golden
   move; a test that adds `index (a, b)` and asserts the version moves.
2. **Fix defect (b)**: extend `Pool::live_drift` to constraints and indexes
   (`crates/ordo-exec/src/shape.rs:154`), one-way, same as columns. Add
   `Drift` variants for constraint presence and for `NULLS NOT DISTINCT`.
3. **`ordo_schema::diff`** — a new module: `diff(live: &LiveShape, declared:
   &Schema) -> Vec<Change>`, with `Change` a closed enum mirroring §5 and a
   `class()` method returning `A | B | C | D`. It diffs `Schema` values, not
   artifact JSON, because the artifact omits defaults, `normalize`,
   `enum_values`, `unique_keys` and `indexes` (§1.1a). `LiveShape` is read
   from `information_schema` by the same query `columns_of` already runs.
4. **`ordo_exec::migrate::probe`** — the `nfc.rs` pattern generalised:
   `duplicates`, `nulls`, `out_of_enum`, `dangling_refs`, `collisions`
   (arbitrary declared mapping), `undecryptable` (destroyed/pruned epochs),
   `rows` (for the freeze estimate). Read-only, one round trip per
   question, no writes, ever.
5. **`ordo plan --against <db>`** — renders the classified change list, each
   with its probe result, the freeze estimate and space count for every
   class-B change, the grant-kill count per op, and for a D1 the number of
   groups that become unrevertable. Refuses to print an appliable plan when
   any probe is non-empty.

**Done when** an operator can point `ordo plan --against` at a live
database with a changed `.ordo` and get a truthful, classified, probed
report — and when adding `index (a, b)` is no longer a silent no-op.

### 8.1 Increment 1 — **landed**

Built: `crates/ordo-plan/src/artifact.rs` (defect (a)),
`crates/ordo-exec/src/shape.rs` (defect (b)),
`crates/ordo-schema/src/diff.rs` (the classifier),
`crates/ordo-exec/src/migrate.rs` (the probes),
`crates/ordo-cli/src/migrate.rs` (`ordo plan --against`). Proved by
`crates/ordo-cli/tests/schema_identity.rs`,
`crates/ordo-exec/tests/migrate.rs` and
`crates/ordo-cli/tests/plan_against.rs`.

**Defect (a) is closed twice over, on purpose.** `entity_artifact` now
emits `default`, `normalize`, `enum_values`, `unique_keys` and `indexes`,
which makes the change *visible* in a deploy diff; and `program_version`
hashes the document **and** the generated DDL, NUL-separated, which makes
identity *total* against the next omission. `registry::store` compares
both halves, so a differing `ddl` under an identical `artifact` is a
`VersionCollision` rather than a silent `AlreadyStored`.

**A correction §1.1(a) owes.** It names `unique (a, b)` as the worse
example, on the grounds that the artifact bytes were byte-identical
across it. They were not. A composite unique key is what
`ordo_plan::writes::set_mode` reads to decide that an unconditioned `set`
**addresses** a row, so declaring one derives `std.<entity>.put`, which
the entity did not have before — the document moved. A single-column
`unique` moved it too, because `Field::unique` was always emitted. The
genuinely silent cases were `index (a, b)`, which constrains nothing and
derives nothing; a changed literal default; and `normalize(lower)`, which
emits **no SQL at all** and therefore had the document as its only
possible home. The consequence §1.1(a) draws is right and one of its two
examples was not.

**Defect (b) is closed, and it changed §4.3's scope.** `live_drift` now
reads `pg_constraint` and `pg_index` beside the columns and compares
`pg_get_constraintdef` / `pg_get_indexdef` text, which is what makes
`NULLS NOT DISTINCT` visible without a special case. The direction is
unchanged: extra live constraints are not drift.

The consequence is larger than §4.3 states. §4.3 names
`ALTER COLUMN DROP NOT NULL` as the one relaxation that breaks the
*currently serving* artifact on its next load. Now that the gate sees
constraints, **every constraint or index drop has the same shape**:
dropping a `unique`, an `enum` check, a foreign key or an index that the
serving artifact still declares makes it drift the moment the DDL lands.
So §5's `C6`, and the drop codes below, are artifact-first sequencing
traps in exactly §4.3's sense, and `ordo plan` should say so. Before this
increment they were invisible and therefore harmless; the fix that makes
them visible is also what makes them sequenced.

**Taxonomy codes minted here, because §5's table is written in the
forward direction and a diff runs both ways.** They are listed so no
second spelling of any of them can arise:

| Code | Change | Class | Note |
|---|---|---|---|
| `A7` | Drop a declared `index` | A | Data-free and re-addable, unlike `C6`. Artifact-first (above). |
| `C7` | Drop a composite `unique` | C | `C6` over a tuple; the same one-way door. |
| `C8` | Drop an `enum` pin | C | Values outside the old set accumulate immediately. |
| `D8` | `sealed indexed` → `sealed` | D | Drops the `$bidx` column, which is the whole blind index. |
| `D9` | An entity stops being a `tree` | D | Drops `$path`. |
| `D10` | Drop a foreign key | D | A ref field that stops being a ref. |

**Refusal classes `ordo plan --against` raises**, none of them a wire
`ErrorCode` and none of them an author-facing compile class, so neither
inventory census owns them; recorded here instead:
`unseal_refused` (§5 D6), `key_change_refused` (§5 D4),
`drop_read_by_active_op` (§5 D3), `probe_not_empty` (§5's rule for all of
C), `probe_unavailable` (the database could not answer), and
`sealed_epochs_unverified` (§7.3 — this command holds no key custody, so
it cannot rule on epoch liveness and says so rather than guessing).

**What increment 1 does not do, named rather than left to discovery.**

- **§5 D5** (`sealed indexed unique` onto a populated plaintext field in
  one deploy) is refused in practice, because the sealed probe always
  answers `sealed_epochs_unverified` — but it is not refused *by name*.
  It should be its own ruling when increment 4 gives the sealed path a
  probe that can come back clean.
- **§5 B4** (rename) and **§5 C4/D7** over a declared mapping need a
  mapping file, which does not exist. A rename reaches the diff as a drop
  and an add and stays that way.
- **§5 A5/A6** are not schema-shape changes. A5 is counted under the
  report's GRANTS section; A6 arrives as A1.
- **The freeze estimate is a floor**, measured from a read-only chunk
  scan that pays for neither the write, the WAL, nor the command-log
  append. The report says so in those words; increment 3 replaces it with
  the backfill's own measured rate.
- **`$migrations` does not exist**, so §4's migration-state refusal is
  not in `verify_candidate`. That is increment 3's second bullet.

### Increment 2 — additive apply

`ordo migrate --apply` executing **class A only**, refusing everything
else by name. Class A DDL in one transaction (Postgres has transactional
DDL and this design uses it), with `CREATE INDEX CONCURRENTLY` outside it
and the invalid-index state surfaced rather than inferred, per
`implementation.md`'s failure table. Each change recorded in `$migrations`
and emitted as a `SchemaMigration` security event.

This alone unblocks the common case — new entity, new optional field, new
index, new constant-defaulted field — with no barrier, no backfill, and no
audit-spine question. The deploy story is exactly today's: apply the DDL,
`ordo apply --no-activate`, then activate.

**Done when** a self-hoster can add a field to a deployed schema and the
deployment keeps serving throughout.

### 8.2 Increment 2 — **landed** (2026-09-20)

Built: `crates/ordo-host/src/schema_apply.rs` (the plan as data, the
probes moved out of the CLI, `schema_state`, `create_schema`, `triage`,
`execute`, `record`, and the composed `apply`),
`crates/ordo-exec/src/migrate.rs` (`$migrations`, `check_violations`,
`index_valid`), `crates/ordo-secevent` (`EventKind::SchemaMigration`),
`crates/ordo-schema/src/ddl.rs` (per-entity emitters, so a column added
to a live table and a column created with its table have one spelling),
`crates/ordo-schema/src/diff.rs` (`C9`/`C10` for `check`,
`Change::gate_blocking`), and the two thin callers in
`crates/ordo-cli/src/{migrate,operator}.rs`. Proved by
`crates/ordo-cli/tests/schema_apply.rs` and the D2 case of
`crates/ordo-host/tests/dev_loop.rs`.

**The apply belongs to the service.** Owner constraint, 2026-09-20. The
library takes a `Pool` and a compiled `Program` and returns structured
outcomes; the CLI renders. A hosted control plane (checklist 18l) adds
the door in front of the same functions. §8's "the deploy story is
exactly today's: apply the DDL, `ordo apply --no-activate`, then
activate" holds, and `ordo apply` now enforces its order: on a database
the gate would refuse it stores and does not activate, naming `ordo
migrate --apply`.

**The gap this plan did not name: nothing created the schema.** `ordo
apply` computed `ddl_for` only to hash it into the version. A first
deployment meant pasting DDL out of `$artifacts`. Closed here rather
than in a separate increment, because the create is the degenerate
class-A apply (every entity is an `A1`) and it reuses
`spacedb::apply_schema`, the path `provision::prepare_target` already
ran for a space's own database. A registry that names a serving version
over no tables is refused as `serving_schema_drifted`, never created
over.

**One rule beyond "class A only", taken and recorded.** A class-C
constraint is applied when it constrains no existing data: the table is
empty, or every column it names arrives in this same apply and its probe
is clean by construction. Those constraints are the column's own
declaration; a column that arrived without its foreign key would be a
column the artifact asserts and the gate refuses on the next load. A
constraint over existing rows was refused as
`constraint_over_existing_rows`, with the `ADD CONSTRAINT … NOT VALID`
then `VALIDATE CONSTRAINT` statement carried for a person who had read
the probe. **Since 2026-10-04 (27d) the apply runs that form itself:** a
foreign key, an enum pin or a check onto a table that exists is added
`NOT VALID` in the transaction and validated as its own step after it
(`plan-evolution-plans.md` §3.7). A `unique` has no online form yet and
over rows is still refused with its statement carried plain.

**Drops are refused whole, including `A7`.** `A7` was minted class A as
data-free and re-addable, and it still sequences artifact-first, so an
apply that ran it before the artifact was activated would make the
serving artifact drift. It is refused as `artifact_first` with the
`DROP INDEX CONCURRENTLY` statement carried. What this exposed: once the
artifact that stops declaring the index **is** activated, `prior` and
`declared` agree and the live index is invisible to the diff, so it is
never proposed again. Increment 5's contract phase needs the
*previously* active version as its prior, which `$activations` holds.
Named here so it is a decision when increment 5 starts.

**Refusal classes the apply raises**, beside §8.1's: `plan_refused`
(anything the plan refused), `serving_schema_drifted` (the database
lacks something the serving version declares, or the registry names a
version over no tables), `not_class_a`, `artifact_first`,
`blind_index_backfill_required` (a `sealed indexed` field onto a
populated table: the `$bidx` column is `NOT NULL` with no default),
`constraint_over_existing_rows`. When any change is refused, nothing is
sent: an apply that landed half its changes would leave a database that
matches no version.

**`$migrations`'s phases** are `applied`, `failed` (the transaction
rolled back, `note` carries the error), `index_invalid` (the
transaction committed and a concurrent index did not become valid;
`note` names the `DROP INDEX CONCURRENTLY` to run) and `validate_failed`
(the transaction committed with a constraint added `NOT VALID`, and its
validation found rows that break it or never got its lock; the
constraint was dropped again and `note` names it). The row is written
on its own connection so a failed apply still leaves a row saying so.
Increment 3 grows the same table into the state machine of §7.1.

### 8.3 25i, a program update reaches every tenant's own database: **landed** (2026-09-29)

**What was seen.** The notes program lost its `gone` field. `serve.sh`
applied the new version, the deployment's database moved, and the owner's
tenant database stayed at the old version. The tenant was held closed as
schema-behind and the person was told the database "cannot be reached". With
one tenant per person every update would have shut out every person. The
checklist item was "migrations reaching every attached database"
(tenancy-topology), and the requirement is R-S3 of `plan-space-routing.md`.

**What an apply does for a tenant's database now.** One function,
`schema_apply::bring_forward`, brings one database to the serving version. It
runs the same plan and triage as the deployment's own apply (`plan`,
`triage`, `execute`, `record`), so a class-A change lands and anything else
is refused by name with nothing sent. It is called from three places:

| Where | When |
|---|---|
| `apply_op::sequence`, through `apply_everywhere_except_default` | An apply through the door (`std.deployment.apply`, `ordo apply --host`), after the deployment's own database has accepted the version and before the activation. A staged apply (`--no-activate`) touches no tenant. |
| `tenant_forward::CatchUp::sweep` | Once at boot, and on every replica each time the node swaps to a new version (`spawn_deploy_listener` nudges it). |
| `tenant_forward::CatchUp::forward_space`, on the `tenant.catchup` worker's tick | Every 15 seconds (`ORDO_TENANT_CATCHUP_SECS`), for each space the resolver is holding closed as behind (`Spaces::awaiting_schema`). |

`ordo apply --db` still touches the one database it names, so a program
applied that way reaches the tenants through the second and third rows.
That is the path `serve.sh` takes.

**How a tenant that missed an apply catches up.** The smallest mechanism the
existing reconciler supports is its tick. `Spaces` already holds each space
it closes for a schema comparison and `Spaces::reconcile` already lifts the
hold when the comparison passes. The catch-up adds the one act between them,
`bring_forward`, under the same `ApplyLock` an apply and an attach take (R-S5),
one database at a time so an apply in between waits for one database and not
for all. It is not leader-only: each replica holds its own register, one
replica moves the database and the others find it current, write nothing and
lift their own hold. A database the sweep could not reach (down, or slow past
20 seconds) makes the next tick sweep again. A tenant created from the
previous version a moment before a swap is behind the new one and is found
the same way.

**One database at a time, bounded.** The door's walk works on 8 databases at
once and gives each 20 seconds (`schema_apply::FORWARD_CONCURRENCY`,
`FORWARD_TIMEOUT`). A database that does not answer costs the walk its own
timeout and nothing else. The apply's answer carries `tenants`: how many were
brought forward, how many were already there, and a line for each that was
held with the refusal. `/health/ready` lists each closed tenant with its
reason, and the log names it (`tenant catch-up: ...`, `apply <version>: tenant
database ... stays closed`).

**A tenant's failure no longer holds back the activation.** R-S3 gated the
activation on every database taking the program. With one tenant per person
that lets one unreachable or refused person stop every deploy for everybody.
The activation is now gated on the deployment's own database, as before, and a
tenant that could not be brought forward is closed alone by the schema-behind
quarantine (R-S4, the default D-B ruled). The standalone `apply_everywhere`
keeps the old gate for its own callers.

**A refused change keeps the tenant closed with the refusal as its reason.**
The reason `/health/ready` shows is the comparison ("schema behind: ...") and
then the refusal ("Bringing it forward was refused: not_class_a: ..."). A
refusal by name is asked once per serving version, so a database is not
written to every tick to hear the same answer. A new version or a restart asks
again. A database that could not be asked (unreachable, timed out, an error
that says nothing about the change) is asked again on the next tick. The
deployment owner decides what to do with a refused tenant, for example by
changing the program.

**A dropped field.** On the deployment's own database the apply tolerates a
drop: `schema_state` reports `Matches { pending }`, the column stays,
increment 5's contract phase is not built, and the artifact activates. A
tenant is treated the same way. `bring_forward` plans without a registry (a
tenant database holds none), so a field the program stopped declaring is not
in its plan, no DDL runs for it and no column is deleted. What is new is that
the version is written down. Nothing did that before: no `$migrations` row
moves when no DDL runs, so a database that already held everything kept
saying it was at the old version. It is recorded now as a `version` row
(`migrate::kind::VERSION`), written only after `schema_state` shows the live
schema holds everything the new version declares. The deployment's own
database gets the same row (`record_version_if_behind`), for the spaces routed
to it. Nothing here deletes data, and nothing settles what increment 5 will do
with the left-over column.

**A failed row no longer counts as a version.** `recorded_version` read the
last `$migrations` row whatever its phase, so a failed apply (`phase =
failed`, `to_version` the new one) made a database look current. It skips
failed rows now.

**The sentence a caller reads.** A quarantine for schema-behind is its own
refusal (`SpaceRefusal::SchemaBehind`) and `space_refusal` gives it its own
sentence: *"space `...` is closed for a moment while its data is brought to
the current version of the app. Nothing was written and the same request will
land when that is done; if it persists, the deployment owner has the
reason."* It is `503 space_unavailable`, names no database and no version
(the reason §9.1 R-C4.4 withholds dial errors), and leaves the detail to
`/health/ready` and the log. The unreachable sentence says "the deployment
owner" where it said "the deployment's operator".

**Tests.** `crates/ordo-host/tests/tenant_forward.rs` (two tenants and an
additive change through the door, a direct apply reached by the catch-up, a
sweep with no request, a database down during the apply, a refused change,
a dropped field), `crates/ordo-host/tests/apply_everywhere.rs` (an
unreachable and a refused database do not stop the apply and are named),
`crates/ordo-host/tests/notes_app.rs` (a person signs up, the program gains a
field and loses it again, and the same person keeps working; a change one
tenant's data cannot take closes that tenant alone) and the unit test of the
sentence in `crates/ordo-host/src/lib.rs`.

### Increment 3 — the barrier and the plaintext backfill

- `$migrations` state machine with a chunk cursor, attempt count, and
  dead-letter (the `$jobs` table has none of these).
- The write barrier: primary check in `Executor::write_attempt`
  (`crates/ordo-exec/src/lib.rs`), returning a **typed retryable rejection
  naming the migration**, plus a Postgres advisory-lock backstop in the
  `APPEND_LOCK_NS` namespace so a hand-written statement or a second
  process cannot slip past the in-process check.
- Chunked backfill on `($space, id)`, class B1–B6 (plaintext only).
- Verify: count + per-entity content hash, *"the same verifier custody
  conversion uses."*
- The drill `implementation.md` demands: kill mid-chunk and assert old-shape
  reads still correct, registry untouched, job resumes, verification
  passes; **write to a row behind the cursor during the freeze and assert
  the write is rejected, not silently dropped**; assert the post-swap row
  set is byte-identical to a freeze-free run over a quiescent database.
- `verify_candidate` refuses to activate over a `backfilling` migration
  (§4.2).

### Increment 4 — sealed and blind-index backfills

B7, B8, B10, through the worker, never SQL. Per-space key lease ordered by
space under one freeze; re-seal under the current epoch; destroyed/pruned
rows refused and counted by the increment-1 probe. C1 over blind indexes.

### Increment 5 — destructive

D1, D2 with explicit confirmation, the undo-horizon disclosure, and the
BYO-DB rule that the **space owner** confirms drops while the domain
developer owns everything else.

### Deferred, deliberately

- **Dual-write from a watermark** (Z-011's named upgrade path).
  `implementation.md` defers it explicitly and gives the reason; build it
  when someone's row counts make the freeze intolerable, with a real
  workload to test against.
- **Per-space shape versions and BYO-DB replay-on-connect.** Needs a space
  directory, which does not exist.
- **`check` invariants.** DL §3 settles them; `ordo-schema` has no `check`
  at all. When it lands, it joins class C.
- **`ordo introspect` / `ordo import`.** Different problem (§0.2).

---

## 9 · The open question this plan does not close

**[open] What does a transforming backfill write into the audit spine, and
can the chain carry it?**

Every other hard part in this document has an answer that survives
inspection, even if the answer is "refuse". This one does not.

The constraints, all verified:

- The command log is per-space, hash-chained, single-writer under a
  per-space advisory lock. A backfill of one entity touches every space, so
  it must append to every chain.
- Per-row images make the log dwarf the data. `migration.md` §3 states the
  problem for import in as many words: *"importing 10M rows as 10M per-row
  image-bearing entries is untenable."*
- Its solution — the **genesis batch group**, carrying a count, a minted-id
  set and a content hash instead of per-row images — is settled for
  **creates**: *"creates have no before, and the after is the database."*
  A backfill is all **converges**, which have a before, and the whole point
  of a converge's before image is that it is the only record of what the
  value was.
- And the summarised form's revert story does not survive contact:
  `critique-consolidated-3.md` **Z-014** (status: open) records exactly
  this defect — *"a corrected rerun converges an existing row without a
  before image; delete by 'minted ids' destroys the prior row and keeping
  it cannot restore the old value."* The same hole, reached from the import
  side.

So the three candidate answers each fail somewhere:

1. **Full per-row images.** Correct, revertible, and produces a log
   larger than the data it describes for every large transform.
2. **Summarised batch groups.** Cheap, tamper-evident, and records
   nothing about what the values *were* — which for a converge is the
   information the audit spine exists to hold. Inherits Z-014.
3. **A pre-transform snapshot outside the chain**, with the chain carrying
   only a hash of it. Restores revertibility, and invents a second durable
   record with its own retention, pruning and erasure story — the thing
   this platform has been careful not to have.

§7.1 proposes (1) for now, because it is the only one that is *correct*,
and it makes the honest disclosure part of `ordo plan`: this migration
will add N entries to M chains. But (1) does not scale, and picking (2)
means answering Z-014 first. **This is the question to settle before
increment 3 starts**, not during it — the barrier, the cursor and the
verify pass can all be built against either answer, but the group encoding
cannot be changed afterwards without moving every chain hash it wrote.
