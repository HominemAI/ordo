# Adding sign-in and a web page to an Ordo app

This guide is for an app developer. It picks up where `docs/quickstart.md`
ends: you have a program, a running `ordo dev` and a typed client. Now you
add passkey sign-in and a React page that shows live data.

You are the app developer and the deployment owner, one person. You write the
program, and you set the deployment's environment. Every setting named below
is yours.

The program used here is `examples/notes/notes.ordo`. It has the guards that
sign-in needs. Use it in place of the quickstart's file. The finished app is
`examples/notes/`, and every step below points at the file there.

---

## 1. What sign-in means in Ordo

Five words. Each one is a different thing.

- A **passkey** is a key pair held by the person's device. The door checks a
  signature. It never sees a secret.
- An **identity** is the account the door makes for a passkey. Its id looks
  like `usr_...`. It belongs to the whole deployment, not to one tenant.
- A **person** is one human. One person has one identity and can hold several
  passkeys.
- A **tenant** is one self-contained database of your app. The notes app gives
  each person a tenant of their own.
- A **member** is an identity's row inside one tenant, with a role. `$caller`
  in your program is that row.

Sign-in proves who the person is. Membership decides which tenant they can
enter. Guards decide which rows they see.

---

## 2. Passkey sign-in on your machine

`ordo dev` reads the same sign-in settings a server reads, when they are set.
Start it like this.

```
ORDO_AUTH_METHODS=passkey \
ORDO_APEX_DOMAIN=localhost \
ORDO_AUTH_ORIGINS=http://localhost:5173 \
ORDO_CORS_ORIGINS=http://localhost:5173 \
ORDO_AUTH_PROVISIONING=auto-onboard \
ordo dev examples/notes --out examples/notes/web/src
```

What each one does:

| Setting | Value here | What it does |
|---|---|---|
| `ORDO_AUTH_METHODS` | `passkey` | Turns sign-in on. Unset means no sign-in at all. |
| `ORDO_APEX_DOMAIN` | `localhost` | The passkey relying party id. A passkey made for one id never works for another. |
| `ORDO_AUTH_ORIGINS` | `http://localhost:5173` | The exact page origins allowed to run a passkey ceremony. |
| `ORDO_CORS_ORIGINS` | `http://localhost:5173` | The page origins allowed to call the door. `*` refuses the boot. `https://*.example.com` needs `ORDO_WILDCARD_ORIGINS=on`. |
| `ORDO_AUTH_PROVISIONING` | `auto-onboard` | The most open way in any tenant may choose. The default is `invite-only`. |

`--out` makes `ordo dev` write `interface.json` there on every save.
The page in section 3 reads it from `web/src`.

**What `localhost` means.** The relying party id is a host name. Browsers
treat `localhost` as a secure place, so passkeys work on it over plain
`http`. Open the page as `http://localhost:5173`. The address `127.0.0.1`
does not match the id `localhost`, and the ceremony fails. Another port is
another origin, so the two lists above must name the port you use.

No hardware key? In Chrome, open DevTools, then WebAuthn, then "Enable virtual
authenticator environment".

**What `ordo dev` does not do.** It serves one tenant, `dev`, and it has no
self-serve provisioning. A passkey sign-up there makes an identity and a
token, and no tenant of the person's own. Section 4 needs the tenant, so for
the whole build-along run the notes deployment instead:

```
cargo build -p ordo-cli -p ordo-host
sh examples/notes/dev/serve.sh
```

`serve.sh` boots `ordo-host` on `127.0.0.1:4000` with every setting in
section 10 set for local use, applies `notes.ordo`, and stays in the
foreground. It needs PostgreSQL 15 or later. Read the file for the variables
it sets. Its secrets are constants, so it is for your machine only.

---

## 3. A page that signs up, signs in and shows live notes

Generate the typed client, as in the quickstart (`ordo gen ts examples/notes`).
It is exported from `@hominemai/ordo-sdk`. Two packages do the work:

- `@hominemai/ordo-sdk` runs the passkey ceremony and holds the generated client. Its
  `createNote(...).commit()` is the write.
- `@hominemai/ordo-react` reads live. The SDK has no live read, so the list comes from
  `useOpQuery`, which follows the door's live feed and reads again when a
  note changes.

Both packages live under `clients/`. The notes app points an alias at them in
`examples/notes/web/vite.config.ts`. Do the same for `@hominemai/ordo-react`.

An app outside this repository installs both from GitHub Packages. They are
private, so the install needs a token.

1. Point the `@hominemai` scope at GitHub Packages. Put this line in the app's
   `.npmrc`:

   ```
   @hominemai:registry=https://npm.pkg.github.com
   ```

2. Create a personal access token (classic) with the `read:packages` scope, and
   keep it out of git. Put it in your user `~/.npmrc`, not the project's:

   ```
   //npm.pkg.github.com/:_authToken=<your token>
   ```

3. Install:

   ```
   npm install @hominemai/ordo-sdk @hominemai/ordo-react
   ```

In CI, the same two lines apply, with the workflow's token in place of a
personal one.

The page, `src/App.tsx`:

```tsx
import { useMemo, useState } from 'react';
import { OrdoClient as ReadClient, OrdoProvider, parseInterface, useOpQuery } from '@hominemai/ordo-react';
import {
  OrdoClient, createNote, listSpaces, registerPasskey, signInWithPasskey,
  type MyNotesRow, type PasskeySession,
} from '@hominemai/ordo-sdk';
import descriptor from './interface.json';
import { describe, refusalText } from './describe.js';
import { untilServed } from './untilServed.js';

const DOOR = 'http://localhost:4000';
const iface = parseInterface(descriptor);

interface Me { token: string; space: string; role: string | null }

function List() {
  const notes = useOpQuery<MyNotesRow>('myNotes', { live: true });
  return (
    <ul>
      {notes.rows.map((n) => <li key={n.id}>{n.title}</li>)}
      {notes.refusal && <li role="alert">{refusalText(notes.refusal)}</li>}
    </ul>
  );
}

function Notes({ me }: { me: Me }) {
  const [title, setTitle] = useState('');
  const [error, setError] = useState('');
  const writes = useMemo(() => new OrdoClient({ baseUrl: DOOR, space: me.space, token: me.token }), [me]);
  const reads = useMemo(() => new ReadClient({ baseUrl: DOOR, iface, space: me.space, token: me.token }), [me]);
  const add = async () => {
    try {
      await createNote(writes, { title, body: '', created: `${new Date().toISOString().slice(0, 19)}Z` }).commit();
      setTitle('');
      setError('');
    } catch (e) {
      setError(describe(e));
    }
  };
  return (
    <OrdoProvider client={reads} live>
      <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="New note" />
      <button disabled={title === ''} onClick={() => void add()}>Add</button>
      {error && <p role="alert">{error}</p>}
      <List />
    </OrdoProvider>
  );
}

export function App() {
  const [me, setMe] = useState<Me | null>(null);
  const [name, setName] = useState('');
  const [error, setError] = useState('');
  const enter = async (go: () => Promise<PasskeySession>) => {
    try {
      const done = await go();
      const space = done.raw['space'];
      if (typeof space !== 'string') throw new Error(`No tenant yet (${String(done.raw['provisioning'] ?? 'not provisioned')}).`);
      await untilServed(new OrdoClient({ baseUrl: DOOR, space, token: done.token }));
      const mine = (await listSpaces({ baseUrl: DOOR }, done.token)).find((s) => s.space === space);
      setMe({ token: done.token, space, role: mine?.role ?? null });
      setError('');
    } catch (e) {
      setError(describe(e));
    }
  };
  const signOut = async () => {
    try {
      const out = await fetch(`${DOOR}/auth/logout`, { method: 'POST', headers: { Authorization: `Bearer ${me?.token}` } });
      if (out.status !== 204) throw new Error('The deployment did not end the session. You are still signed in.');
      setMe(null);
    } catch (e) {
      setError(describe(e));
    }
  };
  return (
    <main>
      <h1>Notes</h1>
      {me ? (
        <>
          <p>Tenant <code>{me.space}</code>, role {me.role ?? 'none'}. <button onClick={() => void signOut()}>Sign out</button></p>
          <Notes me={me} />
        </>
      ) : (
        <>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Your name" />
          <button disabled={name === ''} onClick={() => void enter(() => registerPasskey({ baseUrl: DOOR }, { display: name }))}>Sign up</button>
          <button onClick={() => void enter(() => signInWithPasskey({ baseUrl: DOOR }))}>Sign in</button>
        </>
      )}
      {error && <p role="alert">{error}</p>}
    </main>
  );
}
```

What it does, top to bottom:

- **Sign up** calls `registerPasskey`. The browser asks for a new passkey.
- **Sign in** calls `signInWithPasskey`. The browser offers an existing one.
- **Who am I** is `listSpaces`, the person's own list of tenants and their
  role in each. `GET /auth/spaces` never lists anyone else's.
- **The write** is the generated `createNote`. It takes the token and the
  tenant from the client it is given.
- **The live list** is `useOpQuery('myNotes', { live: true })` inside an
  `OrdoProvider`. After the write commits, the door sends an `invalidate`,
  and the list reads again. The page never refetches by hand.
- **Sign out** is `POST /auth/logout`. The door ends the session, so the
  token is dead even if someone copied it. The page then drops it. A page
  with a device copy also calls `device.purge()` from the SDK's
  `openDevice`, and `registry.purgeAll(person)` for the spaces shared with
  them. That erases what the device held for this person, and every other
  tab of theirs empties at once.

The token lives in memory only. A token lasts an hour (`ORDO_SESSION_SECS`)
and `POST /auth/refresh` renews it for up to 12 hours. The notes app does that
renewal in `web/src/session.ts`. Copy that file when you need it.

### Staying signed in across a reload

A reload keeps the person signed in, by default (owner ruling 2026-10-21).

- Every sign-in also sets a renewal cookie. Scripts on the page cannot read it
  (`HttpOnly`). The browser sends it only to `/auth/refresh`, and only from
  the same site (`SameSite=Strict`).
- On load the page calls `resumeSession(endpoint)` from
  `@hominemai/ordo-sdk/auth/resume.js`. It answers a session, or `null` when
  there is nothing to resume. Show the sign-in screen only on `null`.
- In React, `useResumedSession(() => resumeSession({ baseUrl }))` from
  `@hominemai/ordo-react` answers `checking`, `signed-in` or `signed-out`.
  Build the `OrdoProvider`'s client from its session, so the provider starts
  signed in.
- The token itself still lives in memory. The door keeps only a hash of the
  cookie.
- Each use sets a new cookie. An old one used again ends that sign-in on every
  device, because two browsers hold it.
- Sign-out ends it. So do sign-out everywhere, passkey removal, account
  recovery, a password change and account deletion.
- A cookie unused for 30 days ends (`ORDO_REMEMBER_SECS`). `ORDO_REMEMBER=off`
  turns it off. A page may send `"remember": false` with a sign-in, for a
  shared computer.

**Cross-site pairs fall back to a touch per reload.** The cookie is sent only
when the page and the door are on the same site. `notes.example.com` and
`ordo.example.com` are one site. `notes.example.app` and `ordo.example.com`
are not, and neither are `localhost` and `127.0.0.1`. `SameSite=Strict` is
what keeps another site from using the cookie, and browsers also block
third-party cookies. So a cross-site page gets `null` from `resumeSession`
and asks for a sign-in on every reload, as before. Put the door on a
subdomain of the page's domain to keep people signed in.

The page's origin must be in `ORDO_CORS_ORIGINS`. The door then answers
`Access-Control-Allow-Credentials: true` on `/auth/` routes for that origin,
and on nothing else.

**Only a page on that list redeems the cookie** (2026-10-24). A
`POST /auth/refresh` with the cookie and no token must carry an `Origin` on
`ORDO_CORS_ORIGINS`.

- Any other origin is refused `remember_off`, with a message naming
  `ORDO_CORS_ORIGINS`. So is a request with no `Origin`; a browser always
  sends one on a POST fetch.
- The cookie is neither spent nor cleared by the refusal.
- The door's own origin is not an app origin. Script served from the door
  cannot redeem the cookie, even if a route served HTML.
- A page served through a dev proxy (Vite's `ORDO_DEV_DOOR`) still sends its
  own origin. List it, or every reload asks for a sign-in.

### How long a session lasts, in three layers

Each value is the tenant's, else the app developer's, else the deployment's.
It is resolved at sign-in and again at each refresh, from the tenant the
session is for.

| Value | Deployment default | Deployment limit |
|---|---|---|
| `tokenSecs`: one token's life | `ORDO_SESSION_SECS`, 3600 | `ORDO_SESSION_SECS_LIMIT`, 86400 |
| `maxSecs`: refresh stops after | `ORDO_SESSION_MAX_SECS`, 43200 | `ORDO_SESSION_MAX_SECS_LIMIT`, 30 days |
| `remember`: stay signed in across a reload | `ORDO_REMEMBER`, `on` | `off` is absolute |
| `rememberSecs`: renewal idle life | `ORDO_REMEMBER_SECS`, 30 days | `ORDO_REMEMBER_SECS_LIMIT`, 400 days |
| `rememberMaxSecs`: overall limit | `ORDO_REMEMBER_MAX_SECS`, none | `ORDO_REMEMBER_MAX_SECS_LIMIT`, none |

- **The app developer** sets its own defaults with
  `std.deployment.setSessionPolicy`, in the deployment's own space. It may
  also give tenants a narrower range per value, such as
  `tokenSecsRange=300-7200`. `/health/deep` reads back both layers.
- **A tenant owner** sets the tenant's own values with
  `std.space.setSessionPolicy`. They may be longer or shorter than the app
  developer's default. They must sit inside the app developer's range, or the
  deployment's limit when there is no range. A value outside is refused, and
  the refusal names the range and who sets it.
- The tenant's values also apply to sessions made through its own sign-in
  connections (`std.space.addConnection`).
- `std.space.sessionPolicy` reads what is in force and where each value came
  from.
- A token is never longer than its session. A tenant that turns `remember`
  off ends its renewals at the next reload.

**The overall limit.** With `rememberMaxSecs` set, a person signs in again
N seconds after their sign-in, however often they used it in between.

- The default is none: a device used often enough stays signed in.
- The original sign-in time travels with the cookie through every rotation.
- The cookie, the token and the session never outlive it.
- Write `none` for no limit and `default` to use the next layer's value.
- With `ORDO_REMEMBER_MAX_SECS_LIMIT` set, no layer may choose none or
  longer. A sign-in with nothing chosen ends at that limit.

### Wrong passwords are slowed (owner, 2026-10-25)

Password sign-in has an always-on slowdown per account. It only slows
guessing: it never locks the rightful person out.

- After 5 wrong passwords in a row, each further try waits 1 s, then 2 s,
  4 s, and so on, up to 60 s.
- A try inside the wait is refused unchecked: `429 too_many_attempts`, with
  `Retry-After` and `retry_after` in seconds, and `setting` naming the op
  that changes it.
- The right password, after the wait, signs in and resets the count.
- The count is per account (a hash of the identifier typed), never per
  address. An identifier nobody holds is slowed the same way, so the
  answer says nothing about which accounts exist.
- It is kept in the control database, so every node shares one count. A
  count untouched for a day starts again.
- Each burst is one `Throttled` entry in the deployment's security log.

It is layered like the session values. A sign-in that names a tenant
(`space` in the `/auth/complete` body) uses that tenant's values.

| Value | Default | Deployment setting | Deployment limit |
|---|---|---|---|
| `slowAfter`: wrong tries before the wait | 5 | `ORDO_PASSWORD_SLOW_AFTER` | `ORDO_PASSWORD_SLOW_AFTER_LIMIT`, at most 100 |
| `slowCapSecs`: the longest wait | 60 | `ORDO_PASSWORD_SLOW_CAP_SECS` | `ORDO_PASSWORD_SLOW_CAP_SECS_LIMIT`, at least 1 |

- **The app developer** sets its values with
  `std.deployment.setPasswordSlowdown`, in the deployment's own space, and
  may give tenants a range, such as `slowAfterRange=3-10`. `slowAfter=off`
  turns the slowdown off.
- **A tenant owner** with its own sign-in requirements sets the tenant's
  values with `std.space.setPasswordSlowdown`, inside that range. It may
  turn the slowdown off only where the range starts at 0
  (`slowAfterRange=0-10`), because a password account is the deployment's
  and any sign-in may name the tenant. Each change is in the tenant's
  security log (`password-slowdown-set`).
- To stop tenants changing it, close the op:
  `ORDO_ACCESS_TEMPLATE="std.space.setPasswordSlowdown="`.

The lock is a separate thing and stays off by default (owner, 2026-10-22).

### Reads need no touch

Owner, 2026-10-22: "If you are signed in = that passkey was your
credentials - you are authenticated."

- A platform op that only reads answers a signed-in person with no passkey
  touch. Examples: `std.space.sessionPolicy`, `std.space.shares`,
  `std.space.groups`, `std.space.callersOf`.
- Read or change is the op's own kind. A read is a `GET`.
- A change still asks for a fresh touch, unless `ORDO_STEP_UP_EXEMPT_OPS`
  names it.
- A read freed this way is still closed to delegated tokens. A read named in
  `ORDO_STEP_UP_EXEMPT_OPS` is open to them, as before.
- `ORDO_PRESENCE_ON_READS=on` asks for a touch on reads too, for a strict
  deployment.

The app developer decides whether tenants may set their own. Closing it is one
line in the access template, `ORDO_ACCESS_TEMPLATE="std.space.setSessionPolicy="`,
and `std.space.sessionPolicy=owner,manager,editor,viewer` opens the read to a
page that shows the values.
A tenant owner who tries the closed setter is refused with the setting and
the value that open it.

If the page says it cannot reach the deployment, check the two origin lists in
section 2. Both must name the page's exact origin.

The two helper files are in section 4 (`untilServed`) and section 7
(`describe`).

The notes page also offers the other ways in: an emailed code, outside
connections and guests. The page does not need to be told which. It asks the
door (`GET /auth/methods`, below), so nothing in its build has to match
`ORDO_AUTH_METHODS`. See `examples/notes/README.md` §2 and §6.

### Listing the ways in

`GET /auth/methods` is public and needs no token. A page calls it once at
start and builds its sign-in screen from the answer:

```
GET /auth/methods
{ "passkey": true, "password": false, "email": true, "guest": false,
  "humanCheck": { "kind": "turnstile", "siteKey": "0x4AAA..." },
  "connections": [ { "name": "discord", "label": "Discord", "kind": "oauth2" } ] }
```

It says only what a sign-in button already shows. It never carries a secret, a
client id, a secret file's path or an issuer address. `label` is the button's
words: the provider's own name for the deployment's Discord, GitHub, Google
and Microsoft connections, and the connection's name as written for any other.
A tenant's own connection is always labelled by its name as written. A tenant
that names its connection `google` shows as `google`, never as Google, so the
label never vouches for an issuer the tenant chose. `humanCheck` is `null` when the
check is off. `siteKey` is the public Turnstile key, and appears only when the
app developer set `ORDO_AUTH_HUMAN_CHECK_SITE_KEY`.

With `?tenant=<id>` the answer also lists that tenant's own active providers,
each marked `"tenant": true` (begin them with the tenant named, as in
`plan-tenant-providers.md` §5). A tenant with none, a tenant that does not exist
and a badly formed id all answer alike, so the route cannot list tenants. With
`ORDO_AUTH_TENANT_PROVIDERS` off, a tenant query adds nothing. A deployment
with no sign-in configured answers the same shape with everything off.

The answer carries `Cache-Control: public, max-age=60`, plain or with
`?tenant=`. `ORDO_AUTH_METHODS_MAX_AGE` sets the 60, from 0 to 3600. A page or
a shared cache may show a retired tenant connection for that long. It refuses
new sign-ins at once either way. With the SDK:

```ts
const methods = await signInMethods({ baseUrl: DOOR });
const forTenant = await signInMethods({ baseUrl: DOOR }, { tenant: 'spc_acme' });
```

---

## 4. What happens on the first sign-up

With `ORDO_PROVISION_SELF_SERVE=on-signup`, the door gives each new person a
tenant of their own, as its owner. The sign-up does not wait for it (owner,
2026-10-21). In order:

1. `POST /auth/register` starts the ceremony. The browser makes the passkey.
2. `POST /auth/complete` finishes it. The door makes the identity and the
   token, and writes the person's tenant into the registry.
3. The answer comes back at once: the token, `space: null` and
   `provisioning: "in progress"`.
4. The door makes the tenant on a job of its own: a database, your program
   applied, the person as owner. That takes about a second, and longer while
   many people sign up at once.
5. `GET /auth/provisioning`, with the person's token, says where it stands:
   `provisioning` with a `step`, then `ready` with the `space`.

A person who already has a tenant gets `space` in the answer and
`provisioning: null`. `provisioning` can also say `failed` (the registry could
not be read) or `limit reached` (the hour's sign-ups are used). The sign-in
still worked in every case.

The SDK waits for you. `waitForTenant` reads the status until the tenant is
ready, tells `onProgress` each step, and gives the space id. The notes page
shows "Setting up your account" until then:

```ts
import { TenantError, waitForTenant } from '@hominemai/ordo-sdk';

const done = await registerPasskey(endpoint, { display });
const space =
  typeof done.raw['space'] === 'string'
    ? done.raw['space']
    : await waitForTenant(endpoint, done.token, { onProgress: (s) => show(s.step) });
```

It rejects with a `TenantError`. `failed` carries a `reason` written for the
person; the registry's own note, which can name hosts, is only in the log.
`retryTenant` (`POST /auth/provisioning`) starts a new tenant after a failure,
and so does the person's next sign-in. A slow door is a `timeout`, never
`unavailable`. In both cases the sign-in call throws an `OrdoNetworkError`
(section 7).

**One person, one tenant.** The registry row is written before the answer,
and a unique index lets one row in per person. Two sign-ups at the same moment
get the same tenant.

**A restart in the middle.** A tenant left part-made is made again when the
deployment starts (`provisioning resumed for N tenant(s)` in the log). Every
step can run twice for one tenant.

### While the tenant is being made (owner, 2026-10-21)

**Nothing can be written to it yet.** A call into the tenant before it is
ready is refused `503 tenant_not_ready` with a `Retry-After`. Nothing ran.
Only the person it is for hears this; anyone else gets `access_denied`. Both
clients send the call again on their own, with the `busyRetry` options and
the same `Idempotency-Key`. The wait is `ORDO_PROVISION_NOT_READY_RETRY_SECS`
(default 2).

**It never stops trying.** Every running server sweeps for setups that
stalled, every `ORDO_PROVISION_SWEEP_SECS` (default 30). A lease on the
registry row lets one server run each setup; it lasts
`ORDO_PROVISION_LEASE_SECS` (default 120) and is renewed while the setup
runs. A try that fails waits `ORDO_PROVISION_RETRY_BASE_SECS` (default 15),
doubled after each failure, never more than `ORDO_PROVISION_RETRY_CAP_SECS`
(default 900). Nothing is marked `failed`, and nothing is deleted. The
account stays whatever happens.

**Stuck is reported, from both sides.** After `ORDO_PROVISION_STUCK_SECS`
(default 300) a setup is stuck. It is still tried. It shows up in four places:

| Where | What it says | Who reads it |
|---|---|---|
| `GET /auth/provisioning` | `state: "provisioning"`, `stuck: true`, `attempts`, a `reason` for the person and `setting: "ORDO_PROVISION_STUCK_SECS"` | the person, through your page |
| The security-event stream | one `own-tenant` act with `result=stuck`, once per tenant | the app developer and the deployment owner |
| `/health/deep` | `tenant_setup`: how many are in progress, and each stuck one by id, age, tries and step. Never the person. Any stuck one turns `status` to `degraded` | the deployment owner |
| `/metrics` | the gauges `ordo_tenant_setup_in_progress` and `ordo_tenant_setup_stuck` | the deployment owner's alerts |

The log names each stuck tenant with its last error. The person's status
never shows that error, because it can name hosts.

**What the page shows is yours to choose.** In React, `TenantGate` shows your
own content for each phase and renders the app once the tenant is ready:

```tsx
import { TenantGate } from '@hominemai/ordo-react';

<TenantGate
  baseUrl={DOOR}
  token={session.token}
  settingUp={<Spinner label="Setting up your account" />}
  stuck={(t) => <Notice>{t.status?.reason}</Notice>}
  failed={(t) => <Retry onClick={t.retry} />}
>
  {(space) => <OrdoProvider client={clientFor(space)}><App /></OrdoProvider>}
</TenantGate>
```

A slot left unset gets a plain, unstyled default: a polite `role="status"`
line while setting up or stuck, and a `role="alert"` with a "Try again"
button when it cannot be made. `useTenantStatus` gives the same `phase`
(`checking`, `setting-up`, `stuck`, `ready`, `failed`), `status`, `space` and
`retry()` without rendering anything.

Without React, `watchTenant` is the same hook. It calls your `onStatus` on
each change and has no time limit, unlike `waitForTenant`'s two minutes:

```ts
import { watchTenant } from '@hominemai/ordo-sdk';

const watch = watchTenant(endpoint, done.token, (s) => {
  if (s.stuck) show(s.reason);
  else if (s.state === 'provisioning') show('Setting up your account');
});
const space = await watch.ready;
```

Apply the program before anyone signs up. Someone who signs up first gets no
tenant, and the page says so. Apply, then sign in again.

**The moment before the tenant is served.** The door that made the tenant
serves it before it answers. With more than one replica, another one learns of
the tenant a little later and can answer `access_denied` for a short while.
The page waits for that instead of showing an error:

```ts
import { OrdoRefusal, myNotes, type OrdoClient } from '@hominemai/ordo-sdk';

/** After a sign-up, wait until the deployment serves the new tenant (about 6 s at most). */
export async function untilServed(client: OrdoClient, attempts = 40, everyMs = 150): Promise<void> {
  for (let i = 1; ; i++) {
    try {
      await myNotes(client).fetch();
      return;
    } catch (e) {
      if (!(e instanceof OrdoRefusal) || e.class !== 'access_denied' || i >= attempts) throw e;
      await new Promise((r) => setTimeout(r, everyMs));
    }
  }
}
```

It tries the read up to 40 times, 150 ms apart, then believes the refusal.
`App.tsx` calls it before showing anything.

The deployment-wide sign-up ceiling is `ORDO_PROVISION_SIGNUPS_PER_HOUR`
(default 60, or `unlimited`). A sign-up over it gets `429`, which the SDK
reports as a refused sign-in. You can change it later with the platform op
`std.deployment.setSelfServe`.

---

## 5. Guards: only the author sees their notes

Open `examples/notes/notes.ordo`. Three lines carry the whole rule.

```
entity notes {
  title: text
  body:  text = ""
  created: datetime
  acl (read, write, admin)
}
```

`acl (read, write, admin)` derives a link entity, `notesAccess`, and guards on
`notes`. A row is readable only by a member who holds a `notesAccess` row for
it. `createNote` writes the creator's `admin` row in the same command group as
the note. Nothing else grants access.

`myNotes` has no filter and no permission check:

```
op myNotes unbounded =
  notes[id, rev, title, body, created]
  sort -created
```

It needs none. The compiler inlines the read guard, so rows the caller cannot
read are not in the answer. A write is guarded the same way.

`$caller` is the other half. It is the one reserved parameter. The runtime
binds it to the caller's own `members` row, and a client that tries to send it
is refused (`caller_supplied`). Use it in a body when you want to narrow
further, as in `tasks(assignee == $caller)`. `docs/tutorial.md` §8 to §10 walk
through guards and `$caller` with an example that has more than one member.

In the notes app each person is the only member of their own tenant, so the
tenant boundary already keeps people apart. The guards keep the rule true when
a second person enters the tenant, or when you share one note with a guest.

What a refused write looks like:

| Class | Meaning |
|---|---|
| `access_guard` | The caller can read the row and may not write it. |
| `not_found` | The row is outside the caller's read view. It looks the same as a row that never existed, on purpose. |
| `access_denied` | The door refuses the caller before the op runs. It gives one answer for every reason. |

---

## 6. A second person, and share links

There are three ways another person reaches your tenant.

- **A member.** A tenant owner or manager calls `std.space.invite` with an
  optional `role`, `expires`, `to` and `label`. The answer holds a `ticket`
  once. Your app delivers it, since Ordo sends nothing. The invitee opens your
  page with it. `peekInvitation` shows the tenant, the role and the expiry
  without spending the ticket, and the sign-up call carries the ticket in.
  Without a role, the tenant's default applies (viewer).

  ```ts
  import { peekInvitation, registerPasskey } from '@hominemai/ordo-sdk';

  const DOOR = 'http://localhost:4000';

  /** An invited person opens your page with a ticket. */
  export async function join(ticket: string, name: string) {
    const offer = await peekInvitation({ baseUrl: DOOR }, ticket); // the tenant, the role, the expiry
    const session = await registerPasskey({ baseUrl: DOOR }, { display: name, space: offer.space, invitation: ticket });
    return { offer, session };
  }
  ```

  An invitation lets a newcomer sign up even under the default
  `ORDO_AUTH_PROVISIONING=invite-only`. It works with every method. Pass
  `space` and `invitation` to the call that signs the person in:

  | Method | Where the invitation goes |
  |---|---|
  | Passkey | `registerPasskey(endpoint, { display, space, invitation })` |
  | Password | `signInWithPassword(endpoint, credentials, { space, invitation })` |
  | Emailed code | Both `beginEmailSignIn(endpoint, { address, space, invitation })` and `completeEmailSignIn(endpoint, sent, code, { space, invitation })`. Under invite-only an unknown address is mailed only when `begin` carries a valid invitation. |
  | Discord or GitHub | Add `space` and `invitation` to the `/auth/complete` body that redeems the handoff (section 8). |

  - The invitation must be unspent, unexpired, for `space`, and name nobody.
  - It is spent once. A second person who tries it gets the one refusal.
  - Without a valid one, invite-only refuses with the one answer and stores nothing.
  - The sign-up ceiling and the human check apply as to any sign-up.
- **A guest** holds a seat with no role and reads only what a share names.
  The sharer calls `std.space.share`, and the guest accepts the ticket.
- **A share link** lets a reader with no account open one thing, read-only.

Guests and links are in `docs/howto-sharing.md`, with the calls, the ticket
lifetime and how to take access back. The notes app does all three in
`web/src/components/SharePanel.tsx`, `AcceptView.tsx` and `LinkView.tsx`.

---

## 7. Errors a page must handle

A sign-in page meets three families. A sign-in call throws `PasskeyError`. An
op call returns a refusal. A call that got no answer throws
`OrdoNetworkError`, from any call.

**No answer.** `OrdoNetworkError.kind` says why, so the page does not guess
from a `TypeError`:

| `kind` | Meaning | What fixes it |
|---|---|---|
| `blocked` | The server is up, and the browser refused the call for the page's origin. | The deployment owner sets `ORDO_CORS_ORIGINS` and, for a sign-in call, `ORDO_AUTH_ORIGINS` to the page's origin (section 2). The message names them. |
| `unreachable` | The server could not be reached. | Start it, or fix the address. |
| `timeout` | The server did not answer within `callTimeoutMs`. | Try again, or raise `callTimeoutMs` on the endpoint. |

The SDK tells `blocked` from `unreachable` with one `no-cors` request to
`/health/live`: an opaque answer means the server is up. The answer is kept
for 5 seconds. For the options, see the SDK README.

**Sign-in calls.** `PasskeyError.failure` is one of these:

| `failure` | Meaning | What the page says |
|---|---|---|
| `cancelled` | The person closed the prompt, or it timed out. | Try again when ready. |
| `unsupported` | This browser has no passkeys. | Use a current browser. |
| `unavailable` | The door answered 503: a store could not be read. | Try again in a moment. |
| `refused` | The door gives one answer for every refusal. | Sign up first, or try again. |
| `too_many_attempts` | Too many wrong passwords in a row on this account. `retryAfter` is the wait in seconds. | Try again after `retryAfter` seconds. |
| `reauthentication_required` | A signed-in change needs a fresh first factor. | Prove it again, then retry. |

An outside sign-in (section 8) fails on the return address instead. It carries `?error=authentication_failed`, or `?error=identity_provider_unavailable` when the provider could not be reached or answered a 5xx, 408, 425 or 429 (down or rate limiting). Say "try again later" for the second. Only a final answer from the provider (another 4xx) is `authentication_failed`.

**Op calls.** A refusal has a `class`, a `message`, a `status` and the op's
name. You read it from three places:

| Where | What you get |
|---|---|
| `@hominemai/ordo-sdk` call | A thrown `OrdoRefusal`. Read `e.class` and `e.detail`. |
| `@hominemai/ordo-react` hook | `refusal` on the hook's result, and `state.status === 'refused'` on `useCommit`. |
| `@hominemai/ordo-react` client called by hand | A thrown `OrdoRefusal`. Read `e.class` and `e.refusal`. |

The classes a sign-in page meets:

| Class | Where it comes from | What to do |
|---|---|---|
| `presence_required` | `403` from the account routes (recovery codes, account delete) when the token has no fresh passkey touch. | Call `stepUp`, then repeat the call. |
| `space_paused` | `423`. Someone paused the tenant on purpose. The data is kept and nothing ran. | Show a paused message. Do not retry: waiting does not lift a pause. |
| `human_check_failed` | `403`. A public write came without a human-check token the provider confirmed, or the provider could not be asked. | Run the widget again and send the new token in `X-Ordo-Human`. A token is good for one write. |
| `server_busy` | `503` with `Retry-After`. Every database connection is in use and nothing ran. | The client already retried. Show a busy message and let the person try again. |
| `tenant_not_ready` | `503` with `Retry-After`. The person's own tenant is still being set up (section 4) and nothing ran. | The client already retried. Show the setting-up screen; `TenantGate` does this for you. |
| `serialization_conflict` | `409`. The write lost a race with other writes to the same rows, through the server's retries (`ORDO_WRITE_RETRIES`). Nothing was applied. | The client already retried. Show a busy message and let the person try again. |
| `access_denied` | The door refuses the caller. One answer for every reason. | Right after sign-up, wait (section 4). On a platform op, ask for a touch and retry once. |

Three details you need.

**`presence_required` is folded.** The SDK's passkey helpers turn any refusal
of an account route into `PasskeyError` with `failure: 'refused'`. Ask for the
touch before the call. Platform ops (`std.space.share` and the like) ask for a
touch no older than five minutes, and the door answers `access_denied` when it
is stale. So retry once after a step-up:

```ts
import { OrdoRefusal, stepUp } from '@hominemai/ordo-sdk';

const DOOR = 'http://localhost:4000';

/** Run `act`. If the door refuses it, ask for a passkey touch and run it once more. */
export async function withPresence<T>(
  token: string,
  act: (token: string) => Promise<T>,
  onFresh: (token: string) => void,
): Promise<T> {
  try {
    return await act(token);
  } catch (e) {
    if (!(e instanceof OrdoRefusal) || e.class !== 'access_denied') throw e;
    const fresh = await stepUp({ baseUrl: DOOR }, token); // one passkey touch
    onFresh(fresh.token);
    return act(fresh.token);
  }
}

```

**Both clients name every refusal class the door sends.** A test on the
server side (`crates/ordo-api/tests/client_classes.rs`) fails if either list
falls behind, so a class never arrives as `internal` by mistake. A paused
tenant is `space_paused`, whatever status the deployment owner set for it.

**`server_busy` is retried for you.** Both clients send the call again,
waiting `min(maxDelayMs, firstDelayMs * factor^n)` and never less than
`Retry-After`, plus up to 20% jitter. The defaults are 2 retries, 500 ms, a
factor of 2 and 10 s. A write carries one idempotency key for every attempt,
so a retry cannot write twice. When the retries run out the refusal has class
`server_busy`. Change the numbers per client, and `retries: 0` turns it off.
A write that lost a race (`serialization_conflict`) is retried the same way
under `conflictRetry`: 3 retries from 50 ms, a factor of 2, at most 1 s, each
wait a random time from half to all of it, with the same idempotency key:

```ts
import { OrdoClient as ReadClient, parseInterface } from '@hominemai/ordo-react';
import { OrdoClient } from '@hominemai/ordo-sdk';
import descriptor from './interface.json';

const DOOR = 'http://localhost:4000';
const busyRetry = { retries: 4, firstDelayMs: 250 }; // factor 2, maxDelayMs 10000, jitter on

export const writes = new OrdoClient({ baseUrl: DOOR, space: 'spc_x', token: 'tok', busyRetry });
export const reads = new ReadClient({
  baseUrl: DOOR, iface: parseInterface(descriptor), space: 'spc_x', token: 'tok', busyRetry,
});

```

The words the page shows, `src/describe.ts`:

```ts
import { OrdoNetworkError, OrdoRefusal, PasskeyError } from '@hominemai/ordo-sdk';

/** What to tell a person about a refusal. Works for the SDK's and for a hook's. */
export function refusalText(r: { class: string; status: number; message: string }): string {
  if (r.class === 'server_busy') return 'The service is busy. Try again in a moment.';
  if (r.class === 'space_paused') {
    return 'This tenant is paused. Your data is kept. It works again when it is resumed.';
  }
  if (r.class === 'access_denied') return 'Not allowed. If you only just signed in, wait a moment and retry.';
  return r.message;
}

/** What to tell a person about anything a sign-in or a call throws. */
export function describe(e: unknown): string {
  if (e instanceof PasskeyError) {
    switch (e.failure) {
      case 'cancelled':
        return 'The passkey prompt was closed. Try again when you are ready.';
      case 'unsupported':
        return 'This browser cannot use passkeys.';
      case 'unavailable':
        return 'The deployment cannot check passkeys right now. Try again in a moment.';
      case 'refused':
        return 'That passkey was not accepted. With no account yet, sign up first.';
      default:
        return e.message;
    }
  }
  if (e instanceof OrdoRefusal) return refusalText(e.detail);
  // `blocked`, `unreachable` and `timeout`. The message names the fix.
  if (e instanceof OrdoNetworkError) return e.kind === 'timeout' ? 'The service took too long to answer. Try again in a moment.' : e.message;
  return e instanceof Error ? e.message : 'Something went wrong.';
}
```

`App.tsx` uses `describe` for anything thrown and `refusalText` for the
hook's `refusal`. A refusal that is not in the table falls through to its own
message.

---

## 8. Sign in with Discord or GitHub

A deployment can also let people sign in through an outside provider, over
OAuth2. A provider is a named connection. The name is what the page sends as
`method`. It is 1 to 32 of `a-z`, `0-9` and `-`. Its settings start with
`ORDO_AUTH_<NAME>_`, with the name in capitals and `-` as `_`.

```
ORDO_AUTH_METHODS=passkey,discord
ORDO_PUBLIC_URL=https://api.example.com
ORDO_AUTH_RETURN_URLS=https://app.example.com/signed-in
ORDO_AUTH_DISCORD_PRESET=discord
ORDO_AUTH_DISCORD_CLIENT_ID=1100220033
ORDO_AUTH_DISCORD_CLIENT_SECRET_FILE=/run/secrets/discord
```

Register `https://api.example.com/auth/callback/discord` as the redirect URI in
the Discord developer portal. In general it is
`<ORDO_PUBLIC_URL>/auth/callback/<name>`.

| Setting | Meaning |
|---|---|
| `_PRESET` | `discord` or `github`. Fills the endpoints, scope and claims. |
| `_KIND` | `oauth2`, for a provider with no preset. Then the endpoints and subject claim are required. |
| `_CLIENT_ID` | Required. |
| `_CLIENT_SECRET_FILE` | Required. A path to a file holding the secret. A secret in a variable's value refuses the boot. |
| `_AUTHORIZE_URL`, `_TOKEN_URL`, `_USERINFO_URL`, `_SCOPE`, `_SUBJECT_CLAIM`, `_EMAIL_CLAIM`, `_EMAIL_VERIFIED_CLAIM`, `_DISPLAY_CLAIMS`, `_AVATAR_CLAIM` | Override the preset, or supply what a custom provider needs. URLs are `https`. |
| `_MUTABLE_SUBJECT=acknowledged` | Required to use a subject claim that can change (`email`, `login`, `username`, `name`, `preferred_username`, `global_name`). |
| `ORDO_PUBLIC_URL` | Required with any connection. `https`. |
| `ORDO_AUTH_RETURN_URLS` | Required with any connection. Comma-separated `https` addresses (or `http://localhost`, `http://127.0.0.1`), no fragment. Matched by origin: any path on a listed origin is accepted. |

A connection keeps its endpoints and subject claim for life under its name. A
later boot with different ones refuses. A new provider gets a new name.
`ORDO_EGRESS_DENY_CIDRS` also applies to the provider's endpoints.

**The page runs three steps.**

1. `POST /auth/begin` with `{"method":"discord","return_to":"https://app.example.com/signed-in"}`
   answers `{"redirect": "<provider URL>", "binder": "<secret>"}`. Keep the
   binder in memory or `sessionStorage`, never in a URL. Navigate to `redirect`.
2. The provider returns the browser to Ordo. Ordo exchanges the code and reads
   the profile. It answers `303` to `return_to?handoff=<code>`. On failure the
   address carries `?error=authentication_failed`, or
   `?error=identity_provider_unavailable` when the provider could not be reached.
3. `POST /auth/complete` with
   `{"method":"discord","kind":"handoff","handoff":"<code>","binder":"<binder>"}`,
   plus `space` to name a tenant as with any method, and `invitation` for an
   invited newcomer (section 6). The handoff lives 60
   seconds and is good once. Without the binder it is refused. The answer is
   the usual sign-in answer, with `via: "oauth2:discord"`.

**What the app sees.**

- A session token whose `via` claim is `oauth2:discord`.
- A principal minted at the first sign-in, named with the display name the
  provider vouched for. A `display` the page posts is ignored.
- No presence. A Discord session cannot pass a step-up gate by itself under the
  default presence setting. Self-serve provisioning does not
  depend on which sign-in method was used, so a Discord sign-in gets its own
  tenant like any other.
- A first sign-in counts against `ORDO_PROVISION_SIGNUPS_PER_HOUR`. Over it,
  `/auth/complete` answers `429 quota_exceeded`. A returning person is not counted.
- A tenant admits these people with the provider way of admission, naming the
  provider `oauth2:discord` (or `oauth2:discord@example.com=role` for a
  verified address in that domain).
- A deleted account stays refused. Its Discord id still names it.
- **GitHub carries no email.** The GitHub preset sets no email claim, so a
  GitHub sign-in has no address. An `@domain` rule such as
  `oauth2:github@example.com=role` never matches it. Admit GitHub people by
  provider name (`oauth2:github`) or by invitation. Discord does send a
  verified address, so domain admission works there.

`GET /auth/identities` with the session's bearer token answers the caller's own
linked identities, and only theirs:

```
[{"connection":"discord","subject":"80351110224678912","display":"Nelly",
  "avatar":"https://cdn.discordapp.com/avatars/80351110224678912/8342729096ea3675442027381ff50dfe.png",
  "email":"nelly@example.com","linkedAt":1800000000}]
```

`email` is there only when Discord marked it verified. The list is refreshed at
each sign-in. Ordo keeps no Discord access token, so a later call to Discord on
the person's behalf is the app's own business.

**Security note.** OAuth2 has no signed assertion. The profile is trusted
because it came over TLS from the configured host, and for no other reason.
Prefer OIDC wherever a provider offers it (section 14).

**A provider behind a private CA.** The door checks a provider's certificate
against the public root program. `ORDO_AUTH_PROVIDER_CA_FILE` names a PEM file
of CA certificates to trust as well, for every OAuth2 and OIDC connection, the
deployment's and each tenant's. Validation stays on. A file that cannot be read
or holds no certificate refuses the boot by name. Unset, the public program
alone. A provider inside the deployment's own network also needs its host in
`ORDO_EGRESS_PRIVATE_HOSTS` (section 9). The notes end-to-end stack uses both
to reach its stand-in Discord on loopback (`docs/howto-e2e.md`).

---

### Turning on the human check

Cloudflare Turnstile can sit in front of every sign-in `begin` and in front of
passkey sign-up. It is off by default. Turn it on to slow a script that starts sign-ins in bulk.

```
ORDO_AUTH_HUMAN_CHECK=turnstile
ORDO_AUTH_HUMAN_CHECK_SECRET_FILE=/run/secrets/turnstile
```

The file holds the secret key from your Turnstile widget. A secret in a
variable's value is refused. Also set the widget's public site key, so pages
can draw the widget without a build variable of their own:

```
ORDO_AUTH_HUMAN_CHECK_SITE_KEY=0x4AAA...
```

`GET /auth/methods` reports it. It is public by design, so it is a plain value
and not a file. The page shows the Turnstile widget, and sends the
token it gives as `human` in the `begin` body:

```
POST /auth/begin   {"method": "discord", "return_to": "...", "human": "<token>"}
```

With the SDK, pass `{ human: token }` to `signInWithPasskey` and to
`registerPasskey`.

**Routes it covers.** `POST /auth/begin` for every method (email, OAuth2,
passkey sign-in, a first guest) and `POST /auth/register`, the first leg of
passkey sign-up, which also carries a ticket sign-up. The second leg,
`/auth/complete`, is not checked: it needs the ceremony the first leg minted.

**Routes it leaves alone.** `/auth/recover/begin`, `/auth/password/redeem`
and the other ticket-gated routes need a secret ticket that an administrator
issued, so a script cannot start them in bulk. Signed-in routes need a session.

A missing token, a token Cloudflare rejects and a Cloudflare that cannot be
reached all get the same answer as any failed sign-in, and nothing else is sent
out. The reason is in the security stream. Ordo sends Cloudflare the secret and
the token. It sends no visitor address. Each token is checked once, so the page
asks the widget for a fresh token on each try.

---

## 9. Sending mail

The mail channel is how a deployment sends email. Emailed sign-in codes use it (section 10). Password reset will use it in a later step.

It is off by default. Set `ORDO_MAIL_KIND` to turn it on. There are three kinds.

SMTP:

```
ORDO_MAIL_KIND=smtp
ORDO_MAIL_FROM=noreply@example.com
ORDO_MAIL_HOST=smtp.example.com
ORDO_MAIL_CREDENTIALS_FILE=/run/secrets/mail
```

Resend:

```
ORDO_MAIL_KIND=resend
ORDO_MAIL_FROM=noreply@example.com
ORDO_MAIL_KEY_FILE=/run/secrets/mail-key
```

SMTP on port 587 with STARTTLS:

```
ORDO_MAIL_KIND=smtp
ORDO_MAIL_FROM=noreply@example.com
ORDO_MAIL_HOST=smtp.example.com
ORDO_MAIL_PORT=587
ORDO_MAIL_TLS=starttls
ORDO_MAIL_CREDENTIALS_FILE=/run/secrets/mail
```

Mailpit on your own machine (plaintext, local only):

```
ORDO_MAIL_KIND=smtp
ORDO_MAIL_FROM=noreply@example.com
ORDO_MAIL_HOST=localhost
ORDO_MAIL_PORT=1025
ORDO_MAIL_TLS=none
ORDO_EGRESS_PRIVATE_HOSTS=localhost
```

Plain `mailpit` works with this. There is no credentials file, so Ordo sends no `AUTH`. If a relay answers that it wants a login, the refusal names `ORDO_MAIL_CREDENTIALS_FILE`.

SendGrid:

```
ORDO_MAIL_KIND=sendgrid
ORDO_MAIL_FROM=noreply@example.com
ORDO_MAIL_KEY_FILE=/run/secrets/mail-key
```

- `ORDO_MAIL_FROM` is the sender address, bare, with no display name.
- Secrets come from files only. The credentials file holds the user on line 1 and the password on line 2. The key file holds the API key.
- `ORDO_MAIL_PASSWORD` and `ORDO_MAIL_KEY` as values refuse the boot.
- `ORDO_MAIL_PORT` takes any port from 1 to 65535. The default is 465.
- `ORDO_MAIL_TLS` is `implicit` (the default, port 465), `starttls` (port 587) or `none` (no encryption). Any other value refuses the boot and lists the three. It does not apply to Resend or SendGrid.
- `starttls` is required, never opportunistic. If the relay does not offer it, or anything odd happens around the upgrade, the send is refused. Nothing falls back to plaintext.
- `none` is plaintext SMTP. It is used only when you name it and is never a fallback. The startup line says NO TLS. The relay password and every message cross the network readable, so use it for a local relay or a mock only.
- With `implicit` and `starttls`, certificates are always checked.
- A relay or API inside the deployment's own network is refused by the egress deny list. `ORDO_EGRESS_PRIVATE_HOSTS` lifts that for the hosts you name.
- `ORDO_EGRESS_PRIVATE_HOSTS` is a comma list of exact host names, exact IP addresses or CIDR ranges (`10.0.0.0/8`, `fd12::/16`). It is empty by default and names are case-insensitive. A listed host, or an address in a listed range, may reach loopback, a private range, unique-local or CGNAT. It never opens link-local, the cloud metadata addresses (169.254.169.254, fd00:ec2::254), multicast or reserved addresses, even inside a listed range. Wildcards, ports and schemes refuse the boot, and so does a CIDR with host bits set. It is checked at every boot, mail on or off, and mail, OIDC, OAuth2 and Turnstile all read it.
- When a send is refused and a setting can fix it, the worker's log line names the setting and value. For example: no STARTTLS offered, so set `ORDO_MAIL_TLS=none` or use port 465 with `implicit`. A 535 says to check `ORDO_MAIL_CREDENTIALS_FILE`. A private address says to set `ORDO_EGRESS_PRIVATE_HOSTS=<host>`.

A send is a row in the `$mailoutbox` outbox. The `mail.send` worker drains it. So a send survives a restart and never slows a request.

- A failed send is retried with a growing delay, up to `ORDO_MAIL_MAX_ATTEMPTS` (default 5).
- What is retried: a connection or TLS failure, a timeout, an SMTP 4xx, and an API 5xx, 408, 425 or 429. A Resend or SendGrid rate limit (429) keeps the message queued.
- When the API names a wait (`Retry-After`, in seconds), the next try waits at least that long, up to one day.
- What drops the message at once: an SMTP 5xx, any other API 4xx, a redirect, or another 2xx than the accepted one. The worker's log line names the reason, and `ordo_mail_dropped_total` counts it.
- `ORDO_MAIL_RETRY_BASE_SECS` (default 15) is the wait before the second attempt. It doubles each time, up to `ORDO_MAIL_RETRY_MAX_SECS` (default 900).
- `ORDO_MAIL_BATCH` (default 16) is how many rows the worker claims per pass.
- `ORDO_MAIL_LEASE_SECS` (default 120) is how long a claimed row is hidden from other replicas. Rows are claimed one at a time and each is leased when its own send starts, so a short lease cannot let another replica send the tail of a batch twice. The floor of 30 is above the 20 s send timeout.
- Delivery is at least once. A crash after the relay accepted a message can send it twice. Ordo resends and accepts a rare duplicate (owner, 2026-10-22).
- Every try of one email carries the same `Message-ID`, `<ordo-ROWID-QUEUEDAT@domain>`. The domain is the sender address's domain. A mail client can fold the copies into one.
- `ORDO_MAIL_SEND_SECS` sets how often the worker reads the outbox (default 2).

An app may run its own mail instead. Neither depends on the other (owner ruling, 2026-10-08).

### What does not work yet

Check these before you pick a mail provider.

| You want | Today | What to do instead |
|---|---|---|
| The Mailgun, Postmark, Brevo, Mailjet or SES API | Refused at boot | Use the provider's SMTP where it has one, or Resend or SendGrid. |

To try mail on your own machine, run Mailpit as shown above. A free Resend or SendGrid account also works. Sends go to real inboxes, so send to an address you own.

The mail API gap is on the roadmap (board: `mail-relay-gaps`).

At startup the log prints one of two things:

```
  mail        <label>
  mail        none (ORDO_MAIL_KIND is not set)
```

With mail off, the census also lists `mail.send` as DISABLED.

---

## 10. Signing in with an emailed code

A person types an address, gets a short code by email and types it back. No password and no passkey.

### Turn it on

Add `email` to the methods. The mail channel from section 9 must be on, or the boot is refused.

```
ORDO_AUTH_METHODS=passkey,email
ORDO_MAIL_KIND=resend
ORDO_MAIL_FROM=noreply@example.com
ORDO_MAIL_KEY_FILE=/run/secrets/mail-key
```

The defaults work as they are. Every setting is in `docs/settings.md` section 1.

### The two requests

First the page asks for a code.

```
POST /auth/begin
{ "method": "email", "identifier": "nelly@example.com",
  "human": "<turnstile token, if the check is on>",
  "device": "<marker from the last sign-in, if the page has one>" }
```

The answer is the same for every address:

```
{ "challenge": "chal_...", "binder": "...", "method": "email-otp" }
```

- The page keeps `challenge` and `binder` in memory until the person types the code.
- The answer never says whether an address is known or whether a message was sent.

Then the page sends the code.

```
POST /auth/complete
{ "method": "email", "kind": "email_code",
  "challenge": "chal_...", "binder": "...", "code": "48201937",
  "space": "optional tenant" }
```

The answer is the usual sign-in answer. `via` is `email-otp`. It also carries `device`.

### The device marker

- `device` is a marker for this browser.
- The page keeps it and sends it as `device` on its next email `begin`.
- A marker is honoured for `ORDO_AUTH_EMAIL_DEVICE_DAYS` (30) days.
- A marker gives its holder a separate allowance of messages (see the limits below).

### Who is mailed

| Address | Mailed? |
|---|---|
| Linked to an account that is not deleted | Yes |
| Unknown, with `ORDO_AUTH_PROVISIONING=auto-onboard` | Yes. The first sign-in makes the account. |
| Unknown, with `invite-only`, and `begin` carries `space` and a valid `invitation` | Yes. The first sign-in makes the account and admits it at the invitation's role. |
| Unknown, with `invite-only`, and no valid invitation | No. The security stream says `email-not-admitted`. |
| Linked to a deleted account | Never. The stream says `identity-deleted`. |

A first sign-in counts against `ORDO_PROVISION_SIGNUPS_PER_HOUR`. It runs the self-serve hook, as a first passkey or Discord sign-in does (section 4).

### Limits

All windows are fixed clock hours and days. No limit reads a client's network address.

| Limit | Setting | Default |
|---|---|---|
| Messages to one address, an hour | `ORDO_AUTH_EMAIL_PER_HOUR` | 5 |
| Messages to one address, a day | `ORDO_AUTH_EMAIL_PER_DAY` | 15 |
| Messages an hour for a returning device | `ORDO_AUTH_EMAIL_DEVICE_PER_HOUR` | 3 (0 is off) |
| Messages an hour for the deployment | `ORDO_AUTH_EMAIL_DEPLOYMENT_PER_HOUR` | 1000 |
| Digits in a code | `ORDO_AUTH_EMAIL_CODE_DIGITS` | 8 |
| Life of a code, from `begin` | `ORDO_AUTH_EMAIL_CODE_SECS` | 600 |
| Tries at one code | `ORDO_AUTH_EMAIL_CODE_ATTEMPTS` | 5 |

- A limit that fires is invisible to the caller. `begin` answers as usual and nothing is sent.
- The security stream records `email-limit-recipient`, `email-limit-device` or `email-limit-deployment`, on op `auth.email.send`.
- The last wrong code ends the ceremony. The person asks for a new code.

### What the person sees

- They type an address and press send.
- The page says a code is on its way, whether or not one is.
- A message arrives with the subject "Your sign-in code".
- They type the code. The page signs them in.
- A wrong, late or reused code gets one refusal. The page offers a new code.

### How it stays safe

- A code works once.
- A code works only with the binder of the browser that asked.
- A new code ends older ceremonies for the same address.
- A late message for an older `begin` is not sent.
- Delivery is at least once. A duplicate mints a new code that replaces the old one and keeps the attempt count.
- The code is minted by the `mail.send` worker and stored only as an HMAC.
- The address in the ceremony row is sealed under a key derived from the challenge.
- Email sign-in carries no presence. An action that needs a fresh passkey touch still asks for one.
- The Turnstile check (section 8) runs first in `begin`, when it is on.

The state lives in `$authceremony`, `$authrate` and `$mailoutbox`. `$authrate` holds the counters and device markers. The `auth.ceremony.sweep` worker clears it.

### Risks that remain

- One script can spend an address's hourly allowance. The real person's device lane survives this.
- One script can spend the deployment's hourly ceiling.
- Turning on the human check is the answer to both.

### From the SDK

```ts
import { beginEmailSignIn, completeEmailSignIn } from '@hominemai/ordo-sdk';

const sent = await beginEmailSignIn(endpoint, {
  address: 'nelly@example.com',
  human: turnstileToken,           // optional
  device: localStorage.getItem('device') ?? undefined,  // optional
});
// ask the person for the code, then:
const session = await completeEmailSignIn(endpoint, sent, code);
localStorage.setItem('device', session.device);
```

- `beginEmailSignIn` returns `{ challenge, binder }`.
- `completeEmailSignIn` takes that result, the code and optional options. It returns the sign-in result.
- `session.device` is the marker to keep.

---

## 11. One sign-in across subdomains

### What it is for

Your core site is `login.example.com`. Your app is `app.example.com`. The
person signs in once, at the core site, and lands back on the app page they
came from, signed in. There is no shared cookie and no token in a URL.

### The five steps

1. The subdomain page makes a random binder, keeps it in `sessionStorage`, and sends the person to the core site with `return_to` and the binder's SHA-256 (`binder_hash`).
2. The core site signs the person in, if they are not already. It then calls `POST /auth/handoff` with its bearer token, `return_to` and `binder_hash`.
3. The door checks the session is live and `return_to` is on an allowed origin. It files a one-time code and answers `redirect`: `return_to` with `handoff_code=<code>` added.
4. The core site sends the browser to `redirect`.
5. The subdomain page calls `POST /auth/handoff/redeem` with the code and the binder it kept. It gets its own session.

The code is useless without the binder, which only the starting page holds.
It lives `ORDO_AUTH_HANDOFF_SECS` seconds (default 60, 1 to 300) and works once.

### The settings

| Setting | Set it to |
|---|---|
| `ORDO_AUTH_RETURN_URLS` | The subdomain, for example `https://app.example.com`. Any path on that origin is accepted. |
| `ORDO_CORS_ORIGINS` | The same subdomain page, so it can call the door. |
| `ORDO_WILDCARD_ORIGINS` | Optional. `on` lets you write `https://*.example.com` in the lists above. Off by default. |

- With `ORDO_AUTH_RETURN_URLS` unset, every hand-off is refused.
- A wildcard entry with `ORDO_WILDCARD_ORIGINS` off refuses the boot, and the message names the setting.
- The `*` is one label, `https` only. `https://*.com` and the bare apex are refused.
- With the wildcard on, every subdomain is yours to serve. A forgotten DNS record lets whoever claims it serve a page the lists accept.

### From the SDK

- On the subdomain page, `startSubdomainSignIn` makes and keeps the binder and gives the address to send the person to.
- On return, `finishSubdomainSignIn` reads `handoff_code` from the address, redeems it with the binder, and removes both.
- On the core site, `readHandoffRequest` reads `return_to` and `binder_hash` from the address the person arrived on.
- Once signed in, `issueHandoff` calls `POST /auth/handoff` with the signed-in client's token and returns the address to send the browser to.

### What the new session carries

- The same person.
- The same ceiling. The new session ends when the old one would, so a hand-off cannot lengthen a sign-in.
- The second step, if the old session had one.
- No passkey presence. The subdomain page did not touch an authenticator. A step-up there runs its own passkey ceremony.
- `via` is `handoff`, so an audit and the page can tell how it began.

The new session has its own id. Logging out on the core site does not end it.
"Sign out everywhere" does.

---

## 12. Linking a second sign-in

### What it is for

One account, several ways in. A person signed in with Discord can also sign in with an emailed code, and both open the same account. The owner's ruling: "Existing account logs in and claims another account."

### The three steps

1. Sign in again with a sign-in the account already holds. Call `POST /auth/claim/begin` with your bearer token and that ceremony's complete body. You get a `claim`.
2. Begin the other sign-in. For an emailed code, pass `claim` in `/auth/begin`.
3. Call `POST /auth/claim/complete` with your bearer token, the `claim` and the other ceremony's complete body. It must land inside 300 s.

```
POST /auth/claim/begin
Authorization: Bearer <token>
{ ...the complete body of a fresh sign-in the account holds }
-> { "claim": "<secret>", "expires_at": ... }

POST /auth/begin
{ "method": "email", "identifier": "me@example.com", "claim": "<secret>" }

POST /auth/claim/complete
Authorization: Bearer <token>
{ "claim": "<secret>", ...the complete body of the other sign-in }
-> { "linked": "email:me@example.com", "via": "email-otp", "authenticators": 2 }
```

### The settings

| Setting | Set it to |
|---|---|
| `ORDO_AUTH_LINKING` | `on` (the default) or `off`. Off restores the old rule: a second sign-in is added only with a passkey's fresh presence. Any other value refuses the boot. |
| `ORDO_AUTH_LINKING_SECS` | Seconds between the two ceremonies. 60 to 900, default 300. Outside that refuses the boot. |

### What it never does

- It never joins accounts on a matching email, verified or not. Two ceremonies are the only proof.
- It takes no passkey assertion. A passkey already names the account it was made for.
- It moves nothing between accounts. A sign-in that already names another account is refused with 409 `identifier_taken`.
- It mints no principal, no session and no presence.
- A hand-off session claims the same way, by two fresh ceremonies.
- A stolen token alone cannot link. It cannot complete the first ceremony.

### Answers

| Status | Meaning |
|---|---|
| 401 `authentication_failed` | The one refusal. The reason is in the security stream. |
| 403 `linking_off` | Linking is off. The message names `ORDO_AUTH_LINKING=on`. |
| 409 `identifier_taken` | The sign-in already belongs to an account. Also the answer for a sign-in of a deleted account. |
| 503 `credential_store_unavailable` | A store could not be read. Try again. |

### Recorded

- The link is recorded as `authenticator-linked`. It names the added sign-in and the one that vouched for it.
- Every refusal is recorded with its reason.

### From the SDK

The SDK's `beginClaim` and `completeClaim` wrap the two routes. `claimPasskey` adds a passkey inside an open claim.

### Adding a first passkey

A person who signed up with Discord, GitHub, an emailed code or a password holds no passkey. They add one with a claim. Any sign-in the account holds may vouch for it.

1. Open a claim with `POST /auth/claim/begin`, as above.
2. Call `POST /auth/claim/passkey` with your bearer token and `{"claim": "<secret>"}`. You get creation options for `navigator.credentials.create`. They are bound to your account.
3. Call `POST /auth/claim/complete` with your bearer token, the `claim` and the registration: `{"method": "passkey", "kind": "passkey_registration", "challenge", "client_data_json", "attestation_object"}`.

The key is stored only once it is linked. From then on it is an ordinary passkey. It signs in, and its sign-in proves presence. The link is recorded naming the sign-in that vouched for it. `ORDO_AUTH_LINKING=off` refuses this too.

`/auth/authenticators/link` is unchanged: there, a passkey you hold vouches for a new one.

---

## 13. Guest sign-in

### What it is for

A visitor uses the app at once, with no email and no provider. "Continue as Guest". The guest gets a real account of its own. Later it can claim Discord, an email address or a passkey and keep everything.

Ordo provides the mechanism. Whether your app shows it is your choice. It is off by default.

### Turning it on

| Setting | Set it to |
|---|---|
| `ORDO_AUTH_GUEST` | `on` or `off` (the default). Seeds the stored setting on a boot that finds none. After that the stored value wins. Any other value refuses the boot. |
| `std.deployment.setGuest` | The op that changes it while serving: `{"mode": "on"}`. Owner of the deployment's own space. Without `mode` it answers the setting. |
| `ORDO_AUTH_GUEST_TTL_SECS` | How long a guest that claimed nothing, and that no tenant admitted, is kept. Unset, it is `ORDO_AUTH_REGISTRATION_TTL_SECS` (90 days). It can only shorten that. `0` or a word refuses the boot. |

With guests on, a script can make guests up to the sign-up ceiling. Turn the human check on beside it (`ORDO_AUTH_HUMAN_CHECK=turnstile`). The boot prints one line when guests are on and the check is off.

### Start and come back

```
POST /auth/complete
{ "method": "guest", "kind": "new", "human"?: "<turnstile token>", "display"?: "Guest" }
-> { "token": "...", "via": "guest", "guest_secret": "<43 characters>", ... }

POST /auth/complete
{ "method": "guest", "kind": "resume", "secret": "<guest_secret>" }
-> { "token": "...", "via": "guest", ... }
```

- `guest_secret` is returned once. Keep it on the device, for example in `localStorage`. It is the guest's only way back in until it claims another sign-in.
- Ordo stores the SHA-256 of the secret, never the secret.
- A guest has no `begin`. Each step is one request.
- `space` and `invitation` work as for any sign-in.

### What a new guest goes through

The same as any first sign-in:

1. The setting. Off answers 403 `guests_off`, naming `ORDO_AUTH_GUEST` and `on`.
2. The human check, when it is on. A missing or bad token is the one refusal.
3. The sign-up ceiling, `ORDO_PROVISION_SIGNUPS_PER_HOUR`. A guest counts **whatever the self-serve mode**, since nothing else bounds guests. Over it: 429 `quota_exceeded`, and nobody is made.
4. `ORDO_AUTH_PROVISIONING`. Under `invite-only` a new guest needs an invitation, like anyone.
5. Admission to a named `space`, and the self-serve hook.

A `resume` is not a sign-up and is not counted. It answers only a secret Ordo made, so it never makes a new account.

### Becoming a full account

The guest claims another sign-in (section 12). Its secret is the first ceremony:

```
POST /auth/claim/begin
Authorization: Bearer <the guest's token>
{ "method": "guest", "kind": "resume", "secret": "<guest_secret>" }
-> { "claim": "<secret>", "expires_at": ... }
```

Then complete Discord, an emailed code, or a new passkey with `POST /auth/claim/passkey`. The account keeps its id, its seats and its data. The guest secret still signs in.

### What a guest cannot do

- Step up. It proves no presence, unless you lowered the requirement. A passkey added by a claim can.
- Delete its own account, under the default, for the same reason.
- Be found by email. It has none.

### Removal

- A guest no tenant admitted is removed by the registration sweep, like anyone.
- A guest that claimed nothing goes after `ORDO_AUTH_GUEST_TTL_SECS`, when you set it.
- A guest with a seat, or a tenant of its own, is kept. Removing it is your decision.
- A deleted guest cannot resume. Its secret stays linked to the deleted account and signs nobody in.
- Turning guests off stops new guests. A guest that exists can still resume.

### Answers

| Status | Meaning |
|---|---|
| 401 `authentication_failed` | The one refusal: a wrong secret, a deleted guest, a failed human check, not invited. The reason is in the security stream. |
| 403 `guests_off` | Guests are off. The answer names `ORDO_AUTH_GUEST` and `on`. |
| 429 `quota_exceeded` | The sign-up ceiling is reached. |
| 503 `credential_store_unavailable` | A store could not be read. Try again. |

### From the SDK

`startGuest(endpoint, {human?, display?, space?, invitation?})` answers `{secret, result}`. `resumeGuest(endpoint, secret)` signs in again. `guestCeremony(secret)` is the first ceremony for `beginClaim`. A 403 `guests_off` rejects with failure `guests_off`.

---

## 14. Google, Microsoft and a company login (OIDC)

OIDC is the better way to sign people in through an outside provider. The
provider signs an ID token, and Ordo checks the signature. It works for
Google, Microsoft Entra and any company login with discovery (Okta, Keycloak,
Auth0). A connection is named and set up as in section 8, with the same
`ORDO_PUBLIC_URL`, `ORDO_AUTH_RETURN_URLS`, `_CLIENT_ID` and
`_CLIENT_SECRET_FILE`.

### What to register at the provider

One redirect URI: `<ORDO_PUBLIC_URL>/auth/callback/<name>`. For a connection
named `google` on `https://api.example.com` it is
`https://api.example.com/auth/callback/google`. Copy the client id, and put the
client secret in a file.

### Google

```
ORDO_AUTH_METHODS=passkey,google
ORDO_PUBLIC_URL=https://api.example.com
ORDO_AUTH_RETURN_URLS=https://app.example.com/signed-in
ORDO_AUTH_GOOGLE_PRESET=google
ORDO_AUTH_GOOGLE_CLIENT_ID=1234-abc.apps.googleusercontent.com
ORDO_AUTH_GOOGLE_CLIENT_SECRET_FILE=/run/secrets/google
```

The preset fills the issuer, the scope (`openid email profile`), the picture
claim and the email rule. Google's endpoints come from discovery.

### Microsoft

```
ORDO_AUTH_METHODS=passkey,microsoft
ORDO_AUTH_MICROSOFT_PRESET=microsoft
ORDO_AUTH_MICROSOFT_TENANT=9188040d-6c67-4c5b-b112-36a304b66dad
ORDO_AUTH_MICROSOFT_CLIENT_ID=00000000-0000-0000-0000-000000000000
ORDO_AUTH_MICROSOFT_CLIENT_SECRET_FILE=/run/secrets/microsoft
```

`_TENANT` is the directory (tenant) id, a GUID. A connection names one tenant.
`common`, `organizations`, `consumers` and domain names are refused. Ordo does
not use Microsoft's `email`, because Entra does not verify it for every account
type. Admit Microsoft people by provider name or by invitation.

### A company login

```
ORDO_AUTH_METHODS=passkey,corp
ORDO_AUTH_CORP_KIND=oidc
ORDO_AUTH_CORP_ISSUER=https://login.example.com/realms/staff
ORDO_AUTH_CORP_CLIENT_ID=ordo
ORDO_AUTH_CORP_CLIENT_SECRET_FILE=/run/secrets/corp
```

Ordo reads `<issuer>/.well-known/openid-configuration` for the endpoints. The
name shown to the app is `name`, then `preferred_username`. To pin the
endpoints and skip discovery, set `_AUTHORIZE_URL`, `_TOKEN_URL` and
`_JWKS_URL` together. The full list is in `docs/settings.md` section 1.

The issuer is fixed for life under the connection name. A later boot with a
different issuer refuses. A new issuer gets a new name.

### What Ordo checks

- The ID token's signature, against the keys at the issuer's JWKS address.
- `iss`, byte for byte against the configured issuer.
- `aud` holds your client id, and `azp` agrees when it is present.
- `nonce` is the one this sign-in started with.
- `exp`, `iat` and `nbf`, with 60 seconds of skew, 0 to 120. The deployment
  sets it for every connection with `ORDO_AUTH_OIDC_CLOCK_SKEW_SECS`. One
  connection overrides it with `_CLOCK_SKEW_SECS`.
- The algorithm is on the allow-list: `RS256`, `PS256`, `ES256`, `EdDSA`.
  `ORDO_AUTH_OIDC_ALGS` narrows it for every connection, and `_ALGS` for one.
  `none` and `HS*` are never accepted.

An email is kept only when `email_verified` is the JSON boolean `true`.

### Key rotation and discovery

- Keys and the discovery document are fresh for `ORDO_AUTH_OIDC_CACHE_SECS`.
  That is 300 to 86400 seconds, 3600 by default. The provider's
  `Cache-Control: max-age` shortens it but never lengthens it, and never below
  300. `no-store` and `no-cache` count as 0, so 300.
- A token with an unknown `kid` refetches the keys at once, at most once a
  minute. That 60 seconds is fixed. It caps how hard unknown `kid` values can
  drive your deployment at the provider. A provider that rotates a key needs no
  restart and is seen within a minute.
- A token with no `kid` is accepted when the keys hold exactly one key usable
  for the token's algorithm. With two or more it is refused (`kid-missing`).
- If the JWKS address is unreachable and the cache is warm, the old keys keep
  working for 24 hours past their expiry (`ORDO_AUTH_OIDC_STALE_GRACE_SECS`,
  0 to 604800 seconds). Each such use is recorded as
  `oidc-jwks-stale-cache-used`.
- Discovery refreshes on the same schedule. A failed refresh keeps the last
  good copy and is recorded as `oidc-discovery-stale-copy-used`. A refreshed
  document is checked against the configured issuer again. A moved endpoint
  needs no restart.
- The first use loads discovery. A failed first load is tried again no sooner
  than 60 seconds later, and `begin` answers 503 with the last failure word.

### The page

The page's code is the same as for Discord. `POST /auth/begin` with the
connection name as `method`, navigate to `redirect`, then redeem the handoff
at `POST /auth/complete` with the binder. The session's `via` is
`oidc:<name>`. No SDK change.

On failure the browser is sent `303` to `return_to` with
`?error=authentication_failed`, or `?error=identity_provider_unavailable` when
the provider could not be reached. `begin` answers `503
identity_provider_unavailable` when discovery cannot load. The body carries
`connection` and, where a setting fixes it, `setting`. The message names both.
An issuer on a private address names `ORDO_EGRESS_PRIVATE_HOSTS=<issuer host>`.
A discovery document naming another issuer names `ORDO_AUTH_<NAME>_ISSUER`.

### A provider on a private network

Every request to the provider goes through the egress rail: `https` only,
certificates validated, no redirects. A provider inside your own network is let
through by naming its host in `ORDO_EGRESS_PRIVATE_HOSTS`. That list applies to
sign-in connections (OAuth2 and OIDC) as it does to mail.

---

## 15. A company login's two-step claim as presence

### What it is for

A company login often asks for two steps already. Forcing a passkey on top would be a nuisance. The owner's ruling (R9): the app developer decides whether that claim counts as presence, and a tenant may decide for its own data.

### Turning it on

Each OIDC connection has its own settings. It is off by default.

| Setting | Set it to |
|---|---|
| `ORDO_AUTH_<NAME>_PRESENCE` | `off` (default) or `on`. `on` counts the claim everywhere. Under `off` it counts only in a tenant that accepts it. |
| `ORDO_AUTH_<NAME>_PRESENCE_AMR` | Comma list of `amr` words that mean two steps. Default `mfa`. Each word is 1 to 64 of letters, digits and `:._-`. |
| `ORDO_AUTH_<NAME>_PRESENCE_ACR` | Comma list of `acr` values that mean two steps. Default none. |
| `ORDO_AUTH_UPSTREAM_PRESENCE_SECS` | How old the claim may be. 30 to 300, default 300. Never longer than the 300 s step-up window. |

If both lists are empty, the boot is refused, whatever `_PRESENCE` says. The message names `_PRESENCE_AMR`.

### How a page asks

Add `"presence": true` to `/auth/begin`.

```
POST /auth/begin
{ "method": "<connection>", "return_to": "...", "presence": true }
```

Ordo adds `max_age=0` to the authorize URL. The provider then prompts the person now. Without `"presence": true`, no claim is ever minted, however fresh the provider's `mfa`. Any OIDC connection may be asked, one set `off` included.

### What counts

All of these must hold for a claim to be minted.

- The sign-in asked (`max_age=0` was sent).
- The ID token carries `auth_time`. It is no older than the window and not in the future beyond the connection's clock skew.
- `amr` holds one of the `_PRESENCE_AMR` words, or `acr` equals one of the `_PRESENCE_ACR` values.

A single-factor claim (`amr: ["pwd"]`) or a stale one mints nothing. The sign-in still succeeds, without presence. A minted claim counts only where the next section says.

### Where it counts

- **`on`**: everywhere, acts on the deployment included (`std.deployment.apply`, `std.space.create`, delete).
- **A tenant's acceptance**: only in that tenant's space, whatever the connection's setting (owner, 2026-10-09). The tenant calls `std.space.setAdmission` with `presenceFrom`, a comma list of connection names. An empty list clears it. A name that is not a connection of the deployment is refused, and the message lists the connections. `std.space.admission` answers `presenceFrom`.
- **`off`, and no tenant accepted it**: nowhere.
- A tenant's acceptance never reaches an act on the deployment: `std.deployment.*`, `std.space.create`, `std.space.delete`, `std.space.reap`, or any op in the deployment's own space (`ORDO_SPACE`).

The claim must also be within `ORDO_AUTH_UPSTREAM_PRESENCE_SECS` when the op uses it.

### Answers

| Status | Meaning |
|---|---|
| 400 `presence_not_available` | The method is not an OIDC connection. No setting fixes this, so none is named. |

### Recorded

- At `/auth/complete`, every sign-in whose provider sent `amr`, `acr` or `auth_time` is recorded as `upstream-claim`.
- Act `presence-vouched` (`vouched_by=<connection> at=<auth_time>`) when the claim counted. Act `noted` when it did not.
- The detail names the principal, the connection (`oidc:<name>`), `amr`, `acr` and `auth_time`.
- At the callback, a sign-in that asked and got no presence is recorded as `presence-not-minted`. The `reason` is one of `presence-stale`, `presence-single-factor`, `presence-no-auth-time`, `presence-auth-time-in-the-future`, `presence-no-claims`.
- When the claim stood in for Ordo's second step, act `second-step-satisfied`, naming the principal, the connection and the level (`deployment` or `tenant`, with `space=` for a tenant).

### The second step

The same claim stands in for Ordo's second step (owner, 2026-10-09). It applies where the deployment (`ORDO_AUTH_SECOND_FACTOR`) or a tenant (`secondFactor`) requires one.

- **`on`**: no second step from Ordo at sign-in, at any level. The door lets the session into every tenant.
- **A tenant's acceptance**: that tenant's own requirement is met, when the sign-in names the tenant (`"space"` in `/auth/complete`). The deployment's requirement and the person's own factor still ask. The door still refuses the session in another tenant that requires a step.
- **A stale or single-factor claim, or a connection not accepted**: Ordo's second step, as for a password.
- A reauth (`"reauth": true`) always takes Ordo's own step.

### What it never does

- It never satisfies an act that needs a passkey: `refresh_presence`, linking by `LinkCeremony`, or a recovery acknowledgement.
- A capability or delegated token never carries it.
- A hand-off to another subdomain (`/auth/handoff`) carries none.
- It keeps ageing through a refresh.
- The token format moved to v6. Tokens minted before are refused, and the person signs in again.

---

## 16. Tenants bring their own provider

### What it is for

A tenant's owner can add a sign-in provider of their own, such as the company's login. The tenant's people sign in through it. Newcomers it brings land in that tenant.

### Switching it on

The app developer sets it. It is off by default.

```
ORDO_AUTH_TENANT_PROVIDERS=on
```

It needs `ORDO_PUBLIC_URL` and `ORDO_AUTH_RETURN_URLS`. Off, every op and route below answers a refusal that names the setting. Turning it off stops every tenant's provider at once. The rows stay.

### The ops

A tenant owner calls these in the tenant's own space, with presence. They are refused in the deployment's own space.

| Op | What it does |
|---|---|
| `std.space.addConnection` | Adds a provider |
| `std.space.updateConnection` | Changes it. `kind` and `issuer` cannot change |
| `std.space.retireConnection` | Stops it |
| `std.space.connections` | Lists them. The secret is never shown, `secret` says `set` |

```
std.space.addConnection
{ "name": "acme-sso", "kind": "oidc",
  "issuer": "https://login.acme.example",
  "clientId": "ordo-notes", "clientSecret": "...",
  "scope": "openid email", "role": "viewer" }
```

- `name` is 1 to 32 of `a-z`, `0-9` and `-`.
- `kind` is `oidc` or `oauth2`. An `oauth2` connection also needs its addresses.
- `role` is the role a newcomer gets. It defaults to `viewer`.
- The secret is sealed with the tenant's key and never shown back.
- To move to another issuer, add a new connection.

### The callback address

Every row from `std.space.connections` carries `callback`. Register that address at the provider. It looks like `<ORDO_PUBLIC_URL>/auth/callback/t/<tenant>/<name>`.

### Signing in

Name the tenant at `begin` and again at `complete`.

```
POST /auth/begin
{ "method": "acme-sso", "tenant": "acme", "return_to": "..." }

POST /auth/complete
{ "method": "acme-sso", "tenant": "acme", "kind": "handoff", ... }
```

### Who gets admitted

- The tenant named at `complete`, and no other. Naming another tenant is refused.
- At the connection's `role`.
- Under the deployment's ceiling (`ORDO_AUTH_PROVISIONING`): the provider way must be allowed.
- Under the deployment's provisioning: with `invite-only`, a newcomer needs an invitation.
- Built-in provisioning gives them no tenant of their own.
- Their sign-in works with linking by claim like any other, so they can add a passkey. It reaches only the tenants they belong to.

### The originating admin

A tenant always keeps one owner who signs in without the tenant's own providers. That owner holds a passkey, a password, an emailed code or a deployment connection.

- While the tenant has an active connection, these are refused when they would leave no such owner.
- They are: adding a connection, changing the role of an owner, suspending or removing one, an owner leaving, resetting an owner's password, and unlinking an owner's last such sign-in.
- Each check and its change happen under one lock in the tenant's database, so two changes at the same moment cannot both pass. A change waits up to `ORDO_AUTH_TENANT_LOCK_WAIT_MS` (10000 by default) for the lock, then is refused with a message that names the setting.
- The refusal tells you to make another owner who has a passkey first.

### Addresses

- Private addresses are allowed.
- Loopback, link-local, multicast and reserved addresses are refused.
- The deployment's own ranges (`ORDO_EGRESS_DENY_CIDRS`) are refused.
- Cloud metadata addresses are refused.
- The check runs at connect time, on every address the name resolves to.

### Which deployment settings reach a tenant's connection

A tenant's connection carries no skew, algorithm or cache value of its own. It takes the deployment's:

- `ORDO_AUTH_OIDC_CACHE_SECS`: how long keys and discovery stay fresh.
- `ORDO_AUTH_OIDC_CLOCK_SKEW_SECS`: the clock skew, 60 seconds by default.
- `ORDO_AUTH_OIDC_ALGS`: the accepted algorithms, all four by default.
- `ORDO_AUTH_OIDC_STALE_GRACE_SECS`: how long old keys verify while the issuer is down.
- `ORDO_AUTH_<NAME>_CLOCK_SKEW_SECS` and `_ALGS` do not reach it. They override the deployment's values for one deployment connection only.
- `ORDO_AUTH_<NAME>_PRESENCE` does not (see Presence).

### Presence

A tenant connection has no presence setting. Its two-step claim is made whenever the sign-in asks (`"presence": true` at `begin`, `oidc` only). It counts as presence only in two cases together.

- The sign-in is inside its own tenant.
- The tenant lists the connection's `key` (from `std.space.connections`) in `presenceFrom` of `std.space.setAdmission`.

Another tenant that lists the key is refused. `ORDO_AUTH_<NAME>_PRESENCE` never reaches a tenant's connection. The claim never counts in the deployment's own space or for acts on the deployment.

### Retiring

`std.space.retireConnection` stops new sign-ins through the connection at once, on every replica. `GET /auth/methods?tenant=` may still list it for up to `ORDO_AUTH_METHODS_MAX_AGE` seconds (60 by default), from a cache. Adding the same name again makes a new connection with a new key. People linked through the old one do not carry over.

---

## 17. Going to production

The same variables change from local to live. Nothing in the page changes
except its `DOOR` address.

| Setting | Local | Live |
|---|---|---|
| `ORDO_AUTH_METHODS` | `passkey` | `passkey` |
| `ORDO_APEX_DOMAIN` | `localhost` | The registrable domain your pages live under, for example `inf.smg.icu`. **Changing it later strands every passkey**, so choose once. |
| `ORDO_AUTH_ORIGINS` | `http://localhost:5173` | The page's exact `https` origin, for example `https://notes.inf.smg.icu`. |
| `ORDO_CORS_ORIGINS` | `http://localhost:5173` | The same origin. Keep it exact. `*` refuses the boot. |
| `ORDO_AUTH_PROVISIONING` | `auto-onboard` | `auto-onboard` when strangers may sign up. `invite-only` (the default) when every person arrives by invitation. |
| `ORDO_PROVISION_SELF_SERVE` | `on-signup` | `on-signup` for a tenant per person. `off` (the default) when you place people in tenants yourself. |
| `ORDO_AUTH_SECOND_FACTOR` | unset (`enabled`) | `off`, `enabled`, `required` or `required-for-non-passkey`. Passkey sign-in works with the default. |
| `ORDO_DEPLOYMENT_OWNERS` | unset | Your own account id (`usr_...`), set after you have signed up once. It lets you apply a program over the door as yourself. |

Two more changes are not sign-in settings. The page needs its door address
at build time, and the deployment needs real keys and a real database
password instead of `serve.sh`'s constants. `docs/go-live-notes.md` is the
step-by-step for the notes app, and it checks each step with what you
should see. `docs/deploying.md` §4c has the full first-deployment sequence,
§5 the sign-in routes and §5b the CORS rules.

Find your account id for `ORDO_DEPLOYMENT_OWNERS` the way the notes page
does: it prints it under "About this account", read from the token by
`principalOf` in `web/src/token.ts`.

Before you go live, run through the notes smoke test in
`docs/go-live-notes.md` step 8. Sign up as a second person in a private
window and confirm the two never see each other's notes.

---

## 18. An email address on the account

### What it is for

A person who signs in by passkey or Discord has no address Ordo knows. So
mail from the app, such as a routine's `notify`, cannot reach them. Owner
ruling 2026-10-22: they can add a verified address. "yes - but app dev
could make it required."

### Turn it on

Set the mail channel from section 9. Nothing else. Without mail the routes
answer `403 email_unavailable`, naming `ORDO_MAIL_KIND`.

### The routes

Each takes a session's bearer token. A share link's or a delegated token is
refused.

| Route | Body | Answers |
|---|---|---|
| `GET /auth/email` | | `{address, source, verified_at, required, setting, mail}` |
| `POST /auth/email/begin` | `{address}` | `{challenge, binder}`. A code is mailed. |
| `POST /auth/email/complete` | `{challenge, binder, code}` | `{address, source, verified_at, replaced}` |
| `POST /auth/email/remove` | | `{removed}` |

- The address counts only after `complete`.
- Changing or removing an address needs a fresh passkey touch (`POST /auth/step-up` first). Without one the answer is `403 presence_required`, and the message names the setting. Adding a first address needs no touch. See "Asking for a touch" below.
- The code is the emailed sign-in code of section 10: the same digits, life, tries and limits, and the same settings.
- The message says "Confirm your email address". A confirmation code never signs anybody in, and a sign-in code never confirms an address.
- A wrong, late, used or another person's code is the one refusal, `401 authentication_failed`.
- A change mails the old address to say so. So does a removal. The new address is not in that message.
- `source` is `code`, `sign-in` (the address of an emailed-code sign-in), or the connection that vouched for it.
- An emailed-code sign-in's address stays when you remove. It is a way in, unlinked with `/auth/authenticators/unlink`.

### An outside connection's address

A sign-in whose provider marked the address verified stores it, when the
person has none yet. Discord's `verified: true` and an OIDC
`email_verified` count. It never replaces an address the person chose.

### Where mail goes

`notify` and other app mail use the added address first. Without one they
use the address of an emailed-code sign-in. A person with neither is told
in the app only.

### Making it required

Optional by default. Two layers:

| Layer | Setting |
|---|---|
| Deployment | `ORDO_EMAIL_REQUIRED`: `off`, `all`, or a comma list of ops |
| App developer | `std.deployment.setEmailPolicy {required}` in the deployment's own space. `default` hands it back to the deployment. Without `required` it reads what is in force. |

- `all` covers every op of the app: its own and the derived `std.<entity>.*`. It never covers a platform op, so a person can always manage their account.
- A covered op, called by a signed-in person with no verified address, answers `403 email_required`. Nothing runs. The message names the setting in force.
- The check runs after the access check. A caller who may not call the op learns nothing from it.
- If the address table cannot be read, the call is refused with `503 space_unavailable` and a `Retry-After`. The message names the setting. The same call lands once the table is back. Letting it through would drop a requirement the app developer set (owner, 2026-10-22).
- Not `off` without a mail channel refuses the boot. The op refuses it too.

### Asking for a touch

Owner, 2026-10-22: "Yes by default... But this is the app devs choice."
Changing or removing an address asks for a fresh passkey touch by default.
The app developer may turn it off. Two layers:

| Layer | Setting |
|---|---|
| Deployment | `ORDO_EMAIL_CHANGE_TOUCH`: `on` (default) or `off` |
| App developer | `std.deployment.setEmailPolicy {changeTouch}`: `on`, `off` or `default`. `default` hands it back to the deployment. |

- The refusal is `403 presence_required`. It names the setting in force.
- Adding a first address never asks for a touch.
- The op answers `changeTouch` and `changeTouchSource` beside `required`.

### Recorded

Security events `contact-email-added`, `contact-email-changed` and
`contact-email-removed`, and `email-policy-set`. They name the person and
the source. No event and no log line carries an address.

### From the SDK

```ts
import { accountEmail, beginAccountEmail, confirmAccountEmail, isEmailRequired } from '@hominemai/ordo-sdk';

const now = await accountEmail(endpoint, token);      // what is set, what is required
const sent = await beginAccountEmail(endpoint, token, 'ada@example.org');
await confirmAccountEmail(endpoint, token, sent, code);
```

- `removeAccountEmail` takes it off.
- `isEmailRequired(e)` tells an `email_required` refusal apart.
- In React, `useAccountEmail({ baseUrl, token })` from `@hominemai/ordo-react` gives `email`, `pending`, `needed`, `problem`, and `begin`, `confirm`, `cancel`, `remove`. Notes v2's "Email for reminders" is built on it.
