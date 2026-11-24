#!/usr/bin/env bash
# A throwaway teamspace deployment. Each founder gets a separate tenant.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
state=${E2E_STATE:-"$here/.stack"}
ordo=${ORDO:-"$repo/target/debug/ordo"}
host=${ORDO_HOST_BIN:-"$repo/target/debug/ordo-host"}
web="$repo/examples/teamspace/web"
door_port=${E2E_DOOR_PORT:-4840}
page_port=${E2E_PAGE_PORT:-5390}
provider_port=${E2E_PROVIDER_PORT:-4843}
smtp_port=${E2E_MAIL_SMTP_PORT:-4825}
mail_port=${E2E_MAIL_HTTP_PORT:-4826}
door="http://localhost:$door_port"
page="http://localhost:$page_port"
for bin in "$ordo" "$host"; do
  [ -x "$bin" ] || { echo "Missing binary $bin" >&2; exit 1; }
done
for port in "$door_port" "$page_port" "$provider_port" "$smtp_port" "$mail_port"; do
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
    echo "Port $port is in use; choose an E2E_*_PORT" >&2; exit 1
  fi
done
mkdir -p "$state"
: > "$state/host-pids"
# A unique database avoids dropping another stack's data on shared Postgres.
db="teamspace_e2e_$$_$RANDOM"
source "$repo/examples/e2e-common/stack-lib.sh"
start_pg
E2E_DOOR="$door" E2E_CLIENT_ID=teamspace-e2e E2E_CLIENT_SECRET=test-secret \
  E2E_PROVIDER_PORT="$provider_port" E2E_MAIL_SMTP_PORT="$smtp_port" E2E_MAIL_HTTP_PORT="$mail_port" \
  node "$repo/examples/notes/e2e/stand-ins.mjs" >"$state/mail.log" 2>&1 &
last=$!
pids="$pids $last"
wait_for mail "http://localhost:$mail_port/health" "$last"
export ORDO_SPACE=system
export ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY=teamspace-e2e-master ORDO_DEV_UNSAFE_TEST_KEYS=1
export ORDO_TOKEN_KEY=5555555555555555555555555555555555555555555555555555555555555555
export ORDO_PAGE_KEY=6666666666666666666666666666666666666666666666666666666666666666
export ORDO_HOST_ADDR="127.0.0.1:$door_port"
export ORDO_AUTH_METHODS=passkey,email ORDO_APEX_DOMAIN=localhost
export ORDO_AUTH_ORIGINS="$page" ORDO_CORS_ORIGINS="$page" ORDO_PUBLIC_URL="$door"
export ORDO_AUTH_PROVISIONING=auto-onboard ORDO_PROVISION_SELF_SERVE=on-signup
export ORDO_PROVISION_SIGNUPS_PER_HOUR=10000 ORDO_PG_POOL_SIZE=3 ORDO_PG_IDLE_SECS=5
export ORDO_ACCESS_TEMPLATE='reportByProject=owner,manager; reportByPerson=owner,manager'
export ORDO_MAIL_KIND=smtp ORDO_MAIL_FROM=teamspace@example.com ORDO_MAIL_HOST=localhost
export ORDO_MAIL_PORT="$smtp_port" ORDO_MAIL_TLS=none ORDO_MAIL_SEND_SECS=1
export ORDO_EGRESS_PRIVATE_HOSTS=localhost
for v in ORDO_PG_ROUTING ORDO_TENANT_CREDENTIALS ORDO_TENANT_CREDENTIALS_DIR ORDO_DEPLOYMENT_OWNERS; do unset "$v"; done
start_door
"$ordo" apply "$repo/examples/teamspace" --as teamspace-e2e --direct >"$state/apply.log" 2>&1 || { cat "$state/apply.log" >&2; exit 1; }
(cd "$web" && VITE_ORDO_URL="$door" npm run build -- --outDir "$state/web") >"$state/build.log" 2>&1 || { cat "$state/build.log" >&2; exit 1; }
(cd "$web" && exec node node_modules/vite/bin/vite.js preview --outDir "$state/web" --host localhost --port "$page_port" --strictPort) >"$state/page.log" 2>&1 &
last=$!
pids="$pids $last"
cat >"$state/stack.json" <<JSON
{"door":"$door","page":"$page","mail":"http://localhost:$mail_port"}
JSON
wait_for page "$page/" "$last"
echo "stack: teamspace at $page; door $door"
# shellcheck disable=SC2086 # Wait for every child pid in the list.
wait $pids
