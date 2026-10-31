# Per-space database routing

**Checklist 18c and 18d. A space is pointed at a database, the way a space is already pointed at a bucket. One mechanism, three configurations: the shared default, a dedicated database on the platform's own cluster, and the tenant's own database elsewhere.**

**Status, 2026-09-20.** S1 to S7 are built **as libraries**, and the shipped binary constructs the single-database variant of every one of them, so every space in a running deployment resolves to the one database `ORDO_PG_DB` names. §9 verifies that against the tree, states the requirements to finish the feature, and §10 gives eleven increments smallest first. Also open: S6b (the attestation delivery drain's chain reads), migrations reaching every attached database (checklist 18d item 3, now §9.2), the supervisor's loops iterating `targets()`, and moving a live space (18e, post-release). All four questions in §8 are ruled; §11 holds the eight that still stand.

Companion documents: `plan-opslog.md` (the artifact placement this depends on), `security-logging-design.md` §6.1 (why the deployment stream cannot move), `docs/opslog-growth-explained.html` (what the streams cost), `corpus-rulings.md` 2026-09-12.

Two things are ruled and are not re-opened. **One tenant, one key, with encryption at the database rather than the cluster** (owner, 2026-09-12). And **the artifact placement**: a tenant's chain, its space-scope security stream and its blob tables live with its data; the deployment-scope security stream, identity, access and grants stay central (owner, 2026-09-12, "both makes sense").

One thing is corrected here. An earlier reading of "BYO-DB is already the deployment model" took it to mean database-per-tenant. It does not: it means a deployment runs against a Postgres you supply. What exists is one pool and one database for every space.

## The sizing that shapes this plan

The target is **tens of thousands of spaces in one deployment**. That number kills database-per-tenant as a default: ten thousand databases is roughly 75 GB of system catalog before any data, an autovacuum launcher with three workers cycling ten thousand databases, and migrations measured in hours. Connections are *not* the binding constraint, since a transaction-mode pooler on the cluster host opens backends only for concurrently active tenants, and idle pool definitions are cheap.

So routing must be **data-driven and sparse**: almost every space points at the shared default, a few point elsewhere. The costs above scale with the number of distinct **targets**, not with the number of spaces, and a deployment with fifty targets is comfortably inside every limit.

---

## 0 · The starting state, verified

### 0.1 One pool, one database, and the space is a column [settled]

`PoolConfig` is `{ host, port, user, db, size, acquire_timeout }` with a single `db: String` (`crates/ordo-exec/src/lib.rs:65-75`), populated from `ORDO_PG_HOST`, `ORDO_PG_PORT`, `ORDO_PG_USER` and `ORDO_PG_DB` (`:80-93`). `Pool::connect` dials every connection eagerly so a bad config fails at boot (`:322`). There is no `$spacedb` table, no per-space pool, and nothing anywhere maps a space to a connection.

### 0.2 The executor already pairs a space with a pool [settled]

`Executor` holds `pool`, `schema`, `registry`, `keys` and `space`, and the comment on the last one is the whole argument for this plan: *"The space this executor serves. Keys are per space, so this is the isolation boundary rather than a label"* (`lib.rs:571-583`). The pairing exists. What does not exist is any reason for two executors to hold different pools.

### 0.3 The precedent exists twice, and both halves are already argued [settled]

**A resolver trait.** `StoreResolver::resolve(&self, space: &SpaceId) -> Result<Arc<dyn BlobStore>, BlobError>` (`crates/ordo-blob/src/lib.rs:240-242`), with `SingleStore` as the default implementation, described as *"one store for every space, the default, and honest about the absence it stands in for"* (`:244-246`). That is exactly the shape a pool resolver takes, including the honesty of the default.

**An attachment table.** `$spacebucket` is one row per space with `ensure_table`, `lookup(pool, space)` and `attach(..)` (`crates/ordo-exec/src/bucket.rs:182`, `:189`, `:214`). Its module doc settles three questions a `$spacedb` faces identically:

- **The credential is a reference, never a value.** A secret in an op parameter is a secret in the idempotency cache, in every proxy log, and in the command log, which is plaintext by design; and with a reference, rotating a credential is not a schema-level act.
- **The custody choice is stored rather than inferred**, because a consent page has to state which one was taken.
- **Attaching or changing it is a granted, step-up-gated, command-logged operation**, so storage custody never changes silently.

`BucketCustody` carries `PlatformSealed` and `OwnPerimeter` with a `disclosure()` returning the sentence a consent page prints, written beside the mechanism because *"a disclosure assembled by a renderer is a disclosure that differs per renderer"* (`bucket.rs:104`).

### 0.4 Platform tables are created once, at boot, against one pool [settled]

`Pool::ensure_platform_tables` runs eight DDLs: `$replay`, jobs, the command log, security events, triggers, sealed payloads, `$blobref` and `$blobtomb` (`lib.rs:284-320`). It carries a documented retry because `CREATE TABLE IF NOT EXISTS` is not race-free and *"every deployment that scales horizontally meets it on the first simultaneous rollout"*. Under routing this function has to learn that different targets owe different subsets, which §3 settles.

### 0.5 Cross-space work assumes one connection reaches everything [open]

`PgSecurityStore::try_scopes` returns every scope present in one `$secevents` table (`crates/ordo-exec/src/secevent.rs:546`), and the retention pass walks that list. `blobgc::sweep` already takes a `space` parameter (`blobgc.rs:295-301`) but runs against one pool. Both become loops over targets.

### 0.6 A space can be provisioned, onto the default database only [settled 2026-09-13]

This section used to say nothing provisions a space. That stopped being true with the multi-space host: `ordo_host::provision::provision_space` mints custody, then the founding member row, and `ordo_host::spaces::Spaces` resolves an executor, a fixture and a chain per request. What provisioning cannot do yet is put a space anywhere but the default database, which is S5.

### 0.7 The append lock is per space and is unaffected [settled]

`TxConn::append_group` takes `pg_advisory_xact_lock(APPEND_LOCK_NS, space_lock_key(space))` (`crates/ordo-exec/src/commandlog.rs:120`, `:130`, `:197`). Advisory locks are per database, so moving a space to its own database changes nothing about its own serialisation and removes cross-space advisory-lock namespace sharing that was never contended anyway. **Routing does not raise any tenant's write ceiling**, which is a separate and currently unmeasured limit.

### 0.8 Cross-pod coordination rides Postgres, which routing splits [open]

`jobs.rs` states the design: *"every cross-pod concern riding Postgres, with no broker"*. Clock leadership is a session-scoped advisory lock, job claiming uses `SKIP LOCKED`, and invalidation is `LISTEN`/`NOTIFY` on `ordo.invalidate` (`jobs.rs:230`), consumed by `PgInvalidationBus` which owns a dedicated connection because `LISTEN` is session state the pool would lose (`jobs.rs:242-258`, `crates/ordo-host/src/invalidation.rs:209`). Deploy propagation has its own channel (`crates/ordo-host/src/deploy.rs:582`).

**This is the consequence most likely to be missed.** A `NOTIFY` reaches only listeners connected to the *same database*. A space on a different target publishes invalidations nobody on the control cluster hears, so `?live` subscriptions for that space go silently stale. The bus becomes per target, not per deployment. `lib.rs:195-201` already warns that the bus, not the `live_capable` flag, is *"the honest gate on `?live`"*, and this widens that gate rather than inventing a new one.

**Corrected 2026-09-20.** `PgInvalidationBus::connect` *is* in the serving path (`runtime.rs:1437`). What has no caller is `connect_routed`, the per-target form S7 built, so the sentence above now applies to the routed bus rather than to the bus.

---

## 1 · The seam [built 2026-09-13, S1]

A resolver, mirroring `StoreResolver` exactly:

```
pub trait PoolResolver: Send + Sync {
    fn resolve(&self, space: &SpaceId) -> Result<Arc<Pool>, PoolError>;
}

/// One pool for every space. The default, and honest about the absence.
pub struct SinglePool(Arc<Pool>);
```

Three implementations, which are the three configurations:

- **`SinglePool`** wraps today's behaviour. Every space resolves to the deployment's own pool, and a deployment that never attaches a database never notices this plan landed.
- **`AttachedPools`** reads `$spacedb`, holds a pool per distinct target, and falls back to the default for any space with no row. Pools are created lazily on first resolve and are keyed by target rather than by space, so a hundred spaces sharing a dedicated database share one pool.
- Tests keep whatever they use now.

**Resolution is by target, not by space.** This is the property that makes the sizing work: N spaces on one target cost one pool, and the pool count tracks distinct attachments.

**`Executor` takes a resolved pool rather than resolving.** The executor already carries a space and a pool; the resolver runs above it, at the point a request is bound to a space, so nothing inside the write protocol learns about routing. That keeps `run_write_on` and the revert applier ignorant of topology, which is what allows one code path to serve both configurations.

**As built** (`crates/ordo-exec/src/route.rs`, `crates/ordo-host/src/spaces.rs`). Three differences from the sketch above, each for a reason:

- **`resolve` returns a `Pool`, not an `Arc<Pool>`.** A `Pool` is already a shared handle, so wrapping it would add a second layer of sharing with nothing to share. `Pool::same_handle` is what a caller compares.
- **The resolver lives on `Spaces`, beside the chain logs**, for the logs' reason: a space's database belongs to the space and outlives every deploy. `Spaces::new` builds `SinglePool` over the pool the deployment booted with; `Spaces::routed` takes any other resolver. Export, erasure and the chain proof take the same `Arc` through `Spaces::pools`, so no surface holds a second answer to where a space's rows are.
- **The per-space view is cached only while the resolver's answer holds.** `Loaded::in_space` takes the resolved pool and re-cuts the executor when the handle changes. That is the part of a move this cache owes, and the owner's `routing-migration` ruling asks that routing be able to carry one. A request already in flight still finishes on the old database, which is the migration's own consistency argument and is not made here.

**Routed today:** the op path (reads and writes, through `Spaces::resolve`), the blob door's write and read, `std.space.export`, `std.space.delete`, and the chain proof. **Not yet routed, and each has an increment:** the idempotency cache `$idempotency` and the attestation sink table (which list each joins is S4), and the retention, collection and attestation drains (S6).

---

## 2 · The attachment [built 2026-09-13, S2]

`$spacedb`, on `$spacebucket`'s pattern and for its reasons:

```
"$space"       text PRIMARY KEY
"host"         text NOT NULL
"port"         int  NOT NULL
"user"         text NOT NULL
"db"           text NOT NULL
"credential"   text NOT NULL   -- a reference, never a secret
"custody"      text NOT NULL   -- platform-managed | own-perimeter
"attached_at"  bigint NOT NULL
"attached_by"  text NOT NULL
```

**No secret column**, asserted by a test that reads the DDL string, exactly as `bucket.rs` asserts its own. The credential is a label the operator installed out of band, resolved through the same pattern as keys.

**Custody is `PlatformManaged` or `OwnPerimeter`**, with a `disclosure()` beside it. The distinction is the same one buckets draw and it changes the same thing: under platform custody, destroying the space's keys ends readability, and the operator can verify the chain. Under the tenant's own perimeter the platform cannot reach into their database, so **crypto-erasure becomes an act the tenant performs** and chain verification requires their cooperation. A consent page has to say which one was taken.

**Attaching or moving a space is a granted, step-up-gated, command-logged operation.** Storage custody never changes silently, and the command group is the record of who moved a tenant's data and when.

**Moving a space between targets is out of scope for this plan.** Attachment at provisioning time is the increment; migrating a live space's data from one database to another is a separate build with its own consistency argument, and pretending otherwise would hide the hardest part.

**As built** (`crates/ordo-exec/src/spacedb.rs`). Columns as above, with `port` an integer. One difference from `$spacebucket`, and it follows from the paragraph above: **a different target for an attached space is refused** (`AttachRefusal::WouldMove`), where a bucket attachment replaces it. Replacing a bucket converts custody going forward while stored objects stay readable; replacing a database would point every later request at an empty one and leave the rows and chain behind. The same holds for a custody change on the same database, which changes what deleting a live space does. The attach is one insert that only lands where no row exists, so two racing attaches leave one winner, where the bucket's delete-then-insert would let the second silently replace the first. A stored custody this build cannot parse is an error rather than a fallback, because either fallback could print a false consent sentence. When the live move is built, `WouldMove` is the place it has to change.

---

## 3 · What lives where [settled by the owner, 2026-09-12]

**In the target database, because it travels with the data:** `$commandlog` and `$checkpoints`, the space-scope rows of `$secevents` and their checkpoints, `$blobref`, `$blobtomb`, `$sealedpayload`, `$triggers`, `$replay`, and every entity table.

**In the control database, because it is about more than one tenant:** the deployment-scope rows of `$secevents`, identity and authentication, access profiles, grants and share grants, `$spacebucket`, `$spacedb`, `$jobs` and `$schedules`.

**Two consequences that need building, not just deciding.**

`ensure_platform_tables` splits into two lists. A target database gets the space-scoped set; the control database gets both its own and, for the default target, the space-scoped set as well, since the default target *is* the control database in a deployment that has attached nothing.

**`$secevents` becomes two physical tables in two databases.** Today it is one table with a `scope` column carrying both halves, and `try_scopes` walks it once. The new shape is that the control database prunes the deployment scope and each target prunes its own space scopes. This makes the read-versus-prune custody split cleaner rather than harder, because the two streams can then have genuinely different signers.

**And one rule that follows from the deployment stream being central.** A caller can name a space that does not exist, so **the deployment stream must be writable without opening any target database**. That is true today by accident and becomes a requirement.

**Corrected 2026-09-13, while building S4: `$jobs` is owed by every target, not only the control database.** The write path enqueues its invalidation job into `$jobs` *inside the write's own transaction* (`TxConn::enqueue`, in `Executor::attempt`), which is how invalidation fires only on commit. A space on its own database therefore needs `$jobs` there, and claiming each target's jobs is a loop over targets (S6) beside the listener per target (S7). The same test, "does it have to commit with the space's writes", places every table §3 left unnamed:

- **In the target:** `$idempotency` (it holds sealed copies of the space's write responses, which are tenant data and belong under the tenant's custody), `$logconsumers` and `$attestations` (they describe positions in the space's chain and travel with it), and `$triggerbatches`, `$triggerruns`, `$deadletters` (they ride the write's transaction).
- **In the control database:** `$attestsinks`, `$attestdeliveries`, `$attestdeadletters` (delivery state for a control-plane job, holding the owner's signed destination), `$artifacts` and `$activations` (the program is deployment-wide), `$capabilitygrant`, `$delegation`, `$spacebucket` and `$spacedb`.

**So the DDL split turned out to be a routing split.** All eight boot-time platform tables are owed by every target, because the write path needs each of them inside the effect's transaction, and `Pool::connect` already creates them on every pool it opens. The control-only tables are created on demand by their own modules and only ever against the control pool. No second DDL list is needed; what needed building was sending each row to the right database, which for the security stream is S4.

---

## 4 · Provisioning [built 2026-09-14, S5]

**Built as S5, both halves** (`crates/ordo-host/src/provision.rs`: `prepare_target`, `provision_space_on`). Marked `[proposed]` until 2026-09-20; it was proposed when written and has been built since. One thing is still true of it and is why §9 exists: **nothing in production calls either function**, so no deployment attaches any tenant to anything. §10's T8 is that caller. The sequence as planned:

1. Mint custody: `create_space` in the key provider (`crates/ordo-crypto/src/custody.rs:668`).
2. Decide the target: the default, or a named attachment.
3. If a new target: create the database, run the space-scoped DDL set against it, and write the `$spacedb` row.
4. Seed membership, the access profile and the deployed artifact.
5. Append the command group that records all of it.

**Step 3 is not transactional with the rest**, because `CREATE DATABASE` cannot run inside a transaction in Postgres. So provisioning needs a resumable shape: the `$spacedb` row is written last, and a space with custody and a database but no row is an incomplete provision a retry completes. That asymmetry matches the blob door's, where the object is stored before the reference exists, for the same reason: prefer the failure that costs money over the failure that costs a guarantee.

---

## 5 · Cross-space work [built in part, S6 and S7]

**Built in part.** Security-event retention across targets and the idempotency cache per space are S6; the bus per target is S7. Blob collection, job claiming and the attestation writer still have no runner in the host, and §9.3's D-4 found that `$jobs` cannot carry a tenant across databases at all. The plan as written:

- **Security-event retention** (`crates/ordo-host/src/secdrain.rs`): one pass per target for space scopes, plus one for the control database's deployment scope.
- **Blob collection** (`blobgc::sweep`): already per space, now against the space's own pool.
- **Deploy propagation**: an artifact deployed to a space nudges that space's target.

And one **new** obligation, from §0.8: **an invalidation bus per target**. A `NOTIFY` reaches only listeners on the same database, so a node serving a space on an attached target must hold a listener against that target or that space's `?live` subscriptions go silently stale. This is the sharpest operational consequence of the plan, because the failure mode is silence rather than an error.

---

## 6 · What does not change [settled]

The `$space` column stays on every table, the primary key stays `("$space", "id")`, foreign keys stay composite, the planner keeps returning a table and its predicate as one value, and guards keep compiling into every query. All of it is redundant inside a single-tenant database and all of it stays, for three reasons: one code path serves both configurations, a tenant can be promoted from the shared target to their own without a schema change, and for the tens of thousands of spaces that will always share the default target these four mechanisms remain the **only** isolation they have.

The append lock, the write protocol, the chain encoding, sealing and the key hierarchy are all untouched.

---

## 7 · Increments

**S1 · The resolver seam, with one implementation. [built 2026-09-13]** `PoolResolver`, `SinglePool`, and every production construction site routed through it. Behaviour identical. *Done when* `every_space_resolves_to_the_default_pool` passes and the full suite is unchanged. *Size: small.* Proved beyond its done-when by `a_space_is_served_from_the_database_it_resolves_to`, which routes one space to a second real database and shows its upload lands there and its read is answered from there, and by `a_space_that_moves_is_cut_again` (`crates/ordo-host/tests/spaces.rs`).

**S2 · The attachment table. [built 2026-09-13]** `$spacedb` with `ensure_table`, `lookup`, `attach`, the custody enum and its disclosure, and the no-secret-column assertion. Nothing reads it yet. *Done when* `the_attachment_table_has_no_secret_column`, `a_custody_choice_states_its_disclosure` and a round trip pass. *Size: small.* Also pinned: a different target is refused and leaves the row untouched, racing attaches leave one winner, and an unknown custody is an error (`crates/ordo-exec/tests/spacedb.rs`).

**S3 · `AttachedPools`. [built 2026-09-13]** Resolution by target with lazy pool creation and fallback to the default. *Done when* `two_spaces_on_one_target_share_one_pool`, `a_space_with_no_row_uses_the_default` and `an_unreachable_target_fails_at_resolve_not_mid_write` pass. *Size: a day.*

*As built* (`crates/ordo-exec/src/route.rs`, tests in `crates/ordo-exec/tests/route.rs` and `crates/ordo-host/tests/spaces.rs`):

- **The resolver became asynchronous**, because the answer is in a database and a target has to be dialled. The trait returns a boxed future so it can stay `dyn`; `Spaces::resolve` and `Spaces::pool_of` are `async` with it.
- **A pool per target**, keyed by the attachment's identity, dialled once even under concurrent first requests, and a failed dial is not cached.
- **Credentials.** `PoolConfig` had no password at all. It now carries one, redacted from `Debug`, and `DbCredentials` turns an attachment's label into it. A label the node does not hold is refused before any dial, so a target that would accept an unauthenticated connection is never reached that way by accident.
- **Dials are bounded** by the checkout timeout, where they had no bound and a target dropping packets would have held a request for the operating system's TCP timeout.
- **An unreachable target is refused, never substituted.** A failed dial for an attached space does not fall back to the default database, which would serve the space from the wrong place; the host test attaches the twin to a dead port, leaves its row in the default database, and shows the upload is refused and the row untouched while the other space keeps working.
- **Caching.** An attachment is cached for the process's life, sound because S2 refuses to change one. A space with no row is cached for 30 seconds, because it is almost every space and a control-database round trip per request is the cost of not caching. The test `a_miss_is_remembered_until_it_expires_or_is_forgotten` pins that window rather than hiding it.

**Two obligations this hands to S5, which provisioning has to keep.** (1) **Attach before the founding member exists.** No request can be authorized in a space before it has a member, so if the attachment comes first, no node can have cached a miss for a space that is about to be attached. The 30-second window only matters if this ordering is broken. (2) **Attach only a space with no rows in the default database.** S2 refuses a *different* target for an attached space, but it cannot see that an unattached space already holds rows on the default. Attaching such a space is a live move under another name, and S5's op has to check before it writes the row.

**S4 · The platform table split. [built 2026-09-13]** Two DDL sets, and `$secevents` split by scope across databases. *Done when* `a_target_database_has_no_deployment_scope_rows`, `the_control_database_holds_the_deployment_stream` and `a_denial_naming_an_unknown_space_is_recorded_without_opening_a_target` pass. *Size: a day.*

*As built.* The two DDL sets were not needed (see the correction under §3). The security drain (`crates/ordo-host/src/secdrain.rs`, `SecurityDrain::start_routed`) sends deployment-scope entries to the control database without calling the resolver, and each space's entries to the database it resolves to. A pass is split by destination first and batched second, so a destination that fails requeues only its own suffix: other destinations are still written, nothing committed is requeued, and no scope is reordered, since a scope lives in one destination. Tests in `crates/ordo-host/tests/secdrain_routing.rs`; the first two done-when names are one test, `a_target_database_has_no_deployment_scope_rows`, which asserts both halves, and `an_unreachable_target_holds_back_only_its_own_spaces` pins the failure path.

**S5 · Provisioning. [built 2026-09-14]** The five-step sequence, resumable, with the `$spacedb` row written last. *Done when* `provisioning_creates_custody_database_and_row`, `an_interrupted_provision_is_completed_by_a_retry` and `provisioning_appends_one_command_group` pass. *Size: more than a day. Depends on 11-27's finding that nothing provisions a space today.*

*First half built 2026-09-13* (`ordo_host::provision::prepare_target`, tests in `crates/ordo-host/tests/provision_target.rs`). Everything up to, and not including, the founding member:

1. The space id and the database name are checked.
2. The space must hold no rows on the default database (`erase::rows_held`, which reads erasure's table list minus the two attachment tables) and must not be attached elsewhere. Both are checked before anything is written, so a refused call leaves no keys and no empty database.
3. Custody is minted.
4. The database is created, with "already exists" counted as done (`ordo_exec::create_database`).
5. The schema is applied as one all-or-nothing statement, skipped if `members` already exists (`spacedb::apply_schema`).
6. The attachment is written with `attached_by` set to `system:provisioning`, per the Q-3 ruling.
7. One custody event is recorded on the deployment's security stream, and only when this call attached.

**One departure from the done-when:** `provisioning_appends_one_command_group` became a single custody event on the deployment stream. A command group needs an actor inside the space and a chain to join, and at this point the space has neither. The ruling names the system as the actor, and the deployment stream is where the platform's custody acts already go.

**One bug the tests caught on the way:** `rows_held` first counted the space's own `$spacedb` row as data, so every retry refused itself. The attachment tables are now excluded by name (`erase::ATTACHMENTS`).

*Split in two, 2026-09-25 (11-15).* Steps 1 and 2 above, plus a dial of the target cluster, are now `provision::preflight` — `bootstrap.md` §1.2's **phase 0**, which writes nothing that names the space. Steps 3 onward are `provision::commit_target`. `prepare_target` is the two of them in sequence and is unchanged for every caller it had. The seam exists because `std.space.create` has to put the registry row **between** them: §4.4 wants the row before any external resource, and §1.2 wants every decidable failure refused before the row, and those two only fit together if preflight is separately callable. **The `Preflight` value carries the `ApplyLock`** so R-S5's window does not reopen across the registry write — see §9.5 — and dropping the value is the whole of abandoning a plan, which is what `dryRun` does.

*Second half built 2026-09-14:* `provision_space_on` writes the founding member into the target, last. `directory::routed_engine` builds the door's access engine from every space's own database: unattached spaces from the control database, each attached space from its target, a copy left on the default for an attached space ignored, and a target it cannot read reported in `Directory::unread` rather than failing the whole build.

**S6 · The loops. [core built 2026-09-14]** Retention, blob collection and deploy propagation per target. *Done when* each runs against two targets in a test and touches neither the other's rows. *Size: a day.* Added 2026-09-13 from S3 and S4: **claiming each target's `$jobs`**, **the idempotency cache resolving per space** (its home is the target, per §3), and **security-stream retention walking every target** (S4 routes appends; retention still walks the control database only, so an attached space keeps its whole stream until this lands).

*As built* (tests in `crates/ordo-exec/tests/route.rs`, `crates/ordo-host/tests/secdrain_routing.rs`, `crates/ordo-host/tests/replay_routing.rs`):

- **`PoolResolver::targets()`** lists the default and each distinct target once, however many spaces share it, and names a target it cannot reach rather than leaving it out.
- **Security-stream retention walks every database** (`secdrain::retain_everywhere`): the control database first, then each target. The drain calls it after claiming the deployment's retention tick, so a fleet still runs one pass per period.
- **And so does the checkpoint sweep** (`secdrain::checkpoint_everywhere`, 7d, 2026-09-26), through the same `databases()` helper rather than a second copy of the walk — a second copy would be a second answer to *"where do this deployment's streams live"*, which is the shape in which one pass visits a target the other does not. **The reason is stronger here than for retention:** a database retention misses keeps too much history, which is a storage cost. A database the checkpoint pass misses has streams nothing signs, so `ordo verify` on any of its tenants reports a green walk it cannot back with an end-of-stream check — a silent weakness in the *answer*, not in the storage.
- **The idempotency cache keeps each space's records in the space's database** (`PgReplayCache::routed`), creating its table in each database on first use and answering `InFlight` when the space's database cannot be reached, as it already did for an unreachable cache.

**Three of the loops this plan named have no runner in the host at all:** blob collection (`blobgc::sweep`), job claiming (`claim_jobs`, `claim_due`), and the job that writes `$attestations` (`cadence::attest_due`). Each is a library function called only by tests. The supervisor that will run them (R3) owes one thing to routing: iterate `targets()` rather than the control pool.

**Moved to S6b:** the attestation delivery drain. Its sinks, deliveries and outbox stay on the control database; its two chain reads (`cadence::attestations`, `verifiable_from`) must move to the space's database, and the drain turns database errors into its own `Fault` type in ways that need reading before they are changed.

**A gap found while listing what walks every database, and it is larger than S6:** a schema change deployed after a space is attached never reaches that space's database. Provisioning applies the schema once (`spacedb::apply_schema`), and the load gate verifies only the default. So attached databases would drift from the deployed schema. This is 18d's "migrations applied N times", which is not built, and it has to land before a second schema version is deployed to a deployment with an attached space.

**S7 · The invalidation bus per target. [built 2026-09-14]** A listener per attached target, and `?live` proven across a space on a non-default target. *Done when* `an_invalidation_on_an_attached_target_reaches_a_subscriber` passes. *Size: a day, and it should not be skipped, because its failure mode is silence.*

*As built* (`PgInvalidationBus::connect_routed`, test `crates/ordo-host/tests/invalidation_routing.rs`): one `LISTEN` session per database that `targets()` lists, all feeding one fan-out; `refresh()` adds sessions for databases attached since the bus connected; `unheard()` names a database it could not listen to, and the others keep working. The single-database `connect` is unchanged. **What a deployment still owes:** every node has to call `refresh()` when a database is attached, on a tick or on a signal from the control database. Until a node does, it does not hear that database, and its `?live` subscribers for spaces there go stale, which is the silence this increment exists to remove.

**S8 · The record. [done 2026-09-14]** This plan's answers into `plan-opslog.md`, the checklist rows, and `corpus-rulings.md`. *Size: small.*

---

## 8 · Questions for the owner

**Q-1 · Does the default target stay the control database, or become an ordinary attachment?** Keeping them the same means a deployment that attaches nothing is exactly today's deployment, and the control database also holds every shared tenant's data. Separating them means even the shared tier is a target, which is tidier and costs every small deployment a second database.

**Recommendation: keep them the same.** The tidiness is not worth making the simplest deployment two databases, and S1's `SinglePool` then genuinely changes nothing on the day it lands.

**Q-2 · Is moving a live space between targets in scope at all?** §2 excludes it. The honest version is that promotion from shared to dedicated is the feature customers will ask for the moment they hear the tier exists.

**Recommendation: out of scope here, and filed as its own item.** It needs a consistency argument this plan does not make: the chain must not fork, the undo horizon must survive, and blob references must be repointed or the objects moved. **Filed as roadmap 18e** (post-release), per the owner's `routing-migration` answer.

**Q-3 · Who may attach a database?** Bucket attachment is space-owner consent, step-up gated and command-logged. A database attachment is strictly larger, since it decides where the chain itself lives.

**Recommendation: the same gate, plus the deployment's own administrative grant.** A space owner consents to their data moving; the operator is the party who can actually provision the target.

**Ruled 2026-09-13 (`db-attach-gate`): the system owns it.** At provisioning the deployment chooses the target with no consent step, because there is no owner yet to consent and no space without storage. A change after creation needs both parties, and that is the post-release move. How it is presented to customers is the app developer's. See `corpus-rulings.md`.

**Q-4 · Where do a space's members live once the space has its own database?** Found 2026-09-13 while starting S5, and it blocks S5's founding-member step. The door builds its access engine by reading `members` from one database (`deployment_engine`), but `members` rows are entity rows that other entities reference through composite foreign keys, and a foreign key cannot cross databases. Members only in the target: the door never sees them. Only in the control database: the space's rows cannot reference them. In both: a write across two databases with no transaction over it.

**Recommendation: members live in the space's database, and the engine is built by reading `members` from every target**, one query per target. It is the loop S6 already needs, there is no copy to drift, and export and erasure carry membership with the space. Refreshing a live engine stays the open item it already is.

Both Q-3 and Q-4 are on the decisions doc (`db-attach-gate`, `membership-home`).

**Ruled 2026-09-14 (`membership-home`): in its own database.** The founding member is written to the space's target, and the access engine reads each space's members from the database that space resolves to, ignoring any copy on the default for an attached space. See `corpus-rulings.md`.

---

## 9 · Requirements to finish per-tenant database resolution [gathered 2026-09-20]

**Requirements and increments, not a build.** Every claim below was checked
against the tree rather than taken from §0 to §8, because much has landed since
those were written. §10 holds the increments and §11 the decisions that are the
owner's.

**The object-storage half is `plan-tenant-storage.md`**, gathered in lockstep
on the owner's instruction that the same per-tenant and per-deployment S3
configuration must exist. It mirrors this section's structure, numbers its
requirements `B-…` against these `R-…`, and names four of its increments as
having to ship **with** a `T` here rather than after it: its credential
provider with T3, its failure modes with T5, its usage work with T9, and its
sweep runner after T2. Read the two together; neither is complete on its own.

### 9.0 The state of this plan against the tree [verified 2026-09-20]

**The headline, and the reason these requirements exist.** S1 to S7 are built
as libraries, and the shipped binary wires the single-database variant of
every one of them. `crates/ordo-host/src/runtime.rs:1488` builds
`Spaces::routed` over `ordo_exec::route::SinglePool::new(deployment.current()
.exec.pool.clone())`, so `AttachedPools` is never constructed outside tests
and every space in a running deployment resolves to the one database named by
`ORDO_PG_DB`. The same holds at three more sites:

| Routed variant, built | What the binary constructs instead |
|---|---|
| `AttachedPools` (`route.rs:217`) | `SinglePool` (`runtime.rs:1488-1492`, and `runtime.rs:957` for the exit ops) |
| `directory::routed_engine` (`directory.rs:49`) | `deployment_engine_pooled` (`runtime.rs:893`) |
| `PgInvalidationBus::connect_routed` (`invalidation.rs:280`) | `PgInvalidationBus::connect` (`runtime.rs:1437`) |
| `Installed` as a `DbCredentials` (`route.rs:141`) | nothing; no production code builds one |

So the honest status is: **the mechanism is built and proven in tests, and no
deployment can turn it on.** There is no configuration that selects
`AttachedPools`, and no configuration that installs a credential label, which
§9.1 is about. Nothing here is a design change; what is missing is the wiring,
the configuration and the four obligations below.

**What is stale in this document.**

- The **status line of 2026-09-14** described S1 to S7 as built and left four
  items open. Three of those four are still open and a fifth has appeared
  (the wiring above). Corrected at the head of this document.
- **§0.1** describes a tree with no `$spacedb` and no resolver. It is labelled
  "the starting state" and is kept as history, not as a description of today.
- **§4 Provisioning** is marked `[proposed]`. It is built: `prepare_target`
  and `provision_space_on` (`crates/ordo-host/src/provision.rs`), S5, both
  halves. Re-marked.
- **§5 Cross-space work** is marked `[proposed]`. Retention across targets,
  the idempotency cache per space and the bus per target are built (S6, S7);
  blob collection, job claiming and the attestation writer still have no
  runner in the host. Re-marked with the split named.
- **§0.8's** closing note, that `PgInvalidationBus` is constructed in tests and
  not in the serving path, is **no longer true of the bus and is now true of
  the routed bus**: `runtime.rs:1437` connects the single-database bus in the
  serving path, and `connect_routed` is the one with no caller.
- **§2's** "moving a space between targets is out of scope" stands, and §9.7
  states it as a constraint rather than re-opening it.

**What has landed since that this plan never accounted for**, each of which
carries a requirement below: multi-space hosting and the runtime binary
(§9.0), schema apply (13b, §9.2), import (11-10a) and restore (17j) (§9.6),
metering and the usage rollup (17n, §9.6), per-space pool caps (20a, §9.5),
and the 17aa quarantine shape (§9.4).

**And one ruling lands in the middle of it.** *A tenant is an id, and its name
is an attribute* (`corpus-rulings.md`, 2026-09-21): a tenant is identified by a
minted, opaque, globally unique id, and human names are attributes that are
not unique anywhere. `$spacedb` is keyed on `"$space"`, so the attachment is
already keyed on the id and needs no schema change. What it does change is
every operator-facing surface this plan adds: an opaque id is unreadable, so
an attachment listing, a quarantine report and a rollup row have to carry the
name attribute beside the id for a human to act on, while every join, cache
key and audit entry uses the id alone. Stated once here and assumed
throughout.

### 9.1 Configuration: the default cluster, a tenant's cluster, and where the credential lives

**The default cluster is what the deployment already boots with, and Q-1
stands.** `PoolConfig::from_env` reads `ORDO_PG_HOST`, `ORDO_PG_PORT`,
`ORDO_PG_USER`, `ORDO_PG_DB` and the password, and `Pool::connect` dials
eagerly so a bad default fails at boot rather than at the first request
(`crates/ordo-exec/src/lib.rs:120-140`, `:322`). The default target stays the
control database (Q-1, ruled 2026-09-13), so a deployment that attaches
nothing is byte-for-byte today's deployment. **No new configuration for the
default cluster.**

**R-C1 · Routing is selected explicitly, never inferred.** One variable names
the resolver, in the style `ORDO_KEY_PROVIDER` already sets: `ORDO_PG_ROUTING`,
with `single` (today) and `attached`, and no default that changes behaviour.
The key provider states the reason in the tree: *"custody is selected
explicitly, never inferred from which other variables are set"*
(`runtime.rs:459`). Inferring routing from whether `$spacedb` holds rows fails
in both directions. A control database restored from elsewhere, or left over
from a test, would start a deployment dialling databases nobody configured it
to reach. And a deployment that meant routing but has attached nobody yet
would be indistinguishable from one that did not, so the misconfiguration
stays invisible until the first tenant is attached, which is the worst moment
to find it. `attached` also requires `R-C2`'s credential provider, and a boot
with `attached` and no credential provider refuses.

**R-C2 · A tenant is pointed at a cluster by its `$spacedb` row, keyed on the
minted id.** The row exists and is built (§2): `host`, `port`, `user`, `db`,
`credential`, `custody`, `attached_at`, `attached_by`, primary key `"$space"`.
Under the 2026-09-21 ruling the key is the tenant's minted opaque id and no
schema change is owed, because the column was always the space string. The
tenant's human name is an attribute elsewhere and is **never** part of the
attachment, the pool key (`DbAttachment::identity`) or the audit entry; it
appears only where §9.6 requires it for a human to read. Writing the row is
`spacedb::attach` through `provision::prepare_target`, and a different target
for an attached tenant is refused (`AttachRefusal::WouldMove`).

#### Where the attachment's credentials live

**The rule, and it is already three-quarters enforced.** `$spacedb` has **no
secret column**, asserted by a test that reads the DDL string
(`the_attachment_table_has_no_secret_column`). The reason is `bucket.rs`'s and
it is not a preference: the command log is plaintext by design, so a secret
passed as an op parameter is a secret in the idempotency cache, in every proxy
log and in the command log, and rotating it would become a schema-level act.
A connection string with a password in a table fails all three.

**What exists.** `DbCredentials::password(label) -> Result<Option<String>,
String>` and `Installed`, a `HashMap<String, Option<String>>`
(`crates/ordo-exec/src/route.rs:131-177`). A label the node does not hold is
refused **before any dial**, so a target that would accept an unauthenticated
connection is never reached by accident. `PoolConfig`'s password is redacted
from `Debug`, written out rather than derived (`lib.rs:99-110`).

**What does not exist, and is the gap.** No production code constructs an
`Installed` or any other `DbCredentials`. The only helper,
`Installed::as_configured`, reads `ORDO_PG_PASSWORD` and is a test convenience
for targets that sit on the deployment's own server. So there is no way for an
operator to install a credential label, and therefore no way to attach a
tenant to a cluster that asks for a password.

**R-C3 · The credential provider follows the shape the deployment already
has, which is a named provider and material from a file.** Two precedents in
the tree agree on this. `PoolConfig::from_env` prefers `ORDO_PG_PASSWORD_FILE`
over `ORDO_PG_PASSWORD` and says why: *"a file is the form an orchestrator's
secret store mounts... an environment variable is readable from
`/proc/<pid>/environ` and printed by every `docker inspect`"*. And
`sealed-master` takes two paths, the sealed file and its passphrase, kept
separable because *"neither alone opens anything"*. So:

- `ORDO_DB_CREDENTIALS` names the provider, refusing an unknown value the way
  `ORDO_KEY_PROVIDER` does rather than falling back.
- **`dir`** (the one v1 provider): `ORDO_DB_CREDENTIALS_DIR` is a directory,
  the label is the file name, the file's trimmed contents are the password,
  and an empty file means passwordless (peer, trust or certificate
  authentication), which is the `Ok(None)` the trait already carries. A file
  whose mode is group- or world-readable refuses at boot, matching the
  `0o600` the master key is written with (`ordo-crypto/src/master.rs:331`).
  This is exactly what a Kubernetes secret, a Docker secret and a Vault agent
  sidecar all mount, so it needs no integration per manager.
- **The whole directory is read at boot** and refuses there, for the reason
  every other secret refuses at boot: a credential that is wrong should cost
  milliseconds, not a tenant's first request.
- **Rotation** is an operator changing a file and signalling the node to
  re-read, which is a reload of the provider and never a write to `$spacedb`.
  That is the property the reference-not-value rule was chosen for, and it is
  why a label must be stable across a rotation: the row names the label, the
  file behind the label changes.
- **A KMS or external manager is deliberately not v1.** The `dir` provider
  reduces every manager to a mount, and adding a second provider later needs
  no change to `$spacedb`, the resolver or the audit trail.

#### What the door must never return

**R-C4 · Five refusals, each with a test.**

1. **No surface returns a password.** Not an attachment read, not the attach
   op's response, not an error. `Debug` redaction on `PoolConfig` is pinned by
   a test, and the same test covers any type that comes to hold one.
2. **No surface returns a credential *label* to a tenant.** The label names a
   secret installed on the node, it is deployment-side configuration, and a
   tenant has no use for it. It is returnable to a deployment administrator.
3. **`host`, `port`, `user` and `db` are returnable to a deployment
   administrator and to the tenant whose attachment it is**, because under
   `OwnPerimeter` it is their own cluster and they already know it. Never to
   another tenant, and never on an unauthenticated path.
4. **A dial failure's text is never passed through to the caller.** A Postgres
   connection error carries the host, the user and often the database name, so
   a caller who can provoke one could map the deployment's topology. The
   caller gets the quarantine answer of §9.4 with an opaque reason; the detail
   goes to the deployment-scope security stream and the operator log, which
   §9.6 requires an operator to be able to read.
5. **The existence of an attachment is not a public fact.** A request naming a
   tenant that does not exist and one naming a tenant on an unreachable
   cluster must be indistinguishable to an unauthenticated caller, or the
   attachment table becomes an oracle for which tenants exist.

### 9.2 Schema on a fresh cluster, and on a schema change

**What exists.** `spacedb::apply_schema(pool, ddl)` sends the whole DDL as one
simple-protocol query, which Postgres runs as one implicit transaction, so it
applied completely or not at all; and it skips when `members` is already
present, which is how a retry knows whether it ran
(`crates/ordo-exec/src/spacedb.rs:203-224`). `provision::prepare_target` runs
it against a newly created database, taking the DDL from the caller as *"the
deployment's active schema, which the new database receives"*
(`provision.rs:397`). 13b's apply path (`crates/ordo-host/src/schema_apply.rs`)
plans, triages, executes and records against **one** `Pool`, writing a
`$migrations` row and a deployment-scoped `SchemaMigration` event.

**The two gaps, stated precisely.**

1. **A fresh target gets a schema and no record of which one.** `apply_schema`
   writes no `$migrations` row, so a database provisioned today has an unknown
   schema version and the next apply has to infer it from the live shape.
2. **A schema change never reaches an attached database.** `apply` runs against
   the pool it is handed, and the load gate's live-schema comparison
   (`Verified::from_gate`, `deploy.rs:252`) checks that same one. Both are the
   control database in the binary today. This is 18d item 3, and it has to land
   before a second schema version meets an attached tenant.

**R-S1 · A fresh cluster receives the serving version, and records it.**
`prepare_target` takes the DDL of the **currently serving artifact**, read from
`Deployment::current()` at attach time rather than passed by whoever called,
and writes the matching `$migrations` row into the new database inside the same
all-or-nothing apply. A database with `members` and no `$migrations` row is a
pre-R-S1 database and is treated as "version unknown", which R-S4 reports and an
operator resolves once.

**R-S2 · `$migrations` lives in every database, because it describes that
database's shape.** It joins §3's "in the target" list. The control database
additionally holds a **fleet view**: one row per target with the version it is
known to be at and when that was last confirmed, which is what §9.6's operator
surface reads and what R-S4 refuses on. The fleet view is a cache of the
targets' own rows and is never the authority; a target's `$migrations` is.

**Built 2026-09-25** as `ordo_exec::fleet` (`$fleet`: `label`, `version`,
`confirmed_at`). Three things it does that the paragraph above only implies:

*The cache is a copy, and the code makes it one.* Each target's version is
**read back from that target's own `$migrations`** after the apply and cached
from there, rather than asserted to be the program's version. Asserting the
program's version would make `$fleet` a second claim about the target rather
than a copy of the target's, and the two could then disagree with nothing to
arbitrate them. The tests compare the two rather than either against a
literal.

*There is no `version()` without `confirmed_at`.* A caller that could read
the version alone would be treating the cache as an authority, which is the
one thing this requirement forbids. C-M1 permits this table only as a
*"marked stale report"*, and it keeps that permission only while nothing
routes on it: no pool is resolved, no request gated and no apply refused from
`$fleet`. Those read `spacedb::recorded_version` against the target.

*The key is `route::target_label`, which now has one definition.* `default`,
or `host:port/db`, from one function rather than a `format!` at each site,
because the attach and the apply walk both write this table and two
spellings of one database would be two rows describing one target. It
carries no credential, which is what keeps it printable.

*Writers:* the apply sequence (every walked target, plus the control
database, which the walk deliberately excludes because `sequence` applies it
through the schema step) and `prepare_target` (so a tenant attached and never
applied to again is not missing from the listing until the next deploy).
Both are best-effort and neither fails its caller: a stale `$fleet` is a
worse diagnostic, not a wrong placement.

**A target that has never reported is absent rather than present-with-null**,
because this table records observations and there has been none. A caller
listing the fleet reconciles against `spacedb::all` to find attachments with
no report yet. **Last writer wins**, including when the version went
backwards: a replica that observed an older version later is reporting a real
restore, and a cache that refused it would hide exactly that.

**R-S3 · An apply walks `targets()`, and the order is: every target, then the
activation.** No transaction spans N+1 databases, so the design has to say what
a partial apply means rather than pretend it cannot happen.

- `apply_everywhere` resolves `PoolResolver::targets()`, runs 13b's existing
  per-target sequence unchanged against each (tables and alters in one
  transaction, then each `CREATE INDEX CONCURRENTLY` on its own), and returns a
  per-target outcome. A target it cannot reach is an outcome, not a panic, for
  the same reason `targets()` names a target it cannot reach rather than
  leaving it out.
- **The control database is applied as one of the targets**, because under Q-1
  it *is* the default target and holds every shared tenant's rows. It is applied
  **last** among the targets, so the database that records the fleet's state is
  never ahead of the fleet.
- **The activation follows the applies, and only lands when every target
  reported success.** `$artifacts` and `$activations` are deployment-wide (§3),
  so a program activated while a target is behind would be a program whose
  tables that tenant's database does not have.
- **The `SchemaMigration` security event is written once, to the control
  database, naming every target and its outcome.** It is deployment-scope, so
  S4's drain already sends it there without opening a target.

**Changed 2026-09-29 (25i): the activation is gated on the deployment's own
database, and not on every tenant's.** With one tenant per person, a gate on
every database lets one unreachable or refused tenant stop every deploy. A
tenant that could not be brought forward is closed alone by R-S4's quarantine
and the catch-up (`tenant_forward`) retries it. The walk is bounded (8 at a
time, 20 seconds each), the report names each database that was held, and a
staged apply (`--no-activate`) touches no tenant. `plan-schema-evolution.md`
§8.3 has the whole account.

**R-S4 · A target that is behind the activated artifact quarantines its
tenants by default, and a tenant may hold a contract that says otherwise.**
Serving a tenant from a database that is one version behind means a program
referencing tables that are not there, which fails per statement, late, and
differently for each request. The 17aa shape gives the honest answer instead:
that tenant is quarantined with the reason "schema behind", every other tenant
and the control plane keep serving, and `/health/ready` names it. **That is the
default and it is what a tenant with no contract gets.**

**Owner-ruled 2026-09-21, and this reverses the earlier "explicitly not in
scope":** *"It should be configurable per tenant. This would be a deployment or
possible maintenance window contract."* A tenant whose contract names a
deployment or maintenance window is served the artifact its database matches
until that window opens. So a deployment does hold more than one artifact, for
contracted tenants only, and the resolver picks per tenant rather than per
deployment. The bound on how far back that reaches is **open as D-B2**: the
proposal is one version back and never two, an expiry on every contract, and an
apply that reports at activation time which contracted tenants a destructive
change would break. Until D-B2 is ruled, nothing here is built beyond the
default quarantine, which T-series increments already carry.

**R-S5 · Attach and apply are serialised against each other.** A tenant
attached while an apply is walking `targets()` would receive whichever schema
the attach read and would never be revisited by the walk that is already past
it. One advisory lock in the control database, taken by both `prepare_target`
and `apply_everywhere`, removes the window; the loser waits rather than
refusing, because an attach that refuses during every deploy is an attach that
looks broken. The lock is in the control database and is the same kind of
mechanism `commandlog.rs` already uses for the append lock, not a new one.

**Built 2026-09-24, and the lock is `apply_op::ApplyLock`** — which already
existed and already guarded the apply side across the walk and the
activation. `prepare_target` takes the same one, so this requirement added a
caller rather than a mechanism. Two things written first and removed: a new
session-scoped lock, on the false reasoning that `CREATE DATABASE` forbidding
transactions means a transaction-scoped lock cannot span the attach (the
lock's transaction is on a dedicated connection and never runs the `CREATE`),
and a claim that advisory locks ignore `lock_timeout`, which `ApplyLock`
disproves by using it. **The wait is 30s and a timeout is `RoutingBusy` →
503**, never a 400: nothing about the call was wrong. **An attach now holds
two control connections** for its duration, the lock's and its reads'.

**And it survived the phase-0 split, 2026-09-25 (11-15).** Splitting
`prepare_target` into `preflight` and `commit_target` is exactly the shape that
could have reopened this window: take the lock for the occupancy read, release
it, reserve the registry row, take it again for the attach, and an apply walks
between the two. So the lock is **held in the `Preflight` value** rather than
inside either function — taken in phase 0, carried across
`tenants::reserve`, released when the value drops. `commit_target` takes the
value by ownership, which makes "commit without having preflighted" and
"preflight twice and commit against the older answer" both unrepresentable. The
two-connection cost is unchanged: `reserve` is one more statement on the
connection the reads were already using.

**R-S6 · The load gate runs per target at boot, and reports rather than
refuses.** `Verified::from_gate`'s live-schema comparison runs against each
target the boot can reach. A target whose shape does not match the serving
artifact quarantines its tenants per R-S4. The process still boots, per the
owner, 2026-09-21: *"the system should not be dependent on the rest of the data
designs, deployment space, or tenants"*. A target the boot cannot reach at all
is the same answer as an unreachable target in §9.4 and uses the same register.

### 9.3 What stays in the deployment's database and what travels with the tenant

**§3 settled this and the tree half-agrees.** Verified by reading where each
table is *written*, not where the plan says it belongs. The third column is the
running binary, which matters separately because `runtime.rs` installs
`SinglePool` (§9.0).

| Concern | §3 says | Written through | Verdict |
|---|---|---|---|
| `$commandlog`, `$checkpoints` | target | `TxConn` on `Executor.pool`, set per space by `Loaded::in_space` | **agrees**, structurally routed |
| `$sealedpayload` | target | the write protocol on `Executor.pool` | **agrees** |
| `$blobref`, `$blobtomb` | target | the blob pipeline on `Executor.pool`; `blobgc` takes an explicit pool | **agrees** |
| `$replay` / `$idempotency` | target (§3's correction) | `PgReplayCache::routed` exists; `runtime.rs:1311` calls `new`, which wraps `SinglePool` | **drift**, D-1 |
| `members`, the directory | target (`membership-home`, 2026-09-14) | `directory::routed_engine` exists; `runtime.rs:893` calls `deployment_engine_pooled` | **drift, and the sharpest**, D-2 |
| `$secevents`, space scope | target | `SecurityDrain::start_routed` exists; `runtime.rs:1407` calls `start` | **drift**, D-3 |
| `$secevents`, deployment scope | control | the drain sends it to control without asking the resolver | **agrees** |
| `$jobs`, `$schedules` | target (§3's correction) | enqueued on `Executor.pool`; **consumed only from the control pool** | **contradiction**, D-4 |
| custody / `$keycustody` | unnamed by §3 | `PgKeyStore::open(cfg.pg)` at `runtime.rs:1833`, always the control config | **correct, and should be stated** |
| `$chainwitness` | unnamed by §3 | `Witness::new(control)`, control only by explicit module doc | **correct, and should be stated** |
| identity, `$credential`, `$principal`, `$sessionrevocation` | control | `open(cfg.pg)` at `runtime.rs:1341-1353` | **agrees** |
| `$quotas` | unnamed by §3 | control, keyed by space | **agrees with §3's reasoning** |
| `$usage` (17n) | unnamed by §3 | `UsageRollup::run` walks `pools().targets()` for the counts; writes summary rows to control | **agrees, and is the one worker already built for this** |
| `$usage_replica` | unnamed | control, per replica | **agrees** |
| `$spacebucket`, `$spacedb`, grants, delegation, `$artifacts`, `$activations`, attestation sinks and deliveries | control | control | **agrees** |

**R-H1 · Four drifts to close, and they are §10's wiring rather than new
design.**

- **D-1 · The idempotency cache.** Its module doc says its rows live with the
  space, and `PgReplayCache::routed` was built for that. The binary calls `new`.
  There is a structural reason: `spaces` is constructed at `runtime.rs:1488` and
  the replay cache at `:1311`, so handing it the real resolver requires
  reordering the assembly. That reordering is the increment.
- **D-2 · The directory, which would deny every request in an attached tenant.**
  `provision_space_on` writes an attached tenant's founding `members` row into
  the **target**, per the `membership-home` ruling. The door's engine is built
  by `deployment_engine_pooled`, which reads `members` from the control pool
  only. So the moment routing is turned on, an attached tenant has a member row
  the door cannot see and every request in that tenant is denied by a door
  working exactly as written. `routed_engine` exists and its only caller is a
  test. **This is the first thing §10 wires**, and it is why turning routing on
  is not a one-line change.
- **D-3 · The security drain.** `start_routed` exists; the binary calls `start`.
  Until it is wired, an attached tenant's space-scope security events land in
  the control database, the one place §3 says they must not.
- **D-4 · `$jobs` and `$schedules` contradict §3's own correction.** §3 ruled
  `$jobs` is owed by every target, because a write enqueues its invalidation
  inside the write's transaction. Two things in the tree disagree. Neither table
  has a **`$space` column** (`crates/ordo-exec/src/jobs.rs:44-55`), so a
  target's rows cannot say which tenant they are for. And every consumer reads
  the control pool only: `claim_schedule` and `claim_due` from
  `supervisor.rs:1019`, the leadership lock, and the `LISTEN` bus dialled from
  `cfg.pg`. `claim_jobs` and `finish_job` have **no callers at all**. So an
  attached tenant's enqueued work would land in its own database and be claimed
  by nobody. The requirement: the enqueue-side split stands, `$space` is added
  to `$jobs` so a claimed row knows its tenant, and claiming becomes a walk of
  `targets()` beside the retention walk that already exists. Until that lands,
  routing and background work are mutually exclusive, which is a sentence an
  operator should read rather than a behaviour they should discover.

**R-H2 · Two homes §3 never named, both correct, both worth writing down.**
**Custody is deployment-wide.** `PgKeyStore` dials from `cfg.pg` and has no
routed variant, deliberately: keys are per space, the store holding their
wrapped forms is not, and the master key that wraps them is the deployment's.
That is consistent with the 2026-09-12 ruling (one tenant, one key, encryption
at the database) and is what makes platform-custody crypto-erasure possible.
**The witness is control-only**, by its module doc, and §9.7 C-M5 says why it
must stay there.

### 9.4 Failure modes: unreachable, slow, drifted

**17aa built the shape and this section adds no second mechanism.**
`Spaces::continuity_of` is the one place a verdict is reached and the one place
the quarantine register is written (`crates/ordo-host/src/spaces.rs:421-442`).
`Reconciled` is `Served | Quarantined | Unreadable`, `SpaceRefusal` is
`NoCustody | Unroutable | Rewound`, the register is a
`Mutex<BTreeMap<String, String>>` documented as report-only and *"never a
decision"*, and `/health/ready` prints it as `quarantined: [{space, reason}]`
(`runtime.rs:528`, `:598`). A tenant is quarantined on a failing verdict and
lifted on the next passing one, with no restart, because `resolve` re-asks for
any space not marked continuous.

**R-F1 · An unreachable target quarantines its tenant, through the register
that already exists.** Today `SpaceRefusal::Unroutable` is returned and the
register is **not** written, because `continuity_of` is its only writer and it
answers the rewind question alone. So a tenant whose cluster is down fails
every request while `/health/ready` reports nothing at all. The requirement is
one more writer, not a second register: a failed resolve notes the tenant with
its reason, and a later successful resolve lifts it. The refusal itself is
already correct and must stay: **an unreachable target is never substituted by
the default**, which would serve the tenant from the wrong database.

**R-F2 · An unreachable cluster answers 503, not 500.** `SpaceRefusal` maps to
`ExecError::internal` and `ErrorCode::Internal`
(`crates/ordo-host/src/lib.rs:686-691`), which is right for `Rewound` (the
deployment disagreeing with itself, an operator repair) and wrong for
`Unroutable` (a dependency that is down and may come back). `PoolError::Saturated`
and `StatementTimeout` already answer 503, so this is the code the deployment
already uses for the same class of thing. The body carries an opaque reason per
§9.1 R-C4.4, never the dial error's text.

**R-F3 · A slow target is bounded already, and a repeatedly failing one is
not.** Two bounds travel to a target pool today and both should be pinned by a
test so they keep travelling: the **dial** is bounded by the checkout timeout
(S3, which removed an unbounded TCP wait), and the **statement** is bounded
because `AttachedPools` dials through `Pool::connect` (`route.rs:322`), which
reads `PoolLimits::from_env()` and pins `-c statement_timeout` into the session
options at dial (`lib.rs:666`, `:521-533`, `PoolConfig::pg_bounded`). That
composition is fortunate rather than designed, since 20a landed a week after
S3; §9.5 R-P2 makes it deliberate. What is missing is a bound on **retries**:
S3 deliberately does not cache a failed dial, which is right for recovery and
means a down target costs a fresh dial on every request for every tenant on it.
The requirement is a bounded backoff per target, with the quarantine register
as the place its state is read, so a down cluster costs a dial per backoff
interval rather than a dial per request.

**R-F4 · A drifted target is the same register and the same answer**, per §9.2
R-S4 and R-S6. No new mechanism, no new surface, and the reason string says
"schema behind" so an operator can tell it from "unreachable" without reading
a log.

**R-F5 · The control database going away is not a tenant quarantine.** It is
the deployment being down, and quarantining the control plane would be
quarantining the thing that reports quarantines. Stated so nobody builds it.
`/health/ready`'s continuity check already answers only for the control plane's
own space, which is the same instinct.

**R-F6 · One list, not five.** A target's trouble is currently reported in at
least four places that do not know about each other: `Spaces::quarantined`,
`Directory::unread` (a target whose members could not be read),
`PgInvalidationBus::unheard` (a database no listener reached, whose failure
mode is silence), and the unreachable entries `PoolResolver::targets()` returns
rather than omitting. An operator reading one of them can be told everything is
fine while another holds the answer. The requirement is that `/health/ready`
assembles one per-tenant list from all of them, each entry naming the tenant id,
its human name attribute (§9.1 R-C2, because an opaque id is unreadable), the
reason, and since when. The four sources stay as they are; what is added is the
one place they are read.

### 9.5 The pool, and what 20a's caps mean across clusters

**What 20a built** (`crates/ordo-exec/src/lib.rs`). `PoolLimits { space_reserve,
statement_timeout }`, defaulting to no explicit reserve and 30 seconds, from
`ORDO_PG_SPACE_RESERVE` and `ORDO_PG_STATEMENT_TIMEOUT_MS`. `space_max(size)` is
`size - reserve` with the reserve defaulting to `(size/4).max(1)`, floored at
`size.min(2)`. A checkout takes the **space's share first and the pool's global
permit second**, both inside one checkout budget, and the doc says the order is
the whole mechanism: a checkout holding a connection it cannot use while a space
that has one waits behind it is the deadlock the order removes. `Pool::for_space`
charges a handle to a space, and `Spaces::pool_of` (`spaces.rs:309-327`) is the
one seam every request-path checkout passes through: resolve the target, then
charge to the space.

**R-P1 · The cap applies per pool, and a pool is per target, so the reserve's
meaning is unchanged and its value stops fitting.** The reserve holds
connections out of any single space's reach so one space cannot starve another
**on the same pool**. On the default target, which carries tens of thousands of
tenants, that is exactly right. On a target with one tenant on it, `space_max`
holds back a quarter of the pool to protect nobody, and that tenant's own
cluster refuses its own work at three quarters of capacity. The requirement is
that the reserve is resolved **per pool** rather than read once from the node's
environment: `ORDO_PG_TARGET_POOL_SIZE` and `ORDO_PG_TARGET_SPACE_RESERVE` set
the defaults for dialled targets, distinct from the deployment's own pool.
Deriving the reserve from how many spaces currently resolve to that pool is
tempting and is **not** recommended for v1, because the count changes as tenants
attach and the semaphore would have to be resized underneath live checkouts.
A per-attachment size belongs in `$spacedb` only when an operator asks for it,
and that is a column added later, not now.

**R-P2 · The statement bound travels, and a test says so.** It travels today
because `AttachedPools` calls `Pool::connect`, which reads `PoolLimits::from_env`.
That is a property nobody chose and nobody would notice losing, so the
requirement is a named test that attaches a target and shows a statement
exceeding the bound is refused there with `PoolError::StatementTimeout`, the
same as on the default.

**R-P3 · The connection count an operator must plan for is `size x (K+1)`**,
where K is the number of distinct targets, plus one `LISTEN` session per target
(S7) and one per target for any leadership lock. An operator sizing
`max_connections` on a tenant's own cluster needs that number, and §9.6 requires
it to be visible rather than computed from a doc.

**R-P4 · The listener sessions carry no statement bound, and now there are K+1
of them.** `PoolConfig::connect_config` (the `LISTEN` and leadership-lock dialer)
stays bare by design, which was one unbounded session per deployment and becomes
one per target. Stated rather than changed: a `LISTEN` session runs no
statements, so the bound would protect nothing, and the requirement is only that
an operator's connection budget counts them.

**R-P5 · A sweep must not be charged to nobody.** Every target-walking loop
(security retention, the usage rollup, blob collection when it gets a runner)
uses an **uncharged** handle, bounded by the pool's size and by no space cap at
all. On the default target that is a background pass among thousands of tenants
and it is fine. On a one-tenant target it is a second consumer of a small pool
with no bound, competing with that tenant's own requests. The requirement is
that sweeps charge to a reserved pseudo-space so the per-space cap applies to
them, which reuses `Pool::for_space` and adds no mechanism.

### 9.6 Operations: backup, restore, verification, rollup, what an operator sees

**R-O1 · A restore is named by tenant id, not by two hostnames.** `ordo restore
--record` dials the space's database from `--pg-*` flags and the witness from
`--control-db`, trusting the operator to point each at the right place
(`crates/ordo-cli/src/operator.rs:742-863`, `:349-359`). For one database that
is fine. With N targets it is two independent chances to name the wrong host,
and the failure is a chain compared against the wrong witness. The requirement
is that the command takes the control connection and a **tenant id**, resolves
the tenant's database through `$spacedb`, prints which database it resolved, and
refuses rather than guessing when the tenant is unattached and `--pg-host` was
passed as well. The manual form stays as break-glass.

**R-O2 · A tenant's backup has two halves and the procedure must say so.** The
tenant's cluster holds its chain, sealed payloads, blob references, entity rows
and its own security scope. The control database holds that tenant's `$spacedb`
and `$spacebucket` rows, its grants and delegations, its `$quotas` and `$usage`
rows, its attestation sinks, and the **witness** heads a restore of its cluster
cannot roll back. Restoring only the tenant's cluster gives a tenant with no
route to itself; restoring only the control database gives a route to nothing.
The order is the tenant's database first, then the control rows for that tenant,
and the witness is **not restored**, because the untouched witness is what makes
the restore checkable: boot reconciliation compares against it, quarantines a
chain that moved backwards (§9.4), and `ordo restore --record` writes the marker
that lifts the quarantine. That flow is 17j and exists; what routing adds is
R-O1's resolution.

**R-O3 · Under `OwnPerimeter` the platform does not take the backup.** The
platform cannot reach into a cluster the tenant runs, so backup is the tenant's
act and the platform's contribution is the witness and the record. That is the
same asymmetry §2's disclosure draws for crypto-erasure and chain verification,
and the operator surface must state a tenant's custody before a restore is
planned rather than during one.

**R-O4 · Verification says which custody it ran under.** The chain proof is
routed already (S1). Under `PlatformManaged` an operator can verify a tenant's
chain end to end; under `OwnPerimeter` verification needs the tenant's
cooperation, so a proof run against a cluster the tenant controls is a weaker
statement than the same proof against one the platform controls. The surface
prints the distinction rather than leaving two different facts looking
identical.

**R-O5 · The usage rollup already walks `targets()`, and a target it cannot
reach must leave a gap rather than a zero.** `UsageRollup::run` resolves
`spaces.pools().targets()`, reads counts per target through the routed pool and
writes summary rows to the control database (`workers.rs:570-683`). That split
is right: `$usage` is a control-plane ledger. What is undefined is an
unreachable target during a rollup period. A row of zeros is a silent discount
on a billing ledger and is indistinguishable from a genuinely idle tenant. The
requirement is an explicit gap for that period, carried into the ledger and
visible in `ordo usage`, so the number is known to be missing rather than known
to be zero.

**R-O6 · What an operator must see, in one place.** Six facts, each carrying the
tenant's **name attribute** beside the id, because under the 2026-09-21 ruling
the id is what everything keys on and the name is the only part that reads.

1. Every quarantined tenant, its reason and since when, assembled from the four
   sources of §9.4 R-F6.
2. Each target's schema version against the activated artifact (§9.2 R-S2's
   fleet view), and which tenants are behind.
3. The connection budget per target, `size x (K+1)` plus the listener sessions
   (§9.5 R-P3), rather than one deployment total.
4. Which tenants are on which target, with occupancy, because §9.8 R-X1 makes
   occupancy part of what a tenant was told.
5. Each node's installed credential **labels**, names only (§9.7 C-M3), so an
   incomplete rollout is visible before an attach fails.
6. Each target's custody, so R-O3's question is answered before a restore is
   planned.

### 9.7 Moving a tenant, as a constraint on this design

**The move is 18e and stays out of scope** (§2, Q-2, the owner's
`routing-migration` ruling). What follows are constraints this build must keep
so that the move remains buildable, each of which costs nothing now and is
expensive to retrofit.

**C-M1 · One cache of the attachment, and nothing else remembers it.** The
answer lives in `AttachedPools` alone, cached for the process's life, and that
is sound only because `AttachRefusal::WouldMove` refuses a change. §9.2's fleet
view and §9.6's operator surface must read through the resolver or be marked
plainly as stale reports, because a move that has to reach five caches is a
different and worse build than one that has to reach one. No new code path may
read `$spacedb` directly and keep the answer.

**C-M2 · `WouldMove` stays the one place, and nothing else writes `$spacedb`.**
It is named in 18e as the one thing a move has to change, so a second writer is
a second thing to change and a second race to argue about.

**C-M3 · The fleet's credential labels must be checkable without a dial.** A
move points a tenant at a cluster; if no node holds a credential label for it,
the move fails after the data has been copied. `DbCredentials::password` already
returns `Err` for an unheld label, which answers for one node. The constraint on
this build is that each node reports the **labels it holds** (names only, never
values) on its health surface, so 18e can check the fleet before it copies
anything. That is one field, now, rather than a fleet-wide probe invented later.

**C-M4 · A move's destination must be at the activated schema version, and
R-S1's `$migrations` row in every database is what makes that checkable.** A
tenant moved onto a cluster one version behind is R-S4's quarantine, discovered
after the cutover rather than before it.

**C-M5 · The witness stays in the control database.** `Witness`
(`ordo_exec::witness`, held on `Runtime::witness` over the control pool) is the
deployment's record of the heads it signed, and a restore of a tenant's own
database cannot roll it back. That property is the whole reason boot
reconciliation can catch a rewind, and it is also what lets a move verify that
the copy arrived intact. Moving the witness into the tenant's database as an
optimisation would remove both. Stated here because it is the kind of change
that looks like tidying.

### 9.8 Security: credential reach, what the door refuses, what the audit records

**R-X1 · Credentials do not cross targets, and inside a target they separate
nothing.** A pool is keyed by `DbAttachment::identity` and carries that
attachment's credential, so one tenant's credential never dials another
tenant's cluster. But a pool is per **target**, not per tenant, and S3 built
that deliberately: a hundred tenants attached to one database share one pool and
one credential. Inside that database the only separation is the `$space` column
and the four software defences of §6, which is the shared tier's isolation and
not the dedicated tier's.

**This makes the current disclosure incomplete.** `DbCustody` is
`PlatformManaged | OwnPerimeter` and says who runs the cluster; it says nothing
about whether the tenant is alone on it. A consent page that prints
"your data is in a database your organisation runs" for a tenant sharing that
database with three others is printing a true sentence that reads as a false
one. The requirement is that the disclosure states occupancy as well as
custody, computed from `$spacedb` at the moment consent is taken, and that
attaching a second tenant to a target already attached under `OwnPerimeter` is
**refused**: that perimeter belongs to the first tenant and the platform is not
the party who may put someone else inside it.

**R-X2 · The door refuses five attachments.** Three are built and two are new.

*Built:* a different target for an attached tenant (`AttachRefusal::WouldMove`);
a tenant that already holds rows on the default database (`erase::rows_held`,
which would make the attach a live move under another name); and a dial for a
credential label this node does not hold, refused before any dial so a target
that would accept an unauthenticated connection is never reached by accident.

*New:* **an attachment naming the control database's own host, port and
database**, which is a tenant pointed at the control plane and gives the
tenant's rows a home among identity, grants and every other tenant's
attachment; and **an attachment naming a target already held under
`OwnPerimeter` by another tenant** (R-X1).

**R-X3 · The attachment's host is an address the deployment will dial, and it
is chosen by data.** That is the same class of thing `ordo-egress` exists for:
the egress kernel denies private ranges and the pod CIDR because an outbound
destination chosen by data is an SSRF, and it folds every reason into one
`egress_refused` because *"a distinguishable reason is an oracle"*
(`crates/ordo-egress/src/refusal.rs`). A Postgres dial is not HTTP and does not
pass through that kernel, so the requirement is that an attach applies an
equivalent allowlist at **attach time**, before the row is written: the
operator states which hosts or CIDRs may be attached, and the refusal is one
reason, not several. An attach is an operator act rather than a tenant act
(Q-3), which makes this a guardrail rather than a boundary, and it is still the
difference between a typo and a connection to the pod's own metadata service.

**R-X4 · What the audit records when an attachment changes.** The row carries
`attached_at` and `attached_by`, and S5 writes one custody event on the
**deployment-scope** security stream, only when that call attached. The
requirement adds four things to that event and one place it also has to go.

The event must name: the tenant's **minted id** (never its name attribute,
which is not unique and is not identity); the target's identity as host, port
and database, and the **credential label**, never the credential's value; the
custody choice and the occupancy R-X1 requires; and whether the call created
the attachment or completed an interrupted provision, because those are
different facts and a retry must not read as a second attach.

**And the tenant's own stream must carry it too.** The deployment stream lives
in the control database and the tenant's stream lives in the tenant's, so an
attachment recorded only on the deployment stream is invisible in the tenant's
own export: the tenant cannot show, from their own records, where their data
went. At attach time the tenant has no chain to join, which is why S5 put the
event on the deployment stream, and `provision_space_on` writes the founding
member into the target immediately afterwards. So the requirement is that the
attachment fact is recorded as part of that founding group, which is the first
entry in the tenant's chain and the earliest point one can exist.

## 10 · Increments, smallest first

Each one is behaviour-identical with `ORDO_PG_ROUTING` unset until T4, which is
the point of the ordering: the wiring lands and is proved not to change
anything before the switch that changes something exists.

**T0 · Pin the two properties that travel by accident.** Two tests and no
production change. The statement bound reaches a target pool because
`AttachedPools` dials through `Pool::connect`, which reads `PoolLimits::from_env`
(§9.5 R-P2), and `PoolConfig`'s password stays redacted from `Debug` (§9.1
R-C4.1). Neither was designed to compose and neither would be noticed breaking.
*Files:* `crates/ordo-exec/tests/route.rs`. *Tests:*
`a_statement_on_an_attached_target_is_bounded_too`,
`a_pool_config_never_prints_its_password`. *Done when* both pass and nothing
else changed. *Tier: Sonnet.*

**T1 · The door reads every tenant's members from its own database.** Closes
D-2, which is the one drift that would deny every request in an attached tenant.
`routed_engine` exists and its only caller is a test; the binary calls
`deployment_engine_pooled`. *Files:* `crates/ordo-host/src/runtime.rs` (the door
build at `:893`), `crates/ordo-host/src/directory.rs`. *Tests:*
`an_attached_tenants_founding_member_is_seen_by_the_door`,
`a_target_the_door_cannot_read_is_reported_rather_than_fatal`. *Done when* both
pass and the full suite is unchanged under `SinglePool`. *Tier: Sonnet.*

**T2 · One resolver, four consumers.** Today `runtime.rs` builds four
independent `SinglePool`s: for `Spaces` (`:1488`), for the exit and attestation
ops (`:956`), inside `PgReplayCache::new` (`:1311`) and inside
`SecurityDrain::start` (`:1407`). Each has a routed variant nobody calls
(`PgReplayCache::routed`, `SecurityDrain::start_routed`) and `exit_ops.rs`'s own
doc says it should be handed `Spaces::pools()`. This increment builds `Spaces`
first and hands `spaces.pools()` to all four, which closes D-1 and D-3 and is
the reordering the replay cache's position makes necessary. *Files:*
`runtime.rs`, `replay.rs`, `secdrain.rs`, `exit_ops.rs`. *Tests:*
`one_resolver_serves_the_door_the_cache_the_drain_and_the_exit_ops`, plus the
full suite. *Done when* no `SinglePool` is constructed anywhere in `runtime.rs`
except the one `Spaces` holds. *Tier: Opus*, because it moves boot ordering and
every refusal that hangs off it.

**T3 · The credential provider.** §9.1 R-C3: `ORDO_DB_CREDENTIALS` names the
provider, `dir` reads a directory where the label is the file name, a
group- or world-readable file refuses at boot, an empty file is passwordless,
and the node reports the labels it holds and never their values (§9.7 C-M3).
*Files:* `crates/ordo-exec/src/route.rs` (a `Files` implementation of
`DbCredentials` beside `Installed`), `crates/ordo-host/src/runtime.rs`. *Tests:*
`a_label_with_no_file_refuses_before_any_dial`,
`a_world_readable_credential_file_refuses_at_boot`,
`an_empty_credential_file_is_passwordless`,
`the_health_surface_names_labels_and_never_values`. *Done when* an operator can
install a credential without editing the binary. *Tier: Sonnet.*

**T4 · The switch, and the first tenant served from its own cluster.**
`ORDO_PG_ROUTING` with `single` and `attached`, refusing an unknown value the way
`ORDO_KEY_PROVIDER` does and refusing `attached` with no credential provider
(§9.1 R-C1). `attached` constructs `AttachedPools`, and the bus connects through
`connect_routed` rather than `connect`. *Files:* `runtime.rs`,
`crates/ordo-exec/src/route.rs`. *Tests:*
`an_unknown_routing_value_refuses_at_boot`,
`attached_routing_without_a_credential_provider_refuses`,
`a_deployment_that_attaches_nothing_behaves_identically`, and the end-to-end
`an_attached_tenant_is_served_from_its_own_cluster`, which provisions one tenant
onto a second real database and shows its writes, reads, security events,
idempotency records and `?live` notifications all land there while a tenant on
the default is untouched. *Done when* that end-to-end test passes. *Tier: Opus.*

**T5 · Quarantine covers unreachable and drifted, and there is one list.**
§9.4. A failed resolve writes 17aa's register with its reason and a later
success lifts it, which keeps the register report-only and leaves the decision
where it already is, in the resolve. `Unroutable` answers 503 rather than 500,
carrying an opaque reason, and `/health/ready` assembles one per-tenant list from
`Spaces::quarantined`, `Directory::unread`, `PgInvalidationBus::unheard` and the
unreachable entries `targets()` returns. A failing target backs off rather than
being re-dialled per request. *Files:* `spaces.rs`, `runtime.rs`,
`crates/ordo-host/src/lib.rs:686`, `route.rs`. *Tests:*
`an_unreachable_target_quarantines_its_tenant_and_answers_503`,
`a_recovering_target_lifts_its_own_quarantine`,
`a_dial_error_is_never_returned_to_the_caller`,
`health_ready_names_every_troubled_tenant_from_one_list`,
`a_down_target_is_dialled_once_per_backoff_not_once_per_request`. *Done when*
all five pass. *Tier: Opus.*

**T6 · `$jobs` learns its tenant, and claiming walks targets.** Closes D-4, and
until it lands routing and background work are mutually exclusive. `$space` joins
`$jobs` so a claimed row knows whose it is, and `claim_due`, `claim_schedule` and
the dead `claim_jobs` walk `targets()` beside the retention walk that already
does. *Files:* `crates/ordo-exec/src/jobs.rs` (DDL and enqueue),
`crates/ordo-host/src/supervisor.rs`, `crates/ordo-host/src/workers.rs`. *Tests:*
`a_job_enqueued_in_an_attached_tenant_is_claimed`,
`a_claimed_job_names_the_tenant_it_came_from`,
`claiming_visits_every_target_once`. *Done when* a write in an attached tenant
produces an invalidation that a node actually delivers. *Tier: Opus.* Note that
the `$jobs` DDL change runs through `ensure_platform_tables`, so it reaches every
target that T7 reaches and must be ordered with it.

**T7 · The schema reaches every target.** §9.2, and it is the largest. A fresh
target takes the DDL from `Deployment::current()` and records its `$migrations`
row in the same all-or-nothing apply; `apply_everywhere` walks `targets()` with
the control database last and the activation after every target succeeded; the
load gate runs per target and reports; attach and apply share one advisory lock.
*Files:* `crates/ordo-host/src/schema_apply.rs`, `crates/ordo-host/src/provision.rs`,
`crates/ordo-host/src/deploy.rs`, `crates/ordo-exec/src/migrate.rs`. *Tests:*
`a_fresh_target_records_the_version_it_received`,
`an_apply_reaches_every_target_before_the_activation`,
`a_target_that_fails_holds_back_the_activation`,
`a_target_behind_the_artifact_quarantines_its_tenants`,
`an_attach_during_an_apply_waits_and_receives_the_new_version`. *Done when* a
second schema version is deployed to a deployment with an attached tenant and
both databases hold it. *Tier: Opus.*

**T8 · The attach op, its refusals and its record.** §9.8, and it also gives
`prepare_target` and `provision_space_on` their first production caller, which
they do not have today. Occupancy joins the custody disclosure; a second tenant
on an `OwnPerimeter` target is refused; the control database's own host, port and
database is refused as a target; an attach-time host allowlist refuses with one
reason, on the argument `ordo-egress` already makes; the custody event carries the
minted id, the target's identity, the label, the custody, the occupancy and
whether this call attached or completed an interrupted provision; and the
attachment fact joins the tenant's founding group so the tenant's own export can
show where its data went. *Files:* `provision.rs`, `spacedb.rs`,
`crates/ordo-host/src/platform_ops.rs`. *Tests:*
`attaching_to_the_control_database_is_refused`,
`a_second_tenant_on_an_own_perimeter_target_is_refused`,
`a_host_off_the_allowlist_is_refused_with_one_reason`,
`the_disclosure_states_occupancy_as_well_as_custody`,
`the_attachment_fact_is_in_the_tenants_founding_group`. *Done when* attaching a
tenant is something an operator does through the door. *Tier: Opus.*

**Built 2026-09-24, with three departures from the paragraph above.**

*The op is `std.space.create`, not a new one.* This plan implies
`std.space.attachDatabase`, symmetric with `std.space.attachBucket`. The
symmetry is false: a bucket attaches to a space that already has an owner to
consent, and a database cannot, because `prepare_target` refuses any space
holding rows (`erase::rows_held`) — an owner-consented database attach would
refuse every space ever written to. The `db-attach-gate` ruling (2026-09-13)
says it from the other end: *"during provisioning the system decides... you
cannot have an agreement for a space that does not exist and you cannot have
a space without a place for storage."* So routing is six optional parameters
on the create op, `$db` decides, and a later change is 18e.

*The founding-group half is not built.* D-F, ruled 2026-09-21: *"theres no
provability unless its byo db... I think this belongs to the app not ordo."*
So `the_attachment_fact_is_in_the_tenants_founding_group` does not exist and
should not be written. The deployment-stream record carries the fact.

*Occupancy is exposed, not narrated.* `spacedb::occupants` returns the spaces
in a target; `DbCustody::disclosure` is unchanged. That is D-S8's ruling for
the storage tier applied here: the platform records and exposes the fact, the
developer writes the sentence.

**R-X1's occupancy question was ruled wider than this section frames it.**
D-D, 2026-09-25: *"1 tenant 1 database. same for buckets. database/schema not
cluster."* So the shared tier this section describes — *"a hundred tenants
attached to one database share one pool and one credential"* — is not
something the attach path may produce. One refusal, `DatabaseOccupied`, and
custody is not consulted: the two refusals built for the narrower reading
(one per direction, by which side declared `OwnPerimeter`) collapsed into it.

The **cluster** is still shareable, which is the half that keeps this
affordable: many tenant databases on one Postgres server is the ordinary
case, and that is why the allowlist is a list of hosts. **The default path
still shares the control database** among tenants created without `$db`;
whether that must end is `db-default-tier` on the board and is not assumed
here.

**R-X2's residual, stated rather than left implicit.** The control database
is refused by identity, and `same_database` folds the loopback spellings so
`localhost`, `127.0.0.1` and `::1` are one server. A **DNS name resolving to
that address still passes**, because it is not equal as a string. The
complete check is to dial and compare Postgres's `system_identifier`, which
no spelling disguises. It is not built: this check runs before anything is
written, and catching the three common spellings there beats catching every
spelling after a database has been created.

**Two defects the tests found and the code did not.** The door-level loopback
test is **vacuous on a developer machine** — the conformance cluster is on a
Unix socket (`ORDO_PG_HOST=/tmp/ordo-pg`), so it only ever runs on CI; found
by breaking the normalisation on purpose and watching nothing fail, and now
covered by a unit test that runs everywhere. And a socket directory is a
**path**: folding its case, as a hostname requires, would match `/var/run/PG`
to `/var/run/pg` on a case-sensitive filesystem.

**The custody record had no facts in it.** It said
`space_attached_database` and stopped. It now carries the target's identity,
the credential label, the custody, the occupancy at the moment of attach, and
whether the call attached fresh or completed a provision interrupted after
the database was made — all inside the action string, because
`EventKind::CustodyAction` is hash-chained under a tag and widening the
variant would change the encoding of every event already written.

**`ORDO_PG_ATTACH_HOSTS` denies by default** and is documented in
`deploying.md` §2c. Per-tenant databases therefore do nothing until an
operator names the hosts, which is deliberate: an allowlist defaulting to
open protects only deployments whose operator already knew to set it.

**T9 · The operator surfaces. Built 2026-09-24, with deviations below.** §9.6.
`ordo restore --record --tenant` resolves the tenant's database from its id
(`--space`'s own value — see below, not a second flag) through `$spacedb`; an
unreachable target leaves a gap rather than a zero in the usage ledger, in a new
`$usage_gaps` table, and no longer stalls the rest of the fleet's metering while
it is down; a new `ordo fleet` command prints three of R-O6's six facts — schema
version, occupancy and custody per target — each tenant with its name attribute
beside its id. *Files:* `crates/ordo-cli/src/operator.rs`,
`crates/ordo-host/src/workers.rs`, plus `crates/ordo-exec/src/usage.rs` (the gap
table — not in the original file list) and `crates/ordo-host/src/serve.rs` (kept
in sync with the CLI's own doc claim that the two `usage` surfaces render
byte-identical output). *Not* `runtime.rs`: nothing needed changing there: the
credential provider T3 built (`runtime::credentials_from_env`) was already
`pub fn` and the restore path calls it directly. *Tests:*
`restore_resolves_the_tenants_database_from_its_id`,
`restore_refuses_a_tenant_id_and_a_host_together`,
`an_unreachable_target_leaves_a_gap_not_a_zero`,
`the_operator_surface_prints_the_name_beside_the_id`. *Done when* an operator can
answer "where is this tenant and is it healthy" from one command. *Tier: Sonnet.*

**Two deviations, both argued in `docs/roadmap-checklist.md`'s T9 entry.**
**(1)** `--tenant` is a bare switch, not a second id-bearing flag: the tenant id
this requirement needs is `--space`'s own value (`ordo_host::tenants` mints a
space's id, it does not mint a second identifier for the same space), so a
separate flag would be two spellings of the one thing R-O1 exists to stop having
two of. **(2)** R-O6's six facts are answered in part, not in full: facts 1, 3
and 5 (the quarantine register, the connection budget, installed credential
labels) are the state of a *running replica*, and `ordo fleet` opens a database
connection rather than a socket into one — printing durable rows as if they
answered for live process state would read as complete and would not be. Those
three stay on `/health/ready` (T5 built the quarantine half already) rather than
being approximated here.

**T10 · The record.** This plan's answers into the checklist, `corpus-rulings.md`
and the decisions board. *Tier: Sonnet.*

## 11 · Owner decisions that still stand

### What §8's questions became, and what the code has since answered

All four of §8's questions are **ruled**, and in three cases the code has since
gone further than the ruling.

- **Q-1, the default target.** Ruled 2026-09-13: it stays the control database.
  The code now agrees structurally and not only by policy: the witness, the key
  store, identity, `$quotas` and `$usage` all dial `cfg.pg` unconditionally, and
  `SinglePool::targets()` labels the control pool `"default"`. Closed.
- **Q-2, moving a live space.** Ruled out of scope and filed as 18e.
  `AttachRefusal::WouldMove` is built and is the single place a move has to
  change, so the question is closed and §9.7 records it as five constraints on
  this build rather than re-opening it.
- **Q-3, who may attach.** Ruled 2026-09-13 (`db-attach-gate`): at provisioning
  the system decides, and a later change needs both parties.
  `prepare_target` writes `attached_by = system:provisioning` and one custody
  event, so the ruling is implemented. What the code has **not** answered is that
  nothing calls it: there is no production path by which any tenant is attached
  to anything. T8 is that path.
- **Q-4, where members live.** Ruled 2026-09-14 (`membership-home`): the space's
  own database, and `provision_space_on` writes them there. The code has since
  revealed the other half, which the ruling did not reach: the door still reads
  members from the control pool, so the ruling is written and not wired (D-2).
  T1 wires it. No new decision is needed.

### The decisions that still stand

**D-A · Is per-tenant resolution switched on for this release, or after it?**
*Recommendation: land T0 to T5 now and hold T6 to T8.* T0 to T5 make routing
switchable and prove it identical when off, which is worth having in the release
whether or not anyone attaches a tenant. T6 and T7 are real builds and D-4 means
that until T6 lands, a deployment with routing on has an attached tenant whose
background work is claimed by nobody. Shipping a switch that quietly breaks jobs
is worse than shipping no switch, so if only part of this lands, the switch is
the part that waits.

**D-B · A tenant whose database is behind the activated schema: quarantined, or
served the version its database has? — settled 2026-09-21.** *"It should be
configurable per tenant. This would be a deployment or possible maintenance
window contract."* Per-tenant, with quarantine as the default for a tenant with
no contract and the older artifact served to a tenant whose window has not
opened. This moves off the recommendation, which was unconditional quarantine,
and accepts the multi-artifact cost that recommendation existed to avoid. R-S4
carries the ruling. **D-B2 replaces it as the open question.**

**D-B2 · How far back may a contracted tenant lag, and what ends the
contract? — answered in part 2026-09-21, one clause still open.** The owner:
*"I think one version is fine, I'd rather it be flexible, and contract I mean
app-dev to customer. But yeah one version makes sense ideally but there could be
broken situations where you may want to revert down the chain."*

Two things this settles. **The contract is app-developer to customer**, not
platform to tenant, so Ordo supplies the knob and the enforcement and never the
policy, and the principal who sets it is the deployment owner, which is who the
app developer already is here. And **reverting is not lagging**: a deployment
rolling back down the chain is `registry::activate` naming an older version,
`$artifacts` is never deleted from and the `$activations` foreign key makes
deleting an ever-active artifact impossible, so revert depth is unbounded today
and stays unbounded. Lag is two versions serving at the same instant, which is
the one that needs a second `Loaded` and a per-tenant resolver, and the one the
bound is about. A deep revert moves every tenant together and costs the bound
nothing.

**Settled 2026-09-21:** *"One version back + ability for snapshots (for
requesting customers on the app dev side) and ensure step by step downgrading in
schema."* So the lag bound is **one version**, a deployment holds two artifacts
at most, and two behind quarantines regardless. The safety net for going further
back is a **snapshot**, available on request through the app developer rather
than generated for every tenant, and restoring one across versions walks the
chain **one step at a time** rather than diffing the ends. That last clause is
11-10b's shape and it obliges something not built: the diff engine plans
forward, so each step has to state its own down path or state that it has none.
What limits a deep revert of the *deployment* is unchanged either way: the load
gate refuses a program whose shape no longer matches a migrated database.

**D-C · Does the per-space reserve apply on a target holding one tenant?**
*Recommendation: keep the cap unconditional and configure per-target defaults*
(`ORDO_PG_TARGET_POOL_SIZE`, `ORDO_PG_TARGET_SPACE_RESERVE`), accepting that a
single-tenant target holds back a reserve that protects nobody. Deriving it from
current occupancy means resizing a semaphore underneath live checkouts, which is
a real hazard bought for a configuration default.

**D-D · Must the custody disclosure state occupancy, and is a second tenant on an
`OwnPerimeter` target refused?** *Recommendation: yes to both* (R-X1). This is the
one place these requirements touch something already ruled, the 2026-09-12
disclosure, so it needs the owner rather than an implementer. A pool is per
target and not per tenant, so two tenants on one database have the shared tier's
isolation while being told their data is in a database their organisation runs.
The sentence is true and reads as something stronger.

**D-E · Is an attach-time host allowlist required for v1?** *Recommendation: yes*
(R-X3). An attachment's host is an address the deployment dials, chosen by data,
which is the thing `ordo-egress` exists to refuse for HTTP. An attach is an
operator act rather than a tenant act, so this is a guardrail and not a boundary,
and it is still the difference between a typo and a connection to the pod's own
metadata service. One refusal reason, never several.

**D-F · Does the tenant's own chain carry the attachment fact?**
*Recommendation: yes, in the founding group* (R-X4). It changes what S5 built,
which is why it is a decision. Without it a tenant cannot show from their own
records where their data was put, because the only record is on the deployment
stream in a database they do not hold.

**D-G · Does an unreachable target leave a gap in the usage ledger?**
*Recommendation: yes, an explicit gap* (R-O5). It is billing-visible, which is
why it is the owner's. A row of zeros is a silent discount and is
indistinguishable from a genuinely idle tenant, so the choice is between a
number that is wrong and a number that is known to be missing.

**D-H · Credential provider: a mounted directory only for v1, with no KMS?**
*Recommendation: yes* (R-C3), on the argument `MasterKeySource` already makes for
the master key: the file ships first, *"it is the same file a KMS source later
opens. Moving to a KMS is then a provider swap plus a re-wrap, never a
migration."* A directory reduces every secret manager to a mount, and adding a
second provider later touches neither `$spacedb`, the resolver nor the audit
trail.
