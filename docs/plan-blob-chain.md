# Blob uploads through the write protocol, and the digest in the chain

`plan-blob.md` §8.1 and §8.2, ruled by the owner on 2026-09-08
(`corpus-rulings.md`, "Blob uploads take the write protocol's path, and the
chain carries the digest"). §8.1 is candidate **(c)**: the blob door hands
the write to the write protocol, so an upload is a converge the ordinary
executor runs. §8.2 is **carry the digest**, landed in the same commit.
The ruling also requires the two full-row image builders to become one.

Numbering is `C1–C8` for `roadmap-checklist.md` item 17g′.

Markers: **[settled]** restated from a settled document · **[pinned]** a
ruling recorded in `corpus-rulings.md` · **[proposed]** adopted here,
pending owner review · **[open]** named and not answered.

Status: build plan. **Nothing in it is built.** Every claim about the
current code was read against the tree on 2026-09-08 and carries its file
and line.

---

## 0 · The starting state, verified

Ten facts. Three of them contradict something in `plan-blob.md` §8 or in
`blob-chain-explained.html`, and those three are marked as contradictions
where they appear.

### 0.1 What `attach` does today [settled]

`ordo_exec::blob::attach` (`crates/ordo-exec/src/blob.rs:128`) takes the
caller's `TxConn` and does two things: it inserts the `$blobref` row
(`:138`, through `ordo_crypto::blobref::insert_sql`), then it `UPDATE`s the
entity row's column (`:161-169`) with a literal chosen from the tier
(`bytea_lit(handle)` when inline, `sql_lit(hex(handle))` otherwise,
`:156-160`). It appends no command group; there is no `append_group` call
on this path.

The door around it is `ordo_host::blob::PlatformBlobs::write`
(`crates/ordo-host/src/blob.rs:154`). Its order is: refuse `private`
(`:194`), refuse `inline` (`:207`), seal and store the object
(`:225-241`), open a transaction (`:243`), `require_visible(write=true)`
(`:249`), `attach` (`:251`), commit or roll back (`:265-277`).

**`PlatformBlobs` is constructed nowhere but a test**
(`crates/ordo-host/tests/blob_door.rs:174`), which the checklist already
records under 17g ("the tier is not mounted"). Every wiring change below is
therefore cheap: there is no live deployment whose door shape must be kept.

### 0.2 The rollback comment describes a sweep that cannot see the object [open]

`crates/ordo-host/src/blob.rs:269-274` says that after a rolled-back
attach "the object is now unreferenced" and "the sweep is the mechanism
that is allowed to remove bytes". The sweep cannot see it. Phase 1
(`crates/ordo-exec/src/blobgc.rs:194`) selects **from `$blobref`** and
tombstones rows with no live reference; an upload whose transaction rolled
back has no `$blobref` row, so it is not enumerable and no sweep will ever
reach it. `BlobStore` has `put`, `get`, `head`, `delete` and no `list`
(`crates/ordo-blob/src/lib.rs:204-229`), so nothing in the tree can
reconcile a bucket against the database.

This is a leak that costs money and not a guarantee, it predates this
plan, and (c) does not change it: the reference insert is still inside the
transaction that rolls back. §5 records it and does not fix it.

### 0.3 How a converge binds its right-hand side [settled]

`Fixture::run_write_body` (`crates/ordo-conformance/src/write.rs:519`)
parses the call, resolves the op in the registry (`:546`), binds
parameters **by name** into `Vec<(String, Expr)>` (`:565-585`), substitutes
`let`-bound descriptors before planning (`:611`), and plans each statement.
An addressed converge is `plan_addressed_set` (`:1671`): lock the row
(`:1687`), read guard then write guard (`:1690-1695`), split the `rev`
belief (`:1699`), take the before-image (`:1706`), build the `SET` list
through `assignments` (`:1707`, defined at `:3053`), and emit an `UPDATE`
that also does `rev = rev + 1` (`:1708-1716`).

Every right-hand side reaches SQL through one funnel: `assignments` calls
`eval_expr` (`:3536`), which turns an `Expr` into an
`EvaledVal { gate: CreateVal, sql: String }` (`:328`). **The seam (c) opens
is exactly this**: a value that has an `EvaledVal`-shaped result and no
`Expr` behind it. §1 is the design of that value.

After the mutation batch runs (`:642`), the post-image obligation re-checks
the write guard on every touched row (`:649-686`), the after-images are
taken (`:690-711`), the `CommandGroup` is built (`:713-722`), staged sealed
payloads are flushed under the group's correlation (`:728-738`), and the
group is left pending (`:750`). On the executor path
`Executor::write_attempt` (`crates/ordo-exec/src/lib.rs:883`) runs that
body inside one transaction (`:939`), appends the group inside the same
transaction (`:1024`) and commits (`:1033`).

### 0.4 The two image builders, and exactly how they differ [settled]

`Fixture::row_image` (`crates/ordo-conformance/src/write.rs:2852`) and
`undo::row_image` (`crates/ordo-exec/src/undo.rs:1423`) both iterate
`e.fields`, select `f.column()` for each, and decode by
`f.ty.value_ty()` into `Id` / `Bool` / `Int` / `Text`, with a sealed field
imaged as a `SealedRef` and an empty `bytea` imaged as `Null`
(`write.rs:2877-2907`, `undo.rs:1456-1493`). Five differences:

1. **The space predicate.** Conformance selects
   `WHERE {space_pred} AND "id" = …` (`write.rs:2866-2872`, `space_pred` at
   `:2137`). Undo selects `WHERE "id" = …` with no space column
   (`undo.rs:1443-1449`). **This is a latent bug, not a style difference.**
   Entity tables are `PRIMARY KEY ("$space", "id")`
   (`crates/ordo-schema/src/ddl.rs:378-380`), and ids are minted from a
   process counter as `format!("{}_w{}", e.key, log.next())`
   (`write.rs:1405`), so two spaces in one deployment produce colliding ids
   by construction. Undo's image of `doc_w7` is whichever row Postgres
   returns first. **Corrected while building C1: the omission is in seven
   statements, not four.** The four this section first named are
   `row_image` (`undo.rs:1443-1449`), `row_exists` (`:1373-1380`), the
   referrer scan (`:1344-1351`) and the converge revert's `UPDATE`
   (`:1133-1141`). Three more were found by grepping every `WHERE` in the
   module: the delete branch's own `DELETE` (`:1109`), `lock_row`
   (`:1281`) and `require_writable`'s bare-existence branch for unguarded
   entities (`:1326`), whose guarded sibling was already scoped and
   carried a comment claiming the pair were alike. The `DELETE` is the one
   that mattered: scoping the referrer scan alone lets a revert through
   that the unscoped scan used to refuse by accident, and the unscoped
   `DELETE` then takes both spaces' rows, so a partial fix is a net
   regression for that case. The `INSERT` was already scoped, and its
   comment says why (`undo.rs:1184-1190`). One statement stays unscoped on
   purpose: `find_group`'s `"space" <> …` probe (`:1018`), which exists to
   tell a cross-space revert attempt apart from a typo and returns a
   boolean, not a row.
2. **When the payload is written.** Conformance stages payloads
   (`write.rs:2835`) and flushes them once the group has a correlation
   (`:728`). Undo writes each immediately, with the revert's correlation
   (`undo.rs:1525-1535`).
3. **The lease.** Conformance releases a fresh write lease per sealed field
   with the correlation literal `"image"` (`write.rs:2827-2830`). Undo
   releases one lease for the whole revert, correlated to the group
   (`undo.rs:163-170`). Key releases are meant to be countable per
   correlation, so the literal is a wart worth removing in passing.
4. **No custody.** Undo refuses to image at all (`undo.rs:1473-1479`);
   conformance always holds a provider.
5. **The error type.** `String` versus `RevertFailure`.

Nothing here makes the two disagree on the *bytes of an image* for a single
space, which is why the chain has not been observed to be wrong. Difference
1 makes them disagree on *which row* is imaged as soon as a deployment
serves two spaces.

### 0.5 The crate dependency direction [settled]

`ordo-exec` depends on `ordo-conformance` (`crates/ordo-exec/Cargo.toml`),
and `ordo-conformance` depends on `ordo-syntax`, `ordo-schema`,
`ordo-sema`, `ordo-plan`, `ordo-commandlog`, `ordo-crypto`
(`crates/ordo-conformance/Cargo.toml`). It does **not** depend on
`ordo-blob`.

**It cannot.** `ordo-wasm` depends on `ordo-conformance`
(`crates/ordo-wasm/Cargo.toml`, with a comment explaining that the browser
runs the same `Fixture` constructor) and compiles for
`wasm32-unknown-unknown`. `ordo-blob` pulls `tokio` with `fs` and `net`,
plus `tokio-rustls` and `webpki-roots` (`crates/ordo-blob/Cargo.toml`),
none of which build for that target. This kills the tidiest version of §1
and is the reason §1 looks the way it does.

**This contradicts `plan-blob.md` §8.1 and the explainer's table**, which
both say (a) needs a dependency inversion because "the natural home is
`ordo-exec`, with `ordo-conformance` calling in". The inversion is one
possible answer; §3 argues for a different one, and the wasm constraint is
the fact that decides it.

### 0.6 `Value` is frozen, and an image is a map [settled]

`ordo_commandlog::Value` has six variants and its tag byte is "part of the
canonical layout, frozen" (`crates/ordo-commandlog/src/value.rs:27-51`).
`Image` is `BTreeMap<String, Value>` (`:57`). `encode_image` writes a
`u64` count then each key and value (`chain.rs:275-281`); `encode_value`
writes the tag first (`:283`).

So an extra **map entry** is a lawful encoding of the frozen layout, and a
new **tag** is not. A digest therefore rides as a second field in the image
map. `DecodeError` has no version concept on purpose (`chain.rs:302-311`),
and an image with one more key needs none.

The naming convention for a platform-owned sibling key already exists:
`Field::index_column()` is `"{name}$bidx"` (`ordo-schema/src/lib.rs:142`),
and `$` cannot appear in a declared identifier. The digest key is
`"{field}$digest"`, by the same rule.

### 0.7 Two consumers read image keys as field names [settled]

`undo::imaged_fields` (`crates/ordo-exec/src/undo.rs:1618`) walks every key
in an image, skips `id` and `rev` (`:1632`), and **refuses the whole revert
if a key is not a declared field** (`:1635-1640`,
`unrevertable_image`). `resolve_sealed` (`undo.rs:783-802`) resolves every
`SealedRef` in an image into `Plaintexts` keyed by the image key.

An unannounced `"scan$digest"` key would therefore make every revert of a
row with a blob column refuse. C5 changes both, and that is the whole of
the "adding it later is a migration" cost, paid once.

### 0.8 How triggers derive from groups [settled]

`EventKind::Converged` and `EventKind::Written`
(`crates/ordo-syntax/src/trigger.rs:94`, `:96`) are produced by
`ordo_exec::triggers::classify` (`crates/ordo-exec/src/triggers.rs:154`)
from `CommandKind` alone, with `Written` on every command (`:161`). No
group means no event; a `Converge` command means `converged` and `written`
with no further wiring.

### 0.9 What `blobgc` assumes about images [settled]

The sweep's argument (`crates/ordo-exec/src/blobgc.rs:55-73`) is that "any
command image that could restore the handle is at or before"
`unreferenced_since`, because "an image is written when the reference is
written or removed, never after". **The first half of that sentence is
false today**: no image is written when a reference is written, because no
group is appended. The conclusion survives anyway, because an image written
*later* is what would break it and no such image exists. (c) makes the
sentence true as written, which strengthens the argument rather than
disturbing it.

The reference set is derived from live rows only
(`live_reference_clause`, `:169`), so nothing in the sweep reads an image
and nothing in it changes when images grow a key.

### 0.10 `blob sealed` is admitted by the door and modelled by nothing [open]

`tier_of` (`crates/ordo-host/src/blob.rs:123`) reads only `inline` and
`private`, so a field declared `blob sealed` (as `wide.ordo:137` declares
`contract`) passes the door. Three things then disagree:

- `ddl::column_ty` gives a sealed field `bytea`
  (`crates/ordo-schema/src/ddl.rs:89-92`), while `attach` writes the hex
  handle as a **text literal** for every non-inline field
  (`blob.rs:156-160`), so the column receives the ASCII of the hex string
  rather than a handle or an envelope.
- `row_image` then takes the sealed branch (`write.rs:2880`), `unhex`es
  those ASCII bytes and asks `envelope_epoch` for an epoch it does not
  have.
- `blobgc::live_reference_clause` (`blobgc.rs:169-183`) compares
  `r."contract" = encode(b."ref", 'hex')`, which is `bytea = text` and has
  no operator in Postgres, so a sweep over any schema declaring
  `blob sealed` refuses.

No test covers it: the door's test schema has `scan`, `photo` and `secret`
and no sealed tier (`crates/ordo-host/tests/blob_door.rs:91-98`). §3 has to
decide what a sealed blob column's image is, so this cannot be left
undecided. C3 decides it.

---

## 1 · The seam: a platform-bound value with no source expression

**[pinned]** The ruling: "a type only the blob door can mint, consumed by
value by the write protocol, with no public constructor and no
serialisation", closed the way `PinnedDestination` closes egress pin 2.

The egress precedent, read: `PinnedDestination`
(`crates/ordo-egress/src/dest.rs:249`) has private fields, no public
constructor, no `Clone`, and one producer, `resolve_and_pin` (`:320`),
which performs the obligation the type stands for. `deliver` consumes it by
value, so "re-checked at connect time" is the only way to get a second
delivery (`corpus-rulings.md`, "The pins are constructors and moves, not
checks"). Pin 5 is enforced by a test that reads a `Cargo.toml`
(`crates/ordo-egress/tests/holds_nothing.rs:53`), and pin 5's escape hatch
by a test that reads every manifest in the workspace (`:111`). Both halves
of that precedent are used below: type shape where the crate graph allows
it, and a source-level scan where it does not.

### 1.1 The shape [proposed]

Three types in a one-way sequence, one of which already exists:

```text
bytes --BlobPipeline::put--> Stored --PinnedUpload::mint--> PinnedUpload
                                                              |
                                             consumed by value by the
                                             platform converge entry point
```

- **`Stored`** (`crates/ordo-blob/src/pipeline.rs:134`) is what the sealing
  pipeline returns: handle, epoch, sealed `mime`, sealed `size`, sealed
  `integrity`, ciphertext length. Today its fields are `pub` and anyone can
  build one. C4 makes them private with accessors and leaves
  `BlobPipeline::put` (`:171`) as the only constructor, so a `Stored` is
  evidence that bytes were sealed and stored.
- **`PinnedUpload`** is a new type, **private to `ordo_host::blob`**. Its
  only constructor is a non-`pub` function in that module, called from one
  place inside `PlatformBlobs::write` after `put` returns. It consumes the
  `Stored` by value and adds the binding: space, entity, field, row id,
  created-at. It derives nothing: no `Clone`, no `Copy`, no `Default`, no
  `Debug`, no serde anywhere in the workspace to serialise it with.
- **It carries its own target.** The write protocol reads entity, field and
  row id **out of the token**, never from a parallel argument, so there is
  no call shape in which a token is applied to a row it was not minted for.
- **It carries its own reference statement.** `blobref::insert_sql`
  (`crates/ordo-crypto/src/blobref.rs:194`) is called at mint time and the
  string is held inside the token, so the `$blobref` row and the column
  write are one thing the protocol executes, and the door issues no SQL at
  all.

### 1.2 The crossing, and why it is a trait [proposed]

`PinnedUpload` lives in `ordo-host`; the write protocol lives in
`ordo-conformance`, three crates below it (§0.5). A concrete type cannot
travel downwards. The crossing is therefore a trait declared in
`ordo-conformance`:

```text
pub trait PlatformValue {
    fn target(&self) -> (&str, &str, &str);   // entity, field, row id
    fn column_literal(&self) -> String;       // the SET right-hand side
    fn image_value(&self) -> ordo_commandlog::Value;
    fn digest_envelope(&self) -> &[u8];       // the sealed integrity bytes
    fn reference_sql(&self) -> String;        // the $blobref INSERT
}
```

with the entry point taking it **by value and generically**:

```text
Fixture::run_platform_converge(db, log, value: impl PlatformValue, caller)
```

`impl PlatformValue` by value is the move: the token is consumed, and a
second converge needs a second upload. A private type implementing a public
trait is legal Rust as long as the type is never named in a public
signature, which is why the generic parameter matters.

**What this buys and what it does not.** It buys the whole of the type-level
pin on the concrete value: no public constructor, no clone, no
serialisation, one-shot by move, self-naming target. It does not buy
"nothing else can implement the trait": a sealed trait would exclude
`ordo-host` too, so the trait is implementable in principle by any crate
above `ordo-conformance`. That residue is closed the way the corpus already
closes this class of thing, with a source-level test (§1.3), and it is the
subject of the first open question in §7.

### 1.3 The tests that pin each property [proposed]

- `pin_upload_is_move_only` (in `ordo-host`): a compile-time assertion that
  `PinnedUpload` implements neither `Clone` nor `Copy`, written as a
  generic function bounded on `Clone` that is never called and a
  `static_assertions`-free trait trick, or as a `trybuild` case if the
  workspace grows one. If neither is wanted, the same property is asserted
  by `the_seam_has_one_mint_site` below, which fails on any added
  `#[derive(Clone)]` line only indirectly; state the weaker guarantee
  rather than claim the stronger one.
- `the_seam_has_one_mint_site` (in `ordo-host`, reading source like
  `holds_nothing.rs` does): scan `crates/*/src/**/*.rs`, assert exactly one
  textual construction of `PinnedUpload` and that it is in
  `crates/ordo-host/src/blob.rs`.
- `the_platform_value_trait_has_one_implementor`: same scan, asserting one
  `impl PlatformValue for` in the workspace, with the failure message
  naming why a second one is the hazard (two callers deciding what a
  platform value is).
- `the_seam_is_not_serialisable`: scan asserts no `serde`, `Serialize`,
  `Deserialize`, `to_json` or `encode` mention in the token's module.
- `a_platform_converge_names_its_own_row`: behavioural. Mint a token for
  `(docs, scan, doc_1)`, hand it to the entry point, assert the group's one
  command names `doc_1` and that there is no argument by which another row
  could have been named.

### 1.4 `blob_parameter` is untouched, and this says why [pinned]

`Sema::check_content_params` (`crates/ordo-sema/src/lib.rs:192`) refuses
any op parameter whose type `is_content()`, with the reason recorded at
`:185-191`: a caller-supplied handle names an object the caller did not
upload. Nothing in §1 changes that, and the mechanism is the point: the
token is a Rust value with no textual form, no wire form and no parser
production. There is no `.ordo` source that can name one, so an op author
cannot ask for one and a request body cannot carry one.

`a_blob_parameter_is_still_refused` stays green unchanged, and C4 adds
`no_op_body_can_reach_the_platform_converge`: the entry point is not
called from `run_write_body`, `plan_write_stmt`, or anything reachable from
a parsed op, asserted by the same source scan.

---

## 2 · The upload as a converge

### 2.1 The shape of the group [proposed]

One command, `CommandKind::Converge`, on `(entity, row_id)`, in a group
whose actor is `Actor::User(principal)` and whose correlation is the
protocol's own `cg_{n}` (`write.rs:713-722`). The before-image is the full
row as it was, taken under the row lock; the after-image is the full row as
it is, taken after the mutation batch (`write.rs:690-711`). Both carry the
blob column, and after C5 both carry `"{field}$digest"` when the column is
non-null.

`rev` moves, because the platform converge emits the same
`SET …, "rev" = "rev" + 1` an ordinary addressed set emits
(`write.rs:1708-1716`). A caller holding a `rev` from before an upload
learns it is stale, which is the third consequence `plan-blob.md` §8.1
listed as missing.

`converged` and `written` fire with no new wiring, because `classify`
derives them from `CommandKind::Converge`
(`crates/ordo-exec/src/triggers.rs:154-161`).

### 2.2 The statements, in order [proposed]

Inside one transaction, from `Executor::write_attempt`'s machinery
(`crates/ordo-exec/src/lib.rs:883`), which brings SERIALIZABLE, the
`40001` retry loop, the invalidation enqueue and the in-transaction append:

1. `lock_row` on the target (`write.rs:2761`).
2. read guard, then write guard (`write.rs:1690-1695`).
3. the before-image (`write.rs:1706`, via the single builder of §3).
4. the `$blobref` `INSERT` from `PlatformValue::reference_sql`.
5. the `UPDATE` setting the column from `PlatformValue::column_literal`,
   plus `rev = rev + 1`.
6. the post-image guard obligation (`write.rs:649-686`).
7. the after-image, the group, the payload flush, the append, the commit.

Steps 4 and 5 are both in `mutation_sql` and run in that order, so the
reference exists before the column names it inside the same statement
batch, and the module doc's asymmetry at `blob.rs:10-21` is preserved
verbatim: a `$blobref` row with no column is collectable; a column with no
`$blobref` row is not survivable.

`SpaceAudit` (`write.rs:129-139`) audits both. The `$blobref` insert names
one table and carries `"$space"` with this space's literal
(`blobref.rs:206-218`), so `audit_scoped` (`write.rs:167`) passes it, and
the reference insert is confined by the same mechanism that confines the
row insert beside it, which is what `blobref.rs:82-87` says it was spelled
that way for.

### 2.3 The guard, and what happens to `require_visible` [proposed]

Today the door checks `require_visible(write=true)` inside its transaction
(`crates/ordo-host/src/blob.rs:249`), which runs `guard_filter_query`
(`crates/ordo-exec/src/blob.rs:88`). Under (c) the protocol's own check
applies, and it is strictly stronger in two ways: the row is locked first
(`write.rs:1687`), and the write guard is re-evaluated on the row as it now
is (`write.rs:681-685`), so an upload that moved a row out of the caller's
write view is refused and rolled back.

`require_visible` **survives, for reads**: the read path calls it with
`write=false` (`crates/ordo-host/src/blob.rs:340-348`) and there is no
write protocol involved in serving bytes. Its `write: bool` parameter loses
its only `true` caller, so C4 removes the parameter and the dead branch
rather than leaving a second write-guard implementation alive with no
caller. `guard_filter_query` keeps three callers (the write protocol,
`undo::require_writable`, and the blob read), which is the arrangement the
explainer describes.

### 2.4 The stored object when the converge refuses [settled, unchanged]

Unchanged from today, including its defect. The bytes are already in the
store when the converge runs, because the pipeline stores before the
transaction opens (`crates/ordo-host/src/blob.rs:225-247`), for the
asymmetry argued at `blob.rs:10-21`. A refusal rolls the transaction back,
the object is unreferenced, and per §0.2 no sweep can see it because its
`$blobref` row rolled back too. §5 records it; C-none fixes it.

### 2.5 What the door still does [proposed]

Refuse `private` and `inline` by name, choose the tier from the schema,
check the inline cap, run the pipeline, mint the token, hand it to the
executor, map refusals onto the existing `ApiError` inventory
(`crates/ordo-host/src/blob.rs:106-116`). It issues no SQL and builds no
image. `ordo_exec::blob::attach` is deleted, and with it the only entity-row
`UPDATE` outside the write protocol. That deletion is the invariant the
ruling chose (c) for, and `only_the_write_protocol_updates_entity_rows` is
the test that states it: a source scan for `UPDATE {` / `INSERT INTO {`
against a schema-declared entity table outside
`crates/ordo-conformance/src/write.rs` and the revert applier.

---

## 3 · One image builder

### 3.1 Where it lives, and the argument [proposed]

**`ordo-schema`, as a new `image` module**, with the I/O left at each
caller.

The tempting homes both fail. `ordo-conformance` cannot own it and be
called from `ordo-exec`'s undo, because `ordo-exec` already depends on
`ordo-conformance` and undo would be reaching sideways into a harness
crate for a chain-critical routine. Inverting the dependency, which
`plan-blob.md` §8.1 and the explainer both name as (a)'s cost, means moving
3,633 lines of write protocol, which is not this build.

What actually differs between the two builders is not the decoding rules
(they are identical, §0.4) but the **I/O and the sealing sink**: one is
sync over `WriteDb`, one is async over `Db`; one stages payloads, one
writes them. So the shared part is everything that is not I/O:

```text
// ordo-schema, module `image`
pub fn image_columns(e: &Entity) -> Vec<&Field>;
pub fn image_select(entity: &str, id: &str, space: &str) -> String;
pub fn image_of_row(
    e: &Entity,
    row: &[Option<String>],
    sink: &mut dyn SealSink,
) -> Result<Image, ImageError>;

pub trait SealSink {
    fn seal_ref(&mut self, entity: &str, field: &str, envelope: &[u8])
        -> Result<Value, ImageError>;
}
```

`ordo-schema` depends only on `ordo-syntax` today, so it gains
`ordo-commandlog` (for `Image` and `Value`), which is a pure crate with two
dependencies (`ed25519-dalek`, `sha2`) and no tokio. The wasm build
survives, which is the constraint §0.5 established. `ordo-crypto` is not
needed: `unhex` moves nothing, because the caller hands the sink the raw
cell and the sink does the crypto.

Two callers implement `SealSink`: the write protocol's stages into
`log.payloads` (`write.rs:2835`), the revert applier's inserts immediately
(`undo.rs:1525`). The rule they implement stays one rule, the way
`ordo_crypto::sealed` already holds the rule both payload writers obey.

**The space predicate is inside `image_select`**, so undo's omission
(§0.4) is fixed by construction rather than by remembering, and there is no
signature by which a caller can build the select without naming a space.

### 3.2 The callers after consolidation [proposed]

The write protocol (before-images at `write.rs:1706` and the delete path,
after-images at `:695`), and the revert applier (`undo.rs:1396`, `:1144`,
`:1199`). Nothing else: the blob door builds no image, because it does not
write rows.

### 3.3 The tests that let the old builders be deleted [proposed]

- `both_builders_agree_on_every_corpus_row` (temporary, C2 only): for every
  entity in `market.ordo` and `wide.ordo`, for every seeded row, build the
  image with the old conformance builder, the old undo builder and the new
  shared one, and assert all three encode to identical bytes through
  `ordo_commandlog::chain::encode_image_bytes` (`chain.rs:269`), with
  sealed handles normalised (they are random per call by design,
  `write.rs:2806-2817`, so the comparison is over shape, key set, tags and
  non-sealed values, and the test says so).
- `an_image_is_this_space_s_row`: seed two spaces holding the same minted
  id, image through the shared builder, assert the space asked for is the
  space imaged. This is the regression test for §0.4's defect and it fails
  against undo's builder today.
- `every_declared_field_is_imaged`: the key set of an image equals
  `e.fields`, asserted from the schema, so a future field kind that the
  decoder forgets fails loudly instead of shortening an image.
- The existing revert and chain suites stay green unchanged, which is the
  real evidence: `crates/ordo-exec/tests/commandlog.rs`,
  `crates/ordo-conformance/tests/corpus*.rs`.

Only after those pass do `write.rs:2852` and `undo.rs:1423` get deleted.

---

## 4 · The digest in the image, and verification

### 4.1 The field and its encoding [proposed]

Key `"{field}$digest"`, value `Value::SealedRef(handle)` into
`$sealedpayload`, holding the **verbatim `$blobref.integrity` envelope**
for the handle in the column.

Verbatim, not re-sealed: the envelope is already AAD-bound to
`(entity, field)` (`crates/ordo-blob/src/pipeline.rs:243`), and copying it
is exactly what `row_image` already does for a sealed column ("the image
names these exact bytes", `write.rs:2881-2883`). The payload row records
`entity` and `field`, and `sealed::resolve` opens using the row's own
values (`crates/ordo-exec/src/sealed.rs:114-121`), so the digest resolves
with no new AAD convention and no new key use beyond the handle mint.

The image builder therefore does one extra read per non-null external blob
column: `blobref::lookup_sql` (`crates/ordo-crypto/src/blobref.rs:226`),
which `ordo-schema` cannot issue, so the lookup is the caller's and its
result is passed into `image_of_row`. That keeps the shared builder free of
I/O and keeps the digest a property the caller had to fetch on purpose.

**Plaintext is refused.** A hex digest as `Value::Text` would make the
plaintext command log an unsalted permanent equality index over file
content, answering "did these two rows ever hold the same file" forever and
surviving crypto-erasure. That is the exact ruling recorded at
`crates/ordo-commandlog/src/value.rs:9-21` for sealed values, and a file
digest is a stronger case of it.

### 4.2 Why every chain hash with a blob column moves, and why now [pinned]

The image is part of what the chain hashes (`chain.rs:275-281`), so any
group whose images carry a blob column hashes differently after C5 than
before. Existing chains are not touched: a group already appended keeps the
bytes it was appended with, and verification recomputes over those bytes.
What changes is that groups written after C5 carry one more key.

**Correction to `plan-blob.md` §8.2.** It says adding the digest at 8.1's
landing is free "because 8.1 is the commit that first writes a blob image
at all". That is not true, and §8.1's own "what is NOT broken" paragraph
says why: a delete's before-image already carries every column including
the blob handle, imaged as `Value::Text` through `write.rs:2897-2904`. So
images with a blob column and no digest already exist in every deployed
chain. Landing the digest now is still much cheaper than later (it is one
encoding change instead of two, and every *new* image has one), but the
build must carry a fourth revert-time case for the images that predate it.
§4.3 carries it and §7 asks the owner to confirm it.

### 4.3 Verification at revert time [pinned]

Verification is comparing the chain's recorded digest against the object
that exists. Mechanically, inside `undo::apply` before any write, beside
the existing sealed resolution that already refuses first
(`undo.rs:1124-1131`, the posture at `:783`):

1. resolve `"{field}$digest"` from the image to its 32 plaintext bytes
   (the same `resolve_sealed` pass, once it stops treating the key as a
   field);
2. read `$blobref` for the handle in the image's column, to get the sealed
   `size`, and open it for the plaintext length;
3. compare the opened plaintext length against the length the store
   reports, before transferring a byte. A disagreement refuses at zero
   cost. An agreement proves nothing, so step 4 still runs. Cheap to fail
   and full price to pass, which is the asymmetry that makes the check
   worth having;
4. fetch and hash the object, as `BlobPipeline::verify`
   (`crates/ordo-blob/src/pipeline.rs:445-477`) does, and compare.

**`verify` must stream first.** As written it calls `get(..)` and holds the
whole object in memory before hashing it, which contradicts the promise the
crate pins in `a_hundred_megabyte_upload_never_holds_the_object`
(`crates/ordo-blob/src/lib.rs:15`). Hashing as chunks arrive is constant
memory, and it is the real cost of verification. Fixed in C6 before
anything is wired to it.

Four outcomes:

- **Mismatch** (the object is there and hashes differently): **refuse**,
  before any write, class `blob_integrity`. Proof of corruption or
  tampering, and consistent with undo's refuse-before-write posture.
  [pinned]
- **Absent or unreachable** (no object, no `$blobref` row, or the store
  does not answer): **restore the handle and emit a security event**.
  Nothing was compared, so nothing was disproved, and the undo horizon must
  never depend on object-store availability. [pinned]
- **Gone because the undo horizon passed**: the horizon's existing refusal,
  which is `prune::Floor` (`crates/ordo-exec/src/prune.rs:445`) and the
  sweep's cutoff (`blobgc.rs:246-266`), unchanged. [pinned]
- **No recorded digest** (an image written before C5): restore the handle,
  verify nothing, emit **no** security event, and record the skip on the
  ops log (§4.4). Absence of a digest in an old image is a fact about the
  encoding's age and not evidence about an object. Ruled 2026-09-09, with
  the ops-log condition the owner attached. [pinned]

**No size cap.** Verification always runs, and a revert of a blob converge
reads and hashes the whole object. Ruled 2026-09-09: an application that
does not want unbounded verification cost bounds the object size on the way
in, which is where a limit belongs, because it is the application that
knows what its files are for. Putting the limit at revert time instead
would buy a rare saving at the price of a fifth outcome, a record for the
skipped case, and a per-space policy home that does not exist. The audit
path keeps one behaviour: it verifies.

`revert` (`crates/ordo-exec/src/undo.rs:851`) grows two optional
parameters, `blobs: Option<&BlobPipeline>` and
`events: Option<&ordo_secevent::Recorder>`. `ordo-exec` already depends on
both crates. With `blobs` absent, verification is skipped and the absent
case is not reported, which is the CLI's current shape until C7 wires it.

### 4.4 The security event [proposed]

`EventKind::CustodyAction { action: "blob_object_absent" }`
(`crates/ordo-secevent/src/event.rs:404`, labelled at `:470`), observed
through `Recorder::observe` (`crates/ordo-secevent/src/recorder.rs:328`)
with `Subject::member(principal)`, the op name of the revert, and the
revert's correlation. The precedent for using `CustodyAction` for a
platform act that writes no entity row is `bucket_ops.rs:206-232`, whose
comment argues exactly this reinterpretation.

**No new `EventKind` variant**, under the standing rule that a new class is
an oracle until argued. The action string carries the distinction, and the
event stream's own vocabulary stays at fourteen labels.

**The one skip case goes to the ops log, not here.** An image with no
recorded digest restores without comparing anything, and that is not
evidence that something is wrong. It is still worth being able to find
later, which is the ops log's job. One line, naming the correlation, the
entity, the field, the handle, and that the image carried no digest so
nothing was compared. The distinction the owner drew on 2026-09-09 is the
one the two streams already encode: the security stream is for what means
something is wrong, and a revert that had nothing to compare is not that.

The security event stays for the two cases that are evidence: a digest that
does not match, and an object that is not there.

### 4.5 `ordo verify` grows an object mode [proposed]

`ordo verify --space <spc> --objects`: walk the space's chain, and for
every image carrying a `"{field}$digest"`, compare against the object that
exists, reporting one of the four outcomes per handle and exiting 3 on any
mismatch. It is the offline form of §4.3 and it answers the owner's
definition of verification for a whole space rather than for one revert.
The existing modes are `--from-genesis` and `--proof`
(`crates/ordo-cli/src/operator.rs:284-298`), and this is a third,
independent of both: chain integrity says the record is intact, and this
says the world still matches it.

---

## 5 · What changes for undo, GC, export, and the corpus

> **Topology note, 2026-09-12.** Per-space database routing
> (`plan-space-routing.md`) places `$blobref`, `$blobtomb` and
> `$sealedpayload` in the **tenant's** database beside the chain that
> references them, so the blob collector runs against a space's own pool.
> Nothing in this plan changes: the sweep was already per space, and the
> reference set was already derived from live rows in one space.


- **Undo, image building**: changes. One builder, and its select is
  space-scoped (§3.1), which fixes §0.4's defect.
- **Undo, image consumption**: changes. `imaged_fields` (`undo.rs:1618`)
  skips `"{field}$digest"` the way it skips `id` and `rev`, and
  `insert_columns` (`undo.rs:1565`) with it; `resolve_sealed`
  (`undo.rs:783`) keeps resolving it, and the resolved bytes feed §4.3
  instead of a column. Without this, every revert of a row with a blob
  column refuses with `unrevertable_image`.
- **Undo, unscoped row SQL**: changed in C1, and in seven statements
  rather than three, for §0.4's reason.
- **Blob GC**: unchanged, because the reference set is derived from live
  rows only (`blobgc.rs:169`) and the floor argument (`:55-73`) becomes
  true as written rather than true by luck (§0.9). The sweep's tests stay
  green; one is added asserting that an upload's own group does not change
  what phase 1 marks.
- **Blob GC's blind spot**: unchanged, and still a leak. An upload whose
  transaction rolled back leaves an object no query can name (§0.2).
  Recommended trigger for fixing it: the first deployment that mounts a
  real store and reports bucket cost. The fix is a `list` on `BlobStore`
  plus a reconciliation pass, and it is not this build.
- **Export**: unchanged, because `image_json`
  (`crates/ordo-exec/src/export.rs:1181`) iterates the image map and
  renders a `SealedRef` as `{"sealedRef": "…"}` for any key. Golden bundles
  taken over a fixture with a blob column move by one key per image and are
  regenerated in C5.
- **The conformance corpus**: unchanged in every case, because no corpus op
  writes a blob column (there is no such op, since `blob` is not a
  parameter type). `wide.ordo` gains no new declaration. Two new cases are
  added in C4 for the platform converge, driven through the executor rather
  than through a call string.
- **The chain encoding**: unchanged. One more map entry, no new tag
  (§0.6).
- **`blob_parameter` and the language**: unchanged (§1.4).
- **The door's refusal inventory**: unchanged. Every new refusal maps onto
  an existing `ApiError` class (`crates/ordo-host/src/blob.rs:106-116`);
  the guard refusal now arrives from the write protocol as an
  `access_guard`-classed string, which is the class it already had.

---

## 6 · Increments

Each is green on its own. C1 to C3 are prerequisites that stand alone as
bug fixes; C4 is the ruling; C5 and C6 are §8.2.

**C1 · The unscoped row SQL in undo.** *Done, 2026-09-08.* A private
`space_pred(space)` in `undo.rs`, mirroring the write path's
`Fixture::space_pred`, on all seven unscoped statements (§0.4 item 1):
`row_image`, `row_exists`, the referrer scan, the converge `UPDATE`, the
delete branch's `DELETE`, `lock_row`, and `require_writable`'s unguarded
branch. Five tests in `crates/ordo-exec/tests/undo_space.rs`, each run
against the unscoped code first and seen to fail there.
`require_writable`'s branch has no test and needs none: every call site in
`apply` is reached only after a scoped `row_image` or a scoped `INSERT`
has already proven the row is in this space, so the check cannot observe
its own scoping. The test file's module doc records that argument.
*Size: small, as estimated.*

**C2 · One image builder.** *Done, 2026-09-09.* `ordo_schema::image` with
`image_columns`, `image_select`, `image_of_row` and `SealSink`; both
callers converted; both old builders deleted. The gate ran as specified:
old conformance, old undo and the new builder compared byte for byte
through `encode_image_bytes` over every seeded row of `market.ordo` and
`wide.ordo`, 23 entities with data and 102 rows, no disagreement anywhere,
sealed handles normalised to a constant because they are minted random per
call. That test was deleted with the builders it licensed removing.
`every_declared_field_is_imaged` stays. `scripts/build-wasm.sh` passes,
which is what made `ordo-commandlog` a legal dependency for `ordo-schema`.
`image_select` carries the space predicate, so C1's fix is now structural.
*Size: a day, as estimated.*

Two asymmetries closed in passing, neither predicted by §0.4's list of
five differences. Conformance NFC-normalised `id` and space literals
before quoting (`write.rs`'s `sql_str`) and undo's `sql_lit` did not,
which never produced a disagreement because both values are minted ASCII;
the shared select normalises for both callers. And an int column holding
unparseable text refused with two different messages; it now refuses with
one. Decode failures keep undo's `Store` class rather than becoming
refusals, because `Refused` carries a closed vocabulary and a new class in
it is an oracle until argued.

**C3 · The `blob sealed` tier, decided.** *Done, 2026-09-09.* Refused at
the door by name, `invalid_param`, the third of a pattern beside `inline`
and `private`, with the reason stated. Sealed blob fields leave
`live_reference_clause`, and `blob_fields` returns a `BlobField` struct
rather than a wider tuple now that the caller tests two flags. Refused on
the read path too: decryption there is mechanically possible, unlike
`private`'s, so the argument is the exclusion above, which stays sound
only while no sealed column can hold a live handle. A route that served
one would report as readable an object the sweep is free to delete.
`a_sealed_blob_field_is_refused_by_name`,
`a_sealed_blob_field_is_also_refused_on_read` and
`a_sweep_over_a_sealed_blob_schema_does_not_error` pass. *Size: small, as
estimated.*

The pre-fix failure, worth recording because it is the defect itself:
`ERROR: operator does not exist: bytea = text`. That is a plan-time type
error, so it fires with zero rows in the table, which is why §0.10's
"errors" is exact rather than data-dependent.

Found while building, and not predicted by §0.10: `blobgc`'s own unit
test `the_reference_set_comes_from_the_schema` declared a `blob sealed`
field and asserted it **was** included in the live-reference clause,
encoding the defect as expected behaviour. It never caught the bug
because it exercises the clause as a string builder and never against
Postgres, so the type error could not fire. Its assertions are corrected.

**C4 · The upload is a converge.** *Done, 2026-09-09.* `Stored`'s fields
are private with `put` the only constructor and `Clone` dropped;
`PinnedUpload` is private to the door, derives nothing, has one non-`pub`
constructor and carries its own target and its own `$blobref` INSERT; it
crosses on `PlatformValue` and is consumed by value.
`Fixture::run_platform_converge` and `Executor::attach_write` exist,
`ordo_exec::blob::attach` is deleted, and `require_visible` lost its write
branch. `Executor::write_attempt`'s body became a generic `attempt()`
shared by both entry points. All nine gating tests pass, plus
`pin_upload_is_move_only` (a compile-time probe) and
`the_seam_is_not_serialisable` (grounded in the manifest: `ordo-host`
depends on `serde_json` and never `serde`, so a `Serialize` derive does
not compile there). `image_value` gained a consumer here rather than in
C5: the protocol re-images the row after the mutation and refuses if the
column does not hold the object the token stood for. *Size: more than a
day, as estimated.*

**Five places this plan was wrong about the code**, found while building
and corrected here rather than in the commit alone.

1. **The refusal class changes on the wire, and §5 said it would not.**
   §5 claimed the guard refusal "arrives from the write protocol as an
   `access_guard`-classed string, which is the class it already had". It
   does not for the common case. The protocol evaluates the read guard
   before the write guard, because that order **is** the disclosure rule,
   so a caller who cannot see the row now gets `not_found` where the old
   door gave `access_guard`. A caller who can read but not write still
   gets `access_guard`. This is a fix, since the old door told a caller
   who cannot read `doc_1` that `doc_1` exists, which is the enumeration
   answer the door refuses everywhere else. It is also visible to
   clients, so it is recorded rather than absorbed.
2. **The `40001` retry loop cannot apply (§2.2 listed it as one of four
   things the machinery brings; it brings three).** The retry boundary is
   the whole group re-executed, and re-executing means calling the
   protocol a second time, which the move forbids: the token is consumed
   and only another upload produces another. `attach_write` makes one
   attempt and surfaces a conflict as a retryable
   `PoolError::Serialization`, with nothing committed and nothing
   appended. Keeping a reusable copy inside `ordo-exec` would put "one
   upload, two converges" back inside the boundary that exists to forbid
   it. The cost is real and belongs in §5's leak entry: a conflicted
   upload is retried by the client, and the first object is leaked by
   §0.2's blind spot.
3. **§2.5's "the only entity-row `UPDATE` outside the write protocol" is
   not exact.** `crates/ordo-conformance/src/membership.rs` writes
   `members` and `groupMembers` outside `write.rs`, and
   `crates/ordo-exec/src/erase.rs` deletes entity rows for
   crypto-erasure. Neither is the same defect: membership images before
   and after and stages a group, and erasure is deliberately ungrouped
   because the chain it would append to is destroyed in the same
   statement. The exact claim is that **`attach` was the only entity-row
   write that appended nothing**, and the scan's allowance list carries
   the real inventory with a reason per entry.
4. **C4 cannot observe §2.2's reference-before-column order.** There is
   no foreign key from a blob column to `$blobref`, both statements
   commit atomically, and nothing between them reads the reference, so no
   test here distinguishes the two orders. It becomes load-bearing in
   **C5**, when the image builder looks up `$blobref.integrity` for the
   handle in the column and the reference must already exist when the
   after-image is taken. What C4 does preserve and does test is the outer
   asymmetry: bytes are in the store before the transaction opens, so a
   refusal leaks an object rather than leaving a dangling reference.
5. **`image_value` would have been dead in C4** as §1.2 described it,
   since both images come from `ordo_schema::image`. It has a consumer
   now, described above.

**One wiring hazard for R1, with no test possible yet.** `PlatformBlobs`
gained a `log: Arc<Mutex<WriteLog>>`, because an upload's group joins the
same per-space chain an op's group joins and the door had no log. Only
tests construct `PlatformBlobs` today, so nothing asserts the wiring. A
real deployment must hand the door the **same** `Arc` the executor holds:
a second `Arc` is a second chain for one space, which is the one thing a
chain cannot be. R3 carries the trigger wiring and should carry this.

**C5 · The digest rides the image.** *Done, 2026-09-09.* The
`"{field}$digest"` key, a `SealedRef` into `$sealedpayload` holding the
verbatim `$blobref.integrity` envelope, exactly as §4.1 specified.
`image_of_row` grew a `blob_digests: &BTreeMap<String, Vec<u8>>`
parameter, the caller's completed lookup, so the shared builder still has
no I/O. `is_external_blob` states in one place which tiers can have a
digest at all: not `sealed` (no `$blobref` row, and refused at the door
since C3), not `inline` (its bytes are a `$sealedpayload` row), not
`private`. `imaged_fields` skips the key, `resolve_sealed` needed no
change, and `blobref::parse_handle` is new. *Size: a day, as estimated.*

**§4.1's AAD claim holds, and the detail that makes it hold**:
`seal_ref` is called with the blob field's **real** name, never the
`$digest` map key, because the envelope is AAD-bound to
`(entity, field)` by the pipeline that sealed it and `sealed::resolve`
opens using the payload row's own stored pair. The suffix is a map key
and never an AAD input. A re-seal would have invented a convention where
none was needed.

**One source, decided while building.** `PlatformValue::digest_envelope`,
added in C4 for this increment, is **removed**. It exists only on the
upload path, so it could never answer for a delete's before-image or a
revert's images, which need the same digest. Two sources would be two
answers to "what is this column's digest", differing exactly when a bug
made them differ. Every image is now answered by the `$blobref` lookup
and none other. Removing it took `PinnedUpload.integrity` with it.

**The `imaged_fields` change was load-bearing, not defensive.** Before
it, every revert of a row with a blob column refused:

```
unrevertable_image: the logged image names `docs.scan$digest`,
which the loaded schema no longer declares
```

The skip is keyed on the `$digest` suffix and is sound because `$` opens
a parameter token in the lexer, so it cannot appear in a declared
identifier. The key names no column by construction rather than by
convention.

**A missing reference refuses.** A non-null external blob column whose
handle has no `$blobref` row refuses in both callers rather than imaging
without a digest. The only writer of such a column inserts the reference
first, so a live column with nothing behind it is corruption, and it is
distinguishable from §4.2's pre-C5 images, which carry no such column
value to fail the lookup with.

**Corrections to this section.** C4's finding 4 said the
reference-before-column order becomes observable in C5. It does not.
Both mutation statements complete before either image is read, whatever
their relative order, so no test distinguishes them. What is pinned
instead is the dependency the order exists to satisfy:
`a_column_naming_an_absent_blobref_row_refuses_the_image` deletes the
`$blobref` row after an upload and shows the revert refusing by name.
And **no export golden changed**: `export.rs` declares no schema with a
blob field, so nothing there gained a key.

**C6 · Verification at revert, the event, and the ops-log skip.** *Done,
2026-09-10.* `BlobPipeline::verify` streams (exact framing, so the live
buffer is one sealed chunk plus one plaintext chunk at any object size);
the length fast-fail runs before any transfer; `revert` takes
`blobs: Option<&BlobPipeline>` and `events: Option<&Recorder>`; the four
outcomes are decided in `verify_restored_objects`. All seven named tests
pass, plus two that earned their place:
`an_object_that_will_not_open_refuses_the_revert_as_integrity` and
`a_length_that_agrees_does_not_short_circuit_the_hash`. *Size: a day plus
the streaming fix, as estimated.*

**Three conditions §4.3 does not name, ruled while building.**

1. **An object that is present and fails its AEAD tag** refuses as
   `blob_integrity`. Custody proved two statements earlier that it holds
   what this reference row was sealed under, so a chunk that will not open
   is about the bytes, and it is the same evidence a digest mismatch is
   arriving through the tag instead of the hash. The residual:
   `PipelineError::Custody` deliberately does not say *why*
   (`pipeline.rs:75-78`, "`KeyError` is uninformative about why on
   purpose"), so a lease failure would land here too and read as
   tampering. It cannot happen with the same provider, the refusal quotes
   the underlying error rather than flatly asserting tampering, and the
   direction is fail-safe: it refuses rather than restoring quietly.
2. **A `$blobref` row whose sealed `size` will not open**, or whose size
   does not decode, goes in the absent bucket: restore plus event,
   because nothing was compared.
3. **Undoing a create is not verified.** It restores no handle and makes
   no claim about any object.

**Corrections to this section.** §4.3 step 1's parenthetical, "once it
stops treating the key as a field", is stale: C5 left `resolve_sealed`
iterating the whole image, so `plain["{field}$digest"]` is already the 32
plaintext bytes. And the streaming assertion is a measurement rather than
a hope: two doubles either side of the pipeline share a tally, one
counting bytes handed to the pipeline and one counting bytes handed back
to `open_chunk`, and the peak difference is what the code holds. It was
verified to bite by reinstating the buffering shape, which failed with
`verification held 104817669 bytes of a 104857600-byte object at once`.

**The skip record has no durable home.** It rides
`Reverted.unverified: Vec<UnverifiedObject>`, with
`UnverifiedObject::ops_log_line()` as the named seam rendering §4.4's
line verbatim. Nothing durable holds it: there is no ops log, and none
was invented. See `plan-opslog.md` §0.9 and §3.3, which recommends the
record move to a command-group trailer instead and asks the owner. Until
that is decided, the owner's condition of 2026-09-09 is **half met**: the
restore is audited by the revert's own group, and the fact that a
verifier ran and had nothing to compare is not durably recorded. The
absent-object half is fully met, because the security stream holds it.

**C7 · `ordo verify --objects`.** *Done, 2026-09-21.*
`undo::verify_objects` walks the whole stored chain through the existing
`window` primitive and compares every recorded digest against the object
the store holds, reusing C6's `verify_restored_objects` as the comparison
engine rather than adding a second one. One `Purpose::Read` lease for the
whole walk, on `export.rs`'s economy. It stops at the first `Err`, and
because the walk is in chain order that first error *is* the first
mismatching handle, which is what the required test asserts: it tampers
**both** objects and proves the report names the first and not the
second, so a walk that merely found *a* mismatch would fail it.
`Custody::release` grew a `Purpose` parameter (it had `Write` hardcoded)
and both revert call sites pass `Write` explicitly. Exit 3 on any
mismatch. *Size: more than small, for the three reasons below.*

**Three gaps this increment found in §4.3 and §4.5**, each a place where
the plan described revert-time behaviour and did not notice that an
offline walk cannot inherit it.

1. **§4.3's four outcomes do not cover "a digest is recorded and will not
   open", and for this tool that case is real.** At revert time it cannot
   arise, because `resolve_sealed` hard-refuses the whole image before
   `verify_restored_objects` is reached. An audit walk that hard-stopped
   over one unrelated sealed column on some other row, or over a digest
   whose epoch was crypto-erased, would be an audit tool that stops
   auditing at the first erased tenant. It is bucketed under
   `Unverified::ObjectAbsent` on the same ground the store-unreachable
   case sits there — nothing was compared — rather than by minting a
   fifth outcome, and `verify_restored_objects` now tests for the digest
   key's presence against the **raw image** instead of the resolved
   plaintext so that "never recorded" and "recorded and unreadable" stay
   distinguishable.
2. **§4.5 never says whether the walk checks `before` images, `after`
   images or both.** A revert only ever checks the image it is about to
   write, which is the only thing it acts on. An audit has no such
   restriction, so this checks both. **The undocumented cost that
   follows:** a handle that is one command's `after` and the next
   command's `before` on the same row is verified twice, so the walk can
   read an object more than once. Consistent with §7.3's *"no cap, it
   verifies"*, and stated here because a cost multiplier nobody wrote
   down is a cost somebody discovers on a large chain.
3. **§4.5 does not say the mode needs a program argument and real key
   custody**, unlike the other two `verify` modes, which need neither.
   Both follow from decisions already made: the digest is sealed (§4.1
   refuses a plaintext one), and only the schema says which columns are
   blob columns. The signal that this was C7's scope was in the code
   rather than the plan, in `revert`'s own comment that *"`ordo verify
   --objects` (C7) is where this tool grows the ability"*.

**C8 · The record.** *Done, 2026-09-21.* `plan-blob.md` §8.1/§8.2 carry
`[pinned]`, 17g′ is checked off with what each increment found, and the
three gaps above are recorded here rather than in a commit message. The
blob chain is built: an upload is a converge, the digest rides the image,
a revert verifies before it writes, and the chain can be walked against
the objects it names.

---

## 7 · Questions for the owner

All four are answered, and none is left open. Owner's answers of
2026-09-09, recorded in `corpus-rulings.md`.

**7.1 The seam cannot literally have "no public constructor" on both ends.**
The ruling's words are met by the concrete token (private type, private
constructor, no clone, no serde, consumed by value) and not by the trait it
crosses on, which any crate above `ordo-conformance` could implement
(§1.2). **Answered: take the recommendation.** The trait crossing, pinned
by a one-implementor source test in the manner of `PinnedDestination`.
Inverting the crate dependency stays available later as a move rather than
a redesign.

**7.2 Images that predate the digest.** Images written before C5 carry a
blob handle and no digest, and they exist today (§4.2). **Answered: restore
without verification and without a security event, and log that it
happened.** The owner accepted the recommendation and attached the
condition, drawing the line at a security event himself on the ground that
it would be noise. §4.4 places the record on the ops log.

**7.3 Verification cost at revert.** Verifying reads and hashes the whole
object (§4.3). **Answered: no cap. The application bounds object size on
the way in.** That is where a limit belongs, because the application knows
what its files are for, and it leaves the audit path with one behaviour
instead of a fifth outcome, a record for the skipped case, and a per-space
policy home that does not exist.

The owner also asked whether the object could carry a sealed digest of
itself, so that verification reads only its surface. It cannot be the
witness, and the reason is worth keeping next to the design. A digest
inside the object was written by whoever wrote the object, in the same act,
under the same key, so it agrees with its own body by construction.
Against an attacker holding no keys it adds nothing, because the streaming
AEAD already refuses every chunk they could mint. Against a key holder,
which is the threat an audit chain exists for, the false body and the false
header are written together. In both cases the object verifies itself. The
chain is the only independent witness, and reaching it means the bytes.
Stopping the stream early works only if the digest covers a prefix, which
turns proof of the object into proof of its first N bytes. What the
exchange did produce is the length fast-fail and the streaming fix in §4.3.

**7.4 The `blob sealed` tier.** It is declarable, admitted by the door, and
modelled coherently by nothing (§0.10). **Answered: take the
recommendation.** Refuse it at the door by name in C3, as `inline` and
`private` already are, and leave the declaration legal so a later build
serves it without a syntax change. The refusal states the reason it is not
merely unfinished: the collector matches live references by comparing the
column to the handle and holds no keys on purpose, and sealed handles carry
a fresh nonce, so equal handles never produce equal envelopes. Serving the
tier needs a keyed collector or a deterministic blind index derived from
the handle.
