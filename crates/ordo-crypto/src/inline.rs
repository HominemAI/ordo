//! `blob inline` — the whole blob inside one sealed payload.
//!
//! `definition-language.md` §2 pin (2): *"the blob lives inside the sealed
//! payload (inherits the space DEK with zero extra machinery), serves only
//! through the API (no presigned path), exports and crypto-erases with the
//! row, and is GC-free (row-owned)."*
//!
//! Every one of those five properties is **inherited, not asserted**, and
//! that is the strongest argument for keeping `inline` a first-class tier
//! rather than treating it as an optimisation of the external path:
//!
//! - *inherits the space DEK* — the payload is sealed by
//!   [`crate::custody::KeyProvider::seal_at`] like any other value, under the
//!   current epoch, AAD-bound to `(entity, field)`.
//! - *no presigned path* — there is no object, so there is nothing to
//!   presign.
//! - *exports and crypto-erases with the row* — the export already reads
//!   sealed columns, and `destroy_space` already makes every payload in
//!   the space permanently unopenable.
//! - *GC-free* — `ordo_exec::sealed::collect_pruned_in` already collects
//!   it, **inside the pruning transaction**, with no tombstone and no
//!   second horizon. `$blobtomb` never learns that inline blobs exist.
//!
//! # Why the metadata rides in the envelope rather than in `$blobref`
//!
//! An external blob's `mime` and `size` live in a `$blobref` row because
//! the bytes live somewhere the database cannot see, so *something* has to
//! remember them on this side. An inline blob has no such split: the bytes
//! are already in the payload store, so putting the mime beside them costs
//! one length-prefixed field and buys the four inherited properties above
//! exactly as written. A `$blobref` row for an inline blob would introduce
//! a second thing to collect — and it would have to be collected on the
//! *other* ordering (reference last, §5.3), which is precisely the
//! machinery DL says this tier does not need.
//!
//! # The encoding
//!
//! ```text
//! [u8 version = 1][u16be mime_len][mime bytes][u64be plain_len][bytes]
//! ```
//!
//! Big-endian and length-prefixed, in the house style of
//! `ordo_commandlog::chain`'s canonical layout: one encoding, no serde, no
//! ambiguity about where a field ends. `plain_len` is redundant with the
//! remaining slice length and is carried anyway, because a truncated
//! envelope that still authenticates is impossible under AEAD but a
//! truncated *decode* is not, and a length that disagrees with the tail is
//! a refusal rather than a short read.
//!
//! There is no digest field. The AEAD tag over the whole envelope already
//! authenticates every byte under the space DEK, so a second integrity
//! hash inside it would authenticate nothing new — and, being a function
//! of the plaintext, it would be a content digest sitting *inside* the one
//! place it is provably harmless and one refactor away from the place it
//! is not. External blobs need `integrity` because their bytes leave the
//! envelope; inline blobs never do.

use std::fmt;

/// The version byte. Bumping it is a wire change: old envelopes keep
/// opening, because the byte is read rather than assumed.
pub const INLINE_V1: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InlineError {
    /// The envelope is shorter than its own length prefixes claim, or
    /// carries a version this build does not know.
    Malformed,
    /// A mime type longer than `u16::MAX`, or a body longer than the
    /// policy cap. Encoding refuses rather than truncating.
    TooLarge,
}

impl fmt::Display for InlineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InlineError::Malformed => f.write_str("inline blob envelope did not decode"),
            InlineError::TooLarge => f.write_str("inline blob envelope is too large to encode"),
        }
    }
}

impl std::error::Error for InlineError {}

/// A decoded inline blob: what the payload store holds behind an inline
/// field's handle.
#[derive(Clone, PartialEq, Eq)]
pub struct Inline {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Deliberately not `Debug`-derived: the payload is user content, and a
/// derived `Debug` puts it in every log line that formats a struct
/// containing one. The size and the mime are safe to print *here* because
/// this value only exists after the envelope has been opened, which took a
/// key release; what is not safe is printing it by accident.
impl fmt::Debug for Inline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inline")
            .field("mime", &self.mime)
            .field("bytes", &format_args!("<{} bytes>", self.bytes.len()))
            .finish()
    }
}

impl Inline {
    pub fn new(mime: impl Into<String>, bytes: Vec<u8>) -> Self {
        Inline {
            mime: mime.into(),
            bytes,
        }
    }

    /// The plaintext length the shape summary and `f.size` report.
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// Encode for sealing. The result is handed straight to
    /// [`crate::custody::KeyProvider::seal_at`]; nothing here encrypts.
    pub fn encode(&self) -> Result<Vec<u8>, InlineError> {
        let mime = self.mime.as_bytes();
        if mime.len() > u16::MAX as usize {
            return Err(InlineError::TooLarge);
        }
        let mut out = Vec::with_capacity(1 + 2 + mime.len() + 8 + self.bytes.len());
        out.push(INLINE_V1);
        out.extend_from_slice(&(mime.len() as u16).to_be_bytes());
        out.extend_from_slice(mime);
        out.extend_from_slice(&(self.bytes.len() as u64).to_be_bytes());
        out.extend_from_slice(&self.bytes);
        Ok(out)
    }

    /// Decode what an `open` produced. Every malformation is one error:
    /// which prefix disagreed is not a distinction worth printing, and the
    /// crate's standing rule is that a sealing failure says nothing about
    /// *why*.
    pub fn decode(buf: &[u8]) -> Result<Inline, InlineError> {
        let need = |b: &[u8], n: usize| -> Result<(), InlineError> {
            if b.len() < n {
                Err(InlineError::Malformed)
            } else {
                Ok(())
            }
        };
        need(buf, 3)?;
        if buf[0] != INLINE_V1 {
            return Err(InlineError::Malformed);
        }
        let mime_len = u16::from_be_bytes([buf[1], buf[2]]) as usize;
        let rest = &buf[3..];
        need(rest, mime_len + 8)?;
        let mime = std::str::from_utf8(&rest[..mime_len]).map_err(|_| InlineError::Malformed)?;
        let rest = &rest[mime_len..];
        let plain_len = u64::from_be_bytes(rest[..8].try_into().expect("checked")) as usize;
        let body = &rest[8..];
        if body.len() != plain_len {
            return Err(InlineError::Malformed);
        }
        Ok(Inline {
            mime: mime.to_string(),
            bytes: body.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let i = Inline::new("image/png", vec![0x89, b'P', b'N', b'G', 0, 0xff]);
        let enc = i.encode().unwrap();
        assert_eq!(Inline::decode(&enc).unwrap(), i);
        assert_eq!(i.size(), 6);
    }

    #[test]
    fn an_empty_blob_is_a_blob() {
        let i = Inline::new("application/octet-stream", Vec::new());
        assert_eq!(Inline::decode(&i.encode().unwrap()).unwrap(), i);
    }

    /// The redundant length is the point: a body that disagrees with its
    /// prefix is refused, not silently short-read.
    #[test]
    fn a_disagreeing_length_refuses() {
        let mut enc = Inline::new("text/plain", b"hello".to_vec())
            .encode()
            .unwrap();
        enc.pop();
        assert_eq!(Inline::decode(&enc), Err(InlineError::Malformed));
        enc.push(0);
        enc.push(0);
        assert_eq!(Inline::decode(&enc), Err(InlineError::Malformed));
    }

    #[test]
    fn a_version_this_build_does_not_know_refuses() {
        let mut enc = Inline::new("text/plain", b"x".to_vec()).encode().unwrap();
        enc[0] = 2;
        assert_eq!(Inline::decode(&enc), Err(InlineError::Malformed));
    }

    #[test]
    fn truncation_at_every_length_refuses_rather_than_panicking() {
        let enc = Inline::new("text/plain", b"abcdef".to_vec())
            .encode()
            .unwrap();
        for n in 0..enc.len() {
            assert_eq!(
                Inline::decode(&enc[..n]),
                Err(InlineError::Malformed),
                "a {n}-byte prefix decoded"
            );
        }
    }

    /// Non-UTF-8 in the mime slot is a malformation, not a lossy string.
    #[test]
    fn a_non_utf8_mime_refuses() {
        let mut enc = Inline::new("ab", b"x".to_vec()).encode().unwrap();
        enc[3] = 0xff;
        assert_eq!(Inline::decode(&enc), Err(InlineError::Malformed));
    }

    /// `Debug` never prints the bytes. This is asserted rather than
    /// reviewed because the failure mode is a log line, which nobody sees
    /// until it is in a support ticket.
    #[test]
    fn debug_does_not_print_the_payload() {
        let i = Inline::new("text/plain", b"the-secret".to_vec());
        let s = format!("{i:?}");
        assert!(!s.contains("secret"), "{s}");
        assert!(s.contains("<10 bytes>"), "{s}");
    }
}
