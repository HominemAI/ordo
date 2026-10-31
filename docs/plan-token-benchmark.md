# Token benchmark: what a working app costs an AI agent on Ordo

Status: **plan, 2026-10-29.** Nothing built, nothing run. The owner's
decisions are the questions in §9.

The owner's words (2026-10-29): measure how many AI tokens it takes to build a
working app on Ordo against conventional stacks, and line the method up with
benchmarks we can cite.

Why Ordo should win: one definition file builds tables, migrations, SQL, the
API, sign-in, permissions, audit and a typed SDK, all deterministically. The
agent writes the definition. The platform writes the rest. Only the full user
interface is left to hand-written code, and this benchmark leaves UI out.

---

## 0 · The plan on one screen

- **Claim:** the same app spec, model and harness cost fewer tokens per
  passing app on Ordo, with permissions at least as correct.
- **Tasks:** 6 app specs (3 re-expressed from BaxBench, 3 of our own) plus 4
  change requests on an existing app. Each has one hidden API-level test
  suite, shared by every stack, that also checks permissions and tenant
  isolation.
- **Stacks:** Ordo, Supabase, and plain TypeScript on Postgres.
- **Harness:** Claude Code headless in a fresh container per trial. 5 trials
  per cell. Fixed caps. Tokens read from the API usage fields.
- **Report:** HAL style. Pass rate, cost per passing app with failures
  included, pass@k, and a cost against accuracy chart.
- **Cost:** pilot about $70, full run about $900 (cap $1,500). Harness is
  about 1 L and 2 M of agent work.
- **Fit:** this is the app-level class (§4.3) and the competitive class
  (§4.5) of `eval-harness-plan.md`, measured on its "token cost per working
  app" metric. It reuses that plan's layout, oracle types and retry loop.

## 1 · The claim, stated so it can fail

**Claim.** For the same app spec, model, harness and caps, the total cost of
building a passing app on Ordo, counting failed trials and retries, is at
most half that of the best baseline. Ordo's permission tests pass at an
equal or higher rate.

The threshold (half) is pre-registered before the pilot and not moved after.
It is owner question 4.

**Results that count against Ordo:**

- Cost per passing app is not below the best baseline by the margin, or the
  95% intervals overlap.
- Ordo passes fewer apps. A cheap failure is not a saving.
- Ordo's permission or isolation pass rate is lower than a baseline's.
- The saving holds on greenfield builds and vanishes on change requests.
- The saving holds only when Ordo's grammar prompt is left out of the count.
  It is always counted as input.

Each of these is reported as found, in the same table as the wins.

## 2 · Tasks

Each task is a stack-neutral prose spec plus an operation list: names,
inputs, outputs, who may call each one. The agent sees the spec and 1 smoke
test. The full suite stays hidden.

**Greenfield builds (6):**

| Task | Source | Why |
|---|---|---|
| 3 BaxBench scenarios | BaxBench, data-centric ones only | Citable outside origin. Scenarios that need free computation (image work, shell) are skipped, since Ordo is a data platform. Which 3 is chosen after reading the paper's list. |
| notes v1 | `examples/notes/` | Owned rows, one-at-a-time read sharing. Small. |
| plantcare | `README.md` | Single user, a daily routine, alerts. |
| teamspace | `examples/teamspace/` | One tenant, roles, invitations. The hardest permission model. |

**Change requests (4).** Each starts from a passing reference solution for
that stack, written by us, so every stack starts equal. Seeded rows are
written before the change and checked after it: live data must survive.

1. notes v1 to v2: pins, archive, reminders (files left out).
2. plantcare: share a plant with a second person, read-only.
3. teamspace: add a role with narrower rights.
4. A BaxBench scenario: add a field and a filter.

Nobody we found measures change requests. Most real token spend is there.

**Oracle.** One behavioural suite per task, run through a thin per-stack
adapter that turns neutral calls ("alice creates a note") into that stack's
API. The adapters are written once by us, reviewed, and published. Every
suite includes:

- functional flows (create, read, list, update, remove);
- permission probes: a second person, a person outside the tenant, and a
  signed-out caller each try every operation they must not reach;
- input probes from BaxBench's security checks where they fit.

This is the "behavioural suites" oracle of `eval-harness-plan.md` §5.3.

## 3 · Stacks

| Stack | Why |
|---|---|
| **Ordo** | The subject. Agent gets the CLI, the grammar prompt, `tutorial.md` and `traps.md`. |
| **Supabase** (Postgres, row level security, supabase-js) | The closest conventional backend as a service. Has auth and policies, so permissions are a fair comparison. |
| **Plain TypeScript on Postgres** (Hono, Drizzle, a session library) | What an agent writes when nothing is chosen for it. The no-framework baseline of §4.5. |
| Convex (optional) | Runs its own Fullstack-Bench. Adds a citable bridge if budget allows. |

## 4 · Harness

- **Agent:** Claude Code headless (`claude -p`, JSON output), the same
  version for every trial. It reports per-run usage and cost (fields to
  confirm before building, §8). It is the tool the owner's users would use.
- **Models:** Sonnet 5.5 and Opus 5.5, effort fixed per model and logged.
  A non-Anthropic model through a neutral harness (HAL's) is a later add.
- **Container:** fresh per trial. Stack scaffold, Postgres, the stack's docs
  folder, no network beyond package installs.
- **Prompt:** one template for all stacks. Only the stack name, docs path and
  run command differ.
- **Docs:** each stack gets its official AI docs (Ordo's grammar prompt,
  Supabase's AI guidelines, the TS libraries' README files). Baselines also
  run without vendor docs, and the better arm is reported.
- **Caps:** 150 turns, 60 minutes, $15 per trial. Hitting a cap is a fail,
  and its tokens count.
- **Retry loop:** after the agent stops, the hidden suite runs. Failing test
  names and one-line messages go back, up to 3 rounds. This is the
  diagnostic loop of `eval-harness-plan.md` §2.3, fed by tests.
- **Trials:** 5 per cell (task by stack by model).
- **Token capture:** per API call, 4 counts kept apart: input, output, cache
  read, cache write. Summed per trial, including subagents. Price table
  logged with its date in the run record.
- **Results:** `evals/runs/<date>-<model>-<grammar-hash>/`, plain JSONL,
  as §2 of the eval plan lays out. Tasks live in `evals/tasks/app/<id>/`.

`eval-harness-plan.md` §2.3 says adapters need no tool use. That stays true
for micro and adversarial tasks. This plan adds a second adapter kind, the
agentic one, for app-level tasks only.

## 5 · Metrics and reporting

Per cell:

- pass rate (all hidden tests), and permission pass rate on its own;
- tokens per trial, the 4 counts apart, and dollars;
- **cost per passing app**: all dollars spent in the cell divided by passing
  trials;
- output tokens per passing app (the Tokenmaxxing paper's measure);
- tokens spent after the first failed build or test, apart from the rest:
  where that paper finds the extra spend;
- pass@1 and pass@5, turns, retry rounds, wall time;
- a cost against pass rate chart per model, Pareto front marked, as HAL does.

Spread is a bootstrap 95% interval over trials. Medians, not means, for
token counts: per-task spread can be very wide.

**What we will not claim from this sample:**

- a percentage saving for app sizes we did not run;
- anything about UI work;
- anything about models we did not run;
- a winner in any cell whose intervals overlap.

## 6 · Fairness and threats to validity

| Threat | What we do |
|---|---|
| We wrote Ordo and the tasks. | Specs and suites written stack-neutral before any Ordo solution. Claim and threshold pre-registered. Everything published. |
| Our baseline solutions could be weak. | Reference solutions per stack must pass the suite, proving each task is doable. A reviewer fluent in each stack reads them. |
| Models know TypeScript and Supabase far better than Ordo. | This favours the baselines. Report it. It makes an Ordo win stronger and a loss less telling. |
| Grammar prompt against framework docs. | All docs are counted as input tokens. Baselines run with and without vendor docs. |
| Harness vendor: Claude Code is tuned on general stacks. | Same harness for all. A neutral harness and model later. |
| The adapter layer could favour one stack. | Adapters are small, reviewed and published. |
| Contamination: BaxBench is public. | Our own tasks are new. Report results for both groups apart. |
| UI is excluded. | Stated up front. Ordo does not generate full UI, so the claim is about the backend only. |
| Prompt caching hides cost differently per stack. | 4 counts reported apart, and dollars with cache. |

## 7 · Cost and steps

**Dollars (prices as of 2026-10-29, checked again on run day).** Opus 5.5:
$4 in, $20 out, $0.20 cache read per million tokens. Sonnet 5.5: $2 in, $10
out, $0.20 cache read. Cache writes cost more than plain input.

A trial is estimated at about 2M input tokens, 90% of them cache reads, and
60k output: about $3.50 on Opus, about $2 on Sonnet.

| Run | Trials | Estimate |
|---|---|---|
| Pilot: 2 tasks, 3 stacks, 3 trials, Sonnet | 18 | about $70 |
| Full: 10 tasks, 3 stacks, 5 trials, 2 models | 300 | about $900 |
| Baselines without vendor docs | +100 | about $250 |

Owner question 2 sets the cap.

**Steps and sizes.**

1. Specs, hidden suites, per-stack adapters and reference solutions for 10
   tasks. **L.** Paced by the owner's review of the task list.
2. Runner: containers, headless agent, usage capture, retry loop, JSONL
   results. **M**, 1 to 2 CI rounds.
3. Pilot, fix the harness, freeze it. **M.** Paced by the pilot result; it
   may take 2 tries.
4. Full run. **S** of agent time; a few hours of wall time, paced by rate
   limits.
5. Report and charts. **S.**

## 8 · Research to check before citing

These come from search snippets, partly unverified.

| Source | What we use | Check |
|---|---|---|
| Convex Fullstack-Bench, convex.dev/llm-leaderboard | Closest prior work: 3 apps across Convex, Supabase and a traditional stack. Vendor-run. | Whether it reports tokens at all; its app specs; its test method. |
| Supabase Evals (open source) | Agent tasks on Supabase. | That it exists as described; no token reporting found. |
| BaxBench, arxiv.org/abs/2502.11844 (ICML 2025) | 392 tasks, 28 scenarios by 14 frameworks, OpenAPI spec, functional and exploit tests. | The scenario list; the licence for re-expression. |
| HAL, arxiv.org/pdf/2510.11977, princeton-pli/hal-harness | Dollar cost per agent, Pareto accuracy against cost. | Its exact cost method. |
| "The Best Programming Language for Tokenmaxxing", arxiv.org/pdf/2607.22807 (LMPL 2026) | Language choice changes output tokens; extra tokens come from compile failures and revising passing code. | That the paper exists at this id and venue, and its findings. |
| "Efficient Benchmarking of AI Agents", arxiv.org/pdf/2603.23749 | 400x per-task cost spread on SWE-bench Verified; most benchmarks run 1 trial, Terminal-Bench 5. | The 400x figure and the trial counts. |
| SWE-ABS, arxiv.org/pdf/2603.00520 | Tokens and cost per instance. | What it reports. |
| Claude Code headless JSON output | Per-run usage and cost fields. | Field names, and whether subagent usage is included. |

**Gaps nobody seems to measure:** tokens for a whole app from one spec
across stacks (Convex is closest, and vendor-run); tokens for a change
request on an existing app; a closed definition language against general
stacks; permission correctness beside token cost.

## 9 · Owner answers, 2026-10-29

These outrank the sections above where they differ.

- **Stacks:** all 3 baselines: Supabase, plain TypeScript on Postgres, and
  Convex.
- **Scope:** 1 small, working app first, as near free as possible. The
  10-task set waits for funding.
- **Cost:** runs go through Claude Code on the owner's plan, so they use its
  allowance and no API dollars. The dollar figures in §7 become token counts,
  priced only for comparison. Databases, Supabase and Convex run locally.
- **Telemetry:** save everything: transcripts, usage records, test output,
  container logs, versions. Publish the results, with the raw data kept for
  proof.
- **Win and vendor docs:** as recommended below.
- **First run (answers later the same day):** a trimmed team task
  tracker; Ordo against Supabase only, as soon as possible; each trial is
  one Fable planning pass using the stack's own AI guidance, then Sonnet
  builds from the plan. Both phases count. Plain TypeScript and Convex come
  in a later run.

## 10 · Questions for the owner, as first asked

1. **Which baselines?** Recommendation: Supabase and plain TypeScript on
   Postgres. Add Convex only if the budget allows, since it runs the closest
   rival benchmark.
2. **What budget?** Recommendation: cap the pilot at $100 and the full run at
   $1,500, with the full run gated on the pilot result.
3. **Which models?** Recommendation: Sonnet 5.5 and Opus 5.5 through Claude
   Code headless. Add one non-Anthropic model through a neutral harness in a
   second round.
4. **What margin counts as a win?** Recommendation: Ordo claims a win only if
   its cost per passing app is at most half the best baseline's, intervals
   apart, and its permission pass rate is no lower. Fixed before the pilot.
5. **Publish?** Recommendation: publish the method, tasks, suites, adapters,
   runner and results whatever they show, after the full run. Keep raw
   transcripts internal, as `eval-harness-plan.md` §6 says.
6. **Vendor AI docs for baselines?** Recommendation: run baselines both with
   and without their vendor's AI guidelines, and compare Ordo against the
   better of the two.
