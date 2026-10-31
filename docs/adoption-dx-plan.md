# Ordo Adoption & DX Plan — Overcoming the New-Language Tax

Companion to `definition-language.md`, `implementation.md`,
`tooling-and-embedding-plan.md`, and `engine-plan.md`. Same markers:
**[settled]** / **[lean]** / **[open]**. Status: plan. Prompted by the
competitive review vs Convex: their save-and-it's-live TypeScript loop is the
DX bar, and "you already know the language" is the one advantage Ordo cannot
copy. This doc is what we do instead.

---

## 1. The tax, stated honestly [settled]

Adopting Ordo means learning a grammar. Not a library, not an API surface — a
language, with its own spellings, its own rejections, and (today) with:

- **no Stack Overflow corpus** — nobody can paste an error into a search box
  and find a decade of answers;
- **no npm of examples** — no ecosystem of packages, snippets, starter repos,
  or blog posts to copy from;
- **no muscle memory** — every SQL and TypeScript instinct (JOIN…ON, `if`,
  three-valued null, OFFSET, upsert folklore) is deliberately absent, and some
  of it actively fails to parse.

Convex charges none of this: a developer writes TypeScript, saves, and it's
live. That loop is the adoption bar, and pretending the tax is small is how
language projects die. The tax is real; the plan is to make it **worth paying
fast** and, for most people, **avoidable entirely** (§6).

## 2. What does not work [settled]

**Loosening the grammar.** Every relief valve — synonym spellings, optional
parens, "friendly" coercions, a compat mode that accepts SQL-ish forms — spends
the product's core asset. One-spelling-per-concept is why hallucinated variants
fail to parse instead of parsing to something else (spec §1), why completions
are deterministic, why `fmt` is canonicalization, and why the anti-corruption
marketing claim is checkable at all. A grammar loosened for adoption converges
on being SQL with extra steps, and then there is no reason to adopt it.

The tax is paid down on every other axis: loop speed, tooling, teaching-grade
errors, generated code, and AI authorship. Never on the grammar.

## 3. The feedback-loop bar [settled as bar]

Convex's loop is: edit TypeScript → save → live, sub-second, errors inline.
Ordo's loop must *feel* identical even though a compiler sits in the middle.
The pieces already exist in plan; this section pins them as a bar, not a
feature list.

### 3.1 `ordo dev`: save → live, sub-second
`implementation.md` settles the mechanics: compile-once artifacts, hot swap via
registry + LISTEN/NOTIFY, dev-mode relaxations (auto-confirm destructive
migrations, skip version bookkeeping). The bar: **save-to-serving under one
second** for a domain of realistic size, including additive migration apply.
The closed grammar and hand-written parser (engine-plan) make compile time a
non-issue; the budget is dominated by DDL apply and swap, so dev mode keeps a
warm shadow schema and applies additively without confirmation round-trips.
Anything over a second on save is a bug, not a tuning opportunity.

### 3.2 LSP: the closed grammar is a tooling superpower
Track A2 (`tooling-and-embedding-plan.md`) already claims this; restated here
as adoption strategy: a closed grammar means the LSP can be *complete* in a
way TypeScript servers never are.

- **Deterministic completions** — after `sort` only fields; inside `(…)` only
  fields/params/operators; after a query name, its output fields. There is
  exactly one next-token set; completion is never a guess.
- **Hover = derived truth** — mode label, cost class, field-scope manifest,
  ownership summary. The compiler already knows all of it; nothing is
  hand-maintained, so hover never lies.
- **Inlay hints** — inferred join path on dot-paths, cost-class chip per
  query: the learner *sees* what the language derived for them, which is how
  "schema knows the joins" stops being a slogan and becomes intuition.

The LSP is not a convenience — it is the replacement for the missing Stack
Overflow corpus. The editor answers the questions the internet can't yet.

### 3.3 Errors as teaching [settled as commitment]
The engine's style is already "fails loudly with the right words" (engine-plan
parser ruling; closed error-class inventory in `composition-conformance.md`
§0). Promoted here to a stated product commitment: **every diagnostic names
the rule it enforces and states the fix**, in that order. Examples of the
required shape:

```
error[mixed-booleans]: `and` and `or` mixed without parentheses
  a and b or c
  ^^^^^^^^^^^^
  rule: no precedence table — mixing boolean operators requires parens.
  fix:  (a and b) or c   —or—   a and (b or c)
```

```
error[fan-out]: two independent many-paths in one pipeline
  accounts[]  transactions[]  receipts[]
  rule: at most one many-path per pipeline (star-aggregation double-count).
  fix:  compute one side in a `let` and join it back, or use `match any`.
```

Consequences: error text is part of the conformance surface (asserted by
class, reviewed as prose); every parser rejection added in P1 ships with its
teaching text; and the docs cross-link — the `rule:` line names the spec
section, so the error message is the index into the manual. A new-language
user's first hour is mostly errors; each one must leave them smarter.

## 4. Time-to-first-app

Target: **a working deployed app inside the first hour**, with a meaningful
result inside the first five minutes. Four pieces:

### 4.1 Playground [lean]
Browser playground, zero install: the compiler is WASM-capable by design
(`implementation.md`, Track B4), so parse/bind/rules/golden-SQL run entirely
client-side — type Ordo on the left, see diagnostics, derived schema, mode
labels, cost classes, and planned SQL on the right. No accounts, no backend
for the compile tier. Execution against sample data can follow later via the
embedded-mode WASM engine against an in-browser store; compile-tier alone is
already the "five minutes to aha" vehicle and ships first.

### 4.2 `ordo new` starter templates [lean]
Two to three complete domains, each ~two pages of `.ordo` (the north-star
size, spec §10): **finance** (the design's home domain — accounts,
transactions, categorizer routine), **projects/tasks** (trees, `under`,
alerts), and **inventory or bookings** (link entities, quotas via `check`).
Each template is simultaneously: a working app, a grammar tour, and seed
corpus for the eval harness (`eval-harness-plan.md`). `ordo new finance` →
`ordo dev` → generated API + catalog, first request in minutes.

### 4.3 The grammar prompt is the human onboarding too [settled as direction]
Spec §10 budgets the grammar as a two-page LLM system prompt: one page of
syntax, one page of the semantic rules drift happens on. That artifact **is
the human quick-reference** — the entire language on two pages is an adoption
asset no open grammar can offer, and it must be written once and rendered
twice (prompt file and doc page) so it cannot drift. "The whole language fits
on two pages" is a marketing line only if it stays true; the eval harness's
grammar-version pinning keeps it honest.

### 4.4 The cookbook: "how do I do X" → Ordo idiom [lean]
A worked translation doc mapping the questions people actually arrive with —
phrased as SQL, Convex, and Supabase idioms — to the Ordo spelling, with the
rationale when the answer is "you don't, and here's the blessed pattern":

| Arrival idiom | Ordo answer |
|---|---|
| JOIN … ON | declared refs + dot-paths; `match` only when it carries information |
| GROUP BY … HAVING | `group … { }` + the bar after it (position, not keyword) |
| CASE WHEN | `cond(c1 -> v1, …, velse)` — total, ordered |
| INSERT … ON CONFLICT | unique-key converge-or-create — the one upsert |
| window functions | declined (§11) — client compute / paged reduce / rule-materialized state |
| soft delete flag scoping | named query as the scope, composition everywhere |
| OFFSET pagination | call-time keyset tokens; not syntax |
| Convex mutation | registered write op, mode-labeled, atomic group |
| Supabase RLS policy | two layers, not one: **row**-level rules are `access read` / `access write` guards (and `acl`) in the *shape*; **which ops exist for a role** is the access profile, runtime data. A policy that composes permissively across several policies has no equivalent — guards replace, never compose |
| Firebase "system user" service account | no equivalent, deliberately: a routine is bounded by its trigger (clock ⇒ guards pass; listener ⇒ `$trigger.ids`) and its derived scope manifest, so there is no principal to grant everything to |
| `SELECT a/b` on money columns | `/` yields `decimal` (fixed scale 12); nothing rounds implicitly, so `round`/`floor`/`ceil` is written at the boundary, and money splits use `allocate` |

The raw material already exists: `sql-translation-corpus.md` and
`sql-translation-corpus-2.md` are exactly this in unpolished form. The
cookbook is curation, not invention — and it doubles as the seed for the
adversarial eval class (each "you don't" row is a plausible-variant trap).

## 5. AI as the adoption wedge [settled as strategy]

The honest first-run story for a new language in 2026 is not "learn the
grammar" — it is **"have your assistant write it."** Ordo is unusually built
for this, and the adoption plan leans on it explicitly:

- **The grammar fits in a prompt.** Two pages (§4.3) is within any assistant's
  context. No other backend language can hand an LLM its *entire* surface.
- **Wrong output fails loudly.** The closed grammar means a hallucinated
  variant is a parse error with teaching text (§3.3), which the assistant
  reads and repairs — the retry loop converges instead of silently drifting.
  This claim is measured, not asserted: `eval-harness-plan.md`.
- **Every deployment self-documents.** The capability catalog endpoint
  (`implementation.md`) serves the callable surface — ops, params, verified
  result shapes, mode labels, cost classes — scoped to the asking token, as
  machine JSON and as an LLM-ready document. An assistant pointed at a
  deployment knows everything it may do without a human writing docs.
- **MCP surface [open, planned].** Agents already enter the model as
  principals calling registered ops "HTTP or MCP" (spec §8). A first-party
  MCP server exposing catalog + ops + compile-check makes "connect your
  assistant to your Ordo" a one-liner. Undesigned; belongs with the platform
  phase.

Ordering consequence: the grammar prompt, compile-check endpoint, and catalog
are **adoption-critical path**, not nice-to-haves — they precede the cookbook
in priority because the assistant is the first user.

## 6. Embedding as tax avoidance [settled as strategy]

Track B changes who pays the tax at all. With generated native functions
(client mode) or the linked engine (embedded mode), an application developer
calls `ordo.vendorSpend(from)` with typed params, typed results, typed errors
— and **never writes a line of Ordo**. The language is for the *domain
author*; everyone else consumes the registry through their own language's
type system.

Two audiences, quantified [lean — estimate, revisit with real users]:

| Audience | Size per team | Writes Ordo? | What they need |
|---|---|---|---|
| Domain author | ~1 (often *is* the assistant, human-reviewed) | yes — ~two pages | grammar prompt, LSP, dev loop, cookbook |
| App developer | everyone else (5–20×) | no | generated functions, call-site refinement (B2), version-skew errors (B5) |

The tax is therefore charged to a single role per team — and §5 argues even
that role pays mostly in review, not authorship. The marketing framing
follows: Ordo is not "a language your team must learn"; it is "a schema-and-
ops layer one person (or one prompt) defines, that your whole team consumes
natively." Track B's codegen-fidelity rule (a language lands complete or not
at all) is what makes this claim safe to print.

## 7. Migration paths

Nobody starts from nothing. Two on-ramps:

### 7.1 From raw Postgres [lean]
`ordo introspect <conn>` — read an existing schema, draft `.ordo` shapes:
tables → entities, FKs → refs, obvious `owned` candidates flagged (never
auto-marked — ownership is a claim, spec §3), CHECKs → `check`, indexes
carried, columns outside the closed type inventory reported as an explicit
worklist. Output is a *draft for human editing*, not a conversion — the same
posture as index advisors (suggestions land in the file for approval). Data
stays where it is; BYO-DB is already the deployment model.

### 7.2 From Supabase / Convex [lean]
Concept-mapping tables (the cookbook's §4.4 sibling, organized by source
platform): Convex tables/queries/mutations/actions → shapes/queries/write
ops/tier-2 effectors; Supabase tables/RLS/edge functions → shapes/access
profiles/routines-or-effectors. Honest rows for the gaps in both directions —
what Ordo won't express (arbitrary server code in-process) and what the
source platform can't offer back (mode labels, scope manifests, sealed
fields, the command log). No automated converter in v1; the mapping doc plus
`introspect` (Supabase is Postgres underneath) covers the credible path.

## 8. Sequenced checklist

Effort: **S** ≈ days, **M** ≈ weeks, **L** ≈ months. "Maps to" names the
existing plan item where the work already lives; ★ = adoption-critical path
(§5 ordering).

| # | Item | Effort | Maps to |
|---|---|---|---|
| 1 | ★ Two-page grammar prompt, single-sourced (prompt + doc page) | S | spec §10.2 |
| 2 | ★ Teaching-error commitment: rule+fix shape on every P1 rejection | S (ongoing) | engine-plan P1 |
| 3 | ★ `ordoc check` + compile-check endpoint | S | Track A1 / implementation.md |
| 4 | `ordoc fmt` (canonicalizer) | S | Track A1 |
| 5 | ★ `ordo dev` hot loop at the sub-second bar | M | implementation.md dev mode |
| 6 | VS Code: TextMate grammar + diagnostics | S | Track A2 |
| 7 | Full LSP (completions, hover, inlays, rename) | M | Track A2 |
| 8 | WASM compile tier + browser playground | M | Track B4 / impl. "WASM-capable" |
| 9 | `ordo new` — 2–3 starter domains | S–M | new; feeds eval corpus |
| 10 | ★ Capability catalog endpoint (JSON + LLM rendering) | M | implementation.md catalog |
| 11 | Cookbook (SQL/Convex/Supabase → idiom) | M | curates sql-translation corpora |
| 12 | TS generated functions incl. refinement builders | M | Track B6.2 — **unblocked 2026-09-07**: its input, the B1 interface descriptor, is built (`ordo interface`, `GET /interface`), including the §B3 terminal-verb split the generated types enforce |
| 13 | `ordo introspect` (Postgres → draft shapes) | M | new |
| 14 | Migration mapping docs (Convex, Supabase) | S | new; extends cookbook |
| 15 | MCP server (catalog + ops + compile-check) | M | spec §8 principals [open] |
| 16 | Python/Go/Java client-mode backends | M each | Track B6.4 |
| 17 | Playground execution tier (WASM engine + sample data) | L | Track B4 embedded mode |
| 18 | Eval harness public results (credibility for §5 claims) | M | eval-harness-plan.md |

Sequencing logic: 1–3 cost days and unblock the AI-first onboarding story
before any tooling ships; 5–8 build the loop that must feel like Convex;
9–11 compress time-to-first-app; 12+ shrink who pays the tax at all. Items
1, 2, 9, and 11 also feed the eval harness directly — adoption assets and
credibility assets are the same artifacts.

## 9. What we do not promise [settled]

No compat grammar, no SQL passthrough, no "escape hatch" query text, no
per-team dialect knobs. The moment adoption pressure argues for one of these,
the answer is a better error message, a cookbook row, a generated function,
or an assistant prompt — the tax is paid down everywhere except the grammar.
