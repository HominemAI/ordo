#!/usr/bin/env bash
# Report, and on request remove, the Postgres test clusters builds leave behind.
#
#   scripts/clean-test-clusters.sh            # report every cluster: size, running or stopped
#   scripts/clean-test-clusters.sh --remove   # also delete the STOPPED ones under ~/.ordo-pg
#
# Each agent that runs tests starts its own cluster, so parallel agents never
# share test databases: `ORDO_PGDATA=$HOME/.ordo-pg/<name>` on its own port.
# A test run creates many databases (a tenant is a database), so one cluster
# grows to 2 to 9 GB, and a finished agent leaves it on disk, sometimes still
# running. On 2026-09-29 ten of them held 57 GB with 5.2 GB of disk free.
#
# A running cluster is never touched: it may belong to an agent that is still
# working. Stop it first with its own variables:
#   ORDO_PG_PORT=<port> ORDO_PGDATA=$HOME/.ordo-pg/<name> ORDO_PG_HOST=/tmp/ordo-pg \
#     bash scripts/conformance-pg.sh stop
# The default cluster in $TMPDIR (scripts/conformance-pg.sh with no variables)
# is reported and never removed here; `conformance-pg.sh destroy` does that.
set -euo pipefail

REMOVE=0
case "${1:-}" in
  --remove) REMOVE=1 ;;
  "") ;;
  -h|--help) awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "$0"; exit 0 ;;
  *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
esac

human() { awk -v k="${1:-0}" 'BEGIN { if (k >= 1048576) printf "%.1fG\n", k/1048576; else if (k >= 1024) printf "%.0fM\n", k/1024; else printf "%dK\n", k }'; }

# A cluster is running when its postmaster.pid names a live process.
running() {
  local pid
  [ -f "$1/postmaster.pid" ] || return 1
  pid=$(head -1 "$1/postmaster.pid")
  kill -0 "$pid" 2>/dev/null
}

report() {
  local dir="$1" kib state
  kib=$(du -sk "$dir" 2>/dev/null | cut -f1 || echo 0)
  if running "$dir"; then state=running; else state=stopped; fi
  echo "$state $kib $dir"
}

TOTAL=0
FREED=0
default="${TMPDIR:-/tmp}/ordo-conformance-pgdata"
if [ -d "$default" ]; then
  read -r state kib _ < <(report "$default")
  echo "  $(human "$kib")  $state  $default  (the default cluster; kept)"
  TOTAL=$((TOTAL + kib))
fi
for dir in "$HOME"/.ordo-pg/*/; do
  [ -d "$dir" ] || continue
  dir="${dir%/}"
  read -r state kib _ < <(report "$dir")
  TOTAL=$((TOTAL + kib))
  if [ "$state" = stopped ] && [ "$REMOVE" = 1 ]; then
    rm -rf "$dir"
    FREED=$((FREED + kib))
    echo "  $(human "$kib")  stopped  $dir  removed"
  else
    echo "  $(human "$kib")  $state  $dir"
  fi
done

echo "test clusters used $(human "$TOTAL")"
[ "$REMOVE" = 1 ] && echo "freed $(human "$FREED")"
# Postgres processes whose data directory is gone are orphans.
ps -axo pid=,command= | awk '/postgres -D/ && !/awk/ { for (i=1;i<=NF;i++) if ($i=="-D") print $1, $(i+1) }' |
  while read -r pid dir; do [ -d "$dir" ] || echo "  orphan postgres $pid: $dir no longer exists"; done
df -h . | tail -1
