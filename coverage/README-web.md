# Coverage from application use

This is the first W0 implementation slice in `docs/plan-example-tests.md`.
It discovers source candidates and validates scoped coverage accounting.
It does **not** yet establish a web-only baseline or complete runtime discovery.

## Source inventory

```sh
node scripts/web-coverage-inventory.mjs write
node scripts/web-coverage-inventory.mjs check
node --test scripts/web-coverage.test.mjs
```

`web-inventory.json` records Rust source hashes, build-input hashes, route
expressions and public `std.*` string constants. Dynamic paths are preserved.
Comments and string examples are ignored. Source-embedded test routers and
constants that are not actually registered can remain as candidates. Other
registration forms, generated operations and mounted prefixes need runtime
and call-site review; this scanner is not a Rust parser or a call graph.

`web-scope.json` records source reviews and classified line ranges. The first
6 reviews cover the generic op, live, blob-upload and function declarations.
They do not establish runtime availability. All measured lines initially stay
unresolved, with no exclusions. A range requires its source hash, reviewer and
evidence. Overlaps, stale hashes, unknown files and out-of-bounds ranges fail.

A source change requires inventory regeneration and review of affected mappings.
Do not copy the new digest onto old range hashes or silently discard exclusions.
The checks intentionally fail until the recorded source and review agree.

## Application-run evidence

The reporter accepts only run receipts that identify web-only journey profiles:

```sh
node scripts/web-coverage-report.mjs \
  coverage/web-scope.json coverage-out/web.json coverage-out/run.json
```

A receipt has this shape (replace the placeholders with actual values):

```json
{
  "version": 1,
  "sourceDigest": "digest from the matching source inventory",
  "kind": "web-journey",
  "phase": "web-only",
  "scenario": "staff edits a product while a second browser reads it",
  "profile": "two-hosts-local-storage",
  "result": "passed",
  "separationEvidence": "Describe the actual profile boundary and link the run log",
  "artifacts": [
    { "kind": "lcov", "path": "web.info", "sha256": "artifact SHA-256" },
    { "kind": "runtime-catalog", "path": "catalog.json", "sha256": "artifact SHA-256" }
  ]
}
```

Paths are relative to the receipt. Keep credentials, headers and tokens out of
artifacts. A receipt is evidence supplied by the harness, not cryptographic proof
of how a process ran. Checksums establish artifact integrity only. The reporter
retains the catalogue; runtime reconciliation is still pending. The host's
`/interface` is caller-filtered and does not by itself enumerate every platform
operation or auth/console route. Capture the actor/configuration and supplement
it with registered-spec evidence and actual requests.

Provisioning and app profiles must be collected separately by the real harness.
Do not manufacture a receipt around an existing mixed LCOV file. Starting a
host before a test also executes boot paths: profile-directory separation alone
does not remove counters already accumulated in a long-running process. A real
reset/dump boundary or a proven differential collection method remains necessary.
No existing example report is reclassified as web-only by this change.

## What the output means

- `raw` uses the existing executable-line union accounting.
- `groups` separates reviewed reachable, unresolved and explicitly excluded lines.
- `conservativeMeasured` includes unresolved lines, but does not include unknown
  executable lines in unmeasured files. It is not a complete web denominator.
- `gaps` lists missing and unresolved counts per measured source file.
- `unmeasuredFiles` and `unreviewedEntries` prevent absence from becoming an exclusion.
- A hit in an excluded range fails: the exclusion contradicts observed usage.
- `webPercent` stays null pending runtime and outcome reconciliation.
- `branchCoverage` is explicitly unmeasured. LCOV lines do not establish branches.
- `acceptance` stays incomplete, including when every supplied journey passed.

The Node tests guard this accounting; their hits never contribute to the app
coverage number. Existing product refusals and failures remain unchanged.

## Next W0 work

1. Reconcile registrations across host, identity, console and configuration
   profiles. Resolve dynamic/generated entries and source-only false positives.
2. Review entry contracts, caller chains and worker completion for each family.
3. Implement and prove the profiling boundary in the real stack, including all
   replicas and workers. Retain build configuration and process/binary identities.
4. Collect runtime snapshots, actual request outcomes and raw LLVM JSON from CI.
5. Check branch instrumentation support on the pinned toolchain, then connect
   the outcome ledger and report missing outcomes independently of line hits.
6. Establish the reproducible baseline before W1 or W3 is credited with a gain.

## Outcome ledger

`web-outcomes.md` tracks the newly identified journey families independently
of line coverage. A linked spec means implemented, not passed. CI's Playwright
JSON and `stateful-outcomes` attachment contain observed outcomes; the source
ledger never promotes a pending journey merely because a test exists.

The first stateful journey runs 8 races across 2 real host replicas, retains
one unaffected campaign, retries stale revisions, uploads and reads file bytes,
then deletes the parent and checks cascade and access on both replicas. Replay
its data/action selection with `ORDO_JOURNEY_SEED=<uint32>` and run:

```sh
cd examples/marketing-site/e2e
npx playwright test tests/api/stateful-workspace.spec.ts --project=api
```

This starts the real stack and requires built binaries and Postgres. In this
shared development session, run it in CI rather than building Rust alongside
Claude. The seed reproduces action selection, not network completion order.
The artifact records that order through response statuses, plus revisions and
candidate values. It contains no credentials. Missing outcomes are unexecuted,
not successes. Passing observations before a later failure do not pass the
whole journey. This bounded sequence is not the large-data or capacity profile.
