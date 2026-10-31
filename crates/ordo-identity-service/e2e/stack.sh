#!/usr/bin/env bash
# The throwaway identity service, for the page's end-to-end tests.
#
#     bash crates/ordo-identity-service/e2e/stack.sh
#
# Boots, and stays in the foreground until stopped:
#
#   1. Postgres: a throwaway cluster of its own (scripts/conformance-pg.sh)
#      unless ORDO_PG_HOST names one already running. Either way the
#      service's database is made fresh.
#   2. The real `ordo-identity-service` on http://localhost:$E2E_IDENTITY_PORT,
#      relying party `localhost`, its data folder in the stack's folder. Its
#      first start writes the first invite there (`first-invite`).
#
# Playwright starts this script itself (playwright.config.ts, `webServer`)
# and stops it at the end. It writes what tests need to $E2E_STATE/stack.json.
# Needs `cargo build -p ordo-identity-service` done (or E2E_IDENTITY_BIN
# naming the binary). Every value below is a test value.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)

bin=${E2E_IDENTITY_BIN:-"$repo/target/debug/ordo-identity-service"}
state=${E2E_STATE:-"$here/.stack"}
port=${E2E_IDENTITY_PORT:-4690}
console_port=${E2E_CONSOLE_PORT:-4691}
origin="http://localhost:$port"

[ -x "$bin" ] || { echo "stack: missing $bin: run cargo build -p ordo-identity-service" >&2; exit 1; }
if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
  echo "stack: port $port is in use; stop what holds it or set E2E_IDENTITY_PORT" >&2
  exit 1
fi

rm -rf "$state"
mkdir -p "$state/data"
own_pg=0
pid=''

stop() {
  [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  if [ "$own_pg" = 1 ]; then
    ORDO_PGDATA="$state/pg" ORDO_PG_HOST="$pg_host" ORDO_PG_PORT="$pg_port" \
      bash "$repo/scripts/conformance-pg.sh" destroy >/dev/null 2>&1 || true
  fi
}
trap stop EXIT INT TERM

pg_user=${ORDO_PG_USER:-ordo}
if [ -n "${ORDO_PG_HOST:-}" ]; then
  pg_host=$ORDO_PG_HOST
  pg_port=${ORDO_PG_PORT:-5432}
else
  own_pg=1
  pg_host="/tmp/ordo-id-e2e-$$"
  pg_port=${E2E_PG_PORT:-55493}
  ORDO_PGDATA="$state/pg" ORDO_PG_HOST="$pg_host" ORDO_PG_PORT="$pg_port" ORDO_PG_USER="$pg_user" \
    bash "$repo/scripts/conformance-pg.sh" start >/dev/null
fi

pgbin=${ORDO_PGBIN:-/opt/homebrew/opt/postgresql@17/bin}
psql=${ORDO_PSQL:-"$pgbin/psql"}
command -v "$psql" >/dev/null 2>&1 || psql=psql
db=ordo_identity_e2e
PGPASSWORD=${ORDO_PG_PASSWORD:-} "$psql" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d postgres -q \
  -c "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db" >/dev/null

url="host=$pg_host port=$pg_port user=$pg_user dbname=$db"
[ -n "${ORDO_PG_PASSWORD:-}" ] && url="$url password=$ORDO_PG_PASSWORD"

export ORDO_IDENTITY_DATABASE_URL="$url"
export ORDO_IDENTITY_ORIGIN="$origin"
export ORDO_IDENTITY_LISTEN="127.0.0.1:$port"
export ORDO_IDENTITY_DATA_DIR="$state/data"
export ORDO_IDENTITY_CLIENT_ORIGINS="http://localhost:$console_port"

cat > "$state/stack.json" <<JSON
{
  "origin": "$origin",
  "console": "http://localhost:$console_port",
  "data": "$state/data",
  "bin": "$bin",
  "env": {
    "ORDO_IDENTITY_DATABASE_URL": "$url",
    "ORDO_IDENTITY_ORIGIN": "$origin",
    "ORDO_IDENTITY_DATA_DIR": "$state/data",
    "ORDO_IDENTITY_CLIENT_ORIGINS": "http://localhost:$console_port"
  }
}
JSON

"$bin" > "$state/identity.log" 2>&1 &
pid=$!
wait "$pid"
