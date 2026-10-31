# Security round 6-B — command log, security stream, blob, export/import, supervisor, erasure

Adversarial hardening review, read-only. Scope: the command log and security-event
stream, blob storage and the upload path, bulk export/import (import is two days old and
unreviewed), the worker supervisor, and the erasure path. The door, guards and
provisioning are another reviewer's custody and are not reviewed here.

Every claim below was checked against the code at the cited `file:line`, not against a
status table or a doc's claim. Where I assert a negative I say what I walked.

## 1 · Executive summary

The worst thing I found is that **bulk import buffers every bundle member whole into
memory with no size cap** (`DirectorySource::read` is a bare `std::fs::read`, and blob
objects are read into a `Vec<u8>` before the pipeline's object cap is consulted), so a
single oversized staged member exhausts the host before any refusal fires (B-01). The
second is quieter and worse in kind: the import's resume fingerprint is **the batch's
source ids and nothing about the row values** (`batch_fingerprint`,
`crates/ordo-exec/src/import.rs:1557`), so the design's stated guarantee that "a changed
bundle under a used name refuses instead of skipping" (`plan-import.md` §11.5) is false for
a bundle whose ids are unchanged and whose values are not — a resumed import over such a
bundle skips the batches that already committed and applies the new values to the ones
that did not, and reports success over the inconsistent mix (B-02). The command-log and
security-stream tamper machinery is genuinely strong: the known "retention signed heads it
had not verified" defect is fixed at its root (signing now walks and verifies the whole
stream and signs the pair the walk arrived at, `checkpoint_scope`
`crates/ordo-exec/src/secevent.rs:1197`; retention refuses to cut on any signer refusal,
`retain_scope:1366`), and I found no second instance of that shape. The security-stream
read route and the correlation/space scoping are correctly space-bound and I could not
break their tenant isolation. **The single design decision I would push back on hardest**
is that `import.<entity>.create` is derived to carry **no `rev` for any entity, keyed or
not** (`import_ops`, `import.rs:1045`): a hostile or malformed bundle with two rows under
one unique key silently converges the second onto the first and calls it a load — a
create op that cannot refuse a duplicate key is not a safe way to admit foreign data
(B-03).

## 2 · Threat actors

- **Malicious tenant owner.** Holds owner role in their own space and a bearer token that
  passes every gate on the exit ops. Wants: to read or corrupt another tenant's space; to
  admit a principal into their space that the owner did not grant; to make their space
  look erased while data survives.
- **Malicious ordinary member.** Holds a member row and token in one space. Wants: to read
  the space's security stream, another member's images, or another space's data.
- **Operator who stages files but must not read plaintext.** Can write under
  `ORDO_IMPORT_BUNDLE_DIR` and `ORDO_EXPORT` roots and holds a shell on the host, but is
  not trusted with tenant plaintext. Wants: to read a tenant's opened data, or to steer an
  import into placing attacker bytes on a tenant's rows.
- **Curious/malicious operator with database credentials.** Can `SELECT`/`UPDATE` any
  table. Wants: to forge or launder a chain or stream tamper so a verifier passes green; to
  recompute blind indexes over an erased space; to suppress a checkpoint.
- **Compromised or lagging worker/replica.** Runs the supervisor's loops. Wants: to
  double-apply or skip a destructive pass (prune, retention, erasure) via split-brain.
- **Holder of a stolen or hostile export bundle.** Has bytes shaped like an
  `ordo.space-export.v1` bundle and, in the migration story, a plausible route to get an
  operator to stage it. Wants: memory exhaustion, path escape, mis-placed rows, or an
  authorship/identity lie.
- **Someone with write access to the object store.** Can add/replace/delete objects under
  a space prefix. Wants: to substitute bytes an owner later reads as their file.
- **Supply chain.** Owns a dependency. Out of scope for depth here; noted where a trust
  boundary leans on one.

## 3 · Findings, worst first

### B-01 · Import reads every bundle member whole into memory, with no size cap — Major

- **Threat actor:** holder of a hostile/oversized bundle, plus the operator who stages it
  and the owner who runs the import (owner-only, step-up gated).
- **Claim being attacked:** `plan-import.md` §8 discusses "a million rows" and bounded
  per-group memory, and the blob pipeline advertises an object cap
  (`BlobPolicy::admits_object`, checked in `pipeline.rs:356` "BEFORE the sealer"). The
  implied claim is that import is bounded against a large input.
- **Attack path (concrete):**
  1. Stage a bundle whose `manifest.json` declares one entity and whose
     `data/<entity>.jsonl` is, say, 8 GB of valid JSONL — or a `blobs/…` member of
     arbitrary size — with the correct `sha256` recorded (the attacker controls the bytes,
     so the digest is trivial to compute).
  2. Import calls `member()` → `DirectorySource::read` (`import.rs:187`) which is
     `std::fs::read(self.root.join(name))` — the **entire** file into one `Vec<u8>`. The
     JSONL body is then `String::from_utf8(body)` (`import.rs:1404`), a second full copy,
     and `rows_of` holds every parsed `Row` (`import.rs:1202`).
  3. For a blob member, `attach_objects` calls `member(src, mf, &o.member)`
     (`import.rs:1931`) → the whole object into memory, then hands it to the door as
     `ImportObject::bytes: Vec<u8>`. `PlatformBlobs::attach` (`ordo-host/src/blob.rs:791`)
     sets `declared_len = object.bytes.len()` — so the pipeline's `admits_object` cap
     (`pipeline.rs:356`) is only consulted **after** the object is already fully resident.
- **Evidence:** `crates/ordo-exec/src/import.rs:187` (`std::fs::read`, no length check),
  `:1404` (`String::from_utf8`), `:1931` (object read whole); `crates/ordo-host/src/blob.rs:792`
  (`declared_len` taken from the already-buffered `Vec`); no `max_object`/streaming check
  precedes any of these. Contrast the export path, which streams in 64 KiB steps and
  asserts it (`pipeline.rs` module doc, `a_hundred_megabyte_upload_never_holds_the_object`).
- **Proposed pin `[proposed]`:** cap member size before reading — refuse a member whose
  `std::fs::metadata().len()` exceeds a policy bound (reuse `BlobPolicy::max_object` for
  blob members and a separate JSONL bound), and read blob members as a `ByteStream` off
  disk into the existing `BlobPipeline::put` streaming path rather than collecting first,
  so the object cap refuses at zero-resident-bytes as it already does for a client upload.
- **What the fix must not break:** the round-trip property (an export's own members must
  still import), and the digest check, which must run over the streamed bytes rather than a
  buffered copy.

### B-02 · Resume fingerprint covers source ids only, so a mutated bundle resumes silently — Major

- **Threat actor:** holder of a hostile bundle under a re-used import name; also an honest
  operator who edited a bundle and re-ran, who is silently given a corrupt result.
- **Claim being attacked (quoted, cited):** `plan-import.md` §11.5 — "a changed bundle
  under a used name refuses instead of skipping"; and `batch_fingerprint`'s own doc,
  "makes a re-run of a **changed** bundle under the same import name answer
  `idempotency_mismatch` instead of silently skipping" (`import.rs:1551`).
- **Attack path (concrete):**
  1. Run import `M` over bundle `B`; it commits batches 0..k and is interrupted (or
     completes).
  2. Replace `B`'s row **values** in place — same `id`s, changed fields (e.g. flip an
     `amount`, a status, an owner ref target's payload) — and re-run import `M`.
  3. `batch_fingerprint(chunk)` is `bundle_digest(join("\n", source_ids))`
     (`import.rs:1557`) — it hashes **only the ids**. For every already-committed batch the
     lease's stored fingerprint still matches, so `claim_lease` returns `LeaseState::Applied`
     and the batch is skipped (`import.rs:1639`, `lib.rs:1661`). Batches not yet committed
     run and apply the **new** values.
  4. Result: an import that reports `batches_skipped` + `batches_written` = success, over a
     space that is half the old bundle and half the new one, with no `idempotency_mismatch`
     anywhere. The design's stated refusal never fires because the thing it fingerprints did
     not change.
- **Evidence:** `crates/ordo-exec/src/import.rs:1557` (fingerprint = ids only);
  `crates/ordo-exec/src/lib.rs:2150`–`2200` (`claim_lease` compares the stored `fingerprint`
  string; `Applied` when it matches and `done`). The manifest's per-member `sha256` is
  checked (`member`, `import.rs:869`), but only the *whole file's* digest, and only when the
  file is read this run — a skipped batch's member is still read this run (rows are read for
  all entities up front, `import.rs:1400`), yet the per-batch lease decision never consults
  that digest. So the file digest catches a truncation but does not gate the skip.
- **Proposed pin `[proposed]`:** fold the row **content** into `batch_fingerprint`
  (hash each row's canonical field set, not just its id), so a value change under a used id
  changes the fingerprint and `claim_lease` returns `Mismatch`. Alternatively, refuse a
  resume whenever the bundle's `manifest.json` digest differs from the one the first run
  recorded (store the manifest digest under the import name and compare on resume) — this is
  the cheaper pin and it matches the §11.5 promise directly.
- **What the fix must not break:** an *unchanged* bundle must still resume cleanly
  (idempotent re-run is the whole feature), so the fingerprint must be stable over identical
  content and its ordering.

### B-03 · `import.<entity>.create` carries no `rev`, so a duplicate unique key silently converges — Major

- **Threat actor:** holder of a hostile or malformed bundle; the outcome is silent data
  loss regardless of intent.
- **Claim being attacked (quoted, cited):** `import.rs:1024`–`1039` — the code itself
  states that `import.E.create` "carries no `rev` for any entity, keyed or not, so a source
  bundle with two rows under the same unique key would **silently converge the second onto
  the first** rather than refuse it. That is exactly the 'quietly converged' outcome the
  paragraph below warns against."
- **Attack path (concrete):** stage a bundle for an entity with a `unique` key that
  contains two JSONL rows sharing that key with different other fields. `import_ops`
  (`import.rs:1079`) emits a `Set` write with `OpFlags::default()` and no `rev`, so the
  second row's write is a converge onto the first row rather than a refusal. Both source ids
  land in `$importmap` (`import.rs:1614`), both point at the same minted row, and the
  minted-id unique index (`IMPORTMAP_DDL`, `import.rs:108`) — which exists precisely to keep
  one minted row answering to one source row — is **violated on the second insert**, so the
  batch may in fact fail there rather than silently converge; either way the outcome is
  wrong (a refusal reported as `row_refused` at best, a silent overwrite at worst depending
  on statement ordering within the batch). The row count check (`rows_of`, `import.rs:1237`)
  does not catch it because the file's line count is honest; it is the *keys* that collide.
- **Evidence:** `crates/ordo-exec/src/import.rs:1045`–`1107` (`import_ops`, no `rev` arm),
  `:1024`–`1039` (the acknowledgment), `:108` (`$importmap_minted` unique index that the
  duplicate would collide against).
- **Proposed pin `[proposed]`:** derive `import.E.create` with `rev: null` for keyed
  entities exactly as `std_ops` now does (the code notes 11-10a already fixed `std_ops`'s
  keyless arm), so a within-bundle duplicate key refuses with `expected_absent` before it
  can converge. If the duplicate-key policy is to stay deferred, then at minimum detect a
  repeated unique-key value across a bundle's rows at read time and refuse, rather than
  letting it reach the write.
- **What the fix must not break:** loading a legitimate bundle into an empty space, where
  there are no within-bundle duplicates, must still succeed; and the create semantics import
  relies on (there is nothing to converge onto in an empty space) must hold.

### B-04 · The staging directory reader follows symlinks — Minor

- **Threat actor:** operator (or anything that can write under `ORDO_IMPORT_BUNDLE_DIR`),
  and a hostile bundle assembled off the export path.
- **Claim being attacked (quoted, cited):** `import.rs:181`–`186` — "a caller-supplied
  `..` in one would read outside the bundle … refusing costs nothing and closes the seam for
  a bundle that arrived from elsewhere." The seam is claimed closed against a foreign bundle.
- **Attack path (concrete):** the reader refuses `..` and empty segments but does nothing
  about symlinks. `DirectorySource::read` is `std::fs::read(self.root.join(name))`
  (`import.rs:187`), which **follows** a symlink; and `names()` uses `path.is_dir()`
  (`import.rs:205`), which follows a symlinked directory into the walk. A member
  `data/orders.jsonl` staged as a symlink to a path outside the space's staging tree (a
  sibling tenant's staged bundle, or any host file) is read through. Exploit value is bounded
  by the digest+manifest agreement (the bytes must match the recorded `sha256`, so an
  attacker can only redirect to bytes they already know), and the space component in the path
  is still an authorization boundary for *which directory* is named. So this is containment
  erosion rather than a direct cross-tenant read: it lets a staged bundle pull in content
  from outside the two-directories-under-root invariant the door's `staged()` doc relies on
  (`exit_ops.rs:395` "the resolved path is therefore always exactly two directories under the
  root" — true for the door's own join, but `DirectorySource` then reads members relative to
  it and can leave the subtree via a link).
- **Evidence:** `crates/ordo-exec/src/import.rs:187` (`std::fs::read`, follows links),
  `:205` (`path.is_dir()`, follows links); contrast `crates/ordo-exec/src/creds.rs:370`
  which *does* `canonicalize` and refuse a symlink escaping its mount, so the codebase knows
  the pattern.
- **Proposed pin `[proposed]`:** in `DirectorySource`, `canonicalize` the resolved member
  path and refuse if it does not start with the canonicalized root (the `creds.rs` pattern),
  or open with `O_NOFOLLOW` on the final component and refuse a symlinked member.
- **What the fix must not break:** a legitimate directory bundle (all regular files under
  the root) must still read; the export never writes symlinks, so refusing them costs the
  honest path nothing.

### B-05 · Supervisor leadership has a split-brain window; safe only because the destructive passes claim transactionally — Note

- **Threat actor:** lagging/partitioned replica.
- **Claim being attacked:** the supervisor elects a single leader for leader-only workers.
- **Mechanism:** leadership is a session `pg_try_advisory_lock` on a dedicated connection
  (`jobs.rs:398`), probed each `leader_tick` (default 5s, `supervisor.rs:182`). Between a
  leader's session dying and its next probe, a follower that acquires the lock and the old
  leader both believe they lead for up to one tick. This is **not** a correctness bug for
  the streams in scope: the security-stream retention and checkpoint passes gate on
  `claim_retention_tick` / `claim_checkpoint_tick`, which are transactional schedule claims
  (`secevent.rs:1037`, `:1311`), and `Leadership`'s own doc states leadership "is **not**
  load-bearing for correctness" (`jobs.rs:382`). I record it because the safety rests
  entirely on every leader-only worker having its own transactional claim; a future
  leader-only worker that skipped one would double-run destructively during the window.
- **Proposed pin `[proposed]`:** none required now; keep the invariant explicit — a
  leader-only worker must own a `claim_*`-style transactional guard, asserted in a test, so
  the split-brain window can never become load-bearing.
- **What the fix must not break:** the millisecond hand-off on a rolling restart, which
  depends on dropping the leadership connection explicitly before the pool closes.

## 4 · Prior findings re-examined

| Finding | Disposition | Evidence |
|---|---|---|
| **Retention signs heads it did not verify (the one already found/fixed)** | **Closed** | Signing walks the whole stream and signs the pair the walk arrived at (`checkpoint_scope`, `secevent.rs:1197`, `try_verify_to_head:619`); `retain_scope` step 1 is `checkpoint_scope` and any `Refused` stops the pass before it looks for a cut point (`secevent.rs:1380`–`1399`); `WouldAnchorTamperedHistory` names the exact laundering path and refuses it. |
| **SH-2** feed copies re-index under receiver's salt | **Out of scope / untraced here** | Feeds/sharing are the door reviewer's; nothing in the log/blob/export/import/supervisor/erase scope re-indexes a received copy. Left as `security-hardening-review` had it. |
| **SH-7** the mirror has no storage pins | **Partly answered in scope** | Prune's floor treats `Mirror` as the only evictable consumer and `Archive`/`ClientFirstMirror`/`Listener` as pinning (`prune.rs:519`–`537`, `erase.rs:38`–`47`). A *mirror* still pins nothing in storage terms — consistent with SH-7 — and erasure reports it as `stranded_mirrors` rather than blocking (`erase.rs:302`). Open as designed, not a defect. |
| **SH-12** the envelope as a behavioural side channel | **Untraced / open** | The blob envelope binds AAD to `(entity,field)` and seals mime+size+integrity (`pipeline.rs:39`–`52`, `stream.rs:153`), and `bytes` (ciphertext length) is deliberately plaintext because a bucket listing discloses it anyway (`lib.rs:128`). So size is still an observable side channel by design; the "behavioural" channel SH-12 means (access timing, presence) is not addressed in scope. Open. |
| **SH-14** correlation ids resolve across spaces; revert planning unscoped | **Closed in scope** | Every log lookup is space-scoped: `Store::find`/`revert_group` require a space and `is_logged_elsewhere` returns only a boolean (`commandlog/store.rs:108`–`125`, `:483`–`501`); `RevertError::CrossSpace` makes a cross-space attempt loud rather than silent. Import/attach correlations are per-space deterministic strings (`import.rs:1587`, `:1949`) and `adopt` bounds any external correlation to `[A-Za-z0-9._-]{1,64}` or mints a fresh one (`secevent/correlation.rs:105`). |
| **SH-10** crypto-erasure claimed while backups unaddressed | **Open, honestly named** | `erase.rs:24`–`31` states row deletion "answers the live database and nothing else" and that key destruction is what answers backups; `plan-export.md` §4.6 and `audit-security-trace` home this on the BYO-DB consent page, "not there yet". The mechanism (keys before rows, `erase.rs:408`; blind-index keys SMK-derived so the SMK is destroyed too, `plan-export.md` §4.2) is real and present. Residue (`$secevents` principal ids + op names) is disclosed on `RETAINED` (`erase.rs:115`). Open as a documentation/consent gap, not a code defect. |
| **SH-25** feed/other copies survive crypto-erasure, undocumented | **Closed on documentation** | `RETAINED` is the named, reasoned list of what erasure keeps (`erase.rs:115`), the op returns it in its answer (`exit_ops.rs:661`), and export bundles already written to disk are outside any op's reach by construction — the same class SH-25 named, now stated rather than silent. |

## 5 · What is genuinely strong (do not break it)

- **"A write is not an INSERT," enforced through import.** Import never writes columns; it
  builds `import.E.create(...)` source and runs it through the same `run_write_on`
  (`import.rs:1606`), so seals, blind indexes, guards (no `unguarded` flag ever,
  `import.rs:1082`), ids, images and the chain append are all inherited. Bundle values reach
  the language through `str_literal`, which refuses control characters and escapes only the
  lexer's closed set rather than stripping quotes (`import.rs:953`); source and destination
  ids reach SQL only through `sql_lit`. I looked for a bundle-controlled string reaching SQL
  unescaped and did not find one.
- **The two tamper artifacts are separately keyed and separately verified.** The command
  chain and the security stream use different domain tags (`secevent/chain.rs:119`), different
  checkpoint domains (`store.rs:378`), different signer env vars, and different advisory-lock
  namespaces (`secevent.rs:129` vs `APPEND_LOCK_NS`), with a behavioural test asserting the
  lock independence. A `$commandlog` body stored in `$secevents` fails to decode on the
  domain tag rather than verifying as something (`secevent.rs:396`).
- **Verify-before-sign is now structural, and prune re-verifies the prefix it destroys.**
  Command-log prune verifies the anchor's signature and the *base* checkpoint, then verifies
  the whole prefix about to be deleted from the chain's actual first-stored index
  (`prune.rs:997`–`1036`) — not from the latest checkpoint, which would skip the doomed
  entries. `check_prune` re-verifies signature, range and stored head (`secevent/store.rs:640`).
- **Erasure ordering and discovery.** Keys (DEKs + SMK) before rows (`erase.rs:409`), blob
  objects before `$blobref` truncation (`erase.rs:438`), one CTE-per-table statement under
  SERIALIZABLE holding the append lock (`erase.rs:601`), and the table list discovered from
  `information_schema` rather than hand-written (`erase.rs:479`) so a new space-scoped
  platform table is swept without being named. The system-space refusal is checked before the
  confirmation (`erase.rs:388`).
- **Blob key scheme refuses rather than sanitises.** `space_prefix` refuses any id outside
  `[A-Za-z0-9_-]{1,128}` (`blob/lib.rs:323`), keeping the space→prefix map injective; the
  object key carries space+handle and nothing that would enumerate the schema or a revocation
  timeline (`lib.rs:55`). `FsStore` is `production_ready() == false` and refuses to boot
  outside dev.

## 6 · What I could not determine, and what I would need

- **Whether B-01's OOM is reachable through a real deployment's staging surface.** The door
  itself says "delivering a bundle to the deployment" is unbuilt (`plan-import.md` §11.6) and
  the operator stages by hand today. I confirmed the code path buffers whole; I did not stand
  up a host with a staging directory to measure the failure (that needs a running server on a
  throwaway DB, and another agent shares the cluster). To confirm the size at which it falls
  over I would run one `std.space.import` against a large staged member on an isolated
  Postgres.
- **Whether the `$importmap_minted` unique index turns B-03's duplicate key into a hard
  refusal or a silent converge in every statement ordering.** The index would collide on the
  second insert, but whether that surfaces as `row_refused` or is masked depends on batch
  statement ordering inside `import_batch`'s closure, which I read but did not execute. A
  single targeted `ordo-exec/tests/import.rs` case with two rows sharing a unique key would
  settle it; I did not run it to avoid a concurrent cargo build against the shared cluster.
- **The blob store-substitution threat (write access to the object store).** The read path
  opens with AAD bound to `(entity,field)` and a per-object nonce, and `BlobPipeline::verify`
  checks a sealed integrity digest (`pipeline.rs:666`) — but I did not confirm that the
  serving read path (`ordo-host/src/blob.rs:736`) calls `verify` before returning bytes; it
  appears to open-and-return without an integrity check on the fetched object (the digest is
  used by revert and by an explicit verify, not on every `get`). Determining whether a
  store-side byte substitution that preserves length and AEAD-decrypts is caught on read
  would need a trace of every `get` caller against `verify`; I flag it as unconfirmed rather
  than assert it.
