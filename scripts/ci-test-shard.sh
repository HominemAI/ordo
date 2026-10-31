#!/usr/bin/env bash
# Run one shard of the workspace suite, so CI can run the shards at once.
#
#   scripts/ci-test-shard.sh rest        every crate but ordo-host, and
#                                        ordo-host's unit and doc tests
#   scripts/ci-test-shard.sh host K N    ordo-host's integration test
#                                        files, shard K of N (1-based)
#   scripts/ci-test-shard.sh host K N --list    print shard K's files only
#
# ordo-host's test files are 76% of the suite's run time, so they are
# split by measured seconds: the heaviest file goes to the lightest shard,
# in turn. A file missing from WEIGHTS counts as DEFAULT seconds, so a new
# test file always lands in some shard. The weights only balance the
# shards; every file runs whatever they say.
#
# Weights: seconds per test binary in CI, measured 2026-10-31. Refresh
# them when one shard runs much longer than the others.
set -euo pipefail

WEIGHTS="
notes_app 243
refine_audit 206
marketing_site_app 44
provision_own 42
deploy_apply 38
deployment 36
restore_drill 34
health 33
teamspace_app 32
shopfront_app 31
share_links 28
refine_paging 25
upgrade_continuity 22
tenant_forward 20
"
DEFAULT=6

here="$(cd "$(dirname "$0")/.." && pwd)"
mode="${1:?usage: ci-test-shard.sh rest | host K N [--list]}"

case "$mode" in
  rest)
    cargo test --workspace --exclude ordo-host --no-fail-fast -- --nocapture
    cargo test -p ordo-host --lib --bins --no-fail-fast -- --nocapture
    cargo test -p ordo-host --doc --no-fail-fast
    ;;
  host)
    k="${2:?shard number}"
    n="${3:?shard count}"
    files=$(
      for f in "$here"/crates/ordo-host/tests/*.rs; do
        name=$(basename "$f" .rs)
        w=$(awk -v t="$name" '$1 == t { print $2 }' <<<"$WEIGHTS")
        echo "${w:-$DEFAULT} $name"
      done | sort -k1,1nr -k2,2 | awk -v k="$k" -v n="$n" '
        { best = 1
          for (i = 2; i <= n; i++) if (load[i] < load[best]) best = i
          load[best] += $1
          if (best == k) print $2 }'
    )
    if [ "${4:-}" = "--list" ]; then
      echo "$files"
      exit 0
    fi
    args=()
    for t in $files; do args+=(--test "$t"); done
    if [ ${#args[@]} -eq 0 ]; then
      echo "shard $k of $n has no test files"
      exit 0
    fi
    cargo test -p ordo-host --no-fail-fast "${args[@]}" -- --nocapture
    ;;
  *)
    echo "unknown shard: $mode (rest | host K N)" >&2
    exit 2
    ;;
esac
