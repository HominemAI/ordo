<!-- Generated from skills/ordo/*.md by `ORDO_BLESS_AI_GUIDE=1 cargo test -p ordo-cli --test ai_guide_check`. Edit the skill files, never this one. -->

# Ordo

Ordo turns one folder of `.ordo` files into a multi-tenant backend: the
database, the HTTP door, roles, sign-in and a typed client. You write the
definition. Everything else is derived from it.

This skill is the whole of what an app needs. Do not open the SDK's `.d.ts`
files, its compiled `dist`, `generated/ops.ts` or `interface.json`: every
signature an app touches is in `sdk-signatures.md`, and every op of your app
is one `ordo ops` call away.

## The loop

1. Write or edit the definition (`*.ordo`). Language: `language.md`.
2. `ordo check <dir>`. It prints what it derived and exits 1 on any error.
   Each error names the rule and the fix. Fix, run again.
3. `ordo gen ts <dir> --app <src>`. It writes `<src>/generated/*.ts` (one
   function per op), `<src>/interface.json` (for the React hooks) and
   `<src>/generated/OPS.md`. Run it after every definition change.
4. Look ops up, never guess them:
   - `ordo ops <dir>`: every op, one line each.
   - `ordo ops <dir> <exactName>`: the op's card, with ready-to-paste calls.
   - `ordo ops <dir> <text>`: ops whose name contains the text.
   - Or read `<src>/generated/OPS.md`, which says the same.
5. Call ops from the page (below). `ordo dev <dir>` serves the door on
   `127.0.0.1:4000` with its own Postgres and recompiles on save.

```ordo
domain hello @ 0.1.0

entity notes {
  author -> members
  body:   text
  posted: date
}

op addNote($body: text, $today: date) =
  set notes { author: $caller, body: $body, posted: $today }

op recentNotes unbounded =
  notes[id, rev, body, posted, author.displayName]
  sort -posted
```

## What to reach for first

| Need | Reach for | Read |
|---|---|---|
| show rows | `useOpQuery('op', { params })` | `react.md` |
| write, with conflicts handled | `useCommit('op')` | `react.md` |
| a form for an op | `useOpForm('op')` + `useCommit('op')` | `react.md` |
| sign in, stay signed in, renew, sign out | `createOrdoAuth({ baseUrl })`, in React `useOrdoAuth(auth)` | `signin.md` |
| wait for a new person's tenant | `<TenantGate>` | `signin.md` |
| invite, roles, remove a member | `useCommit('std.space.invite')` after `withPlatformOps` | `react.md`, `guards-and-roles.md` |
| clear an optional field | `null` for its param; the current value keeps it | `client.md` |
| no React (a script, a test, a worker) | the generated functions over `OrdoClient` | `client.md` |
| raw HTTP | only from a tool with no SDK | `client.md` |

Order of preference: `@hominemai/ordo-react` hooks, then the plain client,
then raw `fetch`. Do not hand-write a `useState` + `useEffect` loader, a
sign-in token store or a `fetch('/op/...')` when a hook covers it.

## Rules that hold everywhere

- The grammar is closed. A plausible SQL or TypeScript variant fails to
  parse. If a feature is not in these files, assume it does not exist.
- Isolation is the tenant. Never add an `organisations` entity and a foreign
  key on every row: each customer is a tenant (`language.md`).
- `today` and `now` are not values in ops. The page sends the date.
- Every refusal has a class (`e.class`), and the class says what the page
  does (`client.md`, "Refusals").
- Settings are environment variables named `ORDO_*`. A refused call names
  the setting that would allow it. The app developer is the deployment
  owner and decides them.

## The reference files

Open one only when the task needs it.

| File | Open when |
|---|---|
| `language.md` | writing entities, reads or writes |
| `traps.md` | `ordo check` refused something, or before a tricky op (joins, counts, nulls, upserts) |
| `guards-and-roles.md` | some members must not see some rows; roles; invitations; who gets a tenant |
| `react.md` | page code in React: loading, writing, forms, platform ops |
| `signin.md` | sign-in, staying signed in, renewing, signing out, tenants, invitations |
| `client.md` | calling ops without React, the wire format, refusals, raw HTTP |
| `sdk-signatures.md` | the exact import path, signature or field of any export |

`docs/ai-guide.md` is these files joined into one, for a reader who wants a
single file. It is generated: edit the files here, never the guide.

# The language

Every `ordo` block here passes `ordo check`; every `ordo-refused` block fails
with the code it names (`crates/ordo-cli/tests/ai_guide_check.rs`). A block
starting `domain` is a whole program; any other adds to the one above it.

## The model

| Word | Means |
|---|---|
| deployment | one running Ordo: the door (HTTP server) plus Postgres |
| tenant, or space | one isolated database of the app. Ops always run inside one tenant, named by the `X-Ordo-Space` header |
| member | a person's row in one tenant: the platform entity `members`, with a `role` |
| role | `owner`, `manager`, `editor` or `viewer`. Fixed. There are no others |
| op | a named read or write you declare. The door serves it at `/op/<name>` |
| `$caller` | the calling person's own `members` row in this tenant |

**Isolation is the tenant.** When customers, teams or organisations must
not see each other's data, make each one a tenant. Do not add an
`organisations` entity and a foreign key on every row: rows never cross
tenants, refs stay inside one, and the door refuses a person who is not a
member of the tenant before any op runs. Inside a tenant, every member sees
every row unless you write a guard (`guards-and-roles.md`).

## Files and names

- A program is a folder of `.ordo` files. One file starts with
  `domain <name> @ <semver>`.
- Every name is lowerCamel: `[a-z][A-Za-z0-9]*`. No `_`, no `-`.
- `#` comments to end of line. Strings use double quotes only.
- Durations: `ms s m h d w mo y`. `m` is minutes; months are `mo`.

## Entities

```ordo
domain studio @ 0.1.0

entity projects {
  name:     text(120)                  # required, at most 120 characters
  archived: bool = false               # defaulted: literals only
  lead   -> members?                   # optional ref to a person
}

entity notes (key: note) {             # ids look like note_...
  project -> projects owned            # deleted with its project
  author  -> members                   # required ref
  body:    text
  pinned:  bool = false
  posted:  date
  status:  text = "open"
  check status == "open" or status == "closed"
  index (project, posted)
}

entity tags {
  project -> projects owned
  label:   text
}

entity codes {
  code:  text unique                   # unique within the tenant
  label: text
}
```

- Types: `text`, `text(n)`, `int`, `bool`, `date`, `datetime`,
  `money(p)`, `decimal(p)`, `duration(ms)`, `duration`, `id`, `json`.
- A field is required (`f: t`), defaulted (`f: t = literal`) or nullable
  (`f: t?`). A default is a literal: `today`, `now` and `newid` are refused.
- A ref is `name -> entity`, nullable with `?`. `owned` makes the child
  die with its parent. `tree` marks a self-ref hierarchy.
- `id` and `rev` exist on every entity. Never declare them.
- `members` and `groupMembers` exist in every tenant. Refer to them; never
  declare them. `members` fields: `displayName`, `role`, `kind`
  (`"principal"` or `"group"`), `status` (`"active"`, `"suspended"`,
  `"left"`, `"deleted"`), `joined`, `memberCount`.
- `unique` on one field is a modifier. On several it is a line:
  `unique (a, b)`. Uniqueness is per tenant.
- `check` sees only the row's own columns. A closed set of words is a
  `text` field plus a `check`; there is no `enum` type.
- `f: text sealed` is encrypted at rest: no sort, range or sum on it. Seal
  only what you display.

## Reads

A read is a pipeline: sources, then steps, one per line.

```ordo
op listProjects unbounded =
  projects(archived == false)[id, rev, name, lead, lead.displayName]
  sort name

op projectNotes($project: id) unbounded =
  notes(project == $project)[id, rev, body, pinned, author.displayName, posted]
  sort -pinned, -posted

op recent =
  notes[body, posted]
  sort -posted
  take 20

op people unbounded =
  members(kind == "principal" and status == "active")[id, displayName, role]
  sort displayName

op me =
  members(id == $caller)[id, displayName, role]
```

- `source(condition)[projection]`. The condition and the projection are
  both optional. `notes()` with empty parens never parses.
- Project `id` and `rev` on any read a page edits from: a CAS write needs
  the `rev` it read.
- A dot walks a ref toward one row: `author.displayName`,
  `project.lead.displayName`. It never walks toward many.
- Rename a column with `name: expr`: `[body, who: author.displayName]`.
- Steps: `sort a, -b` (descending with `-`), `take 20`, `take[keys] 1`
  (top 1 per key), `group keys { name: aggregate }`, `reduce { ... }` (one
  row), `| condition` (filter the rows so far), `distinct`,
  `match any src(cond)`, `match none src(cond)`.
- Aggregates: `count`, `count(cond)`, `sum f`, `avg f`, `min f`, `max f`,
  `distinct f`.
- Conditions: `== != < <= > >=`, `and`, `or`, `not`, `in ("a", "b")`,
  `is null`, `is not null`, `contains`, `starts`, `under` (trees).
- Functions: `coalesce`, `cond(c1 -> v1, c2 -> v2, else)`, `round`,
  `floor`, `ceil`, `abs`, `mod`, `least`, `greatest`, `lower`, `trim`,
  `length`, `year`, `month`, `day`, `dow`. Text joins with `+`.
- `unbounded` after the params says the read scans the tenant. Write it on
  every read that lists. `ordo check` refuses a read with no provable row
  bound that lacks it, for example `take $n` where `$n` has no `<=` bound.
- Paging is a keyset token the door hands back. There is no offset.

Two sources in one pipeline join along their declared ref. The first is
kept even when it has no match, with nulls on the other side:

```ordo
op noteCounts unbounded =
  projects[name]  notes[id]
  group projects.id, projects.name { n: count, open: count(status == "open") }
  | n > 0
  sort -n

op latestNote unbounded =
  projects[name]  notes[body, posted]
  sort -notes.posted
  take[projects.id] 1

op quiet unbounded =
  projects[name]
  match none notes(status == "open")
```

## Parameters

`$name: type`, with an optional default (`$lead: id = null`) and bound
(`$limit: int <= 100`). A param without a default is required. The caller
sends params by name. `$caller` is not a param: it is never sent.

A param sent as `null` is the same as a param left out: the door uses its
default. So `null` never overrides a default.

## Writes

`set` makes a described state true. The braces decide the mode, and
`ordo check` prints it.

```ordo
op addNote($project: id, $body: text, $today: date) =
  set notes { project: $project, author: $caller, body: $body, posted: $today }

op renameProject($id: id, $rev: int, $name: text(120), $lead: id = null) =
  set projects { id: $id, rev: $rev, name: $name, lead: $lead }

op clearLead($id: id) =
  set projects(id == $id) { lead: null }

op archiveProject($id: id) =
  set projects(id == $id) { archived: true }

op closeOld($before: date) =
  set notes(status == "open" and posted < $before) { status: "closed" }

op addCode($code: text, $label: text) =
  set codes { code: $code, label: $label }

op deleteProject($id: id) =
  del projects { id: $id }

op startProject($name: text(120), $first: text, $today: date) =
  let p = set projects { name: $name, lead: $caller }
  set notes { project: p.id, author: $caller, body: $first, posted: $today }
```

| Braces hold | Mode |
|---|---|
| no `id`, no unique field | create. Must cover every required field |
| `id` | converge that row (absent: `not_found`) |
| `id` and `rev` | compare-and-set: a stale `rev` answers `cas_miss` |
| every field of a `unique` | converge-or-create (an upsert) |
| `set e(condition) { }` | bulk converge |

- A converge writes only the fields in the braces. Defaults apply on create
  only.
- **An upsert counter**: `set counters { name: $k, n: n + 1 }` creates with
  `n` read as its default (`n: int = 0` stores 1), then adds 1. A read
  field with no default is refused.
- A field in the braces is always written. In `renameProject` above, a
  caller who leaves `lead` out sends the default, `null`, and clears it.
  Give a field its own op when it must stay put unless asked.
- **To clear an optional field**, write `null` in the braces, as `clearLead`
  does, or give the param `= null` and send `null`. Sending `null` for a
  param is the same as leaving it out, so it clears only where the
  param's default is `null`. The generated client makes such a param
  required on a converge, so the page says keep (the current value) or
  clear (`null`) every time.
- `del e { id: $id }` deletes one row and its owned children.
  `del e(condition)` deletes many. Bare `del e` does not parse.
- Several statements in one op are one atomic group.
- `let x = set ...` binds a create only. `x.id` is usable in later
  statements of the same op.

## Functions and secrets

A `function` is for what the language cannot express, such as calling an
outside API. Reach for an op first. The code is a bundled JavaScript file.

```ordo
function suggestTitle in studio {
  call($text: text(20000)) -> json by member
  code "fn/suggest-title.js"
  egress api.anthropic.com
  secrets llmKey
  limit wall 30s, memory 64 mb
}

secret llmKey for api.anthropic.com
```

- `call` and `code` are required. `by member` is the only caller. It
  answers `json`.
- `egress` lists the hosts `fetch` may reach. A hyphenated host is quoted.
- `secret name for host, host` binds a value to exact hosts. The deployment
  owner sets it with `ordo secret set`. `secrets name` on a function must
  name a declared secret bound to one of its `egress` hosts.
- `secret name` alone is a signing key, never sent. The function signs or
  verifies with it through `crypto.subtle.importKey("secret", "name", ...)`
  and needs no `egress`.
- The code is `export default { async call(input, ctx) { ... } }`. It
  gets `ctx.secret("llmKey")` as a handle to put in a header. It never
  reads the value.
- A `call` parameter may be `blob` (`call($file: blob)`); an op parameter
  may not. The page uploads to the blob door first and passes the handle.
  `await ctx.blob(input.file)` answers a `Blob`, read as the caller. A file
  the caller may not read is `not_found`. One call reads at most
  `ORDO_FN_BLOB_BYTES` (20 MiB); `(await file.bytes()).toBase64()` makes
  it JSON-ready.
- The page calls it by name (`client.callFunction`, `useFunction`).
  The body is at most `ORDO_FN_BODY_BYTES` (1 MiB).
  `ordo fn runs` and `ordo fn log` show what happened.

## Commands

| Command | Does |
|---|---|
| `ordo check <dir>` | compile; print entities, guards, modes, bounds. Exit 1 on error |
| `ordo dev <dir>` | local door on `127.0.0.1:4000` with its own Postgres, recompiles on save, prints a dev token |
| `ordo ops <dir> [query]` | the op index, one op's card, or a name search |
| `ordo gen ts <dir> --app <src>` | the typed client, `interface.json` and `OPS.md` into the app |
| `ordo plan <dir> --against <db>` | what a deploy would change. Read-only |
| `ordo apply <dir>` | store and activate the program on a deployment |

A change that adds things (an entity, an optional or defaulted field, an
index) applies live. A new required field over existing rows, a type change
or a drop is refused by name.

## Coming from elsewhere

| You would write | In Ordo |
|---|---|
| `JOIN ... ON` | list both sources; the declared ref joins them |
| `GROUP BY ... HAVING` | `group keys { }` then `\| condition` |
| `CASE WHEN` | `cond(c -> v, else)` |
| `INSERT ... ON CONFLICT` | `set` covering a `unique` field |
| `LIMIT 10 OFFSET 20` | `take 10`; paging by the door's token |
| `WHERE x IN (SELECT ...)` | `match any src(cond)` |
| `NOT EXISTS` | `match none src(cond)` |
| `UPDATE ... SET x = NULL` | `set e(id == $id) { x: null }` |
| a tenant id column | a tenant per customer |
| a `users` table | `members`, refer with `-> members` |
| a row-level policy | `access read` and `access write`, or `acl` |
| a role check in code | the role's op list (`guards-and-roles.md`) |
| `created_at DEFAULT now()` | a `$today: date` param the page sends |
| a transaction | several statements in one op |

# The rules generated code drifts on

Each rule below is one `ordo check` refuses, or one that compiles and means
something other than what was meant. The examples add to the `studio`
program in `language.md`.

**`today` and `now` are not values.** Take the date as a param.

```ordo-refused
# refused: unknown_field
op stale = notes(posted <= today)[body]
```

**`and` with `or` needs parentheses.** There is no precedence.

```ordo-refused
# refused: parse_error
op mixed unbounded = notes(pinned and status == "open" or posted > 2026-01-01)[body]
```

**Null is two-valued.** `null == null` is true, `null != 5` is true, and
every ordering with null is false. Nulls sort last in both directions. A
row kept by a join has nulls on the missing side, so a filter like
`posted < $d` drops it: add `or posted is null` when it should stay.
`count` and `sum` of nothing are 0; `avg`, `min`, `max` of nothing are null.

**One many-branch per pipeline.** Two children of one parent in one
pipeline would multiply rows, so it is refused. Count each side in a `let`
and join the lets back. A `let` ending in `group` is a source, joined on
its group key; list its parents with `[]` so no column clashes.

```ordo-refused
# refused: fan_out
op both unbounded =
  projects[name]  notes[id]  tags[id]
  group projects.id { n: count }
```

```ordo
op busy unbounded =
  let noteCount = projects[]  notes[]  group projects.id { n: count }
  let tagCount  = projects[]  tags[]   group projects.id { t: count }
  projects[name]  noteCount[n]  tagCount[t]
  | coalesce(t, 0) * 2 < coalesce(n, 0)
  sort name
```

**A `let` ending in `reduce` is a scalar**, read by dot. Any other `let` is
a source only.

```ordo
op manyPinned =
  let total = notes reduce { n: count }
  notes(pinned)[body]
  | total.n > 3
```

**Compare by multiplying, not dividing.** `/` yields `decimal`, fails on
`/ 0`, and never lands in a `money` or `int` slot without `round`, `floor`
or `ceil` (`implicit_rounding`).

**A unique field turns a create into an upsert.** `set codes { code: ... }`
converges the row that already has that code. So `let` cannot bind it:

```ordo-refused
# refused: unbindable_write
op twoCodes($c: text) =
  let first = set codes { code: $c, label: $c }
  set codes { code: $c + "2", label: first.id }
```

**A create covers every required field**, refs included.

```ordo-refused
# refused: insert_incomplete
op orphanNote($p: id, $body: text) =
  set notes { project: $p, body: $body, posted: 2026-01-01 }
```

**A defaulted param in a converge is always written.** Left out, it writes
its default. An "edit" op with `$lead: id = null` clears the lead whenever
the page leaves it out. Split such fields into their own ops.

**`null` from the page means "use the default".** It never clears a field by
itself. Clearing is `{ field: null }` written in the op.

**Words that do not exist:** `if` and `CASE` (use `cond`), `uniq` (use
`distinct`), `enum(...)`, `JOIN`, `OFFSET`, `LIMIT` (use `take`), `grant`,
`today` and `now` in ops, `true` as a guard (use `open`), `_` in names.

```ordo-refused
# refused: parse_error
op sorted unbounded = notes[body] sort if(pinned, 0, 1)
```

```ordo
op sorted unbounded =
  notes[body, posted]
  sort cond(pinned -> 0, 1), -posted
```

**A listing read without `unbounded`** is refused when nothing bounds its
rows. Write `unbounded`, or `take n` with a literal or a `<=`-bounded param.

**A function reaches only its `egress` hosts and never reads a secret.**
`ctx.secret("name")` is a handle for a header. The host fills the value in
on the way out, and only for the hosts the secret is bound to. Code cannot
print, hash or compare the value. A secret a function lists must be bound
to one of its `egress` hosts.

```ordo-refused
# refused: invalid_function
function ask in studio {
  call($q: text(200)) -> json
  code "fn/ask.js"
  egress api.one.example
  secrets key
}
secret key for api.two.example
```

A function is not a way round the language. Reads and writes of your own
rows are ops. A function holds no database access and cannot call an op.

**A file reaches a function only as a `blob` argument, read as the
caller.** Do not put file bytes or base64 in the call's JSON: the body
stops at 1 MiB. Upload to the blob door, pass the handle, and read it with
`ctx.blob(input.file)`. The function cannot read a file the caller could
not, nor a handle it was not passed.

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

# React: `@hominemai/ordo-react`

Headless hooks over the app's `interface.json`. They load, write, build
forms and keep state; they render nothing (except `TenantGate`). Reach for
them before any hand-written loader or `fetch`. Everything is imported from
`@hominemai/ordo-react` itself. Exact signatures: `sdk-signatures.md`.

## Setup: one client, one provider

`ordo gen ts <dir> --app <src>` writes `<src>/interface.json`. Import it
(`resolveJsonModule` on) and hand it to `parseInterface`. Do not read it:
look ops up with `ordo ops` or `<src>/generated/OPS.md`.

```tsx
import { useMemo, type ReactNode } from 'react';
import { DescriptorClient, OrdoProvider, parseInterface, withPlatformOps } from '@hominemai/ordo-react';
import doc from './interface.json';
import { auth } from './auth'; // createOrdoAuth, signin.md

// Name every platform op the app calls (see "Platform ops" below).
const iface = withPlatformOps(parseInterface(doc), 'std.space.invite', 'std.space.invitations', 'std.space.setRole');

// Asked once when a platform write is refused for want of a fresh touch.
const touch = async () => {
  await auth.stepUp();
};

export function Workspace({ space, children }: { space: string; children: ReactNode }) {
  const client = useMemo(() => new DescriptorClient({ ...auth.clientOptions(), space, iface }), [space]);
  return <OrdoProvider client={client} stepUp={touch}>{children}</OrdoProvider>;
}
```

- `auth.clientOptions()` is `{ baseUrl, token, renewToken }`. `token` is a
  function, so every call gets a live, renewed token (`signin.md`).
- Another tenant: `client.withSpace(otherSpace)`.
- `live` on `OrdoProvider` (`live` or `live={{ paceMs }}`) makes every read
  hook refetch when the door says its rows changed. Off by default: each
  live hook holds a connection open.
- Leave `stepUp` out when the app has no passkeys.

## Loading rows: `useOpQuery`

```tsx
import { useOpQuery } from '@hominemai/ordo-react';
import type { ProjectNotesRow } from './generated/ops.js';

export function Notes({ project }: { project: string }) {
  const notes = useOpQuery<ProjectNotesRow>('projectNotes', { params: { project } });
  if (notes.loading) return <p role="status">Loading…</p>;
  if (notes.refusal) return <p role="alert">{notes.refusal.message}</p>;
  return (
    <ul>
      {notes.rows.map((n) => (
        <li key={n.id}>{n.body}</li>
      ))}
    </ul>
  );
}
```

- Answers `{ rows, row, loading, refusal, revoked, refetch, retrying, ... }`.
  `row` is `rows[0]`, for a one-row read.
- Params are compared by content: `{ params: { project } }` inline is fine.
- `enabled: false` skips the call until its params are ready.
- `refine: (f) => [f.posted.gt('2026-01-01')]` and `pick: ['body']` narrow
  the rows at the door, over the op's own output columns.
- The first page only (`useOpPages` pages on). A listing that may pass one
  page uses `useOpPages`.
- The row type `<Op>Row` comes from `./generated/ops.js` (names in OPS.md).
  Without it the rows are `Record<string, unknown>`.

## Writing: `useCommit`

```tsx
import { useCommit, useOpQuery } from '@hominemai/ordo-react';

export function AddNote({ project }: { project: string }) {
  const notes = useOpQuery('projectNotes', { params: { project } });
  const add = useCommit('addNote');

  async function save(body: string) {
    const today = new Date().toLocaleDateString('en-CA'); // YYYY-MM-DD, local
    const s = await add.commit({ project, body, today });
    if (s.status === 'committed') notes.refetch();
  }
  // ...render a form calling save(); show add.refusal?.message; disable on add.committing
}
```

- `commit(values, rev?)` answers the new `CommitState` and never throws for
  a refusal. `state.status` is `idle`, `committing`, `committed`
  (`state.response.rows`), `conflict` or `refused` (`state.refusal`).
- A write does not refresh any `useOpQuery`. Call its `refetch()` after
  `committed`, or turn `live` on.
- **CAS.** An op whose braces hold `id` and `rev` declares `rev`. Pass the
  `rev` you read as the second argument, never inside `values`:

```tsx
import { useCommit } from '@hominemai/ordo-react';

const rename = useCommit('renameProject');
const s = await rename.commit({ id: p.id, name: newName }, p.rev);
if (s.status === 'conflict') {
  const current = await rename.resolve(); // the row as stored now, or null if it was deleted
  // show `current` to the person; to keep their edit:
  if (current) await rename.retry({ id: p.id, name: newName }, current.rev as number);
}
```

- `useCommit(name, { autoResolve: true })` fetches the current row as soon
  as a conflict arrives (`state.current`, `state.currentIs`).
- **Clearing an optional field** (an optional ref, a `date?`): on an op
  that converges a row, send `null` to clear it and the current value to
  keep it. Leaving it out writes the param's default, which clears too.
  `ordo ops <dir> <op>` prints which params do this.
- A CAS op committed without `rev` is refused before the wire
  (`missing_param`). There is no last-write-wins path.

## Forms: `useOpForm`

The form's fields come from the op's params: names, types, required,
enum options, reference pickers. `rev` is hidden and owned by the runtime.

```tsx
import { useOpForm, useCommit } from '@hominemai/ordo-react';
import type { FormEvent } from 'react';

export function NewNote({ project, onDone }: { project: string; onDone: () => void }) {
  const form = useOpForm('addNote', { project });
  const add = useCommit('addNote');

  async function submit(e: FormEvent) {
    e.preventDefault();
    const s = await add.commit(form.params);
    if (s.status === 'committed') {
      form.reset();
      onDone();
    }
  }

  return (
    <form onSubmit={submit}>
      {form.visible
        .filter((f) => f.name !== 'project')
        .map((f) => (
          <p key={f.name}>
            <label htmlFor={f.name}>{f.name}</label>
            <input id={f.name} name={f.name} required={f.required}
              type={f.type === 'date' ? 'date' : 'text'}
              value={String(form.values[f.name] ?? '')}
              onChange={(e) => form.setValue(f.name, e.target.value || undefined)} />
            {form.issueFor(f.name) && <span role="alert">{form.issueFor(f.name)?.message}</span>}
          </p>
        ))}
      <button type="submit" disabled={!form.valid || add.committing}>Add note</button>
      {add.refusal && <p role="alert">{add.refusal.message}</p>}
    </form>
  );
}
```

- Set values in their wire type: `int` a number, `bool` a boolean, `money`
  and `decimal` a string, `date` `"YYYY-MM-DD"`, `id` the typed id string.
  `issues` uses the door's own classes and messages.
- Set an emptied field to `undefined`, not `""`: `form.params` drops only
  `undefined` and `null`.
- `f.kind`: `scalar`, `enum` (`f.options`), `reference`
  (`await form.searchCandidates(f.name)` gives rows for a picker), `json`,
  `rev` (hidden).
- For a CAS form: `add.commit(form.params, row.rev)`.

## Platform ops

`std.space.*` ops are not in `interface.json`. Name the ones the app calls
in `withPlatformOps(iface, ...names)`; then `useOpQuery` and `useCommit`
call them like program ops, typed by name.

```tsx
import { useCommit, useOpQuery } from '@hominemai/ordo-react';

const invite = useCommit('std.space.invite');
const s = await invite.commit({ role: 'editor', expires: '7d' });
if (s.status === 'committed') {
  const { ticket } = s.response.rows[0];
  showLink(`${location.origin}/#/join/${space}/${ticket}`);
}

const invitations = useOpQuery('std.space.invitations'); // rows: id, role, label, status, expiresAt, ...
const setRole = useCommit('std.space.setRole');          // commit({ member, role })
const members = useOpQuery('std.space.members');         // rows: member, principal, name, role, status, joined, groups
const remove = useCommit('std.space.remove');            // commit({ member })
const leave = useCommit('std.space.leave');              // commit({}): the caller's own seat
```

Names `withPlatformOps` takes, all `std.space.`: `share`, `shareLink`,
`setLinkPassword`, `setLinkMaxOpens`, `revokeShare`, `revokeLink`, `links`,
`shares`, `invite`, `invitations`, `revokeInvitation`, `setRole`,
`members`, `admit`, `suspend`, `reinstate`, `remove`, `leave`, `profile`,
`setOpAccess`, `resetOpAccess`, `setMemberAccess`, `createGroup`,
`addToGroup`, `removeFromGroup`, `deleteGroup`, `groups`. And
`std.delegation.review`, `approve`, `decline`, `mine`, `revoke`.

- `std.space.members` answers at most 500 rows in member id order. For
  more, pass the last `member` as `params: { after }`. A program read over
  `members[id, displayName, role]` is often simpler.

## Other hooks

| Hook | For |
|---|---|
| `useOpPages(op, { params })` | a read paged on: `rows`, `hasMore`, `more()`, `exhaust()`, `loading`, `refusal` |
| `useOrdoAuth(auth)` | sign-in state over `createOrdoAuth`: `{ phase, session, retry, auth }` (`signin.md`) |
| `useTenantStatus`, `<TenantGate>` | a new person's tenant (`signin.md`) |
| `useRow(id)` | the ops that act on a typed id, from the id alone |
| `useFileUpload`, `useFileDownload`, `useFile` | file fields |
| `useOwnDownload` | the person's own data as a zip |
| `useAccountEmail` | the person's confirmed email address |

# Sign-in, staying signed in, tenants

The door serves `/auth/*`. The deployment owner picks the methods with
`ORDO_AUTH_METHODS` (`email`, `passkey`, `password`, OAuth names). The
page's origin must be in `ORDO_CORS_ORIGINS`.

## One session object

`createOrdoAuth({ baseUrl })` is the front door. Make it once, at module
level, and import it where needed. In React import it from
`@hominemai/ordo-react`; without React from
`@hominemai/ordo-sdk/auth/session.js`. Exact signatures:
`sdk-signatures.md`.

```ts
import { createOrdoAuth } from '@hominemai/ordo-react';

export const DOOR = 'http://127.0.0.1:4000';
export const auth = createOrdoAuth({ baseUrl: DOOR });
```

| Need | Call |
|---|---|
| start signed in after a reload | `useOrdoAuth(auth)` (React), or `await auth.resume()` |
| email code | `auth.beginEmail({ address })`, then `auth.completeEmail(sent, code, { display })` |
| passkey | `auth.registerPasskey({ display })` (first time), `auth.signInWithPasskey()` |
| a live token for every call | `auth.token` (pass the function, do not call it) |
| renew | automatic: a timer renews ahead of expiry, and `auth.token()` renews when near |
| sign out | `await auth.signOut()` |
| the person's tenants and roles | `await auth.spaces()` |
| wait for a new person's own tenant | `<TenantGate baseUrl={DOOR} token={auth.token}>`, or `await auth.waitForTenant()` |
| prove presence again | `await auth.stepUp()` |
| which methods are on | `signInMethods({ baseUrl: DOOR })` from `@hominemai/ordo-sdk/auth/methods.js` |
| read an invitation before sign-in | `peekInvitation({ baseUrl: DOOR }, ticket)` from `@hominemai/ordo-sdk/auth/share.js` |

- `auth.token()` throws a `PasskeyError` with `failure: 'refused'` when
  nobody is signed in. A session that ended (signed out elsewhere, past
  its limit) signs the object out, and `useOrdoAuth` shows `signed-out`.
- Renewal first uses the token, then the browser's renewal cookie. A door
  that cannot be reached throws and keeps the session: never show sign-in
  for it.
- A session from another ceremony (password, second step, guest) is kept
  with `auth.use(session)`.

## The app shell in React

```tsx
import { TenantGate, useOrdoAuth } from '@hominemai/ordo-react';
import { auth, DOOR } from './auth';

export function App() {
  const { phase, retry } = useOrdoAuth(auth);
  if (phase === 'checking') return <p role="status">Loading…</p>;
  if (phase === 'failed')
    return (
      <p role="alert">
        Cannot reach the server. <button type="button" onClick={retry}>Try again</button>
      </p>
    );
  if (phase === 'signed-out') return <SignIn />;
  // With ORDO_PROVISION_SELF_SERVE=on-signup: waits for the person's own tenant.
  return (
    <TenantGate baseUrl={DOOR} token={auth.token}>
      {(space) => <Workspace space={space} />}
    </TenantGate>
  );
}
```

`Workspace` builds the client and `OrdoProvider` (`react.md`). A sign-out
button is `onClick={() => auth.signOut()}`; the shell follows by itself.

- **Same site.** Resume uses an `HttpOnly` cookie, so the page and the door
  must be the same site. `localhost` and `127.0.0.1` are different sites:
  use one name for both. Across sites nothing resumes and the person signs
  in on every reload.
- **Without self-serve tenants**, skip `TenantGate`. `await auth.spaces()`
  answers `[{ space, name, role, via }]`; let the person pick one. An empty
  list means they belong nowhere yet.

## Email sign-in form

```tsx
import { useState, type FormEvent } from 'react';
import { OrdoRefusal, type EmailCodeSent } from '@hominemai/ordo-react';
import { auth } from './auth';

type Invite = { space: string; invitation: string };

export function SignIn({ invite }: { invite?: Invite }) {
  const [address, setAddress] = useState('');
  const [name, setName] = useState('');
  const [sent, setSent] = useState<EmailCodeSent | null>(null);
  const [code, setCode] = useState('');
  const [error, setError] = useState<string | null>(null);
  const say = (e: unknown) => setError(e instanceof OrdoRefusal ? e.refusal.message : 'Something went wrong. Try again.');

  async function send(e: FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      setSent(await auth.beginEmail({ address, ...invite }));
    } catch (err) {
      say(err);
    }
  }

  async function finish(e: FormEvent) {
    e.preventDefault();
    if (!sent) return;
    setError(null);
    try {
      // `display` names a new person; a known person keeps their name.
      const r = await auth.completeEmail(sent, code, { ...invite, display: name });
      if (r.kind === 'second-step') setError('This account needs a second step.');
      // A session: useOrdoAuth now answers signed-in.
    } catch (err) {
      say(err); // a wrong, expired or used code is one refusal: failure 'refused'
    }
  }

  return sent === null ? (
    <form onSubmit={send}>
      <label htmlFor="email">Email</label>
      <input id="email" name="email" type="email" autoComplete="username" required
        value={address} onChange={(e) => setAddress(e.target.value)} />
      <label htmlFor="name">Your name (new accounts)</label>
      <input id="name" name="name" autoComplete="name" value={name} onChange={(e) => setName(e.target.value)} />
      <button type="submit">Email me a code</button>
      {error && <p role="alert">{error}</p>}
    </form>
  ) : (
    <form onSubmit={finish}>
      <label htmlFor="code">Code from the email</label>
      <input id="code" name="code" autoComplete="one-time-code" inputMode="numeric" required
        value={code} onChange={(e) => setCode(e.target.value)} />
      <button type="submit">Sign in</button>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}
```

- Keep `sent` (challenge and binder) in memory only. Never in a URL or storage.
- `beginEmail` answers the same for every address. It never says whether
  the address is known.
- A session may carry `device`. Keep it in `localStorage` and pass it as
  `device` to the next `beginEmail`.

## Without React

```ts
import { createOrdoAuth } from '@hominemai/ordo-sdk/auth/session.js';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';

const auth = createOrdoAuth({ baseUrl: DOOR });
if ((await auth.resume()) === null) {
  // sign in: auth.beginEmail, then auth.completeEmail
}
const space = (await auth.spaces())[0]?.space ?? (await auth.waitForTenant());
const client = new OrdoClient({ ...auth.clientOptions(), space });
```

`auth.clientOptions()` is `{ baseUrl, token, renewToken }`. The plain
functions are there too: `refreshSession(door, token)` and
`signOut(door, token)` in `auth/session.js`.

## Invitations

1. An owner or manager calls `std.space.invite` (`react.md`) and gets a
   `ticket` once. The app builds a link with the space id and the ticket,
   for example `/#/join/<space>/<ticket>`, and shows or mails it. Ordo sends
   no mail for invitations.
2. The landing page may call `peekInvitation(door, ticket)` before sign-in:
   `{ space, role, ticketExpiresAt, named }`.
3. The invitee signs in with `{ space, invitation: ticket }` passed to
   `auth.beginEmail` and `auth.completeEmail` (or to `auth.signInWithPasskey`
   / `auth.registerPasskey`).
4. `session.admission` is `{ space, outcome }`. `admitted` or
   `already-member` means they are in `space`. Anything else (`refused`)
   says nothing about why.
5. `declineInvitation(door, ticket, token?)` spends the ticket.

## A new person's tenant

With `ORDO_PROVISION_SELF_SERVE=on-signup`, the tenant is made just after
the sign-in answers.

- React: `<TenantGate baseUrl token>{(space) => app}</TenantGate>`, with
  optional `settingUp`, `stuck` and `failed` slots. Or
  `useTenantStatus({ baseUrl, token })` for
  `{ phase, space, status, problem, retry }`.
- Plain: `const space = await auth.waitForTenant()`. It throws a
  `TenantError` whose `failure` is `failed`, `off`, `none`, `timeout`,
  `refused`, `limit_reached` or `aborted`.
- Just after a tenant is made or joined, the first call is served.
  `access_denied` is never a delay; `server_busy` is, and is retried.

## Passkeys and step-up

- `auth.registerPasskey({ display, space?, invitation? })` signs up.
- `auth.signInWithPasskey({ space?, invitation? })` signs in.
- `auth.stepUp()` proves presence for a few minutes and keeps the
  re-minted session.
- A `PasskeyError` carries `failure`: `cancelled` (the person dismissed
  the prompt; say nothing), `unsupported`, `refused`, `unavailable`
  (try again), and a few setting-named ones whose `setting` and `value`
  say what the deployment owner would change.

# The plain client, the wire, refusals

Use this outside React (a script, a test, a worker), or where a hook does
not fit. In React prefer `react.md`.

## The generated functions

`ordo gen ts <dir> --app <src>` writes `<src>/generated/`: one function
per op, with its types, importing `@hominemai/ordo-sdk` by package name.
Regenerate after every definition change; never edit it, never open
`generated/ops.ts`. Look ops up with `ordo ops <dir> <name>` (a card with
ready-to-paste calls) or `<src>/generated/OPS.md`.

```ts
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { OrdoRefusal } from '@hominemai/ordo-sdk/core/refusal.js';
import { addNote, projectNotes, renameProject, setLead } from './generated/ops.js';
import { auth } from './auth'; // createOrdoAuth, signin.md

const client = new OrdoClient({ ...auth.clientOptions(), space });

const page = await projectNotes(client, { project }).fetch();      // page.rows, page.hasMore
const all = await projectNotes(client, { project }).rows();         // every page
const some = await projectNotes(client, { project })
  .where((f) => [f.pinned.eq(true)])
  .pick('id', 'body')
  .fetch();

await addNote(client, { project, body, today: '2026-10-30' }).commit();

try {
  await renameProject(client, { id: p.id, rev: p.rev, name: newName }).commit();
} catch (e) {
  if (e instanceof OrdoRefusal && e.class === 'cas_miss') {
    // someone changed it first: reload, show the new state
  } else throw e;
}

// An op that converges a row and fills an optional field: the key is required.
await setLead(client, { id: p.id, lead: null }).commit();   // clears it
await setLead(client, { id: p.id, lead: member }).commit(); // sets or keeps it
```

- Each function is `opName(client, params)`. A read answers a `ReadCall`
  (`.fetch()`, `.pages()`, `.rows()`, `.where()`, `.pick()`); a one-row
  read a `SingleReadCall` (`.fetch()`, `.row()`); a write a `WriteCall`
  (`.commit()`). An op with no params takes no second argument.
- Types per op, named after it in PascalCase: `<Op>Row`, `<Op>Params`,
  `<Op>Fields`, `<Op>Refusal`. Ids: `<Entity>Id` in `generated/ids.ts`.
- Ids are branded per entity. A row's ref column already has the right
  brand. A plain string needs `asId<'projects'>(s)` from
  `@hominemai/ordo-sdk/core/brand.js`, or a cast.
- `rev` is an ordinary param of a CAS op here. Left out, the call is
  refused before the wire (`missing_param`).
- An optional param: leave it out, or pass `null`, to send its default.
  The door reads `null` as left out.
- **Clearing an optional field.** On an op that converges a row (an edit),
  every field in its braces is written, so a param that fills an optional
  field is required in the types: pass the current value to keep it, or
  `null` to clear it (when its default is `null`, the usual `= null`). On a
  create the same param stays optional. `ordo ops <dir> <op>` says, per
  param, how to clear.
- Money and decimals travel as strings. Dates as `YYYY-MM-DD`. Datetimes
  as ISO strings.
- `server_busy` and `serialization_conflict` are retried by the client
  already. `token` may be a function, so it can renew (`signin.md`).

## Calling a function

```ts
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { auth } from './auth';

const client = new OrdoClient({ ...auth.clientOptions(), space });
const { result } = await client.callFunction<{ title: string }>(
  'suggestTitle',
  { text: body },
);
```

`result` is the JSON the function returned. A failure is an `OrdoRefusal`
whose class is `function_error` (the code threw; the message is its own),
`function_timeout`, `function_oom`, `function_busy` (retry later),
`egress_refused` or `space_paused`. Wire: `POST /fn/<name>` with the
parameters as a JSON body. In React use `useFunction(name)`.

A file goes in by handle, never in the body. Upload it to the row's `blob`
field first (`uploadFile`, `useFileUpload`), read the row back for the
handle, then pass it: `client.callFunction('extractDocument', { file:
row.file })`. A file the caller may not read is `not_found`. A body over
`ORDO_FN_BODY_BYTES` (1 MiB) is `bound_violation`.

## The wire

Read: `GET /op/<name>?param=value`. Write: `POST /op/<name>` with a JSON
body. Delete-only op: `DELETE`. Every call sends
`Authorization: Bearer <token>` and `X-Ordo-Space: <tenant id>`. Success is
`{ "rows": [...] }`, with `next` when another page exists; send it back as
the `X-Ordo-Page` header. A refusal is
`{ "error": { "code", "message", "op"?, "param"? } }` with a 4xx or 5xx.
Platform ops (`std.space.*`) use the same shape.

## Raw HTTP

Every sign-in, renewal, sign-out and platform op has an SDK call. Raw
`fetch` is for a tool with no SDK at all (curl, another language); the
wire above is all it needs. Sign-in routes: `POST /auth/begin`,
`/auth/complete` (takes `display`), `/auth/refresh` (bearer, or the
renewal cookie), `/auth/logout`.

## Refusals

Every failure is an `OrdoRefusal` (from `@hominemai/ordo-sdk/core/refusal.js`,
also re-exported by `@hominemai/ordo-react`). Read `e.class`; show
`e.refusal.message` (the door's words; `e.message` adds the class in
front). Hooks give the same detail as `refusal` (`.class`, `.message`,
`.param`). A call that got no answer at all is an
`OrdoNetworkError` (`kind`: `timeout`, `unreachable`, `blocked`).

| Class | Means | Page does |
|---|---|---|
| `access_denied` | the door refused the caller: not a member, removed, op not allowed for the role, or no fresh touch | show the message; drop the tenant if it is gone |
| `not_found` | absent, or guarded from you | reload |
| `cas_miss` | someone changed the row first | reload, show the new state |
| `access_guard` | you may read the row but not write it | show the message |
| `check_violation`, `bound_violation`, `invalid_param`, `missing_param` | bad input | show the message by its `param` |
| `duplicate` | a `unique` value is taken | show the message |
| `server_busy`, `serialization_conflict`, `tenant_not_ready` | transient | the client already retried; show the message |

`e.disposition` sums it up: `conflict`, `restart`, `retry`, `terminal`,
`request`, `denied` or `fault`.

# SDK signatures

From `clients/ordo-react/src` and `clients/ordo-sdk/src`. Every
export an app touches. `readonly` is left off the
fields (all are read-only). `?` marks an optional field.

Import paths:

- `@hominemai/ordo-react`: everything in this package, from the root only.
- `@hominemai/ordo-sdk/<folder>/<file>.js`: one file of the SDK. Prefer
  these subpaths. The SDK root also exports all of it.
- `./generated/ops.js`, `./generated/ids.js`: your app's, from
  `ordo gen ts <dir> --app <src>`.

## Shared types

```ts skip
// @hominemai/ordo-sdk/core/client.js (Row, Params, OpResponse, TokenSource also from ordo-react)
type Row = Record<string, unknown>;
type JsonValue = string | number | boolean | null | JsonValue[] | { [k: string]: JsonValue };
type ParamValue = JsonValue;
type Params = Readonly<Record<string, ParamValue>>;
type TokenSource = string | (() => string | Promise<string>);
interface OpResponse<R = Row> {
  op: string; version: string; mode?: string;
  rows: readonly R[];
  next?: string;              // present iff another page exists; absence is the only end
  servedVersion: string | null; stale: boolean; replayed: boolean; pin: Pin | null;
}
```

## @hominemai/ordo-react

```ts skip
parseInterface(raw: unknown): OrdoInterface        // throws SchemaVersionError | MalformedDescriptorError
withPlatformOps(iface: OrdoInterface, ...names: PlatformOpName[]): OrdoInterface

new DescriptorClient(options: DescriptorClientOptions)
interface DescriptorClientOptions {
  baseUrl: string; space: string; iface: OrdoInterface;
  token?: TokenSource;
  renewToken?: () => Promise<boolean>;   // used by live streams only
  fetch?: typeof fetch; callTimeoutMs?: number;
  busyRetry?: BusyRetryOptions; busyRetries?: number;
  conflictRetry?: ConflictRetryOptions; conflictRetries?: number;
}
client.withSpace(space: string): DescriptorClient
client.fetchOp<R = Row>(name: string, o?: DescriptorCallOptions): Promise<OpResponse<R>>
client.fetchPage<R = Row>(name: string, o?: DescriptorCallOptions): Promise<OpResponse<R>>
client.pageAll<R = Row>(name: string, o?: DescriptorCallOptions): AsyncGenerator<OpResponse<R>>
client.commit<R = Row>(name: string, o?: DescriptorCallOptions): Promise<OpResponse<R>>
interface DescriptorCallOptions { params?: Params; page?: string; refine?: Refine; idempotencyKey?: string; signal?: AbortSignal; onRetry?: OnRetry }
// ordo-react has no `OrdoClient`: that name is only the SDK's generated-op client.

<OrdoProvider client={DescriptorClient} live?={boolean | { paceMs?: number }} device?={DeviceSource}
              stepUp?={() => Promise<void>}>{children}</OrdoProvider>
useOrdo(): DescriptorClient
useInterface(): OrdoInterface

useOpQuery<R = Row>(op: string | OpRef<R, F>, o?: UseOpQueryOptions): UseOpQuery<R>
useOpQuery<N extends PlatformOpName>(op: N, o?): UseOpQuery<PlatformRows[N]>
interface UseOpQueryOptions {
  params?: Params; enabled?: boolean;         // enabled default true
  refine?: Refine | ((fields: F) => Conditions);
  pick?: readonly (keyof R)[];
  live?: boolean; livePaceMs?: number; device?: boolean;
}
interface UseOpQuery<R> {
  rows: readonly R[]; row: R | undefined; singleRow: boolean;
  loading: boolean; refusal: Refusal | null; revoked: boolean;
  stale: boolean; retrying: boolean; refetch: () => void;
  refinement: RefinementSurface; source: 'network' | 'device';
  complete: boolean; live: LiveStatus | null;
  deviceFallback: DeviceFallback | null; deviceFallbackReason: string | null;
}

useOpPages<R = Row>(op: string | OpRef<R, F>, o?: UseOpQueryOptions & { autoFirstPage?: boolean }): UseOpPages<R>
interface UseOpPages<R> {
  rows: readonly R[]; hasMore: boolean; empty: boolean; loading: boolean;
  refusal: Refusal | null; revoked: boolean; stale: boolean;
  more: () => void; restart: () => void; exhaust: (maxPages?: number) => void;
}

useCommit<R = Row>(op: string, o?: { autoResolve?: boolean }): UseCommit<R>
useCommit<N extends PlatformOpName>(op: N, o?): UseCommit<PlatformRows[N]>
interface UseCommit<R> {
  commit: (values: Params, rev?: number) => Promise<CommitState<R>>;
  resolve: () => Promise<Row | null>;            // after a conflict: the row now, or null
  retry: (values: Params, rev: number) => Promise<CommitState<R>>;
  reset: () => void;
  state: CommitState<R>; committing: boolean;
  refusal: Refusal | null; conflict: CasConflict | null;
  cas: boolean;                                   // the op declares `rev`
  model: FormModel; refusals: readonly string[];
}
type CommitState<R> =
  | { status: 'idle' } | { status: 'committing' }
  | { status: 'committed'; response: OpResponse<R> }
  | { status: 'conflict'; conflict: CasConflict; current: Row | null;
      currentIs: 'not_fetched' | 'fetching' | 'found' | 'deleted' | 'unavailable' }
  | { status: 'refused'; refusal: Refusal };
interface CasConflict { kind: 'cas_miss'; refusal: Refusal; attempted: Params; sentRev: number | null }

useFunction<R>(name: string): { call(params?, o?): Promise<R | null>; busy: boolean; error: Error | null }

useOpForm(op: string, initial?: FormValues): UseOpForm
type FormValues = Readonly<Record<string, ParamValue | undefined>>;
interface UseOpForm {
  model: FormModel; fields: readonly FormField[]; visible: readonly FormField[];
  values: FormValues;
  setValue: (name: string, value: ParamValue | undefined) => void;
  setValues: (next: FormValues) => void; reset: () => void;
  issues: readonly FormIssue[]; issueFor: (name: string) => FormIssue | undefined;
  valid: boolean;
  params: Record<string, ParamValue>;            // ready for commit(); drops undefined and null
  searchCandidates: (field: string, limit?: number) => Promise<readonly Row[]>;
}
interface FormField {
  name: string; type: OrdoType; required: boolean;
  max: string | null; min: string | null;        // digit strings
  kind: 'scalar' | 'reference' | 'enum' | 'rev' | 'json';
  options: readonly string[] | null;             // enum
  candidates: CandidateSource | null;            // reference
  hidden: boolean;                               // true for rev
}
interface FormIssue { class: 'missing_param' | 'invalid_param' | 'bound_violation'; message: string; param: string | null }

opRef<R, F = AnyFields>(name: string): OpRef<R, F> // an op name carrying its row type

useOrdoAuth(auth: OrdoAuth): { phase: ResumePhase; session; error; retry(): void; auth }
// also exports the SDK's createOrdoAuth and its types

useResumedSession<S>(resume: () => Promise<S | null>): ResumedSession<S>
interface ResumedSession<S> {
  phase: 'checking' | 'signed-in' | 'signed-out' | 'failed';
  session: S | null; error: unknown;
  retry: () => void; set: (session: S | null) => void;
}

useTenantStatus(o: { baseUrl: string; token: TokenSource; enabled?: boolean; fetch?: typeof fetch }): TenantStatusResult
interface TenantStatusResult {
  phase: 'checking' | 'setting-up' | 'stuck' | 'ready' | 'failed';
  status: TenantStatus | null; space: string | null;
  problem: TenantStatusError | null;   // .problem: 'refused' | 'limit_reached' | 'malformed' | 'unavailable'
  retry(): Promise<void>;
}
<TenantGate baseUrl token settingUp?={slot} stuck?={slot} failed?={slot}>
  {(space: string) => ReactNode}
</TenantGate>                          // slot: ReactNode | ((t: TenantStatusResult) => ReactNode)

useRow(id: string | null | undefined): { row: RowKnowledge; menu: ActionMenu; bind: (op: string) => Record<string, string> | null }
useFileUpload(): { phase; progress; error; upload(target: { entity; field; id }, file: Blob): Promise<void>; cancel(); reset() }
useFileDownload(): { phase; progress; error; download(handle: string, name: string): Promise<void> }
useFile(handle: string | null | undefined): { phase; url: string | null; type: string | null; size: number | null; progress; error }
useOwnDownload(): { download: () => Promise<{ blob: Blob; name: string } | null>; busy: boolean; error: Error | null }
useAccountEmail(o: { baseUrl; token: TokenSource; enabled? }): { email; pending; busy; problem; needed; begin(address); confirm(code); cancel(); remove(); refresh() }

// Refusals, re-exported from the SDK
class OrdoRefusal extends Error   // see below
type Refusal = RefusalDetail      // { class, code, message, op, param, status, declared }
```

## @hominemai/ordo-sdk: ops

```ts skip
// @hominemai/ordo-sdk/core/client.js
new OrdoClient(options: OrdoClientOptions)
interface OrdoClientOptions {
  baseUrl: string; space: string; token?: TokenSource;
  renewToken?: () => Promise<boolean>; fetch?: typeof fetch; callTimeoutMs?: number;
  busyRetry?: BusyRetryOptions; busyRetries?: number;          // default 2 retries
  conflictRetry?: ConflictRetryOptions; conflictRetries?: number; // default 3 retries
}
client.withSpace(space: string): OrdoClient
client.space: string
client.callFunction<R>(name: string, params?: Params, o?): Promise<{ result: R; replayed: boolean }>

// ./generated/ops.js: one function per op
opName(client: OrdoClient, params: OpNameParams): ReadCall<OpNameRow, OpNameRefusal, OpNameFields>
//   ReadCall:       .where(f => cond | cond[]) .pick('a', 'b')
//                   .fetch(o?): Promise<Page<R>>  .pages(o?): AsyncGenerator<Page<R>>  .rows(o?): Promise<R[]>
//   SingleReadCall: .where() .pick() .fetch(o?): Promise<Page<R>>  .row(o?): Promise<R | null>
//   WriteCall:      .commit(o?): Promise<OpResponse<Row>>
// interface Page<R> extends OpResponse<R> { hasMore: boolean }
// o: { page?: string; idempotencyKey?: string; signal?: AbortSignal; onRetry?: OnRetry }
// Field conditions: f.x.eq(v) .ne(v) .lt(v) .le(v) .gt(v) .ge(v) .in([v]) .isNull()
//   (a sealed field: .eq and .in only). Several conditions are AND.

// @hominemai/ordo-sdk/core/brand.js
asId<B extends string>(value: string): Branded<string, B>

// @hominemai/ordo-sdk/core/refusal.js
class OrdoRefusal<C = RefusalClass> extends Error {
  class: C; code: string; status: number;
  refusal: RefusalDetail<C>;          // same object as .detail
  retryable: boolean; retryAfterMs: number | null;
  disposition: 'conflict' | 'restart' | 'retry' | 'terminal' | 'request' | 'denied' | 'fault';
}
interface RefusalDetail<C> { class: C; code: string; message: string; op: string | null; param: string | null; status: number; declared: boolean }
class OrdoLocalRefusal extends OrdoRefusal { local: true }   // refused before the wire

// @hominemai/ordo-sdk/core/door.js
refusalOf(response: Response, op: string, declared?: readonly string[]): Promise<OrdoRefusal>

// @hominemai/ordo-sdk/core/network.js
class OrdoNetworkError extends Error { kind: 'timeout' | 'unreachable' | 'blocked' }
```

## @hominemai/ordo-sdk: sign-in

`door` is `PasskeyEndpoint`. Sign-in calls throw `PasskeyError`;
tenant calls throw `TenantError`. Both extend `OrdoRefusal` and add
`.failure`.

```ts skip
// @hominemai/ordo-sdk/auth/session.js (also from @hominemai/ordo-react)
createOrdoAuth(o: PasskeyEndpoint & { renewBeforeS?: number; autoRenew?: boolean }): OrdoAuth
interface OrdoAuth {
  session(): PasskeySession | null;
  token(): Promise<string>;          // renews near expiry; signed out: PasskeyError 'refused'
  renew(): Promise<boolean>;         // false: the session ended
  resume(o?: ResumeOptions): Promise<PasskeySession | null>;
  beginEmail(start: EmailSignInStart): Promise<EmailCodeSent>;
  completeEmail(sent: EmailCodeSent, code: string, o?: EmailComplete): Promise<SignInResult>;
  signInWithPasskey(o?: SignInOptions): Promise<PasskeySession>;
  registerPasskey(o?: RegisterOptions): Promise<PasskeySession>;
  stepUp(): Promise<PasskeySession>;
  use(session: PasskeySession | null): void;   // a session from another ceremony
  signOut(): Promise<void>;          // not recorded: PasskeyError 'unavailable'
  spaces(): Promise<readonly ListedSpace[]>;
  waitForTenant(o?: WaitForTenantOptions): Promise<string>;
  clientOptions(): { baseUrl; token; renewToken };   // spread into a client
  subscribe(l: (s: PasskeySession | null) => void): () => void;
}
refreshSession(door, token: string): Promise<PasskeySession>   // POST /auth/refresh
signOut(door, token: string): Promise<void>                   // POST /auth/logout

// @hominemai/ordo-sdk/auth/passkey.js
interface PasskeyEndpoint { baseUrl: string; fetch?: typeof fetch; callTimeoutMs?: number; busyRetry?: BusyRetryOptions }
interface PasskeySession {
  token: string;
  expiresAt: number;            // seconds since the epoch
  sessionExpiresAt: number;     // when renewing stops working
  admission?: { space: string; outcome: string };   // 'admitted' | 'already-member' | 'refused'
  notices?: readonly string[]; device?: string;
  raw: Readonly<Record<string, unknown>>;
}
interface SignInOptions { space?: string; invitation?: string; human?: string }
interface RegisterOptions { display?: string; space?: string; invitation?: string; ticket?: string; human?: string }
registerPasskey(door, o?: RegisterOptions) | signInWithPasskey(door, o?: SignInOptions): Promise<PasskeySession>
stepUp(door, token: string): Promise<PasskeySession>
class PasskeyError extends OrdoRefusal {
  failure: 'unsupported' | 'cancelled' | 'refused' | 'unavailable' | 'malformed'
         | 'reauthentication_required' | 'email_unavailable' | 'too_many_attempts' | ...;
  setting?: string; value?: string; retryAfter?: number;
}

// @hominemai/ordo-sdk/auth/email.js
interface EmailSignInStart { address: string; space?: string; invitation?: string; device?: string; human?: string }
interface EmailCodeSent { challenge: string; binder: string }
beginEmailSignIn(door, start: EmailSignInStart): Promise<EmailCodeSent>
interface EmailComplete extends SignInOptions { display?: string }   // display: a new person's name
completeEmailSignIn(door, sent: EmailCodeSent, code: string, o?: EmailComplete): Promise<SignInResult>

// @hominemai/ordo-sdk/auth/second-step.js
type SignInResult =
  | { kind: 'session'; session: PasskeySession }
  | { kind: 'second-step'; step: SecondStep };
signInWithPassword(door, c: { identifier: string; password: string }, o?: SignInOptions): Promise<SignInResult>
completeSecondStep(door, step: SecondStep, answer:
  { kind: 'totp'; code: string } | { kind: 'backup-code'; code: string } | { kind: 'security-key' }
): Promise<{ kind: 'session'; session: PasskeySession; via: string } | { kind: 'proof'; ... }>

// @hominemai/ordo-sdk/auth/methods.js
signInMethods(door, o?: { tenant?: string }): Promise<{
  passkey: boolean; password: boolean; email: boolean; guest: boolean;
  humanCheck: { kind: 'turnstile'; siteKey?: string } | null;
  connections: readonly { name: string; label: string; kind: 'oauth2' | 'oidc'; tenant: boolean }[];
}>

// @hominemai/ordo-sdk/auth/resume.js
resumeSession(door, o?: { onMiss?: (why: 'none' | 'remember_off', message?: string) => void }): Promise<PasskeySession | null>

// @hominemai/ordo-sdk/auth/provisioning.js
tenantStatus(door, token: TokenSource): Promise<TenantStatus>
retryTenant(door, token: TokenSource): Promise<TenantStatus>
waitForTenant(door, token: TokenSource, o?: { onProgress?; signal?: AbortSignal; timeoutMs?: number }): Promise<string>  // the space id
interface TenantStatus { state: 'off' | 'none' | 'provisioning' | 'ready' | 'failed'; space?: string; step?: string; reason?: string; setting?: string; stuck?: boolean }
class TenantError extends OrdoRefusal { failure: 'failed' | 'off' | 'none' | 'timeout' | 'refused' | 'limit_reached' | 'aborted' }

// @hominemai/ordo-sdk/auth/share.js
listSpaces(door, token: string): Promise<readonly { space: string; name: string | null; role: string | null; via: 'member' | 'guest' }[]>
peekInvitation(door, ticket: string): Promise<{ space: string; role: string | null; ticketExpiresAt: number; named: boolean }>
declineInvitation(door, ticket: string, token?: string): Promise<void>

// @hominemai/ordo-sdk/auth/recovery.js
endEverySession(door, token: string): Promise<void>        // sign out everywhere; needs a fresh touch
```
