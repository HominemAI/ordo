#!/usr/bin/env bash
# Run the workspace's tests in shards with cargo-nextest (owner ruling
# 2026-11-01, board task `cut-test-time-by-building-and`).
#
#   scripts/ci-test-shard.sh unit           every crate's unit tests, and the
#                                           test files that need no database
#   scripts/ci-test-shard.sh unit --list    print the unit tier's tests only
#   scripts/ci-test-shard.sh doc            the doc tests (nextest runs none)
#   scripts/ci-test-shard.sh rest           every other test but ordo-host's
#   scripts/ci-test-shard.sh host K N       ordo-host's other test files,
#                                           shard K of N (1-based)
#   scripts/ci-test-shard.sh host K N --list    print shard K's tests only
#   scripts/ci-test-shard.sh run ARGS...    `nextest run ARGS`, for the
#                                           workflow's named checks
#
# The tiers (owner ruling 2026-11-02, board task
# `test-in-proportion-to-the-change` asks 4 and 5): `unit` needs no
# database and no running instance, and CI runs it first. `rest` and
# `host` need Postgres. No test runs in two tiers: NO_DATABASE below is
# the one list of test files that move to `unit`, and DB_UNIT the one
# list of unit tests that stay out of it (found by running `unit` with
# no Postgres reachable). A file joins it only
# when reading it shows it touches no database, server or process; when
# unsure, it stays out.
#
# Each mode builds what it runs. One build shipped to every shard was
# tried and dropped (ask 5): its download took longer than the build.
#
# The `host` shards take every Nth test of ordo-host's integration tests
# (`--partition slice`), so a long test file is spread over all shards.
# Which tests must not run at once is in .config/nextest.toml. The tests
# that need the server to themselves run in a second pass, alone.
#
# On a push to `bleeding`, scripts/ci-affected.mjs narrows each mode:
#
# - ORDO_CI_CRATES: the crates the change reaches, space separated. Each
#   mode builds only those. Unset or `all` means every crate.
# - ORDO_CI_FILTER: a nextest filterset of the tests it reaches (one test
#   file, or the files .config/ci-test-map.json maps an ordo-host source
#   file to). Joined to each mode's own filter. Unset or empty means every
#   test.
# - ORDO_CI_TESTS_ONLY: crates whose tests alone changed. Their doc tests
#   do not run.
#
# A narrowed shard may hold no test at all; that passes.
set -euo pipefail

# Test files that need no database and no running instance: each scans
# the workspace's source. `package::binary`, one per line.
NO_DATABASE="
ordo-api::door_refusal_scan
ordo-console::door_scan
ordo-host::auth_store_failure_scan
ordo-host::blob_error_text_scan
ordo-host::custody_error_text_scan
ordo-host::db_error_text_scan
ordo-host::edge_scan
ordo-host::mail_error_text_scan
ordo-host::provider_client_scan
ordo-host::stored_bytes_scan
"

# Unit tests that need Postgres. They run in `rest`, beside the test files
# that need it too, and never in `unit`.
DB_UNIT='binary_id(ordo-cli) & (
    test(=dev::tests::dev_honours_cors_and_passkey_settings_when_set_and_ignores_them_when_not)
  | test(=dev::tests::the_printed_token_calls_a_real_op_over_real_http)
  | test(=dev_db::tests::a_foreign_table_survives_a_reset_and_ordos_own_are_reset))'

mode="${1:?usage: ci-test-shard.sh unit | doc | rest | host K N [--list] | run ARGS}"
crates="${ORDO_CI_CRATES:-all}"
reach="${ORDO_CI_FILTER:-}"
tests_only=" ${ORDO_CI_TESTS_ONLY:-} "
all=0
[ "${crates// /}" = all ] && all=1

no_db=""
for b in $NO_DATABASE; do no_db="${no_db:+$no_db | }binary_id($b)"; done

# Whether this change reaches crate $1.
reached() { [ "$all" = 1 ] || [[ " $crates " == *" $1 "* ]]; }

# A mode's own filter, narrowed to what the change reaches.
narrow() {
  if [ -n "$reach" ]; then echo "($1) & ($reach)"; else echo "$1"; fi
}

# `nextest MODE BUILD... -- RUN...`: cargo nextest MODE with both lists.
nextest() {
  local sub=$1 build=()
  shift
  while [ $# -gt 0 ] && [ "$1" != -- ]; do build+=("$1"); shift; done
  shift
  cargo nextest "$sub" ${build[@]+"${build[@]}"} "$@"
}

case "$mode" in
  unit)
    # Each reached crate's lib and bins, and its NO_DATABASE files.
    build=()
    if [ "$all" = 1 ]; then
      build=(--workspace)
    else
      for c in $crates; do build+=(-p "$c"); done
    fi
    if [ ${#build[@]} -eq 0 ]; then
      echo "No crate to test."
      exit 0
    fi
    build+=(--lib --bins)
    for b in $NO_DATABASE; do
      if reached "${b%%::*}"; then build+=(--test "${b#*::}"); fi
    done
    filter=$(narrow "(kind(lib) | kind(bin) | $no_db) & not ($DB_UNIT)")
    if [ "${2:-}" = "--list" ]; then
      nextest list "${build[@]}" -- -E "$filter"
      exit 0
    fi
    nextest run "${build[@]}" -- --no-fail-fast --no-tests warn -E "$filter"
    ;;
  doc)
    # --no-fail-fast: one failing crate must not hide the rest.
    if [ "$all" = 1 ]; then
      cargo test --workspace --doc --no-fail-fast
    else
      pkgs=()
      for c in $crates; do
        [[ "$tests_only" == *" $c "* ]] || pkgs+=(-p "$c")
      done
      if [ ${#pkgs[@]} -gt 0 ]; then
        cargo test "${pkgs[@]}" --doc --no-fail-fast
      else
        echo "No reached crate's code changed: no doc tests."
      fi
    fi
    ;;
  rest)
    filter=$(narrow "not package(ordo-host) & ((not (kind(lib) | kind(bin)) & not ($no_db)) | ($DB_UNIT))")
    if [ "$all" = 1 ]; then
      build=(--workspace --exclude ordo-host)
    else
      echo "Testing the crates this change reaches: $crates"
      build=()
      for c in $crates; do
        [ "$c" = ordo-host ] || build+=(-p "$c")
      done
      if [ ${#build[@]} -eq 0 ]; then
        echo "No crate to test in this tier."
        exit 0
      fi
    fi
    # A reached crate may have no tests here at all; that is not a failure.
    nextest run "${build[@]}" -- --no-fail-fast --no-tests warn -E "$filter"
    ;;
  host)
    k="${2:?shard number}"
    n="${3:?shard count}"
    part=(--partition "slice:$k/$n")
    # notes_app's `connections_*` tests count the server's connections, so
    # they run after the rest, one at a time, with nothing beside them.
    alone='binary_id(ordo-host::notes_app) & test(/^connections_/)'
    main=$(narrow "package(ordo-host) & kind(test) & not ($alone) & not ($no_db)")
    alone=$(narrow "$alone")
    if [ "${4:-}" = "--list" ]; then
      nextest list -p ordo-host -- -E "$main" "${part[@]}"
      nextest list -p ordo-host -- -E "$alone" "${part[@]}"
      exit 0
    fi
    status=0
    nextest run -p ordo-host -- --no-fail-fast --no-tests warn -E "$main" "${part[@]}" || status=$?
    nextest run -p ordo-host -- --no-fail-fast --no-tests warn --test-threads 1 \
      -E "$alone" "${part[@]}" || status=$?
    exit "$status"
    ;;
  run)
    shift
    nextest run --workspace -- "$@"
    ;;
  *)
    echo "unknown mode: $mode (unit | doc | rest | host K N | run)" >&2
    exit 2
    ;;
esac
