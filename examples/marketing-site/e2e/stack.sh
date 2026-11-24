#!/usr/bin/env bash
# A throwaway deployment of the real public marketing site.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
state=${E2E_STATE:-"$here/.stack"}
ordo=${ORDO:-"$repo/target/debug/ordo"}
host=${ORDO_HOST_BIN:-"$repo/target/debug/ordo-host"}
web="$repo/examples/marketing-site/web"
door_port=${E2E_DOOR_PORT:-4850}
page_port=${E2E_PAGE_PORT:-5391}
door_b_port=${E2E_DOOR_B_PORT:-4851}
door_b="http://localhost:$door_b_port"
door="http://localhost:$door_port"
page="http://localhost:$page_port"
for bin in "$ordo" "$host"; do
  [ -x "$bin" ] || { echo "Missing binary $bin" >&2; exit 1; }
done
for port in "$door_port" "$door_b_port" "$page_port"; do
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "Port $port is in use; choose an E2E_*_PORT" >&2; exit 1
  fi
done
mkdir -p "$state"
: > "$state/host-pids"
db="marketing_e2e_$$_$RANDOM"
source "$repo/examples/e2e-common/stack-lib.sh"
start_pg
export ORDO_SPACE=system
export ORDO_BLOB_STORE="fs:$state/blobs" ORDO_DEV_UNSAFE_BLOB_STORE=1
export ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY=marketing-e2e-master ORDO_DEV_UNSAFE_TEST_KEYS=1
export ORDO_TOKEN_KEY=1111111111111111111111111111111111111111111111111111111111111111
export ORDO_PAGE_KEY=2222222222222222222222222222222222222222222222222222222222222222
export ORDO_REPLICA_ID=marketing-a
export ORDO_HOST_ADDR="127.0.0.1:$door_port"
export ORDO_AUTH_METHODS=passkey ORDO_AUTH_GUEST=on ORDO_APEX_DOMAIN=localhost
export ORDO_AUTH_ORIGINS="$page" ORDO_CORS_ORIGINS="$page" ORDO_PUBLIC_URL="$door"
export ORDO_AUTH_PROVISIONING=auto-onboard ORDO_PROVISION_SELF_SERVE=on-signup
export ORDO_PUBLIC_HUMAN_CHECK=off ORDO_PUBLIC_WRITES_PER_HOUR=1000
export ORDO_PG_POOL_SIZE=3 ORDO_PG_IDLE_SECS=5
for v in ORDO_PG_ROUTING ORDO_TENANT_CREDENTIALS ORDO_TENANT_CREDENTIALS_DIR ORDO_DEPLOYMENT_OWNERS; do unset "$v"; done
start_door
bootstrap_pid=$last
"$ordo" apply "$repo/examples/marketing-site" --as marketing-e2e --direct >"$state/apply.log" 2>&1 || { cat "$state/apply.log" >&2; exit 1; }
# Sign up through the real door, then restart with this principal as the site
# owner. The seed script resumes this same account. No tokens go in logs.
node "$here/bootstrap-owner.mjs" "$door" "$page" "$state"
kill -TERM "$bootstrap_pid"
wait "$bootstrap_pid"
pids=''
mv "$state/door.log" "$state/bootstrap.log"
ORDO_DEPLOYMENT_OWNERS="$(cat "$state/owner-principal"),$(cat "$state/cache-owner-principal")"
export ORDO_DEPLOYMENT_OWNERS
start_door
ORDO_URL="$door" SITE_DEV_PAGE="$page" SITE_DEV_DIR="$state" \
  sh "$repo/examples/marketing-site/dev/seed" >"$state/seed.log" 2>&1 || { cat "$state/seed.log" >&2; exit 1; }
# A second replica of the same deployment. Keep its log and profile separate.
ORDO_HOST_ADDR="127.0.0.1:$door_b_port" ORDO_REPLICA_ID=marketing-b "$host" >"$state/door-b.log" 2>&1 &
last=$!
pids="$pids $last"
printf '%s\n' "$last" >> "$state/host-pids"
wait_for replica "$door_b/health/live" "$last"
wait_for catalogue "$door_b/op/catalogue" "$last"
(cd "$web" && VITE_ORDO_URL="$door" npm run build -- --outDir "$state/web") >"$state/build.log" 2>&1 || { cat "$state/build.log" >&2; exit 1; }
(cd "$web" && exec node node_modules/vite/bin/vite.js preview --outDir "$state/web" --host localhost --port "$page_port" --strictPort) >"$state/page.log" 2>&1 &
last=$!
pids="$pids $last"
node - "$door" "$door_b" "$page" "$ordo" "$psql_bin" "$repo/examples/marketing-site/site.ordo" "$state" <<'JS'
const fs = require('node:fs');
const [door, doorB, page, ordo, psql, program, state] = process.argv.slice(2);
const pg = Object.fromEntries(['host', 'port', 'user', 'db'].map(key => [key, process.env[`ORDO_PG_${key.toUpperCase()}`]]));
fs.writeFileSync(`${state}/stack.json`, JSON.stringify({ door, doorB, page, ordo, psql, program, pg }));
JS
wait_for page "$page/" "$last"
echo "stack: marketing site at $page; door $door"
# shellcheck disable=SC2086 # Wait for every child pid in the list.
wait $pids
