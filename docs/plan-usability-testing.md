# Usability Testing Plan for Ordo

## Overview

Ordo is a backend platform where a developer writes `.ordo` definitions, runs `ordo dev` to serve locally with hot-reload, calls ops over HTTP, generates a typed TypeScript client, and deploys via `ordo apply`. This plan describes testing whether developers can actually get through the documented happy path (quickstart.md), both automatically on every commit and with real participants in a structured session.

The motivation is concrete: the quickstart was written by running every command against the actual `ordo` binary. On the first run, an agent discovered immediately that `ordo dev` printed no token, leaving the developer unable to call the op they had just written. This wall was invisible to 1,231 unit tests, 100 conformance cases, and 295 integration tests. The automated suite is designed to catch regressions in the happy path fast and loud. The human sessions catch moments of confusion, surprise, and friction that automation cannot see.

### Scope and Assumptions

This plan covers the happy path only: a developer with a Postgres database, a text editor, and a terminal who wants to get from zero to a working op in one sitting. It does not cover: deployment authentication flows, schema migrations, row-level guards, or production operations. It assumes the developer has used a command line and curl before. It assumes the machine is a standard developer workstation (macOS or Linux) with reasonable internet (not metered, not behind a proxy).

Deliberately out of scope: whether an enterprise architect would choose Ordo for their platform (design validation). Whether Ordo is faster than Firebase (benchmark). Whether the generated TypeScript client beats hand-rolled HTTP (UX validation). This plan is about whether the onboarding path actually works as written.

## 1. The Automated Half

### 1.1 The Tasks and Success Criteria

The automated suite walks the exact sequence in docs/quickstart.md from a clean filesystem:

**Task 1: Create and check a `.ordo` file**
- Entry state: Empty directory
- Command: `ordo check notes.ordo` where `notes.ordo` contains the notes domain from quickstart.md §1
- Success: The check output lists entities (notes), ops (addNote, completeNote, listNotes), and std ops without error
- Measurements: Time, whether output contains the word "entities" and all three op names, exact text of any error

**Task 2: Start the dev server**
- Entry state: Valid `.ordo` file, clean Postgres
- Command: `ordo dev . --fresh` (the real flags are `--fresh`, `--out` and `--gen`; there is no `--timeout`, so the harness owns the deadline and kills the child itself)
- Success: Server boots and prints "ordo-host" with listening address, a token line starting "Bearer", and "space=dev"
- Measurements: Time to first "Bearer" token, whether the bearer token is non-empty, whether address is 127.0.0.1:4000
- First-five-minutes checkpoint: Time from directory creation to callable token, as a number the plan calls out

**Task 3: Call an op without token**
- Entry state: Server running from Task 2
- Command: `curl -s -X POST http://127.0.0.1:4000/op/addNote -H 'x-ordo-space: dev' -H 'content-type: application/json' -d '{"title":"test","body":""}'`
- Success: Response is a JSON error with code "access_denied" and message containing "bearer token"
- Measurements: HTTP status, error code, error message text; whether it reveals "missing" or another state

**Task 4: Call an op with token**
- Entry state: Server running, token from Task 2
- Command: As in Task 3, with `--header 'Authorization: Bearer <token from Task 2>'`
- Success: Response is a JSON object with "mode": "creates", "op": "addNote", "rows" array with one row
- Measurements: Time, HTTP status, mode field value, row count, exact error if it fails

**Task 5: Read the list op**
- Entry state: Server running, token from Task 2, one note created by Task 4
- Command: `curl -s http://127.0.0.1:4000/op/listNotes -H 'x-ordo-space: dev' -H 'authorization: Bearer <token>'`
- Success: Response is JSON with "op": "listNotes", "rows" array containing the note from Task 4
- Measurements: Time, number of rows, whether the title matches what was sent in Task 4, exact error if missing

**Task 6: Generate TypeScript client**
- Entry state: Valid `.ordo` file from Task 1
- Command: `ordo gen ts . --out /tmp/ordo-test-gen` (temp directory to avoid side effects)
- Success: Command exits 0, produces interface.json and at least one generated TypeScript file
- Measurements: Time, file count, whether interface.json is valid JSON, whether generated files contain "listNotes"

### 1.2 What It Measures and Records

On every run:
- **Per-task metrics**: Time in seconds (measured by wrapping each command with `time`), success/fail boolean, exact command return code, exit reason (killed by timeout, etc.)
- **Automation assertions**: Does the next step's success message appear in the previous step's output? For Task 2, does output say "try it curl ..."? For Task 3, does the error message match quickstart.md's example exactly? Store these as boolean pass/fail in the JSON report.
- **Wrong-path cases**: A separate test run per edge case: (1) malformed `.ordo` file (missing entity type definition), (2) token that is 5 characters instead of valid bearer format, (3) path that does not exist, (4) network error simulation (if port is in use). For each, measure the error message text, whether it names the problem precisely, whether the output includes "Usage:" or another help text. Store exact error string.
- **The first-five-minutes number**: Wall time from Task 1 start to when the token from Task 2 is printed (measured in seconds from the test's start time clock until the Bearer line appears in stdout; the proposal is a target of under 90 seconds on CI hardware, with a 20% variance band to account for CI jitter)
- **Report format**: JSON file with timestamp, git commit hash, all metrics above, one boolean per assertion, and a summary exit code (0 = all tasks passed, nonzero = at least one hard failure)

### 1.3 Where It Runs and Cadence

- **Where**: GitHub Actions on push to main and on PR, as a separate job after unit tests; locally on `ordo dev` branch changes when a developer runs it
- **Cadence**: Every main push (CI gate), on demand for branches (`cargo test --test quickstart-walk`)
- **Why**: Catches regressions in the happy path within the hour of commit; developer can verify locally before pushing
- **Hard fail**: Return code nonzero on any task failure, causing CI to fail the check and block merge. A regression that silently works is worse than one that is loud.
- **Recorded regression**: When a task succeeds but takes >2x the baseline time, or when the error message in a wrong-path case changes, record as a regression (print JSON to a file, do not fail the job yet)

### 1.4 What Counts as Success and Failure

**Hard failure** (CI job exits nonzero, blocks merge):
- Task 2 does not boot (no "ordo-host" in output within 30 seconds)
- Task 2 boots but prints no Bearer token
- Task 4 returns HTTP 4xx or 5xx (the create op fails)
- Task 5 returns HTTP 4xx or 5xx, or the rows array is empty
- Any task returns a system error (e.g., connection refused, file not found) instead of an application error

**Soft regression** (recorded in JSON, reported weekly, does not block merge):
- Task 1 compile time increases by 50% without a code change explanation
- Error messages in wrong-path cases lose the field name or the remediation (e.g., "error" instead of "missing field: title")
- Help text or suggestion vanishes (e.g., "try it curl ..." disappears)
- The first-five-minutes time exceeds 120 seconds (2x the target)
- A task that was deterministic now flakes (passes 9 of 10 runs)

**Not a failure** (ignored):
- Time jitter from CI hardware variance (expect ±20% variance between runs)
- Transient network timeouts on GitHub (the CI job is waiting for a package download)
- Database connection reset if retried and succeeds (ephemeral Postgres startup flake)
- Postgres version mismatch warnings in logs (as long as the schema creation works)

## 2. The Human Half

### 2.1 Participant Protocol

Run with a developer who is new to Ordo, in under one hour, in a single session. The moderator is silent except to start each task and to note when a participant asks for help. Participant is asked to:

1. Write a `.ordo` file describing a simple domain (e.g. a todo list with title and done fields)
2. Start the dev server and copy the token it prints
3. Use curl to write and read one item
4. Generate the TypeScript client and verify the imports work
5. Edit the `.ordo` file to add a field and observe the hot reload
6. (Optional if time permits: deploy to a test database)

Moderator records, for each task:
- Task success: completed unassisted within 5 minutes? (yes/no)
- Time to completion in seconds
- Whether they used the token the dev server printed verbatim, or had to search for it or ask for it
- If they got stuck: what they tried (looked at docs, asked moderator, tried a different command, re-read output, gave up)
- Any verbatim quotes when they express confusion ("where do I find this?"), surprise ("oh, it was already printed!"), or satisfaction ("that was obvious")
- What they did outside the designed path (skipped a step, reversed order, tried something not suggested)

Do not offer help until explicitly asked. Do not correct them mid-task. Do offer help if they ask. When you do, note what the help was and whether they needed help on the same problem again.

### 2.2 Cognitive Dimensions Questionnaire

After the task, ask these six questions on a 1-5 scale (strongly disagree to strongly agree). These map to Steven Clarke's dimensions most relevant to a backend platform:

1. **Abstraction level**: "The commands match what I expected to do next." (Domain correspondence: is `ordo dev` the obvious way to run for development?)
2. **Learning style**: "I could predict what the next command should be based on what I had just done." (Role expressiveness: does the output say what to do next?)
3. **Progressive evaluation**: "I knew if my changes worked as soon as I saved the file or ran a command." (Can the developer see if they are on the right path?)
4. **Working framework**: "The error messages told me how to fix the problem." (API elaboration: are refusals actionable?)
5. **Consistency**: "The commands and output format felt consistent across steps." (Consistency: is HTTP routing the same for all ops, is JSON the same shape?)
6. **Viscosity (inverse)**: "I did not have to repeat myself or type the same information multiple times." (Viscosity: how much copy-paste of token, space header, etc., is required?)

### 2.3 Coordination and Participant Selection

Participants should be software engineers (or similar technical depth) who have not used Ordo before but are familiar with backend platforms in general (REST APIs, databases, schemas). Ideal participants are colleagues from other teams, contractors, or users who have expressed interest. Compensate if possible; a gift card or coffee budget goes a long way. Aim for a mix of backgrounds (frontend, backend, DevOps) to catch perspective-specific blindspots.

Schedule sessions at least one week apart so fixes from earlier sessions can land before later ones. This is the point of the human testing: if you run all five sessions in parallel, you cannot act on findings in real time.

### 2.4 Outputs: Work Items, Not Reports

Do not write a summary report. Instead, for each participant session:
- File one GitHub issue per distinct problem they hit: title is what went wrong, body includes task they were on, what they did, time spent, quote from participant if one shows intent
- Mark issues with label `usability-testing` and `severity:(major|minor)` based on whether it blocks all progress or is a rough edge
- Link the issue to 17x in roadmap-checklist.md
- Schedule issues for the next sprint if severity is major, evaluate for future if minor

### 2.5 What Each Half Catches

**Automated suite catches:**
- Regressions in command syntax or output format
- Missing required output (the token when `ordo dev` starts)
- Slow paths (Task 2 taking 5 minutes instead of 1)
- Error handling when inputs are malformed (invalid `.ordo` file, bad URL)
- Consistency across runs (the exact error message when something fails)
- HTTP status codes that change unexpectedly

**Automated suite cannot catch:**
- Confusion about which command to run next (invisible to stdout parsing)
- Surprise at a lack of expected features (no help for "where do I set the database name?" when docs say it defaults)
- Frustration from verbosity or repeated setup steps (typing the token and space header on every curl call)
- Assessment of whether an error message is actually helpful after reading it
- Whether the output feels like a progression toward a goal or a series of disconnected commands
- Eye contact or body language suggesting fatigue or being lost

**Human session catches:**
- Unexpected problem-solving paths ("they tried to edit the token and paste it again" suggests output is not clear)
- Moments of delight or friction when interacting with output ("oh, the token was already printed!" is different from "I have to find the token")
- Calibration of whether 90 seconds to first token feels acceptable or glacial to real developers
- Whether Cognitive Dimensions questionnaire scores align with observed behavior
- Tasks people complete in a different order than designed
- Unplanned assists that reveal unclear instructions

**Human session cannot catch:**
- Regressions that only show up on CI hardware or under load
- Edge cases where the `.ordo` file has a specific structure (deeply nested acl, reserved names)
- Behavior across many commits to spot drift
- Performance profiles across multiple runs

## 3. Metrics and Targets

| Metric | Baseline | Target | Notes |
|--------|----------|--------|-------|
| Task 2 time (to Bearer token) | ~60s | <90s | First-five-minutes checkpoint; includes Postgres setup. Meaningful: actual wait on first try. |
| Task 4 success rate | N/A | 100% | HTTP call with token should work on every run. Meaningful: whether the token works. |
| Task 5 success rate | N/A | 100% | Read back what you wrote. Meaningful: data durability. |
| Error message clarity (human rating) | N/A | 4+ / 5 | Post-session questionnaire Q4. Vanity metric on its own; meaningful if combined with time-to-fix. |
| No assists needed (task success) | N/A | 4 of 6 tasks | Participant completes 4 of 6 tasks unassisted. Meaningful: learning curve on the first hour. |
| Issue count per session | N/A | <3 | Issues filed per participant (hard blockers + rough edges). Vanity if high and all minor. |

**Meaningful targets**: Task 2 time (you can measure and improve the actual bottleneck: is it Postgres boot, the compiler, the gate?). Task 4/5 success (if the core create-and-read path fails on any run, that is the blocker). Issue count (fewer than 3 per session means the flow is learnable; more than 5 suggests a redesign is needed).

**Vanity metrics**: Error message rating alone (without pairing it to "did they fix the problem faster?"). Questionnaire scores above 4 across the board (satisfaction without progress). "Participant reported joy" (memorable but anecdotal and unrepeatable). Completion time for Task 1 (how fast they write the `.ordo` file depends on their typing speed, not the tool's design).

**How to act on a metric**: Task 2 time increases from 60s to 90s: run a profiler and measure the compiler, Postgres startup, and gate phases separately (printed in `ordo dev` output). If Postgres is slow, consider a faster init or a persistent cluster. If the gate is slow, check what it is gating. If the compiler grew, check what changed in the source. Questionnaire scores on Q2 (Learning style) drop from 4 to 2: next human session, ask specifically what made the next step unclear, and record verbatim.

## 4. Increments

### Increment A: Scaffold the automated test (no real developer needed)
- Done when: Tasks 1-3 pass and record measurements to JSON; the test script handles setup and teardown
- Tier: No human; Bash + curl + stdout parsing
- Why first: Establishes the baseline time and error messages; minimal dependencies
- Effort: 1–2 days; one developer; runs locally before CI integration

### Increment B: Complete automated walk-through
- Done when: All Tasks 1-6 pass, record to JSON, CI job runs on main and PR, wrong-path cases are tested
- Tier: No human; expand to cover gen, TypeScript compilation, wrong-path cases
- Why: Full happy path baseline and regression detection on every commit; first-five-minutes time is measured
- Effort: 3–5 days; may require debugging of test harness and Postgres ephemeral cluster resets
- Artifact: `.github/workflows/quickstart-walk.yml` or similar, producing JSON report on every run

### Increment C: Design participant protocol
- Done when: Protocol document exists (task wordings, questionnaire, issue template, moderator script) in docs/
- Tier: No human; design and documentation; may benefit from one review with a colleague
- Why: Ready to run sessions consistently without improvising on the fly
- Effort: 1–2 days; define the exact words used for each task, decide how to handle "help me" requests
- Artifact: `docs/protocol-usability-session.md` with task script, questionnaire, and GitHub issue template

### Increment D: Run five participant sessions (pilot)
- Done when: Five developers (not core team) complete the protocol; raw notes and issues filed for blockers
- Tier: Real people; pilot sample size; each session under 60 minutes
- Why: Identify high-severity friction before broader iteration; test whether the protocol itself is workable
- Effort: 1–2 weeks; finding five willing developers, 5–6 hours of moderation + prep, 10–15 hours writing issues
- Artifact: GitHub issues labeled `usability-testing` and `severity:(major|minor)`, linked to 17x

### Increment E: Act on Increment D findings (prioritized)
- Done when: All `major` issues from D are fixed; re-run two sessions to verify the fix closes the observed gap
- Tier: Development; may span multiple sprints depending on findings (could be a quick docs fix or a command redesign)
- Why: Close the loop so testing drives product improvement; verify your fix actually helps
- Effort: Varies widely by issue; re-runs take 2–4 hours
- Artifact: Closed GitHub issues with a note on which fix addressed them

### Increment F: Quarterly regression reporting
- Done when: Automated metrics are tracked and reported weekly in #random; quarterly summary posted to roadmap
- Tier: No human; CI instrumentation + Slack integration; ongoing cadence
- Why: Sustain the baseline and catch drift; surface trends (is the first-five-minutes time climbing?)
- Effort: 2–3 days for CI integration; then 30 minutes per quarter to write summary
- Artifact: Slack bot posts on every CI run; quarterly snapshot to #announcements and the board

## 5. Success Criteria for This Plan

This plan is done when:

1. **The automated suite runs on CI and does not false-positive.** Tasks 1-6 consistently pass on main and PR builds; the suite reports are not noise. A flaky test that fails 10% of the time is worse than having no test, because it trains developers to ignore the result.

2. **The first-five-minutes time is measured and reported.** A baseline number (seconds from directory creation to Bearer token in stdout) is established and tracked over commits. The team has sight into whether this number is climbing or stable.

3. **Five participant sessions are completed and issues are filed.** Real developers new to Ordo have walked the protocol; high-severity blockers have been surfaced. The sessions produce work items, not a report.

4. **At least one issue from a participant session has been fixed and verified.** The loop closes: a participant hit a problem, you fixed it, a follow-up participant did not hit it. This proves the testing is actionable.

5. **The team agrees the metrics are actionable, not vanity.** A score of 4.2/5 on the "error messages are clear" question is vanity if you do not know what to do with it. A time increase from 60 to 90 seconds is actionable (the Postgres boot is the bottleneck, or the token parsing is hidden).

The plan itself is the work in 17x. Product improvements driven by findings (fixing a confusing error message, speeding up the dev loop, adding a help command) are separate checklist items, each owned by whoever takes the issue. This plan measures the happy path; it does not change it by itself.
