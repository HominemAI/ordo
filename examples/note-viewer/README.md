# note_viewer

A second example app. It reads the notes that notes v2
(`examples/notes-v2`) holds. It is its own deployment, with its own
definition file (`viewer.ordo`) and its own page (`web/`).

## What it shows

- **Shared sign-in.** Both apps sign people in through Ordo's identity
  service, as an OIDC provider, with open sign-up and a passkey. One person
  has one subject at the service, so one passkey works in both apps.
- **Data through the door.** `peer notes { std.notes.list  publicNotes }`
  names notes v2 and the 2 of its ops this app calls. Each read is a call to
  notes v2's door. Nothing is copied, nothing is written into notes v2, and
  this app's database role is granted nothing there.
- **Consent by grant.** notes v2 writes `grant note_viewer as itself { … }`,
  opening those 2 ops to this app, tied to its id at the identity service by
  `ORDO_CONSUMER_NOTE_VIEWER_CLIENT`. notes v2's door refuses every other op,
  by name.
- **notes v2 decides who sees what.** A signed-in person's list is
  `std.notes.list`, called for that person: notes v2's acl answers every
  note they hold a level on, their own and the ones shared with them.
- **A public list.** A note tagged `public` in notes v2 shows to anyone,
  with no sign-in. The external `published` reads notes v2's op
  `publicNotes`, called as this app itself.

What it leaves out:

- notes v2 keeps people in its own space here, so `ORDO_PROVISION_SELF_SERVE`
  stays off. A tenant in a database of its own reaches a peer the same way
  (`crates/ordo-host/tests/peer_reads.rs`).

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

Each app runs as its own Postgres role in its own schema, and neither is
granted anything in the other's. The roles are not what keeps the data
apart: the door is. They show that this app needs no database rights in
notes v2 at all.

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
| `ORDO_IDENTITY_DEPLOYMENT` | `notes` | `viewer` |
| `ORDO_CONSUMER_NOTE_VIEWER_CLIENT` | `viewer` | |
| `ORDO_PEER_NOTES_URL` | | notes v2's door |
| `ORDO_PEER_NOTES_DEPLOYMENT` | | `notes` |
| `ORDO_PEER_NOTES_SPACE` | | `system` |

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

Start notes v2 and apply it first. Then, once, a deployment owner of
note_viewer registers its consumer key at the identity service with the door
op `std.peer.registerKey`, passing their own token for `viewer`
(`e2e/tests/door`). note_viewer's apply through its door then checks its
`peer notes` block against notes v2's grant and records its use there.

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
