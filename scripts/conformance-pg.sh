#!/usr/bin/env bash
# Throwaway PostgreSQL cluster for the conformance harness.
#
# Deliberately isolated: its own data directory, a non-default port, and a
# socket in /tmp (the socket path has a 103-byte limit, which a data directory
# under a long scratch path will exceed). It is not registered with
# `brew services` and needs no sudo, so `stop` really does remove it.
set -euo pipefail

PGBIN="${ORDO_PGBIN:-/opt/homebrew/opt/postgresql@17/bin}"
PGDATA="${ORDO_PGDATA:-${TMPDIR:-/tmp}/ordo-conformance-pgdata}"
SOCKET="${ORDO_PG_HOST:-/tmp/ordo-pg}"
PORT="${ORDO_PG_PORT:-55432}"
USER_NAME="${ORDO_PG_USER:-ordo}"
DB="${ORDO_PG_DB:-ordo_conformance}"

case "${1:-}" in
  start)
    mkdir -p "$SOCKET"
    if [ ! -d "$PGDATA" ]; then
      "$PGBIN/initdb" -D "$PGDATA" -U "$USER_NAME" --auth=trust >/dev/null
    fi
    # **The stock `max_connections` is enough**, and the road to knowing
    # that is worth the comment. The workspace suite used to peak at 81
    # client backends against Postgres's default of 100, and
    # `tests/runtime.rs` alone reached 99, so this script briefly started
    # the cluster at 300. That treated the symptom. `PoolConfig::from_env`
    # was not reading `ORDO_PG_POOL_SIZE` at all, so every test that built
    # a pool directly dialled `min(cores, 8)` whatever the environment
    # said. With that fixed and `.cargo/config.toml` setting the variable
    # to 2, the whole workspace peaks at 25.
    "$PGBIN/pg_ctl" -D "$PGDATA" -o "-p $PORT -k $SOCKET -h ''" -l "$PGDATA/server.log" start
    until "$PGBIN/pg_isready" -h "$SOCKET" -p "$PORT" -q; do sleep 0.2; done
    "$PGBIN/createdb" -h "$SOCKET" -p "$PORT" -U "$USER_NAME" "$DB" 2>/dev/null || true
    echo "ready: psql -h $SOCKET -p $PORT -U $USER_NAME -d $DB"
    ;;
  stop)
    "$PGBIN/pg_ctl" -D "$PGDATA" stop || true
    ;;
  destroy)
    "$PGBIN/pg_ctl" -D "$PGDATA" stop || true
    rm -rf "$PGDATA"
    echo "removed $PGDATA"
    ;;
  *)
    echo "usage: $0 {start|stop|destroy}" >&2
    exit 2
    ;;
esac
