# Bring your own custody

Every way a tenant's data can live somewhere other than the deployment's
defaults, in one place. The owner asked for it on 2026-10-05: *"Make sure
we're documented all the methods of BYO custody items."*

Each section says how to attach, change and detach, what is refused and why,
and what the tenant sees. The settings themselves are in
[deploying.md](deploying.md). Sealed fields, key rotation and erasure are in
[howto-custody.md](howto-custody.md). This guide links to both and does not
repeat them.

## The options at a glance

| What | Who brings it | Attach | Change | Detach |
|---|---|---|---|---|
| [A database](#a-database) | the deployment owner, for a tenant | at creation, on `std.space.create` | refused; a move is 18e, after the first deployment | refused; deleting the tenant is the only end |
| [A bucket](#a-bucket) | the tenant owner | `std.space.attachBucket` | `attachBucket` again; files already written stay put | `std.space.detachBucket`; files already written stay put |
| [The master key](#the-master-key) | the deployment owner, for the whole deployment | `ordo keys seal-master` | `ordo keys rotate` per tenant | not offered |
| [Credentials](#credentials) | the deployment owner | a file in `ORDO_TENANT_CREDENTIALS_DIR` | replace the file, restart | remove the file, restart |

Two choices are made at every attach of a database or a bucket, and both are
recorded:

- **Platform-sealed or own perimeter.** Platform-sealed means the tenant's
  storage holds what the platform sealed, and deleting the tenant makes it
  unreadable. Own perimeter means the storage is the tenant's, and the
  platform's erasure leaves the files in it. See
  [What erasure does to files in your own bucket](#what-erasure-does-to-files-in-your-own-bucket).
- **No default.** The call must state the position. The platform never picks
  a disclosure on the tenant's behalf.

## Before any of it

Bring-your-own storage does nothing until the deployment is set up for it.

- `ORDO_TENANT_ATTACH_HOSTS` lists the hosts a tenant's database or bucket may
  be on. **Unset refuses every attach.** See [deploying.md](deploying.md)
  §2c.
- `ORDO_TENANT_CREDENTIALS=dir` and `ORDO_TENANT_CREDENTIALS_DIR` hold the
  credentials, by label. See [Credentials](#credentials).
- Every tenant has a database of its own. A cluster of the tenant's own is
  named on `std.space.create`, and its credential label must be installed
  through `ORDO_TENANT_CREDENTIALS` (deploying.md §4d).
- A bucket of its own needs `ORDO_BLOB_ROUTING=attached` and an
  `ORDO_BLOB_STORE`. Without all three of these and the credentials, the
  bucket ops are not served at all.

## A database

**Every tenant has a database of its own.** What can be brought is the
cluster it lives on. The four levels and their defaults are in
[deploying.md §2d](deploying.md#2d).

**Attach.** At creation, on `std.space.create`, by the deployment owner or the
app developer's backend acting as the deployment:

- `dbHost`, `dbPort`, `dbUser` name the cluster.
- `dbCredential` is a credential label, never a password.
- `dbOwnPerimeter` is required with `dbHost`. `true` is the tenant's own
  perimeter; `false` is platform-managed.
- `dryRun: true` checks all of it and builds nothing.

A tenant made by self-serve sign-up always lands on the default tenant
cluster.

**Change.** Refused. A space attached to one database is never pointed at
another (`AttachRefusal::WouldMove` in `crates/ordo-exec/src/spacedb.rs`).
Moving a live tenant between databases is checklist 18e, ruled for after the
first deployment. A move must keep the chain whole, keep the undo horizon,
repoint every object reference, and have both parties agree.

**Detach.** Not offered. The database holds the tenant's rows and its audit
chain, so there is nothing for the tenant to fall back to. Ending a tenant is
`std.space.delete`.

**What is refused, and why.**

- A host off the allowlist. One reason, no detail, so the list cannot be
  mapped one try at a time.
- The deployment's own database. No tenant is ever put in the control plane.
- A database another tenant already sits on.
- A cluster that does not answer the dial.

**What the tenant sees.** The custody sentence, from the platform, for the
position chosen. For own perimeter it says that values that are not sealed
stay in the tenant's backups and replicas after deletion, and that removing
them is the tenant's to do.

## A bucket

**Attach.** `std.space.attachBucket`, by the tenant owner, with fresh
presence (step-up). A capability token can never stand in for presence, so an
app cannot do this on the owner's behalf.

- `endpoint`, `region`, `bucket` name the target.
- `credential` is a label, never an access key.
- `platformSealed` is required. `true` means the bucket holds ciphertext the
  tenant cannot read alone. `false` means the bucket is the tenant's own
  perimeter.
- `pathStyle` and `keyPrefix` are optional, for a self-hosted gateway and for
  a bucket that holds other things too.

Before the row is written, the platform proves the credential signs, with a
`head` and never a `list`, so a credential scoped the least-privilege way
passes.

**Change.** Call `attachBucket` again with another target. The answer is
`replaced`. The same target again is `unchanged` and records nothing, so a
retry is safe.

**Detach.** `std.space.detachBucket`, by the tenant owner, with fresh
presence. It takes no parameter. The tenant's uploads go to the deployment's
own store from then on. Nothing attached answers `unchanged`. Ruled
2026-10-05 (`bucket-detach`).

**What is refused, and why.**

- A host off the allowlist, or a plaintext endpoint without
  `ORDO_DEV_UNSAFE_BUCKET_ENDPOINT=1`.
- The deployment's own default bucket. The disclosure would say the bytes are
  in a bucket the tenant holds, and they would be in the platform's.
- A bucket another tenant holds as its own perimeter, or an own-perimeter
  attach to a bucket somebody else is in. Sharing a bucket is otherwise fine:
  every object is keyed under its own space.
- A credential label this node does not hold, or one this tenant may not use.
  One answer for both.
- A credential the bucket refuses at the probe.

**Other nodes.** Each node remembers a tenant's bucket for 30 seconds
(`AttachedStores::TTL`). For up to that long after a change or a detach,
another node may still send a new upload to the old target.

**Each file records the bucket it was written to** (ruled 2026-10-05,
`detach-upload-race`, 25ac). An upload is not checked against the tenant's
bucket before it commits.

- The file's `$blobref` row names the bucket its bytes reached, in the
  `store` column. The empty name is the deployment's own store.
- Reads, the storage sweep, erasure, export, the re-seal pass and
  `ordo verify --objects` follow that name, never the tenant's current
  bucket. `ordo verify --objects` signs with the credentials in
  `ORDO_TENANT_CREDENTIALS_DIR`. Without them, a file in a tenant's bucket is
  reported unverified.
- So an upload that lands in the old bucket during a change or a detach stays
  readable there, and the sweep deletes it from there.
- This record is why a change or a detach is allowed while files are held.
  See the next section.

**Changing or detaching a bucket that holds files** (ruled 2026-10-06,
`st-attach-existing`). Before this, both were refused with `bucket_refused`,
because nothing recorded which bucket a file lived in. Now each file's row
records it, so the refusal is gone.

- Each file lives in the bucket it was written to. Its row says which.
- Files written before the change stay there. Nothing is copied.
- That keeps a change quick, and it cannot half-fail in the middle of a copy.
  It also moves no bytes the tenant did not ask to move.
- New uploads go to the new bucket, or to the deployment's own store after a
  detach.
- Old files are still read through the door, swept, erased, exported and
  re-sealed, in the bucket they were written to.
- That needs the old bucket to stay reachable. It also needs its credential
  label to stay installed in `ORDO_TENANT_CREDENTIALS_DIR` on every node.
- If the deployment owner removes that credential, old files cannot be read.
  The same holds if the tenant deletes the old bucket or revokes its key.
- The door then answers an error for those files. It never answers "not found".
- The sweep stops on that bucket and reports it. It does not retire the
  reference, so no file is orphaned in Ordo's records.
- Erasure of the tenant fails loudly until that bucket is reachable again.
- If the tenant deleted the bucket, the bytes are already gone. The rows stay
  until the file is replaced or deleted, and then the sweep clears them.
- The attach and detach answers carry `filesStayingPut`, the number of files
  left behind. It is null on an `unchanged` retry.
- The answers also carry `existingObjects`: this change governs uploads from
  now on, and the old bucket and its credential must stay reachable until the
  old files are gone.
- Old files still count toward the tenant's bytes stored until they are
  deleted.
- Moving old files into the new bucket is a separate step. It is not built.
- The one-for-one exception (2026-09-29, attach a bucket that already holds a
  copy of every object) is not built either. It would become part of that
  move step.
- When D-S5 is built (ruled 2026-09-21, not built), custody is taken per file,
  from each row's recorded bucket. Today erasure deletes every object.

**What the tenant sees.**

- The answer carries the custody sentence for the position chosen, and
  `custody: "platform"` after a detach.
- Every change is recorded on the deployment's stream as a `CustodyAction`:
  `bucket_attached`, `bucket_replaced` or `bucket_detached`, with the target's
  identity. The tenant's own security stream carries nothing about it. Where a
  tenant's files were put is for the app developer to disclose (ruled
  2026-09-29, 22k).
- The credential label is never in an answer to the tenant.

## What erasure does to files in your own bucket

`std.space.delete` decides per file, from the bucket the file's row records
(ruled 2026-09-21, D-S5).

| Where the file is | What erasure does |
|---|---|
| The deployment's own store | Deletes the object. |
| A bucket attached with `platformSealed: true` | Deletes the object. |
| A bucket attached with `platformSealed: false` (own perimeter) | Leaves the object. |

A tenant that has files in more than one place gets each treatment per file.

For files left in your own bucket:

- Erasure destroys the keys first. The left objects cannot be opened again.
- The platform does not delete them. Removing them is yours to do.
- The answer carries `objectsLeft`, one entry per bucket: `store`
  (`endpoint|bucket|keyPrefix`), `count`, `epochs` and `digest`.
- The same entry is written to the tenant's security stream beside
  `space_erased`, as a `CustodyAction`:
  `objects_left store=<endpoint|bucket|prefix> count=<n> epochs=<list> digest=sha256:<hex>`.
- `digest` is a SHA-256 over the sorted object handles, one per line, in hex.
  An app developer can match it against a list from the bucket. It holds no
  key material and no credential.
- The platform records its own act. It does not say the bytes are gone,
  because it did not delete them.
- The signed erasure receipt an app hands its user is separate work (17ab)
  and is not built.

## The master key

The deployment owner brings the master key for the whole deployment. It wraps
each tenant's space master key, and those wrap the per-epoch data keys.

- **Attach.** `ordo keys seal-master`, then `ORDO_KEY_PROVIDER=sealed-master`
  with its key and passphrase files. See [deploying.md](deploying.md) §4.
- **Change.** Per tenant, `ordo keys rotate`, then the re-seal pass. See
  [howto-custody.md](howto-custody.md#rotate).
- **Detach.** Not offered. A deployment without its master key cannot read
  any tenant.

**What is not offered: a tenant's own key.** A tenant cannot bring a key the
platform does not hold. There is no KMS binding and no per-tenant
customer-managed key, and a client-first vault is not built. The gap is
recorded in [audit-topology-gaps.md](audit-topology-gaps.md) §3.2. Every
tenant does have keys of its own, and deleting a tenant destroys them.

## Credentials

A database or bucket attach names a **label**. The deployment owner installs
the material under that label, out of band, so no secret rides a request, a
replay cache or the command log.

- One directory, `ORDO_TENANT_CREDENTIALS_DIR`, read once at boot.
- **A file** is a Postgres password. An empty file is passwordless.
- **A directory** is an S3 key pair: `access_key_id` and `secret_access_key`.
  Empty halves are refused at boot.
- Group- or world-readable material is refused at boot.
- The label `$deployment` means the deployment's own database password, and
  needs no file.

Removing a bucket credential label also cuts off every old file written under
it. See [Changing or detaching a bucket that holds files](#a-bucket).

Changing or removing material is a file change and a restart, because the
directory is read once at boot.

## Not custody

`std.space.attestTo` names where a tenant's history is published. It chooses
a witness, and moves no data, so it is not covered here. See
[howto-audit.md](howto-audit.md).
