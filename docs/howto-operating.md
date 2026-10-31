# Operating an Ordo Deployment

An Ordo deployment is a stateless HTTP service fronting a Postgres database. Each node independently loads the program from the artifact registry, runs a load gate on it, and serves until a newer version arrives. Rolling back is a re-point to a prior version; the audit chain records every transition.

## Deploying a Program

`ordo apply <dir|file.ordo>` compiles a program, stores its artifact in Postgres under a content hash, and points the live deployment at it. Every node loads the new version, verifies its shapes against the database schema, and begins serving, or refuses and keeps serving the old one.

**Two ways to run it, and the difference is what the command holds.**

Against the deployment's **door**, which is the ordinary path once a
deployment is running:

```
export ORDO_TOKEN=…            # a sign-in token; see "Getting a token"
ordo apply ./myprogram.ordo --host ordo.inf.smg.icu --space ordo-system
```

Against the deployment's **database**, which is what a first deployment and a
recovery need:

```
ordo apply ./myprogram.ordo --db prod --as my-service
```

The door path needs no database credentials. It reaches
`std.deployment.apply` on the running deployment, which calls the same apply
the database path calls, and prints the same report, literally the same
text, because both render the same structured outcome.

**`--host`, `--token`, `--space`.** `--host` takes a bare hostname, which
gets `https`, or a spelled scheme. `http://` must be spelled, which is what
addressing a local `ordo dev` looks like (`--host http://127.0.0.1:4000`):
plaintext is never a default for a request carrying a token that can replace
the program. `--host` with an empty value selects the default deployment,
`ordo.inf.smg.icu`. The environment forms are `ORDO_HOST`, `ORDO_TOKEN` and
`ORDO_SPACE`. **With no `--host` and no `ORDO_HOST`, the command talks to the
database exactly as it always has**: the door is opt-in.

`--space` is the **deployment's own** space, the one the binary was
configured with (`ORDO_SPACE` on the server). Applying is the deployment's
act, so the op answers in that space and in no other: an owner of a tenant
space gets `not_found`, and no membership change inside a tenant will change
that.

**`--as` is refused with `--host`.** On the database path `--as` is an
assertion an operator holding credentials makes about themselves. Over the
door the actor written into `$migrations` is the principal the token proved,
so there is nothing to assert. Taking the flag and ignoring it would put a
string in a command line that does not reach the record it looks like it is
writing.

> **Use a passkey for platform ops** (updated 2026-10-03). A password
> sign-in cannot produce presence, by design: `LocalPasswordProvider`'s own
> doc says *"every step-up op stays shut to a password-only session"*, and
> every platform op is step-up gated, `apply` included. Passkey sign-in is
> built and proves presence: set `ORDO_AUTH_METHODS=passkey` (or
> `password,passkey`) and `ORDO_AUTH_ORIGINS`, as `docs/deploying.md`
> describes. The password example below gets a token for ordinary ops only.

**Getting a token.** There is no deployment-owner credential and no API key. A
deployment owner signs in as a principal who is an **owner** of the deployment's own
space, and uses that session's bearer token:

```
curl -s https://ordo.inf.smg.icu/auth/begin \
  -H 'content-type: application/json' \
  -d '{"method":"password","identifier":"you@example.com"}'
curl -s https://ordo.inf.smg.icu/auth/complete \
  -H 'content-type: application/json' \
  -d '{"challenge":"…","password":"…"}'          # → {"token":"…"}
```

Two properties of that token decide what the door will accept, and both are
the platform's rather than the CLI's. It must carry **presence**: applying is
step-up gated like every platform op, and `/auth/refresh` continues a session
without a new sign-in. And it can never be a **capability token**: a
capability token can never satisfy step-up, which is what makes applying a
schema structurally human-only: no third-party client and no delegated agent
can deploy, however its allowlist reads.

**Two applies at once queue.** They serialize on an advisory lock keyed by
the `$migrations` table, taken before the first read of the schema, so two
deploys that arrive together cannot both decide the target is empty. A second
apply waits up to thirty seconds and then refuses with `apply_in_progress`,
which is a retry the caller decides to make rather than one the platform
makes silently: the apply holding the lock may be changing the very schema
the waiter planned against.

**What the load gate checks (before any node serves a new version):**
- The artifact's ops compile to valid SQL.
- Every op's inferred shape matches what Postgres reports when asked.
- The database schema matches the artifact's DDL exactly.

If any check fails, the output names which ops failed and why. The node keeps serving the previous version.

**The schema, before the artifact.** `ordo apply` reads the database's state first and prints one `schema` line:

- `schema     created` on an empty database (none of the program's tables, nothing serving): the whole schema is created in one transaction, recorded in `$migrations`, and the apply goes on to store and activate. This is the first deployment.
- `schema     matches`: the database holds everything the program declares. Store and activate.
- `schema     behind`: the database lacks something the program declares, so no node could load it. The artifact is stored and activation is **refused**; the output names the changes and the command to run (`ordo migrate --apply`, below). Run it, then `ordo apply` again.
- `REFUSED  serving_schema_drifted`: the registry names a serving version and none of its tables exist. Restore the database, or clear `$artifacts` and `$activations` if the registry is what is wrong. Nothing is created over a registry that disagrees with the catalog.

A refusal over the door carries the same class the database path prints,
under a wire code: `apply_refused` (the program or one of its ops does not
compile, and nothing was stored), `not_activated` (stored, and the load gate
would refuse it: run `ordo migrate --apply`), `apply_in_progress`,
`schema_drift` (the registry names a serving version whose tables are gone).

**First deployment:**
```
ordo apply ./myprogram.ordo --db prod --as my-service
schema     created  (12 table(s) on an empty target; `$migrations` #1; event recorded on the deployment stream)
stored     abc123def456…
activated  abc123def456…  (was nothing)
nodes      nudged on `ordo.deploy`
```

**Healthy result, later:**
```
schema     matches
stored     abc123def456…  (new)
activated  abc123def456…  (was xyz789…)
nodes      nudged on `ordo.deploy`
```

**Failure looks like:**
```
REFUSED  2 op(s) in this program do not compile, so no node could load the artifact:
  staleOrders [unbound_required_param] parameter $since is not a known field
  missingJoin [plan_error] no path from orderLine to shipment
```

## Changing the Schema

A program whose entities changed needs the database moved before the artifact can be activated. Two commands, in this order:

1. `ordo plan ./myprogram.ordo --against prod` reads the database and prints every change between it and the program, classified (`A2`, `C1`, `B5`…), probed against the live rows, and costed. Read-only. Exit code 1 means a probe or a ruling refused, and the report says which.
2. `ordo migrate ./myprogram.ordo --apply --db prod --as my-service` runs the same plan and then applies it: class A (a new entity, an optional field, a constant-defaulted field, an index) in one transaction, then each constraint validated and each index built concurrently after it. A foreign key or a check onto a table that exists is added `NOT VALID` in the transaction (a moment's lock, no scan) and validated as its own step (the rows scanned under a lock that lets writes continue). A unique applies only where it constrains no existing data. Recorded in `$migrations` and on the deployment's security stream. Then `ordo apply` stores and activates.

**Healthy result:**
```
APPLY  (5 statement(s))
  tx  A2  ALTER TABLE "orders" ADD COLUMN "note" text;
  tx  A3  ALTER TABLE "orders" ADD COLUMN "priority" bigint NOT NULL DEFAULT 0;
  tx  C9  ALTER TABLE "orders" ADD CONSTRAINT "orders$priority_gte_0" CHECK ("priority" >= 0) NOT VALID;
  vc  C9  ALTER TABLE "orders" VALIDATE CONSTRAINT "orders$priority_gte_0";
  cc  A4  CREATE INDEX CONCURRENTLY "orders$customer$placed$ix" ON "orders" ("$space", "customer", "placed");
  APPLIED  3 statement(s) in one transaction, 1 constraint(s) validated and 1 index(es) built concurrently after it;
           recorded as `$migrations` #7; event recorded on the deployment stream
  next     `ordo apply ./myprogram.ordo --db prod` stores and activates abc123def456…
```

The deployment keeps serving throughout: the serving artifact tolerates the extra column, and the new artifact reads it once activated.

**What it refuses, and what to do:**

- `not_class_a` on a `B` change (a required field over rows, a type change, sealing): increment 3's backfill. Author it as the three-deploy sequence in `plan-schema-evolution.md` §3.2, or wait for the increment.
- `not_class_a` on a drop (`D1`, `D2`, `C6`…) and `artifact_first` on a dropped index: activate the artifact that no longer declares the object first. Drops land in increment 5; a dropped index's `DROP INDEX CONCURRENTLY` is printed for you to run after activation.
- `constraint_over_existing_rows`: a `unique` onto a table with rows. A unique has no `NOT VALID` form, so it would scan every row under the lock; the probe above it is clean and the statement is printed to run by hand after reading the probe. (A foreign key or a check onto rows is no longer refused: it is added `NOT VALID` and validated as its own step.)
- `validate_failed` in `$migrations`: a row that breaks a constraint was written between the probe and the add. The transaction committed, the constraint was dropped again, and the message names the constraint and the table. Fix the rows and apply again; the plan proposes the constraint afresh.
- `blind_index_backfill_required`: a `sealed indexed` field onto a populated table. Increment 4.
- `probe_not_empty`: the live rows violate the constraint the program declares. The report shows the offending values; which row survives is your decision.
- `serving_schema_drifted`: the database does not hold what the serving version declares (a column dropped by hand, a table missing). Repair or restore first; `ordo plan --against` lists the gap.

When any change is refused, nothing is sent. An apply that landed half its changes would leave a database that matches no version.

**If it fails part-way:** `FAILED` with the statement and the database's error. A failure inside the transaction rolls back and changes nothing. A failure on a concurrent index leaves the columns and constraints live and the index present by name and used by nothing; the output names the `DROP INDEX CONCURRENTLY` to run before applying again. Both are recorded in `$migrations` with the phase `failed` or `index_invalid`.

**Reading the record:** `SELECT seq, kind, phase, actor, to_version FROM "$migrations" ORDER BY seq` lists every apply, and the deployment's security stream carries a `schema-migration` entry per row.

## Rolling Back

`ordo rollback --version <hash>` re-points the deployment at a version it has already stored and verified. The version hash appears in `ordo deploys` output.

**Command:**
```
ordo rollback --version abc123def456… --as rollback-reason:incident-12345
```

**Healthy result:**
```
rolled back to abc123def456…  (was xyz789…)
nodes      nudged on `ordo.deploy`
```

**Seeing what is deployed:**

`ordo deploys` lists every stored version and the order in which they were activated.

```
ordo deploys --limit 30
```

Output shows the artifact registry and the activation log in order, so you can see which version was serving at a given time.

## Every tenant's database follows the program

With one database per tenant, an update has to reach each person's database as well as the deployment's own. It does, in three ways.

- **An apply through the door** (`ordo apply --host`, `std.deployment.apply`) applies the change to the deployment's own database and then to each tenant database, 8 at a time and 20 seconds each, before it activates. Safe additive changes land. Anything else is refused by name with nothing sent to that database. The answer carries a `tenants` line: how many were brought forward, how many were already there, and each database that was held with the reason. One database that is down or slow does not fail the apply and does not hold back the activation. A staged apply (`--no-activate`) touches no tenant.
- **A program applied to one database** (`ordo apply --db`, which `examples/notes/dev/serve.sh` uses) reaches no tenant's database by itself. Each replica notices the swap and brings every tenant database forward within seconds.
- **A tenant that was down, or was created from the previous version,** is found by the same catch-up, once at boot and then on a tick (`ORDO_TENANT_CATCHUP_SECS`, default 15).

Until its database has followed, a tenant is closed. The person who calls it is told: "space `...` is closed for a moment while its data is brought to the current version of the app." `/health/ready` lists the tenant under `quarantined` with the versions involved. The log says `tenant catch-up: ...` when one is moved or held.

**A field the program stopped declaring** is left in the tenant's database, as it is in the deployment's own: no column is deleted, and the database records that it is at the new version. **A change a tenant's data cannot take** (a `unique` over duplicate values, or a constraint over a table that holds rows) keeps only that tenant closed. `/health/ready` gives its reason, for example `Bringing it forward was refused: not_class_a: ...`, and it is not asked again until the next version or a restart. The deployment owner decides what to do, for example by changing the program.

## Hot Swap

When `ordo apply` or `ordo rollback` activates a new version, each node independently loads it, runs the load gate, and either begins serving or stays on the old one. There is no restart and no drain. Requests in flight keep running on the version they started with; new requests see the new version.

A swap must pass shape verification because the gate runs on every swap, not only at boot. If the database schema has drifted since the last deploy, the swap refuses and the node keeps serving the old version. This is not a failure. It is how you learn the database state and the serving program disagree.

## Health Endpoints

**`/health/live`**: process liveness.
- Always returns `200 OK` with `{"status": "live"}`.
- Never consults a dependency. A health check that depends on the thing it probes causes one outage to become two.

**`/health/ready`**: readiness to *accept and run a deployment*, not
"is a program running" (owner-ruled 2026-09-20, 17t). A platform's job is
to receive programs, so a correctly configured deployment with **none
applied yet** answers `200`, the same as one serving traffic. Whether a
program is active is reported in the body, never in the status code.
Point a Kubernetes readiness probe at this endpoint (see
`docs/deploying.md`).
- Returns `200 OK` when every check below passes:
  - `database`: a pooled `SELECT 1` round trip.
  - `custody`: this deployment's own space opens under the configured
    key provider.
  - `config`: the *live* program's required configuration is present.
    Everything the binary refuses to boot without (token key, page key,
    master key, `ORDO_PG_*`) cannot be missing here; the one thing a
    **swap** can change after boot is whether the live program declares a
    non-inline `blob` field with no store attached, so that is what this
    asks.
  - `workers`: the workers this process must run are running (none
    quarantined). A process with no supervisor has nothing required, so
    it passes.
  - `deploy_path`: the database role this deployment connects as can
    create tables (`has_database_privilege(..., 'CREATE')`), which is
    what a first `ordo apply` needs to do.
  - `continuity`: the **control plane's own** space accounts for its
    history. A tenant whose chain has moved backwards is quarantined and
    named in `quarantined` below without touching this check or the status
    code (17aa): a tenant's data state never decides whether the
    deployment is ready, because the deployment staying up is what lets
    you record the restore that lifts the quarantine. An in-memory read,
    no I/O.
- Returns `503` with `{"status": "not_ready", "failed": [...], "checks":
  {...}, "program_active": bool}` if any check fails. `checks` gives one
  line per check, in `/health/deep`'s style, enough for a deployment owner to
  see *why*, e.g. which `entity.field` has nowhere to put its bytes,
  without a credential's value, a host, or a port.
- `program_active` is always present, in both the ready and the not-ready
  case: whether the live registry has at least one op. It is a fact about
  the program, independent of readiness.
- `quarantined` is always present too, and is a fact in the same category:
  an array of `{"space": ..., "reason": ...}` naming the spaces this
  deployment is holding closed and what it found in each. An empty array is
  the ordinary answer. A non-empty one on a `ready` deployment means every
  other tenant is being served and the named ones are not. See **The
  restore procedure** for what to do about it. The reason is the continuity
  finding: which record the chain no longer reaches, and from which
  database.
- Named by check (`"database"`, `"custody"`, `"config"`, `"workers"`,
  `"deploy_path"`, `"continuity"`), never by host or port. This is all an unauthenticated
  caller learns beyond what the check itself is about; a load balancer
  gets the signal it needs without credentials.

**`/health/deep`**: per-dependency detail.
- Access-controlled: requires a bearer token with the `platform.health.deep` capability.
- Returns `200 OK` with:
  - The deployment's key provider type (e.g., `TestProvider`).
  - Whether the database and key provider are reachable.
  - The current program version and op count.
  - Shape verification status: which ops were verified, which have no result shape (writes, deletes), which could not be posed to the database, and any live-schema drift.
- If drift is present, the deployment is serving but the database schema does not match what the program asserts. This happens if a deployment was pinned (booted directly without a registry) and later the database drifted, or if a schema change was applied after the program was deployed.

## Metrics

`/metrics` returns Prometheus text exposition format. It is access-controlled: requires the `platform.metrics` capability.

**What is exposed:**
- `ordo_denials_total{space, principal, principal_kind, op}`: failed access checks by space and principal.
- `ordo_key_release_total{space, purpose}`: how many times data was unsealed by purpose (for audit).
- `ordo_shape_verification_failures_total`: times the load gate refused a registry.
- `ordo_invalidations_total{source}`: staleness signals published.
- `ordo_metrics_series_dropped_total`: series refused by cardinality cap (if principals or ops exceed the limit).
- Histograms: `ordo_chain_append_lock_wait_seconds` (time blocked on the per-space advisory lock), `ordo_chain_append_seconds` (time inside the append critical section), and `ordo_op_duration_seconds{op,outcome}` (per-op service latency), all on a shared bucket ladder.

**What is NOT exposed:**
- No traces. OpenTelemetry spans are documented but not emitted.
- No structured logs.
- No push, no exporter, no aggregation across replicas. Counters and histograms reset when the process restarts.

Per-replica counters are summed by the scraper for a deployment-wide rate. One alert rule exists: `ordo_denials_total` over a threshold (set via `ORDO_DENY_ALERT_THRESHOLD`).

## Usage Metering

Off unless `ORDO_USAGE_LEDGER=1`. On, two tables in the control database
answer "what did this deployment serve": `$usage_replica` (requests by
cost class, rows returned, worker runs, one row per space per period per
replica, written by every replica's own `usage.flush` worker) and `$usage`
(writes and rows written from the command log, bytes from
`$sealedpayload`, `$blobref` and the live schema, one row per space per
period, written once a period by the leader's `usage.rollup` worker).
`bytes_rows_estimated` is the one estimated column: it comes from
`pg_class` statistics (`pg_relation_size`/`reltuples`), not an exact `SUM`,
because counting every entity table's row bytes exactly would mean a full
table scan per space per rollup. Every other byte and count column is
exact.

The class counted is the effective class the request ran at (a refinement
on a costlier field raises it above the op's registered class), the same
one the observed-cost numbers record. The columns are unchanged. Rows
written before the owner's 2026-10-23 ruling used the declared (registered)
class instead.

`usage.flush`'s cadence (`ORDO_USAGE_FLUSH_SECS`, default 60 seconds) and
`usage.rollup`'s (`ORDO_USAGE_ROLLUP_SECS`, default one hour) are both
configuration, and both reject a malformed or zero value at boot rather
than silently falling back. `docs/deploying.md` §8c states the trade each
one makes: a shorter flush costs one write per replica per interval and
loses less of a crashed replica's counters; a shorter rollup gives fresher
quotas and dashboards at the cost of more `$usage` rows.

A space's usage over `[from, to)` is `ordo_exec::usage::between`, summing
`$usage` and `$usage_replica` rows in that window: the read a quota check
or a billing report uses. The `GET /usage` route below serves this read
to a caller allowed `platform.usage`, and nothing else exposes the two
tables.

## Quotas

17n M2. Off exactly when metering is off: `ApiState.quota` is `PgQuotas`
when `ORDO_USAGE_LEDGER=1` and `NoQuota` (always `Within`) otherwise, so
turning on the ledger is also what turns on quota enforcement. There is
no second switch to remember. A space with no configured limit is never
refused either way.

**Configure a limit with `ordo quota set`, not an environment variable.**
A limit is per-space and changes without a restart, which a variable
cannot do:

```
ordo quota set --space <s> --quantity <requests|rows_returned> \
  --limit <n> --grace <n> --period-secs <n>
ordo quota show --space <s>
```

v1 gates `requests` and `rows_returned` only. Both are counted at the
door as the request happens (M1a), so the quota is exact. `writes` is
counted from the command log at rollup (M1c) and a quota on it would
notice an overrun up to one rollup period late, which is a quota in name
only; it is 18i, on the post-deployment roadmap, per the owner's ruling
(`corpus-rulings.md`, `write-quota`, 2026-09-19). `ordo quota set` refuses
any other quantity by name, at exit code 2.

**A limit has a grace, and it is a soft limit by default.** Over the
limit alone warns (`ordo_quota_over_limit_total{space,quantity}`
increments) and the request still serves; over limit **plus** grace
refuses with `quota_exceeded` (429). A deployment that wants a hard stop
at the boundary sets `--grace 0`.

**The gate runs after the access check, and that ordering is load-bearing,
not incidental.** A quota refusal discloses that the space exists and is
over its plan; handing that to a caller with no membership in the space
would be the same enumeration the door's uniform `access_denied` exists
to prevent (§4.1 of `api-engine-design.md`). So `gate()` checks quotas
immediately after `check_access` and never before: a non-member of an
over-quota space is answered `access_denied`, identically to a non-member
of any other space.

**The check is synchronous and reads a snapshot, never the database on
the request path.** `usage.flush` refreshes the snapshot on every tick,
after writing this replica's own `$usage_replica` rows. It builds the snapshot from `$quotas`
plus `SUM`s over `$usage_replica` for each configured `(space, quantity)`'s
own period, plus the live, not-yet-flushed counters `Metrics` is already
holding. A limit a replica's own traffic just crossed takes effect on its
very next request, not one flush interval later; a database outage cannot
become a door outage, the same trade `PgSessionRevocations` makes for the
deny list.

## Usage

17n M3, the read surface over M1's two tables. Two ways to read the same
records, and they say byte-identical things about the same closed period
because both render through `ordo_exec::usage::render`:

```
ordo usage --space <s> --from <secs> --to <secs> [--db <name>]
```

```
GET /usage?space=<s>&from=<secs>&to=<secs>
```

The route is access-controlled the way `/metrics` and `/security/alerts`
are: gated on the reserved op name `platform.usage`, checked against **the
deployment's own space**, never the `space` query parameter. `space=`
names whose usage to read, any tenant this process serves, and the
question "may this caller read usage at all" is answered once, for the
deployment, the same shape `/metrics` already has for principals and op
names.

Output is JSONL, one record per `(space, period_start)`: `schema` and
`method_version` (the counting method that produced the row, which can
predate a later change to what a column means), the `$usage` row's own
columns when the rollup has reached that period, `replicas` (one entry per
replica that flushed into it, with its own `requests`/`rows_returned`/
`worker_runs`/`state`), and `totals` summed across those replicas. A
period `usage.flush` wrote that `usage.rollup` has not reached yet still
gets a record: `replicas` and `totals` are there, the `$usage`-only
columns (`chain_from`, `writes`, the byte counts) are absent rather
than the period being dropped.

**Ordo meters. It does not invoice.** Pricing, plans, invoices, payment,
proration, credits, currency: none of that is here, and it is a boundary
rather than a backlog. This platform emits the facts a period actually
had; billing on top of them is a product somebody else builds, carrying
obligations (regulatory, financial) that no backend platform should absorb
on its users' behalf.

## Authentication

Off unless `ORDO_AUTH_METHODS` is set. Off is a legitimate shape: the
deployment verifies bearer tokens some other system minted, which is what
every Ordo deployment was until 2026-09-20. The startup census names the
four routes as unavailable and says which variable turns them on.

| variable | default | what it decides |
|---|---|---|
| `ORDO_AUTH_METHODS` | unset | comma-separated method names. `password` and `passkey` are served; any other name refuses the boot |
| `ORDO_SESSION_SECS` | `3600` | how long one token verifies |
| `ORDO_SESSION_MAX_SECS` | `43200` | the ceiling past which no refresh is granted, counted from the sign-in |
| `ORDO_AUTH_PROVISIONING` | `invite-only` | the **ceiling** on how open any tenant may make itself. `invite-only` allows the invitation way in and nothing wider. `auto-onboard` allows every way (open, provider, first-comer as well), and creates an account for an authenticator nobody invited. Each tenant then chooses within it, with `std.space.setAdmission` |
| `ORDO_AUTH_REGISTRATION_TTL_SECS` | `7776000` (90 days) | how long a sign-up is kept when no tenant admits the person, then removed with its links, passkeys and password. The startup log prints the value in use |
| `ORDO_AUTH_PASSWORD_PENDING_MAX` | `10000` | how many password challenges may wait unanswered at once, 10 to 1000000. Past it `/auth/begin` answers `429 too_many_attempts` naming this setting until some complete or expire (5 minutes). Each challenge id is 128 random bits |

Turning it on creates four tables in the control database at boot:
`$credential` (identifier, argon2id PHC string, `enrolled_at`,
`updated_at`), `$sessionrevocation` (session id, when it was revoked,
and the instant past which the row may be swept), `$principal` (id,
kind, display, `created_at`) and `$authenticator` (authenticator id,
principal, `linked_at`). All four are safe in the database backup:
`$credential` holds no reversible secret, `$sessionrevocation` holds no
principal, no client and no behaviour, and `$principal`/`$authenticator`
hold the same directory an operator could reconstruct from an invite
list.

**A store that cannot be opened refuses the boot.** There is no fallback
to the in-memory pair, because a deployment that authenticates people
until it restarts and then silently has no accounts is worse than one
that refused to start.

**The session length and the ceiling are a trade you own.** A shorter
token means a stale claim such as a presence stamp or a client id lives
less long, at the cost of a refresh round trip more often. A shorter ceiling
means a stolen token is worth less, at the cost of a ceremony more often.
A malformed or zero value refuses the boot rather than falling back, and
a ceiling below the token life refuses too, since it would make the
ceiling the real session length.

### What a logout costs, and what it does not

The door consults the deny list on **every** request: one lock read and
one set lookup, no I/O. The list is loaded when the process starts,
written through on every logout this process serves, and re-read on a
cadence. Nothing queries the database per request.

The consequence for more than one replica: a logout on one replica is
honoured by the others at their next refresh of the list, not instantly.
The window is bounded by that cadence and by the token's own expiry,
because a refresh reads the list on the way through, so a session cannot
be continued past the window whatever the door did in it. A
single-replica deployment sees a logout immediately.

`$sessionrevocation` is bounded by **logouts**, not by logins, and rows
are deleted at each session's ceiling. It is not a log and should not be
read as one: a row's absence means either "never logged out" or "logged
out long enough ago that every token was dead anyway".

### What is not built, and what a deployment owner should expect to hear about

There is no signup page, no email verification and no way to list a
principal's live sessions, by design: nothing records a session, only the
ones somebody ended. "Sign out everywhere" is `POST /auth/logout-all`, and
a person who has lost every passkey has the recovery paths in the next
section.

**The directory is durable (closed 2026-09-20).** Which authenticator
belongs to which principal is read from `$authenticator` on every login,
never cached, so a link or an onboarding is visible to the very next
request, including one served by a different replica. A principal id is
128 bits from the OS CSPRNG, never a counter, committed to `$principal`
with a uniqueness check rather than assumed unique: a colliding draw is
retried rather than minted. `ORDO_AUTH_UNSAFE_MEMORY_DIRECTORY` no
longer exists: there is nothing unsafe left in this path to gate.

## When a Person Loses Their Passkey

Ordo provides the mechanisms. The app developer decides what is enforced,
as settings with defaults, and nobody is required to follow one recovery
ritual. What follows says what each path proves, what it does, and what a
deployment owner runs.

### A second passkey, or a synced one

A person with two passkeys who loses one signs in with the other and
removes the lost one. Nothing weaker than a passkey was introduced, so this
path has no quarantine. A passkey that the platform syncs (iCloud Keychain,
Google Password Manager, a password manager) survives the loss of a device
without Ordo doing anything.

| route | body | answer |
|---|---|---|
| `GET /auth/authenticators` | a session's bearer | each authenticator with its `kind`, `state`, and for a passkey `backup_eligible`, `backed_up`, `registered_at`, `last_used_at`; `notices: ["single_authenticator"]` when the person holds one active way in |
| `POST /auth/authenticators/link/begin` | a session's bearer | `{"registration": <creation options>, "vouch": <request options>}` |
| `POST /auth/authenticators/link` | `{"registration": {…}, "vouch": {…}}` | `{"authenticator", "authenticators"}` |
| `POST /auth/authenticators/unlink` | `{"authenticator", "vouch": {…}, "end_sessions"?: bool}` | `{"removed", "sessions_ended"}`; `409 last_authenticator` for the last one |
| `POST /auth/logout-all` | a session's bearer | `204`; every session the person has ends, this one too |

The `vouch` in a link or an unlink is a fresh assertion from a passkey the
person holds, so a stolen session token alone adds nothing and removes
nothing. The audit event names the passkey that vouched. The challenge for
an unlink's vouch comes from `/auth/step-up/begin`. Removing a passkey
removes the link and the public key, and ends every session by default. The
sign-in answer says how many ways in a person holds and warns when it is
one. Nothing is enforced on the warning.

### Recovery codes

A person can hold a set of ten recovery codes on paper. Each is 24
characters from a 32-symbol alphabet (no `I`, `L`, `O` or `U`), shown as six
groups of four and 120 bits long. NIST SP 800-63B calls this a look-up
secret, and the rules it sets are followed: the codes come from the operating
system's random generator, each works once, and the table stores a hash and
never a code.

| route | body | answer |
|---|---|---|
| `POST /auth/recovery-codes` | a session's bearer, **with fresh presence** | `{"codes": [...10...], "count"}`, once, `Cache-Control: no-store`. The old set is void |
| `POST /auth/recover/code` | `{"code"}` | `{"case", "ticket", "ticket_expires_at", "remaining"}` |

Issuing needs a passkey assertion made in the last five minutes. A password
session, a stale session and a quarantined passkey are refused `403
presence_required`, so a stolen bearer token cannot mint a way back in. A
person who holds no passkey proves a first factor again instead (`/auth/complete
{"reauth": true}`, the handle in `x-reauth`). That stand-in is refused to
anyone who holds a passkey, a quarantined one included: they step up with a
passkey, and a passkey a recovery added counts once its quarantine ends. The
same rule covers the second-factor changes that take the handle. The
person is offered a set (the sign-in answer and the list say how many codes
they have, and the list warns at two or fewer). Nobody is required to hold
one.

**A code works at once.** Spending one opens a recovery case and returns the
recovery ticket in the same answer, so the person registers a new passkey
straight away. Everything that follows a recovery applies: the new passkey is
quarantined from step-up, every session ends, the passkeys they had can
contest, and the unused codes are voided (a new set is theirs to issue once
they have a passkey that carries presence). A contest voids every unused code
again, so codes the recovered passkey issued after its quarantine die with
it. A code alone signs nobody in.

A deployment that cannot accept a day of ordinary reads after a stolen code
sets `RecoveryPolicy::code_delay_secs`. The case then matures later and the
answer carries no ticket.

A deployment answers `429` to every redemption for the rest of a minute once
it has been sent 30 wrong codes in it. A wrong code names nobody, so the
throttle is the deployment's and not a person's. At 120 bits a guess is worth
nothing and the throttle only bounds the cost of being guessed at.

A recovery code is not a recovery phrase. The code proves identity to the
server, and the server holds only its hash. It restores an account and
unlocks no data in a space whose keys the deployment does not hold
(`docs/howto-custody.md`).

### The deployment owner, through the door

A tenant's owner who has lost their only passkey is restored by the
deployment owner, and by nobody inside the tenant (owner, 2026-09-29). A
tenant's manager cannot recover an owner, and neither can another owner of the
tenant: these ops answer only in the deployment's own space, which no tenant
member belongs to.

| op | params | what it does |
|---|---|---|
| `std.identity.recover` | `principal`, `evidence`, `reference`? | opens a case. Answers when it matures (a day by default) and no ticket |
| `std.identity.recoveryTicket` | `case` | collects the ticket once the case has matured, once |
| `std.identity.cancelRecovery` | `case`, `reason`? | ends a case that has not completed. `reason` is `opened-in-error`, `not-verified` or `person-withdrew` |
| `std.identity.closeContest` | `case`, `reason` | ends a completed recovery's contest window now. `reason` is `recovery-verified`, `recovery-disputed` or `opened-in-error`. Needs your presence |
| `std.space.succeedOwner` | `tenant`, `principal`, `evidence`, `reference`?; then `tenant`, `case` | makes an existing, active member the owner of a tenant whose owner is gone for good |

`evidence` is one of `vouched-by-members`, `verified-out-of-band`,
`contract-holder` and `host-break-glass`. `reference` is an opaque reference
into your own ticketing system: at most 120 characters from `A-Z a-z 0-9 . _ :
/ -`. **The platform records the class and does not check it.** It cannot tell
whether you verified somebody out of band, and the record says `checked=no`, so
an auditor can ask you for the evidence behind it.

During the day a case waits, any passkey the person still holds cancels it
(`POST /auth/recover/contest` with an assertion). When it matures you collect
the ticket and hand it to the person by whatever means you already reach them.

**What each stream says.** Your own stream names you (`by=<your principal>`), the
evidence class and the reference. Each tenant the person belongs to is told, in
its own stream, that the deployment owner acted, and never who, and never
another tenant. The tenant's owner reads that through `/security/events`. A
domain revert cannot rewrite it.

**What the person is told.** Every sign-in they make carries `recoveries` until
they acknowledge each one with `POST /auth/recover/ack {"case"}` from a session
with fresh presence. The acknowledgement is recorded. A person recovered a
minute ago has a passkey that carries no presence yet, so they cannot clear
the notice until the quarantine ends, and neither can somebody in their session.

**Owner succession.** When an owner has left or died, nothing needs a
credential: the act is a membership act. `std.space.succeedOwner` opens a case
with the tenant, the member, the evidence class and the reference, and answers
when it may be carried out (a day by default). Called again with the tenant
and the `case` after that, it makes the member an owner. It admits nobody,
touches no credential and ends no session. The departed owner stays a member
until the tenant's new owner removes them. It is recorded in your stream and
in the tenant's, the same two ways. The console does the same from the
Tenants screen, Owner succession: you type the tenant, the successor's id,
the id of each owner who has left, the evidence and your reference. It shows no list of the tenant's members.

**The tenant's managers are told, and may object.** While a succession is
open, and while a recovery through the door of one of a tenant's owners is
open, each manager of that tenant sees it in `owner_cases` at their next
sign-in or token refresh. A manager may object with
`std.space.objectToCase {case}` in the tenant. Nobody else in the tenant may.
An objection **pauses** the case: it does not mature, gives no ticket and
cannot be completed. You decide with `std.identity.resolveObjection {case,
decision, reason}`: `resume` (`evidence-rechecked`, `objector-withdrew`,
`objector-answered`) or `cancel` (`objection-upheld`, `opened-in-error`,
`not-verified`). `std.identity.openCases` lists every open case with each
objection and its resolution.

**Completing a succession ends the departed owner's delegations.** Opening
a succession needs `departed`: the ids of the owners who have left, each an
owner of the tenant at that moment, never the successor. Only they are
departed; any other owner keeps everything. By default, completing it ends
each named one's live delegations in the tenant and
the grant to the app that came with each. The record names every one, and
the new owner can grant again. To keep them, send `endGrants: false` with the
completing call (the console has a checkbox), or set
`ORDO_SUCCESSION_ENDS_GRANTS=off` for the deployment.

### A tenant owner, for the people the tenant created

| op | params | what it does |
|---|---|---|
| `std.space.recoverMember` | `member`; then `member`, `case` | opens a recovery for a member the tenant created. Returns the ticket at once, or when the case matures if a delay is set |
| `std.space.setRecoveryPolicy` | `hold` | turns hold on recovery on or off for this tenant |

A tenant owner may recover a member **only when the tenant created them
(`std.space.createPerson`), they belong to no other tenant, and they are not an
owner.** An owner is restored by the deployment owner, and by nobody inside the
tenant, another owner included. Every refusal is one sentence that says to refer
the person to the deployment owner. It does not say why, so an administrator
learns nothing about which other tenants a person belongs to. Which reason it was
is in the tenant's own security stream (`reason=not-managed-here` or
`reason=target-is-owner`). The moment a managed person is admitted to a second
tenant, the first tenant loses this power over them.

Whether a manager, and not only an owner, may do this is an open question for
the owner, and it is not built: only a tenant's owner can.

**Hold on recovery.** A credential belongs to the deployment and opens every
tenant its holder belongs to, so a recovered person's new passkey opens every
tenant they are in. A tenant that wants more turns hold on: when a recovery of
one of its members completes, their membership in that tenant is suspended and
stays suspended until an owner or manager reinstates them (`std.space.reinstate`,
which asks for presence). It is **off by default**, because it makes a person in
three tenants who loses a phone wait on three administrators. A tenant's only
active owner is never held, since nobody would be left to release it. That
person is the last-owner case, restored by the deployment owner.

### Checking that no link was written by hand

Nothing that can write the directory table can be stopped from writing a row, and
no recovery design changes that. Recovery makes it detectable. Every way a
link comes to exist (a sign-up, a ticket, a link ceremony, a recovery, a
password given) writes an event to the deployment's signed security stream, so a
link with no event behind it is one somebody wrote by hand:

```
ordo verify --identity [--security-key-id <id> --security-public-key <hex>]
```

It verifies the deployment's stream, then reconciles every link in
`$authenticator` against it. A link no event explains is listed with its
principal and its time, and the command **exits 3**, the code for a record that
is wrong. A principal's onboarding explains one link of theirs, the first, and
not a passkey somebody attached afterwards. **A stream trimmed by retention shows
the links it covered as unexplained**, and the report says from which position it
is kept. It is a command and not a worker: run it on a schedule if you want the
detection to be timely.

### What can be tightened, and by whom

The deployment owner sets the floors with `ORDO_RECOVERY_*` (`docs/deploying.md`
has the table). A tenant owner then chooses within them with
`std.space.setRecoveryPolicy`, naming what changes:

| setting | what it does | limit |
|---|---|---|
| `hold` | a recovered person is suspended in this tenant when the recovery completes | off unless the tenant or `ORDO_RECOVERY_HOLD_DEFAULT` turns it on |
| `delaySecs` | a longer wait on a case for the tenant's own members | never below `ORDO_RECOVERY_TENANT_DELAY_SECS` |
| `approvals` | more of the tenant's owners must approve a case | never below `ORDO_RECOVERY_TENANT_APPROVALS`; 10 at most |
| `pathEnabled` | `false` switches off the tenant's administrators' recovery of its members | the deployment owner can still recover them |
| `codesEnabled` | `false` switches recovery codes off for the people the tenant created | the person is told codes are off |

A tenant can only tighten. A tenant loosening a floor would weaken a credential
other tenants rely on, so a lower delay or a lower approvals count is refused
with the deployment's floor in the sentence.

**Several people agreeing.** When approvals are required, the opener is the
first. `std.identity.approveRecovery {case}` is another deployment owner's, and
`std.space.approveMemberRecovery {member, case}` is another owner of the tenant's.
Neither the person the case is about nor somebody who has approved already may.
Until it has enough approvals a case yields no ticket, whatever the delay, and
each approval is recorded. One persuaded person is not enough.

### The deployment owner, from the host

When the deployment owner's own last passkey is gone, no path needs a person
who can sign in except this one:

```
ordo identity recover --principal usr_… --as adam [--evidence host-break-glass] [--ref TICKET-12]
```

It needs the database credentials `ordo` already holds for its other
commands. It opens a recovery case for the person, with no delay, and prints
a recovery ticket once. The evidence class is one of `vouched-by-members`,
`verified-out-of-band`, `contract-holder` and `host-break-glass` (the
default), and `--ref` is an opaque reference into the deployment owner's own
ticketing system. No free text enters the security stream. The act is
recorded at deployment scope under the subject `platform cli`. It is not
mirrored into a tenant's stream from the host, because which tenants a person
belongs to is the running door's to answer.

The person spends the ticket with `POST /auth/recover/begin {"ticket"}` and
`POST /auth/recover/complete` (the ticket and a passkey registration). It
accepts nothing else: no route that takes a password asks for a recovery
ticket, so a recovery cannot end in a password. What completing does:

- registers a **new passkey**, which cannot satisfy step-up until its
  quarantine ends (one day by default, adjustable);
- **ends every session** the person had;
- **retires** the passkeys they had, and keeps them for the contest window
  (three days by default) instead of deleting them;
- removes every second factor they held, once the recovery completes;
- records the acts at deployment scope, mirrored into every tenant the person
  belongs to.

No token comes back. The person signs in with the new passkey, gets a session
that works for everything except step-up, and is told when step-up becomes
available (`presence_available_at`). Ordinary reads and writes work at once.
Export, import, delete and deploy ask for presence, and the answer is a
refusal until the quarantine ends.

### Contesting a recovery

The person whose passkey was retired may not have asked for the recovery. The
retired passkey cannot sign in, cannot step up and cannot vouch. Its holder
who tries to sign in is told (`409 recovery_contestable`, with the case and
the end of the window) and can contest it with `POST /auth/recover/contest`
and an assertion. A contest removes what the recovery added, restores what it
retired, ends every session again and records who contested. A passkey that
is still active can cancel a recovery that has not completed yet the same
way.

Someone who really lost a phone never contests, and at the end of the window
the worker `auth.recovery.sweep` removes the retired passkeys and their
public keys and closes the case. A thief holding the lost device who can also
pass its user verification can contest a legitimate recovery. That denies
service and grants nothing: the person asks the deployment owner again.

A contest also voids every unused recovery code and ends every session and
renewal of the person, so whoever held the recovered passkey keeps nothing.
It removes every passkey, other sign-in and second factor (TOTP, security
key) enrolled since the case opened, so the owner is never asked for a step
only that person can give. Each one removed is recorded as
`recovery.contest-removed` with its kind and id, in the person's stream and
each tenant's. The owner keeps the passkey they contested with. Second
factors the owner had before the case were removed when the recovery
completed and do not come back: the owner enrols them again.

A contest also puts back the sign-in settings that person changed since the
case opened (owner, 2026-10-25): the second-step level (for example "ask for
a second step after a passkey too") and the contact email. Each goes back to
its value when the case opened, and one the owner never had is removed. Each
is recorded as `recovery.contest-reset` with the kind and the old and new
value; an address is never in the record, only `address(<source>)` or `none`.

A person has one live case at a time (open, matured, or inside its contest
window). A second is refused until the first is contested or has run its
course, or until you close the window early.

**Closing a contest window early.** `std.identity.closeContest {case,
reason}`, in the deployment's own space, with your presence. Use it in two
cases:

- `recovery-verified`: the recovery was the owner's, and a thief with the
  lost device keeps contesting it. After the close nothing the case retired
  can contest it.
- `recovery-disputed`: the recovery was a thief's (a stolen code, say) and the
  owner has no device left to contest with. After the close you may open a
  new case for them at once.

Either way the retired passkeys stop working for contests, and the sweep
removes them and their keys within a minute. It is recorded in your stream,
naming you, and in each tenant's as the deployment owner.

**A passkey a recovery added cannot cancel a later case** while that
recovery's contest window, as it was first given, has not ended. Closing the
window early does not shorten this, so closing a thief's case to open the
owner's does not give the thief a veto.

## Browser Pages on Another Origin (CORS)

By default the door answers only pages on its own origin. A page on another
origin, such as a Cloudflare Pages site using the deployment as its backend,
gets no `Access-Control-Allow-Origin` and its preflight `OPTIONS` is refused,
so the browser discards every answer. The only sign is in the browser
console.

Set the allowlist and restart:

```
ORDO_CORS_ORIGINS=https://app.example.com,http://localhost:5173
```

Entries are exact origins, with no path and no trailing slash. `*` is refused
at boot because every call carries a bearer token. To check it from a
terminal:

```
curl -si -X OPTIONS https://<door>/op/listReadings \
  -H 'Origin: https://app.example.com' \
  -H 'Access-Control-Request-Method: GET' \
  -H 'Access-Control-Request-Headers: authorization, x-ordo-space'
```

An allowed origin answers `204` with `Access-Control-Allow-Origin` echoing
it. Any other origin answers `403`. Changing the list needs a restart, and
the console cannot set it yet. See `deploying.md` §5b.

## Backup and Restore

A deployment needs two backups. They answer different questions, and
neither substitutes for the other.

| | Postgres PITR (base backup + WAL) | The export bundle (`ordo apply`'s sibling, `std.space.export`) |
|---|---|---|
| Unit | the deployment: the control database and every database `PoolResolver::targets` lists | one space |
| Answers | "the disk is gone" / "a bad migration ran at 14:03" | "give me my data" / "reconstruct this one tenant" |
| Granularity | any point in time, all spaces at once | the state at export, one space |
| Contains keys? | wrapped keys only: `$keycustody` holds SMKs and DEKs wrapped under the deployment master, and the master itself is never a row (see below) | no; an open export decrypts under the owner's authority, a ciphertext export leaves envelopes sealed |
| Verifiable standalone? | no, it is a database | yes: every command-log group carries `prev` and `hash`, and every bundle member carries a `sha256` in `manifest.json`, so a reader recomputes both with no platform |
| Restores to | this deployment, this shape | anywhere, including a different deployment |
| Cost | continuous | a scan per export |

**Take both. Neither is optional if the other is missing a job.** PITR is
the RPO/RTO instrument: it is continuous, and it is the only thing that can
answer "restore this deployment to 14:03", because a per-space bundle has no
notion of a point in time. The bundle is evidential and portable recovery:
it is the only artifact a third party, including a departing tenant, can
check without this platform's help. A deployment that runs only PITR cannot
hand a tenant their data or prove anything to anyone; a deployment that runs
only bundles cannot recover from a bad Tuesday.

**PITR must cover every database, including the one a deployment owner forgets.**
With per-space databases, `PoolResolver::targets` names every target the
routing layer knows about. A WAL archive over the control database alone
restores routing while leaving the space databases it points at exactly
where they were, which is a deployment that boots pointing at data it
never backed up.

**Bundles are cut on a cadence, and before two acts specifically**: a
destructive migration, and an epoch destruction (`plan-production-ops.md`
Track K, checklist 17i). Neither is reversible by PITR either, because the
first drops columns and the second destroys the key the backup's ciphertext
needs to open. **The ordering rule an epoch destruction owes:** destroy an
epoch only after a backup, of either kind, taken since the re-seal that
retired it completed. A restore from a backup older than a completed
destruction returns payloads under a destroyed epoch, which `std.E.history`
already renders as an explicit unresolvable marker rather than a wrong
answer, but the backup itself cannot be made to open them; the ordering
rule is what keeps that from happening to a backup an operator actually
needs.

**The master key is never inside a backup.** `$keycustody`'s module doc
states the rule this deployment's boot enforces: *"wrapped keys in the
backup are the design; the master in the backup is the misconfiguration."*
`ordo-host` refuses to boot, and `ordo keys init` refuses to run, if any row
in `$keycustody` holds the master's raw bytes or the key derived from it,
naming the table and the remedy (remove the row, then `ordo keys
rewrap-master`). The master key itself, the sealed file plus its passphrase (or
`ORDO_MASTER_KEY` in development only), lives outside every backed-up database and
outside the Postgres data directory. A `pg_basebackup` of a data directory
that also holds the sealed-master file backs up a deployment's data next to
the one thing that must never travel with it.

### The restore procedure

A restore is four steps, and the platform refuses to serve **that space**
until the third of them is done. That refusal is the design: nothing can
stop somebody with `psql`, so what the platform does instead is decline to
serve a chain whose history has moved backwards without saying so.

The refusal is scoped to the space (17aa, 2026-09-21). The deployment
starts, serves every other tenant, and answers `/health/ready` as ready with
the affected space listed under `quarantined`. That is what makes step 3
possible at all: the door you record the restore through is up while the
space it names is closed. A deployment that died of one tenant's history was
a deployment you could not repair.

1. **Restore the database.** PITR to the moment you want, or re-import the
   space's export bundle. Nothing here is Ordo's; this is Postgres and your
   backup tooling.

2. **Start the deployment and read what reconciliation found.** The boot
   reads each space it holds a name for, the system space and
   `ORDO_SPACE`, compares each against the control database's record, and
   prints one line per space. A space it is holding closed gets the two
   lawful remedies under it:

   ```
   QUARANTINE  myspace: not served — the control database recorded a
               checkpoint at 1300 (head c07a…) that this space no longer
               reaches; the head is 999.
               run:  ordo restore --record --space myspace --as <you> --source …
               if this database was restored on purpose; otherwise restore
               the missing suffix.
               every other space is served, and this one is refused until
               the restore is recorded.
   ```

   A space on its own database is not read at boot and does not need to be:
   the same question is asked, by the same code, at the first request that
   names it, and it appears under `quarantined` in `/health/ready` from
   that moment. `curl -s localhost:8080/health/ready | jq .quarantined` is
   the check to run after any restore.

   **What reconciliation will settle on its own:** a difference somebody
   already wrote down. A recorded restore whose marker the chain reaches is
   explained, and the space serves. That is why step 3 works from a running
   deployment: the marker you write now is the marker the next boot reads.

   **What it will not do:** write a marker, guess one, or accept a rewound
   chain because it looks plausible. It repairs nothing in the database
   either: missing rows stay missing. A continuity read that *errors*
   holds the space closed as well, because "I could not check" is not "it
   checks out".

3. **Record the restore.**

   ```
   ordo restore --record --space myspace --as ops@example.com \
     --source pitr --at 1757003600 \
     [--control-db <name>] [--pre-restore-head <idx>:<hex> | --unattested]
   ```

   `--source` is required: a restore from an unidentifiable source is
   refused. `--at` is **seconds** since the epoch, the unit the chain uses
   everywhere. For a bundle, `--source bundle --digest <hex>`, where the
   digest is the `manifest_sha256` that `std.space.export` reported when the
   bundle was cut.

   `--as` names the human who ran the restore. It is **asserted, not
   authenticated**: this tool holds database credentials and attributes,
   the same posture `ordo keys init --as` takes. There is no step-up here
   and there cannot be one, because the door is closed at exactly the moment
   the marker is needed. What ties the marker to an authenticated record is
   the correlation it shares with the `custody-action` written to the
   security-event stream by the same command.

   `--pre-restore-head` says what the chain reached before the restore.
   Leave it out and the command defaults it, in order: the newest checkpoint
   the control database witnessed, then the newest checkpoint the space's
   own table holds, then nothing. The output says which of the three it
   took. `--unattested` forces nothing, which is a legal and **disclosed**
   answer: it means nobody could say how long the chain was, never that
   nothing was lost.

   If ordinary writes landed between the restore and this command, pass
   `--restored-from <idx>` to name where history actually broke; the command
   prints the gap.

   A re-run is safe and is the repair for a half-recorded restore: if the
   marker is in the chain but the control database did not take its row, run
   the same command again and it completes the record. `--again` is only for
   a genuine second restore to the same point.

4. **Verify.**

   ```
   ordo verify --space myspace --control-db <name> \
     [--attestation <file>]… --key-id <id> --public-key <hex>
   ```

   Exit 0 with one `restore` line per marker is a recorded restore, which is
   lawful history. The space is served again from the request after step 3,
   with no restart, and `quarantined` in `/health/ready` empties with it. `--control-db` compares against the heads the control
   database witnessed; without it the output says the witness was not read,
   rather than implying the records agreed. `--attestation` takes a document
   a receiver holds, repeatably, and compares it the same way.

**Which database holds the record, and the one case nothing does.** Every
signed checkpoint head is recorded in `$chainwitness` in the **control**
database, which a restore of a space's own database cannot roll back. In a
routed deployment that is a different database from the space's, so a PITR
of the space's target leaves the witness standing and the restore is caught.
In a single-database deployment there is no other database, so a PITR of the
whole thing rolls the witness back with the chain and reads as intact. That
case is caught by the receivers instead: a head you sent them that this
chain has moved away from, with no marker attesting a pre-restore head at or
above it, is a restore nobody recorded, whatever this deployment's own
records say about each other. That is what `ordo verify --attestation` is
for, and it is the reason to keep the attestation documents your receivers
send back.

**The drill.** `crates/ordo-host/tests/restore_drill.rs` runs all five steps
of `implementation.md`'s drill against a real Postgres on every test run: a
backup, writes after it, a restore, the quarantine, the record, verification
from both anchors, a schema-drift check, a row-for-row comparison against
the pre-backup snapshot, and the bundle re-hashed against its own manifest.
It prints its wall time and says in the line that the number is **not a
gate**: a laptop cluster's timing is not a release criterion, and your
deployment produces the number that is.


## Verifying the Audit Chain

`ordo verify --space <space-id>` recomputes a space's hash chain to detect tampering.

**Command:**
```
ordo verify --space myspace
ordo verify --space myspace --from-genesis
ordo verify --space myspace --key-id my-key-id --public-key <hex> 
```

**Healthy result (exit code 0):**
```
space   myspace
anchor  genesis
VERIFIED 1247 groups · head index 1246 = abc123def456…
```

Or, if a checkpoint exists and the key is supplied:
```
space   myspace
anchor  checkpoint upto 1200 signed by my-key-id
VERIFIED 47 groups · head index 1246 = abc123def456…
```

**After a recorded restore (exit code 0):**
```
space   myspace
anchor  checkpoint upto=1000 at=1757000000 signed by `my-key-id` (signature verified)
VERIFIED 48 groups · head index 1047 = 9f2c… · 1 restore discontinuity at index 1000
restore   index 1000 · restored from 999 = 4b1e… · pitr at 1757003600 · operator ops@example.com · pre-restore head 1299 = c07a…
orphaned  checkpoint upto 1300 head c07a…, explained by restore at index 1000, kept
```

A restore marker is a chain entry, attributed to `system:restore`, that
records that the space's database was restored and where its history
resumed. `verify` reports it and exits 0, because a recorded restore is
lawful history.

**A checkpoint counts when the stored chain agrees with its head.** That is
the rule the anchor and the orphan list both come from. A checkpoint at
position `upto` names a head; the chain holds a hash at `upto - 1`, or a
`prev` at `upto` after a prune cut exactly there. When the two match, the
checkpoint is current history and may anchor verification. When the chain
holds a different hash, the checkpoint was signed over history a restore
discarded: it is **orphaned**, it never anchors, and it is listed and kept.
Its row stays in `$checkpoints`; deleting it is what a dishonest operator
would do. When both entries that could confirm it have been pruned the
chain has no opinion, and the checkpoint is neither anchored on nor listed.

Because the chain decides, a prune that removes a marker does not disturb
verification, and a restore does not stop a space anchoring: after it, the
checkpoint worker signs the new head at the position the orphan already
holds, and `$checkpoints` keeps both rows. The two are told apart by head,
which is why the table is keyed by space, position **and** head.

**An orphan has to be explained.** What explains one is a marker's
`pre-restore head`: a marker attesting a head at or above the orphan's
position says the restore is known to have discarded it. An orphan no
marker bounds is a restore nobody recorded, and it fails verification
(exit 3) naming the checkpoint, whether the chain is shorter than the
checkpoint says or has grown a different history at that position.
A marker whose `pre-restore head` is `unattested` explains nothing: absent
means nobody could say how long the chain was, not that nothing was lost.

Each restore gets its own `restore` line. `--from-genesis` still proves the
prefix below the restore.

Nothing in the tool writes a marker yet: `ordo restore --record` is
increment R2c of checklist 17j.

**Broken chain (exit code 3):**
```
space   myspace
anchor  genesis
FAILED  tamper at index 487: HashMismatch
```

Or:
```
space   myspace
anchor  genesis
FAILED  tamper at index 20: Unreadable (body cannot be decoded as a command group)
```

**Usage error (exit code 2):**
```
`--space` is required
```

**Exit codes:**
- `0`: the chain is clean from the anchor to the head (or the space has no history).
- `3`: the chain is broken. The message names the index and the tamper kind (`HashMismatch`, `Unreadable`, `LinkBroken`).  This is separate from exit code 1 deliberately: a verification failure pages the security team; a transient database error pages the ops team.
- `2`: usage error (missing flag, unknown flag, bad flag value).

By default, verification starts from the latest signed checkpoint if one exists and a verifying key is supplied. Use `--from-genesis` to check from the beginning. A checkpoint is signed with the deployment's Ed25519 key; the verifying key and key id must match exactly. Every receiver who has the published key can verify the checkpoint independently. If no checkpoint exists yet or no key is supplied, verification starts from genesis.

**To sign a checkpoint:**
```
ordo checkpoint --space myspace
```

Output includes the signature and the public key to publish. Checkpoints are the only legal cut point for `ordo prune`.

## Retiring a Trigger Listener

Every trigger listener has a cursor in the space's consumer table. The cursor holds the command log's pruning floor. It is never evicted, because that is what at-least-once delivery means.

A deploy that renames or removes a listener leaves its cursor behind. The log then stops shrinking. `ordo prune --space <spc> --dry-run` names the cursor that holds the floor. `/health/deep` lists every such cursor under `supervisor.orphaned_listeners`.

The supervisor never removes one on its own. During a rolling deploy a replica on the old version may still serve the listener. Retiring a cursor is a deliberate act by the deployment owner, after the rollout is finished.

**To see the cursors:**
```
ordo triggers ls --space myspace --host https://ordo.example.com --token $ORDO_TOKEN
```

Each line shows the consumer name, its kind, its position, how long since it moved, and its standing. The standing is `orphaned`, `live` or `unknown`. It comes from the running deployment's own census, so `--host` and `--token` are needed to get one. Without them every listener reads `unknown`.

**To retire an orphaned listener:**
```
ordo triggers deregister --space myspace --listener 'digest:*.tasks.created' \
  --as usr_adam --host https://ordo.example.com --token $ORDO_TOKEN
```

The command refuses in three cases. Nothing is changed in any of them.

- The name is not registered. Run `ls` to see the names.
- The cursor is not a listener. Mirrors and archives have their own way out.
- The deployment still serves the listener, or its standing is `unknown`. Removing that cursor would reset the at-least-once guarantee for it. If you are sure the rollout is finished, add `--force`.

**The record.** The delete and the record commit in one transaction. The record is a `CustodyAction` in the space's security stream. Its op is `ordo.triggers.deregister` and its action reads `listener_deregistered position=<n> forced=<true|false> listener=<name>`. It carries the time, the space, the listener, the position the cursor held, and `--as`. `--as` is attribution and not authentication. Unset, it is recorded as `operator:unattributed`. `ordo verify` walks the record like any other entry.

After it runs, `ordo prune --space myspace --dry-run` shows the floor moving to the next holder.
