# Member release: leaving a tenant and deleting one's own account

A person can leave a tenant, and delete their account, only when each tenant's release rule clears. The app developer writes the rule.

This plan is the owner's design of 2026-10-05 (`corpus-rulings.md`, *The one-way gaps*; board `person-can-leave` asks 2 and 3; checklist 27aq). His words:

> The ability should exist but that's an app choice and very dependent of many unforeseeable things. In general many apps and systems don't let you delete yourself at all and requires a special request. But if they're attached to tenants, leaving could affect something within the tenant. What if they're an owner or the only manager? What happens to data that references them? But a tenant could have a user-release that can remove a user under whatever app conditions are being met. If that clears then membership from the tenant could be removed and therefore the system could leave. But if a tenant locks a release it cannot be removed. Technically there could be an always true release or always false release. App choice.

Companions: `plan-enrolment.md` (credential, principal, membership; E7 on what tenant deletion does to people), `plan-passkey-recovery.md` (T6, §6), `plan-sharing.md` (seats, `/auth/share/leave`), `plan-syntax-additions.md` §3 and §7 (`$tenant`, the `settings` block), `audit-one-way-gaps.md` (OW-9, OW-10), `audit-topology-gaps.md` 6.8.

Markers: **[settled]** ruled by the owner · **[plan]** proposed here · **[open]** on the board as a draft ask (§8).

Status: plan only. Nothing here is built. Every claim about the code was read against the tree on 2026-10-05.

**Changed 2026-10-06** (owner ruling, person-can-leave ask 11), built in commit eb2c0b0f:
- A leave keeps the seat's `displayName`. It is a dev choice, such as "Bram (Deactivated)".
- The app developer setting `ORDO_MEMBER_LEFT_NAME` replaces the name on leave. Unset or blank keeps it.
- The left seat's `principal` reads `left:<principal>:<seat>`. It is still unlinked, so the person can be admitted again.
- `status` gains `deleted`, for a left seat whose person deleted their account.
- Account deletion marks the account deleted and keeps it. Removal follows the app developer setting `ORDO_ACCOUNT_REMOVAL` (`never`, `now`, `<n>d`).
- §3.2, §5.3, §5.4 and §7 R2, R6 below are marked where they changed.

---

## 0 · The starting state, verified

Three things exist and none of them lets a person out.

| Thing | Where | What it does today |
|---|---|---|
| a **seat** | `members` row, `mem_…`, per tenant | `$caller` binds to it; app rows ref it (`assignee -> members?`); the chain's actor is the principal behind it |
| `std.space.remove` | `crates/ordo-host/src/member_ops.rs:597` | owner only; refuses the last active owner; refuses with `HasReferrers` when any row still refs the seat ("suspend them instead, or move those rows first") |
| `/auth/share/leave` | `crates/ordo-host/src/share_ops.rs:605` | a guest closes their own seat; a member with a role keeps the membership, only the share grant closes |
| tenant deletion | `crates/ordo-host/src/tenant_identities.rs:142-215` | settles the people the tenant made: signed in once becomes independent, never signed in is removed (`registrations::remove_identity`) |
| an independent identity | `/auth/register`, platform providers | has no deletion by anyone (`audit-one-way-gaps.md` OW-10; `audit-topology-gaps.md` 6.8) |

Four facts the plan depends on.

- **The chain attributes to the principal, not the seat.** `Actor::User("usr_…")` (`crates/ordo-commandlog/src/command.rs:33-35`) is hashed into every entry. The id is random. It names a person only through `$principal`.
- **A seat is what rows point at.** The foreign key from an app row to `members.id` is what makes `remove` refuse. Nothing today can end a membership and keep those rows.
- **Guards already evaluate app conditions about a member.** G1 admits reverse walks (`invoices(owner == $caller)`), compiled to SQL at the door. A release rule is the same shape with a different subject.
- **One person, one tenant of their own.** `$spaces.own_of` (`tenants.rs:79`) names the self-serve tenant a person holds while it is `provisioning` or `active`.

---

## 1 · What binds this plan [settled]

- Leaving a tenant exists as a mechanism. Whether it is allowed is the app's choice.
- Deleting one's own account exists as a mechanism. It is gated by the tenants the person belongs to.
- A tenant carries a **release rule** the app developer writes: always true, always false, or conditions.
- A membership ends only when its release clears. An account goes only when every membership has.
- A locked release cannot be passed by the person. The owner: "if a tenant locks a release it cannot be removed."
- The last active owner cannot leave (`member_ops.rs:18-23`, already a rule; `plan-enrolment.md` E3).
- The chain is never rewritten (`audit-topology-gaps.md` 6.8: "retain the fact of the write while losing its content").
- Settings are layered: deployment default, app developer, tenant when exposed, person when exposed (ruled 2026-09-29; `plan-syntax-additions.md` §7).

---

## 2 · The release rule [plan]

**One rule per program, three forms, evaluated against one seat.**

### 2.1 Spelling

```
release member open                                    # always true: any member may leave

release member locked                                  # always false: only the tenant releases

release member when                                    # conditions, all must hold
  not invoices(owner == $member and status == "open")
  and not projects(lead == $member)
```

- `release member` is a top-level line. One per program; two are `duplicate_release`.
- `$member` is a reserved parameter, legal only here. It binds to the seat being released. Like `$caller`, it cannot be supplied.
- The predicate uses the guard grammar G1 already has: reverse walks from `$member`, forward hops through declared refs, literals, `and`, `or`, `not`. Nothing new in the binder.
- A sealed field anywhere in the predicate is `release_on_sealed` (G3 holds).
- Omitting the line means `locked` (draft ask 1). `ordo check` says so in one line: "no release rule: members cannot leave on their own."

### 2.2 The platform's own conditions, always checked

These hold whatever the program writes. They come before the predicate so the answer names the first thing in the way.

| Condition | Refusal | Why |
|---|---|---|
| the seat is the last active owner | `last_owner` (exists) | a tenant with no owner cannot be repaired from inside |
| the tenant is the person's own and is being left by its owner | `last_owner` | same rule; the way out is `std.space.delete` or `std.space.succeedOwner` first |
| the rule is `locked` and the caller is the member | `release_locked` | the owner's lock |
| the predicate is false | `release_held`, naming the atom that failed | "open obligations" |

The answer never carries row ids or counts from the failed atom. It carries the atom's text as the program wrote it. A member learns "open invoices hold your release", not which ones.

### 2.3 The mode as a setting, so a tenant can be given the switch

The predicate lives in the program because it names app entities. The choice between open, locked and the predicate is a value a tenant owner may reasonably want. So the mode is also a setting in the catalogue:

```
settings {
  member.release = rule     tenant        # open | locked | rule; exposed to tenant owners here
}
```

- `rule` means "use the `release member when` predicate". With no predicate written, `rule` is refused at compile time (`release_rule_missing`).
- Exposed to `tenant`, a tenant owner moves it with `std.space.setSetting`. Not exposed, the program's value stands.
- This waits on 24a (`settings-layers`, filed, no machinery). Until then the mode is the `release member` line alone, and R1 reads it from the compiled program. R5 adds the setting when 24a lands; nothing in R1 to R4 changes shape.

### 2.4 CDK

```ts
release.member.open();
release.member.locked();
release.member.when(not(invoices.where(f => f.owner.eq(member).and(f.status.eq('open')))));
```

`member` is a vocabulary constant beside `caller`. `ReleaseNode { kind: 'release'; subject: 'member'; mode: 'open' | 'locked' | 'when'; guard?: GuardNode }` joins `core/ir.ts`.

### 2.5 Compiler checks

| Class | Refused |
|---|---|
| `duplicate_release` | two `release member` lines |
| `release_on_sealed` | a sealed field in the predicate |
| `release_subject` | `$member` outside a release line, or `$caller` inside one |
| `release_rule_missing` | `member.release = rule` with no predicate |
| `release_unknown_entity` | an entity the program does not declare |

Changing the rule is a class A change: no migration, applies on activation.

---

## 3 · Leaving a tenant [plan]

**`std.space.leave`: the caller releases their own seat in the bound tenant, if the rule clears.**

### 3.1 The op

- Bound to the tenant as every op is (`X-Ordo-Space`). No parameter. The seat is `$caller`.
- Any active member may call it. Guests keep `/auth/share/leave`, which already does the right thing for a seat with no role.
- Step-up: fresh presence (`require_step_up`), beside device removal and recovery changes. Ending one's own access is in that class.
- Idempotent: a second call from a seat already `left` answers the same record.

### 3.2 What happens to the seat and the rows

The seat stays. The person goes.

| Thing | After leaving |
|---|---|
| the `members` row | kept, `status = left`, `role = null`, `left_at` set |
| `principal` | **(2026-10-06)** `left:<principal>:<seat>`. Still unlinked, because every "does this person hold a seat" lookup (door, invitation admission, account deletion) matches on principal, and keeping it would read as a live membership and block re-admission. The id behind `left:` lets the person's left seats be found when they delete their account |
| `displayName` | **(2026-10-06)** kept by default. Replaced only when the app developer sets `ORDO_MEMBER_LEFT_NAME`. This supersedes the `member.leftName` plan |
| rows that ref the seat (`assignee -> members?`) | untouched; they point at the seat, which keeps its name and reads `status = left` (2026-10-06) |
| derived access rows (`<entity>Access` with `member -> members`) | deleted, as `revoke_share` does today (`share_ops.rs:580-585`) |
| share grants held by the seat | revoked |
| `groupMembers` rows for the seat | deleted; a group's `memberCount` falls |
| sessions bound to this tenant | the door is rebuilt and announced on `ordo.members`, so the next request is refused here and within a notification everywhere |
| the chain | nothing. Every entry keeps `user:usr_…` |

This is the answer to "what happens to data that references them": it stays, and the app's predicate decides whether such references block the release. An app that wants nothing left behind writes `not tasks(assignee == $member)`.

`status` gains a third value, `left`, beside `active` and `suspended` (`definition-language.md:1094`). From 2026-10-06 it gains a fourth, `deleted`, set on a left seat when its person deletes their account. The door treats `left` as `suspended` does: no access. `std.space.remove` on a `left` seat still works when no rows ref it, and still refuses `HasReferrers` when they do.

### 3.3 Managed identities

A person the tenant made (`std.space.createPerson`, a `$managed` row) exists for that tenant alone (`plan-enrolment.md` A5). Leaving it follows E7's rule, already built for deletion:

- signed in at least once: `$managed.released_at` set, the identity becomes independent (`identity-released`);
- never signed in: the identity is removed with the seat (`identity-removed`, reason `never-signed-in`).

An identity from the tenant's own registered provider is removed with the seat, as E7 does on deletion.

### 3.4 Recorded

- Tenant stream: `MembershipReleased { seat, by: self | owner | deployment, rule: open | locked | rule }`. The principal id is in the entry; the name is not.
- Deployment stream: `identity-released` or `identity-removed` when §3.3 applies.
- The tenant's security stream shows a refused leave as a `Denied` entry with the refusal class, so an owner can see who tried.

---

## 4 · The special-request path [plan]

**When the person cannot leave, somebody who owns the tenant can release them.**

The owner's observation: many systems require a special request. Ordo gives the request two places to land.

### 4.1 The tenant owner releases a seat

`std.space.release($member)`: owner only, step-up, same effects as §3.2.

- Passes a `locked` rule. Locked means "only the tenant releases", and this is the tenant releasing.
- Does not pass the predicate. The app developer's conditions are the app's integrity, and the owner satisfies them the way the app provides (reassign the invoices), then releases. An app that wants an owner override writes it into the predicate: `or $caller.role == "owner"` (G1a, `plan-syntax-additions.md` §3c).
- Differs from `std.space.remove` in one thing: the seat stays as a former member, so referring rows stay. `remove` keeps its meaning and its `HasReferrers` refusal.

### 4.2 The deployment owner, for a person who cannot sign in

A person who has lost every credential cannot call `std.space.leave` or delete their account. `plan-passkey-recovery.md` §6 already gives the deployment owner the role of last resort with an evidence class and a delay. R7 extends that case shape to account deletion: `std.deployment.deleteAccount($principal, $evidence)`, bound to the system space, recorded with the class, contestable by any passkey of the person during the delay. Memberships must already be released; the deployment owner has no power over a tenant's rule. Draft ask 6 puts its timing to the owner.

---

## 5 · Deleting one's own account [plan]

**`POST /auth/account/delete`: when the person is in no tenant, everything that names them goes.**

### 5.1 Why a route and not an op

An op is bound to a space. The account is the deployment's directory. The person's own routes live under `/auth` (`/auth/logout-all`, `/auth/spaces`), and this is one of them.

### 5.2 Preconditions, in the order the answer names them

1. The bearer token is the person's, not a capability or a delegation (as `/auth/spaces` rules).
2. Step-up: fresh presence, and a `confirm` field repeating the principal id, as `std.space.delete` asks for its confirmation.
3. **No tenant of their own** in `provisioning` or `active` (`tenants::own_of`). The way out is `std.space.delete` on it, or `std.space.succeedOwner`.
4. **No seat anywhere** with status `active` or `suspended`, including guest seats. A `left` seat does not count.

A refusal on 3 or 4 answers `memberships_remain` with one row per tenant: id, name, role, and what stands in the way, `own | open | locked | held`. For `held` the atom text from §2.2. This is the person's to-do list, and it is the only place the deployment tells a person which of their tenants are locked.

### 5.3 What is erased

**Superseded 2026-10-06 where it says everything goes at once.** The route now marks the account deleted and keeps it:
- Every session ends.
- The identity is marked deleted (`$principal.deleted_at`). Sign-in, step-up and recovery refuse it.
- Its sign-in links stay, so its identifiers cannot be reused until removal.
- `identity-deleted` is recorded on the deployment stream.
- The person's `left` seats in every tenant become `deleted`, one converge each. The name stays. A tenant that cannot be reached is listed under `seats.unreached`.
- Removal follows `ORDO_ACCOUNT_REMOVAL`: `never` (default), `now`, or `<n>d` run by the `accounts.removal` worker.

The table below is what removal erases. It is the existing `identity_removal::remove` path.


| Table or thing | Act |
|---|---|
| `$authenticator` links, `$passkey` rows, `$credential` | `registrations::remove_identity`, as tenant deletion already does |
| `$principal` row | deleted: display name, identifier, every attribute |
| `$signedin`, `$managed` | deleted |
| recovery codes, open recovery cases, pending second-step enrolments | deleted; an open case is recorded as cancelled |
| invitations addressed to the person's identifier | spent, recorded as `declined` (27ap builds decline) |
| person-layer settings (once 24a exists) | deleted |
| live sessions | `$principalepoch` advanced: every session ends |

### 5.4 What stays, and why that is erasure

**Superseded 2026-10-06 for the account itself.** Until removal, the account and its data stay. The list below is what stays after removal.

- **The chain's actor ids.** `user:usr_…` in every entry the person caused. The id was minted at random (`directory.rs:435`) and, with `$principal` gone, it maps to nobody. That is pseudonymous data with no key, which is what DL's `erasure crypto` promise means for a shared space: the fact of the write stays, the identity behind it is gone (`audit-topology-gaps.md` 6.8).
- **`left` seats** in the tenants the person was in. `principal` is already unlinked. The name stays, and the app decides how to show it (2026-10-06).
- **The security stream.** Append-only and signed. It names the principal id in `Authenticated`, `MembershipReleased`, `IdentityRemoved` entries. It is the record that the deletion happened, which GDPR Art. 17(3)(b) and (e) allow a controller to keep.
- **Rows the person wrote inside tenants.** Those are the tenant's records. Erasing one person's content from a shared space is 6.8, unbuilt, and the app's and tenant's own tools own it.

### 5.5 The receipt

The deployment stream entry `IdentityRemoved { principal, reason: self }` is signed with the rest of the stream. The route's answer carries the entry's position and the stream's checkpoint key id, the same shape `erasure-receipt` gives a tenant. The person keeps it; the deployment cannot later deny the deletion.

### 5.6 One code path with tenant deletion

`tenant_identities.rs` already ends sessions, removes an identity and records it. R6 moves those three into `identity_removal.rs`, called by tenant deletion (reason `tenant-provider`, `never-signed-in`) and by the route (reason `self`) and by R7 (reason `deployment`). The existing test `deleting_a_tenant_settles_the_people_it_made_and_says_so` guards the move.

---

## 6 · Collisions

| Case | Answer |
|---|---|
| Two owners each try to leave at the same moment | the last-owner rule is inside the statement that writes, with the owner rows locked (`member_ops.rs:21-23`); one leaves, the other is refused |
| A leave and a `set tasks { assignee: $member }` race | the predicate and the release are one transaction; the write lands before or after, and a later write may ref a `left` seat, which the app's own guards decide |
| A person leaves while holding an open share window | the window token is bound to the seat; the door refuses it after the rebuild |
| A leave while the tenant is paused (`plan-tenant-pause.md`) | refused with `space_paused`; the person waits or asks the owner |
| Account deletion while a tenant deletion that would release them is in flight | precondition 4 reads the seat after that deletion commits or before it starts; either answer is right at its moment |
| A `left` seat is `remove`d and a row still refs it | `HasReferrers`, as today |

---

## 7 · Increments [plan]

Opus builds each increment. Fable audits the whole at the end (R8). Tests are named so the checklist can carry them.

| # | Builds | Tier | Tests |
|---|---|---|---|
| **R1** | `release member` line: parser, `$member` binding, `ReleaseNode` in the CDK, compiler checks of §2.5, `ordo check` note. The compiled program carries `Release { Open, Locked, When(guard) }` | Opus | `release_open_locked_and_when_parse`; `release_on_sealed_is_refused`; `member_outside_release_is_refused`; `two_release_lines_are_refused`; `cdk_release_emits_the_same_ir` |
| **R2** | `members.status = left`, `left_at`, the door treating `left` as no access, `displayName` kept on leave (2026-10-06: was a placeholder, `member.leftName`; now the app developer setting `ORDO_MEMBER_LEFT_NAME`) | Opus | `a_left_seat_is_refused_at_the_door`; `a_left_seat_keeps_its_rows_and_hides_its_name` (2026-10-06: now `a_leave_keeps_the_seat_name_by_default` and `the_left_name_setting_replaces_the_name_on_leave`); `remove_on_a_left_seat_with_referrers_is_has_referrers` |
| **R3** | `std.space.leave`: the platform conditions of §2.2, predicate evaluation at the door, the effects of §3.2, `MembershipReleased`, door rebuild and announce, step-up | Opus | `leave_under_open_releases_the_seat`; `leave_under_locked_is_release_locked`; `leave_held_names_the_atom_and_no_rows`; `the_last_owner_cannot_leave`; `leaving_deletes_access_rows_and_keeps_referring_rows`; `leaving_ends_the_tenant_sessions_on_every_replica`; `leave_twice_answers_the_same` |
| **R4** | `std.space.release($member)`: owner only, passes `locked`, not the predicate; §3.3 for managed identities on leave and release | Opus | `owner_release_passes_locked`; `owner_release_is_held_by_the_predicate`; `releasing_a_managed_identity_that_signed_in_makes_it_independent`; `releasing_one_that_never_signed_in_removes_it` |
| **R5** | `member.release` in the settings catalogue and `settings { }`, exposed to tenant where the program says so. **Waits on 24a.** Until then R3 reads the mode from the `release` line | Opus | `tenant_sets_member_release_when_exposed`; `member_release_rule_without_a_predicate_is_refused` |
| **R6** | `identity_removal.rs` shared by tenant deletion and account deletion; `POST /auth/account/delete` with §5.2 preconditions, §5.3 erasure, §5.5 receipt (2026-10-06: the route marks the account deleted and removal follows `ORDO_ACCOUNT_REMOVAL`, see §5.3) | Opus | `delete_with_memberships_lists_each_tenant_and_its_bar`; `delete_with_an_own_tenant_is_refused`; `delete_erases_links_passkeys_credential_and_principal` (2026-10-06: now `removal_now_erases_links_passkeys_credential_and_principal`); `delete_ends_every_session`; `delete_cancels_open_recovery_and_spends_invitations`; `the_chain_keeps_the_actor_id_after_deletion`; `deleting_a_tenant_settles_the_people_it_made_and_says_so` (unchanged, guards the move) |
| **R7** | `std.deployment.deleteAccount($principal, $evidence)`: the recovery case shape, delay, contest, evidence class. **Draft ask 6 decides whether now or after first deployment** | Opus | `deployment_delete_waits_its_delay_and_is_contestable`; `deployment_delete_with_memberships_is_refused` |
| **R8** | Audit: every refusal class reachable, no name in any stream entry, `settings.md` and `definition-language.md` rows, `docs/howto-leave.md` for app developers (one page: the three forms, the former-member seat, the delete route) | Fable | reads, no new tests |

Order: R1, R2, R3 are one sprint. R4 and R6 follow. R5 when 24a lands. R7 when ruled.

---

## 8 · Draft board asks [open]

For the lead to put on `person-can-leave`, one ask each. Each ask is under 160 characters, each suggestion under 260.

1. **ask:** When a program writes no release rule, is the default locked (nobody leaves on their own) or open?
   **suggest:** **Locked.** A missing line should not grant an exit the app developer never thought about, and `ordo check` says so in one line.

2. **ask:** Should the predicate live in the program and the open/locked/rule mode also be a setting a tenant owner can be given?
   **suggest:** **Yes, both.** The conditions name app entities, so they are the program's; the switch between forms is a tenant choice when the app exposes it (settings rule, 2026-09-29).

3. **ask:** When a member leaves, should rows that reference them stay and point at a former-member seat, or block the leave?
   **suggest:** **Stay.** The seat remains with the principal unlinked and a placeholder name; the app's predicate decides which references block, and the chain is untouched.

4. **ask:** May a tenant owner release a member past a locked rule? And past the app's conditions?
   **suggest:** **Locked yes, conditions no.** Locked means only the tenant releases; the conditions are the app's integrity, which the owner satisfies first or the app writes an owner clause.

5. **ask:** When a person asks to delete their account while tenants still hold them, refuse and list each tenant with what stands in the way?
   **suggest:** **Yes.** One row per tenant: own, open, locked or held with the condition's text. It is the person's to-do list and the only place they learn which tenants are locked.

6. **ask:** Should the deployment owner be able to delete the account of a person who cannot sign in, as a recovery-style case with evidence and delay?
   **suggest:** **Yes, after first deployment.** The case shape exists in `plan-passkey-recovery.md` §6; it needs a real request before it is built.

7. **ask:** After an account is deleted, the chain keeps `user:usr_…` on every entry the person caused. Is that the erasure Ordo promises?
   **suggest:** **Yes.** The id is random and maps to nobody once `$principal` is gone; the fact of the write stays and the identity does not, which is 6.8's rule.

---

## 9 · What this plan does not do

- Erase one person's content from a shared tenant (`audit-topology-gaps.md` 6.8).
- Decline an invitation or a share ticket (27ap, ruled, built separately).
- Give the deployment owner power over a tenant's release rule.
- Build the settings machinery (24a). R5 waits for it.
