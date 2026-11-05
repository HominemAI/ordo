# Program versions: the core under schema evolution and environments

Status: **build plan, revised 2026-11-02 after review.** The owner ruled the
plan's 4 asks and all 17 of the review's decisions on 2026-11-02. Asks 5 to
7, from building increment 1, were ruled 2026-11-03; ask 6 made the
declared `@` version the name (§1.1). Ask 8 is open (§5). The rehearsal
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
  seam every later single-tenant move calls (§1.11). It asks no passkey
  touch, of a system or a person, strict deployment or not (owner,
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

### 1.5 What serving many versions costs

| Cost | Size | When it is paid |
|---|---|---|
| Memory | one `Loaded` per served version per node, and one `SpaceView` per space per `Loaded`. **Not measured.** Increment 3 measures it at 1,000+ spaces, shares schema and registry by `Arc`, and evicts views per space. | per version in use |
| Load | one compile plus one gate per version per node | at pin time, at boot, and after each Ordo release |
| Idle versions | evicted after `versions.idle_evict_after` | only when a version has no traffic |
| Ordo releases | every reachable version's source compiles and gates under the new release (§2.6) | every release |
| Mixed fleets | during a rolling Ordo upgrade, nodes differ. A pin switch completes when every live node has Y loaded. | each upgrade |
| Fixes | a fix to the app's ops reaches a tenant only when it moves | the app developer's choice (owner, 2026-10-01) |

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

As built (2026-11-03): `Versions` owns `$artifacts`, `$builds`,
`$activations`, the pins, `$pins` and `$moves`, over `ordo_exec::registry`
as its SQL layer. `$code` stays with `functions.rs` and `$plans` does not
exist yet. `VersionRefusal::to_exec_error` is the one door mapping.

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

### 1.10 Background work under pins

Routines, schedules, functions, mail and webhooks:

- Schedules are the union over served versions.
- Each run executes under its tenant's pinned `Loaded`.
- A pin switch hands the cursor over to Y.

As built in increment 3, session 1: each run resolves its tenant first and
runs the routine as the tenant's version declares it, named
`<routine>@<that version>`. A routine the tenant's version does not
declare is skipped and recorded `undeclared`. The schedules and listeners
are still the active version's alone (session 2).

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
  system credential can call, with no passkey touch and no exemption
  setting. A person moving a version is not asked either (owner,
  2026-11-05: *"They could be moving many at a time"*).
- Where it lands: increment 3, session 2. Session 1 built the seam:
  `Versions::pin` is the one single-tenant move, `std.deployment.pin` its
  door op, which asks no touch. The window-timed move and the tenant's own
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
  system's machine key, no passkey touch.
- By the tenant: `std.space.moveOffRevoked`, cause `tenant`, granted to the
  roles in `ORDO_VERSIONS_TENANT_MOVE_ROLES` (`versions.tenant_move_roles`),
  none by default. No touch.
- `ordo versions revoked` and `std.deployment.revoked` show the revoke's
  `moveTo`, and each tenant's own `target` and `nextWindow`, or "no window".
- `$move_targets` is an erasure attachment.
- Tests: `crates/ordo-host/tests/versions_moves.rs`.

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

### 2.7 Orphans and pruning

- Erasure and export read `$orphans`, so a left-behind column is never
  missed.
- Personal or sealed orphans get a drop date, `orphans.personal_drop_after`.
- Pruning refuses a pinned, restorable or retained version, and keeps a
  tombstone.
- `prune.enabled` is off in production and on in dev.
- A destructive command in production asks for the deployment's name.

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
- `ordo apply` targets dev by default.
- **The target enforces the gate.** It fetches the source from the step
  before, never from the caller's tree. It recompiles, recomputes the
  source identity, and refuses on a difference, naming the differing file.
- It re-renders each stored plan and compares the plan digest (§2.3).
- Per-environment and runtime values are bound at the target (§1.1).
- A production pin accepts only a version that arrived through the gate.
- Promotion is an act by a named deployment owner, recorded on the
  deployment's stream (`plan-environments.md` §6).

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
| 3 | **Multi-version serving, dev first.** Pin epoch checked on write and the pin stream nodes tail (from 2, asks 16 and 17). Map of versions, load guards, digest at load, shape fingerprint, per-version catalog, served and minimum versions, sharing rule, client contract, background work, stream close, per-node health. Moves off a revoked version in a window or on demand (ruled 2026-11-05, §1.11). Session 1 built 2026-11-05: the fence, the stream, the map, per-tenant lookup and access, the shape fingerprint, runs per pin, `pin` and `tenantVersion`, the `new_tenant` check (§1.3, §1.4, §1.10). | L | 2 | one bundle, 2 tenants on 2 versions, no refusal (`versions_serving.rs`); routines run per pin (`routines.rs`); a tenant on a revoked version moves in its next window, not before; memory at 1,000+ spaces recorded here | evolution-plans ask 3 (build) |
| 4a | **Environments registered.** Declared chain, today's deployment marked production. | S | 1 | a missing environment is refused with the command that creates it | none |
| 4b | **Promotion gate.** Target enforces it, source from the step before, named deployment owner on the stream, production pins only through the gate. | M | 4a | staging refuses a version dev has not activated; a caller's edited tree is refused naming the file | 27f |
| 5 | **Branch tenants.** `--branch`, name mapping, seed step, ephemeral dev saves, explicit delete. | M | 3, 4a | 2 branches served side by side in dev | 27g (first half) |
| 6 | **Release check.** Reachable versions, digests, functions under the new runtime, override. | M | 3 | an upgrade with an uncompilable version stops, naming it and its tenants | none |
| 6b | **Language editions and `ordo fix`.** | M | 6 | an old edition loads within its window and `ordo fix` rewrites it | none |
| 7 | **Seamless additive move.** Expand, gate, supported set, switch pin. | M | 3 | a refused change leaves the tenant serving X, and the deployment owner sees why | 27c (additive) |
| 8 | **Stored plans.** Graph, `ordo evolve --from --to`, re-render and plan digest, per-tenant stale check, system SQL at run time, receipt, replay. | L | 2 | a plan re-rendered in staging matches dev's digest; a tenant 3 behind replays 3 steps | 27a, 27b, 27an (hint) |
| 9 | **Pre-checks on every tenant.** | M | 8 | a plan over 3 tenants with 2 bad lists both issues with their fixes | 27w |
| 10 | **Rollouts.** 5 strategies, windows and holds, controls, health, alert channel, query rules, minimum version moves, `--move-to`. | L | 7 | a failed batch pauses; a tenant outside its window waits | 27h (strategies) |
| 10b | **Backfill worker.** Resumable, chunked, cursor per tenant, layered settings, backfill functions. | L | 7, 8 | a backfill killed midway resumes from its cursor | none |
| 11 | **Transforming migration on every tenant.** Host-path dual writes, one soak per tenant, `ordo rollback`, grants stay killed. | L | 10, 10b | a rename on 3 tenants under live writes loses no write; a rollback in the soak keeps every write | 13c (reshaped), 27c |
| 12 | **Staging clones.** Masking in production, deny by default, encrypted bundle, sinks, no credentials, clone carries its version. | L | 3, 4b | a clone of a tenant on X serves in staging on X with no real personal data | 27g (second half), 27v |
| 12b | **Signed rehearsal (provisional, §3.2).** | M | 8, 12 | production refuses a plan digest staging did not sign | 27f (rest) |
| 13 | **Sealed backfills.** No plaintext dual write, plaintext dropped on verify, `Purpose::Migration`. | L | 11 | sealing on 3 tenants under live writes verifies by count and hash, and no plaintext column remains | 13d |
| 14 | **Orphans and pruning.** `$orphans` for erasure and export, drop dates, prune rules, tombstones, named-environment confirmations. | M | 8 | a dropped field is orphaned and erased on request | 27y, 13e, 27h (rest) |
| 15 | **`prev` in the language.** | M | 8, syntax design | a declared rename renders as a rename | 27x |
| 16 | **Tenant builds.** After first deployment. | S | 3, 4b, 5, 10 | a beta tenant serves the branch while the rest serve main | 27t |

**Increment 3, what remains after session 2:**

- Session 2 built the revoked-version moves (§1.11): in the window, on
  demand, by the tenant when granted, each to the developer's target.
- Memory at 1,000+ spaces, measured and recorded here; `versions.max_loaded`
  and `versions.memory_budget` set from it; views evicted per space.
- Load guards: `versions.min_residency`, `versions.idle_evict_after`,
  `versions.max_concurrent_loads` as settings.
- Digest at load: recompute the source fingerprint, refuse a mismatch
  (§6 conflict 4).
- Per-version catalog and generated interface; functions per version
  (`functions.reload` serves the active version's only).
- Schedules and listeners as the union over served versions; cursor
  hand-over at a switch.
- Session 2 built `versions.served` and `versions.minimum` (§1.6). Left:
  the refusal of a rollout that strands a tenant (with rollouts), and a
  command that removes a stored version (`--move-to`).
- Sharing rule (§1.7), `versions_disagree`.
- Client contract (§1.9): version on every answer, the distinct refusal for
  an op the version lacks, the SDK's version-changed event, idempotency
  across one switch, the tenant admin's label.
- Stream close on a pin switch.
- Session 2 built per-node health on `/health/deploy`. Left: a version
  still loading is not shown apart from one that loaded.
- Session 2 built provisioning on the tenant's own version (§1.3).
- Session 2 built the console's parts: Deploys says `serving N tenants`
  and `active (new tenants)`; Tenants has a Version column
  (`std.deployment.tenants` carries `version` and `ordinal`). Left: the
  console has no screen to move a tenant, set its target, or name a
  revoke's `--move-to` for a version that is not active.
- Audit leftovers not taken by the CLI-output fixes: times as UTC, window
  caps as settings or recorded as fixed.

What sets the pace:

- Increment 3 is research on memory: say 2 tries.
- Increments 10b and 11 are research: say 2 to 3 tries, with a rehearsal
  against a real tenant's data.
- Increment 12b may change with the provisional design.
- Increment 15 waits on the syntax design (`forward-gaps` ask 23).
- 27z (the `acl` security audit) is outside this plan.

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
   `stored.version` through as a label. Increment 3 recomputes it.
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
| `versions.idle_evict_after` | 30 minutes | deployment |
| `versions.min_residency` | 10 minutes | deployment |
| `versions.max_loaded` | set from increment 3's memory measurement | node |
| `versions.max_concurrent_loads` | 1 | node |
| `versions.memory_budget` | set from increment 3's memory measurement | node |
| `pins.reconcile_interval` | 30 seconds (`ORDO_PINS_RECONCILE_SECS`, built 2026-11-05) | deployment |
| `forward.retry_interval` | 15 seconds | deployment |
| `editions.load_window` | 12 months | Ordo release |

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
  machine key with the `deployment` grant. A touch only at
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
