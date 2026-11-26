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
fs.writeFileSync(`${state}/console.json`, JSON.stringify({ url: `http://localhost:${port}`, data: `${state}/console-data`, databaseUrl: url.toString(), db, psql: s.psql, pg, door: s.door }), {mode: 0o600});
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
export ORDO_CONSOLE_DATABASE_URL="$(value databaseUrl)"
export ORDO_CONSOLE_ORIGIN="$(value url)"
export ORDO_CONSOLE_LISTEN="127.0.0.1:$port"
export ORDO_CONSOLE_DATA_DIR="$state/console-data"
export ORDO_CONSOLE_PAGE_DIR="$state/console-page"
export ORDO_CONSOLE_DEPLOYMENT=main
export ORDO_CONSOLE_DOOR_URL="$(value door)"
export ORDO_CONSOLE_DOOR_SPACE=system
unset ORDO_CONSOLE_SUPERVISOR_SOCKET ORDO_IDENTITY_URL
exec "$console_bin"
