#!/usr/bin/env bash
# Upgrade continuity: the previous Ordo makes the state, this one serves it.
#
#     scripts/upgrade-continuity.sh          # build the previous Ordo, run both tests, clean up
#     scripts/upgrade-continuity.sh build    # only build the previous Ordo
#     scripts/upgrade-continuity.sh clean    # delete the previous build and the cluster
#
# `run` (the default) does four things, each bounded in time:
#
# 1. Exports the previous Ordo's source (`ORDO_UPGRADE_FROM`, a tag or a
#    commit) with `git archive` and builds its `ordo-host` and `ordo` into a
#    target directory of its own, so this checkout's build is not touched.
# 2. Starts a Postgres cluster of its own (unless `ORDO_UPGRADE_OWN_PG=0`,
#    as CI sets, where a service container is the database).
# 3. Runs `crates/ordo-host/tests/upgrade_continuity.rs`: the program
#    upgrade, and the ignored Ordo upgrade with the previous binaries named.
# 4. Stops and deletes the cluster, and deletes the previous build unless
#    `ORDO_UPGRADE_KEEP=1` (a rerun then skips step 1).
#
# The previous Ordo defaults to the repository's first commit, the first
# public release (owner, 2026-10-31). Pin it to the newest release tag
# once releases are cut.
#
# Settings: ORDO_UPGRADE_FROM, ORDO_UPGRADE_DIR (default ~/.ordo-upgrade),
# ORDO_UPGRADE_KEEP, ORDO_UPGRADE_OWN_PG, ORDO_PGDATA, ORDO_PG_HOST,
# ORDO_PG_PORT, ORDO_PGBIN.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
from=${ORDO_UPGRADE_FROM:-$(git -C "$repo" rev-list --max-parents=0 HEAD | tail -1)}
work=${ORDO_UPGRADE_DIR:-$HOME/.ordo-upgrade}
own_pg=${ORDO_UPGRADE_OWN_PG:-1}
keep=${ORDO_UPGRADE_KEEP:-0}

if [ "$own_pg" = 1 ]; then
  export ORDO_PGDATA=${ORDO_PGDATA:-$HOME/.ordo-pg/upgrade-continuity}
  export ORDO_PG_HOST=${ORDO_PG_HOST:-/tmp/ordo-pg-upg}
  export ORDO_PG_PORT=${ORDO_PG_PORT:-55447}
fi

# Run "$@" for at most $1 seconds. Past it the command is stopped, then
# killed, and the step fails.
bounded() {
  local secs=$1
  shift
  "$@" &
  local pid=$!
  (
    sleep "$secs"
    echo "upgrade-continuity: '$*' ran past ${secs}s, stopping it" >&2
    kill -TERM "$pid" 2>/dev/null || true
    sleep 15
    kill -KILL "$pid" 2>/dev/null || true
  ) &
  local watchdog=$!
  local status=0
  wait "$pid" || status=$?
  kill "$watchdog" 2>/dev/null || true
  wait "$watchdog" 2>/dev/null || true
  return "$status"
}

sha=$(git -C "$repo" rev-parse --short=12 "$from^{commit}")
src="$work/src-$sha"
target="$work/target"
old_host="$target/debug/ordo-host"
old_cli="$target/debug/ordo"

build() {
  if [ ! -f "$src/Cargo.toml" ]; then
    rm -rf "$src"
    mkdir -p "$src"
    git -C "$repo" archive "$sha" | tar -x -C "$src"
  fi
  echo "building the previous Ordo ($from = $sha) into $target"
  bounded 2400 cargo build --manifest-path "$src/Cargo.toml" -p ordo-host -p ordo-cli \
    --target-dir "$target"
  [ -x "$old_host" ] && [ -x "$old_cli" ]
}

pg() {
  [ "$own_pg" = 1 ] || return 0
  bounded 120 bash "$repo/scripts/conformance-pg.sh" "$1"
}

clean() {
  pg destroy || true
  if [ "$keep" != 1 ]; then
    rm -rf "$work"
    echo "removed $work"
  fi
}

case "${1:-run}" in
  build)
    build
    ;;
  clean)
    keep=0
    clean
    ;;
  run)
    build
    trap clean EXIT
    pg start
    started=$(date +%s)
    export ORDO_UPGRADE_OLD_HOST="$old_host" ORDO_UPGRADE_OLD_CLI="$old_cli"
    bounded 1500 cargo test --manifest-path "$repo/Cargo.toml" -p ordo-host \
      --test upgrade_continuity -- --include-ignored --test-threads 1 --nocapture \
      a_compatible_program_update the_previous_ordo_hands_over
    echo "upgrade continuity: passed in $(( $(date +%s) - started ))s (from $from)"
    ;;
  *)
    echo "usage: $0 [run|build|clean]" >&2
    exit 2
    ;;
esac
