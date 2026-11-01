#!/usr/bin/env bash
# Lease one of the CI Postgres servers on the cluster for this job.
#
# The servers are a StatefulSet in smg_infrastructure
# (gha-runner/ci-postgres.yaml): ci-postgres-0 to ci-postgres-<N-1>, one per
# runner. Owner ruling 2026-11-01, board task
# ci-postgres-a-dedicated-instance-on.
#
# A lease is a Postgres advisory lock held by a psql left running in the
# background. It lasts until the job ends: the runner kills leftover
# processes, or the pod goes and the server's TCP keepalive drops the
# session. Either way the lock is freed with no cleanup step.
#
# Once leased, every database the last job left is dropped and
# ordo_conformance is made fresh. Tests use fixed database names, so a job
# needs a clean server to itself.
#
# Writes the server's address as ORDO_PG_HOST to $GITHUB_ENV (or prints it
# outside Actions).
#
# Settings:
#   ORDO_CI_PG_SERVERS  how many servers there are (default 8)
#   ORDO_CI_PG_SERVICE  the StatefulSet's name (default ci-postgres)
#   ORDO_CI_PG_WAIT     seconds to keep trying before giving up (default 900)
#   PGUSER, PGPASSWORD  as for psql (default ordo/ordo)
set -euo pipefail

servers=${ORDO_CI_PG_SERVERS:-8}
service=${ORDO_CI_PG_SERVICE:-ci-postgres}
wait_for=${ORDO_CI_PG_WAIT:-900}
export PGUSER=${PGUSER:-ordo}
export PGPASSWORD=${PGPASSWORD:-ordo}
export PGPORT=${PGPORT:-5432}
export PGCONNECT_TIMEOUT=5

dir=${RUNNER_TEMP:-${TMPDIR:-/tmp}}/ci-pg-lease
mkdir -p "$dir"

# Any key works as long as every job uses the same one.
key=42710

try_server() {
  local host=$1 log=$dir/$1.log
  : >"$log"
  # The session prints `leased` and then sleeps, holding the lock, or
  # prints `busy` and exits. setsid keeps it alive after this step ends;
  # -w keeps setsid as its parent, so $! stays the holder.
  setsid -w nohup psql -X -h "$host" -d postgres -At >"$log" 2>&1 <<SQL &
SELECT pg_try_advisory_lock($key) AS got \gset
\if :got
\echo leased
SELECT pg_sleep(86400);
\else
\echo busy
\endif
SQL
  local pid=$!
  for _ in $(seq 1 30); do
    if grep -qx leased "$log"; then
      echo "$pid" >"$dir/holder.pid"
      return 0
    fi
    if grep -qx busy "$log" || ! kill -0 "$pid" 2>/dev/null; then
      return 1
    fi
    sleep 0.5
  done
  kill "$pid" 2>/dev/null || true
  return 1
}

deadline=$((SECONDS + wait_for))
start=$((RANDOM % servers))
host=
while [ -z "$host" ]; do
  for i in $(seq 0 $((servers - 1))); do
    candidate=$service-$(((start + i) % servers)).$service
    if try_server "$candidate"; then
      host=$candidate
      break
    fi
  done
  if [ -z "$host" ]; then
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "No CI Postgres server was free for ${wait_for}s." >&2
      for f in "$dir"/*.log; do echo "--- $f"; cat "$f"; done >&2
      exit 1
    fi
    sleep 10
  fi
done
echo "Leased $host"

# Clear what the last job left. DROP DATABASE cannot run in a transaction,
# so \gexec runs each statement on its own.
count=$(psql -X -h "$host" -d postgres -At -c \
  "SELECT count(*) FROM pg_database WHERE datname NOT IN ('postgres','template0','template1')")
psql -X -q -h "$host" -d postgres -v ON_ERROR_STOP=1 <<'SQL'
SELECT format('DROP DATABASE %I WITH (FORCE)', datname) FROM pg_database
 WHERE datname NOT IN ('postgres','template0','template1') \gexec
CREATE DATABASE ordo_conformance;
SQL
echo "Dropped $count database(s) left on $host"

# Hand the tests the server's address, not its name. On 2026-11-01 (run
# 41285450540) hundreds of test psql calls resolving the name at once got
# "could not translate host name" from the cluster DNS in two jobs at the
# same second. The address lasts as long as the pod, and so does its data.
addr=$(getent hosts "$host" | awk '{print $1; exit}')
if [ -z "$addr" ]; then
  echo "Could not resolve $host." >&2
  exit 1
fi
echo "$host is $addr"

if [ -n "${GITHUB_ENV:-}" ]; then
  echo "ORDO_PG_HOST=$addr" >>"$GITHUB_ENV"
  echo "PGHOST=$addr" >>"$GITHUB_ENV"
else
  echo "ORDO_PG_HOST=$addr"
fi
