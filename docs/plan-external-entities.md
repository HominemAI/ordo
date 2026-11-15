# External entities: tables Ordo did not create

Status: **ruled 2026-10-01, reshaped 2026-11-15** (board `external-entities`).
Checklist 27i, 27j and 27l. 27k is deferred (section 9).
This is the single source for the build agent.

The owner (g22): *"We need to discover a way for the inter-system dependent
workloads. Allowing users to consolidate their external usage into the same
database makes sense. A connector could be a good way to go about this."*

---

## 1 · Where we are

- **Ordo tolerates other tables.** Extra live tables, columns and
  constraints are not drift (`crates/ordo-schema/src/diff.rs:484`).
- **Ordo cannot use them.** A ref points only at an entity. Every entity has
  `$space` and `PRIMARY KEY ($space, id)`. An outside table has neither.
- **Ordo does not notice them.** Nothing records that other tables exist.
- **Built 2026-11-15** on branches `external-entities` (slice A) and `external-entities-bc` (slices B and C). Section 12 says what each slice did and decided.

Rulings:

- **2026-10-01** External entities as generated, read-only translations: yes.
- **2026-10-01** Other systems read Ordo through the door. Grant-based views maybe, later.
- **2026-10-01** Not deferred. First apply lists untracked tables, and a clash is an error naming them.
- **2026-10-01** Ordo's tables under a prefix plus a random id (27k). Now deferred, see 2026-11-15.
- **2026-11-15** The foreign table lives where Ordo's SQL runs: the deployment's database, or each tenant's. A plain join, no door, no hop, no FDW, no second connection.
- **2026-11-15** An external entity is its own kind, `schema.externals`, not an entry of `schema.entities`.
- **2026-11-15** Ids are prefixed text over a raw key. Composite keys and tables with no unique non-null key are refused.
- **2026-11-15** `shared` is required in slice 1. The access profile decides which tenants may call the ops.
- **2026-11-15** 27k, the random schema, is deferred until a real collision appears. Foreign tables take their own schema.
- **2026-11-15** The translation compiles to a SQL subquery, never a view, never a hop.
- **2026-11-15** Read-only. Derived `get` and `list` only. Writes reach a non-Ordo system through its API.
- **2026-11-15** No foreign key from a native ref to a foreign row. A missing row reads as missing.
- **2026-11-15** The translation re-imports on every apply. A human is involved only when an op would break ("that seems sensible").
- **2026-11-15** Live reads are refused in slice 1.
- **2026-11-15** The Postgres reader is built once, for the generator and for `ordo introspect`.
- **2026-11-15** The file states what an external entity is not (section 10).
- **2026-11-15** Order of work: external entities, then the HAL console's reauth, then 0.5.2.

## 2 · The file syntax

The generator writes it. The developer reviews it as text.

```
external customers from crm.customers shared {
  id      <- customer_id
  name    : text    <- full_name
  email   : text?   <- email
  account -> accounts <- account_id
}
```

The order on the first line is `external <name> [(key: ...)] from
<schema>.<table> shared [public] {`.

- `external customers` names the entity. Its key derives by the usual rule
  (`cust`), and `(key: ...)` may override it. A key collision fails the compile.
- `from crm.customers` is the schema and table. Both are always written.
- `shared` is the tenancy word. It is required (section 5).
- `public` after `shared` is optional (owner, 2026-11-15, ask 5). It works as
  on an entity, and `name: text public <- full_name` marks one field as on a
  native field. An authored `op ... public` may then serve the public fields
  on the public path. Derived `std.<name>.*` ops are never public, as for a
  native entity. A guarded external cannot be public, as for an entity
  (`public_guarded`). The public cache for an external's answers is not
  invalidated by writes, since Ordo never writes the table, so a cached answer
  lives until `ORDO_PUBLIC_CDN_S_MAXAGE` ends (default 60 seconds).
- `id <- customer_id` maps the id to one unique, not null column. Its base
  type is read from the generated types, not guessed.
- `name : text <- full_name` is a field with its type written out. `?` means
  nullable. The compiler needs no database. Apply checks the types.
- `account -> accounts <- account_id` is a ref to another external entity
  (`accounts`), read from `account_id`. It holds that entity's typed id.

### Refused at compile time

Each refusal has a class name and says what the developer can do. The names
are proposed, so the build agent may adjust them in one pass.

| Refused | Error | Fix it names |
|---|---|---|
| Any write op (`set`, `create`, `delete`) | `external_read_only` | Write through the system's API from an effector |
| `owned` | `external_owned` | Remove it, the other system owns the rows |
| `acl` | `external_acl` | Gate with the access profile instead |
| `sealed` | `external_sealed` | Remove it, Ordo never wrote the value |
| `tree`, `unique`, `check`, `index` | `external_unsupported` | The other system owns these |
| `public*` | `external_unsupported` | An external owns nothing, so the star reaches nothing. Write `public` |
| `prev` on a field | `external_unsupported` | A translation is re-imported on every apply instead |
| Inherited guards (guards that walk `owned` or `acl`) | `external_inherited_guard` | Write a guard over the entity's own fields |
| Live reads (`?live=1` on an op that reads it) | `external_not_live` | Read it on request, not as a stream |
| A ref FROM an external entity to a native entity | `external_ref_native` | The other system holds no Ordo ids |
| A composite key | `external_composite_key` | Pick one unique column |
| No unique, not null column | `external_no_key` | The table has no identity Ordo can use |
| `shared` missing | `external_needs_shared` | Write `shared` |

A ref from an external entity to another external entity is allowed.

## 3 · How it compiles

`Planner::table` (`crates/ordo-plan/src/lib.rs:1366`) is the one place a base
table is named. For an external entity it emits a derived source instead:

```
(SELECT c."customer_id" AS "$key",
        'cust_' || c."customer_id"::text AS "id",
        c."full_name" AS "name", c."email" AS "email",
        'acct_' || c."account_id"::text AS "account"
   FROM "crm"."customers" AS c) AS "t0"
```

- **No view and no hop.** A view would make Postgres refuse the other
  system's `DROP COLUMN` and needs DDL in every tenant database. Postgres
  flattens the subquery, so the cost is one string concat per returned id.
- **The id is the key plus `_` plus its canonical text.** Keys hold hyphens,
  never underscores, so the first `_` always splits it.
  - `int`, `bigint`, `smallint`: decimal text, `cust_42`.
  - `uuid`: lowercase with hyphens.
  - `text`, `varchar`: verbatim.
- **The raw `$key` column does all the work.** Equality, joins, sort and
  keyset cursors use `$key`, so the foreign indexes serve them and paging stays
  numeric (`cust_10` never sorts before `cust_9`).
  - `id = $2` becomes `"t0"."$key" = CAST(substr($2, 6) AS bigint)`.
  - A native ref joins as `"t0"."$key" = CAST(substr("t1"."customer", 6) AS bigint)`.
- **A bad id param is checked before SQL.** The door checks the prefix and
  that the rest parses as the base type. A bad one is "not found", never a
  cast error. A malformed stored ref must not abort a whole query.
- **Per tenant, the same SQL.** It names `"crm"."customers"` and runs in the
  database `crates/ordo-exec/src/spacedb.rs` resolves for the space. One
  translation serves every tenant that has its own copy of the table.
- **A database without the table** answers `external_missing`, naming the
  table.
- **`verify_scoped`** (`crates/ordo-plan/src/lib.rs:786`) requires
  `"alias"."$space"` on every source. Add a third alias kind, `external`,
  registered only for externals. The belt still catches a hand-rendered source.
- **Std ops.** `std_ops` (`crates/ordo-schema/src/lib.rs:1974`) emits
  `std.customers.get` and `std.customers.list` only. Developer ops may read
  and join it freely.

## 4 · The schema model

The ruling: an external entity is its own kind. About 61 loops over
`schema.entities` in 29 files assume Ordo owns the table and that it has
`$space`. Opting in at 4 sites is safer than opting out at 61.

`schema.externals` holds the entities. Sites that read it:

- **sema** (`crates/ordo-sema`): the checks of section 2, and the NOT NULL
  evidence. `crates/ordo-sema/src/lib.rs:2484-2505` trusts `NOT NULL` from Ordo's
  own DDL. For an external column the evidence comes from the catalog.
- **query planner** (`crates/ordo-plan`): section 3.
- **guards**: `$caller` over the entity's own fields (section 5).
- **SDK descriptor and emitter**: the type is marked external, not live, no
  `rev`, no undo.

Sites that must never see it:

- **diff and DDL planner** (`crates/ordo-schema/src/diff.rs:775-806`, `ddl.rs`).
  Today a declared entity with no live table plans `AddEntity`, and removing
  one plans `DropEntity` if a live table has its name. Matching names would
  `ALTER` the other system's columns. `ddl.rs:527-541` would emit a foreign key.
- **dev reset** (`crates/ordo-cli/src/dev_db.rs:466-469`), which drops every
  table the program declares.
- **erase** (`crates/ordo-exec/src/erase.rs:485`), which deletes by `$space`
  from every entity.
- **export and its drift check** (`crates/ordo-exec/src/export.rs:1058`).
  Foreign rows are not the space's data.
- Also import, reseal, undo, share, exit and pins, which loop `schema.entities`.

A test asserts that a program with an external entity produces an empty plan.

## 5 · Tenancy and guards

- **`shared`** means every space may read every row, subject to the access
  profile. In a tenant's own database the database is the boundary. In the
  deployment's database a foreign table is deployment-wide data, like a price list.
  Without `access read`, every member of every space reads every row. `shared`
  is that choice, written down.
- **Access profile gating.** Which tenants may call the ops is the profile's
  op lines (`docs/howto-roles.md` §8). Nothing new is needed.
- **No `$space`.** There is no space predicate and none may be invented.
- **Guards work over the entity's own fields and `$caller`.** A `read` guard
  is a predicate on the subquery. A guard naming a column the table lacks
  does not compile.
- **A native entity's guard may walk a ref into an external one.** It is a
  join like any other.
- **Row-level tenancy later.** A tenant-column map is not in slice 1.
  Visibility is all rows or none per space.
- **Roles.** Ordo's database role needs `USAGE` on the foreign schema and
  `SELECT` on the table. Apply checks with `has_table_privilege` and refuses
  with the exact `GRANT` statement (`external_no_grant`).

## 6 · Refs

- **Native to external** is a plain text column holding the full typed id.
  There is no foreign key, because Ordo must not veto the other system's
  deletes. The write path checks the prefix only.
- **A missing row reads as missing**, as a deleted row would. Reads left
  join and never fail on a dangling ref.
- **Importing a space** can bring back refs to rows that no longer exist.
  That is expected.
- **External to native** is refused (section 2).

## 7 · Drift and re-import

Ordo does not migrate the foreign table. It never runs DDL on it.

The translation **re-imports on every apply**. On the same read, Ordo
compares each translation with the live table, per database.

| What happened | What Ordo does |
|---|---|
| Unchanged | Nothing. |
| Column added | Takes it, with a note. The translation lists it as unused. |
| A column no op reads changed type or went away | Takes it, with a note. |
| A column an op reads was renamed, retyped or dropped | Stops every op that reads the entity, not only those reading the changed column (owner, 2026-11-15, the whole entity is simpler and the refusal names the fix). Each refuses with `external_changed`, naming the column and the fix. |
| Table gone, or the grant revoked | Same refusal, naming the table or the `GRANT`. |

- **The fix is a regenerated file.** The developer reruns the generator. The
  diff is an ordinary program change, reviewed under
  `docs/plan-evolution-plans.md` §3.1. A human is involved only here.
- **The watcher runs on a timer** and reuses the existing job runner. It
  reports and flips the refusal on. It never rewrites the file.
- **Between a foreign change and the next check** the SDK may promise a shape
  the table no longer has. The refusal turns that into an error, never wrong data.

The owner: *"that seems sensible."*

## 8 · Noticing other systems (27j)

Unchanged. On every apply, Ordo lists the tables and schemas it does not own
and records them in the plan as **"present, not Ordo's"**. Nothing is changed.
`ordo plan` prints the list.

The same read serves the drift check, so it costs one query per database.

## 9 · 27k, the random schema: deferred

Ordo's tables stay in `public` until a real collision appears.

- `'public'` is a literal in `erase.rs:498` and `:568`, `shape.rs:267` to
  `:364`, `migrate.rs:278`, `export.rs:1058`, `dev_db.rs:67`, and `LIVE` in
  `schema_apply.rs:94`.
- 2 instances are live. Moving them is an L with a rehearsal on a copy of each.
- An old binary reading `public` after a move sees no tables, a cutover hazard.
- An external entity creates no table, so it cannot clash.
- **Foreign tables take their own schema.** The first-apply clash error
  stays as ruled 2026-10-01.

## 10 · What an external entity is NOT

These 5 statements go into the developer docs as written.

1. It is **not attested**. No chain, checkpoint or signed head covers its
   rows. A response's head says nothing about them.
2. It is **not sealed and has no history**. Its columns are as plain as the
   other system keeps them. It cannot be undone or reverted.
3. Its ids are **not permanent**. The other system can delete, reuse or change
   a key. A ref to it can dangle or come to name another row.
4. It is **not live at commit**. Changes are seen on the next read. A stream
   or an offline copy is as stale as its delay.
5. Its shape is **not Ordo's promise**. When the other system changes the
   table in a breaking way, every op that reads the entity refuses until a
   regenerated translation is reviewed and applied.

## 11 · The generator

`ordo introspect --external crm.customers` writes the block of section 2.

- **It is built on `migrate::live_shape`** (`crates/ordo-exec/src/migrate.rs:55`),
  already schema-parameterised. Extend it to report the primary key and the
  unique columns.
- **Types come from the table in `docs/migration.md` §2a.** Anything outside
  it goes on the worklist, the same posture as migration.
- **It picks the key.** The primary key when it is one column, else a unique
  not null column, else it refuses with `external_no_key`.
- **It is the one Postgres reader.** `ordo introspect` reuses it later.
- **It runs** by hand at first, and again whenever section 7 refuses. It
  prints text and never touches the database. The re-import on apply uses the
  same reader and compares, it does not write the file.

## 12 · Slices, sizing and tests

Sized in agent sessions. A and B are independent. C needs both.

| Slice | Work | Size |
|---|---|---|
| A | Compiler and planner | M |
| B | Generator | M |
| C | Apply check, "present, not Ordo's" list, watcher | S to M |

What sets the pace: a CI run is about 12 minutes, and one rehearsal against
real foreign tables. A to C together are about 3 sessions and 4 to 5 CI rounds.

**Built.** A on `external-entities`. B and C on `external-entities-bc`, 2026-11-15.
What B and C decided where this plan was silent:

- **The verb** is `ordo introspect --external <schema>.<table> [--name] [--key] [--out] [--db]`.
- **`--key` wins** over a single primary key. It must name a column with a
  single-column unique index that never holds null. The generator does not pick
  a unique column by itself: no primary key and no `--key` is `external_no_key`.
- **`numeric(p,s)` is `decimal(s)`**, not `money`: `money` is whole minor units
  in a `bigint`. `numeric` with no scale, `jsonb`, arrays, `bytea`, floats,
  `interval`, enums and a non-key `uuid` are comments on the worklist.
- **The check compares mapped columns.** A column no op reads is not
  examined, so the plan's "a column no op reads" row means a column the
  translation does not map. Every mapped column counts, whether an op reads it
  or not, because the whole entity is held.
- **A mapped column that now admits null** where the file has no `?` is
  `external_changed`.
- **The check is in `plan_with`**, so `ordo plan`, `ordo apply`, `ordo migrate
  --apply` and the tenant catch-up share it. `std.deployment.apply` runs it
  too, before any schema step. A first apply over a missing table is refused.
- **One wire code**: `external_changed` (HTTP 500) is what a held entity
  answers at run time, whether the cause is a missing table, a changed column
  or a lost grant. The message names which.
- **The watcher watches the deployment's own database.** A tenant's database
  is checked when a program is applied to it. A tenant database that loses the
  table later is not held by the timer.
- **The interval** is `ORDO_EXTERNAL_WATCH_SECS`, default 300.
- **"Present, not Ordo's"** lists tables in `public` that are not the
  program's entities or `$` tables, tables in other schemas that are not
  declared externals, and schemas with no tables. It is capped at 20 lines.

**A. Compiler and planner.**
- `external ... from s.t shared { ... }` in `ordo-syntax`, `schema.externals`
  in `ordo-schema`, the refusals in `ordo-sema`.
- `ddl` and `diff` never see externals. `Planner::table` emits the subquery.
- `crates/ordo-cli/tests/external_check.rs`: every refusal in section 2, one
  case each.
- `crates/ordo-cli/tests/external_plan.rs`: a program with an external gives
  an empty schema plan, and the dev reset and erase leave the table alone.
- `crates/ordo-host/tests/external_read.rs`: get, list, join, sort and paging
  past `cust_9`/`cust_10` against a real foreign table in the conformance
  cluster, and a bad id is "not found". Planner SQL snapshots sit beside the planner.

**B. Generator.**
- `crates/ordo-cli/tests/external_introspect.rs`: each type in `migration.md`
  §2a, key choice, composite and keyless refusals, the worklist. It needs the
  cluster, so it follows `schema_apply.rs`.

**C. Apply check, list, watcher.**
- `crates/ordo-host/tests/external_drift.rs`: each row of the section 7
  table, a missing grant, a tenant database without the table.
- `crates/ordo-host/tests/external_present.rs`: the "present, not Ordo's" list.
- Follow the layout of `crates/ordo-host/tests/apply_everywhere.rs` for per-database runs.

Reused code: `Planner::table`, `verify_scoped`, the `Sources` registry,
`Entity` and key derivation, `Change::refusal` in `diff.rs`, `live_shape`,
`schema_apply.rs` (`plan_with`, `apply_everywhere`,
`apply_everywhere_except_default`), `spacedb.rs`.

Deferred: composite keys, a tenant-column map, live reads, external to native
refs, automatic regeneration of the file.

## 13 · Writes

A foreign table is never written through the translation. That would skip
the other system's rules and Ordo's seal, chain and command log.
A non-Ordo system is written through its own API, called from an effector
(`docs/definition-language.md` around line 2872). It is built when HAL needs it.
Other systems reading Ordo go through the door (ruled 2026-10-01).
