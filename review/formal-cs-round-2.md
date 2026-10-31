# Formal CS Review, Round 2

Reviewer posture: adversarial database / programming-languages / distributed-systems review. The repository is pre-alpha, so a green unit suite is evidence only for the implemented fragments, not for the platform claims. I read the requested documents in the requested order and inspected all four current crates. `cargo test --workspace` passes (101 tests), but there is still no schema binder, planner, SQL executor, migration engine, Postgres command-log store, or Postgres replay store.

## Verdict

The second-round documents close several earlier specification omissions, especially the public description of cursor drift, isolation classes, `$space`, `members`, routine authority, and migration phases. They do not yet define a coherent implementation for client-first conflict acceptance, online backfill, replay linearization, or epoch-spanning uniqueness. Two release-gate corpora also still assert superseded behavior. The strongest remaining failures are protocol-state failures, not wording defects.

## Findings

### C2-001 — Critical — Client-first sequencing decides acceptance before the only party capable of deciding acceptance

**Exact citation.** `docs/implementation.md:502-527` says the server assigns every *accepted* segment a canonical position, a proposal becomes history when the server sequences it, losing commands are then replayed on-device, and a CAS-rejected command “never becomes history.”

**Hole.** Those transitions are cyclic. The server cannot know whether a ciphertext command passes CAS until a device evaluates it against canonical plaintext, but the document first puts the segment into canonical history and only then asks the device to replay it. Re-anchoring a header does not remove a rejected command from an already hashed ciphertext payload.

**Concrete counterexample trace.** Initial row `r=(rev=7, title=A)` is at head H. Offline D1 proposes `P1: CAS 7 -> title=B`; offline D2 proposes `P2: CAS 7 -> title=C`. Server receives P1, sequences it at H+1. It receives P2, sequences it at H+2 because it cannot decrypt either payload. D2 later downloads canonical state and evaluates P2: rev is now 8, so `cas_miss`. If H+2 remains, a rejected command is history, contrary to line 526. If H+2 is removed, the append-only chain and any later positions/checkpoints are rewritten. If the server waits for D2 before assigning H+2, server-arrival order is not sufficient and a lost/offline D2 blocks the per-space chain indefinitely.

**Smallest preserving fix.** Specify an explicit pending/final state machine: uploaded proposals are not chain entries; a server-issued ordering ticket identifies a candidate predecessor; a capable device returns a signed finalized accepted-command set; only that set is appended. Define timeout/abandonment, device loss, and how later arrivals progress. If strict arrival order must survive an absent device, expose a server-verifiable CAS basis (at least row id and expected `rev`) in the authenticated envelope and let the server reject structurally before append.

**Prior closure.** **X-058: relocated, open.** Forking and DEK-rotation prose was added, but the missing acceptance linearization moved into the “accepted segment”/on-device replay boundary.

### C2-002 — Critical — Client-first execution integrity is delegated to an untrusted client with no verifiable witness

**Exact citation.** `docs/implementation.md:467-477` keeps registered artifacts and mode labels as the contract while execution moves to the client; `docs/implementation.md:519-527` states that CAS is evaluated only on-device and the server cannot evaluate it over ciphertext.

**Hole.** Confidentiality from the server does not imply that every enrolled client correctly executes the WASM artifact. A client holding encryption and signing keys can emit well-formed ciphertext for a state transition that no registered op, guard, invariant, uniqueness rule, or CAS permits. The server verifies order and ciphertext integrity, not language semantics. Nothing states that enrolled devices are fully trusted to forge the database, and that trust assumption conflicts with the claim that the same compiled artifacts remain “the contract.”

**Concrete counterexample trace.** D1 holds the space key and a valid device credential but is compromised. The deployed artifact permits `set accounts { id, rev, nickname }` and forbids changing `balance`. D1 constructs ciphertext for `balance = 0`, increments encrypted `rev`, labels the envelope `converged`, signs the proposal, and uploads it. The server can authenticate D1 and chain the ciphertext, but cannot inspect the changed field, enforce the scope manifest, evaluate the row check, or distinguish this from a legitimate artifact execution. Other devices accept a chain-valid, semantically impossible state.

**Smallest preserving fix.** State the trust model explicitly: either an enrolled client is authorized to forge arbitrary state in its space (then withdraw server-enforced op/guard/CAS claims at client-first), or add a verifiable transition mechanism. A minimally invasive partial mechanism exposes authenticated structural witnesses—artifact hash, row id, expected/new rev, touched-field ids, unique/blind-index commitments—and has the server validate those; full value-dependent checks still require a trusted execution assumption or proof system.

**Prior closure.** **X-058: relocated, open.** The earlier question “who applies CAS?” is answered, but “who can verify that it was applied?” is not. **X-051: open by explicit trade-off** for ordering authorship; content signatures do not prove valid execution.

### C2-003 — Critical — Online transforming migrations lose writes made behind the backfill cursor

**Exact citation.** `docs/implementation.md:212-223` specifies additive → backfill → verify → swap and says old artifacts keep the source authoritative during a partial backfill, with reads and writes remaining correct. No dual-write, change capture, repeatable snapshot, or quiescence step is specified.

**Hole.** Keeping the old column authoritative protects current reads, but it does not keep an already-copied target synchronized. A chunk cursor is not a write barrier.

**Concrete counterexample trace.** Migration transforms `price_text` into `price_money`. Backfill copies row 1 as `"10.00" -> 1000` and advances past row 1. A normal old-version op then commits `price_text="12.00"` on row 1; it does not know the new column exists. Backfill finishes rows 2..N. Count verification succeeds. If verification hashes without blocking writes, it can read either side at different snapshots; even a full comparison can pass and then another old-version write can occur before swap. Registry swaps to the new artifact, which reads stale `1000`, losing the committed `1200` state.

**Smallest preserving fix.** Install dual-write/change-capture before the first chunk, record a database LSN/watermark, backfill from a snapshot, replay changes through that watermark, then take a short write barrier for final verify+registry swap. The transform and inverse/compatibility rules must be versioned so old and new artifacts can coexist during the window.

**Prior closure.** **X-071: relocated, open.** The crash-phase taxonomy is now present, but recovery safety moved to the unspecified online-write protocol.

### C2-004 — Critical — Per-epoch blind indexes break uniqueness across a live rotation

**Exact citation.** `docs/implementation.md:372-389` derives blind-index keys per epoch, fans equality lookup across live epochs, and writes only under the newest epoch. `docs/definition-language.md:404-423` promises that uniqueness on sealed fields rides the blind index and that uniqueness violations are typed rejections.

**Hole.** HMACs under different epoch keys are different database values. A SQL unique constraint over the blind-index column therefore enforces uniqueness only within an epoch. Fan-out lookup is not itself an atomic uniqueness constraint, and no cross-epoch locking protocol is given.

**Concrete counterexample trace.** Row U1 stores normalized email `a@example.com` with index `HMAC(K1,a)` under epoch 1. Rotation opens epoch 2 before U1 is re-indexed. A create of U2 with the same email stores `HMAC(K2,a)`. PostgreSQL sees distinct index bytes and accepts both, violating `unique`. Even if application code probes K1 and K2 first, two concurrent creates can both observe absence in old epochs and race unless a stable-key lock/constraint serializes the logical value.

**Smallest preserving fix.** Separate the rotating search index from a stable uniqueness commitment derived from a non-rotating per-space uniqueness key, and put the SQL unique constraint on that commitment. If key rotation must also rotate uniqueness material, serialize by a stable keyed lock and perform a transactional probe across every live epoch; define how re-indexing handles a discovered collision.

**Prior closure.** **X-066: relocated, open by analogy.** The sealed-RHS upsert row-lock recipe is specified, but the same cross-representation race reappears across blind-index epochs.

### C2-005 — Major — The replay API has a check/execute/store race, so concurrent identical requests execute twice

**Exact citation.** `docs/api-engine-design.md:293-315` promises “same result, no re-execution” and that the write plus replay record commit in one transaction. In code, `crates/ordo-api/src/idempotency.rs:139-142` exposes only independent `lookup` and `put`, while `crates/ordo-api/src/router.rs:348-377` performs lookup, awaits executor execution, then stores the result.

**Hole.** There is no atomic claim/reservation operation and no transaction handle connecting `ReplayCache` to `OpExecutor`. A Postgres implementation cannot make the current router sequence atomic merely by backing both traits with Postgres.

**Concrete counterexample trace.** Requests R1 and R2 carry the same `(space, principal, op, key, fingerprint)`. R1 lookup returns Miss. Before R1 commits or calls `put`, R2 lookup returns Miss. Both call a non-CAS create op and mint different ids; both commit. R1 stores response A, R2 overwrites/stores response B. The caller performed one logical request but two writes and two command groups exist. The current memory mutex does not help because it protects each lookup/put call, not the interval containing execution.

**Smallest preserving fix.** Replace `lookup`/`put` with an executor-integrated `claim` protocol (`INSERT ... ON CONFLICT` pending row, fingerprint comparison, wait/replay completed row) and pass one database transaction through claim, domain write, chain append, and completion. Add a two-concurrent-request test; the current tests cover only sequential replay.

**Prior closure.** **X-039: content-disclosure portion closed; exactly-once portion relocated/open.** Space/principal scoping, fingerprint, TTL, and bounds are correctly implemented. The concurrency property is not.

### C2-006 — Major — READ COMMITTED cannot attach an exact log position to a multi-statement read

**Exact citation.** `docs/implementation.md:143-149` assigns reads READ COMMITTED. `docs/implementation.md:646-655` promises that every result carries the exact command-log position at which it was computed and that multiple widgets cannot mix sides of one command group.

**Hole.** At READ COMMITTED, each SQL statement receives a new snapshot. A query statement and a later “read current chain head” statement do not share a database state. The document gives no single-statement or snapshot-export protocol tying result rows to a head.

**Concrete counterexample trace.** Head is 10. T1 executes the op SELECT and sees snapshot S10. T2 commits group 11. T1 then SELECTs the head and returns position 11 with rows from S10. A second widget starts after T2 and returns S11 at position 11. The client sees equal positions and advances both, displaying pre- and post-group-11 state simultaneously—the exact state lines 646-652 prohibit. Reading head first is symmetric: T1 reads 10, T2 commits 11, then T1's op SELECT sees S11 but labels it 10.

**Smallest preserving fix.** Execute the op and obtain its visible log position under one repeatable snapshot, or compile both into one SQL statement whose snapshot is shared. Define replica behavior: a replica must report the replayed chain position corresponding to its snapshot, not the primary's current head.

**Prior closure.** **X-060: isolation-class fix closed for writes; read linearization remains open.** This is a distinct consequence of the chosen READ COMMITTED read class.

### C2-007 — Major — `take` is treated as a candidate bound even when a host predicate may inspect the entire space

**Exact citation.** `docs/definition-language.md:1208-1217` says a bound may be proved from `take`; `docs/definition-language.md:1227-1238` claims the resulting row-cardinality gate prevents accidental O(space) work for every field.

**Hole.** A result bound is not a candidate/work bound when non-pushdown stages precede it. `take 10` bounds output cardinality, but a selective sealed predicate may require decrypting every candidate to discover that fewer than ten match.

**Concrete counterexample trace.** Space has N messages and sealed, non-indexed `body`. Register `op hits = messages(body contains "needle")[id] take 10`. With zero matching rows, execution must fetch and decrypt all N rows before proving the result has size 0. The proposed analysis sees `take 10`, calls the op bounded, and avoids `unbounded`, although work is Θ(N). The same failure occurs when matches are only at the end of the open-key order.

**Smallest preserving fix.** Track at least two bounds: rows entering each host stage and rows emitted. A downstream `take` may bound host input only when every preceding predicate/order operation is pushed down or another independent candidate bound exists. Add negative conformance cases for sealed-filter→take with zero and late matches.

**Prior closure.** **X-068/X-070: relocated, open.** The row-cardinality dimension and effective refinement class were added, but the proof rule conflates output and scanned candidates.

### C2-008 — Major — The pagination release corpus still asserts a property the settled spec refutes

**Exact citation.** `docs/definition-language.md:1161-1174` gives the correct drifting-cursor counterexample and explicitly permits repeats. `docs/composition-conformance.md:1281-1286` still labels C114 “Pagination stability under writes” and concludes “no duplicates, ever.”

**Hole.** The corpus is a claimed release gate. It tests only a row moving ahead of the cursor, which produces a skip, and encodes the obsolete universal conclusion. An implementation can pass C114 while failing to derive or disclose `cursor: drifting`, and the corpus supplies no move-behind duplicate case or post-`take` drift case.

**Concrete counterexample trace.** For C113, page 1 ends at `(spent=16000,cust_1)`. Instead of lifting Bo, a concurrent correction lowers Ada/cust_1 to 5000. The descending page-2 predicate now includes cust_1 again. The settled spec calls this a permitted duplicate; C114's asserted explanation says it cannot happen.

**Smallest preserving fix.** Rename C114 to a drifting-skip case and assert metadata `cursor: drifting`; add the move-behind duplicate case and a composed post-`take` case; add a stable-id case asserting the strong guarantee.

**Prior closure.** **X-059: prose closed; conformance open/relocated.** A fix that is absent from the release gate is not closed.

### C2-009 — Major — Genesis import reruns can converge rows, but the log format can revert only creates

**Exact citation.** `docs/migration.md:246-252` says a corrected rerun converges and updates existing rows. `docs/migration.md:253-258` immediately says genesis batches are create-only and their inverse is delete-by-minted-id without before images. `docs/security-logging-design.md:252-256` defines converge inversion as restoring the before image.

**Hole.** A converge is not invertible from an id set and a batch content hash. The proposed summary has discarded exactly the old state required for group revert.

**Concrete counterexample trace.** First import creates external id E with amount 100. A corrected mapping reruns and converges E to amount 125 in batch B, storing no per-row before image. Operator aborts before the genesis checkpoint and reverts B. Deleting E loses the valid first-import row; keeping E leaves 125 rather than restoring 100. The promised inverse 125→100 cannot be computed from count, ids, and ciphertext content hash.

**Smallest preserving fix.** Make pre-checkpoint reruns strictly create-only and reject any changed existing row into the review manifest, or log converged rows with before images/delta blobs. A practical split is compact create batches plus ordinary full-fidelity correction groups.

**Prior closure.** No exact prior X-id. This is a new contradiction exposed by the migration design added after X-071.

### C2-010 — Major — Match inference is specified as transitive but scheduled as direct-only, and exhaustive ambiguity diagnostics are factorial

**Exact citation.** `docs/definition-language.md:1050-1073` defines inference over all simple paths in an undirected ref multigraph and requires ambiguous errors to enumerate the paths. `docs/engine-plan.md:258-265` schedules “match inference (direct refs only).”

**Hole.** The build plan rejects legal settled programs such as the spec's own `projects receipts` inferred through `tasks`. Separately, enumerating every simple path has factorial worst-case output, so compilation has no polynomial resource bound even though ambiguity can be decided after finding two witnesses.

**Concrete counterexample trace.** Schema `receipts -> tasks -> projects`; query `projects receipts`. The settled spec finds the unique two-edge path. A P3 implementation following “direct refs only” reports no connection. For complexity, construct n entities whose undirected ref graph is `K_n` minus the direct edge `(u,v)`. Query `u v`. The number of simple u-v paths is `sum_{k=0}^{n-2} P(n-2,k)`, Θ((n-2)!). The compiler already knows the query is ambiguous after two paths, but the required diagnostic enumerates factorially many.

**Smallest preserving fix.** Change P3 to the settled direct-edge-preference plus transitive simple-path search. Search only until uniqueness is disproved and report two representative paths plus a count/lower-bound (“at least 2”), not every path. Add the two-hop unique and dense ambiguous schemas to conformance.

**Prior closure.** **X-011: relocated/open.** The path definition is now formal, but the implementation plan contradicts it and the diagnostic complexity remains unbounded.

### C2-011 — Major — The authorization release gate still compiles an illegal entity and asserts the superseded row oracle

**Exact citation.** `docs/definition-language.md:595-604` makes `members` platform-provided and undeclarable. `docs/guards-conformance.md:310-329` models domain-owned `members` rows (and its fixture earlier declares `entity members`). `docs/api-engine-design.md:121-135` resolves unreadable addressed rows to `not_found`; `docs/guards-conformance.md:552-555` still requires `access_guard`.

**Hole.** GC001-GC087 are named as a release gate, but the fixture cannot compile under the settled language and at least GC013/GC026 assert an observable result the API explicitly supersedes. Passing the corpus therefore either requires weakening the spec or implementing the wrong oracle.

**Concrete counterexample trace.** Harness loads its fixture. A conforming binder rejects `entity members { name: text }` because `members` is reserved platform state, so no GC case runs. If the harness special-cases that declaration, invoke `renameTask(task_1)` as Dov: guards corpus expects 403 `access_guard`; API §4.1 requires 404 `not_found`, indistinguishable from a nonexistent id. One executable cannot satisfy both.

**Smallest preserving fix.** Seed the platform `members` table through a harness fixture API rather than Ordo source; update R-G10 and all affected expected outcomes to §4.1; add a reject case proving domain declaration of `members` fails.

**Prior closure.** **X-018: spec closed, corpus open.** The principal/member reconciliation is now well specified. The old fixture was not migrated. The non-enumerability conflict is likewise resolved in the API and still open in the corpus.

### C2-012 — Minor — Cursor-stability labels have an unstated global invalidation dependency

**Exact citation.** `docs/definition-language.md:1155-1167` derives stability from whether *any registered write in the deployment* can move a sort key. `docs/implementation.md:19-25` says shape changes recompile dependent ops, but does not make a newly registered write a dependency of pre-existing read artifacts.

**Hole.** Cursor stability is non-local and anti-monotone: adding an unrelated-looking write op can weaken an existing read without changing its body, callees, or schema. Ordinary dependency tracking will not revisit it.

**Concrete counterexample trace.** Deploy R: `op byStatus = tickets[id,status] sort status`; the only write creates tickets, so R is labeled stable. Later deploy W: `op close($id)=set tickets{id:$id,status:"closed"}`. If the deploy recompiles only W and shape/callee dependents, R retains `cursor: stable`; a concurrent close can move a page-1 row behind the cursor and duplicate it on page 2.

**Smallest preserving fix.** Put write-set summaries in the dependency graph: every read sort key depends on the deployment-wide writers of that `(entity,field)`. Adding/removing/changing a writer must reclassify and version affected reads, with a deploy-diff assertion.

**Prior closure.** **X-059: relocated/open in incremental deployment.** The local classification rule is correct; its invalidation rule is missing.

## Prior-fix verification summary

| Prior item | Status after attack | Evidence |
|---|---|---|
| X-039 replay disclosure/poisoning | **Closed for key scope and fingerprint** | `ReplayKey` includes space/principal/op/key; fingerprint, TTL, and cap tests pass. Concurrent same-key execution remains C2-005. |
| X-040 missing space dimension | **Closed** | Router recognizes `$space`, binds it before access, and includes it in access and replay keys. |
| X-058 client-first merge | **Relocated/open** | Single-chain ordering is stated; acceptance/CAS linearization and execution integrity fail in C2-001/C2-002. |
| X-059 cursor overclaim | **Closed in main spec, open in corpus and deploy invalidation** | C2-008/C2-012. |
| X-060 write isolation | **Substantially closed in prose** | Per-class levels and whole-group retry are specified. Exact read-position linearization remains C2-006; `serialization_conflict` is still proposed/unimplemented. |
| X-066 sealed converge-or-create | **Closed for one-row locking recipe** | The `FOR UPDATE` recipe is explicit. Epoch-spanning unique indexes create C2-004. |
| X-068/X-070 cost honesty | **Relocated/open** | Row bounds and effective refinement class were added, but `take` is an unsound host-input bound (C2-007). |
| X-071 migration rollback | **Relocated/open** | Failure windows and artifact rollback are documented; concurrent-write capture is missing (C2-003). |
| X-018 `members` | **Closed in spec, open in tests** | Lifecycle and `$caller` mapping are explicit; release fixture remains illegal (C2-011). |

## Disagreements

1. I reject the description of client-first merge as `[settled]`. A total order over opaque proposals is settled; acceptance of commands into that order is not.
2. I reject “the write and replay record commit in the same transaction” as implementable through the current `ReplayCache`/`OpExecutor` trait split. The interfaces encode a TOCTOU gap.
3. I reject `take` as generic evidence of bounded host work. It is evidence only of bounded output unless all earlier selection/ranking is pushdown-capable.
4. I reject `guards-conformance.md` as a release gate in its current form. Its fixture and wire expectations contradict settled normative documents.
5. I disagree that path-enumerating diagnostics are harmless compiler ergonomics. The specified output itself is factorial in legal schemas.

## Claims that survived attack

- The two-valued null compilation obligation (`IS [NOT] DISTINCT FROM`, null-safe ordering) is precise and implementable.
- Per-statement static write-mode derivation plus insert-completeness is a coherent closed design; no counterexample found in the current surface.
- Group atomicity as one database transaction, commit-only triggers/invalidations, and whole-group retry is coherent **provided** chain append and idempotency completion participate in that same transaction.
- The main specification's cursor guarantee is now honest: stable only under write-stable keys, otherwise explicitly drifting.
- Server-sequenced per-space hashing gives a single fork-free ciphertext order; the review disputes acceptance and semantic validity, not the ordering construction itself.
- The command-log canonical encoding, per-space separation, image-shape validation, tamper detection, and inverse ordering are internally consistent in the current in-memory crate; its 13 tests pass.
- Space/principal-scoped replay keys and payload fingerprints close the original cross-principal replay disclosure.
- The migration three-deploy rule (add/backfill+swap/soak/drop) preserves a rollback point structurally; it needs change capture, but not a different phase decomposition.
- Static rejection of sealed guards and the requirement that guard entities enter manifests/live dependencies are sound.

## Required next gates

Before calling the execution model conformance-ready: resolve C2-001 through C2-007 in normative protocol text; repair both conformance corpora; add concurrency tests for duplicate idempotency keys, backfill writes behind the cursor, epoch-spanning unique creates, and read-position races; and make the dependency graph reclassify cursors when write sets change.
