# How to run the notes end-to-end tests

The notes app is tested end to end against a throwaway stack (owner ruling,
2026-10-19). Each run boots a fresh deployment, runs every use case, and
throws the deployment away.

The tests live in `examples/notes/e2e/`. The workflow is
`.github/workflows/e2e.yml`.

## What a run does

The stack is `examples/notes/e2e/stack.sh`. It starts, in order:

1. **Postgres.** A cluster of its own, or the one `ORDO_PG_HOST` names (CI's
   service container). The deployment's database is made fresh each run.
2. **The stand-ins** (`stand-ins.mjs`, one Node process):
   - a stand-in Discord over https, with an authorize page that asks for a
     user name and has Allow and Deny;
   - the door's https callback address, which forwards to the door;
   - a mail catcher: SMTP in, and `GET /messages` to read what came.
3. **The real `ordo-host`**, set as `examples/notes/README.md` §6 says:
   passkey, emailed code, Discord and guest sign-in, auto-onboard, a tenant of
   one's own on sign-up, and CORS for the page.
4. **`ordo apply`** of `examples/notes`.
5. **The page**, built by Vite and served by `vite preview`.

Then Playwright runs five projects:

| Project | Browser | What it tests |
|---|---|---|
| `api` | none | The door at request level: the ways in it lists, CORS, an emailed code, the Discord stand-in round trip, and one person's notes kept from another |
| `browser` | Chromium | The page with a virtual passkey device (Chrome DevTools' WebAuthn domain): `tests/browser` (passkey, emailed code, Discord, guest) and `tests/ui` (write, edit, delete, tags, share, offline) |
| `phone` | Chromium as a Pixel 7 | `tests/phone`: sign up, write, edit, delete and share-accept on a narrow touch screen. Each step checks that the page does not scroll sideways and that the buttons are on screen |
| `firefox` | Firefox | `tests/ui` only |
| `webkit` | WebKit (Safari's engine) | `tests/ui` only |

Two more projects, `screenshots` and `screenshots-phone`, compare pictures of
key screens. They run in a separate, report-only step (see "Screenshots").

The virtual passkey device is Chromium only. Firefox and WebKit people sign in
with an emailed code (`join` in `tests/people.ts`, which reads the code from
the mail catcher). Passkey specs sit in `tests/browser`, which only the
`browser` project runs. Sharing a note asks for a fresh passkey touch, so in
`tests/ui` the person who shares (`owner`) is always a Chromium person, and
the reader is the project's browser.

A Firefox gap, found and fixed on 2026-10-20. With a second tab open, or
while the reader held another live feed, a Firefox page's feed request
(`GET /op/myNotes?live=1&sync=1`) after a write stayed pending. The
space rides in a header, so both feeds had one URL, and Firefox made the
second wait on the HTTP cache entry that the first, endless, stream held. The
door now answers members with `Cache-Control: no-store` and the clients open
streams with `cache: 'no-store'`. The two tab cases and the shared note case
run in Firefox again.

A new person's tenant is made after the sign-up answers (owner, 2026-10-21).
The page shows "Setting up your account" until the door says it is ready.
`signedIn` in `tests/support.ts` waits that out for up to two minutes, the
SDK's own wait, so a slow sign-up under load is slow rather than failed. A
test runs for up to three minutes for the same reason. Each door log line
`tenant <id>: provisioned in N ms (...)` gives the steps of one sign-up.

`tests/ui` is the same use cases in three desktop browsers. A use case there
shows up three times in the job summary, once per project.

Each test is one use case, named in plain words. A failure says which use case
broke.

The door trusts the stand-in's test CA through `ORDO_AUTH_PROVIDER_CA_FILE`
(`docs/howto-sign-in.md` §8). Real Discord is checked by hand.

## Run it on your machine

You need Rust, Node 20 or later, and PostgreSQL 15 or later binaries
(`ORDO_PGBIN` if they are not at `/opt/homebrew/opt/postgresql@17/bin`). No
Docker.

One command, from the repository root:

```
bash examples/notes/e2e/run.sh
```

It builds `ordo-host` and `ordo`, installs the page and the tests, installs
Chromium, Firefox and WebKit for Playwright, and runs every use case. It ends with a table of
the use cases, passed or failed.

Some variants:

```
bash examples/notes/e2e/run.sh --project=api        # no browser
bash examples/notes/e2e/run.sh --project=webkit     # one browser (browser, phone, firefox, webkit)
bash examples/notes/e2e/run.sh --project=phone -g "edits a note"
bash examples/notes/e2e/run.sh -g "shared note"     # one use case by name
E2E_SKIP_BUILD=1 bash examples/notes/e2e/run.sh     # binaries already built
```

For a quick loop, keep the stack up in one terminal and run the tests in
another. Playwright reuses a stack that is already running:

```
bash examples/notes/e2e/stack.sh                    # terminal 1
cd examples/notes/e2e && npx playwright test        # terminal 2
```

The stack prints its addresses. Open the page it names in a browser to look
around. Ctrl-C stops everything and removes its Postgres cluster.

On macOS the Firefox build Playwright ships may refuse to start ("Could not
find profile folder"). If it does, run `--project=firefox` on Linux or on
GitHub; the other projects do not need it.

The ports are 4640 (door), 5190 (page), 4643 (stand-in provider), 4625 and
4626 (mail). Each has an `E2E_*_PORT` variable in `stack.sh`. The stack
refuses to start while a port is in use.

`E2E_HUMAN_CHECK=1` turns on Turnstile with Cloudflare's public always-pass
test keys. It needs the internet, so it is off by default.

## Run it from GitHub

When CI runs (owner ruling, 2026-11-01):

- **A push to `bleeding`** runs what the change reaches.
  `scripts/ci-affected.mjs` works it out from the diff. A job it does not
  reach still runs, says "Not reached by this change" and passes.
- **A pull request to `main`** runs every job in full. Every job is a
  required check. `main` moves only through this pull request.
- **Nightly**, 09:00 UTC, every job runs in full on `main`.
- **A full run on `bleeding`**: put the line `ci: full` in a commit
  message, as a trailer. Or start the workflow by hand (below).

The notes e2e runs on a push when the change reaches ordo-host, ordo-cli
or a crate they build on, `examples/notes`, `examples/notes-v2`,
`clients/ordo-sdk` or `clients/ordo-react`.

To run it by hand:

1. Open the repository on GitHub, then **Actions**.
2. Pick **Notes end-to-end** on the left.
3. Press **Run workflow**. Pick the branch.
4. **target**: `throwaway` builds a stack from that commit. `staging` is
   phase 2 and only says it is not built yet.
5. **browser**: on runs every project (Chromium, phone, Firefox, WebKit) and
   the request-level tests. Off runs the request-level tests alone.

A run takes about 10 to 15 minutes. Most of it is the Rust build. The job
installs the three browsers with their system libraries
(`npx playwright install --with-deps chromium webkit firefox`). Measured on
the self-hosted runner, 2026-10-20 (warm Rust cache): 2 minutes to install
the browsers, 1.7 minutes of tests, 4 minutes for the whole job. Every
project runs whenever the job runs.

## Where the results are

- **The job summary**, on the run's page: a table with each use case, passed
  or failed, and the first line of each failure.
- **The artifact `notes-e2e-report`**, at the bottom of the run's page. It
  holds:
  - `report/html/index.html`, Playwright's report;
  - `test-results/`, with a trace, a screenshot and a video of each failed
    test;
  - `.stack/*.log`, the door, the stand-ins, Postgres and the page.

To open a trace: `npx playwright show-trace <trace.zip>`, or drop the file on
<https://trace.playwright.dev>. A trace shows every step, the page at each
step, the network calls and the console.

Locally the same files are under `examples/notes/e2e/`.

## Screenshots

Seven screens are compared with a stored picture (owner ruling, 2026-10-21).
They block nothing yet. The owner's concern is false flags, so every measure
below exists to keep a picture from changing when the page did not.

### What they cover

Chromium, desktop window of 1280 by 720 (project `screenshots`):

1. the sign-in screen;
2. the notes list with three notes and their tags;
3. a note open in the editor;
4. the share panel with an invitation made;
5. "Shared with you" for a reader who accepted an invitation.

Chromium as a Pixel 7 (project `screenshots-phone`): the sign-in screen and
the notes list. The specs are in `examples/notes/e2e/tests/screenshots/`. The
pictures are in `examples/notes/e2e/screenshots/<project>/`.

### How false flags are kept out

- **One place makes the pictures.** The CI runner's container makes and checks
  them. Fonts and text drawing differ on a Mac, so locally the specs skip unless
  `E2E_SCREENSHOTS=1` is set. Pictures made on a Mac must not be committed.
- **A fixed window.** Desktop is 1280 by 720 at scale 1. The phone is a Pixel 7.
  The locale is `en-US`, the time zone UTC, the colours light, and motion is
  reduced.
- **No motion, no caret.** `animations: 'disabled'` and `caret: 'hide'` are set
  for every picture, in `playwright.config.ts`.
- **A resting page.** Before each picture the mouse is moved off the page, the
  focused element loses focus, the page is scrolled to the top, and the fonts
  are loaded.
- **Fixed content.** Each test signs up its own person under a fixed name
  (`Ada`, `Bo`) and writes notes with fixed titles. The main content is not
  masked, it is the same every run.
- **Order that cannot tie.** A note is stamped to the second, so the notes are
  written 1.1 seconds apart. The list then has one order.
- **Masks.** Anything that changes per run is painted over with a solid colour:
  the footer (it holds the account id) in every picture, and the invitation
  link in the share panel. No date shows on these screens, so no fixed clock is
  needed. If a screen ever shows a time, mask it or set `page.clock.setFixedTime`.
- **A small tolerance.** `maxDiffPixelRatio` is 0.01: up to 1 pixel in 100 may
  differ.
- **No retry and no block.** A change never fails the job (next section).

### They block nothing yet

CI runs the screenshot projects in their own step, "Screenshots (report only)",
with `continue-on-error: true`. A change shows as a warning on the step, and the
job stays green. The step has its own stack, report and results, so it cannot
hide or overwrite the main run. When the owner decides they should block, remove
`continue-on-error` from that step.

### How to see a difference

- The job summary has a section "Screenshots (report only)". It lists each
  screen as matched or changed.
- For a change, the artifact `notes-e2e-screenshots` holds the expected, actual
  and diff pictures under `test-results-screenshots/`. The summary names each
  file.
- Open `report-screenshots/html/index.html` in the same artifact to flip
  between them.

### How to update the pictures

CI never commits. A person does.

1. Open **Actions**, then **Notes end-to-end**, then **Run workflow**.
2. Pick the branch and turn on **update-screenshots**.
3. When the run ends, open the job summary. It lists each picture written.
4. Download the artifact `notes-e2e-screenshots` and copy its `screenshots/`
   folder over `examples/notes/e2e/screenshots/`.
5. Look at the pictures. Commit them.

The run uses `--update-snapshots=changed`. It rewrites a picture that differs
and makes one that is missing. Pictures that match stay as they are.

To run the screens on a Mac anyway, to try a spec:
`E2E_SCREENSHOTS=1 bash examples/notes/e2e/run.sh --project=screenshots`.
Expect a mismatch with the CI pictures.

## Add a use case

1. Pick the folder. A use case that needs a passkey goes in
   `tests/browser/` (Chromium only). A use case any desktop browser must
   show goes in `tests/ui/` (Chromium, Firefox and WebKit). One for a narrow
   touch screen goes in `tests/phone/`. One the door alone answers goes in
   `tests/api/`.
2. Add a `test('...')` whose name says the use case in plain words, such as
   `a person shares a note with a link and a stranger reads it`.
3. Use the helpers in `tests/support.ts` and `tests/people.ts`. In
   `tests/ui/` use `join` (a new person by passkey or emailed code, whichever
   the browser can do), `joinByEmail` (two tabs of one person), `owner` (a
   Chromium person who shares) and `fitsWidth` / `reachable` (phone layout).
   The rest: `person` (a browser profile with a
   passkey device), `signUp`, `signIn`, `signOut`, `addNote`, `noteInList`,
   `codeFor` (a code from the mail catcher), and `unique` (a name no other
   test uses). The page has no timer and no focus handler: a change reaches it
   on its open feed, so a test waits for it with `expect(...)` and nudges
   nothing.
4. Each test signs up its own people. Never rely on another test's data:
   tests run side by side.
5. Find things the way a person does: by role and visible label
   (`getByRole`, `getByLabel`). If the page has no label to find, that is a
   finding about the page.
6. Run it locally with `-g "<name>"`, then the whole set, before you push.

A use case that needs a setting the stack does not set: add the setting to
`stack.sh`, in the door section, with a comment saying why.

## Notes v2 over a live v1

The upgrade check runs on the same stack. `examples/notes-v2` holds the v2
program and page. `stack.sh` builds the v2 page in its own step and serves it
under `/v2/` on the same server and origin as v1, so what a browser stored
under v1 is there under v2. `stack.json` names it as `pageV2` and the program
as `programV2`.

The `upgrade` project runs `tests/upgrade/v1-to-v2.spec.ts` in Chromium. Two
people sign up under v1, write notes with tags and share one. The spec then
applies v2 with `applyAndServe`, which waits until the door serves v2 and has
brought every tenant to it. Both people open the v2 page and find every note,
tag and the share, and write a new note. Then they use what v2 adds:

- every v1 note reads unpinned and not archived (the new defaults);
- a pinned note goes first, and an archived one waits behind a filter, both
  with the network off too;
- an image and a text file go on the shared note; the image shows and the
  file downloads with the bytes it was sent with;
- the v1 share shows no file, and the same note shared again under v2 shows
  them to the reader;
- "Download all my notes" gives a zip that holds every note and the image;
- a removed file leaves the owner's and the reader's view.

v2's schema changes are all additive, class A: a field with a constant
default, an optional field and a new entity. `ordo apply` makes them itself.

Files go to a blob store on disk under the stack's folder
(`ORDO_BLOB_STORE=fs:...` with `ORDO_DEV_UNSAFE_BLOB_STORE=1`, set in
`stack.sh`). It changes the program for the
whole stack, so it depends on every other project and runs last. To run it
alone, use `--project=upgrade --no-deps`. `E2E_PROGRAM` still starts the stack
on another program.

The `v2` project runs `tests/v2/` after `upgrade`, in Chromium. It covers
what v2 added later:

- search finds a person's own notes and the notes shared with them, by
  title, text and tag, and never a note that was not shared, with the
  network on and off;
- a share link with a password asks for it, refuses a wrong one, and opens
  with the right one, in a browser with no session.

Run alone with `--project=v2 --no-deps`: it applies v2 itself when the
stack still serves v1.

## The console

The Ordo console has its own end-to-end tests, in
`clients/ordo-console-web/e2e/`. The workflow is
`.github/workflows/console-e2e.yml`. It runs on a push to `bleeding` when
the change reaches ordo-console, ordo-cli, ordo-identity-service,
ordo-supervisor or `clients/ordo-console-web`, and in full on a pull
request to `main` and nightly.

The stack (`stack.sh`) is smaller than the notes one:

1. **Postgres**: the console's own database, made fresh.
2. **The page**, built by Vite into the stack's folder.
3. **The real `ordo-supervisor`**, running `ordo-supervisor-fake-child` as
   the deployment, on `127.0.0.1:4681`. The fake child is a stand-in for
   `ordo-host`. Its settings say how to behave: `ORDO_FAKE_MODE` (healthy,
   unready, crash, hang) and `ORDO_FAKE_CHECK=refuse`. The identity service
   and console children are the same stand-in, idle on spare ports. The
   grace period is 1 second and the ready timeout 5, so a failed change
   rolls back in seconds. Each start adds a line to `.stack/starts`.
4. **The real `ordo-console`** on `http://localhost:4680`, beside the
   supervisor rather than under it, so it keeps its own sign-in for these
   tests. It serves the page and reaches the supervisor's socket
   (`ORDO_CONSOLE_SUPERVISOR_SOCKET`). Its data folder is in the stack's
   folder, so a test reads the first-run setup code from the file
   `setup-code`.
5. **The deployment's door** on `127.0.0.1:4682`: `ordo-host`'s own
   sign-in routes and console key ops over memory stores
   (`crates/ordo-console/examples/e2e_door.rs`, built by
   `cargo build -p ordo-console --example e2e_door`). The console reaches
   it through `ORDO_CONSOLE_DOOR_URL`. The console links to it by
   sign-in; there is no claim code (removed 2026-10-30).

The stack clears every `ORDO_*` variable before the supervisor starts,
because the deployment's revision 1 is built from them.

Build first: `cargo build -p ordo-console -p ordo-cli
-p ordo-identity-service -p ordo-supervisor`, and `cargo build -p
ordo-console --example e2e_door`.

Four Playwright projects, all Chromium, because every screen needs a
passkey and the virtual passkey device is Chromium only:

| Project | What it tests |
|---|---|
| `browser` | In order: first run with the setup code and the 10 recovery codes; sign out and in; invite a second owner, who joins, then remove them; a lost passkey, a recovery code, a forced new passkey and new codes, the old codes refused |
| `runtime` | In order: Health and Runtime on revision 1; a good change; one the settings check refuses, with nothing running touched; one that crashes and rolls back; a roll back from the list; a managed secret whose value no page or API answer shows; a killed process started again while `/health/ready` keeps answering 200 |
| `phone` | A Pixel 7: sign-in, a code from `ordo console recover`, the codes, Health, Runtime and Owners, none scrolling sideways |
| `door` | The link to the deployment by sign-in, a write that asks for the owner's touch at the deployment (in a dialog), deploy and roll back the app, unlink |
| `identity` | The console on the identity service, against its own stack (`stack-identity.sh`: the real `ordo-identity-service`, the console with `ORDO_IDENTITY_URL` set in door-only mode, and the e2e door verifying the service's tokens with an 8 second touch window, `E2E_DOOR_TOUCH_SECS`). Sign up by the first invite, sign in to the console at the service and come back; pause a tenant, which asks for a fresh touch the page takes at the service; invite a second admin, who joins; end their access, after which their console sends them back to sign in |

Every step also fails on a CSP refusal or a page error. The console holds
one deployment and at most two owners, so the tests run in order in one
worker: `runtime` after `browser`, `phone` after it, and `door` last.

Run it:

```
bash clients/ordo-console-web/e2e/run.sh
E2E_SKIP_BUILD=1 bash clients/ordo-console-web/e2e/run.sh --project=phone
```

Ports in use elsewhere: set `E2E_CONSOLE_PORT`, `E2E_HOST_PORT` and
`E2E_DOOR_PORT`; for the identity stack, `E2E_ID_CONSOLE_PORT`,
`E2E_ID_DOOR_PORT` and `E2E_ID_PORT` (4683, 4684, 4685). Playwright starts
both stacks whichever project runs.

It needs Rust, Node 20 or later and PostgreSQL binaries, as above. The
artifact `console-e2e-report` holds the report, traces and
`.stack/console.log`.
