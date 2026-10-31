# External entities: tables Ordo did not create

Status: **ruled, 2026-10-01** (board `external-entities`, all four asks). Not deferred: built before first deployment. From the owner's notes on the Grief Remediation tab
(2026-10-01, rows g22, g25, g26, g27, g30). Board: `external-entities`.
Checklist 27i to 27l.

The owner (g22): *"We need to discover a way for the inter-system dependent
workloads. Allowing users to consolidate their external usage into the same
database makes sense. A connector could be a good way to go about this."*

---

## 1 · Where we are

- **Ordo tolerates other tables.** Extra live tables, columns and
  constraints are not drift, and no plan proposes dropping them
  (`crates/ordo-schema/src/diff.rs:484`).
- **Ordo cannot use them.** A ref points only at an entity, and every entity
  has `$space` and `PRIMARY KEY ($space, id)`. An outside table has neither.
- **Ordo does not notice them.** Nothing records that another system's
  tables exist, so they are easy to forget (g26).
- **Names can collide.** The schema is fixed to `public`. A new entity whose
  name another system uses fails part-way through an apply (g25).
- **The other system cannot use Ordo.** Direct database access is
  discouraged, so an app that shares the database cannot read Ordo's tables
  either (g26).

## 2 · The idea: a projection

The owner's design (g22), made concrete.

An **external entity** is an Ordo entity whose rows live in a table Ordo does
not own. A generated **translation** sits between them:

```
external customer from "crm"."customers" {
  id      <- customers.customer_id     # ids are mapped, prefixed at query time
  name    <- customers.full_name
  email   <- customers.email
}
```

- **It is generated.** The SQL-to-Ordo converter (`migration.md`, the
  introspect step) reads the foreign table and writes the translation.
- **Ordo sees an ordinary entity.** Ops read it, refs point at it, and guards
  apply to it. The entity's typed-id prefix is added and removed at query
  time, so ids look like every other Ordo id.
- **It is read-only to begin with.** Writing to another system's table would
  bypass that system's own rules.
- **It has no `$space` column.** Which tenants may see it is a setting on the
  external entity, and its rows are not sealed or chained by Ordo.

## 3 · When the foreign table changes

Ordo does not migrate the foreign table: it is not Ordo's. On every apply,
and on a timer, Ordo compares each translation with the live table:

| What happened | What Ordo does |
|---|---|
| Table unchanged | Nothing. |
| Table gone | Notes it, and stops serving ops that read it. |
| A column the translation uses changed or went away | Notes it, regenerates the translation, and shows the difference for review. |
| A column was added | Notes it. The translation is unchanged until someone adds it. |

The owner's open point (g22): *"how do the deltas get adjusted?"* Proposed: a
regenerated translation is a new version of the program, so it goes through
the same plan and review as any other change (`plan-evolution-plans.md`).

## 4 · Noticing other systems (g26)

On every apply, Ordo lists the tables and schemas it does not own and
records them in the plan as **"present, not Ordo's"**. Nothing is changed.
`ordo plan` prints the list, so a forgotten dependency stays visible.

## 5 · Names (g25)

The owner: *"a per tenant prefix should be standard requirement, probably
random."* Proposed:

- **Ruled 2026-10-01:** *"yes, but just prefixed with some random
  identifier."* Each database gets a schema named by a fixed prefix plus a
  random identifier, for example `ordo_k3f9`, instead of `public`. (Read as
  a schema name; if the owner meant table-name prefixes inside `public`, this
  line changes.) Ordo's tables live there; other systems
  keep `public` and their own schemas.
- `schema_apply::LIVE` stops being the constant `"public"` and becomes the
  tenant's schema.
- The deployment's own database gets the same treatment, separate from every
  tenant's (g30).

## 6 · Writes from outside (g27)

The owner ruled the direction: *"Direct DB access is discouraged. It seems
expensive to monitor integrity of data source. This should be handled at the
permission and access level."* So:

- Ordo's database role is the only role with write access to Ordo's schema.
  Other systems connect as roles with no grant on it.
- Detecting outside writes is not built. If a customer needs that level of
  assurance, it is addressed then.

## 7 · Letting another system read Ordo (g26)

**Ruled 2026-10-01: the door.** The other system calls ops over HTTP with a
capability token, like any client. Grant-based read-only views *"could be
possible, maybe"*: filed for later, not built.

## 8 · An audit owed first (g30)

The owner: the deployment lives in a different schema or cluster from the
tenants, and *"deserves an audit"*. Before any of the above: list what the
deployment's database holds, what each tenant database holds, which roles
own what, and where a std entity name could clash with a user's. Checklist
27l.

## 9 · Ruled 2026-10-01

1. External entities as generated, read-only projections: **yes**.
2. Ordo's tables under a prefix plus a random identifier: **yes**.
3. Other systems read through **the door**; grant-based views maybe, later.
4. **Not deferred.** The owner: *"they're independent. If there is an issue
   at first-apply Ordo will error and will also show existing untracked
   tables as well."* So the first apply against a database holding tables
   Ordo did not make lists them, and any clash is an error that names them.
