// Does the page actually answer?
//
// This runs `demo.html`'s OWN inline script — extracted verbatim, given a
// four-method fake DOM, and executed in Node. That is the point: a smoke test
// against `pkg/` would prove the wasm links, and prove nothing about the
// base64 blob and the stripped glue that the page actually ships. Here the
// bytes under test are the bytes in the file.
//
//   node crates/ordo-wasm/smoke.mjs
//
// Exits non-zero with the first failure named. Run by `scripts/build-wasm.sh`.

import { readFileSync, writeFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const html = readFileSync(join(here, "demo.html"), "utf8");

// ── the page must reach for nothing ──────────────────────────────────────
//
// The markup can be checked by reading it: no `<script src>`, no `<link>`,
// no absolute URL. The *code* cannot — the wasm-bindgen glue ships a
// `fetch` branch for the case where you hand it a URL instead of bytes, and
// grepping for `fetch(` would condemn a branch this page never enters. So
// the code is checked by execution instead: every network primitive is
// replaced with a throwing stub below, and the page has to boot anyway.
const m = html.match(/<script type="module">([\s\S]*?)<\/script>/);
if (!m) fail("demo.html has no <script type=\"module\">");

const markup = html.replace(m[0], "");
const forbidden = [
  [/<script[^>]+\bsrc\s*=/i, "a <script src=> — the page must carry its own code"],
  [/<link[^>]/i, "a <link> — no external stylesheet or font"],
  [/\bhttps?:\/\//i, "an absolute http(s) URL"],
];
for (const [re, what] of forbidden) {
  if (re.test(markup)) fail(`demo.html's markup contains ${what}`);
}

const noNetwork = (name) => () => {
  throw new Error(`the page called ${name}() — it must load entirely from its own bytes`);
};
globalThis.fetch = noNetwork("fetch");
globalThis.XMLHttpRequest = noNetwork("XMLHttpRequest");
globalThis.WebSocket = noNetwork("WebSocket");
globalThis.EventSource = noNetwork("EventSource");
globalThis.importScripts = noNetwork("importScripts");

// ── run the page's script ────────────────────────────────────────────────

// A DOM with exactly the surface the page uses. Anything it reaches for that
// is not here throws, which is the behaviour worth having: a silent stub
// would let the page half-work and still pass.
const nodes = new Map();
const node = (id) => {
  if (!nodes.has(id)) {
    nodes.set(id, { id, value: "", innerHTML: "", hidden: false, addEventListener() {} });
  }
  return nodes.get(id);
};
globalThis.document = {
  getElementById(id) {
    const known = ["boot", "app", "fixbox", "fixture", "q", "sql", "fwd", "rev", "go-fwd", "go-rev"];
    if (!known.includes(id)) throw new Error(`page asked for unknown element #${id}`);
    return node(id);
  },
};

const tmp = join(tmpdir(), `ordo-wasm-smoke-${process.pid}.mjs`);
writeFileSync(tmp, m[1]);
try {
  await import(pathToFileURL(tmp).href);
} finally {
  rmSync(tmp, { force: true });
}

// ── what it must have said ───────────────────────────────────────────────

const fwd = node("fwd").innerHTML;
const rev = node("rev").innerHTML;

if (node("boot").hidden !== true) fail("the boot line never went away — init did not finish");
if (fwd.includes("class=\"err\"")) fail(`forward pane errored:\n${fwd}`);
if (rev.includes("class=\"err\"")) fail(`reverse pane errored:\n${rev}`);

// Forward: `orders(status == "open")[total]` against the market fixture.
must(fwd, /SELECT/, "forward SQL");
must(fwd, /\btotal\b/, "the projected column");
must(fwd, /&#39;open&#39;|'open'/, "the filter literal");
must(fwd, /total: money/, "the output type");
must(fwd, /clock pinned to/, "the pinned clock");

// Reverse: `SELECT name FROM vendors WHERE active = true` → Ordo → SQL again.
must(rev, /vendors/, "the decompiled entity");
must(rev, /\[name\]|name/, "the decompiled projection");
must(rev, /round trip/i, "the round-trip SQL block");
must(rev, /ROWS were not compared/, "the honesty note about rows");

// The round trip has to produce SQL, not just Ordo.
const roundtrip = rev.split("round trip")[1] || "";
must(roundtrip, /SELECT/, "SQL from the recompiled Ordo");

console.log("ok — both directions answered from the inlined wasm");
console.log(`     page ${(Buffer.byteLength(html) / 1024).toFixed(0)} KB`);

function must(hay, re, what) {
  if (!re.test(hay)) fail(`missing ${what}\n--- pane ---\n${hay}\n---`);
}

function fail(msg) {
  console.error(`FAIL: ${msg}`);
  process.exit(1);
}
