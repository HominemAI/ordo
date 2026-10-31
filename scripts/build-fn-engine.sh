#!/usr/bin/env bash
#
# Build the JavaScript engine backend functions run in, and commit its output.
#
#   scripts/build-fn-engine.sh
#
# `crates/ordo-fn-js` (Boa, plus the Web-shaped prelude) compiles to one
# WebAssembly module. `ordo-fn` embeds the copy at
# `crates/ordo-fn/engine/ordo_fn_js.wasm`, so a workspace build needs no wasm
# toolchain. Run this after changing anything under `crates/ordo-fn-js`, and
# commit the module with the change.
#
# Needs: rustup target add wasm32-unknown-unknown

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
crate="$root/crates/ordo-fn-js"
out="$root/crates/ordo-fn/engine/ordo_fn_js.wasm"

( cd "$crate" && cargo build --release --target wasm32-unknown-unknown )

cp "$crate/target/wasm32-unknown-unknown/release/ordo_fn_js.wasm" "$out"
size=$(wc -c < "$out" | tr -d ' ')
echo "engine: $out ($size bytes)"
shasum -a 256 "$out"
