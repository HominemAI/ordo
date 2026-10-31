# Ordo Quickstart

You have Postgres and a terminal. This gets you from nothing to a running
app: one `.ordo` file, a local server that reloads on save, a call over
HTTP, a typed TypeScript client, and a real deployment. Every command and
every line of output below was run against the current `ordo` binary while
this page was written, so what you see here is real output.

Two words you need before anything else. A **space** is a tenant: one
database's worth of isolated rows, one key, one audit chain. An **op** is a
named, typed unit of work, either a read (a query) or a write (`set`/`del`
statements) that the platform serves over HTTP. You write ops in `.ordo`
files; the platform derives the HTTP route, the SQL, the client types and
the audit trail from them.

## 1. Write a `.ordo` file

Make a directory and put one file in it, `notes.ordo`:

```
domain notes @ 0.1.0

entity notes {
  title: text
  body:  text = ""
  done:  bool = false
}

op addNote($title: text, $body: text) =
  set notes { title: $title, body: $body }

op completeNote($id: id, $rev: int) =
  set notes { id: $id, rev: $rev, done: true }

op listNotes =
  notes[id, title, body, done]
  sort -id
```

That is a complete app: one shape, two writes, one read. Every field is
required, defaulted, or nullable, visibly, so there is no fourth state to
wonder about. `notes` carries no `acl`, so it has no row-level guard: this
is a single-tenant, single-user app, which is the right shape for a first
one. `definition-language.md` and `tutorial.md` are the full language;
`syntax-examples.md` is a denser reference once you know the shape of it.

Run `ordo check notes.ordo` any time to see what the compiler derived
without starting anything:

```
ENTITIES
  members  (id key: mem_)
    principal: text
    displayName: text
    kind: text
    memberCount: int
    role: text  [nullable]
    status: text
    joined: date
    guards: NONE — unguarded
  groupMembers  (id key: gm_)
    group: -> members owned
    member: -> members owned
    unique (group, member)
    guards: NONE — unguarded
  notes  (id key: note_)
    title: text
    body: text
    done: bool
    guards: NONE — unguarded

RELEASE
  no release rule: members cannot leave on their own (locked); see docs/howto-leave.md

OPS
  addNote
    write   mode creates
    reaches notes writes {body, title}
  completeNote
    write   mode converges
    reaches notes writes {done}
  listNotes
    class sql   rows unbounded (pageable)
    [id: id, title: text, body: text, done: bool]
    refinable: id, title, body, done (any operator)
    reaches notes.{body, done, title}
  std.notes.count
    class sql   rows <= 1
    [n: int]
    refinable: id, rev, title, body, done (any operator)
  std.notes.create
    write   mode creates
    reaches notes writes {body, done, title}
  std.notes.delete
    delete   mode deletes
    reaches notes (rows examined, no fields)
  std.notes.get
    class sql   rows <= 1
    [id: id, rev: int, title: text, body: text, done: bool]
    refinable: id, rev, title, body, done (any operator)
    reaches notes.{body, done, title}
  std.notes.list
    class sql   rows unbounded (pageable)
    [id: id, rev: int, title: text, body: text, done: bool]
    refinable: id, rev, title, body, done (any operator)
    reaches notes.{body, done, title}
  std.notes.update
    write   mode converges
    reaches notes writes {body, done, title}
```

`members` and `groupMembers` are not yours: every space has them (§2).
`RELEASE` says whether a member may leave on their own; with no rule they
may not (`howto-leave.md`). `refinable` lists the outputs a caller may
filter on with `.where()` and narrow with `.pick()` (`howto-cost.md`).

The `std.*` ops are not something you wrote. The moment a shape exists, the
platform derives get-by-id, a paged list, create, a full-row CAS update, a
delete and a count for it (`std-library.md`). They are registered
but not exposed until you say so (`std-library.md` has the switch).

## 2. Run the dev loop

```
ordo dev .
```

`ordo dev` brings up its own disposable Postgres cluster (it needs the
Postgres binaries on your path; the data directory is `.ordo/devdb` beside
your sources, and it connects to `ORDO_PG_*` instead if you set them),
compiles your `.ordo` set, creates the schema, and serves on
`127.0.0.1:4000`. It reads the sign-in and CORS settings too when you set
them (`howto-sign-in.md`); with none set, it has no login and prints the
token below. It runs the exact same boot, load gate and hot-swap path a
production `ordo-host` process runs, over a program that came from your
directory instead of the artifact registry.

First run, real output (long runs of `DISABLED` and `worker` lines are cut,
and the path and port are yours):

```
ordo dev  watching /path/to/notes
  database    ordo dev's own cluster  ordo@/var/folders/.../ordo-dev-7d13070415cf:52004/ordo_dev
saved  /path/to/notes ✓ 32ms
       read 8 · compile 2 · delta 5 (3 changes) · reset 16 (no seed.sql, empty)
schema created — the database held none of this program's tables, so the loop created and seeded it:
A1 — new entity `members`
A1 — new entity `groupMembers`
A1 — new entity `notes`
  boot        gate + swap in 150ms
ordo-host 0.1.0  addr=127.0.0.1:4000  space=dev
  database    ordo@/var/folders/.../ordo-dev-7d13070415cf:52004/ordo_dev  pool=4  acquire=5000ms  tls=off
  custody     test (UNSAFE — repository constant)  epoch=1
  system      system  (created at boot)
  continuity  system: intact
  continuity  dev: intact
  program     dev-boot  ops=9  gate: 4 of 9 registered ops shape-verified against an empty shadow (5 have no result shape, 0 were undescribable, 0 live-schema drift)
  door        identity tokens; delegations: registry present
  auth        none (ORDO_AUTH_METHODS is not set)
  platform    std.space.callersOf, std.deployment.apply, ... std.deployment.resumeTenant
  pausing     pause: nobody in a tenant   resume: nobody in a tenant   (the deployment owner always may)   answers set for: no reason
  leaving     a member who leaves keeps their name (the default)
  accounts    a deleted account's data is kept (the default)
  ...
  token       Bearer djR8bWVtX2Rldnw5MjIzMzcyMDM2ODU0Nzc1ODA3fHx8fHw.fGAgYA3BxsbYLg-vJsWOcmejxW5PfsJZkF_jSDIDrNg
              local only — signed by a key that dies with this process
  try it      curl -H "authorization: Bearer djR8bWVt…" -H "x-ordo-space: dev" http://127.0.0.1:4000/op/<yourOp>
```

The `reset` line ends `(seeded)` instead when a `seed.sql` sits beside your
sources, as in the next section. `pausing`, `leaving` and `accounts` name the
platform's defaults for pausing a tenant (`howto-pause.md`), leaving one
(`howto-leave.md`) and deleting an account. A `DISABLED` line is a feature
that needs a setting you have not made, and the line says which.

`members` and `groupMembers` appear even though you never declared them:
they are platform-provided on every space, undeclarable, one row per
principal admitted to the space. `system` is the platform's own space,
created once at boot so a deployment always has somewhere to keep what it
knows about itself; it is not a tenant and nothing you do touches it.

Every save after that recompiles and reprints the journal, with the phase
budget spelled out (`read`, `compile`, `delta`, `gate`, `swap`, each with
its own denominator) so a slow save is diagnosable.
Edit the `.ordo` file and save, and the loop reacts within the same process
with no restart. §6 shows what a schema-changing save prints. `plan-dev-loop.md`
is the design doc behind the loop if you want the full mechanics: its own
Postgres lifecycle, what counts as a resettable change versus an additive
one, and the `--gen` seam for driving a codegen step off every save.

### Becoming a member

Every call to the door, including a read of an entity with no `acl` at
all, needs a caller who holds a `members` row in the space: membership and
row-level guards are separate layers, and membership is the outer one.
`ordo dev` seeds one for you, on every full reset, before it prints
anything else: a principal named `mem_dev`, an `owner` of the `dev` space,
the same `dev` your program always serves in. The `token` line in the
transcript above is a bearer token minted for exactly that principal.
Copy it and there is nothing left to set up.

That token is signed under a key generated fresh for this process and
dies with it; the row it names lives only in the database this loop just
reset. Neither travels, on purpose. See **Where this is thin** for what
that means for a real deployment. A script that wants a token good across
restarts can set `ORDO_TOKEN_KEY` (64 hex characters, `openssl rand -hex
32`) before starting the loop; `ordo dev` signs against that instead of
the ephemeral key.

Want a second test principal, a different role, or a name of your own
choosing? Put a row in `seed.sql` beside your `.ordo` sources, the same
way you seed any other data. The loop runs it verbatim on every full
reset, after its own `mem_dev` row, so nothing you add there can collide
with it:

```sql
INSERT INTO "members" ("$space", "id", "principal", "displayName", "kind", "memberCount", "role", "status", "joined") VALUES
  ('dev', 'mem_1', 'mem_1', 'Dev', 'principal', 1, 'owner', 'active', DATE '2026-01-01');
```

The `principal` column is the name a token carries, so a row you want to
call as has the same value in `id` and `principal` here. `ordo dev` always
binds the space `dev`, so that literal string is not a placeholder. Add `--fresh` once after creating or changing `seed.sql` so
the loop rebuilds the database and reseeds it.

## 3. Call an op

Every op gets one HTTP route, mirrored from what you wrote: a read is
`GET /op/{name}`, a write is `POST /op/{name}`, a pure delete is
`DELETE /op/{name}`. Every call carries the space as a header
(`X-Ordo-Space`) and the caller as a bearer token
(`Authorization: Bearer <token>`); a write's parameters are a JSON body, a
read's are a query string. There is no per-op route to write.

With no token at all, the door refuses, and the message is deliberately the
same one an unknown op name would get, so a guess reveals nothing (real
response, captured against a running loop):

```
$ curl -s -X POST http://127.0.0.1:4000/op/addNote \
    -H 'x-ordo-space: dev' -H 'content-type: application/json' \
    -d '{"title":"buy milk","body":""}'

{"error":{"code":"access_denied","message":"missing bearer token","op":"addNote"}}
```

With the token `ordo dev` printed at startup (`mem_dev`, the principal it
seeded for you, §2), the same call answers:

```
$ curl -s -X POST http://127.0.0.1:4000/op/addNote \
    -H 'x-ordo-space: dev' -H 'authorization: Bearer <token>' \
    -H 'content-type: application/json' \
    -d '{"title":"buy milk","body":""}'

{"mode":"creates","op":"addNote","rows":[{"created":"note_w01a113759754ad69ce3797119160_000000000001","entity":"notes"}],"version":"3e544c3d35a83162"}

$ curl -s http://127.0.0.1:4000/op/listNotes -H 'x-ordo-space: dev' -H 'authorization: Bearer <token>'

{"op":"listNotes","rows":[{"body":"","done":false,"id":"note_w01a113759754ad69ce3797119160_000000000001","title":"buy milk"}],"version":"6ac861032e9cc421"}
```

`<token>` here is exactly what `ordo dev` printed at boot. Paste it as
is, nothing to derive. Step 5 covers where a token comes from against a
real deployment. That is a sign-in. A real deployment's token has to outlive
this process and answer for a principal a login proved.

## 4. Generate the typed client

```
ordo gen ts .
```

This reads your compiled program and (re)writes the generated half of two
packages under `clients/`: `@hominemai/ordo-sdk`, a typed function per op with its
params, its row shape and its declared refusal union, and `@hominemai/ordo-cdk`, a
typed builder for `.ordo` itself. Neither package's generated half is ever
hand-edited; regenerate after every schema change instead. `--out <dir>`
targets a different root; the ordinary case is your repository root, run
from wherever your `.ordo` sources live.

What comes out, for `listNotes` (real, from `clients/ordo-sdk/src/generated/ops.ts`):

```ts
export interface ListNotesRow {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly body: string;
  /** `bool` */
  readonly done: boolean;
}
export function listNotes(client: OrdoClient): ReadCall<ListNotesRow, ListNotesRefusal> { ... }
```

A read can also be refined: `listNotes(client).where(...).pick(...)` filters
and narrows it on the server (`howto-cost.md`).

Using it:

```ts
import { OrdoClient, addNote, listNotes, OrdoRefusal } from '@hominemai/ordo-sdk';

const client = new OrdoClient({
  baseUrl: 'http://127.0.0.1:4000',
  space: 'dev',
  token: () => myToken,
});

await addNote(client, { title: 'buy milk', body: '' }).commit();

const page = await listNotes(client).fetch();
for (const n of page.rows) console.log(n.title, n.done);
```

**`@hominemai/ordo-sdk`** gives you compile-time enforcement: a read is a `ReadCall`
(`.fetch()`/`.pages()`), a write is a `WriteCall` (`.commit()`, no read
verbs), ids are branded per entity so a `NotesId` and a `CustomersId`
cannot be swapped, refusals are the op's own declared union, and money is
an exact decimal string. Every mutation in your codebase is `grep '.commit('`. A call the server
turns away as busy (`server_busy`) is sent again by the client, twice by
default; the `busyRetry` option sets that, and the React hooks use the same
client (`clients/ordo-sdk/README.md`). A write that lost a race with other
writes to the same rows (`serialization_conflict`) is sent again three times
by default, under `conflictRetry`, with the same idempotency key. Both clients name every refusal class
the door can send, so a `switch` over `refusal.class` is checked.

**`@hominemai/ordo-react`** is the headless runtime behind Ordo's
generated UI (`ui-gen.md`). It reads `interface.json` at run time and
renders nothing itself. `useOpQuery('listNotes')`
gives you `rows`, `loading` and `refusal`; `useCommit('addNote')` gives you
a CAS-aware write state machine (`idle` → `committing` → `committed` |
`conflict` | `refused`). Wrap your app in
`<OrdoProvider client={client}>` and the hooks read it from context. A
skin maps these hooks onto real components; none ships in this package.

## 5. Deploy

A running deployment takes a program through its own door (`ordo apply
--host`, `docs/deploying.md` §7). The first program has no door to go
through yet, so the first apply goes straight to the deployment's database.
Both run the same code and print the same report.

**Custody, once per deployment.** Mint a master key and seal it under a
passphrase file (never a flag: flags land in shell history and `ps`):

```
ordo keys seal-master --out master.sealed --passphrase-file pass.txt
```

It prints `sealed`, a `key-id` and a fingerprint. Back the file and the
passphrase up apart from each other; there is no recovery.

**Bring the binary up on the system space.** Point `ORDO_PG_*` at an empty
database. This is the deployment's own database, the control plane, and no
tenant is ever put in it. Set the sealed pair, `ORDO_TOKEN_KEY`,
`ORDO_PAGE_KEY`, and `ORDO_SPACE=system`, and start `ordo-host`. It creates
the platform's own tables and the `system` space and serves with no program
yet; `/health/live` answers and the log prints
`system      system  (created at boot)`.

**Apply your program.** `ORDO_PG_*` still names the deployment's database,
so no `--db` is needed:

```
ordo apply ./notes --as you
```

First apply against an empty database creates the whole schema in one
transaction, stores the compiled artifact under its content hash, and
activates it (real output for the notes program):

```
schema     created  (3 table(s) on an empty target; `$migrations` #1; event recorded on the deployment stream)
stored     e1f5abaca501fa8c
activated  e1f5abaca501fa8c  (was nothing)
nodes      nudged on `ordo.deploy`
```

**Every tenant gets its own database.** There is no setting for it and no
single-database mode. A tenant made through the door (`ordo space create`,
or a sign-up) gets a database of its own on the default tenant cluster:
`ORDO_PG_TENANT_HOST`, `_PORT` and `_USER`, each falling back to the
deployment's own. Set none of them and each tenant's database lands on the
same Postgres server as the deployment's. A credentials directory is
optional, needed only for a cluster dialled under a label of its own. The
catch-up worker brings each tenant's database forward to the program the
node serves. To mint custody for a tenant from the host instead:

```
ordo keys init --space mytenant --as you
```

Point `ORDO_SPACE` at `mytenant` and restart if you want that tenant to be
this deployment's home, or run a second replica against it.
`docs/deploying.md` §2d and §4c have the cluster levels and the full
first-deployment sequence, and what each refusal along the way means.

**Run the image:**

```
docker run -d \
  -e ORDO_PG_HOST=... -e ORDO_PG_USER=... -e ORDO_PG_DB=... \
  -e ORDO_SPACE=system -e ORDO_KEY_PROVIDER=sealed-master \
  -e ORDO_MASTER_KEY_FILE=/run/keys/master.key \
  -e ORDO_MASTER_KEY_PASSPHRASE_FILE=/run/secrets/master-pass \
  -e ORDO_TOKEN_KEY=... -e ORDO_PAGE_KEY=... \
  -p 8080:8080 \
  ghcr.io/hominemai/ordo/ordo-host@sha256:<digest>
```

`docs/deploying.md` §1's table is the full list, with a refuse-to-boot or
refuse-the-feature posture for every one of them; nothing here defaults
silently.

**Point a client at it.** Set `ORDO_AUTH_METHODS=password` on the server
and it exposes `/auth/begin` and `/auth/complete`; your app still has to
call `LocalPasswordProvider::enroll` somewhere to turn a stranger into an
account (there is no signup endpoint), but once a credential exists,
`POST /auth/complete` hands back the same shape of bearer token step 3
used, real this time. For passkeys instead, see `howto-sign-in.md`.

```ts
const client = new OrdoClient({
  baseUrl: 'https://api.example.com',
  space: 'mytenant',
  token: () => auth.accessToken(),
});
```

## 6. Change the schema

Add a field, save, and `ordo dev` shows you the delta immediately, applied
live with no reset because it is additive (real output, adding
`priority: text?` to `notes`):

```
       watch 43 · read 8 · compile 1 · delta 9 (1 change) · apply 0 (1 statement) · gate 62 (4 reads) · swap 16 (installed)
schema changed — applied through the production class-A path (plan-dev-loop.md §4.3); your rows are still there:
A2 — new optional field `notes.priority`
```

Against a deployed database, the same change is three commands. `--db`
names the deployment's database (the one `ORDO_PG_DB` points at):

```
ordo plan ./notes --against mydb           # read-only: what would move, classified and probed
ordo migrate ./notes --apply --db mydb --as you
ordo apply ./notes --db mydb --as you      # store and activate the new artifact
```

`plan` lists `A2  new optional field notes.priority` as class A, `migrate`
runs `ALTER TABLE "notes" ADD COLUMN "priority" text` in one transaction, and
`apply` prints `activated ... (was <old hash>)`. Tenant databases follow by
themselves (the catch-up worker, `docs/deploying.md` §1).

An additive change (a new entity, a new optional or constant-defaulted
field, a new index) applies in one transaction with no downtime; the live
program keeps serving the whole time, because the old artifact tolerates
the extra column and the new one only starts reading it once activated.
`docs/howto-operating.md` has the full command output for every refusal
class this can hit.

## 7. Where to go next

- **`docs/deploying.md`**: the deployment owner's reference, every environment
  variable and its refusal posture, images, custody, the system space, the
  deployment engine.
- **`docs/howto-operating.md`**: applying, rolling back, hot swap, health
  endpoints, day-to-day commands, with real output for every
  outcome.
- **`docs/howto-sign-in.md`**: passkey sign-in and a React page with live
  data, on `ordo dev`.
- **`docs/howto-pause.md`** and **`docs/howto-leave.md`**: pausing a tenant,
  and members leaving a tenant or deleting an account.
- **`docs/definition-language.md`**: the whole language, settled and
  numbered by rule.
- **`docs/tutorial.md`**: two build-alongs, one with a single user, one
  with `acl` and other people in the space; read it once you need guards.
- **`docs/std-library.md`**: what the platform derives for you from a
  shape alone, and the two honesty rules about exposure.
- **`docs/ui-gen.md`**: how a registered op becomes a generated screen.
- **`docs/howto-custody.md`**: sealed fields and blind indexes, for a field
  that needs to stay unreadable to the database itself.
- **`docs/migration.md`** and **`docs/plan-schema-evolution.md`**: bringing
  existing data in, and the full schema-change taxonomy beyond class A.
- **`docs/glossary.md`** and **`docs/traps.md`**: terms, and the specific
  mistakes people have actually made.

## Where this is thin

All the limits, in one place:

- **Authentication needs configuration, for a real deployment.**
  `ORDO_AUTH_METHODS=password` turns on real login, but your app still
  builds the signup flow and password reset itself; neither exists on the
  platform. `ordo dev`'s own token (§2) is a different, narrower thing: it
  mints for `mem_dev`, the principal the loop seeds into its own database,
  under a key that is either generated fresh every run or, when
  `ORDO_TOKEN_KEY` is set, that key (either way local to this process), and
  no substitute for wiring `ORDO_AUTH_METHODS` into something real.
- **No full-text search.** `contains` on an open field runs, but there is
  no ranked or fuzzy text search anywhere in the language or the platform.
- **A schema change that is not additive is refused, by name.** A required
  field over existing rows, a type change, a drop, a backfill: the
  compiler and the migrate command name exactly which class the change is
  and what to do about it, but increments 3 through 5 of schema evolution
  (backfills, drops, blind-index backfill) are not built. Read the refusal
  before assuming the tool is wrong.

Next: `docs/howto-sign-in.md` adds passkey sign-in and a React page with live data.
