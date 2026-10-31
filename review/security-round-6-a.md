# Security round 6 — scope A (custody, the door, guards, provisioning)

Adversarial hardening review. Read-only on the tree; one dev server was run
on 127.0.0.1:4101 over a throwaway cluster and killed. No secret is pasted
here. "Verified" means I saw it in the code or made the running binary do it;
"inferred" means I reasoned from the call graph without executing it.

## 1 · Executive summary

The worst thing found is a **cross-member read of guarded row content through
an inferred join bridge**: when an op lists two entities that the compiler
connects through a third, unlisted entity, that bridge entity's read guard is
never compiled into the query, so a caller reads facts out of rows they may
not see — and the op's own catalog manifest claims the bridge *is* guarded, so
the disclosure is invisible to anyone auditing from the published surface
(A-01, verified live). This is the same defect the recorded gap GC007 names
for dot-paths, but on a second code path (`SourceJoin`, not `Join`) that no
document lists, which is the direct answer to the scope question "does
anything else have GC007's shape": yes, and it is not recorded. Second, member
**suspension does not exist on the serving path** — the engine is built from
`members` reading only `$space, id, role`, the `status` column is dropped, and
nothing calls `AccessEngine::suspend`, so `deploying.md` §5's promise that
"suspension takes effect on the next request" is false and a manually
suspended member keeps full access (A-02, verified live). The single design
decision I would push back on hardest is **building the door's authorization
engine as a whole-table snapshot rebuilt only on deploy/member-announce**
(`engine_from_members`): it is why status is silent, why "membership re-read on
every request" (§5) is not what the code does, and why every future
per-row authorization state has to be squeezed through a columns-in, rebuild-
to-apply pipe. The custody layer, by contrast, is genuinely strong and should
not be disturbed by any fix here.

## 2 · Threat actors

- **Malicious tenant owner.** Holds owner role in their own space, a valid
  token, and the ability to author `.ordo` programs if self-hosting. Wants to
  read other spaces, or read rows inside their space that a guard hides from a
  role they do not hold. (Disclosed: an owner can reach anything in their own
  space by granting themselves acl, leaving a log record — Z-004.)
- **Malicious ordinary member.** Holds a token good for one space at a
  non-owner role. Wants rows guarded away from them: other members' private
  projects, sealed fields, membership graph.
- **Compromised worker/replica.** Runs deployment code, holds the master key
  in RAM, can read Postgres. Wants to widen blast radius or persist.
- **Curious/malicious operator with DB credentials.** Reads and writes
  Postgres directly, cannot read the master key if it is a sealed file whose
  passphrase lives in the orchestrator secret store. Wants plaintext of sealed
  fields, or to forge chain/attestation state.
- **Stolen sealed-master file.** Holds `master.key` but not the passphrase.
  Wants the master.
- **Stolen bearer token.** Holds one live token. Wants to keep using it past
  logout/suspension, or link their own authenticator to the victim principal.
- **Network adversary between client and door.** Sees or replays requests.
  Wants tokens, idempotency keys, oracles in error/timing.
- **Supply chain.** Controls a dependency or a KeyProvider implementation.
  Wants key material or a silent authorization bypass.

## 3 · Findings

### A-01 — Critical — an inferred join bridge drops the bridge entity's read guard

**Threat actor:** malicious ordinary member.

**Claim attacked.** `definition-language.md` §multi-source: *"Disclosed
everywhere: inferred bridges appear in the plan, the scope manifest (their
guards inline too)…"* and `guards-conformance.md` R-G6: *"every entity the
flattened plan reads contributes its read guard as a conjunctive semi-join."*
The op manifest reinforces it: for the probe op below, `row_scope.entities`
lists the bridge with `read: {kind: walk, source: own, walk: "owner ==
$caller"}`, i.e. it claims the guard is applied.

**Attack path (verified live on 127.0.0.1:4101).**
1. Author (or use) an op that lists two entities connected only through a
   third, guarded entity — e.g. `whoWhere = people[name] places[id]`, where
   `people` and `places` connect through `assignments` (which carries
   `access read owner == $caller`). `assignments` is the compiler-inserted
   bridge; it is not listed.
2. As owner `mem_dev`, create an assignment row linking `pers_1 -> plc_1`,
   owned by `mem_dev`.
3. As a different member `mem_bob` (not the owner of that assignment), call
   `GET /op/whoWhere`. Response includes `{"id":"plc_1","name":"Alice"}` —
   `mem_bob` learns that Alice is assigned to place `plc_1`, which is content
   of an `assignments` row `mem_bob`'s own guard forbids reading. `mem_dev`
   and `mem_bob` receive byte-identical results.

**Evidence.** In `crates/ordo-sema/src/lib.rs::bind_multi_source`, a bridge
node is materialized directly as a bare table with no guard:
`crates/ordo-sema/src/lib.rs:3269-3277` (`Node::Bridge(e) => … From::Table {
entity, alias }`). Terminals go through `self.source()` →
`entity_source` (line 1897), which inlines the read guard into
`q.filters` (1976-1984) and is then materialized as a `From::Sub` because it
has filters (the `plain` test at ~3283 requires `filters.is_empty()`), so
listed terminals *are* guarded. Bridges are not. The planner then renders the
`SourceJoin` with only the space predicate, never a guard:
`crates/ordo-plan/src/lib.rs:1241-1279`. The manifest's claim that the bridge
is guarded is produced by a separate walk over the scope set, so it and the
executed SQL disagree — a silent-wrong, not a loud-fail.

**Why worse than GC007.** GC007 (recorded) is the dot-path variant on
`q.joins` (`crates/ordo-sema/src/lib.rs:4737-4744`, rendered at
`crates/ordo-plan/src/lib.rs:1281-1300`), ruled 2026-09-26 to move the guard
into the join `ON`. The bridge variant here is on `q.source_joins`, is not in
`docs/corpus.md` or `corpus-rulings.md`, and the entity is one the *author
never wrote* (the compiler inserted it), so an author reading their own op
source cannot see that a guarded entity is even in the plan — only the
manifest can tell them, and the manifest lies.

**Proposed pin `[proposed]`.** Give `SourceJoin` a guard field exactly as the
GC007 fix gives `Join` one, and in `bind_multi_source` compile the bridge
entity's read guard (via the same `guard_cond` terminals use) into the join's
`ON` for a `Node::Bridge`. The ruled GC007 semantics apply unchanged: guard in
`ON`, the joined columns null out, the anchor row stays. Fold both variants
into one rule so a third join site cannot repeat it.

**What the fix must not break.** Outer-preserving multi-source semantics
(ruling-3 amendment 3): a bridge whose guard fails must null the far columns,
not drop the anchor row, or the "a guard decides what you read, not whether a
row exists" property the platform rests on is violated in the other direction.

### A-02 — Critical — member suspension is unenforced on the serving path

**Threat actor:** malicious ordinary member / operator response to compromise.

**Claim attacked.** `deploying.md` §5: *"Suspension takes effect on the next
request, not at the next token expiry. Suspending a compromised member is
immediate by construction."* And `access.rs:667` checks
`MembershipStatus::Suspended` before anything else.

**Attack path (verified live).** The engine the door actually uses is built by
`engine_from_members`, which reads only three columns —
`crates/ordo-host/src/lib.rs:1591-1592` (`MEMBERS_SQL = SELECT "$space", "id",
"role" …`) — and calls `set_membership(space, id, role)` with no status
(`crates/ordo-host/src/lib.rs:1681`), which always inserts
`MembershipStatus::Active` (`crates/ordo-identity/src/access.rs:470-474`). I
set `members.status = 'suspended'` for `mem_bob` directly in the database and
forced an engine rebuild; `GET /op/taskView` still returned rows. There is no
production caller of `AccessEngine::suspend` anywhere (`grep` for `.suspend(`
outside `/tests/` finds none), and `members` is platform-read-only to op code
(the compiler refuses `set members …` with `reserved_derived_name`, verified),
so there is also **no op that could suspend anyone.** The check at
`access.rs:667` is dead on the table-driven path: the status it reads is always
`Active`.

**Evidence.** `crates/ordo-host/src/lib.rs:1591-1592`, `:1669-1683`;
`crates/ordo-identity/src/access.rs:466-500`; live test above.

**Proposed pin `[proposed]`.** Add `"status"` to `MEMBERS_SQL` and, in
`engine_from_members`, call `engine.suspend(space, principal)` when the row's
status is suspended (after `set_membership`). That closes the table-to-engine
gap. Separately, the immediacy claim in §5 is still only as fast as the
member-announce rebuild (see A-06); until membership is re-read per request,
§5's "next request" wording should be corrected to "next engine rebuild", or
the doc is making a promise the architecture does not keep.

**What the fix must not break.** Reactivation must restore the exact prior
role/groups (Z-005), which the in-memory `suspend`/`reactivate` already
preserve; a table-driven rebuild naturally does too since role is re-read.

### A-03 — Major — GC007 confirmed live and currently in its pre-fix (full-disclosure) state

**Threat actor:** malicious ordinary member.

**Claim attacked.** Same R-G6 text as A-01, dot-path form.

**Attack path (verified live).** `taskView = tasks[title, project.name,
project.budget]`. `tasks` has `access read project.projectMembers(member ==
$caller)` in my probe I used `tasks` unguarded to isolate the dot-path;
`project` (`projects`) has `access read owner == $caller`. As `mem_bob`, a
member of no project, `GET /op/taskView` returned
`{"name":"Acquisition of Globex","budget":9000000,…}` — full project name and
budget for a project owned by `mem_dev`. The dot-path target guard is not
applied and the field is fully populated (not even nulled).

**Assessment against the record.** This is **not worse than recorded** in
kind — GC007 documents exactly this and rules the fix (guard in `ON`, field
nulls, row stays). It is worth recording that the tree is currently in the
*pre-fix* state where the field is fully disclosed rather than nulled, and
that the `taskView` manifest already shows `projects` under `via: "guard"`, so
the catalog again over-claims. Do not re-report GC007 as new; do bundle its
fix with A-01 since they are one defect on two IR nodes.

**Proposed pin `[proposed]`.** The already-ruled GC007 fix, extended to cover
A-01 in the same change.

**What the fix must not break.** As GC007 records: a blank field is the
intended visible outcome; do not turn the LEFT JOIN into an inner join.

### A-04 — Major — a suspended/removed member keeps serving until an engine rebuild; membership is not re-read per request

**Threat actor:** operator revoking a compromised member; malicious member.

**Claim attacked.** `deploying.md` §5: *"Membership is re-read by the door on
every request."* `auth.rs:386-390`: *"the door … re-reads membership on every
check."*

**Attack path (verified live).** I deleted `mem_bob`'s `members` row entirely.
`GET /op/taskView` **still returned rows** and kept doing so until I triggered
a program swap (any save), after which the same call became `access_denied`.
The engine is a snapshot: `IdentityAccess::check` reads
`self.engine` (an `Arc<AccessEngine>` built at boot/rebuild) and never touches
the database (`crates/ordo-host/src/lib.rs:1425-1541`). Membership changes
land only when `announce_members` fires a `pg_notify` and every replica
rebuilds (`crates/ordo-host/src/deploy.rs:972-980`,
`crates/ordo-host/src/runtime.rs:3178-3186`). A direct DB membership change (or
one made by a path that does not announce) is invisible until the next rebuild.

**Evidence.** Live test; `crates/ordo-host/src/lib.rs:1425-1541` (check reads
in-memory engine only); `:1612-1618` doc admits *"a membership change takes
effect at the next swap or the next restart rather than on the next request …
weaker than IdentityAccess's own doc reads."* So the code's own comment
contradicts §5 and `auth.rs`. Session revocation *is* re-read per request
(the deny list), but that is a different fact from membership.

**Proposed pin `[proposed]`.** Either (a) correct §5 and `auth.rs` to say
membership takes effect at the next engine rebuild and make suspension go
through a rebuild path that always announces, or (b) put the membership/status
read in the request path the way the session deny list already is. (a) is the
honest cheap fix; (b) is what the docs currently describe. Pick one; today the
doc and the code disagree.

**What the fix must not break.** The in-memory engine's per-request cost is a
design asset (no DB round trip per call). Option (b) must not reintroduce one;
a per-space in-memory membership map refreshed on the invalidation bus keeps
the cost while shrinking the window.

### A-05 — Minor — the deployment master key stretch is a single HMAC over a 16-char floor for `ORDO_KEY_PROVIDER=local`

**Threat actor:** stolen database backup + a guessable/short `ORDO_MASTER_KEY`.

**Claim attacked.** `master.rs:26-28`: *"A sixteen-character passphrase behind
one HMAC is a sixteen-character passphrase"* — stated as the reason the sealed
file exists, but `local` remains a first-class, non-dev provider.

**Attack path (inferred).** `MasterKeySource::Env::open` accepts any
`ORDO_MASTER_KEY` ≥ 16 bytes and runs it through
`derive_from_secret = hmac_sha256(b"ordo.master.v1", secret)` with no KDF
(`crates/ordo-crypto/src/master.rs:124-135, 163-165`). Every space SMK and DEK
in `$keycustody` is wrapped under that key and the wrapped rows are, by design,
in the database backup (`keystore.rs:11-20`). An adversary with a backup and a
16-char master runs an offline HMAC-then-unwrap guess loop at HMAC speed. The
sealed-file path (`sealed-master`) uses argon2id and is not affected; the
`local` path is the default a laptop and "small self-host" are told to use
(`custody.rs:696`, `master.rs:9-13`).

**Evidence.** `crates/ordo-crypto/src/master.rs:120-165`;
`crates/ordo-crypto/src/custody.rs:707-737`. Verified the running server logs
`custody test (UNSAFE …)` for the dev flag and refuses `test` without it
(`runtime.rs:568-576`) — the guard rails around `test` are good; the gap is
that `local` is a production-selectable provider whose whole security is one
HMAC over an operator-chosen string.

**Proposed pin `[proposed]`.** Keep `local` for laptops but (a) print a
startup posture line stating "offline-crackable master; move to
sealed-master before customer data" (the log already prints `local (env master
key)`, so add the consequence), and (b) consider requiring the same argon2id
stretch for `local` as for the file, or raising the floor well above 16. This
is a disclosed-tradeoff, not a bug; the finding is that the disclosure lives in
a source comment, not in front of the operator selecting it.

**What the fix must not break.** Self-host-is-first-class (custody must not
require a KMS); a KDF on `local` must stay a single boot-time cost, not per
seal.

### A-06 — Minor — `std.space.create` reachability of a fresh tenant depends on an un-transacted notify; a member added directly is never announced

**Threat actor:** none adversarial; correctness/authorization-freshness.

**Claim attacked.** `tenant_ops.rs:687-706`: a created tenant is reachable
"now, or at the next restart", reported honestly in the response.

**Assessment.** This is handled honestly for `std.space.create`. The residual
worth recording: `announce_members` is the *only* trigger that rebuilds the
engine for a membership change (`deploy.rs:972`), and it is called from
tenant creation but there is no membership-mutation op in the build (members
is read-only to op code, verified). So any future member add/remove/suspend
that writes `members` directly must remember to announce, or A-02/A-04's
window becomes permanent. Recorded as the shape to watch, tied to the A-04
fix, not a live exploit today.

### A-07 — Note — the executor re-parses caller values by splicing them into `.ordo` source text

**Threat actor:** malicious caller probing for injection.

**Claim attacked.** Implicit: parameters are data, not code.

**Attack path (attempted, did not break).** `FixtureExecutor::call_source`
(`crates/ordo-host/src/lib.rs:632-652`) renders each bound parameter back into
an op-call *source string* that is then re-parsed by the binder — string
values as `format!("\"{}\"", s.replace('"', ""))`. I could not escape the
literal: quotes are stripped so the string cannot be terminated, and a value
with a NUL or an unterminated quote surfaced as `internal` ("parse:
unterminated string"), not as injected code. So it fails closed today.

**Why it is still a Note.** This is a text-splice trust boundary reached on
every write call, defended only by one `replace('"', "")`. It depends on the
`.ordo` string grammar having no other terminator or escape that survives
quote-stripping (e.g. no backslash-escape that could consume the following
synthetic quote, no alternate string delimiter). That is a property of the
lexer, not of this function, and a future grammar change (raw strings,
alternate quotes, escape sequences) would silently open it. A parse failure
here is also mapped to `internal` (500), which is a mild oracle distinguishing
"my value broke the re-parse" from a clean rejection.

**Proposed pin `[proposed]`.** Bind parameters as values through the planner's
existing `$n` slot mechanism (which the read/write value paths already use —
`render_write_value`, `SPACE_SLOT`) rather than reconstructing source text,
so caller data never re-enters the parser. If the text seam must stay, add a
test asserting no lexer token can be produced from a stripped value.

**What the fix must not break.** Deterministic replay and the single-dialect
rule (`render_write_value`'s doc): values must render identically on read and
write.

## 4 · Prior findings re-examined

| ID | Disposition | Evidence |
|---|---|---|
| **SH-11** (KeyProvider compromise blast radius) | **Partly closed, partly open.** Blast radius is now *bounded per space*: keys never leave the provider (`custody.rs:262-311`), every `release` is metered and streamed to the space's own security log (`metrics.rs:1205-1245`), and destroy is per-space crypto-erasure (`custody.rs:561-596`). Still open: a compromised process holds the *deployment master* in RAM (`master.rs:69-74` states this plainly), so a worker compromise is still all-spaces-at-once for the life of the process. No per-space rate/breadth *alert threshold* on releases was found in scope. Open, narrowed. |
| **SH-18** (bearer tokens bind to no device) | **Still open, by design, and now disclosed.** The token carries an optional `client` claim (`token.rs:144`) but nothing binds it to a device/key; verification is pure HMAC over the payload (`token.rs:191-217`). Logout (deny list) and short expiry + refresh are the mitigations. A stolen token is fully usable until expiry or logout on this replica (and until the deny-list cadence on other replicas). Open. |
| **SH-22** (profile templates unbounded) | **Moot / not the current shape.** There is no per-space profile *template* input a caller supplies; the profile is built deployment-wide in `engine_from_members` from a fixed `build_specs` walk (`lib.rs:1685-1758`) and std ops are registry-present/exposure-opt-in (rule 3), so a viewer does not inherit the std surface. Per-member overrides exist in `AccessProfile.overrides` but are not populated from any caller-facing surface in scope. Moot for the reviewed build. |
| **SH-24** (catalog exposes entity/field names) | **Closed.** `catalog_for`/`interface_for` filter every manifest by the same `access.check` the door runs (`lib.rs:930-943`, `:945-995`), verified live: an anonymous or non-member token gets `[]`; a member sees only ops they can call and, transitively, only entities/fields those ops reach. Entity and field names are scoped to the asker. Closed. |
| **SH-29** (AccessProfile has no per-member overrides) | **Closed in the engine, unwired at the edge.** `AccessProfile` now has `overrides` with allow/deny per principal, honored in `check`/`callers_of` (`access.rs:724-741`, `:876-937`). No production surface populates them yet, but the finding was about their absence in the model, and that is resolved. Closed (mechanism); the surface is future work. |

## 5 · What is genuinely strong (do not break it)

- **The custody trait exposes operations, never keys** (`custody.rs:262-311`).
  Sealing, opening, blind-index and handle minting all happen inside the
  provider under a scoped, correlated `KeyLease`. This is the right shape and
  A-05's fix must not weaken it.
- **Epochs are wrapped, not derived** (`custody.rs:27-35`, `812-968`), so
  crypto-erasure is a real delete; `destroy_space` takes the SMK too because
  blind-index keys are SMK-derived (`custody.rs:571-596`). Verified the store
  does the SMK+DEK delete in one statement (`keystore.rs:458-478`).
- **The sealed-master file**: argon2id at the OWASP profile, 0600 at creation
  via `create_new` (no chmod window), profile not stored in the file,
  `key_id` bound as AAD, indistinct failure (`master.rs:194-343`). Solid.
- **Master-in-backup tripwire**: boot and `ordo keys init` refuse if
  `$keycustody` holds the master or its derived form
  (`runtime.rs:2653-2669`, `keys.rs:216-226`).
- **Token verification**: MAC checked before payload parse, constant-time
  compare, versioned positional payload that refuses a truncated/older width
  rather than reading it as "no session/no delegation"
  (`token.rs:188-217, 292-353, 378-383`). This closes the downgrade oracle
  cleanly.
- **One uniform door answer** for unknown-op / forbidden / bad-token, with the
  delegation gate placed *before* the registry lookup so `delegation_required`
  is not an op-existence oracle (`router.rs:421-444, 639-667`). Verified:
  unknown/forbidden/anonymous all return the same `access_denied`.
- **Space is bound then the access check is made against that bound space,
  and the executor re-verifies it equals the authorized space**
  (`router.rs:582-591, 621-628, 824-826`); `$space`/`$caller` are unshadowable
  reserved slots. Every physical source is space-predicated with a
  belt-and-braces `verify_scoped` refusal (`lib.rs:660-706`).
- **Idempotency keys are keyed on (space, principal, op, key) with a payload
  fingerprint** (`router.rs:861-877`), closing SH-17's "guess the key, get
  another principal's sealed response".
- **Link ceremony** requires fresh presence from an already-linked
  authenticator, unconstructable from a capability token
  (`directory.rs:204-259, 550-612`) — the takeover path is structurally shut.

## 6 · What I could not determine

- **Whether A-01/A-03 leak across *spaces*.** I verified cross-*member*
  disclosure within one space. A ref cannot cross spaces (composite FK) and
  every physical source is space-predicated (`lib.rs:693-704`), so I believe
  it is confined to one space, but I did not build a two-space program to
  prove a bridge cannot be pointed at another tenant's rows. To determine:
  a multi-space dev deployment and a bridge whose FK a malicious operator has
  tampered with.
- **The real-server executor path.** I exercised `FixtureExecutor` via
  `ordo dev`, which its own docs say is the same gate/engine a server binds
  (`dev.rs:1-12`). I did not stand up the full `ordo apply --host` server, so
  A-01/A-02/A-03/A-07 are proven on the dev-embedded path; confirming they are
  identical on the applied-program path would need a Postgres-backed
  `assemble` run, which I avoided to not collide with the other agent's
  cluster.
- **Replica-fleet suspension/logout timing.** Single-replica behavior is
  verified; the multi-replica deny-list cadence and member-announce fan-out I
  read but did not run against two live replicas.
- **Whether any planned membership-mutation op will remember to announce**
  (A-06). There is none today, so nothing to test; it is a forward hazard.
