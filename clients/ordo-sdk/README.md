# `@hominemai/ordo-sdk`

The generated TypeScript client for one Ordo deployment. One typed function
per op; the terminal verb decides what you can do with it.

```ts
import { OrdoClient, cheapActive, addAnonCustomer, OrdoRefusal } from '@hominemai/ordo-sdk';

const client = new OrdoClient({
  baseUrl: 'https://api.example.com',
  space: 'spc_acme',
  token: () => auth.accessToken(),
});

// A read is a lazy builder that must terminate in a read verb.
const page = await cheapActive(client, { cap: '4000.00' }).fetch();
for await (const p of cheapActive(client, { cap: '4000.00' }).pages()) {
  render(p.rows); // p.rows[0].price is a string, so money is exact
}

// A write terminates in `.commit()` and has no read verbs.
try {
  await addAnonCustomer(client, { name: 'Ada' }).commit();
} catch (e) {
  if (e instanceof OrdoRefusal && e.class === 'duplicate') { /* … */ }
}
```

Every mutation in an application is `grep '.commit('`.

**Breaking in 0.3.0.** `SPEC_LIST` became `specList()`. Call the function. It
builds the list on first use and returns the same array after that.

**Breaking in 0.5.0.** One front door for sign-in: `createOrdoAuth`
(resume, renew, sign out, tenants). `completeEmailSignIn` takes `display`.
The package root no longer re-exports the sample generated surface; import
your app's own `generated/ops`. On a converging write the generated client
requires each optional field's key: the current value keeps it, `null`
clears it.

**Changed in 0.4.0.** One request helper, one retry policy and one error
shape: every sign-in error is an `OrdoRefusal`. See "One request helper, one
retry policy, one error shape" below.

---

## Installing it in an app outside this repository

The package is private, published to GitHub Packages under the HominemAI
organization.

1. Add this line to the app's `.npmrc`:

   ```
   @hominemai:registry=https://npm.pkg.github.com
   ```

2. Create a personal access token (classic) with the `read:packages` scope.
   Keep it out of git: put `//npm.pkg.github.com/:_authToken=<your token>` in
   your user `~/.npmrc`, not the project's.

3. Install:

   ```
   npm install @hominemai/ordo-sdk
   ```

A release is a version bump in `package.json` merged to `main`; the
`Packages` workflow publishes it.

---

## Two halves

| | **Static core**: `src/core/` | **Generated**: `src/generated/` |
|---|---|---|
| Written by | a person, once, reviewed | `ordo gen ts`, from `interface.json` |
| Contains | the `fetch` wrapper, the header set, the page loop, the CAS rules, the refusal decoder, the id brand | one function per op, its params, its row shape, its refusal union, and the op table |
| Same bytes for every deployment | yes | no |
| Edited by hand | yes | **never** |

**The generated half declares; the static half decides.** The line is there
because of what happens when it moves: code in the generated half is code
nobody can fix (the fix is lost on the next `ordo gen ts`, and the person who
lost it finds out in production), and data in the static half is data that
goes stale against a deployment and cannot be regenerated.

There is **no op-name heuristic anywhere in this package**. The wire method
comes from `call.method`, the legal verbs from `call.terminal`, the version
pin from `op.version`, the CAS parameter from `statements[].cas_param`, all
of it arriving as data in `src/generated/specs.ts`. That is the property
`ordo.interface/2`'s `call` stamp exists to buy.

## What the types buy you

- **Terminal verbs** (`tooling-and-embedding-plan.md` §B3): a paged read is a
  `ReadCall` (`.fetch()`, `.pages()`, `.rows()`); a read the executor answers
  whole is a `SingleReadCall` (`.fetch()`, `.row()`; `.pages` does not
  exist, because there is no second page); a write or delete is a `WriteCall`
  (`.commit()`, and no read verbs). Misuse is a compile error in your own
  language.
- **Branded ids.** Every id is spelled `id` by the type system, which is why
  the descriptor had to grow `id_entity` at all. `VendorsId` and
  `CategoriesId` are distinct types; the wire still sees the string.
- **Enumerated parameters** are the closed union the descriptor resolved
  through `field_ref`, not `string`.
- **Refusals are the op's own set**, not the whole table: `Extract<RefusalClass, …>`
  per op. A caught class outside it is not a bug: the door has refusals that
  precede any op (`missing_space`, `caller_supplied`, `delegation_required`),
  and `Refusal.declared` says which happened.
- **A `json` parameter is `JsonValue`.** Any JSON value, sent as JSON and never
  as a string holding JSON. The door refuses a document over its size or depth
  limit as `bound_violation` (`docs/plan-json-field.md`).
- **Money is an exact decimal string.** Never a `number`, never a `bigint`.
  `api-engine-design.md` §5 settles the wire shape and the descriptor's
  bounds are decimal digit strings; parsing it into anything else would be
  choosing a second representation for the value the client exists to carry
  unchanged, and rounding it on the way back out. (§B3 proposed
  "bigint-minor-units in TS" before the wire was settled; the wire won.)

## What it refuses before the wire

The door's own classes, run early. They are not a second contract. Bind order is the
door's (§5): unknown keys first, so a typo surfaces before "missing" noise.

- `unknown_param`, `missing_param`, `invalid_param`, `bound_violation`.
- **CAS is unavoidable, not merely available.** An op that asserts a revision
  and is committed without one is refused here, so there is no
  last-write-wins path through this package. The class is the door's
  `missing_param`; only the message differs, and it has to.
- A **standing registration failure** the descriptor already carries
  (`error`) refuses with the descriptor's own class and makes no request.

Validation runs in the `Call` constructor, so a bad argument is refused at
the **call site** rather than at the terminal verb. Otherwise the refusal
points at the wrong line and arrives as a rejected promise a caller who has
not awaited yet sees somewhere else entirely.

## Paging

`.pages()` stops when `next` is absent and on nothing else. Open-field
conditions compile into the plan and pages come back full (ruled 2026-10-05),
but a short or empty page with `next` set is still legal; a loop that
stopped on one would silently lose the rows after it.
`hasMore` is `next !== undefined`, never `rows.length > 0`.

Cursors ride as `X-Ordo-Page`, never in the query string: a cursor there ends
up in proxy logs, browser history and `Referer`.

## Refinement

Every read narrows with `.where` and `.pick` before its verb. Every program
read accepts a refinement, with no off switch (ruled 2026-10-05).

```ts
const page = await cheapActive(client, { cap: '9000' })
  .where((v) => [v.price.gt('4000'), v.name.in(['saw', 'vise'])])
  .pick('name', 'price')
  .fetch();
// page.rows: readonly Pick<CheapActiveRow, 'name' | 'price'>[]
```

- `v` has one field per refinable output, typed as the row types it. Money
  and decimals take decimal strings. A sealed field opted in to refinement
  takes `eq` and `in` only. A field that is not refinable does not compile.
- `.where` appends: conditions combine as AND. `.pick` replaces and narrows
  the row to `Pick<Row, K>`. Each call returns a new builder.
- The envelope is checked at the call site by `core/refine.ts`, the same
  function the device copy uses. A bad one throws `OrdoLocalRefusal` with
  class `invalid_refinement` and the field in `param`.
- It goes on the wire as `$refine`, URL-encoded JSON, with text made NFC.
- `.pages()` sends the same refinement with every cursor and still stops only
  on an absent `next`.
- Each read's refusal union carries `invalid_refinement` and
  `refinement_unbounded`, and `sealed_unsupported` where the op has a sealed
  output.

## Retrying a busy server

When every database connection a deployment holds is in use, the door answers
`503` with class `server_busy` and a `Retry-After` in seconds. Nothing ran.
The client sends the call again.

```ts
new OrdoClient({
  baseUrl, space, token,
  busyRetry: { retries: 2, firstDelayMs: 500, factor: 2, maxDelayMs: 10_000, jitter: true }, // the defaults
});
```

- `busyRetry.retries` is how many times a call is sent again. **Default 2**;
  `0` turns it off. `busyRetries: n` is a shorthand for it.
- The wait before retry `n` (from 0) is
  `min(maxDelayMs, firstDelayMs * factor^n)`: 500 ms, 1 s, 2 s, ... up to 10 s
  with the defaults. It is never less than the `Retry-After` the door sent,
  which is itself held to `maxDelayMs`.
- `jitter` (default on) adds up to 20% at random to each wait, so clients
  refused together do not come back together. `jitter: false` gives the exact
  waits.
- A bad value (a negative or fractional `retries`, a `factor` below 1, a
  negative delay, an unknown key, `busyRetries` and `busyRetry.retries`
  disagreeing) throws `TypeError` when the client is made.
- `tenant_not_ready` (a `503` with `Retry-After`: the person's own tenant is
  still being set up) is retried the same way, with the same options,
  counted on its own. `onRetry` gets `tenant_not_ready` as the reason.
- Another 503 class (`space_unavailable`, `statement_timeout`) is thrown at
  once, because waiting does not repair it.
- A write is safe to send again. The client mints its `Idempotency-Key` once
  per call, and every attempt carries that same key.
- When the retries run out, the last busy answer is thrown as an `OrdoRefusal`
  of class `server_busy`.
- It covers `request`, so every generated call, and the file calls and the
  download (busy only). The sync feed is not retried here; the sync
  manager's reconnect loop handles a busy answer.

## Retrying a write that lost a race

Writes to one row at the same moment are taken one after the other. A write
that loses that race is tried again by the server, `ORDO_WRITE_RETRIES` times
(default 5). When every try loses, the door answers `409` with class
`serialization_conflict`. Nothing was applied. The client sends the call
again.

```ts
new OrdoClient({
  baseUrl, space, token,
  conflictRetry: { retries: 3, firstDelayMs: 50, factor: 2, maxDelayMs: 1_000, jitter: true }, // the defaults
});
```

- `conflictRetry.retries` is how many times a call is sent again. **Default
  3**; `0` turns it off. `conflictRetries: n` is a shorthand for it.
- The wait before retry `n` (from 0) is
  `min(maxDelayMs, firstDelayMs * factor^n)`: 50, 100, 200 ms with the
  defaults. A `Retry-After`, if the door sends one, is honoured the same way
  as for `server_busy`.
- `jitter` (default on) waits a random time from half that wait to all of
  it. Writers that collided on one row collide again if they come back
  together, so the spread here is wider than the busy retry's 20%.
- Bad values throw `TypeError` when the client is made, as for `busyRetry`.
- Only `serialization_conflict` is retried. Another 409 class (`cas_conflict`,
  `duplicate`, `lease_fenced`) is thrown at once: it says something about the
  data that sending again does not change.
- A write keeps its `Idempotency-Key` on every attempt. The door never stores
  a `serialization_conflict` for replay, so the retry really runs, and a write
  that did land earlier is replayed and not run twice.
- Busy and conflict retries are counted apart. `onRetry(attempt, delayMs,
  reason)` is told before each wait, with `reason` `'server_busy'` or
  `'serialization_conflict'`.
- When the retries run out, the conflict is thrown as an `OrdoRefusal` of class
  `serialization_conflict`. Its message names the server's setting and value.

## When a call gets no answer

A browser reports three different failures the same way: a cross-origin call
the server refuses, a server that is down, and a server that is slow. Every
SDK call path throws one error for all three, `OrdoNetworkError`, and its
`kind` says which:

| `kind` | What happened | What fixes it |
|---|---|---|
| `blocked` | The server is up. The browser refused the call because the page's origin is not allowed. | The deployment owner adds the page's origin to `ORDO_CORS_ORIGINS`. For a sign-in or passkey call, also to `ORDO_AUTH_ORIGINS`. |
| `unreachable` | The server could not be reached. | Start it, or fix the address or the network. |
| `timeout` | The server did not answer within the wait. | Raise the wait: `callTimeoutMs`. |

```ts
try {
  await myNotes(client).fetch();
} catch (e) {
  if (e instanceof OrdoNetworkError) show(e.kind, e.message);
}
```

The message names the fix. A `blocked` error carries `pageOrigin` and
`settings` (the variables to set). A `timeout` carries `option`, the name of
the SDK option that sets the wait.

**How `blocked` is told from `unreachable`.** A browser hides why a
cross-origin call failed. So the SDK sends one more request, with
`mode: 'no-cors'`, to `/health/live` on the same server. The browser hides
that answer too, but it does not hide that there was one. An opaque answer
means the server is up, so the origin was refused. No answer means it is
down. A page on the server's own origin is never `blocked`, and a runtime with
no page origin (Node) sends no probe. The probe's answer is kept for 5
seconds, so a burst of failed calls probes once.

Where it is used: the core client (`OrdoClient`), every sign-in and
passkey call, the access and connection ops, delegation, and the sync feeds
(a feed that breaks after it was working is `unreachable`). A call the
caller aborts is not a network error: it throws the abort as it came.

Settings. Each has a default.

| Option | Where | Default |
|---|---|---|
| `callTimeoutMs` | `OrdoClient`, and each sign-in endpoint | 60,000 ms for an op call, 15,000 ms for a sign-in call |
| `network.probePath` | the same | `/health/live` |
| `network.probeCacheMs` | the same | 5,000 |
| `network.probeTimeoutMs` | the same | 5,000 |
| `network.pageOrigin` | the same | the page's own origin |

A client that takes a `fetch` and names no failures itself, such as the
`OrdoClient` of `@hominemai/ordo-react`, gets the same errors from
`namedFetch({ baseUrl })`:

```ts
new HooksClient({ baseUrl, iface, space, token, fetch: namedFetch({ baseUrl }) });
```

## A ready-made device

`openDevice` builds what an app used to build by hand: the replica, sealed at
rest in IndexedDB, and a sync manager over it.

```ts
import { openDevice } from '@hominemai/ordo-sdk';

const device = await openDevice({ client, space, principal });
// <OrdoProvider client={hooks} device={device}> in @hominemai/ordo-react
```

| Option | Default | What it does |
|---|---|---|
| `backend` | IndexedDB under `store` | Where rows are kept. `MemoryBackend` in tests. |
| `store` | `'ordo'` | The IndexedDB database name. Pass your old name when moving onto `openDevice`, or the device syncs again from empty. |
| `manager` | `{}` | `SyncManagerOptions`: backoff, `publishMs`, `online`. |
| `idleMs` | `15_000` | How long a sync stays open after its last screen goes. `Infinity` keeps it open. |
| `eraseAfterMs` | 7 days | Rows held only by syncs unused this long are erased when the device opens. `Infinity` keeps them. |
| `tabs` | `'share'` when `openDevice` opens IndexedDB, `'own'` with a `backend` | Whether the tabs on one replica share one set of feeds. |

What it answers:

- `reconnect()` reopens every feed on the same manager, each only after its
  old one ended, and resolves when each has answered. Overlapping calls share
  one promise. A feed refused for good stays stopped.
- `nudge()` cuts a wait before a retry short and reopens every feed now.
- `purge()` stops every feed, waits for each to end, then erases what is
  held. Sign-out calls it.
- `close()` stops every feed and keeps the rows.
- `replica`, `manager` and the three refiners, for an app that wants them.

**A sync nobody uses closes after 15 s** (owner, 2026-10-21). The hooks in
`@hominemai/ordo-react` count the screens using each op. When the last one
has been gone for `idleMs`, they call `manager.idle(key)`. That ends the feed
and takes the op out of the view, so `complete` and the common position stop
waiting on it. Unlike `drop`, it keeps the rows and the stored result hash:
another tab on the same replica may still show them. Declaring the op again
resumes on the hash, which costs one server read and moves no rows when
nothing changed.

**Rows nobody used for 7 days are erased** (owner, 2026-10-21). The manager
notes each sync's last-used time in the replica, sealed: when it is declared,
when it goes idle, and every `touchEveryMs` (1 hour) while it runs. When
`openDevice` opens, before any feed, `replica.eraseUnused(now - eraseAfterMs)`
lets go of every sync unused for longer, as `drop` would, and forgets its
result hash. A row another sync holds stays. It runs under the replica lock.
A sync with no last-used time, kept before this version, is given "now" on the first open (once per replica), so it is erased after 7 unused days like the rest.

**One set of feeds however many tabs** (`tabs: 'share'`). The tabs of one
person in one space elect a leader: it holds the Web Lock
`ordo-sync-leader:<store>:<ns>`, and the others wait on it. Only the leader
opens feeds and writes frames. After each frame it posts the delta on
`BroadcastChannel('ordo-sync:<store>:<ns>')`, and every other tab's manager
applies it to memory, so screens in every tab match. A follower's `sync()`
asks the leader for the op. The leader keeps a feed while any tab uses it
and idles it `idleMs` after the last lets go. Each tab holds its own lock for
its life, so the leader learns when one dies. When the leader closes, a
waiting tab takes over and resumes every feed on its stored hash. A hidden
leader gives way when a visible tab appears. `purge()` in any tab poisons the
shared replica, and every tab empties at once. Without `BroadcastChannel` or
`navigator.locks`, each tab leads itself. `device.leading` says which this
tab is. A follower's view carries each op's state from the leader's frames;
it does not see the leader's retry count or network error.

**No polling.** The device reloads nothing when a tab regains focus, and runs
no timer (owner, 2026-10-21). The server pushes each change down the open
feed, a tenant made after the server booted included. The only event it
listens to is `online`.

## Staying connected

`SyncManager` keeps each feed open and opens it again when it ends. It also
reconnects by itself when the browser comes back online (the `online` event),
so an app needs no timer and no focus handler to catch up.

- A door that answers `server_busy`, `tenant_not_ready`,
  `serialization_conflict` or `internal` is asked again after a backoff
  (250 ms doubling to 30 s). The door's `Retry-After` is the least it waits.
  The op does not stop.

- `manager.reconnect()` reopens every feed in place and resolves when each
  has answered (see "A ready-made device").
- `manager.nudge()` reconnects every feed now. A feed waiting to retry stops
  waiting, and one that is open is closed and opened again, which runs a
  fresh load. The rows already held stay until that load says otherwise.
  Call it when the app has its own reason to think the connection is stale.
  A feed the server refused for good stays stopped.
- `SyncManagerOptions.online` is where the `online` event is heard. It
  defaults to the page. Pass `null` to turn it off, or an `EventTarget`.
- A tenant made after the server booted needs no reload. The server listens
  on a tenant's database when a feed opens for it, so a change made after the
  feed opened is pushed.
- `ManagedOp.network` holds the `OrdoNetworkError` of the last connection that
  got no answer, and is `null` once a frame arrives. A screen can say "offline"
  or "the server is down" from it.

## Two tabs on one replica

A replica write reads, decides and writes in separate IndexedDB
transactions. Two tabs on one replica could each decide from the same old
record, and the later write would drop the earlier one's record of which ops
hold a row. Each write now runs under a Web Lock named for the replica
(`navigator.locks.request('ordo-replica:<namespace>', ...)`), so a second tab
waits its turn. Where there is no `navigator.locks` (Node, an old browser) a
write runs as before, one at a time within one tab.

With `openDevice` and `tabs: 'share'`, only the leader tab writes frames
(see "A ready-made device"). The lock still guards purge, the moment two
tabs both lead during a handover, and tabs that lead themselves.

## Sign-in, recovery and sharing

These functions are exported from the package root. Each takes a
`PasskeyEndpoint` (the door's base URL, and optionally a WebAuthn container
for tests). `howto-sign-in.md` and `howto-sharing.md` show them in use.

- **Passkeys.** `registerPasskey` signs a new person up, `signInWithPasskey`
  signs them in, and `stepUp` asks for a fresh touch before a sensitive call.
  `toBase64Url` and `fromBase64Url` are the encoding helpers.
- **Emailed code.** `beginEmailSignIn` sends the code and
  `completeEmailSignIn` redeems it.
- **Adding a first passkey.** `claimPasskey(endpoint, token, claim)` adds a new passkey inside a claim any sign-in opened.
- **Linking by claim.** `beginClaim` proves a sign-in the account already
  holds and opens a claim. `completeClaim` proves the new sign-in and links it.
  Pass the claim string to `beginEmailSignIn` as `claim` to claim an address.
  `emailCodeCeremony`, `handoffCeremony` and `passwordCeremony` build the
  proofs. The app developer switches it off with `ORDO_AUTH_LINKING`.
- **Second step and passwords.** `signInWithPassword`, `signInWithPasskeyStep`
  and `completeSecondStep` carry a sign-in through a second step.
  `reauthenticateWithPassword` and `reauthenticateWithPasskey` refresh
  step-up. `listFactors`, `removeFactor`, `setSecondStepLevel`,
  `beginTotpEnrolment`, `confirmTotpEnrolment` and `enrolSecurityKey` manage
  the factors a person holds.
- **Devices and recovery.** `listAuthenticators`, `linkPasskey`,
  `removePasskey` and `endEverySession` manage a person's keys and sessions.
  `issueRecoveryCodes` and `redeemRecoveryCode` run code recovery.
  `recoverWithTicket` redeems a recovery ticket, and `contestRecovery` and
  `acknowledgeRecovery` answer the notice a recovery leaves behind.
- **Sharing and invitations.** `peekShare`, `acceptShare`, `declineShare` and
  `leaveShare` handle a share. `peekInvitation` and `declineInvitation` handle
  an invitation to a tenant. `listSpaces` lists the tenants a person can
  enter. `openWindow` and `windowFeed` read a window, and `openLink` and
  `linkFeed` read a share link, as a guest.

## A person's own tenant

On a deployment that gives each person a tenant of their own, sign-up
answers at once and the tenant is made after it (`docs/howto-sign-in.md`
section 4).

- `tenantStatus(endpoint, token)` reads `GET /auth/provisioning` once. While
  it is being made, `state` is `provisioning` with a `step`, `attempts` and
  `stuck`. After the deployment's `ORDO_PROVISION_STUCK_SECS` (default 300)
  `stuck` is `true` and `reason` says so in words for the person. The server
  keeps trying.
- `watchTenant(endpoint, token, onStatus, options)` is the tenant status
  hook. It calls `onStatus` on each change, has no time limit, reads every
  10 s once stuck (`stuckPauseMs`), and reads on through failed reads
  (`onError`). `ready` resolves with the space id. `stop()` ends it.
- `waitForTenant` is the same with a time limit (two minutes by default),
  for a sign-up screen that would rather give up.
- `retryTenant` (`POST /auth/provisioning`) starts a setup again after a
  failure.

What the person sees is the app developer's choice. In React,
`@hominemai/ordo-react` has `useTenantStatus` and `TenantGate`.

## Delegation: an agent asks, the person decides

An agent can ask a person for a narrow token that lasts hours. The person
reads what it would let the agent do, then approves or declines.

The agent's side needs no token:

```ts
const asked = await requestDelegation(endpoint, { space, agent, person, ops: ['notes.list'], hours: 4 });
// Show asked.request to the person. Keep asked.binder to yourself.
const granted = await waitForDelegation(endpoint, asked, { signal });
// granted.token is the agent's token for granted.ops.
```

`redeemDelegation` is one try and answers `granted` or `waiting`.
`waitForDelegation` polls it every 2 seconds by default. A refusal throws a
`DelegationError` with `.failure` (`refused`, `declined`, `expired`,
`redeemed`, `not_found`) and `.code`. A `refused` message names the setting
the app developer set, such as `ORDO_DELEGATION_MAX_HOURS`.

The person's side takes an `OrdoClient` signed in as that person:
`reviewDelegation`, `approveDelegation`, `declineDelegation`, `myDelegations`
and `revokeDelegation`. Show every part of the disclosure `reviewDelegation`
returns. Approving needs a fresh passkey touch unless the app developer
exempted it, so call `stepUp` first and build the client from the session it
returns. Without it the door answers `access_denied`.

## Files

Upload, show and download over the blob door, with the client's own space
and token. `docs/howto-files.md` has the whole story.

```ts
await uploadFile(client, { entity: 'attachments', field: 'file', id }, file, {
  onProgress: (p) => (bar.value = p.fraction ?? 0),
  signal,
});
const { type, size } = await fileInfo(client, handle); // no bytes read
const shown = await fileUrl(client, handle);           // shown.url for <img src>
shown.release();
await downloadFile(client, handle, 'report.pdf');
```

- `readFile` gives the bytes as a `Blob`. `fileUrl` gives an object URL,
  because a page cannot put a token in `<img src>`. Call `release()` when done.
- Errors are the ones you already catch: `OrdoRefusal` for the door,
  `OrdoLocalRefusal` (`bound_violation`) for a file over 100 MiB,
  `OrdoNetworkError` for no answer, `AbortError` for your own abort.
- Progress uses `XMLHttpRequest`. There is no deadline, because a large
  upload needs longer than any fixed one. Pass a `signal`.
- The first argument is a client, or any object with `fileDoor()`.

## Backend functions

A function the program declares runs at `POST /fn/<name>` as the signed-in
person. `docs/howto-functions.md` has the whole story. The client calls it by
name:

```ts
const { result } = await client.callFunction<{ title: string }>('suggestTitle', {
  text: note.body,
});
```

- The parameters are plain JSON. A name that is not an identifier, or a
  parameter that is not JSON, is refused before the wire as an
  `OrdoLocalRefusal`.
- `result` is the JSON the function returned. `replayed` is true when the
  door answered a repeated `idempotencyKey` with the first answer.
- No `Idempotency-Key` is sent unless you pass `idempotencyKey`.
- Errors are the ones you already catch. A refusal is an `OrdoRefusal` with
  one of `access_denied`, `space_paused`, `function_error`, `function_timeout`,
  `function_oom`, `function_busy` or `egress_refused`. The message names what
  to change, such as `ordo secret set llmKey`. `function_busy` carries the
  door's `Retry-After` in `retryAfterMs`.
- A busy door is retried under `busyRetry`, as for an op.
- The interface descriptor does not list functions, so there is no generated
  function per declared function. The name and the result type are yours.
- `@hominemai/ordo-react` has `useFunction(name)`, which returns `call`,
  `busy` and `error`.

## What is deliberately not here

- **`.live()`**: the SSE verb. `call.terminal` stamps `live` on 76 reads in
  the market catalog and this package emits no method for it.
  `api-engine-design.md` §6 settles the transport, but the descriptor carries
  no log position on the read path, so a subscription's first frame and its
  invalidations cannot be ordered against each other by anything the client
  can see. A `.live()` that reconnected and refetched would ship the exact
  torn read §2's settled design warns about. Same gap `@hominemai/ordo-react` named when
  it deferred live; one answer will cover both.
- **Optimistic apply and typed rollback**, which belong on top of a conflict
  machine and not underneath it.
- **A `cas_miss` conflict machine.** `@hominemai/ordo-react` has one (refetch through
  the catalog's own detail op, exactly one request, a retry re-asserting the
  stale rev refused locally). This package raises the typed refusal and stops
  there: the repair is a UI decision, and a client library that made it would
  be making it for every caller.

## One request helper, one retry policy, one error shape

Every call the SDK makes to the door goes through `doorRequest`
(`src/core/door.ts`): ops, the sync feed, files, sign-in and account calls,
the person's own download, links. It sets the deadline, names a call that
got no answer (`OrdoNetworkError`), and sends the call again under the one
retry policy above. The classes it retries are `RETRY_CLASSES`. A feed is
opened once: the sync manager reconnects on the same classes, with the
door's `Retry-After` as a floor under its backoff (`reconnectDelayMs`).

Every refusal is an `OrdoRefusal`:

- `class`: a code this client knows, or `internal` for one it does not.
- `code`: the door's code verbatim. It differs from `class` only when the
  door is newer than this client.
- `status`: the HTTP status, 0 for a refusal raised before the wire.
- `retryAfterMs`: the door's `Retry-After`, when it sent one.
- `disposition` (what to do) and `retryable` (nothing ran; ask again).
- `detail`, also readable as `refusal` (ordo-react's name).

A module adds a subclass only where its callers switch on something of
their own: `PasskeyError`, `ServerBusyError`, `AccessError`,
`ConnectionError`, `DelegationError`, `TenantError` and `AccountEmailError`
are all `OrdoRefusal`s. A call that got no answer is an `OrdoNetworkError`.

The codes are two closed lists, `REFUSAL_CLASSES` (the op door) and
`EDGE_CODES` (sign-in, accounts, delegation, links, shares).
`crates/ordo-api/tests/client_classes.rs` holds them equal to the server's
`ErrorCode` and `EdgeCode`. `test/one-door.test.ts` fails when a file in
ordo-sdk, ordo-react or ordo-cdk calls `fetch` outside `door.ts`, holds a
second retry loop or backoff, or declares an error class that is not an
`OrdoRefusal` without saying why.

`@hominemai/ordo-react` imports all of this and carries no copy.

**Changed in 0.4.0.**

- The auth errors above extend `OrdoRefusal`. Their `failure` is unchanged.
  `code` and `status` are now always set: `code` is the door's code, else the
  failure's word; `status` is 0 when the door did not answer.
  `DelegationError.code` and `.status` were `null` in that case.
- `RefusalDetail` has `code`. A non-2xx without an Ordo envelope says so in
  its message, where it said `HTTP <status>`.
- An op refusal carries the door's `Retry-After` as `retryAfterMs`.
- File calls and the download retry `server_busy` under `busyRetry`.
- `GuestFeed.token` may answer the string as well as a promise.
- `checkRefine` takes a descriptor op as well as a spec. `beginAccountEmail`,
  `confirmAccountEmail` and `removeAccountEmail` take ordo-react 0.3's form
  too, deprecated.

## Regenerating

```
cargo run -p ordo-cli --bin ordo -- gen ts <dir|file.ordo>
```

The checked-in `src/generated/` is the market conformance fixture's, so the
package has something real to typecheck and test against.
`crates/ordo-gen/tests/generated_files.rs` asserts it is byte-identical to a
fresh run, and that no hand-written file is hiding in the generated
directory.

```
npm run check   # tsc + vitest
```
