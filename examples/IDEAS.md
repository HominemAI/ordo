# Example ideas

Apps worth building as examples later. Each line says what the app is, how it
would be built in Ordo, and what it needs that does not exist yet.

The examples that exist today:

- [`first-app/`](first-app/README.md): the smallest program and page.
- [`notes/`](notes/README.md): notes per person, sharing, live updates and a
  copy on the device.
- [`teamspace/`](teamspace/README.md): a team project tracker in one shared
  tenant, with invitations, admins and members, and admin reports.

- [`shopfront/`](shopfront/README.md): one shop, one tenant, three pages on their own origins (sign-in, shop, staff console) joined by the hand-off.

Small examples of the language are in [`syntax/`](syntax/README.md), one
compiling file per kind of syntax, with the ruled-but-unbuilt additions shown
as snippets. Examples of each concept are in [`concepts/`](concepts/README.md).

**Status words:**

- **built**: works today.
- **asked**: waiting on the owner's answer on the board.
- **ruled**: decided, not built.
- **future**: on the roadmap for after the first deployment.

The board holds the real state. Task ids are in brackets.

---

## Public data

### A public marketing site

A landing page with the app's pricing and product catalogue, readable by
anyone without signing in.

- The rows live in the deployment's own space and are declared `public`.
- Public ops read only public fields. The compiler checks that.
- Reads are served from the public cache, and the database sees only misses.
- An anonymous visitor is the reserved anonymous principal at the door.
- **Built:** `examples/marketing-site/` does this on the public path: public
  reads with no token in the page, and a contact form as a public write.
- **Later:** bake the data into a static site at build time, using a
  credential limited to public ops. **Future** [`build-time-credential`].

### A forum

People post, and everyone can read the posts.

- **Multi-tenant app:** each post stays in its author's own space. It reaches
  readers through a feed and the derived public index. Erasure and consent
  follow the author.
- **Single-tenant app:** everyone is in the one space, so a post is simply a
  row there.
- Moderation is the app's own ops and roles.
- **Needs:** the public path, feeds into the derived index. **Ruled**
  [`public-data` asks 8, 22].

### A marketplace

Many shops list products, and visitors search across all of them.

- A shop is a tenant. It grants its listings to the deployment's space.
- The deployment exposes them through a public op.
- Search runs on the derived public index, never a live query across every
  tenant database.
- Product photos are public because files follow their row.
- **Needs:** public path, derived index, public files. **Ruled**
  [`public-data` asks 4, 7].

### A personal profile or blog

Each person has a public page of their own.

- The person chooses what to publish. The published part is granted to the
  deployment's space and served by a public op.
- The rest of their tenant stays private.
- A half-public row works by a public op that projects only its public
  fields.
- **Needs:** public path, a readable address per page. **Ruled**
  [`public-data` asks 2, 3, 5].

### A contact form or waitlist

Visitors leave their email without an account.

- A public op writes into a private entity, write-only. Nobody reads the rows
  back publicly.
- The deployment can also hold a grant for it, like a form or an upload.
- An hourly cap per op and a human check bound the writes. Both are settings.
- A busy address is only ever slowed, in proxy mode, never refused.
- The writes are ordinary writes: checked and logged.
- **Built:** public writes (create only, an hourly cap per op, a human check),
  used by the contact form in `examples/marketing-site/`.
- **Needs:** a grant for the deployment to hold, so a form can land in a
  tenant's space. Not built. **Ruled** [`public-data` asks 6, 17, 25].

### A public status page

Live counts that anyone can watch.

- A public op over the counts, cached.
- Live updates come through one shared stream per page, not one per visitor.
- **Needs:** public path, shared live streams. **Ruled** [`public-data`
  ask 11].

### An embeddable widget

A tenant's widget runs on that tenant's own website.

- The widget calls Ordo from another origin.
- Each tenant sets its own allowed origins, as a setting the app developer
  exposes.
- **Needs:** per-tenant CORS origins. **Ruled** [`public-data` ask 14]. One
  deployment-wide list is **built** (`ORDO_CORS_ORIGINS`).

---

## Deployment shapes

### A single-tenant deployment

One organisation's internal tool, or one person's site.

- One tenant holds everything, and every person signs into it.
- Roles inside the tenant do the work a multi-tenant app does with tenants.
- Good for showing that Ordo works without the tenant machinery in the way.
- **Built**, apart from anything public.
- **Example:** [`shopfront/`](shopfront/README.md). The shop is the
  deployment's own tenant, customers join it by the open way in, and staff
  are the people who hold a level on the shop.

### A team workspace

A company signs up, invites colleagues, and works in one shared tenant.

- Provisioning on sign-up creates the tenant [**built**].
- Invitations bring colleagues in, by any sign-in method, under
  `auto-onboard` or `invite-only` [**built**].
- Passkey sign-in, a second factor and recovery codes [**built**].
- Roles and guards decide who edits what.
- **Example:** [`teamspace/`](teamspace/README.md).

### Two front ends on subdomains, one deployment

A customer app at `app.example.com` and a staff console at
`admin.example.com`, both served by one Ordo deployment. Sign-in happens once,
at `login.example.com`.

- One program, one set of people and tenants. Both front ends call the same
  ops, and guards decide what each person sees.
- One passkey works on every subdomain: `ORDO_APEX_DOMAIN=example.com`, each
  page listed in `ORDO_AUTH_ORIGINS` and `ORDO_CORS_ORIGINS` [**built**].
- Each front end signing in on its own page works today [**built**].
- Signing in at `login.example.com` and landing back, signed in, on the page
  the person came from uses the hand-off [**built**,
  `docs/plan-subdomain-signin.md`]. Wildcard origins (`https://*.example.com`)
  work with `ORDO_WILDCARD_ORIGINS=on`.
- Shows the case for one app over two: shared people and data, one deploy.
- **Example:** [`shopfront/`](shopfront/README.md), with the login page, the
  shop and the staff console on three origins.

### Two apps, one sign-in

The same product split into two deployments, each with its own program, key
and databases, where people sign in once.

- Independent releases and a smaller blast radius per app.
- Needs one deployment to act as the sign-in provider for the others, or all
  of them trusting one outside provider. **Future** [`shared-identity`].
- Needs a way to keep shared data in step across apps (backend functions or
  webhooks). **Future** [`backend-functions`].

### An app whose product is an API

Customers call the app's ops from their own code.

- Each customer gets an API key: a capability token naming the ops it may
  call.
- Quotas are unlimited by default. The app developer sets one if the app
  wants it.
- Keys are revocable and rotate.
- **Needs:** API keys for customers. **Ruled** [`public-data` asks 13, 21].

---

## Sharing

### Shared documents with outsiders

Send a document to someone who has no account.

- A share link opens chosen rows for a time window. 7 days by default when
  no setting says otherwise.
- A guest signs in to see what was shared with them, attributed as
  themselves.
- **Built** [share links L1 to L5, guests G1 to G9].

### Notes, with sharing and live updates

The existing `notes/` example: notes per person, shared notes, live updates
across tabs, and a copy on the device.

- **Built.** A good base to extend with the ideas above, such as publishing
  a note publicly.

---

## Access control

### Projects with access levels

A team app where each project has its own people, at read, write or admin
level.

- One line on the entity, `acl (read, write, admin)`, derives a real access
  entity (`projectsAccess`) with a row per person per project.
- The levels are ordered, so admin includes write and write includes read.
- The access rows are ordinary rows: listed, changed and audited like any
  other.
- **Built.** `notes/` uses it, and so does the conformance fixture
  `crates/ordo-conformance/fixtures/market.ordo`.

### Owner and collaborator rules

Only a task's owner edits it, and anyone on its project reads it.

- Guards on the entity: `access read owner == $caller or
  projectMembers(member == $caller)` and `access write owner == $caller`.
- A guard follows its entity everywhere, so no op can forget it.
- A row the caller may not read answers "not found", never "not yours".
- **Built** (`market.ordo`).

### Roles: admins and members

Admins manage billing and members; everyone else does the daily work.

- Which ops a role may call is decided at the door, before any op runs.
- Guards then decide which rows inside those ops.
- **Built.** **Example:** [`teamspace/`](teamspace/README.md). The door
  already keeps `std.*` ops, invitations and role changes from members. An
  op the program writes is open to every role at the door, so the example's
  reports check the caller's role in their own body (`teamspace/README.md`,
  "What Ordo still owes this app").

### Admin reports across everything

An admin dashboard that totals every project, whoever owns it.

- The op is marked `unguarded`, which bypasses guards for that op's own
  sources only.
- The flag is loud on purpose: greppable, listed in the manifest, and it
  must be repeated by any op that composes it.
- **Built.** **Example:** [`teamspace/`](teamspace/README.md)
  (`reportByProject`, `reportByPerson`).

### Features by plan

Pro tenants see reports that free tenants do not.

- A guard tests the caller's tenant: `$tenant.plan == "pro"`.
- Compiled to one constant per call, so it costs no join.
- **Ruled, not built** [`syntax-additions` ask 5].

### Content for every signed-in person

Help articles or templates the app owns, readable by anyone signed in.

- An entity marked `deployment` lives in the deployment's own space.
- With no guard, every signed-in person reads it. A guard narrows it, for
  example to admins or to pro tenants.
- Marked `public`, it opens to visitors who are not signed in.
- **Ruled, not built** [`syntax-additions` asks 3, 5; `public-data` ask 23].

### An assistant acting for a manager

A manager lets an assistant act on their behalf, within limits.

- A delegation grant gives the assistant some of the manager's rights, never
  more than the manager has.
- **Built**: a worker acts for a person, who stays the caller, and the
  worker is recorded beside them [`delegation`]. Worker credentials of
  their own are **future**.
- **Example**: `examples/assistant/`, an expense approver with an assistant
  that approves the small ones in the manager's name, until the manager
  revokes. The consent ceremony is planned.

---

## Devices and offline

### A field app that works offline

Inspectors fill in reports where there is no signal.

- Reads already come from the device copy when the network is down
  [**built**].
- Writes made offline, queued and sent later, are planned [`offline-plan`].
  **Future**.

### A personal vault

Secrets encrypted on the device, synced across a person's devices.

- Encryption happens on the client, so the server never sees the plaintext.
- **Ruled, queued** [`vault`, `vault-sync`].

---

## Server-side logic

### An app with custom backend code

Send an email when an order ships, or call another service.

- Backend functions: code the deployment runs on events or a schedule.
- **Future** [`backend-functions`].

### An app over an existing database

Put Ordo in front of tables another system already owns.

- External entities: generated, read-only translations of tables Ordo does
  not own.
- **Ruled, not built** [27i, `docs/plan-external-entities.md`].

### A link shortener

Short links for anything the app shares.

- **Future** [`link-shortener`].

---

## Regulated data

### A medical billing platform

A billing tool for small clinics. Each clinic is a tenant. Front-desk staff
register patients and visits, providers code each visit, billers build
claims, send them to insurers, post payments and work denials, and patients
see and pay their statements.

- **The data.** Patients, providers, payers (insurers), visits with diagnosis
  and procedure codes (ICD-10, CPT), claims and their lines, payments and
  adjustments, denials, and patient statements. Charges and payments are
  `money(2)`; a claim's status moves through draft, submitted, accepted,
  denied, paid.
- **Patient details are sealed.** Name, date of birth, insurance member ID and
  notes are `sealed` fields: the database holds ciphertext, and a lookup by
  member ID uses a blind index [**built**].
- **Who sees what.** Roles and groups per clinic: front desk, provider,
  biller, clinic admin, with per-op access set by each clinic
  (`setOpAccess`) [**built**]. A provider sees only their own patients' visits
  through an `acl` on the visit [**built**].
- **Every access is recorded.** Who read or changed which record, in the
  tenant's hash-chained history and the security stream, which a privacy
  audit asks for [**built**].
- **Reports.** Days in accounts receivable, denial rate by payer, collections
  by provider: grouped reads with `let` joins [**built**]. Totals over sealed
  amounts are not built, so amounts stay unsealed [`sealed-folds`, **future**].
- **A patient portal.** Patients sign in with an emailed code or passkey and
  see only their own statements; a statement link can be a share link
  [**built**].
- **A billing assistant.** An AI agent that drafts claims or flags likely
  denials acts through a delegation grant the biller approves [**built**].
- **Needs:**
  - Sending claims to a clearinghouse and reading remittances back (EDI 837
    and 835): outbound calls and file handling in backend code
    [`backend-functions`, **future**].
  - Searching patients and codes by words [`full-text-search`, **future**].
  - The history of a claim's status and amounts, field by field
    [`field-history`, **future**].
  - Proof that a patient's data was erased on request [`erasure-receipt`,
    **queued**].
- **Outside Ordo.** A real deployment handling health data needs hosting under
  a business associate agreement (in the US, HIPAA) and the clinic's own
  policies. Ordo provides the mechanisms; compliance is the deployment
  owner's.
- Synthesized data only: no real patients, codes from the public code sets.
