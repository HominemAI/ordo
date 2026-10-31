#!/bin/sh
# The notes deployment, on this machine.
#
# Starts a throwaway Postgres, boots `ordo-host` with the settings the README
# gives a deployment owner (passkey sign-in on `localhost`, a tenant of one's
# own for everyone who signs up, the page's origin allowed), applies
# `../notes.ordo`, and stays in the foreground until Ctrl-C.
#
#     sh examples/notes/dev/serve.sh
#
# Then, in another terminal:
#
#     cd examples/notes/web && npm install && npm run dev
#
# and open http://localhost:5173. Nothing here is fit for a real deployment:
# the key provider is the development one, the secrets are constants, and the
# database has no password. `../README.md` says what a real one sets.
#
# Needs: PostgreSQL 15 or later (`ORDO_PGBIN` names its bin directory), and
# `cargo build -p ordo-cli -p ordo-host` done (or `ORDO` and `ORDO_HOST` naming
# the two binaries).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
ordo=${ORDO:-"$repo/target/debug/ordo"}
host=${ORDO_HOST_BIN:-"$repo/target/debug/ordo-host"}
pgbin=${ORDO_PGBIN:-/opt/homebrew/opt/postgresql@17/bin}
data=${NOTES_DEV_DIR:-"$here/.data"}
# A Unix socket path is limited to about 100 bytes, so it is not under $data.
socket=${NOTES_DEV_SOCKET:-/tmp/ordo-notes-pg}
pgport=${NOTES_DEV_PG_PORT:-55490}
addr=${NOTES_DEV_ADDR:-127.0.0.1:4000}
page=${NOTES_DEV_PAGE:-http://localhost:5173}

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
"$pgbin/createdb" -h "$socket" -p "$pgport" -U ordo notes 2>/dev/null || true

# ── The settings ───────────────────────────────────────────────────────────
export ORDO_PG_HOST="$socket" ORDO_PG_PORT="$pgport" ORDO_PG_USER=ordo ORDO_PG_DB=notes
export ORDO_SPACE=system
export ORDO_KEY_PROVIDER=local ORDO_MASTER_KEY=notes-dev-master ORDO_DEV_UNSAFE_TEST_KEYS=1
export ORDO_TOKEN_KEY=1111111111111111111111111111111111111111111111111111111111111111
export ORDO_PAGE_KEY=2222222222222222222222222222222222222222222222222222222222222222
export ORDO_HOST_ADDR="$addr"
# Sign-in and sign-up with a passkey, relying party `localhost`. Set
# ORDO_AUTH_METHODS before running to offer more ways in (passkey,email).
export ORDO_AUTH_METHODS="${ORDO_AUTH_METHODS:-passkey}" ORDO_APEX_DOMAIN=localhost ORDO_AUTH_ORIGINS="$page"
# Anyone can sign up, and each person gets a tenant of their own.
export ORDO_AUTH_PROVISIONING=auto-onboard ORDO_PROVISION_SELF_SERVE=on-signup
# The page may call the door from its own address.
export ORDO_CORS_ORIGINS="$page"
# Files on notes (notes v2) go to a folder on this machine. `fs:` is for
# development only, so the door refuses it without the unsafe switch. A real
# deployment sets ORDO_BLOB_STORE=s3 and the ORDO_BLOB_S3_* settings.
export ORDO_BLOB_STORE="${ORDO_BLOB_STORE:-fs:$data/blobs}" ORDO_DEV_UNSAFE_BLOB_STORE=1
# Mail (emailed sign-in codes, and notes v2's reminders) goes to a mail
# catcher on this machine, in the clear, on port NOTES_DEV_SMTP_PORT. Run one
# there: Mailpit (`mailpit --smtp 127.0.0.1:4625`) or the end-to-end stack's
# own (`E2E_DOOR=http://$addr E2E_CLIENT_ID=x E2E_CLIENT_SECRET=x node
# examples/notes/e2e/stand-ins.mjs`, mail listed at http://localhost:4626/messages).
# With nothing listening, mail is retried and then dropped. A real deployment
# sets ORDO_MAIL_* for its relay over TLS (../README.md §6).
export ORDO_MAIL_KIND=smtp ORDO_MAIL_FROM=notes@localhost ORDO_MAIL_HOST=localhost
export ORDO_MAIL_PORT="${NOTES_DEV_SMTP_PORT:-4625}" ORDO_MAIL_TLS=none ORDO_MAIL_SEND_SECS=1
export ORDO_EGRESS_PRIVATE_HOSTS=localhost

# ── The door, then the program ─────────────────────────────────────────────
"$host" &
door=$!
trap 'kill "$door" 2>/dev/null || true; "$pgbin/pg_ctl" -D "$data/pg" stop >/dev/null 2>&1 || true' EXIT INT TERM

port=${addr##*:}
until curl -fsS "http://127.0.0.1:$port/health/live" >/dev/null 2>&1; do
  kill -0 "$door" 2>/dev/null || { echo "ordo-host stopped before it served" >&2; exit 1; }
  sleep 0.2
done

# A database from an earlier run may hold an older schema. `ordo apply` makes
# the safe additive changes itself (ORDO_APPLY_SAFE_CHANGES, on by default).
# A change that needs the app developer's say is refused, naming the command.
# NOTES_DEV_PROGRAM=examples/notes-v2 serves v2. A routine a program adds
# (v2's reminders) runs from the next start of the door, so start this
# script again after the first apply of v2.
"$ordo" apply "${NOTES_DEV_PROGRAM:-$repo/examples/notes}" --as notes-dev
echo
echo "The door is at http://127.0.0.1:$port and the page belongs at $page."
echo "Ctrl-C stops both the door and the database."
wait "$door"
