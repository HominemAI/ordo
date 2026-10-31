#!/bin/sh
# The teamspace deployment, on this machine.
#
# Starts a throwaway Postgres, boots `ordo-host` with the settings the README
# gives a deployment owner (passkey sign-in on `localhost`, a tenant for each
# founder who signs up, invitations for colleagues, the page's origin
# allowed), applies `../teamspace.ordo`, and stays in the foreground until
# Ctrl-C.
#
#     sh examples/teamspace/dev/serve.sh
#
# Then, in another terminal:
#
#     cd examples/teamspace/web && npm install && npm run dev
#
# and open http://localhost:5174. To fill a team with made-up people,
# projects and tasks, start this script with TEAMSPACE_SEEDABLE=1 and run
# `node examples/teamspace/dev/seed.mjs` (see ../README.md, section 3).
#
# Nothing here is fit for a real deployment: the key provider is the
# development one, the secrets are constants, and the database has no
# password. `../README.md` says what a real one sets.
#
# Needs: PostgreSQL 15 or later (`ORDO_PGBIN` names its bin directory), and
# `cargo build -p ordo-cli -p ordo-host` done (or `ORDO` and `ORDO_HOST_BIN`
# naming the two binaries).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
ordo=${ORDO:-"$repo/target/debug/ordo"}
host=${ORDO_HOST_BIN:-"$repo/target/debug/ordo-host"}
pgbin=${ORDO_PGBIN:-/opt/homebrew/opt/postgresql@17/bin}
data=${TEAMSPACE_DEV_DIR:-"$here/.data"}
# A Unix socket path is limited to about 100 bytes, so it is not under $data.
socket=${TEAMSPACE_DEV_SOCKET:-/tmp/ordo-teamspace-pg}
pgport=${TEAMSPACE_DEV_PG_PORT:-55492}
addr=${TEAMSPACE_DEV_ADDR:-127.0.0.1:4001}
page=${TEAMSPACE_DEV_PAGE:-http://localhost:5174}

for bin in "$ordo" "$host"; do
  [ -x "$bin" ] || { echo "missing $bin: run cargo build -p ordo-cli -p ordo-host" >&2; exit 1; }
done

mkdir -p "$data" "$socket"

# ── Postgres ───────────────────────────────────────────────────────────────
if [ ! -d "$data/pg" ]; then
  "$pgbin/initdb" -D "$data/pg" -U ordo --auth=trust >/dev/null
fi
if ! "$pgbin/pg_isready" -h "$socket" -p "$pgport" -q; then
  "$pgbin/pg_ctl" -D "$data/pg" -o "-p $pgport -k $socket -h ''" -l "$data/pg.log" start >/dev/null
  until "$pgbin/pg_isready" -h "$socket" -p "$pgport" -q; do sleep 0.2; done
fi
"$pgbin/createdb" -h "$socket" -p "$pgport" -U ordo teamspace 2>/dev/null || true

# ── The settings ───────────────────────────────────────────────────────────
export ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pgport" ORDO_PG_USER=ordo ORDO_PG_DB=teamspace
export ORDO_SPACE=system
export ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY=teamspace-dev-master ORDO_DEV_UNSAFE_TEST_KEYS=1
export ORDO_TOKEN_KEY=5555555555555555555555555555555555555555555555555555555555555555
export ORDO_PAGE_KEY=6666666666666666666666666666666666666666666666666666666666666666
export ORDO_HOST_ADDR="$addr"
# Sign-in and sign-up with a passkey, relying party `localhost`. Set
# ORDO_AUTH_METHODS before running to offer more ways in (passkey,email).
export ORDO_AUTH_METHODS="${ORDO_AUTH_METHODS:-passkey}" ORDO_APEX_DOMAIN=localhost ORDO_AUTH_ORIGINS="$page"
# A founder who signs up gets a team of their own. Colleagues come in by
# invitation. Set ORDO_AUTH_PROVISIONING=invite-only to refuse anyone who
# holds no invitation (then nobody new can found a team).
export ORDO_AUTH_PROVISIONING="${ORDO_AUTH_PROVISIONING:-auto-onboard}" ORDO_PROVISION_SELF_SERVE=on-signup
# The app's default per op (its profile template): the two reports are for
# owners and managers. Each tenant's owner may change it for their tenant
# with std.space.setOpAccess.
export ORDO_ACCESS_TEMPLATE="reportByProject=owner,manager; reportByPerson=owner,manager"
# The page may call the door from its own address.
export ORDO_CORS_ORIGINS="$page"
# The seed signs its made-up people in as guests, and the founder invites
# them without a passkey touch. Only when asked, and never on a real
# deployment.
if [ "${TEAMSPACE_SEEDABLE:-}" = 1 ]; then
  export ORDO_AUTH_GUEST=on ORDO_STEP_UP_EXEMPT_OPS=std.space.invite
  echo "TEAMSPACE_SEEDABLE=1: guests are on and std.space.invite asks for no passkey touch"
fi

# ── The door, then the program ─────────────────────────────────────────────
"$host" &
door=$!
trap 'kill "$door" 2>/dev/null || true; "$pgbin/pg_ctl" -D "$data/pg" stop >/dev/null 2>&1 || true' EXIT INT TERM

port=${addr##*:}
until curl -fsS "http://127.0.0.1:$port/health/live" >/dev/null 2>&1; do
  kill -0 "$door" 2>/dev/null || { echo "ordo-host stopped before it served" >&2; exit 1; }
  sleep 0.2
done

"$ordo" apply "$repo/examples/teamspace" --as teamspace-dev
echo
echo "The door is at http://127.0.0.1:$port and the page belongs at $page."
echo "Ctrl-C stops both the door and the database."
wait "$door"
