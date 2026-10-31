# Security review, round 6 (consolidated)

Two independent reviewers (A: custody, the door, guard execution, provisioning; B: the command log and security stream, blobs, export/import, supervisor, erasure) worked without seeing each other. This document merges their files (`review/security-round-6-a.md`, `review/security-round-6-b.md`), one finding traced by the session lead (`L-01`), and the prior-finding dispositions in `docs/audit-security-trace.md`. Nothing has been softened. Machine-readable twin: `review/findings-security-6.json` (ids and severities match this file exactly).

Verification vocabulary used on every entry: **live** (made the running binary do it), **code** (read at the cited file:line), **inferred** (reasoned from the call graph, not executed).

## 0. Verdict

**Counts after deduplication: 13 entries. 3 Critical, 5 Major, 3 Minor, 2 Note.** Sources: 7 A findings, 5 B findings, 1 lead finding (`L-01`) that also absorbs B's unconfirmed item in its section 6. Two A pairs merged (A-01 with A-03), and the A-02/A-04/A-06 group was split into one root cause plus three consequences.

**Read these three first.**
1. **S6-001** (Critical): an inferred join bridge, and a dot-path join, drop the read guard of the entity they traverse. A member reads facts from rows their own guard forbids, and the op's published manifest claims the guard is applied. Verified live on both nodes.
2. **S6-002** (Critical): member suspension does not exist on the serving path. `deploying.md:748` promises it; `MEMBERS_SQL` never selects `status`. Verified live.
3. **S6-003** (Critical, lead-traced): the AEAD associated data for sealed columns and blob streams binds entity and field, not the row id or object handle. The actor the custody design exists to constrain (operator with DB credentials, or object-store write access) cannot read a sealed value but can move it to another row of the same column, and it opens without error. Read by code; two detection questions are open.

**The single design decision the reviewers would push back on hardest.** Two were named, one per reviewer, and both stand:
- Reviewer A: building the door's authorization engine as a **whole-table snapshot rebuilt only on deploy or member announce** (`engine_from_members`). It is the root of S6-002, S6-005 and S6-010 (entry S6-004).
- Reviewer B: deriving `import.<entity>.create` with **no `rev` for any entity**, so a create cannot refuse a duplicate key (S6-007).

If the owner must pick one to reconsider first, it is A's: it produces a Critical and it is where every future per-row authorization state (status, overrides, break-glass) will otherwise have to squeeze through a columns-in, rebuild-to-apply pipe.

**Pointer to the mechanical gates (context only).** The pre-review gates reported 423 findings across 4 failing checks. `keyword-parity` (61, critical by the gate's own weighting) is a separate track and is not expanded here; `rule-coverage` 38, `markers` 15, `absolutes` 309.

**Independent rediscovery, summarized.** No finding was reached by both reviewers (their scopes were disjoint by design). Rediscoveries that do exist: (a) reviewer B flagged the blob read path's missing `verify` on its own, then the lead traced the cause to the AAD (S6-003); (b) reviewer A rediscovered the recorded gap GC007 live (S6-001, `docs/corpus.md:169`); (c) the code itself admits S6-005 (`crates/ordo-host/src/lib.rs:1612-1618`) and S6-007 (`crates/ordo-exec/src/import.rs:1024-1039`), which were then confirmed by a reviewer against the docs that promise the opposite. A finding the code already confesses in a comment while a document promises the reverse counts here as a rediscovery multiplier: severity was not lowered for the confession.

## 1. Threat actors

- Malicious tenant owner (owner role in one space; can author `.ordo` programs if self-hosting).
- Malicious ordinary member (token for one space at a non-owner role).
- Compromised worker or replica (holds the deployment master key in RAM, can read Postgres).
- Operator with database credentials, who must not read sealed plaintext (reads and writes Postgres directly).
- Operator who stages files but must not read plaintext (writes under `ORDO_IMPORT_BUNDLE_DIR` and `ORDO_EXPORT` roots).
- Holder of a stolen or hostile export bundle.
- Someone with write access to the object store.
- Stolen sealed-master file; stolen bearer token; network adversary; lagging or partitioned replica; supply chain.

## 2. Findings

### Critical

#### S6-001 · Critical · A guard is dropped on every join node the compiler adds or lowers: inferred bridge (`SourceJoin`) and dot-path (`Join`)
- **Origin:** A-01, A-03 (GC007 in `docs/corpus.md:169`). Rediscovery: A-03 is an independent live confirmation of a gap already recorded and ruled 2026-09-26.
- **Merge decision: one entry, two sites.** A reports them as one defect on two IR nodes. They share the violated rule (R-G6, "every entity the flattened plan reads contributes its read guard"), the fix shape (guard moves into the join `ON`, joined columns null, anchor row stays), and the risk that a third join site repeats it. One entry means one rule and one conformance test covering all join sites. They are not merged in severity reasoning: site (a) is new, unrecorded and worse; site (b) is recorded, ruled and unfixed. The plan below sequences them together because the ruled GC007 change and the bridge change touch the same `ON`-guard mechanism.
- **Threat actor:** malicious ordinary member.
- **Claim attacked.** `definition-language.md` multi-source: "Disclosed everywhere: inferred bridges appear in the plan, the scope manifest (their guards inline too)". `guards-conformance.md` R-G6: "every entity the flattened plan reads contributes its read guard as a conjunctive semi-join". The op manifest's `row_scope.entities` lists the bridge with `read: {kind: walk, source: own, walk: "owner == $caller"}`, claiming it is applied.
- **Attack path.**
  1. Site (a), bridge: an op lists two entities that only connect through a third guarded entity, for example `whoWhere = people[name] places[id]`, joined through `assignments` (`access read owner == $caller`). `assignments` is compiler-inserted and unlisted.
  2. As owner `mem_dev`, create an assignment linking `pers_1 -> plc_1`.
  3. As `mem_bob`, who does not own that row, call `GET /op/whoWhere`. The response includes `{"id":"plc_1","name":"Alice"}`. `mem_bob` and `mem_dev` receive byte-identical results.
  4. Site (b), dot-path: `taskView = tasks[title, project.name, project.budget]`. `projects` has `access read owner == $caller`. As `mem_bob`, a member of no project, the call returned `{"name":"Acquisition of Globex","budget":9000000,...}`. The field is fully populated, not even nulled, and the manifest shows `projects` under `via: "guard"`.
- **Evidence.** Site (a): `crates/ordo-sema/src/lib.rs:3269-3277` (`Node::Bridge(e)` becomes a bare `From::Table`), against terminals guarded through `entity_source` at `:1897` and `:1976-1984`; rendered without a guard at `crates/ordo-plan/src/lib.rs:1241-1279`. Site (b): `crates/ordo-sema/src/lib.rs:4737-4744`, rendered at `crates/ordo-plan/src/lib.rs:1281-1300`. The manifest claim comes from a separate walk over the scope set, so manifest and executed SQL disagree: a silent-wrong, not a loud-fail.
- **Verified:** live (dev-embedded path via `ordo dev`, `FixtureExecutor`; not yet on an applied-program Postgres-backed server).
- **Proposed pin `[proposed]`.** Give `SourceJoin` a guard field exactly as the GC007 ruling gives `Join` one; for `Node::Bridge`, compile the bridge entity's read guard through the same `guard_cond` terminals use into the join `ON`. Fold both into one rule. Additionally derive the manifest's per-entity guard claim from the executed plan rather than from a separate scope walk, so the two cannot disagree again.
- **What the fix must not break.** Outer-preserving semantics (ruling-3 amendment 3): a failing guard nulls the far columns and keeps the anchor row. A blank field is the intended visible outcome. Do not turn a LEFT JOIN into an inner join.
- **Owner decision:** none needed for the ruled semantics. The manifest-derivation pin is mechanical.

#### S6-002 · Critical · Member suspension is unenforced on the serving path
- **Origin:** A-02. Consequence of S6-004.
- **Threat actor:** malicious ordinary member; equally, the operator responding to a compromised member.
- **Claim attacked.** `docs/deploying.md:748`: "Suspension takes effect on the next request, not at the next token expiry. Suspending a compromised member is immediate by construction." The engine's own check (`crates/ordo-identity/src/access.rs:667`) tests `MembershipStatus::Suspended` first.
- **Attack path.**
  1. `mem_bob` is a compromised member. An operator sets `members.status = 'suspended'` for `mem_bob` (directly in the database, since no op can do it: `members` is platform-read-only, `set members` is refused with `reserved_derived_name`).
  2. The engine is rebuilt (or already built) by `engine_from_members`, which selects only `"$space","id","role"` and calls `set_membership(space, id, role)` with no status.
  3. `set_membership` always inserts `MembershipStatus::Active` (`crates/ordo-identity/src/access.rs:470-474`). `GET /op/taskView` still returns rows.
  4. There is no production caller of `AccessEngine::suspend` (grep outside `/tests/` finds none), so the check at `access.rs:667` is dead on the table-driven path.
- **Evidence.** `crates/ordo-host/src/lib.rs:1591-1592` (`MEMBERS_SQL`), `:1669-1683`, `:1681`; `crates/ordo-identity/src/access.rs:466-500`, `:667`. I re-read `MEMBERS_SQL` in the tree while consolidating: it is exactly `SELECT "$space", "id", "role" FROM "members"`.
- **Verified:** live, plus re-read by the consolidator.
- **Proposed pin `[proposed]`.** Add `"status"` to `MEMBERS_SQL`; in `engine_from_members` call `engine.suspend(space, principal)` after `set_membership` when the row is suspended. Separately, correct `deploying.md:748` to "next engine rebuild" unless S6-005 is resolved by option (b).
- **What the fix must not break.** Reactivation restores the exact prior role and groups (Z-005), which the in-memory suspend/reactivate already preserve and a table-driven rebuild reproduces because role is re-read. The single-database and routed directory must keep selecting the same columns from one constant.
- **Owner decision:** yes, jointly with S6-005 (see contradiction table, RULE rows 1 and 2), because the doc text depends on which freshness model is chosen. The `MEMBERS_SQL` change itself is mechanical.

#### S6-003 · Critical · Sealed-value AAD binds entity and field but not row id or object handle, so a value or blob can be transplanted undetected
- **Origin:** L-01 (traced by the session lead, not found cold), and reviewer B section 6 "blob store-substitution threat" (unconfirmed by B). Rediscovery status: B flagged the missing `verify` on the blob read path independently; the lead traced the cause and widened it to sealed columns. Reviewer B also listed the `(entity, field)` AAD binding under "what is strong" (`pipeline.rs:39-52`, `stream.rs:153`), so this entry contradicts a reviewer's endorsement; see disagreements.
- **Threat actors:** operator with database credentials (sealed columns); someone with write access to the object store (blobs). This is the actor the custody design exists to constrain: they cannot read the plaintext, and they can move it.
- **Claim attacked.** The AAD is documented as making an object "moved between fields refuse to open" (`crates/ordo-blob/src/pipeline.rs:184`; `docs/plan-blob.md:336`, "the seal's AAD binds `(entity, field)`... so an object moved between fields refuses to open"). That is true and is the whole of what it guarantees. The implicit claim attacked is that sealed content stays attached to the row it was sealed for.
- **Attack path.**
  1. Blobs: attacker with object-store write access swaps two objects within the same column of the same space. Both are sealed under the same space key with the same AAD, so both decrypt cleanly. A row now serves another row's file. The sealed integrity digest on `$blobref` would catch it, but it is consulted only by revert and by explicit `BlobPipeline::verify`, not on `get`. I grepped `crates/ordo-host/src/blob.rs` while consolidating: it contains no call to `verify`, and the read path at `:736` opens and returns.
  2. Sealed columns: an operator with database credentials copies a sealed envelope from row R1 to row R2 in the same column. `crates/ordo-crypto/src/lib.rs:89` is the same `fn aad(entity, field)`, so it opens without error on R2.
- **Evidence.** `crates/ordo-crypto/src/stream.rs:153` and `crates/ordo-crypto/src/lib.rs:89` (both `fn aad(entity: &str, field: &str)`; the first carries a comment stating identity with the single-shot envelope is deliberate); `crates/ordo-blob/src/pipeline.rs:184`, `:666` (`verify`); `crates/ordo-host/src/blob.rs:736`.
- **Verified:** code (AAD signatures re-read; absence of `verify` on the host blob read path re-read). Not exercised live.
- **Open questions, flagged and not assumed either way (from the lead).**
  1. Would the row's hash-chain images expose a transplant on a later `ordo verify`? If chain images hash the envelope bytes as stored, a DB-side transplant that bypasses the write path might be caught after the fact, which would reduce detection latency but not prevent the read of the wrong content in the interim.
  2. Do blind-index columns (HMAC of plaintext stored beside the envelope) make a transplant detectable by disagreement between envelope and index? That would only cover `sealed indexed` columns and only if some path compares them.
  Severity is Critical on the strength of the un-detected window on `get`; it would drop to Major if both questions resolve in favour of detection for every sealed column and blob.
- **Proposed pin `[proposed]`.** Bind the row id into the AAD for sealed columns, and the row id plus the object handle for blobs. Also make the blob `get` path verify the sealed integrity digest (or state in the docs why it does not).
- **What the fix must not break.** Bulk import re-seals under the destination key with a newly minted row id, so the AAD must be computed from the destination id at seal time, never from the source id. `ordo keys rewrap-master` must continue to move no envelope at all (it rewraps keys, and AAD changes must not put a row id into any key-wrapping path). The existing entity-and-field binding must remain.
- **Owner decision:** yes. It changes a stored-format property, and pre-deployment there is no migration owed (memory: nothing has deployed), but the owner must rule whether verify-on-get costs are acceptable for streaming reads (a partial range read cannot check a whole-object digest). See open questions.

### Major

#### S6-004 · Major · Root cause: the door's authorization engine is a whole-table snapshot rebuilt only on deploy or member announce
- **Origin:** A executive summary design pushback; root of A-02, A-04, A-06.
- **Threat actor:** malicious member; operator revoking a compromised member.
- **Claim attacked.** `deploying.md:776`: "Membership is re-read by the door on every request"; `crates/ordo-host/src/auth.rs:386-390`: "the door ... re-reads membership on every check".
- **Mechanism.** `IdentityAccess::check` reads `self.engine`, an `Arc<AccessEngine>` built by `engine_from_members` at boot and on rebuild, and never touches the database (`crates/ordo-host/src/lib.rs:1425-1541`). The only trigger for a rebuild on membership change is `announce_members` firing `pg_notify` (`crates/ordo-host/src/deploy.rs:972-980`, `crates/ordo-host/src/runtime.rs:3178-3186`). Columns that are not read into the engine (status) do not exist for authorization.
- **Consequences, each its own entry:** S6-002 (status dropped, Critical), S6-005 (membership changes invisible until rebuild, Major), S6-010 (a member write that forgets to announce makes that window permanent, Minor).
- **Evidence.** As above, plus the code's own admission at `crates/ordo-host/src/lib.rs:1612-1618`: a membership change "takes effect at the next swap or the next restart rather than on the next request... weaker than IdentityAccess's own doc reads".
- **Verified:** live (via A-04) and code.
- **Proposed pin `[proposed]`.** Keep the in-memory engine (per-request cost is a design asset) but stop making deploy the only refresh trigger: refresh membership and status per space on the existing invalidation bus, the way the session deny list is already re-read. Whichever freshness model is chosen, state it once in the docs and derive the other statements from it.
- **What the fix must not break.** No database round trip per request. Reactivation semantics (Z-005). Uniform door answer.
- **Owner decision:** yes (freshness model: correct the docs to "next rebuild", or build per-request/invalidation-bus freshness).

#### S6-005 · Major · A suspended or removed member keeps serving until an engine rebuild
- **Origin:** A-04. Consequence of S6-004.
- **Threat actor:** malicious member; operator revoking a compromised member.
- **Claim attacked.** `deploying.md:776` and `crates/ordo-host/src/auth.rs:386-390` (see S6-004).
- **Attack path.** Delete `mem_bob`'s `members` row. `GET /op/taskView` still returns rows and keeps doing so until a program swap (any save), after which it becomes `access_denied`. A direct DB membership change, or one from a path that does not announce, is invisible until the next rebuild. Session revocation is re-read per request (deny list) and is a different fact.
- **Evidence.** `crates/ordo-host/src/lib.rs:1425-1541`, `:1612-1618`; `crates/ordo-host/src/deploy.rs:972-980`; `crates/ordo-host/src/runtime.rs:3178-3186`.
- **Verified:** live (single replica); multi-replica fan-out read but not run.
- **Proposed pin `[proposed]`.** Option (a): correct `deploying.md` section 5 and `auth.rs` to "next engine rebuild" and route suspension through a rebuild that always announces. Option (b): read membership/status in the request path via an in-memory per-space map refreshed on the invalidation bus.
- **What the fix must not break.** Per-request cost stays in memory; option (b) must not add a DB round trip.
- **Owner decision:** yes (same ruling as S6-004).

#### S6-006 · Major · Import's resume fingerprint covers source ids only, so a changed bundle resumes silently
- **Origin:** B-02.
- **Threat actor:** holder of a hostile bundle under a reused import name; equally an honest operator who edited a bundle and reran.
- **Claim attacked.** `docs/plan-import.md` section 11.5 (line 767): "a changed bundle under a used name refuses instead of skipping". Also the code's own doc at `crates/ordo-exec/src/import.rs:1551`: "makes a re-run of a **changed** bundle under the same import name answer `idempotency_mismatch` instead of silently skipping".
- **Attack path.**
  1. Run import `M` over bundle `B`; it commits batches 0..k and is interrupted or completes.
  2. Replace row values in place (same ids, changed fields) and rerun `M`.
  3. `batch_fingerprint` (`import.rs:1557`) is a digest of `source_id` joined by newline: ids only. Every committed batch's stored fingerprint still matches, so `claim_lease` returns `Applied` and the batch is skipped (`import.rs:1639`, `crates/ordo-exec/src/lib.rs:1661`, `:2150-2200`).
  4. Uncommitted batches apply the new values. The run reports success over a half-old, half-new space with no `idempotency_mismatch`. The per-member `sha256` (`import.rs:869`) catches truncation but does not gate the skip.
- **Evidence.** I re-read `import.rs:1553-1559` while consolidating: the function joins `r.source_id` only.
- **Verified:** code (reviewer B) and re-read by the consolidator; not executed.
- **Proposed pin `[proposed]`.** Either hash each row's canonical field set into the fingerprint, or record the bundle's `manifest.json` digest under the import name on the first run and refuse a resume on mismatch (cheaper, matches section 11.5 directly).
- **What the fix must not break.** An unchanged bundle must still resume cleanly (idempotent rerun is the feature); the fingerprint must be stable over identical content and ordering.
- **Owner decision:** no for the fix (mechanical, code must meet the doc). Choose between the two pins at implementation time.

#### S6-007 · Major · `import.<entity>.create` carries no `rev`, so a duplicate unique key in a bundle converges silently
- **Origin:** B-03. Self-disclosed in code (`crates/ordo-exec/src/import.rs:1024-1039`).
- **Threat actor:** holder of a hostile or malformed bundle; the outcome is silent data loss regardless of intent.
- **Claim attacked.** The code's own comment: a source bundle with two rows under the same unique key "would silently converge the second onto the first rather than refuse it. That is exactly the 'quietly converged' outcome the paragraph below warns against."
- **Attack path.** Stage a bundle for a `unique`-keyed entity with two JSONL rows sharing the key and differing elsewhere. `import_ops` (`import.rs:1045-1107`) emits a `Set` with `OpFlags::default()` and no `rev`. Both source ids land in `$importmap` (`:1614`), both point at one minted row. The minted-id unique index (`IMPORTMAP_DDL`, `:108`) may fail the second insert as `row_refused`, or the write may overwrite silently, depending on statement ordering. The row-count check (`rows_of`, `:1237`) cannot see it because the line count is honest.
- **Verified:** code. B could not settle whether the index turns it into a refusal in every ordering; that needs one targeted test in `crates/ordo-exec/tests/import.rs` (open question 4).
- **Proposed pin `[proposed]`.** Derive `import.E.create` with `rev: null` for keyed entities as `std_ops` now does (its 11-10a fix), so a within-bundle duplicate refuses with `expected_absent`. Failing that, detect a repeated unique-key value at read time and refuse.
- **What the fix must not break.** Loading a legitimate bundle into an empty space must succeed; the create semantics import relies on (nothing to converge onto in an empty space) must hold.
- **Owner decision:** yes. The code comment says the duplicate-key policy was deliberately deferred, and the reviewer names it the design decision to push back on. The owner rules refuse vs. converge.

#### S6-008 · Major · Import reads every bundle member whole into memory with no size cap
- **Origin:** B-01.
- **Threat actor:** holder of a hostile or oversized bundle; the operator who stages it; the owner who runs it (owner-only, step-up gated).
- **Claim attacked.** Implied: `plan-import.md` section 8 (bounded per-group memory for "a million rows") and the blob pipeline's object cap (`BlobPolicy::admits_object`, `pipeline.rs:356`, checked "BEFORE the sealer").
- **Attack path.** Stage a `data/<entity>.jsonl` of several GB, or a `blobs/...` member of arbitrary size, with the correct `sha256`. `DirectorySource::read` is `std::fs::read` (`crates/ordo-exec/src/import.rs:187`); the body is copied again by `String::from_utf8` (`:1404`); `rows_of` holds every parsed row (`:1202`). Blob members go through `attach_objects` (`:1931`) into a `Vec<u8>`, and `PlatformBlobs::attach` (`crates/ordo-host/src/blob.rs:791-792`) sets `declared_len` from that buffer, so the object cap runs only after the object is resident. Export, by contrast, streams in 64 KiB steps.
- **Verified:** code. B did not measure the failure size; staging is manual today (`plan-import.md` section 11.6 says delivery is unbuilt), so reachability through a real deployment is unconfirmed.
- **Proposed pin `[proposed]`.** Check `std::fs::metadata().len()` against a policy bound before reading; stream blob members off disk into `BlobPipeline::put` so the cap refuses at zero resident bytes.
- **What the fix must not break.** Round trip (an export's own members must import) and the digest check, which must run over the streamed bytes.
- **Owner decision:** no for the mechanism; the JSONL size bound value is a policy number the owner sets.

### Minor

#### S6-009 · Minor · `ORDO_KEY_PROVIDER=local` stretches the master with a single HMAC over a 16-byte floor
- **Origin:** A-05.
- **Threat actor:** stolen database backup plus a short `ORDO_MASTER_KEY`.
- **Claim attacked.** `crates/ordo-crypto/src/master.rs:26-28`: "A sixteen-character passphrase behind one HMAC is a sixteen-character passphrase", given as the reason the sealed file exists, while `local` stays a selectable non-dev provider.
- **Attack path.** `MasterKeySource::Env::open` accepts any secret of 16 bytes or more and runs `hmac_sha256(b"ordo.master.v1", secret)` with no KDF (`master.rs:124-135`, `:163-165`). Wrapped SMK/DEK rows are in the backup by design (`keystore.rs:11-20`). An adversary with a backup runs an offline guess loop at HMAC speed. The sealed-file path uses argon2id and is unaffected.
- **Verified:** inferred (the derivation was read; no crack was attempted).
- **Proposed pin `[proposed]`.** Print a startup posture line stating the consequence ("offline-crackable master; move to sealed-master before customer data"); consider the same argon2id stretch for `local` as a single boot-time cost, or a higher floor.
- **What the fix must not break.** Self-host is first class (custody must not require a KMS); the KDF must be a boot-time cost, not per seal.
- **Owner decision:** yes (disclosed trade-off; whether to harden `local` or only warn).

#### S6-010 · Minor · A member write that does not announce leaves the engine stale permanently; nothing announces a direct change
- **Origin:** A-06. Consequence of S6-004.
- **Threat actor:** none adversarial; authorization freshness.
- **Claim attacked.** `crates/ordo-host/src/tenant_ops.rs:687-706`: a created tenant is reachable "now, or at the next restart", reported honestly.
- **Mechanism.** `announce_members` (`deploy.rs:972`) is the only rebuild trigger for a membership change and is called from tenant creation. No membership-mutation op exists (members is read-only to op code, verified), so any future add/remove/suspend that writes `members` directly must remember to announce or the S6-002/S6-005 window becomes permanent.
- **Verified:** code. Forward hazard, not a live exploit.
- **Proposed pin `[proposed]`.** Resolve with S6-004: make the refresh trigger the membership write itself (invalidation on change), not a caller's memory.
- **What the fix must not break.** The honest "now or at restart" reporting in `std.space.create`.
- **Owner decision:** no (falls out of S6-004).

#### S6-011 · Minor · The import staging reader follows symlinks
- **Origin:** B-04.
- **Threat actor:** operator or anything that can write under `ORDO_IMPORT_BUNDLE_DIR`; a hostile bundle assembled off the export path.
- **Claim attacked.** `crates/ordo-exec/src/import.rs:181-186`: refusing `..` "closes the seam for a bundle that arrived from elsewhere".
- **Attack path.** `DirectorySource::read` is `std::fs::read(self.root.join(name))`, which follows a symlink; `names()` uses `path.is_dir()` (`:205`), which follows a symlinked directory. A member staged as a symlink to a file outside the staging tree is read through. Exploit value is bounded because the bytes must match the manifest `sha256`, so an attacker can only redirect to bytes they already know. This erodes the "exactly two directories under the root" invariant of `crates/ordo-host/src/exit_ops.rs:395`.
- **Verified:** code.
- **Proposed pin `[proposed]`.** `canonicalize` the member path and refuse if it leaves the canonicalized root (pattern already in `crates/ordo-exec/src/creds.rs:370`), or open with `O_NOFOLLOW`.
- **What the fix must not break.** Directory bundles of regular files; export never writes symlinks.
- **Owner decision:** no.

### Note

#### S6-012 · Note · The executor re-parses caller values by splicing them into `.ordo` source text
- **Origin:** A-07.
- **Threat actor:** malicious caller probing for injection.
- **Claim attacked.** Implicit: parameters are data, not code.
- **Attack path (attempted, did not break).** `FixtureExecutor::call_source` (`crates/ordo-host/src/lib.rs:632-652`) renders bound parameters into op-call source, strings as `format!("\"{}\"", s.replace('"', ""))`, then re-parses. Quotes are stripped so a literal cannot be terminated; NUL or unterminated input surfaced as `internal`, not as injected code. It fails closed today, and the safety is a property of the lexer (no escape or alternate delimiter surviving quote-stripping), which a future grammar change could silently void. The `internal` (500) mapping is a mild oracle for "my value broke the re-parse".
- **Verified:** live (attempted exploit failed).
- **Proposed pin `[proposed]`.** Bind parameters through the planner's `$n` slots (`render_write_value`, `SPACE_SLOT`) so caller data never re-enters the parser; if the seam stays, add a test that no lexer token can be produced from a stripped value. Contrast with the import path, which uses `str_literal` (control-character refusal, closed-set escaping, `import.rs:953`) rather than stripping.
- **What the fix must not break.** Deterministic replay and the single-dialect rule.
- **Owner decision:** no.

#### S6-013 · Note · Supervisor leadership has a split-brain window; safe only because destructive passes claim transactionally
- **Origin:** B-05.
- **Threat actor:** lagging or partitioned replica.
- **Mechanism.** Leadership is a session `pg_try_advisory_lock` (`jobs.rs:398`), probed each `leader_tick` (5s default, `supervisor.rs:182`). For up to one tick, an old and a new leader both believe they lead. Correct today because retention and checkpoint gate on transactional claims (`crates/ordo-exec/src/secevent.rs:1037`, `:1311`) and `Leadership`'s doc says it is "not load-bearing for correctness" (`jobs.rs:382`). A future leader-only worker without its own `claim_*` guard would double-run destructively.
- **Verified:** code.
- **Proposed pin `[proposed]`.** Assert in a test that every leader-only worker owns a `claim_*`-style transactional guard.
- **What the fix must not break.** The millisecond hand-off on rolling restart (leadership connection dropped before the pool closes).
- **Owner decision:** no.

## 3. Contradiction table (system documents against code)

"Winner" names which side has code evidence for what actually happens today. Where the document states the intended behaviour and the code is defective, the document is marked as the spec and the fix is in code. `RULE` marks rows where the owner must decide which side moves.

| # | Document claim (file:line) | What the code does (file:line) | Winner | RULE |
|---|---|---|---|---|
| 1 | `docs/deploying.md:748` "Suspension takes effect on the next request, not at the next token expiry. Suspending a compromised member is immediate by construction." | `MEMBERS_SQL` selects `"$space","id","role"` and no status (`crates/ordo-host/src/lib.rs:1591-1592`); `set_membership` always inserts Active (`crates/ordo-identity/src/access.rs:470-474`); no production caller of `suspend` | Code (doc is false today) | RULE: build enforcement (S6-002) or reword to "next rebuild" |
| 2 | `docs/deploying.md:776` "Membership is re-read by the door on every request" | `IdentityAccess::check` reads an in-memory engine, no DB read (`crates/ordo-host/src/lib.rs:1425-1541`); code comment admits "next swap or next restart" (`:1612-1618`) | Code | RULE: freshness model (S6-004) |
| 3 | `crates/ordo-host/src/auth.rs:386-390` "the door ... re-reads membership on every check" | Same as row 2 | Code | RULE, with row 2 |
| 4 | `docs/plan-import.md:767` (section 11.5) "a changed bundle under a used name refuses instead of skipping" | `batch_fingerprint` digests source ids only (`crates/ordo-exec/src/import.rs:1557`); resume skips on id match (`:1639`) | Code (doc is false for changed values) | No: code moves to meet the doc (S6-006) |
| 5 | `crates/ordo-exec/src/import.rs:1551` "makes a re-run of a **changed** bundle ... answer `idempotency_mismatch` instead of silently skipping" | Function body at `:1557` hashes ids only | Code | No, with row 4 |
| 6 | `docs/definition-language.md` multi-source: "inferred bridges appear in the plan, the scope manifest (their guards inline too)"; `docs/guards-conformance.md` R-G6 "every entity the flattened plan reads contributes its read guard" | Bridge is a bare `From::Table` (`crates/ordo-sema/src/lib.rs:3269-3277`), rendered with the space predicate only (`crates/ordo-plan/src/lib.rs:1241-1279`) | Doc is the spec; code is defective | No: code moves (S6-001) |
| 7 | Op manifest `row_scope.entities` lists the bridge with `read: {kind: walk, source: own, walk: "owner == $caller"}` (published surface, produced by a separate scope walk) | Executed SQL applies no bridge guard (same lines as row 6) | Code | No: derive manifest from the plan (S6-001) |
| 8 | `docs/corpus.md:169` GC007 "Ruled 2026-09-26: the guard belongs in the join's `ON`" | `Join` IR carries no guard field; dot-path target unguarded (`crates/ordo-sema/src/lib.rs:4737-4744`, `crates/ordo-plan/src/lib.rs:1281-1300`); still pre-fix and fully disclosing, not nulling | Code state is pre-fix; ruling is the spec | No: implement the ruling (S6-001). The GC007 record does not list the bridge site; add it |
| 9 | `docs/plan-blob.md:336` and `crates/ordo-blob/src/pipeline.rs:184` "an object moved between fields refuses to open" (read as the integrity guarantee for sealed content) | AAD is `seal:{entity}:{field}` only (`crates/ordo-crypto/src/lib.rs:89`, `crates/ordo-crypto/src/stream.rs:153`); the digest is not checked on `get` (`crates/ordo-host/src/blob.rs:736`) | Statement is true and narrow; the custody threat model needs more | RULE: identity binding and verify-on-get (S6-003) |
| 10 | `crates/ordo-exec/src/import.rs:181-186` refusing `..` "closes the seam for a bundle that arrived from elsewhere" | `std::fs::read` and `path.is_dir()` follow symlinks (`import.rs:187`, `:205`) | Code | No (S6-011) |
| 11 | `docs/plan-import.md` section 8 (bounded memory for "a million rows", line 55, 426) and the blob object cap `pipeline.rs:356` "BEFORE the sealer" | Members read whole (`import.rs:187`, `:1404`, `:1931`); cap consulted after the buffer exists (`crates/ordo-host/src/blob.rs:791-792`) | Code | No (S6-008) |
| 12 | `import.rs:1024-1039` code comment names the "quietly converged" outcome as the one to avoid | `import_ops` still derives create with no `rev` (`import.rs:1045-1107`) | Code (the comment describes the defect it ships) | RULE: refuse vs. converge (S6-007) |
| 13 | `crates/ordo-identity/src/access.rs:667` checks `MembershipStatus::Suspended` first | Status is always Active on the table-driven path, so the branch is dead | Code | With row 1 |
| 14 | `docs/audit-security-trace.md:30` "No contradiction between the reviews and the code was found" | Rows 1 to 13, and the SH dispositions in section 5 below | Reviewers (code evidence) | Correct the trace |
| 15 | `docs/audit-security-trace.md:26-28` counts (13 closed, 8 homed, 10 untraced) | The tables list 13 closed, 9 homed (SH-8, 10, 15, 16, 17, 20, 23, 26, 30) and 9 untraced (SH-2, 7, 11, 12, 14, 18, 22, 24, 29); 13+9+9 = 31 | Tables (the summary is wrong; the trace's own note at lines 70-71 half-admits it) | Correct the trace |

Note on row 1: `docs/deploying.md:1130` also says "takes effect on the very next request", but it is about quota counters, not membership, and is not a contradiction.

## 4. Prior findings against the trace (`audit-security-trace.md`)

The trace said its ten untraced findings were "probably" answered by later work and that none contradicted the code. The reviewers traced them with code evidence. Both positions are shown; the code-evidenced position is marked.

| SH id | Trace position | Reviewer position | Code evidence favours |
|---|---|---|---|
| SH-11 KeyProvider compromise blast radius | Untraced | A: partly closed, partly open. Bounded per space; keys never leave provider (`custody.rs:262-311`); every `release` metered and streamed (`metrics.rs:1205-1245`); destroy is per-space crypto-erasure (`custody.rs:561-596`). Still open: a compromised process holds the deployment master in RAM (`master.rs:69-74`), so a worker compromise is all-spaces for the process lifetime; no per-space release-rate alert found. | A. Open, narrowed. |
| SH-18 bearer tokens bind to no device | Untraced; guessed "probably" closed by 20b durable credentials | A: still open by design. `client` claim exists (`token.rs:144`) but nothing binds it; verification is pure HMAC (`token.rs:191-217`). Mitigations are logout deny list and short expiry. | A. The trace's guess is wrong on the code. |
| SH-22 profile templates unbounded | Untraced | A: moot for this build. No caller-supplied template input; profile built deployment-wide (`lib.rs:1685-1758`). | A. |
| SH-24 catalog exposes entity/field names | Untraced | A: closed. `catalog_for`/`interface_for` filter by the door's own `access.check` (`lib.rs:930-995`); verified live, anonymous or non-member gets `[]`. Caveat: this closes SH-24 for names, and S6-001 shows the manifest's guard claims can still be wrong. | A, with the S6-001 caveat. |
| SH-29 AccessProfile no per-member overrides | Untraced | A: closed in the engine (`access.rs:724-741`, `:876-937`), unwired at the edge; no production surface populates overrides. | A (mechanism closed; surface is future work). |
| SH-14 correlation ids across spaces | Untraced; guessed "probably" closed by per-space chains and scoped `revert --group` | B: closed in scope. Every lookup space-scoped (`commandlog/store.rs:108-125`, `:483-501`); `RevertError::CrossSpace`; `adopt` bounds external correlation (`secevent/correlation.rs:105`). | B. |
| SH-25 feed copies survive crypto-erasure | Closed (`erase.rs:81`) | B: closed on documentation (`erase.rs:115`, `exit_ops.rs:661`); exported bundles on disk are outside any op's reach. | Agreement. B places `RETAINED` at `erase.rs:115`; the trace says `:81`. Line drift, not disagreement. |
| SH-10 crypto-erasure vs. backups | Homed, unbuilt (BYO-DB consent page) | B: open, honestly named (`erase.rs:24-31`); mechanism real (keys before rows, `erase.rs:408`). | Agreement. |
| SH-7 the mirror has no storage pins | Untraced | B: open as designed; `Mirror` is the only evictable consumer (`prune.rs:519-537`); erasure reports `stranded_mirrors` (`erase.rs:302`). | B. |
| SH-12 envelope as behavioural side channel | Untraced | B: open; ciphertext length is deliberately plaintext (`blob/lib.rs:128`). Also see S6-003: the envelope's AAD scope is now a finding, not only a side channel. | B. |
| SH-2 feed copies re-index under receiver's salt | Untraced | B: not reached in its scope. | Neither; still untraced. |

Retention signing unverified heads (the previously found defect): B confirms **closed** at its root (`checkpoint_scope`, `crates/ordo-exec/src/secevent.rs:1197`; `retain_scope:1366`, `:1380-1399`) and found no second instance.

## 5. What the reviewers agreed is strong (do not break it)

Custody and the door (A):
- The custody trait exposes operations, never keys; sealing, opening, blind index and handle minting occur inside the provider under a scoped `KeyLease` (`custody.rs:262-311`). S6-003 and S6-009 fixes must not weaken this.
- Epochs are wrapped, not derived, so crypto-erasure is a real delete; `destroy_space` also takes the SMK because blind-index keys are SMK-derived (`custody.rs:27-35`, `:571-596`, `:812-968`; `keystore.rs:458-478`).
- Sealed-master file: argon2id at OWASP profile, 0600 via `create_new`, `key_id` bound as AAD, indistinct failure (`master.rs:194-343`).
- Master-in-backup tripwire at boot and `ordo keys init` (`runtime.rs:2653-2669`, `keys.rs:216-226`).
- Token verification: MAC before parse, constant-time compare, versioned positional payload that refuses older widths (`token.rs:188-217`, `:292-353`, `:378-383`).
- One uniform door answer for unknown op, forbidden and bad token; delegation gate placed before registry lookup (`router.rs:421-444`, `:639-667`).
- Space bound, then checked against that space, then re-verified by the executor; `$space` and `$caller` unshadowable; every source space-predicated with `verify_scoped` (`router.rs:582-591`, `:621-628`, `:824-826`; `lib.rs:660-706`).
- Idempotency keys keyed on (space, principal, op, key) with payload fingerprint (`router.rs:861-877`).
- Link ceremony needs fresh presence from an already-linked authenticator (`directory.rs:204-259`, `:550-612`).

Log, stream, blob, import, erasure (B):
- "A write is not an INSERT" holds through import: bundle values go through the same `run_write_on`, inheriting seals, blind indexes, guards (never `unguarded`), ids, images and chain append (`import.rs:1606`, `:1082`); `str_literal` refuses control characters and escapes a closed set (`:953`); ids reach SQL only through `sql_lit`.
- The command chain and security stream are separately keyed and verified: distinct domain tags (`secevent/chain.rs:119`), checkpoint domains (`store.rs:378`), signer env vars, advisory-lock namespaces (`secevent.rs:129`); cross-decode fails on the domain tag (`secevent.rs:396`).
- Verify-before-sign is structural and prune re-verifies the prefix it destroys (`prune.rs:997-1036`; `secevent/store.rs:640`).
- Erasure ordering and discovery: keys before rows (`erase.rs:409`), blob objects before `$blobref` truncation (`:438`), one CTE-per-table statement under SERIALIZABLE holding the append lock (`:601`), table list from `information_schema` (`:479`), system-space refusal before confirmation (`:388`).
- Blob key scheme refuses rather than sanitises (`space_prefix`, `blob/lib.rs:323`); `FsStore` is `production_ready() == false`.

Constraint that follows for every fix in this document: S6-003 must not touch `ordo keys rewrap-master` (no envelope moves) or the import's destination-id sealing; S6-001 must keep outer-preserving semantics; S6-002 and S6-005 must keep the zero-DB-round-trip check.

## 6. Prioritized plan

Sizing: S under a day, M a few days, L a week or more. Documentation corrections come first because they are cheap, remove false claims immediately, and make the later code changes land against a stated target. Items needing an owner ruling are separated so mechanical work does not wait on them.

### P0

Mechanical (no owner decision):
1. **S** Correct `docs/audit-security-trace.md`: the count table (13/9/9), the "no contradiction" line, and the dispositions for SH-11, 14, 18, 22, 24, 29 per section 4 above (document correction only).
2. **S** Add the bridge site to the GC007 record in `docs/corpus.md` and `docs/corpus-rulings.md`, and record `S6-001` as a Critical there (record only).
3. **M** S6-001: implement the GC007 ruling for `Join` and the same `ON`-guard for `SourceJoin` bridges, in one change with one conformance test spanning both; derive the manifest guard claim from the executed plan.
4. **S** S6-002 mechanical half: add `status` to `MEMBERS_SQL` and call `suspend` in `engine_from_members` (needs the chosen freshness wording from the P0 decision below only for the doc text).

Needs OWNER DECISION:
5. **S to decide, M to build** S6-004/S6-005/S6-002 freshness model: correct the docs to "next rebuild" (S) or build invalidation-bus membership freshness (M). Doc lines to change under either ruling: `docs/deploying.md:748`, `:776`, `crates/ordo-host/src/auth.rs:386-390`.
6. **M** S6-003: rule whether the AAD binds row id (and handle for blobs), and whether `get` verifies the digest; then implement and add tests that a transplanted value and a swapped blob refuse.

### P1

Mechanical:
7. **S** S6-006: record the manifest digest under the import name and refuse a mismatching resume (or fold content into the fingerprint).
8. **M** S6-008: bound member size before read; stream blob members into `BlobPipeline::put`.
9. **S** S6-011: canonicalize and refuse symlink escape in `DirectorySource`.
10. **S** Settle S6-003's open questions by test: run `ordo verify` after a DB-side transplant; check blind-index disagreement on a `sealed indexed` column (produces the evidence to keep or drop the Critical).
11. **S** S6-007 test: one `ordo-exec/tests/import.rs` case with two rows sharing a unique key, to establish current behaviour.

Needs OWNER DECISION:
12. **S to decide, S to build** S6-007: refuse vs. converge on within-bundle duplicate keys.
13. **S** S6-009: warn only, or add argon2id to `local`, or raise the floor.

### P2

Mechanical:
14. **M** S6-012: bind parameters through `$n` slots so caller data never re-enters the parser (or add the lexer-token test).
15. **S** S6-013: a test asserting every leader-only worker owns a `claim_*` guard.
16. **S** S6-010: closed by item 5's ruling; verify a member write triggers refresh without the writer remembering to announce.
17. **M** Re-verify S6-001, S6-002, S6-012 on an applied-program Postgres-backed server (all were proven on the dev-embedded path only).
18. **M** Re-run this review's untested areas: two-space bridge test (open question 1), two-replica suspension timing (open question 3).

## 7. Open questions for the owner

Questions no reviewer could settle. These are for the decisions board, not chat.

1. **Cross-space reach of S6-001.** Reviewer A verified cross-member disclosure within one space and believes composite FKs and space predicates confine it, but did not build a two-space program or tamper with a bridge FK. Recommendation: one multi-space dev test before declaring it confined.
2. **Applied-server parity.** S6-001, S6-002 and S6-012 were proven through `ordo dev` (`FixtureExecutor`), which its docs say uses the same gate and engine. A Postgres-backed `ordo apply --host` run was not done.
3. **Replica-fleet timing.** Suspension and logout across replicas (deny-list cadence, member-announce fan-out) were read, not run against two live replicas.
4. **S6-007 outcome.** Whether the `$importmap_minted` unique index turns a duplicate key into a hard refusal in every statement ordering, or a silent overwrite. One targeted test settles it.
5. **S6-008 reachability.** Whether the OOM is reachable through a real staging surface (delivery is unbuilt per `plan-import.md` section 11.6), and the size at which it falls over.
6. **S6-003, chain images.** Whether the row's hash-chain images expose a transplant on a later `ordo verify` (not verified by the lead).
7. **S6-003, blind indexes.** Whether the HMAC of the plaintext stored beside the envelope makes a transplant detectable by envelope/index disagreement, and whether any path compares them.
8. **S6-003, ranged reads.** A partial or ranged blob `get` cannot check a whole-object digest cheaply. Does the owner accept verify-on-get for whole reads only, with AAD identity binding carrying the ranged case?
9. **Freshness model** (S6-004): "membership takes effect at the next rebuild" or "on the next request". The docs currently claim the second and the code implements the first.
10. **Future membership-mutation ops** (S6-010): none exist; whoever builds the first must not depend on remembering to announce.
11. **SH-2** remains untraced by anyone in this round.

---

## 8. Fixes and rulings, 2026-09-28

The owner answered the five board rows the same evening. What follows is
what was built against those answers and what the building itself found.

### S6-001 is fixed, and auditing for it found a third instance

Owner: *"Fix (see if there are any other places where a similar issue could
come up, as it seems something similar was previously found already)."*

**Every site where a table enters a query was audited**, which is thirteen
`From::Table` constructions in `ordo-sema` plus the two join nodes, and the
`whole` planner in `ordo-plan`:

| Site | What brings the row in | Verdict |
|---|---|---|
| `entity_source` | a source the author listed | guarded, always was |
| `walk_refs` → `Join` | a dot-hop, `task.project.name` | **unguarded (GC007). Fixed** |
| `bind_multi_source` → `SourceJoin`, bridge | an entity inserted to connect two listed sources | **unguarded. Fixed** |
| `bind_multi_source` → `SourceJoin`, term | a listed source | guarded: its guard is in its own filters, and a source with filters is lowered as a sub-select that carries them |
| `whole` signpost | the one-hop object for each ref a row holds | **unguarded. Found by this audit. Fixed** |
| `whole` owned child | the owned closure | guarded by inheritance in the usual case; now guarded explicitly, because a child may declare a guard of its own |
| `write_value_expr` | a computed write right-hand side | anchor is the row being written; its dot-hops go through `walk_refs` and are covered by the same fix |
| `guard_filter_query`, `guard_hop_exists`, `guard_atom_cond`, `bare_query` | hops **inside a guard body** | not re-guarded, by design: a guard is the author's rule and evaluates to a boolean. Recorded, because a guard on one entity that reads another is an inference channel the author has chosen |
| `let`, union arms, sub-selects | built from sources | guarded through `entity_source` |

**The third instance.** `whole` returns a row plus a one-hop object for every
ref it holds. The anchor's guard was inlined and each signpost's target was
read with the space predicate and nothing else. Proven before it was fixed:

```
whole cases("cas_1")   as a caller who may not read the dossier
{"dossier":{"id":"dos_1","secret":"the secret"},"id":"cas_1","title":"Case one"}
```

The module's own doc said a whole-read *"discloses exactly what an ordinary
read of that row would, no more"*. **It is not reachable from a served
deployment today**: nothing in `ordo-host`, `ordo-exec` or `ordo-api` calls
the whole planner, so this was latent rather than live. It is fixed before
anything serves it.

**The fix is one decision in one place.** `Binder::joined_bound` makes, for a
row the compiler brought in, the same three-way decision `entity_source`
makes for a row the author listed. `Join` and `SourceJoin` each gained a
`guard` field, rendered in the join's `ON` for the reason the space predicate
already is: the join is LEFT so the near row survives, and a target the
caller may not read becomes indistinguishable from a ref that is null. In
`whole`, one `row_guard` function now serves the anchor, the signposts and
the children, where before only the first had one.

**Tests, each falsified.** `crates/ordo-conformance/tests/guards_compiler_joins.rs`
carries its own program, because `market.ordo` cannot hold the falsifying
case (every dot-path in it reaches a target whose guard is implied by its
referrer's). Four tests: the dot-hop, a filter through the dot-hop so that it
is not an oracle, the bridge, and the signpost. With the dot-hop guard
removed the first two fail and print `"secret": "the secret"`; with the
bridge guard removed the third fails and prints `"label": "Vault"`; the
fourth was watched failing against the unfixed planner.

### S6-007 is fixed, and its description here was wrong

Owner: *"Refuse."*

`import.<entity>.create` now carries the create-once belief (`rev: null`)
under exactly `std_ops`' condition, so a second bundle row under a unique
key that already holds one is refused with `expected_absent`.

**Falsifying the test corrected the finding.** Section 2 says two rows under
one unique key *"converge silently"*. They did not. With the belief removed,
the write protocol converges, the importer finds no `created` row in the
answer, refuses with *"the protocol accepted … and reported no created
row"*, and the batch rolls back. So the duplicate was already refused, **by
accident**, with a message that names the wrong thing. The defect was a
refusal nobody designed and nobody could read, which is Minor rather than
Major. The fix makes it deliberate and makes it say the key is taken.

Both the reviewer and the code's own comment described the silent version.
Neither had run it.

### Rulings recorded

| Row | Owner's answer | Status |
|---|---|---|
| `sec6-guards-on-joins` | Fix, and look for other places | **Built**, three sites |
| `sec6-import` | Refuse | **Built** for the duplicate key; the fingerprint, the size caps and the symlink rule are mechanical and still owed |
| `sec6-local-master` | Take recommendation | Ruled: refuse `ORDO_KEY_PROVIDER=local` outside development. Owed |
| `sec6-membership-snapshot` | *"I don't know what a member is here, but yes this should be fixed and meet spec"* | Discuss; answered in the board row. The spec is `definition-language.md`'s `members` table: `status` is `active` / `suspended` and *"suspension is a command, not a delete"* |
| `sec6-transplant` | *"This sounds computationally expensive at run time"* | Open; answered in the board row. The cost is in the row below |

**On the cost of S6-003's fix, since it was the objection.** Binding a row's
identity into the associated data adds the row id, about thirty bytes, to
what the cipher already authenticates on every seal and every open. There
is no extra query, no extra storage and no extra round trip; the cipher is
already running and is handed slightly more to read. The expensive variant
is a different proposal, verifying a stored file's whole digest on every
read, and **that one is withdrawn**: once the handle is in the associated
data a swapped file fails to decrypt, so the digest check on read buys
nothing the cheap fix has not already bought.
