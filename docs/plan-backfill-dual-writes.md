# The backfill worker and dual writes: how one tenant moves through a transforming step

Status: **B1, B3 and B6 built 2026-11-23** (`plan-versions.md` increment
10b; `crates/ordo-host/src/backfill.rs`). B2, B4 and B5 are not built.
What was built differs from the text below in the places marked
"Built". Written by the review
of the evolution plans (`review-evolution-plans-2026-11-23.md`). It is the
design of `plan-versions.md` increments 10b, 11a and 11b, which had one
table line each and 5 bullets in §2.4. It replaces increment 3 of
`plan-schema-evolution.md` (the write barrier).

Read this first:

- A transforming step changes data that already exists: a rename, a type
  change, a split, a new required field.
- The tenant keeps serving version X the whole time (ruled seamless, 27c).
- A worker fills the new shape in chunks and can be killed at any point.
- The host writes both shapes while the worker runs and through the soak
  (ruled 2026-10-01; in the host only, D10).
- Then the pin switches. A revert inside the soak is a pin switch back.

What is ruled and what is proposed:

- Ruled: dual writes in the host, no triggers (D10). One open soak per
  tenant (D11). No plaintext dual write for a sealing step (D9). Soak
  length 7 days, 0 allowed. Summarised audit groups by default
  (2026-09-08, 2026-09-20). Backfill functions by code hash (D16).
- Everything else below is this plan's proposal. Sections 3 and 4 are the
  research.

---

## 1 · The phases of one move

`$moves` in the control database holds one open move per tenant
(`plan-versions.md` §1.2). Its `phase` takes these values:

| Phase | The tenant serves | Writes go to | Ends when |
|---|---|---|---|
| `expanding` | X | old shape | the step's expand has run on the tenant's database |
| `backfilling` | X | old shape and new shape | every `$backfills` cursor is at its end |
| `verifying` | X | both | count and content hash match |
| `gated` | X | both | Y passes its gate on this database; `$migrations` adds Y |
| `soaking` | Y | new shape and old shape | `soak.length` passes, or a revert |
| `closed` | Y | new shape | the old columns are in `$orphans`; the row leaves `$moves` |
| `failed` | X | old shape | the deployment owner retries or skips |

- The pin switches between `gated` and `soaking`, in one transaction with
  its `$pins` row (`plan-versions.md` §2.1, step 5).
- A failure in any phase before `soaking` leaves the pin on X. Dual writes
  stop. The new columns stay; they are extra columns, which is not drift.
- A revert in `soaking` is a pin switch to X. The move goes back to
  `gated`, and both shapes are still current.
- After `closed`, a revert runs the step's reverse as a new move.

---

## 2 · The cursor: `$backfills`

One table in **each tenant's own database**, made with Ordo's other tables.

| Column | Holds |
|---|---|
| step | the stored plan's step id |
| entity | the entity being filled |
| cursor | the last `id` done, in `($space, id)` order |
| rows done, rows total | for the report |
| attempts, last error | for retry and the dead-letter state |
| state | `running`, `done`, `dead` |
| hash state | the running content hash, so verify does not start over |

Why in the tenant's database:

- A chunk and its cursor commit in one transaction. A kill between them
  is impossible.
- It travels with the tenant: a restore, an export, a database move.
- A tenant with a database of its own (BYO) needs no second connection.

A chunk:

1. Take the tenant's move lock (an advisory lock in its database).
2. `UPDATE` the next `backfill.chunk_rows` rows after the cursor: new
   columns from the step's forward mapping over the row's current old
   columns.
3. Advance the cursor and the hash state.
4. Commit. All under `backfill.chunk_timeout` and a `lock_timeout`.

- A chunk that times out is retried with half the rows. 5 failed attempts
  mark the cursor `dead`, the move `failed`, and the rollout counts it.
- The mapping is an Ordo expression from the stored plan, never SQL from a
  person (`plan-versions.md` §2.3).

**Locks.** `plan-versions.md` §2.4 says "under the apply lock". A backfill
can run for hours, and the apply lock is the deployment's. Proposed:

- The apply lock is held for `expanding` and for the pin switch only.
- The move lock, per tenant, is held per chunk.
- So a new apply is not blocked by a running backfill. It is refused by
  the one-open-rollout rule instead (`plan-versions.md` §3.7, Q3).

**Across tenants.** `backfill.concurrency` (8) moves at once per
deployment. Proposed `backfill.per_database` (2), so 8 tenants in one
shared database do not take its whole pool.

**Built (10b, 2026-11-23).**

- A step fills a field with `ordo evolve --fill <entity>.<field>=<expr>`
  (B2, a new required field; B5, an optional field made required). The
  fill is in the plan's options and its receipt, so in the digest.
- The mapping is rendered by `ordo_schema::check::render_mapping`, the
  row-local slice a `check` uses. The render types it against an empty
  database of X, so a value the column refuses is a plan error.
- The expand adds the column nullable. Its `$migrations` row names X, so
  the database records X until the backfill verifies and `NOT NULL` lands.
- `$backfills` holds the cursor, rows, attempts, last error and state.
  `$backfill_spaces` holds each space's count and running content hash.
- A chunk takes `pg_try_advisory_xact_lock`. A second worker is told
  `backfill_busy`; it does not wait. The per-tenant move lock and
  `backfill.concurrency` across tenants wait for 11b.
- A failed chunk is retried with half the rows; 5 in a row mark it
  `dead`. An explicit run again retries a dead cursor.
- Settings: `chunk_rows` 1000, `chunk_timeout` 30 s, 5 attempts, and a
  `budget` of chunks per run. They are defaults until increment 10 stores
  them in Ordo (Q15).

---

## 3 · Dual writes: the move overlay (research)

The hard part. Say 2 to 3 tries.

**What must hold.** A write that lands while the worker runs must reach
the new shape. Otherwise a row behind the cursor has a stale new column.

**Why the worker and a live write cannot disagree.** Both compute the new
columns from the row's old columns with the same forward mapping, under
the row's lock. Whichever runs second writes the same value. No ordering
between them is needed.

**The design.**

- A **move overlay** is a variant of version X's `Loaded` for one step: X's
  ops, where every write to a changed entity also sets the new columns by
  the forward mapping, in the same command group.
- After the switch the overlay is Y's `Loaded`, where every write also
  sets the old columns by the reverse mapping.
- There is one overlay per (version, step), shared by every tenant in that
  phase. Not one per tenant.
- `Deployment::for_space` already answers a tenant's `Loaded` (increment
  3). It answers the overlay while the tenant's `$moves` phase asks for
  one. The phase rides the pin stream, so every node switches together and
  the pin fence refuses a node that is behind.
- A mid-move write passes X's guards and Y's (D10). A write Y's guards
  refuse is refused, naming both versions. The pre-check finds these rows
  first (increment 9).
- The command group records one write. The audit chain sees it once.

**What has no reverse.**

- A step with no generatable reverse (a lossy cast, a merge) writes no old
  shape after the switch. It is the one-way step: confirmed before it runs
  (`plan-versions.md` §2.2), and its revert needs a backup.
- A sealing step writes no plaintext after the switch (D9).

**What a try must answer:**

1. Can the planner emit the extra `SET` from a stored mapping for every
   option on the menu (`plan-evolution-plans.md` §3.2)?
2. What does the overlay cost in memory per (version, step)?
3. Does a write that Y's guard refuses read clearly to the person who made
   it?

---

## 4 · Shared databases

Most tenants share a database, and each row carries `$space`. A step's
DDL is per table, so it is per database, not per tenant.

- **Expand** runs once per database, at its first tenant's move. Later
  tenants find it done.
- **Constraints of the new shape** (`NOT NULL`, a unique index, a check)
  cannot be added while a tenant in that database still writes as X.
  - Until the last tenant there has switched, the host enforces them on
    Y's writes.
  - When the last one switches, the constraint is added with a validate
    step that takes no long lock.
- **Contract.** A column is orphaned per tenant in `$orphans`. The column
  is dropped, when pruned, only after every tenant in that database has
  orphaned it.
- A tenant in a database of its own has none of this: its first tenant is
  its last.

This is research too: one try, on the conformance cluster with 3 tenants
in one database and 1 in its own.

---

## 5 · Verify and the audit record

**Verify** (phase `verifying`):

- Row count per entity.
- A content hash: forward mapping over the old columns equals the new
  columns, by the verifier custody conversion uses.
- A mismatch fails the move and names the first row.

**The audit record** (ruled 2026-09-08 and 2026-09-20):

- One summarised group per backfill in the tenant's command chain: the
  step, the mapping, the row count, the content hash.
- With `backfill.audit_images` on, full per-row images in place of it. The
  app developer chooses per backfill, and the group says which kind it is.
- The report says, before the move runs, which kind it will write, and
  that a summarised backfill's old values come back only from a backup.
- The chain's encoding is frozen. The new group kind is added as an
  optional trailer, and a golden test of today's bytes lands first (the
  order `plan-opslog.md` D1a used).

**Built (10b, 2026-11-23).**

- Goldens of a plain group, a group with both flag tags and a restore
  marker landed first (37bba572), then trailer `0x04` (3f931aed).
- The record: kind (summarised or images), space, step digest, entity,
  field, mapping, row count, content hash. Actor `system:backfill`.
- The content hash is SHA-256 chained per space over each row's space,
  id and new value, in `($space, id)` order
  (`ordo_commandlog::backfill_row_hash`). Chunk sizes do not change it.
- Verify: no row left null; for a new column filled by an expression,
  every row equals the mapping, naming the first row that does not; each
  space's count and hash recompute. A mismatch refuses and nothing is
  made required.
- The summaries and `done` are written in one transaction, so a run again
  never writes them twice.
- Images are refused for an entity with sealed or file fields, which need
  the write path's custody. The summary is the default there.
- The receipt says, before the step runs, which kind it writes and that a
  summarised backfill's old values come back only from a backup.

---

## 6 · Backfill functions (D16)

- A step may name a backend function by code hash, with an optional
  reverse, for a change the menu cannot express.
- The worker calls it once per chunk: rows in, new column values out.
- It runs with the limits of any function (`plan-backend-functions.md` §9).
- **Ruled 2026-11-23 (Q14):** a backfill function may use secrets and
  call out only if the backfill declares which secrets and hosts, and the
  plan shows them. Undeclared use is refused. The cases: re-encrypting
  under a new key, hashing with a pepper, geocoding, fetching files into
  storage, an AI summary. Staging's rehearsal marks such a backfill as not
  reproducible, since the same rows may not give the same values.
- Dual writes need the mapping on every live write. A function is too
  slow for the write path. So a step with a backfill function is a one-way
  step unless its forward mapping for single rows is also an expression.
  The plan says so when it is rendered.

**Built (10b, 2026-11-23).**

- `--fill <entity>.<field>=fn <name> [secrets a,b] [hosts h,i]`. The
  function must be declared in Y with stored code; the receipt records
  its code hash.
- The secrets and hosts must be within the function's own declaration,
  or the render refuses naming the line to add.
- The call gets exactly the declared lists, no ops and no files. It runs
  once per chunk per space: `{"rows": [...]}` in, `{"values": [...]}` out.
  Each call is a `$fnruns` row as `system:backfill`.
- A function with a secret or a host is marked `"reproducible": false` in
  the receipt, for staging's rehearsal (12b) to read.
- The one-way rule above is not enforced yet: it belongs to the dual
  writes of 11a.

---

## 7 · Sealed fields, blobs and tenant storage

- **Sealed backfills** are increment 13. They use this worker with a key
  lease per space, `Purpose::Migration`, and no plaintext dual write (D9).

**Built (13, 2026-11-24, first part: cce76e88, 7f231dd5).**

- A stored step that makes a `text` field `sealed` (B7) expands a `bytea`
  column `<field>$seal` beside the plaintext. `ordo apply` refuses B7 as
  `seal_needs_stored_plan`, naming `ordo evolve`.
- The worker seals each row's plaintext under its space's key, at its row
  (`Anchor::Row`), the way the write path does. One lease per space per
  chunk, `Purpose::Migration` (new in `ordo-crypto`), so the key-release
  count names the step. The plaintext column is never written.
- Verify opens every sealed value in memory and compares it with the row's
  plaintext, then each space's count and content hash. The hash is over
  the envelopes, never the plaintext.
- A verify failure clears the sealed column and the cursor, so a run again
  seals every row as it then is.
- After verify: the plaintext column is dropped, the sealed one renamed to
  the field's name, and its `NOT NULL` set when the field is required.
- The receipt shows the backfill (`mapping` `seal`, `sealFrom`) and what
  happens to the plaintext. Its old values stay in earlier backups and WAL.
- Refused by name: `sealed indexed` or `sealed unique` in one step
  (`sealed_index_backfill_not_built`, B8), a field that is not `text`
  before (`seal_type_not_built`), a node with no key custody.
- Not built: the plaintext kept by `seal.keep_plaintext`; live writes during
  the seal and the pin switch at its end (11a); every tenant by a rollout
  (11b). Each tenant runs it with `ordo evolve --tenant`, which pins the
  tenant as its seal ends. The deployment's own space serves the version
  before between `--run` and the activation.
- **Blobs.** An object's key is `<space>/<handle>`
  (`plan-tenant-storage.md`). It holds no field name. A renamed blob field
  moves no object, in the deployment's bucket or a tenant's own.
- A type change to or from a blob field is not on the menu. It is refused.
- **A tenant database that cannot be reached** leaves the tenant on X.
  Ruled 2026-11-23 (Q10): the rollout counts it as waiting, not failed.
- **A destructive step on a tenant's own database** needs that tenant
  owner's confirmation (DL §9, `plan-schema-evolution.md` increment 5).
  The tenant waits in the rollout until it is given.

---

## 8 · Build steps

| Step | Increment | Size | What |
|---|---|---|---|
| B1 | 10b | M | `$backfills`, the chunk loop, retry and dead-letter, the move lock, phases `expanding` and `backfilling` with no live writes |
| B3 | 10b | M | Verify; the golden test, then the summarised group and the full-image option |
| B6 | 10b | S | Backfill functions |
| B2 | 11a | L, research, 2 to 3 tries | The move overlay: forward dual writes on X, both guards, the phase on the pin stream |
| B4 | 11a | M | The switch, reverse dual writes on Y, the soak, revert in the soak, soak close into `$orphans` |
| B5 | 11b | M | Driven by a rollout; shared-database constraints (§4); `ordo rollback` and `--dry-run` |

What sets the pace:

- B2 is research. A rehearsal against a copy of a real tenant's data ends
  each try.
- B3 touches the frozen chain encoding: 1 extra review.
- A CI round is about 35 minutes for the full suite. About 2 rounds per
  step.
- Q10 and Q14, and Q3 for B5.

---

## 9 · Tests

Host tests (`crates/ordo-host/tests/`, beside `apply_everywhere.rs`):

- Kill the worker mid-chunk. Old-shape reads are still right, the registry
  is untouched, the worker resumes from its cursor, verify passes.
- Write to a row behind the cursor and to one ahead of it during
  `backfilling`. Both have the right new columns at verify.
- The result equals a run over a quiet database, row for row.
- A write Y's guard refuses is refused during the move, naming both
  versions.
- Revert in the soak: every write made on Y is there on X.
- 3 tenants in one database: the `NOT NULL` lands only after the third
  switches.
- The chain's golden bytes are unchanged for every existing group kind.

Through the examples (owner, 2026-11-23), in `examples/notes-v3`'s e2e
(`plan-versions.md` increment 7a):

1. Seed 500 notes on notes v2. Open the page.
2. Apply v3, which renames `title` to `heading` with `prev`.
3. While the backfill runs, type into 3 notes and add 1. Expect each
   change on the page after a reload.
4. After the switch, expect all 501 notes, with the typed text.
5. Roll back inside the soak. Edit a note on v2. Expect every edit from
   step 3 and step 5.
6. Kill the host during step 3 in a second run. Restart. Expect the same.

---

## 10 · Open questions for the owner

- **Q10. A tenant whose own database cannot be reached during a rollout:
  waiting or failed?** Recommendation: **waiting.** The default failure
  threshold is 1 tenant, so one tenant's offline database would pause
  every rollout for everyone.
- **Q14. May a backfill function call out or read secrets?**
  Recommendation: **no, off by default, with `backfill.function_egress`
  as the opt-in.** A function that calls out can give different values in
  staging and production, which is what the rehearsal exists to rule out.
- Q3 (one open rollout) is in `plan-versions.md` §3.7.
