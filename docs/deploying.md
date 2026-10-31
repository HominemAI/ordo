# Deploying Ordo

What a deployment owner has to decide, and what the platform decides for them.
Every number here is a real default in the code.

Setting up your own deployment for the first time?
[`docs/first-deployment.md`](first-deployment.md) takes the deployment
owner from the secrets script to a signed-in console and a deployed
program.

The deployment owner runs a deployment from the console
([`docs/howto-console.md`](howto-console.md)). Deployment people live in
the identity service ([`docs/howto-identity.md`](howto-identity.md)). A
deployment without it (an upgrade that names no
identity database) can still name owners of its own space at boot with
`ORDO_DEPLOYMENT_OWNERS` (the table in section 1).

New to Ordo and want the short path from nothing to a deployed app first?
[`docs/quickstart.md`](quickstart.md) is that path; this is the full
operator reference it points back to.

---

## 1 · The shape of a deployment

```
  HTTP  ─→  ordo-api      the door: method mirror, envelope, access check,
                          idempotency, page tokens. No per-op code.
            ordo-host     the wiring: specs derived from the program,
                          IdentityAccess at the door, executor beneath.
            ordo-exec     connections, transactions, the venue gate.
            Postgres      your database. BYO.
```

`ordo-api` depends on no sibling crate: its `OpRegistry`, `OpExecutor`,
`AccessChecker`, `InvalidationBus`, and `ReplayCache` are the seam. If you
are embedding Ordo in something larger, those five traits are the whole
contract, and `ordo-host` is a worked example of fulfilling them rather
than a framework you must adopt.

### The server binary

`ordo-host` is the deployable. It binds a socket, and it is configured
**entirely by environment**: no config file, no flags. An orchestrator
supplies environment; an image manifest makes it reviewable; and every
loader already in the tree is an environment reader, so a second mechanism
would mean two places to look.

```
ordo-host          the server
ordo               the operator tool (apply, prune, checkpoint, revert)
```

Two binaries, not one, and the split is deliberate: an operator tool and a
serving process have different blast radii and different bases. The *role*
half of "single static binary, role by config" is honoured by
`ORDO_WORKERS` (below); the single-binary half is not, and should not be.

**Three refusal postures**, and which one a variable gets is a decision:

| posture | meaning |
|---|---|
| refuse to boot | the deployment is unsafe or incoherent without it |
| refuse the feature, boot, and say so | the platform is coherent without it, and pretending otherwise turns a missing option into an outage |
| default, and publish the default | a tuning knob with one obvious right answer |

What is never available: silently defaulting a secret, and silently
disabling a mandatory control.

**Every environment refusal happens before the first dial.** A deployment
missing its token key fails in milliseconds with one message rather than
after a pool timeout against a database it should never have touched, and
a misconfigured replica in a rolling deploy dies before it takes a
connection slot from a healthy one.

| variable | posture | notes |
|---|---|---|
| `ORDO_PG_HOST` | **refuse** | the pool's own default is a socket directory on a laptop |
| `ORDO_PG_USER` | **refuse** | |
| `ORDO_PG_DB` | **refuse** | `ordo_conformance` is a test database's name |
| `ORDO_PG_PORT` | default `5432` | |
| `ORDO_PG_PASSWORD`, `ORDO_PG_PASSWORD_FILE` | optional | **the pool does not read `PGPASSWORD`**: `tokio-postgres` reads no environment at all. Prefer the `_FILE` form. Unset is legitimate (peer auth, a socket, an IAM sidecar). |
| `ORDO_PG_POOL_SIZE` | computed | `available_parallelism().clamp(2, 8)` |
| `ORDO_PG_ACQUIRE_TIMEOUT_MS` | default `2000` | `Saturated` is retryable by design, and a caller sees it as `503 server_busy` (§2a) |
| `ORDO_PG_SPACE_RESERVE` | computed | connections held out of **one space's** reach, default `max(1, size / 4)`. Refuses at boot if it is not smaller than the pool (§2a) |
| `ORDO_PG_STATEMENT_TIMEOUT_MS` | default `30000` | the bound on a single statement, pinned on every pooled connection at dial. `0` is no bound (§2b) |
| `ORDO_REGEX_MAX_LEN` | default `256`, `1` to `10000` | the longest `matches` pattern a statement runs, in characters. A longer one, or one holding a backreference (`\1` to `\9`), is refused as `invalid_param` before the regex engine runs it |
| `ORDO_PG_IDLE_SECS` | default `30` | how long a connection may sit unused before the pool closes it, and how long a tenant's database keeps its live-update listener after its last open feed ends. Pools dial on demand and hold **no minimum**, so a tenant nobody is using holds no connection within this period. `0` keeps every connection once dialled. With a database per tenant this is the setting that decides what an idle tenant costs the server (25g). A busy tenant still holds up to `ORDO_PG_POOL_SIZE` connections |
| `ORDO_TENANT_ATTACH_HOSTS` | **deny everything** | comma-separated hosts a tenant may be attached to: **both** a database host and a bucket endpoint's host. **Unset means no tenant can be attached anywhere**, so `std.space.create`'s `$db` and `std.space.attachBucket`'s `$endpoint` are both refused until it is set (§2c) |
| `ORDO_TENANT_CATCHUP_SECS` | default `15` | how often each replica looks for tenants whose database is behind the program this node serves and brings them forward (25i). A tenant's own database is moved by an apply through the door, and by this catch-up when the apply reached only the deployment's own database (`ordo apply --db`, which is what `examples/notes/dev/serve.sh` runs), when the tenant was created from the previous version, or when its database was down at the time. A swap and a boot each ask for a pass at once, so this sets only how soon a tenant that was unreachable is found again. A change the tenant's data cannot take is not retried until the next version; the tenant stays closed and `/health/ready` gives the reason |
| `ORDO_NODE_BEAT_SECS` | default `5` | how often each node writes its row in `$nodes`: the version its door serves and whether its tenant catch-up for that version has finished. `ordo apply` waits on these rows, and `GET /health/deploy` shows them (§7, "When a new version is served"). A row older than three beats counts as a stopped node. `0` refuses the boot |
| `ORDO_PG_TENANT_HOST` | the deployment's own | where a tenant's database is created when the call names no cluster. Per field, so changing the host keeps the port (§2d) |
| `ORDO_PG_TENANT_PORT` | the deployment's own | |
| `ORDO_PG_TENANT_USER` | the deployment's own | |
| `ORDO_PG_TENANT_CREDENTIAL` | `$deployment` | a credential **label**; the reserved `$deployment` means this deployment's own password. A label of its own needs `ORDO_TENANT_CREDENTIALS` (§2d) |
| `ORDO_PG_ATTACH_HOSTS` | **deny everything** | the same list under the name 18d shipped. Read **in addition** to the variable above, not instead of it |

<a id="2c"></a>
### §2c · The attach allowlist, and why it denies by default

A tenant can be given a database of its own by passing `$db` and its
siblings to `std.space.create`, and a bucket of its own through
`std.space.attachBucket`. The host on either call is **an address this
deployment will dial, chosen by whoever made the call**, which is the class
of thing `ordo-egress` exists to refuse for outbound HTTP. A Postgres dial
does not pass through that kernel, and an S3 dial is that exact shape, so the
allowlist is applied at attach.

**One list for both.** Owner-ruled 2026-09-21: the bucket endpoint allowlist
*"should be tied in with the db allowlist setup. implication really is if i
defined it on my platform it is allowed."* So there is one set of hosts, and a
host on it may be a tenant's database server or a tenant's bucket endpoint.

```
ORDO_TENANT_ATTACH_HOSTS=db-a.internal,db-b.internal,10.0.0.4,s3.eu-west-1.amazonaws.com
```

`ORDO_PG_ATTACH_HOSTS` is the name 18d shipped and it still works. Both are
read and the two lists are **unioned**, so setting the newer name does not
turn the older one off. Set one or the other; setting both is not an error and
gives you their union.

Exact hosts, comma separated. A hostname matches case-insensitively; a Unix
socket directory (anything starting with `/`) matches case-**sensitively**,
because it is a filesystem path and two cases can be two directories. There
is no wildcard and no suffix match: a list naming `db.internal` does not
admit `evil-db.internal`.

**Unset denies everything.** That is the same position every other
address-shaped control here takes (a credential label this node does not
hold is refused before any dial), and it is chosen on purpose: an allowlist
that defaulted to open would protect only the deployments whose operator
already knew to configure it, which is the set that needed it least. The
practical consequence is that **per-tenant databases and per-tenant buckets
do nothing until you set this**, and the refusal a caller sees says so without
naming the list.

The refusal carries **one reason and no detail** about why a host was
declined. A refusal that distinguished "not on the list" from "malformed"
would be an oracle a caller maps one attempt at a time.

**This does not gate the deployment's own database or its own bucket.**
`ORDO_PG_HOST` and `ORDO_BLOB_S3_*` are the operator's own configuration and
are never checked against this list; attaching a tenant to that same database,
or to that same bucket, is refused outright, by identity rather than by name,
so `localhost` and `127.0.0.1` are not two answers to one question and neither
are `https://s3.example.com` and `https://s3.example.com:443`.

<a id="2d"></a>
### §2d · `ORDO_PG_TENANT_*`, and the four clusters

**Every tenant has a database of its own.** Ruled 2026-09-25: *"there exist a
default cluster which databases are created in. 1 tenant one database."* The
database name is minted from the tenant's space id, so it is never a caller's
to choose and two tenants cannot collide on one.

What a setting chooses is the **cluster**, and there are four levels:

1. The **console's** own database, which belongs to the console rather than to
   any deployment it manages.
2. The **deployment's** own: `ORDO_PG_HOST` and its siblings. This is the
   control plane, and no tenant is ever put in it.
3. The **default cluster for tenants**: `ORDO_PG_TENANT_HOST` and its
   siblings, where a tenant's database is created when the call names nothing.
4. A **per-tenant** cluster, named in full on the `std.space.create` call.

**Each level defaults to the one above, so configuring nothing is a valid
configuration.** *"someone deploying could use the same cluster for all 3 if
they chose."* Leave `ORDO_PG_TENANT_*` unset and every tenant gets its own
database on the deployment's own server, which is the ordinary small
deployment.

The fallback is **per field**. An operator pointing tenants at another server
usually changes the host and keeps the port, and a setting that demanded all
four would be three more chances to mistype the ones that were not the point.

**`ORDO_PG_TENANT_CREDENTIAL` is a label, and its default is reserved.**
`$deployment` means "the password this deployment already holds", which is what
lets level 3 work with no credentials directory installed at all. Any other
value is looked up in the credential provider (`ORDO_TENANT_CREDENTIALS`).
With no provider set, a boot that names such a label is refused.

**There is no routing setting.** Every tenant always gets its own database. A
leftover `ORDO_PG_ROUTING=attached` boots with a warning that the setting is
gone, so remove the line. `ORDO_PG_ROUTING=single`, or any other value, refuses
the boot.

**These hosts need no allowlist entry.** `ORDO_PG_ATTACH_HOSTS` exists to
refuse an address *chosen by data*; a cluster the operator set in the
deployment's own environment is not that, and is allowed by definition (§2c,
D-S6: *"if i defined it on my platform it is allowed"*). Without that, an
unset allowlist would refuse a deployment its own default tenant cluster and
no tenant could ever be created.

**Check the plan before you make the tenant: `dryRun`.** Pass `"dryRun": true`
to `std.space.create` and the call runs its preflight and stops: it resolves
your parameters to a cluster, mints the database name it would use, dials that
cluster to prove it answers, and returns:

```jsonc
{ "space": null, "name": "Acme", "owner": "customer_ada", "reachable": null,
  "plan": { "target":   "db.internal:5432/spc_7kq…",
            "database": "spc_7kq…",
            "version":  "a13f…",          // the schema it would receive
            "databaseStanding": false,    // it would create, not adopt
            "occupancy": 0 } }
```

Nothing is reserved and no id is minted, so two dry runs in a row both
succeed and neither leaves anything to clean up. It refuses everything the
real call refuses (a host off the allowlist, the control database, a
database another tenant already sits on, a cluster that does not answer), so
a dry run that passes is a call whose remaining failure modes are
infrastructure faults rather than mistakes.

That distinction is the point: a refusal **before** anything is built says
"Nothing was created" and leaves no registry row. A failure **after** names
the tenant it left and tells you that releasing it is `std.space.reap`. If
you see a tenant in the troubled list, something really did get part-built.

**Every call to `std.space.create` needs an `Idempotency-Key`, including the
dry run, and each needs its own.** The header is optional on every other write
in the platform and mandatory here, because this op's effect reaches outside
the database it writes to: without a key, a retry after a network failure
mints a second KMS key and a second database, answers with the second tenant's
id, and leaves the first for somebody to find by reading the cluster. Both
rows are `active` and correct, and nothing reconciles them. A call with no key
is refused with `missing_param` before anything is built.

Use a **fresh key per call**. The replay cache binds a key to a fingerprint of
the request body, so reusing a dry run's key for the real call is refused as
`idempotency_mismatch`: the safe direction, since the alternative would be
handing you the plan again with `"space": null` and a 200 for a call you meant
to run, but still a wasted round trip.

**Reuse a key deliberately, to find out whether a call landed.** That is the
case the requirement exists for. If a create dies in transport you cannot tell
"the door never saw it" from "a tenant was built and the answer was lost", and
those need opposite next steps. Re-send with the *same* key and the door
replays the first answer instead of building a second tenant. `ordo space
create` mints a key per invocation, prints it if the call fails in transport,
and takes `--idempotency-key` so you can pin it:

```
ordo space create --name "Acme" --owner usr_ada --host … --token … --space …
# REFUSED  https://…/op/std.space.create: connection reset
#          re-run with --idempotency-key ordo-space-create-4821-17a9f… to
#          learn whether this one landed, rather than building a second tenant
```

**A plaintext bucket endpoint needs `ORDO_DEV_UNSAFE_BUCKET_ENDPOINT=1`**, and
being on the allowlist is not enough. The S3 binding admits `http://` to
`localhost`, `127.0.0.1`, `::1`, `*.localhost` and the bare name `minio`,
which a self-host laptop with a local gateway needs; in a pod it is a request
into the deployment's own network carrying the SigV4 credential in the clear.
This is a separate flag from `ORDO_DEV_UNSAFE_BLOB_STORE`, which is about the
deployment's own store being a filesystem: a laptop that wants a local MinIO
for a tenant has not agreed to put customer files on a directory. An `https`
loopback endpoint needs no flag: the allowlist alone governs it, because the
credential is not on the wire.
| `ORDO_HOST_ADDR` | default `127.0.0.1:8080` | loopback by default: a `cargo run` must not put a door on the network. The image sets `0.0.0.0:8080` explicitly. |
| `ORDO_SHUTDOWN_GRACE_SECS` | default `30` | see below |
| `ORDO_SPACE` | **refuse** | a deployment whose tenant is named `conformance` is not a deployment. Set it to `system` for a first deployment that has no tenant yet; set it to a tenant only once that tenant has custody, or the boot refuses (§4c). |
| `ORDO_DEPLOYMENT_OWNERS` | default unset | **Read only by a door without the identity service.** With `ORDO_IDENTITY_URL` set it is ignored, never parsed and never a refusal, and the boot log names it: deployment people live in the identity service (`ordo-identity-service invite`), and an owner row it wrote before stays as an owner of the home space, with no platform rights. Without the identity service: comma-separated **principal ids made owners of the deployment's own space** (`ORDO_SPACE`) at boot. This is how a deployment gets an owner without anybody arriving first: first-comer enrolment is off by default, and setup belongs on the server. **It adds and never removes**: an owner already there is left alone, so a restart writes nothing, and removing a deployment owner is `std.space.remove`, after which you edit the variable to match (a removed owner the variable still names is added again on the next boot). **Naming someone who has not signed in yet:** a principal id is minted when a person registers, so it cannot be known ahead. The person registers on your app's page like anyone, you read their id from the page or the principal directory, set this variable to it and restart. The row is written whether or not the principal exists yet, and it waits for the person. A principal that already holds a lower role or a suspended row keeps it and the boot log says so; a role or a suspension is somebody's act, and changing it is `std.space.setRole` or `std.space.reinstate`. On `ORDO_SPACE=system` before the first `ordo apply`, the `members` table does not exist and the rows are written by the first deploy the running node sees, or by the next boot. Each row written is recorded on the deployment's stream as `Enrolment { act: "deployment-owner" }`, and the startup log names it. An id the door cannot address (letters, digits, `_` and `-` only) refuses the boot. |
| `ORDO_KEY_PROVIDER` | **refuse** | `sealed-master` \| `local` \| `test`. Explicit, never inferred from which other variables are set. A deployment uses `sealed-master`. `local` and `test` are development only. |
| `ORDO_MASTER_KEY_FILE`, `ORDO_MASTER_KEY_PASSPHRASE_FILE` | required by `sealed-master` | the file `ordo keys seal-master` wrote, and its passphrase file (keep the passphrase in the secret store) |
| `ORDO_MASTER_KEY` | required by `local` | development only. One HMAC is the whole stretch, so a short secret can be guessed offline from a database backup. |
| `ORDO_DEV_UNSAFE_TEST_KEYS` | default off | the development flag. It makes `ORDO_KEY_PROVIDER=test` selectable and is required by `ORDO_KEY_PROVIDER=local`, which refuses to boot without it (S6-009). |
| `ORDO_TOKEN_KEY` | **refuse** | 64 hex characters. A defaulted token key means every deployment built from this source can mint tokens every other one accepts. |
| `ORDO_PAGE_KEY` | **refuse** | 64 hex characters. A shared page key makes a cursor minted in one deployment valid in another. |
| `ORDO_CHECKPOINT_KEY`, `_KEY_ID` | refuse the **feature** | unset disables checkpointing and attestation, and the startup log says so. `ordo checkpoint` still works by hand. |
| `ORDO_SECURITY_CHECKPOINT_KEY`, `_KEY_ID` | refuse the **feature** | unset means security-stream retention reports INACTIVE and prunes nothing, rather than cutting at a position no signature vouches for |
| `ORDO_BLOB_STORE` | conditional | `s3` (with `ORDO_BLOB_S3_*`) or `fs:<path>`. **Required exactly when the loaded program declares a stored `blob` field**. See §4b. |
| `ORDO_DEV_UNSAFE_BLOB_STORE` | default off | the only thing that makes `fs:` selectable; `FsStore` is a test double with no durability story |
| `ORDO_EXIT_BUNDLE_DIR` | refuse the **feature** | where `std.space.export` writes bundles, one sub-directory per space. Unset leaves `std.space.export` and `std.space.delete` unregistered (they answer as any unknown op) and the startup log says so. Set, it must be an absolute path the process can create, or the boot refuses. The bundle holds the whole space: put it on a volume with the access controls you would give a database dump. |
| `ORDO_IMPORT_BUNDLE_DIR` | refuse the **feature** | where `std.space.import` READS a staged bundle. Unset leaves `std.space.import` unregistered and the startup log says so. Set, it must be an absolute path that **already exists**: this one is an input, so the boot checks it rather than creating it, because an empty directory nobody meant to point at answers every import with `bundle_unknown` and looks like a bug in the op. A bundle is staged at `<root>/<space>/<name>` and the caller names only `<name>`. **The space component is an authorization boundary**: one root serves every tenant, so a flat layout would let any tenant's owner load another tenant's staged bundle into a space whose keys they hold. A **people map** is staged beside the bundle as `<bundle>.people`, a JSON object of source member id to destination member id: rows that name a person resolve through it, and any name it does not cover stops the import. It may only name people who are already members of the destination, so a staged file can never hand out access. |
| `ORDO_PRESENCE_ON_READS` | default `off` | `on` or `off`. **On asks for a fresh passkey assertion on platform reads too**, for a strict deployment. Off, a signed-in person reads with no touch: owner, 2026-10-22, "If you are signed in = that passkey was your credentials - you are authenticated." A read is an op whose kind is `Read`, served on `GET`. A read freed this way stays closed to delegated tokens. Any other value refuses the boot. The startup log says which. |
| `ORDO_STEP_UP_EXEMPT_OPS` | default unset | comma-separated **platform ops that do not require a fresh passkey assertion**, each an exact name or a prefix ending `.*`: `std.space.admit,std.space.setRole`, or `std.space.*`. **Unset is presence required on every platform op that changes something**, and a change nobody names keeps asking, including one added by a later release. A platform op that only reads asks for none (owner, 2026-10-22) unless `ORDO_PRESENCE_ON_READS=on`. Which ops ask is your application's decision (owner ruling, 2026-09-29), and this is where you make it, for any of them: `std.deployment.apply`, `std.space.create`, `std.space.import`, `std.space.export`, `std.space.delete`, the membership and admission ops, the tenant provider ops (`std.space.addConnection`, `std.space.updateConnection`, `std.space.retireConnection`, `std.space.connections`), and the rest. The role gate is unchanged: an exempt op is still refused to a caller the engine does not list for it. **An exempt op is also one a capability token may be granted**, because the rule that keeps a delegation from satisfying step-up has nothing to say about an op that asks for none. An entry that names no platform op refuses the boot and lists the ones that exist. The startup log names what is exempt. |
| `ORDO_ACCESS_TEMPLATE` | default unset | the app developer's **default for who may call each program op**: `reportByProject=owner,manager; reportByPerson=owner,manager`. Entries split on `;` or a newline, each `op=role,role`. `op=` closes the op to every role until a tenant opens it. **Unset keeps the built-in default**: every role calls authored reads, editors and up authored writes, managers and owners the `std.*` ops. A tenant's owner may change it for their tenant (`howto-roles.md`). Naming a platform op, a pattern, an unknown role, or an op twice refuses the boot. A name the program does not have does nothing. Read once at boot; the boot census prints a `profiles` line. |
| `ORDO_DELEGABLE_OPS` | default unset | comma-separated **delete ops a delegated token may call**, each an exact op name: `purgeDone,std.tasks.delete`. A delegated token is one that acts for a person, such as a worker credential (`howto-workers.md`). **Unset closes every delete to delegated tokens**; reads and ordinary writes stay open, bounded by the token's allowlist and by what the delegator may do. Platform writes (`std.space.*`, `std.person.*` and the rest) stay closed whatever you set, because their record would name only the delegator; naming one refuses the boot, and so does a pattern. A name the program does not have does nothing. A refusal answers `access_denied` and is recorded as `OpNotDelegable`. The startup log names what is open. |
| `ORDO_DELEGATION_AGENTS` | default unset | comma-separated **agents that may ask a person for delegation**, each `id=name`: `agt_assistant=Expense assistant`. The name is what the person reads on the review (`std.delegation.review`), so it is yours and never the agent's. **Unset accepts no request**, and a request answers `403 delegation_refused` naming this setting. A malformed entry refuses the boot. See `howto-delegation.md`. |
| `ORDO_DELEGATION_MAX_HOURS` | default `8` | the longest delegation a person may grant through the ceremony, 1 to 720 hours. A longer request is refused naming the setting and its value. |
| `ORDO_DELEGATION_REQUEST_SECS` | default `600` | how long a delegation request waits for the person, and after approval for the agent to redeem it, 60 to 3600 seconds. |
| `ORDO_DELEGATION_MAX_OPS` | default `10` | the most ops one delegation request may name, 1 to 100. |
| `ORDO_SESSION_REFRESH_SECS` | default `15` | how often each server process re-reads the session deny list (logouts, and every session ended by a reset or a password change). **This is the longest a second replica keeps serving a session another replica ended**, so it is a security setting. A single replica sees a logout at once. Zero or unparseable falls back to the default. |
| `ORDO_ACCESS_REFRESH_SECS` | default `30` | how often each server process re-reads who the members are. **This is the longest a suspended or removed member can keep being served**, so it is a security setting and not a tuning knob: lower it if an incident must be containable faster, and know that the cost is one read of the members table per period per process. Nothing is restarted and no connection is dropped when it fires. Zero or unparseable falls back to the default rather than disabling the refresh, because a deployment with no refresh is the defect this setting exists to close. |
| `ORDO_AUTH_METHODS` | default unset | comma-separated: `password`, `passkey`. Unset means a deployment that verifies tokens somebody else minted. Any other word refuses the boot. |
| `ORDO_APEX_DOMAIN` | **refuse** with `passkey` | the WebAuthn relying-party id, a bare lowercase domain such as `example.com`. **Changing it later strands every enrolled passkey**, because the authenticator scopes a credential to it and there is no migration. Without `passkey` in `ORDO_AUTH_METHODS` it is read, recorded, and used by nothing. |
| `ORDO_AUTH_ORIGINS` | **refuse** with `passkey` | comma-separated origins that may run a passkey ceremony, such as `https://app.example.com`. Each must be the apex domain or a subdomain of it. Set this **and** `ORDO_CORS_ORIGINS`: they usually hold the same values, and they are two settings because they answer two questions, which page may run a ceremony and which page may call the door at all. An address on another domain refuses the boot and points at the next row. With `ORDO_WILDCARD_ORIGINS=on`, `https://*.<apex>` is accepted. |
| `ORDO_AUTH_RELATED_ORIGINS` | default unset | comma-separated `https://` origins on **other domains** that may use this deployment's passkeys (WebAuthn Related Origin Requests). This makes the server accept them. **A browser accepts them only if `https://<ORDO_APEX_DOMAIN>/.well-known/webauthn` lists them**, so the deployment owner must publish `{"origins": ["https://app.other-brand.test"]}` there, with content type `application/json`. The door answers that path itself (`GET /.well-known/webauthn`), but the browser asks the apex host, which is usually a marketing site and not the door: publish the same text there, or route that one path to the door. Browsers limit how many distinct domains one document may name (five in the current draft), so a longer list passes here and stops working in the browser. |
| `ORDO_AUTH_RP_NAME` | default the apex domain | the name an authenticator shows when it asks the person to create or use a passkey |
| `ORDO_AUTH_USER_VERIFICATION` | default `required` | `required` or `preferred`. `preferred` accepts an authenticator that cannot verify the person (a PIN or a biometric), and the weaker proof is visible in the token's presence rather than silent. Any other word refuses. |
| `ORDO_CORS_ORIGINS` | default unset | comma-separated exact origins that a browser page may call the door from, such as `https://app.example.com,http://localhost:5173`. Unset means same-origin only. **If your page is served from another origin, this must be set or the browser silently discards every answer.** See §5b. `*` and any entry that is not a bare scheme, host and port refuse the boot. With `ORDO_WILDCARD_ORIGINS=on`, an entry may be `https://*.example.com` (`plan-subdomain-signin.md`). |
| `ORDO_USAGE_LEDGER` | default off | `1` turns on request/rows/writes metering (`$usage_replica`, `$usage`) and the `usage.flush`/`usage.rollup` workers, and, with it, the quota gate (`PgQuotas` in place of `NoQuota`; see §8c). Off costs nothing and reports nothing. |
| `ORDO_REPLICA_ID` | default 8 random hex chars | this process's identity in `$usage_replica`. A replica that restarts is a new replica; its rows still sum. |
| `ORDO_USAGE_FLUSH_SECS` | default `60` | how often this replica writes its counters. Refuses `0` and a value that does not parse. See §8c. |
| `ORDO_USAGE_ROLLUP_SECS` | default `3600` | how often the leader rolls the command log and byte tables into `$usage`. Refuses `0` and a value that does not parse. See §8c. |
| `ORDO_OBSERVED_COST_MAX_OPS` | default `4096` | how many ops the observed cost table holds (§2d). A new op past it is not recorded, and `ordo_op_observed_dropped_total` counts the loss. A value that does not parse keeps the default and says so on stderr. |
| `ORDO_OBSERVED_COST_MAX_SPACE_ENTRIES` | default `10000` | how many (tenant, op) entries the observed cost table holds (§2d). `0` keeps no per-tenant numbers. Past it, new entries are dropped and counted. |
| `ORDO_AUTH_REGISTRATION_TTL_SECS` | default `7776000` (90 days) | how long a sign-up is kept when no tenant admits the person, then removed with its authenticator links, passkeys, password and unredeemed tickets. The owner ruled the 90-day default on 2026-09-29; the app developer or deployment owner may set another. The startup log prints the value in use. Refuses `0` and a value that does not parse. To keep sign-ups for a very long time, set a very large number. |
| `ORDO_AUTH_CLAIM_CODE` | removed | **Removed on 2026-10-30**, with `std.deployment.setClaimCode`. A deployment that still sets it (or `ORDO_AUTH_CLAIM_CODE_FILE`) boots, and the startup log says it is ignored and names the first-comer setting; delete the line. First-comer admission is a tenant's `firstComer=true` (`std.space.setAdmission`), off by default and allowed only under `ORDO_AUTH_PROVISIONING=auto-onboard`. The deployment's own space never admits a first-comer: switching it on there is refused, and deployment people come from the identity service. A sign-in that still sends `claim_code` and is refused is told so, with the setting named in `admission.setting`. |
| `ORDO_SHARE_MAX_SECS` | default unset: no ceiling | Optional. The longest a share made with `std.space.share` may last, in seconds. The platform sets no ceiling of its own (ruled 2026-09-29). A share states its own lifetime with `$expires` and is refused past this ceiling. The window closes at that instant whatever day the guest accepts, and the ticket that carries the share lives at most 30 days. Needs `ORDO_AUTH_METHODS`, because a share is redeemed by a person signing in. A value that is not a positive number refuses the boot. |
| `ORDO_SHARE_LINKS` | default `on` | `on` or `off`. Whether open share links (`std.space.shareLink`, `POST /auth/link`) exist. On by default (ruled 2026-09-29). Off refuses the mint and every open, whatever is stored. A link is a bearer secret: whoever holds it reads, with no account, so what it may list is checked at the mint (read ops only, guard-bearing, no platform op) and it always carries an expiry. Needs `ORDO_AUTH_METHODS`. Refuses any other value. |
| `ORDO_SHARE_LINK_DEFAULT_SECS` | default `604800` (7 days) | The **deployment layer** of a link's lifetime: how long a link lives when its sharer states no `$expires` and its tenant chose nothing. A default and never a cap: the platform sets no maximum (ruled 2026-09-29, "no max (not enforced by us)"), so a sharer may ask for longer. The 7 days when unset is `BlobPolicy::share_ttl_default`. A value that is not a positive number of seconds refuses the boot. |
| `ORDO_SHARE_LINK_TENANT_LIFETIME` | default `off` | `on` or `off`. The **app developer** exposing the tenant layer: with `on`, a tenant's owner may call `std.space.setShareLinkLifetime` to choose how long its links live (`30d`, or `default` to take the choice back). The choice is the default for that tenant's links and the longest one, because the tenant asked for it, and it is kept on the tenant's admission policy row (it moves to the general tenant settings store when checklist 24a builds one). With `off` nobody may set it and a value already stored is not applied. |
| `ORDO_PROVISION_SELF_SERVE` | default `off` | `off` or `on-signup`. Whether a person who signs in and holds no tenant of their own is **given one, as its owner** (`plan-provisioning-on-signup.md`). The sign-in answers at once with `provisioning: "in progress"` and the tenant is made on a job of its own; `GET /auth/provisioning` says when it is ready, and the SDK's `waitForTenant` waits for it (`howto-sign-in.md` §4). A tenant left part-made by a restart is made again at boot. Off runs nothing here. A sign-up costs a `CREATE DATABASE`, a schema apply and a custody key (about 110 ms locally). The hook runs after the session is minted, on every sign-in, so a failed provision retries on the next one and a person who registered before you turned it on is given a tenant when they next sign in. The answer of `/auth/complete` carries `space` (the tenant, or null) and `provisioning` (null, `in progress`, `failed`, `presence required` or `limit reached`); the sign-in succeeds whatever comes of the tenant. A sign-in that proves no presence (a password) gets no tenant while `std.space.create` still requires presence (`ORDO_STEP_UP_EXEMPT_OPS`). **Stored, and the stored value wins**: the environment seeds it on a boot that finds nothing stored, `std.deployment.setSelfServe` (owner of the deployment's own space) changes it, and an environment that disagrees is named in the log and loses. Tenants of people who never come back are kept; deleting them is the app developer's decision (`std.space.delete`). Refuses any other value. **Read it back** on `/health/deep` under `self_serve`: the mode and the sign-up limit. |
| `ORDO_PROVISION_SIGNUPS_PER_HOUR` | default `60` | A number, or `unlimited`. The sign-ups an hour the whole deployment accepts **while self-serve is `on-signup`**. A registration without a ticket (`POST /auth/register`) is one, counted before the ceremony begins, so the 61st in an hour is refused with `429 quota_exceeded` and mints nothing. The hook giving a person a tenant is counted the same way, separately, and a sign-in refused there answers `provisioning: "limit reached"`. A sliding hour, kept in `$signup` so every replica counts the same rows; two replicas checking at the same instant can each let one more through. A default, not a ceiling: change it with `std.deployment.setSelfServe` (`signupsPerHour`). Seeds the stored setting like the row above. |
| `ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR`, `ORDO_SHARE_LINK_MISS_LIMIT` | off | Proxy mode's (`ORDO_PROXY_HEADER`, `ORDO_PROXY_TRUSTED`). Past either, an address is slowed, never refused. Without proxy mode they have no effect, and a boot that finds either set prints one line saying what turns it on. |
| `ORDO_AUTH_TEMPORARY_PASSWORDS` | default `off` | `allow` lets a tenant's owner type a password for a person it created (`std.space.setTemporaryPassword`). **Off by default**, because the person should choose their own: `std.space.createPerson` and `std.space.resetCredential` hand the owner a one-time ticket instead. When on, every use is recorded with who set it, the person's other sessions end, and the person is marked as needing to choose a new password (the sign-in route must ask `ManagedStore::must_change`; it does not yet). Refuses any other value. |

**A tenant is not provisioned at boot; the system space is.** A binary that
minted keys for whatever `ORDO_SPACE` said would turn a typo into a
brand-new tenant with brand-new keys, serving an empty space that looks
perfectly healthy. So `ORDO_SPACE` naming a tenant with no custody
**quarantines that space** and names the command that fixes it, while the
deployment comes up and serves everything else (17ac; until 2026-09-21 it
refused the boot outright). The one space the binary creates without being
asked is its own: the **system space**, id `system`, whose name is a
constant in the binary rather than anything you type. See §4c.

**A database nobody has run a ceremony against is not a boot failure
either.** The platform's own tables, `$keycustody` included, are created on
the way up beside the artifact registry's. Creating that table is not
minting custody: an empty `$keycustody` grants nobody anything, and a
tenant with no keys in it is still refused.

**A database nobody has deployed to is not a boot failure, and it is not a
readiness failure either** (owner-ruled 2026-09-20, 17t). With no active
version the process starts, `/health/live` answers 200, `/health/ready`
answers 200 too: the database, custody, configuration, workers, deploy
path and continuity checks it runs are about whether this process can accept
and run a deployment, and a fresh one with nothing applied yet can. The
`continuity` check asks only about the control plane's own space: a tenant
whose chain has moved backwards is listed in the body's `quarantined` array
and does not make this node unready (17aa). An anonymous caller to `/health/ready` or `/health/deploy` gets the status
alone (owner-ruled 2026-10-25). The deployment owner's token gets the full
body, with its check names, `quarantined` and `credential_labels`. The body's
`program_active` is `false`, honestly, while the node waits for a deploy
notification. Refusing to boot, or refusing readiness, would each make the
ordinary first `ordo apply` impossible: you would need a program in the
database before you could start, or bring into rotation, the server the
program is deployed to.

**Platform ops.** The binary serves the `std.space.*` ops and
`std.deployment.apply` beside the program's own, through the same door and
the same access check. `std.space.callersOf` and `std.deployment.apply` are
always served: the second is 18l's deploy door, whose state is the control
pool and the deployment's own space name, and a platform that could not be
deployed to is the state 17t's readiness ruling says it must not be in. The exit ops need
`ORDO_EXIT_BUNDLE_DIR`; `std.space.import` on `ORDO_IMPORT_BUNDLE_DIR`;
`std.space.attestTo` and `std.space.chainProof`
need `ORDO_CHECKPOINT_KEY`, the attestation worker's own condition.
`std.space.attachBucket` needs three things together:
`ORDO_BLOB_ROUTING=attached`, an `ORDO_BLOB_STORE`, and a credential
provider (`ORDO_TENANT_CREDENTIALS`). On `single` every space's blobs resolve
to the one `ORDO_BLOB_STORE` and an attachment would be recorded and never
honoured, which is worse than the op being absent: the consent page it prints
says the objects are in the tenant's bucket. An op
that is not served is not registered, so it answers exactly as a name
nobody registered does. A program version that compiles a `std.space.*`
name is refused at boot and at a hot swap.

**The startup log** prints one block on stdout before the listener binds:
the address, the space, the database (user, host, database: never whether
a password was supplied), the custody kind and epoch, the system space and
what this boot did about it, the program and its
gate summary, the blob store, and a `DISABLED` line for every feature that
is off **naming the variable that would enable it**. A feature that is off
and unmentioned is a feature an operator believes is on. No secret, no
derived secret, and no length of a secret appears in it.

**Shutdown** is `SIGTERM` or `SIGINT`: stop accepting, drain in-flight HTTP
bounded by `ORDO_SHUTDOWN_GRACE_SECS`, cancel the workers, flush the
security drain (the count is logged: a bounded, known loss of audit record
belongs in the log rather than in nobody's memory), release leadership,
close the pool. **`terminationGracePeriodSeconds` must exceed
`ORDO_SHUTDOWN_GRACE_SECS`**: a platform grace equal to the app's is a
SIGKILL wearing a rollout's clothes.

**No TLS, in either direction.** The binary serves plain HTTP and expects a
proxy or an ingress to terminate; `ORDO_HOST_ADDR=0.0.0.0:8080` on a public
interface with no proxy is serving bearer tokens in the clear. The pool
dials Postgres with `NoTls`, so a managed Postgres that requires TLS is one
this deployment cannot reach yet. Both are named limits, not oversights.

**Serve the door over HTTP/2 at the proxy** (owner, 2026-10-21). Over
HTTP/1.1 a browser opens at most 6 connections to one host, shared by every
tab. Each open sync feed and live stream holds one, so a person with several
tabs open can use them all, and then every call waits, writes included. Over
HTTP/2 the calls share one connection and that limit does not apply. The
binary itself speaks HTTP/1.1, so this is a setting on the proxy or ingress
that terminates TLS: turn HTTP/2 on toward the browser. The SDK also keeps
one set of sync feeds per person across tabs (a leader tab runs them), which
helps on HTTP/1.1 but does not remove the limit.

### Images, and pulling by digest

`.github/workflows/images.yml` builds and pushes three images to GHCR: on
every push to `main` and on every `v*` tag it builds, smoke-tests and pushes
`linux/amd64`, and no other architecture (ruled 2026-09-29). No secret is
involved: GHCR authenticates with the automatic `GITHUB_TOKEN`.

```
ghcr.io/hominemai/ordo/ordo             the one image: the deployment, the console
                                        and the identity service, under ordo-supervisor
ghcr.io/hominemai/ordo/ordo-cli         the operator tool
ghcr.io/hominemai/ordo/ordo-playground  the browser surface
```

The deployment-only `ordo-host` image is no longer built (owner ruling,
2026-10-27). Its binary is in the one image and in the release's binary
artifacts, for anyone who builds their own image.

**The image's identity is its content digest** (`sha256:…`), the same way a
deployed program's identity is its version hash (§7): two builds of the same
commit produce the same digest, and the digest is what the run's summary
prints as *"Identity (deploy this)"*. Everything else, including
`sha-<short>`, the branch name, a semver tag and `latest`, is a **pointer**
to a digest, not the identity, and a pointer can move under you between the
time you read it and the time you pull it.

**Deploy by digest.** In `docker/compose.one.yml`, set the image to the
digest from the run summary:

```
ORDO_IMAGE=ghcr.io/hominemai/ordo/ordo@sha256:<digest> \
  docker compose -f docker/compose.one.yml up -d
```

`first-deployment.md` walks the whole first run.

Find the digest for a given commit or tag in that push's Actions run
summary; find the run itself from a `sha-<short>` tag if you only have a
commit. A rollback names the same digest a prior run already published,
never a tag, for the reason §7's `ordo rollback --version <hash>` names one
rather than a pointer.

`ordo` and `ordo-playground` each need a reachable Postgres and the
environment §1's table lists; `ordo-cli` needs only `ORDO_PG_*` to point at
a deployment's database (see §9). None of the three images bakes in a
default host, so a misconfigured container fails at boot rather than
reaching the wrong database.

## 1b · `ordo dev`: the local loop

`ordo dev <dir>` (`plan-dev-loop.md`) is the same assembly a server gets
(`ordo_host::runtime::assemble_embedded`, the same gate, the same
`Deployment::swap`) over a program compiled straight from a directory of
`.ordo` files, watched and recompiled on every save. It is not a
production server: it composes its own `RuntimeConfig` rather than reading
one from the environment, and its custody is always
`CustodyChoice::Test`, the provider a real deployment gates behind
`ORDO_DEV_UNSAFE_TEST_KEYS`, because a dev loop's database is regenerated
seed data by design (see below), never production's.

```
ordo dev path/to/project           # brings up its own Postgres, watches, serves
ordo dev path/to/project --fresh   # destroys the owned cluster's data first
```

**The database.** `ORDO_PG_HOST` (or any of its siblings) set: `ordo dev`
connects to what you named. Unset: it owns the whole lifecycle:
`initdb`/`pg_ctl` on a port it allocates, a data directory under
`<project>/.ordo/devdb` (so a restart is warm), and it stops the cluster
on exit (Ctrl-C, or any signal `ordo-host`'s own shutdown handles).

**The schema delta.** Every save classifies what changed
(`ordo_schema::diff` against the live database) and, on any non-empty
delta, additive or destructive, resets the database and reapplies the
DDL, reseeding from `seed.sql` beside the sources when one exists. **Corrected 2026-09-20:** the class-A path exists now (13b), and
`dev_db::classify` triages through `schema_apply::triage`, so an additive
change is applied and the rows survive. The reset is what happens to a
delta the production path refuses, which is the narrow reading, not
"auto-confirm destructive migrations".
`seed.sql` is plain SQL: the fixture harness's `@BIDX(...)`/`@SPACE@`
placeholder rendering is not implemented for it, so a project seeding a
`sealed indexed` field needs its own answer for now.

**A page on another origin, and passkeys.** When they are set, `ordo dev`
reads `ORDO_CORS_ORIGINS` and the sign-in settings a server reads:
`ORDO_AUTH_METHODS`, `ORDO_APEX_DOMAIN`, `ORDO_AUTH_ORIGINS`,
`ORDO_AUTH_PROVISIONING`, `ORDO_SESSION_SECS` and `ORDO_SESSION_MAX_SECS`
(§5, §5b). A malformed value refuses the start, as it does on a server. Unset,
the loop has no login and no CORS, as before, and prints a development token.
The loop is one database and one space, so a person who signs in lands in
`dev`. It composes no tenant credentials provider and no self-serve
provisioning, so an app that gives each person a tenant of their own (the
notes app) still runs on `ordo-host` (`examples/notes/dev/serve.sh`).

**The port.** `ORDO_DEV_ADDR` (default `127.0.0.1:4000`) is where the
door binds. Loopback by default, same reasoning as `ORDO_HOST_ADDR`.

**The journal.** Every save prints its own phase breakdown: `watch`,
`read`, `compile`, `descriptor`/`generator` (with `--out`), `delta`,
`reset` (when one ran), `gate`, `swap`, with each phase's own
denominator (`gate 330 (76 reads)`), and the phases sum to the printed
total. `crates/ordo-host/tests/dev_loop.rs` is the test that keeps this
honest.

**The descriptor.** `--out <dir>` writes `interface.json` there whenever
its content changes, and does nothing on a save that only touched an op's
body. `--gen <command>` runs after a write, with the descriptor's path in
`ORDO_INTERFACE_PATH`: the seam a generator (`@hominemai/ordo-cdk`, for instance)
plugs into; `ordo-cli` itself never learns the generator's language. A
generator that exits non-zero prints its output and does not stop the
loop.

D1–D4 are all built.

## 2 · The connection pool is a decision, not a detail

`tokio-postgres` ships no pool on purpose, and the one in `ordo-exec` is
explicit for the same reason:

| knob | default | why |
|------|---------|-----|
| `size` | `available_parallelism`, clamped to [2, 8] | A backend is a *process*. More connections than cores buys queueing inside the server, where you cannot see it, instead of in front of it, where you can. |
| `acquire_timeout` | 2s | Waiters queue FIFO on a semaphore and then fail typed (`Saturated`). Backpressure a caller can retry beats an unbounded internal queue. |
| `space_reserve` | `max(1, size / 4)` | Connections one space cannot take, so a tenant queueing slow work leaves the others something. §2a. |
| `statement_timeout` | 30s | One statement's own bound, set as a session GUC at dial. §2b. |

**The pool is your concurrency bound.** Nothing executes without a
checkout, so the API layer inherits the limit for free: there is no
second admission-control knob to keep in sync, and no way for request
concurrency to exceed database concurrency.

Threads: there is exactly one pool of them, tokio's runtime. Each
connection gets a lightweight task driving its socket; nothing is
thread-per-connection. The only OS threads borrowed elsewhere are
`spawn_blocking` workers used by the write protocol, which is sequential
by nature (lock → judge → image → mutate).

Connections dial **at boot**, so a wrong host or a missing database fails
at start rather than on the first customer request. A connection found
closed at checkout is replaced there, where the caller is already paying
latency.

**Transactions own their connection.** Committing returns it to the pool;
*dropping an uncommitted transaction discards the connection*, which
closes the socket and makes the server abort. That is the one rollback
path that cannot be forgotten, misordered, or half-run.

### 2a · One tenant cannot take the whole pool

The size above is the bound; this is how it is **divided**, and division
is the whole of tenancy. Before 20a one semaphore served every space, so
one customer's slow scan held every connection and every other customer's
request failed `Saturated` having done nothing wrong.

Each space now has a share of `size - space_reserve` concurrent
checkouts. A checkout takes its space's share first and the pool's permit
second, inside one `acquire_timeout`: taking the pool's permit first
would mean holding a connection while waiting for the right to use it,
which is the same starvation wearing a different shape, and two budgets
would let a request wait twice as long as you declared.

Three things to know before you tune it:

- **The share binds one space, not every space.** Two spaces at their
  caps can still reach the pool's bound and a third then takes
  `Saturated`. Raise the reserve as the number of busy tenants per
  database goes up; a fair queue across k tenants is not built.
- **A checkout is charged to a space only when it was resolved through
  one.** Request-path work is (`Spaces::pool_of` charges it). Boot-time
  migrations, `ordo-cli` commands and the loops that walk every target
  run on an uncharged handle and are bounded by the pool alone, which is
  what they were before.
- **The floor is `min(size, 2)`.** A pool of two or three cannot both
  serve a tenant and reserve anything, so on one the share stops being a
  bound rather than halving a single-tenant deployment.

**What a caller sees when it is full.** `Saturated` reaches the caller as
`503 server_busy`, with `Retry-After: 1`. Nothing ran, and the same
request can be sent again. Before 25z it answered `500 internal`.

### 2b · One statement cannot run forever

Every pooled connection is dialled with `statement_timeout` in its
session options, so it carries the bound from the moment it exists,
including the re-dial that replaces a dead connection mid-request.
Postgres cancels at SQLSTATE 57014 and the caller gets
`statement_timeout` (503), which is a class they can act on rather than a
500 they cannot distinguish from a crash.

It reaches **everything** that runs on a pooled connection, including a
writer blocked in the chain-append lock and a maintenance pass over a
large table. That is the point of it and also its cost: raise the value
for a migration window rather than discovering the bound halfway through
one, and remember that 0 means no bound. `PoolConfig::connect_config`,
which is what a `LISTEN` session and the supervisor's leadership lock
dial with, deliberately carries no bound: those are session-scoped and
the timeout would cancel them.

### 2c · Which tenant is holding it

`ordo_pool_space_capped_total{space}` counts checkouts that queued behind
their own space's share, and a space reaches its cap only by holding
every connection the cap allows, so this series names whoever is
consuming the pool. `ordo_pool_saturated_total{space}` names whoever was
refused. Both carry `space` **whether or not** `ORDO_METRICS_PER_SPACE`
is set, because their population is the spaces a checkout was refused
for rather than the spaces you have: if nothing is saturating, neither
series exists. `ordo_pool_wait_seconds` is the healthy-path histogram and
stays behind that opt-in with the other high-cardinality families.

### 2d · What each op really costs

Every execution records what it cost, per op and per tenant: calls,
errors, total and longest duration, rows returned, and the cost class it
really ran at. A refinement on a sealed field can raise a request's class
above the one the op was registered with (`api-engine-design.md` §7.1),
so both are kept. Nothing acts on these numbers yet. They are collected
now so that plan tiers (`plan-tiers.md`, Ordo v2) have history to start
from.

The numbers live in memory, per replica, and start again at a restart.
Two settings bound them: `ORDO_OBSERVED_COST_MAX_OPS` and
`ORDO_OBSERVED_COST_MAX_SPACE_ENTRIES` (§1).

On `/metrics`:

- `ordo_op_observed_calls_total{op,class}`, where `class` is the class
  the request ran at.
- `ordo_op_observed_errors_total{op}`.
- `ordo_op_observed_seconds_total{op}` and `ordo_op_observed_seconds_max{op}`.
- `ordo_op_observed_rows_total{op}`, rows the successful reads returned.
- `ordo_op_observed_dropped_total`, entries a cap refused.

With `ORDO_METRICS_PER_SPACE` on, the same families also appear as
`ordo_space_op_observed_*` with a `space` label. They are separate names
so that summing one family never counts a call twice.

The deployment owner reads the same table with
`std.deployment.observedCost`, in the deployment's own space. Without
parameters it answers one row per op. With `space`, it answers that
tenant's rows; with `bySpace: true`, every tenant's. Each row carries
`op`, `space`, `calls`, `errors`, `totalMicros`, `maxMicros`,
`rowsReturned`, `registeredClass` and `effectiveClasses` (calls by
class). This is what the console reads.

## 3 · Page size is yours; paging is not optional

Reads page by **keyset** over the plan's total order: never OFFSET, which
skips and repeats rows under concurrent writes. Page size is a
deployment-wide setting rather than a caller's parameter, because page
size is a cost decision and cost decisions belong to the platform.

Cursors are opaque, MAC'd, and bound to space, op, and principal. Three
consequences worth knowing before you tune anything:

- A cursor another principal presents is `invalid_cursor`, not a seek.
- A cursor from another space is refused even though the MAC would verify.
  Space is in the MAC precisely to stop that.
- An honestly stale cursor (the op was redeployed) answers `version_skew`:
  restart from page one, distinct from a forgery.

Rotating the page key invalidates outstanding cursors. That costs paging
clients one restart, which is the correct price for a key rotation.

## 4 · Keys and custody

> **Status, stated because this page is read by people about to deploy.**
> This section is now the code. `LocalKeyProvider` is a real provider over
> a real store: per-space SMKs and wrapped-not-derived DEK epochs live in
> `$keycustody` (`ordo_exec::keystore::PgKeyStore`), the deployment master
> key comes from `ORDO_MASTER_KEY` in development or the sealed file in a
> deployment, and **custody is an input to loading a
> program**: `deploy::verify_candidate` assigns the fixture's provider and
> asserts it by identity, so a deployment cannot read through the
> operator's custody and write under the repository's test constant. That
> was true until 2026-09-07 and is the reason `ORDO_KEY_PROVIDER=test`
> refuses without `ORDO_DEV_UNSAFE_TEST_KEYS`.
>
> What is still owed: any KMS binding. `ORDO_MASTER_KEY` is a
> **plaintext master key in the process environment**, stretched with one
> HMAC. It is readable from `/proc/<pid>/environ`, printed by every
> `docker inspect`, captured by every crash reporter that dumps the
> environment, and guessable offline from a database backup if the secret is
> short. **The server refuses `ORDO_KEY_PROVIDER=local` unless
> `ORDO_DEV_UNSAFE_TEST_KEYS=1` is set** (S6-009, ruled 2026-09-28), and so
> do the `ordo keys` commands when they are given no key file. A deployment
> uses `sealed-master`: run `ordo keys seal-master`, then set
> `ORDO_KEY_PROVIDER=sealed-master`, `ORDO_MASTER_KEY_FILE` and
> `ORDO_MASTER_KEY_PASSPHRASE_FILE`. `ordo dev` needs no setting.

Two levels, and the split matters:

- **SMK**: one space master key, held by the `KeyProvider`, never leaving
  it.
- **DEK**: per-epoch data keys the SMK wraps. Every sealed value is
  sealed under exactly one, and every envelope carries its **epoch tag**
  from the first write, so rotation is a re-seal job rather than a
  wire-format migration.

**Blind-index keys derive from the SMK, not the epoch.** This is a
deliberate, documented trade: DEK rotation stops bounding index
linkability, and in exchange logical uniqueness holds continuously,
including across the revocations that force rotation. Without it, a lazy
re-index makes the same plaintext hash two ways at once, and a
`sealed indexed unique` field admits duplicates exactly when a webhook
retry is most likely to hit it.

### What a `KeyProvider` must implement

- `smk()`: the master key material.
- `dek(epoch)`: 32 bytes per epoch. How they exist is yours: a real
  provider unwraps a stored, SMK-wrapped random DEK.
- `current_epoch()`: what new seals are written under.

**Rotation must be possible.** A key that cannot rotate is compromised
forever the moment it leaks, so a provider that cannot rotate is not a
provider. *How often* is your call: on suspected compromise, always,
immediately, without waiting for a schedule.

A deployment that has never rotated is **surfaced, not punished**: the
honest disclosure is the product.

## 4b · Where blob bytes go

`ORDO_BLOB_STORE`, in one of two forms:

```
ORDO_BLOB_STORE=s3                       # binding from ORDO_BLOB_S3_*
ORDO_BLOB_STORE=fs:/var/lib/ordo/blobs   # test double; needs the dev flag
```

The selector is explicit, for the same reason `ORDO_KEY_PROVIDER` is: a
deployment that meant S3 and mistyped must get a refusal, never a silent
demotion to a directory on the pod's ephemeral disk.

**`fs:` needs `ORDO_DEV_UNSAFE_BLOB_STORE=1`.** A filesystem has no
durability story anybody has written down, so `FsStore` is a test double
and says so: `BlobStore::production_ready()` answers `false`, and that
answer is the authority rather than the scheme string, so a future binding
that answers `false` inherits the rule without anybody remembering to add
it. It is a **second** dev flag rather than a reuse of
`ORDO_DEV_UNSAFE_TEST_KEYS` because the two unsafe different things: a
laptop running a development master key is not a deployment that
agreed to put customer files on a filesystem, and one flag unlocking both
would hand an operator who asked for the first the second in silence.

**When a store is required** (ruled 2026-09-07): *exactly when the loaded
program declares a `blob` field that is not `inline`.* Three cases, and the
rule falls out of what each can actually do:

- **No blob field.** Every blob route addresses `entity.field` and the port
  refuses a field that is not a blob, so no upload is reachable. A required
  store would be a dependency nothing can touch, and would make every
  deployment of every program written before blobs need a bucket.
- **`blob inline` only.** Those bytes ride the row's sealed payload and
  never reach the object store; that is what makes the tier GC-free by
  inheritance. Requiring a bucket for bytes that never leave Postgres would
  be requiring the wrong thing.
- **A stored `blob` field.** The declaration is a promise the schema has
  already made: the column exists, `std.E.create`/`.update` accept the
  field, the interface descriptor publishes it, and a generated client
  renders a file input. With no store the first upload is a 500 in
  production on the path the program was written for. This is not "refuse
  the feature and say so": the platform is not coherent without it.

**This is the one refusal that follows the dial**, and the exception is
forced: whether a store is *needed* is a property of the deployed program,
and the program lives in the database. The variable's syntax and the S3
binding's own required variables are validated before any dial, so a typo
still refuses in milliseconds; only necessity waits for the program. A
**swap** applies the same rule and refuses rather than installs: the node
keeps serving the version it has, which is the deploy channel's stated
policy.

## 4c · The system space, and the order a first deployment runs in

**The system space is the platform's own home.** Its id is `system`, chosen
by the platform rather than by you: it is a constant in the binary, read
from no variable and no command line. The boot creates it when it is
absent, so a deployment always has somewhere to keep what it knows about
itself. Owner-ruled 2026-09-20 (checklist 17v).

It is not a tenant, and three properties say how:

- **Nobody names it.** A tenant's id reaches the binary from outside, in
  `ORDO_SPACE`, in a request header, or after `--space` on an operator
  command. This one never does, which is why creating it cannot turn a typo
  into a tenant.
- **Nobody erases it.** `std.space.delete` refuses it before it checks the
  confirmation, the same refusal `$`-prefixed platform spaces get.
- **It holds what the platform authored.** Its configuration, and as those
  surfaces arrive, its catalogue and reference data. A third party's content
  is never here: a third party is a tenant, so what they publish lives in
  their own space and is shared outward.

Everything else about it is ordinary. It has a chain, guards, versioning and
deploys exactly as a tenant does.

**A first deployment, in order.** Point `ORDO_PG_*` at an empty database,
set the sealed-master pair (`ORDO_KEY_PROVIDER=sealed-master`, `ORDO_MASTER_KEY_FILE`, `ORDO_MASTER_KEY_PASSPHRASE_FILE`), the token key, the page
key, and `ORDO_SPACE=system`. Start the binary. It creates the platform's
tables, mints custody for the system space, and serves with no program:
`/health/live` answers 200 and the startup log prints
`system      system  (created at boot)`. No `ordo keys init` and no
`ordo apply` run before this.

Then, in either order:

```
ordo apply <dir> --as <you>                     # the first program
ordo keys init --space <tenant> --as <you>      # the first tenant's custody
```

The system space's founding member row needs `members`, which a program's
DDL creates, so on a database with no program the boot mints custody and the
log says `custody only`. The first `ordo apply` writes the row, on the swap
it triggers, with no restart: the log prints `system space system: founding
row written`.

**Pointing `ORDO_SPACE` at a tenant that has no custody closes that one
space.** The deployment binds, answers, and holds the named space shut with
the remedy as its reason:

```
  QUARANTINE  smg: not served — no key custody on this deployment: nothing
              has created the tenant `smg`, or a provision was interrupted.
              run:  ordo keys init --space smg --as <you>
              the deployment is up and every other space is served; this
              one is refused until it exists.
```

`/health/ready` answers `200` and names it under `quarantined`, because a
tenant nobody has created yet is not an outage of the deployment. Requests
naming that space are refused until it exists.

**This changed on 2026-09-21 (17ac)** and the old behaviour was a boot
refusal, so a deployment pointed at an unprovisioned tenant used to exit
before binding. That was the last place one space's state decided whether a
whole process started. What it protected is unchanged: the binary still
reads custody and never creates it, so a typo in `ORDO_SPACE` mints
nothing.

The shape a first cluster deployment meets is therefore simpler than it
was: bring the deployment up, create the tenant through the door with
`std.space.create`, and point `ORDO_SPACE` at it when you want that tenant
to be this deployment's home.

**The master key of the first boot is the one everything is wrapped
under.** Custody minted at boot is wrapped under whatever master key
was set at the time, so a mistyped master on a first boot is a system
space wrapped under the typo. The next boot under the right master refuses
with *"custody exists but this master key does not open it"*, and
`ordo keys rewrap-master` is the way across.

### The deployment owner, today

The deployment owner is the person who runs this deployment
(`glossary.md`, **deployment owner**). Today:

- **Who they are:** with the identity service, its people
  (`ordo-identity-service invite`); `ORDO_DEPLOYMENT_OWNERS` is then
  ignored. Without it, principals named in `ORDO_DEPLOYMENT_OWNERS`, made
  owners of the deployment's own space (`ORDO_SPACE`) at boot. The variable
  adds owners and never removes them.
- **How they sign in:** with the identity service, at its page, with a
  passkey (`howto-identity.md`). Without it, the same way the app's users
  do, on the app's own sign-in page: a deployment owner registers like
  anyone, you read their principal id, set the variable and restart.
- **What they can do:** platform ops on the deployment's space: recovery
  cases, succession, membership of that space, and `ordo apply --host`.

The owner ruled on 2026-10-01 that deployment owners sign in separately,
through the back end (board `deployment-owner-identity`, checklist 18o).
The identity service is that ruling, built. The model without it stays for
a deployment that names no identity database, until a later, announced
release removes it.

## 4d · One database per tenant, and the `$deployment` credential

Every tenant has a database of its own, on the default tenant cluster unless
the `std.space.create` call names another. No setting turns this on, and there
is no single-database mode. Configuring nothing works: each tenant gets a
database on the deployment's own Postgres server.

The credentials directory is optional. It is needed only when a cluster is
dialled under a label other than `$deployment`:

```
ORDO_TENANT_CREDENTIALS=dir
ORDO_TENANT_CREDENTIALS_DIR=/run/ordo/credentials     # mode 0700
```

A per-tenant cluster's `$dbCredential` is refused at the `std.space.create`
call when no provider is set. An `ORDO_PG_TENANT_CREDENTIAL` with a label of
its own is refused at boot. `ORDO_BLOB_ROUTING=attached` needs the provider
too.

**The directory needs no `$deployment` file.** A tenant made by
`std.space.create` or by sign-up is a database the door dials under the label
`$deployment`, meaning "this deployment's own database". The server answers
that label with the password it already holds (`ORDO_PG_PASSWORD` or
`ORDO_PG_PASSWORD_FILE`), so the password is stated once. This holds for
provisioning, the door's reader, the resolver and `ordo restore --tenant`. A
`$deployment` file in the directory is used only when the deployment holds no
password of its own. The answer is for the deployment's own cluster: point
`ORDO_PG_TENANT_HOST` at another server and name a credential of its own in
`ORDO_PG_TENANT_CREDENTIAL`, or that server is sent this deployment's
password. A directory with wider modes than `0600`/`0700` still refuses the
boot. If a tenant's database cannot be read, the door prints one `door:` line
naming it and, for an authentication failure, the variable to set.
Live updates reach a tenant's own database because the server listens on that
database. A listener is one server connection, and Postgres cannot listen to two databases on one, so the server
opens it when a live feed opens on a tenant and closes it
`ORDO_PG_IDLE_SECS` (default 30) after the last feed on that database ends.
A tenant with no feed open holds no listening connection. The deployment's
own database is listened to for the server's life, which is one connection.
A listener that failed or ended is opened again by the next feed and on each
`ORDO_ACCESS_REFRESH_SECS` tick (default 30).

**What a tenant costs the server** (measured, 25g, `ORDO_PG_POOL_SIZE=6`):
before, 7 connections per tenant for good (6 pool, 1 listener), so a default
`max_connections` of 100 ran out at 12 tenants. Now 0 for an idle tenant,
and while it is in use up to the pool size plus 1 for an open feed. A feed's
listener goes `ORDO_PG_IDLE_SECS` after the feed closes.

**Walking every tenant holds a few connections, not one each** (measured
2026-10-21 with 10 to 150 tenants, fixed the same day). Three events touch
every tenant database: the server's boot, the first background pass after new
tenants are made, and a deploy. Before the fix each opened one connection per
tenant at once and held it for `ORDO_PG_IDLE_SECS` (121 tenants gave 122
connections in 1 second, and at `max_connections` 100 the boot pass failed
with "too many clients"). Now a walk dials one tenant, uses it and closes the
connection, and at most `ORDO_PG_TENANT_DIAL_CONCURRENCY` (default 8, range 1
to 64) tenants are dialled at once across all walks. With 60 tenants the peak
went from 63 connections to 2. A tenant being served still keeps its own pool
as described above. Size `max_connections` for the pool size of each tenant in
use at once, plus about this setting for the walks.

A walk that has to queue for a dial slot waits at most
`ORDO_PG_TENANT_DIAL_WAIT_MS` (default 60000, range 1 to 3600000). When the
wait runs out, the step for that tenant fails with `dial_wait_exceeded`,
which names this setting and says it can be tried again. The boot pass, the
catch-up sweep and a deploy treat it like an unreachable tenant: they report
it held and carry on with the rest. A bad value refuses the boot, naming the
setting and the default. Raise it if a slow cluster holds the slots longer
than a minute; lower the cap's pressure first by checking for a stuck dial.

## 5 · Credentials

Bearer tokens are minted and verified by `ordo-identity`: domain-separated
HMAC, length-prefixed payload, MAC checked *before* the payload is parsed,
constant-time comparison, expiry denying at the instant itself.

Every verification failure is **one** opaque error. A verifier that
distinguishes "bad signature" from "expired" from "malformed" hands an
attacker a decision procedure: forge, watch which complaint changes, learn
where the boundary is.

The same discipline runs one layer up. Bad token, no membership,
suspended, op not in role, capability outside its allowlist, stale
step-up: all one `Deny` on the wire. The *reasons* are rich and they go
to the operator's audit trail, where they belong.

Two properties to rely on:

- **The clock is the checker's**, never the token's. An expiry cannot be
  outrun by a caller lying about the time.
- **A change to who the members are takes effect within
  `ORDO_ACCESS_REFRESH_SECS` (default 30), and nothing is restarted.** Each
  running server process keeps the member list in memory and re-reads it
  from the database on that period, then swaps the new list in. A request
  already in flight finishes against the list it started with; the next one
  sees the new one. No connection is dropped and no program is reloaded.
  This covers a member who is **suspended** (`status = 'suspended'`), one
  who is reinstated, and one whose row is removed. Verified against a running
  server on 2026-09-28: the same process answered 200, then 403 four seconds
  after the row was set to suspended, then 200 after it was set back, then
  403 after the row was deleted.

  **Suspending is a command.** `std.space.suspend` suspends a member and
  `std.space.reinstate` lifts it (`member_ops.rs`); both are owner-only by
  default and refuse to suspend the last active owner. **What it does not
  do:** the period is a ceiling, so a change is not instant, up to
  `ORDO_ACCESS_REFRESH_SECS` on each process. Until 2026-09-28 this
  section promised suspension was *"immediate by construction"* while the
  member read never selected the status column.

### Where a token comes from, and how a session ends

The routes below are all `POST`, all on the deployment's own HTTP surface.
They exist only when `ORDO_AUTH_METHODS` is set; a deployment without it
answers `auth_unconfigured` rather than 404, so a wiring mistake does not
read as a missing feature. The first four are the session; the passkey
routes, the two password routes and the arrival fields follow them.

| route | body | answer |
|---|---|---|
| `/auth/begin` | `{"method": "password", "identifier": "…"}` | `{"challenge": "…", "method": "password"}` |
| `/auth/complete` | the challenge, the identifier, the password, `display` | `{"token", "expires_at", "session_expires_at"}` |
| `/auth/refresh` | nothing; `Authorization: Bearer <token>` | a new `{"token", "expires_at", "session_expires_at"}` |
| `/auth/logout` | nothing; `Authorization: Bearer <token>` | `204`, always |

**Passkeys** (`passkey` in `ORDO_AUTH_METHODS`). Byte strings are base64url
without padding, and `client_data_json` is posted as the exact bytes the
browser produced. `@hominemai/ordo-sdk` does all of it (`signInWithPasskey`,
`registerPasskey`, `stepUp`), and the recovery routes further down
(`listAuthenticators`, `linkPasskey`, `removePasskey`, `endEverySession`,
`issueRecoveryCodes`, `redeemRecoveryCode`, `recoverWithTicket`,
`contestRecovery`, `acknowledgeRecovery`). A sign-in answer's `notices`,
`presenceAvailableAt` and `recoveries` are on the `PasskeySession` it returns.

| route | body | answer |
|---|---|---|
| `/auth/begin` | `{"method": "passkey"}` | `{"challenge", "method", "rp_id", "user_verification", "allow_credentials": [], "timeout_ms"}` |
| `/auth/register` | `{"method": "passkey", "display"?, "ticket"?}` | the creation options: `challenge`, `rp_id`, `rp_name`, `user {id, name, display_name}`, `algorithms`, `resident_key`, `attestation`, `exclude_credentials`, `timeout_ms` |
| `/auth/complete` | `kind: "passkey_assertion"` (`challenge`, `credential_id`, `client_data_json`, `authenticator_data`, `signature`) or `kind: "passkey_registration"` (`challenge`, `client_data_json`, `attestation_object`, `display`, `ticket`?) | a token **with presence**, so a step-up op is allowed for five minutes |
| `/auth/step-up/begin` | nothing; `Authorization: Bearer <token>` | request options with `allow_credentials` narrowed to the person's own passkeys |
| `/auth/step-up` | a `passkey_assertion`; `Authorization: Bearer <token>` | the same session, re-minted with fresh presence |

Registering is signing up under `auto-onboard`, and is refused under
`invite-only` (and stores no key) unless it presents a `ticket` a
`std.space.*` op issued for a passkey. A ceremony begun with a ticket cannot
finish without it. A person who already holds a factor cannot add one with a
ticket. `GET /.well-known/webauthn` answers the related origins when
`ORDO_AUTH_RELATED_ORIGINS` is set.

**Naming a tenant at sign-in.** `/auth/complete` (any method) takes an
optional `space`, and with it an optional `invitation` (an invitation ticket
for that tenant). A `claim_code` is no longer read (removed 2026-10-30); a
refused arrival that sends one also gets `message` and `setting`, naming
the tenant's `firstComer` setting. The answer then
carries `"admission": {"space", "outcome"}`, where `outcome` is `admitted`,
`already-member` or `refused`. A refusal is not a failed sign-in: the person
is signed in to the deployment either way, and `refused` is the answer whether
the tenant does not exist, does not admit strangers, or was handed a ticket
that is not for this person.

| route | body | answer |
|---|---|---|
| `/auth/password/redeem` | `{"ticket", "identifier", "password"}` | `204`. A person an administrator created gets a first password with the ticket the administrator handed over. `409 identifier_taken` (the ticket is not spent) is told to the holder of a valid ticket only. Everything else is the one refusal. |
| `/auth/password/change` | `{"identifier", "password", "new_password"}` | `204`. The current password proves who is asking. Every session the person had ends, so they sign in again. This is how a temporary password is replaced. |
| `GET /auth/methods` (`?tenant=<id>` adds that tenant's providers) | none | `{passkey, password, email, guest, humanCheck, connections}`: the ways in this deployment serves, for a page to build its sign-in screen. Public, cached for 60 seconds, no secret or client id. `howto-sign-in.md` §3 |

**A person's own passkeys, and recovery.** Every route below needs `passkey`
in `ORDO_AUTH_METHODS`. `docs/howto-operating.md` ("When a Person Loses Their
Passkey") says what each one proves and what it does.

| route | body | answer |
|---|---|---|
| `/auth/logout-all` | a session's bearer | `204`. Every session the person has ends, this one too. |
| `GET /auth/authenticators` | a session's bearer | the person's authenticators, with a `single_authenticator` notice when they hold one |
| `/auth/authenticators/link/begin`, `/auth/authenticators/link` | a session's bearer; then `{"registration", "vouch"}` | adds a passkey, vouched by a fresh assertion from one they hold |
| `/auth/authenticators/unlink` | `{"authenticator", "vouch", "end_sessions"?}` | removes a passkey, its key with it; `409 last_authenticator` for the last |
| `/auth/recovery-codes` | a session's bearer, with fresh presence | ten recovery codes, once; the old set is void; `403 presence_required` without presence |
| `/auth/recover/code` | `{"code"}` | opens a recovery and returns its ticket at once; `429` after 30 wrong codes in a minute |
| `/auth/recover/ack` | `{"case"}`, a session's bearer with fresh presence | `204`. The person has seen a recovery of theirs. Until then every `/auth/complete` answer carries it in `recoveries` |
| `/auth/recover/begin` | `{"ticket"}` | creation options for the new passkey, bound to the ticket |
| `/auth/recover/complete` | the ticket and a `passkey_registration` | `{"recovered", "case", "authenticator", "retired", "presence_available_at", "contest_until"}`; no token, every session ended |
| `/auth/recover/contest` | a `passkey_assertion` | a retired passkey contests a completed recovery (`{"contested"}`); an active one cancels one that has not completed (`{"cancelled"}`) |

The deployment owner's recovery and succession ops are platform ops, served
wherever platform ops are and answering only in the deployment's own space:
`std.identity.recover`, `std.identity.recoveryTicket`,
`std.identity.cancelRecovery` and `std.space.succeedOwner`. Owner-only, and
step-up gated unless `ORDO_STEP_UP_EXEMPT_OPS` names them. Without passkey
authentication they answer that recovery is not configured. A tenant's owner has
two more, answering in the tenant: `std.space.recoverMember` (a member the
tenant created) and `std.space.setRecoveryPolicy` (hold on recovery, off unless
the tenant turns it on, kept in `$recoverytenant`).

The host command is `ordo identity recover --principal <id> --as <who>
[--evidence <class>] [--ref <r>]`. It opens a recovery with no delay and
prints the ticket once. The tables are `$recovery` (the cases) and the
recovery columns of `$authenticator`, created at boot beside the other
authentication tables. The worker `auth.recovery.sweep` (60 seconds) removes
the passkeys a recovery retired, and their public keys, when the contest
window closes.

**Recovery's settings.** Ordo provides the mechanisms and you decide what is
enforced, so every setting has a default and each is refused rather than
clamped: an unparseable value, one below its floor, or a combination that breaks
a rule refuses the boot and names the variable. The startup log prints what was
settled (`recovery    paths ...`). All are optional.

| variable | default | what it decides |
|---|---|---|
| `ORDO_RECOVERY_QUARANTINE_SECS` | `86400` (a day) | how long a recovered passkey cannot satisfy step-up (owner, 2026-09-29: a day's block on export after recovery, adjustable). Floor `3600`. May not be longer than the contest window |
| `ORDO_RECOVERY_CONTEST_SECS` | `259200` (three days) | how long a passkey a recovery retired may contest it. Floor `3600` |
| `ORDO_RECOVERY_OPERATOR_DELAY_SECS` | `86400` | how long a case the deployment owner opens through the door waits before it may yield a ticket. Floor `3600` |
| `ORDO_RECOVERY_SUCCESSION_DELAY_SECS` | `86400` | the same for `std.space.succeedOwner`. Floor `3600` |
| `ORDO_RECOVERY_HOST_DELAY_SECS` | `0` | the delay on `ordo identity recover`. Zero: whoever holds the database and the master key is already sovereign, so a delay would restrain nobody. The record is the control |
| `ORDO_RECOVERY_CODE_DELAY_SECS` | `0` | the delay after a recovery code is spent. Zero (owner, 2026-09-29: a code works at once). A deployment that cannot allow a day of ordinary reads after a stolen code sets one |
| `ORDO_RECOVERY_TENANT_DELAY_SECS` | `0` | the least delay on a case a tenant owner opens for a member the tenant created. A tenant may raise it for itself and never lower this |
| `ORDO_RECOVERY_OPERATOR_APPROVALS` | `1` | how many deployment owners must approve a case opened through the door, or a succession. The opener is the first. 1 to 10 |
| `ORDO_RECOVERY_TENANT_APPROVALS` | `1` | the least approvals on a tenant's recovery of a member. A tenant may raise it and never lower this. 1 to 10 |
| `ORDO_RECOVERY_PATHS` | `code,tenant,operator,host` | the paths this deployment allows. A path left out refuses (`ordo identity recover` says so when `host` is off) |
| `ORDO_RECOVERY_MANAGERS` | `off` | whether a tenant's managers may recover a member below manager, as its owners may (owner, 2026-09-29: "if the app dev sets it that way"). Never a peer or an owner. A tenant may turn it off for itself with `managersMayRecover` and cannot turn it on when this is off. `on` or `off` |
| `ORDO_RECOVERY_HOLD_DEFAULT` | `off` | whether a tenant that has not chosen holds a recovered person (owner, 2026-09-29: a recovered person is not locked out of each tenant) |

The floors are the plan's numbers and are not the owner's. A tenant can only
tighten: `std.space.setRecoveryPolicy` refuses a delay or an approvals count
below yours, and takes a higher one.

**The second step's settings.** A second step is a code from an authenticator
app or a code-generating token, a security key, or one of the person's backup
codes (the same set recovery uses), asked for after a password or a code sign-in
and, when a person or a tenant turns it on, after a passkey. Ordo provides the
mechanisms and you decide what is enforced: a person turns it on for themselves
by enrolling a factor, a tenant can require it, and you can enable or require it
for the deployment. A level can only tighten the one above it. Each variable is
optional, each is refused rather than clamped, and the startup log prints what
was settled.

| variable | default | what it decides |
|---|---|---|
| `ORDO_AUTH_SECOND_FACTOR` | `enabled` | `off`: no second step anywhere, and no factor can be added, and a tenant cannot require one. `enabled`: a person may add a factor and is then asked for it; a tenant may require one. `required`: every sign-in needs a second step, passkeys included, and a person with no factor is made to enrol one before a token is minted. `required-for-non-passkey`: the same, except a passkey sign-in |
| `ORDO_AUTH_SECOND_STEP_ATTEMPTS` | `5` | wrong answers one second-step ceremony allows before it ends and the person signs in again. At least 1 |
| `ORDO_AUTH_SECOND_FACTOR_LOCK_AFTER` | `20` | consecutive wrong answers, across ceremonies, that lock one factor (owner, 2026-09-29). A locked factor still counts as held, so a password alone never signs in; it refuses every answer until the person removes it and enrols again. Backup codes are counted per person on the same setting and a fresh set clears the lock. At least 1 |
| `ORDO_AUTH_SECOND_STEP_SECS` | `300` | how long a half-signed-in state lasts, and how long a factor's enrolment that has not been confirmed lasts. At least 30 |
| `ORDO_AUTH_SECOND_FACTOR_MAX` | `10` | how many factors one person may hold. At least 1 |
| `ORDO_AUTH_REAUTH_SECS` | `300` | how long a fresh first factor stays good for changing a factor or issuing backup codes. At least 30 |

**A tenant's level** is `secondFactor` on `std.space.setAdmission`
(`inherit`, `required`, `required-for-non-passkey`), set by a tenant owner with
presence like every admission setting. It is refused, by name, when the
deployment is `off`, and `required-for-non-passkey` is refused when the
deployment is `required`. The door asks it of every human session on every op,
so a person who signed in without naming the tenant is refused there and signs in
again naming it. The door holds the levels in memory; the worker
`second-step.levels.refresh` (`ORDO_ACCESS_REFRESH_SECS`, 30 by default) re-reads them so another replica learns
of a change, and the replica that took the call knows at once.

**A person's level** is `also-for-passkeys` or `when-enrolled` (the default),
kept in `$secondfactorpolicy`. The tables are `$secondfactor` (the factors, and
one lock row per person who has tried a backup code) and `$secondfactorpolicy`,
created at boot, and `$passkey` gained a `role` column. A code-generating
factor's secret is sealed under your key custody in the deployment's own space
(`system`), never stored in the clear.

**A person manages their factors** with a session's bearer token. `GET
/auth/factors` lists them, `POST /auth/factors/totp/begin` and `/confirm` and
`POST /auth/factors/key/begin` and `/complete` enrol one, `POST
/auth/factors/{id}/remove` removes one and `POST /auth/factors/policy` sets
their own level. Starting an enrolment, removing a factor, changing the level
and issuing backup codes each need a fresh first factor: a passkey assertion
still inside the step-up window, or a reauth handle. The handle comes from
`POST /auth/complete` with `"reauth": true` and the session's bearer, is
answered by the second step when the person holds a factor, and is sent back in
the `x-reauth` header. It is spent by one act. Removing a factor ends every other
session the person has. A person a level makes enrol has no token yet; the
handle and binder of their half-signed-in state authorise the enrolment routes
until it expires, and the second step is then completed with the factor they
enrolled.

**A reset or a recovery removes every second factor once it completes**, and
not when it is asked for or opened (owner, 2026-09-29): a recovery when it
completes, a credential reset when the person redeems its ticket. A factor an
attacker enrolled while they held the old password does not outlive it.

**A second factor never carries presence.** A password and a key, a password and
a code, or a passkey and a code can each sign in. Only a passkey used as a first
factor satisfies step-up, so a stolen password plus a key an attacker enrols
cannot export a tenant.

`expires_at` is when this token stops verifying. `session_expires_at` is
the ceiling: the instant past which no refresh is granted and a ceremony
is required again. Defaults are one hour and twelve hours,
`ORDO_SESSION_SECS` and `ORDO_SESSION_MAX_SECS`.

**Refresh before `expires_at`, not after.** An expired token cannot be
refreshed, because the expiry is checked before anything else. A client
that waits for a 401 has already lost the session.

**A refresh does not rotate the session id and does not re-check
membership.** It continues the same session with a later expiry and a
narrower or equal scope. Membership is re-read by the door on a period
(`ORDO_ACCESS_REFRESH_SECS`, default 30) rather than on every request, so a
member who has been suspended or removed can refresh a session for at most
that long and is then refused everything: the door decides authority and
these routes decide only whether the session lives.

**`/auth/logout` answers `204` for anything.** A garbage string, another
deployment's token and a real one are the same answer, so whoever holds a
stolen token learns nothing about whether it was worth anything. The one
exception is `503 session_store_unavailable`, which means the logout was
**not** recorded: do not discard the token, and retry.

**A logged-out token stops working immediately**, on this process. Across
replicas it stops working at the next refresh of the deny list, which is
a cadence rather than a broadcast: `ORDO_SESSION_REFRESH_SECS`, default 15,
run by the `sessions.refresh` worker. Single-replica deployments see it at
once.

### What the platform does not build, and you must

The auth surface is a credential path and a session, and nothing else.
Every item below is an application's to build, over the platform's own
ops and your own tables:

- **No signup endpoint.** Nothing turns "a stranger wants an account"
  into an enrolment. `LocalPasswordProvider::enroll` is the call, and
  when, whether and on what terms it happens is your product.
- **No self-service "forgot password" for a password account.** An
  administrator's reset gives the person a one-time ticket they redeem at
  `/auth/password/redeem` for a password of their own. A person who lost a
  passkey uses recovery (`/auth/recover/*`, with recovery codes). Nothing
  mails a password-reset link by itself; build that over the mail channel if
  your product needs it.
- **No signup-time uniqueness or policy.** The credential table keys on
  the identifier you supply; what a legal identifier is, and what a legal
  password is, are yours.
- **Signing out everywhere exists.** `POST /auth/logout-all` ends every
  session the person has, this one too, and a recovery ends them all as well.
  What does not exist is a *list* of a person's live sessions, because by
  design nothing knows them: a session's whole state is in its token, and
  the only durable record is the sessions somebody ended. So there is no
  deployment-owner view of live sessions either.
- **SSO is OIDC, and only OIDC.** Sign-in through an OpenID Connect issuer
  is built (`ORDO_AUTH_<NAME>_*`, discovery and JWKS through the egress
  kernel; `plan-auth-federated.md`). SAML and OAuth2 without OIDC have no
  implementation. `StubOidcProvider` remains a test double that does no
  network I/O.
- **Passkeys are reachable over HTTP.** Enrolment, sign-in, step-up and
  recovery run through `/auth/begin`, `/auth/complete`, `/auth/step-up` and
  `/auth/recover/*`. They need `passkey` in `ORDO_AUTH_METHODS`, and
  `ORDO_APEX_DOMAIN` as the relying-party id.

Credentials, session revocations, and the directory linking an
authenticator to a principal are all in Postgres (`$credential`,
`$sessionrevocation`, `$principal`, `$authenticator`), each created at
boot with no in-memory fallback. A store that cannot be opened refuses
the boot rather than starting an authentication surface with no
accounts behind it.

### Grants die when the scope they consented to changes

A capability grant pins the combined hash of the **manifests** of the ops
it was granted over. Change what an op touches or discloses, a new
entity, a guard that stopped applying, and the grant is dead until a
human re-consents.

This makes the catalog an enforcement input. In deployment-owner terms: **you cannot quietly
widen what an integration can reach.** Widening it revokes it.

### What your consent surface has to say about space owners

**This one is yours to build, and the sentence is ours to suggest.** The
platform gives you the mechanism and the derived scope; how you present
consent is your product's decision, not ours.

But there is a disclosure the platform cannot make on your behalf and
cannot enforce. `definition-language.md` §3 settles it (Z-004): *"No acl
row is private from the space owner, and that is the definition of an
owner."* An owner who can administer acl rows can grant themselves any
level on any row, so nothing in their space is structurally hidden from
them. It is a **disclosed property**: a space is the unit of
custody, and whoever holds custody can reach what they hold, the same way
a database owner can read their database.

It is bounded by the thing that makes it acceptable: **the owner cannot
look without leaving a record.** Every such reach is an ordinary
attributed command in the space's chain, because the route to reading is
granting, and grants are writes. That is precisely why the owner
capability covers grant, revoke and level change and stops there; a read
exemption would be the one reach that left no trace.

So `bootstrap.md` §5 asks that anyone joining a space they do not own be
told, in their own language, rather than by a policy link:

> the owner of this space can reach anything in it, and every time they do
> it is logged

And the corollary belongs beside it, because it is the actionable half:
**members who need rows the owner cannot reach need a different space, not
a different rule** (`modeling-guidance.md` §A's space-per-user), not a
second owner.

## 5b · Browser pages on another origin (CORS)

The door sends no CORS headers unless you ask. A page served from a
different origin than the deployment (a static site on Cloudflare Pages
calling an Ordo deployment as its backend is the ordinary case) is refused by
the browser: the preflight `OPTIONS` fails and the page never sends the real
call. The failure shows in the browser console only, and the server log shows
nothing wrong. Set the allowlist:

```
ORDO_CORS_ORIGINS=https://app.example.com,http://localhost:5173
```

- Each entry is an exact origin: scheme, host and optional port, no path and
  no trailing slash. Matching ignores case.
- **`*` is refused at boot.** Every call carries a bearer token in
  `Authorization`, and a wildcard would let a script on any site use one.
  List the origins.
- An allowed origin gets `Access-Control-Allow-Origin` on every response and
  its preflight is answered `204` with the methods, the headers Ordo calls
  send (`Authorization`, `X-Ordo-Space`, `Idempotency-Key` and the rest) and a
  ten minute max age. Every response carries `Vary: Origin`.
- An origin not on the list gets `403` on the preflight.
- The door never sends `Access-Control-Allow-Credentials`, because it reads
  no cookie.
- Not set from the console yet. The console is unbuilt (checklist 18o), so
  the setting is an environment variable and a restart.

## 6 · Registration gates (deploy-time, not request-time)

A program that violates these does not deploy:

| gate | what it demands |
|------|-----------------|
| `unacknowledged_unbounded` | an op whose row count grows with the data carries `unbounded` |
| `unacknowledged_unguarded` | an op composing a live guard bypass carries `unguarded` itself |
| `unguarded_superfluous` | the `unguarded` flag must actually strip a guard |
| `id_key_collision` | two entities deriving the same typed-id key, naming both |

All of them are the same idea: a cost or an exposure must be **stated at
the point where it happens**, so it is greppable and it lands in the
manifest a human consents to.

## 7 · Deploying a program, and rolling one back

Compiled ops are data. `ordo apply <dir>` compiles the program, stores its
artifact in Postgres under a **program version hash**, points the deployment
at it, and notifies every replica on `ordo.deploy`. `ordo deploys` lists what
is stored, what is serving, and every re-point in order; `ordo rollback
--version <hash>` re-points at a version the registry already holds.

Three things worth knowing before you run it:

- **Rollback is not a special path.** The pointer is an append-only log, so a
  rollback is an ordinary activation naming an older version: the same
  mechanism forwards. Old artifacts are retained, and a foreign key from the
  activation log enforces that rather than a convention.
- **A node re-runs the load gate on every swap, not only at boot.** A version
  is compiled, its specs registered, its shapes verified against an empty
  shadow built from *its own* DDL, and then compared against the **live**
  schema. Any of those refusing means the node keeps serving what it had; a
  node that cannot load a new version must not stop serving the old one, and
  the refusal is an operator alert rather than an outage.
- **`--no-activate` stores without re-pointing.** `migration.md`'s
  additive-then-swap sequence puts a column in the database one deploy before
  the artifact that reads it, so storing an artifact whose migration has not
  run yet is legal and staged. Activating it before the migration is what the
  live-schema check refuses, by name, listing what is missing.

`ordo apply` asserts an actor with `--as`. It holds database credentials, so
that attributes; it does not authenticate.

### When a new version is served

Activating a version does not mean the door serves it yet. Each node must
first swap to it, rebuild its access engine, switch its routines and bring
every tenant's database to it. Until then, a page that calls an op the new
version added is refused `access_denied`. So `ordo apply` waits.

- **By default it returns when every running node serves the version** and
  has finished its tenant catch-up. It prints `served <version> on N
  node(s)`, how long it waited and how long the slowest catch-up took.
- **A tenant left behind is named, not waited for.** It prints a `held`
  line for each database the catch-up could not bring forward, with the
  reason. That tenant stays closed, and `/health/ready` gives the reason.
  A hold that may pass (the database was unreachable, or busy) is waited
  for until it does or the wait ends.
- **`--wait-secs <n>`, or `ORDO_APPLY_WAIT_SECS`, bounds the wait.** The
  default is 300. Past it the apply exits 1 and names each node and what it
  is still doing. The version stays active.
- **`--no-wait` returns once the version is active**, which was the only
  behaviour before 2026-10-22. Use it in a script that checks later.
- **With no node running** it returns at once: the first node to start
  serves the version.
- **The catch-up moves several tenant databases at once**, as many as
  `ORDO_PG_TENANT_DIAL_CONCURRENCY` allows (default 8). Each database is
  brought forward once, and one that fails is held while the rest go on.
- **A routine a version adds, retimes or removes takes effect at the
  switch**, with no restart. The apply's wait includes it.

How it knows: each node writes one row in `$nodes` in the deployment's own
database. The row says which version the door serves and which version's
catch-up last finished. A node writes it after each switch, after each
catch-up pass and every `ORDO_NODE_BEAT_SECS` (default 5). It removes it
when it shuts down. A row older than three beats is a node that stopped
without saying so, and nothing waits for it. A node older than 2026-10-22
writes no row, so the apply does not wait for it.

**Check it yourself.** `GET /health/deploy` on the door answers the same
thing as JSON, to the deployment owner's token (an anonymous asker gets
`{"status": "settled" | "pending", "settled": true | false}` and nothing
that names a node or a version, audit F12):

```json
{ "version": "6f6dcf1789c1f76e", "settled": true, "held": 0,
  "pending": [],
  "nodes": [{ "node": "replica-a", "serving": "6f6dcf1789c1f76e",
              "caughtUp": "6f6dcf1789c1f76e", "held": 0, "retrying": false,
              "sweepMs": 4120, "seenSecsAgo": 2 }] }
```

`?version=<hash>` asks about one version; without it, the active one.
`settled` is true when no running node is still working on it. `pending`
names each node that is, and what it is doing. `held` is a count: the
database labels hold a host and a port, which this public answer does not
print. `ordo apply` against the database names them. `ordo apply --host`
waits on this endpoint.

A page can ask it too. A page built for a new version can poll it until
`settled` before it calls an op the old version did not have.

### The door: `ordo apply --host`

Once a deployment is running, the ordinary path is its own door rather than
its database (owner-ruled 2026-09-20; 17u and 18l):

```
ordo apply ./program.ordo --host ordo.inf.smg.icu --space <the deployment's own space>
```

signed in with `ordo login` (`howto-identity.md`, section 7), with a
machine key in `ORDO_MACHINE_KEY` for CI, or with a bearer token in
`ORDO_TOKEN` or `--token`. `ordo.inf.smg.icu` is the
default and `--host` with an empty value selects it; `ORDO_HOST` is the
environment form. A bare hostname gets `https`, and `http://` must be
spelled, which is what addressing a local `ordo dev` looks like
(`--host http://127.0.0.1:4000`). With neither flag nor variable the command
talks to the database exactly as it did before: the door is opt-in.

What the far end runs is `std.deployment.apply`, a platform op that calls the
same `ordo_host::apply_op::run` the database path calls. The two print the
same report because they render the same structured outcome through the same
function, and a refusal carries the same class under a wire code
(`apply_refused`, `not_activated`, `apply_in_progress`, `schema_drift`).

Four things about the door are worth knowing before an operator meets them:

- **Who may.** A deployment person or machine key from the identity
  service, wherever it runs. Without it, a deployment **owner**, which is
  an owner of the deployment's own space, the one the binary was
  configured with. Not a manager and not
  an editor of it, and not the owner of a tenant space: the access profile is
  deployment-wide policy installed in every space, so a tenant owner would
  otherwise pass every check and be able to replace the program every space
  runs. Bound to a tenant space the op answers `not_found`, which is the true
  answer: it is not there, and no membership change inside a tenant will make
  it appear.
- **What is recorded.** The actor in `$migrations` is the principal the door
  proved from the token. There is no `--as` over the door and the flag is
  refused with `--host`, because a string that does not reach the audit
  record it looks like it is writing is worse than no string.
- **The token.** With the identity service: a deployment token from it,
  with a passkey touch in the last 5 minutes (`ordo login --touch`), or a
  machine key's token, which the owner allowed to deploy with nobody
  present (plan Q2), in the deployment's own space only. Without it: an
  ordinary sign-in token (`/auth/begin`, then `/auth/complete`), which must
  carry presence because applying is step-up gated. Either way a capability
  token can never satisfy step-up; see `docs/howto-operating.md` for the
  exact calls.
- **Two applies queue.** They serialize on an advisory lock keyed by the
  `$migrations` table, taken before the first read of the schema: what must
  not interleave is the *decision*, not only the DDL, because two applies that
  both read an empty target are two correct executions of a sequence that
  raced at its first read. A second apply waits thirty seconds and then
  refuses with `apply_in_progress` rather than retrying itself: the apply
  holding the lock may be changing the schema the waiter planned against, so
  the retry has to re-plan and only the caller can decide to.

**What stays on the database, and why.** `ordo keys init`, `ordo verify`,
`ordo checkpoint`, `ordo prune` and `ordo revert` do not move and are not
going to. Each has to work when the door does not: verification in particular
is very often run *because* the engine is refusing requests, and a
verification that can only be performed by the thing under suspicion is not a
verification. `ordo keys init` is sharper still: it exists because a
deployment will not mint custody for a space it was merely told about, and
the running server is the thing that cannot boot without the custody it
creates.

## 7b · Upgrading, and what survives it

Live data survives upgrades (ruled 2026-10-19). There are two kinds:

- **The app's program changes.** `ordo apply` of a new version over a
  running one.
- **Ordo itself is upgraded.** A newer `ordo-host` started on the same
  databases with the same keys (`ORDO_TOKEN_KEY`, `ORDO_PAGE_KEY`,
  `ORDO_MASTER_KEY` or the key provider's own).

After either, with compatible program changes (an optional field, a field
with a constant default, an index, a new entity, new ops, a guard clause
that keeps everyone's access):

- every person signs in with the same passkey, emailed code or password, to
  the same account and tenant;
- session tokens and renewal cookies issued before still work;
- rows, files, shares, share links and delegations are intact;
- a write retried with its idempotency key is not written twice;
- the security streams, the sign-in links and each tenant's command chain
  verify (`ordo verify`).

Two things end cleanly, and the SDK recovers from both:

- **An open live feed** carries on across a program update. Across an Ordo
  upgrade its connection closes when the old process stops; the SDK opens it
  again.
- **A page cursor** continues when the op it was minted for is unchanged.
  An Ordo upgrade that changes how ops compile (2026-10-26 did) changes
  every op's version, and the cursor is refused `version_skew` (409). The
  SDK starts that list again from page one.

### How it is tested

`crates/ordo-host/tests/upgrade_continuity.rs` makes all of the above (two
people with passkeys, one with an emailed code, a member with a password, a
share, a share link, a delegation, a file, an open feed, a cursor, an
idempotency key) and checks each item by name after the upgrade.

- **Program update:** runs in `cargo test --workspace`, so on every push.
  About 6 s.
- **Ordo upgrade:** `scripts/upgrade-continuity.sh` builds the previous Ordo
  (pinned in the script, `ORDO_UPGRADE_FROM` to change it) into its own
  target folder, starts its own Postgres cluster, runs both tests, then
  deletes the cluster and the extra build. About 75 s on a laptop. CI runs
  it as the `upgrade` job.

```
scripts/upgrade-continuity.sh            # build, run, clean up
ORDO_UPGRADE_KEEP=1 scripts/upgrade-continuity.sh   # keep the previous build for a rerun
scripts/upgrade-continuity.sh clean      # delete the previous build and the cluster
```

A check that fails is a continuity break. File it under "Reported fixes".
Until it is fixed, list it in the test's `KNOWN_PROGRAM` or `KNOWN_BINARY`
with the reason, so the run names it on every pass, and add a strict copy
of the test, `#[ignore = "KNOWN BREAK: ..."]`, that passes an empty list.
None is listed today.

## 8 · The command log's own operations

`ordo checkpoint --space <spc>` signs an anchor at the head. Checkpoints are
what verification resumes from, and they are the **only legal cut point for a
prune**.

`ordo prune --space <spc>` is the one command that destroys audit evidence.
Read it with `--dry-run` first. It needs no key and answers the question you
actually have: where would you cut, and what is holding the floor. The floor
is the greatest checkpoint at or below both the undo horizon and the slowest
registered consumer, and never past the chain's last entry. A real prune
requires the verifying key, because it checks the cut point's signature *and*
re-verifies the prefix it is about to destroy: deleting tampered history
would destroy the evidence it was tampered with.

Two consequences to plan for:

- **A stalled mirror is evicted from the floor calculation and named.** Only
  a mirror: a listener, an archive sink, and a client-first mirror pin the
  floor however stale, and the supported way any of them stops pinning is
  explicit deregistration.
- **Outstanding client pins are not protected by the floor, by design.** A
  layer-1 pin costs its holder nothing and asks the deployment for nothing,
  so the deployment does not know it exists. Every answered response carries
  `x-ordo-verifiable-from`, the earliest position from which recomputation
  is still possible, so a client that was away across a prune learns it in
  one request instead of by fetching entries that are gone. That header is
  *not* covered by `x-ordo-head-sig`, deliberately; it says what is feasible,
  and the evidence for a lawful cut is the signed checkpoint at that position.

## 8b · Outbound egress, and the two obligations only you can discharge

Everything this deployment sends outward goes through one client
(`ordo-egress`) enforcing `security-logging-design.md` §7b's six pins. Five of
them are enforced in code and need nothing from you: `https` only with
mandatory certificate validation, resolve-then-pin with the connection made to
the resolved address, no redirect following, signed deliveries, and a
per-space allowlist. **Two things are yours.**

**Set `ORDO_EGRESS_DENY_CIDRS` to your pod and service ranges.** The deny-list
*categories* are compiled in and cannot be switched off: loopback,
link-local (which is the cloud metadata endpoint), RFC1918, CGNAT,
unique-local, multicast, reserved. What the platform cannot know is **your**
cluster's internal address plan, because a self-hosted cluster's pod range
differs from every other one. Comma-separated CIDRs, network addresses only:

```
ORDO_EGRESS_DENY_CIDRS=10.244.0.0/16,10.96.0.0/12
```

A malformed value is a **boot error**, not a warning. The failure mode of a
silently-dropped range is that a customer-named sink can reach your own
service network, which is the entire subject of §7b. A CIDR with host bits set
(`10.1.0.0/8`) is refused rather than normalised, because a deny list that
quietly widens a typo is a deny list nobody can review.

If you run a single node with no private pod network, leaving it unset is
correct and is not a misconfiguration.

**Run `attest.deliver` under a credential-less identity.** §7b's fifth pin,
*"egress runs under an identity that holds nothing… this is the control that
survives a bug in the other four"*, is a **process** property, and the
platform can only enforce its static half: `ordo-egress` depends on no
`ordo-*` crate, so the code that opens sockets to customer-named hosts cannot
reach the `KeyProvider` or the pool, and a test asserts that by parsing the
manifest. The rest is yours. The worker slot is named `attest.deliver`
(`plan-runtime.md` R3), so it can be scheduled in a pod of its own with:

- **no instance-metadata access**: deny egress to 169.254.169.254 at the
  network policy as well, because defence that exists in only one place is a
  single bug away from not existing;
- **no cloud IAM role**, or one with no permissions;
- **no KMS/Vault reachability**;
- database credentials scoped to the attestation tables if your database
  supports it.

A deployment that runs everything in one pod is not broken (the other five
pins still hold), but it has not got the control that survives a bug in them,
and it should know which of the two it has chosen.

**What a delivery discloses, for your consent page.** The payload is the
signed head: space id, chain position, head hash, timestamp, signature, the
publication cadence, and the owner key's fingerprint. **No row data, no
actor, no correlation id.** The destination is owner-signed, so re-pointing it
is an owner act with a step-up ceremony and a security-stream record: a
capability token cannot do it at all. Deliveries that fail are retried with
backoff and then dead-lettered; dead letters are visible on the ops surface
and name the destination that was attempted.

## 8c · Usage metering

Off by default. Set `ORDO_USAGE_LEDGER=1` to turn on request, rows-returned,
worker-run, write and byte-stored metering. Off, `MeteredExecutor` and
`TriggersRunner` still call into the ledger on every request and every run,
but the ledger is a no-op, so the cost of leaving it off is one branch, not
a database write. Nothing here phones home: everything metering writes
stays in your own control database (`$usage_replica`, `$usage`), and
nothing reads it from outside your deployment unless you build a reader.

**Two workers, two cadences, both configurable.**

- **`usage.flush`** (`ORDO_USAGE_FLUSH_SECS`, default `60`): every
  replica's own loop. It writes what that one process has counted since its
  last flush into `$usage_replica`, keyed by `(space, period, replica)`, so
  two replicas in one period are two rows that sum rather than one row a
  race could drop. Requires no leadership: every replica flushes itself.
- **`usage.rollup`** (`ORDO_USAGE_ROLLUP_SECS`, default `3600`, one hour):
  leader-only. It reads `$commandlog` between the last rollup's position
  and the current head, `$sealedpayload` and `$blobref` for exact byte
  sums, and the live schema's entity tables for an estimated byte
  footprint, and writes one `$usage` row per space per period.

**The trade each cadence makes, so you can tune it rather than guess.**

- A shorter flush costs one write per replica per interval and loses less
  on a crash: a replica that dies mid-period loses at most one interval's
  counters, recorded as `'open'` rather than silently smoothed over. A
  longer flush costs fewer writes and risks more.
- A shorter rollup gives fresher quotas and usage dashboards: the sums a
  quota check or a billing report reads are never more than one rollup
  period stale. The cost is more `$usage` rows, one hour keeps it at 24
  rows per space per day, one minute is 1,440.

Both variables reject `0` (a cadence that never sleeps between ticks) and
any value that fails to parse as a whole number of seconds, at boot, before
the first dial. That is the same posture `ORDO_PG_POOL_SIZE` and every other
tuning knob in §1's table take: configurable, and a malformed value refuses
rather than silently falling back to the default.

### 8c·1 Quotas (17n M2)

No environment variable: a limit is per-space and set by the deployment owner, with
`ordo quota set --space <s> --quantity <requests|rows_returned> --limit <n>
--grace <n> --period-secs <n>` (`ordo quota show --space <s>` to read it
back). `ApiState.quota` is `PgQuotas` exactly when `ORDO_USAGE_LEDGER=1`;
off, it is `NoQuota`, which is always `Within` and never refuses. A space
with no `$quotas` row is never refused under either.

**Soft by default.** Over the limit alone warns and serves
(`ordo_quota_over_limit_total{space,quantity}`); over limit **plus** grace
refuses with `quota_exceeded` (429). `--grace 0` makes it a hard stop.

**The gate runs after the access check and never before**: the ordering
is the point, not an implementation detail. A quota refusal says the space
exists and is over its plan; a caller with no membership in that space
gets `access_denied` instead, identically to a non-member of any other
space, so the door's non-enumerability property (§4.1,
`api-engine-design.md`) survives quotas intact.

**Freshness.** `PgQuotas::check` is synchronous and reads only an
in-memory snapshot; it takes no database handle at all. `usage.flush`
rebuilds that snapshot on every tick, after writing this replica's own
`$usage_replica` rows: `$quotas` plus `SUM`s over `$usage_replica` for
each configured quantity's own period, plus this replica's live,
not-yet-flushed counters. A limit this replica's own traffic just crossed
therefore takes effect on the very next request rather than waiting a
flush interval, and a database outage cannot become a door outage.

**v1 gates `requests` and `rows_returned` only.** Both are counted at the
door as the request happens, so the quota is exact. `writes` is counted
from the command log at rollup and a quota on it would notice an overrun
up to one rollup period late. That is 18i, on the post-deployment roadmap, per
the owner's ruling (`corpus-rulings.md`, `write-quota`, 2026-09-19).
`ordo quota set` refuses any other quantity by name.

### 8c·2 Usage (17n M3)

`ordo usage --space <s> --from <secs> --to <secs>` and `GET
/usage?space=<s>&from=<secs>&to=<secs>` both render through
`ordo_exec::usage::render` over the same `between` read, so a caller
reading either surface for the same closed period reads byte-identical
JSONL. The route is gated like `/metrics`: reserved op name
`platform.usage`, checked against the deployment's own space regardless of
which space `space=` names, since this is an operator surface over every
tenant the process serves. No new environment variable: access is the
deployment's ordinary access model, the same one `/metrics` and
`/security/alerts` already use.

**This is the meter.** Pricing, plans, payment,
proration and currency are explicitly out of scope, on purpose: this
platform's job ends at emitting the facts, and whoever bills on top of
them takes on the regulatory and financial obligations that come with it.

## 9 · Operating checklist

- First deployment: an empty database, the secrets, `ORDO_SPACE=system`,
  start the binary. It creates its own tables and its own space; nothing is
  run by hand before it (§4c).
- `ordo keys init --space <tenant> --as <you>` for each tenant, before a
  deployment names that tenant in `ORDO_SPACE`.
- `ordo check` before deploy: entities, keys, guards with provenance, per
  op the cost class, row bound, and output shape.
- `ordo apply` to store and activate; `ordo deploys` to see what is serving.
  Against a running deployment, `ordo apply --host <h> --space <s>` with a
  token in `ORDO_TOKEN` needs no database credentials at all.
- `ordo prune --dry-run` before `ordo prune`, every time.
- Point `ORDO_PG_*` at the database; the pool dials at boot and fails loud.
- Set the page key, the token key, and the `KeyProvider` from real secret
  material, not defaults.
- If a browser page on another origin will call the deployment, set
  `ORDO_CORS_ORIGINS` (§5b). Unset, that page cannot read any answer.
- Watch for `Saturated`: it means the deployment is at its declared
  concurrency bound. Raise `size` only if the *database* has headroom.
  Otherwise you are moving the queue somewhere you cannot see it.
- Watch `ordo_pool_space_capped_total{space}` beside it: it says which
  tenant is holding the connections, and it is there with per-space
  metrics off (§2c).
