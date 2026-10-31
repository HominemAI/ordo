# Evolution plans: how a schema change is made, kept and undone

Status: **ruled in part, 2026-10-01** (board `evolution-plans` asks 1, 2, 4, 5; ask 3 open). From the owner's notes on the Grief Remediation tab

> **The command is `ordo evolve`** (ruled 2026-10-04, `syntax-additions` ask 1).
> Where this plan says `ordo plan`, read `ordo evolve`. `ordo plan` stays the
> compiler's artifact. At the end of a clean `ordo apply`, Ordo offers to
> remove `prev` lines that have landed in every environment.
(2026-10-01, rows g7, g8, g11, g16, g18, g19, g25, g39). Companion to
`plan-schema-evolution.md`, which this extends and does not replace.
Board: `evolution-plans`. Checklist 27a to 27d.

---

## 1 · How it works today

The owner asked (g7): *"How is the migration planned? How is the migration
generated? Does it happen at compile time?"* The answer, checked against the
code:

1. **`ordo apply` compiles the `.ordo` files.** Compiling produces an
   artifact: the ops, the shapes, and the DDL a fresh database would need.
   Its content hash is the version. Compiling never looks at a database.
2. **The plan is made at apply time, on the server.**
   `schema_apply::plan` reads three things: the version serving now, the
   live schema in Postgres, and the new artifact. `ordo_schema::diff`
   compares them and labels each difference A, B, C or D
   (`plan-schema-evolution.md` §5).
3. **Triage turns the labels into SQL or refusals.** Class A is sent.
   A class C constraint is sent when it cannot touch existing rows. Every
   other change is refused by name, with the increment that will handle it.
4. **The SQL runs, and a `$migrations` row records it**, whether it landed
   or not.
5. **Tenants follow.** `bring_forward` runs the same plan against each
   tenant's own database (25i).

So the migration is **generated and never written by hand**. It is made
fresh at each apply, and kept afterward only as a record of what was sent
(the `$migrations` row). Nobody can review it before it runs, and no
reverse is made with it. That is the root of most of the
owner's questions below.

### What "rollback" covers today (g8, g11)

| Kind | Today |
|---|---|
| Point back at the previous version | **Works.** `ordo rollback --version <hash>`. Old artifacts are all kept; a foreign key enforces it. |
| ...when the new version only added things | **Works.** The added columns stay in the database. The old version ignores them, and extra columns are not drift. |
| ...when the new version made something optional (B6) | **Breaks.** The old version expects NOT NULL, rows now hold nulls. Nothing stops this yet. |
| Undo the schema change itself ("devolution") | **Not built.** Applying the older program would be a drop, which is class D, increment 5. |

So yes: today rollback means "serve the old version again". It does not undo
the schema.

### Why only one version back (g7)

The owner remembered a reason but not what it was. It is ruling D-B2
(2026-09-21, board `db-lag-bound`): *"One version back + ability for
snapshots... and ensure step by step downgrading in schema."*

The bound is on **serving**, not on **keeping**:

- **Keeping old versions is cheap, and already done.** Every artifact ever
  applied stays in `$artifacts`. Every `.ordo` program is text in git.
- **Serving old versions is the expensive part.** A node that serves tenants
  on versions 6 and 11 at once runs two sets of ops, two gates and two sets
  of shapes, and every fix must be proved against both. The ruling bounded
  that to one version back.

## 2 · What the owner asked for

From the notes, in his words where they matter:

- **State to state, declarative** (g19). *"Generating change is important to
  stop schema drift and evolution issues."*
- **A system that flags each delta and offers options for each type** (g19).
- **Reverts kept in mind from the start** (g11, g19).
- **Render first, apply when ready** (g33). *"An apply compiles/renders and
  then after, it can be applied when ready."*
- **Keep migrations N versions back from the earliest tenant** (g7). If every
  tenant is on 11 and one is on 6, version 5 stays available to that tenant.
- **Plan for `lock_timeout`** (g16) and **a more efficient constraint check**
  (g18), because Ordo's tables are more complicated than most.
- **A failing tenant should revert** (g39), to be discussed.
- **Does the deployment support all active versions?** (g25).

## 3 · The proposal

### 3.1 The plan becomes a stored artifact

Split `ordo apply` into two steps:

```
ordo plan  <dir> --against <deployment>   compile, diff, render the plan, store it
ordo apply <plan-id>                      run a stored plan when ready
```

`ordo apply <dir>` stays as the two run back to back.

A stored plan holds, for one step from version X to version Y:

- every change, its class, and its probe results at render time;
- the **forward** statements;
- the **reverse** statements, or the reason there are none (`D-B2` already
  requires each step to state its down path);
- the **option chosen** for each change that needs one (3.2);
- the live shape it was rendered against, so a stale plan refuses to run.

It is content-addressed like an artifact, kept in `$plans` beside
`$artifacts`, and printable. The generator stays the only author: a person
chooses options, never edits SQL.

**Ruled 2026-10-01:** *"yes, errors are rejected."* A plan whose probes or
checks fail is refused at render time and never stored. And (ask 5) the
choices live in the plan, with *"at least a receipt that can be committed so
there's a code revision of the changes"*: `ordo plan` writes the plan's
receipt (each change, its class, the option chosen, the forward and reverse
statements) to a file beside the `.ordo` sources, for the repository.

### 3.2 Each delta offers options

Today a class B change is refused. Instead, the plan names the options and
waits for a choice:

| Change | Options offered |
|---|---|
| Rename a field (B4) | rename (copy, swap, drop) · treat as drop and add |
| Change a type (B3) | cast · map with an expression · refuse |
| Optional to required (B5) | fill nulls with a value · fill with an expression · refuse while nulls exist |
| New required field, no default (B2) | constant · expression · refuse |
| Add a unique (C1) with duplicates | refuse (default) · show me the duplicates |
| Drop a field (D1) | drop now · keep the column, stop serving it (contract later) |
| Change a money or decimal field's places (B11) | rescale · keep. **Built** (25aa) as `--places` on `ordo plan` and `ordo migrate --apply`, until the stored plan holds it |

A choice is recorded in the plan and travels to every tenant that runs it,
so every tenant gets the same answer.

### 3.3 Reverse is planned with forward

Each step's reverse is generated with its forward:

- **A1 to A4** reverse cleanly: drop what was added. Data in it is lost,
  and the plan says how many rows.
- **B6** (required to optional) reverses only when no nulls were written. The
  plan says so, and the probe checks at revert time.
- **D1** (drop) has no reverse once the contract runs. Before it runs, the
  column still exists and the reverse is "serve it again".

A revert is a plan from Y back to X, run like any other.

**Ruled 2026-10-01: dual writes, Stripe's pattern.** During a step's soak the
write path writes both shapes, so a revert switches back to a previous shape
that stayed current. This replaces increment 3's write freeze. A step with
no generatable reverse renders with a confirmation: *"accept this one way
change; revert requires this or manual intervention."* It is not rejected.

### 3.4 Keep every artifact and plan

**Ruled 2026-10-01** (ask 2): *"No need to get rid of artifacts at all
unless app dev wants to then."* Every artifact and every plan is kept. The
app developer may prune, as a setting; Ordo never prunes on its own. Given D-B2's step-by-step rule, a tenant on 6
reaches 11 by running 6 to 7, 7 to 8, and so on. It never diffs 6 against
11 directly, which is also the answer to row g7's open question: **mappings
belong to steps, and a lagging tenant replays the steps.**

### 3.5 Serving more than one version (g25)

Two ways, and this is the decision the board asks:

1. **Keep the ruling (one version back).** A tenant two or more behind is
   held closed until it is brought forward. Simple, and what 25i built.
2. **Serve every version any tenant is on.** No tenant is ever held closed
   for being behind. The node loads one artifact per live version. Cost:
   every op, guard and fix must hold for each version still served.

**Asked again 2026-10-01.** The owner: *"This is really up to the app dev"*,
and for 30 tenants on scattered versions among 400,000, *"what's the actual
harm?"* The costs, put on the board with the ask:

- **Small, and growing with the count:** one loaded program and one gate run
  per served version on each node; the interface served per version; a fix to
  the app's ops reaching a tenant only when it moves.
- **Paid once any old version is served:** every Ordo release must still load
  every served artifact, so the artifact format becomes a long-term promise;
  and sharing between tenants (feeds, windows, see-through) needs a rule when
  their versions differ on the shared entity.

New recommendation: **the app developer lists which versions stay served**
(default: every version a tenant is on). Ordo serves any version its binary
can load, and refuses sharing across disagreeing versions until a translation
exists. This replaces D-B2's bound, so it waits for the owner's ruling.
Custom builds for special tenants (beta testers, a promoted dev branch) are
the same mechanism, filed for after first deployment (`tenant-builds`).

### 3.6 Migration is seamless (g39)

**Ruled 2026-10-01** (ask 4): *"Migration should be seamless. Old version
should not stop serving until new version is functional. So rollback should
be possible but not required because failed migration would not impact
running tenant."*

Today a tenant that cannot be brought forward is held closed as
schema-behind. The ruling replaces that. For each tenant:

1. The old version keeps serving throughout.
2. The plan's expand step runs (additive, which the old version tolerates).
3. The new version is checked against the tenant's database (the shape gate).
4. Only then does the tenant switch to the new version.
5. If any step fails, the tenant never switched. It keeps serving the old
   version, and the deployment owner is told which tenant, which step and
   why. Nothing needs rolling back.

The owner flagged that this *"may require more in depth planning"*. The hard
part is a class B step, where the backfill's write freeze would interrupt
the old version; that design belongs to increment 3 and must meet this
rule.

### 3.6a What cannot be generated forward (board `forward-gaps`)

Ruled 2026-10-01, in `corpus-rulings.md`. In short:

- **Refuse, with the options in the error.** Decisions happen on paper.
- **Pre-checks run every check against every tenant** and report all issues
  at once, each with its Ordo-level remediation (27w).
- **`prev` declares renames and correspondence:** `entity b (prev a) {}`,
  `field x (prev c)`, one version back only (27x). Source or plan: ask 23.
- **Orphan instead of drop.** Undeclared fields and entities are kept and
  reported; pruning is explicit (27y). Revert to a version that used an
  orphaned field is then a switch, never a restore.
- **A change that would not compile fresh fails the migration the same way.**
  A tree's root rows simply have no parent; a tree with no parent ref at all
  is a compile error in both cases.
- **Design rule for the syntax:** few keywords, hard to hallucinate, the
  proof done underneath. Agents write Ordo too.

### 3.7 Locks (g16) and constraint checks (g18)

- **Every statement runs with `SET lock_timeout`** (default 2 seconds, a
  setting). A statement that cannot get its lock gives up and retries with
  backoff, up to a limit, then the plan reports where it stopped.
- **Constraints are added `NOT VALID`, then validated** (`VALIDATE
  CONSTRAINT` takes a weaker lock and lets writes continue). For uniques,
  the index is built `CONCURRENTLY` first and attached with `ADD CONSTRAINT
  ... USING INDEX`. The probes already tell us before we start whether
  validation will pass.
- **Per tenant, these run tenant by tenant**, so a slow tenant never blocks
  the others.

## 4 · What changes in the code

| Piece | Change |
|---|---|
| `schema_apply::plan` | Returns a `StoredPlan`, written to `$plans`. |
| `triage` | Takes the plan's chosen options instead of refusing class B. |
| `execute` | Sets `lock_timeout`; retries; reports the step it stopped on. |
| `bring_forward` | Runs stored plans step by step from the tenant's version. |
| `ordo plan` / `ordo apply` | Render and run as two steps. |
| Retention | A pruning pass keeps what 3.4 says, nothing older. |

## 5 · Open for the owner

On the board as `evolution-plans`:

1. ~~Render first, apply when ready~~ **Ruled yes; errors are rejected.**
2. ~~Keep from one before the oldest tenant~~ **Ruled: keep everything; the app developer may prune.**
3. **Open:** should the app developer decide which versions stay served, replacing D-B2?
4. ~~A failing tenant~~ **Ruled: seamless; the old version serves until the new one works.**
5. ~~Choices in the plan~~ **Ruled: in the plan, with a committable receipt.**
