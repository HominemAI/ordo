#!/bin/sh
# The assistant deployment, on this machine.
#
# Starts a throwaway Postgres, boots `ordo-host` with the settings the README
# gives a deployment owner, applies `../assistant.ordo`, writes the program's
# interface descriptor for the agent, and stays in the foreground until
# Ctrl-C.
#
#     sh examples/assistant/dev/serve.sh
#
# Then, in another terminal:
#
#     node examples/assistant/dev/seed.mjs       # a manager, 3 staff, expenses
#     node examples/assistant/agent/assistant.mjs  # asks the manager, then waits
#     node examples/assistant/dev/approve.mjs    # the manager reads and approves
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
data=${ASSISTANT_DEV_DIR:-"$here/.data"}
# A Unix socket path is limited to about 100 bytes, so it is not under $data.
socket=${ASSISTANT_DEV_SOCKET:-/tmp/ordo-assistant-pg}
pgport=${ASSISTANT_DEV_PG_PORT:-55498}
addr=${ASSISTANT_DEV_ADDR:-127.0.0.1:4008}

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
"$pgbin/createdb" -h "$socket" -p "$pgport" -U ordo assistant 2>/dev/null || true

# ── The settings ───────────────────────────────────────────────────────────
export ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pgport" ORDO_PG_USER=ordo ORDO_PG_DB=assistant
export ORDO_SPACE=system
export ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY=assistant-dev-master ORDO_DEV_UNSAFE_TEST_KEYS=1
export ORDO_TOKEN_KEY=7777777777777777777777777777777777777777777777777777777777777777
export ORDO_PAGE_KEY=8888888888888888888888888888888888888888888888888888888888888888
export ORDO_HOST_ADDR="$addr"
# The manager signs up as a guest, so a script can stand in for a person.
# A founder gets a tenant of their own; colleagues come in by invitation,
# which `std.space.invite` makes without a passkey touch here. A real
# deployment keeps passkeys and leaves both of those off.
export ORDO_AUTH_METHODS="${ORDO_AUTH_METHODS:-passkey}" ORDO_APEX_DOMAIN=localhost ORDO_AUTH_ORIGINS=http://localhost:5175
export ORDO_AUTH_PROVISIONING=auto-onboard ORDO_PROVISION_SELF_SERVE=on-signup
# The manager is a guest, and a guest holds no passkey, so the approval of a
# delegation is exempted from the passkey touch here too. A real deployment
# leaves `std.delegation.approve` asking for presence.
export ORDO_AUTH_GUEST=on ORDO_STEP_UP_EXEMPT_OPS=std.space.invite,std.delegation.approve
# The agents this deployment accepts delegation requests from, and the name
# the manager sees on the review. Unset, no agent may ask.
export ORDO_DELEGATION_AGENTS="agt_assistant=Expense assistant"
# The longest delegation the manager may grant, in hours. 8 is the default.
export ORDO_DELEGATION_MAX_HOURS=8
# Which delete ops a delegated token may call. The assistant deletes nothing,
# so no delete is opened: every delete and every platform write stays closed
# to it. Approving and listing are not deletes and need no entry here.
export ORDO_DELEGABLE_OPS=

# ── The door, then the program ─────────────────────────────────────────────
"$host" &
door=$!
trap 'kill "$door" 2>/dev/null || true; "$pgbin/pg_ctl" -D "$data/pg" stop >/dev/null 2>&1 || true' EXIT INT TERM

port=${addr##*:}
until curl -fsS "http://127.0.0.1:$port/health/live" >/dev/null 2>&1; do
  kill -0 "$door" 2>/dev/null || { echo "ordo-host stopped before it served" >&2; exit 1; }
  sleep 0.2
done

"$ordo" apply "$repo/examples/assistant" --as assistant-dev
# The agent builds its calls from this descriptor.
"$ordo" interface "$repo/examples/assistant" --out "$data"
echo
echo "The door is at http://127.0.0.1:$port."
echo "Ctrl-C stops both the door and the database."
wait "$door"
