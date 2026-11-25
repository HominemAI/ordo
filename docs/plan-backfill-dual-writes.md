# The backfill worker and dual writes: how one tenant moves through a transforming step

Status: **B1, B3 and B6 built 2026-11-23** (`plan-versions.md` increment
10b; `crates/ordo-host/src/backfill.rs`). **B2 and B4 built 2026-11-24**
(increment 11a, try 1; `crates/ordo-host/src/dual_writes.rs`,
`crates/ordo-exec/src/overlay.rs`). **B5 part 1 built 2026-11-24** (increment
11b; `crates/ordo-host/src/every_tenant.rs`). **B5 part 2 built 2026-11-24**: §4 on
the deployment's own database, `backfill.concurrency`, grants that stay killed.
**B5 part 4 built 2026-11-24**: fills on the deployment's own database, and
the new shape's uniques and checks held (§4).
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
- Settings: `chunk_rows` 1000, `chunk_timeout` 20 s, 5 attempts, and a
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

**Built (11a, try 1, 2026-11-24).** What the try found:

- The overlay is not a variant of X's `Loaded`. It is data on the tenant's
  pool handle (`Pool::with_overlay`), run by the write protocol after the
  op's statements and before the chain append, in the same transaction. It
  sets the other version's columns on every row the command group wrote,
  by `(entity, id)`, then checks the guards on those rows. The op's plan is
  untouched, so every op of X and Y gets it, with no compiler change.
- 1: the forward `SET` is the backfill's own SQL, the mapping lowered by
  `render_mapping`. A rename's reverse is the new column. A B5 fill keeps
  what a row holds (`coalesce`). A backfill function has no per-row
  mapping, so its step runs as before, with no dual writes (D16).
- 2: one `MoveOverlay` per move behind an `Arc`, a few hundred bytes: its
  SQL strings. A tenant's view is cut again when its overlay changes.
- 3: a refused write answers `check_violation`, naming both versions, the
  field, the row and the commands that finish or revert the move.
- The guards are each side's `required` on the moved fields: the database
  cannot hold them while both versions write, since a `NOT NULL` is
  checked per statement, before the overlay runs. The expand opens the new
  column; its `NOT NULL` lands at the soak's close.
- A rename's old column keeps its `NOT NULL`, so the version before still
  loads over the database (increment 15's rule). An insert by Y does not
  name it, so it takes its default first: its own, or the new field's when
  it has none. A required field renamed with no default is refused
  (`prev_default_required`). The reverse `SET` keeps the old value when Y
  leaves the new one empty (`coalesce`), and the guard refuses the write.
- **Renames outside a move** (2026-11-24, replacing increment 15's sync
  trigger; the database, not the node, says since the follow-up). A rename
  `ordo apply` makes, or a stored step runs with `--run`, has both versions
  serving on one database until the pins move, with no `$moves` row. The
  rename's step records it in that database's `$renames`, after
  `LOCK TABLE "$renames" IN EXCLUSIVE MODE` and before its new column, all
  in one transaction. Every write's first statement is
  `LOCK TABLE "$renames" IN ROW SHARE MODE`, which takes no snapshot, then
  it reads the table and runs each rename's dual write as its own version
  reads it (`MoveOverlay::for_renames`). A write that locked first commits
  before the new column exists, and the copy reaches it; a write after reads
  the rename. No node needs to have read anything, so a node slow to read
  the registry loses no write (`prev_rename.rs`,
  `two_nodes_one_never_told_lose_no_write_across_a_rename`). A
  `SELECT … FOR SHARE` there lost 2 writes in 341: it takes the
  snapshot before it waits. A soak's close and a rename's reverse delete
  the row, so an orphaned old column is not written again. A move's own
  overlay takes precedence.
- The overlay is keyed by version, never by phase: forward for a write on
  X, reverse on Y. A node that took the new pin before the new move cannot
  run one version's overlay under the other. The pin stream reads the
  moves before the pins.
- A rename in a stored plan is copied by the backfill worker (B4), in
  chunks, rather than by one `UPDATE` in the step (increment 15's `ordo
  apply` path keeps its `UPDATE`). Verify under live writes checks the
  mapping on every row and records the count and hash it reads; the
  per-chunk hashes are not compared, since live writes change them.
- No rehearsal against a copy of a real tenant's data was run (§8).

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

**Built (B5 part 2, 2026-11-24).** One try, as said. What it found:

- Since 2026-10-06 every new tenant gets a database of its own. The
  deployment's own database holds the deployment's own space and tenants
  made before then. A database a tenant attached may hold several spaces.
- **Expand once.** The first tenant's move there runs the expand and the
  copy for every space's rows. A later tenant finds the columns there and
  the cursor at its end; verify runs again under live writes.
- **No per-tenant forward overlay is needed for a rename.** The expand
  records it in `$renames`, so every space there, on either version, keeps
  both columns current. A fill has no such record: only a move's own
  writes run its mapping. A fill there is refused as
  `shared_fill_not_built`, naming `ordo evolve --run`.
- **The close is per database, not per tenant.** A soak there closes only
  once no space there serves X: no tenant pinned to X outside a soak of
  this step, and the deployment's own space past X. Until then `--close-soak`
  and the soak's end say `soak_shared`, naming who. Then every soak of the
  step there closes together, and the `NOT NULL`, the record of Y, the
  orphan, the `$renames` row and the reader grants happen once. The
  catch-up's tick waits for the last soak there to be due (Q1). A required
  renamed field always has a default (`prev_default_required`), so an
  insert by X never fails on the new column meanwhile.
- `ordo evolve --run` of the step on that database is refused while a move
  through it is open there (`moves_open`): it would land the constraints
  under the soaks.
- **A seal** on a database several tenants share: the end raises every
  one's pin fence in its transaction, to the epoch the catch-up pins each
  with next (`StepContext::fences`). Part 1 refused it.
- **Concurrency.** `backfill.concurrency` (default 8) and
  `backfill.per_database` (default 2) are stored settings, set with `ordo
  versions set`. A rollout's turn gathers its tenants with a database of
  their own into waves of `concurrency` and runs each wave at once, a
  semaphore per database. The tick reports the most that ran at once.
- Tests: `prev_rename.rs`
  (`tenants_on_the_deployments_own_database_move_and_their_soaks_close_together`,
  2 tenants and the own space, writes through the real write path),
  `sealed_backfill.rs`
  (`a_seal_on_a_database_3_tenants_share_fences_each_of_them`),
  `rollouts.rs`
  (`a_turn_moves_tenants_at_once_up_to_backfill_concurrency_and_per_database`).
- **Part 3 (2026-11-24):** a rollout moves a tenant on the deployment's
  own database with dual writes too. It may start with that database on
  the version before when the stored path is one rewriting step; the own
  space waits for the first move's copy.
- **Part 4 (2026-11-24): a fill there.** The move records each fill in
  that database's `$fills` (`ordo_exec::overlay::FILLS_DDL`) under
  `"$renames"` in `EXCLUSIVE` mode, before the expand and the copy. Every
  write reads it under the same fence and, while its version does not
  require the field, sets the column by the mapping and checks it is set
  (`MoveOverlay::for_database`). So the copy runs once, and every space
  there keeps the column current, as `$renames` keeps a rename. The close
  removes the row after the `NOT NULL` lands; so does a `--run` step's end.
- **Part 4: uniques and checks of the new shape.** In a step with dual
  writes a unique or a check the new version adds leaves the expand for
  `after` (`schema_apply::Triaged::held`; the receipt's `held`, present
  only then, so older digests hold). The move's overlays guard it on every
  row the moving tenant writes, on either version, refused by name. The
  copied rows are checked before the switch, and every row at the close
  and at a `--run` step's end: a row that breaks it refuses as
  `held_constraint_broken`, naming up to 5 rows as `space/id` and the
  command to run again. Nothing lands then.
  - A check lands at the close, `NOT VALID` then `VALIDATE`, which takes
    no long lock.
  - **Built differently:** a unique lands at the switch, after the check
    of the copied rows. The new version's `set` converges on a unique key
    with `INSERT … ON CONFLICT`, which needs the index (found by the test).
    A space on the version before in the same database meets it through
    its overlay's copy. Since part 5 that is refused by name: the overlay
    reads the database client's typed `is_unique_violation` and answers
    `duplicate` (409), naming the field, the row and `ordo evolve
    --tenant`, never the database's own text
    (`ordo_exec::overlay::Duplicate`).
  - **Part 5:** the guard of a unique reads a plain index over the space
    and its columns, `<unique>$held`, built `CONCURRENTLY` after the
    expand and dropped at the close. The guard is spelled with `=` so the
    index is used; a row holding a null there is asked again with `IS NOT
    DISTINCT FROM`. Tests: `prev_rename.rs`
    (`a_space_on_the_version_before_that_meets_the_new_unique_is_refused_by_name`,
    `the_guard_of_a_held_unique_reads_its_plain_index_until_the_close`,
    which reads the guard's `EXPLAIN`).
  - The pre-check of the expand skips a held constraint: it would read a
    column the expand has not made, and the copied rows are checked
    instead.
- Tests: `prev_rename.rs`
  (`a_fill_on_the_deployments_own_database_keeps_every_space_current_and_a_check_waits_for_the_close`,
  `a_unique_the_new_version_adds_is_checked_against_the_copy_and_lands_at_the_switch`,
  `a_step_run_with_a_held_check_names_the_rows_that_break_it`); notes e2e
  `rollout-moves` (the new version's check refuses Cat's title in the
  soak, by name).
- Not built: dropping an orphaned column per tenant.

**Grants a move killed stay killed (V-53, built 2026-11-24).** The door
decides from the token, so a grant Y refuses would read again after a
revert to X. At the switch, every live grant in the tenant that Y refuses,
judged as the door judges it, is recorded in `$grant_kills` (control
database). Every node reads it with the moves, and the door refuses the
grant as `GrantKilledByMove`, answered `access_denied` (part 3, as a
revoked share is, so a client drops that share's device copy) naming both
versions and `std.space.share`. A grant made after the switch is never
recorded, so sharing again reads. Test: `notes_app.rs`
(`a_grant_a_move_killed_stays_killed_after_the_revert`).

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
  (`seal_then_index`, renamed in part 2), a field that is not `text`
  before (`seal_type_not_built`), a node with no key custody, and a field
  a reader app reads (`seal_read_by_reader`): the reader would read
  ciphertext. It names the reader and the fix: a reader version that reads
  the sealed field (7b, not built), or the table off the `reader` line in a
  version before the seal.

**Built (13, 2026-11-24, second part: 64612e41).**

- **Live writes during the seal.** The seal records itself in the tenant
  database's `$seals` (`ordo_exec::overlay::SEALS_DDL`), in its expand's
  transaction, under `"$renames"` in `EXCLUSIVE` mode: 11a's fence. Every
  write holds `"$renames"` in `ROW SHARE` from its first statement, reads
  `$seals` under it, and, while its version declares the field plaintext,
  seals what it wrote into `<field>$seal` in its own transaction with its
  own custody (`overlay::apply_seals`, `Purpose::Write`). No node needs to
  have read anything.
- Verify runs as verify under live writes: it opens every row against its
  plaintext and records the count and hash it reads. The per-chunk hashes
  are not compared, since writes reseal rows.
- **The end** runs in one transaction under the same exclusive lock: a row
  left unsealed (written outside the write path after verify) refuses it
  (`seal_unfinished`), then the plaintext is dropped, the sealed column
  renamed, and the `$seals` row deleted.
- **The pin at the end.** With `ordo evolve --tenant`, the end also raises
  the tenant's pin fence to the epoch of the pin that follows, in the same
  transaction. A node still on the version before is told the pin moved
  (503, retried), never a type error on the column. A seal is therefore
  the last step of a replay: a path with steps after it is refused, naming
  the version to stop at. `--budget` and `--chunk-rows` reach this path.
  If the pin then fails, the tenant writes nothing until a run again pins
  it.
- **`seal.keep_plaintext`** (`ordo evolve --keep-plaintext
  <entity>.<field>`, D9's opt-in): stored in the plan's options and so in
  its digest. The plaintext column is renamed `<column>$plaintext`, never
  written again, and recorded in `$orphans` for erasure, export and
  `ordo evolve --prune`. The receipt names the risk: its values stay
  readable until pruned, and in WAL and backups for good.
- **B8, a sealed field gains `indexed`.** Recorded in `$seals` as kind
  `index`; `<field>$bidx` is added open to nulls. The worker opens each
  row's sealed value under a `Purpose::Migration` lease per space per
  chunk, normalizes it as the field declares and writes its blind index (a
  null indexes the null sentinel). A write by the version before computes
  the index of what it sealed. Verify computes every index again. The end
  makes `<field>$bidx` `NOT NULL` under the lock. `ordo apply` refuses it
  (`index_needs_stored_plan`); `sealed indexed unique` is refused
  (`sealed_unique_backfill_not_built`), since its duplicates can only be
  found over the indexes the backfill computes.
- Tests: `sealed_backfill.rs`
  (`writes_during_the_seal_land_sealed_and_none_is_lost`,
  `keep_plaintext_keeps_the_column_as_an_orphan_and_says_why_not_to`,
  `a_sealed_field_gains_its_blind_index_under_live_writes`,
  `a_blind_index_outside_a_stored_plan_or_unique_is_refused_by_name`);
  notes e2e project `seal`: Ada edits a note and writes one while her seal
  stops at its budget; both land sealed; the page reads them after.

**Built (13, 2026-11-24, third part: 949c9879, branch `evo-13-part3`).**

- **B10, a sealed indexed field's `normalize` changes.** Every row's blind
  index is computed again by the worker under the new normalization into
  `<field>$bidx$next`, one `Purpose::Migration` lease per space per chunk.
  The step is recorded in `$seals` as kind `reindex`, with the
  normalization (`$seals.normalize`, new). Every sealed writer fills the
  new index under it, beside its own. Verify computes every index again.
  The end, under the lock: a row left without one refuses it, the old
  index is dropped, the new one takes its name and becomes required, and
  the field's unique, composite keys and indexes are made again over it.
- `ordo apply` refuses B10 (`reindex_needs_stored_plan`). A `normalize`
  change on a field with no stored blind index changes nothing stored and
  is applied. The `normalize` probe no longer runs `lower()` over
  ciphertext.
- The diff compares declarations against the version a tenant's data was
  written under too, so a tenant's own database sees the change.
- **A rollback reads a whole index** (owner, 2026-11-24, PR 80). The end
  keeps the old index as `<field>$bidx$prev`, recorded in `$seals` as kind
  `kept` with the old normalization and in `$orphans`. Every write keeps it
  current until `ordo evolve --prune` removes it (the close). An
  activation of a version that declares the kept normalization trades it
  back in before its gate (`apply_op::restore_kept_indexes`), so `==`
  there finds every row; forward again trades back. Test:
  `a_rollback_across_a_new_normalize_finds_every_row`.
- **A tenant's own database follows its pin across B10** (built
  2026-11-24, branch `fix-b10-tenant-rollback`). The kept row also records
  `live`, the normalization of the field's own index, so a trade needs no
  version named. A node trades the kept index in before it serves a
  tenant on the version that declares its normalization
  (`Spaces::resolve`, once per space, version and pin epoch), whatever
  moved the pin: `ordo versions pin`, a move off a revoked version, a
  rollout, a tenant build, a restore, the catch-up. The trade asks the
  normalization again under the `$renames` lock, so two nodes trade once,
  and raises the tenant's pin fence to the epoch served, so a node still on
  the pin before is told the pin moved and writes nothing into the index it
  no longer reads. The catch-up's apply path and a stored step's run read a
  kept index under the target's normalization as declared already: `ordo
  rollback` moves the tenant back rather than refusing
  `reindex_needs_stored_plan`, and a rollout forward again does not compute
  the index again. A database shared with a tenant pinned to the other
  version is refused for the tenant moved, naming the other tenant and
  `ordo versions pin`. Tests: `crates/ordo-host/tests/kept_index_tenants.rs`
  (each path, the fence, the shared refusal).
- **Refused: a tenant on the deployment's own database pinned apart from
  the active version across B10.** Its index there is under the active
  version's normalization, and only an activation trades it, for every
  space at once. `ordo versions pin` refuses the move, naming `ordo
  versions pin <tenant> <active>` and `ordo rollback <version>`; a pin made
  any other way is not served, with the same words.
- **B10 on the deployment's own database keeps every space there
  serving.** `ordo evolve --run` of a step that only reindexes keeps the
  active version's index the field's own and the new one beside it as
  `<field>$bidx$prev` (`$seals` kind `kept`, the new normalization); the
  activation trades it in for every space at once. The own space does not
  move ahead. A database one reindex ahead of the serving version is not
  held as behind (`Spaces::ahead_by_reindex`, from the stored plans), so
  a tenant there reads and writes between the run and the activation.
  Test: `a_tenant_on_the_deployments_database_serves_between_the_run_and_the_activation`.
- **Refused: a second B10 on a field whose first one is still kept**
  (`reindex_kept_index`, naming `ordo evolve --prune`): its end would
  replace the kept index. A step back to the kept normalization is not a
  second B10: it is read as declared already, and nothing is computed.
- **`sealed unique` by backfill.** B8 and B10 under `unique`: once the
  indexes verify, the run reads the duplicates over them and refuses the
  end, naming each space and its rows (`sealed_unique_duplicates`). The
  step stays recorded, so writes keep the index current while the rows are
  fixed; a run again ends it. The constraint lands in the end's
  transaction, which asks again under the lock. A `unique` gained with the
  index is folded into the backfill's end; a run again reads the backfill
  in progress from `$seals` (`Choices::indexing_live`).
- **The deployment's own space through a seal on its database.** `ordo
  evolve --run` of a sealing step (B7, B8, B10) first gives the own space a
  pin at the active version (`Versions::own_pin_made`, a `$pins` row and
  the nudge), so its writes carry an epoch. The end raises its pin fence
  in the end's transaction. The own space then moves to the version the
  database holds (`Versions::pin_own_for_step`), first, as a rollout moves
  it (Q4). "Active" waits for the activation. A node still on the version
  before is told the pin moved (503, retried); the new version's writes
  land sealed.
- Tests: `sealed_backfill.rs`
  (`sealed_unique_by_backfill_refuses_duplicates_by_row_then_lands`,
  `a_new_normalize_computes_every_index_again_under_live_writes`,
  `a_new_normalize_under_unique_refuses_collisions_naming_the_rows`,
  `a_new_normalize_outside_a_stored_plan_is_refused_by_name`,
  `the_own_space_moves_with_a_seal_on_its_database_and_its_writes_land_sealed`);
  notes e2e project `seal`: `--run` names the own space on 0.3.4 while
  0.3.3 stays active.
- Not built: activating a version that runs a tenant's stored seal on a
  shared database, and tenants that share the deployment's database during
  its seal (11b); a seal's end killed between the drop and the record (a
  run again does not find the sealed column).
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
| B2 | 11a | L, research, 2 to 3 tries | The move overlay: forward dual writes on X, both guards, the phase on the pin stream. **Built 2026-11-24, try 1** |
| B4 | 11a | M | The switch, reverse dual writes on Y, the soak, revert in the soak, soak close into `$orphans`. **Built 2026-11-24** |
| B5 | 11b | M | Driven by a rollout; shared-database constraints (§4); `ordo rollback` and `--dry-run`. **Part 1 built 2026-11-24**: the rollout, the activation, `ordo rollback` and `--dry-run`. **Part 2 built 2026-11-24**: §4, concurrency, grants stay killed |

What sets the pace:

- B2 is research. A rehearsal against a copy of a real tenant's data ends
  each try.
- B3 touches the frozen chain encoding: 1 extra review.
- A CI round is about 35 minutes for the full suite. About 2 rounds per
  step.
- Q10 and Q14, and Q3 for B5.

---

**Built (B4, 2026-11-24).**

- `$moves` gains `step` and `overlay`; phases `backfilling`, `gated`,
  `soaking` beside increment 7's. `ordo evolve --tenant <id>`
  (`std.deployment.replay`) opens the move, runs the expand once, the
  backfill under `--budget` and `--chunk-rows`, verify, and the switch.
  Run again, it resumes. `--soak` sets `soak.length` for the move: `7d` by
  default, `0` closes at the switch. It is a run option, not in the plan's
  digest (asked).
- Opening, the switch, a revert and the close each raise the tenant's pin
  epoch in the transaction that changes the move, and the fence in the
  tenant's database at once.
- `--revert` (`std.deployment.revertSoak`) moves the pin back in the soak,
  phase `gated`. `--close-soak` (`std.deployment.closeSoak`) closes it now;
  the tenant catch-up's tick closes it when `soak_until` passes. The close
  lands the new columns' `NOT NULL`, records Y, writes the old column to
  `$orphans` (increment 14's table) and makes the reader grants again.
- A hand pin, an activation and the catch-up leave a tenant in such a move
  alone; `ordo versions pin` refuses naming the move's commands.
- Refused: a tenant on the deployment's own database (shared, 11b); a path
  whose rewriting step is not the only one left.
- Tests: `notes_app.rs`,
  `a_rename_under_live_writes_loses_no_write_and_a_revert_in_the_soak_keeps_every_write`;
  notes e2e project `moves` (`tests/evolve/dual-writes.spec.ts`).

**Built (B5, part 1, 2026-11-24).** `crates/ordo-host/src/every_tenant.rs`.

- **A rollout** moves each tenant with a database of its own through its
  stored path to the target (`forward_tenant`), when one leads there from
  the version its database records. No stored path: the apply path, as
  before.
- A path of one rewriting step is the move above, with the default 7-day
  soak. A rewriting step among others is refused, naming the version to
  replay to first, as `ordo evolve --tenant` refuses it.
- The backfill takes `backfill.budget` chunks per turn. The tenant is
  `moving` with its rows done until the switch, which pins it; the
  rollout names itself on that `$pins` row.
- Any other path, a seal included, is replayed and the tenant pinned.
- **An activation's catch-up** replays the stored path on each tenant
  database the same way (`forward_database`), with the node's backfill
  functions and key custody. A rewriting step there runs as `ordo evolve
  --run` runs it: `$renames` keeps both columns current. Without
  `backfill.budget`, a long backfill holds the apply lock for its length.
- **`ordo rollback` to X** puts back every tenant soaking on a move from X
  (`revert_soaks_from`), after the activation, under the apply lock. It
  also runs when X is still active, which is how a rollout aborted in the
  soak is undone. Through the door each tenant database's fence rises;
  with `--direct` only the pin epoch does.
- **`--dry-run`** lists each tenant not on X: back with every write, stays
  (X already active, or a tenant build), or moves back and loses what a
  renamed field took since its soak closed. On the deployment's own
  database `$renames` keeps both columns, so nothing is lost there.
- Tests: `notes_app.rs`
  (`a_rollout_moves_3_tenants_through_a_rename_under_live_writes_and_a_rollback_in_the_soak_keeps_every_write`,
  `an_activation_runs_each_tenant_stored_step_a_seal_included`); notes e2e
  project `rollout-moves` (`tests/evolve/every-tenant.spec.ts`).
- Not built in part 1, built in part 2 (§4): a tenant on a shared
  database in a move of its own, the new shape's constraints at the
  close, soaks on a read table closing together; `backfill.concurrency`
  and `backfill.per_database`; grants a move killed staying killed after a
  revert.

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
