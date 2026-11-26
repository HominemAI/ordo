#!/usr/bin/env bash
# The real console beside the real marketing hosts. No stand-in door.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
state=${E2E_STATE:-"$here/.stack"}
console_bin=${ORDO_CONSOLE_BIN:-"$repo/target/debug/ordo-console"}
port=${E2E_REAL_CONSOLE_PORT:-5392}
[ -x "$console_bin" ] || { echo "Missing real console binary" >&2; exit 1; }
mkdir -p "$state/console-data"
# The console has its own database, never the deployment's database.
node - "$state" "$port" <<'JS'
const fs = require('node:fs');
const [state, port] = process.argv.slice(2);
const runtimePort = process.env.E2E_RUNTIME_PORT || "5393";
const s = JSON.parse(fs.readFileSync(`${state}/stack.json`, 'utf8'));
const pg = s.pg;
const db = `${pg.db}_console`;
const url = new URL('postgresql://localhost');
url.username = pg.user;
url.password = process.env.ORDO_PG_PASSWORD || '';
url.hostname = pg.host.startsWith('/') ? 'localhost' : pg.host;
url.port = pg.port;
url.pathname = `/${db}`;
if (pg.host.startsWith('/')) url.searchParams.set('host', pg.host);
fs.writeFileSync(`${state}/console.json`, JSON.stringify({ url: `http://localhost:${port}`, data: `${state}/console-data`, databaseUrl: url.toString(), runtimeDoor: `http://localhost:${runtimePort}`, db, psql: s.psql, pg, door: s.door }), {mode: 0o600});
JS
# Read specific fields; never echo the connection secret.
value() { node -e 'process.stdout.write(String(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))[process.argv[2]]))' "$state/console.json" "$1"; }
psql=$(value psql)
pg_host=$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).pg.host' "$state/console.json")
pg_port=$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).pg.port' "$state/console.json")
pg_user=$(node -p 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).pg.user' "$state/console.json")
console_db=$(value db)
[[ "$console_db" =~ ^[a-z][a-z0-9_]*$ ]] || exit 1
PGPASSWORD=${ORDO_PG_PASSWORD:-} "$psql" -h "$pg_host" -p "$pg_port" -U "$pg_user" -d postgres -qc "create database \"$console_db\""
(cd "$repo/clients/ordo-console-web" && npm run build -- --outDir "$state/console-page") >"$state/console-build.log" 2>&1
# Restore exactly the fixture host settings, without printing secrets.
while IFS= read -r -d '' assignment; do export "$assignment"; done < <(
  node -e 'for (const [k,v] of Object.entries(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")))) process.stdout.write(`${k}=${v}\0`)' "$state/host-env.json"
)
export ORDO_CONSOLE_DATABASE_URL="$(value databaseUrl)"
export ORDO_CONSOLE_ORIGIN="$(value url)"
export ORDO_CONSOLE_LISTEN="127.0.0.1:$port"
export ORDO_CONSOLE_DATA_DIR="$state/console-data"
export ORDO_CONSOLE_PAGE_DIR="$state/console-page"
export ORDO_CONSOLE_DEPLOYMENT=main
export ORDO_CONSOLE_DOOR_URL="$(value door)"
export ORDO_CONSOLE_DOOR_SPACE=system
unset ORDO_CONSOLE_SUPERVISOR_SOCKET ORDO_IDENTITY_URL
# A third real host is owned by the real supervisor. Restart tests affect
# this process only; the public-site replicas continue serving their journeys.
export ORDO_HOST_ADDR="127.0.0.1:${E2E_RUNTIME_PORT:-5393}" ORDO_REPLICA_ID=marketing-console
export ORDO_SUPERVISOR_HOST_BIN="${ORDO_HOST_BIN:-$repo/target/debug/ordo-host}"
export ORDO_SUPERVISOR_CONSOLE_BIN="$console_bin"
# The supervisor intentionally clears unregistered environment variables.
# Coverage-only launchers restore the LLVM destination and record every real
# child PID, including restarts. They exec the production binary unchanged.
if [ -n "${LLVM_PROFILE_FILE:-}" ]; then
  python3 - "$state" "$ORDO_SUPERVISOR_HOST_BIN" "$ORDO_SUPERVISOR_CONSOLE_BIN" "$LLVM_PROFILE_FILE" <<'PYWRAP'
import os, shlex, sys
state, host, console, profile = sys.argv[1:]
for kind, binary in [('host', host), ('console', console)]:
    path = os.path.join(state, f'profile-{kind}')
    with open(path, 'w') as out:
        out.write('#!/bin/sh\n')
        out.write(f'export LLVM_PROFILE_FILE={shlex.quote(profile)}\n')
        out.write(f'printf "%s\\n" "$$" >> {shlex.quote(os.path.join(state, "host-pids"))}\n')
        out.write(f'exec {shlex.quote(binary)} "$@"\n')
    os.chmod(path, 0o700)
PYWRAP
  export ORDO_SUPERVISOR_HOST_BIN="$state/profile-host"
  export ORDO_SUPERVISOR_CONSOLE_BIN="$state/profile-console"
fi
export ORDO_SUPERVISOR_STATE_DIR="$state/supervisor" ORDO_SUPERVISOR_RUN_DIR="$state/run"
export ORDO_SUPERVISOR_LISTEN="127.0.0.1:${E2E_SUPERVISOR_PORT:-5394}"
export ORDO_SUPERVISOR_GRACE_SECS=10 ORDO_SUPERVISOR_READY_SECS=120
# No identity database is configured in this profile: the supervisor's
# supported local-console mode leaves that service off, without a fake child.
unset ORDO_IDENTITY_DATABASE_URL
exec "${ORDO_SUPERVISOR_BIN:-$repo/target/debug/ordo-supervisor}"
