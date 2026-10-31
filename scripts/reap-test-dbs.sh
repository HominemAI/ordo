#!/usr/bin/env bash
# Sweeps every namespaced test database whose owning process has exited.
#
# `ordo_conformance::Pg::named` (crates/ordo-conformance/src/lib.rs) stamps
# every throwaway test database with `<stem>_<pid>_<nanos>`, so two runs of
# the same suite never collide. It also reaps a stem's own dead-pid siblings
# the next time anything asks for a database under that stem — but a stem a
# developer stops running (a deleted test, a crate nobody's touched in a
# while) never gets asked for again, and its orphan sits forever. This is
# the sweep for that case: it looks at every database on the cluster, not
# just one stem, and drops the ones whose pid is provably gone.
#
# Safe to run any time, including while other suites are mid-run: it only
# ever touches a database whose `<pid>` suffix names a process `ps` says is
# not running, and it skips anything that does not parse as
# `<stem>_<pid>_<hex>` in the first place.
set -euo pipefail

HOST="${ORDO_PG_HOST:-/tmp/ordo-pg}"
PORT="${ORDO_PG_PORT:-55432}"
USER_NAME="${ORDO_PG_USER:-ordo}"
PSQL="${ORDO_PSQL:-/opt/homebrew/opt/postgresql@17/bin/psql}"

psql_admin() {
  "$PSQL" -h "$HOST" -p "$PORT" -U "$USER_NAME" -d postgres -v ON_ERROR_STOP=1 -q "$@"
}

names=$(psql_admin -t -A -c "SELECT datname FROM pg_database WHERE datname ~ '_[0-9]+_[0-9a-f]+\$'")

dropped=0
kept=0
while IFS= read -r name; do
  [ -z "$name" ] && continue
  # <stem>_<pid>_<nanos-hex>: pid is the second-to-last underscore field.
  pid=$(echo "$name" | sed -E 's/^.*_([0-9]+)_[0-9a-f]+$/\1/')
  case "$pid" in
    ''|*[!0-9]*) continue ;;
  esac
  if ps -p "$pid" > /dev/null 2>&1; then
    kept=$((kept + 1))
    continue
  fi
  echo "dropping $name (pid $pid not running)"
  psql_admin -c "DROP DATABASE IF EXISTS \"$name\" WITH (FORCE)"
  dropped=$((dropped + 1))
done <<< "$names"

echo "reaped $dropped orphan(s), left $kept (owning process still running)"
