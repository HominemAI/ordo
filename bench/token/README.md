# Token benchmark: one app, Ordo against Supabase

How many tokens it costs Claude Code to build the same working app on each
stack. Plan: `docs/plan-token-benchmark.md` (§9, the owner's answers, wins).

**Status, 2026-10-29:** harness built. No real trial has run.

- Ordo: the reference passes the suite, 26 of 26. The dry run passes, and
  the broken dry run fails round 0 and recovers in repair round 1.
- Supabase: not yet run against real Supabase, since Docker was not up.
  The reference migration applies to plain Postgres 17 with Supabase's
  `auth` stubbed, and its row level security passed 25 hand checks there.
  The reference page builds. See "Checking the Supabase side".

**Two Ordo series.** Ordo runs before 2026-10-30 got the SDK as TypeScript
source in `vendor/`, and the planner read about 250 KB of it. Since then
the SDK arrives as built packages in `node_modules`, the way supabase-js
does, and `docs/ai-guide.md` carries the API signatures. The old runs used
a different workspace, so they are a separate series: do not average them
with new ones.

## The first run, as ruled

- **One app:** a trimmed team task tracker (`spec/app.md`). Teams, 2 roles
  (owner, member), tasks with a title, a done flag and an assignee.
- **Two stacks:** Ordo and Supabase.
- **Each trial, two phases, both counted:**
  1. Plan: `claude-fable-5-1` reads the spec and the stack's own AI
     guidance, and writes `PLAN.md`.
  2. Build: `claude-sonnet-5-5` builds from the plan.
- **Same interface on both stacks:** one stack-neutral spec, the same
  screens, labels and roles. One hidden suite runs against both.

## What is where

| Path | What |
|---|---|
| `spec/app.md` | the neutral spec: rules, screens, exact labels |
| `spec/bindings/<stack>.md` | how the spec maps onto each stack; appended to the spec as `## Stack` |
| `guidance/ordo/MANIFEST` | the Ordo guide a trial gets (`docs/ai-guide.md`), copied from this repo at the run's commit |
| `guidance/supabase/` | Supabase's official AI prompts, pinned (`SOURCE.md`: URL, commit, date, hashes) |
| `starter/` | the empty Vite React page, plus each stack's overlay |
| `stacks/<stack>/up.sh` | starts one app: backend, mail catcher, page on `http://localhost:5310` |
| `suite/` | the hidden Playwright suite and the per-stack adapters. Agents never see it |
| `reference/<stack>/` | our own builds. Both must pass the suite, which shows the suite is fair |
| `runner/` | one trial (`trial.mjs`), a batch (`batch.mjs`), the scores (`score.mjs`) |
| `runs/` | results. Ignored by git: transcripts and bundles are large |

## Before the first run

1. Node 20 or later.
2. The suite, once:
   ```sh
   cd bench/token/suite && npm install && npx playwright install chromium
   ```
3. **Ordo:** build both binaries from the commit you run from, and put them
   on PATH (or set `ORDO` and `ORDO_HOST_BIN`). Postgres 17 binaries at
   `/opt/homebrew/opt/postgresql@17/bin`, or set `ORDO_PGBIN`.
   ```sh
   cargo build -p ordo-cli -p ordo-host
   ```
4. **Supabase:** Docker running, and the `supabase` CLI on PATH. The first
   `supabase start` pulls its images, so run the dry trial once first.
5. Claude Code signed in to the owner's plan (`claude --version` works).

## Running

```sh
cd bench/token

# One real trial.
node runner/trial.mjs --stack ordo
node runner/trial.mjs --stack supabase

# A batch: stacks interleaved, never overlapping.
node runner/batch.mjs --stacks ordo,supabase --trials 3

# The scores (plan §5) and the win test (plan §1).
node runner/score.mjs
```

Dry runs spend nothing. A stub stands in for the agent and copies the
reference build in. Use them to check the machine before a real run.

```sh
node runner/trial.mjs --stack ordo --dry          # should PASS, 26 of 26
node runner/trial.mjs --stack ordo --dry=broken   # fails round 0, the stub repair fixes it
node runner/score.mjs --include-dry
```

Options for `trial.mjs`: `--out <dir>` (default `runs/`), `--work <dir>`
(where the throwaway workspace goes, default the system temp folder),
`--keep-workspace`, `--plan-model`, `--build-model`, `--max-turns 150`,
`--minutes 60`, `--repairs 3`, `--claude <path>`.

## One trial, step by step

1. **Workspace.** A fresh temp folder under git, holding only:
   `SPEC.md` (spec plus the stack's binding), `guidance/`, the starter page
   in `web/`, `run-stack.sh` (the same `up.sh` the suite uses), and for Ordo
   the two SDK packages as tarballs in `.bench/pkgs/` (`npm install` unpacks
   them into `node_modules`; no source folder). Commit "starter".
2. **Plan.** Fable, with tools Read, Glob, Grep and Write only. Prompt:
   `runner/prompts/plan.md`. It writes `PLAN.md`.
3. **Build.** Sonnet, prompt `runner/prompts/build.md`. Tools: Bash, Read,
   Write, Edit, MultiEdit, Glob, Grep, TodoWrite, Task. WebFetch and
   WebSearch are refused, so the only docs are the pinned ones.
4. **Score.** The runner stops anything the agent left running, starts the
   app with the bench's own pristine `up.sh`, and runs the suite.
5. **Repair.** While it fails: up to 3 rounds. Each resumes the build
   session and is told only the failing test names (`prompts/repair.md`).
   A stack that will not start is fed back as one name:
   `the app did not start with run-stack.sh`.

**Caps** cover all agent phases of a trial together: 150 turns and 60
minutes. Each call gets the turns left (`--max-turns`) and is stopped at the
deadline. A cap hit is a fail, and its tokens count.

## What the agent sees: isolation

Each trial should see only its workspace, and the same Claude Code on both
stacks. `--bare` would do most of it, but it takes only an API key, and the
runs use the owner's plan. So each piece is switched off on its own
(`runner/lib/agent.mjs`), identically for both stacks.

**Where it runs.** The workspace is a fresh folder in the system temp
folder. The runner refuses a `--work` folder with a `.git`, `.claude`,
`CLAUDE.md` or `CLAUDE.local.md` anywhere above it, so no repository and no
home folder (`~/.claude`) is in reach of Claude Code's upward search.

**Flags on every `claude -p`:**

| Flag | Keeps out |
|---|---|
| `--setting-sources project,local` | the owner's user settings: hooks, permissions, plugins, env. The workspace has no settings of its own |
| `--safe-mode` | CLAUDE.md files, skills, installed plugins, hooks, MCP servers, custom commands and agents, output styles, workflows |
| `--disable-slash-commands` | every skill and slash command, built-in ones too |
| `--strict-mcp-config --mcp-config '{"mcpServers":{}}'` | every MCP server but the empty list |
| `--no-chrome` | the browser extension |
| `--permission-mode dontAsk`, `--allowedTools`, `--disallowedTools WebFetch WebSearch` | any tool not listed above |

**Environment.** Set: `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`,
`CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`, `ENABLE_CLAUDEAI_MCP_SERVERS=false`
(the connectors on the owner's claude.ai account),
`CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`. Dropped: `CLAUDECODE` and
every `CLAUDE_CODE_*` the runner inherited (a runner started from inside a
Claude Code session carries that session's), `ANTHROPIC_API_KEY` and
`ANTHROPIC_AUTH_TOKEN` (they would bill the API instead of the plan).
`CLAUDE_CONFIG_DIR` is kept: it says where the plan's sign-in lives.

**Recorded.** Each phase in `record.json` carries `command`: the argv, the
folder, the variables set and the names dropped (never values), and `line`,
one shell line that reproduces it. A dry run prints that line for each
phase and records it with `ran: false`, so the command can be checked
without spending anything.

**macOS temp and cache folders are open** (2026-10-30). The first real
trial showed git and other developer tools failing: they write a cache into
the per-user temp and cache folders (`getconf DARWIN_USER_TEMP_DIR`,
`DARWIN_USER_CACHE_DIR`) whatever `TMPDIR` says. Both are now readable and
writable, so an agent could read other programs' temp files there. The home
folder, `~/.ssh`, `~/.claude` and other repositories stay closed. That first
trial ran before this change and is kept as a pilot, not scored with the rest.

**What still leaks, or may:**

- **Not yet checked live.** The flags are the ones `claude --help` lists
  (2.1.312), and the variables are ones the binary names. No real trial has
  run. The first one's `init` event lists the tools, MCP servers, slash
  commands and plugins it loaded: check it shows none of the owner's.
- **Built in, the same on both stacks.** Claude Code's own system prompt,
  its built-in tools and subagents, and built-in plugins (safe mode keeps
  those). The system prompt names the workspace path, the OS and the date.
- **Policy.** Admin-managed (managed policy) settings still apply, if the
  machine has any.
- **The account.** The plan's sign-in is the owner's. Its claude.ai
  connectors are off; whether the account adds anything else to the prompt
  shows in the first `init` event.
- **The workspace's own settings.** The runner deletes any
  `.claude/settings.json` or `settings.local.json` in the workspace before
  each phase and records it under the phase's `sandbox.tampering`. The
  sandbox file is passed with `--settings`, which outranks them anyway.

### The sandbox

Every `claude -p` runs in Claude Code's own sandbox (macOS Seatbelt),
the same settings for both stacks. `runner/lib/sandbox.mjs` writes
`runs/<id>/sandbox-settings.json` and passes it with `--settings`. It sits
outside the workspace and scratch folder, so the agent cannot write it. The
runner checks its sha256 before each phase. Each phase's `sandbox` field in
`record.json` names the file and its hash. Keys, from the
[settings reference](https://code.claude.com/docs/en/settings-reference) and
the [sandboxing page](https://code.claude.com/docs/en/sandboxing):

| Key | Value |
|---|---|
| `sandbox.enabled`, `failIfUnavailable` | true. No silent run without it |
| `sandbox.allowUnsandboxedCommands`, `excludedCommands` | false, empty. No retry outside |
| `sandbox.filesystem.allowWrite` | the workspace and a scratch folder (`TMPDIR`, npm cache, XDG dirs) |
| `sandbox.filesystem.denyRead` | `~`, `/Users`, `/Volumes`, `/tmp`, `/private/tmp`, `/private/var/folders` |
| `sandbox.filesystem.allowRead` | the workspace, scratch, the Ordo and Postgres binaries, the Docker socket |
| `sandbox.network.allowedDomains`, `strictAllowlist` | `registry.npmjs.org`, `localhost`, `127.0.0.1`. Anything else is refused |
| `sandbox.network.allowLocalBinding` | true: dev servers and the stack's ports |
| `sandbox.network.allowUnixSockets` | the Docker socket (found with `docker context inspect`), for both stacks |
| `permissions.blockReadsOutsideWorkingDirectories`, `permissions.deny` | the Read, Glob, Grep, Edit and Write tools (not Bash) stay out of `~` |

The network list came from the starter's lockfiles (every `resolved` URL is
`registry.npmjs.org`) and from sampling open connections during a Supabase
dry run. The one other host seen was GitHub, the Supabase CLI's update
check; it is not needed and is refused. Image pulls are done by the Docker
daemon, outside the sandbox.

**Contained now:** reading `~/.claude`, `~/.ssh`, `~/.config`, other
repositories and the rest of the home folder; writing anywhere but the
workspace and scratch; any network beyond the list; running a command
outside the sandbox.

**Still open:**

- **The Docker socket.** Both stacks get it, so the agent can ask the
  Docker daemon for anything: a container that mounts the owner's home
  folder reads it. Equal for both, not contained.
- **Not yet proven live.** Claude Code has no command that runs one shell
  command in its sandbox, and the dry stub never starts `claude`. The first
  real trial proves it. After it, run
  `node runner/sandbox-check.mjs runs/<id>`: it lists tool results with
  `Operation not permitted`, `sandbox`, `EPERM` or `ENOTFOUND`. A denial on a
  path or host the build needed means `lib/sandbox.mjs` is too tight (likely
  places: the Supabase CLI's config under `~`, the Docker socket path, the
  sandbox's own temp folder). If the agent tried `cat ~/.ssh/...` and the
  transcript shows no denial, the sandbox was not on.
- **Read of the system folders.** `/usr`, `/opt`, `/etc` and the like stay
  readable; tools need them.
- **Playwright and browsers.** No browser download host is allowed; the
  hidden suite runs outside the sandbox and the agent has no browser.

## What is recorded

Each trial writes `runs/<id>/` and appends one line to `runs/results.jsonl`.

| File | What |
|---|---|
| `record.json` | the line in `results.jsonl`, pretty: everything below in one place |
| `manifest.json` | every file the agent was given, with its sha256 |
| `PLAN.md` | the plan, as written |
| `transcripts/<phase>.jsonl` | the full stream-json transcript of `plan`, `build`, `repair-N` |
| `transcripts/<phase>.prompt.md`, `.stderr` | the exact prompt, and stderr |
| `tests/round-N/` | `results.json` (Playwright), `suite.log`, failure traces, `stack.log` and the stack's own logs |
| `workspace.bundle` | the workspace's git history, one commit per phase. `git clone workspace.bundle` gives it back |

`record.json` holds:

- `versions`: Claude Code, Node, npm, Playwright, the repo commit and
  whether it was dirty, the OS; for Ordo the sha256 of both binaries; for
  Supabase the CLI and Docker versions.
- `inputs`: the spec's sha256 and each guidance file's.
- `phases`: per phase the model, start and end times, turns, exit, whether
  it hit a cap, the session id, and usage (below).
- `rounds`: per round the pass count, the @perm count, the failing names.
- `pass`, `tests`, `perm`, `turns`, `repair_rounds`, `cap_hit`,
  `tokens` (4 counts and their total), `tokens_after_first_failure` (the
  repair phases), and `cost_usd_list_price` (Claude Code's estimate).

## Token usage: the fields, and subagents

Checked against Claude Code's docs (headless, CLI reference, Agent SDK cost
tracking), 2026-10-29, Claude Code 2.1.312. Not yet against a live run,
since no real trial has run.

- **The final `result` event** has `usage`, `modelUsage`, `total_cost_usd`,
  `num_turns`, `duration_ms`, `duration_api_ms`, `session_id`, `subtype`
  (`success`, `error_max_turns`, `error_during_execution`, and others).
- **`usage`** is snake_case: `input_tokens`, `output_tokens`,
  `cache_read_input_tokens`, `cache_creation_input_tokens`. **It covers the
  main loop only, without subagents.**
- **`modelUsage`** is per model, camelCase: `inputTokens`, `outputTokens`,
  `cacheReadInputTokens`, `cacheCreationInputTokens`, `costUSD`. **It
  includes subagents.** So does `total_cost_usd`.
- **`assistant` events** carry `message.usage` and `message.id`. One API
  response can arrive as several events with the same id, so a sum must
  dedupe by id. Per-event `output_tokens` can be a placeholder. Subagent
  messages carry `parent_tool_use_id`.

**What the bench counts:** the sum over `modelUsage`, so subagents count.
It also keeps the deduplicated sum of `assistant` events, main thread and
subagents apart, as a cross-check. If a phase is stopped before its
`result` event (the time cap), that sum is used and the record says so.
`runner/test/usage.test.mjs` checks the parsing.

`--max-turns` is documented but not listed by `claude --help` in 2.1.312.

## Scoring (plan §5)

`node runner/score.mjs` prints, per stack and model pair:

- pass rate, with a bootstrap 95% interval; permission probe pass rate on
  its own (the tests tagged `@perm`);
- **tokens per passing app**: every token spent in the cell, failed trials
  included, divided by the trials that passed, with a 95% interval;
- output tokens per passing app; median tokens per trial, the 4 counts apart;
- the share spent after the first failed round;
- list-price dollars per passing app;
- pass@1, pass@5, median turns, repair rounds, wall time, cap hits.

It ends with plan §1's win test. List-price cost decides it (owner,
2026-10-30); the same test on raw tokens is printed beside it. Ordo wins
only at half the best baseline's cost per passing app or less, intervals
apart, with a permission pass rate no lower. Dry runs are left out unless
`--include-dry` is given.

## The suite

26 tests (`suite/tests/`): 11 through the page, 15 through the backend, 11
of those the `@perm` probes. Each stack's adapter (`suite/adapters/`)
turns a neutral call ("the member removes the owner") into that stack's
API. Every refused write is checked twice: the call must fail, and the
owner's read-back must show nothing changed. A refusal by row level
security, which changes nothing and raises no error, counts as refused.

Run it by hand against a running stack:

```sh
bash stacks/ordo/up.sh <workspace> /tmp/state       # in one terminal
cd suite && BENCH_STACK=ordo BENCH_STACK_JSON=/tmp/state/stack.json npx playwright test
```

## Checking the Supabase side

Once Docker is running:

```sh
cd bench/token && node runner/trial.mjs --stack supabase --dry
```

It should end `PASS: 26/26`. It starts local Supabase from the reference's
migration, builds the reference page, and runs the whole suite. The first
run pulls Supabase's images, so it is slow.

The runner refuses to start a Supabase trial while `docker info` fails, and
an Ordo trial without Postgres binaries, so a machine fault is never fed to
the agent as a failing test.

## Where the two stacks differ, and what we did

- **Docs size.** Ordo's guidance is one file, `docs/ai-guide.md`, its
  general guide for AI agents: about 23 KB. Supabase's 5 AI prompts are
  about 25 KB. Until 2026-10-29 Ordo's was 5 docs, about 240 KB.
  `SOURCE.md`, the bench's note on where Supabase's files came from, is not
  given to the agent.
- **What the agent may configure.** On Ordo, `backend/deploy.env` holds the
  app's choices (who gets a team, which ops ask for no passkey touch). On
  Supabase the app lives in SQL; the bench's own `config.toml` sets the
  machine and the agent's is not used. Neither may change ports, mail or
  the sign-in method.
- **The API the probes call.** Each binding names the ops or tables the
  probes need. These differ in form by nature: Ordo ops by name, Supabase
  tables with row level security and 2 functions.
- **"Nobody changes a role."** On Supabase this is a missing policy. On
  Ordo the owner may call `std.space.setRole`, and it is refused because it
  asks for a passkey touch an emailed-code person cannot give. The probe
  passes on both; the reason differs.
- **The machine.** Trials run on the host, not in a container per trial
  (plan §4). The agent has Bash. Run batches on a machine with nothing
  else on ports 4710, 4725, 4726, 5310, 55510 or 54321 to 54324: the
  runner stops whatever holds them before each scoring round.
- **The owner's own Claude Code setup.** Kept out by flags and variables,
  the same on both stacks. See "What the agent sees: isolation".
