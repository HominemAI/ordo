#!/usr/bin/env bash
#
# Build the compiler for the browser and fold it into one HTML file.
#
#   scripts/build-wasm.sh
#
# Three steps, and the third is the one that matters: `wasm-pack` produces
# `crates/ordo-wasm/pkg/`, `inline.mjs` folds it into `crates/ordo-wasm/demo.html`
# with the module base64-inlined, and `smoke.mjs` runs that file's own script
# in Node and asserts both directions answered. A build that compiles but does
# not answer fails here rather than in a browser tab.
#
# `ordo-wasm` is NOT a workspace member (see its manifest), so `cargo
# test --workspace` never touches it. This script is what does.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
crate="$root/crates/ordo-wasm"

command -v wasm-pack >/dev/null || {
  echo "wasm-pack not found — cargo install wasm-pack" >&2
  exit 1
}
command -v node >/dev/null || { echo "node not found" >&2; exit 1; }

# The host-side tests cover the answer shaping (the only code this crate
# owns). They run first because they fail in two seconds and the wasm build
# takes rather longer.
( cd "$crate" && cargo test )

( cd "$crate" && wasm-pack build --target web --release --out-dir pkg )

node "$crate/inline.mjs"
node "$crate/smoke.mjs"

wasm="$crate/pkg/ordo_wasm_bg.wasm"
raw=$(wc -c < "$wasm" | tr -d ' ')
gz=$(gzip -9c "$wasm" | wc -c | tr -d ' ')
b64=$(( (raw + 2) / 3 * 4 ))
echo
echo "wasm    raw $raw  gzip $gz  base64 $b64 bytes"
echo "page    $(wc -c < "$crate/demo.html" | tr -d ' ') bytes  ($crate/demo.html)"
