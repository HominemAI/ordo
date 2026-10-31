# Ordo CI and images — design & build plan

Companion to `corpus.md` (the assurance model this pipeline has to carry
into CI without weakening it), `deploying.md` §7 and §9 (what a deployment
is, and the artifact-hash identity discipline the image tags are made to
rhyme with), `scripts/conformance-pg.sh` (the cluster the harness expects),
and `roadmap-checklist.md` §15.

Markers: **[settled]** restated from a settled document · **[proposed]**
adopted here, pending owner review · **[open]** named and not answered.

Status: build plan. **Nothing in it is wired.** The three workflow files
this plan adds are `workflow_dispatch`-only, so they appear in the Actions
tab and run when somebody presses a button, and never otherwise. The live
`ci.yml` is untouched. Every claim about the current code was checked
against the code and carries its file.

---

## 0 · What this plan is answering

"Plan out a GHA pipeline for image building and testing."

Two halves, and they are less symmetric than they sound. **Testing** is
mostly already right: `.github/workflows/ci.yml` exists, it runs the three
gates against a real Postgres service container, and — unusually for a
first attempt — it already gets the three environment overrides this repo
needs (§2.1). It wants five changes, one of which is a genuine correctness
fix (§2.3).

**Images** is the half where the standard recipe does not apply, because
the thing the recipe is for does not exist yet. §1 is that finding, and it
is the most important section in this document.

---

## 1 · The starting state, verified

Every increment below is a delta on these facts. They were read, not
assumed.

### 1.1 There is no deployable binary

`crates/ordo-host` is **a library**. It has no `[[bin]]` in its
`Cargo.toml` and no `src/main.rs`. What it exposes is
`ordo_host::serve::platform_router(state) -> Router` and a `PlatformState`
assembled from an access checker, a `Deployment`, a pool, a `KeyProvider`,
a space and a metrics handle. Nothing in the workspace binds a socket to
it: the only two `main.rs` files in `crates/` are `ordo-cli/src/main.rs`
and `ordo-playground/src/main.rs`, and the only `TcpListener::bind` in the
tree is the playground's.

`node.router()`, which every host test calls, is a test helper. The router
is exercised through `tower::ServiceExt::oneshot` — in-process, no socket.

So the sentence "build an image for `ordo-host`, the deployable" describes
work that has not been done. A workflow that ran `cargo build -p ordo-host
--bin ordo-host` today fails with *no bin target named `ordo-host`*.

**This is the plan's first increment and everything about the host image
waits on it.** It is not a CI problem and it should not be solved inside a
Dockerfile — a `main.rs` written to make a container start is a `main.rs`
nobody reviewed as a deployment entrypoint.

The entrypoint is not a formality, because two orderings in `serve.rs` are
load-bearing and a hurried binary can route around both:

- `PlatformState::new` takes an `Arc<Deployment>`, and a `Deployment` can
  only be booted from a `Verified`, whose only constructor runs the shape
  gate. The type enforces "gate before surface". A binary that reaches for
  `Deployment::pinned` to get past a red gate at boot has disabled the
  check that `deploying.md` §7 promises runs on every swap.
- `ordo-crypto` ships exactly one `KeyProvider` — `TestProvider`, whose
  master key is **a constant in this repository** (`deploying.md` §4 says
  so in a status note). An image that boots with it and serves is an image
  that seals production data under a public key. The entrypoint must refuse
  to start with `TestProvider` unless something explicit says this is a
  test deployment.

### 1.2 There is no `Cargo.lock`

It is in `.gitignore`. For a workspace of libraries that is a defensible
choice; for a workspace that produces **binaries and images** it is not,
and it damages three separate things in this plan:

- **Reproducibility.** Two builds of the same commit can resolve different
  semver-compatible dependency versions and produce different bytes, hence
  different image digests. §4 makes the digest the image's identity; an
  identity that changes without the input changing is not one.
- **Cache correctness.** `Swatinem/rust-cache` keys on the manifests *and*
  the lockfile. With no lockfile the key is coarser: an upstream release
  changes what is compiled without changing the key.
- **cargo-chef.** The recipe still works, but the cooked dependency layer
  misses whenever any transitive dependency publishes, which is the layer
  the whole image build time rests on.

It also means CI can go red on a commit that touched nothing, which is the
kind of failure that teaches people to re-run rather than to read.

### 1.3 The harness's defaults are all laptop defaults

`ordo_conformance::Pg::default` (`crates/ordo-conformance/src/lib.rs:88`)
reads five environment variables, and **every default is wrong on a
runner**:

| var | default | why it is wrong in CI |
|---|---|---|
| `ORDO_PG_HOST` | `/tmp/ordo-pg` | A Unix **socket directory**, not a host. `conformance-pg.sh` starts its cluster with `-k $SOCKET -h ''` — TCP is off entirely. A GHA service container publishes TCP on localhost and no socket the job can see. |
| `ORDO_PG_PORT` | `55432` | The throwaway cluster's deliberately non-default port. |
| `ORDO_PG_USER` | `ordo` | Fine, but only if the service container's role is `ordo` **and is a superuser** — see below. |
| `ORDO_PG_DB` | `ordo_conformance` | Fine if the service creates it. |
| `ORDO_PSQL` | `/opt/homebrew/opt/postgresql@17/bin/psql` | A macOS Homebrew path. It exists on exactly one kind of machine. |

The last one is the one that catches people, and it is worth stating plainly
because it shapes both the CI job and the playground image: **the harness
executes SQL by running the `psql` binary**, not through a driver
(`ordo-conformance/src/lib.rs:19` says so in its own module docs — statements
reach Postgres through the `psql` process, and the separators are chosen so
its output needs no second parser). Anything that runs the corpus needs a
Postgres *client program* on `PATH`, not just a reachable server.

`Pg::named` creates databases on demand with `CREATE DATABASE` issued
against the `postgres` admin database. So the connecting role must be able
to create databases. Setting `POSTGRES_USER: ordo` on the service container
is what makes this work — the official image makes `POSTGRES_USER` the
superuser. An ordinary role would fail the first `Pg::named` call in every
suite at once, which reads like a harness bug and is a permissions bug.

**Per-suite isolation needs nothing extra from CI.** `corpus.md` explains
why each suite owns its databases (the fixture reset opens with `DROP SCHEMA
public CASCADE`, and two suites sharing a database tear down each other's
tables mid-run — diagnosed as a flake four separate times before it was
fixed). `Pg::named` handles that entirely inside the test process. One
service container serves every concurrent suite; there is no per-suite
service, no `--test-threads=1`, and no reason to add either. **Do not** add
`--test-threads=1` to "fix" a flake: it would hide the isolation property
rather than test it.

### 1.4 Some tests read the repository, not just the crate

`class_inventory.rs`, `goldens.rs` and the `ordo-cli` interface tests derive
a repo root from `CARGO_MANIFEST_DIR` and read `docs/*.md` — the inventory
tests parse `api-engine-design.md` and `composition-conformance.md` tables
directly. `cargo test` therefore requires the **docs tree**, not only
`crates/`. This is fine in CI (a checkout has everything) and it is one more
reason the conformance suite must not run inside an image build, where the
context is usually trimmed (§6.2).

### 1.5 The dependency graph is pure Rust, and that is good news

`ordo-crypto` is `chacha20poly1305` + `sha2`. `ordo-identity` is
`ed25519-dalek`, `p256`, `argon2`, `getrandom`. `ordo-exec` speaks the
Postgres wire protocol via `tokio-postgres` **with no TLS feature enabled**.
There is no `openssl`, no `native-tls`, no `bindgen`, no `cc` anywhere in
the manifests.

So the runtime image needs glibc and `libgcc_s` and nothing else — which
makes `gcr.io/distroless/cc-debian12` correct rather than aspirational, and
makes native cross-target release builds trivial. **[open]** If TLS to
Postgres is added later, choose a rustls-backed feature rather than
`native-tls`, specifically so this stays true.

### 1.6 `ordo-conformance` is inside the deployable's dependency graph

`crates/ordo-host/Cargo.toml` and `crates/ordo-exec/Cargo.toml` both list
`ordo-conformance` under `[dependencies]`, not `[dev-dependencies]`. The
test harness — its `psql` shell-out, `Fixture`, and the market and wide
fixtures embedded via `include_str!` — is compiled into the deployable.

Nothing on the serving path calls `Pg`, so this is weight rather than a
hazard. But it is weight the deployable should not carry, and "the shipped
server contains the conformance fixtures" is a sentence worth not having to
say. **[open]** — recorded here as a finding, not scheduled; the fix is a
layering change in `ordo-exec`, not a CI change.

### 1.7 The playground binds loopback

`crates/ordo-playground/src/main.rs` hardcodes `let addr =
"127.0.0.1:7878"`. Correct for `cargo run`, fatal in a container: the
process starts, the port opens, and nothing outside the container can
reach it. Increment 0c.

### 1.8 There is no `clients/` directory on this branch

Checklist 14b (`@ordo/react`) is unbuilt. The JS job is written and probes
for the directory, so it reports a notice and skips rather than failing for
an absent thing.

---

## 2 · Increment 1 — the test pipeline

**File:** `.github/workflows/ci-proposed.yml` **[proposed]**, unwired.

It is a successor to the live `ci.yml`, kept as a separate file rather than
an edit so the owner can read the two side by side. Adopting it means
uncommenting its `push`/`pull_request` block and deleting `ci.yml` **in the
same commit** — two workflows racing for one cache key and one service port
is worse than either.

### 2.1 What the live `ci.yml` already gets right

Stated because it deserves to be, and because a rewrite that lost any of it
would be a regression:

- `postgres:17`, matching `conformance-pg.sh`'s `postgresql@17`. The corpus
  asserts returned rows; a version mismatch moves row ordering and null
  sorting under it.
- All five `ORDO_PG_*`/`ORDO_PSQL` overrides, including the two that are
  genuinely non-obvious (§1.3).
- `POSTGRES_USER: ordo`, which is what makes `Pg::named` able to create
  databases.
- `psql` installed as a package, because the harness runs the binary.
- `ORDO_UPDATE_GOLDEN` absent, so goldens are compared rather than
  rewritten.
- The executed-case assertion exists at all, which is the discipline
  `corpus.md` asks for.

**What it got wrong, found 2026-09-15** by the first runs to reach every
crate, and fixed in both workflows:

- **Two readers of one password.** `PGPASSWORD` served `psql`, but the Rust
  pool reads `ORDO_PG_PASSWORD`, which was unset, so every pool connection
  was refused over TCP ("invalid configuration: password missing") while a
  local socket, which asks for none, passed. Tests that attach databases on
  the same server authenticate through `Installed::as_configured`.
- **Stopping at the first failing binary.** Without `--no-fail-fast`, that
  refusal in `ordo-cli` ended every run, so `ordo-exec`, `ordo-host` and the
  corpus had never run on CI.
- **The runner's disk.** A debug build of this workspace and the Postgres
  container outgrew the hosted runner's free space. Postgres panicked
  writing WAL ("No space left on device") and every later test met "the
  database system is in recovery mode". The workflows free unused
  preinstalled SDKs and build without debug info or incremental state.

### 2.2 The five changes

1. **`concurrency` with `cancel-in-progress`**, off on `main`. A superseded
   run holds a service container and a runner for a full workspace compile.
   `main` is excluded because a push to `main` is the run whose result gets
   recorded.
2. **`timeout-minutes: 40`.** The default is six hours, and the shape of a
   failure here is a `psql` waiting on a service container that never
   became healthy.
3. **`RUST_BACKTRACE: 1`.** The failure this pipeline exists to catch is an
   execution-tier panic several frames inside the harness.
4. **An explicit assertion that `ORDO_UPDATE_GOLDEN` is unset**, before the
   suite runs. Its absence is currently a property of nobody having set it.
   A run with it set is green by construction and proves nothing
   (`golden.rs` rewrites instead of comparing), so the hole is worth
   closing against a repository variable somebody adds later for a reason
   that made sense at the time.
5. **The executed-case assertion, fixed.** §2.3.

Plus two additions: the inventory tests by name (§2.4) and the JS job
(§2.5).

### 2.3 The executed-case assertion is currently weaker than it reads

`corpus.md`: *"A green run that never reached Postgres would prove nothing,
so CI asserts its own execution: it runs a single known case and greps for
the pass line, rather than trusting that the suite ran."*

The live `ci.yml` implements it as:

```
cargo test -p ordo-conformance c001 -- --nocapture | tee /tmp/c001.log
grep -q "test result: ok. 1 passed" /tmp/c001.log
```

Two defects, and they compose:

- **`c001` is a substring filter.** It matches
  `c001_projection_subset_over_a_composed_source` in `corpus.rs` **and**
  `gc001_to_gc004_one_op_four_principals_four_answers` in
  `guards_corpus.rs`. Two test binaries each report `1 passed`.
- **`test result: ok. 1 passed` says *some* single test passed, not
  *which*.** If the corpus case were renamed or deleted, the guards case
  alone keeps the grep green.

An execution proof that survives the disappearance of the thing it proves
is not a proof — which is `corpus.md`'s own argument about a case that
passes because it stopped checking something, arriving one layer out in the
CI script.

The fix names the test exactly, in its own binary, and greps for that
name's own ok-line:

```
cargo test -p ordo-conformance --test corpus \
  c001_projection_subset_over_a_composed_source -- --exact --nocapture \
  2>&1 | tee /tmp/c001.log
grep -qE '^test c001_projection_subset_over_a_composed_source \.\.\. ok$' /tmp/c001.log
grep -q 'test result: ok. 1 passed' /tmp/c001.log
```

`C001` is the right case to pick: it is an *execute*-tier case — it plans
SQL, runs it against the seeded fixture and asserts returned rows — so its
pass line cannot be produced without a reachable server. A compile-tier
case would prove nothing here.

**Done when:** the assertion fails on a runner with the Postgres service
removed, and fails if `c001_projection_subset_over_a_composed_source` is
renamed. Both are checkable by trying them.

### 2.4 The two-directional inventory, by name

`crates/ordo-api/tests/inventory_is_closed.rs` holds a closed pair over
`api-engine-design.md`'s table and `ErrorCode::ALL`:

- `every_class_the_engine_can_emit_is_in_the_document` — a class the wire
  can return that the document does not list is an undocumented refusal a
  caller cannot handle.
- `every_documented_class_is_one_the_engine_has` — a class the document
  lists that nothing raises is API surface someone writes a handler for and
  never reaches.

Both already run inside `cargo test --workspace`. They are **re-run as their
own named steps** so a red inventory says *which direction* broke in the job
list, without anyone opening a log. Everything is already compiled, so the
cost is a process spawn.

The third leg of the same triangle is added beside them:
`every_refusal_class_named_is_in_the_wire_inventory` in
`crates/ordo-cli/tests/interface.rs`, which joins the interface descriptor
against the document. Its own doc comment explains why the three close:
`ordo-plan` cannot depend on `ordo-api`, so the descriptor test reads the
document, `ordo-api`'s pair pins the document to the enum, and `ordo-host`'s
crossing test joins the descriptor to the enum directly.

**Done when:** deleting a row from `api-engine-design.md`'s table produces a
failure whose *step name* identifies the direction.

### 2.5 The JS job

A separate job, `continue-on-error: true`, no Postgres service, its own
timeout. Three reasons for the separation and none of them is tidiness: a
failing vitest must not be able to hide a green Rust run; neither should
wait on the other's service container; and 14b is unbuilt, so a hard
dependency would make the Rust pipeline's health contingent on work that
has not started.

It probes for `clients/` and emits a `::notice::` and skips when absent.
`pnpm install --frozen-lockfile`, `tsc --noEmit`, `vitest run`.

**Done when 14b lands:** flip `continue-on-error` to `false` in the same
change that makes the client a shipped artifact, and say so in its checklist
row.

### 2.6 Caching

`Swatinem/rust-cache@v2`, `shared-key: rust-gates`.

It keys on the runner, the rustc version, the job, and a hash of every
`Cargo.toml` and `Cargo.lock` it finds; it caches `~/.cargo` and the
*dependency* parts of `target/`, never first-party crates — a stale
first-party artifact is how a cache starts hiding a miscompile, and that is
worth more than the seconds it would save.

The `shared-key` is distinct from the image workflow's buildx scopes so a
Docker build and a test run cannot evict each other.

**The lockfile caveat from §1.2 applies here and is the reason 0b is in the
plan at all.**

---

## 3 · Increment 2 — the images

**Files:** `.github/workflows/images.yml`, `docker/Dockerfile.cli`,
`docker/Dockerfile.host` (since console step 10, the `host` target of
`docker/Dockerfile`), `docker/Dockerfile.playground` — all **[proposed]**
and unwired.

### 3.0 Increment 0 — what has to land first

| | | |
|---|---|---|
| **0a** | `crates/ordo-host/src/main.rs` and a `[[bin]] ordo-host` | §1.1. Must assemble `PlatformState` from a `Verified` deployment (never `Deployment::pinned` to get past a red gate), bind `0.0.0.0` from `ORDO_HOST_ADDR`, and **refuse to start under `TestProvider` without an explicit opt-in**. |
| **0b** | Commit `Cargo.lock`; remove it from `.gitignore` | §1.2. Then add `--locked` to every build in CI and in the Dockerfiles, so a drifting dependency fails the build rather than silently entering an image. |
| **0c** | `ORDO_PLAYGROUND_ADDR` in `ordo-playground/src/main.rs` | §1.7. Default to `127.0.0.1:7878` so `cargo run` is unchanged. |

`images.yml`'s `host` and `playground` jobs each **probe for their blocker
and skip with a notice**, so the workflow is honest about what is unbuilt
rather than red about it. The `cli` job runs today.

### 3.1 Three images

| image | what it is | base | buildable today |
|---|---|---|---|
| `ordo-host` | the deployable — the door, the wiring, the executor | `distroless/cc-debian12:nonroot` | no — 0a |
| `ordo-cli` | the operator tool, `ordo` | `distroless/cc-debian12:nonroot` | **yes** |
| `ordo-playground` | the browser surface | `debian:bookworm-slim` + `postgresql-client-17` | no — 0c |

**Why `distroless/cc` and not `base`:** Rust binaries need `libgcc_s` for
unwinding, which `distroless/base` does not carry. `cc` does. **Why not
static musl:** it would work (§1.5 — no C dependencies), and it is the
obvious next optimisation, but glibc is what the workspace is tested
against and a musl swap changes allocator behaviour under a threaded
workload. Not now; not ruled out.

**Non-root** on both distroless images comes from the `:nonroot` tag, uid
65532, restated as an explicit `USER 65532:65532` so a Kubernetes
`runAsUser` and the image agree without anyone reading the tag. The
playground image has no such tag and creates the user itself.

**Layer caching** is `cargo-chef`: `chef prepare` reduces the workspace to a
manifest-only skeleton, so the expensive dependency layer is keyed on the
manifests alone and survives a source edit. Without it, every source change
recompiles every dependency, because the whole tree is one `COPY`.

### 3.2 The playground gets its own image

The question was whether it rides in the host image. It should not, and the
reasons are mechanical:

1. **It does not use the host's serving path.** It builds its own axum
   router and executes SQL by shelling out to the **`psql` binary** through
   `ordo_conformance::Pg` — not through `ordo-exec`'s pool. So it needs a
   Postgres client program in its runtime, and the deployable does not.
   Merging them means putting `psql` and an apt-managed package feed into
   the deployable to serve a demo.
2. **It rebuilds three databases at boot.** `App::seed` drops and recreates
   `ordo_playground`, `ordo_playground_market` and `ordo_playground_wide`.
   A process that runs `DROP SCHEMA public CASCADE` on startup must not
   share an image, a pod, or a database with the deployable — and the
   playground's own module docs record that it was moved off the
   conformance databases for exactly this reason after reads raced a
   `cargo test` run.
3. **It pins the clock** to `Clock::CORPUS` and serves fixture data. It is a
   demonstration surface, and its blast radius should be visible in the
   deployment topology rather than only in a code comment.

Separate image, separate database, separate pod.

### 3.3 The smoke test

**An image that builds but cannot boot must not publish.** Every job builds
`linux/amd64` with `load: true`, *runs* it, and only then builds the
multi-arch manifest list — which reuses every layer from the buildx cache.

The two-phase shape is forced rather than chosen: a multi-arch buildx build
cannot be `docker load`ed (the daemon holds one platform per tag), and an
arm64 image on an amd64 runner executes under QEMU, which makes a boot
failure ambiguous between the image and the emulator.

**`ordo-cli`** — runs `ordo check /fx/market.ordo` against the real market
fixture bind-mounted in. `check` parses, binds and reports, touching no
database, so it exercises the compiler front end inside the runtime base —
which is the thing a wrong base image breaks. Then asserts a bare
invocation exits **2** (usage), which proves the `ENTRYPOINT` is the binary
rather than a shell that swallowed the arguments. The same check runs as a
`--target smoke` stage inside `Dockerfile.cli`, so a local
`docker build` catches it too.

**`ordo-host`** — runs the container against a Postgres service and asserts
three endpoints. The interesting one is `/health/deep`, and it needs
explaining, because the obvious version of this test is wrong.

`/health/deep` is **owner-gated**. It names the deployment's `KeyProvider`
and its database, so it is checked against a reserved op that grants to
owners only; `deployment.rs`'s own test asserts 403 for unauthenticated,
403 for a bad token, 403 for a non-owner member, 200 for the owner. A smoke
test that curls it expecting 200 would fail on a correct image. A smoke test
that curls it and accepts any response would pass on a broken one.

So the assertion is on **which** status, with three distinct meanings:

| | |
|---|---|
| **403** | the route is mounted **and** the access check is live. Pass. |
| **404** | the platform router did not mount. The binary booted with no deployment surface at all. |
| **200** | the route is mounted and **the gate is missing** — an unauthenticated caller just read the KeyProvider's identity out of the image. |

The 200 case is the one most worth catching before a push, and it is
exactly the case a laxer smoke test passes.

`/health/live` must be 200 with a `"live"` body: it touches nothing by
design (a probe that consults a dependency turns one outage into two), which
is what makes it a clean "the process came up" signal.

`/health/ready` must be **200 or 503, never 404**. Owner-ruled 2026-09-20
(17t): readiness means ready to *accept* a deployment, not "a program is
deployed" — database, custody, configuration, workers and the deploy path
all reachable and correct is **200**, even for a bare image with nothing
applied, and the body's `program_active` says so honestly. **Phase 2
[open]:** once the smoke can run `ordo apply` against the service database
first, assert `program_active: true` in the body too, which then proves the
shape gate ran *inside the image*. That is the strongest available boot
proof and it is worth building.

**`ordo-playground`** — asserts the boot *seeded*, not merely that a port
opened. `main.rs` prints `seeded: <db>` per database and `UNAVAILABLE: <db>
(…)` when one could not be built; `UNAVAILABLE` is a startup that succeeds
and a product that does not work, and it looks perfectly healthy from
outside. The smoke fails on any `UNAVAILABLE` line and requires at least one
`seeded:`, which is what proves `psql` reached Postgres *from inside the
image* — the single most likely thing to be wrong about that image.

### 3.4 Registry and tags

**GHCR**, `ghcr.io/<owner>/<repo>/<image>`. Chosen for one reason above all
others: it authenticates with the automatic `GITHUB_TOKEN`, so **this plan
requires no secrets at all**. Docker Hub or ECR would each add a credential
and a rotation obligation, which is a real cost and should be paid
deliberately rather than by default.

Tagging is built to rhyme with the identity discipline `deploying.md` §7
already runs on programs: `ordo apply` stores an artifact under its
**version hash**, `$activations` references it by that hash, and `ordo
rollback --version <hash>` names one the registry already holds. The
identity is the content, and the pointers are separate from it.

The image tagging says the same thing:

**The identity is the content digest** (`sha256:…`), emitted by
`build-push-action` and written into the run summary as *"Identity (deploy
this)"*. It is the only thing a deployment manifest should name.

Everything `metadata-action` produces is a **pointer**:

| tag | what it is for |
|---|---|
| `sha-<short>` | immutable in practice, and the one that traces back to a commit. Deploy by digest; find the commit by this. |
| `<branch>` | moves. For pulling "whatever main is" by hand. |
| `v1.2.3`, `v1.2` | move only if someone force-pushes a tag — which is exactly why they are still not the identity. |
| `latest` | default branch only. A convenience for `docker run`. **Never in a deployment manifest.** |

The recommendation is stated in the release notes the workflow generates,
so it reaches the person choosing what to deploy rather than only the person
who read this file: **deploy by digest, not by tag.**

### 3.5 Multi-arch

`linux/amd64` and `linux/arm64` via `docker/setup-buildx-action` +
`docker/setup-qemu-action`. arm64 matters because Graviton and Apple Silicon
are both ordinary places to run this, and because the release binaries
(§4) already cover four targets — an amd64-only image would be the odd one
out.

QEMU builds arm64 slowly. The buildx GHA cache (`type=gha,mode=max`,
**scoped per image** so three images do not evict each other) is what makes
that tolerable, and the smoke phase runs on native amd64 so the slow path is
never in the feedback loop for a boot failure.

**[open]** If arm64 build time becomes the bottleneck, the answer is a
native `ubuntu-24.04-arm` runner in a matrix with `docker/build-push-action`
digest outputs merged by `docker buildx imagetools create` — not more QEMU.

---

## 4 · Increment 3 — the release path

**File:** `.github/workflows/release.yml` **[proposed]**, unwired. Trigger
on adoption: `push: tags: ['v*']`.

**Images** are not rebuilt here. `images.yml` already builds, smokes and
pushes on a `v*` tag; rebuilding would produce a second set of bytes for the
same commit under the same tag, and **two digests claiming to be one release
has none**. The release job states the dependency and links to the other
workflow's summary.

**CLI binaries** for four targets — `x86_64`/`aarch64` × linux-gnu/darwin —
on native runners rather than by cross-compilation. Cross would work (§1.5),
but a binary built on the platform it runs on is one whose linkage nobody
has to reason about, and the matrix is four short jobs. `fail-fast: false`,
so one target failing does not hide the other three.

`ordo` is an operator tool: someone runs it from a laptop or a bastion
against a deployment's database (`deploying.md` §9). Handing them a
container for that is worse ergonomics than a tarball, so the release
carries both.

Each binary is **smoke-tested the same way the image is** — it compiles the
market fixture. A release artifact that cannot parse the project's own
fixture is not a release artifact. Each archive ships a SHA-256 beside it,
which is the same argument the artifact version hash makes one layer up: a
download is only as good as the thing that says what it should have been.

**SBOM and provenance — recommended, and wired, not mandated.**

- Images: `provenance: mode=max` and `sbom: true` on
  `build-push-action`. BuildKit generates both and attaches them to the
  image. `mode=max` records the full build definition **including the base
  image digest**, which is the question you actually have when a CVE lands
  on a base.
- Binaries: `actions/attest-build-provenance@v2` over the tarballs.

Both cost one step and no secret. Neither is load-bearing for anything else
in this plan, so both can be deleted by anyone who disagrees without
breaking a build.

**Cosign — recommended against, for now.** Keyless cosign via OIDC would add
a signature over the digest. It is real value, but it duplicates what
`attest-build-provenance` already gives (a verifiable statement tying the
artifact to this repository and workflow) and it adds a verification story
someone has to document and support. Revisit when there is an external
consumer who asks for it; adding it later costs one step and changes nothing
already published.

### 4.1 What is deliberately not in scope

- **No CD.** Nothing in this plan deploys. `deploying.md` §7 makes
  deployment a two-step operator act — `ordo apply` stores an artifact and
  re-points a registry, and the node re-runs the load gate on the swap — and
  the whole point of that design is that a human names the version. A
  pipeline that pushed an image and then activated a program would be
  automating past the gate the platform exists to run.
- **No Helm chart, and not yet an argument for a minimal one.** Considered,
  and declined for a specific reason rather than a general one: a deployment
  today is "one pod plus a Postgres connection", and the interesting
  configuration is the `KeyProvider`, the page key and the token key —
  **all three of which are unbuilt** (`deploying.md` §4's status note;
  checklist 8a–8d). A chart written now would template environment
  variables for secrets that do not have a provider yet, and would then be
  the document people trust about custody. Write it when 8a–8d land, and
  write it against what they actually take. A `docker run` line and a
  documented env list are honest in the meantime.
- **No image vulnerability scanning gate.** Worth adding (Trivy or Grype on
  the built image, reported not blocking, at first). Not in this increment
  because a scanner that blocks on a base-image CVE nobody can fix that
  week teaches people to bypass it. **[open]**
- **No `cargo audit` / `cargo deny`.** Same class of decision, same
  argument, and it interacts with §1.2: an advisory gate over an
  unlockfiled dependency graph fails on days when nothing changed.
  Sequence it after 0b. **[open]**

---

## 5 · What was ruled out, and why

**Building images on every pull request.** Three images × two architectures,
with arm64 under QEMU, costs more runner-minutes than the entire test suite
— to answer "does it containerise?", which changes on approximately none of
them. The Dockerfiles change rarely and the dependency graph changes rarely;
when either does, the change is visible in the diff and can be built on
demand with `workflow_dispatch`. Images build on `main` and on tags.
*Reconsider if* a Dockerfile or a manifest starts changing weekly, at which
point a `paths:`-filtered PR build is the narrow answer — not a blanket one.

**Running the conformance suite inside the image build.** Wrong layer,
three times over:

- It needs a **live Postgres server** for the execute tier. A `RUN` step in
  a Docker build has no service container and no network the workflow
  controls; wiring one in means either a sidecar during build (fragile,
  unsupported in buildx cleanly) or a Postgres inside the builder stage,
  which is a second cluster with second defaults and a second way to be
  wrong.
- It needs the **docs tree** (§1.4), because the inventory tests parse
  `api-engine-design.md`. A build context trimmed to `crates/` fails them.
- It puts the assurance argument behind a **cache**. A cached `RUN cargo
  test` layer is a test result nobody ran, which is a more elaborate version
  of the exact failure `corpus.md` opens by refusing: a green run that never
  reached Postgres.

Tests run in `ci.yml` against a service container. The image build compiles
and smoke-tests; those are different questions and they belong in different
layers.

**A single monolithic workflow.** Testing and image-building have different
triggers (every PR vs. `main` and tags), different costs, different failure
meanings, and different permissions — `images.yml` needs `packages: write`
and `id-token: write`, and `ci.yml` needs neither. Merging them grants the
token that can push an image to every pull request run, which is a real
escalation for an aesthetic gain. Separate files also mean a red X in the
checks list names *which* thing broke.

**`--test-threads=1` to stabilise the suite.** It would appear to work and
it would hide the property `Pg::named` exists to provide (§1.3, and
`corpus.md`'s "Databases"). If the suite is flaky under concurrency, that is
a suite that has stopped isolating its databases and it should fail.

**`latest` as a deployable tag.** Ruled out in §3.4. A mutable pointer is
not an identity, and this project's whole deployment story is built on that
distinction.

**A `HEALTHCHECK` instruction in the host image.** Distroless has no shell
and no `curl`, and adding either to satisfy a `HEALTHCHECK` would undo the
base image. Probes belong to the orchestrator, which does the HTTP itself.

**Docker Hub as the registry.** It would require a secret and a rotation
obligation. GHCR requires neither (§3.4).

---

## 6 · Sequence and done-when

| | increment | done when |
|---|---|---|
| **0a** | `ordo-host` binary | `cargo build -p ordo-host --bin ordo-host` produces a binary that binds `ORDO_HOST_ADDR`, assembles `PlatformState` from a `Verified` deployment, and **exits non-zero** at boot under `TestProvider` without an explicit opt-in. `images.yml`'s host probe stops skipping. |
| **0b** | commit `Cargo.lock` | The file is tracked, `.gitignore` no longer lists it, and `--locked` is on every `cargo build` in the Dockerfiles and the release workflow. Two builds of one commit produce the same image digest. |
| **0c** | playground bind address | `ORDO_PLAYGROUND_ADDR` is read in `main.rs`, defaulting to `127.0.0.1:7878`. `images.yml`'s playground probe stops skipping. |
| **1** | test pipeline | `ci-proposed.yml` is wired and `ci.yml` deleted, in one commit. The executed-case assertion fails with the Postgres service removed **and** fails if `C001`'s test is renamed. A deleted row in `api-engine-design.md`'s table produces a failure whose step name identifies the direction. |
| **2** | images | `images.yml` builds, smokes and pushes three images on `main`. Every smoke gate in §3.3 has been observed to fail on a deliberately broken image at least once — a gate nobody has seen fail is a gate nobody has tested. |
| **3** | release | A `v*` tag produces a GitHub release with four smoke-tested tarballs, their checksums and provenance, and links to three image digests. |

Increment 1 does not depend on 0a–0c. Increment 2's `cli` job does not
either. Only the host and playground images wait.

---

## 7 · Findings recorded, not scheduled

- **§1.6** — `ordo-conformance` is a normal dependency of `ordo-host` and
  `ordo-exec`. The conformance harness and its fixtures are compiled into
  the deployable. Weight, not a hazard; a layering fix, not a CI one.
- **§2.3** — the live `ci.yml`'s executed-case assertion is satisfiable by a
  test other than the one it names. Fixed in `ci-proposed.yml`; the live
  file is untouched by this plan.
- **§1.2** — no `Cargo.lock`, and three separate consequences.
- **§1.5** — the pure-Rust dependency graph is an asset. Keep it: prefer
  rustls over `native-tls` if TLS is ever added, or the distroless base and
  the trivial cross-target release matrix both stop being free.

## Related

- [`corpus.md`](corpus.md) — the assurance model the CI job has to carry
  without weakening.
- [`deploying.md`](deploying.md) — what a deployment is, and the
  artifact-hash identity discipline §3.4 rhymes with.
- [`roadmap-checklist.md`](roadmap-checklist.md) §15 — the work items.
- `scripts/conformance-pg.sh` — the cluster the service container stands in
  for, and the source of every default in §1.3.
