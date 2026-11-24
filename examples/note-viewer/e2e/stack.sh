#!/usr/bin/env bash
# The throwaway two-deployment stack, for the federation end-to-end tests.
#
#     bash examples/note-viewer/e2e/stack.sh
#
# Boots, in order, and stays in the foreground until it is stopped:
#
#   1. Postgres. A throwaway cluster of its own (scripts/conformance-pg.sh),
#      unless ORDO_PG_HOST names one already running (CI's service
#      container). One database for both apps, with two login roles and two
#      schemas: `notes_app` owns `notes`, `viewer_app` owns `viewer`. The
#      identity service gets a database of its own.
#   2. HTTPS in front (tls-front.mjs): an OIDC issuer and an app's callback
#      must be https, and everything here serves plain http on loopback.
#   3. The identity service, with `notes` and `viewer` registered, each with
#      open sign-up and its callback as a return URL.
#   4. notes v2 (examples/notes-v2) as role `notes_app` in schema `notes`.
#      Its people sign in through the identity service (`accounts`). The
#      deployment's own space admits anyone who signs in, as an editor.
#      `reader note_viewer` grants `viewer_app` its two tables.
#   5. note_viewer (examples/note-viewer) as role `viewer_app` in schema
#      `viewer`, its peer `notes` set to schema `notes`. Its people sign in
#      through the same service. ORDO_PEER_SHARED_ROLE is left unset: the two
#      roles are the point.
#   6. The two pages, built with Vite and served by `vite preview`.
#
# Playwright starts this script (playwright.config.ts, `webServer`) and stops
# it at the end. It writes what tests need to $E2E_STATE/stack.json.
#
# Needs: `cargo build -p ordo-cli -p ordo-host -p ordo-identity-service`,
# `npm ci` in examples/notes-v2/web and examples/note-viewer/web, Node 20 or
# later, and PostgreSQL 15 or later binaries (ORDO_PGBIN) when no
# ORDO_PG_HOST is given. Every value below is a test value.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
notes_web="$repo/examples/notes-v2/web"
viewer_web="$repo/examples/note-viewer/web"

ordo=${ORDO:-"$repo/target/debug/ordo"}
host=${ORDO_HOST_BIN:-"$repo/target/debug/ordo-host"}
idsvc=${E2E_IDENTITY_BIN:-"$repo/target/debug/ordo-identity-service"}
state=${E2E_STATE:-"$here/.stack"}

id_port=${E2E_IDENTITY_PORT:-4790}
id_tls=$((id_port + 1))
notes_port=${E2E_NOTES_PORT:-4740}
notes_tls=$((notes_port + 1))
viewer_port=${E2E_VIEWER_PORT:-4750}
viewer_tls=$((viewer_port + 1))
notes_page_port=${E2E_NOTES_PAGE_PORT:-5290}
viewer_page_port=${E2E_VIEWER_PAGE_PORT:-5291}

issuer="https://localhost:$id_tls"
notes_door="http://localhost:$notes_port"
viewer_door="http://localhost:$viewer_port"
notes_public="https://localhost:$notes_tls"
viewer_public="https://localhost:$viewer_tls"
notes_page="http://localhost:$notes_page_port"
viewer_page="http://localhost:$viewer_page_port"
ca="$repo/crates/ordo-host/tests/fixture/ca.pem"

for bin in "$ordo" "$host" "$idsvc"; do
  [ -x "$bin" ] || { echo "stack: missing $bin: run cargo build -p ordo-cli -p ordo-host -p ordo-identity-service" >&2; exit 1; }
done
if [ "${E2E_NO_PAGES:-0}" != 1 ]; then
  for w in "$notes_web" "$viewer_web"; do
    [ -d "$w/node_modules" ] || { echo "stack: run npm ci in $w first" >&2; exit 1; }
  done
fi

for port in "$id_port" "$id_tls" "$notes_port" "$notes_tls" "$viewer_port" "$viewer_tls" "$notes_page_port" "$viewer_page_port"; do
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
mkdir -p "$state/identity"
pids=''
own_pg=0

stop() {
  # LLVM flushes the host profile on normal exit. Keep Postgres alive until
  # every child has handled SIGTERM and finished.
  for p in $pids; do kill -TERM "$p" 2>/dev/null || true; done
  for p in $pids; do wait "$p" 2>/dev/null || true; done
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
db=${E2E_PG_DB:-federation_e2e}
id_db="${db}_identity"
if [ -n "${ORDO_PG_HOST:-}" ]; then
  pg_host=$ORDO_PG_HOST
  pg_port=${ORDO_PG_PORT:-5432}
  pg_user=${ORDO_PG_USER:-ordo}
else
  own_pg=1
  pg_port=${E2E_PG_PORT:-55462}
  pg_user=ordo
  socket=/tmp/ordo-fed-e2e-pg
  pgdata="${TMPDIR:-/tmp}/ordo-fed-e2e-pgdata-$pg_port"
  ORDO_PGDATA="$pgdata" ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" \
    bash "$repo/scripts/conformance-pg.sh" destroy >/dev/null 2>&1 || true
  ORDO_PGDATA="$pgdata" ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pg_port" ORDO_PG_DB=postgres \
    bash "$repo/scripts/conformance-pg.sh" start >"$state/pg.log" 2>&1
  pg_host=$socket
fi
admin_password=${ORDO_PG_PASSWORD:-}
psql_bin=${ORDO_PSQL:-$( [ -n "${ORDO_PGBIN:-}" ] && echo "$ORDO_PGBIN/psql" || command -v psql || echo /opt/homebrew/opt/postgresql@17/bin/psql)}
as_admin() { # database, sql
  PGPASSWORD="$admin_password" "$psql_bin" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d "$1" -v ON_ERROR_STOP=1 -qc "$2"
}
role_password=fed-e2e-password
for d in "$db" "$id_db"; do
  as_admin postgres "drop database if exists \"$d\" with (force)"
  as_admin postgres "create database \"$d\""
done
# The two roles are cluster-wide, so a rerun finds them. Each owns its schema
# and nothing else: no superuser, no membership in the other.
for role in notes_app viewer_app; do
  as_admin postgres "DO \$\$ BEGIN CREATE ROLE $role LOGIN PASSWORD '$role_password';
    EXCEPTION WHEN duplicate_object THEN ALTER ROLE $role LOGIN PASSWORD '$role_password'; END \$\$"
done
as_admin "$db" "CREATE SCHEMA notes AUTHORIZATION notes_app; CREATE SCHEMA viewer AUTHORIZATION viewer_app;
  GRANT CONNECT, TEMPORARY, CREATE ON DATABASE \"$db\" TO notes_app, viewer_app;
  REVOKE CREATE ON SCHEMA public FROM PUBLIC;"

# ── 2. HTTPS in front ──────────────────────────────────────────────────────
node "$here/tls-front.mjs" "$id_tls=$id_port" "$notes_tls=$notes_port" "$viewer_tls=$viewer_port" >"$state/tls-front.log" 2>&1 &
last=$!
pids="$pids $last"

# ── 3. The identity service ────────────────────────────────────────────────
id_url="host=$pg_host port=$pg_port user=$pg_user dbname=$id_db"
[ -n "$admin_password" ] && id_url="$id_url password=$admin_password"
id_env() {
  exec env ORDO_IDENTITY_DATABASE_URL="$id_url" ORDO_IDENTITY_ORIGIN="$issuer" \
    ORDO_IDENTITY_LISTEN="127.0.0.1:$id_port" ORDO_IDENTITY_DATA_DIR="$state/identity" \
    ORDO_IDENTITY_SIGN_UPS_PER_ADDRESS=100 "$@"
}
# Registered before the service starts: a running service reads a new
# registration only on its next pass, up to a minute later. Each app
# registers its callback alone: its pages are no client origin, so they
# cannot spend the service's session cookie.
(id_env "$idsvc" deployment add notes \
  --return "$notes_public/auth/callback/accounts" --sign-up open) >"$state/register.log" 2>&1
(id_env "$idsvc" deployment add viewer \
  --return "$viewer_public/auth/callback/accounts" --sign-up open) >>"$state/register.log" 2>&1
id_env "$idsvc" >"$state/identity.log" 2>&1 &
last=$!
pids="$pids $last"
wait_for "the identity service" "$issuer/health/ready" "$last"

# ── 4. and 5. The two doors ────────────────────────────────────────────────
# What both doors share. Each sets its own role, schema, port and client id.
# A public answer over an external is held 2 seconds here (60 by default), so
# a note written in notes v2 shows on note_viewer's public list at once.
door_env() { # role, schema, port, public, page, client id, then the command
  local role=$1 schema=$2 port=$3 public=$4 page=$5 client=$6
  shift 6
  exec env ORDO_PG_HOST="$pg_host" ORDO_PG_PORT="$pg_port" ORDO_PG_DB="$db" \
    ORDO_PG_USER="$role" ORDO_PG_PASSWORD="$role_password" ORDO_PG_SCHEMA="$schema" \
    ORDO_SPACE=system ORDO_PG_POOL_SIZE=3 \
    ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY="federation-e2e-master-$schema" ORDO_DEV_UNSAFE_TEST_KEYS=1 \
    ORDO_TOKEN_KEY=1111111111111111111111111111111111111111111111111111111111111111 \
    ORDO_PAGE_KEY=2222222222222222222222222222222222222222222222222222222222222222 \
    ORDO_HOST_ADDR="127.0.0.1:$port" \
    ORDO_APEX_DOMAIN=localhost ORDO_AUTH_ORIGINS="$page" ORDO_CORS_ORIGINS="$page" \
    ORDO_AUTH_PROVISIONING=auto-onboard \
    ORDO_AUTH_METHODS=accounts ORDO_PUBLIC_URL="$public" ORDO_AUTH_RETURN_URLS="$page" \
    ORDO_AUTH_ACCOUNTS_KIND=oidc ORDO_AUTH_ACCOUNTS_ISSUER="$issuer" \
    ORDO_AUTH_ACCOUNTS_CLIENT_ID="$client" ORDO_AUTH_ACCOUNTS_CLIENT_AUTH=none \
    ORDO_AUTH_PROVIDER_CA_FILE="$ca" ORDO_EGRESS_PRIVATE_HOSTS=localhost \
    ORDO_BLOB_STORE="fs:$state/blobs-$schema" ORDO_DEV_UNSAFE_BLOB_STORE=1 \
    ORDO_READER_NOTE_VIEWER_ROLE=viewer_app ORDO_PEER_NOTES_SCHEMA=notes \
    ORDO_PUBLIC_CDN_S_MAXAGE=2 \
    RUST_LOG="${RUST_LOG:-info}" "$@"
}
notes_env() { door_env notes_app notes "$notes_port" "$notes_public" "$notes_page" notes "$@"; }
viewer_env() { door_env viewer_app viewer "$viewer_port" "$viewer_public" "$viewer_page" viewer "$@"; }

# The deployment's own space admits whoever signs in: notes v2 people as
# editors, who write notes; viewer people as viewers. A deployment owner sets
# this with `std.space.setAdmission`; the stack writes the row before anyone
# arrives.
admit() { # schema, role
  as_admin "$db" "SET search_path TO $1;
    INSERT INTO \"\$admissionpolicy\" (\"space\", \"open\", \"default_role\") VALUES ('system', true, '$2')
      ON CONFLICT (\"space\") DO UPDATE SET \"open\" = true, \"default_role\" = '$2';"
}

notes_env "$host" >"$state/notes.log" 2>&1 &
last=$!
pids="$pids $last"
printf '%s\n' "$last" >> "$state/host-pids"
wait_for "notes v2" "$notes_door/health/live" "$last"
(notes_env "$ordo" apply "$repo/examples/notes-v2" --as fed-e2e --direct) >"$state/notes-apply.log" 2>&1 ||
  { cat "$state/notes-apply.log" >&2; echo "stack: ordo apply of notes v2 failed" >&2; exit 1; }
admit notes editor
# notes v2's environment, for a test that applies another notes program over
# it with the same `--direct` apply (tests/v3, increment 7a). Test values.
(notes_env env) >"$state/notes.env"

viewer_env "$host" >"$state/viewer.log" 2>&1 &
last=$!
pids="$pids $last"
printf '%s\n' "$last" >> "$state/host-pids"
wait_for "note_viewer" "$viewer_door/health/live" "$last"
(viewer_env "$ordo" apply "$repo/examples/note-viewer" --as fed-e2e --direct) >"$state/viewer-apply.log" 2>&1 ||
  { cat "$state/viewer-apply.log" >&2; echo "stack: ordo apply of note_viewer failed" >&2; exit 1; }
admit viewer viewer

cat >"$state/stack.json" <<EOF
{
  "issuer": "$issuer",
  "notesDoor": "$notes_door",
  "viewerDoor": "$viewer_door",
  "notesPage": "$notes_page/",
  "viewerPage": "$viewer_page/",
  "ordo": "$ordo",
  "programV3": "$repo/examples/notes-v3",
  "pg": { "host": "$pg_host", "port": "$pg_port", "db": "$db", "admin": "$pg_user", "rolePassword": "$role_password" },
  "logs": "$state"
}
EOF

if [ "${E2E_NO_PAGES:-0}" = 1 ]; then
  echo "stack: up without pages. Logs in $state."
  # shellcheck disable=SC2086
  wait $pids
  exit 0
fi

# ── 6. The pages ───────────────────────────────────────────────────────────
# notes v2's page arrives at the deployment's own space (VITE_ORDO_SPACE),
# since nobody here has a tenant of their own.
(cd "$notes_web" && VITE_ORDO_URL="$notes_door" VITE_ORDO_SPACE=system node_modules/.bin/vite build --outDir "$state/notes-page" --emptyOutDir) >"$state/notes-page-build.log" 2>&1 ||
  { cat "$state/notes-page-build.log" >&2; echo "stack: the notes v2 page did not build" >&2; exit 1; }
(cd "$viewer_web" && VITE_ORDO_URL="$viewer_door" node_modules/.bin/vite build --outDir "$state/viewer-page" --emptyOutDir) >"$state/viewer-page-build.log" 2>&1 ||
  { cat "$state/viewer-page-build.log" >&2; echo "stack: the note_viewer page did not build" >&2; exit 1; }
(cd "$notes_web" && exec node_modules/.bin/vite preview --outDir "$state/notes-page" --host localhost --port "$notes_page_port" --strictPort) \
  >"$state/notes-page.log" 2>&1 &
pids="$pids $!"
(cd "$viewer_web" && exec node_modules/.bin/vite preview --outDir "$state/viewer-page" --host localhost --port "$viewer_page_port" --strictPort) \
  >"$state/viewer-page.log" 2>&1 &
last=$!
pids="$pids $last"
wait_for "the notes v2 page" "$notes_page/" "$last"
wait_for "the note_viewer page" "$viewer_page/" "$last"
echo "stack: up. notes v2 $notes_page, note_viewer $viewer_page, identity $issuer. Logs in $state."
# shellcheck disable=SC2086
wait $pids
