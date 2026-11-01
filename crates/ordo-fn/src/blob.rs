//! Files in (`plan-backend-functions.md` increment 2): `ctx.blob(file)`.
//!
//! A function's `$file: blob` argument is a handle the page got from the
//! blob door. The function reads the bytes through [`FnBlobs`], which the
//! caller hands in already bound to the person calling. So a read is that
//! person's read: the door's `std.$blob.get` check and the row guard both
//! apply, and a file they may not read is [`BlobError::NotFound`], the same
//! answer the blob door gives.
//!
//! The function host adds two checks of its own before the seam is asked:
//! the handle must be one of the call's `blob` arguments, and the bytes read
//! in one call may not pass [`crate::Limits::blob_bytes`].

/// The setting that sets [`crate::Limits::blob_bytes`]. Named here because
/// the refusal names it.
pub const BLOB_BYTES_ENV: &str = "ORDO_FN_BLOB_BYTES";

/// The bytes one call may read through `ctx.blob`, all files together, by
/// default: 20 MiB, the largest file ellanor's `extract-document` takes.
pub const DEFAULT_BLOB_BYTES: u64 = 20 * 1024 * 1024;

/// One file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnBlob {
    /// The media type recorded at upload. The uploader's claim.
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Why a file was not read. The one typed error of `ctx.blob`
/// (AGENTS.md rule 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobError {
    /// No such file, or the caller may not read it. One answer for both,
    /// as at the blob door, so a handle is no oracle.
    NotFound,
    /// The handle is not one of this call's `blob` arguments.
    NotArgument,
    /// The file would take the call past its cap. Refused before a byte
    /// is read.
    TooLarge {
        size: u64,
        /// What the call had left to read.
        left: u64,
        /// The call's cap.
        limit: u64,
    },
    /// This deployment has no file storage.
    NoStore,
    /// File storage failed or is busy. Nothing about the call was wrong.
    Unavailable { cause: String },
    /// The read failed in a way no retry fixes. The cause is kept.
    Failed { cause: String },
}

impl BlobError {
    /// The reason word the function sees as `e.code`, and the run's outcome
    /// class when the function lets it end the call.
    pub fn kind(&self) -> &'static str {
        match self {
            BlobError::NotFound => "blob_not_found",
            BlobError::NotArgument => "blob_not_argument",
            BlobError::TooLarge { .. } => "blob_too_large",
            BlobError::NoStore => "blob_no_store",
            BlobError::Unavailable { .. } => "blob_unavailable",
            BlobError::Failed { .. } => "blob_failed",
        }
    }

    pub fn remedy(&self) -> crate::Remedy {
        match self {
            BlobError::Unavailable { .. } => crate::Remedy::Retry,
            BlobError::NotFound | BlobError::TooLarge { .. } => crate::Remedy::FixInput,
            BlobError::NotArgument | BlobError::NoStore | BlobError::Failed { .. } => {
                crate::Remedy::Refuse
            }
        }
    }
}

impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobError::NotFound => f.write_str(
                "no such file, or the caller may not read it; pass a handle the caller \
                 uploaded or may read",
            ),
            BlobError::NotArgument => f.write_str(
                "ctx.blob reads only the call's own `blob` arguments; pass input.<name> for a \
                 parameter declared `$name: blob`",
            ),
            BlobError::TooLarge { size, left, limit } => write!(
                f,
                "the file is {size} bytes and this call may read {left} more of its {limit}; \
                 send a smaller file, or raise {BLOB_BYTES_ENV}"
            ),
            BlobError::NoStore => f.write_str(
                "this deployment has no file storage; configure a blob store to read files",
            ),
            BlobError::Unavailable { cause } => {
                write!(f, "file storage is unavailable ({cause}); try again")
            }
            BlobError::Failed { cause } => write!(f, "the file could not be read: {cause}"),
        }
    }
}

impl std::error::Error for BlobError {}

/// Reads files for one call, as the person calling. Made per call by the
/// caller of [`crate::FnHost::call`], already bound to that person, their
/// tenant and their delegation, if any.
#[async_trait::async_trait]
pub trait FnBlobs: Send + Sync {
    /// The file `handle` names. A file over `max` bytes is refused with
    /// [`BlobError::TooLarge`] before its bytes are read.
    async fn read(&self, handle: &str, max: u64) -> Result<FnBlob, BlobError>;
}

/// No file storage: every read is [`BlobError::NoStore`].
pub struct NoBlobs;

#[async_trait::async_trait]
impl FnBlobs for NoBlobs {
    async fn read(&self, _handle: &str, _max: u64) -> Result<FnBlob, BlobError> {
        Err(BlobError::NoStore)
    }
}
