# One-way capability gaps (2026-10-05)

A read-only sweep for pairs where one direction exists and its counterpart does not. The sealed-read gap (`serve-sealed-reads`, 27ao) is known and left out. Line numbers are from the tree at the time of the sweep. "Not found" means a search of `crates/`, `docs/roadmap-checklist.md` and the board turned up nothing.

| id | Pair | Side that exists | Side that is missing | Tracked | Severity |
|---|---|---|---|---|---|
| OW-1 | attach a bucket / detach or change it | `std.space.attachBucket`, `crates/ordo-host/src/bucket_ops.rs:98`; `bucket::attach`, `crates/ordo-exec/src/bucket.rs:430` | No detach and no change op. `spacedb::detach` (`crates/ordo-exec/src/spacedb.rs:501`) is a library function that only `std.space.reap` calls, and only for a space that never went active. A cache `forget` exists (`bucket.rs:639`). Tests and comments already speak of a bucket "the owner has detached" | Partly: board `routing-migration` (databases only), `st-attach-existing`. Bucket detach: no | medium |
| OW-2 | attestation webhook: set the sink / switch it off, read it back | `std.space.attestTo`, `crates/ordo-host/src/attest_ops.rs:77`, calling `webhook::register_sink` (`crates/ordo-exec/src/webhook.rs:287`, call at `attest_ops.rs:197`) | `deregister_sink` (`webhook.rs:355`) and `reset_cursor` (`webhook.rs:377`) have no caller in production code. No op or command reads the sink back; `read_sink` (`webhook.rs:390`) is used only by the drain | board `attestation` (11-12) covers layers 2 to 4 as "building". The off switch and read-back: no | medium |
| OW-3 | webhook deliveries: dead-letter / read or retry the dead letters | `webhook::dead_letters`, `webhook.rs:660`; `fail_delivery` writes them | Only tests call `dead_letters` (`crates/ordo-host/tests/attest_webhook.rs:416`). Nothing shows them to the space owner and nothing re-queues one. The checklist promises "dead letters on the ops surface" (`roadmap-checklist.md` near line 4727) | Promise in the checklist item for layer 2. A task for the missing reader: no | medium |
| OW-4 | trigger runs: dead-letter / retry | `triggers::dead_letters`, `crates/ordo-exec/src/triggers.rs:652`; counted and shown in the supervisor overview (`crates/ordo-host/src/supervisor.rs:748`) | Read-only summary of the most recent entries. No retry, requeue or clear. `ordo triggers` has `ls` and `deregister` only (`crates/ordo-cli/src/triggers.rs:159`) | no | low |
| OW-5 | export a ciphertext bundle / import it | `SealedMode::Ciphertext`, `crates/ordo-exec/src/export.rs:72` and `:372`; reached by the `ciphertext` param, `crates/ordo-host/src/exit_ops.rs:320` | `import_space` refuses any bundle whose sealed mode is not `opened` (`crates/ordo-exec/src/import.rs:1352`, message at `:454`, door code `sealed_unsupported`, `exit_ops.rs:718`). A deployment that holds no key for a space can export, and no one can import what it exported | The refusal is documented as a custody act "nobody has ruled". Board task for the ruling: not found | medium |
| OW-6 | blob attributes: write `.mime` and `.size` / read, compare, index them | Both are sealed into the `$blobref` row for every blob field (`crates/ordo-crypto/src/blobref.rs`) | The compiler answers `not_a_ref` for `photo.mime` (`docs/definition-language.md:476`). No blind index over the row | Checklist 27aj, "Owed" list; board `money-types` sits beside it. A task of its own: not found | medium |
| OW-7 | tenant settings: set / read the effective value | `std.space.setRecoveryPolicy` (`crates/ordo-host/src/identity_ops.rs:69`, `set_policy` at `:638`) and `std.space.setShareLinkLifetime` (`crates/ordo-host/src/share_links.rs:100`, handler at `:851`) | No read op for either. `std.space.admission` is the one setter with a reader (`crates/ordo-host/src/admission.rs`). Lifetime is read only inside `lifetime_of` (`share_links.rs:462`) and the recovery policy only through `recovery.policy()` (`recovery.rs:1113`), the deployment-wide value | board `settings-layers` (24a to 24d), "Filed", not planned | low |
| OW-8 | sign in / list and end one session | `/auth/logout`, `/auth/logout-all` (`crates/ordo-host/src/serve.rs:339`) | No route lists a person's sessions and none ends one other than the current one. The deny list is internal (`crates/ordo-host/src/sessions.rs`) | no | low |
| OW-9 | invite / decline; share / decline | `std.space.invite`, `revokeInvitation` (`invite_ops.rs:51`); `/auth/share/peek`, `accept`, `leave` (`serve.rs:379`) | The invitee or the guest cannot decline. An unwanted ticket waits until it expires or the sharer revokes it. A member also has no self-leave: `std.space.remove` is owner only (`member_ops.rs`) and `/auth/share/leave` is for guests | no | low |
| OW-10 | create a person / delete the person's account | `std.space.createPerson` (`crates/ordo-host/src/credential_ops.rs:68`); self-serve sign-up (`/auth/register`, `self_serve.rs`) | No op or route erases a person from the directory. Tenant deletion exists (`std.space.delete`); person deletion does not | no (the board has `erasure-receipt`, which is about tenants) | medium |
| OW-11 | create a tenant that owns a bucket / delete its objects | `std.space.create` with a bucket; `std.space.delete` (`exit_ops.rs:79`) | Delete does not remove the tenant's bucket objects, and no proof of erasure is collected | board `st-delete-objects`, `st-delete-objects-2`, `ds5-status-contradiction`: "Ruled, not built", queued | medium |
| OW-12 | write the deployment security log / read it | Deployment-scope entries are written to the control database | The per-space reader exists (`platform.security.events`, `serve.rs:104`). The deployment-wide reader and the role that reads without pruning do not exist (checklist 22f) | checklist 22f, board `opslog-role-split`, queued | medium |

## Evidence

**OW-1, bucket attach without detach.** `attachBucket` is owner-only and step-up gated, and the module doc argues that custody "never changes silently". The same rule means it never changes at all: the attachment is keyed on its identity, and a different one is a "genuine change" only in the idempotency sense. The one function that removes an attachment (`spacedb::detach`) says it is the single exception and serves reap alone. An app developer who attached the wrong bucket has no op to undo it. Board `st-attach-existing` and `routing-migration` are near, but neither names a detach.

**OW-2, attestation sink with no off switch.** `attestTo` nominates an owner-signed destination. The matching `deregister_sink` and `reset_cursor` exist in `webhook.rs` and only tests touch them. The only production use of `reset_cursor` is inside a restore (`crates/ordo-exec/src/undo.rs:1866`). A tenant owner who nominated a destination cannot remove it, and cannot see which one is set.

**OW-3, webhook dead letters nobody reads.** The outbox backs off, then dead-letters. The listing function exists and is called by tests only. The trigger side at least surfaces dead letters in the supervisor overview; the webhook side surfaces nothing. A delivery that fails for good is lost without a trace a deployment owner can see.

**OW-4, trigger dead letters without retry.** The supervisor overview carries a total and the latest entries, which is the read. There is no way to retry or clear a dead run. The workaround is a code fix and a new event, which is why this is low.

**OW-5, ciphertext export has no way back in.** Export in ciphertext mode is built for a deployment that holds no key. Import refuses it and says re-keying is an unruled custody act. The two halves make a bundle that can be written and never restored on Ordo.

**OW-6, blob `.mime` and `.size`.** The values are written and sealed on every upload. The language reference says they are "ordinary sealed values", and the compiler refuses to resolve them. This is the same shape as the sealed-read gap, one level up: the data is there and the binder has no path to it. It is listed under 27aj as owed work, with no task of its own.

**OW-7, settings with no effective-value reader.** A setter's answer may echo what it stored. I found no read op for the tenant recovery policy or the tenant share-link lifetime. `std.space.admission` shows the pattern that works. Settings in layers (24a) is where an effective-value read belongs.

**OW-8, sessions.** A person can end the current session or all sessions. They cannot see the list or end one. A lost device means ending everything.

**OW-9, no decline and no self-leave.** An invitation or a share ticket can be accepted or revoked by its maker and otherwise sits until it lapses. A member has no way out of a tenant short of an owner removing them.

**OW-10, person deletion.** People are created by the app developer's op or by self-serve sign-up. Nothing removes the directory entry. A tenant can be exported and deleted; the person who owned it stays. I searched for delete-account, erase-person and person-deletion wording in code and checklist and found none.

**OW-11, tenant delete leaves objects.** `std.space.delete` is crypto-erasure of the tenant's database. Objects in the tenant's own bucket are left, and the app owns the erasure proof (`st-delete-objects-2`). Tracked, so listed for completeness.

**OW-12, deployment security log has a writer and no deployment-wide reader.** The log is chained and signed. A tenant can read its own stream. The deployment owner reading across tenants, without the power to prune, is built nowhere. Tracked as 22f.

## Pairs checked and found complete

Create and delete tenant (`create`, `delete`, `reap`); grant and revoke share, link and invitation (`revokeShare`, `revokeLink`, `revokeInvitation`); suspend, reinstate, remove, set role for members; export and import in opened mode; backup and restore (`checkpoint`, `revert`, `restore`, `rollback`); key lifecycle (`ordo keys` has init, rotate, reseal, destroy, status); second factor (begin, confirm, list, remove); passkeys (link, unlink, list); recovery (ticket, approve, cancel, contest); live (`GET /live`, `POST` and `DELETE /live/{stream}`); triggers (register by deploy, `deregister`, `ls`).

## Review (Fable, 2026-10-05)

Every claim above was checked against the tree, the rulings, the plans, the checklist and the board. Line numbers are from the tree on 2026-10-05. Verdicts: **wrong** (the sweep misread the code), **intended** (a ruling or design chose one way), **planned** (tracked work already covers the missing side), **unintended** (nothing chose it), **contrary** (it crosses a stated principle).

| id | Verdict | Evidence | Tracked by |
|---|---|---|---|
| OW-1 | half wrong, half unintended | The change half is wrong: `bucket::attach` is "attach or replace" (`crates/ordo-exec/src/bucket.rs:429-437`, `AttachOutcome::Replaced`), and the checklist calls `Replaced` "a documented custody conversion" (`roadmap-checklist.md:10095`). A change is refused only while objects are held (`st-attach-existing`). The detach half stands: `spacedb::detach` serves reap alone (`spacedb.rs:491-501`), nothing removes a `$bucket` row for an active tenant, and `bucket.rs:555` and `roadmap-checklist.md:10097` already speak of "a bucket the owner had detached" as if it could happen. No ruling chose it. | Change: `st-attach-existing`, `routing-migration` / 18e (databases). Detach: nothing. |
| OW-2 | unintended | Verified. `deregister_sink` (`webhook.rs:355`) has no production caller; `read_sink` (`:390`) is called by the drain only (`attestdrain.rs:468`); `attest_ops.rs` has no off branch. The 2026-09-08 ruling (`corpus-rulings.md` ~4555) argues the role and the verb: `attestTo` "decides where a space's history is published", "naming a third party who will hold signed statements about the chain forever". That is about what a delivery promises once sent. Nothing argues that the destination can never be changed to none, and `attestTo` itself replaces it with another. | `attestation` (11-12), which owes witness formats only. The off switch and the read: nothing. |
| OW-3 | unintended | Mostly verified. `webhook::dead_letters` is called by tests alone. The sweep's "surfaces nothing" overstates: `attestdrain.rs:339-342` sets `ordo_attest_deadletters_total` each pass, so a count exists. The entries do not reach anyone. The checklist marks 11-12 built with "dead letters on the ops surface" (`roadmap-checklist.md:4727`); the trigger side shows entries on the supervisor overview (`supervisor.rs:748`), the webhook side shows a number. Ruling 2026-09 "a dead letter is not resurrected" (`corpus-rulings.md:7677`) is about the enqueue filter, not about a reader. | Claimed built under 11-12. A reader: nothing. |
| OW-4 | intended, for now | Verified. `roadmap-checklist.md:6754`: dead letters are "listed under `supervisor` on `/health/deep` and never acted on", re-audited 2026-09-26. `supervisor.rs:746`: "dead letters are an operator alert, and this is where the alert can be read. Paging stays the deployment's." A retry would re-run code that failed 5 times; the fix is a deploy. Low. Leave it. | Checklist 17 supervisor section. No task owed. |
| OW-5 | planned (same keys), intended (re-key) | Verified. `plan-export.md:200`: the hex layout exists "so a holder of the same keys can re-import". `plan-import.md:401` and checklist 11-10a (`roadmap-checklist.md:4554`) list "ciphertext-mode import into a space holding the same keys" as deferred. Re-keying into another space is refused on purpose: `import.rs:454` "a custody act nobody has ruled, so it is refused rather than guessed". | 11-10a deferred list covers the same-key half. The re-key ruling has no task; it can wait for a case. |
| OW-6 | planned | Verified. `definition-language.md:476-482`: "Not built yet [2026-10-04]: `f.mime` and `f.size` do not resolve. This is settled and owed by the compiler". | Checklist 27aj. Covers exactly the missing side. |
| OW-7 | planned, loosely | Verified: no read op in `identity_ops.rs` OPS or `share_links.rs` OPS. Checklist 24d (done 2026-09-29) says "no general store, write op, exposure declaration or resolver exists; 24a is that machinery". A resolver is the effective-value read. 24a is filed with no go (`settings-layers`, work `filed`). | `settings-layers` / 24a, as "resolver". Not as a read op a tenant can call. |
| OW-8 | intended | Ruling `session-shape`, 2026-09-20 (`corpus-rulings.md:6991-7008`): "nothing can list a principal's live sessions ... not merely unbuilt but unbuildable in this shape ... Revisit if a compliance regime demands an inventory of live sessions." Its other half was revisited: `/auth/logout-all` exists via `$principalepoch` (`sessions.rs:88`). | `durable-auth` (20b), built. |
| OW-9 | unintended | Verified. `invite_ops.rs:51-56` OPS are invite, invitations, revokeInvitation; share routes are peek, accept, leave (`serve.rs:379-382`); `member_ops.rs` ops are owner-only; `share_ops.rs:585`: "A member who holds a role keeps the membership". `plan-enrolment.md:302,341`: a ticket ends by redeem, expiry or revoke. `plan-passkey-recovery.md:115` (T6, departing employee) has the tenant remove the membership. No ruling or plan names a decline or a self-leave. | Nothing. |
| OW-10 | partly wrong; the rest contrary | Wrong half: `std.space.delete` settles the people the tenant made (`plan-enrolment.md` E7; `tenant_identities.rs:142,200` calling `registrations::remove_identity`; test `deleting_a_tenant_settles_the_people_it_made_and_says_so`). Standing half: an independent identity, from `/auth/register` or a platform provider, has no deletion by anyone. `plan-passkey-recovery.md:115`: "An independent identity keeps existing after the person leaves a tenant", said of memberships, not of a person who wants out. `audit-topology-gaps.md` 6.8 already records per-person erasure as unbuilt. `definition-language.md:3312` promises "erasure crypto # GDPR Art.17 / CCPA delete". Erasure that only a tenant can have is erasure that a person cannot. | `audit-topology-gaps.md` 6.8, no task. `erasure-receipt` is tenants. |
| OW-11 | planned | Verified. Ruling 2026-09-21 (D-S5 settled): keys destroyed, objects left, a custody entry records what remained, the app owns the proof. | `st-delete-objects-2`, `st-delete-objects`, 17ab; queued. Covers the direction as ruled. |
| OW-12 | planned | Verified. `secevents_route` (`serve.rs:1012`) requires `space` and refuses `DEPLOYMENT_KEY`; `ordo-cli` has no `seclog`. `plan-opslog.md` D5 specifies the deployment reader, R4 the role. | `opslog-role-split` (22f) queued, `opslog` (11-22) building; D5 named in the task's `where`. |

### New findings

| id | Pair | Verdict | Evidence | Tracked by |
|---|---|---|---|---|
| OW-13 | declare places on a money or decimal field / change them on a stored field | unintended | `diff.rs:818-837` compares `ddl::column_ty` strings with `same_type`, which cuts the type at `(` (`diff.rs:145-157`). `Money(_)` is `bigint` for every places value (`ddl.rs:99`); `Decimal(p)` is `numeric(1000, p)` and loses `p` at the cut. `shape.rs:458` cuts the same way. So `price: money(0)` to `money(2)`, or `decimal(2)` to `decimal(4)`, is no change to evolution and no mismatch to the shape gate. Stored minor units then read 100 times smaller; a decimal write finer than the live scale is rounded by Postgres. The ruling of 2026-10-05 (`definition-language.md:171-173`) says "a stored value widened into an expression is never rounded". `plan-schema-evolution.md:442` (B3, type change) would cover it if the diff saw it. Nothing has deployed, so no migration is owed; the hazard stands for every future change. | Nothing. `money-types` owes `allocate` only. |
| OW-14 | stop a tenant / start it again | unintended | Member suspend and reinstate exist (`member_ops.rs:48-49`). Tenant states are provisioning, active, failed, reaped (`bootstrap.md` state machine; `reap_op.rs:246`). Nothing holds a live tenant still. The only stop for an active tenant is `std.space.delete`, which is crypto-erasure and has no way back. No ruling, plan or task names a tenant pause. | Nothing. |
| OW-15 | apply a schema change / undo it | planned | `std.deployment.apply` and `ordo rollback --version` exist. `plan-evolution-plans.md:47-52`: rollback "means serve the old version again. It does not undo" the schema change; devolution "Not built". Reverse-with-forward is its §3.3; drops are 13e. | `evolution-plans` (27a to 27c ruled, ask 3 waiting), `forward-gaps`, 13e. Covers the direction. |
| OW-16 | share with a guest / list the shares | unintended, low | `std.space.share` mints a seat; `revokeShare($seat)` takes the seat id (`share_ops.rs:61-65`). No op lists a tenant's live seats. Links have `std.space.links`, invitations `std.space.invitations`. `plan-sharing.md` §3.3 "never sees a list of people" is about enumerating people; a list of one's own seats names nobody new. | `sharing`, built. Nothing for the list. |
| OW-17 | set the deployment's self-serve mode and claim code / read them back | unintended, low | `std.deployment.setSelfServe` and `setClaimCode` write (`self_serve.rs:65`). I found no reader in `ordo-host` or `ordo-cli`. The same shape as OW-7, one layer up. | `settings-layers` / 24a, loosely. |

Also checked and found deliberate: `ordo keys destroy` with no undo ("there is no escrow and no recovery", `corpus-rulings.md:6203`); a tenant's recovery `hold` can be turned off again (`recovery_hold.rs:57,96`); `setShareLinkLifetime "default"` clears the tenant's choice (`share_links.rs:867`); self-serve sign-up has an off switch (`SelfServe::Off`).

### Board-ready asks

One question per ask. The lead turns these into tasks.

**OW-1, bucket detach**

- ask: May a tenant owner detach its own bucket and return to platform custody, under the same refusal as a change while objects are held?
- suggest: **Yes, as `std.space.detachBucket`: owner-only, step-up gated, refused while objects are held.** The pin forbids a silent custody change; a detach that is consented, step-up gated and logged is as loud as the attach.

**OW-2, attestation sink**

- ask: May a tenant owner switch off its attestation destination, and read back which one is set?
- suggest: **Yes: `attestTo` with no destination removes the sink, and a read op shows it.** `deregister_sink` and `read_sink` exist already; the owner who chose a witness must be able to see it and end it.

**OW-3, webhook dead letters**

- ask: Should webhook dead letters be readable by the tenant owner and the deployment owner, with a retry?
- suggest: **Yes: a read op per tenant and the entries on the supervisor overview; no retry yet.** The checklist promises them on the ops surface and only a count exists; retry waits for a case that needs it.

**OW-9, decline**

- ask: May an invitee decline an invitation, and a guest decline a share ticket, before it expires?
- suggest: **Yes, on the peek route: a decline spends the ticket and is recorded.** A refused ticket should not sit live for days; the tenant sees the decline in its stream.

**OW-9, self-leave**

- ask: May a member leave a tenant on their own, without an owner removing them?
- suggest: **Yes: `/auth/share/leave` for role holders too, refused for the last owner.** The identity is the person's and the membership is the tenant's; leaving should not need the tenant's act.

**OW-10, a person deletes their account**

- ask: May a person with an independent identity delete their own account, and what happens to their memberships?
- suggest: **Yes, step-up gated: credential, links and passkeys go, memberships are removed, a last owner is refused.** The language promises erasure; today only a tenant can be erased.

**OW-13, places change**

- ask: Should a change of places on a stored money or decimal field be refused, or made a type change with a stated transform?
- suggest: **Refuse it until it is a B3 type change with an explicit scale factor.** Today `money(0)` to `money(2)` passes the diff and the shape gate unseen, and stored values read 100 times smaller.

**OW-14, tenant pause**

- ask: Should a tenant be pausable as a whole, refusing calls and holding its data, with a resume?
- suggest: **Yes: `std.space.pause` and `resume`, for the owner and the deployment owner, with a setting for what paused calls answer.** Today the only stop for a live tenant is delete.

**OW-16, list shares**

- ask: Should a tenant owner be able to list its outgoing guest shares, as it lists links and invitations?
- suggest: **Yes, `std.space.shares`: one row per live seat with its ops and expiry.** Revocation takes a seat id that nothing shows after the share answer; a list of one's own seats names nobody new.

**OW-17, read deployment settings back**

- ask: Should the deployment's self-serve mode and claim code setting be readable after they are set?
- suggest: **Yes, on the deployment status surface: the mode, and whether a code is set, never the code.** A setting with a writer and no reader can only be checked by trying it.
