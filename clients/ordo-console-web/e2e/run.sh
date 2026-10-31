#!/usr/bin/env bash
# One command: build what the stack needs, then run the console's
# end-to-end tests.
#
#     bash clients/ordo-console-web/e2e/run.sh
#     bash clients/ordo-console-web/e2e/run.sh --project=phone
#     E2E_SKIP_BUILD=1 bash clients/ordo-console-web/e2e/run.sh
#
# Arguments go to `playwright test`. Playwright starts the throwaway stack
# (stack.sh) and the one on the identity service (stack-identity.sh), and
# stops them after the last test.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)

if [ "${E2E_SKIP_BUILD:-0}" != 1 ]; then
  (cd "$repo" && cargo build -p ordo-console -p ordo-cli -p ordo-identity-service -p ordo-supervisor && cargo build -p ordo-console --example e2e_door)
fi
[ -d "$here/../node_modules" ] || (cd "$here/.." && npm ci)
[ -d "$here/node_modules" ] || (cd "$here" && npm ci)
(cd "$here" && npx playwright install chromium >/dev/null)

cd "$here"
npx playwright test "$@"
