//! BYO bucket: the per-space attachment, and the resolver that reads it.
//!
//! `definition-language.md` §2, Blob storage pipeline pin (1):
//! *"**attaching or changing a space's BYO bucket (or BYO database) is
//! itself a granted operation** — space-owner consent, step-up gated,
//! command-logged; storage custody never changes silently, and the
//! platform's credential for the target lives behind the same resolver
//! pattern as keys (`KeyProvider`-style, never in definitions)."*
//!
//! # The credential does not ride the op
//!
//! That sentence is the reason this table has no secret in it. An
//! attachment names a **credential reference** — a label the operator
//! installed out of band — and never an access key. Three things follow,
//! and the third is the one that decides it:
//!
//! 1. A secret in an op parameter is a secret in the request envelope,
//!    which is a secret in the idempotency cache and in every proxy log
//!    between the caller and the door.
//! 2. It would be a secret in the **command log**, which is plaintext by
//!    design — the audit spine is not a vault, and putting one there makes
//!    "the log is readable" a security incident rather than a property.
//! 3. Rotating a credential would otherwise be a schema-level act: a new
//!    key would mean a new attachment command, and the old key would stay
//!    in the chain forever. With a reference, the operator rotates what the
//!    reference points at and the space's attachment does not move.
//!
//! # The custody choice is the disclosure
//!
//! §2 again: *"BYO buckets get the BYO-DB custody choice: platform-sealed
//! (their bucket holds ciphertext they can't read alone) or their own
//! perimeter."* [`BucketCustody`] is that choice, and it is stored because
//! the consent page has to state which one was taken. It is not a
//! performance knob and it is not reversible without re-sealing every
//! object: a space whose bucket holds ciphertext it cannot read alone has
//! a different answer to "what happens when you leave" than one whose
//! bucket is its own perimeter.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ordo_blob::{BlobError, BlobStore, S3Config, S3Store, StoreResolver};
use ordo_crypto::custody::SpaceId;

use crate::creds::BucketCredentials;
use crate::{sql_lit, Pool, PoolError};

pub const BUCKET_TABLE: &str = "$spacebucket";

/// One row per space, replaced in place: a space has one bucket or the
/// deployment's default, never two.
///
/// `attached_at` and `attached_by` are here because the consent page and
/// the security stream both answer *who changed storage custody, and
/// when*, and an answer assembled from the event stream alone would be
/// gone the first time the stream was pruned.
///
/// There is **no secret column**, and the absence is asserted by a test
/// that reads this string. See the module doc.
///
/// `path_style` and `key_prefix` arrived after the table shipped
/// (`docs/plan-tenant-storage.md` §2 B-C2, U1). There is no migration
/// machinery for platform tables — `bucket::ensure_table` runs `CREATE
/// TABLE IF NOT EXISTS` on every boot and stops there, the same limit
/// `$secevents."about"` and `$attestdeliveries."head"` already worked
/// around — so each column needs its own `ALTER TABLE ... ADD COLUMN IF
/// NOT EXISTS`, idempotent the same way the table's own creation is. Both
/// are nullable with no default: a row written before these columns
/// existed has nothing to backfill, and `NULL` says exactly "derive as
/// today" rather than a sentinel pretending to a choice nobody made.
pub const BUCKET_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$spacebucket\" ( \
  \"$space\" text NOT NULL PRIMARY KEY, \
  \"endpoint\" text NOT NULL, \
  \"region\" text NOT NULL, \
  \"bucket\" text NOT NULL, \
  \"credential\" text NOT NULL, \
  \"custody\" text NOT NULL, \
  \"attached_at\" bigint NOT NULL, \
  \"attached_by\" text NOT NULL \
); \
ALTER TABLE \"$spacebucket\" ADD COLUMN IF NOT EXISTS \"path_style\" boolean; \
ALTER TABLE \"$spacebucket\" ADD COLUMN IF NOT EXISTS \"key_prefix\" text";

/// Who can read the objects, stated so a consent page can state it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketCustody {
    /// The platform seals before the bytes reach the bucket. The space's
    /// own bucket holds ciphertext it cannot read alone — which is the
    /// stronger position and the one that keeps crypto-erasure meaningful,
    /// because destroying the DEK makes the objects unopenable wherever
    /// they sit.
    PlatformSealed,
    /// The bucket is the space's own perimeter: they hold the objects and
    /// the platform's erasure of their keys does not reach into it.
    ///
    /// **This changes what crypto-erasure means**, exactly as BYO-DB does,
    /// and that is the whole reason the choice is recorded rather than
    /// inferred.
    OwnPerimeter,
}

impl BucketCustody {
    pub fn as_str(self) -> &'static str {
        match self {
            BucketCustody::PlatformSealed => "platform-sealed",
            BucketCustody::OwnPerimeter => "own-perimeter",
        }
    }

    pub fn parse(s: &str) -> Option<BucketCustody> {
        match s {
            "platform-sealed" => Some(BucketCustody::PlatformSealed),
            "own-perimeter" => Some(BucketCustody::OwnPerimeter),
            _ => None,
        }
    }

    /// The sentence a consent page prints. Written here rather than in the
    /// UI, because a disclosure assembled by a renderer is a disclosure
    /// that differs per renderer.
    pub fn disclosure(self) -> &'static str {
        match self {
            BucketCustody::PlatformSealed => {
                "Files are sealed by the platform before they reach your bucket. Your \
                 bucket holds ciphertext you cannot read without the platform's keys, and \
                 deleting this space destroys those keys — so the objects become \
                 permanently unreadable wherever they sit."
            }
            BucketCustody::OwnPerimeter => {
                "Files are stored in your bucket under your own perimeter. Deleting this \
                 space destroys the platform's keys, which does NOT make these objects \
                 unreadable — they are yours, and removing them is yours to do."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// A **label**, never a secret. See the module doc.
    pub credential: String,
    pub custody: BucketCustody,
    /// `https://host/bucket/key` rather than `https://bucket.host/key`,
    /// stated by the space rather than derived from the endpoint.
    ///
    /// `S3Config::new`'s derivation — path style on for a non-TLS or
    /// IP-literal endpoint, virtual-host style otherwise — is right for
    /// the deployment's own store, where the operator controls the
    /// endpoint, and wrong for an attachment: a self-hosted MinIO or Ceph
    /// behind TLS on a real hostname needs path style, the derivation
    /// gives it virtual-host style, and the failure is a DNS lookup for
    /// `<bucket>.<host>` that does not resolve. `None` means "derive as
    /// `S3Config::new` does", so a row written before this field existed
    /// reads back with the identical meaning it had before. See
    /// `docs/plan-tenant-storage.md` §2 B-C2.
    pub path_style: Option<bool>,
    /// A prefix applied ahead of the space segment in every key this
    /// attachment's objects live under, so a tenant's own bucket — which
    /// they may also use for other things — can be told where this
    /// platform's objects are allowed to live. `None` (and, in storage,
    /// the empty string) means today's layout: `<space>/<hex handle>`
    /// with nothing ahead of it. Refused on
    /// [`ordo_blob::space_prefix`]'s charset at the door, for its own
    /// reason restated one level up: a sanitised hostile prefix can
    /// collapse two different inputs onto one, and the prefix is exactly
    /// what stands between this attachment's objects and whatever else
    /// lives in the bucket.
    pub key_prefix: Option<String>,
    pub attached_at: i64,
    pub attached_by: String,
}

impl Attachment {
    /// The identity a repeat is keyed on.
    ///
    /// **This is how the idempotency gap is closed here rather than
    /// inherited.** `exit_ops` records that `PlatformExecutor` drops the
    /// durable `ReplayLease`, *"safe here only because both exit ops are
    /// idempotent by construction"* — an export is a pure function of
    /// state and an erasure of an erased space does nothing. Attaching a
    /// bucket is **not** idempotent by construction: run twice with
    /// different arguments it changes storage custody twice, and a crash
    /// between reservation and response would leave a caller unable to
    /// tell which attachment stands.
    ///
    /// So the op is keyed on the target's identity: an attachment whose
    /// identity equals the one already stored is a **no-op that reports
    /// success**, and one that differs is a genuine change with its own
    /// record. A repeat is then safe without the durable lease, for a
    /// stated reason rather than by inheritance.
    ///
    /// `attached_at` and `attached_by` are excluded, and that is the
    /// point: a retry an hour later by the same owner is the same
    /// attachment, and treating it as a change would put a false custody
    /// event in the space's stream every time a client retried.
    ///
    /// `path_style` and `key_prefix` are **included**: either one changes
    /// where in the bucket this attachment's objects are read from, which
    /// is the same kind of fact `bucket` and `endpoint` already are, and
    /// leaving them out would make restating only `$keyPrefix` read as a
    /// retry of the same attachment rather than the change it is.
    pub fn identity(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.endpoint,
            self.region,
            self.bucket,
            self.credential,
            self.custody.as_str(),
            self.path_style
                .map(|b| if b { "1" } else { "0" })
                .unwrap_or("-"),
            self.key_prefix.as_deref().unwrap_or(""),
        )
    }

    /// The name recorded on each `$blobref` and `$blobtomb` row written to
    /// this bucket (ruled 2026-10-05, `detach-upload-race` ask 2).
    ///
    /// It carries **everything that addresses the object** and nothing
    /// else: the fields [`Attachment::identity`] covers, so a name read
    /// back resolves to the same cached store the live attachment does.
    /// `attached_at` and `attached_by` are left out for `identity`'s own
    /// reason. The credential is the **label**, never key material, and it
    /// is the label the tenant owner supplied on attach.
    ///
    /// It has to be complete because the attachment row may be gone by the
    /// time the object is read. A detach deletes `$spacebucket`'s row and a
    /// replace overwrites it, and an object written to the old bucket in
    /// that window is still there. The name is the only record of where.
    ///
    /// JSON rather than [`Attachment::identity`]'s `|`-joined form, because
    /// this one is parsed back and an endpoint is free text.
    pub fn store_name(&self) -> String {
        serde_json::json!({
            "endpoint": self.endpoint,
            "region": self.region,
            "bucket": self.bucket,
            "credential": self.credential,
            "custody": self.custody.as_str(),
            "pathStyle": self.path_style,
            "keyPrefix": self.key_prefix,
        })
        .to_string()
    }

    /// The attachment a recorded [`Attachment::store_name`] describes, or
    /// `None` when it does not parse. `attached_at` and `attached_by` come
    /// back empty: they are not part of where an object is.
    pub fn from_store_name(name: &str) -> Option<Attachment> {
        let v: serde_json::Value = serde_json::from_str(name).ok()?;
        let text = |k: &str| v.get(k)?.as_str().map(str::to_string);
        Some(Attachment {
            endpoint: text("endpoint")?,
            region: text("region")?,
            bucket: text("bucket")?,
            credential: text("credential")?,
            custody: BucketCustody::parse(&text("custody")?)?,
            path_style: v.get("pathStyle").and_then(|b| b.as_bool()),
            key_prefix: text("keyPrefix").filter(|p| !p.is_empty()),
            attached_at: 0,
            attached_by: String::new(),
        })
    }
}

/// What an attach did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachOutcome {
    /// No attachment existed; this space now has one.
    Attached,
    /// The identical attachment was already stored. Nothing changed and
    /// nothing was recorded — a retry is not a custody event.
    Unchanged,
    /// A different attachment was replaced. **A custody conversion**, and
    /// the one case that must reach the security stream and the consent
    /// page.
    Replaced,
}

pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(BUCKET_DDL).await.is_err() {
        pool.rows(BUCKET_DDL).await?;
    }
    Ok(())
}

pub async fn lookup(pool: &Pool, space: &SpaceId) -> Result<Option<Attachment>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"endpoint\", \"region\", \"bucket\", \"credential\", \"custody\", \
                    \"attached_at\", \"attached_by\", \"path_style\", \"key_prefix\" \
             FROM \"{BUCKET_TABLE}\" WHERE \"$space\" = {}",
            sql_lit(&space.0)
        ))
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let cell = |i: usize| row.get(i).and_then(|c| c.as_deref()).unwrap_or_default();
    // The two nullable columns are read through the raw cell, never
    // through `cell`'s `unwrap_or_default`, because `NULL` and `""` mean
    // different things here — `NULL` is "derive as today" (`path_style`)
    // or "no prefix" without ever having been asked (`key_prefix`), and
    // collapsing that into a default before the `Option` exists would be
    // exactly the sentinel-pretending-to-a-value the DDL's own doc argues
    // against.
    let raw = |i: usize| row.get(i).and_then(|c| c.as_deref());
    let path_style = match raw(7) {
        Some("t") | Some("true") => Some(true),
        Some("f") | Some("false") => Some(false),
        _ => None,
    };
    // Empty and NULL both mean "no prefix" — the same fold the column's
    // own doc promises, so a row from before this column existed and a
    // row that explicitly cleared it read back identically.
    let key_prefix = raw(8).filter(|s| !s.is_empty()).map(|s| s.to_string());
    Ok(Some(Attachment {
        endpoint: cell(0).to_string(),
        region: cell(1).to_string(),
        bucket: cell(2).to_string(),
        credential: cell(3).to_string(),
        custody: BucketCustody::parse(cell(4)).unwrap_or(BucketCustody::PlatformSealed),
        attached_at: cell(5).parse().unwrap_or(0),
        attached_by: cell(6).to_string(),
        path_style,
        key_prefix,
    }))
}

/// Every attachment this deployment holds, one row per space.
///
/// [`crate::spacedb::all`]'s twin, and it exists for the same one reason:
/// [`occupants`] needs to ask a question about a *target* rather than about
/// a space, and `$spacebucket` is keyed by space. Ordered by space so two
/// readers of the same table see the same order.
pub async fn all(pool: &Pool) -> Result<Vec<(SpaceId, Attachment)>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"$space\" FROM \"{BUCKET_TABLE}\" ORDER BY \"$space\""
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let space = SpaceId(row.first().and_then(|c| c.clone()).unwrap_or_default());
        // Read back through `lookup` rather than by widening the SELECT
        // above, so there is exactly one place that knows the column order
        // and the two nullable columns' NULL-versus-empty fold. A second
        // parser would be a second chance to read `path_style` as `false`
        // when the row says "derive".
        if let Some(at) = lookup(pool, &space).await? {
            out.push((space, at));
        }
    }
    Ok(out)
}

/// The host part of an attachment's endpoint, folded for comparison.
///
/// **The host and not the URL**, because `https://s3.example.com` and
/// `https://s3.example.com:443` are one endpoint and a string comparison
/// would call them two. Case is folded because a hostname is
/// case-insensitive by definition, which is
/// [`crate::spacedb`]'s `fold` argument unchanged; there is no socket-path
/// case here, because an S3 endpoint is always a URL.
///
/// An endpoint this cannot parse folds to its trimmed lowercase self rather
/// than to an error: the caller's own endpoint went through
/// `ordo_blob::S3Config::new` before it got here, and a *stored* row that
/// will not parse must still be able to occupy a bucket. Collapsing it to
/// `None` would make an unparseable row invisible to [`occupants`], which
/// is the one direction that failure may not go.
pub fn endpoint_host(endpoint: &str) -> String {
    match ordo_blob::http::Endpoint::parse(endpoint) {
        Ok(e) => e.host.to_ascii_lowercase(),
        Err(_) => endpoint.trim().to_ascii_lowercase(),
    }
}

/// Who else is already in this bucket (18p B-X5, ruled by D-D).
///
/// **The bucket is the unit, and the prefix is not part of the key.** D-D,
/// 2026-09-25: *"1 tenant 1 database. same for buckets. database/schema not
/// cluster."* The storage analogue of "database/schema not cluster" is that
/// the **bucket** is what may not be shared and the endpoint — an account,
/// a gateway, a region — may: many tenant buckets behind one endpoint is
/// the ordinary case and is what keeps a tenant costing a bucket rather
/// than an account.
///
/// `key_prefix` is deliberately **not** in the key, and the reason changed
/// on 2026-09-25 without the code changing. It was excluded because a
/// second tenant in one bucket was refused whatever prefix they stated.
/// The owner's clarification of D-D — *"1 bucket can have folders so 1
/// bucket defined per - but it could be the same and bucketname/tenant
/// would be the option"* — makes a bucket the analogue of a **cluster**
/// and a folder the analogue of a **database**, so sharing is allowed and
/// the caller decides what the occupancy means.
///
/// It stays out of the key because this function answers *who else is in
/// this bucket*, which is what [`crate::bucket::BucketCustody::OwnPerimeter`]
/// is about: a perimeter is the bucket, not a folder inside it. A caller
/// that wants "who else is under this exact prefix" would be asking about
/// collision, and `ordo_blob::object_key` already makes that impossible by
/// keying every object under its own space.
pub async fn occupants(
    pool: &Pool,
    endpoint: &str,
    bucket: &str,
) -> Result<Vec<(SpaceId, Attachment)>, PoolError> {
    let host = endpoint_host(endpoint);
    Ok(all(pool)
        .await?
        .into_iter()
        .filter(|(_, a)| endpoint_host(&a.endpoint) == host && a.bucket == bucket)
        .collect())
}

/// How many objects this space holds, in whichever stores they were written
/// to.
///
/// `$blobref` is the authoritative reference set (the sweep reads it and
/// `BlobStore` has no `list`), so this is one statement. Until 2026-10-06 it
/// gated a bucket change (D-S2). Since each row records its store (25ac) and
/// the owner lifted that refusal (`st-attach-existing` ask 2), it only
/// counts: the attach and detach answers say how many files stay in the
/// bucket they were written to.
///
/// **The pool is the space's own**, not the control pool: `$blobref` lives
/// in the database the space's rows live in, so asking the control database
/// about a tenant attached elsewhere would answer zero for a space with a
/// full bucket. A table that does not exist answers zero, which is a
/// deployment whose program has no non-inline blob field and therefore no
/// object anywhere.
pub async fn objects_held(pool: &Pool, space: &SpaceId) -> Result<i64, PoolError> {
    let table = ordo_crypto::blobref::BLOB_REF_TABLE;
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM \"{table}\" WHERE \"$space\" = {}",
            sql_lit(&space.0)
        ))
        .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(_) if !table_exists(pool, table).await? => return Ok(0),
        Err(e) => return Err(e),
    };
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// Asked with `to_regclass` rather than by matching an error string, for
/// `provision::directory_exists`' reason: a table that does not exist is a
/// deployment with no blob tier, and a table that exists and will not read
/// is a failure and stays one.
async fn table_exists(pool: &Pool, table: &str) -> Result<bool, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT to_regclass({}) IS NOT NULL",
            sql_lit(&format!("\"{table}\""))
        ))
        .await?;
    Ok(matches!(
        rows.first().and_then(|r| r[0].as_deref()),
        Some("t") | Some("true")
    ))
}

/// Remove a space's attachment, so the space resolves to the deployment's
/// own store again. Returns the attachment that stood, or `None` when there
/// was none.
///
/// For `std.space.detachBucket` (owner ruling 2026-10-05, `bucket-detach`).
/// The op refuses a space that holds objects before it calls this, the
/// same refusal as a change: the objects would stay in a bucket nothing
/// resolves to. Idempotent, so a retry after a lost answer is not an error.
pub async fn detach(pool: &Pool, space: &SpaceId) -> Result<Option<Attachment>, PoolError> {
    let standing = lookup(pool, space).await?;
    if standing.is_some() {
        pool.rows(&format!(
            "DELETE FROM \"{BUCKET_TABLE}\" WHERE \"$space\" = {}",
            sql_lit(&space.0)
        ))
        .await?;
    }
    Ok(standing)
}

/// Attach or replace, keyed on the target's identity.
pub async fn attach(
    pool: &Pool,
    space: &SpaceId,
    want: &Attachment,
) -> Result<AttachOutcome, PoolError> {
    let existing = lookup(pool, space).await?;
    let outcome = match &existing {
        Some(a) if a.identity() == want.identity() => return Ok(AttachOutcome::Unchanged),
        Some(_) => AttachOutcome::Replaced,
        None => AttachOutcome::Attached,
    };
    // Delete-then-insert rather than upsert, so the statement reads the way
    // the act does: one bucket per space, replaced whole. An upsert with a
    // partial column list is how an attachment ends up half old.
    let tx = pool.begin().await?;
    tx.rows(&format!(
        "DELETE FROM \"{BUCKET_TABLE}\" WHERE \"$space\" = {}",
        sql_lit(&space.0)
    ))
    .await?;
    tx.rows(&format!(
        "INSERT INTO \"{BUCKET_TABLE}\" \
         (\"$space\", \"endpoint\", \"region\", \"bucket\", \"credential\", \"custody\", \
          \"attached_at\", \"attached_by\", \"path_style\", \"key_prefix\") \
         VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
        sql_lit(&space.0),
        sql_lit(&want.endpoint),
        sql_lit(&want.region),
        sql_lit(&want.bucket),
        sql_lit(&want.credential),
        sql_lit(want.custody.as_str()),
        want.attached_at,
        sql_lit(&want.attached_by),
        opt_bool_lit(want.path_style),
        opt_text_lit(want.key_prefix.as_deref()),
    ))
    .await?;
    tx.commit().await?;
    Ok(outcome)
}

/// `NULL` for "not stated", a boolean literal otherwise. Kept beside
/// `sql_lit` rather than folded into it, because `sql_lit` is used
/// everywhere a value is never absent and giving it an `Option` overload
/// would let a caller write `NULL` into a `NOT NULL` column by forgetting
/// which one they meant.
fn opt_bool_lit(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "true",
        Some(false) => "false",
        None => "NULL",
    }
}

/// `NULL` for "not stated". `Some("")` cannot reach here — `attach`'s
/// caller stores an empty prefix as `None`, the same fold `lookup` reads
/// back — so this only ever has to choose between `NULL` and a quoted
/// value.
fn opt_text_lit(s: Option<&str>) -> String {
    match s {
        Some(v) => sql_lit(v),
        None => "NULL".to_string(),
    }
}

// ── the resolver ─────────────────────────────────────────────────────────

/// Resolution through `$spacebucket`: each attached space in the bucket
/// its row names, every other space in the deployment's own store.
///
/// `plan-tenant-storage.md` §11 U5. It lives here rather than in
/// `ordo-blob` for the reason that section states: `ordo-blob` is the
/// crate that exists to keep an HTTP client and a TLS stack in one place
/// and must not gain a Postgres dependency, while `ordo-exec` already
/// depends on `ordo-blob` and already holds the table this reads. It is
/// [`crate::route::AttachedPools`]'s sibling and sits beside
/// `$spacebucket` exactly as that one sits beside `$spacedb`.
///
/// # What this does that `AttachedPools` does not have to
///
/// **It honours `path_style` and `key_prefix`.** Both are columns U1 added
/// (§2 B-C2) and both change where in a bucket an object is addressed, so
/// a resolver that read the row and ignored them would produce a store
/// that talks to the right bucket about the wrong objects. `path_style`
/// **overrides** `S3Config::new`'s derivation rather than adding to it:
/// the derivation is right for an endpoint the operator controls and
/// wrong for a tenant's gateway, and `None` still means "derive as today"
/// so a row written before the column existed keeps the meaning it had.
/// `key_prefix` is re-checked against [`ordo_blob::space_prefix`]'s
/// charset here even though `BucketOps::attach` already refused a hostile
/// one at the door, because a row can predate that check, can be written
/// by a future caller, and can be edited by an operator with `psql`, and
/// the prefix is the one thing standing between this platform's objects
/// and whatever else is in the tenant's bucket.
///
/// # What it does not have to do that `AttachedPools` does
///
/// **There is no dial and so there is no backoff.** Building an
/// [`S3Store`] opens no socket: it is a configuration and a signer, and
/// the first network call is the first `put` or `get`. `AttachedPools`
/// needs [`crate::route::AttachedPools::backoff`] because `Pool::connect`
/// eagerly dials and a downed cluster would otherwise cost one connection
/// attempt per request. Nothing here can fail that way, so nothing here
/// pretends to guard against it. A bucket that will not answer fails at
/// the operation, and turning that into a refusal of the blob routes
/// while the rest of the tenant keeps serving is U7's subject, not this
/// increment's.
///
/// **Custody is not read.** [`BucketCustody`] decides what a consent page
/// says and what deleting the space means for objects already written; it
/// does not decide where the bytes go or how they are addressed. Sealing
/// happens in `BlobPipeline` before a store is ever asked, so a resolver
/// that branched on custody would be a second place the custody choice was
/// interpreted, and two interpretations of a disclosure is how a
/// disclosure stops being true.
///
/// # What is cached, and for how long
///
/// **One TTL over hits and misses both**, and this is the one place the
/// database sibling's reasoning does not carry over. `AttachedPools`
/// caches an attachment for the life of the process, which it earns from
/// `spacedb::attach` refusing to point an already-attached space somewhere
/// else: the answer a node cached is the answer that stands. [`attach`]
/// here has no such refusal, because [`AttachOutcome::Replaced`] is a
/// real outcome and a documented custody conversion, so a hit cached forever
/// would let a node go on writing into a bucket the owner has detached,
/// with nothing in the request path that could ever notice. Bounding both
/// answers with one interval is one rule rather than two, and it costs one
/// control-plane read per space per interval.
///
/// The miss half is the same argument `AttachedPools` makes for its own:
/// almost every space has no row, and asking the control database on every
/// request would put a round trip in front of every upload and every
/// download in the deployment. [`AttachedStores::forget`] lets the node
/// that just attached drop its own answer at once rather than waiting the
/// interval out.
///
/// **Stores are cached by [`Attachment::identity`]**, not by space, so a
/// hundred spaces attached to one bucket share one store and one signer.
/// Identity is the right key because it is exactly the set of fields that
/// change what the store talks to: change any of them and this is a
/// different target and gets a different store; change only `attached_at`
/// or `attached_by` and it is the same target restated, which is what
/// makes a retried attach cost nothing here.
pub struct AttachedStores {
    /// Where `$spacebucket` is read.
    control: Pool,
    /// The store a space with no row resolves to: the deployment's own,
    /// the single one `ORDO_BLOB_STORE` named. This is what makes turning
    /// the switch on a no-op until something is actually attached.
    default: Arc<dyn BlobStore>,
    credentials: Arc<dyn BucketCredentials>,
    ttl: Duration,
    routes: Mutex<HashMap<String, Route>>,
    stores: Mutex<HashMap<String, Arc<dyn BlobStore>>>,
    /// How many stores this node has built, in total, successful reuse not
    /// counted. What makes the identity cache observable: a second space
    /// on the same target must not add to this, and a count of *live*
    /// stores cannot tell "reused" from "rebuilt and replaced".
    built: std::sync::atomic::AtomicUsize,
}

/// What this node last read about one space. See the TTL argument on
/// [`AttachedStores`]: both arms expire, which is the difference from
/// `route::Route`.
#[derive(Clone)]
enum Route {
    Attached { at: Attachment, until: Instant },
    Default { until: Instant },
}

impl AttachedStores {
    /// How long an answer about one space is remembered.
    ///
    /// `AttachedPools::MISS_TTL`'s value, deliberately: an operator
    /// reasoning about how stale a routing answer can be on this node
    /// should have one number to hold, not two that happen to differ.
    pub const TTL: Duration = Duration::from_secs(30);

    /// `control` is where `$spacebucket` is read. `default` serves every
    /// space with no row, and is the store `ORDO_BLOB_STORE` configured.
    ///
    /// Does no I/O: the table is ensured by the caller that decided to run
    /// attached routing, so a deployment on `single` issues no statement
    /// it did not issue before.
    pub fn new(
        control: Pool,
        default: Arc<dyn BlobStore>,
        credentials: Arc<dyn BucketCredentials>,
    ) -> Self {
        AttachedStores {
            control,
            default,
            credentials,
            ttl: AttachedStores::TTL,
            routes: Mutex::new(HashMap::new()),
            stores: Mutex::new(HashMap::new()),
            built: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Drop what this node remembers about `space`, so the next resolve
    /// reads `$spacebucket` again. For the node that has just attached or
    /// replaced.
    pub fn forget(&self, space: &SpaceId) {
        self.routes
            .lock()
            .expect("the route cache is not poisoned")
            .remove(&space.0);
    }

    /// How many stores this node has built. See the field.
    pub fn stores_built(&self) -> usize {
        self.built.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// What `$spacebucket` says about this space, through the cache.
    async fn route(&self, space: &SpaceId) -> Result<Option<Attachment>, PoolError> {
        let cached = self
            .routes
            .lock()
            .expect("the route cache is not poisoned")
            .get(&space.0)
            .cloned();
        let now = Instant::now();
        match cached {
            Some(Route::Attached { at, until }) if now < until => return Ok(Some(at)),
            Some(Route::Default { until }) if now < until => return Ok(None),
            _ => {}
        }
        let found = lookup(&self.control, space).await?;
        let until = now + self.ttl;
        let route = match &found {
            Some(at) => Route::Attached {
                at: at.clone(),
                until,
            },
            None => Route::Default { until },
        };
        self.routes
            .lock()
            .expect("the route cache is not poisoned")
            .insert(space.0.clone(), route);
        Ok(found)
    }

    /// The store one attachment addresses, built once per target.
    fn store_for(&self, at: &Attachment) -> Result<Arc<dyn BlobStore>, BlobError> {
        let id = at.identity();
        if let Some(store) = self
            .stores
            .lock()
            .expect("the store cache is not poisoned")
            .get(&id)
        {
            return Ok(Arc::clone(store));
        }
        let store = self.build(at)?;
        self.built
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.stores
            .lock()
            .expect("the store cache is not poisoned")
            .insert(id, Arc::clone(&store));
        Ok(store)
    }

    /// One attachment's row plus one installed credential, into a store.
    ///
    /// The whole of "the credential does not ride the op" (this module's
    /// own doc) lands in the two lines that ask
    /// [`BucketCredentials::key_pair`]: the row named a label, the node
    /// holds the material, and the two meet here and nowhere earlier. A
    /// label this node does not hold is a refusal **before** a store
    /// exists, which is the same posture `route::target_config` takes on
    /// the database side: never a dial as nobody.
    fn build(&self, at: &Attachment) -> Result<Arc<dyn BlobStore>, BlobError> {
        Ok(S3Store::shared(store_config(
            at,
            self.credentials.as_ref(),
        )?))
    }
}

/// One attachment's row plus one installed credential, into the
/// configuration a store is built from.
///
/// **Lifted out of [`AttachedStores::build`] by 18p U6 so the attach-time
/// probe and the resolver cannot disagree.** The probe's whole claim is
/// *this credential signs for this target*, and a probe that assembled its
/// own configuration would be proving it about a target the resolver may
/// address differently — a different `path_style`, a different prefix, and
/// a probe that passes while every upload fails. One function, two callers,
/// so the claim is about the configuration that will actually be used.
pub fn store_config(
    at: &Attachment,
    credentials: &dyn BucketCredentials,
) -> Result<S3Config, BlobError> {
    let (access_key_id, secret_access_key) = credentials
        .key_pair(&at.credential)
        .map_err(BlobError::Config)?;
    let mut cfg = S3Config::new(
        &at.endpoint,
        &at.region,
        &at.bucket,
        access_key_id,
        secret_access_key,
    )?;
    // **The row's answer wins over the derivation**, and only when the
    // row gave one. See the column's own doc: `None` is "derive as
    // `S3Config::new` does", so a row written before U1 keeps the
    // behaviour it had, and `Some` is the tenant stating what their
    // own gateway requires, which is a fact the derivation cannot see.
    if let Some(path_style) = at.path_style {
        cfg.path_style = path_style;
    }
    // Re-checked here rather than trusted from the door. See the type
    // doc: a row can predate `BucketOps::attach`'s check or be written
    // past it, and a prefix is what separates this platform's objects
    // from the rest of the tenant's bucket. Refused, never sanitised,
    // for `space_prefix`'s own stated reason.
    if let Some(prefix) = &at.key_prefix {
        ordo_blob::space_prefix(&SpaceId(prefix.clone())).map_err(|e| {
            BlobError::Config(format!(
                "the attachment's key prefix is not addressable: {e}"
            ))
        })?;
    }
    cfg.key_prefix = at.key_prefix.clone();
    Ok(cfg)
}

#[async_trait::async_trait]
impl StoreResolver for AttachedStores {
    /// The space's own bucket when it has a row, the deployment's store
    /// when it has none.
    ///
    /// A failed read of `$spacebucket` is [`BlobError::Transport`] and not
    /// [`BlobError::Config`]: `Config`'s own doc says it is raised at
    /// construction and never mid-request, and this is mid-request. What
    /// matters more is what it is **not**: it is not
    /// [`BlobError::NotFound`], so a control database that is unreachable
    /// can never be served to a caller as "that object does not exist".
    /// The two are different incidents and `BlobPipeline::store_for` says
    /// so; collapsing them would turn an outage into a tenant being told
    /// their file is gone.
    async fn resolve(&self, space: &SpaceId) -> Result<Arc<dyn BlobStore>, BlobError> {
        Ok(self.current(space).await?.0)
    }

    /// The store and its name, from one read of the route, so the name an
    /// upload records is the store its bytes reached. A node whose cached
    /// route still names a bucket that was detached a moment ago writes
    /// there and records that bucket, which is the ruled behaviour
    /// (2026-10-05, `detach-upload-race` ask 2): nothing is re-checked at
    /// upload, and the object stays readable and collectable where it
    /// landed.
    async fn current(&self, space: &SpaceId) -> Result<(Arc<dyn BlobStore>, String), BlobError> {
        let attached = self.route(space).await.map_err(|e| {
            BlobError::caused_by("reading this space's bucket attachment", e.class(), e)
        })?;
        match attached {
            Some(at) => Ok((self.store_for(&at)?, at.store_name())),
            None => Ok((Arc::clone(&self.default), String::new())),
        }
    }

    /// The store a recorded name addresses, **without** reading
    /// `$spacebucket`. The row that named the bucket may be gone (a
    /// detach) or may name another one (a replace), and the object is
    /// still where it was written. The empty name is the deployment's own
    /// store.
    ///
    /// A name that does not parse is [`BlobError::Config`]: it is not
    /// [`BlobError::NotFound`], for `resolve`'s reason, because a row this
    /// build cannot read is an incident and never "that file is gone".
    async fn named(&self, _space: &SpaceId, name: &str) -> Result<Arc<dyn BlobStore>, BlobError> {
        if name.is_empty() {
            return Ok(Arc::clone(&self.default));
        }
        let at = Attachment::from_store_name(name).ok_or_else(|| {
            BlobError::Config("an object's recorded store name does not parse".into())
        })?;
        self.store_for(&at)
    }
}

/// The resolver an offline tool reads objects through: one that follows the
/// store each row **records**.
///
/// `StoreResolver::named` is what the sweep, erasure, export and
/// `ordo verify --objects` ask. A `SingleStore` answers every name with the
/// deployment's own store, which is right for a deployment that has never
/// attached a bucket and wrong for one that has: an object written to a
/// tenant's bucket would be looked for in the deployment's own store and
/// reported absent. [`AttachedStores`] resolves a recorded name by parsing it
/// and building the store it addresses, with no read of `$spacebucket`.
///
/// `credentials` is the node's provider, as the serving runtime builds it
/// from `ORDO_TENANT_CREDENTIALS`. With `None`, a row that names a bucket
/// cannot be opened and the tool says so, rather than answering from the
/// deployment's own store.
pub fn recorded_stores(
    pool: Pool,
    default: Arc<dyn BlobStore>,
    credentials: Option<Arc<dyn BucketCredentials>>,
) -> Arc<dyn StoreResolver> {
    let credentials: Arc<dyn BucketCredentials> = match credentials {
        Some(c) => c,
        None => Arc::new(NoBucketCredentials),
    };
    Arc::new(AttachedStores::new(pool, default, credentials))
}

/// What [`recorded_stores`] signs with when the node holds no credential
/// provider: nothing, and a refusal that names the setting and no label.
struct NoBucketCredentials;

impl BucketCredentials for NoBucketCredentials {
    fn key_pair(&self, _label: &str) -> Result<(String, String), String> {
        Err(
            "a file is recorded in a tenant's bucket and no credential provider is \
             configured: set ORDO_TENANT_CREDENTIALS=dir and ORDO_TENANT_CREDENTIALS_DIR"
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table holds no secret, and the absence is asserted rather than
    /// reviewed — a credential column would put an access key in every
    /// backup, every replica and every `pg_dump`.
    #[test]
    fn the_attachment_table_holds_no_secret() {
        let lower = BUCKET_DDL.to_lowercase();
        for forbidden in ["secret", "access_key", "accesskey", "password", "token"] {
            assert!(
                !lower.contains(forbidden),
                "`{forbidden}` in the attachment table: the credential is a REFERENCE, \
                 and a secret here is a secret in every backup"
            );
        }
        assert!(BUCKET_DDL.contains("\"credential\" text"));
    }

    /// A retry is the same attachment; a different target is a change.
    /// This is what lets the op be safe without the durable replay lease.
    #[test]
    fn identity_ignores_when_and_who_and_nothing_else() {
        let base = Attachment {
            endpoint: "https://s3.eu-west-1.amazonaws.com".into(),
            region: "eu-west-1".into(),
            bucket: "acme-files".into(),
            credential: "acme-prod".into(),
            custody: BucketCustody::PlatformSealed,
            path_style: None,
            key_prefix: None,
            attached_at: 1,
            attached_by: "mem_a".into(),
        };
        let retried = Attachment {
            attached_at: 999,
            attached_by: "mem_b".into(),
            ..base.clone()
        };
        assert_eq!(base.identity(), retried.identity());

        for changed in [
            Attachment {
                bucket: "acme-other".into(),
                ..base.clone()
            },
            Attachment {
                credential: "acme-staging".into(),
                ..base.clone()
            },
            Attachment {
                custody: BucketCustody::OwnPerimeter,
                ..base.clone()
            },
            Attachment {
                region: "us-east-1".into(),
                ..base.clone()
            },
            // U1: stating either new field is a genuine change, not a
            // retry — restating only `$keyPrefix` must not read as the
            // same attachment repeated.
            Attachment {
                path_style: Some(true),
                ..base.clone()
            },
            Attachment {
                key_prefix: Some("incoming".into()),
                ..base.clone()
            },
        ] {
            assert_ne!(
                base.identity(),
                changed.identity(),
                "a different target must not read as a retry"
            );
        }
    }

    /// The two custody positions say different things about what deleting
    /// the space does, and both say it in the platform's own words.
    #[test]
    fn each_custody_choice_discloses_what_erasure_means() {
        assert!(BucketCustody::PlatformSealed
            .disclosure()
            .contains("permanently unreadable"));
        assert!(BucketCustody::OwnPerimeter
            .disclosure()
            .contains("does NOT make these objects unreadable"));
        assert_eq!(
            BucketCustody::parse("platform-sealed"),
            Some(BucketCustody::PlatformSealed)
        );
        assert_eq!(BucketCustody::parse("whatever"), None);
    }

    /// **One bucket has one spelling** (D-D, through
    /// [`occupants`]). The occupancy question is asked by comparing stored
    /// endpoints to a caller's, and a comparison on the URL string would
    /// call `https://s3.acme.com` and `https://s3.acme.com:443` two
    /// endpoints — which is a second tenant walking into an occupied bucket
    /// by adding a port.
    #[test]
    fn one_endpoint_has_one_spelling_however_it_is_written() {
        let host = "s3.eu-west-1.amazonaws.com";
        for spelling in [
            "https://s3.eu-west-1.amazonaws.com",
            "https://S3.EU-West-1.AmazonAWS.com",
            "https://s3.eu-west-1.amazonaws.com:443",
            "http://s3.eu-west-1.amazonaws.com:9000",
        ] {
            assert_eq!(endpoint_host(spelling), host, "{spelling}");
        }
        // And endpoints that genuinely differ still do, so the fold is not
        // collapsing everything onto one host.
        assert_ne!(endpoint_host("https://gateway.acme.com"), host);
        // An endpoint that will not parse folds to itself rather than to
        // nothing: a stored row that cannot be parsed must still occupy its
        // bucket, because invisible-to-`occupants` is the one direction this
        // failure may not go.
        assert_eq!(endpoint_host("  Not-A-URL "), "not-a-url");
    }

    /// A recorded store name reads back as the attachment that wrote it,
    /// with the same identity, so the store it resolves to is the one the
    /// live attachment had (ruled 2026-10-05, `detach-upload-race` ask 2).
    #[test]
    fn a_recorded_store_name_reads_back_as_the_bucket_it_names() {
        let at = Attachment {
            endpoint: "https://s3.example.com|odd".into(),
            region: "eu-west-1".into(),
            bucket: "tenant-files".into(),
            credential: "tenant-a".into(),
            custody: BucketCustody::OwnPerimeter,
            path_style: Some(true),
            key_prefix: Some("ordo".into()),
            attached_at: 7,
            attached_by: "mem_owner".into(),
        };
        let name = at.store_name();
        assert!(name.contains("\"bucket\":\"tenant-files\""), "{name}");
        let back = Attachment::from_store_name(&name).expect("parses");
        assert_eq!(back.identity(), at.identity());
        assert_eq!(back.bucket, "tenant-files");
        assert_eq!(Attachment::from_store_name(""), None);
        assert_eq!(Attachment::from_store_name("{\"bucket\":1}"), None);
    }
}
