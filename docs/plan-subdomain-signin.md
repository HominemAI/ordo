# One sign-in across subdomains: the hand-off

Owner rulings, 2026-10-09, board task `subdomain-signin`, all three "ok":

1. The core site (`login.example.com`) signs the person in with any method, then sends them back with a one-time code. The subdomain (`app.example.com`) swaps the code for its own session. No shared cookie. No token in a URL.
2. Return addresses match by origin, with any path, so a person lands back where they were.
3. Wildcard origins such as `https://*.example.com` are allowed when switched on. Off by default.

Standing principles: secure by default, looser modes opt-in, and every refusal a setting can fix names the setting and the value. Nothing reads a client IP address.

## On one screen

| Step | Who | What |
|---|---|---|
| 1 | subdomain page | Makes a random binder. Keeps it in `sessionStorage`. Sends the person to the core site with `return_to` and the binder's SHA-256 (`binder_hash`). |
| 2 | core site | Signs the person in, if they are not already. Calls `POST /auth/handoff` with its bearer token, `return_to` and `binder_hash`. |
| 3 | door | Checks the session is live and `return_to` is on an allowed origin. Files a one-time code. Answers `redirect`: `return_to` with `handoff_code=<code>` added. |
| 4 | core site | Sends the browser to `redirect`. |
| 5 | subdomain page | Calls `POST /auth/handoff/redeem` with the code and the binder it kept. Gets its own session. |

## What the code is bound to

- **The person's session.** The code names the principal, the session's start, its second step, its client and its way in. Issuing it needs a live session: not ended, not ended by "sign out everywhere", not a capability or delegated token, not a deleted account.
- **The return origin.** `return_to` must be on an origin `ORDO_AUTH_RETURN_URLS` allows. If the redeem request carries an `Origin` header, it must be that origin.
- **The binder.** The row is filed under the binder's hash. The redeem presents the binder itself. A code alone is useless. A wrong binder leaves the code where it is, so a stolen code cannot be burned by its thief.
- **A short life.** `ORDO_AUTH_HANDOFF_SECS`, default 60, at most 300.
- **One use.** The row is taken by one delete, as every ceremony row is (`ordo_identity::ceremony`).

The code travels in a URL. It is not a token: it cannot be presented at the door, and it cannot be redeemed without the binder that only the starting page holds. It is the same shape as the OAuth2 return leg's `handoff` code (`plan-auth-federated.md` §3.2).

## A new route, not `/auth/complete`

`/auth/complete` finishes a ceremony that proves a first factor. It resolves the method in the registry, may onboard a newcomer, counts sign-ups, spends invitations, runs the second-step gate and admits to a tenant. A hand-off proves none of those things. The person already has a session; the code only carries it to another page. Routing it through `complete` would put a path that cannot prove a factor inside the code that mints principals. So it is its own pair:

| Route | Takes | Answers |
|---|---|---|
| `POST /auth/handoff` | `Authorization: Bearer <session>`, `{return_to, binder_hash}` | `{redirect, expires_at}` |
| `POST /auth/handoff/redeem` | `{code, binder}` | `{token, via, expires_at, session_expires_at}` |

Neither can create a principal, link an authenticator or join a tenant.

## What the new session carries

| Claim | Carried? | Why |
|---|---|---|
| principal | yes | The same person. |
| session start | yes | The new session ends at the old one's ceiling. A hand-off cannot extend how long a sign-in lasts. |
| second step | yes | It is a fact about the sign-in, as `refresh` carries it. Without it a tenant that requires a second step would refuse every subdomain. |
| client | yes | No wider than the session that issued it. |
| presence | **no** | Presence says a person touched an authenticator, on that page, at that moment. The subdomain page did not. A step-up there runs its own passkey ceremony, which works on any subdomain under `ORDO_APEX_DOMAIN`. |
| `via` | `handoff` | So an audit, and the page, can tell this session came by hand-off. |

The new session has its own id. Logging out on the core site does not end it; "sign out everywhere" does.

## Return addresses by origin

`ORDO_AUTH_RETURN_URLS` entries are now read as origins: scheme, host and port. Any path on a listed origin is accepted. An entry written as a full address before this change still works, and so does every other path on its origin. A fragment is still refused. The same list serves the OAuth2 return leg and the hand-off. It is optional unless an outside provider is configured; with it unset, every hand-off is refused, and the security stream says to set it.

A URL that could be read two ways is refused: user-info (`https://app.example.com@evil.com`), a backslash, a percent sign in the host, a trailing dot, whitespace.

## Wildcards

`ORDO_WILDCARD_ORIGINS=on` lets an entry be `https://*.example.com`. The `*` is one whole label. At least two labels follow it, so `*.com` is refused. It matches `https://app.example.com` and never `https://example.com`, `https://a.b.example.com` or `https://x.example.com.evil.com`. https only.

With the setting off, a wildcard entry refuses the boot, and the refusal says `ORDO_WILDCARD_ORIGINS=on`. A listed wildcard that silently matched nothing would be found by a person failing to sign in.

One setting covers three lists:

| List | Safe with a wildcard? |
|---|---|
| `ORDO_AUTH_RETURN_URLS` | Yes. A code sent to a page under the domain is useless without the binder, which only the starting page holds. |
| `ORDO_AUTH_ORIGINS` | Yes. The wildcard must sit under `ORDO_APEX_DOMAIN`. Every page under the relying party id may already use its passkeys in the browser. |
| `ORDO_CORS_ORIGINS` | Yes, with care. The answer still names the one origin that asked, never `*`, with `Vary: Origin`. The door reads no cookie and never sends `Access-Control-Allow-Credentials`, so a page gains nothing it does not already hold: it must bring a bearer token of its own. The bare `*` stays refused. |

The cost: a forgotten DNS record on a subdomain lets whoever claims it serve a page that the lists accept. That page still holds no binder and no token. The app developer who turns the setting on accepts that every subdomain is theirs to serve.

## Security events

- `session-handoff issued principal=... origin=... via=...` when a code is filed.
- `session-handoff redeemed principal=... origin=...` and `authenticated` when one is swapped.
- Every refusal is `authentication-failed` with a fixed reason word. A return refused for its origin reads `return-not-allowed: add the return origin to ORDO_AUTH_RETURN_URLS ...`.

The caller is told one thing: `authentication_failed`. A store that cannot be read is `credential_store_unavailable`, as elsewhere.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `ORDO_AUTH_RETURN_URLS` | none | Allowed return origins; any path on one. |
| `ORDO_AUTH_HANDOFF_SECS` | 60 | How long a hand-off code lives, 1 to 300. |
| `ORDO_WILDCARD_ORIGINS` | off | `on` allows `https://*.<domain>` in the three origin lists. |

## The SDK

- Subdomain side: `startSubdomainSignIn` makes the binder, keeps it, and gives the address to send the person to. `finishSubdomainSignIn` reads `handoff_code` from the address, redeems it with the binder, and removes both.
- Core side: `issueHandoff` calls `POST /auth/handoff` with the signed-in client's token and returns the address to send the browser to.

## Tests

`crates/ordo-host/tests/auth_handoff.rs`, `crates/ordo-identity/src/origin.rs`, `crates/ordo-api/src/cors.rs`, and the SDK's `handoff.test.ts`.
