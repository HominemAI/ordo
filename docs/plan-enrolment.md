# Enrolment: how a person gets a credential and becomes a member

This plan reflects the owner's answers of 2026-09-29 (`corpus-rulings.md`,
*Enrolment* and *Sign-in through outside providers*).

Companion to `plan-passkey-signin.md` (built in parallel with this; its §7
names every file both touch), `plan-auth-federated.md` (SAML, OAuth2, email
code) and `plan-passkey-recovery.md`, and to
`security-logging-design.md` §2 and §3, `definition-language.md`'s `members`
table, and checklist 20b and 21b.

Markers: **[settled]** ruled by the owner · **[plan]** proposed here ·
**[open]** named and not answered.

Status: **E1 to E7 built**, and both open questions answered (see *Built*, at the end).
Every claim in the plan about the code was read against the tree on
2026-09-29.

---

## 0 · The starting state, verified

**There are three things, and the code has two and a half of them.**

| Thing | What it is | Where it lives today |
|---|---|---|
| a **credential** | what proves somebody is who they say: a password hash, a passkey | deployment-wide. `$credential` and the directory's authenticator links |
| a **principal** | a person (or agent, or service) known to the deployment, `usr_…` | deployment-wide. `Directory::create_principal` mints `{prefix}_{id}` (`directory.rs:435`) |
| a **membership** | that person admitted to one tenant, with a role and a status, `mem_…` | per tenant. The platform's `members` entity, which has both an `id` and a `principal` column |

**The half that is missing is the join between the last two.** The door
builds its engine from `MEMBERS_SQL`, which selects `$space, id, role,
status`, and keys each membership by `members.id`
(`ordo-host/src/lib.rs`, `engine_from_members`). A sign-in mints a token
for the **principal** the directory resolved. Nothing reads
`members.principal`, so a principal `usr_abc` is looked up among memberships
keyed `mem_…` and is a stranger in every tenant. It works in tests and in
`ordo dev` because both hand themselves a token whose subject *is* the
member id.

So today one person cannot be a member of two tenants under one identity,
and that is the exact case the owner asked about.

**Four more facts the plan depends on.**

1. **Nothing can create a credential.** `CredentialStore::put` is reached
   only through `LocalPasswordProvider::enroll`, and every caller of
   `enroll` is a test (checklist 20b).
2. **Nothing can create a second member.** The only production writer of
   `members` is provisioning's founding owner (checklist 21b).
3. **Auto-onboarding exists and stops one step short.**
   `Directory::resolve_or_onboard` under `Provisioning::AutoOnboard` mints a
   principal and links the authenticator on first sign-in. It creates no
   membership, so the person it admits to the deployment is admitted to
   nothing in it.
4. **The policy is deployment-wide.** `Provisioning` is one value for the
   whole deployment (`ORDO_AUTH_PROVISIONING`, `invite-only` |
   `auto-onboard`), documented as *"per deployment (design §3)"*.

---

## 1 · What the owner ruled [settled]

2026-09-29, verbatim:

> Service owns the credentialing we need auto-provisioning.

> In terms of passkey it does not make sense to enforce a particular flow -
> in that way it's not easy to use and therefore makes a bad app.

> Invitation does make sense - but it should not stop signups from existing
> if a tenant chooses it or if they provision 3rd party auth systems that
> auto-provision - each new sign in is an enrollment (auto enrollment)
> unless they want to manually provision and invite.

> In single tenant deployment it makes sense that first come first served -
> but in a multi user setting - we may want something more.

> Also what about multi-tenant users. This is another layer where
> credentialing can't explicitly stay scoped to just a tenant but needs some
> control or management from the system above.

On passwords:

> before the user exists the tenant creates that credential for the user -
> then the user can own it. If the user needs to reset a password that's a
> little different but the tenant should have the power for it and should be
> logged appropriately. Sovereignty is the weird part so a deployment may
> need to own it at the service level. It is inately less secure and
> therefore passkey is a better option.

**Read as principles**, which is how a ruling is applied everywhere it
reaches:

| | Principle |
|---|---|
| **P1** | Credentials and identities belong to the **deployment**. A tenant never stores one |
| **P2** | **Admission** to a tenant belongs to the **tenant**, and the tenant chooses how open it is |
| **P3** | The default is that a sign-in **is** an enrolment. Anything stricter is a choice somebody made |
| **P4** | No flow is compulsory. Invitation, open sign-up and auto-provisioning from an outside provider can all be on at once |

**An invitation lets a newcomer sign up under invite-only** (owner ruling
2026-10-09, federated ask 9; checklist 25ar). It works with every method:
passkey, password, emailed code and an outside provider. `/auth/complete`
checks the invitation before the identity step: unspent, unexpired, for the
named tenant, and naming nobody. It spends it in the same statement, then
mints the person (`Onboarding::Invited`) and admits them at its role. Two
sign-ups racing with one invitation spend it once. Without a valid one,
invite-only refuses with the one answer, as before. The sign-up ceiling,
the human check and a deleted account's links apply as to any sign-up.
| **P5** | A tenant has power over credentials it created, and every use of that power is recorded |
| **P6** | One person, one identity, many tenants. What one tenant does must not reach into another |

P5 and P6 collide. §4 is where that is settled, and the owner confirmed
the settlement (A1 below).

**The owner's answers of 2026-09-29** to this plan's questions, and two
answers given on `plan-auth-federated.md` that reach into it:

| | Answer | Where it lands |
|---|---|---|
| **A1** | A tenant resets a password only for a person it created who belongs nowhere else. *"Agreed."* | §4.2, now settled |
| **A2** | A stranger admitted by an open tenant is a **viewer** by default. *"This is app dev territory but default should be viewer."* | §2.2 |
| **A3** | The first-person claim code is a setting: *"This should be managed via console / api / envs."* No default named; off until the owner says otherwise | §3, E5 |
| **A4** | A typed temporary password is a recorded option, off by default. Recommendation taken | §4.1 |
| **A5** | When a tenant is deleted, the people it created become independent if they ever signed in and are removed if they never did; people who came through the tenant's own registered provider are destroyed with it; a platform sign-in remains and what it then reaches *"thats an app dev's decision"* | §4.6, E7 |
| **F3** | A registration with no admission is kept for a set time and then removed: *"it should be registration and they stay there for some x amount of time and then get nuked... this is really up to the app dev."* The time is a deployment setting | §2.1, E4 |
| **F5** | No automatic joining of accounts. *"Existing account logs in and claims another account."* | §2.1, one sentence; the mechanism is `plan-auth-federated.md` F13 |

---

## 2 · The model [plan]

### 2.1 Two acts that today are one word

"Enrolment" has been covering two different acts, and separating them is
what makes every ruling above expressible.

- **Enrolling an identity** is a deployment act. A credential is verified,
  and if the deployment has not seen it before a principal is minted for
  it. This is what `resolve_or_onboard` already does.
- **Admitting a member** is a tenant act. A principal is given a membership
  in one tenant, with a role. Nothing does this today.

P3 says the first is automatic. P2 says the second is the tenant's to
configure. They are both "auto-provisioning" and they are configured in
different places.

**A registration that no tenant admits is kept for a while, then removed
[settled, F3].** The owner chose the middle way between this plan (keep
every enrolled identity) and `plan-auth-federated.md` §2.3 (mint nothing
until admitted). An identity enrolled at sign-in and admitted nowhere is
kept for `ORDO_AUTH_REGISTRATION_TTL_SECS`, a deployment setting, and then
removed with its links, its passkey rows and its credential, by a
supervised worker on the `access.refresh` pattern. Admission anywhere
before the sweep keeps it. The default is 90 days, ruled by the owner on
2026-09-29 (question 6). E4 builds the sweep.

**Two identities join only when the person joins them [settled, F5].** A
signed-in person completes a ceremony on the other method and the two
authenticators become one principal. Nothing joins by email or by name.
The mechanism is `plan-auth-federated.md` F13; this plan only states the
rule so that no admission path assumes otherwise.

### 2.2 Admission policy, per tenant

One setting on the tenant, and the values are not exclusive (P4):

| Way in | What happens on a sign-in by somebody who is not a member |
|---|---|
| **open** | they are admitted, with the tenant's default role, which is **viewer** unless the app developer set another (A2) |
| **invitation** | they are admitted if they hold an invitation, with the role the invitation names |
| **provider** | they are admitted if they arrived through a named outside provider, optionally only from named domains, with the role the rule names |
| **first-comer** | they are admitted **as owner** if and only if the tenant has no members at all |
| none of the above | they are refused, with the door's one answer |

A tenant may switch on any combination. A tenant with nothing switched on
admits only people an administrator adds by hand, which is *"unless they
want to manually provision and invite"*.

### 2.3 What the deployment keeps

`ORDO_AUTH_PROVISIONING` stops being the policy and becomes the **ceiling**:
which ways in a tenant on this deployment is allowed to switch on. A
deployment that must never admit a stranger sets the ceiling to invitation
and no tenant can open itself wider. This keeps the earlier ruling that it
be *"configurable to some degree, different info sec has different levels
of compliance"*, and puts the compliance control where a compliance officer
would look for it.

### 2.4 The door resolves a person to a member

On every request the door has a verified principal and a tenant named by
the request. It needs the membership row whose `principal` is that person,
in that tenant. The engine is rebuilt to key memberships by
`(space, principal)` and to carry the member id, which is what `$caller`
binds to and what guards compare against. One principal then resolves to a
different member id in each tenant, which is the multi-tenant case working
as designed rather than by coincidence.

---

## 3 · First come, first served [plan]

**Superseded on 2026-09-29.** The owner: *"setup belongs on the server side
end of story."* The deployment's own owner is established on the server side
and never by being first to arrive, so first-comer is **off by default
everywhere**, the deployment's own tenant included. It stays as a mechanism an
app developer may switch on. The paragraphs below are the design as it was
first written; where they say "on by default", read "off until somebody turns
it on". *Built*, at the end, says what a deployment can do today to name its
first owner.

The rule the owner first gave for a single-tenant deployment: the first person
to sign in owns it.

**It is the `first-comer` way in of §2.2**, and it needs one guard to be
safe on a public address. Between the moment a deployment is reachable and
the moment its owner signs in, anybody who finds the address can be first.
For a laptop or a private network that window does not matter. For a
public deployment it is the whole attack.

First-comer closes itself permanently the moment it is used. A deployment may require a
**claim code** for it, printed once in the startup log, so that being
first also requires having read the log.

**The claim code is a setting [settled, A3]:** *"This should be managed via
console / api / envs."* So it is one value, `first_comer_claim_code`,
readable and writable three ways: the environment at boot
(`ORDO_AUTH_CLAIM_CODE=required|off`), a platform op bound to the system
space that the console will call, and the console itself when it exists
(18o). Off by default, because the owner named no default and the ruling
is that no flow is compulsory; on in the go-live guide, since that guide
is for a public address.

In a multi-tenant deployment a new tenant is created by somebody who is
already signed in, and that person is its founding owner, so there is no
window and first-comer is not needed.

---

## 4 · Passwords: the give, the reset, and sovereignty [plan]

### 4.1 The give

The owner: *"before the user exists the tenant creates that credential for
the user, then the user can own it."*

**Built as a ticket, not as a password.** An administrator creates the
person and receives a one-time **enrolment ticket** to pass on. The person
redeems it and sets their own password. Two things follow from doing it
this way round:

- the administrator never knows the password, so *"then the user can own
  it"* is true from the first second rather than after a forced change;
- the same ticket is what an invitation is, and what a first passkey
  registration needs, so one mechanism serves three flows and none of them
  is compulsory.

A deployment that needs the other shape, an administrator setting a
temporary password by hand, has it as an option **[settled, A4]**: off by
default, switched on per deployment, forcing a change at first sign-in,
and recording that a temporary password was issued and by whom.

**There is no mail channel in the platform.** A ticket is shown to the
administrator, who delivers it however they already reach that person.
When a mail channel exists (`plan-auth-federated.md` needs one for email
codes) the ticket can travel on it.

### 4.2 The reset, and the collision between P5 and P6

A tenant *"should have the power for it"*. A credential belongs to the
deployment and unlocks every tenant that person is in. So an administrator
of one tenant resetting a password would be reaching into every other
tenant that person belongs to.

**Settled by who created the identity [settled, A1: *"Agreed."*].**

| | A **managed** identity | An **independent** identity |
|---|---|---|
| Created by | a tenant's administrator, through a ticket | the person themself, by signing up or arriving through a provider |
| Who may reset its credential | that tenant's administrators, and the person | the person, and the deployment owner |
| What a tenant it belongs to may do | everything | suspend or remove **the membership**. Nothing to the credential |

A managed identity that is later admitted to a second tenant **stops being
managed**, at that moment and on the record, because from then on a reset
by the first tenant would reach into the second. The first tenant keeps
every power over the membership and loses the power over the credential.
This is the owner's sentence, *"sovereignty is the weird part so a
deployment may need to own it at the service level"*, made into a rule:
the deployment owns every credential, and lends control of one to a tenant
for exactly as long as that tenant is the only place it is used.

### 4.3 What a reset does

It issues a new ticket, invalidates the old credential, and **ends every
session that person has**. The last of those needs a capability the
platform does not have: there is logout for one session and no way to end
all of them (checklist 20b lists *"no sign out everywhere"*). A reset that
left the old sessions running would be a reset an attacker could outlast,
so ending them is part of this work and not a follow-up.

### 4.4 What is recorded

Every act below writes to the security event stream, naming who did it, to
whom, and under which authority. An act on a credential is recorded at
**deployment** scope, since the credential is the deployment's, and
mirrored to the tenant whose administrator performed it, so that tenant's
owner can read it without being shown anybody else's.

Identity enrolled · member admitted, and by which way in · ticket issued ·
ticket redeemed · ticket expired unused · credential reset, and by whom ·
identity changed from managed to independent · membership suspended,
reinstated, removed · first-comer claim used.

### 4.5 What a password can never do

Unchanged, and restated so nobody expects otherwise: a password proves a
secret and not that a person is present, so a password session cannot
deploy a program, create a tenant, or import, export or delete one. The
owner's view that a password *"is inately less secure and therefore passkey
is a better option"* is already how the platform behaves. A second step
after a password (`plan-second-factor.md`) changes who can sign in and not
this.

### 4.6 When a tenant is deleted [settled, A5]

`std.space.delete` ends memberships. What it does to the identities the
tenant created is the owner's answer, in three rows:

| The person | What happens | Why |
|---|---|---|
| a **managed** identity that has signed in at least once | becomes **independent**, on the record. Credential, links and passkeys stay | deleting it outright could destroy their only way into tenants they have since joined |
| a **managed** identity that never signed in | **removed**: credential, links, passkey rows, unused tickets | nobody ever held it |
| an identity that came through the tenant's **own registered provider** (`plan-auth-federated.md` F12, a tenant-scoped connection) | **destroyed with the tenant**: the connection is retired and every `oidc:<that connection>:…` or `saml:<that connection>:…` link goes with it. A principal left with no link is removed | the tenant's provider was the only thing vouching for them |
| the same person, if they also hold a platform sign-in (a passkey, a password, a deployment-wide provider) | the platform sign-in **remains**; what it then reaches is *"an app dev's decision"*: nothing, until some tenant admits them | the identity is the deployment's (P1) |

Every row is recorded at deployment scope: `IdentityReleased` (managed to
independent), `IdentityRemoved { reason }`. E7 builds it.

---

## 5 · Increments [plan]

Each is independently shippable. Each done-when names a test that fails
without the increment.

| | What | Done when |
|---|---|---|
| **E1** | The door resolves `(principal, tenant)` to a member. `MEMBERS_SQL` selects `principal`; the engine keys by it and carries the member id | one principal with memberships in two tenants is served in both with a different `$caller` in each, and refused in a third |
| **E2** | Enrolment tickets: minted, single-use, expiring, redeemable once | a redeemed ticket cannot be redeemed again; an expired one is refused with the door's one answer |
| **E3** | Membership commands: admit, change role, suspend, reinstate, remove, each announcing on `ordo.members` so every server process refreshes at once | a suspension through the command is refused on the next request, not after the refresh period; the last owner cannot be removed |
| **E4** | Admission policy per tenant, under the deployment's ceiling; the default role, viewer unless set (A2); **the sweep of registrations no tenant admitted** (F3): `ORDO_AUTH_REGISTRATION_TTL_SECS`, a supervised worker | a tenant set to `open` admits a stranger's first sign-in as a viewer; the same tenant under an invitation-only ceiling cannot be set to `open`; `a_registration_admitted_nowhere_is_removed_after_the_ttl_and_one_admitted_in_time_is_kept` |
| **E5** | First-comer, with the claim code as a setting reachable by environment, op and console (A3) | the first sign-in to an empty tenant owns it; the second does not; with a claim code required, a first sign-in without it is refused; `the_claim_code_setting_changed_through_the_op_is_what_the_next_boot_reads` |
| **E6** | The password give and the reset, with managed and independent identities, the typed temporary password as a recorded option off by default (A4), and ending every session | an administrator resets a managed identity and its old sessions stop; the same administrator is refused for an independent one; an identity admitted to a second tenant becomes independent; `a_temporary_password_is_refused_unless_the_deployment_allows_it_and_is_recorded_when_it_does` |
| **E7** | What tenant deletion does to identities (§4.6, A5) | `a_deleted_tenants_managed_identity_that_signed_in_survives_as_independent`; `one_that_never_signed_in_is_removed`; `an_identity_from_the_tenants_own_provider_is_removed_with_it_and_its_platform_sign_in_remains` |

**E1 comes first** and everything else stands on it. Passkey sign-in
(`plan-passkey-signin.md`, S0 to S7) is built in parallel and needs E1 to
serve a member and E2 to register by ticket; its §3 says what it takes
from each increment and its §7 names every file both plans touch and who
lands first. Its stores (S1) and its SDK module (S6) touch nothing this
plan touches. Ending every session (E6) is recovery R1's mechanism; whoever
ships first builds it (`plan-passkey-recovery.md` §4.3).

---

## 6 · Questions for the owner: four answered, two open

1. **The default role for somebody admitted through an open tenant.**
   **Answered 2026-09-29: viewer**, and the app developer may change the
   default (*"This is app dev territory but default should be viewer."*).
2. **Whether first-comer needs a claim code on a public deployment.**
   **Answered 2026-09-29: it is a setting**, *"managed via console / api /
   envs."* Off by default until the owner names a default. §3.
3. **Whether an administrator may set a temporary password by hand.**
   **Answered 2026-09-29: yes**, as a recorded option, off by default. §4.1.
4. **What happens to a managed identity when its tenant is deleted.**
   **Answered 2026-09-29:** independent if it ever signed in, removed if
   never, and an identity from the tenant's own provider is destroyed with
   it. §4.6.
5. **Whether a person can see which tenants they belong to.**
   **Answered 2026-09-29: yes**, to the person themself and to nobody else.
   Built as `std.person.tenants` (see *Built*).
6. **The default for `ORDO_AUTH_REGISTRATION_TTL_SECS`** (§2.1, E4).
   **Answered 2026-09-29: 90 days**, configurable. The plan had recommended
   7 days and the owner chose longer.

---

## 7 · What this plan does not do

- It does not design SAML, OAuth2 or email codes
  (`plan-auth-federated.md`), or what happens when a passkey is lost
  (`plan-passkey-recovery.md`).
- It does not wire passkey sign-in. That is `plan-passkey-signin.md`,
  built in parallel.
- It does not build a mail channel. Tickets are delivered by a person until
  one exists.
- It does not build the console. Every setting here is reachable through an
  op, which is what a console will call.

---

## Built

E1 to E6, in order, one commit each, on the branch `worktree-agent-adf80ffa1657889ab`.
The two answers of 2026-09-29 and E7 follow them, at the end of this section.
Where the plan turned out wrong against the code, the entry says so.

### E1 · The door resolves a person and a tenant to a member

`MEMBERS_SQL` selects `principal` as its **fifth** column (new columns go at
the end, every reader indexes by position). `engine_from_members` keys each
membership by `(space, principal)` and carries the member id
(`AccessEngine::set_member`, `member_id_of`, `principal_of_member`). The door
hands the executor the member id as `$caller`. A row with no principal, and
the old four-column shape, are keyed by their id, so tokens minted for member
ids still work.

- `crates/ordo-identity/src/access.rs`, `crates/ordo-host/src/lib.rs`,
  `platform_ops.rs` (`callersOf` resolves its asker from member id back to
  principal).
- Test `crates/ordo-host/tests/enrolment.rs`:
  `one_principal_in_two_tenants_is_a_different_member_in_each_and_a_stranger_in_a_third`
  and three more. Three of the four fail against the old `lib.rs`.

**The plan was wrong about who depends on the old behaviour.** Not only the
tests. The conformance seed wrote `principal` `p1` to `po` beside ids `mem_1`
to `mem_o`, `ordo dev` seeded `principal` `dev` beside id `mem_dev`, and
`docs/quickstart.md` taught `('dev', 'mem_1', 'p1', ...)`. All of them minted
tokens for the id, so all of them stopped working when the door began to read
the column. They now write principal equal to id (`seed.sql`, `dev_db.rs`,
`quickstart.md`, and two tests). Two things are left inconsistent and are named
here. Security-stream subjects recorded by the platform ops (`exit_ops`,
`attest_ops`, `bucket_ops`) are the member id, and the ones the door records
are the principal. `std.space.create` still writes the founding row with member
id equal to the principal (`tenant_ops.rs`); it no longer has to, and giving it
a minted `mem_...` id is a separate change.

### E2 · Tickets

`crates/ordo-host/src/tickets.rs`: `TicketStore` (memory and Postgres),
`Tickets` (issue, redeem, sweep), `redeem`, `redeem_for`, `Want`. 256 random
bits, only the SHA-256 stored, three days by default and thirty at most.
Single use is one `UPDATE ... WHERE "redeemed_at" IS NULL RETURNING`. Unknown,
expired, spent and malformed are one `TicketError::Refused`. New: `pgsync.rs`
(the one-connection bridge the new stores share), `enrolment.rs` (records
enrolment acts), and one event kind, `EventKind::Enrolment { act, detail }`
(chain tag 15) in `ordo-secevent`.

- Tests `tests/tickets.rs` (12): including sixteen threads racing one ticket
  across two Postgres stores, and a table that holds a hash. Five of the first
  ten fail with the single-use marking removed.

**The seam differs from `plan-passkey-signin.md` §7.** That plan names
`redeem(ticket, now)`. A function with no store cannot redeem, so the store is
the first argument: `tickets::redeem(store, ticket, now)`, and `Tickets::redeem`
holds it. `Redeemed` has the three fields that plan reads (`principal`,
`purpose`, `display`) and two more (`space`, `role`).

A ticket presented by the wrong person, for the wrong tenant or for the wrong
purpose is refused **without being spent** (`Want`, added in E4), so nobody can
burn another person's ticket by presenting it.

Not wired: the route that lets a person redeem a **password** ticket. It is
`CredentialOps::redeem_password` (E6), a function, because a route is `auth.rs`
or `serve.rs`, which are passkey sign-in's.

### E3 · The membership commands (checklist 22h)

`crates/ordo-host/src/member_ops.rs`: `std.space.admit`, `setRole`, `suspend`,
`reinstate`, `remove`. Owner-only. `admit` mints a `mem_` id. The last active
owner cannot be removed, suspended or demoted, with the owner rows locked
inside the writing statement. Each command rebuilds the door on this replica
through a hook the runtime hands in (`refresh_access`) and announces on
`ordo.members`, so a suspension is refused on the next request.

- Test `tests/membership_commands.rs` (6), against Postgres, through the real
  door with a `SwappableAccess` and the same refresh hook the runtime wires.
  Three of six fail with the refresh skipped and the owner guard removed.
- Error codes: no new ones. Refusals use `tenant_refused`, `not_found`,
  `duplicate` and `has_referrers`. A `member_refused` code would be clearer and
  costs an edit to the wire inventory.

**A consequence the plan did not state.** Owner-only platform ops are also
step-up ops (`engine_from_members`), so a **password** session cannot admit,
suspend or remove anybody. Only a passkey session can. The plan says a password
session cannot deploy, create a tenant or import, export or delete; it did not
say a password session also cannot administer members. Whether it should is a
question for the owner.

### E4 · Admission policy, the ceiling, the sweep

`crates/ordo-host/src/admission.rs`: `Way`, `Ceiling`, `Policy`, `PolicyStore`
(`$admissionpolicy`), `Admission::on_arrival`, `set_policy`. Ops
`std.space.setAdmission` and `std.space.admission`, owner-only. The default
role is `viewer` (A2) and the app developer may change it. An invitation is
always honoured; open, provider rules and first-comer are the choices. The
ceiling is `ORDO_AUTH_PROVISIONING` read as what it now is, and is checked when
a policy is set and when it is used.

`registrations.rs`: the sweep (F3), a supervised leader-only worker. It removes
an identity older than the lifetime that no tenant admitted, with its links,
passkey rows, password, tickets, and (E6) its managed and temporary rows. It
removes nobody when any database it must read for members cannot be read.

- Tests `tests/admission.rs` (7) and `tests/registrations.rs` (3),
  `a_registration_admitted_nowhere_is_removed_after_the_ttl_and_one_admitted_in_time_is_kept`
  among them. Seen failing with the ceiling check removed and with the admitted
  filter removed.

**The registration lifetime had no default when this was built.** It has one
now: see *The registration lifetime defaults to 90 days*, below.

**The plan was wrong that E4 rewrites `runtime.rs` 410-419 and 2020-2027.** The
ceiling is derived from `AuthConfig.auto_onboard` at E4's own wiring site, so
neither shared site was touched, and `Provisioning` still governs whether an
unknown authenticator is enrolled as an identity.

**`on_arrival` has no caller yet.** `/auth/complete` signs a person in to the
deployment and names no tenant, and `auth.rs` is passkey sign-in's file. The
sign-in route must call it once a sign-in names its tenant, passing the ticket
or the provider it arrived through. No route was added to `serve.rs`.

### E5 · First come, first served

In `admission.rs`. The first-comer way: the first arrival at a tenant with no
members owns it. The latch is a row in `$firstcomer` inserted with `ON CONFLICT
DO NOTHING`, so of eight racing arrivals exactly one wins, and a tenant emptied
by hand does not open again. Bounded by the ceiling. It was built on by
default for the deployment's own tenant and is now off there too: see *First
come, first served is off by default*, below.

The claim code is one setting, `ClaimStore` (`$deploymentsetting`):
`ORDO_AUTH_CLAIM_CODE=required|off` seeds it on a boot that finds nothing
stored, `std.deployment.setClaimCode` (the deployment's own space, owner-only)
writes it and returns a new code once, and **the stored value wins after that**.
An environment that disagrees is named in the boot log. Only a hash is stored.
Off by default, as the owner left it. The console is not built and will call
the op.

- Test `tests/first_comer.rs` (8), including
  `the_claim_code_setting_changed_through_the_op_is_what_the_next_boot_reads`.
  Four fail with the latch, the code check and the count check removed.

Also fixed, in E5: `runtime::platform_specs` answers `false` for a name it does
not know, so E4's ops were absent from a real deployment. A unit test in
`runtime.rs` now names every membership and admission op.

**A finding outside this plan.** That same filter does not name
`std.space.create` or `std.space.reap`, so the binary's registry does not seem
to serve either. `platform_ops::specs()` defines them and `platform_ops_for`
wires their state, but the census of a running deployment lists neither.

### E6 · The password give, the reset, managed and independent identities

`crates/ordo-host/src/credential_ops.rs`. `std.space.createPerson` creates the
person, admits them, marks them managed by the tenant (`$managed`) and returns
a ticket once. The person redeems it with `CredentialOps::redeem_password`; a
taken identifier is refused before the ticket is spent.
`std.space.resetCredential` works only for a person the tenant created who
belongs nowhere else. One refusal covers never managed, released and belongs
elsewhere, and the rule is checked against every database served and fails
closed. Every admission calls `MemberOps::admitted`, which releases a managed
person admitted to a second tenant, on the record. A reset removes the password
and its link, ends every session, and issues a new ticket. Passkeys are left
alone. `std.space.setTemporaryPassword` is the recorded option, off unless
`ORDO_AUTH_TEMPORARY_PASSWORDS=allow`.

**Ending every session** is built here, as `plan-passkey-recovery.md` §4.3
expected of whichever plan ships first: `SessionRevocations::principal_ended`
and `end_all`, a `$principalepoch` table beside the deny list, checked by the
door and by `session::refresh`. `end_all(at)` ends sessions that started at or
before `at`, so a tie goes to the safe side, and it is monotonic. Recovery R1
can call it.

- Test `tests/credential_reset.rs` (7). Five fail with the session end, the
  managed check, the elsewhere check and the allow check removed.

Deviations and gaps, stated:

- The managed state is a table beside `$principal`, not columns on it, so
  `principal_directory.rs` and its `PRINCIPAL_DDL` are untouched. "Ever signed
  in", which E7 needs, is not recorded.
- **A temporary password is marked must-change, and nothing enforces it at
  sign-in yet.** `ManagedStore::must_change` is the question the sign-in route
  asks; the route is `auth.rs`.
- `PgSessionRevocations::refresh` is not scheduled by any worker (it was not
  before this work). Another replica sees a logout, or now an ended set of
  sessions, only after it restarts or something calls `refresh`. A single
  replica sees both at once. Scheduling it is a small worker and is owed before
  a second replica is run.
- Capability and delegated tokens name no session, so ending sessions does not
  end them (`plan-passkey-recovery.md` open question 6).

### What waits

| On | What |
|---|---|
| the sign-in route (passkey plan) | `Admission::on_arrival`, `CredentialOps::redeem_password`, `ManagedStore::must_change`; a way for the sign-in answer to carry `std.person.tenants`' list |
| F12, tenant-scoped provider connections | `TenantConnections` names which connections a tenant owns and retires them. Nothing implements it yet, so the third row of §4.6 is built and has no real connection to act on |
| a re-run of the deletion settlement | a settlement that fails after the erasure is reported in the answer and is not retried by anything |

### The registration lifetime defaults to 90 days (owner, 2026-09-29)

`ORDO_AUTH_REGISTRATION_TTL_SECS` defaults to `7776000`. The variable still
overrides it, `0` and a value that does not parse still refuse the boot, and the
startup log prints the value in use and whether it is the default
(`registrations.rs`: `DEFAULT_TTL_SECS`, `ttl_from_env`, `describe_ttl`). The
sweep is therefore always on, so the census no longer lists
`registrations.sweep` as disabled, and a deployment that must keep every
sign-up sets a very large number. `docs/deploying.md` and
`docs/howto-operating.md` carry the new default.

- Tests in `tests/registrations.rs`:
  `the_lifetime_defaults_to_ninety_days_and_a_bad_one_refuses` and
  `the_census_line_names_the_lifetime_in_use`. Both fail with the default set
  to seven days.

### A person sees their own tenants: `std.person.tenants` (owner, 2026-09-29)

A platform op with no parameter, in `platform_ops.rs` (`MY_TENANTS`,
`person_specs`, `PlatformOps::my_tenants`), reading the new
`AccessEngine::spaces_of`. It answers one row per tenant the caller belongs to:
`space`, `name`, `role`, `status`. `name` is the label the tenant's creator gave
it, and is null for the deployment's own space.

**Why an op and not an auth route.** The door has one pipeline: the access
check, the catalog, metering and the single refusal. An op gets all of it. A
route beside `/auth/logout` would need its own copy of each, in `auth.rs` or
`serve.rs`, which the passkey work owns. Two more reasons in the shape of the
question:

- **Nobody else can be asked about.** The op takes no parameter. The person is
  the principal the door proved, so no call can name another. The tenant the
  call ran in learns only that its member called an op, as it learns of every
  call, and never the other tenants in the answer.
- **The engine already knows.** `spaces_of` reads the memberships the door
  decides with, so a suspended or removed membership is as fresh here as it is
  at the door, and no database is read per tenant.

Every role may call it and it asks for no presence, so a password session reads
it. `lib.rs` gives it to viewers, editors, managers and owners and leaves it out
of the step-up list, through `platform_ops::person_scoped`.

**A gap, stated.** Every op call names a tenant, and the door refuses a person
who is not a member of it. So the op is answered in any tenant the person
already belongs to, and a person who belongs to none has nothing to list. The
question a sign-in leaves is *which tenant do I name first*. Answering it
belongs in the sign-in response (`auth.rs`), which can carry this list, or in a
route beside `/auth/refresh`. `AccessEngine::spaces_of` is the function either
would call.

- Tests `tests/my_tenants.rs` (3): a person sees their own with their role in
  each, nobody sees anybody else's, and the door serves every role with no
  presence and refuses a stranger. All three fail with the filter to the caller
  removed and with the door grant removed.

### E7 · What tenant deletion does to identities

`crates/ordo-host/src/tenant_identities.rs`: `TenantIdentities::after_delete`,
called by `std.space.delete` once the erasure has happened (`platform_ops.rs`
arm for `exit_ops::DELETE`, wired in `runtime.rs` where credentials are). The
answer's row gains `identities: { released, removed, retired }`.

| The person | What it does |
|---|---|
| a managed identity that ever signed in | `$managed.released_at` is set. Credential, links and passkeys stay. Recorded as `identity-released` |
| a managed identity that never did | removed with its links, passkey rows, password, unused tickets and rows beside it. Recorded as `identity-removed` |
| a person who came through the tenant's own provider | the `oidc:<connection>:` and `saml:<connection>:` links go. A principal left with no link is removed and its sessions end. One with a platform link stays |

Every act is recorded at deployment scope, naming the owner who deleted the
tenant. The tenant's own stream is not written to, since the tenant is gone.
Another tenant's people are not touched. A connection whose name starts like
another (`acme` and `acmecorp`) is another connection.

**Where "ever signed in" is recorded, and why there.** In `$signedin`
(`principal`, `first_at`), written by `DirectoryStore::note_sign_in`, which
`ordo_identity::sign_in` calls on every completed sign-in. That function is
what `/auth/complete` calls before it mints a session, so it is the place a
session comes from, and it is not `auth.rs`. It is durable and synchronous. The
security event stream is not a place to keep it: the recorder collapses events
and declares loss under load, and whether a person is kept or removed cannot
depend on whether an event survived. Only the first sign-in is kept, so a later
one is an insert that does nothing. A failed write refuses the sign-in, because
an unrecorded sign-in could later be taken for an identity nobody used.

- The removal was lifted out of the sweep into `registrations::remove_identity`
  so the sweep and this share it. It now also deletes the `$signedin` row.
- Tests `tests/tenant_identities.rs` (5) and `tests/exit_ops.rs`
  (`deleting_a_tenant_settles_the_people_it_made_and_says_so`). The three named
  in §5 are there. Each fails with its branch removed, and the through-the-door
  test fails with the settlement unwired.

**Deviations and gaps, stated.**

- The plan named events `IdentityReleased` and `IdentityRemoved { reason }`.
  E2 built one event kind, `Enrolment { act, detail }`, and these are two acts
  of it (`identity-released`, `identity-removed`) with the reason in the
  detail. No chain tag was added.
- **The third row has no real provider to act on.** A tenant registering its own
  provider is F12 and is not built. `TenantConnections` is the seam, and
  `NoConnections` is what is wired. The test uses a stand-in that owns the
  connection `acme`.
- **A principal destroyed with the tenant's provider may be a member of another
  tenant.** Its memberships there are left as rows naming a principal that no
  longer exists, as any removed identity leaves them. The plan says the person is
  destroyed, and this follows it.
- A settlement that fails after the erasure is reported in the answer and is
  not retried, and the erasure is not undone. It is idempotent, so a later call
  is safe, and nothing makes that call yet.
- Nothing has deployed, so an identity that signed in before `$signedin`
  existed reads as never signed in. No migration is owed.

### First come, first served is off by default (owner, 2026-09-29)

The owner: *"setup belongs on the server side end of story."* `PolicyStore::get_for`,
which switched the first-comer way on for the deployment's own tenant when it had
chosen nothing, is gone (`admission.rs`). A tenant that never chose has nothing
on, the home tenant included. The mechanism, the latch, the claim code and the
setting are untouched, for an app developer who turns it on with
`std.space.setAdmission`, under the ceiling.

- Test `tests/first_comer.rs`:
  `first_comer_is_off_when_nothing_turns_it_on_even_in_the_deployments_own_tenant`.
  It fails against the code that defaulted the home tenant on.

**Can a deployment name its first owner without first-comer? Not through
anything built.** What the tree has:

- `provision::ensure_system_space` writes a founding row at boot, but only for
  the **system space**, and its owner is the fixed service principal
  `system:platform`. It is not a person and holds no credential.
- The deployment's own tenant (`ORDO_SPACE`) gets **no** founding row from any
  boot. Nothing reads an environment variable or a CLI argument naming its
  owner. A row appears there only if somebody inserts it into `members` by
  hand, or the fixture and `ordo dev` seed one.
- `std.space.create` (and `ordo space create --owner`) names a founding owner,
  but for a **new** tenant, and it is owner-only, step-up and bound to the home
  tenant. It needs the home tenant's owner to exist already.
- `std.space.admit` needs an existing owner too.

So the first owner of the deployment's own tenant has no server-side path.
What is missing is one act on the server side: a setting or command that names
the first owner and writes the home tenant's founding row, with two parts.
**(1) Who.** Naming a person needs an identity that exists before they sign
in: an enrolment ticket minted server-side for a named person (`Tickets::issue`
exists and no command calls it outside the door), or a named principal already
in the directory. **(2) Where it runs.** At boot, through `provision_space_on`
on the home tenant, idempotent on the member id so a restart adds no second
owner, in the same place `ensure_system_space` runs. Nothing of this is built.
A deployment with database access can insert the row by hand today, which is
the thing this would replace.

### Tenant invitations: `std.space.invite` (checklist 24e)

Nothing issued a ticket of purpose `Invitation`, so a tenant whose admission is
invitation-only could admit nobody. `crates/ordo-host/src/invite_ops.rs` adds
three platform ops, wired where tickets exist (authentication configured) and
named in `runtime::platform_specs`.

| Op | Kind | Parameters | Answer |
|---|---|---|---|
| `std.space.invite` | write | `role`, `expires`, `to`, `label`, all optional | `ticket` (once), `expiresAt`, `role`, `outcome` |
| `std.space.invitations` | read | none | one row per open invitation: `id`, `role`, `for`, `label`, `issuedBy`, `issuedAt`, `expiresAt` |
| `std.space.revokeInvitation` | write | `id` | `id`, `outcome` |

- **Who.** Tenant owner and manager, like the other tenant-scoped platform ops.
  Presence follows the app developer's `ORDO_STEP_UP_EXEMPT_OPS` like every
  platform op. A manager cannot name a role above their own, so an invitation
  is not a way to make an owner by proxy.
- **Role.** One of owner, manager, editor, viewer. When none is named the
  ticket names none, and redemption applies the tenant's default (viewer unless
  the app developer set another with `std.space.setAdmission`).
- **The secret.** Returned once. The app delivers it; Ordo does not send it.
  Only the hash is stored. A listing names an invitation by the first 16 hex
  characters of that hash, never by the ticket.
- **Lifetime.** `expires` is a duration such as `7d`, stated per invitation.
  When absent the ticket default applies (3 days, `tickets::DEFAULT_TTL_SECS`).
  This module adds no maximum. The ticket store still clamps every ticket to
  `tickets::MAX_TTL_SECS` (30 days), a fixed value that checklist 24c owes a
  setting. The answer's `expiresAt` is the instant that applies.
- **List and revoke.** Built. `TicketStore` gained `list_open` and
  `revoke_open` (memory and Postgres). Both default to an error, so a store
  that cannot do them refuses and never answers an empty list. A revoked ticket
  is deleted, so redeeming it is the door's one refusal.
- **The record.** `invitation-issued` and `invitation-revoked`, in the tenant's
  own stream through `enrolment::record` (`deployment: false`, since an
  invitation is an act on a membership). Neither names the ticket. The ticket
  store's own `ticket-issued` still lands in the deployment stream.
- Test `tests/invitations.rs` (8), against Postgres and the real
  `Admission::on_arrival`. The role test and the list-and-revoke test fail with
  the role ceiling and the store's revoke removed.

No change to `auth.rs` or `serve.rs` was needed: the sign-in route already
passes `invitation` to `on_arrival`.
