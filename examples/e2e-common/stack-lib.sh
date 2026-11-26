#!/usr/bin/env bash
# Source with repo, state and db set by the example's stack.
: "${repo:?}" "${state:?}" "${db:?}" "${host:?}" "${door:?}"
pids=''
own_pg=0
stop() {
  for p in $pids; do kill -TERM "$p" 2>/dev/null || true; done
  for p in $pids; do wait "$p" 2>/dev/null || true; done
  if [ "$own_pg" = 1 ]; then
    ORDO_PGDATA="$pgdata" ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" \
      bash "$repo/scripts/conformance-pg.sh" destroy >/dev/null 2>&1 || true
    rmdir "$socket" 2>/dev/null || true
  fi
}
trap stop EXIT
trap 'exit 130' INT TERM
wait_for() {
  local what=$1 url=$2 pid=$3 tries=0
  until curl -fsSk "$url" >/dev/null 2>&1; do
    kill -0 "$pid" 2>/dev/null || { echo "stack: $what stopped; see $state" >&2; exit 1; }
    tries=$((tries + 1))
    [ "$tries" -lt 300 ] || { echo "stack: $what did not answer $url" >&2; exit 1; }
    sleep 0.2
  done
}
start_pg() {
  [[ "$db" =~ ^[a-z][a-z0-9_]*$ ]] || { echo 'Invalid test database name' >&2; exit 1; }
  if [ -n "${ORDO_PG_HOST:-}" ]; then
    pg_host=$ORDO_PG_HOST pg_port=${ORDO_PG_PORT:-5432} pg_user=${ORDO_PG_USER:-ordo}
  else
    own_pg=1 pg_port=${E2E_PG_PORT:-55462} pg_user=ordo
    socket=$(mktemp -d /tmp/ordo-e2e-pg.XXXXXX)
    pgdata="$state/pg"
    ORDO_PGDATA="$pgdata" ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" ORDO_PG_DB=postgres \
      bash "$repo/scripts/conformance-pg.sh" start >"$state/pg.log" 2>&1
    pg_host=$socket
  fi
  export PGPASSWORD=${ORDO_PG_PASSWORD:-}
  psql_bin=${ORDO_PSQL:-$(command -v psql || echo "${ORDO_PGBIN:-/opt/homebrew/opt/postgresql@17/bin}/psql")}
  "$psql_bin" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d postgres -qc "create database \"$db\""
  export ORDO_PG_HOST="$pg_host" ORDO_PG_PORT="$pg_port" ORDO_PG_USER="$pg_user" ORDO_PG_DB="$db"
}
start_door() {
  "$host" >"$state/door.log" 2>&1 &
  last=$!
  pids="$pids $last"
  printf '%s\n' "$last" >> "$state/host-pids"
  wait_for ordo-host "$door/health/live" "$last"
}
