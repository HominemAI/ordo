# Per-tenant object storage

**Requirements and increments, not a build.** Gathered 2026-09-20, in lockstep
with the per-tenant database requirements in `plan-space-routing.md` §9 to §11.
The owner: *"While we're doing this we need to make sure the same per tenant /
deployment S3 configuration exists."*

**Where this sits.** `plan-blob.md` designed and built the blob subsystem
(B1 to B5) and `plan-space-routing.md` §9 gathered what is still owed to make a
tenant's **database** its own. This document is the same pass for a tenant's
**bucket**. It is a separate file rather than a twelfth section of
`plan-space-routing.md` because it has to cross-reference `plan-blob.md` as
heavily as it cross-references the database plan, and appending a second
requirements pass to a document that already carries one would bury both. Every
requirement here is numbered `B-…` against the database plan's `R-…`, and each
section names the database section it parallels, so the two read side by side.

**Sections, and the database section each one mirrors.**

| Here | There |
|---|---|
| §1 The state of the tree | §9.0 |
| §2 Configuration | §9.1 (first half) |
| §3 Credentials | §9.1 (`Where the attachment's credentials live`) |
| §4 What travels with the tenant | §9.3 |
| §5 Failure modes | §9.4 |
| §6 The garbage sweep | §9.5 R-P5, and new |
| §7 Key rotation and re-sealing | new |
| §8 Exit and import | §9.6, §9.7 |
| §9 Usage and billing | §9.6 R-O5 |
| §10 Security | §9.8 |
| §11 Increments | §10 |
| §12 Owner decisions | §11 |

## 1 · The state of the tree, verified [2026-09-20]

**The headline is the database plan's headline, one tier over.** The seam is
built, the table is built, the op is written, and the shipped binary wires the
single-store variant, so no deployment can turn any of it on.
`crates/ordo-host/src/runtime.rs:1524` builds
`ordo_blob::SingleStore::new(store.clone())` from the one `ORDO_BLOB_STORE`
binding, and `runtime.rs:1755` registers `std.space.attachBucket` on the
unavailable list with the reason *"no resolver reads an attached bucket yet
(every space resolves to the one ORDO_BLOB_STORE)"*. `platform_specs`
(`runtime.rs:669`) states the same thing as policy: the op is registered
**never, yet**, because *"registering it would record attachments that nothing
honours"*.

| Built | What the binary constructs instead |
|---|---|
| `StoreResolver` (`ordo-blob/src/lib.rs:232`) | `SingleStore` (`runtime.rs:1524`) |
| `bucket::lookup` / `bucket::attach` over `$spacebucket` | nothing; no production code reads the table |
| `BucketOps` (`ordo-host/src/bucket_ops.rs`) | unregistered, on the unavailable list |
| `BucketOps.credentials: Vec<String>` | no configuration populates it |

**And there is a fifth gap the database side does not have.** `BlobPipeline`
holds `store: Arc<dyn BlobStore>` directly (`ordo-blob/src/pipeline.rs:205`),
not a resolver. So the serving path has **no per-space seam at all**: every
`put` and `get` through the blob door goes to the one store the pipeline was
built with. On the database side `Spaces::pool_of` is already the one seam
every request-path checkout passes through, and wiring `AttachedPools` behind
it reaches every caller. Here there is no equivalent, and giving the pipeline a
resolver is the first real build (§11 U2), not a wiring change.

**What is already per-space and correct.** Three callers take a
`StoreResolver` and resolve per space today: `blobgc::sweep`
(`crates/ordo-exec/src/blobgc.rs:302`), the re-seal pass's `move_objects`
(`crates/ordo-exec/src/reseal.rs:1187`) and `erase_space` through
`ExitOps.stores` (`crates/ordo-host/src/exit_ops.rs:114`). They resolve
`SingleStore` today and would resolve a real attachment unchanged. The object
key is already tenant-scoped: `<space>/<hex handle>`, with `space_prefix`
refusing a hostile space id rather than sanitising one
(`ordo-blob/src/lib.rs:255`).

**What `$spacebucket` already settles** (`crates/ordo-exec/src/bucket.rs`).
The table is `("$space" PRIMARY KEY, endpoint, region, bucket, credential,
custody, attached_at, attached_by)`, with a test reading the DDL string to
assert **no secret column** (`the_attachment_table_holds_no_secret`).
`Attachment::identity` is `endpoint|region|bucket|credential|custody`, which is
what makes a repeat of `attachBucket` a no-op that records nothing, and
`AttachOutcome` is `Attached | Replaced | Unchanged`. `BucketCustody` is
`PlatformSealed | OwnPerimeter` and carries `disclosure()`, one sentence per
variant, written in the platform rather than in a renderer.

**What the disclosure settles, and what it does not.** It settles the
**erasure** question, which is the one that differs between the two variants:
under `PlatformSealed` the tenant's bucket holds ciphertext they cannot open
alone and destroying the space's keys makes the objects permanently unreadable
wherever they sit; under `OwnPerimeter` destroying the platform's keys does not
make them unreadable and removing them is the tenant's to do. It does **not**
settle occupancy (§10 B-X1), it does not settle who pays for objects the
platform leaves behind (§7), and it says nothing about the objects already in
the deployment's default store at the moment of the attach, which the op's own
response calls out in `existingObjects`: *"objects already stored stay where
they are; this attachment governs uploads from now on"*.

**One defect found while reading, and it is a one-line fix.** `BucketOps::attach`
returns `"credential": want.credential` in its response rows
(`bucket_ops.rs:184`). The caller is the space owner, which is a tenant, and
the database plan's R-C4.2 says a credential label is deployment-side
configuration a tenant has no use for. The op is unregistered, so nothing has
leaked; the requirement is B-X4 and the test is named in U1.

**What the blob side records.** The custody event is recorded on the
deployment's stream only, with the space as the event's object. It was once
also written to the tenant's own security stream (`space: Some(space)`,
`Relationship::Member`). The owner ruled on 2026-09-29 that it should not be
(checklist 22k, `db-chain-attach`): where a tenant's files were put is for the
app developer to disclose. Built: `BucketOps::record_change` writes one
observation, with `Relationship::None`. An attach, a replace and a detach all
go through it.

## 2 · Configuration: the deployment's store, a tenant's bucket

*Mirrors §9.1's first half.*

**The default store needs no new configuration, for the database plan's
reason.** `blob_from_env` (`runtime.rs:728`) reads `ORDO_BLOB_STORE` with two
forms, `s3` (taking `ORDO_BLOB_S3_{ENDPOINT,REGION,BUCKET,ACCESS_KEY_ID,
SECRET_ACCESS_KEY}` through `S3Config::from_env`) and `fs:<path>`, refuses an
unknown word rather than falling back, and refuses `fs:` outside dev mode on
the authority of `BlobStore::production_ready` rather than on the scheme
string. Whether a store is **needed** is a question about the loaded program
(`blob_needed`), asked after the dial. A deployment that attaches nothing keeps
exactly this behaviour.

**B-C1 · Resolution is selected explicitly, never inferred, and it is the same
variable shape.** `ORDO_BLOB_ROUTING`, with `single` (today) and `attached`,
refusing an unknown value, with no default that changes behaviour. The argument
is `ORDO_PG_ROUTING`'s and it is stronger here: `$spacebucket` rows can already
be written by a deployment that later turns routing on, because the op that
writes them is gated on registration rather than on a resolver, so inferring
from "the table has rows" would start a deployment reading attachments nobody
configured it to honour. `attached` additionally requires B-K1's credential
provider and refuses at boot without it.

**Whether it is one variable or two is the owner's, and the recommendation is
one.** `ORDO_PG_ROUTING` and `ORDO_BLOB_ROUTING` can be turned on
independently, which §4 B-H3 shows is a real configuration a tenant can be in.
A single `ORDO_TENANT_ROUTING` would couple them and remove that. See D-S1.

**B-C2 · A tenant is pointed at a bucket by its `$spacebucket` row, and the row
needs two columns it does not have.**

1. **`path_style`.** `S3Config` carries it and derives it in `new`:
   `!endpoint.tls || local || host.parse::<IpAddr>().is_ok()`
   (`s3.rs:134`). That derivation is right for the deployment's own store,
   where the operator controls the endpoint, and wrong for an attachment: a
   self-hosted MinIO or Ceph behind TLS on a real hostname needs path style,
   the derivation gives it virtual-host style, and the failure is a DNS lookup
   for `<bucket>.<host>` that does not resolve. The tenant cannot express the
   thing their own gateway requires. The column is nullable, `NULL` meaning
   "derive as today", so no existing row changes meaning.
2. **`key_prefix`.** `object_key` is `<space>/<hex handle>` and the space id is
   deliberately in it, so that an operator holding a bucket and an incident can
   answer "which objects are this tenant's" without the database. In the
   deployment's own bucket that prefix separates tenants. In a tenant's **own**
   bucket the tenant is alone, the prefix is their own opaque id, and a bucket
   they also use for other things has no way to be told where Ordo's objects
   are allowed to live. One optional prefix, applied ahead of the space
   segment, is the whole ask. Empty means today's layout. It must be refused by
   the same charset `space_prefix` refuses on, for the same reason.

Nothing else in `S3Config` is missing: `endpoint`, `region` and `bucket` are
already columns and are validated at attach time by the same `S3Config::new`
the binding uses (`bucket_ops.rs:145`), so an attachment that could not boot is
refused at the door rather than at the first upload.

**B-C3 · A role rather than a key pair is not v1, and the reason is the
binding.** `S3Store` signs with SigV4 over a static `access_key_id` and
`secret_access_key`, and refuses an empty one rather than attempting anonymous
access (`s3.rs:125`). Role assumption (`AssumeRole`, `AssumeRoleWithWebIdentity`,
IRSA, or an EC2/ECS metadata credential) is a **second protocol**: an STS call,
a third `x-amz-security-token` header in the canonical request, and an
expiry the store has to refresh underneath live requests. All three are
additions inside `s3.rs` rather than changes to `BlobStore` or to
`$spacebucket`, exactly as multipart is. The credential provider of §3 is where
a role would appear when it does, as a provider that returns a triple with an
expiry instead of a pair without one, and the attachment still names a label.
See D-S4.

**B-C4 · A `fs:` store is never an attachment target.** `ORDO_BLOB_STORE=fs:`
already needs `ORDO_DEV_UNSAFE_BLOB_STORE=1`, on the authority of
`production_ready`. An attachment has no scheme field and `bucket_ops` validates
through `S3Config::new`, so this is true today by construction. It is written
down because the obvious way to make attachments testable is to let one name a
directory, and that is how a test double becomes a tenant's only copy.

## 3 · Credentials: what S3 needs that a Postgres password does not

*Mirrors §9.1's `Where the attachment's credentials live`.*

**The rule is already settled and is already enforced here.** `$spacebucket`
has no secret column and a test reads the DDL to say so. The reason is
`bucket.rs`'s own module doc and it is the reason the database plan borrowed:
the command log is plaintext by design, so a secret passed as an op parameter
is a secret in the request envelope, in the idempotency cache, in every proxy
log and in the chain, and rotating it would become a schema-level act. The
2026-09-08 ruling states it as ruling 5: *a BYO-bucket attachment names a
credential, never carries one*. `BucketOps` enforces the other half: a label
the deployment did not install is refused at the door before anything is
stored, with one `not_found` answer covering both "nobody installed it" and
"this space may not use it", so the parameter enumerates nothing.

**What does not exist.** `BucketOps.credentials` is a `Vec<String>` that
nothing populates, because nothing constructs `BucketOps`. There is no way for
an operator to install a bucket credential, which is the same gap
`DbCredentials` has.

**B-K1 · Object storage takes the same provider as the database, not its own,
and the reason is the operator rather than the protocol.** One
`ORDO_TENANT_CREDENTIALS` naming the provider and one `dir` implementation
reading one mounted directory, shared by `$spacedb` and `$spacebucket`. The
argument:

- **The thing being solved is identical.** Both need a stable label, material
  that lives in a file an orchestrator mounts, rotation by changing the file
  rather than the row, refusal at boot on a group- or world-readable file, and
  a health surface that names the labels a node holds and never their values.
  All six properties are the database plan's R-C3 and none of them is about
  Postgres.
- **Two providers would be two rotation procedures and two boot refusals**,
  and a tenant with both a database and a bucket would have credentials
  installed two ways on the same node. §9.7 C-M3 requires a node to report the
  labels it holds so a move can be checked before it copies anything; one list
  is checkable and two are two lists an operator will keep in sync by hand.
- **The label namespace stays flat and shared.** A label is a name for
  material, and the attachment that names it already says what kind of target
  it is. A label installed for a database and named by a bucket attachment
  fails at the first use in exactly the way B-K2 describes, which is a refusal
  and not a confusion.

**B-K2 · What S3 needs that a Postgres password does not, and how the one
provider carries it.** A Postgres label resolves to one string, and
`DbCredentials::password` returns `Result<Option<String>, String>` where
`Ok(None)` is a passwordless target. S3 needs **two** strings that are useless
apart, and a role would need three plus an expiry. The other three things S3
needs are already attachment columns and must stay there:

| What S3 needs | Where it lives | Why not in the credential |
|---|---|---|
| access key id | the credential file | it is half of a secret |
| secret access key | the credential file | it is the other half |
| region | `$spacebucket.region` | it is part of the target's address and part of the SigV4 scope; it is not secret and an operator reads it in a listing |
| endpoint | `$spacebucket.endpoint` | same, and it is what the attach-time allowlist of B-X3 checks |
| path style | `$spacebucket.path_style` (B-C2) | a property of the gateway, not of the key |
| session token, if a role | the credential, with an expiry | deferred, B-C3 |

So the `dir` provider grows one shape and no second provider: a label may
resolve to a **file** (the database case, whose trimmed contents are the
password, and whose emptiness means passwordless) or to a **directory named by
the label** holding `access_key_id` and `secret_access_key` as separate files.
Two files rather than one two-line file, for `sealed-master`'s stated reason
that the pieces are kept separable, and because that is the layout a Kubernetes
secret mounts by key. Both forms refuse at boot on a mode that is group- or
world-readable, matching the `0o600` the master key is written with.

**B-K3 · The whole directory is read at boot and refuses there.** A bucket
credential that is wrong should cost milliseconds at boot rather than a
tenant's first upload, which is the database plan's argument and is worse here:
a failed upload has already consumed the client's bytes.

**B-K4 · Rotation is a file change and a reload, and never a write to
`$spacebucket`.** This is the property the reference-not-value rule was chosen
for, and on the S3 side it is the common case rather than the rare one: access
keys are rotated on a schedule by every provider's own guidance, while a
Postgres password often is not. A label must therefore be stable across a
rotation, and the rotation must not produce a `Replaced` outcome, because
`Attachment::identity` includes the credential label and a `Replaced` writes a
custody event. Rotating a key is not a custody change, and recording one would
put a false "storage custody changed" in the owner's own stream on a schedule.

**B-K5 · A KMS is deliberately not v1**, for D-H's reason on the database side,
and with one more: the material is the same file a KMS provider later opens, so
moving is a provider swap and a re-mount rather than a migration of
`$spacebucket`, the resolver or the audit trail.

## 4 · What travels with the tenant and what does not

*Mirrors §9.3.*

**A per-tenant bucket moves the object bytes and nothing else.** Everything
that names, opens, bills or retires those bytes stays where it was. Verified by
reading where each thing is written.

| Concern | Where it lives | Moves with the bucket? |
|---|---|---|
| the sealed object bytes | the resolved store, at `<space>/<hex handle>` | **yes, and only this** |
| `$blobref` rows (`ref`, `epoch`, `entity`, `field`, sealed `mime`/`size`/`integrity`, plaintext `bytes`, `created_at`) | the tenant's database | no; moves with the **database** |
| the entity column holding the hex handle | the tenant's database | no; moves with the database |
| `$blobtomb` | the tenant's database | no; moves with the database |
| the DEK that opens the objects | `$keycustody` in the **control** database, wrapped under the deployment's master key | no, and §9.3 R-H2 says why custody is deployment-wide |
| `$spacebucket` itself | the control database | no |
| `$usage.bytes_blob` | the control database, summed from `$blobref` | no |
| the `bucket_attached` custody event | the deployment's security stream (ruled 2026-09-29, not the tenant's) | no; stays with the deployment |

**B-H1 · The reference set is in the database, which is the property every
other section depends on.** `BlobStore` has **no `list`**, deliberately
(`ordo-blob/src/lib.rs:161`): a listing is how a bucket enumerates a tenant, and
the authoritative reference set is `$blobref`, which the sweep already reads.
Two consequences to state once and reuse: the platform cannot answer any
question about a bucket without that tenant's database, and it cannot
reconcile a bucket against the database even if it wanted to. That absence is
what makes a bucket without its database inert, and it is also what makes §5's
half-written write invisible.

**B-H2 · Nothing records which store an object was written to, and today that
is a silent data-loss path.** *Since 2026-10-05 each `$blobref` and
`$blobtomb` row records its store (`store` column, 25ac,
`detach-upload-race` ask 2), and the sweep, erasure, export, the re-seal pass
and the door's read follow it. *The D-S2 refusal was lifted 2026-10-06 (`st-attach-existing` ask 2): a change or a detach is allowed while files are held, and old files stay in the bucket their row names.* `$blobref` has ten columns and none of them names
a store. The attach op's own response promises a split population: *"objects
already stored stay where they are; this attachment governs uploads from now
on"* (`bucket_ops.rs:198`). Three consumers resolve **one** store per space and
would act on the wrong one:

- **`blobgc::sweep`** resolves the current store, deletes, then stamps the
  tombstone and removes the `$blobref` row. `BlobStore::delete` is
  **idempotent on an absent key**, which is what makes phase 2 crash-safe, and
  here it means the delete of an object that lives in the *other* store
  succeeds. The reference is removed, the bytes stay in the old store forever,
  and nothing names them ever again, because there is no `list`.
- **`erase_space`** takes the same resolver and uses `$blobref` as the list of
  objects to delete (`exit_ops.rs:298-318`). A deletion request over a split
  population reports success while leaving bytes behind. That is a data
  remanence failure on the one operation whose whole point is that nothing is
  left.
- **`reseal::move_objects`** reads the old handle from the current store, so a
  re-seal over a split population fails on the objects that are elsewhere,
  which is the benign one of the three because it stops rather than proceeding.

Two ways to close it, and they are the owner's (D-S2). **Recommended: refuse
the attach**, on `erase::rows_held`'s precedent, when the space already holds
`$blobref` rows, and delete the `existingObjects` sentence from the response,
because moving the population is a migration and the database side already
ruled a live move out of scope and filed it as 18e. The alternative is a
`store` column on `$blobref` naming the attachment generation each object was
written under, which makes the split legal and makes every consumer resolve per
row rather than per space. That is a larger build and it is the shape a move
will need anyway, so it is the right thing **later** and the wrong thing to ship
before there is an attach at all.

**B-H3 · A tenant can have its own database, its own bucket, both or neither,
and all four are real.** `ORDO_PG_ROUTING` and `ORDO_BLOB_ROUTING` are separate
switches (§2 B-C1), so this is a configuration matrix rather than a hypothetical.

- **Neither.** Today. Rows on the default database, bytes in the default store.
- **Database only.** The tenant's references live in their database and their
  bytes in the deployment's shared bucket. The bucket's objects are keyed
  `<space>/<handle>`, so they are still separable by prefix, and the tenant's
  disclosure must not say their files are in their own perimeter. This is the
  combination most likely to be configured by accident, because attaching a
  database is the headline act.
- **Bucket only.** The tenant's bytes are in their bucket and their references
  are rows in the shared control database beside every other tenant's. Under
  `OwnPerimeter` this is the sharpest disclosure problem in the document: the
  tenant holds the objects and the platform holds, in a database the tenant
  does not control, the only index that says what those objects are. Erasure is
  still crypto-erasure of a key the platform holds, so `OwnPerimeter`'s
  disclosure sentence stays true, and it reads as stronger than the
  arrangement is.
- **Both.** The intended shape, and the only one where a tenant's data has one
  home. It is also the only one where a backup is self-consistent: §9.6 R-O2
  already says a tenant's backup has two halves, and a bucket makes it three.

**B-H4 · The three halves of a tenant's backup, and the order.** Extending
R-O2 rather than restating it. The tenant's **database** holds the references
and the chain; the tenant's **bucket** holds the bytes; the **control**
database holds `$spacedb`, `$spacebucket`, the wrapped keys, the quotas, the
usage ledger and the witness. Restoring the database without the bucket gives
references to bytes that are not there, which is a dangling reference the shape
gate cannot see and is exactly what §5's floor exists to prevent in the other
direction. Restoring the bucket without the database gives objects nobody can
name and nobody can list. The order is bucket, then database, then the control
rows for that tenant, and the witness is not restored, for R-O2's reason. The
bucket goes first because an object with no reference is inert and an object
the sweep has not been told about cannot be deleted by it, while a reference
with no object is a broken read on the first request.

## 5 · Failure modes: unreachable, misconfigured, half-written

*Mirrors §9.4. Same register, same list, no second mechanism.*

**Where storage genuinely differs, and it changes the answer.** A tenant whose
database is unreachable cannot serve **any** request. A tenant whose bucket is
unreachable can serve every request that does not touch a byte, which in most
programs is nearly all of them. So quarantining the tenant on an unreachable
bucket would take down request paths that never needed the store, and the
17aa shape is used here for reporting rather than for refusal.

**B-F1 · An unreachable store is reported through the register 17aa already
has, and refuses only the blob routes.** The entry names the tenant, its name
attribute, the reason and since when, and it joins the one per-tenant list
§9.4 R-F6 requires `/health/ready` to assemble. What it does not do is make
`Spaces::continuity_of` return `Quarantined`, because that answer is read by
every request. The distinction is one an operator must be able to see in the
list, so the reason string says "store unreachable" and a tenant may carry both
that and a database reason at once.

**B-F2 · A read failure and a write failure are different problems and get
different answers.**

*A read.* `BlobError` separates `NotFound` from `Transport` and `Refused`
already. The requirement is that **`NotFound` from the store, when `$blobref`
holds the row, is never a 404 to the caller.** The platform's own record says
the object exists, so "this file does not exist" is a false statement that
tells the caller to stop retrying and tells a tenant their file is gone. It is
the unavailable answer of B-F4 with an operator-visible entry, and it is the
one condition that should page, because on a per-tenant bucket it means either
a deletion the platform did not make or an attachment pointed at the wrong
bucket. A `NotFound` with no `$blobref` row stays a 404, because then the
record agrees.

*A write.* `BlobPipeline::put` writes the object and writes **nothing** to the
database; the caller writes the `$blobref` row and the entity column in one
transaction, because *"a `$blobref` row and a column that disagree is worse
than either alone"* (`pipeline.rs:224`). So the order is object first,
reference second, and a failure between them leaves an object no `$blobref` row
names. With no `list`, `blobgc` can never see it and no operator can ever find
it. On the deployment's own store that is the platform's own cost and is
invisible; on a tenant's bucket it is a line on the tenant's bill for bytes the
platform will never account for and cannot delete.

**B-F3 · The half-succeeded write is closed with the table that already
exists.** Before the `put`, the door writes a `$blobtomb` row for the minted
handle with `unreferenced_since = now`. If the transaction commits, phase 2's
re-check under the row lock finds the live reference and cancels the tombstone,
which is exactly what it already does for a revert. If anything fails, the
tombstone stands and the ordinary sweep deletes the object once the floor has
passed. No second mechanism, no reconciliation pass, and no `list`. It costs one
insert per upload and it turns an unbounded leak into an object that retires on
the same floor as everything else.

**B-F4 · An unreachable store answers 503, not 500, and never carries the
store's text.** `BlobError::Transport` and `BlobError::Refused` both carry a
string documented as *"the operator's text; never the caller's"*
(`ordo-blob/src/lib.rs:137`), which is the property §9.1 R-C4.4 requires and
which needs a test rather than a comment. A bucket endpoint, a region and a
provider's own error body together map a deployment's storage topology, and the
`Refused` case can carry an access-denied message naming the credential's
principal.

**B-F5 · A misconfigured attachment refuses at resolve, not mid-request.**
`BlobError::Config` is documented as *"Raised at construction, never
mid-request"*, and `S3Config::new` already refuses plaintext HTTP to a
non-loopback endpoint, a bucket name outside the S3 charset and an empty
credential. `BucketOps` runs that same constructor at attach time with a probe
credential, so a bad target is refused before the row is written. What is
missing is the **credential** half: the probe passes literal `"probe"` strings,
so an attachment naming a real label whose material is wrong is accepted and
fails at the first upload. The requirement is that an attach, once the resolver
exists, performs one `head` of a key that cannot exist and accepts any answer
that proves the credential signed, which distinguishes "cannot reach" from
"reached and was refused" at the moment the operator can still fix it.

**B-F6 · A failing store backs off, and the backoff is per attachment.** S3's
own errors are retryable by class and the binding has no cache of a failed
connection, so a down bucket costs a fresh dial per request for every request
that touches a byte. The bound is the database plan's R-F3, with the register as
the place its state is read.

**B-F7 · The deployment's own store going away is not a tenant report.** It is
the deployment's blob tier being down, it is reported once on the deployment's
own readiness, and it is stated here so nobody files one entry per tenant for
one cause. This is R-F5 one tier over.

## 6 · The garbage sweep across buckets

**What is built.** `blobgc::sweep(pool, stores, schema, space, policy, now)` is
already per space and already resolves per space
(`crates/ordo-exec/src/blobgc.rs:302`, `:332`). Phase 1 marks in one
transaction against Postgres only; phase 2 re-checks the live reference under
the tombstone's row lock, deletes the object, stamps `deleted_at`, then removes
the `$blobref` row last. The floor is `prune::Floor`, the same value from the
same function as the log prune, reported with the same `FloorHolder`.

**What has no runner.** `workers.rs:873` carries one unfilled slot:
`("blob.gc", "blobgc::sweep is built and floored; its cadence is an operator
decision nobody has made")`. So no deployment collects blobs today, on any
store.

**B-G1 · "It must not delete another tenant's objects" is already true three
times over, and the requirement is to keep all three.** The anti-join is scoped
by `$space`; the object key is `<space>/<hex handle>` with `space_prefix`
refusing a hostile id rather than sanitising one, which keeps the mapping
injective; and the store is resolved per space. Any one of the three would be
enough and the design has all three on purpose. The requirement is a test that
sweeps a space attached to bucket A while a space attached to bucket B holds
objects, and shows B untouched, plus the existing contract case "spaces do not
share keys" run against both bindings.

**B-G2 · The one way a sweep does delete the wrong tenant's bytes is B-H2's
split population**, and it is not a cross-tenant delete but a same-tenant
orphan: the sweep resolves the current store, `delete` succeeds on an absent
key by design, the tombstone is stamped and the reference is removed while the
bytes sit in the store the object was actually written to. Closing B-H2 closes
this.

**B-G3 · The runner walks two dimensions, and the databases come first.**
`$blobref` lives in the tenant's database and the objects live in the tenant's
store, so a sweep visits every target from `PoolResolver::targets()`, and for
each space on that target resolves that space's store. The database walk is the
outer loop because the reference set is what bounds the work; there is no way
to enumerate from the store side and there must not be one.

**B-G4 · A store the sweep cannot reach stops that tenant's sweep and stamps
nothing.** The code is already right: a `BlobError` from `store.delete` becomes
`SweepError::Blob` and the sweep returns before the `UPDATE`, so a tombstone
stays open and the retry is ordinary. What is owed is the runner's side, which
is §9.4's answer one tier over: a target or a store it could not reach is an
**outcome**, not a panic, and it joins the one per-tenant list of §5 B-F1 with
the reason "store unreachable, objects not collected" and since when. A tenant
whose bucket has been down for a month is a tenant whose storage bill has been
growing for a month, and that is the operator's to see before the tenant's.

**B-G5 · `BlobStore` must keep having no `list`, and a per-tenant bucket is
the moment that gets questioned.** The trait's own doc says why: a store that
could list makes "reconcile the bucket against the database" look cheap, and
that reconciliation is the one operation whose failure mode is deleting bytes a
revert still needs. Per-tenant buckets add two new reasons to want one, and
both have answers elsewhere in this document: "what is really in the tenant's
bucket" is B-H2's split population, closed by refusing the attach; and "what is
the tenant actually paying for" is §9 B-U2, answered from `$blobtomb` without
touching the store.

**B-G6 · The floor is shared with the log, so a lagging consumer becomes a
storage bill.** `cutoff_of` takes the minimum of the undo horizon and whatever
a pinned consumer holds, and `FloorHolder` names the one cause. On the
deployment's own store a mirror consumer stalled for thirty days is a platform
cost nobody sees. On a tenant's own bucket it is that tenant's invoice, for a
reason inside the platform. The requirement is that `FloorHolder` reaches the
**tenant's** own operator surface and not only the deployment operator's, so
the tenant can see why objects they deleted are still being paid for.

**B-G7 · The cadence stays one deployment-wide setting.** See D-S3. A
per-attachment cadence would be N schedules to reason about and would let a
tenant defer their own deletions past whatever retention their disclosure
promised, and the quantity that genuinely varies per tenant is the **cost** of
a sweep, which the floor and the batch size already bound.

## 7 · Key rotation and re-sealing into a tenant's own bucket

**What K1 does, read from the code.** `reseal::move_objects`
(`crates/ordo-exec/src/reseal.rs:1169-1382`) resolves one store for the space,
reads each object under the old epoch, opens every chunk, mints a **new**
handle under the current epoch, seals, verifies in memory before writing
anything, `put`s a new object at `<space>/<new hex handle>` in the **same**
store, inserts a new `$blobref` row, and CAS-updates the entity column. The old
object and the old `$blobref` row are deliberately left alone, so a retained
command image and a revert still resolve, and they clear on `blobgc`'s floor.

**So a re-seal doubles the stored bytes for the length of the undo horizon**,
which defaults to ninety days (`cadence::LogPolicy::DEFAULT`). That is the
fact everything in this section follows from.

**B-R1 · The doubling is not an operator's choice, which is what makes it a
tenant's problem.** Rotation is mandatory on membership revocation. So a
tenant's stored bytes roughly double, for ninety days, because a member left,
with no act by the tenant and no act by the operator. On the deployment's own
store the platform absorbs it and nobody notices. On a tenant's own bucket it
appears on their invoice, and under `OwnPerimeter` it appears there with no
explanation the platform has offered.

**B-R2 · The pass states what it will cost before it starts.** It already knows
the number: `SUM("bytes")` over `$blobref` at the epoch, which is the same
column `usage::bytes_blob` reads. The requirement is that opening a pass on a
space with an attachment records and surfaces the duplicated bytes and the date
the duplicate clears, which is the floor, so the tenant's bill has an
explanation attached to it rather than a discrepancy.

**B-R3 · Who pays and who may delete, per custody.** Under `PlatformSealed`
nothing changes in kind: the platform's credential writes the new object and
the sweep deletes the old one, and the tenant pays for storage they agreed to
hold ciphertext in. Under `OwnPerimeter` the platform still holds a write
credential, because it cannot put an object without one, so it can still
delete, and the sweep must keep running. The disclosure's sentence
*"removing them is yours to do"* is about the objects surviving **erasure**,
and it must not be read as "the platform will not delete anything in your
bucket", because the sweep deletes unreferenced objects and that is what keeps
the tenant's bill bounded. The requirement is that the `OwnPerimeter`
disclosure says both halves: the platform removes objects nothing references
any more, and the platform's key destruction does not make surviving objects
unreadable.

**B-R4 · `destroy` refuses while old objects remain, and per-tenant buckets
make that refusal tenant-visible.** `destroy_gate` counts `$blobref` rows by
epoch (`reseal.rs:487`, `blobref::epoch_count_sql`) and returns
`Gate::Refused { population: Objects, count }` while any survive. Those rows
clear only when `blobgc` retires them, which needs the floor to pass and needs
a runner that does not exist (§6). So today an epoch that carried objects can
**never** be destroyed. That is already true on the deployment's own store, so
a per-tenant bucket does not create it; what a per-tenant bucket creates is a
tenant who can see it, because a tenant who asked for crypto-erasure after a
revocation is told "not yet" for at least the undo horizon and is meanwhile
paying for both copies. The requirement is that `Gate::Refused` names the floor
date rather than only the count, so "not yet" has a when.

**B-R5 · `Gate::Clear` must state custody, because under `OwnPerimeter` there
is a second thing the pass cannot reach.** The confirmation already prints the
one thing it cannot reach, a backup taken before the pass finished. A tenant's
own bucket adds copies the tenant made and, more importantly, **bucket
versioning and object lock**, which keep prior versions a `DELETE` does not
remove. Under `PlatformSealed` that is survivable and worth saying plainly: the
retained version is ciphertext under a destroyed key, so the erasure claim
holds. Under `OwnPerimeter` it is the tenant's own arrangement and the platform
should not claim anything about it. Object lock is also named in
`corpus-rulings.md:4508` as the strongest property in its list and is
unbuilt; nothing here asks for it, and this is the place its absence has to be
disclosed rather than assumed.

**B-R6 · A blocked Objects population is per space and joins the one list.**
`recount` sets Objects to `blocked` when no store answered and never re-opens
it automatically; `ordo keys reseal` re-opens it (`reseal.rs:776-790`). With
per-tenant buckets, one tenant's unreachable bucket blocks one tenant's pass
while every other tenant's completes, which is the right behaviour and is
invisible unless it is reported. It goes in §5 B-F1's list with the reason
"re-seal blocked, store unreachable".

## 8 · Exit and import

**B-E1 · The export reads through the pipeline's own store, so it is the second
place that has to become resolver-aware.** `export::write_blobs`
(`crates/ordo-exec/src/export.rs:669-789`) calls `pipe.store.get(...)` and
`pipe.get(...)`, both of which go to the one store `BlobPipeline` was built
with. Giving the pipeline a resolver (§11 U2) fixes the door and the export
together, which is why they are one increment rather than two.

**B-E2 · "A missing object is a refusal, not an omission" becomes
load-bearing.** The export already says so and already returns
`ExportError::SealedUnopened` rather than a short bundle. With a per-tenant
bucket the common cause of a missing object stops being a bug and becomes an
unreachable endpoint, so the refusal must distinguish "your bucket did not
answer" from "this object is not there", for §5 B-F2's reason: one is a retry
and the other is an incident. Neither text reaches the caller (B-F4); the
distinction is in the operator's copy.

**B-E3 · The bundle's layout does not change.** `blobs/<entity>/<field>/<id>`
stays what `migration.md` §6 specifies, and the bucket's key stays
`<space>/<hex handle>`. The 2026-09-08 ruling 4 already argued they are
different layouts deliberately, and a per-tenant bucket is not a reason to
revisit it: the export can produce the readable layout because it holds the
`$blobref` rows, and that is still true when the rows and the bytes are in
different places.

**B-E4 · A tenant leaving under `OwnPerimeter` should not have to pay to be
handed their own bytes.** An export copies every object out of the tenant's
bucket, through the platform, into a bundle, and under `OwnPerimeter` the
tenant already holds the objects. That is an egress bill and a second copy for
no gain. The manifest already carries `blobs_carried` and `blob_objects`, so the
bytes-free form is expressible today: a bundle carrying the `$blobref` rows,
the entity/field/id mapping and the object keys, stating that the bytes are in
the bucket the tenant holds. The requirement is that the export offers it and
that the manifest says which form was produced, because a bundle whose
`blobs_carried` is false for a schema with blob fields must not read as a bundle
that lost them.

**B-E5 · Erasure's ordering becomes load-bearing.** `erase_space`
(`blobgc.rs:481-510`) deletes every object in the resolved store and then
truncates `$blobref` and `$blobtomb`, and `erase::ATTACHMENTS` includes
`$spacebucket` (`erase.rs:510`). So the attachment row must be deleted **after**
the objects, or the resolver has nothing to resolve and the objects are
orphaned in a bucket with no row naming it. 17g already records this ordering
going wrong twice before it landed inside `erase_space`; with a per-tenant
bucket it is the difference between a deletion and a silent failure. A named
test is owed.

**B-E6 · What `std.space.delete` does under `OwnPerimeter`.** Ruled
2026-09-21 (D-S5): destroy the keys, leave the objects, record what was left.
Built. `blobgc::erase_space` decides per file from the `store` the row
records. An empty name is the deployment's own store, and an attachment under
`PlatformSealed` is the platform's to manage, so both are deleted as before.
An attachment under `OwnPerimeter` is the tenant's: the object is left. It
returns `ErasedObjects { deleted, left }`, and `left` holds one `ObjectsLeft`
per bucket (store as `endpoint|bucket|keyPrefix`, count, epochs, and a
SHA-256 digest of the sorted handles). `ExitOps::delete` writes each as a
`CustodyAction` (`objects_left ...`) beside `space_erased` in the space's
security stream, which erasure retains, and returns it as `objectsLeft`. No
key material and no credential label is in either. Tests:
`an_erase_deletes_files_in_the_deployments_store`,
`an_erase_leaves_files_in_a_tenant_owned_bucket_and_reports_them`,
`an_erase_of_a_tenant_with_files_in_both_stores_does_both`,
`an_erase_still_deletes_from_a_platform_sealed_bucket`
(`crates/ordo-exec/tests/blobgc.rs`) and
`erasing_a_space_leaves_files_in_its_own_bucket_and_records_them`
(`crates/ordo-host/tests/exit_ops.rs`).

**`ordo verify --objects` follows each file's recorded store.** The command
used to build a `SingleStore`, which answers every name with the deployment's
own store, so a file in an attached bucket was reported absent. It now builds
`bucket::recorded_stores`, an `AttachedStores` that resolves a recorded name
without reading `$spacebucket`. It signs with `ORDO_TENANT_CREDENTIALS`. With
no provider, a row that names a bucket is reported unverified. Test:
`verify_objects_follows_the_store_each_file_records`
(`crates/ordo-host/tests/blob_door.rs`).

**B-E7 · Import moves no blob bytes and must keep refusing.**
`ImportError::BlobUnsupported` refuses a bundle carrying objects, with the text
naming 11-10a, and refuses on both the manifest count and any member named
`blobs/` (`crates/ordo-exec/src/import.rs:1036-1049`). That refusal is correct
and a per-tenant bucket does not change it. What a per-tenant bucket adds is
the **one import path that needs no bytes to move**: a tenant arriving with
their objects already in their own bucket attaches that bucket and imports a
bytes-free bundle whose `$blobref` rows already name the handles that are
there. It costs no transfer and no egress. It is exactly B-H2's split
population seen from the other side, so it is available only under D-S2's
second option, and it is the strongest argument for that option.

## 9 · Usage and billing: bytes stored

**B-U1 · Bytes stored is measured from the database and must stay that way.**
`usage::bytes_blob` is `SELECT COALESCE(SUM("bytes"),0) FROM "$blobref" WHERE
"$space" = …` (`crates/ordo-exec/src/usage.rs:452`), an exact sum of a plaintext
`bigint` column, taken by the leader-only `UsageRollup` against each target
pool. It never touches a store. A per-tenant bucket therefore changes **nothing
about how the quantity is obtained**, and measuring it at the provider instead
would mean giving `BlobStore` the `list` §6 B-G5 says it must not have. Stated
plainly because "the tenant has their own bucket, so ask the bucket" is the
obvious wrong turn.

**B-U2 · The number is referenced bytes, and the tenant's invoice is stored
bytes.** Three populations sit between them and all three are knowable from the
database: objects tombstoned and awaiting the floor, which is up to ninety days
of deletions; the duplicate copy an open re-seal pass is holding (§7 B-R1); and
orphans from half-written uploads, which §5 B-F3 turns into ordinary tombstones
precisely so they stop being unknowable. On the deployment's own store the
difference is the platform's and nobody asks. On a tenant's own bucket the
tenant reads their provider's invoice and asks. The requirement is that the
rollup carries the tombstoned-but-not-deleted bytes as a **separate quantity**,
a sum over `$blobref` joined to open `$blobtomb` rows, so the difference is
explainable from the platform's own record rather than being a discrepancy the
tenant discovers.

**B-U3 · The 2026-09-21 ruling applies to `bytes_blob` through the database,
and a gap is owed for the database being unreachable, never for the bucket
being unreachable.** The ruling splits door-side counters from database-side
quantities and names *"bytes from the sealed payloads, the blob references"*
among the latter. So: a tenant's **database** unreachable at rollup means the
measurement did not happen and the period records a gap. A tenant's **bucket**
unreachable at rollup means nothing at all, because the rollup never asked the
bucket. Both halves need saying, because "unreachable storage, therefore a
gap" is the reasonable and wrong inference, and a gap invented for a cause that
did not affect the measurement is as dishonest as a zero.

**B-U4 · `$usage` has no way to say "gap" and that column is shared work.**
The table's columns are `space, period_start, period_end, chain_from, chain_to,
writes, rows_written, bytes_sealed, bytes_chain, bytes_blob,
bytes_rows_estimated, method_version` (`usage.rs:78-88`). An unreachable target
today makes `UsageRollup::run` return an error and write no row
(`workers.rs:609-614`), and a space with nothing new also writes no row
(`:640-645`). So the absence of a row means both "quiet" and "not measured",
which is precisely the ambiguity the ruling exists to remove. One `state`
column distinguishing measured from gap, with the reason, serves `bytes_blob`
and every other database-side quantity at once. It belongs to the database
plan's T9 and is named here so the two are not built twice.

## 10 · Security: credential reach, what the door refuses, what the audit records

*Mirrors §9.8.*

**B-X1 · A credential never signs for another tenant's bucket, and inside one
bucket the prefix is the only separation.** A store is built from one
attachment, so the resolver's cache is keyed by `Attachment::identity` and one
tenant's key pair only ever signs requests at that tenant's endpoint and
bucket. Two tenants pointed at the **same** bucket share one credential and are
separated by the `<space>/` prefix alone. `space_prefix` refusing a
hostile id rather than sanitising one is what keeps that separation injective,
and it is asserted by `a_space_id_cannot_climb_out_of_its_prefix`.

**Superseded in part by D-D (2026-09-25): the attach path may no longer
produce that shared tier.** *"1 tenant 1 database. same for buckets.
database/schema not cluster."* One tenant per **bucket**, so B-X3.2 below is
the general rule rather than a rule about declared perimeters, and the
paragraph above now describes only what the **default** store still is: every
space with no `$spacebucket` row resolves to the one `ORDO_BLOB_STORE` and is
separated from its neighbours by the prefix alone. Whether that must end is
`db-default-tier`'s twin and is not assumed here.

**A per-tenant `keyPrefix` inside one shared bucket does not satisfy the
ruling, and the reason is who enforces the line.** The analogue of
"database/schema not cluster" is that the bucket is the unit and the endpoint
— an account, a gateway, a region — is not: many tenant buckets behind one
endpoint stays the ordinary case. A prefix is tempting to read as what a
schema is to a database, but a Postgres schema is a boundary the **server**
draws through `GRANT`, while a key prefix is enforced only by an IAM policy
that D-S7 says outright the platform cannot enforce, and the credential a
prefix sits behind signs for the whole bucket. So `bucket::occupants` keys on
the endpoint host and the bucket, and never on the prefix.

**B-X2 · The platform's credential never needs `s3:ListBucket`, and saying so
is worth more than any policy the platform could enforce.** `BlobStore` has
`put`, `get`, `head` and `delete`, and no `list`. So the least-privilege policy
for a tenant's credential is `s3:PutObject`, `s3:GetObject` and
`s3:DeleteObject` on `<bucket>/<key_prefix>/<space>/*`, and nothing else. A
credential scoped that way cannot enumerate the tenant's own objects even if it
leaks, and cannot touch anything else in a bucket the tenant also uses. The
platform cannot enforce the tenant's IAM policy, so the requirement is that
this policy is **documented as the one the attachment expects** and that the
attach-time probe (§5 B-F5) is a `head` rather than a `list`, so a correctly
scoped credential passes it. See D-S7.

**B-X3 · The door refuses six attachments. Two are built and four are new.**

*Built.* A credential label the deployment did not install, answered
`not_found` with one message covering both "nobody installed it" and "not for
this space", so the parameter enumerates nothing. A target whose shape
`S3Config::new` rejects, answered `param_type_mismatch` at attach time rather
than at the first upload.

*New.*

1. **The deployment's own default bucket and endpoint.** A tenant pointed at
   the platform's own store, with an attachment row telling them and their
   disclosure page that the bytes are in a bucket they hold. This is
   "attaching to the control database" one tier over.
2. **A bucket declared as another tenant's own perimeter.** Written here as
   a rule about `OwnPerimeter`, briefly widened on 2026-09-25 to refuse any
   shared bucket, and **narrowed back the same day** by the owner's
   clarification of D-D: *"1 bucket can have folders so 1 bucket defined per -
   but it could be the same and bucketname/tenant would be the option."*

   A **bucket** is the analogue of a **cluster** and may be shared; a
   **folder** is the analogue of a **database**. `ordo_blob::object_key`
   already keys every object `<space>/<handle>`, so two tenants in one bucket
   cannot collide whatever prefix either states, and the widened refusal was
   guarding against something the layout had already made impossible. What
   remains is D-S8's own question, which a folder does not answer because it
   is about a disclosure rather than a collision: refused in **both**
   directions, a tenant arriving into a declared perimeter and a tenant
   claiming one over a bucket somebody is in.
3. **A space that already holds `$blobref` rows**, per §4 B-H2 and D-S2. *Lifted 2026-10-06 (`st-attach-existing` ask 2): files already written stay in the bucket they were written to.*
4. **An endpoint off the operator's allowlist, refused with one reason.** This
   is `ordo-egress`'s argument and it is **sharper here than on the database
   side**, because an S3 dial is an HTTP request to a host chosen by data,
   which is the exact shape `ordo-egress` exists to refuse, and because
   `S3Config::new`'s existing check has a hole: it permits plaintext HTTP to
   anything `is_local` accepts, which is `localhost`, `127.0.0.1`, `::1`,
   `*.localhost` and the bare name `minio` (`s3.rs:166-172`). In a pod, a
   request to `http://minio/` or `http://127.0.0.1:9000/` is a request into
   the deployment's own network, and an `https://169.254.169.254` endpoint
   passes the TLS check and reaches the cloud metadata service. The
   requirement is an operator-stated allowlist of hosts applied
   before the row is written, one refusal reason and not several, **and** the
   loopback exception gated behind the same kind of dev flag `fs:` already
   needs. See D-S6.

   **It is not an allowlist of its own.** D-S6 as ruled: *"yes but should be
   tied in with the db allowlist setup. implication really is if i defined it
   on my platform it is allowed."* So the list is
   `ordo_host::provision::HostAllowlist` — the type the database tier built —
   and both tiers check against the same one. `from_env` reads
   `ORDO_TENANT_ATTACH_HOSTS` and `ORDO_PG_ATTACH_HOSTS` and **unions** them:
   the second spelling is the one 18d T8 shipped and the decisions board
   already names, and a precedence would let an operator who added the first
   silently switch the second off. Hosts rather than CIDRs, which is what the
   database tier built; a CIDR form is one parse away and nothing here depends
   on the difference.

**B-X4 · What the door must never return. Six, each with a test.**

1. **No surface returns an access key id or a secret access key.** No type in
   `$spacebucket`'s path holds one, and the type that does, `S3Config`, must
   redact both from `Debug` the way `PoolConfig` redacts its password. It does
   not today: `S3Config` derives `Debug` with both fields (`s3.rs:72`). That is
   a `{:?}` away from a credential in a log line and it is the smallest fix in
   this document.
2. **No surface returns the credential label to a tenant.** `BucketOps::attach`
   returns it today (`bucket_ops.rs:184`). It is deployment-side configuration
   naming material installed on the node, and a space owner has no use for it.
   Returnable to a deployment administrator.
3. **`endpoint`, `region`, `bucket` and `key_prefix` are returnable to a
   deployment administrator and to the tenant whose attachment it is**, because
   it is their bucket and they already know it. Never to another tenant.
4. **A store error's text is never passed through to the caller.**
   `BlobError::Transport` and `Refused` carry the provider's body, which can
   name the credential's principal, the bucket and the account. The crate's own
   doc already says the string is the operator's and never the caller's; the
   requirement is the test.
5. **The existence of an attachment is not a public fact**, for R-C4.5's
   reason: otherwise the attachment table is an oracle for which tenants exist
   and which of them pay for their own storage.
6. **An object's stored length stays plaintext and that is not a leak.**
   `$blobref."bytes"` is plaintext and `ObjectHead` returns it, and the reason
   is written in the tree: it is *"already observable to anyone who can list
   the bucket"*. Under `OwnPerimeter` that party is the tenant, who owns the
   object. Recorded so the next reader does not seal it and break the usage
   rollup for nothing.

**B-X5 · Occupancy is recorded and exposed, and the developer writes the
sentence.** Under the 2026-09-21 ruling the platform's obligation is the fact
and not the prose: which store a space resolves to, and whether anything else
resolves there, readable by the operator and by the developer whose
application it is. `BucketCustody::disclosure` stays what it is, a sentence
about erasure, and it is the developer who tells the person whose data it is
whether they are alone in a bucket. The refusal of B-X3.2 is the platform's
half, because a second tenant inside a perimeter the first tenant was told was
theirs is not a disclosure problem the developer can fix after the fact.

**B-X6 · What the audit records, and the two streams owe each other the
opposite halves.** Both halves are **security** streams. The tenant's own
**chain** records nothing about where its data was put: D-F, ruled
2026-09-21, *"theres no provability unless its byo db… I think this belongs to
the app not ordo"*, so there is no founding group and no command group, which
is also what this op has always done — it writes no entity row, which is
`plan-export.md` §5.1's reinterpretation of "command-logged".

**Amended 2026-09-29: both halves are now one.** The custody event is
recorded on the **deployment** stream only. The earlier text here asked for a
copy on the tenant's own stream as well, so a tenant could show from its own
records where its data went. The owner ruled against that. The deployment's
stream is the one place that lists custody changes across every tenant, and
it names the space as the event's object. The event must name
the tenant's minted id, the target's identity as endpoint, region and bucket,
the credential **label** and never its value, the custody choice, the occupancy
B-X5 records, and whether the call attached or replaced. `AttachOutcome::
Unchanged` continues to record nothing, which is already right: a retry is not
a custody change, and neither is a key rotation behind a stable label (§3
B-K4).

**`ordo_secevent::route` sends one observation to exactly one scope.** The
deployment's copy names the platform and carries the space as the event's
*object* (`StreamEntry::about`), which is what makes one listing say which
tenant it was.

## 11 · Increments, smallest first

Every one is behaviour-identical with `ORDO_BLOB_ROUTING` unset until U5, for
§10's reason on the database side: the wiring lands and is proved to change
nothing before the switch that changes something exists. The `T` numbers below
are `plan-space-routing.md` §10's.

**Where `AttachedStores` lives, decided here so the increments can name it.**
`ordo-blob` must not gain a Postgres dependency; it is the crate that exists to
keep an HTTP client and a TLS stack in one place. `ordo-exec` already depends on
`ordo-blob` (`blobgc` takes a `&dyn StoreResolver`) and already holds
`bucket.rs`. So `AttachedStores` implements `ordo_blob::StoreResolver` inside
`ordo_exec::bucket`, beside the table it reads, exactly as `AttachedPools` sits
in `ordo_exec::route` beside `$spacedb`.

**U0 · Pin the three properties that hold by accident.** Tests only, no
production change. *Files:* `crates/ordo-blob/src/contract.rs`,
`crates/ordo-host/tests/blob_door.rs`. *Tests:*
`a_store_error_is_never_returned_to_the_caller`,
`the_object_key_is_the_space_and_the_handle_and_nothing_else` (promote the
existing unit assertion into the contract suite so both bindings run it),
`spaces_with_different_stores_do_not_share_keys`. *Done when* all three pass
and nothing else changed. *Tier: Sonnet.*

**U1 · The two credential leaks, and the two columns.** §10 B-X4.1 and B-X4.2,
plus §2 B-C2. `S3Config` redacts both credential fields from `Debug`, written
out rather than derived, on `PoolConfig`'s precedent. `BucketOps::attach` stops
returning the credential label. `$spacebucket` grows nullable `path_style` and
`key_prefix`, with `NULL` and empty meaning today's behaviour so no stored row
changes meaning, and `key_prefix` refused on `space_prefix`'s charset.
*Files:* `crates/ordo-blob/src/s3.rs`, `crates/ordo-exec/src/bucket.rs`,
`crates/ordo-host/src/bucket_ops.rs`, `crates/ordo-host/src/platform_ops.rs`.
*Tests:* `an_s3_config_never_prints_its_credential`,
`the_attach_response_never_names_the_credential_label`,
`an_attachment_can_state_path_style_its_gateway_requires`,
`a_hostile_key_prefix_is_refused_rather_than_sanitised`,
`an_attachment_written_before_these_columns_reads_back_unchanged`.
*Done when* all five pass. *Tier: Sonnet.*

**U2 · A half-written upload becomes a tombstone instead of an orphan.** §5
B-F3. Independent of everything else here and of the database plan entirely.
It makes a leak that exists on every deployment today **collectable**, which
U8's runner is then what collects; without U2 the orphan stays unreachable
even after the sweep ships, because nothing can enumerate it. So it should land
before U8 and it can land first if only one thing lands. The door writes a
`$blobtomb` row for the minted handle before the `put`; the sweep's existing
phase-2 re-check cancels it when the reference commits. *Files:*
`crates/ordo-host/src/blob.rs`, `crates/ordo-exec/src/blobgc.rs`. *Tests:*
`a_failed_upload_leaves_a_tombstone_not_an_orphan`,
`a_committed_upload_cancels_its_own_tombstone`,
`the_sweep_retires_an_abandoned_upload_once_the_floor_passes`. *Done when* an
upload interrupted between the object write and the transaction leaves nothing
the sweep cannot reach. *Tier: Opus*, because it sits inside the write path and
the sweep's crash-safety argument.

**U3 · The pipeline and the export take a resolver.** §1's fifth gap and §8
B-E1. `BlobPipeline` holds a `StoreResolver` rather than a `BlobStore`, and
`export::write_blobs` reads through it. This is the seam the database side
already has in `Spaces::pool_of`, and building it is what makes every later
increment wiring rather than surgery. *Files:*
`crates/ordo-blob/src/pipeline.rs`, `crates/ordo-host/src/blob.rs`,
`crates/ordo-exec/src/export.rs`, `crates/ordo-host/src/exit_ops.rs`,
`crates/ordo-host/src/runtime.rs`. *Tests:*
`every_blob_route_resolves_the_spaces_own_store`,
`an_export_reads_the_spaces_own_store`,
`a_deployment_with_one_store_behaves_identically`, plus the full suite.
*Done when* no `Arc<dyn BlobStore>` is held anywhere outside a resolver.
*Tier: Opus.*

**U4 · The credential provider, shared with the database. Must ship with T3.**
§3 B-K1 to B-K5. One `ORDO_TENANT_CREDENTIALS` selector and one `dir`
provider serving both `$spacedb` and `$spacebucket`: a label resolves to a file
(a password, empty meaning passwordless) or to a directory named by the label
holding `access_key_id` and `secret_access_key`. Refuses at boot on a group- or
world-readable mode, reads the whole directory at boot, and reports the labels
it holds and never their values. **This must ship with T3 rather than after
it**, because two providers would be two rotation procedures, two boot
refusals and two label lists on the same node, and §9.7 C-M3 requires one
checkable list before a move copies anything. *Files:* a new
`crates/ordo-exec/src/creds.rs` (or `route.rs` beside `Installed`),
`crates/ordo-host/src/runtime.rs`, `crates/ordo-host/src/bucket_ops.rs`.
*Tests:* `a_bucket_label_resolves_to_a_key_pair`,
`a_database_label_resolves_to_a_password`,
`a_world_readable_credential_refuses_at_boot`,
`a_label_with_no_material_refuses_before_any_request`,
`the_health_surface_names_labels_and_never_values`. *Done when* an operator can
install both kinds of credential the same way. *Tier: Sonnet.*

**U5 · `AttachedStores` and the switch.** `ORDO_BLOB_ROUTING` with `single` and
`attached`, refusing an unknown value and refusing `attached` with no
credential provider. `AttachedStores` reads `$spacebucket`, caches by
`Attachment::identity`, and resolves the deployment's default store for a space
with no row. *Files:* `crates/ordo-exec/src/bucket.rs`,
`crates/ordo-host/src/runtime.rs`. *Tests:*
`an_unknown_blob_routing_value_refuses_at_boot`,
`attached_blob_routing_without_a_credential_provider_refuses`,
`a_deployment_that_attaches_nothing_behaves_identically`, and the end-to-end
`an_attached_tenant_stores_and_reads_its_bytes_in_its_own_bucket`, which puts
one tenant on a second real bucket and shows its uploads, reads, exports and
sweep all land there while a tenant on the default is untouched.
*Done when* that end-to-end test passes. *Tier: Opus.*

**U6 · The attach op is registered, with its refusals and its record.
Built 2026-09-24.** §10 B-X3 and B-X6, and it is what removes the
unconditional line from the unavailability census. The refusals; the
attach-time probe that proves the credential signs; the custody event naming
the target and the label and never the value, on **both security** streams;
occupancy recorded and exposed. *Files:*
`crates/ordo-host/src/runtime.rs` (`platform_specs`, `buckets_wired`, the
census and the wiring), `crates/ordo-host/src/bucket_ops.rs`,
`crates/ordo-exec/src/bucket.rs`, and — not foreseen here —
`crates/ordo-api/src/error.rs` plus `docs/api-engine-design.md` for the
`bucket_refused` class, and `crates/ordo-host/src/provision.rs` for the shared
allowlist.
*Tests:* `attaching_the_deployments_own_bucket_is_refused`,
`a_second_tenant_in_one_bucket_is_refused_whatever_the_custody_says`
(renamed from `a_second_tenant_on_an_own_perimeter_bucket_is_refused`, because
D-D made the old name describe a narrower property than the code has),
`an_endpoint_off_the_allowlist_is_refused_with_one_reason`,
`a_loopback_endpoint_needs_the_dev_flag`,
`an_attach_probes_the_credential_before_the_row_is_written`,
`attaching_a_space_that_already_holds_objects_is_refused` (per D-S2; *renamed `..._is_allowed` 2026-10-06, `st-attach-existing`*),
`the_custody_event_reaches_the_tenants_stream_and_the_deployments`, plus
`the_attach_bucket_op_is_registered_only_where_an_attachment_is_honoured`,
`both_attach_host_variables_are_one_list`,
`one_endpoint_has_one_spelling_however_it_is_written`,
`a_refused_probe_is_the_credentials_fault_and_an_unreachable_one_is_not` and
`the_deployments_own_bucket_is_recognised_however_the_endpoint_is_spelt`.
*Done when* attaching a bucket is something an operator does through the door.
*Tier: Opus.*

**"The six refusals" is seven checks.** B-X3 counts six; D-S6's plaintext
gate is not one of them, so the code has both. And the D-S2 check is skipped
for an attachment identical to the one standing — otherwise a retry of a live
attachment would be refused the moment the tenant uploaded anything, which is
the opposite of the idempotency the op is keyed on. The four
parameter-only checks still run on a retry, because a retry may not become a
way to learn that an endpoint is permitted. *The D-S2 check itself was lifted 2026-10-06 (`st-attach-existing` ask 2).*

**What U6 did not build, recorded rather than left as drift.**
`AttachedStores::forget` still has no caller. U5 built it for the node that
has just attached, and wiring it means plumbing the concrete resolver type
into `BucketOps`; it is not in this done-when and the 30-second TTL already
bounds the staleness U5 argued was acceptable. D-S2's carve-out — *"unless
everything matches i.e. a 1 for 1 migration would be indistinguishable"* — is
also not built: proving that means a `head` per `$blobref` row against the new
bucket, since there is no `list` by design, and neither the cost bound nor the
partial-match behaviour is specified. Both are the owner's to schedule. *The carve-out is moot for the refusal since 2026-10-06, when the refusal was lifted (`st-attach-existing` ask 2). It would now be part of the separate, unbuilt step that moves old files.*

**U7 · Failure modes, and the tenant joins the one list. Must ship with T5.**
§5. The blob routes refuse while the rest of the tenant keeps serving; a
store `NotFound` with a `$blobref` row is never a 404; a store error's text
never leaves; a failing store backs off; and the entry joins the single
per-tenant list `/health/ready` assembles. **With T5 rather than after it**,
because T5 builds that list from four sources and adding a fifth afterwards
means building the assembly twice. *Files:*
`crates/ordo-host/src/blob.rs`, `crates/ordo-host/src/spaces.rs`,
`crates/ordo-host/src/serve.rs`, `crates/ordo-exec/src/bucket.rs`. *Tests:*
`an_unreachable_store_refuses_the_blob_routes_and_not_the_rest`,
`a_missing_object_with_a_live_reference_is_never_a_404`,
`health_ready_names_a_tenant_whose_store_is_unreachable`,
`a_down_store_is_dialled_once_per_backoff_not_once_per_request`.
*Done when* all four pass. *Tier: Opus.*

**U8 · The sweep gets its runner, and it walks both dimensions. After T2.**
§6, and it fills `workers.rs`'s one remaining `Slot`. The outer loop is
`PoolResolver::targets()` and the inner is each space's resolved store; an
unreachable target or store is an outcome; the cadence is one deployment-wide
setting (D-S3). **After T2** rather than with it, because the runner needs the
single resolver T2 hands to the four consumers, and running it against a
`SinglePool` would collect only the default database's tenants while reporting
a complete sweep. *Files:* `crates/ordo-host/src/workers.rs`,
`crates/ordo-host/src/supervisor.rs`, `crates/ordo-exec/src/blobgc.rs`.
*Tests:* `the_sweep_visits_every_target_and_every_store`,
`a_sweep_of_one_tenant_leaves_another_tenants_bucket_untouched`,
`a_store_it_cannot_reach_stamps_no_tombstone`,
`the_floor_holder_names_one_cause_across_targets`.
*Done when* a deleted blob in an attached tenant is gone from that tenant's
bucket after the floor. *Tier: Opus.*

**As built (2026-10-04, 2026-10-05).** `workers::BlobGc`, on
`ORDO_BLOB_GC_SECS` (default 3600). Each pass writes **one security entry per
tenant that had something deleted or was stopped**, in that tenant's own stream (ruled
2026-10-05): a `CustodyAction` by `Subject::platform("blob.gc")`, op
`blob.gc`, action `blob_collected deleted=N not_deleted_store=N
not_deleted_database=N pass=<id>`, one correlation per pass. A pass that
stopped writes its entry even with `deleted=0`, so storage that refused
every delete is on the record (ruled 2026-10-05). A tenant with nothing due
gets no entry. A store that fails partway stops that tenant's
part of the pass, keeps the count of what was deleted, and names the cause.

**U9 · Usage and the operator surface. Ships with T9.** §9. `$usage` gains the
`state` column that distinguishes measured from gap, which serves every
database-side quantity and not only `bytes_blob`; the tombstoned-but-not-
deleted bytes become a separate quantity so a tenant's invoice is explainable;
`FloorHolder` reaches the tenant's surface; and the operator's per-tenant view
gains which store each tenant resolves to, its occupancy and its custody,
beside the six facts R-O6 already lists. *Files:*
`crates/ordo-exec/src/usage.rs`, `crates/ordo-host/src/workers.rs`,
`crates/ordo-cli/src/operator.rs`. *Tests:*
`an_unreachable_target_leaves_a_gap_not_a_zero`,
`an_unreachable_bucket_leaves_no_gap_because_nothing_was_measured`,
`the_rollup_reports_bytes_awaiting_collection_separately`,
`the_operator_surface_names_each_tenants_store_and_its_occupancy`.
*Tier: Sonnet.*

**U10 · The record.** This document's answers into `roadmap-checklist.md` 18p,
`corpus-rulings.md` and the decisions board. *Tier: Sonnet.*

**Which must ship with a database increment, in one place.**

| Storage increment | Ships with | Why |
|---|---|---|
| U4, the credential provider | **T3** | one provider, or two rotation procedures and two label lists on one node |
| U7, failure modes | **T5** | one per-tenant list assembled once, from five sources rather than four |
| U9, usage and the surface | **T9** | one `state` column on `$usage`, one operator view |
| U8, the sweep's runner | **after T2** | it needs the single resolver, or it sweeps only the default database while reporting a complete pass |

U0, U1, U2 and U3 are independent of the database plan entirely and can land in
any order beside it. U2 is the one worth landing whatever else happens, because
it makes a leak that exists today on every deployment collectable at all, and
it must precede U8 for that to matter.

## 12 · Owner decisions

**D-S1 · One routing switch for both tiers, or two?**
*Recommendation: two, `ORDO_PG_ROUTING` and `ORDO_BLOB_ROUTING`.* §4 B-H3 shows
all four combinations are real, and the two tiers will not be ready in the same
release: the database side has T0 to T5 close to switchable and the storage
side needs U3 before anything resolves. One switch would make the earlier tier
wait for the later one. The cost is that "this tenant is self-hosted" stops
being one fact, which §10 B-X5's recorded occupancy and custody answers by
making both facts visible in one place.

**D-S2 · Does an attach refuse a space that already holds objects, or does
`$blobref` learn which store each object went to?**
*Recommendation: refuse, for v1.* Today the attach op promises the opposite in
`existingObjects`, and §4 B-H2 shows what that promise costs: `blobgc` deletes
against the wrong store, succeeds because an absent-key delete is idempotent by
design, removes the reference and leaves bytes nothing can ever name, and
`erase_space` reports a deletion that did not reach them. Refusing is
`erase::rows_held`'s precedent, it is one query, and it makes the population a
migration rather than a side effect, which is what 18e already ruled for
databases. The alternative, a `store` column on `$blobref` resolved per row, is
the right thing **later**: it is what a move will need, and §8 B-E7 shows it is
what makes a tenant arriving with a full bucket cheap. It should not ship
before there is an attach at all.
*Superseded 2026-10-06 (`st-attach-existing` ask 2): the `store` column shipped with 25ac, and the refusal was lifted. Old files stay in the bucket they were written to.*

**D-S3 · Is `blob.gc`'s cadence one deployment-wide setting?**
*Recommendation: yes, and this also closes 17g's open cadence decision.* The
slot has been waiting on an operator decision since it was built. A
per-attachment cadence would be N schedules to reason about and would let a
tenant defer their own deletions past whatever retention their disclosure
promised. What genuinely varies per tenant is the cost of a sweep, and the
floor and the batch size already bound that.

**D-S4 · Do bucket credentials support a role rather than a key pair in v1?**
*Recommendation: no, a key pair only.* `S3Store` signs SigV4 over a static
pair, and a role means an STS call, a third `x-amz-security-token` header in
the canonical request, and an expiry refreshed underneath live requests. All
three are additions inside `s3.rs`, so nothing about `$spacebucket`, the
resolver or the audit trail changes when a role arrives, and §3 B-K2's provider
already has the shape that would return a triple with an expiry. Worth saying
plainly to the tenants it affects: a tenant on AWS attaching their own bucket
creates an IAM user with the policy of §10 B-X2, not a role.

**D-S5 · Under `OwnPerimeter`, does `std.space.delete` delete the tenant's
objects?**
*Recommendation: no. Destroy the keys, leave the objects, and report how many
were left and where.* `erase_space` deletes every object in the resolved store
unconditionally today, and the `OwnPerimeter` disclosure says the objects are
the tenant's and removing them is theirs to do. Both cannot be right, and the
disclosure is the half the tenant has already been shown. It is also the half
that makes sense of the custody choice existing at all: under `PlatformSealed`
the erasure is the key destruction and the objects become unopenable wherever
they sit, and under `OwnPerimeter` the erasure is the key destruction and the
objects stay readable because the tenant holds them. A platform reaching into a
perimeter it was told is the tenant's, to delete irreversibly, is the wrong
default. The alternative, if you would rather nothing be assumed, is a required
parameter on the exit op when the space is attached under `OwnPerimeter`, on
the `platformSealed: bool` precedent where `required: true` forces the caller
to state a position.

**D-S6 · Is an attach-time endpoint allowlist required for v1, and does the
loopback exception move behind a dev flag?**
*Recommendation: yes to both.* An attachment's endpoint is an HTTP destination
chosen by data, which is exactly what `ordo-egress` exists to refuse, and
`S3Config::new`'s current check permits plaintext HTTP to `localhost`,
`127.0.0.1`, `::1`, `*.localhost` and the bare name `minio`, while an
`https://169.254.169.254` endpoint passes it outright. The self-host story
genuinely needs a local gateway, which is why the exception exists; a dev flag
keeps it for the laptop and removes it from the pod, which is the same shape
`fs:` already uses. One refusal reason and never several, because a
distinguishable reason is an oracle.

**D-S7 · Does the platform publish the least-privilege bucket policy its
credential expects, with no `s3:ListBucket`?**
*Recommendation: yes.* It is a public commitment about what the platform's
credential can do inside a tenant's bucket, which makes it the owner's rather
than an implementer's. It costs nothing, because `BlobStore` has no `list` and
no future binding may add one (§6 B-G5), and it turns a leaked attachment
credential into one that cannot enumerate even its own tenant's objects. It
also gives the attach-time probe its shape: a `head`, never a `list`, so a
correctly scoped credential passes.

**D-S8 · Is a second tenant on an `OwnPerimeter` bucket refused, and does the
platform record and expose occupancy?**
*Recommendation: yes to both*, as the 2026-09-21 ruling now shapes it. The
platform records and exposes which store a space resolves to and whether
anything else resolves there, and the developer writes the sentence their
product uses. The refusal is the half a developer cannot write their way out
of: a second tenant placed inside a perimeter the first tenant was told was
theirs is not a disclosure that can be corrected afterwards. This is D-D one
tier over, and answering the two the same way is most of what keeps the two
plans one design.
