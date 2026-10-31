#!/usr/bin/env bash
# The throwaway notes stack, for the end-to-end tests.
#
#     bash examples/notes/e2e/stack.sh
#
# Boots, in order, and stays in the foreground until it is stopped:
#
#   1. Postgres. A throwaway cluster of its own (scripts/conformance-pg.sh),
#      unless ORDO_PG_HOST names one already running (CI's service
#      container). Either way the database `$E2E_PG_DB` is dropped and made
#      fresh.
#   2. The stand-ins (stand-ins.mjs): a stand-in Discord over https, the
#      door's https callback address, and a mail catcher.
#   3. The real `ordo-host`, set as examples/notes/README.md §6 says a
#      deployment is: passkey, emailed code, Discord and guest sign-in;
#      auto-onboard; a tenant of one's own on sign-up; CORS for the page.
#   4. `ordo apply` of the program ($E2E_PROGRAM, examples/notes by default).
#   5. The notes page, built with Vite and served by `vite preview`. The
#      notes v2 page (examples/notes-v2/web) has its own build step and is
#      served by the same server under /v2/, so it is the same origin and a
#      person's stored sign-in and device keys carry over. Only the upgrade
#      test opens it, after it applies examples/notes-v2 over the running v1.
#
# When the page answers, everything behind it is up. Playwright starts this
# script itself (playwright.config.ts, `webServer`) and stops it at the end.
# It writes what tests need to know to $E2E_STATE/stack.json.
#
# Needs: `cargo build -p ordo-cli -p ordo-host` done (or ORDO and
# ORDO_HOST_BIN naming the binaries), `npm ci` in examples/notes/web, Node 20
# or later, and PostgreSQL 15 or later binaries (ORDO_PGBIN) when no
# ORDO_PG_HOST is given. Every value below is a test value.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
web="$repo/examples/notes/web"
web2="$repo/examples/notes-v2/web"

ordo=${ORDO:-"$repo/target/debug/ordo"}
host=${ORDO_HOST_BIN:-"$repo/target/debug/ordo-host"}
state=${E2E_STATE:-"$here/.stack"}
program=${E2E_PROGRAM:-"$repo/examples/notes"}

door_port=${E2E_DOOR_PORT:-4640}
page_port=${E2E_PAGE_PORT:-5190}
provider_port=${E2E_PROVIDER_PORT:-4643}
smtp_port=${E2E_MAIL_SMTP_PORT:-4625}
mail_http_port=${E2E_MAIL_HTTP_PORT:-4626}

door="http://127.0.0.1:$door_port"
# The page reaches the door by `localhost`, the page's own site, so the
# renewal cookie (SameSite=Strict) is sent and a reload stays signed in.
# `localhost` and `127.0.0.1` are two sites to a browser.
page_door="http://localhost:$door_port"
page="http://localhost:$page_port"
public="https://localhost:$provider_port"

for bin in "$ordo" "$host"; do
  [ -x "$bin" ] || { echo "stack: missing $bin: run cargo build -p ordo-cli -p ordo-host" >&2; exit 1; }
done
[ -d "$web/node_modules" ] || { echo "stack: run npm ci in $web first" >&2; exit 1; }

# A stack still stopping (or another one) would answer the health checks
# below in this one's place and then vanish. Wait up to 20 seconds for each
# port to be free, then refuse by name.
for port in "$door_port" "$page_port" "$provider_port" "$smtp_port" "$mail_http_port"; do
  tries=0
  while (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; do
    tries=$((tries + 1))
    [ "$tries" -lt 100 ] || {
      echo "stack: port $port is in use; stop what holds it or set the E2E_*_PORT variables" >&2
      exit 1
    }
    sleep 0.2
  done
done

rm -rf "$state"
mkdir -p "$state"
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

wait_for() { # what, url, pid
  local what=$1 url=$2 pid=$3 tries=0
  until curl -fsSk "$url" >/dev/null 2>&1; do
    kill -0 "$pid" 2>/dev/null || { echo "stack: $what stopped before it answered (logs in $state)" >&2; exit 1; }
    tries=$((tries + 1))
    [ "$tries" -lt 600 ] || { echo "stack: $what did not answer $url in 2 minutes" >&2; exit 1; }
    sleep 0.2
  done
}

# ── 1. Postgres ────────────────────────────────────────────────────────────
db=${E2E_PG_DB:-notes_e2e}
if [ -n "${ORDO_PG_HOST:-}" ]; then
  pg_host=$ORDO_PG_HOST
  pg_port=${ORDO_PG_PORT:-5432}
  pg_user=${ORDO_PG_USER:-ordo}
else
  own_pg=1
  pg_port=${E2E_PG_PORT:-55452}
  pg_user=ordo
  # A Unix socket path is limited to about 100 bytes, so it is not under $state.
  socket=/tmp/ordo-e2e-pg
  pgdata="${TMPDIR:-/tmp}/ordo-e2e-pgdata-$pg_port"
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

# ── 2. The stand-ins ───────────────────────────────────────────────────────
client_id=notes-e2e
client_secret="stand-in-secret-$RANDOM$RANDOM"
printf '%s' "$client_secret" >"$state/discord-secret"
E2E_DOOR="$door" E2E_CLIENT_ID="$client_id" E2E_CLIENT_SECRET="$client_secret" \
  E2E_PROVIDER_PORT="$provider_port" E2E_MAIL_SMTP_PORT="$smtp_port" E2E_MAIL_HTTP_PORT="$mail_http_port" \
  node "$here/stand-ins.mjs" >"$state/stand-ins.log" 2>&1 &
last=$!
pids="$pids $last"
wait_for "the stand-ins" "$public/health" "$last"
wait_for "the mail catcher" "http://localhost:$mail_http_port/health" "$last"

# ── 3. The door ────────────────────────────────────────────────────────────
export ORDO_PG_HOST="$pg_host" ORDO_PG_PORT="$pg_port" ORDO_PG_USER="$pg_user" ORDO_PG_DB="$db"
export ORDO_SPACE=system
export ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY=notes-e2e-master ORDO_DEV_UNSAFE_TEST_KEYS=1
export ORDO_TOKEN_KEY=1111111111111111111111111111111111111111111111111111111111111111
export ORDO_PAGE_KEY=2222222222222222222222222222222222222222222222222222222222222222
export ORDO_HOST_ADDR="127.0.0.1:$door_port"
# The ways in. Passkey is first; the page asks the door for the rest.
export ORDO_AUTH_METHODS=passkey,email,discord
export ORDO_APEX_DOMAIN=localhost ORDO_AUTH_ORIGINS="$page" ORDO_CORS_ORIGINS="$page"
export ORDO_AUTH_PROVISIONING=auto-onboard ORDO_PROVISION_SELF_SERVE=on-signup
# Every test signs people up from one address in a few minutes.
export ORDO_PROVISION_SIGNUPS_PER_HOUR=10000
# Every person is a tenant with a database and a pool of its own, and
# Postgres stays at its default 100 connections (CI's service container
# cannot change it). Small pools, released after 5 idle seconds, keep a run's
# few dozen tenants under that ceiling. At the defaults a run of about 60
# tenants hit "too many clients", and the door then refused writes as
# access_denied (examples/notes/README.md §6 on sizing max_connections).
export ORDO_PG_POOL_SIZE=${ORDO_PG_POOL_SIZE:-3} ORDO_PG_IDLE_SECS=${ORDO_PG_IDLE_SECS:-5}
export ORDO_AUTH_GUEST=on
# Files on notes (notes v2) go to a blob store on this machine's disk, under
# the stack's own folder. `fs:` is for tests and development only, so the
# door refuses to boot on it without the unsafe switch beside it. A real
# deployment sets ORDO_BLOB_STORE=s3 and the ORDO_BLOB_S3_* settings.
export ORDO_BLOB_STORE="fs:$state/blobs" ORDO_DEV_UNSAFE_BLOB_STORE=1
# Emailed codes go to the mail catcher, in the clear, on loopback.
export ORDO_MAIL_KIND=smtp ORDO_MAIL_FROM=notes@example.com ORDO_MAIL_HOST=localhost
export ORDO_MAIL_PORT="$smtp_port" ORDO_MAIL_TLS=none ORDO_MAIL_SEND_SECS=1
export ORDO_EGRESS_PRIVATE_HOSTS=localhost
# Discord is the stand-in: the preset's claims, the stand-in's addresses, and
# its CA added to the anchors the door checks a provider against.
export ORDO_PUBLIC_URL="$public" ORDO_AUTH_RETURN_URLS="$page"
export ORDO_AUTH_DISCORD_PRESET=discord ORDO_AUTH_DISCORD_CLIENT_ID="$client_id"
export ORDO_AUTH_DISCORD_CLIENT_SECRET_FILE="$state/discord-secret"
export ORDO_AUTH_DISCORD_AUTHORIZE_URL="$public/oauth2/authorize"
export ORDO_AUTH_DISCORD_TOKEN_URL="$public/api/oauth2/token"
export ORDO_AUTH_DISCORD_USERINFO_URL="$public/api/users/@me"
export ORDO_AUTH_PROVIDER_CA_FILE="$repo/crates/ordo-host/tests/fixture/ca.pem"
# The human check is off unless asked for. With E2E_HUMAN_CHECK=1 it is
# Cloudflare Turnstile with Cloudflare's public always-pass test keys, which
# needs the internet from both the browser and the door.
if [ "${E2E_HUMAN_CHECK:-0}" = 1 ]; then
  printf '%s' 1x0000000000000000000000000000000AA >"$state/turnstile-secret"
  export ORDO_AUTH_HUMAN_CHECK=turnstile ORDO_AUTH_HUMAN_CHECK_SECRET_FILE="$state/turnstile-secret"
  export ORDO_AUTH_HUMAN_CHECK_SITE_KEY=1x00000000000000000000AA
fi
for v in ORDO_PG_ROUTING ORDO_TENANT_CREDENTIALS ORDO_TENANT_CREDENTIALS_DIR ORDO_DEPLOYMENT_OWNERS; do unset "$v"; done

RUST_LOG=${RUST_LOG:-info} "$host" >"$state/door.log" 2>&1 &
last=$!
pids="$pids $last"
wait_for "ordo-host" "$door/health/live" "$last"

# ── 4. The program ─────────────────────────────────────────────────────────
"$ordo" apply "$program" --as notes-e2e >"$state/apply.log" 2>&1 ||
  { cat "$state/apply.log" >&2; echo "stack: ordo apply failed" >&2; exit 1; }

# ── 5. The page ────────────────────────────────────────────────────────────
(cd "$web" && VITE_ORDO_URL="$page_door" node_modules/.bin/vite build --outDir "$state/page" --emptyOutDir) >"$state/page-build.log" 2>&1 ||
  { cat "$state/page-build.log" >&2; echo "stack: the page did not build" >&2; exit 1; }
# The v2 page. It has no node_modules of its own: it borrows the v1 page's (a
# link, ignored by version control) and builds with that Vite. It goes inside
# the v1 output folder, under /v2/, which the v1 build above has already emptied.
[ -e "$web2/node_modules" ] || ln -s "$web/node_modules" "$web2/node_modules"
(cd "$web2" && VITE_ORDO_URL="$page_door" "$web/node_modules/.bin/vite" build --base /v2/ --outDir "$state/page/v2" --emptyOutDir) >"$state/page2-build.log" 2>&1 ||
  { cat "$state/page2-build.log" >&2; echo "stack: the v2 page did not build" >&2; exit 1; }
(cd "$web" && exec node_modules/.bin/vite preview --outDir "$state/page" --host localhost --port "$page_port" --strictPort) \
  >"$state/page.log" 2>&1 &
last=$!
pids="$pids $last"

cat >"$state/stack.json" <<EOF
{
  "door": "$door",
  "page": "$page",
  "pageV2": "$page/v2/",
  "programV2": "$repo/examples/notes-v2",
  "provider": "$public",
  "mail": "http://localhost:$mail_http_port",
  "program": "$program",
  "pg": { "host": "$pg_host", "port": "$pg_port", "user": "$pg_user", "db": "$db" },
  "ordo": "$ordo",
  "logs": "$state"
}
EOF

wait_for "the page" "$page/" "$last"
echo "stack: up. Page $page, door $door, provider $public, mail http://localhost:$mail_http_port/messages"
echo "stack: logs in $state. Ctrl-C stops everything."
# shellcheck disable=SC2086
wait $pids
