#!/usr/bin/env bash
# One command: build what the stack needs, then run every end-to-end test.
#
#     bash examples/notes/e2e/run.sh                   # all use cases
#     bash examples/notes/e2e/run.sh --project=api     # request level only, no browser
#     bash examples/notes/e2e/run.sh -g "shared note"  # one use case by name
#
# Arguments go to `playwright test`. Playwright starts the throwaway stack
# (stack.sh) before the first test and stops it after the last. The report
# is in examples/notes/e2e/report/html, failures' traces in test-results/.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)

if [ "${E2E_SKIP_BUILD:-0}" != 1 ]; then
  (cd "$repo" && cargo build -p ordo-cli -p ordo-host)
fi
[ -d "$repo/examples/notes/web/node_modules" ] || (cd "$repo/examples/notes/web" && npm ci)
[ -d "$here/node_modules" ] || (cd "$here" && npm ci)
(cd "$here" && npx playwright install chromium webkit firefox >/dev/null)

cd "$here"
status=0
npx playwright test "$@" || status=$?
node summary.mjs || true
exit "$status"
