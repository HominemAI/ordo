# The first program: a notes app with passkey sign-in

Checklist `17ae`. Ruled 2026-09-28: *"a note taking app that stores notes.
You type a note, it stores it and makes sure theres users that can login
with a passkey."* Recorded in `corpus-rulings.md` (2026-09-28, ruling 5), and
answered in six parts on 2026-09-29 (*"The second factor, and the notes app"*).

Companion documents: `plan-provisioning-on-signup.md` (P0 to P5, the platform
feature this app is the first consumer of), `plan-passkey-signin.md` (S0 to
S7), `plan-enrolment.md` (E1 to E6, built), `sharing-options.html` (the ruled
sharing design), `plan-client-cache.md` (C1 to C9), `deploying.md` §4c, §5b
and §7, and `examples/first-app`, whose shape this plan reuses.

Markers: **[settled]** ruled by the owner · **[plan]** proposed here ·
**[open]** named and not answered · **[unconfirmed]** a construct the
compiler has not been shown to accept.

Status: **planned, not built.** The owner's build order of 2026-09-29: *"the
notes app built last and planned now."* Every claim about the code was read
against the tree at `fc46564` on 2026-09-29. Nothing has deployed, so no
shape change owes a migration.

## What changed on 2026-09-29

- **One person, one tenant.** Sign-up gives each person a tenant of their own. The single tenant `notes` of the earlier plan is gone, and with it the claim code, first-comer and the `editor` default role.
- **Sharing a note is sharing between tenants**, which is inter-space sharing (`11-2`, ruled Option A 2026-09-08). It becomes a dependency with its own increments, W1 to W4, in §5.
- **The page is `notes.inf.smg.icu`** and the passkey relying party is `inf.smg.icu`, which also covers the live door at `ordo.inf.smg.icu`.
- **The deployment's owner is set up on the server** (`plan-provisioning-on-signup.md` P0), never by arriving first.
- Cloudflare stays as it is until N9 has measured it.

---

## 0 · The starting state, verified

| Fact | Where |
|---|---|
| The live deployment at `ordo.inf.smg.icu` has been up since 2026-09-20 with no program | checklist `17ae`; `deploying.md` §7 |
| The first-deployment runbook is written: boot on an empty database, custody at boot, `ordo apply` over the door | `deploying.md` §4c |
| `examples/first-app` walks one entity, one write and one read end to end | `examples/first-app/README.md` |
| Passkey verification is built and tested. Serving it is S0 to S7: S0 and S1 are built, S2 onward is in progress; `/auth/register` does not exist yet | `plan-passkey-signin.md`, Built; `crates/ordo-host/src/auth.rs:673-676` |
| The relying party refuses an origin that is not the RP id or a subdomain of it, and refuses `http://` off loopback | `crates/ordo-identity/src/passkey.rs:189-239` |
| E1 to E6 are built: the door resolves a person to a member per tenant; tickets; the membership commands; admission policy under the deployment's ceiling; first-comer as a choice; the password give and reset | `plan-enrolment.md`, Built |
| `std.space.create` is served by the binary and creates a tenant with its own database and keys, for a deployment owner who is present. Nothing gives a person a tenant at sign-up | `crates/ordo-host/src/tenant_ops.rs`; `runtime.rs:1005-1032`; `plan-provisioning-on-signup.md` §0 |
| CORS is built for the environment: `ORDO_CORS_ORIGINS` lists exact origins, unset means same-origin only, `*` refuses the boot, an unlisted origin's preflight answers `403`, credentials are never allowed | checklist `14g`; `deploying.md` §5b; `crates/ordo-api/src/cors.rs` |
| The passkey origins are their own setting, `ORDO_AUTH_ORIGINS`, being built as S2, apart from the CORS allowlist | `plan-passkey-signin.md` §2.5; ruled 2026-09-29, *"Passkeys can potentially be used on more than one address"* |
| Inter-space sharing: the enforcement half passes real-Postgres tests, a guest reads exactly the rows their acl row names, and revocation of the acl row closes them at the next call. Registration is what is missing: `register_share` has no caller | `crates/ordo-conformance/tests/guards_corpus.rs:1637` (GC059, GC060), `:1661` (GC061, GC066); `crates/ordo-identity/src/grants.rs:239`; checklist `11-2` |
| The engine keys a member by `(space, principal)` and carries the member id `$caller` binds to. A row with no role stated is a **viewer**, so a guest cannot be a null-role row as `definition-language.md` describes it; the engine needs a guest kind with no profile | `crates/ordo-host/src/lib.rs:1698-1742` |
| The door takes the capability path only from the token; the grant registry is not consulted to admit a request. Nothing in the host mints a capability token for a person | `crates/ordo-identity/src/access.rs:714-770`; `grants.rs` module doc; `grep -rn "with_capability" crates/ordo-host/src` finds nothing |
| The door consults the registry for a revoked delegation on every call. It does not for a grant | `lib.rs:1401` |
| The SDK client is bound to one space and `withSpace` makes a second | `clients/ordo-sdk/src/core/client.ts:108` |
| The device replica is built through C7 and is per space and principal. C8 is not | `plan-client-cache.md` §7; `clients/ordo-sdk/src/replica/` |
| The replica's sealing, per-space stores and purge on revocation are checked at the owner's word (SH-7). Owed: device keys under a passkey, a lock when idle, a retention policy | checklist `22m` |
| The default access profile allows reads for every role, writes for editor and up, `std.*` for owner and manager | `lib.rs:1745-1770` |
| `acl` derives the link entity, the guards, `grant…Access`, `revoke…Access` and the creator's bootstrap row | `crates/ordo-cli/tests/golden/market.artifact.json`; `guards_corpus.rs:2590` |
| The deployment sits behind Cloudflare, which cuts long requests | checklist `17u`, follow-up 5 |

---

## 1 · What a tenant is in this app [settled]

The owner found the earlier question about "the app's own space" confusing.
Stated plainly:

| Word | In the notes app |
|---|---|
| **The program** | `notes.ordo`, applied once, deployment-wide. Every tenant runs the same program at the same version. A tenant created after an apply receives the version being served (`tenant_ops.rs:103-112`) |
| **A tenant** | **one person's notes.** Created at their first sign-in, with its own database and its own keys. The person is its owner and its only member. Nobody joins it |
| **The deployment's own space** (`ORDO_SPACE`) | holds no notes. It is where the program is applied from and where the deployment owner acts |
| **The deployment owner** | a person named on the server (`ORDO_DEPLOYMENT_OWNERS`, P0). They also sign up and get a notes tenant like anyone |
| **A shared note** | read through a window into the sharer's tenant. The note never leaves its tenant; the reader is a guest there, for that note |

So "one user, one tenant" is a provisioning setting on the deployment
(`ORDO_PROVISION_SELF_SERVE=on-signup`), and the app never creates a tenant
itself.

---

## 2 · What the app does in v1 [plan]

| In v1 | Why |
|---|---|
| A person signs up with a form and a passkey, in a browser, and is signed in with a tenant of their own | the ruling: *"sign up via a form, press their passkey button, and bam theyre signed in"* |
| A note has a title and a body, both plain text | one entity, no sealed field, no blob |
| A person sees their own notes, newest first | the guard the ruling asks for |
| Create, edit with compare-and-set, delete | the three write modes the compiler labels |
| Share one note, read-only, with another person, by a link; unshare it | the ruling. Between tenants, §5 |
| A "shared with me" list | the other side of the share |
| Notes readable on the device with the network off | the replica is on by default (ruled 2026-09-14) |

| Deliberately left out | Why |
|---|---|
| Timestamps on a note | `today` and `now` are legal only as shape defaults (`traps.md` line 24) and no test shows a `datetime` default compiling. The command log carries the time. Newest first is `sort -id` |
| Search | `contains` against a parameter is **[unconfirmed]**. Tier-0 refinement on the device (C6) covers a filter box |
| Write-level sharing, groups, folders, tags, attachments, markdown | each is a second entity or a feature the first program does not need to prove |
| Writes while offline | optimistic apply and rollback are not built. A write with no network is refused and the page says so |
| Password sign-in, SSO, recovery of a lost passkey, backup codes, a second factor | the owner's order puts recovery and the second factor before this app, so v1 gets whatever has landed and adds no surface of its own |
| A second tenant for one person | `plan-provisioning-on-signup.md` P5, after the first deployment |
| Choosing who to share with from a list | invitation, never enumeration (`definition-language.md`, Inter-space sharing). A share is a link the sharer sends |

---

## 3 · The program [plan]

### 3.1 The listing

`examples/notes/notes.ordo`:

```
# Notes owned by their author, shared read-only one note at a time.
# `acl` derives the link entity `notesAccess`, the guards on `notes`,
# `grantNotesAccess` and `revokeNotesAccess`, and the creator's admin
# row in the same command group as the create.

domain notes @ 0.1.0

entity notes {
  title: text
  body:  text = ""
  acl (read, write, admin)
}

op createNote($title: text, $body: text) =
  set notes { title: $title, body: $body }

op editNote($id: id, $rev: int, $title: text, $body: text) =
  set notes { id: $id, rev: $rev, title: $title, body: $body }

op deleteNote($id: id) =
  del notes { id: $id }

op myNotes unbounded =
  notes[id, rev, title, body]
  sort -id
```

Unchanged from the earlier plan except that `shareNote` is gone: the acl row
a share writes is written by the platform at accept time, as the sharer,
through the derived `grantNotesAccess` (§5.3). `unshareNote` is the derived
`revokeNotesAccess`.

What `ordo check` should derive: `members` and `groupMembers`
platform-provided; `notes` guarded by `notesAccess(member == $caller …)`
with the group disjunct; `notesAccess { note -> notes owned, member ->
members owned, level, unique (note, member) }`; `createNote` `creates` and
guard-exempt (self-rooted); `editNote` `converges` under CAS; `deleteNote`
an addressed delete; `myNotes` `sql`, unbounded, pageable; plus
`std.notes.*`, `std.notesAccess.*`, `grantNotesAccess`, `revokeNotesAccess`.

### 3.2 Every construct, and where it is built

| Construct | Evidence it compiles today |
|---|---|
| `domain notes @ 0.1.0` | `examples/first-app/readings.ordo:9` |
| `title: text`, `body: text = ""` | `docs/quickstart.md` §1 |
| `acl (read, write, admin)` | `crates/ordo-syntax/src/declparse.rs:443-451`; `market.ordo:140-143`; `guards_corpus.rs:851` |
| the creator receives `admin` in the create's command group (G10a) | `guards_corpus.rs:2590` |
| `set notes { … }` create, and `set notes { id, rev, … }` compare-and-set | `docs/quickstart.md` §3 and §1 |
| `del notes { id: $id }` | `market.ordo:298` |
| `notes[id, rev, title, body]` with `rev` projected | `crates/ordo-conformance/tests/std_library.rs:783` |
| `sort -id`, `op … unbounded =` | `examples/first-app/readings.ordo:20`; `market.ordo:165` |
| `grantNotesAccess`, `revokeNotesAccess` | `market.artifact.json` carries `grantBoardsAccess`, `revokeBoardsAccess` |

**[unconfirmed]**, tried in N1 with `ordo check` and moved in or kept out:

| Construct | What was looked for |
|---|---|
| `text(n)` as a field bound | the length form is documented for parameters only (`declparse.rs:1312`) |
| `title contains $q` | no fixture uses `contains` with a parameter |
| `notes(id == $id)[…]` as an authored addressed read | no fixture has the form; `std.notes.get` would serve a detail read for the owner |
| `std.notesAccess.list` as the sharer's list of shares on a note | the golden shows the derived `std.boardsAccess.*`; the list form is not exercised by a test |
| `datetime = now` as a shape default | `traps.md` says legal; no test declares one |

### 3.3 What the guard buys, stated so it can be tested

| Caller, in a tenant | `myNotes` | `editNote` on a note | `deleteNote` | `revokeNotesAccess` |
|---|---|---|---|---|
| the owner (the person) | all their notes | `converges` | deletes | allowed |
| a guest holding `read` on one note, through a window token | that note only | `access_guard` on it, `not_found` on the rest | the same | `access_guard`, and the op is not in the window anyway |
| a guest after revocation | nothing | `access_denied` at the door | the same | the same |
| a person with no window into this tenant | `not_found` at the door: not a member, no grant | the same | the same | the same |

There is no viewer row and no editor row: the tenant has one member with
role `owner`, and every other reader is a guest with no role. The earlier
plan's `editor` default falls away.

---

## 4 · The front end [plan]

### 4.1 Shape

React, built with Vite, over `@ordo/sdk`. Reasons, against the two other
choices:

| Choice | Why not first |
|---|---|
| A plain page with `fetch`, as `examples/first-app` | it proved the door already. It cannot hold the replica, and the ceremony code plus the CAS path would be a second hand-written copy of what the SDK enforces |
| `@ordo/react` | C8 (`@ordo/react` over the replica) is not built and the two packages are independent by design. The replica lives in the SDK, and offline reads are in v1 |

`ordo gen ts` writes one typed function per op. The app owns one small
piece of state the SDK does not: what to show on `cas_miss` (refetch, show
"changed elsewhere", let the person merge and retry with the new `rev`).

Pages: sign up or sign in · my notes (list, new note) · one note (edit,
delete, share link, unshare) · shared with me · about (this person's
principal id, for the deployment owner's own use, and sign out).

### 4.2 Signing up, signing in, and landing in one's own tenant

| Step | Page | Door |
|---|---|---|
| sign up | a form with a display name, then `registerPasskey(client, {display})` (S6) | `POST /auth/register` answers creation options (S4) |
| the browser ceremony | `navigator.credentials.create()` or `.get()` | |
| complete | `POST /auth/complete` | verifies; enrols the identity (P3); **provisions the person's tenant** (P2) and answers `space` with the token |
| land | the page binds its client with `withSpace(space)`, opens the replica for `(space, principal)` and lists `myNotes` | |
| sign in, later | `signInWithPasskey(client)` (S6) | `/auth/begin` and `/auth/complete`; the answer carries the same `space` |
| `space: null` | the page shows the `provisioning` reason ("in progress", "failed") and offers to try again; the identity is real and the row is visible to the deployment owner | §2.3 of the provisioning plan |
| hold the session | token in memory only; `POST /auth/refresh` before `expires_at` | |
| sign out | `POST /auth/logout`, then `LocalReplica.purge()` for every open store | `204`, always |

The token never lands in `localStorage`. A token is good from any device
that holds it until it expires or the person logs out (18x); the page's
expiry is `ORDO_SESSION_SECS` and the ceiling `ORDO_SESSION_MAX_SECS`.

### 4.3 Offline: the replica, on by default

`myNotes` is syncable by C4a's rule: anchored on one entity, not grouped,
`id` projected. The page declares it once per space it holds:

```ts
const replica = await LocalReplica.open(new IndexedDbBackend(), space, principal);
const sync = SyncManager.start(client, replica, [{ spec: specs.myNotes }]);
```

A window into another tenant is a second `(space, principal)` store, synced
the same way, with the window token (§5.3). A refusal of `access_denied`
after a revocation purges it (C7).

| Works | Because |
|---|---|
| the list answers from the device at once and with the network off | C3, C5, C6 |
| a note shared or unshared while the device was away appears or disappears on reconnect | C4c |
| a revocation purges what the window synced | C7 |
| a raw IndexedDB dump holds no title, body, id or space | C3, checked at the owner's word (SH-7) |

| Does not | So |
|---|---|
| a write while offline | the button is disabled with the reason; the text stays in the editor |
| `?live=1&sync=1` through Cloudflare | a long-lived stream may be closed on a cadence; the manager reconnects and reloads. Measured in N9 before anything is changed |

---

## 5 · Sharing between tenants: what is built, what is owed [plan under a ruling]

> **Superseded 2026-09-29 by `docs/plan-sharing.md`** (G1 to G9, L1 to L5).
> Its findings are kept and cited there; W1 to W4 below are replaced by those
> increments, and the notes app's N8 depends on G1 to G7.

Ruled 2026-09-08, Option A: *"grant-shaped, no syntax."* A guest is *"an
ordinary member holding one grant"*. The design is `sharing-options.html` §2
(Option A) and §4. The notes app needs the smallest slice of it: one person
reads one note in another person's tenant, and the sharer can stop it.

### 5.1 What is built and what is owed

| Part of Option A | State | Where |
|---|---|---|
| A guest's read executes inside the sharer's space under its guards, `$caller` bound to the guest's member row; a guest sees exactly the rows their acl row names, and nothing under a role | **built and tested**, GC059 to GC066 against Postgres | `guards_corpus.rs:1637`, `:1661` |
| `register_share`: a share is a `NewGrant` with a foreign principal; sharing an unguarded op is refused (`guest_exposure_unguarded`) | **built**, no caller | `grants.rs:239` |
| The grant registry, durable, with `record`, `revoke`, `is_live`, `callers_of` | **built** | `crates/ordo-host/src/grants.rs:77` (`PgGrantRegistry`) |
| The capability path at the door: space, expiry, per-op pins, never falls through to roles | **built** | `access.rs:754` |
| A guest member row with no role and no profile | **owed.** The engine reads a role-less row as a viewer (`lib.rs:1711-1718`) | W1 |
| The invitation and consent ceremony: link, accept, principal binding, consent in both chains | **owed**, *"exists in no document"* (`sharing-options.html` §4) | W1, W2 |
| A token that carries the share grant | **owed.** Nothing mints one for a person | W2 |
| Revocation refused at the next call | **owed** for grants; built for delegations at `lib.rs:1401` | W3 |
| The SDK half | **owed** | W4 |
| Local sealing with a server-released key, so a revoked guest's device cannot read what it cached | **not needed for v1**: the replica purges on refusal (C7), which is the honest online case. The rest is `5g-rev` | |

### 5.2 The flow, and who consents to what

Invitation, never enumeration. The sharer does not learn the reader's
principal id until the reader accepts, and never sees a list of people.

| Step | Who | What |
|---|---|---|
| 1 | the sharer, on a note | `std.space.invite($ops: ["myNotes"], $grants: [{entity: "notes", id: <note>, level: "read"}], $expires: <365 days>)`, bound to their own tenant. Answers a ticket. The page shows `https://notes.inf.smg.icu/accept#<ticket>` to copy and send |
| 2 | the reader, signed in | opens the link. The page shows the sharer's tenant name and what accepting discloses (their display name to the sharer) and asks for consent |
| 3 | the reader | `POST /auth/share/accept {ticket}` with their bearer token |
| 4 | the platform | redeems the ticket (E2, purpose `Share`); binds the reader as a guest in the sharer's tenant, or reuses the seat if they already are one; records the grant (`register_share` then `PgGrantRegistry::record`); **runs `grantNotesAccess(<note>, <seat>, "read")` as the sharer**, with the share id as its correlation; records consent in both chains; announces the sharer's tenant so every replica rebuilds its door |
| 5 | the reader's page | `POST /auth/window {space}` mints a window token; `client.withSpace(space)` with it; `myNotes` there lists the shared note under "shared with me" |
| 6 | the sharer, later | `revokeNotesAccess` on one note (unshare), or `std.space.revokeShare($seat)` to close the window entirely |

**Why the platform writes the acl row (step 4).** The alternative is a
second step for the sharer after the reader accepts, because the sharer
cannot name a member row that does not exist yet. The ticket carries the
sharer's stated intent, as an invitation ticket carries a role (E2), and the
write is attributed to the sharer in their own chain, which is where it
happened. The platform derives `grantNotesAccess` and knows its shape. This
is the one design choice in §5 a reviewer should look at; the fallback is
the two-step flow with a "pending" state on the sharer's page.

**One seat per person per tenant.** `$caller` binds to one member id, so a
reader sharing two notes from one sharer holds one seat and two acl rows.
Accept finds the existing seat by principal.

### 5.3 The increments, scoped to the notes app

These belong to `11-2` and are named here so the notes app can depend on
them. Whoever builds `11-2` in full writes its own plan; these four are the
slice this app needs. Tests in `crates/ordo-host/tests/sharing.rs`, new.

| | What | Files | Done when |
|---|---|---|---|
| **W1** | **The guest seat and the invitation.** A `members` row with `role = 'guest'`, which the engine loads with `set_guest(space, principal, member_id)`: bound for `$caller`, no profile, no role; `std.space.invite` (owner-only, presence per the app developer's setting), which validates each op against `register_share`'s rule at invite time and mints an E2 ticket with purpose `Share` and the payload of §5.2 step 1 | `crates/ordo-identity/src/access.rs` (`set_guest`, `Role` untouched), `crates/ordo-host/src/lib.rs` (`engine_from_members`), `tickets.rs` (`Purpose::Share`, `Want`), a new `share_ops.rs`, `platform_ops.rs` (spec), `runtime.rs` (`platform_specs`) | `sharing.rs::a_guest_row_binds_caller_and_holds_no_role` (a guest's session token calling a read op in the sharer's space is refused at the door, and their window token reaches the guarded rows), `::inviting_an_unguarded_op_is_refused_before_a_ticket_exists` |
| **W2** | **Accept and the window token.** `POST /auth/share/accept`; the seat bound or reused; the grant recorded; the acl grants applied as the sharer; consent recorded in both chains; `announce_members`. `POST /auth/window {space}`: for a live share grant of the token's principal in that space, a capability token with the grant's pins and an expiry of `min(grant, ORDO_SESSION_SECS)`, re-minted from the session as needed, so logout ends the window within one token life | `auth.rs` (two handlers, `router`), `serve.rs`, `share_ops.rs`, `crates/ordo-host/src/grants.rs` | `::an_accepted_share_lets_the_guest_read_exactly_the_shared_note_and_nothing_else`, `::a_spent_or_expired_share_ticket_is_the_one_answer`, `::a_second_share_to_the_same_person_reuses_the_seat`, `::a_window_token_is_refused_in_any_other_space` |
| **W3** | **Revocation.** `std.space.revokeShare($seat)`: `GrantRegistry::revoke`, the seat marked `revoked`, `announce_members`; the door consults `is_live` for a capability token that names a share grant, as it does for a delegation at `lib.rs:1401`; `/auth/window` refuses a revoked grant. `POST /auth/share/leave {space}` for the reader | `share_ops.rs`, `lib.rs` (the registry gate), `auth.rs` | `::a_revoked_share_is_refused_at_the_next_call_and_the_window_cannot_be_reminted`, `::a_reader_who_leaves_is_refused_at_the_next_call` |
| **W4** | **The SDK.** `acceptShare(client, ticket)`, `openWindow(client, space)` answering a client bound to that space, `leaveShare`; `GET /auth/spaces` (P4) lists guest bindings with `via: "guest"` | `clients/ordo-sdk/src/auth/share.ts` (new), `index.ts`, tests with `fetch` stubbed | `the_window_client_sends_the_window_token_and_the_shared_space_header`, `accept_posts_the_ticket_byte_for_byte` |

**Order: W1, W2, W3, W4.** W1 needs E2 (built) and E1 (built). W2 needs
S3 (a real session token to accept with) and P4 for the listing. W4 can
start once W2's JSON shape is fixed, which is §5.2.

### 5.4 Is it a hard blocker?

**For v1 as ruled, yes**: *"a person can share a note with another person"*
is in the ruling. **For the first live sign-in, no**: N1 to N7 ship notes,
sign-up and one's own tenant without it, and N8 adds it. The order puts the
first live measurements before the sharing build on purpose, so that the
door, the ceremony and provisioning are measured on their own first.

---

## 6 · Where it runs [plan]

| Part | Where | Notes |
|---|---|---|
| the door | `https://ordo.inf.smg.icu` | up since 2026-09-20 with no program. `ORDO_SPACE` stays the deployment's own space and holds no notes |
| the page | Cloudflare Pages, custom domain `notes.inf.smg.icu` | the owner: *"cloudflare pages as the front with ordo as the baas"* (`14g`). The owner holds the DNS |
| the program | applied deployment-wide with `ordo apply ./examples/notes --host ordo.inf.smg.icu --space <deployment space> --token …` | step-up gated, so the token is a passkey token of a deployment owner (P0). Until then the break-glass path is `ordo apply --db` |
| a person's tenant | created at their first sign-in (P1, P2), on the deployment's default cluster, with its own database and keys | the person is its owner |

### 6.1 The settings, live and local

| Variable | Live | Local (`ordo dev`) | Why |
|---|---|---|---|
| `ORDO_AUTH_METHODS` | `passkey` | `passkey` | S2 |
| `ORDO_APEX_DOMAIN` | `inf.smg.icu` | `localhost` | the relying party id. The page is a subdomain of it, so `RelyingParty::new` accepts it (`passkey.rs:230-238`). A passkey registered here also works at `ordo.inf.smg.icu` and at any later app under the apex. Changing it later strands every passkey, so it is set once |
| `ORDO_AUTH_ORIGINS` (S2) | `https://notes.inf.smg.icu` | `http://localhost:5173` | which page may run a ceremony. `http://` is accepted on loopback only |
| `ORDO_CORS_ORIGINS` | `https://notes.inf.smg.icu` | `http://localhost:5173` | which page may call the door (`14g`). Exact origin, no path, no trailing slash. Two settings on purpose (`plan-passkey-signin.md` §2.5) |
| `ORDO_AUTH_PROVISIONING` | `auto-onboard` | `auto-onboard` | the deployment's ceiling (E4): anyone can sign up |
| `ORDO_PROVISION_SELF_SERVE` | `on-signup` | `on-signup` | one person, one tenant (P2) |
| `ORDO_PROVISION_SIGNUPS_PER_HOUR`, `…_PER_ADDRESS_PER_HOUR` | 60, 5 | unset | P3 |
| `ORDO_DEPLOYMENT_OWNERS` | the owner's principal, after his first sign-up | the dev principal | P0 |
| `ORDO_AUTH_REGISTRATION_TTL_SECS` | 7 days, if the owner takes the recommendation | unset | E4. Under `on-signup` it only ever reaches a person whose provisioning failed and who never returned |

How a call reaches the door from the page: the browser sends
`Authorization`, `X-Ordo-Space: <the person's tenant>` and `X-Ordo-Version`,
so every call is preflighted; `ordo_api::cors::apply` answers the `OPTIONS`
for a listed origin and `403` for any other. A window call is the same with
the sharer's space in the header and the window token. Locally,
`ORDO_CORS_ORIGINS=http://localhost:5173` on `ordo dev` does the same, so
the app never carries a proxy.

---

## 7 · Dependencies

| Dependency | Plan | Blocks | What the app needs from it |
|---|---|---|---|
| Passkey sign-in served: S2 (the relying party from the environment), S3 (sign-in), S4 (registration), S6 (the SDK ceremony) | `plan-passkey-signin.md` | **hard** | a real person gets a token with presence, in a browser |
| E1, built: the door resolves `(principal, tenant)` to a member | `plan-enrolment.md` | done | the signed-in `usr_…` binds `$caller` in `notes` |
| A tenant of one's own at sign-up: P1, P2; the bounds, P3; the deployment owner, P0 | `plan-provisioning-on-signup.md` | **hard** | *"sign up via a form, press their passkey button, and bam theyre signed in"* |
| The person's list of tenants, P4 | the same | soft | the page can bind to `space` from `/auth/complete` without it; needed for "shared with me" |
| Sharing between tenants, W1 to W4 | §5.3, checklist `11-2` | **hard for v1 as ruled**, not for N1 to N7 | one person reads one note in another's tenant, and the sharer can stop it |
| A published image carrying passkey, CORS and provisioning, and the live instance moved to it | `15a` | **hard** | the live door refuses `passkey` today and predates `cors.rs`. Images are x64 only (ruled 2026-09-29) |
| Custody minted on the live deployment: the sealed master | `17b`; `go-live.html` step 2 | **hard** | every tenant's keys are minted under it at provisioning |
| `ORDO_CORS_ORIGINS`, `ORDO_AUTH_ORIGINS`, `ORDO_APEX_DOMAIN` set on the live deployment | `deploying.md` §5b; S2 | configuration | §6.1, and a restart |
| S0, built: the auth routes record what they decide | `plan-passkey-signin.md` | done | the first sign-ups leave a security record |
| E3, built: the membership commands | `plan-enrolment.md` | done | lets N9 measure a suspension |
| C8 `@ordo/react` over the replica; C4d resume on reconnect | `plan-client-cache.md`, `plan-replica-feed.md` | no | the app uses the SDK directly; a reconnect reloads, measured in N9 |
| The console for the settings | `18o` | no | environment variables are enough for one page on one origin |
| Recovery R1 to R4, backup codes, the second factor | `plan-passkey-recovery.md`, `plan-second-factor.md`; `23a`, `23b` | no | the owner's order puts them before this app; the app adds nothing to them |

**The hard blockers, in order:** S2, S3, S4 and S6 with P1 and P2 beside
them (P1 needs no sign-in and can start now); P0 and P3; the image (`15a`);
custody and the three address settings on the live instance; then W1 to W4
for N8.

---

## 8 · Increments

Each has a "done when" a person can check without reading code.

| | What | Done when |
|---|---|---|
| **N1** | The program under `ordo dev`, two tenants seeded by hand, the five **[unconfirmed]** constructs each tried with `ordo check` | `ordo check` prints the derivation of §3.1; with `curl` and two tokens in two spaces: A's note is absent from B's `myNotes` in B's space; B calling A's space answers `not_found` at the door; A's `deleteNote` removes it |
| **N2** | The page, locally, on `http://localhost:5173` against `ordo dev` with `ORDO_CORS_ORIGINS=http://localhost:5173`, a printed token and space pasted in | a person creates, edits, deletes a note in a browser; editing the same note in two tabs shows "changed elsewhere" in the second and a retry with the new `rev` lands |
| **N3** | Sign-up and one's own tenant. Needs S2, S3, S4, S6, P1, P2. `ordo dev` with `ORDO_AUTH_METHODS=passkey`, RP id `localhost`, `ORDO_PROVISION_SELF_SERVE=on-signup` | **a new person types a name, presses the passkey button, and is looking at an empty notes list in a tenant of their own**; a second browser profile does the same and has a different tenant; each writes a note the other cannot see; refreshing keeps the session; a later sign-in lands in the same tenant; sign out ends it; `ordo dev`'s log shows the two provisions and how long each took |
| **N4** | Cross-origin, locally. The page on `:5173`, `ordo dev` on `:4000` | every call and the sync stream succeed from the listed origin; with `ORDO_CORS_ORIGINS` unset the browser reports the block and the preflight answers `403`; the page's own error says which variable to set |
| **N5** | Offline. `SyncManager` declares `myNotes` | with the network off the list still shows every note; a raw IndexedDB dump has no plaintext; sign-out empties the store |
| **N6** | Live. Needs the image, custody, the three address settings, P0 and P3. The deployment owner signs up on the page, sets `ORDO_DEPLOYMENT_OWNERS` to his principal, restarts, applies the program over the door with his passkey token | `ordo deploys --host ordo.inf.smg.icu` shows the notes artifact active; `/health/ready` lists the owner's tenant as served; `/health/deep` counts one own tenant; `curl` from a laptop with his token lists an empty `myNotes` |
| **N7** | The page on Pages at `notes.inf.smg.icu` | **the owner signs up with a passkey on his phone and writes a note**; a second person does the same on their own phone; neither sees the other's note; the security stream shows both sign-ups and both provisions; the sixth sign-up from one address in an hour is refused |
| **N8** | Sharing. Needs W1 to W4 and P4 | **the owner shares a note by sending a link; the second person opens it, consents, and reads the note under "shared with me"**; they cannot edit it; a second shared note from the same sharer appears without a second consent to the tenant; the owner unshares one and it disappears at the reader's next load; the owner revokes the share and the reader's window is refused at the next call and purged from their device; both chains carry the consent |
| **N9** | Measurements, §9.2 | the table in §9.2 has a number in every row, taken on the live deployment |

---

## 9 · What the app is for

### 9.1 What it should prove

| Claim | Where it is made | Shown by |
|---|---|---|
| A guard over more than one caller holds in production, and `not_found` is byte-identical for absent and forbidden | `definition-language.md` G12 | N1, N8 |
| `acl` bootstrap: the creator administers the row they made, in one command group | `definition-language.md` lines 1454-1482 | N1 |
| A sign-in is an enrolment (P3), and under the app developer's setting a provisioning | `corpus-rulings.md` 2026-09-29; `plan-provisioning-on-signup.md` | N3, N7 |
| One person is one identity: `$caller` binds to the member the door resolved from a `usr_…`, in their own tenant and as a guest in another's | `plan-enrolment.md` §2.4; `sharing-options.html` Option A | N3, N8 |
| A passkey ceremony completes against the served door with the RP id and origin the deployment configured, from a subdomain of the apex | `passkey.rs`, `webauthn.rs` | N3, N7 |
| A page on another origin reaches the door and an unlisted origin does not, on a real deployment behind Cloudflare | `14g` | N4, N7 |
| A tenant is provisioned inside one request, with a database and keys, and is served by every replica without a restart | `tenant_ops.rs`; `11-15` | N3, N7 |
| The device replica answers offline, is sealed, and a revocation removes rows | `plan-client-cache.md` C3 to C7 | N5, N8 |
| A share is a grant with a foreign principal, consented on both sides, revocable at the next call | ruled 2026-09-08; `grants.rs` | N8 |
| A program is applied over the door by a person present, and the actor recorded is the one the token proved | `deploying.md` §7 | N6 |
| Compare-and-set refuses the losing write and the page repairs it | `ui-gen.md` §1 | N2 |

### 9.2 Measurements worth taking

| Measurement | How | Why it matters |
|---|---|---|
| Sign-up, form to notes list, p50 and p95, including provisioning | page timing | the number the ruling is about: *"bam theyre signed in"* |
| Provisioning alone: `CREATE DATABASE`, schema apply, custody, activation | the server log per provision | whether one tenant per person is affordable at the deployment's size, and whether it fits under Cloudflare's request limit |
| Own tenants created, and how many have a second sign-in after a week | `/health/deep`; the stream | the input to the idle-tenant question (`plan-provisioning-on-signup.md` §6) |
| `myNotes` latency, p50 and p95, through Cloudflare | browser timing over a day | the first field number for the read path |
| `createNote` and `editNote` latency | the same | the write path, including the command log |
| Sign-in ceremony time, begin to token | page timing | |
| How long `?live=1&sync=1` stays open through Cloudflare, and what a reconnect costs | the manager's `attempts` and `loaded` | decides whether C4d resume is needed. **Measure first; change nothing about Cloudflare until the number says to** (ruled 2026-09-29) |
| Time from accept to the shared note on the reader's device; from revoke to it leaving | two devices, one clock | the window's revocation story, measured |
| Time from a suspension to a `403` at the door | `ORDO_ACCESS_REFRESH_SECS` default 30 | verified locally at four seconds; unmeasured in the field |
| Pool connections at rest and under two people, each with their own database | `/health/deep` | one database per person is one pool per tenant; a deployment has no number yet |
| Command log growth per note written and edited | `$commandlog` per tenant | the first real input to the pruning cadence |
| Token refresh: whether a page open longer than `ORDO_SESSION_SECS` stays signed in | leave a tab open | the refresh-before-expiry rule, tested by a person |

---

## 10 · Open for the owner [open]

Two questions, both on the platform feature, are in
`plan-provisioning-on-signup.md` §6: whether v1 needs an idle policy for
tenants of people who never return (recommend no, measure first), and the
two sign-up caps' defaults (recommend 60 an hour for the deployment, 5 an
hour from one address).

Decided here as the app developer's, under a ruling, and listed so they are
visible:

- **A share lasts 365 days** and is renewed by sharing again. `register_share` makes the caller state an expiry and this is the app's.
- **A share is read-only** and shares the one op `myNotes`. Write-level sharing is a later version.
- **The platform writes the acl row at accept, as the sharer** (§5.2). The fallback is a two-step flow.
- **The relying party is `inf.smg.icu`** and the page is `notes.inf.smg.icu`, as the owner said. The earlier recommendation of `smg.icu` is withdrawn.

Whoever integrates this plan files the two questions on the board as asks
on a task, and adds both plan files to `docs/board-docs.jsonl` with
`board.py docs set`. This document names them and does not file them.

---

## 11 · What this plan does not do

- It does not wire passkey sign-in (S2 to S7), build provisioning on
  sign-up (P0 to P5) or build sharing (W1 to W4). It lists what it needs
  from each and the order.
- It does not build C8 or search.
- It does not decide what the console shows. Every setting named here is
  deployment configuration an op or the console can later drive.
- It does not change `examples/first-app`, which stays the unguarded
  end-to-end proof this one is measured against.
- It does not change anything about Cloudflare. N9 measures first.

---

## Built

Built 2026-09-29 on the branch `worktree-agent-a1bb20a51a704d848`, locally, on top of `7197828`. N1 to N5 and N8 are built. **N6, N7 and N9 are not**: they touch the live deployment and DNS, which the owner does himself, and the exact settings and steps are in `examples/notes/README.md`, section 8. Nothing under `crates/` was changed except one new test file.

What the app is: `examples/notes/notes.ordo` (the program), `examples/notes/web/` (the page) and `examples/notes/dev/serve.sh` (a local deployment). The proofs are `crates/ordo-host/tests/notes_app.rs`, which boots the real `ordo-host` binary against real Postgres, and `examples/notes/web/test/`.

### How it is proved

`notes_app.rs` sets the settings the README gives a deployment owner: passkey sign-in, `ORDO_AUTH_PROVISIONING=auto-onboard`, `ORDO_PROVISION_SELF_SERVE=on-signup`, and the page's origin in `ORDO_CORS_ORIGINS` and `ORDO_AUTH_ORIGINS`. It then acts as the page does, sending the bytes `clients/ordo-sdk` sends (`auth/passkey.ts`, `auth/share.ts`, `core/client.ts`), with the software authenticator (`test-support`) in place of `navigator.credentials`. One test also runs the page's own modules under Node against the same binary (`web/test/live.test.ts`). Five tests, all passing, and 12 consecutive full runs passed. One earlier full run failed four of the five, with notes written and read back empty, and did not recur. Its cause was not found.

### N1 · The program

`notes.ordo` is the plan's §3.1 with three changes, each forced by what the platform does today, and each explained in the file and in the README, sections 1 and 7.

| Construct the plan could not confirm | Result | Used |
|---|---|---|
| `text(n)` as a field bound | parse error | `text` |
| `contains $q` | compiles, not run | not used |
| `notes(id == $id)[…]` | compiles, not run | not used |
| `std.notesAccess.list` | compiles, and answers the owner's `admin` row and each guest's `read` row | the page lists readers with it |
| `datetime = now` as a shape default | passes `ordo check`, refused when the DDL is lowered | a `created` field the page stamps |

Changes to the program: id parameters are `text`, because the door refuses the ids the platform mints (`is_typed_id` allows one underscore and the ids carry two); `deleteNote` empties the note and marks it `gone`, because a note always has its creator's `admin` row and a `del` on a referenced row is refused even for an `owned` reference; `createNote` takes a `created` stamp and `myNotes` sorts on it, because ids do not sort by age past nine. `unshareNote` is added (the derived `revokeNotesAccess` takes `id`).

Done-when: `ordo check` prints the derivation. Two people's notes are separate and a person calling another's tenant is refused: proved in `two_people_sign_up_and_each_keeps_notes_in_a_tenant_of_their_own`. **The plan says `not_found`. The door answers `access_denied`, 403, byte-identical to the answer for a tenant that does not exist.** A `deleteNote` removes the note from the list and the text from the database.

### N2 · The page

React, Vite, `@ordo/sdk`: sign up, notes list, new note, edit with compare-and-set, delete, share, Shared with me, accept a link. A save from a stale copy shows what is stored and lets the person keep theirs or take the other. Two saves at one revision, the second `cas_miss`, are in `notes_app.rs`; the screen is `NoteEditor.tsx`, and the logic is tested in `web/test/notes.test.ts` (36 page tests in all against a stubbed door, and seven more that need a running deployment).

**`ordo dev` cannot serve the page.** It sets `auth: None` and `cors: None` in `dev_runtime_config`, so `ORDO_CORS_ORIGINS` is ignored and there is no sign-in. The page runs against `ordo-host` (`dev/serve.sh`). For notes alone against `ordo dev`, Vite forwards the door's routes from its own origin (`ORDO_DEV_DOOR`) and a development token can be pasted; that path was run with `curl` through Vite and not in a browser. No browser was driven at any point: the page was type-checked, built and tested, and nobody has looked at it.

### N3 · Sign-up and sign-in

A registration answers a token and `space`, the person's own tenant. Proved: two people get different tenants and each is that tenant's one owner in its own database; a later sign-in lands in the same one; `GET /auth/spaces` lists it; refresh answers a new token and logout ends the session (the token is refused, a refresh of it is `401`, a sign-in after starts a clean one). The token is held in memory only, so **a reload signs the person out**, and one passkey touch signs them in again. The plan's "refreshing keeps the session" is read as the token renewal, not the page reload.

**Sign-up answers before the door serves the tenant.** The door learns of a new tenant by a notification and rebuilds. The first call can be `access_denied` for 65 to 120 ms as measured, and the page waits up to six seconds (`untilServed`). The same applies to a new seat after an accept, for `/auth/spaces`.

**Routing is no longer a setting.** Since 2026-10-06 the `ORDO_PG_ROUTING` setting is gone. Every tenant has a database of its own, and `notes_app.rs` sets no routing or credentials settings.

The plan's N3 looks for the provision time in `ordo dev`'s log. Neither `ordo dev` nor `ordo-host` prints one.

### N4 · Another origin

For the listed origin every preflight the page sends is answered `204` with the origin and no credentials header: `/op/myNotes`, `/op/createNote`, `/op/deleteNote`, `/auth/register`, `/auth/complete`, `/auth/share/accept`, `/auth/window`, `/auth/spaces`. An unlisted origin gets `403`. With `ORDO_CORS_ORIGINS` unset the preflight is `405`, from the router. The page turns a failed fetch into a message naming `ORDO_CORS_ORIGINS` and `ORDO_AUTH_ORIGINS` and its own origin (`friendly`, tested). The sync stream was not tested from a browser origin.

### N5 · Offline

`Device` (`web/src/offline.ts`) keeps `myNotes` in the SDK's replica through the sync manager. Proved against the binary: notes reach the device; a reopened replica reads them with the network off; a raw dump of the IndexedDB holds no title, body, id, tenant or person; a write with no network is refused and changes nothing; sign-out leaves no record and no key.

What it does not do: no writes offline; the page must stay open, because a reload offline shows the sign-in screen (no token is kept, and the replica cannot be opened without knowing who); the seal is not a lock (the key is in the same IndexedDB); no retention policy.

**The server does not push to a tenant made after it booted.** `PgInvalidationBus::refresh`, which adds a listener for a newly attached database, has no caller in the host, so a `?live=1&sync=1` feed on a new tenant carries the load and then heartbeats. Observed: a note written after the feed opened did not arrive in ten seconds. The page therefore opens the feed again after each write, on focus, when the network returns and every half minute. That is polling with the SDK's own manager. The fix is one call at provisioning and a periodic tick.

### N8 · Sharing

Proved in `a_note_shared_with_another_person_is_read_only_and_can_be_taken_back` and, for the reader's device, in `web/test/live.test.ts`: `std.space.share` for one note and `myNotes`; peek; accept; the ticket spent; the reader listed as a guest; the window reads the shared note and no other; every write is refused through it in every shape, and the reader's own session is no way in; a window token is refused in any other tenant and cannot open a window; a second share to the same person reuses the seat; the sharer lists readers with `std.notesAccess.list`; unshare takes one note off the reader's next load; `std.space.revokeShare` refuses the next call, refuses a new window, and drops the guest listing. On the reader's device the shared note appears, leaves on unshare, and the whole window is erased when the sharer closes it. Acceptance is recorded in the sharer's log and the deployment's security log, as ruled. The plan's "both chains carry the consent" was not asserted in the test.

The share and revoke buttons call platform ops, which ask for a passkey touch no older than five minutes; the page asks once when the door refuses and tries again (`withPresence`). That path is tested against a stubbed door and not against the binary, which would need a five minute wait.

### N8b · Share links on the page

Built 2026-09-29. The share panel offers two ways side by side, each labelled: **Anyone with the link can read** (`std.space.shareLink`, a `#/link/<secret>` address shown once with its expiry, and a list of the note's links with **Revoke link** from `std.space.links` and `std.space.revokeLink`) and **Share with a person** (the guest invitation, unchanged). The link route (`LinkView`) calls `openLink` and shows the note read-only. It is checked in `App` before the sign-in screen, and no session is created for it, so it works in a private window. An expired or revoked link is one message, because the door gives one answer. The three platform op specs are hand-written in `web/src/platform.ts` (`ordo gen ts` writes only the program's ops), so `gen.sh` is unchanged. Proved in `web/test/link.test.ts` (stub door) and in a new case in `web/test/live.test.ts`, run by `the_page_keeps_notes_on_the_device_and_erases_a_closed_share`: mint as the owner, read with no session, refused writes, refused on an open client and on a new open after revoke. The route's precedence over sign-in in `App` is not covered by a test, because the page has no DOM test harness.

### What the platform owes the app

Listed in `examples/notes/README.md`, section 7, with the fix for each: the id check; the delete of a referenced row and `subtree` on a non-tree entity; the missing notification listener for new tenants; `ordo dev`; the `$deployment` file; `PasskeySession` carrying no `space`; the `datetime = now` default that `ordo check` accepts; the per-address sign-up cap, which counts the peer of the socket the server sees, so when the door's traffic arrives through Cloudflare's proxy it counts Cloudflare's edge (the README says to set it `unlimited` until `ORDO_TRUSTED_PROXY_HOPS` exists, and that the plan's "sixth sign-up refused" then cannot be observed).

### Server gaps closed

Checklist §25, closed on the branch `worktree-agent-ae4083d57efcf39cc` on top of `3c5a02d`, one commit each. Each has a test that failed without the fix.

| Gap | Fix | Where | Test |
|---|---|---|---|
| 25a, the door refused minted ids | `is_typed_id` takes a letter-led key, then `_`, then segments joined by `_`; the key may hold a hyphen (`note-acce_…`) | `crates/ordo-api/src/bind.rs` | `wire_shapes`; and `notes_app.rs` calls `editNote`, `deleteNote` and `unshareNote` with minted ids again |
| 25c, no live updates for a tenant made after boot | `PgInvalidationBus::refresh` is called when the members notification arrives (sent when a tenant is created or attached, heard on every node) and on the access refresh tick. The design in `invalidation.rs` names both | `runtime.rs` (`spawn_deploy_listener`, `pg_bus`), `workers.rs` (`AccessRefresh`) | `a_feed_on_a_tenant_made_after_boot_carries_a_note_written_later` |
| 25b, `subtree` on a non-tree entity failed at run time | Refused when the op is registered (`subtree_not_tree`), so `ordo check` reports it | `crates/ordo-plan/src/writes.rs` | `subtree_on_an_entity_with_no_tree_ref_never_registers` |
| 25d, `ordo dev` had no sign-in and no CORS | `dev_runtime_config` reads `ORDO_CORS_ORIGINS` and the passkey settings through `RuntimeConfig::cors_from_env` and `auth_from_env` | `crates/ordo-cli/src/dev.rs`, `runtime.rs` | `dev_honours_cors_and_passkey_settings_when_set_and_ignores_them_when_not` |
| 25e, `datetime = now` passed check and failed DDL | `ordo check` runs the DDL printer, so it refuses what apply would (`unsupported_default`). `now` itself is not built | `crates/ordo-cli/src/lib.rs`, `crates/ordo-schema/src/ddl.rs` | `a_default_the_ddl_cannot_lower_is_refused_by_check` |
| 25e, `$deployment` undocumented | `deploying.md`, §4d | `docs/deploying.md` | none (prose) |

**Left open.** The delete of a row with owned children (25b's first half) is with the owner: the spec says owned links cascade (`definition-language.md`, "Ownership", "Deletes: owned cascades") and also that owned subtrees need the explicit `subtree` form (§6 rule 6), and `subtree` is built for tree entities only. So the notes app still soft-deletes with `gone`. From 25e: `single` routing losing a provisioned tenant's reads, the door serving a new tenant up to 120 ms after sign-up answers, and minted ids not sorting by age.

**Workarounds removed:** the `text` id parameters (`editNote`, `deleteNote`, `unshareNote` take `id`, and the generated client is regenerated). **Kept:** the `gone` soft delete, the page-stamped `created`, and `dev/serve.sh`, which `ordo dev` cannot replace because it has one space and no self-serve provisioning. The page's re-opening of the feed is redundant now and was left as a safety net.

### Not done

N6, N7 and N9. C8, search and writes offline (§2). The plan's §10 asks were not filed on the board, as instructed.
