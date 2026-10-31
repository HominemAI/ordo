# Formal CS Review — Round 4

**Reviewer stance:** adversarial PhD-level review in databases, programming languages, and distributed systems  
**Date:** 2026-08-09  
**Scope:** the documents and Rust crates named in the round-4 brief, read in the mandated order  
**Repository state:** read-only review; this file is the only artifact written

## Verdict

The corpus is materially stronger than round 3. In particular, space-bound capability grants, space-threaded execution, MAC-protected pagination, ACL-root creation, and DEK-independent blind indexes are real repairs rather than prose shuffles. The complete workspace test suite is green: **164 tests passed**.

The system is nevertheless not ready to support its strongest correctness claims. There are six release-blocking clusters:

1. The client-first protocol does not define one coherent signed object, does not pin the evaluator artifact, and does not make semantic outcomes or truthful inverse data authoritative.
2. The API interfaces cannot place domain state, command-log append, and replay completion in one database transaction, despite the documents making that coupling load-bearing.
3. Expiring replay reservations have no fencing generation, so a stale holder can delete or overwrite its successor and cause more than the admitted second execution.
4. Exact log-positioned reads and cross-request snapshot consistency are neither implementable from the stated READ COMMITTED recipe nor represented in `ExecResult`.
5. Transforming backfill and SMK blind-index rotation lack a write bridge or quiescence barrier through their scan/verify/swap boundary.
6. `std.space.create` is not representable in the current API or capability types, and the KMS orphan argument contradicts itself at its only crash boundary.

The most important distinction from round 3 is this: I no longer treat “every member device can decrypt the space” as an undisclosed security bug. The documents now explicitly define client-first as a mutually trusting cryptographic domain. That closes Z-001 by product ruling, but it does not close semantic validation, convergence, or truthful-history defects.

## Method and evidence

I read the requested documents in the specified order and then inspected every current crate and its tests. I ignored the deterministic document-gate failures identified in the brief (36 rule-coverage items, 17 markers, 646 absolutes, and 4 site requests); keyword/plan parity was treated as passing.

Executed:

```text
cargo test --workspace
```

Result: 18 `ordo-api` unit tests, 41 API integration tests, 16 command-log tests, 53 identity tests, and 36 syntax tests passed; all doc tests passed. No source, document, fixture, or configuration file was modified.

## Findings

### C4-001 — Client-first signs two incompatible notions of “the header”

**Severity:** Critical  
**Prior disposition:** Y-031 remains open; part of Z-002 remains open.

**Exact citations:**

- `docs/implementation.md:718-733` says a device signs a header carrying `payload_digest`, believed `prev`, device id, and counter, and that the signature proves content at the believed position.
- `docs/implementation.md:744-754` says re-anchoring changes `prev`, that the device re-signs, but also that the signature is over “content fields” and only `prev` changes.

These statements cannot all be true of one signature.

**Concrete execution trace:**

1. Devices D1 and D2 are offline at head H0 and each signs a proposal whose believed `prev` is H0.
2. The server sequences D1 at position 1, producing head H1.
3. The server sequences D2 at position 2 with canonical `prev = H1`.
4. If D2's signature covers believed `prev`, as the first passage requires, that signature is invalid on the canonical header. If D2 crashes after upload but before acknowledgment, the server has sequenced content for which no valid canonical device signature exists.
5. If D2's signature excludes `prev`, as the second passage requires, re-signing is unnecessary and the signature cannot prove what head D2 believed it extended. The server may attach that content signature to any canonical position.

This is not an implementation detail. It is ambiguity over the bytes whose authenticity defines the protocol.

**Smallest preserving fix:** define two immutable signed records:

- device-signed proposal `P = (payload_digest, device_id, counter, believed_prev, artifact_hash)`;
- server-signed sequencing receipt `R = (hash(P), canonical_position, canonical_prev)`.

The canonical chain commits to both `P` and `R`. No device re-signing occurs, and a device crash after upload cannot leave a half-attested chain link.

---

### C4-002 — “Caught and undoable” has neither an authoritative validator nor trustworthy inverse data

**Severity:** Critical  
**Prior disposition:** Z-002 open; Y-031 relocated here remains open.

**Exact citations:**

- `docs/implementation.md:640-668` says the blind server cannot validate legal state transitions, before-images, checks, CAS, or guards, but claims violations become “attributed, caught, and undoable.”
- `docs/implementation.md:734-778` defines content-blind sequencing and a deterministic no-op only for a `rev` mismatch; it does not define the state effect of every other guard/check/uniqueness/ref-integrity failure.
- `crates/ordo-commandlog/src/store.rs:299-329` constructs a revert by trusting the recorded images and swapping them.

**Concrete execution trace:**

1. At canonical prefix N, row `acct_1` has `balance = 100`, and a check requires `balance >= 0`.
2. A legitimate but malicious member submits a correctly signed, correctly encrypted proposal setting `balance = -100`. It records a false before-image `balance = 0`.
3. The server must sequence it because all transport facts are valid and the server is content-blind.
4. An honest device later detects the failed check. The corpus says “flag,” not whether this non-CAS command is a deterministic no-op or an applied-but-invalid state change.
5. If it applied, the generic `revert_group` swaps the untrusted images and restores `0`, not the actual predecessor `100`. If no honest device remains online, no stated actor ever produces the flag or corrective proposal.

Thus attribution is supported, but “caught” needs an honest-validator availability assumption and “undoable” needs canonical predecessor-derived images. A signature proves who supplied bytes; it does not make those bytes a truthful inverse.

**Smallest preserving fix:** pin one deterministic evaluator and define, for every invariant class, either (a) failure produces a canonical no-op outcome, or (b) the applied transition and inverse are derived from the prior canonical state rather than proposal-supplied images. If outcomes are intended to be authoritative to other parties, append a validator certificate over `(proposal_hash, artifact_hash, outcome, derived-image-hash)` and state the honest-validator/quorum assumption. Otherwise narrow the promise to “detectable by any honest, up-to-date device; correction requires a new proposal.”

---

### C4-003 — Client-first proposals do not pin the artifact/schema that defines their fold

**Severity:** Critical  
**Prior disposition:** Z-016 open.

**Exact citations:**

- `docs/implementation.md:689-693` requires clients to use the “same compiled artifacts.”
- `docs/implementation.md:722-726` enumerates the proposal header fields but includes no artifact, op-version, schema, or evaluator hash.
- `docs/implementation.md:755-781` concludes that every device at position N has exactly the same state.
- `docs/implementation.md:31-37` retains old hot-deployment artifacts, so the material needed for a pin already exists server-side.

**Concrete execution trace:**

1. D1 goes offline with artifact A1. The deployment installs A2, changing a default, check, field encoding, or command interpretation.
2. D1 creates proposal P under A1. Its header contains only payload digest, believed predecessor, device id, and counter.
3. The blind server sequences P while A2 is current; it has no protocol rule permitting rejection or selecting A1.
4. D1 folds P under A1. D2, freshly enrolled under A2, folds the same canonical bytes under A2.
5. The devices may derive different outcomes, images, or states while both comply with the document.

Canonical byte order does not imply deterministic state unless the transition function is part of the ordered input.

**Smallest preserving fix:** add the compiled evaluator/artifact hash and schema hash to the immutable device-signed proposal. A receiver must fetch and verify that retained artifact before folding. Define a retirement rule that refuses new proposals under an expired artifact while still permitting deterministic replay of already sequenced proposals.

---

### C4-004 — The current interfaces cannot realize the promised atomic domain/log/replay commit

**Severity:** Critical  
**Prior disposition:** Z-012 remains open; the command-log transaction part of X-060 remains an implementation blocker.

**Exact citations:**

- `docs/api-engine-design.md:382-387` and `docs/api-engine-design.md:422-430` require replay completion to commit in the same transaction as the domain write and chain append.
- `docs/implementation.md:220-228` makes that coupling a precondition of safe whole-group retry.
- `crates/ordo-api/src/router.rs:448-458` awaits `executor.execute` and only afterward calls `replay.put`.
- `crates/ordo-api/src/spec.rs:211-246` gives `OpExecutor::execute` no transaction or replay-completion handle.
- `crates/ordo-api/src/idempotency.rs:241-265` exposes independent `put`/`abandon` calls with no database transaction parameter.
- `crates/ordo-commandlog/src/store.rs:42-80` exposes an independent `append(&mut self, group)` and borrowed lookup results, not an append on the executor's database transaction.

**Concrete execution trace:**

1. Request R atomically reserves idempotency key K.
2. `OpExecutor::execute` commits domain mutation D (and, in the intended future, chain entry L) and returns `ExecResult`.
3. The process dies at `router.rs:450`, before `replay.put` at lines 451-457.
4. K remains pending. After timeout, a retry takes K and executes D again. If D is not self-idempotent, there are two effects. If it is, the retry can still return a different result or append another group.
5. Moving `put` before `execute` merely inverts the error: the replay table can promise an outcome whose domain transaction rolls back.

No Postgres implementation hidden behind the current traits can join a transaction that the executor has already committed and concealed. The same problem applies to `Store::append`: the trait provides neither the transaction handle nor an owned/asynchronous database result shape.

**Smallest preserving fix:** introduce one transactional unit-of-work owned by the execution layer. It must execute the plan, derive command images, acquire the per-space append lock, append the chain group, and conditionally finalize the replay generation on the same database transaction/connection. The router should receive only the committed response. A practical interface is an `execute_claimed(..., ReplayLease)` operation, or a transaction-context trait passed through all three repositories; independent post-return `put` and `append` are not sufficient.

---

### C4-005 — Stale replay takeover has no fencing and lets an old holder destroy its successor

**Severity:** Critical  
**Prior disposition:** Z-012 open, relocated from the initial claim race to lease ownership.

**Exact citations:**

- `docs/api-engine-design.md:366-387` permits takeover after 60 seconds but specifies no fencing generation.
- `crates/ordo-api/src/idempotency.rs:241-265` makes `put` and `abandon` identify only the replay key, not the reservation owner.
- `crates/ordo-api/src/idempotency.rs:357-368` deletes a stale pending slot.
- `crates/ordo-api/src/idempotency.rs:420-428` creates the replacement reservation without a generation.
- `crates/ordo-api/src/idempotency.rs:432-468` allows any old holder to overwrite a slot or abandon whichever pending holder is current.
- `crates/ordo-api/src/idempotency.rs:696-719` tests takeover but stops before the displaced holder returns.

**Concrete execution trace:**

1. T1 claims K at t=0 and runs an honest operation taking 65 seconds.
2. At t=60, T2 purges T1's slot, claims the same K, and starts executing.
3. At t=61, T1 fails with an unstorable error and calls `abandon(K)`. Since T2's slot is merely `Pending`, T1 deletes T2's reservation.
4. T3 now claims K and executes a third copy.

An alternative suffix is equally bad: T2 completes and stores response R2; then late T1 calls `put(K,R1)` and overwrites R2. For a different-payload stale takeover, the recorded fingerprint can also move backward. Atomic takeover prevents two callers from winning the *same generation*; it does not revoke the old process.

**Smallest preserving fix:** `claim` must return a monotonically increasing, unguessable lease/fencing generation. `put` and `abandon` must be conditional on `(key, generation, state = pending)`, and a stale call must be a no-op/error. The domain transaction must also check/finalize that same generation. Either renew the lease while an honest operation is live or prove the maximum operation duration is below the timeout; configuration prose is not fencing.

---

### C4-006 — “Permanent” create binding neither preserves the original outcome nor survives all current crash/eviction paths

**Severity:** Major  
**Prior disposition:** Z-018 partially closed for changed payloads, but the create-once claim remains open in implementation.

**Exact citations:**

- `docs/api-engine-design.md:375-380` calls the create key-to-payload binding permanent.
- `crates/ordo-api/src/idempotency.rs:93-111` calls it a create-once guarantee, then admits the in-memory implementation evicts it at the cap.
- `crates/ordo-api/src/idempotency.rs:449-459` installs the binding only during `put`, after execution.
- `crates/ordo-api/src/idempotency.rs:746-768` explicitly expects the same key and payload to reserve and execute again after response expiry.
- `crates/ordo-api/tests/api.rs:1117-1120` confirms a creating request executes twice after TTL.

**Concrete execution trace:**

1. Pure-create op C under key K creates row r and returns 200.
2. The response expires. The supposedly permanent fingerprint remains, but a same-payload retry is returned `Reserved` and executes C again.
3. A pure create now returns `duplicate`, not the original 200; a converge-or-create may recompute timestamps/defaults or append a second group. “Same request, same result, no re-execution” no longer holds.
4. More seriously, if the process crashes after creating r but before `put`, no permanent binding was installed. A retry with edited payload can execute. In the memory implementation, ordinary capacity eviction eventually permits the same outcome even without a crash.

The new fingerprint does close the narrow round-3 changed-payload case while the binding exists. It does not itself implement create-once or replay the original outcome.

**Smallest preserving fix:** persist a non-evictable creation record transactionally with the row: `(scope, principal, op, key, fingerprint, created_row_id, terminal_outcome)`. Same fingerprint returns the stored outcome or deterministically reconstructs it without executing; different fingerprint is a permanent mismatch. Do not classify an LRU entry as permanent.

---

### C4-007 — Exact log-positioned reads are absent, and the cross-request snapshot claim needs a read-at-position mechanism

**Severity:** Critical  
**Prior disposition:** Z-013 open; X-060 remains relocated here.

**Exact citations:**

- `docs/implementation.md:212-218` assigns reads READ COMMITTED.
- `docs/implementation.md:910-920` promises every read is labeled with the exact per-space command-log position at which it was computed and that multiple queries display one position.
- `crates/ordo-api/src/spec.rs:185-189` gives `ExecResult` only rows and an optional cursor.
- `crates/ordo-api/src/router.rs:747-766` emits no command-log position in the success envelope.

**Concrete execution trace A — one read operation:**

1. Read transaction R executes `SELECT rows...` under READ COMMITTED snapshot S7.
2. Writer W commits command group 8.
3. R executes a second statement `SELECT chain_head` and obtains position 8.
4. R returns pre-W rows labeled position 8. Reversing the statement order yields post-W rows labeled position 7.

Only a single SQL statement that reads both data and head from one statement snapshot, or a REPEATABLE READ transaction, can avoid this interleaving.

**Concrete execution trace B — one screen with two requests:**

1. Q1 refetches widget A at position 8.
2. Group 9 commits and changes data relevant to both widgets.
3. Q2 refetches widget B at position 9.
4. A rule that displays when all widgets have “reached 8” now shows A@8 with B@9, contradicting “one position.” Requiring equality can starve under continuous writes.

A numeric label is not a historical snapshot API. PostgreSQL cannot generally answer arbitrary later requests at an old MVCC snapshot after the owning transaction ends.

**Smallest preserving fix:** first add a position to `ExecResult` and obtain rows plus head in one REPEATABLE READ transaction or one SQL statement. For a multi-query screen, batch the ops into one transaction/snapshot and return one position, or provide an explicit bounded-lifetime snapshot token/read-at-position substrate. Otherwise weaken the cross-query guarantee to monotone freshness and allow mixed positions.

---

### C4-008 — Transforming backfill can still lose a committed old-shape write after scan or verification

**Severity:** Critical  
**Prior disposition:** Z-011 open; X-071 remains relocated here.

**Exact citations:**

- `docs/implementation.md:281-307` specifies additive → backfill → verify → swap and says old artifacts remain authoritative during backfill.
- `docs/implementation.md:290-292` claims resumable cursor processing plus count/content-hash verification is sufficient before the registry swap.
- No dual-write, change-log capture, version watermark, or write-quiescence barrier is specified in that protocol.

**Concrete execution trace:**

1. A transform splits old column `name` into new columns `first_name` and `last_name`.
2. Backfill copies row id 10 (`Ada Lovelace`) and advances its chunk cursor beyond 10.
3. The still-current old artifact updates id 10 to `Ada King`. It writes only the authoritative old shape.
4. Resume begins after id 10, so the new shape remains stale. Verification may detect this but has no repair rule and may never quiesce under traffic.
5. Even if verification observes equality, the same write can commit in the gap after verification and before registry swap. The swap then makes stale new-shape data authoritative.

Chunk resumability handles worker death; it does not handle writes behind the cursor or in the verify/swap gap.

**Smallest preserving fix:** from a recorded watermark onward, dual-write both shapes or capture every old-shape change in an ordered change table/WAL stream. Backfill to the watermark, replay captured changes, take a short write barrier, drain through the barrier, verify, and swap the registry plus write routing atomically. Add a test that updates an already-scanned row and another that commits between verify and swap.

---

### C4-009 — Genesis rerun convergence is still not invertible from the summarized batch

**Severity:** Major  
**Prior disposition:** Z-014 open; `bootstrap.md` now acknowledges rather than fixes it.

**Exact citations:**

- `docs/migration.md:225-230` omits per-row before/after images and records only count, minted-id set/range, and content hash.
- `docs/migration.md:253-265` says reruns converge or correct existing rows, while revert is delete-by-minted-ids.
- `docs/bootstrap.md:397-405` explicitly states that these promises cannot both hold.

**Concrete execution trace:**

1. A prior partial import already created external id E as row r with value `old`.
2. A corrected rerun converges E to value `new`; r is not newly minted by this batch.
3. If r is omitted from the minted-id set, reverting the batch does not restore `old`.
4. If r is included so the batch can be “undone,” delete-by-id destroys a row that pre-existed this batch rather than restoring it.

No choice of minted-id membership encodes the missing before-image/provenance bit.

**Smallest preserving fix:** make initial genesis batches expected-absent/create-only and fail loudly on a changed pre-existing external id. Emit corrections/convergences as ordinary image-bearing command groups. Alternatively, summarize creates but retain per-row before-images for converged rows; the inverse then deletes only provenance-marked creations and restores corrections.

---

### C4-010 — `scope deployment` and idempotent space creation are not representable in the current engine

**Severity:** Major  
**Prior disposition:** Z-020 relocated/open; Y-002 closed for ordinary space grants but open for deployment-scoped grants.

**Exact citations:**

- `docs/api-engine-design.md:65-71` and `docs/bootstrap.md:54-72` require `std.space.create` to bind no space and consult a deployment capability.
- `crates/ordo-api/src/spec.rs:100-120` has no op scope field.
- `crates/ordo-api/src/router.rs:300-341` unconditionally requires a space before access checking.
- `crates/ordo-identity/src/principal.rs:306-349` makes `SpaceId` mandatory in every `CapabilityGrant`.
- `crates/ordo-api/src/idempotency.rs:133-139` makes `space` mandatory in every `ReplayKey`.
- `docs/bootstrap.md:624-632` admits deployment-scoped grants are still open.

**Concrete execution trace:**

1. A correct `POST /op/std.space.create` omits `$space`, because the space does not exist.
2. The current router reaches `bind_space` and returns `missing_space` before authorization.
3. Supplying a sentinel/future id violates the documented contract and still cannot be authorized: the only capability constructor requires a real `SpaceId` and access checking compares it to the bound space.
4. There is also no replay namespace for the call, because every replay key requires a space. A crash after activation but before recording the API result cannot be replayed by the documented engine.

Implementing bootstrap as an out-of-band endpoint would abandon the claimed one generated surface, one door, and ordinary idempotency semantics.

**Smallest preserving fix:** add a closed `OpScope::{Space, Deployment}` to `OpSpec`; use a typed authorization target and `CapabilityScope::{Space(SpaceId), Deployment(DeploymentId)}`; key replay by the same typed scope; branch the router's envelope rule from the registered artifact, rejecting `$space` for deployment scope. Add end-to-end tests for no-space create, a supplied-space rejection, deployment-grant consent, retry after activation, and ordinary grants never satisfying deployment scope.

---

### C4-011 — Bootstrap's KMS crash creates exactly the orphan its invariant says cannot exist

**Severity:** Major  
**Prior disposition:** Z-020 remains open at external-resource recovery.

**Exact citations:**

- `docs/bootstrap.md:82-93` says the registry reservation precedes key creation and that “nothing after it is orphanable.”
- `docs/bootstrap.md:397-400` correctly identifies the crash after `createSpaceKey` returns but before its handle is recorded, then incorrectly concludes the registry row always names the key.
- `docs/bootstrap.md:407-413` elevates “every orphan is reachable from one table” to the rule that makes the saga tractable.

**Concrete execution trace:**

1. Phase 1 persists registry row S with no KMS handle.
2. `createSpaceKey(S)` durably creates K. The KMS returns handle h to the process.
3. The process dies before the platform-DB transaction that writes h.
4. S cannot name K; a registry-only reconciler cannot distinguish “K was never created” from “K exists under an unknown handle.” A retry can create K2. If the KMS cannot list by a stable external name, K is unrecoverable residue.

Recording a handle “before the key is used” prevents use of an unnamed key; it does not retroactively record the result before the creating call returns.

**Smallest preserving fix:** allocate and persist a deterministic external resource name/idempotency token derived from S before calling the KMS. `createSpaceKey(token)` must return the same handle on retry, or the KMS adapter must support lookup-by-token. Reconciliation then resolves the handle from the persisted token before deciding to resume or destroy.

---

### C4-012 — Page-token authentication omits the space, so a cursor is valid in another tenant context

**Severity:** Major  
**Prior disposition:** X-059 partially closed (tamper/principal/op repair survived), but cursor context integrity remains open.

**Exact citations:**

- `crates/ordo-api/src/page.rs:14-27` authenticates domain, op, principal, and payload, but not space.
- `crates/ordo-api/src/page.rs:75-98` confirms the HMAC input has no space.
- `crates/ordo-api/src/router.rs:655-675` decodes after space binding but does not pass the bound space.
- `crates/ordo-api/src/router.rs:747-764` mints the next cursor without the space.
- `crates/ordo-api/tests/api.rs:805-871` covers forgery, principal replay, and cross-op replay, but not cross-space replay.

**Concrete execution trace:**

1. Principal P may call `listInvoices` in spaces A and B.
2. Page 1 in A ends at keyset `(amount=10_000, id=inv_A9)` and returns token T.
3. P presents T to the same op/version in B.
4. The MAC verifies because op, principal, version payload, and deployment key match. The B executor seeks after A's boundary, skipping or reshaping B's result set despite no B-issued cursor authorizing that position.

This defeats the stated reason for the MAC—preventing caller-selected seeks—within the common multi-space principal model. It is also a straightforward omission from the isolation context already used by access and replay.

**Smallest preserving fix:** include the bound space, length-prefixed, in the page-token authenticator and pass it to both `encode` and `decode`. Add a cross-space rejection test using the same principal, op, and version.

---

### C4-013 — `is_logged_elsewhere` intentionally implements a cross-space correlation oracle

**Severity:** Major  
**Prior disposition:** Y-032 open; scoped image retrieval is fixed, existence disclosure is not.

**Exact citations:**

- `crates/ordo-commandlog/src/store.rs:63-80` first explains why global lookup is a tenant leak, then adds `is_logged_elsewhere` to disclose a global existence bit.
- `crates/ordo-commandlog/src/store.rs:138-146` scans every other space.
- `crates/ordo-commandlog/src/store.rs:258-280` exposes a distinct `CrossSpace` error.
- `crates/ordo-commandlog/src/store.rs:307-323` returns that error precisely when the correlation exists elsewhere.

**Concrete execution trace:**

1. Caller A has revert permission in space A and learns or guesses correlation c from a webhook, support ticket, client log, or timing side channel in space B.
2. `revert(A,c)` returns `CrossSpace` if c exists in any other tenant.
3. `revert(A,c')` returns `UnknownGroup` when c' exists nowhere.
4. A has learned a deployment-global fact about B. Repeating this for leaked candidate ids correlates activity across spaces.

The comment at lines 77-79 is logically false: a scoped lookup reveals “not in A”; it does not reveal “present somewhere else.” A security event can be loud internally without changing the caller's response.

**Smallest preserving fix:** remove global existence from the request/revert path and return `UnknownGroup` for every scoped miss. Record a suspected cross-space attempt only in a privileged deployment security stream if an internal correlation service independently recognizes it; do not expose the branch on the wire or in the public error type.

---

### C4-014 — Join inference mandates exponential output and invokes an NP-hard general case

**Severity:** Major  
**Prior disposition:** new formalization of the unresolved complexity/decidability risk around the settled inference rule.

**Exact citations:**

- `docs/definition-language.md:1575-1578` requires an ambiguity error to enumerate simple paths.
- `docs/definition-language.md:1580-1604` considers every connecting subgraph for three or more sources, chooses the minimum-edge one, and enumerates all minimum alternatives.

**Concrete execution trace:**

For the two-source case, construct source s, sink t, and k layers with two entities per layer. Connect s to both layer-1 entities, make every consecutive pair of layers complete bipartite, and connect both final entities to t. There are `2^k` equal-length shortest simple paths. At k=40 the specified diagnostic must enumerate over one trillion paths even though ambiguity is decidable after finding two witnesses.

For three or more terminals, finding a minimum-edge connecting subgraph is the unweighted Steiner-tree problem in a general graph. The decision problem is NP-hard; enumerating all optimum trees can add exponential output even after optimization succeeds. “Compile time” does not make the problem tractable, and package/schema authors control the graph.

**Smallest preserving fix:** for two sources, run BFS while counting shortest paths saturated at 2 and retain at most two witness paths. For k terminals, use an explicitly bounded fixed-parameter algorithm (for example, dynamic programming parameterized by k), cap k/graph size, and return `join_inference_too_complex` beyond the bound. Diagnostics should print two alternatives plus a count/lower bound, never every alternative. Requiring explicit bridges for over-bound cases preserves determinism and gives the author a terminating remedy.

---

### C4-015 — SMK blind-index rotation has no stated write linearization through scan and swap

**Severity:** Major  
**Prior disposition:** Y-009 is closed for DEK rotation; a narrower SMK-rotation uniqueness gap remains.

**Exact citations:**

- `docs/implementation.md:454-477` correctly derives BIK from the SMK, independent of DEK epoch.
- `docs/implementation.md:502-510` says an SMK rotation builds a new digest column “offline” and swaps atomically, without defining whether writes are frozen or dual-written.
- `docs/engine-plan.md:881-895` requires old/new columns never both queryable but specifies no write barrier or catch-up protocol.

**Concrete execution trace under the natural “shadow column is offline” reading:**

1. Rotation scans row ranges into `digest_new`; old artifacts continue writing only `digest_old`.
2. After the scan passes range R, a write inserts distinct value y in R. The old unique index accepts it and no `digest_new(y)` is stored.
3. Verification completes or has already covered R; the system atomically swaps the read/write key and unique constraint to `digest_new`.
4. A retry inserting y computes the new digest. The existing y row has null/missing new digest, so the new unique constraint does not collide and logical uniqueness is violated.

If “offline” means the entire write surface is quiesced from scan start through verified swap, this execution is excluded and the design is sound at an availability cost. The corpus must say that explicitly; “old and new never both queryable” is not a write invariant.

**Smallest preserving fix:** either state and enforce a write freeze covering scan, verification, and key/constraint swap, or dual-write old and new digests from a watermark, backfill, drain post-watermark changes under a short barrier, then atomically switch the active key and uniqueness constraint. Add a duplicate/retry test in the scan-behind-cursor and verify/swap windows.

---

### C4-016 — The glossary simultaneously states both sides of the blind-index ruling

**Severity:** Minor  
**Prior disposition:** Y-009 is substantively closed; this is contradictory guidance, not a second algorithmic defect.

**Exact citations:**

- `docs/glossary.md:398-406` correctly says blind-index keys do not derive per DEK epoch and explains why.
- `docs/glossary.md:408-415`, in the same entry, says they do derive per epoch and calls Y-009 unsettled.
- `docs/engine-plan.md:1497-1508` notices the stale text but describes the whole glossary entry as stale even though its first half has already been repaired.

**Concrete execution trace:** a developer follows lines 398-406 and implements one stable SMK-derived BIK; another follows lines 408-415 and implements an epoch-derived BIK. Both can cite the same glossary entry, and the latter reintroduces duplicate identities on ordinary DEK rotation.

**Smallest preserving fix:** delete `glossary.md:408-415`, mark Y-009 closed, and replace it with the distinct SMK scan/barrier caveat from C4-015.

## Requested prior-ID disposition ledger

| Prior ID | Round-4 disposition | Evidence / destination |
|---|---|---|
| **Z-001** | **Closed by explicit product ruling, not cryptographic repair.** I disagree with retaining it as a defect if the consent is binding. | `implementation.md:640-681` plainly says all enrolled devices hold the space key and same-space guards are convention/UI. The product must never claim hostile-member row confidentiality at this custody position. |
| **Z-002** | **Open.** | C4-001 and C4-002. Blind transport sequencing is valid; semantic outcome, honest-validator availability, and truthful inverse data are not. |
| **Z-011** | **Open.** | C4-008. Resumability does not capture writes behind the cursor or after verification. |
| **Z-012** | **Open, relocated.** | The atomic in-memory first-claim race is fixed and tested, but C4-004 and C4-005 show the transaction and stale-holder races. |
| **Z-013** | **Open and stronger.** | C4-007. The API carries no position, READ COMMITTED admits label skew, and cross-request “one position” needs a snapshot API. |
| **Z-014** | **Open, acknowledged.** | C4-009; `bootstrap.md:404` concedes the contradiction. |
| **Z-016** | **Open.** | C4-003. No artifact/schema hash is signed into a proposal. |
| **Z-018** | **Partially closed / relocated.** | A retained fingerprint rejects changed payloads. C4-006 covers crash-before-bind, eviction, and same-payload re-execution/outcome drift. |
| **Z-020** | **Relocated, not closed in the engine.** | The noncallable `provisioning` saga is a defensible external-state model. C4-010 and C4-011 block its current representability and recovery proof. |
| **Y-002** | **Closed for space-scoped grants; open for deployment scope.** | `CapabilityGrant` now structurally carries one space and cross-space tests pass. `std.space.create` still has no representable deployment grant (C4-010). |
| **Y-003** | **Closed at the specification/conformance level.** | G10a (`definition-language.md:1045-1081`) exempts only self-rooted create guards; the same-group creator grant is explicit (`1252-1280`), with GC091/GC092 cases. Execution-tier implementation is still owed. |
| **Y-009** | **Closed for DEK rotation.** | BIK derives from SMK, not DEK epoch (`implementation.md:454-477`; `engine-plan.md:861-868`). C4-015 is the distinct SMK transition, and C4-016 is stale prose. |
| **Y-015** | **Closed in code.** | `OpExecutor::execute` takes `space`; the router carries the approved space and cross-space/disagreement tests pass. |
| **Y-031** | **Open / split.** | Payload MAC commitment exists, but signed-header semantics, outcome certification, and artifact pinning remain C4-001–C4-003. |
| **Y-032** | **Open in a narrower form.** | Images are retrieved only by scoped `find`, but `is_logged_elsewhere` deliberately exposes cross-space existence (C4-013). |
| **X-059** | **Partially closed.** | HMAC tamper protection and principal/op binding exist and have tests. Space binding is missing (C4-012); confidentiality of readable base64 keysets should also be treated as a separate disclosure decision. |
| **X-060** | **Relocated/open.** | Write-isolation prose is coherent, but the exact read position and transaction interfaces remain C4-004/C4-007. |
| **X-071** | **Open under Z-011.** | The new failure-state table handles killed workers, not concurrent source writes (C4-008). |

## Disagreements with prior framing

1. **Z-001 should not remain “Fatal” once the trust domain is explicit.** A shared client-first vault in which every enrolled member receives the space key cannot simultaneously offer cryptographic row secrecy among those members. The current corpus now says exactly that and requires consent. The correct review result is “capability intentionally unavailable,” not “implementation failed to provide it.” Any UI/catalog text that still implies hostile-member row privacy would reopen the issue immediately.

2. **A blind server can validly provide total transport order.** Z-002 should not be read as proving that ciphertext sequencing itself is impossible. A server can authenticate senders, enforce counters/quotas, commit to ciphertext, and serialize arrival. The remaining failure is stronger semantic language—legal transition, truthful history, deterministic outcome across versions, and reversible invalid state—without a pinned evaluator and validator assumption.

3. **The command-log advisory-lock design is a valid linearization point.** `pg_advisory_xact_lock` per space, acquired inside the transaction that appends and held through commit, can serialize chain heads without globally serializing spaces. The defect is that the current `Store`/executor/replay interfaces do not carry that transaction, not that a per-space hash chain is inherently non-linearizable.

4. **The provisioning saga is a legitimate alternative to distributed atomic commit.** Requiring KMS, object storage, a platform DB, and a BYO DB to participate in one ACID transaction would be the wrong model. A durable, noncallable provisioning state with compensation can be correct. The review objections are the unnameable KMS resource, absent fencing/idempotent recovery, and lack of an engine surface—not the use of a saga itself.

## Claims that survived adversarial retesting

- **Capability space isolation:** `CapabilityGrant` has a private mandatory `SpaceId`; access compares it before expiry/allowlist. Tests cover own-space allow and wrong-space deny.
- **Executor space propagation:** the approved space is a named executor argument, not a bound user parameter. Tests cover cross-space denial and executor receipt.
- **ACL-root creation:** G10a is a static, narrow self-rooted exception, while parent-rooted guards remain evaluated. The same command group contains root creation and creator grant. The rule is internally coherent.
- **Ordinary concurrent replay claim:** for one non-stale generation in one process, the mutex-backed cache elects one reserver and all other callers wait. The 16-thread test is relevant and passes.
- **Cursor cryptographic primitive:** HMAC construction, constant-time comparison, version check, principal binding, and op binding are implemented and tested. C4-012 is an omitted context component, not a break in HMAC.
- **DEK-rotation uniqueness:** because BIK is SMK-derived, a DEK epoch change leaves digest bytes and the unique constraint unchanged. This directly closes the old cross-epoch equality bug.
- **Per-space chain mechanics in memory:** canonical hashing, link verification, correlation uniqueness per space, image shape checks, and reverse-order inverse construction pass their unit tests. Production concurrency/atomicity remains owed under C4-004.
- **Bootstrap visibility boundary:** if phase 5 is the only transition to `active` and every door refuses `provisioning`, external callability can linearize at activation. That is the right safety invariant for a saga, subject to C4-010/C4-011 recovery work.

## Release recommendation

Do not treat the green Rust suite as a release proof. It is a useful prototype suite, but the production stores and executor transaction boundary do not exist. Before a correctness-bearing release, the minimum blocking sequence is:

1. rule the client-first signed-record/evaluator/outcome model (C4-001–C4-003);
2. redesign execution around one domain/log/replay transaction and fenced replay leases (C4-004–C4-006);
3. define and implement exact snapshot semantics or narrow the claim (C4-007);
4. add watermark/change-capture/barrier protocols for transforming backfill and SMK reindex (C4-008, C4-015);
5. make deployment scope a first-class typed target and make KMS creation externally idempotent (C4-010, C4-011);
6. bind cursors to space and remove the global correlation oracle (C4-012, C4-013);
7. bound join inference and diagnostics (C4-014).

The design has moved from “contradictory everywhere” to “several coherent components separated by missing transactional and trust interfaces.” That is substantial progress, but those interfaces are exactly where database and distributed-system correctness lives.
