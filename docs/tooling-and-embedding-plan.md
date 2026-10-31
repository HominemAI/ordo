# Ordo Tooling & Embedding Plan

Separate from `engine-plan.md` (the engine build). Three tracks: **A** — ship
the compiler as a standalone developers add to IDEs, generating schema and
artifacts from `.ordo`; **B** — surface Ordo *inside* host languages:
generated native functions instead of (or on top of) HTTP endpoints, with
call-site refinement; **C** — typed builder frontends that define domains in
host-language code and synthesize canonical `.ordo` (the CDK pattern).
Status: **partly built.** A2's increment 0 (the TextMate grammar) landed
2026-09-07; B1 (the interface descriptor) the same day; **B2's TypeScript
backend and the whole of Track C landed 2026-09-07** as `crates/ordo-gen`,
`clients/ordo-sdk` and `clients/ordo-cdk` (checklist 14d, 14f) — one
generator, two consumers. Everything else here is still a plan, and each
section says which half it is.

---

## Track A — Standalone compiler + IDE integration

### A1. The `ordoc` standalone
The engine is library-first (engine-plan), so the standalone is a thin shell:
one static binary per platform, no runtime deps, embeddable and IDE-callable.

```
ordoc check  <dir>            # parse + bind + rules; spanned diagnostics (JSON or human)
ordoc build  <dir> -o out/    # emit everything derivable from source:
   out/schema.sql             #   Postgres DDL (tables, checks, indexes, FKs, tree paths)
   out/registry.json          #   op artifacts: planned SQL, mode labels, cost class,
                              #   scope manifests, version hashes
   out/interface.json         #   the Interface Descriptor (see B1) — input to all codegen
ordoc fmt    <dir>            # canonical formatter — one spelling ⇒ formatting is
                              #   canonicalization, zero style options
```

`ordoc build` **is** "generate schema from ordo": DDL out of shapes, planned
SQL out of queries, and a machine-readable description of every registered op.
It runs with no database and no platform — the compile-only tier of the
conformance pipeline is exactly this binary.

### A2. Language server (`ordo-lsp`)
Same crates, long-running, incremental. The closed grammar makes every LSP
feature unusually strong:

- **Diagnostics as-you-type** — the engine's loud, spanned errors verbatim.
- **Hover** = derived truth: an op's mode label, cost class, and field-scope
  manifest; an entity's ownership summary (`owns: receiptLines; referenced
  by: …`). Nothing hand-maintained — the compiler already knows all of it.
- **Completions** — a closed grammar completes deterministically: after `sort`
  only fields; inside `(…)` only fields/params/operators; after a query name,
  its output fields. Post-parse-error completion stays useful because the
  error is always local.
- **Go-to-definition / find-references** across entities, refs, queries
  (composition edges), routines.
- **Inlay hints**: inferred join path on dot-paths, cost-class chip per query.
- **Rename** — safe because references are closed and resolvable.

Editors: VS Code extension first (TextMate grammar for instant highlighting +
LSP client), then the LSP serves JetBrains/Neovim/Helix/Zed for free.
Distribution: rustup-style install script, Homebrew, npm wrapper binary
(esbuild pattern) so `npx ordoc` works, GitHub releases with signed builds.

**Sequenced and part-built: [`plan-editor-tooling.md`](plan-editor-tooling.md).**
That document owns the editor half of A2 — the grammar, the two extensions,
the JetBrains ladder and distribution — and does not restate the capability
list above; it sequences it. Its increment 0, the TextMate grammar plus a
two-directional test pinning its keyword list to the lexer's, **landed
2026-09-07** (`editors/ordo.tmLanguage.json`). Two facts it adds that this
section did not have to face: the LSP is gated on the trigger/routine
declaration form settling, because `parse_program` accepts `domain`, `entity`
and `op` and nothing else; and hover is a projection of B1's descriptor, not
a second walk of the binder.

### A3. Dev loop
`ordoc dev` = watch + incremental recompile + diagnostics stream (the engine
half of the platform's `ordo dev`); the LSP consumes the same incremental
compiler, so editor and CLI never disagree.

---

## Track B — In-language embedding ("bootstrap, but better")

### The reframe
Today's model: deploy `.ordo` → platform exposes named ops over HTTP → a
generated TS SDK calls them. Track B makes Ordo feel native in the host
language: **a build step turns the registry into typed functions in Java /
Python / Go / TS**, so application code calls `vendorSpend(from)` as a
function, not "an endpoint." Two depths, one codegen:

| Mode | What runs where | Wire |
|---|---|---|
| **Client mode** | generated functions wrap the op protocol | op name + params (+ refinement envelope) to a running Ordo |
| **Embedded mode** | engine core linked in-process; talks to Postgres directly | none — the app *is* the platform node |

Client mode is the near-term default (works against any deployment; smallest
surface). Embedded mode is the "better bootstrap": a Java/Go service adds
`.ordo` files next to its source, the build generates functions **and** links
the engine, and there is no separate server at all. Same artifacts either way.

### B1. The Interface Descriptor (the one thing everything shares) — **BUILT 2026-09-07**
`interface.json`: entities (fields, types, refs, nullability, sealed marks),
ops (params + bounds, result shape, mode label, cost class, live-capability,
**output-field manifest**), version hash. Every language backend is a pure
function of this file — adding a language never touches the engine. At
runtime the same descriptor is **shape-verified against an empty shadow
schema and served as the capability catalog** (see `implementation.md`) —
generated code and the live deployment can never silently disagree about a
result shape.

**As built** (`ordo_plan::interface`, `ordo interface <dir> [--out <dir>]`,
`GET /interface` · `GET /.well-known/ordo-interface`):

- It is a **projection**, never a second walk. Ops come from the catalog
  manifest (`ordo_plan::artifact::catalog_view`) and entities from
  `entity_artifacts` — both projections of `op_artifact`, the one producer.
  The module takes JSON in and JSON out, so it cannot reach a binder even by
  accident, and `ordo-cli/tests/interface.rs` joins the two documents in both
  directions: nothing invented, nothing settled omitted.
- **Catalog scoping is one filter, not two rules.** The serving route filters
  manifests with the same check the door runs and *then* describes them, so
  the descriptor a caller receives contains exactly the ops their catalog
  shows. Everything derived across ops — the typed-id action index, the
  entity half — is derived after the filter.
- **§B3's terminal verbs are stamped** on every op as `call`: the method, the
  verb a builder must end in (`fetch`/`live`/`pages` vs `commit`), CAS
  default, refinability, liveness, paging. A renderer needs no name
  heuristics.
- **Typed-id action discovery** (`ui-gen.md` §2) rides on
  `params[].id_entity`, which the artifact now derives: the type system
  spells every id `id`, so *which* entity an id parameter addresses is a fact
  only the body carries, and it is read there once.
- **`schema_version` + `digest`** give it the identity discipline deploys
  have; the golden is `crates/ordo-cli/tests/golden/market.interface.json`.

What it deliberately does **not** carry: the reserved UI annotations
(`label`, `desc`, `order`, `choices`, `danger`). Those belong to `ui-gen.md`
§1's *surface* descriptor, which is a projection of this file plus them, and
the grammar does not lex one of them yet.

### B2. Call-site refinement — `.filter[fields]` done honestly
The requested `queryName.filter[field, field]` exists in the language already:
it is the **composition call form** `name(conditions)[projection]`. Track B
surfaces exactly that grammar at the call site — and *only* that grammar:

```java
// Java — builders generated per-op, so fields are host-compile-time checked
var rows = ordo.vendorSpend(Params.from(LocalDate.parse("2026-01-01")))
    .where(v -> v.billed().gt(Money.of(4000)))   // conditions: output fields only
    .pick(VendorSpend.NAME, VendorSpend.BILLED)  // projection subset
    .fetch();
```
```python
rows = ordo.vendor_spend(from_=date(2026, 1, 1)) \
    .where(F.billed > 4000).pick("name", "billed").fetch()
```
```go
rows, err := ordo.VendorSpend(ctx, ordo.VendorSpendParams{From: from}).
    Where(q.Billed.Gt(4000)).Pick(q.Name, q.Billed).Fetch()
```

**The line that must hold:** refinement is *structured data, never query
text* — a refinement envelope (condition tree over the op's output fields +
field-name list), validated server-side against the output manifest. It can
only **narrow** (scope shrinks, never grows), it obeys the same null truth
table and cost classes (an open field's condition is `sql`; a sealed field
takes a condition only where declared `sealed indexed refinable`, equality
only; see `howto-cost.md`), and unknown fields are rejected by name.
This is not "SQL over the wire returning" — the expressible surface is the
composition parens/brackets, nothing more. Server-side it compiles through
the same inliner as written composition, so semantics are identical to having
written a wrapper query — post-`take`, post-everything. **Built** 2026-10-05
(`crates/ordo-host/src/refine_call.rs`): the call is built as an AST and
bound, so pages under a refinement come back full.

Two tiers so casual callers stay cheap: **Tier 0** (always on): projection
subset + conditions with `== != < <= > >= in is-null` over output fields —
covers the `.filter[field,field]` ask. **Tier 1** (per-op opt-in flag at
registration): full condition grammar incl. text ops. **Every read ships
Tier 0** (ruled 2026-10-05: refinement is a standard feature with no off
switch); writes, platform ops and public reads take none. Tier 1 stays a
future opt-in.

### B3. What generated functions cover
- **Reads**: typed params (bounds enforced client-side too, same error text),
  typed result records (ids as branded/newtype ids per language, `money` as
  BigDecimal / decimal.Decimal / big.Rat-wrapper / **an exact decimal string
  in TS** — `bigint`-minor-units was written here before the wire was
  settled, and `api-engine-design.md` §5 settles `money` as a decimal string
  with the descriptor's bounds as decimal digit strings; a client that parsed
  it into a bigint would be choosing a second representation for the value it
  exists to carry unchanged),
  pagination as language-native iteration (Java `Stream`/`Flow`, Python
  async iterators, Go iterator func + channels, TS `AsyncIterable`) over the
  keyset token protocol, `?live` as subscriptions (Flow / async generator /
  channel / AsyncIterable) with refetch-through-scope semantics preserved.
- **Writes**: one function per registered write op; the mode label lands in
  the generated docstring/Javadoc ("this op creates-or-converges"); the
  idempotency envelope is automatic in the binding's retry loop.
- **Errors**: the platform's typed rejections (`conflicting_identity`,
  `duplicate`, CAS miss, bound violations) as typed exceptions/results —
  never stringly-typed.
- **Terminal verbs — the call-site kind contract [settled]**: reads are lazy
  builders that must terminate in a read verb (`.fetch()` / `.live()` /
  `.pages()`); writes terminate in **`.commit()`** and have no read verbs, no
  refinement. Enforced by the generated types, so misuse is a compile error
  in the caller's own language. Every mutation in an app is `grep '.commit('`;
  the wire method (GET/POST/DELETE per derived kind) follows automatically.
  Every SDK language carries the same split.

### B4. Embedded mode mechanics
Rust core exposes a C ABI (`ordo-core.h`) + the WASM build:

| Host | Binding | Notes |
|---|---|---|
| TS/JS | **WASM** (wasm-bindgen) | compiler already WASM-capable by design; browser playground + Node embedded mode |
| Python | PyO3 native module | first-class fit; wheels via maturin |
| Go | cgo against the C ABI | static-link friendly; iterator-based results |
| Java | Panama (jextract) — JNI fallback | modern JDKs; Gradle/Maven plugin runs codegen |

Embedded mode = binding + connection string: the engine executes plans
against Postgres in-process, command log and all. What it does **not** get
for free: multi-node invalidation fan-out, the identity service, channels —
embedded is the single-node/self-host shape (exactly the "bootstrap" use),
and graduating to a platform deployment is a config change, not a rewrite,
because the artifacts are identical.

### B5. Version skew, solved once
Generated code pins the registry **version hash** it was built from; every
call carries it; the server answers with drift status (compatible /
narrowed / breaking) and the binding surfaces it as a typed warning/error.
CI regenerates on `.ordo` change — same PR shows the schema diff and the
generated-function diff side by side.

### B6. Order of attack
1. ~~**Descriptor** (B1) out of `ordoc build`~~ — **done 2026-09-07**; it was
   cheap once artifacts existed, exactly as predicted. What was *not* free:
   the artifact had to grow four facts before the descriptor could be a
   projection rather than a second walk (see B1's "as built").
2. ~~**TS backend** first (the generated SDK is already planned platform work —
   this *is* it, plus refinement builders), including the WASM/browser story.~~
   — **done 2026-09-07** as `crates/ordo-gen` + `clients/ordo-sdk`
   (checklist 14d), *without* the WASM/browser story. **The refinement
   builders are built** (2026-10-05, refinement R4): `.where` and `.pick` on
   every generated read, typed per op, checked before the wire by the
   validator the device copy shares. The crate is a pure
   descriptor consumer, so items 4's languages really are templates plus a
   thin runtime — the shape is now demonstrated rather than predicted.
3. ~~**Refinement envelope** server-side (validator + inliner reuse + the two
   tiers)~~ — **Tier 0 built 2026-10-05** (`plan-refinement.md`, R1 to R6):
   every program read is refinable, the executor applies `$refine` in the
   plan, live and sync take it. Tier 1 is not built.
4. **Python**, then **Go**, then **Java** client-mode backends (pure
   descriptor consumers; each is templates + a thin runtime ~1–2 KLOC).
5. **Embedded mode** after engine v1 hardens: C ABI, PyO3, cgo, Panama.
6. LSP (Track A2) proceeds in parallel — it shares the incremental compiler,
   not the codegen.

## Track C — Builder frontends (define in code, synth to canonical `.ordo`)

For authors who reject DSLs on principle ("functions, params, chaining — not
text with special meaning"), the CDK/Pulumi resolution: **typed definition
builders in the host language that synthesize canonical `.ordo`**. The
grammar is closed and the IR is data, so a builder is nearly mechanical:

```ts
const transactions = entity("transactions", {
  account:  ref(accounts),
  amount:   money(),
  date:     date().default(today),
  notes:    text().default("").sealed(),
  payee:    ref(payees).optional().sealed().indexed(),
});

const vendorSpend = op("vendorSpend", { from: p.date() }, q =>
  q.source(vendors.pick("name"), invoices.where(f.issued.gte(p.from)).pick("total"))
   .group(vendors.id, vendors.name, { n: agg.count(), billed: agg.sum(f.total) })
   .bar(f.billed.gt(0)));
```

**BUILT 2026-09-07** as `clients/ordo-cdk` (checklist 14f), out of the same
generator as the SDK. **The sketch above is not the shipped API**: it and the
converter demo page's pane disagree with each other and, in three places, with
the grammar — there is no `join` and no `upsert` in the language, `.bar` names
a character rather than a step, and a string-valued `.where('a or b')` would
be a second parser. `clients/ordo-cdk/README.md` tabulates all nine
divergences with reasons; the shipped spelling is `b.source(src('products')
.args(f.active.eq(lit(true))).pick('name', 'price'))`. Both sketches should be
corrected to match.

Rules that keep it honest — a **frontend, never a second semantics**:

- `synth` emits canonical `.ordo` (via `ordo fmt`) — the synthesized file is
  the artifact that is reviewed, diffed, committed, and deployed. *As built:*
  there is no `ordo fmt` (Track A1 is unbuilt), so "canonical" is the
  printer's own one spelling and the tested property is **parse-and-reparse
  equality** — `parse(original)` and `parse(synth(…))` are the same AST,
  spans excepted. When `ordo fmt` lands it becomes the normalizer on both
  sides and the claim tightens to bytes. Builders
  may loop and abstract; the output is always the flat, readable form. One
  spelling per concept holds at the artifact level, which is where it earns
  its keep (deploy diffs, consent derivation, LLM corpus).
- The builder API is generated from the same grammar definition the parser
  uses — it cannot drift, and anything expressible in the builder is
  expressible in `.ordo` and vice versa (isomorphism is a conformance-tested
  property, not an aspiration). *As built:* the vocabulary is a projection of
  `Keyword`, `ScalarTy`, `CmpOp`, `TextOp`, `BinOp`, `DurationUnit`,
  `functions()` and `AGGREGATE_SHAPES`, matched exhaustively so a new word
  fails `cargo build` until its TypeScript spelling is decided; and the
  isomorphism test machine-translates each corpus program's parsed AST into
  builder calls, synthesizes, and reparses — **31 of 31 round-trip, no
  misses**. It found two real printer bugs on the first run (see
  `corpus-rulings.md`, 2026-09-07).
- Everything downstream is unchanged: same compiler, same manifests, same
  cost classes, same registry. The platform never knows which frontend
  authored a definition.
- Type-level mirroring where the host language allows: field names and
  entity references are typed objects (not strings) in TS, so IDE
  autocomplete/refactor work; conditions use the builder's closed combinator
  set — a Turing-complete *escape* into arbitrary predicates does not exist,
  because it couldn't compile anyway.

TS builder only, for now — the SDK's audience is the builder's audience. A
second language (Python for the agent-tooling world, Go for backend shops)
is demand-gated, not promised. Positioning: `.ordo` text and the builder are
two doors into one room — teams pick per taste, artifacts stay identical.

### Risks / lines we don't cross
- Refinement must never grow into query-text-over-the-wire; the envelope is
  the composition grammar, closed, manifest-validated, narrowing-only.
- FFI surface is a maintenance tax — keep the C ABI tiny (compile, plan,
  execute, subscribe) and version it.
- Codegen fidelity beats codegen breadth: a language lands only with the full
  typed-error/pagination/live story, or not at all — a half backend teaches
  users the SDK can't be trusted.
