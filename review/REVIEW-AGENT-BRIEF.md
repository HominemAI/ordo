# Briefing: run the Ordo design review

Copy everything below the line into the other agent system as its task prompt.
It assumes the agent runs on this machine with shell + file access.

---

## Who you are and what you're doing

You are running an adversarial design review of **Ordo**, a pre-alpha
backend platform, and producing a **versioned report**. Ordo is spec-first:
most of it is written down and not yet built, so you are reviewing *design
documents plus a partial Rust implementation*, not a shipping product.

Ordo in one paragraph: a closed, deterministic definition-and-query language
that compiles to PostgreSQL, wrapped in a platform (auth, storage, realtime,
automation, generated UI, audit). Developers declare *shapes* (entities) and
*ops* (registered queries/writes); the compiler derives the SQL, DDL, HTTP
endpoints, permissions surface, and audit machinery. Every deployment splits
into **spaces** (one world of data, membership, keys, history each) with a
per-space **custody dial** running from platform-managed encryption to fully
client-held keys, and a hash-chained **command log** under every write. The
central bet: a grammar rigid enough that a wrong LLM emission *fails to parse*
rather than parsing into something subtly wrong.

## Repo layout

```
~/DevWorkspace/ordo/
  README.md              honest status; start here
  docs/                  the design corpus (~25 files, ~600 KB)
    definition-language.md    THE spec — language, guards, access model
    implementation.md         platform: custody, deployment, catalog, client-first
    glossary.md               vocabulary (read early)
    tutorial.md               Part 1 single-user, Part 2 multi-tenant
    traps.md                  known ambushes
    modeling-guidance.md      space granularity, sharing, authorization modeling
    std-library.md            platform-derived ops
    ui-gen.md                 surface descriptors + app manifest
    api-engine-design.md      HTTP surface, errors, methods-by-kind
    security-logging-design.md  identity providers, command log
    engine-plan.md            build plan + lexical rulings (G1–G14)
    migration.md              migrating in/out, genesis import
    *-conformance.md          executable test corpora (composition, guards)
    critique-consolidated.md  PRIOR review output — 86 findings, X-001…
    security-hardening-review.md  PRIOR security review — SH-1…SH-31
  crates/                Rust workspace (parser, api, identity, commandlog)
  review/                where your output goes
~/.claude/skills/design-review-pipeline/   the pipeline itself
```

**Read `SKILL.md` in that skills directory first** — it defines the whole
method (five tracks, deterministic gates, anti-patterns). Follow it.

## Step 1 — deterministic gates (do this before any agent work)

```bash
cd ~/DevWorkspace/ordo
python3 ~/.claude/skills/design-review-pipeline/checks/run_checks.py \
  --config review/checks.config.json \
  --out review/report.json \
  --html review/report.html \
  --history review/history \
  --label "<short-name-for-this-round>"
```

This is free and instant. It emits machine-readable findings, a self-contained
dashboard, and archives the run so the next one can diff against it. Label runs
by *what changed* ("baseline", "post-fix-round-2"), never by date.

Twelve gates run: code-block lexical conformance, link/anchor integrity,
retired-vocabulary scan, status-marker coverage, spec↔lexer keyword parity,
documented↔implemented error codes, conformance-ruling coverage, build-plan
pillar coverage, glossary coverage, absolute-claim registration, cross-doc
constant agreement, and site hygiene. Read the output before doing anything
else — anything a gate already found is not worth an agent's attention.

Also run `cargo test --workspace` (needs `export PATH="$HOME/.cargo/bin:$PATH"`)
and record the result; the crates must stay green.

## Step 2 — the agent tracks

Per the skill, run these as **separate agents with no shared context**, in
parallel, each read-only unless writing its own review file:

1. **Expert formal review** — a research mathematician and a PhD computer
   scientist, run as distinct personas. Demand *counterexamples, not opinions*.
   Target: null/equality semantics, decimal and duration algebra, ordering and
   pagination guarantees, path-inference definitions, concurrency and isolation,
   client-first merge semantics, decidability of any static analysis claimed.
2. **Security hardening review** — threat-actor list first (curious operator,
   compromised worker, malicious tenant, malicious guest, stolen device with a
   local mirror, supply chain, network adversary), then findings with attack
   path → proposed pin → *what the fix must not break*. Writes
   `docs/security-hardening-review-N.md`, everything marked `[proposed]`.
3. **Skeptic persona panel** — research real practitioner critiques of Supabase,
   Firebase, Cloudflare's dev ecosystem, Convex, and local-first/CRDT tools;
   build 3–5 personas who hate existing tech *and* new tech, each with a
   documented scar; have each read as themselves and produce verbatim
   Discord/HN-style reactions, walk-away reasons, grudging compliments, and a
   pilot verdict.
4. **AI-buildability test** — same build brief to two tiers. Cheap tier gets
   only README + glossary + tutorial + traps and must emit `# BUILDER NOTES`;
   strong tier gets the full spec and must emit `# SPEC FRICTION`. Judge both:
   would it compile per the spec, is it idiomatic, **and which errors are the
   docs' fault?** Do not execute anything.

If a prior round exists (`critique-consolidated.md`), each track must also
**verify the prior fixes actually close their holes** — a fix that relocates a
problem is itself a finding.

## Step 3 — consolidate

One agent merges everything into `docs/critique-consolidated-N.md`: stable IDs
cross-referencing origins, aggressive deduplication (a finding found by two
independent tracks is a severity multiplier — say so), recorded reviewer
disagreements, a doc-vs-doc contradiction table, what reviewers agreed is
strong, a P0/P1/P2 plan sequenced doc-fixes-before-code, and the open questions
only the owner can settle.

Findings that exist only in an agent's chat reply must be pasted **verbatim**
into the consolidator's prompt — summarizing loses exactly the detail that
matters.

**Also emit `review/findings.json`** — an array of
`{id, title, severity, area, origin, status}` — and point
`external_findings.json_file` in `review/checks.config.json` at it. That makes
panel findings round-trip into the dashboard by ID, so the next round can
assert "X-017 closed" instead of re-litigating it.

## Step 4 — re-run the gates and hand over

Re-run the Step 1 command with a new `--label`. The dashboard will show a delta
strip (closed / new / carried) and an across-runs trend.

Deliver:
- `review/report.html` — open it; it is self-contained, no server needed
- `review/report.json`, `review/findings.json`, `review/history/run-*.json`
- the consolidated critique document
- a short written summary: what the gates caught, the fatal findings, what the
  prior round's fixes actually closed, and the decisions that need the owner

## Ground rules

- **Never soften.** Accuracy over encouragement; lead with anything fatal.
- **Never review with the design's own context.** Fresh agents, cold eyes.
- **Non-overlapping file ownership** for anything that writes; agents die
  mid-flight and leave broken states, so verify the build yourself afterward.
- Do not commit anything. Do not modify `docs/` except the review files named
  above.
- Prior-round context lives in `critique-consolidated.md` §2 (fatal set) and §8
  (open questions) — read those before deciding what to re-test.
