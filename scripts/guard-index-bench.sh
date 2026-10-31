#!/usr/bin/env bash
# The guard-index benchmark (`crates/ordo-host/tests/notes_app.rs`,
# `my_notes_at_five_thousand_notes_uses_the_access_index`): notes v2 on the
# real binary with 5,000 notes and 5,000 access rows. It prints the plan of
# `myNotes` both ways, the door's read time and a device sync's load time.
#
# On a cluster of its own (data directory, socket, port), stopped and
# deleted afterwards unless ORDO_PGDATA names one to keep.
set -euo pipefail

cd "$(dirname "$0")/.."

keep="${ORDO_PGDATA:+yes}"
export ORDO_PGDATA="${ORDO_PGDATA:-$HOME/.ordo-pg/guard-index}"
export ORDO_PG_HOST="${ORDO_PG_HOST:-/tmp/ordo-pg-guardix}"
export ORDO_PG_PORT="${ORDO_PG_PORT:-55511}"
export ORDO_PG_USER="${ORDO_PG_USER:-ordo}"

cleanup() {
  if [ -z "$keep" ]; then
    scripts/conformance-pg.sh destroy || true
  fi
}
trap cleanup EXIT

if ! "${ORDO_PGBIN:-/opt/homebrew/opt/postgresql@17/bin}/pg_isready" -h "$ORDO_PG_HOST" -p "$ORDO_PG_PORT" -q 2>/dev/null; then
  scripts/conformance-pg.sh start
fi
cargo test -p ordo-host --test notes_app my_notes_at_five_thousand_notes -- --ignored --nocapture
