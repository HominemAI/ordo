# note_viewer

A second example app. It reads the notes that notes v2
(`examples/notes-v2`) holds. It is its own deployment, with its own
definition file (`viewer.ordo`) and its own page (`web/`).

## What it shows

- **Shared sign-in.** Both apps sign people in through Ordo's identity
  service, as an OIDC provider, with open sign-up and a passkey. One person
  has one subject at the service, so one passkey works in both apps.
- **Shared data.** `peer notes` names notes v2. Three `external` lines read
  its `notes` and `notesAccess` tables. Nothing is copied, and nothing is
  written into notes v2.
- **Consent by Postgres.** notes v2 writes `reader note_viewer reads notes,
  notesAccess`. Each app has its own database role. notes v2 grants the
  viewer's role those 2 tables and the sign-in columns. Postgres refuses it
  every other table.
- **Guards across deployments.** `access read entryAccess(member ==
  $caller)` is notes v2's own acl, read from the viewer. On a peer's table,
  `$caller` is the peer's member for the same person. A signed-in person
  sees every note they hold a level on in notes v2: their own, and the ones
  shared with them.
- **A public list.** A note tagged `public` in notes v2 shows to anyone,
  with no sign-in. `publicNotes` is a public op over its own external,
  `published`, which has no guard. The op filters on the tag.

What it leaves out:

- A note shared with a group is not shown. notes v2's acl also follows
  group membership; this guard reads `member == $caller` only.
- A peer row matches the person's member in any of notes v2's spaces
  (`docs/definition-language.md`, "Known gap").
- notes v2 must keep people in its own space. A peer reads the deployment's
  database, never a tenant's own database, so `ORDO_PROVISION_SELF_SERVE`
  stays off.

## Run both apps locally

The quickest way is the end-to-end stack. It starts Postgres with the two
roles, the identity service, both doors and both pages:

```
cargo build -p ordo-host -p ordo-cli -p ordo-identity-service
(cd examples/notes-v2/web && npm ci)
(cd examples/note-viewer/web && npm ci)
(cd examples/note-viewer/e2e && npm ci)
bash examples/note-viewer/e2e/stack.sh
```

notes v2 is at http://localhost:5290 and note_viewer at
http://localhost:5291. The identity service is at https://localhost:4791,
with the repository's test certificate. A browser refuses a passkey on a
page whose certificate it does not trust. Start Chrome with that one key
trusted, in a profile of its own:

```
open -na "Google Chrome" --args --user-data-dir=/tmp/ordo-fed \
  --ignore-certificate-errors-spki-list=dmjOrASrxqt8nLqNAaxP7LuJUpdPzc/F+u3D3xlxBMs=
```

The three servers all set cookies on `localhost`. Use one browser profile
for one person.

### Why two roles

Each app runs as its own Postgres role, and that is what makes the
`reader` line mean something. With one role for both, notes v2 refuses to
start (its reader names its own role) and note_viewer refuses to start (it
reads the peer with no grant). `ORDO_PEER_SHARED_ROLE=on` on both lets
them run as one role, for local work. Then no grants are made and Postgres
enforces nothing. `ordo dev` turns it on. The stack above does not.

## The settings each app needs

Both apps, besides the usual door settings (`docs/settings.md`):

| Setting | notes v2 | note_viewer |
|---|---|---|
| `ORDO_PG_USER` | its own role, `notes_app` | its own role, `viewer_app` |
| `ORDO_PG_SCHEMA` | `notes` | `viewer` |
| `ORDO_PG_DB` | the same database | the same database |
| `ORDO_AUTH_METHODS` | `accounts`, beside any other way in | `accounts` |
| `ORDO_AUTH_ACCOUNTS_KIND` | `oidc` | `oidc` |
| `ORDO_AUTH_ACCOUNTS_ISSUER` | the service's `ORDO_IDENTITY_ORIGIN`, plus its `ORDO_IDENTITY_BASE_PATH` if it has one | the same |
| `ORDO_AUTH_ACCOUNTS_CLIENT_ID` | `notes`, its handle at the service | `viewer` |
| `ORDO_AUTH_ACCOUNTS_CLIENT_AUTH` | `none` | `none` |
| `ORDO_PUBLIC_URL` | its https address; the callback is under it | the same |
| `ORDO_AUTH_PROVISIONING` | `auto-onboard` | `auto-onboard` |
| `ORDO_READER_NOTE_VIEWER_ROLE` | `viewer_app` | |
| `ORDO_PEER_NOTES_SCHEMA` | | `notes` |

At the identity service, register each app with open sign-up and its
callback (`docs/howto-identity.md`, "Connect an app to the identity
service"):

```
ordo-identity-service deployment add notes \
  --return <notes ORDO_PUBLIC_URL>/auth/callback/accounts --sign-up open
ordo-identity-service deployment add viewer \
  --return <viewer ORDO_PUBLIC_URL>/auth/callback/accounts --sign-up open
```

Give no `--origin`. A client origin may spend the person's session cookie
at the service, which an app's page must not. An app registered with
`--origin` before 2026-11-19 drops it with `deployment set notes
--no-origins`.

Register before the service starts, or wait up to a minute for a running
service to read it.

In each app, the deployment owner opens the deployment's own space to
people who sign in, with `std.space.setAdmission` (`open`, and the role:
an editor in notes v2, a viewer in note_viewer). The stack writes that
row directly.

Start notes v2 and apply it first. note_viewer refuses to start until
notes v2 has started once with sign-in on, and until notes v2's grants
are made.

The pages:

| Build setting | notes v2 (`examples/notes-v2/web`) | note_viewer (`web/`) |
|---|---|---|
| `VITE_ORDO_URL` | notes v2's door | note_viewer's door |
| `VITE_ORDO_SPACE` | `system`, so a sign-in arrives in the deployment's own space | `system` by default |
| `VITE_ORDO_CONNECTION` | | `accounts` by default |

After a change to `viewer.ordo`, run `npm run gen` in `web/`.

## Tests

`e2e/` is the federation end-to-end suite. `docs/howto-e2e.md`, "Two
deployments", says what it covers and how to run it.
