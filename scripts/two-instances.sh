#!/usr/bin/env bash
# Two `ordo-host` processes on one database and one tenant
# (`crates/ordo-host/tests/two_instances.rs`, board task
# `horizontal-scaling-before-v1`, item 2).
#
# Each test boots the real binary twice, so they are `#[ignore]` and do not
# run in the ordinary suite. This script runs them.
#
# Locally it starts a Postgres cluster of its own, with its own data
# directory and port. On CI (`CI=1`), it uses the Postgres the workflow
# already provides and starts nothing.
#
# Pass extra test arguments after the script name, for one test:
#   scripts/two-instances.sh the_guessing_slowdown_counts_tries_on_both
set -euo pipefail

cd "$(dirname "$0")/.."

if [ "${CI:-}" = "1" ]; then
  exec cargo test -p ordo-host --test two_instances -- --ignored --nocapture --test-threads=1 "$@"
fi

export ORDO_PGDATA="${ORDO_PGDATA:-$HOME/.ordo-pg/two-instances}"
export ORDO_PG_HOST="${ORDO_PG_HOST:-/tmp/ordo-pg-two}"
export ORDO_PG_PORT="${ORDO_PG_PORT:-55495}"
export ORDO_PG_USER="${ORDO_PG_USER:-ordo}"

cleanup() {
  ORDO_PGDATA="$ORDO_PGDATA" ORDO_PG_HOST="$ORDO_PG_HOST" ORDO_PG_PORT="$ORDO_PG_PORT" \
    scripts/conformance-pg.sh stop || true
}
trap cleanup EXIT

ORDO_PGDATA="$ORDO_PGDATA" ORDO_PG_HOST="$ORDO_PG_HOST" ORDO_PG_PORT="$ORDO_PG_PORT" \
  scripts/conformance-pg.sh start

cargo test -p ordo-host --test two_instances -- --ignored --nocapture --test-threads=1 "$@"
