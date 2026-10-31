//! The per-space hash chain and its canonical byte encoding.
//!
//! # Canonical encoding — frozen layout
//!
//! Deterministic by construction: no maps with unstable order (images are
//! `BTreeMap`s), no floats, no textual serialization ambiguity. All integers
//! big-endian; all strings length-prefixed UTF-8 (NFC-normalized upstream).
//!
//! That parenthesis was an assumption with nothing behind it until 9u: no
//! crate normalized anything, so a chain hash could differ between two
//! deployments handed the same text in two encodings. The upstream now
//! exists and is named — `ordo_syntax::nfc`, applied at the lexer for
//! literals, at `ordo-api`'s door for caller values, and at the write
//! path's SQL funnel. This module still does not normalize: a log that
//! silently rewrote what it was handed could not be an audit spine.
//!
//! ```text
//! str        := u64-BE byte-length ‖ UTF-8 bytes
//! value      := tag:u8 ‖ payload
//!                 0 Null       (no payload)
//!                 1 Bool       u8 (0|1)
//!                 2 Int        i64-BE
//!                 3 Text       str
//!                 4 Id         str
//!                 5 SealedRef  16 raw bytes (opaque handle)
//!                 6 Decimal    str, the canonical spelling (27ak)
//! image      := u64-BE field-count ‖ (str key ‖ value)*      # key order
//! opt-image  := 0x00 | 0x01 ‖ image
//! command    := str id ‖ str space ‖ str entity ‖ str row_id
//!               ‖ kind:u8 (0 create, 1 converge, 2 delete)
//!               ‖ opt-image before ‖ opt-image after ‖ u64-BE at
//! group      := str correlation ‖ str actor-attribution ‖ u64-BE count ‖ command*
//!               ‖ trailer? ‖ trailer? ‖ trailer?
//! trailer    := 0x01                      # owner administration (DL §3 r6)
//!             | 0x02                      # restored unverified (opslog D7)
//!             | 0x03 ‖ restore            # restore marker (production-ops R2a)
//! restore    := str space ‖ u64-BE idx ‖ hash32
//!               ‖ opt-head ‖ source ‖ str operator
//! opt-head   := 0x00 | 0x01 ‖ u64-BE idx ‖ hash32
//! source     := 0x00 ‖ u64-BE at | 0x01 ‖ digest32
//! hash       := SHA-256( prev-hash (32 bytes) ‖ group )
//! ```
//!
//! The actor encodes as its canonical attribution string (`Display`) — one
//! rendering, hashed as displayed. Genesis `prev` is 32 zero bytes.
//!
//! # The decimal tag, added by presence
//!
//! Tag `6` arrived with the storable `decimal(p)` (27ak, ruled 2026-10-04),
//! for the reason the trailers below arrived the way they did: a value tag
//! is a byte only a decimal value renders, so every group written before it
//! holds no decimal and renders exactly the bytes it rendered before. That
//! identity is pinned by `tests.rs`'s `PRE_DECIMAL_TAG_GOLDEN`, computed
//! over a group carrying every older tag before tag `6` existed.
//!
//! The payload is one canonical spelling ([`crate::value::Decimal`]):
//! trailing zeros stripped, no leading zeros, no `+`, no `-0`. A value has
//! one spelling and so one hash, whatever scale its column declares, and
//! the decoder refuses any other spelling as [`DecodeError::BadDecimal`].
//! `money` does not use it: money is whole minor units and rides
//! [`Value::Int`], as it always has.
//!
//! # The trailer, and why a frozen layout could grow one
//!
//! The layout above is frozen and unversioned, so adding a field to
//! [`CommandGroup`] is not a free act: the chain hashes the *rendering*, and
//! any byte added to the rendering of an existing group changes that group's
//! hash — which would invalidate every chain already written, every signed
//! checkpoint over it, and every golden that pins one. A field encoded
//! unconditionally (`0x00` for "no", `0x01` for "yes") does exactly that: it
//! adds a byte to groups that predate the field and have no opinion about it.
//!
//! So each flag is encoded **by presence, not by value**. A group that is
//! neither owner administration nor a restore with something unverified
//! renders exactly the bytes it rendered before either field existed (both
//! trailers are absent) and therefore hashes to exactly the same place. A
//! group flagged one way renders one additional byte; flagged both ways,
//! two. The backward compatibility is not a compatibility *shim*; it is the
//! identity `encode(unflagged) == the old encoding`, held by construction
//! and pinned by a golden hash computed before each field was added:
//! `tests.rs`'s `PRE_FLAG_GOLDEN` for the first, `PRE_SECOND_FLAG_GOLDEN`
//! for the second, and `PRE_THIRD_FLAG_GOLDEN` (with
//! `PRE_THIRD_FLAG_BOTH_TAGS_GOLDEN`, a group carrying both older tags) for
//! the third.
//!
//! The third trailer, `0x03`, is the restore marker
//! ([`crate::command::RestoreMarker`]) and the first tag followed by a
//! payload. Presence still decides everything: a group with no marker
//! renders no `0x03` and no payload byte. The payload is a fixed sequence
//! of fields whose own option and variant bytes are closed (`0x00` or
//! `0x01`, anything else is `BadTag`), so it has one rendering per marker
//! and the bijection below holds with it.
//!
//! Canonicity survives, and it has to: two renderings of one group would let
//! one group be re-hashed two ways. Present tags are rendered in ascending
//! order, `0x01` before `0x02`, and a decoder walks the trailer bytes
//! expecting that order: each byte read must be a known tag strictly
//! greater than the last one it saw. That one rule does the work a
//! per-tag special case would otherwise need for every combination:
//!
//! - an explicit `0x00` "no flags" byte is refused, for either position,
//!   because `0x00` names no tag: "no flags" has one spelling, which is
//!   silence;
//! - `0x02` alone (owner administration absent, the restore flag present)
//!   is accepted, because absence of the first tag is itself silence, not a
//!   byte;
//! - `0x02` then `0x01`, the two known tags out of order, is refused,
//!   because it is a second spelling of `0x01 ‖ 0x02`;
//! - `0x01 0x01`, or any repeated tag, is refused, because a tag names a
//!   fact and a fact is not stated twice;
//! - anything past the tags that exist today is refused, because a
//!   group has at most one trailer per flag;
//! - anything after the restore marker's payload is refused with no
//!   special case: `0x03` is the highest tag, so no byte after it can be
//!   strictly greater and known.
//!
//! The map from group to bytes stays a bijection in every direction this
//! produces, and only the bytes that gained a meaning changed one.
//!
//! Unknown trailer tags are refused rather than skipped, for the same
//! reason: the ascending-order rule refuses any byte that is not one of the
//! tags this decoder knows, wherever it appears. A future flag would take a
//! new tag byte, `0x04`, keeping the order, and an older reader meeting it
//! fails loudly instead of decoding a group whose recorded meaning it did
//! not see. For an audit spine that is the right failure: a flag silently
//! dropped is a record that quietly says something other than what was
//! written.
//!
//! # The encoding is also a decoding
//!
//! It was write-only while the only store was in memory: the `Vec` held the
//! groups, and the bytes existed solely to be hashed. A durable store has no
//! `Vec` — it holds bytes and must hand back a [`CommandGroup`], so
//! [`decode_group`] is the exact inverse and `encode_group ∘ decode_group`
//! is the identity on every group the gate admits. That matters beyond
//! convenience: `verify` recomputes the hash from *decoded stored content*,
//! so a byte flipped in the database is caught by the chain rather than
//! quietly reappearing as a different group.

use sha2::{Digest, Sha256};

use crate::command::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, RestoreMarker,
    RestoreSource, SpaceId, Timestamp,
};
use crate::value::{Image, Value};

/// A SHA-256 chain hash. Genesis prev = all zeros.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainHash(pub [u8; 32]);

impl ChainHash {
    pub const GENESIS: ChainHash = ChainHash([0u8; 32]);
}

impl std::fmt::Display for ChainHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// A group as stored: the group plus its chain position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainedGroup {
    pub group: CommandGroup,
    pub prev: ChainHash,
    pub hash: ChainHash,
}

/// Where verification first failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TamperKind {
    /// Stored `prev` does not equal the previous entry's stored hash.
    BrokenLink,
    /// Recomputed hash of (prev ‖ encoding) differs from the stored hash —
    /// the group's content was altered after append.
    HashMismatch,
    /// The chain could not be read at all — the store failed, or an entry's
    /// stored bytes are not a decodable group.
    ///
    /// A verification failure, deliberately, and not a separate error type:
    /// the alternative is a `verify` that returns `Ok` for a chain it never
    /// managed to look at, which is the most dangerous answer this function
    /// can give. "I could not check" and "it checks out" must not be the
    /// same value.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TamperAt {
    pub space: SpaceId,
    /// Index into the space's chain (0-based) of the first bad entry.
    pub index: usize,
    pub kind: TamperKind,
}

impl std::fmt::Display for TamperAt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.kind {
            TamperKind::BrokenLink => "chain link broken",
            TamperKind::HashMismatch => "content hash mismatch",
            TamperKind::Unreadable => "chain could not be read",
        };
        write!(
            f,
            "space `{}`: {} at index {}",
            self.space, what, self.index
        )
    }
}

impl std::error::Error for TamperAt {}

/// Hash a group onto the chain: `SHA-256(prev ‖ canonical-encoding(group))`.
pub fn hash_group(prev: ChainHash, group: &CommandGroup) -> ChainHash {
    let mut hasher = Sha256::new();
    hasher.update(prev.0);
    hasher.update(encode_group(group));
    ChainHash(hasher.finalize().into())
}

/// Verify a whole chain, from genesis. Only correct for a space that has
/// never been pruned — see [`verify_chain_from`], which this delegates to.
pub fn verify_chain(space: &SpaceId, chain: &[ChainedGroup]) -> Result<(), TamperAt> {
    verify_chain_from(space, chain, ChainHash::GENESIS, 0)
}

/// Verify a chain slice against an explicit anchor: every link points at its
/// predecessor's stored hash, every stored hash recomputes from stored
/// content. Returns the first failure, link checked before content.
///
/// `anchor` is the hash the slice's first entry must point back at, and
/// `base_index` is that entry's position in the full chain, so a reported
/// index means the same thing whether or not the prefix is still stored.
///
/// The anchor exists because pruning is a supported operation
/// (`policy commandlog { checkpoint … }`): once a prefix is dropped, the
/// surviving suffix does *not* start at genesis, and verifying it as if it
/// did reports `BrokenLink` at index 0 — making a lawful prune
/// indistinguishable from tampering. Callers verifying a pruned space pass
/// the head of the verified checkpoint the prune cut at.
pub fn verify_chain_from(
    space: &SpaceId,
    chain: &[ChainedGroup],
    anchor: ChainHash,
    base_index: usize,
) -> Result<(), TamperAt> {
    let mut expected_prev = anchor;
    for (offset, entry) in chain.iter().enumerate() {
        let index = base_index + offset;
        if entry.prev != expected_prev {
            return Err(TamperAt {
                space: space.clone(),
                index,
                kind: TamperKind::BrokenLink,
            });
        }
        if hash_group(entry.prev, &entry.group) != entry.hash {
            return Err(TamperAt {
                space: space.clone(),
                index,
                kind: TamperKind::HashMismatch,
            });
        }
        expected_prev = entry.hash;
    }
    Ok(())
}

// ───────────────────────── canonical encoding ───────────────────────────

/// The canonical bytes a group hashes as. Public because a durable store
/// persists exactly these — storing anything else would mean the thing
/// verified and the thing stored were two different artifacts.
pub fn encode_group(group: &CommandGroup) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, &group.correlation.0);
    put_str(&mut out, &group.actor.to_string());
    out.extend_from_slice(&(group.commands.len() as u64).to_be_bytes());
    for cmd in &group.commands {
        encode_command(&mut out, cmd);
    }
    // Present only when set: see "The trailer" in the module doc. A group
    // flagged neither way renders bytes byte-identical to what this
    // function produced before either field existed, which is what keeps
    // every chain already written verifiable. The two tags are pushed in
    // ascending order on purpose: that is the one order `decode_group`
    // accepts, so there is exactly one lawful rendering per pair of flags.
    if group.owner_administration {
        out.push(TRAILER_OWNER_ADMINISTRATION);
    }
    if group.restored_unverified {
        out.push(TRAILER_RESTORED_UNVERIFIED);
    }
    if let Some(marker) = &group.restore {
        out.push(TRAILER_RESTORE);
        encode_restore(&mut out, marker);
    }
    out
}

/// The restore marker's payload, after its `0x03` tag. Field order is the
/// grammar's `restore` production and is frozen with it.
fn encode_restore(out: &mut Vec<u8>, marker: &RestoreMarker) {
    put_str(out, &marker.space.0);
    put_position(out, marker.restored_from);
    match marker.pre_restore_head {
        None => out.push(0),
        Some(head) => {
            out.push(1);
            put_position(out, head);
        }
    }
    match &marker.source {
        RestoreSource::Pitr { at } => {
            out.push(0);
            out.extend_from_slice(&at.0.to_be_bytes());
        }
        RestoreSource::Bundle { manifest_sha256 } => {
            out.push(1);
            out.extend_from_slice(manifest_sha256);
        }
    }
    put_str(out, &marker.operator);
}

fn put_position(out: &mut Vec<u8>, (idx, hash): (u64, ChainHash)) {
    out.extend_from_slice(&idx.to_be_bytes());
    out.extend_from_slice(&hash.0);
}

/// The first trailer tag: this group is DL §3 rule 6 owner administration.
const TRAILER_OWNER_ADMINISTRATION: u8 = 0x01;

/// The second trailer tag (`plan-opslog.md` D7): this group's revert
/// restored at least one blob handle it had no recorded digest to verify
/// against. See [`crate::command::CommandGroup::restored_unverified`].
const TRAILER_RESTORED_UNVERIFIED: u8 = 0x02;

/// The third trailer tag (`plan-production-ops.md` R2a): this group is a
/// restore marker, and the tag is followed by its payload. See
/// [`crate::command::CommandGroup::restore`].
const TRAILER_RESTORE: u8 = 0x03;

fn encode_command(out: &mut Vec<u8>, cmd: &Command) {
    put_str(out, &cmd.id.0);
    put_str(out, &cmd.space.0);
    put_str(out, &cmd.entity);
    put_str(out, &cmd.row_id);
    out.push(match cmd.kind {
        CommandKind::Create => 0,
        CommandKind::Converge => 1,
        CommandKind::Delete => 2,
    });
    encode_opt_image(out, cmd.before.as_ref());
    encode_opt_image(out, cmd.after.as_ref());
    out.extend_from_slice(&cmd.at.0.to_be_bytes());
}

fn encode_opt_image(out: &mut Vec<u8>, image: Option<&Image>) {
    match image {
        None => out.push(0),
        Some(img) => {
            out.push(1);
            encode_image(out, img);
        }
    }
}

/// Public for the sealed-redaction test: the encoded image is exactly what
/// the chain hashes, so "plaintext never in the log" is checkable in bytes.
pub fn encode_image_bytes(image: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    encode_image(&mut out, image);
    out
}

fn encode_image(out: &mut Vec<u8>, image: &Image) {
    out.extend_from_slice(&(image.len() as u64).to_be_bytes());
    for (key, value) in image {
        put_str(out, key);
        encode_value(out, value);
    }
}

fn encode_value(out: &mut Vec<u8>, value: &Value) {
    out.push(value.tag());
    match value {
        Value::Null => {}
        Value::Bool(b) => out.push(u8::from(*b)),
        Value::Int(i) => out.extend_from_slice(&i.to_be_bytes()),
        Value::Text(s) => put_str(out, s),
        Value::Id(s) => put_str(out, s),
        Value::SealedRef(h) => out.extend_from_slice(h),
        Value::Decimal(d) => put_str(out, d.as_str()),
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u64).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

// ───────────────────────── canonical decoding ───────────────────────────

/// Why a stored group would not decode. Every variant means the stored
/// bytes are not something [`encode_group`] ever produced — so it is
/// corruption or tampering, never a lawful older format: the layout is
/// frozen and unversioned by design.
///
/// The optional trailers do not weaken that. A group written before either
/// flag existed decodes *successfully*, as a group flagged neither way,
/// because its bytes are a lawful current encoding rather than an older
/// format needing a fallback, which is exactly why there is still nothing
/// here for a version to mean.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// Ran off the end of the buffer.
    Truncated,
    /// Bytes remain after a complete group — a group is the whole message.
    TrailingBytes,
    /// A length prefix claims more than the buffer holds, or a count is
    /// absurd. Kept distinct from `Truncated` because it is the shape a
    /// hostile blob takes: a 2^63 length prefix over four bytes of payload.
    BadLength,
    /// A string field is not UTF-8.
    BadUtf8,
    /// A tag byte names no variant (value tag, kind byte, option byte).
    BadTag(u8),
    /// The actor string is not one of the five canonical attributions.
    BadActor(String),
    /// A decimal value whose stored spelling is not its canonical one
    /// (`0.0800` where the chain only ever writes `0.08`). Two spellings of
    /// one value would let one group hash two ways.
    BadDecimal(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated => f.write_str("stored group ends mid-field"),
            DecodeError::TrailingBytes => f.write_str("stored group has bytes after its end"),
            DecodeError::BadLength => f.write_str("stored group has an impossible length prefix"),
            DecodeError::BadUtf8 => f.write_str("stored group has a non-UTF-8 string"),
            DecodeError::BadTag(t) => write!(f, "stored group has unknown tag {t}"),
            DecodeError::BadActor(s) => write!(f, "`{s}` is not a canonical actor attribution"),
            DecodeError::BadDecimal(s) => {
                write!(f, "`{s}` is not the canonical spelling of a decimal")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// The exact inverse of [`encode_group`].
pub fn decode_group(bytes: &[u8]) -> Result<CommandGroup, DecodeError> {
    let mut r = Reader { b: bytes, at: 0 };
    let correlation = CorrelationId(r.str()?);
    let actor = Actor::parse(&r.str()?)?;
    let count = r.usize_len()?;
    let mut commands = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        commands.push(decode_command(&mut r)?);
    }
    // The optional trailers. Absent (the shape every group written before
    // either flag existed takes) means neither flag; an explicit `0x00` is
    // refused, so "no flags" keeps exactly one spelling and the encoding
    // stays a bijection. Present tags are read in ascending order, which is
    // the one order `encode_group` produces: each byte must be a known tag
    // strictly greater than the last one seen, so each subset of the three
    // tags has exactly one spelling, and every other byte (an unknown tag,
    // a repeat, two known tags out of order, or a byte beyond the tags that
    // exist today) is trailing rather than a second spelling of something.
    // `0x03` carries a payload, read in place; because it is the highest
    // tag, the same rule refuses any byte after the payload.
    let mut owner_administration = false;
    let mut restored_unverified = false;
    let mut restore = None;
    let mut last_tag = 0u8;
    while r.at != r.b.len() {
        let tag = r.byte()?;
        if tag <= last_tag {
            return Err(DecodeError::TrailingBytes);
        }
        match tag {
            TRAILER_OWNER_ADMINISTRATION => owner_administration = true,
            TRAILER_RESTORED_UNVERIFIED => restored_unverified = true,
            TRAILER_RESTORE => restore = Some(decode_restore(&mut r)?),
            _ => return Err(DecodeError::TrailingBytes),
        }
        last_tag = tag;
    }
    Ok(CommandGroup {
        correlation,
        actor,
        commands,
        owner_administration,
        restored_unverified,
        restore,
    })
}

/// The inverse of `encode_restore`. A payload cut short fails as the
/// reader fails anywhere else (`Truncated`, or `BadLength` when a string's
/// length prefix outruns the bytes left); an option or source byte other
/// than `0x00`/`0x01` is `BadTag`, so the payload has one spelling.
fn decode_restore(r: &mut Reader<'_>) -> Result<RestoreMarker, DecodeError> {
    let space = SpaceId(r.str()?);
    let restored_from = r.position()?;
    let pre_restore_head = match r.byte()? {
        0 => None,
        1 => Some(r.position()?),
        other => return Err(DecodeError::BadTag(other)),
    };
    let source = match r.byte()? {
        0 => RestoreSource::Pitr {
            at: Timestamp(r.u64()?),
        },
        1 => RestoreSource::Bundle {
            manifest_sha256: r.hash32()?,
        },
        other => return Err(DecodeError::BadTag(other)),
    };
    let operator = r.str()?;
    Ok(RestoreMarker {
        space,
        restored_from,
        pre_restore_head,
        source,
        operator,
    })
}

fn decode_command(r: &mut Reader<'_>) -> Result<Command, DecodeError> {
    let id = CommandId(r.str()?);
    let space = SpaceId(r.str()?);
    let entity = r.str()?;
    let row_id = r.str()?;
    let kind = match r.byte()? {
        0 => CommandKind::Create,
        1 => CommandKind::Converge,
        2 => CommandKind::Delete,
        other => return Err(DecodeError::BadTag(other)),
    };
    let before = decode_opt_image(r)?;
    let after = decode_opt_image(r)?;
    let at = Timestamp(r.u64()?);
    Ok(Command {
        id,
        space,
        entity,
        row_id,
        kind,
        before,
        after,
        at,
    })
}

fn decode_opt_image(r: &mut Reader<'_>) -> Result<Option<Image>, DecodeError> {
    match r.byte()? {
        0 => Ok(None),
        1 => Ok(Some(decode_image(r)?)),
        other => Err(DecodeError::BadTag(other)),
    }
}

fn decode_image(r: &mut Reader<'_>) -> Result<Image, DecodeError> {
    let n = r.usize_len()?;
    let mut img = Image::new();
    for _ in 0..n {
        let key = r.str()?;
        let value = decode_value(r)?;
        img.insert(key, value);
    }
    Ok(img)
}

fn decode_value(r: &mut Reader<'_>) -> Result<Value, DecodeError> {
    match r.byte()? {
        0 => Ok(Value::Null),
        1 => Ok(Value::Bool(r.byte()? != 0)),
        2 => Ok(Value::Int(r.u64()? as i64)),
        3 => Ok(Value::Text(r.str()?)),
        4 => Ok(Value::Id(r.str()?)),
        5 => {
            let mut h = [0u8; 16];
            h.copy_from_slice(r.take(16)?);
            Ok(Value::SealedRef(h))
        }
        6 => {
            let s = r.str()?;
            match crate::value::Decimal::parse(&s) {
                Some(d) if d.as_str() == s => Ok(Value::Decimal(d)),
                _ => Err(DecodeError::BadDecimal(s)),
            }
        }
        other => Err(DecodeError::BadTag(other)),
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        let end = self.at.checked_add(n).ok_or(DecodeError::BadLength)?;
        let out = self.b.get(self.at..end).ok_or(DecodeError::Truncated)?;
        self.at = end;
        Ok(out)
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        let mut n = [0u8; 8];
        n.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(n))
    }

    fn hash32(&mut self) -> Result<[u8; 32], DecodeError> {
        let mut h = [0u8; 32];
        h.copy_from_slice(self.take(32)?);
        Ok(h)
    }

    /// A chain position: `u64-BE idx ‖ hash32`.
    fn position(&mut self) -> Result<(u64, ChainHash), DecodeError> {
        let idx = self.u64()?;
        Ok((idx, ChainHash(self.hash32()?)))
    }

    /// A length or count. Refused before it is used to size anything: a
    /// hostile blob's first move is a huge prefix over a short buffer, and
    /// the honest answer is a typed error, not an allocation.
    fn usize_len(&mut self) -> Result<usize, DecodeError> {
        let n = self.u64()?;
        let n = usize::try_from(n).map_err(|_| DecodeError::BadLength)?;
        if n > self.b.len() - self.at {
            // Every element costs at least one byte, so a count larger than
            // the bytes remaining cannot be honest.
            return Err(DecodeError::BadLength);
        }
        Ok(n)
    }

    fn str(&mut self) -> Result<String, DecodeError> {
        let n = self.usize_len()?;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }
}
