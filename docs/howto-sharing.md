# Sharing with a guest

A tenant can let a person from outside see some of its rows, read-only, for a
stated time, and take it back at any moment. That person is a **guest**. This
guide is for an app developer and says what works today, what is owed, and
what a guest sees. Share links, for readers with no account, are near the end. The design and its reasons are in `docs/plan-sharing.md`.

---

## What a guest is

A guest holds a **seat** in your tenant: a `members` row with no role. The
seat carries the guest's deployment principal, so the same person is a guest
in your tenant and whatever they are in others. Nothing in your program
changes and nothing is added to the language.

A seat with no role reaches nothing by role. A session token for a guest is
refused at the door with `access_denied`; the record says `NoRole`. What a
guest reaches comes from a **share grant** and from nothing else: the ops
the sharer listed, each pinned to what it meant when the share was made, over
the rows the guest's acl rows admit.

`std.person.tenants` lists a guest seat with `role: null`. A member is listed
with the word for their role. `GET /auth/spaces` is the same list for a page
to call after sign-in, with `via: "guest"` for a seat with no role and
`via: "member"` for one with a role, and it stops listing a seat once it is
suspended.

## Sharing, as the sharer

The sharer is a manager or an owner of the tenant. They call
`std.space.share` in their own tenant.

| Parameter | What it is |
|---|---|
| `ops` | comma-separated op names the guest may call |
| `rows` | a JSON array of `{entity, id, level}`; `level` is `read` |
| `expires` | how long the window lasts, from now: `7d`, `30d` |
| `to` | optional: the principal of a person who already holds a seat here |
| `label` | optional: a name the guest is shown |

The answer is a **ticket** and when it stops working. Hand the ticket to the
person. It is a bearer secret, so send it the way you would send a password
reset, and it lives 30 days at most.

Every listed op must pass these rules, and the first that fails is the
answer:

1. It is not a platform op. A share is a window, never administration.
2. It is an op of your program.
3. It is a read. A share is read-only until guests may write.
4. It is not marked `unguarded`.
5. It touches at least one entity with a read guard, or restricts rows to
   `$caller`. Otherwise a guest would see every row the op can reach
   (`guest_exposure_unguarded`).
6. The sharer can call it themselves. A share carries only the sharer's
   own authority.

A refused share makes no ticket. A share names only rows of entities its ops
touch.

**Lifetime.** The sharer states it. The platform sets no ceiling; a deployment owner may set one with `ORDO_SHARE_MAX_SECS` (unset by default). The window closes at the instant `expires` states, whenever the guest
accepts.

**Presence.** `std.space.share` asks for a fresh passkey assertion like
every platform op, unless you exempt it with `ORDO_STEP_UP_EXEMPT_OPS`.

## Taking it back

`std.space.shares` lists what is live (owner ruling, 2026-10-05): one row per
seat that holds a live share, with `seat`, the `displayName` the guest
consented to show, `guest` (false for a member who was also shared with),
`ops`, `grantedAt` and `expiresAt`. It never names the principal. The manager
and the owner may call it, as they may call `revokeShare`.

`std.space.revokeShare` with `seat` (the guest's member id) closes the
window. In this order:

1. The guest's share grants are revoked in the grant registry. The door asks
   the registry about a share grant on **every call**, so the next call is
   refused whatever the token's own expiry says.
2. For a guest seat, the acl rows the shared ops' entities hold for that seat
   are deleted.
3. A seat with no role is suspended and the door is rebuilt on every replica.

A member who holds a role keeps the membership and their own acl rows; only
the share grant is closed.

If the grant registry cannot be reached, calls that present a share grant are
refused and nothing else is: members and ordinary capability grants are
served as before.

A guest can also leave (`POST /auth/share/leave`, below). That closes the same
window from their side.

Revoking stops access. It does not undo what a guest already read.

## Accepting, as the guest

The guest opens your accept page with the ticket in the URL fragment. The page
calls the SDK, and the SDK calls the door:

| Step | SDK | Route | Token |
|---|---|---|---|
| 1. Show what is offered | `peekShare(endpoint, ticket)` | `POST /auth/share/peek` | none |
| 2. Sign in, or sign up | `signInWithPasskey`, `registerPasskey` | `/auth/...` | |
| 3. Accept | `acceptShare(endpoint, token, ticket, displayName)` | `POST /auth/share/accept` | the guest's session |
| 4. Read | `openWindow(endpoint, session, space)`, then any generated op on the client it answers | `POST /auth/window`, then `X-Ordo-Space` | the window token |
| 5. Leave | `leaveShare(endpoint, token, space)` | `POST /auth/share/leave` | the guest's session |
| Or decline | `declineShare(endpoint, ticket, token?)` | `POST /auth/share/decline` | none, or the named person's session |

**Decline** refuses a ticket before it is accepted (owner ruling, 2026-10-05).
It spends the ticket, so nobody can accept it afterwards, and no seat is made.
The decline is written to the deployment's security log and to the sharer's
tenant log as `share-declined`. A ticket that names nobody may be declined by
whoever holds it, because they could accept it. A ticket that names a person
needs that person's session; anyone else is refused and the ticket stays whole.

An invitation to a tenant can be declined the same way:
`peekInvitation(endpoint, ticket)` (`POST /auth/invitation/peek`) shows the
tenant, the role and the expiry without spending it, and
`declineInvitation(endpoint, ticket, token?)` (`POST /auth/invitation/decline`)
spends it. The tenant's `std.space.invitations` then lists it with
`status: "declined"` and `declinedAt` until the instant it would have expired,
and the tenant log carries `invitation-declined` under the same id.

**Peek** answers the tenant's name, what the sharer called the share, the ops,
the rows, when the window ends, and two sentences to show the guest: what
accepting discloses (the name they type is shown to the sharer) and that the
tenant's owner can reach anything in it and every time they do it is logged.
It does not spend the ticket. It says nothing about who issued the ticket or
who it names beyond whether it names somebody.

**Accept** needs a person's session and a display name. In this order: the
ticket is looked at without being spent; the sharer is checked again against
the program as it is now; the ticket is spent; the guest's seat is made (or
the seat they already hold is reused, or reactivated if an earlier leave
suspended it; a person who already holds a role gets no seat); the acl rows
are written **as the sharer**, so they land in the sharer's own chain under
the sharer's guards; the grant is recorded; the tenant is announced to every
replica; and the consent is recorded in the deployment's security log and in
the sharer's tenant log.

A second share to the same person reuses the seat and replaces the grant with
one for both shares' ops and the later end. A row the person already holds at
any level is left alone, so a `read` share never takes `write` away.

A ticket that names a person is that person's alone. Somebody else presenting
it is refused, and the ticket is not spent.

**The window.** `openWindow` answers an `OrdoClient` bound to the shared
tenant. Its token is a capability token for the guest's live share grant,
carrying the pins the guest consented to (not the deployment's pins today), so
an op that was widened after the accept refuses the window until the sharer
shares again. It ends at the earlier of the grant's end and one session life
and is minted again a little before it ends. It cannot step up, cannot reach a
platform op, and cannot open a window or accept a share of its own.

**One answer.** An unknown, spent, expired or mis-addressed ticket, a sharer
who is gone, a program that changed, a row the sharer may not open, a window
that was closed and a window that never existed are all `share_refused` (403).
The security log says which.

**On the device.** A replica of the shared tenant is per tenant and per person,
so the guest's replica of their own tenant is untouched. When the sharer closes
the window, every feed open on it is told at once, as it is when one note is
taken back. The feed refetches, is refused `access_denied`, and the sync
manager releases what that op synced. It does not wait for a reconnect.

A feed held on the window's own token ends as well, even when its holder is
also a member who could read the rows another way. Every long-lived read asks
the whole door again (the share gate and the access check) before every pass,
and on a quiet feed every `ORDO_LIVE_RECHECK_SECS` (30 s by default). A
refusal is the feed's last frame, `event: error`, and the feed closes. A
logout or a removed member ends a feed the same way.

## Provisioning and sharing

Under `ORDO_PROVISION_SELF_SERVE=on-signup` a stranger who accepts a share
signs up first and is given a tenant of their own in the same request. Their
`GET /auth/spaces` then lists their own tenant (`via: "member"`) and the
sharer's (`via: "guest"`).

## Share links: a person with no account

A **share link** lets anyone who holds a secret read some rows, read-only, for
a stated time, with no sign-in. It is on by default. Use it for "let them look
at this" where the reader has no account and you do not want them to make one.
A person who should be recognised, or who may later write, is a guest, above.

A link is a bearer secret: whoever has it reads. So it always expires, you can
revoke it at any moment, and what it may list is checked when you make it.

### What a link is

Making a link makes a **link seat** in your tenant: a `members` row with no
role whose principal is `lnk_...`, a principal that is in no directory and can
never sign in. The sharer's acl rows for the shared rows are written for that
seat, as the sharer, in the sharer's own chain. A share grant (`shr_...`)
lists the ops. A link is exactly a window token whose holder nobody owns.

### Making one

A manager or an owner calls `std.space.shareLink` in their own tenant.

| Parameter | |
|---|---|
| `ops` | comma-separated op names |
| `rows` | a JSON array of `{entity, id, level}`; `level` is `read` |
| `expires` | optional, a duration like `7d`. See the lifetime below |
| `label` | optional, names the link in your list; shown as the seat's name |
| `password` | optional, up to 256 characters, at least the tenant's `minLength` (4 unless set). Whoever opens the link must give it. See below |
| `maxOpens` | optional, a whole number from 1, as text. After that many opens the link is used up. See below |

The answer holds `link` (the secret, shown **once**), `id`, `seat`,
`expiresAt`, `ops`, `hasPassword` and `maxOpens`. Store the secret where you will hand it out; the
platform keeps only its hash, and it is in no log, chain or list.

The op list must pass the same checks as a guest share. A link refuses a
platform op, a write, an `unguarded` op, an op with no reachable guard, an op
the sharer cannot call, and a row the sharer does not administer. A refused mint writes nothing. A guest or another link
cannot mint, because a seat with no role reaches nothing.

**Files.** If an op is `std.<entity>.get`, the grant also carries the blob
door's own check, so the holder reads the file of a shared row through
`GET /blob/{ref}` and the file of no other row. A revoked link stops serving
files at the next call, like ops.

### The lifetime

The platform sets no maximum of its own. It has three levels.

| Level | Whose | Set by |
|---|---|---|
| Deployment default | deployment owner | `ORDO_SHARE_LINK_DEFAULT_SECS`, 7 days when unset. A default, never a cap |
| The tenant's choice | tenant owner | `std.space.setShareLinkLifetime`, only where you (the app developer) set `ORDO_SHARE_LINK_TENANT_LIFETIME=on` |
| The link | the sharer | `expires` on the mint |

With no `expires`, a link gets its tenant's choice, else the deployment
default. A tenant that chose a lifetime cannot mint a link longer than it. A
tenant that chose nothing has no bound, so `expires: "5y"` is a five-year link
if you want one. `std.space.setShareLinkLifetime` takes a duration or
`default`, and refuses when you have not exposed it. To let tenants choose,
expose the op in your app and set the variable; to keep it yours, leave it off.

### A limit on opens

A link can allow only so many opens (owner, 2026-10-22: "after n accesses it
is unavailable"). Set `maxOpens` at the mint. Change it later with
`std.space.setLinkMaxOpens` (`link` is the link's `id`, `maxOpens` is a number
or `none`). A limit below the opens used makes the link used up at once.

- **What counts as an open.** The first successful open by a device. A
  renewal of the token is not an open. A reload on the same device is not an
  open. A wrong password is not an open. The SDK sends a random `device` id
  with each open and keeps it in `localStorage`. The door stores only a salted
  hash of it, per link.
- **Why.** The page renews its token about every session life. Counting each
  renewal would use up a 1-open link while one reader is still reading it. A
  device that already opened the link keeps renewing after the limit is
  reached. A client that sends no `device` cannot be told from a new one, so
  each of its opens counts.
- **Used up.** A new device is refused like an expired link:
  `403 link_refused`, one message, so the holder learns nothing. The
  tenant's security log keeps the reason, `link-used-up`.
- **Atomic.** A place is taken with one guarded update on the link row, so
  two opens at once cannot both take the last one.
- **What the sharer sees.** `std.space.links` shows `opensUsed`, `maxOpens`
  and `state: "used-up"`. `opens` stays the count of every successful open,
  renewals included.

**Expiry** is the other end of a link. It exists already: every link ends,
by `expires` on the mint, the tenant's choice or the deployment default (see
the lifetime above). Past it the state is `expired`.

### A password

A link can require a password (owner, 2026-10-22). Whoever opens it must
give the password before any grant is issued. Send it to the reader by a
different way than the link itself.

- Set it at the mint with `password`, or later with
  `std.space.setLinkPassword` (`link` is the link's `id`). Leave
  `password` out to clear it. Either call resets the wrong-try count and
  any lock.
- Only a slow, salted argon2id hash is stored. The password is in no answer,
  log, chain or list. `std.space.links` says only `hasPassword`.
- Wrong tries are counted per link. The lock is **off by default** (owner,
  2026-10-22). A wrong password is refused, one try at a time, and the link
  never locks. When a layer sets `tries` to a number, the link locks for a
  time at that many wrong tries, even for the right password. The right
  password resets the count. The lock is recorded in the tenant's security
  log as `link-locked`.
- Wrong tries are also **slowed, always** (owner, 2026-10-25). After 5
  wrong in a row, each further try waits 1 s, then 2 s, 4 s, and so on, up
  to 60 s. A try inside the wait is refused unchecked, `429
  too_many_attempts` with `Retry-After`. The right password resets it. It
  never locks: the reader waits at most a minute. The count is per link,
  kept in the control database, so every node shares it. Each burst is one
  `Throttled` entry in the tenant's security log and the deployment's.
- A password is **at least 8 characters** (owner, 2026-10-25). Only a
  password being set is measured: a link given a shorter one before keeps
  it, and it still opens the link.

Five values are layered the way session settings are (owner: "It is
their space"): `tries` (wrong tries before the lock, `off` by default),
`lockSecs` (how long it lasts), `minLength` (the shortest password, 8),
`slowAfter` (wrong tries before the slowdown, 5) and `slowCapSecs` (its
longest wait, 60).

| Level | Whose | Set by |
|---|---|---|
| Deployment | deployment owner | defaults `ORDO_SHARE_LINK_PASSWORD_TRIES` (`off`), `_LOCK_SECS` (900), `_MIN_LENGTH` (8), `_SLOW_AFTER` (5), `_SLOW_CAP_SECS` (60). Absolute limits `_TRIES_LIMIT` (most tries, 100), `_LOCK_SECS_LIMIT` (shortest lock, 60), `_MIN_LENGTH_LIMIT` (shortest minimum, 8), `_SLOW_AFTER_LIMIT` (most tries before slowing, 100), `_SLOW_CAP_SECS_LIMIT` (shortest longest wait, 1) |
| App developer | you, in the deployment's own space | `std.deployment.setLinkPasswordLimits`: `tries`, `lockSecs`, `minLength`, `slowAfter`, `slowCapSecs`, and optionally a range for tenants, such as `triesRange=2-10` |
| Tenant | tenant owner | `std.space.setLinkPasswordLimits`: the same five, looser or tighter |

You may turn the slowdown off (`slowAfter=off`). A tenant may turn it off
only where your `slowAfterRange` starts at 0, such as `slowAfterRange=0-10`;
otherwise the refusal names that range. Shorter passwords than 8 need the
deployment owner to lower `ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH_LIMIT`.

The value in force is the tenant's, else yours, else the deployment's. A
tenant chooses inside your range, or inside the deployment's limits when you
set none. A value outside is refused, `link_password_limit_out_of_range`,
naming the range and who sets it. `default` clears a value and `none` a
range. `tries` takes a number or `off` (stored and answered as 0). A range
that starts above 0, such as `triesRange=2-10`, stops tenants turning the
lock off. Each setter answers the values in force, where each came from, and
the range a tenant may choose from.

To stop tenants changing their own, close the op in the access template:
`ORDO_ACCESS_TEMPLATE="std.space.setLinkPasswordLimits="`. The owner is then
told which line opens it, as with `std.space.setSessionPolicy`.

### Opening one, as the reader

`POST /auth/link {"link": "<secret>"}`, no `Authorization`. It answers a
capability token for the link seat, `space`, `expires_at` and the `ops` the
link lists. The token ends at the earlier of the link's end and one session
life, so a page that stays open opens the link again a little before it ends.
The SDK does that:

```ts
import { openLink } from '@hominemai/ordo-sdk';

// The secret is in the URL fragment the sharer sent. Read it there, never in a
// query string, so it does not reach a server log or a Referer header.
const link = new URLSearchParams(location.hash.slice(1)).get('link')!;
const client = await openLink({ baseUrl: 'https://api.example.com' }, link);
const boards = await boardList(client).fetch();
```

`openLink` opens the link at once, so a link that is unknown, expired or
revoked is refused now. The client is bound to the tenant the door names, so
the page does not need to know it. A refusal is one message,
`that link cannot be used`, whatever the reason.

A link with a password is the exception, because the reader holds the
secret and is owed the reason. Pass `password`, and handle the four typed
failures:

```ts
try {
  client = await openLink(endpoint, link, { password });
} catch (e) {
  if (e instanceof PasskeyError && e.failure === 'link_password_required') askForPassword();
  else if (e instanceof PasskeyError && e.failure === 'link_password_wrong') askAgain();
  else if (e instanceof PasskeyError && e.failure === 'too_many_attempts') waitFor(e.retryAfter);
  else if (e instanceof PasskeyError && e.failure === 'link_locked') waitFor(e.retryAfter);
  else throw e;
}
```

A used-up link is the one refusal, `that link cannot be used`, so the reader
is not told why. The SDK sends a `device` id (option `device`, else a stored
random one) so a renewal is not counted again.

The SDK sends the password again each time it opens the link to renew the
token. `@hominemai/ordo-react` has no link hook, so the page renders its own
prompt from these failures.

**What the door does.** Unknown, malformed, expired, revoked, a grant that is no longer live,
and a deployment with links off are one answer, `403 link_refused`. There
is no limit on wrong guesses: a link secret is 256 random bits, and Ordo
decides nothing on a client's address, which behind a proxy is the proxy's.
Every open is recorded in the deployment's security log and in the tenant's.

Only after the secret matched a live link, a link with a password answers
one of three more, each retryable:

| Answer | When |
|---|---|
| `401 link_password_required` | no `password` in the body |
| `401 link_password_wrong` | the password is not the link's |
| `429 too_many_attempts` | the slowdown: wrong tries in a row on this link. `Retry-After` and `retry_after` give the seconds, `setting` names `std.space.setLinkPasswordLimits`. Not a lock |
| `429 link_locked` | too many wrong tries, where a layer turned the lock on. `retry_after` gives the seconds left, `setting` names `std.space.setLinkPasswordLimits` |

They tell the reader nothing the secret did not already tell them.

### Revoking and listing

`std.space.revokeLink` with `link` (the link's `id`, not its secret) revokes
the share grant, takes back the acl rows, suspends the seat and marks the
link revoked. The **next** call with a token from that link is refused, and
the link opens no more. `std.space.links` lists the tenant's links with label,
ops, rows, opens, ends, state (`live`, `expired`, `revoked`, `used-up`),
`hasPassword`, `lockedUntil`, `opensUsed` and `maxOpens`, newest first. It never shows the secret, the password or
its hash.

### What a link can never do

- Write, or call a platform op, whatever `ORDO_STEP_UP_EXEMPT_OPS` says.
- Act as the sharer or as anyone: every read is attributed to `lnk_...`.
- Reach an unguarded op, another row, or another tenant.
- Share onward, or open a window: a link token is not a session.
- Open `blob private` content. That waits on F7 Phase 6.

## What is owed

| Owed | Where |
|---|---|
| `blob private` content in a link or a share | F7 Phase 6 |
| Removing expired and revoked link rows | filed; nothing has deployed, so nothing grows yet |
| A guest that writes | after the first deployment (18ab) |
| Taking back the acl rows a share wrote for a person who holds a role | a share does not record which rows it wrote |
