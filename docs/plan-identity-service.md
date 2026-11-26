# Plan: the identity service for deployment people

**A third service in the one image holds the people and machines that run
a deployment. It issues short signed tokens. The deployment's door and the
console verify them with its public key and never hold these people
themselves. It is Ordo's own, the bare minimum, and it is v1.**

Written 2026-10-27 against the tree at `b5f9a9b8`. Board task
`identity-service` (18o). It follows the owner's rulings of 2026-10-27 in
`corpus-rulings.md`: "deployment people, from the back end", "Ordo Cloud v1
is plain Ordo, hosted", and "(later): one identity service, in the one
image", which replaced "deployment accounts, for now" the same night. Those
rulings outrank every earlier plan, including `plan-console.md` sections 5.3,
8 and 9.

Revised 2026-10-27 for the owner's 10 answers (section 10). Two changed
the plan: an upgrade moves existing console owners and their passkeys into
the service with no action from them (6.1, 6.2), and sessions last 7 days
idle and 30 days at most (2.3).

Words. A **deployment person** is someone who runs the deployment: deploys,
changes settings, invites others. A **machine key** is a credential for
software that does the same with nobody present (the provisioning service,
CI). The **deployment realm** is those people and machines; the **app
realm** is the app's own tenants, users and tokens. The **home space** is
`ORDO_SPACE` (`system` in the compose files), the deployment's own space.
The **provisioning service** is the owner's private service that sets up
Ordo Cloud instances.

---

## 1 · The shape, in 6 lines

1. A new binary, `ordo-identity-service`, in `crates/ordo-identity-service`.
   Its own Postgres database, its own passkey relying party, its own page.
2. A small supervisor is the image's entrypoint. It runs the deployment, the
   console and the identity service as siblings. None is another's parent.
3. The service signs 10 minute tokens with an Ed25519 key. The door and the
   console verify them with its published public key, cached.
4. A deployment token reaches platform ops and nothing else. An app token
   never reaches a platform op. The app's own sign-in is unchanged.
5. People arrive by single-use invite link. Access can end. The first admin
   comes from the provisioning call or the first boot, never from a page.
   An upgraded deployment's console owners are moved in at first start,
   passkeys included, and sign in as before.
6. Every act is in a hash-chained audit log with who did it, with signed
   checkpoints, on by default.

---

## 2 · Where it lives

### 2.1 A new crate, and what it reuses

`crates/ordo-identity-service`, binary `ordo-identity-service`, settings
prefixed `ORDO_IDENTITY_`. Its own crate rather than a mode of the console,
because the ruling needs the console and the identity service to run without
each other, and one binary with two modes shares one boot and one failure.

It reuses `ordo-identity` for the parts that are hard to get right, and
takes the console's own sign-in code with it:

| Piece | From | Note |
|---|---|---|
| Passkey ceremonies | `ordo_identity::passkey::PasskeyProvider`, `webauthn`, `MemoryCeremonyStore` | The service implements `PasskeyStore` over its own table, as `ordo-console/src/passkeys.rs` does today. That file moves here |
| Recovery codes, setup codes | `ordo-console/src/codes.rs` | Moves here as invite and recovery code minting, hashing, display |
| Sealing | `ordo-console/src/sealing.rs` | Moves to `ordo-identity` as a shared module, since both the console and the service seal with a key file |
| Ed25519 signing | `ed25519-dalek`, `ordo_commandlog::sign` | Token signing and audit checkpoints |
| JWS verification | `ordo_identity::jose` (EdDSA, `kid` from a JWKS) | The door and the console verify tokens with it. Nothing new is written for the verifier |
| Hash chain | `ordo_commandlog::ChainHash`, `hash = SHA-256(prev ‖ body)` | The audit log |
| Guessing limits | The shape of `console_keys::MAX_ATTEMPTS_PER_WINDOW` | A per-node throttle on invite, recovery code and machine sign-in tries, keyed by a hash of what was presented |

It never opens the deployment's directory. `$principal`, `$authenticator`
and `$passkey` are the app realm's. The service has its own `people` table
and its own passkey table, in its own database. A deployment person who is
also an app user holds 2 accounts, 2 passkeys, and that is by design: the
realms do not meet.

Amended 2026-11-19 (11.3): the realms meet where a deployment connects to
the service as an OIDC provider. An app that does so signs its users in
with their account at the service, and links them in its own
`$authenticator` as `oidc:<connection>:<sub>`. The service still never
opens the deployment's directory.

### 2.2 Its database

Postgres, its own database and role, `ORDO_IDENTITY_DATABASE_URL` or
`ORDO_IDENTITY_DATABASE_URL_FILE`. Required; an unset value refuses the
boot and names the setting. Boot refuses a URL that names the deployment's
or the console's database. It may live on the console's cluster as a second
database, which is what the compose file does, so the image needs no new
cluster (ruling: "no extra hosting needed"). SQLite is out for the console's
reason (2026-10-23, ask 14).

Tables, each with `created_at` and a deployment handle column so the
private provisioning service can later hold many deployments in one
service (`plan-console.md` 11):

| Table | Holds |
|---|---|
| `people` | id `p_…`, display name, role (`admin`, the only one), `ended_at`, `ended_by`, `console_owner_id` for a person the import brought (6.1) |
| `passkeys` | credential id, person, algorithm, COSE key, sign count, user handle, backup flags, registered at, last use. The console's columns, unchanged, so the import is a copy |
| `recovery_codes` | hashed, single use, voided on renewal |
| `invites` | hash of the secret, label, role, issued by, issued at, expires at, used at, used by, revoked at |
| `sessions` | id hash, person or machine, kind (`browser`, `cli`), created, last seen, `touched_at`, absolute end, ended at and why |
| `cli_grants` | one-time codes a browser approval mints for a CLI, with the PKCE challenge, 60 seconds |
| `machine_keys` | id `m_…`, name, Ed25519 public key, created by, last used, ended at |
| `signing_keys` | kid, public key, private key sealed with `ORDO_IDENTITY_KEY_FILE`, created, active from, retired at |
| `audit` | the chained log, section 7 |
| `checkpoints` | signed heads of the audit chain |
| `imports` | one row per source imported, `console:<handle>`: when, the RP ID in force, counts of people, passkeys and codes. Its presence is what makes the import run once (6.1) |
| `identity_migrations` | its migrations, never edited once shipped |

### 2.3 Its settings

| Setting | Means |
|---|---|
| `ORDO_IDENTITY_LISTEN` | default `127.0.0.1:8082`; the image sets `0.0.0.0:8082` |
| `ORDO_IDENTITY_ORIGIN` | the address people open it at. Required. In the one image the supervisor fills it from `ORDO_CONSOLE_ORIGIN` with the port set to 8082 when unset (3.1) |
| `ORDO_IDENTITY_RP_ID` | the relying party id its passkeys bind to, default the origin's host. In the one image the supervisor fills it from the console's RP ID when unset, so passkeys made at the console keep working (6.2). Boot refuses an origin whose host is not this id or under it, and names both settings |
| `ORDO_IDENTITY_DATABASE_URL[_FILE]` | its database. Required |
| `ORDO_IDENTITY_IMPORT_CONSOLE_URL[_FILE]` | the console's database, read once at first start to move its owners in (6.1). The supervisor sets it in the one image; unset means no import |
| `ORDO_IDENTITY_KEY_FILE` | the sealing key for signing keys at rest; made at first boot on the volume |
| `ORDO_IDENTITY_CLIENT_ORIGINS` | origins allowed to ask it for tokens from a browser, with credentials: the console's origin. One whose host is the RP ID or under it may also run the touch ceremony in its own page (5.4 step 6). Other client origins reach the ceremony by the redirect to the service's sign-in page |
| `ORDO_IDENTITY_DEPLOYMENT` | the deployment handle tokens name in `aud`; default `main`, the console's `DEFAULT_DEPLOYMENT`. The import reads the console rows of this handle |
| `ORDO_IDENTITY_PROVISIONER_KEY` | an Ed25519 public key of the provisioning service; at first boot it becomes machine key `provisioner`. Hosting-bound, so it lives in the container's environment (Ordo Cloud v1 ruling) |
| `ORDO_IDENTITY_INVITE_DAYS` | invite life, default 7, 1 to 30 (Q9) |
| `ORDO_IDENTITY_TOKEN_SECS` | token life, default 600, 60 to 3600 (Q9). Ending a person's access or a machine key takes effect within this many seconds: a token already issued works until it expires (5.7) |
| `ORDO_IDENTITY_SESSION_IDLE_HOURS` | browser and CLI sessions end after this long unused. Default 168 (7 days), 1 to 720 (Q9). The owner expects 24 or 12 to be common, so the unit is hours |
| `ORDO_IDENTITY_SESSION_MAX_HOURS` | a session's absolute end, measured from sign-in. Default 720 (30 days), 1 to 2160, never below the idle value (Q9) |
| `ORDO_IDENTITY_KEY_ROTATE_DAYS` | signing key rotation, default 90, 0 off |
| `ORDO_IDENTITY_GUESSES_PER_ADDRESS` | tries of one kind of guessable secret (invite, recovery code, CLI code, machine sign-in) one source address may make per minute. Default 30, 1 to 100000 (owner, 2026-10-27) |
| `ORDO_IDENTITY_GUESSES_SHARED` | the ceiling for one kind over everyone together per minute. Default 600 (20 times the per-address default), 1 to 10000000, never below the per-address value; the boot refusal names this setting. It only stops a flood: the secrets are long random codes, and one stranger must not shut a kind of sign-in for everyone. A try refused for its address is not counted here |
| `ORDO_PROXY_HEADER`, `ORDO_PROXY_TRUSTED` | the door's proxy-mode settings, read the same way: the header naming the visitor, and the proxies trusted to set it (addresses or CIDR blocks). Both or neither; a boot refusal names the one missing. Unset, the socket's address is the visitor's, so behind a proxy all visitors share one address count. The header is believed only from a trusted peer; the visitor is its right-most address that is not a trusted proxy |

The door and the console each get 1 setting, `ORDO_IDENTITY_URL`: where the
service answers from their side (`http://127.0.0.1:8082` in the one image).
A door without it accepts no deployment token and says so in the startup
log.

---

## 3 · The supervisor and the one image

### 3.1 Siblings, under one small parent

Today `ordo-console` is the entrypoint and spawns `ordo-host`
(`plan-console.md` 6, `Dockerfile`). A console crash restarts both. The
ruling: "if the thing that manages the thing depends on the thing its
managing its no bueno." So the supervisor moves out of the console.

- New crate `crates/ordo-supervisor`, binary `ordo-supervisor`, the image's
  entrypoint. It is the console's `supervisor/` module (`child.rs`,
  `run.rs`, `spec.rs`, `state.rs`, the launcher `--supervised-child`),
  moved, with 3 children instead of 1.
- Children and unix users: `ordo-identity-service` 65531, `ordo-console`
  65532, `ordo-host` 65533. The supervisor runs as 65530 and carries the 4
  file capabilities the console carries today (`CAP_SETUID`, `CAP_SETGID`,
  `CAP_SETPCAP`, `CAP_CHOWN`, permitted only). The console binary loses
  them. The launcher's refusals (any caller but the supervisor, root as a
  target, its own identity as a target) stay.
- Each child gets a cleared environment with its own settings and its
  secrets as files in its own tmpfs folder (`/run/ordo/<child>`, 0700,
  the child's user). The console's folder holds the console database URL;
  the identity service's holds its database URL, key file path and, for
  the import, the console database URL; the deployment's holds what it
  holds today.
- The supervisor fills 3 identity settings from the console's when they
  are unset, so an upgraded compose file needs no new line:
  `ORDO_IDENTITY_ORIGIN` from `ORDO_CONSOLE_ORIGIN` with port 8082,
  `ORDO_IDENTITY_RP_ID` from `ORDO_CONSOLE_RP_ID` or the console origin's
  host, and `ORDO_IDENTITY_IMPORT_CONSOLE_URL` from the console's database
  URL. A value set by the deployment owner always wins.
- Backoff, crash loop and readiness rules are the ones in
  `supervisor/mod.rs`, applied per child. One child looping never stops
  the other 2. The supervisor's own `/health/live` answers on
  `127.0.0.1:8079` for the container's probe; each child keeps its own
  health routes.
- Shutdown: SIGTERM to the deployment first, then the console, then the
  identity service, each with its grace period.

### 3.2 How the console still manages the deployment

The console keeps its runtime plane (restart, settings, logs, rollback) by
asking the supervisor, over a Unix socket at `/run/ordo/supervisor.sock`,
owned by the supervisor, group 65532, mode 0660. Only the console's unix
user can open it, and the deployment's and the identity service's cannot.
Calls, the same 5 the console has today (`plan-console.md` 6.1): `status`,
`logs(tail)`, `current`, `apply(spec)`, `restart`. Each names a child.
`apply` and `restart` are accepted only for `deployment`. `status` and
`logs` are answered for all 3, so the console's Health screen shows the
identity service too.

Where the live spec lives. The supervisor must start the deployment before
the console is up, so it cannot read the console's database. It keeps the
live deployment spec and the last good one on its own volume
(`/var/lib/ordo-supervisor`, 0700), sealed with `ORDO_SUPERVISOR_KEY_FILE`
(the shared sealing module of 2.1). The console keeps the revision history,
diff, `check-config` run and the person's reauth, and hands the supervisor
the resolved spec with secret values; the supervisor writes the files and
restarts the child. Revision 1 is still built from the container's `ORDO_*`
variables, by the supervisor, on a volume that holds no spec yet.

`ORDO_CONSOLE_MODE` goes. The console is in supervise mode when
`ORDO_CONSOLE_SUPERVISOR_SOCKET` is set, and in door-only mode when it is
not. The `supervisor` cargo feature goes with the code it guarded;
`tests/door_only.rs` becomes a test that a console without the socket
setting has no runtime routes.

### 3.3 The image

One image, `ordo`: `ordo-supervisor` (entrypoint), `ordo-identity-service`,
`ordo-console`, `ordo-host`, the `ordo` CLI, the console page and the
identity service's page. Ports 8080 door, 8081 console, 8082 identity
service. `docker/compose.one.yml` gains an `identity-db` database on the
console's Postgres service and the 3 `ORDO_IDENTITY_*` settings.

The `host` and `console` Dockerfile targets, `docker/compose.split.yml`,
the console-only binary build and the `ordo-host:` and `ordo-console:`
images in `images.yml` go (ruling: "The separate deployment-only and
console-only images are no longer something Ordo provides"). The binaries
stay in the one image and in the release's binary artifacts, so someone can
build their own image. `howto-console.md`, "One image or two" and "The
door-only statement", say so.

---

## 4 · Tokens and keys

### 4.1 The token

A compact JWS (RFC 7515), 3 segments, `alg: EdDSA` over Ed25519 (RFC 8037).
Chosen because `ordo_identity::jose` already verifies exactly this with a
JWKS and a `kid`, and because the door tells it from an app token by shape:
an app token is 2 segments with an HMAC (`ordo_identity::token`), a
deployment token is 3. The door never tries one verifier on the other's
string.

Header: `{"alg":"EdDSA","kid":"<kid>","typ":"ordo-deployment+jwt"}`.

| Claim | Means |
|---|---|
| `iss` | `ORDO_IDENTITY_ORIGIN` |
| `aud` | `ordo-deployment:<handle>`. The door and the console of that deployment accept it. An app token has no `aud` and a different format, so the realms cannot be confused |
| `sub` | `p_…` a person, `m_…` a machine key, `breakglass:<unix user>` for section 5.8 |
| `kind` | `person`, `machine`, `breakglass` |
| `role` | `admin`, the only role for now |
| `client` | `console`, `cli`, or the machine key's name. Audit only |
| `sid` | the session id, so the audit joins sessions and the door can refuse a session the service ended |
| `touched_at` | the last passkey assertion in this session, as seconds. Absent on a machine token |
| `iat`, `exp`, `jti` | issued, expiry (`iat` + 600 by default), id |

The door maps `touched_at` to `user_presence_at` in `TokenClaims`, so the
existing step-up gate (300 seconds, `DEFAULT_STEP_UP_MAX_AGE_SECS`) works
unchanged. A write older than that answers `presence_required`, and the
client asks the service for a reauth (5.4, 5.5).

Machine tokens carry no reauth. A machine key satisfies step-up on platform
ops in the home space by policy, because CI applying a program is the point
of the key (open question Q2). It never satisfies anything in a tenant
space.

### 4.2 Keys, publishing, caching, rotation

- At first boot the service makes 2 Ed25519 pairs, one for tokens and one
  for audit checkpoints, seals the private halves with
  `ORDO_IDENTITY_KEY_FILE` and stores them in `signing_keys`.
- `GET /.well-known/jwks.json` publishes the public halves with `kid`,
  `use: sig`, `alg: EdDSA`, plus `Cache-Control: max-age=600`.
- The door and the console each hold 1 client to the service
  (`identity_keys.rs` in each, the one typed error `IdentityKeysError`, per
  AGENTS.md rule 9). It fetches the JWKS at boot, refreshes every 10
  minutes, and once at most per minute on an unknown `kid`. The door also
  writes the set to `$identitykeys` in the control database and reads it at
  boot, so a restart with the service down still verifies. Issued tokens
  keep working while the service is down; only new sign-ins and renewals
  wait for it.
- Rotation, every 90 days or `ordo-identity-service rotate-keys`: a new
  pair is added to the JWKS and used for signing 1 hour later, so every
  cache has it. The old pair stays published 24 hours after its last token
  expired, then is retired. A retired key verifies nothing.
- Compromise: `rotate-keys --now` signs with the new key at once and
  retires the old one; every token dies within 10 minutes. It is written to
  the audit log.

### 4.3 The realm boundary at the door

`IdentityAccess::check` (`lib.rs`) grows one branch before the HMAC
verifier: a 3 segment token is verified against the cached JWKS, `iss`,
`aud`, `exp`. Then:

| Caller | Platform op bound to the home space | Read-only tenant admin ops (`console_keys::TENANT_OPS_OPEN` today) | Any app op, any space | Any other op in a tenant |
|---|---|---|---|---|
| Deployment person, touch fresh | allowed | allowed | denied | denied |
| Deployment person, touch stale | reads allowed, writes `presence_required` | `presence_required` | denied | denied |
| Machine key | allowed, writes included | denied | denied | denied |
| App owner of the home space | denied | denied | as today | as today |
| App user, app capability token, share link, delegation | denied | denied | as today | as today |

Every denial is the door's one answer, `403 access_denied`, with the reason
in the security record. The home space's founding row (`system:platform`,
`SYSTEM_PRINCIPAL`) stays: the door still resolves the space, and nobody
signs in as it. What goes is any app principal's right to platform ops
through an owner row in the home space (section 6, `ORDO_DEPLOYMENT_OWNERS`).

The door's `/auth/*` routes refuse a deployment token: it has no app
session to manage. Its session lives at the identity service.

---

## 5 · Flows

### 5.1 The first admin, from provisioning (Ordo Cloud)

1. The provisioning service makes an Ed25519 pair per instance and puts the
   public half in the container's environment as
   `ORDO_IDENTITY_PROVISIONER_KEY`.
2. At first boot the service finds an empty `people` table and the key, and
   writes machine key `provisioner` with role admin. Recorded.
3. The provisioning service signs in as that machine (5.6) and calls
   `POST /invites {label, role: admin}`. It gets the link and hands it to
   the customer by its own mail. The customer enrols (5.2).
4. The same machine key makes CI keys and more invites later: it holds a
   minting grant for `deployment` keys (owner, 2026-11-05: a key mints only
   within its minting grant, `docs/howto-identity.md` §6). Ending it is
   `POST /machine-keys/m_…/end`, from an admin.

### 5.2 The first admin, from first boot (self-hosting)

1. First boot, empty `people` after the import of 6.1 (nothing to import,
   or no console database named), no provisioner key: the service mints 1
   invite link, prints it once to its log, and writes it to
   `/var/lib/ordo-identity/first-invite` (0600, deleted when used). It
   lasts 7 days (`ORDO_IDENTITY_INVITE_DAYS`). An import that brought a
   live admin means no first invite.
2. The deployment owner reads it with `docker compose logs` and opens it.
3. Any later invite from the host is `docker exec <c>
   ordo-identity-service invite --label "…"`, recorded under
   `unix:<user>`.

No page claims anything. A link is minted on the server, once, and spent
once.

### 5.3 Invite link and sign-up

1. An admin opens People in the console, or runs `ordo people invite
   --label "Dana"`. The console calls `POST /invites` with the admin's
   token. The link is `https://<origin>/join#<secret>`; the secret rides in
   the fragment, so it reaches no server log.
2. The console shows the link to copy. Sending it by mail is offered once
   mail is set up (ruling: "Email later").
3. Dana opens it. The service's page checks the hash: unknown, expired,
   spent or revoked is one answer, "this link does not work", with the
   reason in the audit log.
4. Dana types a display name and registers a passkey against the service's
   relying party. 10 recovery codes are shown once. The invite is marked
   used by `p_…`.
5. Dana lands signed in, with a browser session cookie (`HttpOnly`,
   `SameSite=Lax` so the redirect in 5.4 carries it, its own origin).
6. The invite's row records issuer, time, expiry, who used it and when.
   `POST /invites/<id>/revoke` kills an unspent one.

**Open sign-up, built 2026-11-19.** Sign-up is per app (owner,
2026-11-17). A registered deployment in `open` mode takes people with no
invite: `/sign-up?deployment=<handle>&return=<url>` on the service's page,
`POST /sign-up/begin` and `/sign-up/finish` behind it. Steps 4 and 5 are
the same code as an invite's: one passkey, 10 recovery codes, a browser
session. A person already signed in joins with `POST /sign-up/join`. The
person becomes an app member of that deployment only (11.2).

### 5.4 Console sign-in

The console holds no owners and no passkeys of its own any more. The
browser holds the identity service's session, and the console page gets
tokens from it. An owner enrolled at the console before the upgrade signs
in here with that same passkey (6.2); nothing is re-enrolled.

1. The console page finds no token and sends the browser to
   `<identity origin>/sign-in?client=console&return=<console origin>/`.
2. The service page runs the passkey ceremony, or finds a live session and
   asks for nothing. It redirects back.
3. The console page calls `POST <identity origin>/token {aud}` with
   `credentials: include` (CORS allowed by `ORDO_IDENTITY_CLIENT_ORIGINS`).
   It gets a 10 minute token and keeps it in memory, never in storage. It
   renews 1 minute before expiry the same way.
4. Every console API call carries the token as a bearer. The console server
   verifies it with the cached JWKS, as the door does.
5. For in-app management the console server calls the door with the same
   token, as that person. The console no longer holds a credential of its
   own at the door. G14 (console key) and G15 (relayed touch) go.
6. A write the door refuses with `presence_required` needs a reauth. The
   console page asks the service for the ceremony (`POST /touch/begin`) and
   reads the RP ID in the options. One touch covers 300 seconds, as today.
   - **Console host is the RP ID or under it:** the ceremony runs in the
     console page. The browser signs the console's origin into the
     assertion, and the service accepts each client origin under its RP ID
     for that (built in step 6). `POST /touch/finish` answers a token with
     a fresh `touched_at`, and the page retries.
   - **Any other host** (Human Agent Link's console, owner, 2026-11-15): a
     passkey made under RP ID `hominem.ai` cannot be used at another
     host, so the page goes to the service's
     `/sign-in?client=console&deployment=<handle>&return=<url>&touch=1`.
     A live session sees "Confirm it's you" and one passkey button; with
     no session the normal sign-in runs, which is a fresh touch itself.
     The browser then returns to the console. The next token the page
     fetches carries the touch.
   - After the return the person repeats the action. Carrying the pending
     action across the redirect is a later improvement.

   The console answers the door's `presence_required` and its own stale
   reauth with the one word `step_up_required`.
7. Sign-out ends the service session (`POST /sessions/end`); the console
   forgets its token.

The service down means no new sign-in anywhere, for up to 10 minutes of
leftover tokens. It is a sibling under the supervisor, restarted in 1
second, so that window is short. Runtime fixes that need no sign-in do not
exist; break-glass (5.8) is the way in when the service itself is broken.

### 5.5 `ordo login`

Browser plus a loopback callback, with PKCE. Chosen over a device code
because the owner asked for "the cli login through the browser (session)",
a loopback flow has nothing to type and binds the browser to the one CLI
process that started it, and a headless host has the machine key (5.6). A
device code can be added later if SSH-only hosts need it (open question Q5).

1. `ordo login --identity https://id.example.com` (or `ORDO_IDENTITY_URL`).
   The CLI listens on `127.0.0.1:<random port>`, makes a PKCE verifier and
   a state, and opens the browser at `<origin>/cli?port=…&state=…&
   code_challenge=…`. It prints the URL too, for a browser on another
   machine.
2. The person signs in (passkey) and sees "Sign the CLI in on this
   computer? Port 51234". Approving mints a `cli_grants` row (60 seconds)
   and redirects to `http://127.0.0.1:<port>/callback?code=…&state=…`.
3. The CLI checks the state and calls `POST /cli/redeem {code, verifier}`.
   It gets a CLI session: a renewal secret, 7 days idle and 30 days at
   most by default (`ORDO_IDENTITY_SESSION_IDLE_HOURS`, `_MAX_HOURS`, Q9),
   stored in `~/.config/ordo/sessions.json` (0600), keyed by the identity
   origin.
4. Every command gets a token with `POST /token {session, aud}` and sends
   it to the door as the bearer. The token carries `touched_at` from the
   login.
5. After 300 seconds a platform write answers `presence_required`. The CLI
   says so and runs `ordo login --touch`: the same loopback flow, an
   assertion only, no new session. CI never hits this: it uses a machine
   key.
6. `ordo logout` ends the session at the service and deletes the file
   entry. `ordo whoami` prints `sub`, kind, role and the session's end.

`ORDO_TOKEN` keeps working for a raw token (`wire.rs`). The session is the
default when none is given.

### 5.6 Machine keys

1. An admin makes one: People, "Machine keys", or `ordo people machine-key
   add --name ci-main`. The service makes the Ed25519 pair, stores the
   public half, and shows the private half once as
   `ordo-mk-v1:<kid>:<base64>`. Made by the service so a CI secret is one
   value to paste; a key the machine made can be registered instead with
   `--public-key`.
2. Use: `ORDO_MACHINE_KEY` in CI. `ordo apply` calls
   `POST /machine/begin {key}` for a challenge (nothing stored, MAC under a
   derived key, 60 seconds, as `console_keys` does), signs it, and
   `POST /machine/complete {key, challenge, signature}` answers a 10 minute
   token, `kind: machine`. Throttled per node.
3. The door lets it apply, activate, set quotas, pause tenants: platform
   ops in the home space. Every act is recorded under `m_…` and the key's
   name.
4. Ending: `POST /machine-keys/m_…/end`. New sign-ins refuse at once;
   tokens die within 10 minutes.

### 5.7 Ending access

1. An admin ends a person: People, "End access", a typed confirmation and
   a reauth, or `ordo people end p_…`. The service sets `ended_at`, ends
   every session, voids recovery codes and unspent invites they issued.
2. New sign-ins and renewals refuse. Live tokens die within 10 minutes
   (`ORDO_IDENTITY_TOKEN_SECS`, at most 3600: ending access takes effect
   within that many seconds at the door and the console);
   the door refuses `sid` values the service lists as ended if the door
   polled them (open question Q3; v1 is expiry only).
3. The last admin cannot be ended; the answer names break-glass. Machine
   keys outlive the person who made them, because they belong to the
   deployment.
4. This covers support for Ordo Cloud: the owner's support access is a
   person or machine key in the customer's realm that the customer can end,
   and every act of it is in the audit log. No support window exists
   (ruling: "No support account for now").

### 5.8 Break-glass, host only

Two commands, run inside the container only (`docker exec`, `kubectl exec`).
Neither adds power: whoever can run them already holds the host.

- `ordo-identity-service invite --role admin`: a new admin's invite link.
  This is also recovery when every passkey is lost.
- `ordo-identity-service break-glass-token --minutes 15`: prints one admin
  token, `sub: breakglass:<unix user>`, `kind: breakglass`, with a fresh
  `touched_at`, for `ordo --token`. For when the page cannot be reached.

Both are written to the audit log with the unix user and time, and the
console's Health screen shows "break-glass used at …" for 7 days.

---

## 6 · What happens to today's pieces

| Today | Becomes |
|---|---|
| Console owners, passkeys, sessions, recovery codes, setup codes (`store.rs` migration 1, `codes.rs`, `passkeys.rs`, `recover.rs`, `server.rs` sign-in routes, the first-run setup code) | Gone from the console. The code moves to the identity service (2.1). The rows move too: owners, passkeys and live recovery codes are copied into the service at its first start (6.1). The console tables are left as they are and dropped in a console migration one release after the cut-over, so a rollback keeps its data. `ordo console recover` is replaced by 5.8 |
| Console key G14 (`link.rs`, `credentials` table, door's `console_keys.rs`, `auth/console.rs`, `std.deployment.addConsoleKey`, `revokeConsoleKey`, `consoleKeys`, `ORDO_AUTH_CONSOLE_SESSION_SECS`, `$consolekey`) | Gone. The console calls the door as the signed-in person (5.4). An existing link needs nothing from the owner: the new console never reads its sealed key, the door never sees it again, and both rows are dropped one release later. The Deployment screen's link step becomes "Is the door reachable?" only. Step 6 closes it wherever `ORDO_IDENTITY_URL` is set: the console refuses to boot while it holds a key (`ordo-console forget-door-key` forgets it, audited), and the door refuses console key sign-in and every console key session. The code stays for a console without the service until the one image (step 7) carries the service everywhere; step 8 removes it |
| Relayed reauth G15 (`/auth/step-up` for a console session) | Gone. The reauth is at the identity service and rides in the token |
| `ORDO_DEPLOYMENT_OWNERS` (`provision.rs`, `runtime.rs`, `deploying.md` section 1) | Ignored wherever `ORDO_IDENTITY_URL` is set, with a boot-log line naming it, never a refusal (owner, 2026-10-30, replacing the boot refusal first planned). A door without the identity service reads it as before. Existing owner rows in the home space stay as app memberships and give no platform rights |
| Claim code (`ORDO_AUTH_CLAIM_CODE`, `std.deployment.setClaimCode`, `admission::ClaimStore`, `self_serve.rs` "whether a claim code is set", `boot_record.rs` line 213) | Gone. First-comer admission for app tenants stays as the app developer's setting, off by default, and is refused for the home space. Built in step 8 on every deployment (owner, 2026-10-30: "No Remove it now."): a door that still sets `ORDO_AUTH_CLAIM_CODE` boots and logs it as ignored, naming the first-comer setting; the stored row waits for the drop migration |
| The `system` space's founding row `system:platform` | Stays. It is a row to resolve, never a sign-in |
| `docker/compose.split.yml`, Dockerfile targets `host` and `console`, the console-only build and its `supervisor` feature, `images.yml` jobs for the 2 images, `tests/door_only.rs` | Gone, per 3.3. The binaries remain in the one image and in the release artifacts |
| `first-deployment.md` step 1 ("Choose the one image or the split images"), step 5, step 6 | Rewritten: one image; sign in by the first invite link; no link step |

### 6.1 Migration for an existing deployment

The live notes v2 deployment must survive this (2026-10-19), and the owner
ruled that an upgrade affects nothing (Q7): nobody re-enrols. The cut-over
is one image upgrade:

1. The new image boots. The supervisor finds a console volume with no
   supervisor spec and builds revision 1 from the environment, as the
   console does today. The console's revision history stays in its
   database.
2. The identity service boots on its new database, runs its migrations,
   and then the import below. The console's owners are now its people,
   with their passkeys. No first invite is printed.
3. The owner opens the console. It sends them to the service (5.4), which
   asks for a passkey. Their old passkey answers, because the relying
   party is the same (6.2). They are in.
4. The console's old owner tables are left untouched by the new console
   and dropped one release later.
5. The door's old console keys and `$consolekey` rows are ignored and
   dropped one release later. Any `ORDO_DEPLOYMENT_OWNERS` value in the
   compose file is ignored and named in the boot log; the owner deletes
   the line when convenient.
6. Rehearsed on a copy of the live database before the release, as the
   notes e2e upgrade test does, with step 3 as the proof (section 8,
   "Upgrade").

**The import.** The identity service runs it itself, at every boot, before
it listens. It is a no-op unless `ORDO_IDENTITY_IMPORT_CONSOLE_URL` is set
and `imports` has no row for `console:<handle>`.

- It opens the console database with that URL, reads only, and inside one
  transaction on its own database copies the rows of
  `ORDO_IDENTITY_DEPLOYMENT`'s handle.
- What moves: every `owner` row, including removed ones, as a person. A
  live owner is a live admin; a removed one is a person with `ended_at` set
  to `removed_at` and `ended_by` `console`, so old audit lines still name
  someone. The console's label becomes the display name; the owner id is
  kept in `console_owner_id`, the new `p_…` is the token's `sub`.
- Every `passkeys` row, column for column: credential id, algorithm,
  public key, sign count, user handle, backup flags, registered at, last
  used. The user handle is kept as it is, because a discoverable
  credential hands it back at sign-in and the service looks the person up
  by it.
- Every `recovery_codes` row that is unused and not voided, with its
  lookup and hash as they are. `codes.rs` moves with them (2.1), so the
  hashing is the same and the printed codes still work.
- Not moved: `sessions` (a console cookie on the console's origin; the
  service's cookie is on its own origin, so one reauth at the
  first visit is the whole cost), and `setup_codes` (unspent console
  invite and recover links point at console pages that no longer exist).
  Each unspent setup code gets an audit row naming it as voided, so an
  admin knows to issue a new link.
- Idempotent. Each copy is `ON CONFLICT DO NOTHING` on the console's own
  ids, and the `imports` row is written last in the same transaction. A
  crash mid-way imports nothing; the next boot does it all. A second
  console pointed at the same service is a second `imports` row, never a
  duplicate person.
- Recorded: 1 audit row for the import (actor `boot`, the console handle,
  the RP ID in force, the counts) and 1 per person moved. The
  `imports` row keeps the counts and the RP ID, and a later boot whose RP
  ID differs from it logs a warning that imported passkeys will not
  assert (6.2).
- The console's tables are never written to. A rollback to the old image
  finds them as they were.

### 6.2 The relying party must not change

A passkey is bound to the relying party id it was registered under, and a
browser only asserts it for a page whose host is that id or under it. The
console registered its passkeys under `ORDO_CONSOLE_RP_ID`, or the host of
`ORDO_CONSOLE_ORIGIN` when unset (`config.rs`). The identity service takes
that same id, by the supervisor's default (3.1), and the deployment owner
keeps 2 things as they are:

- `ORDO_CONSOLE_ORIGIN`, and `ORDO_CONSOLE_RP_ID` if it was set, exactly
  as before the upgrade. The console still needs its origin for its own
  cookie and for `ORDO_IDENTITY_CLIENT_ORIGINS`; the RP ID is what the
  passkeys answer to.
- `ORDO_IDENTITY_ORIGIN`'s host is that RP ID or a host under it. The port
  is free: WebAuthn ignores it, so `https://console.example.com:8082` and
  `https://id.console.example.com` both work under `console.example.com`,
  and `https://id.example.com` does not. A proxy in front maps the chosen
  address to the service's port 8082.

On `localhost` the RP ID is `localhost` and the port is ignored, so the
compose file's `http://localhost:8081` console and `http://localhost:8082`
service share it with nothing to set. That is the live notes v2 case.

Boot refuses an `ORDO_IDENTITY_ORIGIN` outside the RP ID, naming both
settings and the 2 shapes that work. The console page's redirect (5.4) is
to the identity origin, so once the service boots the old passkeys assert
there. A deployment owner who truly wants another host accepts one
re-enrolment per person, by setting `ORDO_IDENTITY_RP_ID` to the new host
and sending invites; the service says so in that refusal, since it is the
only path that loses the old passkeys.

---

## 7 · Audit

Three records, each signed, each with who.

| Record | Where | What | Signed by |
|---|---|---|---|
| The identity service's | its `audit` table, `hash = SHA-256(prev ‖ body)`, a checkpoint every 100 entries or 10 minutes | invite issued, used, revoked; sign-up; sign-in; touch; session ended; CLI approved; machine key made, first used, ended; person ended; break-glass; key rotation; provisioner made. Every row: actor (`p_…`, `m_…`, `unix:…`, `boot`), subject, outcome, client, time. Tokens issued are not rows; sessions are |
| The door's | the deployment's security stream (`ordo-secevent`), as today | every platform op with `sub`, `kind`, `client` and `sid` from the token, allowed or denied; every refused deployment token (`TokenUnverifiable`, wrong audience, stale touch) | `ORDO_SECURITY_CHECKPOINT_KEY` (the security stream's key; corrected 2026-10-28 from `ORDO_CHECKPOINT_KEY`, which signs the command log): made at first boot and stored sealed when unset, instead of optional (board ask 4, "ok"). Set is still honoured |
| The console's | its `audit` table, as today | runtime applies, restarts, rollbacks, invites it relayed, with the person's `sub` | gains the same chain and checkpoint as the identity service, sharing the module |

`ordo-identity-service audit verify` and `ordo verify` check a chain
against its checkpoints and a public key; a break says the entry and the
kind, in the vocabulary `ordo_commandlog` has (`TamperAt`, `TamperKind`).
The console shows the service's log on a "Deployment people" screen,
newest first, and a red line when verify last failed.

---

## 8 · Tests

| Level | Where | Proves |
|---|---|---|
| Unit, identity service | `crates/ordo-identity-service/src/*` | token mint and verify; `aud`, `iss`, `exp`, `kid` refusals are one error; invite single use and expiry; session idle and absolute ceilings; PKCE check; chain hash and checkpoint; last admin refused; throttle |
| Store, identity service | `crates/ordo-identity-service/tests/store.rs` (Postgres) | migrations; the boot refusal on the deployment's or console's database; first-invite once; provisioner key once |
| Import, identity service | `crates/ordo-identity-service/tests/import.rs` (Postgres, a console database made by the console's own migrations) | 2 owners, 3 passkeys and live recovery codes move column for column; a removed owner arrives ended; used and voided codes stay behind; unspent setup codes are voided in the audit; a second boot imports nothing; a boot killed before the `imports` row redoes it all with no duplicate; no console URL means no import; the console tables are unchanged byte for byte; a live import means no first invite; the RP ID refusal names both settings |
| Upgrade | `crates/ordo-identity-service/tests/upgrade.rs`, with the SDK's software authenticator | a passkey registered against the console's relying party on a copy of the live schema asserts at the service after the import, under RP ID `localhost` from port 8082 and under `console.example.com` from `console.example.com:8082`; the same passkey fails under `id.example.com`, which is the refusal, not a silent sign-in failure; an old recovery code signs in once |
| Reauth, identity service | `crates/ordo-identity-service/tests/page.rs`, `tests/flows.rs` | `/sign-in` serves the reauth step; a touch taken at the service page with no `aud` shows in the next `POST /token` for a deployment |
| Host integration | `crates/ordo-host/tests/identity_tokens.rs`, with an in-process fake issuer (a signer and a JWKS route) | admin token reaches platform ops; machine token applies; stale touch answers `presence_required`; tenant op denied; app op denied; app owner of the home space denied platform ops; ended `sid`; unknown `kid` refetches once; service down, cached and `$identitykeys` still verify; 2 segment token never hits the JWS path |
| Authorization matrix | `tests/authz_support/actors.rs`, `policy.rs` | new actors: `deployment_admin` (fresh), `deployment_admin_stale`, `machine_key`, `cli_session`, `deployment_ended` (Dead), `deployment_wrong_aud` (Dead), `deployment_foreign_issuer` (Dead). Realm proofs as rows: every app actor × every platform op is `Denied`; every deployment actor × every app op in every space is `Denied`; `machine_key` × every tenant op is `Denied`. The route guard keeps every new route in the policy |
| Console | `crates/ordo-console/tests/console.rs`, `door_link.rs` becomes `door_as_person.rs` | sign-in by redirect with a fake issuer; API refuses a token for another `aud`; door calls carry the person's token; `presence_required` surfaces as a reauth ask; `status.identity.rpId` follows `ORDO_IDENTITY_RP_ID`, else the origin's host; no runtime routes without the socket setting |
| Supervisor | `crates/ordo-supervisor/tests/` with the fake host | 3 children start; kill each one, the other 2 keep answering; crash loop in one; the socket refuses `apply` for any child but `deployment`; the socket is unreachable as 65533; revision 1 from the environment; a sealed spec survives a restart |
| Images smoke | `images.yml`, the `ordo` job | 3 processes as 65531, 65532, 65533; the supervisor's caps and the console's lack of them; `/.well-known/jwks.json` answers; the first invite is printed; `docker kill` of `ordo-console` leaves the door answering; `break-glass-token` plus `ordo apply` of the notes program succeeds |
| Console e2e | `clients/ordo-console-web/e2e` | sign-up by invite link; console sign-in; invite a second admin; end their access and see the refusal; make a machine key; reauth in the console page, where the console's host is the RP ID (the redirect case is not in e2e: the stack lists one console origin on `localhost`, and a second origin would change it for every test; the redirect is guarded by the page and flow tests and the build); the upgrade case: enrol at the old console, upgrade the image, open the console and sign in with that same passkey, no invite, no setup code |
| CLI | `crates/ordo-cli/tests/login.rs` | the loopback flow against the fake issuer; the session file mode; `--touch`; `ORDO_MACHINE_KEY` sign-in; `ORDO_TOKEN` still wins when given |

---

## 9 · Steps

Each step merges on its own and the tree works after it. Sizes per
AGENTS.md "Sizing work". A CI round is about 12 minutes.

| Step | What | Size | Pace set by |
|---|---|---|---|
| 1 | **The service, server side.** Crate, store, migrations, signing keys, JWKS, passkeys and recovery codes moved from the console, invites, people, sessions, `/token`, `/touch`, machine keys, audit chain, first-invite boot, provisioner key, `invite`, `break-glass-token`, `rotate-keys`, `audit verify`. Then the console import of 6.1 with its tests and the RP ID refusal of 6.2, as the third session | L (3 sessions) | 3 CI rounds; a review of the token claims after the first session and of the import after the third |
| 2 | **Its page.** Sign-up by link, sign-in, CLI approval, account (passkeys, recovery codes). Plain HTML and JS served by the service, the SDK's passkey ceremony code. Invokes `modern-web-guidance` first | M | 1 CI round |
| 3 | **The door verifies.** `identity_keys.rs` client and cache, `$identitykeys`, the branch in `IdentityAccess::check`, the realm table of 4.3, `sub` in the security stream, `ORDO_SECURITY_CHECKPOINT_KEY` made at first boot, host integration tests | M | 2 CI rounds |
| 4 | **Authorization matrix.** The new actors and the realm rows | M | 1 CI round, `scripts/authz-matrix.sh` |
| 5 | **The CLI.** `ordo login`, `--touch`, `logout`, `whoami`, the session file, machine key sign-in, `ordo people` (invite, end, machine-key) | M | 1 to 2 CI rounds |
| 6 | **The console on the service.** Sign-in redirect, bearer on the API, door calls as the person, step-up through the service, the People screen for deployment people and machine keys, the service's audit on screen; G14 and G15 code paths removed from the console and the link screen cut to a reachability check | L (2 sessions) | 2 CI rounds; a security review between them; the console e2e cases |
| 7 | **The supervisor and the one image.** `ordo-supervisor` crate from the console's module, 3 children, socket, sealed live spec, the 3 identity defaults filled from the console's settings (3.1), Dockerfile, `compose.one.yml`, `images.yml` smoke, the split targets and `compose.split.yml` removed, `first-deployment.md` and `howto-console.md` rewritten. The upgrade forgets the old console key itself (step 6 made a console in identity mode refuse to boot while it holds one), so the owner does nothing | L (2 sessions) | 2 CI rounds; a rehearsal that kills each child in turn |
| 8 | **Removals at the door.** Console keys, the claim code, `ORDO_DEPLOYMENT_OWNERS` refused with its message, `deploying.md`, the authz policy without the `console` actors, the drop migrations scheduled. Owner, 2026-10-30: only where the identity service runs; a deployment without it keeps the old sign-in with a banner and a boot-log line, and `ORDO_DEPLOYMENT_OWNERS` is ignored with a log line, never refused. Built: no console key is made with `ORDO_IDENTITY_URL` set; the claim code is removed everywhere (owner, 2026-10-30) and the home space refuses a first-comer; the drop migrations wait for the announced release that removes the old sign-in | M | 1 CI round |
| 9 | **Upgrade rehearsal and docs.** `howto-identity.md` (people, invites, machine keys, CLI, break-glass, backups: the identity database and `ORDO_IDENTITY_DATA_DIR` mounted as a persistent volume, owner 2026-10-27, with `ORDO_IDENTITY_KEY_FILE` in it, what to keep the same in `ORDO_CONSOLE_ORIGIN` per 6.2), the e2e upgrade case, and a rehearsal on a copy of the live notes v2 deployment that ends with the owner's existing passkey signing in | M | the rehearsal; the owner's go for the live upgrade |

About 12 to 14 sessions and 15 to 17 CI rounds. Steps 1 to 3 run in order.
After 3, steps 4, 5 and 6 are independent and 2 may run at once on
separate worktrees. Step 7 needs 6 (the console without its own owners).
Step 8 needs 6 and 7. Step 9 is last. Ordo Cloud's provisioning call (5.1)
is usable after step 1 plus 3; `ordo login` after step 5.

Each step updates `roadmap-checklist.md` 18o and the board in the same
commit, and `node scripts/board-agrees.mjs` passes before it.

---

## 10 · Questions for the owner, answered 2026-10-27

Asked on the board as `identity-service-plan-10-questions` and recorded
in `corpus-rulings.md`, "the identity service plan, 10 answers". 8 took
the recommendation. Q7 and Q9 changed the plan; the sections named carry
the change.

| # | Question | Recommendation | Answer |
|---|---|---|---|
| Q1 | Where does the service's database live? Its own cluster, or a second database on the console's cluster? | A second database on the console's cluster, which is what the compose file does. No new cluster. A self-hoster may point it anywhere | Yes. As recommended (2.2) |
| Q2 | May a machine key satisfy the reauth on platform writes (apply, activate, quotas) with nobody present? | Yes, in the home space only. CI applying a program is what the key is for. Never in a tenant space, and every act is recorded under the key's name | Yes. As recommended (4.1, 4.3) |
| Q3 | Ending access: is "tokens die within 10 minutes" enough for v1, or should the door poll the service every 60 seconds for ended sessions? | Expiry only for v1. Add the 60 second poll later if a case needs it | Yes. Expiry only (5.7) |
| Q4 | May a deployment person still read tenant admin ops (members, grants, `callersOf`) with a reauth, as the console key can today? The console's People screens depend on it | Keep it, read-only, touch-gated, the exact list `TENANT_OPS_OPEN` holds today. Writes inside a tenant stay the tenant's own | Ok. As recommended (4.3) |
| Q5 | `ordo login`: loopback callback only, or also a device code for SSH-only hosts? | Loopback only in v1. A headless host uses a machine key. Device code is a later S | Ok. As recommended (5.5) |
| Q6 | The claim code goes everywhere. Does first-comer admission for app tenants stay, as the app developer's setting, off by default? | Stays. It is an app-realm choice, and the home space refuses it | Yes. As recommended (6) |
| Q7 | The live deployment's upgrade: console owners enrol a new passkey by invite link; the old console passkeys are left behind. Accept? | Accept. Passkeys are bound to the console's origin; moving them is a bigger build than the service | **No.** "They shouldn't need to re-enrol. Upgrade should affect nothing." Owners, passkeys and recovery codes move in by a one-time import at the service's first start, and the service keeps the console's RP ID so the passkeys still assert (6.1, 6.2). Step 1 grew by 1 session (9) |
| Q8 | The service's public address: its own host (`id.example.com`, port 8082 in the image) or a path under the console? | Its own host and port. Passkeys bind to it, the console page talks to it with CORS, and the proxy example maps it | Yes. Its own host and port, with 6.2's rule: the host is the console's RP ID or under it, the port is free |
| Q9 | Lifetimes: 10 minute tokens, 30 days idle and 90 days absolute for browser and CLI sessions, 7 day invites | As stated, each a setting with a range | **Changed.** Sessions 7 days idle and 30 days at most by default, both settings; the owner expects 24 or 12 hours to be common, so `ORDO_IDENTITY_SESSION_IDLE_HOURS` and `_MAX_HOURS` take hours. Invites 7 days (`ORDO_IDENTITY_INVITE_DAYS`). Tokens 10 minutes (`ORDO_IDENTITY_TOKEN_SECS`). (2.3, 5.5) |
| Q10 | May the console restart the identity service through the supervisor, or only the deployment? | Only the deployment. The supervisor restarts a crashed identity service itself. The console shows its status and logs | Ok. As recommended (3.2) |

---

## 11 · What this plan does not do

- Roles beyond `admin` ("We'll have to get into permission breakdown
  later"). Open sign-up's `member` is a membership with no token (11.2).
- Outside providers for deployment people. The client code exists
  (`ordo_identity::oidc`, `oauth2`); wiring it to the service is a later M.
- Sending invite links by mail. Offered once mail is set up.
- A support window for Ordo Cloud (`support-window`, filed).
- Screens for many deployments. The routes are built (below); a web page
  listing them is not.
- Machine keys for a registered deployment. Machine keys stay with the
  service's own deployment.
- Reauth in a console page whose host is outside the service's RP ID.
  Related Origin Requests could allow it; not built.
- Moving people from a deployment's own service to a shared one. It is a
  new relying party, so each person joins again by invite.

### 11.1 Many deployments in one service, built 2026-11-14

The owner's go: "lets build the shared auth". One service at
`https://auth.hominem.ai`; `hal` and the `ordo` test instance sign in
there. Code: `crates/ordo-identity-service/src/deployments.rs`; tests:
`tests/shared.rs`; how to: `howto-identity.md` section 14.

- The service's own handle is its home. Its people are accounts: one
  passkey, one session, every deployment.
- Migration 4: `deployments` (handle, client origins, return URLs),
  `deployment_members`, `people.home`, `invites.target`. Every person
  before it keeps home access.
- `POST /token` and `/touch/finish` issue for the home or a live
  registered deployment. The person must be a member, and a browser page
  must be one of that deployment's client origins. The token says who;
  the deployment's members say who may.
- `POST /sign-in/return` checks a return address against the named
  deployment's list. The sign-in page asks it before it sends the browser
  on. A path refuses any whitespace, control character or second leading
  separator, the rule `Origin::of_url` applies.
- The touch needs no membership: every person manages their own
  account. A token from it needs the membership of its `aud`.
- Ending a deployment ends its memberships. Registering an ended handle
  again starts empty, and is refused while live members are left.
- The registry is held in memory and refreshed each minute and on every
  change this process makes; only the membership is read per call.
- A registered deployment's token reaches its own members and invites.
  Ending a person from it ends the membership, never the account.
  Machine keys, the audit log and the registry answer the home's token
  alone.
- Registering: `POST /deployments` (an admin with a fresh reauth, or the
  provisioner's machine key) or `ordo-identity-service deployment add` on
  the host.
- The supervisor starts no identity service when `ORDO_IDENTITY_URL` is
  not loopback, or `ORDO_SUPERVISOR_IDENTITY_ADDRESS=external`, and hands
  the door and the console the wiring as set.
- The console's `ORDO_IDENTITY_DEPLOYMENT` may differ from its own handle.
- The binary runs alone as its own container; a test boots it with its
  own settings only. The shared provider is this service by itself: no
  deployment wraps it and no app lives beside it (owner, 2026-11-15). The
  account app, where a person manages passkeys, is a client of its contract
  like any other app.

### 11.2 Open sign-up per deployment, built 2026-11-19

The owner's ruling, 2026-11-17: sign-up is per app. The example notes
apps use open sign-up with a passkey; invites stay beside it. Code:
`deployments.rs` (`SignUp`), `flows.rs` (`open_begin`, `open_finish`,
`open_join`); tests: `tests/open_sign_up.rs`; page: `page/sign-up.*`;
how to: `howto-identity.md` section 14.

- Migration 5: `deployments.sign_up` (`invite` by default, so every
  earlier row keeps its behaviour) and `deployment_members.role`
  (`admin` by default, as every earlier membership was).
- The mode is set at registration (`signUp`, `--sign-up`) and changed by
  `POST /deployments/<handle>/sign-up` or `deployment sign-up`, with the
  service's own token. The home is never open (`home_invite_only`).
- Open sign-up and the invite flow share one account path: passkey,
  recovery codes, session. Only the way in and its audit row differ
  (`invite.used`, `sign-up.open`, `sign-up.joined`).
- An open member's membership has role `member`. It holds no token for
  the deployment, because a token's only role is `admin` and a stranger
  must not become an app's admin. `POST /token` answers
  `app_member_only`. An invite or `member add` makes them an admin. The
  last-admin rule counts admins only.
- The mode is read from the database on each call, so a change counts at
  once on every instance.
- Open sign-up is counted per address under its own kind,
  `ORDO_IDENTITY_SIGN_UPS_PER_ADDRESS` (10 a minute), with the shared
  ceiling of the other kinds.
- Signing app members in to the app: built 2026-11-19 (11.3).
### 11.3 The service as an OIDC provider, built 2026-11-19

The owner's ruling, 2026-11-17, ask 3: "the identity service is Ordo's
built-in credential system, and it signs in app users as well as
deployment people ... Each deployment connects to it as an OIDC provider
under one connection name, so one person has one subject both
deployments recognise." Code: `src/oidc.rs`; tests:
`tests/oidc_provider.rs`, `ordo-host/tests/identity_service_oidc.rs`;
how to: `howto-identity.md` section 14, connect an app.

**Amendment to 4.3.** The app realm and the deployment realm stay apart
at the door: a deployment token reaches platform ops only, an app token
app ops only. They now meet in one place, where a deployment connects to
the service as an OIDC client. One account at the service signs a person
in to the console as an admin and to an app as its user. The two tokens
stay distinct: an ID token's `aud` is the bare handle, its `typ` is
`JWT`, it carries none of a deployment token's claims, and every
deployment token verifier refuses it.

- Discovery at `<issuer>/.well-known/openid-configuration`. The issuer
  is `ORDO_IDENTITY_ORIGIN` plus `ORDO_IDENTITY_BASE_PATH` (11.4). With
  no base path it is the `iss` deployment tokens carry.
- Authorization code flow, PKCE `S256` required, at `/oidc/authorize`
  and `/oidc/token`. The client id is a registered handle. The redirect
  URI must be exactly a return URL (11.4). An unknown client or
  redirect is refused on the service's page and never followed.
- Public clients only (`token_endpoint_auth_methods_supported: none`). A
  code reaches only a registered return URL, and only the app that began
  the sign-in holds the verifier, so a secret adds a value to manage and
  no protection here. The app sets `ORDO_AUTH_<NAME>_CLIENT_AUTH=none`,
  added to Ordo's OIDC client for this.
- The ID token is EdDSA under the existing token keys and JWKS. Ordo's
  OIDC client already verified EdDSA, so the service gained no second key
  type.
- `sub` is the person's id, the same for every deployment. Only a live
  member (`admin` or `member`) gets a code, and the membership and the
  session are checked again at the exchange.
- Migration 6: `oidc_codes`. A code is 256 bits, hashed, 60 seconds,
  spent by its first exchange whatever the outcome, bound to client,
  redirect URI, PKCE challenge and nonce. It is too long to guess, so the
  token endpoint is not counted by the guessing limits, which an app's
  one server address would trip.
- Audit rows: `oidc.token` for each exchange, `refused` with the reason
  when it fails. No row per code (11.4).
- Not built: refresh tokens, userinfo, `prompt` and `max_age` (the ID
  token's `auth_time` lets the app judge), and sign-out everywhere.

### 11.4 Fixes from the review of 11.2 and 11.3, 2026-11-19

A read-only review of PRs 26 and 27 found 4 faults. Tests:
`tests/oidc_provider.rs`, the 5 tests under "review of PRs 26 and 27".

- **An app's pages were trusted callers.** Registering a callback
  required its origin among the client origins, and every client origin
  was in the CORS list with credentials and in the reauth's origins. A
  script on an app's page could spend the session cookie: `/me`,
  `/sign-up/join`, and under the RP ID a reauth whose answer with no
  `aud` was the home's admin token. Now a return URL may sit on any
  origin and makes it no caller; an app registers its callback alone.
  Rows made before keep their client origins. `deployment list` and the
  boot log warn about a client origin that holds an `/auth/callback/`
  return URL, and `deployment set <handle> --no-origins` (or `POST
  /deployments/<handle>/addresses`) drops it, keeping the members.
  Beside that, a reauth with no `aud` and `/cli/approve` answer only the
  service's page or `ORDO_IDENTITY_CLIENT_ORIGINS`, and `/sign-up/join`
  only the service's page.
- **Codes and audit rows grew with no bound.** Each authorization by a
  signed-in member wrote a code row and a signed audit row, and open
  sign-up lets anyone be a member. Now a session gets
  `ORDO_IDENTITY_AUTHORIZE_PER_SESSION` codes a minute (20), past which
  the app is sent `temporarily_unavailable`; the minute tick deletes
  spent and expired codes and closed guessing windows; the audit row is
  written at the exchange only. A `refused` exchange row needs a live
  code, so it is bounded by the same limit.
- **The redirect URI matched by prefix.** The OIDC flow now takes only
  an exact return URL: scheme, host, port and path, no query. The
  page's `/sign-in/return` keeps the prefix rule, which consoles use for
  their deep links.
- **The issuer left out the base path.** Two services under different
  base paths on one origin shared an issuer, so an app's peer could
  read one's subjects as the other's. The OIDC issuer is now the origin
  and the base path. Deployment tokens keep the origin as `iss`, since
  doors check it against `ORDO_IDENTITY_ORIGIN`; so under a base path
  the two differ. With no base path nothing changed.

### 11.5 Tokens between deployments, built 2026-11-26

Step G2 of `plan-federation-grants.md`: one Ordo deployment (the
consumer) calls another's door (the holder). This service says who is
calling. The holder decides what they may do (G3). Code:
`src/exchange.rs`, `ordo-identity/src/peer_token.rs`; tests:
`tests/federation_exchange.rs`; how to: `howto-identity.md` section 14,
"Let one deployment call another".

- **How a consumer proves itself.** A client assertion (RFC 7523,
  `private_key_jwt`): an EdDSA JWT with `iss` and `sub` its handle, `aud`
  the token endpoint or the issuer, a `jti`, and an `exp` at most
  `ORDO_IDENTITY_CLIENT_ASSERTION_SECS` (120) from now. Each `jti` works
  once; migration 7 keeps its tagged hash in `client_assertions` until
  the assertion expires, and the minute tick deletes it.
- **The consumer key.** Ed25519, derived from the deployment's master key
  and its handle (`peer_token::consumer_key`). No person makes, copies or
  stores a private key. Only the public half is registered here: the
  `consumerKey` of `POST /deployments`, `deployment add --consumer-key`,
  or later `POST /deployments/{handle}/consumer-key` and `deployment
  consumer-key`. The route also takes that deployment's own admin token,
  so a deployment can register its key with a token it already holds.
  Replacing the key rotates it; `null` or `--none` removes it.
- **On behalf of a person.** RFC 8693 token exchange at
  `/oidc/token`: the person's ID token this service issued to the
  consumer (`subject_token_type` `id_token`), the assertion, and
  `audience`, the holder's handle. The token: `sub` the person, `aud`
  `ordo-peer:<holder>`, `act.sub` the consumer, `client_id`, `kind`
  `person`, header `typ` `ordo-peer+jwt`.
- **As itself.** `client_credentials` with the same assertion and
  `audience`. The token: `sub` the consumer, `kind` `client`, no `act`,
  header `typ` `ordo-client+jwt`. A verifier that wants a person refuses
  it by its `typ`.
- **Lifetimes.** `ORDO_IDENTITY_PEER_TOKEN_SECS`, 300 by default, 30 to
  3600. A token on behalf of a person never outlives the ID token it was
  exchanged for.
- **Neither is another token.** `aud` `ordo-peer:<handle>` is neither a
  deployment token's `ordo-deployment:<handle>` nor an ID token's bare
  handle, and each `typ` is its own. Door, console, app and the
  service's admin routes refuse both, and an exchanged token is not a
  subject token in turn.
- **Refusals.** A consumer that did not prove itself: `invalid_client`,
  401, no audit row, so a stranger cannot grow the log. An audience
  that is not a live registered deployment, or is the consumer:
  `invalid_target`. A subject token expired, foreign, not an ID token,
  issued to another client, or for a person no longer a member of the
  consumer: `invalid_grant`.
- **Audit.** `oidc.exchange` (actor the person) and `oidc.client-token`
  (actor the consumer), `ok` or `refused` with the reason and the
  holder.
- **Not built.** The host side: deriving the key from the master key in
  `ordo-host`, registering it, and signing assertions (G4, G5). The
  holder's check (G3). A subject token lives as long as the ID token, so
  a consumer acts on behalf of a person only shortly after they signed
  in; how a consumer keeps acting later is open (owner question, G2
  report).

- An identity origin outside the console's RP ID without re-enrolment.
  WebAuthn Related Origin Requests could allow it (the SDK's `related`
  origins, as the console link uses today); not in v1.
- Zero-downtime restarts and more than 1 node of the identity service.
