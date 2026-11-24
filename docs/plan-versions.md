# Program versions: the core under schema evolution and environments

Status: **build plan, revised 2026-11-02 after review.** The owner ruled the
plan's 4 asks and all 17 of the review's decisions on 2026-11-02. Asks 5 to
7, from building increment 1, were ruled 2026-11-03; ask 6 made the
declared `@` version the name (§1.1). Ask 8 was ruled 2026-11-03 (§5;
this line said open until the review of 2026-11-23). The rehearsal
design (§3.2) is provisional. Increment 1 built 2026-11-03. Increment 2,
session 1 (pins, `Versions`, `ordo versions list|show`) built 2026-11-03;
session 2's labels, commits and `ordo versions label|diff` built 2026-11-04.
Review: `review-plan-versions.md` (54 findings). Rulings:
`corpus-rulings.md`, "2026-11-02: program versions plan and its review".
Joins `plan-schema-evolution.md`, `plan-evolution-plans.md` and
`plan-environments.md` around what they share. Boards: `evolution-plans`,
`environments`, `forward-gaps`, `tenant-builds`, `schema-evolution`,
`program-versions-the-core-under-schema`, and the 3 review tasks
`versions-plan-review-*`.

**Reviewed 2026-11-23** against everything that landed after 2026-11-05.
The report is `review-evolution-plans-2026-11-23.md`. Each edit below is
marked "(review 2026-11-23)". Text marked **Proposed** is not ruled; its
open question is numbered Q1 to Q16 in that report. 2 plans were started:
`plan-backfill-dual-writes.md` (increments 10b, 11a, 11b) and
`plan-evolution-federation.md` (increment 7b).

The owner, 2026-11-02: *"Environments and this [schema evolution] I find very
important and partially intertwined because of versioning."*

Read this first:

- Schema evolution moves one tenant from version X to version Y.
- Environments move one version from dev to staging to production.
- Both need the same thing: a tenant pinned to a version, and a node that
  serves more than one version at once.
- So that core is built first, and the rest sits on it.
- Every number in this plan is a setting with a secure default (owner,
  2026-11-02). §7 lists them.

---

## 1 · The shared core

### 1.1 What a version is

Ruled 2026-11-02 (D1, D6, V-1). Revised 2026-11-03 by the owner (ask 6):
the developer's declared version names a version. Built 2026-11-03
(increment 1).

What runs is the app's source times the Ordo release that compiles it.
Each side owns its own changes:

| | Names or records | Decided by |
|---|---|---|
| **Version** | `domain notes @ 0.2.0`, as declared | the developer |
| **Build** | the compiled document and DDL, plus the Ordo release, one `$builds` row each | the Ordo release |
| **Source fingerprint** | full SHA-256 of the files as written; a check, never a name | promotion (§3.1) |

How an apply names a program (`schema_apply::name_version`):

- It compiles the new source and compares the compiled program with each
  stored version under the same number, newest first.
- When the stored build came from this Ordo release, the stored bytes are
  compared directly.
- When it came from another release, the stored source is compiled under
  this one first. Only the developer's changes then count, never Ordo's.
- A match is that version. Comments, spacing and blank lines never reach
  the compiled program, so a lint pass is not a release.
- No match under a new number is that number.
- No match under a used number is **refused**, naming the next number:
  one patch above the highest stored (owner, 2026-11-03, ask 13: "We
  need to block on version in the same file if the file has changed",
  formatting excepted). The source then always names what is applied.
  A change to a function's code alone is a real change too.
- New content under a number lower than the highest applied is refused
  too, naming the next number, so numbers rise in the order changes were
  made (ask 9). Going back to stored content is a match, not new content.
- Every stored version has an order number, `#1`, `#2`, ..., in the order
  it was first stored, never reused (ask 8). `ordo deploys` and the apply
  report show it beside the name: `#4 · 0.2.1`. Rows stored before are
  numbered by when they were applied.
- `ordo apply --bump` writes that number into the file and applies again,
  over the door as well (ask 12). A plain apply never edits files.
  `ordo dev` uses no registry and is never refused.
- A program with no `@` is named as before, by the 16-hex digest of what
  it compiles to (ask 8).
- A version stored before 2026-11-03 keeps its 16-hex name (ask 7). The
  registry fills in its declared version and fingerprint from its source.

Also:

- A change to ops alone is a new version with no schema change (ruled
  2026-10-03).
- A source with no edition line is edition 1, for good (ask 5). Later
  editions are named in the source (§2.6).
- **Settings** (D6): per-environment and runtime values sit outside every
  comparison and are bound at the target. Their declaration sits inside.
  Rollout settings are per environment.
- **One side moves at a time.** An apply that changes the program while an
  Ordo upgrade's release check is open cannot tell whose change is whose.
  The release check (§2.6, increment 6) reports a stored version whose
  source compiles differently under the new release, with its tenants.
- Command output prints the whole name, which `ordo rollback` takes.

### 1.2 Where versions are stored

| Table | Database | Holds |
|---|---|---|
| `$artifacts` | the deployment's own | name, declared version, source fingerprint, parent versions, source, sequence number, label, git commit, who applied it |
| `$builds` | the deployment's own | build identity per version: document, DDL, Ordo release, the minimum Ordo release that loads it |
| `$code` | the deployment's own | function code by code hash, under `Versions` |
| `$activations` | the deployment's own | each re-point of the deployment, append-only |
| `$revocations` | the deployment's own | **new (2b):** each revoke and unrevoke of a version: act, actor, reason, time. Append-only |
| `$pins` | the control database | **new:** each pin change: tenant, from, to, actor, cause, rollout id, time. Append-only, written in the pin's transaction, with a stream event |
| `$moves` | the control database | **new:** one open move per tenant: from, to, phase, soak deadline |
| `$migrations` | each tenant's own | the shape the data is in, and the set of versions that shape supports |
| `$plans` | the deployment's own | **new (27a):** one stored step, any edge X to Y, forward and reverse |
| `$orphans` | each tenant's own | **new:** columns a move left behind, with their personal or sealed flag and drop date |
| `$rollouts`, `$rollout_tenants` | the control database | **new (review 2026-11-23, §3.7):** one row per rollout, and one row per tenant in it with its state and reason |
| `$backfills` | each tenant's own | **new (review 2026-11-23, `plan-backfill-dual-writes.md` §2):** one cursor per step and entity, advanced in the chunk's own transaction |
| `$release_checks` | the control database | **new (review 2026-11-23, §2.8):** each release check and each override: Ordo release, result, actor, time |

- Every artifact is kept until the app developer prunes it (27b, ruled
  2026-10-01). Pruning follows §2.7 and leaves a tombstone.
- A `revoked` mark, set only by the deployment owner (D8, §1.11).

### 1.3 The pin

Today there is no pin:

- `Deployment` holds one `RwLock<Arc<Loaded>>` (`deploy.rs`).
- Every tenant is served that one version.
- `Spaces::schema_behind` compares the tenant database's recorded version
  with it. A mismatch holds the tenant closed (`SpaceRefusal::SchemaBehind`).
- `tenant_forward::CatchUp` retries every 15 seconds.

The pin this plan adds:

- **`$spaces.version`**, in the control database: the version this tenant
  is served. Only `Versions` writes it, for a deployment-owner principal.
- It is separate from the tenant's `$migrations`, which says what shape the
  data is in. The two differ on purpose during a move.
- `resolve` refuses a pin outside the tenant's supported set. A restore that
  puts the pin ahead of the data re-pins or holds with a named refusal.
- **Pin epoch.** Each tenant's pin carries an epoch. The write transaction
  checks it, so a node with a stale pin cannot write.
- **"Active"** means the version new tenants and rollouts target.

**New tenants** (ruled 2026-11-02, plan ask 2; D14):

- A new tenant starts on the last version that finished a rollout.
- `versions.new_tenant` names another, such as a beta build.
- No request picks its own version. A beta signup maps to that setting.
- "Active" flips when a rollout finishes. A running rollout re-reads its
  tenant set, so a tenant created mid-rollout is moved too.

**As built in increment 2, session 1** (2026-11-03, `ordo-host/src/versions.rs`):

- Pins are recorded, not served on. The door serves the one loaded
  version until increment 3.
- Rollouts do not exist, so the active version stands in for "last version
  that finished a rollout".
- Every activation (apply, `std.deployment.activate`, `ordo rollback`)
  re-pins every `provisioning`, `active` and `paused` tenant in the
  activation's transaction. A tenant `versions.new_tenant` started on a
  beta build moves too.
- A rollback's pins say `rollback`: the target was stored before the
  version it replaces.
- A new tenant is pinned right after its row is written, in its own
  transaction. A pin that fails is logged, and boot's backfill pins it.
- A space attached by hand with no `$spaces` row is not pinned.
- Every pin change takes one advisory lock (`PIN_LOCK_NS`) first, so a
  tenant created during an activation gets the version it makes active
  (review, 2026-11-04).
- An apply adds `$spaces`' columns only when the pin's are missing: the
  table's setup locks it exclusively once per column.
- Not yet built then: the pin epoch check in the write transaction and
  the stream event (built in increment 3, below), the principal check on
  who may pin, and the rollout id on `$pins` (increment 10).

**As built in increment 2, session 2** (2026-11-04, `versions.rs`,
`versions_diff.rs`):

- `$artifacts.label` and `$artifacts.git_commit`, both nullable, added
  idempotently like `ordinal`. Neither moves a version.
- A version is named by its name or its order number, `#4`
  (`Versions::resolve`). A name not stored is refused, naming every
  stored version.
- `ordo apply --label "<text>"` stores a label, on both paths. A re-apply
  of the same version with a label replaces it; with none, keeps it.
- A label is one line of at most 80 characters, trimmed. Anything else is
  refused before the apply starts, saying the limit.
- The CLI records the source's git commit: `git rev-parse HEAD` where the
  source lives, `-dirty` when `git status --porcelain` shows the program
  or its function code changed. No git or no repository: nothing.
- The first commit recorded for a version stays. A re-apply from another
  checkout does not change where the version came from.
- `ordo versions label <version> "<text>"` or `--clear`, locally or
  through `std.deployment.label`, deployment owner only, like activate.
- `ordo versions diff <a> <b>` shows the schema change and a unified
  source diff; `--schema` or `--source` shows one. Locally or through
  `std.deployment.diff`.
- The schema change is `ordo_schema::diff`, the differ `ordo plan` uses,
  run against `<a>`'s DDL built empty in a shadow namespace
  (`ordo_shadow_diff_<random>`), dropped after.
- The apply report, `ordo deploys`, `versions list|show` print label and
  a 12-character commit. `std.deployment.versions` carries `label` and
  `gitCommit`, camel case like its other keys.

**As built in increment 3, session 1** (2026-11-05, `versions.rs`,
`pin_stream.rs`, `deploy.rs`, `spaces.rs`, `ordo_exec`):

- **Where the epoch check reads** (ask 16's open point): a `$pin_fence`
  row per tenant in the tenant's own database, made with Ordo's other
  tables in every database a write reaches. The control database stays the
  pin's source. The fence is the newest epoch any node wrote under.
- Every write transaction on a tenant's handle reads the fence first. An
  older epoch than the fence is refused as `PoolError::PinMoved`
  (retryable) and writes nothing. A newer one raises the fence in the same
  transaction.
- **What the caller sees** (audit 2026-11-05): nothing. The door reads that
  tenant's pin from the control database again, resolves again and writes
  once more. A second refusal answers `server_busy`. A routine run reads the
  pin again and is retried by the runner.
- The door looked the op up, bound it and checked access under the old
  version. So the write is made again only when the new version declares
  the op the same way (its plan may differ). Otherwise it answers
  `server_busy`, nothing written, and the caller's retry passes the door
  under the new pin (review, 2026-11-05).
- A whole read of the pins is merged with the stream under one lock: per
  tenant the newer epoch wins, and the read position never moves back.
- **The pin stream** (ask 17): every pin change notifies `ordo_pins` in its
  own transaction. Nodes read `$pins` by `seq` from the last row they read.
  Every pin change holds the pin lock and draws `seq` under it, so rows
  commit in `seq` order. A node also reads every pin again every
  `pins.reconcile_interval` (`ORDO_PINS_RECONCILE_SECS`, 30 s).
- **One tenant moves on demand**: `Versions::pin`, cause `pin`, through
  `std.deployment.pin` and `ordo versions pin <tenant> <version>`. It is the
  seam every later single-tenant move calls (§1.11). It asks no
  reauth, of a system or a person, strict deployment or not (owner,
  2026-11-05: *"If they are logged in they proved their access"*). The
  role gate is unchanged: deployment owner only.
- `std.deployment.tenantVersion` and `ordo versions list --tenant <id>`:
  one tenant's version, epoch and pin history.
- `ordo versions list|show` say `active (new tenants)` and `serving N
  tenants`. The console says the same since session 2.
- Since 2026-11-05 `versions.new_tenant` is stored in Ordo (§7), so every
  node reads one value and the warning about nodes disagreeing on it is
  gone. Each node still writes the value it last read to `$nodes`, for
  `/health/deploy`.
- An activation still moves every tenant at once, a tenant pinned
  elsewhere included (until rollouts, increment 10).

**As built in increment 3, session 2** (2026-11-05, `deploy.rs`,
`provision.rs`, `pin_stream.rs`, `tenant_forward.rs`):

- A new tenant is provisioned with its own version's tables: the version
  it is pinned to (a resumed provision), or else the one
  `versions.new_tenant` names. Its database's `$migrations` names that
  version, so a beta that adds a table opens on its pin.
- Each node keeps the `versions.new_tenant` version loaded, whether or not
  a pin names it yet. A node that has not loaded it refuses the create,
  retryable, nothing written; the caller sees no version or setting, the
  log names both.
- The catch-up leaves a database at a version some tenant is pinned to, or
  at the `versions.new_tenant` version, as it is: it is on its pin, not
  behind.
- `ordo_exec::erase::rows_held` counts only tables the database has, so a
  version that declares a new table can be checked against the control
  database.

### 1.4 How the door picks the version per request

1. The door binds the space, as now.
2. `Spaces::resolve` reads the tenant's pin from memory. Pins load at boot,
   refresh on a `NOTIFY` (27ar), and reconcile on a timer and on listener
   reconnect.
3. `Deployment::loaded(version)` returns that version's `Arc<Loaded>`.
4. A load recompiles the source and recomputes both identities. A mismatch
   refuses the load (V-4).
5. Versions load at pin time and at boot. A lazy load is single-flight,
   capped, never started by an anonymous call, and kept for a minimum time.
6. On first open per node, a cheap shape fingerprint checks the tenant's
   database against the version.
7. An in-flight request keeps its own `Arc`, as today.

- The catalog and generated interface are served per version.
- A pin switch closes the tenant's streams with a reconnect signal.
- Refusals give version and setting detail to the deployment owner's token
  only. End users get a plain refusal.

**As built in increment 3, session 1** (2026-11-05):

- `Deployment` holds the active `Loaded` and a map of the others, by name,
  and every tenant's pin and epoch. `Deployment::for_space` answers the
  tenant's `Loaded`; `Spaces::resolve` uses it, so reads, writes, blobs and
  routines run under the tenant's version.
- The door looks an op up in the named tenant's version
  (`OpRegistry::get_in`). The access engine grants the ops of every served
  version, by name; an op both declare is pinned as the active one
  declares it.
- Steps 2 and 5: pins load at boot, refresh on the nudge, reconcile on a
  timer. Versions load at boot and when a pin first names one, one at a
  time, never from a request. A version no pin names is dropped at once
  (no minimum residency yet).
- A version beside the active one is gated in its shadow like any load.
  Its live comparison is the shape fingerprint (step 6), not the
  deployment database's drift.
- Step 6: on first open per node, a tenant pinned to a non-active version
  whose `$migrations` does not name it has its tables read and diffed
  against the version, with no prior. Anything missing closes the tenant,
  and `/health/ready` names it and the fix. Not checked: a column the
  version does not know that has no default.
- A tenant pinned to a version this node has not loaded answers
  `space_unavailable`, retryable, naming no version.
- Except the registry's active version: the deploy channel loads it, with
  its live check, and swaps. Until the swap lands, a tenant pinned to it is
  served the version it was pinned to before, under that pin's epoch, and
  that version stays loaded. A node that has swapped raises the fence, so
  from its first write this node's writes are refused (`server_busy`) until
  its own swap lands. A tenant this node held no earlier pin for answers
  `space_unavailable` until then (review, 2026-11-05). The node reads
  every pin just before it swaps, so the catch-up the swap starts sees the
  new pins and no tenant is served the old version from a stale pin.

**Load guards, built in the increment 3 remainder** (`deploy.rs`,
`pin_stream.rs`, `spaces.rs`; tests in `versions_background.rs`):

- `versions.min_residency` (stored in Ordo, default 10m): a version no
  pin names stays loaded until it has been loaded this long.
- `versions.idle_evict_after` (stored in Ordo, default 30m, or `off`): a
  version beside the active one that served nothing this long is
  unloaded. The pin stream does not load it again by itself. A signed-in
  request for one of its tenants wakes it and waits for the load, up to
  5 seconds (`WAKE_WAIT`), then is answered as ever. Past that it answers
  `server_busy` with `Retry-After`, which the SDK retries by itself. A
  request that meets the version while it loads waits too. No anonymous
  call wakes one (V-29), and none needs to: a signed-out call reaches
  only the deployment's own space, on the active version, which is never
  evicted. Kept: the active version, the one `versions.new_tenant`
  names, and everything during a swap. The op path waits; a function
  call or a file read meeting an evicted version still answers
  `space_unavailable` (a later change).
- `versions.max_concurrent_loads` (node, `ORDO_VERSIONS_MAX_CONCURRENT_LOADS`,
  default 1, 1 to 16): loads in one pass at once. One pass runs at a
  time, so a version never loads twice at once.
- `/health/deploy` lists `loading` per node: what is loading now, apart
  from `alsoServing`.
- Both stored guards are set with `ordo versions set min-residency |
  idle-evict-after <value>` or `std.deployment.setVersionSetting`. A value
  that is not a duration is refused, naming the setting and the command.

### 1.5 What serving many versions costs

| Cost | Size | When it is paid |
|---|---|---|
| Memory | one `Loaded` per served version per node, and one `SpaceView` per space per `Loaded`. Measured in increment 3 (below): a view is 67 to 222 KB, so 1,000 spaces cost 65 to 217 MB per version. Bounded by `versions.memory_budget` and view eviction. | per version in use |
| Load | one compile plus one gate per version per node | at pin time, at boot, and after each Ordo release |
| Idle versions | evicted after `versions.idle_evict_after` | only when a version has no traffic |
| Ordo releases | every reachable version's source compiles and gates under the new release (§2.6) | every release |
| Mixed fleets | during a rolling Ordo upgrade, nodes differ. A pin switch completes when every live node has Y loaded. | each upgrade |
| Fixes | a fix to the app's ops reaches a tenant only when it moves | the app developer's choice (owner, 2026-10-01) |

**Measured in the increment 3 remainder** (`tests/versions_memory.rs`,
release build, macOS, 2026-11-23; resident memory by `ps`, so the
figures are approximate):

| Program | Source | One `Loaded` | One view | 1,000 views | Cutting one view |
|---|---|---|---|---|---|
| teamspace | 4 KB | 3.6 MB (first, so it carries the process's warm-up) | 222 KB | 217 MB | 83 µs |
| shopfront | 5 KB | 0.3 MB | 132 KB | 129 MB | 115 µs |
| notes-v2 | 8 KB | 0.05 MB | 67 KB | 65 MB | 107 µs |

- A view clones the version's schema and registry, so it is the cost
  that grows with spaces. The `Loaded` itself is small.
- A view is cheap to cut again, about 0.1 ms. So views are evicted, and
  the schema is not yet shared by `Arc`: that changes the public type of
  `ordo_exec::Executor` and every test that builds one, and is left as a
  separate change while 2 other increments work in the same code.
- What was set from it:
  - `versions.memory_budget` (`ORDO_VERSIONS_MEMORY_BUDGET`, 512MB): a
    view counts as 256 KB, the largest rounded up. Each loaded version
    keeps its share, the least recently used evicted first, on every pin
    read (`PinStream::evict_views`). Views idle for
    `versions.idle_evict_after` go too. A view a request holds is never
    evicted.
  - `versions.max_loaded` (`ORDO_VERSIONS_MAX_LOADED`, 8): a load past it
    is refused, naming the setting, and that version's tenants stay closed
    on this node.
- Tries: 2. The first, a counting allocator, is refused by the
  workspace's `unsafe_code = "forbid"`; the second reads resident memory.

Rollout arithmetic at `backfill.concurrency` 8 and 1 second per tenant:

- 5,000 tenants: about 10 minutes.
- 400,000 tenants: about 14 hours.
- So `rollout.window` is a target, never a deadline. A rollout waits for
  its backfills to finish.

### 1.6 Which versions stay served

Ruled 2026-10-01 (`evolution-plans` ask 3): the app developer decides.
Ruled 2026-11-02 (D15).

- `versions.served`, default `in-use`: every version some tenant is pinned
  to.
- `versions.minimum`, default none. Tenants below it are moved in their
  maintenance window.
- Removing a version that still has tenants needs `--move-to`.
- A rollout or settings change that would strand a tenant is refused. The
  refusal names the setting and the value to add.
- This replaces D-B2's one-version-back bound (2026-09-21).

**As built in increment 3, session 2** (2026-11-05, `versions.rs`,
`revoked_moves.rs`, `runtime.rs`):

- `versions.served`: `in-use` (the default) or a comma list of names
  or `#n`. With a list, a pin, a move target, a `--move-to` and
  `versions.new_tenant` outside it are refused (`not_served`), naming the
  value to add. The active version is always served.
- `versions.minimum`: a name or `#n`. Nothing is pinned, activated
  or started on a version stored before it (`below_minimum`). Tenants on
  one move in their maintenance window, by the same sweep and the same
  targets as off a revoked version, and on demand.
- A change to either is refused when it is made: a name not stored, a
  served list that leaves out a version tenants are pinned to (naming the
  value to add, `ordo versions set served <list>,<version>`), and a
  minimum above the active version. Built as environment variables first,
  then moved into Ordo the same day (§7).
- Each `Versions` call reads the stored values, so the CLI's `--db` path
  and the door enforce the same ones.
- Not built: removing a stored version, so `--move-to` on removal waits
  for that command.

### 1.7 Sharing across versions

Ruled 2026-11-02 (D7).

- The data owner's version and guards decide every read of its rows.
- A shared read (feeds, windows, see-through) compares the shape of the
  fields the reading op reads. Additive changes keep sharing.
- Differing guards, acl, release or sealing on those fields refuse the read
  as `versions_disagree`, naming both versions and the entity.
- During a move, the older shape is served from the dual-written columns.
- A rollout option moves share partners together (§3.5).
- The owner's exception, a migration rule for the shared subset, is a later
  item.

**Not built in the increment 3 remainder** (2026-11-23). What was found:

- Consented feeds (the copy) are not built, so there is no feed to compare.
- A window (a guest's share) calls the owner's op in the owner's tenant,
  so the owner's version already decides the read.
- The gap is the pins. A share and a capability grant pin each op's
  manifest and guards, and the door's drift check compares against them.
  Both come from the access engine, which holds one manifest per op: the
  active version's (§1.4, session 1). So a share made in a tenant on X is
  pinned to the active version's shape, and the drift check for a tenant
  on X compares against the active version, not X.
- `versions_disagree` belongs there: the engine holds manifests per
  version, a grant records the version it was pinned under, and a call in
  a tenant on another version compares the fields read. That changes the
  access engine (`ordo-identity`) and is its own session.

### 1.8 One interface (rule 9)

- **`Versions`**, one client in `ordo-host`, is the only reader and writer of
  `$artifacts`, `$builds`, `$code`, `$activations`, `$plans`, `$pins`,
  `$moves` and the pins.
- It has one typed error, `VersionRefusal`. Each carries its cause and says
  whether the caller should retry, refuse, or fix input.
- One place at the edge turns it into an HTTP answer.
- Promotion between deployments goes through the door's client, never a
  direct database connection to another environment.
- Pin changes are also a stream any node can tail, so a second region needs
  no shared `NOTIFY`.
- **Every evolution command is a door op** (review 2026-11-23; owner,
  2026-11-05, "the CLI goes through the door"). `ordo evolve`, `ordo
  rollout ...`, `ordo rollback`, `ordo promote` and the pre-checks call
  `std.deployment.*` ops. None opens a database connection.
- The long work (a rollout, a backfill) runs in the host. The CLI starts it
  and reads its state. A closed terminal stops nothing.
- `--direct` stays only where it is ruled: the first apply, and `ordo
  rollback --direct` when no host will boot. Each use is recorded at the
  next boot. `ordo migrate --apply --direct` is removed (2026-11-23):
  `std.deployment.evolve` replaced it (increment 8, §6 conflict 9).

As built (2026-11-03): `Versions` owns `$artifacts`, `$builds`,
`$activations`, the pins, `$pins` and `$moves`, over `ordo_exec::registry`
as its SQL layer. `$code` stays with `functions.rs`.
`VersionRefusal::to_exec_error` is the one door mapping.

As built (2026-11-23, increment 8): `$plans` has its own client,
`evolve::Plans`, with one typed error, `EvolveRefusal`, which wraps
`VersionRefusal`. It sits beside `Versions` and not inside it, because
increment 7 was changing `versions.rs` at the same time. Folding it in is
a move of code, not of behaviour.

### 1.9 Clients across versions

During a rollout one front-end bundle meets 2 versions. Mobile builds cannot
switch.

- Every answer carries the tenant's version.
- An op the tenant's version lacks gets its own refusal, distinct from
  `access_denied` and `version_skew`.
- The SDK detects features and raises a "version changed" event.
- An idempotency key is honoured across one switch. The SDK documents how
  page cursors behave at a switch.
- Tenant settings show the tenant admin their version's label.
- **Device copies** (review 2026-11-23). The SDK's local replica
  (`clients/ordo-sdk/src/replica`) orders rows by log position. It records
  no program version, so a row of X's shape can outlive a move to Y.
  **Ruled (Q7, owner, 2026-11-23), built in increment 3, session 3:** on
  the feed's `version_changed` frame the sync manager drops the
  device copy of that tenant and fills it again. One rule, the same one
  the public cache uses on a deploy.
- **Built in the increment 3 remainder** (`router.rs`, `deploy.rs`,
  `revoked_moves.rs`; tests in `versions_background.rs`):
  - Every answer past the door carries `x-ordo-tenant-version`, the
    version serving the tenant. CORS exposes it.
  - `op_not_in_version` (409): an op another served version declares,
    called in a tenant whose version lacks it. Said only to a caller the
    door would let call it there; anyone else gets the unknown op's
    `access_denied`. It names no version. The SDK reads it as `restart`.
  - An idempotency key is honoured across one switch: on a payload
    mismatch the door tries the fingerprint under the op's hash in the
    tenant's previous version. A node keeps the op tables of the last 8
    versions it dropped for this. A node that never served the previous
    version answers `idempotency_mismatch`, as before.
  - `std.space.version`: a tenant's managers and owner read its version,
    order number, label and whether it is revoked.
  - Not built: the SDK's own "version changed" event on the header.
- Offline writes (`plan-offline.md`) are filed for after the first
  deployment. When built, a queued write replays under the tenant's
  version at replay time, and an op that version lacks gets the distinct
  refusal above.

### 1.10 Background work under pins

Routines, schedules, functions, mail and webhooks:

- Schedules are the union over served versions.
- Each run executes under its tenant's pinned `Loaded`.
- A pin switch hands the cursor over to Y.

As built in increment 3, session 1: each run resolves its tenant first and
runs the routine as the tenant's version declares it, named
`<routine>@<that version>`. A routine the tenant's version does not
declare is skipped and recorded `undeclared`.

Built in the increment 3 remainder (`routines.rs`, `workers.rs`):

- The schedules and listeners are the union over served versions,
  rebuilt when a version loads or drops (`LiveRoutines::switch_served`).
- Each tenant is drained and fired only what its own version declares. A
  tenant on a version this node has not loaded keeps its cursors still.
- Two versions declaring one routine share one schedule row. Where their
  timings differ, the active version's timing wins.
- The hand-over at a switch: a routine both versions declare keeps its
  cursor, so no change runs twice and none is missed. A routine the new
  version adds starts at the head. A listener the new version lacks has
  its cursor retired. A switch made while a node was down retires nothing
  on that node.
- Tests: `versions_background.rs`.

**Backend functions under a pin** (review 2026-11-23; functions are V1,
ruled 2026-10-31; `plan-backend-functions.md` §4.3):

- Code is stored by hash in `$code`, so each version keeps its own code and
  a rollback brings the old code back. This is built.
- A call at `/fn/<name>` runs the function the tenant's version declares,
  as `function:<name>@<that version>`. Built in increment 3, session 3:
  one table per loaded version, read when the version loads and dropped
  when it is no longer served (`HostFunctions::sync`). A function the
  tenant's version does not declare answers as an unknown one, whatever
  another version declares.
- A function's `uses` ops resolve in the same version as the function
  (built in session 3: `CallerDoor::spec` asks the tenant's version).
- A call in flight at a pin switch finishes on the version it started on.
  Its writes meet the pin fence like any write: refused as `PinMoved`, and
  made again only when the new version declares the op the same way.
- Secrets and egress hosts are bound at the target (D6). A version whose
  function names a secret the environment lacks answers `secret_missing`
  for that function. The promotion gate lists these before it moves
  anything (§3.1).
- A function named as a step's backfill (D16) is covered in
  `plan-backfill-dual-writes.md` §6.

### 1.11 Revocation and end of life

**Ruled 2026-11-02 (D8).** The owner first asked who decides a version is
flawed, how it is known, and whether Ordo acts without the developer's
consent. The answer:

- Only the deployment owner marks a version revoked:
  `ordo version revoke <v> --reason "..."`.
- Ordo never decides a version is flawed and acts on nothing before that
  mark. It only warns, for example when an Ordo release fixes a flaw an old
  version relies on, naming the versions and tenants affected.
- Warnings are visible in the Ordo console (owner, 2026-11-02).
- After the mark, its tenants move off it in their maintenance windows.
- A revoked version is never pinned, activated, rolled back to, promoted or
  given to new tenants.
- A report lists tenants on revoked versions.

End of life without a flaw goes through `versions.minimum` (§1.6).

**Ruled 2026-11-04 (asks 18 to 22, 24).** Revoking the active version is
refused unless the deployment owner names where tenants go. A revoke can be
undone, and both acts stay on record. Moves timed to a window wait for
increment 3, when each tenant is served its own pin. Maintenance windows
are built now: a range or a cron with a length, with a message, for every
tenant, only some, or all but some. A window is only a block of time; it
changes nothing by itself (ask 24). Windows are their own feature and know
nothing of versions.

**As built in increment 2b** (2026-11-04, `versions.rs`, `windows.rs`,
`deploy_ops.rs`, `window_ops.rs`):

- `ordo versions revoke <v> --reason "..." [--move-to <w>]`, `unrevoke <v>
  --reason "..."` and `revoked`, locally or through `std.deployment.revoke`,
  `unrevoke` and `revoked`, deployment owner only. A reason is 1 to 500
  characters.
- The mark is the newest `$revocations` row of the version. Every revoke
  and unrevoke takes the pin lock (`PIN_LOCK_NS`), so none interleaves
  with a pin change.
- Revoking the active version without `--move-to` is refused, naming the
  flag and every stored version not revoked. With it, the move passes
  `ordo rollback`'s load gate under the apply lock, then the activation,
  the pins and the revoke are one transaction. The pins' cause is
  `activate` or `rollback`, as any activation's is.
- `--move-to` with a version not active is refused: nothing would move.
- A revoked version is refused, naming the revoke and `ordo versions
  unrevoke`, as an activation, a rollback, an apply's activation, a
  `--move-to`, `versions.new_tenant` when it is set and at pin time, and any pin
  target (the backfill included). Class `version_revoked`, wire code
  `apply_refused`.
- A tenant on a version revoked while it was not active stays there:
  `ordo versions revoked` lists it, up to 1,000 per version with the full
  count.
- `std.deployment.versions` carries `revoked: {reason, actor, at}` per
  stored version. `ordo versions list` marks it; `show` lists every
  revoke and unrevoke.
- Windows: `$windows`, `$window_tenants`, `$window_skips` in the control
  database, behind `Windows` (one typed error, `WindowRefusal`).
  `$window_tenants` is an erasure attachment, so erasing a tenant removes
  it from every window. `ordo windows set|delete|skip|unskip|list`, and
  `std.deployment.windows`, `setWindow`, `deleteWindow`, `skipWindow`,
  `deleteSkip`. `howto-operating.md` has the rules and caps.
- Console (2026-11-04): the Deploys screen shows each version's `#n`,
  label, commit and revoke mark, revokes and unrevokes with a required
  reason, asks where to move when the version serves, and warns of
  revoked versions that still have tenants (D8). The Maintenance screen
  lists every time, upcoming and past, in local time or UTC, and adds,
  edits, deletes and skips windows. Door refusals show as given.
  `crates/ordo-console/src/server/{deploys,windows}.rs`,
  `clients/ordo-console-web/src/screens/{Deploys,Windows}.tsx`; tests in
  `crates/ordo-console/tests/door_link.rs` and the e2e `windows` project.
- `std.deployment.revoked` on a deployment with nothing applied answers
  an empty list. It was a refusal (`$artifacts` missing), found by the
  console's e2e run.
- Not built: moves at a window (increment 3), a time zone on a cron (a
  later extension).

**Ruled 2026-11-05: when and where a revoked version's tenants move.**

- No deadline. A tenant on a revoked version moves only in its maintenance
  window, or on demand. On demand is the deployment owner's act, or the
  tenant's own if the app developer grants it (off by default). No window
  and no demand: the tenant stays.
- The target is the app developer's choice: the active version by
  default, or any named version that is not revoked. Ordo picks nothing
  beyond that default.
- The owner: *"technically the developer could even have their own system
  managing automatic updates."* So the move on demand is a door op a
  system credential can call, with no reauth and no exemption
  setting. A person moving a version is not asked either (owner,
  2026-11-05: *"They could be moving many at a time"*).
- Where it lands: increment 3, session 2. Session 1 built the seam:
  `Versions::pin` is the one single-tenant move, `std.deployment.pin` its
  door op, which asks no reauth. The window-timed move and the tenant's own
  trigger call it with their own actor and cause.

**As built in increment 3, session 2** (2026-11-05, `versions.rs`,
`revoked_moves.rs`, `windows.rs`, `deploy_ops.rs`):

- One move: `Versions::move_off_revoked`, one transaction under the pin
  lock. It refuses a tenant not on a revoked version (`not_on_revoked`), so
  two nodes in the same window move a tenant once.
- The target, in this order: the tenant's own (`$move_targets`, set by
  `ordo versions target` and `std.deployment.setMoveTarget`), the revoke's
  `--move-to` (`$revocations.move_to`, now taken on a version that is not
  active), the active version. A revoked target is refused
  (`move_target_revoked`), naming `ordo versions target`. Ordo picks no
  other.
- In a window: every node sweeps on its pin reconcile tick. A tenant whose
  window holds now, not skipped, moves, cause `window`, actor
  `ordo:window:<id>` (`Windows::open_at`). At most 1,000 tenants a tick.
- On demand: `ordo versions move <tenant>` and
  `std.deployment.moveOffRevoked`, cause `demand`, deployment owner or a
  system's machine key, no reauth.
- By the tenant: `std.space.moveOffRevoked`, cause `tenant`, granted to the
  roles in `ORDO_VERSIONS_TENANT_MOVE_ROLES` (`versions.tenant_move_roles`),
  none by default. No reauth.
- `ordo versions revoked` and `std.deployment.revoked` show the revoke's
  `moveTo`, and each tenant's own `target` and `nextWindow`, or "no window".
- `$move_targets` is an erasure attachment.
- Tests: `crates/ordo-host/tests/versions_moves.rs`.

### 1.12 What is recorded where (review 2026-11-23)

New section. The plan named `$pins` and "the deployment's stream" and no
rule between them. `plan-opslog.md` §3.2 gives the rule: the deployment's
security record takes an act whose subject is the platform or a deployment
person. It is hash-chained, so it is not a place for bulk rows.

| Act | Recorded in | Status |
|---|---|---|
| Each pin change, with actor, cause and rollout id | `$pins` | built |
| Revoke, unrevoke | `$revocations` | built |
| A version setting changed | `$version_settings` | built |
| A `--direct` use | the security log, at the next boot | built |
| A plan stored by `ordo evolve` | `$plans`, with its actor; act `plan-stored` (and `plan-run`, `replayed`) on the deployment's stream | built (increment 8) |
| The deployment owner's approval of a sign-in method removal (Q6) | the deployment's stream, act `sign-in-removal-approved`, with the methods and the count | built (increment 9) |
| A backfill on one tenant | that tenant's command chain, one summarised group (ruled 2026-09-08, 2026-09-20) | increment 10b |
| A promotion, and a break-glass promotion | the deployment's stream (§3.1, §3.2) | increments 4b, 12b |
| Rollout start, pause, resume, skip, retry, abort, finish | `$rollouts`, and **ruled 2026-11-23 (Q9)** one entry each on the deployment's stream | increment 10 |
| A one-way confirmation, a prune, a `--move-to` | **ruled 2026-11-23 (Q9):** the deployment's stream, naming the person | increments 11a, 14 |
| A release check and its override | `$release_checks` | built (increment 6) |

- A rollout's per-tenant moves stay in `$pins` and `$rollout_tenants` only.
  5,000 chained entries per rollout would bury the record its reader
  sweeps.
- A new entry kind is appended, never inserted. An older binary reads a
  newer kind as `BadTag` (`plan-opslog.md` §0.10), so an Ordo release that
  adds one says so in its release notes.
- The console is incidental (owner, 2026-11-14). Every line it shows about
  a rollout or a refusal is also in the command's own answer and the log.

### 1.13 Sign-in under versions (review 2026-11-23)

New section. The `auth` block is in the definition file since PR 13
(`plan-configuration-in-the-file.md`). It is inside the source fingerprint,
so it is versioned and promoted with the program. The host does not read
it yet (step 1b).

- **Sign-in is one per deployment, never one per pin.** A person signs in
  before any tenant is known. `plan-configuration-in-the-file.md` §2.4
  builds one `AuthApp` from the active program and swaps it on activation.
- So a tenant pinned to X is signed in under the active version's `auth`
  block. A branch tenant's or a beta build's `auth` block changes nothing,
  and the apply report says so.
- **Ruled 2026-11-23 (Q5):** `AuthApp` follows "active" (§1.3). It swaps when a
  rollout finishes, and a rollback swaps it back.
- A swap ends no session (standard behaviour: people stay signed in). The
  relying party id is set on the instance alone (owner, 2026-11-14), so no
  version change can orphan a passkey.
- A version that removes a sign-in method can leave people with no way back
  in. **Ruled 2026-11-23 (Q6):** the pre-check counts them and `ordo
  evolve` stops on the warning. It goes on only with the deployment owner's
  explicit approval (a confirmation, recorded on the stream). The warning
  gives the migration steps: ask those people to add another method first,
  or keep the method one more version and tell them.
  **Built in increment 9:** the approval is `ordo evolve
  --approve-sign-in-removal <count>`. It must name the count the check
  finds now, so it never covers more people than it named.
- Ceilings and provider credentials are the instance's (owner, 2026-11-14).
  They are bound at the target like every per-environment value (§1.1, D6).
  A value the target's ceiling cuts is held at the ceiling, and the report
  names the ceiling.
- The `requires` check runs in the planning phase (owner, 2026-11-14, ask
  6). So it runs where a plan is rendered, at each promotion target (§3.1),
  and before a rollout starts. An unmet requirement refuses, and the live
  version keeps serving.
- Tenant and person layers keep their order: tenant, else file, else
  deployment. The file's value is the active version's.
- The identity service is its own binary with its own database (owner,
  2026-11-15). Its tables move with the Ordo release, never with a program
  version. Nothing in this plan migrates it.

### 1.14 The public path and the deployment's own space (review 2026-11-23)

New section. The public path serves the deployment's own space
(`design-public-path.md` §3). Its cache key holds a program version. On
every install of a version a node drops the whole public cache
(`public::after_deploy`).

- The version in the key is the version that serves the deployment's own
  space. Until rollouts that is the active version, as built.
- A tenant's pin switch changes no public answer, so it drops nothing.
  Loading a version beside the active one must not call `after_deploy`.
  Checked in increment 3, session 3: only the active version's swap calls
  it, and `versions_loaded.rs` tries a pin beside it.
- When the deployment's own space changes version, every node drops the
  public cache and the read layer rebuilds its entity index, as today.
- The CDN purge hook is called once for that change. An answer of the old
  shape must not outlive the move in a CDN.
- **Ruled 2026-11-23 (Q4):** the deployment's own space is a named step of a
  rollout and moves first.

### 1.15 Externals, peers and readers (review 2026-11-23)

New section. External entities (PR 14), the `peer` line (PR 22) and the
`reader` line (PR 24) landed after this plan was written. The full plan is
`plan-evolution-federation.md`. What this plan relies on:

- An external is not Ordo's table. The schema planner, DDL, `$orphans`,
  erase and prune never see it. `prev` on an external's field is refused
  (owner, 2026-11-15, ask 7).
- Its translation is in the source, so it is inside the version. Its live
  shape is not. The external check (`plan_with`) runs in every gate, per
  database.
- The schema an external or a peer resolves to is an instance setting
  (`ORDO_EXTERNAL_<SCHEMA>_SCHEMA`, `ORDO_PEER_<NAME>_SCHEMA`). It is bound
  at the target and sits outside the plan digest (§2.3).
- A `reader` line is a fact about the deployment, never about one tenant.
  Grants follow the active version's file, as built (`deploy.rs`).
- A pin protects a reader from nothing. The reader reads the other
  deployment's columns directly, whatever version either side is on.

---

## 2 · Schema evolution on the core

### 2.1 A move is a pin switch

Per tenant, ruled seamless (27c):

1. The tenant serves version X throughout.
2. The step's expand runs on the tenant's database. X ignores the additions;
   extra columns are not drift.
3. A backfill fills the new shape, when the step has one.
4. Version Y is gated against that database, and `$migrations` adds Y to
   the supported set.
5. The pin switches to Y, in one transaction with its `$pins` row.
6. Any failure before step 5 leaves the pin on X. The tenant stays open, and
   the deployment owner is told the tenant, the step and the reason.

This replaces 25i's hold-closed for every case where X is still served.

**Built 2026-11-23 (increment 7)** for additive steps, on a tenant with a
database of its own. A tenant on the deployment's own database moves with
the activation, since that database was gated first. The hold-closed stays
for a tenant already pinned to the active version whose database is
behind. Step 3 (backfill) is increment 10b.

Added by the review of 2026-11-23:

- **The gate in step 4 includes the external check.** It is in `plan_with`
  already. A tenant database that lacks a foreign table Y reads fails its
  gate, and the tenant stays on X.
- **Shared databases.** Tenants that share a database share its tables. The
  expand runs once per database, at its first tenant's move. The contract
  waits for its last tenant's soak. Constraints of the new shape cannot
  land while a tenant in that database still writes as X
  (`plan-backfill-dual-writes.md` §4).
- **A tenant's own database and bucket.** A database that cannot be reached
  leaves the tenant on X. **Ruled 2026-11-23 (Q10):** it counts as waiting, never
  as failed. Blob objects are keyed `<space>/<handle>`, so a renamed blob
  field moves no object in any bucket.
- **A version move and a database move never overlap.** Each refuses while
  the other is open on that tenant (`plan-space-routing.md` §9.7, C-M4).

### 2.2 Writes during the move (dual writes)

Ruled 2026-10-01: Stripe's pattern replaces the write freeze. Ruled
2026-11-02: D9, D10, D11, plan ask 4.

- While X serves, a write must reach the new shape too, or the backfill
  misses it.
- After the switch, through the soak, Y also writes the old shape, so a
  revert is a pin switch back to X.
- **Ordo's write path only** (D10). Dual writes run in the host, inside one
  command group. A mid-move write passes both versions' guards, and the
  audit chain sees it. No generated triggers.
- **Sealing steps** (D9) skip the plaintext dual write. The plaintext column
  is dropped once the seal verifies. Revert runs through the worker.
  Keeping the column is the opt-in `seal.keep_plaintext`, and its refusal
  names the risk: plaintext in WAL and backups cannot be undone (D6 of
  `plan-schema-evolution.md`).
- **Soak length** (ask 4): `soak.length`, default 7 days, per plan, 0
  allowed. After it, the old columns are orphaned and kept, and a revert
  needs the step's reverse mapping run.
- **One open soak per tenant** (D11). A new step may start during a soak.
  It closes the earlier soak after the one-way confirmation.
- A step with no reverse renders with the confirmation *"accept this one way
  change; revert requires this or manual intervention."*
- The audit spine gets a summary per backfill. Full images are the app
  developer's per-backfill option, `backfill.audit_images` (ruled 2026-09-08
  and 2026-09-20).

### 2.3 Stored plans and receipts (27a, 27b)

Ruled 2026-11-02: D3, D5, D16, plan ask 3.

- **Versions form a graph** with parent links (D5). `ordo evolve --from X
  --to Y` renders any edge and stores it in `$plans`.
- Replay takes a stored path. A tenant on 6 reaches 9 by 6 to 7, 7 to 8, 8
  to 9. It never diffs 6 against 9.
- **Each target re-renders** the step from X's and Y's stored source and the
  recorded options, and compares the plan digest with the rehearsed one
  (D3). A difference refuses.
- The receipt beside the `.ordo` sources is a record, never an input. It
  lists each change, its class, the option chosen, forward and reverse.
- Option expressions are Ordo expressions, never SQL.
- SQL for Ordo's own system tables is rendered at run time, so a stored
  plan does not freeze them.
- The plan stores the expected from-version. The stale check compares each
  tenant's recorded version and drift at run time. Render-time probes are
  advisory.
- A plan with errors is refused and never stored.
- **Backfill functions** (D16): a step may name a backend function by code
  hash, with an optional reverse, for changes the menu cannot express. The
  chunked worker runs it and the receipt records it. No raw SQL. The menu
  stays the default.
- **`prev` cleanup** (ask 3) waits for every environment to pass the rename.
  A lagging tenant replays the stored rename without the `prev` line.
- Pre-checks run on every tenant and report every issue at once (27w).
  **Built in increment 9** (`precheck.rs`): every database the deployment
  serves from is asked, shared or per tenant, and each issue names the
  database, the space and the fix. A database at another version is listed
  as not asked; its own step checks it when its path reaches it.

Added by the review of 2026-11-23:

- **`ordo evolve` is ruled** (2026-10-04, `syntax-additions` ask 1): it
  renders and stores the plan, and frees `ordo plan` for the compiler's
  output. It is a door op, `std.deployment.evolve` (§1.8).
- **`prev` is ruled into the source** (2026-10-04), spelled as
  `plan-syntax-additions.md` §5 has it. Increment 15 no longer waits on a
  syntax design.
- **The plan digest leaves out what an instance binds.** An external's
  resolved schema and its live shape differ between environments by design
  (owner, 2026-11-17). They sit outside the digest, like settings (D6).
  The translation in the source sits inside.
- **Pre-checks also run** the external check, the `requires` check (§1.13)
  and the reader check (`plan-evolution-federation.md` §3), and report
  them with the rest.

### 2.4 Every tenant database

Today a transforming migration runs one database at a time with `--db`.
`bring_forward` sends class A only, 8 at a time (`FORWARD_CONCURRENCY`),
each under a 20-second timeout (`FORWARD_TIMEOUT`). With the pin:

- the rollout (§3.5) picks which tenants move;
- a **resumable backfill worker** runs each tenant's step in chunks, with a
  cursor per tenant, under the apply lock;
- concurrency, chunk size, chunk timeout and budget are layered settings
  (§7);
- the deployment's answer lists moved, current, waiting and failed tenants.

With a database per tenant, each backfill touches one tenant.

The worker's design is `plan-backfill-dual-writes.md` (review 2026-11-23).
Before it, this section was the whole design of increments 10b and 11.

### 2.5 Rollback

Ruled 2026-11-02 (D12).

- `ordo rollback` moves every pin on Y back to X at once.
- `--dry-run` lists each tenant and what it loses past its soak.
- Grants a move killed stay killed after a revert.

### 2.6 Language editions and the release check

Ruled 2026-11-02: D4, plan ask 1.

- The source names its language edition.
- Ordo loads an old edition for `editions.load_window`, then refuses it with
  a refusal naming `ordo fix`.
- `ordo fix` rewrites old sources to the current edition.
- **Before an Ordo upgrade**, the release check compiles and gates every
  version reachable on a stored path, recomputes its identities, and loads
  every served function under the new runtime.
- It names each failing version and its tenants, and the tenants to move
  first.
- If one slips through, only the affected tenants close. Every other tenant
  and the control plane keep serving, as a rewound tenant does today.
- A recorded override closes only the named tenants, so a security upgrade
  is never blocked.

The design is §2.8 (review 2026-11-23).

### 2.7 Orphans and pruning

- Erasure and export read `$orphans`, so a left-behind column is never
  missed.
- Personal or sealed orphans get a drop date, `orphans.personal_drop_after`.
- Pruning refuses a pinned, restorable or retained version, and keeps a
  tombstone.
- `prune.enabled` is off in production and on in dev.
- A destructive command in production asks for the deployment's name.

Added by the review of 2026-11-23:

- Pruning is `ordo evolve --prune <field>`, a plan choice with a second
  confirmation typed at apply time (owner, 2026-10-03, `syntax-additions`
  ask 11). `plan-syntax-additions.md` §6 writes it as `ordo plan --prune`,
  from before `ordo evolve` was ruled.
- An external is never orphaned. Removing its declaration stops serving it.
- An orphaned column in a table a `reader` line names is no longer
  written, and the reader can still read it. That is stale data with no
  error. `plan-evolution-federation.md` §3 closes it (Q1).
- An orphaned `public` field stops being served at once, and the public
  cache drops (§1.14).

Built 2026-11-23 (increment 14):

- `$orphans`, one row per orphaned field or entity, in each database.
  Erasure, export, the reader grants and pruning read it.
- Every orphan gets a drop date, 30 days by default
  (`ORDO_ORPHANS_PERSONAL_DROP_AFTER`). The language cannot mark a field
  personal yet, so every orphan is treated as personal. The date is shown
  as due. Nothing drops by itself.
- `ordo evolve --prune <entity.field>` asks twice. In production it also
  asks for the deployment's name, the domain of the active version.
- A reader loses an orphaned column when it is orphaned, by a column grant.
  The soak's close (Q1) replaces that moment once soaks exist (11a).

### 2.8 The release check and editions: design (review 2026-11-23)

New section. Increments 6 and 6b had one table line each.

**What it is.** A check that every program version this deployment may
serve still compiles and loads under a new Ordo release, run before the
release serves anyone.

**Which versions are checked:**

- every version a tenant is pinned to;
- the `versions.new_tenant` version and every name in `versions.served`;
- every `--move-to` and move target;
- every version on a stored path between any 2 of those (§2.3).

**What is checked, per version:**

1. The stored source compiles under the new release.
2. What it compiles to equals what `name_version` expects (§1.1), so the
   release moved no version.
3. Its edition is inside `editions.load_window`.
4. Each function's code loads under the new runtime.
5. Its `auth` block is inside the new release's platform ranges (§1.13).
6. Each stored plan on its paths re-renders to its digest (§2.3).

**Where it runs.** Only the new binary can compile under the new release,
and the running host is the old one. There is one image (owner,
2026-11-09).

- **Ruled 2026-11-23 (Q8):** a mode of the new image, `ordo release-check`. It
  reads the control database and writes one `$release_checks` row. The
  deployment owner runs it before changing the image tag.
- It serves nothing and changes no pin. It is a `--direct` use and is
  recorded as one.
- The same check runs at every boot of a new release. There it only logs
  and closes the failing versions' tenants. It never stops the boot (owner,
  2026-11-06: an upgrade must not stop a running deployment).
- Its answer names each failing version, its tenants, and the tenants to
  move first.
- Peers and readers on another Ordo release are named too
  (`plan-evolution-federation.md` §5, Q16).

**Editions (6b).**

- `ordo fix` rewrites a source file on the developer's machine. It needs no
  door and no database.
- The fixed source is applied like any change. When it compiles to the
  same program it is the same version (§1.1), with the new edition
  recorded on its build.
- A version outside the window is refused at load, naming `ordo fix`. Its
  tenants close and every other tenant serves.

As built (increment 6b, 2026-11-23):

- The line is `edition <n>`, at the top level, once per program. No line
  is edition 1.
- What each edition means is a table in `ordo_plan::edition`, per Ordo
  release. Each edition after the first has a rule and a rewrite.
- The window is measured against the release's own date
  (`RELEASE_DATE`, set with the version), never the clock. A running
  deployment never stops loading a version as time passes. Only a newer
  release drops an edition, and its release check names the versions
  first.
- Only edition 1 is released. Edition 2 is written: bare `money` is
  refused and `ordo fix` writes `money(0)`. A debug build's
  `ORDO_TEST_EDITION_2_SINCE` makes it current, and
  `ORDO_TEST_RELEASE_DATE` dates the build.
- The gate is in the one compile, so a load, an apply, `ordo check` and
  `ordo dev` refuse alike: `edition_unknown`, `edition_expired` and
  `edition_rule`, each naming its fix.
- The applied rewrite moves the stored row's source, edition and
  fingerprint (`Applied::EditionMoved`). Its name, build and tenants stay.

**Build steps:**

| Step | Size | What |
|---|---|---|
| R1 | M | the checked set, compile and compare, the report, `$release_checks`, the `ordo release-check` mode |
| R2 | S | functions under the new runtime, `auth` ranges, plan digests |
| R3 | S | the boot-time run: log, close only the failing tenants, the recorded override |
| R4 | M | editions: the load window, the refusal, `ordo fix` (increment 6b) |

**Tests:**

- Host: a stored version made uncompilable by a test-only release flag is
  named with its tenants, and the other tenants serve after boot.
- Through an example (owner, 2026-11-23): the notes upgrade test
  (`examples/notes/e2e/tests/upgrade/v1-to-v2.spec.ts`) gains a case that starts the previous release's image, applies notes v2,
  runs `ordo release-check` from the new image, swaps, and expects every
  note still there. This is the "live data survives an upgrade" rule
  (owner, 2026-10-19) as a test.

**What sets the pace:** 2 CI rounds for R1, and one rehearsal on the `ordo`
test instance.

**Built 2026-11-23 (increment 6, R1 to R3).** See §4, row 6. Decided
conservatively, and asked:

- `ordo release-check` needs `--direct`, like every command that opens the
  database, and the refusal names it.
- The override does not change what the boot does: the boot already closes
  only the failing tenants. It is the recorded decision that lets a deploy
  pipeline go on, since the command exits 1 on a failure.
- The boot goes on past a compile, spec or shape refusal of the active
  version. A tampered row, live drift and an external's setting still refuse
  the boot, naming their fix, as before.
- The shape gate and the plan re-render build a shadow namespace and drop
  it, as a load does. No stored row changes.

### 2.9 Restore and import across versions: design (review 2026-11-23)

New section. §1.3 says a restore that puts the pin ahead of the data
"re-pins or holds". No increment carried it. The board task
`snapshot-restore` (checklist 11-10b) waits on the same thing: `import.rs`
refuses any version difference as `ImportError::ArtifactMismatch`.

**What it is.** A tenant's snapshot taken on X restores into a deployment
whose other tenants are on Y, and serves at once.

**Design:**

- The snapshot names its version and carries its `$migrations`,
  `$backfills` and `$orphans`, which live in the tenant's own database.
- X stored, not revoked, not below `versions.minimum`: the restore pins the
  tenant to X in the restore's transaction, with cause `restore` and a new
  epoch. The tenant serves X and moves to Y like any tenant behind.
- X not stored here: refused, naming the version. A staging clone carries
  its version's source with it (§3.4); a plain restore does not.
- X revoked or below the minimum: the restore succeeds and the tenant
  waits, closed, for its move. The answer names `ordo versions move`.
- A restore in the middle of a move: the tenant's own `$backfills` cursor
  wins over `$moves`, and the move resumes from it.
- `ImportError::ArtifactMismatch` stays for one case: a snapshot newer
  than every stored version.

**Build steps:** V1 (M): pin on restore, the 3 refusals, the resumed move.
V2 (S): the same for `ordo import` of an exit bundle.

**Tests:** host, one per case above. Through an example: notes takes a
snapshot on v2, moves to v3, restores, and the page shows the v2 notes
under the v2 shape, then moves again.

**Ruled 2026-11-23 (Q13):** a restore may pin a tenant below
`versions.minimum`. The tenant waits closed for its move.

---

## 3 · Environments on the core

### 3.1 A declared chain, one source identity

Ruled 2026-10-31: each environment is its own deployment with its own
secrets. Ruled 2026-10-08: each is served at a real address. Ruled
2026-11-02: D1, D3, D6, D13.

- **The chain is declared**, `promotion.chain`, default dev, staging,
  production. Skipping staging is `promotion.skip_staging`, off by default,
  named in the refusal.
- Environments are registered in config. Today's live deployment is marked
  production. A missing environment is refused with the command that
  creates it.
- `ordo apply` targets dev by default. **Built 2026-11-24 (owner, ask 5):**
  dev is the first step of the chain. A deployment that registered nothing
  is its own first step, so a single deployment applies as before. A later
  step refuses an apply with no `--env`, naming the first step's address
  and `ordo promote`.
- **The target enforces the gate.** It fetches the source from the step
  before, never from the caller's tree. It recompiles, recomputes the
  source identity, and refuses on a difference, naming the differing file.
- It re-renders each stored plan and compares the plan digest (§2.3).
- Per-environment and runtime values are bound at the target (§1.1).
- A production pin accepts only a version that arrived through the gate.
- Promotion is an act by a named deployment owner, recorded on the
  deployment's stream (`plan-environments.md` §6).

Added by the review of 2026-11-23:

- **`ordo promote` goes through the target's door** (§1.8). The target
  fetches the source through the door of the step before. **Built
  2026-11-24:** each environment records its door (address, its own space,
  its handle at the identity service) on the next step. The CLI asks the
  person's sign-in for a token for the step before's handle, or takes
  `--from-token`, and hands it to the target, which reads `versionSource`
  there with it. The step before answers who it read; the target refuses a
  different person. A staging deployment is gated as production is (owner,
  2026-11-24, ask 4).
- **Ruled 2026-11-24 (Q11):** no key to set up. The owner
  found a machine key a nuisance (*"Having to provide a key sounds like
  work"*), with GitHub-driven promotion coming. So staging reads dev's
  version as the person promoting, who must be an admin in both and is
  one subject at the identity service. Later a GitHub workflow does the
  same with GitHub's own token.
- **The gate runs the target's own checks before it stores anything:** the
  `requires` check (§1.13), each function's secrets (§1.10), and the
  external and reader checks (§1.15). Each refusal names the setting to
  set on the target.
- The `auth` block arrives inside the version. The target's ceilings may
  cut it, and the report says where.
- With the identity service, each environment is a deployment registered
  with it. The person who promotes is named by that service's subject, the
  same in every environment.

### 3.2 Rehearsal and attestation (provisional)

**Provisional.** Ruled 2026-11-02 (D2), with the owner's note: *"document
this explicitly - it may likely need to be revised in the future."*

The design:

1. Staging runs the stored plan on a clone of named production tenants
   (§3.4).
2. When it runs green, staging signs an attestation: source identity, plan
   digest, build identity, Ordo release, the rehearsal result and the
   clone's tenants.
3. A named deployment owner promotes. Production checks the signature,
   re-renders, and compares both digests.
4. Production warns or refuses on a different Ordo release, per
   `promotion.release_mismatch` (default refuse).
5. An attestation older than `promotion.attestation_max_age` is refused.
6. Break-glass: when staging is down, a named deployment owner may promote
   with a recorded reason. The stream marks it.

What it proves:

- The exact source and plan production runs ran green on real-shaped data.
- Under the same Ordo release.
- Who promoted it, and when.

What it does not prove:

- That production tenants not cloned behave the same. Per-tenant pre-checks
  (27w) cover that.
- That production's data has not changed since the rehearsal.

What may change:

- The signing key and who holds it.
- What the attestation names, and its age limit.
- Whether a clone is required, or a dry run on production suffices for
  some step classes.
- The break-glass path.

### 3.3 Dev branches are tenants on their own version

- `ordo apply --branch feature-x` stores the version in dev and pins a dev
  tenant named for the branch to it.
- Branch names map to tenant names strictly. A collision is refused. A seed
  step fills a new branch tenant.
- Several branches are served at once (ruled 2026-10-08, environments ask
  6).
- `ordo dev` saves are ephemeral. Only `apply` stores a version.
- A branch tenant is removed only by an explicit command. Branches unused
  for `branch.idle_list_after` are listed.
- A branch's own additions not in main are kept unused on return to main,
  never dropped silently (D5).

### 3.4 Staging clones named production tenants

Ruled 2026-11-02 (D17).

- The deployment owner names the tenants.
- Masking runs **inside production, before export**, and denies by default:
  every field not marked safe is masked and blobs are dropped.
  `clone.mask` may name safe fields; turning masking off is an opt-in (27v).
- The bundle is encrypted to staging's key and re-sealed under staging's
  keys on import.
- Credentials, passkeys, sessions and invites are never copied.
- In staging, mail, push and egress go to sinks.
- The clone is pinned to its source tenant's version. A version staging
  never saw comes down with the clone, recompiled, gated and recorded as a
  clone.
- Staging then rehearses the stored plan on the clone (§3.2).

Added by the review of 2026-11-23:

- **A tenant with its own database or bucket.** The clone lands in
  staging's storage, never in the tenant's. No production database or
  bucket credential is copied.
- **Foreign tables are not cloned.** They are not Ordo's. Staging needs its
  own copy of each foreign table and its own peer, named by staging's
  settings. Without them the external check fails the rehearsal, naming
  the table.
- **People.** With the identity service, a person's sign-in lives outside
  the deployment. A clone carries members and no sign-in link. **Ruled
  2026-11-23 (Q12):** staging registers with the identity service as its
  own deployment. A cloned tenant is opened by any staging deployment admin
  or a person the staging deployment allows. Cloned members' own sign-ins
  are never carried.

**Built, session 1 (2026-11-24, increment 12).** `crates/ordo-host/src/clones.rs`:

1. On staging, `ordo clones clone <tenant> --reason "..."`
   (`std.deployment.cloneIn`) asks production's door for the tenant as the
   person, with their own token for production (`step_door.rs`,
   `clone_out`). Production must answer as `production` and for the same
   person.
2. Production (`std.deployment.cloneOut`) exports the tenant at its pinned
   version and masks it there (`mask_bundle`). Masked values keep their
   type: text becomes `masked-<tag>`, unique per row; numbers 0; dates
   2000-01-01; a JSON document keeps its shape with every leaf replaced;
   an optional field null. Refs keep their ids.
3. Always dropped, masking off included: files, the command log (its
   images hold the old values), each member's name and sign-in link.
4. The bundle is sealed to staging's clone key (`ordo_crypto::recipient`:
   X25519, then XChaCha20-Poly1305, bound to the tenant, the source
   identity and the environment). Staging prints its key with `ordo clones
   key`; production records it with `ordo clones set-key staging <key>`.
   The secret is sealed under staging's own keys.
5. Staging finds the version it holds with the same source identity, makes
   a new tenant pinned to it, writes the members with principal
   `$clone:<id>`, and imports the rows through the write protocol, so every
   value is sealed again under staging's keys. Recorded in `$clones` and on
   both deployments' streams.
6. Q12: a staging deployment admin opens a clone's app ops as its founding
   owner, recorded as the agent beside it. `clone.readers` names others
   signed in to staging. The door never maps a `$clone:` principal, so a
   cloned member cannot sign in.
7. Foreign tables are not exported, and the answer names them.

`clone.mask` and `clone.readers` are stored in `$version_settings` (Q15),
set with `ordo clones set mask|readers <value>`.

Not built yet:

- Sinks for mail, push and egress in staging.
- A version staging does not hold coming down with the clone. It is
  refused, naming `ordo promote <version>`.
- A tenant with its own database or bucket: the clone lands in staging's
  shared database, and no production credential is carried, but its
  attachment is not recreated.
- The clone travels in one answer, at most 64 MiB.
- The notes example test.

### 3.5 Rollout is which pins move when

Ruled 2026-10-01 (27h). Ruled 2026-11-02 (D15). The maintenance window was
ruled 2026-09-21.

| Strategy | Moves |
|---|---|
| rolling (default) | a batch every `rollout.batch_interval`, aiming to finish within `rollout.window` |
| all | every tenant at once |
| canary | `rollout.canary_count` first, the rest after `rollout.canary_wait` |
| group first | a named group, then the rest when it is healthy |
| by query | the tenants a query selects, in its order |

- **Maintenance window and hold.** Every strategy respects a tenant's
  window and hold flag. A tenant outside its window is listed as waiting.
- **By query** is an Ordo expression over a tenant-facts view tenants
  cannot edit, run read-only.
- **Share partners** may move together, as a rollout option.
- **Healthy** means the gate passed and the error rate stays under
  `rollout.health_error_rate`.
- **Failure.** `rollout.failure_threshold` failed tenants pause the
  rollout. Failed tenants stay on the old version.
- **Told** means an event on `rollout.alert_channel`, default the
  deployment's stream.
- **Controls:** `ordo rollout status|resume|skip|retry|abort`. `ordo apply`
  returns when the rollout starts.
- Tenant builds (27t) pin named production tenants to a version that came
  through the gate.

The data model and the states are §3.7 (review 2026-11-23).

### 3.6 Blue-green and other strategies: a V2 candidate

The owner, 2026-11-02: blue-green and more elaborate strategies are to be
considered, possibly for V2. Not in the V1 build order.

How blue-green would sit on pins:

- Green is a second set of nodes with Y loaded, beside blue serving X.
- The switch is a pin switch for every tenant at once, plus a traffic shift
  at the edge.
- Revert is the shift back while the soak's dual writes hold.
- It needs per-node version sets and edge routing, which V1 does not have.

Other candidates: rollouts weighted by tenant traffic, and automatic
rollback on a health signal.

### 3.7 Rollouts: design (review 2026-11-23)

New section. §3.5 lists the strategies and the controls. It had no data
model, no states and no word on what an apply does once rollouts exist.

**What changes for `ordo apply`.** Today an activation re-pins every
tenant at once (§1.3). From increment 10 an apply stores the version and
starts a rollout with the environment's `rollout.strategy`. The strategy
`all` is today's behaviour. Dev defaults to `all`.

**Data.** In the control database, behind `Versions`:

- `$rollouts`: id, target version, strategy and its options, state, who
  started it, when, the counts.
- `$rollout_tenants`: rollout id, tenant, state, the reason, the time.
- `$pins.rollout` already has its column (§1.3).

**A rollout's states:** `running`, `paused`, `finished`, `aborted`.

**A tenant's states in a rollout:**

| State | Means |
|---|---|
| `queued` | in a later batch |
| `waiting` | outside its maintenance window, on hold, or its database cannot be reached (Q10) |
| `moving` | its move is open (§2.1) |
| `moved` | pinned to the target |
| `failed` | its gate, pre-check or backfill refused; it serves the old version |
| `skipped` | the deployment owner skipped it |

**The worker.**

- One node leads a rollout at a time, under an advisory lock in the
  control database. Another node takes over when the lock is free.
- Each tick it reads the tenant set again, so a tenant created mid-rollout
  is in it (§1.3).
- A move is `Versions::pin` for an additive step, or the move in
  `plan-backfill-dual-writes.md` for a transforming one.
- "Active" flips when no tenant is `queued`, `waiting` or `moving`, or
  when the deployment owner finishes it by hand with the rest skipped.
- **Ruled 2026-11-23 (Q3):** one open deployment-wide rollout per
  deployment. A second is refused, naming `ordo rollout status` and `ordo
  rollout abort`. One tenant may always be moved on its own to a hotfix
  version, during a rollout or not, and the rollout then skips that tenant.
- `abort` stops new moves. Moved tenants stay. `ordo rollback` is what
  moves them back.

**The deployment's own space** is a tenant set of 1 that moves first
(§1.14, Q4).

**Door ops:** `std.deployment.rollout.start|status|resume|skip|retry|
abort|finish`. Deployment owner, or a machine key with the `deployment`
grant. No reauth (owner, 2026-11-05).

**Split (see §4).** Increment 10 is the core: `$rollouts`, the worker,
`rolling` and `all`, windows and holds, the controls, the failure
threshold. Increment 10c adds canary, group first, by query, share
partners, health by error rate, the alert channel.

**Tests:**

- Host: a failed batch pauses; a tenant outside its window waits; the
  leader dies and another node finishes; a tenant made mid-rollout moves.
- Through an example: teamspace, with 3 tenants. The test applies the
  next version with `rolling`, clicks through a tenant that has moved and
  one that has not, and expects each to show its own version's page.

**What sets the pace:** the owner's answers to Q3 and Q4, and 2 CI rounds.

---

## 4 · Build order

Each increment ships alone. Sizes per AGENTS.md: S is part of a session, M
about 1 session and 1 to 2 CI rounds, L 2 to 3 sessions with a review. A CI
round is about 35 minutes for the full suite.

| # | Increment | Size | Needs | Done when | Closes |
|---|---|---|---|---|---|
| 1 | **Version identity.** Full SHA-256 source identity, build identity recorded apart, edition field, settings outside the hash. Byte-stable across macOS and Linux CI. `ordo dev` serves under the real hash. | M | none | same source gives same digest on both platforms; an Ordo-only change moves no version | 27e |
| 2 | **`Versions` client and pins.** `$spaces.version`, `$pins`, `$moves`, pin epoch, `VersionRefusal`, labels and `ordo versions list\|show\|diff`, "active" and new-tenant rule. Every tenant pinned to the active version. Session 1 built 2026-11-03: pins, `$pins`, `$moves`, `VersionRefusal`, new-tenant rule, `list\|show`. Session 2 built 2026-11-04: labels, git commits, `#n` names, `label\|diff` locally and over the door. Built. The epoch check and the pin stream moved to increment 3 (asks 16 and 17, 2026-11-04). | L | 1 | pins written with history; `std.deployment.versions` lists tenants per version; suite unchanged | none (core) |
| 2b | **Revocation and maintenance windows.** Owner-set mark with reason, unrevoke, `--move-to` for the active version, the report of tenants on revoked versions (D8, asks 18 to 20). Per-tenant maintenance windows: range or cron, all, only or except, skips, the 24-month list (asks 22, 24). Built 2026-11-04: backend, door, CLI, and the console's screens and warnings. Moves at a window are increment 3 (ask 21). | M | 2 | a revoked version cannot be pinned, activated or given to new tenants; windows are set, skipped and listed the same through the door and on the host | none |
| 3 | **Multi-version serving, dev first.** Pin epoch checked on write and the pin stream nodes tail (from 2, asks 16 and 17). Map of versions, load guards, digest at load, shape fingerprint, per-version catalog, served and minimum versions, sharing rule, client contract, background work, stream close, per-node health. Moves off a revoked version in a window or on demand (ruled 2026-11-05, §1.11). Session 1 built 2026-11-05: the fence, the stream, the map, per-tenant lookup and access, the shape fingerprint, runs per pin, `pin` and `tenantVersion`, the `new_tenant` check (§1.3, §1.4, §1.10). The remainder, 2026-11-23 (PR 40): schedules and listeners per version and the cursor hand-over (bf5e6b16), the client contract (3f7171dc), load guards and `loading` health (f40bed05), UTC times and fixed window caps (d8f1fc01), the console's revoke target (d46f2e48), memory measured with view eviction, `max_loaded` and `memory_budget` (95fb3cfb). Left: the sharing rule (§1.7), the schema shared by `Arc`. | L | 2 | one bundle, 2 tenants on 2 versions, no refusal (`versions_serving.rs`); routines run per pin (`routines.rs`); a tenant on a revoked version moves in its next window, not before; memory at 1,000+ spaces recorded here | evolution-plans ask 3 (build) |
| 4a | **Environments registered.** Declared chain, today's deployment marked production. **Built 2026-11-23, commit 0c62ff1a:** `$environments`, `std.deployment.environments` and `registerEnvironment`, `ordo environments list\|register`. Changing which environment a deployment is, removing and renaming one (owner, 2026-11-23) built in fbc3b4c6: `set-own`, `remove`, `rename`, each with a reason and on the stream; the deployment's own and one a stored promotion names are refused. | S | 1 | a missing environment is refused with the command that creates it | none |
| | *Owner, 2026-11-23: 4b is built with 4a, and so are changing a deployment's own environment and removing or renaming one ("It's all part of the same plan"). Reading the source from the step before waited on Q11, ruled 2026-11-24 and built in the finish below.* | | | | |
| 4b | **Promotion gate.** Target enforces it, source from the step before, named deployment owner on the stream, production pins only through the gate. The target's own checks: `requires`, function secrets, externals and readers (§3.1, review 2026-11-23). **Built 2026-11-23, fbc3b4c6:** `promotion.rs`, `std.deployment.promote` and `versionSource`, `ordo promote`, `ordo apply --env`, `$promotions`. **Finished 2026-11-24, c3ac5754 (owner rulings 2026-11-24):** the target reads the version from the step before's door itself, as the person promoting, with their own token for it from the same sign-in (Q11, no machine key; `step_door.rs`, the one client to another deployment's door). The gate no longer takes a source or an "activated" claim from the caller. The step before must answer as the environment the target expects and must have read the same person. `SourceCredential` is the one seam a GitHub workflow's token joins later. Each environment records its door (`ordo environments door`, `std.deployment.setEnvironmentDoor`: address, own space, identity handle). Staging is gated as production is: every step after the first pins only what arrived through the gate. `ordo apply` with no `--env` goes to the first step of the chain (dev; a deployment that registered nothing is its own first step, so it applies as before); a later step is refused naming dev's address and `ordo promote`. The reader check runs on the target (`ordo_exec::peer::check_readers`). `promotion.skip_staging` built (`ordo environments set skip-staging on`, `$promotion_settings`), named in a refusal about a staging step before. `ORDO_PROMOTION_PLAIN_HTTP` allows a plain address to another host. Tests: `crates/ordo-host/tests/promotion.rs` (real doors for dev and staging and one fake identity service; each refusal), `crates/ordo-cli/tests/promote.rs`. **Not built:** the `requires` check. The language has no `requires` yet: the parser refuses it (`ordo_syntax::auth`) and the host derives no needs (`plan-configuration-in-the-file.md` §3, step 2). The gate calls it once that step lands. | M | 4a | staging refuses a version dev has not activated; a caller's edited tree is refused naming the file | 27f |
| 5 | **Branch tenants.** `--branch`, name mapping, seed step, ephemeral dev saves, explicit delete. | M | 3, 4a | 2 branches served side by side in dev | 27g (first half) |
| 6 | **Release check.** Designed in §2.8 (review 2026-11-23), steps R1 to R3: the checked set, digests, functions under the new runtime, `auth` ranges, the `ordo release-check` mode of the one image, the boot-time run, the override. **Built 2026-11-23 (R1 to R3), commit a2edc640:** `release_check.rs` in ordo-host. The checked set: every pinned version and the active one, `versions.new_tenant`, `versions.served`, every move target, every version on a stored path between them. Per version: the source fingerprint, the compile under this release, the `auth` ranges (reported as their own check from the compile), the door's specs and the platform's reserved names, the shape gate over a dropped shadow, each function's code by hash and its table, and each stored plan's digest re-rendered. A version the release builds to other bytes is a note, never a failure (§1.1). Reads tolerate a control database from any earlier release and never run the registry's setup. `ordo release-check --direct [--json] [--override --reason <why>]`: a `--direct` use, recorded before it runs; exits 1 when a version fails, naming it, its tenants and the fix; `--override` records the upgrade going ahead and exits 0. Each run and each override is a `$release_checks` row (release, act, result, actor, reason, report, time). The first boot of each release runs the check in the background, logs it and records it; it never holds the boot. The boot no longer stops when the active version fails to compile, register, or pass the shape gate under the new release: it serves no program, that version's tenants are closed, and every tenant pinned to another version serves. A tenant pinned to a version the node does not serve is answered `server_busy` and listed on `/health/ready`, never `access_denied` or an unknown op. A test build reads `ORDO_TEST_UNCOMPILABLE` (a release build never does) to make a version uncompilable. Tests: `ordo-host/tests/release_check.rs` (a pinned version and the active version made uncompilable, each with a real boot; a moved plan digest), `ordo-cli/tests/release_check_cli.rs`, and `upgrade_continuity.rs`, whose Ordo upgrade (CI's `upgrade (first-commit)` and `upgrade (functions)`) runs `ordo release-check` from this build between the previous Ordo and this one and checks the boot's own run. **Not built:** the notes e2e case with 2 images (the e2e stack builds one Ordo; the 2-image case is `upgrade_continuity.rs`); a JavaScript compile of function code without a call (the engine compiles it at each call); peers and readers on another release (`plan-evolution-federation.md` §5); editions (6b). | M | 3 | an upgrade with an uncompilable version names it and its tenants, and every other tenant serves; the notes upgrade test passes through it | none |
| 6b | **Language editions and `ordo fix`.** §2.8, step R4. **Built 2026-11-23, commit b3d8b5ed:** the `edition <n>` line (`ordo-syntax`), the edition table and gate (`ordo_plan::edition`): the load window against the release's date, refusals `edition_unknown`, `edition_expired`, `edition_rule`, each naming its fix. The fingerprint takes the source's edition. `ordo fix <dir> [--write]`: a dry run prints the diff; `--write` compiles the rewrite, then writes it. An apply of the fixed source names the same version and moves its stored source and edition. The release check: an `edition` check outside the window, a note inside it, marked `soon` within 3 months of its end. Only edition 1 is released; edition 2 (bare `money` refused, `ordo fix` writes `money(0)`) is current only under a debug build's flag. Tests: `ordo-host/tests/editions.rs` (within the window both tenants serve; outside it only the pinned tenant closes and the check names it; fixed and applied, it is the same version and serves again; the refusals), `ordo-cli/tests/fix_cli.rs` (the dry run, the write, the same artifact, an unknown flag), `ordo-syntax/tests/edition.rs`, and the notes e2e (`tests/upgrade/v1-to-v2.spec.ts`: notes v1 at edition 1 is fixed, applied as 0.1.0, and the page works). **Built 2026-11-24:** the editor grammar colours `edition <n>` (`declaration-edition`, a contextual word; test in `editor_grammar.rs`). **Not built:** releasing edition 2 is the owner's call. | M | 6 | an old edition loads within its window and `ordo fix` rewrites it | none |
| 7 | **Seamless additive move.** Expand, gate, supported set, switch pin. **Built 2026-11-23:** an activation pins a tenant with a database of its own only when the apply's walk brought that database to Y; every other one keeps its pin on X and gets an open move in `$moves` (`Versions::activate_with`, phase `expanding`, `waiting` or `refused`, with `reason`). The catch-up's sweep expands the database (`bring_forward`), whose gate is `schema_state` over Y; the supported set is the database's `$migrations` `version` row for Y beside X's shape, which `Spaces::fits` checks for a tenant on X. Then `Versions::finish_moves` switches the pin, cause `move`, in one transaction with its `$pins` row. Unreachable is `waiting` and swept again (Q10); a refusal by name is `refused` and not retried for Y. The owner reads each move on `std.deployment.tenants` (`move`), in the apply's report and in `ordo apply`'s wait. `Versions::pin` closes a tenant's open move. Test: `notes_app.rs`, `a_change_one_tenants_data_cannot_take_leaves_that_tenant_serving_its_version`. Not built: a command to retry a refused move (a new version retries it); the console does not show moves. | M | 3 | a refused change leaves the tenant serving X, and the deployment owner sees why | 27c (additive) |
| 7a | **Evolution through the examples** (new, review 2026-11-23; owner, 2026-11-23). `examples/notes-v3`: one additive change, one rename with `prev`, one removed field. The harness applies it over a running notes v2 with a browser open, and note-viewer reads through it. Later increments add cases to it. **Built 2026-11-23:** `examples/notes-v3` (0.3.0) adds `notes.starred` (constant default) with `starNote` and `starredNotes`, and stops declaring `attachments.size`; `addAttachment` still takes `$size`, so the v2 page still adds a file. The rename waits for increment 15: `prev` does not parse yet outside an external. There is no v3 page: the v2 page stays open across the move. **Found and fixed by it:** an apply that added a field beside a drop the serving version still reads was refused whole (`drop_read_by_active_op` counted against the addition). The drop is now left pending and the addition applied, on the deployment's database and each tenant's (`schema_apply::without_drops`; test `tenant_forward.rs`, `an_addition_beside_a_drop_the_serving_version_reads_applies`). Tests: notes e2e project `v3` (`tests/v3/v2-to-v3.spec.ts`: notes, a pin, a tag and a file kept with no reload and no alert, a note added after, then a reload) and note-viewer e2e project `v3` (`tests/v3/notes-v3.spec.ts`: the viewer lists every note through the move). Increment 7 is d583de86. | M | 7 | the notes page keeps every note across v2 to v3 with no reload error; the test is in the example's own e2e | none |
| 7b | **Federation under versions** (new, review 2026-11-23). `plan-evolution-federation.md`, steps F1 to F6: readers named in a plan, column grants that follow the served fields, every loaded version checked against its peers, the checks in rollback and promotion. | M, in 2 parts: F1, F3, F4 with 8; F2, F5, F6 with 11a | 7, 8, 11a | note-viewer never reads a stale column: it reads through the soak, then is refused naming the fix | none |
| 8 | **Stored plans.** Graph, `ordo evolve --from --to` as the door op `std.deployment.evolve` that replaces `ordo migrate --apply --direct` (review 2026-11-23), re-render and plan digest, per-tenant stale check, system SQL at run time, receipt, replay. **Session 1 built 2026-11-23** (6d93397c, ef5fe610, 9856f637): `$plans` and `evolve.rs`; the step renders from the 2 stored sources against an empty database of X, so the digest is the same in every environment; `--digest` refuses another; a plan with errors is never stored; the receipt lists each change, class, option, forward and reverse, and leaves out Ordo's own tables; replay walks the stored path, each step checking the recorded version, drift and the digest; `std.deployment.evolve`, `plans`, `replay`; `ordo evolve [<dir>] --from --to [--run]`, `--tenant`, `--list`; F1 and F4's rollback check. Tests: `stored_plans.rs`. **Through the examples, built 2026-11-23** (branch `evo-examples-evolve`): the notes e2e runs the identity service, and its developer signs in with `ordo login` approved by passkey (`tests/developer.ts`); project `evolve` (`tests/evolve/v2-to-v3.spec.ts`) stores v3, runs `ordo evolve --from 0.2.0 --to 0.3.0`, then `--run`, with the v2 page open. `ordo migrate --apply --direct` is removed and refuses naming `ordo evolve` (§6 conflict 9). **Not built:** option expressions beyond `--places`, backfill functions (D16, with 10b). | L | 2 | a plan re-rendered in staging matches dev's digest; a tenant 3 behind replays 3 steps | 27a, 27b, 27an (hint) |
| 9 | **Pre-checks on every tenant.** With the external, `requires` and reader checks (§2.3, review 2026-11-23). **Built 2026-11-23** (f8828906): `precheck.rs`. `ordo evolve` stores the plan, then asks every database (shared or per tenant) the data probes and the external check, and lists every issue with its database, spaces and fix. `--run` refuses while one is open, and `run_step` refuses a tenant's step the same way. A database at another version is listed as not asked. Readers are listed from the render (F1). The sign-in removal check (Q6): a version whose `auth` block drops a method counts the people left with none, and is not stored until `--approve-sign-in-removal <count>` names that count; the approval is act `sign-in-removal-approved` on the stream. Tests: `prechecks.rs`. **Not built:** the `requires` check (the declaration is not in the language; the answer says so), the peer note on a removed shared provider (`plan-evolution-federation.md`), **Through the examples, built 2026-11-23** (branch `evo-examples-evolve`): project `evolve-checks` (`tests/evolve/prechecks.spec.ts`) stores 0.3.1 (`fixtures/notes-v3-1.ordo.fixture`, v3 with `title` unique) over a tenant holding 2 notes with one title; `ordo evolve` lists them with the fix, `--run` is refused, the title is changed on the page, and `--run` passes. | M | 8 | a plan over 3 tenants with 2 bad lists both issues with their fixes | 27w |
| 10 | **Rollouts, the core** (split, review 2026-11-23; design in §3.7). `$rollouts`, the leading worker, `rolling` and `all`, windows and holds, controls as door ops, the failure threshold, the stranded-tenant refusal, minimum version moves, `--move-to`, the deployment's own space, the acts recorded (§1.12). | L | 7 | a failed batch pauses; a tenant outside its window waits; a second node finishes a rollout whose leader died | 27h (strategies, first half) |
| 10c | **Rollout strategies** (split from 10, review 2026-11-23). Canary, group first, by query over tenant facts, share partners together, health by error rate, the alert channel. | M | 10 | a canary that fails its health check pauses the rest | 27h (strategies, rest) |
| 10b | **Backfill worker.** `plan-backfill-dual-writes.md` steps B1, B3, B6 (review 2026-11-23): `$backfills`, the chunk loop, verify, the summarised audit group, backfill functions. **Built 2026-11-23** (37bba572, 3f931aed, 0d4082f5, f684a30a): a stored plan fills a field it makes required (B2, B5) with `--fill <entity>.<field>=<Ordo expression>` or `=fn <name> [secrets ..] [hosts ..]`; the step expands the column nullable, `backfill.rs` fills it in chunks with its cursor in `$backfills` in the same transaction, verify recomputes each space's count and content hash, then `NOT NULL` lands. Until then the database records the version before the step, so a run again resumes from the cursor. 5 failed chunks mark it `dead`; a run again retries. The audit record is trailer `0x04` on the chain: one summarised group per space, or with `--audit-images` one image group per space per chunk; goldens of every older group kind landed first. Backfill functions run once per chunk with exactly the declared secrets and hosts (Q14), and the receipt marks them not reproducible. Tests: `backfill.rs` (the kill mid-chunk, verify, images, dead, render refusals, functions), `notes_app.rs` (`a_backfill_over_existing_notes_shows_the_new_values_once_it_completes`), the command log's goldens. **Not built:** the Playwright test in `examples/notes-v3` (the harness holds no deployment-owner credential for `ordo evolve`; asked), live writes during the backfill (11a), the settings stored in Ordo (`backfill.chunk_rows` and the rest are defaults until 10 stores them, Q15), the move lock and `backfill.concurrency` across tenants (11b), sealed backfills (13), refusing `ordo revert` of a backfill group by name. | L | 7, 8 | a backfill killed midway resumes from its cursor; the chain's goldens hold | 13c (backfill half) |
| 11a | **Dual writes on one tenant** (split from 11, review 2026-11-23). `plan-backfill-dual-writes.md` steps B2, B4: the move overlay in the host write path, both versions' guards, the soak, the reverse writes, soak close into `$orphans`. Research: 2 to 3 tries. | L | 10b | a rename on 1 tenant under live writes loses no write; a revert in the soak keeps every write | 13c (reshaped) |
| 11b | **Every tenant** (split from 11, review 2026-11-23). Step B5: moves driven by a rollout, shared databases, `ordo rollback` and its `--dry-run`, grants stay killed. | M | 10, 11a | a rename on 3 tenants under live writes loses no write; a rollback in the soak keeps every write | 27c |
| 12 | **Staging clones.** Masking in production, deny by default, encrypted bundle, sinks, no credentials, clone carries its version. **Session 1 built 2026-11-24** (branch `evo-12-clones`, §3.4 "Built"): `clones.rs`; production masks before export (deny by default, `clone.mask`, `off` opt-in), drops files, the command log and every member's name and sign-in link; the bundle is sealed to staging's clone key (`ordo_crypto::recipient`) recorded with `ordo clones set-key`; staging reads it from production's door as the person, pins a new tenant to the version with the same source identity and imports it under its own keys; Q12 built: a staging admin or a `clone.readers` person opens it as its founding owner, and a cloned member's `$clone:` principal is never mapped. Door ops `std.deployment.cloneKey`, `setCloneKey`, `cloneOut`, `cloneIn`, `clones`, `setCloneSetting`; `ordo clones key\|set-key\|clone\|list\|set`. Tests: `ordo-host/tests/clones.rs` (2 real doors: the clone serves masked notes through staging's door, no production value anywhere in staging's database, a cloned member refused, each refusal), `ordo-cli/tests/clones_cli.rs`. **Not built:** sinks, a version staging does not hold (refused naming `ordo promote`), a tenant's own database or bucket recreated, the notes example test. | L | 3, 4b | a clone of a tenant on X serves in staging on X with no real personal data | 27g (second half), 27v |
| 12b | **Signed rehearsal (provisional, §3.2).** | M | 8, 12 | production refuses a plan digest staging did not sign | 27f (rest) |
| 13 | **Sealed backfills.** No plaintext dual write, plaintext dropped on verify, `Purpose::Migration`. | L | 11b | sealing on 3 tenants under live writes verifies by count and hash, and no plaintext column remains | 13d |
| 14 | **Orphans and pruning.** `$orphans` for erasure and export, drop dates, prune rules, tombstones, named-environment confirmations. Pruning as `ordo evolve --prune` with its second confirmation; the reader rule for orphaned columns (§2.7, review 2026-11-23). **Built 2026-11-23** (239afd67, 525a781d, aefe7a8a): `$orphans` in each database (`ordo_exec::orphans`), written wherever a database is brought to a version that stops declaring a field or an entity: the deployment's own apply, each tenant's database brought forward, each stored step run (`schema_apply::record_orphans`). Each row has its columns, a sealed flag, a drop date and, once pruned, a tombstone. A version that declares the field again removes its row. Erasure deletes a space's rows from an orphaned table too, and names the orphans it reached (`std.space.delete`, `orphans`). Export writes each orphan under `orphans/` with the row's id. `std.deployment.orphans` and `ordo evolve --orphans` list them. `std.deployment.prune` and `ordo evolve --prune <entity.field>`: the first ask prunes nothing and says what would go; `--confirm <name>` prunes; in production `--confirm-deployment <domain>` too. Refused when `prune.enabled` is off (off in production; `ordo environments set prune on`), for a name no database records, and for a field the active version or a pinned one still declares. The reader rule (Q1): a table on a `reader` line that holds an orphaned column is granted by column, without it. The reader's check holds the entity with `external_changed`, naming the column, the read app and the fix. Tests: `orphans.rs`, `reader_grants.rs` (`a_reader_loses_an_orphaned_column_with_a_named_error`); notes e2e project `orphans` (`tests/orphans/orphans.spec.ts`); note-viewer e2e project `orphans` (`tests/orphans/orphaned-column.spec.ts`, fixture `notes-no-tags`). **Found by it:** `std.space.delete` failed on a tenant with a database of its own, which had no `$logconsumers`; erasure now makes it. **Not built:** soaks (11a), so an orphan is recorded when the database moves, not at a soak's close; nothing drops at the drop date by itself; the language cannot mark a field personal, so every orphan gets a drop date; restorable versions (17) are not yet checked by pruning; an orphaned `public` field (§1.14) is not handled apart. | M | 8 | a dropped field is orphaned and erased on request | 27y, 13e, 27h (rest) |
| 15 | **`prev` in the language.** The syntax is ruled (2026-10-04) and written in `plan-syntax-additions.md` §5 (review 2026-11-23). Parser, the 3 compile checks, the 3 plan checks, the CDK's `prev()`, and the offer to remove landed `prev` lines. Refused on an external. | M | 8 | a declared rename renders as a rename; notes v3 uses it | 27x |
| 16 | **Tenant builds.** After first deployment. | S | 3, 4b, 5, 10 | a beta tenant serves the branch while the rest serve main | 27t |
| 17 | **Restore and import across versions** (new, review 2026-11-23; design in §2.9). | M | 3, 8 | a snapshot taken on X restores beside tenants on Y and serves X | 11-10b (`snapshot-restore`) |

**Increment 3, what remains after session 3** (session 3: 2026-11-23):

- Session 2 built the revoked-version moves (§1.11): in the window, on
  demand, by the tenant when granted, each to the developer's target.
- Memory at 1,000+ spaces, measured and recorded here; `versions.max_loaded`
  and `versions.memory_budget` set from it; views evicted per space. Done
  in the remainder (§1.5). Left: sharing the schema by `Arc`.
- Load guards: `versions.min_residency`, `versions.idle_evict_after`,
  `versions.max_concurrent_loads` as settings. Built in the remainder
  (§1.4, "Load guards").
- Session 3 built digest at load (9acbc2cf): every load of a stored
  version recomputes its source fingerprint and refuses a mismatch,
  naming the version (`LoadRefusal::Digest`, §6 conflict 4).
- Session 3 built functions per version (9acbc2cf): one table per
  served version, and a function's `uses` resolve in its version (§1.10).
  Left: the per-version catalog and generated interface.
- Schedules and listeners as the union over served versions; cursor
  hand-over at a switch. Built in the remainder (§1.10).
- Session 2 built `versions.served` and `versions.minimum` (§1.6). Left:
  the refusal of a rollout that strands a tenant (with rollouts), and a
  command that removes a stored version (`--move-to`).
- Sharing rule (§1.7), `versions_disagree`. Not built in the remainder;
  §1.7 says what was found and where it goes.
- Client contract (§1.9): version on every answer, the distinct refusal for
  an op the version lacks, idempotency across one switch and the tenant
  admin's label are built in the remainder. Built 2026-11-24 in
  `clients/ordo-react`: `DescriptorClient` reads `x-ordo-tenant-version`
  off every answer, `onVersionChange(from, to)` fires when it moves (the
  first answer is the baseline), and every read hook refetches, with or
  without a live feed (`test/live.test.ts`, "the version header"). Built
  2026-11-24 (second PR): the read lives once, in `VersionWatch`
  (`clients/ordo-sdk/src/core/version.ts`). `OrdoClient.versions` and
  ordo-react's `DescriptorClient` use it. The sync manager drops and
  refills the device copy on a change, as on the `version_changed` frame
  (Q7; `manager.test.ts`). No example step, the §4 table names none.
- Session 3 built stream close on a pin switch (9acbc2cf). A feed
  (`?live=1`, a sync, a read on `/live`) records the version its tenant
  was served when it opened. When a pin moves or the active version
  swaps, the node asks every feed's door at once
  (`LiveStreams::recheck_now`). A moved tenant's feed ends with
  `event: version_changed`, class `space_unavailable`, naming no version.
  The SDK's sync manager and ordo-react's live hub know the frame by its
  name and reconnect at once on the normal backoff. The sync manager
  drops the tenant's device copy and fills it again (Q7, ruled
  2026-11-23).
- **Added by the review of 2026-11-23:** one function table per loaded
  version (§1.10, built in session 3); loading a version beside the active
  one drops no public cache (§1.14, checked in session 3); the SDK drops
  its device copy on "version changed" (§1.9, Q7); the external check and
  the peer links run for every loaded version, not the active one alone
  (`plan-evolution-federation.md`, step F3).
- Session 3 built F3's first two parts (9acbc2cf): the external
  timer reads every served version's externals, and a hold holds only the
  tenants of the version it was found for. Peer links were already read
  for every loaded version, at load and each minute. Left: the timer over
  tenant databases.
- Session 3's tests: `versions_loaded.rs` (a running node: functions per
  pin and the negative, digest, public cache, feed close) and
  `external_review.rs` (a hold per version).
- Session 2 built per-node health on `/health/deploy`. The remainder
  added `loading`: a version still loading, apart from `alsoServing`.
- Session 2 built provisioning on the tenant's own version (§1.3).
- Session 2 built the console's parts: Deploys says `serving N tenants`
  and `active (new tenants)`; Tenants has a Version column
  (`std.deployment.tenants` carries `version` and `ordinal`). Left: the
  console has no screen to move a tenant, set its target, or name a
  revoke's `--move-to` for a version that is not active. The Tenants
  screen's Version button (pin, move now, target) was built in f4459729.
  The remainder added the last: Deploys' revoke of a version that does
  not serve takes "Its tenants move to", the active version by default,
  and the warning says where each revoked version's tenants go
  (`windows.spec.ts`).
- Audit leftovers not taken by the CLI-output fixes: times as UTC, window
  caps as settings or recorded as fixed. Done in the remainder: `ordo
  versions show` (applied, activations) and `ordo versions tenant` (pin
  history) print times as `2026-11-05 14:02 UTC`, as revoke times
  already did. The window caps are recorded as fixed (§7, "Fixed caps").

What sets the pace:

- Increment 3 is research on memory: say 2 tries.
- Increments 10b and 11a are research: say 2 to 3 tries, with a rehearsal
  against a real tenant's data.
- Increment 12b may change with the provisional design.
- Increment 15 waited on the syntax design. That was ruled 2026-10-04, so
  it waits only on increment 8 (review 2026-11-23).
- 27z (the `acl` security audit) is outside this plan.

**The order after the review of 2026-11-23:**

- 3 (session 3), then 4a, 7, 7a. That gives a version move a test through
  an example before anything larger is built on it.
- Then 8, 9, 15 and the first part of 7b.
- Then 10, 10b, 11a, 11b and the rest of 7b.
- 4b, 5, 6, 6b and 17 fit anywhere after their needs.
- The 16 open questions in `review-evolution-plans-2026-11-23.md` set the
  pace more than the sizes do. Q1 to Q4 hold increments 10 and 11a.

**Tests through the examples** (owner, 2026-11-23: behaviour is tested
through the example apps). Each increment from 7 on adds its "done when"
to an example's own e2e, beside its host tests:

| Increment | Example | The test |
|---|---|---|
| 6 | notes | the upgrade test runs the release check between 2 images |
| 6b | notes | notes v1, at edition 1 under a build where edition 2 is current: `ordo fix` dry run, `--write`, applied as 0.1.0, the page works (project `upgrade`) |
| 7, 7a | notes-v2 to notes-v3 | apply with a browser open; every note still shows |
| 8 | notes-v2 to notes-v3 | `ordo login`, then `ordo evolve` and `--run` with the page open; every note still shows (project `evolve`) |
| 9 | notes-v3 to fixture 0.3.1 | a duplicate title is listed with its fix, `--run` refused, fixed on the page, `--run` passes (project `evolve-checks`) |
| 7b | notes-v2 and note-viewer | the viewer reads through a rename, then is refused naming the fix |
| 10, 10c | teamspace, 3 tenants | each tenant shows its own version's page during a rolling rollout |
| 11a, 11b | notes-v3 | type into a note during the backfill; nothing typed is lost; roll back in the soak |
| 13 | notes-v3 | seal a field on live data; the page reads it and the table holds no plaintext |
| 17 | notes-v3 | snapshot on v2, move, restore, read the v2 notes |
| CLI | one example, start to end | `ordo dev`, `apply`, `evolve`, `rollout status`, `rollback`, each through the door (owner, 2026-11-23, ask 5) |

---

## 5 · Open decisions

The plan's asks 1 to 4 and the review's D1 to D17 were ruled 2026-11-02.
See `corpus-rulings.md`. Building increment 1 raised four, on the board
task `program-versions-the-core-under-schema`:

- Ask 5, ruled 2026-11-03: a source with no edition line is edition 1 for
  good.
- Ask 6, ruled 2026-11-03: the declared `@` version names a version.
  Its "revision with a warning" half was replaced the same day by ask 13.
- Ask 9, ruled 2026-11-03: new content under a number lower than the
  highest applied is refused, naming the next.
- Ask 10, ruled 2026-11-03: a program with no `@` is applied with a note
  saying how to declare one, never refused.
- Ask 11, moot after ask 13: promotion needs no rule for `+r` names.
- Ask 12, ruled 2026-11-03: `ordo apply --bump` raises the number in the
  file, opt-in.
- Ask 13, ruled 2026-11-03: a real change under a used number is refused
  on every path, naming the next number; comments and spacing never are.
- Ask 7, ruled 2026-11-03: a version stored under a 16-hex name keeps it.
- Ask 8, ruled 2026-11-03: every version shows its order number (#1, #2,
  ...). A program with no `@` is still named by the 16-hex digest of what
  it compiles to.

**After the review of 2026-11-23:** 16 questions, Q1 to Q16, in
`review-evolution-plans-2026-11-23.md`. 13 were ruled 2026-11-23 on the
board. Q3 (tenant hotfixes beside a rollout), Q11 (how promotion reads the
step before) and Q14 (backfill functions calling out) were asked again; all
three are ruled (Q11 on 2026-11-24).
Nothing marked **Proposed** is built before its question is answered.

---

## 6 · Conflicts

Resolved by `f4df3abe` (2026-11-02): evolution-plans ask 3 and D-B2, 13c's
write freeze, shared tables in `plan-schema-evolution.md` §6, the artifact
format promise, and the `--staging` and `--prod` flags.

Still open:

1. **The code holds behind tenants closed.** 25i's `SchemaBehind` and the
   15-second catch-up are what 27c replaces. `plan-schema-evolution.md`
   §8.3 reads as current behaviour.
2. **`ordo dev` versions are not hashes.** Fixed 2026-11-03: `dev.rs`
   labels a save `0.2.0+dev.` and 12 hex of its source fingerprint, so it
   reads as the declared version and swaps on every change. It still
   writes no `$artifacts` row, because dev saves are ephemeral (§3.3).
3. **The version is 64 bits.** Resolved 2026-11-03 by the owner's ruling:
   a version is named by its declared `@` version, not a digest. The full
   SHA-256 fingerprint is kept as a check for promotion.
4. **A load does not check the version.** `deploy.rs:466-505` passes
   `stored.version` through as a label. Resolved in increment 3, session
   3: every load of a stored version recomputes its source fingerprint and
   refuses a mismatch (`LoadRefusal::Digest`, naming the version).
5. **`plan-environments.md` gates on activation.** Its §2.1 refuses
   staging "until dev runs this version". The 2026-11-02 ruling (D2) adds a
   signed rehearsal.
6. **`plan-evolution-plans.md` stores the live shape.** Its §3.1 has a
   stale plan refuse against the shape it was rendered on. Ruled 2026-11-02
   (D3): each target re-renders, and the stale check runs per tenant at
   run time.
7. **`plan-schema-evolution.md` builds a write barrier.** Its class B and
   increment 3 still describe the barrier under the 2026-10-01 banner.
   Dual writes now run in the host write path only (D10), and sealing steps
   have none in plaintext (D9).
8. **Constants in code.** `schema_apply.rs` fixes `FORWARD_CONCURRENCY = 8`
   and `FORWARD_TIMEOUT = 20 s`, and `ordo apply` waits 300 seconds
   (`ordo-cli/src/main.rs:127`). Each becomes a setting (§7).
9. **`ordo migrate --apply` runs only with `--direct`** (review
   2026-11-23). The owner ruled the CLI through the door on 2026-11-05 and
   kept `--direct` for 9 commands that must work with no server. A
   transforming migration is not one of them. Increment 8's
   `std.deployment.evolve` replaces it. Built 2026-11-23: `ordo evolve
   --run` moves the deployment's own database through the door. **Removed
   2026-11-23** on the coordinator's brief (`evo-examples-evolve`): `ordo
   migrate` now refuses whatever follows it, with the usage code, naming
   `ordo apply --no-activate` and `ordo evolve --run`. Its callers, tests and
   docs moved to `ordo evolve`.
10. **`plan-schema-evolution.md` §9 read as open** (review 2026-11-23). The
    backfill audit question was ruled 2026-09-08 and 2026-09-20. The
    section now says so.
11. **The public cache drops on every install** (review 2026-11-23).
    `public::after_deploy` is right while one version serves. With many, it
    must fire only when the deployment's own space changes version (§1.14).
    Checked in increment 3, session 3: it fires on the active version's
    swap alone. Rollouts must keep it so.
12. **Reader grants and `AuthApp` follow "active"** (review 2026-11-23).
    Both were designed when an activation moved every tenant at once. With
    rollouts, "active" flips at the end. Q2 and Q5 ask whether that is the
    right moment for each.
13. **The board's `db-lag-bound` and `db-behind-schema` still owe "one
    version back"** (review 2026-11-23). D15 replaced D-B2's bound on
    2026-11-02 (§1.6). The board tasks were not changed by this review.
14. **`plan-space-routing.md` C-M4 named "the activated schema version"**
    (review 2026-11-23). With pins it is the tenant's pinned version. Fixed
    there.

---

## 7 · Settings

The owner, 2026-11-02: *"avoid hardcoded values and elevate to potential
configuration where possible."* Each setting is layered (deployment,
environment, and plan or tenant where named) with a secure default. Every
refusal a setting can fix names the setting and the value.

| Setting | Default | Layer |
|---|---|---|
| `versions.served` | `in-use` | deployment, stored in Ordo |
| `versions.minimum` | none | deployment, stored in Ordo |
| `versions.new_tenant` | last version that finished a rollout (the active version until rollouts) | deployment, stored in Ordo |
| `versions.idle_evict_after` | 30 minutes, or `off` (built in increment 3, stored in Ordo) | deployment |
| `versions.min_residency` | 10 minutes (built in increment 3, stored in Ordo) | deployment |
| `versions.max_loaded` | 8 (`ORDO_VERSIONS_MAX_LOADED`, set from increment 3's measurement) | node |
| `versions.max_concurrent_loads` | 1 (`ORDO_VERSIONS_MAX_CONCURRENT_LOADS`, built in increment 3) | node |
| `versions.memory_budget` | 512MB of per-space views (`ORDO_VERSIONS_MEMORY_BUDGET`, set from increment 3's measurement) | node |
| `pins.reconcile_interval` | 30 seconds (`ORDO_PINS_RECONCILE_SECS`, built 2026-11-05) | deployment |
| `forward.retry_interval` | 15 seconds | deployment |
| `editions.load_window` | 12 months after the next edition became current, measured against the release date (built in 6b, `ordo_plan::edition::LOAD_WINDOW_MONTHS`) | Ordo release |

**The three version settings live in Ordo** (owner, 2026-11-05: *"that
lives within ordo and should likely not be an env because it is dependany
on what is applied since that is where versioning happens"*). Built the same
day:

- Stored in `$version_settings` in the control database, append-only, with
  who, when and why. A setting's value is its newest row's.
- Set through the door (`std.deployment.setVersionSetting`, read with
  `std.deployment.versionSettings`), the CLI (`ordo versions settings`,
  `ordo versions set <served|minimum|new-tenant> <value> | --clear
  --reason`) and the console's Deploys screen. The deployment owner, or a
  machine key with the `deployment` grant. A reauth only at
  `ORDO_SECURITY_LEVEL=high`.
- The checks boot made run on the change, and refuse it naming the
  setting, the value and the command. Boot still checks the stored values,
  and only logs (owner, 2026-11-02: an upgrade never stops a running
  deployment).
- A change nudges the pin channel; each node takes it with the pins
  (`pin_stream.rs`) and on its reconcile tick.
- `ORDO_VERSIONS_SERVED`, `ORDO_VERSIONS_MINIMUM` and
  `ORDO_VERSIONS_NEW_TENANT`: on a boot where one is set and nothing is
  stored for it, its value is stored once, as `ordo:versions`, with the
  checks a change has. After that each boot logs that it is ignored and can
  be removed.
- Environment variables stay for settings about the binary itself.
| `soak.length` | 7 days, 0 allowed | plan |
| `backfill.concurrency` | 8 | environment |
| `backfill.chunk_rows` | 1,000 | plan |
| `backfill.chunk_timeout` | 20 seconds | plan |
| `backfill.budget` | none | environment |
| `backfill.audit_images` | off | plan |
| `seal.keep_plaintext` | off | plan |
| `rollout.strategy` | rolling | environment |
| `rollout.batch_interval` | 5 minutes | environment |
| `rollout.window` | 1 hour, a target | environment |
| `rollout.canary_count` | 1 tenant | environment |
| `rollout.canary_wait` | 1 hour | environment |
| `rollout.failure_threshold` | 1 tenant | environment |
| `rollout.health_error_rate` | 1% | environment |
| `rollout.alert_channel` | the deployment's stream | environment |
| `apply.wait` | until the rollout starts; 300 seconds at most | command |
| `tenant.maintenance_window` | none: no window; set by `ordo windows set` for every tenant, some, or all but some (2b) | deployment, tenant |
| `promotion.chain` | dev, staging, production | deployment |
| `promotion.skip_staging` | off | deployment |
| `promotion.release_mismatch` | refuse | environment |
| `promotion.attestation_max_age` | 7 days (provisional) | environment |
| `clone.mask` | deny every field not marked safe | environment |
| `clone.sinks` | on | environment |
| `orphans.personal_drop_after` | 30 days | environment |
| `prune.enabled` | off in production, on in dev | environment |
| `branch.idle_list_after` | 14 days | deployment |

Defaults not set by a ruling are proposals, checked in the increment that
builds them.

**Fixed caps** (recorded in the increment 3 remainder, not settings). Each
bounds input or the cost of one answer, like every other text and list
bound in Ordo. None is a policy a deployment would tune.

| Cap | Value | Why fixed |
|---|---|---|
| A window's message | 2,000 characters | an input bound |
| A skip's reason | 500 characters | an input bound |
| A window or skip id | 64 characters | an input bound |
| Tenants one window lists | 1,000 | more is `all` or `all except`; bounds one write |
| The window list's reach | 12 months back and ahead | the 24-month list the owner ruled (asks 22, 24) |
| Times one cron window gives in the list | 20,000 | about once an hour; bounds the list's cost |
| Times one list answers | 100,000 | the list is cut and says so |

Added by the review of 2026-11-23:

| Setting | Default | Layer |
|---|---|---|
| `rollout.deployment_space` | `first` (ruled 2026-11-23, Q4) | environment |
| `rollout.unreachable` | `wait` (ruled 2026-11-23, Q10) | environment |
| `backfill.per_database` | 2 moves at once in one database (proposed) | environment |
| `backfill.function_egress` | off unless the backfill declares it (ruled 2026-11-23, Q14) | plan |

- **Where these settings live.** The owner ruled 2 things. Version
  settings live in Ordo, because they depend on what is applied
  (2026-11-05). Configuration lives in the file unless it is a credential
  or differs per instance (2026-11-14).
- Every `rollout.*`, `backfill.*`, `soak.*`, `promotion.*`, `clone.*`,
  `orphans.*`, `prune.*` and `branch.*` setting differs per environment
  (D6), so none belongs in the file.
- **Ruled 2026-11-23 (Q15):** they are stored in Ordo beside the 3 version settings, in
  `$version_settings`, set through the door with who, when and why. The
  "plan" layer is the option recorded in the stored plan. This follows the
  2026-11-05 ruling's reason.
