#!/bin/sh
# The marketing-site deployment, on this machine.
#
# Starts a throwaway Postgres, boots `ordo-host` with the settings a public
# site needs, applies `../site.ordo`, and stays in the foreground until Ctrl-C.
#
#     sh examples/marketing-site/dev/serve.sh
#
# Then, in another terminal:
#
#     sh examples/marketing-site/dev/seed          # fills the content
#     cd examples/marketing-site/web && npm install
#     npm run dev                                  # http://localhost:5183
#
# Nothing here is fit for a real deployment: the key provider is the
# development one, the secrets are constants, the database has no password,
# guest sign-in is on only so that `seed` can act as the site owner, and the
# human check on the contact form is off.
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
data=${SITE_DEV_DIR:-"$here/.data"}
# A Unix socket path is limited to about 100 bytes, so it is not under $data.
socket=${SITE_DEV_SOCKET:-/tmp/ordo-site-pg}
pgport=${SITE_DEV_PG_PORT:-55496}
addr=${SITE_DEV_ADDR:-127.0.0.1:4010}
page=${SITE_DEV_PAGE:-http://localhost:5183}

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
"$pgbin/createdb" -h "$socket" -p "$pgport" -U ordo site 2>/dev/null || true

# ── The settings ───────────────────────────────────────────────────────────
export ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pgport" ORDO_PG_USER=ordo ORDO_PG_DB=site
export ORDO_SPACE=system
export ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY=marketing-site-dev-master ORDO_DEV_UNSAFE_TEST_KEYS=1
export ORDO_TOKEN_KEY=1111111111111111111111111111111111111111111111111111111111111111
export ORDO_PAGE_KEY=2222222222222222222222222222222222222222222222222222222222222222
export ORDO_HOST_ADDR="$addr"
# Guest sign-in (ORDO_AUTH_GUEST) lets `seed` be the site owner with no passkey. The site owner
# gets a tenant of their own on first sign-in.
export ORDO_AUTH_METHODS="${ORDO_AUTH_METHODS:-passkey}" ORDO_AUTH_GUEST=on
export ORDO_APEX_DOMAIN=localhost ORDO_AUTH_ORIGINS="$page"
export ORDO_AUTH_PROVISIONING=auto-onboard ORDO_PROVISION_SELF_SERVE=on-signup
# The site owner owns the deployment's space (`system`), where public data
# lives. `seed` saves the owner's principal id on its first run; start this
# script again after that so the id is read.
if [ -s "$data/owner-principal" ]; then
  ORDO_DEPLOYMENT_OWNERS="$(cat "$data/owner-principal")"
  export ORDO_DEPLOYMENT_OWNERS
fi
# The contact form is a public write, and the human check is on by default.
# It is off here so the form works with no Turnstile account. A real deployment
# leaves this unset and sets ORDO_PUBLIC_HUMAN_SECRET_FILE to a file holding
# the Turnstile secret key instead.
export ORDO_PUBLIC_HUMAN_CHECK=off
# The page may call the door from its own address.
export ORDO_CORS_ORIGINS="$page"

# ── The door, then the program ─────────────────────────────────────────────
"$host" &
door=$!
trap 'kill "$door" 2>/dev/null || true; "$pgbin/pg_ctl" -D "$data/pg" stop >/dev/null 2>&1 || true' EXIT INT TERM

port=${addr##*:}
until curl -fsS "http://127.0.0.1:$port/health/live" >/dev/null 2>&1; do
  kill -0 "$door" 2>/dev/null || { echo "ordo-host stopped before it served" >&2; exit 1; }
  sleep 0.2
done

"$ordo" apply "$repo/examples/marketing-site" --as site-dev
echo
echo "The door is at http://127.0.0.1:$port and the page belongs at $page."
echo "Next: sh examples/marketing-site/dev/seed"
echo "Ctrl-C stops both the door and the database."
wait "$door"
