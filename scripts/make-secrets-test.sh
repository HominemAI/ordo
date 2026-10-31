#!/usr/bin/env bash
# Test for scripts/make-secrets.sh. Runs in a temp dir, needs no docker.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
# A stand-in for `ordo keys seal-master --out F --passphrase-file P`.
cat > "$tmp/ordo" <<'F'
#!/bin/sh
while [ $# -gt 0 ]; do [ "$1" = --out ] && out="$2"; shift; done
echo sealed > "$out"
F
chmod +x "$tmp/ordo"
export ORDO="$tmp/ordo"
fail() { echo "FAIL: $*" >&2; exit 1; }
for layout in one split; do
  d="$tmp/$layout"
  "$here/make-secrets.sh" "$layout" --dir "$d" --no-group >"$tmp/out" || fail "$layout: script failed"
  want=8; [ "$layout" = one ] && want=10
  [ "$(ls "$d" | wc -l | tr -d ' ')" = "$want" ] || fail "$layout: expected $want files"
  for f in token-key page-key deployment-db-password console-db-password master-passphrase master.key console-database-url deployment-db-password-app; do
    [ -s "$d/$f" ] || fail "$layout: $f missing or empty"
    mode="$(ls -l "$d/$f" | cut -c1-10)"
    [ "$mode" = "-r--r-----" ] || fail "$layout: $f mode $mode"
  done
  cmp -s "$d/deployment-db-password" "$d/deployment-db-password-app" || fail "$layout: app copy differs"
  grep -q "$(cat "$d/console-db-password")" "$d/console-database-url" || fail "$layout: url lacks password"
  grep -q "back up\|Back up" "$tmp/out" || fail "$layout: no backup note"
  before="$(cat "$d/token-key")"
  if "$here/make-secrets.sh" "$layout" --dir "$d" --no-group 2>"$tmp/err"; then fail "$layout: overwrote"; fi
  grep -q refusing "$tmp/err" || fail "$layout: no refusal text"
  [ "$(cat "$d/token-key")" = "$before" ] || fail "$layout: file changed"
done
# A `one` folder made before the identity service gets its 2 files added,
# and nothing else changes.
d="$tmp/one"
grep -q "$(cat "$d/identity-db-password")" "$d/identity-database-url" || fail "identity url lacks password"
rm "$d/identity-db-password" "$d/identity-database-url"
before="$(cat "$d/console-database-url" "$d/token-key")"
"$here/make-secrets.sh" one --dir "$d" --no-group >"$tmp/out" || fail "upgrade: script failed"
[ -s "$d/identity-db-password" ] && [ -s "$d/identity-database-url" ] || fail "upgrade: identity files missing"
[ "$(cat "$d/console-database-url" "$d/token-key")" = "$before" ] || fail "upgrade: a file changed"
rm "$d/identity-database-url"
if "$here/make-secrets.sh" one --dir "$d" --no-group 2>"$tmp/err"; then fail "half an identity pair was accepted"; fi
echo "make-secrets: ok"
