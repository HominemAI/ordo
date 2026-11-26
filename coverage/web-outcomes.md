# Web application outcome ledger

Updated 2026-11-26. This is an incomplete ledger, not a branch percentage.
Implemented and validated are separate states below. The inventory still
needs per-handler enum/error and boolean reconciliation. No absence here proves
an outcome unreachable. Source revision and results come from the CI run.

## Stateful journey, validated in a failing suite

Spec: `examples/marketing-site/e2e/tests/api/stateful-workspace.spec.ts`.
Profile: real two-host marketing deployment, shared Postgres, local blob store.
This journey passed in CI runs 45387422748 and 45894957979. Other tests in those
runs failed, so neither run is release acceptance.
Actor: authenticated site owner; anonymous caller for private-file refusal.
Output: Playwright JSON plus `stateful-outcomes` attachment, including seed,
actions, candidate writes, response statuses and assertion outcomes.

| Outcome id | Required observation | Downstream source family |
|---|---|---|
| sequence.create | 3 distinct records acknowledged by the application | API binding, executor writes |
| sequence.cas-race | Exactly 1 commit and 1 `409 cas_miss`; both replicas read the winner; control record unchanged | executor CAS and transactions |
| sequence.stale-retry | Retrying the old revision refuses; both replicas retain the acknowledged winner | executor CAS refusal |
| sequence.upload-read | Bytes uploaded through replica B read identically through A and B | host blob, local blob store |
| sequence.anonymous-file-refused | Private handle refuses tokenless reads with `403 access_denied` | blob authorization |
| sequence.delete-cascade | Both replicas show parent and children absent, old handle `404 not_found`, control unchanged | executor owned deletion, blob access |

Repeated outcome IDs represent separate turns. Failure aborts later turns;
missing observations must stay unexecuted. The overall test must pass before
this sequence is release evidence. Coverage remains mixed setup/application.

## Additional journeys implemented

Paths below are relative to `examples/marketing-site/e2e/tests/`, except identity.
All requests reach production binaries and real Postgres. No stand-in door is
used by these new console journeys. A pending row is not coverage evidence.

| Journey | Actual observations required | Validation |
|---|---|---|
| `browser/lifecycle.spec.ts`: tenant | Create, write, policy change/readback, malformed policy, active/paused reap refusal, last-owner refusal, colleague leave, pause/read/write refusal across replicas, resume/data retained, wrong confirmation, delete | Passed in 45897056095 before the colleague-leave extension; that extension is pending |
| `browser/lifecycle.spec.ts`: secrets | Set/replace/version/list/remove, redaction, anonymous refusal, repeated removal | Passed in 45894957979 and 45897056095 |
| `browser/lifecycle.spec.ts`: operations | Live/ready/deep health, metrics, usage/security exports, cost/quotas, forbidden actor and malformed query | Passed in 45897056095 |
| `browser/protocol.spec.ts`: CORS | Real external page, private preflight refusal, public access, allowed origin and credential mode | Passed in 45894957979 and 45897056095 |
| `browser/protocol.spec.ts`: renewals | Concurrent bearer refresh, use all tokens, UI logout, refusal across replicas | Passed in 45894957979 and 45897056095 |
| `browser/protocol.spec.ts`: cookies | HttpOnly/path/SameSite, missing/wrong Origin, rotation, real replay grace and family revocation | Pending |
| `browser/protocol.spec.ts`: live origins | Real embed blocked/allowed/revoked, other tenant remains blocked, wildcard refused | Pending |
| `browser/real-console.spec.ts` | Initial setup, real door link, key rotation, tenant create/pause/resume/delete, real supervised host restart with retained data, unlink/relink, passkey loss/recovery and old-code refusal | Pending. Real door exposed the console's missing idempotency header in 45897056095; fixed with a keyed replay regression |
| `browser/portability.spec.ts` | UI selects tenant, exports bundle, operator stages it, UI imports into another tenant, compares typed values, retries import, refuses invalid bundle names | Pending. Operator file staging is explicit; there is no claimed browser bundle-transfer endpoint |
| `browser/volume.spec.ts` | 257 records from 3 writers, browser filter sees all records, replica B deletion arrives live, cleanup | Passed in 45897056095. This is a data-size check, not a latency SLO |
| Identity service `e2e/tests/identity.spec.ts` | Passkey CLI approval/denial, redemption/reuse, session-to-token exchange, Ed25519 machine authentication, invalid proof/replay/mint refusal and revocation | Pending. Run 45896467506 caught the missing session-to-token exchange; corrected |

## Required outcomes still missing or awaiting further journeys

The implementations above do not complete their entire families. The table
below remains the checklist for success/refusal/recovery outcomes, including
those not reached by the implemented slices.

Each row groups work, not a claim of exhaustive branch enumeration.

| Family / slice | Success outcomes | Refusal / recovery outcomes | Source anchors |
|---|---|---|---|
| Tenant creation W2 | Authorized create; configured self-service; read/write in new tenant | Self-service disabled, foreign tenant, partial provision and cleanup | host tenant_ops.rs, self_serve.rs |
| Tenant pause W2/W5 | Tenant-authorized and deployment-owner pause/resume; data preserved; other tenant works | Default role refusal, paused writes/blobs refuse, tenant cannot lift deployment pause | host pause.rs |
| Tenant exit W3/W5 | Leave, crypto-erasure/delete; reap failed provisioning; repeat reap | Last-owner rule, wrong actor, active/paused reap refusal, failed cleanup resumes | host release_ops.rs, exit_ops.rs, reap_op.rs |
| Console bootstrap W5 | First setup, invite/remove owner, passkey recovery and step-up | Reused/expired recovery, removed owner, wrong passkey | console server.rs |
| Console trust W5 | Link real door, rotate key, unlink/forget/relink | Old key refused, door unavailable, partial link failure | console server.rs, host deployment_access.rs |
| Console runtime W5/W6 | Restart/apply/rollback actual host, reconnect and read persisted data | Unavailable supervisor, failed activation, recovery | console server.rs, supervisor |
| Machine credentials W2 | Browser approval, redemption/use, rotation/revocation | Rejected/expired/reused approval, wrong audience/scope, human-presence action refused | identity-service routes.rs, host deployment_access.rs |
| Browser security W2/W6 | Actual allowed preflight/fetch; cookie renewal | Disallowed origin, cookie scope, missing Origin, expired refresh, simultaneous renewals | API cors.rs, host auth |
| Live policy W2/W6 | Change session/provider/origin/self-service policy while session/feed exists | Effective restriction across replicas; retired provider and secret refused | host session_policy.rs, tenant_providers.rs, tenant_origins.rs |
| Secret lifecycle W4 | Set/list/replace/remove; real consumer observes allowed changes | Listing redacts value; missing/foreign/removed secret refuses | host secrets.rs, function and egress |
| Operational APIs W5/W6 | Usage/cost/health/security reads; empty/populated/paged; quota change | Wrong actor/tenant, dependency outage, no secret/tenant leak | host serve.rs, observed_cost.rs, security_record.rs |
| Long sequences W6 | Repeated share/edit/upload/revoke/delete/rejoin; large dataset and reconnect | In-flight revocation, slow consumer, concurrent owners; independent tenant preserved | application and downstream handlers |

## Evidence exclusions and next harness work

The existing console tenant and usage browser specs use stand-in door answers
(`tenants_door.rs`, `usage_door.rs`); its runtime suite supervises a fake host.
Those suites can validate console behavior, but cannot satisfy the real tenant,
executor, storage or runtime outcomes above. Connect the console to the real
host before crediting those paths. Do not replace these useful UI checks or
silently label their fixtures as end-to-end production execution.

The marketing fixture now connects the console to real hosts and a real
supervisor. Its new journey must pass before those paths receive credit.
Remaining work includes owner invitation/removal, failed activation and rollback,
provider lifecycle, successful reap, delivery consumers and fault profiles.
Argo is outside this work.

Raw LLVM JSON, toolchain details and source/run provenance accompany LCOV.
The phase remains `mixed-setup-and-application`; branch instrumentation is not
enabled. Runtime registration reconciliation, executable-range review and
setup separation are still required before publishing a web-only percentage.

## Storage extension, awaiting execution

`browser/storage.spec.ts` uses the Data panel and supported attachment APIs
against a real SeaweedFS S3 service. It requires acknowledged local and S3
files to remain readable across both replicas, a retry to report unchanged,
a missing credential to refuse, and object-store outage/recovery to preserve
files and unrelated application access. A local run without this fixture is
explicitly skipped; full CI requires it. No hits or outcomes are credited yet.

## Provider configuration extension, awaiting execution

`browser/connections.spec.ts` drives the Team panel's OIDC connection controls.
It adds, rotates, lists, retires and re-adds a connection, checks secret
redaction and rejects issuer mutation, duplicate registration, anonymous
listing, foreign-tenant sign-in and retired-provider sign-in. Its unused issuer
is not a working provider fixture. Discovery/JWKS, successful consent and
callback, provider outage and key rotation still need their own real-provider
journeys. No successful federation outcome is credited by this test.

## Scope direction

Owner clarification, 2026-11-26: subsequent implementation is restricted to
examples, tests and coverage/CI support. Product defects are recorded in
`web-findings.md`, not fixed here. The 2 earlier production fixes remain in the
PR; no further runtime changes are authorized by this work.

## Function file inspection extension, awaiting execution

The campaign detail page calls `inspectCampaignFile` once per explicitly
supplied blob, at most 32 files. The actual function reads the campaign and
its file ownership before reading bytes through `ctx.blob`. The UI reports
metadata and byte count, never file content. `api/function-files.spec.ts`
checks success, mismatched campaign refusal, anonymous refusal and actual
function-run records, including failure records and missing-run refusal.
The browser upload journey also invokes inspection. No runtime hits are
credited until execution. Local HTTPS receiver trust remains a source finding
in `web-findings.md`, not an implemented production change.

## Maintenance extension, awaiting execution

A separate real-console browser journey creates a one-off window, reloads to
verify persistence, refuses an inverted range, creates and edits a recurring
window, adds/removes its skip, and deletes only its own windows. Requests go
through the existing console UI to the actual host. Window configuration alone
does not prove that a scheduled deployment or promotion ran. No such outcome
is credited here.
