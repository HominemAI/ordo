# Ordo AI-Authoring Eval Harness — Plan

Companion to `definition-language.md` (§1 anti-hallucination principle, §10
grammar-as-prompt), `engine-plan.md` (P1–P7), `composition-conformance.md`
(the pipeline this reuses), and `adoption-dx-plan.md` (§5 — the claims this
harness makes credible). Same markers: **[settled]** / **[lean]** / **[open]**.
Status: plan. The rest of the system is still moving; flexibility against that
churn is a design requirement here (§6), not an afterthought.

---

## 1. Purpose [settled]

Ordo's strongest competitive claim is that AI-written code cannot silently
corrupt data or overreach scope: the closed grammar makes hallucinated
variants fail to parse, mode labels and manifests make intent inspectable,
and the gate makes writes attributed and revertible. Today that claim is
argued from design. It is worth much more **measured** — and it is only
credible published, with method and corpus, reproducible by a skeptic.

Three claim families, each reduced to a number:

1. **Hallucination containment.** "A plausible variant fails to parse — never
   parses to something else." Measured: parse-reject rate on intentionally
   drifted prompts, and (the half that matters) **zero cases** where drifted
   input parses to a semantically different accepted program.
2. **No silent wrong.** "Generated apps: zero silent corruption or
   overreach." Measured: across generated queries/writes/routines that the
   platform *accepts*, the rate of outputs that differ from the oracle
   without any typed error — the **silent-wrong rate**, the headline metric.
3. **Competitive.** The same natural-language tasks, same models, same judge,
   against a raw-SQL/TS-backend baseline (and optionally Convex/Supabase):
   count silent-wrong outcomes per stack. The claim is comparative — not
   "LLMs write perfect Ordo" but "when they're wrong in Ordo, you find out."

Non-goals: benchmarking model quality per se; prompt-engineering leaderboards;
proving generated code is *good* (cost classes and review handle that) — only
that wrongness is loud.

## 2. Harness architecture [lean]

Model-agnostic, engine-decoupled, plain files throughout.

```
evals/
  tasks/<class>/<task-id>/
    task.yaml            # spec version, class, NL prompt, grammar-version pin,
                         #   context flags (catalog? schema? examples?)
    schema.ordo          # the domain the task runs against (may ref a shared fixture)
    seed.json            # seed rows for execution-tier tasks
    oracle/              # one or more oracle files (see §6.2)
  fixtures/              # shared domains + seeds (starter templates from
                         #   adoption-dx-plan §4.2 are the first fixtures)
  runs/<date>-<model>-<grammar-hash>/   # append-only results, plain JSONL
```

### 2.1 Stages
Per task: **generate → parse → bind/rules → compile-golden → execute →
compare**. Every stage after `generate` is the existing conformance pipeline
(`composition-conformance.md` pipeline design; engine-plan P6) — parse via
`ordo-syntax`, bind + compile rules via `ordo-sema`, artifact/golden via
`ordo-plan`/`ordo-artifact`, execution against seeded dockerized Postgres,
canonical-JSON compare. **The harness adds a generation front-end and an
oracle back-end to a pipeline that already exists; it does not build a second
compiler path.** A task may terminate legitimately at any stage: for
adversarial tasks, `parse` *is* the finish line.

### 2.2 Decoupling rule [settled]
The harness drives the engine **only** through the `ordoc` CLI (`check`,
`build`; Track A1) and consumes shapes **only** through `interface.json`
(Track B1). No linking engine crates, no reading internal IR. Engine churn —
parser rewrites, planner changes, new rules — cannot break the harness as
long as the CLI contract and descriptor schema hold; conversely the harness
exercises exactly the surface third parties get, so its results describe the
product, not a lab build. Error-class assertions use the closed inventory
(`composition-conformance.md` §0) — classes, never message text.

### 2.3 Generation adapters [lean]
An adapter is: model id + a function from task context to prompt to `.ordo`
text. The prompt is assembled from versioned parts: the **two-page grammar
prompt** (spec §10.2 artifact) always; the task's schema; optionally the
**capability catalog** rendering (for tasks against a deployed fixture);
optionally k worked examples. Any LLM with a text API qualifies — adapters
are ~a page each, and the harness never depends on one vendor's features
(no tool-use requirement, no structured output requirement). Retry loop:
on typed failure, feed the diagnostic back verbatim, up to N attempts —
the diagnostic-repair convergence is itself a measured quantity (§3).

## 3. Metric taxonomy [lean]

| Metric | Definition | Reading |
|---|---|---|
| **parse-reject rate** | share of adversarial-class outputs rejected at parse/bind | higher = grammar working; the paired number — drifted input *accepted as something else* — must be 0 |
| **silent-wrong rate** | accepted programs whose output ≠ oracle with no typed error, over all accepted programs | the headline; target 0, report honestly |
| **loud-wrong rate** | wrong but rejected/errored (parse, bind, check, typed runtime) | Ordo's design converts silent→loud; this is where the mass should sit |
| **scope-overreach caught** | generated ops whose field-scope manifest exceeds the task's stated scope, flagged by manifest diff | manifests as tripwire — caught at review time, pre-deploy |
| **cost-class violations caught** | ops classifying `unbounded-host` without the marker → `unacknowledged_unbounded` | the gate working: no accidental O(space) dashboards |
| **retry-to-success curve** | attempts-to-first-passing per task, per model | measures "errors as teaching" for machines; median should be small and falling |
| **token cost per working app** | total tokens (incl. retries) to a passing app-level task | the economics of "have your assistant write it" |

Silent-wrong is scored per stack identically in competitive runs (§4.5):
for baselines there is no manifest/mode-label/gate stage, so their wrongness
has fewer chances to become loud — that asymmetry *is* the result.

## 4. Eval classes

### 4.1 Micro [lean]
Single query or write from an NL spec, against a fixture schema + seed.
Oracle: exact rows (canonical JSON), or expected write effect (post-state
rows + expected mode label). Hundreds of tasks, cheap, run per-commit.
First corpus source: the conformance corpus's exec cases re-expressed as NL
specs, plus the SQL-translation corpora (each entry is already an NL-ish
intent with a known answer).

### 4.2 Adversarial [lean]
Prompts seeded with **plausible-variant traps**: SQL idioms (JOIN…ON,
HAVING, OFFSET, CASE WHEN, ON CONFLICT DO UPDATE), precedence assumptions
(`a and b or c` unparenthesized), synonym spellings (`filter`, `where`,
`from`, `!=` folklore on null), three-valued-null expectations, window
functions. Two sub-kinds: (a) the *prompt* pushes toward the trap but a
correct Ordo answer exists — success = correct Ordo, failure-mode of
interest = drifted output, which must reject; (b) the prompt requests the
inexpressible — success = the model declines or the output rejects loudly
(`parse_error` et al.), catastrophic failure = accepted-but-different.
Cookbook rows (`adoption-dx-plan.md` §4.4) and G14's intentionally-absent
list are the trap inventory.

**Six traps belong in the seed set on day one, because each has already been
observed defeating a real generation run** (`traps.md` owns the prose; this is
the eval-facing list):

| Trap | Correct answer | Catastrophic failure |
|---|---|---|
| Compare two independently grouped aggregates ("fewer than half the members finished") | two `let`s, provenance join, `coalesce` on both sides, compare by multiplying | one pipeline (must reject), or a `let` version missing the `coalesce` — **accepted and silently wrong on every empty group** |
| Write-open | `access write open` | `access write true` (must reject); omitting `access write` (must reject) |
| Money split | `allocate(total, weights)` | independent `round`s per share — accepted, sums to less than the total |
| Money division into a field | explicit `round`/`floor`/`ceil` at the boundary | `decimal` assigned to `money` (must reject) |
| "My team's tasks" | a guard or acl relationship | `assignee == $caller` — accepted, returns nothing, and looks like an empty result rather than an error |
| Listener routine over a guarded entity | scoped to `$trigger.ids`, or `run … unguarded:` stated | an unscoped listener body (must reject) |

The pattern worth noticing across the rows: the *loud* failures are cheap, and
the two rows marked "accepted and silently wrong" are the ones the silent-wrong
metric exists to catch. Weight them accordingly when the corpus is sampled.

### 4.3 App-level [open — platform-gated]
The north-star test: an NL product spec ("a two-page finance tracker with…")
→ generated ~two-page domain → `ordo apply` to a scratch deployment → a
**behavioral suite** runs against the deployed API (through the generated SDK
or raw op protocol): CRUD flows, invariant probes (quota `check`s hold under
concurrent writes), scope probes (a viewer-role token cannot reach manager
ops), pagination determinism. Oracle: behavioral assertions + invariant
checks, not golden rows — many correct schemas exist for one spec. Needs the
platform runtime (`apply`, access model, catalog); design now, run later.

### 4.4 Longitudinal [lean]
The same task corpus re-run per (model version × grammar version), results
appended to `runs/`, never rewritten. Answers: is the grammar getting easier
for models over time; did a spec change regress parse-reject or repair
convergence; which models are safe to recommend in docs. Cheap by
construction — it is just "keep the old runs and plot."

### 4.5 Competitive [lean]
Matched tasks (the micro + app-level corpora, stack-neutral NL phrasing)
generated for baseline stacks: raw SQL + TypeScript handlers first
(no-framework baseline); optionally Convex and Supabase variants. Same
models, same retry budget, same judge, same seeds. Scored on the same
taxonomy — with the honest note that baselines lack a manifest/gate stage,
so "scope-overreach caught" becomes "scope-overreach *occurred*," verified
by the behavioral probes. Publish the task corpus and judge with the
numbers or the comparison is marketing, not evidence.

## 5. Flexibility requirements [settled as requirements]

The engine, spec, and platform are all moving. The harness survives by:

1. **Versioned task format.** `task.yaml` carries `format: <n>`; the runner
   supports current and previous. Plain files, no database, no bespoke
   binary formats — a task is reviewable in a PR and greppable forever.
2. **Grammar-version pinning.** Every task and every run records the
   **spec hash** (the version hash `ordoc build` already emits) plus the
   grammar-prompt revision. Results are always date-stamped against a
   specific language state; a spec change never silently invalidates old
   numbers — it starts a new longitudinal series.
3. **Oracle plurality.** Four oracle types, extensible: **exact rows**
   (canonical JSON), **invariant checks** (queries over post-state that must
   hold), **property tests** (relations between outputs, e.g. group sums
   equal reduce total), **error-class expectations** (this input must reject
   with this class). App-level adds behavioral suites. A task declares which
   oracles apply; the runner treats them uniformly as pass/fail with
   evidence.
4. **Pluggable judges.** Deterministic comparison wherever possible (rows,
   classes, invariants). Where judgment is unavoidable (does this schema
   satisfy the spec's intent?), the judge is a declared, versioned component
   — rubric-based LLM judge or human — recorded in the run metadata, and
   swappable without touching tasks.
5. **CLI-only engine contact** (§2.2) — restated as a requirement because it
   is the load-bearing one.

## 6. Publish vs. keep internal [lean]

The harness is a marketing weapon only if the method is public and the
numbers are unflattering-capable.

**Publish:**
- The headline metrics per release: silent-wrong rate, parse-reject rate
  (with the accepted-as-something-else count, expected 0), competitive
  table.
- The full adversarial and micro task corpora, fixtures, seeds, judge
  rubrics, and runner — reproducible by anyone with an API key. The
  two-page grammar prompt is already public by design.
- The methodology note, including what the harness does *not* claim (§1
  non-goals) and known corpus biases.
- Longitudinal charts once ≥2 grammar or model generations exist.

**Internal:**
- Raw generation transcripts (model-vendor terms, noise).
- Per-model failure taxonomies while models churn — publish aggregates,
  not scoreboards; the product claim is about Ordo, not about ranking
  vendors.
- Prompt-assembly experiments (k-examples, catalog-inclusion ablations) —
  interesting, unstable, not claims.
- App-level results until the platform stages are real enough that a
  failure means something (a harness bug published as a product number is
  unrecoverable).

Rule: nothing publishes without the corpus + runner that produced it.

## 7. Sequencing [lean]

| Stage | Needs | What runs |
|---|---|---|
| now | nothing built | task format spec'd; adversarial + micro corpora authored from conformance cases, translation corpora, cookbook traps; grammar prompt drafted (shared with adoption plan item 1) |
| P1–P4 land | `ordoc check`/`build` | **compile tier**: full adversarial class (parse/bind/reject metrics), micro-class golden-artifact + mode-label + manifest + cost-class checks — parse-reject and scope/cost tripwire numbers exist here, before any database |
| P5–P6 land | write compilation + conformance runner | **execution tier**: micro-class row/post-state oracles against seeded Postgres; silent-wrong rate becomes measurable — first publishable headline |
| interface.json (B1) | descriptor | harness consumes shapes/manifests through the public artifact; decoupling rule fully in force |
| competitive baselines | none of the above | independent — baseline stacks need only Docker + the corpus; can start alongside the compile tier |
| platform (`apply`, access, catalog) | platform phase | app-level class; catalog-in-prompt adapters; token-cost-per-app |
| ongoing | any run ever completed | longitudinal series from the first run onward |

The deliberate consequence: **the two claims that sell — hallucination
containment and silent-wrong — are measurable the moment the engine's P1–P6
land**, on exactly the artifacts the engine plan already commits to building.
The harness's own new code is the generation adapters, the oracle runner,
and the corpus: small, boring, and decoupled — as it must be, since
everything it measures is still moving.
