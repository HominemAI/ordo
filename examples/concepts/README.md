# Ordo concepts, by example

Each section explains one concept in 2 or 3 lines, shows a small example (a
program excerpt, a command or an HTTP call), says what the concept guarantees,
and names the document to read next. `docs/glossary.md` is the master list of
terms. The syntax of the language itself is in [`examples/syntax`](../syntax).

The `.ordo` files here pass `ordo check`, the same way the syntax files do
(see "How to check" in the syntax README: the exit code is 0 even when an op
does not compile, so look for `DOES NOT COMPILE` too).

Sections marked **Proposed, not built** describe ruled design with no code yet.
They name the plan they come from.

| Concept | Where |
|---|---|
| [Deployment](#deployment) | `docs/deploying.md` |
| [The deployment's space (system space)](#the-deployments-space-system-space) | `docs/deploying.md` 4c |
| [Tenant](#tenant) | `docs/glossary.md` |
| [Member](#member) | [17-members-and-groups](../syntax/17-members-and-groups.ordo) |
| [Principal and person](#principal-and-person) | `docs/plan-enrolment.md` |
| [Deployment owner, app developer, tenant owner](#deployment-owner-app-developer-tenant-owner) | `docs/glossary.md` |
| [The door](#the-door) | `docs/api-engine-design.md` |
| [Tokens and capability tokens](#tokens-and-capability-tokens) | `docs/deploying.md` 5 |
| [Passkey sign-in and presence](#passkey-sign-in-and-presence) | `docs/plan-passkey-signin.md` |
| [Second factor](#second-factor) | `docs/plan-second-factor.md` |
| [Recovery codes and account recovery](#recovery-codes-and-account-recovery) | `docs/howto-operating.md` |
| [Ops as the API](#ops-as-the-api) | [01-ops-as-the-api](01-ops-as-the-api.ordo) |
| [Composition](#composition) | [syntax 09](../syntax/09-composition.ordo) |
| [Guards, acl and door roles](#guards-acl-and-door-roles) | [02-who-decides-what](02-who-decides-what.ordo) |
| [Groups](#groups) | [syntax 17](../syntax/17-members-and-groups.ordo), [08-groups-and-project-access](08-groups-and-project-access.ordo) |
| [The command log and its hash chain](#the-command-log-and-its-hash-chain) | [06-undo-and-history](06-undo-and-history.ordo) |
| [Undo and restore](#undo-and-restore) | [06-undo-and-history](06-undo-and-history.ordo) |
| [Sealed fields and custody](#sealed-fields-and-custody) | [03-sealed-fields-and-custody](03-sealed-fields-and-custody.ordo) |
| [Invitations, enrolment, provisioning on sign-up](#invitations-enrolment-provisioning-on-sign-up) | [07-team-live-and-device-copy](07-team-live-and-device-copy.ordo) |
| [Share links and guests](#share-links-and-guests) | [04-sharing-a-board](04-sharing-a-board.ordo) |
| [Live updates and the device copy](#live-updates-and-the-device-copy) | [07-team-live-and-device-copy](07-team-live-and-device-copy.ordo) |
| [Versions and deploys](#versions-and-deploys) | [05-version-1](05-version-1.ordo), [05-version-2](05-version-2.ordo) |
| [Settings and their layers](#settings-and-their-layers) | `docs/settings.md` |
| [The security event stream](#the-security-event-stream) | `docs/howto-audit.md` |
| [The standard library, cost classes, step-up](#the-standard-library-cost-classes-step-up) | `docs/std-library.md` |
| [Domains, apps and clients](#domains-apps-and-clients) | `docs/glossary.md` |
| [Routines, triggers, feeds, delegation](#routines-triggers-feeds-delegation-routines-built) | `docs/definition-language.md` 7, 8 |
| [Public data](#public-data) | `docs/plan-public-data.md` |

---

## Deployment

A deployment is one database, one program, one master key and one space of its
own, served by one or more replicas. It is a logical thing. Adding replicas
scales it. It serves many tenants, and every tenant it serves lives inside it.

```
# first boot, in order (docs/deploying.md 4c). The server reads environment only.
ORDO_PG_HOST=db.internal  ORDO_PG_USER=ordo  ORDO_PG_DB=ordo
ORDO_KEY_PROVIDER=sealed-master
ORDO_MASTER_KEY_FILE=/run/secrets/master  ORDO_MASTER_KEY_PASSPHRASE_FILE=/run/secrets/pass
ORDO_SPACE=system                           # plus the token key and the page key
ordo-host                                   # creates the platform tables and the system space
ordo apply ./myprogram --as you             # the first program
```

**Guarantees.** Every environment refusal happens before the first database
dial. A secret never defaults silently and a mandatory control is never
silently off. Per deployment, not per tenant: the program, the master key,
identity (a login belongs to the deployment), the access engine and the
deployment's own security chain.

**Read next.** `docs/deploying.md` 1 and 4c, `docs/quickstart.md`.

## The deployment's space (system space)

Every deployment keeps one space for itself, with the id `system`. The binary
creates it at boot. It is not a tenant. It holds what the platform authored:
configuration, and (as those arrive) catalogue and reference data. A third
party's content never lives there. A third party is a tenant.

```
ordo-host 0.1.0  addr=127.0.0.1:4000  space=dev
  system      system  (created at boot)
```

**Guarantees.** Nobody names it from outside, so a typo cannot create it.
`std.space.delete` refuses it. It boots on its own custody, so no tenant's
state can stop it. Dependence runs one way, from a tenant to the system space.
The word "control plane" means this space and nothing above it.

**Proposed, not built:** `deployment` entities, rows that live in this space
and that any signed-in person (or a visitor, if `public`) can read. See the
syntax README, "deployment entities", and `docs/plan-syntax-additions.md` 3.

**Read next.** `docs/glossary.md` "system space", `docs/deploying.md` 4c.

## Tenant

A tenant is a space that belongs to someone: a customer, a team, a household. Its
id is minted (`spc_` plus 26 characters) and carries all the uniqueness. The name
is for people. The mechanism is always a space: its own members, access profile,
key, command chain and (ruled 2026-09-25) its own database.

```
ordo space create --name acme --owner <principal> \
    --host https://ordo.example.com --token $ORDO_TOKEN --space system
# prints the minted space id
```

**Guarantees.** Tenants never merge. Uniqueness is always per tenant: a
`unique` field never collides across tenants. Deleting a tenant destroys its key,
which erases it. Exporting a tenant is total disclosure, so both acts are
owner-only and need a fresh passkey assertion (step-up).

**Read next.** `docs/glossary.md` "space" and "tenant", `docs/bootstrap.md`.

## Member

A member is one row in the platform `members` entity: one per person admitted to
one tenant. It has an id (`mem_...`), a `role` (owner, manager, editor, viewer, or
null for a guest), a `status` and a `displayName`. You never declare it. A program
refs it. `$caller` is the caller's own members row in the tenant of the request.

```
entity notes {
  author -> members
  body:   text
  access read   open
  access write  author == $caller
}
op addNote($body: text) = set notes { author: $caller, body: $body }
```

**Guarantees.** No program mints members. A caller with no row in the tenant
matches nothing, so every `$caller` guard is false for them. The last active
owner cannot be removed, suspended or demoted. A suspension or removal reaches
every replica within 30 seconds (`ORDO_ACCESS_REFRESH_SECS`), and at once on the
replica that took the command.

**Read next.** `docs/definition-language.md` 3 "members", `docs/glossary.md`.

## Principal and person

A principal is anything whose actions must be attributable: a human (`usr_...`),
a routine, a worker, an agent, a source, a client. A person is a human principal.
Credentials and identities belong to the deployment. Admission to a tenant
belongs to the tenant. So one person holds one identity and can be a member of
many tenants, with one members row in each.

```
GET /auth/spaces          # the signed-in person's tenants
[ { "space": "spc_7kq...", "name": "Acme",  "role": "owner", "via": "member" },
  { "space": "spc_a1b...", "name": "Dana's", "role": null,    "via": "guest"  } ]
```

`std.person.tenants` gives the same list as an op. Every command in the log
names its principal: `user:<id>`, `routine:<name>`, `source:<name>`,
`agent:<name> on behalf of user:<id>`.

**Guarantees.** A tenant never stores a credential. What one tenant does to its
members cannot reach into another tenant. Every use of a tenant's power over a
credential it created is recorded.

**Read next.** `docs/plan-enrolment.md` 1 (principles P1 to P6),
`docs/definition-language.md` 8 "Principals".

## Deployment owner, app developer, tenant owner

Three people, three kinds of power. Ordo provides mechanisms. The app developer
decides what is enforced, with defaults.

| Person | Does | Uses |
|---|---|---|
| App developer | Writes the program: entities, ops, guards. Decides which settings exist and which layers may change them. | `.ordo` files, `ordo check`, the SDK and CDK |
| Deployment owner | Runs the deployment: keys, backups, applying programs, recovery. Not a customer of the app. | `ordo apply`, `ordo keys`, `ordo verify`, environment |
| Tenant owner | Owns one tenant: its members, invitations, shares and any setting the app developer exposes. | `std.space.invite`, `std.space.share`, `std.space.setAdmission` |

Today a deployment owner is an ordinary principal named in
`ORDO_DEPLOYMENT_OWNERS` and made owner of the deployment's own space at boot.
**Ruled 2026-10-01, not built:** the deployment owner signs in separately, through
the backend, with the console as the tool and its own credentials.

**Guarantees.** A capability token can never apply a program. Applying needs a
fresh passkey assertion from a human, so no integration or delegated agent can
deploy. A grant is never in a file, so a program cannot consent for anyone.

**Read next.** `docs/glossary.md` "deployment owner", `docs/deploying.md` 4c.

## The door

Every call goes through one door, and every call carries a credential and a
tenant. There is no unauthenticated path. A call without a token is refused with
the same message an unknown op gets, so a guess reveals nothing.

```
$ curl -s -X POST http://127.0.0.1:4000/op/addNote \
    -H 'x-ordo-space: dev' -H 'content-type: application/json' -d '{"title":"buy milk"}'
{"error":{"code":"access_denied","message":"missing bearer token","op":"addNote"}}
```

The order is fixed: registry lookup, method check, tenant bind, access check,
parameter bind, refinement, cursor, execute. An anonymous caller is the reserved
anonymous principal: it has no members row, so every guard is false.

**Guarantees.** One access check, before planning. A denial is one opaque answer
on the wire (bad token, no membership, suspended, op not in role, stale step-up),
with the real reason in the audit record. The method mirrors the op's kind:
`GET` for reads, `POST` for writes with a `set`, `DELETE` for pure deletes. A
wrong method fails `method_mismatch` before anything else.

**Read next.** `docs/api-engine-design.md` 1 to 4, `docs/deploying.md` 5.

## Tokens and capability tokens

A person gets a bearer token by signing in. It is signed with a key only the
deployment holds, lives 1 hour by default (`ORDO_SESSION_SECS`) and can be
refreshed until a 12 hour ceiling (`ORDO_SESSION_MAX_SECS`). A synthetic caller
(an agent, an integration, an API client) gets a **capability token**: an
explicit list of op names, a pinned hash of exactly those ops' manifests, and a
mandatory expiry.

```
POST /auth/refresh    Authorization: Bearer <token>        # before expires_at, never after
POST /auth/logout     Authorization: Bearer <token>        # 204 for anything, always
```

**Guarantees.** The clock is the checker's, never the token's. An expired token
cannot be refreshed. If an op in a capability grant changes what it touches (a
new entity, a dropped guard), the hash no longer matches and the grant is dead
until a human consents again. A capability token can never satisfy step-up.

**Proposed, not built:** an `apikey` profile in the program, the ceiling of ops
and the quota default for API keys that a tenant issues to its own customers. See
the syntax README, "apikey profiles", and `docs/plan-syntax-additions.md` 4.

**Read next.** `docs/deploying.md` 5, `docs/glossary.md` "capability token".

## Passkey sign-in and presence

A passkey is the sign-in method that proves a person is there. A sign-in with a
passkey produces a token **with presence**, and presence lasts 5 minutes. Platform
ops (apply, create a tenant, export, delete, share, invite) need it. They ask for a
fresh assertion (step-up). A password alone can never produce presence.

```
POST /auth/begin     { "method": "passkey" }
POST /auth/complete  { "kind": "passkey_assertion", "challenge": "...", "credential_id": "...",
                       "client_data_json": "...", "authenticator_data": "...", "signature": "..." }
  -> { "token": "...", "expires_at": ..., "session_expires_at": ... }
```

In the SDK: `signInWithPasskey`, `registerPasskey`, `stepUp` (see
`examples/notes/web/src/session.ts`).

**Guarantees.** A stolen password plus an attacker's second factor cannot export a
tenant, because only a passkey used as a first factor satisfies step-up. A
recovered passkey cannot satisfy step-up for a day (adjustable).

**Read next.** `docs/plan-passkey-signin.md`, `docs/deploying.md` 5.

## Second factor

A second step is a code from an authenticator app, a security key, or one of the
person's backup codes. A person turns it on by enrolling a factor. A tenant can
require it. The deployment owner sets a floor. A level can only tighten the one
above it.

```
ORDO_AUTH_SECOND_FACTOR=enabled     # off | enabled | required | required-for-non-passkey
POST /auth/factors/totp/begin       # a person enrols a code generator
```

**Guarantees.** A second factor never carries presence. A locked factor (20 wrong
answers in a row, adjustable) still counts as held, so a password alone never signs
in. A reset or a recovery removes every second factor when it completes, so a
factor an attacker enrolled does not outlive the old password.

**Read next.** `docs/plan-second-factor.md`, `docs/deploying.md` 5.

## Recovery codes and account recovery

A person with a passkey can mint 10 recovery codes, shown once. Each code proves
who the person is to the server, which holds only its hash. Spending one opens a
recovery that ends with a new passkey. The other paths are the tenant owner (for
people the tenant created), the deployment owner through the door, and the host
command.

```
POST /auth/recovery-codes      # needs fresh presence; the old set is void
POST /auth/recover/code        { "code": "..." }   # opens a recovery, returns a ticket
ordo identity recover --principal usr_01J8... --as ada     # from the host, prints a ticket once
```

**Guarantees.** Recovering an account is separate from recovering data. In every
custody mode the deployment holds today, a person who loses their passkey loses
access until they are recovered, and loses no data. A recovery code is not a
recovery phrase: the code never carries key material. Every session ends when a
recovery completes, and the retired passkeys can contest it for 3 days.

**Read next.** `docs/howto-operating.md` "When a Person Loses Their Passkey",
`docs/plan-passkey-recovery.md`.

## Ops as the API

An `op` is the one registration keyword. Its kind comes from its body. Each op
compiles once per version into a named artifact: planned SQL, mode label, cost
class, and the fields it touches. There is no free-form query text at run time.
The compiled program is projected into `interface.json`, the one contract the
generated SDK, the CDK and generated screens share.

```
ordo interface examples/concepts/01-ops-as-the-api.ordo      # the descriptor
ordo gen ts examples/concepts/01-ops-as-the-api.ordo         # regenerate the typed SDK and CDK
```

```ts
await openTicket(client, { customer, subject, opened }).commit();   // a write ends in .commit()
const rows = await openTickets(client).fetch();                     // a read: .fetch() / .pages() / .rows()
```

**Guarantees.** Method follows kind, mirrored onto HTTP. The descriptor is
byte-stable, versioned and digested. A caller sends the version its code was
built from (`X-Ordo-Version`) and learns of skew (`X-Ordo-Version-Skew: stale`).
Every entity also gets derived `std.<entity>.get`, `list`, `count`, `create`,
`update`, `delete` ops with no authoring.

**Read next.** `docs/api-engine-design.md`, `docs/std-library.md`,
`docs/tooling-and-embedding-plan.md`.

## Composition

A read op can be the source of another read. The callee finishes first, with its
own sort and take, then the caller filters its result. A write op is called and
never composed.

```
op cheapActive($cap: money) unbounded = activeProducts(price < $cap)[name, price, vendor]
op topTenUnder($cap: money) = topTen(price < $cap)[name, price]    # filters the top 10 result
```

**Guarantees.** Composition flattens to one SQL plan at compile time. A cycle is a
compile error. The scope manifest derives through any depth, so composition cannot
launder access. A callee's guards cannot be stripped by the caller.

**Read next.** [syntax 09](../syntax/09-composition.ordo),
`docs/definition-language.md` 5 "Named-query composition".

## Guards, acl and door roles

Three layers, each deciding one thing. The door decides whether a caller may call
an op at all. A guard decides which rows the op sees or may change. `acl` is the
one-line form of a guard.

| Layer | Decides | Written | Refusal |
|---|---|---|---|
| Door: roles, per-member overrides, capability allowlists | May this caller call this op | Runtime data in the tenant's access profile. Never `.ordo`. | `access_denied` |
| Guard: `access read` / `access write` | Which rows | On the entity, in `.ordo` | `not_found` (cannot read), `access_guard` (cannot write) |
| `acl (read, write, admin)` | Per-row levels per member | One line on the entity | Same as guards |

```
entity cases { title: text   status: text   acl (read, write, admin) }       # derives casesAccess and the grant ops
op caseReport unbounded unguarded = cases[status] group status { n: count }
```

See [02-who-decides-what](02-who-decides-what.ordo).

**Guarantees.** A guard follows its entity into every op, so none can forget it.
A row you cannot read answers `not_found`, the same as a row that does not exist.
Bypass is one greppable word, `unguarded`, and any op that composes it repeats it.
The space owner can always administer acl rows, checked at the door, and every
such act is a logged command.

**Read next.** `docs/definition-language.md` 3 "Access guards" and "acl" and 8
"Access model", `docs/access-model-explained.html`.

## Groups

A group is a members row with `kind` `group`, linked to its people by the platform
`groupMembers` entity. An acl row or a profile that names a group applies to its
members. `== $caller` never expands a group. Groups cannot contain groups.

```
op groups unbounded = members(kind == "group")[displayName, memberCount]
```

**Guarantees.** A caller's effective level is the maximum over every row that
applies to them, direct or through a group. Revoking one row reports the paths
that remain, because access can arrive by several routes. `memberCount` counts
people, and it is an upper bound where someone is reachable twice. A hand-written
group walk in a guard does not compile today (syntax README, Findings).

**Read next.** `docs/glossary.md` "group", `docs/definition-language.md` 3 "Groups".

A tenant can define its own roles over resources. It makes groups ("Design",
"Finance"), and a project admin grants a project to a group or a person with
the op `acl` derives. See [08-groups-and-project-access](08-groups-and-project-access.ordo)
and `docs/howto-roles.md`.

## The command log and its hash chain

Every accepted write is a command with a before and after image. All commands from
one op call form a command group: one transaction, one actor, one correlation id.
Each group is one link in the tenant's chain: `hash = SHA-256(prev || body)`.

```
ordo checkpoint --space spc_7kq                 # sign the head
ordo verify --space spc_7kq --db ordo --key-id chain-2026 --public-key <hex>
# VERIFIED 112 groups · head index 511 = 357b69…
```

Every response also carries `x-ordo-position`, `x-ordo-head`, `x-ordo-head-key` and
`x-ordo-head-sig`. A client that keeps one signed head is a witness: a rewrite
shows on its next request.

**Guarantees.** Tamper evidence. Detection is cheap, prevention is not claimed.
Verification names the first bad index and the kind of tamper. Pruning cuts only
at a signed checkpoint and never past the slowest registered consumer. Sealed
values appear in images as opaque handles only. The log still shows who changed
which row and when, even where the contents are sealed.

**Read next.** `docs/howto-audit.md`, `docs/howto-operating.md` "Verifying the
Audit Chain".

## Undo and restore

Undo appends the inverse of a command group as new history. History is never
rewritten. A restore is a different act: the whole database came back from a backup,
and the platform records that fact before it serves the tenant again.

```
ordo revert examples/concepts/06-undo-and-history.ordo \
    --space spc_acme --group <correlation id> --as ops:ada --dry-run
ordo restore --record --space spc_acme --as ada --source pitr --at 1757000000
```

**Guarantees.** A revert is checked: every touched row is tested against the
`--as` principal's write guard before and after. Reverting a revert is allowed and
is a redo. A group already reverted is refused. A restored tenant whose chain moved
backwards without a record is closed (`QUARANTINE`) while every other tenant keeps
serving. Take both backups: continuous PITR for "the disk is gone", the per-tenant
export bundle for portable, checkable data.

**Read next.** `docs/howto-audit.md` "Reverting a Write", `docs/howto-operating.md`
"Backup and Restore".

## Sealed fields and custody

A sealed field is encrypted before it reaches the database. Each tenant has a
master-wrapped key and a data key in numbered epochs. A blind index lets an
equality lookup run at SQL speed with no key in the database.

```
ordo keys init   --space acme --as ops:ada          # mint custody
ordo keys rotate --space acme --reason "laptop lost" --as ops:ada
ordo keys status --space acme --as ops:ada          # the pass, per epoch
ordo keys destroy --space acme --epoch 3 --as ops:ada --confirm acme:3
```

See [03-sealed-fields-and-custody](03-sealed-fields-and-custody.ordo).

**Guarantees.** The command log holds handles, never plaintext. Blind-index keys
come from the master key, so a data-key rotation never moves a digest and `unique`
stays true. `destroy` refuses while anything is still under the epoch and prints
the consequence for old backups before it proceeds. Destroying a tenant's key is
crypto-erasure. Known gap: nothing runs the blob sweep yet, so a rotated tenant
with files cannot reach "destroyable" (`docs/howto-custody.md`).

**Custody dial.** A tenant sits somewhere from managed keys, through its own
database or bucket, to client-first (end-to-end, the server holds ciphertext).
Guards are a convention boundary at the client-first stop, not a cryptographic one.

**Read next.** `docs/howto-custody.md`, `docs/glossary.md` "DEK epoch / rotation"
and "custody dial".

## Invitations, enrolment, provisioning on sign-up

A sign-in is an enrolment. How a person gets into a tenant is the tenant's choice,
inside a ceiling the deployment sets. An **invitation** names a role and has a
secret ticket that the app delivers. **Provisioning on sign-up** gives each person a
tenant of their own as they sign up.

```
# a tenant owner invites a person (a platform op; the ticket is shown once)
POST /op/std.space.invite   { "role": "editor", "expires": "7d", "to": "usr_01J8..." }
# the person signs in naming the tenant and the ticket
POST /auth/complete   { ..., "space": "spc_7kq...", "invitation": "<ticket>" }
  -> { "admission": { "space": "spc_7kq...", "outcome": "admitted" } }
ORDO_PROVISION_SELF_SERVE=on-signup        # every new person gets a tenant of their own
```

**Guarantees.** An invitation is always honoured. Open sign-up, provider rules and
first-comer are the tenant's choices, off by default. A manager cannot name a role
above their own. A refused admission is not a failed sign-in, and `refused` is the
answer whether the tenant is missing, closed to strangers or the ticket is not for
that person. Sign-up provisioning is off by default, capped at 60 an hour (adjustable, with no per-address limit), and an identity no tenant admits is swept after 90 days.

**Read next.** `docs/plan-enrolment.md`, `docs/plan-provisioning-on-signup.md`.

## Share links and guests

A tenant can let someone outside read chosen rows, read-only, for a stated time,
and take it back at any moment. A **guest** is a signed-in person who holds a seat
(a members row with no role). A **share link** is a secret URL for a reader with no
account. The program says nothing. The platform checks the ops a share lists.

```
POST /op/std.space.share      { "ops": "boardView,cardsOnBoard",
                                "rows": [{"entity":"boards","id":"boar_...","level":"read"}],
                                "expires": "7d" }          # a ticket, once
POST /op/std.space.shareLink  { ... }                       # a link secret, shown once
POST /auth/link               { "link": "<secret>" }        # the reader opens it
```

```ts
const client = await openLink({ baseUrl }, link);        // link read from the URL fragment
const cards = await cardsOnBoard(client, { board }).fetch();
```

See [04-sharing-a-board](04-sharing-a-board.ordo).

**Guarantees.** Every listed op must be a read, not `unguarded`, guard-bearing, and
callable by the sharer, or the share is refused and no ticket exists
(`guest_exposure_unguarded`). Revocation closes the window at the next call, since
the door asks the grant registry on every call. A link always expires (7 days by
default, never a ceiling). The door keeps no per-address miss count. A guest
or link never writes, never calls a platform op and never shares onward. Revoking
stops access. It does not undo what was already read.

**Read next.** `docs/howto-sharing.md`, `docs/plan-sharing.md`.

## Live updates and the device copy

Every named read is live-capable, and the caller chooses at call time. A live call
streams `invalidate` events and no data. The client re-reads through the normal
op under its own access. A read the app syncs is also kept in a copy on the device
(IndexedDB, per tenant and per person, sealed with a device key), so a list shows
at once and shows offline.

```
GET /op/topicList?live=1
data: {"rows":[...]}             # first frame: the ordinary answer
event: invalidate                # on each relevant commit; refetch now
data: {}
: hb                             # heartbeat every 15 s
```

See [07-team-live-and-device-copy](07-team-live-and-device-copy.ordo) and the
working page in `examples/notes/web`.

**Guarantees.** The stream subscribes before the first read, so no commit slips
between. A membership or acl write invalidates guarded live reads, so permission
changes are visible changes. A device holds only what an ordinary call would
return, and a row is replaced only by an equal or higher chain position. Sign-out
purges the copy and a refusal purges what that op synced. Writes still need the
network today.

**Read next.** `docs/plan-client-cache.md`, `docs/api-engine-design.md` 6.

## Versions and deploys

`ordo apply` compiles the program into an artifact. Its content hash is the
version, and every artifact is kept. Applying points the deployment at it, and
every replica swaps with no restart after the load gate passes. The schema change
is planned at apply time, labelled by class, and only additive changes are sent.

```
ordo plan    examples/concepts/05-version-2.ordo --against prod    # read-only report
ordo migrate examples/concepts/05-version-2.ordo --apply --db prod # the additive part
ordo apply   examples/concepts/05-version-2.ordo --db prod         # store and activate
ordo deploys                                                       # stored, serving, every re-point
ordo rollback --version abc123def456…
```

See [05-version-1](05-version-1.ordo) and [05-version-2](05-version-2.ordo). Version 2
adds an optional field and an entity, and stops declaring `shelf`. That field is
orphaned, which means kept and unserved.

**Guarantees.** The old version keeps serving until the new one passes the load
gate. A change a tenant's database cannot take closes only that tenant, with the
reason on `/health/ready`. A removed field is never dropped by `ordo apply`.
Rollback re-points and does not undo the schema change.

**Proposed, not built:**

- Plans as stored artifacts with options per change (`ordo apply <plan-id>`) and a
  committed receipt: `docs/plan-evolution-plans.md` (ruled in part, 2026-10-01).
- `prev`, to say a renamed field is the same field, and `ordo plan --prune` to drop
  an orphan with a second confirmation: `docs/plan-syntax-additions.md` 5 and 6.

```
entity customers (key: cust, prev: clients) {     # Proposed, not built
  fullName: text prev(name)
}
ordo plan . --against prod --prune customers.fax  # Proposed, not built
```

**Read next.** `docs/howto-operating.md` "Deploying a Program" and "Changing the
Schema", `docs/plan-schema-evolution.md`.

## Settings and their layers

A setting is a limit or a choice with a default. It can sit at four layers, and each
layer can only move it as far as the one above allows. Ordo ships the default. The
app developer may set it and may hand it down. Then the tenant, then one person,
then the single act (a parameter on one call).

| Layer | Written today |
|---|---|
| Deployment default | environment: `ORDO_SESSION_SECS`, `ORDO_SHARE_LINK_DEFAULT_SECS`, `ORDO_RECOVERY_*` |
| App developer | a few environment switches. The program has no spelling yet. |
| Tenant | a platform op, where the app developer exposes it: `std.space.setAdmission`, `std.space.setShareLinkLifetime` |
| The act | a parameter, such as `expires` on a share |

**Proposed, not built:** a `settings { }` block in the program with the layer words
`tenant` and `person` and bounds, plus `std.space.setSetting` and
`std.person.setSetting`. `docs/plan-syntax-additions.md` 7.

```
settings {                                              # Proposed, not built
  share.linkLife         = 7d     tenant <= 30d
  commandlog.undoHorizon = 90d    tenant >= 90d
  live.refetchPace       = 250ms  person >= 100ms
}
```

**Guarantees.** The act's value is checked against the layers above it. A tenant
can only tighten what the deployment sets. Nothing is a hardcoded system value if
a layer could reasonably choose it: `docs/settings.md` lists the constants that
still are.

**Read next.** `docs/settings.md`, `docs/audit-settings-wiring.md`.

## The security event stream

The command log answers "who did what". It cannot answer "who tried what". The
security event stream does: denials with the attempted op, step-up demands and
outcomes, key releases, break-glass use, grant mint and expiry, authenticator link
and unlink, first-login provisioning, and the deployment's own custody acts. Each
tenant has one, and the deployment has one that belongs to no tenant.

```
ordo verify --deployment --db ordo --security-key-id sec-2026 --security-public-key <hex>
ordo checkpoint --security --deployment
```

**Guarantees.** It is hash-chained and checkpointed like the command log, and signed
under a different key, so compromising one does not forge the record of the other.
`ordo verify` reports the two verdicts apart and never merges them. A checkpoint is
signed only after the stream verifies. The deployment's stream is the only place a
sweep across many tenants shows. A keyless verification still catches an edited
entry and says it cannot prove nothing was cut off the end.

**Read next.** `docs/howto-audit.md` "The Security Stream", `docs/glossary.md`
"security-event stream".

## The standard library, cost classes, step-up

**Standard library.** For every entity the platform derives `std.<entity>.get`,
`list`, `count`, `create`, `update`, `delete` (and `put` with one unique key,
`deleteSubtree` on trees). `ordo check` lists them. They are registered, and
exposing one is a separate act. For an entity with sealed fields they cover the
open fields only. `std.space.*` and `std.person.*` are platform ops for tenants and
people.

**Cost class.** Every op carries a price tag from its worst stage: `sql`,
`blind-index`, `bounded-host` or `unbounded-host`. `unbounded` is the author's
acknowledgment that work grows with the tenant. Today the compiler requires it for
unbounded host work over sealed fields.

**Step-up.** A fresh passkey assertion within 5 minutes, required by every platform
op and by ops a client marks. A capability token can never give it.

**Read next.** `docs/std-library.md`, `docs/definition-language.md` 5 "Cost
classes".

## Domains, apps and clients

A **domain** is a versioned package of shapes and ops. It holds no tenants, grants
or data. An **app** is a named screen manifest above domains, compile-checked
against the op catalogue (not built). A **client** is an OIDC relying party, a thing
that signs in and calls ops, such as a web shell (declaration not built). Domain,
app and tenant are many-to-many.

```
domain notes @ 0.1.0        # the one line the compiler reads today
```

**Guarantees.** Consent is runtime state and never ships in a package. Several apps
on one tenant are windows onto one world with the same members, profiles and chain.

**Read next.** `docs/glossary.md` "domain", "app", "client", `docs/ui-gen.md`.

## Routines, triggers, feeds, delegation (routines built)

Routines are built: a `routine` declaration with a `trigger` clause compiles and
runs (`examples/syntax/20-routines.ordo`, `docs/howto-routines.md`). Feeds are
not built.

- **Routine and trigger.** Automation written in `.ordo`, as a `routine` with a
  `trigger` clause: a listener on the command envelope (`trigger when
  billing.invoices.ingested`) or a clock (`trigger every day 08:00Z`). The top-level
  `when ... -> run x` form is refused. A trigger sees shape and time, never values. A clock-woken run passes
  guards. A listener-woken run sees only rows reachable from `$trigger.ids`.
  Delivery is at-least-once. `ordo triggers ls` and `ordo triggers deregister`
  exist for the listener cursors. `docs/definition-language.md` 7 and 8.
- **Feed.** A standing, both-ends-consented, one-way copy of a named query's output
  into another tenant. The receiver owns the copies, and its own triggers fire on
  them. A window (guest) is the live alternative. `docs/glossary.md`
  "feed vs window".
- **Delegation.** Acting on behalf of a user needs that user's own consent. The
  record and the door are built. `docs/plan-delegation-surface.md`.

## Public data

**Built (2026-10-10).** `public` makes a field, an entity or an op readable with no
token. Nothing is public unless declared, and a public op may read only public
fields. A site key is a label and not security. Guide: `docs/howto-public.md`.
Example: `examples/syntax/21-public.ordo`.

```
entity plans {
  name:  text  public
  price: money(2) public
  margin: money(2)                  # private
}
op pricing public = plans(price > 0)[name, price]
```

**What it guarantees.** The compiler refuses `public` on a sealed field and a
public op that reads a private field. A public result is cached and dropped exactly
when an entity it read changes, with no per-call audit row and no guard walk. The
anonymous caller is still the reserved anonymous principal, so "every call carries a
credential" holds. Public writes and the `deployment` line are not built.

**Read next.** `docs/plan-public-data.md`, [examples/IDEAS.md](../IDEAS.md) "Public
data".
