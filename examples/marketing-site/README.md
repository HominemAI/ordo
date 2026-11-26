# Marketing site

A landing page that reads its products, pricing and testimonials from Ordo.
Visitors never sign in and the page holds no token. A contact form lets a
visitor leave a message.

## What it shows

- `site.ordo`: public product fields, public plans and testimonials, private
  product notes and enquiries, and these ops:
  - Public reads: `catalogue`, `product` (by slug), `pricing`, `reviews`.
  - One public write: `contact`.
  - For the site owner: `addProduct`, `addPlan`, `addTestimonial`, `enquiryList`.
- `dev/serve.sh` starts a throwaway Postgres and the Ordo server.
- `dev/seed` signs in as the site owner and writes synthesized content (10
  products, 3 plans, 8 testimonials).
- `web/` is a small React page: hero, product grid, pricing table,
  testimonials, contact form. It reads `VITE_ORDO_URL` (the server) and
  optionally `VITE_TURNSTILE_SITE_KEY`.

## What is public, and how it is declared

- A `public` line inside an entity marks every field of that entity.
- `public` at the end of a field line marks that one field.
- `op pricing public = ...` marks an op as callable with no token.
- A public read may read public fields only. That covers its output, its
  conditions and its sort keys.
- An entity that a public op uses has no `acl` and no `access` line.
- Public data lives in the deployment's own space, `system`.
- A visitor calls `GET <door>/op/pricing`. There is no token and no space header.
- Answers are cached, so the database sees only the misses.

The site owner adds content with the ordinary add ops. They carry the owner's
token and `X-Ordo-Space: system`.

## The contact form is a public write

- `contact` only creates a row in `enquiries`. It takes no `id`, no condition and no `$caller`.
- The visitor POSTs JSON to `<door>/op/contact` with no token.
- The answer holds the new row's id, `{"rows":[{"created":"<id>"}]}`.
- The visitor can write and cannot read. `enquiries` has no `public` line.
- The site owner reads enquiries with `enquiryList`, signed in.

## The human check

A public write must prove a person sent it. This is on by default.

- The page loads the Cloudflare Turnstile widget and sends its token in the
  `X-Ordo-Human` header.
- Build the page with `VITE_TURNSTILE_SITE_KEY=<site key>`. The site key is public.
- The deployment owner puts the Turnstile secret key in a file and sets
  `ORDO_PUBLIC_HUMAN_SECRET_FILE` to its path.
- `ORDO_PUBLIC_HUMAN_PROVIDER` picks `turnstile` (the default), `hcaptcha` or `recaptcha`.
- With neither a secret file nor `ORDO_PUBLIC_HUMAN_CHECK=off`, `contact` answers
  403 `human_check_failed`, and the message names these settings.
- The page shows a plain message for `human_check_failed` and `quota_exceeded`.

## The hourly cap

`ORDO_PUBLIC_WRITES_PER_HOUR` caps each public write op, per node. The default
is 10000. `0` turns the cap off. Past it, `contact` answers 429
`quota_exceeded`, and the message names the setting.

## Run it

You need PostgreSQL 15 or later, Node 18 or later, and the two binaries built:
`cargo build -p ordo-cli -p ordo-host`.

```
sh examples/marketing-site/dev/serve.sh     # terminal 1: database and server
sh examples/marketing-site/dev/seed         # terminal 2: content
cd examples/marketing-site/web && npm install
npm run dev                                 # open http://localhost:5183
```

The server listens on `127.0.0.1:4010` and the page on port 5183. The page
origin is fixed because the server's CORS setting names it.

Two things in `dev/` are for local use only:

- `serve.sh` sets `ORDO_PUBLIC_HUMAN_CHECK=off`, so the form works with no
  Turnstile account. A real deployment leaves it unset and sets a secret file.
- The site owner must own `system` to write there. The first `seed` run saves
  the owner's principal id in `dev/.data/owner-principal`. If it says the
  write was refused, stop `serve.sh`, start it again (it reads that file into
  `ORDO_DEPLOYMENT_OWNERS`) and run `seed` again.

`seed` keeps the site owner's guest secret in `dev/.data`, so a second run is
the same owner. It does not add products twice, because a product converges on
its `slug`.

## Notes

- A visitor can create a row and can change nothing else.
- Nothing limits one visitor apart from another. Put a CDN or a proxy limit in front.
- Share links still exist for sharing private rows with a person who has no
  account. See `docs/howto-sharing.md`.

Read next: `docs/howto-public.md`.

## Tests

Products mark their public fields individually. `internalNote` is private and
seeded with `INTERNAL-<slug>` so a leak is visible in a test.

The Playwright suite boots a fresh PostgreSQL database, applies this example,
seeds through the real door and serves the built page. It checks public reads,
private-operation refusals, refinements, tokenless live, space headers, contact
writes, the signed-in path, request counters and the page's response bodies.

```sh
# Build only when no other cargo build is running.
cargo build -p ordo-host -p ordo-cli
(cd examples/marketing-site/web && npm ci)
cd examples/marketing-site/e2e
npm ci
npx playwright install chromium
npx playwright test
```

PostgreSQL 15 or later is required. `ORDO_PGBIN` names its binary directory.
`ORDO` and `ORDO_HOST_BIN` can name prebuilt binaries. The default door and
page ports are 4850 and 5391; override them with `E2E_DOOR_PORT` and
`E2E_PAGE_PORT`. `E2E_STATE` changes the log and state directory. A supplied
`ORDO_PG_HOST` uses that server with a unique database; it never drops an
existing database. These are development keys and a disabled human check.

The stack caps public writes at 1000 per hour. This suite does not exercise
the limit outcome; the separate low-quota door is still needed for that check.
The notes isolation suite checks private-file downloads (S1).

### Public cache checks

The stack also runs door B on port 4851 (`E2E_DOOR_B_PORT` overrides it),
with the same database and keys and a distinct replica id. Both addresses
and the database connection details are in `.stack/stack.json`.

`tests/api/public-cache.spec.ts` checks cache hits against PostgreSQL's table
scan counters, write invalidation on each node, both a comment-only apply and a version bump, and
2 distinct signed-in owners bypassing the public cache. Counter checks wait
for equal samples 11 seconds apart and include a miss control that must
increase the count. Cross-node observations are attached to the test report.
Each test removes the plans it created through the ordinary delete op.

Run just these checks with `npx playwright test public-cache.spec.ts`.
The full suite also retains S2's strict refusal comparisons.

The real version-bump test boots a second throwaway deployment on free ports
and its own database. It keeps failed migrations from disrupting the other
checks. Its logs are test attachments, and its host pids join the coverage
shutdown check. No shared fixture is reset to hide a failed deployment.

## Studio workspace

Open `/studio` to manage the site with a passkey. The deployment owner grants
access to the `system` space. Creating an account alone grants no studio access.
A colleague can paste an invitation ticket before signing in or creating a
passkey. Invitations are issued from Team and expire after 7 days.

- Products, Plans and Reviews: create, edit with revision checks, and delete.
  Saved content appears on the public page. Product staff notes stay private.
- Inbox: search enquiries, keep private notes, track outcomes, and read totals.
  Another editor's changes arrive through the live feed.
- Campaigns: private briefs, JSON tags, exact money budgets, due dates, and
  draft, review, published and archived states. Campaign publication records
  approval; it does not automatically publish a product or testimonial.
- Editorial review: a sandboxed function checks brief completeness without an
  external service or API key. Approval changes state and records its history
  atomically. A stale revision saves neither change.
- Files: upload and download campaign assets. Campaign access controls the
  files too. Deleting a campaign also deletes its assets and reminders.
- Campaign access: grant read, write or admin access to individual members,
  and revoke it. A read grant cannot edit the campaign.
- Reminders: set, move or clear a reminder. The scheduled routine records when
  it fires. Email delivery needs the deployment's mail service configured.
- Team: invitations, roles, suspension and reinstatement. Sensitive actions
  ask for a fresh passkey touch when the door requires it.
- Download studio data: requests the server's own ZIP export, including the
  operations marked `download` and their files, under the caller's access.

The development stack enables an unsafe local file store beside its database.
Use the normal production blob store and authentication settings outside this
throwaway stack. The guest seed identity is not a browser login. Configure the
principal of a real passkey account as a deployment owner to administer a
local site through `/studio`.

## Workspace tests

The real-stack suite adds API and browser workflows for content revision
conflicts, public-field isolation, enquiry reporting and refinement, JSON tag
queries, idempotent creation, file round trips, scheduled reminders, atomic
approval, validation rollback, invitation acceptance, campaign ACL revocation,
role changes, suspension and reinstatement. Browser workflows sign in using a
real WebAuthn ceremony and test live updates without a page reload.

The stack creates a virtual passkey owner before restarting the door with its
principal in `ORDO_DEPLOYMENT_OWNERS`. Its test credentials remain in the
private `.stack` directory and are never uploaded as artifacts. The bootstrap
page uses a temporary origin; the test page starts only after seeding and both
replicas are ready.

```sh
cd examples/marketing-site/e2e
npm ci
npx playwright test -g 'staff|campaign|colleague|workspace|editorial'
```

Run `.github/workflows/examples-e2e.yml` with `coverage=true` to collect Rust
lines executed by this application. The result is measured coverage, not a
promise that a larger feature list reaches 100%. Compare reports built from
the same Rust revision before attributing a percentage change to these flows.

### Lifecycle and real-console journeys

The Workspace selector opens an active tenant's studio. Scheduled reminders
run in tenant workspaces; the deployment's `system` space is excluded from the
routine scheduler. Data can prepare an export or import a bundle that an
operator has staged under the tenant's import directory. This is not a browser
bundle upload or download transport.

The suite starts a real console on port 5392 and a real supervisor on 5394.
The supervisor owns a third real host on 5393. The console links to host A;
its Runtime page restarts only the third host. The journey checks persisted
campaign data after that restart. Local console setup, deployment step-up and
recovery use actual WebAuthn ceremonies and recovery codes.

CI builds `ordo-console` and `ordo-supervisor` alongside the host and CLI, and
installs `clients/ordo-console-web` before running the suite. Ports can be
changed with `E2E_REAL_CONSOLE_PORT`, `E2E_RUNTIME_PORT` and
`E2E_SUPERVISOR_PORT`. The console has its own disposable database.

Additional journeys cover tenant pause/resume/deletion, live origin changes,
secret replacement, operational reads, cookie replay, portability and 257-row
browser results. See `coverage/web-outcomes.md` for validation status. New tests
are not marked passing merely because they compile.

Team also offers a confirmed workspace leave. The program permits members to
leave; the platform still refuses the last owner. A leaving member loses
access while their colleagues keep the shared records.

### File-storage integration profile

The Data panel can attach an operator-configured S3 bucket for new files and
return new files to deployment storage. Existing files stay in their recorded
store. The form accepts a credential label, never secret material, and keeps
files sealed with workspace keys.

Full marketing CI starts the pinned SeaweedFS S3 fixture used elsewhere in the
repository. Its storage journey creates a disposable tenant, uploads before
and after attachment, reads through both replicas, stops only that named S3
container, observes refusal and recovery, then detaches and checks all files.
A local run needs `bash s3-fixture.sh start`, `E2E_S3_ENDPOINT=http://127.0.0.1:8333`
and `E2E_S3_CONTAINER=ordo-marketing-s3`. Stop it with `bash s3-fixture.sh stop`.
Without those settings, local runs explicitly skip this profile; full CI
requires it. Implementation is not yet validated runtime evidence.

### Tenant sign-in connections

Team's Sign-in providers section lets a tenant owner add an OIDC connection,
rotate its secret and retire it with confirmation. The page never loads a
stored secret back into an input. The example fixture enables the supported
`ORDO_AUTH_TENANT_PROVIDERS=on` setting; other deployments decide whether to
enable it. Keep an owner who can sign in without the tenant's own providers.

The connection lifecycle journey checks redaction, duplicate/issuer-change
refusals, foreign-tenant access, retirement on both replicas and re-addition
with a new connection key. It does not claim a successful external-provider
login: its issuer is deliberately unused, and no external call is needed for
these configuration outcomes.

Campaign details also offer file inspection. The page submits each explicit
blob handle to a sandboxed function that checks campaign ownership and reads
through the caller's blob permissions. Up to 32 files are inspected per click;
the page shows their media types and byte counts, not their contents. The
application journeys inspect the corresponding real function-run records as
a deployment owner and verify that a mismatched campaign is refused.
