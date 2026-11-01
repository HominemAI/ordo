#!/usr/bin/env bash
# Build the workspace's tests once and run them in shards with cargo-nextest
# (owner ruling 2026-11-01, board task `cut-test-time-by-building-and`).
#
#   scripts/ci-test-shard.sh archive FILE   build the tests and write a
#                                           nextest archive to FILE
#   scripts/ci-test-shard.sh unpack FILE    unpack FILE into this checkout
#   scripts/ci-test-shard.sh doc            the doc tests (nextest runs none)
#   scripts/ci-test-shard.sh rest           every crate but ordo-host, and
#                                           ordo-host's unit tests
#   scripts/ci-test-shard.sh host K N       ordo-host's integration tests,
#                                           shard K of N (1-based)
#   scripts/ci-test-shard.sh host K N --list    print shard K's tests only
#   scripts/ci-test-shard.sh run ARGS...    `nextest run ARGS`, for the
#                                           workflow's named checks
#
# After `unpack`, the run modes use the unpacked build and compile nothing.
# Without it they build what they run, so they work on any machine.
#
# The `host` shards take every Nth test of ordo-host's integration tests
# (`--partition slice`), so a long test file is spread over all shards.
# Which tests must not run at once is in .config/nextest.toml. The tests
# that need the server to themselves run in a second pass, alone.
#
# ORDO_CI_CRATES narrows `archive`, `doc` and `rest` to the crates a push to
# `bleeding` reaches (scripts/ci-affected.mjs, space separated). Unset or
# `all` means every crate. ordo-host's unit and doc tests run only when it
# is listed; the workflow runs the `host` shards only then too.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
mode="${1:?usage: ci-test-shard.sh archive FILE | unpack FILE | doc | rest | host K N [--list] | run ARGS}"
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

# `nextest MODE BUILD... -- RUN...`: from the unpacked build when there is
# one (BUILD is ignored), else built here from BUILD. `cargo-nextest` is
# called directly so a job without a Rust toolchain can run the build.
nextest() {
  local sub=$1 build=()
  shift
  while [ $# -gt 0 ] && [ "$1" != -- ]; do build+=("$1"); shift; done
  shift
  if [ "${ORDO_NEXTEST_UNPACKED:-}" = 1 ]; then
    cargo-nextest nextest "$sub" \
      --binaries-metadata "$here/target/nextest/binaries-metadata.json" \
      --cargo-metadata "$here/target/nextest/cargo-metadata.json" \
      --target-dir-remap "$here/target" \
      --workspace-remap "$here" \
      "$@"
  else
    cargo nextest "$sub" ${build[@]+"${build[@]}"} "$@"
  fi
}

case "$mode" in
  archive)
    file="${2:?archive FILE}"
    if [ "$all" = 1 ]; then
      cargo nextest archive --workspace --archive-file "$file"
    else
      echo "Building the tests of the crates this change reaches: $crates"
      cargo nextest archive ${pkgs[@]+"${pkgs[@]}"} --archive-file "$file"
    fi
    ;;
  unpack)
    file="${2:?unpack FILE}"
    # `list` with --extract-to unpacks the archive and keeps it. The test
    # binaries were built in this checkout's path on the build job, and
    # `env!("CARGO_BIN_EXE_…")` and `env!("CARGO_MANIFEST_DIR")` are those
    # paths, so target/ goes back where it was.
    cargo-nextest nextest list --archive-file "$file" \
      --extract-to "$here" --extract-overwrite --workspace-remap "$here" \
      >/dev/null
    echo "Unpacked $file into $here/target"
    if [ -n "${GITHUB_ENV:-}" ]; then
      echo "ORDO_NEXTEST_UNPACKED=1" >>"$GITHUB_ENV"
    fi
    ;;
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
    echo "unknown mode: $mode (archive | unpack | doc | rest | host K N | run)" >&2
    exit 2
    ;;
esac
