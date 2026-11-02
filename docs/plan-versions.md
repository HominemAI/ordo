# Program versions: the core under schema evolution and environments

Status: **build plan, 2026-11-02.** Joins `plan-schema-evolution.md`,
`plan-evolution-plans.md` and `plan-environments.md` around what they share.
Boards: `evolution-plans`, `environments`, `forward-gaps`, `tenant-builds`,
`schema-evolution`.

The owner, 2026-11-02: *"Environments and this [schema evolution] I find very
important and partially intertwined because of versioning."*

Read this first:

- Schema evolution moves one tenant from version X to version Y.
- Environments move one version from dev to staging to production.
- Both need the same thing: a tenant pinned to a version, and a node that
  serves more than one version at once.
- So that core is built first, and the rest sits on it.

---

## 1 · The shared core

### 1.1 What a version is

A version is the hash `ordo_plan::artifact::program_version(doc, ddl)`. It
covers:

- the ops, as the byte-stable `ordo plan` document;
- the schema, as the DDL a fresh database needs;
- each function's code hash.

A change to ops alone is a new version with no schema change (ruled
2026-10-03).

### 1.2 Where versions are stored

| Table | Database | Holds |
|---|---|---|
| `$artifacts` | the deployment's own | version, plan document, **source**, DDL, who applied it. Never deleted from. |
| `$activations` | the deployment's own | each re-point of the deployment, append-only |
| `$migrations` | each tenant's own | what that database's schema was moved to (`spacedb::recorded_version`) |
| `$plans` | the deployment's own | **new (27a):** one stored step, X to Y, forward and reverse |

Every artifact is kept; the app developer may prune (27b, ruled 2026-10-01).

### 1.3 How a tenant is pinned today, and how it will be

Today there is no pin:

- `Deployment` holds one `RwLock<Arc<Loaded>>` (`deploy.rs`).
- Every tenant is served that one version.
- `Spaces::schema_behind` compares the tenant database's recorded version
  with it. A mismatch holds the tenant closed (`SpaceRefusal::SchemaBehind`).
- `tenant_forward::CatchUp` retries every 15 seconds.

The pin this plan adds:

- **`$spaces.version`**, in the control database: the version this tenant
  is served.
- It is separate from the tenant database's `$migrations` row, which says
  what shape the data is in.
- The two differ on purpose during a move. After the expand step the data
  holds both shapes, and the pin still says X until the switch.
- A new tenant is pinned to the active version (open decision 2).

### 1.4 How the door picks the version per request

1. The door binds the space, as now.
2. `Spaces::resolve` reads the tenant's pin from memory. Pins are loaded at
   boot and refreshed on a `NOTIFY`, the way pauses are (27ar).
3. `Deployment::loaded(version)` returns that version's `Arc<Loaded>`.
4. A version not yet loaded is compiled and gated on first use, against a
   tenant database that is at that version.
5. An in-flight request keeps its own `Arc`, as today.

The catalog and the generated interface are served per version, since a
client needs the ops of its tenant's version.

### 1.5 What serving many versions costs

| Cost | Size | When it is paid |
|---|---|---|
| Memory | one `Loaded` per served version per node: fixture, executor, specs, manifests, and one `SpaceView` per space. **Not measured.** Increment 3 measures it. | per version in use |
| Load | one compile plus one gate (a shadow schema build) per version per node | first request for that version, and after each Ordo release |
| Idle versions | evicted after a quiet period, loaded again on demand | only when a version has no traffic |
| Ordo releases | every release must compile every served version's **source** and pass its gate against the stored DDL | every release, once any old version is served |
| Fixes | a fix to the app's ops reaches a tenant only when it moves | the app developer's choice (owner, 2026-10-01) |

The release cost is the real one. A stored artifact is recompiled from its
source at every load (`verify_candidate`), so Ordo's promise is on the
compiler. Increment 6 checks it before an upgrade.

### 1.6 Which versions stay served

Ruled 2026-10-01 (`evolution-plans` ask 3, *"Yes they should"*): the app
developer decides. Proposed setting:

- `served_versions`, default `in-use`: every version some tenant is pinned
  to.
- A tenant pinned to a version not in the list is refused. The refusal names
  the setting and the value to add.
- This replaces D-B2's one-version-back bound (2026-09-21).

### 1.7 Sharing across versions

The owner: sharing between tenants on different versions *"would need some
sort of data agreement. If the schemas for the shared items is not
consistent it cannot be served through a normal means."*

- A shared read (feeds, windows, see-through) compares the shared entity's
  shape in both tenants' versions.
- Same shape: served as now.
- Different shape: refused as `versions_disagree`, naming both versions and
  the entity.
- The owner's exception, a migration rule for the shared subset, is a later
  item.

### 1.8 One interface (rule 9)

- **`Versions`**, one client in `ordo-host`, is the only reader and writer of
  `$artifacts`, `$activations`, `$plans` and the pins.
- It has one typed error, `VersionRefusal`. Each carries its cause and says
  whether the caller should retry, refuse, or fix input.
- One place at the edge turns it into an HTTP answer.
- Promotion between deployments goes through the door's client, never a
  direct database connection to another environment.

---

## 2 · Schema evolution on the core

### 2.1 A move is a pin switch

Per tenant, ruled seamless (27c):

1. The tenant serves version X throughout.
2. The step's expand runs on the tenant's database. X ignores the additions;
   extra columns are not drift.
3. A backfill fills the new shape, when the step has one.
4. Version Y is gated against that database.
5. The pin switches to Y.
6. Any failure before step 5 leaves the pin on X. The tenant stays open, and
   the deployment owner is told the tenant, the step and the reason.

This replaces 25i's hold-closed for every case where X is still served.

### 2.2 Writes during the move (dual writes)

Ruled 2026-10-01: Stripe's pattern replaces the write freeze.

- While X serves, a write must reach the new shape too, or the backfill
  misses it.
- After the switch, through the soak, Y also writes the old shape, so a
  revert is a pin switch back to X.
- Proposed mechanism: the stored plan's forward and reverse mappings run in
  the write path for tenants mid-move. Plaintext mappings may run as
  generated triggers. Sealed fields cannot be mapped in SQL and go through
  the host's write path.
- A step with no reverse renders with the confirmation *"accept this one way
  change; revert requires this or manual intervention."*
- The audit spine gets a summary per backfill, with full images as the app
  developer's per-backfill option (ruled 2026-09-08 and 2026-09-20).

### 2.3 Stored plans and receipts (27a, 27b)

- `ordo evolve` renders one step, X to Y, and stores it in `$plans`.
- It writes a committable receipt beside the `.ordo` sources: each change,
  its class, the option chosen, forward and reverse statements.
- A plan with errors is refused and never stored.
- `ordo apply` runs a stored plan.
- A tenant on 6 reaches 9 by replaying 6 to 7, 7 to 8, 8 to 9. It never
  diffs 6 against 9.
- Pre-checks run on every tenant and report every issue at once (27w).

### 2.4 Every tenant database, not one at a time

Today a transforming migration runs one database at a time with `--db`.
`bring_forward` sends class A only and refuses the rest. With the pin:

- the rollout (section 3.4) picks which tenants move;
- each tenant runs the stored step on its own database, 8 at a time
  (`FORWARD_CONCURRENCY`), under the apply lock;
- the deployment's answer lists moved, current and failed tenants.

With a database per tenant, each backfill touches one tenant.

---

## 3 · Environments on the core

### 3.1 Three deployments, one hash

Ruled 2026-10-31: dev, staging and production are three deployments, each
with its own secrets. Ruled 2026-10-08: each is served at a real address.

- `ordo apply` targets dev by default.
- Promotion to staging is refused until dev's `$activations` shows the hash.
  Production is refused until staging's does.
- The artifact travels by hash. The target recompiles the source and
  refuses if its own hash differs (27e makes that test meaningful).
- With stored plans, what staging ran is what production runs.

### 3.2 Dev branches are tenants on their own version

- `ordo apply --branch feature-x` stores the version in dev and pins a dev
  tenant named for the branch to it.
- Several branches are served at once. This is multi-version serving, dev
  first (ruled 2026-10-08, environments ask 6).
- A branch tenant is removed only by an explicit command.

### 3.3 Staging clones named production tenants

- The deployment owner names the tenants. Each is exported and imported
  through the bundle path and re-sealed under staging's keys.
- Personal data is masked by default; the app developer may turn it off
  (27v).
- The clone is pinned to the version its source tenant was on. Today import
  refuses any version difference (`ImportError::ArtifactMismatch`), so
  multi-version serving in staging is what makes this work without 11-10b.
- Staging then rehearses the stored plan on the clone.

### 3.4 Rollout is which pins move when

Ruled 2026-10-01 (27h). A strategy picks the order of pin switches:

| Strategy | Moves |
|---|---|
| rolling (default) | a batch every 5 minutes, all within an hour |
| all | every tenant at once |
| canary | a random n first, the rest after a wait |
| blue/green | a named group, then the rest when it is healthy |
| by query | the tenants a query selects, in its order |

A failed batch leaves its tenants on the old version and pauses the rollout.
Tenant builds (27t) pin named production tenants to a branch's version.

---

## 4 · Build order

Each increment ships alone. Sizes per AGENTS.md. A CI round is about 35
minutes for the full suite.

| # | Increment | Size | Needs | Done when | Closes |
|---|---|---|---|---|---|
| 1 | **Version identity.** Byte-stable compile test across macOS and Linux CI. `ordo dev` serves under the real hash, replacing its `dev-<sha>` label. | S | none | the same source gives the same hash on both CI platforms; dev's served version equals `ordo plan`'s | 27e |
| 2 | **`Versions` client and tenant pin.** `$spaces.version`, `VersionRefusal`, pins in memory. Every tenant pinned to the active version, so behaviour is unchanged. | M | 1 | pins written at tenant creation and activation; `std.deployment.versions` lists tenants per version; suite unchanged | none (core) |
| 3 | **Multi-version serving, dev first.** `Deployment` holds a map of versions, loads on first use, evicts idle ones. Per-version catalog. `served_versions`. Sharing rule. | L | 2 | dev serves 2 tenants on 2 versions at once; a cross-version share is refused naming both; memory and load time per version recorded here | evolution-plans ask 3 (build) |
| 4 | **Promotion by hash.** Three deployments; `ordo apply` to dev; staging and production gated on the step before. | M | 1 | staging refuses a hash dev has not activated; production refuses one staging has not | 27f |
| 5 | **Branch tenants.** `--branch`, explicit delete. | M | 3, 4 | 2 branches served side by side in dev; a merge deletes nothing | 27g (first half) |
| 6 | **Release check.** Before an upgrade, every served version compiles and gates under the new binary. | S | 3 | an upgrade with an uncompilable served version is stopped, naming it and its tenants | none |
| 7 | **Seamless additive move.** Expand, gate, switch pin. A failure keeps the tenant open on X. Replaces 25i's hold. | M | 3 | a refused change leaves the tenant serving X, and `/health/ready` and the deployment owner see why | 27c (additive) |
| 8 | **Stored plans and receipts.** `ordo evolve`, `$plans`, receipt file, reverse SQL, options, step replay, prune setting off. | L | 2 | a plan rendered in dev runs unchanged in staging; a tenant 3 behind replays 3 steps; the migration hint of 27an lands | 27a, 27b, 27an (hint) |
| 9 | **Pre-checks on every tenant.** All checks, all tenants, one report with remediation. | M | 8 | a plan over 3 tenants with 2 bad lists both issues with their fixes | 27w |
| 10 | **Rollout strategies.** The 5 strategies; env settings until 18ac lands. | M | 7 | rolling moves batches on its timer; a failed batch pauses the rollout | 27h (strategies) |
| 11 | **Transforming migration on every tenant.** Plaintext backfill per tenant with dual writes, verify, switch, soak, revert by switch. Replaces `--db` one at a time. | L | 7, 8, 10 | a rename on 3 tenants under live writes loses no write; a revert in the soak serves X with every write present | 13c (reshaped), 27c |
| 12 | **Staging clones with masking.** Named tenants, re-sealed, masked by default, pinned to their source version. | L | 3, 4 | a clone of a tenant on X serves in staging on X with no real personal data | 27g (second half), 27v |
| 13 | **Sealed backfills per tenant.** B7, B8, B10 through the worker. | L | 11 | sealing a plaintext field on 3 tenants under live writes verifies by count and hash | 13d |
| 14 | **Orphan, prune, and destructive commands that name their environment.** | M | 8 | a dropped field is orphaned and reported; pruning asks for the deployment's name in production | 27y, 13e, 27h (rest) |
| 15 | **`prev` in the language.** | M | 8, syntax design | a declared rename renders as a rename, not a drop and add | 27x |
| 16 | **Tenant builds.** A branch's version pinned to named production tenants. After first deployment. | S | 3, 5, 10 | a beta tenant serves the branch while the rest serve main | 27t |

What sets the pace:

- Increment 3 is research on memory: say 2 tries.
- Increment 11's dual-write mechanism is research: say 2 to 3 tries, with a
  rehearsal against a real tenant's data.
- Open decisions 1 and 4 gate increments 6 and 11.
- Increment 15 waits on the syntax design (`forward-gaps` ask 23).
- 27z (the `acl` security audit) is outside this plan.

---

## 5 · Open decisions

Each goes on the board as an ask.

1. **When a new Ordo release cannot load a version still served, what
   happens?**
   *Recommendation:* **Check before the upgrade, and close only those
   tenants if one slips through.** The check names each version and its
   tenants. At boot, every other tenant and the control plane keep serving,
   as a rewound tenant does today.

2. **Which version does a new tenant start on?**
   *Recommendation:* **The active version, with a setting to name another.**
   It matches today, and a beta signup page can then create tenants on a
   beta build without a second deployment.

3. **Should `prev` cleanup wait for every tenant to pass the rename, or only
   every environment?**
   *Recommendation:* **Every environment is enough.** The stored plan holds
   the rename, so a lagging tenant replays it without the `prev` line in the
   source.

4. **After a tenant switches, how long should its old shape keep being
   written before dual writes stop?**
   *Recommendation:* **7 days, a setting per plan.** Revert is a pin switch
   inside it. After it, the old columns are orphaned and kept, and a revert
   needs the step's reverse mapping run.

---

## 6 · Conflicts

1. **Ask 3 is ruled on the board and open everywhere else.** The board
   recorded *"Yes they should"* on 2026-10-01 (answer 295). `corpus-rulings.md`,
   `plan-evolution-plans.md` §3.5 and §5, the task's `where`, and 27t still
   call it open. D-B2 is not marked replaced.
2. **13c still describes the write freeze.** The checklist and
   `plan-schema-evolution.md` increment 3 build a barrier. The 2026-10-01
   ruling replaced the freeze with dual writes.
3. **`plan-schema-evolution.md` §6 assumes shared tables.** It says one set
   of tables holds every space and per-space shape versions are deferred.
   Every tenant has its own database (ruled 2026-09-25, 2026-10-06), so
   per-tenant versions are now the core.
4. **The code holds behind tenants closed.** 25i's `SchemaBehind` and the
   15-second catch-up are what 27c replaces. `plan-schema-evolution.md` §8.3
   reads as current behaviour.
5. **"The artifact format becomes a long-term promise"** (`plan-evolution-plans.md`
   §3.5). The code stores source and recompiles at every load, so the
   promise is on the compiler.
6. **`ordo dev` versions are not hashes.** `dev.rs` labels them `dev-` plus
   6 bytes of the source's SHA-256 and writes no `$artifacts` row. Promotion
   by hash needs the real version.
7. **`--staging` and `--prod` flags** (`plan-environments.md` §2.1) read
   like a mode inside one deployment. Ruled 2026-10-31: there is no `--env`
   flag, and each environment is its own deployment. The flags must name a
   target deployment.
