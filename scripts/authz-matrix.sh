#!/usr/bin/env bash
# The authorization matrix (`crates/ordo-host/tests/authz_matrix.rs`).
#
# It boots the real `ordo-host` binary on real Postgres and walks every door
# route times every actor times every object, so it is `#[ignore]` and does
# not run in the ordinary suite. This script runs it.
#
# Locally it starts a Postgres cluster of its own, with its own data
# directory and port, so it never touches the shared conformance cluster's
# databases. On CI (`CI=1`), it uses the Postgres the workflow already
# provides (ORDO_PG_HOST / ORDO_PG_PORT in the environment) and starts
# nothing.
set -euo pipefail

cd "$(dirname "$0")/.."

if [ "${CI:-}" = "1" ]; then
  # The workflow's Postgres service. Nothing to start or stop here.
  exec cargo test -p ordo-host --test authz_matrix -- --ignored --nocapture
fi

# A cluster of this suite's own (owner's rule for parallel pen-testing: share
# nothing). Its own data directory, socket and port.
export ORDO_PGDATA="${ORDO_PGDATA:-$HOME/.ordo-pg/authz-matrix}"
export ORDO_PG_HOST="${ORDO_PG_HOST:-/tmp/ordo-pg-authz}"
export ORDO_PG_PORT="${ORDO_PG_PORT:-55491}"
export ORDO_PG_USER="${ORDO_PG_USER:-ordo}"

cleanup() {
  ORDO_PGDATA="$ORDO_PGDATA" ORDO_PG_HOST="$ORDO_PG_HOST" ORDO_PG_PORT="$ORDO_PG_PORT" \
    scripts/conformance-pg.sh stop || true
}
trap cleanup EXIT

ORDO_PGDATA="$ORDO_PGDATA" ORDO_PG_HOST="$ORDO_PG_HOST" ORDO_PG_PORT="$ORDO_PG_PORT" \
  scripts/conformance-pg.sh start

cargo test -p ordo-host --test authz_matrix -- --ignored --nocapture
