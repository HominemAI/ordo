#!/usr/bin/env bash
# Write Postgres's view of every connection and every lock wait to a file.
#
#   scripts/pg-snapshot.sh <out-file> [label]
#
# For the end-to-end stacks and their CI jobs: when sign-ins fail with
# `server_busy`, this says what each backend was doing, what it waited on,
# who blocked it, and which locks were waited for (board task
# `store-pool-held-blocks-signin`). The door logs the same view itself at the
# moment its store pool refuses (`ORDO_PG_BUSY_SNAPSHOT`); this one is taken
# from outside, when the stack stops or a job fails.
#
# Reads the usual libpq variables (PGHOST, PGPORT, PGUSER, PGPASSWORD) or
# ORDO_PG_HOST / ORDO_PG_PORT / ORDO_PG_USER / ORDO_PG_PASSWORD, and ORDO_PSQL
# for the psql binary. Statements are shown without their literals and cut to
# 200 characters. Bounded: 5 s to connect, 5 s per statement. Never fails the
# caller: a snapshot that cannot be taken says why in the file.
set -uo pipefail

out=${1:?usage: pg-snapshot.sh <out-file> [label]}
label=${2:-snapshot}
psql_bin=${ORDO_PSQL:-$(command -v psql || echo /opt/homebrew/opt/postgresql@17/bin/psql)}
export PGHOST=${PGHOST:-${ORDO_PG_HOST:-localhost}}
export PGPORT=${PGPORT:-${ORDO_PG_PORT:-5432}}
export PGUSER=${PGUSER:-${ORDO_PG_USER:-ordo}}
export PGPASSWORD=${PGPASSWORD:-${ORDO_PG_PASSWORD:-}}
export PGCONNECT_TIMEOUT=5
export PGOPTIONS='-c statement_timeout=5000'
export PGAPPNAME='ordo: pg-snapshot'

{
  echo "== $label at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "== pg_stat_activity (every client backend; * marks the door's store pool)"
  "$psql_bin" -d postgres -X -q -P pager=off -c "
    SELECT CASE WHEN application_name = 'ordo: store pool' THEN '*' ELSE '' END AS m,
           pid, datname, application_name AS app, backend_type, state,
           wait_event_type || '/' || wait_event AS wait,
           (extract(epoch FROM now() - xact_start) * 1000)::bigint AS xact_ms,
           (extract(epoch FROM now() - query_start) * 1000)::bigint AS query_ms,
           pg_blocking_pids(pid) AS blocked_by,
           left(regexp_replace(left(query, 4000), '''([^'']|'''')*''?', '''?''', 'g'), 200) AS sql
      FROM pg_stat_activity
     WHERE pid <> pg_backend_pid()
     ORDER BY cardinality(pg_blocking_pids(pid)) DESC, query_start NULLS LAST" 2>&1
  echo "== pg_locks (waits, and every lock their blockers hold)"
  "$psql_bin" -d postgres -X -q -P pager=off -c "
    SELECT l.pid, l.locktype, l.mode, l.granted, d.datname, l.relation, l.transactionid,
           l.objid, pg_blocking_pids(l.pid) AS blocked_by
      FROM pg_locks l LEFT JOIN pg_database d ON d.oid = l.database
     WHERE NOT l.granted
        OR l.pid IN (SELECT unnest(pg_blocking_pids(w.pid)) FROM pg_locks w WHERE NOT w.granted)
     ORDER BY l.granted, l.pid
     LIMIT 200" 2>&1
} >>"$out"
exit 0
