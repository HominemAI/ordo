#!/usr/bin/env bash
# Rehearse an upgrade on a copy of a live deployment (identity service plan
# 6.1 and step 9), ending with the deployment owner's real passkey signing
# in. Run by the deployment owner, on their own computer, never on the
# deployment's host.
#
#   OLD_IMAGE=<the image the deployment runs> NEW_IMAGE=<the image to move to> \
#     bash scripts/rehearse/live-copy.sh <copy folder>
#
# THE COPY FOLDER. Taken from the deployment as docs/first-deployment.md
# step 8 and docs/howto-identity.md section 11 back it up, from the folder
# that holds its compose file (project `ordo-one`):
#
#   compose.yml        the compose file, exactly as it runs
#   secrets/           a copy of the folder its `secrets:` name (./secrets)
#   deployment-db.sql  docker compose exec -T deployment-db pg_dumpall -U ordo
#   console-db.sql     docker compose exec -T console-db pg_dumpall -U console
#   volumes/<v>.tar    each named volume the `ordo` service mounts, at least
#                      console-data; identity-data and supervisor-data too
#                      when they exist:
#                        docker run --rm -v ordo-one_<v>:/v:ro -v "$PWD/volumes:/out" \
#                          busybox:1.36 tar -C /v -cf /out/<v>.tar .
#
# The copy holds the deployment's keys. Keep it on an encrypted disk and
# delete it after. This script copies it into a private temporary folder,
# never changes the copy folder, and removes everything it made at the end.
#
# WHAT IT DOES, each step checked:
#   0. Restores the 2 database dumps and the volumes into a throwaway
#      compose project of its own (ordo-rehearse-live-copy).
#   1. Starts the copy on OLD_IMAGE: the console and the deployment serve,
#      and the active program version is read from the deployment database.
#   2. Moves it to NEW_IMAGE with the same files: it serves, the same
#      program is active, and you sign in with your passkey.
#   3. Turns on the identity service with
#      ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true (unless the compose file
#      already names an identity database): the console's owners are moved
#      in, and you sign in again, at /identity, with the same passkey.
#   After each sign-in it reads the copy's own database to confirm the
#   sign-in happened there: a sign-in on the live site cannot pass.
#
# NO ROUTE OUT. The copy's containers sit on an internal Docker network: no
# mail, no file store, no outside sign-in provider and no other server can
# be reached from them, so nothing the copy does touches the deployment it
# came from or anyone's data. A small proxy (caddy) is the only container
# with a published port, on 127.0.0.1.
#
# THE PASSKEY'S DOMAIN. A passkey only signs in on its relying party's
# host (ORDO_CONSOLE_RP_ID, or the host of ORDO_CONSOLE_ORIGIN).
#   - localhost: the copy is served at http://localhost:$COPY_PORT (default
#     18381). A passkey made under `localhost` works on any port, so
#     nothing else is needed. ORDO_CONSOLE_ORIGIN is set to that address
#     for the copy only; the relying party stays `localhost`.
#   - a real host, such as console.example.com: the copy must be served as
#     https://console.example.com on this computer. Before running:
#       a. brew install mkcert && mkcert -install   (once; trusts a local
#          certificate authority on this computer only)
#       b. add to /etc/hosts:   127.0.0.1 console.example.com
#          then quit and reopen the browser.
#     The script refuses to start until the host resolves to 127.0.0.1, so
#     the browser cannot reach the live site by mistake. Afterwards remove
#     the line (and `mkcert -uninstall` if you like).
#
# Options: PASSKEY=skip runs without the sign-in steps (no terminal, or no
# owner at hand); KEEP=1 leaves the copy running; COPY_PORT and DOOR_PORT
# (default 18380) are the published ports in localhost mode.
# Needs docker compose 2.24 or later, curl and python3.
set -euo pipefail

copy=${1:?usage: OLD_IMAGE=… NEW_IMAGE=… bash scripts/rehearse/live-copy.sh <copy folder>}
old_image=${OLD_IMAGE:?set OLD_IMAGE to the image the deployment runs now}
new_image=${NEW_IMAGE:?set NEW_IMAGE to the image to upgrade to}
project=ordo-rehearse-live-copy
cport=${COPY_PORT:-18381}
dport=${DOOR_PORT:-18380}
say() { printf '\n== %s\n' "$*"; }
die() { echo "live-copy: $*" >&2; exit 1; }

# ── the inputs ───────────────────────────────────────────────────────────
copy=$(cd "$copy" && pwd)
for f in compose.yml secrets deployment-db.sql console-db.sql volumes; do
  [ -e "$copy/$f" ] || die "the copy folder has no $f (see this script's header)"
done
[ -f "$copy/volumes/console-data.tar" ] || die "the copy folder has no volumes/console-data.tar: the console's key is in it"
v=$(docker compose version --short 2>/dev/null || echo 0)
python3 - "$v" <<'PY' || die "docker compose is $v; 2.24 or later is needed (for !reset)"
import sys
parts = [int(x) for x in (sys.argv[1].lstrip("v").split(".") + ["0", "0"])[:2] if x.isdigit()]
sys.exit(0 if tuple(parts) >= (2, 24) else 1)
PY

# The console's origin and relying party, as the compose file sets them.
setting() { { grep -E "^[[:space:]]*$1:" "$copy/compose.yml" || true; } | head -1 | sed -E "s/^[^:]*:[[:space:]]*//; s/[\"']//g; s/[[:space:]]*(#.*)?$//"; }
live_origin=$(setting ORDO_CONSOLE_ORIGIN)
[ -n "$live_origin" ] || die "compose.yml sets no ORDO_CONSOLE_ORIGIN"
rp_id=$(setting ORDO_CONSOLE_RP_ID)
read -r scheme host port < <(python3 - "$live_origin" <<'PY'
import sys
from urllib.parse import urlsplit
u = urlsplit(sys.argv[1])
print(u.scheme, u.hostname, u.port or (443 if u.scheme == "https" else 80))
PY
)
rp_id=${rp_id:-$host}
identity_named=0
grep -Eq '^[[:space:]]*(ORDO_IDENTITY_DATABASE_URL(_FILE)?|ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER):' "$copy/compose.yml" && identity_named=1

if [ "$rp_id" = localhost ]; then
  mode=localhost
  origin="http://localhost:$cport"
  door="http://127.0.0.1:$dport"
  curl_to() { curl -s "$@"; }
else
  mode=domain
  origin="$live_origin"
  door="http://127.0.0.1:$dport"
  command -v mkcert >/dev/null || die "the passkeys belong to $rp_id: install mkcert and run \`mkcert -install\` once (header, step a)"
  resolved=$(python3 -c 'import socket,sys; print(socket.gethostbyname(sys.argv[1]))' "$host" 2>/dev/null || true)
  [ "$resolved" = 127.0.0.1 ] || die "$host resolves to ${resolved:-nothing}, not 127.0.0.1. Add \`127.0.0.1 $host\` to /etc/hosts first (header, step b), so nothing here can reach the live site"
  # Every request this script makes goes to this computer, whatever DNS says.
  curl_to() { curl -s --resolve "$host:$port:127.0.0.1" "$@"; }
fi
for p in "$cport" "$dport" $([ "$mode" = domain ] && echo "$port"); do
  if (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then die "port $p on 127.0.0.1 is in use"; fi
done

# ── the throwaway project ────────────────────────────────────────────────
work=$(mktemp -d "${TMPDIR:-/tmp}/ordo-live-copy.XXXXXX")
chmod 0700 "$work"
cp "$copy/compose.yml" "$work/compose.yml"
cp -R "$copy/secrets" "$work/secrets"
# Readable inside the containers, whatever owner the copy has here. The
# folder itself is 0700 and removed at the end.
chmod 0444 "$work/secrets"/*
dc() { docker compose -p "$project" --project-directory "$work" -f "$work/compose.yml" -f "$work/copy.yml" "$@"; }
cleanup() {
  if [ "${KEEP:-0}" = 1 ]; then echo "kept: docker compose -p $project, files in $work"; return; fi
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; dc logs --no-color ordo 2>&1 | tail -60 >&2; exit 1; }

services=$(docker compose -f "$work/compose.yml" --project-directory "$work" config --services)
for s in ordo deployment-db console-db; do
  grep -qx "$s" <<<"$services" || die "compose.yml has no service \`$s\`; this script expects docker/compose.one.yml's services"
done

if [ "$mode" = domain ]; then
  mkdir -p "$work/certs"
  mkcert -cert-file "$work/certs/cert.pem" -key-file "$work/certs/key.pem" "$host" >/dev/null 2>&1 \
    || die "mkcert could not make a certificate for $host"
  chmod 0444 "$work/certs"/*
  cat > "$work/Caddyfile" <<EOF
{
	admin off
	auto_https disable_redirects
}
https://$host:$port {
	tls /certs/cert.pem /certs/key.pem
	reverse_proxy ordo:8081
}
http://:$dport {
	reverse_proxy ordo:8080
}
EOF
  publish="[\"127.0.0.1:$port:$port\", \"127.0.0.1:$dport:$dport\"]"
else
  cat > "$work/Caddyfile" <<EOF
{
	admin off
}
http://:$cport {
	reverse_proxy ordo:8081
}
http://:$dport {
	reverse_proxy ordo:8080
}
EOF
  publish="[\"127.0.0.1:$cport:$cport\", \"127.0.0.1:$dport:$dport\"]"
fi

# The override: the image of the stage, no published ports but the
# proxy's, every service on the internal network, and the database URL
# files on a volume with the owner and mode the image expects.
write_override() { # image share(0|1)
  {
    echo "services:"
    echo "  ordo:"
    echo "    image: $1"
    echo "    ports: !reset []"
    echo "    networks: [inside]"
    echo "    environment:"
    [ "$mode" = localhost ] && echo "      ORDO_CONSOLE_ORIGIN: $origin"
    echo "      ORDO_CONSOLE_DATABASE_URL_FILE: /run/copy-secrets/console-database-url"
    [ -f "$work/secrets/identity-database-url" ] && grep -q 'ORDO_IDENTITY_DATABASE_URL_FILE' "$work/compose.yml" \
      && echo "      ORDO_IDENTITY_DATABASE_URL_FILE: /run/copy-secrets/identity-database-url"
    [ "$2" = 1 ] && echo "      ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER: \"true\""
    echo "    volumes: [copy-secrets:/run/copy-secrets:ro]"
    for s in $services; do
      [ "$s" = ordo ] && continue
      echo "  $s:"
      echo "    ports: !reset []"
      echo "    networks: [inside]"
    done
    echo "  copy-proxy:"
    echo "    image: caddy:2.8.4"
    echo "    networks: [inside, outside]"
    echo "    ports: $publish"
    echo "    volumes:"
    echo "      - ./Caddyfile:/etc/caddy/Caddyfile:ro"
    [ "$mode" = domain ] && echo "      - ./certs:/certs:ro"
    echo "networks:"
    echo "  inside: { internal: true }"
    echo "  outside: {}"
    echo "volumes:"
    echo "  copy-secrets:"
  } > "$work/copy.yml"
}
write_override "$old_image" 0

# ── 0. restore ───────────────────────────────────────────────────────────
say "0. restoring the copy into project $project"
dc create >/dev/null 2>&1 || fail "the compose files do not load: $(dc config 2>&1 | tail -5)"
docker run --rm -v "${project}_copy-secrets:/s" -v "$work/secrets:/in:ro" busybox:1.36 sh -c '
  cp /in/console-database-url /s/ && chown 65532:65532 /s/console-database-url && chmod 0400 /s/console-database-url
  if [ -f /in/identity-database-url ]; then
    cp /in/identity-database-url /s/ && chown 65530:65530 /s/identity-database-url && chmod 0400 /s/identity-database-url
  fi' || fail "could not place the database URL files"
for tarfile in "$copy/volumes"/*.tar; do
  name=$(basename "$tarfile" .tar)
  vol="${project}_$name"
  docker volume inspect "$vol" >/dev/null 2>&1 || die "volumes/$name.tar has no volume \`$name\` in compose.yml"
  docker run --rm -v "$vol:/v" -v "$copy/volumes:/in:ro" busybox:1.36 tar -C /v -xpf "/in/$name.tar" \
    || fail "could not restore volume $name"
  echo "volume $name restored"
done
dc up -d --wait deployment-db console-db >/dev/null 2>&1 || fail "the databases did not come up"
restore() { # service dump
  dc exec -T "$1" sh -c 'psql -X -q -U "$POSTGRES_USER" -d postgres' < "$2" > /dev/null 2> "$work/$1.err" || true
  # The empty cluster already has its superuser and first database.
  if grep 'ERROR' "$work/$1.err" | grep -v 'already exists' | grep -q .; then
    grep 'ERROR' "$work/$1.err" | grep -v 'already exists' | head -10 >&2
    fail "restoring $2 into $1 failed"
  fi
  echo "$1 restored from $(basename "$2")"
}
restore deployment-db "$copy/deployment-db.sql"
restore console-db "$copy/console-db.sql"

sql() { # service database query -> one value
  dc exec -T "$1" sh -c "psql -X -q -t -A -U \"\$POSTGRES_USER\" -d $2" <<<"$3" 2>/dev/null | head -1
}
deployment_db=$(dc exec -T deployment-db sh -c 'echo "$POSTGRES_DB"' | tr -d '\r')
active() { sql deployment-db "$deployment_db" 'SELECT version FROM "$activations" ORDER BY seq DESC LIMIT 1'; }
owners=$(sql console-db ordo_console 'SELECT count(*) FROM owner WHERE removed_at IS NULL')
echo "the copy holds $owners live console owners; active program: $(active || echo none)"

serves() { # what
  for _ in $(seq 1 180); do
    if curl_to -f "$origin/health/ready" 2>/dev/null | grep -q '"deployment":"ready"' \
      && curl_to -f -o /dev/null "$door/health/ready" 2>/dev/null; then
      return 0
    fi
    sleep 1
  done
  fail "$1: the console or the door did not answer ready"
}

ask_signin() { # where
  [ "${PASSKEY:-ask}" = skip ] && { echo "PASSKEY=skip: no sign-in at this step"; return 1; }
  # For testing this script: a command signs in instead of a person, with
  # COPY_ORIGIN and COPY_STAGE (console or identity) in its environment.
  if [ -n "${SIGNIN_CMD:-}" ]; then
    COPY_ORIGIN=$origin COPY_STAGE=$stage bash -c "$SIGNIN_CMD" || fail "SIGNIN_CMD failed"
    return 0
  fi
  [ -r /dev/tty ] || { echo "no terminal: no sign-in at this step (PASSKEY=skip to say so)"; return 1; }
  printf '\nOpen %s/ in your browser and sign in with your passkey%s.\nPress Enter when you are signed in, or type s and Enter to skip. ' "$origin" "$1"
  read -r answer < /dev/tty
  [ "$answer" != s ]
}

# ── 1. the old image ─────────────────────────────────────────────────────
say "1. the copy on $old_image"
dc up -d >/dev/null 2>&1 || fail "the old image did not come up"
serves "on the old image"
before=$(active || true)
echo "it serves; active program: ${before:-none}"

# ── 2. the new image, the same files ─────────────────────────────────────
say "2. the copy on $new_image, the same compose file"
write_override "$new_image" 0
dc up -d >/dev/null 2>&1 || fail "the new image did not come up"
serves "on the new image"
[ "$(active || true)" = "$before" ] || fail "the active program changed: was ${before:-none}, is $(active || echo none)"
echo "it serves, with the same program active"
proven=()
t0=$(date +%s)
stage=console
[ "$identity_named" = 1 ] && stage=identity
if ask_signin ""; then
  if [ "$identity_named" = 1 ]; then
    n=$(sql console-db ordo_identity "SELECT count(*) FROM audit WHERE action = 'sign-in' AND outcome = 'ok' AND at >= $t0")
  else
    n=$(sql console-db ordo_console "SELECT count(*) FROM passkeys WHERE last_used_at >= $t0")
  fi
  [ "${n:-0}" -ge 1 ] || fail "the copy recorded no passkey sign-in since step 2 began: were you on $origin, served by this computer?"
  echo "the copy recorded your passkey sign-in"
  proven+=("step 2: your passkey signed in on the new image")
fi

# ── 3. the identity service ──────────────────────────────────────────────
if [ "$identity_named" = 0 ] && [ "${IDENTITY:-share}" = share ]; then
  say "3. ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true"
  write_override "$new_image" 1
  dc up -d >/dev/null 2>&1 || fail "the opt-in did not come up"
  serves "with the identity service"
  for _ in $(seq 1 120); do
    dc logs --no-color ordo 2>&1 | grep -q "moved the console's owners in" && break
    sleep 1
  done
  moved=$(dc logs --no-color ordo 2>&1 | grep -o "moved the console's owners in: [^.]*" | tail -1)
  [ -n "$moved" ] || fail "the identity service did not move the console's owners in"
  echo "$moved"
  t1=$(date +%s)
  stage=identity
  if ask_signin " (the console sends you to /identity)"; then
    n=$(sql console-db ordo_identity "SELECT count(*) FROM audit WHERE action = 'sign-in' AND outcome = 'ok' AND at >= $t1 AND actor IN (SELECT id FROM people WHERE console_owner_id IS NOT NULL)")
    [ "${n:-0}" -ge 1 ] || fail "the copy's identity service recorded no sign-in by a moved-in owner since step 3 began"
    echo "the copy's identity service recorded your sign-in, as a moved-in owner"
    proven+=("step 3: the same passkey signed in at the identity service")
  fi
fi

say "PASS: the copy upgrades and serves"
if [ ${#proven[@]} -eq 0 ]; then
  echo "Not proven here: a real passkey signing in (no sign-in was made)."
else
  printf '  %s\n' "${proven[@]}"
fi
