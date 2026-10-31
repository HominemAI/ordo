//! `FsStore` — the test double, and **not a deployment target**.
//!
//! # The argument, since two bindings invites "two behaviours"
//!
//! *For.* The language change and the GC design are the parts that must be
//! right, and neither should wait on a TLS stack. CI has no object-storage
//! service and `docker/` has none. `MemoryKeyStore` and `ordo-api`'s "one
//! implementation each today, in-memory" are the established pattern for
//! exactly this.
//!
//! *Against, answered.* A filesystem has no eventual consistency, no 5xx,
//! no key-length limit, and — the one that actually decides it — no
//! durability story anybody has written down. **So it is a test double and
//! not a supported deployment target**: [`BlobStore::production_ready`]
//! answers `false`, and a deployment configuration naming it outside dev
//! mode refuses to boot with the reason, exactly as
//! `LocalKeyProvider::from_env` refuses a default master key.
//!
//! Conformance is held by making the *trait's* contract the test surface:
//! one suite in [`crate::contract`], both bindings, the S3 run gated on a
//! live endpoint the way the Postgres tiers already are.
//!
//! # What it does model faithfully
//!
//! The two behaviours the sweep and the upload path stand on:
//! `put` refuses an existing key (`create_new`, one syscall, no
//! check-then-write window) and `delete` succeeds on an absent one. Those
//! are the two the contract suite asserts of both bindings, because they
//! are the two the platform's correctness rests on rather than its
//! performance.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ordo_crypto::custody::{BlobHandle, SpaceId};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::{object_key, BlobError, BlobStore, ByteRange, ByteStream, ObjectHead};

/// How much a `get` reads per chunk. Small enough that a 100 MB object
/// streams in bounded memory, large enough that the syscall count is not
/// the story.
const CHUNK: usize = 64 * 1024;

pub struct FsStore {
    root: PathBuf,
}

impl FsStore {
    /// Root the store at a directory. It is created if absent; a root that
    /// exists and is not a directory is a configuration refusal, because
    /// discovering it at the first upload would mean discovering it in
    /// production.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, BlobError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)
            .map_err(|e| BlobError::Config(format!("blob root {}: {e}", root.display())))?;
        if !root.is_dir() {
            return Err(BlobError::Config(format!(
                "blob root {} is not a directory",
                root.display()
            )));
        }
        Ok(FsStore { root })
    }

    /// Convenient `Arc<dyn BlobStore>` for a resolver.
    pub fn shared(root: impl AsRef<Path>) -> Result<Arc<dyn BlobStore>, BlobError> {
        Ok(Arc::new(FsStore::new(root)?))
    }

    fn path(&self, space: &SpaceId, handle: &BlobHandle) -> Result<PathBuf, BlobError> {
        // `object_key` has already refused any space id that could climb
        // out of the root, so this join cannot escape. Going through the
        // same function the S3 binding uses is the point: one key scheme,
        // one refusal, two backends that cannot drift.
        Ok(self.root.join(object_key(space, handle)?))
    }
}

fn io(e: std::io::Error) -> BlobError {
    match e.kind() {
        std::io::ErrorKind::NotFound => BlobError::NotFound,
        std::io::ErrorKind::AlreadyExists => BlobError::AlreadyExists,
        // Kept with its cause. A filesystem fault is the deployment's
        // (broken), and not worth another attempt.
        _ => BlobError::caused_by("filesystem", ordo_fault::FaultClass::Broken, e),
    }
}

#[async_trait::async_trait]
impl BlobStore for FsStore {
    fn binding(&self) -> &'static str {
        "filesystem"
    }

    /// Never. See the module doc.
    fn production_ready(&self) -> bool {
        false
    }

    async fn put(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        len: u64,
        mut body: ByteStream,
    ) -> Result<ObjectHead, BlobError> {
        use futures::StreamExt;
        let path = self.path(space, handle)?;
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(io)?;
        }
        // `create_new` — the refusal is one syscall, not a check followed
        // by a write with a window between them. A handle is minted per
        // upload and never reused, so reaching this is a repeated CSPRNG
        // seed inside one space-epoch or a bug; either way the honest
        // answer is a refusal, not an overwrite of somebody's document.
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map_err(io)?;
        let mut bytes = 0u64;
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            bytes += chunk.len() as u64;
            if let Err(e) = f.write_all(&chunk).await {
                // A partial object is worse than none: it would satisfy a
                // `head` and fail an `open`, which reads as corruption
                // rather than as a failed upload.
                let _ = tokio::fs::remove_file(&path).await;
                return Err(io(e));
            }
        }
        if let Err(e) = f.flush().await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(io(e));
        }
        if bytes != len {
            // A truncated upload must not become a successful `put`: the
            // object would satisfy a `head`, fail its AEAD tag, and read
            // as corruption rather than as an upload that did not finish.
            let _ = tokio::fs::remove_file(&path).await;
            return Err(BlobError::Refused(format!(
                "the body declared {len} bytes and carried {bytes}"
            )));
        }
        Ok(ObjectHead { bytes })
    }

    async fn get(
        &self,
        space: &SpaceId,
        handle: &BlobHandle,
        range: Option<ByteRange>,
    ) -> Result<ByteStream, BlobError> {
        let path = self.path(space, handle)?;
        let mut f = tokio::fs::File::open(&path).await.map_err(io)?;
        let total = f.metadata().await.map_err(io)?.len();
        let (offset, remaining) = match range {
            None => (0, total),
            Some(r) => {
                if r.offset >= total && !(r.offset == 0 && total == 0) {
                    return Err(BlobError::Range);
                }
                let avail = total - r.offset;
                (r.offset, r.len.map(|n| n.min(avail)).unwrap_or(avail))
            }
        };
        if offset > 0 {
            f.seek(std::io::SeekFrom::Start(offset)).await.map_err(io)?;
        }
        // `unfold` rather than a hand-written `poll_fn`: the read future
        // has to survive a `Pending`, and a `poll_fn` that constructs one
        // per poll drops it — which is a lost wakeup on a slow disk and,
        // worse, a silently short read. Held here because the same shape
        // is about to be written twice more (the S3 body, the door), and
        // getting it wrong once is enough.
        let s = futures::stream::unfold((f, remaining), move |(mut f, mut left)| async move {
            if left == 0 {
                return None;
            }
            let want = CHUNK.min(left as usize);
            let mut buf = vec![0u8; want];
            match f.read(&mut buf).await {
                Err(e) => Some((Err(io(e)), (f, 0))),
                // Short of the length the metadata promised: the file was
                // truncated under us. Reporting it as end-of-stream would
                // hand a caller a partial object that authenticates as a
                // complete one right up to the AEAD tag.
                Ok(0) => Some((
                    Err(BlobError::Refused(
                        "the object ended before its recorded length".into(),
                    )),
                    (f, 0),
                )),
                Ok(n) => {
                    buf.truncate(n);
                    left -= n as u64;
                    Some((Ok(buf), (f, left)))
                }
            }
        });
        Ok(Box::pin(s) as ByteStream)
    }

    async fn head(&self, space: &SpaceId, handle: &BlobHandle) -> Result<ObjectHead, BlobError> {
        let m = tokio::fs::metadata(self.path(space, handle)?)
            .await
            .map_err(io)?;
        Ok(ObjectHead { bytes: m.len() })
    }

    async fn delete(&self, space: &SpaceId, handle: &BlobHandle) -> Result<(), BlobError> {
        match tokio::fs::remove_file(self.path(space, handle)?).await {
            Ok(()) => Ok(()),
            // Idempotent, and this is load-bearing: phase 2 of the sweep
            // stamps the tombstone *after* the delete, so a crash between
            // the two leaves a retry to delete an object that is already
            // gone.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e)),
        }
    }
}
