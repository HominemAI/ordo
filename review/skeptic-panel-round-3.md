# Ordo cold skeptic panel — round 3

Date: 2026-08-03  
Method: external scars first, then a cold read in the requested order. The panelists are fictional composites, not claims about the named authors or commenters in the bibliography. Their formative incidents are grounded in the linked public postmortems and practitioner threads.

## Bottom line

No panelist would run a production pilot today. That is mostly a product-stage verdict, not a rejection of the design: the repository says plainly that nothing is installable, there is no planner or executor, and authorization is not implemented. Two panelists would fund or join a design-partner exercise; three would not spend team time until there is a runnable artifact.

The design itself survives more abuse than the ordinary “new BaaS” pitch. In particular, registered operations, derived and inspectable scope, explicit cost classes, narrow write modes, the command-group model, and unusually candid exit/custody limitations all earned reluctant approval.

Three actual design/document defects remain serious enough to block trust even after discounting the pre-alpha status:

1. **Client-first acceptance is circular or underspecified.** The server sequences an unreadable proposal before the only party able to evaluate CAS has replayed it, yet a rejected command is said never to become history. The protocol lacks a precise accept/sequence/validate/commit state machine and failure recovery.
2. **The licensing FAQ gives unsafe AGPL guidance.** It says a modified Ordo used internally owes nothing because internal use is not distribution. AGPL §13 is triggered by remote network interaction, not distribution; the FSF says modified network software must offer source to all users interacting with it. Internal employees using the modified server may therefore be users entitled to an offer. This needs counsel, not confident prose.
3. **The operational spine is still a proposal plus promises.** Per-space write serialization is acknowledged but unmeasured; restore, migration recovery, observability, device enrollment, and local-store rules are partly `[proposed]`; no restore drill or benchmark exists. That is better documentation, not yet evidence.

## External scar map used to construct the panel

| Ecosystem | Documented incident or practitioner critique | Scar carried into the review |
|---|---|---|
| Supabase | [Supabase incident on February 12, 2026](https://supabase.com/blog/supabase-incident-on-february-12-2026), 2026-02-13 | A monitoring deployment enabled a region-wide network block; pre-production lacked regional parity; detection and escalation were slow; every service dependent on that regional database was unavailable for 3h42m. “Postgres underneath” does not remove platform-level correlated failure. |
| Supabase | [Ask HN: Supabase PG upgrade wiped production DB, PITR backups failing](https://news.ycombinator.com/item?id=47394240), 2026 | A practitioner report of production data loss, disk exhaustion, failing recovery, and poor support response. The specific claims are user-reported rather than independently adjudicated, so the scar is about the need for customer-executable restore evidence, not a finding that every claim was proven. |
| Firebase | [Fighting vendor lock-in and designing testable serverless apps](https://news.ycombinator.com/item?id=19727218), 2019-04-23 | The linked practitioner account reports a bill moving from roughly $25 toward $2,000 after a usage-reporting change without an application-usage change; the HN argument also captures the counter-scar that portability layers can become expensive “fake work.” |
| Firebase | [Firebase bill is usually $50, but I was surprised to see a $70k bill in one day](https://news.ycombinator.com/item?id=42732714), 2025-01 | Practitioner discussion of catastrophic bill amplification and the interaction between public access, abuse, reads, and cost. The durable lesson is that cost limits and access control are one failure domain. |
| Firebase | [Cloud Firestore pricing](https://cloud.google.com/firestore/pricing), current documentation accessed 2026-08-03 | First-party documentation confirms per-operation charging and that security-rule `exists()`, `get()`, and `getAfter()` can add billable reads. A permission design can have a runtime-cost consequence invisible at the call site. |
| Cloudflare | [Cloudflare service outage June 12, 2025](https://blog.cloudflare.com/cloudflare-service-outage-june-12-2025/), 2025-06-12 | A third-party storage failure took Workers KV down and cascaded across Access, WARP, Workers AI, Turnstile, Pages builds, and other products for 2h28m. Cloudflare explicitly accepted responsibility for the chosen dependency and broad blast radius. |
| Cloudflare | [Code Orange: Fail Small](https://blog.cloudflare.com/fail-small-resilience-plan/), 2025-12-19 | After major November and December 2025 incidents, Cloudflare made failure isolation an explicit resilience program. “One integrated platform” is both convenience and correlated-failure surface. |
| Convex | [How Convex Took Down T3 Chat: June 1, 2025 Postmortem](https://news.convex.dev/how-convex-took-down-t3-chat-june-1-2025-postmortem/), 2025-06-03 | Convex publicly accepted fault for an outage caused by its reactive database machinery. The scar is that derived reactivity and subscriptions are operational mechanisms, not free semantic sugar. |
| Convex | [How Convex Works — Hacker News](https://news.ycombinator.com/item?id=40020516), 2024-04 | Practitioners and Convex staff identify the narrower fit: JavaScript/TypeScript backend, online workloads, a less integrated auth story than some BaaS competitors, and specialized workloads living elsewhere. A coherent abstraction can still be the wrong boundary. |
| Local-first / CRDT | [Local-First Software: You Own Your Data, in spite of the Cloud](https://www.inkandswitch.com/local-first/static/local-first.pdf), 2019 | The foundational paper explicitly separated CRDT merge from the remaining production system and described Automerge production use as immature at the time. “Local-first” is a product architecture, not a library feature. |
| Local-first / CRDT | [An Interactive Intro to CRDTs — Hacker News](https://news.ycombinator.com/item?id=47237460), 2026 | Direct practitioner discussion emphasizes that complex application semantics remain hard even when replicas converge; calendar recurrence and authority are examples where merge mathematics does not choose correct product meaning. |
| Yjs | [Yjs documentation](https://docs.yjs.dev/), accessed 2026-08-03 | First-party docs call Yjs modular and network-agnostic and say scaling collaborative-editing backends is nontrivial. Persistence, auth, transport, and operations remain the adopter’s system. |

## Persona 1 — Rina Solberg, the BaaS escape artist

**Stack history.** PHP/MySQL monoliths → Rails/Postgres → Firebase Realtime Database → Supabase/Postgres with Auth, Storage, Realtime, and Edge Functions → a deliberately boring managed Postgres plus a small API. She dislikes both stored-procedure archaeology and “database plus twelve magic sidecars.”

**Formative disaster.** Her composite scar is the failure shape in Supabase’s February 2026 postmortem: one auxiliary monitoring deployment changed a region-scoped AWS control, the staging environment lacked production parity, and the database, Auth, APIs, Storage, Realtime, and Functions failed together for 3h42m. She also keeps the Ask HN data-loss/PITR report pinned, not as a proven universal indictment, but because provider-owned recovery that customers cannot rehearse is not recovery she will promise to a board.

**What would actually convince her.** A runnable self-hosted release; an operator-run restore drill from documented backups; a destructive-migration kill test; a one-command export imported into plain Postgres; and a failure-domain diagram that states which Ordo features die when Postgres, object storage, KMS, identity email, or the job runner dies.

**First impression.** “This is the first BaaS manifesto I’ve read that knows a database platform is mostly an incident-response product. Unfortunately it is still a manifesto.”

**Discord/HN-style reaction, verbatim.**

> lol, so the cure for Supabase is a brand-new language, a brand-new compiler, a brand-new auth model, a brand-new audit chain, a brand-new sync protocol, and a brand-new hosted company? The docs are excellent because docs are currently 90% of the product. “One binary plus Postgres” is the nicest sentence here; “client-first, generated UI, routines, feeds, custody conversion” is how it becomes twelve products again. Also AGPL plus “hosted runs ahead” means I get the reassuring source code after customers already depended on the unreleased behavior. **[Unfair parts: Ordo explicitly scopes the engine as whole-engine AGPL, exposes plain outputs, and says the public release lag is cadence rather than permanent feature withholding. A pre-alpha cannot yet demonstrate the feared permanent divergence.]**

**Top three walk-away reasons.**

1. Nothing can be installed, and the end-to-end authorization claim is not present in the parser or executor. This is product-stage skepticism.
2. Backup/restore and migration recovery are marked `[proposed]`; no restore has been timed. This is an evidence deficit with design consequences.
3. A single Postgres dependency carries registry, jobs, invalidation, idempotency, audit chain, and customer data. The topology is admirably simple, but its degradation modes have not been exercised. This is product skepticism until fault-injection evidence exists, not automatically a defect.

**Top three grudging compliments.**

1. The repository does not pretend “Postgres underneath” means ordinary Postgres operations are sufficient; it calls out chain continuity, key-provider separation, migration soak windows, and artifact/schema mismatch after restore.
2. Exported SQL/DDL/data and Apache-licensed specs/corpora are a materially better exit posture than proprietary data models.
3. The explicit distinction between a guest window and a consented copy is more honest than most sharing systems’ hand-waving about revocation.

**Verdict.** **No production pilot; yes to a two-week paper/design-partner review.**

**Exact evidence that flips it.** A tagged release she can deploy without vendor assistance, plus a published transcript of: start with a populated guarded app; kill a transforming migration at 60%; recover; delete the primary; restore from base backup + WAL and object-store versions; verify the chain discontinuity; run conformance; export; load the export into stock Postgres. Include wall times, data hashes, commands, and all manual steps.

## Persona 2 — Malik Chen, the bill-shock mobile lead

**Stack history.** Objective-C/Core Data → Android Room → Firebase Auth/Firestore/Functions → Cloud Run/Postgres → SQLite-first mobile apps with a narrow sync API. He hates hand-built synchronization and hates per-read billing that turns a render loop into a financial event.

**Formative disaster.** The Firebase practitioner record gives him two versions of the same lesson: a reported cost jump from about $25 toward $2,000 after a usage-accounting change, and the later $70k-in-a-day incident discussion. Google’s own Firestore pricing docs confirm that security-rule lookups may add reads. His scar is not “usage billing is evil”; it is “authorization, subscriptions, retries, and UI bugs can multiply the meter outside the line of code the developer is reviewing.”

**What would actually convince him.** Enforced per-space budgets, hard workload ceilings, cost estimates in `ordo plan`, a chaos case showing an abusive client cannot create an unbounded bill, and measured sync/mirror behavior on real iOS/Android devices under airplane mode, clock skew, low storage, and process death.

**First impression.** “I like that cost class is part of the type of an operation. I do not like that infrastructure cost is nowhere close to being part of the type of a deployment.”

**Discord/HN-style reaction, verbatim.**

> Closed grammar for LLMs is Firebase Rules with better typography. You’re still asking me to believe the compiler catches the expensive thing, except this time the expensive thing can be post-decrypt host scans, live invalidations, routine retries, mirror sync, feed copies, key rotation, and a serialized chain append. A four-name cost lattice is not a bill cap. And “SDK fills `$today`” is exactly the kind of client convention that becomes a security boundary by accident. **[Unfair parts: the docs explicitly refuse to treat the caller-supplied date as audit time, distinguish registered from effective cost after refinement, and reject unbounded host work unless acknowledged.]**

**Top three walk-away reasons.**

1. Cost classes describe computational venue/bounds, not dollars, queue amplification, storage growth, outbound traffic, or retry multiplication. Actual design/product gap.
2. The local mirror is security- and correctness-critical but its storage rules are only proposed and no mobile SDK exists. Product-stage skepticism.
3. Client-first conflict behavior is rejection and replay, but the acceptance state machine is not coherent enough to implement from the prose. Actual design defect.

**Top three grudging compliments.**

1. Recomputing effective cost after call-time refinement closes a real “cheap registered op, expensive request” hole.
2. `unbounded` being explicit and compositional is a better failure mode than a surprise Firestore invoice.
3. The docs plainly state that an offline revoked device keeps cached data and that mirror scope is a copy/consent decision.

**Verdict.** **No pilot.** He will not put the sync path on user devices until it exists as a testable protocol and SDK.

**Exact evidence that flips it.** A public mobile test matrix with at least two devices editing the same and different rows offline; deterministic transcripts for every accept, CAS miss, retry, process death, key rotation, and replay; maximum on-device and server storage growth; network bytes; queue depth; and a hard quota demonstration where a malicious subscription/routine/feed loop stops safely instead of merely alerting.

## Persona 3 — Ivo Petrov, the platform consolidation survivor

**Stack history.** Bare metal/Postgres/HAProxy → Kubernetes + Kafka + Vault + Prometheus → Cloudflare Workers/KV/R2/Durable Objects/Access → a smaller Kubernetes platform with explicit dependency ownership. He hates snowflake servers and hates “integrated developer platforms” whose products secretly share a control plane.

**Formative disaster.** Cloudflare’s June 12, 2025 postmortem is his canonical diagram: Workers KV’s central backing-store failure cascaded into identity logins, WARP, Workers AI, Pages builds, Turnstile-related services, and dashboard components. Cloudflare’s later “Fail Small” program reinforces the lesson that common building blocks can broaden blast radius unless failure isolation is a first-class property.

**What would actually convince him.** A dependency graph, SLOs per subsystem, degraded-mode behavior, multi-node correctness tests, operational dashboards, and evidence that one hot tenant cannot monopolize chain locks, worker queues, connection pools, or KMS capacity.

**First impression.** “Postgres as the only coordinator is a strong simplification. It is also the place I draw a red circle and write ‘everything queues here.’”

**Discord/HN-style reaction, verbatim.**

> Every platform says “no Redis, no broker, no coordinator” right before Postgres becomes Redis, broker, coordinator, artifact registry, job queue, idempotency store, command bus, audit ledger, tenant router, and the actual database. Congrats, you deleted operational products by turning one product into all of them. The per-space lock is at least admitted, but `LISTEN/NOTIFY` plus SSE plus jobs plus migrations plus backfills is not a topology; it’s a future incident timeline. **[Unfair parts: the design deliberately narrows stateful dependencies, keeps statement execution outside the chain lock, supports separate worker scaling, and treats the lock as a measured characteristic rather than denying it.]**

**Top three walk-away reasons.**

1. No single-space throughput, p99 latency, concurrent-space, queue-fairness, or restore measurement exists. Evidence deficit.
2. There is no demonstrated fail-small behavior for the shared Postgres/KMS/object-store dependencies. Product-stage skepticism.
3. The docs call observability and DR proposed while product claims depend on them. The design is not release-ready by its own bar.

**Top three grudging compliments.**

1. The design names the serialization point and identifies the workloads that contend with human writes.
2. Correlation between trace and command group, separate security-event retention, and explicit high-cardinality handling are operationally literate.
3. Hot artifact rollback by registry re-point plus shadow-schema shape verification is a clean deploy model if implementation matches it.

**Verdict.** **No pilot; would review a benchmark candidate.**

**Exact evidence that flips it.** Reproducible load and fault-injection results: 1, 10, 100, and 1,000 active spaces; a single hot space; slow KMS signer; worker backlog; dropped `LISTEN/NOTIFY` connection; Postgres failover; object-store outage; restore. Publish p50/p95/p99, lock wait, retry rate, fairness, RPO/RTO, and which API guarantees degrade or stop.

## Persona 4 — Nadia Okafor, the reactive-database burn victim

**Stack history.** Meteor/Mongo oplog → Firebase listeners → GraphQL subscriptions → Convex → explicit commands plus read models on Postgres. She hates polling CRUD and hates systems where a seemingly innocent query creates a platform-owned recomputation graph.

**Formative disaster.** Convex’s June 1, 2025 T3 Chat postmortem is the kind she respects and fears: the vendor accepted responsibility for a production outage involving the machinery that reruns queries and updates subscriptions after mutations. The HN “How Convex Works” discussion supplies the quieter scar: a beautiful integrated model still constrains language, auth choices, and workload fit.

**What would actually convince her.** A precise invalidation algorithm, bounded fan-out, subscription backpressure, query dependency inspection, storm tests, and proof that authorization changes invalidate every affected live result without leaking a pre-change page from cache or idempotency replay.

**First impression.** “The command log as trigger bus and stale-signal/refetch model is conservative in a good way. The phrase ‘live-capable’ still hides an invalidation engine I have not seen.”

**Discord/HN-style reaction, verbatim.**

> This is Convex for people who want the database to apologize in Postgres. You replaced reactive functions with registered ops and then quietly rebuilt subscriptions, dependency manifests, routines, triggers, retry queues, generated clients, and generated UI around them. “Signal stale, refetch under scope” sounds safe until a permission edit fans out to 80k sessions and every client stampedes the one database that also runs your job queue. **[Unfair parts: Ordo does not promise result-diff streaming, treats `live` as a call-time choice, and explicitly includes guard entities in invalidation scope. The stampede is a plausible unmeasured risk, not an observed Ordo defect.]**

**Top three walk-away reasons.**

1. Live-query invalidation, backpressure, coalescing, reconnect, and cache correctness are not demonstrated. Product-stage skepticism.
2. Routine scope is trusted domain code rather than row-guarded code. This is a defensible trust-boundary decision, but she needs deploy-time re-consent semantics and a proof that scope expansion truly kills existing grants.
3. The same per-space chain orders humans, routines, feeds, and sources, so reactive storms can become foreground write latency. Unmeasured design risk.

**Top three grudging compliments.**

1. Blind triggers plus value-reading routines make the observation boundary explicit.
2. At-least-once delivery, idempotency identity, dead-letter behavior, and commit-only invalidation are the correct questions to answer.
3. Scope manifests that expand only through deploy and re-consent are more inspectable than dynamic database function reachability.

**Verdict.** **Conditional non-production pilot only after a runnable engine exists.** She is one of the two willing to design-partner because the reactive model is deliberately less magical than Convex/Firebase.

**Exact evidence that flips it.** A demo with 10,000 live subscribers, permission/group changes, bulk writes, hot deploy, dropped connections, routine redelivery, and a slow client. Show bounded database work per command, reconnection semantics, no unauthorized cached page after revocation, and no duplicate committed command under retries.

## Persona 5 — Jun Park, the local-first semantic pessimist

**Stack history.** Lotus Notes replication → CouchDB/PouchDB → operational transform → Yjs → Automerge → SQLite with application-specific conflict presentation. He hates server-authoritative spinner apps and hates the claim that convergence equals correctness.

**Formative disaster.** The 2019 local-first paper is formative precisely because it is honest: CRDT merging is only one part of a production local-first system, and early Automerge was not yet a general production answer. The 2026 HN CRDT discussion reinforces his daily experience that calendar recurrence, authority, deletion, schema migration, and domain invariants stay difficult after convergence is solved. Yjs’s own docs say backend scaling is nontrivial and leave transport/persistence choices modular.

**What would actually convince him.** A formal protocol state machine, executable model or TLA+/PlusCal spec, multi-device property tests, schema-migration semantics for offline replicas, deletion/tombstone policy, compaction behavior, and user-visible conflict UX.

**First impression.** “Reject-and-replay is a legitimate alternative to CRDTs. It becomes legitimate only when every state transition around reject and replay is specified.”

**Discord/HN-style reaction, verbatim.**

> ‘No CRDT’ is not a feature if the replacement is “the later device eventually replays something and the user sees a CAS miss.” Which device is online to validate? What if it never reconnects? Did the server already chain the ciphertext it cannot validate? Can other devices build on that head? If you remove the failed command, the hash changes; if you keep it, the rejected command became history. This is distributed commit with the coordinator blindfolded, described as five bullets. Also “typed unique ids make most conflicts benign” is the kind of sentence written before two offline devices edit the same invoice status. **[Unfair parts: same-row conflicts being surfaced rather than silently merged is a principled product choice, and Ordo does not claim CRDT-like automatic semantic correctness.]**

**Top three walk-away reasons.**

1. **Actual defect:** steps 1–4 of the settled merge rule do not establish when a proposal becomes an accepted chain segment relative to device-side CAS. “Server assigns each accepted segment its position,” “proposal is not history until server sequences it,” and “rejected command never becomes history” cannot all be implemented without an additional validation/commit handshake or a distinction between proposal-chain and command-chain.
2. Offline schema/version migration is not specified in the client-first section: a device can return with commands compiled against an old shape after server deploy/migration. Actual design hole.
3. The local execution vehicle is future WASM/embedded work while the top of `implementation.md` says no WASM in v1 and the compiler is server-only. The custody dial’s far end is labeled settled on top of an unavailable execution substrate. Relocated scope/status hole.

**Top three grudging compliments.**

1. The design refuses silent last-writer-wins and admits server-authored order even at the top of the custody dial.
2. It states that the server can drop, delay, or reorder proposals and narrows the mitigation to detection rather than prevention.
3. It understands that local storage is a database, that revoked offline replicas retain copies, and that key loss must be a user-visible custody choice.

**Verdict.** **No pilot and no protocol implementation from the present prose.**

**Exact evidence that flips it.** A normative wire/state-machine spec covering proposal creation, admission, version negotiation, validation, sequencing, acknowledgment, CAS failure, abandoned devices, duplicate delivery, compaction, schema migration, key rotation, and chain proof; then a model-checking report showing safety (no rejected command in canonical history, no fork, no accepted command lost) and liveness assumptions, followed by randomized three-device tests.

## Product skepticism versus actual design defects

| Item | Classification | Why |
|---|---|---|
| Nothing installable; no planner/executor/UI/SDK | **Product-stage blocker, not a design defect** | The README states this plainly. A prospect cannot pilot it, but the honesty prevents this from being a deceptive claim. |
| Authorization not implemented | **Product-stage blocker with a previously fatal implementation gap** | The normative design is now substantial; the current lexer/parser still does not recognize `access`, `acl`, or the flags. It is not evidence that the semantics are wrong, only that the central claim does not run. |
| Per-space chain lock with no numbers | **Unmeasured design risk** | Total per-space order structurally bounds throughput. It may be acceptable; without measurements and workload distributions nobody knows. |
| DR, migration recovery, observability, device security marked proposed | **Evidence/status gap** | The proposals are thoughtful, but calling surrounding platform properties settled can cause readers to mentally promote proposals into commitments. |
| Client-first accept/sequence/CAS lifecycle | **Actual design defect / underspecification** | The prose gives mutually dependent acceptance conditions without a transaction protocol that a blind server and occasionally offline validator can execute. |
| Offline replica versus schema/op-version changes | **Actual missing design** | Compiled artifacts are versioned server-side, but no client-first rebase/reject/migrate rule is given for proposals authored under an old artifact. |
| AGPL internal-use FAQ row | **Actual documentation/legal defect** | It reasons from “not distribution,” but AGPL §13 separately governs remote network interaction. The claim should be removed or replaced with counsel-reviewed, qualified guidance. |
| “Cost class” is not a hard budget | **Product/control gap** | The existing lattice correctly describes execution venue and static bounds; it does not claim to be a dollar cap. Adoption still needs quotas and budget controls. |
| Tier-1 routines bypass row guards | **Design choice, not presently a defect** | The trust boundary, derived field scope, trigger bound, re-consent, and attribution are explicit. Teams may reject the model, but it is coherent once implemented. |
| Closed language lacks ad-hoc OLAP/windows | **Intentional product fit** | The modeling guide says to export to a warehouse and calls Ordo the wrong tool for exploratory BI. |

## Prior fatal fixes: verification and relocated holes

This section checks only the five earlier fatal findings. It does not repeat deterministic gate findings such as marker scans, keyword parity, plan coverage, or ruling-reference coverage.

### X-002 — licence absent: fixed, with a relocated legal hole

**Verified fixed:** `LICENSE` (AGPL-3.0-only), `LICENSE-APACHE`, and `NOTICE` now exist. `docs/licensing.md` separates engine, spec/surfaces, and generated output; it states the hosted/open relationship and the business model.

**Stale contradiction:** README line 119 still says “no licence file yet.” That is a documentation freshness bug, not the old fatal absence.

**Relocated hole R3-LIC-1:** `docs/licensing.md` line 75 says modified internal use owes nothing because internal use is not distribution. [AGPL §13](https://www.gnu.org/licenses/agpl-3.0.en.html) applies when a modified version supports remote network interaction and requires an offer to all remote users. The [FSF GPL/AGPL FAQ](https://www.gnu.org/licenses/gpl-faq.en.html#UnreleasedModsAGPL) likewise says modified AGPL software on a website must offer source to users. Whether particular employees are “users” in a specific deployment is legal advice; the current categorical “nothing” is not safe documentation.

**Status:** old fatal fixed; new high-severity documentation defect.

### X-001 — authorization absent from parser/spec/plan: two-thirds fixed, runtime still absent

**Verified fixed at design level:** the guard rules are now normative in `definition-language.md`; the build plan has explicit syntax, resolution/ACL derivation, inlining, write-precondition, and conformance phases; the definition of done makes authorization a gate.

**Verified not implemented:** `crates/ordo-syntax/src/token.rs` currently ends its keyword inventory at the non-authorization language and contains no `Acl`, `Access`, `Unguarded`, `Unbounded`, or `Reproducible` token. README correctly says authorization is specified and not implemented.

**Stale contradictions:** README says the build plan has no guard-binding stage even though the current `engine-plan.md` has one. Conversely, `engine-plan.md` still says the guard norms live in the corpus rather than the spec, although the current spec says its subsection is normative and the corpus originates none of the rules.

**Status:** design fatal fixed; implementation remains a declared release blocker; document synchronization is poor enough to confuse reviewers about what was actually remediated.

### X-003 — no performance/restore/DR/observability: acknowledged and designed, not closed

**Verified improvement:** `implementation.md` now calls the chain lock a structural per-space serialization point, names the relevant metrics, lays out migration failure windows, proposes backup/restore behavior, and specifies a restore drill.

**Still open:** the document explicitly says there is no benchmark and no timed restore. Migration recovery, DR, and observability sections are `[proposed]`. No execution evidence exists.

**Relocated hole R3-OPS-1:** the prior critique asked for numbers and drills; the repository now contains a good description of the numbers and drills it still has not run. This changes “unasked question” into “known release blocker,” which is real progress but not closure.

**Status:** fatal for production adoption remains; design-review fatal reduced.

### X-004 — routine identity/unguarded acknowledgment: design fixed

**Verified fixed:** `definition-language.md` now states that tier-1 routines are trusted domain code, not callers; guards pass for that principal; derived scope, trigger, and attribution are the constraints. `run(...) unguarded unbounded:` gives the required acknowledgment slot. The tutorial and glossary repeat the model.

**Caveat:** no routine parser/executor implements it. The semantics are nevertheless coherent enough for conformance work.

**Status:** prior design fatal fixed; implementation pending.

### X-005 — ACL bootstrap deadlock: design fixed

**Verified fixed:** the creator receives the top ACL row in the same command group; routine/system/source creation gets no invented human grant; a space owner can administer any ACL row; the derived shape summary and chain disclose the implicit grant.

**Caveat:** the implementation does not exist. The spec also needs tests for create/revert atomicity and the owner escape, but the core deadlock has a clear answer.

**Status:** prior design fatal fixed; implementation pending.

## Cross-persona fatal set

The word “fatal” here means sufficient to stop the relevant adoption decision, not necessarily an irreparable architecture flaw.

1. **Pilot fatal — no runnable product.** Five of five stop here for a production pilot.
2. **Security fatal — authorization is prose, corpus, and plan, not executing code.** Five of five refuse real multi-user data until guarded execution and negative conformance cases run.
3. **Protocol fatal — client-first chain acceptance is not implementable from the settled text without another state machine.** Three of five call this a design defect; the other two would simply exclude client-first from a pilot.
4. **Operations fatal — no benchmark or restore drill.** The BaaS and platform personas refuse production even if the parser lands tomorrow.
5. **Trust fatal for legal review — categorical AGPL internal-use guidance is unsafe.** Two enterprise-oriented personas would stop procurement until corrected; the others would defer to counsel.

## Disagreements

- **Is a closed language lock-in?** Rina and Malik say yes at the application-semantics layer even with Postgres export. Nadia says registered commands are exactly the lock-in she will accept if the conformance suite and plain outputs are real. Jun distinguishes data exit from behavior exit: rows may leave while generated permission, routine, and sync semantics still require a rewrite.
- **Is routine guard bypass acceptable?** Nadia accepts it as domain code with derived scope and re-consent. Rina wants optional row-scoped service identities for least privilege. Ivo cares less about the philosophical label than whether scope expansion reliably invalidates grants and pages operators.
- **Is one Postgres a virtue?** Ivo calls it the right simplification only after failover and queue-isolation evidence. Rina strongly prefers it to a web of proprietary services. Malik worries that shared infrastructure metering still lacks a hard cap.
- **Is reject-and-replay better than CRDTs?** Jun says it can be, but only with a complete protocol and conflict UX. Nadia prefers explicit conflicts for business state. Malik predicts mobile users will interpret repeated CAS misses as data loss unless reconciliation is excellent.
- **Does AGPL help adoption?** Rina values fork insurance; Ivo’s employers may ban it; Jun likes Apache conformance materials more than the engine license. Everyone prefers the current split to source-available licensing.

## What survived the cold read

1. **Registered operations instead of runtime query text.** All five see real value in an enumerable, versioned callable surface, especially for agents and permissions.
2. **Derived facts are review artifacts.** Mode labels, field scope, cost class, effective guards, and cursor behavior being emitted rather than inferred from conventions is one of Ordo’s strongest ideas.
3. **Write semantics are unusually legible.** Create/converge/CAS/upsert/bulk derived from statement shape, insert completeness, and group atomicity reduce familiar accidental states.
4. **The documentation admits sharp edges.** Caller-supplied time, null behavior, fan-out, mutable pagination keys, open writes, routine trust, revoked offline copies, envelope leakage, and server-authored order are not hidden.
5. **Exit is designed at several layers.** Plain database substrate, full-space export, self-hosting, forkable engine, Apache spec/corpora, and unlicensed generated outputs form a credible direction even though behavior migration remains work.
6. **The space model is coherent.** Custody, membership, history, export, and deletion share a boundary; the modeling guide explains when that coupling is wrong rather than pretending every app wants it.
7. **Ordo knows what it declines.** No ad-hoc OLAP, no silent merge, no runtime arbitrary SQL, no offset paging, no hidden per-user op mutations. Skeptics prefer a narrow product with explicit refusals to an everything-platform that discovers limits in production.

## Evidence package that would change the panel result fastest

In order:

1. Ship a minimal runnable vertical slice: shapes, registered read/write ops, access guards/ACL, Postgres planning/execution, command groups, and `ordo check/plan/apply`. Exclude generated UI, client-first, feeds, and hosted-platform polish from the first proof.
2. Run the guard corpus against seeded Postgres with the no-guards canary and publish artifacts.
3. Publish the single-space contention benchmark and the kill/restore drill, including raw commands and timings.
4. Replace the client-first prose with a normative state machine and model-check it before implementation.
5. Correct the licensing FAQ through counsel; remove categorical legal conclusions that the license text does not support.
6. Add hard quotas/budgets and a derived deployment-cost surface alongside computational cost class.

If Ordo produces those six things, the panel changes from **zero production pilots / two design partners** to roughly **two constrained production pilots / four design partners**. The strongest surviving bet is not “seventy lines is an app.” It is narrower and more credible: **a closed operation language may make permissions, cost venue, mutations, and agent capabilities inspectable as one compiler artifact.**

## Bibliography

1. Paul Copplestone, [“Supabase incident on February 12, 2026”](https://supabase.com/blog/supabase-incident-on-february-12-2026), 2026-02-13.
2. Hacker News, [“Ask HN: Supabase PG upgrade wiped production DB, PITR backups failing”](https://news.ycombinator.com/item?id=47394240), 2026 (practitioner report; allegations not independently adjudicated here).
3. Hacker News / Vacation Tracker discussion, [“Fighting vendor lock-in and designing testable serverless apps”](https://news.ycombinator.com/item?id=19727218), 2019-04-23.
4. Hacker News, [“Firebase bill is usually $50, but I was surprised to see a $70k bill in one day”](https://news.ycombinator.com/item?id=42732714), 2025-01.
5. Google Cloud, [“Cloud Firestore pricing”](https://cloud.google.com/firestore/pricing), accessed 2026-08-03.
6. Jeremy Hartman and CJ Desai, Cloudflare, [“Cloudflare service outage June 12, 2025”](https://blog.cloudflare.com/cloudflare-service-outage-june-12-2025/), 2025-06-12.
7. Dane Knecht, Cloudflare, [“Code Orange: Fail Small — our resilience plan following recent incidents”](https://blog.cloudflare.com/fail-small-resilience-plan/), 2025-12-19.
8. Convex, [“How Convex Took Down T3 Chat: June 1, 2025 Postmortem”](https://news.convex.dev/how-convex-took-down-t3-chat-june-1-2025-postmortem/), 2025-06-03.
9. Hacker News, [“How Convex Works”](https://news.ycombinator.com/item?id=40020516), 2024-04.
10. Martin Kleppmann et al., Ink & Switch, [“Local-First Software: You Own Your Data, in spite of the Cloud”](https://www.inkandswitch.com/local-first/static/local-first.pdf), 2019.
11. Hacker News, [“An Interactive Intro to CRDTs”](https://news.ycombinator.com/item?id=47237460), 2026.
12. Yjs, [documentation](https://docs.yjs.dev/), accessed 2026-08-03.
13. Free Software Foundation, [GNU Affero General Public License v3, §13](https://www.gnu.org/licenses/agpl-3.0.en.html), 2007.
14. Free Software Foundation, [GNU license FAQ: modified AGPL software offered over a network](https://www.gnu.org/licenses/gpl-faq.en.html#UnreleasedModsAGPL), accessed 2026-08-03.
