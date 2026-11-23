#!/bin/sh
# Regenerate src/generated and src/interface.json from ../viewer.ordo.
#
# `ordo gen ts` writes the typed client for one program under a root, at
# clients/ordo-sdk/src/generated. This app keeps its own copy, so the script
# writes to a scratch root, keeps the SDK half, and points its imports at the
# SDK's hand-written core through the `@hominemai/ordo-sdk` alias (see vite.config.ts).
# The same as examples/notes-v2/web/gen.sh.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ordo=${ORDO:-"$here/../../../target/debug/ordo"}
# Relative paths, so the commands OPS.md names work on any machine.
cd "$here"
"$ordo" gen ts .. --app src
