# Ordo F7 — the client-first vault protocol: design & build plan

Companion to `engine-plan.md` (Track F), `implementation.md` (key custody,
space custody conversion, client-first mode, the merge rule, snapshots,
device keys), `security-logging-design.md` §7, `definition-language.md`
(the custody dial, `policy vault`), and `bootstrap.md` §2E. Same markers:
**[settled]** / **[lean]** / **[open]**.

Status: build plan. The protocol is largely settled prose; this turns it
into a build order and names where it is not settled.

---

## 0 · What this document is for

`engine-plan.md` gives F7 six bullets and a done-when clause — the right
density for a track list and the wrong one for the engineer who has to
build it, because **F7 is not a phase, it is an inversion of the trust
boundary every other phase assumed**. All of Track F so far has one shape:
the server holds keys, opens payloads, evaluates rules, writes the chain.
F7 keeps the ops, artifacts and manifests identical and moves all four
verbs somewhere else.

Three questions to answer before anyone writes code:

1. Which guarantees survive the inversion, which change status, which die.
2. What the wire records are, byte for byte, and how they relate to the
   chain `ordo-commandlog` already computes.
3. What order to build in so each step is testable against Postgres rather
   than against a promise.

---

## 1 · What client-first actually changes

### 1.0 Where the code stands today

Every claim below is a delta on this baseline:

- `ordo-crypto` — `KeyProvider` returns the SMK **to the server process**.
  `seal`/`open` are host-side. `BIK = HMAC(SMK, "blind-index:…")`,
  epoch-independent, as settled. One provider (`TestProvider`, fixed key);
  no KMS, no per-space SMK, no wrapped DEK series, no `Signer` riding it.
- `ordo-exec` — `read_as` plans, executes, calls `open_sealed`, which
  **hardcodes `TestProvider`**. No key release, no epoch selection, no
  correlation binding.
- `ordo-commandlog` — chain hashes `SHA-256(prev ‖ canonical(group))` over
  a decoded group. `SealedRef` is the corrected 16-byte handle. `Store` has
  one impl (`InMemoryStore`); `Signer` has one (`StubSigner`, a labeled
  fold, not a signature). `revert_group` inverts by **swapping recorded
  images**.
- Nothing in the workspace knows what custody is. `policy` is a lexer
  keyword with no parser behind it.

So F7 does not modify an existing vault implementation. It adds a second
execution model beside a first one that is itself half-built.

### 1.1 The guarantee ledger

| Guarantee | Server-custody today | Vault mode | Status |
|---|---|---|---|
| Row guards, read | Predicate inlined into every op's SQL; unseen rows never leave the database | Every enrolled device holds the space key and can decrypt any row. A read guard shapes what the app *shows* | **Dies as a boundary** [settled] |
| Row guards, write | Precondition after `SELECT … FOR UPDATE`; failure is a loud `access_guard` and the group rolls back | Evaluated in the fold against the canonical prefix; failure is a **canonical no-op with outcome `access_guard`**, identical on every device | **Survives, weaker** [settled] |
| `check`, bounds, referential integrity | Server-enforced, transactional | Fold-enforced, canonical no-op with recorded outcome | **Survives, weaker** [settled] |
| `rev` / CAS | Executor precondition | Fold precondition against the canonical prefix | **Survives, weaker** [settled] |
| Blind-index equality | `field$bidx` column, HMAC host-side after key release, equality resolves in SQL | No server row store, so no server SQL. On-device the store is plaintext, so equality is an ordinary predicate | **Label survives; mechanism runs nowhere** [settled] |
| `sealed indexed unique` | `UNIQUE (space, hmac)` — a referee | No referee. A duplicate is a fold-time no-op with outcome `duplicate` | **Detection, not prevention** [settled] |
| Converge-or-create | Compiler picks the mode; server converges onto the matching key | Fold must resolve the mode against the canonical prefix, or an offline `create` becomes a lost write | **[open] — Q6** |
| Command-log images | Resolved by the executor; the record **is** the truth | Authored and sealed by the writing device; the record is a **claim** | **Status change: record → claim** [settled] |
| `revert_group` | Inverts by swapping recorded images | Must recompute the before-image from the canonical prefix; a mismatch is signed evidence | **Different code path** [settled] |
| Chain / `verify(space)` | `SHA-256(prev ‖ canonical(group))` | Commits to device-signed `P` and server-signed `R`, over bytes the server never opened | **Survives; different link encoding** [settled] |
| Ordering | Server-assigned, server-authored | **Unchanged** — the explicitly-priced loss at the top of the dial | **Unchanged** [settled] |
| `whole` / subtree | One SELECT building nested JSON, **excluding sealed fields** | Runs on-device where the store is plaintext — but the artifact is identical at every dial position, so `whole` does **not** widen | **Survives; must not widen — Q2** |
| Envelope (names, ids, actor, kind, timestamps, correlation, position, volume) | Plaintext | Plaintext | **Unchanged at every position** [settled] |
| Page tokens | Server-minted, MAC'd | Paging happens inside the device; `$page` never crosses the wire for a domain read | Moves |
| Live subscriptions | Position-based invalidation | **Unchanged** — positions are envelope data | **Unchanged** |
| Idempotency | Server-side replay cache | The device's `counter` and `hash(P)` are the replay key; the segment endpoint is idempotent by receipt | Replaced |

### 1.2 The guard question, answered exactly

"Can the server still evaluate a guard over data it cannot read?" has a
sharper answer than it looks: **the situation never arises.** A guard
referencing a sealed field — directly or through a walk — is a compile
error (`guard_on_sealed`) at every position on the dial. Guards walk the
plaintext skeleton only, and skeleton, ids and `acl` rows are plaintext at
every position. A vault-mode server holding the row store could evaluate
every guard in the domain.

So guard evaluation does not leave the server because of readability. It
leaves for three separable reasons:

1. **There is no server row store.** The server's copy is ciphertext plus
   oplog segments. There is nothing to evaluate against.
2. **The guard is not a confidentiality boundary anyway** [settled]. Every
   enrolled device holds the space key. Spaces needing members who cannot
   read each other's rows do not belong at this stop on the dial.
3. **A second evaluator is a second source of divergence**, which F7
   forbids by name.

The honest line for the consent page, derived rather than written: *write
guards keep their meaning and lose their enforcement venue; read guards
lose their meaning entirely.*

### 1.3 What must NOT change, and must be tested for

The compiled artifact, op set, manifests, mode labels, cost classes and
scope hashes are **identical at every dial position**. This is F7's
premise and the property most likely to be broken by accident — every
"just special-case client-first here" in the compiler is a defect. The
test that catches it: compile the whole corpus twice, once per custody,
and assert byte-identical artifact JSON. It belongs in Phase 1, before
anything else is built.

---

## 2 · The oplog segment format and the chain

### 2.1 Which chain [settled]

**The same chain.** Not a parallel one, not a client log the server merely
stores. Per-space, server-sequenced: the server assigns each accepted
segment its position, hashing `prev ‖ segment-header` over ciphertext it
never opens. One chain per space, no forks, and `verify(space)` verifies
exactly it.

That settles topology and leaves encoding open, because links differ:

- **Managed link** — over a decoded `CommandGroup` with per-field images.
- **Vault link** — commits to device-signed `P` and server-signed `R`,
  over a payload the server never opened.

A converted space has both kinds in one chain, in order, switching at the
conversion group. `verify(space)` must handle that (Q1).

### 2.2 The two signed records [settled]

```
P = ( payload_digest = SHA-256(sealed payload bytes),
      device_id, counter, believed_prev,
      artifact_hash, schema_hash )      # device-signed, immutable forever

R = ( hash(P), canonical_position, canonical_prev )
                                        # server-signed, immutable forever
```

A device signs `P` exactly once, at authorship, and never re-signs — which
is what makes "a device killed between upload and acknowledgment leaves no
sequenced link lacking a valid device signature" true rather than hoped
for. `believed_prev` stays in `P` forever, so where the device thought it
was writing and where it landed are both on the record permanently, and
their disagreement **is** the offline-concurrency signal.

`artifact_hash` and `schema_hash` are load-bearing, not bookkeeping:
canonical byte order does not imply identical state unless the transition
function is part of the ordered input.

### 2.3 The snapshot record [settled, with one defect — Q7]

```
S = ( state_digest, at_position, chain_head_at_position,
      artifact_hash, schema_hash, device_id )   # device-signed
```

Sequenced exactly like a proposal and receipted, which is what makes a
snapshot *verifiable against the chain* rather than trusted.

### 2.4 What is not settled

The canonical byte encoding of `P`/`R`/`S`; the signature algorithm; what
a payload contains; what `counter` guarantees; and whether the sequencing
unit is the segment or the group. All five are §7 questions.

Recommendation covering all: **extend `chain.rs`'s existing frozen
vocabulary** — tagged values, big-endian u64 lengths, length-prefixed
UTF-8, `BTreeMap` order — rather than writing a second encoder. Two
canonical encodings in one chain is two chances to disagree.

---

## 3 · Key custody in vault mode

### 3.1 Where keys live

The device-held **space key is the SMK**. It wraps the DEK epoch series
exactly as the server-side SMK does, and `BIK = KDF(SMK, "blind-index",
entity, field)` derives from it exactly as settled. The two-level
structure is unchanged; only the holder moves.

One consequence F0 must be extended for: F0 already builds the startup
assertion that no key material reaches a database backup. F7 adds its
sibling — **assert that no vault-space SMK is ever presented to a
server-side `KeyProvider`**, and that the sequencing path has no
dependency edge to `ordo_crypto::open` at all. "Content-blind by
construction, not by discipline" means an architecture test, not a review
comment.

The server keeps its own `KeyProvider` in a vault deployment: it signs
checkpoints and receipts, MACs tokens, seals control-plane rows. It simply
has no path to a space key.

### 3.2 How a second device joins

Enrollment is an op — consented, step-up gated, command-logged — so the
chain shows every device that ever held keys, when, and by whom. "There is
no ambient device" [settled]. The cryptography is not settled (Q4).

### 3.3 Device loss [settled]

Two options, presented at conversion time, never silently:

- **User-held recovery phrase (default).** The platform holds nothing.
  Losing the last device *and* the phrase loses the space. That is the
  honest cost of the top dial position and belongs on the consent page in
  those words.
- **Platform escrow (opt-in).** A visible custody step **down**, disclosed
  as one, with the capability diff computed like every other conversion.

**No silent escrow, ever.** A recovery path the user did not choose is a
custody position they did not consent to.

### 3.4 Revocation and the SMK-derived BIK rule

The settled rule — BIK derives from the SMK, **not** the DEK epoch —
matters more here than in managed mode, for a reason the spec states for
managed custody and does not restate:

In managed custody a per-epoch BIK lets `sealed indexed unique` admit
duplicates across a rotation boundary. In vault mode there is **no
`UNIQUE (space, hmac)` constraint to fall back on** — the fold's
`duplicate` determination is the only uniqueness the space has. If digests
changed at every DEK rotation, and rotation is mandatory on every
membership revocation, then every revocation would silently redefine what
"the same unique key" means, on every device, mid-fold.

The cost, disclosed rather than discovered: **revocation does not remove a
departed member's equality oracle.** Rotation rolls the DEK so the removed
device cannot open new epochs, but it still holds the SMK, therefore every
BIK, therefore can still answer "did these two values match" over retained
ciphertext. Closing that needs an SMK rotation, which in vault mode has no
offline-build-and-atomic-swap venue (Q5).

---

## 4 · What the server can still do

### 4.1 Server-side, in a vault space

- **The door.** Authenticate, bind the space, resolve the principal, check
  membership/role/capability/step-up. All over the plaintext control
  plane; `ordo-identity` needs no vault awareness.
- **Sequencing.** Verify the device signature over `P`; verify enrollment
  at this position; verify `counter`; verify `artifact_hash` names a
  retained artifact; apply quota; append; sign and return `R`. Acceptance
  is not endorsement and decides nothing about whether the commands apply.
- **Storage and delivery.** Segments, snapshots, position ranges,
  ordering, durability. *When and how to send is client policy.*
- **The chain's operational surface.** `verify(space)`, checkpoints, the
  separate attestation cadence, `chainProof(from,to)`, archive, pruning.
- **`std.space.*`.** Members, profile, grants, groups. These **must** be
  server-served: mirroring them is forbidden — "a stolen device must not
  yield the permission map or the audit spine".
- **Deploy and catalog**, and **the envelope** (log-positioned reads, live
  invalidation, the security-event stream, deny-volume alerting).

### 4.2 Server-side, no longer

Query execution over values; host stages; routines over values; triggers
reading values; blind-index lookups; uniqueness enforcement; guard
enforcement; `check` enforcement; CAS; insert completeness; before-image
truthfulness; `revert_group` inversion; genesis `seed` import (refused
outright); windowed cross-space shares of sealed content.

### 4.3 Can a vault space serve any SQL-class read?

**No domain read, of any cost class, including `sql`.** The cost class is
a property of the *op*, not the venue, and is unchanged — but there is no
server-side row store for any class to execute against. An `sql`-class
read in a vault space has the same server venue as an `unbounded-host`
one: none.

What the server still serves is the **control plane**, which was never
sealed at any dial position. So: *a vault space serves zero domain reads
server-side, and its control-plane reads are exactly as readable to the
operator as in a managed space.*

### 4.4 Cost and venue

`CostClass` stays as it is — four variants, compile-time,
custody-independent, byte-stable in the artifact. F7 adds a **venue**,
resolved at bind time from the space's custody, and it is **not** a fifth
cost class:

```
venue := server | device
```

The catalog renders both, because an agent's catalog is its consent page
in machine form and "you may call this, but not from here" must not be
discovered at runtime. `read_as` gains a custody check ahead of the venue
gate; the SDK, having learned custody from the catalog, should never send
it. The server-side refusal is the backstop, not the mechanism.

Adding a class instead of a venue would make the artifact
custody-dependent and break F7's premise in the first week.

---

## 5 · Migration

### 5.1 What is settled

- **A space moves along the dial in place, and back** — a tracked,
  resumable migration job: chunked streaming re-encryption, progress
  checkpointed per chunk, verified by count plus content hash per entity,
  nothing deleted until the target copy verifies. Not export/reimport.
- **Reversal is the same mechanism backwards, with re-consent**, because
  leaving expands what the server can see.
- **A space is always created managed and converted** [proposed]: a blind
  server cannot validate its own genesis. Costs nothing real — the genesis
  group contains no sealed values.
- **`seed` is refused with `custody: client-first`**; import happens
  on-device through the embedded engine.
- **The capability loss is computed, not prose**, derived from op
  manifests the way scope manifests are.

### 5.2 Three costs that fall out of settled rules elsewhere

They belong on the consent page:

1. **Converting to vault gives forward blindness only.** Pre-conversion
   images are sealed under epochs whose keys the server still holds, and
   §7 forbids destroying an epoch key while any image within
   `undoHorizon` is tagged with it. "Your plaintext never touched our
   server" is **not true** of a converted space until that horizon
   passes — and the honest date is derivable, so state it.
2. **The chain's link encoding changes at the conversion group** (Q1).
3. **Converting out requires a key-holding device**, online and caught up.
   A space whose last device and recovery phrase are gone cannot convert
   out — it cannot do anything, which is the settled cost of the top of
   the dial arriving somewhere people will not expect it.

Also: exit replays the canonical order to build the tables, and every
proposal that folded to a no-op stays in history and produces no row. The
row count will not equal the command count, and the verifier must not
assume it does.

### 5.3 Schema migration under client-first custody — RULED (2026-09-05)

§5.1 and §5.2 are about migration **along the custody dial** — managed to
vault and back. They say nothing about the different question the owner
asked: *what happens when the developer changes the schema and the vault
is already client-first?* That gap was real and is now closed.

**The ruling, in the owner's own framing:** *"A schema change like that is
not content specific, it is schema specific. The schema on the vault end
is already known and changed by the dev, not the tenant — therefore if
migration is needed a migration script can be generated and passed to the
client."*

That is right, and it is the sentence that unblocks the whole question.
The reason vault migration looked impossible was the assumption that
migrating rows means reading them. It does not. **A schema migration is a
transformation authored against the *shape*, and the shape is public** —
the server knows every entity, field and type, because the developer
declared them and deploy identity versions them. Adding a field, widening
a type, renaming, splitting one field in two: none of these need to know
what any row says. So the server computes the transformation from the
schema diff and hands it over, without ever opening a payload.

Three constraints follow, each of which must hold or the mechanism is
unsafe rather than merely incomplete.

1. **The migration is expressed in Ordo's own closed expression grammar,
   not in arbitrary code.** This is the load-bearing one. A migration
   script is *code the server hands the client*, and the entire premise of
   a vault space is that the client does not trust the server. A
   compromised server could ship a "migration" that re-encrypts under an
   attacker's key or copies a sealed field into an open one, and the client
   would run it, because migrations are routine. The grammar closes this by
   construction: it has no I/O, no loops, and no way to name a destination,
   so **the client can read the migration and verify what it does before
   running it**. A transformation that cannot be expressed in that grammar
   is *refused*, not escalated to something more powerful — the same
   discipline that refuses a `check` the compiler cannot lower honestly.

2. **Uniqueness is the one content-dependent class, and a blind index
   already answers it.** Adding `unique (a, b)` is a claim about values,
   not shape, so it is the exception to the paragraph above. But a blind
   index is an HMAC over the sealed value, and the server can compare two
   HMACs for equality without reading either — so **equality-uniqueness is
   checkable server-side**, provided the fields carry blind indexes, which
   becomes a precondition on that migration class. Ordering constraints —
   a range, a `<` comparison — remain uncheckable under any custody the
   server has, and are refused rather than approximated.

3. **"Other clients must update" needs a schema floor, or it is a wish.**
   A client that never returns leaves rows on the old shape forever, and a
   client running old code against a new shape writes rows that are wrong
   in a way nothing notices. The space records the **minimum shape version
   it accepts**, and the server refuses writes from any client below it, by
   class, naming the version. **An old client then fails loudly instead of
   corrupting quietly** — the reason the deploy path refuses rather than
   degrades. Reads may stay permissive longer than writes; writes are what
   do damage.

### 5.4 Multi-device sync is a gap, named rather than assumed

The owner's reasoning included *"if the vault is theirs they already would
have it synced to their other devices"*. Checked, and it does not hold
today: §3.2 gives every enrolled device the space key, so every device
**can** decrypt — but nothing **pushes** a change to a device that is not
present. Enrollment is not sync.

This is recorded as its own item rather than folded into "migration",
because it is a distinct problem: migration transforms rows, sync gets a
key and a shape version to a device that is not here. Conflating them
would let the harder half hide inside the easier one.

---

## 6 · Build order

Each phase is independently testable against real Postgres and ends in a
done-when clause in `engine-plan.md`'s style.

### F7.pre — prerequisites, named so they are not discovered

F7 cannot start until these land:

- **F2's Postgres `Store`.** A vault protocol whose chain lives in a
  `BTreeMap` is a demo.
- **An asymmetric `Signer`.** `StubSigner` is a labeled fold. Receipts,
  checkpoints and attestations need Ed25519 behind the existing trait.
- **Checklist 6a–6f, P5's write rules.** The deterministic evaluator's
  outcome inventory **is** that rule set. Building the evaluator first
  means writing those rules for the first time in the client, which
  guarantees server and device disagree the moment the server catches up.
- **The embedded/WASM engine.**

Not counted below. Call it another 8 to 12 agent sessions.

### Phase 1 — the custody dimension exists

`policy vault` parses; the space registry carries custody plus conversion
state; the door binds it; the catalog renders `venue` per op; `read_as`
refuses a domain op in a vault space ahead of planning. No cryptography,
no devices, no segments — this phase exists so every later phase has
somewhere to hang.

*Done when:* a `client-first` space serves `std.space.members()` and
refuses `openTasks` with a typed venue error naming the custody; the
catalog labels every domain op `venue: device`; and **the whole corpus
compiles to byte-identical artifact JSON under both custodies**.

### Phase 2 — the two signed records, and a chain carrying both

Canonical encodings for `P`/`R`/`S` in the crate's frozen vocabulary. A
`Link` enum with a **hashed** kind tag. Ed25519 behind `Signer` plus a
`Verifier` half. `verify_chain_from` checks each vault link's `R` against
its `P` and predecessor, each managed link as now.

*Done when:* `verify(space)` names the first bad index for six seeded
tampers (swapped payload, re-paired receipt, forged receipt, truncated
chain, wrong `canonical_prev`, link with no device signature); a chain
that switches encoding mid-way verifies end to end and reports a lawful
switch as lawful; encoding goldens are byte-stable; and a verifier holding
only the public key can verify a slice with no store access.
*Needs:* Q1, Q3, Q8.

### Phase 3 — content-blind sequencing

The segment endpoint. Refusals on transport facts only.

*Done when:* a proposal whose payload is 4 KB of `/dev/urandom` sequences
and appears in `verify(space)`; a retired artifact is refused with its
deprecation date; a reused `counter` is refused **and raises a security
event** while a gapped one is accepted and recorded; a device killed after
upload re-fetches its receipt by `hash(P)` and finds exactly one sequenced
link, never two; and **an architecture test asserts the sequencing module
has no dependency edge to `ordo_crypto::open`**.
*Needs:* Q9.

### Phase 4 — the deterministic evaluator

The largest phase, and the one deciding whether F7 is a protocol or a
second product. The fold: given a canonical prefix and the artifacts each
proposal names, produce (state, per-command outcome). Every invariant
class resolves to a canonical no-op with a recorded outcome, identically
everywhere.

The governing constraint from F7 itself: *this is the same evaluator P4/P5
emit for the server; a second implementation is a second source of
divergence.* Which makes the device store's engine the central decision of
the feature (Q10).

Determinism obligations pinned here, not later: **no wall clock in the
fold, ever**; **a total order in every emitted `ORDER BY`** (a partial
order is implementation-defined, and two devices are two implementations
of the same planner's freedom); **session settings pinned** — datestyle,
timezone, `standard_conforming_strings`, and a deterministic collation;
and **ids are device-generated in vault mode**, which contradicts
`typed-id-keys.md`'s "ids are server-generated" and needs updating rather
than quietly diverging.

*Done when:* **the managed executor is the oracle** — the full write
corpus, executed once through `ordo-exec` against Postgres and once
through the fold over the equivalent canonical order, produces identical
final state and identical typed outcomes, case by case; two independent
fold runs over 200 canonical orders produce byte-identical state digests;
native and WASM builds agree byte for byte.
*Needs:* Q6, Q10.

### Phase 5 — artifact pinning and the deprecation window

Retained-artifact storage and fetch; verification against `artifact_hash`
before folding; refusal at sequencing for non-retained artifacts;
retirement as an **announced deprecation window**, never a silent swap.

*Done when:* a device offline across an artifact change folds to
**byte-identical state** with one online throughout; a retired artifact is
refused with the announcement date it was given; and an already-sequenced
proposal under a now-retired artifact still replays under it, forever.

### Phase 6 — devices: enrollment, wrapping, revocation

Device records in the chain; per-device signing and wrapping keys bound to
a passkey-PRF secret; wrap of the SMK to a joiner; a
short-authentication-string ceremony; un-enrolment; DEK epoch roll on
revocation as one attributed group; recovery phrase; escrow as an
explicit, disclosed step down the dial.

*Done when:* a second device joins, folds, and reaches byte-identical
state; a removed device's next proposal is refused and it cannot open the
next epoch, while segments it already holds stay open to it — and the
removal confirmation says so; the epoch roll is one group naming who
removed whom; **a re-wrap leaves every blind-index digest byte-identical**;
and a recovery phrase restores a space to a fresh device with no
platform-held material.
*Needs:* Q4, Q5.

### Phase 7 — snapshots, cold start, the discard floor

Snapshot production on the `policy vault` cadence; verification against
the chain head it claims; cold-start selection; the discard floor at the
newest snapshot every enrolled device acknowledged; enrolled devices as
**non-evictable** consumers; un-enrolment as the remedy; the dashboard
naming the device holding the floor.

*Done when:* a cold start over 100k commands with a snapshot every 5k
reads **one snapshot plus at most 5k segments — measured, not asserted**;
an abandoned device pins the floor and the dashboard names it *before* an
operator asks; and two snapshots claiming one position with disagreeing
state digests raise a divergence alarm rather than being deduplicated.
*Needs:* Q7 — **this phase cannot be built as specified.**

### Phase 8 — canonical-derived inverses

`revert_group` for a vault space recomputes the before-image from the
canonical prefix and treats the proposal's image as a claim. A mismatch is
signed evidence that a specific device supplied a false image — "not
silently preferred either way".

Note the code shape: today's `revert_group` swaps `before`/`after`. That
is correct for managed custody and wrong for vault custody, and the fix is
a second entry point taking a fold state, not a flag on the first.

*Done when:* a proposal carrying a **false before-image** is sequenced; if
it violates an invariant it applies nothing; if valid but false, reverting
it restores the true canonical predecessor and reports the mismatch naming
the signing device; and the finding is itself a signed, chained record.

### Phase 9 — conversion, both directions

The chunked, resumable, verify-before-delete job both ways; the computed
capability diff; the encoding switch; the three §5.2 disclosures.

*Done when:* a 100k-row space converts, is killed at 60%, resumes, and
verifies by count plus per-entity content hash; `verify(space)` passes
across the encoding switch; the capability diff is computed from op
manifests and matches the catalog's `venue` labels exactly, with zero
prose; conversion out requires re-consent and a caught-up key-holding
device; and the consent page states the forward-blindness-only cost with a
real date derived from `undoHorizon`.

---

## 7 · Open questions — decisions the spec has not settled

Each has a recommendation. Each is cheap to decide now and expensive after
a chain exists that anyone would be sad to invalidate — the same argument
F0 made about `SealedRef`, applying again.

**Q1 — How does one chain carry two link encodings?** (Phases 2, 9)
*Recommend:* a **link-kind tag byte inside the hashed bytes** — `0x00`
managed group, `0x01` vault proposal, `0x02` snapshot. Hashing it means a
verifier cannot be induced to re-interpret a vault link as a managed
group. Encodings that happen to be distinguishable today are a parsing
accident, not a guarantee.

**Q2 — Does `whole` widen in vault mode?** (Phases 1, 4)
*Recommend:* **no.** The sealed exclusion is a shape decision baked into
the artifact, and F7's premise is that the artifact is identical at every
stop. A custody-dependent `whole` would make output shape, runtime shape
verification and SDK types vary by dial position. If sealed fields in
`whole` are wanted, that is a language change for every custody.

**Q3 — Canonical encoding and signature algorithm.** (Phase 2)
*Recommend:* extend `chain.rs`'s frozen vocabulary — big-endian u64
lengths, length-prefixed UTF-8, fixed-width byte fields, no JSON, no
serde. **Ed25519** for device signatures and receipts, matching the
asymmetric-checkpoint ruling so verification needs only a public key and
one published verifier covers all three record kinds.

**Q4 — Enrollment cryptography and joiner authentication.** (Phase 6)
The spec fills in the governance and not the mechanism.
*Recommend:* per-device Ed25519 signing key + X25519 wrapping key, both
derived from a passkey-PRF secret so they bind to the authenticator rather
than to disk; **HPKE** to wrap the SMK; a short authentication string over
the joiner's public keys shown on both devices and confirmed by the human.
One standard construction, no bespoke handshake — and the same
safety-number pattern the logging design already invokes by name.

**Q5 — Is SMK rotation available in vault mode?** (Phase 6)
Rotation is required of every provider, and SMK rotation is specified as
an offline swap behind a write barrier. In vault mode there is no
server-side digest column to swap and nowhere to put the barrier.
*Recommend:* **v1 does not offer SMK rotation in a vault space**, and says
so where it is decided. Disclose at revocation rather than implying: a
removed member keeps a working equality oracle over blind-indexed fields
for retained ciphertext. Remedies are un-enrolment plus DEK roll (bounds
the future) or a full re-key via conversion out and back (bounds the
past). Shipping a best-effort online re-index and calling it rotation
would be the mis-selling the implementation doc warns about twice.

**Q6 — Is a write's mode resolved at authorship or in the fold?** (Ph. 4)
*Recommend:* **resolve in the fold.** The command carries the *intent* and
the fold picks the branch against the canonical prefix. Resolving at
authorship reproduces exactly the webhook-redelivery failure the
SMK-derived BIK rule exists to prevent: the device creates because it had
not seen the other row, the fold rejects it as `duplicate`, and a
legitimate write is lost with no conflict anyone can act on. This affects
the command encoding, so decide it in Phase 2, not Phase 4.

**Q7 — `S.state_digest` cannot detect divergence as specified.** (Ph. 7)
**A defect in the spec, not in a plan.** `state_digest` is defined over
the *sealed* snapshot bytes, and sealing uses a fresh random nonce per
write (`ordo-crypto::seal` already does). So two devices that folded to
**identical** state produce **different** digests. As written the field
cannot compare folds, cannot detect a lying device, and cannot support the
byte-identical-state claim F7's own done-when makes.
*Recommend:* `S` carries **two** digests — `payload_digest` over the
sealed bytes (storage integrity, matching `P`) and `state_digest` over a
**canonical plaintext serialization** of the folded state (comparable
across devices). Disclose that a whole-space state digest sits in the
plaintext envelope: it is a coarse commitment to an unguessable object,
not a per-field oracle, and it is what buys the free property in Phase 7 —
two snapshots at one position with disagreeing digests are provable
divergence or a lying device.

**Q8 — Is the sequencing unit the segment or the command group?** (Ph. 2)
*Recommend:* **one segment carries exactly one command group**, so
positions stay group positions and every position-consuming mechanism —
checkpoints, pruning floors, `undoHorizon`, log-positioned reads,
`std.E.history`, revert-by-group — works unchanged. Batching many
proposals into one upload is a transport concern. The alternative re-types
every position in the platform to buy a batching optimization the
transport layer already provides.

**Q9 — What does `counter` guarantee?** (Phase 3)
*Recommend:* strictly monotonic per device, per space. A **gap** is
accepted and recorded — a device may abandon a draft, and refusing gaps
would wedge it forever. A **reuse** is refused and raises a security
event: two different proposals under one `(device, counter)` is either a
compromised key or a cloned store, and both are what the security stream
exists for.

**Q10 — What engine backs the device store?** (Phase 4 — the largest)
Unspecified anywhere. The spec gives the device store its security rules
and never says what it is.
*Recommend:* **Postgres compiled to WASM** (PGlite or equivalent),
persisted to OPFS/IndexedDB, sealed at rest under the device key. The
reason is F7's own rule about second implementations: the platform's
entire correctness apparatus — emitted SQL, golden DDL, runtime shape
verification against an empty shadow schema, `whole`'s `json_build_object`
projection, keyset pagination — is Postgres-shaped. Retargeting to SQLite
is a second dialect, which is a second evaluator wearing a different hat,
and it puts dialect drift on the critical path of the byte-identical-state
claim. The cost is a multi-megabyte client payload and a slower cold
start, both real; the alternative cost is a divergence class no test suite
can enumerate. If the payload proves intolerable, the fallback is **native
devices only in v1** — a scope cut, not a correctness compromise.

**Q11 — Does the vault server keep a blind-index digest table?** (Ph. 1, 4)
*Recommend:* **no server-side digest index in v1.** A server-held digest
table over a space the server otherwise cannot read is a permanent,
unsalted equality index **in the plaintext envelope** — it survives
crypto-erasure, because destroying the key does not touch the envelope,
and it is readable to the operator at exactly the dial position purchased
to prevent that. Same argument that killed the content-hash `SealedRef`,
with more force rather than less.

**Q12 — Do the plaintext control-plane rows stay in Postgres?** (Phase 1)
*Recommend:* **yes**, and say so on the consent page. They are enumerated
as plaintext at every dial position; `std.space.*` must never be mirrored
to a device; and the door needs them to answer at all. This is not a leak
being introduced — it is the leak already disclosed, given a location.

---

## 8 · Scope

Agent sessions (AGENTS.md, "Sizing work"), prerequisites landed, excluding client SDK surface, UI, and
the consent-page work each phase implies.

| Phase | Scope | Why that size |
|---|---|---|
| 1 · Custody dimension | **1–2 sessions** | Registry field, policy parsing, catalog rendering, one gate. The dual-compile golden is most of the value and an afternoon of the work |
| 2 · Signed records + chain | **2–3 sessions** | Encoding is mechanical in an existing vocabulary; Ed25519 is a dependency; the mixed-encoding verifier and its tamper corpus are the real work |
| 3 · Content-blind sequencing | **2 sessions** | One endpoint over machinery F2 built. The architecture test is worth a day |
| 4 · Deterministic evaluator | **6–10 sessions, and an unknown number of tries** | The whole phase. WASM engine integration, determinism pinning, the parity corpus against the managed executor. Doubles if Q10 goes to a second dialect |
| 5 · Artifact pinning | **2–3 sessions** | Depends on F6's registry; the deprecation announcement surface is new |
| 6 · Devices | **4–6 sessions** | WebAuthn PRF, HPKE, the SAS ceremony, recovery phrase, escrow disclosure, revocation as a logged group. Three touch UI this excludes |
| 7 · Snapshots and compaction | **3–4 sessions** | Production and verification are small; the floor calculation with non-evictable devices, the dashboard, and the *measured* cold-start number are not |
| 8 · Canonical inverses | **2–3 sessions** | Second `revert_group` entry point plus mismatch-as-evidence |
| 9 · Conversion both ways | **4–5 sessions** | Resumable chunked job, per-entity verification, computed capability diff, encoding switch, three disclosures |
| **Total** | **26–36 sessions** | About 2 to 4 weeks of wall time with 2 agents at a time, before SDK and UI. Phase 4's parity corpus sets the real pace |
| Prerequisites (not counted) | **8–12 sessions** | Postgres `Store`, Ed25519 `Signer`, checklist 6a–6f, embedded engine |

The estimate is honest about one thing in particular: **Phase 4 is more
than a third of the work and carries all the schedule risk.** Everything
before it is plumbing over settled prose; everything after is finite. If
Phase 4's parity corpus does not go green, the rest of F7 is not worth
building, because "byte-identical state on every device" is the only claim
that makes the protocol worth its cost.

---

## 9 · What F7 does not include

Stated as a list so nobody has to infer it from silence:

- **No CRDT, no automatic merge, no reconciliation layer.** Conflicts
  resolve by rejection and replay. The seam for a later resolution layer
  is defined and v1 ships without it on purpose.
- **No device-side proof of ordering.** Ordering is platform-authored at
  every dial position including this one. The platform cannot read, forge
  or substitute content; it can in principle drop, delay or reorder
  proposals. The mitigation is attestation — **detection, not
  prevention**, and the custody page uses that word.
- **No guard enforcement, no `check` enforcement, no server refereeing.**
  Members are mutually trusting, cryptographically.
- **No per-member confidentiality within a space.**
- **No server-side query execution, routines over values, host stages, or
  blind-index lookups.**
- **No `seed` / genesis import into a vault space.**
- **No native client-first genesis in v1.** Create managed, convert.
- **No SMK rotation in vault mode in v1** (Q5), and therefore no removal
  of a departed member's equality oracle short of a full re-key.
- **No remote wipe, and no recovery of data a removed device already
  holds.** The removal confirmation says so rather than implying
  otherwise.
- **No pseudonymous actors and no coarsened timestamps.** The envelope
  stays plaintext, and the command log stays a high-resolution behavioral
  record even here. That is a dial position that does not exist yet.
- **No sophisticated sync scheduling.** The platform ships a default;
  batching, radio and power windows are client problems, deliberately.
- ~~**No sealed-blob range reads.** Chunked STREAM is still absent from
  `ordo-crypto`; blobs in a vault space are whole-object `private`.~~
  **Lifted.** `ordo_crypto::stream` landed with `plan-blob.md` B3: the
  STREAM construction, a fixed 64 KiB plaintext chunk, and `plan_range`,
  which resolves a plaintext range to the stored bytes to fetch. A vault
  space's blobs can be range-read; what Phase 6 still owes is the
  client-side half — a `private` object is sealed under a passkey-PRF key
  the platform never holds, so the *client* runs the same construction,
  and the platform serves the ciphertext range it is asked for.
- **No cross-space device store.** Six spaces, six unlockable stores.
- **No windowed cross-space shares of sealed content.** The offer is a
  copy, computed on-device.
