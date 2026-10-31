# How to Use Sealed Fields and Key Custody

Sealing stores sensitive data encrypted at rest; the platform holds ciphertext in the database while the keys stay separate. The trade-off is immediate: writes are slower, certain queries become impossible, but unauthorized database access reveals nothing of the sealed values.

A tenant's own database, its own bucket, the deployment's master key and the credentials behind them are in [howto-byo-custody.md](howto-byo-custody.md).

## Sealed Fields

A sealed field lives in the per-space encrypted payload instead of in a plaintext column. Declare one with the `sealed` keyword:

```ordo
entity customers {
  name:  text
  email: text?  sealed
}
```

The database stores ciphertext; Ordo decrypts it in the host process when a read needs the plaintext. What this means in practice:

**On write:** the plaintext is encrypted under the space's current data encryption key (DEK), wrapped in an envelope tagged with the epoch it used, and the envelope bytes are stored in a dedicated sealed-payload table. The command image carries a 16-byte opaque handle instead of the value itself, so the log never holds plaintext.

**On read:** the API opens the payload and returns the plaintext to the caller. A read that returns a sealed field is a `bounded-host` read. Postgres returns the page's rows as stored, with the guard deciding which rows. The host then takes one read key release for the page and opens that page's envelopes, each at the row it was sealed for. A page costs one release, recorded as one `KeyReleased` security event in the tenant's stream. A page with nothing to open releases nothing, and an anonymous caller never opens a sealed value. A query returning 10,000 sealed values is not faster than returning 100.

**What a read cannot do:** a read whose host work grows with the data (`unbounded-host`) is refused with `sealed_unsupported`, naming the sealed field and the fix. That is a many-side join, a `distinct` or a group over the opened rows, with no `take`. A read answered whole, with no page, needs a bound the plan proves, such as `take n` or a lookup by id.

**What stays impossible:** you cannot filter, sort, or aggregate on a sealed field without pulling it into the host and working through all the data. A filter on an unindexed sealed field, such as `orders(memo contains "urgent")`, is refused today with `sealed_unsupported` (host-side filtering, F0, is not built); an `indexed` sealed field supports equality lookup. `orders[memo] sort memo` refuses to compile: sealed sort keys are prohibited because the index that would enable them would leak order information about the plaintext.

**What a seal binds:** every envelope is authenticated against the entity, the field and the thing it belongs to. For a column value that is the id of its row. For a stored file it is the object's handle, for the file's chunks and for its sealed `mime`, `size` and `integrity`. For a stored idempotent response it is the principal, op and key of the record. Somebody who holds the database and no key can therefore copy an envelope to another row, or swap two files in the object store, and the read fails instead of serving the wrong value. A chain image's payload keeps the label of the row it images. What this does not stop is deleting rows, restoring an older copy, or swapping two images of one row and one field with each other.

**What a ranged write costs:** `set orders(status == "done") { memo: "x" }` seals once per row it touches, because the row's id is part of the seal. It is one statement per row instead of one for the range. A ranged write to an open field is unchanged.

### Example from the Fixture

From `crates/ordo-conformance/fixtures/market.ordo`:

```ordo
entity orders {
  customer -> customers
  placed: date
  status: text = "open"
  total:  money
  memo:   text = ""   sealed
}

entity customers {
  name:  text
  email: text?  sealed indexed unique normalize(lower)
  tier:  text = "basic"
}
```

The `memo` field on orders is sealed. The `email` field on customers is sealed *and* indexed (a blind index, described below). Both are encrypted; neither is readable in the database itself.

---

## Blind Indexes

A blind index lets you do exact-match lookups on a sealed field at SQL speed, without ever exposing the plaintext to the database. Declare it by adding `indexed` to a sealed field:

```ordo
entity customers {
  email: text?  sealed indexed unique
}
```

What happens: when a write seals the email value, the system also computes an HMAC of it using a per-space, per-field key that never leaves the KeyProvider. That 32-byte digest goes into a separate `field$bidx` column in the database as a plaintext indexed value. An equality query like `customers(email == "alice@example.com")` hashes the search term with the same key and key, and the database finds it through the blind-index column at normal SQL speed.

**What blind indexes enable:**
- `==` and `in` comparisons at SQL speed with no key material touching the database
- `unique` constraints on sealed fields (the uniqueness check uses the blind-index digest)
- Converge-or-create writes addressing by a sealed key (e.g., upsert a customer by email)

**What they do not enable:**
- Range queries (`email > "a@"` refuses to compile)
- Substring search (`email contains "example"` is post-decrypt only)
- Sorting (`sort email` fails: order information is not recoverable from a hash)
- Any ordering at all

The digest leaks only equality patterns: two writes of the same email produce the same digest, which an attacker can observe. But without the key, they cannot reverse-engineer the plaintext, and different spaces use different keys, so digest values are not comparable across tenants.

### Null Handling

A nullable sealed indexed field stores a reserved sentinel value in the blind-index column when the field is null. This is the only way to make `sealed indexed unique` work for null values. Without it, Postgres would store SQL NULL in the index, and `UNIQUE NULL` constraints permit unlimited nulls.

From `crates/ordo-crypto/src/custody.rs`:

```rust
pub const NULL_SENTINEL: &[u8] = b"\xff\xffordo.blind.null.v1\xff\xff";
```

This sentinel is deliberately not valid UTF-8, so no text value can produce it. The result: a nullable unique sealed field admits at most one null row, exactly as the language spec requires, and queries like `customers(email == null)` hash the sentinel and find it through the index.

### Example from the Fixture

From `crates/ordo-conformance/fixtures/market.ordo`, the `upsertCustomer` operation:

```
op upsertCustomer($name: text, $email: text, $tier: text = "basic") =
  set customers { email: $email, name: $name, tier: $tier }
```

This is a converge-or-create write. It finds or creates a customer by matching the blind-index digest of the email. Because email is `sealed indexed unique`, the write addresses by the digest rather than by a primary key, so duplicate emails are structurally impossible.

---

## normalize(lower)

Apply normalization to a field to decide what "the same value" means for equality, uniqueness, and blind indexes. The only normalization currently available is `lower`, which lowercases the text and applies Unicode NFC normalization.

Declare it in the shape:

```
entity customers {
  email: text?  sealed indexed unique normalize(lower)
}
```

The normalization is applied *on write*. When you store an email, Ordo normalizes it to NFC form and lowercases it, then seals the normalized bytes. When you query with `email == "Alice@EXAMPLE.COM"`, the query engine normalizes your search term the same way, and the blind-index lookup matches.

**Why this matters:** without normalization, "alice@example.com", "ALICE@EXAMPLE.COM", and "alice@example.com" (with a non-ASCII lookalike) are three different sealed values with three different digests. For email addresses, you almost certainly want them treated as one. Without `normalize(lower)`, the unique constraint allows case variants, and searches miss half the data.

**What gets normalized:** the semantically-significant bytes that go into the sealed envelope and the blind-index digest. Skeleton fields (open text fields like `name`) are not affected by this declaration. They stay in their stored form.

**The cost:** normalization is extra work on every write. For most applications it is negligible; for bulk imports of millions of records it might matter. The alternative is a duplicate-cleaning routine after the fact.

---

## Key Rotation and Crypto-Erasure

Every space has a space master key (SMK) and a series of data encryption keys (DEKs), one per epoch. New writes seal under the current DEK; a value written earlier stays sealed under the epoch that wrote it until the re-seal pass moves it. Rotation is three operator acts, in this order: mint the next epoch and enqueue the pass (`rotate`), let the deployment's `keys.reseal` worker move every ciphertext off the old epoch, then destroy the old epoch's key once nothing is under it (`destroy`). A fourth act, `reseal`, is the forced re-encrypt you run after an exposure when the rotation already happened.

All four take the same custody flags as `ordo keys init`: `--space`, `--as`, `--db`, and `--master-key-file` with `--passphrase-file`. Without them the commands read `ORDO_MASTER_KEY`, and refuse to unless `ORDO_DEV_UNSAFE_TEST_KEYS=1` is set, because that key is stretched with one HMAC and is for development only.

### Rotate

```
ordo keys rotate --space acme --reason "laptop lost 2026-09-19" --as ops:ada \
  --master-key-file /etc/ordo/master.key --passphrase-file /run/secrets/master-pass
```

This mints the next DEK (random, wrapped under the SMK, never derived from it) and enqueues a re-seal pass for the epoch it replaces. The output prints the four counts the pass will move: `payloads` (the command images' sealed payloads in `$sealedpayload`), `columns` (every live sealed column), `blobrefs` (the sealed `mime`, `size` and `integrity` envelopes on blob references), and `objects` (stored blobs, which move to a new handle). The decision is recorded as a `custody-action` in the space's security-event stream.

Nothing else changes at that moment: every existing value still opens, because the old DEK is still there. Blind-index digests do not move at all, at rotation or during the pass; they are derived from the SMK, not from any epoch, so `sealed indexed unique` holds continuously.

### Watch the pass

The `keys.reseal` worker runs on every deployment, leader-only, and moves one chunk per tick (`ORDO_RESEAL_CHUNK` rows, default 256; `ORDO_RESEAL_PAUSE_MS` between chunks, default 250). It needs no write barrier: writes always seal under the newest epoch, and every update the pass makes is a compare-and-set on the old envelope bytes, so a row written while the pass runs is skipped rather than overwritten.

```
ordo keys status --space acme --as ops:ada ...
```

prints the current epoch, every live epoch with its four counts and its standing (`current`, `pass running on columns: advanced 3s ago`, `destroyable: re-sealed 2026-09-19`, or `retired, no pass`), and every pass row with its cursor, counts and attempts. The same answer is on the metrics endpoint: `ordo_reseal_rows_remaining{space,population}` and `ordo_reseal_stuck_seconds{space}`, the seconds since the cursor last advanced while rows remain. A pass whose stuck seconds keep rising while rows remain is failing its chunk; `status` shows the attempts, and `/health/deep` shows the worker's failures.

`blobgc` runs as the `blob.gc` worker whenever `ORDO_BLOB_STORE` is set, every `ORDO_BLOB_GC_SECS` seconds (default 3600). No command runs it on demand, so a shorter `ORDO_BLOB_GC_SECS` is the only lever to speed it up.

A pass is complete when every count for the old epoch is zero. Three populations reach zero by the pass alone. Objects are different: a stored blob is moved to a new handle and a new reference row, and the old row and object stay until `blobgc` collects them, which happens once no live row and no retained command image names the old handle. Until then the epoch's `objects` count is non-zero, `status` shows the pass `blocked`, and `destroy` refuses. That is the undo-horizon rule applied to objects, and it is the honest state: the old bytes are still reachable from the log's history. **What that costs, accepted by the owner on 2026-09-20 and disclosed here rather than buried:** because `destroy` refuses while objects remain, the retired DEK stays alive until `blobgc` collects them. A rotation does not end the old key's life by itself; the sweep does. If you rotated because a key may have been exposed, the exposure window closes when the sweep runs and `destroy` succeeds, so shorten `ORDO_BLOB_GC_SECS` (default 3600) rather than waiting for the default cadence, and watch `ordo keys status` until the epoch reports `destroyable`.

### Force a re-encrypt after an exposure

```
ordo keys reseal --space acme --as ops:ada ...
```

enqueues a pass for every retired epoch that still holds anything and has no open pass, targeted at the current epoch. Run it after `rotate` if a pass was lost, after `blobgc` has retired old object pairs to give a `blocked` pass another sweep, or on a deployment where an epoch was minted before this worker existed. It is idempotent: a second run enqueues nothing.

### Destroy an epoch

```
ordo keys destroy --space acme --epoch 3 --as ops:ada ...
```

counts all four populations under epoch 3 and refuses by name with the number if any is non-zero (`refused: 12 payloads still under epoch 3`), or refuses if 3 is the epoch new writes seal under. When the gate clears it prints the one consequence nothing can undo:

```
re-sealed 2026-09-19
consequence: backups and archived log bundles taken before 2026-09-19 hold this
epoch's ciphertext and will not open for sealed fields in this space once it is
destroyed. Destroy only after a backup taken since the re-seal completed.
to proceed: add `--confirm acme:3`
```

The token names the space and the epoch, so a command copied from another space refuses. With the token, the wrapped DEK is deleted, and every envelope still sealed under it anywhere (a captured copy, an old backup) can never be opened again: this is the post-compromise recovery the envelope design exists for, and the same property that makes losing key material unrecoverable. The destruction is recorded in the space's security-event stream.

### What the pass never does

It never writes a plaintext content hash anywhere: each row is verified in memory (open under the old epoch, seal under the new, open the result, compare, drop both) and the pass table keeps only a digest over `(handle, new envelope)`, which proves a chunk was processed and nothing about its contents. It never appends a command group, because it changes no row's value. It never touches a `$bidx` column. And it never rotates the SMK: that is a re-index of every blind index under a write barrier, a different and rarer operation that this tooling does not perform.

### A value that will not open

Three different things can stop a sealed value from opening, and Ordo tells them apart (owner, 2026-10-23):

| What happened | Answer | Retry? |
|---|---|---|
| The epoch key was deliberately erased (`destroy-epoch`) | the value is "destroyed" (revert refusal) | no |
| The key is present and the value fails to authenticate or decrypt | `sealed_unreadable` (500): unreadable, possibly tampered | no |
| The key store is busy or down | `server_busy`, or the store's own failure | busy: yes |

"Destroyed" means only a key someone erased on purpose. A value that fails with its key present is never called destroyed. The message names the entity and field. It never holds the value or key material.

**You are alerted.** Each unreadable open adds 1 to `ordo_sealed_unreadable_total{space,epoch}` and records a `sealed-unreadable` event (entity, field, epoch) on the space's security-event stream. The event is recorded at most once per entity, field and epoch every 5 minutes, so a corrupted table does not flood the stream. The counter counts every failure.

**Last successful decrypt.** `GET /health/deep` (deployment owner) has a `sealed_decrypts` section. For each space and key epoch it shows `last_success_at`, and, since that success, `failing_since` and `last_failure_at` (Unix seconds, `null` for never). It reads the answers of the key provider to opens of sealed values and blob chunks. It reads no value. A read updates memory only. Changed entries are written to the control database table `$decrypt_seen` at most once every `ORDO_DECRYPT_SEEN_SECS` (default 60) and at graceful shutdown, and are loaded at boot, so "worked until 14:02, failing since 14:03 after the restart" survives a restart (a wrong master key mounted after a restart looks exactly like this). A crash loses at most one interval. There is one row per space and epoch, not per node. No custody read op carries it today.

### Blind-Index Keys Are Not Rotated

Blind-index keys are SMK-derived and epoch-independent. They do not change when you rotate the DEK. This is deliberate: a DEK rotation must not touch any digest, or you would re-index the entire sealed indexed column and face duplicates across the rotation boundary (during exactly the revocation that forced rotation in the first place).

From `crates/ordo-crypto/src/custody.rs`:

```rust
fn blind_index(
    &self,
    space: &SpaceId,
    entity: &str,
    field: &str,
    value: BlindValue<'_>,
) -> Result<[u8; 32], KeyError> {
    // SMK-derived and epoch-independent, per the settled rule: a DEK
    // rotation must not move a single digest, or `sealed indexed
    // unique` admits duplicates across the boundary — during exactly
    // the revocations that force rotation.
    let smk = self.smk_of(space)?;
    let bik = hmac_sha256(&smk, format!("blind-index:{entity}:{field}").as_bytes());
    …
}
```

---

## The Sealed Payload Store

When you write a sealed field, the plaintext is encrypted and stored in a separate table called `$sealedpayload`. The command image in the log carries only a 16-byte random handle, never the ciphertext or plaintext.

### Why the Handle Is Random, Not Content-Derived

From `crates/ordo-crypto/src/custody.rs` (lines 256–322), the `mint_sealed_ref` method and its doc comment explain:

The handle is drawn from a keyed pseudorandom function seeded with fresh random bytes, using the space's key material as salt. The plaintext is **not** an input to the handle: two writes of the same plaintext produce two different handles.

This matters for revert semantics: if a write's handle collided with another write's handle, reverting one would erase the other's data from the payload store. By making handles collision-resistant through randomness and per-space keying, a collision inside one space becomes a primary-key violation that the database rejects (the honest answer), not a silent data loss.

### What Happens on Revert

From `crates/ordo-crypto/src/sealed.rs` (lines 45–70), the module doc explains:

Garbage collection of payloads happens in two cases:

1. **The command group is pruned.** When pruning removes a chain entry below a checkpoint, the handles in that image become unnameable. Collection is done in the same transaction as the entry delete, so a crash between the two does not leave orphaned payloads.

2. **The epoch key is destroyed.** The payload row stays in the table, but it is now unopenable. Deleting the row would only replace one unresolvable answer ("the key is gone") with a less honest one ("there was never a payload").

When you revert a write that sealed a value, the platform looks up the payload by its handle and opens it to produce the pre-image. If the payload cannot be produced (because the epoch was destroyed, or the handle was never stored), the revert is refused with a typed rejection. A key store that is busy or down is not read as a destroyed key: a full pool answers `server_busy` (try again), and any other store failure answers as that failure. The old data is safe from accidental erasure; you must destroy an epoch *deliberately* and *after* the undo horizon, or you will strand reverts.

### Example: Converge-or-Create with Sealed Fields

A converge (upsert) over a sealed indexed field stores the payload *twice*, once for the before-image, once for the after-image. From `crates/ordo-crypto/src/sealed.rs` (lines 38–42):

```
The cost is stated rather than hidden: a converge over a sealed field
stores the payload **twice** — once for the before-image, once for the
after-image — and a bulk write over N rows stores 2N. Sealed values are
small (they are fields, not blobs — a blob's bytes go to the object
store under `ordo_crypto::stream`, and only a `blob inline` payload lands
here, bounded by `BlobPolicy::inline_cap`), and the alternative is an
equality oracle nobody opted into.
```

This is a deliberate trade-off: the payload store does not deduplicate. If you update a customer's email twice in one day, you have two envelope rows and two handles in the log. The alternative would be a deduplicating store that would let an attacker tell whether two writes were equal just by looking at the handles, which would defeat the random handle. So the cost is paid once: every sealed write costs storage proportional to the size of the sealed values, and that cost is stated in your API artifact.

### What the Command Log Holds

The command image holds the handle, not the payload:

```
| image | {
|   ...
|   "memo$ref": [16 random bytes],
|   ...
| }
```

The payload table holds:

```
| $sealedpayload |
| $space | ref | epoch | entity | field | correlation | payload |
| tenant_a | [handle] | 3 | orders | memo | cmd_123 | [envelope] |
```

The envelope is the sealed ciphertext: `[epoch tag][nonce][ciphertext || tag]`. When you read the order, the API looks up the handle in the payload table, extracts the envelope, and decrypts it using the key for the named epoch. When you delete the command group, the correlation index makes cleanup fast: one anti-join against the command log, and all orphaned payloads are deleted in the same transaction.

## Known limits

**The log shows who changed what, and when, even where the contents are
sealed** (SH-12, accepted by the owner 2026-09-29). Each entry in a space's
command log carries the entity, the row's id, the kind of change, the time
and the actor in the clear. The chain and the listeners need them. Sealing
protects what a field holds. It does not hide that a record changed, who
changed it, or when. Somebody who can read the database can learn who is
active in a space and when they work, without reading any sealed value.


## A lost passkey and your data

Recovering a person's **account** and recovering their **data** are two
different acts. In every custody mode the deployment holds today (`Local`,
`SealedMaster`, `Test`), the deployment's own key provider opens a space's
sealed values for an authorised member. A person who has lost their passkey
loses access until they are recovered, and loses no data. Every recovery path
(a second passkey, a recovery code, the deployment owner, a tenant owner)
restores the account and the data behind it.

A vault space, where the deployment has no path to the space key, is
different, and it is planned and not built. A recovered person gets an
account and a new passkey. A new passkey has a different output from the one
it replaces, so it unwraps nothing the old one wrapped. What restores the data
is a device already enrolled in the vault, the vault's own recovery phrase, or
an escrow the tenant chose knowingly. If none of those exists, the data is
gone, and no recovery path changes that.

A recovery code is not a recovery phrase. The code proves who a person is to
the server, which holds only its hash. A phrase is key material the server
never sees. They must not be merged or presented as one thing: merging them
would put key material in a table the deployment owner can read, which is
silent escrow.
