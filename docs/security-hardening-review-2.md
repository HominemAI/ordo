# Ordo Security Hardening Review — Round 2

Adversarial re-read after the round-1 fixes. Scope: verification of SH-1…SH-31
against the current docs **and** the current crates; first security review of
material that is new since round 1 (**groups**, **denial chain entries**, the
**`routine:` principal**, **genesis import**, **exit ops**, **outbound sinks**,
**break-glass**, **licensing**); and a re-audit of `ordo-api`, `ordo-identity`,
`ordo-commandlog`.

**Everything here is [proposed].** Findings are written as pins because that is
the reviewable form, not because they are adopted. The four commitments still
constrain every pin: per-space sovereignty is a dial; everything encrypted
unless granted; consent is never in config; disclosure is derived, not declared.

Two framing observations before the tables.

- **The round-1 P0s that were about *one file* got fixed well.** Idempotency
  (SH-17), the error taxonomy (SH-5), the DEK story (SH-9), sealed refinement
  (SH-4), the mirror (SH-7) are genuinely closed or close to it, and several are
  better than what round 1 asked for. The round-1 P0s that were about *a
  dimension threaded through many files* — space, and blind-index derivation —
  are half-threaded, and the half that is missing is the executor.
- **The new material is where the risk now lives.** Groups, denial chain
  entries, the `routine:` principal and genesis import were all specified since
  round 1 and none has been reviewed. Three of the four introduce an
  authorization or audit hole larger than anything round 1 found. The pattern is
  consistent: each is a feature whose *bound* is asserted in prose in the same
  paragraph that describes it, and in each case the prose is falsified by
  another paragraph of the same document.

---

## Part A — Round-1 verification

**Closed 5 · Partial 9 · Open 17.**

`CLOSED` = the attack named in round 1 is structurally unavailable.
`PARTIAL` = the primary attack is closed, a named residual survives.
`OPEN` = the round-1 attack still works as written.

| # | Round-1 subject | Status | Residual attack / what is missing |
|---|---|---|---|
| **SH-1** | Blind-index salting | **PARTIAL** | Derivation is per-space (`definition-language.md` §"Sealed fields": "salted per-space") and per-epoch (`implementation.md`: "from `(SMK, epoch)`"). **Entity and field are still not in the derivation.** Inside one space, `patients.email` and `staff.email` produce identical digests for the same value, so a DB-provider snoop joins across entities and pools frequency analysis across every field sharing a value domain. Cross-*space* linkage is closed; cross-*entity* linkage is not. |
| **SH-2** | Feed copies re-index under receiver's salt | **OPEN** | No document says a feed copy recomputes blind indexes. `definition-language.md` §"Consented feeds" specifies the copy with no index rule. If digests ride the copy, SH-1's cross-space join returns through the front door with consent as cover — the exact round-1 attack, untouched. Genesis import is not covered either (§SH2-9). |
| **SH-3** | Digest truncation / bucketing | **OPEN** | No mention of truncation, bucketing, or a candidate-set-plus-confirm read path anywhere in `docs/`. `normalize(lower)` + blind index on auth-grade email remains a full-precision dictionary target. |
| **SH-4** | Sealed refinement oracle | **PARTIAL** | Doc side is **excellent** and closed: opt-in per field, equality-only, no sealed conditions under aggregate-only ops, effective cost class recomputed per request (`std-library.md` §2.1 rules 1–4, `api-engine-design.md` §7.1). Two residuals: (a) `ordo-api`'s `FieldSpec` carries **only a name** — no sealed flag, no refinable flag — so `refine::validate` cannot enforce rules 1–3 at the door and `RefineTier` is per-*op*, not per-*field*; (b) the oracle survives at reduced resolution through `list` row counts and page fill (§SH2-12). |
| **SH-5** | `not_found` / `access_guard` / `access_denied` | **PARTIAL** | Code is **closed and well-reasoned**: `ErrorCode` and `ExecError` both carry `NotFound` and `AccessGuard`; `access_denied` is door-only; unreadable rows answer `not_found`. Two doc-level residuals: `definition-language.md` **G12 still mandates `access_guard`** for an addressed write outside the read view and explicitly argues against the `not_found` blur that `api-engine-design.md` §4.1 adopted; and the door-denial code is specified three different ways (G18 `access_denied`, api-engine §4.1 `unknown_op`, code `access_denied`). See §SH2-13. |
| **SH-6** | MAC'd, principal-bound page tokens | **OPEN** | `page.rs` is unchanged: `base64url(JSON{v,k})`, no MAC, no principal binding, row values in the payload, still accepted in the query string. Forged cursors still seek arbitrarily in sort space — and now compose with page fill into a sharper oracle (§SH2-12). |
| **SH-7** | Client mirror storage rules | **CLOSED** | `implementation.md` §"The local store is a database" adopts every pin — device-key sealing under passkey-PRF, auto-lock with idle timeout, per-space stores, `policy mirror` disclosure, `std.space.*`/log slices never mirrored, revocation poisoning — and states the offline-attacker limit without euphemism. Extended to *every* mirror, not only client-first. Still `[proposed]`, and `policy mirror` is absent from the policy grammar. |
| **SH-8** | Device enrollment / escrow | **CLOSED** | Enrollment is a step-up-gated command-logged op; removal rewraps and rolls the epoch forward; "segments it already holds are gone" is stated plainly; escrow is an explicit dial choice with "no silent escrow, ever." |
| **SH-9** | DEK rotation | **CLOSED** | Two-level SMK/DEK_n with epoch tags shipping before first customer data, rotation as a logged op with three named triggers, lazy resumable re-seal on the conversion job, and an unusually honest "what rotation does not fix." Residuals are new findings, not gaps in the pin: the SMK itself never rotates, and rotation is the maximum-key-exposure window (§SH2-11). |
| **SH-10** | Erasure vs backups | **PARTIAL** | Strong: "Nothing key-shaped" in backups, enforced by a **startup assertion**; a restore restores ciphertext; the provider's soft-delete window is conceded. Missing: the `KeyProvider::destroy(space) -> DestroyReceipt` API, and the command-logged receipt whose stated residual window the confirmation dialog derives from. The honesty is prose; the mechanism is not built. |
| **SH-11** | Key-release blast radius | **PARTIAL** | Release is derived from op access and logged; `ordo_key_release_total{purpose}` + distinct-spaces-per-principal is a mandatory breadth alert. Missing: (a) **refusal of releases with no live correlation** — correlation is a log field, not a precondition; (b) **worker releases are not covered** — workers attribute as `system:<component>` and the derived-from-op-access rule has no analogue for a backfill or re-seal job; (c) the release log has **no stated integrity properties** — it is the operational stream, unchained, which is the one thing round 1 said it must not be. Rotation makes (b) acute (§SH2-11). |
| **SH-12** | Envelope pseudonymization | **OPEN** | `ordo-commandlog::Command` still stores plaintext `entity`, `row_id`, and a plaintext actor string, all hashed into the chain. No pseudonymization, no clock-granularity policy. `implementation.md` §Observability now concedes the same leak for telemetry ("discloses entity names, actor, and timing at every custody level") without proposing a fix. |
| **SH-13** | Owner-verifiable chain | **OPEN**, partly by deliberate trade | `Signer::verify` still defaults to re-sign-and-compare (symmetric); `StubSigner` is an FNV fold; `chainProof` is `[proposed]` only. Client counter-signing is now **explicitly declined** by the merge rule, honestly: "even at the top of the dial, ordering is platform-authored… the platform can in principle drop, delay, or reorder proposals. Detection, not prevention, is the property being offered." That trade is defensible — but it is currently written in `implementation.md` and nowhere near a consent page. |
| **SH-14** | Space-scoped `find` / `revert_group` | **OPEN** | `Store::find(&self, correlation)` still takes no space, and `InMemoryStore::find` still does `self.chains.values().flat_map(…)` — a scan of **every space's chain**. `revert_group(store, correlation, now)` still takes no space and no principal. A guessed or leaked correlation id still yields another space's before/after images. New and worse: `append` calls `find` for duplicate detection, so correlation ids are **globally unique across tenants** (§SH2-14). |
| **SH-15** | Feed re-share provenance | **OPEN** | No origin chain, no `noReshare`, no downstream revocation notice. |
| **SH-16** | Guest catalog / acl co-holder scoping | **OPEN** | No exposure-scoped catalog rule, no guest exclusion from the acl-derived link entity's read guard, no guest mirror TTL. Groups make the co-holder leak worse: an acl row naming a group discloses group membership to everyone else on the row. |
| **SH-17** | Idempotency replay | **CLOSED** | `ReplayKey = (space, principal, op, key)` with a SHA-256 `RequestFingerprint` over `space ‖ op ‖ version_hash ‖ bound params ‖ refinement ‖ cursor`, length-prefixed and domain-separated. Fingerprint is over **bound** params, so transport spellings normalize — correct. Mismatch is a hard 409, never a miss and never a hit. TTL and entry cap present. Postgres backing and same-transaction commit are pinned in `implementation.md` §Deployment topology, and multi-node on the memory impl is called a **correctness defect**. Residual is a new finding: no in-flight reservation (§SH2-15). |
| **SH-18** | Token binding / `presence_at` | **OPEN** | `TokenClaims.fresh_user_presence` is still a `bool`. No DPoP, no channel-bound step-up, no caller-identity binding. A token minted with presence carries step-up authority for its entire life — which now gates **export** (§SH2-16) and break-glass. |
| **SH-19** | `$space` in the request | **PARTIAL** — and inverted from what round 1 expected | **Code closed it well:** `$space` is in the closed envelope key set, `X-Ordo-Space` is the header form, header-vs-envelope disagreement is `missing_space`, there is no default, and it binds **before** `check_access` in `gate()`. **Docs never closed it:** `api-engine-design.md` §2's closed `$`-key set is still `$refine`/`$page` only, there is no `missing_space` in §4's taxonomy, and §12 records the binding as an unresolved "hard prerequisite… a larger ruling than this document owns." **And the dimension stops at the door:** `OpExecutor::execute` receives no space (§SH2-1), and the capability path of `AccessEngine::check` ignores it entirely (§SH2-2). |
| **SH-20** | Delegation budgets | **OPEN** | Delegation is still one prose paragraph. No budgets, no rate/volume caps, no per-op opt-in for destructive ops, no declaration syntax, no op to mint or revoke one — and no delegation-resolution step anywhere in `security-logging-design.md` §6's access-check pipeline, so `agent:<name> on behalf of user:<id>` has no specified evaluation point. |
| **SH-21** | Package publisher keys | **OPEN** | No TOFU/pinning, namespace claims, edit-distance warnings, or yank records. |
| **SH-22** | Profile-template diff | **OPEN** | Template application is still a side effect of enabling a domain. Now compounded: templates can seed **group→op allowlists** (§SH2-7). |
| **SH-23** | Break-glass monitoring | **OPEN** | The shape is unchanged and good (explicit grant, step-up, time-boxed, command-logged, never default, ciphertext for E2E). **None of the four round-1 pins landed**: no owner notification on grant/first-use/expiry, no deployment-level budget or per-operator counter, no `system:break-glass/<grant-id>` chain tag, no declinable pre-authorization with computed capability loss. There is also no approver distinct from the grantor and no revocation path short of expiry. |
| **SH-24** | Catalog / descriptor trimming | **OPEN** | No exposure-scoped catalog. Worse than round 1: the error paths now enumerate the manifest for free (§SH2-17). |
| **SH-25** | Erasure propagation across feeds | **OPEN** | Not addressed. Genesis import adds a second escape route: the id-mapping table (§SH2-9). |
| **SH-26** | Sink payload classes | **OPEN** | "Payload class stated (minimized vs full)" is unchanged; nothing computes or compile-checks either class. Plus a new and larger issue in the same feature (§SH2-3). |
| **SH-27** | Manifest hashing | **PARTIAL** | The *semantics* improved substantially: grants now pin **per-op** hashes plus ordered output field sets, which correctly separates narrowing from expansion — better than round 1 asked for. The *primitive* did not: `ManifestHash` is still FNV-1a 64, and `combined_manifest_hash` still FNV-folds op names and per-op hashes into 64 bits, in the one code path a malicious package author controls the inputs to. |
| **SH-28** | Denial-volume alerting | **CLOSED** | Mandatory alert, named metric, and an explicit "a deployment that silences these is misconfigured and the dashboard says so." Two soft spots: `ordo_denials_total{principal_kind,op}` has **no space label** while the alert is specified "per principal per space", and per-space series are opt-in for cardinality; and "above baseline" is undefined. |
| **SH-29** | Per-member overrides | **PARTIAL** | Docs settled it well — overrides are commands, union-then-override ordering is pinned, deny-by-override beats allow-by-group, `callersOf` stays exhaustive with the *via*. Code has not followed: `AccessProfile` is still `ops_by_role` with no override map and no group map. |
| **SH-30** | Live-stream re-check | **OPEN** | Confirmed in code: `live_inner` access-checks once at subscribe and never again; nothing closes a stream on revocation. Worse than round 1 knew: the bus is not space-scoped (§SH2-8). |
| **SH-31** | Auto-onboard | **PARTIAL** | Structurally sound — `resolve_or_onboard` mints a principal and links an authenticator, and grants **no membership in any space**, so the round-1 concern is closed by construction. The rate limit is still not stated, and the onboarding event only reaches the operational stream. Now load-bearing for a new attack: minted principals are the collapse-key diversity that defeats denial rate-collapse (§SH2-5). |

---

## Part B — New findings

### B1. The space dimension stops at the door

**SH2-1 — `OpExecutor::execute` never receives the space. SH-19 is closed at the door and open one line later.**

*Loop.* `router::gate` binds the space, passes it to `AccessChecker::check`, and
stores it on `Gate`. `run_call` then uses `gate.space` for exactly one thing —
the replay key — and calls:

```rust
state.executor.execute(op, bound, refinement, cursor).await
```

No space. The trait has no space parameter, and `api-engine-design.md` §12's
trait table describes `OpExecutor` as "run a prepared plan with bound params /
refinement / cursor" — so this is the *documented* contract, not an
implementation lag.

*Attack.* The executor must obtain a space from somewhere, and there are only
three candidates, all bad. (a) **From `BoundParams`** — caller-controlled,
never cross-checked against `gate.space`: send `$space: spc_victim` to satisfy
the access check and a `space` parameter naming another space to steer
execution. The door approves one space; the executor reads another. (b) **From
the token** — which collapses the multi-space product, the thing SH-19's fix
existed to prevent. (c) **From ambient/global state** — a single default
database, at which point space isolation is decorative. This is SH-19's original
attack moved one layer down, and the layer it moved to is the one that touches
data. `implementation.md` also makes the space the BYO-DB resolver key and the
chain-append advisory-lock key, both of which need it here.

*Pin [proposed].* `execute(op, space, bound, refine, page)` — space as a named
parameter on the trait, not a member of `BoundParams`, so it cannot be
shadowed by an op parameter of the same name. The executor asserts that the
space it plans against is the space the door approved; a disagreement is
`internal`, never a silent choice. `$space` is stripped from the parameter map
before binding (the router already does this via `split_envelope`) and is
**rejected as a declared op parameter name** at registration. Add the
integration test the suite is missing: the harness `AccessChecker` already
denies `space != SPACE`, and no test exercises it.

*Must not break.* Nothing. This is the model both crates already claim.

**SH2-2 — Capability grants have no space dimension at all; an agent token works in every space in the deployment.**

*Loop.* `AccessEngine::check(claims, space, op, now)`:

```rust
if let Some(grant) = &claims.capability {
    // expiry → manifest hash → allowlist
} else {
    // human path: this is the ONLY branch that reads `space`
}
```

The capability branch never touches `space`. `CapabilityGrant` is
`{ ops, manifest_hash, expires }` — no space field. Every capability test in
`tests.rs` uses one space, so nothing catches it.

*Attack.* An agent, source, or API client consented in the family-budget space
calls its allowlisted ops against **any space id it names** and the engine
allows it. The op names are deployment-wide (`std.transactions.list`,
`finance.setCategory`), so the attacker needs only a space id — and space ids
leak through invitations, share links, URLs, and support tickets. Round 1's
SH-20 pinned "delegation is per-space, never per-principal-globally"; this is
that hole, in the layer below delegation, for every synthetic principal. It is
strictly worse than the human path, which is correctly space-scoped, and it
inverts the design's own claim that capability tokens are the *tightest*
authority.

Note the interaction with SH2-1: with a space-blind executor **and** a
space-blind capability path, a compromised agent token is a deployment-wide
read.

*Pin [proposed].* `CapabilityGrant` gains a mandatory `space: SpaceId`. The
capability branch checks `grant.space == *space` **first**, before expiry, so
the cheapest check is also the isolating one. A grant spanning two spaces is two
grants, consented twice — which is already what `definition-language.md` says
about the tier-2 bridge ("capability tokens for both spaces, consented on each
side"). Sources declared in IaC name their space in the declaration.

*Must not break.* One consent page per space is already the model; this makes
the token match it.

### B2. Outbound sinks

**SH2-3 — The sink URL is user-supplied, unvalidated, and fetched from inside the deployment perimeter. This is SSRF to the cloud metadata endpoint.**

*Loop.* `definition-language.md` §"Notification channels": "an outbound HTTP
hook destination carrying a space's data is a per-space granted sink —
space-owner consent naming the endpoint." The endpoint is named by a customer.
Across all 25 docs there is **no** target-URL validation rule, no scheme
restriction, no loopback/RFC1918/link-local deny-list, no DNS-rebinding
protection, no TLS requirement, and no outbound request signing.

*Attack.* Any space owner — i.e. any paying customer, the lowest bar in the
threat model — grants a sink pointing at `http://169.254.169.254/latest/meta-data/iam/…`
(or the GCP/Azure equivalent, or `http://localhost:5432`, or an internal
service on the pod network). The platform makes the request from inside the
perimeter and, because sinks deliver payloads and log deliveries, hands the
response or its metadata back to the tenant. On a cloud deployment that path
ends at instance credentials, and instance credentials end at the
`KeyProvider`. **A single-tenant feature yields deployment-wide key custody
compromise**, which is the one outcome the entire custody design exists to
prevent. Second-order: the absence of a `https`-only rule means an operator-path
network adversary reads every sink payload in cleartext; the absence of
signing means any sink receiver can be spoofed by anyone who learns the URL.

*Pin [proposed].*
- **`https` only**, certificate validation mandatory, TLS 1.2 floor. No opt-out;
  a self-hosted deployment that wants plaintext sinks forks.
- **Resolve-then-pin egress**: resolve the hostname at grant time and at each
  delivery, reject any resolution in loopback, link-local (169.254/16,
  fd00::/8), RFC1918, CGNAT, or the deployment's own pod/service CIDRs, and
  **connect to the resolved address**, not the hostname, so a rebind between
  check and connect cannot win. Redirects are not followed.
- **Sign every delivery**: `X-Ordo-Signature` = HMAC over timestamp ‖ body under
  a per-sink secret shown once at grant, with a replay window. A sink receiver
  must be able to authenticate the platform.
- **Egress runs from a dedicated identity with no instance-credential access** —
  a separate egress worker or an outbound proxy — so even a bypass of the above
  lands somewhere that holds nothing.
- Round-1 SH-26 still stands on top of this: `minimized` derived from the
  envelope tier, `full` from the op's manifest, compile-checked.

*Must not break.* Sinks stay grants, never config; the consent page stays
computed. The deny-list is disclosed on that page ("this sink may not point
inside the deployment"), so the restriction is derived like everything else.

### B3. Genesis import

`docs/migration.md` §3 is `[proposed]` and has never been security-reviewed. It
is the largest new attack surface in the corpus, because it is the one write
path that deliberately weakens attribution, triggers, and images at once.

**SH2-4 — Import has no stated authority gate, suppresses triggers by a flag on the envelope, and closes with a checkpoint that declares prior history out of scope.**

*Loop.* Three sentences, from three bullets of the same section:
- Actor: "`source:import-<name>` — a source principal… IaC-declared."
- Triggers: "Batch envelopes carry an import flag; listener triggers do not fire
  per batch."
- Closure: the genesis checkpoint's "meaning is stated plainly: **history before
  this point is the source system's problem**."

And a fourth fact, from the whole document: **nothing anywhere states who may
run an import.** Export and deletion are `owner-only, step-up gated,
command-logged`. Import — which mints a write actor, suppresses the reactive
layer, writes summarized images, and signs a boundary — carries no stated role,
no step-up, no capability. §5's dual-write bridge then keeps the same
quiet-flag rules running against a **live** space for an open-ended window.

*Attack.* Three, ascending.
1. **Trigger kill-switch.** The import flag rides the *envelope* — writer-set,
   not engine-derived. Anyone who can set it silences every listener-driven
   routine in the space, including whatever the domain uses for fraud checks,
   approval workflows, or alerting, for as long as they keep writing. There is
   no stated bound on the quiet window and no statement about whether concurrent
   ordinary user writes fire their triggers during it.
2. **History laundering.** Rows arrive with **no per-row before/after images**
   (only count, id set, and a content hash the importer computed over what the
   importer wrote), `std.E.history` shows one entry — "arrived in import X,
   batch N" — and the genesis checkpoint asserts that everything before the
   boundary is somebody else's problem. In a product whose entire pitch is the
   tamper-evident chain, import is a supported path for injecting data with the
   audit trail switched off.
3. **Guards are unspecified on the import path, and the specified answer is
   broken either way.** `migration.md` never says whether guards evaluate on
   imported writes. But `definition-language.md` §3 says there is "no path at
   all for `routine:`, `worker:`, `source:`, or `system:` principals" to hold a
   members row, and G15 says a principal with no members row makes every guard
   evaluate false. So a `source:import-*` actor either (a) fails every write
   into a guarded entity, making import unusable in exactly the domains that
   need it, or (b) bypasses guards silently — the same G15 problem routines got
   a `[settled]` ruling for and sources did not.

*Pin [proposed].*
- **Import is an exit-op-class operation**: owner-only, step-up gated,
  command-logged, with the same ceremony as export. It is the *inbound* half of
  the same trust boundary and deserves the same door.
- **The import flag is engine-derived, never writer-set.** It is a property of
  a live, owner-authorized import session bound to one mapping name and one
  expiry — not a bit on an envelope. Sessions appear in the chain at open and
  close; the quiet window has a **maximum duration** and its expiry is a logged
  command.
- **Quiet is per-import-actor, not per-space.** Ordinary writes during the
  window fire their triggers normally; only commands attributed to that
  import actor are suppressed. State this explicitly — the dual-write bridge
  makes the window long enough to matter.
- **Rule the source-principal guard question**, in `definition-language.md`
  where G15 lives, not in `migration.md`. The defensible answer: import writes
  run with guards **evaluated and satisfied by an explicit owner authorization
  recorded in the session**, not bypassed — so "who authorized this data to
  exist" has an answer that is a person.
- **Blind indexes and sealing on import** get the round-1 SH-2 rule verbatim:
  computed under the receiving space's derived key, on the ordinary write path.
  `migration.md` states sealing ("Custody is respected, not deferred") and is
  silent on indexes.

*Must not break.* Bulk import at scale — none of the above adds per-row images
or per-row trigger fan-out. The summarized-image design is sound; it is the
missing door and the writer-set flag that are not.

**SH2-5 — The id-mapping table is an unsealed, unguarded, undated re-identification key that survives crypto-erasure.**

*Loop.* Five mentions, total, across `migration.md`: a "persisted **old-id →
new-id mapping table**… queryable during the transition… deletable after — its
deletion is an ordinary attributed command." Never stated: where it lives,
whether it is sealed, who may read it, how long it persists, whether it appears
in the command log, whether it is covered by the space's DEK. The custody
bullet ("Sealed fields seal on entry… Blob values upload through the blob
pipeline") does not mention it.

*Attack.* Old primary keys from real systems are not opaque. They are email
addresses, account numbers, national IDs, MRNs, Stripe customer ids, Firebase
uids. The mapping table is therefore a **plaintext join key between the
customer's external identity space and Ordo's internal ids**, sitting beside
sealed data whose whole purpose is that the operator cannot perform that join.
Consequences: (a) a DB-provider snoop or curious operator reads it and
re-identifies every sealed row's subject without decrypting anything — the
SH-1 attack, but complete and requiring no cryptanalysis; (b) `deletable after`
is opt-in and nothing forces it, so the default outcome is permanent retention;
(c) if it is not sealed under the space DEK, **crypto-erasure does not reach
it** — destroy the SMK and the mapping between the erased subject's external
identity and their orphaned ciphertext survives, which is exactly the
resurrection risk SH-10 closed for keys and leaves open for identifiers.

*Pin [proposed].* The mapping table is **an ordinary sealed entity in the space
it belongs to**, not engine metadata: old-id values are `sealed indexed`
(equality lookup is the only access pattern a redirect needs), it inherits the
space's DEK and dies with it, it is guarded like any entity, its reads land in
the key-release log, and it carries a **mandatory TTL defaulting to the import
session's expiry** with deletion as an automatic logged command rather than an
optional one. If a deployment wants it longer, that is a disclosed policy on the
space's consent page, because a re-identification table is consent-relevant.

*Must not break.* Redirects and the dual-write bridge — both are equality
lookups on the old id, which a blind index serves.

**SH2-6 — Revert of a re-run import deletes pre-existing rows.**

*Loop.* Two bullets, three lines apart. Idempotency: "a corrected mapping
re-runs and **updates**" via converge-or-create on the external-id key. Revert:
"genesis batches are **create-only** (converge on re-run), so the group inverse
is **delete-by-minted-ids** — `revert_group` works from the id set without
per-row images."

*Attack.* Not adversarial; a data-loss path. Import run 1 creates rows. Run 2
with a corrected mapping *converges* some of them — the batch group records
those ids in its minted-id set, but the rows are not new and there are no
before-images to restore. Reverting run 2 issues delete-by-minted-ids and
**destroys rows that predate it**, with no image to recover them from, because
the summarized-image design discarded them by construction. An operator
correcting a mapping and then rolling back loses data the rollback was supposed
to protect.

*Pin [proposed].* A batch group records **created ids and converged ids as
separate sets**, and the inverse is delete-for-created-only. Converged rows are
**not revertible from a genesis batch** — say so at revert time, loudly, and
require a `--accept-partial-revert` acknowledgment naming the count. Better
still: re-runs after the first are ordinary attributed writes with ordinary
images, not genesis batches; genesis mode is for the first pass only, which is
what "genesis" means.

*Must not break.* Bulk scale — a second corrective pass is orders of magnitude
smaller than the first.

### B4. Denial entries in the command log

**SH2-7 — A non-member can append attacker-chosen entries to any space's tamper-evident chain, and the rate-collapse key does not stop it.**

*Loop.* `dashboard-plan.md` §5b [settled]: "Every refusal at the door
(`access_denied`) and every row-level guard rejection (`access_guard`) appends
an actor-attributed entry to **the space's chain** carrying principal, op,
space, outcome, and instant." Bounded by: "denials are rate-collapsed per
(principal, op) within a short window."

Now read the router. `gate()` runs `bind_space` **before** `check_access`. So on
any registered op name, an attacker chooses the space via `X-Ordo-Space`,
fails the access check precisely *because* they have no membership there, and
the denial is attributed to a space they have no relationship with.

*Attack.*
1. **Cross-space chain injection.** Pick any op name (obtainable from the
   method-mismatch oracle, §SH2-13, or from any public SDK). Pick any space id.
   Fire. Each denial appends a hashed entry to a victim space's chain. The
   attacker is not a member; that is the *precondition*, not an obstacle.
2. **The collapse key omits space.** Collapse is per `(principal, op)`. One
   principal fires the same op at 10,000 different spaces and collapses into
   *one* bucket — which means either the entries are wrongly merged across
   spaces (and the record is false) or collapse does not apply per-space (and
   the bound does not bind). The document cannot mean both.
3. **Collapse is defeated by key diversity anyway.** Rotate the op name — every
   registered op is a fresh bucket. Rotate the principal — `AutoOnboard`
   (SH-31) mints one per federated login with no stated rate limit, and even
   under `InviteOnly` a delegation-holding agent is a distinct principal. N
   principals × M ops = N·M buckets per window.
4. **Ageing out breaks verification.** "They age out under the retention policy
   independently of command history." Denial entries "hash into the chain like
   any command." **You cannot remove a link from a hash chain and still verify
   it** — `verify_chain` recomputes `prev` linkage over the stored slice and
   fails at the first gap. So either denials never age out (and the DoS is
   permanent chain growth in a victim's space) or `verify(space)` breaks for
   every space that ever received one. Neither is stated.
5. **Anonymous denials have no principal.** No token at all still reaches
   `check_access` after `bind_space`, so an unauthenticated request produces a
   denial with no principal to key collapse on — one bucket for the whole
   internet, which both defeats collapse and lets one attacker mask another's
   probing.

The params exclusion ("never the attempted arguments") correctly closed
chosen-plaintext poisoning. It closed the wrong field: **the op name is
attacker-supplied** and the space is attacker-supplied, and both are in the
entry.

*Pin [proposed].* The instinct is right — "who was refused" belongs in a
tamper-evident record — but the command log is the wrong home, which is what
`security-logging-design.md` §6 [pinned] already says (§SH2-8).
- **Denials go to a separate append-only chained security-event stream, per
  space, with its own head and its own retention** — which is exactly the
  proposal §6 points at. A separate stream ages out without breaking the
  command chain's verification, and a space owner reads both.
- **Only denials by a principal with a members row in that space append to that
  space's stream.** A refusal of someone with no relationship to the space is a
  *deployment* event, not that space's history; it goes to the platform stream
  where the operator's abuse alerting already lives. This is the load-bearing
  rule: no outsider writes to a space's record.
- **Collapse key is `(space, principal, op)`** and collapse carries first
  instant, last instant, and count. Anonymous denials never enter a space
  stream.
- **Per-space, per-window entry cap** with overflow collapsing to one
  "N further denials suppressed" marker, so the record degrades before it grows.

*Must not break.* The property §5b was built for: "who was refused, when" stays
answerable with integrity, and the burst signal the operator wanted gets
*better*, because it is no longer diluted by every stranger on the internet.

**SH2-8 — Two [settled]/[pinned] rulings say opposite things about where denials go.**

*Loop.* `dashboard-plan.md` §5b [settled]: "a denial is a chain event."
`security-logging-design.md` §6 [pinned]: "**Denials land in the operational
log, not the command log** [pinned] — no state changed, and the command log
records what happened, not what was refused. **That ruling stands.**" §6 then
points at "`dashboard-plan.md` §2.1" for the open half — a section that does not
exist; the real content is §5b, which reaches the opposite conclusion.
`implementation.md` §Observability still describes the separate security-event
stream as the home.

*Attack.* Not an attack — an unresolved ruling in the audit substrate, where
"which document is current" decides whether an outsider can write to a chain.
Flagged because §SH2-7's severity depends entirely on which of these ships, and
because a dangling cross-reference is how a security ruling gets implemented
twice, differently.

*Pin [proposed].* Resolve in favour of §6 (separate chained stream), fix the
dangling reference, and mark the loser explicitly superseded rather than leaving
two live [settled] markers. `implementation.md` already describes the winner.

**SH2-9 — The highest-signal probing produces error codes that §5b does not chain.**

*Loop.* §5b chains `access_denied` and `access_guard`. But
`api-engine-design.md` §4.1 routes op-enumeration probing to `unknown_op` and
row-enumeration probing to `not_found` — neither of which is in §5b's set. So
the two probes non-enumerability was designed to make quiet are also the two the
denial record cannot see. Non-enumerability's stated trade is "quieter for the
attacker and *louder* in the metrics"; the metrics half only covers half the
probes.

*Pin [proposed].* The security-event stream records the **decision**, not the
wire code: `not_found`-because-guard is a distinct recorded outcome from
`not_found`-because-absent, and `access_denied`-for-unregistered-name from
`access_denied`-for-forbidden. The wire stays uniform; the record does not.
This is the whole point of separating the two surfaces.

### B5. The `routine:` principal

**SH2-10 — Routines are outsider-triggerable, whole-table, guard-exempt readers, and the sentence that says otherwise is contradicted two sections earlier in the same document.**

*Loop.* `definition-language.md` §8: "**A tier-1 routine runs as a
distinguished `routine:` principal that access guards evaluate as trusted
platform code — guards pass.**" Three bounds are offered in place of guards.
The second is: "**Its trigger is its second constraint.** A routine runs when
its clock or its listener says so… and **never on demand from an outsider**.
There is no path by which a caller invokes a routine to launder their own
visibility."

§7, the trigger vocabulary, in the same document:

| Kind | Fires on |
|---|---|
| `ingested` | a create attributed to a **`source:` principal** (webhook, sensor, mail slot) |
| `fed` | a write attributed to **`source:feed-*`** — an inter-space feed copy landing |

And §8 on feeds: "Copies land as ordinary writes on the receiver's chain
(`source:feed-<name>`), so **the receiver's own triggers and rules fire on
them**."

*Attack.* The claim conflates *not directly callable* with *not
outsider-triggerable*. They are different properties and only the first holds.

1. A **source principal** is by definition an outsider — "write-only outsiders
   (bank webhooks, camera hubs, email-ingest, sensors)… never readable access."
   It writes one row. `ingested` fires. A routine now runs with **guards
   passing** over every entity in its manifest — including rows the source has
   no read path to at all.
2. A **feed from another space** does the same via `fed`, and the document
   presents this as the feature ("cross-space reactivity becomes local
   reactivity").
3. A **guest** with one ACL row, or a **delegated agent under prompt
   injection**, makes one legitimate write and gets the same effect.

The laundering completes on the write side. The routine's manifest bounds
*which entities and fields*, and — this is the load-bearing gap — **has no row
dimension at all**. Guards are the only row-level bound in the system and
routines are exempt from them. So a routine reads whole entities and writes
derived rows: an alert, a rollup, a summary, an `auditFindings` row. If the
attacker holds an ACL row on any output row, or the output entity is
unguarded, or the output is a per-account rollup they legitimately see, then
attacker-controlled input has caused guard-exempt reading of data they may not
see, condensed into something they may. Every command is correctly authorized
and correctly attributed to `routine:<name>`; nothing is denied; no
denial-volume alert fires, because there was no denial.

Two aggravating details. `definition-language.md` §8 also grants agents
"registered ops (HTTP or MCP) **plus trigger requests**" — an outsider→trigger
surface that `api-engine-design.md` does not document at all. And the mechanism
is specified three incompatible ways across the corpus: "guards pass"
(definition-language), "guards do not apply at all" / "would evaluate false"
(glossary, traps), and "an **open spec ruling** — the engine's job is to make it
a typed, named failure" (engine-plan P3). `guards-conformance.md` has **zero**
routine cases and zero group cases, so nothing asserts any of the three.

*Pin [proposed].* Keep "routines are not callers" — the reasoning is sound and
the alternative (a system-user row) is worse. Bound the row dimension instead:
- **`$trigger` is a mandatory scope for listener-driven routines, not an
  affordance.** A listener routine's entity sources must be constrained by
  `$trigger.ids` (or a walk anchored to them); reading an entity unrestricted
  by the trigger requires the `unbounded` flag *and* a new
  `unguarded` acknowledgment at the routine header — the flags already exist and
  already sit in that slot. Clock routines, which get an empty id set, take the
  flags by construction and are therefore greppable.
- **Split the trust boundary by trigger provenance.** A routine reachable from
  `ingested` or `fed` — i.e. wakeable by an outsider — is a distinguished class:
  its manifest is rendered on the *source's* and the *feed's* consent pages,
  because consenting to a feed now means consenting to what it can wake.
- **Routine output re-enters through the ordinary guard path.** A routine writes
  as `routine:`; a *reader* of those rows is still guarded. Pin that a routine
  may not write into an entity whose read guard is weaker than the read guards
  of every entity it read — a compile-time check over the manifest, which is
  exactly the kind of derived check the design already does well. This is the
  one that actually closes the laundering.
- **Resolve the three mechanisms to one sentence** and add conformance cases;
  update `engine-plan.md` P3, which still records the ruling as open.

*Must not break.* Clock-driven rules over guarded domains — the failure mode
that motivated "guards pass" — keep working unchanged, because clock routines
declare their flags and the manifest still bounds them. What changes is that a
routine's reach becomes as visible as an op's.

### B6. Groups

**SH2-11 — Group membership confers ops and roles, and nothing constrains who may edit a group relative to what the group grants.**

*Loop.* Three settled statements, from three documents.
`definition-language.md` §8: "A group may be named anywhere a principal may —
**memberships**, profiles, acl rows, guard walks."
`security-logging-design.md` §6: the effective grant set is "the union of (role
allowlist) ∪ (**allowlist of each group they belong to** in this space) ∪
(per-member overrides)."
`definition-language.md` §3: "**Group membership changes are ordinary audited
commands** — the same runtime state as roles and profiles."

Nowhere is there a rule about *who* may run those commands relative to what the
group confers.

*Attack.* Straight privilege escalation, three ways.
1. **Via group→op allowlist.** Group `leads` is listed in the profile with
   `space.delete`. Alice is an editor who can call whatever op the domain
   registered for managing group membership. She adds herself to `leads` and
   inherits the allowlist. The access engine has no notion that this membership
   write is different from any other row write.
2. **Via group→membership role.** Groups are nameable in *memberships*, so a
   group can hold the `manager` role. Adding a member to it is a role grant
   wearing a data edit's clothes.
3. **Via guard resolution.** "`x == $caller` is satisfied when `x` is the
   caller's own members row **or any group row the caller belongs to**." So
   adding yourself to a group satisfies every guard anywhere in the space whose
   walk reaches that group — potentially thousands of rows across many entities,
   from one write.

The generalization: **group membership is grant-making, and grant-making is
being modeled as ordinary entity data.** That breaks commitment 3 — consent is
never in config — not by putting a grant in a file, but by putting it behind an
op whose own authority is unrelated to the authority it confers. The same hole
exists one layer up and predates groups: a manager who may edit the access
profile can add owner-tier ops to the manager role, because
`security-logging-design.md` pins that "roles have no implicit hierarchy" and
`AccessProfile::allows` is a flat lookup.

*Pin [proposed].* One rule, applied to all three surfaces:
- **No-escalation invariant.** A principal may modify a group's membership, a
  profile's role→op mapping, or a per-member override **only if their own
  effective grant set is a superset of the authority being conferred**. Adding
  someone to a group that grants `space.delete` requires holding
  `space.delete`. This is checkable in one query — the design already computes
  effective sets for `callersOf`, and this is that computation run against the
  writer.
- **Group-membership edits are platform commands with their own door**, not
  domain-authored ops over a members-shaped entity. There is already no
  domain-authored path to minting a members row; extend the same rule to
  changing one.
- **`group_nesting` is enforced on every write path**, including genesis import
  (§SH2-4) and the dual-write bridge. Today it is checked "at write time" in
  one document, appears in no error-code table, and has no conformance case.
- **`std.space.callersOf` must be asserted, not assumed**, to resolve groups —
  `guards-conformance.md` has no group cases at all, and this is the query the
  entire model's best property rests on.

*Must not break.* Groups as the answer to org hierarchy. Departments and
contractor cohorts are managed by people who already hold the authority they
are delegating — the invariant is a no-op in the intended case and only bites
the escalation case.

### B7. Crates — re-audit

**SH2-12 — The invalidation bus is keyed on op name alone: a cross-tenant write-timing channel on every live subscription.**

*Loop.* `MemoryBus` keys channels by op name; `InvalidationBus::subscribe(op_name)`
takes no space; `InvalidationEvent { op: String }` carries none.
`live_inner` subscribes with `state.bus.subscribe(&op.name)`.

*Attack.* A subscriber to `GET /op/listInvoices?live=1` in space A receives an
`invalidate` event **every time any principal in any space writes through that
op**. Op names are deployment-wide. So a legitimate one-space customer gets a
sub-second, real-time feed of every other tenant's write activity on every op
they share — which is a business-intelligence leak (competitor transaction
volume and rhythm), the SH-12 activity-profiling attack available to *tenants*
rather than only the operator, and a cross-space covert channel a compromised
agent can use to signal out of a space it is confined to. It is also an
amplification vector: one write wakes every subscriber in the deployment.
Compounding: SH-30 is still open, so a revoked principal keeps receiving the
feed until they disconnect.

*Pin [proposed].* Subscribe on `(space, op)`; `InvalidationEvent` carries the
space; the LISTEN/NOTIFY fan-out in `implementation.md` uses a per-space
channel. Then close SH-30 on the narrowed stream: re-check the door per signal
(it is a door check, not a query) and close streams on revocation.

*Must not break.* Statelessness and no-stickiness — a per-space channel is the
same mechanism with a wider key.

**SH2-13 — Unauthenticated op-surface enumeration through `method_mismatch` and `missing_space`.**

*Loop.* `router::dispatch` answers an unregistered name with `door_denial`
(403 `access_denied`) — correct, and `door_denial`'s doc comment reasons about
it well. But `gate()` then runs **method enforcement** and **`bind_space`**
*before* `check_access`, and the comment concedes it: "a caller who sends the
wrong method, or omits the space, can distinguish a registered op from an
unregistered one… Both are noisy, authenticated probes."

*Attack.* They are not authenticated. `check_access` is the only place a token
is examined, and it runs last. So with **no `Authorization` header at all**:
`GET /op/createInvoice` → 405 `method_mismatch` with an `Allow: POST` header if
the op exists, 403 `access_denied` if it does not. One request per name, no
credentials, no rate limit stated. The deployment's entire op surface — which
is to say its domain vocabulary, the thing SH-24 exists to protect — enumerates
to the open internet. `POST` with no space does the same via `missing_space`.

Three-way spec conflict makes this hard to test against: `definition-language.md`
G18 says door denial is `access_denied`, `api-engine-design.md` §4.1 says
`unknown_op` (404), the code says `access_denied` (403). And §4.1's table has no
row for method mismatch or missing space.

*Pin [proposed].* Move the access check to the front of `gate()` for
**authentication** (is this a valid token at all), keeping the space-scoped
**authorization** check after `bind_space` where it must be. An unauthenticated
request gets `door_denial` before method enforcement or space binding, so the
oracle needs a valid token — at which point the denial-volume alerting §6 relies
on can actually attribute it. Add `method_mismatch` and `missing_space` rows to
§4.1's table with their residual stated, and resolve the door-denial code to
one answer across all three documents.

*Must not break.* The pipeline order that SH-19 pinned: space still binds before
the authorization decision. This splits one check into two, it does not reorder
the one that matters.

**SH2-14 — Correlation ids are globally unique across tenants, and `find`/`revert_group` still scan every space.**

*Loop.* SH-14's crate-level pin was not applied. Additionally,
`InMemoryStore::append` calls `self.find(&group.correlation)` for duplicate
detection — and `find` flat-maps over **all** chains.

*Attack.* Beyond SH-14's unchanged residual (a guessed correlation id yields
another space's before/after images through `revert_group`): correlation
uniqueness is now a **cross-tenant namespace**. If any correlation id is
attacker-influenceable — an idempotency-derived id, an intent id, a
client-proposed group id in the client-first path — then writing it in space A
makes it permanently unusable in space B, i.e. a targeted write-denial across a
tenancy boundary. The Postgres implementation will inherit the shape as a global
unique index, which is also a scaling bottleneck on the hottest write path.

*Pin [proposed].* `find(&self, space, correlation)`; `revert_group(store, space,
correlation, principal, now)`; uniqueness is `(space, correlation)`. Revert
becomes a registered op carrying the door check plus the write guards of every
entity in the group. The `Store` trait makes space the primary partition, which
BYO-DB makes physical anyway.

**SH2-15 — The replay cache has no in-flight reservation, so concurrent retries double-execute.**

*Loop.* `run_call` does `lookup` → `execute` → `put`. Nothing occupies the key
between lookup and put.

*Attack.* Two concurrent requests with the same `Idempotency-Key` — the exact
shape of a client retry after a timeout, where the first request is still
running — both miss, both execute. For a `converge` this is benign; for a
non-idempotent authored op (transfer, charge, issue) it is a duplicate side
effect delivered under a header that promised exactly-once. `implementation.md`
pins that the replay record commits in the write's transaction, which fixes the
*durability* half and not the *concurrency* half.

*Pin [proposed].* `put` a `Pending` reservation at lookup time under the same
key; a lookup that finds `Pending` with a matching fingerprint answers
`retry_in_flight` (409, retryable, `Retry-After`), and a mismatched fingerprint
still answers `idempotency_mismatch`. Reservations carry a short lease so a
crashed node's key frees itself. In Postgres this is an insert-on-conflict on
the replay table, which the same-transaction pin already implies.

*Must not break.* The single-caller retry story — a sequential retry still
replays verbatim.

**SH2-16 — Export is step-up gated by a boolean that never expires, and its contents are unspecified.**

*Loop.* `definition-language.md` §"Exit ops" is six lines: export and deletion
are "owner-only, step-up gated, command-logged ops (export = total
disclosure)." The compliance pack adds "(JSON + files)". That is the entire
content specification. Meanwhile `TokenClaims.fresh_user_presence` is a `bool`
(SH-18, still open).

*Attack.* (a) Step-up is not fresh. A token minted with presence at any point
carries export authority for its whole life, so the ceremony gates the *login*,
not the *export*. `security-logging-design.md` says "a short freshness window"
and the crate cannot express one. (b) The bundle's contents are undefined on
every question that matters: are sealed values exported as plaintext (bypassing
the `SealedRef` redaction rule and the `std sealed` default that §2.4 just
established) or as ciphertext? Are wrapped keys included — without which a
ciphertext export is useless and with which the export *is* the space? Is the
command log included, and the permission map, which `implementation.md`
explicitly forbids mirroring to a device? (c) Delivery is entirely unspecified —
no signed link, no TTL, no size bound, no rate limit — while `share links` in the
same document get a full TTL/revocation/bearer treatment. A total-disclosure
artifact deserves at least what a share link gets.

*Pin [proposed].* `presence_at: Timestamp` checked against a per-op freshness
window at the door (SH-18), with export's window measured in minutes. Export
emits a **manifest of its own contents**, derived not declared, and the
confirmation dialog renders it: which entities, which sealed fields in the clear
and under whose key release, whether the chain is included. Delivery is a
one-time signed URL with a short TTL, delivery and download are separate logged
commands, and export is rate-limited per space per window with the limit stated.

**SH2-17 — Error messages enumerate the manifest and the parameter list, undoing SH-24 before it ships.**

*Loop.* `bind::bind_params`: "unknown parameter `{key}` — valid parameters:
{listing}" over every declared param. `refine::check_field`: "unknown field
`{field}` — output fields: {manifest}". Both are past the door but available to
any token that may call the op, including guests and minimal-scope capability
tokens.

*Attack.* SH-24's pin trims catalogs and descriptors so a guest sees only the
ops in their exposure list with manifests limited to those ops' outputs. A
single garbage refinement field returns the full output manifest anyway;
a garbage parameter returns the full parameter list. Descriptor trimming that
the error path undoes is not trimming. The vocabulary is the disclosure —
`substanceUseEpisodes.relapseDate` is a diagnosis whether it arrives from
`std.catalog()` or from a 400.

*Pin [proposed].* Enumerating alternatives is a good DX property and should be
kept where it is safe: name the offending key in `param` (already done), and
include the valid-alternatives listing **only when the caller's token may see
the descriptor**, which is the same predicate SH-24's trimming computes.
Otherwise the message names the offending key alone. One rule, one predicate,
both surfaces.

**SH2-18 — No constant-time comparison anywhere, including for a MAC.**

*Loop.* `Signer::verify` defaults to `self.sign(bytes) == signature` — a
byte-slice `==` on a MAC. `RequestFingerprint` derives `PartialEq` over
`[u8; 32]`. The round-1 review's proposed page-token MAC would land in the same
pattern. A repo-wide search finds no constant-time requirement for tokens,
idempotency keys, blind-index digests, `rev` values, or signatures.

*Attack.* Early-exit comparison on a MAC verification path is a forgery oracle
under repeated queries. Today the reachable surface is small (the stub signer
is not production), which is exactly why it is cheap to pin now and expensive
after `Signer` has three implementations and page tokens are MAC'd.

*Pin [proposed].* A `ct_eq` helper (`subtle::ConstantTimeEq`) used for every
comparison of a secret or an authenticator: signatures, MACs, fingerprints,
tokens, blind-index digests. State it as a crate-level rule so new code inherits
it, and add it to the conformance assertions next to §4.1's timing assertion —
which is currently the corpus's only timing statement and only claims "naturally
close in cost."

**SH2-19 — Login timing enumerates users, in the function whose doc comment promises it does not.**

*Loop.* `LocalPasswordProvider::complete`:

```rust
let stored = self.credentials.get(identifier).ok_or(AuthError::BadCredentials)?;
if !self.hasher.verify(password, stored) { return Err(AuthError::BadCredentials); }
```

The struct's doc says: "Anti-enumeration: `begin` issues a challenge whether or
not the identifier exists, and unknown-identifier and wrong-password both fail
as `BadCredentials`."

*Attack.* An unknown identifier returns before any hashing. A known identifier
runs argon2id in production — tens to hundreds of milliseconds. The response
*code* is identical and the response *time* is not, so the directory enumerates
over the network. Two smaller issues in the same file: `pending` in both
providers is an unbounded map with no expiry, so `begin` is an unauthenticated
memory-exhaustion primitive; and `Directory::create_principal` mints sequential
ids (`usr_00000001`) shared across kinds, so a principal id discloses join order
and total principal count — which lands in the command-log envelope forever
(SH-12).

*Pin [proposed].* Verify against a **fixed dummy hash** when the identifier is
unknown, so both paths do one KDF. Bound and expire `pending` (TTL plus a cap,
same treatment the replay cache just received). Mint principal ids from the
UUIDv7 the doc already promises, and per-kind rather than from one counter.

### B8. Key custody, revisited

**SH2-20 — Rotation is the maximum key-exposure window, and it is mandatory on every revocation.**

*Loop.* Rotation "mints `DEK_{n+1}`… old rows and blobs re-seal through the
same chunked, checkpointed conversion job." Re-sealing by definition requires
**both** `DEK_n` (to open) and `DEK_{n+1}` (to close). Rotation triggers include
"**membership revocation** (mandatory)". SH-11 is only partially closed: worker
releases are not scoped, not correlated, and not refusable.

*Attack.* The re-seal worker is the actor most likely to be compromised — it
processes untrusted content and runs long-lived batch jobs — and during a
rotation it holds the largest key set that exists for a space. Since revocation
mandates rotation, an attacker who can cause or predict a revocation can
schedule their attack for the window in which the maximum key material is
resident in worker memory. "Every departing employee opens a key-exposure
window" is an unfortunate property for a design whose revocation story is
otherwise strong.

Second: **the SMK never rotates.** Two-level keys bound a DEK compromise
beautifully and bound an SMK compromise not at all — an SMK opens every epoch,
past and future, and it is the thing the `KeyProvider` releases to workers.

*Pin [proposed].* Re-seal workers get **epoch-scoped, purpose-scoped,
correlation-bound release**: `release(space, epoch, purpose=reseal,
correlation)` yields only the two epochs the current chunk needs, refuses
without a live correlation to a running rotation job, and is rate- and
breadth-capped per worker — the SH-11 pins, applied to the job that most needs
them. Add an SMK rotation path (rewrap all DEKs under SMK', destroy SMK) as an
explicitly rarer, owner-initiated, step-up-gated op, so the top of the key
hierarchy has a recovery state too.

**SH2-21 — The blind-index re-index is itself the correlation oracle rotation was meant to close.**

*Loop.* "Blind-index keys derive per epoch… so a rotation re-salts every blind
index. **Equality lookups fan across live epochs until the re-index
completes.**"

*Attack.* During the fan-out window both digests for the same value are present
and queryable. A DB-provider snoop or curious operator observing the re-index
sees, per row, an old digest replaced by a new one — a **linkage pair**. Collect
them and the re-salting buys nothing: the operator maintains a running map from
epoch-1 digests through epoch-n digests for every row that existed before the
rotation. Rotation's forward secrecy for *values* holds; its re-salting of the
*index* does not survive being observed, and the observer is the party the
blind index exists to defend against.

*Pin [proposed].* Re-index writes are **not incremental in place**. Build the
epoch-`n+1` index column offline, then swap atomically and drop the old column,
so the observer sees two populated columns and one transition rather than N
correlated row updates. If an online build is unavoidable, randomize row order
and batch so the pairing is not observable by write order — and state in the
shape summary that a rotation's re-salt is **best-effort against an observer
with continuous DB access**, since that is the honest claim. Fold SH-1's missing
entity/field domain separation into the same derivation change, since both
require the same re-index.

**SH2-22 — `SealedRef` is a deterministic content hash: an unsalted equality index on every sealed field, opt-in to nothing.**

*Loop.* `ordo-commandlog::Value::SealedRef([u8; 32])` — "SHA-256 of the sealed
ciphertext this field's value lives in", confirmed in `security-logging-design.md`
§7. It is in the plaintext command image and is hashed into the chain.

*Attack.* The security of `SealedRef` as a redaction rests entirely on an
invariant no document states: that the ciphertext is unique per write. If the
sealing path is deterministic, or — far more likely — if the sealed payload
store is **content-addressed and therefore deduplicating**, then two rows with
the same plaintext produce the same `SealedRef`. That makes the command log an
**unsalted, unbounded, permanent equality index over every sealed field**,
including fields the author deliberately did not mark `indexed`. It defeats
SH-1's entire budget argument ("the choice to index is priced at declaration"),
because nobody chose this. It survives crypto-erasure, since the log envelope is
plaintext and the DEK's destruction does not touch it. And it is visible at
*every* custody level including client-first, where the envelope is the one
thing the server can read.

*Pin [proposed].* State the invariant and make it structural: sealed payloads
are sealed with a **fresh random nonce per write**, so `SealedRef` is unique per
command and carries no equality information. If deduplication is wanted for
blobs, it is a per-field opt-in with the equality disclosure priced at
declaration exactly as `indexed` is. Add a conformance case: sealing the same
plaintext twice must produce two different `SealedRef`s — the crate already has
a sealed-redaction test that checks the encoded bytes, so this is the same
test's mirror image.

### B9. Licensing and the federation surface

**SH2-23 — No foreign-engine protocol surface exists; the real finding is the one write path that behaves like one.**

*Loop and assessment.* The AGPL/Apache split creates a legal invitation to build
a second engine — deliberately, and the reasoning is good ("an encumbered
specification is not a specification"). The security question is whether a
foreign or modified engine can speak to a first-party deployment. **It cannot,
because no such surface is specified.** Inter-space sharing is
intra-deployment: guest binding exchanges deployment-wide principal ids, feeds
are "a standing routine" inside one engine, and the only cross-space bridge is a
tier-2 service principal holding capability tokens for both spaces. Token
introspection and federation endpoints are explicitly out of v1 — "this issuer
serves this deployment's clients only." The only inbound federation is
OIDC-as-relying-party, i.e. a foreign IdP, which is an ordinary and well-scoped
trust relationship. **The threat is nil today**, as suspected.

Three things worth writing down before it stops being nil.
1. **The sync bridge is the shape of the risk.** `migration.md` §5 specifies "a
   feed-shaped, one-way replicator" replaying an external system's changes as
   import batches — an external, long-lived, trigger-suppressing, summarized-image
   write path into a live space, with no stated authority gate (§SH2-4). It is
   not a foreign *engine*, but it is a foreign *writer* with import privileges,
   and it is the component a real federation feature would grow from.
2. **"Modified engine" is undefined**, while the engine ships pluggable
   `Signer`, `AuthProvider`, `PasswordHasher`, and `KeyProvider` traits. A
   deployment running a third-party `Signer` or `KeyProvider` is not obviously a
   "modification" under the licensing prose, and it is *entirely* a
   modification under the threat model — those two traits are the root of the
   chain-integrity and key-custody stories.
3. **The hosted deployment runs code months ahead of anything published**, by
   the document's own statement, with no version handshake or capability
   negotiation specified anywhere. That is fine while there is one engine per
   deployment and becomes a compatibility-downgrade surface the moment two
   engines speak.

*Pin [proposed].* Add one paragraph to `licensing.md` stating the security
boundary the spec already implies: **a deployment's engine talks only to
itself**; there is no inter-engine protocol, spaces do not federate across
deployments, and any future one is a designed feature with its own consent
model — not an emergent property of two people running the same code. Then pin
that **provider traits are part of the trusted computing base**: a deployment's
computed security summary names which `Signer`, `KeyProvider`, and
`AuthProvider` implementations are loaded, so "we run stock Ordo" is a checkable
claim rather than a stated one.

---

## Part C — Checklist

### P0 — before any data that matters lands

| # | Item |
|---|---|
| **SH2-3** | Sink SSRF: https-only, resolve-then-pin egress with an internal-address deny-list, no redirects, signed deliveries, egress identity with no instance credentials. **Currently a path from any customer to the deployment's key custody.** |
| **SH2-1** | `OpExecutor::execute` takes the space; executor asserts it matches the door's; `$space` unusable as an op parameter name. SH-19 is closed at the door and open at the executor. |
| **SH2-2** | `CapabilityGrant` gains a mandatory space; the capability path checks it first. **Every agent token is currently deployment-wide.** |
| **SH2-7 SH2-8** | Denials to a separate chained per-space security-event stream; only members' denials reach a space's stream; collapse key `(space, principal, op)`; per-window cap. Resolve the §5b / §6 contradiction before either ships. |
| **SH2-4** | Genesis import gets an owner-only step-up-gated door; the quiet flag becomes an engine-derived session with a maximum duration; quiet is per-import-actor; rule the source-principal guard question in the language spec. |
| **SH2-10** | `$trigger` as a mandatory scope for listener routines; outsider-wakeable routines as a distinguished, consent-visible class; routine output may not weaken read guards. Resolve the three conflicting statements of the mechanism. |
| **SH-1** | Blind-index derivation gains entity and field. Cannot be retrofitted without a re-index — and SH2-21 says the re-index needs redesigning anyway, so do both once. |
| **SH2-22** | Fresh nonce per sealed write, with a conformance case. Deduplicating sealed payloads turns the command log into an unsalted equality index over every sealed field. |

### P1 — before multi-tenant or third-party anything

| # | Item |
|---|---|
| **SH2-12** | Invalidation bus keyed `(space, op)`; then SH-30's per-signal re-check and stream close on revocation. |
| **SH2-11** | No-escalation invariant on group membership, profile edits, and overrides; group edits as platform commands; `group_nesting` on every write path; group cases in the conformance corpus. |
| **SH2-13** | Split authentication (front of the pipeline) from space-scoped authorization (after the space binds); add `method_mismatch`/`missing_space` to §4.1; resolve the three-way door-denial code conflict. |
| **SH2-5** | id-mapping table as a sealed, guarded, TTL'd entity in its space. |
| **SH2-14** | Space-scoped `find`/`revert_group`; `(space, correlation)` uniqueness; revert as a guarded registered op. |
| **SH2-20** | Epoch- and purpose-scoped, correlation-bound worker key release; an SMK rotation path. |
| **SH2-21** | Offline index build and atomic swap for re-salting; state the observer caveat. |
| **SH2-15** | In-flight reservation in the replay cache; `retry_in_flight`. |
| **SH2-16** | `presence_at` instead of a bool (SH-18); export content manifest; signed short-TTL delivery; rate limit. |
| **SH-6** | MAC'd, principal-bound page tokens — still entirely unimplemented, and now load-bearing for SH2-12's page-fill oracle. |
| **SH-5 docs** | Update G12 and G18 to match §4.1 and the code; the language spec still mandates the ruling the API spec overturned. |
| **SH-19 docs** | Put `$space`, `X-Ordo-Space`, `missing_space`, and the binding order into `api-engine-design.md` §2 and §4. The code is ahead of the spec here. |
| **SH-27** | SHA-256 over a canonical encoding, in code. The per-op field-set semantics are right; the primitive is still a 64-bit FNV fold. |
| **SH-11** | Correlation-refusal on key release; worker release coverage; a chained release log. |
| **SH-29** | `AccessProfile` gains group allowlists and per-member overrides — the docs settled it, the crate has neither. |

### P2 — before the ecosystem story

| # | Item |
|---|---|
| **SH2-6** | Separate created/converged id sets in genesis batches; loud partial-revert. |
| **SH2-9** | Record the decision, not the wire code, in the security-event stream. |
| **SH2-17** | Alternative-enumeration in error messages gated by the same predicate as descriptor trimming. |
| **SH2-18** | Constant-time comparison as a crate-level rule. |
| **SH2-19** | Dummy-hash verify on unknown identifiers; bounded challenge maps; UUIDv7 principal ids. |
| **SH2-23** | The "one deployment, one engine" boundary paragraph; provider traits named in the deployment security summary. |
| **SH-2 SH-3** | Feed and import re-indexing under the receiver's key; digest truncation. |
| **SH-23** | Break-glass notification, budgets, chain tagging, declinable pre-authorization — all four round-1 pins, none landed. |
| **SH-12 SH-13** | Envelope pseudonymization; asymmetric checkpoints and `chainProof`. The merge rule's "ordering is platform-authored" trade needs to reach the consent page, not just `implementation.md`. |
| **SH-15 SH-16 SH-21 SH-22 SH-24 SH-25 SH-26 SH-30** | Unchanged from round 1. |
| — | `policy keys` and `policy mirror` are referenced but absent from the policy grammar; `group_nesting` and `refinement_unbounded` are absent from error-code tables; `guards-conformance.md` has zero group and zero routine cases. |

---

## Part D — The one to argue about

**"Guards pass for the `routine:` principal."**

The reasoning that produced it is genuinely good, and the alternative it
rejects — a system-user members row granted everything — is worse. The failure
mode it avoids is real and nasty: clock-driven rules silently reading zero rows
forever. And the replacement bound, the derived scope manifest, is the best
mechanism in the design: computed, greppable, per-field, consent-rendered,
undriftable.

It is also the only place in Ordo where **row-level authorization is switched
off**, and the manifest — the thing offered in its place — has no row dimension.
It bounds *which entities and fields*, never *which rows*. So the substitution
is not like-for-like: guards answer "which rows", the manifest answers "which
columns", and the document swaps one for the other as though they were the same
kind of bound. Every tier-1 routine is therefore a whole-table reader over its
manifest, forever, by default, and there is no acknowledgment word for it —
`unguarded` is required only when *composing a callee*, explicitly not for a
routine's own direct entity sources.

That would be defensible for a purely clock-driven trust boundary. The
justification offered is exactly that: "a routine runs when its clock or its
listener says so… **never on demand from an outsider**." But §7 of the same
document defines `ingested` as "a create attributed to a `source:` principal"
and `fed` as "a write attributed to `source:feed-*`", and §8 says feed copies
land as ordinary writes so "**the receiver's own triggers and rules fire on
them**" — presented as the feature that makes feeds the blessed app-to-app
interface. A source principal is defined two paragraphs later as a write-only
outsider with "never readable access." So the design simultaneously holds that
routines cannot be triggered by outsiders and that outsider writes are a
first-class trigger kind. Both cannot be true, and the one that is false is the
one carrying the security argument.

The counter-proposal, to argue over rather than adopt: **the trigger's
provenance determines the routine's bound.** A clock-driven routine is the
domain acting on itself and keeps "guards pass" as written — that is the case
the ruling was built for and it is correct there. A routine wakeable by
`ingested` or `fed` is not the domain acting on itself; it is the domain acting
on an outsider's cue, and it should be **scoped to `$trigger.ids` by default**,
with anything wider requiring the `unguarded` flag in the slot that already
exists for it. The cost is real — a handful of legitimate routines that
aggregate across a whole entity in response to an event get one extra word in
their header — which is why this is an argument and not a finding. But it makes
the routine's reach as greppable as an op's, it puts the acknowledgment at the
point of the bypass exactly as `unguarded` already does everywhere else, and it
turns the sentence "there is no path by which a caller invokes a routine to
launder their own visibility" from an assertion into a property.

Everything else in this design is structural: guards inline, manifests derive,
scope expansion kills grants, consent pages compute, and `std sealed` — round
1's Part D argument, adopted — made sealed disclosure an authored act. This is
the last place a security boundary rests on a sentence rather than on a
mechanism, and the sentence is contradicted by its own document.
