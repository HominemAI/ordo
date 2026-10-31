# Ordo blob & content custody — design & build plan

Companion to `definition-language.md` §2 (Content types, Blob storage
pipeline, Share links — all `[settled]`), `implementation.md` ("Server
stack", "Keys are two-level"), `migration.md` §6 (exit ops), `plan-export.md`
(the manifest gap this closes), `f7-vault-plan.md` (what `blob private`
waits on), and `audit-board-coverage.md` row 7.

Markers: **[settled]** restated from a settled document · **[proposed]**
adopted here, pending owner review · **[open]** named and not answered.

Status: **built** — B1–B5 landed (checklist 17g). §0 and §1 below are
kept in the present tense they were written in, because they are the
*starting state* this plan was reasoned from and rewriting them would
destroy the record of what was true when each decision was made. Read them
as of before the build; §7's increments and §8's ruling request are the
current text.

Every claim about the code carried its file and line when written, and §8
was re-verified against the tree after the build.

---

## 0 · The problem, stated precisely

### 0.1 Ordo cannot hold a file

`ScalarTy` (`crates/ordo-syntax/src/decl.rs:46`) is
`Text | Int | Money | Decimal | Bool | Date | Datetime | Duration(_) | Id`.
The keyword table that admits a type
(`ordo_syntax::declparse::scalar_ty`, `crates/ordo-syntax/src/declparse.rs:824`)
ends with a closed-set rejection naming eight words. There is no `blob`, no
`uri`, no `inline`. Grep across `crates/` finds **zero** occurrences of an
object-storage dependency, an HTTP client, or a TLS stack: the complete
third-party dependency inventory of the workspace is `argon2, async-trait,
axum, base64, chacha20poly1305, ed25519-dalek, futures, getrandom, p256,
serde, serde_json, sha2, tokio, tokio-postgres, tower,
unicode-normalization, wasm-bindgen`. Postgres is dialled `NoTls`
(`crates/ordo-exec/src/lib.rs:266`).

So an application that needs to hold a file today cannot express it. That
is the whole of finding 7 on the coverage audit, and it is the shape of the
hole: not a partial implementation to finish, a language member and a
storage tier that do not exist.

### 0.2 Two other things are already waiting on it

**The export declares the gap machine-readably.** `ordo-exec/src/export.rs`
ships an `unbuilt` list in every bundle manifest (`export.rs:982`):

> `"what": "blob bytes"`, `"why": "blob custody is unbuilt — there is no
> blob field type, no object-store resolver and no inline payload — so this
> bundle omits the per-entity file layout `migration.md` §6 describes.
> Nothing was silently dropped: there are no blob fields in this schema to
> drop."`

It has a test (`crates/ordo-exec/tests/export.rs:496` asserts
`m["unbuilt"][0]["what"] == "blob bytes"`). **That assertion is this
plan's closing condition**: when B5 lands, the assertion is rewritten, not
deleted, and the rewrite is the proof.

**Generated UI wants file fields.** The interface descriptor pins the
format-hint vocabulary *to the type inventory*, deliberately, rather than
opening a second metadata channel (`crates/ordo-plan/src/interface.rs:194`,
`clients/ordo-react/src/descriptor.ts:30`). A renderer cannot offer a file
input until the inventory has a member that means "file".

### 0.3 What this document is not

It is not `f7-vault-plan.md`. `blob private` — client-sealed under a
passkey-PRF key — needs the PRF, the device wrapping and the SAS ceremony
that F7 Phase 6 delivers (`f7-vault-plan.md:524`). This plan builds `blob`
(space-sealed, platform-held key), `uri` (no custody), and `blob inline`
(bytes in the sealed row payload), and hands `private` to F7 with the seam
in place. F7 already names the dependency in the other direction: *"No
sealed-blob range reads. Chunked STREAM is still absent from `ordo-crypto`;
blobs in a vault space are whole-object `private`"* (`f7-vault-plan.md:767`).
B3 below lands chunked STREAM, so that exclusion lifts.

---

## 1 · The starting state, verified

Every increment in §7 is a delta on these facts. They were read, not
assumed.

**The sealer.** `ordo_crypto::seal_with` (`crates/ordo-crypto/src/lib.rs:84`)
produces `[epoch][24-byte fresh random nonce][ciphertext||tag]` under
XChaCha20-Poly1305, AAD-bound to `(entity, field)` so a payload moved to
another column refuses to open. It is **single-shot**: it takes a
`&[u8]` and returns a `Vec<u8>`. There is no streaming form, and the crate
says so (`crates/ordo-crypto/src/lib.rs:13`): *"Chunked STREAM for blobs is
still absent — range reads over sealed blobs stay future work."*

**Custody.** `KeyProvider` (`crates/ordo-crypto/src/custody.rs:207`) is the
one seam; it never returns key material. `release(space, purpose,
correlation) -> KeyLease`; `Purpose` is `Read | Write | Reseal`
(`custody.rs:135`). `Epoch` is a `u8` and leads every envelope
(`custody.rs:113`). `rotate` mints the next epoch and does **not** re-seal
(`custody.rs:350`); `destroy_epoch` is the narrow crypto-erasure primitive
and *"callers must not reach this while any command image inside the
space's undo horizon names the epoch"* (`custody.rs:360`); `destroy_space`
destroys every DEK **and the SMK**, because a surviving SMK leaves the
blind indexes answering *was the value X* (`custody.rs:376`).

**The handle precedent, and it is a corrected ruling.**
`KeyProvider::mint_sealed_ref` (`custody.rs:322`) mints the opaque 128-bit
handle a command image carries in place of a sealed value:

```text
seed   = 32 fresh CSPRNG bytes, drawn per call and discarded
salt   = per-space key material, labelled ("$sealed", "handle.v1:<epoch>")
handle = HMAC(salt, seed)[..16]
```

`ordo_commandlog::Value`'s doc (`crates/ordo-commandlog/src/value.rs:9`)
records that it *was once specified as SHA-256 of the ciphertext*, and why
that was withdrawn: it makes the plaintext command log *"an unsalted
permanent equality index over every sealed field … and **survives
crypto-erasure**, because destroying the key does not destroy the
digests."* §2 below is that ruling applied to blobs.

**The sealed payload store — the direct model for a reference table.**
`crates/ordo-crypto/src/sealed.rs:76`:

```sql
CREATE TABLE "$sealedpayload" (
  "$space" text, "ref" bytea, "epoch" smallint,
  "entity" text, "field" text, "correlation" text, "payload" bytea,
  PRIMARY KEY ("$space", "ref") )
```

Three rules pinned there: **never an upsert** (`sealed.rs:154`), **never
deduplicating** — *"a deduplicating store reintroduces exactly that oracle
through the back door"* (`sealed.rs:24`), **no index on `payload`**, asserted
by a test that reads the DDL (`sealed.rs:266`). The space predicate is what
makes a handle un-transferable between tenants (`sealed.rs:185`). It is
registered at boot in `Pool::ensure_platform_tables`
(`crates/ordo-exec/src/lib.rs:296`).

**Its garbage collection.** `sealed::collect` (`crates/ordo-exec/src/sealed.rs:208`)
is an anti-join —`DELETE FROM "$sealedpayload" p WHERE NOT EXISTS (SELECT 1
FROM "$commandlog" c WHERE c.correlation = p.correlation)` — run **inside
the pruning transaction** (`crates/ordo-exec/src/prune.rs:904`). The
argument is at `prune.rs:119`: *"If the delete commits and the collection
does not … the query that would find it needs the very rows that were
removed. Unreachable and undeletable is worse than either alone."* No
reference counting, no mark phase, because a handle appears in exactly one
group by construction (`sealed.rs:186`).

**The undo horizon.** `LogPolicy` (`crates/ordo-exec/src/cadence.rs:92`)
carries `undo_horizon: Duration`, default 90 days, and its doc says it
*"also gates when a retired DEK epoch key may be destroyed."*
`prune::Floor` (`crates/ordo-exec/src/prune.rs:445`) resolves it to a
position and `FloorHolder` (`prune.rs:378`) names *why* the floor is where
it is — `UndoHorizon`, a lagging `Consumer`, or the `RetainedTail`.

**Revert restores rows, and re-seals under the current epoch.** A revert of
a delete applies an inverse `Create`: resolve every handle to plaintext
first, refusing before any write (`crates/ordo-exec/src/undo.rs:783`), then
`INSERT` (`undo.rs:1171`), sealing each value freshly
(`undo.rs:1666`). Three reasons are given (`undo.rs:1598`), the first being
that writing an old envelope back *"would quietly re-introduce a retired
epoch into a live row and undo part of a rotation that a revocation
forced."* This is §4's whole constraint.

**Platform ops.** `platform_ops::specs()`
(`crates/ordo-host/src/platform_ops.rs:176`) is the single source; the
recipe is stated in the module doc at `:48` — *"a spec in `specs`, an arm in
`PlatformOps::execute`, and a grant in `crate::deployment_engine`"*, plus
*"write ops will additionally need a command group and a chain append."*
`owner_only` at `:306`; the registration loop that calls
`engine.require_step_up(&spec.name)` is `crates/ordo-host/src/lib.rs:1379`.
`require_step_up` (`crates/ordo-identity/src/access.rs:554`) names its own
registry, and **custody conversion is on that list**.

**The door has one route.** `ordo_api::router`
(`crates/ordo-api/src/router.rs:98`) is `Router::new().route("/op/{name}",
any(dispatch))`. `dispatch` takes `body: Bytes`
(`router.rs:142`) — fully buffered. There is no `DefaultBodyLimit` anywhere
in `crates/`, so axum's implicit 2 MB default governs, untested and
undocumented. The only streaming in the tree is SSE for live invalidation
(`router.rs:995`).

**No BYO-anything resolver exists.** `PoolConfig` (`crates/ordo-exec/src/lib.rs:60`)
is read from env and takes no space; `PlatformState.space` is a `String`
(`crates/ordo-host/src/serve.rs:80`). The only resolver-shaped pair in the
tree is `KeyProvider` + `WrappedKeyStore` (`custody.rs:207`, `:407`), whose
own doc explains the split: *"so a deployment's **storage** choice and its
**custody** choice are independent."* That is the template `definition-
language.md:346` names.

**No share-link machinery.** The only expiring-and-revocable artifact is the
capability grant (`crates/ordo-host/src/grants.rs:46`, `is_live` at
`crates/ordo-identity/src/grants.rs:100`). Session tokens expire
(`crates/ordo-identity/src/token.rs:90`) but are not revocable per-link.

### 1.1 One defect found while reading, which B2 must fix first

**`param_ty` has a wildcard, and it is already costing something.**
`crates/ordo-host/src/lib.rs:340`:

```rust
fn param_ty(t: &ScalarTy) -> ParamTy {
    match t {
        ScalarTy::Int => ParamTy::Int,
        ScalarTy::Money => ParamTy::Money,
        ScalarTy::Date => ParamTy::Date,
        ScalarTy::Bool => ParamTy::Bool,
        ScalarTy::Id => ParamTy::Id,
        ScalarTy::Duration(_) => ParamTy::Duration,
        _ => ParamTy::Text { max_len: None },
    }
}
```

`ParamTy` has no `Datetime` (`crates/ordo-api/src/spec.rs:26`), and
`api-engine-design.md` §5's wire table has no `datetime` row — yet
`$t: datetime` parses (`declparse.rs:836`, and a test writes one at
`declparse.rs:2634`). So a `datetime` parameter binds at the door **as
unbounded text**, with no ISO-8601 validation, today. The wildcard is the
reason nobody noticed.

This matters to this plan out of proportion to its size: it is the **one
place** a new `ScalarTy` slips through silently instead of breaking the
build, and it is the door — the place where "it bound as text" becomes a
caller-supplied string reaching a plan. §3 counts it as ratchet 7 and B2
converts it to an exhaustive match.

---

## 2 · The ruling on handle addressing: keyed-random, never content-hash

**[proposed] A blob handle is 128 bits drawn from a per-space, per-epoch
keyed stream — `mint_sealed_ref`'s derivation, one domain over. It is never
derived from the bytes, the ciphertext, the mime type, the size, the row,
or a counter.**

```text
seed   = 32 fresh CSPRNG bytes, drawn per upload and discarded
salt   = per-space key material, labelled ("$blob", "handle.v1:<epoch>")
handle = HMAC(salt, seed)[..16]
```

Spelled as a second defaulted method on `KeyProvider`, `mint_blob_ref`, for
the same reason `mint_sealed_ref` is defaulted (`custody.rs:314`): a
wrapping provider — metering, a KMS — mints handles under its own custody
for free, with no method to forget to forward. A distinct domain (`$blob`
rather than `$sealed`) keeps the blob handle key disjoint from the sealed-
payload handle key, so a blob handle can never resolve as a payload handle.

### 2.1 The argument

**It is not merely a precedent; it is a correction that would be re-broken.**
`value.rs:9` records that the sealed handle *was* specified as SHA-256 of
the ciphertext and that the specification was wrong. Content-addressing
blobs adopts the withdrawn spelling in a new place, one deploy after the
codebase wrote down why.

**For a sealed blob field the question is already closed by the type
system.** A `blob` in a `sealed` field puts its reference into the field's
command image, and the image of a sealed field is `Value::SealedRef` by
rule — *"random per write; never derived from the value or its ciphertext,
so it leaks nothing — not even equality"* (`value.rs:35`). There is no legal
encoding of a content hash in that slot. Content addressing would therefore
have to mean *content-addressed in the bucket, randomly-handled in the log*
— two identities per object, and the mapping between them is itself the
oracle. One identity, or the oracle comes back through the join.

**The leak is worse for blobs than for columns.** A sealed column holds a
name, an amount, an email. A blob holds a document: a signed contract, an
identity photo, a template every tenant in an industry uses. A content hash
in an object key answers *"do these two spaces hold the same file?"* across
every tenant, forever, to anyone with bucket-level read — which is precisely
the question a per-space DEK exists to make unanswerable, and it survives
`destroy_space` for the same reason the digest case does: destroying the key
does not destroy the hash.

**The three things the salt buys transfer unchanged** (`custody.rs:274`):
the RNG's failure domain stops at the space boundary, so a repeated seed —
a restored VM snapshot, two replicas from one image — is a primary-key
violation inside one space rather than a cross-tenant equality signal;
resolution is scoped by construction rather than by a `WHERE` clause (here,
by the `$blobref` primary key *and* the bucket key prefix, two independent
mechanisms agreeing); and the stored value stays metadata-free, so an
object key does not advertise its epoch and therefore does not print a
**revocation timeline** to anyone holding a bucket listing and no keys.

### 2.2 What content addressing was for, and how each half is kept

**Deduplication** is refused, for the reason `$sealedpayload` refuses it
(`sealed.rs:24`): dedup *is* the oracle. Two uploads of the same file store
it twice. The cost is stated rather than hidden — `sealed.rs:36` already
states the analogous cost for converges over sealed fields — and it is
larger in absolute terms here, so the shape summary that already prices
`inline` against external must price it.

**Integrity** is kept, and DL §2 already says where: the row stores a
*"sealed integrity hash"*. Sealed is the operative word. The digest lives
inside an envelope under the space DEK, so the platform can verify a fetched
object against it, and nobody without the DEK can use it to compare two
objects. A plaintext digest column would be the content hash by another
name.

### 2.3 A correction this plan owes `definition-language.md` §2

§2 says *"Blobs are immutable (new content = new blob id → serving URLs are
permanently cacheable)"*. That sentence reads as though content addressing
were the mechanism. It is not, and it does not need to be: **immutability
comes from never reusing a handle, not from deriving one.** A keyed-random
handle minted per upload, inserted and never upserted — the rule
`$sealedpayload` already enforces (`sealed.rs:154`) — gives exactly the
property the parenthesis claims. §2 should be amended to say so, because a
reader implementing from that sentence would reach for a hash.

### 2.4 Two exceptions considered and declined

**Content addressing for unsealed `blob` fields only.** Declined: it makes
the addressing scheme a function of a field modifier, so adding `sealed` to
an existing blob field — a class-B change in `plan-schema-evolution.md`'s
taxonomy — would have to rewrite every object key in the bucket. One
scheme, whatever the custody.

**Content addressing for `uri`.** A non-issue: `uri` holds an address, not a
handle, so there is nothing to address. It carries an optional integrity
hash and no custody, and DL §2 already states the consent-page consequence
("referenced, not held").

### 2.5 The object key

`<space-key-prefix>/<hex handle>` and nothing else. No entity, no field, no
date, no epoch. Reason (3) above for the epoch; for entity and field,
because a bucket listing would otherwise enumerate the schema to anyone who
can list. `entity`, `field` and `epoch` live in the `$blobref` row, exactly
as `$sealedpayload` carries them, and the seal's AAD binds `(entity, field)`
the same way, so an object moved between fields refuses to open
(`sealed.rs:112`).

`migration.md` §6's *"blob bytes, laid out by entity/field/id"* describes the
**export bundle's** layout, not the bucket's, and the export can produce it
because it holds the `$blobref` rows. No conflict.

---

## 3 · Every ratchet the type-add trips

Adding `ScalarTy::Blob` and `ScalarTy::Uri` is deliberately loud. This is
the inventory, split by whether the compiler forces the decision or lets it
pass. **Making the soft ones hard is part of B2, not a follow-up.**

### 3.1 Hard — the build breaks until someone decides

| # | Site | What it forces |
|---|---|---|
| 1 | `ScalarTy::as_str` — `crates/ordo-syntax/src/decl.rs:60` | The spelling. Exhaustive match, no wildcard. |
| 2 | `ordo_schema::ddl::column_ty` — `crates/ordo-schema/src/ddl.rs:78` | **The column.** Exhaustive, and already argued: the `Decimal` arm is *"named rather than left to a wildcard so that if it ever becomes storable it lands on `numeric` and not on `double precision`."* |
| 3 | `ordo_exec::export::columns_of` — `crates/ordo-exec/src/export.rs:411` | The bundle encoding. Carries the standing comment: *"No wildcard arm, deliberately … a `_ => Text` would decide it by accident and silently."* |
| 4 | `ordo_syntax::declparse::scalar_ty` — `crates/ordo-syntax/src/declparse.rs:824` | The keyword, **and its closed-set rejection message**, which names the eight legal types in prose. |
| 5 | `ordo_commandlog::Value::tag` — `crates/ordo-commandlog/src/value.rs:42` | Fires only if a blob reference needs a new `Value` variant. Tag bytes are **frozen** ("part of the canonical layout"). §4.1 rules that it does not. |
| 6 | `platform_ops::manifest`'s `ParamTy` match — `crates/ordo-host/src/platform_ops.rs:348` | Fires only if `ParamTy` grows. §4.2 rules that it does not. |

Ratchets 5 and 6 are the interesting ones: they fire *if and only if* the
plan makes the wrong decision, which is the best kind of ratchet.

### 3.2 Soft — silently wrong today, made hard in B2

| # | Site | What slips through |
|---|---|---|
| 7 | **`ordo_host::param_ty` — `crates/ordo-host/src/lib.rs:348`, `_ => ParamTy::Text { max_len: None }`** | A blob parameter binds at the door as unbounded text. Already mis-binds `datetime` (§1.1). **The most dangerous arm in the tree for this change.** |
| 8 | `ordo_plan::interface::align` — `crates/ordo-plan/src/interface.rs:267`, `_ => "left"` | A file field renders left-aligned like prose. |
| 9 | `ordo_sema::binary_ty` — `crates/ordo-sema/src/lib.rs:5026`, `_ => l` | `blob + blob` types as `blob`. |
| 10 | `ordo_sema::fn_domain` — `crates/ordo-sema/src/lib.rs:5069` | `None` means *"accepts any scalar"*, so `lower(f)`, `length(f)`, `hash(f)` are admitted by default unless blob is excluded from every domain. |
| 11 | `ordo_sema::lit_ty` / `check_param_ty` — `lib.rs:4997`, `:4932` | No literal form exists for a blob (correct), but nothing says so by name. |
| 12 | `clients/ordo-react/src/descriptor.ts:39` `OrdoType` + `form.ts:238`'s `default: return null` | The TS union omits the member and the validator waves it through. |
| 13 | **`editors/ordo.tmLanguage.json:47` `support.type.ordo`, and the literal regexes at `:385` and `:392`** | **Not ratcheted at all.** See below. |
| 14 | `ordo_exec::undo::literal()` — `crates/ordo-exec/src/undo.rs:1728` | Refuses `Value::SealedRef` on the plain-value path; a blob reference needs the same guard or it reaches SQL as a literal. |

**Ratchet 13 is a hole in an otherwise complete parity test.**
`crates/ordo-syntax/tests/editor_grammar.rs` cross-checks the editor grammar
against the lexer for reserved words (`Keyword::ALL`, `:158`), event kinds
(`EventKind::ALL`, `:359`), expression functions (`:365`), aggregates
(`:370`), allocate modes (`:375`) and duration units (`DurationUnit::ALL`,
`:380`). It does **not** check `support.type.ordo`, because there is no
`ScalarTy::ALL` — grep confirms none exists. So adding `blob` to the
language and forgetting the grammar leaves it uncoloured, and every VS Code
user learns the type set is nine words from a file that says eight, with no
test failing.

**B2 adds `ScalarTy::ALL` and the missing assertion.** That converts ratchet
13 from soft to hard and closes a gap that pre-dates this work. It also
removes the two hand-written regexes' independence — they must be derived
from the same list or asserted against it.

### 3.3 Why this is a feature

The point of the exhaustive-match discipline is that **a language change
announces itself everywhere it must be decided**. Six sites stop the build
and demand an answer; the plan's job is to make sure the other eight also
demand one. After B2 the count is fourteen hard and zero soft, and the next
person who adds a scalar type inherits a better ratchet than this one did.

---

## 4 · The shape

### 4.1 The column: a reference, never bytes

**[proposed]**

| Declaration | Column | Holds |
|---|---|---|
| `f: blob` | `text` | the hex handle |
| `f: blob sealed` | `bytea` | the sealed envelope over the hex handle |
| `f: blob inline` | `bytea` | a `SealedHandle` into `$sealedpayload` — the bytes themselves |
| `f: uri` | `text` | the address |

One field, one column. That is not aesthetics: `ordo_schema::diff`'s
`Change::AddOptionalField { entity, field }`
(`crates/ordo-schema/src/diff.rs:199`) names a *field*, `live_drift`
compares columns one-for-one, and the class-A additive path
(`plan-schema-evolution.md` increment 2) works unchanged if a blob field is
one column. Four sibling columns for `(handle, mime, size, hash)` would make
one field add read as four changes and would put plaintext `size` in the
database for a sealed field.

The metadata lives in a reference table beside the payload store, with the
same column list for the same reasons:

```sql
CREATE TABLE "$blobref" (
  "$space" text, "ref" bytea, "epoch" smallint,
  "entity" text, "field" text,
  "mime" bytea, "size" bytea, "integrity" bytea,
  "bytes" bigint, "created_at" bigint,
  PRIMARY KEY ("$space", "ref") )
```

`mime`, `size` and `integrity` are **sealed envelopes** when the field is
sealed, plaintext otherwise. `size` is sealed for the same reason a value
is: file sizes fingerprint documents. `bytes` is the *ciphertext* length,
plaintext, because the store needs it to serve a range and it is already
observable from the bucket. `$blobref` registers in
`Pool::ensure_platform_tables` beside `SEALED_PAYLOAD_DDL`
(`crates/ordo-exec/src/lib.rs:296`).

**No new `Value` variant** (ratchet 5 does not fire). A blob reference in a
sealed field is a `Value::SealedRef` like any other sealed value — the
handle is sealed and the envelope goes to `$sealedpayload`. In an unsealed
field it is a `Value::Text` holding the hex. Both existing encodings are
correct; adding a sixth tag byte to a frozen layout to say "this text is a
blob handle" would put a type tag in the audit spine that the schema already
answers.

`mime` and `size` are readable as `f.mime` / `f.size` — a dot-path on a
blob field, which is the spelling a ref already uses, so the grammar gains
no new form.

### 4.2 What the language admits, and what it refuses

**[proposed]** A `blob` value is opaque. It is:

- **not comparable, not sortable** — a keyed-random handle has no order and
  no meaningful equality; two uploads of one file compare unequal, and a
  language that let you write `f == g` would be promising something the
  addressing scheme deliberately destroyed;
- **not aggregatable** except by `count`;
- **not `unique`, not `indexed`** — a blind index over a random handle
  indexes randomness; `f.size`/`f.mime` are ordinary sealed values and index
  normally;
- **never a parameter** (ratchet 6 does not fire) — bytes are not a JSON
  wire value, and `api-engine-design.md` §5's `ParamTy` table mirrors the
  language for values that fit an envelope key. Bytes arrive through the
  blob door (§B3), which is why that door exists;
- **never a `default`**, and never the RHS of a write expression. The only
  producer of a blob value is an upload.

`uri` is text-shaped: comparable, sortable, uniquable, sealable, blind-
indexable. It is an address with an optional integrity hash and no custody.

These are the answers ratchets 8–11 demand, stated once here so the fourteen
sites all say the same thing.

### 4.3 What goes in the bucket

Ciphertext, always, for a sealed field — DL §2's pin. The platform seals
before the bytes reach the store, and serving decrypts in the platform path
after the access check and key release. So **uploads to a space-sealed blob
proxy through the API**; presigned direct-to-bucket is available only to
`private` (already ciphertext), which is F7's.

---

## 5 · Garbage collection, and why the undo horizon owns it

### 5.1 The reference set

Two sources, both derivable and neither optional:

1. **Live rows.** The compiler knows every blob field — `ddl_for` already
   walks every field of every entity — so the live reference set is a union
   of `SELECT <col> FROM <entity>` over a list the schema produces.
2. **Retained command images.** Every `$commandlog` image naming a handle,
   which for a sealed field means every `$sealedpayload` row whose plaintext
   *is* a blob reference.

(2) is the part a naive implementation drops, and dropping it breaks a
guarantee rather than leaking bytes.

### 5.2 The undo-horizon tie, stated as the constraint it is

A row deleted a minute ago is revertible for `LogPolicy::undo_horizon` —
90 days by default (`cadence.rs:117`) — and the revert is an inverse
`Create` that re-`INSERT`s the row from its before-image
(`undo.rs:1171`). If a sweep deleted the bytes in between, the revert
succeeds and restores a row pointing at nothing: a dangling reference the
shape gate cannot see, produced by a mechanism whose whole job is to make
deletes reversible.

So **blob GC's floor is `prune::Floor`** — the same value, from the same
function, with the same `FloorHolder` (`prune.rs:378`) reported for the same
reasons. Not a second horizon, not a `BlobPolicy` number. When a lagging
mirror consumer holds the log's floor, it holds the blob sweep's floor too,
and the operator sees one name for one cause.

### 5.3 The ordering, which is the inverse of the payload case

`$sealedpayload` collects *inside* the pruning transaction because deleting
the log first would leave payloads nothing can name (`prune.rs:119`). Bytes
in a bucket cannot join that transaction, so the safe order inverts:
**delete the reference last.**

**[proposed]** A two-phase sweep with a tombstone table:

```sql
CREATE TABLE "$blobtomb" (
  "$space" text, "ref" bytea,
  "unreferenced_since" bigint, "deleted_at" bigint,
  PRIMARY KEY ("$space", "ref") )
```

1. **Phase 1, transactional, Postgres only.** The anti-join over §5.1's two
   sources finds handles with no live row and no retained image; `INSERT` a
   tombstone. Nothing in the bucket is touched, so a crash here costs
   nothing.
2. **Phase 2, out of transaction, idempotent.** For each tombstone whose
   `unreferenced_since` is below `Floor::position`, re-check the anti-join
   under the row lock, `DELETE` the object, then set `deleted_at`. A crash
   between the delete and the mark leaves the tombstone open and the sweep
   retries; bucket deletes are idempotent.
3. **A revert cancels a tombstone** rather than racing it — the re-check in
   phase 2 is what makes that true without a lock across the two systems.

The asymmetry is deliberate and worth stating: this order can leak an
object, which costs money; the other order can break a revert, which costs a
guarantee.

### 5.4 `blob inline` is GC-free, and now for a reason

DL §2 calls `inline` GC-free ("row-owned"). In this design that is not an
assertion, it is an inherited mechanism: the bytes are a `$sealedpayload`
row, so `sealed::collect_pruned_in` already collects them, in the pruning
transaction, with no new machinery and no tombstone. That is the strongest
argument for keeping `inline` a first-class tier rather than treating it as
an optimisation of the external path.

---

## 6 · Ruled out, with reasons

**Bytes in Postgres (`bytea`) as anything but the inline tier.** Four
reasons, and the fourth is the one that decides it:

1. It puts the largest bytes in the thing every backup, every replica and
   every `pg_dump` copies. `implementation.md` prices Postgres as *the only
   stateful dependency that must exist*; inflating it inflates the floor of
   every self-host.
2. TOAST puts a multi-megabyte value on the read path of a query that
   projected other fields, and the cost-class system has no way to say so.
3. The export bundle's per-entity JSONL would carry megabytes of hex per row
   — `select_expr` already renders sealed columns as `encode(…, 'hex')`
   (`export.rs:459`) — and the bundle would stop being loadable by ordinary
   tooling, which is the property `plan-export.md` §2 exists to hold.
4. **No range reads, ever.** A video, a large PDF, a resumable download all
   need a byte offset; a `bytea` column has one read and it is the whole
   value.

The 256 KB inline cap is exactly the point where those four costs are
smaller than the second system. That is why the cap is a **number**, tunable
by policy, rather than a principle.

**A second storage engine.** One trait, one binding, per the owner's ruling.
Self-hosters point the S3-compatible binding at a local gateway pod
(MinIO/SeaweedFS); hosted deployments point it at S3/R2. A filesystem
binding exists, and is argued below as a **test double**, not an engine.

**CDN and edge caching.** Immutable, never-reused handles make this purely
additive later: nothing about the handle scheme, the key layout, or the
share-link contract changes when a cache is put in front. Building it now
buys a configuration knob and an invalidation story for a load nobody has.

**Content-addressed deduplication.** §2.

**Server-side media processing** — thumbnails, transcoding, vision, OCR,
extraction. `blob private` forbids it by design; `blob` would need a media
pipeline, a job class and a cost story. It is a product, not an increment.

**Resumable and multipart upload.** Deferred with a stated trigger: when a
real upload exceeds the request timeout. B3 builds streaming single-request
upload with a policy cap; multipart is an addition inside the same trait.

**`blob private`.** F7 Phase 6 (`f7-vault-plan.md:524`). B3 leaves the seam
— the store never inspects the bytes, so a client-sealed object rides the
same `put`/`get` — and the presigned direct-to-bucket path is F7's to open.

---

## 7 · Build order

### B1 — the store trait, the two bindings, and the resolver

A new crate, `ordo-blob`, so the dependency blast radius of an HTTP client
and a TLS stack stays in one place and never reaches the compiler crates.
Today the workspace has **no HTTP client and no TLS at all**, so this is a
genuinely new dependency axis and it gets its own increment.

- `BlobStore`: `put(space, handle, reader)`, `get(space, handle, range)`,
  `head(space, handle)`, `delete(space, handle)`. Streaming both ways.
- `StoreResolver::resolve(&SpaceId) -> Arc<dyn BlobStore>` — the shape
  `definition-language.md:346` names, mirroring `KeyProvider` +
  `WrappedKeyStore`. The default implementation returns one store for every
  space, which is honest about the fact that no space directory exists
  (`plan-schema-evolution.md` §1 records the same absence).
- `FsStore` — the test double. `S3Store` — SigV4 over one HTTP client, hand-
  rolled over the existing `sha2` dependency in the house style
  (`ordo-identity/src/token.rs` and `ordo-commandlog/src/chain.rs` are the
  precedents for hand-rolled canonical crypto).
- `KeyProvider::mint_blob_ref`, defaulted, `$blob` domain (§2).
- `BlobPolicy { inline_cap: 256 KiB, max_object, share_ttl_default }` (renamed from `share_ttl_max` 2026-09-29: seven days is a default, not a cap) shaped
  like `LogPolicy` (`cadence.rs:92`): a plain struct, an associated
  `DEFAULT`, authored in the definition language, carried by value.

**On the filesystem binding — the argument, since it invites the "two
bindings, two behaviours" objection.** For: the language change (B2) and the
GC design (B4) are the parts that must be right, and neither should wait on
a TLS stack; CI has no object-storage service and `docker/` has none;
`MemoryKeyStore` (`custody.rs:429`) and `ordo-api`'s *"one implementation
each today, in-memory"* (`api-engine-design.md` §12) are the established
pattern. Against, answered: a filesystem has no eventual consistency, no
5xx, no key-length limit — **so it is a test double and not a supported
deployment target**, a production configuration naming it refuses to boot
with the reason, exactly as `LocalKeyProvider::from_env` refuses a default
master key (`custody.rs:519`). Conformance is held by making the *trait's*
contract the test surface: one suite, both bindings, the S3 run gated on a
live endpoint the way the Postgres tiers already are
(`ordo-conformance/src/lib.rs:155`).

*Done when:* one trait-conformance suite passes against `FsStore` and,
gated on a live endpoint, against `S3Store` — including a range read, an
overwrite refusal, and a delete of an absent key. And a deployment
configuration naming `FsStore` outside dev mode refuses to boot, with the
message saying which property it cannot provide.

### B2 — `blob` and `uri` through parser → schema → DDL, and every ratchet

The language change. Ratchets 1–6 fire; 7–14 are converted from soft to
hard in the same commit, because a plan that leaves them soft has not
actually announced the change.

- `ScalarTy::{Blob, Uri}`, `ScalarTy::ALL`, `as_str`, the `scalar_ty`
  keyword table and its closed-set message.
- `inline` as a field modifier beside `sealed` / `indexed` on
  `ordo_schema::Field`.
- The columns of §4.1 and the `$blobref` table.
- The refusals of §4.2, each with the teaching-error shape
  `adoption-dx-plan.md` §3.3 commits to: `rule:` then `fix:`.
- `param_ty` made exhaustive (§1.1) — which also surfaces the `datetime`
  defect, whose fix is a `ParamTy::Datetime` and a row in
  `api-engine-design.md` §5's table.
- `ScalarTy::ALL` asserted against `editors/ordo.tmLanguage.json`'s
  `support.type.ordo` in `crates/ordo-syntax/tests/editor_grammar.rs`,
  joining the five inventories it already checks.

*Done when:* `entity doc (key: doc) { file: blob sealed  avatar: blob
inline  link: uri }` parses, binds, emits DDL, survives the shape gate and
appears in `ordo interface`'s golden — and every one of the fourteen sites
in §3 either broke the build during the change or is covered by a new
assertion that would have made it break.

### B3 — the blob door: streaming, sealing, caps, chunked STREAM

- **Two new routes**, and the argument for why they are not ops: an op's
  contract is `dispatch(…, body: Bytes)` — fully buffered, governed by an
  implicit 2 MB limit — and its envelope carries JSON parameters. Bytes are
  not a parameter (§4.2). So `PUT /blob/{entity}/{field}/{id}` and
  `GET /blob/{ref}` are a second door, access-checked by the same
  `check_access` (`router.rs:453`) against the same bound space, with a
  streaming body.
- **Chunked STREAM in `ordo-crypto`.** `seal_with` is single-shot
  (`lib.rs:84`); range reads need the STREAM construction
  `implementation.md` names and `lib.rs:13` records as absent. A chunk
  header carrying the epoch byte, per-chunk nonces derived from a base nonce
  and a counter, and a final-chunk tag so truncation is detected. This is
  the one piece of genuinely new cryptography in the plan and it is a
  separate reviewable unit. It also lifts F7's stated exclusion
  (`f7-vault-plan.md:767`).
- **Caps, made explicit.** An explicit `DefaultBodyLimit` on the op route —
  making the implicit 2 MB deliberate and tested — and a streaming byte
  counter on the blob route refusing at `BlobPolicy::max_object` with the
  existing `bound_violation` class. No new error class: the standing rule is
  that a new refusal class is an oracle until argued
  (`platform_ops.rs:126`), and "too large" is a bound, which the door
  already has a word for.
- `blob inline`: the cap is checked before the sealer, so an oversize write
  is refused before a byte is sealed, and the bytes land in
  `$sealedpayload` through the existing `put`
  (`crates/ordo-exec/src/sealed.rs:138`).

*Done when:* a 100 MB upload to a sealed blob field streams through the
sealer to the store in bounded memory (asserted, not assumed); a range read
at an offset in the middle returns the correct plaintext; an oversize
`blob inline` write is refused with `bound_violation` before any byte is
sealed; and a test that reads the stored object directly asserts the
plaintext does not appear in it.

### B4 — reference GC, floored at the undo horizon

`$blobtomb`, the two-phase sweep of §5.3, the reference set of §5.1, the
floor of §5.2. `inline` needs nothing (§5.4).

*Done when:* deleting the last row referencing an object leaves the bytes
present, and a revert inside the undo horizon restores a row whose blob
still reads; and after the horizon passes the sweep removes the object and
reports the same `FloorHolder` the log prune reports, for the same reason.

### B5 — BYO bucket as a granted op, and export

- **`std.space.attachBucket`** by the recipe `platform_ops.rs:48` states: a
  spec in `specs()`, an arm in `execute`, `owner_only`, and
  `engine.require_step_up` from the registration loop
  (`crates/ordo-host/src/lib.rs:1379`). Attaching a bucket is custody
  conversion, which `require_step_up`'s own doc already lists
  (`access.rs:546`).
- **The idempotency gap is closed here, not inherited.**
  `exit_ops.rs:37` records that `PlatformExecutor` drops the durable
  `ReplayLease`, *"safe here only because both exit ops are idempotent by
  construction."* Attach-bucket is not. Either the lease is carried, or the
  op is keyed on the target's identity so a repeat is a no-op. This is B5's
  first bullet, not a footnote.
- **The record**: a `CustodyAction { action: "bucket_attached" }` on the
  security-event stream, beside `space_exported` and `space_erased`
  (`exit_ops.rs:157`). The same reinterpretation of "command-logged" that
  `plan-export.md` §5.1 argued applies for the same reason — this writes no
  entity row.
- **The disclosure**: attaching a bucket changes what crypto-erasure means,
  the way BYO-DB does. Platform-sealed (their bucket holds ciphertext they
  cannot read alone) versus their own perimeter is the choice, and the
  consent page states which one was taken. The BYO-DB consent gap on the
  `export` decision row is the same gap and should be answered in the same
  breath.
- **Export**: `columns_of` grows its blob arm (ratchet 3), the bundle grows
  `blobs/<entity>/<field>/<id>` per `migration.md` §6, and
  `manifest_json`'s `unbuilt` list loses its only entry.

*Done when:* the export manifest's `unbuilt` list is empty for a schema
carrying blob fields and the bundle holds the bytes at the declared layout —
and `crates/ordo-exec/tests/export.rs:496`'s `unbuilt` assertion has been
**rewritten to assert the empty list**, rather than deleted.

### Deferred, deliberately

- **`blob private`** — F7 Phase 6. B3 leaves the seam.
- **Share links.** Settled in DL §2 (minting is an op, TTL bounded by space
  policy, revocable, key in the URL fragment for `private`). Nothing in the
  tree mints a revocable bearer link; the nearest machinery is the
  capability grant's `expires` + `revoked_at`
  (`crates/ordo-identity/src/grants.rs:100`), which is the right shape to
  reuse. It waits on B3's serving path and on `private` for the fragment
  half, so building it before either would be building against two unbuilt
  things.
- **Multipart / resumable upload**, per §6.
- **A space directory.** `StoreResolver` is written to want one and given a
  single-store default until it exists. Same absence
  `plan-schema-evolution.md` defers for per-space shape versions.

---

## 8 · The ruling this plan owes, and what shipped without it

B1–B5 are built. This section is the one thing that was **not** decided,
sharpened by having built the rest — the original text asked half of it,
and the build surfaced the other half, which is the larger one.

Two questions, in dependency order. The second cannot be answered until
the first is, and the first is not what §8 originally asked.

---

### 8.1 **[pinned]** What does the upload itself write into the chain?

Ruled 2026-09-08, see `corpus-rulings.md` and `plan-blob-chain.md`.

**As built: nothing.** `ordo_exec::blob::attach`
(`crates/ordo-exec/src/blob.rs:128`) inserts the `$blobref` row
(`:138`) and then `UPDATE`s the entity row's column (`:161`), both inside
the caller's transaction — and **appends no command group**. There is no
`append_group` call on the blob path at all.

So a blob upload mutates a row and leaves nothing in `$commandlog`. Three
consequences, none of them subtle once stated:

- **An upload cannot be reverted.** Attaching a file is not an act the undo
  machinery can see, so there is no inverse to compute. Replacing a
  document is likewise irreversible — the old handle is overwritten in the
  column and survives only until `blobgc`'s floor passes, at which point
  the object is collected and the previous version is gone.
- **No trigger fires.** `EventKind::Converged` and `EventKind::Written`
  (`crates/ordo-syntax/src/trigger.rs:94`, `:96`) are derived from command
  groups. A listener on `converged` over an entity with a blob field does
  not see the file arrive, which is precisely the event most such listeners
  exist for.
- **`rev` does not move.** The row's version is unchanged by a file
  replacement, so a caller holding `rev` has a token that is stale in fact
  and valid in form.

**What is NOT broken, stated so the ruling is not made against a worse
picture than the real one.** B4's garbage-collection argument survives
intact. A row's *delete* does go through the write protocol, and its
before-image carries every column including the blob handle — so the handle
**is** in the audit spine by that path, and §5.2's claim (every image that
could restore a handle is at or before `unreferenced_since`) holds. The
sweep is correct as built. What is missing is the record of the *upload*,
not the record that could resurrect it.

**Why this was not simply fixed.** The append is mechanically easy:
`TxConn::append_group` (`crates/ordo-exec/src/commandlog.rs:183`) takes the
caller's transaction, which is the transaction `attach` already holds. The
hard part is the **image**. `row_image`
(`crates/ordo-conformance/src/write.rs:2852`) builds a *full-row* image over
every field, and for a sealed field it mints a `SealedHandle` and stores the
payload through `seal_image_ref` (`:2818`). It is `pub(crate)` in
`ordo-conformance`.

Writing a second image builder in `ordo-exec` is the "written twice" hazard
in the one place it is least survivable. Two builders that disagree about
what an image is do not fail loudly; they produce a chain that verifies and
reverts into the wrong row state. The `$blobref` shape lives in
`ordo-crypto` for exactly this reason — *"a table whose shape is spelled
twice is a table that will be spelled two different ways"*
(`crates/ordo-crypto/src/sealed.rs`) — and an image is a stronger case of
the same rule, because the chain hashes it.

#### The candidates

**(a) [proposed] The blob door builds a converge group itself.** The
honest reading of what a blob write *is*: a converge on the row, before and
after images both full rows, appended in the transaction that already
writes the column.

*Needs:* `row_image` lifted out of `ordo-conformance` into something both
the write protocol and the blob door call — one builder, two callers, which
is the arrangement `guard_filter_query` already has for the guard predicate
(the write protocol, `undo::require_writable`, and now
`blob::require_visible` all share it). That is the precedent and it is a
good one.

*Costs:* the lift is real work and it crosses a crate boundary that
currently runs the wrong way (`ordo-exec` depends on `ordo-conformance`).
The natural home is `ordo-exec`, with `ordo-conformance` calling in — which
is a dependency inversion, not a move.

*This is what shipped, minus the group.* `attach` already writes the column
in the right transaction with the guard checked inside it; what it does not
do is build the images and append. Nothing about the current code has to be
undone to reach (a) — it has to be extended.

**(b) [declined, unless (a) proves impossible] The upload returns a handle
and a subsequent op sets the column.** Splits the act in two: `PUT /blob/…`
stores the object and answers with its handle, and the caller then runs an
ordinary write op that assigns it — which makes the column write an
ordinary converge with no new machinery at all.

*Why it is refused today:* `blob` is not a parameter type
(`blob_parameter`, `crates/ordo-sema/src/lib.rs:200`), and the refusal's own
`fix:` line says why — *"an op parameter cannot name an object the caller
did not put there"*. An unscoped handle parameter is a caller naming an
arbitrary object; the only thing between that and a cross-row read is the
`("$space", "ref")` predicate, which stops cross-*tenant* reads and not
cross-*row* ones inside a space.

*What it would need:* a **scoped** handle — one the platform can prove this
caller minted, for this `(entity, field, row)`, recently. That is a new
authenticated artifact with a TTL, a binding and a revocation story, which
is a share link pointing inwards. It is more machinery than (a), not less,
and it introduces a second thing that looks like a capability.

**(c) [open] Something better.** The one shape worth naming: make the blob
door call the **write protocol** rather than reimplement a slice of it —
i.e. the upload compiles to a converge the ordinary executor runs, with the
handle bound by the platform rather than by the caller. This is (b)'s
ergonomics with (a)'s safety, and it needs the write path to accept a
platform-supplied value that has no source expression. Whether that seam
exists or is worth opening is the owner's call, not this plan's.

**The ask:** rule between (a) and (c). (b) is declined here with reasons; if
the ruling disagrees, the scoped-handle design needs its own section.

---

### 8.2 **[pinned]** Should the chain carry a digest of the object?

Ruled 2026-09-08, see `corpus-rulings.md` and `plan-blob-chain.md`.

This is §8's original question, and it is narrower than it was, because
**the digest already exists** — it is just not in the chain.

`$blobref` carries a **sealed** `integrity` column
(`crates/ordo-crypto/src/blobref.rs:111`), computed over the plaintext as
the bytes stream past and sealed under the space DEK, and
`BlobPipeline::verify` (`crates/ordo-blob/src/pipeline.rs:445`) opens it and
compares. So the cost objection the original §8 raised — *"a hash of every
uploaded byte on the write path"* — is already paid, and paid for a reason
DL §2 settles independently (*"the row stores a … sealed integrity hash"*).
What remains open is whether that digest belongs in the **command image**.

The unchanged parts of the argument:

- **The bytes are not in the chain and cannot be.** A converge's before
  image exists so the old value is recoverable; for a sealed column the
  payload store holds the plaintext behind the handle. For an external blob
  the "payload" is an object in a bucket, and the chain's guarantee would
  then depend on a system the chain cannot verify — the first time anything
  in the audit spine has depended on storage it does not own.
- **A revert of a blob converge restores a *handle*, and whether that
  handle still resolves is B4's floor, not the chain's promise.** §5.2 makes
  them agree by construction. The question is whether "agree by
  construction" is enough, or whether the chain should carry something that
  makes the dependency **checkable** — so a revert verifies before
  declaring success rather than restoring a reference on faith.

What the build adds to it:

- **A digest in the image is a digest in a frozen encoding.** `Value`'s tag
  bytes are *"part of the canonical layout, frozen"*
  (`crates/ordo-commandlog/src/value.rs`), and `encode_value`
  (`crates/ordo-commandlog/src/chain.rs:283`) writes the tag first. Carrying
  a digest means either a new `Value` variant — a sixth tag byte, which §4.1
  argued against and which this plan's own ruling avoided — or a second
  field in the image map, which is cheaper and still moves every chain hash
  that carries one. **Adding it later is a migration; adding it at 8.1's
  landing is free**, because 8.1 is the commit that first writes a blob
  image at all.
- **A sealed digest in an image is not a content address.** Worth saying
  explicitly, because §2 spent its length refusing content-derived
  identifiers: the digest would be *sealed*, in a payload the log resolves
  through a handle, so it answers "is this the object I stored" to the
  platform and answers nothing at all to a reader of the plaintext log.
  That is the same distinction `$blobref.integrity` already relies on.
- **A failed verification at revert time still needs a rule.** Refuse, or
  restore-and-warn. `undo`'s existing posture is to refuse before any write
  (`crates/ordo-exec/src/undo.rs:783` resolves every handle to plaintext
  first, refusing before it inserts anything), and consistency argues for
  refuse — but a revert that refuses because a *bucket* is unreachable makes
  the undo horizon depend on object-store availability, which is the
  dependency the first bullet says the chain must not have. **This is the
  sharp edge and it is why the answer is not obvious.**

**The ask:** rule *carry* or *do not carry*, and if carry, rule the
failed-verification behaviour in the same breath.

---

### 8.3 What is safe to ship without the ruling, and what is not

**[settled] Safe, and shipped.** Everything that does not depend on the
upload being in the chain: the store tier and both bindings, the language
change, chunked STREAM, the sealing pipeline, the caps, the reference GC
and its floor, the BYO-bucket op, and the export. B4's correctness argument
is independent of 8.1 — see the "what is NOT broken" paragraph above — and
its tests exercise the delete-and-revert path that carries the handle.

**[open] Not safe to build on, until 8.1 is ruled.** Anything that assumes
a blob write is visible in history: a trigger on file arrival, an
undo/redo surface that offers to un-attach a document, a history view that
shows when a file changed, and any migration that replays the log to
reconstruct blob references. None of these exist, and none should be
started against the current behaviour.

**The honest one-line summary of what is deployed:** a blob upload is a
row mutation that the audit spine does not record, and every other property
this plan claims is unaffected by that. It is a gap, it is bounded, it is
named here, and it is the last thing between this feature and complete.
