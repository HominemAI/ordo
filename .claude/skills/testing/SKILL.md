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
```

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

## The full suite: push and watch

CI (`.github/workflows/ci.yml`) runs fmt, clippy and
`cargo test --workspace --no-fail-fast` against a `postgres:17` service on a
hosted runner.

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
   and the run looks smaller than it is.
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
