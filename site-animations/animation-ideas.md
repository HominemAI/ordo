# Animation ideas

Candidate concepts for future interactive animation pages, in the style of the four
built so far (single-file, anime.js, camera-grid scenes, typed captions, click-gated).

Legend: ✅ built/tested in the codebase today · 📐 settled spec, not yet built ·
⭐ highest animation payoff. Claims should be re-verified against main before
building — see the fact-check pass on The Ordo Flow (Sep 2026) for the bar.

## Built pages (published artifacts)

| Page | File | Artifact | Covers |
|---|---|---|---|
| The Ordo Flow 💧 | `process-flow.html` | https://claude.ai/code/artifact/7fb33553-5814-47b2-9f47-525b49e1da92 | The dev experience end to end: write (ordo + planned CDK tab) → plan/apply machine → generated surface → live app |
| Sovereign Spaces 🗝️ | `sovereignty.html` | https://claude.ai/code/artifact/233d1ad4-2e03-4b53-92ac-73071f6f4141 | Spaces, custody dial, hash-chain + tamper demo, consented feeds |
| Writing Ordo ✍️ | `writing-ordo.html` | https://claude.ai/code/artifact/0665ad88-8c55-4603-9c91-a12773c730c7 | Shapes with annotations, watchable read pipeline, derived write modes, routines teaser |
| Where Rows Live 🗄️ | `storage.html` | https://claude.ai/code/artifact/0cce8132-60fa-462a-987b-e37b250674a1 | DDL lowering, $space tenancy, life of one write, verify/checkpoint/erase |
| The Ordo Guide 📘 | `ordo-guide.html` | https://claude.ai/code/artifact/60b4e38b-6c48-43d6-9a68-b36ac9db3d13 | Stripe-docs-style written reference across all of the above, with diagrams and built/spec status tags |

## Language & grammar

- ⭐✅ Derived write modes (no insert/update/delete; the statement *is* the mode) — in Writing Ordo
- ⭐✅ Read pipelines you can watch (filter → shape → sort → take) — in Writing Ordo
- ✅ Loud refusals — a gallery of errors that refuse politely (`[parse_error]`, `[id_key_collision]`, sealed-sort refusal); the compiler as a character
- ✅ Typed ids (`plnt_01J8…` — prefix derivation rules, collision fails the compile)
- ✅ First-class durations (`7d` → unit-safe arithmetic; no ms bugs)
- ✅ Upsert-by-key lifecycle (converge-or-create; the alert resolve/re-raise dance)
- ✅ Defaults are literals; `= today` belongs to the writer, visibly
- 📐 Guards / the access model (the ~87-case conformance corpus as a gauntlet)

## Sovereignty & custody

- ⭐✅ The custody dial (managed → sealed → client-held) — in Sovereign Spaces
- ⭐✅ Blind indexes — "what the attacker sees" split-screen: equality at SQL speed while sort/range refuse to compile
- ✅ Sealed fields & the 16-byte log handle (plaintext never touches the log)
- ✅ Spaces as sovereign units (membership, keys, history per space) — in Sovereign Spaces
- ✅ Crypto-erasure (destroy an epoch key → provable deletion) — in Where Rows Live
- ✅ normalize(lower) — what "the same email" means; unicode lookalikes

## The log & proof

- ⭐✅ Hash chain + tamper demo — in Sovereign Spaces
- ✅ Signed checkpoints (Ed25519; verify starting mid-chain) — in Where Rows Live
- ✅ Verify / revert; genesis of zeros
- ✅ "No raw UPDATE path" — literally the parser's error text

## Storage & tenancy

- ⭐✅ $space in every key; cross-space FK refused — in Where Rows Live
- ✅ Verbatim quoted identifiers (one name in source, DDL, log)
- ✅ plan's exact DDL diff / apply's versioned artifact (apply runs no DDL)
- 📐 Schema evolution / migrations (plan-schema-evolution.md)

## Runtime & API

- ✅ Ops as the only surface (`/op/{name}`, typed params — "the API that can't sprawl")
- ⭐✅ The 403 byte-identical to "never existed" + the catalog that omits rather than marks — cinematic security story, proved by test
- ✅ Runtime access profiles (viewer/manager/editor/owner; change who calls what without recompiling)
- ✅ Version headers / skew detection (x-ordo-version-skew)

## Automation (engine ✅, wiring 📐)

- ⭐ Everything-is-an-event — webhooks, cron, email, SMS, clicks all reducing to two primitives (listener + clock); "five rivers, one bed"
- ✅ Debounce / coalesce / dead-letter — a noisy event stream visibly tamed
- ✅ Trigger provenance bound (R-G20: a routine may only touch what woke it)
- 📐 Routes with fallback (push → email) and the egress rail

## Sharing & interface

- 📐 Consented feeds — in Sovereign Spaces (tagged spec-only)
- 📐 Delegation surface / agents acting as themselves
- ✅ The Interface Descriptor → headless React ("everything wired, nothing rendered")
- 📐 Migration-in (`ordo introspect`, genesis import — "moving day")

## Next up (recommended order)

1. **Everything-is-an-event** — distinctive thesis, strong visuals, engine already built and Postgres-tested
2. **The refusal gallery / 403-identical story** — security posture as personality; every claim already proved by tests

## Build notes (learned the hard way)

- Next-arrows need explicit click listeners — `armNav()` only styles; wire
  `Object.entries({'go-x':'x',…})` per page
- A reveal container must not keep `.rv` if only its children are animated
- Chrome caches the local test server's pages — cache-bust (`?v=n`) when verifying
- Fact-check every code/DDL/API string against main before publishing; the artifact
  carries a "spec-tag" pattern for settled-but-unbuilt features
