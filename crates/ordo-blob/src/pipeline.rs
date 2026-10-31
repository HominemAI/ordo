//! The one straight path: **bytes → sealer → the space's resolved store**,
//! and back.
//!
//! `definition-language.md` §2, Blob storage pipeline [settled], is one
//! sentence and this module is it. Everything above it (a route, an access
//! check, a row) and everything below it (a bucket, a filesystem) is
//! somebody else's; what lives here is the part that must not be written
//! twice, because writing it twice is how one path ends up sealing and the
//! other does not.
//!
//! # Bounded memory is the requirement, not a goal
//!
//! Nothing here ever holds an object. A 100 MB upload moves through in
//! 64 KiB steps, and the assertion that this is true
//! (`a_hundred_megabyte_upload_never_holds_the_object`) counts chunks
//! rather than trusting the shape — the way this regresses is a `collect`
//! slipping in during a refactor, and a `collect` looks completely
//! innocent.
//!
//! It regressed exactly that way on the one path whose job is to read
//! whole objects: [`BlobPipeline::verify`] collected an object and then
//! hashed it. It opens and hashes chunk by chunk now, and
//! `verifying_a_hundred_megabyte_object_never_holds_it` measures the gap
//! between what the store hands over and what reaches `open_chunk`, which
//! is what this code is holding at that instant. Asserting that a large
//! object merely succeeds would pin nothing, because a buffering version
//! succeeds too.
//!
//! # What is sealed, and when
//!
//! **Object storage always holds ciphertext** for sealed content (DL §2),
//! and the platform seals *before* the bytes reach the store. So an upload
//! to a space-sealed blob proxies through this pipeline; a presigned
//! direct-to-bucket PUT would let a client put unsealed bytes under a
//! sealed field's key, which is the one thing that rule exists to prevent.
//! Presigning stays `blob private`'s (`f7-vault-plan.md` Phase 6), where
//! the bytes are already ciphertext when they leave the client.
//!
//! The metadata is sealed too, and each for its own reason:
//!
//! - `mime` — a content type is a strong hint about a document's kind.
//! - `size` — file sizes fingerprint documents. A plaintext size beside a
//!   sealed blob answers "is this the standard NDA?" without a key.
//! - `integrity` — the "sealed integrity hash" DL §2 names. **Sealed is
//!   the operative word.** The digest lets the platform verify a fetched
//!   object against what it stored; a *plaintext* digest column would be
//!   the content hash by another name, complete with the cross-tenant
//!   equality it was rejected for, and it would survive crypto-erasure.
//!
//! `bytes` — the stored ciphertext length — stays plaintext, because a
//! range read needs it and a bucket listing already discloses it.
//!
//! # What this module does not decide
//!
//! It does not choose the tier. `blob inline` never reaches here: its
//! bytes are a `$sealedpayload` row ([`ordo_crypto::inline`]), which is
//! what makes it GC-free by inheritance rather than by assertion. The
//! caller checks [`BlobPolicy::inline_cap`] **before the sealer** and
//! routes accordingly, so an oversize inline write is refused before a
//! byte is sealed.

use std::sync::Arc;

use futures::StreamExt;
use ordo_crypto::custody::{
    Anchor, BlobHandle, Epoch, KeyError, KeyLease, KeyProvider, Purpose, SpaceId,
};
use ordo_crypto::stream::{self, StreamHeader, CHUNK, SEALED_CHUNK};
use sha2::{Digest, Sha256};

use crate::{BlobError, BlobPolicy, BlobStore, ByteRange, ByteStream, StoreResolver};

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PipelineError {
    /// The store said no. Carries the store's own answer.
    Store(BlobError),
    /// Custody said no — no lease, an unknown space, a destroyed epoch, or
    /// an envelope that did not open. Deliberately not distinguished
    /// further here; `KeyError` is uninformative about *why* on purpose.
    Custody(KeyError),
    /// The object is larger than `BlobPolicy::max_object`.
    ///
    /// One class for every "too large", and it maps onto the door's
    /// existing `bound_violation`. The standing rule is that a new refusal
    /// class is an oracle until argued, and "too large" is a bound, which
    /// the door already has a word for.
    TooLarge { limit: u64, declared: u64 },
    /// The body did not deliver the length it declared.
    Truncated { declared: u64, got: u64 },
    /// The requested range is not inside the object.
    Range,
    /// The upload's intent could not be recorded, so no byte was written.
    ///
    /// Carries the recorder's typed fault: its class (a full pool is
    /// busy, answered `server_busy`), and its own text, which is an
    /// operator's and never a caller's. The door flattens it onto the same `internal` answer it
    /// gives for a custody or a store failure, for [`PipelineError::Store`]'s
    /// reason.
    ///
    /// **It is a refusal rather than a warning**, and that is the whole of
    /// [`UploadIntent`]'s value. Storing the object anyway when the intent
    /// could not be written would produce exactly the orphan the intent
    /// exists to prevent, at the moment the deployment is least able to
    /// notice. A refused upload costs the caller a retry; a stored one
    /// costs bytes nothing can ever enumerate.
    Undeclared(ordo_fault::StoreFault),
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::Store(e) => write!(f, "{e}"),
            PipelineError::Custody(e) => write!(f, "{e}"),
            PipelineError::TooLarge { limit, declared } => write!(
                f,
                "an upload of {declared} bytes exceeds the {limit}-byte object cap"
            ),
            PipelineError::Truncated { declared, got } => write!(
                f,
                "the upload declared {declared} bytes and delivered {got}"
            ),
            PipelineError::Range => f.write_str("that range is not inside the object"),
            PipelineError::Undeclared(why) => write!(
                f,
                "the upload's intent could not be recorded, so nothing was stored: {why}"
            ),
        }
    }
}

impl std::error::Error for PipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PipelineError::Store(e) => Some(e),
            PipelineError::Undeclared(e) => Some(e),
            PipelineError::Custody(e) => Some(e),
            _ => None,
        }
    }
}

impl PipelineError {
    /// What a caller can do about it, read from the type: the store's class,
    /// custody's (busy or broken), the recorder's, and `Refused` for the
    /// request's own faults (too large, truncated, a range outside).
    pub fn class(&self) -> ordo_fault::FaultClass {
        use ordo_fault::FaultClass;
        match self {
            PipelineError::Store(e) => e.class(),
            PipelineError::Custody(e) if e.is_busy() => FaultClass::Busy,
            PipelineError::Custody(_) => FaultClass::Broken,
            PipelineError::Undeclared(e) => e.class(),
            PipelineError::TooLarge { .. }
            | PipelineError::Truncated { .. }
            | PipelineError::Range => FaultClass::Refused,
        }
    }

    /// The upload was larger than the object cap.
    pub fn is_too_large(&self) -> bool {
        matches!(self, PipelineError::TooLarge { .. })
    }
}

impl From<BlobError> for PipelineError {
    fn from(e: BlobError) -> Self {
        PipelineError::Store(e)
    }
}
impl From<KeyError> for PipelineError {
    fn from(e: KeyError) -> Self {
        PipelineError::Custody(e)
    }
}

/// Told the handle at the one instant that closes the upload's crash
/// window: **after the handle is minted and before a byte reaches the
/// store.**
///
/// # Why the pipeline calls out rather than the door calling twice
///
/// The handle is minted inside [`BlobPipeline::put`], from a lease this
/// crate holds and an epoch it read, and it has to stay there: it is what
/// makes a [`Stored`] evidence that these bytes were sealed under this
/// space's key rather than a struct any caller could assemble. So the door
/// cannot know the handle before the `put` and cannot write anything about
/// it before the `put` either. Handing the pipeline somewhere to say the
/// handle is the only ordering that puts a record of the object in front of
/// the object, and the ordering is the entire requirement. A record
/// written *after* the `put` leaves the same unreachable object, in a
/// smaller window.
///
/// # Why a trait, and why it is not a database handle
///
/// `ordo-blob` owns the bytes tier: an HTTP client, a TLS stack, a SigV4
/// signer, and deliberately no Postgres driver, which is why
/// `ordo_crypto::blobref` is a separate crate at all. A trait keeps this
/// crate unable to name a pool while letting `ordo-host`, which holds one,
/// record the intent. It is the same split [`crate::BlobStore`] draws in
/// the other direction.
///
/// The one implementor writes a `$blobtomb` row
/// ([`ordo_crypto::blobref::tomb_declare_sql`]). Nothing here knows that,
/// and nothing here should: a deployment whose record of intent is
/// somewhere else is a deployment this trait already serves.
#[async_trait::async_trait]
pub trait UploadIntent: Send + Sync {
    /// Record that `handle` is about to be written in `space`.
    ///
    /// Returning `Err` **refuses the upload** and no byte is stored, which
    /// is the point: see [`PipelineError::Undeclared`]. The fault's class
    /// says busy or broken; its text is the recorder's own and is treated
    /// as an operator's.
    ///
    /// `store` is the name of the store the bytes are about to go to
    /// ([`crate::StoreResolver::current`]), recorded with the intent so an
    /// interrupted upload is collected from the bucket it landed in.
    async fn declare(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        store: &str,
    ) -> Result<(), ordo_fault::StoreFault>;
}

/// What an upload declares about itself before a byte moves.
pub struct Upload<'a> {
    pub space: &'a SpaceId,
    /// The column the object belongs to. Binds the AAD, so an object
    /// moved between fields refuses to open.
    pub entity: &'a str,
    pub field: &'a str,
    pub mime: &'a str,
    /// The **plaintext** length. Declared, never discovered — see
    /// [`BlobStore::put`] for the three reasons, of which the deciding one
    /// is that a stream disagreeing with its length is a truncated upload
    /// and must not become a successful write.
    pub plain_len: u64,
    /// The request or job this upload belongs to, for the key lease.
    pub correlation: &'a str,
    /// Where the intent to write this object is recorded, before the bytes
    /// are.
    ///
    /// A field on the upload rather than a second `put` entry point, so
    /// every call site states its answer: a second entry point would let a
    /// path that should declare its intent reach the one that does not by
    /// being written the obvious way. `None` means nothing will be able to
    /// find this object if the caller never records the handle, and it is
    /// correct for exactly two kinds of caller: a test with no database
    /// behind it, and a caller that writes its reference row in the same
    /// act as the object. Neither is the door.
    pub intent: Option<&'a dyn UploadIntent>,
}

/// Everything the `$blobref` row needs, and nothing the caller has to
/// recompute.
///
/// **The fields are private and [`BlobPipeline::put`] is the only
/// constructor**, so a `Stored` in hand is evidence that these bytes were
/// sealed under this space's key and written to the store. Before that, any
/// caller could assemble one from values it chose, and the type asserted
/// nothing beyond its own shape. It is the first of the two steps
/// `docs/plan-blob-chain.md` §1.1 draws: bytes become a `Stored` here, and
/// `ordo_host::blob` turns a `Stored` into a token bound to one row.
///
/// Not `Clone`, for the same reason the token above it is not: an upload is
/// one object stored once, and two copies of the evidence would be two
/// converges the store never saw a second set of bytes for.
#[derive(Debug, PartialEq, Eq)]
pub struct Stored {
    handle: BlobHandle,
    epoch: Epoch,
    /// Sealed envelopes, ready for the row. Never plaintext.
    mime: Vec<u8>,
    size: Vec<u8>,
    integrity: Vec<u8>,
    /// The ciphertext length — plaintext, and observable anyway.
    bytes: i64,
    /// The name of the store the bytes were written to.
    store: String,
}

impl Stored {
    /// The name of the store the bytes were written to, for the `$blobref`
    /// row. Reads and the sweep follow it (ruled 2026-10-05,
    /// `detach-upload-race` ask 2). Empty for the deployment's own store.
    pub fn store(&self) -> &str {
        &self.store
    }

    /// The handle the row's column will carry, and the object's name in the
    /// bucket.
    pub fn handle(&self) -> &BlobHandle {
        &self.handle
    }

    /// The key epoch the envelopes below were sealed under. Recorded on the
    /// reference row so a payload written before a rotation keeps naming
    /// the key that opens it.
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The sealed mime envelope.
    pub fn mime(&self) -> &[u8] {
        &self.mime
    }

    /// The sealed plaintext-length envelope.
    pub fn size(&self) -> &[u8] {
        &self.size
    }

    /// The sealed digest envelope: SHA-256 over the plaintext, AAD-bound to
    /// `(entity, field)` by the seal.
    pub fn integrity(&self) -> &[u8] {
        &self.integrity
    }

    /// The ciphertext length.
    pub fn bytes(&self) -> i64 {
        self.bytes
    }
}

/// The pipeline. Holds `Arc`s rather than references because the streams
/// it builds outlive the call that built them.
///
/// # Why a resolver and not a store
///
/// It held one `Arc<dyn BlobStore>` until 2026-09-23, which made the
/// serving path the only part of the blob tier with **no per-space seam at
/// all**: the sweep, the re-seal pass and `erase_space` each already took a
/// [`StoreResolver`] and asked it per space, while every `put` and every
/// `get` through the door went to the one store the pipeline was
/// constructed with. A deployment could therefore attach a bucket to a
/// space and still serve that space's bytes out of the deployment's own,
/// which is the failure mode the whole attachment exists to rule out.
///
/// The field is [`StoreResolver`] rather than `BlobStore` so that there is
/// exactly one place a space's store is chosen, and it is the same trait
/// the other three callers already ask. `Spaces::pool_of` is this shape on
/// the database side: one seam every request-path checkout passes through,
/// so wiring an attachment behind it reaches every caller at once rather
/// than needing a branch at each.
///
/// **The resolution is per call, never cached on the pipeline.** A cache
/// here would be a second answer to "where are this space's bytes",
/// diverging from the resolver's own the moment an attachment changed, and
/// caching is the resolver's business: `SingleStore` has nothing to cache,
/// and an attached resolver caches by `Attachment::identity` where it can
/// see the row that invalidates it.
#[derive(Clone)]
pub struct BlobPipeline {
    /// Which store a space's bytes are in. Never a store: see above.
    pub stores: Arc<dyn StoreResolver>,
    pub keys: Arc<dyn KeyProvider>,
    pub policy: BlobPolicy,
}

/// A resolver pinned to one recorded store name: what
/// [`BlobPipeline::in_store`] reads through. Every question it is asked
/// answers the named store, so the pipeline's own methods follow the
/// record without a branch of their own.
struct Recorded {
    inner: Arc<dyn StoreResolver>,
    name: String,
}

#[async_trait::async_trait]
impl StoreResolver for Recorded {
    async fn resolve(&self, space: &SpaceId) -> Result<Arc<dyn BlobStore>, BlobError> {
        self.inner.named(space, &self.name).await
    }

    async fn current(&self, space: &SpaceId) -> Result<(Arc<dyn BlobStore>, String), BlobError> {
        Ok((
            self.inner.named(space, &self.name).await?,
            self.name.clone(),
        ))
    }

    async fn named(&self, space: &SpaceId, name: &str) -> Result<Arc<dyn BlobStore>, BlobError> {
        self.inner.named(space, name).await
    }
}

impl BlobPipeline {
    pub fn new(
        stores: Arc<dyn StoreResolver>,
        keys: Arc<dyn KeyProvider>,
        policy: BlobPolicy,
    ) -> Self {
        BlobPipeline {
            stores,
            keys,
            policy,
        }
    }

    /// The store this space's bytes are in.
    ///
    /// Every read and write below goes through here, and so does every
    /// caller outside this crate that needs the bytes rather than the
    /// plaintext, which the export does
    /// (`plan-tenant-storage.md` §8 B-E1). It exists so that no caller has
    /// to know the pipeline's field is a resolver, and so that a caller
    /// reaching for a store gets the *space's* one by construction rather
    /// than by remembering to ask.
    ///
    /// A resolver that cannot answer is a store failure and is reported as
    /// one: the space's bucket being unreachable and the object being
    /// absent are different incidents ([`PipelineError::Store`] carries the
    /// resolver's own words for the operator), and neither reaches the
    /// caller as anything but `internal`.
    pub async fn store_for(&self, space: &SpaceId) -> Result<Arc<dyn BlobStore>, BlobError> {
        self.stores.resolve(space).await
    }

    /// This pipeline, reading and writing the store a recorded name
    /// addresses rather than the store the space's uploads go to now.
    ///
    /// **Every read of an existing object goes through this**, with the
    /// name on its `$blobref` row (ruled 2026-10-05, `detach-upload-race`
    /// ask 2). An object written while the tenant's bucket was being
    /// detached or replaced is in the old bucket, its row says so, and a
    /// read that asked the space's current route would look in the wrong
    /// place. The keys and the policy are the same; only the store moves.
    pub fn in_store(&self, name: &str) -> BlobPipeline {
        BlobPipeline {
            stores: Arc::new(Recorded {
                inner: Arc::clone(&self.stores),
                name: name.to_string(),
            }),
            keys: Arc::clone(&self.keys),
            policy: self.policy,
        }
    }

    /// Seal an object and store it, in bounded memory.
    ///
    /// The order is deliberate and each step is refusable before the next:
    /// the cap first (so an oversize upload costs no key release), then
    /// the lease, then the handle, **then the intent**, then the bytes.
    /// Nothing is written to the database here. The caller does that, in
    /// the transaction that also sets the row's column, because a
    /// `$blobref` row and a column that disagree is worse than either
    /// alone.
    ///
    /// The intent sits between the handle and the bytes because that is the
    /// only gap where it closes anything. Before the handle there is
    /// nothing to record; after the `put` the object already exists
    /// unrecorded, which is the leak. See [`UploadIntent`].
    pub async fn put(&self, up: &Upload<'_>, body: ByteStream) -> Result<Stored, PipelineError> {
        // The cap, BEFORE the sealer and before custody is touched. A
        // refused upload must cost nothing and leave nothing.
        if !self.policy.admits_object(up.plain_len) {
            return Err(PipelineError::TooLarge {
                limit: self.policy.max_object,
                declared: up.plain_len,
            });
        }

        // The space's store, resolved once and used for both the `put` and
        // the cleanup delete below, so a half-written object cannot be
        // removed from a different store than the one it landed in.
        //
        // It sits here, after the cap and before the key release, for the
        // cap's own reason: an upload to a space whose store cannot be
        // resolved is refused before it costs a key release, a minted
        // handle or a tombstone. Under `SingleStore` this cannot fail, so
        // the placement is invisible to a deployment that has attached
        // nothing; it is the attached case that would otherwise pay for a
        // refusal it was always going to get.
        //
        // The store's **name** comes back in the same call and rides the
        // intent and the `Stored`, so the row records the bucket the bytes
        // reached rather than whatever the space resolves to by the time
        // the row commits (ruled 2026-10-05, `detach-upload-race` ask 2).
        let (store, store_name) = self.stores.current(up.space).await?;

        let lease = self
            .keys
            .release(up.space, Purpose::Write, up.correlation)?;
        let epoch = self.keys.current_epoch(up.space)?;
        // Keyed-random, per upload, from a stream keyed by (space, epoch).
        // Never a function of the bytes — see `mint_blob_ref`.
        let handle = self.keys.mint_blob_ref(&lease, epoch)?;

        // **The record of the object, before the object.** From here on an
        // interruption of any kind, whether this process dying, the
        // transaction the caller is about to open rolling back, or the body
        // ending short, leaves something that names the handle. That is the
        // whole of what `BlobStore`'s missing `list` costs and the whole of
        // what this buys back.
        //
        // A refusal here stores nothing, deliberately: see
        // `PipelineError::Undeclared`.
        if let Some(intent) = up.intent {
            intent
                .declare(up.space, &handle, &store_name)
                .await
                .map_err(PipelineError::Undeclared)?;
        }

        let header = StreamHeader::new(epoch);

        // The digest is computed as the bytes go past, so nothing is read
        // twice and nothing is buffered to hash it.
        let digest = Arc::new(std::sync::Mutex::new(Sha256::new()));
        let delivered = Arc::new(std::sync::atomic::AtomicU64::new(0));

        // Everything sealed for this object is bound to its handle: the
        // chunks and the three attributes on its reference row. Two objects
        // of one column then cannot be swapped in the store, because the
        // chunks of one do not open under the handle of the other.
        let object = ordo_crypto::hex(&handle);
        let sealed = self.sealing_stream(
            &lease,
            up,
            &object,
            epoch,
            header,
            body,
            Arc::clone(&digest),
            Arc::clone(&delivered),
        );

        let stored_len = stream::sealed_len(up.plain_len);
        let head = store.put(up.space, &handle, stored_len, sealed).await?;

        // The store checked the *ciphertext* length. This checks the
        // plaintext one, which is the number the caller declared and the
        // number a short body would have failed. Both are needed: a body
        // that ended early produces too few sealed bytes AND too few
        // plaintext bytes, and only the second names what actually went
        // wrong.
        let got = delivered.load(std::sync::atomic::Ordering::SeqCst);
        if got != up.plain_len {
            // Best-effort: the object should not survive a failed upload.
            // A leaked object costs money; a half-object that satisfies a
            // `head` and fails its tag costs an incident.
            //
            // Best-effort is now good enough rather than merely all there
            // is. The declared intent outlives this delete whether it
            // worked or not, so a half-object this line failed to remove is
            // one the sweep retires on the floor, and one it did remove
            // leaves a tombstone the sweep closes against a `delete` the
            // store contract makes idempotent.
            let _ = store.delete(up.space, &handle).await;
            return Err(PipelineError::Truncated {
                declared: up.plain_len,
                got,
            });
        }

        let hash = digest.lock().expect("digest").clone().finalize().to_vec();

        Ok(Stored {
            handle,
            epoch,
            mime: self.keys.seal_at(
                &lease,
                up.entity,
                up.field,
                Anchor::Object(&object),
                up.mime.as_bytes(),
            )?,
            size: self.keys.seal_at(
                &lease,
                up.entity,
                up.field,
                Anchor::Object(&object),
                up.plain_len.to_string().as_bytes(),
            )?,
            integrity: self.keys.seal_at(
                &lease,
                up.entity,
                up.field,
                Anchor::Object(&object),
                &hash,
            )?,
            bytes: head.bytes as i64,
            store: store_name,
        })
    }

    /// The adapter that turns a plaintext stream into a sealed one.
    ///
    /// `last` is known without lookahead because the plaintext length was
    /// declared: the final chunk is `chunk_count(plain_len) - 1`. That is
    /// the second thing the declared length buys, and it is why a
    /// discover-as-you-go signature would have needed a one-chunk
    /// lookahead buffer and a special case for the empty object.
    #[allow(clippy::too_many_arguments)]
    fn sealing_stream(
        &self,
        lease: &KeyLease,
        up: &Upload<'_>,
        object: &str,
        epoch: Epoch,
        header: StreamHeader,
        body: ByteStream,
        digest: Arc<std::sync::Mutex<Sha256>>,
        delivered: Arc<std::sync::atomic::AtomicU64>,
    ) -> ByteStream {
        struct S {
            body: ByteStream,
            buf: Vec<u8>,
            counter: u32,
            chunks: u64,
            emitted_header: bool,
            done: bool,
            body_done: bool,
        }
        let state = S {
            body,
            buf: Vec::with_capacity(CHUNK),
            counter: 0,
            chunks: stream::chunk_count(up.plain_len),
            emitted_header: false,
            done: false,
            body_done: false,
        };
        let keys = Arc::clone(&self.keys);
        let lease = lease.clone();
        let entity = up.entity.to_string();
        let field = up.field.to_string();
        let object = object.to_string();
        let base = header.base;
        let header_bytes = header.encode().to_vec();

        Box::pin(futures::stream::unfold(state, move |mut s| {
            let keys = Arc::clone(&keys);
            let lease = lease.clone();
            let entity = entity.clone();
            let field = field.clone();
            let object = object.clone();
            let digest = Arc::clone(&digest);
            let delivered = Arc::clone(&delivered);
            let header_bytes = header_bytes.clone();
            async move {
                if s.done {
                    return None;
                }
                if !s.emitted_header {
                    s.emitted_header = true;
                    return Some((Ok(header_bytes), s));
                }
                // Fill one chunk, or run out of body.
                while s.buf.len() < CHUNK && !s.body_done {
                    match s.body.next().await {
                        None => s.body_done = true,
                        Some(Err(e)) => {
                            s.done = true;
                            return Some((Err(e), s));
                        }
                        Some(Ok(part)) => {
                            delivered
                                .fetch_add(part.len() as u64, std::sync::atomic::Ordering::SeqCst);
                            digest.lock().expect("digest").update(&part);
                            s.buf.extend_from_slice(&part);
                        }
                    }
                }
                if s.buf.is_empty() && s.counter as u64 >= s.chunks {
                    return None;
                }
                let take = CHUNK.min(s.buf.len());
                let plain: Vec<u8> = s.buf.drain(..take).collect();
                let last = s.counter as u64 + 1 == s.chunks;
                let out = keys.seal_chunk(
                    &lease,
                    &entity,
                    &field,
                    Anchor::Object(&object),
                    epoch,
                    &base,
                    s.counter,
                    last,
                    &plain,
                );
                s.counter += 1;
                if last {
                    s.done = true;
                }
                match out {
                    Ok(ct) => Some((Ok(ct), s)),
                    Err(e) => {
                        s.done = true;
                        Some((Err(BlobError::Refused(e.to_string())), s))
                    }
                }
            }
        }))
    }

    /// Read a plaintext range out of a stored object.
    ///
    /// `from`/`len` are **plaintext** coordinates, which is the whole
    /// point: a caller asking for the fiftieth megabyte of a video should
    /// not have to know the chunk size or the tag overhead, and the
    /// arithmetic that translates them lives in one tested function
    /// ([`ordo_crypto::stream::plan_range`]) rather than at every call
    /// site.
    ///
    /// `None` for `len` means "to the end". A range that starts past the
    /// end is [`PipelineError::Range`], never an empty success — an empty
    /// success is how a truncated download reads as a complete one.
    #[allow(clippy::too_many_arguments)]
    pub async fn get(
        &self,
        space: &SpaceId,
        entity: &str,
        field: &str,
        handle: &BlobHandle,
        plain_len: u64,
        from: u64,
        len: Option<u64>,
        correlation: &str,
    ) -> Result<Vec<u8>, PipelineError> {
        let plan = stream::plan_range(plain_len, from, len).ok_or(PipelineError::Range)?;
        let lease = self.keys.release(space, Purpose::Read, correlation)?;
        // Resolved once for both fetches below. Two resolutions would be
        // two chances to read the header from one store and the body from
        // another, and an object whose header and body disagree is an AEAD
        // failure reported as corruption.
        let store = self.store_for(space).await?;

        // The header is fetched separately and is 21 bytes: it carries the
        // epoch, and without the epoch there is no key to ask for. Two
        // round trips rather than one, deliberately — folding the header
        // into the range fetch would mean every range read also read the
        // first chunk, which for a range in the middle of a large object
        // is 64 KiB of pointless traffic.
        let head_bytes = crate::collect(
            store
                .get(
                    space,
                    handle,
                    Some(ByteRange::new(0, Some(stream::HEADER as u64))),
                )
                .await?,
        )
        .await?;
        let header = StreamHeader::decode(&head_bytes)
            .map_err(|_| PipelineError::Custody(KeyError::Seal))?;

        let fetched = crate::collect(
            store
                .get(
                    space,
                    handle,
                    Some(ByteRange::new(plan.stored_offset, Some(plan.stored_len))),
                )
                .await?,
        )
        .await?;

        let object = ordo_crypto::hex(handle);
        let chunks = stream::chunk_count(plain_len);
        let mut out = Vec::with_capacity(fetched.len());
        let mut at = 0usize;
        let mut counter = plan.first_chunk;
        while at < fetched.len() {
            let take = SEALED_CHUNK.min(fetched.len() - at);
            let last = counter as u64 + 1 == chunks;
            out.extend_from_slice(&self.keys.open_chunk(
                &lease,
                entity,
                field,
                Anchor::Object(&object),
                header.epoch,
                &header.base,
                counter,
                last,
                &fetched[at..at + take],
            )?);
            at += take;
            counter += 1;
        }
        let start = plan.discard as usize;
        let end = start + plan.keep as usize;
        if end > out.len() {
            // The object is shorter than the reference row says. That is a
            // real divergence between the store and the database, not a
            // range mistake, and it must not silently return a prefix.
            return Err(PipelineError::Store(BlobError::Refused(
                "the stored object is shorter than its recorded length".into(),
            )));
        }
        Ok(out[start..end].to_vec())
    }

    /// Verify a stored object against the sealed integrity digest on its
    /// reference row.
    ///
    /// This is what the digest is *for*, and it is why the digest is
    /// sealed rather than plaintext: the platform can check an object it
    /// fetched, and nobody holding the database and no keys can use the
    /// column to compare two objects.
    #[allow(clippy::too_many_arguments)]
    pub async fn verify(
        &self,
        space: &SpaceId,
        entity: &str,
        field: &str,
        handle: &BlobHandle,
        plain_len: u64,
        sealed_integrity: &[u8],
        correlation: &str,
    ) -> Result<bool, PipelineError> {
        let lease = self.keys.release(space, Purpose::Read, correlation)?;
        let object = ordo_crypto::hex(handle);
        let want = self.keys.open_at(
            &lease,
            entity,
            field,
            Anchor::Object(&object),
            sealed_integrity,
        )?;
        self.hashes_to(&lease, space, entity, field, handle, plain_len, &want)
            .await
    }

    /// The same comparison, against a digest the caller has **already
    /// opened**.
    ///
    /// A revert reaches the 32 plaintext bytes through the command log
    /// rather than through `$blobref`: the image carries
    /// `"{field}$digest"` as a `SealedRef` into the payload store, and the
    /// pass that resolves every sealed key in an image has opened it
    /// before this is called (`docs/plan-blob-chain.md` §4.3 step 1). Two
    /// entry points and one comparison, so the chain's recorded digest and
    /// the reference row's own are checked by the same code rather than by
    /// two that could drift.
    #[allow(clippy::too_many_arguments)]
    pub async fn verify_digest(
        &self,
        space: &SpaceId,
        entity: &str,
        field: &str,
        handle: &BlobHandle,
        plain_len: u64,
        want: &[u8],
        correlation: &str,
    ) -> Result<bool, PipelineError> {
        let lease = self.keys.release(space, Purpose::Read, correlation)?;
        self.hashes_to(&lease, space, entity, field, handle, plain_len, want)
            .await
    }

    /// Fetch, open and hash, **holding one chunk at a time**.
    ///
    /// # The length fast-fail
    ///
    /// `head` before `get`: the store reports the object's stored length
    /// without moving a byte of it, and [`stream::sealed_len`] turns the
    /// caller's plaintext length into the number that length must be. A
    /// disagreement refuses at zero transfer cost.
    ///
    /// An agreement proves nothing, because every tampering that preserves
    /// length survives it, so the hash still runs. Cheap to fail and full price
    /// to pass is the asymmetry that makes the check worth having, and it
    /// is why a length match must never short-circuit the comparison
    /// below.
    ///
    /// # Why it streams
    ///
    /// The crate's promise is that nothing here ever holds an object
    /// (`a_hundred_megabyte_upload_never_holds_the_object`), and reading a
    /// whole object to hash it broke that promise on the one path whose
    /// job is to read whole objects. Chunks are opened as they arrive and
    /// dropped as they are hashed, so the live buffer is one sealed chunk
    /// plus one plaintext chunk whatever the object's size.
    ///
    /// The framing is exact rather than greedy: every chunk's sealed
    /// length is a function of the declared plaintext length and the
    /// chunk's index, so a short delivery is caught by name rather than by
    /// an AEAD tag failing on a boundary that moved.
    #[allow(clippy::too_many_arguments)]
    async fn hashes_to(
        &self,
        lease: &KeyLease,
        space: &SpaceId,
        entity: &str,
        field: &str,
        handle: &BlobHandle,
        plain_len: u64,
        want: &[u8],
    ) -> Result<bool, PipelineError> {
        // One resolution for the `head` and the body both: the length this
        // check believes and the bytes it hashes have to come from the same
        // store, or the mismatch below reports a divergence that is really
        // two different objects.
        let store = self.store_for(space).await?;
        let head = store.head(space, handle).await?;
        if head.bytes != stream::sealed_len(plain_len) {
            return Ok(false);
        }

        let mut body = store.get(space, handle, None).await?;
        let object = ordo_crypto::hex(handle);
        let chunks = stream::chunk_count(plain_len);
        let mut digest = Sha256::new();
        let mut buf: Vec<u8> = Vec::with_capacity(SEALED_CHUNK + stream::HEADER);
        let mut framing: Option<(Epoch, stream::NonceBase)> = None;
        let mut opened: u64 = 0;

        while let Some(part) = body.next().await {
            buf.extend_from_slice(&part?);
            loop {
                let Some((epoch, base)) = framing else {
                    if buf.len() < stream::HEADER {
                        break;
                    }
                    let h = StreamHeader::decode(&buf[..stream::HEADER])
                        .map_err(|_| PipelineError::Custody(KeyError::Seal))?;
                    buf.drain(..stream::HEADER);
                    framing = Some((h.epoch, h.base));
                    continue;
                };
                if opened == chunks {
                    break;
                }
                let take = sealed_chunk_len(plain_len, opened, chunks);
                if buf.len() < take {
                    break;
                }
                let last = opened + 1 == chunks;
                let plain = self.keys.open_chunk(
                    lease,
                    entity,
                    field,
                    Anchor::Object(&object),
                    epoch,
                    &base,
                    opened as u32,
                    last,
                    &buf[..take],
                )?;
                buf.drain(..take);
                digest.update(&plain);
                opened += 1;
            }
        }

        // `head` said the object was whole, so a stream that ran out early
        // or ran on past the last chunk is the store disagreeing with
        // itself. Neither is a comparison, so neither may answer `false`:
        // a divergence between the store and its own metadata is reported
        // as a store fault, which is the answer that leaves the caller
        // free to say nothing was disproved.
        if opened != chunks || !buf.is_empty() {
            return Err(PipelineError::Store(BlobError::Refused(
                "the stored object does not match the length the store reported for it".into(),
            )));
        }
        Ok(digest.finalize().as_slice() == want)
    }
}

/// The sealed length of chunk `index` of an object whose plaintext is
/// `plain_len` bytes: a full chunk everywhere but the last, and the
/// remainder plus one tag there.
///
/// A zero-byte plaintext is one chunk of nothing but a tag, matching
/// [`stream::chunk_count`]'s own rule, so the empty object needs no
/// special case anywhere above.
fn sealed_chunk_len(plain_len: u64, index: u64, chunks: u64) -> usize {
    if index + 1 < chunks {
        SEALED_CHUNK
    } else {
        (plain_len - index * CHUNK as u64) as usize + stream::TAG
    }
}
