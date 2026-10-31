# Binding a sealed value to the row it belongs to

`security-review-6.md` S6-003, checklist `21c`. Owner-ruled 2026-09-28:
*"yes build it."*

Markers: **[settled]** ruled · **[plan]** this document's proposal ·
**[open]** named and not answered.

Status: **built 2026-09-29 on a branch, not yet merged** (section 6). Every
claim about the code in sections 0 to 5 was read against the tree on
2026-09-28.

---

## 0 · What is wrong, in one paragraph

Every sealed value is authenticated against a label, the associated data,
and the label is `seal:{entity}:{field}` (`ordo-crypto/src/lib.rs:89`,
`stream.rs:153`). It names the column. It does not name the row, or for a
stored file the handle. So an envelope lifted from one row opens cleanly on
another row of the same column, and somebody who holds the database and no
key can move a sealed value they cannot read.

## 1 · A correction to what the owner was told

The owner's objection was cost at run time, and the answer given was that
the cost is close to nothing: about thirty more bytes of associated data on
a cipher that is already running. **That is true for every write that
addresses one row, and it left out the case where it is not.**

`Fixture::assignments` (`ordo-conformance/src/write.rs:3820`) seals a value
**once** and writes it as a constant into `UPDATE … SET col = '\x…'`. For a
ranged write, `set notes(done == false) { memo: "x" }`, that one envelope
lands on every row the condition matches.

Once the label names the row, one envelope cannot serve many rows. A ranged
write to a sealed field has to read the ids it will touch and seal once per
row. So:

| Write | Cost today | Cost with the row bound |
|---|---|---|
| create | one seal | one seal |
| addressed update (`id ==`) | one seal | one seal |
| ranged update of a **sealed** field | one seal, any number of rows | **one seal per row** |
| ranged update of an open field | none | none |
| read, export, revert | one open per value | one open per value |

The third row is the honest cost, and it is proportional to the rows
touched. It applies only to sealed fields and only to ranged writes.

**It also closes a second leak nobody had recorded.** Today the rows touched
by one ranged write hold **byte-identical ciphertext**, so anyone reading the
table learns those rows share a value without opening anything. Per-row
sealing ends that as a side effect.

## 2 · What the fix touches

Counted, not estimated: **65 call sites** outside tests, **27** in tests, and
**10** implementations of `KeyProvider`.

| Surface | What binds | What has to change |
|---|---|---|
| a sealed column | the row id | every seal and open of a column value |
| `$sealedpayload` (chain images) | the row id of the row it is an image of | the table has `entity` and `field` and **no row column**; it gains one, and `PendingPayload` carries it. A payload is a copy of the row's envelope, so it keeps the row's label and no value is sealed twice |
| a stored file's chunks | the object handle | `seal_chunk` / `open_chunk` |
| a file's sealed `mime`, `size`, `integrity` | the object handle | three seals in `BlobPipeline` |
| the idempotency journal's sealed response | the idempotency key | `replay.rs` |
| key rotation | unchanged ids | `reseal.rs` opens and seals under the same row id, and reads it for payloads from the new column |
| a read that opens host-side | the row id | the plan must select `id` even when the projection does not |
| import | the **destination's** minted id | nothing: it goes through the write protocol, which seals at the row it creates |
| `ordo keys rewrap-master` | nothing | unchanged: it moves no envelope |

## 3 · Three questions the build has to answer [open]

1. **Upsert by a unique key onto a sealed field.** The row's id is not known
   until the database resolves the key. The write has to resolve first and
   seal second, inside the same transaction.
2. **A ranged write over many rows.** Per-row sealing turns one statement
   into a read plus N writes. It needs a bound, or a registration-time
   refusal for an unbounded ranged write to a sealed field, which would be
   consistent with how the cost gate treats unbounded reads.
3. **Two images of one row and one field can still be swapped with each
   other**, because they share a label. That is a rollback of one row's own
   value and not a transplant between rows. Binding the payload's own `ref`
   as well would close it and would mean sealing every image separately.
   Recorded as the limit of this fix rather than folded into it.

## 4 · Increments [plan]

Every increment lands with all of its seals **and** all of its opens, since
a value sealed under the new label is unreadable under the old one.

- **B1 · the label.** `seal_at` / `open_at` on `KeyProvider`, taking an
  anchor, on all ten implementations. The old methods stay until B4.
- **B2 · columns and their images.** Create, addressed update, read, export,
  revert, rotation, and `$sealedpayload`'s row column. Ranged writes to a
  sealed field seal per row. Falsified by transplanting an envelope between
  two rows in the database and requiring the open to fail.
- **B3 · files.** Chunks and the three sealed attributes bind the handle.
  Falsified by swapping two objects in the store.
- **B4 · the journal, and the old methods are deleted**, so nothing can seal
  without an anchor by accident.

## 5 · What this does not do

It does not stop somebody who holds the database from deleting rows,
restoring an older copy, or withholding everything. The owner's point
stands: control of the database is control of a great deal. What it changes
is that **rearranging sealed values stops being silent**. A moved value
fails to open, loudly, where today it opens as if it had always been there.

---

## 6 · Built (2026-09-29)

Built on the branch `worktree-agent-a4e059876b8d8384a`, one commit per
increment. Nothing is merged and nothing is pushed. No migration was
written: values sealed under the old label do not open, and nothing has
been deployed with real data.

### The label

The associated data of a stored value is now
`seal:{entity}:{field}:{kind}:{id}`, where the kind is `row:`, `obj:` or
`key:`. The three kinds are a closed enum, `ordo_crypto::Anchor`, so a row
id can never be read as an object handle that spells the same text. The old
label `seal:{entity}:{field}` remains only for wrapping keys
(`seal_with`, `open_with`: the space master key, the data keys and the
master file), which belong to no row. The two labels differ in shape, so a
value sealed under one never opens under the other, and a test says so.

### The increments

- **B1, the label.** `KeyProvider::seal_at` and `open_at`, taking an
  `Anchor`, on all ten implementations. The old `seal` and `open` stayed
  until B4.
- **B2, columns and their images.**
  - Create: the id is minted before the value is sealed, and the seal names
    it.
  - Addressed update: sealed at the addressed id.
  - Upsert and ranged update: see the answers below.
  - Read: the planner packs the row id in front of each sealed output that
    a level reads from a table (`[row id][0x00][envelope]`, one `bytea`),
    and the host splits it and opens at that id. A level that reads the
    output through a subquery, union or CTE passes the packed value on
    untouched. This is how "select `id` even when the projection does not"
    was met without adding a column to any plan's shape. It changes the SQL
    of every plan with a sealed projection, so the two ordo-cli goldens and
    the digests that derive from them moved, and the two generated client
    files that carry those digests were updated by the same substitution.
  - Export, revert and rotation open and seal at the row.
  - `$sealedpayload` gained a column, `anchor`, and not a `row` column as
    section 2 says. A chain image of a blob's digest is a copy of the
    reference row's envelope, which binds an object and not a row, so the
    column holds the anchor's label (`row:ord_1` or `obj:…`) and the
    payload opens at whatever it names.
- **B3, files.** `seal_chunk` and `open_chunk` take an `Anchor`. The
  pipeline seals every chunk and the three attributes on the reference row
  at `Object(handle)`, and opens them there on read, on `verify`, on export,
  on the host blob door and on the revert-time object check. The re-seal
  pass that moves an object to a new handle opens its attributes at the old
  handle and seals them at the new one, whatever epoch each was under,
  because carrying an envelope across unchanged would leave it bound to the
  handle it left.
- **B4, the journal, and the old methods are gone.** The idempotency
  journal seals a response at `Key(principal, op, key)`. `KeyProvider::seal`
  and `open` are deleted, along with the temporary branch in the payload
  and re-seal code that read an empty anchor as the old label. Nothing can
  seal a stored value without an anchor now, because the method does not
  exist.

### The three questions

1. **Upsert by a unique key onto a sealed field.** Resolve first, seal
   second, in the same transaction. The probe already resolves the key
   under `FOR UPDATE`. When it finds a row, the converge half is sealed at
   that row's id. When it finds none, the create half is sealed at the id
   this statement minted. The one gap is a concurrent insert of the same key
   between the probe and the `INSERT`: then the converge arm of
   `ON CONFLICT` would write a value sealed at the wrong id. The existing
   keyed post-image check already refuses that case (`expected_absent`) and
   the transaction rolls back, so the misbound value never commits. I kept
   that check as the answer and did not add a second mechanism.
2. **A ranged write over many rows.** A ranged write that assigns a sealed
   field seals once per row and issues one `UPDATE` per row. A ranged write
   that assigns no sealed field stays one statement. I chose no bound and
   no registration-time refusal. The ranged path already reads the ids it
   will touch, locks each row and builds a before and an after image for
   each, and every image of a sealed field stages a payload, so the write
   already does work proportional to the rows in its range. The extra seal
   is the same order of work, so it does not move the write into a new cost
   class. The cost gate lets an unbounded host-class read register when the
   author acknowledges it, and refuses only the unacknowledged. A refusal of
   an unbounded ranged write to a sealed field would be a new gate with no
   acknowledgment to pair it with, for a cost the write already carries.
   The side effect named in section 1 holds: the rows of one range no
   longer share ciphertext, and a test asserts that.
3. **Two images of one row and one field swapped with each other.** Not
   folded in. It is recorded as the limit of this fix, as ruled. Both
   images share the row's label, so they can still be exchanged. That is a
   rollback of one row's own value.

### The falsification tests

Each fails when the anchor is dropped from the associated data. I made the
change by hand (the label ignores the anchor) and watched them fail, then
restored it.

- **B2:** `an_envelope_moved_to_another_row_does_not_open` in
  `crates/ordo-conformance/tests/row_bound_seals.rs` copies one row's
  envelope over another's in the database and requires the read to fail
  (seen failing without the change). Its sibling
  `a_created_row_holds_an_envelope_that_opens_only_at_its_own_id` does the
  same over the create path (seen failing without the change).
- **B3:** `two_objects_swapped_in_the_store_do_not_open` in
  `crates/ordo-blob/tests/pipeline.rs` exchanges the stored bytes of two
  objects of one column and requires both reads and the integrity check to
  fail (seen failing without the change).
  `the_sealed_attributes_open_only_at_their_own_object` does the same for
  `mime`, `size` and `integrity` (seen failing without the change).
- **B4:** `a_response_moved_to_another_record_does_not_replay` in
  `crates/ordo-host/tests/replay_durable.rs`.

Also added: `an_upsert_seals_at_the_row_the_key_resolved`,
`a_ranged_write_seals_every_row_at_its_own_id` and
`a_chain_payload_records_the_row_it_images` (conformance), and the unit
tests in `ordo-crypto` for the label, including
`a_chunk_moved_to_another_object_of_the_same_column_does_not_open`.

### Not done

- **Verify on `get`.** S6-003 also proposed that the blob read path check
  the sealed integrity digest. It does not. The swap it was meant to catch
  is now caught by the handle in the chunk labels, and a partial range read
  cannot check a whole-object digest. The owner's ruling on the cost of
  verify-on-get is still open.
- **Question 3**, as above.
- **The blind index** is not bound to a row. It is an HMAC of the value and
  is meant to be comparable across rows.
- **`$sealedpayload` in a database that already has the table.** The DDL is
  `CREATE TABLE IF NOT EXISTS`, so a database that holds the old table does
  not gain the `anchor` column. That matches the ruling that nothing owes a
  migration, and it means a development database made before this change
  has to be recreated.

### What surprised

- The write path that seals a column value is the conformance fixture's
  (`ordo-conformance/src/write.rs`), which the executor and the host both
  run. Every seal on a column write is there, which made the count of call
  sites in section 2 misleading about where the work was.
- The planner had no notion of a hidden output, so the row id could not be
  added as a column without touching sema, the plan, the paging wrapper and
  the artifact. Packing it into the sealed cell touched only the select list
  and its two openers.
- A swapped object was never caught on `get`. Only the revert-time check
  looked at the digest. The handle in the chunk labels now catches it on
  `get` with no further code.
