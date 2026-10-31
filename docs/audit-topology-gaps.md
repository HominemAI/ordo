# Audit: the settled topology against what buyers ask for

**Date:** 2026-09-12 · **Specification:** [`plan-topology-audit.md`](plan-topology-audit.md) ·
**Method:** the buyer's question first, then the mechanism in this tree, then a
verdict on the axis §0 defines. No cargo was run; every claim is a read of the
source at a cited line, plus a caller search for anything that looked built.

The topology under audit is the one this session settled: spaces as the tenancy
boundary, per-space routing onto a sparse set of database targets, a control
plane beside the tenant databases, a system space for platform-owned content,
sealed fields with per-space keys, guards compiled into every query, a per-space
hash-chained command log, and windows and feeds as the only cross-space paths.

Sixty-two capabilities, grouped by the plan's eight dimensions. Eight are
structural, two are refused with the refusal cited, nine are built and
unreachable, and the rest are unbuilt. Section 3 is the one to read first if you
read only one: it records where §4's predictions were wrong, and it corrects one
claim in the brief that set this audit going.

---

## 0 · How to read this, and the one fact that frames every row

**The verdict axis.** **Structural** means the topology forbids it and changing
the answer costs a redesign. **Unbuilt** means the topology permits it and
nobody wrote it, which costs a sprint. **Refused** means it was considered and
declined, and the entry cites where the reason is recorded, so the cost is a
conversation about whether the reason still holds.

**Evidence, not opinion.** Every verdict carries a `file:line` from this tree.
Where a thing exists and is not reachable, the entry says so, because "it
exists" and "it is wired" are different answers and this audit found eleven more
cases of the first without the second.

**The frame.** `ordo-host` declares no `[[bin]]` and has no `main.rs`. The two
router constructors, `ordo_api::router` (`crates/ordo-api/src/router.rs:118`)
and `ordo_host::serve::platform_router` (`crates/ordo-host/src/serve.rs:151`),
are merged in exactly one place outside tests, and that place is a doc comment
showing how a deployment would do it (`crates/ordo-host/src/serve.rs:27`). The
workspace builds `ordo` (`crates/ordo-cli/Cargo.toml:7-9`) and
`ordo-verify-proof` and nothing that listens on a socket. This is tracked as
checklist 17a and `plan-ci-images.md:361`, so it is a known state rather than a
discovery.

It matters here because it changes the meaning of half the rows below. Where an
entry says **wired**, it means real non-test code calls real non-test code
across a crate boundary. It never means a running deployment reaches it, because
no running deployment exists in this tree. Where an entry says **no production
caller**, the gap is one level worse: the code is unreachable even from the
assembly the tests perform.

**Two facts about the surface, because they recur.** The whole HTTP surface is
nine routes: `/op/{name}`, two blob routes
(`crates/ordo-api/src/router.rs:120`, `:147-148`), three health routes, `/metrics`,
`/security/alerts`, and the catalog and interface pairs
(`crates/ordo-host/src/serve.rs:153-164`). There is **no authentication route at
all**: the door verifies bearer tokens (`crates/ordo-host/src/lib.rs:1164`,
`:1226`) and `ordo_identity::token::mint` (`crates/ordo-identity/src/token.rs:91`)
has no caller outside tests, so nothing in the tree issues the credential the
door checks.

And `deployment_engine`, the one function shaped like production bootstrap,
deploys and role-gates every platform op spec (`crates/ordo-host/src/lib.rs:1471-1505`)
without attaching five of their handlers: `with_exit`, `with_attest` and
`with_buckets` are optional builders (`crates/ordo-host/src/platform_ops.rs:736`,
`:742`, `:759`) that only tests call. The refusal when they are missing is
honest and typed: "this deployment registered an exit op and wired no state for
it" (`:772-780`). So export, delete, attestTo, chainProof and attachBucket are
authorized, visible in the catalog, and answer an internal error.

**What a customer would be told**, the plan's step 6, is folded into the cost
column rather than given its own, because in almost every row the workaround is
the same two sentences and repeating them fifty times would bury the rows where
it differs.

---

## 1 · The table

### Dimension 1 · Tenancy operations
*"Can you restore one customer to last Tuesday without touching the others?"*
Benchmark: **Nile, WorkOS and Clerk** for what B2B buyers have been taught to
expect; **Neon and PlanetScale** for the per-tenant economics.

| # | Capability, as the buyer asks it | Verdict | Evidence | Cost |
|---|---|---|---|---|
| 1.1 | "Create my second customer" | **unbuilt** | `deployment_engine` hardcodes `SpaceId(SPACE)` with `SPACE = "conformance"` (`crates/ordo-host/src/lib.rs:88`, `:1317`). `create_space` exists only on the key provider and every caller is `TestProvider` (`crates/ordo-crypto/src/custody.rs:668`, `:922-933`). `plan-space-routing.md` §0.6 states it. | More than a day, and it gates seven rows below. `plan-space-routing.md` S5 is the sequence. |
| 1.2 | "Restore one customer to last Tuesday" | **unbuilt** | No backup code of any kind: `backup`, `pg_dump`, `snapshot` match only comments about surviving backups (`crates/ordo-exec/src/erase.rs:17`, `:28`, `crates/ordo-exec/src/bucket.rs:259`). Undo is per command group (`crates/ordo-exec/src/undo.rs:949`), not per point in time. | A sprint, and it needs a design: see §2.8. |
| 1.3 | "How far back does undo reach, and can my tenants differ?" | **unbuilt** | `LogPolicy::undo_horizon` defaults to 90 days and is a Rust constant (`crates/ordo-exec/src/cadence.rs:96`, `:116-117`). The CLI prints its own limitation: "`policy commandlog` is not yet parsed from source" (`crates/ordo-cli/src/operator.rs:490`). | Small. The value is already threaded; the missing part is parsing it and keying it per space. |
| 1.4 | "Point-in-time recovery for the whole deployment" | **unbuilt** | Tracked as 17j. Nothing in `crates/` touches WAL archiving or a restore drill. | Operational rather than code, plus the boot assertion 17j names. |
| 1.5 | "Clone a customer into staging" | **unbuilt** | No match in tree for `clone_space`, `move_space` or `relocate`. | Small once 1.1 lands, because export already produces the bundle. |
| 1.6 | "Move a customer onto their own database" | **unbuilt, deferred with a recorded reason** | `plan-space-routing.md:114` puts it out of scope and Q-2 states why: the chain must not fork, the undo horizon must survive, and blob references must be repointed. | A build with its own consistency argument. It is the feature customers ask for the day they hear the tier exists. |
| 1.7 | "Delete a customer completely" | **built, no production caller** | `ExitOps::delete` (`crates/ordo-host/src/exit_ops.rs:67`, `:260`) calls `erase::erase_space` (`crates/ordo-exec/src/erase.rs:357`), which destroys the space master key first (`:395`) and then sweeps blobs (`:425`). `PlatformExecutor::new` is constructed only in tests. | Wiring, behind 17a. The mechanism is stronger than the question assumes: see §4. |
| 1.8 | "Export a customer when they leave" | **built, no production caller, and it is not a backup** | `export_space` (`crates/ordo-exec/src/export.rs:791`) behind `std.space.export` (`crates/ordo-host/src/exit_ops.rs:65`, dispatch at `crates/ordo-host/src/platform_ops.rs:792`). The checklist records the ruling: "export is a disclosure format, not a backup format" and `ordo import` is unbuilt (`docs/roadmap-checklist.md:3531-3533`). | Wiring, plus `ordo import` as its own build. |

### Dimension 2 · Isolation under load
*"What happens to my other customers when one of them runs a bad query?"*
Benchmark: **Supabase** for the Postgres-native pooling expectation. There is no
good comparator for the append lock, because no comparator hash-chains writes.

| # | Capability, as the buyer asks it | Verdict | Evidence | Cost |
|---|---|---|---|---|
| 2.1 | "One customer runs a slow query. What do the others see?" | **unbuilt** | One `tokio::sync::Semaphore` sized `available_parallelism().clamp(2, 8)` serves every space (`crates/ordo-exec/src/lib.rs:87-91`, `:255-256`). `Pool::acquire` takes no space argument (`:359-375`); losers get `PoolError::Saturated` after a 2 second wait. No per-space budget, reservation or accounting exists. | A sprint. This is the heaviest gap in the audit and §2.7 argues why. |
| 2.2 | "What stops a query running forever?" | **unbuilt** | No match in tree for `statement_timeout`. `dial` sets no session GUCs (`crates/ordo-exec/src/lib.rs:270-278`). The cost gate classifies venue and row bounds, never index coverage (`crates/ordo-plan/src/cost.rs:1-25`). | One line on `dial`, plus a policy decision about the value. The cheapest high-value fix in this audit. |
| 2.3 | "How many writes a second can one of my customers do?" | **structural, and unmeasured** | `pg_advisory_xact_lock(APPEND_LOCK_NS, space_lock_key(space))` serialises every append for one space (`crates/ordo-exec/src/commandlog.rs:112`, `:126-129`, `:203-207`). The lock is transaction-scoped, so it is held to commit. | The ceiling is real. Its size is unknown and §3 shows it is narrower than the plan predicted. |
| 2.4 | "Can I see whether that ceiling is biting?" | **built, no production caller** | `AppendObserver::lock_wait` (`crates/ordo-exec/src/metrics.rs:38-45`) is fed from the append path (`crates/ordo-exec/src/commandlog.rs:221`), but `Pool::observing` (`crates/ordo-exec/src/lib.rs:347`) is called only from `crates/ordo-host/tests/lock_wait.rs` and `crates/ordo-host/tests/deployment.rs:224`. | Wiring, behind 17a. The instrument that would answer 2.3 is the instrument nothing turns on. |
| 2.5 | "Can one customer's write abort another's?" | **structural at the shared target** | Two spaces take different advisory locks and still share `$commandlog`, so SSI raises `40001` between them (`crates/ordo-exec/src/commandlog.rs:163-170`). It reaches only the two serializable classes (`crates/ordo-exec/src/lib.rs:202-203`) and is retried five times with backoff (`:250`, `:434-458`). | None today. Routing removes it per target. Worth stating because the symptom is a stranger's retry storm. |
| 2.6 | "Per-tenant rate limits and quotas" | **unbuilt** | No match in tree for a rate limit, quota or throttle. `RETRY_BUDGET` (`crates/ordo-exec/src/lib.rs:250`) is per request and unrelated. Tracked as 17n. | A sprint, and `plan-production-ops.md` M1-M3 already designs it. |
| 2.7 | "Which customer is causing this?" | **built, off by default** | Per-space metric labels are opt-in behind `ORDO_METRICS_PER_SPACE` (`crates/ordo-host/src/metrics.rs:100-103`, `:275-281`), with a documented cardinality argument. | None. The default is defensible; the disclosure is that a deployment which never sets it cannot attribute a slowdown. |
| 2.8 | "Priority, admission control, QoS" | **unbuilt** | No match in tree. | Not owed until 2.1 and 2.6 exist. |

### Dimension 3 · Data platform expectations
*"Do I get search, analytics and time-series, or do I need three more systems?"*
Benchmark: **Supabase** for the Postgres-native set; **CipherStash** for the
encrypted half.

| # | Capability, as the buyer asks it | Verdict | Evidence | Cost |
|---|---|---|---|---|
| 3.1 | "Full-text search with ranking" | **refused** | `docs/sql-translation-corpus.md:810-812`, I5: "not possible (by design for v1)", with `matches` reserved and the stated workaround being "search-box UX is client-side over synced data". I6 refuses fuzzy and trigram on the same ground (`:814`). | A conversation, and §3 shows the refusal's own workaround is unbuilt. |
| 3.2 | "Substring and pattern search" | **built, and unindexed** | `TextOp` is a closed three: `Contains`, `Starts`, `Matches` (`crates/ordo-syntax/src/ast.rs:67-71`), emitted as `strpos`, `left(...)` and `~` (`crates/ordo-plan/src/lib.rs:1700-1702`). The only index DDL is a plain btree on declared columns (`crates/ordo-schema/src/ddl.rs:567`). | None to keep. The mitigation is real: the primary key leads with `"$space"` (`crates/ordo-schema/src/ddl.rs:377-382`), so a scan is confined to one tenant and paged. |
| 3.3 | "Vector search over embeddings" | **unbuilt, and foreclosed by the extension rule** | No match in tree for vector, embedding or knn as a language or planner feature. `CREATE EXTENSION` appears nowhere in the tree, which is what the no-extension property means in practice. | A redesign if done in Postgres. A sprint if done as a projection beside it: see §3. |
| 3.4 | "Aggregates and grouping" | **built** | Nine aggregates parse (`crates/ordo-syntax/src/declparse.rs:1738-1745`) and emit (`crates/ordo-plan/src/lib.rs:1428-1515`), with `GROUP BY` at `:1239-1244`. | None. |
| 3.5 | "Analytics across all my tenants" | **structural** | `Planner::table` returns the source and its space predicate as one value (`crates/ordo-plan/src/lib.rs:1028-1035`), the planner holds exactly one space (`:1008-1019`), and `verify_scoped` refuses any unregistered or unconstrained alias with `unscoped_source` (`:676-703`), on every exit path (`:766`, `:778`, `:896`). A ref cannot cross spaces because the foreign key is composite (`:1211`). | A redesign to change. The workaround is a derived projection, and §2.1 argues it is the right one. |
| 3.6 | "Sort, range or aggregate an encrypted column" | **structural** | Sealed values may be projected and nothing else; every other position raises `sealed_unsupported` (`crates/ordo-sema/src/lib.rs:26-33`, `:3952-3957`). | A redesign, and §5 argues against taking it. |
| 3.7 | "Equality lookup on an encrypted column" | **built** | `sealed indexed` is an HMAC blind index computed inside the provider so the key never reaches Postgres (`crates/ordo-crypto/src/custody.rs:252-264`), with its own cost class reaching the executor (`crates/ordo-exec/src/lib.rs:641`). | None. This is the row the comparison set does not beat: see §4. |
| 3.8 | "Permissions that depend on an encrypted field" | **structural** | Guards walk the plaintext skeleton only; a sealed hop raises `guard_on_sealed` (`crates/ordo-schema/src/lib.rs:2338-2348`). | A redesign. The modelling answer is to guard on membership rather than on the protected value. |
| 3.9 | "Read replicas for my reporting load" | **unbuilt** | `PoolConfig` holds one host and one database (`crates/ordo-exec/src/lib.rs:65-76`). Read ops are already classified `READ COMMITTED, read-only` and the isolation table calls them replica-eligible (`:199`). | A sprint, and cheaper than most, because the classification that makes it safe already exists. |
| 3.10 | "Time-series and telemetry volume" | **unbuilt, with a recorded answer** | `corpus-rulings.md:5331-5342`: the blob tier is the unchained tier, and since C4 an upload is an ordinary converge, so a thousand records in one sealed object cost one chain append. | A conversation plus modelling guidance. What is given up is ad-hoc query over the detail. |
| 3.11 | "A query result cache" | **unbuilt** | Three caches exist and none is one: the op-registry version cache (`crates/ordo-exec/src/registry.rs:5`, `:85`), the idempotency replay cache (`crates/ordo-api/src/idempotency.rs:6-117`), and the live-query invalidation bus, which carries no data (`crates/ordo-host/src/invalidation.rs:1-31`). | Not owed. Nothing measured says it is the bottleneck. |

### Dimension 4 · Schema and change management
*"How do I ship a breaking change to ten thousand tenants?"*
Benchmark: **Neon and PlanetScale** for branching; **Convex** for ergonomics.

| # | Capability, as the buyer asks it | Verdict | Evidence | Cost |
|---|---|---|---|---|
| 4.1 | "Apply a schema change to a running deployment" | **unbuilt** | `ordo apply` "does not run DDL" by its own doc (`crates/ordo-cli/src/operator.rs:501`); `ordo plan --against` reads and probes and writes nothing (`crates/ordo-exec/src/migrate.rs:1-30`). `plan-schema-evolution.md` §0.1 states it and increments 2 to 5 are unbuilt. | Tracked as 13b-13e and 17e. The plan exists and is good. |
| 4.2 | "Ship it without taking my tenants down" | **structural** | `plan-schema-evolution.md` §6, verbatim: "There is no per-space migration... A 40-minute backfill of `transactions` is a 40-minute write outage on `transactions` for every space in the deployment." One set of tables holds every space's rows. | A redesign to remove, and routing only removes it per target. §2.4 works it through. |
| 4.3 | "Tenant A installs an app tenant B does not have" | **structural** | `$artifacts` and `$activations` carry no `$space` column (`crates/ordo-exec/src/registry.rs:66-78`) and the module doc states the consequence: `$activations` "records which version **the deployment** points at, one row per re-point" (`:14-15`). | A large redesign touching the registry, the planner cache, deploy propagation and the load gate. `access-model-explained.html` §2 prices the three options. |
| 4.4 | "Roll a bad deploy back" | **built** | `Deployment::swap` behind a `Verified` gate (`crates/ordo-host/src/deploy.rs:33-52`), reachable from `ordo rollback` and `ordo deploys` (`crates/ordo-cli/src/operator.rs:224-225`). | None, except that it rolls back every tenant at once, which follows from 4.3. |
| 4.5 | "Preview and branch environments" | **unbuilt** | No match in tree for a branch or preview deployment concept. | A sprint for a second deployment against a second database. A redesign for anything per space, because it inherits 4.3. |
| 4.6 | "Per-tenant app versions" | **structural** | Same evidence as 4.3. | Same. |

### Dimension 5 · Identity and access
*"Does this fit our SSO, and can I give a contractor access to one project?"*
Benchmark: **WorkOS and Clerk** for packaging; **Supabase** for the built-in set.

| # | Capability, as the buyer asks it | Verdict | Evidence | Cost |
|---|---|---|---|---|
| 5.0 | "How does any user get a credential?" | **unbuilt, and it is the door's own prerequisite** | The HTTP surface has no authentication route (`crates/ordo-host/src/serve.rs:153-164`, `crates/ordo-api/src/router.rs:120-148`). `token::mint` has no caller outside tests (`crates/ordo-identity/src/token.rs:91`). `session.rs` describes the ceremony-to-token step in the conditional: mint "could put a" session into a token (`crates/ordo-identity/src/session.rs:5`). | A sprint, and it precedes every other row in this dimension. |
| 5.1 | "Log in with our Okta or Entra" | **unbuilt** | `StubOidcProvider` models the authorization-code shape with no network I/O, and says so: a real provider "additionally exchanges the code (PKCE) and validates the ID-token signature against the issuer's JWKS; those steps are network I/O" (`crates/ordo-identity/src/provider.rs:515-523`). Of six `AuthMethodKind` variants (`crates/ordo-identity/src/principal.rs:468-475`), three have an implementation and none reaches a route. | A sprint. The trait, the ceremony, the state and nonce handling and the authenticator-id discipline are already there. |
| 5.2 | "SAML" | **unbuilt** | No match in tree. | A sprint on top of 5.1, and a decision about whether to carry it at all. |
| 5.3 | "SCIM provisioning from our directory" | **unbuilt** | No match in tree. Compounded by 1.1: there is nothing to provision into. | A sprint, behind 1.1. |
| 5.4 | "An API key for our integration" | **the mechanism is built; the issuance surface is not** | `CapabilityGrant` carries an explicit op allowlist pinned to the manifest hash, and `AccessEngine::grant_for` mints one (`crates/ordo-identity/src/access.rs:615`). No HTTP op calls it: the whole platform op surface is nine ops and none of them issues a grant (`std.space.export`, `.delete`, `.callersOf`, `.attestTo`, `.chainProof`, `.attachBucket`, `platform.health.deep`, `.metrics`, `.security.alerts`). | A sprint, and it is mostly the consent page rather than the crypto. |
| 5.5 | "A contractor on one project and nothing else" | **built in the language, unsettled at the door** | The window is the mechanism. `Role` has four variants and none is a guest (`crates/ordo-identity/src/principal.rs:105-110`), and `access.rs` contains no occurrence of "guest", so what admits a null-role members row is undecided (`access-model-explained.html` §7, open question 8). Delegation is enforced on every request that claims one (`crates/ordo-api/src/router.rs:408-417`) and nothing anywhere creates one: `DelegationRegistry::record` and `PgGrantRegistry` are constructed only in `crates/ordo-host/tests/`. | A day to settle, then a sprint. Until then the window path is unproven. |
| 5.6 | "A public read for our marketing page" | **unbuilt at the door, and the language says otherwise** | G15 makes anonymous a caller whose guards evaluate false; `check_access` refuses a missing bearer before any guard compiles (`crates/ordo-api/src/router.rs:722`, `:730`, and the blob gate the same at `:206`). | One construction site in `router.rs`, plus three rules `access-model-explained.html` §3 proposes. Cheap and load-bearing for any paywall. |
| 5.7 | "Delegated administration" | **partial** | Roles resolve per space, and the corpus records that there is no deployment-level role concept, so the operator who reads the deployment security artifact can also prune and disable it (`docs/corpus-rulings.md:5182-5188`). | A sprint, and the corpus already calls it "an item, not a solved problem". |
| 5.8 | "Find a user across our tenants" | **refused** | `definition-language.md:2922-2924`: "Invitation, never enumeration... no cross-space principal lookup exists." | A conversation. The reason is the one that makes the topology worth having. |

### Dimension 6 · Compliance and data governance
*"Where does my data live, and can you prove what happened to it?"*
Benchmark: **CipherStash** for the encrypted half; **WorkOS** for the packaging.

| # | Capability, as the buyer asks it | Verdict | Evidence | Cost |
|---|---|---|---|---|
| 6.1 | "Each of my customers gets their own key" | **built** | One space master key per space, per-epoch data keys under it, both wrapped (`crates/ordo-crypto/src/custody.rs:545-559`, `:636-649`). Sealing happens before Postgres sees the value, so the boundary is in the data rather than in the storage. | None. §4 argues this is the strongest row in the audit. |
| 6.2 | "We hold the key" | **unbuilt** | The master key comes from `ORDO_MASTER_KEY` and a deployment without one is refused rather than defaulted (`crates/ordo-crypto/src/custody.rs:651-661`). No KMS binding exists; Vault, AWS and GCP are planned in 17b. | A sprint per binding. The trait is already shaped for it (`custody.rs:23-25`). |
| 6.3 | "Rotate the key" | **built, no production caller** | `rotate` is a required trait method with two implementations (`crates/ordo-crypto/src/custody.rs:485`, `:859`, `:992`) and every call site is a test. The checklist is blunter than this audit could be: "`rotate()` has **no production caller at all**", and so revocation never opens a new epoch (17i). It also re-seals nothing. | A sprint, and 17i already scopes it. The correctness half is larger than the logging half. |
| 6.4 | "Keep German customers in Germany" | **unbuilt** | No match in tree for residency, region or jurisdiction outside the S3 signer. Routing makes it reachable and nothing implements it. | A sprint on top of 1.1 and `plan-space-routing.md` S3, because a target already has a host. |
| 6.5 | "Stream our audit log to our SIEM" | **unbuilt** | Nothing reads either stream out. `SecurityStore::entries` (`crates/ordo-secevent/src/store.rs:228`) is called only by integrity and pruning code. The nine platform ops include no reader for `$commandlog` or `$secevents`, and the CLI's verbs are `check, plan, interface, gen ts, apply, rollback, deploys, verify, checkpoint, prune, revert` (`crates/ordo-cli/src/main.rs:105`, `:160-163`), which operate on integrity rather than content. `std.space.chainProof` discloses "positions, 32-byte digests and signatures" and explicitly not rows, images, actors or correlation ids (`crates/ordo-host/src/attest_ops.rs:22-24`), and `AttestOps` has no constructor anywhere in the tree including tests. | A sprint. Partly homed in 17m and 7d, and §3 argues the buyer-facing half is not covered there. |
| 6.6 | "Retention and legal hold" | **retention built, legal hold unbuilt** | Retention runs through `prune` with a floor and a named floor holder, reachable from `ordo prune` (`crates/ordo-cli/src/operator.rs:440-459`). Legal hold "has no producer anywhere in `crates/`, so deletion cannot respect one and does not pretend to" (`docs/roadmap-checklist.md:3628`). | A sprint for hold, and it interacts with 6.7. |
| 6.7 | "Right to erasure for a whole customer, provably" | **built, no production caller, and stronger than asked** | Deletion destroys the space master key, not only the data keys, because blind-index keys are master-derived and a surviving master leaves `$bidx` answerable by anyone who can recompute the HMAC (`docs/roadmap-checklist.md:3535-3545`). Keys go before rows. | Wiring, behind 17a. |
| 6.8 | "Forget one individual inside a shared space" | **unbuilt** | Erasure is whole-space only: `erase_space` takes a `SpaceId` and destroys its custody (`crates/ordo-exec/src/erase.rs:357`, `:395`). No match in tree for `delete_principal` or a per-subject erasure primitive. A DSAR against one person in a multi-user tenant has no implemented answer. | A sprint, and it is the harder erasure because the chain must retain the fact of the write while losing its content. |
| 6.9 | "Evidence for our SOC 2 audit" | **partial** | The chain, signed checkpoints, `ordo verify`, and layer-1 pinning on every response (`crates/ordo-api/src/attest.rs:1-22`, headers at `crates/ordo-api/src/router.rs:475-504`). No control mapping, no evidence export, and the client that would consume the pin does not read it: no match for `x-ordo-head` anywhere under `clients/`. | A sprint for the reader, plus documentation work that is not engineering. |

### Dimension 7 · Realtime and offline
*"Does it work on a plane, and what happens when two people edit at once?"*
Benchmark: **Convex** and **Firebase** for live queries; **Jazz** for local-first.

| # | Capability, as the buyer asks it | Verdict | Evidence | Cost |
|---|---|---|---|---|
| 7.1 | "The screen updates when someone else writes" | **built, single process, and unconsumed** | `?live=1` is honoured in the generated router and subscribes through the bus (`crates/ordo-api/src/router.rs:1171-1220`); writes publish inside the transaction (`crates/ordo-exec/src/lib.rs:996-999`). Every constructed state wires `MemoryBus`; `PgInvalidationBus::connect` has one caller in the tree and it is `crates/ordo-host/tests/deployment.rs:251`. The shipped React client never subscribes: no match for `EventSource` or `live=1` under `clients/ordo-react/src`. | A sprint on the client, wiring on the server. The server already names the honest gate (`crates/ordo-host/src/lib.rs:196-201`). |
| 7.2 | "Presence, who else is here" | **unbuilt** | The only matches are WebAuthn user presence, a different concept. | A sprint, and it depends on 7.1. |
| 7.3 | "Two people edit the same row" | **built and wired** | CAS on `rev`, with `cas_miss` and `version_skew` as distinct answers (`crates/ordo-api/src/error.rs:125`, `:396`, `:401`), decided at compile time (`crates/ordo-plan/src/artifact.rs:661-674`) and enforced at runtime (`crates/ordo-exec/src/undo.rs:1482`). | None. |
| 7.4 | "It works on a plane" | **unbuilt** | No client-side store: no match for `indexeddb`, `opfs`, `localStorage` or a cache under `clients/`. Tracked as 5g-pre. | The largest single unbuilt item in this audit, 8 to 12 engineer-weeks by `f7-vault-plan.md` §6's own estimate. |
| 7.5 | "Revoke what a device already has" | **unbuilt, ruled** | 5g-rev records the three layers and that only the third binds. The server half is buildable alone. | A sprint for the server half, behind 7.4 for the rest. |

### Dimension 8 · Integration
*"How does this talk to the rest of our stack?"*
Benchmark: **Firebase** for what a large installed base treats as table stakes.

| # | Capability, as the buyer asks it | Verdict | Evidence | Cost |
|---|---|---|---|---|
| 8.1 | "Call our endpoint when a row changes" | **unbuilt** | The one outbound path is the attestation webhook (`crates/ordo-exec/src/webhook.rs:1-30`), and even that never runs: `AttestDelivery`, which spawns the delivery loop, has no caller outside `crates/ordo-host/tests/attest_webhook.rs`. The egress kernel names feed delivery, notification sinks and import bridges as future consumers (`crates/ordo-egress/src/lib.rs:15-16`). | A sprint per consumer, and the hard half is already paid: one client, six pins, one place to audit (17k). |
| 8.2 | "Change data capture into our warehouse" | **unbuilt, and the substrate exists** | No match in tree for CDC, logical replication or a replication slot. The per-space hash-chained `$commandlog` is a better change feed than a WAL tail, and nothing reads it out. | A sprint. §3 argues this is the cheapest high-value row in the audit. |
| 8.3 | "A trigger that calls a URL" | **unbuilt** | Triggers enqueue in-process routine runs over three tables (`crates/ordo-exec/src/triggers.rs:72-93`), with no HTTP anywhere in the module. The declaration form itself is unbuilt because its one operand is lean (checklist 12a). | Behind 12a and 8.1. |
| 8.4 | "Import our existing data" | **unbuilt** | `ordo import` is named as owed by the export ruling (`docs/roadmap-checklist.md:3533`) and does not exist. | A sprint, and it is the other half of 1.8. |
| 8.5 | "Give us an OpenAPI spec" | **unbuilt** | No match in tree for OpenAPI. The descriptor is `ordo.interface/2` (`crates/ordo-plan/src/interface.rs:114`), which carries more than OpenAPI can express: guard provenance, cost class, key release, the bypass disclosure. | Small. A second backend in `ordo-gen` that projects the descriptor down. |
| 8.6 | "A client in Python or Go" | **unbuilt** | `ordo-gen` emits TypeScript only, for `clients/ordo-sdk` and `clients/ordo-cdk` (`crates/ordo-gen/src/lib.rs:1-53`, `:64`). The CLI refuses any other backend by name and says they are planned (`crates/ordo-cli/src/main.rs:135-141`). | A sprint per language, and each is a pure consumer of the same descriptor. |

---

## 2 · The structural findings, at length

These are the eight that change plans rather than schedules.

### 2.1 One query reaches one space, and that is the whole product

`Planner::table` is the only way to name a base table, and it returns the source
and the predicate that confines it **as one value**, so a caller cannot take the
first and forget the second (`crates/ordo-plan/src/lib.rs:1028-1035`). The
planner holds exactly one space for the life of a plan, either bound by the door
or reserved as a single parameter slot (`:1008-1019`). Then `verify_scoped` scans
the emitted SQL and refuses any alias the planner did not register, or any
physical alias without its `"$space"` equality, with the code `unscoped_source`
(`:676-703`). It runs on every exit path (`:766`, `:778`, `:896`), and
`ordo_plan::plan` is called from the executor's real read and write paths
(`crates/ordo-exec/src/lib.rs:653`, `:1259`).

**What this forbids.** Any single query that reads two tenants. That includes the
obvious case, a cross-tenant join, and the one buyers actually ask for, which is
a dashboard counting something across their whole customer base.

**What changing it would cost.** A redesign, and not of the planner alone. The
composite primary key `("$space", "id")` (`crates/ordo-schema/src/ddl.rs:377-382`),
the composite foreign keys, and the compiled guards all rest on one space per
statement. Under `plan-space-routing.md` it becomes physically impossible as
well as logically refused, because two spaces may be in two databases.

**The workaround, and it is the right one.** A derived projection, rebuilt from
what each space consented to expose, never a source of truth. The corpus already
reaches for exactly this shape for cross-publisher discovery and gives the reason
that generalises: a projection can be pruned when a publisher is erased, and
copies in ten thousand tenant databases cannot be recalled
(`docs/corpus-rulings.md:5294-5301`).

### 2.2 One schema describes one deployment

`$artifacts` and `$activations` carry no `$space` column
(`crates/ordo-exec/src/registry.rs:66-78`) and the module doc states what follows
at `:14-15`: `$activations` "records which version **the deployment** points at,
one row per re-point, newest wins."

Two buyer questions die here and they look unrelated. A marketplace where tenant
A installs an app tenant B does not: not expressible. Per-tenant schema
variation of any kind, including a custom field: not expressible. Both are the
same row in the same table.

**What changing it would cost.** `access-model-explained.html` §2 prices it
honestly. Per-space activation touches the artifact registry, the planner's
cache, deploy propagation on `DEPLOY_CHANNEL` (`registry.rs:88`) and the load
gate, which builds its verification shadow from the DDL the artifact asserts its
database holds and would need a per-space shadow. It also interacts badly with
routing, because the registry stays in the control database while entity tables
move to targets, so the control database would hold the authority for DDL applied
to databases it does not own.

**The decision that comes first is what an app is allowed to be.** If an app may
define its own shapes, per-space activation is the only honest answer. If an app
is configuration over shapes the platform already defines, installing is a row
and costs almost nothing. That decision is owed before the engineering is scoped.

### 2.3 A transforming migration is a deployment-wide write outage

Follows from 2.2 and is worth separating because it is what an operations buyer
asks. `plan-schema-evolution.md` §6 says it in its own words: "There is no
per-space migration, and the plan must say so on the first page of its output...
A 40-minute backfill of `transactions` is a 40-minute write outage on
`transactions` for every space in the deployment."

Routing narrows this and does not remove it. The freeze is per entity and applies
to every space on the target being migrated, so the long tail sharing the default
target still freezes together. A tenant promoted to their own database gets their
own migration window, which is one more thing the dedicated tier is actually
worth.

### 2.4 Sealed values are opaque to the database and to the guards

Two separate refusals with two separate codes, and conflating them costs a
schema redesign. A sealed field may be projected and nothing else; comparing,
sorting, grouping or computing from it raises `sealed_unsupported`
(`crates/ordo-sema/src/lib.rs:26-33`, `:3952-3957`). Separately, a guard walk
that touches a sealed field raises `guard_on_sealed`
(`crates/ordo-schema/src/lib.rs:2338-2348`), so authorization can never depend on
the protected value.

The one exception is equality on a `sealed indexed` field, answered in SQL by an
HMAC digest computed inside the key provider so the key never enters Postgres
(`crates/ordo-crypto/src/custody.rs:252-264`). It answers one question
completely, and leaves the rest of search to the refusal above.

**Why not widen it.** Order-revealing encryption would buy ranges and is refused
in three places with the same three words: "no order-revealing schemes, ever"
(`docs/definition-language.md:671`, `docs/glossary.md:263`, `docs/traps.md:158`).
§5 argues the refusal should stand.

### 2.5 The WAL is cluster-wide, so a per-tenant key can never cover the write stream

One Postgres instance is one write-ahead stream carrying every database's writes,
and it feeds every standby and every archive. Per-tenant keys, by any mechanism,
cover tables and indexes and never the stream (`docs/corpus-rulings.md:5259-5265`).

This is recorded as a limit and reads as the sharpest argument in the corpus for
application-level sealing, because a sealed value is ciphertext before Postgres
ever sees it, and therefore ciphertext in the WAL, in every standby, and in every
backup. The topology is what creates the argument.

### 2.6 The append lock is a per-space write ceiling, and it is narrower than the plan assumed

`pg_advisory_xact_lock(APPEND_LOCK_NS, space_lock_key(space))` makes one writer
at a time the only appender for one space
(`crates/ordo-exec/src/commandlog.rs:203-207`). The lock is transaction-scoped,
so it is held from acquisition to commit and cannot be released early.

What it covers is the part that surprised this audit. The lock is taken **as late
as possible**. The entity effect runs first, off the lock
(`crates/ordo-exec/src/lib.rs:963-973`), and `append_group` is the last statement
before commit (`:1055`). Inside the lock there are three statements: read the
head, refuse a duplicate correlation, hash and insert
(`crates/ordo-exec/src/commandlog.rs:239-297`). `crates/ordo-host/tests/lock_wait.rs`
proves the ordering mechanically rather than by reading the call order, using
`pg_locks` to assert that no advisory lock is held while the replay journal's
update runs and exactly one is held the moment the append returns.

So the per-space ceiling is one over the cost of three round trips plus a commit
fsync, not one over the cost of a whole write transaction. It is still a ceiling,
it is still per space, and no amount of hardware raises it, because it is a
serialization point rather than a throughput limit. What is not true is the
implication that a tenant's ordinary write volume runs through it: their query
planning, access check, guard evaluation and entity writes all happen outside it.

**What is genuinely missing is the number.** There is no throughput benchmark
anywhere in the tree: no `benches/` directory, no criterion dependency, and
`lock_wait.rs` measures wait latency under a synthetic hold rather than a rate.
Its own doc comment says why it is not the measurement that is owed. And the
instrument that would answer the question in production, `Pool::observing`, has
no production caller (§1, row 2.4). The release gate is unmeasured and currently
unmeasurable.

### 2.7 One pool serves every tenant, and this is the isolation gap

This is the finding the plan did not predict and it is the heaviest in the audit.

The pool is a single `tokio::sync::Semaphore` sized
`available_parallelism().clamp(2, 8)` (`crates/ordo-exec/src/lib.rs:87-91`,
`:255-256`), and `Pool::acquire` takes no space argument (`:359-375`). The module
doc is candid that this is deliberate: "The pool IS the platform's DB-side
concurrency bound" (`:9-11`). That argument is correct about the *total* bound
and silent about how the bound is divided, and division is the whole of tenancy.

Three consequences compose badly.

**The queue is FIFO across tenants.** One space issuing slow queries holds
connections; every other space's request queues behind it on the same semaphore
and fails with `PoolError::Saturated` after two seconds. Eight thousand tenants
share between two and eight permits.

**Nothing bounds a single query.** There is no `statement_timeout` anywhere in
the tree and `dial` sets no session GUCs (`:270-278`). The cost gate classifies
where work happens and how many rows a plan yields, never whether a predicate has
an index (`crates/ordo-plan/src/cost.rs:1-25`), so a `contains` over a large
tenant's table is a sequential scan that passes every gate the platform has.

**Nothing attributes the damage.** Per-space metric labels are opt-in
(`crates/ordo-host/src/metrics.rs:275-281`), for a defensible cardinality reason,
so a deployment that has not set `ORDO_METRICS_PER_SPACE` sees a slow deployment
and no name.

**The verdict is unbuilt rather than structural**, which is the good news: the
topology permits a per-space permit budget, and `PoolResolver` (S1 of
`plan-space-routing.md`) is the seam a budget would sit behind. The bad news is
that routing does not fix it for the tenants who most need fixing. Promoted
tenants get their own target and their own pool. The tens of thousands sharing
the default target, which is every tenant who has not paid to leave, keep sharing
one semaphore.

### 2.8 Per-tenant restore has no Postgres-shaped answer, and Ordo has a better-shaped one

Worth stating structurally because the obvious plan is wrong. The Postgres answer
to "restore one customer" is a per-database point-in-time restore, and at tens of
thousands of spaces on a sparse set of targets, restoring the target restores
every tenant on it. Physical restore is the wrong instrument here for the same
reason database-per-tenant was rejected as a default.

The instrument Ordo already has is the per-space hash-chained command log with
a revert applier that undoes one group (`crates/ordo-exec/src/undo.rs:949`) and a
checkpoint that signs a head (`:669`). "Restore this tenant to last Tuesday" is
expressible as a walk back through that chain to a timestamp. What is missing is
the walk, a horizon that is configurable per space rather than a 90-day Rust
constant (`crates/ordo-exec/src/cadence.rs:96`), and the blob-side question of
what an object restored past its sweep means.

That is design work over mechanisms that already exist, and it is the row where
Ordo could answer a procurement question better than the comparison set rather
than worse.

---

## 3 · Where §4's predictions were wrong

The plan asked for disconfirmation and this is the section it asked for.

### 3.1 Disconfirmed: the append lock does not cap a tenant's write rate the way the prediction implies

§4 predicted "the per-space append lock caps a single tenant's write rate and no
amount of hardware raises it". The second half stands. The first half overstates
what the lock covers, and §2.6 has the evidence: the lock is taken as late as
possible, after the entity effect, and holds three statements plus the commit.
The prediction would lead a reader to believe the write path serialises per
tenant. Almost all of it does not.

**And the more useful finding is underneath it.** The prediction calls the
ceiling "an unmeasured release gate". It is worse than unmeasured: there is no
benchmark harness in the tree at all, and the production instrument that would
measure it has no caller. The owed work is not "measure the ceiling", it is
"build the thing that can measure it, then measure it".

### 3.2 Disconfirmed: this market does not ask for per-tenant keys, and Ordo already has them anyway

§4 named as uncertain "whether this market asks for per-tenant keys or merely for
the phrase encrypted at rest". The audit's answer is that the question has the
wrong subject.

Ordo **already has** one key per tenant, in the strongest form available: a space
master key with per-epoch data keys under it
(`crates/ordo-crypto/src/custody.rs:545-559`, `:636-649`), applied above the
storage so it costs nothing per tenant and reaches tens of thousands of them.
Crypto-erasure is per tenant and destroys the master rather than only the data
keys, for a reason that is itself evidence of how carefully this was thought
through (`docs/roadmap-checklist.md:3535-3545`).

What buyers actually ask, and what Ordo does not have, is **customer-managed**
keys. The master key comes from `ORDO_MASTER_KEY`
(`crates/ordo-crypto/src/custody.rs:651-661`) and there is no KMS binding, so the
answer to "we hold the key" is no. That is a different gap with a different cost:
one sprint per binding against a trait already shaped for it, rather than a
cryptographic redesign.

**So the prediction should be replaced.** The question to put to a buyer is
whether they will accept a per-tenant key the platform holds. In regulated
segments the answer is no, which makes 6.2 a procurement gate and 6.1 a marketing
asset.

### 3.3 Partly disconfirmed: the no-extension property costs less than the plan feared

§4 flagged as uncertain "whether the no-extension property is worth what it
costs, given that it rules out `pg_tde`, `pgvector` and full-text extensions in
one stroke". The claim is verified true in the strictest sense: `CREATE EXTENSION`
appears nowhere in the tree, and the rule has already shaped decisions on record
rather than being asserted after the fact, for instance choosing `sha256(bytea)`
because it is in core since 11 and rejecting SipHash because it is not
(`docs/corpus-rulings.md:833`, `:852`).

The cost is smaller than the three-way framing suggests, and it is smaller for
three different reasons.

**`pg_tde` is already refused on other grounds.** The scale correction killed
database-per-tenant as a default, and with it `pg_tde` as a general answer
(`docs/corpus-rulings.md:5214-5228`). The extension rule is not the binding
constraint there; the catalog size and the migration time are.

**Full-text is refused on its own grounds.** `sql-translation-corpus.md:810-812`
refuses ranked search for v1 with a stated workaround, so removing the extension
rule would not change the language surface, only the implementation available if
the refusal were reversed.

**`pgvector` is the one where the rule genuinely binds**, and even there the
usual answer for a multi-tenant platform is a separate vector store keyed by a
tenant-scoped id rather than a column in the transactional database, which the
topology permits today.

**So the property is worth what it costs**, and the honest cost is one
capability, vector search, at the price of running on every managed Postgres
there is. That trade is good. What deserves stating is the second-order cost the
plan did not name: every capability the rule forecloses lands on the projection
side of §2.1, and Ordo has no projection machinery yet. The extension rule is
cheap; the missing projection substrate is what makes it feel expensive.

### 3.4 Disconfirmed in detail: the refused answer to search depends on unbuilt machinery

The full-text refusal is recorded and reasoned, and its stated escape is
"search-box UX is client-side over synced data"
(`docs/sql-translation-corpus.md:811-812`). There is no client-side sync. No
client store exists anywhere under `clients/` and the item that would build one,
5g-pre, is unstarted.

So the refusal currently has no workaround at all. A buyer asking for search is
told to do it on the client, and the client cannot. That does not make the
refusal wrong; it makes it conditional on 7.4, which nothing records.

### 3.5 Confirmed, and sharper than predicted

Three predictions held and got worse on inspection. **Per-tenant backup** is not
merely unbuilt: there is no backup code of any kind, and §2.8 shows the obvious
Postgres-shaped fix does not fit the topology. **A reader for either security
stream** is unbuilt, and the audit adds that the whole platform op surface is
nine ops with no reader among them, so the tenant cannot read their own chain
either. **The client and its local cache**: confirmed, and the shipped React
client does not even consume the live-query subscription the server already
serves.

### 3.6 Disconfirmed: `platform.security.alerts` is no longer unreachable, and the record that says otherwise is stale

The brief that commissioned this audit names four calibration examples of
built-and-unwired machinery. Three hold. The fourth does not, and correcting it
matters more than confirming the others, because it is cited as a live security
defect.

`ALERTS_OP` is deployed and granted to `Role::Owner` alongside
`HEALTH_DEEP_OP` and `METRICS_OP` in `deployment_engine`
(`crates/ordo-host/src/lib.rs:1423-1447`), and the comment beside it records both
the old bug and the fix: the route "already authorizes against it and nothing
deployed it, so the route denied everyone in every deployment built through this
function (a mounted route with no op behind it), found by D0's own test rather
than by an operator reaching for it during an incident". The companion defect,
`authorize` resolving the check space from a caller-supplied `x-ordo-space`
header, was fixed in the same change. A regression test exists in
`crates/ordo-host/tests/deployment.rs`.

**Where the stale reading comes from.** `plan-opslog.md` §0.6 is the finding
("mounted and its op is deployed nowhere", `:66`) and D0 is the increment that
closes it (`:262`); the plan is doing its job and the increment has landed, so
reading §0 alone gives the wrong answer. `corpus-rulings.md:5197-5201` is the one
record that states it in the present tense with no increment beside it:
"`platform.security.alerts` is mounted with its op deployed nowhere, so it
answers 403 to everyone." That sentence was true on 2026-09-10 and is not true of
this tree. Neither file is this audit's to edit; both are noted so the next
reader does not rediscover a closed bug, and so the next audit does not inherit
the same calibration example.

### 3.7 A prediction the plan did not make

Eleven things are built and have no production caller. Three of the brief's four
calibration examples are confirmed: `MeteredKeys::with_stream` (defined at
`crates/ordo-host/src/metrics.rs:877`, every caller a test, and `MeteredKeys::new`
itself constructed only in a test fixture), `rotate`, and `PgInvalidationBus`.
Eight more were not named: `AttestDelivery` and its delivery loop,
`Pool::observing` and with it the whole append-lock instrument,
`PlatformExecutor::new` and the five platform-op handlers `deployment_engine`
never attaches, `AccessEngine::grant_for`, `DelegationRegistry::record`,
`token::mint`, `LocalKeyProvider::from_env`, and the trigger runner. `AttestOps`
is worse than all of them: it has no constructor anywhere in the tree, including
tests.

They share one cause, which is 17a, so the fix is one item rather than eleven.
What is worth recording is the pattern this repo keeps producing: a mechanism
built to a high standard, with its tests, its refusal classes and its module doc
argued, and no line anywhere that turns it on. An audit that counts features
would score this tree far higher than a buyer would.

---

## 4 · What Ordo does that the comparison set does not

Recording this because an audit that only finds gaps produces a plan to become a
worse version of someone else.

**A per-tenant key at a scale nobody else offers it.** CipherStash sells
application-level encryption and does not bound tenancy; Supabase and Neon offer
one key per cluster or per instance. Ordo gives one key per space across tens of
thousands of spaces with no storage overhead, because the boundary is in the data
rather than in the storage (`crates/ordo-crypto/src/custody.rs:545-559`). Nothing
in the comparison set has both properties.

**Crypto-erasure that reaches the backups.** Deletion destroys the space master
key, so a restored backup, a lagging standby and every WAL segment become
unreadable together (`docs/roadmap-checklist.md:3535-3545`). Every comparator's
answer to right-to-erasure is a delete statement that backups outlive.

**Equality on encrypted columns at SQL speed, with the leakage stated.** The
blind index is an HMAC computed inside the key provider so the key never enters
Postgres (`crates/ordo-crypto/src/custody.rs:252-264`), and the leakage is named
in the language reference rather than in a whitepaper: equality patterns, salted
per space (`docs/definition-language.md:667-672`). CipherStash is the right
comparator and Ordo's posture is the more conservative one, because it refuses
order-revealing schemes outright.

**A hash-chained per-tenant command log with a working undo.** Not an audit table
that writes alongside the data. The chain is the write path, the revert applier
is real (`crates/ordo-exec/src/undo.rs:949`), and `ordo verify` walks it from a
cron job with nothing but the database and a published key
(`crates/ordo-cli/src/operator.rs:220`). Firebase, Supabase and Convex have no
equivalent.

**Every response is an attestation.** Layer-1 pinning ships the signed chain head
with each answer (`crates/ordo-api/src/attest.rs:1-22`, headers at
`crates/ordo-api/src/router.rs:475-504`), so every client holding a 100-byte pin
is a witness and detection latency for a rewritten history is one request. The
shipped client does not read it yet, which is 6.8's gap, and the server half is
built and unusual.

**A planner that cannot emit an unscoped query.** The source and its predicate
are one value and a belt check refuses anything else (`crates/ordo-plan/src/lib.rs:1028-1035`,
`:676-703`). Supabase's RLS is the right comparator and it is enforced by the
database on statements an application composes; Ordo's is enforced by the only
thing that can compose a statement at all.

**Consent that is greppable.** `access read open` is a keyword rather than a
boolean field, deliberately, so every public surface in a schema is one search
(`docs/definition-language.md:2915-2918`). A reviewer can enumerate the
disclosure decisions in a codebase. No comparator has an equivalent.

**Cost and disclosure at deploy time.** Every op carries its cost class, the
entities it reaches through composition, each guard with its provenance and the
bypass disclosure for `unguarded` ops (`crates/ordo-host/src/serve.rs:10-17`).
The comparison set finds out what a query costs when it runs.

---

## 5 · Capabilities to refuse deliberately

So that not building these is a position rather than an omission.

**Order-revealing encryption, and therefore ranges over sealed fields.** Refused
in three documents in identical words (`docs/definition-language.md:671`,
`docs/glossary.md:263`, `docs/traps.md:158`). The refusal should stand. Every ORE
scheme leaks order, order plus a known distribution recovers plaintext, and the
buyers most likely to ask are the ones who will be harmed. The honest answer is
that a field you need to range over should not be sealed, and the compiler
already says so by name.

**Cross-space principal lookup.** Refused at `docs/definition-language.md:2922-2924`:
invitation, never enumeration. The refusal is load-bearing for the whole tenancy
claim, because a directory that can be searched across tenants is a directory
that leaks one customer's people to another.

**A per-space filtered view of the deployment security stream.** Refused with the
reason recorded at `docs/corpus-rulings.md:5164-5180`: the entries name
principals who are strangers to that space, so the filter would disclose one
tenant's people to another and every filter bug would be a tenancy breach in the
artifact where a breach is worst. Offer the derived count that names nobody.

**A second storage engine.** Refused at `docs/plan-blob.md:591`: one trait, one
binding, and a filesystem binding that is a test double and refuses to claim
production readiness.

**Cross-tenant analytics as a query.** Not yet refused anywhere, and it should be,
because §2.1 makes it a redesign and the projection answer is better on its
merits. Refusing it in writing converts a permanent "not yet" into a stated
position with a named alternative.

**Public read as a property of a row.** The keyword-over-boolean argument is
already recorded; what is not recorded is the refusal of the boolean form. An
`isPublic` field would satisfy the walk grammar and defeat the review property in
one word.

---

## 6 · The three questions this audit could not answer from the code

**1. At what write rate does the append lock actually bind for a real tenant?**
The ceiling is one over three statements plus a commit fsync, per space. Whether
that is 200 writes a second or 2,000 depends on the target's fsync latency, and
whether it matters depends on what a real tenant's peak write rate is. Nothing in
this repo knows either number. The billing-sweep case in
`access-model-explained.html` §5 is the one worked example and it is hypothetical:
ten thousand subscription re-checks on the first of the month, serialised through
one lock in one space. A customer conversation, or a synthetic load test, settles
it. A grep does not.

**2. Do the buyers in this segment want to hold their own key, or only to be told
one exists per tenant?** §3.2 splits the question and cannot answer the half that
matters. Building a KMS binding is a sprint; building the ceremony, the consent
page, the disclosure about what the platform can no longer do, and the support
burden of a customer who loses their key is much more. The answer decides whether
6.2 is the third item on the roadmap or the thirtieth, and the code cannot say.

**3. Is an app allowed to define its own shapes?** §2.2 shows this single
decision sets the cost of per-tenant installs, per-tenant schema variation, the
system space's ability to hold a catalogue tenants do not have, and the tier
ladder's shape. One answer costs a row; the other costs the artifact registry,
the planner cache, deploy propagation and the load gate. It is a product
decision, it is already named as owed in `access-model-explained.html` §8, and
nothing in the tree has an opinion.

---

*Companion to [`plan-topology-audit.md`](plan-topology-audit.md) (the method),
[`plan-space-routing.md`](plan-space-routing.md) (the topology),
`access-model-explained.html` (the three homes and the door),
`opslog-growth-explained.html` (what the streams cost), and
`corpus-rulings.md` 2026-09-10 through 2026-09-12. Every line number was read
from this tree on 2026-09-12 with no cargo run. Findings with verdicts and costs
only: what to do about them is [`roadmap-checklist.md`](roadmap-checklist.md) §20.*
