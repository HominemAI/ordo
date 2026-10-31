//! The platform's half of the blob door.
//!
//! `ordo-api` declares [`ordo_api::BlobPort`] and knows nothing about
//! schemas, pools or sealers — the crate depends on no sibling by design,
//! exactly as it does for `OpExecutor`. This is the implementation, and it
//! is where the three things the door's signature does not show actually
//! happen:
//!
//! 1. **The row guard runs.** The router's access check answers "may this
//!    principal call this op at all"; it does not answer "may this
//!    principal see this row". On a read that is
//!    `ordo_exec::blob::require_visible`; on a write it is the write
//!    protocol's own pair of checks, since C4 (below). Both evaluate
//!    `ordo_sema::guard_filter_query`, which the revert path also uses, so
//!    a guard that hides a row hides its file by construction rather than
//!    by two implementations agreeing.
//! 2. **The tier is chosen from the schema**, not from the request. A
//!    caller cannot ask for a blob to be stored inline, or externally;
//!    the field's declaration decided that, and the field's declaration is
//!    what is read here.
//! 3. **The inline cap is checked before the sealer**, so an oversize
//!    inline write is refused before a byte is sealed and before a key is
//!    released.
//!
//! # The order of a write, and why it is that order
//!
//! store the object → mint the token → hand it to the write protocol,
//! which locks the row, checks the guards, images it, inserts `$blobref`,
//! sets the column, re-checks the write guard, images it again, and
//! appends the group inside the transaction that commits all of it.
//!
//! The object goes first because the two failures are not symmetric. If
//! the transaction fails after the object is stored, the object is
//! unreferenced and the sweep collects it: that costs money. If the column
//! were written first and the object never arrived, a live row would point
//! at nothing, a dangling reference the shape gate cannot see, because
//! the column's type is still right. `plan-blob.md` §5.3 draws the same
//! asymmetry for collection and resolves it the same way: prefer the
//! failure that costs money over the one that costs a guarantee.
//!
//! The guard is checked **inside** the transaction, after the object is
//! stored. Checking it before storing would be cheaper and would be wrong
//! in one specific way: the check and the write would then be two
//! transactions, and a guard that stopped admitting the row in between
//! would let the write land anyway.
//!
//! # An upload is a converge (`docs/plan-blob-chain.md` §2)
//!
//! This door used to issue the two statements itself, through
//! `ordo_exec::blob::attach`, and appended no command group. So an upload
//! could not be reverted, fired no `converged` and no `written`, and did
//! not move `rev`. No other write of an entity row in this tree was
//! unrecorded like that.
//!
//! It issues no SQL now, and builds no image. What it does is refuse the
//! tiers it cannot serve, choose the tier from the schema, run the
//! pipeline, mint a [`PinnedUpload`] from what the pipeline returned, and
//! hand that to `Executor::attach_write`. Everything the write protocol
//! does for a `set` statement then happens for an upload, with no new
//! wiring: `classify` derives `converged` and `written` from the command
//! kind alone, `rev` moves because the platform converge emits the same
//! `rev = rev + 1`, and the group makes the upload revertible.
//!
//! # The object-first order leaks, and the leak used to be unreachable
//!
//! The asymmetry above is still the right one, preferring the failure that
//! costs money over the one that costs a guarantee. What was wrong is that
//! "the sweep collects it" was being read as though it were already true,
//! and it was not. `blobgc` finds unreferenced objects by anti-joining `$blobref`
//! against the schema's blob columns, so it can only see handles the
//! database already names. An upload interrupted between the `put` and the
//! commit leaves a handle **no** row names, and `BlobStore` has no `list`,
//! by design, so nothing in the deployment could enumerate it: not the
//! sweep, not an export, not an operator holding the bucket's credentials
//! and the database at once. The bytes stayed, unreachable, for as long as
//! the bucket existed, and on a tenant's own bucket they were a line on the
//! tenant's bill that the platform could neither account for nor delete.
//!
//! So this door declares the upload's intent before the `put`
//! ([`ordo_blob::UploadIntent`], recorded as a `$blobtomb` row by
//! [`Intent`] below), and the `$blobref` insert cancels that intent in the
//! same statement that writes it. An interrupted upload leaves an ordinary
//! tombstone, which the ordinary sweep retires on the ordinary floor. The
//! cost is one insert per upload. What it buys is that "the sweep collects
//! the leak" becomes a true sentence rather than the intention of one.

use std::sync::Arc;

use ordo_api::{
    ApiError, BlobBody, BlobMeta, BlobPort, BlobRead, BlobRequest, BlobVerb, ErrorCode,
};
use ordo_blob::{BlobPipeline, PipelineError, Stored, Upload, UploadIntent};
use ordo_conformance::write::PlatformValue;
use ordo_crypto::custody::{BlobHandle, SpaceId};
use ordo_exec::blob as durable;
use ordo_exec::FaultClass;
use ordo_schema::{FieldTy, Schema};
use ordo_syntax::ScalarTy;

/// The blob port over a live deployment.
pub struct PlatformBlobs {
    /// **The same resolver the op path holds** ([`crate::FixtureExecutor`]),
    /// not a second one built the same way, because an upload's command
    /// group joins the same per-space chain an op's group joins. A door
    /// with a log of its own would be a second chain for the same space,
    /// which is the one thing a chain cannot be, and with a log per space
    /// that is now a mistake two equivalent-looking resolvers would make
    /// silently.
    ///
    /// It is also where the read path finds the space's database, which is
    /// why the door holds no pool of its own. A pool here would be a second
    /// answer to "where are this space's rows", and for a space that had
    /// been moved it would read the references from the database the space
    /// left.
    pub spaces: Arc<crate::spaces::Spaces>,
    /// The sealing pipeline, which since `plan-tenant-storage.md` §11 U3
    /// holds a `StoreResolver` rather than one store.
    ///
    /// So this door needs no store of its own and must not grow one, for
    /// the reason the field above needs no pool: a store here would be a
    /// second answer to "where are this space's bytes", and for a space
    /// with a bucket of its own it would be the wrong one on the path that
    /// writes the bytes. The door names the space on every call and the
    /// pipeline resolves it; a bucket that cannot be resolved arrives as
    /// [`PipelineError::Store`] and leaves as `internal`, which is the
    /// same answer an unreachable store has always produced here.
    pub pipeline: BlobPipeline,
}

/// An upload that has been sealed, stored, and bound to exactly one row.
///
/// This is the token `docs/plan-blob-chain.md` §1 designs, and it is the
/// whole of the enforcement. Its fields are private, its one constructor is
/// [`mint`] below and is not `pub`, it derives nothing (no `Clone`, no
/// `Copy`, no `Default`, no `Debug`, and there is no serde in this crate's
/// dependency set to serialise it with), and
/// `Fixture::run_platform_converge` takes it **by value**. So a second
/// converge needs a second upload, and there is no argument beside the
/// token by which it could be applied to a row it was not minted for.
///
/// The precedent is `ordo_egress::PinnedDestination`, which stands for
/// "this destination passed all three pins, on this resolution, and may be
/// dialled once" in the same way and for the same reason. What a type
/// cannot say here is said by a source scan instead:
/// `the_seam_has_one_mint_site` asserts this struct is constructed in one
/// place in the workspace.
///
/// It consumes the [`Stored`] the pipeline returned, so the evidence that
/// bytes were sealed and written is spent when the binding is made.
struct PinnedUpload {
    entity: String,
    field: String,
    row_id: String,
    /// The hex handle, already quoted as a SQL literal. Computed at mint
    /// time so the protocol chooses no spelling: `ddl::column_ty` gives an
    /// external blob column `text`, and this is the one place that fact
    /// becomes a literal.
    column: String,
    /// The same handle as an image value. Derived here, compared by the
    /// protocol against the image it reads back off the row.
    image: ordo_commandlog::Value,
    /// The `$blobref` `INSERT`, rendered at mint time, so the door issues
    /// no SQL and the protocol has both statements in hand.
    reference: String,
}

impl PlatformValue for PinnedUpload {
    fn target(&self) -> (&str, &str, &str) {
        (&self.entity, &self.field, &self.row_id)
    }

    fn column_literal(&self) -> String {
        self.column.clone()
    }

    fn image_value(&self) -> ordo_commandlog::Value {
        self.image.clone()
    }

    fn reference_sql(&self) -> String {
        self.reference.clone()
    }
}

/// The one mint site in the workspace.
///
/// Not `pub`, and takes the `Stored` by value: what makes a `PinnedUpload`
/// trustworthy is that the only way to get one is to have run the pipeline,
/// and the only way to have run the pipeline is to have written the bytes.
///
/// There is no `inline` parameter and there must not be one. `blob inline`
/// is refused by name in [`PlatformBlobs::write`] before a byte is sealed,
/// so no token can exist for an inline field, and the column spelling here
/// is unconditionally the external one: the hex handle as a text literal,
/// which is what `ddl::column_ty` gives a non-sealed, non-inline blob
/// column. `attach`'s old two-armed literal is gone with it, because the
/// arm it would have chosen cannot be reached.
fn mint(
    stored: Stored,
    space: &SpaceId,
    entity: &str,
    field: &str,
    row_id: &str,
    created_at: i64,
) -> PinnedUpload {
    let reference = ordo_crypto::blobref::insert_sql(
        space,
        stored.handle(),
        stored.epoch(),
        entity,
        field,
        stored.mime(),
        stored.size(),
        stored.integrity(),
        stored.bytes(),
        created_at,
        // The bucket the bytes reached, which reads and the sweep follow
        // (ruled 2026-10-05, `detach-upload-race` ask 2).
        stored.store(),
    );
    let hex = ordo_crypto::hex(stored.handle());
    PinnedUpload {
        entity: entity.to_string(),
        field: field.to_string(),
        row_id: row_id.to_string(),
        column: ordo_exec::sql_lit(&hex),
        image: ordo_commandlog::Value::Text(hex),
        reference,
    }
}

/// The upload's declared intent, written to the space's own database.
///
/// One row in `$blobtomb`, before the bytes exist
/// ([`ordo_crypto::blobref::tomb_declare_sql`] argues the shape). It is
/// **not** written in the write protocol's transaction and must not be:
/// that transaction does not exist yet when this runs, and the whole value
/// of the row is that it survives the transaction never happening.
///
/// The pool is the space's, resolved through [`crate::spaces::Spaces`] like
/// every other per-space read this door makes, so a space that has been
/// moved declares its intent in the database it moved to rather than in the
/// one it left.
struct Intent {
    pool: ordo_exec::Pool,
    /// The same instant the `$blobref` row will carry as `created_at`, so
    /// the record of the object and the object's own row agree about when
    /// the upload began rather than differing by however long the bytes
    /// took to move.
    at: i64,
}

#[async_trait::async_trait]
impl UploadIntent for Intent {
    async fn declare(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        store: &str,
    ) -> Result<(), ordo_identity::StoreFault> {
        self.pool
            .rows(&ordo_crypto::blobref::tomb_declare_sql(
                space, handle, self.at, store,
            ))
            .await
            .map(|_| ())
            // The operator's text, never the caller's: `from_pipeline`
            // flattens `Undeclared` onto `internal("blob storage")` with
            // the custody and store failures, for the same reason. Its
            // class is the pool's, so a full pool stays busy.
            .map_err(ordo_exec::PoolError::into_fault)
    }
}

/// What the schema says about one blob field. Read from the declaration,
/// never from the request: a caller cannot ask for a blob to be stored
/// inline, to be treated as client-sealed, or to be sealed under the
/// space key.
struct Tier {
    inline: bool,
    private: bool,
    /// `blob sealed`: declarable (`ordo-schema` admits it) and refused
    /// here by name. See the refusal in `write`/`read` for why: it is not
    /// a missing branch, it is a tier the collector cannot serve.
    sealed: bool,
}

/// A refusal that names nothing. Used wherever distinguishing the reasons
/// would answer a question the caller has not earned — an absent handle, a
/// handle in another space, a row the guard hides — for `door_denial`'s
/// reason.
fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no such blob")
}

/// **A paused tenant's files are still** (27ar): the blob door neither
/// stores nor reads for it, as its ops are refused. Asked after the door's
/// access check, like the op path's gate, and answered with the same class.
fn paused(spaces: &crate::spaces::Spaces, space: &str) -> Result<(), ApiError> {
    match spaces.paused_record(space) {
        None => Ok(()),
        Some(record) => {
            let e = crate::pause::refusal(&record, &spaces.pause_answers());
            let mut err = ApiError::new(e.code, e.message);
            err.status = ordo_api::error::paused_status(e.code, e.status);
            Err(err)
        }
    }
}

fn internal(what: &str) -> ApiError {
    ApiError::new(ErrorCode::Internal, what.to_string())
}

/// A [`FaultClass`] as the blob door's answer, through the edge's one
/// mapper ([`ApiError::by_fault`]). A busy or retryable fault is the `busy`
/// answer the caller passes (the store's, or the plain `server_busy`);
/// every other class is the one flattened `internal("blob storage")`,
/// because the caller can do nothing about it and a more specific answer
/// would describe the deployment. A request's own faults (too large, a bad
/// range) are answered by [`from_pipeline`] before a class is asked.
fn by_class(class: FaultClass, busy: ApiError) -> ApiError {
    ApiError::by_fault(class, busy, internal("blob storage"))
}

/// An upload intent that could not be recorded (`PipelineError::Undeclared`),
/// by the fault's class. A full pool is `server_busy` (reported fix 25z).
fn store_failure(e: &ordo_identity::StoreFault) -> ApiError {
    by_class(e.class(), ApiError::server_busy())
}

/// File storage asked for a later attempt (a 5xx, a 429, `SlowDown`, a
/// connection that failed) and the binding's own retries ran out. A
/// `server_busy`, so it carries `Retry-After` and every client sends it
/// again; the message names no store.
fn storage_busy() -> ApiError {
    ApiError::new(
        ErrorCode::ServerBusy,
        "file storage did not answer in time. Nothing was changed, and the same request can          be sent again shortly",
    )
}

/// The blob store's error, by its class ([`ordo_blob::BlobError::class`]).
fn from_blob(e: &ordo_blob::BlobError) -> ApiError {
    by_class(e.class(), storage_busy())
}

/// Custody's error: a busy key store is busy, every other custody failure
/// stays the one opaque answer.
fn from_custody(e: &ordo_crypto::KeyError) -> ApiError {
    let class = if e.is_busy() {
        FaultClass::Busy
    } else {
        FaultClass::Broken
    };
    by_class(class, ApiError::server_busy())
}

/// The database's error, by its class (`PoolError::class`).
fn from_pool(e: &ordo_exec::PoolError) -> ApiError {
    by_class(e.class(), ApiError::server_busy())
}

/// Map a pipeline refusal onto the door's existing inventory.
///
/// **No new class.** "Too large" is `bound_violation`, which the door
/// already has a word for; a truncated body is `invalid_param`, because
/// the request did not deliver what it declared; custody and store
/// failures are `internal`, because the caller can do nothing about them
/// and a more specific answer would describe the deployment.
fn from_pipeline(e: PipelineError) -> ApiError {
    match e {
        PipelineError::TooLarge { limit, declared } => ordo_api::blob::too_large(limit, declared),
        PipelineError::Truncated { declared, got } => ApiError::new(
            ErrorCode::InvalidParam,
            format!("the upload declared {declared} bytes and delivered {got}"),
        ),
        PipelineError::Range => ApiError::new(
            ErrorCode::BoundViolation,
            "that range is not inside the object",
        ),
        // Deliberately flattened. `KeyError` is uninformative about *why*
        // an envelope did not open, on purpose, and a door that reported
        // "epoch 3's key is destroyed" versus "no lease" would be
        // answering questions about custody to whoever asked.
        //
        // `Undeclared` joins them rather than getting a class of its own.
        // The caller asked to store a file and the deployment could not
        // record that it was about to; that is the deployment's fault, it
        // is retryable, and naming it would tell the caller which of the
        // platform's two stores is unwell.
        PipelineError::Custody(e) => from_custody(&e),
        PipelineError::Store(e) => from_blob(&e),
        PipelineError::Undeclared(e) => store_failure(&e),
        _ => internal("blob storage"),
    }
}

/// Map a write-protocol refusal onto the door's existing inventory.
///
/// **No new class, and no second classifier.** The protocol's refusals are
/// already `class: message` strings from the closed inventory, and
/// [`crate::classify`] is the one thing in this crate that reads them: the
/// op path routes every write refusal through it, and an upload is now a
/// write. So a guard refusal arrives as `access_guard` and a vanished row
/// as `not_found`, which are the classes this door already answered with,
/// and anything unrecognised is `internal` rather than a class the door
/// invents.
fn from_protocol(e: ordo_exec::ExecFail) -> ApiError {
    let refusal = crate::exec_failure(e);
    ApiError::new(refusal.code, refusal.message)
}

fn from_refusal(e: durable::BlobRefusal) -> ApiError {
    match e {
        // Both flattened onto one answer: a handle that is not there and a
        // row this caller may not see are the same response, or the
        // difference is an oracle over sixteen guessable bytes.
        // The guard's refusal included (owner, 2026-10-25): a private file
        // another person may not read answers `not_found`, never
        // `access_guard`, which would say the handle exists.
        durable::BlobRefusal::NotFound | durable::BlobRefusal::Guard(_) => not_found(),
        durable::BlobRefusal::NotABlobField(m) => ApiError::new(ErrorCode::InvalidParam, m),
        durable::BlobRefusal::Store(e) => from_pool(&e),
        _ => internal("blob storage"),
    }
}

/// Resolve `entity.field` to a blob field, or refuse.
///
/// A field that is not a blob is `not_found` rather than a description of
/// the schema: the blob door must not become a way to enumerate which
/// fields of which entities exist.
fn tier_of(schema: &Schema, entity: &str, field: &str) -> Result<Tier, ApiError> {
    let e = schema.entity(entity).ok_or_else(not_found)?;
    let f = e.field(field).ok_or_else(not_found)?;
    match f.ty {
        FieldTy::Scalar(ScalarTy::Blob) => Ok(Tier {
            inline: f.inline,
            private: f.private,
            sealed: f.sealed,
        }),
        _ => Err(not_found()),
    }
}

fn parse_handle(hex: &str) -> Result<[u8; 16], ApiError> {
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(not_found());
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| not_found())?;
    }
    Ok(out)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl BlobPort for PlatformBlobs {
    async fn write(
        &self,
        req: &BlobRequest,
        declared_len: u64,
        mime: &str,
        body: BlobBody,
    ) -> Result<(), ApiError> {
        let (Some(entity), Some(field), Some(row_id)) = (&req.entity, &req.field, &req.row_id)
        else {
            return Err(internal("a blob write names an entity, a field and a row"));
        };
        paused(&self.spaces, &req.space)?;
        // One read of the live program and of the space binding, held for
        // the whole call, for `FixtureExecutor::execute`'s reason: a deploy
        // landing mid-upload must not move the schema out from under a
        // write that has already chosen its tier from it.
        //
        // The space is the request's, and resolving it here is what closes
        // the asymmetry `tests/spaces.rs` aims at: the sealer and the
        // `$blobref` row took their space from the header while the
        // converge ran through an executor whose space was fixed at
        // construction. The two coincided in every deployment there was,
        // which is exactly the kind of agreement that stops holding the
        // day a second tenant exists.
        let bound = self.spaces.resolve(&req.space).await.map_err(|e| {
            if e.is_busy() {
                ApiError::server_busy()
            } else {
                internal(&e.to_string())
            }
        })?;
        let schema = &bound.live.fixture.schema;
        let tier = tier_of(schema, entity, field)?;
        let space = bound.view.space.clone();

        // `blob inline` never reaches the object store — its bytes are a
        // `$sealedpayload` row, which is what makes the tier GC-free by
        // inheritance rather than by assertion. That path needs a command
        // group to carry the payload's correlation, which is the write
        // protocol's, so it is refused honestly here rather than served by
        // a second write path that would not be command-logged.
        // **`blob private` is declarable and not servable**, and the
        // refusal names why rather than reading as a missing feature.
        //
        // Client-sealed custody means the platform holds no key: the
        // client encrypts before upload and the platform streams
        // ciphertext as-is. This door cannot do that — the pipeline seals
        // on the way in, which is the whole reason a space-sealed upload
        // proxies through the API at all — and it must not simply skip the
        // sealer, because then a `private` field's bytes would reach the
        // bucket in whatever state the client sent them, with nothing
        // having checked that the client sealed anything.
        //
        // What is owed is F7 Phase 6: the passkey-PRF derivation, the
        // device wrapping, and the presigned direct-to-bucket path that is
        // safe for `private` precisely because the bytes are already
        // ciphertext when they leave the client. The seam is built — this
        // crate never inspects a byte, so a client-sealed object rides the
        // identical `put`/`get`.
        if tier.private {
            return Err(ApiError::new(
                ErrorCode::InvalidParam,
                format!(
                    "`{entity}.{field}` is `blob private`: client-sealed custody, where the \
                     platform holds no key. This door seals on the way in, so it cannot \
                     serve that tier — and skipping the sealer would put whatever the \
                     client sent into the bucket with nothing having checked it was \
                     sealed. The presigned direct-to-bucket path that makes `private` \
                     work is F7 Phase 6's, and it is not built."
                ),
            ));
        }
        if tier.inline {
            // `invalid_param`, not a new class. The request named a field
            // this route does not serve, which is a property of the
            // request; a `blob_inline_unsupported` would be a new refusal
            // class, and the standing rule is that a new class is an
            // oracle until argued.
            return Err(ApiError::new(
                ErrorCode::InvalidParam,
                format!(
                    "`{entity}.{field}` is `blob inline`, whose bytes ride the row's sealed \
                     payload and are therefore written by the write protocol inside a \
                     command group — not by the blob door, which has no group to attribute \
                     them to. Declare the field as an external `blob` to upload through \
                     this route."
                ),
            ));
        }

        if tier.sealed {
            // **The third of the pattern beside `private` and `inline`,
            // and for a different reason than either.** `blob sealed` is
            // declarable and passes `tier_of` above; nothing downstream
            // models it. `ddl::column_ty` gives the column `bytea` for any
            // sealed field before it ever reaches the blob arm
            // (`ordo-schema/src/ddl.rs:90`), while `attach` would write the
            // hex handle as a **text** literal, the same as any other
            // external blob, so the column would hold the ASCII of a hex
            // string rather than a handle or an envelope. That is a
            // symptom, not the reason.
            //
            // The reason is the collector. `blobgc::live_reference_clause`
            // proves a `$blobref` row is live by comparing the entity's
            // column against `encode(b."ref", 'hex')`, which only works
            // when the column holds the same handle the reference row
            // does. A sealed handle is minted fresh per call, which is the
            // whole point of sealing it, so two writes of the identical
            // underlying handle never produce equal envelopes, and no
            // column value the collector could compare against would ever
            // equal another. This is not a missing branch the collector
            // forgot: it holds no keys on purpose, so it cannot open an
            // envelope to see what is inside, and there is no keyless
            // comparison that makes two sealed handles of the same object
            // equal. Serving this tier needs a keyed collector or a
            // deterministic blind index derived from the handle, and
            // neither exists. Excluding the field from the collector
            // instead (this build does) only stays sound because this
            // refusal keeps the column from ever holding a live handle in
            // the first place.
            //
            // `invalid_param`, not a new class, for the same reason
            // `inline` isn't one: the request named a field this route
            // does not serve, which is a property of the request. The
            // declaration stays legal in the language so a later build can
            // serve it without a syntax change.
            return Err(ApiError::new(
                ErrorCode::InvalidParam,
                format!(
                    "`{entity}.{field}` is `blob sealed`, which is declarable but not \
                     servable: the collector matches live references by comparing the \
                     column against the handle and holds no keys on purpose, and a sealed \
                     handle carries a fresh nonce, so equal handles never produce equal \
                     envelopes and the comparison cannot be made to work at all. Serving \
                     this tier needs a keyed collector or a deterministic blind index \
                     derived from the handle, and neither is built."
                ),
            ));
        }

        // The space's own database, for the intent row. Resolved before the
        // pipeline runs so an unreachable database refuses the upload at
        // its start rather than after the bytes have moved, and so that
        // the one thing that must precede the `put` cannot fail for a
        // reason the `put` already paid for.
        let pool = self
            .spaces
            .pool_of(&space.0)
            .await
            .map_err(|e| by_class(e.class(), ApiError::server_busy()))?;
        // Minted once and used twice: the intent row's `unreferenced_since`
        // and the `$blobref` row's `created_at`. Two calls to `now_ms`
        // would put the record of the object before the object by however
        // long the upload took, which reads as a clock disagreeing with
        // itself in the one table an operator consults when storage will
        // not shrink.
        let at = now_ms();
        let intent = Intent { pool, at };

        let stored = self
            .pipeline
            .put(
                &Upload {
                    space: &space,
                    entity,
                    field,
                    mime,
                    plain_len: declared_len,
                    correlation: &req.correlation,
                    // **The line that makes the leak collectable.** Without
                    // it the bytes below reach the store with nothing in
                    // the database naming them, and if the transaction
                    // underneath never commits, nothing ever will.
                    intent: Some(&intent),
                },
                Box::pin(futures::StreamExt::map(body, |r| {
                    r.map_err(ordo_blob::BlobError::Transport)
                })),
            )
            .await
            .map_err(from_pipeline)?;

        // The bytes are written. Binding them to a row is a converge, and
        // the token is what the write protocol converges with. The door's
        // last act is to mint it; every statement after this is the
        // protocol's, inside one SERIALIZABLE transaction that also appends
        // the group.
        //
        // The guard is not checked here. It used to be, with
        // `require_visible(write=true)` inside a transaction this function
        // opened. The protocol's own check is strictly stronger: it locks
        // the row before evaluating anything, and it re-evaluates the write
        // guard on the row as it now is, so an upload that moves a row out
        // of the caller's write view is refused and rolled back rather than
        // committed on the strength of a check made before the mutation.
        //
        // A refusal rolls the transaction back, so no `$blobref` row and no
        // column survive. The object does: it is now unreferenced, and it
        // is not deleted here, because deleting it would be a second
        // failure path racing the sweep, and the sweep is the mechanism
        // that is allowed to remove bytes, under the undo-horizon floor and
        // with a re-check. Leaking an object costs money; a delete path
        // that can run without the floor costs the guarantee.
        //
        // The intent row above is what the rollback leaves behind, and it
        // is what makes the sentence "the sweep collects it" true. On
        // success the `$blobref` insert inside this transaction cancels the
        // intent in the same statement, so a committed upload is never a
        // live object with a tombstone queued against it.
        let value = mint(stored, &space, entity, field, row_id, at);
        bound
            .view
            .exec
            .attach_write(
                bound.view.fixture.clone(),
                bound.log.clone(),
                value,
                req.principal.clone(),
                req.agent.clone(),
            )
            .await
            .map(|_| ())
            .map_err(from_protocol)
    }

    async fn read(
        &self,
        req: &BlobRequest,
        from: u64,
        len: Option<u64>,
    ) -> Result<BlobRead, ApiError> {
        let Resolved {
            space,
            reference,
            handle,
            mime,
            plain_len,
        } = self.resolve(req).await?;

        let partial = from != 0 || len.is_some();
        // From the bucket the reference row names, not from where the
        // space's uploads go now. An upload that landed while the tenant's
        // bucket was being detached or replaced is in the old bucket and
        // stays readable there (ruled 2026-10-05, `detach-upload-race` ask
        // 2); nothing was checked or refused when it was written.
        let bytes = self
            .pipeline
            .in_store(&reference.store)
            .get(
                &space,
                &reference.entity,
                &reference.field,
                &handle,
                plain_len,
                from,
                len,
                &req.correlation,
            )
            .await
            .map_err(from_pipeline)?;

        Ok(BlobRead {
            mime,
            plain_len,
            bytes,
            from,
            partial,
        })
    }

    async fn meta(&self, req: &BlobRequest) -> Result<BlobMeta, ApiError> {
        let r = self.resolve(req).await?;
        Ok(BlobMeta {
            mime: r.mime,
            size: r.plain_len,
        })
    }
}

impl PlatformBlobs {
    /// Everything a read knows before a byte moves: the reference row, the
    /// tier checks, the row guard, and the opened media type and size. The
    /// bytes read and the metadata read both go through here, so the guard
    /// that hides a file hides its type and size by construction.
    async fn resolve(&self, req: &BlobRequest) -> Result<Resolved, ApiError> {
        let Some(hex) = &req.handle_hex else {
            return Err(internal("a blob read names a handle"));
        };
        paused(&self.spaces, &req.space)?;
        let handle = parse_handle(hex)?;
        let space = SpaceId(req.space.clone());
        let pool = self
            .spaces
            .pool_of(&req.space)
            .await
            .map_err(|e| by_class(e.class(), ApiError::server_busy()))?;

        // The reference row carries the entity and the field, which is why
        // it carries them: the AAD binds `(entity, field)`, so a resolver
        // must not have to be told which column it is opening for.
        let reference = durable::lookup(&pool, &space, &handle)
            .await
            .map_err(|e| from_pool(&e))?
            .ok_or_else(not_found)?;

        // The schema, not a space view: the program is deployment-wide, and
        // every space-carrying value on this path (`space`, the reference
        // row, the pipeline's lease) already comes from the request's own
        // binding.
        let schema = &self.spaces.deployment().current().fixture.schema;
        let tier = tier_of(schema, &reference.entity, &reference.field)?;
        if tier.private {
            // Symmetric with the write refusal, and it has to be: serving
            // a `private` object means streaming ciphertext the caller
            // decrypts, with the key riding the URL fragment. Opening it
            // here is impossible — the platform holds no key — and
            // streaming it as-is through a path whose contract is "the
            // bytes you get are plaintext" would hand a caller ciphertext
            // labelled as their file.
            return Err(ApiError::new(
                ErrorCode::InvalidParam,
                "that field is `blob private`: the platform holds no key for it, so it \
                 cannot be served through this route. The presigned path with the key in \
                 the URL fragment is F7 Phase 6's, and it is not built.",
            ));
        }
        if tier.sealed {
            // **Also refused on read, and it has to be, for a different
            // reason than `private`'s.** Nothing stops this handler from
            // decrypting a sealed field's envelopes; the mime and size
            // sealing here is the same pipeline every tier uses. The
            // reason to refuse anyway is `live_reference_clause`: this
            // build excludes sealed blob fields from the collector's
            // reference query (`blobgc.rs`), which is only sound because
            // the write refusal above keeps a sealed column from ever
            // holding a live handle. If this route served one anyway, a
            // `$blobref` row reachable only through a sealed column would
            // be a live reference the collector does not know about, and
            // it would be swept as garbage while still readable through
            // here, the exact dangling-reference failure the door's
            // module doc calls "a guarantee" rather than "money". A tier
            // that cannot be written but can still be read is its own
            // trap: refusing both keeps "no live object under a sealed
            // column" true in both directions this door has.
            return Err(ApiError::new(
                ErrorCode::InvalidParam,
                "that field is `blob sealed`: declarable but not servable. The collector \
                 leaves it out of the live-reference query, because a sealed handle \
                 carries a fresh nonce and the collector holds no keys, so the comparison \
                 cannot be made to work at all. Writes to the field are refused for the \
                 same reason, and reads are refused with them: an object the collector \
                 cannot see is an object the sweep may delete, so this route must not \
                 report one as readable.",
            ));
        }

        // A visitor with no token reads a file only when its field is public:
        // files follow their row (`design-public-path.md` §1). Anything else
        // is the door's one answer, the same bytes a tokenless request got
        // before the public path existed.
        let anonymous = req.principal == ordo_api::ANONYMOUS;
        if anonymous {
            let public = schema
                .entity(&reference.entity)
                .and_then(|e| e.field(&reference.field))
                .is_some_and(|f| f.public);
            if !public {
                return Err(
                    ApiError::new(ErrorCode::AccessDenied, "missing bearer token")
                        .with_op(ordo_api::BlobVerb::Read.std_op(ordo_api::blob::BLOB_ENTITY)),
                );
            }
        }

        // The row the handle is attached to, and then the guard over it.
        // A handle whose row was deleted resolves to no row, which is
        // `not_found` — the bytes may still be there (the undo horizon
        // keeps them, because a reverted delete must find them), but they
        // are not reachable through a door that serves rows.
        let row_id = durable::owning_row(
            &pool,
            &space,
            &reference.entity,
            &reference.field,
            tier.inline,
            &handle,
        )
        .await
        .map_err(|e| from_pool(&e))?
        .ok_or_else(not_found)?;

        // A public field's entity has no read guard (`public_guarded`), so a
        // visitor's read has no guard to walk.
        if !anonymous {
            let tx = pool.begin().await.map_err(|e| from_pool(&e))?;
            let guard = durable::require_visible(
                &tx,
                schema,
                &space,
                &reference.entity,
                &row_id,
                &req.principal,
            )
            .await;
            let _ = tx.rollback().await;
            guard.map_err(from_refusal)?;
        }

        // The mime and the plaintext size are sealed envelopes on the
        // reference row. Opening them is a key use like any other and runs
        // under the same correlation as the range read below.
        let lease = self
            .pipeline
            .keys
            .release(&space, ordo_crypto::Purpose::Read, &req.correlation)
            .map_err(|e| from_custody(&e))?;
        let object = ordo_crypto::hex(&handle);
        let open = |bytes: &[u8]| {
            self.pipeline
                .keys
                .open_at(
                    &lease,
                    &reference.entity,
                    &reference.field,
                    ordo_crypto::Anchor::Object(&object),
                    bytes,
                )
                .map_err(|e| from_custody(&e))
        };
        let mime = String::from_utf8(open(&reference.mime)?)
            .unwrap_or_else(|_| "application/octet-stream".into());
        let plain_len: u64 = String::from_utf8(open(&reference.size)?)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| internal("blob storage"))?;
        Ok(Resolved {
            space,
            reference,
            handle,
            mime,
            plain_len,
        })
    }
}

/// What [`PlatformBlobs::resolve`] found.
struct Resolved {
    space: SpaceId,
    reference: ordo_crypto::blobref::BlobRef,
    handle: BlobHandle,
    mime: String,
    plain_len: u64,
}

/// **A bulk import's objects go through this door, and through nothing
/// else.**
///
/// `ordo_exec::import` loads an export bundle's rows, and a file is not a
/// column: `Sema::check_content_params` refuses a blob-typed op parameter,
/// so no create can carry one, and the bundle's own blob column is the
/// **source** space's handle, which names an object in the source's bucket
/// under the source's key. So the rows go in first and each object is bound
/// to the row the import minted for it, here, by the ordinary upload.
///
/// This implementation is four lines of adaptation and that is the whole
/// point of it. Every property of an upload is inherited rather than
/// restated for the import case: `private`, `inline` and `sealed` are
/// refused by name, the tier comes from the schema and not from the caller,
/// the intent is declared before the bytes so an interrupted import's
/// objects are collectable, the bytes are sealed under the **destination**
/// space's key, the row is locked and its write guard evaluated before and
/// after the mutation, the digest rides the image, and the whole of it is
/// one command group in the destination's chain. An import that wrote the
/// column itself would have none of that, and it would be the second upload
/// path `docs/plan-blob-chain.md` C1–C8 ruled out — which
/// `tests/upload_seam.rs` asserts against this workspace's source, mint site
/// and all.
///
/// The seam points this way because of where the crates are:
/// `ordo_exec::import` is three crates below this one, so the trait is
/// declared there and implemented here, exactly as `PlatformValue` is for
/// the same reason (§7.1, ruled by the owner on 2026-09-09).
#[async_trait::async_trait]
impl ordo_exec::import::BlobDoor for PlatformBlobs {
    async fn attach(&self, object: ordo_exec::import::ImportObject<'_>) -> Result<(), String> {
        let declared_len = object.bytes.len() as u64;
        let request = BlobRequest {
            space: object.space.to_string(),
            principal: object.principal.to_string(),
            // An import runs as the importing principal, never on behalf
            // of anyone.
            agent: None,
            verb: BlobVerb::Write,
            entity: Some(object.entity.to_string()),
            field: Some(object.field.to_string()),
            row_id: Some(object.row_id.to_string()),
            // A write addresses a row and a column; the handle is the
            // destination's to mint, and an import that named one would be
            // asking for the source's.
            handle_hex: None,
            correlation: object.correlation.to_string(),
        };
        let bytes = object.bytes;
        let body: BlobBody = Box::pin(futures::stream::once(async move {
            Ok::<Vec<u8>, String>(bytes)
        }));
        self.write(&request, declared_len, object.mime, body)
            .await
            .map_err(|e| format!("{}: {}", e.code.as_str(), e.message))
    }
}

/// The verb a request carries, exposed so a caller can log it without
/// re-deriving it. Kept here rather than in `ordo-api` because the door's
/// own code branches on the route, not on this.
pub fn verb_of(req: &BlobRequest) -> BlobVerb {
    req.verb
}

#[cfg(test)]
mod pins {
    use super::PinnedUpload;

    /// **`PinnedUpload` implements neither `Clone` nor `Copy`**, asserted at
    /// compile time rather than by reading the source.
    ///
    /// The trick is that an inherent associated const outranks a trait's
    /// const of the same name, and the inherent one below only exists when
    /// `T: Clone`. So `Probe::<T>::CLONEABLE` resolves to `true` for a
    /// cloneable `T` and falls through to the blanket trait's `false` for
    /// one that is not. A `Clone` derive added to the token turns this
    /// assertion red on the line that added it.
    ///
    /// Why it matters: the ruling in `corpus-rulings.md` is that the pin is
    /// a constructor and a move, the way `PinnedDestination` is. A clone
    /// would make one upload into two converges, each asserting in the
    /// chain that an object was written for it, when the store saw one set
    /// of bytes.
    struct Probe<T>(std::marker::PhantomData<T>);

    trait NotCloneable {
        const CLONEABLE: bool = false;
    }

    impl<T> NotCloneable for Probe<T> {}

    impl<T: Clone> Probe<T> {
        const CLONEABLE: bool = true;
    }

    /// `const`, so this is decided when the crate compiles and not when
    /// the suite runs. A `#[test]` would have been the weaker form of the
    /// same claim: it would let a cloneable token exist in a build until
    /// somebody ran the tests.
    #[test]
    fn pin_upload_is_move_only() {
        const {
            assert!(
                !Probe::<PinnedUpload>::CLONEABLE,
                "an upload token that can be cloned is an upload that can be converged \
                 twice, and the second converge names bytes nobody stored a second time"
            )
        };
        // The probe itself has to be able to answer `true`, or the
        // assertion above is satisfied by a broken oracle rather than by
        // the property.
        const { assert!(Probe::<String>::CLONEABLE, "the probe detects `Clone`") };
    }
}
