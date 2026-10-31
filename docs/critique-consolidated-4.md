# [review/proposed] Ordo adversarial critique — round 4

> **Status: REVIEW / PROPOSED ONLY. Nothing in this document is adopted.**
> This report consolidates round-4 evidence and proposes priorities. A finding
> status is an audit disposition, not a silent change to the specification.

## 0. Fatal facts first

Five facts stop a production pilot now. They are not softened by the green
prototype tests or by strong design prose.

| ID | Fatal fact | Why adoption stops |
|---|---|---|
| **X-001** | Authorization does not execute | `acl` and `access` lex, but no parser/AST/binder/planner/executor implements guarded rows. A fake-injected `OpExecutor` cannot establish the central security claim. |
| **X-003** | There are no performance numbers or executed restore/DR results | The per-space serialization point, restore procedure, and alert inventory remain unevidenced. No adopter can price capacity or recovery. |
| **X-084** | There is no runnable end-to-end product | No installable release, CLI, planner, apply path, SDK, generated UI, or complete vertical slice exists. Four of four panelists rejected a production pilot. |
| **Y-005** | Consent is not represented by an authority-complete manifest | Field and row-scope comparisons omit effects, write modes, egress, key release, parameter influence, composition, and intrinsic step-up. An old grant can outlive changed authority, or every safe narrowing can fail closed with no promised semantic diff. |
| **AA-007** | The operation that creates every tenant is not representable in the current engine | `std.space.create` binds no space, but `OpSpec`, the router, `AccessChecker`, `CapabilityGrant`, and replay keys all structurally require one. The control-plane root is prose-only. |

The fatal set is after deduplication. **Independent reproduction is a severity
multiplier**: Y-005 was reproduced by SH4-001 and all four skeptics; AA-007 was
independently reached by C4-010, SH4-012, the build review, and all four
skeptics. Adjacent failures such as bootstrap actor identity and KMS recovery
remain separate because they have different counterexamples and fixes.

## 1. Method, normalization, and evidence boundary

Sources were read in the required order: README/glossary; round-3 critique,
security review, findings ledger, and deterministic report; then formal math,
formal CS, security, skeptic, and all three AI-build round-4 reports. Current
docs and crates were inspected to adjudicate closure. A test name was never
treated as proof without checking the behavior and interface it exercised.

Security review 4 uses `Critical`, `High`, and `Moderate`. This consolidation
keeps `Critical` as **Critical** unless independent adoption evidence raises an
already stable finding to Fatal. `High` maps to **Critical** when it permits
cross-space influence, durable account compromise, plaintext retention, or an
authority bypass; otherwise it maps to **Major**. `Moderate` maps to **Major**
for a cross-tenant existence/context oracle and **Minor** for bounded surface
enumeration. This is normalization into Fatal/Critical/Major/Minor/Info, not a
softening of the security review.

Statuses are exactly `open`, `closed`, `relocated`, or `carried`. `relocated`
means the old surface was repaired and a narrower failure now exists at a new
boundary. `carried` means round 4 did not materially change the prior hole.
Effort is **S** (≤1 day), **M** (≤1 week), or **L** (>1 week). `RULE` requires
an owner decision before code.

## 2. Round-3 fix audit

### Actually closed

- Client-first hostile-member confidentiality (**Z-001**) and owner-private ACL
  rows (**Z-004**) were closed by explicit product contraction, not
  cryptography: all client-first keyholders are trusted, and a space owner may
  reach every row. The disclosure must remain prominent.
- ACL-root creation ordering (**Y-003/Y-102**), ordinary capability space
  binding (**Y-002**), executor space propagation (**Y-015**), trigger
  provenance (**Y-004/Y-029**), op-intrinsic step-up (**Z-003**), Gregorian
  literals (**Z-010**), SHA-256 hashing (**X-029**), and field refinement
  metadata (**X-031/Y-033**) are real design/code repairs.
- The significant-digit rule (**Y-006/X-007**) was withdrawn. The replacement
  `NUMERIC(38,12)` regression found as M4-001 was also repaired after review by
  using unconstrained `numeric` plus `round(value,12)`.
- Nullable `in` (**Z-008/X-011/M4-006**), the illegal `members` fixture
  (**X-018**), cursor space binding (**C4-012/SH4-005**), and the contradictory
  blind-index glossary text (**C4-016/SH4-010**) were repaired during the
  round-4 no-brainer sweep.

### Relocated rather than closed

- Atomic replay's first-claim race (**Z-012**) is fixed in one in-memory
  generation; transaction ownership and stale-holder fencing are **AA-004** and
  **AA-005**.
- Client-first pre-acceptance validation (**Z-002**) was withdrawn, but the new
  signed-record, deterministic-fold, and “caught and undoable” claims fail as
  **AA-001–AA-003**.
- Suspension at the door (**Z-005**) moved into live/session lifecycle
  (**AA-022**); authenticator freshness (**Z-006**) moved into ceremony-bound
  proof (**AA-021**); random `SealedRef` representation (**Y-012**) moved into
  append enforcement (**AA-023**).
- Whole-tenant bootstrap prose (**Z-020**) moved into deployment scope,
  actor/beneficiary identity, KMS idempotency, and owner-doc contradictions
  (**AA-007–AA-009**, **AA-028**).
- Match-plan parity (**X-009/Z-009/Y-018**) moved into contradictory uniqueness
  rules and intractable diagnostics (**AA-011/AA-012**).
- Denial-event ownership (**Y-008**), DEK/undo-horizon handling (**Y-010**), and
  direct/group combination and revocation (**Y-013**) are settled in their
  owner documents but contradicted by the current first-read glossary. Their
  original owner defects closed; each is relocated as a major documentation
  contradiction.

### Still open or carried

Executable authorization, authority manifests, performance/restore evidence,
snapshot-positioned reads, online backfill capture, cursor proof scope, null
arithmetic, mixed durations, std rollup closure, app/profile grammar, nullable
operation parameters, hard budgets, observability, and export round-trip
evidence remain open. Plan edits for replay transaction/fencing are useful but
do not close **AA-004/AA-005** because the current traits still cannot carry the
promised transaction or generation. **X-068** also remains the same open defect:
although the returned/examined distinction was added, the universal gate still
admits `take` without proving it precedes the unbounded stage.

### Explicit prior stable-ID ledger

The following is the authoritative round-4 disposition of every retained X/Y/Z
ID. Titles and origins are preserved in `review/findings.json`.

| Status | IDs |
|---|---|
| **open** | X-001, X-068 |
| **carried** | X-003, X-010, X-012, X-051, X-059, X-070, X-071, X-072, X-073, X-078, X-084, X-085, X-086, Y-005, Y-017, Y-019, Y-020, Z-007, Z-011, Z-013, Z-014, Z-017, Z-021, Z-022 |
| **relocated** | X-008, X-009, X-039, X-058, X-060, Y-008, Y-010, Y-012, Y-013, Y-018, Y-031, Y-032, Z-002, Z-005, Z-006, Z-009, Z-012, Z-016, Z-020, Z-023 |
| **closed** | X-002, X-004, X-005, X-006, X-007, X-011, X-018, X-019, X-022, X-023, X-026, X-029, X-031, X-032, X-040, X-066, X-074, X-075, X-079, X-080, Y-001, Y-002, Y-003, Y-004, Y-006, Y-007, Y-009, Y-011, Y-014, Y-015, Y-016, Y-021, Y-022, Y-023, Y-024, Y-025, Y-026, Y-027, Y-028, Y-029, Y-030, Y-033, Y-034, Y-035, Y-036, Y-037, Y-038, Y-100, Y-101, Y-102, Y-103, Y-104, Y-105, Y-106, Z-001, Z-003, Z-004, Z-008, Z-010, Z-015, Z-018, Z-019 |

## 3. Detailed findings

### X-001 — Authorization is specified but does not execute · **Fatal** · status: open · orig: skeptic-panel round-4, prior X-001

- **Claim.** A closed language derives and enforces row authorization.
- **Counterexample.** `acl`/`access` are lexer keywords, but the AST/parser have
  no entity/guard productions and the only API executor is injected/test fake;
  no implementation can raise `access_guard` from a compiled guard.
- **Smallest fix.** Ship one Postgres-backed vertical slice from parsed shape to
  guard IR, inlining, guarded read/write, command group, and release corpus.
- **Constraints.** Preserve one door, non-enumerating reads, loud write denial,
  composition flattening, and the narrow G10a exception.

### X-003 — Performance and recovery remain evidence-free · **Fatal** · status: carried · orig: skeptic-panel round-4, prior X-003/X-072

- **Claim.** Per-space chains, encrypted storage, migrations, live queries, and
  restore procedures are operable.
- **Counterexample.** No workload, writes/sec, p99, hot-space profile, timed
  restore, fault transcript, RPO, or RTO result exists.
- **Smallest fix.** Publish a reproducible topology/workload plus kill/restore
  transcript executed by a second operator.
- **Constraints.** Do not replace measured tails with averages or treat backup
  presence as successful recovery.

### X-084 — The platform outcome is not runnable · **Fatal** · status: carried · orig: skeptic-panel round-4 4/4

- **Claim.** Definitions yield APIs, guarded data, routines, and generated UI.
- **Counterexample.** There is no installable binary, planner, apply command,
  SDK, generated UI, or tutorial that executes end to end.
- **Smallest fix.** A pinned local release that builds one guarded multi-user
  app, shows a denial, performs a command, and exports/restores it.
- **Constraints.** Keep the README's pre-alpha honesty until the artifact exists.

### Y-005 — Grant manifests are not authority-complete · **Fatal** · status: carried · orig: SH4-001, skeptic-panel round-4 independent reproduction

- **Claim.** A deploy can decide semantic narrowing and retain consent safely.
- **Attack path.** Keep fields and row guards unchanged while adding an outbox
  write, delete, sealed-key release, egress host, weaker parameter influence,
  new composed callee, or lower step-up. Current field/row shape misses it;
  opaque hash equality cannot classify narrowing.
- **Smallest fix.** A versioned canonical `AuthorityManifest` ADT covering op
  kind, params/influence, reads/writes/deletes, guards/bypasses, output, sealed
  release, effects/egress, step-up, cost, trigger provenance, and flattened
  composition, with a componentwise partial order.
- **Constraints.** Per-op storage, collision-resistant canonical encoding,
  exact space scope, expiry, fail-closed incomparable changes, and silent only
  for proved narrowing.

### X-068 — The global row-work gate still admits take as evidence for examined-row bounds · **Major** · status: open · orig: DA4-001, M4-010

- **Claim.** The universal registration gate prevents accidental O(space) work.
- **Counterexample.** The spec now says `take` bounds returned rather than
  examined rows, yet its global evidence list still admits `take` without
  proving early termination before the unbounded stage; the glossary repeats
  the same generic evidence rule.
- **Smallest fix.** Remove bare `take` from global bound evidence. Admit it only
  when the planner proves an access path can stop before unbounded work; bound
  partition count separately for partitioned take.
- **Constraints.** Preserve the useful returned/examined distinction and do not
  relabel one-row output as one-row work.

### Y-008 — Denial-event ownership is settled in owner documents but contradicted by the glossary · **Major** · status: relocated · orig: DA4-002, expert F2, SH2-8

- **Claim.** Denials live in the separate per-space security-event stream and
  never enter the command log.
- **Counterexample.** Security logging, dashboard, and implementation owners
  agree, while the glossary still says the destination is contradicted and a
  chained stream is merely proposed.
- **Smallest fix.** Synchronize the glossary to the settled owner contract and
  separate current stream ownership from any future chaining proposal.
- **Constraints.** Preserve denial visibility without letting unauthenticated
  attempts contend on or append to the command chain.

### Y-010 — DEK rotation and undo-horizon image handling are settled in owner documents but contradicted by the glossary · **Major** · status: relocated · orig: DA4-003, expert F4

- **Claim.** Epoch keys survive while any command image is inside
  `undoHorizon`; older unresolved images receive an explicit marker.
- **Counterexample.** Implementation, security logging, and std owner docs say
  this, but the glossary still calls the old-image question unsettled.
- **Smallest fix.** Replace the stale glossary sentence with the retained-key
  and beyond-horizon marker rule; keep SMK reindex linearization as the distinct
  open AA-013 issue.
- **Constraints.** Do not conflate DEK epoch retention with SMK blind-index
  rotation.

### Y-013 — Direct/group combination and revocation are settled in owner documents but contradicted by the glossary · **Major** · status: relocated · orig: DA4-004, expert F6, SH2-11

- **Claim.** Effective direct/group level is the maximum surviving path and
  revocation removes only the named path.
- **Counterexample.** Definition/security owners agree, but the glossary still
  tells first readers that differing levels and per-person revocation are
  unruled.
- **Smallest fix.** Copy the maximum-level/surviving-path rule into the glossary
  and retain the explicit-group-hop warning separately.
- **Constraints.** Do not introduce group nesting or make a group lower a
  principal's direct role.
- **Closed 2026-09-05.** Ruled in `corpus-rulings.md` — the more permissive
  level wins and the audit reports both paths — with the glossary's
  *"avoid granting the same principal both directly and through a group"*
  replaced by the rule. Both constraints held: `group_nesting` is now
  produced rather than merely documented, and the `read`-direct/`admin`-group
  and `admin`-direct/`read`-group orderings are both asserted against real
  rows, so a group demonstrably cannot lower a direct grant.

### AA-001 — Client-first signs incompatible proposal headers · **Critical** · status: open · orig: C4-001, Y-031, Z-002

- **Claim.** A device signature covers the proposal's believed predecessor and
  survives canonical server re-anchoring.
- **Counterexample.** D2 signs `prev=H0`; after D1, canonical `prev=H1`. Covering
  `prev` invalidates D2's signature; excluding it cannot prove D2's belief and
  makes re-signing meaningless.
- **Smallest fix.** Device-signed immutable proposal `(payload digest, device,
  counter, believed_prev, artifact)` plus server-signed receipt `(proposal hash,
  canonical position, canonical_prev)`; chain commits to both.
- **Constraints.** No post-upload device liveness dependency; preserve opaque
  sequencing and attribution.

### AA-002 — “Caught and undoable” lacks a validator and truthful inverse · **Critical** · status: open · orig: C4-002, Z-002

- **Claim.** Invalid client-first commands are attributed, caught, and undoable.
- **Counterexample.** A malicious member submits an invalid transition with a
  false before-image. The blind server cannot classify the failure; generic
  revert swaps attacker-supplied images and restores the wrong predecessor.
- **Smallest fix (RULE).** Define every invariant failure as a canonical no-op
  or derive effect/inverse from prior canonical state; name the honest validator
  or quorum assumption and certificate.
- **Constraints.** Invalid proposals may remain attempt history, never truthful
  state history; correction must progress after device loss or the claim must
  be narrowed.

### AA-003 — Client-first proposals omit evaluator/schema identity · **Critical** · status: open · orig: C4-003, Z-016

- **Claim.** All devices deterministically fold one canonical stream.
- **Counterexample.** D1 proposes under A1 after A2 changes a default, check, or
  encoding. The signed header carries no artifact/schema hash; D1 and D2 may
  derive different outcomes from identical bytes.
- **Smallest fix.** Sign artifact and schema hashes; retain/fetch verified old
  evaluators; reject new proposals under expired artifacts while preserving
  replay of already accepted ones.
- **Constraints.** Version retirement cannot make old history uninterpretable.

### AA-004 — Replay completion cannot share the domain/log transaction · **Critical** · status: **partially closed 2026-08-14**

> **Update.** The domain write and the chain append now share one
> transaction (`de25eda`): the write path talks to a `WriteDb` seam the
> executor fills with an open transaction, `SELECT … FOR UPDATE` precedes
> guard evaluation, and any refusal rolls the group back — no partial
> effect, no command group appended. What survives of this finding is the
> **third leg**: `ReplayCache::put` is still a post-return call, so a
> process dying between commit and completion leaves a claimed key with a
> committed effect and no recorded outcome. That remainder is tracked with
> AA-005 (fencing), which shares its fix.

The original finding, unedited: · orig: C4-004, SH4-002 independent reproduction

- **Claim.** Domain mutation, command append, and replay completion commit once.
- **Counterexample.** `OpExecutor::execute` returns before `ReplayCache::put`;
  neither trait carries a common transaction. A crash between them commits the
  write but leaves a retry free to execute again.
- **Smallest fix.** One execution-owned unit of work takes the replay lease,
  mutates domain state, derives images, locks/appends the chain, stores the
  response, and finalizes replay in one transaction.
- **Constraints.** Router sees only a committed result; BYO cross-store designs
  must state an inbox/outbox contract rather than call it atomic.

### AA-005 — Replay takeover is unfenced · **Critical** · status: open · orig: C4-005, SH4-002 independent reproduction

- **Claim.** A stale reservation can be taken over without duplicate effects.
- **Counterexample.** T2 replaces expired T1; late T1 then abandons or overwrites
  T2's slot because completion identifies only the key. T3 can execute a third
  copy.
- **Smallest fix.** `claim` returns a monotonic/unguessable generation; renew,
  finalize, abandon, and domain commit compare-and-set that generation.
- **Constraints.** A fenced stale holder can neither change replay state nor
  commit domain effects.

### AA-006 — “Permanent” create binding is neither permanent nor replay-equivalent · **Major** · status: open · orig: C4-006

- **Claim.** A creating idempotency key permanently binds payload and returns
  the first result.
- **Counterexample.** Binding installs after execution, is capacity-evicted,
  and same-payload retries execute after response TTL, producing duplicate or
  different outcomes.
- **Smallest fix.** Persist `(scope,principal,op,key,fingerprint,row,outcome)` in
  the creation transaction; same payload returns the stored outcome without
  execution, different payload always mismatches.
- **Constraints.** No LRU eviction of the semantic binding.

### AA-007 — Deployment-scoped creation is unrepresentable · **Fatal** · status: open · orig: C4-010, SH4-012, skeptic-panel round-4

- **Claim.** `std.space.create` is an ordinary generated `scope deployment` op.
- **Counterexample.** The router, `AccessChecker`, executor, capability grant,
  and replay key all require `SpaceId`; no space exists yet. A dummy space turns
  one tenant into authority over every tenant.
- **Smallest fix (RULE).** Closed `OpScope::{Deployment,Space}` plus typed
  authorization/replay/execution targets; distinct deployment grants/audience;
  reject `$space` on deployment ops.
- **Constraints.** Never weaken exact-space binding or add wildcard space grants.

### AA-008 — Bootstrap actor and owner cannot both be the ACL creator · **Critical** · status: open · orig: SH4-012, skeptic-panel round-4, BJ4

- **Claim.** Genesis records the real operator/system actor and ordinary creator
  semantics grant the intended owner every root ACL.
- **Counterexample.** The normative creator is the attributed principal's member
  row. `system:bootstrap` or `user:<operator>` is not the separately minted
  owner, so the asserted owner grant does not follow.
- **Smallest fix (RULE).** Separate `actor` from an explicitly signed
  `beneficiary`, or require owner-attributed/delegated creation; encode one
  narrow genesis grant primitive.
- **Constraints.** Preserve honest attribution, one initial owner, no
  caller-supplied grant list, and non-callable provisioning.

### AA-009 — KMS creation can escape the registry · **Major** · status: open · orig: C4-011

- **Claim.** Every bootstrap orphan is reachable from the registry row.
- **Counterexample.** KMS creates K and returns handle h; the worker dies before
  persisting h. Registry S cannot distinguish absent K from unreachable K; a
  retry creates K2.
- **Smallest fix.** Persist a deterministic external idempotency token before
  KMS call; require create/lookup-by-token to return the same handle.
- **Constraints.** Keep the provisioning saga; do not require distributed ACID.

### AA-010 — Cross-space correlation is an existence oracle · **Major** · status: open · orig: C4-013, SH4-007 independent reproduction

- **Claim.** `CrossSpace` is a safe loud response after scoped lookup misses.
- **Attack path.** Probe leaked correlation ids in space A. `CrossSpace` means
  the id exists in another tenant; `UnknownGroup` means nowhere.
- **Smallest fix.** Uniform scoped `UnknownGroup`; any privileged global check
  happens after response and writes only an internal security event.
- **Constraints.** Preserve scoped images, per-space uniqueness, attribution,
  and rate limits.

### AA-011 — Match uniqueness rules contradict each other · **Major** · status: open · orig: M4-014

- **Claim.** Inference uses exactly one path, direct-edge preference, and unique
  shortest path; adding a bridge can only remove ambiguity.
- **Counterexample.** `A-B` plus `A-C-B` is rejected by “one path” but accepted
  by the other two. Adding C to triangle A/B/C creates three minimum trees.
- **Smallest fix.** One minimum-edge rule; delete exact-one-path and monotonicity
  claims; explicit bridge/path syntax where source listing is intended to pick.
- **Constraints.** Deterministic, schema-local, and teachable diagnostics.

### AA-012 — Match inference requires intractable work/output · **Major** · status: open · orig: M4-015, C4-014 independent reproduction

- **Claim.** The compiler enumerates every ambiguous path/tree.
- **Counterexample.** Forty diamond layers have `2^40` shortest paths; the
  multi-terminal rule is general Steiner tree and NP-hard.
- **Smallest fix.** BFS counting saturated at two for two sources; bound terminal
  count/graph size or require explicit bridges; report two witnesses and omit
  the rest.
- **Constraints.** Never make diagnostic size proportional to all alternatives.

### AA-013 — SMK reindex has no write linearization · **Critical** · status: open · orig: C4-015

- **Claim.** A shadow blind-index column can be scanned then atomically swapped.
- **Counterexample.** A write lands behind the scan only in the old digest;
  after swap, reinserting that plaintext finds no new digest and violates
  logical uniqueness.
- **Smallest fix.** Freeze writes through scan/verify/swap, or dual-write from a
  watermark, drain under a barrier, verify, then switch key and constraint.
- **Constraints.** DEK rotation remains independent; old/new namespaces must not
  both be query-authoritative.

### AA-014 — Rounding mode is mandatory but unspellable · **Major** · status: open · orig: M4-002

- **Claim.** Authors choose half-even versus half-up explicitly.
- **Counterexample.** Only `round(x)` and `round(x,n)` exist; `round(2.5,0)`
  cannot express which valid policy returns 2 or 3.
- **Smallest fix (RULE).** Required closed mode argument, or one global pinned
  mode and removal of author-choice claims.
- **Constraints.** Cross-venue goldens at signed half ties.

### AA-015 — Month/week buckets have no types · **Major** · status: open · orig: M4-004

- **Claim.** `month` and `week` return sortable, composable scalars.
- **Counterexample.** The closed type inventory defines neither representation
  nor JSON/parameter comparison for those bucket keys.
- **Smallest fix.** Return existing canonical dates (month start/ISO Monday) or
  define expression-only types with wire encodings.
- **Constraints.** Same grouping/order in host, SQL, SDK, and descriptors.

### AA-016 — Duration guidance contradicts its owner rule · **Minor** · status: open · orig: M4-005

- **Claim/counterexample.** `traps.md` both permits calendar/calendar ordering
  and says only `duration(ms)` is orderable.
- **Smallest fix.** Replace the stale “same unit class” text with the exact
  declaration-site type rule.
- **Constraint.** Do not reopen the now-reproducible Y-016 type repair.

### AA-017 — Anchor id does not total-order multi-source rows · **Critical** · status: open · orig: M4-008

- **Claim.** Appending anchor id makes every order total.
- **Counterexample.** One customer joined to two orders produces two rows with
  identical `(sort keys, customer.id)`.
- **Smallest fix.** Carry a stable grain identity tuple containing all necessary
  source ids; reject pagination if injectivity cannot be proved.
- **Constraints.** Preserve single-source/group-key cases and hide internal ids
  from ordinary output when not projected.

### AA-018 — Derived count violates the unbounded gate · **Critical** · status: open · orig: M4-011

- **Claim.** No O(space) op registers silently; `std.E.count` needs no marker
  because it returns one row.
- **Counterexample.** Exact count over 100M uncapped rows examines 100M entries.
- **Smallest fix (RULE).** Require a declared cap/deployment opt-in recorded like
  `unbounded`, or retract the universal theorem and expose the exemption.
- **Constraints.** One row returned must never masquerade as one row examined.

### AA-019 — std whole is both paged and truncated · **Major** · status: open · orig: M4-012

- **Claim.** Every collection is `{items,count,next}` default 100; another rule
  says capped with `truncated`, while the cap remains open.
- **Counterexample.** An invoice with 101 lines has two incompatible result
  types and no settled registrability bound.
- **Smallest fix.** Choose one envelope and fixed/policy-recorded cap; remove the
  other contract.
- **Constraints.** Generated SDK and UI type must expose omitted rows honestly.

### AA-020 — allocate is partial over its declared domain · **Major** · status: open · orig: M4-016

- **Claim.** Integer weights always allocate parts summing to total.
- **Counterexample.** Empty, `(0,0)`, and `(1,-1)` have no proportional share.
- **Smallest fix.** Nonempty, every weight ≥0, sum >0, one typed error; define
  negative-total sign handling.
- **Constraints.** Preserve deterministic largest-remainder ties.

### AA-021 — Authenticator linking is not ceremony-bound dual proof · **Critical** · status: open · orig: SH4-003, Z-006

- **Claim.** Fresh presence on an existing factor authorizes adding a new one.
- **Attack path.** A stolen bearer reuses a recent generic presence timestamp
  and names a known credential id; no old-factor signature binds this new key.
- **Smallest fix.** One-use server challenge binding principal, exact old
  credential, new public key/provider result, RP/client, nonce, and expiry;
  verify both proofs atomically.
- **Constraints.** Separate delayed recovery, notify old factors, rotate refresh
  families, preserve zero-factor first-link exception.

### AA-022 — Live invalidation crosses tenants and revocation · **Critical** · status: open · orig: SH4-004, Z-005

- **Claim.** Live subscriptions are scoped refetch signals and suspension closes
  access.
- **Attack path.** Bus channels are keyed only by op. Space A observes/amplifies
  B's activity; a pre-suspension stream continues indefinitely.
- **Smallest fix.** Key events/subscriptions by `(space,op,artifact/dependency)`;
  structurally include space; bind connection to revocation and periodically or
  eventfully recheck.
- **Constraints.** Preserve subscribe-before-read, coalescing, heartbeat, and
  refetch-through-scope.

### AA-023 — Sealed log redaction is unenforced · **Critical** · status: open · orig: SH4-006, Y-012

- **Claim.** Sealed plaintext cannot enter retained command images because the
  rule lives in the type.
- **Attack path.** `Image` accepts `Value::Text`; append has no schema/manifest
  and permanently hashes/archives a compromised executor's plaintext.
- **Smallest fix.** Private-constructor, artifact-bound `LogImage`; only a
  schema-aware redaction boundary can create it, with fresh opaque handles for
  sealed fields.
- **Constraints.** Preserve canonical encoding, full open images, random
  non-content-derived handles, and crypto-erasure.

### AA-024 — Import quiet mode is caller-selectable authority · **Critical** · status: open · orig: SH4-008

- **Claim.** Genesis import may suppress listener triggers safely.
- **Attack path.** A stolen import credential sets the quiet flag after genesis,
  bypassing normalization, alarms, effects, or guard expectations.
- **Smallest fix.** Intrinsic-step-up `std.space.import.begin` creates a one-use
  session bound to mapping/source hashes, fields, limits, actor, state, and
  expiry; only executor derives quiet.
- **Constraints.** Explicitly state checks/guards that still run and the one
  baseline listener run that replaces suppression.

### AA-025 — Pre-door errors enumerate op names · **Minor** · status: open · orig: SH4-013

- **Claim/counterexample.** Registered ops return method mismatch or
  `missing_space` before bearer authorization; unknown names return
  `access_denied`, revealing name and kind.
- **Smallest fix.** Validate generic bearer/scope before registry-specific
  errors; unknown and unauthorized stay identical.
- **Constraint.** Preserve method mirroring after authorization and no default
  space.

### AA-026 — Enum is half-defined · **Major** · status: open · orig: BJ4-003

- **Claim/counterexample.** The type inventory is closed without enum; a lean
  field proposal exists; no enum parameter type exists. The strong build uses
  both.
- **Smallest fix (RULE).** Settle field+parameter enum grammar, migration and UI,
  or teach `text + check + choices` as the buildable fallback.
- **Constraint.** One type inventory owns wire and form behavior.

### AA-027 — ACL-derived and explicit root guards have no algebra · **Major** · status: open · orig: BJ4-005

- **Claim/counterexample.** Strong combines `acl` with explicit guards on that
  same root, but replacement is specified only for descendants; replace,
  compose, conflict, and reject yield different responder write authority.
- **Smallest fix (RULE).** Define root coexistence or reject it and publish the
  canonical membership vocabulary that achieves the intended levels.
- **Constraint.** Never silently choose the more permissive interpretation.

### AA-028 — Bootstrap owner documents contradict one another · **Major** · status: open · orig: BJ4-006, skeptic-panel round-4

- **Claim/counterexample.** `/op` versus `/ops`; `apps[]` versus required OIDC
  `clients[]`; always-managed conversion versus selectable native client-first;
  no custody default versus policy defaults; settled scope versus bootstrap
  open items all coexist.
- **Smallest fix.** One RULE table owned by `bootstrap.md`, mechanically linked
  by API/language/UI docs; one executable golden request.
- **Constraints.** Keep apps distinct from clients and deployment grants
  distinct from space grants.

### AA-029 — Group wording causes silent authorization denial · **Major** · status: open · orig: BJ4-009 cheap+strong independent reproduction

- **Claim.** Onboarding broadly says guards expand groups.
- **Counterexample.** Hand-written `member == $caller` never expands; both AI
  tiers legally grant groups then silently deny their humans in explicit note
  or organization guards.
- **Smallest fix.** Teach the exact explicit disjunct
  `member.groupMembers.group(member == $caller)` in the first group recipe and
  reserve “automatic expansion” for ACL-derived guards.
- **Constraints.** Keep op-body equality exact and groups non-nested in v1.

### AA-030 — Managed offline writes have no durable queue contract · **Major** · status: open · orig: BJ4-017 cheap+strong independent reproduction

- **Claim.** Two managed-custody devices can edit offline and reconcile by CAS.
- **Counterexample.** CAS defines server conflict only; no durable client queue,
  op/artifact pin, retry order, idempotency ownership, schema skew, or user
  conflict lifecycle is specified. Client-first supplies those mechanics but
  forbids the brief's guest/server-routine combination.
- **Smallest fix.** Specify a managed queue record and replay protocol with
  version/rev/idempotency keys and explicit conflict presentation.
- **Constraints.** No silent merge and no accidental promotion to client-first.

### AA-031 — Consumer eviction voids at-least-once delivery · **Major** · status: open · orig: V-002

- **Claim/counterexample.** Trigger delivery is settled at-least-once, but a
  stalled consumer may be evicted and its unread range pruned.
- **Smallest fix (RULE).** Either make eviction an explicit delivery-loss event
  that changes the guarantee, or retain/replay through an owner-approved floor.
- **Constraint.** Retention policy must not silently rewrite delivery semantics.

### AA-032 — Client-first authoritative segments have no compaction rule · **Major** · status: open · orig: V-008

- **Claim/counterexample.** Server mirrors are described as evictable, but the
  client-first device store is authoritative and accumulates canonical/rejected
  proposal history without a safe compaction/purge protocol.
- **Smallest fix (RULE).** Define checkpoint, tombstone, proof-retention, offline
  device, and poisoned-command compaction semantics.
- **Constraint.** Preserve verifiability and prevent long-offline resurrection.

### AA-033 — Unreadable-parent create errors conflict · **Major** · status: open · orig: V-010

- **Claim/counterexample.** The spec makes an unreadable parent `not_found`; the
  guard corpus expects `access_guard`, and each claims normative ownership.
- **Smallest fix (RULE).** Choose non-enumeration or loud guard rejection and
  update owner, corpus, API error mapping, and tests together.
- **Constraint.** The result must not reveal parent existence to unauthorized
  callers.

### Carried material findings not duplicated above

| IDs | Claim/counterexample | Smallest fix | Constraint |
|---|---|---|---|
| X-010 / Y-019 | Std rollup still says “same closure as whole” and uses the withdrawn one-root proof. | Copy the strict per-anchor closure into `std-library.md`; add duplicate-far-side conformance. | Do not change display `whole` while fixing aggregation. |
| X-012 | Legal null arithmetic has no single host/SQL result. | Strict null propagation except named null-eliders; typed boundary rejection. | Cross-venue goldens. |
| X-051 | One doc settles external attestation while implementation marks it proposed and ships only `StubSigner`. | One owner maturity marker plus signed-head/chainProof implementation. | Detect-not-prevent and start-time limits remain explicit. |
| X-059 / Y-017 | Stable cursors exclude std writers and eligibility changes while promising unconditional no-skip/no-duplicate. | Rename current label or prove all executable writers, creates/deletes, filters/guards, and deploy changes; otherwise snapshot. | Preserve honest `drifting`. |
| X-070 | Request refinements lack registered cost/unbounded/index metadata in `OpSpec`. | Carry planner cost proof into artifact/door. | Do not weaken sealed equality-only checks. |
| X-071 / Z-011 | Transforming backfill misses writes behind scan and after verification. | Watermark + dual-write/CDC + barrier + atomic routing swap. | Old/new artifacts coexist safely. |
| X-072 / X-073 | Restore and observability are procedures/proposals, not evidence. | Execute drills and ship metrics/alerts. | Report actual failures and tails. |
| X-078 / Z-021 / Z-022 | Nullable params and app/form/detail/live/profile/guest grammar remain non-normative. | One compile-owned UI/profile surface and clearing/null rules. | Generated security exposure cannot live in sidecar YAML. |
| Z-007 | Mixed `(months,ms)` action/order and `date ± duration` type remain absent. | Months-first operator table; sub-day date arithmetic rejects or yields explicit datetime conversion. | Static result type. |
| Z-013 | READ COMMITTED and current `ExecResult` cannot carry one exact snapshot/head. | One statement or repeatable snapshot plus typed snapshot proof; batch or bounded snapshot token for screens. | Replica reports its applied position. |
| Z-014 | Import correction plus delete-by-minted-id is not an inverse. | Create-only genesis or per-row created/updated/unchanged ledger with CAS rollback. | Never delete a pre-existing/interveningly edited row. |
| Z-017 | Cost class is computational venue, not quota/fairness. | Hard per-space budgets for storage, egress, queue, live fanout, routines. | Keep the computational lattice. |

## 4. Reviewer disagreements and mutually weakening fixes

| Topic | Position A | Position B | Consolidated ruling |
|---|---|---|---|
| Client-first trust | Round-3 framing treated shared keys as a fatal confidentiality defect. | CS/security/panel accept mutually trusting keyholders as an honest product contraction. | Z-001 closed only while consent states it plainly. Do not “repair” it by implying guards are cryptographic. |
| Client-first validity | Security review accepts detected-and-revertible as the contracted model. | Formal CS proves the validator, inverse, signed record, and evaluator are still undefined. | Keep the contraction, but AA-001–003 remain open. Attribution is not validation. |
| Bootstrap saga | One distributed ACID transaction would be simple to state. | Formal CS correctly says a durable provisioning saga is the right model for KMS/BYO stores. | Keep the saga; add typed deployment scope and external idempotency. Do not weaken exact-space grants. |
| Cursor semantics | A weaker drifting label is useful and honest. | “Stable” currently claims an unconditional theorem after excluding writers. | Keep drifting; narrow/rename stable or implement a complete proof/snapshot. |
| Std count | Product may want count everywhere without author ceremony. | Formal math says derived ops receive no mathematical exemption from examined-row cost. | Owner must choose explicit deployment opt-in or retract the universal no-accidental-O(space) claim. |
| Group equality | Automatic expansion is ergonomic. | Exact equality keeps query/write semantics simple and bounded. | Keep exact equality; repair onboarding with the explicit membership hop. Silent automatic expansion would weaken reviewability. |
| Owner ACL reach | Quorum recovery could preserve private-from-owner rows. | Current product defines owner as universal custodian. | Keep the chosen contract unless product scope changes. Adding quorum casually would break single-owner bootstrap and exit. |

## 5. Document-versus-document contradictions

| Subject | Document A | Document B | Winner / RULE owner | Finding |
|---|---|---|---|---|
| Grant authority | `definition-language.md:2445–2487`: field + row-scope manifest | `security-logging-design.md:138–150`: ordered output-field subset; crates store opaque combined hash | **RULE:** one authority-complete ADT owned by security/access docs; both current partials lose | Y-005 |
| Bootstrap route | `api-engine-design.md:18–21`: `/op/{name}` | `bootstrap.md:465`: `/ops/std.space.create` | API route owner wins; repair bootstrap golden | AA-028 |
| Deployment scope maturity | `api-engine-design.md:65–75`: settled exemption | `bootstrap.md:624–632,704–706`: grant/wire scope open | **RULE:** bootstrap owns product decision; API/code follow one typed scope | AA-007/028 |
| Bootstrap actor | `definition-language.md:1259–1313`: creator is attributed member | `bootstrap.md:173,183–191`: operator/system actor but owner gets creator grant | **RULE:** explicit actor/beneficiary primitive | AA-008 |
| Genesis mode | `bootstrap.md:211–225`: always managed then convert | `bootstrap.md:675–700`: native client-first also settled/selectable | **RULE:** choose supported protocols and one status marker | AA-028 |
| Custody defaults | `bootstrap.md:21`: required, no default | `definition-language.md:2803–2808`: policy defaults | Required explicit consent wins unless owner changes product contract | AA-028 |
| Exact read position | `implementation.md:905–920`: exact position and shared screen point | `implementation.md:215` READ COMMITTED; `ExecResult` has no proof | Snapshot carrier/mechanism wins; otherwise narrow claim | Z-013 |
| Cursor stability | `ui-gen.md:42–45`: total order means cursors are always stable | `definition-language.md:1709–1745`: writable keys are `cursor: drifting`, and composed post-`take` pagination always drifts | Language owner wins; total order prevents ties, not row movement | X-059/AA-017 |
| Live scope | command/access model is per space | `InvalidationBus::subscribe(op)` and event `{op}` omit space | Per-space invariant wins | AA-022 |
| Whole collection | `std-library.md:60`: paged `{items,count,next}`, default 100 | `std-library.md:438–450,587–591`: capped/truncated, cap open | **RULE:** choose one typed envelope | AA-019 |
| Rollup closure | `definition-language.md:711–743`: strict closure, old proof withdrawn | `std-library.md:347–358`: same as whole, one-owner proof | Language owner wins; migrate std | X-010/Y-019 |
| Match uniqueness | `definition-language.md:1553–1604`: exact-one, direct, shortest, monotonic bridge | These predicates disagree on simple counterexamples | **RULE:** unique minimum-edge rule | AA-011 |
| Group guards | `definition-language.md:851–890`: equality never expands; explicit hop | tutorial/glossary broadly say guards expand groups | Normative exact spelling wins; repair onboarding | AA-029 |
| Unbounded work | `definition-language.md:1786–1817`: examined-row gate | same section still lists `take`; std exempts count as reduce | Examined-stage proof wins | X-068/AA-018 |
| Parent error | language/API favor non-enumerating `not_found` | guard corpus expects `access_guard` | **RULE:** security owner decides, then one source | AA-033 |

Rows marked RULE cannot be repaired safely by editorial preference alone.

## 6. AI buildability comparison

| Dimension | Cheap tier | Strong tier |
|---|---|---|
| Written-spec compile | **Fail.** `let` binds converge-or-create (`unbindable_write`), all growing lists/routine omit `unbounded`, and app/form is illustrative. | **Core close but uncertain; complete artifact fails.** Enum parameters, ACL+root guard coexistence, and routine `filled` are unsettled; UI/profile YAML is not grammar. |
| Idiomaticity | Independent incident ACL roots detach organization authority; caller-supplied groups and creator auto-admin are not an Ordo-safe tenant model. | One space/org, ACL root, owned incidents, explicit guest link, private-note replacement, CAS, create-once, id sort, and managed custody are strongly Ordo-shaped. |
| Round-3 improvement | Fixed `displayName`, nullable-param invention, id-sort stability, both guard verbs, and member refs. | Fixed lexical formatting, G10a usage, create-once, assignee eligibility, stable paging, trigger provenance, and overdue lifecycle. |
| Round-3 regression | New loud `let` misuse; group-based note access silently denies the intended responders. | New dependence on lean/unspecified enum field+parameter forms; app names are passed as identity clients. |
| Silent-wrong risk | Still fatal: organization authority is disconnected; creator becomes incident admin; arbitrary groups/assignees/bylines; guest can receive multiple incidents; changed payload converges. | Lower but material: group-backed explicit guards deny; security-critical guest/profile exposure lives in YAML; root-guard algebra is undefined; managed offline delivery is assumed. |
| Brief coverage | Missing real tenant genesis, exact-one guest, trustworthy authorship, create-once, live root detail, complete UI. | Backend mostly covered; generated UI/profile/invite/client binding and durable managed-offline delivery remain incomplete. |

Responsibility split:

- **Author faults:** cheap's independent incident ACL, caller-supplied authority,
  spoofable `$by`, arbitrary assignees, stale overdue lifecycle, missing root
  live subscription; strong's app-name/client confusion and elective enum risk.
- **Mixed:** cheap `let`/unbounded/create-once failures because the full spec has
  rules but onboarding omits or contradicts them; both tiers' group denial
  because normative text is precise but onboarding overgeneralizes.
- **Docs/design faults no author can close:** complete app/profile/guest grammar,
  nullable params, root ACL+explicit-guard algebra, deployment scope/actor
  bootstrap contract, managed offline queue, and current runtime absence.

Both tiers correctly used implicit id/rev, CAS for ordinary edits, owned child
modeling, both guard verbs, `displayName`, sealed storage separate from access,
UTC clock routines, and immutable-id ordering. Do not break those gains.

## 7. Skeptic panel verdict and sources

**Verdict: 0/4 production pilots.** One panelist would accept only a synthetic,
single-owner managed lab. Four of four cite no runnable product, non-executable
authorization/consent, and contradictory genesis scope/identity. Three of four
also require performance/failure/restore evidence. This is a colder result than
round 3's two design partners despite genuine spec improvements.

The panel's source ledger includes the [Supabase February 2026 regional
incident](https://supabase.com/blog/supabase-incident-on-february-12-2026), a
[72-hour restore-support failure report](https://github.com/orgs/supabase/discussions/39896),
[Firebase billing amplification](https://news.ycombinator.com/item?id=42732714),
[Cloudflare's June 2025 dependency cascade](https://blog.cloudflare.com/cloudflare-service-outage-june-12-2025/),
[Convex migration-away guidance](https://www.youtube.com/watch?v=AlHnXJLfz5Y),
and practitioner reports on [local replica corruption](https://news.ycombinator.com/item?id=38248900)
and [long-offline resurrection/poison](https://news.ycombinator.com/item?id=44473590).
These justify demands for evidence; they are not claims that unbuilt Ordo has
experienced those incidents. Full source treatment is in
[`review/skeptic-panel-round-4.md`](../review/skeptic-panel-round-4.md).

## 8. Deterministic gates — separate from semantic findings

The **pre-review-round-4** deterministic baseline found **703 findings across
four failed checks**:

| Failed check | Count | Severity | Meaning |
|---|---:|---|---|
| Rule coverage | 36 | major | Conformance rulings lacked owner-spec references. |
| Site external requests | 4 | major | The site requested the Apache ICLA PDF externally. |
| Marker coverage | 17 | minor | Required sections lacked recognized maturity markers. |
| Absolute-claim registration | 646 | minor | Absolutes were absent from the claims register. |

Eight checks passed, notably keyword parity and plan coverage, which had failed
in round 3. `cargo test --workspace` passed **164 tests, 0 failures**. Those
tests cover the implemented lexer/identity/API/command-log fragments, not the
missing authorization/runtime/UI/restore surfaces.

The checked-in `review/report.json` is a later `round-4-nobrainers` run and
therefore must not replace the baseline: it reports 120 deterministic matches
after intervening edits. Neither run's regex matches are expanded into semantic
AA IDs. Deterministic and reviewer findings remain separate.

## 9. Strengths every reviewer says not to break

- Honest pre-alpha status and explicit “nothing is installable” boundary.
- Registered, enumerable, versioned operations instead of runtime query text.
- Exact ordinary-space binding through grant, access check, replay key, and
  executor, with cross-space negative tests.
- Narrow G10a ACL-root creation and explicit trigger provenance.
- Create/converge/CAS distinctions, full images, and typed conflicts.
- Open-skeleton defaults for sealed entities and equality-only sealed refinement.
- Domain-separated SHA-256/HMAC and constant-time verification.
- Per-space single-chain groups, scoped image lookup, and attributed reversions.
- Honest client-first mutual-trust/server-blindness disclosure.
- Strong sink-SSRF architecture: resolve/connect checks, redirect refusal,
  credential isolation, signed delivery, and explicit consent.
- Corrected licensing posture and credible plain-Postgres/export/self-host exit
  direction, once executable.
- Conformance-first discipline and the repaired cheap/strong experiment as a
  detector of documentation gaps.

## 10. Sequenced remediation — docs before code

### Owner decisions only

| Priority | Effort | Decision | IDs |
|---|---|---|---|
| P0 | M | Define client-first signed proposal/receipt, evaluator pin, outcome/validator/inverse contract. | AA-001–003 |
| P0 | M | Define typed deployment scope and bootstrap actor versus beneficiary. | AA-007/008 |
| P0 | M | Define authority-complete manifest ADT and partial order. | Y-005 |
| P0 | S | Choose rounding mode syntax/global policy and std count exemption/opt-in. | AA-014/018 |
| P0 | S | Choose ACL+explicit-root algebra and unreadable-parent error. | AA-027/033 |
| P1 | S | Choose whole pagination envelope, match rule/limits, and bucket types. | AA-011/012/015/019 |
| P1 | M | Choose managed offline queue and client-first compaction/delivery-loss contracts. | AA-030–032 |

### P0/P1/P2 × S/M/L execution matrix

| Priority | Effort | Docs/protocol first | Code/evidence after ruling |
|---|---|---|---|
| P0 | S | Synchronize bootstrap route/apps/clients/genesis/defaults; group wording; duration trap. | Golden bootstrap envelope; docs CI. |
| P0 | M | Publish authority ADT, replay UoW/fencing, bootstrap typed scope, snapshot proof, auth-link ceremony, live scope, sealed-log type, import session. | Refactor traits; implement negative suites for authority mutation, replay kills/leases, link theft, cross-space live/correlation, plaintext log append, import quiet bypass. |
| P0 | L | Specify the guarded vertical slice and migration capture protocol. | Parser/IR/planner/Postgres executor, guard corpus, domain/log/replay transaction, CDC/watermark/barrier, exact snapshot result. |
| P1 | S | Repair std rollup/whole/count text, match diagnostics, allocate domain, enum fallback, unbounded onboarding. | Parser/type/SQL goldens and two-witness diagnostics. |
| P1 | M | Finalize app/profile/guest/nullable-param grammar and managed offline record. | Generated list/detail/create UI, invite/exposure flow, durable queue and conflicts. |
| P1 | L | Publish benchmark/restore/fault methodology and hard budget model. | Reproducible capacity kit, timed encrypted restore, failure transcript, per-space quotas. |
| P2 | S | Make attestation maturity markers and op-enumeration order consistent. | Signed snapshot envelope; uniform pre-door errors. |
| P2 | M | Specify KMS token recovery, SMK reindex, genesis disposition rollback. | Adapter idempotency, dual-write/freeze tests, CAS disposition ledger. |
| P2 | L | Define production observability, client-first poison/purge/compaction, and export round trip. | Metrics/alerts, recovery tools, clean-environment export/import proof. |

## 11. Owner-only open questions

1. What exact object does a client-first device sign, who certifies its fold,
   and what honest-party availability assumption supports “caught”?
2. Are invalid client-first transitions canonical no-ops, or applied invalid
   states corrected later; who derives trustworthy inverse images?
3. Is deployment authorization a distinct grant/audience, and how are the real
   bootstrap actor and owner beneficiary represented without impersonation?
4. What complete dimensions constitute authority, and what componentwise order
   permits silent narrowing?
5. Is `std.E.count` explicitly exempt from human `unbounded` acknowledgment, or
   must deployment policy opt in to every derived O(space) operation?
6. Does root-level explicit `access` replace, compose with, or conflict with an
   `acl`-derived guard on the same entity?
7. Is the canonical UI/profile/guest surface part of `.ordo`, a separate typed
   manifest, or intentionally outside the compiler's authority model?
8. What durable offline guarantees apply in managed custody, and what compaction
   and resurrection rules apply when client-first segments are authoritative?

Everything else above follows mechanically from an existing choice and should
not be returned to the owner as an open-ended design exercise.
