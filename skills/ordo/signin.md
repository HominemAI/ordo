# Sign-in, staying signed in, tenants

The door serves `/auth/*`. The deployment owner picks the methods with
`ORDO_AUTH_METHODS` (`email`, `passkey`, `password`, OAuth names). The
page's origin must be in `ORDO_CORS_ORIGINS`.

## One session object

`createOrdoAuth({ baseUrl })` is the front door. Make it once, at module
level, and import it where needed. In React import it from
`@hominemai/ordo-react`; without React from
`@hominemai/ordo-sdk/auth/session.js`. Exact signatures:
`sdk-signatures.md`.

```ts
import { createOrdoAuth } from '@hominemai/ordo-react';

export const DOOR = 'http://127.0.0.1:4000';
export const auth = createOrdoAuth({ baseUrl: DOOR });
```

| Need | Call |
|---|---|
| start signed in after a reload | `useOrdoAuth(auth)` (React), or `await auth.resume()` |
| email code | `auth.beginEmail({ address })`, then `auth.completeEmail(sent, code, { display })` |
| passkey | `auth.registerPasskey({ display })` (first time), `auth.signInWithPasskey()` |
| a live token for every call | `auth.token` (pass the function, do not call it) |
| renew | automatic: a timer renews ahead of expiry, and `auth.token()` renews when near |
| sign out | `await auth.signOut()` |
| the person's tenants and roles | `await auth.spaces()` |
| wait for a new person's own tenant | `<TenantGate baseUrl={DOOR} token={auth.token}>`, or `await auth.waitForTenant()` |
| prove presence again | `await auth.stepUp()` |
| which methods are on | `signInMethods({ baseUrl: DOOR })` from `@hominemai/ordo-sdk/auth/methods.js` |
| read an invitation before sign-in | `peekInvitation({ baseUrl: DOOR }, ticket)` from `@hominemai/ordo-sdk/auth/share.js` |

- `auth.token()` throws a `PasskeyError` with `failure: 'refused'` when
  nobody is signed in. A session that ended (signed out elsewhere, past
  its limit) signs the object out, and `useOrdoAuth` shows `signed-out`.
- Renewal first uses the token, then the browser's renewal cookie. A door
  that cannot be reached throws and keeps the session: never show sign-in
  for it.
- A session from another ceremony (password, second step, guest) is kept
  with `auth.use(session)`.

## The app shell in React

```tsx
import { TenantGate, useOrdoAuth } from '@hominemai/ordo-react';
import { auth, DOOR } from './auth';

export function App() {
  const { phase, retry } = useOrdoAuth(auth);
  if (phase === 'checking') return <p role="status">Loading…</p>;
  if (phase === 'failed')
    return (
      <p role="alert">
        Cannot reach the server. <button type="button" onClick={retry}>Try again</button>
      </p>
    );
  if (phase === 'signed-out') return <SignIn />;
  // With ORDO_PROVISION_SELF_SERVE=on-signup: waits for the person's own tenant.
  return (
    <TenantGate baseUrl={DOOR} token={auth.token}>
      {(space) => <Workspace space={space} />}
    </TenantGate>
  );
}
```

`Workspace` builds the client and `OrdoProvider` (`react.md`). A sign-out
button is `onClick={() => auth.signOut()}`; the shell follows by itself.

- **Same site.** Resume uses an `HttpOnly` cookie, so the page and the door
  must be the same site. `localhost` and `127.0.0.1` are different sites:
  use one name for both. Across sites nothing resumes and the person signs
  in on every reload.
- **Without self-serve tenants**, skip `TenantGate`. `await auth.spaces()`
  answers `[{ space, name, role, via }]`; let the person pick one. An empty
  list means they belong nowhere yet.

## Email sign-in form

```tsx
import { useState, type FormEvent } from 'react';
import { OrdoRefusal, type EmailCodeSent } from '@hominemai/ordo-react';
import { auth } from './auth';

type Invite = { space: string; invitation: string };

export function SignIn({ invite }: { invite?: Invite }) {
  const [address, setAddress] = useState('');
  const [name, setName] = useState('');
  const [sent, setSent] = useState<EmailCodeSent | null>(null);
  const [code, setCode] = useState('');
  const [error, setError] = useState<string | null>(null);
  const say = (e: unknown) => setError(e instanceof OrdoRefusal ? e.refusal.message : 'Something went wrong. Try again.');

  async function send(e: FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      setSent(await auth.beginEmail({ address, ...invite }));
    } catch (err) {
      say(err);
    }
  }

  async function finish(e: FormEvent) {
    e.preventDefault();
    if (!sent) return;
    setError(null);
    try {
      // `display` names a new person; a known person keeps their name.
      const r = await auth.completeEmail(sent, code, { ...invite, display: name });
      if (r.kind === 'second-step') setError('This account needs a second step.');
      // A session: useOrdoAuth now answers signed-in.
    } catch (err) {
      say(err); // a wrong, expired or used code is one refusal: failure 'refused'
    }
  }

  return sent === null ? (
    <form onSubmit={send}>
      <label htmlFor="email">Email</label>
      <input id="email" name="email" type="email" autoComplete="username" required
        value={address} onChange={(e) => setAddress(e.target.value)} />
      <label htmlFor="name">Your name (new accounts)</label>
      <input id="name" name="name" autoComplete="name" value={name} onChange={(e) => setName(e.target.value)} />
      <button type="submit">Email me a code</button>
      {error && <p role="alert">{error}</p>}
    </form>
  ) : (
    <form onSubmit={finish}>
      <label htmlFor="code">Code from the email</label>
      <input id="code" name="code" autoComplete="one-time-code" inputMode="numeric" required
        value={code} onChange={(e) => setCode(e.target.value)} />
      <button type="submit">Sign in</button>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}
```

- Keep `sent` (challenge and binder) in memory only. Never in a URL or storage.
- `beginEmail` answers the same for every address. It never says whether
  the address is known.
- A session may carry `device`. Keep it in `localStorage` and pass it as
  `device` to the next `beginEmail`.

## Without React

```ts
import { createOrdoAuth } from '@hominemai/ordo-sdk/auth/session.js';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';

const auth = createOrdoAuth({ baseUrl: DOOR });
if ((await auth.resume()) === null) {
  // sign in: auth.beginEmail, then auth.completeEmail
}
const space = (await auth.spaces())[0]?.space ?? (await auth.waitForTenant());
const client = new OrdoClient({ ...auth.clientOptions(), space });
```

`auth.clientOptions()` is `{ baseUrl, token, renewToken }`. The plain
functions are there too: `refreshSession(door, token)` and
`signOut(door, token)` in `auth/session.js`.

## Invitations

1. An owner or manager calls `std.space.invite` (`react.md`) and gets a
   `ticket` once. The app builds a link with the space id and the ticket,
   for example `/#/join/<space>/<ticket>`, and shows or mails it. Ordo sends
   no mail for invitations.
2. The landing page may call `peekInvitation(door, ticket)` before sign-in:
   `{ space, role, ticketExpiresAt, named }`.
3. The invitee signs in with `{ space, invitation: ticket }` passed to
   `auth.beginEmail` and `auth.completeEmail` (or to `auth.signInWithPasskey`
   / `auth.registerPasskey`).
4. `session.admission` is `{ space, outcome }`. `admitted` or
   `already-member` means they are in `space`. Anything else (`refused`)
   says nothing about why.
5. `declineInvitation(door, ticket, token?)` spends the ticket.

## A new person's tenant

With `ORDO_PROVISION_SELF_SERVE=on-signup`, the tenant is made just after
the sign-in answers.

- React: `<TenantGate baseUrl token>{(space) => app}</TenantGate>`, with
  optional `settingUp`, `stuck` and `failed` slots. Or
  `useTenantStatus({ baseUrl, token })` for
  `{ phase, space, status, problem, retry }`.
- Plain: `const space = await auth.waitForTenant()`. It throws a
  `TenantError` whose `failure` is `failed`, `off`, `none`, `timeout`,
  `refused`, `limit_reached` or `aborted`.
- Just after a tenant is made or joined, the first call is served.
  `access_denied` is never a delay; `server_busy` is, and is retried.

## Passkeys and step-up

- `auth.registerPasskey({ display, space?, invitation? })` signs up.
- `auth.signInWithPasskey({ space?, invitation? })` signs in.
- `auth.stepUp()` proves presence for a few minutes and keeps the
  re-minted session.
- A `PasskeyError` carries `failure`: `cancelled` (the person dismissed
  the prompt; say nothing), `unsupported`, `refused`, `unavailable`
  (try again), and a few setting-named ones whose `setting` and `value`
  say what the deployment owner would change.
