#!/usr/bin/env bash
# Repeat the store pool stress test at several shapes and say what it saw.
#
#   scripts/stress-store-pool.sh [runs-per-shape] [shape ...]
#
# A shape is WORKERS:PEOPLE:CONCURRENCY, the door runtime's worker threads,
# the people who sign up (each a tenant) and the client threads at once.
# The default shapes are 2:40:8 4:40:16 1:30:8 2:60:24, two runs each.
#
#   scripts/stress-store-pool.sh 5 2:40:8          # five runs of one shape
#   STRESS_SECS=120 scripts/stress-store-pool.sh   # a shorter bound per run
#
# Board task `store-pool-held-blocks-signin`: in CI the door's one-connection
# store pool stayed in use for about 2 minutes and every emailed sign-in got
# `503 server_busy`. The test (crates/ordo-host/tests/store_pool_stress.rs,
# ignored in the normal suite) recreates the e2e's shape in one process.
#
# It starts a Postgres cluster of its own ($HOME/.ordo-pg/stress-store-pool on
# port 55493, per scripts/clean-test-clusters.sh), so other work is not
# disturbed, and stops it at the end. Every run is bounded: STRESS_SECS (240)
# for the load plus 180 s, after which the run is killed. Logs go to
# $STRESS_LOGS (default: a new folder under ${TMPDIR:-/tmp}).
#
# As CI connects (STRESS_TCP=1, the default): over TCP to localhost with a
# SCRAM password, so every dial pays the handshake a debug build pays in CI.
# STRESS_TCP=0 uses a Unix socket with trust, as the local suites do.
set -uo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
runs=${1:-2}
shift || true
shapes=("$@")
[ ${#shapes[@]} -gt 0 ] || shapes=(2:40:8 4:40:16 1:30:8 2:60:24)

export ORDO_PGDATA=${STRESS_PGDATA:-$HOME/.ordo-pg/stress-store-pool}
export ORDO_PG_PORT=${STRESS_PG_PORT:-55493}
export ORDO_PG_HOST=/tmp/ordo-pg
export ORDO_PG_USER=ordo
tcp=${STRESS_TCP:-1}
pgbin=${ORDO_PGBIN:-/opt/homebrew/opt/postgresql@17/bin}
export ORDO_PG_DB=ordo_conformance
export STRESS_SECS=${STRESS_SECS:-240}
logs=${STRESS_LOGS:-$(mktemp -d "${TMPDIR:-/tmp}/stress-store-pool.XXXXXX")}
mkdir -p "$logs"

# One battery at a time: a second one would share the cluster, and its exit
# would stop the cluster under the first.
[ "$tcp" = 1 ] && data="$ORDO_PGDATA-tcp" || data="$ORDO_PGDATA"
if "$pgbin/pg_ctl" -D "$data" status >/dev/null 2>&1; then
  echo "the stress cluster ($data) is already running: another battery is using it" >&2
  exit 1
fi

stop_cluster() {
  "$pgbin/pg_ctl" -D "$ORDO_PGDATA" stop >/dev/null 2>&1 || true
}
trap stop_cluster EXIT
trap 'exit 130' INT TERM

if [ "$tcp" = 1 ]; then
  ORDO_PGDATA="$ORDO_PGDATA-tcp"
  if [ ! -d "$ORDO_PGDATA" ]; then
    pw=$(mktemp)
    echo ordo >"$pw"
    "$pgbin/initdb" -D "$ORDO_PGDATA" -U ordo --pwfile="$pw" \
      --auth-local=trust --auth-host=scram-sha-256 >"$logs/cluster.log" 2>&1
    rm -f "$pw"
  fi
  mkdir -p /tmp/ordo-pg
  "$pgbin/pg_ctl" -D "$ORDO_PGDATA" -o "-p $ORDO_PG_PORT -k /tmp/ordo-pg -h localhost" \
    -l "$ORDO_PGDATA/server.log" start >>"$logs/cluster.log" 2>&1 ||
    { echo "the cluster did not start: $logs/cluster.log" >&2; exit 1; }
  until "$pgbin/pg_isready" -h localhost -p "$ORDO_PG_PORT" -q; do sleep 0.2; done
  "$pgbin/createdb" -h /tmp/ordo-pg -p "$ORDO_PG_PORT" -U ordo "$ORDO_PG_DB" 2>/dev/null || true
  export ORDO_PG_HOST=localhost ORDO_PG_PASSWORD=ordo PGPASSWORD=ordo
else
  bash "$repo/scripts/conformance-pg.sh" start >"$logs/cluster.log" 2>&1 ||
    { echo "the cluster did not start: $logs/cluster.log" >&2; exit 1; }
fi

# Build once, then run the binary itself, so a killed run kills the test and
# not only cargo.
bin=$(cd "$repo" && cargo test -p ordo-host --test store_pool_stress --no-run --message-format=json 2>"$logs/build.log" |
  sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -1)
[ -x "$bin" ] || { echo "the test did not build: $logs/build.log" >&2; exit 1; }

limit=$((STRESS_SECS + 180))
total=0 failed=0 busy_runs=0
for shape in "${shapes[@]}"; do
  IFS=: read -r workers people concurrency <<<"$shape"
  for i in $(seq 1 "$runs"); do
    total=$((total + 1))
    log="$logs/run-$total-w$workers-p$people-c$concurrency.log"
    STRESS_WORKERS=$workers STRESS_PEOPLE=$people STRESS_CONCURRENCY=$concurrency \
      STRESS_DB="ordo_stress_$total" \
      perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$limit" \
      "$bin" --ignored --exact the_store_pool_stays_free_under_the_e2e_shape --nocapture \
      >"$log" 2>&1
    rc=$?
    busy=$(grep -c "store pool: server_busy" "$log")
    incident=$(grep -c "pool incident: end of postgres view" "$log")
    stalled=$(grep -c "STALLED: thread stacks" "$log")
    last=$(grep "stress: done" "$log" | tail -1 | sed 's/^stress: //')
    [ "$rc" = 0 ] || failed=$((failed + 1))
    [ "$busy" = 0 ] || busy_runs=$((busy_runs + 1))
    case $rc in
      0) verdict=passed ;;
      142) verdict="KILLED after ${limit}s" ;;
      *) verdict="FAILED ($rc)" ;;
    esac
    echo "run $total shape $shape: $verdict; busy lines $busy, incident blocks $incident, stalls $stalled; ${last:-no summary} ($log)"
  done
done
echo "$total runs, $failed failed, $busy_runs with a busy store pool. Logs: $logs"
[ "$failed" = 0 ]
