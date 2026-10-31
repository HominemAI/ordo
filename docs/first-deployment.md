# Your first deployment

For the deployment owner. This page takes you from nothing to a signed-in
console and a deployed program. Do the steps in order. Each says what you
should see.

You need:

- A Linux host with Docker and the compose plugin.
- `openssl` and the `ordo` tool on the host (the script uses both).
- A TLS proxy for anything beyond `localhost` (see step 5).

The details live elsewhere: [`howto-console.md`](howto-console.md) for the
console, [`howto-identity.md`](howto-identity.md) for deployment people and
`ordo login`, [`deploying.md`](deploying.md) for every setting.

## 1. Know what you are starting

One image, `ordo`, holds everything. Its entrypoint, `ordo-supervisor`,
runs 3 programs side by side, each as its own unix user:

- the identity service (where people sign in),
- the console,
- the deployment (`ordo-host`).

The compose file is `docker/compose.one.yml`. Build the image from the
repository (`docker build -f docker/Dockerfile -t ordo:dev .`) or set
`ORDO_IMAGE` to the published one.

- The console runs the deployment through the supervisor: settings,
  restart, roll back, all on the Runtime screen.
- Separate deployment-only and console-only images are not provided. The
  binaries are in the image and in the release's binary artifacts, so you
  can build your own.

You should see: nothing yet. This step is only the map.

## 2. Make the secrets

Run the script from the repository root.

```
scripts/make-secrets.sh one
```

It makes the files in `docker/secrets/`, with the mode and group each
reader needs. It asks for `sudo` once, to set groups. It never overwrites
a file. If one exists it refuses and changes nothing. Run it once for a new
deployment.

You should see: `Made N secret files in ...` and a list of names, each
`-r--r-----` except the 2 database URL files, which the script lists with
their own modes.

Back up now:

- `master.key` and `master-passphrase`, together, off this host.
- Keep them apart from your other backups.
- Lose either one and the data they seal cannot be opened.

Keep `docker/secrets/` out of git.

## 3. Your own Postgres (optional)

The compose file starts 2 Postgres clusters for you. Skip this step if you
want that.

To use your own, you need 2 separate clusters, or 2 separate databases with
2 separate roles at the least:

| | Database | Role | Used by |
|---|---|---|---|
| The deployment | `ordo` | `ordo` | `ordo-host` |
| The console | `ordo_console` | `console` | the console |
| The identity service | `ordo_identity` | `identity` | the identity service |

Rules:

- The console refuses a URL that names the deployment's database.
- If the console's cluster is down, nobody can sign in. The deployment keeps
  running. Separate clusters keep one failure from taking down both.
- A copy of either database alone yields no secrets.
- The identity service's database lives on the console's cluster, as
  `ordo_identity` with its own role. The compose file's `identity-db-setup`
  makes it, and the compose file names it. The database is always
  explicit (owner, 2026-10-30): name it with `ORDO_IDENTITY_DATABASE_URL_FILE`,
  or opt in with `ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true` to let the
  service make `ordo_identity` on the console's server, with the console's
  role, at first start. With neither, the identity service does not start
  and the console keeps its own sign-in, with a banner naming both
  settings. A later release, announced ahead, will require one.

Required privileges:

- The `console` role owns `ordo_console` and can create tables in it.
- The `ordo` role owns `ordo` and can create tables in it. It makes the
  platform tables at first boot.
- The `ordo` role also needs `CREATEDB` if tenants get a database each on
  the same server (the default). Leave it off if you set
  `ORDO_PG_TENANT_*` to another cluster.

Example, as a Postgres superuser:

```
CREATE ROLE ordo LOGIN PASSWORD '...' CREATEDB;
CREATE DATABASE ordo OWNER ordo;
-- on the console's cluster:
CREATE ROLE console LOGIN PASSWORD '...';
CREATE DATABASE ordo_console OWNER console;
CREATE ROLE identity LOGIN PASSWORD '...';
CREATE DATABASE ordo_identity OWNER identity;
```

Point the settings at them:

- Deployment: set `ORDO_PG_HOST`, `ORDO_PG_PORT`, `ORDO_PG_USER`,
  `ORDO_PG_DB` and `ORDO_PG_PASSWORD_FILE` in the compose file. Put the
  password in `docker/secrets/deployment-db-password-app`.
- Console: put the whole URL in `docker/secrets/console-database-url`, for
  example `postgres://console:PASSWORD@console-host:5432/ordo_console`.
- Identity: put its URL in `docker/secrets/identity-database-url`.
- Delete the `deployment-db`, `console-db` and `identity-db-setup`
  services from the compose file, and their `depends_on` lines.
- The password files the script wrote for the bundled databases
  (`deployment-db-password`, `console-db-password`) are then unused. Keep
  the two copies the deployment and console read in step with your roles.

You should see: after step 4, no `Connection refused`. If the database
cannot be reached, boot says:
`could not reach ORDO_PG_HOST:ORDO_PG_PORT, which is host:port`, and what
to check. The console's refusal names its host and port the same way.

## 4. Start it

```
docker compose -f docker/compose.one.yml up -d
docker compose -f docker/compose.one.yml logs ordo | grep -i invite
```

You should see: a line with the first invite link. If you see no line,
read the whole log. A refusal names the setting to fix.

## 5. Sign in and save the recovery codes

1. Open the first invite link from the log.
2. Make a passkey when the browser asks.
3. Save your recovery codes. They are shown once.

You should see: the console, signed in.

Facts:

- People sign in at the identity service, with passkeys. The first person
  to use the first invite link becomes the first admin.
- By default the identity service is reached under the console's own
  address, at `/identity` (for example `http://localhost:8081/identity`).
  The console passes that path through, so no third port is published and
  passkeys belong to the console's host.
- To give the identity service an address of its own, set
  `ORDO_SUPERVISOR_IDENTITY_ADDRESS=own-port` (port 8082 in the image) or
  set `ORDO_IDENTITY_ORIGIN`. Then publish and proxy that port. Its host
  must be the passkey scope (`ORDO_CONSOLE_RP_ID`, or the console origin's
  host) or under it.
- Store the recovery codes away from the passkey.
- Passkeys are tied to the host in `ORDO_CONSOLE_ORIGIN` (or
  `ORDO_CONSOLE_RP_ID`). Choose it now and keep it: `howto-identity.md`,
  section 9, says what a change of host costs.
- Ports bind `127.0.0.1` only and speak plain HTTP. Put a TLS proxy in
  front, set `ORDO_CONSOLE_ORIGIN` to its HTTPS address, and never publish
  the ports on `0.0.0.0`.

Add a second admin soon, so one lost passkey does not lock you out. See
`howto-identity.md`, "Invite people".

## 6. Check the door

1. Open **Deployment**.
2. Read "Is the door reachable?".

You should see: the door reachable. There is no link step and no console
key to make. The console manages the deployment through the supervisor,
and reaches the door only to read it.

Deployment people live in the identity service. Invite more with
`ordo-identity-service invite` or `ordo people invite`. The door ignores
`ORDO_DEPLOYMENT_OWNERS` here and its boot log says so.

## 7. Deploy a first program

The example is notes v2, in `examples/notes-v2/`.

1. Open **Deploys** in the console.
2. Apply the program from `examples/notes-v2/notes.ordo`.
3. Touch your passkey when the box asks.

From a terminal, sign in once with
`ordo login http://localhost:8081/identity` (your console origin, then
`/identity`), and `ordo apply examples/notes-v2 --host <door> --space system`
does the same, with no token to copy (`howto-identity.md`, section 7;
`deploying.md`, section 7).

You should see: a report like

```
schema     created  (... on an empty target)
stored     <hash>
activated  <hash>  (was nothing)
```

and the Deploys screen lists the version as active.

## 8. Back up

Back up these, and keep keys apart from data.

1. The deployment's database (`deployment-db` volume).
2. The console's cluster (`console-db` volume). It also holds the identity
   service's database, `ordo_identity`.
3. `master.key` with `master-passphrase`, off host.
4. The 3 data volumes. Mount each persistently
   (`howto-identity.md`, "Back up", for the identity service's):
   - `/var/lib/ordo-identity`: the identity service's data. It holds
     `identity.key`. The identity service's signing keys do not open
     without it, and it will not boot.
   - `/var/lib/ordo-console`: `console.key`.
   - `/var/lib/ordo-supervisor`: the deployment's live revision, sealed,
     and `supervisor.key`.

`docker/compose.one.yml` names these volumes (`identity-data`,
`console-data`, `supervisor-data`). Do not run without them: an anonymous
volume is lost on `docker compose down` and then `up`. A fourth,
`fn-cache`, holds the compiled function engine. Losing it costs one
compile of about a second.

A database copy without the keys yields no secrets. Keys without a database
open nothing. Store them in different places. Detail: `howto-console.md`,
"Backups", and `howto-custody.md`.

You should see: a restore test on a spare host opens the console and the
deployment. Do this once, before you need it.

## If something goes wrong

- Boot refuses: the message names the setting and what to change.
- Every admin locked out: `howto-identity.md`, "Break-glass and
  recovery".
- Database `Connection refused`: check the host and port the message names.
