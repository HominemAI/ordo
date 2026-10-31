# Ordo Implementation Choices

Companion to `definition-language.md`. Same markers: [settled] / [lean] /
[open], plus **[proposed]**, a gap this document fills with a concrete design
that has not been ruled on yet.

## Language & runtime [settled]

- **Rust** for the entire platform: compiler, core-api, workers, identity, crypto.
- **Web UI stays TypeScript/React**. Rust ends at the API boundary.
- **No WASM in v1**, and nothing foreign runs in-process at all: tier-1
  routines are `.ordo` rules (our compiler, our executor); tier-2 intelligence
  is external service-principals over the API. The host loop invokes routines
  through a `RoutineExecutor` trait (one implementation today: the rules
  engine), so an in-process sandbox executor (wasmtime) is a future *addition*
  behind the same interface, with no contract, consent, or language changes.

## Compiler [settled]

- Server-only. No client ever parses `.ordo`; browsers call registered ops via
  a generated typed SDK (names, param types + bounds, result shapes from
  registration metadata). Dev tooling validates via a server compile-check
  endpoint.
- **Compile once per query per version.** Compilation is a deploy-time event.
  `ordo apply` stores artifacts (planned SQL, pinned refs, mode label, cost
  class, scope manifest) keyed by version hash. Runtime = op lookup → access
  check → bind params → execute prepared plan. The parser is never on the hot
  path; requests carry no language.
- Shape changes recompile dependent ops within the same deploy; diffs surface
  any label/cost-class drift for review.
- **Hot deployment [settled]**: compiled ops are data, and the artifact registry
  lives in Postgres; nodes hold an in-memory cache. `ordo apply` writes new
  versions and nudges nodes (LISTEN/NOTIFY); each swaps atomically. In-flight
  requests finish on their version, the next request gets the new one. No
  restarts; the process restarts only for platform-binary upgrades. Old
  artifacts are retained, so **rollback = re-point the registry at the prior
  version hash** (same mechanism, backwards, also restart-free). Additive
  migrations apply online in the same flow.
- **Dev mode [settled]**: `ordo dev` watches source, recompiles on save
  through the real compiler, swaps sub-second. Relaxations: auto-confirm
  destructive migrations, skip version-history bookkeeping.
- Parser: hand-written recursive descent (picked in `engine-plan.md`, built
  in `crates/ordo-syntax`). IR as real ADTs.

## Runtime shape verification & the capability catalog [settled]

After compile, at load (and on every hot swap), the platform **refreshes its
own knowledge of every registered function** and publishes it:

- **Empty-set execution as ground truth.** Each op's planned SQL runs once
  against an **empty shadow instance of the schema** (scratch Postgres schema
  built from the same DDL, zero rows). The result carries no data but
  Postgres reports the exact result columns and types, the op's **observed
  shape**. The compiler's statically inferred shape must match the observed
  shape exactly (names, types, nullability, order); mismatch fails the load
  loudly. Shape drift between planner and database is a bug that can never
  reach a caller. Ops whose stages are host-side (sealed compute) verify the
  SQL portion this way and the host portion against the typed IR. The
  inferred shape is authoritative there, and marked so.
- **The catalog.** The verified shapes, joined with registration metadata,
  become the deployment's **capability catalog**. Per op it holds: name, params with
  types and bounds, exact result shape, mode label (reads: live/pageable;
  writes: creates/converges/…), cost class, refinable fields, required
  step-up. Two renderings from one source: machine JSON (the interface
  descriptor the SDK generators consume; same artifact, now
  runtime-verified) and an **LLM-ready document** (compact prose-plus-
  signatures, "if you want to work with this data, here are your options"),
  served at a well-known endpoint alongside the grammar prompt (§10's two
  pages), so an agent's context can carry the entire callable surface.
- **Scoped to the asker.** The catalog is access-checked like everything
  else: a token sees only the ops it may call. An agent's catalog *is* its
  consent page in machine form. Catalog responses carry the registry version
  hash; a hot swap refreshes shapes and republishes atomically with the swap.

## Server stack

- axum + tokio; tokio-postgres + deadpool. Job queue: minimal in-house
  LISTEN/NOTIFY + `FOR UPDATE SKIP LOCKED` (no external broker; per earlier
  ruling, log-as-bus is implementation, not promise).
- Crypto: RustCrypto (XChaCha20-Poly1305 sealed payloads, chunked STREAM format
  for blobs so range reads work), HMAC blind indexes. **Every seal uses a fresh
  random nonce, and the sealed payload store never deduplicates** [settled].
  Equal plaintexts must never produce equal ciphertext or equal payload
  identity, because the command log's `SealedRef` sits in a plaintext envelope
  and would otherwise become a permanent equality index over sealed fields
  (`security-logging-design.md` §7). **Key custody behind a
  `KeyProvider` trait from day one**, the same resolver pattern as databases,
  blob storage, and routine execution: implementations for cloud KMS (GCP,
  AWS), Vault, and a local master key for dev/small self-hosts. No provider is
  hardcoded anywhere.
- Blob storage: S3-compatible client (R2 first); space→bucket resolver mirrors
  the space→database resolver (BYO bucket = BYO-DB pattern).
- Two binaries, one workspace: `ordo` (the operator tool: apply, revert,
  prune) and `ordo-host` (the server). They ship as separate images with
  different bases and different blast radii (`plan-ci-images.md` §3.1); the
  server's role (api / worker / all-in-one) is a runtime flag (`ORDO_WORKERS`),
  never a rebuild. Self-host = `ordo-host` + Postgres (+ optional bucket).

**The mechanism for the binary, key custody and the worker supervisor is
settled, not a lean guess: `plan-runtime.md` (1,915 lines, owner-approved GO,
executing) specifies each against the code that already exists, with a
done-when per increment (§6).**

- **The binary (R1)** assembles around a load path that already exists:
  `deploy::load_active` (`crates/ordo-host/src/deploy.rs:357`) reads the
  active artifact, compiles it, registers its specs, runs the shape gate,
  and returns the `Verified` state `PlatformState::new` requires. R1 adds
  the socket, configuration-by-environment, and graceful shutdown around
  that path: assembly, not invention. **Not yet merged to main**: there is
  no `[[bin]]` in `crates/ordo-host/Cargo.toml` and no
  `crates/ordo-host/src/main.rs`; the only `TcpListener::bind` in the tree
  today is the playground's.
- **Keys (R2)** run behind the `KeyProvider` trait named above; the
  production path wraps `LocalKeyProvider` over a Postgres-backed
  `PgKeyStore`, sourced from an operator-supplied master key (env, a sealed
  file, or a KMS-wrapped file) rather than `TestProvider`, whose master key
  is a constant in this repository (`crates/ordo-crypto/src/custody.rs:501`,
  `:726`) and must never be what a deployment's write path seals through.
- **The supervisor (R3)** runs long-lived workers (clock leadership, drain,
  triggers) under a `Worker` trait with panic capture, exponential backoff,
  and quarantine after ten consecutive failures, driving the loop steps
  that are already built and tested in `crates/ordo-exec/src/triggers.rs`.

Each increment's done-when is stated in full in `plan-runtime.md` §6; for
example, R1: `cargo build -p ordo-host --bin ordo-host` boots against a
service Postgres, `/health/ready` answers 200-or-503, and `SIGTERM` under
load drains in-flight requests; R3: a write reaches a registered
`RunHandler` through production `drain_listener` → `fire_due` →
`claim_runs` → `finish_run`, and a panicking worker restarts, is counted,
and quarantines while the door keeps serving.

## Deployment topology [settled]

**One scalable pod + a Postgres connection is a complete deployment.** The
all-in-one role serves API, live sessions, triggers, and jobs from one
container; replicas scale horizontally with **no coordination service
beyond Postgres**. Every cross-pod concern rides the database:

- **Invalidation & hot swap**: LISTEN/NOTIFY fans out to every replica; SSE
  sessions need no stickiness (any pod serves any subscriber; each pod
  relays notifications to its own sessions).
- **Job queue**: `FOR UPDATE SKIP LOCKED` in Postgres. Replicas compete
  safely, with no broker.
- **Clock triggers**: **at-most-one-holder** via a Postgres advisory-lock
  leadership claim, plus the schedule row's own `FOR UPDATE SKIP LOCKED` claim,
  which is what actually delivers *fires once*. The two are not the same
  property and the earlier "exactly-one-firer via advisory lock" conflated
  them: a lock guarantees that at most one replica holds leadership at a time.
  It says nothing about a holder that dies mid-fire, and it does not survive
  being taken on a pooled connection (below). Exactly-once is the **claimed
  schedule row committing in the firing transaction**; the lock only keeps
  replicas from stampeding.
- **Chain append**: per-space advisory lock serializes group hashing (the
  chain is a per-space ordering point by design; the lock makes it explicit
  and scale-out-safe). This is a **throughput characteristic, not an
  implementation detail**. See "Write path" below, which states what it
  costs and when it bites.
- **Connection pooling is a correctness constraint on both of the above**
  [settled]. A **session-scoped** advisory lock (`pg_advisory_lock`) is owned by
  a *backend session*, and under a transaction-pooled pgbouncer a session is
  handed to a different client after every transaction, so the lock outlives
  the work it was protecting, is released by whoever happens to hold that
  backend next, or is never released at all. Two rules:
  - **Chain append and the security-event stream take transaction-scoped locks**
    (`pg_advisory_xact_lock`), acquired inside the transaction that appends and
    released by its commit or rollback. This is pooling-agnostic and is the
    default for everything on the write path.
  - **Anything genuinely session-scoped (the leadership claim) requires a
    session-pooled or direct connection**, declared as such in the deployment
    manifest, or must be re-expressed as a **lease row** with an expiry the
    holder renews. A deployment running leadership through a transaction pooler
    is misconfigured; the binary asserts the connection mode it needs at
    startup rather than discovering it during an incident.
- **Idempotency replay cache**: Postgres-backed (a replay table), never
  in-memory-only. A retry landing on a different replica must still
  replay and not re-execute. Keyed and fingerprinted per
  `api-engine-design.md` §9; the in-memory implementation is a test double,
  and a multi-node deployment running on it is a **correctness** defect, not
  a missing feature.

**Jobs sidecar / split**: the worker role runs as (a) the same all-in-one
process (small deployments), (b) a **sidecar container** in the API pod
(job capacity scales with API replicas, which is fine when load correlates), or
(c) a separate worker Deployment (independent scaling for heavy routine/
import load). All three are the same binary with a different flag; moving
between them is a manifest edit, never a rebuild.

**The full manifest set for production** (beyond the pod + PG): object
storage (R2/S3 or none if every blob field is `inline`/`uri`); a
KeyProvider (cloud KMS / Vault / local master key for small self-hosts);
ingress with SSE-friendly settings (response buffering off, sensible
idle timeouts) and a stable apex domain, supplied by configuration
(`ORDO_APEX_DOMAIN` [proposed], see `licensing.md`). It is the passkey RP
id, so changing it
strands authenticators. Pick it once; an email provider for OTP login;
Postgres backups (plus bucket lifecycle rules); and standard
metrics/log scraping. Nothing else is needed: no Redis, no broker, no coordinator,
no cron runner. Postgres is the only stateful dependency that must exist.

## Outbound egress: sinks, webhooks & feeds [pinned]

Every outbound HTTP destination in this platform is **named by a customer** and
fetched **from inside the deployment's own perimeter**. Until now nothing in the
corpus validated one, which made a per-space notification sink a path from any
space owner to `http://169.254.169.254/…`, the cloud metadata endpoint, whose
response is instance credentials, which are the credentials the `KeyProvider`
trusts. That is a single-tenant feature reaching **deployment-wide key
custody**, and it is the one outcome the whole custody design exists to prevent.
`http://localhost:5432` and the pod/service network are the same class of
target, and because sinks deliver payloads and log deliveries, the response or
its metadata comes back to the tenant.

The rule is owned by `security-logging-design.md` §7b; the implementation is
owned here, and it is not optional or per-deployment:

- **`https` only**, certificate validation mandatory, TLS 1.2 floor. No opt-out
  flag exists, in any config surface, at any deployment size.
- **Resolve-then-pin, re-checked at connect time.** Resolve at grant time and
  again at every delivery; reject loopback (including `0.0.0.0/8`, which
  reaches localhost on Linux), link-local (169.254.0.0/16, fe80::/10),
  unique-local (`fc00::/7`), RFC1918, CGNAT, and the deployment's own
  pod/service CIDRs; **connect to the resolved address**, not the hostname, so
  a DNS rebind between check and connect cannot win. **Every** resolved address
  must pass, not just the one used, and addresses are canonicalised
  (IPv4-mapped, IPv4-compatible, NAT64 `64:ff9b::/96`) before the check.
  `::ffff:127.0.0.1` is not loopback by `Ipv6Addr::is_loopback` and reaches
  127.0.0.1 anyway. The deny-list CIDRs are deployment config (a self-hosted
  cluster's pod range differs, `ORDO_EGRESS_DENY_CIDRS`), the *categories* are
  not, and no flag switches one off.
  Implemented in `ordo_egress::deny`.
- **Redirects are never followed.** A 3xx is a delivery failure.
- **Deliveries are signed**: `X-Ordo-Signature` = HMAC-SHA256 over
  `version ‖ timestamp ‖ destination ‖ body`, each part length-prefixed
  (u64 big-endian), under a per-sink secret shown once at grant, with a replay
  window. Never the checkpoint signing key: the payload's Ed25519 signature is
  minted on the attestation cadence and copied verbatim, so the egress
  identity holds a per-sink shared secret and nothing that authors history.
  Implemented in `ordo_egress::sign`, with the receiver's half published in the
  same module.
- **Egress runs as its own identity with no instance-credential access and no
  `KeyProvider` reachability**: a dedicated egress worker or an outbound
  proxy. This is the control that still holds when one of the others has a bug,
  which is the only reason to pay for a separate identity at all.

  In one process the full sentence is a **deployment** obligation
  (`deploying.md`, Outbound egress), and the static half is enforced by the
  dependency graph: `ordo-egress` depends on **no `ordo-*` crate**, so the code
  that opens a socket to a customer-named host cannot name `KeyProvider` or
  `Pool`, and `ordo-egress/tests/holds_nothing.rs` asserts it by parsing the
  manifest. `plan-runtime.md` R3's slot `attest.deliver` is what makes the pod
  half schedulable.
- **Per-space egress allowlist**, consented at sink creation and rendered on the
  space's consent page (including the deny-list disclosure: "this sink may not
  point inside the deployment"). Adding a host is a consent event, never a
  config edit.
- **Same rules for every outbound fetcher**, not just notification sinks: feed
  delivery, import/sync bridges, and any future callback share the egress path
  rather than each growing their own HTTP client. One client, one policy, one
  place to audit. That client is `ordo-egress`, shipped 2026-09-08 as the first
  increment of its first consumer (the attestation webhook); the pins are
  constructors and moves rather than checks, and `corpus-rulings.md` records
  the shape of each.
- **No response byte reaches the caller.** A delivery returns a status code and
  nothing else: response headers are consumed only to find where they end, and
  the body is read to a cap and dropped without being decoded. This closes the
  exfiltration half of SSRF by return type and not by a rule about logging.
  A receiver that wants to say something says it with a status code.

## Write path: isolation, atomicity & chain-append throughput

### Isolation level per operation class [proposed]

The spec's atomicity guarantees have to name an isolation level, because
"evaluates atomically at commit" is not something PostgreSQL does at the
default level. Pinned per class, and the language's phrasing should be read
against this table.

| Operation class | Isolation | Why |
|---|---|---|
| Reads (incl. `?live` initial reads) | READ COMMITTED, read-only txn | replica-eligible; log-positioned reads (below) supply cross-query consistency, not the isolation level |
| Addressed write: create / CAS converge / delete by id | READ COMMITTED | the conflict detector is `rev` (CAS) and the unique constraint, both exact at any level; misses are typed `cas_miss` / `duplicate` |
| Converge-or-create (the one blessed upsert) | READ COMMITTED + `INSERT … ON CONFLICT` | atomic in one statement, **except** when the converge RHS reads a sealed field, which is host work and cannot be one statement: the recipe is `INSERT … ON CONFLICT DO NOTHING`, then `SELECT … FOR UPDATE` on the unique key, decrypt, `UPDATE`, all inside the group's transaction. Pinned here because "race-free" otherwise depends on an unwritten recipe |
| Bulk `set` / `del` with a condition | **SERIALIZABLE** | this is the class whose condition the spec describes as evaluating atomically; only SERIALIZABLE buys that sentence |
| Any command group with more than one statement | **SERIALIZABLE** | a group is atomic across its statements by definition; mixed read-then-write groups are exactly the anomaly READ COMMITTED admits |

- **Retry policy is part of the pin.** SERIALIZABLE means serialization
  failures (`40001`), and a database-level retry is not enough: the retry
  boundary is the **whole command group**, re-executed from scratch. Three
  properties make that safe and all three are load-bearing: triggers and
  invalidations fire **on commit**, so an aborted attempt fires nothing; the
  idempotency record is written **in the same transaction as the group**, so a
  retried group cannot consume its key twice or return a different answer than
  it stored; and the chain append happens once, at the end, for the attempt
  that commits.
- **Retry budget is finite** (small, with jitter). Exhaustion is a *retryable
  conflict*, not an internal error. It needs a wire code, and the closed
  taxonomy does not have one yet. It is flagged in `api-engine-design.md` §4 as
  `serialization_conflict` [proposed].
- Reads never take a serializable snapshot, so nothing on the read path pays
  for this.

### Chain append is a per-space serialization point [settled as a characteristic]

`hash = SHA-256(prev ‖ group)` forces a **total order per space**. That is the
point of the chain, and its cost is structural rather than incidental: every
write in a space passes through one ordering point, so **per-space write
throughput is bounded by chain-append rate**, no matter how many replicas
serve the API.

- **What is not contended.** The lock is *per space*, so spaces are independent,
  and horizontal scale across spaces is unaffected. Reads never take it.
  Parsing, planning, binding, access checks, and statement execution all happen
  outside it: the lock is taken as late as possible, covering group hashing and
  append only.
- **When it bites.** (a) One space with many concurrent writers, the case
  `modeling-guidance.md` actively steers toward ("one space per household /
  team / project"), so the guidance and this characteristic are in tension and
  the tension is real. (b) Bursty machine writers in a space: feed arrivals,
  source mail-slots, import backfills, and trigger-driven converges all queue
  with human writes. (c) Anything that widens the critical section, such as a slow
  `Signer`, a synchronous archive write, or a long transaction that holds the
  lock across statement execution. (c) is the failure mode to guard against in
  code review; (a) and (b) are the ones to size for.
- **No benchmark exists.** No writes/sec, no p99, no rows-per-space, no
  concurrent-space count has been measured, and nothing in these docs should
  be read as implying one. The number is a **release-blocking measurement**:
  single-space write throughput under the lock, p99 op latency, and restore
  wall time (below) are the three, and `modeling-guidance.md`'s space-sizing
  advice is provisional until they exist.
- **Instrumentation before optimization.** `ordo_chain_append_lock_wait_seconds`
  (Observability, below) is the production answer to "is this biting yet",
  and it must exist before any tuning does. **It now does**
  (`ordo_host::metrics`, checklist 5f): the registry holds histograms as well
  as counters, `Metrics::quantile` computes what Prometheus's
  `histogram_quantile` computes, and `TxConn::append_group` times the
  `pg_advisory_xact_lock` statement alone, so the series is the *wait* and not
  the append. `ordo_op_duration_seconds` exists alongside it, which unblocks
  the p99 measurement.
  **The wait is instrumented as a pair.** `ordo_chain_append_seconds` measures
  the critical section itself (head read, duplicate check, hash, insert), and
  the pair is what makes either number a diagnosis: a
  wait that grew while the section stayed flat is more writers, and a wait
  that grew because the section grew is (c) above, which was previously
  something only code review could catch. Neither series is a benchmark and
  neither is the release gate; they are what lets the gate's run say *why*.
- **Shapes if it does bite [proposed], in ascending disruption**: group-commit
  (batch several groups, hash them under one lock acquisition, keep per-group
  hashes as leaves of a batch root); a chain over batch roots rather than per
  group (cheaper but coarser: revert and per-group verification get one
  indirection); sub-space chains with a periodic join (breaks "one chain per
  space", which is a promise, so it is last). None of these is v1, and none
  should be built before the measurement.

## Migrations: failure modes, rollback & recovery [proposed]

`migration.md` classifies migrations additive / transforming / destructive and
runs the heavy ones as resumable backfill jobs. What follows is the missing
half: what a **failure** leaves behind, and what the operator does about it.

**The invariant that makes recovery possible:** a deploy never destroys the
readable shape in the same deploy that introduces the new one. Additive →
backfill → verify → swap → soak → destructive is **three deploys**, and only
the third is unrecoverable without a restore.

| Failure window | State it leaves | Operator action |
|---|---|---|
| **Before DDL**: plan accepted, nothing applied | none; registry still serves the previous artifacts | re-run forward |
| **During DDL** | DDL for one migration runs in **one transaction** (PostgreSQL has transactional DDL, and this design uses it), so a death rolls the whole statement set back to the old shape, and the old artifacts still serve. Exception: `CREATE INDEX CONCURRENTLY`, which cannot be transactional and leaves an **invalid index**, inert for reads and writes, but it exists | re-run forward; for the index case `DROP INDEX` then re-run. The invalid-index state is surfaced, not inferred |
| **During backfill** (the "dies at 60%" case) | the new column/table exists and is **partially populated**; the **source column is still present and still authoritative**, because a transforming migration never drops its source before the swap. The artifact registry has **not** been re-pointed. Every op still reads the old shape, so reads and writes are correct and complete throughout | resume (default; the job is chunk-checkpointed exactly like custody conversion), or abandon → drop the new column. Neither loses data |
| **After backfill, before swap** | both shapes present and verified equal (count + per-entity content hash, the same verifier custody conversion uses) | swap is the ordinary registry re-point: atomic, restart-free, and **rollback is the same swap backwards** |
| **After swap, before the destructive deploy** | new shape authoritative, old column still present through the soak window | rollback is a registry re-point. This is the last cheap rollback |
| **After the destructive deploy** | source gone | rollback is a **restore** (next section). This is why the drop is its own deploy with its own confirmation |

- **Partial state is visible, never inferred.** A migration job in any of these
  windows is a first-class row in the dashboard's registry/deploy view
  (`dashboard-plan.md` §3): phase, chunk cursor, rows remaining, verification
  result, and whether the registry has been re-pointed. An operator never has
  to read logs to learn whether a space is half-migrated.
- **Forward-only, with an escape.** Transforming migrations are **resumable
  forward** and are not reversible in flight. The reverse of a half-applied transform
  is a new transforming migration, authored and planned like any other. What is
  reversible is the *swap*, and that is the only irreversible-
  looking step that is actually cheap.
- **The write barrier: v1 freezes, and says so** [settled; X-071 / Z-011 /
  C4-008, open across three review rounds]. The table above is correct about
  *crashes* and was silent about *concurrency*, which is where the actual data
  loss lives. A write that lands in a chunk the scan has already passed goes to
  the source column and not the new one; if it lands after verification, the
  swap makes it disappear. Resumability does not help: the job resumes at its
  cursor, which is already past the row.

  So a transforming migration **quiesces writes to the entity being
  transformed, from scan start through verified swap.** Reads are unaffected
  throughout; the source column stays authoritative and every op still serves
  the old shape. Only writes to that one entity block, and they block with a
  typed, retryable rejection naming the migration, not a hang.

  **The cost is real and belongs in the operator's face.**
  `ordo plan` states the freeze, names the entity, and estimates the duration
  from live row count and measured chunk throughput before anything runs; an
  operator who finds the number unacceptable declines the plan. For a large
  entity this is a genuine write outage, and the plan shows the number up front.

  **The upgrade path is a decision to take later:**
  dual-write from a watermark: write both shapes past the watermark, backfill
  behind it, drain the post-watermark tail under a short barrier, then swap.
  That reduces the freeze to the drain. It is deliberately **not** in v1: it
  adds a second write path, a watermark protocol and a drain-completeness proof
  to a subsystem whose failure mode is silent data loss. The freeze is provable
  in a paragraph. When someone's row counts make it intolerable, that is the
  moment to build the watermark, with a real workload to test it against.

- **Drill it.** The conformance/DR suite kills a transforming backfill
  mid-chunk and asserts: old-shape reads still correct, registry untouched,
  job resumes to completion, verification passes. It also **writes to a row
  behind the scan cursor during the freeze and asserts the write is rejected,
  not silently dropped**, and asserts that the post-swap row set is
  byte-identical to a freeze-free run over a quiescent database.

## Backup, restore & disaster recovery [proposed]

- **What is backed up.** PostgreSQL (base backups + WAL archiving for PITR) and
  object storage (versioned bucket + lifecycle rules). **Nothing key-shaped.**
  Key material lives in the `KeyProvider` and must never appear in a database
  backup. This is a deployment invariant worth a **startup assertion** (a local
  master key configured *inside* the backed-up database is a misconfiguration
  the binary refuses to start on).
- **A restore restores ciphertext.** Because keys are not in the backup, a
  crypto-erased space stays erased through any restore that does not also
  restore the KeyProvider's key material. The caveat is the provider's
  soft-delete window: erasure receipts state the real residual window derived
  from the provider (see the compliance pack and "Key custody &
  DEK rotation" below).
- **RPO/RTO are per-deployment and currently unmeasured.** No restore has been
  timed; no number is claimed here. Restore wall time is one of the three
  release-blocking measurements named above.

**Chain continuity, the part that is not ordinary Postgres DR.** Restoring a
space to an earlier point rewinds its chain: every checkpoint issued after the
restored head, and every head hash an owner or client pinned, now names a chain
the database no longer contains. The pin:

- **A restore is a fact in the chain, not a silent rewind.** On restore, the
  platform appends a `system:restore` command group as the **first group after
  the restored head**, carrying the restored head hash, the pre-restore head
  hash where it is known (from the last checkpoint or an owner-held pin), the
  restore point, and the operator. `verify(space)` then reports *"chain valid;
  one restore discontinuity at position N"* and not *"tampered at N"*.
  That is the difference between a documented event and an alarm.
- **Orphaned checkpoints are retained, not deleted.** A checkpoint naming a
  position beyond the restored head is marked orphaned and kept. It is
  evidence about what the chain used to say, and destroying it is exactly the
  move a dishonest operator would make.
- **This weakens the chain's guarantee and the trust model must say so.** An
  operator who can restore can rewind history; the discontinuity marker makes
  that *visible* to anyone verifying, and visible-to-the-owner is the strongest
  claim available while the platform holds the checkpoint key. See the
  exportable-proof mitigation under "Client-first" below.

**What a restore drill looks like** (quarterly, and after every destructive
migration):

1. Restore the latest base backup + WAL to a scratch instance; record wall time.
2. Run `verify(space)` over a sample of spaces; assert exactly one
   discontinuity marker and no other break. **Not yet runnable**: nothing
   appends the `system:restore` group and `verify` has no concept of a
   discontinuity, so this step has no marker to find and the drill cannot
   produce the number step 5 publishes.
3. Re-run **empty-shadow shape verification** for the whole artifact set
   against the restored schema. A restore can land the database on a shape the
   registry does not expect, and this is the check that catches it. The empty
   shadow alone does not do it: both sides of that comparison descend from the
   artifact's own DDL, so it proves the planner and Postgres agree and says
   nothing about the *restored* database. `ordo_exec::shape::Pool::live_drift`
   is the half that looks at the live schema, and it is what makes this step
   executable.
4. Execute one read op per domain against a known row set and compare.
5. Publish the wall time. A drill that does not produce a number did not run.

**Client-first spaces.** The server's copy is ciphertext plus oplog segments,
so a restore rewinds *sync state*; devices re-anchor through the ordinary
segment-acknowledgment path, which means a restored server looks to a device
like a server that fell behind, a case the protocol already handles. The
corollary: for a client-first space the **device is
the authoritative copy**, so DR for that space assumes at least one enrolled
device still holds it, and platform DR alone cannot reconstruct one.

## Observability [proposed]

Previously an open question. For a platform whose pitch is auditability that
was untenable on its own; it is also a **prerequisite for two security
controls**: denial-volume alerting (`security-logging-design.md` §6.1) and
key-release breadth alerting. Neither can exist without a substrate.
Denial-volume alerting is also the only surface on which a probe *sweeping*
spaces is visible at all, since the per-space stream deliberately records
members only.

- **Traces: OpenTelemetry, one span per request.** Root-span attributes are
  fixed: `ordo.op`, `ordo.op_version`, `ordo.space`, `ordo.principal`,
  `ordo.principal_kind`, `ordo.correlation` (**the command-group correlation
  id**, so a trace and a chain group join on one key), `ordo.cost_class`
  (registered and effective; the two differ under refinement, see
  `api-engine-design.md` §7), `ordo.outcome` (the closed error code or `ok`),
  and `ordo.replay`. Child spans mirror the dispatch pipeline exactly:
  registry lookup → access check → bind → refinement validation → cursor decode
  → plan execute → host stages → key release → chain append. The chain-append
  span carries the lock wait, which is where the per-space serialization
  question gets answered in production.
- **What never appears in a span, log line, or metric label**: parameter
  values, row values, sealed plaintext, page tokens, `Idempotency-Key` values,
  bearer tokens. Telemetry is **envelope-tier by the same rule the command log
  follows**. Like the command log, it therefore discloses entity names,
  actor, and timing at every custody level (see the envelope-disclosure note
  under custody conversion).
- **Structured logs**: JSON, one event per request, carrying the same
  correlation id. Alongside them are two distinct security records that must
  not be conflated:
  - **The per-space security-event stream** is append-only and hash-chained,
    members-only, with its own head, checkpoints, and retention. It is the
    *tamper-evident* record of denials, and it is owned by
    `security-logging-design.md` §6.1, not by this section. It is not part of
    the command chain and never takes the chain-append lock.
  - **The deployment security log** is ordinary structured logging, where
    everything without a space relationship lands: denials from non-members and
    anonymous callers, plus step-up demands and outcomes, key releases (space,
    purpose, correlation), break-glass activation, grant mint/expiry,
    authenticator link/unlink, and first-login provisioning.
  Metrics (below) are the *alerting* substrate over both. Three artifacts, three
  retentions, one correlation id joining them.
- **The metrics that matter** (short by intent; per-space series are opt-in
  because space is high-cardinality):

  | Metric | Answers |
  |---|---|
  | `ordo_chain_append_lock_wait_seconds` (histogram) | is the per-space serialization point biting |
  | `ordo_chain_append_seconds` (histogram) | how wide the critical section is (the denominator the wait above is only interpretable against) |
  | `ordo_op_duration_seconds{op,outcome}`, `ordo_op_errors_total{code}` | the ordinary service view, per op |
  | `ordo_denials_total{principal_kind,op}` | the substrate for denial-volume alerting; every oracle in the security review is a high-call-count pattern |
  | `ordo_key_release_total{purpose}` + distinct spaces per principal | the substrate for key-release breadth alerting |
  | `ordo_job_queue_depth{kind}`, `ordo_job_dead_letters_total`, `ordo_backfill_rows_remaining{migration}` | worker health and migration progress |
  | `ordo_live_sessions`, `ordo_invalidation_lag_seconds` | liveness of the live path |
  | `ordo_registry_swaps_total`, `ordo_shape_verification_failures_total` | deploy health |
  | `ordo_chain_verify_failures_total`, `ordo_checkpoint_age_seconds` | audit-spine health |

- **Mandatory alerts.** A deployment that silences these is misconfigured and
  the dashboard says so: denial volume per principal per space above baseline;
  key-release breadth above a principal's declared plan; chain verification
  failure or checkpoint age past policy; dead-letter growth; shape-verification
  failure on swap.
- **Health endpoints.** `/health/live` (process), `/health/ready` (registry
  loaded, shape verification passed, database and KeyProvider reachable),
  `/health/deep` (per-dependency detail, **access-controlled** since it names
  the deployment's KeyProvider and database). All three are envelope-tier; no
  endpoint serves space data.
- Renders in `dashboard-plan.md` §3 (routine/trigger monitor, chain health) and
  answers its §7 open item on where platform-health metrics live.

## Key custody & DEK rotation [settled]

Keys are **two-level**. Each space has one **space master key (SMK)**, held by
the `KeyProvider` and never leaving it; the SMK **wraps a series of per-epoch
data-encryption keys** (`DEK_1`, `DEK_2`, …), and every sealed value is sealed
under exactly one of them. Without this, a DEK compromised in year three
decrypts year-one backups and every archived log bundle, forever, and there is
no post-compromise recovery state to reach.

- **Epoch tagging, from the first write.** Every sealed row payload, sealed
  blob chunk header, sealed command image, and archived log bundle carries an
  **epoch tag** naming the DEK that sealed it. The tag is envelope data, not
  secret. Retrofitting a tag onto unversioned ciphertext is a migration nobody
  enjoys, so **the tag ships before first customer data** even if rotation
  itself lands later. (One byte in the wire format; a deployment that exhausts
  it has a re-tag migration, not a design change.)
- **Blind-index keys derive from a dedicated index key that rotates only with
  the SMK** [settled; this replaces "blind-index keys derive per epoch"]. The
  derivation is `BIK = KDF(SMK, "blind-index", entity, field)`: domain-separated
  per entity and field, and **independent of the DEK epoch**. A DEK rotation
  therefore does not touch any blind index. There is no re-salting, no re-index and no
  cross-epoch fan-out.
  - **Why the per-epoch derivation had to go.** During a lazy re-index the same
    plaintext hashes two ways at once, so a `sealed indexed unique` field
    **admits duplicates** across the epoch boundary and converge-or-create
    **creates instead of converging**, because the unique key it addresses by
    does not match the old-epoch row. The concrete failure is mundane and
    likely: a webhook redelivery or a source retry during a rotation writes a
    second record instead of converging onto the first. And since rotation is
    **mandatory on every membership revocation**, this fires in ordinary
    operation, in whichever space just lost a member. Uniqueness is a correctness
    claim and cannot be suspended for the duration of a background job.
  - **The consequence.** Blind indexes no longer get forward
    secrecy from DEK rotation. An adversary who obtained `BIK` keeps a working
    equality oracle over that entity/field until the **SMK** rotates, which is the
    narrower and rarer event. That is the trade: rotation stops bounding index
    linkability, and in exchange logical uniqueness holds continuously,
    including across every revocation. Values keep their forward secrecy
    regardless, because they are sealed under the DEK, not the index key.
  - **Rotation must be *possible*; its cadence is the deployment's call
    [settled].** A key that *cannot* rotate is compromised forever the moment
    it leaks, and the BIK inherits its lifetime from the SMK, so every
    KeyProvider implementation must support rotation, and a provider that
    cannot is not a provider. **How often** is a deployment owner's decision and
    the platform sets no mandate: `policy keys { rotate <cadence> }` sets the deployment
    default, a space may request a different cadence (tighter or looser) and
    the deployment may grant or refuse it, and rotation always happens on
    demand, since suspected compromise does not wait for a schedule. A deployment
    that has never rotated is *surfaced*: the dashboard says so
    and the consent page reflects it. The disclosure is the product.
  - **A built-in provider exists so small deployments are not gated on a KMS
    [settled].** Requiring GCP KMS or Vault to run a hobby app would make
    self-hosting second-class, which this platform exists to avoid. The
    `LocalKeyProvider` (a master key from the environment or a file, wrapping
    per-space SMKs in Postgres, with the same rotation interface as any other
    provider) is a first-class implementation. It cannot protect keys from
    someone holding the machine, and the consent page and the shape summary say
    so. Moving to a cloud KMS or Vault later is a provider swap plus a re-wrap
    and needs no migration. "EZ mode" means fewer prerequisites and the same
    disclosure. (The old DEK epochs it wrapped are retained per the undo-horizon
    rule above; rotating the SMK re-wraps them rather than destroying them,
    which is what keeps history readable across a rotation.)
  - **SMK rotation is therefore the index's rotation too**, and it is a real
    re-index: build the new index column **offline and swap it atomically**.
    Rows are not updated in place. **"Offline" is a write barrier** (C4-015): writes to the blind-indexed fields of that entity are
    quiesced from scan start through verified swap, which is the same rule and the
    same reasoning as a transforming backfill's freeze (Migration & failure
    windows above). Without it, a value written to a range the scan has already
    passed lands only in the old digest column, and after the swap the new
    unique constraint does not see it, so a retry inserts a logical duplicate.
    "Old and new are never both queryable" is a *read* invariant and does not
    exclude that execution. The same dual-write-from-a-watermark upgrade applies
    here when the freeze becomes intolerable, and is deferred for the same
    reason. An in-place re-index shows an observer
    with continuous database access the old digest and the new digest for the
    same row. That is a linkage pair per row, which hands the observer exactly the map
    the re-salt was meant to destroy. If an online build is ever unavoidable,
    randomize row order and batch, and state in the shape summary that the
    re-salt is **best-effort against an observer with continuous DB access**.
  - **The rejected alternative** is to keep per-epoch
    derivation and **block writes to blind-indexed unique fields until the
    re-index completes**. It is correct, and it means every membership
    revocation freezes writes to exactly the fields most likely to be an
    entity's identity (email, external id) for the duration of a background
    job. That is an availability outage triggered by an HR event.
  - **Disclosure: the null sentinel is readable** [settled]. A
    `sealed indexed unique` field HMACs a reserved null sentinel for null
    values (`definition-language.md`, uniqueness), which makes the sentinel the
    single most common digest in that column, so **whoever holds the database
    can tell which rows have a null in a sealed field**, without any key. The
    blind index conceals *values* and does not conceal *nullness* or
    cardinality. This belongs on the shape summary next to the
    field, because "is this field set" is sensitive for some fields
    (a diagnosis, a flag, a termination date) and an author choosing
    `sealed indexed unique` should be choosing it knowingly.
- **Rotation is an op, and never a background surprise.** It mints `DEK_{n+1}`
  under the SMK, marks it the write epoch, and lands as an ordinary attributed
  **command group in the space's own chain** recording who rotated, when, which epoch
  opened, and what triggered it. Triggers: **membership revocation** (mandatory;
  see client-first below, where a removed device holds a wrapped key),
  **suspected compromise** (owner-initiated, step-up gated), and **policy
  schedule** (`policy keys { rotate 1y }`).
- **Re-sealing is lazy, resumable, and not new machinery.** Old rows and blobs
  re-seal through the **same chunked, checkpointed conversion job** space
  custody conversion uses (next section): progress per chunk, verify before
  delete, interruption resumes, throttled so a rotation cannot starve the
  deployment. Reads accept any live epoch; writes always use the newest. An
  epoch is **retired** only when the job reports zero remaining references.
- **Crypto-erasure interaction.** Destroying the **SMK**
  orphans **every epoch at once**. That is the erasure primitive, and it is
  what `policy compliance { erasure crypto }` destroys. Destroying a single
  **retired epoch key** is the narrower operation, and it kills exactly what is
  still tagged with that epoch: after a completed re-seal pass that is nothing
  live, and *is* the historical material that was never re-sealed, namely archived
  command-log bundles and backups from that epoch stop opening. So the
  rotation confirmation states the consequence plainly: **rotate then retire =
  post-compromise recovery, at the cost of old archives; rotate and keep the
  old key = forward secrecy for new writes only.**
- **Destruction of an epoch key waits for the undo horizon, and retiring an
  epoch does not destroy its key** [settled]. Command-log before/after images name sealed payloads
  by `SealedRef`, and the chain hashes the ref, so a re-seal pass **cannot**
  rewrite historical images without breaking the chain from that position
  forward. Destroying a retired epoch key therefore orphans every image sealed
  under it, which would take `std.E.history` values and `revert_group`'s
  converge-with-before-image out **inside the window the platform guarantees
  undo for**, in whichever space just revoked a member. The rule: **an epoch key
  may not be destroyed while any command image within the space's `undoHorizon`
  is tagged with it.** The full trade, the beyond-horizon
  behaviour, and why the alternative (truncating undo at each rotation) was
  rejected are in `security-logging-design.md` §7. Crypto-erasure is unaffected. It
  destroys the **SMK**, orphaning every epoch at once with no per-epoch
  cooperation.
- **Re-seal workers get scoped key release** [proposed]. Re-sealing needs both
  `DEK_n` and `DEK_{n+1}`, so a rotation is the largest key set a space's keys
  ever have resident at once, held by the longest-running batch process. Because
  revocation mandates rotation, an attacker who can cause or predict a
  departure can schedule for that window. Release to the re-seal job is
  therefore epoch-scoped, purpose-scoped, and correlation-bound
  (`release(space, epoch, purpose=reseal, correlation)`), refused without a live
  correlation to a running rotation job, and rate- and breadth-capped per
  worker.
- **What rotation does not fix.** Ciphertext already exfiltrated under `DEK_n`
  stays readable to whoever holds `DEK_n`. Rotation bounds the future; it
  bounds the past only to the extent the old epoch key is destroyed and no copy
  ever left the deployment.

## Space custody conversion [lean]

A space can move along the sovereignty spectrum **in place**, including to full
end-to-end (client-held keys), and back. Planning:

- **Mechanism**: a tracked, resumable migration job (the standard queue), not
  export/reimport. Chunked streaming re-encryption: rows and blobs are unsealed
  under the current DEK and resealed under the target custody (client-wrapped
  DEK for E2E); progress checkpointed per chunk, verifiable by count + content
  hash per entity. Interruption resumes; nothing is deleted until the target
  copy verifies. Blobs re-seal via the same chunked STREAM format.
- **Limited capacity, stated up front**: the capability loss is *computed*.
  The compiler already knows every op/routine manifest, so the
  conversion consent page enumerates exactly which operations survive (sealed
  fields drop to blind-index equality, host-compute ops move client-side,
  server routines shrink to what their manifests can still see). Nothing is
  silently disabled; the list is derived the same way scope manifests are.
- **Limited transfer**: conversion runs under throttles/quotas (rate-capped
  re-encryption, off-peak scheduling for large spaces) so a conversion can't
  starve the deployment; BYO targets (DB/bucket) apply the same resolver paths
  as day-one placement.
- **Auditability**: the conversion is itself a logged, attributed command
  group; direction, target custody, and the derived capability diff are in the
  log. Reversal is the same mechanism run in reverse (re-consent is required
  because scope expands).

### What the dial does not cover: envelope disclosure [settled]

The dial moves **values**. It does not move the **envelope**, and every
position, including the far end, leaks the same envelope. This is stated here
because the custody page is where a reader forms their mental model, and "E2E
means the server knows nothing" is wrong in a way that matters:

- **Plaintext at every position**: entity and field *names* (the schema is
  compiled server-side and must be), row ids, the **actor** string, command
  kind (create/converge/delete), timestamps, command-group correlation, chain
  position, and per-space volume. `security-logging-design.md` §7 seals sealed
  *values* behind `SealedRef`; it does not and cannot seal the skeleton.
- **Therefore the command log is a high-resolution behavioral record even for
  a client-first space.** Who acted, on which entity, how often, at what hour,
  in what bursts: all readable by the platform operator, at every dial
  position. For many spaces that record is more sensitive than the values it
  describes.
- **Say it where it is decided.** The custody consent page states this
  alongside the computed capability list, in the same derived form:
  the conversion page shows what stops being readable **and** what remains
  readable regardless. Telemetry follows the same boundary (see Observability)
  and inherits the same disclosure.
- **What would narrow it** [proposed, not v1]: per-space pseudonymous actor
  handles resolvable only with the space key, and coarsened timestamps in
  archived bundles. Both trade auditability for privacy and neither is free.
  They are a dial position that does not exist yet, and pretending the current
  one is that position is the failure mode this note exists to prevent.

## Client-first mode (full E2E) [settled]

The far end of the sovereignty dial, offered **as a visible per-space option**
within the same product. The client's local store is the authoritative working
copy (plaintext lives only on the user's devices), and the server holds a
store it cannot read.

### What blindness costs, stated once [settled]

A space owner who moves the dial here is **buying server blindness**, and
blindness has one price: **the server stops being a referee.** Enforcement
(guards, `check` invariants, CAS,
insert-completeness, uniqueness, referential integrity, and the truthfulness of
before-images) requires reading plaintext values. A server holding
ciphertext can verify who signed a segment, in what order it arrived, and that
the chain links; it cannot verify that what arrived was a *legal state
transition*. Encryption protects confidentiality, not the integrity of meaning:
a tamper-evident log of lies is still a log of lies.

Three consequences follow, and they are the terms of the trade:

1. **Members are mutually trusting, cryptographically.** Every enrolled device
   holds the space key, so a member can decrypt rows their guard view would
   hide. At this custody position **guards are a UI and convention boundary
   and are not a cryptographic one**. A shared password vault makes the same bargain.
   Spaces needing members who cannot read each
   other's rows do not belong at this stop on the dial.
2. **Violations are detected and revertible, not prevented.** Devices
   re-validate incoming commands against the same compiled artifacts and flag
   what does not hold; the chain still attributes every write and the images
   still support group revert. There is no CRDT convergence here:
   no automatic reconciliation of conflicting edits and no growing merge
   metadata, because detection needs one evaluation and convergence needs a
   protocol. The guarantee degrades from "impossible" to "attributed, caught,
   and undoable," which is the same posture the managed side takes when an
   application bug slips past its checks.
3. **No cryptography closes this.** The hostile
   party here is *legitimately authenticated*: a member the space admitted.
   Proof systems prove statements about data; they cannot prove which human is
   at a keyboard, and possession-plus-presence (a passkey touch) is the ceiling
   of what any scheme delivers. This is authorization of a party you already
   trusted, which is a governance problem.

**The consent page at this stop says all three**, in these words or plainer:
members can read everything in the space; the server can no longer enforce the
rules the schema declares; bad writes are caught and reversible, and are not
blocked. An owner who accepts that has made an informed trade. An owner who was
allowed to believe otherwise has been mis-sold, which is the only actual defect
in this area.

- **What the server keeps doing**: everything structural. `ordo apply` still
  compiles the domain (schema, ops, migrations, version hashes), and the
  space still gets auth, membership, the command-log envelope, and durable
  storage. Data rows and command images are **sealed client-side under
  client-held keys** before they reach the server; the chain hashes
  ciphertext; blobs are `private`-custody by default.
- **Where queries run**: on the client, against the local store, using the
  **same compiled artifacts** (the compiler is WASM-capable by design, and the
  embedded-mode engine from `tooling-and-embedding-plan.md` §B4 is the
  execution vehicle). Registered ops, manifests, and mode labels stay the
  contract; only the execution venue moves.
- **Sync is an encrypted database sync**: the platform defines the vault
  protocol: sealed oplog segments + periodic sealed snapshots (cadence,
  cold-start rule and discard floor in "Snapshots and compaction" below),
  resumable, integrity-chained, multi-device via key wrapping. It guarantees delivery,
  ordering per space, and storage. **How and when to send is a client
  policy** (batching, radio/power windows, conflict presentation): the
  platform ships a default sync policy, and sophisticated scheduling belongs
  to the client. The **merge rule** that makes "ordering per
  space" true under offline multi-device writes is the subsection below. It
  was the missing half of this `[settled]` mark.
- **Limitations this creates**: no server-side routines over
  values, no server-side query execution, no platform vision/LLM services,
  no blind-index lookups unless the client computes and ships the index
  values. The consent page derives the surviving-capability list exactly as
  custody conversion does, computed from manifests.
- **Entry and exit are the conversion mechanism** (section above): a space
  converts to client-first in place (tracked, resumable, re-encrypted) and
  back out with re-consent, since leaving expands what the server can see.

### The merge rule: server arrival order is the order [settled]

Two devices, both offline, both extend the same head. The server cannot read
the segments, so it cannot merge by content, and a chain that admits forks is
not a chain. The rule has five parts:

1. **Server arrival order is the canonical order.** The per-space chain is
   **server-sequenced**: the server assigns each accepted segment its position,
   hashing `prev ‖ segment-header` over ciphertext it never opens. There is one
   chain per space, no forks, and `verify(space)` verifies exactly it.
   **Two immutable signed records, never one** [settled; C4-001, and the earlier
   single-header design was self-contradictory and is withdrawn]. It described
   a device signature covering the believed `prev` *and* said re-anchoring
   changes `prev` while the signature covers only content fields. Both cannot
   hold: if the signature covers `prev`, it is invalid at the canonical
   position; if it does not, re-signing is pointless and the signature cannot
   say what head the device believed it was extending. The bytes whose
   authenticity defines the protocol were ambiguous. So:

   - **The proposal `P`, signed by the device, immutable forever:**

     ```
     P = ( payload_digest    = SHA-256(sealed payload bytes),
           device_id, counter,
           believed_prev,
           artifact_hash, schema_hash )
     ```

   - **The sequencing receipt `R`, signed by the server, immutable forever:**

     ```
     R = ( hash(P), canonical_position, canonical_prev )
     ```

   The chain commits to **both**. `verify(space)` checks each link's `R`
   against its `P` and its predecessor. A device signs `P` exactly once, at
   authorship, and **never re-signs anything**, so a device that crashes after
   upload and before acknowledgment cannot leave a sequenced link with no valid
   device signature, which the old design permitted.

   Three properties follow. The chain covers content: a server that swapped,
   truncated, or re-paired a payload is detectable by any device holding the
   payload, though the server never reads one. Authorship is attributable and
   *positional intent is preserved*: `believed_prev` stays in `P` forever, so
   "what this device thought it was extending" and "where it actually landed"
   are both permanently on the record, and their disagreement is exactly the
   offline-concurrency signal. And the server cannot re-point content: `R`
   binds one proposal to one position, so a server that attaches `P` to a
   different position produces a receipt that does not verify.

   **`artifact_hash` and `schema_hash` are load-bearing** (C4-003). Canonical byte order does not imply identical state unless the
   *transition function* is part of the ordered input. Without the pin: a
   device goes offline under artifact A1, the deployment installs A2 changing a
   default or a check, the blind server sequences the proposal under A2 with no
   rule permitting otherwise, and two compliant devices fold the same canonical
   bytes to different states. So a receiver **fetches and verifies the named
   retained artifact before folding**, and folds under that one, using the same
   retained-artifact machinery hot deployment already keeps server-side. A
   proposal naming an artifact the deployment no longer retains is refused at
   sequencing (a transport fact, so a blind server can enforce it); one already
   sequenced replays under its named artifact forever. Artifact retirement is
   therefore a **deprecation window announced to devices** and is never a silent
   swap. An offline device must be able to learn that its artifact is expiring
   before its proposals start bouncing.
2. **Device oplog segments are PROPOSALS and are not chain links.** What a device
   produces offline is a sealed, self-consistent, locally-ordered *proposal*
   with a header naming the head it believed it was extending. A proposal is
   not history until the server sequences it. This is what makes fork-free
   ordering possible without the server reading anything. **Sequencing is
   content-blind**: the server accepts or refuses a proposal on transport facts
   only (authentication, quota, well-formed header) and never on what it
   contains, because it cannot read one. Acceptance is not endorsement, and it
   decides nothing about whether the commands inside will apply.
3. **Devices adopt the canonical order by receipt; nothing is re-signed and
   nothing is rewritten.** The acknowledgment *is* the server's receipt `R`:
   assigned position, canonical `prev`, and a signature over `hash(P)`. The
   device verifies `R` against the `P` it holds and adopts the canonical chain.
   `P` does not change (not its digest, not its `believed_prev`, not its
   signature), so "re-anchoring" is no longer an operation performed on signed
   bytes at all. It is the device learning where its immutable proposal landed.
   **Payloads are never re-sealed, never re-encrypted, and never rewritten**,
   so adoption is cheap, offline-safe, and touches no ciphertext the device may
   no longer hold keys for. (The earlier design had the device re-sign a header
   after `prev` changed. That is what C4-001 killed: it made the signed bytes
   mutable, and a crash in the window left content sequenced with no valid
   signature over it.)
4. **State is a deterministic fold over the canonical order, and every device
   computes the same one** [settled; this is the part that was contradictory].
   Two proposals touched the same row. The rule that resolves it must be a
   *function of the canonical sequence*, or devices diverge:
   - **Every key-holding device applies segments in canonical order, using the
     same deterministic rule.** A command whose `rev` precondition does not
     match the state produced by the preceding prefix is **rejected on every
     device, identically**, because every member device holds the keys and can
     evaluate it. Convergence does not depend on which device wrote the
     command, on any user's answer, or on any server judgement. The server never
     evaluates CAS: it cannot read the values, and it does not need to.
   - **Every invariant behaves that way, not just `rev`** [settled; C4-002].
     The earlier text defined a deterministic outcome for `rev` mismatch alone
     and left every other failure class (guards, `check`s, uniqueness,
     referential integrity, bounds) undefined, which meant an invalid command
     had no stated state effect and the promise "caught and undoable" rested on
     an honest device happening to be online. So: **a command that fails any
     invariant under the canonical prefix is a canonical no-op**, evaluated
     identically by every key-holding device under the artifact `P` names, with
     a recorded outcome (`cas_miss`, `access_guard`, `check_failed`,
     `duplicate`, …) that is a function of the sequence and nothing else. There
     is nothing to catch later because nothing was applied.
   - **Inverse images are derived from canonical state, never from the
     proposal** [settled; C4-002]. A member can sign a perfectly valid
     proposal carrying a *false* before-image. If revert trusted recorded
     images, reverting it would restore the attacker's fiction rather than the
     real predecessor. So the before-image used by `revert_group` in a
     client-first space is **recomputed by the reverting device from the
     canonical prefix immediately preceding the group**, and the
     proposal-supplied image is treated as a claim to be checked against it,
     not as data to be trusted. A disagreement between the two is itself a
     finding: it is signed evidence that a specific device supplied a false
     image, which is precisely the attribution this custody stop promises.
   - **What the promise is.** At this
     custody stop the claim is: *an invariant violation never takes
     effect on any honest device, is attributable to the device that signed it,
     and correcting the record requires a new proposal.* The
     server does not block it: the server is blind and blocking is what you traded
     away. Detection of a *member acting in bad faith within their rights* (the
     balance they were allowed to set, set maliciously) remains a
     governance problem, and no evaluator resolves it.
   - **A rejected command is still history.** It was sequenced, it is hashed
     into the chain, and its recorded outcome is `cas_miss`, deterministic and
     identical for every device. The earlier phrasing ("a rejected command never
     becomes history") contradicted part 1, which had already sequenced it, and
     would have made the chain's contents depend on a later, device-local
     decision. What is *not* history is a state change: a rejected command
     changes nothing.
   - **The user-visible conflict is a notification, and the fix is a new
     command.** The writing device is the one that surfaces `cas_miss` to its
     user, identically to how an online conflict surfaces today, but whatever
     the user does next enters as an **ordinary new proposal**, sealed, signed,
     and sequenced like any other. There is no re-interpretation of the old
     segment, no rewrite, and no re-anchoring of anything but headers (part 3).
   - There is no CRDT and no silent merge. A device that has received the chain
     up to position N **and folded it under the artifact each proposal names**
     holds exactly the state every other such device holds at position N;
     devices behind the head are *behind*, never divergent. The artifact clause
     is the condition that makes the sentence true (C4-003).
5. **Membership revocation triggers DEK rotation, as a logged command group.**
   A removed member's device holds a wrapped key, and no protocol message takes
   that back. Revocation therefore rotates (see "Key custody & DEK rotation"):
   the space key is rewrapped for remaining devices, the removed device stops
   being able to open **new** epochs, and the segments it already holds are
   gone. Rotation is one command group in
   the space's chain, so "who was removed and when the key turned over" is one
   query.

### Snapshots and compaction: the fold must be bounded [settled]

Everything above says state is a **deterministic fold over the canonical
order**. Taken alone that is the append-only ledger problem in its purest form:
a device that has been in a space for two years cold-starts by replaying two
years of segments, and the cost of opening the app grows without bound while
the data in it does not. "Periodic sealed snapshots" appeared once in the sync
description with no cadence, no rule that a device may load one instead of
folding, and no permission to discard anything. That is the name of a compaction
design and not the design (V-008). This section is the design.

**A snapshot is a sealed, chained, ordinary participant in the protocol.** A
device with keys serializes its folded state at a canonical position, seals it
client-side, and uploads it as a snapshot record:

```
S = ( state_digest = SHA-256(sealed snapshot bytes),
      at_position, chain_head_at_position,
      artifact_hash, schema_hash,
      device_id )                                   # device-signed
```

The server sequences `S` exactly as it sequences a proposal (content-blind,
transport facts only) and issues a receipt. So a snapshot is verifiable
against the chain: any device can check that `S` claims a
position whose head matches the chain it already has, and that the device which
produced it was a member at that position.

**Cadence is policy, with a default.**

```
policy vault {
  snapshot  every 5000 commands or 7d   # whichever comes first
  keep      3                           # snapshots retained before pruning
}
```

**Cold start loads the newest snapshot it can open, then folds forward.** This
is the rule the earlier text was missing and without which snapshots
accomplished nothing: a device is **permitted, and by default expected**, to
initialize from a verified snapshot rather than from position zero, then fold
only the segments after `at_position`. Startup cost becomes a function of the
snapshot interval, not of the space's age. A device that cannot open a snapshot
(a key epoch it lacks) falls back to the newest one it can, and only in the
limit to a full fold.

**Discarding is allowed, and bounded by exactly one rule.** Segments below the
newest snapshot that **every enrolled device has acknowledged** may be
discarded by the server. That floor is the client-first case of the ordinary
pruning floor, and the eviction rule from `security-logging-design.md` §7
applies with the sign flipped: a client-first device's cursor is
**non-evictable**, because the device is the authoritative store and pruning
past it destroys data and not a cache. So in a client-first space, an
abandoned enrolled device *does* pin history. The remedy is to
**un-enroll it**, which is a membership operation, attributable, logged, and
already triggers DEK rotation (part 5 above). The dashboard's log-weight view
names the device that is holding the floor.

**A snapshot is not a checkpoint and does not replace history.** Discarding
segments discards the ability to *revert* through them and to audit them; the
chain of receipts remains, so the space can still prove the sequence existed
and was not rewritten. A space that wants the full record keeps `archive` on,
in which case the archive sink is a non-evictable consumer and the floor never
passes it. This is the same rule as everywhere else in the platform.

**Owed to the build plan:** the vault protocol has no deliverable in
`engine-plan.md`'s track list. Snapshot production, verification, cold-start
selection, and the enrolled-device floor are F-track work and are named there.

**Extension point: a reconciliation layer [proposed, not in v1].**
The rule above resolves conflicts by *rejection and replay*, which needs no new
semantics. It leaves room for an
optional **RESOLUTION / RECONCILIATION layer** above it (diff-style merges, or
per-field reconciliation) that would turn some `cas_miss` outcomes
into automatic merges instead of user-visible conflicts. The seam is defined:
such a layer consumes rejected commands plus the canonical state, and emits new
commands through the ordinary write path, so it can never bypass guards,
manifests, or the chain. v1 ships without it because typed
unique ids make most real conflicts benign: two devices editing different rows
never collide, and same-row collisions are exactly the case a user wants to see.
If the evidence later says otherwise, the layer is additive.

**The trade this makes.** Server-sequencing
collides with the proposal (in the security review) that the client
**counter-signs the group hash** at the top of the dial, so that the platform
cannot author history in an E2E space at all.

- **Kept:** a single, fork-free, verifiable, server-sequenced order per space,
  which is what makes `verify(space)`, checkpoints, revert-by-group, log
  positions, and pruning floors mean anything.
- **Lost:** device-side proof of *ordering*. The client signs its proposal's
  **content** (via the header's `payload_digest`, part 1) and cannot sign the
  canonical link, because it does not choose the link. So even at the top of the
  dial, **ordering is platform-authored**. The platform cannot read, forge, or
  substitute content, but it can in principle drop, delay, or reorder proposals.
  The digest narrows the loss to *ordering alone*: without it the
  platform could also have re-paired headers with payloads.
- **The mitigation** [settled; the full design and its cadence now
  live in `security-logging-design.md`, "Independent verification:
  attestation"; this bullet is the summary, not the source]:
  (a) **asymmetric checkpoint signatures** (Ed25519 behind the existing
  `Signer` trait) so verification needs only a public key; (b) **exportable
  chain proofs**: a `chainProof(from, to)` op returning head hashes, signed
  checkpoints, and per-group hashes, verifiable offline by a published
  verifier; (c) **owner-verifiable checkpoints**: the owner's client pins head
  hashes somewhere the platform does not control, so a later reorder or
  omission is *detectable by the owner* even though it was not *preventable*.
  The property offered is detection and the custody
  page should say that word.

### Device keys, enrollment, loss & revocation [proposed]

"Multi-device via key wrapping" is one sentence carrying a whole subsystem.
Filling it in:

- **Device enrollment is an op**: consented, step-up gated, command-logged.
  The space's own chain therefore shows **every device that ever held keys**,
  when it was enrolled, and by whom. There is no ambient device.
- **Removing a device rewraps the space key and rolls the epoch forward** (the
  rotation mechanism above). The removed device's wrapped copy stops opening
  new epochs. **Segments and snapshots it already holds are already open to it.
  That data is gone**, and no protocol recovers it. The removal confirmation
  says so and does not imply a remote wipe.
- **Key loss is a real loss, and recovery is an explicit choice on the dial.**
  Two options, presented at conversion time:
  - **User-held recovery phrase (default).** The platform holds nothing, and
    losing both the last device and the phrase loses the space. This is the
    cost of the top dial position and it belongs on the consent page in
    those words.
  - **Platform escrow (opt-in).** This is a visible custody step **down**, disclosed
    as one, with the capability diff computed the same way every other
    conversion computes it.
  **No silent escrow, ever.** A recovery path the user did not choose is a
  custody position they did not consent to.

### The local store is a database: client storage rules [proposed]

The on-device mirror is load-bearing in three places (search runs over it,
guests hold one, and in a client-first space it *is* the authoritative store),
and it currently has no storage pins at all. Unencrypted, unkeyed, unscoped and
unaged, it is the softest surface in the system: a stolen or imaged laptop
yields a whole-space export with no token, no log entry, and no possible
revocation, including rows the server has never been able to read.

- **Sealed at rest under a device key**, wrapped by a passkey-PRF-derived key,
  the same mechanism `blob private` custody already uses. Unlocking is a
  WebAuthn ceremony.
- **Auto-lock.** The unwrapped key lives in memory only: idle timeout
  (policy-tunable, default 15m), eviction on tab close, re-ceremony on wake.
  **Locked = ciphertext on disk**, and the app renders sealed markers exactly
  as `ui-gen.md` §8 already specifies. The locked state is a shape the UI
  layer already knows how to draw.
- **Per-space mirrors, separately keyed.** No cross-space store, ever. A device
  enrolled in six spaces holds six independently unlockable stores.
- **Mirror scope is a disclosed policy:**
  `policy mirror { keep 90d | keep <n> rows | full }`, surfaced on the space's
  consent page. A mirror is a **copy**, and copies are consent-relevant.
- **Never mirrored:** `std.space.*` results and command-log slices. A stolen
  device must not yield the permission map or the audit spine.
- **Revocation reaches the mirror, best-effort against an offline attacker and
  mandatory against an online one.** Revocation marks the space **poisoned at
  next contact**; the client purges before serving any read. **A device that
  never reconnects keeps its cached plaintext**, and
  no server-side action changes that. Revocation bounds future access and not past
  copies, as the DEK-rotation section says about
  exfiltrated ciphertext, and the reason auto-lock and mirror scope (which
  bound *how much* is cached) are the controls that actually pay here.

This is the local-first shape (the cache *is* the store) delivered as a dial
setting: same schema, same ops, same audit spine, different custody.

## Identity [settled; resolved in `security-logging-design.md`]

- **In-house core, every authentication method behind a provider trait.** The
  earlier open question (in-house on `webauthn-rs` vs an off-the-shelf IdP
  sidecar) is answered: invite-gating, capability tokens, and the directory are
  custom regardless, and a sidecar dilutes the one-binary story, so the
  directory, sessions, capability tokens, and step-up are in-house, while
  Keycloak, Ory, Google, Discord, and enterprise SAML are *configurations* of an
  `AuthProvider` implementation. Full ruling, trait family, and token model:
  `security-logging-design.md` §§1–5.

## Client delivery [settled]

- Call-time envelope modes: `live` (invalidation subscriptions), page tokens
  (keyset from the op's sort), `heavy` (streamed row batches + SDK auto-paging
  under scroll + chunk cache: memory LRU, IndexedDB opt-in).
- **Snapshot consistency, log-positioned reads [settled]**: every read
  result carries the per-space command-log position it was computed at.
  Multi-query screens and the IndexedDB mirror hold their reads to **one
  position**: after an invalidation, affected queries refetch and the client
  advances the visible snapshot only when every refetch has reached the new
  position, so two widgets can never show pre- and post-write states of the
  same command group simultaneously. The mirror stores its high-water
  position and applies feed batches atomically per group. (Positions are
  envelope data, so this works even where the server cannot read
  values.)
- Blobs never ride query payloads: descriptor objects with session-scoped
  immutable URLs; browser HTTP cache does the heavy lifting.
- **The mirror is storage, and storage has rules.** The IndexedDB mirror named
  here (chunk cache, `heavy` mode, log-positioned snapshots) is governed by
  "The local store is a database" under client-first above (sealed at rest
  under a device key, auto-lock, per-space, scope-disclosed, purged on
  revocation). Those rules apply to *every* mirror, not only client-first
  ones: a server-custody space's cached rows are plaintext on a laptop just
  the same.
