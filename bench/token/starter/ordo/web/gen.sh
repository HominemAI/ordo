#!/bin/sh
# Write the typed client for ../backend into src/generated, and the
# interface descriptor the React hooks read into src/interface.json.
#
# `ordo gen ts` writes under <out>/clients/ordo-sdk/src/generated. This keeps
# that half and points its imports at the installed SDK package.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
ordo gen ts "$here/../backend" --out "$tmp"
rm -rf "$here/src/generated"
mkdir -p "$here/src/generated"
for f in "$tmp"/clients/ordo-sdk/src/generated/*.ts; do
  sed -e "s#'\.\./core/#'@hominemai/ordo-sdk/core/#g" "$f" >"$here/src/generated/$(basename "$f")"
done
ordo interface "$here/../backend" --out "$here/src"
echo "wrote $here/src/generated and $here/src/interface.json"
