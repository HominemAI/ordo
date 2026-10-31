//! `BlobPipeline::verify` reads in bounded memory, and refuses a wrong
//! length before it reads at all.
//!
//! `tests/pipeline.rs` pins the upload side by counting: the body is
//! generated lazily and the store counts the sealed bytes without
//! collecting them, so a `collect` slipping into the sealing stream shows
//! up as a test that exhausts memory rather than as one that passes
//! slowly. The read side had no such natural counter. `verify` is handed a
//! store and asked for one boolean, and a version that buffers the whole
//! object returns exactly the same boolean as one that does not. That is
//! how `verify` came to hold a hundred megabytes with the crate's
//! bounded-memory promise written at the top of the file it lived in. A
//! test that merely passed on a large object would have caught none of it.
//!
//! # What is measured, rather than assumed
//!
//! Two doubles, either side of the pipeline, and one shared tally between
//! them:
//!
//! - [`Watched`] wraps the store and counts every byte it **hands to** the
//!   pipeline.
//! - [`Opener`] wraps custody and counts every byte the pipeline **hands
//!   back** to `open_chunk`.
//!
//! The difference is the object bytes the pipeline is holding at that
//! instant, and the peak of that difference over the whole call is its
//! working set. A streaming read never has more than one sealed chunk in
//! hand; a read that collected the object would take all hundred megabytes
//! from the store before opening a single chunk, so its peak is the whole
//! object. The two shapes are three orders of magnitude apart, which is
//! why the bound below can be both generous and decisive.
//!
//! Neither double changes a byte or an answer: `Watched` delegates to a
//! real `FsStore` and `Opener` to the real `TestProvider`, so the object
//! under test is a real sealed object and the verification is the real
//! one.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use ordo_blob::{
    BlobError, BlobPipeline, BlobPolicy, BlobStore, ByteRange, ByteStream, FsStore, ObjectHead,
    Upload,
};
use ordo_crypto::custody::{
    Anchor, BlindValue, BlobHandle, Epoch, KeyError, KeyLease, KeyProvider, Purpose, SpaceId,
    TestProvider,
};
use ordo_crypto::stream::{NonceBase, SEALED_CHUNK};

/// The tally both doubles write to. `delivered` runs ahead of `opened` by
/// exactly what the pipeline is holding.
#[derive(Default)]
struct Tally {
    delivered: AtomicU64,
    opened: AtomicU64,
    /// The high-water mark of `delivered - opened`, sampled at every
    /// `open_chunk`. Sampling there rather than at every delivery is what
    /// makes the number meaningful: it is the moment the pipeline has the
    /// most in hand.
    peak_held: AtomicU64,
    /// How many times the store was asked for bytes. The length fast-fail
    /// asserts this is zero.
    gets: AtomicUsize,
    heads: AtomicUsize,
}

impl Tally {
    fn delivered(&self, n: u64) {
        self.delivered.fetch_add(n, Ordering::SeqCst);
    }

    fn opened(&self, n: u64) {
        let held = self
            .delivered
            .load(Ordering::SeqCst)
            .saturating_sub(self.opened.fetch_add(n, Ordering::SeqCst) + n);
        self.peak_held.fetch_max(held, Ordering::SeqCst);
    }
}

/// The store, watched. Every method delegates; `get` additionally counts
/// what the stream yields, chunk by chunk, as it yields it.
struct Watched {
    inner: Arc<dyn BlobStore>,
    tally: Arc<Tally>,
    /// When set, `head` reports this instead of the object's real length:
    /// the store and the database disagreeing, which is what the fast-fail
    /// exists to catch cheaply.
    head_says: Option<u64>,
}

#[async_trait::async_trait]
impl BlobStore for Watched {
    fn binding(&self) -> &'static str {
        "watched"
    }

    fn production_ready(&self) -> bool {
        false
    }

    async fn put(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        len: u64,
        body: ByteStream,
    ) -> Result<ObjectHead, BlobError> {
        self.inner.put(space, handle, len, body).await
    }

    async fn get(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        range: Option<ByteRange>,
    ) -> Result<ByteStream, BlobError> {
        self.tally.gets.fetch_add(1, Ordering::SeqCst);
        let inner = self.inner.get(space, handle, range).await?;
        let tally = Arc::clone(&self.tally);
        Ok(Box::pin(futures::stream::unfold(
            (inner, tally),
            |(mut s, tally)| async move {
                use futures::StreamExt;
                let next = s.next().await?;
                if let Ok(bytes) = &next {
                    tally.delivered(bytes.len() as u64);
                }
                Some((next, (s, tally)))
            },
        )))
    }

    async fn head(&self, space: &SpaceId, handle: &BlobHandle) -> Result<ObjectHead, BlobError> {
        self.tally.heads.fetch_add(1, Ordering::SeqCst);
        let real = self.inner.head(space, handle).await?;
        Ok(match self.head_says {
            Some(bytes) => ObjectHead { bytes },
            None => real,
        })
    }

    async fn delete(&self, space: &SpaceId, handle: &BlobHandle) -> Result<(), BlobError> {
        self.inner.delete(space, handle).await
    }
}

/// Custody, watched. Only `open_chunk` is instrumented; every other method
/// is the provider's own answer, unaltered.
struct Opener {
    inner: Arc<TestProvider>,
    tally: Arc<Tally>,
}

impl KeyProvider for Opener {
    fn release(
        &self,
        space: &SpaceId,
        purpose: Purpose,
        correlation: &str,
    ) -> Result<KeyLease, KeyError> {
        self.inner.release(space, purpose, correlation)
    }

    fn blind_index(
        &self,
        space: &SpaceId,
        entity: &str,
        field: &str,
        value: BlindValue<'_>,
    ) -> Result<[u8; 32], KeyError> {
        self.inner.blind_index(space, entity, field, value)
    }

    fn seal_at(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.inner.seal_at(l, e, f, a, v)
    }
    fn open_at(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.inner.open_at(l, e, f, a, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        epoch: Epoch,
        base: &NonceBase,
        counter: u32,
        last: bool,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.inner.seal_chunk(
            lease, entity, field, anchor, epoch, base, counter, last, plaintext,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        epoch: Epoch,
        base: &NonceBase,
        counter: u32,
        last: bool,
        chunk: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.tally.opened(chunk.len() as u64);
        self.inner.open_chunk(
            lease, entity, field, anchor, epoch, base, counter, last, chunk,
        )
    }

    fn current_epoch(&self, space: &SpaceId) -> Result<Epoch, KeyError> {
        self.inner.current_epoch(space)
    }

    fn rotate(&self, space: &SpaceId, reason: &str) -> Result<Epoch, KeyError> {
        self.inner.rotate(space, reason)
    }

    fn destroy_epoch(&self, space: &SpaceId, epoch: Epoch) -> Result<(), KeyError> {
        self.inner.destroy_epoch(space, epoch)
    }

    fn destroy_space(&self, space: &SpaceId) -> Result<(), KeyError> {
        self.inner.destroy_space(space)
    }

    fn create_space(&self, space: &SpaceId) -> Result<(), KeyError> {
        self.inner.create_space(space)
    }
}

struct Rig {
    dir: std::path::PathBuf,
    pipe: BlobPipeline,
    tally: Arc<Tally>,
    space: SpaceId,
}

impl Drop for Rig {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn rig(name: &str, head_says: Option<u64>) -> Rig {
    let dir = std::env::temp_dir().join(format!(
        "ordo-blob-verify-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let tally = Arc::new(Tally::default());
    let store: Arc<dyn BlobStore> = Arc::new(Watched {
        inner: FsStore::shared(&dir).unwrap(),
        tally: Arc::clone(&tally),
        head_says,
    });
    let keys: Arc<dyn KeyProvider> = Arc::new(Opener {
        inner: Arc::new(TestProvider::with_space("tenant-a")),
        tally: Arc::clone(&tally),
    });
    Rig {
        dir,
        pipe: BlobPipeline::new(
            Arc::new(ordo_blob::SingleStore::new(store)),
            keys,
            BlobPolicy::DEFAULT,
        ),
        tally,
        space: SpaceId("tenant-a".into()),
    }
}

fn upload(space: &SpaceId, len: u64) -> Upload<'_> {
    Upload {
        space,
        entity: "documents",
        field: "contract",
        mime: "application/pdf",
        plain_len: len,
        correlation: "req-1",
        intent: None,
    }
}

const MB: usize = 1 << 20;

/// **A 100 MB object is verified without ever being held.**
///
/// The body is generated lazily so the test does not hold it either, and
/// the assertion is on the peak gap between what the store handed over and
/// what the opener had been given back. A `verify` that collected the
/// object would show a peak of the whole hundred megabytes; a streaming
/// one shows a couple of chunks.
#[tokio::test]
async fn verifying_a_hundred_megabyte_object_never_holds_it() {
    let r = rig("big", None);
    const N: u64 = 100;
    let total = MB as u64 * N;

    let body = Box::pin(futures::stream::unfold(0u64, |i| async move {
        if i == N {
            None
        } else {
            Some((Ok(vec![(i % 256) as u8; MB]), i + 1))
        }
    })) as ByteStream;

    let stored = r
        .pipe
        .put(&upload(&r.space, total), body)
        .await
        .expect("put");

    r.tally.delivered.store(0, Ordering::SeqCst);
    r.tally.opened.store(0, Ordering::SeqCst);
    r.tally.peak_held.store(0, Ordering::SeqCst);

    assert!(
        r.pipe
            .verify(
                &r.space,
                "documents",
                "contract",
                stored.handle(),
                total,
                stored.integrity(),
                "req-2",
            )
            .await
            .expect("verify"),
        "the object it just stored must verify"
    );

    // Every byte was read, so this is a whole-object verification and not
    // a check that stopped early.
    assert_eq!(
        r.tally.opened.load(Ordering::SeqCst),
        ordo_crypto::stream::sealed_len(total) - ordo_crypto::stream::HEADER as u64,
        "verification must cover every chunk of the object"
    );

    // Three sealed chunks is roughly twice the working set the streaming
    // shape can need (one chunk being opened, plus the tail of the store
    // read that is filling the next), and one five-hundredth of the
    // object. A `collect` cannot fit under it.
    let peak = r.tally.peak_held.load(Ordering::SeqCst);
    assert!(
        peak <= 3 * SEALED_CHUNK as u64,
        "verification held {peak} bytes of a {total}-byte object at once; the streaming read \
         holds one chunk, so something is buffering"
    );
}

/// **A length the store disagrees with refuses before any transfer.**
///
/// `$blobref` records the sealed plaintext length and the store reports
/// the stored one; [`ordo_crypto::stream::sealed_len`] is the function
/// between them. When they disagree the object cannot be what the chain
/// recorded, and saying so costs one `head` and no bytes.
///
/// The store here is told to misreport its length, which is the same
/// observation as a truncated or replaced object without needing one: the
/// pipeline sees a number that does not match and must stop. `gets` is the
/// assertion that carries the claim: the store was never asked for
/// content, which is a fact about what happened rather than about how long
/// it took.
#[tokio::test]
async fn a_length_the_store_disagrees_with_refuses_before_any_transfer() {
    let plain = vec![7u8; 300_000];
    let stored_len = ordo_crypto::stream::sealed_len(plain.len() as u64);
    // One byte off is the hardest case for a check that is allowed to be
    // approximate. This one is not allowed to be.
    let r = rig("length", Some(stored_len - 1));
    let stored = r
        .pipe
        .put(
            &upload(&r.space, plain.len() as u64),
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put");

    let gets_after_put = r.tally.gets.load(Ordering::SeqCst);

    let ok = r
        .pipe
        .verify(
            &r.space,
            "documents",
            "contract",
            stored.handle(),
            plain.len() as u64,
            stored.integrity(),
            "req-2",
        )
        .await
        .expect("a length disagreement is an answer, not an error");

    assert!(!ok, "a length that disagrees cannot be the recorded object");
    assert_eq!(
        r.tally.gets.load(Ordering::SeqCst),
        gets_after_put,
        "the store was asked for bytes after a length disagreement had already settled it"
    );
    assert_eq!(
        r.tally.opened.load(Ordering::SeqCst),
        0,
        "nothing was opened, so nothing was transferred"
    );
    assert!(
        r.tally.heads.load(Ordering::SeqCst) > 0,
        "the fast-fail has to have asked the store for the length"
    );
}

/// **A length that agrees proves nothing, so the hash still runs.**
///
/// The other half of the asymmetry, and the one a well-meaning
/// optimisation would delete: cheap to fail, full price to pass. Tampering
/// that preserves length is the ordinary case (a byte flipped, a page
/// swapped), so a length match that short-circuited the comparison would
/// pass exactly the objects verification exists to catch.
#[tokio::test]
async fn a_length_that_agrees_does_not_short_circuit_the_hash() {
    let r = rig("agree", None);
    let plain = vec![3u8; 200_000];
    let stored = r
        .pipe
        .put(
            &upload(&r.space, plain.len() as u64),
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put");

    r.tally.opened.store(0, Ordering::SeqCst);
    assert!(r
        .pipe
        .verify(
            &r.space,
            "documents",
            "contract",
            stored.handle(),
            plain.len() as u64,
            stored.integrity(),
            "req-2",
        )
        .await
        .expect("verify"));
    assert_eq!(
        r.tally.opened.load(Ordering::SeqCst),
        ordo_crypto::stream::sealed_len(plain.len() as u64) - ordo_crypto::stream::HEADER as u64,
        "a matching length must not stop the object from being read and hashed"
    );
}

/// The plaintext-digest entry point reaches the same comparison as the
/// envelope one. A revert opens the digest through the command log, so
/// both doors have to answer identically about one object.
#[tokio::test]
async fn an_already_opened_digest_verifies_the_same_object() {
    let r = rig("opened", None);
    let plain = vec![9u8; 5_000];
    let stored = r
        .pipe
        .put(
            &upload(&r.space, plain.len() as u64),
            ordo_blob::once(plain.clone()),
        )
        .await
        .expect("put");

    let want = <sha2::Sha256 as sha2::Digest>::digest(&plain).to_vec();
    assert!(r
        .pipe
        .verify_digest(
            &r.space,
            "documents",
            "contract",
            stored.handle(),
            plain.len() as u64,
            &want,
            "req-2",
        )
        .await
        .expect("verify"));

    let mut wrong = want.clone();
    wrong[0] ^= 1;
    assert!(!r
        .pipe
        .verify_digest(
            &r.space,
            "documents",
            "contract",
            stored.handle(),
            plain.len() as u64,
            &wrong,
            "req-2",
        )
        .await
        .expect("verify"));
}
