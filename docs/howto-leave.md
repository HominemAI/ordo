# How a member leaves a tenant, and how a person deletes their account

For app developers. Built in checklist 27aq. The design is
`plan-member-release.md`; the rulings are in `corpus-rulings.md`, 2026-10-05 and 2026-10-06.
The 2026-10-06 ruling changed what a leaver's seat reads and what a deleted
account leaves behind.

## With no release line, nobody can leave

If your program has no `release member` line, **members are locked**. A
member who calls `std.space.leave` is refused with `release_locked`. Only the
tenant can take them out, with `std.space.remove`.

`ordo check` says so under `RELEASE`:

```
RELEASE
  no release rule: members cannot leave on their own (locked); see docs/howto-leave.md
```

If your users cannot leave, this is why.

## The release line

One per program. Three forms:

```
release member open
release member locked
release member when not invoices(owner == $member and status == "open")
```

- `open`: any member may leave.
- `locked`: nobody leaves on their own. The same as no line.
- `when …`: a member may leave once the conditions hold.

`$member` is the seat that is leaving. Each condition is `[not] entity(walk)`,
where the walk is a guard walk. Join conditions with `and` or `or`, and use
parentheses when you mix them. A rule longer than one line goes inside
parentheses:

```
release member when (not invoices(owner == $member and status == "open")
  and not projects(lead == $member))
```

The rule is yours. Ordo builds no override past it. If you want a tenant to
have a say, write it into the rule, for example a condition over a row the
tenant controls.

## What happens to rows that point at the person

By default they stay. They keep pointing at the seat, and the seat keeps its
name and reads `status = left`. Nothing the person wrote is deleted.

You may say otherwise, in a block after the rule:

```
release member open {
  remove drafts.author
  reassign tasks.assignee to people(member == $member).manager or owner
}
```

- `remove e.f` deletes the rows of `e` whose `f` is the seat, with the rows
  they own, as an ordinary delete would.
- `reassign e.f to …` moves `f` to the first target that names an active
  member other than the leaver:
  - `owner`: the tenant's longest-standing other owner.
  - `entity(walk).field`: a member your own rows name, such as a manager or a
    delegate. If the walk finds no one, or more than one, the next target is
    tried.
- If no target names anyone and rows still point at the seat, the leave is
  refused with `release_held`. Nothing is written.
- Every target is found before any effect runs, so a `remove` cannot change
  who a later `reassign` names.

Each effect names a plaintext ref to `members`. Refs you do not name are kept.

## How a left seat reads

By default a left seat keeps the name it had. Your app decides how to show it.

To replace the name on leave, set `ORDO_MEMBER_LEFT_NAME` (app developer):

```
ORDO_MEMBER_LEFT_NAME="former member"
```

- Unset or blank: the name is kept. This is the default.
- Set: its text replaces `displayName` on leave.
- Over 256 characters: the boot is refused.
- History keeps the name either way.

To show "Bram (Deactivated)", read the seat's `status` in program code:

```
op notesWithAuthorStatus unbounded =
  notes[body, author.displayName, author.status]
  sort author.displayName
```

Your app then renders the label when `status` is `left` or `deleted`.

The left seat's `principal` reads `left:<principal>:<seat>`. It is unlinked on
purpose, so the person can be admitted again. The id after `left:` lets their
left seats be found when they delete their account.

## What a leave does

`std.space.leave` takes no parameter: the seat is the caller's. Every role may
call it. It asks for a fresh passkey step-up, like the other platform ops.

In order, in one transaction:

1. **The platform's own conditions.** The last active owner cannot leave,
   and nor can the owner of a tenant that is their own (`last_owner`).
2. **Your rule.** `release_locked`, or `release_held` naming each condition
   that holds the seat, as you wrote it. It never names a row.
3. **Your effects**, then the platform's: the seat's acl rows and group
   memberships go.
4. **The seat** stays, with `status = left`, no role and no link to the
   person. It keeps its `displayName`, unless you set `ORDO_MEMBER_LEFT_NAME`.

Steps 3 and 4 are one command group in the tenant's history, attributed to
the person leaving. Each row is imaged before and after, `rev` moves, and
triggers and live subscriptions see it as they see any write. Reverting that
group undoes the leave's rows and puts the seat back. The reverting person
must be allowed to write every row the group touched.

Your entity guards are not checked against the person leaving. Your release
rule is what allows these effects. Structure still holds: a row that other
rows still point at is not deleted, and the leave is refused with
`release_held`.

After that, the person's share grants in the tenant are revoked, a person the
tenant made becomes independent, the act is recorded in the tenant's stream,
and every replica refuses the seat on its next request. A revert does not
undo these.

Refusals are `tenant_refused`, and the class is the first word of the message.
A second leave from the same seat answers the same.

A guest seat (no role) leaves with `POST /auth/share/leave`.

## Deleting an account

`POST /auth/account/delete` with a session's bearer token, a fresh passkey
step-up, and the principal id typed back:

```
POST /auth/account/delete
{"confirm": "usr_…"}
```

**While any tenant holds the person, it is refused**, with one row per
tenant:

```
409
{
  "error": {"code": "memberships_remain", "message": "…"},
  "tenants": [
    {"space": "spc_…", "name": "Acme", "role": "editor", "status": "active",
     "bar": "held", "held": ["not invoices(owner == $member and status == \"open\")"]},
    {"space": "spc_…", "name": "Beta", "role": "viewer", "status": "active",
     "bar": "open"}
  ]
}
```

| `bar` | What stands in the way |
|---|---|
| `open` | nothing: call `std.space.leave` there first |
| `held` | the rule's conditions, listed in `held` |
| `locked` | no release line, or `locked`: the tenant must remove them |
| `last_owner` | they are the last active owner |
| `own` | the tenant is their own: delete it or hand it on |
| `guest` | a guest seat: `POST /auth/share/leave` |

Ordo produces no client app, so this body is the list your app shows.

When nothing holds them, the account is marked deleted and kept. In order:

1. Every session ends.
2. The identity is marked deleted. Sign-in, step-up and recovery all refuse
   it.
3. The sign-in links stay. Its email, password identifier and passkey cannot
   be used by anyone else until removal.
4. `identity-deleted` is recorded on the deployment stream.
5. The person's `left` seats in every tenant get `status = deleted`. The name
   stays.
6. The data stays, unless `ORDO_ACCOUNT_REMOVAL` says otherwise.

A refused sign-in looks like any other refused sign-in to the caller. The
stream reason is `identity-deleted`.

The answer is a receipt:

```
200
{"deleted": "usr_…", "at": 1760000000,
 "removal": {"setting": "never", "removeAt": null, "removed": false},
 "seats": {"marked": 1, "unreached": []},
 "receipt": {"stream": "deployment", "act": "identity-deleted",
             "reason": "self", "correlation": "…"}}
```

- `setting` is `never`, `now` or `after`.
- `removeAt` is the due time in seconds. For `now` it is the request time.
- `seats.unreached` lists each tenant that could not be reached.

### When the account is removed

The app developer sets `ORDO_ACCOUNT_REMOVAL`:

| Value | What happens |
|---|---|
| `never` | The account is kept. This is the default. |
| `now` | It is removed in the same request. |
| `<n>d` or `<n>` | It is removed after n days. |

- Anything else refuses the boot.
- The `accounts.removal` worker runs the timed removals. It runs under `now` and `<n>d`, on the
  leader only, every 600 s, at most 32 per pass.
- Nothing is scheduled when an account is deleted. Each pass works out who
  is due from `deleted_at` and the setting in force, so a changed setting
  applies to accounts already deleted: shorter days remove those now past
  due, `now` removes everyone waiting at the next boot, and `never` removes
  no one.
- Removal deletes everything in the deployment's directory that names the
  person. It is recorded as `identity-removed`. The reason is `self` for now
  and `deleted-account-due` from the worker.
- The registration sweep (`ORDO_AUTH_REGISTRATION_TTL_SECS`) skips deleted
  accounts.

The history keeps the random `usr_…` id on every entry the person caused.
After removal, nothing is behind the id, so it names nobody. The history is
never rewritten.

## A person's own download

A person can take a copy of their data before they leave, or at any time.
Owner ruling 2026-10-22: only if the app developer allows it, and the app
developer decides what "all" is.

**It is off until you turn it on.** Mark the read ops whose rows belong in it
with `download`:

```
op allNotes unbounded download =
  notes[id, title, body, tags, created, pinned, archived]
  sort -created

op myFiles unbounded download =
  attachments[id, note, name, kind, size, created, file]
```

- Each marked op is one entity in the file.
- Its projection is the fields. A field you leave out is not in the file.
- Its conditions are the filter. `notes(archived == null)[…]` leaves archived
  notes out.
- A blob field in the projection brings the files. Leave it out and no file
  of that entity is in the download.
- The row guard decides the rows. The door runs each op as the person, so a
  person gets only what they may read.
- It is the person's own. A delegated token and a share guest are refused by
  default (owner, 2026-10-22). The refusal names the setting.
- The app developer opens it to delegated tokens by naming `download` in
  `ORDO_DELEGABLE_OPS`, for example `ORDO_DELEGABLE_OPS=download`. No setting
  opens it to a guest.

`ordo check` refuses a mark it cannot honour: on a write (`download_write`),
on an op with a parameter that has no default (`download_params`), on a
`public` op (`download_public`), on an op that does not read one entity
directly (`download_source`), and on two marked ops over one entity
(`download_entity_twice`).

### The call

`GET /download`, with the person's token and `X-Ordo-Space`. The SDK has one
call for it: `client.downloadOwn()`. In React, `useOwnDownload()` gives
`download()`, `busy` and `error`. Saving the file is the page's choice.

With nothing marked, the door answers `access_denied`, and the message says
how to turn it on.

### The file

A zip. Every entry is stored, with its size and checksum in its header, so
any unzip tool opens it.

```
manifest.json                     what is in it, and "complete"
notes.json                        one object per row, the op's fields
attachments.json
files/attachments/<row id>/file   a file's bytes
```

In an entity's JSON a blob field holds `{ "path", "type", "size" }`, or
`{ "missing": "<why>" }` when the file is not in the zip, or `null` when the
row has none. `manifest.json` lists each entity with its op, its row count
and its file count. `complete` is `false` when any file is missing.

### Bounds and the record

- The answer is streamed. The rows are read first, so a refusal is an
  ordinary refusal. Then the zip is written as it is made, one file in
  memory at a time. Nothing is written to the server's disk.
- `ORDO_OWN_DOWNLOAD_PER_HOUR` (default 6): downloads one person may start in
  one tenant in an hour, per node. Past it, `quota_exceeded` with
  `Retry-After`.
- `ORDO_OWN_DOWNLOAD_MAX_BYTES` (default 1 GiB, at most 4000000000): the
  bytes of files one download holds. A file past it is listed as missing.
- Each download is recorded on the tenant's security-event stream, as a
  `custody-action` of `std.space.download`, naming the entities.

This is not the tenant's export. `std.space.export` is the owner's, takes
the whole tenant, and writes a bundle on the server's disk.

## Not built yet

- The mode as a layered setting (`member.release`) waits on the settings
  machinery (checklist 24a). Until then the program's line is the rule.
- A deployment owner deleting the account of someone who cannot sign in is
  ruled, and placed after the first deployment.
- Scrubbing a deleted person's name from their tenant seats on removal. The
  name stays on seats, and the app decides how to show it.
