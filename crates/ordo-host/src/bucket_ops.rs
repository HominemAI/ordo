//! `std.space.attachBucket` — storage custody, as a granted operation.
//!
//! `definition-language.md` §2 pin (1): *"attaching or changing a space's
//! BYO bucket (or BYO database) is itself a granted operation —
//! space-owner consent, step-up gated, command-logged; storage custody
//! never changes silently."*
//!
//! Built by the recipe `platform_ops`'s module doc states: a spec in
//! `specs()`, an arm in `PlatformOps::execute`, `owner_only`, and
//! `engine.require_step_up` from the registration loop — which the loop
//! already applies to every platform op, so this one inherits it rather
//! than opting in. `require_step_up`'s own doc lists **custody
//! conversion** among what belongs there, and attaching a bucket is
//! exactly that.
//!
//! # Owner-only, not manager
//!
//! `platform_ops::owner_only`'s standing argument: a manager administers
//! *who may do what*; ending a space and disclosing all of it are not
//! administration. Changing where a space's files live, and who can read
//! them, is the same kind of act — it decides what leaving the platform
//! means, which is the owner's decision and nobody else's.
//!
//! # The idempotency gap is closed here, not inherited
//!
//! `exit_ops` records that `PlatformExecutor` drops the durable
//! `ReplayLease`, *"safe here only because both exit ops are idempotent by
//! construction"*. Attaching a bucket is not: run twice with different
//! arguments it changes storage custody twice. So the op is **keyed on the
//! target's identity** (`ordo_exec::bucket::Attachment::identity`): a
//! repeat of the same attachment is a no-op that reports success and
//! writes no custody event, and a different one is a genuine change with
//! its own record. That makes a repeat safe for a stated reason rather
//! than by inheritance.
//!
//! # The credential is a reference
//!
//! No secret rides this op. See `ordo_exec::bucket`'s module doc for the
//! three reasons, of which the deciding one is that the command log is
//! plaintext by design and a secret in it makes "the log is readable" an
//! incident rather than a property.
//!
//! # The refusal order (18p U6, §10 B-X3), and what each one leaves
//!
//! It is [`crate::provision::prepare_target`]'s order, because it is the
//! same problem one tier over: everything decidable from the parameters
//! alone comes first, then the reads, then the probe, then the write.
//! Nothing below the first write can be reached by a call that was going
//! to be refused.
//!
//! 1. **The target's shape**, through the same `S3Config::new` the binding
//!    uses, and the `keyPrefix` charset. `param_type_mismatch`.
//! 2. **The endpoint host is on the operator's allowlist** (D-S6).
//!    Checked before the credential is looked up and before any dial,
//!    because an S3 dial is an HTTP request to a host chosen by data —
//!    the exact shape `ordo-egress` exists to refuse — and because
//!    `S3Config::new`'s own check has the hole §10 B-X3.4 names: it
//!    permits plaintext http to anything `is_local` accepts, and an
//!    `https://169.254.169.254` endpoint passes it outright.
//! 3. **A plaintext endpoint needs the dev flag** (D-S6's second half).
//! 4. **Not the deployment's own default bucket** (B-X3.1).
//! 5. **The credential label resolves to material this node holds**, one
//!    `not_found` covering "nobody installed it" and "not for this space".
//! 6. **A retry short-circuits here.** An attachment identical to the one
//!    standing is answered `unchanged` without the checks below, and
//!    records nothing. The four checks above still run, because they are
//!    pure functions of the parameters and a retry should not be a way to
//!    learn that an endpoint is permitted.
//! 7. **The files already held are counted, not refused.** Until
//!    2026-10-06 a space holding `$blobref` rows was refused here (D-S2),
//!    because nothing recorded which bucket a file lived in. Since 25ac
//!    each row records its store, and reads, the sweep, erasure, export
//!    and the re-seal pass follow it, so the owner lifted the refusal
//!    (`st-attach-existing` ask 2). Old files stay where they are; the
//!    count goes in the answer as `filesStayingPut`. Read in the space's
//!    own database, never the control one.
//! 8. **Nobody else's own perimeter** (D-S8). A bucket may be shared and
//!    a tenant's declared own perimeter may not, in either direction.
//!    Sharing is safe by construction: `ordo_blob::object_key` keys every
//!    object `<space>/<handle>`, so a bucket is the analogue of a cluster
//!    and a folder the analogue of a database.
//! 9. **The probe**: the credential signs a `head` at the target, before
//!    the row is written. A `head` and never a `list`, per D-S7, so a
//!    correctly scoped credential passes.
//! 10. **The row**, then occupancy re-read, then the record.

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_blob::{BlobError, BlobStore, S3Config, S3Store};
use ordo_crypto::SpaceId;
use ordo_exec::bucket::{self, AttachOutcome, Attachment, BucketCustody};
use ordo_exec::creds::BucketCredentials;
use ordo_exec::route::PoolResolver;
use ordo_exec::Pool;
use serde_json::json;

use crate::provision::HostAllowlist;

pub const ATTACH_BUCKET: &str = "std.space.attachBucket";

/// `std.space.detachBucket`: the tenant returns to the deployment's own
/// store (owner ruling 2026-10-05, `bucket-detach`: *"Yes - should be an
/// option"*). Owner only and step-up gated like the attach, and recorded
/// like the attach. Files already written stay in the detached bucket and
/// are still read and cleaned up there (ruled 2026-10-06,
/// `st-attach-existing` ask 2).
pub const DETACH_BUCKET: &str = "std.space.detachBucket";

/// What a change or a detach does to the files a tenant already holds, in
/// the answer to both. Ruled 2026-10-06 (`st-attach-existing` ask 2).
pub const EXISTING_OBJECTS: &str = "this change governs uploads from now on. Files already \
     written stay in the bucket they were written to and are still read and cleaned up there, \
     so that bucket and its credential must stay reachable until they are gone. Nothing is \
     copied; moving old files is a separate step.";

/// The deployment's **own** default store, as the identity an attachment
/// would have to equal to be it (§10 B-X3.1).
///
/// `None` on a deployment whose default store is not S3 — an `fs:` test
/// double has no endpoint and no bucket an attachment could name, and
/// §2 B-C4 records that an attachment can never be an `fs:` target anyway,
/// so there is nothing to compare.
///
/// **The bucket and the endpoint's host, not the whole URL and not the
/// region.** A tenant pointed at the platform's own bucket gets an
/// attachment row and a disclosure page telling them the bytes are in a
/// bucket they hold, and they are in the platform's. The region is not part
/// of it because two regions naming one bucket is one bucket; the endpoint
/// host is, because `s3.example.com/files` and `gateway.acme.com/files` are
/// two buckets that happen to share a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnStore {
    /// Folded through [`ordo_exec::bucket::endpoint_host`] on the way in,
    /// so the comparison is against one spelling.
    endpoint_host: String,
    bucket: String,
}

impl OwnStore {
    /// From a `https://host[:port]` endpoint, the shape an attachment
    /// states.
    pub fn at_endpoint(endpoint: &str, bucket: &str) -> OwnStore {
        OwnStore {
            endpoint_host: bucket::endpoint_host(endpoint),
            bucket: bucket.to_string(),
        }
    }

    /// From a bare host, the shape `ordo_blob::S3Config` holds after it has
    /// parsed the deployment's own `ORDO_BLOB_S3_ENDPOINT`. A separate
    /// constructor rather than letting [`OwnStore::at_endpoint`] fall
    /// through its own parse failure: relying on that fallback would make
    /// the deployment's own bucket unrecognisable the day the fallback
    /// changed.
    pub fn at_host(host: &str, bucket: &str) -> OwnStore {
        OwnStore {
            endpoint_host: host.trim().to_ascii_lowercase(),
            bucket: bucket.to_string(),
        }
    }

    fn is(&self, endpoint: &str, bucket: &str) -> bool {
        bucket::endpoint_host(endpoint) == self.endpoint_host && bucket == self.bucket
    }
}

/// **What proves a credential signs, before the row is written** (§5 B-F5,
/// D-S7).
///
/// A trait rather than a bare function for one reason: the probe is the only
/// part of this op that touches the network, so it is the only part a test
/// cannot exercise against a real target, and an op whose refusals can only
/// be tested by standing up S3 is an op whose refusals are not tested. The
/// production implementation is [`HeadProbe`] and it is what every
/// deployment gets; a test substitutes an answer and drives every other
/// check through the real door.
#[async_trait::async_trait]
pub trait BucketProbe: Send + Sync {
    /// `Ok` when the credential signed and the target answered.
    async fn probe(&self, cfg: &S3Config, space: &SpaceId) -> Result<(), BlobError>;
}

/// The one handle a probe asks about.
///
/// Fixed rather than random, and it does not have to be either: both a 200
/// and a 404 are a pass, so what the key addresses is irrelevant and a
/// per-call random handle would only make the request harder to recognise
/// in a gateway's access log. All-zero is a handle `mint_blob_ref` will
/// never produce, which keeps the probe from ever addressing a real object.
const PROBE_HANDLE: ordo_crypto::custody::BlobHandle = [0u8; 16];

/// `head` at the target, which is the probe D-S7 asks for.
///
/// **A `head` and never a `list`.** §10 B-X2 publishes the least-privilege
/// policy the platform's credential expects — `PutObject`, `GetObject` and
/// `DeleteObject` under the attachment's prefix, and no `ListBucket` — so a
/// probe that listed would refuse exactly the tenants who scoped their
/// credential the way the platform asked them to.
///
/// **`NotFound` is the passing answer**, and it is the one a correctly
/// scoped credential gives: S3 answers 404 for an absent key when the
/// caller holds `GetObject` on it and 403 when it does not, so the
/// difference between the two *is* the question being asked. A 200 passes
/// too — it means the same thing about the credential — and is unreachable
/// in practice because [`PROBE_HANDLE`] addresses no object.
pub struct HeadProbe;

#[async_trait::async_trait]
impl BucketProbe for HeadProbe {
    async fn probe(&self, cfg: &S3Config, space: &SpaceId) -> Result<(), BlobError> {
        let store: Arc<dyn BlobStore> = S3Store::shared(cfg.clone());
        match store.head(space, &PROBE_HANDLE).await {
            Ok(_) | Err(BlobError::NotFound) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// The handler, and the pieces it needs.
pub struct BucketOps {
    /// The control database, where `$spacebucket` lives.
    pub pool: Pool,
    /// **The deployment's one pool resolver**, for counting the files a
    /// space already holds. `$blobref` lives in the database a space's rows
    /// live in, so asking the control database about a tenant attached
    /// elsewhere would answer "no files" for a space with a full bucket.
    pub pools: Arc<dyn PoolResolver>,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
    /// The credential material the operator installed, behind the same
    /// provider `$spacedb` labels resolve through (§3 B-K1).
    ///
    /// **This replaced a `Vec<String>` of labels.** The list and the
    /// material were two answers to one question, and the list could hold a
    /// label that names a *Postgres password* — which passed the door's
    /// check and then failed when a store was built, at the first upload.
    /// Asking the provider is one lookup, it answers "not installed",
    /// "not a bucket label" and "not for you" identically, which is the
    /// non-enumerating refusal §10 B-X3 asks for, and what comes back is
    /// exactly what the probe needs.
    pub credentials: Arc<dyn BucketCredentials>,
    /// The hosts this deployment may attach, **one list shared with the
    /// database tier** (D-S6). See [`HostAllowlist`]: read from the
    /// environment once at the edge and passed down, never read inside the
    /// check.
    pub hosts: HostAllowlist,
    /// The deployment's own default bucket, when it has one.
    pub own_store: Option<OwnStore>,
    /// `ORDO_DEV_UNSAFE_BUCKET_ENDPOINT`: whether a plaintext endpoint may
    /// be attached. See [`BucketOps::refuse_endpoint`].
    pub allow_plaintext: bool,
    pub probe: Arc<dyn BucketProbe>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl BucketOps {
    fn param<'a>(&self, bound: &'a BoundParams, name: &str) -> Result<&'a str, ExecError> {
        bound
            .0
            .get(name)
            .and_then(|v| v.as_str())
            .ok_or_else(|| ExecError::internal(format!("`${name}` did not reach the platform op")))
    }

    /// The two address refusals, D-S6's pair, before anything is read.
    ///
    /// **One reason and no detail for the allowlist.**
    /// `ordo_egress::refusal`'s argument and
    /// `provision::ProvisionRefusal::HostNotAllowed`'s, unchanged: a caller
    /// who learns *why* a host was refused maps the allowlist one attempt
    /// at a time, and an attach is reachable by every space owner on the
    /// deployment.
    ///
    /// **The plaintext exception moves behind a dev flag.**
    /// `S3Config::new` permits `http://` to `localhost`, `127.0.0.1`,
    /// `::1`, `*.localhost` and the bare name `minio`, which the self-host
    /// story genuinely needs on a laptop and which in a pod is a request
    /// into the deployment's own network. So it is gated the way `fs:`
    /// already is — [`ordo_blob::BlobStore::production_ready`]'s posture
    /// with its own flag — and a **separate** flag from
    /// `ORDO_DEV_UNSAFE_BLOB_STORE` for that variable's own stated reason:
    /// they unsafe different things, and a self-host laptop whose tenant
    /// bucket is a local MinIO is not a deployment that agreed to put
    /// customer files on a filesystem with no durability story.
    ///
    /// An `https` endpoint on loopback is **not** gated here. It is TLS, so
    /// the credential is not on the wire, and what stops it reaching the
    /// pod's own network is the allowlist, which denies by default. Gating
    /// it too would be a second control over the same fact.
    fn refuse_endpoint(&self, endpoint: &str) -> Result<(), ExecError> {
        let parsed = ordo_blob::http::Endpoint::parse(endpoint).map_err(|e| {
            ExecError::new(
                ErrorCode::ParamTypeMismatch,
                format!("that endpoint cannot be addressed: {e}"),
            )
        })?;
        if !self.hosts.allows(&parsed.host) {
            return Err(ExecError::new(
                ErrorCode::BucketRefused,
                "that endpoint is not one this deployment may attach a bucket at. Nothing \
                 was attached",
            ));
        }
        if !parsed.tls && !self.allow_plaintext {
            return Err(ExecError::new(
                ErrorCode::BucketRefused,
                "a plaintext endpoint is a development convenience and this deployment is \
                 not in development mode: the object bytes are sealed but the SigV4 \
                 credential is not, and a captured signature is a valid request. Set \
                 ORDO_DEV_UNSAFE_BUCKET_ENDPOINT=1 if that is genuinely what you want. \
                 Nothing was attached",
            ));
        }
        Ok(())
    }

    /// `std.space.attachBucket`.
    pub async fn attach(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let endpoint = self.param(bound, "endpoint")?;
        let region = self.param(bound, "region")?;
        let bucket_name = self.param(bound, "bucket")?;
        let credential = self.param(bound, "credential")?;

        // **A bool, not a `custody: text` with two legal spellings**, and
        // the difference is which layer refuses a typo. `exit_ops`' spec
        // makes the same choice for the same reason and states it: text
        // would put an enum check inside this handler, and an executor
        // **may not say `invalid_param`** — that is the door's word (§4.1
        // non-enumerability), so `ApiError::from_exec` flattens it to
        // `internal`. A typo would have reached the caller as a 500.
        //
        // A bool has no invalid value: `bind_params` type-checks it at the
        // door, in the door's own vocabulary, before this runs. `required:
        // true` still forces the author to state a position, so nothing is
        // defaulted on the owner's behalf.
        //
        // Found by driving the op through the real door for the first time
        // (`tests/attach_bucket.rs`). The unit tests underneath could not
        // see it, because they never crossed the layer that enforces the
        // rule — which is the whole argument for that file existing.
        // Both new. **A bool, optional, no enum to typo** — `pathStyle`
        // states the one fact `S3Config::new`'s derivation cannot see: a
        // self-hosted gateway behind TLS on a real hostname needs path
        // style and the derivation would give it virtual-host style
        // instead. Absent means "derive as today", so an attachment that
        // never mentions it behaves exactly as one from before this
        // parameter existed. See `docs/plan-tenant-storage.md` §2 B-C2.
        let path_style = bound.0.get("pathStyle").and_then(|v| v.as_bool());

        // `keyPrefix` is refused rather than sanitised, on
        // `ordo_blob::space_prefix`'s own charset and for its own reason:
        // a sanitised hostile prefix can collapse two different inputs
        // onto one, and the prefix is what stands between this
        // attachment's objects and whatever else lives in the tenant's
        // bucket. Empty is "not stated" — today's layout — and is never
        // checked against the charset, the same way an absent `pathStyle`
        // is never checked against being a bool: nothing was asked for.
        let key_prefix = match bound.0.get("keyPrefix").and_then(|v| v.as_str()) {
            None | Some("") => None,
            Some(prefix) => {
                ordo_blob::space_prefix(&SpaceId(prefix.to_string())).map_err(|e| {
                    ExecError::new(
                        ErrorCode::ParamTypeMismatch,
                        format!(
                            "`$keyPrefix` is not usable as a bucket key prefix: {e}. Refused \
                             rather than sanitised, because a sanitised hostile prefix can \
                             collapse two different inputs onto one."
                        ),
                    )
                })?;
                Some(prefix.to_string())
            }
        };

        let custody = match bound.0.get("platformSealed").and_then(|v| v.as_bool()) {
            Some(true) => BucketCustody::PlatformSealed,
            Some(false) => BucketCustody::OwnPerimeter,
            None => {
                return Err(ExecError::internal(
                    "`$platformSealed` did not reach the platform op",
                ))
            }
        };

        // The endpoint and bucket are validated by the same constructor the
        // S3 binding uses, so an attachment that would not boot is refused
        // here rather than at the first upload. The credential is a
        // placeholder: what is being checked is the *shape* of the target,
        // and the real secret is resolved when a store is built.
        //
        // `param_type_mismatch`, which **is** executor-raisable and is
        // what this is: the door typed `$endpoint` as a `uri` and checked
        // its shape, but "https, or http only on loopback" and the S3
        // bucket-name charset are facts about the *target* that no
        // `ParamTy` expresses.
        ordo_blob::S3Config::new(endpoint, region, bucket_name, "probe", "probe").map_err(|e| {
            ExecError::new(
                ErrorCode::ParamTypeMismatch,
                format!(
                    "that bucket target cannot be reached as configured: {e}. Checked here \
                     rather than at the first upload, because discovering it then means \
                     discovering it in production."
                ),
            )
        })?;

        // D-S6's pair, before the credential lookup and before any read.
        self.refuse_endpoint(endpoint)?;

        // **The deployment's own store is not a target** (§10 B-X3.1).
        // This is `prepare_target`'s `ControlDatabase` refusal one tier
        // over, and the cost of not having it is the same shape: a row and
        // a consent page telling a tenant their bytes are in a bucket they
        // hold, while the bytes sit in the platform's own bucket beside
        // every other tenant's.
        if self
            .own_store
            .as_ref()
            .is_some_and(|own| own.is(endpoint, bucket_name))
        {
            return Err(ExecError::new(
                ErrorCode::BucketRefused,
                "that is this deployment's own default bucket, so attaching it would record \
                 a custody change and a disclosure saying these objects are in a bucket you \
                 hold, when they would be where they already are. Nothing was attached",
            ));
        }

        let sid = SpaceId(space.to_string());
        let want = Attachment {
            endpoint: endpoint.to_string(),
            region: region.to_string(),
            bucket: bucket_name.to_string(),
            credential: credential.to_string(),
            custody,
            path_style,
            key_prefix,
            attached_at: (self.now)() as i64,
            attached_by: caller.to_string(),
        };

        // The credential label, resolved to material rather than checked
        // against a list. `not_found`, in `callers_of`'s shape: a
        // credential nobody installed, one that names a Postgres password,
        // and one this space may not use are **one answer**, so the
        // parameter enumerates nothing. An operator who installed a label
        // knows its name; a caller guessing learns only that this guess was
        // wrong.
        //
        // `not_found` rather than `invalid_param` because the executor may
        // not say the latter — see the custody comment above.
        //
        // The configuration comes back from `bucket::store_config`, which
        // is the function `AttachedStores` builds its store with, so the
        // probe below is about the target the resolver will actually
        // address rather than one assembled here.
        let cfg = bucket::store_config(&want, self.credentials.as_ref()).map_err(|_| {
            ExecError::new(
                ErrorCode::NotFound,
                "no credential is installed under that name — or none this space may use; \
                 which answer this is, is deliberately not disclosed. A bucket attachment \
                 names a credential the operator installed out of band, never an access \
                 key, which would ride the request envelope, the idempotency cache and the \
                 plaintext command log.",
            )
        })?;

        bucket::ensure_table(&self.pool)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;

        // **A retry stops here**, before the checks that are about changing
        // where a space's bytes go. See the module doc, step 6: a retry
        // changes nothing, so it records nothing and dials nothing.
        let standing = bucket::lookup(&self.pool, &sid)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        if standing
            .as_ref()
            .is_some_and(|a| a.identity() == want.identity())
        {
            // Occupancy is read rather than assumed to be 1. It is 1 on
            // every attachment this build wrote, and a row written before
            // D-D was ruled can have a neighbour; answering 1 there would
            // be the response contradicting the table.
            let occupancy = bucket::occupants(&self.pool, &want.endpoint, &want.bucket)
                .await
                .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?
                .len();
            return Ok(self.answer(space, &want, AttachOutcome::Unchanged, occupancy, None));
        }

        // **Files already written stay where they are** (ruled 2026-10-06,
        // `st-attach-existing` ask 2, which lifted the D-S2 refusal). Each
        // `$blobref` row records the store its bytes reached (25ac), and
        // reads, the sweep, erasure, export and the re-seal pass follow
        // that name rather than this table. So a change moves new uploads
        // only, and nothing is copied. The count is read so the answer can
        // say how many files stay behind; it gates nothing.
        //
        // The space's own pool, never the control one: `$blobref` lives in
        // the database the space's rows live in.
        let pool = self
            .pools
            .resolve(&sid)
            .await
            .map_err(|_| unavailable("this space's database could not be reached"))?;
        let held = bucket::objects_held(&pool, &sid)
            .await
            .map_err(|_| unavailable("this space's object references could not be read"))?;

        // **A bucket may be shared; a tenant's own perimeter may not**
        // (D-S8, and the 2026-09-25 clarification of D-D).
        //
        // **This was built the other way round and was wrong.** D-D reads
        // *"1 tenant 1 database. same for buckets. database/schema not
        // cluster."* The first build of this op took "same for buckets" to
        // mean a bucket holds one tenant, full stop, and refused a second
        // tenant whatever prefix they stated. The owner's clarification:
        // *"1 bucket can have folders so 1 bucket defined per - but it
        // could be the same and bucketname/tenant would be the option. The
        // deployment can also have its own bucket defined."*
        //
        // So the tiers line up one level lower than the first reading put
        // them. A **bucket** is the analogue of a **cluster**, shareable; a
        // **folder** is the analogue of a **database**, and is what one
        // tenant has to itself. `ordo_blob::object_key` already makes every
        // key `<space>/<handle>`, so two tenants in one bucket cannot
        // collide however their prefixes are set — the separation the
        // clarification describes is what the layout has always done.
        //
        // What survives is D-S8, which the owner ruled *"agreed"* and which
        // asks a different question: a bucket declared as one tenant's
        // **own perimeter** is that tenant's, and the platform is not the
        // party who may put somebody else inside it. That is about a
        // disclosure rather than about collision, which is why a folder
        // does not answer it.
        let sitting = bucket::occupants(&self.pool, &want.endpoint, &want.bucket)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        let clash = sitting.iter().find(|(o, a)| {
            o != &sid
                && (a.custody == BucketCustody::OwnPerimeter
                    || want.custody == BucketCustody::OwnPerimeter)
        });
        if let Some((other, a)) = clash {
            // The holder's space id **is** named, on `DatabaseOccupied`'s
            // precedent: it is not the allowlist, and a deployment
            // administrator resolving this needs to know whose bucket it
            // is. The space id is minted and opaque and carries no customer
            // name.
            let holder = other.0.clone();
            let why = if a.custody == BucketCustody::OwnPerimeter {
                "that bucket is already held as their own perimeter, and the platform is \
                 not the party who may put a second tenant inside it"
            } else {
                "this attachment declares the bucket its own perimeter, and a perimeter \
                 with somebody else already inside it is not what that discloses"
            };
            return Err(ExecError::new(
                ErrorCode::BucketRefused,
                format!(
                    "`{holder}` is already in that bucket and {why}. Sharing a bucket is \
                     otherwise fine — every object is keyed under its own space — so \
                     another bucket, or a different custody, attaches without this. \
                     Nothing was attached"
                ),
            ));
        }

        // **The probe, before the row is written** (§5 B-F5, D-S7). An
        // attachment whose credential cannot sign is a tenant whose next
        // upload fails, and the place to find that out is the call that
        // asked for it. After the refusals above, so a target that was
        // never going to be accepted costs no network call.
        self.probe.probe(&cfg, &sid).await.map_err(probe_refusal)?;

        let outcome = bucket::attach(&self.pool, &sid, &want)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;

        // Occupancy read **after** the attach, so the number recorded is
        // the one that was true once this space was in rather than the one
        // that was true just before. §10 B-X5 makes occupancy part of what
        // is recorded and exposed, and a record saying `occupancy=0` for a
        // bucket with a tenant in it would be the record disagreeing with
        // the fact.
        let occupancy = bucket::occupants(&self.pool, &want.endpoint, &want.bucket)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?
            .len();

        // Recorded on a **change** only. A retry is not a custody event,
        // and recording one would put a false "storage custody changed" in
        // the owner's own stream every time a client retried — which is
        // the record that is supposed to mean something.
        if outcome != AttachOutcome::Unchanged {
            self.record(space, caller, &want, outcome, occupancy);
        }

        Ok(self.answer(space, &want, outcome, occupancy, Some(held)))
    }

    /// `std.space.detachBucket`: the tenant returns to platform custody.
    ///
    /// # The order
    ///
    /// 1. **Nothing attached is an answer, not a refusal.** `unchanged`,
    ///    so a retry after a lost answer lands, and no custody event.
    /// 2. **The files already held are counted, not refused** (ruled
    ///    2026-10-06, `st-attach-existing` ask 2). Each `$blobref` row names
    ///    the bucket its bytes are in, so once the attachment row is gone
    ///    the files are still read, swept and erased there. That bucket and
    ///    its credential label must stay reachable until they are gone.
    /// 3. **The row is removed**, then the record, on the deployment's stream, as the
    ///    attach records.
    ///
    /// No probe and no allowlist: nothing is dialled, and the deployment's
    /// own store is where every tenant with no row already lives.
    ///
    /// # Other nodes
    ///
    /// `AttachedStores` remembers an answer for [`bucket::AttachedStores::TTL`],
    /// on every node, as it does after a replace. Until that interval ends,
    /// a node may still send a new upload to the detached bucket. The
    /// window is the attach's, stated rather than hidden.
    ///
    /// Such an upload is not stranded. Its `$blobref` row records the
    /// bucket it was written to, and reads and the sweep follow that name
    /// rather than this table (ruled 2026-10-05, `detach-upload-race` ask
    /// 2). Nothing is re-checked at upload.
    pub async fn detach(&self, space: &str, caller: &str) -> Result<ExecResult, ExecError> {
        let sid = SpaceId(space.to_string());
        bucket::ensure_table(&self.pool)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        let Some(standing) = bucket::lookup(&self.pool, &sid)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?
        else {
            return Ok(detach_answer(space, None, "unchanged", None));
        };

        let pool = self
            .pools
            .resolve(&sid)
            .await
            .map_err(|_| unavailable("this space's database could not be reached"))?;
        // Read to say how many files stay in the detached bucket, never to
        // refuse (ruled 2026-10-06, `st-attach-existing` ask 2).
        let held = bucket::objects_held(&pool, &sid)
            .await
            .map_err(|_| unavailable("this space's object references could not be read"))?;

        let removed = bucket::detach(&self.pool, &sid)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        match removed {
            Some(was) => {
                self.record_change(space, caller, DETACH_BUCKET, "bucket_detached", &was, 0);
                Ok(detach_answer(space, Some(&was), "detached", Some(held)))
            }
            // Another call detached it between the read and the delete.
            None => Ok(detach_answer(space, Some(&standing), "unchanged", None)),
        }
    }

    /// The response rows, assembled in one place so the retry
    /// short-circuit and the ordinary path cannot answer differently.
    fn answer(
        &self,
        space: &str,
        want: &Attachment,
        outcome: AttachOutcome,
        occupancy: usize,
        held: Option<i64>,
    ) -> ExecResult {
        ExecResult {
            position: None,
            rows: vec![json!({
                "space": space,
                "bucket": want.bucket,
                "endpoint": want.endpoint,
                "region": want.region,
                // **Never the credential.** `want.credential` is a label,
                // not a secret, but it is deployment-side configuration
                // naming material installed on the node, and a space
                // owner has no use for it — returnable to a deployment
                // administrator, never to a tenant. See
                // `docs/plan-tenant-storage.md` §10 B-X4.2.
                "custody": want.custody.as_str(),
                "pathStyle": want.path_style,
                "keyPrefix": want.key_prefix,
                "outcome": verb(outcome),
                // The sentence a consent page prints, from the platform
                // rather than from a renderer — a disclosure assembled by
                // the UI is a disclosure that differs per UI.
                "disclosure": want.custody.disclosure(),
                // **§10 B-X5: the fact, and the developer writes the
                // sentence.** Ruled 2026-09-21 on D-S8, and the number is
                // now 1 by construction because D-D refuses a second
                // tenant. It is still returned rather than dropped: it is
                // what an application would have to state if the rule ever
                // widened, and a caller reading `occupancy` learns the
                // platform is keeping the count rather than assuming it.
                "occupancy": occupancy,
                // Files already written stay in the store they were written
                // to and are read and cleaned up there (ruled 2026-10-06,
                // `st-attach-existing` ask 2). Saying so, with the count, is
                // the difference between a tenant who knows the old bucket
                // must stay reachable and one who deletes it.
                "filesStayingPut": held,
                "existingObjects": EXISTING_OBJECTS,
            })],
            next_cursor: None,
        }
    }

    /// `EventKind::CustodyAction`, beside `space_exported` and
    /// `space_erased` — and beside epoch rotation and custody conversion,
    /// which are what this is actually a sibling of.
    ///
    /// The same reinterpretation of "command-logged" that `plan-export.md`
    /// §5.1 argued, for the same reason: this writes no entity row, so
    /// there is no command group to append. The requirement §2 is making —
    /// *storage custody never changes silently* — is met by the half of
    /// the spine that records decisions about a space rather than changes
    /// to its rows.
    ///
    /// # One stream, the deployment's
    ///
    /// Ruled 2026-09-29: a bucket attach is written to the deployment's
    /// record and not to the tenant's own security stream. Nothing is
    /// recorded in the tenant's chain either (D-F, 2026-09-21), so there is
    /// no founding-group entry and no command group here.
    ///
    /// # The detail rides the action string
    ///
    /// `CustodyAction` carries one string and is hash-chained under a tag
    /// (`ordo_secevent::chain`), so widening the variant would change the
    /// encoding of every event already written. Everything §10 B-X6 asks
    /// the record to name therefore goes in that string, in a form that
    /// splits: the target's identity is `Attachment::identity`'s seven
    /// fields unchanged — in which `credential` is a **label** and never a
    /// secret, which is the property `ordo_exec::bucket`'s module doc
    /// states and this relies on — then the occupancy and whether this call
    /// attached or replaced. The space's minted id is not repeated because
    /// the observation already carries it.
    fn record(
        &self,
        space: &str,
        caller: &str,
        want: &Attachment,
        outcome: AttachOutcome,
        occupancy: usize,
    ) {
        let action = format!("bucket_{}", verb(outcome));
        self.record_change(space, caller, ATTACH_BUCKET, &action, want, occupancy);
    }

    /// One custody change, on the deployment's stream and nowhere else.
    /// The attach, the replace and the detach all write through here.
    ///
    /// Ruled 2026-09-29 (`db-chain-attach`, checklist 22k): the tenant's
    /// own security stream carries nothing about where its files were put.
    /// That is the application's to disclose. `Relationship::None` routes
    /// to `Scope::Deployment` and carries the space as the event's *object*
    /// rather than its home (see `Observation::space`).
    ///
    /// # The subject is the member who asked
    ///
    /// The deployment's record is now the only record of the change, so it
    /// names the caller, as the deployment-scope records of an erasure
    /// (`exit_ops`) and of an identity removal do. While the tenant's own
    /// stream carried a copy, the deployment's half named the platform and
    /// left the member to the tenant's copy; 73fbc0dc dropped that copy and
    /// with it the only mention of who made the call (audit 2026-10-06).
    fn record_change(
        &self,
        space: &str,
        caller: &str,
        op: &str,
        verb: &str,
        want: &Attachment,
        occupancy: usize,
    ) {
        let Some(stream) = &self.stream else {
            return;
        };
        let action = format!("{verb} {} occupancy={occupancy}", want.identity());
        stream.observe(
            ordo_secevent::Observation {
                space: Some(ordo_commandlog::SpaceId(space.to_string())),
                subject: ordo_secevent::Subject::member(caller),
                op: op.to_string(),
                kind: ordo_secevent::EventKind::CustodyAction { action },
                at: ordo_commandlog::Timestamp((self.now)()),
                correlation: ordo_secevent::correlation::adopt(&format!("bucket_{space}")),
            },
            ordo_secevent::Relationship::None,
        );
    }
}

/// What `std.space.detachBucket` answers. `custody` is `platform`: the
/// tenant's objects from here on go to the deployment's own store.
/// `filesStayingPut` is how many files the tenant already holds, which stay
/// readable in the bucket they were written to. `None` (JSON `null`) when
/// nothing changed, so nothing was counted.
fn detach_answer(
    space: &str,
    was: Option<&Attachment>,
    outcome: &str,
    held: Option<i64>,
) -> ExecResult {
    ExecResult {
        position: None,
        rows: vec![json!({
            "space": space,
            "bucket": was.map(|a| a.bucket.as_str()),
            "endpoint": was.map(|a| a.endpoint.as_str()),
            "custody": "platform",
            "outcome": outcome,
            "disclosure": "this space's objects are stored in the deployment's own store, \
                           sealed under this space's keys, from now on.",
            "filesStayingPut": held,
            "existingObjects": EXISTING_OBJECTS,
        })],
        next_cursor: None,
    }
}

/// A store this deployment could not reach, in the door's vocabulary.
///
/// **503 and never the store's own text** (§10 B-X4.4). `BlobError::
/// Transport` and `Refused` carry the provider's response body, which can
/// name the credential's principal, the bucket and the account — for a
/// tenant on their own bucket that is their own infrastructure described
/// back to them by a third party, and for anyone else it is somebody
/// else's. The crate's own doc already says the string is the operator's
/// and never the caller's.
fn unavailable(what: &str) -> ExecError {
    ExecError::new(
        ErrorCode::SpaceUnavailable,
        format!("{what}. Nothing was attached, and the identical call lands once it can be"),
    )
}

/// What the probe's failure means, split the one way an operator acts on.
///
/// A **refusal** is the credential: the target answered, and answered that
/// this credential may not do that there. The operator or the tenant fixes
/// the bucket policy or the installed key, and the identical call never
/// lands until they do, which is the 400 family's property.
///
/// A **transport** failure is the target: a 5xx, a TLS failure, a name that
/// does not resolve. Nothing about the call was wrong and the identical one
/// lands once the bucket answers, which is 503.
///
/// Neither carries the store's text, for [`unavailable`]'s reason.
fn probe_refusal(e: BlobError) -> ExecError {
    // Read from the error's type. A transient failure is the target; the
    // passing answers (`head` finding the object, not finding it) are not
    // refusals; anything else is the store refusing the credential.
    if e.is_transient() {
        return unavailable("that bucket could not be reached");
    }
    match e {
        BlobError::NotFound | BlobError::AlreadyExists | BlobError::Range => {
            ExecError::of_fault(e.class(), ErrorCode::Internal, format!("the attach-time probe answered `{e}`"))
        }
        _ => ExecError::new(
            ErrorCode::BucketRefused,
            "that bucket refused the credential this deployment holds for it. The              attachment was proved before it was stored rather than at the first upload,              so nothing was attached",
        ),
    }
}

fn verb(o: AttachOutcome) -> &'static str {
    match o {
        AttachOutcome::Attached => "attached",
        AttachOutcome::Replaced => "replaced",
        AttachOutcome::Unchanged => "unchanged",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The action names that reach the security stream. Asserted because
    /// an operator's alerting keys off them, and a renamed action is a
    /// silently disabled alert.
    #[test]
    fn the_recorded_actions_are_stable_names() {
        assert_eq!(verb(AttachOutcome::Attached), "attached");
        assert_eq!(verb(AttachOutcome::Replaced), "replaced");
        assert_eq!(verb(AttachOutcome::Unchanged), "unchanged");
    }

    /// **The deployment's own store is recognised through a spelling.**
    /// A unit test because the door-level refusal can only be driven with
    /// whichever spelling a test happens to send, and the property is that
    /// the comparison is on the endpoint's *host* and the bucket rather
    /// than on the URL: a port, a trailing element or a different case
    /// must not be a way past it.
    #[test]
    fn the_deployments_own_bucket_is_recognised_however_the_endpoint_is_spelt() {
        let own = OwnStore::at_endpoint("https://s3.eu-west-1.amazonaws.com", "platform-files");
        for spelling in [
            "https://s3.eu-west-1.amazonaws.com",
            "https://S3.EU-West-1.AmazonAWS.com",
            "https://s3.eu-west-1.amazonaws.com:443",
        ] {
            assert!(
                own.is(spelling, "platform-files"),
                "`{spelling}` is the deployment's own store and was not recognised"
            );
        }
        // And it does not collapse buckets or endpoints that genuinely
        // differ, so the assertions above are not passing because
        // everything matches everything.
        assert!(!own.is("https://s3.eu-west-1.amazonaws.com", "acme-files"));
        assert!(!own.is("https://gateway.acme.com", "platform-files"));
    }

    /// **The probe's two failures answer with two classes, and neither
    /// carries the store's text** (§10 B-X4.4).
    ///
    /// The split is the one an operator acts on: a bucket that refused the
    /// credential is somebody's policy to change and the identical call
    /// never lands until they do; a bucket that could not be reached is a
    /// retry. A unit test because the mapping is `probe_refusal`'s own and
    /// the door test can only reach it through a probe double.
    #[test]
    fn a_refused_probe_is_the_credentials_fault_and_an_unreachable_one_is_not() {
        let refused = probe_refusal(BlobError::Refused(
            "HTTP 403: AccessDenied for arn:aws:iam::1:user/ordo on acme-files".into(),
        ));
        assert_eq!(refused.code, ErrorCode::BucketRefused);
        let unreachable = probe_refusal(BlobError::Transport(
            "dial s3.acme.com:443: connection refused".into(),
        ));
        assert_eq!(unreachable.code, ErrorCode::SpaceUnavailable);
        for e in [&refused, &unreachable] {
            for leak in [
                "AccessDenied",
                "arn:aws",
                "acme-files",
                "connection refused",
            ] {
                assert!(
                    !e.message.contains(leak),
                    "`{leak}` reached the caller: {}",
                    e.message
                );
            }
        }
    }
}
