#!/usr/bin/env bash
# Source the printed exports before building and keep them through reporting.
set -euo pipefail
cd "$(dirname "$0")/.."
case "${1:-}" in
  env)
    cargo llvm-cov show-env --export-prefix
    ;;
  report)
    suite=${2:?usage: e2e-coverage.sh report SUITE}
    [[ "$suite" =~ ^[a-z0-9-]+$ ]] || { echo 'Invalid suite name' >&2; exit 1; }
    case "$suite" in
      notes) state=examples/notes/e2e/.stack ;;
      federation) state=examples/note-viewer/e2e/.stack ;;
      *) echo "Unknown suite: $suite" >&2; exit 1 ;;
    esac
    test -s "$state/host-pids"
    # show-env names profiles with the process id and the binary signature.
    # A CLI profile alone must never pass for a host that was killed early.
    while read -r pid; do
      profile=$(find "$(dirname "${LLVM_PROFILE_FILE:?coverage environment missing}")" \
        -maxdepth 1 -name "*-$pid-*.profraw" -size +0c -print -quit)
      test -n "$profile" || { echo "No LLVM profile for host $pid" >&2; exit 1; }
    done < "$state/host-pids"
    mkdir -p coverage-out
    cargo llvm-cov report --lcov --output-path "coverage-out/lcov-$suite.info"
    ;;
  *) echo 'Usage: e2e-coverage.sh env | report SUITE' >&2; exit 1 ;;
esac
