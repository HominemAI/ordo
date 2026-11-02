---
name: testing
description: How Ordo's suites are run since 2026-09-15 - affected crates locally, the full workspace on GitHub Actions. Use before running cargo test, when a suite is slow, when deciding what to run for a change, or when reading a CI result.
---

# Running the suites

The full Rust workspace suite takes about **82 minutes locally** and about
**11.5 minutes on CI**. So the full suite runs on CI, and only the crates a
change can reach run on this machine.

## Why the local suite is slow

macOS `syspolicyd`/XProtect vets every freshly linked binary, and a test
binary is fresh on every build. That is roughly 20 seconds per binary and
about 50 of the 82 minutes. It is not the test code, and no amount of test
splitting removes it.

The fix is to add the terminal to Developer Tools, which exempts what it
launches from that vetting. **Adam runs it, not Claude** (it changes a macOS
security setting):

```
sudo spctl developer-mode enable-terminal
```

Then System Settings → Privacy & Security → Developer Tools → enable the
terminal app. **Done on 2026-09-17 and measured on 2026-09-18:** a cold
workspace test build takes 30s, fresh binaries launch in milliseconds, and
seven crates (846 tests) run in about 90s. If suites turn slow again, check
`syspolicyd` CPU first.

## Locally: the crates a change can reach

Run the crate you changed plus the crates that depend on it:

```
cargo test -p <crate>
cargo nextest run -p <crate>      # what CI runs; same tests, many files at once
```

CI runs cargo-nextest (since 2026-11-01). It is not installed by default:
`bash scripts/ci-install-nextest.sh` fetches the pinned release and prints
where it put it; copy `cargo-nextest` into `~/.cargo/bin`. `cargo install
cargo-nextest --locked` also works.

nextest runs each test in its own process, and tests from many test files
at once. So:

- A file-wide `static SERIAL: Mutex` no longer keeps a file's tests one at a
  time. Neither does a `static ONCE` that seeds a shared database "once per
  binary": it now seeds once per test, under the file's other tests.
- Such files are listed in `.config/nextest.toml`, in a test group with
  `max-threads = 1`. **A new test file with either pattern goes in that
  list**, or it races under nextest and passes under `cargo test`.
- Measured 2026-11-01: ordo-conformance without its group failed 49 of 359
  tests; with it, all pass.
- An in-process limit (a `Semaphore`, an `RwLock`) does not reach across
  processes either. notes_app's deployments are capped by the `notes-app`
  group (3), and the files that boot deployments one at a time by
  `serial-in-file` (1): 4 at once against a 100-connection server. CI runs
  at most 4 tests at once per shard (`test-threads` in the `ci` profile).
- A test that counts the server's connections needs it to itself, which no
  group gives. notes_app's `connections_*` tests run in a second pass of the
  `host` shards, alone and one at a time.

To run one CI shard here: `bash scripts/ci-test-shard.sh unit`, `rest` or
`host 1 4`. Each honours `ORDO_CI_CRATES` and `ORDO_CI_FILTER` (below). Add
`--list` after `unit` or `host K N` to see which tests it gets.

The unit tier needs no Postgres: every crate's lib and bin tests, and the
source-scan test files in `NO_DATABASE` (scripts/ci-test-shard.sh). It ran
1,110 tests in 1.8 s locally on 2026-11-02, with no database reachable.
`DB_UNIT` there lists the 3 unit tests (ordo-cli `dev`, `dev_db`) that need
one; they run in `rest`. A new unit test that needs Postgres goes in
`DB_UNIT`. A new test file that needs none may go in `NO_DATABASE`, once
reading it shows it touches no database, server or process.

The dependencies worth knowing:

- `ordo-api` → `ordo-host`, `ordo-playground`
- `ordo-gen` → `ordo-cli`
- `ordo-syntax`, `ordo-schema`, `ordo-sema` → nearly everything, so a change
  there is a full-suite change and belongs on CI

The SDK is separate and fast, so it always runs in full:

```
cd clients/ordo-sdk && npm run check    # typecheck + vitest
```

## Before you push Rust: fmt and clippy, in that order

CI runs `cargo fmt --all -- --check` and then
`cargo clippy --workspace --all-targets -- -D warnings` **before** any test.
A formatting miss therefore fails the run with **zero tests executed**, which
reads like a catastrophe in the log and is usually one unwrapped line.

```
cargo fmt --all                                    # write
cargo fmt --all -- --check                         # prove
cargo clippy --workspace --all-targets -- -D warnings
```

This bit on 2026-09-16: three test functions written by script had lines
rustfmt wanted wrapped, and the run died at `Format` with `test result
lines: 0`. Tests had been run locally; fmt and clippy had not. Code inserted
by a script is the likeliest to need this, because nothing formatted it on
the way in.

## What CI runs, and when

Owner ruling, 2026-11-01. It holds for ci.yml, e2e.yml, console-e2e.yml
and identity-e2e.yml.

| Trigger | Runs |
|---|---|
| Push to `bleeding` | What the change reaches |
| Pull request to `main` | Everything. Every job is a required check |
| Nightly on `main`, 09:00 UTC | Everything |
| Run by hand (workflow_dispatch) | Everything |
| Push with a `ci: full` trailer | Everything |

- `main` moves only through a pull request from `bleeding`. Nothing runs on
  a push to `main` except images.yml and packages.yml.
- Each workflow's `changes (...)` job runs `scripts/ci-affected.mjs`. It
  diffs the push against the previous `bleeding` head. On a force-push or
  a new branch it diffs against where the branch left `origin/main`. With
  no base it runs everything.
- A job the change does not reach still runs. It says "Not reached by this
  change" and passes, so every check name exists on every run.
- To force a full run on `bleeding`, end a commit message with the line
  `ci: full`. Any commit in the push counts.
- To see what a change reaches before pushing:
  `node scripts/ci-affected.mjs --diff "$(git merge-base origin/bleeding HEAD)"`.
- The filter's tests: `node scripts/ci-affected.test.mjs`. CI runs them in
  the `audit` job. They also check `.config/ci-test-map.json` (below).

How the filter reads a path:

- Cargo.lock, the root Cargo.toml, rust-toolchain, `.github/`,
  `.cargo/config.toml`, `docker/`, `scripts/ci-*`, or a path it does not
  know: everything.
- A crate's code: that crate and every crate that depends on it. The
  tiers get the list in `ORDO_CI_CRATES`; the `host` shards run only when
  ordo-host is in it.
- A test file under `crates/<X>/tests/`: that test binary alone, and no
  unit or doc tests (owner, 2026-11-02, `test-in-proportion-to-the-change`
  ask 1). A helper there (`tests/mail_support/`, `tests/fixture/`): the
  binaries whose source names it; when none does, all of X's binaries.
- A file under `crates/ordo-host/src/`: the ordo-host test binaries
  `.config/ci-test-map.json` lists for it, plus ordo-host's unit and doc
  tests (ask 2). `"all"`, or a file not in the map, runs every one. Crates
  that depend on ordo-host still run whole.
- The narrowing reaches the shards as `ORDO_CI_FILTER`, a nextest
  filterset (the `test_filter` output). Empty means every test.
- `clients/<pkg>/`: that client's job. ordo-sdk and ordo-react also run
  every example, the notes e2e and ordo-host's tests.
- `examples/<app>/`: that example's job. notes and notes-v2 also run the
  notes e2e.
- Docs and `*.md`: nothing, unless a crate's source names the file in a
  string (`include_str!`, a fixture path). Then that crate's tests run.
- Each e2e runs when a crate it builds is reached.

The test map (`.config/ci-test-map.json`):

- A new file under `crates/ordo-host/src/` must be added, or
  `node scripts/ci-affected.test.mjs` fails and says so. `"all"` is
  always safe.
- A list names test binaries (`tests/<name>.rs`). Err toward more: a test
  left off a list does not run on `bleeding` when that file changes. The
  pull request to `main` still runs it.
- Files every request goes through (lib.rs, runtime.rs, serve.rs, auth/,
  settings.rs and the like) are `"all"`.
- Changing the map runs everything.

Job names are unique across all workflows; required checks match by name.
Rename a job only together with the ruleset.

## The full suite: push and watch

CI (`.github/workflows/ci.yml`) runs on the self-hosted runner, each test
job against its own leased CI Postgres server:

- `test (unit)` runs first: the unit tier, no Postgres, no Node. The
  `test` shards and `upgrade` wait for it and fail when it fails.
- `test (rest)` runs fmt, clippy, every crate's tests but ordo-host's and
  the unit tier's, then the doc tests, the S3 contract, the C001 proof and
  the inventories.
- `test (host K 4)` each run every 4th ordo-host integration test
  (`--partition slice:K/4`).
- Each `test` job builds what it runs from the warm `rust-gates` cache.
  One shared build was tried on 2026-11-01 and dropped: its 1.74 GB took
  4.5 min per job to download, against about 1 min to build.

A push to `bleeding` runs only the reached crates; open the pull request to
`main` for the full suite, or push with `ci: full`.

```
git push
gh run list --limit 3
gh run watch <run-id>
```

`scripts/watch-ci.sh <sha>` waits for the run for a commit, watches it, and
then prints step timings, the pass and fail totals, and the failure lines.

Reading a result:

- Judge a run by the **result count and the failures**, not by the pass total
  alone. A run that covers half the workspace can still print a large number.
- nextest prints one `PASS`/`FAIL` line per test and a `Summary` line per
  shard (`N tests run: N passed`), not cargo's `test result:` lines.
- `gh run list --commit <short sha>` returns nothing; match the `headSha`
  prefix in the JSON instead.
- BSD `sed` does not treat `\t` as a tab. Parse logs with Python.

## What CI needs, and what broke it before

Three things were wrong when the workflow was repaired on 2026-09-15, and all
three are easy to reintroduce:

1. **`ORDO_PG_PASSWORD`** is what the Rust pool reads; `PGPASSWORD` is only
   for `psql`. Tests that dial Postgres use
   `PgRoute::as_configured(label)`, which takes the password when that
   variable is set and goes passwordless when it is not. A test that hardcodes
   `passwordless` passes locally and fails on CI. The same holds for a
   credential file a test writes: put `ORDO_PG_PASSWORD` in it when set,
   nothing when not. A label made of a word like `local` also matches the
   host `localhost` in a printed label, so give a test credential a name that
   appears nowhere else.

   **Run CI's setup locally before pushing a suite that dials tenants.** A
   trust-auth socket cluster admits everything, so a missing password shows
   only over TCP. Start a scram cluster on your own port:

   ```
   echo ordo > $TMPDIR/pw
   initdb -D $D -U ordo --auth=scram-sha-256 --pwfile=$TMPDIR/pw
   pg_ctl -D $D -o "-p 55463 -c listen_addresses=localhost -k $D" -l $D/log start
   PGPASSWORD=ordo createdb -h localhost -p 55463 -U ordo ordo_conformance
   export ORDO_PG_HOST=localhost ORDO_PG_PORT=55463 ORDO_PG_USER=ordo \
     ORDO_PG_DB=ordo_conformance ORDO_PG_PASSWORD=ordo PGPASSWORD=ordo ORDO_PSQL=psql
   ```

   Leave `max_connections` at 100 as CI does. Stop it and delete `$D` after.
   A door or recovery test that fails once in a while on CI and not here is
   usually a clock: a frozen test clock and a live one meet in the same
   second.
2. **`--no-fail-fast`**, or one failing crate hides the rest of the workspace
   and the run looks smaller than it is. nextest stops at the first failure
   by default; CI's `ci` profile (`NEXTEST_PROFILE=ci`) and the shard script
   both turn that off.
3. **Disk.** The runner fills up and Postgres dies with
   `PANIC ... No space left on device`. The workflow frees the preinstalled
   SDKs and builds with `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_INCREMENTAL=0`.

When a database error is unclear, `ordo_exec::described` appends the source
chain; `e.to_string()` on a `tokio_postgres::Error` prints only the kind and
hides the cause.

## Keep the test databases

The local Postgres test databases stay ("we'll keep the DB now for testing").
Do not propose replacing them with fakes.

## Before a push

Never start a cargo build while a background suite is running; they fight over
the same target directory. Check for a running suite first.
