# plan-versions.md: merged review

Merged 2026-11-02 from 6 reviews of `docs/plan-versions.md`.

Sources:

| Tag | File |
|---|---|
| R1-use | review-versions-usability.md |
| R1-flex | review-versions-flexibility.md |
| R1-sec | review-versions-security.md |
| R2-use | round2-usability.md |
| R2-flex | round2-flexibility.md |
| R2-sec | round2-security.md |

Totals: 54 findings. 2 critical, 24 high, 21 medium, 7 low.
Every critical and high claim that names code was checked. None was found wrong.
Two are design-only, because the code they describe does not exist yet.

---

## 1 · Merged findings

"Inc" is the plan's build increment (§4). "Higher taken" means reviewers rated it differently.

### Critical

**V-1. The version is 64 bits, and promotion trusts it as a security boundary.**
- Found by: R1-sec H1 (high), R2-sec C1 (critical). Higher taken.
- Change: keep 16 hex for display. Gate promotion and loads on the full 256-bit digest of document, DDL and every function's code bytes.
- Inc: 1, 4.
- Checked: confirmed. `crates/ordo-plan/src/artifact.rs:412-417` keeps `[..8]`. `crates/ordo-exec/src/registry.rs:147-148` says the version "is a *name* rather than a security boundary". `VersionCollision` fires only inside one deployment.

**V-2. Stored plans carry SQL from dev to every production tenant, outside the hash.**
- Found by: R1-sec C1 (critical), R2-sec H2 (high). Higher taken.
- Change: each target re-renders the step from X's and Y's stored source plus recorded options and compares a full plan digest. The receipt is a record, never an input. Option expressions are Ordo expressions, not SQL.
- Inc: 4, 8.
- Checked: design-only. `$plans` does not exist yet. The plan text (§2.3, inc 8) says the dev-rendered plan "runs unchanged", and §3.1 gates on the version hash only.

### High

**V-3. A version is the compiler's output, so an Ordo release changes it.**
- Found by: R1-use H2, R1-flex H1, R2-flex H1.
- Change: split identity. Source identity (source, function code, language edition) for pins, plans and promotion. Build identity (doc plus DDL, Ordo release) recorded per environment. A release that changes only the build hash moves no pin.
- Inc: 1, 4, 6.
- Checked: confirmed. `artifact.rs:412` hashes `to_bytes(doc)` and `ddl`, both compiler renderings. `$artifacts` records no Ordo release.

**V-4. A load recompiles source and never checks the result against the version.**
- Found by: R1-sec H2 (high), R2-sec M1 (medium). Higher taken.
- Change: recompute the digest at every load and in the release check. Refuse on mismatch, or serve only with a recorded acceptance.
- Inc: 3, 6.
- Checked: confirmed. `crates/ordo-host/src/deploy.rs:466-505` compiles `stored.source` and gates against `stored.ddl`. `stored.version` is passed through as a label. `Verified::from_gate` (`deploy.rs:252`) does not recompute it either.

**V-5. Every old source must compile under every future release, and the release check can block a security upgrade.**
- Found by: R1-flex H2, R2-flex H2, R2-sec M6, R2-use L2.
- Change: a language edition in the source. A deprecation window. `ordo fix` to rewrite old sources. The release check covers versions reachable on a stored chain, not only served ones. A recorded override closes only the named tenants.
- Inc: 6, 8, plus a new editions increment.

**V-6. History is a tree, and the plan models a line.**
- Found by: R1-use H4, R1-flex H3, R2-flex H3, R2-use M12.
- Branches, tenant builds, hotfixes for stuck tenants and reverts all fork. No step exists from a branch back to main, and a branch version is never "activated".
- Change: versions as a graph with parent links. `ordo evolve --from --to`. Replay finds a stored path. Rule that a branch's own additions are orphaned on return. A "patch X" flow for stuck tenants.
- Inc: 5, 8, 16.

**V-7. The app's front end and SDK are outside the version model.**
- Found by: R1-use H1 (high), R1-flex M5 (medium), R2-use H1 (high). Higher taken.
- During a rollout one bundle meets 2 versions. Added ops refuse `access_denied`, changed ops refuse `version_skew`. Mobile builds cannot switch.
- Change: every answer carries the tenant's version. A distinct refusal for an op the version lacks. SDK feature detection and a "version changed" event. Inc 3 test: one bundle, 2 tenants on 2 versions, no refusal.
- Inc: 3.
- Checked: confirmed. `clients/ordo-react/src/client.ts:260,319,421` send `X-Ordo-Version: op.version`. `docs/deploying.md:1350` says an added op is refused `access_denied`. `crates/ordo-api/src/error.rs:756` maps `version_skew`.

**V-8. Routines, schedules, functions, mail and webhooks are not per version.**
- Found by: R1-use H3.
- Change: schedules are the union over served versions. Each run executes under its tenant's pinned `Loaded`. A pin switch hands the cursor over. Add a test to inc 3.
- Inc: 3.
- Checked: confirmed. `docs/howto-routines.md:41` fires once per deployment; `:253` switches routines per node version.

**V-9. Settings in code (18ac) and per-environment values collide with promotion by hash.**
- Found by: R1-use M5, R1-flex M2, R2-use H3, R2-flex H7, R2-flex L3. Higher taken.
- Change: rule now that per-environment and runtime values are bound at the target and sit outside identity. The hash covers their declaration. Rollout settings are per environment.
- Inc: 1, 4.

**V-10. The sharing rule compares shape, not guards or row scope.**
- Found by: R1-sec H4, R2-sec H9.
- Change: compare the shared entity's full contract (shape, guards, acl, release, sealing). The data owner's version and guards decide every read of its rows.
- Inc: 3.

**V-11. Equal-shape sharing breaks every share for the length of every rollout.**
- Found by: R1-use H6 (high), R2-use H2 (high), R1-flex M1, R2-flex M2. Higher taken.
- Change: compare the projection the reading op reads, so additive changes pass. Serve the older shape from dual-written columns during a move. A rollout option to move share partners together.
- Inc: 3, 10.

**V-12. No way to revoke a version with a known flaw.**
- Found by: R1-sec H3, R2-sec H4, R1-flex M6.
- Change: a `revoked` mark with actor and reason. A revoked version is never pinned, activated, rolled back to, promoted or given to new tenants. Its tenants are force-moved or closed. A report of tenants on revoked versions.
- Inc: new, after 2.

**V-13. Tenant builds put a dev branch's version on production tenants past staging.**
- Found by: R1-sec H6, R2-sec H3.
- Change: a production pin accepts only a version that arrived through the promotion gate. Inc 16 depends on 4.
- Inc: 4, 16.

**V-14. The promotion gate checks "dev activated it", not a rehearsal, and drops the named approver.**
- Found by: R1-use M7, R1-flex M7, R1-sec M5, R2-sec H1 (high), R2-flex H3 (part). Higher taken.
- An activation rolled back a second later still counts. The gate's location is unstated. Staging's answer is unsigned.
- Change: the target enforces the gate server-side. The step before signs an attestation (full digest, plan digest, Ordo release, rehearsal result). A named deployment owner promotes, on the stream. A break-glass path when staging is down.
- Inc: 4.
- Checked: confirmed the dropped ruling. `docs/plan-environments.md:148-149`: promotion "is an act by a named deployment owner, recorded on the deployment's stream". plan-versions §3.1 omits it.

**V-15. Sealing a field keeps writing plaintext through the soak, then keeps the column.**
- Found by: R1-sec H5, R2-sec H7.
- Change: a sealing step has no reverse dual write. Revert goes through the worker. The plaintext column is dropped once the seal verifies.
- Inc: 11, 13.
- Checked: confirmed conflict. `docs/plan-schema-evolution.md:484` (D6) refuses unsealing because plaintext in WAL and backups cannot be undone. The soak does exactly that.

**V-16. Staging clones move plaintext and real contact details.**
- Found by: R1-sec M2 (medium), R2-sec H8 (high). Higher taken.
- Change: mask inside production before export, default-deny. Encrypt the bundle to staging's key. Never clone credentials, passkeys, sessions or invites. Mail, push and egress go to sinks in staging.
- Inc: 12.
- Checked: confirmed. `docs/plan-import.md:298` refuses ciphertext bundles, so a bundle holds plaintext. `crates/ordo-exec/src/import.rs:16` seals on arrival.

**V-17. Pin switches have no actor, no history and no stated authority.**
- Found by: R1-sec M1 (medium), R2-sec H5 (high). Higher taken.
- Change: an append-only `$pins` (tenant, from, to, actor, cause, rollout id, at) in the pin's transaction, plus a stream event. Only `Versions` writes it, for a deployment-owner principal.
- Inc: 2.

**V-18. Dual-write triggers run beneath the guards and outside the audit chain.**
- Found by: R2-sec H6 (high), R1-sec L3 (low). Higher taken.
- Change: dual writes in the host write path, inside one command group. A mid-move write passes both versions' guards. Triggers, if kept, have a recorded lifecycle.
- Inc: 11.

**V-19. A node that missed a `NOTIFY` keeps an old pin and loses writes mid-move.**
- Found by: R2-sec H10.
- Change: a per-tenant pin epoch checked in the write transaction. Reconcile pins on a timer and on listener reconnect.
- Inc: 2, 3, 11.
- Checked: design-only. Pins do not exist yet.

**V-20. One gate per version stands in for every tenant, and a restore puts the pin ahead of the data.**
- Found by: R1-flex H4 (high), R2-sec M3, R2-flex M7, R2-flex L2. Higher taken.
- Change: each tenant's `$migrations` records the versions its shape supports. `resolve` refuses a pin outside that set. A cheap shape fingerprint on first open per node. Restore re-pins or holds with a named refusal.
- Inc: 3, 7.

**V-21. Backfills reuse a 20-second forward walk, and the hour window does not fit the owner's scale.**
- Found by: R2-flex H4 (high), R1-use M2 (medium). Higher taken.
- 400,000 tenants at 8 at a time and 1 second each is about 14 hours.
- Change: backfills as resumable, chunked jobs with a cursor per tenant. Concurrency, budget and window as layered settings. Rollout waits on completion. Put the arithmetic in §1.5.
- Inc: 7, 10, 11.
- Checked: confirmed. `crates/ordo-host/src/schema_apply.rs:3126` `FORWARD_CONCURRENCY = 8`, `:3131` `FORWARD_TIMEOUT = 20 s`, both constants.

**V-22. Overlapping soaks are undefined, and one pin column cannot hold them.**
- Found by: R1-flex M3, R2-use M10, R2-flex H5 (high). Higher taken.
- Change: a move record per tenant (from, to, phase, soak deadline). One open soak per tenant: a new step closes the earlier soak, or is refused. Soak length per plan, 0 allowed.
- Inc: 2, 11. Open decision 4.

**V-23. The migration menu has no escape hatch.**
- Found by: R2-flex H6.
- Splits, merges, moves between entities and code-based transforms cannot be expressed.
- Change: a step may name a backfill backend function by code hash, with optional reverse, run by the chunked worker. The menu stays the default.
- Inc: 8, 11.

**V-24. "Render once, run everywhere" fights the stale-plan check.**
- Found by: R2-use H4 (high), R2-flex M6.
- The plan stores dev's live shape and probes. Production tenants differ.
- Change: the plan stores the expected from-version. The stale check compares each tenant's recorded version and drift at run time. Render-time probes are advisory.
- Inc: 8, 9.

**V-25. No 2am way back for every tenant, and "rollback" and "active" are undefined under pins.**
- Found by: R1-use M4, R2-use H5 (high). Higher taken.
- Change: define "active" as what new tenants and rollouts target. One `ordo revert` moves all pins on Y back at once, dry-runnable, and prints what each tenant loses past the soak.
- Inc: 2, 11.

**V-26. The per-tenant maintenance window, ruled 2026-09-21, is missing.**
- Found by: R1-use H5.
- Change: a per-tenant window and hold flag every strategy respects. A tenant outside its window is listed as waiting, not failed.
- Inc: 10.
- Checked: confirmed. `docs/corpus-rulings.md:8316-8320` records the window ruling.

### Medium

**V-27. No rollout controls, and "told" and "healthy" are undefined.** R1-use M1, R2-use M2, R2-use M9. Change: `ordo rollout status|resume|skip|retry|abort`, a failure threshold, `apply` returns at rollout start, a named alert channel, health as gate plus refusal and error rate. Inc 10. (`ordo apply` waits 300 s then exits 1: `crates/ordo-cli/src/main.rs:127`.)

**V-28. Memory grows with spaces times versions.** R1-flex M4, R2-flex M4. Change: measure at 1,000+ spaces in inc 3; share schema and registry by `Arc`; evict views per space. Inc 3. (`deploy.rs:110-120` keeps one `SpaceView` per space per `Loaded`.)

**V-29. On-demand compile is a load an outsider can trigger.** R1-sec M3, R2-sec M2. Change: load at pin time and boot; else single-flight, a load cap, no load from anonymous calls, minimum residency. Inc 3.

**V-30. Orphaned and mid-move columns escape erasure and export.** R1-sec M4, R2-sec M5. Change: an orphan register per tenant that erasure and export read; a default drop date for personal or sealed orphans. Inc 11, 14.

**V-31. Versions pile up, and `served_versions` can strand tenants.** R1-use M3, R1-flex L1, R2-use M6. Change: a minimum version setting that moves stragglers; removing a version needs `--move-to`; refuse a rollout or settings change that strands a tenant. Inc 3, 10.

**V-32. Humans cannot address versions.** R1-use M8, R2-use M4. Change: sequence number, git commit and label in `$artifacts`; `ordo versions list|show|diff`; rename one of the two "dev"s. Inc 2.

**V-33. The dev loop gets heavy.** R1-use M6. Change: `ordo dev` saves are ephemeral; only `apply` stores; pruning on by default in dev. Inc 1, 5.

**V-34. Streams opened on X live on after a switch or revocation.** R2-use M1, R2-sec M10. Change: a pin switch closes the tenant's streams with a reconnect signal. Inc 3.

**V-35. The live deployment's habits change under it.** R2-use M3. Change: register environments in config, mark today's deployment as production, refuse with the command that creates the missing one. Inc 4.

**V-36. Three tiers are hard-coded.** R2-use M5, R2-flex M1. Change: a declared promotion chain, default dev, staging, production; skipping staging is a setting named in the refusal. Inc 4.

**V-37. New tenants during a rollout land on the wrong version.** R1-use L1, R2-use M7. Change: state when "active" flips; a rollout re-reads its tenant set. Open decision 2.

**V-38. Staging cannot clone a tenant on a version staging never saw.** R2-use M8. Change: a clone carries its artifact down, recompiled and gated, recorded as a clone. Inc 12.

**V-39. Promotion recompiles which source is unstated.** R2-use M11, R2-sec C1 (part). Change: fetch source from the step before, never the caller's tree; a refusal names the differing file. Inc 4.

**V-40. Mixed fleets during a rolling Ordo upgrade.** R2-flex M3. Change: record the minimum Ordo release per version; a pin switch completes when every live node has Y loaded. Inc 3, 6.

**V-41. Function code and its runtime sit outside the version checks.** R2-flex M5, R2-sec M1 (part). Change: name the code store in §1.2 under `Versions`; inc 6 loads every served function under the new runtime. Inc 6.

**V-42. Stored plan SQL freezes Ordo's system tables at render time.** R2-flex M8. Change: store app-level changes; render system-table SQL at run time, or version the plan format. Inc 8.

**V-43. Pruning erases evidence and cuts replay chains.** R2-sec M8, R2-flex M7 (part). Change: refuse pruning a pinned, restorable or retained version; keep a tombstone. Inc 14.

**V-44. A beta signup page invites caller-chosen versions.** R2-sec M4. Change: no request names a version; signup maps to an owner-set, attested, unrevoked version. Open decision 2.

**V-45. Sealed backfills release keys with no migration purpose.** R2-sec M9. Change: `Purpose::Migration` naming plan and tenant; alert on releases outside a backfill. Inc 13. (`plan-schema-evolution.md:515` already asks for it.)

**V-46. "By query" rollout: whose query, with what rights.** R2-sec M11, R1-sec L1. Change: an Ordo expression over a tenant-facts view tenants cannot edit, run read-only. Inc 10.

**V-47. Promotion ignores the Ordo binary version.** R2-sec M7. Change: the attestation names it; production warns or refuses on a difference. Inc 4.

### Low

**V-48. Health and apply assume one version per node.** R1-use L2. Change: per-version `$nodes` and `/health/deploy`. Inc 3.

**V-49. Refusals reveal versions and settings to end users and other tenants.** R1-sec L2, R2-sec L1. Change: full detail to the deployment owner's token only. Inc 2, 3.

**V-50. Page cursors and retried writes break at each switch.** R1-use L3, R2-use L4. Change: SDK guidance; honour an idempotency key across one switch. Inc 3.

**V-51. Branch tenant names collide and pile up.** R2-use L1, R2-sec L2. Change: strict name mapping, collision refusal, seed step, last-use list. Inc 5.

**V-52. Tenant admins never learn their version.** R2-use L3. Change: show the label in tenant settings. Inc 3.

**V-53. A revert may revive grants a move killed.** R2-sec L3. Change: killed grants stay killed. Inc 11.

**V-54. One control database and `NOTIFY` assume one region.** R2-flex L1. Change: pin changes as a stream any node can tail. Inc 2.

---

## 2 · Decisions for the owner

17 decisions. Each is a real choice, not obvious work.

| # | Finding | Question | Recommendation |
|---|---|---|---|
| D1 | V-3 | Should a version be the hash of the source (plus a language edition) rather than the compiler's output? | **Yes, split it.** Source identity drives pins, plans and promotion. The build hash and Ordo release are recorded per environment. An Ordo upgrade alone then moves no tenant. |
| D2 | V-1, V-14 | What must the step before prove for promotion: an activation, or a signed rehearsal on a clone? | **A signed rehearsal.** Staging signs full digest, plan digest and Ordo release after the plan ran green on a clone. A named deployment owner promotes, on the stream. |
| D3 | V-2 | Does a stored plan travel to staging and production, or does each target re-render it from source and options? | **Re-render at each target.** Compare a full plan digest with the attested one and refuse on a difference. The receipt is a record only. |
| D4 | V-5 | Should Ordo add language editions and stop loading an old edition after a stated window? | **Yes.** The source names its edition. Ordo loads old editions for a stated window, ships `ordo fix`, and the release check names tenants to move first. |
| D5 | V-6 | Should versions form a graph with parent links, and where do a branch's own additions go on return to main? | **A graph.** `ordo evolve` renders any edge and replay takes a stored path. A branch's additions not in main are orphaned on return, never dropped silently. |
| D6 | V-9 | Which settings sit outside the version hash? | **Per-environment and runtime values sit outside.** Their declaration stays inside. Rollout settings are per environment. Rule it before 18ac and inc 4. |
| D7 | V-10, V-11 | When 2 tenants on different versions share, whose rules decide, and must the shape match exactly? | **The data owner's version and guards decide.** Shape is compared on the fields the reading op reads, so additive changes keep sharing. Guards must match or the read is refused. |
| D8 | V-12 | What happens to tenants on a revoked version: forced move, or closed? | **Forced move, closed if the move fails.** The refusal names the override. A revoked version is never a pin, revert or promotion target. |
| D9 | V-15 | Should a sealing step skip the plaintext dual write and drop the plaintext column once the seal verifies? | **Yes.** It applies D6's reasoning: plaintext in WAL and backups cannot be undone. Revert for a sealing step runs through the worker. Keeping the column is an opt-in that names the risk. |
| D10 | V-18 | Dual writes: generated triggers, or the host write path only? | **The host write path only.** Both versions' guards are checked and the audit chain sees every write. Cost: no SQL-level speed-up for plaintext mappings. |
| D11 | V-22 | May a new step start while the previous one is still in its soak? | **Yes, and it closes the earlier soak** with the one-way confirmation. One open soak per tenant. Soak length per plan, 0 allowed. |
| D12 | V-25 | What does `ordo rollback` do once tenants carry pins? | **It moves every pin on Y back to X at once.** It is dry-runnable and prints what each tenant loses past the soak. "Active" means what new tenants and rollouts target. |
| D13 | V-36 | Is staging required, or may a deployment owner declare a shorter chain? | **A declared chain, default dev, staging, production.** Skipping staging is a setting, off by default, named in the refusal. |
| D14 | V-37, V-44 | During a rollout, which version does a new tenant get? | **The last version that finished a rollout.** No request names a version; a beta signup maps to an owner-set, attested version. Folds into open decision 2. |
| D15 | V-31 | Should a minimum version force stragglers to move, rather than close them? | **Yes.** Below the minimum, tenants move at their window. Removing a version with tenants needs `--move-to`. Nothing strands a tenant silently. |
| D16 | V-23 | Should a step be able to name a backfill backend function for changes the menu cannot express? | **Yes, by code hash, with an optional reverse.** The chunked worker runs it and the receipt records it. The menu stays the default. No raw SQL. |
| D17 | V-16 | Should staging masking be default-deny: mask every field not marked safe, and drop blobs? | **Yes, masked inside production before export.** Credentials, sessions and invites are never cloned. Mail, push and egress go to sinks in staging. |

---

## 3 · Round comparison

| Found by | Count | Findings |
|---|---|---|
| Round 1 only | 4 | V-8, V-26, V-33, V-48 |
| Round 2 only | 19 | V-19, V-23, V-24, V-34, V-35, V-36, V-38, V-39, V-40, V-41, V-42, V-43, V-44, V-45, V-47, V-51, V-52, V-53, V-54 |
| Both | 31 | the other 31 |

Round 2 found 19 new findings and repeated 31 of round 1's 35. Its categories pushed reviewers to the edges of the model. Gates (V-24, V-39), toolchain drift across nodes and runtimes (V-40, V-41, V-42), lifecycle and pruning (V-43), and lazy and stale state (V-19, V-34) were all prompted by named categories. Round 2 also raised severity on several shared findings (V-14, V-16, V-17, V-21, V-22, V-25), and spent more of its reading on code. Open review still found what no category named. Background work under pins (V-8, high) is the largest gap round 2 missed. The already-ruled per-tenant maintenance window (V-26) went unnoticed too, even though category 6 asked for missed rulings. So did the weight of the dev loop (V-33) and health assuming one version per node (V-48). The guidance widened coverage; it did not replace an open read.

---

## 4 · Plan changes

Edits to `docs/plan-versions.md`, each tied to findings.

### Sections

1. **§1.1 What a version is.** Split source identity from build identity. Use the full 256-bit digest for every comparison and keep 16 hex for display. Add a language edition. State that per-environment values sit outside the hash. (V-1, V-3, V-5, V-9, V-41)
2. **§1.2 Where versions are stored.** Add `$pins` (append-only history), a per-tenant move record, the function code store, the orphan register, a `revoked` mark, and label, sequence and git commit columns on `$artifacts`. Add the Ordo release to `$artifacts`. Resolve "never deleted from" against pruning. (V-17, V-22, V-41, V-30, V-12, V-32, V-3, V-43)
3. **§1.3 Pin.** Define "active" under pins. Add the pin epoch. Add the per-tenant set of supported versions in `$migrations`. (V-25, V-19, V-20)
4. **§1.4 Door picks the version.** Recompute the digest at load. Per-tenant shape check on first open. Load at pin time and boot; guard any lazy load. Close a tenant's streams on a switch. Reconcile pins on reconnect. (V-4, V-20, V-29, V-34, V-19)
5. **§1.5 Cost.** Memory per space per version. Rollout and backfill arithmetic at 5,000 and 400,000 tenants. Mixed fleets. (V-28, V-21, V-40)
6. **§1.6 served_versions.** Add a minimum version. Refuse changes that strand a tenant. End users get a plain refusal. (V-31, V-49)
7. **§1.7 Sharing.** Rewrite: the data owner's guards decide; compare the read projection plus guards. Serve the older shape during a move. (V-10, V-11)
8. **New §1.9 Clients across versions.** Version header, distinct refusal, SDK feature detection, switch event, cursors, idempotent retries, tenant admin view. (V-7, V-50, V-52)
9. **New §1.10 Background work.** Routines, schedules, functions, mail and webhooks under pins. (V-8)
10. **New §1.11 Revocation and end of life.** (V-12, V-31, V-53)
11. **§2.2 Dual writes.** Host write path only, both guards checked. Sealing steps have no plaintext dual write. One open soak per tenant. (V-18, V-15, V-22)
12. **§2.3 Stored plans.** Graph with parent links. Re-render at each target, compare plan digest. Stale check per tenant at run time. Backfill function escape hatch. System-table SQL rendered at run time. (V-6, V-2, V-24, V-23, V-42)
13. **§2.4 Every tenant.** Resumable, chunked backfill worker, not the 20-second forward walk. Layered concurrency and budget. (V-21)
14. **§3.1 Promotion.** Server-side gate, signed attestation, named deployment owner on the stream, source from the step before, Ordo release named, declared chain, break-glass, how today's deployment becomes production. (V-1, V-14, V-39, V-47, V-36, V-35)
15. **§3.2 Branches.** Name mapping, seed step, ephemeral dev saves, dev pruning on. (V-51, V-33)
16. **§3.3 Clones.** Mask in production, default-deny, encrypted bundle, no credentials, sinks for mail, push and egress, clone carries its artifact down. (V-16, V-38)
17. **§3.4 Rollout.** Per-tenant maintenance window and hold. Controls and exit codes. Named alert channel and health. "By query" rules. Active flip point. (V-26, V-27, V-46, V-37)
18. **§5 Open decisions.** Add D1 to D17. Fold D14 into decision 2, D11 into decision 4, D4 and the override into decision 1.
19. **§6 Conflicts.** Add: the 2026-09-21 maintenance window ruling (V-26); `plan-environments.md` §6's named deployment owner (V-14); D6 against sealing dual writes (V-15); `registry.rs:147` saying the version is not a security boundary (V-1); `plan-evolution-plans.md` §3.1's stale check against run-everywhere (V-24).

### Increments

| Change | Increment | Findings |
|---|---|---|
| Add full digest, identity split, settings boundary | 1 | V-1, V-3, V-9 |
| Add `$pins`, move record, pin epoch, labels, "active" and rollback defined | 2 | V-17, V-19, V-22, V-25, V-32 |
| Add client contract, background work, sharing rewrite, memory at 1,000+ spaces, load guards, stream close, per-node health | 3 | V-7, V-8, V-10, V-11, V-28, V-29, V-34, V-48 |
| **New 2b: revocation**, after 2 | new | V-12 |
| Split 4: 4a registers environments for the live deployment; 4b attestation, named owner, declared chain | 4 | V-35, V-14, V-36, V-39, V-47 |
| Recompute digest, check reachable versions, load functions under new runtime, override | 6 | V-4, V-5, V-41 |
| Per-tenant supported-version set and shape check | 7 | V-20 |
| Graph, re-render at target, per-tenant stale check, system SQL at run time | 8 | V-2, V-6, V-24, V-42 |
| **New 10b: resumable backfill worker**, before 11, with the function escape hatch | new | V-21, V-23 |
| Windows, controls, alert channel, health, query rules | 10 | V-26, V-27, V-46 |
| Host-path dual writes, one soak per tenant, `ordo revert`, grants stay killed | 11 | V-18, V-22, V-25, V-53 |
| Masking default-deny, sinks, no credentials, clone carries artifact | 12 | V-16, V-38 |
| Sealing without plaintext dual write, migration purpose | 13 | V-15, V-45 |
| Orphan register for erasure and export, prune rules, tombstones | 14 | V-30, V-43 |
| Needs 4 as well as 3, 5, 10 | 16 | V-13 |
| **New: language editions and `ordo fix`**, after 6 | new | V-5 |
