//! Chunked STREAM — sealing an object nobody can hold in memory, and
//! reading a range out of the middle of it.
//!
//! [`crate::seal_with`] is single-shot: it takes a `&[u8]` and returns a
//! `Vec<u8>`. That is right for a field and impossible for a file. This
//! crate's own doc has recorded the gap since F4 — *"chunked STREAM for
//! blobs is still absent — range reads over sealed blobs stay future
//! work"* — and `f7-vault-plan.md` records the consequence in the other
//! direction: *"No sealed-blob range reads … blobs in a vault space are
//! whole-object `private`."* This module is what lifts both.
//!
//! # The construction, and why this one
//!
//! STREAM (Hoang–Reyhanitabar–Rogaway–Vizár, *Online Authenticated-
//! Encryption and its Nonce-Reuse Misuse-Resistance*, CRYPTO 2015) — the
//! same construction `age` and RustCrypto's `aead::stream` use, and it is
//! taken rather than invented for the reason this crate takes
//! XChaCha20-Poly1305 rather than writing one: **primitives and published
//! constructions come from outside; only the canonical byte layout is
//! ours.**
//!
//! The whole of STREAM is a nonce discipline. Each chunk is sealed with
//! the ordinary AEAD under a nonce built from three parts:
//!
//! ```text
//! nonce = [base: 19 random bytes][counter: u32 big-endian][final: 0 or 1]
//! ```
//!
//! and that buys three properties an author would otherwise have to
//! remember to check by hand:
//!
//! 1. **Reordering is caught.** The counter is in the nonce, so chunk 7's
//!    ciphertext does not authenticate at position 4. Nothing has to
//!    compare an index.
//! 2. **Truncation is caught.** Exactly one chunk — the last — carries
//!    `final = 1`, so a stream cut short ends on a chunk whose tag says it
//!    was not the end. Without the flag, dropping the tail of an object
//!    produces a *shorter valid object*, which is the failure mode a
//!    per-chunk MAC does not catch and the one that matters most for a
//!    document.
//! 3. **Cross-object splicing is caught**, by the base being fresh per
//!    object, and cross-*column* splicing by the AAD, exactly as
//!    [`crate::seal_with`] binds `(entity, field)` so a payload moved to
//!    another column refuses to open.
//!
//! # Why a fixed plaintext chunk size
//!
//! [`CHUNK`] is 64 KiB and is **part of the format**, not a tuning knob.
//! Range reads are the reason: a reader given a plaintext offset has to
//! find the ciphertext offset without reading anything first, and that is
//! arithmetic only when every chunk but the last is the same size. A
//! variable chunk size would need a length index, the index would have to
//! be authenticated, and the whole thing would be a second format on top
//! of this one.
//!
//! It also fixes the overhead at a stated number: 16 bytes of tag per
//! 65536 of plaintext, 0.024%, plus a 21-byte header.
//!
//! # What the header carries, and what it does not
//!
//! ```text
//! [version: 1][epoch: 1][base nonce: 19]
//! ```
//!
//! The **epoch leads the envelope** exactly as it does in the single-shot
//! format, and for the identical reason: a later read has to find the
//! right key, and DEK rotation must be a re-seal job rather than a wire
//! migration. Note that this is the *object's* header, inside the bucket —
//! it is not the object *key*, which carries no epoch, because a bucket
//! listing that advertised epochs would print a revocation timeline to
//! someone holding no keys.
//!
//! There is no length, no digest and no chunk count in the header. The
//! length is the object's length, which the store already knows; a digest
//! would be a content hash living one refactor away from becoming an
//! address; and the chunk count is derivable and would be a second
//! statement of the same fact that could disagree with the first.

use crate::custody::{Anchor, Epoch};
use crate::SealError;

/// The plaintext bytes per chunk. **Part of the format.** See the module
/// doc: range arithmetic needs a fixed size, and a variable one would need
/// an authenticated length index — a second format on top of this one.
pub const CHUNK: usize = 64 * 1024;

/// The AEAD tag each chunk carries.
pub const TAG: usize = 16;

/// One sealed chunk, at every position but the last.
pub const SEALED_CHUNK: usize = CHUNK + TAG;

/// `[version][epoch][base nonce]`.
pub const HEADER: usize = 1 + 1 + 19;

/// The format version. Read rather than assumed, so a future layout can
/// be introduced without every stored object becoming unreadable.
pub const STREAM_V1: u8 = 1;

/// The fresh per-object randomness the nonces are built from.
pub type NonceBase = [u8; 19];

/// A parsed object header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamHeader {
    pub epoch: Epoch,
    pub base: NonceBase,
}

impl StreamHeader {
    /// Mint a header for a new object. The base is fresh CSPRNG bytes per
    /// object — never per deployment, never per space, and never derived
    /// from the content, which would make it a content address.
    pub fn new(epoch: Epoch) -> StreamHeader {
        let seed = crate::custody::random32();
        let mut base = [0u8; 19];
        base.copy_from_slice(&seed[..19]);
        StreamHeader { epoch, base }
    }

    pub fn encode(&self) -> [u8; HEADER] {
        let mut out = [0u8; HEADER];
        out[0] = STREAM_V1;
        out[1] = self.epoch;
        out[2..].copy_from_slice(&self.base);
        out
    }

    /// Every malformation is one error, deliberately — the crate's
    /// standing rule is that a sealing failure says nothing about *why*.
    pub fn decode(bytes: &[u8]) -> Result<StreamHeader, SealError> {
        if bytes.len() < HEADER || bytes[0] != STREAM_V1 {
            return Err(SealError);
        }
        let mut base = [0u8; 19];
        base.copy_from_slice(&bytes[2..HEADER]);
        Ok(StreamHeader {
            epoch: bytes[1],
            base,
        })
    }
}

/// `[base][counter BE][final]` — the whole of STREAM.
fn nonce(base: &NonceBase, counter: u32, last: bool) -> [u8; 24] {
    let mut n = [0u8; 24];
    n[..19].copy_from_slice(base);
    n[19..23].copy_from_slice(&counter.to_be_bytes());
    n[23] = u8::from(last);
    n
}

fn aad(entity: &str, field: &str, anchor: Anchor<'_>) -> Vec<u8> {
    // The same AAD the single-shot envelope uses. Deliberately identical
    // rather than a stream-specific string: the property being bought is
    // "this ciphertext belongs to this column, at this object", and it is
    // the same property, so it is the same binding. A `stream:` prefix
    // would create two answers to one question.
    crate::aad_at(entity, field, anchor)
}

/// Seal one chunk. `last` must be true for exactly one chunk, and it must
/// be the final one.
#[allow(clippy::too_many_arguments)]
pub fn seal_chunk_with(
    key: &[u8; 32],
    entity: &str,
    field: &str,
    anchor: Anchor<'_>,
    base: &NonceBase,
    counter: u32,
    last: bool,
    plaintext: &[u8],
) -> Vec<u8> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::XChaCha20Poly1305;
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .encrypt(
            (&nonce(base, counter, last)).into(),
            Payload {
                msg: plaintext,
                aad: &aad(entity, field, anchor),
            },
        )
        .expect("XChaCha20-Poly1305 encryption is total over byte inputs")
}

/// Open one chunk. A chunk that authenticates at a different position, in
/// a different column, in a different object, or with a different
/// finality, does not open — and every one of those is the same
/// [`SealError`].
#[allow(clippy::too_many_arguments)]
pub fn open_chunk_with(
    key: &[u8; 32],
    entity: &str,
    field: &str,
    anchor: Anchor<'_>,
    base: &NonceBase,
    counter: u32,
    last: bool,
    chunk: &[u8],
) -> Result<Vec<u8>, SealError> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::XChaCha20Poly1305;
    if chunk.len() < TAG {
        return Err(SealError);
    }
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(
            (&nonce(base, counter, last)).into(),
            Payload {
                msg: chunk,
                aad: &aad(entity, field, anchor),
            },
        )
        .map_err(|_| SealError)
}

// ── the arithmetic, which is the part most likely to be wrong ────────────

/// The stored length of an object whose plaintext is `plain` bytes.
///
/// A zero-byte plaintext is **one** chunk, not zero: the final-flagged
/// chunk is what proves the object ended, and an object with no chunks
/// would be indistinguishable from a truncation to nothing.
pub fn sealed_len(plain: u64) -> u64 {
    let full = plain / CHUNK as u64;
    let rest = plain % CHUNK as u64;
    let chunks = if rest == 0 && plain != 0 {
        full
    } else {
        full + 1
    };
    HEADER as u64 + plain + chunks * TAG as u64
}

/// How many chunks an object of `plain` plaintext bytes has.
pub fn chunk_count(plain: u64) -> u64 {
    let full = plain / CHUNK as u64;
    let rest = plain % CHUNK as u64;
    if rest == 0 && plain != 0 {
        full
    } else {
        full + 1
    }
}

/// The plaintext byte range `[from, to)` a caller asked for, resolved to
/// the **stored** bytes that have to be fetched and the offset to discard
/// from the front of what comes back.
///
/// This is the whole point of the chunked format, and it is where a
/// mistake is expensive rather than loud: a range that is off by a chunk
/// still decrypts — into the wrong bytes — because every chunk is
/// individually valid. So it is a pure function with its own tests rather
/// than arithmetic inlined at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangePlan {
    /// Where to start reading the stored object.
    pub stored_offset: u64,
    /// How many stored bytes to read.
    pub stored_len: u64,
    /// The counter of the first chunk fetched.
    pub first_chunk: u32,
    /// Bytes to drop from the front of the decrypted result.
    pub discard: u64,
    /// Plaintext bytes to keep after the discard.
    pub keep: u64,
}

/// Plan a range read. `plain_len` is the object's plaintext length, which
/// the reference row knows.
///
/// Returns `None` when the range is empty or starts past the end — a
/// refusal, never an empty success, because an empty success is how a
/// truncated download reads as a complete one.
pub fn plan_range(plain_len: u64, from: u64, len: Option<u64>) -> Option<RangePlan> {
    if from >= plain_len && !(from == 0 && plain_len == 0) {
        return None;
    }
    let want = len.unwrap_or(plain_len - from).min(plain_len - from);
    if want == 0 {
        return None;
    }
    let chunk = CHUNK as u64;
    let first = from / chunk;
    // The last chunk that holds a byte we want. `want >= 1` here, so
    // `from + want - 1` is a real byte index.
    let last = (from + want - 1) / chunk;
    let n = last - first + 1;
    Some(RangePlan {
        stored_offset: HEADER as u64 + first * SEALED_CHUNK as u64,
        stored_len: n * SEALED_CHUNK as u64,
        first_chunk: u32::try_from(first).ok()?,
        discard: from - first * chunk,
        keep: want,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJ: Anchor<'static> = Anchor::Object("obj_1");

    fn key() -> [u8; 32] {
        [7u8; 32]
    }

    #[test]
    fn a_header_round_trips_and_a_foreign_version_refuses() {
        let h = StreamHeader::new(3);
        let bytes = h.encode();
        assert_eq!(StreamHeader::decode(&bytes).unwrap(), h);
        assert_eq!(bytes[1], 3, "the epoch leads the envelope");
        let mut bad = bytes;
        bad[0] = 2;
        assert!(StreamHeader::decode(&bad).is_err());
        assert!(StreamHeader::decode(&bytes[..HEADER - 1]).is_err());
    }

    /// Two objects never share a nonce base, so a chunk from one cannot be
    /// spliced into the other even at the same position.
    #[test]
    fn every_object_gets_a_fresh_base() {
        let a = StreamHeader::new(1);
        let b = StreamHeader::new(1);
        assert_ne!(a.base, b.base);
    }

    #[test]
    fn a_chunk_round_trips() {
        let h = StreamHeader::new(1);
        let plain = vec![0xabu8; 1000];
        let ct = seal_chunk_with(&key(), "docs", "file", OBJ, &h.base, 0, true, &plain);
        assert_eq!(ct.len(), plain.len() + TAG);
        assert_eq!(
            open_chunk_with(&key(), "docs", "file", OBJ, &h.base, 0, true, &ct).unwrap(),
            plain
        );
    }

    /// **Reordering.** Chunk 7 does not authenticate at position 4,
    /// without anything comparing an index.
    #[test]
    fn a_reordered_chunk_does_not_open() {
        let h = StreamHeader::new(1);
        let ct = seal_chunk_with(&key(), "docs", "file", OBJ, &h.base, 7, false, b"seven");
        assert!(open_chunk_with(&key(), "docs", "file", OBJ, &h.base, 4, false, &ct).is_err());
        assert!(open_chunk_with(&key(), "docs", "file", OBJ, &h.base, 7, false, &ct).is_ok());
    }

    /// **Truncation.** The final flag is what makes a cut-short object
    /// fail rather than become a shorter valid one — the failure mode a
    /// per-chunk MAC does not catch.
    #[test]
    fn a_truncated_object_cannot_pass_as_a_shorter_one() {
        let h = StreamHeader::new(1);
        let mid = seal_chunk_with(&key(), "docs", "file", OBJ, &h.base, 0, false, b"first");
        // A reader that reached the end of the stream here would try to
        // open chunk 0 as the last one, and it does not open.
        assert!(open_chunk_with(&key(), "docs", "file", OBJ, &h.base, 0, true, &mid).is_err());
        assert!(open_chunk_with(&key(), "docs", "file", OBJ, &h.base, 0, false, &mid).is_ok());
    }

    /// **Cross-column splicing**, refused by the same AAD the single-shot
    /// envelope uses.
    #[test]
    fn a_chunk_moved_to_another_column_does_not_open() {
        let h = StreamHeader::new(1);
        let ct = seal_chunk_with(&key(), "docs", "file", OBJ, &h.base, 0, true, b"contract");
        assert!(open_chunk_with(&key(), "docs", "avatar", OBJ, &h.base, 0, true, &ct).is_err());
        assert!(open_chunk_with(&key(), "invoices", "file", OBJ, &h.base, 0, true, &ct).is_err());
    }

    /// **Two objects in one column**, refused by the handle in the
    /// associated data, even when they share a nonce base. This is the swap
    /// an object-store writer performs (S6-003): both objects are sealed
    /// under the same key at the same column, and only the handle tells
    /// them apart.
    #[test]
    fn a_chunk_moved_to_another_object_of_the_same_column_does_not_open() {
        let h = StreamHeader::new(1);
        let ct = seal_chunk_with(
            &key(),
            "docs",
            "file",
            Anchor::Object("obj_1"),
            &h.base,
            0,
            true,
            b"mine",
        );
        assert!(open_chunk_with(
            &key(),
            "docs",
            "file",
            Anchor::Object("obj_2"),
            &h.base,
            0,
            true,
            &ct
        )
        .is_err());
        assert!(open_chunk_with(
            &key(),
            "docs",
            "file",
            Anchor::Object("obj_1"),
            &h.base,
            0,
            true,
            &ct
        )
        .is_ok());
    }

    /// **Cross-object splicing**, refused by the fresh base.
    #[test]
    fn a_chunk_from_another_object_does_not_open() {
        let a = StreamHeader::new(1);
        let b = StreamHeader::new(1);
        let ct = seal_chunk_with(&key(), "docs", "file", OBJ, &a.base, 0, true, b"mine");
        assert!(open_chunk_with(&key(), "docs", "file", OBJ, &b.base, 0, true, &ct).is_err());
    }

    #[test]
    fn a_flipped_bit_does_not_open() {
        let h = StreamHeader::new(1);
        let mut ct = seal_chunk_with(&key(), "docs", "file", OBJ, &h.base, 0, true, b"payload");
        ct[3] ^= 1;
        assert!(open_chunk_with(&key(), "docs", "file", OBJ, &h.base, 0, true, &ct).is_err());
    }

    /// A zero-byte object is one final-flagged chunk, not zero chunks:
    /// with no chunks there is nothing that says the object ended.
    #[test]
    fn an_empty_object_still_has_a_chunk() {
        assert_eq!(chunk_count(0), 1);
        assert_eq!(sealed_len(0), HEADER as u64 + TAG as u64);
    }

    #[test]
    fn the_sealed_length_is_the_plaintext_plus_a_tag_per_chunk() {
        let c = CHUNK as u64;
        assert_eq!(sealed_len(1), HEADER as u64 + 1 + TAG as u64);
        assert_eq!(sealed_len(c), HEADER as u64 + c + TAG as u64);
        assert_eq!(sealed_len(c + 1), HEADER as u64 + c + 1 + 2 * TAG as u64);
        assert_eq!(chunk_count(c), 1);
        assert_eq!(chunk_count(c + 1), 2);
        assert_eq!(chunk_count(2 * c), 2);
    }

    /// The arithmetic, against an independently computed answer. A range
    /// off by a chunk still *decrypts* — into the wrong bytes — so this is
    /// checked rather than reviewed.
    #[test]
    fn a_range_plan_lands_on_the_right_bytes() {
        let c = CHUNK as u64;
        // Wholly inside chunk 0.
        let p = plan_range(10 * c, 100, Some(50)).unwrap();
        assert_eq!(p.first_chunk, 0);
        assert_eq!(p.stored_offset, HEADER as u64);
        assert_eq!(p.stored_len, SEALED_CHUNK as u64);
        assert_eq!(p.discard, 100);
        assert_eq!(p.keep, 50);

        // Straddling the boundary between chunks 1 and 2.
        let p = plan_range(10 * c, 2 * c - 5, Some(10)).unwrap();
        assert_eq!(p.first_chunk, 1);
        assert_eq!(p.stored_offset, HEADER as u64 + SEALED_CHUNK as u64);
        assert_eq!(p.stored_len, 2 * SEALED_CHUNK as u64);
        assert_eq!(p.discard, c - 5);
        assert_eq!(p.keep, 10);

        // Open-ended, from the middle.
        let p = plan_range(3 * c, 2 * c, None).unwrap();
        assert_eq!(p.first_chunk, 2);
        assert_eq!(p.keep, c);
        assert_eq!(p.discard, 0);

        // Clamped to the object rather than reading past it.
        let p = plan_range(100, 90, Some(1000)).unwrap();
        assert_eq!(p.keep, 10);

        // Past the end is a refusal, never an empty success.
        assert!(plan_range(100, 100, Some(1)).is_none());
        assert!(plan_range(100, 200, None).is_none());
        assert!(plan_range(100, 0, Some(0)).is_none());
    }

    /// The plan and the seal agree: sealing an object, planning a range
    /// over it, and opening exactly the chunks the plan names returns
    /// exactly the requested plaintext. Run over sizes that straddle every
    /// boundary the arithmetic has.
    #[test]
    fn a_planned_range_returns_exactly_the_requested_bytes() {
        let c = CHUNK;
        for plain_len in [0usize, 1, 100, c - 1, c, c + 1, 2 * c, 2 * c + 7] {
            let plain: Vec<u8> = (0..plain_len).map(|i| (i % 251) as u8).collect();
            let h = StreamHeader::new(1);

            // Seal the whole object, exactly as the door does.
            let mut stored = h.encode().to_vec();
            let n = chunk_count(plain_len as u64) as usize;
            for i in 0..n {
                let lo = i * c;
                let hi = ((i + 1) * c).min(plain_len);
                stored.extend_from_slice(&seal_chunk_with(
                    &key(),
                    "docs",
                    "file",
                    OBJ,
                    &h.base,
                    i as u32,
                    i + 1 == n,
                    &plain[lo..hi],
                ));
            }
            assert_eq!(stored.len() as u64, sealed_len(plain_len as u64));

            for (from, len) in [
                (0u64, None),
                (0, Some(1u64)),
                (1, Some(3)),
                (plain_len.saturating_sub(1) as u64, Some(1)),
                (c as u64 / 2, Some(c as u64)),
            ] {
                let Some(p) = plan_range(plain_len as u64, from, len) else {
                    continue;
                };
                // Fetch exactly what the plan asks for, clamped to the
                // object the way a store's range read clamps.
                let end = ((p.stored_offset + p.stored_len) as usize).min(stored.len());
                let fetched = &stored[p.stored_offset as usize..end];

                let mut out = Vec::new();
                let mut at = 0usize;
                let mut counter = p.first_chunk;
                while at < fetched.len() {
                    let take = SEALED_CHUNK.min(fetched.len() - at);
                    let last = counter as u64 + 1 == chunk_count(plain_len as u64);
                    out.extend_from_slice(
                        &open_chunk_with(
                            &key(),
                            "docs",
                            "file",
                            OBJ,
                            &h.base,
                            counter,
                            last,
                            &fetched[at..at + take],
                        )
                        .expect("a chunk the plan named must open"),
                    );
                    at += take;
                    counter += 1;
                }
                let got = &out[p.discard as usize..(p.discard + p.keep) as usize];
                assert_eq!(
                    got,
                    &plain[from as usize..(from + p.keep) as usize],
                    "len={plain_len} from={from} len={len:?}"
                );
            }
        }
    }
}
