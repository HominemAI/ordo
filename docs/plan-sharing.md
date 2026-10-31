# Sharing between tenants, guests, and share links

> **Answered 2026-09-29** (`corpus-rulings.md`): acceptance recorded in the sharer's log and the deployment security log; share links on by default, 7 days by default, no platform maximum; a guest writing is post-deployment (18ab). Section 7 is answered. **Link lifetime is layered** (owner, same day): the deployment's default, a tenant's own choice when the app developer exposes it, and each link's lifetime within that. `ORDO_SHARE_LINK_MAX_SECS` as a fixed deployment cap is withdrawn; L1 builds a tenant-level setting, stored like E4's admission policy, that the app developer can expose.

Checklist `11-2 / 11-3` (sharing between tenants, ruled 2026-09-08), `11-2a`
(share links), and the guest half of `5g-rev`. The owner, 2026-09-29:

> "Plan share links / the ability to have sign-inless / external grants. Make
> sure our existing choices / plans do not collide with anything we've
> currently designed or is built. Plan and implement guests - inter-tenant
> sharing is an important piece that we went through at the very begining
> because it coincides with things like sharing finances. We had designed a
> couple of methods one being a "window" - lets revise our design choices and
> also consider - if deployments own users than a user can be multi-tenanted
> and could get exposure through that lens - but a window could also make
> sense."

This plan supersedes `docs/plan-first-program.md` §5 (W1 to W4), the notes
app's first cut at sharing. Its findings are kept and cited; its increments
are replaced by G1 to G9 and L1 to L5 below. The notes app's N8 now depends
on G1 to G7.

Companion documents: `docs/sharing-options.html` (the 22-point consent
surface and the three architectures; still the reference for the consent
points A1 to E6), `docs/plan-enrolment.md` (E1 to E7, built),
`docs/plan-passkey-signin.md` (S0 to S7, built),
`docs/plan-provisioning-on-signup.md` (P0 to P5, planned),
`docs/plan-row-bound-seals.md` (built on a branch).

Markers: **[settled]** ruled by the owner or restated from a settled document
· **[plan]** proposed here · **[seam]** work another plan owns · **[open]**
needs the owner.

Status: **planned, not built.** Every claim about the code was read against
the tree at `f6d8231` on 2026-09-29. Nothing has deployed, so no shape change
owes a migration.

Words. The code and the language say "space". This plan says "tenant" for
the same thing, because that is the word the owner uses. A "guest" is a
person bound into a tenant they do not belong to. A "seat" is the members
row that binds them. A "window" is what a guest reads through.

---

## 0 · The starting state, verified

What exists, where, and what each piece is missing. Line numbers are from
`f6d8231`.

| Piece | State | Where |
|---|---|---|
| The 2026-09-08 ruling: Option A, grant-shaped, no syntax; a guest is an ordinary member holding one grant; no guest role exists | **ruled** | `docs/roadmap-checklist.md:3952-3962` |
| A guest's read executes inside the sharer's tenant under its guards, `$caller` bound to the guest's seat; the guest sees exactly the rows their acl row names; no role fallback; a write at read level is `access_guard`; revocation closes the window at the next call | **built and tested** against Postgres, GC059 to GC062 and GC066 | `crates/ordo-conformance/tests/guards_corpus.rs:1637-1690`, listed at `:2743-2744` |
| The corpus binds the guest at the executor, as `mem_g`, through `Fixture::run_case_as`. It never passes the host's door | fact that shapes G1 | `guards_corpus.rs:41-51`; the seat is `seed.sql:97`, role `NULL`; its acl row is `seed.sql:133` |
| `register_share`: a share is a `NewGrant` with a foreign principal and no client; an unguarded op refuses the whole share (`guest_exposure_unguarded`) | **built, no caller** outside its own three unit tests | `crates/ordo-identity/src/grants.rs:239-266`, tests `:268-360` |
| The grant registry: `record`, `revoke`, `callers_of`, `grants_of`, `is_live`; per (grant, op) rows with the whole pinned shape; **not consulted to admit a request** | **built** | `grants.rs:20-34`, `:130-174`; `crates/ordo-host/src/grants.rs:46-90` (`$capabilitygrant`), opened on the control pool at `runtime.rs:1564` |
| The capability path at the door: tenant first, expiry, per-op drift (fatal drift is `ManifestChanged`), allowlist, never falls through to roles | **built** | `crates/ordo-identity/src/access.rs:767-792` |
| `$caller` for a capability token whose principal holds a seat is that seat's id; with no seat it stays the principal id, which no guard matches | **built** (E1) | `crates/ordo-host/src/lib.rs:1516-1527` |
| Delegation: its own record and table, consulted at the door on every delegated call, `delegation_required` decided by a token gate before the op is known; an unreachable registry refuses delegated calls and nothing else | **built** | `crates/ordo-identity/src/delegation.rs:1-70`, `crates/ordo-host/src/delegation.rs`, `lib.rs:1385-1430` |
| `engine_from_members` reads a `NULL` role as **viewer** ("an absent role must not read as absent membership"), and the profile gives every role every read op | **built, and it is the gap** | `lib.rs:1725-1732`, `:1759-1761` |
| E1: a person is one deployment principal with a members row per tenant; `MEMBERS_SQL` reads `principal`; the engine keys by `(space, principal)` | **built** | `lib.rs:1626-1631`, `access.rs:492-520`, `docs/plan-enrolment.md:401-413` |
| `std.person.tenants`: a person's own list of tenants, every role, no presence | **built** | `crates/ordo-host/src/platform_ops.rs:162-176`, `docs/plan-enrolment.md:611-648` |
| The membership commands `std.space.admit`, `setRole`, `suspend`, `reinstate`, `remove`, owner-only, rebuilding the door and announcing on `ordo.members` | **built** (E3) | `crates/ordo-host/src/member_ops.rs:1-50` |
| Tickets: a hashed bearer secret, a purpose (`Password`, `Passkey`, `Invitation`), a tenant, a role, an optional named holder, a 30-day cap, redeemed once | **built** (E2) | `crates/ordo-host/src/tickets.rs:57-68`, `:91-108`, `:130-141`, `:208` |
| Presence on platform ops is the app developer's setting, `ORDO_STEP_UP_EXEMPT_OPS`; an exempt op may be granted to a capability token | **built** | `crates/ordo-host/src/presence.rs:53`, `lib.rs:1962-1966`, `docs/deploying.md:254` |
| A tenant's members live in its own database; the door reads each tenant's members from the database it lives in | **ruled and built** | `docs/corpus-rulings.md:5471-5495`, `crates/ordo-host/src/directory.rs:120-160` |
| Under routing a window is a second connection to a second database and can never be joined against the guest's own rows | **recorded** | `docs/corpus-rulings.md:5380-5388` |
| Keys are per tenant; a sealed read is a logged key release `release(space, Purpose::Read, correlation)` | **built** | `crates/ordo-crypto/src/custody.rs:233-258`, `crates/ordo-host/src/blob.rs:720`, `replay.rs:241` |
| Row-bound seals: the label is `seal:{entity}:{field}:{kind}:{id}` | **built on a branch, not merged** | `docs/plan-row-bound-seals.md:9`, §6 |
| The device replica: per tenant and principal, sealed, purged and poisoned on revocation; `std.space.*` never copied | **built** | `clients/ordo-sdk/src/replica/replica.ts:120-144`, `docs/corpus-rulings.md:9368-9373` |
| Share links: minting is an op, logged, expiring, revocable; bearer, so expiry is mandatory; `private` content serves with the key in the URL fragment | **settled, not built** | `docs/definition-language.md:451-458`, `docs/roadmap-checklist.md:3944-3950` |
| Blob presigning and `blob private` serving | **not built** | `crates/ordo-blob/src/s3.rs:49-55`, `docs/definition-language.md:382-383` |
| Feeds: routines, triggers, egress, `source:feed-*` attribution, SH-2's re-indexing | **not built** | `docs/sharing-options.html` Option B; `docs/corpus-rulings.md:9337-9344` |
| The auth routes: `begin`, `complete`, `logout`, `refresh`, `register`, password redeem and change, step-up, `.well-known/webauthn`. No share, window or link route | **built** | `crates/ordo-host/src/auth.rs:1513-1533` |
| `GET /auth/spaces` with `via: guest` | **planned** (P4) | `docs/plan-provisioning-on-signup.md`, P4 row |

One correction to `docs/plan-first-program.md` §5.1. It cites the viewer
reading at `lib.rs:1711-1718`; at `f6d8231` it is `lib.rs:1725-1732`.

---

## 1 · What binds this plan [settled]

| | Rule | What it fixes here |
|---|---|---|
| Ruled 2026-09-08 | Option A: the window first, grant-shaped, no syntax. A guest is an ordinary member holding one grant. No guest role | the seat is a members row; the authority is a grant; nothing is added to the language |
| The owner's principle, 2026-09-29 | Ordo provides the mechanism; the app developer decides what is enforced, with settings and defaults (`corpus-rulings.md:9454-9474`) | who may share, whether open links exist, how long a link lives, and what presence a share needs are settings with defaults |
| P1, P6 (`corpus-rulings.md:9201`, `:9226`) | Credentials and identities belong to the deployment. One person, one identity, many tenants | a guest is the same principal everywhere. The sharer never learns a credential, only a seat |
| DL §3, §8 | A guest gets a members row with a null role, minted by the platform at accept; the name disclosed is the guest's consent; invitation, never enumeration; guard-bearing ops only; revocation stops access, not memory | the seat, the ceremony, the exposure rule, and what revocation honestly does |
| `security-logging-design.md:217-225` | A grant is scoped to exactly one tenant; mandatory expiry; two tenants are two grants | one share grant per (tenant, guest); a share states its expiry |
| Ruled 2026-09-14 | A tenant's members live in its own database | the seat is written to the sharer's tenant's database; the grant to the control database |
| 5g-rev, ruled 2026-09-12 | "windows are safer use of sharing"; revocable local caching is windows only | the window is the default; nothing is copied |
| The notes ruling, 2026-09-29 | A person can share a note with another person; one person one tenant | the concrete case this plan is measured on |

---

## 2 · Revising the design: window, membership, feed [plan]

### 2.1 The lens the owner added

The deployment owns people (P1, P6). Since E1, a person is one principal
with a members row in each tenant they belong to (`lib.rs:1626-1631`). So
"admit the other person into my tenant" already exists as a mechanism: a
members row keyed by their principal, and the door serves them there under
whatever that row confers.

The question is what the row confers. Two answers exist in the tree today:

| The row's authority comes from | What the person reaches | Where the reach is decided |
|---|---|---|
| **a role** (owner, manager, editor, viewer) | every op the tenant's profile lists for that role, tenant-wide; today that is every read op for every role (`lib.rs:1759-1761`), filtered per row by guards where an op has one, and unfiltered where it has none | the profile, at the door, human path (`access.rs:793-812`) |
| **a grant** (a capability token) | exactly the ops listed, each pinned to what it meant at consent, filtered per row by guards; never a role's ops | the token, at the door, capability path (`access.rs:767-792`) |

A guest, in the ruling's words, is "an ordinary member holding one grant".
So a guest is the second row of that table, and a viewer is the first. **The
window and multi-tenant membership are the same mechanism.** Both bind the
person as a member of the sharer's tenant. Both attribute their calls in the
sharer's chain as themselves. Both use the same guards and the same
`$caller`. They differ in three defaults:

| Default | Window (guest) | Membership with a role |
|---|---|---|
| `members.role` | `NULL` | `viewer` or higher |
| Door authority | a share grant: named ops, pinned, expiring | the role's profile: tenant-wide, no expiry |
| Token used in the sharer's tenant | a window token (capability-shaped, minted from the session for that tenant) | the session token |
| Reaches an unguarded op | never (`register_share` refuses it: `grants.rs:249-253`) | yes, if the profile lists it |
| Ends by | revoking the grant, or the seat leaving; expiry | `std.space.remove` or `suspend`; no expiry |
| Promotion | `std.space.setRole` on the seat turns a guest into a member; the grant may stay or be revoked | n/a |

This plan therefore builds one thing, the seat, with two authorities on top
of it, and recommends the guest default for sharing. The owner's lens is
right about the mechanism and this plan says so plainly: exposure comes
through membership in both cases. What the window adds is the narrower
authority and the expiry.

### 2.2 The comparison

Two concrete cases. **Finances**: two people, each with a tenant of their
own (built-in provisioning), one wants the other to see the household
accounts and transactions, and may later want them to categorise. **A
note**: one person shares one note, read-only, from their tenant.

| | Window (guest seat + share grant) | Membership with a role (seat + viewer) | Feed (copies into the receiver's tenant) |
|---|---|---|---|
| **What the receiver sees** | the ops listed in the share, over the rows their acl rows admit. Finances: `listAccounts`, `listTransactions` over the two shared accounts. Note: `myNotes` over one note | every op the viewer profile lists, tenant-wide, guard-filtered where guarded and unfiltered where not. Finances: every read op in the finance program, including any unguarded summary. Note: every read op in the notes program | a copy of a named query's output, as their own rows in their own tenant, under their own guards |
| **What the sharer can take back, and how fast** | one row: `revoke<Entity>Access`, next call. Everything: `std.space.revokeShare`, next call (G5). Expiry ends it unattended | `std.space.remove`, next request on this replica and within a notification elsewhere (`member_ops.rs:26-33`). Nothing ends it unattended | future copies only. Copies already sent are the receiver's outright (DL §8) |
| **The sharer's audit chain** | the seat mint, the acl grants, and every guest call, attributed to the guest as themselves; `callersOf` lists the guest under the token via | the admit, and every call, attributed to the person; `callersOf` lists them under the role via | the sharer's side of the consent and each batch's key release |
| **The receiver's audit chain** | nothing lands in the receiver's own tenant. The consent is in the sharer's chain and in the deployment's security stream against the receiver's principal (§3.4) | the same | every copy, as `source:feed-<name>`; their triggers fire on it |
| **Per-tenant databases** | the seat and the acl rows are in the sharer's database, where the rows are (ruled 2026-09-14). The grant is in the control database with every grant. A window is a second connection to the sharer's database; it cannot be joined against the receiver's rows (`corpus-rulings.md:5380`). "My notes and notes shared with me" is two reads merged by the host or the client, never one statement | the same | copies are local and joinable; the crossing is per batch, through the host |
| **Sealed fields** | the read runs in the sharer's tenant, so the host opens the field under the sharer's key, one logged release per read (`custody.rs:233`, `blob.rs:720`). No key crosses. Row-bound seals change nothing: the row is the sharer's. Client-first and `private` content is unsharable by window (DL:2929-2932) | the same | each batch is opened under the sharer's key and re-sealed under the receiver's key and the receiver's row id. Row-bound seals make a byte copy impossible, which is SH-2 enforced in the seal layer. A standing, indefinite key release, unspecified anywhere |
| **Cost to build** | the ceremony, the seat reading in the engine, the window token, the registry gate at the door: G1 to G7 | G1 plus the existing `std.space.admit`, which is built. Cheapest, and the widest door | routines, triggers, egress, `fed`, R-G20, SH-2, the re-seal pipeline. Not started |
| **What an app developer writes** | nothing in the program. `acl` on the entity gives the guards and the grant ops. The app calls `std.space.share`, shows the accept page, and calls `openWindow` | nothing. The app calls `std.space.admit` with a role | a named query per feed, and the consent on both sides |
| **Fits when** | "let them see this, and then stop": a note, an account, a client's folder, an accountant for the quarter | the person is a participant, not a visitor: a spouse who should see the whole household, a colleague joining. For the finances case, the honest answer is often one household tenant with two members (Option C as modelling guidance), which is admission, not sharing | the receiver needs the data offline, under their own custody, driving their own automation; or the sharer's tenant is client-first |

### 2.3 Recommendation

**The window is the default for sharing between tenants.** It is what was
ruled, its enforcement half already passes against Postgres, it is the only
door 5g-rev can make revocable on a device, and it exposes the fewest ops.
Membership with a role is the same mechanism with a wider default, and it is
already built (E3); this plan makes the two interchangeable on one seat
(`setRole` promotes a guest; a guest's grant on a role-holder is redundant
and allowed). Feeds stay after the automation tier, as the 2026-09-08 ruling
already ordered; nothing here needs one.

For the finances case specifically: two people who both live in the numbers
should share one tenant as members. A person who only checks in, or an
accountant, gets a window. Both are two commands away from each other on the
same seat.

---

## 3 · The design [plan]

### 3.1 The seat

A members row in the sharer's tenant: `principal` = the guest's deployment
principal, `role` = `NULL`, `displayName` = the name the guest consented to
disclose, `status` = `active`, minted by the platform at accept as an
attributed command in the sharer's chain (DL:1004-1010). It is the row
`seed.sql:97` already shows.

The engine reads it as a guest: bound for `$caller` (`member_id_of`), no
role, no profile allowlist, so a session token presenting it at the door is
refused on the human path with a new `DenyReason::NoRole` (wire:
`access_denied`, like every denial). `spaces_of` reports `role: None`.
`membership_reaches` is false for a guest, so a guest can never be a
delegator's reach and never satisfies the "sharer's own reach" rule below.

One seat per person per tenant, because `principal` is unique within a
tenant (DL:969) and the engine keys by `(space, principal)`. A second share
to the same person reuses the seat. A person who is already a member is not
given a seat; the share adds acl rows and a grant to their existing row.

### 3.2 The share grant

`register_share` as built (`grants.rs:239`), recorded through
`PgGrantRegistry::record`, with these rules added in the host:

| Rule | Why | Where |
|---|---|---|
| Guard-bearing ops only | built: `grants.rs:249-253` | unchanged |
| **No platform op**: nothing under `std.space.`, `std.person.`, `std.deployment.` | platform ops are pinned `unguarded: false` (`lib.rs:1930-1936`, `:1979-1985`), so `register_share` alone would accept `std.space.callersOf`; step-up keeps a capability token out of it today, and `ORDO_STEP_UP_EXEMPT_OPS` can remove that (`deploying.md:254`). A share is a window, never administration | `share_ops.rs`, G2 |
| **The sharer's own reach**: every listed op is one the sharer may call, by `membership_reaches` | the same rule delegation has (`DelegationBeyondDelegator`, `access.rs:762-765`): a share carries only the sharer's authority | `share_ops.rs`, G2 |
| **The sharer administers every row named**: the derived `grant<Entity>Access` succeeds for each, as the sharer | R-G13: an acl row is written by the holder of admin on it, or the owner at the door | G3, by running the op |
| One live share grant per (tenant, guest); a second share is revoke-then-record with the union of ops and the later expiry | the window token carries one grant; revocation names one id; the same rule delegation uses (`AlreadyLive`) | G3 |
| The id is minted with the prefix `shr_` | the door consults the registry for these and no other grants (§3.5) | G3 |
| Expiry is stated by the sharer, capped by the tenant's ceiling `ORDO_SHARE_MAX_SECS` (default 365 days) | `grants.rs:237-238`: the caller states it; DL: TTL bounded by tenant policy | G2 |

### 3.3 The ceremony: invitation, never enumeration

The sharer does not know the reader's principal and never sees a list of
people. The share travels as a ticket (E2), purpose `Share`.

| Step | Who | What |
|---|---|---|
| 1 | the sharer | `std.space.share($ops, $rows: [{entity, id, level}], $expires, $to?, $label?)` in their own tenant. Validated per §3.2. Issues a ticket: purpose `Share`, `space` = the sharer's tenant, `principal` = `$to` when the sharer names a person they have shared with before (the seat's principal), else `None`, `display` = `$label`, payload = the intent, TTL ≤ 30 days (`tickets.rs:57`). Answers `{ticket, expires_at}`. Presence per the app developer's setting, like every platform op |
| 2 | the reader | opens the app's accept page with the ticket in the fragment. The page calls `POST /auth/share/peek {ticket}` (no token needed) and shows: the sharer's tenant name, the ops, the rows' labels the sharer typed, the expiry, the name that will be disclosed, and the owner-reach sentence (`sharing-options.html` B3) |
| 3 | the reader | signs in, or signs up (P3: a sign-in is an enrolment; under `on-signup` they also get a tenant of their own), then `POST /auth/share/accept {ticket, display_name}` with their session token |
| 4 | the platform | redeems the ticket with `Want{purpose: Share, holder: the caller}`; mints or reuses the seat in the sharer's tenant, attributed to the reader; runs `grant<Entity>Access` per row **as the sharer**, correlated to the share id (kept from the notes plan, §5.2, with its stated fallback); `register_share` and `record`; `announce_members`; records the consent (§3.4). Answers `{space, name, seat, grant, expires}` |
| 5 | the reader's app | `POST /auth/window {space}` (§3.5); `client.withSpace(space)` with the window token; reads |
| 6 | the sharer, later | `revoke<Entity>Access` on one row, or `std.space.revokeShare($seat)`; or expiry |
| 6' | the reader, later | `POST /auth/share/leave {space}` |

A ticket is single-use. Whoever accepts first holds the seat; the sharer
sees the disclosed name and can revoke. The sharer's page says so.

### 3.4 What is recorded, and where

| Event | Sharer's tenant chain | Deployment security stream | Registry |
|---|---|---|---|
| share minted | the platform op, attributed to the sharer, with the intent | | |
| accept | the seat mint (attributed to the guest); the acl grants (attributed to the sharer, correlated to the share) | `Enrolment { act: "share-accepted" }` against the guest's principal, naming the tenant and the grant | the grant, live |
| each guest call | the call, attributed to the guest as themselves | denials, as for anyone | |
| each sealed read | | `KeyReleased` for the sharer's tenant, purpose `read` | |
| revoke, leave, expiry | the acl deletes; the seat suspended | `Enrolment { act: "share-revoked" / "share-left" }` | `revoked_at` |

The guest's own tenant's chain gets nothing. DL §8 says "consent lands in
both spaces' chains"; under P6 the other party is a person, who may hold
many tenants or none, so the consent is recorded against the person in the
deployment stream instead. This is a departure from DL's wording and is
question 1 of §7.

### 3.5 The window token

`POST /auth/window {space}` with a session token answers a capability token
for the caller's live share grant in that tenant: `TokenSpec { principal,
capability: Some(grant), expires: min(grant.expires, now + ORDO_SESSION_SECS),
session: None, client: None }` (`token.rs:141-171`). The grant's pins are the
**consented** pins decoded from the registry (`pins::decode_pins`,
`pins.rs:377`), never re-pinned from the deployment, so a widened op refuses
the window with `ManifestChanged` exactly as it refuses any grant
(`access.rs:781`).

The token is bound to one tenant (`GrantSpaceMismatch`), carries no session
(logout ends it within one token life, by expiry), and cannot satisfy
step-up (`access.rs:56-57`), so no platform op is reachable through it
whatever the presence setting says.

**Revocation at the door.** `grants.rs:20-34` states the registry is not
consulted to admit a request, and names revocation as the one place the
boundary bends, for a deployment that chooses it. This plan chooses it for
share grants, on delegation's argument (`delegation.rs:38-59`): the person
who revokes a share is not the token's holder and gets no say in its life.
`IdentityAccess::check` consults `GrantRegistry::is_live` for a capability
token whose grant id starts `shr_`, after the engine allows, on every call.
An unreachable registry refuses those calls and nothing else. Ordinary
capability grants (`grn_`) keep today's rule.

### 3.6 Feeds

Not in this plan. When the automation tier exists, a feed is two share
grants (one to read the slice in the sharer's tenant, one to write copies in
the receiver's), a routine, and a per-row re-seal under the receiver's key
and row id. SH-2 (blind indexes recomputed under the receiver's key) and the
standing key release both need their own plan then.

---

## 4 · Share links and sign-in-less grants [plan]

A share link is a bearer credential: whoever holds it reads. DL settled the
shape (`definition-language.md:451-458`): minted by an op, access-checked,
command-logged, TTL bounded by tenant policy, revocable, expiry mandatory.
This plan puts it on the same seat and grant as a guest, so the door, the
guards, the audit and the revocation are the ones already argued.

### 4.1 Two kinds of link, kept apart

| | Invitation link (§3.3) | Open share link |
|---|---|---|
| What it carries | an offer: a ticket | the authority: a bearer secret |
| Who can use it | one person, who signs in or signs up; the seat is theirs | anyone holding it; no account |
| Used how many times | once | until expiry or revocation |
| Reads attributed to | the person | the link's own seat, `lnk_<id>` |
| Data access is | not bearer: the person's session mints the window | bearer |
| Forwarded, it | hands the offer to the first to accept, visibly | hands the data to everyone who receives it |
| Relation to 18s | this is 18s's shape: the link names the request, the session supplies the authority, and a ticket naming a principal binds it to the intended recipient (`tickets.rs:135-141`) | the opposite by design, and said so |

### 4.2 The open share link

| Part | Design |
|---|---|
| Mint | `std.space.shareLink($ops, $rows, $expires, $label)` in the sharer's tenant. Same validation as a share (§3.2) plus: **read ops only**, `$expires` ≤ `ORDO_SHARE_LINK_MAX_SECS` (default 7 days; hard cap 30 days, the ticket rule). Mints a **link seat**: a members row, `principal` = `lnk_<id>` (a new prefix; a principal that can never sign in), `role NULL`, `displayName` = `$label`; the acl rows as the sharer; a share grant (`shr_`) for the link principal; a `$sharelink` row in the control database: `hash` of the secret (`tickets::hash_of`), `space`, `grant`, `seat`, `expires`, `revoked_at`, `opens`, `issued_by`. Answers the secret **once** |
| Open | `POST /auth/link {link}` with no `Authorization`. Hash lookup; expired, revoked, unknown and malformed are one answer. Answers a capability token for the link seat (§3.5's shape, expiry min(link, session life)), increments `opens`, records `Enrolment { act: "link-opened" }` with the address. A miss is rate-limited per address on P3's pattern (18r question 2) |
| Read | the token calls ops with `X-Ordo-Space` = the sharer's tenant, through the ordinary door. Guards bind `$caller` to the link seat, so exactly the rows the sharer granted are visible |
| Files | `GET /blob/{ref}` with the link token. The blob door checks `std.<entity>.get` against the same checker (`crates/ordo-api/src/blob.rs:33-38`), so a link whose grant lists `std.<entity>.get` serves the file of a shared row and no other. Space-sealed bytes are opened by the host under the sharer's key, logged. `blob private` (key in the URL fragment) waits on F7 Phase 6 and presigning (`s3.rs:49-55`); filed, not built here |
| Revoke | `std.space.revokeLink($link)`: grant revoked, `revoked_at` set, seat suspended, acl rows deleted as the sharer. Refused at the next call through §3.5's gate |
| List | `std.space.links`: the tenant's links, label, rows, opens, expiry, state. Manager and owner. `callersOf` already lists the link seat under the token via |
| Setting | `ORDO_SHARE_LINKS` = `on` (default) or `off`; `ORDO_SHARE_LINK_MAX_SECS`. The app developer's, reachable by environment and later by op and console, on the claim code's pattern |
| Shortening (18r) | a link secret is long by construction. 18r maps a short code to a long link and resolves by redirect; the short code is never the authority. Nothing here waits on it; when it lands, a share link is one more thing it shortens |

### 4.3 How a link relates to the other credentials

| | Session token | Capability token | Delegation | Window token | Link token |
|---|---|---|---|---|---|
| Holder | a person | an app or agent | an agent acting as a person | a guest, in one tenant | whoever opened the link |
| Authority | role profile | its grant | the delegator's reach ∩ the grant | the share grant | the link's grant |
| `$caller` | the member | the principal, or its seat | the agent (11-4a open) | the guest seat | the link seat |
| Step-up | yes, by presence | never | never | never | never |
| Ends by | logout, expiry | expiry, revoke | delegator revokes | sharer revokes, guest leaves, expiry | sharer revokes, expiry |
| Registry consulted at the door | sessions list | no | yes | yes (`shr_`) | yes (`shr_`) |

A link token is a window token whose principal nobody owns. That is the
whole design, and why it costs L1 to L5 and no second door.

### 4.4 What a link can never do

1. Write. The mint refuses any op that is not a read.
2. Call a platform op, whatever `ORDO_STEP_UP_EXEMPT_OPS` says: refused by name at the mint, and a capability token never satisfies step-up.
3. Act as the sharer, or as anyone. Every read is attributed to `lnk_<id>`.
4. Reach an unguarded op, or a row the sharer did not grant.
5. Reach another tenant. The grant is scoped to one.
6. Outlive its expiry, or the ceiling.
7. Share onward. A link seat is a guest, and a guest cannot mint shares (§3.2's reach rule: a guest reaches no op by role).
8. Open a `sealed` field the sharer's tenant cannot release, or `private` content, until F7 Phase 6.
9. Be minted by a guest, a link, or a capability token: minting is a platform op under the presence setting, and the reach rule refuses a caller with no role.
10. Be found by guessing: the secret is at least 128 bits, hashed at rest, and misses are rate-limited.

---

## 5 · Collisions

Every existing design or built piece sharing touches, and what this plan
does to it. **Overturned** items are for the owner to see.

| Piece | Where | Verdict |
|---|---|---|
| The 2026-09-08 ruling: Option A, grant-shaped, no syntax, no guest role | `roadmap-checklist.md:3952` | **agrees.** Nothing is added to the language; the seat is a members row; the authority is a grant |
| Guest binding at accept, null role, disclosed name | DL:981-983, :1004-1010 | **agrees**, and G1 makes the engine honour it |
| DL §8 "an ACL row may name a principal who is not a member" | DL:2908-2912 | **extends** the wording: the guest holds a members row with no role, as §3 says and `sharing-options.html` §5 noted. G9 edits the sentence |
| DL §8 "consent lands in both spaces' chains" | DL:2925-2927 | **overturned**, §3.4: recorded in the sharer's chain and against the person in the deployment stream, because the other party is a person (P6). Question 1 |
| `engine_from_members`: `NULL` role reads as viewer | `lib.rs:1725-1732` | **overturned.** `NULL` is a guest. Today a null-role seat with a session token reaches every read op the viewer profile lists, including unguarded ones. G1 |
| `register_share` | `grants.rs:239` | **extends**: two host-side rules (no platform op, the sharer's reach) and the `shr_` id. The function itself is unchanged |
| The grant registry "is not consulted to admit a request" | `grants.rs:20-34` | **extends** along the bend the module itself names: consulted for `shr_` grants only, on delegation's argument. Ordinary grants unchanged |
| Delegation grants | `delegation.rs`, `lib.rs:1385-1430` | **agrees.** A share is not a delegation: a guest acts as themselves. The registry gate and the reach rule are reused as patterns, not as code paths |
| Capability tokens: one tenant, mandatory expiry, per-op pins, never step-up | `security-logging-design.md:217-225`, `access.rs:767-792` | **agrees.** A window token and a link token are capability tokens |
| The presence setting `ORDO_STEP_UP_EXEMPT_OPS` | `presence.rs:53`, `deploying.md:254` | **agrees**, and closes a gap it opens: an exempt platform op could be granted to a capability token, so shares and links refuse platform ops by name |
| 5g-rev: revocable local caching, windows only | `roadmap-checklist.md:824-856` | **agrees.** Nothing is copied. The online half (purge on refusal) already exists in the replica; the lease and server-released key stay 5g-rev's |
| The replica feed: op-scoped sync, per tenant and principal, poisoned on revocation | `corpus-rulings.md:5587-5600`, `replica.ts:120-144` | **agrees.** A window client syncs the shared ops in the sharer's tenant into a separate sealed replica; the refusal after revocation poisons it. G7 verifies with a test |
| Per-tenant databases; members in their own database | `corpus-rulings.md:5471`, `directory.rs:120-160` | **agrees.** The seat is in the sharer's database; the grant is in the control database like every grant; a window is a second connection; "shared with me" is merged by the host or client, never in SQL |
| Row-bound seals | `plan-row-bound-seals.md` §6 | **agrees.** A window opens the sharer's row under the sharer's key; nothing is re-anchored. A feed would have to re-seal per row, which is right |
| Built-in provisioning on sign-up, one tenant per person | `plan-provisioning-on-signup.md` | **agrees**, and **extends** P4: `GET /auth/spaces` lists guest seats with `via: guest`, `role: null`. A share is minted from a person's own tenant; a stranger who accepts one signs up and gets their own tenant in the same request |
| `std.person.tenants` | `platform_ops.rs:162-176` | **extends**: `role` is null for a guest seat. G6 |
| The notes plan W1 to W4 | `plan-first-program.md:302-317` | **superseded.** Kept: the flow of §5.2, the platform writing acl rows as the sharer, one seat per person, the window token minted from the session, the four test names' intent. **Overturned**: W1's `role = 'guest'` string in `members` becomes `NULL`, as DL and the seed say; W1's `set_guest` becomes `set_member` with `Option<Role>`. Added: links, the reach rule, the platform-op rule, the registry gate, the security stream record |
| SH-2: feed copies re-index under the receiver's key | `security-hardening-review.md:91`, `corpus-rulings.md:9337-9344` | **agrees.** A window copies nothing, so SH-2 does not arise. It stays owed by the feed (22l) |
| GC063 to GC065 in `NOT_RUN` | `guards_corpus.rs:2750-2758`, `guards-conformance.md:1410-1430` | **re-spelled**, as `sharing-options.html` proposed: registration cases through `register_share` and the host's validation, not a `share to guest:` line in source. G8 |
| 18r, the shortener | `roadmap-checklist.md:10801` | **agrees**: the short code is never the authority. Nothing waits on it |
| 18s, an emailed link that is not a bearer credential | `roadmap-checklist.md:10861` | **agrees**: the invitation link is that shape; its step 3 exists when the ticket names the recipient |

---

## 6 · Increments [plan]

Each is shippable alone. Each done-when names a test that fails without it.
New host tests go in `crates/ordo-host/tests/sharing.rs` and
`crates/ordo-host/tests/share_links.rs`, both new. Nothing below runs cargo
in this plan; the builders do.

### 6.1 Guests: no decision needed

| | What | Files | Done when |
|---|---|---|---|
| **G1** | **The seat.** `Membership.role: Option<Role>`; `AccessEngine::set_member(…, Option<Role>)`; `set_membership` keeps `Role` and wraps it; `engine_from_members` maps `NULL` to `None` (any fixture that relied on `NULL` as viewer is fixed; at `f6d8231` only `seed.sql:97` has one, and it is the guest); the human path with `None` denies `DenyReason::NoRole`; `spaces_of` returns `Option<Role>`; `membership_reaches` is false for `None`; `RecordedDecision::NoRole` in the security stream | `crates/ordo-identity/src/access.rs` (`Membership`, `set_member`, `check`, `spaces_of`, `membership_reaches`, `callers_of`), `crates/ordo-host/src/lib.rs:1725-1732`, `platform_ops.rs` (`my_tenants` reads `Option<Role>`), `crates/ordo-secevent` (the decision), every caller of `set_member` (E1's four, plus tests) | `sharing.rs::a_null_role_seat_binds_caller_and_reaches_no_op_by_role`: a principal with a `NULL` seat presents a session token for a read op the viewer profile lists and is refused `access_denied`; the same principal with a capability token listing the op reads the guard-admitted rows and `$caller` is the seat id. The first half fails at `f6d8231` |
| **G2** | **`std.space.share`.** The platform op: `$ops`, `$rows`, `$expires`, `$to?`, `$label?`; validation per §3.2 (`register_share`, no platform op, the sharer's reach, `ORDO_SHARE_MAX_SECS`); `Purpose::Share` with the intent as the ticket's payload (`TicketSpec` gains `payload: Option<String>`); manager and owner by default like the other tenant-scoped platform ops (`lib.rs:1948-1957`); presence per the app developer's setting | new `crates/ordo-host/src/share_ops.rs`, `tickets.rs` (`Purpose::Share`, payload), `platform_ops.rs` (spec, `owner_only` untouched), `runtime.rs` (`platform_specs`, the two settings, the census line), `docs/deploying.md` | `sharing.rs::a_share_ticket_carries_the_intent_and_expires`, `::sharing_an_unguarded_op_a_platform_op_or_an_op_beyond_the_sharer_is_refused_before_a_ticket_exists` (three refusals, no ticket row after each) |
| **G3** | **Accept.** `POST /auth/share/peek {ticket}` and `POST /auth/share/accept {ticket, display_name}`; `Want{purpose: Share, holder}`; the seat minted or reused (`principal` unique in the tenant), attributed to the guest; the acl grants run as the sharer, correlated to the share; `register_share` with a `shr_` id, revoke-then-record for a second share; `PgGrantRegistry::record`; `announce_members`; `Enrolment { act: "share-accepted" }` | `auth.rs` (two handlers, `router`), `serve.rs`, `share_ops.rs`, `crates/ordo-host/src/grants.rs` | `sharing.rs::an_accepted_share_binds_a_seat_records_the_grant_and_writes_the_acl_rows_as_the_sharer`, `::a_second_share_to_the_same_person_reuses_the_seat_and_replaces_the_grant`, `::a_ticket_naming_another_person_is_not_spent_by_the_wrong_holder`, `::a_member_who_is_shared_with_gets_no_second_seat` |
| **G4** | **The window token.** `POST /auth/window {space}`: the caller's live `shr_` grant in that tenant, pins decoded from the registry (`decode_pins`), expiry `min(grant, ORDO_SESSION_SECS)`, no session, no client | `auth.rs`, `crates/ordo-host/src/grants.rs` (a `live_share_of(space, principal)` query), `crates/ordo-identity/src/pins.rs` (only if `decode_pins` needs a public shape it lacks) | `sharing.rs::a_window_token_reads_exactly_the_shared_rows_and_is_refused_in_any_other_space`, `::a_window_token_carries_the_consented_pins_so_a_widened_op_refuses_the_window` (redeploy the op with `$caller` dropped; the next call is `access_denied`; `ManifestChanged` in the stream), `::a_window_token_cannot_step_up_or_call_a_platform_op` |
| **G5** | **Revocation.** `std.space.revokeShare($seat)`: registry revoke, acl deletes as the sharer, seat suspended, `announce_members`; `POST /auth/share/leave {space}` for the guest; `IdentityAccess::check` consults `is_live` for `shr_` grants after the engine allows; an unreachable registry refuses those calls only; `/auth/window` refuses a revoked grant | `share_ops.rs`, `lib.rs` (the gate, beside the delegation gate), `auth.rs`, `member_ops.rs` (suspend on a seat, reused) | `sharing.rs::a_revoked_share_is_refused_at_the_next_call_and_no_window_can_be_reminted`, `::a_guest_who_leaves_is_refused_at_the_next_call`, `::an_unreachable_grant_registry_refuses_window_calls_and_serves_members` |
| **G6** | **Listing.** `std.person.tenants` answers `role: null` for a seat; P4's `GET /auth/spaces` answers `via: "guest"` [seam: P4] | `platform_ops.rs`, `auth.rs` (with P4) | `my_tenants.rs::a_guest_seat_is_listed_with_no_role_and_a_member_is_not_listed_as_a_guest` |
| **G7** | **The SDK.** `acceptShare(client, ticket, displayName)`, `peekShare`, `openWindow(client, space)` answering a client bound to that tenant with the window token and re-minting before expiry, `leaveShare`; the replica for `(space, principal)` is poisoned on the first refusal after revocation (existing behaviour, asserted) | new `clients/ordo-sdk/src/auth/share.ts`, `index.ts`, `replica/sync.ts` (assert only), vitest with `fetch` stubbed | `share.test.ts::the_window_client_sends_the_window_token_and_the_shared_space_header`, `::accept_posts_the_ticket_and_the_name_byte_for_byte`, `::a_refusal_after_revocation_poisons_the_replica_of_that_space_only` |
| **G8** | **The corpus.** GC063 and GC064 become host tests over `share_ops` validation (an op with no reachable guard: `read_guard` absent and `caller_restricted` false on every entity; an `unguarded` op); GC065 asserts the recorded basis per op from the pins; `NOT_RUN` shrinks by three; `guards-conformance.md:257-258`, `:740`, `:1410-1430` re-spelled | `guards_corpus.rs:2750-2758`, `crates/ordo-host/tests/sharing.rs`, `docs/guards-conformance.md` | `sharing.rs::gc063_gc064_gc065_registration` and `NOT_RUN` no longer names them |
| **G9** | **Documents.** DL §8 wording (§5 above); `deploying.md` rows for `ORDO_SHARE_MAX_SECS`; `howto` page for app developers: share, accept page, window, revoke, what the guest sees; `plan-first-program.md` §5 pointed here | `docs/definition-language.md:2908-2912`, `docs/deploying.md`, a new `docs/howto-sharing.md`, `docs/plan-first-program.md` | `node scripts/board-agrees.mjs` passes and the checklist's 11-2 row cites this plan |

**Order: G1, G2, G3, G4, G5, G6, G7, G8, G9.** G1 stands alone and is the
one increment that changes behaviour a test already exercises. G3 needs G2;
G4 needs G3; G5 needs G4. G6 can go any time after G1. G7 after G4's JSON is
fixed. The notes app's N8 needs G1 to G7 and P4.

### 6.2 Share links: no decision needed unless question 2 moves the default

| | What | Files | Done when |
|---|---|---|---|
| **L1** | **The mint.** `std.space.shareLink`; read ops only; the link seat (`lnk_` principal, `NULL` role, label as name); the acl rows as the sharer; the `shr_` grant; `$sharelink` (hash, space, grant, seat, expires, revoked_at, opens, issued_by); the secret once; `ORDO_SHARE_LINKS`, `ORDO_SHARE_LINK_MAX_SECS` | `share_ops.rs`, new `crates/ordo-host/src/share_links.rs` (table, hash, sweep on the tickets pattern), `platform_ops.rs`, `runtime.rs`, `docs/deploying.md` | `share_links.rs::a_link_is_minted_once_logged_and_bounded_by_the_ceiling` (the secret is not in the chain, the hash is in the table, `expires` ≤ ceiling), `::a_link_over_a_write_op_or_a_platform_op_is_refused`, `::links_off_refuses_the_mint` |
| **L2** | **Open.** `POST /auth/link {link}` without a token; one answer for unknown, expired, revoked, malformed; the capability token for the link seat; `opens` incremented; the security event; misses rate-limited per address | `auth.rs`, `share_links.rs` | `share_links.rs::an_opened_link_reads_exactly_the_shared_rows_and_nothing_else`, `::an_unknown_expired_or_revoked_link_is_the_one_answer`, `::a_link_token_cannot_write_step_up_or_call_a_platform_op`, `::the_sixth_miss_from_one_address_in_a_minute_is_refused` |
| **L3** | **Revoke and list.** `std.space.revokeLink`, `std.space.links`; G5's gate refuses a revoked link at the next call | `share_ops.rs`, `share_links.rs` | `share_links.rs::a_revoked_link_is_refused_at_the_next_call`, `::the_list_shows_opens_and_state_and_never_the_secret` |
| **L4** | **Files.** A link whose grant lists `std.<entity>.get` serves `GET /blob/{ref}` for a shared row's file and refuses another row's; sealed bytes opened under the sharer's key and logged | `crates/ordo-host/src/blob.rs` (assert only; the door is shared), tests | `share_links.rs::a_link_serves_the_file_of_a_shared_row_and_not_another` |
| **L5** | **The SDK.** `openLink(link)` answering a client bound to the tenant with the link token; a page pattern in the howto | `clients/ordo-sdk/src/auth/share.ts`, `docs/howto-sharing.md` | `share.test.ts::open_link_posts_the_secret_and_binds_the_answered_space` |

**Order: L1, L2, L3, L4, L5**, after G5 (the gate) and G1 (the seat). L4 is
a test over existing code. `blob private` links are filed beside F7 Phase
6, not built here.

### 6.3 Waits on the owner

| Item | Waits on |
|---|---|
| Recording the consent in the guest's own tenant's chain as well (§3.4) | question 1 |
| Open links on by default (L1's `ORDO_SHARE_LINKS=on`) | question 2. If off, L1 ships with `off` and nothing else changes |
| Write-level shares for a bound guest (G2 accepting write ops) | question 3. G2 ships read-only until answered; the notes app needs read only |
| Feeds, SH-2's re-indexing, the standing key release | the automation tier; already ordered second by the 2026-09-08 ruling |
| Shortening a link (18r), `blob private` links (F7 Phase 6), a per-device lease and server-released key (5g-rev) | their own items |

---

## 7 · Open for the owner [open]

1. **Where the guest's consent is recorded.** DL says both tenants' chains.
   Recommend **the sharer's chain plus the deployment stream against the
   person**: the other party is a person, who may hold no tenant. Yes or
   no.
2. **Are open (sign-in-less) share links on by default?** Recommend **on**,
   7 days by default, 30 days at most, the app developer's setting.
3. **May a share to a bound guest carry write ops?** Recommend **yes, at
   the sharer's choice**, guard-bearing and within the sharer's reach as
   for reads; never for an open link.

Decided here under the rulings, listed so they are visible: a share is
minted by manager and owner by default, like the other tenant-scoped
platform ops; a share's default ceiling is 365 days; a link's secret is
never in any chain; the guest's own tenant is untouched by a share.

---

## 8 · What this plan does not do

- It does not build feeds, routines, triggers or egress.
- It does not build P4 (`GET /auth/spaces`); G6 adds `via: guest` to it when it lands.
- It does not build `blob private` serving, presigning, or the fragment key.
- It does not build 5g-rev's lease or server-released key.
- It does not settle `$caller` under delegation (11-4a). A delegated agent reading through a window is two open items, and neither is built here.
- It does not write the console. Every setting is an environment variable an op or the console can later drive.
- It does not touch the board, the checklist, `corpus-rulings.md` or `decisions.html`. Whoever integrates it files §7's three questions as asks on one task, records the supersession of `plan-first-program.md` §5 on the checklist's 11-2 row, and adds this file to `docs/board-docs.jsonl`.


---

## Built

Built on the branch `worktree-agent-a9a53afd8ed936661` on 2026-09-29, in the
order G1, G2, G5, G6, G8, then the documents. G3, G4 and G7 are not built,
and G3, G4 and G5's routes wait on `auth.rs` (another change is editing it).
Share links (L1 to L5) are not built. The owner's answers to §7 arrived
during the build: acceptance is recorded in the sharer's log and the
deployment's security log, share links are on by default, and a guest that
writes is after the first deployment (18ab). G2 is therefore read-only, and
nothing was built that depends on the other two.

### G1 · The seat

`Membership.role` is `Option<Role>`; `AccessEngine::set_member` takes
`Option<Role>` and `set_membership` wraps a `Role`. `engine_from_members`
reads a null role as a guest (`None`), and reads an unrecognised role word
the same narrow way; before, both read as a viewer. The human path refuses a
guest with `DenyReason::NoRole`, recorded as `RecordedDecision::NoRole` (tag
19, appended). `spaces_of` returns `Option<Role>`, `membership_reaches` is
false for a guest, and `callers_of` lists a guest by token only.
`std.person.tenants` reads the option.

Where: `crates/ordo-identity/src/access.rs`, `crates/ordo-secevent/src/event.rs`,
`crates/ordo-host/src/lib.rs`, `platform_ops.rs`.

Tests: `sharing.rs::a_null_role_seat_binds_caller_and_reaches_no_op_by_role`
(seen failing with the viewer reading restored), and two engine tests in
`ordo-identity/src/tests.rs`.

### G2 · `std.space.share`

The platform op, manager and owner by default, presence per the app
developer's setting. `share_ops::validate` checks each op (not a platform op,
deployed, a read, not `unguarded`, a reachable guard, within the sharer's
reach), the rows, and the lifetime against `ORDO_SHARE_MAX_SECS` (default 365
days), and the op issues a ticket with purpose `Share` and the intent as its
payload. The pure `validate` is what G8 runs.

Where: new `crates/ordo-host/src/share_ops.rs`, `tickets.rs`,
`platform_ops.rs`, `runtime.rs` (`platform_specs`, wiring, census line),
`docs/deploying.md`.

Tests: `sharing.rs::a_share_ticket_carries_the_intent_and_expires` and
`::sharing_an_unguarded_op_a_platform_op_or_an_op_beyond_the_sharer_is_refused_before_a_ticket_exists`.
Both failed by absence before the op existed, and the second was seen failing
with the platform-op rule disabled.

Differences from the plan:

- **`TicketSpec` did not gain a `payload` field.** It has literals in
  `credential_ops.rs` and four test files, so a field would have touched
  every caller. `TicketRecord` and `Redeemed` carry the payload and
  `Tickets::issue_with_payload` sets it; `issue` is unchanged. The `$ticket`
  table gains a `payload` column, added by `ALTER TABLE ... IF NOT EXISTS`.
- `$ops` is comma-separated text and `$rows` is JSON text, because a platform
  op's parameters are scalars. `$expires` is a `duration` (`30d`).
- **`$expires` runs from the moment of the share**, not from acceptance. The
  ticket lives at most 30 days, so a guest who accepts on day 29 of a 30-day
  share has a day. The window's end is fixed at the share.
- The reachable-guard rule (GC063) is in `validate`, beside `register_share`,
  because `register_share` checks only `unguarded`.
- `$to` must name a person who already holds a seat in the tenant.
- One live setting was added, `ORDO_SHARE_MAX_SECS`. The plan said "two
  settings"; the second is L1's.

### G5 · Revocation

`std.space.revokeShare($seat)` revokes the seat's live `shr_` grants, deletes
the acl rows of the shared ops' entities for a guest seat, and suspends a
guest seat (rebuilding the door and announcing). A member who holds a role
keeps the membership and their own rows. `ShareOps::leave(space, principal)`
does the same for the guest, for the route that will call it.

The gate: `IdentityAccess::token_gate` asks the grant registry whether a
token's `shr_` grant is live, on every call that presents one. A grant that is
not live, an unknown grant, no registry, and an unreachable registry all
refuse with the uniform `access_denied`, recorded as `RecordedDecision::
ShareNotLive` (tag 20). Nothing else asks the registry. `IdentityAccess` gains
`shares` and `with_shares`; `door_for` wires it.

Where: `share_ops.rs`, `crates/ordo-host/src/lib.rs`, `runtime.rs`,
`crates/ordo-api/src/spec.rs` and `router.rs` (`TokenGate::ShareNotLive`),
`crates/ordo-secevent/src/event.rs`, `platform_ops.rs`.

Tests: `sharing.rs::a_revoked_share_is_refused_at_the_next_call`,
`::a_guest_who_leaves_is_refused_at_the_next_call`,
`::an_unreachable_grant_registry_refuses_window_calls_and_serves_members`.
All three failed with the gate disabled.

Differences from the plan:

- **The gate is in `token_gate`, not in `check`.** `check` is synchronous and
  the registry is asynchronous; delegation had the same problem and the same
  answer. The refusal is the door's uniform answer, and it runs before the op
  registry, so a closed window tells its holder nothing an open one would not.
- **Not built, waits on `auth.rs`:** `POST /auth/share/leave` (the route), and
  that `/auth/window` refuses a revoked grant (G4). The test named in the plan
  as `..._and_no_window_can_be_reminted` was cut to
  `a_revoked_share_is_refused_at_the_next_call`; it asserts the registry no
  longer holds the grant live, which is what `/auth/window` will read.
- The blob door does not run the token gate, so a `shr_` token is not asked
  about there. Nothing serves blobs to a guest yet (L4).
- Acl rows a share wrote are taken back only for a guest seat. Until G3
  records which rows a share wrote, they cannot be told from rows a member
  holds by ordinary means.

### G6 · Listing

`std.person.tenants` answers `role: null` for a seat with no role.

Where: `platform_ops.rs` (from G1).

Test: `my_tenants.rs::a_guest_seat_is_listed_with_no_role_and_a_member_is_not_listed_as_a_guest`
(seen failing with the viewer reading restored).

Not built: `via: "guest"` on `GET /auth/spaces` (P4's route, in `auth.rs`).

A gap the plan did not name: `std.person.tenants` is allowed to four roles,
so a person who is **only** a guest (no tenant of their own) is refused the op
in the tenant they are a guest in (`NoRole`). Under `on-signup` provisioning
they have a tenant of their own and ask there. `GET /auth/spaces` is the route
that closes it for a guest-only person.

### G8 · The corpus

GC063 to GC065 run as `sharing.rs::gc063_gc064_gc065_registration`, over
`share_ops::validate`, with no database. `guards_corpus.rs` lists them in a
new `EXECUTED_ELSEWHERE` (and the coverage test checks each named test
exists), so `NOT_RUN` shrank by three. `guards-conformance.md` (R-G18, the
stage, the table, the three cases) is re-spelled. The corpus's `vendorNames`
is not in the fixture; `activeProducts` stands in.

The plan expected the basis of `openTasks` to be `tasks` alone; the pins list
the entities the walk reaches (`projectMembers`, `projects`, `tasks`), and the
test asserts `tasks` is among them.

Seen failing with the reachable-guard rule disabled.

### G9 · Documents

Done for what is built: `definition-language.md` §8 (a guest is a seat with no
role, refusal at registration, acceptance in the sharer's log and the
deployment stream), `deploying.md` (`ORDO_SHARE_MAX_SECS`), new
`howto-sharing.md` (marks what is owed), `plan-first-program.md` §5 points
here, `guards-conformance.md`, `corpus.md`.

Not done, and not mine to do: `node scripts/board-agrees.mjs`, the
checklist's 11-2 row citing this plan, and adding `howto-sharing.md` and
`plan-sharing.md` to `docs/board-docs.jsonl`. The howto's accept, window and
SDK sections wait on G3, G4 and G7.


## Built, second pass: the guest's half

Built 2026-09-29 on branch `worktree-agent-ae5e90d36e73363ea`, after G1, G2, G5, G6 and G8 above. G3, G4, G5's leave route and G6's `via: guest` are here; G7 (the SDK) follows in its own entry. The owner's answers are built as ruled: acceptance is recorded in the sharer's log and the deployment's security log, and shares are read-only.

### G3 · Accept

`POST /auth/share/peek {ticket}` (no token) and `POST /auth/share/accept {ticket, display_name}` (a person's session). The ceremony is `share_accept.rs` (new), the routes are `auth/share.rs` (new), and `ShareOps` gained one field, `accept: Option<AcceptWiring>` (the door's engine, and an `AclRunner`), so the two struct literals (`runtime.rs`, `tests/sharing.rs`) name it.

The order, which is the order of what cannot be undone: the ticket is looked at without being spent (`Want` names the guest as holder, so a ticket that names somebody else is refused and stays whole); the sharer is checked again against the program as it is now (still an active member with a role, every op still a guard-bearing read within the sharer's reach); then the ticket is spent, atomically; then the seat; then the acl rows; then the grant; then the door is announced; then the consent is recorded.

- **The seat** is a members row with a null role under the name the guest typed, minted, or reactivated if an earlier leave or revocation suspended it. A person who already holds a role gets no seat: the share adds a grant to the row they have.
- **The acl rows** run as the derived `grant<Entity>Access` through the deployment's executor with the sharer's member id as `$caller` (`ExecutorAcl`). They land in the sharer's own command chain, attributed to the sharer, and succeed only where the sharer administers the row. A refusal undoes what this accept wrote for a seat it minted and refuses the accept. A row the person already holds at any level is skipped: the grant converges on `(row, member)`, so writing `read` over a `write` would take access away.
- **The grant** is `register_share` over the pins as validated now, id `shr_` plus 128 random bits, recorded, and only then are older live share grants of that person in that tenant revoked. The new one carries the union of the ops and the later end.
- **The consent** is `Enrolment { act: "share-accepted" }` in the deployment's security stream against the guest, mirrored to the sharer's tenant stream.
- **One answer.** A ticket that is unknown, spent, expired or for somebody else, a sharer who is gone, a program that changed, and a row the sharer may not open are all `403 share_refused`. The stream keeps which.

Tests in `tests/sharing.rs`: `an_accepted_share_binds_a_seat_records_the_grant_and_writes_the_acl_rows_as_the_sharer`, `a_second_share_to_the_same_person_reuses_the_seat_and_replaces_the_grant`, `a_ticket_naming_another_person_is_not_spent_by_the_wrong_holder`, `a_member_who_is_shared_with_gets_no_second_seat`. Seen failing with the acl writes skipped, with the holder check removed, and with the skip-a-held-row rule removed.

Differences from the plan:

- **The acl rows are correlated to the share by the grant id in the record**, not by a correlation id on the write. The executor takes no correlation, and passing an idempotency lease for it would make a retry a no-op with a different meaning. The `share-accepted` event names the grant, the seat and the number of rows, and the rows themselves are in the sharer's chain under the sharer's actor.
- **The ticket is spent before the writes.** Two people racing for one ticket leave one winner. A failure after that point burns the ticket and the sharer shares again. Everything that can be checked first is, so the failure left is a database that stops answering.
- **`std.space.revokeShare` still takes back acl rows only for a guest seat.** A person who holds a role keeps the rows, because a share cannot tell the rows it wrote from the rows a member holds. This is the gap G5 named, and G3 does not close it.

### G4 · The window token

`POST /auth/window {space}` with a person's session answers a capability token for the live `shr_` grant that person holds in that tenant (`ShareOps::window`, `auth/share.rs`).

- **The pins are the consented ones.** They are decoded from the registry, one row per op, never taken from the deployment. `a_window_token_carries_the_consented_pins_so_a_widened_op_refuses_the_window` widens `boardList` after the accept (`unguarded` set); the window is refused with `ManifestChanged` in the security stream, while a token re-pinned from the widened deployment is allowed, which is the failure the registry pins prevent.
- **The end** is the earlier of the grant's and one session life (`ORDO_SESSION_SECS`). No session and no client are on the token, so logout ends it by expiry, and this route wants a session, so a logged-out person cannot mint another.
- **Refused as one answer** (`share_refused`): no live grant in that tenant, a revoked one, an expired one. An unreachable registry is `share_unavailable`.
- **A window is not a session.** `POST /auth/window`, `/auth/share/accept` and `/auth/step-up/begin` refuse a window token, and the door refuses it every platform op, whatever `ORDO_STEP_UP_EXEMPT_OPS` says, because the grant lists none.

The plan named a `live_share_of(space, principal)` query in `crates/ordo-host/src/grants.rs`. `grants_of` already answers it, so `grants.rs` and `pins.rs` are untouched.

Tests in `tests/sharing.rs`: `a_window_token_reads_exactly_the_shared_rows_and_is_refused_in_any_other_space`, `a_window_token_carries_the_consented_pins_so_a_widened_op_refuses_the_window`, `a_window_token_cannot_step_up_or_call_a_platform_op`, and for G5's `/auth/window` refusal `a_revoked_share_cannot_be_reminted_as_a_window`. The last was seen failing with the liveness filter removed. The pins test has no cheaper counterexample than the contrast inside it, which passes only because the window's pins come from the registry.

### G5 · The leave route, and G6 · `via: guest`

`POST /auth/share/leave {space}` (a person's session) calls `ShareOps::leave`, which was built with G5 and had no route. The grant is revoked, the acl rows the shares wrote for a seat with no role are taken back, the seat is suspended and the door announced, so the token the guest holds is refused at the next call and `/auth/window` refuses them. A person who holds a role keeps the membership, and only the shares close. A person with no seat there is the one `share_refused`. Both `share-left` events land, in the deployment's stream and the sharer's. With this the plan's G5 is complete: `revokeShare`, the registry gate, the unreachable-registry refusal, `/auth/window` refusing a revoked grant, and the leave route.

`GET /auth/spaces` answers `via: "guest"` and `role: null` for a seat with no role, and drops the seat once it is suspended (P4's route, one mapping). This closes the gap G6 named: a person who is only a guest can now list where they are a guest, without holding a role anywhere.

Tests in `tests/sharing.rs`: `a_guest_who_leaves_by_the_route_is_refused_at_the_next_call` and `a_guest_seat_is_listed_as_a_guest_and_a_member_is_not`. The second was seen failing with `via` always `member`.

### G7 · The SDK

`clients/ordo-sdk/src/auth/share.ts` (new), exported from `index.ts`: `peekShare(endpoint, ticket)`, `acceptShare(endpoint, token, ticket, displayName)`, `openWindow(endpoint, session, space, options?)`, `leaveShare(endpoint, token, space)`, and `listSpaces(endpoint, token)` for P4's `GET /auth/spaces`.

`openWindow` answers an `OrdoClient` bound to the shared tenant whose token is the window token. It mints the first token at once, so a closed or never-made share is refused now and not at the first read. `session` is a token or a function answering the current one. The token is minted again `refreshBeforeSecs` (default 30) before it ends, once for any number of calls at that moment. A mint refused after the sharer closed the window is thrown to the call that needed it. Every refusal of a share is one `PasskeyError('refused', 'that share cannot be used')`.

Tests in `clients/ordo-sdk/test/share.test.ts` (8): `sends the window token and the shared space header`, `asks for a new window token before the old one ends, once for concurrent calls`, `is refused when there is no window, and when the sharer closes it`, `accept posts the ticket and the name byte for byte`, `peek needs no session and says what accepting discloses`, `leave posts the tenant, and the list is the person's own`, `a refused share is one message whatever the reason`, and `a refusal after revocation purges the replica of that space only`. `npm run check`: typecheck clean, 122 tests pass. Seen failing with the accept body key changed and the refresh margin removed (three tests), and with the manager's release on a revoking refusal removed (the replica test).

**Where the plan is wrong against the code.** It says the replica is "poisoned on the first refusal after revocation (existing behaviour)". It is not. `SyncManager` releases the rows the revoked op held (`#releaseHeld`) and stops the op on `access_denied`, `access_guard` or `delegation_required`, and `LocalReplica.poison()` is a separate call that `replica.ts` leaves to the caller. The test asserts what the code does: the shared tenant's replica is purged by the refusal, an explicit `poison()` of it leaves the person's own tenant's replica alone, and the two are separate namespaces because a replica is keyed by `(space, principal)`. Its name says purges, not poisons.

`openWindow` takes an endpoint and a session, not "a client", because an `OrdoClient` keeps its base address and token private and the window needs both to mint.

`docs/howto-sharing.md` now covers accept, window, leave and the person's list, and its owed table names what is still owed: share links (L1 to L5), a guest that writes (18ab), and taking back the acl rows a share wrote for a person who holds a role.


## Built, third pass: share links

Built 2026-09-29 on branch `worktree-agent-aa20e6171cc2496f8`, after G1 to G9. The owner's answers are built as ruled: links are on by default, the lifetime is layered, and the platform sets no maximum of its own. `ORDO_SHARE_LINK_MAX_SECS` was withdrawn and does not exist.

**The lifetime, in three layers.**

| Layer | Whose | Where it lives | What it does |
|---|---|---|---|
| Deployment default | deployment owner | `ORDO_SHARE_LINK_DEFAULT_SECS`, else `BlobPolicy::share_ttl_default` (7 days) | the lifetime of a link that states none, in a tenant that chose none. Never a cap |
| Tenant choice | tenant owner, when the app developer sets `ORDO_SHARE_LINK_TENANT_LIFETIME=on` | `$admissionpolicy.share_link_secs`, through `std.space.setShareLinkLifetime` | replaces the default for the tenant's links and is the longest one, because the tenant chose it |
| The link | the sharer | `$expires` on `std.space.shareLink` | its own lifetime, within what its tenant allows |

The tenant's choice is stored the way E4's admission policy stores its values, as one nullable column on the same per-tenant row, written without touching the other columns. **It should move to checklist 24a's general tenant settings store when that exists.** A stored choice is not applied while the app developer has not exposed the layer.

### L1 · The mint

`std.space.shareLink($ops, $rows, $expires?, $label?)`, manager and owner. It runs `share_ops::validate`, so a link lists only deployed reads that are not `unguarded`, have a reachable guard, are not platform ops and are within the sharer's reach. A seat with no role reaches nothing, so a guest or a link cannot mint. Then, in the order of what is easiest to undo: the link seat (a members row, principal `lnk_<random>`, no role, named by the label), the acl rows as the sharer, a `shr_` grant for the link's principal, and a `$sharelink` row in the control database (hash of the secret, tenant, grant, seat, ends, opens). A failure after the seat exists takes back the seat and the acl rows. The secret is 256 random bits with the prefix `lks_`, answered once, hashed with its own domain (`ordo.sharelink.v1`, not the ticket's), and in no chain, stream or listing.

Where: new `crates/ordo-host/src/share_links.rs`; `platform_ops.rs` (`links`, `with_links`, the arm, the owner-only name, the specs chain), `runtime.rs` (`platform_specs`, wiring, `ensure_table`), `lib.rs`, `share_ops.rs` (`close`, `seat_of` and `Closed` made `pub(crate)` for L3), `docs/deploying.md`, `docs/settings.md`.

Tests in `tests/share_links.rs`: `a_link_is_minted_once_logged_and_stored_only_as_a_hash`, `a_link_over_a_write_op_or_a_platform_op_is_refused`, `links_off_refuses_the_mint`, `the_deployment_default_is_a_default_and_never_a_cap`, `a_tenant_choice_sets_and_bounds_its_links_only_when_the_app_developer_exposes_it`. Each was seen failing with its change undone: the secret stored in the row, the acl rows and the seat left behind after a refused row (two separate mutations), the off switch ignored, the default used as a cap, and the tenant's bound removed.

Differences from the plan:

- **`$expires` is optional.** The plan's `ORDO_SHARE_LINK_MAX_SECS` and its 30-day hard cap are gone. With no `$expires` a link gets the tenant's choice, else the deployment default.
- **The test named `..._bounded_by_the_ceiling` is replaced** by the two lifetime tests, because there is no ceiling to bound by.
- **The ticket cap does not apply.** A link is not a ticket, so the 30-day ticket limit does not bound it.
- **No sweep.** The plan asked for a sweep of the table on the tickets' pattern. Expired and revoked rows are kept, and `std.space.links` shows them with their state. Nothing has deployed, so nothing yet grows without bound. A sweep is filed for when it does.
- **Link ops are one module, not folded into `share_ops.rs`**, so the file that other work edits stays as it was.

### L2 · Open

`POST /auth/link {link}` with no token (`auth/link.rs`, new; routed in `auth.rs` and `serve.rs`, with `AuthState::wire_links`). The hash is looked up in `$sharelink`; an unknown, malformed, expired or revoked link, a grant that is no longer live, and a deployment with links off are one answer, `403 link_refused`, and the stream keeps which. The answer is a capability token for the link seat: the principal is `lnk_<id>`, the pins are the consented ones decoded from the registry (`ShareOps::window`, shared with G4), one tenant, no session, no client, no presence, ending at the earlier of the link's end and one session life. `opens` is incremented and `Enrolment { act: "link-opened" }` is recorded with the address, in the deployment's stream and the sharer's tenant's.

A miss counts against the caller's address, kept in memory (a restart forgives, which suits a per-minute limit). An address with 5 misses in 60 seconds is answered `429 quota_exceeded` before anything is looked up, whatever it presents, so a right guess after five wrong ones is no better than a wrong one. The counter follows at most 10,000 addresses and drops the oldest past that. Behind a proxy the address is the proxy's until a trusted-proxy header is built (the same limit P3 has); a request with no known address shares one bucket.

Tests in `tests/share_links.rs`: `an_opened_link_reads_exactly_the_shared_rows_and_nothing_else`, `an_unknown_expired_or_revoked_link_is_the_one_answer`, `a_link_token_cannot_write_step_up_or_call_a_platform_op`, `the_sixth_miss_from_one_address_in_a_minute_is_refused`. Seen failing with the open count not incremented, the revoked check removed, the expiry check removed, misses not counted, presence put on the token, and the token's principal changed.

Differences from the plan:

- **The token test reads the claims.** Refusing a platform op at the door also holds without the change, because a capability token never satisfies step-up, so a refusal alone would not fail. The test asserts what the token is (principal, no session, no client, no presence, no `std.` op in the grant), which the route's mutations do fail.
- **Missing secret is not a miss.** A body with no `link` is answered `link_refused` and does not count, because it cannot be a guess.

### L3 · Revoke and list

`std.space.revokeLink($link)`, manager and owner: `$link` is the link's id (`lnk_...`), never its secret. It reuses G5's closing (`ShareOps::close`): the seat's share grants are revoked in the registry, so the door refuses the next call through G5's gate; the acl rows the seat holds are taken back; the seat is suspended and the door announced. Then `revoked_at` is set. An id that is not this tenant's link is one `not_found`. The act is in the tenant's stream only, because a link has no person to record it against.

`std.space.links` lists the tenant's links, newest first: id, label, ops, rows, opens, ends, `state` (`live`, `expired` or `revoked`) and who issued it. It never returns the secret or its hash, and never another tenant's rows.

Tests in `tests/share_links.rs`: `a_revoked_link_is_refused_at_the_next_call`, `the_list_shows_opens_and_state_and_never_the_secret`. Seen failing with the seat not closed as a guest, `revoked_at` not set, the hash listed in place of the label, and the list not scoped to the tenant. A viewer is refused the revoke at the door and the manager is not.

### L4 · Files

A link that lists any `std.<entity>.get` serves `GET /blob/{ref}` for the file of a shared row, and refuses the file of another row, an upload, and everything once it is revoked.

Two things the plan did not say, and one it had wrong:

- **The blob door checks the pseudo-op `std.$blob.get`, not `std.<entity>.get`.** `blob_gate` cannot name the entity before the handle is resolved, and resolving it is a read, so the door check runs against the reserved `std.$blob.get` and the row guard inside `PlatformBlobs::read` is the real authorisation. A grant that lists only `std.<entity>.get` therefore reached the door and was refused there. The mint now adds the pin of `std.$blob.get` to the grant when any listed op is a `std.<entity>.get`, provided the sharer reaches it. `validate` cannot be asked to accept `std.$blob.get` itself, because it has no entity and so no reachable guard. What separates one row's file from another's is the row guard over the link seat's acl rows, which is the guard that hides the row.
- **The blob door did not run the token gate**, as the G5 note said. `blob_gate` in `crates/ordo-api/src/router.rs` (now `async`) asks `token_gate` first, exactly as `dispatch` does, for both `GET` and `PUT`, and answers a share grant that is no longer live with the door's uniform `access_denied`. The delegation refusal is the same as at `dispatch`. This closes it for every `shr_` token, guests included, and for delegations, which had the same gap.
- **The revoke already made the file door refuse**, because it takes back the acl rows and the row guard then hides the file. The gate matters when the rows survive: `revoke` closes the grant first and the rows second, so a stop between them leaves rows behind. The test puts a row back to stand for that, and it fails with the gate removed.

Sealed bytes are opened by the host under the sharer's key, one key release per read, by the existing path (`PlatformBlobs::read`, `keys.release(space, Read, correlation)`), whose correlation names the principal the door resolved (the link seat). `blob sealed` and `blob private` fields are still refused by that route (`private` waits on F7 Phase 6), so a link cannot reach them.

Test in `tests/share_links.rs`: `a_link_serves_the_file_of_a_shared_row_and_not_another`. It uploads two files as the sharer, opens a link over one row, reads that file byte for byte, is refused the other, is refused an upload, and after the revoke is refused with the acl row restored. Seen failing with the token gate removed from the blob door, and with the `std.$blob.get` pin left out of the grant.

### L5 · The SDK

`openLink(endpoint, link, options?)` in `clients/ordo-sdk/src/auth/share.ts`, exported from `index.ts` with `LinkOptions`. It posts the secret to `POST /auth/link` with no `Authorization` and answers an `OrdoClient` bound to the tenant the door names (the holder need not know it). It opens the link at once, so an unknown, expired or revoked link is refused now, and opens it again `refreshBeforeSecs` (default 30) before the token ends, once for any number of calls at that moment. A refusal, and a 429 from an address that has missed too often, are one `PasskeyError('refused', 'that link cannot be used')`. The secret is sent to the one route and never on a read.

Tests in `clients/ordo-sdk/test/share.test.ts` (2): `open_link_posts_the_secret_and_binds_the_answered_space` (the plan's name) and `opens the link again before the token ends, and says one thing when it is refused`. `npm run check`: typecheck clean, 136 tests pass. Seen failing with the body key changed, the refresh margin removed and the refusal message left as the door's.

`docs/howto-sharing.md` now has the app developer's page for links: making one, the three lifetime layers, opening one from the URL fragment, revoking and listing, and what a link can never do.

**Where the plan is wrong against the code.**

- It gave `ORDO_SHARE_LINK_MAX_SECS`, a 30-day hard cap and a test `..._bounded_by_the_ceiling`. The owner withdrew the cap; see the layers above.
- It said the blob door checks `std.<entity>.get` (L4). It checks `std.$blob.get` (see L4).
- It said the blob door "is shared" so L4 would be "assert only". The door did not run the token gate and needed the change above.
- It said the mint "answers the secret once" and that a `$sharelink` sweep follows the tickets' pattern. No sweep is built (see L1).
- It said the security event for an open is `Enrolment { act: "link-opened" }` "with the address". It is, and it also lands in the sharer's tenant's stream, like the guest's consent.
- Section 4.2 says `std.space.links` is "Manager and owner", and `callersOf` "already lists the link seat under the token via". Both hold. The plan's status line and section 0 still say "planned, not built" and were not edited here.

**Hardcoded values introduced** (for checklist 24c): the miss limit of 5 and its 60-second window (`MISS_LIMIT`, `MISS_WINDOW_SECS`), the 10,000 addresses the miss counter follows, the 120-character label limit (`MAX_LABEL`), the `lnk_` and `lks_` prefixes and the 256-bit secret, and the SDK's 30-second refresh margin. The 7-day default is not new: it is `BlobPolicy::share_ttl_default`, now read through `ORDO_SHARE_LINK_DEFAULT_SECS`.
