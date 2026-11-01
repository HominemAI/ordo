#!/usr/bin/env bash
# Run the workspace's tests in shards with cargo-nextest (owner ruling
# 2026-11-01, board task `cut-test-time-by-building-and`).
#
#   scripts/ci-test-shard.sh doc            the doc tests (nextest runs none)
#   scripts/ci-test-shard.sh rest           every crate but ordo-host, and
#                                           ordo-host's unit tests
#   scripts/ci-test-shard.sh host K N       ordo-host's integration tests,
#                                           shard K of N (1-based)
#   scripts/ci-test-shard.sh host K N --list    print shard K's tests only
#   scripts/ci-test-shard.sh run ARGS...    `nextest run ARGS`, for the
#                                           workflow's named checks
#
# Each mode builds what it runs. One build shipped to every shard was
# tried and dropped (ask 5): its download took longer than the build.
#
# The `host` shards take every Nth test of ordo-host's integration tests
# (`--partition slice`), so a long test file is spread over all shards.
# Which tests must not run at once is in .config/nextest.toml. The tests
# that need the server to themselves run in a second pass, alone.
#
# ORDO_CI_CRATES narrows `doc` and `rest` to the crates a push to
# `bleeding` reaches (scripts/ci-affected.mjs, space separated). Unset or
# `all` means every crate. ordo-host's unit and doc tests run only when it
# is listed; the workflow runs the `host` shards only then too.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
mode="${1:?usage: ci-test-shard.sh doc | rest | host K N [--list] | run ARGS}"
crates="${ORDO_CI_CRATES:-all}"
all=0
[ "${crates// /}" = all ] && all=1
host_reached=$all
pkgs=()
for c in $crates; do
  [ "$c" = all ] && continue
  pkgs+=(-p "$c")
  [ "$c" = ordo-host ] && host_reached=1
done

# `nextest MODE BUILD... -- RUN...`: cargo nextest MODE with both lists.
nextest() {
  local sub=$1 build=()
  shift
  while [ $# -gt 0 ] && [ "$1" != -- ]; do build+=("$1"); shift; done
  shift
  cargo nextest "$sub" ${build[@]+"${build[@]}"} "$@"
}

case "$mode" in
  doc)
    # --no-fail-fast: one failing crate must not hide the rest.
    if [ "$all" = 1 ]; then
      cargo test --workspace --doc --no-fail-fast
    elif [ ${#pkgs[@]} -gt 0 ]; then
      cargo test "${pkgs[@]}" --doc --no-fail-fast
    fi
    ;;
  rest)
    host_unit='(package(ordo-host) & (kind(lib) | kind(bin)))'
    if [ "$all" = 1 ]; then
      filter="not package(ordo-host) | $host_unit"
      build=(--workspace)
    else
      echo "Testing the crates this change reaches: $crates"
      filter=""
      for c in $crates; do
        [ "$c" = ordo-host ] && continue
        filter="${filter:+$filter | }package($c)"
      done
      [ "$host_reached" = 1 ] && filter="${filter:+$filter | }$host_unit"
      build=(${pkgs[@]+"${pkgs[@]}"})
    fi
    if [ -z "$filter" ]; then
      echo "No crate to test."
      exit 0
    fi
    # A reached crate may have no tests at all; that is not a failure.
    nextest run ${build[@]+"${build[@]}"} -- --no-fail-fast --no-tests warn -E "$filter"
    ;;
  host)
    k="${2:?shard number}"
    n="${3:?shard count}"
    part=(--partition "slice:$k/$n")
    # notes_app's `connections_*` tests count the server's connections, so
    # they run after the rest, one at a time, with nothing beside them.
    alone='binary_id(ordo-host::notes_app) & test(/^connections_/)'
    main="package(ordo-host) & kind(test) & not ($alone)"
    if [ "${4:-}" = "--list" ]; then
      nextest list -p ordo-host -- -E "$main" "${part[@]}"
      nextest list -p ordo-host -- -E "$alone" "${part[@]}"
      exit 0
    fi
    status=0
    nextest run -p ordo-host -- --no-fail-fast -E "$main" "${part[@]}" || status=$?
    nextest run -p ordo-host -- --no-fail-fast --no-tests warn --test-threads 1 \
      -E "$alone" "${part[@]}" || status=$?
    exit "$status"
    ;;
  run)
    shift
    nextest run --workspace -- "$@"
    ;;
  *)
    echo "unknown mode: $mode (doc | rest | host K N | run)" >&2
    exit 2
    ;;
esac
