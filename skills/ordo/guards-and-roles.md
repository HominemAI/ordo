# Guards, roles and membership

## Guards: who sees which rows inside a tenant

Skip this section when every member may see every row of their tenant.
Membership alone already keeps tenants apart.

```ordo
domain vault @ 0.1.0

entity folders {
  name: text
  acl (read, write, admin)             # per-row sharing
}

entity documents {
  folder -> folders owned              # inherits the folder's guards
  title:  text
}

entity reviews {
  document -> documents                # not owned: guards itself
  reviewer -> members
  verdict:  text = "pending"
  access read   document.folder.foldersAccess(member == $caller)
  access write  reviewer == $caller
}

entity drafts {
  author -> members
  body:   text
  access read   author == $caller
  access write  author == $caller
}

entity posts {
  author -> members
  body:   text
  access read   open
  access write  author == $caller
}

op newFolder($name: text) =
  set folders { name: $name }

op shareFolder($folder: id, $member: id, $level: text) =
  set foldersAccess { folder: $folder, member: $member, level: $level }

op everyDraft unguarded unbounded =
  drafts[body, author.displayName]
```

- An entity with `access` lines states both verbs. One alone is a compile
  error. Open is spelled `open`.
- `acl (levels)` derives a link entity `<entity>Access { <singular>,
  member, level }` and the ops `grant<Entity>Access`,
  `revoke<Entity>Access` and `set<Entity>AccessLevel`. The creator of a row
  gets the top level in the same write. A tenant owner may always
  administer acl rows.
- An `access` line on an owned child replaces the inherited guard whole.
- A guard is a walk from the row to `$caller`: dots toward one, then at
  most one step into a collection with a condition.
- A write to a row the caller cannot read answers `not_found`, the same as
  a missing row. One they may read but not write answers `access_guard`.
- `== $caller` matches the person's own row, never a group they are in.
- `unguarded` on an op skips guards for that op. Use it rarely.

```ordo-refused
# refused: incomplete_access_clause
entity secrets {
  owner -> members
  body:  text
  access read owner == $caller
}
```

## Which ops a role may call

Set outside the program. The default:

- every role calls the program's reads;
- `editor`, `manager` and `owner` call its writes;
- `manager` and `owner` call the derived `std.<entity>.*` ops.

Change it per op with `ORDO_ACCESS_TEMPLATE="op=role,role; op2=owner"`,
read at boot. A role cannot be invented. To hide a button a role cannot
use, the page reads the member's role (`listSpaces`, or an op over
`members(id == $caller)`); the door refuses the call either way.

## Membership: platform ops

Called like any op: `useCommit(name)` or `useOpQuery(name)` after
`withPlatformOps` (`react.md`). Every op below is in it.

| Op | Params | Who may | Does |
|---|---|---|---|
| `std.space.invite` | `role?`, `expires?` (`"7d"`), `to?`, `label?` | owner, manager | answers `{ ticket, expiresAt, role, outcome }` once. Ordo sends nothing: the app shows or mails the link. With no `role` the invitee becomes a `viewer` |
| `std.space.invitations` | none | owner, manager | lists invitations, never the ticket |
| `std.space.revokeInvitation` | `id` | owner, manager | |
| `std.space.setRole` | `member`, `role` | owner | |
| `std.space.remove` | `member` (a `members` id) | owner | removes them. Their next request is refused |
| `std.space.suspend`, `std.space.reinstate` | `member` | owner | |
| `std.space.members` | `after?` | owner, manager | lists members, 500 a page |
| `std.space.leave` | none | any member | the caller leaves |

The last active owner cannot be removed, suspended or demoted.

**Presence.** A platform op that changes something asks for a fresh
passkey touch, and is refused `access_denied` without one. An emailed code,
a password and an OIDC sign-in prove no touch. The deployment owner lists
the ops people may call without one in `ORDO_STEP_UP_EXEMPT_OPS`
(`std.space.invite,std.space.remove`). Ops not listed stay refused to
anyone without a passkey, which is also a way to forbid them. With
passkeys, give `OrdoProvider` a `stepUp` (`react.md`).

## Who gets a tenant

| Setting | Effect |
|---|---|
| `ORDO_AUTH_PROVISIONING=invite-only` (default) | a stranger signs up only with an invitation |
| `ORDO_AUTH_PROVISIONING=auto-onboard` | anyone with a working sign-in may sign up |
| `ORDO_PROVISION_SELF_SERVE=on-signup` | each new person gets a tenant of their own, as `owner`. Its name is their display name |

- **Create a tenant.** There is no create op for an app. Set
  `ORDO_PROVISION_SELF_SERVE=on-signup` and a new person gets one. Wait
  for it with `<TenantGate>` or `waitForTenant` (`signin.md`).
- **Join by invitation.** An owner or manager calls `std.space.invite` and
  shows a link carrying both the space id and the ticket, for example
  `/#/join/<space>/<ticket>`. The invitee signs in with
  `{ space, invitation }`; the session's `admission.outcome` says if they
  joined (`signin.md`).
