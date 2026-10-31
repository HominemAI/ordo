# Tenant pause: holding a tenant still, and the layer outside it

A tenant can be paused and resumed. While paused it refuses its own ops and keeps its data. Its people can still sign in and act in the deployment's space, which is the layer outside.

This plan is the owner's ruling of 2026-10-05 (`corpus-rulings.md`, *The one-way gaps*; board `tenant-pause` ask 1; checklist 27ar). His words:

> Agreed. Pausability should exist as well as a layer that exists outside of that paused space, the in-between space that allows a tenant to still login and do some actions. But that may actually be a deployment layer thing.

Companions: `bootstrap.md` §4.1 (the registry state machine), `plan-syntax-additions.md` §3 (`deployment` entities, `$tenant`) and §7 (`settings`), `plan-runtime.md` §3.4 (the worker set), `plan-sharing.md` (seats and windows), `plan-client-cache.md` (sync), `plan-member-release.md` (leaving), `audit-one-way-gaps.md` OW-14.

Markers: **[settled]** ruled by the owner · **[plan]** proposed here · **[open]** on the board as a draft ask (§9).

Status: plan only. Nothing here is built. Every claim about the code was read against the tree on 2026-10-05.

---

## 0 · The starting state, verified

The only stop for a live tenant is deletion.

| Thing | Where | Today |
|---|---|---|
| tenant states | `crates/ordo-host/src/tenants.rs:118-130` | `provisioning`, `active`, `failed`, `reaped`. "Active: the only state a tenant serves in" |
| the registry row | `$spaces` (`tenants.rs:69-80`) | `state`, `phase`, `state_at`, `note`, `own_of` |
| a hold | `crates/ordo-host/src/spaces.rs:162, 276, 322` | `SchemaBehind` holds a tenant closed while its database is behind; `reconcile` lifts it; the door answers `space_unavailable` with "closed for a moment" |
| member suspension | `member_ops.rs:527-595` | one seat at a time; refused on the next request here and within a notification everywhere |
| `std.space.delete` | `exit_ops.rs:79` | crypto-erasure, owner only, step-up, no way back |
| the person's tenant list | `/auth/spaces`, `std.person.tenants` | lists active seats with role and `via`; nothing about the tenant's state |
| the deployment's space | `provision.rs`, `erase.rs:354` (`SYSTEM_SPACE`) | the system space; `std.deployment.setClaimCode` is bound to it; `deployment` entities and `$tenant` are planned in `plan-syntax-additions.md` §3 |
| listener delivery | `supervisor.rs:1189` | "Nothing is dropped, delivery is paused, and a paused listener" holds its position |

Three facts the plan depends on.

- **Sign-in is deployment-wide.** A session is the person's, not a tenant's. A paused tenant cannot stop a sign-in; only the door into that tenant can refuse.
- **The resolver already knows how to hold a tenant closed** with a reason and lift it later. Pause is a second reason for the same hold, with its own answer.
- **The workers are deployment-wide loops** that walk tenants (`plan-runtime.md` §3.4). Each one that touches a tenant's rows has to ask whether that tenant is paused.

---

## 1 · What binds this plan [settled]

- A tenant can be paused as a whole and resumed.
- A paused tenant refuses calls and holds its data.
- There is a layer outside the paused tenant where its people can still sign in and do some things.
- That layer is likely the deployment layer. The owner: "that may actually be a deployment layer thing." §4 says yes and why.
- Ordo provides the mechanism. What a paused tenant answers is a setting with a default; the app developer decides the rest.

---

## 2 · The state [plan]

**`paused` is a fifth registry state, reachable from `active` and back.**

```
provisioning ──► active ◄──► paused
     │              │           │
     │              └───────────┴──► (deleted)
     └──► failed ──► reaped   (terminal)
```

- `active → paused` by `std.space.pause` or `std.deployment.pauseTenant`.
- `paused → active` by `std.space.resume` or `std.deployment.resumeTenant`.
- `paused → deleted` by `std.space.delete`, as from `active`. Pausing first is the recommended order (quiesce, export, delete) and is never required.
- `provisioning` and `failed` cannot be paused: there is nothing serving to hold still.
- No timeout. The reconciler (`tenants::time_out_stale`) leaves `paused` alone. A pause lasts until a resume.

The registry row gains three columns: `paused_at`, `paused_by` (`owner` or `deployment`, never a name), `paused_note` (a closed list, §3.3). `IF NOT EXISTS` on each, as 11-15 did for the lifecycle columns.

---

## 3 · The acts [plan]

### 3.1 Who may

| Act | Who | Bound to | Gate |
|---|---|---|---|
| `std.space.pause($note?)` | the tenant owner | the tenant | owner only (`platform_ops::owner_only`), step-up, and the setting `pause.byOwner` (default on, draft ask 1) |
| `std.space.resume` | the tenant owner | the tenant | owner only, step-up; refused `paused_by_deployment` when the deployment paused it |
| `std.deployment.pauseTenant($space, $note)` | the deployment owner | the system space | as `std.deployment.setClaimCode`: refused unless the bound space is the platform's own |
| `std.deployment.resumeTenant($space)` | the deployment owner | the system space | same; lifts either kind of pause |

Whoever is higher wins: a deployment pause is not lifted by the tenant owner (draft ask 4). A tenant pause is lifted by either.

### 3.2 Effects, in order

1. The registry row moves to `paused` in one statement guarded on `state = 'active'`, so two pausers cannot both record it, and a pause on a paused tenant answers the same record.
2. The resolver takes a hold on the tenant with reason `paused`. Every replica is told on the same channel membership changes use (`ordo.members`-style announce), so the next request is refused here and within a notification everywhere. The refresh timer is the ceiling.
3. Live subscriptions on the tenant are closed with a terminal `space_paused` frame (§5).
4. Recorded: tenant stream `TenantPaused { by, note }`, deployment stream the same with the tenant id. Resume records `TenantResumed`.

Resume is the reverse: row to `active`, hold lifted, announce, record. Routines due during the pause are skipped and the skip is recorded; nothing fires late (draft ask 5).

### 3.3 The note

A closed list, never free text, so nothing personal enters a stream: `owner`, `billing`, `abuse`, `legal`, `maintenance`, `other`. Draft ask 6 asks whether the tenant owner may attach a free-text notice for its members; the recommendation keeps the stream clean and puts any prose in a setting (§4.3).

---

## 4 · What a paused tenant answers [plan]

**Every ordinary op is refused with `space_paused`. Reads may be kept by a setting.**

### 4.1 The answer

- New error code `space_paused`, distinct from `space_unavailable`. Unavailable means "retry, it will land" (`lib.rs:158-170`). Paused means "do not retry; somebody has to resume". A client that retried a pause forever would be told to.
- The body carries `paused_at`, `paused_by` (`owner` or `deployment`), `note` and the notice text (§4.3). No name.
- The setting:

```
settings {
  pause.answer = refuse     tenant        # refuse | read-only
  pause.notice = ""         tenant        # text shown to members, under 280 characters
  pause.byOwner = on                      # may a tenant owner pause their own tenant
}
```

`refuse` is the default (draft ask 2). `read-only` keeps every read serving, refuses every write, and still closes live subscriptions and sync pulls, because a paused tenant should look still from outside, not slowly changing. Until 24a lands these three are program lines read by the door, as `member.release` is in `plan-member-release.md` §2.3.

### 4.2 Platform ops on a paused tenant

| Still answers | Refused `space_paused` |
|---|---|
| `std.space.resume`, `std.space.export`, `std.space.delete`, `std.space.chainProof`, `std.space.callersOf`, `std.space.settings`, `platform.security.events` | every app op, `std.space.share`, share windows, `std.space.invite`, `std.space.admit`, and custody changes (`std.space.attachBucket`, `std.space.detachBucket`, `std.space.attestTo`): nothing inside moves |
| `std.space.suspend`, `std.space.reinstate`, `std.space.remove`, `std.space.setRole`: an owner can still remove a departing employee during a pause | `std.space.leave` and `std.space.release` (`plan-member-release.md`): a release evaluates app rows, and the app is still |
| `std.person.tenants`, `/auth/spaces`: list the tenant with `state: paused` and the notice | public reads and public writes of this tenant: the public path answers the notice with the cache life |

### 4.3 What members see

- `/auth/spaces` and `std.person.tenants` gain a `state` field per tenant, `active` or `paused`, and for `paused` the `paused_at`, `paused_by` and `pause.notice` text. The app's shell reads the list after sign-in already (`auth/spaces.rs:1-8`); this is where it learns to show the notice.
- The notice is the app developer's text, tenant-exposable so an owner pausing for a holiday can say so in their own words.

---

## 5 · What keeps running and what stops [plan]

**History keeps being secured. Nothing new happens inside.**

| Worker (`plan-runtime.md` §3.4) | During a pause | Why |
|---|---|---|
| `triggers.drain`, `triggers.batch`, `triggers.runner`, `triggers.clock` | skip the tenant; listeners hold their position, batches hold, clock ticks are skipped and recorded, in-flight runs finish | nothing new happens inside; nothing is lost |
| `commandlog.checkpoint`, `commandlog.attest`, `attest.deliver` | run | they secure history already written; a paused chain has no new entries and the workers idle |
| `secevent.drain`, `secevent.checkpoint`, `secevent.retain` | run | the pause, every refusal and the resume are events |
| `commandlog.prune` | runs | retention is a policy on time, and a pause is not a reason to keep more or less |
| `keys.reseal` | runs | key hygiene does not wait |
| `usage.flush`, `usage.rollup` | run | storage is still held and still counted |
| `provisioning.reconciler`, the catch-up (`tenant_forward.rs`) | run; a paused tenant's database is still brought forward on a program swap | a resume onto a stale schema would only hold the tenant closed again for a different reason |
| `blob.gc` | skips the tenant | a sweep over a tenant nobody can write is safe but pointless; it waits |

Live and sync clients:

- A live subscription (`?live`) is closed with one terminal frame `space_paused` carrying the §4.1 body. The SDK raises `onPaused`, and does not reconnect until told the tenant resumed.
- A device copy (`plan-client-cache.md`) keeps what it holds. Pulls stop. Queued offline writes stay queued and are not sent; the SDK marks them `held: paused`. On resume the device pulls first, then sends, as after any outage.
- A share window token is refused at the door like every other call.

---

## 6 · The in-between layer [plan]

**It is the deployment's space. Ordo supplies sign-in, the list with state, and the platform acts. The app developer builds the screens as deployment entities and ops.**

The owner suspected a deployment layer thing. Checked against what exists and is planned, nothing new is needed as a place.

| What a person of a paused tenant can do | Where it already is, or is planned |
|---|---|
| sign in | sessions are deployment-wide; nothing changes |
| see that the tenant is paused, since when, by whom (role) and the notice | `/auth/spaces`, `std.person.tenants` (§4.3) |
| export the tenant's data | `std.space.export`, owner only, answers on a paused tenant (§4.2) |
| resume, if they may | `std.space.resume` (§3.1) |
| delete the tenant | `std.space.delete` (§4.2) |
| read the app's own notices, plans, invoices | `deployment` entities (`plan-syntax-additions.md` §3a); a guard may test `$tenant.status == "paused"` (§3c) |
| pay a bill, contact the owner, ask for a resume | the app's ops bound to the deployment's space, writing deployment entities; a routine there may call `std.deployment.resumeTenant` (§7) |

Why the deployment's space and nothing else:

- The request is bound to the paused tenant only when the client names it (`X-Ordo-Space`). A call bound to the system space is a different door, already open to every signed-in person at level 2 (§3 of `plan-syntax-additions.md`).
- `$tenant` is planned to carry the tenant's `status`. `paused` is one more value of it, so "show this notice only to paused tenants" is a guard the app writes.
- Ordo does not bill and does not hold the owner's contact. Both are app facts and belong in app entities.

What the app developer controls here: the notice text, whether reads continue, whether the owner may self-pause, and everything in the deployment's space. What Ordo fixes: sign-in works, the state is listed, export, resume and delete answer, and nothing inside the tenant moves.

---

## 7 · Resume by the app, with a routine [plan]

A deployment-layer routine may call `std.deployment.resumeTenant($space)`:

```
routine paidUp on subscriptions.paid =
  std.deployment.resumeTenant(space: subscriptions.tenant)
```

- Only when the pause was `billing`, by the deployment, so a routine cannot lift a tenant owner's own pause or an `abuse` or `legal` one. The op refuses otherwise with `resume_not_for_routine`.
- The actor in the stream is `routine:paidUp`, as every platform-born routine (`Actor::Routine`).
- This is one increment (P6) and waits on `plan-syntax-additions.md` §3 (deployment entities) being built.

---

## 8 · Relations and collisions

| With | Rule |
|---|---|
| member suspension | orthogonal. A suspended seat stays suspended through pause and resume |
| `std.space.delete` | answers on a paused tenant; a pause is not a precondition |
| `std.space.leave` (`plan-member-release.md`) | refused `space_paused`; the release predicate reads app rows and the app is still |
| the `SchemaBehind` hold | both are holds in `Spaces`. A tenant may carry both; the door answers `space_paused` first because it is the one a person can act on |
| `provisioning` | cannot be paused; a tenant whose creation is in flight is held by its own state |
| self-serve `Given::Space` (`self_serve.rs:619`) | a person whose own tenant is paused is answered with it and `state: paused`, never a second tenant |
| two pausers at once | the guarded statement lets one through; the other reads the record and answers it |
| an export during a pause | consistent by construction: nothing writes |
| apply during a pause | the program swap applies to the deployment and the catch-up brings the paused database forward; the tenant stays paused |

---

## 9 · Increments [plan]

Opus builds each increment. Fable audits the whole at the end (P7).

| # | Builds | Tier | Tests |
|---|---|---|---|
| **P1** | `State::Paused`, the three registry columns, the guarded transition both ways, the reconciler leaving it alone | Opus | `active_pauses_and_resumes`; `provisioning_and_failed_cannot_pause`; `two_pausers_one_record`; `paused_is_not_timed_out`; `delete_answers_on_a_paused_tenant` |
| **P2** | the `paused` hold in `Spaces`, `space_paused` error code and body, announce across replicas, the §4.2 table of ops that still answer | Opus | `a_paused_tenant_refuses_app_ops_with_space_paused`; `export_resume_delete_and_member_ops_answer_while_paused`; `share_windows_and_invites_are_refused_while_paused`; `the_hold_lifts_on_resume_on_every_replica`; `paused_beats_schema_behind_in_the_answer` |
| **P3** | `std.space.pause`, `std.space.resume`, `std.deployment.pauseTenant`, `std.deployment.resumeTenant`; `pause.byOwner`; the note list; `TenantPaused` and `TenantResumed` in both streams | Opus | `owner_pauses_with_step_up`; `owner_cannot_resume_a_deployment_pause`; `deployment_resumes_either`; `pause_by_owner_off_refuses_the_owner`; `the_note_is_from_the_closed_list`; `pause_and_resume_are_recorded_in_both_streams` |
| **P4** | `/auth/spaces` and `std.person.tenants` carry `state` and the pause body; `pause.answer` with `read-only`; `pause.notice` | Opus | `the_list_shows_a_paused_tenant_with_its_notice`; `read_only_serves_reads_and_refuses_writes`; `read_only_still_closes_live_subscriptions`; `public_path_answers_the_notice` |
| **P5** | workers skip a paused tenant per §5: triggers hold, clock skips recorded, `blob.gc` skips; live closes with the terminal frame; the SDK's `onPaused`, held offline writes, pull-then-send on resume | Opus | `triggers_hold_position_through_a_pause`; `clock_ticks_during_a_pause_are_skipped_and_recorded`; `checkpoint_attest_prune_and_reseal_still_run`; `live_closes_with_space_paused_and_reconnects_on_resume`; `offline_writes_are_held_and_sent_after_resume` |
| **P6** | resume by a deployment routine (§7). **Waits on `plan-syntax-additions.md` §3** | Opus | `a_routine_resumes_a_billing_pause`; `a_routine_cannot_resume_an_owner_or_abuse_pause` |
| **P7** | Audit: every op classified in §4.2 behaves as the table says, no name in any stream entry, `bootstrap.md` §4.1 and `settings.md` rows, `docs/howto-pause.md` for app developers (one page: the states, the setting, the outside layer as deployment entities) | Fable | reads, no new tests |

Order: P1, P2, P3 are one sprint. P4 and P5 follow. P6 when §3 is built.

---

## 10 · Draft board asks [open]

For the lead to put on `tenant-pause`, one ask each. Each ask is under 160 characters, each suggestion under 260.

1. **ask:** May a tenant owner pause their own tenant, with the app developer able to switch that off?
   **suggest:** **Yes, a setting `pause.byOwner`, default on.** The owner of a space may hold it still; an app that wants pauses to go through the deployment turns it off.

2. **ask:** Should a paused tenant refuse every call by default, with reads kept only when the app chooses?
   **suggest:** **Yes: `pause.answer = refuse` by default, `read-only` as the other value.** Still means still; an app that wants a read-only showroom asks for it.

3. **ask:** Is the in-between layer the deployment's space, with no new place built?
   **suggest:** **Yes.** Sign-in is deployment-wide, the tenant list shows the state, export/resume/delete answer, and the app's notices and billing are deployment entities with `$tenant`.

4. **ask:** Should a pause by the deployment owner be beyond a tenant owner's resume?
   **suggest:** **Yes.** Whoever is higher wins; a tenant owner lifts only their own pause, and the deployment owner lifts either.

5. **ask:** Should routines that fell due during a pause fire on resume, or be skipped and recorded?
   **suggest:** **Skipped and recorded.** A burst of late clock ticks is a surprise; listeners hold their position and catch up, which is the case that matters.

6. **ask:** May a tenant owner write free text for members to see while paused?
   **suggest:** **Yes, as the tenant-exposable setting `pause.notice`, never in the stream.** The stream keeps a closed note list; prose lives in a setting the app can show.

7. **ask:** Should a pause carry an automatic resume time?
   **suggest:** **Not now.** A pause lasts until a resume; a timed one can be a routine in the deployment's space once §3 is built.

---

## 11 · What this plan does not do

- Bill anyone, or hold the tenant owner's contact. Those are the app's facts in deployment entities.
- Pause a single person. Member suspension exists and stays as it is.
- Build deployment entities or `$tenant` (`plan-syntax-additions.md` §3). P6 waits for them.
- Build the settings machinery (24a). The three settings are program lines until it lands.
