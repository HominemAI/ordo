# Ordo runtime — the server binary, key custody & the worker supervisor: design & build plan

The three things that stood between "the platform works" and "the platform
runs". Each was a *mechanism the tree already had, with nobody to call it*,
and the gap column is the state each track was written against:

| Track | The gap it was written against | State |
|---|---|---|
| **R1** — the `ordo-host` binary | `ordo-host` is a library; nothing in the workspace binds a socket to `platform_router` | **built.** `crates/ordo-host/src/main.rs` is `from_env` → `assemble` → `serve` over `runtime.rs`; `tests/runtime.rs` spawns the real process |
| **R2** — a production `KeyProvider` | the only provider a deployment can select is `TestProvider`, whose master key is a constant in this repository — and a program loaded from the registry seals through it *whatever provider the operator supplied* | **built.** R2a, R2b, R2c and the ceremony; `crates/ordo-exec/src/keystore.rs` is the `PgKeyStore` |
| **R3** — the worker supervisor | every loop *step* is built and tested; no long-running task calls one | **built and wired.** `crates/ordo-host/src/supervisor.rs` (the trait, the loop, quarantine, leadership) and `src/workers.rs` (fourteen workers), started from `runtime::serve` before the door binds |
| **R4** — CI wiring | four workflow files exist; three are `workflow_dispatch`-only, and the host image job probes for a `main.rs` that does not exist and skips with a notice | **built.** The host image job boots the binary against the service Postgres |

Numbering is `R1–R4` for `roadmap-checklist.md` §17. **This document adds
nothing to the checklist itself.**

Companion to `plan-ci-images.md` (whose increment 0a sketched R1's
constraints — this supersedes and expands it, §1.0), `plan-production-ops.md`
(whose K1 and M1 explicitly ride "the supervisor `plan-runtime.md` R3
defines" — §3.1 is that contract), `plan-dev-loop.md` D3 (the same assembly
minus the watcher, §1.7), `docs/howto-custody.md` and `deploying.md` §4.

Markers: **[settled]** restated from a settled document · **[proposed]**
adopted here, pending owner review · **[open]** named and not answered ·
**[pinned]** a ruling recorded in `corpus-rulings.md`.

Status: **R1–R4 have landed.** This started life as a build plan whose
status line said "nothing in it is wired", and that line survived the build
by eight days. Sections 0 through 5 are now read as the **design and the
record**: §0's findings are what was true before the build, and each
section's "done when" names the test that holds it. Where the code and a
section disagree, the code wins and the section is corrected; §3.4's worker
table, §3.5's leadership API and §7's findings list were all corrected that
way on 2026-09-26. Every claim about current code carries its file and line.

---

## 0 · The starting state, verified

Every increment below is a delta on these nine facts. They were read, not
assumed, and four of them changed the plan.

### 0.1 There is no server process anywhere in the tree

`crates/ordo-host/Cargo.toml` has no `[[bin]]` and there is no
`crates/ordo-host/src/main.rs`. The only two `main.rs` files under
`crates/` are `ordo-cli/src/main.rs` and `ordo-playground/src/main.rs`, and
the only `TcpListener::bind` in the tree is the playground's.
`node.router()` in `crates/ordo-host/tests/deployment.rs:180` is a test
helper driven through `tower::ServiceExt::oneshot` — in-process, no socket.

Two consequences the manifest makes concrete, and both are R1 work rather
than boilerplate:

- **`ordo-host` does not depend on a runtime that can serve.** Its tokio
  features are `["macros", "rt", "sync", "time"]` — no `net`, no
  `rt-multi-thread`, no `signal`. `axum::serve` needs `net`; a supervisor
  needs `rt-multi-thread`; graceful shutdown needs `signal`. The
  dev-dependency block already adds `rt-multi-thread` for the tests, which
  is exactly the tell that the serving half was never built.
- **There is no argument parser in the workspace.** `ordo-cli` hand-rolls
  its flags (`crates/ordo-cli/src/operator.rs:220-226`). R1 adds no
  dependency: the binary is configured entirely by environment (§1.1),
  which is also what an image and an orchestrator want.

### 0.2 The load path a binary needs already exists, and it is `load_active`

This is the best news in the document, and it means R1 is assembly rather
than invention. `deploy::load_active(pool, cfg, keys, space)`
(`crates/ordo-host/src/deploy.rs:357`) reads the active artifact out of the
registry, compiles it, registers its specs, runs the shape gate, and returns
a `Verified` — the type `PlatformState::new` and `Deployment::boot` require
(`serve.rs:116`, `deploy.rs:412`). `verify_candidate`'s doc
(`deploy.rs:300-316`) states the four-step order and why each step is where
it is.

So the binary does **not** need a fixture file, a `--program` flag, or a
compile step of its own. It needs a pool, a `KeyProvider`, a space, and the
database the operator already ran `ordo apply` against. The gate-before-
surface property `plan-ci-images.md` §1.1 asks R1 to preserve is preserved
*by taking this path at all*: there is no way to reach `PlatformState` from
`load_active` except through a passed gate.

`Deployment::pinned` (`deploy.rs:464`) is the escape hatch that must not
appear in the binary. It carries no report, and `health_ready` already
refuses to call such a deployment ready (`serve.rs:194-197`) — so a binary
that used it would boot, serve, and never become ready, which is the
confusing failure rather than the loud one.

### 0.3 A loaded program seals under a constant test key, whatever provider it was given

**This is the finding that reorders the plan, and R1 cannot ship without
R2's first half because of it.**

`verify_candidate` takes `keys: Arc<dyn KeyProvider>` and threads it into
the `ordo_exec::Executor` it builds (`deploy.rs:346-352`). That covers the
**read** path: `Executor::read_page_as` resolves blind indexes and opens
sealed payloads through `self.keys` (`crates/ordo-exec/src/lib.rs:640`,
`:645`).

The **write** path does not go through `Executor::keys`. It goes through
the fixture:

```rust
// crates/ordo-host/src/lib.rs:563-572 — FixtureExecutor::execute, write arm
let summary = live.exec.write_leased(live.fixture.clone(), …)
```

and `write_leased` hands that fixture to the Stage-5 protocol, whose
`impl` block is `impl Fixture` (`crates/ordo-conformance/src/write.rs:333`).
Every seal, every blind-index digest and every sealed-handle mint in it is
`self.keys` — that is, `Fixture::keys` (`write.rs:1228`, `:1251`, `:1255`,
`:1264`, `:2600`, `:2828`, `:2832`, `:3099`, `:3116`, `:3120`, `:3134`).

And `Fixture::compile_source` — the constructor `verify_candidate` reaches
through `Fixture::from_source` — hardcodes it:

```rust
// crates/ordo-conformance/src/lib.rs:318
keys: std::sync::Arc::new(ordo_crypto::TestProvider::with_space(FIXTURE_SPACE)),
```

So a binary that boots correctly through `load_active`, holding a real
`KeyProvider`, **reads** through the operator's custody and **writes**
sealed values under `TestProvider`'s constant — `b"ordo-test-smk-00"`,
published in this repository at `custody.rs:737`. Worse than a refusal to
boot: the deployment works, the round trip succeeds, and the ciphertext is
openable by anyone who has cloned the repo.

The same constructor pins `space: fixture_space()` — `"conformance"`
(`lib.rs:319`, `:248`) — which is why `ordo_host::SPACE` is the string
`"conformance"` (`crates/ordo-host/src/lib.rs:84`) and why the executor
asserts the door's space equals it (`lib.rs:518-522`).

The fix is small and it is R2's, not a drive-by: custody and tenancy are
**inputs to loading a program**, not properties of the compiler's output.
Recorded as a ruling — §5, and `corpus-rulings.md`.

The test harness has been quietly compensating for this the whole time:
`metered_fixture` (`tests/deployment.rs:190-197`) assigns `f.keys` *after*
construction, and its comment says why in as many words — *"the executor
reads through `f.keys`, and the Stage-5 write protocol seals through the
very same handle."* The seam is already known to be the fixture field; what
is missing is anything on the production path that sets it.

### 0.4 The pool cannot authenticate to Postgres

`PoolConfig::pg()` (`crates/ordo-exec/src/lib.rs:91-105`) sets host, port,
user, dbname and session options. There is **no password field**, no
`.password(…)`, and no TLS — `dial` connects with `NoTls` (`:269`).
`PoolConfig::from_env` (`:76-89`) reads four variables and none of them is a
credential.

`tokio-postgres` does not fill the gap from the environment: there is no
`env::var` anywhere in `tokio-postgres-0.7.18/src/config.rs`. The live
`ci.yml` sets `PGPASSWORD: ordo` (`.github/workflows/ci.yml:37`) and the
proposed host smoke passes `-e PGPASSWORD=ordo`
(`.github/workflows/images.yml:288`) — that variable reaches
`ordo-conformance`'s `psql` shell-out and **not** the pool. Locally it never
showed, because `scripts/conformance-pg.sh` starts a cluster on a Unix
socket with `trust`.

So: any Postgres that asks for a password is a Postgres this deployment
cannot dial. `ORDO_PG_PASSWORD` is R1 work (§1.1), and TLS is named and
deferred (§4).

### 0.5 The door and the access engine shell out to `psql`

`MemberAccess::check` (`crates/ordo-host/src/lib.rs:365-392`) answers the
door's access question by calling `Fixture::run_case_params(&self.pg, …)` —
`ordo_conformance::Pg`, which executes by running the `psql` **binary**
(`crates/ordo-conformance/src/lib.rs:19`). `deployment_engine`
(`lib.rs:1269`) builds the whole `AccessEngine` from `pg.query` over
`members`, the same way.

Three things follow, and they decide R1's shape:

1. A deployable that used `MemberAccess` would need `psql` on `PATH`,
   which contradicts `plan-ci-images.md` §3.1's distroless base and puts an
   apt feed into the image that serves customer data.
2. It is a **blocking** process spawn inside an async request path, once
   per request. A pooled `SELECT 1` costs microseconds; a `psql` fork costs
   milliseconds and holds a runtime worker thread.
3. It is also the conformance token form — `member:<id>` — which
   `ordo-host`'s own module doc already calls out as honestly absent:
   *"tokens are the conformance `member:<id>` form rather than a verified
   credential"* (`lib.rs:22-24`).

`IdentityAccess` (`lib.rs:931`) is the real door: signed tokens, one `Deny`
for every reason, the clock as an input, denials into the security-event
recorder and the metrics registry. It holds an `Arc<AccessEngine>` and does
**no** I/O per request. R1 mounts that one. The engine it holds still has to
be *built* from somewhere, and today the only builder reads through `psql`
— so R1 adds a pooled builder beside `deployment_engine` rather than
promoting the fixture path (§1.3).

### 0.6 Five secrets, and two of them have no loader at all

| Secret | What it protects | Loader today |
|---|---|---|
| deployment master key | wraps every space's SMK, hence every sealed value and every blind index | `LocalKeyProvider::from_env` → `ORDO_MASTER_KEY`, refused when unset (`custody.rs:519-529`) |
| command-log checkpoint key | signs checkpoints and attestations; the published half is how a third party checks history | `Ed25519Signer::from_env` → `ORDO_CHECKPOINT_KEY` + `_KEY_ID`, refused when unset (`crates/ordo-commandlog/src/sign.rs:113-122`) |
| security-stream checkpoint key | signs the security-event stream's own checkpoints — **deliberately not `ORDO_CHECKPOINT_KEY`** | `security_signer_from_env` → `ORDO_SECURITY_CHECKPOINT_KEY` + `_KEY_ID`, refused when unset (`crates/ordo-secevent/src/store.rs:121-138`) |
| **token key** | mints and verifies every bearer token the door accepts | **none** — `TokenKey::from_secret` only (`crates/ordo-identity/src/token.rs:50`) |
| **page key** | MACs pagination cursors to space, op and principal | **none** — `PageKey::from_secret` only (`crates/ordo-api/src/page.rs:92`) |

The two with no loader are the two a hurried binary invents a constant for,
and `PageKey::from_secret(b"ordo-deployment-test")` is what the test harness
does (`tests/deployment.rs:288`). R1 gives both an env loader with the same
posture the other three already have: **unset is a refusal, never a
default** — the sentence `custody.rs:521` and `sign.rs:79-82` both already
write.

`ORDO_MASTER_KEY` deserves its own line, because it is the thing R2
replaces rather than the thing R2 builds on: it is a **plaintext master key
in the process environment**, readable from `/proc/<pid>/environ`, printed
by every `docker inspect`, and captured by every crash reporter that dumps
the environment. It is the right default for a laptop and the wrong one for
a deployment holding customer data. §2.

### 0.7 Every worker step is built; nothing calls one

The checklist says it plainly (`roadmap-checklist.md:3825-3830`): *"Every
loop step is built and tested — `claim_runs`, `fail_run`, `fire_due`,
`fire_clock`, `drain_listener` — and none of them is called from a
long-running task in `ordo-host`. That is deployment wiring, and it is the
one remaining sense in which 'nothing drains the queue' is still true."*

Verified, with the two the checklist does not list. The third column is the
state R3 was written against; the fourth is what calls each step now, and
the enabling condition is §3.4's:

| Step | Where | Called then? | Called now |
|---|---|---|---|
| `drain_listener` — log → batches, cursor last | `triggers.rs:726` | no | `triggers.drain`, when the program declares a listener |
| `fire_due` — batches → runs, one transaction | `triggers.rs:410` | no | `triggers.batch`, same condition |
| `claim_runs` / `finish_run` / `fail_run` | `triggers.rs:539`, `:579`, `:607` | no | `triggers.runner`, when a `RunHandler` is registered (12a) |
| `fire_clock` — `$schedules` → runs | `triggers.rs:805` | no | `triggers.clock`, when the program declares a clock routine |
| `claim_checkpoint_tick` / `claim_attest_tick` | `cadence.rs:254`, `:268` | no | `commandlog.checkpoint` / `commandlog.attest`, with `ORDO_CHECKPOINT_KEY` |
| `secevent::claim_retention_tick` / `retain_scope` | `secevent.rs` | **yes** — the `secevent.retain` worker | `secevent.retain` |
| `secevent::claim_checkpoint_tick` / `checkpoint_scope` | `secevent.rs` | **yes** — the `secevent.checkpoint` worker, plus `ordo checkpoint --security` | `secevent.checkpoint` |
| `prune::prune` | `prune.rs:812` | only `ordo prune`, the operator CLI | `commandlog.prune`, and only with `ORDO_WORKER_PRUNE=1` |
| `SecurityDrain::start` | `crates/ordo-host/src/secdrain.rs:255` | **yes — but only from the test harness** (`tests/deployment.rs:297`) | `runtime::assemble`, step 11, before the door binds |

`SecurityDrain` is the exception that proves R3's shape: it is already a
supervised background task with health, a bounded outbox, a
`shutdown() -> Result<usize>` that reports what it flushed (`secdrain.rs:328`),
and a `/health/deep` section that reports `wired: false` as a first-class
answer rather than an absence (`serve.rs:319-339`). R3 does not redesign it.
R3 generalises it and registers it as one worker among several.

**What R3 cannot deliver, stated now rather than discovered later.** There
is no routine *body* executor on any production path. `run_routine` exists
only in `crates/ordo-exec/tests/routine_provenance.rs:73`; the declaration
forms are checklist 12a and are not owed until §7's and §8's two spellings
are reconciled; `Actor::Routine` has no producer at all
(`roadmap-checklist.md:3800-3808`). So R3 delivers the **delivery loop** —
event to batch to run to claim to disposition — with a registered handler
seam where 12a's executor plugs in. §3.6 says exactly what that seam is and
what the done-when may therefore honestly assert.

### 0.8 Clock leadership is a session lock taken on a pooled connection

`TxConn::try_lead(realm)` runs `pg_try_advisory_lock` and documents its
contract precisely (`jobs.rs:210-229`): *"A session advisory lock is held
for the life of this connection and released when it closes, so a dead
leader's claim does not outlive its process."* Every word is true of the
lock. The hazard is in where the connection goes next.

`TxConn::commit` pushes its client back onto the pool's idle list
(`crates/ordo-exec/src/lib.rs:525-532`), and `rollback` does the same
(`:536-543`). So a leader elected through a pooled `TxConn` leaves a session
lock on a connection that immediately goes back to serving unrelated
queries, and the lock is released only when that connection dies. Within one
process the effects compound in the confusing direction: advisory locks are
re-entrant per session, so a *re-election on the same borrowed connection*
succeeds, while a re-election that happens to borrow a different one fails —
permanently, for the life of the process, with no error anywhere.

Nothing calls `try_lead` outside `crates/ordo-exec/tests/jobs.rs:119-126`,
which is why it has never bitten. It would bite on the first supervisor tick
of the first replica. **[pinned]** in §5: leadership is held on a dedicated
connection, never a pooled one.

The good news beside it: leadership is not load-bearing for correctness.
`claim_schedule`'s doc is explicit (`jobs.rs:130-136`) — *"This is the
fires-once property, and it does not come from leadership"* — `FOR UPDATE
SKIP LOCKED` plus the committed `next_due` advance is what makes firing
exactly-once. Leadership only keeps a hundred replicas from all trying at
once. A supervisor whose leadership is broken is wasteful, not wrong, which
is the right way round for a first version.

### 0.9 Two platform tables are not created at boot

`Pool::connect` calls `ensure_platform_tables`
(`crates/ordo-exec/src/lib.rs:286-309`), which creates six: replay, `$jobs`,
`$commandlog`, the security-event tables, the **trigger** tables, and
`$sealedpayload`. It deliberately retries once, because
`CREATE TABLE IF NOT EXISTS` is not race-free between two booting replicas
(`:287-295`).

Two tables are outside it:

- **`$logconsumers`** — `prune::ensure_tables` (`prune.rs:161`), called only
  by `ordo prune` (`crates/ordo-cli/src/operator.rs:388`). This is the
  checklist's own wrinkle (`roadmap-checklist.md:3831-3837`): *"a deployment
  that starts a listener before `ordo prune` has ever run finds no cursor
  table."* And the cursor table is not incidental — it is where
  `register_listener` pins the pruning floor (`triggers.rs:685-700`), which
  is at-least-once delivery seen from the other side.
- **`$attestations`** — `cadence::ensure_tables` (`cadence.rs:164`), called
  by nothing outside tests. Its own doc says the separation was incidental:
  *"Separate from `Pool::ensure_platform_tables` only because that function
  lives in a file this change does not own; a deployment should call both."*

Also outside it, and correctly so: `registry::ensure_tables`
(`crates/ordo-exec/src/registry.rs:190`), created by `ordo apply`
(`operator.rs:575`). The registry is the operator's, not the server's — a
server that created it could boot against an empty one and report
`NoActiveVersion` where the honest answer is "you have not deployed here".

§3.7 fixes the first two, and states why the fix is at the supervisor and
not in `ensure_platform_tables`.

---

## 1 · R1 — the `ordo-host` binary

### 1.0 What this supersedes

`plan-ci-images.md` §3.0's increment **0a** is one table row: *"Must
assemble `PlatformState` from a `Verified` deployment (never
`Deployment::pinned` to get past a red gate), bind `0.0.0.0` from
`ORDO_HOST_ADDR`, and refuse to start under `TestProvider` without an
explicit opt-in."* Every word of it stands. What it could not know, because
it was scoped as a CI blocker rather than as an entrypoint design, is:

- the write path's custody hole (§0.3), which makes "refuse under
  `TestProvider`" a **two-place** check rather than one;
- that the pool cannot authenticate (§0.4), so the binary cannot reach a
  managed Postgres at all;
- that the only wired access checker shells out to `psql` (§0.5);
- that two of the five secrets a deployment needs have no loader (§0.6).

0a is not wrong; it is a quarter of the work. This section is the whole of
it. `plan-ci-images.md`'s smoke assertions (`images.yml:277-323`) are the
acceptance test and are not restated here.

### 1.1 Configuration: every variable, and what absence means

**All configuration is environment.** No config file, no flags, no
dependency added. Three reasons and none is taste: an orchestrator supplies
environment, `deploying.md:267` already tells operators to *"Point
`ORDO_PG_*` at the database"*, and every existing loader in the tree
(`LocalKeyProvider::from_env`, `Ed25519Signer::from_env`,
`PoolConfig::from_env`, `Metrics::default`, `KeyTable::from_env`) is already
an environment reader. A second mechanism would mean two places to look.

**Three refusal postures**, and which one a variable gets is a decision, not
an accident:

| Posture | Meaning | Precedent in the tree |
|---|---|---|
| **Refuse to boot** | the deployment is unsafe or incoherent without it | `custody.rs:521` — *"refusing to run on a default key"* |
| **Refuse the feature, boot, and say so** | the platform is coherent without it, and pretending otherwise turns a missing option into an outage | `secdrain.rs:248` (retention with no checkpoint key); `serve.rs:319-327` (`wired: false`) |
| **Default, and publish the default** | a tuning knob whose absence has one obvious right answer | `Metrics::default` (`metrics.rs:263-287`) |

What is **never** available: silently defaulting a secret, and silently
disabling a mandatory control. `Metrics::default`'s own doc draws that line
for the deny-volume alert — *"A mandatory control that a missing variable
silences is not a control"* — and R1 applies the same rule everywhere.

#### The database

| Variable | Posture | Notes |
|---|---|---|
| `ORDO_PG_HOST` | **refuse to boot** | `PoolConfig::from_env` defaults it to `/tmp/ordo-pg`, a Unix socket **directory** on the author's laptop (`lib.rs:82`). Inheriting that in a server is how a deployment ends up pointed at nothing. The binary reads the variable itself and refuses when unset. |
| `ORDO_PG_PORT` | default `5432` | The harness default is `55432`, the throwaway cluster's deliberately non-default port. A server has no business inheriting it. |
| `ORDO_PG_USER` | **refuse to boot** | |
| `ORDO_PG_DB` | **refuse to boot** | Same argument as the host: `ordo_conformance` is a test database's name. |
| `ORDO_PG_PASSWORD` | optional, **new** | §0.4. `PoolConfig` grows `password: Option<String>` and `pg()` calls `.password(…)` when it is `Some`. Unset is legitimate (peer/trust auth, a socket, an IAM sidecar) so it cannot be a refusal — but a connect failure that names an authentication problem prints *"set `ORDO_PG_PASSWORD` if this server requires one"*, because the text Postgres returns names neither the variable nor the fact that this client never reads `PGPASSWORD`. |
| `ORDO_PG_POOL_SIZE` | default: `available_parallelism().clamp(2, 8)` | Today's computed default (`lib.rs:85-88`), now overridable; `deploying.md:30-33` already describes `size` as a deployment decision. |
| `ORDO_PG_ACQUIRE_TIMEOUT_MS` | default `2000` | `PoolError::Saturated` is retryable by design; the number belongs to the deployment. |

**`ORDO_PG_PASSWORD` is a secret in the environment and the plan says so.**
It is not R2's to fix — a database password reaches libpq-shaped clients
through the environment everywhere, and unlike the master key it protects
one database rather than every space's ciphertext in every backup.
`ORDO_PG_PASSWORD_FILE` is accepted as an alternative, for the same reason
R2 prefers a file for the master key, and is the form the image docs
recommend.

#### The socket

| Variable | Posture | Notes |
|---|---|---|
| `ORDO_HOST_ADDR` | default `127.0.0.1:8080` | Loopback by default, exactly `plan-ci-images.md` 0c's argument for the playground: a `cargo run` must not put a door on the network. The image sets `0.0.0.0:8080` explicitly (`images.yml:289` already does), so exposure is a deliberate line in a manifest. |
| `ORDO_SHUTDOWN_GRACE_SECS` | default `30` | §1.5. |

#### The space

| Variable | Posture | Notes |
|---|---|---|
| `ORDO_SPACE` | **refuse to boot** | Today the space is the compile-time constant `ordo_host::SPACE = "conformance"` (`lib.rs:84`), the executor refuses any other (`lib.rs:518-522`), and the fixture's own space is pinned to `FIXTURE_SPACE` at construction (`crates/ordo-conformance/src/lib.rs:319`). A production deployment whose tenant is named `conformance` is not a deployment. R2 makes the space an input to loading; R1 reads it and refuses when unset. Multi-space serving stays what `plan-dev-loop.md` §6 calls it — unbuilt, and needing a space directory. |

#### Custody, and the `TestProvider` refusal

| Variable | Posture | Notes |
|---|---|---|
| `ORDO_KEY_PROVIDER` | **refuse to boot** | An explicit selector: `local`, `sealed-master` (R2), `test`. Not inferred from which other variables happen to be set — a deployment that meant `sealed-master` and mistyped the file path must get a refusal, never a silent demotion to something weaker. |
| `ORDO_MASTER_KEY` | required by `local` | Exists (`custody.rs:519-529`), already refused-when-unset with a length floor. Its **plaintext-in-environment** cost is R2's subject. |
| `ORDO_MASTER_KEY_FILE`, `ORDO_MASTER_KEY_PASSPHRASE_FILE`, `ORDO_KMS_KEY_URI` | required by `sealed-master` | R2, §2.3. |
| `ORDO_DEV_UNSAFE_TEST_KEYS` | the opt-in, default off | The only thing that makes `ORDO_KEY_PROVIDER=test` selectable. |

**Refusing `TestProvider` is two checks, not one**, and §0.3 is why. The
first is the obvious one — the provider the operator selected. The second is
the one a plan that had not read `write.rs` would miss: after `load_active`
returns, the binary asserts that the loaded program's custody **is the
provider it supplied**, because the write path seals through
`Loaded.fixture.keys` and nothing on the production path sets that field.
Until R2 lands the injection this assertion fails on every boot — which is
correct, and is why R1 and R2's first half ship together (§6).

The assertion is by **identity**, not by type name: `Arc::ptr_eq` between
the provider the binary constructed and `live.fixture.keys`. A type-name
check (`provider_name`, `serve.rs:341-346`) would pass for a second,
differently-configured `LocalKeyProvider` — which is precisely the shape of
the bug being guarded against.

And one more, from `implementation.md:358-363`, which asks for it in as many
words: **a `local` master key whose backing store is the deployment's own
backed-up Postgres is a misconfiguration the binary refuses to start on.**
Key material in a database backup defeats the property the whole erasure
story rests on — *"a crypto-erased space stays erased through any restore
that does not also restore the KeyProvider's key material"* (`:364-369`).
R2's `PgKeyStore` (§2.4) holds **wrapped** keys, which is why it is
permitted; what is refused is the *master* key living there.

#### The door's two secrets

| Variable | Posture | Notes |
|---|---|---|
| `ORDO_TOKEN_KEY` | **refuse to boot** | 64 hex characters. New loader `TokenKey::from_env`, on `Ed25519Signer::from_env`'s exact shape (`sign.rs:113-122`) including its refusal wording. A defaulted token key means every deployment sharing this build can mint tokens for every other. |
| `ORDO_PAGE_KEY` | **refuse to boot** | New loader `PageKey::from_env`. The cursor MAC binds a page to space, op and principal (`crates/ordo-api/src/page.rs:86-104`); a shared key makes a cursor minted in one deployment valid in another. |

Both are what `deploying.md:268` already tells operators to do — *"Set the
page key, the token key, and the `KeyProvider` from real secret material,
not defaults"* — with, until now, no mechanism to do it through. The test
harness's `PageKey::from_secret(b"ordo-deployment-test")`
(`tests/deployment.rs:288`) is what a binary without a loader copies.

#### Signing keys, and the features whose absence they disable

| Variable | Posture | Notes |
|---|---|---|
| `ORDO_CHECKPOINT_KEY`, `ORDO_CHECKPOINT_KEY_ID` | **refuse the feature** | `Ed25519Signer::from_env` (`sign.rs:113`). Unset means the checkpoint and attestation workers do not start; `ordo checkpoint` from the operator CLI still works, so history stays anchorable by hand. Named in the startup log (§1.6) and on `/health/deep`. |
| `ORDO_SECURITY_CHECKPOINT_KEY`, `_KEY_ID` | **refuse the feature** | `store.rs:114-116`. Unset means security-stream retention reports itself INACTIVE and prunes nothing rather than cutting at a position no signature vouches for — already what `DrainConfig`'s default does, and `tests/deployment.rs:301-308` exercises exactly that case on purpose. Correct behaviour, not a degradation. |

#### Tuning: all defaulted, all published at boot

`ORDO_DENY_ALERT_THRESHOLD`, `ORDO_DENY_ALERT_WINDOW_SECS`
(`crates/ordo-secevent/src/alert.rs:89-91`), `ORDO_METRICS_PER_SPACE`
(`crates/ordo-host/src/metrics.rs:281`), `ORDO_KEY_ABBREVIATIONS`
(`crates/ordo-schema/src/lib.rs:386`), plus R3's worker knobs (§3.4). The
binary reads none of these itself — it lets the existing constructors read
them and **prints what they resolved to**, which is the half that is missing
today.

**A documentation defect found while enumerating, and it is the kind that
costs an incident.** `howto-operating.md:103` tells operators the
deny-volume threshold is set via `ORDO_DENIAL_VOLUME_THRESHOLD_PER_SPACE`.
No such variable exists anywhere in `crates/`; the code reads
`ORDO_DENY_ALERT_THRESHOLD` (`alert.rs:89`). An operator who sets the
documented name gets the built-in default and an alert surface that reports
itself *unconfigured* in a body they did not read. Fixed in R1's docs
commit; recorded in §7.

#### `ORDO_APEX_DOMAIN` — named, and honestly deferred

`licensing.md:237` and `security-logging-design.md:855` both say the host
binary reads it at boot and that the engine should refuse to start without
it; `audit-board-coverage.md:133` records that there is no
`ORDO_APEX_DOMAIN` anywhere in `crates/` and that the checklist files it
under "left to the deployment". Both cannot stand.

R1's answer: **the refusal belongs to the passkey provider, not to the
binary**, and no passkey provider is wired
(`crates/ordo-identity/src/passkey.rs:26` says so). A binary that refused to
start over a missing RP id, with nothing that could consume one, would be
refusing on behalf of a feature it does not have. So R1 reads the variable
when set, records it in the startup log, and **the refusal lands in the
increment that mounts the passkey endpoints** — the rule
`plan-production-ops.md` §M2 states for `quota_exceeded` and that
`inventory_is_closed.rs:29-39` enforces one layer up: a control ships with
its producer. `[proposed]`; §7.

### 1.2 The assembly, in order, and why the order is the design

```
1  read and validate the environment            → every refusal here, before any I/O
2  Metrics::new()                               → the substrate the door and workers report into
3  Pool::connect(cfg)                           → dials every connection; runs ensure_platform_tables
4  supervisor bootstrap DDL                     → prune::ensure_tables + cadence::ensure_tables (§3.7)
5  build the KeyProvider                        → refuse `test` unless the dev flag says otherwise
6  deploy::load_active(pool, cfg, keys, space)  → compile · register · gate · drift ⇒ Verified
7  assert Arc::ptr_eq(keys, loaded.fixture.keys) → §0.3, the second TestProvider check
8  Deployment::boot(verified)                   → one lock, one Loaded
9  AccessEngine, built from the pool            → §1.3
10 IdentityAccess::sharing(engine, token_key, …) → one engine, shared with platform_ops
11 SecurityDrain::start(recorder, pool, metrics) → the door's own recorder, not a second one
12 ApiState { registry, executor, access, bus, replay, attest, page_key }
13 PlatformState::new(…).with_security_drain(health)
14 Supervisor::start(…)                         → R3
15 axum::serve(listener, api.merge(platform)).with_graceful_shutdown(…)
```

Four orderings are load-bearing, and a hurried entrypoint reorders all four:

- **Every environment refusal precedes every dial (1 before 3).** A
  deployment missing its token key should fail in milliseconds with one
  message, not after a pool timeout against a database it should never have
  touched. It also means a misconfigured replica in a rolling deploy dies
  before it takes a connection slot from a healthy one.
- **The gate precedes the surface (6 before 13).** Enforced by the type
  system already: `PlatformState::new` takes an `Arc<Deployment>`,
  `Deployment::boot` takes a `Verified`, and `Verified`'s only constructors
  run the gate (`deploy.rs:123-171`). The binary's contribution is to *not*
  reach for `Deployment::pinned` (`deploy.rs:464`) when the gate is red. A
  red gate is a non-zero exit carrying `LoadRefusal`'s own text, which
  already names the op and both shapes (`tests/deployment.rs:97-119` asserts
  that text).
- **The drain starts before the door (11 before 15).** The door writes
  denials into `IdentityAccess::stream`, a bounded in-memory recorder whose
  own doc says entries *"accumulate in memory and are lost on restart"* with
  no drainer (`lib.rs:958-961`). Starting the door first opens a window
  whose only denials are the ones that get dropped.
- **The supervisor starts before the door (14 before 15).** A write that
  lands before `drain_listener` has a cursor is a write whose events are
  read from position 0 on the first drain — correct, but it makes the
  listener's first pass walk the whole log. Registering cursors before
  traffic keeps the first drain bounded.

`load_active` returning `LoadRefusal::NoActiveVersion` (`deploy.rs:364`) is
**not** a boot failure. It is a deployment nobody has run `ordo apply`
against yet — exactly the image-smoke case `plan-ci-images.md` §3.3
describes: the process starts, `/health/live` answers 200, `/health/ready`
answers 503 naming `"registry"` and `"shape"`, `/health/deep` answers 403.
The binary boots into a **no-program** state, says so once in the startup
log, and subscribes for a deploy notification (§1.4). Refusing to start
would make `ordo apply` impossible to run against a fresh deployment through
the ordinary path: you would need a database with a program in it before you
could start the server the program is deployed to.

### 1.3 The access engine, without `psql`

§0.5: `IdentityAccess` is the door R1 mounts, and it needs an
`Arc<AccessEngine>`. The only builder is `deployment_engine(f, pg)`
(`lib.rs:1269`), which reads `members` through `Pg::query` — the `psql`
shell-out.

R1 adds `deployment_engine_pooled(&Loaded, &Pool) -> Result<AccessEngine>`:
the same function with its one directory read replaced by
`pool.rows(r#"SELECT "id", "role" FROM "members" ORDER BY "id""#)`. Every
other line — the role mapping, the per-role profile, `std` ops being
registry-present and exposure-opt-in, the `deploy_op` manifest pins, the
reserved platform-op grants — is unchanged and **must** be, because that
function is where `std-library.md` §5's rules 2 and 3 are implemented and
where the capability pins that make ops truth enforceable are installed
(`lib.rs:1313-1327`).

Two things this deliberately does **not** do:

- **It does not delete `deployment_engine`.** The conformance suites call it
  with a `Pg`, and rewriting them is not R1's change. Two callers, one body,
  one directory query behind a small trait — the shape
  `ordo_exec::undo::Db` already uses.
- **It does not make the directory live.** The engine is built once at boot
  and rebuilt on a swap, so a membership change takes effect on the next
  swap or the next restart. That is *weaker* than `IdentityAccess`'s own doc
  promises — *"a suspension takes effect on the next request rather than at
  the next expiry"* (`lib.rs:926-929`) — and the sentence is true of the
  engine's `now`-driven checks, not of its membership table. **[open]**,
  §7: a membership refresh on the invalidation bus is the obvious answer,
  and it is a change to `ordo-identity`'s ownership of that state rather
  than to the binary.

`MemberAccess` stays where it is, used by the tests that use it. R1 adds a
module note recording that it is not a deployable door and why — so the next
person assembling a binary does not reach for it because it has the shorter
constructor.

### 1.4 The router, and the deploy channel

```rust
let app = ordo_api::router(api_state)
    .merge(ordo_host::serve::platform_router(platform_state));
```

That is the composition `serve.rs:26-28` documents and
`tests/deployment.rs:180-183` performs. Nothing new.

Beside it, `deploy::listen_for_deploys(&cfg)` (`deploy.rs:573`) — its
**first caller**, which is the property the channel split exists to provide
(`registry.rs:82-88`: an invalidation says your rows may be stale, a deploy
says your whole registry is). A notification names a version; the node
`load_version`s it, and on success `Deployment::swap` installs it.

The refusal policy is already written and R1 only has to obey it
(`deploy.rs:567-572`): *"a node that cannot load the new version must not
stop serving the old one, and the refusal is an operator alert, not an
outage."* So a failed swap increments
`ordo_shape_verification_failures_total`, logs `LoadRefusal`'s text, and
changes nothing about what is being served. `Swapped::AlreadyServing` logs
distinctly from `Swapped::Installed`, because several replicas hearing one
`NOTIFY` is the ordinary case and must not read as several swaps
(`deploy.rs:386-389` says exactly this).

**A swap rebuilds the access engine** (§1.3) and re-runs the custody
assertion (step 7), because a swapped-in `Loaded` carries a freshly compiled
fixture — and therefore, until R2, a fresh `TestProvider`. A boot-time-only
assertion would not catch it. This is the second reason the check is
`Arc::ptr_eq` rather than a type name.

`jobs::listen`'s doc is explicit that reconnection is not handled and that a
dead channel surfaces as the receiver closing (`jobs.rs:271-276`): *"A bus
that went quiet while still claiming to be live would be worse than one that
says it stopped."* R1 honours that by making the deploy listener a
**supervised worker** (§3.3) — its exit is a restart with backoff and a
counted metric, not silence.

### 1.5 Graceful shutdown

`SIGTERM` and `SIGINT`. Seven steps, each because something in the tree
would otherwise lose data:

```
1  stop accepting            axum with_graceful_shutdown; the listener closes
2  drain in-flight HTTP      bounded by ORDO_SHUTDOWN_GRACE_SECS (default 30)
3  cancel the workers        one CancellationToken; each finishes its current unit
4  flush the security drain  SecurityDrain::shutdown() -> Result<usize>
5  release leadership        drop the dedicated leadership connection (§3.5)
6  close the pool            drop; every socket closes
7  exit 0 — or 1 if step 2 hit its bound
```

- **1 before 3.** Workers use the pool; so do in-flight requests. Stopping
  the workers first frees connections for the requests that are finishing.
- **2 is bounded, and the bound is published.** An in-flight write holds the
  per-space append lock; a shutdown that killed it mid-transaction is a
  rollback — safe, but a refused customer write. Thirty seconds is longer
  than any bounded write and matches every orchestrator's default kill
  grace, which is precisely why **`terminationGracePeriodSeconds` must
  exceed it**: a platform grace equal to the app's is a SIGKILL wearing a
  rollout's clothes. Stated in the docs commit.
- **4 after 3, and after 1.** The drain's input is the door's recorder and
  the workers' events; flushing before either has stopped flushes a moving
  target. `shutdown()` already returns how many entries it appended
  (`secdrain.rs:328`). The binary logs that number, and logs any *unflushed*
  remainder as an error — a bounded, known loss of audit record belongs in
  the log rather than in nobody's memory.
- **5 is the one nobody would think of** without §0.8: leadership is a
  session lock, so releasing it means closing its connection. Doing that
  explicitly, before the pool goes, lets the replacement replica take
  leadership in milliseconds instead of waiting out a TCP timeout.

`plan-production-ops.md` §M1 asks for one more thing here — *"Also drained
on graceful shutdown"* — and R3 gives it a slot (§3.6): a worker may
register a **final act** that runs between steps 3 and 4.

### 1.6 The startup log

One block, once, on stdout, before the listener binds. It is the
deployment's answer to *"what did you decide?"*, and it exists because every
posture in §1.1 that is not "refuse to boot" is a silent difference between
two deployments that otherwise look identical.

As built, one `worker` line per running worker carrying the cadence it is
actually on, and one `DISABLED` line per worker that is not:

```
ordo-host <version>  addr=0.0.0.0:8080  space=acme
  database    ordo@db.internal:5432/acme  pool=8  acquire=2000ms  tls=off
  custody     sealed-master (file)  epoch=3
  program     v_9f2a41  ops=27  gate: 26 verified, 1 no-result-shape, 0 drift
  door        identity tokens; delegations: registry present
  metrics     per-space series: off   deny-alert: 50 in 300s (configured)
  worker      triggers.drain           tick 1000ms
  worker      triggers.clock           tick 1000ms, leader only
  worker      keys.reseal              tick 60000ms, leader only
  worker      provisioning.reconciler  tick 60000ms, leader only
  worker      secevent.drain           unsupervised: its own tick, its own flush
  DISABLED    checkpoint       — ORDO_CHECKPOINT_KEY is not set
  DISABLED    attest           — ORDO_CHECKPOINT_KEY is not set
  DISABLED    secevent.retain  — ORDO_SECURITY_CHECKPOINT_KEY is not set
```

The cadence is on the line because a tick is overridable (§3.4) and an
operator who set `ORDO_WORKER_<NAME>_TICK_MS` needs to see that it took
effect. Printing it for every worker rather than only the overridden ones is
the same posture as the `DISABLED` lines: a default nobody states is a default
somebody will assume is something else.

Three rules for it:

- **A disabled feature gets a line naming the variable that would enable
  it.** That is the whole point: a feature that is off and unmentioned is a
  feature an operator believes is on. It is `/health/deep`'s `wired: false`
  posture (`serve.rs:319-327`) moved to the moment somebody is looking.
- **No secret, no derived secret, and no length of a secret.** Custody
  prints the provider kind and the current epoch — the same two facts
  `/health/deep` already discloses to an owner (`serve.rs:266-271`) — and
  nothing else. The database line prints user, host and database, never
  whether a password was supplied: "password: set" is a fact about the
  environment of a process an attacker is already reading.
- **The gate's summary is printed, not only its verdict.** `ShapeReport`
  already renders it (`report.summary()`, used at
  `tests/deployment.rs:83`), and the counts are how an operator notices the
  gate going hollow — a coverage collapse that still passes is exactly what
  `tests/deployment.rs:52-62` asserts against.

Logging is `println!`/`eprintln!`. **No logging framework, no structured
logs, no tracing.** `howto-operating.md:97-101` already records that the
platform has none and says so honestly; adding one inside the entrypoint
would make the binary's observability better than the library's, which is
the wrong way round. Named in §7.

### 1.7 The seam `plan-dev-loop.md` D3 depends on

D3 is *"bind a socket to `ordo_api::router(...).merge(platform_router(...))`,
run the gate, build a `Verified` through `from_gate`, `swap`, and let the
existing bus tell connected clients"*, and its Deferred section already says
how the two must relate: *"D3 binds a socket for dev. A production server
binary is `plan-ci-images.md`'s blocker and its own piece of work; D3 should
be written so `ordo serve` is the same composition minus the watcher, and
should say so in code rather than duplicate it."*

R1 provides that in three items:

```rust
pub struct RuntimeConfig { /* every §1.1 value, already validated */ }
impl RuntimeConfig { pub fn from_env() -> Result<RuntimeConfig, BootRefusal>; }

pub struct Runtime { /* pool, keys, deployment, api, platform, supervisor */ }
pub async fn assemble(cfg: &RuntimeConfig) -> Result<Runtime, BootRefusal>;
pub async fn serve(rt: Runtime, addr: SocketAddr) -> Result<(), ServeError>;
```

`assemble` is steps 2–14 of §1.2 over the **struct**, never over the
environment; `from_env` is the only environment reader and `main.rs` is
`from_env` → `assemble` → `serve` and nothing else. Then:

- `ordo dev` (D3) is `assemble` over a config it built itself, `serve`, and
  a watcher calling `deployment.swap(verified)` on save.
- The conformance harness's `boot` (`tests/deployment.rs:204`) can migrate
  onto `assemble` when someone wants it to — which is how the binary stops
  being able to drift from the thing the suite proves.

The split is not architecture for its own sake: it is the only way D3's dev
loop and R1's server are the same gate, and it keeps `plan-dev-loop.md` §7's
third option — *"Serve ungated in dev. Rejected outright"* — rejected by
construction rather than by discipline.

### 1.8 What R1 does not do

- **It serves one space**: `ORDO_SPACE`, asserted against the executor's.
  Multi-space needs a space directory, which does not exist
  (`plan-dev-loop.md` §6; `plan-schema-evolution.md` §8's deferred list).
- **It does not run DDL.** Neither does `ordo apply`
  (`crates/ordo-cli/src/operator.rs:501`). Schema evolution is
  `plan-schema-evolution.md`'s. A binary whose database disagrees with its
  program refuses to load that program and names the op and the column.
- **It adds no CD.** `plan-ci-images.md` §4.1's argument stands unchanged: a
  human names the version.
- **It adds no TLS**, inbound or outbound. §4 says where each belongs.

**Done when** `cargo build -p ordo-host --bin ordo-host` produces a binary
that boots against a service Postgres and answers `/health/live` 200,
`/health/deep` 403 unauthenticated, `/health/ready` 200-or-503 — and exits
non-zero *before dialing* for each of: no `ORDO_MASTER_KEY`, no
`ORDO_TOKEN_KEY`, no `ORDO_PAGE_KEY`, no `ORDO_SPACE`, and
`ORDO_KEY_PROVIDER=test` without the dev flag.
And when a `SIGTERM` under load finishes every in-flight request, logs the
number of security entries it flushed, and exits 0 — while a request issued
after the signal meets a closed listener rather than a process disassembling
itself underneath it.

---

## 2 · R2 — a production `KeyProvider`

### 2.0 What 8a actually landed, and what three documents disagree about

`roadmap-checklist.md:931-950` marks 8a–8d `[x]` at `f9939e9`.
`deploying.md:77-86` still opens §4 with *"`ordo-crypto` today ships one
provider — `TestProvider`, whose master key is a constant in this repository
— and it is constructed at its call sites, so there is no seam to swap it."*
And `roadmap-checklist.md:4502-4506`, written later, calls the KeyProvider,
the page key and the token key *"all three unbuilt, items 8a–8d"*.

The code settles it, and all three documents are partly right:

| Landed at `f9939e9` | Still absent |
|---|---|
| the `KeyProvider` trait, exposing operations and never keys (`custody.rs:206-398`) | any `WrappedKeyStore` over **Postgres** — `MemoryKeyStore` is the only implementation (`custody.rs:429`) |
| per-space SMKs, wrapped-not-derived DEK epochs, `rotate`/`destroy_epoch`/`destroy_space` | a master-key source that is not **plaintext in the environment** (`custody.rs:519-529`) |
| `LocalKeyProvider` over an injected store, and `from_env` refusing rather than defaulting | any **injection at load**: `verify_candidate`'s `keys` argument never reaches the write path (§0.3) |
| the injection seam itself — `Fixture::keys` is a field, not a call site | a way to **provision** a space's custody outside `LocalKeyProvider`'s inherent methods |

`roadmap-checklist.md:1160-1163` is the sharpest statement of the gap and it
is about the operator tool rather than the server: *"`ordo revert` itself
still refuses a sealed entity, for the reason that is now true: **the binary
has no `KeyProvider`** (there is no `WrappedKeyStore` over Postgres for it to
boot one against), stated rather than faked."* One `PgKeyStore` (§2.4)
closes that and R1's custody question at once.

So R2 is four things, in this order: **the injection (2.1)**, **the
`PgKeyStore` (2.4)**, **the sealed-master source (2.3)**, and **the ceremony
(2.5)**. The first is a prerequisite for R1 shipping at all.

### 2.1 R2a — custody and tenancy are inputs to loading a program

The defect is §0.3. The fix is four lines in `verify_candidate`
(`deploy.rs:324-328`), between compiling the fixture and wrapping it in an
`Arc`:

```rust
let mut fixture = Fixture::from_source(&stored.source, "")…?;
fixture.keys = keys.clone();          // the write path's custody — the field is already `pub`
let fixture = Arc::new(fixture.in_space(&space.0));   // and its tenancy
```

`Fixture::keys` is already a public field, documented as *"INJECTED rather
than constructed at call sites … the seam F0's done-when asks for"*
(`crates/ordo-conformance/src/lib.rs:227-231`). `in_space` already exists and
already says why it is the right shape — *"One schema, one registry, one key
provider (which derives per space already), and a different tenancy value"*
(`:324-332`). Nothing is invented; something is finally connected.

Two guards keep it connected:

- **In `verify_candidate`**, an assertion that the returned `Loaded`'s
  fixture custody is `Arc::ptr_eq` with the argument. A future refactor that
  reintroduces a `Fixture::from_source` call after the assignment fails a
  test rather than shipping a silent downgrade.
- **In the binary**, §1.2 step 7, on every boot *and every swap*. Two
  independent mechanisms agreeing, which is the arrangement `mint_sealed_ref`
  and the payload store already use for handle scoping
  (`custody.rs:290-296`).

**Ruled (2026-09-08):** *a program's custody and its space are inputs to
loading it, never properties of its compilation.* Recorded in
`corpus-rulings.md`. The argument in one line: the compiler is a pure
function of source text, and a constructor that mints custody is a
constructor that makes the same source text mean different key material
depending on who called it — which is exactly how a test key reached a
production write path with nobody having made a decision.

`TestProvider` keeps its constructor and its constant. Nothing about it was
wrong; what was wrong was that it was the *default* on a path a deployment
takes. Its own doc already says the constant is deliberate — *"nothing it
seals is secret, and looking secret would be worse than being plain"*
(`custody.rs:723-725`) — and that sentence stays true only while nothing
production reaches it.

### 2.2 What `ORDO_MASTER_KEY` costs, stated once

`LocalKeyProvider::from_env` is not a stub and R2 does not delete it.
`implementation.md:546-556` settles that a built-in provider must exist so
that *"small deployments are not gated on a KMS"* and that the local provider
is *"a first-class implementation, not a stub"* which *"does not pretend: it
cannot protect keys from someone holding the machine."* R2 keeps it, and
keeps that disclosure.

What R2 refuses to keep is **the environment** as the only way to supply the
master key, for five reasons that are mechanical rather than atmospheric:

1. **`/proc/<pid>/environ`.** Any process running as the same uid reads it.
   In a container that is every sidecar sharing the process namespace.
2. **`docker inspect` prints it**, as does every orchestrator's pod
   description, and both are readable by roles that were never granted key
   access.
3. **Crash reporters dump the environment**, and a core dump of the serving
   process is a file somebody puts in a ticket.
4. **Children inherit it.** The operator tool forks `psql`
   (`crates/ordo-conformance/src/lib.rs:19`), so the deployment master key
   sits in the environment of every `psql` process an operator's `ordo
   apply` or `ordo prune` spawns — and in that process's own
   `/proc/<pid>/environ`. R1 removes the fork from the *door* (§1.3); it does
   not remove it from the CLI.
5. **The stretch is one HMAC.** `LocalKeyProvider::new` computes
   `hmac_sha256(b"ordo.master.v1", secret)` (`custody.rs:509-514`), and
   `from_env` enforces only a 16-character floor. A sixteen-character
   passphrase behind one HMAC is a sixteen-character passphrase; there is no
   memory-hard KDF between an operator's typing and the key that wraps every
   space.

(5) is the one that decides §2.3's design, and the workspace already has the
answer: `argon2 = "0.5"` is a dependency of `ordo-identity`
(`crates/ordo-identity/Cargo.toml:8`), which itself depends on **no** sibling
crate, and the OWASP argon2id profile is already pinned as constants —
`ARGON2ID_M_COST_KIB = 19_456`, `T_COST = 2`, `P_COST = 1`
(`crates/ordo-identity/src/provider.rs:309-316`). Adding `argon2` to
`ordo-crypto` adds one already-resolved, pure-Rust crate to the graph, so
`plan-ci-images.md` §1.5's "no `openssl`, no `bindgen`, no `cc`" property —
which is what makes `distroless/cc` and the four-target release matrix free —
survives intact.

### 2.3 R2b — the sealed-master source, and why it ships before any KMS

**The insight that keeps this small: `LocalKeyProvider` does not need
replacing.** `LocalKeyProvider::new(secret: &[u8], store)` already takes
bytes. Everything below is a *source* for those bytes, not a fifth
implementation of a trait with fifteen methods.

```rust
pub enum MasterKeySource {
    /// ORDO_MASTER_KEY. Kept, first-class for a laptop and a small
    /// self-host, and printed as `local (env)` in the startup log so the
    /// posture is never invisible.
    Env,
    /// ORDO_MASTER_KEY_FILE + ORDO_MASTER_KEY_PASSPHRASE_FILE.
    SealedFile { path: PathBuf, passphrase: PathBuf },
    /// ORDO_MASTER_KEY_FILE + ORDO_KMS_KEY_URI. Same file format; the
    /// unwrap step is a KMS call instead of an argon2id derivation.
    KmsEnvelope { path: PathBuf, uri: String },
}
impl MasterKeySource { pub fn open(&self) -> Result<[u8; 32], KeyError>; }
```

**One file format for both non-env sources**, and it is the format the crate
already has:

```
ordo-master-v1 ‖ key_id ‖ salt(16) ‖ seal_with(kek, 0, "master", key_id, master32)
```

where `kek` is `argon2id(passphrase, salt)` for `SealedFile` and the
KMS-unwrapped data key for `KmsEnvelope`. `seal_with`/`open_with` are
`crates/ordo-crypto/src/lib.rs:86` and `:120`; the wrap/unwrap pattern is
exactly what `LocalKeyProvider::wrap` already does for SMKs and DEKs
(`custody.rs:559-577`). No new crypto is designed here, and that is the
point: a key file whose format was invented for the occasion is a key file
nobody can audit.

**Recommendation: `SealedFile` ships first, and it is the default the docs
recommend for every deployment that is not already inside a cloud's IAM.**
Four reasons, each disqualifying for the alternative order:

1. **A KMS at boot makes a KMS outage a boot outage** — and boot is exactly
   when a fleet is rolling. `SealedFile` needs no network. The platform's
   stated dependency floor is *"no Redis, no broker, no coordinator, no cron
   runner: Postgres is the only stateful dependency that must exist"*
   (`implementation.md:160-162`); a mandatory KMS would be the second.
2. **It is testable in CI without a secret.** `plan-ci-images.md` §3.4 chose
   GHCR specifically so *"this plan requires no secrets at all"*. A
   sealed-master boot test is a file and a passphrase the test writes; an
   AWS/GCP KMS test is a cloud account, a credential, and a rotation
   obligation. R2's done-when (below) is only reachable at all with the file
   source.
3. **Self-host is first-class by ruling**, not by charity
   (`implementation.md:546-556`). A custody story whose only non-plaintext
   option is a cloud KMS makes self-host second-class in exactly the way
   that sentence forbids.
4. **It is the same file a KMS source later opens.** Moving to a KMS is then
   *"a provider swap plus a re-wrap, never a migration"* — the sentence
   `implementation.md:556` already promises — because the envelope, the
   store, the epochs and every digest are unchanged.

**The first KMS binding should be Vault Transit, not AWS KMS** — and this is
the recommendation most likely to be argued with, so the reasons are
concrete:

- **No SDK.** Transit is `POST /v1/transit/decrypt/<key>` with a token
  header. AWS's and GCP's Rust SDKs are large trees; the workspace's pure-Rust
  graph is an asset `plan-ci-images.md` §1.5 asks explicitly to keep, and
  §7 there names it as a thing to protect deliberately.
- **It is the only one CI can actually test**, for reason (2) above: Vault
  runs in a container beside Postgres in the same `services:` block. An
  untested KMS binding is a boot path nobody has executed.
- **It is the only one a self-hoster can also run**, which stops the KMS
  path from being a hosted-only feature and keeps one code path exercised by
  both audiences.

Then AWS KMS, then GCP KMS, **each behind a cargo feature, default off**, so
the shipped image's dependency graph and the release matrix stay what they
are. `implementation.md:86-90` lists all three plus Vault and settles the
shape — *"No provider is hardcoded anywhere"* — but names no order; this is
the order and it is `[proposed]`.

**The honest counter-argument, recorded rather than buried:** if the first
hosted customer is on AWS, invert the last two. The envelope format is
identical either way, so the order costs one increment to change and nothing
to have got wrong.

**And the disclosure a KMS envelope owes.** The KMS wraps the *master*; the
master is unwrapped once at boot and stays resident in process memory, and
per-space SMKs and DEKs are unwrapped from the store as needed. A
KMS-*native* provider — where key material never leaves the KMS — would mean
one network call per seal, that is, per sealed field per row, which is not
affordable at the write path's rate. So the envelope shape is the right one
and the disclosure is: **a KMS here buys "the key is not on the disk and not
in the environment"; it does not buy "the key never exists in this host's
RAM."** That sentence goes in `howto-custody.md`, in those words, beside the
local provider's existing *"it cannot protect keys from someone holding the
machine."*

### 2.4 R2c — `PgKeyStore`, the missing `WrappedKeyStore`

`WrappedKeyStore` exists precisely so *"a deployment's storage choice and its
custody choice are independent"* (`custody.rs:402-407`), and the Postgres
implementation the doc names as the self-host default has never been
written.

```sql
CREATE TABLE IF NOT EXISTS "$keycustody" (
  "space"   text   NOT NULL,
  "kind"    text   NOT NULL,   -- 'smk' | 'dek'
  "epoch"   int    NOT NULL,   -- 0 for an SMK row
  "wrapped" bytea  NOT NULL,
  PRIMARY KEY ("space", "kind", "epoch")
);
```

Four properties, each forced by something already written:

- **Only wrapped material is stored.** The master never appears, which is
  what makes the table safe to include in the database backup and what makes
  `implementation.md:358-363`'s startup assertion (§1.1) coherent: wrapped
  keys in the backup are the design; the master in the backup is the
  misconfiguration.
- **`drop_space` is one statement pair in one transaction.** The trait's doc
  requires it and says why: *"a store that fails halfway through the loop
  leaves a space whose erasure is partial, and partial crypto-erasure is
  indistinguishable from none for the field it did not reach"*
  (`custody.rs:416-424`). One `DELETE … WHERE "space" = $1` does it, which is
  strictly better than the in-memory store's two-lock dance.
- **It is created by `ordo keys init`, not at boot.** §2.5.
- **It unblocks `ordo revert` on sealed entities** (`roadmap-checklist.md:1160-1163`)
  and `ordo_exec::undo`'s `None`-`KeyProvider` signature note
  (`crates/ordo-exec/src/undo.rs:138-141`), which both name the absence of
  this exact type as their reason.

### 2.5 R2d — the key ceremony

Custody that only a running server can create is custody nobody performed.
Three operator acts, all in `ordo`, all `--as`-attributed the way every other
operator command already is (`deploying.md:228-229`).

**`ordo keys seal-master --out <path> [--key-id <id>]`** — mints 32 CSPRNG
bytes, prompts for a passphrase twice, derives the KEK with argon2id at the
pinned profile, writes the §2.3 file, and prints **the key id and a
fingerprint of the public-safe kind** — `sha256("ordo.master.fp.v1" ‖ master)[..8]`,
which is enough to tell two master keys apart in a log and useless for
recovering either. It never prints the key and never writes it anywhere but
the sealed file. Refuses to overwrite an existing file.

**`ordo keys init --space <s> --as <who>`** — creates `$keycustody` if
absent, then mints the space's SMK (random, wrapped under the master) and
epoch 1's DEK — `LocalKeyProvider::create_space` (`custody.rs:533`), which is
already idempotent. Records an `EventKind::CustodyAction { action:
"space_provisioned" }` in the security-event stream, which is where a
decision about a space belongs by the exit-ops precedent
(`crates/ordo-host/src/exit_ops.rs:24-36`).

**Provisioning is never a boot side effect**, and the code already argues
this better than a plan could. `LocalKeyProvider::release` refuses an unknown
space *"rather than silently creating keys, so a typo cannot mint a tenant"*
(`custody.rs:617-620`). A binary that provisioned `ORDO_SPACE` on demand
would turn a typo into a brand-new tenant with brand-new keys, serving an
empty space that looks perfectly healthy — and `TestProvider` demonstrates
exactly that failure as a *convenience*, with its own doc warning that it
*"re-creates custody on demand … That is convenience for fixtures, not a
custody model"* (`custody.rs:797-803`). So the binary refuses to boot on
`KeyError::UnknownSpace` and prints the remedy:

```
custody: no keys exist for space `acme`
  run:  ordo keys init --space acme --as <you>
  (the deployment will not mint custody for a space it was merely told about)
```

`/health/ready` would eventually say this too — it already probes
`current_epoch` (`serve.rs:205-211`) — but never-ready-with-a-named-check is
a worse operator experience than refuse-at-boot-with-the-command.

**`ordo keys rewrap-master --from <old> --to <new> --as <who>`** — reads
every `$keycustody` SMK row, unwraps under the old master, re-wraps under the
new, in one transaction. **This is not SMK rotation and the two must not be
confused**, because their costs differ by orders of magnitude:

| | moves | cost |
|---|---|---|
| **master re-wrap** (R2) | the wrapping of each space's SMK | one row per space; no envelope and **no digest** moves |
| **SMK rotation** (`implementation.md:557-560`, checklist 11-11) | every blind-index key, since BIKs are SMK-derived (`custody.rs:668-669`) | a full re-index of every `sealed indexed` column, offline build plus atomic swap under a write barrier |
| **DEK rotation** (`rotate`, plan-production-ops K) | the write epoch; old epochs stay readable | free at rotation, then a re-seal pass |

#### What the operator holds, and what loss means

**Two things, deliberately separable**: the sealed file and the passphrase
(or the KMS grant). The file may live on the volume or in the image; the
passphrase belongs in the orchestrator's secret store. Neither alone opens
anything, which is what makes it safe for the file to travel with the
deployment.

**Loss, in three cases, and the third is the reassuring one:**

| Lost | Consequence |
|---|---|
| the passphrase (file intact) | every space's SMK is unopenable. Every sealed field's value and every blind index in every space is gone at once. Skeleton columns, the command log's plaintext envelope, and every unsealed field still read. |
| the file (passphrase intact) | identical. |
| the **database** (both intact) | nothing is lost that the backup held. *"A restore restores ciphertext … a crypto-erased space stays erased through any restore that does not also restore the KeyProvider's key material"* (`implementation.md:364-369`) — and here the key material was never in the backup, so it opens. |

What the first two cases mean concretely, because "sealed data is lost" is
too soft: a read that projects a sealed field fails; a converge over a
`sealed indexed unique` field cannot address its row, because addressing goes
through a digest it can no longer compute; and **`revert` of any write that
sealed a value is refused**, because the revert path opens the payload to
produce the pre-image and a payload that cannot be produced is a typed
rejection rather than a guess (`howto-custody.md:221`).

**There is no escrow and there is no recovery, and that is a decision.**
`implementation.md:1069-1071` pins it: *"No silent escrow, ever. A recovery
path the user did not choose is a custody position they did not consent to."*
The ceremony document says the consequence in one sentence rather than
letting an operator infer it:

> The property that makes crypto-erasure real — that destroying key material
> is a delete, not a derivation — is the same property that makes losing key
> material unrecoverable. They are not two facts. Back up the sealed file and
> the passphrase separately, to somewhere that is not this deployment's
> database.

Splitting the passphrase across custodians (two-of-three, an envelope in a
safe) is an **operator practice the ceremony doc describes and the platform
does not implement**. Building Shamir into `ordo-crypto` would add a
threshold scheme to a crate whose entire dependency list is two crates, to
solve a problem an envelope and a policy already solve — and it would make
the platform a participant in a recovery story `implementation.md:1060-1071`
deliberately leaves to the person whose data it is.

### 2.6 What R2 does not do

- **No SMK rotation and no re-seal pass.** Checklist 11-11 and
  `plan-production-ops.md` track K. R2 supplies the `PgKeyStore` K1's worker
  needs and stops there.
- **No `policy keys { rotate <cadence> }` parsing.** The declaration surface
  is its own item; `cadence::LogPolicy` records the same situation for
  `policy commandlog` (`cadence.rs:76-79`).
- **No per-space provider selection.** One deployment, one provider, one
  master. BYO-KMS per space needs a space directory.

**Done when** the binary boots with `ORDO_KEY_PROVIDER=sealed-master`, a file
written by `ordo keys seal-master` and a passphrase file, provisions nothing,
refuses a space `ordo keys init` has not created — and, once initialised,
seals a real value through the door and reads it back byte-identically.
And when three refusals are tests rather than intentions: `ORDO_KEY_PROVIDER=test`
without the dev flag exits non-zero; a program loaded through
`verify_candidate` whose fixture custody is not `Arc::ptr_eq` with the
supplied provider fails to boot; and a wrong passphrase produces
`KeyError::Seal` with no indication of *which* byte was wrong, matching
`SealError`'s existing "every bad envelope is the same error" posture
(`crates/ordo-crypto/src/lib.rs:69-72`).

---

## 3 · R3 — the worker supervisor

### 3.1 The contract, as `plan-production-ops.md` already assumes it

That document names this supervisor three times and never defines it,
because it is this document's to define:

- *"**No worker supervisor.** K1 and M1 both need one; both ride the
  supervisor `plan-runtime.md` R3 defines rather than growing a second."*
  (`:55-57`)
- K1: *"A per-space, chunked, checkpointed, resumable pass, running on the
  worker supervisor `plan-runtime.md` R3 defines. **Not a new scheduler.**"*
  (`:197-198`)
- M1: *"Drained by a named schedule — `claim_schedule("usage.rollup")` … on
  the supervisor `plan-runtime.md` R3 defines. **Also drained on graceful
  shutdown.**"* (`:658-660`)

Five obligations fall straight out of those three sentences, and R3 is not
finished until all five hold:

1. **A worker is registered, not spawned inline.** "Rides the supervisor"
   means there is something to ride.
2. **The supervisor owns the tick; the worker owns the cadence semantics.**
   K1 and M1 both name `claim_schedule` explicitly, and `jobs.rs:153-165`
   explains why: `claim_due` advances every due schedule by whichever period
   the caller passed, so two cadences through one call means the loser adopts
   the winner's. A supervisor that owned periods would reintroduce exactly
   that bug one layer up.
3. **Chunked and resumable work must not be killed by a tick.** K1 is a pass
   over every sealed payload in a space. The supervisor gives a worker a
   cancellation token and never a deadline.
4. **A worker may register a final act at shutdown.** M1's partial period.
5. **Per-space where the work is per-space, per-deployment where it is not.**

And three this document adds, from §0.7 and §0.8:

6. **A worker panic must not take the door down** — and, less obviously,
   must not be *silent* (§3.3).
7. **Restart with backoff, and the restart is counted** (§3.3).
8. **A worker that is not running is named as not running** (§1.6, §3.4).

### 3.2 The `Worker` trait

Built at `crates/ordo-host/src/supervisor.rs`. Three details differ from the
design below and the code is what shipped: the future alias is `BoxFut`, not
`BoxFuture`; `on_shutdown` has a default body that does nothing, so a worker
with no final act does not have to write one; and cancellation is a local
`Cancel` (two `Arc`s and a `Notify`) rather than
`tokio_util::sync::CancellationToken`, which would have been a new dependency
for two fields.

```rust
pub trait Worker: Send + Sync + 'static {
    /// Stable. It is a metric label, a startup-log line and a /health/deep key.
    fn name(&self) -> &'static str;
    fn cadence(&self) -> Cadence;
    fn requires_leadership(&self) -> bool { false }
    /// One unit of work. Returns what it did, and whether there is more
    /// waiting — never loops internally.
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>>;
    /// Obligation 4. Runs once, between "workers cancelled" and "drain
    /// flushed" in §1.5. Defaulted to a no-op.
    fn on_shutdown<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<(), WorkerError>>;
}

pub enum Cadence {
    /// Fixed interval, this replica's own clock.
    Tick(Duration),
    /// A named row in `$schedules`, claimed by `claim_schedule`. Obligation 2.
    Schedule { name: &'static str, period: Duration },
    /// Runs continuously; `run` blocks on its own source (a LISTEN channel).
    Continuous,
}

pub struct Progress { pub units: u64, pub more: bool }
```

`Progress::more` is the piece that makes obligation 3 cheap: a chunked pass
returns `more: true` and the supervisor re-enters `run` immediately instead of
waiting out the tick, so K1 gets a throttle knob (`units` per chunk, an
inter-chunk delay) without an inner loop the supervisor cannot cancel. A
worker with nothing to do returns `more: false` and sleeps its cadence.

`WorkerCtx` carries the pool, the `KeyProvider`, the `Arc<Deployment>` (so a
worker reads the *live* program, not a snapshot from boot — the same reason
`PlatformState` holds the deployment rather than the registry,
`serve.rs:81-88`), the `Arc<Metrics>`, the space, a `now()`, the cancellation
token, and one thing the design did not anticipate: a `Timing`. `Timing`
holds the backoff unit and the leadership tick, one second and five seconds
in production, and it exists so a quarantine test can reach the tenth
consecutive failure in seconds without changing the backoff *sequence* or the
threshold, both of which stay `triggers::backoff_secs` and
`QUARANTINE_AFTER`. A test that had to wait out five minutes of real backoff
is a test somebody eventually marks `#[ignore]`.

A trait rather than a closure for three reasons that are each mechanical: the
name has to be `'static` because it is a metric label; the cadence has to be
inspectable *before* the worker runs, for the startup log's enabled/disabled
census; and `on_shutdown` has to exist on every worker without every
registration site carrying a second closure it will forget to supply.

### 3.3 Supervision: panics, backoff, quarantine, and the metric

Each registered worker is one `tokio::spawn`ed loop. The loop is where every
obligation in §3.1 that is not about scheduling gets discharged.

**Panics are caught, counted, and restarted from.** `run`'s future is wrapped
in `AssertUnwindSafe(fut).catch_unwind()` — `futures` is already a dependency
of `ordo-host`, and the workspace sets no `panic = "abort"`, so unwinding is
available. This is obligation 6, and the reason it needs saying is that the
default is *worse than a crash*: a panic in a spawned tokio task kills that
task alone and nothing anywhere notices. A trigger runner that panicked once
on a malformed payload would stop delivering forever, the door would keep
answering 200, and `/health/ready` would keep saying ready. Silent is the
failure mode; the door surviving is already true.

**A caught panic is not a resumed program, and one hazard says so.**
`Metrics` holds its state behind `std::sync::Mutex`
(`crates/ordo-host/src/metrics.rs:116`, `:237-260`) and every reader takes it
with `.expect(...)`. A panic *while holding* one poisons it, and the next
denial the door records panics too — so catching a worker panic can convert
a worker fault into a door fault through the shared registry. R3 does not fix
`Metrics`; it records the hazard (§7) and makes the supervisor's own
accounting panic-free by construction (atomics, not the `Mutex` families).
`Fixture`'s shared `Arc<Mutex<WriteLog>>` on the write path
(`crates/ordo-host/src/lib.rs:413-415`) is the same class of hazard and is
named in §7 too.

**Backoff reuses the platform's one backoff.** `triggers::backoff_secs`
(`triggers.rs:530-537`) is `1 << clamp(attempts, 0, 6)` — a second, then two,
capped at sixty-four — and it is engine-pinned there *"where an operator can
find it rather than buried at a call site"* (`:521-526`). The supervisor
calls that function. One shape of backoff in the platform, not two.

The consecutive-failure counter resets on a run that *succeeds and lasted
longer than the current backoff*, not on any success. A worker that fails
after doing one unit of work would otherwise sit at attempt 1 forever and
hammer a broken dependency at one-second intervals while reporting progress.

**After ten consecutive failures a worker is quarantined, not restarted
forever.** It stops, and it is reported — `quarantined` on `/health/deep`
beside the security drain's section, and a counter. Two reasons this is the
right end state rather than an infinite loop:

- An infinite retry against a permanently broken dependency is a log flood
  that hides every other fault, which is how an incident gets longer.
- Stopping is **visible in the data**, not only in a log, and it is visible
  in the right place. A quarantined `triggers.drain` stops advancing its
  listener cursor, and a listener cursor pins the pruning floor and is
  non-evictable by design (`triggers.rs:44-49`, `prune::ConsumerKind`). So a
  broken listener shows up as a log that stops shrinking — the exact signal
  §7's floor rule is built to produce, arriving without anyone adding a
  second alarm. At-least-once is preserved throughout: nothing is dropped,
  delivery is merely paused.

**The metrics**, on the existing registry. The first four are the counters the
design named; the last two arrived with leadership failover (§3.5) and the
last is a **gauge**, which the registry did not have when the paragraph below
was written:

| Series | Labels | Answers |
|---|---|---|
| `ordo_worker_runs_total` | `worker`, `outcome` = `ok`/`error`/`panic` | is it running, and is it working |
| `ordo_worker_restarts_total` | `worker`, `reason` = `error`/`panic` | obligation 7 |
| `ordo_worker_units_total` | `worker` | is it making progress, as distinct from merely running |
| `ordo_trigger_deadletters_total` | `listener` | §3.6 |
| `ordo_supervisor_leadership_changes_total` | `to` = `leader`/`follower` | a flapping leader, which a scrape interval can miss on the gauge alone |
| `ordo_supervisor_leader` (gauge) | none | is this replica the one running the leader-only half |

`ordo_supervisor_leader` is set only by a supervisor that has a leader-only
worker to run, so an **absent** series is a replica that never competed, which
is a different fact from a replica that competed and lost.

`runs_total{outcome}` and `units_total` are deliberately separate: a drain
that runs a thousand times and moves zero units is healthy-looking on the
first series and is the actual failure. This is the same distinction
`DrainHealth` already draws between `appended` and `failures`
(`secdrain.rs:171-181`).

**Last-success age goes on `/health/deep`, not into Prometheus.** The
registry had counters and histograms and no gauges when this was written, and
inventing one for an age would have been a substrate change to carry a number
that `/health/deep` is already the right surface for. The registry *has*
gauges now (`Metrics::set`), added for `ordo_supervisor_leader` and K3's key
counts, and the decision is unchanged for a different reason: an age is
meaningless in a scrape that arrives between two runs, and the number an
operator wants during an incident is on the surface they are already reading.
`/health/deep` is where
`DrainHealth::failing_for` and `degraded` already live
(`serve.rs:328-338`). The new `workers` section mirrors that section exactly,
including the posture that **not running is a first-class value**: a worker
that was never enabled reports `enabled: false` with the reason, in the same
shape `security_stream` reports `wired: false` (`serve.rs:319-327`).

### 3.4 The worker set

**As built**, at `crates/ordo-host/src/workers.rs`, registered by
`runtime::start_supervisor`. Fourteen supervised workers, one external, one
declared slot. The design's table had nine rows and three slots; the
differences are listed under the table.

| Worker | Cadence | Leader? | Enabled when | Calls |
|---|---|---|---|---|
| `triggers.drain` | `Tick(1s)` | no | the program declares a listener | `drain_listener` (`triggers.rs:726`) |
| `triggers.batch` | `Tick(1s)` | no | ditto | `fire_due` (`triggers.rs:410`) |
| `triggers.runner` | `Tick(250ms)` | no | **a `RunHandler` is registered** (§3.6) | `claim_runs` / `finish_run` / `fail_run` |
| `triggers.clock` | `Tick(1s)` | **yes** | the program declares a clock routine | `fire_clock` (`triggers.rs:805`) |
| `commandlog.checkpoint` | `Tick(5s)` | yes | `ORDO_CHECKPOINT_KEY` | `claim_checkpoint_tick` + `checkpoint_due` |
| `commandlog.attest` | `Tick(5s)` | yes | `ORDO_CHECKPOINT_KEY` | `claim_attest_tick` + `attest_due` |
| `attest.deliver` | `Tick(cfg)` | no | `ORDO_CHECKPOINT_KEY` | the attestation outbox (`attestdrain.rs`) |
| `secevent.checkpoint` | `Tick(60s)` | yes | `ORDO_SECURITY_CHECKPOINT_KEY`, and the drain holds a signer | `claim_checkpoint_tick` + `checkpoint_scope` |
| `secevent.retain` | `Tick(60s)` | yes | `ORDO_SECURITY_CHECKPOINT_KEY`, and the drain holds a retainer | `claim_retention_tick` + `retain_scope` |
| `commandlog.prune` | `Schedule{"commandlog.prune", 1h}` | yes | **`ORDO_WORKER_PRUNE=1`, default off**, plus `ORDO_CHECKPOINT_KEY_ID` and `ORDO_CHECKPOINT_PUBKEY` | `prune::prune` (`prune.rs:812`) |
| `keys.reseal` | `Tick(pause)` | yes | always; idles on one indexed read until `ordo keys rotate` enqueues a pass | K1's re-seal pass (`reseal.rs`) |
| `usage.flush` | `Tick(ORDO_USAGE_FLUSH)` | no | `ORDO_USAGE_LEDGER` | M1's per-replica ledger flush; **has an `on_shutdown`** |
| `usage.rollup` | `Schedule{"usage.rollup", ORDO_USAGE_ROLLUP}` | yes | `ORDO_USAGE_LEDGER` | M1's rollup, over every database `rt.spaces` reaches |
| `provisioning.reconciler` | `Tick(60s)` | yes | always | `tenants::time_out_stale` (11-15) |
| `secevent.drain` | its own | no | always | `SecurityDrain` (`secdrain.rs:255`), **an `External`, not a `Worker`** |
| `blob.gc` | — | — | never | a declared slot: `blobgc::sweep` is built and floored, and its cadence is an operator decision nobody has made |

Six differences from the design, each with its reason:

- **`secevent.drain` is an `External`, not a `Worker`.** The design said R3
  would "wrap it in the `Worker` shape"; the build did not, and the reasoning
  is in `corpus-rulings.md` under "The security drain stays outside the
  supervisor" (2026-09-18). What a supervised worker hands to the supervisor
  is exactly *when it stops* and *when it flushes*, and those two are the
  drain's own guarantees: it has a bounded outbox and a
  `shutdown() -> Result<usize>` that reports what it wrote. So it appears in
  the census with `supervised: false` beside its own health, which keeps
  obligation 8 (it is named) without moving guarantees that were already
  correct. It is in the **startup log's** census too, as of 2026-09-26: the
  log's claim is "these are the loops in this process", and the one loop that
  was running and unsupervised was the one the log did not mention.
- **`commandlog.checkpoint`, `commandlog.attest`, `secevent.retain` and
  `secevent.checkpoint` are `Cadence::Tick`, not `Cadence::Schedule`.**
  Obligation 2 is intact and it is discharged one level down: each of those
  steps *is* a schedule claim (`claim_checkpoint_tick`, `claim_attest_tick`,
  `claim_retention_tick`), carrying its own name and its own period from the
  policy the operator set. Putting the period in the `Cadence` as well would
  have been two places to write one number. `Cadence::Schedule` is therefore
  used by the two workers whose step is *not* itself a claim,
  `commandlog.prune` and `usage.rollup`.
- **`keys.reseal` and `usage.rollup` are real workers, not slots.** K1 and M1
  landed and filled them.
- **`attest.deliver`, `usage.flush` and `provisioning.reconciler` are
  workers the design did not list.** Each rides this supervisor rather than
  growing a second one, which is §3.1's whole point, and each arrived with
  the increment that needed it.
- **`usage.flush` is the only worker with an `on_shutdown`.** Obligation 4
  exists for exactly M1's partial period, and M1 is what uses it.
- **`blob.gc` is a slot that is always named as disabled.** A named
  not-running worker is the point of obligation 8, and a sweep whose cadence
  nobody has chosen is more honestly a DISABLED line than an invented
  default.

**`secevent.retain` and `secevent.checkpoint` are one artifact's storage and
its evidence, and they are
deliberately two rows** (7d, 2026-09-26). `secevent.retain` reclaims storage
on a storage cadence; `secevent.checkpoint` signs the head on an evidence
cadence, because how often a marker is signed bounds how long entries can sit
at the end of a stream where nothing signed knows they exist. They were one
for a while — retention's step 1 did the signing — and the consequences were
that a six-hour storage sweep set the detection window, that turning retention
off silently turned truncation detection off, and that the signature went on a
head nothing had verified. The last one let a prune delete the evidence of a
tamper and leave a stream verifying green under the deployment's own
signature. `cadence.rs`'s module doc already argues this for the command
chain's `checkpoint` and `attest`; this is the same rule on the other
artifact.

Five more of these rows carry a decision rather than a transcription:

**`triggers.drain` needs no leadership, and the module doc is why.** Two
replicas draining the same listener both read forward from the same cursor
and both accumulate the same events into `$triggerbatches`. That is harmless,
and `drain_listener`'s own doc has already argued it for the crash case:
*"a crash mid-drain replays the window and re-accumulates — at-least-once,
exactly as promised, and harmless because row ids are a set"*
(`triggers.rs:719-724`). Two replicas are the same situation as one replica
crashing, so the same argument covers it. Adding leadership here would buy
nothing and would make a leader's death a delivery stall.

**`triggers.batch` is separate from `triggers.drain`, and that is not
tidiness.** `fire_due` closes a debounce window, and a window closes on the
*clock*: `batch_due` returns true when `now - last_at >= window`
(`triggers.rs:382-394`). A batch whose last event arrived five minutes ago
with no writes since must still fire. A drain-triggered-only design would
leave every debounced batch of a quiet listener open forever, which is the
failure that looks exactly like "the trigger did not match".

**`triggers.clock` is the only trigger worker that takes leadership**, and it
takes it as an optimisation rather than for correctness. `fire_clock` calls
`claim_schedule` per routine, and `claim_schedule` is fires-once by
`FOR UPDATE SKIP LOCKED` plus the committed `next_due` advance — the doc says
so in as many words (`jobs.rs:130-136`, and `fire_clock`'s own note at
`:798-804`: *"Leadership is the caller's business … `claim_schedule` is what
makes firing exactly-once. §7 wants both and they are not the same
property"*). Leadership keeps a hundred replicas from all trying at once. A
supervisor whose leadership is broken is wasteful, not wrong.

**`commandlog.prune` is off by default, and stays an operator act.**
`deploying.md` §8 and `howto-operating.md` both document `ordo prune
--dry-run` before `ordo prune`, and pruning is the one worker whose failure
mode is destructive rather than delayed. It is *safe* — `prune` refuses
anything the floor does not permit and returns a typed `PruneRefusal`
(`prune.rs:755-804`) — and it is heavyweight, taking the append lock. So the
slot exists, the cadence is a named schedule, and enabling it is one
deliberate variable. A deployment that never sets it loses nothing except
automation.

**`secevent.drain` is adopted, not rewritten.** It already has health, a
bounded outbox, a tick, a retention sub-cadence and a `shutdown()` that
reports what it flushed. R3 wraps it in the `Worker` shape so it appears in
the census and the metrics with everything else, and changes none of its
behaviour. It is the model the other eight are built to match, which is why
§0.7 called it the exception that proves the shape.

**Configuration.** `ORDO_WORKERS` is an optional comma-separated allowlist
restricting the set (`ORDO_WORKERS=triggers.drain,triggers.batch` on a
dedicated pod); unset means every worker whose enabling condition holds.
`ORDO_WORKER_PRUNE=1` is the one opt-in. Every worker not running gets a
`DISABLED` line naming its reason (§1.6) — obligation 8.

Per-worker tick overrides are `ORDO_WORKER_<NAME>_TICK_MS`, where `<NAME>` is
the worker's name uppercased with `.` written `_`, so `triggers.drain` is
`ORDO_WORKER_TRIGGERS_DRAIN_TICK_MS`. All defaulted, and each running
worker's effective cadence is printed beside its name in the startup log.
Three properties of the override, all built and tested
(`tests/supervisor.rs`, `a_tick_override_moves_one_workers_cadence_and_leaves_the_rest`
and `an_unreadable_tick_override_refuses_rather_than_defaulting`):

- **It is resolved in one place**, `supervisor::resolve_tick`, rather than in
  fourteen `cadence()` implementations, so a worker cannot forget to honour
  it and the trait in §3.2 does not change.
- **It moves the tick and never a `Cadence::Schedule` period.** The period is
  what `claim_schedule` writes into `$schedules.next_due`, and obligation 2
  is that the worker owns it. On a scheduled worker the override changes only
  how often this replica asks whether the schedule is due.
- **An unreadable value, or a zero, is a boot refusal** rather than a silent
  fall back to the default. A variable an operator set and the process
  ignored is the configuration form of a worker that is not running and says
  nothing, which is the thing obligation 8 exists to prevent.

### 3.5 Leadership, on a connection of its own

§0.8 is the finding; this is the fix.

As built, at `crates/ordo-exec/src/jobs.rs:396`. The realm is a `&str` rather
than a pre-hashed `i64`, so the caller names the space and `realm_key` derives
the advisory key, and the error is a `String` rather than a `PoolError`
because `acquire` dials its own connection and never goes through the pool:

```rust
pub struct Leadership { _client: tokio_postgres::Client }   // dropping it releases the lock
impl Leadership {
    pub async fn acquire(cfg: &PoolConfig, realm: &str) -> Result<Option<Leadership>, String>;
}
```

A **dedicated connection**, dialed by the supervisor and never returned to
the pool — precisely the shape `jobs::listen` already uses, and for a related
reason. `listen`'s doc explains that `LISTEN` is session state and *"a pooled
connection goes back to the idle list the moment a query finishes"*
(`jobs.rs:262-276`). An advisory lock taken with `pg_try_advisory_lock` is
session state in exactly the same sense, and `TxConn::commit` returns its
client to the idle list (`crates/ordo-exec/src/lib.rs:525-532`). So a leader
elected through a pooled `TxConn` leaves the lock on a connection that goes
back to serving unrelated queries, holds it until that connection dies, and —
because advisory locks are re-entrant per session — re-elects successfully or
unsuccessfully depending on which connection it happens to borrow next.

**Ruled (2026-09-08):** *a session-scoped Postgres lock is held on a
connection the pool does not own.* Recorded in `corpus-rulings.md`.
`TxConn::try_lead` carries the doc note (added 2026-09-26, `jobs.rs:206-224`)
pointing at `Leadership` and saying that calling it on a pooled connection
leaks the lock; it is not deleted, because
`crates/ordo-exec/tests/jobs.rs`'s
`leadership_bounds_who_tries_and_the_claim_delivers_once` tests the primitive
and the primitive is correct.

The realm is derived from the deployment's space, the way
`commandlog::space_lock_key` and `secevent::scope_lock_key`
(`crates/ordo-exec/src/secevent.rs:124`) already derive advisory keys from
names — so two deployments sharing one Postgres cluster do not contend for
one another's leadership. Re-acquisition is attempted on a slow tick
(default 5s); a follower that becomes leader starts its leader-only workers,
and a leader that loses the connection stops them. `Leadership` is dropped
explicitly at §1.5 step 5, so a rolling restart hands leadership over in
milliseconds rather than at TCP timeout.

**A leader asks a different question from a follower, and this is not
symmetry that was missed.** A leader cannot learn it has lost the lock by
trying to take it again: advisory locks are re-entrant per session, so
re-acquiring on the session that already holds it always succeeds. The only
way a session lock is lost is that the session ended. So `Overseer::contend`
has a follower ask "can I take the lock" and a leader ask "is my session
still there", both bounded by the tick, and both answers write the per-worker
`leader` flag that the loops read between units. That is what makes a follower
which wins start its leader-only workers on their next pass, and a leader
whose connection died stop them before their next unit.

Two things the build added around it. `Supervisor::shutdown` releases
leadership by taking the connection out of its mutex rather than by being
dropped, which is what made the release survive the bug in §7's first entry:
a shutdown that needs sole ownership of the supervisor does nothing at all
while `/health/deep` holds a handle. And leadership is contended **only if
some registered worker wants it** (`LeaderState::wanted`), so a deployment
running nothing leader-only does not hold a lock that a replacement replica
then waits on.

Tested by `leadership_fails_over_between_two_supervisors`
(`tests/supervisor.rs`): two supervisors on one database, one leader, takeover
after the leader shuts down, and a lock session killed with
`pg_terminate_backend` noticed and retaken.

### 3.6 The run handler, and what "end-to-end" can honestly mean

§0.7: there is no routine *body* executor anywhere on a production path.
`run_routine` exists only in `crates/ordo-exec/tests/routine_provenance.rs:73`;
the declaration forms are checklist 12a; `Actor::Routine` has no producer at
all (`roadmap-checklist.md:3800-3808`), so two of §7's seven event kinds
cannot fire on the real write path yet.

R3 therefore ships the loop and the **seam**, and says so:

```rust
pub trait RunHandler: Send + Sync + 'static {
    fn handle<'a>(&'a self, run: &'a Run, ctx: &'a WorkerCtx)
        -> BoxFuture<'a, Result<(), String>>;
}
```

`Run::context(at)` already builds the `$trigger` a body would receive
(`triggers.rs:507-518`), and `triggers::reachable` already resolves
`$trigger.ids` into the row bound R-G20 requires (`:867`). The handler is the
one thing missing, and it is 12a's.

**With no handler registered, `triggers.runner` does not start**, and the
startup log says `DISABLED triggers.runner — no routine executor is
registered (checklist 12a)`. That is §1.6's rule applied to the case it
matters most for: a runner that started, claimed runs and dead-lettered every
one of them would look like a working trigger system destroying work.

Disposition, all of it already built: success is `finish_run`; failure is
`fail_run`, which retries with `backoff_secs` until `MAX_ATTEMPTS` and then
dead-letters (`triggers.rs:527-537`, `:607-639`). Dead letters are *"an
operator alert, which loses nothing"* (`triggers.rs:46`), and R3 gives that
alert a surface: `ordo_trigger_deadletters_total{listener}`, and the most
recent entries from `dead_letters(pool, space)` (`triggers.rs:652`) on
`/health/deep`. Paging remains the deployment's, the same posture
`alerts_route` already takes — *"It does not page anyone, it does not
aggregate across replicas, and it forgets everything on restart"*
(`serve.rs:370-381`).

So the honest reading of R3's done-when: **end-to-end means write → command
log → `drain_listener` → `$triggerbatches` → `fire_due` → `$triggerruns` →
`claim_runs` → a registered handler, inside a running binary.** Every arrow
is production code. The handler in the test records what it received; the
handler in production is 12a. Nothing in the loop is stubbed, and the one
thing that is missing is named rather than faked.

### 3.7 The `$logconsumers` bootstrap, fixed where it belongs

§0.9. `$logconsumers` is created by `prune::ensure_tables`, which only `ordo
prune` calls, so a deployment that starts a listener before anyone has ever
pruned finds no cursor table. `$attestations` is in the same position for a
different reason — its own doc calls the separation incidental
(`cadence.rs:159-163`).

**The fix goes in `Supervisor::start`, not in `ensure_platform_tables`**, and
the checklist already made this trade deliberately
(`roadmap-checklist.md:3831-3837`): *"the trigger tables are in
`ensure_platform_tables` because accumulation rides the write transaction and
must exist before the first write, while a cursor is registered by whoever
starts the listener."* The supervisor **is** whoever starts the listener.
Moving `$logconsumers` into `ensure_platform_tables` would create it in every
process that opens a pool — the playground, every conformance suite, every
`ordo` operator command — for a table only listeners and the pruner use, and
would quietly reverse a decision somebody wrote down.

So step 4 of §1.2 is `prune::ensure_tables(&pool)` and
`cadence::ensure_tables(&pool)`, in that order, with their existing
retry-once shape, before any worker registers. `ordo prune` keeps its own
call; both are idempotent, and two callers of an idempotent DDL guard is the
correct number when two independent processes both need the table.

As built this is `supervisor::bootstrap`, called first thing in
`Supervisor::start`, and it creates a **third** table on the same argument:
`reseal::ensure_tables` for K1's pass table, written by `ordo keys rotate` and
by the worker and read by both. Whoever starts the worker creates it, for the
same reason `$logconsumers` is not in `ensure_platform_tables`.

**A consequence the fix exposes, and it must not be fixed the obvious way.**
`register_listener` registers a `ConsumerKind::Listener`, which is
**non-evictable** — because *"evicting one and resuming it at the current
floor would silently turn at-least-once into zero-times"*
(`triggers.rs:679-683`). So every listener the supervisor registers pins the
pruning floor forever, and a deploy that renames or deletes a listener leaves
an immortal cursor pinning a log nobody will ever read.

The tempting fix is for the supervisor to deregister, on swap, every cursor
whose listener is no longer in the loaded program. **That is wrong, twice.**
During a rolling deploy replica B on v2 would deregister a listener replica A
is still serving on v1, silently resetting its at-least-once guarantee. And
the module doc has already ruled the question: *"a listener that should stop
pinning the floor is deregistered, **explicitly and attributably** —
`prune::deregister_consumer`, which is already the only way a non-evictable
kind leaves the calculation"* (`triggers.rs:45-49`).

So R3 **reports and does not act**: orphaned listener cursors — registered,
non-evictable, and not present in the live program — are listed on
`/health/deep` with their positions and their age, and `ordo prune`'s
existing floor report already names the holder
(`prune::FloorHolder`, `prune.rs:378-443`). Removing one is an operator act
with an `--as`. `[proposed]`: a small `ordo triggers deregister <listener>
--as <who>` beside it, so the operator act has a spelling that does not
require reading `prune.rs`.

### 3.8 What R3 does not do

- **No routine executor** (§3.6, checklist 12a).
- **No rate caps.** §7 lists them among the modifiers and gives no spelling
  anywhere — *"the least-specified thing in the section"*
  (`roadmap-checklist.md:3813-3815`). A supervisor is not the place to invent
  one.
- **No second process, no second scheduler, no leader election beyond
  `claim_schedule` + `try_lead`.** §5.
- **No cross-space fan-out.** One deployment, one space (§1.8). K1's
  per-space pass gets its space from `WorkerCtx`; the loop over spaces
  arrives with the space directory.

**Done when** a write issued through the door of a running `ordo-host` binary
reaches a registered `RunHandler` with the right `$trigger` — id set,
event kinds, batch id — with nothing between them stubbed: the command-log
append, `drain_listener`, `$triggerbatches`, `fire_due`, `$triggerruns` and
`claim_runs` are all the production functions, and the test asserts the run
was `finish_run`ed rather than dead-lettered.
And when a worker that panics on its first unit is restarted with the
platform's own backoff, is counted in `ordo_worker_restarts_total{reason="panic"}`,
is quarantined after ten consecutive failures with `enabled: false` and a
reason on `/health/deep` — while the door answers `/health/live` 200
throughout and serves every request unaffected.

**Done**, and each clause names the test that holds it
(`crates/ordo-host/tests/supervisor.rs` unless said otherwise):

| Clause | Test |
|---|---|
| write → handler, `finish_run`ed, nothing stubbed | `a_write_through_the_door_reaches_a_run_handler` |
| no handler ⇒ the runner does not start, and the census says why | `without_a_run_handler_the_runner_does_not_start` |
| panic caught, restarted on `backoff_secs`, counted as `reason="panic"`, and the door's metrics registry still works | `a_panicking_worker_is_restarted_and_does_not_poison_the_door` |
| quarantine after ten, `enabled: false` with a reason, `/health/live` 200 and an op succeeding throughout, over a real socket | `a_quarantined_worker_is_disabled_on_health_deep_while_the_door_stays_live` |
| leadership fails over, and a killed lock session is noticed | `leadership_fails_over_between_two_supervisors` |
| a named schedule fires once per period | `secevent_retain_claims_its_own_schedule_once_per_period`, `secevent_checkpoint_claims_its_own_schedule_once_per_period` |
| orphaned listener cursors are reported and never acted on | `an_orphaned_listener_cursor_is_reported_and_left_alone` |
| a tick override moves one worker and no other | `a_tick_override_moves_one_workers_cadence_and_leaves_the_rest` |
| an unreadable override refuses instead of defaulting | `an_unreadable_tick_override_refuses_rather_than_defaulting` |
| `on_shutdown` runs, between "workers cancelled" and "drain flushed", in the **spawned binary** | `tests/runtime.rs`, `the_binary_boots_serves_and_drains_on_sigterm` |
| every worker is in the census, running or DISABLED with a reason | same test |
| `on_shutdown` closes M1's partial period | `tests/usage.rs` |

---

## 4 · R4 — CI wiring

Four workflow files exist and none of them runs on anything but a button:
`ci-proposed.yml`, `images.yml` and `release.yml` are all
`workflow_dispatch`-only by construction, and the live `ci.yml` is untouched.
`plan-ci-images.md` designed all three and this increment flips them live —
plus the three additions R1–R3 make possible and one that R1 makes
*necessary*.

### 4a — `Cargo.lock`, first, because everything else keys on it

`plan-ci-images.md` §1.2 makes the case (reproducibility, cache correctness,
cargo-chef) and §3.0's 0b is the item. One detail it does not mention, and it
matters at the moment somebody runs `git rm --cached`: `.gitignore`'s bare
`Cargo.lock` line matches **at any depth**, so removing it also un-ignores
`crates/ordo-wasm/Cargo.lock`. That crate is excluded from the workspace and
built on its own by `scripts/build-wasm.sh` (root `Cargo.toml:22-23`), and it
produces a *published* artifact — `demo.html`. So commit both locks; an
excluded crate that ships bytes has the same reproducibility argument as the
workspace that ships images.

Then `--locked` on every `cargo` invocation in the Dockerfiles, in
`ci-proposed.yml` and in `release.yml`, so a drifting dependency fails a build
rather than entering an image quietly.

### 4b — adopt `ci-proposed.yml`, delete `ci.yml`, in one commit

`plan-ci-images.md` §2 is the design and its five changes stand unmodified.
Two additions, both forced by §0.4.

**`ORDO_PG_PASSWORD: ordo` in the job environment.** The live workflow sets
`PGPASSWORD: ordo` (`ci.yml:37`), which reaches `ordo-conformance`'s `psql`
shell-out and reaches **nothing else**: `tokio-postgres` has no `env::var`
anywhere in its `Config` (verified against `tokio-postgres-0.7.18`), and
`PoolConfig::pg()` never calls `.password(…)`. Once R1 adds the field the
variable has to be set, and it should be set in the same commit that adds it.

**A pool-tier execution proof, beside the psql-tier one.**
`plan-ci-images.md` §2.3 fixes the executed-case assertion so that a green run
cannot be produced without a reachable server — but the case it names, C001,
reaches Postgres through `psql`. The **deployable** reaches Postgres through
`Pool`, and no assertion covers that path. The same argument
`corpus.md` makes about a suite that never reached the database applies one
layer over: a green run that never opened a pooled connection proves nothing
about the thing that ships.

```
cargo test -p ordo-host --test deployment \
  the_deployment_surface_is_mounted -- --exact --nocapture 2>&1 | tee /tmp/pool.log
grep -qE '^test the_deployment_surface_is_mounted \.\.\. ok$' /tmp/pool.log
```

That test calls `Pool::connect` (`tests/deployment.rs:219`), so its pass line
cannot be produced without an *authenticated* pooled connection.

It also answers a question this plan cannot answer by reading: today, either
the pool-backed suites are failing on the runner, or the service container is
accepting host connections without a password. The mechanism above says they
cannot both be false. Making that assertion explicit is how the answer stops
depending on nobody looking. **Done when** it has been observed to fail with
`ORDO_PG_PASSWORD` removed.

### 4c — the host image job stops skipping, and its smoke proves R1–R3

`images.yml`'s host job already probes for `crates/ordo-host/src/main.rs`
and emits a notice rather than a failure (`:204-212`; `:390-398` is the
playground's twin). R1 makes the probe
pass, and the existing three-way `/health/deep` assertion — 403 pass, 404 no
router, 200 **the gate is missing** (`images.yml:250-274`) — is the acceptance
test for R1's router mount, unchanged.

Four additions, one per increment:

1. **`plan-ci-images.md` §3.3's "Phase 2 `[open]`" becomes reachable.** That
   document says: *"once the smoke can run `ordo apply` against the service
   database first, tighten `/health/ready` to 200, which then proves the shape
   gate ran **inside the image**. That is the strongest available boot proof
   and it is worth building."* R1's `load_active` path is what makes it
   buildable: the smoke runs the `ordo-cli` image's `ordo apply` against the
   service Postgres, restarts the host container, and asserts `/health/ready`
   is **200** rather than 200-or-503. A gate that ran and passed inside the
   image is the one thing no other assertion in the pipeline covers.
2. **R2's refusal is a smoke gate, not only a unit test.** Run the image with
   `ORDO_KEY_PROVIDER=test` and no `ORDO_DEV_UNSAFE_TEST_KEYS`; assert a
   **non-zero exit** and a log line naming the provider. This is
   `plan-ci-images.md` §3.0's 0a done-when, executed rather than asserted, and
   it is worth doing at the image layer because the failure it guards against
   — a deployment shipping under the repository's constant key — is a
   property of the *image* somebody deploys.
3. **The startup log is asserted to name at least one disabled feature.**
   `grep -q '^  DISABLED '` on the container's logs. The bare image has no
   `ORDO_CHECKPOINT_KEY`, so three lines are expected; the assertion is
   cheap and it catches the regression that matters — a binary that quietly
   enables everything, or one whose census stops printing.
4. **R3's delivery loop, without needing `psql` in the image.** Issue a write
   through the door, then poll `/health/deep` until the `workers` section
   shows `triggers.drain` and `triggers.batch` with `units > 0`. No client
   program is added to the distroless base (which `plan-ci-images.md` §3.1
   and §5's `HEALTHCHECK` argument both protect), and the assertion is about
   the loop rather than about a log line.

**R3's real end-to-end test is not here.** A handler-to-`finish_run`
assertion belongs in `crates/ordo-host/tests/` against the service Postgres,
where it can register a `RunHandler` and read `$triggerruns` directly. The
image smoke proves the loop is *running in the image*; the integration test
proves it is *correct*. Different questions, different layers — the same
split `plan-ci-images.md` §5 makes when it refuses to run the conformance
suite inside a Docker build.

### 4d — `release.yml`, with one change

`plan-ci-images.md` §4's design is unchanged: images are not rebuilt, four
CLI targets on native runners, each smoke-tested against the market fixture,
checksums beside the tarballs, provenance and SBOM wired but not mandated,
cosign recommended against for now.

The one change R1 forces: **`ordo-host` joins the tarball matrix.** The first
instinct is to refuse — the deployable is an image, and a loose server binary
invites somebody to run it without the manifest that supplies its
environment. The instinct is wrong here, for two reasons that are settled
rather than aesthetic. `implementation.md:93-94` states the self-host shape as
*"Self-host = binary + Postgres (+ optional bucket)"*, and
`implementation.md:546-556` settles that self-host is first-class and not a
stub. And §1.1's postures are precisely what makes a loose binary safe to
hand out: an `ordo-host` with no environment **refuses to start and names
each missing variable**, which is a better first experience than a container
that starts and serves under a test key.

### 4e — sequencing, and what does not move

Increment 4b does not depend on R1–R3 and can land first; 4c waits on all
three. `images.yml`'s `cli` job runs today and is unaffected.

Nothing here adds CD. `plan-ci-images.md` §4.1's argument is unchanged and is
worth restating because a runtime plan is exactly where somebody would add
it: *"a pipeline that pushed an image and then activated a program would be
automating past the gate the platform exists to run."*

**Done when** a push to `main` produces one green Actions run that builds the
workspace, runs the three gates against a service Postgres with both
execution proofs green, builds three images, boots the host image against
that Postgres, applies the market fixture through the CLI image, and gets
`/health/ready` 200 — with the `ORDO_KEY_PROVIDER=test` run in the same job
exiting non-zero.
And when every gate added here has been observed to fail at least once
against a deliberately broken input, because — `plan-ci-images.md` §6 — a gate
nobody has seen fail is a gate nobody has tested.

---

## 5 · Ruled out, with reasons

**A separate worker binary or crate.** The strongest argument for one is
familiar: workers and the door have different failure profiles, and a worker
that leaks memory should not be able to page whoever owns the API. It is
still wrong here, and for reasons specific to this platform rather than
general:

- **The deployment story is one pod.** `implementation.md:160-162` pins the
  dependency floor — *"no Redis, no broker, no coordinator, no cron runner:
  Postgres is the only stateful dependency that must exist"* — and
  `:93-94` pins the shape: *"Single static `ordo` binary; role (api / worker
  / all-in-one) by config."* Note what that sentence rules and what it does
  not: it rules **one binary with a role switch**, and `ORDO_WORKERS` (§3.4)
  *is* that switch. A deployment that wants a worker pod runs the same image
  with `ORDO_WORKERS=triggers.drain,triggers.batch,triggers.runner` and no
  traffic routed to it. A second binary would be a second artifact, a second
  image, a second smoke test and a second place for the assembly in §1.2 to
  drift — to buy a separation the environment already buys.
- **Every worker needs the whole assembly anyway.** `triggers.runner` needs
  the loaded program to execute a routine body; `keys.reseal` needs the
  `KeyProvider` and the space; `usage.rollup` needs the metrics registry the
  door writes into. A worker binary would import `ordo-host` entire and then
  re-implement §1.2 minus fifteen lines.
- **The counting argument goes the other way.** M1's usage counters
  accumulate *in the door's process* (`plan-production-ops.md:625-631`), and
  its rollup drains them. In-process, the rollup reads memory. Split, it
  reads a second thing that has to be written down first — which is a stream,
  which is what §3.2 there rules out at length.

The one thing a split *would* buy — that a worker cannot take the door down —
is bought instead by §3.3, which is a smaller mechanism with a smaller
failure surface. And the reverse of that concern is also worth stating: the
supervisor exists partly so that "let the process crash and let the
orchestrator restart it" is *not* the policy, because a restart takes the
door with it, and the door is the thing with the availability target.

**Hot configuration reload.** No SIGHUP, no watched config file, no
re-reading the environment. Two arguments and the second is decisive:

- Almost nothing in §1.1 can change safely under a live process. The token
  key, the page key and the master key are all bound to material already
  issued or already sealed; changing the space is changing which tenant is
  served; changing the pool size means re-dialing. What is left — tick
  intervals and the alert threshold — is a restart's worth of value.
- **The program already hot-swaps, and it does so through a gate.** The
  deploy channel (`registry.rs:82-88`, §1.4) reloads the thing that actually
  changes often, and `Deployment::swap` takes a `Verified` so the swap cannot
  route around the shape gate. A second reload mechanism, for configuration,
  with no gate and no equivalent of `LoadRefusal`, would be the ungated twin
  of the one careful mechanism this system has. If configuration ever needs
  to move without a restart, it should move through the registry as data with
  a gate, not through a signal.

**Leader election beyond `claim_schedule` plus `try_lead`.** No raft, no etcd,
no Kubernetes `Lease`. The decisive fact is that **leadership is not
load-bearing for correctness here**: `claim_schedule`'s doc states it —
*"This is the fires-once property, and it does not come from leadership"*
(`jobs.rs:130-136`) — and `fire_clock` repeats it. `SKIP LOCKED` plus the
committed `next_due` advance is the correctness mechanism, and it is a
property of the database every replica already shares. A consensus system
would add a second stateful dependency, contradicting
`implementation.md:160-162`, to make a *wasteful* condition slightly less
wasteful. Kubernetes `Lease` is a smaller version of the same trade and fails
the same test, with the extra cost that it only works on Kubernetes and this
platform's self-host story is first-class.

What `try_lead` on a dedicated connection does buy is real and modest: a
hundred replicas do not all wake up and attempt a hundred no-op claims every
tick. That is the whole requirement.

**Auto-deregistering orphaned listener cursors.** §3.7. Ruled out by
`triggers.rs:45-49`, which already requires removal to be explicit and
attributable, and by the rolling-deploy race that would let a v2 replica
silently reset a v1 replica's at-least-once guarantee.

**Provisioning key custody at boot.** §2.5. Ruled out by
`custody.rs:617-620`'s own argument: *"a release for an unknown space is
refused rather than silently creating keys, so a typo cannot mint a tenant."*

**Escrow or key-splitting inside `ordo-crypto`.** §2.5. Ruled out by
`implementation.md:1069-1071` — *"No silent escrow, ever"* — and by
proportion: a threshold scheme added to a crate whose entire dependency list
is two crates, to solve a problem an envelope and a written practice already
solve.

**A logging or tracing framework, in this increment.** Deferred rather than
ruled out, and the distinction matters. `howto-operating.md:97-101` records
that the platform has no structured logs and no traces at all; adding one
*inside the entrypoint* would give the binary better observability than the
library it wraps, which is backwards, and would make the choice of framework
on behalf of every crate in a plan about deployment. It is a platform-wide
decision and it should be made as one. §7.

---

## 6 · Sequence and done-when

| | Increment | Depends on | Done when |
|---|---|---|---|
| **R2a** | custody and space become inputs to loading (`verify_candidate` injects; `Fixture::keys` set) | — | A program loaded through `verify_candidate` seals through the supplied provider: a test asserts `Arc::ptr_eq(loaded.fixture.keys, provided)` and that a value written through the door opens under the operator's custody and **not** under `TestProvider`'s constant. |
| **R2c** | `PgKeyStore` | R2a | `LocalKeyProvider` boots over Postgres, seals and opens across a process restart, and `drop_space` removes an SMK and every DEK under it in one transaction. `ordo revert`'s sealed-entity refusal loses its stated reason. |
| **R1** | the binary | R2a, R2c | `cargo build -p ordo-host --bin ordo-host` boots against a service Postgres; `/health/live` 200, `/health/deep` 403 unauthenticated, `/health/ready` 200-or-503; exits non-zero *before dialing* for each missing secret and for `ORDO_KEY_PROVIDER=test` without the dev flag; `SIGTERM` under load finishes in-flight requests and logs what the drain flushed. |
| **R2b** | sealed-master source + ceremony commands | R1 | Boots under `ORDO_KEY_PROVIDER=sealed-master` from a file `ordo keys seal-master` wrote, refuses a space `ordo keys init` has not created, and seals+opens a real value. A wrong passphrase is `KeyError::Seal` with no indication of which byte was wrong. |
| **R3** | the supervisor | R1 | A write through the door of a running binary reaches a registered `RunHandler` with the right `$trigger`, through production `drain_listener` → `fire_due` → `claim_runs`, and is `finish_run`ed. A worker that panics restarts with `backoff_secs`, is counted, quarantines after ten failures — and the door serves throughout. |
| **R4** | CI live | R1–R3 | One green `main` run: three gates with both execution proofs, three images, host image booted against the service Postgres, market fixture applied, `/health/ready` **200**, and the `test`-provider run exiting non-zero. Every new gate observed to fail once against a broken input. |

R2a is first and it is not negotiable: R1 cannot honestly claim to refuse
`TestProvider` while the write path still reaches one. 4a (`Cargo.lock`) and
4b (the test pipeline) depend on nothing and may land at any point.

**All six have landed.** R2a, R2c, R1 and R2b on 2026-09-07 and merged
2026-09-18; R3 reviewed and finished 2026-09-18, with K1 and M1 filling its
two slots afterwards; R4 on 2026-09-19. R3's done-when is broken out clause by
clause against its tests at the end of §3.8, and two wiring defects found on
2026-09-26 are the first two entries in §7.

---

## 7 · Findings recorded, not scheduled

The first two were found on 2026-09-26 and **fixed**, because both were R3's
own wiring. They are recorded because of what they have in common: each was a
supervisor that did nothing, silently, with no failing test and no log line,
which is the exact defect class R3 exists to close and the reason its tests
are written to count rather than to check presence.

- **`serve` never shut the supervisor down, on any deployment.** §1.5 step 3
  was written `if let Some(s) = Arc::into_inner(supervisor)`, and
  `PlatformState` holds a clone of that same `Arc` so `/health/deep` can
  answer the worker census, so `into_inner` returned `None` every single time.
  The workers were never cancelled, no `on_shutdown` ran, and leadership was
  released only when the process's socket closed rather than explicitly at
  step 5. M1's partial usage period is what went missing on every restart.
  Fixed by making `Supervisor::shutdown` take `&self`, which cannot be
  defeated by a handle somebody else holds; a `Vec<JoinHandle>` behind a lock
  is the whole cost. The lesson for the next surface like this: **a cleanup
  path that needs sole ownership of a shared handle is a cleanup path that
  does not run**, and it fails by omission, so only a test that asserts the
  *count* catches it. `tests/runtime.rs` now asserts the final-act count and
  its position relative to the drain flush.
- **`provisioning.reconciler` was registered inside the
  `listeners.is_empty()` else-branch**, so it ran only on a deployment that
  declared a trigger listener, and no deployment declares one until checklist
  12a. Its own comment said "unconditional". The census named it neither
  running nor DISABLED, which is the one state obligation 8 forbids, so the
  startup log of every test that spawned the binary had been printing the
  evidence since the worker was written two days earlier (`65435ab`,
  2026-09-25). A provision that died half way therefore stayed
  `provisioning` instead of turning `failed` and joining `tenants::troubled`,
  which is `bootstrap.md` §4.1's one detection query. Fixed by hoisting the
  registration out of a branch it had nothing to do with, and
  `tests/runtime.rs` now asserts that every worker it expects is in the
  running census by name.

- **The production write path runs `impl Fixture` from `ordo-conformance`.**
  `Executor::write_attempt` calls `fixture.run_write_on(&db, …)`
  (`crates/ordo-exec/src/lib.rs:928`), and the Stage-5 protocol's `impl`
  block is `impl Fixture` (`crates/ordo-conformance/src/write.rs:333`).
  `plan-ci-images.md` §1.6 recorded the dependency edge and classified it as
  *"weight rather than a hazard"* on the grounds that *"nothing on the serving
  path calls `Pg`."* That reading was about `Pg`, and it is true about `Pg`.
  It is not true about the crate: every customer write in a deployed Ordo
  executes a method on the conformance harness's fixture type, which is also
  why §0.3's custody hole existed and was invisible. Not scheduled here — it
  is a layering change to `ordo-exec` and `ordo-conformance`, not a runtime
  change — but the classification should move from "weight" to "the serving
  path lives in the test crate", because the next person to read §1.6 will
  otherwise conclude it is cosmetic.

- **A caught panic can poison a shared `std::sync::Mutex` and move a worker
  fault into the door.** `Metrics` holds `series`, `histograms`, `gauges` and
  `alerts` behind `std::sync::Mutex`, and every access used to be
  `.expect(...)`: a panic while holding one poisons it, and the next denial
  the door records panics too. **Mostly closed**, and the remainder is
  recorded precisely rather than described loosely. `metrics::unpoisoned`
  takes every lock ignoring poison, with the argument for why that is right
  for this state specifically (counters and histogram buckets have no
  invariant across the map beyond "these are numbers", so a lost increment
  beats losing every future one), and thirteen accesses go through it.
  **Three do not:** `Metrics::bins`, `evaluate_alerts` and `render_alerts`,
  the last two reached from `/security/alerts`. R3's own accounting is
  atomics, so the supervisor cannot be the thing that poisons anything, and
  the three sites are `ordo-host`'s to finish. The `unpoisoned` doc claimed
  all of them had moved; corrected 2026-09-26.
  `FixtureExecutor`'s `Arc<Mutex<WriteLog>>` — *"Shared across requests"*
  (`crates/ordo-host/src/lib.rs:411-415`) — is the same class, on the write
  path, with a second consequence: it is a process-wide lock on every write,
  which is a contention axis nobody has measured, and §6g's concurrency test
  already turned up a deadlock in it. That one is unchanged and is
  `ordo-host`'s.

- **`howto-operating.md:103` names an environment variable that does not
  exist.** `ORDO_DENIAL_VOLUME_THRESHOLD_PER_SPACE` appears nowhere in
  `crates/`; the code reads `ORDO_DENY_ALERT_THRESHOLD`
  (`crates/ordo-secevent/src/alert.rs:89`). An operator who sets the
  documented name gets the built-in default and a surface that reports itself
  unconfigured. Fixed in R1's docs commit; recorded here because it is the
  kind of defect that only surfaces during an incident.

- **Three documents disagree about whether 8a–8d landed.**
  `roadmap-checklist.md:931-950` marks all four `[x]`;
  `deploying.md:77-86` still says `TestProvider` is the only provider and
  there is no seam; `roadmap-checklist.md:4502-4506` calls the KeyProvider,
  page key and token key *"all three unbuilt, items 8a–8d"*. §2.0 reconciles
  them against the code. `deploying.md` §4's status note wants rewriting when
  R2 lands, and the §15 parenthetical wants deleting.

- **`implementation.md:93-94`'s "single static `ordo` binary" and the two
  shipped binaries.** The workspace has `ordo` (operator tool) and, after R1,
  `ordo-host` (server); `plan-ci-images.md` §3.1 builds them as separate
  images with different bases and different smoke tests. The *role* half of
  that sentence is honoured (§3.4, `ORDO_WORKERS`); the *single binary* half
  is not, and the split is right — an operator tool and a serving process
  have different blast radii, different bases and different reasons to exist.
  The sentence should be amended to say so rather than left to contradict the
  tree.

- **The access engine's membership table is boot-time, not live** (§1.3), so
  a membership change takes effect on the next swap or restart —
  weaker than `IdentityAccess`'s doc reads (`lib.rs:926-929`). A refresh on
  the invalidation bus is the obvious fix and belongs to `ordo-identity`.

- **No TLS to Postgres.** `dial` connects with `NoTls`
  (`crates/ordo-exec/src/lib.rs:269`). Every managed Postgres offers TLS and
  most default to requiring it, so this is a real limit on where a deployment
  can run. `plan-ci-images.md` §1.5 already names the constraint on the fix:
  choose a **rustls**-backed feature, never `native-tls`, or the pure-Rust
  dependency graph — and with it the distroless base and the free
  cross-target release matrix — stops being true.

- **No inbound TLS either.** The binary serves plain HTTP and expects a proxy
  or an ingress to terminate. That is the right default and it is not written
  down anywhere; R1's docs commit says it, because a deployment that puts
  `ORDO_HOST_ADDR=0.0.0.0:8080` on a public interface with no proxy is
  serving bearer tokens in the clear.

- **`TxConn::try_lead` leaks its session lock into the pool** (§0.8). R3
  routes around it with a dedicated connection (`jobs::Leadership`); the
  primitive itself got a doc note rather than a change, on 2026-09-26, because
  `crates/ordo-exec/tests/jobs.rs`'s
  `leadership_bounds_who_tries_and_the_claim_delivers_once` tests it and the
  primitive is correct — it is the *caller* that has to own the connection.
  Nothing outside that test calls it.

- **`ORDO_APEX_DOMAIN` is required by two documents and read by no code**
  (§1.1). R1 reads it and defers the refusal to the increment that mounts the
  passkey endpoints. `licensing.md:237`, `security-logging-design.md:855` and
  `audit-board-coverage.md:133` should be reconciled in that increment, not
  before.

- **No structured logging anywhere in the platform** (`howto-operating.md:97-101`).
  R1 prints to stdout deliberately (§1.6). Choosing a framework is a
  platform-wide decision and this is not the plan that should make it.

---

## Related

- [`plan-ci-images.md`](plan-ci-images.md) — increment 0a, which R1 supersedes
  and expands; the image smoke assertions R4 flips live; §1.5's pure-Rust
  dependency argument, which §2.3 and §7 both protect.
- [`plan-production-ops.md`](plan-production-ops.md) — K1 and M1, which ride
  §3's supervisor; track K's re-seal pass, which needs §2.4's `PgKeyStore`.
- [`plan-dev-loop.md`](plan-dev-loop.md) D3 — the same assembly minus the
  watcher; §1.7 is the seam.
- [`plan-schema-evolution.md`](plan-schema-evolution.md) — why the binary does
  not run DDL, and what a deployment does when its database disagrees with
  its program.
- [`deploying.md`](deploying.md) §4, §7, §9 — the custody contract R2 finally
  meets, the swap the binary listens for, and the operating checklist that
  told operators to set three secrets no loader existed for.
- [`howto-custody.md`](howto-custody.md) — the sealed-field and rotation story
  §2.5's ceremony section extends.
- `corpus-rulings.md` — the two rulings recorded from this plan (2026-09-08).
