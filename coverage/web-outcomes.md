# Web application outcome ledger

Initial slice, 2026-11-24. This is an incomplete ledger, not a branch percentage.
Every row below remains pending real-stack validation. The inventory still
needs per-handler enum/error and boolean reconciliation. No absence here proves
an outcome unreachable. Source revision and results come from the CI run.

## Implemented journey, awaiting execution

Spec: `examples/marketing-site/e2e/tests/api/stateful-workspace.spec.ts`.
Profile: real two-host marketing deployment, shared Postgres, local blob store.
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

## Required families still to implement against real dependencies

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

Next: add a real-console/real-host fixture with disposable tenants and genuine
deployment-owner passkeys; trace its supported operations before adding missing
controls. Keep console credentials out of public application sessions. Record
fault injection only for fixture processes. Argo is outside this work.
