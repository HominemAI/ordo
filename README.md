# Ordo

A closed, deterministic language and platform that compiles to Postgres.

Ordo minimizes your backend work and lets you focus on the creative and
functional parts of your application. Backends have been made over and over
again. Stop reinventing the wheel every time.

- No more preferred implementations and libraries competing for your attention.
- No more guesswork on security and best practices.

## Why a compiler for AI-written software?

Ordo is built to save tokens by reducing the work that needs to be
generated. Large backend definitions scattered throughout your code keep
getting loaded into context. Every app reuses the same components and core
implementations: the same framework, the same imports, the same core
structures.

Should you really be spending tokens writing the same thing every time,
correcting mistakes, and rereading all that context whenever your agent
finds an issue? Nope.

Declare your app with a concise SQL abstraction. Everything except the full
UI and your app's functional logic is derived deterministically from one
definition file:

- Backend infrastructure: the tables, the migrations and the SQL for each op.
- Reusable API operations and live feeds.
- A secure, verifiable ledger of every write.
- A security and permissions model.
- Built-in tenancy, with a database per tenant.
- Data sovereignty, from server-managed keys to keys held by the client,
  with tenant-owned databases and storage on the way.
- A typed client, an op index and an agent skill, so a coding agent looks
  up what the app may call in a few lines.

`ordo check` reports errors before anything runs.

You declare shapes (entities), queries, writes and routines in a grammar with
one spelling per concept. Every deployment is divided into **spaces**: one
world of data, membership, keys and history each, with a per-space **custody
dial** (from platform-managed encryption to full client-held keys) and a
hash-chained **command log** under every write. There is no free-form query text
at runtime and no raw UPDATE path. Everything registers at deploy time as a
named, versioned operation with a declared scope. The bet is that a grammar
rigid enough for an LLM to emit correctly or fail loudly is also the
right one for humans to trust.

## The whole pitch in about 70 lines

From [`docs/syntax-examples.md`](docs/syntax-examples.md) §9, the `plantcare`
capstone, a complete **single-user** micro-app. `ordo check` accepts every
line of it. (Multi-tenant is not much longer, but it is different: see
`tutorial.md` Part 2.)

```
domain plantcare @ 0.1.0

entity plants {
  name:      text
  room:      text?
  everyDays: duration(ms) = 7d               # watering cadence, a first-class duration
  notes:     text = ""     sealed
}

entity waterings {
  plant -> plants   owned
  at:    date
  by:    text = ""
}

entity alerts {                        # the routine's nags are ordinary rows
  key:      text unique                #   upsert-by-key lifecycle
  severity: text = "warning"
  title:    text
  status:   text = "active"
  raised:   date
}

op thirsty($today: date) unbounded =  # execution-relative date passed in (SDK fills)
  plants[name, room, everyDays]  waterings[at]
  sort -waterings.at
  take[plants.id] 1
  | (at <= $today - everyDays) or at is null   # never-watered has null at
  sort cond(at is null -> 0, 1), at            # nulls sort LAST always, so the
                                               #   never-watered plants need an
                                               #   explicit key to reach the top

op history($plant: id) =
  waterings(plant == $plant)[at, by]
  sort -at
  take 30

op water($plant: id, $by: text, $today: date) =
  set waterings { plant: $plant, by: $by, at: $today }
  set alerts(key == "thirsty:" + $plant and status == "active") { status: "resolved" }

op addPlant($name: text, $room: text, $everyDays: duration(ms)) =
  set plants { name: $name, room: $room, everyDays: $everyDays }

op pruneLogs($before: date) =            # maintenance op: prune old logs
  del waterings(at <= $before)

routine thirstCheck in plantcare {
  trigger every day 09:00Z
  run($today: date):
    plants[id, name, everyDays]  waterings[at]
    sort -waterings.at
    take[plants.id] 1
    | (at <= $today - everyDays) or at is null
    each ->
      set alerts { key: "thirsty:" + plants.id,
                   title: name + " needs water",
                   raised: $today,
                   status: "active" }   # converge touches only described fields:
                                        #   without this, a resolved alert never
                                        #   re-raises
}

# Deploy: `ordo check`, then `ordo apply` → API ops (thirsty, history, water,
# addPlant, pruneLogs), the daily check, auth, permissions and audit. The
# generated list/detail/form UI and push notifications are planned (`app`,
# `page`, `channel` and `route` are refused today).
#
# plantcare has one user, so it declares no guards. An app whose data a
# second person shares needs §8's authorization constructs (see
# `tutorial.md` Part 2).
```

## Current status

**Early, and it runs.** The language, the compiler and a server work today.
Some things in the design docs are not built yet. Each doc marks those in
place, and `docs/syntax-examples.md` marks unbuilt syntax with `# planned:`.

What runs today:

- **The compiler and CLI.** `ordo check`, `ordo plan`, `ordo apply` and
  `ordo dev` compile a `.ordo` file and deploy it to Postgres.
- **A server.** `ordo-host` serves the operations over HTTP, with spaces,
  the hash-chained command log, guards and `acl`, and sealed fields.
  Published images are available.
- **Sign-in for your app's users.** Passkeys, Discord and GitHub, and an
  emailed code. Recovery and share links are built.
- **Sign-in for the deployment itself.** A separate identity service
  (`crates/ordo-identity-service`, all 9 steps of
  [`docs/plan-identity-service.md`](docs/plan-identity-service.md) are built).
  It holds the people and machines that run a deployment, with passkeys,
  invite links, recovery codes and machine keys. It issues short signed
  tokens that the door and the console verify with its public key. The CLI
  signs in through it with `ordo login`. See
  [`docs/howto-identity.md`](docs/howto-identity.md).
- **Clients.** `@hominemai/ordo-sdk` (0.5.0) is a typed client generated from
  your program. `createOrdoAuth` is its one sign-in object, and
  `@hominemai/ordo-react` has hooks for live data, including `useOrdoAuth`.
- **Agent tooling.** `ordo skill install` writes the Ordo agent skill into a
  project. `ordo ops <dir>` looks up what an app may call.
  `ordo gen ts <dir> --app <src>` writes the client, the React hooks and an
  `OPS.md` index into your app.
- **A playground.** `crates/ordo-playground` compiles programs in the browser.

Being built for v1:

- **Backend functions.** Sandboxed calls run in WebAssembly inside the Ordo
  binary (wasmtime), with a fresh instance per call that starts in
  microseconds. A function reaches data only through ops and gets secrets
  as handles. The design is in
  [`docs/plan-backend-functions.md`](docs/plan-backend-functions.md).

What is planned, not built:

- Long-running backend services, deployed from the program (v2).
- A generated list, detail and form UI (`app` and `page` declarations).
- Push, SMS and in-app notifications (`channel` and `route` declarations).
- Aggregates over a sealed value, and a few syntax forms marked `# planned:`
  in [`docs/syntax-examples.md`](docs/syntax-examples.md).

## Features and progress

| Status | Meaning |
|---|---|
| ✅ Done | Built and tested |
| 🚧 In progress | Started, with work left |
| 📋 Planned | Designed or decided, not started |
| 💭 Open for thoughts | Still looking for thoughts on this before planning |

| Area | Feature | Status |
|---|---|---|
| Language | Entities, queries, writes and routines in one definition file | ✅ Done |
| Language | `ordo check`, `plan`, `apply` and `dev` | ✅ Done |
| Language | Registered, versioned operations with a declared scope | ✅ Done |
| Language | JSON fields and filtering | ✅ Done |
| Language | Schema changes across versions | 🚧 In progress |
| Language | Generated list, detail and form UI | 📋 Planned |
| Data | Spaces, and a database per tenant | ✅ Done |
| Data | Permissions from guards and `acl` | ✅ Done |
| Data | Hash-chained command log, checked by `ordo verify` | ✅ Done |
| Data | Sealed fields and a custody dial per space | ✅ Done |
| Data | Files, stored under the space's custody | ✅ Done |
| Data | Export and space deletion | ✅ Done |
| Data | Sharing: guests, time windows and share links | ✅ Done |
| Data | Tenants bring their own database and storage | 🚧 In progress |
| Data | Loading an existing app's data in | 🚧 In progress |
| Data | Importing from Supabase | 💭 Open for thoughts |
| Data | Client-first spaces, keys held only on devices | 📋 Planned |
| Data | Offline writes, and counts and groups on the device | 📋 Planned |
| Data | Full-text search | 💭 Open for thoughts |
| Data | Vector search | 💭 Open for thoughts |
| Data | Reading a field's past values | 💭 Open for thoughts |
| Data | Ordered lists and drag order | 💭 Open for thoughts |
| Sign-in | Passkeys, emailed codes, Discord, GitHub, Google and company logins | ✅ Done |
| Sign-in | Recovery codes, second factors, invitations and roles | ✅ Done |
| Sign-in | Delegation: an agent acting for a person | ✅ Done |
| Sign-in | One sign-in across an app's subdomains | ✅ Done |
| Sign-in | Tenants bring their own sign-in provider | ✅ Done |
| Sign-in | Identity service for the people who run a deployment | ✅ Done |
| Sign-in | Mapping a provider's groups to roles | 💭 Open for thoughts |
| Sign-in | Merging two existing accounts | 💭 Open for thoughts |
| Backend | Routines on a schedule and on changes | ✅ Done |
| Backend | Triggers that deliver events to other systems | 🚧 In progress |
| Backend | Public ops with a built-in cache | ✅ Done |
| Backend | Backend functions in WebAssembly (v1) | 📋 Planned |
| Backend | Long-running backend services (v2) | 📋 Planned |
| Backend | Push, SMS and in-app notifications | 💭 Open for thoughts |
| Clients | Typed TypeScript client generated from the program | ✅ Done |
| Clients | React hooks with live feeds | ✅ Done |
| Clients | A local cache on the device | ✅ Done |
| Clients | Every setting in the program and the CDK | 💭 Open for thoughts |
| Agents | Agent skill (`ordo skill install`) and op lookup (`ordo ops`) | ✅ Done |
| Agents | A VS Code extension | 💭 Open for thoughts |
| Operating | The console, and one image that runs everything | ✅ Done |
| Operating | Proxy mode and limits by visitor address | ✅ Done |
| Operating | Security event stream | 🚧 In progress |
| Operating | Backup and restore drill | 📋 Planned |
| Operating | Applying from a connected GitHub repository | 💭 Open for thoughts |

The board (`scripts/board.py`) and `docs/roadmap-checklist.md` hold the
detail behind each row.

Two claims made elsewhere in these docs have a narrower scope than they sound.
**"Sixty lines is an app"** is true for single-user apps. The multi-tenant
version needs `acl`, both access verbs and a bootstrap path (still short, not
sixty lines). **"One line derives your permission system"** derives
everything except the first access row, which the platform grants to the
row's creator, and the second, which you write as an ordinary `set` on the
derived link entity.

## Release blockers

Two things stand between this and a first release:

- **The backup and restore drill** has not been executed.
- **Published performance numbers.** The token benchmark is not a blocker
  (owner, 2026-10-30). It runs after release.

## Licensing

- The engine and server (`crates/`) are **AGPL-3.0-only** (`LICENSE`).
- The spec, conformance corpora and reference docs (`docs/`) and the client
  packages `clients/ordo-sdk`, `clients/ordo-react` and `clients/ordo-cdk`
  are **Apache-2.0** (`LICENSE-APACHE`).
- Everything else follows the default in `NOTICE`: AGPL-3.0-only.
- What Ordo generates from your definitions is yours. No licence obligation
  falls on it.

The full path map is in `NOTICE`. The reasoning is in
[`docs/licensing.md`](docs/licensing.md).

## How the language was designed

About 215 real SQL patterns were translated into Ordo, each with a verdict:
it translates directly, it is problematic and needed a ruling, or it is not
possible by design. Those verdicts
decided what the language omits. They are in
[`docs/sql-translation-corpus.md`](docs/sql-translation-corpus.md) and
[`docs/sql-translation-corpus-2.md`](docs/sql-translation-corpus-2.md).

## Quickstart

Running your own deployment? [`docs/first-deployment.md`](docs/first-deployment.md)
is the path from the secrets script to a signed-in console and a deployed
program.

To run something before you read anything, [`docs/quickstart.md`](docs/quickstart.md)
goes from an empty directory to a deployed app with one
`.ordo` file, `ordo dev`, a call over HTTP, the generated TypeScript client,
and a real deploy.

## Reading order

**Start here (learning the language):**

1. [`docs/glossary.md`](docs/glossary.md): the vocabulary (space, principal,
   op, sealed, custody, and the rest).
2. [`docs/tutorial.md`](docs/tutorial.md): **both parts.** Part 1 builds the
   capstone above, annotated line by line, every concept introduced where it
   first appears. Part 2 builds a small multi-tenant app: `acl`, guard
   inheritance, both access verbs, `$caller`, the day-one bootstrap, groups,
   guests. Read Part 2: the jump from one user to several
   is where real Ordo apps have gone wrong, including an LLM-written one that
   shipped a schema that looked guarded and was write-open.
3. [`docs/traps.md`](docs/traps.md): known mistakes and how to avoid them.
   The first three entries have each broken a real build.
4. [`docs/syntax-examples.md`](docs/syntax-examples.md): every construct,
   pure syntax: §8 is the authorization surface, §9 the capstone above.
5. [`docs/modeling-guidance.md`](docs/modeling-guidance.md): space granularity,
   the sharing chooser, reporting recipes, and authorization modeling. Two
   sections belong in a first read:
   **§C7**, comparing two independently grouped aggregates (the quorum /
   "fewer than half" shape: its absence cost two real builds their central
   feature), and **§D**, authorization modeling: which of the four layers a
   rule belongs in, the bootstrap pattern, and how routines are bounded. Read
   §D before your second app; read §C7 the first time you need to compare two
   counts.
6. [`docs/writing-data.md`](docs/writing-data.md): **changing** data:
   derived write modes (there is no `insert`/`update`/`delete`), the two
   guard checks and why their order is load-bearing, why creates prove
   their guard against a row that does not exist yet, what a command group
   records, and the closed error inventory.
7. [`docs/definition-language.md`](docs/definition-language.md): the full spec.

**Running one:** [`docs/deploying.md`](docs/deploying.md): pool sizing and
why it is your concurrency bound, page size and cursor binding, key custody
and what a `KeyProvider` must implement, credentials and the one-answer
rule, and the deploy-time registration gates.

**Building the engine:** [`docs/engine-plan.md`](docs/engine-plan.md) (gap
rulings + build plan), then [`docs/composition-conformance.md`](docs/composition-conformance.md)
and the SQL translation corpora.

**Platform design:** [`docs/implementation.md`](docs/implementation.md),
[`docs/security-logging-design.md`](docs/security-logging-design.md),
[`docs/api-engine-design.md`](docs/api-engine-design.md); then
[`docs/std-library.md`](docs/std-library.md) and [`docs/ui-gen.md`](docs/ui-gen.md) (planned, not built).

**Evaluating adoption:** [`docs/adoption-dx-plan.md`](docs/adoption-dx-plan.md)
§1 states the cost of adopting a new language; read that section first.
