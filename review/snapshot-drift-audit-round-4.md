# Snapshot drift audit — round 4

Date: 2026-08-09  
Snapshot: the current working tree, read cold after `docs/critique-consolidated-4.md` and `review/findings.json`  
Method: contract text and implementation were inspected directly. Tests were used only as a gate, never as evidence that a contract is complete.

## Verdict

The current ledger has four stale dispositions. `X-068` is still the same open defect, not a relocated one. `Y-008`, `Y-010`, and `Y-013` are fixed in their owner documents but contradicted by the current glossary, so they are relocated documentation-consistency defects rather than closed records. Their current `info` severities also understate first-read normative contradictions; all three should be `major`.

The five requested fatal records remain correctly unresolved: `X-001` and `AA-007` are `open`; `X-003`, `X-084`, and `Y-005` are `carried`. All 33 `AA-*` records remain correctly `open` at their recorded severity and with their recorded title. Apart from the four corrections below, every `X-*`, `Y-*`, and `Z-*` record marked `closed` or `relocated` remains supportable in this snapshot.

## Ledger drift corrections

### DA4-001 — X-068

- **Current ledger:** `relocated`, `critical`, “The global row-work gate still admits take as evidence for examined-row bounds.”
- **Correct:** `open`, `critical`, same title.
- **Exact evidence:** `docs/definition-language.md:1789-1800` correctly distinguishes returned rows from examined rows, but the universal gate at `docs/definition-language.md:1813-1815` still admits `take` as cardinality evidence without proving that it occurs before the potentially unbounded stage. `docs/syntax-examples.md:199-209` explicitly says that `take` bounds returned rows but an unfiltered entity still examines everything, and names `X-068`. `docs/glossary.md:325-343` repeats the generic `take` evidence rule.
- **Concurrent-change classification:** partial repair. The returned/examined distinction was added, but the original global-gate defect remains at its original owner and therefore was not relocated.

### DA4-002 — Y-008

- **Current ledger:** `closed`, `info`, “Denial-event ownership is synchronized.”
- **Correct:** `relocated`, `major`, “Denial-event ownership is settled in owner documents but contradicted by the glossary.”
- **Exact evidence:** `docs/security-logging-design.md:232-250,258-267` settles denials in a separate per-space security-event stream and says they never enter the command log. `docs/dashboard-plan.md:216-231` and `docs/implementation.md:397-410` follow that contract. In contrast, `docs/glossary.md:305-320` says the destination is still contradicted and presents a chained security stream as a proposal, explicitly citing `Y-008`.
- **Concurrent-change classification:** owner contract repaired; stale first-read glossary text relocates the defect rather than allowing closure.

### DA4-003 — Y-010

- **Current ledger:** `closed`, `info`, “DEK rotation and undo-horizon image handling are specified.”
- **Correct:** `relocated`, `major`, “DEK rotation and undo-horizon image handling are settled in owner documents but contradicted by the glossary.”
- **Exact evidence:** `docs/implementation.md:551-560`, `docs/security-logging-design.md:426-454`, and `docs/std-library.md:253-259` forbid destroying an epoch key while any command image remains inside `undoHorizon` and define the beyond-horizon unresolvable marker. `docs/glossary.md:408-419` instead says that before-images outliving an old epoch have no settled answer, citing `Y-010`/`C4-015`.
- **Concurrent-change classification:** normative design repaired; stale glossary prose preserves a narrower, relocated contradiction.

### DA4-004 — Y-013

- **Current ledger:** `closed`, `info`, “Direct/group combination and revocation are ruled.”
- **Correct:** `relocated`, `major`, “Direct/group combination and revocation are settled in owner documents but contradicted by the glossary.”
- **Exact evidence:** `docs/definition-language.md:1233-1246` selects the maximum surviving direct/group level and defines revocation as removing one path; `docs/security-logging-design.md:186-192` agrees. `docs/glossary.md:191-194` nevertheless says differing direct/group levels and per-person revocation are not yet ruled, citing `Y-013`.
- **Concurrent-change classification:** owner rule repaired; the defect now survives only in the glossary and is therefore relocated, not closed.

## Fatal-record verification

| ID | Ledger disposition | Cold-snapshot evidence | Result |
|---|---|---|---|
| X-001 | `fatal / open` | `README.md:92-118` says authorization, guard productions, planner, and executor are not implemented. `crates/ordo-syntax/src/token.rs` has no ACL/access grammar family, while `crates/ordo-api/src/spec.rs:230-238` merely injects an `OpExecutor`. The claimed guard set in `docs/api-engine-design.md:503-506` is absent from the actual `OpSpec` at `crates/ordo-api/src/spec.rs:103-120`. | unchanged |
| X-003 | `fatal / carried` | `README.md:131-133` disclaims performance and executed restore/DR evidence. `docs/implementation.md:350-360` describes what a drill should publish, but the repository contains no measured results or executed restore artifact. | unchanged |
| X-084 | `fatal / carried` | `README.md:92-120` explicitly says there is no installable platform, planner/executor/apply path, generated UI, or generated SDK. | unchanged |
| Y-005 | `fatal / carried` | The authority model remains space + operation allowlist + manifest hash + expiry (`crates/ordo-identity/src/principal.rs:327-361`; `docs/security-logging-design.md:117-146`). It does not express writes/deletes, effects, egress, key release, parameter influence, composition, or step-up constraints. | unchanged |
| AA-007 | `fatal / open` | Prose introduces deployment scope (`docs/api-engine-design.md:65-71`; `docs/bootstrap.md:54-73`), but `OpSpec` has no scope (`crates/ordo-api/src/spec.rs:103-120`), `OpExecutor` and `AccessChecker` require a space (`spec.rs:230-238,267-280`), router dispatch gates and binds one (`router.rs:214-250,304-341`), replay keys are space-scoped, and `CapabilityGrant` requires `SpaceId` (`crates/ordo-identity/src/principal.rs:327-361`). | unchanged |

## All AA records

Every row below was checked against the present contract/source, not inferred from a passing test. The recorded severity and title remain correct, and the correct status remains `open`.

| ID | Direct evidence that the defect remains |
|---|---|
| AA-001 | `docs/implementation.md:719-755` has the device sign a header containing its believed predecessor, then has the server re-anchor that predecessor and require another signature. |
| AA-002 | `docs/implementation.md:661-669,756-779` promises caught/undoable proposals but supplies only deterministic CAS/rejection, not an authoritative validator or truthful inverse. The command-log revert path still relies on stored before/after images. |
| AA-003 | The proposal header at `docs/implementation.md:723-727` contains payload digest, believed predecessor, device, and counter, but no evaluator artifact or schema identity. |
| AA-004 | `crates/ordo-api/src/router.rs:448-457` executes first and calls replay completion afterward; the `OpExecutor`/`ReplayCache` interfaces expose no common transaction carrier despite `docs/implementation.md:223-228`. |
| AA-005 | `crates/ordo-api/src/idempotency.rs:226-264` has reservation/put/abandon but no lease generation or fencing token. A stale holder can therefore complete after takeover. |
| AA-006 | The permanent create binding is installed only during `put` and is capacity-evicted with replay state (`crates/ordo-api/src/idempotency.rs:398-459`), so crash or eviction can admit a second create. |
| AA-007 | Deployment scope remains unrepresentable in the API/grant/replay implementation; see the fatal-record table above. |
| AA-008 | `docs/bootstrap.md:102-139,169-175` permits a genesis actor other than the intended owner, but `bootstrap.md:191` and the creator ACL rule attribute ownership to the creator. |
| AA-009 | `docs/bootstrap.md:88-90` creates the KMS key before recording its handle. A crash in that interval contradicts the no-orphan assertion at `bootstrap.md:409-413`. |
| AA-010 | `crates/ordo-commandlog/src/store.rs:65-82,153-160` intentionally exposes and implements `is_logged_elsewhere` by scanning other spaces, an existence oracle. |
| AA-011 | `docs/definition-language.md:1551-1611` simultaneously specifies exactly one path, direct preference, minimum shortest-path behavior, and a false monotonicity claim for bridges. |
| AA-012 | The same match-inference section (`definition-language.md:1582-1599`) requires all-path/subgraph diagnostics without a bound, retaining the unbounded general case. |
| AA-013 | `docs/implementation.md:503-510` specifies an offline scan-and-swap for blind-index rotation without a write freeze or dual-write linearization; `implementation.md:513-517` explicitly rejects the freeze alternative. |
| AA-014 | `docs/definition-language.md:155-169` requires authors to select rounding mode, but the callable forms at `definition-language.md:591-606` provide only `round(x)`/`round(x,n)`. |
| AA-015 | The type inventory at `docs/definition-language.md:134-143` has no month/week bucket result type or wire encoding, although functions at `definition-language.md:597-599` promise sortable scalars. |
| AA-016 | `docs/traps.md:107-149` first permits same-unit-class comparison including calendar duration, then its table says a bare calendar duration is a compile error. |
| AA-017 | `docs/definition-language.md:1705-1707` appends only one anchor id to every sort; multi-source rows can share it. `docs/composition-conformance.md:383-385` confirms the same anchor-only tie-break. |
| AA-018 | The universal gate at `docs/definition-language.md:1813-1824` and `docs/std-library.md:427-436` treat reduce-shaped `count` as bounded evidence even though it can examine the whole space. |
| AA-019 | `docs/std-library.md:60` says whole-collection output is paged `{items,count,next}`, while `std-library.md:438-450` makes it capped/truncated; `std-library.md:587-591` leaves the cap open. |
| AA-020 | `docs/definition-language.md:607-635` promises `allocate` for all weights but defines no result for empty, negative, or zero-sum inputs. |
| AA-021 | `docs/security-logging-design.md:359-377` checks only a recent presence assertion and factor identity; there is no one-use challenge binding the named old factor to the new authenticator ceremony. |
| AA-022 | `InvalidationEvent` and subscription are op-only (`crates/ordo-api/src/spec.rs:274-280`); the router subscribes/refetches without a space-scoped event or continuous suspension/revocation check (`router.rs:714-739`). |
| AA-023 | Public command-log `Value` still admits both text and `SealedRef`, and `Image` is an unconstrained public map (`crates/ordo-commandlog/src/value.rs:27-57`); append has no artifact/schema input to enforce sealed fields. |
| AA-024 | `docs/migration.md:266-275` leaves trigger suppression as a caller/batch import flag and tunable policy, without a privileged session or ceremony. |
| AA-025 | `crates/ordo-api/src/router.rs:228-274` returns method mismatch/missing-space outcomes before the access door, allowing operation-name enumeration. |
| AA-026 | The language type inventory (`docs/definition-language.md:134-143`) omits enum even though derived ACL uses it (`definition-language.md:1202-1204`) and UI descriptors assume it (`docs/ui-gen.md:42`). |
| AA-027 | Guard replacement is defined only for child inheritance (`docs/definition-language.md:994-1011`); root ACL derivation (`definition-language.md:1198-1224`) has no `acl` + explicit-access combination rule. |
| AA-028 | Contradictions persist: bootstrap uses `/ops/` while the API/router use `/op/`; bootstrap inventory says `apps` but its example says `clients`; custody defaults and managed conversion differ; and `docs/bootstrap.md:62` says exactly one deployment op while `docs/api-engine-design.md:65-67` names both create and reap. |
| AA-029 | `docs/glossary.md:181-183` says guards broadly expand groups, while `docs/definition-language.md:877-890` requires an explicit group disjunct, silently denying group-backed members under the broad reading. |
| AA-030 | `docs/implementation.md:695-703` describes only the client-first sync path. No managed-custody durable offline mutation record, retry, artifact pin, or user-conflict contract is specified. |
| AA-031 | At-least-once delivery is promised at `docs/definition-language.md:2294` onward, but `docs/security-logging-design.md:580-595` permits stalled-consumer eviction and floor advancement without a delivery-loss rule. |
| AA-032 | `docs/implementation.md:695-703` makes client-first segments/snapshots authoritative; `docs/security-logging-design.md:601-606` protects a cursor, but no segment checkpoint, tombstone, purge, or compaction protocol exists. |
| AA-033 | `docs/definition-language.md:1117-1122` returns `not_found` for an unreadable parent on create, while `docs/guards-conformance.md:889-899` expects `access_guard` for that case. |

## Closed and relocated X/Y/Z inventory

The following dispositions remain correct after inspection. The four omitted IDs are the DA4 corrections above.

- **Closed (62):** `X-002 X-004 X-005 X-006 X-007 X-011 X-018 X-019 X-022 X-023 X-026 X-029 X-031 X-032 X-040 X-066 X-074 X-075 X-079 X-080`; `Y-001 Y-002 Y-003 Y-004 Y-006 Y-007 Y-009 Y-011 Y-014 Y-015 Y-016 Y-021 Y-022 Y-023 Y-024 Y-025 Y-026 Y-027 Y-028 Y-029 Y-030 Y-033 Y-034 Y-035 Y-036 Y-037 Y-038 Y-100 Y-101 Y-102 Y-103 Y-104 Y-105 Y-106`; `Z-001 Z-003 Z-004 Z-008 Z-010 Z-015 Z-018 Z-019`.
- **Relocated (17):** `X-008 X-009 X-039 X-058 X-060`; `Y-012 Y-018 Y-031 Y-032`; `Z-002 Z-005 Z-006 Z-009 Z-012 Z-016 Z-020 Z-023`.

The confirmation included the changed API page/router/tests, command-log implementation, syntax token inventory, README, glossary, bootstrap, implementation, security logging, standard library, definition-language, examples, and check configuration/output. No additional disposition drift was found in those records.

## Concrete contradiction introduced or preserved by the fixes

`docs/ui-gen.md:42-45` now says pagination always has stable cursors because total order is a language guarantee. The normative rule at `docs/definition-language.md:1709-1745` says stability depends on write-stable sort keys, labels any writable-key order `cursor: drifting`, and makes composed post-`take` pagination always drifting. Total order prevents ties; it does not prevent rows from moving across pages. This is a concrete current contradiction associated with carried `X-059`/open `AA-017`; it does not change either ledger disposition, but the UI descriptor sentence should not survive the fixes.

## Gates

- Deterministic review report: **118 findings** — rule coverage **38**, markers **14**, absolutes **66**; severity split major **20**, minor **98**.
- `cargo test --workspace`: **166 passed, 0 failed** (API library 18, API integration 42, command log 17, identity 53, syntax 36; doc tests 0).

These gates describe the snapshot successfully exercised. They do not override any of the contract defects above.
