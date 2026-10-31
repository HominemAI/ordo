# How to run the identity service

For the deployment owner. The identity service holds the people and
machines that run a deployment: who may sign in to the console, deploy,
and invite others. It signs short tokens that the deployment's door and
the console check with its public key. The design is
`plan-identity-service.md`; the console's side is
[`howto-console.md`](howto-console.md); every setting is in
[`settings.md`](settings.md).

Starting from nothing? [`first-deployment.md`](first-deployment.md) takes
you to a signed-in console first. This page is the reference after that.

Words used here:

- A **deployment person** runs the deployment. Every one is an `admin`
  for now. Their id looks like `p_…`.
- A **machine key** lets software, such as CI, do the same with nobody
  present. Its id looks like `m_…`.
- Your app's own users and tenants are a different realm. Nothing here
  changes how they sign in.

## 1. Where it runs

The one image runs it beside the console and the deployment, as unix user
65531, on port 8082 inside the container. Browsers reach it one of 2 ways:

| `ORDO_SUPERVISOR_IDENTITY_ADDRESS` | Address | You publish |
|---|---|---|
| `console-path` (the default) | the console's own origin, under `/identity`, for example `https://console.example.com/identity` | nothing new: the console passes `/identity` through |
| `own-port` | the console's origin with port 8082, for example `https://console.example.com:8082` | port 8082, through your TLS proxy |

Setting `ORDO_IDENTITY_ORIGIN` yourself also gives it its own address.
Its host must follow the rule in section 9.

The door and the console reach it inside the container at
`http://127.0.0.1:8082`. The supervisor sets that for them as
`ORDO_IDENTITY_URL`.

## 2. Give it a database

The service never makes a database on its own. Choose one of 2 ways
(owner, 2026-10-30):

1. **A database you name.** Set `ORDO_IDENTITY_DATABASE_URL_FILE` to a
   file holding its URL. `docker/compose.one.yml` does this: its
   `identity-db-setup` service makes the role `identity` and the database
   `ordo_identity` on the console's Postgres cluster, and
   `scripts/make-secrets.sh one` writes `identity-database-url`.
2. **The opt-in.** Set `ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true`. At
   first start the service makes `ordo_identity` on the console's Postgres
   server, with the console's role, and uses it. The boot log says
   ``made database `ordo_identity` beside the console's``. The console's
   role must be allowed to create a database; if it is not, the boot
   refuses and names `ORDO_IDENTITY_DATABASE_URL`.

Either way, the service refuses a URL that names the console's database or
the deployment's.

### A deployment with neither

An upgraded deployment whose compose file names neither keeps working as
before, with nothing to run by hand:

- The supervisor does not start the identity service. Its health shows it
  as stopped.
- The console keeps its own sign-in. Each owner signs in at the console
  with the passkey they already have.
- Every console page shows a banner: "Sign-in still happens at this
  console, as before. To move it to the identity service, set
  ORDO_IDENTITY_DATABASE_URL (or ORDO_IDENTITY_DATABASE_URL_FILE), or set
  ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true. A later, announced release
  will require one of them."
- The boot log says the same, from the supervisor (`the identity service
  is not started: …`) and from the console (`people sign in at this
  console, as before the one image: …`).
- The door keeps reading `ORDO_DEPLOYMENT_OWNERS`.

Set one of the 2 settings when you are ready. At its next start the
service moves the console's owners in (section 10).

## 3. The first admin

On a new deployment with nobody in it, the service makes one invite link
at first start, once.

- It is in the log: `docker compose -f docker/compose.one.yml logs ordo |
  grep -i invite`.
- It is also in the file `first-invite` in the service's data folder
  (`/var/lib/ordo-identity`).
- It works once, within `ORDO_IDENTITY_INVITE_DAYS` (7 by default).

Open it, give your name, make a passkey, and save the 10 recovery codes it
shows. You are the first admin.

An upgraded deployment prints no first invite: its console owners are
moved in instead (section 10).

On Ordo Cloud the first admin comes from the provisioning service, through
`ORDO_IDENTITY_PROVISIONER_KEY`, which becomes the machine key
`provisioner` at first boot.

## 4. Invite people

Each invite is a single-use link for one new admin.

- In the console: **Deployment people**, "Invite an admin". It asks for
  your passkey touch, then shows the link once.
- From a terminal: `ordo people invite --label "Sam"`.
- On the host, with no sign-in at all: section 8.

Send the link to the person. They open it, make a passkey, and save their
own recovery codes. A link lives `ORDO_IDENTITY_INVITE_DAYS` days and can
be revoked from the same screen while it is open.

Add a second admin early, so one lost passkey never locks the deployment.

## 5. End someone's access

- In the console: **Deployment people**, "End access", type to confirm,
  and touch your passkey.
- From a terminal: `ordo people end p_…` (ids from `ordo people list`).

What happens:

- New sign-ins and renewals are refused at once. Their sessions end, their
  recovery codes stop working, and invites they made and nobody used are
  revoked.
- A token they already hold works until it expires: at most
  `ORDO_IDENTITY_TOKEN_SECS` (600 seconds by default). The door and the
  console do not ask the service on each call.
- The last admin cannot be ended. The refusal names the host commands of
  section 8.

## 6. Machine keys

A machine key signs CI in, to deploy with nobody present.

1. Make one: in the console, **Deployment people**, "Machine keys", or
   `ordo people machine-key add --name ci-main`. The secret,
   `ordo-mk-v1:…`, is shown once. A key your CI made itself can be
   registered instead with `--public-key <base64url>`.
2. In CI, set `ORDO_MACHINE_KEY` (or `ORDO_MACHINE_KEY_FILE`) to the
   secret and `ORDO_IDENTITY_URL` to the service's public address. `ordo
   apply` then signs in by itself.
3. List them with `ordo people machine-key list`. End one with `ordo
   people machine-key end m_…`. New sign-ins are refused at once; its
   tokens die within the token life.

A machine key may deploy, activate, and set quotas in the deployment's own
space. It never acts inside a tenant. It belongs to the deployment, so it
outlives the person who made it. Every act is recorded under its id and
name.

## 7. The command line

The `ordo` tool signs in through your browser.

```
ordo login https://console.example.com/identity
```

- It opens the browser at the service's page. You approve with your
  passkey, and the browser hands the session back to `ordo` on this
  computer.
- `--no-browser` prints the link instead, for a browser on another
  screen.
- With `ORDO_IDENTITY_URL` set to the same address, `ordo login` needs no
  argument.
- On `localhost` plain `http://` is allowed. Elsewhere it is refused
  unless `ORDO_IDENTITY_PLAIN_HTTP=on`.

The session is kept in `sessions.json` in your Ordo config folder (or
`ORDO_CONFIG_DIR`), mode 0600. It lasts `ORDO_IDENTITY_SESSION_IDLE_HOURS`
unused (168, 7 days) and `ORDO_IDENTITY_SESSION_MAX_HOURS` at most (720,
30 days).

| Command | What it does |
|---|---|
| `ordo login [<url>]` | signs this computer in |
| `ordo login --touch` | a fresh passkey touch for writes, on the session you hold |
| `ordo whoami [<url>]` | who the next command acts as, from which credential, and until when |
| `ordo logout [<url>]` | ends the session at the service and deletes it here |
| `ordo people list` | everyone, and whose access has ended |
| `ordo people invite [--label <text>]` | a sign-up link for a new admin |
| `ordo people end <p_id>` | ends a person's access now |
| `ordo people machine-key add --name <name> [--public-key <base64url>]` | a key for CI; its secret is shown once |
| `ordo people machine-key list` | every machine key |
| `ordo people machine-key end <m_id>` | ends a machine key now |

Every `ordo people` command takes `--identity <url>`.

Changes need a passkey touch in the last 5 minutes. At a terminal, `ordo`
runs the touch by itself and sends the call again. In a script, the
refusal says to run `ordo login --touch`.

Which credential a command uses, first match wins:

1. `--token` or `ORDO_TOKEN`.
2. `ORDO_MACHINE_KEY` or `ORDO_MACHINE_KEY_FILE`, with `ORDO_IDENTITY_URL`.
3. The session from `ordo login`.

So `ordo apply ./notes --host https://door.example.com --space system`
works with no token once you have signed in.

## 8. Break-glass and recovery

### One lost passkey

Sign in on the service's page with "Lost your passkey? Use a recovery
code". A code works once. It lets you do one thing: make a new passkey on
this device. Then your old passkeys stop working and you get 10 new codes.

Make new codes any time on your account page (`<address>/account`),
"Make new recovery codes".

### Every admin locked out, or the page unreachable

Two host commands run inside the container. Neither adds power: whoever
can run them already holds the host. Each is written to the audit log
with the unix user and the time.

| Command | What it does |
|---|---|
| `ordo-identity-service invite [--label <text>]` | a new admin's invite link, valid `ORDO_IDENTITY_INVITE_DAYS` days. This is the recovery when every passkey is lost |
| `ordo-identity-service break-glass-token [--minutes <1..60>]` | one admin token, 15 minutes by default, for `ordo --token` when the page cannot be reached |

Two more: `ordo-identity-service rotate-keys [--now]` makes a new signing
key, and `ordo-identity-service audit verify` checks the audit chain.

In the one image the service's settings are filled in by the supervisor,
so a host command needs them given again. Run it as the service's unix
user, with its own copy of the database URL, and the same address and
relying party the running service has (its boot log prints each value it
filled). For the compose file on `localhost`:

```
docker compose -f docker/compose.one.yml exec -u 65531 \
  -e ORDO_IDENTITY_ORIGIN=http://localhost:8081 \
  -e ORDO_IDENTITY_BASE_PATH=/identity \
  -e ORDO_IDENTITY_RP_ID=localhost \
  -e ORDO_IDENTITY_DATABASE_URL_FILE=/run/ordo/identity/ORDO_IDENTITY_DATABASE_URL \
  ordo ordo-identity-service invite --label "recovery"
```

Behind a proxy, use your HTTPS console origin and its host instead. With
the opt-in of section 2, name the console's URL instead of the database
URL: `-e ORDO_IDENTITY_IMPORT_CONSOLE_URL_FILE=/run/ordo/identity/ORDO_IDENTITY_IMPORT_CONSOLE_URL`.
A compose file from before the one image keeps its run folder at
`/run/ordo-console`, so the files are under `/run/ordo-console/identity/`.

Open the link it prints and make a passkey.

## 9. Keep the relying party the same

A passkey belongs to one relying party id (RP ID): a host name. A browser
offers it only on a page whose host is that id or under it. The ports do
not matter.

The service takes the console's RP ID: `ORDO_CONSOLE_RP_ID`, or the host
of `ORDO_CONSOLE_ORIGIN`. So that every passkey keeps working:

- Keep `ORDO_CONSOLE_ORIGIN` exactly as it was, and `ORDO_CONSOLE_RP_ID`
  too if you set it. Changing the host strands every passkey made under
  the old one.
- If the service gets its own address, its host must be the RP ID or a
  host under it. Under `console.example.com`,
  `https://console.example.com:8082` and `https://id.console.example.com`
  work; `https://id.example.com` does not, and the boot refuses it, naming
  both settings.
- On `localhost` the RP ID is `localhost` for every port, so the compose
  file's `http://localhost:8081` needs nothing more.

To move to another host on purpose, set `ORDO_IDENTITY_RP_ID` to it and
send every person a new invite. That is the one path that loses the old
passkeys, and the boot refusal says so.

## 10. Upgrading a deployment with console owners

When the service starts for the first time with a database and the
console's owners exist, it moves them in before it listens:

- Every owner becomes a person, with their passkeys and unused recovery
  codes, column for column. A removed owner arrives with access ended.
- It reads the console's database and never writes to it, so going back
  to the old image finds it as it was.
- It runs once. The boot log and the audit log record the counts and the
  RP ID in force.
- Open console invite and recovery codes are not moved. Each is recorded
  as voided; make a new invite if one was pending.

Each owner then opens the console, is sent to the service, and signs in
with the passkey they already have. Nobody enrols again. The step by step
for the old compose file is `howto-console.md`, "Upgrading from the image
before the identity service".

To rehearse it first on a copy of your deployment, see
`scripts/rehearse/live-copy.sh` (section 12).

## 11. Back up

Back up together:

1. **Its database**, `ordo_identity`. In the compose file it is on the
   console's cluster, so the `console-db` volume or a `pg_dumpall` of that
   cluster holds it.
2. **Its data folder**, `ORDO_IDENTITY_DATA_DIR` (`/var/lib/ordo-identity`).
   It holds `identity.key` (`ORDO_IDENTITY_KEY_FILE`), the key its signing
   keys are sealed with, and `first-invite`.

Mount the data folder as a persistent named volume.
`docker/compose.one.yml` mounts `identity-data` there.

**The old compose file.** A compose file from before the one image mounts
only `console-data`. With it, `/var/lib/ordo-identity` and
`/var/lib/ordo-supervisor` are anonymous volumes. `docker compose up -d`
keeps them, but `docker compose down` and then `up` makes new, empty ones.
The service then refuses to boot: `ORDO_IDENTITY_KEY_FILE does not open
the token signing key …`. Add the 2 named volumes before your next
`down`, as `howto-console.md` shows.

Without `identity.key` the database's signing keys cannot be opened. A
new key file opens none of them, so restore the one backed up with the
database. A database copy alone yields no private key, and the key file
alone opens nothing, so store them apart.

The deployment's own data and the console's are backed up as
`first-deployment.md` step 8 and `howto-custody.md` say.

## 12. Rehearse an upgrade on a copy

`scripts/rehearse/live-copy.sh` restores a copy of a deployment into a
throwaway local stack on the image it runs, upgrades it to the new image,
checks it serves with the same program active, and waits while you sign
in with your real passkey, first at the console and then at `/identity`.
After each sign-in it reads the copy's own database, so a sign-in on the
live site cannot pass for one on the copy.

The copy is one folder, taken from the deployment's compose folder:

| File | How |
|---|---|
| `compose.yml` | the compose file, as it runs |
| `secrets/` | a copy of `docker/secrets/` |
| `deployment-db.sql` | `docker compose exec -T deployment-db pg_dumpall -U ordo` |
| `console-db.sql` | `docker compose exec -T console-db pg_dumpall -U console` |
| `volumes/<name>.tar` | each named volume of the `ordo` service: `console-data`, and `identity-data` and `supervisor-data` when they exist |

The copy's containers sit on an internal Docker network with no route
out, so it cannot send mail, reach a file store, or touch the deployment
it came from. A passkey under `localhost` works on the copy's own port. A
passkey under a real host needs that host served on your computer: one
`/etc/hosts` line and a local certificate from `mkcert`, which the script
checks before it starts. Its header has the steps. The copy holds the
deployment's keys: keep it on an encrypted disk and delete it after.

## 13. Its settings

The ones a deployment owner sets most. Each refuses a bad value at boot,
naming the setting.

| Setting | Default | What it does |
|---|---|---|
| `ORDO_IDENTITY_DATABASE_URL_FILE` | none | A file with its database URL (section 2). `ORDO_IDENTITY_DATABASE_URL` holds the URL itself |
| `ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER` | `false` | `true` makes `ordo_identity` on the console's server (section 2) |
| `ORDO_SUPERVISOR_IDENTITY_ADDRESS` | `console-path` | Where browsers reach it (section 1) |
| `ORDO_IDENTITY_ORIGIN` | filled by the supervisor | Its public origin. Set it to give the service its own address |
| `ORDO_IDENTITY_BASE_PATH` | `/identity` in `console-path` mode | The path it is served under |
| `ORDO_IDENTITY_RP_ID` | the console's RP ID | The passkeys' relying party (section 9) |
| `ORDO_IDENTITY_DATA_DIR` | `/var/lib/ordo-identity` | Its volume |
| `ORDO_IDENTITY_KEY_FILE` | `identity.key` in the data folder | The sealing key; made at first boot |
| `ORDO_IDENTITY_INVITE_DAYS` | 7 | Invite life, 1 to 30 |
| `ORDO_IDENTITY_TOKEN_SECS` | 600 | Token life, 60 to 3600; how soon ending access takes effect |
| `ORDO_IDENTITY_SESSION_IDLE_HOURS` | 168 | A session ends after this long unused, 1 to 720 |
| `ORDO_IDENTITY_SESSION_MAX_HOURS` | 720 | A session's absolute end, 1 to 2160, never below the idle value |
| `ORDO_IDENTITY_KEY_ROTATE_DAYS` | 90 | Signing key rotation, 0 to 365; 0 turns it off |
| `ORDO_IDENTITY_GUESSES_PER_ADDRESS` | 30 | Tries per minute of one kind of code from one address |
| `ORDO_IDENTITY_GUESSES_SHARED` | 600 | Tries per minute of one kind from everyone together |
| `ORDO_IDENTITY_PROVISIONER_KEY` | none | The provisioning service's Ed25519 public key (Ordo Cloud) |
| `ORDO_PROXY_HEADER`, `ORDO_PROXY_TRUSTED` | none | Read the visitor's address from a trusted proxy, as the door does |

The supervisor also fills `ORDO_IDENTITY_CLIENT_ORIGINS` (the console's
origin) and `ORDO_IDENTITY_IMPORT_CONSOLE_URL` (the console's database,
for section 10) when you leave them unset. A value you set always wins,
and the boot log names each value it filled.

## What it does not do

- Roles beyond `admin`.
- Sign-in through outside providers for deployment people.
- Sending invite links by mail. You send the link.
- More than one copy of the service.
