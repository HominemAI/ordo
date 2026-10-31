# `ordo dev` — the sub-second hot loop: design & build plan

Companion to `adoption-dx-plan.md` §3.1 (`[settled as bar]`),
`implementation.md` ("Dev mode", "Hot deployment", "Runtime shape
verification"), `api-engine-design.md` §6 and §11 (live transport, hot
swap), and `plan-schema-evolution.md` (whose increment 1 landed and whose
increment 2 this plan depends on).

Markers: **[settled]** restated from a settled document · **[proposed]**
adopted here, pending owner review · **[open]** named and not answered.

Status: build plan. **Nothing is built.** Every claim about the current
code was checked against the code and carries its file and line.

---

## 0 · The problem, stated precisely

### 0.1 The bar is settled and nothing measures it

`adoption-dx-plan.md` §3.1 pins it:

> The bar: **save-to-serving under one second** for a domain of realistic
> size, including additive migration apply. … Anything over a second on
> save is a bug, not a tuning opportunity.

`roadmap-checklist.md` item 16e states the position honestly: *"`ordo dev`
does not exist … There is no `dev` and no watch loop anywhere in the tree,
so the bar measures nothing."* Grep confirms it. `ordo`'s subcommands are a
hand-rolled `match` on `argv[0]` in `crates/ordo-cli/src/main.rs:82` and
`:118` — `check`, `plan`, `plan --against`, `interface`, `apply`,
`rollback`, `deploys`, `verify`, `checkpoint`, `prune`, `revert`. No `dev`.
There is **no file watcher anywhere**: no `notify` crate, no `inotify`, no
`fsevent` in any manifest. There is **no benchmark harness**: no `benches/`,
no `criterion`, no `[[bench]]`.

A settled bar that nothing measures is a claim, not a property. This plan's
first increment is the measurement, and the rest of the plan spends the
budget the measurement discloses.

### 0.2 There is also no server

`crates/ordo-host` declares no `[[bin]]` and has no `src/main.rs`; the only
binaries in the workspace are `ordo-cli` and `ordo-playground`.
`ordo_host::serve::platform_router` (`crates/ordo-host/src/serve.rs:147`)
hands back an axum `Router` and nothing outside tests binds a socket to it.
`docker/Dockerfile.host` (retired at console step 10 for the `host` target
of `docker/Dockerfile`) said so in its own first line (*"PROPOSED — not
wired, AND not yet buildable"*). And
`deploy::listen_for_deploys` (`crates/ordo-host/src/deploy.rs:573`) — the
LISTEN half of the hot-swap loop — **has no caller anywhere in the tree.**

So `ordo dev` is not a wrapper around a running server. It is the first
thing in the repository that runs one, and that is a fact about scope
rather than a complaint: the parts it composes are all built and tested,
and none of them has ever been composed.

### 0.3 What this document is not

It is not `plan-schema-evolution.md`. That document owns the diff, the
probes, the classification and the apply; D2 below *consumes* increment 1's
classifier and *waits on* increment 2's apply, and says what it does in the
meantime. It is not `plan-editor-tooling.md` (the LSP is §3.2 of the
adoption plan, a different loop). It is not the `@ordo/cdk` generator,
which is in flight elsewhere; D4 builds the seam it plugs into and
deliberately does not build a TypeScript emitter in Rust.

---

## 1 · The starting state, verified

**The compile is fast, and it is one function.**
`Fixture::from_source(src, seed)`
(`crates/ordo-conformance/src/lib.rs:293`) delegates to `compile_source`
(`:297`), which is the single canonical sequence: `parse_program` →
`KeyTable::from_env` → `build_with_keys` → `ddl_for` → `Registry::from_program`
→ `acl_admin_ops` → `std_ops` → `cost::check_registration`. **It spawns
nothing** — no `psql`, no pool, no process, no I/O beyond reading env vars.
It is pure CPU. Eighteen call sites; **none of them is timed.**

**The source-reading layer already defines the watch set.**
`Sources::read(&Path)` (`crates/ordo-cli/src/sources.rs:39`) handles a
single file or a recursive directory, filters on `extension == "ordo"`
(`:145`), and sorts by **relative** path bytes (`:51`) so the concatenated
buffer — and therefore spans, op order and artifact bytes — is
machine-independent. `Sources::at(span)` (`:102`) maps a buffer position
back to `file:line:col`. That is exactly a diagnostic an editor can jump
to, already built, unused by anything that watches.

**The playground already compiles on request against a seeded database.**
`ordo_playground::server::forward` (`crates/ordo-playground/src/server.rs:156`)
binds, classifies cost, plans, and — when asked — executes.
`App::load()` (`:60`) *"compile[s] the fixtures; touches no database"*, and
`App::seed()` (`:83`) rebuilds three databases at boot via `Fixture::reset`.
**There is no timing instrumentation anywhere in the crate**, and execution
is a blocking shell-out to `psql`.

**The database seam splits in two, and only one half is usable here.**
`Pg` (`crates/ordo-conformance/src/lib.rs:81`) forks a `psql` process for
**every** `exec` and every `query` (`:136`). That is the harness's seam and
it is fine for tests. The real driver is `ordo_exec::Pool`
(`crates/ordo-exec/src/lib.rs:312`) over `tokio-postgres`, `NoTls`, config
from `ORDO_PG_*` (`:75`). **`ordo dev` must use `Pool`.** A loop that shells
out per statement cannot have a millisecond budget.

**The throwaway cluster pattern exists as a shell script.**
`scripts/conformance-pg.sh` (40 lines): `initdb -D $PGDATA -U ordo
--auth=trust` **only if `$PGDATA` does not exist**, `pg_ctl start -o "-p
$PORT -k $SOCKET -h ''"` (**unix socket only, no TCP**), busy-wait on
`pg_isready`, `createdb || true`; `stop` leaves the data dir; `destroy`
removes it. Port is **fixed at 55432**, not allocated. There is **no cleanup
trap**. Tests do not invoke it — they connect and, on failure, print *"The
execution tier needs a Postgres cluster. Start one with
`scripts/conformance-pg.sh start` and re-run"*
(`crates/ordo-conformance/src/lib.rs:155`). CI does not use it either; the
workflow runs a `postgres:17` service container.

**Hot swap is built, gated, and never driven.**
`Deployment::swap(&self, next: Verified) -> Swapped`
(`crates/ordo-host/src/deploy.rs:541`) is one write lock and one pointer
move. `Verified` (`:106`) is a newtype with a private field, and its only
constructors are `Verified::from_gate` (`:136`) — which takes a
`ShapeReport` **by value**, so possession of a report is proof the gate ran
— and `verify_candidate` (`:317`), which loads from the artifact registry.
`from_gate`'s own doc (`:129`) names this plan's caller:

> This exists for the embedder whose program did not come out of the
> artifact registry — **`ordo dev`**, a single-program self-host, the
> deployment tests — so that "I ran the gate" and "my source came from
> Postgres" stay separate claims.

The seam this plan needs was designed for it and has been waiting.

**The gate is the expensive part.** `shape::verify_registry`
(`crates/ordo-host/src/shape.rs:222`) opens **its own pool**, runs
`DROP SCHEMA IF EXISTS ordo_shadow CASCADE`, calls `build_shadow(ddl)` to
recreate the whole schema from scratch, reads `live_drift`, and then walks
`build_specs(f)` **serially**, and for every read op re-parses a probe call,
re-binds, re-plans, and awaits one `observed_shape(sql)` round trip
(`shape.rs:266`–`:358`). Concretely, for the market fixture — 378 lines, the
"realistic size" §3.1 asks about — that is **18 entities' worth of DDL plus
76 serial describe round trips** (the interface golden counts 165 ops: 76
read, 64 write, 25 delete).

**The invalidation bus is built end to end.**
`InvalidationBus` (`crates/ordo-api/src/spec.rs:550`), `MemoryBus`
(`crates/ordo-api/src/bus.rs:15`), `PgInvalidationBus::connect`
(`crates/ordo-host/src/invalidation.rs:203`) over
`INVALIDATE_CHANNEL = "ordo.invalidate"` (`crates/ordo-exec/src/jobs.rs:230`),
and SSE at `crates/ordo-api/src/router.rs:910` — subscribing *before* the
initial read, `invalidate` frames carrying `{}` and never data, `KeepAlive`
every 15s. `DEPLOY_CHANNEL = "ordo.deploy"` is separate
(`crates/ordo-exec/src/registry.rs:88`) precisely because *"an invalidation
says your cached rows may be stale, and a deploy says your whole registry
is."*

**The descriptor already has an incrementality primitive.**
`interface_document` (`crates/ordo-plan/src/interface.rs:125`) sets
`value["digest"] = digest(&value)` (`:158`), where `digest` (`:166`) is
sha256 over the document minus the digest key. `interface_json`
(`crates/ordo-cli/src/lib.rs:358`) re-runs the whole pipeline every call —
there is no cache and no incremental path, and at ~30 ms of pure CPU there
does not need to be one.

**Schema evolution's classifier is built; its apply is not.**
`ordo_schema::diff::{Change, Class, diff}`
(`crates/ordo-schema/src/diff.rs:199`, `:165`, `:636`) is a closed 27-variant
enum whose `class()` (`:370`) is derived from `code()`'s first byte.
`ordo_exec::migrate`'s probes read and never write
(`crates/ordo-exec/src/migrate.rs:5`). `migrate::plan_against_database`
(`crates/ordo-cli/src/migrate.rs:100`) is the in-process seam. The verdict
text (`:546`) is explicit: *"Apply is not built yet — increment 2 executes
class A."*

### 1.1 Three facts found while reading that change the plan

**(a) The budget's owner is probably the shape gate, not DDL apply.**
`adoption-dx-plan.md` §3.1 reasons that *"the budget is dominated by DDL
apply and swap"*. On the code as it stands, a save that changes **nothing
about the schema** still pays a full `DROP SCHEMA … CASCADE`, a full
`build_shadow`, and 76 serial describe round trips, because `swap` takes a
`Verified` and `Verified` requires a `ShapeReport`. Whether that fits in a
second on a warm local cluster is an empirical question nobody has asked.
This is why D1 is measurement and D3 is the optimisation — and why the plan
does not state a budget number before measuring one.

§3.1's own remedy is the right one and this plan adopts it: *"dev mode keeps
a warm shadow schema."* But the remedy has to be aimed by data, because
"warm shadow" removes the DDL rebuild and leaves the 76 round trips.

**(b) `ordo-cli` cannot host a server as configured.** Its `tokio` is
`features = ["rt"]` — single-threaded, one runtime built per command
(`crates/ordo-cli/src/migrate.rs:70`). It depends on no `axum` and no
`ordo-host`. `ordo dev` needs `rt-multi-thread`, `time`, `sync`, a watcher
crate, and `ordo-host`. That is a real structural change to the crate's
character — from compiler front end plus operator scripts to a process that
serves — and D3 should make it deliberately rather than by accretion.

**(c) The one wall-clock budget test in the tree is the right precedent,
and it argues its own generosity.** `crates/ordo-host/tests/secdrain.rs:661`
asserts `start.elapsed() / 100_000 < Duration::from_micros(50)` with:

> Deliberately generous: this runs on shared CI hardware in a debug build.
> It is here to catch an order-of-magnitude regression — a round trip or an
> O(n) scan sneaking onto the request path — not to police nanoseconds.

Every budget in D1 copies that comment and that spirit.

---

## 2 · The loop, and where each phase already lives

| # | Phase | Exists? | Where |
|---|---|---|---|
| 1 | fs event → the changed file | **no** | new; watch set = `Sources::collect`'s walk (`sources.rs:133`) |
| 2 | re-read sources | yes | `Sources::read` (`sources.rs:39`) |
| 3 | parse → bind → plan → register | yes | `Fixture::from_source` (`ordo-conformance/src/lib.rs:293`) |
| 4 | diagnostics to terminal | half | `Sources::at(span)` (`:102`) gives `file:line:col`; nothing renders a spanned frame |
| 5 | schema delta | yes (classify) | `live_shape` + `diff` (`ordo-schema/src/diff.rs:636`) |
| 6 | apply the delta | **no** | schema-evolution increment 2 (class A); §4 gives the interim |
| 7 | shape gate | yes | `shape::verify_registry` (`ordo-host/src/shape.rs:222`) |
| 8 | swap | yes | `Verified::from_gate` → `Deployment::swap` (`deploy.rs:136`, `:541`) |
| 9 | tell connected clients | yes | `pg_notify` → `PgInvalidationBus` → SSE (`invalidation.rs:203`, `router.rs:910`) |
| 10 | regenerate the descriptor | yes | `interface_json` (`ordo-cli/src/lib.rs:358`), digest-gated |

Ten phases, one new (1), one blocked on another plan (6), one half-built
(4). Everything else is composition — which is why the risk in this work is
not "can it be built" but "does the composition fit in the budget", and why
the measurement comes first.

---

## 3 · The measurement design

### 3.1 What "save-to-serving" means, precisely

**[proposed]** The clock starts at the write to the `.ordo` file and stops
when **a request that could only succeed against the new program succeeds**.

Not "compile finished", not "swap returned". Those are the compiler's claims
about itself, and a loop that reported them would pass its own test while a
developer's browser still showed the old answer. The end of the interval has
to be observed from outside the deployment.

Operationally, the test appends a new op to the source, writes the file, and
polls the door through `platform_router` with
`tower::ServiceExt::oneshot` — a dependency `ordo-host` already carries as a
dev-dependency — until the call returns `200` instead of `unknown_op`. T1 is
the first success.

Two properties fall out of that definition and both matter. It measures the
whole pipeline including the parts nobody thinks about (the fs event's own
latency, the terminal write). And it cannot be satisfied by a shortcut that
serves before the gate, because a program that has not swapped does not
answer.

### 3.2 The journal: the loop prints exactly what the test asserts

**[proposed]** One struct, `DevTiming`, one producer, two consumers. Every
save prints:

```
saved  domain/market.ordo                                        ✓ 412ms
       watch 3 · read 2 · compile 31 · delta 8 (0 changes)
       gate 297 (76 reads) · swap 0 · notify 1 · descriptor 12
```

and the test asserts on the same struct. The rules:

- **The phase numbers must sum to the total**, asserted, so the journal
  cannot lie by omission. Unaccounted milliseconds are the thing a budget
  test exists to find.
- **Every number carries its denominator.** `gate 297 (76 reads)` rather
  than `gate 297`, because a budget without its denominator is a number that
  gets bumped. When someone adds forty std ops the count moves in the
  journal and a reviewer sees why the time moved.
- The journal prints on **every** save, pass or fail. A loop that only
  reports when it is slow trains people to ignore it.

### 3.3 The test

`crates/ordo-host/tests/dev_loop.rs`, `#[tokio::test(flavor =
"multi_thread")]`, its own database via `Pg::named("ordo_dev_loop")` — the
per-suite isolation convention the sixteen existing host suites use.

The representative save is the **market fixture** (378 lines, 18 entities,
165 ops, 76 reads) with one op appended: §3.1's "domain of realistic size",
already in the tree, already the golden's subject.

**Two constants, and only one of them moves.**

```rust
/// The product bar. `adoption-dx-plan.md` §3.1, [settled as bar].
/// Moving this needs an owner ruling, not a commit.
const BAR: Duration = Duration::from_millis(1000);

/// What the loop actually costs today. Ratchet: this may only ever
/// DECREASE. A change that raises it is a regression and reviews like
/// moving a golden.
const CEILING: Duration = /* set by D1's first measurement */;
```

The test asserts `total < CEILING` and separately **prints** whether `BAR`
was met. That is the honest arrangement while the two differ: the loop is
policed against what it costs, the product is measured against what it
promised, and the gap is visible in CI output on every run instead of living
in a document. When `CEILING <= BAR` the second assertion becomes a third
constant nobody has to think about again.

Each budget carries `secdrain.rs:657`'s comment verbatim in spirit: this is
here to catch an order-of-magnitude regression on shared CI in a debug
build, not to police milliseconds.

**Per-phase ceilings, not just a total.** A total-only budget lets one phase
absorb another's improvement and hides the regression. Each phase in §2 gets
its own ceiling under the same ratchet rule.

### 3.4 Why a budget test and not a benchmark

There is no `criterion` in the tree and this plan does not add one. A
benchmark answers "how fast is this function" and needs statistical
machinery, a stable machine and a separate CI job. A budget test answers
"did the loop get an order of magnitude worse", runs in the suite everyone
already runs (`cargo test --workspace -- --nocapture`, which CI already
does), and fails the build. The bar in §3.1 is a product promise, and a
product promise wants a test.

---

## 4 · The database story in dev

### 4.1 The cluster

**[proposed]** `ordo dev` manages its own Postgres, reusing
`conformance-pg.sh`'s pattern — `initdb --auth=trust`, unix socket, `pg_ctl`
— moved into Rust so it is one process's responsibility, with three
corrections the script's own shape invites:

- **Allocate the port**, do not fix it. 55432 is hardcoded
  (`conformance-pg.sh:13`), so two projects open at once collide, and a
  developer's dev loop is exactly the case where two are open at once.
- **Own the lifecycle.** The script has no cleanup trap and `stop` leaves
  `$PGDATA`. `ordo dev` stops its cluster on exit and keeps the data
  directory under the project (so a restart is warm), with `ordo dev
  --fresh` to destroy it.
- **Data directory per project**, not per machine.

`ORDO_PG_*` still override everything, so pointing `ordo dev` at an existing
cluster stays one env var — the escape hatch a self-hoster and CI both need.

### 4.2 The delta on save

Read `live_shape` (`crates/ordo-exec/src/migrate.rs:55`), `diff` against the
declared schema (`ordo-schema/src/diff.rs:636`), classify with `Change::class()`
(`:370`). Three outcomes:

- **No changes** — the common case, and it must cost a query and nothing
  else. Skip apply entirely.
- **Class A only** — apply through schema-evolution increment 2's
  `ordo migrate --apply`, the same code path production runs, with no
  relaxation whatsoever.
- **Anything else** — §4.3.

Probes (`migrate::duplicates`, `nulls`, `out_of_enum`, `dangling_refs`, …)
run **only** for classes B and C, never on the no-change path. They are
round trips, and the no-change path is the one that has to be fast.

### 4.3 The honest shortcut, and where the line is

`implementation.md`'s settled dev-mode relaxations are *"auto-confirm
destructive migrations, skip version-history bookkeeping."* **This plan
narrows the first one, and says so explicitly rather than quietly
implementing something else.**

**Kept: skip version-history bookkeeping.** A save every few seconds writing
an `$artifacts` row and an `$activations` row would fill the registry with
noise and make `ordo deploys` useless. Dev builds a `Verified` directly
through `Verified::from_gate` — the constructor whose doc names `ordo dev`
as its intended caller (`deploy.rs:129`) — and never touches the registry.
No property is weakened: the gate still runs, the swap still takes a
`Verified`, and *"I ran the gate"* and *"my source came from Postgres"* stay
the separate claims that doc says they are.

**Narrowed: "auto-confirm destructive migrations" becomes "offer a reset".**
Auto-confirming a destructive migration means running `ALTER TABLE … DROP
COLUMN` without asking — which requires a class-D apply path
(schema-evolution increment 5) that does not exist, will not exist for
months, and would then exist *primarily to serve dev*. Code on the
destructive path that only dev exercises is code reviewed with dev's eyes
and run against production's data the first time someone sets a flag wrong.

The shortcut that costs nothing is the one already in the tree:
`Fixture::reset` (`crates/ordo-conformance/src/lib.rs:408`) —
`DROP SCHEMA public CASCADE; CREATE SCHEMA public;`, re-run the DDL, re-seed.
It is instant, it is always correct, it is already tested, and it is honest
about what it does: **your dev data is seed data, and it is regenerated.**

So the line, stated as a rule:

> **`ordo dev` never runs a migration production would not run.** Class A
> goes through the production apply path unchanged. Anything else is not
> migrated at all — the loop offers a reset, names the changes that forced
> it, and re-seeds.

This is a *stronger* statement than §3.1's, and the strengthening is the
point: it means the destructive apply path has exactly one caller, an
operator who confirmed, forever.

A consequence to state rather than discover: this makes seed data a
first-class part of a project. `Fixture` carries a `seed: &'static str` with
`@SPACE@` and `@BIDX(entity,field,value)@` placeholders expanded by
`rendered_seed` (`ordo-conformance/src/lib.rs:443`). `ordo dev` needs the
same thing from a file beside the sources — `seed.sql` by convention — and
without one, a reset leaves an empty database and the loop says so.

### 4.4 Before increment 2 lands

Schema-evolution increment 2 is not built. Until it is, D2 ships with **only
the reset path**: any schema delta at all, including a purely additive one,
offers a reset. That is a worse loop and it is not a blocker, because the
common save — editing an op body, adding a query — changes no schema at all
and takes the no-change path. D2's done-when has two rungs for exactly this
reason, and the second one is satisfied by someone else's increment.

**Landed 2026-09-20 (13b).** The second rung is in: `dev_db::classify`
returns `Delta::Apply` when `ordo_host::schema_apply::triage` refuses
nothing, and the loop calls `schema_apply::execute`, the same function
`ordo migrate --apply` calls. The rows survive. Everything the triage
refuses is a reset, and so is an empty database, because the reset is the
path that seeds. What §4.3 kept (no registry, no `$migrations` row) is
still kept.

---

## 5 · Ruled out, with reasons

**HMR of op logic beyond swapping the deployment.** `Deployment::swap` takes
a `Verified` and `Verified`'s field is private with two constructors, both
of which run or consume the gate (`deploy.rs:106`, `:136`, `:317`). A
finer-grained "just replace this one op" path would need a third way to
reach a `Loaded`, and the absence of a third way *is* the property. The swap
is already one write lock and one pointer move — there is nothing to make
faster, only a gate to make cheaper, which is D3's actual job.

**Watching anything that is not a `.ordo` file.** `Sources::collect`
(`sources.rs:133`) already defines the set: files with extension `ordo`
under the root. The watcher registers exactly that set. No `node_modules`,
no `Cargo.toml`, no `.env`, no generated output — a loop that rebuilds
because a dependency's lockfile changed is a loop nobody trusts.

**A bundler, and any JavaScript build step.** `ordo dev`'s job ends at "the
descriptor on disk changed". What an app's toolchain does next is the app's
toolchain's business, and Vite/esbuild/Next already watch files better than
this would.

**A TypeScript emitter inside `ordo-cli`.** There is none today — grep for
`gen_ts` / `typescript` across `crates/` returns zero — and the `@ordo/cdk`
decision already ruled generator-first, one generator, two outputs.
Putting a TS emitter in the Rust workspace would make the workspace own a
TypeScript formatter's output and would be the second generator the ruling
declined. D4 builds the seam instead.

**A dev-only executor, an in-memory Postgres, or a stubbed shape gate.**
The bar is about the real loop. A loop that is fast because it skipped the
gate measures nothing, and §3.1's own sentence — *"through the real
compiler"* — settles it.

**HTTPS, a dev proxy, multi-project workspaces, and remote dev.** Each is a
product; none is the loop.

---

## 6 · Build order

### D1 — watch → compile → report, measured

The loop with nothing in the middle: watch the `.ordo` set, re-read, compile
through `Fixture::from_source`, print spanned diagnostics. **No database, no
serve, no swap.** This is `ordo check` on a timer, and it exists to make the
measurement real before anything expensive is added to it.

- The watcher (`notify`, added to `crates/ordo-cli/Cargo.toml`, which has no
  shared workspace dependency table to extend), debounced, over
  `Sources::collect`'s walk.
- Spanned diagnostics: `Sources::at(span)` (`sources.rs:102`) already
  resolves `file:line:col`; D1 adds the frame — source line, caret span,
  and the `rule:` / `fix:` shape `adoption-dx-plan.md` §3.3 commits to.
- `DevTiming`, the journal of §3.2, and the budget test of §3.3 with its
  first `CEILING`.

*Done when:* editing one op in the market fixture prints a spanned,
rule-and-fix diagnostic in a terminal and a passing save prints its phase
journal — and `crates/ordo-host/tests/dev_loop.rs` asserts a named per-phase
and total ceiling that the journal's own numbers sum to.

### D2 — the local database and the schema delta

`ordo dev` owns a cluster (§4.1) and classifies every save's delta (§4.2),
applying class A through the production path and offering a reset for
anything else (§4.3), with `seed.sql` as the convention.

*Done when:* `ordo dev` on a clean checkout brings up its own Postgres on an
allocated port, applies the schema, seeds it, and tears it down on exit —
and a save that adds an optional field is applied while the loop keeps
serving, while a save that drops one refuses to migrate, names the change by
its `Change::code()`, and offers the reset. **Second rung, satisfied by
schema-evolution increment 2:** the additive half runs `ordo migrate
--apply`'s code path rather than a reset.

### D3 — serve, gate, swap, reload

The composition nobody has performed: bind a socket to
`ordo_api::router(...).merge(platform_router(...))`, run the gate, build a
`Verified` through `from_gate`, `swap`, and let the existing bus tell
connected clients.

- The **warm shadow** §3.1 asks for: `verify_registry` currently drops and
  rebuilds `ordo_shadow` on every call (`shape.rs:239`, `:245`). Dev builds
  it once and applies each save's class-A delta to the shadow and the live
  schema together, so a no-schema-change save pays no DDL at all.
- Whatever the D1 journal says the remaining gate cost is, aimed at it. Two
  named candidates, both safe, in this order: **run the per-op describes
  concurrently** over a larger pool — they are read-only `DESCRIBE`s against
  a schema nothing else touches, and the loop at `shape.rs:266` is serial
  only because nothing needed it not to be; and **skip the describes whose
  plan bytes did not change**, keyed on `sha256(plan.sql)` against the
  shadow's own DDL hash, since an op whose SQL is byte-identical against an
  identical schema has an identical observed shape.
- Wire `listen_for_deploys` (`deploy.rs:573`) — its first caller — so a
  second process's `ordo apply` reaches this one, which is the property the
  channel split (`registry.rs:85`) exists to provide.

*Done when:* saving a file that adds an op makes that op callable through
the door without a restart, an SSE subscriber on an affected read receives
an `invalidate` frame, and the total from file-write to first successful
call to the new op is inside `CEILING` — with the gate's share of it named
in the journal.

**Explicitly not relaxed here:** the swap still takes a `Verified` and the
gate still runs on every save. D3 makes the gate cheap; it never makes it
optional.

### D4 — the descriptor on save, and the generator seam

- `ordo dev --out <dir>` writes `interface.json` on every save — the
  machinery `crates/ordo-cli/src/main.rs:142` already has.
- **Digest-gated**, which is the whole of "incremental": `interface_document`
  already computes `digest` (`interface.rs:166`) over the document minus the
  digest key, so the loop writes the file only when the digest moves. No
  dependency graph, no cache invalidation, no second source of truth — a
  content hash the emitter already produces. Compile is ~30 ms of pure CPU;
  nothing else in the loop needs to be incremental and pretending otherwise
  would add machinery to save nothing.
- **The seam, not the generator.** `ordo dev --gen <command>` runs a
  configured command after a digest change, with the descriptor's path in
  the environment. `@ordo/cdk` plugs in the day it exists; `ordo-cli` never
  learns TypeScript. If the command fails, the loop prints its output and
  keeps serving — a broken generator must not take the backend down.

*Done when:* a save that changes an op's signature rewrites
`interface.json` and runs the configured generator exactly once; a save that
changes only an op's body leaves the file untouched and does not run it; and
a generator that exits non-zero prints its output without stopping the loop.

### Deferred, deliberately

- **A real `ordo serve`.** D3 binds a socket for dev. A production server
  binary is `plan-ci-images.md`'s blocker and its own piece of work; D3
  should be written so `ordo serve` is the same composition minus the
  watcher, and should say so in code rather than duplicate it.
- **LSP integration.** `adoption-dx-plan.md` §3.2 and
  `plan-editor-tooling.md`. The compile-and-diagnose half D1 builds is the
  same computation, and the two should share a diagnostic type before the
  LSP grows its own.
- **Multi-space dev.** The host serves one hardcoded space
  (`ordo_host::SPACE`), and no space directory exists.

---

## 7 · The open question this plan does not close

**[open] Does the shape gate fit in the budget, and if not, what gives?**

D1 answers the first half empirically and the plan is deliberately written
so that the answer can be "no". If a warm shadow plus concurrent describes
plus plan-hash skipping still does not fit a second on a realistic domain,
there are exactly three moves and two of them are acceptable:

1. **Verify a subset in dev and the whole registry on a cadence** — every
   save gates the ops whose plans changed, and a background pass gates the
   rest. Weaker, and honestly weaker: the property becomes "every op is
   gated within N seconds of a save" rather than "before it serves". It
   would need a name, a disclosure, and a rule about what happens to a
   request that arrives for an ungated op.
2. **Raise the bar.** `adoption-dx-plan.md` §3.1 is `[settled as bar]` by
   the owner, and a measured impossibility is a legitimate reason to revisit
   a settled number — but it is the owner's to revisit, and the measurement
   is what earns the conversation.
3. **Serve ungated in dev.** Rejected outright. It deletes the property
   `Verified` exists to hold, and it deletes it in the environment where
   developers form their expectations about what the platform guarantees.

This is the question to settle **after D1 and before D3 optimises**, because
D3's two optimisations are worth building either way and option 1's design
is not.
