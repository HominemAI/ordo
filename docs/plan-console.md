# Plan: the Ordo console

**The console is how a deployment owner deploys, observes and manages one
Ordo deployment from a browser. It is its own application, with its own
server, its own Postgres database and its own sign-in. In the public image
it starts and supervises the deployment process inside the same image, so it
keeps serving its pages when the deployment is down or crash-looping. It
manages the deployment two ways: in-app changes through the door, and the
deployment's runtime (environment, settings, restarts, rollback) through its
supervisor. It ships in the open-source release.**

Written 2026-10-22, revised 2026-10-23 (twice) against the tree at
`5ba6bda2`. Checklist item 18o.

Rulings this plan follows (`corpus-rulings.md`):

- 2026-09-21, "The control plane is a service above deployments" and "The
  console, the operator, and where deployments run".
- 2026-09-25, the four database levels (`roadmap-checklist.md` 18d): the
  console has its own database, which belongs to the console and not to
  any deployment.
- 2026-10-22, "the console is required for launch" and "custom settings,
  for Ordo v2".
- 2026-10-23, "the console's launch scope", "the console's implementation
  decisions" and "the console runs inside the one image".

It builds on `dashboard-plan.md`, whose section 2 ("the dashboard is a
client, not a backdoor") stands for everything inside the application.
That document's break-glass mode and plan-variant pane are not in launch
scope.

Words: the **deployment owner** is the person who runs the deployment (for
a self-hoster, also the app developer). The **operator** is software: the
later, private service that provisions many deployments. "Tenant" and
"space" mean the same thing here.

### What changed on 2026-10-23

- Own server, database and owner sign-in; the console holds a deployment
  credential (asks 6, 8).
- Environment settings editable at launch, applied by a restart (ask 9).
- No Docker, Kubernetes or manual driver. The console supervises the
  deployment process inside the one image (asks 13, 16).
- Separate deployment-only and console-only images are offered (ask 16).
- Console database is Postgres, its own database; SQLite is gone (ask 14).
- Lost passkey: `ordo console recover`, recovery codes, a second owner
  (ask 17).
- Stack: `ordo-sdk` for door calls, plain React, no `ordo-react` data
  hooks; ruled (ask 6).
- Ruled earlier: metadata view, caps in v2, slow ops, rollback and preview
  at launch (asks 5, 10 to 12).

---

## 1 · What the console is, in seven lines

1. A Rust server (`ordo-console`) and a web page it serves. The page talks
   only to the console server, never to the deployment.
2. The console has its own Postgres database. Its own state (owner,
   sessions, runtime revisions, sealed secrets, its audit log) lives there
   and nowhere else.
3. The console owner signs in to the console with a passkey. That works
   when the deployment is down.
4. For in-app management, the console server calls the door with a
   deployment credential it holds sealed. Platform writes still need a
   human touch at the deployment (step-up).
5. For the runtime, the console is the **supervisor** of the deployment
   process in the same image: it spawns it, health-checks it, restarts it
   with new environment and settings, and rolls back if it is not ready in
   time. There are no public runtime drivers.
6. Every screen is a rendering of a platform op, a platform route, or a
   supervisor call. A screen that needs data no op gives is a gap in Ordo,
   fixed in Ordo.
7. Open source manages one deployment. The code keys everything by a
   deployment handle, so the operator can later hand it a list.

---

## 2 · Who uses it

| Person | What they need | What that means for the design |
|---|---|---|
| Self-hoster who is also the app developer | Apply a version, change a setting, see it is healthy, fix a stuck tenant | Runtime, deploy and health screens first |
| Deployment owner who is less technical | "Is it up? Why not? Who is using it? Can I turn this off?" | Plain words, no raw JSON by default |
| Deployment owner who runs things their own way | Own scaling, own orchestrator | The separate images (section 9) |

Tenant owners (later, optional) get their own tenant's people and shares in
v2; launch has one console owner (two with the failsafe).

Rules for less technical readers, on every screen: lead with a status
sentence and fold the detail away; when the deployment is down say so, then
why (exit code, last log lines), then the one button that most likely fixes
it; show the door's refusal, which names the setting
(`refusals-name-the-setting`); dangerous acts show what will happen, ask
for a typed confirmation, then a touch; no raw SQL, ever.

---

## 3 · Two planes, one rule

The console changes the deployment in exactly two ways.

| Plane | What it changes | How | Works when the deployment is down? |
|---|---|---|---|
| **In-app** | Programs, tenants, people, sign-in policy, shares, quotas | Platform ops at the door, as the deployment owner, step-up on writes | No. The screens say "the deployment is not answering" and link to Runtime |
| **Runtime** | Environment, settings, secrets, restart, settings rollback | The supervisor, in the one image only | Yes. This is how a broken deployment is fixed |

**No back door, restated.** The console never opens the deployment's
database, never reads its keys, and never writes its tables. In-app state
changes only through the door, where the deployment's own checks and audit
apply. Runtime changes only through the supervisor, each as a numbered
revision in the console's database. A test asserts the console connects only
to its own database, and boot refuses if the two database URLs name the same
database.

**In the separate images there is no runtime plane.** The console-only image
reaches the deployment through the door and nothing else. It cannot start,
stop, restart or change the settings of the deployment, and the Runtime
screen says so. The docs say so too (section 9).

The supervisor is powerful: whoever can restart the deployment can change
any setting, including ones that loosen security. That power is guarded by
the console's sign-in and step-up, its audit log, and the deployment's own
boot record (G16), so a change is never silent on either side.

---

## 4 · Screens, the calls behind them, and the gaps

**How to read the tables.** "Calls" is what exists today in
`crates/ordo-host/src`, or a supervisor call (section 6). "Gap" is an op or
route Ordo lacks. Gaps are numbered G1 to G17 and gathered in section 7.

Three facts shape the in-app screens:

- **The deployment owner owns the home space** (`ORDO_SPACE`).
  `std.deployment.*` ops answer only there (`apply_op.rs`, check 3).
- **`std.space.*` ops answer only to members of that tenant.** Ruled
  2026-10-23 (ask 5): the owner sees every tenant's name, status, counts
  and usage, not its members or shares.
- **Some numbers are per node.** `/metrics`, `/security/alerts` and
  `std.deployment.observedCost` answer for the node that served the
  request. `/health/deploy` gathers every node.

### 4.1 Health (home screen)

| Shows | Calls | Gap |
|---|---|---|
| Running, crash-looping or stopped, restarts, last exit code, last 200 log lines | supervisor `status`, `logs` | none (supervisor) |
| Which runtime revision is live, and whether the deployment confirmed it | supervisor `status`; `/health/deep` | **G16** |
| Ready or not, quarantined tenants | `GET /health/ready` | none |
| Database, key provider, supervisor, workers, security drain, stuck tenant setup | `GET /health/deep` | none |
| Each node, its version, catch-up done or not | `GET /health/deploy` | none |
| Deny-volume alert | `GET /security/alerts` | none |
| Chain verified, last checkpoint | nothing through the door | G11 (v2) |

The first row comes from the supervisor, so the home screen has something
true to say when the door is silent. In the console-only image the first
two rows are absent and the screen says why.

### 4.2 Runtime (new)

| Shows or does | Calls | Gap |
|---|---|---|
| Version (the image's), every environment setting, secrets as set or unset | supervisor `current`; console database | none |
| Edit settings, then preview the change | console diff; `ordo-host check-config` with the new settings | **G17** |
| Restart with the new settings, watch it come up, auto roll back if not ready in time | supervisor `apply`; `/health/ready` | none |
| Restart without a change | supervisor `restart` | none |
| History of runtime revisions, who, when, why; roll back to one | console database; supervisor `apply` | none |
| What the deployment actually took (effective value and source) | `std.deployment.settings` | **G7** |

G7 and the console's own record together show drift: a value the console
set that the deployment did not take, or one set outside the console.

A new **version** is a new image, which the supervisor cannot change from
inside. The deployment owner upgrades the image on their platform. At boot
the console records the version and starts the deployment on the live
settings revision. If it is not ready, the console offers the previous
settings revision; rolling back the image is the platform's.

### 4.3 Deploys (programs)

| Shows or does | Calls | Gap |
|---|---|---|
| Apply a program (upload `.ordo` source, activate or store only) | `std.deployment.apply` | none |
| Wait until every node serves it | `GET /health/deploy`, polled | none |
| Preview what an apply will do to the schema, nothing stored | nothing; `ordo plan` is local | **G3** |
| Stored versions and activation history | nothing; `ordo deploys` reads the database | **G1** |
| Roll back (activate a stored version) | nothing; `ordo rollback` reads the database | **G2** |
| Prune old versions | `ordo prune` | v2 |

A program rollback (G2) and a runtime rollback (4.2) are different acts.
The screens say "Roll back the app" and "Roll back the server".

### 4.4 Tenants

| Shows or does | Calls | Gap |
|---|---|---|
| Every tenant: name, id, status, counts, usage, when made | nothing; `ordo fleet` reads the database | **G4** |
| Create a tenant | `std.space.create` | none |
| Remove one stuck in provisioning | `std.space.reap` | none |
| Pause and resume | `std.deployment.pauseTenant`, `resumeTenant` | none |
| Quarantined tenants and why | `GET /health/ready` | none |
| Export, delete, attach a bucket (owner of that tenant) | `std.space.export`, `delete`, `attachBucket`, `detachBucket` | none |

### 4.5 People (inside a tenant where the owner holds a role)

| Shows or does | Calls | Gap |
|---|---|---|
| Members, role, joined | `std.space.members` specified, not built | **G5** |
| Change role, remove, per-member access; invite, list and revoke invitations | `setRole`, `remove`, `setMemberAccess`, `invite`, `invitations`, `revokeInvitation` | none |
| Admission, recovery, succession, delegations | existing `std.space.*`, `std.delegation.*` ops | none |
| Groups; who can call an op | `groups`, `createGroup`, `deleteGroup`, `addToGroup`, `removeFromGroup`, `callersOf` | group members, `callableBy`: **G5** |
| Access profile | `profile`, `setOpAccess`, `resetOpAccess` | none |
| Grants given to apps | `std.space.grants` specified, not built | **G5** |

People search across the deployment (old G6) goes to v2 with break-glass,
per ask 5.

### 4.6 Sign-in settings

| Shows or does | Calls | Gap |
|---|---|---|
| Deployment session policy, self-serve, guest, email policy, claim code, link password limits | `std.deployment.set*` ops | reading current values: **G7** |
| A tenant's session and recovery policy, OIDC connections, allowed origins | existing `std.space.*` ops | none |
| Settings that live in the environment (`ORDO_AUTH_*`) | Runtime screen, via restart | none |

### 4.7 Shares and links

Unchanged from the first plan; every op exists. No gap.

### 4.8 Security log

| Shows | Calls | Gap |
|---|---|---|
| One tenant's security record | `GET /security/events?space=` | none |
| The deployment-wide record | refused by name in `serve.rs` | **G8** |
| Attestation and dead letters | `std.space.attestation`, `attestationDeadLetters`, `chainProof` | none |
| Every in-app change the console made | partial | **G12** |
| Every runtime change, and who signed in to the console | console audit log | none (console) |

### 4.9 Usage and limits

| Shows or does | Calls | Gap |
|---|---|---|
| Usage per tenant over a range | `GET /usage` | none |
| Quotas: show and set | `ordo quota` reads the database | **G9** |
| Caps page with hit history | partial counters on `/metrics` | G10, v2 (ask 10) |

### 4.10 Every setting

- **At launch**, the Runtime screen edits any setting in `settings.md`. A
  change is a new revision and a restart.
- **Platform-op settings** (4.6, pause, origins, quotas) change live
  through the door.
- **In v2**, `settings-layers` (24a to 24d) makes many settings live and
  adds app-defined settings, secrets and flags. A setting that becomes live
  moves from "applies on restart" to "applies now".

### 4.11 Observability and query performance

| Surface | What it carries |
|---|---|
| supervisor `status`, `logs` | process state and output, with the deployment down or up |
| `/health/*` | live, ready, deep, deploy (as today) |
| `/metrics` | op latency, pool wait and saturation, chain, denials, quotas, reseal, shape checks, observed cost |
| `std.deployment.observedCost` | per op (and per tenant if asked): calls, errors, total and longest duration, rows, effective cost class |

**Slow ops table at launch** (ask 11). Built on
`std.deployment.observedCost`: ranked by mean and longest duration, with
error rate and cost class. Numbers are per replica and reset at a restart,
so the table names the node and time since its start.

Still v2 (G13): tracing, per-request slow log, plan views.

---

## 5 · The console server, its database, and the stack

### 5.1 What the server does

- Serves the page, with strict headers (CSP `default-src 'self'`,
  `frame-ancestors 'none'`, no referrer).
- Signs the console owner in (5.3) and keeps sessions.
- Offers a JSON API to the page. Each call checks the session, then calls
  the door with the deployment credential (section 8) or the supervisor
  (section 6).
- Supervises the deployment process in its own task tree, apart from the
  web server.
- Seals secrets in its database with its own key, and writes its own audit
  log: who, what, when, the outcome.
- Answers `/health/live` and `/health/ready`. Readiness never depends on
  the deployment.

The page never holds a deployment token. The deployment needs no CORS entry
for the console.

### 5.2 Its database

The console's own database (level 1 of the four, 2026-09-25). It is
**Postgres, its own database** (ask 14). The deployment owner points
`ORDO_CONSOLE_DATABASE_URL` at any cluster. It may be the cluster the
deployment uses, as a separate database with its own role; that is
irrelevant to the console. There is no SQLite option and no default
database: an unset URL is a boot refusal that names the setting. SQLite is
out because persistence and moving data between nodes are unreliable for it.

| Table | Holds |
|---|---|
| `owner`, `passkeys`, `sessions`, `recovery_codes` | the console owners and their sign-in |
| `deployments` | one row in open source: handle, door URL, mode (supervise or door-only) |
| `revisions` | every runtime revision: settings, secret references, who, why, outcome |
| `secrets` | sealed secret values the console manages (6.4) |
| `credentials` | the sealed deployment credential (8.2) |
| `audit` | every console action |

Boot refuses if the console's database URL and the deployment's name the
same database. Migrations are the console's own and run at its boot.

**The cost, said plainly.** If the console's cluster is down, nobody can
sign in. The login page and `/health/*` still answer, and the supervisor
keeps the running deployment up. Putting the console's database on a
different cluster from the deployment's removes the shared failure; the
guide and the compose example do that. Backups are the deployment owner's.
The console key is backed up apart from the database.

### 5.3 Console sign-in

1. **First run.** The console prints a one-time setup code to its log and a
   file in its volume. The first person to enter it enrols a passkey and
   becomes console owner. The code then dies.
2. **Recovery codes.** On enrolment the console shows 10 single-use codes
   once and stores only their hashes. A code signs the owner in once and
   forces a new passkey and a new set.
3. **Sign-in** is a passkey ceremony against the console's own relying
   party. Nothing at the deployment is involved.
4. **Session**: HttpOnly, `SameSite=Strict`, its own origin.
5. **Step-up**: runtime and credential changes need a fresh touch (300
   seconds, as at the door).
6. **A second console owner**, invited with a one-time code, who enrols
   their own passkey. Removing either needs a touch. One lost passkey is
   then not a lockout.
7. **Lockout failsafe** (ask 17, recommended): `ordo console recover`, run
   inside the image (`docker exec`, `kubectl exec`), prints a one-time
   setup code that enrols a new passkey. The command is written to the
   audit log (time, unix user), and when the code is used every old console
   session ends. It adds no power: anyone who can run it already controls
   the deployment. It leaves the console key at the deployment alone.

More console users, and roles among them, are v2.

### 5.4 The stack (ask 6, ruled 2026-10-23)

The server is Rust. The page layer was ruled on 2026-10-23 ("ok sounds
good"), after the owner asked why `ordo-react` would be used without Ordo
data:

- **Door calls** go through the console **server**, using `ordo-sdk`. The
  page never calls the door.
- **The page** is plain React with Vite, talking only to the console's JSON
  API.
- **No `ordo-react` data hooks.** They read Ordo data from a browser, and
  this page holds none.

The JSON API in 5.1 is the same under any page stack, so the build order
holds. A server-rendered page is the fallback. Page work invokes
`modern-web-guidance:modern-web-guidance` first, per `AGENTS.md`.

---

## 6 · The supervisor

The public image runs `ordo-console` as its main process, and it spawns
`ordo-host` as a child in the same container. This is not how standard pods
are run. It is chosen for simplicity (2026-10-23).

### 6.1 What it does

| Call | Does |
|---|---|
| `status()` | running, starting, crash-looping or stopped; restarts; last exit code; live revision label |
| `logs(tail)` | the last lines of the child's output, in a ring buffer |
| `current()` | settings, secret references, version |
| `apply(spec)` | restart the child with a new spec |
| `restart()` | the same spec, a new process |

A **spec** is settings as name and value, secret references and a revision
label. Every applied spec is a revision; rollback is `apply` of an earlier
one.

**Every apply:**

1. Diff against the live spec and show it.
2. Run `ordo-host check-config` with the new settings (G17), as a short
   separate process. A refusal stops here, touching nothing running.
3. Touch (step-up).
4. SIGTERM the old child, wait for its grace period (default 30 seconds),
   then SIGKILL. Spawn the new child.
5. Poll its `/health/ready` until ready or the timeout (default 120
   seconds).
6. Not ready, or exited: spawn the last good revision, record the failure,
   say so on screen.
7. Ready: mark the revision good; G16 lets the deployment confirm the label.

On one node a restart is a short outage. A zero-downtime swap is v2.

### 6.2 When the deployment is down or crash-looping

The web server and the supervisor are separate tasks. A child that dies,
hangs or loops never stops the console serving its pages, and the console
reports ready while the child is down.

- **Backoff.** Restart after 1 second, then 2, 4 and so on up to 60. The
  delay resets after 10 minutes ready.
- **Crash loop.** 5 exits in 10 minutes makes the state `crash-looping`.
  The supervisor keeps trying every 60 seconds. Health leads with the exit
  code, the last log lines and a "roll back" button.
- **Auto rollback.** A crash loop within 10 minutes of an apply rolls back
  to the previous good revision, once. A second failure stops automatic
  changes and waits for the owner.

### 6.3 The child's environment and secrets

- The child gets a cleared environment holding only its own settings. The
  console key, the console database URL and session secrets are never in it.
- A secret is a file in a private tmpfs directory (mode 0600, the child's
  unix user), and the child gets its `_FILE` path. Ordo already refuses
  secrets in plain variables (`settings.md`).
- **Managed** secrets are sealed in the console database. **Referenced**
  secrets name a file the deployment owner mounted, which the console never
  reads. The screen shows set or unset and when it last changed, never the
  value.
- The deployment's database URL is such a secret. The console unseals it
  only to write the file and never connects with it.
- **First revision.** Revision 1 is built from the `ORDO_*` variables the
  container started with (not `ORDO_CONSOLE_*`). After that the console
  database is the source; later container variables are ignored and the
  Runtime screen says so.

### 6.4 One node per image

One image is one console and one deployment process. For several nodes the
deployment owner uses the separate images (section 9). Managing several
copies across nodes is not planned for the open-source console: it belongs to
Ordo's own cloud service, in the private operator (owner, 2026-10-23).

---

## 7 · The gaps, gathered

Each gap is added to `ordo-host`, owner-only in the home space unless said
otherwise, a write behind step-up, a read without. Most help the CLI too.

| Id | Add | Replaces | Size | Launch? |
|---|---|---|---|---|
| G1 | `std.deployment.versions`: stored versions, activation history | `ordo deploys` | S | yes |
| G2 | `std.deployment.activate($version)`: program rollback | `ordo rollback` | S | yes (ask 12) |
| G3 | `std.deployment.plan($program)`: schema triage, nothing stored | `ordo plan` against a live database | M | yes (ask 12) |
| G4 | `std.deployment.tenants`: id, name, status, counts, usage | `ordo fleet` | S | yes (ask 5) |
| G5 | `std.space.members`, `grants`, `groupMembers`, `callableBy` | nothing | M | yes |
| G6 | people search across the deployment | nothing | M | v2 (ask 5) |
| G7 | `std.deployment.settings`: each setting's effective value, default, range, source; secrets as set or unset | reading env on the box | M | yes |
| G8 | deployment-wide security record | nothing | S | yes |
| G9 | `std.deployment.quotas`, `setQuota` | `ordo quota` | S | yes |
| G10 | caps report with hits | nothing | M | v2 (ask 10) |
| G11 | chain health: last verify, last checkpoint | `ordo verify` | S | v2 |
| G12 | every console write through the door leaves a record | partial today | S | yes |
| G13 | tracing, slow-request log, plan views | nothing | L | v2 |
| G14 | **console key**: a credential kind `console` bound to the owner, sign-in by signed challenge, never presence; `addConsoleKey`, `consoleKeys`, `revokeConsoleKey` | nothing | M | yes |
| G15 | **presence relay**: step-up for a G14 session by the owner's passkey assertion, made in the browser on the console's origin | nothing | M | yes |
| G16 | **boot record**: the deployment records its image digest, settings fingerprint and `ORDO_RUNTIME_REVISION` in the security stream and `/health/deep` | nothing | S | yes |
| G17 | **`ordo-host check-config`**: read every setting, refuse as boot would, serve nothing, exit | nothing | S | yes |

G12 is a check, then fill: `quotas.rs` and `tenants.rs` do not record yet.
G17 is also what the supervisor runs before every apply. G14 to G16 serve
the door in both image modes.

---

## 8 · How the console signs in to the deployment

The deployment's renewal cookie is `SameSite=Strict` (`renewal.rs`), and
the console may sit on any address the app developer picks. So the browser
never holds the deployment session. The console server does.

### 8.1 Two kinds of trust, kept apart

- The **console key** lets the console server act as the deployment owner
  for reads and for writes that need no presence.
- **Presence** still comes from a human touch at the deployment's relying
  party. The console key can never satisfy step-up, the same rule that
  stops a capability token (`howto-operating.md`). So platform writes,
  including `apply`, stay human-only.

### 8.2 The console key (G14)

| Step | How |
|---|---|
| Minted | The console makes an Ed25519 key pair. The private half never leaves the console |
| Stored | Sealed in the console database with the console's own key (`ORDO_CONSOLE_KEY_FILE`). A copy of the database alone does not yield it |
| Enrolled | Once, through the door: the owner signs in at the deployment with their passkey in the console page (8.3), and the console calls `std.deployment.addConsoleKey($publicKey, $name)` with step-up. The deployment stores it as a credential of the owner, of kind `console` |
| Used | The console server signs a challenge from `/auth/begin`, gets a short session (default 15 minutes), and renews it by signing again. No cookie, no browser |
| Rotated | Every 30 days by default, or on demand: mint a new pair, enrol it with the old session and a touch, retire the old one |
| Revoked | From the console, or at the deployment by the owner signed in with a passkey (`std.deployment.revokeConsoleKey`, or the CLI). A revoked key stops at its next sign-in |
| Listed | `std.deployment.consoleKeys`: each key, its name, last use |

First run of a new deployment: the console drives the existing claim flow
(`/auth/claim/*`). It can set the claim code itself through the
supervisor (in the separate images the deployment owner sets it), then
the owner claims with their passkey and enrols the console
key in the same sitting.

### 8.3 Step-up across sites (G15)

1. The console server asks the door for a step-up challenge for its G14
   session.
2. The page runs the passkey ceremony for the deployment's relying party.
   The browser allows it from another domain when the console's origin is in
   `ORDO_AUTH_RELATED_ORIGINS` (WebAuthn Related Origin Requests), or in
   `ORDO_AUTH_ORIGINS` when it shares the domain. The console sets that
   through the supervisor when linked; in the separate images the page shows
   the value to set.
3. The console server hands the assertion to the door, which checks it is
   the same principal as the G14 session and marks that session present for
   300 seconds.

One touch covers a short run of changes, and it is always the person's own.

---

## 9 · Packaging: one image, and two separate builds

Ruled 2026-10-23 (asks 13, 16): "I do not want to provide a public driver
because there are so many different ways to deal with scaling. Let's just do
the 1 image for now."

### 9.1 The one image

The image `ordo` carries `ordo-console`, `ordo-host` and the `ordo` CLI. It
starts `ordo-console`, which spawns and supervises `ordo-host` (section 6).
This is the quickstart and the default in every guide.

### 9.2 The separate builds

For deployment owners who run things their own way, the release also
publishes `ordo-host` (the deployment only) and `ordo-console` (the
console only, in door-only mode).

**Between these two, the console talks to the deployment only through the
door.** It has no runtime control. It cannot start, stop, restart or change
the settings of the deployment, and it holds no deployment secrets. The
Runtime screen is read-only: it shows what the deployment reports (G7, G16),
and the deployment owner changes settings on their own platform. If the door
is silent the screen says "the deployment is not answering" and no more,
because the console has no view of the process. Everything in-app works the
same in both modes.

The docs say this in `howto-console.md` and the Runtime screen says it
again.

### 9.3 What the release carries

- `crates/ordo-console`, `clients/ordo-console-web` (the page), and
  `docker/Dockerfile` building the three images; `images.yml` publishes
  them.
- Two compose files: the one image with a two-database Postgres, and the
  separate images.
- No Helm chart, no Docker or Kubernetes driver. A deployment owner on
  Kubernetes runs whichever image they like, however they see fit.

### 9.4 What a self-hoster sets on the console

`ORDO_CONSOLE_LISTEN`, `ORDO_CONSOLE_PUBLIC_URL` (its own relying party),
`ORDO_CONSOLE_KEY_FILE` (the sealing key), `ORDO_CONSOLE_DATABASE_URL` (its
own Postgres database; required; the URL holds a password, so
`ORDO_CONSOLE_DATABASE_URL_FILE` reads it from a file, and setting both
refuses the boot), `ORDO_CONSOLE_MODE` (`supervise`, or
`door-only` in the console-only image) and `ORDO_CONSOLE_DEPLOYMENT_URL`
(the door, from the console's side). `ORDO_CONSOLE_KEY_ROTATE_DAYS` (30,
1 to 365) says when the console key is due for rotation; a rotation takes
the owner's touch, so the console asks rather than rotating alone. Every
other deployment setting is a
revision the console holds and shows on the Runtime screen.

---

## 10 · Security

| Concern | Answer |
|---|---|
| Back door into app data | None. The console never connects to the deployment's database. In-app changes go through the door, checked and recorded by the deployment |
| The console holds the deployment's environment | In the one image it holds every setting and sealed secret and writes them to the child's files. The console database and key are the crown jewels: the key is a file on the console's volume, never in the child's environment, and the database role is the console's own. A copy of the database alone yields no secrets |
| Process supervision in one container | The child runs as a different unix user (65533; the console is 65532), with a cleared environment and no access to the console's volume (0700) or key. The console binary holds `CAP_SETUID`, `CAP_SETGID`, `CAP_SETPCAP` and `CAP_CHOWN` as permitted file capabilities only, and is `root:65532` mode 0750 so the child's user cannot run it. The launcher refuses any caller but the console (its real uid must own the binary or be in its group, and its kernel-read parent must be the pid the console named, running as that uid), and refuses root or the console's own identity as the target, each with exit 77. It empties the bounding set, switches user, drops every capability and sets no-new-privileges before `ordo-host` starts; the console raises `CAP_CHOWN` to give each secret file to the child's user. The console's `/proc`: `hidepid` is a mount option only the platform can set, so the console marks itself not dumpable, and the kernel refuses any other process its memory, environment and open files there; the child still sees its pid and command line. If the launcher cannot switch users while `ORDO_CONSOLE_CHILD_USER` is set, the console refuses to start, naming the setting; it never falls back to its own user (images run 40172394128 did, when the container lacked `CAP_SETPCAP`). Setting it empty accepts one user, and the child can then read the console's volume and key. Rehearsed in step 2 (2b) |
| Child lifetime | In the console's process group with a parent-death signal, so no orphan holds the port. A console crash restarts both |
| Runtime power | Can change any setting. Guarded by console sign-in, step-up per apply, the console audit log, and the deployment's boot record (G16) |
| Console sign-in | Passkey only, its own relying party. Setup code once; recovery codes (hashed, single use, a used code forces a new passkey); a second owner; `ordo console recover` only from inside the image, logged, ends old sessions |
| Console session | HttpOnly, `SameSite=Strict`, its own origin. Idle and absolute limits as settings |
| Deployment credential | Console key (G14), sealed at rest, short sessions, rotated, revocable from either side |
| Writes at the deployment | Need the owner's own touch (G15). The console key never satisfies step-up |
| Console-only image | No runtime power and no deployment secrets. A stolen session can do only what the door allows, writes behind the owner's touch |
| Page | CSP `default-src 'self'`, no third-party scripts, `frame-ancestors 'none'`, no tokens in storage |
| Console database down | Sign-in stops. Login page and health still answer; the deployment keeps running (5.2) |
| Audit | Console actions in the console log; door actions in the deployment's record (G12); boot record in the deployment's record (G16) |
| Supply chain | Lockfiles, pinned versions, images built in CI, distroless base |

What the console does not defend against: someone who already controls the
container or its host. They control the deployment without it.

---

## 11 · Ready for many deployments later

Every console row and route is keyed by a deployment handle
(`/d/<handle>/tenants`). Open source holds one and redirects `/` to it.
Each deployment has its own console key. The operator later fills the list
and adds a "deployments" screen above. Nothing below it changes.

---

## 12 · Build order

Each step ships: the console works after it, with fewer screens. Sizes per
`AGENTS.md` "Sizing work". A CI round is about 12 minutes.

| Step | What | Size | Pace set by |
|---|---|---|---|
| 1 | **Server, Postgres database, own sign-in, recovery, one image.** `ordo-console`, store and migrations, setup code, passkey sign-in and step-up, recovery codes, second owner, `ordo console recover`, audit log, the page shell, the one Dockerfile | L (2 sessions) | 2 CI rounds; the stack answer (O1) before the page work |
| 2 | **Supervisor.** Spawn, environment and secret files, privilege drop, readiness poll, backoff and crash-loop rules, apply flow with auto rollback, G17, revisions, Health from the supervisor, Runtime screen | L (2 sessions) | 2 CI rounds; a rehearsal that kills the child, crash-loops it and applies bad settings while the pages keep serving; a review of the unix-user split before step 3 |
| 3 | **Link to the deployment.** G14, G15, G16, claim flow, key rotation, Health from the door | L (2 sessions) | 2 CI rounds; a security review between the sessions; a cross-site rehearsal on two domains |
| 4 | **Deploys.** G1, G2, G3; Deploys screen | M | 2 CI rounds |
| 5 | **Tenants.** G4; Tenants screen; usage per tenant | M | 1 to 2 CI rounds |
| 6 | **Settings.** G7; sign-in settings and every setting, editable through restart, drift shown | M | 1 to 2 CI rounds |
| 7 | **Usage, quotas, slow ops.** G9; usage screen; slow ops from `observedCost` | S to M | 1 CI round |
| 8 | **People.** G5; People screens | L (2 sessions) | 2 CI rounds |
| 9 | **Shares, links, security log.** G8, G12 | M | 1 to 2 CI rounds |
| 10 | **Separate images.** `ordo-host` and `ordo-console` builds, door-only mode with the read-only Runtime screen, the second compose file, a test that door-only mode holds no runtime code path | M | 1 to 2 CI rounds |
| 11 | **Ready to launch.** `howto-console.md` (including the door-only statement), compose examples, a usability pass with a less technical person (`plan-usability-testing.md`) | M | the owner's tester and answers |

About 14 to 16 sessions and 16 to 20 CI rounds in all. That is two to three
sessions less than the Docker and Kubernetes plan: no drivers, no Helm
chart, no RBAC. Steps 1 to 3 run in order. After step 3, steps 4 to 10 are
independent; two may run at once on separate worktrees, one owning the
shared shell files. Step 10 needs only steps 1 and 2.

Each step updates `roadmap-checklist.md` 18o and the board in the same
commit, and adds e2e tests for its screens (`howto-e2e.md`).

---

## 13 · What goes to v2

- **Live settings**: `settings-layers` (24a to 24d), so many settings
  change without a restart; app-defined settings, secrets and flags.
- **Caps page** with hit history (G10).
- **Query tracing** and plan views (G13).
- **Chain health**, command-log explorer, group revert (G11).
- **People search** across tenants (G6), with break-glass.
- **More console users** and roles among them.
- **Zero-downtime swap** on one node.
- **Prune** old versions.
- **Plan-variant pane** (`dashboard-plan.md` 5d, `plan-tiers.md`).
- **The operator**: many deployments, provisioning, billing. Private fork.
- **Image rollback** driven by the console (the version is the image's, and
  the platform owns it).

---

## 14 · Decisions

All decisions are ruled. Asks 5 and 7 to 16 were ruled on 2026-10-23, and
asks 6 and 17 the same day (`corpus-rulings.md`, "the console runs inside
the one image"):

- **Stack** (ask 6, "ok sounds good"): door calls go through the console
  server with `ordo-sdk`. The page is plain React. No `ordo-react` data
  hooks.
- **Lockout failsafe** (ask 17, "yes document it"): `ordo console recover`
  inside the image, recovery codes, and a second console owner. The
  command is logged and ends old console sessions.

Step 1a (server, database, sign-in, recovery, audit) is built in
`crates/ordo-console` (checklist 18o).
