#!/usr/bin/env bash
# The throwaway console stack, for the console's end-to-end tests.
#
#     bash clients/ordo-console-web/e2e/stack.sh
#
# Boots, in order, and stays in the foreground until it is stopped:
#
#   1. Postgres. A throwaway cluster of its own (scripts/conformance-pg.sh),
#      unless ORDO_PG_HOST names one already running (CI's service
#      container). Either way the console's database is made fresh.
#   2. The page, built by Vite into the stack's folder.
#   3. The real `ordo-supervisor`, running `ordo-supervisor-fake-child` as
#      the deployment: a stand-in for `ordo-host` whose settings say how to
#      behave (ORDO_FAKE_MODE, ORDO_FAKE_CHECK;
#      crates/ordo-supervisor/src/bin/fake_child.rs). Its identity service
#      and console children are the same stand-in, idle on spare ports.
#      Then the real `ordo-console`, beside it rather than under it, so it
#      keeps its own sign-in for these tests: it serves that page on its own
#      origin, with its data folder in the stack's folder (the setup code
#      lands there), and manages the deployment through the supervisor's
#      socket, as in the one image. The identity stack is stack-identity.sh.
#   4. The deployment's door, for the link screens: `ordo-host`'s own
#      sign-in routes and console key ops over memory stores
#      (crates/ordo-console/examples/e2e_door.rs), and the deploy ops over
#      a fresh database of their own (<db>_app). The console reaches it
#      through ORDO_CONSOLE_DOOR_URL; its passkeys share the relying party
#      `localhost`, with the console's origin listed.
#
# Playwright starts this script itself (playwright.config.ts, `webServer`)
# and stops it at the end. It writes what tests need to $E2E_STATE/stack.json.
#
# Needs: `cargo build -p ordo-console -p ordo-cli -p ordo-supervisor` and
# `cargo build -p ordo-console --example e2e_door` done (or ORDO_CONSOLE_BIN,
# E2E_SUPERVISOR_BIN, E2E_HOST_BIN, E2E_DOOR_BIN and ORDO naming the
# binaries), `npm ci` in clients/ordo-console-web, and
# PostgreSQL 15 or later binaries (ORDO_PGBIN) when no ORDO_PG_HOST is given.
# Every value below is a test value.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
web=$(cd "$here/.." && pwd)
repo=$(cd "$here/../../.." && pwd)

console_bin=${ORDO_CONSOLE_BIN:-"$repo/target/debug/ordo-console"}
supervisor_bin=${E2E_SUPERVISOR_BIN:-"$repo/target/debug/ordo-supervisor"}
host_bin=${E2E_HOST_BIN:-"$repo/target/debug/ordo-supervisor-fake-child"}
door_bin=${E2E_DOOR_BIN:-"$repo/target/debug/examples/e2e_door"}
ordo=${ORDO:-"$repo/target/debug/ordo"}
state=${E2E_STATE:-"$here/.stack"}
port=${E2E_CONSOLE_PORT:-4680}
host_port=${E2E_HOST_PORT:-4681}
door_port=${E2E_DOOR_PORT:-4682}
# `localhost`, not 127.0.0.1: the passkey relying party is `localhost`.
origin="http://localhost:$port"

for bin in "$console_bin" "$ordo" "$supervisor_bin" "$host_bin" "$door_bin"; do
  [ -x "$bin" ] || { echo "stack: missing $bin: run cargo build -p ordo-console -p ordo-cli -p ordo-supervisor and cargo build -p ordo-console --example e2e_door" >&2; exit 1; }
done
[ -d "$web/node_modules" ] || { echo "stack: run npm ci in $web first" >&2; exit 1; }

tries=0
while (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; do
  tries=$((tries + 1))
  [ "$tries" -lt 100 ] || { echo "stack: port $port is in use; stop what holds it or set E2E_CONSOLE_PORT" >&2; exit 1; }
  sleep 0.2
done
if (exec 3<>"/dev/tcp/127.0.0.1/$host_port") 2>/dev/null; then
  echo "stack: port $host_port is in use; stop what holds it or set E2E_HOST_PORT" >&2
  exit 1
fi
if (exec 3<>"/dev/tcp/127.0.0.1/$door_port") 2>/dev/null; then
  echo "stack: port $door_port is in use; stop what holds it or set E2E_DOOR_PORT" >&2
  exit 1
fi

rm -rf "$state"
mkdir -p "$state/data" "$state/run"
pids=''
own_pg=0

stop() {
  # Postgres's view while the door still runs, beside its log: what each
  # connection waits on and who blocks it (store-pool-held-blocks-signin).
  if [ -n "${pg_host:-}" ]; then
    PGHOST="$pg_host" PGPORT="$pg_port" PGUSER="$pg_user" ORDO_PSQL="${psql_bin:-}" \
      bash "$repo/scripts/pg-snapshot.sh" "$state/pg-at-stop.log" "stack stopping" || true
  fi
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
db=${E2E_PG_DB:-ordo_console_e2e}
if [ -n "${ORDO_PG_HOST:-}" ]; then
  pg_host=$ORDO_PG_HOST
  pg_port=${ORDO_PG_PORT:-5432}
  pg_user=${ORDO_PG_USER:-ordo}
else
  own_pg=1
  pg_port=${E2E_PG_PORT:-55454}
  pg_user=ordo
  # A Unix socket path is limited to about 100 bytes, so it is not under $state.
  socket=/tmp/ordo-console-e2e-pg
  pgdata="${TMPDIR:-/tmp}/ordo-console-e2e-pgdata-$pg_port"
  ORDO_PGDATA="$pgdata" ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" \
    bash "$repo/scripts/conformance-pg.sh" destroy >/dev/null 2>&1 || true
  ORDO_PGDATA="$pgdata" ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" ORDO_PG_DB=postgres \
    bash "$repo/scripts/conformance-pg.sh" start >"$state/pg.log" 2>&1
  pg_host=$socket
fi
export PGPASSWORD=${ORDO_PG_PASSWORD:-}
psql_bin=${ORDO_PSQL:-$( [ -n "${ORDO_PGBIN:-}" ] && echo "$ORDO_PGBIN/psql" || command -v psql || echo /opt/homebrew/opt/postgresql@17/bin/psql)}
"$psql_bin" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d postgres -qc "drop database if exists \"$db\" with (force)"
"$psql_bin" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d postgres -qc "create database \"$db\""
# The app's own database, which the door's deploy ops apply programs to.
app_db="${db}_app"
"$psql_bin" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d postgres -qc "drop database if exists \"$app_db\" with (force)"
"$psql_bin" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d postgres -qc "create database \"$app_db\""
url="host=$pg_host port=$pg_port user=$pg_user dbname=$db"
[ -n "${ORDO_PG_PASSWORD:-}" ] && url="$url password=$ORDO_PG_PASSWORD"

# ── 2. The page ────────────────────────────────────────────────────────────
(cd "$web" && node_modules/.bin/vite build --outDir "$state/page" --emptyOutDir) >"$state/page-build.log" 2>&1 ||
  { cat "$state/page-build.log" >&2; echo "stack: the page did not build" >&2; exit 1; }

# ── 4. The deployment's door (started first: the console reaches it) ───────
# Its std.deployment.settings reads the stand-in host's environment file,
# and holds guest sign-in stored `off`, which wins over the variable.
E2E_DOOR_ADDR="127.0.0.1:$door_port" E2E_DOOR_ORIGIN="$origin" \
  E2E_DOOR_CHILD_ENV="$state/child.env" E2E_DOOR_STORED="auth_guest=off" \
  ORDO_PG_HOST="$pg_host" ORDO_PG_PORT="$pg_port" ORDO_PG_USER="$pg_user" ORDO_PG_DB="$app_db" \
  "$door_bin" >"$state/door.log" 2>&1 &
pids="$pids $!"

# ── 3. The supervisor, then the console ────────────────────────────────────
# The supervisor builds the deployment's revision 1 from every ORDO_*
# variable that is no other child's and not its own, so all of them are
# cleared (CI sets ORDO_PG_PASSWORD, ORDO_PSQL and more), and only the fake
# host's own go in. Clearing the deployment's database settings also leaves
# the console's same-database refusal nothing to compare against.
for v in $(compgen -e | grep '^ORDO_' || true); do unset "$v"; done
spare_console=$((port + 10))
spare_identity=$((port + 11))
(
  export ORDO_SUPERVISOR_IDENTITY_BIN="$host_bin"
  export ORDO_SUPERVISOR_CONSOLE_BIN="$host_bin"
  export ORDO_SUPERVISOR_HOST_BIN="$host_bin"
  export ORDO_SUPERVISOR_STATE_DIR="$state/supervisor"
  export ORDO_SUPERVISOR_RUN_DIR="$state/run"
  export ORDO_SUPERVISOR_LISTEN="127.0.0.1:$((port + 12))"
  # Short, so a failed apply rolls back in seconds, not minutes.
  export ORDO_SUPERVISOR_GRACE_SECS=1
  export ORDO_SUPERVISOR_READY_SECS=5
  export ORDO_CONSOLE_LISTEN="127.0.0.1:$spare_console"
  export ORDO_IDENTITY_LISTEN="127.0.0.1:$spare_identity"
  export ORDO_HOST_ADDR="127.0.0.1:$host_port"
  export ORDO_FAKE_STARTS="$state/starts"
  export ORDO_FAKE_ENV_OUT="$state/child.env"
  # The door reads that file: names and paths, never a secret's content.
  export ORDO_FAKE_ENV_PLAIN=1
  exec "$supervisor_bin"
) >"$state/supervisor.log" 2>&1 &
pids="$pids $!"
tries=0
until [ -S "$state/run/supervisor.sock" ]; do
  tries=$((tries + 1))
  [ "$tries" -lt 300 ] || { cat "$state/supervisor.log" >&2; echo "stack: the supervisor did not open its socket" >&2; exit 1; }
  sleep 0.2
done

export ORDO_CONSOLE_DATABASE_URL="$url"
export ORDO_CONSOLE_ORIGIN="$origin"
export ORDO_CONSOLE_LISTEN="127.0.0.1:$port"
export ORDO_CONSOLE_DATA_DIR="$state/data"
export ORDO_CONSOLE_PAGE_DIR="$state/page"
export ORDO_CONSOLE_DEPLOYMENT=main
export ORDO_CONSOLE_SUPERVISOR_SOCKET="$state/run/supervisor.sock"
export ORDO_CONSOLE_DOOR_URL="http://127.0.0.1:$door_port"
# The door's own space (examples/e2e_door.rs).
export ORDO_CONSOLE_DOOR_SPACE=home
"$console_bin" >"$state/console.log" 2>&1 &
last=$!
pids="$pids $last"

cat >"$state/stack.json" <<JSON
{
  "console": "$origin",
  "data": "$state/data",
  "databaseUrl": "$url",
  "ordo": "$ordo",
  "host": "http://127.0.0.1:$host_port",
  "door": "http://127.0.0.1:$door_port",
  "starts": "$state/starts",
  "logs": "$state"
}
JSON

tries=0
until curl -fsS "$origin/health/ready" >/dev/null 2>&1; do
  kill -0 "$last" 2>/dev/null || { cat "$state/console.log" >&2; echo "stack: the console stopped before it answered" >&2; exit 1; }
  tries=$((tries + 1))
  [ "$tries" -lt 600 ] || { echo "stack: the console did not answer in 2 minutes" >&2; exit 1; }
  sleep 0.2
done
echo "stack: up. Console $origin/d/main/, logs in $state. Ctrl-C stops everything."
# shellcheck disable=SC2086
wait $pids
