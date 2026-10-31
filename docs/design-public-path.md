# The public path at runtime: anonymous reads, the cache, API keys

Status: **ruled 2026-10-03; phase 1 (steps 1, 3, 4) built 2026-10-10**
(`public-path-design`, all 12 asks). Steps 2 and 5 to 11 are not built. The
departures are listed under §11. The human check: Turnstile, reCAPTCHA and hCaptcha, verified by the
server, off until a key is set. It follows `plan-public-data.md` and designs what
happens when a request arrives. The spelling of
the declarations is `plan-syntax-additions.md`, written beside this. This
document assumes those declarations exist: an op or field marked `public`, a
site label, an API key profile.

Every limit named here is a setting with a shipped default (§9). People are
the deployment owner, the app developer, a tenant's owner, and a visitor.

---

## 0 · In one screen

- **A request with no token** becomes a call by the reserved anonymous
  principal, allowed only on ops the compiler marked public, in the
  deployment's space and nowhere else (§1).
- **A public read** skips the per-call audit row, the guard walk and the
  membership lookup. It still binds its parameters, checks method, version,
  site and rate (§2).
- **Results are cached** by op, arguments and program version, and dropped
  the moment an entity the op reads commits a change. The signal is the
  `ordo.invalidate` notify the write path already sends (§3).
- **A read layer** serves the cache from the same binary in a mode. It
  holds one LISTEN connection per database and no request pool. Misses go
  upstream once, shared by every visitor waiting on the same key (§4).
- **Public writes** run the ordinary checked, logged pipeline, behind an
  hourly cap per op and a human check the provider confirms (§5). Built in
  phase 2.
- **API keys** are hashed records that the door resolves in memory to a
  capability grant. Allowlist, quota, revocation and rotation are rows (§7).
- **CORS** becomes three layered lists. Public reads carry no credential, so
  they answer `*` and need no preflight (§8).

---

## 1 · The anonymous principal at the door

**Lead:** the door keeps its one rule, every call carries a credential. For a
tokenless request to a public op, the door supplies the credential itself: the
reserved anonymous principal, bound to the deployment's space.

### Today

- `check_access` refuses a request with no bearer before anything else runs:
  `None => Err(door_denial(&op.name, None))`
  (`crates/ordo-api/src/router.rs:800` to `:815`, message "missing bearer
  token"). Nothing is recorded; the stream and the metrics see no entry.
- The file door is the same shape: `blob_gate`
  (`crates/ordo-api/src/router.rs:199`) refuses a tokenless `GET /blob/{handle}`.
- With a token, `IdentityAccess::check` (`crates/ordo-host/src/lib.rs:1538`)
  verifies the MAC (`ordo_identity::verify`, `crates/ordo-identity/src/token.rs:192`),
  consults the revocation list, then runs `AccessEngine::check`
  (`crates/ordo-identity/src/access.rs:742`): suspension, delegation,
  capability pins or membership. Every denial is recorded to the security
  stream and counted in `ordo_denials_total`.
- The space is bound from `X-Ordo-Space` or `$space` before the check
  (`gate`, `router.rs:621`), and `Gate::approved_space` refuses to execute
  against a space other than the one authorized.

### The design

A tokenless request takes a short branch inside `check_access`, before the
token path, and nowhere else:

1. **Is the op public?** `OpSpec` gains `public: PublicMode` beside
   `live_capable` (`crates/ordo-api/src/spec.rs:358`), set by the compiler
   from the declaration. `None` is every op today. A tokenless request to an
   op that is not public gets today's `door_denial`, byte for byte, so a probe
   learns nothing about which ops exist.
2. **Which space?** The deployment's own space, always
   (`ordo_exec::erase::SYSTEM_SPACE`, `crates/ordo-exec/src/erase.rs:354`).
   A public request names no space. If it names one anyway, and it is not the
   deployment's space, the answer is the same `access_denied`. A tenant's
   space is never reachable without a token, which is what keeps tenants
   non-enumerable (§10).
3. **Who is the caller?** The reserved id `$anonymous`. The `$` is the
   character the platform reserves for names a caller cannot produce
   (`SYSTEM_SPACE_PREFIX`, `erase.rs:337`), so no directory entry, members
   row or token can ever carry it. One principal per deployment, since public
   data lives in one space. `access-model-explained.html` §3 proposed one per
   space before the model placed public data in the deployment's space; one is
   now enough.
4. **Which site?** If the deployment has registered site labels, the request
   carries one as the `site` query parameter (a parameter, not a header, so a
   public GET stays a simple request with no preflight, §8). A label that is
   unknown or switched off is refused with the same `access_denied`; the
   reason goes to the deployment stream and to the `site` label on the public
   counters. A deployment with no sites registered ignores the parameter.
   The label is for telling sites apart and switching one off. It secures
   nothing, and the design never treats it as a secret (ruled 2026-10-03).

The branch returns `Gate { principal: "$anonymous", space: SYSTEM_SPACE, .. }`
and the rest of the pipeline runs on it. `$caller` is bound to `$anonymous`
exactly as it is for a member (`run_call`, `router.rs:839`), so an executor
that ever met the anonymous caller would evaluate every guard false (DL G15).
The public path does not reach guards (§2), so this is a backstop, not the
mechanism.

### What the anonymous principal may do

| Request | Answer |
|---|---|
| `GET /op/{public read}` | served, through the cache (§2, §3) |
| `GET /op/{public read}?live=1` | served, as a shared stream (§4) |
| `POST /op/{public write}` | ordinary write pipeline, with limits (§5) |
| `GET /blob/{handle}` for a file of a public row | served through the cache; files follow their row (ruled) |
| anything else | `access_denied`, the door's one answer |

No platform op (`std.space.*`, `std.deployment.*`) is ever public. The
compiler refuses the qualifier on them, and the door checks it again from
the spec because the two are cheap and the failure is expensive.

### Blob reads

`blob_gate` takes the same branch: a tokenless `GET /blob/{handle}` is served
when the handle's row belongs to a public entity and the field is public. The
check uses the pseudo-op `std.$blob.get` with the entity, which already exists
for share links (`crates/ordo-host/src/share_links.rs:68`). Bytes are served
with the same cache headers as results (§3).

A public file's URL needs no token, so it is served under the same rules as
any file (`docs/howto-files.md`, "How a file is served"): the sandbox policy,
`nosniff`, and `attachment` as `application/octet-stream` unless its type is
on the default list of types shown inline.

---

## 2 · The public read path

**Lead:** the per-call checks a member's read pays exist to decide who may see
which rows. For a public op the compiler has already decided: everyone, every
public field. So those checks are not run. What remains are the checks that
guard the server, not the data.

### Skipped, and why each is safe to skip

| Skipped | Why it is safe |
|---|---|
| Per-call audit row to the security stream | Ruled 2026-10-03 (ask 10): public pulls are not logged. Counts live on the web layer as metrics (§2, counters). |
| Guard walk | A public op may read only public fields. A public field is readable by everyone, so a guard's answer is known in advance: yes. |
| Membership lookup | `$anonymous` has no members row and the op's plan never consults one. A public read op may not mention `$caller` (compile-time rule, §2 below), so there is nothing to look up. |
| `AccessEngine::check` | There are no pins and no role to compare. The one question, "is this op public", is a field on the spec. |
| Quota gate against a tenant | The request is in the deployment's space. Load is bounded by the cache and the rate limits instead (§3, §9). |

**The compile-time rule that makes this hold.** The field-scope manifest
(`op_scope`, `crates/ordo-plan/src/artifact.rs:216`, DL 9l) lists every field
of every entity an op reaches, flattened through composition, so a callee's
field is in the caller's scope. The compiler refuses a `public` op whose scope
contains a field that is not public. That covers:

- an output field that is not public;
- a join or ref hop into a non-public entity to read a field;
- a field read only in a condition. A condition over a secret field leaks it by
  bisection, which is the argument `aggregate_only` already makes
  (`crates/ordo-api/src/spec.rs:367`). If the manifest ever excludes condition
  fields, this check must widen to include them;
- `$caller` anywhere in a public read op. The result of a public read is the
  same for every visitor. That is what makes it cacheable, and it is checked,
  not hoped.

### Still checked at runtime, and why

| Check | Where | Why it stays |
|---|---|---|
| Method matches kind (GET for reads) | `gate`, `router.rs:621` | A write cannot be addressed as a read. |
| Op is public in the **active** artifact | `check_access` | Public is per program version. An op that was public in version 7 and is not in version 8 stops the moment 8 activates. |
| Parameters bind to the declared types | `bind::bind_params` | Visitor input is attacker input. Typed, bounded, refused loudly. |
| `$refine` | refused on the public path in v1 (`invalid_refinement`) | Each distinct refinement is a distinct cache key. Unbounded keys are a starvation surface (§10). A setting can open it later. |
| `$page` | allowed | Cursors are minted per `(space, op, principal)`. Every visitor shares `$anonymous`, which is safe here because every visitor sees the same rows. |
| Site label known and on | `check_access` | The one switch a deployment has for a site. |
| Per-address and per-site rate | read layer, §4 | Protects the database and the other tenants. |
| Cache lookup | read layer | The database is reached on a miss only. |

### Counters

The web layer counts, the stream does not. New series, with the existing
label cap (`crates/ordo-host/src/metrics.rs:94`):

- `ordo_public_requests_total{op,site,outcome}` with outcome `hit`, `miss`,
  `refused`, `limited`.
- `ordo_public_cache_entries` and `ordo_public_cache_bytes`.
- `ordo_public_streams` for open live streams.

A tokenless request to a non-public op is counted as `refused` and recorded to
the deployment stream as `Subject::Unverified`, once per address per window,
not once per request. The deployment stream, never a tenant's: the property
quoted at `crates/ordo-host/src/lib.rs:1567` stays true because the anonymous
principal has no relationship with any tenant.

---

## 3 · The cache and exact invalidation

**Lead:** a public result is stored under its op, its arguments and its
program version. It is dropped when any entity the op reads commits a change.
Ordo already knows that set and already sends that signal.

### The key

```
(program version, op name, canonical bound params, page token)
```

- **Program version** is the active artifact's version
  (`ordo_plan::artifact::program_version`, `crates/ordo-plan/src/artifact.rs:195`,
  stored as `Artifact.version`, `crates/ordo-exec/src/registry.rs:90`). The
  space is not in the key because it is always the deployment's space.
- **Canonical params**: the bound parameter map serialised with sorted keys.
  The same request spelled two ways is one entry.
- **The key leaves the site label out**, because the label does not change
  the answer. The label appears on the counters.

### The entry

The success envelope as bytes, its `ETag` (SHA-256 of the bytes), the list
of entities the op subscribes to, the time stored. Bounded by
`public.cache.bytes` with least-recently-used eviction.

### Where invalidation comes from

The write path already does the hard half. Inside the write transaction,
for every entity the write touched, `ordo-exec` sends
`tx.notify("ordo.invalidate", entity)` (`crates/ordo-exec/src/lib.rs:1796`;
the undo path at `crates/ordo-exec/src/undo.rs:2294`). It rides the
transaction, so it is delivered at commit and discarded on rollback.

`PgInvalidationBus` (`crates/ordo-host/src/invalidation.rs:213`) is the
LISTEN side. It holds one connection per database and an index from entity
name to the read ops that entity can change (`entity_index`,
`invalidation.rs:64`), built from each op's sources, its callees' sources and
every entity reached by a guard walk (`subscribed_entities`,
`invalidation.rs:83`, DL G16). It errs toward over-invalidation.

The cache subscribes to the same bus, as `?live=1` does
(`subscribe_in`, `router.rs:1348`). On an event for op X, every entry whose
op is X is dropped, all argument variants at once. There is no per-row
invalidation: the signal carries an entity name and nothing else, by design
(`invalidation.rs:27`), and the cache follows that.

**Nothing is learned from the signal.** The payload is an entity name the
op's manifest already lists. The cache refetches through the op itself, so
it holds only what the op would answer anyone.

**Guard entities over-invalidate here, harmlessly.** A public op has no guard
walk to subscribe, but `entity_index` adds the entities of guards on the
entities it reads. A membership write would then drop a public entry that did
not change. The cost is one refetch. A later refinement builds the public
index without the guard set; it is listed in §11 as optional.

### Deploy

On `ordo.deploy` (`DEPLOY_CHANNEL`, `crates/ordo-exec/src/registry.rs:88`,
notified inside `activate`'s transaction at `registry.rs:263`, heard by
`listen_for_changes`, `crates/ordo-host/src/deploy.rs:942`), the whole public
cache is dropped and the entity index is rebuilt from the new artifact. The
version is in the key, so an old entry could never be served anyway; dropping
frees the memory and keeps one rule. A deploy may also move rows
(`schema_apply::bring_forward`), which is the second reason not to keep
entries across one.

### Writes that bypass the executor

`ordo apply --db` and a manual SQL change send no notify. They are the
deployment owner's acts against the database and sit outside the platform's
promises. The read layer exposes `POST /public/_drop` (internal, token
required) for that case, and the CLI gets `ordo public drop [op]`.

### A lost listener

If the LISTEN connection drops, the cache cannot know what it missed. On
reconnect it drops everything (`PgInvalidationBus::unheard` already names the
databases it is not hearing, `invalidation.rs:513`). While disconnected it
serves with `max-age=0, s-maxage=0` so nothing downstream extends the window.

### CDN headers

Every public response carries:

```
Cache-Control: public, max-age=0, s-maxage=<public.cdn.s_maxage>, stale-while-revalidate=<public.cdn.swr>
ETag: "<sha256 of the body>"
Cache-Tag: op:<name>, entity:<e1>, entity:<e2>, ...
Vary: Accept
```

- `max-age=0` keeps the browser honest; a shared cache may hold it for
  `s-maxage`.
- `Cache-Tag` (Cloudflare) and `Surrogate-Key` (Fastly) carry the op and the
  entities. One purge by tag drops every variant.
- `If-None-Match` answers `304` from the stored `ETag` with no body.

### Telling caches to drop erased content

Ruled: Ordo stops serving at once and tells caches; copies already taken are
out of reach. The mechanism:

1. Unpublishing is a write (`public` flag flipped, row deleted, tenant erased
   and its rows pruned from the index, §6). The write notifies
   `ordo.invalidate`. The cache drops the entry at once.
2. The read layer calls the purge hook with the tags it just dropped:
   `public.cdn.purge_url` and `public.cdn.purge_secret`, a POST per batch of
   tags, through `ordo-egress` so the destination is vetted. Unset means no
   CDN, and the hook is skipped.
3. A CDN without a purge hook is stale for at most `s-maxage`. That bound is
   the reason the default is short (§9, ask 2).

`std.space.delete` (`crates/ordo-host/src/exit_ops.rs`) is the erasure. It
prunes the index rows (§6), which is a write, which notifies. No new path.

### Multi-replica

Each node listens for itself. There is no local shortcut and no gossip: a
write on node A goes out through `pg_notify` and comes back to every node
through its own listener, including A (`invalidation.rs:19`). Two nodes
with caches of their own are consistent to within notification latency,
which is milliseconds inside one Postgres.

Nothing is shared between nodes in v1. A shared store (one key-value service
holding the entries) is a later increment; the key and entry shapes above
are what it would hold.

---

## 4 · The read layer

**Lead:** the same binary, available out of the box. Ruled 2026-10-03 (ask
5): *"it being there should not be a separate mode unless there would be
meaningfully extra overhead. It should just be available to use out of the
box no extra configuration steps."* Every node serves public requests from
its cache, with no flag and no setup. A node can still be started without a
request pool (`ordo serve --public-read`) when a deployment wants a tier of
its own, but nothing requires it. A visitor never costs a database
connection on a cache hit.

### Why the same binary

The read layer needs the compiled artifact (which ops are public, their
parameters, their entity sets), the bus, the cache module, the CORS layer and
the metrics. All of that is `ordo-api` and `ordo-host` code. A second service
would carry a second copy of the registry loader, and the one that only runs
in production is the one that rots (the argument at `invalidation.rs:24`).

A deployment with one node runs the cache inside its API node, from the same
module, and needs no read layer. The read layer is what a deployment adds
when public load is the biggest load.

### What a read node holds

| Held | From | Refreshed |
|---|---|---|
| The active artifact: specs, public flags, entity index | `$artifacts` and `$activations` at boot | on `ordo.deploy` |
| The result cache | misses, §3 | dropped by `ordo.invalidate` and deploy |
| Site labels and their on/off state | the settings store (24a) | on its notify |
| Rate limit windows per address and per site | memory | rolls with time |
| Open live streams | visitors | §4, streams |
| One LISTEN connection per database that holds public entities | `PgInvalidationBus` | reconnect on loss |

It holds **no request pool**, no token key, no credential store and no
tenant database connection. A compromised read node yields public data and
nothing else.

### A miss

1. The read node looks up the key. Miss.
2. If another request for the same key is already upstream, this one waits
   for that answer. One fetch per key, however many visitors arrive at once.
   This is the single-flight rule, and it is what stops a cache stampede
   after an invalidation on a popular op.
3. The fetch is an ordinary `GET /op/{name}` to an API node, with the
   `site` parameter and no token. The API node runs §2 and answers. The read
   node stores the entry with the entities the API node lists in a response
   header, `X-Ordo-Entities`, which is the op's subscribed set.
4. The answer goes to every waiter.

The API node is the only thing that talks to the database. Its own in-process
cache answers a read node's miss when it has the entry, so two read nodes
missing the same key cost the database one query.

### How it learns of invalidations

By LISTEN, the same path as an API node. A read node outside the database's
network uses the relay instead: `GET /public/_invalidations?live=1` on an API
node, an internal SSE stream of `(op)` events with the same semantics as
`?live=1`. Both end in the same `InvalidationBus` on the read node.

### Live public pages share one stream

Today each `?live=1` subscriber holds one bus receiver and one executor read
(`live_inner`, `router.rs:1294`). N visitors on one public page would be N
reads per invalidation.

On the read layer, a live public request joins a **channel per cache key**:

1. The first visitor on a key creates the channel. The channel subscribes to
   the bus for the op once and fetches the result once.
2. Every visitor on the key receives the same frames. Fan-out is one buffer
   written to N sockets.
3. On an invalidation the channel refetches once (single-flight, §4) and
   sends the fresh envelope to every visitor as the event. Today's protocol
   sends an empty `invalidate` and the client refetches; a public result is
   the same for everyone, so sending it saves N round trips (ask 9).
4. The last visitor leaving closes the channel after `public.stream.linger`.

Heartbeat and lag behave as today (`HEARTBEAT`, `router.rs:1343`). A lagged
visitor gets the next frame and nothing is lost that a refetch would not
recover. `public.stream.max_per_node` bounds open sockets; past it, a new
stream is refused with `429` and the page falls back to polling.

---

## 5 · Public writes

**Lead:** a contact form or a waitlist is a public op that writes into a
private entity. It runs the ordinary write pipeline, checked and logged
(ruled). What is added in front is an hourly cap per op (built as ruled in ask 25),
and a human check.

### The pipeline

`POST /op/{public write}` with no token takes §1's branch, then the ordinary
stages: envelope split, bind, idempotency, execute, command log. Nothing is
skipped. The write runs in the deployment's space as `$anonymous`.

**Write-only, enforced by the compiler.** The op's output is the id or
nothing. A public write that returned fields of the row it wrote would be a
public read in disguise. The compiler refuses output fields that are not
public, which is §2's rule applied to the write's output.

**Attribution in the command log.** The group's actor is `$anonymous`, with
the site label in the command's metadata. The caller's address is **not**
written to the chain. The chain is per-space, append-only and exported with
the space; a visitor's address in it is a behavioural log nobody asked for,
retained forever. The address lives in the rate limiter's window only.

**Idempotency.** `ReplayKey` is `(space, principal, op, key)` with a payload
fingerprint (`crates/ordo-api/src/idempotency.rs`). Every visitor is
`$anonymous`, so two visitors who send the same `Idempotency-Key` share an
entry. The fingerprint makes a different payload a `409 idempotency_mismatch`
and never the other visitor's response; an identical payload under an
identical key is one submission replayed, which is what the key means. The
explainer's concern (`access-model-explained.html` §3, "shared namespace")
applies to reads of private rows, which a public write does not return.

### Grants for forms and uploads (ask 17)

The owner widened the ruling: the deployment may hold a grant, a form or an
upload, that lets a public write land in a tenant's space (an enquiry into a
vendor's inbox). Sketch:

- The tenant's owner grants the deployment's space a capability over one
  write op, with pins, through the ordinary consent page. It is recorded in
  the grant registry like any grant (`GrantRegistry::record`,
  `crates/ordo-identity/src/grants.rs:130`).
- The public write op in the deployment's space names the grant. At run time
  the executor performs the tenant-side write as the deployment's service
  principal under the grant's pins, in the tenant's chain, with
  `submitted_by: $anonymous` and the site label in the command metadata.
- Revoking the grant stops the form. A revoked grant is one row.

Uploads go through `PUT /blob/{entity}/{field}/{id}` with the same branch in
`blob_gate`. `public.upload.max_bytes` bounds the body. Whether a public
upload is accepted at all is the app developer's declaration, off unless
declared.

### Rate limits

Two windows, both settings (§9), both checked **before** the human check so a
flood costs no verification calls:

| Window | Key | Default | Where |
|---|---|---|---|
| per address | the caller's address, from `Peer` (`crates/ordo-host/src/auth/signup.rs:28`) | unlimited (ruled, ask 4) | read layer or API node, whichever is first |
| per tenant | the space the write lands in | unlimited | API node, from the quota gate |

**The address behind a proxy.** `Peer` is the socket's peer, and its own
comment says a trusted-proxy header is not built (`signup.rs:26`). A read
layer sits behind a load balancer or a CDN almost always, so every visitor
would share one address and one window. `public.trusted_proxies` lists the
proxy addresses whose `X-Forwarded-For` is believed; from any other peer the
header is ignored. The share link and recovery windows gain the same input.

The per-address window follows the shape `share_links::Misses` already uses
(`crates/ordo-host/src/share_links.rs:363`): a deque of timestamps per
address, pruned on read. It becomes one `AddressWindow` type shared by share
links, recovery codes (`recovery_codes.rs:40`) and the public path.

**Unlimited by default** (ask 4): *"this logic can be complex and things
like turnstile can stop it. There could be valid reasons for many writes."*
The window exists as a setting for the app developer or tenant to turn on.

Counters are per node in v1. A limit of 10 a minute set on 3 nodes admits up
to 30 a minute from one address in the worst case. A shared window through
`PgSync` (`crates/ordo-host/src/pgsync.rs`) is on the post-deploy roadmap
(ask 12; board `shared-rate-windows`, checklist 18ae).

A limited request answers `429 quota_exceeded`, the code `POST /auth/link`
already uses for an address (`crates/ordo-host/src/auth/link.rs:40`).

### The human check

Ruled: rate limits **and** a human check. A public write op declares that it
requires one (the spelling is the syntax plan's). The request carries the
proof in `X-Ordo-Human`. The door verifies it before executing:

| Provider | Verification | Needs |
|---|---|---|
| `pow` (built in) | the proof solves a challenge the read layer issued at `GET /public/_challenge`, at `public.human.pow_bits` difficulty, within `public.human.challenge_secs` | nothing outside Ordo |
| `turnstile`, `hcaptcha`, `recaptcha` | a POST to the provider's verify URL with `public.human.secret`, through `ordo-egress` | an account with the provider |
| `off` | none | a developer who has decided the rate limit is enough |

Failure answers `403 human_check_failed`. The default provider is ask 3. A
public write that requires the check and gets no header is refused the same
way.

Because the check runs on `POST`, the preflight question of §8 applies: the
header makes the request non-simple and the browser preflights it. The CORS
layer must list `X-Ordo-Human` in `ALLOW_HEADERS` (`crates/ordo-api/src/cors.rs:62`).

---

## 6 · The derived public index

**Lead:** posts stay in their authors' spaces. The public reaches them through
an index in the deployment's space that is rebuilt from what each tenant
consents to expose, never authoritative, and pruned on erasure. Sketch only.

### Shape

One entity in the deployment's space, declared by the app developer, holding
a **projection**: author space, source entity, source id, the public fields
copied, `published_at`, `withdrawn_at`. Its rows are written only by the
index builder. Public ops read it like any public entity, so §2 and §3 apply
unchanged: a public feed op over the index is cached and invalidated by the
index entity's own `ordo.invalidate`.

### Feeding it

- A tenant consents by a setting the app developer exposes (24a), off unless
  set. Consent is the whole of the disclosure decision.
- The source is a **feed** from the tenant's space: the tenant's public
  projection op, consumed as a sync (`GET /op/{name}?live=1&sync=1`,
  `crates/ordo-api/src/sync.rs`). A sync already sends `load` then `change`
  frames with `upserts` and `deletes` keyed by the anchor's id, and already
  invalidates on guard changes. The index builder is a worker that holds one
  such sync per consenting tenant and writes the frames into the index.
- Because the copy lands in the deployment's space, every public read is
  local and joinable, and the routing inversion of 2026-09-12 (a window is a
  second database under routing) is avoided.

### Pruning

- A `delete` frame removes the row.
- Withdrawn consent stops the sync and removes every row from that space.
- `std.space.delete` of a tenant removes every row from that space before
  the erasure completes. The removal is a write, so it notifies, so the
  caches drop, so the purge hook fires (§3).

### Rebuilding

The index is a projection. `ordo public reindex` drops it and replays every
consenting tenant's sync from a fresh load. Correctness never depends on the
index having been kept; it is a cache with rows.

### What it needs from feeds

1. A server-side consumer of a sync stream, running as a worker in the
   deployment with a service credential for each consenting tenant's space.
   Today's consumer is a device.
2. The projection op in the tenant's space must be `public` so the compiler
   proves it exposes only public fields. Its consent-gated exposure to the
   deployment is the feed; its public marking is what makes the index's rows
   safe to serve.
3. A tombstone for "consent withdrawn" distinct from "row deleted", so the
   builder can remove a whole space's rows in one act.
4. Search is a Postgres full-text index over the projection in v1. An
   external search service is a later increment and reads the same rows.

---

## 7 · API keys for an app's customers

**Lead:** an API key is a credential. A hashed record maps it to a principal,
a space and an allowlist of ops with pins. The door resolves it in memory to
a capability grant and runs the engine unchanged. It is never anonymous and
never a share link.

### Issuance

`std.space.issueApiKey` runs in the space the key will reach: a tenant's
space for a tenant's customer, the deployment's space for the app's own
customers when they are rows there. It takes the ops, a label, an optional
expiry, and the key's principal (below). It answers the secret **once**:

```
ok_<key id>_<secret>
```

The key id is 16 characters, visible, so a leaked key can be named in a
revocation and in a log without the secret. The secret is 256 random bits.
Only `SHA-256(secret)` is stored; a 256-bit random value needs no slow hash
(the share link makes the same choice, `share_links.rs:28`).

### Storage

`$apikey` in the control database:

| Column | Holds |
|---|---|
| `key_id` | primary key, the visible part |
| `hash` | SHA-256 of the secret part |
| `space`, `principal` | where it acts and as whom |
| `grant_id` | the capability grant recorded for it, with pins, in `GrantRegistry` |
| `label` | what the issuer called it |
| `created_at`, `expires_at`, `revoked_at` | its life |
| `rotated_from`, `retire_at` | rotation, below |
| `last_used_day` | one date, not a timestamp, so the table is not a usage log |

The grant itself is an ordinary `NewGrant`
(`crates/ordo-identity/src/grants.rs:57`) with `client = None` and pins read
off the live deployment (`AccessEngine::grant_for`). `callersOf` lists the key
beside every other grant, which is the audit surface the owner already has.

### Whose authority

Two kinds, chosen at issuance:

| Kind | Principal | Reach | Fits |
|---|---|---|---|
| **Personal** | the issuer's own principal | the allowlist, intersected with what the issuer can do; a `$caller`-bound guard sees the issuer's members row | a customer who signs in and automates what they can already do |
| **Service** | a seat of its own: a members row with no role, as a share link's seat is (`share_links.rs:14`), with a new principal kind `key` | the allowlist; rows through acl rows the issuer grants it, or an op the developer wrote for it | an integration that must outlive any person |

A personal key is a delegation from the issuer to the key, so the engine's
delegation rule applies (`access.rs:762`): the key's authority is the
intersection, and a demoted issuer demotes the key. A service key is the
seat's own authority and survives the issuer leaving.

**Both ship together** (ruled 2026-10-03, ask 6: *"both"*).

### At the door

`bearer` sees `ok_`. The host's `IdentityAccess::check` takes a branch before
`verify`:

1. Split id and secret. Look the id up in an in-memory map
   `key_id → (hash, grant, principal, expires, revoked)`.
2. Constant-time compare the hash. Mismatch, unknown id, revoked or expired
   is one answer: `access_denied`, recorded as `Subject::Unverified` to the
   deployment stream, counted in `ordo_denials_total` under one bucket. Same
   as an unverifiable token today (`lib.rs:1572`).
3. Synthesize `TokenClaims { principal, capability: Some(grant), .. }` and run
   `AccessEngine::check` unchanged. The capability path is exclusive by token
   shape and never falls to roles (`access.rs:780`).

**No database read on the request path.** The map follows
`PgSessionRevocations` (`crates/ordo-host/src/sessions.rs`): loaded when the
store is built, written through on issue, revoke and rotate, re-read on a
cadence, and nudged by `pg_notify('ordo.apikeys')` so another replica learns
within the notify latency rather than the cadence. The map is small: keys are
issued by people, in the hundreds or thousands, not per visitor.

Why not exchange the key for a short token at `POST /auth/key`, as a share
link is exchanged at `POST /auth/link`? That is the cleaner door, and a
customer's SDK could hide it. Most API products expect the key as the bearer,
and the in-memory map costs the door one hash and one lookup. Ask 7.

### Quotas

Default **unlimited** (ruled, ask 21). `PgQuotas` keys limits by `(space,
quantity)` today (`crates/ordo-host/src/quotas.rs:55`) and treats a missing
row as `Within` (`quotas.rs:380`). It gains a `subject` column: `*` for the
space, or a key id. The check runs the space row and the key row and takes
the worse (`worse_of`, `quotas.rs:321`). Layered:

| Layer | Sets | How |
|---|---|---|
| deployment default | a ceiling for every key, unset means none | `ORDO_APIKEY_REQUESTS_PER_PERIOD` |
| app developer | a ceiling per plan or per op class | program setting (24a) |
| tenant, when exposed | a ceiling per key it issued | `std.space.setApiKeyQuota` |

Usage counts land in the existing ledger as the key's `subject`.

### Revocation and rotation

- `std.space.revokeApiKey <key id>` sets `revoked_at`, revokes the grant in
  the registry, writes through to the map, notifies. Immediate on the replica
  that served it; elsewhere within the notify. Idempotent.
- `std.space.rotateApiKey <key id>` mints a new secret for a new key id with
  the same grant and principal, and sets the old key's `retire_at` to now plus
  `apikey.rotation_overlap` (default 24 hours). Both verify until then. A
  deploy that narrows an op applies to both silently (`access.rs:793`).
- Expiry is in the record and in the synthesized claims, so an expired key
  is refused from memory.

### How a key differs

| | Anonymous principal | Share link | API key |
|---|---|---|---|
| Credential | none; the door supplies `$anonymous` | a secret opened once for a token | a secret presented on every call |
| Who | everyone | whoever holds the link | one customer, named |
| Space | the deployment's | one tenant | the space it was issued in |
| Ops | every public op | the pins listed at mint, reads only | the allowlist, reads and writes |
| Guards | not run; compile-time public | run, over the seat's acl rows | run, as the principal |
| Audit | counters only | ordinary | ordinary |
| Quota | rate limits, per address and site | the tenant's | its own, default unlimited |
| Revocation | switch the site off | one row, checked on every call | one row, in memory within the notify |

---

## 8 · Per-tenant CORS origins

**Lead:** one deployment-wide list becomes three layered lists. The preflight
answers the union, the actual response answers the one list that matches
the request's space. Public reads carry no credential and answer `*`.

### Today

`ORDO_CORS_ORIGINS` is parsed at boot (`RuntimeConfig::cors_from_env`,
`crates/ordo-host/src/runtime.rs:574`; `CorsPolicy::parse`,
`crates/ordo-api/src/cors.rs:81`). `apply` wraps the finished router
(`cors.rs:141`), so the preflight is answered **before routing** and before
the space is known. `*` is refused because every call carries a bearer
(`cors.rs:25`).

### The constraint a preflight imposes

A browser's `OPTIONS` carries `Origin` and the **names** of the headers the
real request will send, not their values. So the preflight cannot know which
tenant the real request is for. Two answers:

1. **Preflight answers the union** of the deployment list, the developer
   list and every tenant's list. Allowing a preflight only lets the browser
   send the request. It discloses nothing, because the origin that asks is the
   one being told about itself.
2. **The actual response answers the exact list** for the request's space,
   once `X-Ordo-Space` is read. An origin not on that space's list gets no
   `Access-Control-Allow-Origin`, and the page cannot read the answer. This is
   the enforcement, and it is per tenant.

A deployment that routes tenants by host (`tenant.app.example`) resolves the
space at preflight from the host and answers the exact list there too.

### The layers

| Layer | Who | Where | Default |
|---|---|---|---|
| deployment | deployment owner | `ORDO_CORS_ORIGINS`, unchanged | none |
| app developer | the program | a settings declaration (24a) | none |
| tenant | its owner, only when the developer exposes it | `std.space.setOrigins`, stored with the tenant's settings (today's pattern is `$admissionpolicy`, `crates/ordo-host/src/admission.rs:233`) | none |

Effective list for a space = deployment ∪ developer ∪ that tenant's own.
Each entry is still an exact origin; `*` is still refused on every layer,
for the reason `cors.rs` gives. The lists live in memory on every node and
refresh on their notify, like the other door inputs.

### Public requests

A public GET carries no `Authorization`, no `X-Ordo-Space` and no custom
header, so the browser sends it with no preflight at all. The response
carries `Access-Control-Allow-Origin: *`, which is correct and safe: there is
no credential a foreign page could spend, and the data is public. This is the
one place the wildcard is right, and the layer sends it only on §1's branch.

A public write with `X-Ordo-Human` is preflighted. The preflight for a public
op answers `*` as well, with `X-Ordo-Human` and `Idempotency-Key` allowed.

---

## 9 · Settings

**Lead:** every number here is a layered setting with a shipped default and
no ceiling of Ordo's own (owner, 2026-09-29, `settings-layers`). Each lands
in the 24a mechanism when it exists and as an environment variable until then.
Nothing is a fixed rule.

| Setting | Layer | Default | What it bounds |
|---|---|---|---|
| `public.cache.bytes` | deployment | 256 MiB | the result cache per node |
| `public.cache.min_fresh_ms` | developer, per op | 0 (exact) | the shortest time an entry survives an invalidation; 0 is the ruled behaviour |
| `public.cdn.s_maxage` | developer | 60 s (ask 2) | how long a shared cache may hold a public answer |
| `public.cdn.swr` | developer | 300 s | how long stale may be served while refetching |
| `public.cdn.purge_url`, `public.cdn.purge_secret` | deployment | unset | the purge hook |
| `public.refine` | developer | off | whether `$refine` is accepted on public reads |
| `public.read.per_address_per_minute` | developer, tenant when exposed | 600 | public reads from one address |
| `public.write.per_address_per_minute` | developer, tenant when exposed | 10 (ask 4) | public writes from one address |
| `public.write.per_tenant_per_minute` | deployment, developer | unlimited | public writes landing in one space |
| `public.upload.max_bytes` | developer | 10 MiB | a public upload body |
| `public.trusted_proxies` | deployment | none (the socket peer is the address) | which peers' `X-Forwarded-For` names the visitor |
| `public.human.provider` | developer | `pow` (ask 3) | which human check |
| `public.human.pow_bits`, `public.human.challenge_secs` | developer | 18, 300 | the built-in check's cost and life |
| `public.human.secret` | deployment | unset | a provider's secret |
| `public.stream.max_per_node`, `public.stream.linger` | deployment | 10,000, 30 s | open live streams; how long an empty channel waits |
| `public.sites` | developer | none | the registered site labels and their on/off state |
| `apikey.default_expiry` | developer | none (does not expire) | a key's life when the issuer sets none |
| `apikey.rotation_overlap` | developer, tenant when exposed | 24 h | how long both secrets verify |
| `apikey.refresh_secs` | deployment | 15 s | the in-memory map's cadence, beside the notify |
| `ORDO_APIKEY_REQUESTS_PER_PERIOD` and the per-plan and per-key rows | deployment, developer, tenant | unlimited | a key's quota |
| `cors.origins` | deployment, developer, tenant when exposed | none | the three lists of §8 |

Defaults follow 24c's rule: a value that behaves like today where today has a
value, and the smallest safe value where it does not. The owner decides the
ones marked with an ask.

---

## 10 · Threats

| Attack | What is gained | What stops it |
|---|---|---|
| **Cache poisoning**: a visitor gets a bad answer stored under a key others read | every visitor served the attacker's content | The cache stores only what the executor answered. Visitor input reaches the key as typed, bound parameters and never as content. There is no write into the cache from outside; `_drop` only removes. |
| **Key starvation**: a crawler asks a public op with endless distinct arguments | every request a miss; the database does the work | `$refine` is off on the public path. Declared parameters are typed and bounded by the compiler. Per-address read limits. Single-flight stops a stampede on any one key. |
| **A crawler starving other tenants** | tenants' requests queue behind public load | Public load is served by the read layer from the cache and never holds a request pool. The database sees misses only, one per key per invalidation. `pool_space_capped` already guards the pool per space (`metrics.rs:211`). |
| **A public op reads a non-public field through a join** | a private field served to everyone | The field-scope manifest flattens composition and refs; the compiler refuses any non-public field in scope, including in a condition (§2). The runtime relies on that proof and does not re-derive it. |
| **Bisection through a condition** on a secret field | the secret, to the dollar | The same manifest rule; it is the `aggregate_only` argument at `spec.rs:367`. If condition fields ever leave the manifest, the check must follow them. |
| **Enumeration of tenants** by asking public ops with `X-Ordo-Space` | which spaces exist | A tokenless request that names any space but the deployment's answers `access_denied`, the same bytes as an unknown op. The anonymous principal is bound to one space. |
| **A tokenless request to a non-public op** probes which ops exist | the op list | `door_denial` is identical for unknown and non-public. |
| **API key leaked** in a repository | the customer's allowlisted ops | Only the hash is stored, so a database copy yields no key. The visible id names the key in a revocation. Revocation is one row, immediate locally and within a notify elsewhere. A personal key is capped by its issuer's reach; a service key by its acl rows and pins. |
| **API key brute force** | a key by guessing | 256 random bits. One uniform answer for unknown and wrong. Counted in one bucket so series cannot be minted by guessing. |
| **A revoked key still works on another replica** | a window of calls | The notify closes it within milliseconds; the cadence (`apikey.refresh_secs`) is the floor if the notify is lost. |
| **A site key used as a secret** by a developer who misreads it | nothing, by design | The label unlocks only public ops, which need no label. The docs and the setting's name say "label". |
| **Spoofed `X-Forwarded-For`** to dodge the per-address window | a fresh window per request | The header is read only from peers in `public.trusted_proxies`; from anyone else the socket peer is the address. |
| **Form flood** | a tenant's inbox filled, the chain grown | per-address limit first, human check second, both before execution. The write lands in a private entity nobody reads publicly. |
| **A stranger grows a tenant's audit spine** | noise in a tenant's stream | Anonymous denials go to the deployment stream, never a tenant's, once per address per window (§2). |
| **Stale erased content at a CDN** | content after erasure | The purge hook fires on the invalidation; without a hook the window is `s-maxage`. Copies already taken are out of reach (ruled). |
| **Preflight union leaks a tenant's origins** | one origin learns it is allowed somewhere | The preflight tells an origin about itself only. The actual response enforces the exact list. |
| **Public write replay across visitors** | one visitor's response shown to another | The replay entry is bound to the payload fingerprint; a public write returns no private fields. |

---

## 11 · Build order

Each increment is independently testable and deployable with the rest unbuilt.

| # | Increment | Touches | Tests that prove it |
|---|---|---|---|
| 1 | **Built (phase 1, 706020ab, 5c6f2fa4, bb615bac).** **`public` on the spec, and the anonymous door.** `OpSpec.public`; the branch in `check_access` and `blob_gate`; `$anonymous`; the deployment's space bound; `$refine` refused; the uniform refusal. No cache. | `ordo-api/src/spec.rs`, `router.rs`, `ordo-host/src/lib.rs`, the artifact loader | tokenless public read is served; tokenless non-public op answers byte-identical `access_denied`; a space other than the deployment's is refused identically; a public write runs as `$anonymous` and is in the command log; a public op's file is served and a private row's file is not. |
| 2 | **Site labels.** The `sites` setting, the `site` parameter, on/off, counters. | `ordo-host` settings, `router.rs`, `metrics.rs` | a switched-off site is refused and counted; an unknown label is refused; no label is accepted when none are registered. |
| 3 | **Built (phase 1, 706020ab, 5c6f2fa4, bb615bac).** **The in-process result cache with exact invalidation.** Key, entry, bus subscription, drop on deploy, `ETag`/`304`. | new `ordo-api/src/public.rs`, `ordo-host/src/invalidation.rs` | second identical read is a hit; a write to a read entity makes the next read a miss and fresh; a write to an unrelated entity leaves the hit; a deploy drops everything; a lost listener drops everything on reconnect. |
| 4 | **Built (phase 1, 706020ab, 5c6f2fa4, bb615bac).** **CDN headers and the purge hook.** `Cache-Control`, `Cache-Tag`, the egress call. | `public.rs`, `ordo-egress` | headers carry the settings; a drop posts the dropped tags once; an unset hook posts nothing. |
| 5 | **The read layer mode.** `ordo serve --public-read`, single-flight, upstream fetch with `X-Ordo-Entities`, the relay stream. | `ordo-host/src/serve.rs`, `public.rs` | 100 concurrent misses on one key reach upstream once; an invalidation heard by LISTEN drops the read node's entry; the relay delivers the same event; the read node opens no request pool. |
| 6 | **Shared live public streams.** Channel per key, push of the fresh envelope, linger, the per-node cap. | `public.rs`, `router.rs` live path | N subscribers cause one refetch per invalidation; all N receive the frame; the cap refuses the N+1th with `429`. |
| 7 | **Built (phase 2, 2026-10-11).** **Public write limits and the human check.** `AddressWindow`, per-tenant window, `pow` and the provider hook, `X-Ordo-Human` in CORS. | `ordo-host` auth, `cors.rs`, `ordo-egress` | the 11th write from one address in a minute is `429`; a write without proof is `403 human_check_failed`; a solved challenge passes; the address is absent from the command log. |
| 8 | **API keys.** `$apikey`, issue, revoke, rotate, the in-memory map and notify, the door branch, quota `subject`. | `ordo-host/src/apikeys.rs` (new), `lib.rs`, `quotas.rs`, platform ops | a key calls an allowlisted op and not another; the secret appears nowhere but the issue answer; revoke refuses on the next call; rotate verifies both during the overlap and one after; a per-key quota row refuses at the limit while the space stays `Within`; a personal key cannot exceed its issuer. |
| 9 | **Built (phase 2, 2026-10-11).** **Per-tenant CORS.** The three lists, the union preflight, the exact actual answer, `*` on the public branch. | `cors.rs`, tenant settings | a tenant's origin passes preflight and reads the answer for its space and not another's; a public GET from any origin reads the answer with no preflight; `*` is refused as a list entry on every layer. |
| 10 | **The derived index.** The index entity, the sync consumer worker, consent on and off, pruning on `std.space.delete`, `reindex`. | `ordo-host/src/workers.rs`, new builder | a consenting tenant's post appears in the public feed; withdrawing consent removes every row; erasing the tenant removes them and invalidates the feed op; `reindex` from empty equals the live index. |
| 11 | *Optional.* A public entity index without guard entities; a shared cache store across nodes; shared rate windows through `PgSync`. | `invalidation.rs`, `public.rs`, `pgsync.rs` | fewer spurious drops under membership writes; one miss across two read nodes; one count across replicas. |

### Built in phase 1 (2026-10-10, 706020ab, 5c6f2fa4, bb615bac)

| Row | Where | Tests |
|---|---|---|
| 1 | `crates/ordo-plan/src/public.rs` (compile classes), `crates/ordo-schema/src/lib.rs` (`resolve_public`), `crates/ordo-api/src/router.rs` (`public_call`, `anonymous_home`, `ANONYMOUS`), `crates/ordo-host/src/public.rs` | `the_anonymous_door` in `crates/ordo-host/tests/public_path.rs`; `a_public_rows_file_is_served_and_a_private_rows_is_not` in `crates/ordo-host/tests/blob_door.rs`; `crates/ordo-plan/tests/public_ops.rs`; `crates/ordo-conformance/tests/public_catalogue.rs` |
| 3 | `crates/ordo-api/src/public.rs` (cache), `crates/ordo-host/src/public.rs` (settings, purge hook) | `the_cache_and_its_invalidation` in `crates/ordo-host/tests/public_path.rs`; the unit tests in `crates/ordo-api/src/public.rs` |
| 4 | `crates/ordo-api/src/public.rs`, `crates/ordo-host/src/public.rs` | `the_cdn_headers` in `crates/ordo-host/tests/public_path.rs` |

Departures from the design as written:

- **Tokenless `?live=1` answers `not_live`.** Shared public streams (row 6) come later.
- **No per-address read limit.** The 2026-10-06 ruling is that no decision uses
  a client address.
- **The purge secret is a file.** `ORDO_PUBLIC_CDN_PURGE_SECRET_FILE` names it.
  A secret in `ORDO_PUBLIC_CDN_PURGE_SECRET` is refused at boot.
- **A lost listener stores nothing.** While the database listener is lost the
  cache keeps no new entry, and answers say `s-maxage=0`. The design said to
  serve with `s-maxage=0`. This is stricter.
- **Settings are environment variables** until the settings mechanism 24a exists.
- **`public_tenant_entity` is not enforced.** The `deployment` entity line is
  not built, so the public path reads the deployment's space.

### Built in phase 2 (2026-10-11)

| Row | Where | Tests |
|---|---|---|
| 7 | `crates/ordo-plan/src/public.rs` (the three write classes), `crates/ordo-api/src/router.rs` and `public.rs` (the door order, the answer), `crates/ordo-host/src/public.rs` (the cap, the human check, the counters), `crates/ordo-host/src/proxy.rs` (proxy mode) | `crates/ordo-plan/tests/public_ops.rs`: `a_public_write_that_creates_compiles`, `a_public_write_that_changes_a_row_is_refused`, `a_public_write_keyed_on_a_unique_field_is_refused_unless_create_once`, `a_public_create_once_on_a_unique_field_compiles`, `a_public_write_into_a_guarded_entity_is_refused`, `a_public_write_aiming_a_ref_at_a_private_row_is_refused`. `crates/ordo-host/tests/public_writes.rs`: `a_public_write_creates_a_row_as_the_anonymous_principal`, `a_public_write_cannot_reach_outside_what_its_op_names`, `the_human_check_is_on_by_default_and_names_its_settings`, `public_writes_past_the_hourly_cap_are_refused_by_name`, `a_public_write_drops_the_public_answers_that_read_its_entity`, `proxy_mode_slows_a_busy_visitor_and_never_refuses`. `crates/ordo-host/tests/marketing_site_app.rs`. Unit tests in `crates/ordo-host/src/proxy.rs` and `public.rs` |
| 9 | `crates/ordo-api/src/cors.rs` (the layers, the public `*`), `crates/ordo-host/src/tenant_origins.rs` (the lists, `std.space.setOrigins`, `std.space.origins`) | `a_tenant_lists_its_own_origins_when_the_deployment_allows_it` in `crates/ordo-host/tests/public_writes.rs`; `crates/ordo-api/tests/cors_layers.rs`; the unit tests in `crates/ordo-host/src/tenant_origins.rs` |

The public counters `ordo_public_requests_total{op, kind, outcome}` and
`ordo_public_slowed_total` were built with row 7. `examples/marketing-site` is
on the public path, with its contact form as a public write.

Departures from the design as written, phase 2:

- **A per-op hourly cap** replaces the per-address and per-tenant windows
  (ruled, ask 25). `ORDO_PUBLIC_WRITES_PER_HOUR`, per node.
- **No `pow` provider.** The providers are `turnstile`, `hcaptcha` and
  `recaptcha`, and the check is on by default (ruled).
- **No `public.trusted_proxies` per-address window.** Proxy mode slows a busy
  visitor and never refuses (ruled, ask 28).
- **Two CORS layers, not three.** The developer layer waits on 24a. The layers
  are the deployment's and the tenant's. Tenant lists are off until
  `ORDO_CORS_TENANT_ORIGINS_MAX` is set.
- **Public writes into a tenant through a grant** (§5, "Grants for forms and
  uploads") are not built. A public write lands in the deployment's space.
- **The write answer keeps `created` only** for each row.

Increments 1 to 4 make a single-node deployment correct and cacheable.
Increment 5 is the horizontal scaling the owner asked for. 7 to 10 can
proceed in any order after 1.

---

## 12 · Draft board asks

Each is one question for the owner, with the recommendation first.

1. **ask:** Should a public read accept `$refine` in v1, or only its declared parameters?
   **suggest:** **Declared parameters only.** Every distinct refinement is a distinct cache key, so an open `$refine` lets one crawler turn every request into a database read.

2. **ask:** What should the shipped default for `s-maxage` on public responses be?
   **suggest:** **60 seconds.** A CDN with no purge hook is stale for at most this long after an erasure, and a hooked CDN drops the entry at once anyway.

3. **ask:** Which human check ships as the default for public writes: the built-in proof-of-work, or none until a provider is configured?
   **suggest:** **The built-in proof-of-work.** The ruling asks for a human check, and a default that needs an account with a third party is a default that is off.

4. **ask:** What should the shipped default per-address limit on public writes be?
   **suggest:** **10 a minute.** A person fills a form a few times at most, and a flood of 10 a minute per address costs a tenant little while the developer tunes it.

5. **ask:** Should the read layer be the same binary in a mode (`ordo serve --public-read`) rather than a separate service?
   **suggest:** **Same binary, in a mode.** It needs the artifact, the bus and the cache module that already exist, and a second service would carry a second copy of the registry loader.

6. **ask:** Which API key kind ships first: personal keys that act as their issuer, or service keys with a seat of their own?
   **suggest:** **Personal keys first.** A customer signs in and automates what they can already do, every existing guard applies, and no new seat or acl machinery is built.

7. **ask:** Is an API key presented directly as the bearer on every call, or exchanged once for a short-lived token?
   **suggest:** **Directly, resolved in memory.** API customers expect the key as the bearer, and the map costs the door one hash and one lookup with no database read.

8. **ask:** May `public` on an entity carry a row condition, so drafts stay private without a condition in every public op?
   **suggest:** **Yes.** A landing page has drafts, and one condition on the declaration is checked once by the compiler instead of remembered in every op. (For the syntax plan too.)

9. **ask:** On a public live stream, should Ordo push the fresh result, or send `invalidate` and let each visitor refetch as today?
   **suggest:** **Push the result.** A public result is identical for every visitor, so one frame saves N refetches, and the private path keeps today's signal.

10. **ask:** Should the command log record a public write as `$anonymous` plus the site label, with the caller's address kept out of the chain?
    **suggest:** **Yes, address out.** The chain is exported with the space and kept forever, and an address there is a behavioural log nobody asked for; the rate limiter holds it for a minute.

11. **ask:** Should a deploy drop the whole public cache, including entries for ops the deploy did not change?
    **suggest:** **Yes, drop everything.** A deploy may move rows, the version is in the key anyway, and one rule is easier to trust than a per-op diff.

12. **ask:** Are per-address rate windows counted per node in v1, accepting that 3 nodes admit up to 3 times the limit?
    **suggest:** **Per node in v1.** The window is a cost bound, not a number a tenant relies on, and a shared count through `PgSync` is one later increment.
