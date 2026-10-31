#!/usr/bin/env bash
# The Ordo bench stack: one app workspace, served on this machine.
#
#     bash up.sh <workspace> [state-dir]
#
# The workspace holds `backend/` (the program, and an optional
# `backend/deploy.env`) and `web/` (a Vite React page). This script starts,
# in order, and stays in the foreground until it is stopped:
#
#   1. A throwaway Postgres cluster of its own.
#   2. A mail catcher (mail-catcher.mjs), so emailed codes can be read.
#   3. `ordo-host`: the bench's settings, then `backend/deploy.env`, then the
#      bench's settings again, so the bench's always win.
#   4. `ordo apply backend`.
#   5. The page: `vite build`, then `vite preview` on http://localhost:5310.
#
# When the page answers, the stack is up. It writes <state>/stack.json.
#
# The bench owns the machine settings (database, keys, ports, origins, mail,
# and the one way in: an emailed code). `deploy.env` owns the app's choices:
# who gets a team, which platform ops skip a passkey touch, the access
# template. Every value here is a test value. Nothing is fit for a real
# deployment.
#
# Needs: Node 20 or later, PostgreSQL 15 or later binaries (ORDO_PGBIN), and
# the `ordo` and `ordo-host` binaries (ORDO and ORDO_HOST_BIN, or on PATH).
# BENCH_NO_WEB=1 skips the page, for working on the program alone.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
ws=$(cd "${1:?usage: up.sh <workspace> [state-dir]}" && pwd)
state=${2:-"$ws/.stack"}

ordo=${ORDO:-$(command -v ordo || true)}
host=${ORDO_HOST_BIN:-$(command -v ordo-host || true)}
pgbin=${ORDO_PGBIN:-/opt/homebrew/opt/postgresql@17/bin}
catcher=${BENCH_MAIL_CATCHER:-"$here/mail-catcher.mjs"}

door_port=${BENCH_DOOR_PORT:-4710}
page_port=${BENCH_PAGE_PORT:-5310}
smtp_port=${BENCH_MAIL_SMTP_PORT:-4725}
mail_port=${BENCH_MAIL_HTTP_PORT:-4726}
pg_port=${BENCH_PG_PORT:-55510}

door="http://127.0.0.1:$door_port"
# The page reaches the door by `localhost`, its own site, so the renewal
# cookie is sent and a reload stays signed in.
page_door="http://localhost:$door_port"
page="http://localhost:$page_port"

for bin in "$ordo" "$host"; do
  [ -n "$bin" ] && [ -x "$bin" ] || { echo "stack: set ORDO and ORDO_HOST_BIN to the ordo and ordo-host binaries" >&2; exit 1; }
done
[ -d "$ws/backend" ] || { echo "stack: $ws/backend is missing" >&2; exit 1; }

for port in "$door_port" "$page_port" "$smtp_port" "$mail_port" "$pg_port"; do
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "stack: port $port is in use; stop what holds it" >&2
    exit 1
  fi
done

rm -rf "$state"
mkdir -p "$state"
pgdata="$state/pg"
# A Unix socket path is limited to about 100 bytes, so it is not under $state.
socket=$(mktemp -d "${TMPDIR:-/tmp}/bp.XXXXXX")
pids=''

stop() {
  for p in $pids; do kill "$p" 2>/dev/null || true; done
  wait 2>/dev/null || true
  "$pgbin/pg_ctl" -D "$pgdata" stop -m immediate >/dev/null 2>&1 || true
  rm -rf "$socket"
}
trap stop EXIT
trap 'exit 130' INT TERM

wait_for() { # what, url, pid
  local what=$1 url=$2 pid=$3 tries=0
  until curl -fsS "$url" >/dev/null 2>&1; do
    kill -0 "$pid" 2>/dev/null || { echo "stack: $what stopped before it answered (logs in $state)" >&2; exit 1; }
    tries=$((tries + 1))
    [ "$tries" -lt 600 ] || { echo "stack: $what did not answer $url in 2 minutes" >&2; exit 1; }
    sleep 0.2
  done
}

# ── 1. Postgres ────────────────────────────────────────────────────────────
"$pgbin/initdb" -D "$pgdata" -U ordo --auth=trust >"$state/initdb.log" 2>&1
"$pgbin/pg_ctl" -D "$pgdata" -o "-p $pg_port -k $socket -h ''" -l "$state/pg.log" start >/dev/null
until "$pgbin/pg_isready" -h "$socket" -p "$pg_port" -q; do sleep 0.2; done
"$pgbin/createdb" -h "$socket" -p "$pg_port" -U ordo bench

# ── 2. The mail catcher ────────────────────────────────────────────────────
BENCH_MAIL_SMTP_PORT="$smtp_port" BENCH_MAIL_HTTP_PORT="$mail_port" node "$catcher" >"$state/mail.log" 2>&1 &
last=$!
pids="$pids $last"
wait_for "the mail catcher" "http://127.0.0.1:$mail_port/health" "$last"

# ── 3. The door ────────────────────────────────────────────────────────────
bench_settings() {
  export ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" ORDO_PG_USER=ordo ORDO_PG_DB=bench
  export ORDO_SPACE=system
  export ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY=bench-master-key-for-tests ORDO_DEV_UNSAFE_TEST_KEYS=1
  export ORDO_TOKEN_KEY=7777777777777777777777777777777777777777777777777777777777777777
  export ORDO_PAGE_KEY=8888888888888888888888888888888888888888888888888888888888888888
  export ORDO_HOST_ADDR="127.0.0.1:$door_port"
  export ORDO_AUTH_METHODS=email
  export ORDO_APEX_DOMAIN=localhost ORDO_AUTH_ORIGINS="$page" ORDO_CORS_ORIGINS="$page"
  export ORDO_PROVISION_SIGNUPS_PER_HOUR=10000
  export ORDO_MAIL_KIND=smtp ORDO_MAIL_FROM=bench@example.com ORDO_MAIL_HOST=localhost
  export ORDO_MAIL_PORT="$smtp_port" ORDO_MAIL_TLS=none ORDO_MAIL_SEND_SECS=1
  export ORDO_EGRESS_PRIVATE_HOSTS=localhost
  export ORDO_PG_POOL_SIZE=${ORDO_PG_POOL_SIZE:-3}
}
bench_settings
if [ -f "$ws/backend/deploy.env" ]; then
  # Only ORDO_* lines, KEY=VALUE, no shell. Quotes around the value are dropped.
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in ''|'#'*) continue ;; esac
    key=${line%%=*}
    val=${line#*=}
    val=${val#\"}; val=${val%\"}
    case "$key" in
      ORDO_[A-Z0-9_]*) export "$key=$val" ;;
      *) echo "stack: deploy.env: skipped a line that does not set an ORDO_ variable: $key" >&2 ;;
    esac
  done <"$ws/backend/deploy.env"
  bench_settings
fi
for v in ORDO_PG_ROUTING ORDO_TENANT_CREDENTIALS ORDO_TENANT_CREDENTIALS_DIR ORDO_DEPLOYMENT_OWNERS; do unset "$v"; done
env | grep '^ORDO_' | grep -v 'KEY=' | sort >"$state/door.env"

RUST_LOG=${RUST_LOG:-info} "$host" >"$state/door.log" 2>&1 &
last=$!
pids="$pids $last"
wait_for "ordo-host" "$door/health/live" "$last"

"$ordo" apply "$ws/backend" --as bench >"$state/apply.log" 2>&1 ||
  { cat "$state/apply.log" >&2; echo "stack: ordo apply failed" >&2; exit 1; }

# ── 4. The page ────────────────────────────────────────────────────────────
if [ "${BENCH_NO_WEB:-}" != 1 ]; then
  [ -d "$ws/web/node_modules" ] || (cd "$ws/web" && npm install --no-audit --no-fund) >"$state/npm.log" 2>&1 ||
    { tail -30 "$state/npm.log" >&2; echo "stack: npm install failed in web/" >&2; exit 1; }
  (cd "$ws/web" && VITE_BACKEND_URL="$page_door" npx --no-install vite build --outDir "$state/page" --emptyOutDir) >"$state/page-build.log" 2>&1 ||
    { tail -40 "$state/page-build.log" >&2; echo "stack: the page did not build" >&2; exit 1; }
  (cd "$ws/web" && exec npx --no-install vite preview --outDir "$state/page" --host localhost --port "$page_port" --strictPort) \
    >"$state/page.log" 2>&1 &
  last=$!
  pids="$pids $last"
fi

cat >"$state/stack.json" <<EOF
{
  "stack": "ordo",
  "door": "$door",
  "page": "$page",
  "mail": "http://127.0.0.1:$mail_port",
  "logs": "$state"
}
EOF

[ "${BENCH_NO_WEB:-}" = 1 ] || wait_for "the page" "$page/" "$last"
echo "stack: up. Page $page, door $door, mail http://127.0.0.1:$mail_port/messages"
echo "stack: logs in $state. Ctrl-C stops everything."
wait $pids
