# How to make data public

For app developers. Phases 1 and 2 are built: public reads, the cache and the
CDN headers (2026-10-10), then public writes, per-tenant CORS, proxy mode and
the public counters (phase 2, 2026-10-11). The design is `design-public-path.md`. The spelling is in
`definition-language.md`, under "`public`".

## What public means

Most of Ordo refuses a call with no token. `public` is how you say, on
purpose, that some data may be read by anyone.

- A visitor is a person with no token, such as someone on your pricing page.
- A visitor's call runs as the reserved principal `$anonymous`.
- It runs in your deployment's own space (`system`), and nowhere else.
- Nothing is public unless you wrote `public` next to it.

## How to declare it

`public` is a reserved word. It has four places.

| Where | Means |
|---|---|
| End of a field line | This field may be read by a public op. |
| An entity line of its own: `public` | Every field of this entity. |
| An entity line of its own: `public*` | This entity and every entity it owns. |
| After an op's name: `op pricing public =` | This op runs on the public path. |

Rules:

- `public*` follows `owned` refs only. A plain ref does not carry it.
- A child's own `public` or `public*` line replaces the one it inherits.
- A public op reads public fields only. That includes conditions and sort keys.
- A file on a public row is public. A private row's file is not.
- The CDK spells it `t.text().public()`, entity `.public()` and `.publicAll()`,
  and op `.public()`.
- `ordo check` prints `(public: anyone may call it, with no token)` beside a public op.

## What the compiler refuses

| Class | Meaning |
|---|---|
| `public_sealed` | A sealed field cannot be public. |
| `public_private` | A `blob private` field cannot be public. |
| `public_guarded` | The entity has `access read` or `acl`. Put public rows in an unguarded entity. |
| `public_reads_private` | A public op reads a non-public field, or the entity has no public field. |
| `public_composes_private` | A public op composes an op that is not public. |
| `public_caller` | `$caller` in a public op or a public write. A public op has no caller. |
| `public_write_not_create` | A statement in a public write that updates, converges or deletes. A create on a unique field is allowed only as a create-once (`rev: null`). |
| `public_write_guarded` | A public write into an entity that has a guard or an `acl`. |
| `public_write_ref` | A public write sets a ref from a visitor's value to an entity with no public field. The answer would show whether that row exists. |

A condition on a private field would leak it a bit at a time. That is why a
condition counts as a read.

## A complete program

A pricing page. The margin stays private.

```
domain shopfront @ 1.0.0

entity plans {
  name:   text     public
  price:  money(2) public
  margin: money(2)                 # private: no public op may read it
}

op pricing public =
  plans(price > 0)[name, price]
  sort name
  take 50

op margins =                       # not public: members only
  plans[name, margin]
  sort name
  take 50

op addPlan($name: text(80), $price: money(2), $margin: money(2)) =
  set plans { name: $name, price: $price, margin: $margin }
```

A visitor reads the prices with no token:

```
curl -si https://<door>/op/pricing
```

The answer carries the cache headers below. Calling `margins` with no token,
or `pricing` in another space, gets the door's usual `access_denied`.

## What a visitor can and cannot reach

Can:

- Call a public read with no token and no space header.
- Page through it with `$page`.
- Fetch `GET /blob/{handle}` with no token, if the file's field is public.

Cannot:

- Call any op that is not public.
- Name any other space.
- Use `$refine`. It is refused as `invalid_refinement`.
- Open a live stream. A tokenless `?live=1` answers `not_live`.

A member with a token can call a public op too. It runs on the ordinary path.

A public read writes no audit row, walks no guards and looks up no membership.
It takes no quota. The compiler's checks are what make that safe.

## What bounds public reads

A visitor has no token, so nothing ties a call to a person. Two things bound
the load:

- **The cache.** A repeated call is a hit and reads no database.
- **The miss cap.** Each public op may send `ORDO_PUBLIC_MISSES_PER_MINUTE`
  misses to the database per minute, on each node (600 by default). Past it,
  a miss is refused `429 quota_exceeded`, and the message names the setting.
  Hits are still served.

Each distinct parameter value is its own cache entry. A visitor who varies a
parameter makes every call a miss, so the cap is what holds the database.
With `ORDO_PUBLIC_CACHE_BYTES=0` every call is a miss.

Nothing refuses one visitor apart from another. Ordo does not rely on client
addresses by default. Proxy mode exists and only slows a busy visitor, it never
refuses (see "Proxy mode"). **Put a CDN in front of a public site.** Its cache
and its page protection (Cloudflare's, for example) stop most floods first.
Rely on those first. Proxy mode is the line after them.

## Public writes

A public write lets a visitor create a row, such as a contact form. It runs as
`$anonymous` in the deployment's space. It may write into a private entity.

Public writes land in the deployment's space. Writes into one tenant's own
space are filed for later (board task `public-writes-tenant-space`).

```
domain marketing @ 1.0.0

entity enquiries {
  name:    text
  email:   text
  message: text
}

op contact($name: text(120), $email: text(254), $message: text(4000)) public =
  set enquiries { name: $name, email: $email, message: $message }
```

The rules:

- Every statement must be a create.
- No `id`, no condition, no `del`.
- A unique field in the braces needs `rev: null`. See "Keyed creates" below.
- A file field cannot be set. A blob is never an op parameter.
- State a bound on every text parameter.

The compiler refuses the rest by class. See the table above.

What happens at the door, in order:

1. The hourly cap for this op is read. A full hour is refused before the provider is asked.
2. The human check runs.
3. The cap is taken. Only writes that passed the check use the hour.
4. The ordinary write pipeline runs as `$anonymous`: bind, idempotency, execute, command log.
5. Public answers that read this entity are dropped.

The command log records the actor `$anonymous` and no address.

### Keyed creates

A create may be keyed on a unique field, such as a newsletter email:

```
entity subscribers {
  email: text(200) unique
}

op subscribe($email: text(200)) public =
  set subscribers { email: $email, rev: null }
```

The visitor gets the same answer whether the email is new or already on the
list. Nothing is written for a duplicate, and the existing row is not changed.
The answer names no id and carries no rows:

```
{"op":"subscribe","version":"1.0.0","mode":"creates","rows":[]}
```

Both cases give the same status, the same body and the same headers. A
duplicate is counted in monitoring as `duplicate`, which only the deployment
owner sees. Without `rev: null` the statement would converge onto the existing
row and change it, so the compiler still refuses that.

The answer keeps only the id of each row:

```
{"op":"contact","version":"1.0.0","mode":"creates","rows":[{"created":"<id>"}]}
```

It carries `Cache-Control: no-store` and no signed head headers. It is never cached.

### The human check

A public write needs a token from a human-check widget. The check is on by default.

1. Pick a provider: Turnstile (the default), hCaptcha or reCAPTCHA. Set
   `ORDO_PUBLIC_HUMAN_PROVIDER` for the other two.
2. Put the widget's site key in your page. The widget gives the page a token.
3. Save the provider's secret key in a file on the server.
   - Sign-in already uses Turnstile with `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE`?
     Public writes use the same secret, address and timeout. Set nothing more.
   - Otherwise, or for a different key or provider, set
     `ORDO_PUBLIC_HUMAN_SECRET_FILE` to its path. It overrides the sign-in one.
4. The page sends the token in the header `X-Ordo-Human` (up to 4096 characters).

```
curl -si -X POST https://<door>/op/contact \
  -H "X-Ordo-Human: $WIDGET_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name":"Ada","email":"ada@example.com","message":"Hello"}'
```

The server checks the token with the provider by one form POST through
`ordo-egress`. The visitor's address is not sent to the provider.

A failed check answers `403 human_check_failed`. The causes:

- No header.
- The provider refused the token. A token is good for one write.
- The provider could not be reached. The check fails closed, and the log says so.
- No provider is configured. The message names
  `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE`, `ORDO_PUBLIC_HUMAN_SECRET_FILE` and
  `ORDO_PUBLIC_HUMAN_CHECK=off`.

`ORDO_PUBLIC_HUMAN_CHECK=off` turns the check off. Do that only on purpose.

### The hourly cap

Each public write op may make `ORDO_PUBLIC_WRITES_PER_HOUR` writes an hour on
each node (10000 by default). Over it, the answer is `429 quota_exceeded`, and
the message names the setting. 0 turns the cap off.

## What is cached, and when it is dropped

Each node keeps answers in memory. The key is the program version, the op,
the sorted bound parameters and the page token.

An answer is dropped:

- Exactly when a write commits to an entity the op reads.
- All at once on a deploy.
- All at once when the database listener is lost.

While the listener is lost, nothing is stored, and answers say `s-maxage=0`.
Past the byte bound the least recently used answers go first.

## The headers, and what a CDN does with them

| Header | Value |
|---|---|
| `Cache-Control` | `public, max-age=0, s-maxage=60, stale-while-revalidate=300` |
| `ETag` | sha256 of the answer. `If-None-Match` answers `304`. |
| `Cache-Tag` | `op:<name>, entity:<e>` for each entity the op reads |
| `Surrogate-Key` | the same tags, separated by spaces |
| `Vary` | `Accept` |
| `X-Ordo-Cache` | `hit` or `miss` |

- `max-age=0` keeps browsers asking. `s-maxage` lets a CDN keep the answer.
- `stale-while-revalidate` lets the CDN serve an old answer while it fetches a new one.
- The tags let you purge one op or one entity at the CDN.
- Set `ORDO_PUBLIC_CDN_PURGE_URL` and Ordo posts `{"tags":[...]}` when answers are dropped.
- The purge call uses https, goes through `ordo-egress`, and carries a bearer secret read from a file.
- A purge endpoint on a private address is refused. The log line names `ORDO_EGRESS_PRIVATE_HOSTS` and the host to add.

## Settings

The deployment owner sets these in the environment. A value that is not a
whole number in its range refuses the boot, and names the variable, the range
and the default.

| Variable | Default | Means |
|---|---|---|
| `ORDO_PUBLIC_CACHE_BYTES` | 268435456 (256 MiB) | Cache size per node, 0 to 64 GiB. 0 turns the cache off. |
| `ORDO_PUBLIC_CDN_S_MAXAGE` | 60 | Seconds a CDN may keep an answer, 0 to 604800 (7 days). |
| `ORDO_PUBLIC_CDN_SWR` | 300 | Seconds a CDN may serve a stale answer, 0 to 604800. |
| `ORDO_PUBLIC_MISSES_PER_MINUTE` | 600 | Misses one public op may send to the database per minute, per node, 0 to 1000000. 0 turns the cap off. |
| `ORDO_PUBLIC_CDN_PURGE_URL` | unset | Where to post purges. Unset posts nothing. https only. |
| `ORDO_PUBLIC_CDN_PURGE_SECRET_FILE` | unset | A file holding the bearer secret. |
| `ORDO_PUBLIC_CDN_PURGE_TIMEOUT_SECS` | 5 | How long one purge call may take, 1 to 60. A timeout is logged naming this setting. |

| `ORDO_PUBLIC_WRITES_PER_HOUR` | 10000 | Writes one public write op may make per hour, per node, 0 to 1000000. 0 turns the cap off. |
| `ORDO_PUBLIC_HUMAN_CHECK` | on | `on` or `off`. Whether a public write needs a human check the provider confirms. |
| `ORDO_PUBLIC_HUMAN_PROVIDER` | turnstile | `turnstile`, `hcaptcha` or `recaptcha`. |
| `ORDO_PUBLIC_HUMAN_SECRET_FILE` | `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE` | A file holding the provider's secret key. Falls back to the sign-in one for Turnstile. Neither set, with the check on, public writes are refused by name. |
| `ORDO_PUBLIC_HUMAN_VERIFY_URL` | `ORDO_AUTH_HUMAN_CHECK_VERIFY_URL`, then the provider's | Where tokens are verified. https only. |
| `ORDO_PUBLIC_HUMAN_VERIFY_SECS` | `ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS`, then 5 | How long one verification may take, 1 to 30. |
| `ORDO_CORS_TENANT_ORIGINS_MAX` | 0 | How many origins one tenant may list, 0 to 1000. 0 lets no tenant list any. |
| `ORDO_PROXY_HEADER` | unset | The header naming the visitor's address, such as `CF-Connecting-IP` or `X-Forwarded-For`. Unset: proxy mode is off. |
| `ORDO_PROXY_TRUSTED` | unset | The proxies' addresses: a comma list of addresses or CIDR blocks. |
| `ORDO_PROXY_SLOW_AFTER` | 120 | Tokenless requests one visitor may make per minute before the delay starts, 0 to 1000000. 0 never slows. |
| `ORDO_PROXY_SLOW_MS` | 250 | The delay in ms for each further `SLOW_AFTER` requests in the minute, 1 to 60000. |
| `ORDO_PROXY_SLOW_MAX_MS` | 5000 | The longest delay in ms, 0 to 60000. |
| `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR` | off | Sign-ups one address may make per hour before its next is slowed. Proxy mode only. |
| `ORDO_SHARE_LINK_MISS_LIMIT` | off | Wrong link secrets one address may send per minute before its next open is slowed. Proxy mode only. |

`ORDO_PUBLIC_CDN_PURGE_SECRET` and `ORDO_PUBLIC_HUMAN_SECRET` are refused. A
secret does not belong in the environment. Use the file. These move to the settings mechanism (24a) when it exists.

## Per-tenant CORS

A tenant's own site may need to call the door. The tenant owner lists its
origins with platform ops:

- `std.space.setOrigins` takes `origins`, a comma list of exact origins.
- `std.space.origins` answers `{origins, max}`.

The rules:

- Owner only. Like other platform ops, they ask for recent presence by default.
- Off until the deployment owner sets `ORDO_CORS_TENANT_ORIGINS_MAX`. Until
  then the refusal is `tenant_refused`, naming a setting such as
  `ORDO_CORS_TENANT_ORIGINS_MAX=10`.
- Exact origins only. `*` and `https://*.x` are refused on a tenant list,
  whatever `ORDO_WILDCARD_ORIGINS` says.
- The lists are stored in `$spaceorigins` in the control database. Other nodes
  re-read them every `ORDO_ACCESS_REFRESH_SECS`.

The list for a space is `ORDO_CORS_ORIGINS` plus that tenant's list.

- A preflight answers the union, and tells an origin only about itself.
- A real response answers the exact list for the space named in `X-Ordo-Space`.
- A space named only in a POST body gets the deployment list alone.

Public answers (reads and writes, success or refusal) carry
`Access-Control-Allow-Origin: *` whatever the lists say. They expose
`X-Ordo-Cache` and `ETag`. A preflight to `/op/...` that does not ask to send
`Authorization` is answered `*`, and allows `Content-Type`, `Idempotency-Key`,
`X-Ordo-Page`, `X-Ordo-Version` and `X-Ordo-Human`.

## Proxy mode

Off by default. Until both proxy variables are set, no decision uses a client
address.

To turn it on, set `ORDO_PROXY_HEADER` and `ORDO_PROXY_TRUSTED` together. One
alone refuses the boot, naming the other. The boot prints one line starting
`proxy mode: on`.

- A tokenless request to `/op/` or `/blob/` (not OPTIONS) from a visitor over
  `ORDO_PROXY_SLOW_AFTER` in the current minute waits before it runs.
- The wait is `ORDO_PROXY_SLOW_MS` for each further `SLOW_AFTER` requests,
  capped at `ORDO_PROXY_SLOW_MAX_MS`.
- It never refuses.
- Callers with a token are not paced. They have quotas.
- The header is believed only when the socket peer is a trusted proxy. From
  anyone else, the socket address is the visitor's.
- In a list such as `X-Forwarded-For`, the visitor is the right-most address
  that is not a trusted proxy.
- Two more address limits, off by default, slow down and never refuse:
  sign-ups per hour (`ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR`) and wrong
  share-link secrets per minute (`ORDO_SHARE_LINK_MISS_LIMIT`). Set without
  proxy mode, each has no effect, and the boot says so.
- No address is logged, or kept past its window (an hour for sign-ups, a
  minute otherwise).

The sign-up per-address limit and the share-link miss limit are still ignored.
They would refuse by address.

## Monitoring

Every public request is counted in `ordo_public_requests_total{op, kind, outcome}`.

- `kind` is `read`, `write` or `door`.
- `outcome` is `hit`, `miss`, `written`, `limited`, `human_check_failed`,
  `failed` or `refused`.
- A tokenless call the door refused for a non-public or unknown op is counted
  under op `-`, never under the name asked. A probe cannot mint series.

`ordo_public_slowed_total` counts requests that proxy mode slowed.

## How to put rows in the deployment's space

Public data lives in the deployment's space, `system`. A member of that space,
for example the deployment owner when `ORDO_SPACE` is the system space, calls
an ordinary write op with `X-Ordo-Space: system`:

```
curl -si -X POST https://<door>/op/addPlan \
  -H "Authorization: Bearer $TOKEN" \
  -H "X-Ordo-Space: system" \
  -H "Content-Type: application/json" \
  -d '{"name":"basic","price":"5.00","margin":"1.00"}'
```

The write commits, the cache drops the answers that read `plans`, and the next
visitor sees the new row.

## Not built yet

- Site labels.
- The separate read layer and single-flight misses.
- Shared live public streams.
- API keys.
- The derived public index.
- The `deployment` entity line. `public_tenant_entity` is not enforced, and the
  public path reads the deployment's space.
- A row condition on `public`. It is ruled yes and not built.
- The settings `public.cache.min_fresh_ms` and `public.refine`.
- `ordo public drop` and `POST /public/_drop`.
- Public uploads.
- The grant that lets a public write land in a tenant's space.
- The counters `ordo_public_cache_entries` and `ordo_public_cache_bytes`.
