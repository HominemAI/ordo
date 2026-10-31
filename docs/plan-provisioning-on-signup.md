# Provisioning on sign-up: a tenant of one's own

> **Answered 2026-09-29:** 60 sign-ups an hour and 5 per address by default, configurable; tenants of people who never return are kept, and deleting them is the app developer's decision. Section 6 is answered.

Checklist `11-15` (the provisioning lifecycle). `bootstrap.md` §3.2, the
self-serve entry path, which is settled and not built. The owner, 2026-09-29,
on the notes app: *"this is simply how provisioning works - an app dev needs
built in provisioning."* And: *"in this app 1 user 1 tenant. User should be
able to sign up via a form, press their passkey button, and bam theyre signed
in."*

This is a platform feature. The notes app (`plan-first-program.md`) is its
first consumer and does not own it. Any app developer turns it on.

Companion documents: `plan-passkey-signin.md` (S0 to S7; the sign-in route
this plan hooks), `plan-enrolment.md` (E1 to E6, built), `bootstrap.md`
§1 to §4, `deploying.md` §4c.

Markers: **[settled]** ruled by the owner or restated from a settled document
· **[plan]** proposed here · **[seam]** work another plan owns.

Status: **P0 and P1 built (see Built at the end); P2 onward planned.** Every claim about the code was read against
the tree at `fc46564` on 2026-09-29. Nothing has deployed, so no shape change
owes a migration.

---

## 0 · The starting state, verified

| Fact | Where |
|---|---|
| `std.space.create` exists and is served by the binary since 2026-09-29. It is owner-only, step-up gated, and answers only when bound to the deployment's own space. `owner` is a parameter; the creator is the door's principal | `crates/ordo-host/src/tenant_ops.rs:382-710`; `runtime.rs:1005-1032` (`platform_specs`) |
| Provisioning is phased: preflight (phase 0), registry row first, then custody and the database, then activation, then `announce_members` so every replica serves the tenant without a restart | `tenant_ops.rs:485-689`; `provision.rs:813` (`preflight`), `:1244` (`provision_prepared`); `tenants.rs:326` (`reserve`), `:376` (`activate`), `:393` (`fail`) |
| A stuck provision is timed out by the reconciler and marked `failed`; releasing what it built is `std.space.reap`, which a person calls | checklist `11-15`; `tenants.rs:442` (`time_out_stale`); `reap_op.rs` |
| Every tenant gets a database and custody keys at creation; the cluster defaults to the deployment's own | ruled 2026-09-25; `tenant_ops.rs:241-374` (`target_for`) |
| The self-serve entry path is settled: caller any human, only when policy enables it, owner is the caller, step-up required, a per-principal limit checked in preflight, default off | `bootstrap.md` §3.2 |
| Registration at the door mints a principal (P3). The answer carries `token`, `expires_at`, `session_expires_at` and no space | `plan-passkey-signin.md` §0.3, S4; `auth.rs:320` (`complete`), `:569-571` |
| `/auth/register` is not built yet. The router has `begin`, `complete`, `logout`, `refresh` | `auth.rs:673-676`; S2 onward in progress |
| E4's `Admission::on_arrival` admits a person into a tenant that exists. It has no caller | `admission.rs:392`; `plan-enrolment.md`, Built, E4 |
| The registration sweep removes an identity no tenant admitted. A person admitted to their own tenant is never swept | `registrations.rs`; E4 |
| Nothing rate-limits the auth routes. Quotas gate per space, after the access check | `quotas.rs` module doc |
| The system space's founding owner is `SYSTEM_PRINCIPAL`, "Ordo platform". No human is a deployment owner until something admits one | `provision.rs:194-214` (`ensure_system_space`) |
| First-comer (E5) is built and is ruled not a default anywhere: *"setup belongs on the server side end of story"* | `corpus-rulings.md` 2026-09-29, the notes app |
| The presence requirement on every platform op, tenant creation included, is a setting the app developer holds. Default: presence required | `corpus-rulings.md` 2026-09-29, *"What is enforced is app design"* |
| A person may see the list of their own tenants. No op lists it yet | ruled 2026-09-29 (`signin-details`); `plan-enrolment.md`, What waits |

---

## 1 · What binds this plan [settled]

| | Rule | What it fixes here |
|---|---|---|
| `bootstrap.md` §3.2 | Self-serve is off by default. The owner is the caller. Step-up is required. A per-principal limit is checked in preflight | the setting, its default, who owns the tenant, and the bound |
| The owner's principle | Ordo provides the mechanism; the app developer decides. *"We do not enforce an app developer's choices but we can have it as default"* | one setting, reachable by environment, op and console, as the claim code is (E5) |
| P3 | A sign-in is an enrolment | under this setting, an enrolment is also a provisioning |
| Presence is app design | The presence requirement on creating a tenant is the app developer's setting, presence required by default | a sign-in without presence provisions nothing unless the app developer relaxed it |
| Ruled 2026-09-25 | Every tenant has a database. The cluster is the deployment's default | a sign-up creates a Postgres database. Said plainly in §2.4 |
| `bootstrap.md` §4.4 | The registry row is written before any resource | the latch is the registry row (§2.2) |
| The notes ruling | The deployment's own owner is established on the server side, never by arriving first | §3, P0 |

---

## 2 · The model [plan]

### 2.1 One setting

`ORDO_PROVISION_SELF_SERVE`, a deployment setting stored in
`$deploymentsetting` on the claim code's pattern (E5): the environment seeds
it on a boot that finds nothing stored, `std.deployment.setSelfServe` (the
deployment's own space, owner-only) writes it, the stored value wins after
that, and the console will call the op.

| Value | Meaning |
|---|---|
| `off` | Default. Nothing here runs. `std.space.create` is the only way a tenant comes into being |
| `on-signup` | Every person who signs in and holds no tenant of their own is given one, as its owner, in the same request |

A third value, `on-request` (a signed-in person asks for a tenant, and for
a second one under `perPrincipalLimit`), is §5's P5 and waits for after the
first deployment.

### 2.2 What "a tenant of one's own" is, and the latch

An **own tenant** is a tenant whose registry row names the person as
`own_of`. One column on `$tenants`, and one partial unique index:

```
"own_of" text NULL
CREATE UNIQUE INDEX ... ON "$tenants" ("own_of") WHERE "state" IN ('provisioning', 'active')
```

The index is the whole of "one person, one own tenant":

- two racing provisions for one principal: the second `reserve` fails on the
  index and reads the first's row instead;
- a crash between the registry row and activation: the row is
  `provisioning`, the next sign-in finds it and answers "in progress", the
  reconciler times it out to `failed`, and a person reaps it;
- a `failed` or `reaped` row is outside the index, so the next sign-in
  provisions again. A failure costs one sign-in and leaves one visible row.

No second table. The registry row is written first (`bootstrap.md` §4.4), so
the latch and the row are the same write.

### 2.3 The hook, in the sign-in route

After `sign_in` succeeds in `/auth/complete` and before the answer is
written, when the setting is `on-signup`:

1. Presence. If the sign-in's method carries no presence and the app
   developer's presence setting still requires it for tenant creation,
   skip: the answer carries `space: null` and `provisioning: "presence
   required"`. A passkey sign-in has presence by construction.
2. Look up the principal's own tenant. `active`: answer its id.
   `provisioning`: answer `space: null, provisioning: "in progress"`.
3. None: `TenantOps::provision_own(principal, display, now)`, §2.4.
4. On success: `space: <id>`. On refusal: `space: null, provisioning:
   "failed"`, the row is `failed`, and the sign-in still succeeds. The
   identity is real whatever became of its tenant.

The hook runs on every sign-in, and not only on the registration that
minted the principal. That is what makes a failed provision retryable, and
it gives a tenant to a person who registered before the setting was turned
on.

The answer of `/auth/complete` gains two fields:

```
{ "token": …, "expires_at": …, "session_expires_at": …,
  "space": "spc_…" | null,
  "provisioning": null | "in progress" | "failed" | "presence required" }
```

### 2.4 `TenantOps::provision_own`

A second entry into the provisioning `TenantOps::create` already runs, and
the same phases. Where it differs:

| | `std.space.create` | `provision_own` |
|---|---|---|
| Who calls | a deployment owner, present, bound to the home space | the sign-in route, on the platform's own setting |
| Owner | the `owner` parameter | the signed-in principal |
| Creator recorded | the door's principal | the same principal. Attribution `user:<principal>` (`bootstrap.md` §3.2) |
| `name` | the caller's | the display the person registered with, else the principal id |
| Cluster | `$dbHost` or the default | the default (level 3), always |
| `dryRun` | yes | no |
| Step-up | the door's gate | §2.3 step 1 |
| Registry row | `reserve` | `reserve` with `own_of` set. A conflict on the index is the "already has one" answer |
| Announce | `announce_members` | the same |

Everything from preflight to activation is the existing code. `create`'s
check 3 (bound to the home space) does not apply: no space is bound, because
the person has none yet.

**A sign-up creates a Postgres database.** Every tenant has one (ruled
2026-09-25) and this plan does not change that. A sign-up therefore costs a
`CREATE DATABASE`, a schema apply at the serving version, and a custody key.
P1's done-when includes the number, locally, and the notes app measures it
live.

### 2.5 What stops one person creating thousands

| Brake | Where |
|---|---|
| One own tenant per principal | the index, §2.2 |
| A principal costs a registration ceremony, and a registration is recorded | S4, S0 |
| A deployment-wide cap on sign-ups: `ORDO_PROVISION_SIGNUPS_PER_HOUR`, default 60 | P3. Checked at `/auth/register` before the ceremony begins, so a refused sign-up mints nothing. Refused with `429` (`QuotaExceeded`, the one 429 in the inventory) |
| A per-address cap: `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR`, default 5 | P3. The address as the deployment sees it. Revised 2026-10-11: proxy mode's (`crate::proxy`), off by default and read only in proxy mode, where the visitor's address comes from the trusted proxy's header. Past it a sign-up is slowed, never refused (`public-data` ask 28) |
| The count of own tenants is on `/health/deep` and in the census | P3 |

What this plan does not brake: a person who signs up and never returns keeps
a tenant, and its database, for as long as the deployment runs. The
registration sweep does not reach them, because they are admitted. An idle
policy is the app developer's, and it waits for the first measurement
(`plan-first-program.md` §8). §6 asks the owner whether v1 needs one.

### 2.6 What is recorded

On the deployment's stream, `Enrolment { act: "own-tenant" }` naming the
principal and the space, and on failure the same act with the refusal.
The tenant's genesis group records the person as creator and founding owner,
as `provision_prepared` already does. The setting's changes are recorded as
the claim code's are.

---

## 3 · The deployment owner, named on the server [plan]

The notes ruling: *"setup belongs on the server side end of story."* And
first-comer is not a default anywhere. So something on the server must make
a human the deployment's owner, and today nothing does (§0, last rows).

**`ORDO_DEPLOYMENT_OWNERS`**, a comma-separated list of principal ids, read
at boot. After `ensure_system_space`, an owner row in the deployment's own
space for each principal named, idempotent, recorded on the stream as
`Enrolment { act: "deployment-owner" }`. The variable adds and never
removes: removing a deployment owner is `std.space.remove` (E3), and the
variable is edited to match. A principal it names that does not exist yet
is written anyway; the row waits for the person.

How a deployment owner gets a principal id: they register on the app's
page like anyone, read their id from the page, set the variable, and
restart. Two minutes, and every step is on the server or in the person's
own hands.

This is `bootstrap.md` §3.1's bootstrap admin, arriving by configuration.
It is P0 because the notes app cannot be applied over the door by a present
person without it.

---

## 4 · Increments [plan]

Each is shippable alone. Each done-when names a test that fails without the
increment. New host tests go in `crates/ordo-host/tests/provision_own.rs`,
a new file.

| | What | Files | Done when |
|---|---|---|---|
| **P0** | **The deployment owner named at boot.** `ORDO_DEPLOYMENT_OWNERS`; owner rows in the home space after `ensure_system_space`; recorded | `runtime.rs` (the boot block near `:3534`), `provision.rs`, `docs/deploying.md` §1 table | `provision_own.rs::a_principal_named_in_the_environment_owns_the_home_space_after_boot`, `::the_variable_adds_and_a_second_boot_writes_nothing`, `::a_removed_owner_is_not_re_added_unless_named_again` |
| **P1** | **`provision_own` and the latch.** The `own_of` column and partial index; `TenantOps::provision_own`; `tenants::reserve` taking `own_of`; `tenants::own_of(principal)` | `tenants.rs` (`ensure_table`, `reserve`, one query), `tenant_ops.rs`, `provision.rs` (nothing, if `provision_prepared` is enough) | `::a_person_gets_one_own_tenant_however_many_times_it_is_asked` (eight racing calls, one active tenant), `::a_failed_own_tenant_does_not_block_the_next_attempt`, `::an_own_tenant_has_the_serving_schema_a_database_of_its_own_and_its_person_as_owner`; the test prints the wall time of one provision, and the number goes in this table |
| **P2** | **The setting and the hook.** `ORDO_PROVISION_SELF_SERVE`, `std.deployment.setSelfServe`, `$deploymentsetting`; the hook in `complete` (§2.3); the two answer fields; the presence rule; the census line | `admission.rs` or a new `self_serve.rs` (the setting, on `ClaimStore`'s pattern), `auth.rs` (`complete`), `runtime.rs` (`platform_specs`, census), `platform_ops.rs` (the op spec), `docs/deploying.md` | `::a_first_sign_in_under_on_signup_answers_a_token_and_a_space_the_token_can_use` (the token then calls an op in that space and is its owner), `::a_second_sign_in_answers_the_same_space`, `::under_off_the_answer_has_no_space_and_no_tenant_exists`, `::a_sign_in_without_presence_provisions_nothing_by_default`, `::the_setting_changed_through_the_op_is_what_the_next_boot_reads` |
| **P3** | **The bounds.** The two caps of §2.5, checked at `/auth/register` and at the hook; `429`; the own-tenant count on `/health/deep` | `auth.rs` (`register`, once S4 lands; `complete`), `serve.rs` (deep health), `runtime.rs` (census) | `::the_sixty_first_sign_up_in_an_hour_is_refused_and_no_principal_is_minted`, `::the_sixth_sign_up_from_one_address_is_refused_and_the_seventh_from_another_is_not`, `::deep_health_counts_own_tenants` |
| **P4** | **A person's own list of tenants.** `GET /auth/spaces` with the bearer token: `[{space, name, role, via}]` for the token's principal, `via` being `member` or, once sharing lands, `guest`. Nobody else's list | `auth.rs` (new handler, `router`), `serve.rs`, `member_ops.rs` (one query across the databases served) | `::a_person_lists_their_own_tenants_and_a_capability_token_lists_nothing`, `::the_list_names_no_tenant_the_person_was_removed_from` |
| **P5** | **On request, and the per-principal limit.** `on-request`; a signed-in person asks for a tenant; `perPrincipalLimit`, default 1. The per-address cap's trusted-proxy header is built with proxy mode (2026-10-11), before the first deployment | after the first deployment | filed on the checklist beside `11-15` when P1 to P4 land |

**Order: P0, P1, P2, P3, P4.** P0 touches nothing the others touch and can
go first or last. P1 has no dependency on passkey sign-in and can start now.
P2 needs S3 (a sign-in that reaches `complete` with presence) to be tested
end to end; its unit half can be tested with a password sign-in and the
presence setting relaxed. P3's `/auth/register` half waits for S4 [seam].
P4 stands alone.

---

## 5 · Where this collides with passkey sign-in and enrolment

| File | This plan | Others | Rule |
|---|---|---|---|
| `crates/ordo-host/src/auth.rs` | P2 adds the hook at the end of `complete`; P3 adds a check at the top of `register`; P4 adds a handler | S3, S4, S5, S7 edit `begin`, `complete`, `register`, `router` | **S3 and S4 land first.** P2's hook is one call after `sign_in` succeeds and touches no ceremony code |
| `crates/ordo-host/src/tenant_ops.rs` | P1 adds `provision_own` beside `create` | 11-15's later work | append only |
| `crates/ordo-host/src/tenants.rs` | P1 adds a column, an index and a query | | append only; the column goes last (every reader indexes by position) |
| `crates/ordo-host/src/runtime.rs` | P0 the boot block; P2, P3 the census and `platform_specs` | S2 (`auth_from_env`, the registry loop, the census) | S2 lands first; this plan adds lines and renames nothing |
| `crates/ordo-host/src/admission.rs` | P2 may add the setting beside `ClaimStore`, or a new file | E4, E5, built | append only |
| `docs/deploying.md` | rows for four variables | S2, E4, E5 rows | append; the lead session merges |

---

## 6 · Open for the owner

1. **Whether v1 needs an idle policy for own tenants** (§2.5). A person who
   signs up and never returns keeps a tenant and a database. Recommend
   **no policy in v1**: measure how many sign-ups go idle on the notes app
   first (`plan-first-program.md` §8), and file the policy for after the
   first deployment. Deleting a tenant is `std.space.delete` and needs a
   person today.
2. **The two caps' defaults**: 60 sign-ups an hour for the deployment and
   5 an hour from one address. Recommend those numbers; both are settings.

Decided here under a ruling, and listed so it is visible: the deployment
owner is named by `ORDO_DEPLOYMENT_OWNERS` at boot (§3). The ruling is
*"setup belongs on the server side"*; the variable is this plan's reading of
it, and the owner can move it to a CLI command over the database instead.

---

## 7 · What this plan does not do

- It does not build `/auth/register` or the ceremonies. That is S3 to S7.
- It does not admit a person into anybody else's tenant. That is E4's
  `on_arrival`, which the sign-in route calls once a sign-in names a tenant.
- It does not delete idle tenants (§6, question 1).
- It does not build the console. Every setting here is reachable through an
  op, which is what a console will call.
- It does not build the invited path (`bootstrap.md` §3.3). `std.space.create`
  with an `owner` parameter is that path today.

---

## Built

Built 2026-09-29 on branch `worktree-agent-a0df85d6a8eeb27fd`, P0 and P1 only. P2 onward, the hook in `/auth/complete`, is not built, and `auth.rs` and `serve.rs` are untouched.

### P0 · The deployment owner named at boot

- `ORDO_DEPLOYMENT_OWNERS` is a comma-separated list of principal ids. `RuntimeConfig::from_env` parses it into `RuntimeConfig::deployment_owners`, so an id the door cannot address refuses the boot before the dial (`provision.rs`, `parse_deployment_owners`).
- `provision::ensure_deployment_owners` writes an active owner row in the deployment's own space (`ORDO_SPACE`, the space `std.space.create` is bound to) for each principal named. It reuses `MemberOps::admit_row`, whose `WHERE NOT EXISTS` on the principal makes it idempotent: a second boot writes nothing and the row keeps its id.
- It runs in `assemble` after `ensure_system_space` and before the door reads `members`, so the boot that writes the row serves it. It runs again after `ensure_system_space` in the deploy listener, so a deployment that booted against an empty database (no `members` table yet, the ordinary first minute) gets its owners on the first `ordo apply` the node sees.
- It waits, and says so in the log, when the `members` directory does not exist or the space has no key custody. The custody check keeps a mistyped `ORDO_SPACE` from gaining member rows.
- A principal that already holds a lower role or a suspended row is left alone and the log names it. A role change or a suspension is somebody's act.
- Recorded as `Enrolment { act: "deployment-owner" }` on the deployment stream, mirrored to the space's, only when a row was written.
- It adds and never removes. A removed owner stays removed while the variable does not name them, and is added again when it does.

**How a deployment owner who has not signed in yet is named.** By principal id, as the plan says. A principal id is minted at registration, so it cannot be known before the person has registered. The person registers on the app's page like anyone, the deployment owner reads their id, sets the variable and restarts. The row is written whether or not the principal exists, and it waits for the person. A ticket the server mints and prints was not built. It would need a ticket kind, a redemption route and a place to print it, and that is more than P0 asked.

Files: `crates/ordo-host/src/provision.rs`, `crates/ordo-host/src/runtime.rs` (config field, boot block, deploy listener, `report_deployment_owners`), `docs/deploying.md` §1 table. `RuntimeConfig` gained a field, so the four struct literals (`ordo-cli/src/dev.rs` and three host tests) name `deployment_owners: Vec::new()`.

Tests, in `crates/ordo-host/tests/provision_own.rs`, run against the assembled runtime: `a_principal_named_in_the_environment_owns_the_home_space_after_boot`, `the_variable_adds_and_a_second_boot_writes_nothing`, `a_removed_owner_is_not_re_added_unless_named_again`, and beside them `a_deployment_owner_is_recorded_when_written_and_not_when_found` and `the_variable_is_parsed_strictly_and_kindly`. The three named tests fail with the boot call taking an empty list.

### P1 · `provision_own` and the latch

- `tenants.rs`: the `$spaces` table gains `own_of text NULL` as its last column (added to a table that predates it, like the lifecycle columns) and the partial unique index `"$spaces_own_of"` over `own_of` where `state IN ('provisioning','active')`. The registry is `$spaces`, not `$tenants` as this plan wrote. `reserve` takes `own_of: Option<&str>` and now answers whether it wrote the row; `false` means the latch held. `tenants::own_of(pool, principal)` returns the live own tenant. `Tenant` carries `own_of`. Every reader selects one column list, `COLUMNS`.
- `tenant_ops.rs`: `TenantOps::provision_own(principal, display)` runs the phases `create` runs (preflight, registry row, `provision_prepared`, activation, `announce_members`) with the principal as owner and creator, the display as the name (else the principal id), the default cluster only, no `dryRun`, and no home-space check. It answers `OwnTenant::Created`, `Existing` or `InProgress`. The latch is looked at before preflight, again after it (preflight may have waited on the apply lock), and then the reserve decides. A failure after the row exists marks it `failed` and returns the error naming the row. Recorded as `Enrolment { act: "own-tenant" }` with `result=created` or `result=failed`.
- One own-tenant provision runs at a time per process (`own_gate`). Provisioning is already serialised deployment-wide by the apply lock, and every waiter on that lock holds a pooled connection, so a burst of sign-ups would spend the control pool on waiters. The gate holds no connection while waiting.
- No `now` parameter: `provision_own` reads the clock as `create` does.

Tests in `provision_own.rs`: `a_person_gets_one_own_tenant_however_many_times_it_is_asked` (eight racing calls, one active tenant, one database), `a_failed_own_tenant_does_not_block_the_next_attempt` (an in-progress row is reported, a real custody failure leaves a `failed` row, the next attempt provisions), `an_own_tenant_has_the_serving_schema_a_database_of_its_own_and_its_person_as_owner`, and `the_registry_admits_one_live_own_tenant_per_principal`, which races eight `reserve` calls straight at the index and fails with the index removed (8 rows written instead of 1).

**Measured, 2026-09-29, local Postgres 17, one provision:** 109 ms wall time for a `CREATE DATABASE`, the schema apply at the serving version, a custody key and the founding row. The notes app measures it live.

**What proved wrong against the code.** The plan says the racing test needs the index. With the apply lock held from preflight through reserve, and the latch looked at again after preflight, the eight racing `provision_own` calls end with one tenant even without the index, so the index is what holds if that lock ever narrows, and the `reserve` race test is the one that proves it. The plan also wrote `provision_own(principal, display, now)` and the registry as `$tenants`; the code has `$spaces` and reads its own clock.

### P2 and P3 · The setting, the hook and the limits

Built 2026-09-29 on branch `worktree-agent-ae5e90d36e73363ea`, in one commit because the limits and the hook share one module and one table.

**The setting** (`crates/ordo-host/src/self_serve.rs`). `ORDO_PROVISION_SELF_SERVE` is `off` (default) or `on-signup`. It lives in `$deploymentsetting` on the claim code's pattern: the environment seeds it on a boot that finds nothing stored, `std.deployment.setSelfServe` writes it, the stored value wins after that, and a disagreeing environment is named in the log. The op is owner-only, answers only in the deployment's own space, and is named in `runtime::platform_specs`. `RuntimeConfig` gained `provisioning: SelfServeConfig`, so the struct literals in `ordo-cli/src/dev.rs` and four host tests name `provisioning: Default::default()`.

**The limits are settings, not system values.** `ORDO_PROVISION_SIGNUPS_PER_HOUR` (default 60) and `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR` (default 5) are stored beside the mode and changed by the same op (`signupsPerHour`, `signupsPerAddress`; a number or `unlimited`). The two defaults are the owner's, 2026-09-29. Nothing is a ceiling: `unlimited` lifts either.

**The hook** (`auth/signup.rs`, called once in `/auth/complete` after the token is minted, and once in `/auth/register`). Under `on-signup`: presence first (a sign-in with none, while `std.space.create` still requires it, answers `provisioning: "presence required"`), then the person's own tenant is looked up (`active` answers its id, `provisioning` answers `in progress`), then the provision limits are checked, then `TenantOps::provision_own`. Failure answers `provisioning: "failed"`. The sign-in succeeds in every case. The answer gains `space` and `provisioning` wherever provisioning is wired, and they are null under `off`.

**The limits** count under `on-signup` only. A registration without a ticket is one sign-up, counted before the ceremony begins (`429 quota_exceeded`, nothing minted). A provision is counted separately with the same two limits, and a refusal there answers `provisioning: "limit reached"`. A person who already holds a tenant is not counted. The count is a sliding hour in `$signup`, so every replica reads the same rows. The check and the insert are serialised in one process, so two replicas can each let one more through. The address is the socket's peer, taken from connection info the server now passes to the router (`into_make_service_with_connect_info`); a request with no address is held to the deployment-wide limit only. `/health/deep` carries `own_tenants` and the startup log carries a `provisioning` line.

Files: `self_serve.rs` (new), `auth/signup.rs` (new), `auth.rs` (a `signup` cell and `wire_signup`, the `Peer` argument on `register` and `complete`, one call in each), `platform_ops.rs` (spec, dispatch, owner-only), `runtime.rs` (config field, wiring, census line, connection info), `serve.rs` (`own_tenants`), `docs/deploying.md` (three rows).

Tests, in `crates/ordo-host/tests/provision_own.rs` (`mod signup`) and `deployment.rs`: `a_first_sign_in_under_on_signup_answers_a_token_and_a_space_the_token_can_use`, `a_second_sign_in_answers_the_same_space`, `under_off_the_answer_has_no_space_and_no_tenant_exists`, `a_sign_in_without_presence_provisions_nothing_by_default`, `the_setting_changed_through_the_op_is_what_the_next_boot_reads`, `the_sixty_first_sign_up_in_an_hour_is_refused_and_no_principal_is_minted`, `the_sixth_sign_up_from_one_address_is_refused_and_the_seventh_from_another_is_not`, `a_limit_is_a_setting_the_app_developer_changes`, `the_hook_is_limited_and_answers_limit_reached`, and in `deployment.rs` `deep_health_counts_own_tenants`. With the hook and the register check made no-ops, seven of the nine `signup` tests fail; the two that pass are about the store and about `off`, which the hook does not touch.

Where the plan differs from the code:

- **The token is not shown calling an op in the new tenant.** The test reads the tenant's own `members` table and finds the token's principal as its one owner. Calling an op through the door needs `Routing::Attached` with a credential directory, so the door can read the new tenant's members from its own database. That is a boot with more wiring than this increment adds, and it is P1's `announce_members` path.
- **Two counts, not one.** The plan says both limits are checked "at `/auth/register` and at the hook". Counting a fresh sign-up in both would spend two of the 60 on one person, so each place has its own count.
- **`std.deployment.setSelfServe` also carries the limits**, so the app developer has one op for the whole setting.
- **`Signup` needs `TenantOps`**, which `platform_ops` holds, so the runtime hands the same `Arc` to the auth state. Sign-in with the hook wired is therefore available only where `std.space.create` is.

### P4 · A person's own list of tenants

`GET /auth/spaces` with the session's bearer token answers `[{space, name, role, via}]` for the token's principal and nobody else's. It is `auth/spaces.rs` (new), one line in each router (`auth.rs`, `serve.rs`), and a `TenantLister` the runtime wires over the door's live engine (`SwappableAccess::current`) and the control pool for names. There is no parameter that names a person.

- `via` is `member` for a seat with a role and `guest` for a seat with none, with `role: null`. This is G6's half of the same route, built with it because it is one line of the mapping. Its test is with the guest tests in `sharing.rs`.
- Only **active** seats are listed. A suspended seat, which is what a guest who left or was revoked becomes, is not.
- A capability token, and a delegation, answer `[]` with 200. They are an app's or a window's authority, not the person asking. A missing or unverified token, and a session that was ended, are the one 401.
- The plan put a query in `member_ops.rs` "across the databases served". The door's engine already holds every served tenant's members (E1, and `std.person.tenants` reads the same), so the route reads that and needs no second query. The cost is that the list is as fresh as the door, which is the property the test wants: removed means absent.

Tests in `provision_own.rs` (`mod signup`): `a_person_lists_their_own_tenants_and_a_capability_token_lists_nothing` and `the_list_names_no_tenant_the_person_was_removed_from`. Both fail with the route unmounted (404).
