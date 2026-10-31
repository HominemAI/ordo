#!/bin/sh
# The shopfront deployment, on this machine.
#
# Starts a throwaway Postgres, boots `ordo-host` with the settings the README
# gives an app developer (passkey sign-in on `localhost`, three page origins,
# one tenant everyone joins), applies `../shopfront.ordo`, names the shop's
# owner, fills the shop with synthesized data, and stays in the foreground
# until Ctrl-C.
#
#     sh examples/shopfront/dev/serve.sh
#
# Then, in another terminal:
#
#     cd examples/shopfront/web && npm install && npm run dev
#
# and open http://localhost:5181 (the shop) or http://localhost:5182 (the
# staff console). Both send you to http://localhost:5180 to sign in.
#
# The first run signs up a seed owner with a software passkey, restarts the
# door with that person named in `ORDO_DEPLOYMENT_OWNERS`, and runs
# `seed.mjs`. Later runs reuse what `.data` holds. Delete `.data` to start
# again.
#
# Settings this script reads:
#
#   ORDO_AUTH_METHODS      the ways in the login page offers (default passkey)
#   SHOPFRONT_OWNERS       more deployment owners, as account ids (usr_...)
#   SHOPFRONT_SEED=off     skip the synthesized data
#   SHOPFRONT_LOGIN_PAGE, SHOPFRONT_SHOP_PAGE, SHOPFRONT_ADMIN_PAGE
#                          the three page origins (defaults below)
#
# Nothing here is fit for a real deployment: the key provider is the
# development one, the secrets are constants, and the database has no
# password. `../README.md` says what a real one sets.
#
# Needs: PostgreSQL 15 or later (`ORDO_PGBIN` names its bin directory), Node 18
# or later, and `cargo build -p ordo-cli -p ordo-host` done (or `ORDO` and
# `ORDO_HOST_BIN` naming the two binaries).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
ordo=${ORDO:-"$repo/target/debug/ordo"}
host=${ORDO_HOST_BIN:-"$repo/target/debug/ordo-host"}
pgbin=${ORDO_PGBIN:-/opt/homebrew/opt/postgresql@17/bin}
data=${SHOPFRONT_DEV_DIR:-"$here/.data"}
# A Unix socket path is limited to about 100 bytes, so it is not under $data.
socket=${SHOPFRONT_DEV_SOCKET:-/tmp/ordo-shopfront-pg}
pgport=${SHOPFRONT_DEV_PG_PORT:-55494}
addr=${SHOPFRONT_DEV_ADDR:-127.0.0.1:4010}
login=${SHOPFRONT_LOGIN_PAGE:-http://localhost:5180}
shop=${SHOPFRONT_SHOP_PAGE:-http://localhost:5181}
admin=${SHOPFRONT_ADMIN_PAGE:-http://localhost:5182}
# The deployment's own tenant. In a single-tenant deployment it is the shop.
space=${SHOPFRONT_SPACE:-shopfront}

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
"$pgbin/createdb" -h "$socket" -p "$pgport" -U ordo shopfront 2>/dev/null || true

# ── The settings ───────────────────────────────────────────────────────────
export ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pgport" ORDO_PG_USER=ordo ORDO_PG_DB=shopfront
export ORDO_SPACE="$space"
export ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY=shopfront-dev-master ORDO_DEV_UNSAFE_TEST_KEYS=1
export ORDO_TOKEN_KEY=5555555555555555555555555555555555555555555555555555555555555555
export ORDO_PAGE_KEY=6666666666666666666666666666666666666666666666666666666666666666
export ORDO_HOST_ADDR="$addr"
# One relying party, `localhost`, for all three pages. Each page is its own
# origin, so each is listed. Set ORDO_AUTH_METHODS before running to offer
# more ways in (passkey,email).
export ORDO_AUTH_METHODS="${ORDO_AUTH_METHODS:-passkey}" ORDO_APEX_DOMAIN=localhost
export ORDO_AUTH_ORIGINS="$login,$shop,$admin"
# The pages call the door from their own origins.
export ORDO_CORS_ORIGINS="$login,$shop,$admin"
# The login page may hand a session back to the shop and to the console.
export ORDO_AUTH_RETURN_URLS="$shop,$admin"
# Tenants may open their doors to strangers (the shop admits customers that
# way), and nobody is given a tenant of their own: there is one tenant.
export ORDO_AUTH_PROVISIONING=auto-onboard ORDO_PROVISION_SELF_SERVE=off

# What seed.mjs needs to reach the door as the pages do.
export SHOPFRONT_DOOR="http://$addr" SHOPFRONT_SPACE="$space" SHOPFRONT_LOGIN_PAGE="$login"
export SHOPFRONT_DATA="$data"

owners() {
  o=""
  [ -f "$data/owner" ] && o=$(cat "$data/owner")
  if [ -n "${SHOPFRONT_OWNERS:-}" ]; then
    o="${o:+$o,}$SHOPFRONT_OWNERS"
  fi
  echo "$o"
}

door=""
stop_door() {
  if [ -n "$door" ]; then
    kill "$door" 2>/dev/null || true
    wait "$door" 2>/dev/null || true
    door=""
  fi
}
trap 'stop_door; "$pgbin/pg_ctl" -D "$data/pg" stop >/dev/null 2>&1 || true' EXIT INT TERM

port=${addr##*:}
start_door() {
  o=$(owners)
  if [ -n "$o" ]; then
    ORDO_DEPLOYMENT_OWNERS="$o" "$host" &
  else
    env -u ORDO_DEPLOYMENT_OWNERS "$host" &
  fi
  door=$!
  until curl -fsS "http://127.0.0.1:$port/health/live" >/dev/null 2>&1; do
    kill -0 "$door" 2>/dev/null || { echo "ordo-host stopped before it served" >&2; exit 1; }
    sleep 0.2
  done
}

# ── The door, then the program ─────────────────────────────────────────────
# The tenant needs keys before the door can serve it. The door never mints
# them for a name it was handed; the app developer runs this once.
"$ordo" keys init --space "$space" --as shopfront-dev
start_door
"$ordo" apply "$repo/examples/shopfront" --as shopfront-dev

# ── The owner, named on the server ─────────────────────────────────────────
# The deployment's owner is set on the server: an account signs up, its id
# goes in ORDO_DEPLOYMENT_OWNERS, and the door restarts. Here the account is
# the seed's own software passkey.
if [ ! -f "$data/owner" ]; then
  node "$here/seed.mjs" owner
  stop_door
  start_door
fi

if [ "${SHOPFRONT_SEED:-on}" != "off" ] && [ ! -f "$data/seeded" ]; then
  node "$here/seed.mjs"
fi

echo
echo "The door is at http://127.0.0.1:$port."
echo "Pages: sign-in $login, shop $shop, staff console $admin."
echo "Ctrl-C stops both the door and the database."
wait "$door"
