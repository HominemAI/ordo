//! The bytes tier: one store trait, two bindings, and the per-space
//! resolver.
//!
//! `definition-language.md` §2, Blob storage pipeline [settled]: *"one
//! straight path — bytes → (sealer, per custody) → the space's resolved
//! store. The store resolves per space: platform R2/S3 by default, BYO
//! bucket as a first-class resolver target."*
//!
//! # What this crate is not
//!
//! It is not the sealer, and it never inspects a byte. Sealing happens
//! **before** the bytes reach a store and opening happens after they leave
//! one, which is what lets `blob private` — client-sealed under a
//! passkey-PRF key the platform never holds — ride the identical
//! `put`/`get` with no branch here. `f7-vault-plan.md` Phase 6 opens the
//! presigned direct-to-bucket path for that tier; the seam it needs is
//! this crate having no opinion about what is inside an object.
//!
//! It also does not create the `$blobref` table or mint handles. Those are
//! `ordo_crypto::blobref` and `ordo_crypto::custody::KeyProvider::mint_blob_ref`,
//! and they live there so that nothing which creates a Postgres table has
//! to depend on a TLS stack.
//!
//! # The two seams, and why they are two
//!
//! [`BlobStore`] is *where the bytes are*. [`StoreResolver`] is *which
//! store a space's bytes are in*. They are separate for the reason
//! `KeyProvider` and `WrappedKeyStore` are separate — *"so a deployment's
//! storage choice and its custody choice are independent"* — one level
//! down: a deployment's **default** store and a space's **BYO bucket** are
//! independent, and BYO is then an entry in a resolver rather than a
//! branch inside every call site.
//!
//! **Everything in this tree that touches bytes asks the resolver.** The
//! sweep, the re-seal pass and `erase_space` always did;
//! [`pipeline::BlobPipeline`] joined them in `plan-tenant-storage.md` §11
//! U3, which is what closed the gap that mattered most. The pipeline is
//! the only path that *creates* objects, so a per-space seam every reader
//! had and the writer did not was a seam a tenant's bytes could be written
//! past. Nothing outside a resolver holds an `Arc<dyn BlobStore>` now; a
//! caller that needs the bytes asks [`pipeline::BlobPipeline::store_for`]
//! and names the space. Resolution is `async` from U5 onwards, because an
//! attached resolver answers out of the control database; [`StoreResolver`]
//! argues that at the trait.
//!
//! [`SingleStore`] is the default resolver and returns one store for every
//! space. That is honest about a real absence: there is no space directory
//! in this tree — `PoolConfig` is read from env and takes no space,
//! `PlatformState.space` is a `String` — and `plan-schema-evolution.md` §1
//! records the same absence for per-space shape versions. The resolver is
//! written to want one; it is given a single-store default until there is
//! one to want.
//!
//! # The object key
//!
//! `<space>/<hex handle>` and nothing else. No entity, no field, no date,
//! no epoch — see [`ordo_crypto::blobref`] for why each is absent, the
//! sharpest being the epoch: rotation is mandatory on membership
//! revocation, so an epoch in a bucket listing prints a **revocation
//! timeline** to someone holding no keys.
//!
//! The space *is* in the key, and that is a considered exception rather
//! than an oversight. Hashing it was declined: it would leave an operator
//! holding a bucket and an incident unable to answer "which objects are
//! this tenant's" without the database that is currently down, and the
//! space id is not what this design conceals — file equality is. What the
//! prefix must never do is *escape*, so [`space_prefix`] refuses any space
//! id outside a strict charset rather than sanitising one.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::Stream;
use ordo_crypto::custody::{BlobHandle, SpaceId};

pub mod contract;
pub mod fs;
pub mod http;
pub mod pipeline;
pub mod policy;
pub mod s3;

pub use fs::FsStore;
pub use pipeline::{BlobPipeline, PipelineError, Stored, Upload, UploadIntent};
pub use policy::BlobPolicy;
pub use s3::{S3Config, S3Store};

/// A stream of plaintext-or-ciphertext chunks. The crate never looks
/// inside one.
///
/// `Vec<u8>` rather than a `Bytes`-shaped type because the workspace has
/// no `bytes` dependency and this crate is already the one that adds a
/// dependency axis; one more for a reference-counted buffer is not worth
/// it at these chunk sizes.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, BlobError>> + Send>>;

/// A half-open byte range over an object's stored (ciphertext) bytes.
///
/// `len` is `None` for "to the end". Ranges are over the **stored** bytes,
/// not the plaintext, because that is what a store can serve without a
/// key; translating a plaintext range into a ciphertext range is the
/// chunked-STREAM layer's job, one level up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub offset: u64,
    pub len: Option<u64>,
}

impl ByteRange {
    pub fn new(offset: u64, len: Option<u64>) -> Self {
        ByteRange { offset, len }
    }

    /// The inclusive last byte, for stores that spell ranges that way
    /// (which is both of them: HTTP `Range` is inclusive, and so is the
    /// arithmetic a file seek needs).
    pub fn last(&self) -> Option<u64> {
        self.len.map(|n| self.offset + n.max(1) - 1)
    }
}

/// What a store knows about an object without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectHead {
    /// The stored length in bytes — ciphertext for a sealed field. This is
    /// the number `$blobref."bytes"` carries in plaintext, and the reason
    /// it is safe to carry in plaintext is that it is exactly this:
    /// already observable to anyone who can list the bucket.
    pub bytes: u64,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BlobError {
    /// No object under that key in that space.
    NotFound,
    /// An object already exists under that key.
    ///
    /// Not a normal path and not a race to paper over: handles are
    /// keyed-random and minted per upload, so a second `put` under one
    /// handle means either a repeated CSPRNG seed inside one space-epoch
    /// or a bug. `mint_blob_ref`'s salt is what makes the first case a
    /// refusal here instead of a cross-tenant equality signal, and this
    /// error is the refusal.
    AlreadyExists,
    /// The range asked for is not inside the object.
    Range,
    /// The transport failed with no client error to keep: a protocol
    /// fault, a dropped connection. Transient:
    /// [`BlobError::is_transient`]. Carries the operator's text; never the
    /// caller's.
    Transport(String),
    /// The store refused with no status to keep: a request the binding
    /// itself would not send, a filesystem answer that is not "absent".
    /// Permanent.
    Refused(String),
    /// The binding cannot be configured as asked. Raised at construction,
    /// never mid-request.
    Config(String),
    /// The store answered with an HTTP status that is not a success. The
    /// status and the store's `Retry-After` are kept, and the class is read
    /// from them ([`BlobError::class`]): 5xx, 408, 425 and 429 ask for a
    /// later attempt (S3's `SlowDown` is a 503), 404 is not found, 401 and
    /// 403 are forbidden, 413 and 416 are bad input, other 4xx are broken.
    Status {
        status: u16,
        /// The store's `Retry-After`, when it sent seconds.
        retry_after: Option<std::time::Duration>,
        /// The store's own words, for the owner's log.
        message: String,
    },
    /// A client error underneath (a socket, a TLS session, a file, the
    /// database that names a space's bucket), kept as the error's
    /// [`source`](std::error::Error::source) with the class its owner gave
    /// it.
    Caused {
        context: String,
        class: ordo_fault::FaultClass,
        source: Arc<dyn std::error::Error + Send + Sync>,
    },
}

impl PartialEq for BlobError {
    fn eq(&self, other: &BlobError) -> bool {
        match (self, other) {
            (BlobError::NotFound, BlobError::NotFound)
            | (BlobError::AlreadyExists, BlobError::AlreadyExists)
            | (BlobError::Range, BlobError::Range) => true,
            (BlobError::Transport(a), BlobError::Transport(b))
            | (BlobError::Refused(a), BlobError::Refused(b))
            | (BlobError::Config(a), BlobError::Config(b)) => a == b,
            (
                BlobError::Status {
                    status: s1,
                    retry_after: r1,
                    message: m1,
                },
                BlobError::Status {
                    status: s2,
                    retry_after: r2,
                    message: m2,
                },
            ) => s1 == s2 && r1 == r2 && m1 == m2,
            (
                BlobError::Caused {
                    context: c1,
                    class: k1,
                    source: e1,
                },
                BlobError::Caused {
                    context: c2,
                    class: k2,
                    source: e2,
                },
            ) => c1 == c2 && k1 == k2 && e1.to_string() == e2.to_string(),
            _ => false,
        }
    }
}

impl Eq for BlobError {}

impl fmt::Display for BlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobError::NotFound => f.write_str("no object under that handle"),
            BlobError::AlreadyExists => f.write_str("an object already exists under that handle"),
            BlobError::Range => f.write_str("that range is not inside the object"),
            BlobError::Transport(m) => write!(f, "blob store transport: {m}"),
            BlobError::Refused(m) => write!(f, "blob store refused: {m}"),
            BlobError::Config(m) => write!(f, "blob store configuration: {m}"),
            BlobError::Status {
                status, message, ..
            } => write!(f, "blob store answered HTTP {status}: {message}"),
            BlobError::Caused {
                context, source, ..
            } => write!(f, "blob store: {context}: {source}"),
        }
    }
}

impl std::error::Error for BlobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BlobError::Caused { source, .. } => {
                Some(&**source as &(dyn std::error::Error + 'static))
            }
            _ => None,
        }
    }
}

impl BlobError {
    /// A client error underneath, with the context that names the step and
    /// the class its owner gave it.
    pub fn caused_by(
        context: impl Into<String>,
        class: ordo_fault::FaultClass,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> BlobError {
        BlobError::Caused {
            context: context.into(),
            class,
            source: Arc::new(source),
        }
    }

    /// A network step that failed (connect, TLS, read, write): transient.
    pub fn network(context: impl Into<String>, source: std::io::Error) -> BlobError {
        BlobError::caused_by(context, ordo_fault::FaultClass::Retryable, source)
    }

    /// A store's HTTP answer that is not a success.
    pub fn status(
        status: u16,
        retry_after: Option<std::time::Duration>,
        message: String,
    ) -> BlobError {
        BlobError::Status {
            status,
            retry_after,
            message,
        }
    }

    /// The HTTP status the store answered, when it answered one.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            BlobError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// How long the store asked the caller to wait, when it said.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            BlobError::Status { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    fn io_kind(&self) -> Option<std::io::ErrorKind> {
        match self {
            BlobError::Caused { source, .. } => {
                source.downcast_ref::<std::io::Error>().map(|e| e.kind())
            }
            _ => None,
        }
    }

    /// Is another attempt worth making? Decided here, once, from the type:
    /// the S3 binding retries these with a bounded backoff, and the blob
    /// door answers them `server_busy` (retried by every client), never 500.
    pub fn is_transient(&self) -> bool {
        self.class().may_retry()
    }

    /// No object there (a 404, an absent file).
    pub fn is_not_found(&self) -> bool {
        match self {
            BlobError::NotFound => true,
            BlobError::Status { status, .. } => *status == 404,
            _ => self.io_kind() == Some(std::io::ErrorKind::NotFound),
        }
    }

    /// The store refused this deployment's credentials or permissions
    /// (401, 403, a permission error). The deployment owner fixes it; it is
    /// not the caller's input, so its class is broken.
    pub fn is_forbidden(&self) -> bool {
        match self {
            BlobError::Status { status, .. } => matches!(status, 401 | 403),
            _ => self.io_kind() == Some(std::io::ErrorKind::PermissionDenied),
        }
    }

    /// The request, not the store: a range outside the object, a key that
    /// exists, a body the store calls too large (413) or a range it cannot
    /// serve (416).
    pub fn is_bad_input(&self) -> bool {
        match self {
            BlobError::Range | BlobError::AlreadyExists => true,
            BlobError::Status { status, .. } => matches!(status, 413 | 416),
            _ => self.io_kind() == Some(std::io::ErrorKind::AlreadyExists),
        }
    }

    /// The store is rate limiting: a 429 or a 503 (`SlowDown`), any status
    /// that carried a `Retry-After`, or a busy database underneath.
    pub fn is_busy(&self) -> bool {
        self.class().is_busy()
    }

    /// What a caller can do about it. Busy: the store is rate limiting, send
    /// the same work shortly. Retryable: a transport fault or another 5xx,
    /// 408 or 425. Refused: not found, bad input. Broken: forbidden, any
    /// other store refusal, a configuration fault.
    pub fn class(&self) -> ordo_fault::FaultClass {
        use ordo_fault::FaultClass;
        if self.is_not_found() || self.is_bad_input() {
            return FaultClass::Refused;
        }
        match self {
            BlobError::Transport(_) => FaultClass::Retryable,
            BlobError::Status {
                status,
                retry_after,
                ..
            } if transient_status(*status) => {
                if matches!(status, 429 | 503) || retry_after.is_some() {
                    FaultClass::Busy
                } else {
                    FaultClass::Retryable
                }
            }
            BlobError::Caused { class, .. } => *class,
            _ => FaultClass::Broken,
        }
    }
}

/// Is an HTTP status one that asks for a later attempt? Every 5xx, and 408,
/// 425 and 429. The same rule as `ordo_egress::refusal::retryable_status`.
pub fn transient_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500..=599)
}

/// Where the bytes are.
///
/// Four methods, streaming both ways. There is deliberately no `list`: a
/// listing is how a bucket enumerates a tenant, and the platform never
/// needs one — the reference set is `$blobref`, which is authoritative and
/// which the sweep already reads. A store that could list would make
/// "reconcile the bucket against the database" look like a cheap idea, and
/// that reconciliation is the one operation whose failure mode is deleting
/// bytes a revert still needs.
#[async_trait::async_trait]
pub trait BlobStore: Send + Sync {
    /// A name for the binding, for the boot refusal and for operator
    /// messages. Not a URL: it must be safe to print.
    fn binding(&self) -> &'static str;

    /// Whether this binding may back a production deployment.
    ///
    /// `FsStore` answers `false`, and the host refuses to boot on it
    /// outside dev mode with the reason attached — the shape
    /// `LocalKeyProvider::from_env` already uses to refuse a default
    /// master key. A test double that quietly becomes a deployment target
    /// is how a filesystem with no durability story ends up holding the
    /// only copy of a customer's documents.
    fn production_ready(&self) -> bool;

    /// Write an object of exactly `len` bytes. **Refuses if the key
    /// exists** — see [`BlobError::AlreadyExists`].
    ///
    /// # Why the length is declared and not discovered
    ///
    /// Three reasons, and the third is the one that decides it.
    ///
    /// 1. S3 wants `Content-Length`. The alternative is HTTP chunked
    ///    transfer, which S3 accepts only under the streaming-signature
    ///    protocol — a second signing scheme, for no gain.
    /// 2. The policy cap has to refuse **before** the bytes arrive, not
    ///    after 100 MB have been read to discover they were 100 MB.
    /// 3. A stream that disagrees with its declared length is a truncated
    ///    upload, and truncation is exactly what a store must not turn
    ///    into a successful `put`. Declaring the length gives every
    ///    binding the same one-line check; discovering it gives every
    ///    binding the same silent corruption.
    async fn put(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        len: u64,
        body: ByteStream,
    ) -> Result<ObjectHead, BlobError>;

    /// Read an object, or a range of one.
    async fn get(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        range: Option<ByteRange>,
    ) -> Result<ByteStream, BlobError>;

    async fn head(&self, space: &SpaceId, handle: &BlobHandle) -> Result<ObjectHead, BlobError>;

    /// Remove an object. **Idempotent**: deleting an absent key succeeds.
    ///
    /// That is not politeness, it is what makes phase 2 of the sweep
    /// crash-safe. The tombstone is stamped *after* the delete, so a crash
    /// between the two leaves the sweep to retry a delete that has already
    /// happened, and a store that refused would turn a safe retry into a
    /// stuck tombstone.
    async fn delete(&self, space: &SpaceId, handle: &BlobHandle) -> Result<(), BlobError>;
}

/// Which store a space's bytes are in.
///
/// The shape `definition-language.md` §2 names — *"per-space BYO bucket via
/// the same resolver pattern as BYO-DB"* — and the shape `KeyProvider` +
/// `WrappedKeyStore` already set. `resolve` returns an `Arc` rather than a
/// reference because a BYO resolver will hold a cache keyed by space and
/// hand out clones, and a signature that returned `&dyn` would force that
/// cache to be a leak.
///
/// # Why this is async
///
/// It was synchronous until `plan-tenant-storage.md` §11 U5, and it could
/// be for exactly as long as the only implementation was
/// [`SingleStore`], which answers out of a field. An attached resolver
/// answers out of the **control database**: the row that says which
/// bucket a space is on is `$spacebucket`, and reading it is a query. A
/// synchronous signature leaves three ways to do that and all three are
/// worse than this one:
///
/// 1. **Block on the read inside `resolve`.** A `block_on` inside a
///    synchronous method called from an async task deadlocks a
///    single-threaded runtime and stalls a worker thread on a
///    multi-threaded one. The failure is a hang, which is the one failure
///    mode a storage tier must not add.
/// 2. **Snapshot the whole table periodically.** Then a space attached
///    since the last snapshot resolves to the *deployment's* store and
///    serves its bytes out of the wrong bucket, silently, until the
///    refresh. That is the precise failure the attachment exists to rule
///    out, so a design that reintroduces it on a timer is not a design.
/// 3. **Resolve lazily behind a `BlobStore` that defers.** A store whose
///    every method first resolves would keep this signature and move the
///    failure from resolution to the first `put`, collapsing "this
///    space's bucket is unreachable" and "that object is not there" into
///    one site. `pipeline::BlobPipeline::store_for`'s own doc argues they
///    are different incidents.
///
/// So the read is awaited where it happens, which is also what
/// `ordo_exec::route::PoolResolver::resolve` already does for the
/// database side of the same question. Two resolvers answering "which
/// target is this space's" with two different shapes would be two shapes
/// for one idea.
///
/// # Where a space writes, and where an object is
///
/// Two questions, and since 2026-10-05 they have different answers
/// (`detach-upload-race` ask 2). [`StoreResolver::resolve`] and
/// [`StoreResolver::current`] answer *where this space's uploads go now*.
/// [`StoreResolver::named`] answers *where one object is*, from the name
/// recorded on its `$blobref` row when it was written. A tenant's bucket
/// can be detached or replaced while an upload is in flight, and a node
/// may keep routing to the old bucket for its cache interval; the upload
/// lands there and its row says so. Every read and every delete of an
/// existing object follows the recorded name, so nothing is checked or
/// refused at upload and nothing is stranded.
///
/// A resolver with one store per space names nothing: [`current`]'s
/// default records the empty name and [`named`]'s default answers the
/// space's store. A resolver whose answer for a space can change, which is
/// `ordo_exec::bucket::AttachedStores`, overrides both.
///
/// [`current`]: StoreResolver::current
/// [`named`]: StoreResolver::named
#[async_trait::async_trait]
pub trait StoreResolver: Send + Sync {
    async fn resolve(&self, space: &SpaceId) -> Result<Arc<dyn BlobStore>, BlobError>;

    /// The store this space's uploads go to now, and the name to record on
    /// each object written there. One call, so the name recorded is the
    /// name of the store the bytes actually reached, whatever a concurrent
    /// detach did in between.
    async fn current(&self, space: &SpaceId) -> Result<(Arc<dyn BlobStore>, String), BlobError> {
        Ok((self.resolve(space).await?, String::new()))
    }

    /// The store a recorded name addresses. The empty name is the
    /// deployment's own store.
    async fn named(&self, space: &SpaceId, name: &str) -> Result<Arc<dyn BlobStore>, BlobError> {
        let _ = name;
        self.resolve(space).await
    }
}

/// One store for every space — the default, and honest about the absence
/// it stands in for. See the crate doc.
pub struct SingleStore(Arc<dyn BlobStore>);

impl SingleStore {
    pub fn new(store: Arc<dyn BlobStore>) -> Self {
        SingleStore(store)
    }
}

#[async_trait::async_trait]
impl StoreResolver for SingleStore {
    /// Awaits nothing and never fails. The `async` is the trait's, for the
    /// reason stated there; this implementation is still a field read, and
    /// a deployment on `single` pays no round trip to find its store.
    async fn resolve(&self, _space: &SpaceId) -> Result<Arc<dyn BlobStore>, BlobError> {
        Ok(Arc::clone(&self.0))
    }
}

/// The key prefix a space's objects live under.
///
/// Refuses rather than sanitises. A sanitiser turns a hostile space id
/// into a *different* valid key, which means two space ids can collapse
/// onto one prefix — and a prefix collision between tenants is the exact
/// cross-tenant read the composite primary key and the keyed handle stream
/// both exist to prevent. Refusing keeps the mapping injective.
///
/// The charset is deliberately narrower than either backend needs: no
/// slash (which would let a space id claim another's prefix, or climb out
/// of the bucket root on a filesystem), no dot-dot, no whitespace, no
/// percent, nothing that an S3 key, a URL path segment and a POSIX path
/// component do not all agree about.
pub fn space_prefix(space: &SpaceId) -> Result<&str, BlobError> {
    let s = space.0.as_str();
    let ok = !s.is_empty()
        && s.len() <= 128
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(s)
    } else {
        Err(BlobError::Config(
            "a space id that addresses objects must be 1..=128 bytes of \
             [A-Za-z0-9_-]; it is refused rather than sanitised, because two \
             sanitised ids can collapse onto one bucket prefix"
                .into(),
        ))
    }
}

/// The full key: `<space>/<hex handle>`.
pub fn object_key(space: &SpaceId, handle: &BlobHandle) -> Result<String, BlobError> {
    Ok(format!(
        "{}/{}",
        space_prefix(space)?,
        ordo_crypto::hex(handle)
    ))
}

/// Collect a stream into one buffer. Test and small-object convenience;
/// the serving path never uses it, which is what "streaming" has to mean
/// to be worth the trait.
pub async fn collect(mut s: ByteStream) -> Result<Vec<u8>, BlobError> {
    use futures::StreamExt;
    let mut out = Vec::new();
    while let Some(chunk) = s.next().await {
        out.extend_from_slice(&chunk?);
    }
    Ok(out)
}

/// One buffer as a stream. Same caveat.
pub fn once(bytes: Vec<u8>) -> ByteStream {
    Box::pin(futures::stream::once(async move { Ok(bytes) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_space_id_cannot_climb_out_of_its_prefix() {
        for hostile in [
            "../other",
            "a/b",
            "",
            ".",
            "..",
            "a b",
            "a%2fb",
            "a\\b",
            "\u{202e}gpj.exe",
        ] {
            assert!(
                space_prefix(&SpaceId(hostile.into())).is_err(),
                "accepted {hostile:?}"
            );
        }
        assert_eq!(
            space_prefix(&SpaceId("tenant-a_1".into())).unwrap(),
            "tenant-a_1"
        );
    }

    // The object key carries the space and the handle and nothing else —
    // promoted into `contract::object_key_is_space_and_handle_only`
    // (2026-09-21, U0) so every binding's own conformance run asserts it
    // for itself, rather than one binding-agnostic unit test standing for
    // all of them.

    #[test]
    fn a_range_knows_its_last_byte() {
        assert_eq!(ByteRange::new(0, Some(10)).last(), Some(9));
        assert_eq!(ByteRange::new(5, Some(1)).last(), Some(5));
        assert_eq!(ByteRange::new(5, None).last(), None);
    }
}

#[cfg(test)]
mod class_tests {
    use super::*;
    use ordo_fault::FaultClass;
    use std::time::Duration;

    fn st(status: u16, ra: Option<u64>) -> BlobError {
        BlobError::status(status, ra.map(Duration::from_secs), "x".into())
    }

    #[test]
    fn a_status_is_classified_once_from_its_number() {
        assert_eq!(st(503, None).class(), FaultClass::Busy);
        assert_eq!(st(429, None).class(), FaultClass::Busy);
        assert_eq!(st(500, Some(3)).class(), FaultClass::Busy);
        assert_eq!(st(500, None).class(), FaultClass::Retryable);
        assert_eq!(st(408, None).class(), FaultClass::Retryable);
        assert_eq!(st(404, None).class(), FaultClass::Refused);
        assert!(st(404, None).is_not_found());
        assert_eq!(st(413, None).class(), FaultClass::Refused);
        assert!(st(413, None).is_bad_input());
        assert!(st(403, None).is_forbidden());
        assert_eq!(st(403, None).class(), FaultClass::Broken);
        assert_eq!(st(400, None).class(), FaultClass::Broken);
        assert_eq!(st(503, Some(2)).retry_after(), Some(Duration::from_secs(2)));
    }

    #[test]
    fn variants_without_a_status_have_a_class() {
        assert_eq!(BlobError::NotFound.class(), FaultClass::Refused);
        assert_eq!(BlobError::Range.class(), FaultClass::Refused);
        assert_eq!(BlobError::AlreadyExists.class(), FaultClass::Refused);
        assert_eq!(
            BlobError::Transport("x".into()).class(),
            FaultClass::Retryable
        );
        assert_eq!(BlobError::Refused("x".into()).class(), FaultClass::Broken);
        assert_eq!(BlobError::Config("x".into()).class(), FaultClass::Broken);
    }

    #[test]
    fn a_caused_error_keeps_its_source_and_reads_the_io_kind() {
        let denied = BlobError::caused_by(
            "filesystem",
            FaultClass::Broken,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert!(denied.is_forbidden());
        assert!(std::error::Error::source(&denied).is_some());
        let net = BlobError::network("connect", std::io::Error::other("reset"));
        assert!(net.is_transient());
        assert_eq!(net, net.clone());
    }
}
