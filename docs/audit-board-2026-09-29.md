# Audit of the decisions board, 2026-09-29

The owner asked for a full audit: *"Things that are not built but are decided and still need to be built should have queued - some things say done / decided but it doesnt tell me anything of the status of the work. for each item i want to also store where that work was completed - or if it was just a decision/question that did not generate work."*

## Method

- Every task was checked against the code and the git history, not against the board's own words.
- Two reviewers took half each. Neither changed the repository.
- Every commit and file they named was then tested mechanically: 230 commits and 361 paths, all found.
- Each task now carries `work` (where the work stands) and `where` (where it was done). Read them with `python3 scripts/board.py get <id> work where`.

## Where the work stands

The 219 tasks the audit covered. The task `audit-decisions`, added by the audit, is not counted.

| Work | Tasks |
|---|---|
| Built | 139 |
| Building: started, with a remainder | 26 |
| Queued: decided and not started | 17 |
| Blocked | 5 |
| Waiting on the owner | 4 |
| Filed: no go given | 20 |
| Decision only: no work came of it | 8 |
| **All** | **219** |

## Statuses the code contradicted (28)

Each was corrected on the board.

| Task | What was found |
|---|---|
| `tenant-storage` | Says Gathering requirements; requirements were delivered 2026-09-21 and seven of eleven increments are built. Should read Building: U0 to U6. |
| `triggers` | Says delivery loop live. Only tests supply a TriggerWiring; in a deployed binary triggers.drain, triggers.batch and triggers.runner are all disabled until 12a lands. Should read Built in library, disabled in the binary. |
| `corpus` | Says Go: queued; both halves of the go are on main. Should read Built. |
| `ci-images` | Says Go: queued; most of it is built and running. Should read Building: release run owed. |
| `sharing` | Says Go: queued; the library half was built 2026-09-13. Should read Building. |
| `provisioning` | Says Queued; the lifecycle is built. Should read Building. |
| `secevent-key-rotation` | Says Ruled; it was built the same day. Should read Built. |
| `dotpath-guard-semantics` | Says Ruled; the fix is on main. Should read Built. |
| `first-program` | Says Ruled; the app is owed and cannot ship until passkey sign-in and 14g land. Should read Blocked. |
| `production-gaps` | Says Filed, and the sub-line says none started; 20a and 20b are well under way. Should read Building. |
| `revocable-cache` | Says Filed; the owner ruled build it (2026-09-12) and half is built. Should read Blocked on sharing registration. |
| `db-lag-bound` | Ruled hides that none of it is built; should read Queued |
| `db-behind-schema` | Ruled; should read Building (default only) |
| `tenant-storage-req` | Ruled; should read Building |
| `st-delete-objects` | Ruled; should read Queued |
| `st-delete-objects-2` | Ruled; should read Queued (17ab) |
| `st-bucket-policy` | Ruled; should read Building |
| `st-gc-cadence` | Ruled; should read Queued (U8) |
| `st-attach-existing` | Ruled; should read Building (exception not built) |
| `db-reserve` | Ruled; should read Building (per-target setting not built) |
| `cli-audit` | Ruled; should read Building |
| `door-only` | Ruled; should read Building (17z scenarios 3 to 9 open) |
| `feed-id` | Ruled; should say superseded by feed-id-uigen |
| `opslog-role-split` | Ruled; should read Queued |
| `opslog` | Ruled; should read Building |
| `backfill-audit` | Ruled; should read Queued (13c) |
| `breakglass` | Ruled; should read Building (grant-gated operation not built) |
| `measurements` | Done; should read Blocked (measurements wait on a deployment) |

## Builds that rulings called for and nothing tracked

Filed as checklist section 22.

| Item | What is owed | Task |
|---|---|---|
| 22a | The per-tenant lag contract | `db-behind-schema, db-lag-bound` |
| 22b | Erasing a space leaves the objects in a bucket the tenant owns | `st-delete-objects` |
| 22c | The exception for attaching a bucket that holds the tenant's own objects | `st-attach-existing` |
| 22d | Publishing the bucket policy in an operator document | `st-bucket-policy` |
| 22e | A connection reserve per target | `db-reserve` |
| 22f | A role that reads the ops log and cannot prune it | `opslog-role-split` |
| 22g | The modelling guidance on routing | `sharing-under-routing` |
| 22h | Commands to add, suspend and remove a member | `membership-commands` |
| 22i | A refinement that can reach the door | `effective-cost` |
| 22j | Four small things recorded only in prose | `many-deployments, deployment-scope, door-only, blob-store` |

## Put to the owner

Five questions are on the board as the task `audit-decisions`.

## Queued, building and blocked, with what is owed

### Queued: decided and not started

- **`erasure-receipt`** (17ab). Owed: the erasure receipt and a credential read through the provider that survives rotation, both specified in checklist 17ab (open) and corpus-rulings.md 2026-09-21 (9ea4dcf). No code for either exists in crates/.
- **`usability-testing`** (17x). The plan is written, docs/plan-usability-testing.md (737a219, 8f44c6c), and the owner took the recommendation. None of it is built: no automated quickstart walk exists under crates/*/tests and no sessions are recorded. Checklist 17x open.
- **`triggers-deregister`** (new). Owed: the operator command ordo triggers deregister that records who retired a listener. The library call prune::deregister_consumer exists (crates/ordo-exec/src/prune.rs) with no command in front of it; 17d's entry lists the command as still open.
- **`vault`** (5g). Owed: all of F7, nine phases in docs/f7-vault-plan.md. Checklist 5g is [~] planned, not started; no vault code in crates/. Its prerequisite 5g-pre (the client cache) is built.
- **`vault-sync`** (18h). Owed: form 2 sync through the F7 oplog, recorded in docs/f7-vault-plan.md 5.4. Checklist 18h open, no code. Waits on the vault itself (5g).
- **`delegation`** (11-4a, 11-4b, 11-4c, 11-4d / 17h). The core grant and door are built (11-4 closed: crates/ordo-identity/src/delegation.rs, crates/ordo-host/src/delegation.rs, 9c2094f). Owed: increment 2, $caller rebinding (11-4a, 17h), then 11-4b to 11-4d, per docs/plan-delegation-surface.md. Not started.
- **`sec6-transplant`** (21c). Owed: bind each seal to its row, planned in docs/plan-row-bound-seals.md (f752292). Checklist 21c open, not built.
- **`sec6-local-master`** (21e). Owed: ORDO_KEY_PROVIDER=local refusing outside development; crates/ordo-host/src/runtime.rs still accepts local with no such check. Ruling in docs/corpus-rulings.md (section 3, 2026-09-28); checklist 21e open.
- **`cors-at-the-door`** (14g). Owed: a configurable CORS allowlist at the door, settable from the console. No CORS code in crates/; checklist 14g open.
- **`db-lag-bound`** (18d · D-B2 · 22a). Nothing built. Owed: serve a contracted tenant one version back, requested snapshots, and a step-by-step down path per schema change. Ruling in docs/corpus-rulings.md (D-B2 settled, 2026-09-21).
- **`st-delete-objects`** (18p · D-S5 · 22b). Not built. crates/ordo-exec/src/blobgc.rs erase_space still deletes every object whatever the custody. Ruling in docs/corpus-rulings.md (Erasure proof..., 2026-09-21).
- **`st-delete-objects-2`** (18p · D-S5). Owed as checklist 17ab (erasure receipt), open; no receipt code in crates/. Ruling in docs/corpus-rulings.md (Erasure proof..., 2026-09-21).
- **`st-gc-cadence`** (18p · D-S3). Owed as U8 in docs/plan-tenant-storage.md (18p). blob.gc is still a disabled Slot in crates/ordo-host/src/workers.rs.
- **`cli-door-table`** (17y · 2). Not started. rollback, deploys, migrate --apply, quota, usage, restore --record still take database credentials; crates/ordo-cli/src/wire.rs has only apply, space create and import. Started table at tag salvage/cli-door-table (17y item 2).
- **`opslog-role-split`** (plan-opslog §2.3 · 22f). Nothing built: no deployment-log reader (plan-opslog D5, ordo seclog) and no read-only role (R4). Residual recorded in docs/plan-opslog.md Q-2; checklist 11-22 open.
- **`sharing-under-routing`** (plan-space-routing · 22g). Owed: docs/modeling-guidance.md must say windows cost more than feeds under routing; it does not mention routing. Ruling in docs/corpus-rulings.md. Checklist 22g.
- **`backfill-audit`** (13c). Not built: $migrations has no backfilling phase in use and no chunked backfill (crates/ordo-exec/src/migrate.rs). Owed as checklist 13c. Ruling: docs/corpus-rulings.md 2026-09-08.

### Building: started, with a remainder

- **`tenant-storage`** (18d / 17g). U0 to U6 of docs/plan-tenant-storage.md are built: 408c5ad (U0, U1), 6a1c891 (U2), 5762f44 (U3), 52c32d5 (U4), e06f69b (U5), 6e15764 (U6, tests/attach_bucket.rs). Still owed under 18p: U7 failure modes (did not ship with T5), U8 sweep runner, U9 usage, U10 the record.
- **`live-instance`** (env / 15a). The instance runs at ordo.inf.smg.icu from the external smg_infrastructure/ordo repo, v0.1.0 is tagged, and docs/deploying.md and docs/go-live.html describe it. Owed: a program deployed to it, a tenant with custody and authentication switched on (docs/go-live.html, 17ae).
- **`noisy-neighbours`** (20a). Built 2026-09-20 (930be66, 150a2af): per-space pool share and statement bound in crates/ordo-exec/src/lib.rs, attribution in crates/ordo-host/src/metrics.rs, tests/noisy_neighbour.rs, docs/deploying.md 2a. Owed under 20a: a fair queue across tenants, an index-aware cost gate, and the observer on AttachedPools.
- **`bulk-import`** (11-10a). Engine crates/ordo-exec/src/import.rs (v1 2026-09-20), blob leg 2fd819b (tests/import_blobs.rs), door std.space.import and ordo import d8e941c (tests/import_op.rs), people map 40645ac. Owed under 11-10a: delivering a bundle to a hosted deployment, and the foreign-system mapping file.
- **`schema-evolution`** (13a, 13b, 13c, 13d, 13e / 17e). Increments 1 and 2 built: 13a (b757536, the $migrations record) and 13b, crates/ordo-host/src/schema_apply.rs (acc725e). Owed: 13c barrier and backfill (blocked on plan section 9), 13d sealed backfills, 13e destructive apply, per docs/plan-schema-evolution.md.
- **`triggers`** (§7). The loop code is built: crates/ordo-exec/src/triggers.rs, workers in crates/ordo-host/src/workers.rs, exercised by crates/ordo-host/tests/supervisor.rs and ordo-exec/tests/triggers.rs. The shipped binary supplies no listeners and no handler (TriggerWiring::default in runtime.rs), so every trigger worker boots disabled. Owed: 12a.
- **`effective-cost`** (audit · 22i). Checklist 11-8 (closed): ordo_plan::cost refine_candidate_bound in crates/ordo-plan/src/cost.rs, tests crates/ordo-plan/tests/refine_cost.rs and crates/ordo-host/tests/cost_class.rs. The gate is idle: no op can be marked refinable and the executor ignores $refine (11-8 remainders 1 and 3).
- **`secevent`** (7d / 17m). 7d(a) verifier ecddae6, 7d(b) route /security/events 9b07bd6 (crates/ordo-host/src/serve.rs), signer 65cdc7c (crates/ordo-exec/src/secevent.rs). Owed: 7d(c) the dashboard explorer; 17m open.
- **`ci-images`** (15a). CI green and gate drill passed: .github/workflows/ci.yml, images.yml, gate-drill.yml (321e3e1). Owed: a successful v* release run; gh shows the v0.1.0 release runs cancelled or failed and no release published (15a).
- **`sharing`** (11-2 / 11-3 / 17q). register_share exists in crates/ordo-identity/src/grants.rs with unit tests (73492f5). Owed: nothing in the host or CLI calls it, so no one can register a share; checklist 11-2 and 17q still open.
- **`attestation`** (11-12). Layers 2 to 4 built: crates/ordo-egress, crates/ordo-verify-proof, chainProof; one witness format shipped. Owed: four witness formats, checklist 11-12 is [~].
- **`provisioning`** (11-15). Built: state machine and std.space.reap c0c8e11, reconciler 65435ab, phase 0 04fd5e1, item 44 b962dfb, item 22 bbd43fb (crates/ordo-host/src/tenants.rs, crates/ordo-exec/src/spacedb.rs). Owed: the remaining docs/bootstrap.md inventory items; 11-15 open.
- **`tenancy-topology`** (18c / 18d). S1 to S7 on main (55e7af1 S6 core, 448bc08 S7), S8 record b1cbab1. Owed: S6b, migrations reaching every attached database, and moving a live space (18e, post-release). 18c and 18d open.
- **`sec6-import`** (21d). Duplicate keys refused in a77cf1a (crates/ordo-exec/src/import.rs, tests crates/ordo-exec/tests/import.rs). Owed under 21d: fingerprint the values not only ids (S6-006), cap what is read (S6-008), refuse symlinks in staging (S6-011).
- **`production-gaps`** (20a–20f). 20a mostly built (noisy-neighbours), 20b partly built (durable-auth; sign-in still owed), 20c partly answered by ordo import (11-10a) and dcd0af9. 20d change feed, 20e search, 20f collected: not started.
- **`db-behind-schema`** (18d · D-B · 22a). Default quarantine built: crates/ordo-host/src/spaces.rs Reconciled::SchemaBehind (94dcb1b). The per-tenant contract that serves the older program is not built.
- **`tenant-storage-req`** (18p). U0 to U6 built: 408c5ad, 6a1c891, 5762f44, 52c32d5, e06f69b, c810149. U7 (failure modes), U8 (sweep runner), U9 (usage/operator view), U10 (record) owed per docs/plan-tenant-storage.md section 11.
- **`st-bucket-policy`** (18p · D-S7 · 22d). Head-only probe built in U6 (crates/ordo-host/src/bucket_ops.rs, c810149). The policy itself is only in docs/plan-tenant-storage.md B-X2; no operator document publishes it.
- **`st-attach-existing`** (18p · D-S2 · 22c). Refusal built in U6: bucket.rs objects_held, checked in crates/ordo-host/src/bucket_ops.rs (c810149). The one-for-one exception is not built; its shape (a head per $blobref row) is in docs/corpus-rulings.md 2026-09-21.
- **`db-reserve`** (18d · D-C · 22e). Unconditional reserve built: ORDO_PG_SPACE_RESERVE via PoolLimits in crates/ordo-exec/src/lib.rs. One deployment-wide value; no per-target setting exists.
- **`topology`** (18o / 18c). Deployment and tenant clusters configurable: ORDO_PG_* and ORDO_PG_TENANT_* (dcd4579), routing wired via T4 (2a42d00). The console with its own cluster and the one-pod release are filed under 18o.
- **`cli-audit`** (17y). Finding 1 built: std.space.import and ordo import over the door (d8e941c), crates/ordo-cli/src/wire.rs. Findings 2 and 4 to 8 open in checklist 17y.
- **`door-only`** (17z). The circularity ruling is built as 17aa pieces 1 and 2 (ee5a37e). Scenarios 3 to 9 (untrusted-server verify, operator lockout, long ops, large payloads, version skew, limits) stay open in checklist 17z.
- **`replica-engine`** (5g-pre / plan-client-cache §8). Local refinement built as C6 (db6e1ca). PGlite waits for F7, checklist 5g (planned, not started).
- **`opslog`** (11-22). Own artifact by default exists: deployment scope routed, chained, retained (D1+D2 6d453ca, S4 d2e0adb). The off toggle (plan-opslog D3) and the reader (D5) are not built. Checklist 11-22 open.
- **`breakglass`** (11-26). Five comments corrected (e53122a); ruling in docs/corpus-rulings.md. The grant-key-gated operation is not built; checklist 11-26 open.

### Blocked

- **`auth-bootstrap`** (20b). Built: password sign-in and durable sessions (20b); the passkey checker crates/ordo-identity/src/passkey.rs exists but the server refuses to wire it. Diagnosis db1192b. Owed: enrolment, passkey sign-in, recovery, federated, per the three plans.
- **`membership-commands`** (21b · 22h). Nothing built: no command adds, suspends or removes a member. Planned in docs/plan-enrolment.md; waits on enrolment-questions. Enforcement of a status change already exists (20f1b2f).
- **`first-program`** (17ae). The notes app is not written; examples/first-app is a readings app (9231cbd). It waits on passkey sign-in being served and CORS (14g). Ruling in docs/corpus-rulings.md section 5; checklist 17ae open.
- **`revocable-cache`** (5g-rev). Purge on revocation shipped with C7 (d4e756a, clients/ordo-sdk). Owed: the server-side content-key release for a shared window; it needs share registration (11-2) first. Checklist 5g-rev open.
- **`measurements`** (5f). Mechanisms built (5f: f294459, 540c29a). The three measurements wait on a live deployment; checklist 5f is [~].

## Notes from the reviewers

Things a task's two fields could not hold. Those marked fixed were corrected in the same commit as this document.

- `st-default-tier`: The ruling names four bucket levels (console, deployment, tenant default, per-tenant). There is no tenant-default bucket setting: unattached tenants use the deployment's ORDO_BLOB_STORE, the storage twin of level 3 that the database side got as ORDO_PG_TENANT_HOST.
- `db-default-tier`: Overlaps db-always-a-database; the two tasks describe one build.
- `actions-billing`: .github/workflows/gate-drill.yml still runs on ubuntu-latest (GitHub-hosted), so the gate drill would still hit the billing refusal.
- `live-instance`: The deployment itself lives outside this repo, so it cannot be verified from here.
- `platform-ref-import` (fixed): The ruling is recorded in plan-import.md and the 11-10a checklist entry; a grep of docs/corpus-rulings.md finds no section for it, which AGENTS.md rule 6 asks for.
- `durable-auth`: plan-passkey-recovery.md found enrolled passkey public keys are still held in process memory, so passkey credentials do not survive a restart; 20b stays open.
- `effective-cost` (fixed): The gate is built but idle: no op-level refinable marker exists, so no refinement reaches the door, and the executor ignores $refine (11-8 remainders 1 and 3, 5c). Those remainders are unfiled as their own board task.
- `vault`: The task list shows 5g as checked; in the checklist it is [~], planned.
- `runtime`: 17b stays open: SMK rotation and re-seal (11-11, filed as 18j), any KMS binding, and custody selection for ordo revert are still owed.
- `sharing`: The checklist 11-2 text still says registration is owed and does not mention 73492f5.
- `export`: The bundle declared blob bytes as waiting on blob custody; not checked whether blobs are exported now that 17g landed.
- `delegation`: Until 11-4a lands, a delegated read over a $caller-guarded entity returns nothing.
- `blob-chain` (fixed): The sub-line still says C7 and C8 remain; they landed in f62be1c.
- `blob-store`: A local-volume store was named as a later addition, never the default; it is not filed anywhere I found.
- `attestation`: The task list shows 11-12 as checked; the checklist marks it [~] with four formats owed.
- `bootstrap-inventory-split` (fixed): Rule 6 asks for rulings in corpus-rulings.md too; this one is missing there.
- `ships-a-client` (fixed): 110 of 165 ops are still not syncable; the sub-line counts 55.
- `auth-bootstrap`: Passkey sign-in wiring could start now without the answers; it has not.
- `membership-commands` (fixed): The task list shows 21b as checked; 21b covers enforcement, and the commands are not tracked by an open checklist item I found.
- `sec6-membership-snapshot`: The suspend command itself is owed under membership-commands.
- `sec6-notes`: Nothing to build unless the splice is replaced; this may be closer to none.
- `leaked-test-server`: 17ad also records the pool-size root cause fixed 2026-09-25 (ci-max-connections); that fix does not stop the leak.
- `post-deploy-filed` (fixed): The sub-line names four items; the bundle holds six, one of them built.
- `lean-ledger` (fixed): The checklist shows fourteen open 19x items, while the sub-line says eleven.
- `production-gaps`: 20c's checklist text still says ordo import is unbuilt.
- `revocable-cache`: The task list shows 5g as checked; the relevant item is 5g-rev, which is open.
- `whole`: Nothing in the host serves whole yet, per a77cf1a.
- `custody`: Production custody (17b) still owes SMK rotation and any KMS binding.
- `sealed`: Seals are not yet bound to their row (21c, sec6-transplant).
- `pinning`: Checklist 8q is marked [~], not [x]; the task list shows it checked.
- `db-lag-bound`: 18d says the contract-lag increment 'is not in T0 to T10' and no checklist item or increment carries it. The step-down half also depends on 11-10b (snapshot-restore, Filed).
- `db-behind-schema`: the per-tenant lag contract (second loaded artifact, per-tenant resolver) has no checklist item; 18d prose says it is outside T0 to T10.
- `tenant-storage-req`: None of U7's or U8's named tests exist in crates/.
- `st-delete-objects`: no checklist item carries 'leave the objects under OwnPerimeter and write a custody entry'. 17ab item 1 only assumes it (receipt carries the count left).
- `st-bucket-policy`: publishing the least-privilege policy (PutObject/GetObject/DeleteObject, no ListBucket) in howto-custody.md or deploying.md has no item.
- `st-gc-cadence`: The Slot reason in workers.rs still says the cadence is 'an operator decision nobody has made', which the 2026-09-21 ruling made false.
- `st-attach-existing`: the exception has no increment or checklist item; 18p only flags it for the owner.
- `db-credential-provider` (fixed): The sub-line still says 'KMS first'; which-kms dissolved that the same day. The row title (mounted directory, no KMS) is what was built.
- `db-chain-attach`: 18p flags an open reading for the owner: U6 still writes the attach to the tenant's security stream (B-X6), which D-F's rationale may also cancel.
- `db-reserve`: corpus-rulings.md says the reserve 'is set once per target'; the per-target defaults (plan's ORDO_PG_TARGET_*) do not exist and no item tracks them. Owner may consider the global knob enough.
- `tenant-db-requirements`: 18d stays open only for the D-B contract lag (untracked, see db-behind-schema) and 18e.
- `topology` (fixed): The sub-line 'per-tenant databases are ... not wired into the binary' is stale: T4 and T8 wired them (2a42d00, d257e2a).
- `many-deployments`: The deployment-id prerequisite was to be added 'when each table is next touched'; nothing tracks it beyond 18o prose.
- `door-only`: The break-glass connection string the owner described in 17z has no build item.
- `recovery-acts`: 17z itself stays open for its other scenarios.
- `rotation-objects` (fixed): The disclosure tells operators to 'run blobgc deliberately', but nothing runs blobgc: blob.gc is a disabled Slot in crates/ordo-host/src/workers.rs and no CLI calls it. A rotated space with blobs can never reach destroyable until U8 lands.
- `r4-images`: Release binaries were later ruled linux/x64 only (release.yml, 2026-09-25); images.yml still builds arm64 on tags. Probably intended, worth a glance.
- `refine-tier-local`: Ruling says server Tier 0 is 'scheduled with the refinement feature'; the only open item carrying it is 16d, which has no go.
- `feed-cap`: The page wait default is 'not yet measured' per the plan.
- `opslog-role-split`: the deployment-level role that reads and cannot prune has no checklist item; plan-opslog calls it 'filed rather than solved' with nothing to build on.
- `per-tenant-install`: The owner's answer ('tenants should be able to select between versions') asks for a build; 20f calls it 'not work at all until someone decides to pay for a redesign'. Those two disagree and no item carries it as work.
- `sharing-under-routing`: corpus-rulings says the modelling guidance 'has to say' windows cost more than feeds under routing; docs/modeling-guidance.md does not mention routing.
- `opslog`: D3's named test the_default_is_on_with_no_variable_set does not exist in crates/.
- `backfill-audit`: 13c's own text still says 'Blocked on section 9', which this ruling unblocked.
- `breakglass`: 11-26 says the grant-gated operation is 'unclaimed by any tracked row' on the board.
- `deployment-scope`: The CapabilityGrant follow-up has no owner: 20f notes AccessEngine::grant_for has no caller and no op mints a grant.
- `rotation`: A space holding blobs cannot reach destroyable until blobgc has a runner (U8); see rotation-objects.
- `decompile`: The two defect fixes named in the sub-line were not traced to a commit.
- `idempotency`: Checklist 8q is marked [~] although its title says both halves built.
- `std-library`: Checklist 9k is [~]: five verbs are not derived, each with a recorded reason (lean, open cap, or no log source).

## Not fixed here

- Checklist text that lags the code: 11-2 does not mention share registration (73492f5), 20c still says `ordo import` is unbuilt, 13c still says it is blocked on section 9, and 8q and 9k are marked partial while built.
- The `blob.gc` slot in `crates/ordo-host/src/workers.rs` still says its cadence is undecided. It was ruled on 2026-09-21.
- `.github/workflows/gate-drill.yml` still runs on a GitHub-hosted runner.
