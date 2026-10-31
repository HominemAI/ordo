#!/usr/bin/env bash
# Turn off synchronous_commit on a CI job's throwaway Postgres service.
#
#   scripts/ci-pg-fast-commits.sh
#
# A commit then returns before its WAL is flushed to disk. The service is
# thrown away with the job, so nothing durable is lost. What it removes is
# runner disk noise: one fsync took 2 s on a loaded runner (e2e run
# 40154332786) and a sign-in failed behind it.
#
# **The product fix is the real one** (board task
# `store-pool-held-blocks-signin`): the door's store pool has more than one
# connection and a wait that rides out a slow commit
# (`ORDO_PG_STORE_POOL_SIZE`, `ORDO_PG_STORE_ACQUIRE_TIMEOUT_MS`), because
# cloud disks stall like this in production too. This only keeps CI from
# testing the runner's disk.
#
# The setting is reloadable, so no restart. Reads ORDO_PG_HOST, ORDO_PG_PORT,
# ORDO_PG_USER and ORDO_PG_PASSWORD, and ORDO_PSQL for the psql binary.
set -euo pipefail

psql=${ORDO_PSQL:-psql}
export PGPASSWORD=${PGPASSWORD:-${ORDO_PG_PASSWORD:-}}
args=(-X -v ON_ERROR_STOP=1 -h "${ORDO_PG_HOST:-localhost}" -p "${ORDO_PG_PORT:-5432}"
  -U "${ORDO_PG_USER:-ordo}" -d postgres)

# Two `-c`: ALTER SYSTEM may not run inside the one transaction a single
# `-c` with two statements would be.
"$psql" "${args[@]}" -c "ALTER SYSTEM SET synchronous_commit = off" \
  -c "SELECT pg_reload_conf()"
echo "synchronous_commit is now $("$psql" "${args[@]}" -Atc 'SHOW synchronous_commit')"
