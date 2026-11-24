# Plan: test Ordo through its example apps

- **Goal.** Prove Ordo's guarantees through its example apps, as close to 100% as can be reached, and measure what those tests reach.
- **Rulings.** `docs/corpus-rulings.md`, "2026-11-23: test through the example apps": the goal, and asks 1 to 5 of board task `close-the-coverage-gap-through-the`.
- **Input.** `docs/audit-feature-tests-2026-11-23.md` (413 guarantees graded E, E*, I, U, N). "Audit line N" below means the row on line N of that file.
- **How to use it.** Take the increments in order, one pull request each. Each says what to build, the test steps, how CI runs them, and when it is done.
- **Not decided here.** Anything not in the rulings goes to "Open questions for the owner" at the end. Do not settle it yourself.

## Ground rules for the implementing agent

### Read before you write

1. `AGENTS.md`: the rules, and "Sizing work".
2. `docs/corpus-rulings.md`: the last 2 sections only.
3. `docs/audit-feature-tests-2026-11-23.md`: "What the audit found", "The 16 guarantees with no test", and the rows your increment names.
4. `docs/howto-e2e.md`: how the notes and federation stacks run. "Add a use case" is the house style for tests.
5. `.claude/skills/testing/SKILL.md`: what to run locally and what CI runs.
6. The example's `README.md`, its `.ordo` file, its `dev/serve.sh`, and `examples/notes/e2e/stack.sh` as the model stack.

### Running one example's tests on your machine

- Build only what the stack starts: `cargo build -p ordo-host -p ordo-cli` (add `-p ordo-identity-service` for the federation stack).
- Notes: `E2E_SKIP_BUILD=1 bash examples/notes/e2e/run.sh -g "<test name>"`.
- Federation: `cd examples/note-viewer/e2e && npx playwright test -g "<test name>"`.
- A new example suite: `cd examples/<example>/e2e && npm ci && npx playwright test -g "<test name>"`.
- Keep a stack up in one terminal (`bash stack.sh`) and run tests in another, as `docs/howto-e2e.md` says.
- Postgres 15 or later binaries are needed. `ORDO_PGBIN` names their directory.

### What you never do

- Never run the full cargo suite locally (`cargo test --workspace`, `cargo nextest run --workspace`). CI does that.
- You may run one test file of a crate you changed: `cargo test -p <crate> --test <file>`.
- Never start cargo while another cargo build or suite runs on the machine.
- Never touch the board (`scripts/board.py`, `docs/board*.jsonl`) or `docs/roadmap-checklist.md`. The owner's Claude session updates both after your PR.
- Never decide an open question. Build what is decided, and list the question.
- Never weaken a product refusal to make a test pass. If the product is wrong, the test stays red and the PR says so.

### How tests are written

- A test name says the use case in plain words: `a visitor with no token cannot open a live stream`.
- Each test signs up its own people and uses `unique()` names. Tests never share data.
- Find elements the way a person does: `getByRole`, `getByLabel`, visible text. A missing label is a finding about the page; add the label.
- No sleeps to wait for something to happen. Use `expect(...)` with its timeout.
- **Every negative guarantee tries the forbidden thing and sees it refused**, with the refusal class and status checked.
- **Every behaviour guarantee observes the behaviour**, not a proxy for it.
- To prove something did **not** arrive on a live feed, use a sentinel: do the forbidden-to-arrive write, then a write that must arrive on the same feed. When the sentinel shows, assert the first never did. This is deterministic.
- To prove "with no reload", set `window.__e2eMarker = crypto.randomUUID()` before the act and assert the same value after it.
- Page code: AGENTS.md asks web work to consult the `modern-web-guidance` skill first. If your tool cannot load it, follow the example's existing patterns and say so in the PR body.
- Copy in docs and READMEs: short sentences, numbers as digits, no em dashes.

### One pull request per increment

- Branch from `bleeding`: `tests/<increment id>-<short name>`, for example `tests/s2-public-never-leaks`.
- Open the PR into `bleeding`. Never into `main`.
- Update the example's `README.md` "tests" section and `docs/howto-e2e.md` in the same PR when you add a suite or a stack setting.

### What "done" means

1. Every workflow the PR touches is green on CI.
2. A coverage run on your branch, from increment 0 on: `gh workflow run coverage.yml --ref <branch>`. Its numbers go in the PR body: the unit number, the examples number, and every crate that moved.
3. `coverage/floor.json` raised to the new numbers in the same PR, where they rose.
4. The PR body lists each guarantee that moved, as: audit line, the guarantee's first words, old level, new level, test file and test name.
5. The PR body ends with "Questions for the owner", even if empty.

## Increment 0: coverage of the example tests, and the ratchet

**Goal.** Every e2e stack can run on instrumented binaries. The examples number is reported beside the unit number. A per-crate ratchet runs on pull requests into `main` and by hand. Rulings: asks 1 and 2.

**Guarantees covered.** None directly. Every later increment shows its gain through this one.

**Files.**

- `scripts/e2e-coverage.sh` (new). Two commands:
  - `env`: prints the exports from `cargo llvm-cov show-env --export-prefix`. A workflow sources it before `cargo build`.
  - `report <suite>`: runs `cargo llvm-cov report --lcov --output-path coverage-out/lcov-<suite>.info` with the same environment, in the same job that built the binaries.
- `scripts/coverage-crates.mjs` (new, Node, no dependencies):
  - Reads 1 or more lcov files. Merges them by file and line (a line counts as hit if any file hit it).
  - Groups lines by `crates/<name>/src/`. Prints a table: crate, lines, hit, percent.
  - `--label unit|examples --out coverage-out/<label>.json` writes the per-crate numbers.
  - `--check coverage/floor.json` fails when a crate is below its floor by more than the tolerance (0.1 points until the owner rules; see the questions). It prints `raise <crate> <label> to <pct>` for each crate above its floor.
- `coverage/floor.json` (new): `{ "tolerance": 0.1, "unit": { "<crate>": <pct> }, "examples": { "<crate>": <pct> } }`. First filled from the first green run.
- Every e2e `stack.sh` (`examples/notes/e2e/stack.sh`, `examples/note-viewer/e2e/stack.sh`, and each new one): `stop()` sends SIGTERM to each `ordo-host` and then `wait`s for it. LLVM writes the `.profraw` only when the process exits normally. `ordo-host` already ends cleanly on SIGTERM (`crates/ordo-host/src/runtime.rs`, the `SignalKind::terminate` handler). Check that a `.profraw` file appears per host process.
- Each stack honours `ORDO` and `ORDO_HOST_BIN` already. In coverage mode point them at the instrumented binaries. Check where `cargo llvm-cov show-env` makes the build write them.
- `.github/workflows/e2e.yml`, `federation-e2e.yml`, and the new `examples-e2e.yml` (increment S1 makes it): add a `coverage` boolean input to `workflow_dispatch` and `workflow_call`. When true:
  - force `REACHED=true` (skip `scripts/ci-affected.mjs`);
  - install `llvm-tools-preview` and `cargo-llvm-cov` as `coverage.yml` does;
  - source `scripts/e2e-coverage.sh env` before `cargo build`;
  - after the tests, run `scripts/e2e-coverage.sh report <suite>` even when a test failed;
  - upload `coverage-out/lcov-<suite>.info` as artifact `lcov-<suite>`;
  - skip the screenshots step.
- `.github/workflows/coverage.yml`:
  - Triggers: `pull_request: branches: [main]` and `workflow_dispatch`. Nothing else (ask 2).
  - Job `unit`: today's job. It also writes `coverage-out/lcov-unit.info` and uploads it.
  - Jobs `notes`, `federation`, `examples`: call the 3 e2e workflows with `coverage: true`.
  - Job `ratchet`: needs all of them. Downloads every `lcov-*`. Runs `coverage-crates.mjs` twice (`--label unit` on the unit file, `--label examples` on the rest), then `--check coverage/floor.json`. Writes both tables to `$GITHUB_STEP_SUMMARY`, with one line on top: `Unit X%. Examples Y%.`
  - Do not use `continue-on-error` on the ratchet step. It is the check.

**Test steps.** None in a browser. The checks on the tooling:

1. Run the notes suite instrumented on CI. Expect `lcov-notes.info` with hits in `crates/ordo-host/src/serve.rs`.
2. Lower one floor in a scratch commit by hand to above the measured value. Expect the ratchet job to fail and name the crate. Revert.
3. Raise a floor by 0.05. Expect the job to pass (inside tolerance).

**CI.** Manual dispatch on the branch while building. After merge, it runs on every PR into `main`.

**Acceptance.** A dispatched run on `bleeding` is green and its summary shows both numbers and the per-crate tables. `coverage/floor.json` holds those numbers.

**Size.** L. 2 to 3 sessions. The pace is CI: each coverage run is about 35 minutes, and wiring 3 workflows takes 3 to 5 runs.

## System guarantees

These come before the per-example suites because they are the owner's named examples and the audit's weakest system rows.

### S1: tenant isolation across every path

**Goal.** One tenant's data is never reachable from another, on every path the examples have: op, refinement, feed, file, export, search, device copy, a write naming another tenant's row, and the space header.

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 105 | U | One tenant's data is never visible to another, through any op, feed, file, export, search or device copy |
| 106 | I | A write in one tenant can never touch, or point at, another tenant's rows |
| 107 | I | Every guard holds on every read path: an op, a refinement, a file, a person's own download |
| 109 | I | Nothing the app developer did not declare is reachable, and a forbidden call looks exactly like one to an op that does not exist |
| 822 | U | Signing out or switching person erases what the device held, so the next person never sees the first person's rows |
| 379 | U | A device copy never leaves rows after the person signs out or the access is revoked (purge, poison) |

**Files.**

- `examples/notes/e2e/tests/v2/isolation.spec.ts` (new): the notes v2 paths (feeds, files, export, search, device copy).
- `examples/notes/e2e/tests/support.ts`: add `liveEvents(page)` (collects the events a page's open feed receives, through the SDK's debug hook or by wrapping `fetch` for `?live=1` responses) and `idbRows(page, store)` (reads the SDK's IndexedDB rows).
- Teamspace e2e scaffold (new, used again by X3):
  - `examples/teamspace/e2e/{package.json,playwright.config.ts,tsconfig.json,stack.sh}` modelled on `examples/note-viewer/e2e/`.
  - `stack.sh` boots one `ordo-host` with the settings in `examples/teamspace/dev/serve.sh` plus `ORDO_AUTH_METHODS=passkey,email` and the mail catcher from `examples/notes/e2e/stand-ins.mjs`, applies `teamspace.ordo` with `ordo apply`, builds `examples/teamspace/web` with Vite and serves it with `vite preview`. Projects `api` and `browser` (Chromium, virtual passkey).
  - `examples/teamspace/e2e/tests/api/isolation.spec.ts` (new).
- `examples/e2e-common/stack-lib.sh` (new): `start_pg`, `wait_for`, `start_door`, `stop` (SIGTERM then `wait`), lifted from the notes stack. New stacks source it. Do not refactor the notes or note-viewer stacks in this PR.
- `.github/workflows/examples-e2e.yml` (new): one job per example with a stack (`teamspace e2e` now; later `marketing-site e2e`, `shopfront e2e`, `first-app e2e`, `assistant e2e`). Same triggers as `e2e.yml`. Add a flag per example to `scripts/ci-affected.mjs` (an app change under `examples/<name>/`, or a build of `ordo-host` or `ordo-cli`).

**Test steps.** In notes v2 (`isolation.spec.ts`). Ada and Bo each sign up; each gets a tenant of their own.

1. `a note id from another tenant is not found, exactly as an id that does not exist`
   - Ada writes a note. Bo calls `oneNote` with Ada's note id.
   - Expect the same status and the same body (bytes, after removing the request id) as Bo calling `oneNote` with a made-up id of the right prefix.
2. `a refinement cannot reach another tenant's rows`
   - Bo calls `myNotes` with a `$refine` filtering on Ada's note id and on Ada's title.
   - Expect zero rows, not a refusal that admits the row exists.
3. `a write in one tenant never shows on another tenant's open feed`
   - Ada's page is open on her list; `liveEvents` is recording.
   - Bo writes a note titled `bo-<unique>`. Then Ada writes `ada-sentinel-<unique>` in her own tenant.
   - Expect the sentinel in Ada's list with no reload. Expect no event and no list row carrying `bo-<unique>`.
4. `another tenant's file is not found, exactly as a file that does not exist`
   - Ada attaches a file. Bo requests its download handle with his token, and with no token.
   - Expect the same answer as for a handle that does not exist.
5. `a person's own download holds only their tenant's rows`
   - Ada and Bo each write 2 notes. Bo downloads `allNotes` and `myFiles` (the `download` ops).
   - Expect Bo's file to hold his 2 titles and none of Ada's.
6. `only the owner exports a tenant, and another tenant's export is refused`
   - Bo calls the tenant export op naming Ada's space. Expect it refused, with the same answer as an unknown space.
7. `search never finds a note from another tenant`
   - Ada writes a note with the word `zebra-<unique>`. Bo searches it. Expect no result.
8. `a write cannot point at another tenant's row`
   - Bo calls `addAttachment` with `$note` set to Ada's note id. Expect a refusal (`cross_space_ref` or `not_found`; record which in the PR). Expect Ada's note to have no new attachment.
9. `the space header naming another tenant is refused like an unknown op`
   - Bo calls `myNotes` with `X-Ordo-Space: <Ada's space>`.
   - Expect the same status and body as Bo calling `/op/noSuchOp` with his own space.
10. `signing out erases the device copy, and the next person in the same browser sees none of it`
    - Ada signs in, writes 3 notes, waits until `idbRows` holds them.
    - Ada signs out. Expect `idbRows` empty.
    - Bo signs in, in the same browser context. Expect none of Ada's titles in the page or in IndexedDB.

In teamspace (`tests/api/isolation.spec.ts`). Two founders, Fay and Gus, each own a team.

11. `a founder cannot read another team's board, team or report`
    - Gus calls `board`, `projectTeam` and `reportByProject` with Fay's project ids, and with `X-Ordo-Space` naming Fay's team.
    - Expect zero rows for his own space, and a refusal identical to an unknown op for Fay's space.
12. `an undeclared op and a forbidden op look the same`
    - With no token, and with Gus's token, call `/op/noSuchOp` and `/op/reportByProject` in Fay's space.
    - Expect byte-identical status and body.

**CI.** Notes: the `v2` project in `e2e.yml`, as today. Teamspace: the `teamspace e2e` job in `examples-e2e.yml`.

**Acceptance.** All 12 tests green on CI. Lines 105, 106, 107 and 109 move to E. Lines 379 and 822 move to E.

**Size.** L. 2 sessions: the notes tests in one, the teamspace scaffold and tests in the other. Pace: 3 to 4 CI rounds of about 12 minutes; one coverage run.

### S2: a public page never leaks, and tokenless live is refused

**Goal.** A visitor with no token can read only what is public, from the marketing site. Every way to reach private data is tried and refused.

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 85 | I | Tenant data can never be reached from a public page: no private field, private op, tenant space, refinement, tokenless write or private file |
| 86 | N | A visitor cannot open a live stream (a tokenless ?live=1 answers not_live) (untested #2) |
| 786 | U | Public reads reject $refine (invalid_refinement) and tokenless ?live=1 (not_live) |
| 802 | N | Public requests are counted in ordo_public_requests_total by op, kind, outcome; a probe for an unknown op cannot mint series (untested #14) |
| 779 | U | A signed-in member calling a public op takes the ordinary path in their own space |

**Changes to the example.**

- `examples/marketing-site/site.ordo`: on `products`, replace the entity-level `public` line with field-level `public` marks, and add one private field: `internalNote: text = ""` with no `public` mark. Bump the domain to `0.2.1`. The public ops keep their output lists unchanged.
- `examples/marketing-site/dev/seed`: seed `internalNote` with `INTERNAL-<slug>`.
- README: one line on the private field.

**Files.**

- Marketing-site e2e scaffold (new, used again by S3 and X4): `examples/marketing-site/e2e/{package.json,playwright.config.ts,tsconfig.json,stack.sh}`. The stack boots `ordo-host` as `dev/serve.sh` does, applies `site.ordo`, runs `dev/seed` against it, and serves the built page. Projects `api` and `browser`.
- `examples/marketing-site/e2e/tests/api/public-never-leaks.spec.ts` (new).
- `examples/marketing-site/e2e/tests/browser/public-never-leaks.spec.ts` (new).
- The notes stack already holds private files: `examples/notes/e2e/tests/v2/isolation.spec.ts` gets step 8 below.

**Test steps.** All with no token unless the step says so.

1. `a visitor reads the catalogue, pricing and reviews and never a private field`
   - `GET /op/catalogue`, `/op/pricing`, `/op/reviews`, `/op/product?slug=<seeded>`.
   - Expect 200 and the seeded rows. Expect no `internalNote` key and no `INTERNAL-` text anywhere in the 4 bodies.
2. `a visitor cannot call a private op, and it looks like an op that does not exist`
   - `GET /op/enquiryList` and `GET /op/noSuchOp`. Expect the same status and body.
3. `a visitor cannot refine a public read`
   - `GET /op/catalogue` with a `$refine` on `name`, then on `internalNote`.
   - Expect `invalid_refinement` both times. Expect the second answer to be identical to the first apart from the request id, so it does not reveal that the field exists.
4. `a visitor with no token cannot open a live stream`
   - `GET /op/catalogue?live=1`. Expect the refusal class `not_live`. Expect the response to end; it is not a stream.
5. `a visitor cannot name a tenant space`
   - `GET /op/catalogue` with `X-Ordo-Space: system` and with `X-Ordo-Space: <a made-up space>`. Expect both refused alike, or both served as the plain public read; record which. A private op with a space header and no token is refused like an unknown op.
6. `a visitor's contact write creates a row and can change nothing`
   - `POST /op/contact` with valid fields. Expect `{"rows":[{"created":"<id>"}]}`.
   - `POST /op/contact` with an extra `id` param set to that id. Expect a refusal; the owner's `enquiryList` still shows one row with the first message.
   - `POST /op/addProduct` with no token. Expect refused like an unknown op.
7. `a signed-in member calling a public op gets the ordinary path`
   - The site owner, signed in, calls `catalogue` with token and `X-Ordo-Space: system`.
   - Expect no `X-Ordo-Cache` header. Expect the same rows as the visitor saw.
8. (notes, `isolation.spec.ts`) `a private file is never served without a token`
   - Ada attaches a file. Fetch its download address with no token. Expect refused, with the same answer as a made-up handle.
9. `public requests are counted, and an unknown op mints no series`
   - Scrape `/metrics` as the stack's metrics caller (see `crates/ordo-host/src/serve.rs` `/metrics` access).
   - Do one hit, one miss, one `contact`, one over the hourly cap (stack sets `ORDO_PUBLIC_WRITES_PER_HOUR=1000`; this step uses a separate door started with `=1`, or skip the limit outcome and say so), and 20 calls to `/op/probe-<random>`.
   - Expect `ordo_public_requests_total` series for `catalogue` with the right outcome labels and counts. Expect no series whose `op` label is any `probe-` name.
10. (browser) `the landing page shows the public content and never the private field`
    - Open the page. Expect the product grid, the pricing table and the testimonials.
    - Collect every response body the page received. Expect no `INTERNAL-` text.

**CI.** `marketing-site e2e` job in `examples-e2e.yml`.

**Acceptance.** Lines 85, 86 and 786 move to E. Line 802 moves to I (it is read from `/metrics`, not a page). Line 779 moves to E.

**Size.** M. 1 session, 2 CI rounds.

### S3: the public cache, proven by counting database reads, on one node and across nodes

**Goal.** A cache hit reads nothing from the database. A write on one node drops the cached answer on every node.

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 87 | I | The public call cache is used, and on one node a write, a deploy or a lost listener drops what it holds, so a visitor never sees stale data |
| 89 | N | A write on one node drops the cached public answers on every other node (untested #3) |
| 88 | I | The public cache never serves one caller's private answer to another |

**Files.**

- `examples/marketing-site/e2e/stack.sh`: start a second `ordo-host` on the same database (door B, port 4011, its own `ORDO_REPLICA_ID`), same keys and settings. Print both addresses in `stack.json`.
- `examples/marketing-site/e2e/tests/api/public-cache.spec.ts` (new).
- `examples/marketing-site/e2e/tests/reads.ts` (new): `tableReads(table)` returns `seq_scan + idx_scan` from `pg_stat_user_tables` for the entity's table, read with `psql` as the stack's admin user. It handles the stats delay: Postgres flushes a backend's counters at most once a second, and an idle backend within 10 seconds. So `tableReads` polls until 2 reads 11 seconds apart agree.

**Test steps.**

1. `a cache hit reads nothing from the database`
   - `GET /op/pricing` once on door A (a miss). Expect `X-Ordo-Cache: miss`.
   - Take `before = tableReads('plans')`.
   - `GET /op/pricing` 20 times on door A. Expect each `X-Ordo-Cache: hit` and the same body as the miss.
   - Take `after = tableReads('plans')`. Expect `after == before`.
   - Control, in the same test: `GET /op/pricing?x=<unique>` if params make a new key, or the owner's write in step 2. Expect the counter to rise after a miss, so the test can tell a count that does not move from one that cannot.
2. `a write drops the cached answer, and the next read is fresh`
   - The site owner calls `addPlan` with name `plan-<unique>` on door A.
   - `GET /op/pricing` on door A. Expect `X-Ordo-Cache: miss` and the new plan in the body.
3. `a write on one node drops the cached answer on the other node`
   - Warm `GET /op/pricing` on door B until it answers `hit`.
   - The owner calls `addPlan` on door A.
   - `GET /op/pricing` on door B. Expect, within the expect timeout, `X-Ordo-Cache: miss` and the new plan in the body.
   - Expect no answer from door B after the write's commit that is a `hit` with the old body. Poll every 100 ms and record each answer.
4. `a deploy drops every cached answer`
   - Warm `pricing` on both doors. Run `ordo apply` of `site.ordo` with a changed comment only (a new version). Expect the next read on each door to be a miss.
5. `two members never share one cached answer`
   - Two signed-in members call `catalogue` with tokens. Expect no `X-Ordo-Cache` header on either, and `tableReads('products')` to rise for each call.

**CI.** `marketing-site e2e` job.

**Acceptance.** Line 87 moves to E, with the read count as the proof the audit asked for. Line 89 moves to E. Line 88 moves to E.

**Size.** M. 1 session. The stats delay makes the test slow (about 30 seconds); that is fine.

### S4: live updates across clients and across nodes

**Goal.** A change shows with no reload on every open client: another person's, and one held by another server. A routine fires once with 2 servers. An ended session is refused on the other server.

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 95 | E | A change shows on every open client of the same data with no reload (adds: a second person, a shared note) |
| 96 | I | A change written through one server reaches a live client held by another server |
| 99 | I | A routine fires once for the deployment, however many servers run |
| 141 | I | Signed in stays signed in: across reload, server restart and upgrade (adds: a restart under an open page) |
| 98 | U | Two parts of one screen never show a write half applied |

**Files.**

- `examples/notes/e2e/stack.sh`: `E2E_SECOND_DOOR=1` starts door B on the same database (port 4641, own `ORDO_REPLICA_ID`), and builds a second copy of the v2 page pointing at it (`/v2b/`, `VITE_ORDO_URL` = door B). Add both page origins to `ORDO_AUTH_ORIGINS` and `ORDO_CORS_ORIGINS`. Add `restart_door A` as a stack command the tests can call (a small control endpoint in `stand-ins.mjs`, or a file the stack watches).
- `examples/notes/e2e/playwright.config.ts`: a project `nodes` that sets `E2E_SECOND_DOOR=1` and runs `tests/nodes/*.spec.ts`. Chromium only.
- `examples/notes/e2e/tests/nodes/live-across.spec.ts` (new).

**Test steps.**

1. `a shared note edited by its owner changes on the reader's open page with no reload`
   - Ada shares a note with Bo (as `ui/notes.spec.ts` does). Bo's page shows it. Set the no-reload marker.
   - Ada edits the title to `t2-<unique>`.
   - Expect Bo's page to show `t2-<unique>` and the marker unchanged.
2. `a write through one server shows on a page held by the other server`
   - Ada signs in on page A (door A). She signs in on page B (door B) by emailed code.
   - On page B she writes `node-b-<unique>`. Expect it on page A with no reload.
   - Reverse it: write on A, expect it on B.
3. `signing out on one server ends the session on the other`
   - Ada is signed in on pages A and B (same session token copied by the test into page B's storage, or the hand-off; record which).
   - She signs out on page A. On page B she writes a note.
   - Expect page B to show the signed-out state and the write refused. Expect no note written (check as Ada after signing in again).
   - This replaces the `#[ignore]` test `crates/ordo-host/tests/two_instances.rs:1342` in practice. Leave that file alone.
4. `a reminder fires once with two servers running`
   - Copy `upgrade/v1-to-v2.spec.ts:252` into this project. Set a reminder 5 seconds ahead.
   - Expect exactly 1 notice in the page and exactly 1 email at the mail catcher after 30 seconds.
5. `a page stays signed in when its server restarts`
   - Ada's page is open on door A. The test restarts door A.
   - Expect her list still shown, then a new note written after the restart to land, with no sign-in screen.
6. `two lists over one entity never show a write half applied`
   - Needs a page with 2 lists over `notes` (the list and the pinned strip, if notes v2 has one; otherwise add a "Pinned" strip that reads `myNotes` filtered by `pinned`).
   - Record both lists at every animation frame (`requestAnimationFrame` hook) while Ada pins a note.
   - Expect no frame where the note is pinned in one list and not in the other.

**CI.** `e2e.yml` runs the `nodes` project in its own step, after the main run, with its own stack.

**Acceptance.** Lines 96, 99 and 141 move to E. Line 98 moves to E if step 6 lands; otherwise it stays U and the PR says why.

**Size.** M. 1 to 2 sessions. The second door in the notes stack is the risky part (ports, two page builds).

### S5: taking access away reaches open feeds

**Goal.** Revoking a share, signing out everywhere, leaving a tenant and withdrawing a delegation each empty the open screen at once, with no reload, and stop the feed.

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 117 | I | Taking back a share, withdrawing a delegation, or signing out ends access everywhere: ops, open feeds, the device copy and every server |
| 812 | U | A live stream never leaks another person's rows: a revoked or refused feed empties the hook and stops |
| 120 | I | A person who leaves a tenant loses access at once and their access rows go |
| 119 | I | An agent acting for a person can call only the approved ops, never steps up, and stops at the next call when withdrawn |
| 312 | I | A person can leave a tenant themselves |

**Files.**

- `examples/notes/e2e/tests/ui/revocation.spec.ts` (new).
- `examples/teamspace/e2e/tests/browser/leave.spec.ts` (new). If the teamspace page has no "Leave team" control, add one to `MembersView.tsx` or the account menu, calling the leave op (`docs/howto-roles.md` or `crates/ordo-host/tests/member_release.rs` name it).
- `examples/assistant/e2e/tests/api/delegation-feed.spec.ts` (new). The assistant stack is made here (API only; X6 adds the page): `examples/assistant/e2e/stack.sh` boots the host as `dev/serve.sh` does, runs `dev/seed.mjs`.

**Test steps.**

1. `a reader's open page loses a note the moment the share is taken back`
   - Ada shares a note with Bo. Bo's page shows it; set the no-reload marker.
   - Ada takes the share back.
   - Expect the note gone from Bo's page, the marker unchanged, and `idbRows` holding no row with the note's id.
   - Bo calls `oneNote` with its id. Expect `not_found`.
2. `signing out everywhere ends the feed in every tab`
   - Ada has 2 tabs open on her list. In tab 1 she uses "Sign out everywhere".
   - Expect tab 2 to show the signed-out state with no reload, and its live request to have ended (the response closed).
   - Then the test writes a note as Ada from a fresh session. Expect tab 2 to show nothing new.
3. (teamspace) `a colleague who leaves the team loses the open board at once`
   - Fay invites Hal as editor (the README walkthrough). Hal opens a project board.
   - Hal leaves the team. Expect his board empty and the team gone from his picker, with no reload.
   - Hal calls `board` with the project id. Expect refused.
   - Fay's Members view no longer lists Hal. `projectTeam` holds no row for him.
4. (assistant, API) `a withdrawn delegation ends the agent's feed and its next call`
   - The manager approves a grant for `pendingExpenses` and `approveSmallExpense` (as `manager/approve.mjs` does).
   - The agent opens `pendingExpenses?live=1` with its token.
   - The manager revokes (`approve.mjs --revoke` logic).
   - Expect the agent's stream to end within one idle tick, and its next call to answer `delegation_required`.
   - Expect the agent, before the revoke, refused `approveExpense`, `rejectExpense`, a delete and `std.space.setRole` with `access_denied` (README §2 table).

**CI.** Notes in `e2e.yml` (`tests/ui`, so 3 browsers). Teamspace and assistant in `examples-e2e.yml` (`assistant e2e` job is new).

**Acceptance.** Lines 117, 812, 120, 312 move to E. Line 119 moves to E after X6 adds the page; here it is I through the real agent script.

**Size.** M. 1 to 2 sessions.

## The 16 guarantees with no test

Where each of the 16 goes:

| # | Guarantee | Where |
|---|---|---|
| 1 | A receipt can be checked by someone outside the deployment | V1, if the owner says yes to a verifier mode (question 2) |
| 2 | A visitor cannot open a live stream | S2 step 4 |
| 3 | A write on one node drops cached public answers on every other node | S3 step 3 |
| 4 | No parameter, sealed value or token in a log line, span or metric label | U1 |
| 5 | Changing a money field's places never silently changes amounts | U1 |
| 6 | Durations compare and sort only within one unit class | U3 (built, then tested) |
| 7 | A null parameter equals an omitted one | U1 |
| 8 | A write op cannot be a source or be composed | U2 |
| 9 | A tenant's own op choices survive a deploy | U1 |
| 10 | A chain proof discloses no space content | V1 |
| 11 | The identity service refuses a database URL naming another's database | C2 |
| 12 | The first-deployment path works end to end | C2 |
| 13 | A database on the wrong host or port is refused naming host:port | C2 |
| 14 | Public requests are counted; a probe mints no series | S2 step 9 |
| 15 | A tenant in its own database does not reach a peer (documented gap) | U3 |
| 16 | Nothing warns on an unindexed open filter (documented gap) | U3 |

### U1: four untested guarantees through the examples

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 151 | N | A parameter value, sealed value or token never appears in a log line, span or metric label |
| 197 | N | Changing a money field's places never silently changes amounts: migrate demands rescale or keep |
| 214 | N | A null param equals an omitted param: the default applies, so null never overrides a default |
| 294 | N | A tenant's own choice survives a deploy; an unset op follows the template; a new op arrives at its template default |

**Changes to the examples.**

- `examples/teamspace/teamspace.ordo`: `createTask` gains `$status: text = "todo"` and writes `status: $status`. The page sends nothing for it.
- `examples/teamspace/e2e/fixtures/teamspace-0.1.1.ordo` (new): `teamspace.ordo` plus one new read op, `doneTasks($project: id)`, and a template entry for it in the stack's `ORDO_ACCESS_TEMPLATE` (`doneTasks=owner,manager`).
- `examples/shopfront/e2e/fixtures/shopfront-price3.ordo` (new): `shopfront.ordo` with `price: money(3)`. The shopfront stack is made in X2; until then this test uses a minimal stack in `examples/shopfront/e2e/stack.sh` that X2 grows.

**Files.**

- `examples/notes/e2e/tests/api/no-secrets-in-logs.spec.ts` (new).
- `examples/teamspace/e2e/tests/api/defaults-and-template.spec.ts` (new).
- `examples/shopfront/e2e/tests/cli/money-places.spec.ts` (new). It drives the `ordo` binary with `child_process.execFile`.

**Test steps.**

1. (notes) `a marker value sent as a parameter, a sealed value and a token never reach the log or the metrics`
   - Make `M = "MARK-" + unique()`. Write a note whose title and body are `M`. Upload a file named `M.txt`. Send a refused call carrying `M` (a title over the length bound).
   - Read the session token `T` the page holds.
   - Scrape `/metrics`. Read the door's log file from `.stack/` (the path `stack.json` names).
   - Expect neither `M` nor `T` nor the first 16 characters of `T` in the log or the metrics.
   - If notes v2 declares a sealed field, write `M2` into it and expect it absent too. If none, say so in the PR; S1's notes do not seal.
   - Run the door with `RUST_LOG=debug` for this test (a stack variable), so spans are in the log.
2. (teamspace) `an explicit null takes the parameter's default`
   - Call `createTask` with `"status": null`. Expect the new task's `status` to be `todo` in `board`.
   - Call it with no `status` key. Expect `todo`.
   - Call it with `"status": "doing"`. Expect `doing`.
3. (teamspace) `a tenant's own op choice survives a deploy, and a new op arrives at the template default`
   - Fay (owner) opens `reportByProject` to a group holding Hal, with `std.space.setOpAccess`, as `teamspace_app.rs` `a_tenant_owner_opens_the_report_to_a_group` does.
   - Hal reads the report. Expect rows.
   - Run `ordo apply examples/teamspace/e2e/fixtures/teamspace-0.1.1.ordo --host <door>` as the deployment owner.
   - Expect Hal still reads `reportByProject` (the tenant's choice kept).
   - Expect Hal refused `reportByPerson` (never opened, still at the template).
   - Expect Hal refused `doneTasks`, and Fay served it (new op at the template default).
4. (shopfront) `changing a money field's places is refused until the developer says rescale or keep`
   - Seed a product at price `12.34`.
   - `ordo plan shopfront-price3.ordo --host <door>`. Expect exit 1 and the output to name `products.price` and the `--places` flag.
   - `ordo apply` with the same file. Expect refused; `catalogue` still answers `12.34`.
   - `ordo apply ... --no-activate`, `ordo evolve --from ... --to ... --places products.price=rescale --run`, then `ordo apply`. Expect `catalogue` to answer `12.340`.
   - In a fresh stack, `--places products.price=keep`. Expect the stored number unchanged, read back as `docs/howto-values.md` says `keep` reads. Record the exact value in the test.

**CI.** Notes in `e2e.yml` (`api` project). Teamspace and shopfront in `examples-e2e.yml`.

**Acceptance.** Lines 151, 197, 214, 294 move from N to E (or I for steps 1 and 4, which read the log and drive the CLI; say which in the PR).

**Size.** M. 1 to 2 sessions.

### U2: a write op cannot be a source

**Guarantee covered.** Audit line 247, N: *A write op can't be a source for a read or composed into another op.*

**Why not an example.** It is a compile refusal. Examples hold programs that compile. The ruling covers definition files by compile tests.

**Files.**

- `crates/ordo-cli/tests/fixtures/refusals/write-as-source.ordo` and `write-composed.ordo` (new): first-app's `readings` plus a read op that reads from `addReading`, and one that composes it.
- `crates/ordo-cli/tests/check_exit.rs`: 2 tests that run the `ordo check` binary on each file.

**Test steps.**

1. `ordo check refuses a read that uses a write op as its source`. Expect exit 1, and the refusal class and the op's name in the output, with the file and line.
2. `ordo check refuses a write op composed into another op`. Same expectations.

**CI.** `ci.yml`'s test job. Run locally with `cargo test -p ordo-cli --test check_exit`.

**Acceptance.** Line 247 moves from N to I.

**Size.** S.

### U3: the 3 guarantees first left out

The owner, 2026-11-23, ask 10: *"No. That's the whole point of testing it."* The plan first left these 3 out as 2 documented gaps and 1 unbuilt rule. Each now gets a test through an example, and the product work it needs.

**#15, a tenant in its own database reaching a peer.** `definition-language.md` "Known gap": the peer is in the shared database, so a tenant kept in a database of its own does not reach it.

- Today's behaviour is unknown: a wrong answer, an empty page or a raw database error. First, write the test and see.
- The guarantee to test: that tenant's op on a peer table is refused with a named class that says why and what to change. It never returns an empty or wrong answer.
- Example: `examples/note-viewer` with a second tenant moved to its own database (the per-tenant database path S1 uses). Click the shared-notes view; expect the named refusal on the page, and the shared-database tenant still sees its notes.
- If closing the gap (reaching the peer from a tenant database) is wanted instead of a refusal, that is a question for the board before building.
- Size: M, 1 to 2 CI rounds.

**#16, a slow filter on an unindexed open field.** `howto-cost.md` §7: nothing warns today.

- Product work: the host notes when a filter on an open field with no index scanned more than a set number of rows (a setting, default 10,000), and reports it once per op and field: a line in `ordo dev`, a metric with the op and field as labels (no values), and the console's ops page.
- Example: teamspace, seeded with 20,000 tasks. Filter by an unindexed open field; expect the warning in the `ordo dev` output and on the console, naming the op, the field and the index to add. Add the index; filter again; expect no warning.
- Size: M, 2 CI rounds.

**#6, durations compare and sort only within one unit class.** `definition-language.md` "Duration semantics": bare `duration` (the calendar class), `incomparable_duration` and `duration_unit_class` are settled and not built.

- Product work: the two-component duration model the doc describes, replacing the engine's duration type (`audit-doc-contradictions.md` F4). This is the larger part.
- Example: first-app gains a reminder with `every: duration` (calendar) and a timer with `duration(ms)`. Through the page: set "every 1 month" and see the next date land on the same day next month; sort by the timer. Compile tests: comparing the 2 classes is refused with `incomparable_duration`, and a months value into an exact field with `duration_unit_class`.
- Size: L, 2 to 3 sessions with a review, 3 CI rounds. It may move to its own plan once started.

**Acceptance.** All 3 audit lines move from N to E.

## Per-example click-and-expect suites

Ask 4: every example with a page gets tests that do something and see the result. Each suite below lists its tests as action, then expected result. Shared steps from S1 to U1 are not repeated.

### X1: notes v2

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 373 | U | A page draws from what it has at once, says whether it is complete, and never pretends to be done |
| 381 | U | Tabs share one set of feeds with one leader and the others follow |
| 384 | U | No polling: the SDK never reloads on focus and runs no timer |
| 468 | U | SDK and React file helpers (progress, release, abort, error classes) |
| 821 | U | The device copy is kept current from the server feed and survives a restart |
| 523 | U | A forged, foreign or stale OIDC ID token signs nobody in |
| 134 | I | No write is lost or applied twice (adds: an idempotent retry through the page) |

**Files.** `examples/notes/e2e/tests/v2/page.spec.ts` (new). `examples/note-viewer/e2e/tests/api/oidc-forged.spec.ts` (new). `examples/note-viewer/e2e/tls-front.mjs` or a new `bad-issuer.mjs`: a stand-in issuer that can sign ID tokens with a wrong `aud`, a wrong `nonce`, an expired `exp`, `alg: none` and `HS256`.

**Test steps.**

1. `a person pins, archives and unarchives a note, and each shows at once`
   - Pin a note. Expect it at the top with a pinned mark. Archive it. Expect it gone from the list and shown under Archived. Unarchive. Expect it back.
2. `a reminder can be cleared before it fires`
   - Set a reminder 10 seconds ahead. Clear it. Wait 20 seconds. Expect no notice and no email.
3. `the list says it is still loading until the first answer, and never shows an empty state before it`
   - Delay the `myNotes` response by 2 seconds with `page.route`. Expect the loading hint, then the notes. Expect the empty-state text never shown while the hint is.
4. `three tabs share one live feed`
   - Open 3 tabs of one person. Count open `?live=1` requests to the door across the context.
   - Expect exactly 1. Close the leader tab. Expect 1 again within 5 seconds, from another tab, and a write still reaching both.
5. `an idle page makes no requests, and focus does not reload`
   - Open the list. Record requests to the door for 30 seconds while idle, then blur and focus the page.
   - Expect no request apart from the one open live stream.
6. `an upload shows progress and can be cancelled, leaving no attachment`
   - Throttle the network (CDP `Network.emulateNetworkConditions`). Upload a 5 MB file. Expect a progress value between 1 and 99. Press Cancel.
   - Expect no attachment on the note after a reload, and no orphan: `myFiles` lists none.
7. `the device copy survives a reload with the network off`
   - Load the list. Go offline. Reload. Expect the notes shown from the device.
   - Go online. Write from another tab. Expect it shown.
8. `a write sent twice with one idempotency key lands once`
   - Intercept the first `createNote` POST, let it reach the door, then abort the response (`route.fetch()` then `route.abort()`). The SDK retries.
   - Expect exactly 1 note with that title.
9. (federation) `a forged or stale ID token signs nobody in`
   - For each bad token kind: drive the callback with it. Expect the sign-in refused, a refusal class named, and no session.
   - Rotate the stand-in issuer's key. Expect a good token under the new key to sign in, with no door restart.

**CI.** Steps 1 to 8: `v2` project in `e2e.yml`. Step 9: `api` project in `federation-e2e.yml`.

**Acceptance.** Lines 373, 381, 384, 468, 821, 523 move to E. Line 134 moves to E.

**Size.** M to L. 2 sessions: steps 1 to 8, then the bad issuer.

### X2: shopfront

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 244 | I | A multi-statement op lands whole or not at all |
| 229 | I | A write that violates check aborts the whole group, names the check, and leaves nothing changed |
| 835 | U | Forms derived from op params validate in wire types (money string, bounds, id prefix, enum) |
| 385 | U | Long lists page: more() appends, a refinement rides every page, stale cursors restart cleanly |
| 833 | U | A listing ends only when `next` is absent |
| 832 | U | Refusal classes map to page behaviour (access_denied, not_found, access_guard, ...) |
| 176 | U | Every id says what it is (prefix from the entity), is server-minted |

The shopfront README §8.5 says the pages were never driven in a browser. This suite closes that.

**Changes to the example.**

- Admin page, Products tab: the "Add product" form uses `useOpForm('addProduct')` from `@hominemai/ordo-react` in place of the hand-written form.
- Shop page: the catalogue pages, 20 at a time, with a "More" button (`more()`). The seed has 45 products. Read `crates/ordo-host/tests/refine_paging.rs` and `clients/ordo-react` for how a list pages; if `catalogue` must change (`take`, or a paged op), change it and say so in the README.
- `examples/shopfront/README.md` §7 and §8: the new tests; delete gap 5.

**Files.**

- `examples/shopfront/e2e/{package.json,playwright.config.ts,tsconfig.json,stack.sh}` (new). The stack does what `dev/serve.sh` does: boots the host with the 3 page origins (5180 login, 5181 shop, 5182 admin), registers the seed owner with `dev/passkey.mjs`, restarts with `ORDO_DEPLOYMENT_OWNERS`, runs `seed.mjs`, builds the web app once and serves it on the 3 ports.
- `examples/shopfront/e2e/tests/browser/shop.spec.ts`, `admin.spec.ts` (new).

**Test steps.**

1. `a customer signs in through the login page and lands back on the shop, signed in`
   - Open the shop. Press **Sign in**. Expect the login page's origin.
   - Sign up with a passkey. Expect the shop's origin and the person's name at the top.
   - Expect no token in the URL at any point (record every navigation).
2. `the colour filter shows only products of that colour`
   - Choose `red`. Expect every shown product's colour to be red, and the count to equal the seed's red count.
3. `the catalogue pages with More, and a filter rides every page`
   - Expect 20 products. Press More. Expect 40, no repeats. Press More. Expect 45 and no More button.
   - Choose `red`, press More until it goes. Expect only red products on every page.
4. `a customer places an order and sees it waiting to ship, with the total at the catalogue price`
   - Order 3 of a product priced `12.50`. Expect a row under My orders: quantity 3, total `37.50`, not shipped.
   - Expect the order id in the page (or the network answer) to start with the `orders` prefix.
5. `a customer is turned away from the admin page`
   - Open the admin origin. Expect the "you hold no level on the shop" message and no orders.
6. `a customer cannot call a staff op, and the page says so in words`
   - From the customer's page context, call `markShipped` on their own order and `restock` on a product (through the SDK in `page.evaluate`).
   - Expect `access_guard` for the readable product and `not_found` for an order of another customer. Expect the README's refusal words, not a raw class.
7. `staff ship an order, and the customer sees it shipped with no reload`
   - The customer's My orders is open; set the no-reload marker. A staff member (made with `seed.mjs make-staff`) ships it in the admin page.
   - Expect the customer's row to show shipped and the carrier, marker unchanged. Expect stock down by 3.
8. `shipping twice is refused, and the stock is taken once`
   - Staff press Ship again on the same order. Expect a refusal shown. Expect stock unchanged by the second press.
9. `shipping more than is in stock is refused, and no shipment is recorded`
   - Restock a product to 2. A customer orders 5. Staff ship it.
   - Expect the refusal to name the check `stock >= 0`. Expect the order still not shipped, and `allOrders` showing no shipment for it. This is the failed second write undoing the first.
10. `the add-product form refuses bad input before sending, in the door's own words`
    - Price `12.345`. Expect a money error naming 2 places, and no request sent.
    - Price `abc`. Expect a money error.
    - Stock `-1` then Save. Expect the `stock >= 0` refusal from the door.
    - A good product. Expect it in the catalogue on the shop page with no reload.
11. `editing a product with a stale revision is refused, and the editor keeps their text`
    - 2 staff tabs open the same product. Tab 1 saves a new name. Tab 2 saves another.
    - Expect tab 2 refused, its typed name still in the field.
12. `a customer never sees another customer's orders`
    - 2 customers each order. Expect each My orders to show only their own. Expect `allOrders` called by a customer to return only their own.

**CI.** `shopfront e2e` job in `examples-e2e.yml`.

**Acceptance.** Lines 244, 229, 835, 385, 833, 832, 176 move to E.

**Size.** L. 2 to 3 sessions: the stack and 3 origins, then the tests, then the form and paging changes.

### X3: teamspace

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 184 | U | The same op returns text in codepoint order (Z before a) on every deployment |
| 213 | U | Nulls sort last whichever way the sort goes |
| 292 | U | The app developer's template sets the default per op, and a bad template refuses boot |
| 829 | U | A CAS op sent without rev is refused before the wire (missing_param) |

**Changes to the example.**

- `teamspace.ordo`: add `op dueSoon($project: id) unbounded = tasks(project == $project)[id, title, due] sort due`, and a "Due soon" list in `BoardView.tsx`.

**Files.** `examples/teamspace/e2e/tests/browser/team.spec.ts`, `tests/api/order-and-template.spec.ts` (new).

**Test steps.**

1. `a founder signs up, makes a project, adds tasks and moves them across the board`
   - Sign up with a passkey. Expect to be the team's owner (Members shows `owner`).
   - Create a project, add yourself to its team, add 3 tasks. Expect 3 cards in To do.
   - Move one to Doing. Expect it in Doing after a reload.
2. `an invited colleague joins as editor and sees only their projects`
   - Invite as editor. Expect a link of the form `#/join/<space>/<ticket>` and a passkey touch asked first.
   - Open it in a second context; sign up. Expect Projects, Members and Report hidden. Expect only the tasks of projects they are on.
3. `the owner changes the colleague's role, and the colleague's page changes`
   - Make the colleague `manager`. Expect the colleague's Projects and Report to appear after their next action, with no sign-in.
4. `the report totals every project and person, including ones the admin is not on`
   - Seed tasks with known statuses. Expect the report's counts to match, row by row.
5. `two people edit one task: the second save is refused and keeps its text`
   - As in `notes.spec.ts:137`, on a task title.
6. `names sort in codepoint order`
   - Invite 3 colleagues named `a-<u>`, `Z-<u>`, `Aa-<u>`. Read `people`. Expect `Aa-`, `Z-`, `a-` in that order.
7. `tasks with no due date sort last`
   - 3 tasks: due tomorrow, none, today. Expect "Due soon" to show today, tomorrow, then the one with none.
8. `a move with no revision is refused before it is sent`
   - In `page.evaluate`, call `moveTask` through the generated client without `rev`. Expect a `missing_param` refusal and no request in the network log.
9. `a bad access template refuses the boot and names the setting`
   - Start a second door with `ORDO_ACCESS_TEMPLATE=reportByProject=nobody`. Expect it to exit non-zero and its log to name `ORDO_ACCESS_TEMPLATE` and `nobody`.

**CI.** `teamspace e2e` job.

**Acceptance.** Lines 184, 213, 292, 829 move to E (line 292 to I: a boot, not a page).

**Size.** M. 1 to 2 sessions.

### X4: marketing site

**Guarantees covered.** Audit line 83 (E, a second example) and the human check and cap of line 85.

**Files.** `examples/marketing-site/e2e/tests/browser/site.spec.ts` (new). The stack gets `E2E_HUMAN_CHECK=1` as the notes stack has: Turnstile's public always-pass and always-fail test keys. It needs the internet; run it on CI only.

**Test steps.**

1. `a visitor sees products, pricing and testimonials in rank order`
   - Open the page. Expect the 10 product cards in seed rank order, 3 plans with the highlighted one marked, 8 testimonials.
2. `a visitor opens a product by its link`
   - Click a product. Expect its page with its description and features.
3. `a visitor sends the contact form and is thanked, and the owner reads it`
   - Fill name, email, company, message; send. Expect the thank-you text.
   - The owner, signed in, reads `enquiryList`. Expect the message.
4. `a message over 2000 characters is refused in words`
   - Expect the form's error, and no new enquiry.
5. `with the human check on, a failing check is refused in words that name the fix`
   - Use the always-fail site key. Send. Expect the page's `human_check_failed` message.
6. `past the hourly cap the visitor is told to try later`
   - A door with `ORDO_PUBLIC_WRITES_PER_HOUR=1`. Send twice. Expect the page's `quota_exceeded` message the second time, and the response message to name `ORDO_PUBLIC_WRITES_PER_HOUR`.

**CI.** `marketing-site e2e` job.

**Acceptance.** The human check and cap parts of line 85 move from I to E.

**Size.** S.

### X5: first app

**Guarantees covered.** Line 176 (ids, second example). First-app README §5: "the rendering has not been opened in a browser".

**Files.** `examples/first-app/e2e/{package.json,playwright.config.ts,stack.sh}` (new). The stack runs `ordo dev examples/first-app` (the real binary), reads the token it prints, and starts `web/serve.py` with it. `examples/first-app/e2e/tests/browser/readings.spec.ts` (new).

**Test steps.**

1. `a person adds a reading and sees it at the top of the list`
   - Type `first-<u>`, press Add. Expect it first in the table. Add `second-<u>`. Expect it first, `first-<u>` second.
   - Expect each id to start with the `readings` prefix shown in README §3.
2. `a reading cannot be given its own id`
   - POST `addReading` through `serve.py` with an `id` field. Expect a refusal shown in the refusal banner, and no row with that id.
3. `with no token the page shows the refusal, not a blank table`
   - Restart `serve.py` with no `ORDO_TOKEN`. Reload. Expect the banner to show the refusal class.
4. `the page holds no credential`
   - Expect `localStorage` and `sessionStorage` empty, and no `authorization` header on any request the browser sent.

**CI.** `first-app e2e` job.

**Acceptance.** Line 176 moves to E. README §5 is replaced by a line naming the test.

**Size.** S.

### X6: assistant

The assistant has no page. Ask 4 lists it, so it gets one: the manager's page.

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 119 | I | An agent acting for a person can call only the approved ops, never steps up, and stops at the next call when withdrawn |
| 117 | I | ... withdrawing a delegation ... ends access everywhere (the page half) |
| 118 | I | Widening what an op reads or discloses kills every grant over it until a person consents again |
| 601 | U | A capability grant dies when an op's output expands, applies silently when it narrows |

**Changes to the example.**

- `examples/assistant/web/` (new, React on `@hominemai/ordo-react`, Vite; copy teamspace's `web/` layout):
  - sign in (guest, as the seed does, or passkey);
  - **Pending**: `pendingExpenses`, live, with Approve and Reject;
  - **History**: `history`, live, showing `decidedVia`;
  - **Assistant**: the consent screen for a waiting request (what each op reads and changes, from the door, as `manager/approve.mjs` prints it), an Approve button, and Revoke.
- `examples/assistant/README.md`: "Run it" gains the page.

**Files.** `examples/assistant/e2e/stack.sh` (grown from S5) also runs `agent/assistant.mjs` in a loop mode (add `--watch`: keep the feed open and approve small expenses as they come). `examples/assistant/e2e/fixtures/assistant-wider.ordo` (new): `pendingExpenses` also returns `note`. `tests/browser/manager.spec.ts` (new).

**Test steps.**

1. `the manager reads what the assistant asks for, in words, and approves it`
   - The agent asks. Open Assistant. Expect the 2 ops named, with what each reads and changes. Approve with a passkey touch.
2. `the assistant approves a small expense and the history names the op it used`
   - A staff member submits 42.50. Expect it to leave Pending and show in History as approved via `approveSmallExpense`, with no reload.
3. `a large expense waits for the manager`
   - Submit 250.00. Expect it to stay in Pending. The manager approves it. Expect History to show `approveExpense`.
4. `after Revoke, the assistant's next call is refused and nothing more is approved`
   - Press Revoke. Submit 30.00. Expect it to stay in Pending for 10 seconds while the agent runs, and the agent's log to show `delegation_required`.
5. `widening an op the grant covers ends the grant until the manager consents again`
   - With a live grant, apply `assistant-wider.ordo` with `ordo apply`.
   - Expect the agent's next call refused, and the Assistant screen to show the grant needs consent again. Approve again. Expect the agent to work.

**CI.** `assistant e2e` job.

**Acceptance.** Lines 119, 118, 601 move to E.

**Size.** L. 2 sessions: the page, then the tests.

## The CLI's whole life, and README steps as scripts

Ask 5: CI runs each example's README steps as a script, and at least one example goes from `ordo dev` through apply and deploy to a running deployment.

### C1: `ordo dev` with the first app

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 644 | I | ordo dev: one command brings up its own Postgres, serves, recompiles on every save with no restart, additive schema saves keep rows |
| 645 | I | The token ordo dev prints really calls an op as the seeded mem_dev member; no token is refused identically to an unknown op |
| 646 | U | seed.sql and --fresh reseed the dev database |
| 647 | I | A broken save prints a located error and the loop keeps serving the old program |
| 648 | I | ordo gen ts writes both typed packages deterministically and writes nothing for a program that does not compile |

**Files.** `scripts/cli-life/dev.sh` (new, bash, `set -eu`, every wait bounded by a timeout). It works on a copy of `examples/first-app` in a temp directory. `.github/workflows/examples-e2e.yml` job `cli life`.

**Steps.**

1. Run `ordo dev <copy>` in the background, its output to a log. Expect `addr=127.0.0.1:4000` and a `token` line within 60 seconds.
2. With the printed token: `addReading` then `listReadings`. Expect the row. With no token: expect the same answer as `/op/noSuchOp`.
3. Add `note: text = ""` to `readings` and save. Expect a `saved` line, the old row still listed, and `note` in the answer, with the same process id.
4. Save a file with a syntax error. Expect a located error (file, line, column). Expect `listReadings` still answering.
5. Fix it. Add `seed.sql` with 1 insert. Restart with `--fresh`. Expect `(seeded)` in the log and only the seeded row.
6. `ordo check` the copy: exit 0. On the broken file: exit 1.
7. `ordo gen ts` twice into 2 directories. Expect identical trees. On the broken file, expect nothing written.
8. Stop with SIGTERM. Expect exit 0 and the Postgres cluster gone.

**Acceptance.** Lines 644 to 648 move to E (the CLI as a developer runs it). Note the move as "E (CLI)" in the PR.

**Size.** M. 1 session.

### C2: the first deployment, apply, deploy, roll back and verify

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 748 | N | make-secrets plus compose bring up console, identity and deployment; the log shows the invite; Deploys applies notes v2 and lists it active (untested #12) |
| 750 | N | A database on the wrong host or port refuses with a message naming host:port (untested #13) |
| 582 | N | The identity service refuses a database URL that names the console's or the deployment's database (untested #11) |
| 632 | I | Rolling back is an ordinary activation and leaves a record |
| 75 | I | History cannot be changed behind the platform's back (adds: `ordo verify` after real use) |
| 650 | I | ordo login / whoami / touch / machine key / logout |

**Files.** `scripts/cli-life/first-deployment.sh` (new). `examples/first-app/e2e/tests/deploy/console.spec.ts` (new, Playwright, Chromium, virtual passkey) for the browser steps. Job `first deployment` in `examples-e2e.yml`, on GitHub-hosted `ubuntu-latest` only (it needs Docker; see question 6).

**Steps.** Follow `docs/first-deployment.md` word for word.

1. `docker build -f docker/Dockerfile -t ordo:dev .` (or reuse the `images.yml` build artifact).
2. `scripts/make-secrets.sh one`. Expect `Made N secret files`. Run it again. Expect a refusal and no file changed (compare checksums).
3. `docker compose -f docker/compose.one.yml up -d`. Expect the invite line in the log within 120 seconds.
4. (Playwright) Open the invite link. Make a passkey. Expect the recovery codes shown once, then the console.
5. (Playwright) Deployment screen. Expect "door reachable".
6. `ordo login <console>/identity --no-browser`; Playwright opens the printed URL and confirms. Expect `ordo whoami` to name the person.
7. `ordo apply examples/first-app --host <door> --space system`. Expect `schema created`, `stored`, `activated`.
8. Call `addReading` and `listReadings` with the CLI session's token. Expect the row.
9. Apply a changed copy (a new field). Then `ordo rollback --version #1 --host <door>`. Expect the first version active, the row still there, and `ordo versions list` to show both with a rollback record.
10. `ordo verify` against the deployment's database. Expect exit 0. Edit one command-log row with psql. Expect exit 3 naming the index.
11. Wrong port: restart with the database port changed. Expect the log to name `host:port` and what to check.
12. Wrong identity database: point the identity service's database URL file at the deployment's database. Expect its boot refused, naming the clash.
13. `ordo logout`. Expect `ordo whoami` to say signed out.

**Acceptance.** Lines 748, 750, 582 move from N to E. Lines 632, 75, 650 move to E.

**Size.** L. 2 to 3 sessions. Pace: an image build per CI round (about 15 minutes more), and the Docker runner question.

### C3: every example's README steps, run as a script

**Goal.** Each README's run steps work as written.

**Files.**

- `scripts/readme-steps/<example>.sh` (new), one per example: notes, notes-v2 (via `docs/first-deployment.md` §7), note-viewer, shopfront, teamspace, marketing-site, first-app, assistant. Each runs the README's commands in order, with bounded waits, and checks the "you should see" lines the README states.
- `scripts/readme-steps/agree.mjs` (new): for each README, extracts every shell line in the run sections' fenced blocks and fails if a line is missing from the matching script. So a README change without a script change fails CI.
- Job `readme steps` in `examples-e2e.yml`, a matrix over the examples.

**Steps, per script.** The README's commands. For example, `shopfront.sh`: `cargo build`, `sh dev/serve.sh` in the background, wait for `127.0.0.1:4010/health/live`, `npm install && npm run build`, `seed.mjs make-staff <id>`, then curl the catalogue and expect 45 products. The walkthroughs that need a browser are covered by X1 to X6; the script checks the server side of each step.

**Acceptance.** All 8 scripts green. `agree.mjs` fails on a scratch README edit, and passes after the script follows.

**Size.** M. 1 to 2 sessions. Pace: one CI round per broken README step found.

## V1: proof verification in shopfront

Ask 3: shopfront delivers orders to a small receiver that checks each with `ordo-verify-proof`, and a tampered delivery is refused.

**Prerequisite, a product change (question 1).** A booted host cannot deliver an attestation to a receiver on `localhost`. `crates/ordo-host/src/attestdrain.rs` `DeliveryConfig::from_env` uses `TlsRoots::webpki()` only and `DenyList::categories_only()`, which denies loopback. It does not read `ORDO_EGRESS_PRIVATE_HOSTS` or any trust-anchor file, as mail and OIDC do. Do not change it until the owner answers.

**Guarantees covered.**

| Audit line | Level now | Guarantee |
|---|---|---|
| 73 | I | Anyone outside the deployment can check a tenant's history offline, and the check fails when one link is moved or the key is not the one they trust |
| 417 | N | A chain proof discloses no space content (untested #10) |
| 415 | I | A third party can verify a chain proof without the deployment; no key means UNVERIFIED; a key they do not trust is no evidence |
| 76 | I | Every answered request carries a signed head a client can pin |
| 74 | N | A receipt can be checked by someone outside the deployment (untested #1; only with question 2) |

**Changes to the example.**

- `examples/shopfront/receiver/` (new): `receiver.mjs` (Node, https on port 4060 with `crates/ordo-host/tests/fixture/localhost.pem`), `package.json`, and a 1-screen page at `/` listing each order with **Verified** or **Refused** and the reason.
  - `POST /attest`: an attestation delivery. Check `X-Ordo-Signature` (HMAC as `docs/howto-audit.md` "Authenticating a delivery" says) and `X-Ordo-Timestamp` (300 seconds). Write the body to a file and run `ordo-verify-proof attestation --file <f> --key <hex> --key-id <id>`. Keep the head when exit is 0. Answer 422 with the exit code otherwise.
  - For each new order (the receiver reads `allOrders` as a staff member, signed in with `dev/passkey.mjs`): fetch `std.space.chainProof` over the range from the last verified position to the order's, save its `proof` field verbatim, run `ordo-verify-proof proof --file <f> --key <hex> --key-id <id> --head <last attested head>`. Mark the order Verified on exit 0, Refused on 3, and an input error on 1.
- `dev/serve.sh` and the e2e stack: set `ORDO_CHECKPOINT_KEY`, `ORDO_CHECKPOINT_KEY_ID`, and whatever the owner's answer to question 1 adds. The shop owner runs `std.space.attestTo` (consent bytes from `ordo-verify-proof consent`, signed with an Ed25519 key the receiver holds). Find how the attestation cadence is set for a space and set it low (every few commands) for the test.
- `examples/shopfront/README.md`: a new section, "Checking the shop's record from outside".

**Files.** `examples/shopfront/e2e/tests/browser/receiver.spec.ts` (new).

**Test steps.**

1. `each order reaches the receiver and is verified against the shop's record`
   - A customer places 3 orders. Open the receiver's page.
   - Expect 3 rows, each Verified, within 60 seconds. Expect the receiver's log to show exit 0 from `ordo-verify-proof` for each.
2. `a tampered delivery is refused`
   - Take a real delivery the receiver logged. Change 1 hex digit of `head`, recompute the HMAC with the sink secret (so only the Ed25519 check can catch it), and POST it.
   - Expect 422, exit code 3 in the answer, and a Refused row naming verification failed.
3. `a proof with one link moved is refused`
   - Take a saved proof. Swap 2 adjacent `hash` values. Run the receiver's check on it.
   - Expect exit 3 and Refused.
4. `a proof checked under a key the receiver does not trust is not evidence`
   - Check a real proof with another key. Expect exit 3. With no key, expect `UNVERIFIED` in the output and the row not marked Verified.
5. `a chain proof carries none of the shop's content`
   - Use known values: a product named `SECRET-<u>`, a customer named `CUST-<u>`, an order note `NOTE-<u>`, and the request's correlation id.
   - Fetch a proof over the range holding those writes. Expect none of the 4 strings in it.
6. (only with question 2) `an account-deletion receipt checks out offline`
   - A customer deletes their account. Expect the receipt. Run the new verifier mode on it. Expect exit 0, and exit 3 after changing 1 byte.

**CI.** `shopfront e2e` job. The job builds `ordo-verify-proof` too.

**Acceptance.** Lines 73, 415, 76 move to E. Line 417 moves from N to E. Line 74 moves to E only with question 2.

**Size.** L. 2 sessions after the product change. The product change is M on its own.

## Summary

| Id | Increment | Size | What sets the pace |
|---|---|---|---|
| 0 | Coverage of the example tests, and the ratchet | L | 3 to 5 coverage runs of about 35 minutes |
| S1 | Tenant isolation across every path (and the teamspace stack) | L | 3 to 4 CI rounds; the shared stack library |
| S2 | Public page never leaks (and the marketing stack) | M | 2 CI rounds |
| S3 | Public cache by database-read count, across nodes | M | the stats flush delay; 2 CI rounds |
| S4 | Live across clients and nodes | M | a second door in the notes stack |
| S5 | Revocation reaching open feeds | M | the teamspace leave control; the assistant stack |
| U1 | 4 untested through examples | M | 2 CI rounds |
| U2 | A write op cannot be a source | S | 1 CI round |
| U3 | The 3 first left out: peer from a tenant database, slow-filter warning, durations | 2 M and 1 L | the duration model is the bulk |
| X1 | Notes v2 suite and forged ID tokens | M to L | the bad-issuer stand-in |
| X2 | Shopfront suite | L | 3 origins; form and paging changes |
| X3 | Teamspace suite | M | 2 CI rounds |
| X4 | Marketing site suite | S | needs the internet for Turnstile test keys |
| X5 | First app suite | S | 1 CI round |
| X6 | Assistant page and suite | L | a new page |
| C1 | `ordo dev` life | M | 1 to 2 CI rounds |
| C2 | First deployment, apply, roll back, verify | L | Docker image build per round; question 6 |
| C3 | README steps as scripts | M | one round per broken README step |
| V1 | Verify-proof in shopfront | L | owner's answer to question 1; the product change |

Total: about 29 to 37 agent sessions and 52 to 67 CI rounds, with U3. Every coverage run adds about 35 minutes.

### Expected coverage gain

These are estimates. Increment 0 measures the real baseline. The unit number stays about 86.5%; this work moves the examples number.

| Id | Crates that should move | Estimate, examples number |
|---|---|---|
| 0 | baseline: ordo-host, ordo-api, ordo-exec, ordo-identity, ordo-blob | 40% to 55% of workspace lines (CLI, console, conformance unreached) |
| S1 | ordo-host (spaces, blob door, export), ordo-api | +1 to +2 points |
| S2, S3 | ordo-host `public.rs`, `invalidation.rs`, `pgsync.rs`, `metrics.rs` | +1 point; `public.rs` toward 80% |
| S4, S5 | ordo-host `pgsync.rs`, `sessions.rs`, `delegation.rs`, `member_ops.rs` | +1 to +2 points |
| U1 | ordo-plan, ordo-cli `migrate`, ordo-host `access_template.rs` | +1 point |
| X1 to X6 | ordo-host auth, `oidc.rs`, `consent.rs`, refine paging; ordo-sdk paths | +3 to +5 points together |
| C1, C2 | ordo-cli (`dev.rs`, apply, rollback, verify), ordo-supervisor, ordo-console, ordo-identity-service | +5 to +8 points: these crates are near 0 in the examples today |
| C3 | little new code; it guards the READMEs | under +1 point |
| V1 | ordo-verify-proof (binary), `attestdrain.rs`, `attest_ops.rs`, ordo-egress | +1 to +2 points |

## Open questions for the owner

**Answered on the board, 2026-11-23** (task `close-the-coverage-gap-through-the`):

- Question 1: yes. Deliveries honour `ORDO_EGRESS_PRIVATE_HOSTS` and gain `ORDO_EGRESS_CA_FILE`, both off by default.
- Question 3: a crate may dip 0.1 of a point before the ratchet fails.
- Question 5: the TypeScript clients join the ratchet after the Rust crates.
- Questions 8 and 9: no. All 3 are tested, in U3.
- Question 11: no. `two_instances.rs` runs in the coverage job on PRs into `main`.

The list below is as first written.


1. **Attestation delivery to a test receiver needs a product change.** `DeliveryConfig::from_env` ignores `ORDO_EGRESS_PRIVATE_HOSTS` and has no trust-anchor setting, so a booted host cannot deliver to `https://localhost`. **Recommendation:** honour `ORDO_EGRESS_PRIVATE_HOSTS` for attestation as mail and OIDC do, and read extra anchors from a new `ORDO_EGRESS_CA_FILE` (off by default, a refusal names it). Fallback if no: the receiver pulls proofs only, with no webhook.
2. **Receipts (untested #1) have no verifier.** `ordo-verify-proof` has `proof`, `attestation` and `consent` modes, none for a receipt. **Recommendation:** add a `receipt` mode for the account-deletion receipt in V1; leave tenant erasure receipts to their own task.
3. **Ratchet tolerance.** Coverage moves a little between identical runs. **Recommendation:** 0.1 points per crate.
4. **Is the ratchet a required check on PRs into `main`?** **Recommendation:** yes, required, so "may not lower" holds.
5. **What the ratchet covers.** **Recommendation:** every Rust crate for the unit number; for the examples number, every crate the examples reach above 0. Add the TypeScript clients later as their own ratchet.
6. **Docker for C2.** The cluster runners may have no Docker. **Recommendation:** C2 runs on GitHub-hosted `ubuntu-latest` always, whatever `ORDO_RUNNERS` says.
7. **How to count database reads.** **Recommendation:** `pg_stat_user_tables` scan counters: no extension, works on the CI service Postgres. `pg_stat_statements` needs a preload the service container cannot set.
8. **Untested #15 and #16 are documented gaps, not promises.** **Recommendation:** leave them out of this plan. File #15 as its own task if a tenant in its own database should be refused from reaching a peer.
9. **Untested #6, durations.** The doc says bare duration is not built. **Recommendation:** test it when it is built; out of this plan.
10. **The assistant page is new UI.** **Recommendation:** a minimal manager page (Pending, History, Assistant with consent and Revoke), in the teamspace page's style.
11. **`crates/ordo-host/tests/two_instances.rs` stays `#[ignore]`.** S4 covers its cases end to end. **Recommendation:** leave it as is; revisit after S4 is green.
12. **The audit says no example declares money or a multi-statement op.** Shopfront declares `money(2)` and `markShipped` writes twice. **Recommendation:** correct the audit's line in the owner's session; X2 step 9 proves the failed second write undoes the first.

## W: reach the web-accessible Rust decision trees

Planning supplement, 2026-11-24. This adds an execution plan, not a claim
that these features or their tests are complete. It refines the earlier
increments around the owner's request: minimum application functionality
that exercises nearly all Rust reachable from an app, down its decision trees.

### W scope and success measure

The target is 100% of executable lines reachable through supported web
interfaces, approached through real application journeys. There is no
measured web-only percentage yet. The 33.57% report is a workspace example
number; it is not this denominator and must not be relabelled as one.

Include application ops, platform ops, authentication, live streams, blobs,
functions, identity-service APIs and the console's web APIs. Include shared
Rust libraries and asynchronous work caused by those calls: database work,
scheduled jobs, delivery retries, invalidation, backfill and recovery.
Deployment-owner APIs count, with their real authorization requirements.

No unit-test hits contribute. No internal Rust function calls substitute for
requests. CLI setup can provision the stack, but its coverage must be reported
separately from coverage collected during web journeys. Keep the earlier CLI
lifecycle work as a separate release check, not a way to inflate this target.

Count lines and decision outcomes separately. A line executed once does not
prove every branch on that line. The goal is all reachable outcomes, not all
combinations of inputs. Add combinations where state, permissions, retries or
concurrency interact. Do not promise a percentage gain from a feature list.

### Starting evidence

The last inspected matching-source example union measured 52,915 / 157,646
Rust lines (33.57%). It includes failed scenarios and is not release acceptance.
Useful priorities from its file-level report are:

| Source | Example line coverage | Next slice |
|---|---:|---|
| `ordo-exec/src/import.rs`, `export.rs` | 0% | W3 |
| `ordo-host/src/bucket_ops.rs`, `ordo-blob/src/s3.rs` | 0% | W3 |
| `ordo-host/src/identity_ops.rs`, `auth/recover.rs` | 0% | W2 |
| `ordo-host/src/attest_ops.rs` | 0% | W4 |
| `ordo-host/src/backfill.rs` | 0% | W5 |
| `ordo-host/src/evolve.rs` | 6.0% | W5 |
| `ordo-host/src/clones.rs` | 10.0% | W5 |
| `ordo-host/src/windows.rs` | 2.2% | W5 |
| `ordo-console` | 0% | W0 inventory, then W5 |

These are source targets, not estimates of attainable gains. Rebaseline at W0
and do not compare these percentages to a different source revision.

### W0: establish the denominator and the missing-journey ledger

**First deliverable.** A versioned inventory, tied to a Rust commit, with one
entry per route or registered operation family. Reconcile it with the runtime
catalogue in each supported configuration. Regex searches are discovery aids,
not proof that an operation is registered or callable.

Source entry points to inventory:

- `crates/ordo-api/src/router.rs`: `/op/{name}`, both live protocols, blob
  upload/read/metadata, `/fn/{name}`.
- `crates/ordo-host/src/auth/`: the shared authentication route registry.
- `crates/ordo-host/src/serve.rs`: catalogue, interface, deployment health,
  usage, metrics and security surfaces.
- `crates/ordo-host/src/platform_ops.rs` and `runtime.rs`: platform specs,
  dispatch and configuration-dependent registration.
- `crates/ordo-identity-service/src/`: service routes and their host callers.
- `crates/ordo-console/src/server.rs`: console web routes and downstream calls.

For each entry record: method/name, registration condition, allowed actors,
request contract, handler, downstream modules/workers, configuration profile,
application action, test, observed result and remaining uncovered ranges.
Trace call sites as well as declarations. Trace worker enqueue and completion.
Do not assume a crate is out of scope because its first report is 0%.

Classify executable ranges as:

- **Reachable:** an identified web call or its downstream worker can execute it.
- **Unresolved:** reachability has not yet been established. Keep these visible
  and in the conservative denominator until reviewed.
- **Non-web:** evidence identifies a CLI-only, startup-only or other path with
  no web caller. Record the callers and why the path is excluded.
- **Unreachable invariant:** a specific outcome is impossible under supported
  wiring. Record the invariant and supporting source. Lack of a test is not proof.

Review exclusions by function/range, not blanket crate exclusions. Keep raw
workspace coverage beside the scoped result. Show excluded and unresolved line
counts, and fail inventory validation when source changes invalidate ranges.
Code absent from an instrumented binary is unmeasured, not automatically excluded.

Add an application-only report script and a reachability manifest under
`coverage/`, plus validation tests for the accounting itself. Those script tests
are not product-coverage evidence. Reuse the existing LCOV union accounting.
Collect raw LLVM coverage JSON as well, to preserve branch data when available.
Check the pinned Rust/LLVM toolchain's branch support in CI before choosing
flags. If it cannot report branches, publish the explicit outcome ledger;
never present line coverage as branch coverage.

Start profiling after provisioning; reset or separately tag setup profiles.
Use distinct profiles per process and scenario, stop workers cleanly and merge
only matching source revisions. Capture profiles on failed runs too, but label
those results as failed. Compare baseline and additions using the same binaries,
configuration and denominator. Record both newly reached and lost lines.

**Acceptance:** every registered surface is mapped; every denominator exclusion
has evidence; each zero-hit reachable range has an assigned journey or a named
investigation. A missing route/configuration fails the inventory check.
**Size:** L, 2 to 3 sessions, 2 CI rounds. Call-graph review and branch tooling set the pace.

### Minimal application additions

Reuse existing pages and authentication. Add controls only where they unlock a
new path. Do not build a generic endpoint console or expose privileged actions
to ordinary members. Page mutations use the shared typed client and error path.
Tests may send malformed HTTP requests the UI correctly prevents, attached to
the corresponding real user journey.

| Surface | Minimum additional functionality | Why it exists |
|---|---|---|
| Marketing studio | Saved filters, paged results, bulk edit, conflict resolution, activity | Query and write branches without another business app |
| Studio account and team | Credentials, sessions, recovery, groups, access policy and sharing | Full identity and permission lifecycle |
| Studio data and storage | Import a staged bundle, export, files, tenant storage settings | Data portability, custody, ownership and backend failures |
| Studio automation | Function runs, scheduled jobs, delivery status and retry | Async execution and its failure paths |
| Existing owner console | Version preview/apply, evolution, backfill status, clone and rollback controls where supported | Web-accessible deployment operations |
| Existing note-viewer and assistant | Federation consent and delegated-action controls | Cross-origin trust and delegated permissions |
| Existing shopfront receiver | Verify delivered proof and show accepted/rejected delivery | Complete the app-triggered attestation journey |

One small data model can cover most value paths: a campaign with money, dates,
optional values, JSON tags and a sealed note; owned tasks and assets; a unique
external reference; membership and sharing. Add a field or operation only after
mapping it to an uncovered planner, executor or binding outcome. Duration
semantics remain U3 product work, not an assumed existing capability.

### Decision-tree template for every journey

Each journey names its entry point, starting state, actor, action, branch and
visible outcome. It also states which data and permissions must remain unchanged.
Use the following applicable outcomes, marking unsupported ones with evidence:

1. Allowed success; empty/single/multiple results; enough rows for 3 pages.
2. Missing, malformed, wrong-type, out-of-range and boundary input; null versus
   omitted/default; incompatible method, version, cursor or refinement.
3. Anonymous, valid member, wrong role, foreign tenant, expired/revoked session,
   stale presence, delegated caller and deployment owner where supported.
4. Duplicate/idempotent retry, stale revision, uniqueness conflict and failure
   after an earlier write in the same transaction. Observe rollback.
5. Permission/state change while a read, live stream, upload or job is in flight.
6. Real dependency timeout/disconnect/refusal, restart and recovery. Verify data
   preservation and exactly the documented retry/delivery semantics.
7. Configuration enabled, disabled and supported alternatives; registration and
   refusal behavior must match the configured capability.

Assert response class and status, user-visible state and subsequent reads by
both authorized and unauthorized people. Use distinct tenants and principals.
For absence on a live feed, wait for a permitted sentinel then assert the secret
never arrived. Use observable readiness and bounded polling, not fixed sleeps.

### W1: query, write and live behavior

Extend the studio's existing lists and editor. Add filter/sort/search controls,
relationship summaries and one atomic bulk action. Seed boundary values and
more than 2 pages. Cover open/closed refinements, aggregates, joins, defaults,
JSON, sealed searches where supported, delete/cascade and transaction rollback.

Use 2 browsers on 2 host replicas. Cover both live transports, subscribe/remove,
reconnect, stale cursors, empty results becoming nonempty, ordering changes,
permission loss, tenant switching, backpressure and public cache invalidation.
Reuse S1 through S5 assertions, including the currently failing strict checks.
No cache bypass merely to make the page see a fresh answer.

**Targets:** `ordo-api` bind/refine/page/router/live/idempotency, `ordo-plan`,
`ordo-exec` query/write, host refinement, public/cache and invalidation paths.
**Acceptance:** every selected query/write branch has a page journey and the
matching malformed/refused request variants; live and tenant isolation survive
concurrent changes. **Size:** L, 2 to 3 sessions, 2 to 3 CI rounds.

### W2: account, access and recovery

Add account settings to the existing session UI. Exercise every configured
sign-in method, credential enrolment/removal, second factors, session renewal,
sign-out, recovery codes, recovery holds/objections and owner succession.
Use the actual mail capture service, virtual WebAuthn devices and a protocol
OIDC provider. Ordo's auth, token checks, session storage and policy remain real.

Add groups, scoped sharing, invitation expiry/reuse and tenant access choices.
Use the existing assistant for delegation consent, scope restriction, expiry
and withdrawal; use note-viewer for federation approval and revocation.
Drive the already exposed `std.identity.*` recovery cases and
`std.space.recoverMember`/`succeedOwner` as the proper actors.

**Targets:** host `identity_ops.rs`, `auth/recover.rs`, credentials, factors,
recovery policies, membership, consent, delegation and identity-service flows.
**Acceptance:** each configured method works end to end and cannot bypass its
required factor/presence/role; recovery does not transfer another person's data;
revocation reaches both replicas and already open feeds.
**Size:** L per authentication/recovery and sharing/federation slice, 4 to 6
sessions total, 4 to 6 CI rounds. Provider and recovery state setup set the pace.

### W3: data portability and storage

Add a data panel using `std.space.export` and `std.space.import`, distinct from
an ordinary SDK row download. The existing import reads a bundle staged under
`ORDO_IMPORT_BUNDLE_DIR`; `runtime.rs` confirms that an unset directory
leaves the import operation unregistered.
The fixture stages real bundles in the destination space's authorized directory;
the page selects a bundle and invokes import. Do not invent an upload API or
let browser input select an arbitrary server path.

Exercise correct and malformed bundles, schema mismatch, people mapping,
sealed fields, attached blobs, duplicate import, resume after interruption and
foreign-space/path traversal refusals. Compare actual row values, references,
access and file bytes after export/import. Compare page reads after failure.

Add storage controls for `std.space.attachBucket` and `detachBucket`. Run file
journeys against both local storage and an S3-compatible service, including
metadata, missing/corrupt objects, quota/size boundaries, backend outage and
retry. Verify the applicable public/private and tenant authorization branches.
Crypto-erasure/deletion uses disposable spaces and proves old credentials and
handles no longer read data while an unaffected tenant continues working.

**Targets:** host `exit_ops.rs`, `bucket_ops.rs`, `blob.rs`; executor import,
export, bucket, usage; `ordo-blob/src/s3.rs`; relevant custody/crypto paths.
**Acceptance:** a complete round trip preserves content and boundaries;
interrupted imports resume without duplicate business records; storage failures
do not acknowledge data that cannot subsequently be read.
**Size:** L, 2 to 3 sessions, 3 CI rounds. Bundle and S3 profiles set the pace.

### W4: functions, jobs and deliveries

Extend campaign automation with run status and inputs, a bounded batch task,
one scheduled task and one outbound integration. Cover typed result validation,
permitted/forbidden nested ops, limits, timeout, cancellation where exposed,
missing secrets and unavailable destinations. Use a controllable HTTPS receiver
for delayed, malformed, refused and successful responses.

Run the real scheduler and delivery workers. Observe the resulting data or
receiver message, retry/dead-letter state, and replay/retry only through the
supported application action. Do not manually call a worker's Rust function.
Use shopfront's proof receiver for `std.space.attestTo`, `chainProof`,
`attestation` and delivery dead letters. Verify tampering is rejected by the real
verifier. Keep the existing V1 receipt capability questions visible.

**Targets:** `ordo-fn`, `ordo-egress`, host routines, function runs, attestation
and delivery workers, proof verification, command log and cryptographic checks.
**Acceptance:** successful, refused and failed runs have the documented effects;
retries and restarts preserve the stated delivery semantics and caller scope.
**Size:** L, 2 to 3 sessions, 2 to 3 CI rounds.

### W5: web owner operations and schema lifecycle

Use the existing console where it already exposes the action. Add the smallest
missing control for a registered web operation. Do not put deployment ownership
credentials in the marketing app's public session or widen its roles.

Exercise version source/preview, activation, tenant pinning, revocation,
evolution, replay and rollback checks. Source anchors include `deploy_ops.rs`,
`evolve.rs` (`std.deployment.evolve`, `plans`, `replay`, `rollbackCheck`),
`clones.rs` (`cloneOut`, `cloneIn`) and `window_ops.rs`.

Prepare 2 small schema revisions of the same campaign application. Add a
required field through a backfill and change a constrained value type. Test
invalid source, unsafe changes, explicit money rescale/keep, failed mapping,
concurrent writes during backfill, interrupted worker, resume and activation
only after verification. Exercise shared and dedicated tenant databases.
Clone to an isolated destination; verify tenant data/custody restrictions and
refusals for invalid keys/source. Exercise maintenance windows and environment
promotion when registered. After every transition, both old and new browser
sessions read/write according to the served-version contract.

**Targets:** console HTTP handlers, syntax/sema/schema code reached by source
submission, host apply/evolve/backfill/version/clone/window/promotion paths.
**Acceptance:** data survives successful evolution and refused/failed changes;
replicas agree on serving state; rollback eligibility is observed before use.
**Size:** 2 L slices, 4 to 6 sessions, 4 to 6 CI rounds. Backfill and multi-node
state convergence set the pace. This uses local disposable stacks, never Argo.

### W6: failure profiles and the remaining branches

Apply this alongside W1 through W5, then use the uncovered-range ledger to find
what is still missing. Do not create more business pages until an existing page
plus data/configuration/fault variation has been ruled out.

Run supported profiles for public/authenticated access, shared/dedicated tenant
DBs, local/S3 storage, single/multiple hosts and local/federated identity.
Pairwise combinations limit routine CI cost; explicitly run combinations that
share state or trust boundaries. Do not claim a pairwise matrix proves all cases.

A fixture controller may stop/restart only test processes, interrupt proxy links,
delay receiver replies and change supported configuration. Application actions
still enter through HTTP. Do not fabricate a Rust error result or stub Ordo's
clients, access checks, executor or storage implementation. Stage corrupt data
only when modeling a real persisted-data failure, record how it can arise, and
verify the app-visible diagnosis and unaffected tenant behavior.

Include limits, retry exhaustion, contention, session expiry, concurrent owners,
unsupported configuration and hostile requests. Test secret redaction on both
success and error paths. Security-sensitive refusals remain release findings
when wrong; never relax assertions to improve coverage.

**Acceptance:** every remaining reachable zero-hit range has a concrete journey,
a reproduced defect, or an explicit unresolved investigation. Documented gaps
remain gaps, consistent with the owner's refusal to omit them in U3.
**Size:** M per remaining family, 1 to 2 CI rounds each. Count is measured after W5.

### Coverage review: explicit journeys missing from the first draft

Source review on 2026-11-24 found the following families under-specified.
They are mandatory entries in W0 and extensions to the named slices, not
reasons to build another application. Their registration and supported behavior
must be checked before selecting exact request parameters.

| Family | Minimum journey and outcomes | Owner slice / source evidence |
|---|---|---|
| Tenant lifecycle | Create a space through the app; self-service allowed/refused; pause/resume as tenant and deployment owner; leave, last-owner refusal where required, reap and delete; unrelated tenant keeps working | W2/W3/W5; `tenant_ops.rs`, `self_serve.rs`, `pause.rs`, `release_ops.rs`, `reap_op.rs`, `fleet.rs` |
| Console bootstrap and trust | First setup, owner invitation/removal, console passkey recovery and step-up; link the door, rotate/revoke its key, unlink/forget and relink; old authority stays refused | W5; `ordo-console/src/server.rs` setup, owner and door routes |
| Console runtime control | Apply, rollback and restart through console HTTP; unavailable runtime/door and partial failure; browser reconnects and verifies persisted app data | W5/W6; console runtime routes and their real downstream supervisor calls |
| Machine and device credentials | Browser approval followed by actual token redemption/use; rejected/expired/reused approval; wrong audience/grant, rotation and revocation; no elevation into human-only presence actions | W2; identity-service `/cli/approve`, `/cli/redeem`, `/machine/begin`, `/machine/complete`, host `deployment_access.rs` |
| Live policy changes | Set tenant/deployment session policy and password slowdown; change provider connection, origin list and self-service policy while sessions/feeds exist; verify documented inheritance and effective permissions | W2/W6; `session_policy.rs`, `guess_limits.rs`, `tenant_providers.rs`, `tenant_origins.rs` |
| Secret lifecycle | Set/list/replace/remove an integration secret; listing never returns its value; function/delivery observes permitted changes; removed and foreign secrets stay inaccessible | W4/W5; `secrets.rs`, function and egress consumers |
| Operational read surfaces | Read usage, observed cost, health and security records as allowed/forbidden actors; empty/populated/paged output and unavailable dependency; no tenant names or secrets leaked | W5/W6; `serve.rs`, `observed_cost.rs`, `security_record.rs`, quotas and usage handlers |

**Actual browser protocol behavior (W1/W2/W6).** Run same-origin and allowed/
forbidden cross-origin pages, with actual preflight and fetch credential modes.
Exercise public versus authenticated routes, tenant-specific origins, refresh
cookie scope and supported cookie attributes, missing/mismatched Origin, expired
refresh and concurrent renewal. Verify the implemented anti-forgery checks on
cookie-authenticated mutations. A Node request cannot prove browser CORS or
cookie enforcement. CORS refusal must not be confused with server authorization:
`ordo-api/src/cors.rs` explicitly allows some actual requests while withholding
browser-readable cross-origin access.

At the HTTP boundary also cover malformed JSON, unexpected content types,
invalid UTF-8 where applicable, body limits, duplicate/ambiguous parameters,
wrong method, invalid encoded names and forwarded-address handling under trusted
and untrusted proxy configurations. Assert named refusal and no side effects.
Only test protocol variants the deployed server supports; record others as such.

**Identity-provider lifecycle (W2/W6).** Cover discovery and JWKS retrieval,
unknown key id, key rotation/cache refresh, issuer/audience/signature/time
refusals, unavailable provider and changed/retired tenant connection. Browser
consent must preserve redirect/state/nonce/PKCE requirements wherever the
supported flow uses them. Include an actual identity-service-backed profile;
a controlled external provider complements it rather than replacing it.
Use `ordo-identity-service/src/routes.rs` and the host's federation callers as
the inventory sources. Routes containing `cli` still count if they are HTTP
callable; the name alone is not evidence of a non-web exclusion.

**Stateful sequences and realistic scale (W6).** In addition to isolated tests,
run a seeded sequence of create/share/edit/upload/revoke/delete/rejoin actions
through normal interfaces. Persist the seed and action log for reproduction.
Check tenant separation, reference integrity, acknowledged writes and permissions
after each transition. Use actual read interfaces as the oracle, supplemented
by independent artifact/receiver checks. Exercise one large supported dataset,
slow consumer and sustained reconnect profile; define bounds from the product
contract or mark a missing limit as unresolved instead of inventing an SLO.

**Outcome completeness (W0/W6).** Map each applicable enum/error variant and
important boolean outcome in the reached handler chain to a journey, invariant
or unresolved entry. Reconcile this with dynamic branch data where supported.
Line/branch totals cannot establish full path coverage, and the full combination
of paths is generally unbounded. Focus sequence combinations on shared mutable
state and trust boundaries; record the residual risk explicitly.

These additions enlarge W2, W5 and W6. Re-estimate their session and CI counts
after W0 rather than retaining the first draft's sizes as a complete total.

### Known reachability questions, not assumed exclusions

- `ordo-exec/src/undo.rs` and `reseal.rs`: initial caller search finds direct
  invocations in CLI `operator.rs` and `keys.rs`. W0 must trace console/host
  callers before classifying individual functions. A generic browser shell
  executing CLI commands would not establish native web reachability.
- Supervisor and generator: inspect actual console and web-deployment callers;
  zero hits alone establishes nothing about reachability.
- Import staging and export artifact retrieval: distinguish an HTTP operation
  from a complete browser file-transfer path. Missing transport functionality
  is a separately identified product gap, not an imaginary existing endpoint.
- Disabled routes, impossible wiring guards and unbuilt semantics: retain
  evidence and the expected user contract. Do not implement new public APIs
  solely to turn currently non-web Rust into denominator coverage.

### Execution order and release evidence

1. W0 inventory and reproducible web-only baseline.
2. W1 query/write/live, then W3 portability/storage. These reuse the existing
   studio and attack measured executor gaps with little new UI.
3. W2 account/recovery and federation, then W4 jobs/deliveries.
4. W5 owner operations in 2 reviewed slices.
5. W6 remaining outcomes, prioritized by impact and uncovered lines.

Reuse the earlier S/U/X/V increments rather than implementing their tests twice.
All slices stay in PR #59 (owner, 2026-11-24). Before page work, load the required modern web
skill. Keep work in an isolated checkout, preserve Claude's active files and
follow this plan's board/roadmap handoff. No local full Rust builds alongside
another agent's builds. Sizes above are provisional; W0 resolves the unknowns.

Every PR reports the exact source revision, scenario/profile matrix, assertions
passed/failed, scoped line coverage, branch/outcome coverage, newly reached
ranges and remaining gaps. Attach the per-scenario profiles and reproduction
instructions. A failure found through usage gets a regression journey and a
Reported fixes handoff with symptom, cause when established, expected fix and
release impact. Do not mark the overall increment complete while its required
journeys fail.

Release evidence requires all supported entry points represented, all critical
permission/data-loss/recovery outcomes passing, no unresolved release-blocking
findings, and an honest disposition for every remaining uncovered range. A high
percentage cannot override a failed isolation or durability assertion. The
near-100% target is approached by closing that ledger, not by shrinking the
reported denominator or adding unit-test hits.
