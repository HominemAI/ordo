#!/usr/bin/env bash
# Rehearse the supervisor (identity service plan step 7, section 8
# "Supervisor"): kill each child of the one image in turn, with SIGKILL,
# and check that the supervisor starts it again while the other 2 keep
# running the same process.
#
#   IMAGE=ordo:dev bash scripts/rehearse/kill-each-child.sh
#
# It runs its own Postgres and the image on its own docker network, with
# the least the image needs (4 capabilities, a read-only root, the run
# folder on a tmpfs). Processes are seen and killed from a busybox
# container that shares the image's process namespace, since the image is
# distroless. It removes only what it made.
set -euo pipefail

image=${IMAGE:?set IMAGE to the one image}
name=ordo-rehearse-kill
net=$name-net
fail() { echo "FAIL: $*" >&2; docker logs "$name" 2>&1 | tail -60 >&2; exit 1; }
cleanup() {
  docker rm -f "$name" "$name-db" >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup

docker network create "$net" >/dev/null
docker run -d --name "$name-db" --network "$net" \
  -e POSTGRES_USER=ordo -e POSTGRES_PASSWORD=ordo -e POSTGRES_DB=ordo_console postgres:17 >/dev/null
for _ in $(seq 1 60); do
  docker exec "$name-db" pg_isready -U ordo >/dev/null 2>&1 && break
  sleep 1
done
docker exec "$name-db" pg_isready -U ordo >/dev/null 2>&1 || fail "Postgres did not start"

docker run -d --name "$name" --network "$net" \
  --cap-drop ALL --cap-add SETUID --cap-add SETGID --cap-add SETPCAP --cap-add CHOWN \
  --read-only --tmpfs /run/ordo:uid=65530,mode=0711 --tmpfs /tmp:noexec,nosuid,nodev \
  -e ORDO_CONSOLE_DATABASE_URL=postgres://ordo:ordo@$name-db:5432/ordo_console \
  -e ORDO_CONSOLE_ORIGIN=http://localhost:8081 \
  -e ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true \
  -e ORDO_PG_HOST=$name-db -e ORDO_PG_USER=ordo -e ORDO_PG_DB=postgres -e ORDO_PG_PASSWORD=ordo \
  -e ORDO_SPACE=rehearse -e ORDO_KEY_PROVIDER=test -e ORDO_DEV_UNSAFE_TEST_KEYS=1 \
  -e ORDO_TOKEN_KEY=1111111111111111111111111111111111111111111111111111111111111111 \
  -e ORDO_PAGE_KEY=2222222222222222222222222222222222222222222222222222222222222222 \
  "$image" >/dev/null

# Inside the image's namespaces: its processes and its own /health/live.
inside() { docker run --rm --pid "container:$name" --network "container:$name" busybox:1.36 sh -c "$1"; }
live() { inside 'wget -qO- http://127.0.0.1:8079/health/live' 2>/dev/null || true; }
# The child itself: its program with no argument. The relay in front of
# it is `ordo-supervisor --supervised-child ...`, so it never matches.
pid_of() { inside "ps -o pid,args | awk '\$2 == \"/usr/local/bin/$1\" && NF == 2 { print \$1; exit }'"; }
all_ready() {
  local l
  l=$(live)
  for c in identity console deployment; do
    echo "$l" | grep -q "\"$c\":\"ready\"" || return 1
  done
}

for _ in $(seq 1 180); do all_ready && break; sleep 1; done
all_ready || fail "the 3 children were never ready: $(live)"
echo "3 children ready"

for victim in ordo-identity-service ordo-console ordo-host; do
  before=$(pid_of "$victim")
  [ -n "$before" ] || fail "no $victim process: $(inside 'ps -o pid,user,comm,args')"
  others=()
  for other in ordo-identity-service ordo-console ordo-host; do
    [ "$other" = "$victim" ] || others+=("$other=$(pid_of "$other")")
  done
  inside "kill -9 $before"
  back=""
  for _ in $(seq 1 120); do
    now=$(pid_of "$victim" || true)
    if [ -n "$now" ] && [ "$now" != "$before" ] && all_ready; then back=$now; break; fi
    sleep 1
  done
  [ -n "$back" ] || fail "$victim was not started again: $(live)"
  for o in "${others[@]}"; do
    [ "$(pid_of "${o%%=*}")" = "${o#*=}" ] || fail "${o%%=*} was restarted with $victim"
  done
  echo "killed $victim (pid $before): back as pid $back; the other 2 kept their process"
done
docker logs "$name" > /dev/null 2>&1
echo "PASS: each child, killed in turn, came back alone"
