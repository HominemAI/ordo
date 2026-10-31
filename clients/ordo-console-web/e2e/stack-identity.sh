#!/usr/bin/env bash
# The throwaway console stack on the identity service, for the console's
# end-to-end tests of identity mode (docs/plan-identity-service.md 5.4).
#
#     bash clients/ordo-console-web/e2e/stack-identity.sh
#
# Boots, in order, and stays in the foreground until it is stopped:
#
#   1. Postgres: a throwaway cluster of its own, unless ORDO_PG_HOST names
#      one already running (CI's service container). Its databases are
#      made fresh: the identity service's, the console's, the door's app.
#   2. The real `ordo-identity-service` on http://localhost:$E2E_ID_PORT,
#      relying party `localhost`, with the console listed as a client. Its
#      first start writes the first invite to its data folder.
#   3. The deployment's door (examples/e2e_door.rs) verifying the service's
#      tokens, with a short touch window (E2E_DOOR_TOUCH_SECS).
#   4. The real `ordo-console` in door-only mode with ORDO_IDENTITY_URL set:
#      people sign in at the service; the console calls the door as them.
#
# Playwright starts it (playwright.config.ts, the second `webServer`). It
# writes what tests need to $E2E_STATE/stack.json. Every value is a test value.
# Needs: cargo build -p ordo-console -p ordo-identity-service, and
# cargo build -p ordo-console --example e2e_door.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
web=$(cd "$here/.." && pwd)
repo=$(cd "$here/../../.." && pwd)

console_bin=${ORDO_CONSOLE_BIN:-"$repo/target/debug/ordo-console"}
id_bin=${E2E_IDENTITY_BIN:-"$repo/target/debug/ordo-identity-service"}
door_bin=${E2E_DOOR_BIN:-"$repo/target/debug/examples/e2e_door"}
state=${E2E_ID_STATE:-"$here/.stack-identity"}
port=${E2E_ID_CONSOLE_PORT:-4683}
door_port=${E2E_ID_DOOR_PORT:-4684}
id_port=${E2E_ID_PORT:-4685}
origin="http://localhost:$port"
id_origin="http://localhost:$id_port"

for bin in "$console_bin" "$id_bin" "$door_bin"; do
  [ -x "$bin" ] || { echo "stack-identity: missing $bin" >&2; exit 1; }
done
[ -d "$web/node_modules" ] || { echo "stack-identity: run npm ci in $web first" >&2; exit 1; }
for p in "$port" "$door_port" "$id_port"; do
  if (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then
    echo "stack-identity: port $p is in use" >&2
    exit 1
  fi
done

rm -rf "$state"
mkdir -p "$state/console-data" "$state/id-data"
pids=''
own_pg=0

stop() {
  for p in $pids; do kill "$p" 2>/dev/null || true; done
  wait 2>/dev/null || true
  if [ "$own_pg" = 1 ]; then
    ORDO_PGDATA="$pgdata" ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" \
      bash "$repo/scripts/conformance-pg.sh" destroy >/dev/null 2>&1 || true
  fi
}
trap stop EXIT
trap 'exit 130' INT TERM

# ── 1. Postgres ────────────────────────────────────────────────────────────
if [ -n "${ORDO_PG_HOST:-}" ]; then
  pg_host=$ORDO_PG_HOST
  pg_port=${ORDO_PG_PORT:-5432}
  pg_user=${ORDO_PG_USER:-ordo}
else
  own_pg=1
  pg_port=${E2E_ID_PG_PORT:-55455}
  pg_user=ordo
  socket=/tmp/ordo-console-id-e2e-pg
  pgdata="${TMPDIR:-/tmp}/ordo-console-id-e2e-pgdata-$pg_port"
  ORDO_PGDATA="$pgdata" ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" \
    bash "$repo/scripts/conformance-pg.sh" destroy >/dev/null 2>&1 || true
  ORDO_PGDATA="$pgdata" ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" ORDO_PG_DB=postgres \
    bash "$repo/scripts/conformance-pg.sh" start >"$state/pg.log" 2>&1
  pg_host=$socket
fi
export PGPASSWORD=${ORDO_PG_PASSWORD:-}
psql_bin=${ORDO_PSQL:-$( [ -n "${ORDO_PGBIN:-}" ] && echo "$ORDO_PGBIN/psql" || command -v psql || echo /opt/homebrew/opt/postgresql@17/bin/psql)}
pw=''
[ -n "${ORDO_PG_PASSWORD:-}" ] && pw=" password=$ORDO_PG_PASSWORD"
for db in ordo_console_id_e2e ordo_console_id_e2e_app ordo_console_id_e2e_identity; do
  "$psql_bin" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d postgres -qc "drop database if exists \"$db\" with (force)"
  "$psql_bin" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d postgres -qc "create database \"$db\""
done
console_url="host=$pg_host port=$pg_port user=$pg_user dbname=ordo_console_id_e2e$pw"
id_url="host=$pg_host port=$pg_port user=$pg_user dbname=ordo_console_id_e2e_identity$pw"

# ── The page ──────────────────────────────────────────────────────────────
(cd "$web" && node_modules/.bin/vite build --outDir "$state/page" --emptyOutDir) >"$state/page-build.log" 2>&1 ||
  { cat "$state/page-build.log" >&2; echo "stack-identity: the page did not build" >&2; exit 1; }

# ── 2. The identity service ───────────────────────────────────────────────
(
  for v in $(compgen -e | grep '^ORDO_' || true); do unset "$v"; done
  export ORDO_IDENTITY_DATABASE_URL="$id_url"
  export ORDO_IDENTITY_ORIGIN="$id_origin"
  export ORDO_IDENTITY_LISTEN="127.0.0.1:$id_port"
  export ORDO_IDENTITY_DATA_DIR="$state/id-data"
  export ORDO_IDENTITY_CLIENT_ORIGINS="$origin"
  export ORDO_IDENTITY_DEPLOYMENT=main
  exec "$id_bin"
) >"$state/identity.log" 2>&1 &
pids="$pids $!"
tries=0
until curl -fsS "$id_origin/health/ready" >/dev/null 2>&1; do
  tries=$((tries + 1))
  [ "$tries" -lt 600 ] || { cat "$state/identity.log" >&2; echo "stack-identity: the identity service did not answer" >&2; exit 1; }
  sleep 0.2
done

# ── 3. The door ───────────────────────────────────────────────────────────
E2E_DOOR_ADDR="127.0.0.1:$door_port" E2E_DOOR_ORIGIN="$origin" E2E_DOOR_TOUCH_SECS=8 \
  ORDO_IDENTITY_URL="http://127.0.0.1:$id_port" ORDO_IDENTITY_ORIGIN="$id_origin" ORDO_IDENTITY_DEPLOYMENT=main \
  ORDO_PG_HOST="$pg_host" ORDO_PG_PORT="$pg_port" ORDO_PG_USER="$pg_user" ORDO_PG_DB=ordo_console_id_e2e_app \
  "$door_bin" >"$state/door.log" 2>&1 &
pids="$pids $!"

# ── 4. The console ────────────────────────────────────────────────────────
(
  for v in $(compgen -e | grep '^ORDO_' || true); do unset "$v"; done
  export ORDO_CONSOLE_DATABASE_URL="$console_url"
  export ORDO_CONSOLE_ORIGIN="$origin"
  export ORDO_CONSOLE_LISTEN="127.0.0.1:$port"
  export ORDO_CONSOLE_DATA_DIR="$state/console-data"
  export ORDO_CONSOLE_PAGE_DIR="$state/page"
  export ORDO_CONSOLE_DEPLOYMENT=main
  export ORDO_CONSOLE_DOOR_URL="http://127.0.0.1:$door_port"
  export ORDO_CONSOLE_DOOR_SPACE=home
  export ORDO_IDENTITY_URL="http://127.0.0.1:$id_port"
  export ORDO_IDENTITY_ORIGIN="$id_origin"
  exec "$console_bin"
) >"$state/console.log" 2>&1 &
last=$!
pids="$pids $last"

cat >"$state/stack.json" <<JSON
{
  "console": "$origin",
  "identity": "$id_origin",
  "identityData": "$state/id-data",
  "identityBin": "$id_bin",
  "identityDatabaseUrl": "$id_url",
  "logs": "$state"
}
JSON

tries=0
until curl -fsS "$origin/health/ready" >/dev/null 2>&1; do
  kill -0 "$last" 2>/dev/null || { cat "$state/console.log" >&2; echo "stack-identity: the console stopped" >&2; exit 1; }
  tries=$((tries + 1))
  [ "$tries" -lt 600 ] || { echo "stack-identity: the console did not answer in 2 minutes" >&2; exit 1; }
  sleep 0.2
done
echo "stack-identity: up. Console $origin/d/main/, identity $id_origin, logs in $state."
# shellcheck disable=SC2086
wait $pids
