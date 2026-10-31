// Fold `pkg/` into one self-contained HTML file.
//
// `demo.template.html` carries the page; this script substitutes two things
// into it: the wasm-bindgen glue (`/*__GLUE__*/`) and the wasm module itself,
// base64-encoded (`__WASM_B64__`). The result loads with no network access of
// any kind, which is the requirement a strict-CSP artifact imposes and the
// reason the module is inlined rather than fetched.
//
//   node crates/ordo-wasm/inline.mjs
//
// Run by `scripts/build-wasm.sh`, which builds `pkg/` first.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const pkg = join(here, "pkg");

const wasm = readFileSync(join(pkg, "ordo_wasm_bg.wasm"));

// The glue is an ES module. It goes inline into a `<script type="module">`,
// where its `export` declarations would be inert but are stripped anyway:
// an inline module nothing imports has no business declaring exports, and
// the page calls the functions directly from the same scope.
const glue = readFileSync(join(pkg, "ordo_wasm.js"), "utf8")
  .replace(/^export function /gm, "function ")
  .replace(/^export \{[^}]*\};?\s*$/gm, "");

const html = readFileSync(join(here, "demo.template.html"), "utf8")
  .replace("/*__GLUE__*/", () => glue)
  .replace("__WASM_B64__", () => wasm.toString("base64"));

if (html.includes("__WASM_B64__") || html.includes("/*__GLUE__*/")) {
  throw new Error("a placeholder survived substitution — the template changed");
}

const out = join(here, "demo.html");
writeFileSync(out, html);

const kb = (n) => `${(n / 1024).toFixed(0)} KB`;
console.log(`wasm  ${kb(wasm.length)} raw`);
console.log(`page  ${kb(Buffer.byteLength(html))} (${out})`);
