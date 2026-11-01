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
