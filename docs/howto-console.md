# How to run the Ordo console

For the deployment owner. Built in checklist 18o. The design is
`plan-console.md` and `plan-identity-service.md`; the compose file is
`docker/compose.one.yml`.

Starting from nothing? [`first-deployment.md`](first-deployment.md) takes you
from the secrets to a signed-in console and a deployed program. This page
is the console's reference. Deployment people, invites, machine keys and
the `ordo login` command line are in
[`howto-identity.md`](howto-identity.md).

The console is a web page for the person who runs an Ordo deployment. It
shows how the deployment is doing and lets you change it without a
terminal.

## First run

1. Start the one image with `docker/compose.one.yml` (see "The one image"
   below).
2. Read its log for the first invite link:
   `docker compose -f docker/compose.one.yml logs ordo | grep -i invite`.
3. Open the link in a current browser. It lands at the identity service,
   by default under the console's own address at `/identity`.
4. Make a passkey when the browser asks.
5. Save your recovery codes. They are shown once.

Facts about sign-in:

- People sign in at the identity service, with passkeys. The console has no
  setup code of its own.
- The first invite link works once. The first person to use it becomes the
  first admin.
- Passkeys are tied to the console's host (the RP ID). Keep
  `ORDO_CONSOLE_ORIGIN` as it is: `howto-identity.md`, section 9, says what
  a change of host costs.

## Recovery codes, a second admin, a lockout

These belong to the identity service now, and `howto-identity.md` covers
each:

- Recovery codes: 10 at sign-up, each works once and only to make a new
  passkey (section 8).
- A second admin: **Deployment people**, "Invite an admin" (section 4).
  Add one soon, so one lost passkey does not lock you out.
- Every admin locked out: the host command
  `ordo-identity-service invite` (section 8).

### Without the identity service

A deployment that names no identity database keeps the console's own
sign-in (`howto-identity.md`, section 2). There:

- The console has its own recovery codes, 10 per owner, and its **Owners**
  screen invites a second owner with a one-time code. Up to 2 people own
  the console.
- The lockout failsafe runs inside the image and prints a one-time code,
  valid 1 hour:

  ```
  docker compose -f docker/compose.one.yml exec ordo ordo console recover
  ```

  Add `--owner <id>` for one named owner. Enter the code on the sign-in
  page and make a new passkey. It is written to the console's audit log.

## The console and the deployment

The console manages the deployment through the supervisor's socket
(`ORDO_CONSOLE_SUPERVISOR_SOCKET`). It calls the door as the signed-in
person, with the token the identity service gave them. There is no link
step and no console key at the deployment to make or rotate.

Open **Deployment** to see whether the door is reachable. The screen asks
one thing: "Is the door reachable?". If not, check `ORDO_CONSOLE_DOOR_URL`
and that the deployment is running.

Without the identity service, the console keeps its old link step: it
links to the door once with your deployment passkey, and then signs in
with a console key of its own. Your browser runs that passkey from the
console's page, so the deployment must list the console's address, in
`ORDO_AUTH_ORIGINS` when the console shares the deployment's domain, or
`ORDO_AUTH_RELATED_ORIGINS` when it does not. The screen names the address
to add.

## The touch at the deployment

The console can read, and can do some writes. It can never stand in for
you. Anything that changes the deployment needs a fresh touch of your own
passkey. With the identity service the touch is made there and rides in
your token; without it, it is made at the deployment.

- A box asks: "Touch your passkey for the deployment".
- One touch covers the next 5 minutes of changes.
- The console never satisfies it for you. This is on purpose.

## Each screen in a line

| Screen | What it is for |
|---|---|
| Health | Is the deployment up and ready; database, key provider, nodes, quarantined tenants |
| Runtime | The deployment's status, logs, settings, secrets as set or unset, restart, revisions and roll back. Read-only in door-only mode |
| Deploys | Apply a program, see stored versions, roll back the app |
| Deployment | Is the door reachable; see your touch |
| Tenants | Create, pause, resume, delete, export and attach a bucket |
| People | Members, roles, groups, access, invitations, who may join, recovery (inside a tenant where you hold a role) |
| Sign-in | The deployment's and a tenant's sign-in settings |
| Settings | Every setting, with the value the deployment took and where it came from |
| Usage | Usage per tenant, quotas, and the slowest ops |
| Shares | Links and shares in a tenant, and the security log |
| Deployment people | People and machine keys at the identity service: invite, end access |
| Owners | Only without the identity service: console owners; invite a second one |

Two things have the word "rollback". **Roll back the app** (Deploys) goes
to an older program. **Roll back the server** (Runtime) goes to older
settings. They are different acts.

## Settings that matter

These are the console's own settings. Every other setting belongs to the
deployment and lives on the Runtime screen.

| Setting | Default | What it does |
|---|---|---|
| `ORDO_CONSOLE_DATABASE_URL` | none; boot refuses | The console's own Postgres database |
| `ORDO_CONSOLE_DATABASE_URL_FILE` | none | A file that holds the URL. Use it: the URL has a password. Setting both refuses the boot, naming this one |
| `ORDO_CONSOLE_ORIGIN` | none; boot refuses | The address you open the console at. Passkeys are tied to it |
| `ORDO_CONSOLE_RP_ID` | the origin's host | The console's passkey scope |
| `ORDO_CONSOLE_LISTEN` | `127.0.0.1:8081` (the images set `0.0.0.0:8081`) | Where the server listens |
| `ORDO_CONSOLE_SUPERVISOR_SOCKET` | none; the supervisor sets `/run/ordo/supervisor.sock` | The supervisor's socket. With it the console manages the deployment (status, logs, apply, rollback, restart). Without it the console is in door-only mode |
| `ORDO_CONSOLE_DOOR_URL` | `http://127.0.0.1:8080` in the image; none otherwise | The deployment's door |
| `ORDO_CONSOLE_DOOR_PLAIN_HTTP` | `off` | `on` allows plain http to a door that is not loopback. Use it only on a private network |
| `ORDO_CONSOLE_DOOR_SPACE` | `conformance` | The deployment's own space. Set it to the deployment's `ORDO_SPACE` (usually `system`) |
| `ORDO_CONSOLE_DEPLOYMENT` | `main` | The handle of the one deployment |
| `ORDO_CONSOLE_DATA_DIR` | `/var/lib/ordo-console` | The console's volume: `console.key`. Mount it as a persistent volume |
| `ORDO_CONSOLE_KEY_FILE` | `console.key` in the data dir | The key that seals secrets in the console database |
| `ORDO_CONSOLE_SESSION_IDLE_SECS` | 1800 (30 minutes) | Without the identity service: unused time before a console session ends. With it, sessions are the service's (`ORDO_IDENTITY_SESSION_*_HOURS`) |
| `ORDO_CONSOLE_SESSION_MAX_SECS` | 43200 (12 hours) | Without the identity service: longest a console session may live |
| `ORDO_CONSOLE_DATABASE_POOL_SIZE` | 4 | Connections to the console database |
| `ORDO_CONSOLE_RUN_DIR` | `/run/ordo` | Where the supervisor puts its socket and the console's secret files. Mount a tmpfs |

A bad value refuses the boot. The message names the setting and the value
to set.

Settings the console no longer reads:

- `ORDO_CONSOLE_MODE`: gone. The console says "remove the line". Whether it
  manages the deployment now depends on `ORDO_CONSOLE_SUPERVISOR_SOCKET`.
- `ORDO_CONSOLE_CHILD_USER`, `ORDO_CONSOLE_CHILD_GRACE_SECS`,
  `ORDO_CONSOLE_CHILD_READY_SECS` and `ORDO_CONSOLE_HOST_BIN`: the
  supervisor reads them as defaults of its own `ORDO_SUPERVISOR_*`
  settings.
- `ORDO_CONSOLE_KEY_ROTATE_DAYS`: there is no console key at the deployment
  to rotate.

The identity service's settings are `ORDO_IDENTITY_*`. Two matter here:
`ORDO_IDENTITY_BASE_PATH` is the path it is served under (`/identity` by
default), and `ORDO_IDENTITY_DATA_DIR` is its volume.

## The one image

One image, `ordo`, with the compose file `docker/compose.one.yml`. Its
entrypoint, `ordo-supervisor` (unix user 65530), runs 3 children as
siblings, each its own unix user:

| Child | Unix user | What it does |
|---|---|---|
| The identity service | 65531 | Passkey sign-in for people |
| The console | 65532 | This page; manages the deployment |
| The deployment, `ordo-host` | 65533 | Your program |

- The console manages the deployment through the supervisor's socket
  (`ORDO_CONSOLE_SUPERVISOR_SOCKET`, set by the supervisor to
  `/run/ordo/supervisor.sock`): status, logs, apply, rollback, restart.
- The console keeps the revision history in its database. The supervisor
  keeps the live revision, sealed, on its volume `/var/lib/ordo-supervisor`.
- Deployment secrets are sealed by the supervisor and handed over to the
  deployment. The console holds none.
- The image needs 4 capabilities: SETUID, SETGID, CHOWN and SETPCAP. It
  needs no KILL: a child of another user is stopped through its launcher,
  which runs as that user. The compose file drops ALL and adds those 4, has
  a read-only root, a tmpfs at `/run/ordo` (uid 65530, mode 0711) and at
  `/tmp` (noexec), and a `stop_grace_period` of 200s.
- Each database has its own Postgres cluster. The identity service's
  database, `ordo_identity`, is on the console's cluster.

Separate deployment-only and console-only images are not provided (owner
ruling). The binaries stay in the one image and in the release's binary
artifacts, so you can build your own image.

### Where the identity service is reached

- By default (`ORDO_SUPERVISOR_IDENTITY_ADDRESS=console-path`) it is under
  the console's own origin at `/identity`, for example
  `http://localhost:8081/identity`. The console passes that path through.
  No third port is published and passkeys belong to the console's host.
- `own-port`, or setting `ORDO_IDENTITY_ORIGIN`, gives it an address of its
  own, port 8082 in the image. You publish and proxy that port. Its host
  must be the RP ID or under it.

### Door-only mode

A console started without `ORDO_CONSOLE_SUPERVISOR_SOCKET` is in door-only
mode. It talks to the deployment only through its door and has no way to
run a process.

It cannot:

- Start, stop or restart the deployment.
- Change the deployment's settings. Every runtime write is refused, and the
  refusal names `ORDO_CONSOLE_SUPERVISOR_SOCKET`.
- Read the deployment's secrets. It holds none.
- See the process. If the door is silent, the screen says "the deployment
  is not answering" and no more.

The Runtime screen is read-only. Everything in the app still works:
deploys, tenants, people, sign-in settings, usage, shares, the security
log. Each write at the deployment still needs your touch.

## Upgrading from the image before the identity service

Use this if you run the one image from before the identity service. Its
compose file had a tmpfs at `/run/ordo-console` (uid 65532), `cap_add`
without KILL, no identity lines, and `console-database-url` owned by 65532,
mode 0400.

With that same compose file:

```
docker compose pull && docker compose up -d
```

You change nothing. What happens:

- The supervisor takes `/run/ordo-console` as its run folder.
- It reads `console-database-url` as the console's user, through its
  launcher, and gives each child a private copy.
- The console carries its live runtime revision over to the supervisor
  (audited as `runtime.carry-over`).
- The old file names no identity database, so the identity service does
  not start. The console keeps its own sign-in and its console key, and
  each owner signs in as before. The boot log and a banner on every
  console page name the 2 settings that start the service. The door keeps
  reading `ORDO_DEPLOYMENT_OWNERS`. A later release, announced ahead,
  will require one of the 2 settings.

To move sign-in to the identity service, set one setting:

- `ORDO_IDENTITY_DATABASE_URL_FILE`: a database you made for it, or
- `ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true`: the service makes
  `ordo_identity` on the console's Postgres server, with the console's role.

Then the service imports the console's owners, passkeys and live recovery
codes, the console forgets its old console key itself (audited), the door
makes no new console key and ignores `ORDO_DEPLOYMENT_OWNERS` (its boot log
says so; delete the line), and each
owner signs in at `http(s)://<console origin>/identity` with the passkey
they already have.

Keep `ORDO_CONSOLE_ORIGIN` (and `ORDO_CONSOLE_RP_ID`, if you set it) exactly
as before. If the identity service gets its own origin, its host must be the
RP ID or under it.

**Before your next `docker compose down`, add 2 named volumes.** With the old
file, `/var/lib/ordo-identity` and `/var/lib/ordo-supervisor` are anonymous
volumes. `up -d` keeps them. But `down` followed by `up` makes new ones, and
the identity service then refuses to boot, because its signing keys no
longer open without `identity.key`. Add the volumes as the newer
`docker/compose.one.yml` has them:

```
volumes:
  - identity-data:/var/lib/ordo-identity
  - console-data:/var/lib/ordo-console
  - supervisor-data:/var/lib/ordo-supervisor
```

The newer `docker/compose.one.yml` is the recommended file. If you adopt it,
run `scripts/make-secrets.sh one` again to add the identity service's 2
secret files.

## Backups

Back up these, and keep keys apart from data.

1. **The console's cluster** (`console-db` volume). It holds sealed
   secrets, revisions and the audit log, the console's old owner tables,
   and the identity service's database `ordo_identity`.
2. **The 3 data volumes**, each mounted persistently:
   - `/var/lib/ordo-identity` (`ORDO_IDENTITY_DATA_DIR`): the identity
     service's sealing key, `identity.key` (`ORDO_IDENTITY_KEY_FILE`). Its
     signing keys do not open without it.
   - `/var/lib/ordo-console`: `console.key` (`ORDO_CONSOLE_KEY_FILE`). It
     opens the sealed secrets in the console database.
   - `/var/lib/ordo-supervisor`: the deployment's live revision, sealed, and
     `supervisor.key`.
3. **`master.key` and `master-passphrase`**, off host.

A copy of the database alone yields no secrets. A copy of the keys alone
opens nothing. Together they open everything, so store them in different
places.

If you lose a key, what it seals cannot be opened. Boot refuses, naming the
setting (for the console, `ORDO_CONSOLE_KEY_FILE`). Point it at the key's
backup.

The deployment's own database and master key are backed up as
`howto-custody.md` says.

## A different cluster for the console's database

Give the console its own Postgres cluster. The compose file does.

- Boot refuses a console URL that names the deployment's database.
- If the console's cluster is down, nobody can sign in. The login page and
  `/health/*` still answer, and the deployment keeps running.
- With 2 clusters, one failure does not take down both the console and the
  deployment.
- A role of its own, on a database of its own, is the least you must do if
  you share a cluster.

## What the console does not defend against

Someone who already controls the container or its host. They control the
deployment without the console.

Also:

- A stolen unlocked device. Until its session ends it acts as you
  (`ORDO_IDENTITY_SESSION_IDLE_HOURS` and `_MAX_HOURS`; without the
  identity service, 30 minutes unused and 12 hours at most).
- A phished passkey prompt for the deployment. Read the box before you
  touch.
- Losing both the console database and its key together.
