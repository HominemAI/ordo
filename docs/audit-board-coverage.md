# Audit — what the readiness board does not account for

Scope: the Ordo Readiness Board (14 components, 62 features, 5 blockers), read
against the design corpus rather than against `roadmap-checklist.md`. The board
was assembled from the checklist, and the checklist tracks **work items** —
things somebody decided to do. A design document describes the **system**,
including parts nobody has opened a work item for. Those are what this audit
looks for.

Audit files (`audit-*.md`, `critique-*.md`, `security-hardening-review*.md`)
were excluded by instruction — they are records, not designs.

**Outcome legend**

- **(a) covered** — on the board already, possibly under another name; the row
  is named.
- **(b) missing** — settled in a design document, no row anywhere on the board.
- **(c) not owed** — described, but marked `[lean]` / `[open]` / `[proposed]`,
  so arguably not a feature yet. Filed so it is not mistaken for a gap.

**Code column** — verified by grep against `crates/`, not inferred from the
checklist. `none` means the capability has no implementation; `substrate` means
the machinery underneath it exists but the capability itself does not.

---

## 1. Headline — six subsystems with no board representation

The board's six groups are *language/reads, language/authorization,
language/writes, custody & audit spine, door & runtime, tooling & assurance*.
Nothing in that partition has a home for **automation**, **sharing across
spaces**, **distribution**, **client surfaces**, or **schema evolution**. Five
whole design documents and two settled specification sections land in that hole.

1. **The automation tier — routines and triggers.** `definition-language.md` §7
   is titled `## 7. Triggers [settled]` — settled outright, not under the §8
   `[lean]` pin. It specifies listener and clock primitives, a closed
   event-kind vocabulary (`created`/`converged`/`deleted`/`written`/`ingested`/
   `fed`/`reverted`), `debounce`/`coalesce`, the blind-but-addressable
   `$trigger` context, at-least-once delivery with dead-lettering, and the
   archive-floor guarantee for listener cursors. §8 adds tier-1 routines with
   `each ->`, derived scope manifests, and `routine:<name>@<version>`
   attribution. **The board has no row for any of it**, and one of its rules is
   an authorization rule: `guards-conformance.md` R-G20 (cases GC096–GC098)
   makes *trigger provenance the authorization bound* — a listener-woken
   routine may touch only rows reachable from `$trigger.ids` unless it carries
   `unguarded`. That is the confused-deputy closure for `ingested`/`fed`
   triggers fired by outsiders, and it is invisible on a board whose
   authorization group has eleven rows.

2. **Inter-space sharing — guest windows and consented feeds.**
   `definition-language.md` §8 settles both doors: the *window* (an `acl` row
   naming a non-member, guest calls under their own identity, attributed in the
   granting space's chain) and the *copy* (a consented feed landing as ordinary
   writes attributed `source:feed-<name>`, so the receiver's own triggers fire).
   `guards-conformance.md` R-G18 adds the compile-time gate: an op exposed to a
   guest must be **guard-bearing**, or `guest_exposure_unguarded`. Eight corpus
   cases (GC059–GC066) plus GC063–GC065 pin it. **No board row.**

3. **Domain packages.** `domain-packages.md` end to end, plus
   `std-library.md` §6 — which settles the namespacing outright
   (`std.<pkg>.<entity>.<verb>`, exports as the semver-stable surface) — plus
   `definition-language.md` §8's `domain … @ <semver>` block and its `bootstrap`
   block. **No board row.**

4. **Generated UI.** `ui-gen.md` §1–§4 are `[settled]`: surface descriptors as
   `out/surfaces.json`, shape-verified against the catalog; typed-id action
   discovery; CAS-by-default commit; catalog-scoped descriptors ("an app's
   renderable surface *is* its catalog"); the headless runtime; static
   renderers; ejection as an off-ramp. `std-library.md` §9 treats generated UI
   as the std library's north star. **No board row.** Worth a ruling on its
   own: `definition-language.md` §13 still lists generated UI under *Open
   items* as "undesigned", which `ui-gen.md` contradicts.

5. **Migration into and out of Ordo.** `migration.md` §1–§5 are `[proposed]`
   (outcome (c) — see the table). But **§6, "Migration out", is `[settled]`**,
   restating `definition-language.md` §8's exit ops: full space export and
   space deletion always exist, owner-only, step-up gated, command-logged,
   BYO-DB included. `licensing.md` makes that the *first of the three legs*
   the whole no-gate licensing argument rests on. **No board row, no code.**

6. **Schema evolution — the deployment diff engine.**
   `definition-language.md` §9 is `[settled]`: one plan/apply engine across all
   definition kinds, three change classes (additive / transforming /
   destructive), four sub-classes of defaulted-field addition, declared
   mappings, backfill jobs, sealed transforms routed through the worker,
   per-space shape versions replayed per BYO-DB target. `implementation.md`
   adds the three-deploy invariant and the resumable chunk-checkpointed
   backfill (`[proposed]`) — and the **write barrier / freeze is `[settled]`**
   (X-071 / Z-011 / C4-008). The board's `apply / rollback / deploys` row
   covers the *artifact* registry half only. `ordo-schema::ddl::ddl_for` emits
   a whole schema from scratch; there is no diff, no `ALTER`, no backfill
   anywhere in `crates/`. With the shape-verification gate shipped and
   refusing any boot whose database disagrees with its compiler, a deployed
   Ordo schema currently **cannot change**.

---

## 2. Findings

Ranked by consequence: authorization and custody first, then correctness gates,
then convenience.

| # | Document · section | Capability | Outcome | Code | Suggested board row |
|---|---|---|---|---|---|
| 1 | `definition-language.md` §7 (`[settled]`); §8 Routines (`[settled]` concepts under a `[lean]` file-format pin); `guards-conformance.md` R-G20, GC096–GC098 | **Trigger + routine tier**: listener/clock primitives, closed event-kind vocabulary, `debounce`/`coalesce`, `$trigger` context, at-least-once + dead-letter, `each ->`, derived scope manifests, and **trigger provenance as the authorization bound** | (b) | **substrate only** — `ordo-exec/src/jobs.rs` has the job queue, `$schedules`, clock leadership and LISTEN/NOTIFY; `routine`/`trigger` are reserved words in `ordo-syntax/src/token.rs` with **no AST node and no parser production**; no `routine:` principal runs anything | New group *Automation & events*: `Job queue & clock leadership` — **Shipped**; `Triggers (listener / clock)` — **Not started**; `Routines (tier-1 rules)` — **Not started**; `Trigger provenance bound (R-G20)` — **Not started** |
| 2 | `definition-language.md` §8 Inter-space sharing; `guards-conformance.md` R-G18, GC059–GC066 | **Guest windows**: `share to guest:<name> { op }` exposure registration, the guard-bearing compile gate (`guest_exposure_unguarded`), guest principal kind, invitation-not-enumeration, custody-computed shareability, revocation at next call | (b) | **none** — `crates/ordo-conformance/tests/guards_corpus.rs:2752` states it outright: "share/exposure registration does not exist: there is no `share to guest:…`". The corpus's "guest" is an ordinary member holding one grant | language/authorization: `Guest exposure (share to guest)` — **Not started** |
| 3 | `definition-language.md` §8 Inter-space sharing | **Consented feeds**: one-way standing replication of a named query, both ends consent, copies land as ordinary writes (`source:feed-<name>`) so the receiver's triggers fire; the blessed app-to-app interface | (b) | **none** — no `feed` anywhere in `crates/` | language/authorization: `Consented feeds` — **Not started** |
| 4 | `definition-language.md` §8 Principals (`[settled]`); `bootstrap.md` §3.3 | **Delegation grants**: delegation is itself a grant — user → agent, expiring, revocable, command-logged, *distinct from the token's op allowlist*; the agent provisioning path requires a live one | (b) | **none** — no match for `delegat` in `ordo-identity`. `PrincipalKind::Agent` exists; the grant that makes it lawful does not | language/authorization: `Delegation grants (agent on behalf of user)` — **Not started** |
| 5 | `security-logging-design.md` §2 (`[pinned]`/`[settled]`), §3, §6 | **Authentication**: the `AuthProvider` trait and its methods — passkey/WebAuthn (**the sole step-up source**), password (argon2id), email OTP, OIDC, SAML, OAuth2; anti-enumeration in `begin`; the dual-proof authenticator-linking ceremony; last-authenticator lockout refusal; first-login provisioning policy | (b) | **partial** — `ordo-identity/src/provider.rs` has the trait, `LocalPasswordProvider` (hashing via `PlainTestHasher`, not argon2id) and `StubOidcProvider`. **No passkey/WebAuthn provider exists**, though `access.rs::StepUpRequirement` and `directory.rs` already gate on WebAuthn presence | door & runtime: `Auth providers & authenticator linking` — **Partial** |
| 6 | `security-logging-design.md` §7b (`[pinned]`); `implementation.md` Outbound egress (`[pinned]`); `definition-language.md` §8 Notification channels (`[settled]`) | **Outbound egress**: one HTTP client, one policy, six mandatory SSRF pins (https-only, resolve-then-pin with connect-to-resolved-address, no redirects, signed deliveries, credential-less egress identity, per-space consented allowlist) governing notification sinks, webhooks, feed delivery and import bridges | (b) | **none** — one comment in `ordo-crypto/src/lib.rs`, nothing else | custody & audit spine: `Outbound egress (sinks, SSRF pins)` — **Not started** |
| 7 | `definition-language.md` §2 Content types (`[settled]`) | **Blob / content custody**: `blob`, `blob private` (client-sealed under a passkey-PRF key), `uri`, `blob inline` with a policy size cap; the bytes→sealer→space-resolved-store pipeline; BYO bucket attachment as a granted, step-up-gated, logged op; reference GC; share links with the key in the URL fragment | (b) | ~~**none**~~ **BUILT 2026-09-08** — `plan-blob.md` B1–B5, checklist 17g. `ScalarTy` gained `Blob`/`Uri`; `ordo-blob` holds the store trait and two bindings; `ordo_crypto::stream` the chunked seal; `ordo_exec::blobgc` the reference GC floored at the undo horizon; `std.space.attachBucket` the granted BYO-bucket op. **Not** built: share links (wait on `private`), and `blob private` is declarable and refused by name at every path that would touch the plaintext (F7 Phase 6). The tier is built and **not mounted** — nothing constructs a `StoreResolver`, which is a config ruling somebody owes | custody & audit spine: `Blob & content custody tiers` — **Done, not mounted** |
| 8 | `api-engine-design.md` §7.1 (`[settled]`); `std-library.md` §2.1 rule 4 (`[settled]`) | **Per-request effective cost class**: `effective = registered ⊔ class_of(refinement)`; a sealed refinement field raises the class to blind-index or host-side; reject with `refinement_unbounded` when the effective class is unbounded-host and the op carries no `unbounded` acknowledgment | (b) | **none** — `ordo-api/src/refine.rs:73` explicitly defers it ("is the planner's and is not decided here"); no `effective` in `ordo-plan/src/cost.rs`; `ErrorCode` has **no** `RefinementUnbounded`, though `serialization_conflict` and `lease_fenced` both landed | door & runtime: `Effective cost class per request` — **Not started**. Security-bearing: a caller can refine on a sealed field and force whole-entity host decryption past a gate the op never acknowledged |
| 9 | `definition-language.md` §9 (`[settled]`); `implementation.md` Migrations (write barrier `[settled]`, rest `[proposed]`) | **Schema evolution engine**: additive/transforming/destructive classification, four defaulted-field sub-classes, declared mappings, resumable chunk-checkpointed backfill, per-space shape versions replayed per target, the write barrier/freeze during a transform | (b) settled core; (c) the three-deploy invariant and failure-window table | **none** — `ordo-schema::ddl::ddl_for` emits whole-schema DDL only; no `ALTER`, no diff, no backfill. `ordo-host/src/deploy.rs` is artifact registry + live-schema drift *refusal* | tooling & assurance: `Schema migration (plan diff, backfill, freeze)` — **Not started**. With shape verification shipped, a deployed schema cannot currently change |
| 10 | `migration.md` §6 (`[settled]`, restating `definition-language.md` §8 exit ops); `licensing.md` "Why there is no gate" | **Exit ops**: full space export (schema.sql, per-entity JSONL, blob bytes, command-log archive bundle) and space deletion by crypto-erasure — owner-only, step-up gated, command-logged, BYO-DB included | (b) | **BUILT 2026-09-06** — `ordo-exec/src/{export,erase}.rs`, `ordo-host/src/exit_ops.rs`, checklist 11-10, plan `docs/plan-export.md`. Blob bytes wait on blob custody and the manifest declares that; bundle delivery and the confirmation window remain | custody & audit spine: `Space export / deletion (exit ops)` — **Done**. This is leg 1 of the three-leg trust argument `licensing.md` prints |
| 11 | `implementation.md` Key custody (`[settled]`); `deploying.md` §4; `security-logging-design.md` §7 | **Key rotation and epoch lifecycle**: rotation as an attributed command group, SMK rotation = blind-index re-index (offline build + atomic swap under a write barrier), the lazy resumable re-seal job, epoch retire-then-destroy, the undo-horizon destruction gate, crypto-erasure | (b) | **partial** — `ordo-crypto/src/custody.rs` has `rotate`, retire, `destroy_epoch`, `Purpose::Reseal`, `LocalKeyProvider`, `WrappedKeyStore`; `sealed.rs` documents the undo-horizon gate. **No re-seal worker, no write barrier, no re-index path** | custody & audit spine: `Key rotation & epoch lifecycle` — **Partial**. The board's `Key custody` row stops at "SMK wrapping per-epoch DEKs" and says nothing about rotation |
| 12 | `security-logging-design.md` §7 (`[settled]`); `implementation.md` Client-first (`chainProof` `[settled]`) | **Attestation layers 2–4**: the opt-in per-space attestation webhook with an owner-signed destination the platform cannot silently redirect; witnesses (WORM bucket, git, OpenTimestamps, Rekor); the public transparency log; the exportable `chainProof(from, to)` op and its published offline verifier | (b) | **BUILT 2026-09-08** (checklist 11-12, `corpus-rulings.md`). Layer 2: `ordo_exec::webhook` + `std.space.attestTo`, owner-signed destination re-verified before every send, at-least-once outbox with backoff and dead letters, schema `ordo.attestation/1`. Layer 3's generic mechanism (the signed POST) plus **one** witness format — the owner's own HTTPS endpoint, §7's own cheapest-first entry; WORM/git/OpenTimestamps/Rekor are named as owed with a reason each. Layer 4: `std.space.chainProof` emitting `ordo.chainproof/1` + `ordo-verify-proof`, a binary with no driver and no socket. Underneath all of it, `ordo-egress` — §7b's six pins as types. The public transparency log stays **declined**, per the `attestation` decisions row. Still absent: the archive destination key in `LogPolicy` (no archive sink exists to point at) | custody & audit spine: `Attestation layers 2–4 (webhook, witness, transparency log)` — the board should now read **layers 1, 2 and 4 built; layer 3 one format of five; transparency log declined**, which is a different sentence from either "not started" or "done" |
| 13 | `implementation.md` Runtime shape verification; `std-library.md` §3 `std.catalog()`; `adoption-dx-plan.md` §8 item 10 (★ adoption-critical) | **Capability catalog**: verified shapes + registration metadata served per token, in two renderings (machine JSON for SDK generators, LLM-ready prose), republished atomically with hot swap; an empty catalog for an anonymous asker rather than a 403 | (b) | **exists** — `ordo-host/src/serve.rs` routes `/catalog` and `/.well-known/ordo-catalog`, scoped per op by the same checker the door uses, with `catalog_prose` as the second rendering | door & runtime: `Capability catalog` — **Shipped**. It is an *enforcement* input (grants pin manifest hashes) and the agent-facing surface, and it appears on the board only inside other rows' notes |
| 14 | `api-engine-design.md` §7 (`[settled]`); `tooling-and-embedding-plan.md` §B2; `std-library.md` §2.1 rules 1–3 | **Refinement envelope (`$refine`, Tier 0)**: narrowing-only structured conditions over the output manifest, `refinable: none \| basic` at registration, post-everything semantics, sealed-field rules (not refinable by default; `eq`/`in` only; aggregate-only ops accept none) | (b) | **exists** — `ordo-api/src/refine.rs` implements the tier check, manifest validation and all three sealed rules verbatim | door & runtime: `Refinement envelope ($refine)` — **Partial** (rule 4 is finding #8) |
| 15 | `bootstrap.md` §1–§4 (`[settled]`) | **Space provisioning lifecycle**: `std.space.create` as one atomic surface, the seven-phase model, the 46-item input inventory, the genesis command group at chain position 0, the `provisioning → active \| failed → reaped` state machine, the `bootstrapTimeout` reconciler, the per-resource compensation table, `std.space.reap` | (a) **partial** — the board's `std.space.*` row ("Blocked — needs a platform-op path") nods at the ops, but the lifecycle, reconciler and compensation table are invisible behind one line | **none** | Promote to its own row — custody & audit spine or a new *Platform lifecycle* group: `Space provisioning (genesis group, reconciler, reap)` — **Not started** |
| 16 | `bootstrap.md` §1.1 (`[settled — this document owns it]`); §8 (`[open]`); `api-engine-design.md` §2 | **Deployment-scoped ops**: the `scope deployment` op class that binds no space (`$space` becomes `unknown_param`) and is authorized against a *deployment* capability rather than a per-space profile | mixed — the op class is (b); the `CapabilityGrant` addition it needs and its wire rendering are **(c) `[open]`**, explicitly awaiting an owner ruling | **none** | Fold into the row above rather than filing separately; note the open ruling |
| 17 | `implementation.md` Client delivery (`[settled]`); `ui-gen.md` §2 | **Log-positioned reads / snapshot consistency**: every read carries the per-space command-log position; a multi-widget page holds one position so two surfaces can never straddle a command group; feed batches applied atomically per group. Also `heavy` streamed row batches and the client chunk cache | (b) | **none** — no `log_position`, no `heavy` in `crates/` | door & runtime: `Log-positioned reads (snapshot consistency)` — **Not started**. `ui-gen.md`'s headless runtime is specified on top of it |
| 18 | `domain-packages.md` (whole); `std-library.md` §6 (`[settled]`); `definition-language.md` §8 `domain` + `bootstrap` blocks | **Domain packages**: package-qualified namespacing, exports as the semver surface, artifact-hash identity, upgrade-as-deploy with per-space custody-aware classification, profile-template instantiation, uninstall dispositions, signed registry-as-data, package hash riding routine attribution | (b) for the settled core (namespacing, versioning-by-hash, scope-drift-kills-grants); **(c)** for the manifest format, export-list syntax, side-by-side majors and the hosted registry, all `[lean]`/`[open]` in §1/§7 | **none** | New group *Distribution*: `Domain packages` — **Not started** |
| 19 | `ui-gen.md` §1–§4 (`[settled]`), §3 static renderers (`[settled]`) | **Generated UI**: surface descriptors from `ordoc build`, shape-verified and catalog-scoped; typed-id action discovery; CAS-by-default commit; the `@ordo/react` headless runtime; static renderers; per-surface ejection | (b) for §1–§4; **(c)** for §5's block set, page composition and layout hints (illustrative), and §10's open items — note §5's *binding grammar* is explicitly normative | **none** | New group *Clients & surfaces*: `Surface descriptors` — **Not started**. Also flag the `definition-language.md` §13-vs-`ui-gen.md` status contradiction |
| 20 | `migration.md` §1–§5 (`[proposed]`) | **Migration in**: `ordo introspect` per source (Postgres, Supabase, Firestore, Convex, CSV/JSON), the mapping file, genesis import mode (batch command groups, summarized images, genesis checkpoint, quiet triggers), the verify pass and didn't-migrate manifest, three cutover patterns | **(c)** — the document marks §1–§5 `[proposed]`, "adopted here, pending owner review". Genesis import mode also carries an unresolved contradiction (`bootstrap.md` §4.3, Z-014) | **none** | Optional, low: tooling & assurance: `ordo introspect / import` — **Planned**. Not a gap until the proposal is ruled on |
| 21 | `security-logging-design.md` §6.1 (`[settled]`) | **Security-event stream** — the board's row reads "Partial · no drainer; denials sit in an in-process buffer", which **understates what shipped**: `ordo-secevent/src/chain.rs` has `hash_entry`, `verify_stream_from`, `verify_stream`; `store.rs` a store; `alert.rs` a `DenyVolumeMonitor` implementing the mandatory `ordo_denials_total` alert | (a) — but the note is wrong. Genuinely missing: the owner-facing read surface, a single `verify()` covering both chains, the dashboard explorer, and the drain tick | **exists** (chain, verify, store, alert) | Amend the note rather than add a row; the drainer is one of four remainders, not the whole story |
| 22 | `security-logging-design.md` §6.1; `implementation.md` Observability; `bootstrap.md` §8 | **Deployment operational security log** — the third artifact of the three-artifact split, where non-member, anonymous and unrelated-space denials land, and the only place a **cross-space sweep** is visible (a principal probing a hundred spaces appears in none of their streams, by design) | **(c) `[proposed]`** — `bootstrap.md` §8 names it "a whole missing subsystem" and it exists in no document as a design | **none** | Not a row yet. Worth naming in the security-event row's note so the members-only blind spot is not read as full coverage |
| 23 | `implementation.md` Backup, restore & DR (`[proposed]`); Observability (`[proposed]`) | **Backup / restore / DR**: PITR, the no-keys-in-the-backup startup assertion, the `system:restore` chain-discontinuity group, orphaned-checkpoint retention, the quarterly restore drill; and the OTel tracing / structured-log / metrics split | **(c)** `[proposed]` — and largely **(a)**: blocker 5f already names the missing restore mechanism and the missing histogram families | **partial** — `live_drift` exists; nothing appends `system:restore`; `metrics.rs` is a counter registry | No new row. Credit the board: 5f is accurate |
| 24 | `tooling-and-embedding-plan.md` (whole: "Status: plan, nothing here is building yet"); `adoption-dx-plan.md` §8 | **`ordoc` standalone, `interface.json` descriptor, LSP, generated SDK, in-language embedding, builder frontends** | **(c)** for Tracks A and C; the **B1 descriptor** and **B3 terminal-verb contract** are marked `[settled]`, and the generated typed SDK is `[settled]` in `implementation.md` | **none** | One row, tooling & assurance: `Interface descriptor + generated SDK` — **Not started**. The rest is a plan, not a gap |
| 25 | `plan-tiers.md` (`[proposed]`, "Not v1") | **Plan tiers**: compile-every-variant, per-`(space, op)` selection, guard/score/regime, shadow runs, split trials, client venue | **(c)** — explicitly deferred to Track F's operate phase | **none** | No row. But two items the document itself calls cheap-now-expensive-later are worth a checklist line: reserve a `tier` field in the artifact, and record observed cost from the first executor |
| 26 | `dashboard-plan.md` (`[lean]` inventory, `[settled]` custody posture) | **Operator dashboard** — panes as registry-driven plugins, three personas, and **break-glass operator mode** (`[settled as shape]`: explicit time-boxed grant, step-up gated so capability tokens can never satisfy it, command-logged, never a default) | **(c)** for the pane inventory; break-glass is an authorization capability settled as shape with mechanics `[lean]` | **none** | No dashboard row yet. Consider one authorization row: `Break-glass operator mode` — **Not started**, since it is the only sanctioned path into a space's data by a non-member |
| 27 | `eval-harness-plan.md` ("Status: plan") | **AI-authoring eval harness**: silent-wrong rate, parse-reject rate, adversarial trap corpus, longitudinal and competitive runs | **(c)** — a plan. The board's `Conformance corpus`, `Golden files` and `Doc-block parse harness` rows are adjacent but measure different things | **none** | No row |
| 28 | `licensing.md` | Not a capability document. Its one engineering item is `ORDO_APEX_DOMAIN` (**`[proposed]`**) and the WebAuthn RP-id immutability consequence — "changing the apex strands every existing passkey" | **(c)** | **none** — no `ORDO_APEX_DOMAIN` in `crates/`; the doc asks the engine to refuse to start without it | No row. Relevant to finding #5 |

---

## 3. Two board-accuracy notes found while checking

Not coverage gaps, but they undercut rows the board marks as complete.

1. **`Error inventories — Shipped, spec ✓ how-to ✓ tested ✓`, noted "both checked
   two-directionally against the documents".** `refinement_unbounded` is
   specified in `api-engine-design.md` §4 and §7.1 and in `std-library.md` §2.1
   rule 4, and is **absent from `ordo-api::ErrorCode`** — while
   `serialization_conflict` and `lease_fenced`, which arrived by the same route,
   are present. Whatever the two-directional check covers, it did not catch
   this one, which means the check is narrower than the row claims.

2. **`Security event stream — Partial`, noted "No drainer on any host path".**
   The row reads as though only a drainer is missing. The chain, `verify_stream`,
   the store and the mandatory deny-volume alert monitor all exist; the
   remainders are four (drain tick, owner read surface, a unified `verify()`
   across both chains, the explorer). Understating what shipped is the milder
   error, but it hides three of the four.

One item the board treats as blocked is now stale in the *documents*, not the
board: `guards-conformance.md` §0 still lists `unbindable_write` as having no
producer. It has one — `crates/ordo-conformance/tests/write_let.rs` — and the
board's `Write-valued let` row is correct.

---

## 4. Documents checked and found fully represented

Checked against the board with no gap warranted:

- **`api-engine-design.md`** — routes, envelope, header protocol, the closed
  error taxonomy, non-enumerability (§4.1), parameter binding, SSE live
  transport, page tokens, idempotency and its Postgres contract, version skew,
  hot swap. Every one maps to a board row (`Error inventories`, `Page tokens`,
  `Durable idempotency`, `Live subscriptions`, `Hot swap`, `Access checker`).
  Two exceptions are filed above as #8 and #14 (§7 and §7.1), and §4.1's
  residual `method_mismatch` / `missing_space` enumeration channel is `[open]`
  and disowned by the document itself.
- **`writing-data.md`** — every capability has a row: derived write modes, mode
  labels, `$caller` in RHS, two-phase guard evaluation, command groups and
  images, typed-id minting, single-transaction atomicity, the tree subsystem,
  `has_referrers`, the registration-time refusal class. Fully covered.
- **`typed-id-keys.md`** — covered by `Entities, fields, refs` ("typed-id keys
  derived per entity") and the `id_key_collision` gate in `deploying.md` §6.
- **`composition-conformance.md`** — call form, binding rules, callee-first
  ordering, one-plan flattening, manifest non-widening, cost class as max of
  inlined stages, marker non-inheritance, cursor-stability labels. All under
  `Named-query composition`, `Keyset pagination` and `Conformance corpus`. Its
  two settled-unimplemented items (`breaking_dependents` / `dependency_failed`,
  pending incremental registration) are checklist items, not board gaps.
- **`guards-conformance.md`** — R-G1 through R-G21 map cleanly onto the eleven
  authorization rows, including the create-exemption (R-G11a), the owner
  door capability (R-G13), non-enumerability (R-G10) and the `unguarded`
  marker rules (R-G8). The three exceptions are filed above as #1, #2 and #26.
  Notably, the twelve capabilities the document has **no** mechanism for —
  field-level masking, deny rules, re-delegation limits, expiring acl grants,
  impersonation — are absent by design, not by omission, and correctly have no
  board row.
- **`std-library.md`** — the `std.E.*` row, its `list`-blocked note, and the
  `whole`/`history` not-derivable note match §0 exactly; `std.space.*` matches
  the blocked row. §2.1 rules 1–3 are implemented and unlisted only as part of
  finding #14.
- **`deploying.md`** — pool, page size, registration gates, deploy/rollback,
  checkpoint and prune all have rows. Its §4 status warning ("the design below
  is settled and the code is not") is now out of date in the code's favour.
- **`engine-plan.md`** — a build plan whose Track P/Track F phases are the
  board's own spine; F7 has a row, F3 has a row, F5/F6 remainders appear as
  blockers. Represented.
- **`security-logging-design.md`** — apart from findings #5, #6, #12 and #22,
  every subsystem has a row: capability tokens, grant registry, step-up,
  suspension, the command log, `SealedRef` redaction, checkpoints, prune and
  the consumer floor.
- **`bootstrap.md`** — see #15 and #16; the rest (input inventory, prohibitions,
  operator checklist) is documentation of one op, not separate capabilities.
- **`definition-language.md`** — the read, write and authorization sections are
  densely covered; the gaps are §2 content types (#7), §7 (#1), §8 sharing
  (#2, #3), §8 delegation (#4), §8 exit ops (#10) and §9 (#9). §11 (`extend`)
  is a settled *decline* and correctly has no row; §8's `policy` blocks are
  partly built (`LogPolicy`) and partly `[lean]` (`vault`, `llm-egress`,
  `retention` — the last has only its `system:retention` actor string in code).

---

## 5. Suggested shape of the additions

The board's six groups cannot absorb these without distortion. Two new groups
and one rename would carry them:

- **Automation & events** — job queue & clock leadership (Shipped), triggers,
  routines, trigger provenance bound.
- **Platform lifecycle & distribution** — space provisioning, exit ops, schema
  migration, domain packages.
- **Clients & surfaces** — capability catalog (Shipped), surface descriptors,
  interface descriptor + generated SDK, log-positioned reads.

Into existing groups: `Guest exposure` and `Consented feeds` and `Delegation
grants` and `Break-glass operator mode` into *language/authorization*; `Blob &
content custody`, `Key rotation & epoch lifecycle`, `Outbound egress` and
`Attestation layers 2–4` into *custody & audit spine*; `Auth providers`,
`Refinement envelope` and `Effective cost class per request` into *door &
runtime*.

That is roughly 20 rows, of which four would be **Shipped** — which is itself
the finding: the board undercounts what exists almost as often as it omits what
does not.
