# [review/proposed] Ordo adversarial critique — round 3

> **Status: REVIEW / PROPOSED ONLY. Nothing in this document is adopted.**
> It consolidates independent round-3 reviews, preserves stable prior IDs, and
> proposes fixes and priorities. A status here records review evidence; it does
> not silently change the specification.

## 0. Method and severity normalization

Sources read in full: the prior consolidated critique and two security reviews;
the pre-round-3 deterministic report; round-3 formal mathematics and CS reviews;
security review 3; the five-person skeptic panel; and the cheap, strong, and
judgment AI-build reports. Current design/code was consulted only to adjudicate
reported contradictions.

Security review 3 uses `Critical` and `High`. This report maps its **Critical**
to **Fatal** when the hole falsifies an advertised custody/authorization model
for all admitted actors (SH3-001/007), and maps **High** to **Critical** because
each permits authority expansion, control-plane bypass, or durable account
compromise. This is a normalization into the established Fatal/Critical/Major/
Minor scale, not a softening of the security review.

Independent rediscovery is recorded verbatim. When two or more tracks found the
same hole, they are merged and every origin is cited; **independent rediscovery
multiplies severity**. Adjacent holes are not merged merely to shorten the list.

Effort: **S** ≤ 1 day, **M** ≤ 1 week, **L** > 1 week. `RULE` means the owner
must choose a product or semantic contract before implementation.

## 1. Executive fatal set

There are nine adoption-stopping facts after stable-ID deduplication. Some are
architecture defects; others are release/evidence blockers that remain fatal
for a production pilot even where the design direction is credible.

| ID | Fatal finding | Why it stops adoption now |
|---|---|---|
| **Z-001** | Client-first possession of the per-space key defeats row confidentiality | A legitimate or stolen enrolled device can bypass the client engine and decrypt rows its guard view would hide. The advertised same-space ACL/profile model is not a cryptographic boundary at this custody position. |
| **Z-002** | A blind server cannot validate client-first proposal acceptance or semantic correctness | The protocol sequences unreadable proposals before CAS can be decided, or trusts the submitting device to validate itself. Rejected commands enter history, the chain is rewritten, or liveness depends on an absent/hostile device. |
| **Y-005** | Grant “narrowing” compares output fields, not authority | An update can add writes, remove guards, add egress, widen triggers, or add key release with the same output set and silently retain consent. SH3-002 independently rediscovered and strengthened the prior fatal. |
| **Y-001** | Customer-controlled sink URLs are an SSRF path toward key-custody credentials | HTTPS/internal-address/redirect/DNS controls and a credential-free egress identity remain absent; one tenant can target the deployment perimeter. |
| **Y-002** | Capability grants are not space-bound | An agent/source token consented in one space can authorize its op allowlist in another. Together with **Y-015**, this is end-to-end tenant isolation failure. |
| **Y-003** | ACL creator bootstrap contradicts pre-insert guard ordering | Either every first ACL-bearing create deadlocks or the compiler relies on an unstated authorization exception. **Y-102/X-005** are relocated aliases, not extra findings. |
| **Y-004** | Guard-exempt routines can be woken by outsider-attributed writes | A `source:`/feed/guest/agent write can wake whole-manifest trusted code; the replacement manifest has no row dimension. |
| **X-001** | The central authorization claim still does not execute | Guard norms and the plan are now substantially repaired, but the lexer/parser/runtime still do not implement `acl`/`access` or guarded execution. Five of five panelists refuse real multi-user data. |
| **X-003** | No benchmark or restore drill exists | The per-space serialization point is admitted and operational designs are better, but there are still no throughput/p99/restore numbers and no customer-reproducible kill/restore evidence. |

The skeptic panel adds one categorical result: **zero of five would run a
production pilot today**. Two would join a design-partner exercise; three would
wait for a runnable artifact. That is not a sixth architecture finding, but it
is the adoption verdict this review must not obscure.

## 2. Prior-fix audit: what closed, relocated, and stayed open

### Earlier fatal findings

| Prior | Round-3 status | Evidence |
|---|---|---|
| **X-001** authorization absent | **Relocated/open** | Guard norms are in the spec, the plan has binding/inlining stages, and definition-of-done gates authorization. The current lexer/runtime still cannot execute it; README and engine-plan contain stale descriptions of one another. |
| **X-002** licence absent | **Relocated** | `LICENSE`, `LICENSE-APACHE`, `NOTICE`, and `docs/licensing.md` now exist. README still says no licence, and categorical AGPL internal-use advice is unsafe (**Z-015**). |
| **X-003** performance/DR/observability absent | **Carried/open** | The questions are now asked and designs are proposed. No benchmark, timed restore, or fault-injection transcript exists. |
| **X-004** routine identity/flag slot | **Closed at design level** | `run(...) unguarded unbounded:` is spellable and trusted-routine semantics are stated. Implementation is pending; routine rationale remains inconsistent (**Y-029**) and outsider-wakeable confused-deputy risk remains **Y-004**. |
| **X-005** ACL bootstrap deadlock | **Relocated** | Creator auto-grant states the intended outcome, but pre-insert guard ordering still contradicts it (**Y-003/Y-102**). Universal owner recovery separately relocates confidentiality into **Z-004**. |

### Material prior fixes that really closed

- **X-006**: uniqueness now uses null-safe semantics and rejects null address
  keys; the math review did not reopen that ruling.
- **X-019 / Y-100**: explicit read guard without write guard is rejected; both
  builders stated both verbs.
- **X-023**: std defaults to the open skeleton for sealed entities.
- **Y-101**: routine flags are expressible.
- **Y-103 / X-074**: `let` is now findable (not exercised in round 3).
- **Y-104 / X-039, narrow scope only**: replay keys include space/principal/op
  and fingerprint. Concurrent execution remains open as **Z-012**.
- **Y-105 / X-026**: DEK epochs and rotation are designed. Epoch-spanning
  uniqueness (**Y-009**) and log-image/re-index consequences remain.
- **Y-106 / X-040, door only**: space binds before access. The executor and
  capability paths remain open as **Y-015/Y-002**.
- **X-059, prose only**: `cursor: drifting` is honest in the main spec. The
  release corpus, proof calculus, and deploy invalidation remain open.
- **X-060, writes only**: isolation/retry prose is substantially repaired.
  Exact read-result position is still false under READ COMMITTED (**Z-013**).
- **X-066, one-row recipe only**: the sealed RHS locking recipe is present.
  Cross-epoch uniqueness is still **Y-009**.
- **X-071, failure phases only**: migration failure windows are documented.
  Live writes behind the cursor are still lost (**Z-011**).

### Stable prior-ID ledger

This ledger is authoritative for prior IDs included in `review/findings.json`.
“Carried” means no round-3 evidence closed or materially changed the original
hole. “Relocated” means a fix closed the original surface and recreated the
failure at another boundary; the linked Z/Y item gives the new form.

| ID | Status | Round-3 disposition |
|---|---|---|
| X-001 | relocated | Design/spec/plan repaired; runtime authorization absent. |
| X-002 | relocated | Licence exists; stale README and **Z-015** remain. |
| X-003 | carried | No measured performance or restore evidence. |
| X-004 | closed | Routine flag/identity design fixed. |
| X-005 | relocated | See **Y-003**, **Y-102**, **Z-004**. |
| X-006 | closed | Null-safe uniqueness ruling survived attack. |
| X-007 | relocated | `/0` closed; chosen significant-digit mechanism fails (**Y-006/M2-001**). |
| X-008 | relocated | Equality/clamp improved; **Y-016/Z-007** remain. |
| X-009 | relocated | Path semantics fixed; plan/diagnostic complexity is **Z-009**. |
| X-010 | relocated | Main spec fixed; std contract remains stale. |
| X-011 | relocated | Explicit equality mapping fixed; nullable `in` is **Z-008**. |
| X-012 | carried | Null arithmetic remains explicitly unruled. |
| X-018 | relocated | `members` is specified; guards corpus fixture remains illegal/stale. |
| X-019 | closed | Both access verbs required. |
| X-022 | closed | Multi-tenant tutorial exists; org-wide ACL + guest recipe still missing. |
| X-023 | closed | Open-skeleton std default adopted. |
| X-026 | closed | DEK epoch design adopted. |
| X-029 | carried | FNV manifest hashing remains. |
| X-031 | relocated | Refinement rules good in docs; code cannot enforce per-field flags (**Y-033**). |
| X-032 | relocated | API/code chose non-enumerating errors; release corpus remains stale. |
| X-039 | relocated | Scope/fingerprint closed; concurrency is **Z-012**. |
| X-040 | closed | Door binding closed; executor/capability holes are separate **Y-015/Y-002**. |
| X-051 | carried | Independent owner-verifiable chain remains open. |
| X-058 | relocated | Fork ordering stated; confidentiality/acceptance fail as **Z-001/Z-002**. |
| X-059 | relocated | Main prose fixed; corpus/proof/invalidation remain. |
| X-060 | relocated | Write isolation improved; read position is **Z-013**. |
| X-066 | relocated | Single representation recipe fixed; epoch uniqueness is **Y-009**. |
| X-068 | relocated | Cardinality gate added; `take` is not an examined-row bound. |
| X-070 | relocated | Effective class specified; enforcement metadata missing (**Y-033**). |
| X-071 | relocated | Failure taxonomy added; live backfill protocol is **Z-011**. |
| X-072 | carried | No executed backup/restore/chain reconciliation drill. |
| X-073 | carried | Observability remains proposed, not evidenced. |
| X-074 | closed | `let` discoverability fixed. |
| X-075 | carried | Deterministic keyword parity still fails. |
| X-078 | carried | App manifest grammar remains illustrative (**Z-022**). |
| X-079 | carried | SDK-filled parameter mechanism remains proposed. |
| X-080 | closed | Capstone defects repaired in current README/tutorial direction. |
| X-084 | relocated | Claims are better scoped; client-first and pilot claims still need evidence. |
| X-085 | carried | Exit direction is credible; executable round-trip is absent. |
| X-086 | carried | Strong tier validates grammar wedge; cheap tier still fails silently on authority. |

| ID range | Status | Round-3 disposition |
|---|---|---|
| Y-001–Y-030 | carried unless named below | No adopted fix was found; titles/origins remain unchanged in JSON. |
| Y-003 | open | Reconfirmed by both AI builds and BJ3-013. |
| Y-005 | carried/fatal | Independently rediscovered by SH3-002; severity reinforced. |
| Y-006 | carried | Independently proved by M2-001. |
| Y-009 | carried | Independently proved by C2-004. |
| Y-016 | carried | M2-004 supplies the missing proof-calculus counterexample. |
| Y-017 | carried | M2-009, C2-012, and BJ3-019 independently reinforce it. |
| Y-023 | carried | Strong build depends on a space-scope ruling. |
| Y-026 | carried | Cheap build was taught to project nonexistent `members.name`. |
| Y-029 | carried | Both build judgment and panel still find rationale inconsistent. |
| Y-031 | relocated | Absorbed into the broader client-first acceptance failure **Z-002**. |
| Y-032–Y-038 | carried | No round-3 closure evidence. |
| Y-100 | closed | Both access verbs present in both builds. |
| Y-101 | closed | Strong build uses routine flag slot. |
| Y-102 | relocated | Desired bootstrap result stated; ordering contradiction **Y-003** remains. |
| Y-103 | closed | Findability repaired. |
| Y-104 | closed | Transport scope/fingerprint fixed; **Z-012** is a distinct concurrency hole. |
| Y-105 | closed | Epoch design adopted; downstream epoch holes remain separate. |
| Y-106 | closed | Door binding fixed; execution propagation is **Y-015**. |

Explicit carried-ledger entries not otherwise expanded below: **Y-010** DEK
rotation versus before-images; **Y-011** `access true`/guest exposure;
**Y-018** larger-source match inference; **Y-019** stale rollup proof;
**Y-020** platform-derived unbounded acknowledgment; **Y-021** refinement
ordering; **Y-022** guest creator administration; **Y-024** missing authored
grant statement; **Y-025** proposed SDK-filled parameters; and **Y-027**
independently grouped aggregate comparison. Each remains carried/open with its
original title, severity, and origin preserved in `review/findings.json`.

## 3. Findings by area

### Client-first trust boundary

### Z-001 — Client-first possession of the space key defeats hostile-member row confidentiality · **Fatal** · status: open · orig: SH3-001

- **Claim.** Client-first changes execution venue while retaining the same
  roles, profiles, ACLs, and row guards inside a space.
- **Attack path.** A legitimate viewer/editor device, or a stolen enrolled
  device, receives material that opens per-space epochs. It ignores the SDK and
  local engine, reads synced/local ciphertext directly, and decrypts rows its
  guard view would hide. Future rotation cannot erase segments already held.
- **Smallest fix (RULE).** Choose one honest contract: (A) client-first spaces
  are cryptographic trust domains and every enrolled key holder may read all
  sealed values, with conversion rejected/warned for mutually distrustful row
  views; or (B) add per-principal/per-ACL cryptographic compartments and a
  rekey/sync design that never sends decryptable material outside each set.
- **Fix constraints.** Preserve offline use, server blindness, deterministic
  artifacts, conversion disclosure, and honesty about already-copied plaintext.

### Z-002 — A blind server cannot validate client-first acceptance, CAS, guards, checks, or truthful history · **Fatal** · status: open · orig: C2-001, C2-002, SH3-007, skeptic-panel round-3 (independent rediscovery)

Independent math-free protocol review, formal CS, security architecture, and
the cold panel all reached the same boundary; **independent rediscovery
multiplies severity**.

- **Claim.** Server arrival order is canonical; a proposal becomes history when
  sequenced; stale commands replay on-device; rejected commands never become
  history; the same artifacts/guards/checks/CAS remain the contract.
- **Counterexample.** D1 and D2 both propose CAS from rev 7. The blind server
  sequences both, then D2 discovers `cas_miss`: keeping D2 makes a rejected
  command history; removing it rewrites the chain. Waiting for D2 makes an
  absent/hostile device a liveness gate. A compromised enrolled device can
  fabricate a balance change, guard result, check result, revision, and before
  image; its signature proves device origin, not valid artifact execution.
- **Smallest fix (RULE).** Separate immutable proposal receipts from accepted
  command history. Append accepted effects only after a verifier independent of
  the submitting device validates them. Choose trusted/attested execution,
  proofs over committed state, an authorized quorum, or explicitly withdraw
  server-enforced semantic guarantees against malicious enrolled devices.
- **Fix constraints.** Preserve offline proposals, one canonical order, visible
  conflicts, server blindness, append-only history, and progress after device
  loss. Content signatures bind space, device, principal, op/version, base state
  commitment, effects, and nonce.

### Y-031 — Client-first segment commitment and CAS sequencing were under-specified · **Major** · status: relocated · orig: expert #8

The narrower round-2 symptom is now subsumed by **Z-002**. Its ID is preserved
and not reused.

### Authorization, grants, identity, and tenant egress

### Y-001 — Customer-controlled sink URLs are an SSRF path toward deployment credentials · **Fatal** · status: open · orig: SH2-3

- **Attack path.** A space owner grants a sink at cloud metadata, loopback,
  RFC1918/link-local, or a rebinding hostname. The request originates inside the
  deployment and can reach instance credentials and then `KeyProvider`.
- **Smallest fix.** HTTPS-only; resolve then pin; deny internal ranges after
  every resolution; no redirects; signed deliveries; execute through a
  dedicated egress identity/proxy with no instance credentials.
- **Fix constraints.** Sink remains runtime consent, never config; payload class
  remains derived and compile-checked.

### Y-002 — Capability grants carry no space and authorize synthetic callers deployment-wide · **Fatal** · status: open · orig: SH2-2, BJ3-016

- **Attack path.** A token consented for op `finance.setCategory` in space A
  names leaked space B; the capability branch never compares space. With
  **Y-015**, the data executor is also space-blind.
- **Smallest fix.** A capability grant carries one mandatory `SpaceId`, checked
  before expiry/allowlist; multi-space bridges hold separately consented grants.
- **Fix constraints.** Preserve one consent surface per space and explicit
  two-sided bridge consent.

### Y-004 — Guard-exempt routines are outsider-wakeable confused deputies · **Fatal** · status: open · orig: expert #9, SH2-10

- **Attack path.** `ingested` and `fed` are explicitly source/feed events. One
  outsider-controlled write wakes trusted routine code whose manifest bounds
  columns but not rows, allowing cross-row summaries/writes visible to the
  attacker.
- **Smallest fix.** Listener routines are anchored to `$trigger.ids` by default;
  wider reads require a routine-header acknowledgment and appear on source/feed
  consent surfaces. Reject output guards weaker than every read guard, or adopt
  an equivalent compile-time non-laundering rule.
- **Fix constraints.** Clock routines remain usable, scope stays derived, and
  routines do not acquire fake human membership rows.

### Y-005 — Output-field grant pinning cannot detect row-scope or effect expansion · **Fatal** · status: carried · orig: expert #7, SH3-002 (independent rediscovery)

- **Claim.** A changed manifest with an equal/subset ordered output field set is
  a silent narrowing.
- **Attack path.** A package keeps `[id,status]` but adds a payment write,
  removes `member == $caller`, adds `unguarded`, adds egress, widens a routine
  trigger, or releases a sealed key. The old token survives because output is
  unchanged.
- **Smallest fix.** Define a componentwise authority lattice covering read and
  sealed scope, write fields/statements/modes/deletes, effective guards and
  bypasses, bounds/refinement, output, egress, trigger/row anchor, and flattened
  composition. Silent narrowing requires monotonic non-expansion in every
  component; collision-resistant hashes authenticate, not classify, the
  canonical manifest.
- **Fix constraints.** Preserve genuine silent narrowing, per-op isolation,
  deterministic diffs, derived consent, and package updates.

### Z-003 — Client-scoped step-up lets callers shop for a weaker client · **Critical** · status: open · orig: SH3-003

- **Claim.** Exit/device/recovery operations are always step-up gated, but the
  requirement is configured on each client.
- **Attack path.** The PWA lists `space.delete`; an older mobile/CLI client does
  not. A stolen token calls the common op through the weaker client, converting
  an invariant into client preference.
- **Smallest fix.** Put minimum step-up on the op artifact/immutable platform
  op. Clients may add but never subtract. Bind token issuer, audience, and
  authorized party; evaluate intrinsic op policy independently.
- **Fix constraints.** Preserve one issuer/many apps and stricter client UX.

### Z-004 — Universal owner ACL administration means no ACL row is private from the space owner · **Critical** · status: open · orig: SH3-004

- **Claim.** ACLs protect row confidentiality; the owner can always administer
  any ACL row as orphan recovery.
- **Attack path.** A malicious owner self-grants top level on a whistleblower,
  HR, medical, or private-board row and reads it normally. The ACL adds one
  logged command, not confidentiality.
- **Smallest fix (RULE).** Either declare owners universal readers and render
  the override in every manifest, `callersOf`, privacy/share/custody surface;
  or replace immediate recovery with quorum/holder approval or delayed,
  notified break-glass that does not silently decrypt.
- **Fix constraints.** Preserve creator bootstrap, orphan recovery, owner exit,
  deterministic guards, and attribution.

### Z-005 — Suspension is display state, not an authorization input · **Critical** · status: open · orig: SH3-005

- **Attack path.** A compromised member is suspended, but the door still finds
  `(space,principal)->Role`; old access/refresh tokens and live streams continue,
  and client-first sync/epoch acceptance is not clearly revoked.
- **Smallest fix.** Require `members.status == active` before role/profile
  resolution. Suspension atomically revokes session families, closes live
  streams, blocks sync proposals, and rotates the future epoch where necessary.
  Reactivation is separate and does not revive old credentials.
- **Fix constraints.** Keep suspension reversible, audited, fast, and honest
  about offline copies.

### Z-006 — Authenticator linking turns transient session theft into permanent takeover · **Critical** · status: open · orig: SH3-006

- **Attack path.** A stolen bearer session calls `Directory::link` to attach an
  attacker passkey/OIDC subject. No fresh old-factor proof, bound new-factor
  ceremony, notification, or session rotation is pinned.
- **Smallest fix.** Transaction-bound dual proof: fresh existing authenticator
  plus proof of the new authenticator under one principal/client/nonce/expiry;
  notify existing factors; rotate refresh families; make recovery a distinct,
  delayed/notified path.
- **Fix constraints.** Preserve multiple providers, anti-enumeration, last-factor
  protection, and explicit recovery.

### Y-003 — ACL creator grant conflicts with pre-insert create-guard ordering · **Fatal** · status: open · orig: expert #10, strong-build, BJ3-013 (independent rediscovery)

- **Counterexample.** The derived ACL write guard runs before insert; the
  same-group creator grant does not exist yet, so the sanctioned bootstrap is
  rejected unless the compiler has an unstated special case.
- **Smallest fix.** Normatively define the derived ACL create exception and its
  exact ordering, with create/revert atomicity tests; or withdraw auto-bootstrap
  and supply another atomic platform operation.
- **Constraint.** Do not make `unguarded` + runtime exposure the ordinary
  bootstrap: that relocates structural safety into configuration discipline.

### Z-019 — Organization-responder assignee eligibility is inexpressible · **Critical** · status: open · orig: BJ3-015 (cheap+strong)

- **Claim.** An incident assignee is a responder in the incident organization.
- **Counterexample.** `assignee -> members?` accepts any space member, including
  a null-role guest; row-local checks cannot assert the organization ACL row.
  A filtered UI picker is not enforcement.
- **Smallest fix.** Add a blessed cross-row existence precondition/guarded ref
  constraint whose path is compiled to an indexed semi-join and enters the op
  manifest.
- **Fix constraints.** Remain closed, statically path-bound, and guard-aware.

### Z-020 — Whole-tenant atomic bootstrap has no single platform surface · **Critical** · status: open · orig: BJ3-012 (cheap+strong)

- **Counterexample.** Space creation, owner `members` row, domain enablement,
  organization row, and ACL creator grant span separate surfaces. The authored
  op can atomically couple only the final domain row and its grant.
- **Smallest fix.** Define one platform bootstrap command/state machine with
  compensating failure semantics, or scope “atomic bootstrap” explicitly to
  domain-row + ACL-row and make earlier states resumable and non-callable.
- **Fix constraints.** Consent/profile state remains runtime state, never IaC.

### Language semantics and compiler proof obligations

### Y-006 — The pinned decimal mechanism cannot implement 38 significant digits · **Critical** · status: carried · orig: expert #2, M2-001 (independent proof)

`NUMERIC(p,s)` fixes total and fractional digits, not significant digits across
arbitrary magnitudes. `1/3` and `10^38/3` defeat any fixed cast. Choose fixed
fractional scale with an explicit half-even function and exponent bounds, or
specify a magnitude-dependent significant-digit algorithm and exact SQL.

### Z-007 — Mixed duration application and `date ± duration` have no single type/result · **Major** · status: open · orig: M2-002, M2-003

- **Counterexamples.** `2026-01-30 + (1mo + 2d)` is March 2 months-first and
  March 1 milliseconds-first. `2026-01-01 + 12h` could be a date, datetime, or
  error; runtime duration values cannot choose a static return type.
- **Smallest fix.** Publish the operator matrix; apply months then milliseconds;
  allow `date ± duration -> date` only when static analysis proves integral-day
  milliseconds, otherwise require datetime.
- **Constraints.** Define negative/multiplication behavior and UTC for datetime.

### Y-016 — Duration comparison has no reproducible static proof calculus · **Major** · status: carried · orig: expert #3, M2-004

Specify a finite abstract domain such as `Zero | Months | Milliseconds |
MixedOrUnknown` and transfer/join rules for fields, params, arithmetic, `cond`,
and `coalesce`. Otherwise two sound compilers accept different languages.

### X-012 — Null arithmetic remains explicitly unruled · **Major** · status: carried · orig: strong #8, M2-005

Make arithmetic/functions strict on null except named null-eliding functions;
a null crossing a non-nullable boundary is a typed runtime rejection. Add
host/SQL goldens.

### Z-008 — Nullable `in` violates the two-valued equality model under native SQL · **Major** · status: open · orig: M2-006

`null in (null)` must be true under Ordo equality, while PostgreSQL `IN (NULL)`
is unknown. Define membership as a disjunction of `==` and compile each term
null-safely; define the empty list.

### X-068 — `take` bounds output, not rows examined · **Major** · status: relocated · orig: strong #12, M2-007, C2-007 (independent rediscovery)

The new row-cardinality gate repeats the prior cost overclaim: a sealed host
filter or host sort before `take 1` can scan/decrypt all N rows. Track produced
rows and per-stage examined rows; downstream `take` bounds upstream work only
when pushdown/early termination is proved.

### Z-009 — Match plan contradicts settled transitive inference and mandated diagnostics can be factorial · **Major** · status: open · orig: M2-011, C2-010 (independent rediscovery)

The spec permits unique transitive simple paths; engine-plan schedules direct
refs only. Requiring every ambiguous path to be enumerated permits exponential
or factorial diagnostic output. Implement settled transitive inference, stop
after two witnesses disprove uniqueness, and report two paths plus “additional
paths omitted.”

### Z-010 — Lexer accepts nonexistent Gregorian dates · **Minor** · status: open · orig: M2-012

`2026-02-31`, `2025-02-29`, and `2026-04-31` lex as dates. Validate Gregorian
month length/leap years before AST construction; include 1900/2000 cases and
datetime variants.

### Distributed execution, migrations, and audit

### Z-011 — Online transforming backfill loses writes behind the cursor · **Critical** · status: open · orig: C2-003

- **Trace.** Backfill copies row 1, an old artifact updates its source column,
  backfill advances, verification/swap reads the stale target. “Old remains
  authoritative” protects current reads, not already-copied target state.
- **Smallest fix.** Install dual-write/change capture before chunk 1, record a
  snapshot/LSN, backfill, replay through a watermark, then take a short write
  barrier for final verify+swap.
- **Constraints.** Version transform compatibility so old/new artifacts coexist;
  preserve the three-deploy rollback structure.

### Y-009 — Per-epoch blind indexes cannot enforce one logical uniqueness constraint · **Critical** · status: carried · orig: expert F3, C2-004 (independent rediscovery)

`HMAC(K1,email)` and `HMAC(K2,email)` are distinct SQL values. During rotation,
the same email can be inserted twice; probe-then-write races too. Use a stable
per-space uniqueness commitment or stable keyed lock plus transactional probes
across live epochs. Search-index rotation and uniqueness authority are separate.

### Z-012 — Replay lookup/execute/put races and double-executes concurrent retries · **Critical** · status: open · orig: SH2-15, C2-005 (independent rediscovery)

Two same-key requests both miss before either `put`, both execute, and produce
two command groups. Replace lookup/put with an atomic `claim`/pending lease in
the same transaction as domain write, chain append, and completion; matching
callers wait/replay, mismatches remain hard errors.

### Z-013 — READ COMMITTED cannot label a multi-statement read with its exact log position · **Major** · status: open · orig: C2-006

Rows may come from snapshot S10 and a later head query return 11, letting two
widgets share label 11 while showing opposite sides of group 11. Execute result
and visible head in one repeatable snapshot or one SQL statement; replicas
report their replayed position, not the primary's head.

### Z-014 — Genesis rerun convergence is not invertible from a minted-id set · **Major** · status: open · orig: SH2-6, C2-009 (independent rediscovery)

A corrected rerun converges an existing row without a before image; delete by
“minted ids” destroys the prior row and keeping it cannot restore the old value.
Separate created/converged ids and make corrections ordinary full-image groups,
or reject changed existing rows during genesis.

### X-059 — Cursor repair is absent from release corpora and incomplete as a dependency proof · **Critical** · status: relocated · orig: C2-008, M2-008, M2-009, C2-012

`composition-conformance.md` still says “no duplicates, ever”; aggregate and
guard-visible keys can move without a write brace naming the derived key; adding
a new writer can invalidate an old stable read without recompilation. Repair
C113–C115, derive base-field plus row-set/guard dependencies, and make writer
summaries deploy dependencies.

### X-018 — Platform `members` is fixed in the spec and still illegal in the guard release fixture · **Major** · status: relocated · orig: C2-011

Seed platform members through a harness API, reject domain declaration, and
update stale `access_guard` expectations to the adopted non-enumerating error.
Until then the named authorization release gate cannot compile and asserts the
superseded oracle.

### Product, legal, and operational evidence

### Z-015 — Licensing FAQ gives unsafe categorical AGPL internal-use guidance · **Critical** · status: open · orig: skeptic-panel R3-LIC-1

The old “no licence” fatal is closed, but saying modified internal use owes
nothing because it is not distribution ignores AGPL §13 remote-network use.
Remove the categorical conclusion and obtain counsel-reviewed, qualified text.
This is documentation/legal accuracy, not a request for this review to provide
legal advice.

### Z-016 — Client-first has no old-artifact/schema proposal rule · **Major** · status: open · orig: skeptic-panel persona 5

An offline device can return with proposals produced under an old shape/artifact
after server migration. Define version negotiation, reject/rebase/migrate rules,
compaction compatibility, and user-visible failure in the normative protocol.

### Z-017 — Cost class is not a hard deployment budget · **Major** · status: open · orig: skeptic-panel personas 2–4

The lattice prices venue and static bounds, not dollars, storage growth, egress,
queue amplification, live-refetch storms, routine/feed loops, or tenant fairness.
Add enforceable per-space quotas/budgets and a derived deployment-cost surface.
Do not weaken the existing computational lattice; this is a distinct control.

### AI-authoring and interface completeness

### Z-018 — “Idempotent creation” silently converges changed payloads without CAS · **Critical** · status: open · orig: BJ3-017 (cheap+strong independent tier reproduction)

Both builders used a unique request key plus mutable payload in no-id `set`.
Reusing the semantic key later updates title/severity/summary—and in the strong
build resets status to open—without `rev`. Transport fingerprinting expires and
does not make the semantic key immutable. Add create-once/assert-same-payload
semantics, or stop calling converge-or-create idempotent creation and teach the
silent update prominently.

### Z-021 — Nullable operation parameters required by generated forms have no settled syntax · **Major** · status: open · orig: BJ3-004

Fields use `?`, but parameter types do not define nullable/defaulted spelling.
The cheap builder invented `$dueOn: date?`; the strong builder emitted separate
set/clear ops. Settle nullable parameter types, binding, wire representation,
and how generated edit forms clear nullable refs/dates.

### Z-022 — App/form/detail/live grammar remains illustrative, so a complete generated UI is not spec-compilable · **Major** · status: open · orig: BJ3-008, BJ3-009

`form` is invented, arbitrary authored `detail` is open, and `live` on detail is
unpinned. Finalize the manifest grammar, action/form placement, binding, and
detail/live semantics before claiming complete definition emission.

### Z-023 — Normative/onboarding examples teach invalid member display and inconsistent unbounded scans · **Major** · status: open · orig: BJ3-005, BJ3-006

The tutorial teaches `assignee.name`, but the closed platform `members` shape
has no name; the cheap app therefore projects a nonexistent field. Tutorial
routine language also weakens the settled requirement for `unbounded`. Add a
real display-profile field/mechanism or remove the invalid projection, and make
every scan example obey the registration gate.

## 4. Document-versus-document contradictions

| Subject | Document A | Document B | Proposed winner | Finding |
|---|---|---|---|---|
| ACL create ordering | `definition-language.md` G10: guards pre-insert | Same section: creator grant in same group is sanctioned bootstrap | **RULE:** explicit derived-create exception with ordering/tests, or another bootstrap | Y-003/Y-102 |
| Client-first acceptance | `implementation.md`: sequence accepted proposals by arrival | Same section: device later evaluates CAS; rejected command never history | Neither; add pending/receipt/final state machine | Z-002 |
| Client-first guard parity | Language/access docs promise same ACL/profile model | One wrapped per-space key gives an enrolled device all decryptable material | **RULE:** trust-domain disclosure or cryptographic compartments | Z-001 |
| Grant narrowing | Security docs: subset output fields = narrowing | SH3-002/Y-005: output says nothing about write/guard/egress/trigger authority | Componentwise authority lattice wins | Y-005 |
| Step-up owner | Client block/config demands step-up | Exit/device ops are described as always step-up | Intrinsic op minimum wins; clients may only strengthen | Z-003 |
| Duration application | `(months,ms)` and each component's local rule | No order/type for mixed/sub-day `date ± duration` | Months-first; publish typing table | Z-007 |
| Nullable membership | Two-valued equality applies to comparisons | SQL mapping omits `in`; native `IN` is 3VL | Expand to null-safe equality disjunction | Z-008 |
| Bounded work | `take` proves a candidate bound | A host filter/sort before take can examine N rows | Examined-stage bound wins | X-068 |
| Cursor guarantee | Main spec labels mutable keys drifting | Composition corpus says “no duplicates, ever” | Main spec wins; repair release corpus | X-059 |
| Cursor proof | Spec says write braces decide stability | Aggregates/guards/creates/deletes move keys indirectly | Dependency + row-set analysis wins | X-059/Y-017 |
| Match reach | Main spec permits unique transitive simple path | Engine plan schedules direct refs only | Main spec wins; bounded diagnostic witnesses | Z-009 |
| Rollup closure | Main spec removes shallow far side | `std-library.md` says same closure as `whole` | Main spec wins; repair std owner doc | X-010 |
| `members` ownership | Main spec: platform-provided, undeclarable | Guard corpus declares it and expects stale oracle | Main spec/API win; migrate fixture | X-018 |
| Read position | Results promise exact command-log position | READ COMMITTED gives per-statement snapshots | Repeatable snapshot/single statement | Z-013 |
| Genesis rerun | Rerun converges existing rows | Revert is create-only delete-by-minted-id | Full-image corrections or reject converge | Z-014 |
| Licence state | Files/licensing doc exist | README says no licence file | Files win; update README | X-002 |
| AGPL internal use | Licensing FAQ says internal non-distribution owes nothing | AGPL §13/network-use guidance is not distribution-only | Remove categorical claim; counsel review | Z-015 |
| Authorization plan state | Current engine plan has guard stages | README says the plan has none; plan says norms remain corpus-only | Current spec/plan win; synchronize both | X-001 |
| Member display | Tutorial uses `assignee.name` | Normative platform shape has no `name` | **RULE:** add display-profile mechanism or remove example | Y-026/Z-023 |
| UI source | Brief/pitch requires list/detail/create UI | `ui-gen.md` grammar remains illustrative/open | Finalize grammar before claim | Z-022 |

Rows marked `RULE` are genuine owner decisions. The remaining rows are stale
copies or missing protocols: one source clearly wins and synchronization is
mechanical after the protocol is chosen.

## 5. AI buildability comparison

| Dimension | Cheap tier | Strong tier |
|---|---|---|
| Likely written-spec compile | **Fail.** Nullable params, missing `unbounded`, nonexistent `members.name`, unbound page arg, invented `form`. | **Fail as written, close after local edits.** Newline after `or`, detached projections, unsupported `unique guest`; app/detail/live still unsettled. |
| Idiomaticity | Recognizable shapes/CRUD, but separate incident ACL silently defeats organization authority. | One space/org, owned ACL root, explicit note guard, CAS, id sort, separate guest app, runtime exposure table: substantially idiomatic. |
| Brief coverage | Incomplete membership/guest/admin/UI/offline handoff. | Covers most requested behavior; whole-tenant bootstrap, invitation/profile machinery, create page remain unavailable/unsettled. |
| Silent-wrong risk | Fatal: wrong tenancy rule; spoofable authors; non-exact guest access; semantic key edits; claimed stable mutable timestamp. | Lower but real: semantic key edits/reset; assignee may be guest; bootstrap special case; runtime space/capability/live holes. |
| Loud errors | Several parser/manifest failures are good failure modes. | Formatting regressions are loud and cheap to fix. |
| Platform reality | Neither runs: no guard binder/planner/executor/UI. | Same. Core source quality cannot compensate for runtime space loss. |

The **cheap-vs-strong gap is material**: full-corpus access produces a much more
coherent authority model and a better caveat ledger. It does not produce a safe,
complete app because several brief requirements are not language-expressible.
The surprising strong-tier regression is lexical line structure, an author
error that should fail loudly.

Docs/design faults versus author faults:

- **Author faults:** strong line breaks/detached projections; cheap unbound page;
  cheap wrong organization/incident ACL modeling after failing to find a recipe;
  spoofable `$author`; exposing an unguarded bootstrap or note/write ops too
  broadly would be deployment-author faults.
- **Mixed responsibility:** nullable-param invention, `form`, single-ref unique
  spelling, missing `unbounded`, and cheap exact-one sharing—the docs do not
  provide a closed, findable recipe.
- **Design/platform faults no careful author can repair:** ACL create ordering;
  whole-tenant bootstrap; invitation/suspension/profile inventory; assignee
  cross-row eligibility; member display; UI grammar; uniqueness space scope;
  executor/capability space propagation; client-first validity; create-once
  semantic idempotency.

Both tiers got important things right: implicit id/rev, CAS for ordinary edits,
owned child modeling, both access verbs, sealing distinct from authorization,
clock/query/`each -> set`, paging in the API envelope, and consent remaining
runtime state.

## 6. Skeptic panel verdict and sources

**Verdict:** zero production pilots; two design partners. The panel separates
honest pre-alpha status from design defects. The fatal adoption set is: no
runnable vertical slice; authorization not executing; client-first acceptance
not implementable from current prose; no benchmark/restore drill; unsafe legal
FAQ guidance for enterprise review.

The panel's scars came from the Supabase 2026 regional-control outage and a
practitioner PITR/data-loss report; Firebase usage/billing amplification and
first-party pricing; Cloudflare's 2025 dependency cascade and Fail Small work;
Convex's T3 Chat reactive outage; the Local-First paper and CRDT practitioner
discussion; and Yjs's own modular/scaling documentation. Those sources support
the demand for executable restore, bounded cost, fail-small evidence, reactive
backpressure, and a complete offline protocol; they are not treated as evidence
that Ordo has suffered incidents it has not yet been capable of suffering.

Evidence that changes the verdict fastest:

1. Runnable guarded vertical slice: shapes, registered ops, ACL/guards,
   Postgres execution, command groups, and check/plan/apply.
2. Guard corpus on seeded Postgres with a no-guards canary.
3. Reproducible single-space contention benchmark and kill/restore drill.
4. Normative, model-checked client-first state machine.
5. Counsel-reviewed licensing FAQ.
6. Hard quota/budget surface alongside computational cost class.

## 7. Deterministic gates (separate from panel findings)

The `pre-review-round-3` deterministic run found **494 findings across five
failed checks**:

| Failed gate | Count | Severity | Meaning |
|---|---:|---|---|
| keyword parity | 15 | critical | Spec reserved words absent/present asymmetrically in parser table. |
| plan coverage | 2 | critical | Blind index and command log absent from build-plan pillar coverage. |
| rule coverage | 34 | major | Conformance rulings not referenced by owner specs. |
| marker coverage | 15 | minor | Required sections lack status markers. |
| absolute-claim registration | 428 | minor | Absolutes are unregistered in the claims register. |

Seven other gates passed: code-block lexical checks, links/anchors, retired
lexicon, error parity, glossary coverage, constants, and site checks. Separately,
`cargo test --workspace` passed **101 tests, 0 failures**. That proves only the
implemented fragments; there is still no binder/planner/SQL executor/migration
engine/Postgres log/replay store.

These deterministic matches are intentionally **not expanded into 494 semantic
Z findings**. They remain machine findings, separate from panel/formal IDs.

## 8. What every review agreed is strong

- Registered, enumerable, versioned operations instead of runtime query text.
- Derived review artifacts: mode, field/effect scope, cost, guards, cursor
  behavior, and consent inputs should be computed, not claimed.
- Create/converge/CAS/upsert/bulk semantics and insert completeness are unusually
  legible and reduce familiar partial-write failures.
- Conformance-first development remains the project's strongest discipline,
  even though two corpora now need migration to repaired semantics.
- Guard inlining and flattened composition address classic laundering holes;
  the fatal gap is execution/coverage, not the core idea.
- Space as the shared custody/membership/history/export boundary is coherent,
  and modeling guidance admits when it is the wrong boundary.
- The docs disclose hard limits: caller time, mutable cursors, offline retained
  copies, envelope leakage, server-authored order, and rejected OLAP use cases.
- Plain Postgres outputs, export direction, AGPL engine, Apache spec/corpora, and
  unrestricted generated output form a credible exit direction once executable.
- The cheap/strong experiment supports the AI-authoring wedge for syntax and
  demonstrates that the remaining dangerous failures cluster where docs/design
  cannot yet express a complete authority model.

## 9. Prioritized remediation

Doc/protocol rulings precede code. Owner decisions are separated so engineers do
not implement mutually incompatible fixes.

### P0 — owner decisions and normative protocol text

| # | Item | IDs | Effort |
|---|---|---|---|
| P0-1 | **RULE:** choose client-first trust model (all key holders trusted vs cryptographic compartments) and scope claims | Z-001 | M design / L implementation |
| P0-2 | **RULE:** define proposal receipt → validation → accepted-command state machine and independent verifier/trust assumption; include old-artifact proposals | Z-002, Z-016, Y-031 | M spec/model / L implementation |
| P0-3 | Replace output projection with componentwise authority manifest lattice | Y-005 | M |
| P0-4 | **RULE:** state whether ACL rows are private from owners; disclose universal override or design constrained recovery | Z-004 | S ruling / M design |
| P0-5 | Make step-up intrinsic to protected ops; specify suspension and authenticator-link ceremonies | Z-003, Z-005, Z-006 | M |
| P0-6 | Rule ACL create-guard ordering and whole-tenant bootstrap states | Y-003, Y-102, Z-020 | M |
| P0-7 | Fix implementability semantics: decimal, mixed/date duration, duration proof calculus, null arithmetic, nullable `in` | Y-006, Z-007, Y-016, X-012, Z-008 | M |
| P0-8 | Correct unsafe AGPL FAQ statement and stale README/plan status text | Z-015, X-002, X-001 | S (counsel review separate) |

### P0 — code/release gates after rulings

| # | Item | IDs | Effort |
|---|---|---|---|
| P0-9 | Implement end-to-end authorization vertical slice and repair keyword parity/plan coverage | X-001, X-075 | L |
| P0-10 | Thread space through executor and capability grant; space-scope invalidation/revocation | Y-002, Y-015, SH2-12/SH-30 | M |
| P0-11 | Add atomic replay claim/transaction; concurrent same-key test | Z-012 | M |
| P0-12 | Add migration change capture/watermark/barrier; kill-behind-cursor test | Z-011 | L |
| P0-13 | Separate stable uniqueness commitment from rotating search indexes | Y-009 | M |
| P0-14 | Repair guard and composition release corpora before calling them gates | X-018, X-059 | M |

### P1 — language/DX and operational proof

| # | Item | IDs | Effort |
|---|---|---|---|
| P1-1 | Track per-stage examined rows; fix cursor dependency/invalidation analysis | X-068, X-059, Y-017 | M |
| P1-2 | Finalize nullable params, UI form/detail/live grammar, member display, and org+guest recipe | Z-021, Z-022, Z-023, Y-026 | M |
| P1-3 | Add create-once/assert-same-payload and guarded cross-row ref eligibility | Z-018, Z-019 | M |
| P1-4 | Fix match plan/diagnostics, rollup owner doc, Gregorian lexer | Z-009, X-010, Z-010 | S/M |
| P1-5 | Make read result+log position one snapshot; repair genesis correction/revert | Z-013, Z-014 | M |
| P1-6 | Publish benchmark, restore/fault-injection transcript, and hard per-space budgets | X-003, X-072, X-073, Z-017 | M/L |

### P2 — carried security/ecosystem work

| # | Item | IDs | Effort |
|---|---|---|---|
| P2-1 | Harden sink egress against SSRF and derive/compile-check payload classes | Y-001 | M |
| P2-2 | Resolve denial-stream ownership/retention/collapse and migrate the losing document | Y-007, Y-008 | M |
| P2-3 | Require fresh nonce per sealed write; prove repeated plaintext yields distinct `SealedRef` | Y-012 | S |
| P2-4 | Complete group authority/no-escalation, conflict, expansion, and grammar semantics | Y-013, Y-014, Y-028, Y-037 | M/L |
| P2-5 | Space-scope correlation lookup/revert and make revert a guarded registered op | Y-032 | M |
| P2-6 | Carry sealed/refinable metadata into `FieldSpec` and add missing space-negative tests | Y-033, Y-034 | S/M |
| P2-7 | Rule transaction-pool-safe locks and null-index leakage disclosure/mitigation | Y-035, Y-036 | M |
| P2-8 | Synchronize stale engine-plan guard/routine/match rulings | Y-038 | S |

None is closed by round-3 attention shifting elsewhere.

## 10. Owner-resolvable open questions only

1. **Client-first trust:** Is every enrolled device/key holder trusted for every
   row in the space, or must hostile-member isolation survive client-first?
2. **Client-first validity:** What independent party/mechanism validates value-
   dependent guards/checks/CAS before a command becomes authoritative history?
3. **ACL privacy:** Is a space owner definitionally a universal reader, or can
   a row be private from the owner? What recovery contract follows?
4. **Decimal:** fixed fractional scale or a specified magnitude-dependent
   significant-digit algorithm? What exponent/storage bound applies?
5. **Bootstrap:** Is ACL creation a normative special case to pre-insert guards,
   and is “atomic tenant bootstrap” domain-row-only or one platform command?
6. **Owner recovery/step-up:** Which ops carry immutable minimum step-up, and
   what recovery can operate without a currently usable authenticator?
7. **Member identity surface:** Is display identity a field on platform members,
   a separate profile entity, or intentionally outside Ordo query data?

Everything else in the plan is a mechanical consequence of an existing ruling
or a missing implementation/evidence package; it should not be sent back to the
owner as an open-ended design question.
