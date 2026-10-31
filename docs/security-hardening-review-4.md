# [proposed] Security hardening review 4 — boundary guarantees, not component-local claims

**[proposed] Status.** This is a cold security-architecture review. Every statement, severity, disagreement, status decision, and pin in this document is proposed; none is an adopted product ruling until it is explicitly accepted elsewhere.

**[proposed] Scope.** The review reads the current design and the present `ordo-api`, `ordo-identity`, `ordo-commandlog`, and `ordo-syntax` boundaries together. A guarantee is treated as closed only when the information needed to enforce it survives every boundary through which the operation travels.

**[proposed] Result.** Thirteen findings remain: two critical, eight high, and three moderate. Several round-3 changes are real and worth preserving, but the strongest new closure claim — atomic replay — is still impossible across the current traits.

## [proposed] Executive decision

**[proposed] Single hardest pushback.** Withdraw the claim that replay is atomic until the idempotency reservation, domain mutation, command-chain append, and replay completion have one transaction owner and a fencing token. `OpExecutor::execute` commits before `ReplayCache::put` is called (`crates/ordo-api/src/router.rs:448-457`), while the two traits share neither a transaction nor a reservation generation. A crash or a 60-second lease takeover can therefore execute the same write twice. This is not an implementation detail left for Postgres; the current interface makes the promised implementation unrepresentable.

**[proposed] Release posture.** SH4-001 and SH4-002 are release blockers for capability-bearing or mutating deployments. SH4-003, SH4-004, SH4-006, SH4-008, SH4-009, SH4-010, SH4-011, and SH4-012 must either be pinned before implementation proceeds or explicitly removed from the product's security claims. SH4-005, SH4-007, and SH4-013 are bounded but real isolation failures.

## [proposed] Threat actors, exactly

- **[proposed] Curious operator.** A platform or database operator can inspect ordinary database state, object storage, logs, metrics, and deployment metadata, but is not a member of the target space and is not assumed to possess owner-held client-first keys.

- **[proposed] Compromised worker.** One API, executor, listener, migration, identity, or command-log worker is under attacker control; other services, database constraints, signing keys, and owner devices may remain honest.

- **[proposed] Malicious tenant.** A legitimate owner or member of space A uses every authorized surface, malformed input, timing channel, leaked identifier, and retry pattern to learn about or influence space B.

- **[proposed] Malicious guest.** An unauthenticated caller or bearer of only a deliberately public/share surface probes names, error classes, identifiers, cursors, and public operation behavior without membership authority.

- **[proposed] Stolen device with local mirror.** The attacker has a device, browser profile, cached access or refresh material, local mirror, and any keys already released to that device; they do not automatically possess a fresh authenticator assertion made specifically for a new ceremony.

- **[proposed] Supply chain.** A malicious or compromised domain package, generated artifact, compiler input, dependency, image, or deployment update attempts to widen authority while retaining existing grants or to induce an implementer to follow contradictory security text.

- **[proposed] Network adversary.** A caller can delay, duplicate, replay, race, and reorder requests and observe status, length, timing, and connection lifetime; TLS cryptography is not presumed broken.

## [proposed] Findings

### [proposed] SH4-001 — Critical — the hashed manifest is not an authority-complete grant contract

**[proposed] Exact claim.** SHA-256 now authenticates bytes, but the design does not define one canonical authority manifest whose bytes cover every way an op can exercise authority. `definition-language.md:2438-2487` pins fields and row-scope shape; the older token model still says ordered output fields (`security-logging-design.md:119-145`); neither pin includes the complete write set, written fields, outbox/effect set, egress destinations and envelope tier, key-release class, parameter influence, or intrinsic step-up minimum. The identity crate accepts opaque `ManifestHash` values and folds them into one combined hash (`crates/ordo-identity/src/access.rs:118-142`, `crates/ordo-identity/src/principal.rs:284-344`) rather than carrying the per-op semantic diff the documents require. `OpSpec` likewise has no authority-manifest field (`crates/ordo-api/src/spec.rs:100-120`).

**[proposed] Concrete attack.** A supply-chain update keeps an op's output and row guards unchanged but adds an outbox write, releases a sealed field to a worker, widens the fields mutated on an already-written entity, or adds a new egress destination. Under the documented field-plus-row digest, the existing capability can remain semantically unchanged even though its effects widened. Under the current identity implementation, hashing an arbitrary artifact may instead kill every narrowing update; that is fail-closed availability, not the promised decidable narrowing rule. There is no typed contract that prevents either implementation choice.

**[proposed] Affected custody.** Managed, BYO-storage, and client-first are affected because grants, effectors, key release, and consent are control-plane authority even where value custody differs. Client-first additionally cannot rely on the server inspecting plaintext to notice an undeclared effect.

**[proposed] Pin.** Define and version one canonical `AuthorityManifest` ADT per op. It must include operation kind; parameter schema and security-relevant influence; every read and write entity, verb, field, guard walk, `$caller` restriction, ACL walk, `unguarded` marker, and trigger provenance; output disclosure; sealed/key-release class; outbox/effect and egress host/envelope declarations; intrinsic step-up requirement; cost/unbounded acknowledgement; and composition dependencies. A grant stores the per-op manifest and digest, not only one combined digest. Deploy computes a typed old-to-new authority partial order: every expansion kills the grant, every provable narrowing may continue and is audit-noted, and incomparable changes kill. Golden canonical-encoding and mutation tests must flip the digest for each authority dimension independently.

**[proposed] Must not break.** Preserve SHA-256 domain separation, mandatory expiry, exact one-space grant binding, explicit op allowlists, and the rule that clients can only strengthen step-up.

**[proposed] Prior-ID status.** Y-005 is **open** and relocated here. The SHA-256 collision weakness is **closed**, but SHA-256 does not close incomplete semantics. SH3-002 remains **open** at the artifact boundary.

### [proposed] SH4-002 — Critical — atomic replay has no shared transaction or fencing generation

**[proposed] Exact claim.** Atomic `claim` closes only the original lookup race. The API calls the executor and only afterward calls `ReplayCache::put` (`crates/ordo-api/src/router.rs:438-457`). The replay trait has no database transaction argument and returns `Reserved` without an owner token (`crates/ordo-api/src/idempotency.rs:220-264`). Reservations expire after 60 seconds (`crates/ordo-api/src/idempotency.rs:124-134`), and `put` unconditionally replaces the current slot even if the original holder lost its lease (`crates/ordo-api/src/idempotency.rs:432-460`). The prose requirement that `put` commit in the domain transaction (`crates/ordo-api/src/idempotency.rs:86-90`) cannot be fulfilled through these interfaces.

**[proposed] Concrete attack.** A network adversary invokes a legitimate operation whose execution lasts longer than the reservation lease, then retries the same request. The second call removes or takes over the stale reservation and begins a second execution while the first still runs. The first caller later writes `Done` over the newer holder because no generation is checked. Separately, a worker crash after the domain commit and command append but before `put` leaves a stale reservation; the next retry executes the committed write again. Outbox intents, create-like side effects, and any write not protected by a coincidental unique key can duplicate.

**[proposed] Affected custody.** Every server-mediated mutation is affected in managed and BYO-storage spaces, and client-first control-plane or sync-acceptance operations are affected even if offline local edits do not use this route.

**[proposed] Pin.** Make replay ownership part of the write transaction. `claim` must return a random or monotonic reservation generation; the executor must receive that generation and a transaction context; domain mutation, chain append, permanent create binding, response material, and transition to `done` must commit atomically in the same space database. Every completion and abandon is compare-and-set on the generation. A live holder must renew its lease or use database transaction liveness, and takeover must fence the old holder before any second execution. If BYO storage prevents one transaction, specify a durable inbox/outbox state machine with exactly one side-effect committer and an explicit at-least-once contract; do not call it atomic replay.

**[proposed] Must not break.** Preserve `(space, principal, op, key)` scoping, canonical request fingerprints, permanent payload binding for creating ops, bounded waiter behavior, and encrypted cached responses.

**[proposed] Prior-ID status.** Z-012 is **relocated/open**. The concurrent lookup race is closed, but crash atomicity, live-lease takeover, and stale-winner fencing remain open. SH2-15 is therefore only partially closed.

### [proposed] SH4-003 — High — authenticator linking does not prove an old factor specifically authorized this link

**[proposed] Exact claim.** `LinkCeremony::from_claims` copies a general token `user_presence_at` and a caller-supplied `AuthenticatorId` (`crates/ordo-identity/src/directory.rs:154-194`). `Directory::link` checks only timestamp freshness and whether that named id is currently linked (`crates/ordo-identity/src/directory.rs:281-311`). No server challenge, old-authenticator assertion signature, credential-id binding, new-credential public key, or one-time ceremony nonce crosses this boundary. The audit event therefore records which old factor the caller named, not which factor cryptographically vouched.

**[proposed] Concrete attack.** An attacker steals a token from a device shortly after any presence-bearing login, learns the victim's existing credential id from the local profile or logs, and calls link within the five-minute window while presenting an attacker-controlled new authenticator id. The directory accepts the still-fresh timestamp as the old proof and converts transient session theft into a permanent login. On a stolen device with a client-first mirror, the permanent credential can then enroll another device and receive future key wraps.

**[proposed] Affected custody.** Identity is deployment-wide, so every custody mode is affected; client-first has the largest blast radius because account persistence can become durable key access.

**[proposed] Pin.** Make linking a server-minted, one-use ceremony that binds principal id, existing credential id, new credential public key or provider registration result, relying-party/client context, and expiry. Verify a fresh assertion from that exact existing credential over the ceremony challenge and proof of possession of the new credential; consume the nonce and insert the link atomically. Rotate refresh-token families, notify all existing factors, and issue a short rollback token outside the ordinary session. Recovery remains delayed and separate.

**[proposed] Must not break.** Preserve capability-token refusal, future-timestamp refusal, the zero-factor-only `linkFirst` exception, last-authenticator unlink refusal, audit attribution of both authenticators, and the separate recovery path.

**[proposed] Prior-ID status.** Z-006 is **relocated/open**. The ordinary-session-only path is narrowed, but the alleged dual proof is still a bearer claim plus a caller-named factor rather than two ceremony-bound proofs.

### [proposed] SH4-004 — High — live invalidation crosses spaces and outlives authorization

**[proposed] Exact claim.** The command store is mostly space-scoped, but the live bus is keyed only by op name: `InvalidationEvent` contains no space and `subscribe` accepts no space (`crates/ordo-api/src/spec.rs:271-280`; `crates/ordo-api/src/bus.rs:14-42`). The router checks access once, subscribes to `op.name`, and streams every later signal without rechecking membership, token status, or suspension (`crates/ordo-api/src/router.rs:680-739`). This breaks the stated suspension obligation to close live subscriptions and means command-log space scoping is not preserved downstream.

**[proposed] Concrete attack.** A malicious tenant subscribes to a common live op in space A and observes invalidation timing caused by writes in space B, revealing work patterns even though payloads are not pushed. A compromised member opens subscriptions before suspension; after the operator suspends and revokes them, those connections continue receiving activity signals indefinitely. A malicious tenant can also generate high-volume writes in one space and force invalidation/refetch churn for subscribers in every other space using the same op.

**[proposed] Affected custody.** All custody modes leak activity metadata and accept cross-space amplification. Client-first spaces may reveal sync cadence even though values stay sealed.

**[proposed] Pin.** Key publication and subscription by `(space, op, artifact version or dependency set)`, include the space in every event type, and reject any publish/subscribe mismatch structurally. Tie each subscription to session and membership revocation, cancel it on suspension, and periodically or eventfully revalidate long-lived authorization. Preserve a deployment-level operational metric separately; do not obtain it by multiplexing tenant streams.

**[proposed] Must not break.** Preserve subscribe-before-initial-read, refetch-through-scope rather than data push, lag coalescing, and heartbeat behavior.

**[proposed] Prior-ID status.** Z-005 is **relocated/open**: the ordinary door check is closed, but refresh/session/sync/live lifecycle remains incomplete and this live path demonstrably stays open. SH2-12 and the command-log space-scoping claim remain **open downstream**.

### [proposed] SH4-005 — Moderate — cursor authentication omits the space security context

**[proposed] Exact claim.** Page tokens are now HMAC-authenticated and bound to op and principal, but the HMAC input omits space (`crates/ordo-api/src/page.rs:75-98`). The router decodes with only op and principal (`crates/ordo-api/src/router.rs:655-675`) and mints the next token without space (`crates/ordo-api/src/router.rs:744-764`). The API design still documents an unsigned base64 JSON token (`docs/api-engine-design.md:352-364`). The payload is authenticated, not confidential: keyset values remain readable base64 JSON.

**[proposed] Concrete attack.** A principal belonging to two tenants can take a cursor minted from space A and submit it to the same op in space B. The valid MAC turns A's keyset values into an otherwise unforgeable seek position inside B, violating the claim that a cursor resumes one authorized result context. Query-form cursor support additionally copies those values into proxy logs, history, and referrers; a curious operator can read them even though they were described as opaque.

**[proposed] Affected custody.** All custody modes use the same transport. The issue is strongest where open sort keys themselves are sensitive metadata; sealed sort keys remain prohibited and must stay prohibited.

**[proposed] Pin.** Add the exact bound space and cursor-schema version to the HMAC transcript and to the encode/decode APIs. Reject a cursor before execution when any of principal, space, op, or version differs. Update the design formula to the implemented authenticated format and call the payload authenticated rather than confidential. Generated SDKs must use the header carrier; query support should be explicitly unsafe for sensitive sort keys or removed.

**[proposed] Must not break.** Preserve constant-time MAC comparison, principal and op binding, independent `version_skew`, stateless resumption, implicit id tie-breaks, and sealed-sort prohibition.

**[proposed] Prior-ID status.** The cursor-forgery part of SH-6 is **closed**. Context binding is **relocated/open** here.

### [proposed] SH4-006 — High — `SealedRef` is a convention at append, not an enforced redaction type

**[proposed] Exact claim.** The command-log documentation says the redaction rule “lives in the type,” but an `Image` is only `BTreeMap<String, Value>` and any field can carry `Value::Text` (`crates/ordo-commandlog/src/value.rs:7-57`). `Command::validate` checks only the before/after presence shape, and `CommandGroup::validate` checks only nonempty/single-space structure (`crates/ordo-commandlog/src/command.rs:91-142`). The store receives no schema or field-sensitivity manifest before permanently hashing and retaining the image.

**[proposed] Concrete attack.** A compromised executor or malicious package constructs a command for a sealed field using `Value::Text(plaintext)` instead of `Value::SealedRef(handle)`. The store accepts it, hashes it, checkpoints it, archives it, and makes it part of every backup. Destroying the space key no longer erases that value. A similar bug can reuse handles and silently restore the equality index that random handles were intended to remove.

**[proposed] Affected custody.** Managed and BYO-storage have direct plaintext risk because the executor sees values. Client-first cannot have an honest server redact values it never sees, so its proposal format must structurally prohibit plaintext sealed values before acceptance; a malicious member can still choose to disclose their own plaintext, but must not cause another member's ciphertext to be logged as plaintext.

**[proposed] Pin.** Introduce an artifact-bound redaction boundary that converts a schema-aware execution image into a private-constructor `LogImage`. For every sealed field, the boundary stores only a freshly CSPRNG-minted, payload-store-resolved opaque handle; open fields use the ordinary variants. `Store::append` accepts only validated groups carrying the artifact/schema version used for redaction. Add negative tests proving plaintext, ciphertext, deterministic handles, missing sealed-field metadata, and handle reuse cannot cross append.

**[proposed] Must not break.** Preserve fresh random non-content-derived handles, canonical encoding, per-space hash chains, full open-field before/after images, and crypto-erasure semantics.

**[proposed] Prior-ID status.** Y-012 is **relocated/open**. Deterministic ciphertext hashing is closed in the representation, but representation alone does not enforce that sealed fields use it.

### [proposed] SH4-007 — Moderate — the “loud” cross-space revert error is a tenant existence oracle

**[proposed] Exact claim.** `Store::find` now requires a space, but the same public storage contract adds `is_logged_elsewhere`, which scans every other tenant (`crates/ordo-commandlog/src/store.rs:63-80`, `crates/ordo-commandlog/src/store.rs:133-145`). `revert_group` returns `CrossSpace` when the correlation exists elsewhere and `UnknownGroup` otherwise (`crates/ordo-commandlog/src/store.rs:258-323`). The comment calls the disclosure bounded, but a scoped lookup tells a caller only “not here”; it does not tell them “present in some other tenant.”

**[proposed] Concrete attack.** A malicious tenant obtains correlation ids from support tickets, URLs, shared traces, timestamp-derived ids, or a compromised client log and submits them to revert in its own space. `CrossSpace` confirms that another tenant has a command group with that id; `UnknownGroup` confirms that none does. Repeated probes correlate another tenant's activity without exposing its images.

**[proposed] Affected custody.** Every custody mode shares command identifiers and revert semantics; the leak is control-plane metadata rather than value plaintext.

**[proposed] Pin.** Remove `is_logged_elsewhere` from the tenant-facing store/revert path and return one scoped `UnknownGroup` result. If cross-space probing should alert, perform a separately privileged existence check after forming the uniform response and write only a deployment security event unavailable to the caller. Rate-limit and attribute the probe.

**[proposed] Must not break.** Preserve mandatory space on `find`, per-space correlation uniqueness, single-space groups, reverse-order inverse planning, and the rule that no other tenant's images leave the store.

**[proposed] Prior-ID status.** Y-032 is **relocated/open**. Unscoped image retrieval is closed; cross-space existence disclosure was introduced by the proposed remedy.

### [proposed] SH4-008 — High — genesis import has a writer-selectable trigger bypass without a pinned authority ceremony

**[proposed] Exact claim.** `migration.md:216-287` gives import a source actor and says batch envelopes carry an import flag that suppresses listener triggers, but it does not define who may enter import mode, the intrinsic step-up requirement, a one-time import session, whether ordinary guards run, or a server-only state that prevents an arbitrary source batch from setting the quiet flag. RLS and source policies are only guard suggestions (`migration.md:120-128`, `migration.md:149-150`).

**[proposed] Concrete attack.** A stolen import credential, compromised migration worker, or malicious mapping invokes ordinary writes as `source:import-*` with the quiet flag after the intended genesis window. Listener-enforced normalization, outbox intents, alarms, and derived maintenance do not run. If guards are bypassed, the source writes rows outside its normal authority; if guards are not bypassed, the documented import may be impossible. Either interpretation gives an attacker an unreviewed security branch.

**[proposed] Affected custody.** Managed and BYO-storage imports run at the server/database boundary. Client-first import runs on device and therefore needs the same signed session and proposal restrictions when batches sync.

**[proposed] Pin.** Define an intrinsic step-up, owner/operator-authorized `std.space.import.begin` that mints a single-use import session bound to space, actor, mapping-file hash, source snapshot/hash, custody mode, permitted entities/fields, batch limits, and expiry. Only the executor may derive the quiet bit from a live session while the space is in a non-callable import state; callers never send it. The artifact must state which guards/checks still run and which listener effects are replaced by one explicit baseline run. End/abort consumes the session and checkpoints the exact mapping and verification hashes.

**[proposed] Must not break.** Preserve attributed batch groups, bounded batching, commit-driven triggers, explicit `$trigger.ids` provenance, signed genesis checkpoint, client-first on-device sealing, and the ability to rehearse safely.

**[proposed] Prior-ID status.** SH2-4 remains **open**. Y-004's ordinary listener confused-deputy path is closed, but import introduces a separate privileged provenance class that has not been ruled.

### [proposed] SH4-009 — High — import rerun rollback can delete a row the import did not create

**[proposed] Exact claim.** The migration design makes reruns converge by external-id unique keys (`migration.md:248-260`) but says the inverse deletes by the batch's minted-id set (`migration.md:261-263`). `bootstrap.md:400-404` correctly notes that these properties conflict: a rerun can converge onto a pre-existing row and a later abort can delete it. A plain old-id/new-id table also does not record whether the batch created, changed, or merely found each row.

**[proposed] Concrete attack.** A crash causes an import to restart, or a malicious mapping deliberately collides with a valuable existing external id. The rerun records or reuses that row's id. Abort then applies delete-by-id and removes the pre-existing row, including any edits made after the first attempt. The action is logged as a revert even though it is not the mathematical inverse of what that attempt did.

**[proposed] Affected custody.** All custody modes are affected; client-first merely moves execution to a device and does not fix inverse semantics.

**[proposed] Pin.** In the same transaction as each row mutation, persist a per-import-session disposition ledger: `created(id, resulting_rev)`, `updated(id, before_image, resulting_rev)`, or `unchanged(id, observed_rev)`. Abort deletes only `created` rows at the exact resulting revision, restores only `updated` rows by CAS from the recorded revision, and does nothing to `unchanged`; any intervening edit produces a conflict requiring owner choice. Seal or minimize legacy identifiers and apply an explicit retention/deletion policy to the mapping table.

**[proposed] Must not break.** Preserve server-minted ids, idempotent reruns, batch-level audit attribution, reverse batch order, and the rule that rollback is new history rather than history rewriting.

**[proposed] Prior-ID status.** Z-014 is **open**. The bootstrap document relocated the contradiction accurately but did not close it.

### [proposed] SH4-010 — High — the glossary still instructs the unsafe per-epoch blind-index design it says was withdrawn

**[proposed] Exact claim.** `glossary.md:398-406` correctly says blind-index keys derive from the space master key and do not change per DEK epoch. The same entry then says at `glossary.md:407-415` that blind-index keys derive per epoch, that cross-epoch uniqueness remains open, and that no answer is settled. `implementation.md:454-480` and `definition-language.md:498-503` use the stable SMK-derived BIK. The security rule is therefore both true and false in one glossary entry.

**[proposed] Concrete attack.** A supply-chain implementer or migration author follows the later paragraph, emits a new blind-index key per DEK epoch, and permits old and new index values concurrently. During mandatory rotation, the same canonical plaintext hashes two ways; `sealed indexed unique` admits duplicates, and converge-or-create creates instead of converging. A malicious tenant can time duplicate registration attempts during rotation to exploit the split namespace.

**[proposed] Affected custody.** Managed and BYO-storage enforce the index server-side. Client-first clients compute and submit indexes and therefore need the same stable derivation and version conformance to avoid divergent uniqueness decisions.

**[proposed] Pin.** Delete the stale “two rotation gaps” paragraph and make the glossary point only to the settled derivation `BIK = KDF(SMK, "blind-index", entity, field)`. Add a conformance test that rotates DEK epochs, writes equal canonical values on both sides of the rotation, and requires one `(space, hmac)` uniqueness conflict. Treat SMK/BIK rotation as a separate offline or dual-read migration with writes to unique blind-index fields blocked until one index namespace is authoritative.

**[proposed] Must not break.** Preserve per-entity/per-field domain separation, canonical value bytes, uniqueness scoped by space, the explicit loss of forward secrecy until SMK rotation, and the separately disclosed nullness leakage of the null sentinel.

**[proposed] Prior-ID status.** Y-009 is **closed in the normative implementation rule but relocated/open as a contradictory public instruction**. It is not safe to call the finding closed while the glossary reopens it verbatim.

### [proposed] SH4-011 — High — signed-head and exact read-position claims have no API or snapshot carrier

**[proposed] Exact claim.** The design says every read result carries the exact per-space command-log position and a signed head at that position (`docs/implementation.md:905-918`; `docs/security-logging-design.md:497-510`; `docs/bootstrap.md:242`). `ExecResult` contains only rows and an optional cursor (`crates/ordo-api/src/spec.rs:185-189`), and the success envelope contains only op, version, mode, rows, and next cursor (`crates/ordo-api/src/router.rs:744-766`). The isolation table leaves reads at READ COMMITTED (`docs/implementation.md:202-234`), which does not hold one snapshot across a multi-statement read.

**[proposed] Concrete attack.** An honest concurrent write can land between two statements of a composed read, so there is no single exact command-log position that describes all returned rows. A compromised worker can attach a cached valid signed head to stale or mixed-snapshot rows because the executor returns no transaction-derived position and the signature is not coupled at this boundary. Clients then advance a multi-query screen or local mirror believing the result is one snapshot, defeating the consistency and one-request equivocation-detection claims.

**[proposed] Affected custody.** Managed and BYO-storage reads need database snapshot coupling. Client-first mirrors need a signed feed/group high-water mark rather than a server plaintext snapshot, but the same envelope must distinguish those proof types.

**[proposed] Pin.** Extend the executor result with a typed snapshot proof containing space, chain position, head hash, artifact version, and proof kind. A single-statement read may acquire data and head from one database snapshot; every multi-statement read must use one REPEATABLE READ/exported snapshot or be rejected as non-positionable. The signing service signs the exact `(space, position, head, artifact version)` tuple obtained from that transaction, and the router cannot substitute another value. Client-first responses carry the signed accepted-group high-water mark and continuity proof. Add concurrent-write tests for composed reads and negative tests for head/result substitution.

**[proposed] Must not break.** Preserve replica eligibility where a replica can prove its applied command position, per-space chains, cached signatures per unchanged head, client continuity checks, and the separation between optional external witnesses and always-on client pinning.

**[proposed] Prior-ID status.** Z-013 is **open**. The new attestation design increases the consequence of the old snapshot gap; it does not close it.

### [proposed] SH4-012 — High — deployment-scoped bootstrap is unrepresentable and its creator identity is contradictory

**[proposed] Exact claim.** `bootstrap.md:42-61` makes `std.space.create` the one op with no bound space, but every current API call requires a space before access and execution (`crates/ordo-api/src/router.rs:290-341`), `AccessChecker` always takes a space (`crates/ordo-api/src/spec.rs:253-268`), and grants are structurally one-space (`crates/ordo-identity/src/principal.rs:306-363`). `bootstrap.md:627-632` itself leaves deployment scope open. The genesis description also attributes operator creation to `user:<operator>` (`bootstrap.md:165-175`, `bootstrap.md:332-338`) while claiming the attributed principal's members row is the owner and therefore receives creator ACL grants (`bootstrap.md:183-191`); those are different humans on the operator path.

**[proposed] Concrete attack.** An implementation that invents a dummy `$space` makes membership or a grant in that dummy tenant the authorization root for creating every tenant. An implementation that reuses an arbitrary existing space lets its owner reach a deployment operation. If the ordinary creator rule is applied during operator provisioning, the operator rather than the intended owner becomes the semantic creator of bootstrap ACL rows, or the creates fail because the operator has no membership in the new space. A special case that silently retargets grants to the owner breaks the claimed universal creator rule unless it is explicit and artifact-tested.

**[proposed] Affected custody.** Every space begins through this boundary; BYO credentials and client-first custody selection make bootstrap especially sensitive.

**[proposed] Pin.** Add a closed `OpScope = Deployment | Space` to the artifact, router, access engine, replay key, audit record, and executor. Deployment capabilities are a distinct issuer audience and grant type, never a missing or wildcard space, and cannot satisfy space-scoped checks. Define `std.space.create` and `std.space.reap` as immutable platform ops with intrinsic step-up. In the genesis group, separate `actor` from `beneficiary`: attribution remains the real operator/agent, while the only sanctioned bootstrap ACL rule explicitly grants the intended owner. Prove the registry row, owner membership, profile, domain roots, ACL rows, key epoch, and genesis head become callable only after all durable phases complete.

**[proposed] Must not break.** Preserve mandatory exact-space binding for every ordinary op, no wildcard capability grants, non-callable provisioning state, no outbound sinks at bootstrap, one initial owner, honest actor attribution, and resumable cross-system cleanup.

**[proposed] Prior-ID status.** Z-020 is **open**. Y-002, Y-015, and Y-034 are **closed for ordinary space-scoped operations** and must not be weakened to make bootstrap fit; the missing scope is a new distinct type, not an exception to those pins.

### [proposed] SH4-013 — Moderate — unauthenticated and unauthorized callers can enumerate registered op names before the door

**[proposed] Exact claim.** Unknown names return `access_denied`, but registry lookup occurs before token validation (`crates/ordo-api/src/router.rs:120-140`). For a registered op, method mismatch and missing space are returned before access (`crates/ordo-api/src/router.rs:220-250`); for an unknown name, the same requests return `access_denied`. The code acknowledges the distinguishers but calls them authenticated probes (`crates/ordo-api/src/router.rs:253-274`), even though no bearer is checked on either early path.

**[proposed] Concrete attack.** A malicious guest or network adversary sends a candidate name with each HTTP method and no space. A 405 with an `Allow` header or a 400 `missing_space` confirms the op exists and reveals its kind; 403 means the name is absent. Enumeration identifies high-value export, delete, break-glass, migration, and domain-specific operations for later credential theft or social engineering.

**[proposed] Affected custody.** The registry is deployment-wide, so the leak spans all custody modes even without a valid space.

**[proposed] Pin.** Parse only the generic bearer and space envelope before lookup. Any request without a valid bearer receives the same `access_denied` independent of name, method, or space. For a bearer, unknown and unauthorized names remain identical; method, parameter, version, and live-capability errors become visible only after the token is authorized for that op in the bound scope. Define one generic missing/invalid-space result that does not require an `OpSpec`, and rate-limit deployment-wide probes.

**[proposed] Must not break.** Preserve the one generated route, method-kind mirroring after authorization, no default space, no version header before authorization, and uniform unknown-versus-forbidden responses.

**[proposed] Prior-ID status.** SH2-13 is **open**. The direct unknown-op response was closed, but the method and missing-space side channels remain and are unauthenticated.

## [proposed] Prior-pin and claimed-closure ledger

- **[proposed] Z-001 — closed as a disclosed product property, not as cryptography.** Client-first members mutually hold the space key; hostile-member row confidentiality is not promised. The consent language must remain explicit and no server-enforcement claim may creep back in.

- **[proposed] Z-002 — closed as a disclosed product property, not as server validation.** A blind server cannot validate plaintext guards, checks, CAS truth, or history truth; the current detected-and-revertible/honest-device boundary is the product contract.

- **[proposed] Z-003 — closed.** Op-intrinsic step-up is combined with client additions by strongest requirement in `ordo-identity`; a permissive client cannot subtract it. The stale “client-level” label in `bootstrap.md:252` should be corrected without changing the rule.

- **[proposed] Z-004 — closed as the stated owner contract.** An owner can reach every row in the space and that is disclosed as what ownership means; cross-space isolation, command logging, and consent remain mandatory.

- **[proposed] Z-005 — relocated/open at SH4-004.** Door denial is implemented, but refresh-family revocation, live cancellation, pending-sync refusal, and key-epoch rotation are not one enforced suspension transaction.

- **[proposed] Z-006 — relocated/open at SH4-003.** Freshness and factor existence are checked, but a ceremony-specific signed assertion is absent.

- **[proposed] Y-001 — closed at the architecture pin, pending implementation conformance.** Egress now requires scheme/port policy, post-resolution IP filtering, redirect revalidation, DNS-rebinding defense, credential isolation, owner consent, and signed delivery. No production sink exists in the reviewed crates, so release tests must still prove the pin.

- **[proposed] Y-002 — closed for space-scoped grants.** `CapabilityGrant` requires one space and the access engine checks it first. Deployment scope must be added as a separate type under SH4-012, never as a wildcard.

- **[proposed] Y-003 — closed.** G10a is the narrow self-rooting create exception and dependent writes remain guarded. Bootstrap's actor/beneficiary mismatch is a separate SH4-012 issue.

- **[proposed] Y-004 — closed for ordinary routines.** Clock routines pass guards; listener routines are restricted to rows reachable from `$trigger.ids`; wider access requires manifest-visible `unguarded`. Import mode remains separately open at SH4-008.

- **[proposed] Y-005 — open at SH4-001.** Row-scope pinning fixed the guard-only counterexample but not the complete authority lattice.

- **[proposed] Y-009 — relocated/open at SH4-010.** The SMK-derived BIK is settled in the implementation document; the glossary immediately contradicts it.

- **[proposed] Y-012 — relocated/open at SH4-006.** Fresh random `SealedRef` handles close deterministic equality only when the append boundary enforces their use for sealed fields.

- **[proposed] Y-013 — closed.** Direct and group roles combine by maximum level, operation sets union, and per-member deny applies last; tests cover group resolution and suspension.

- **[proposed] Y-015 — closed for space-scoped execution.** `OpExecutor::execute` receives the exact approved space as a distinct argument and reserved `$space` parameters are rejected.

- **[proposed] Y-029 — closed.** The definition language, glossary, modeling guidance, migration guidance, and traps now agree on clock-versus-listener provenance.

- **[proposed] Y-032 — relocated/open at SH4-007.** Images and normal find are scoped, but `is_logged_elsewhere` exposes cross-tenant existence.

- **[proposed] Y-033 — closed.** `FieldSpec` now carries sealed/refinable metadata; sealed refinement defaults off, permits equality only when opted in, and is refused for aggregate-only ops. Do not remove these per-field checks.

- **[proposed] Y-034 — closed.** API and identity tests now exercise bound-space disagreement and cross-space grant denial. The ordinary executor path also carries the bound space.

- **[proposed] Z-012 — relocated/open at SH4-002.** Atomic reservation is real; transaction atomicity and fencing are not.

- **[proposed] Z-015 — closed.** `docs/licensing.md:73-113` now withdraws categorical modified-internal-use advice, makes the answer fact-dependent, recommends counsel, and distinguishes source offers to actual remote users from publication to the world. Keep the “not legal advice” and operative-license-file precedence.

## [proposed] Disagreements with current reasoning

- **[proposed] A loud cross-space revert response is not a security control.** Alerting can be loud internally while the tenant response remains indistinguishable; SH4-007 rejects the current claim that a caller already learned “exists elsewhere” merely by naming an id.

- **[proposed] A MAC does not make cursor contents opaque.** It makes them authentic. The docs and SDK guidance must not imply confidentiality while the JSON keyset is readable and query transport remains supported.

- **[proposed] A fresh timestamp in a bearer token is not dual proof.** It is reusable step-up authority inside a window. Authenticator enrollment needs a transaction-specific challenge signed by the old factor and bound to the new factor.

- **[proposed] A hash primitive is not an authority schema.** SHA-256 closes collisions only after the exact security-relevant byte inventory and its comparison order are pinned.

- **[proposed] “Same transaction later” cannot repair an interface that has no transaction carrier.** The replay and executor traits must change before the Postgres implementation can satisfy their prose.

- **[proposed] Product-scope rulings should remain rulings.** Z-001, Z-002, and Z-004 should not be relitigated as hidden cryptographic promises; the hard requirement is that consent, generated UI, export, and operator documentation never imply stronger protection.

## [proposed] Strengths not to break

- **[proposed] Keep exact space as a structural argument.** Grants, access checks, replay keys, command groups, and executors now carry the space explicitly for ordinary operations, with cross-space denial tests.

- **[proposed] Keep op-intrinsic step-up and non-delegable presence.** Client configuration can add or tighten, never exempt, and future timestamps are refused.

- **[proposed] Keep the honest client-first boundary.** Mutual member trust, server blindness, detected-and-revertible violations, and rejected proposals as history are substantially safer than pretending the server can referee plaintext it cannot see.

- **[proposed] Keep G10a narrow and trigger provenance explicit.** Self-rooting create and `$trigger.ids` are specific, auditable exceptions rather than ambient bypasses.

- **[proposed] Keep per-field sealed refinement enforcement.** Default-off opt-in, equality-only blind-index use, and no sealed aggregate oracle are the right shape.

- **[proposed] Keep random opaque command-log handles.** Never return to deterministic ciphertext digests or value-derived references; add the append enforcement in SH4-006 around this representation.

- **[proposed] Keep canonical, domain-separated SHA-256 and constant-time HMAC verification.** The cryptographic primitives are no longer the weak part; context completeness is.

- **[proposed] Keep per-space single-chain groups and mandatory scoped lookup.** Remove only the global existence oracle, not the single-space structural checks.

- **[proposed] Keep replay key isolation and permanent create bindings.** SH4-002 adds transaction ownership and fencing; it does not undo space/principal/op scoping or payload mismatch behavior.

- **[proposed] Keep the corrected licensing posture and egress/SSRF pin.** Both now state uncertainty and boundary conditions more honestly than their earlier categorical forms.

## [proposed] Acceptance gates

- **[proposed] Gate A — artifact authority mutation suite.** Every read, write, guard, field, trigger, key release, effect, egress, step-up, and cost expansion must produce an incomparable-or-expanded manifest and kill an existing grant.

- **[proposed] Gate B — replay crash and lease suite.** Kill the worker before and after each domain write, chain append, response serialization, and replay transition; force operations past the lease; prove one committed group and one replay response with generation fencing.

- **[proposed] Gate C — identity ceremony suite.** A stolen ordinary token, a presence-bearing token from another ceremony, a caller-named old credential, a replayed challenge, a capability token, and an unproved new credential must all fail linking.

- **[proposed] Gate D — tenant-isolation suite.** Publish invalidations, present cursors, probe correlations, and request op names across two spaces with the same and different principals; assert no cross-space event, seek context, existence bit, or pre-door enumeration escapes.

- **[proposed] Gate E — sealed-log suite.** Attempt every `Value` variant for every sealed field through a compromised-executor test double; only a fresh opaque handle may reach canonical log bytes, archives, checkpoints, or replay responses.

- **[proposed] Gate F — migration fault suite.** Crash, retry, collide external ids, edit rows between attempts, abort, and resume across every batch boundary; prove the disposition ledger's inverse never deletes or restores a row the attempt did not own.

- **[proposed] Gate G — snapshot/attestation suite.** Inject concurrent commits between every read statement and try to substitute valid heads from earlier and later positions; either one exact proof covers the result or the operation fails as non-positionable.

- **[proposed] Gate H — bootstrap scope suite.** Prove that no space token, owner, agent, missing-space request, or dummy space can satisfy a deployment capability, while the real actor remains distinct from the sole initial owner's ACL benefit.
