//! The security stream's canonical encoding and its hash chain.
//!
//! # Same machinery, different table — and a different domain
//!
//! The arithmetic is `ordo-commandlog`'s and is not reimplemented:
//! [`ChainHash`] is that crate's type, genesis is its 32 zero bytes,
//! `hash = SHA-256(prev ‖ body)` is its rule, and a failure is reported as
//! its [`TamperAt`] / [`TamperKind`] so one verifier vocabulary covers both
//! artifacts. What is *not* shared is the body layout, because the bodies
//! describe different things.
//!
//! Every security body starts with [`DOMAIN`]. That prefix is the reason a
//! command group's bytes and a security entry's bytes can never be confused
//! for one another: a stored `$commandlog` body replayed into
//! `$secevents` fails to decode instead of verifying, and the reverse
//! likewise. Domain separation costs sixteen bytes per entry and removes a
//! whole class of cross-grafting from the threat model — which matters more
//! here than usual, because the entire point of the second stream is that
//! it is not the first one.
//!
//! # Frozen layout
//!
//! ```text
//! str          := u64-BE byte-length ‖ UTF-8 bytes
//! opt-str      := 0x00 | 0x01 ‖ str
//! scope        := 0x00                      # deployment
//!               | 0x01 ‖ str space
//! subject      := 0x00                      # unverified: no payload at all
//!               | 0x01 ‖ str actor-attribution ‖ kind:u8
//!               | 0x02 ‖ str actor-attribution ‖ kind:u8
//!                      ‖ str token-kind ‖ str client ‖ str sid
//! kind         := tag:u8 ‖ payload          # see `encode_kind`
//! entry        := DOMAIN ‖ scope ‖ subject ‖ str op ‖ kind
//!                 ‖ u64-BE first_at ‖ u64-BE last_at ‖ u64-BE count
//!                 ‖ u64-BE n ‖ (str correlation)*
//!                 ‖ trailer?
//! trailer      := 0x01 ‖ str space          # about (plan-opslog.md D1b)
//! hash         := SHA-256( prev-hash (32 bytes) ‖ entry )
//! ```
//!
//! Deterministic by construction: no maps, no floats, decisions emitted in
//! `BTreeSet` order, all integers big-endian, all strings length-prefixed.
//! The layout is frozen and unversioned, exactly like the command chain's —
//! anything that does not decode is corruption, never an older format.
//!
//! # The one lawful way it changes
//!
//! "Frozen" means **no tag is ever re-used and no existing tag's payload
//! ever changes shape**. A *new* kind tag may be appended to the end of the
//! set, because every byte a previous binary wrote still decodes to exactly
//! what it decoded to before. The cost is one-directional and worth stating:
//! a *newer* entry read by an *older* binary is `BadTag`, which that binary
//! correctly reports as corruption rather than skipping. So a tag addition
//! is a forward step the whole fleet has to take before the first entry
//! carrying it is written, and it is the same rule a new
//! [`RecordedDecision`] follows. Tag 13
//! ([`EventKind::EventsDropped`](crate::event::EventKind::EventsDropped))
//! is the first one added this way.
//!
//! # Subject tag 2: a deployment token's facts
//!
//! `plan-identity-service.md` section 7 asks the door's record to carry a
//! deployment token's `kind`, `client` and `sid` beside `sub`. They belong
//! to the subject, so they travel in it, under a new subject tag added the
//! lawful way above: tags 0 and 1 keep their payloads, so every entry
//! written before the field existed renders the same bytes and hashes to
//! the same place, and an entry made without a deployment token still
//! renders tag 1. Only a subject that carries
//! [`TokenFacts`](crate::event::TokenFacts) renders tag 2. Because the
//! facts sit in the hashed body, changing any of them breaks the chain at
//! that entry. A binary older than the tag reads tag 2 as `BadTag`, the
//! same one-directional cost every new tag pays.
//!
//! # The trailer: which space a deployment entry was about
//!
//! `plan-opslog.md` §1.2 requires the deployment stream to name the space a
//! stranger probed, and §0.2 verified the defect: `Observation` carried it,
//! `route` consumed it to choose a scope, and the entry dropped it on the
//! floor. The fix is a field,
//! [`StreamEntry::about`](crate::event::StreamEntry::about), and it cannot
//! be added the way an ordinary field would be: the layout above is frozen,
//! and a byte added unconditionally to every entry's rendering moves the
//! hash of every entry already written, voiding every stored chain and
//! every signed checkpoint over it.
//!
//! So `about` is encoded **by presence, not by value**, the exact technique
//! `ordo_commandlog::chain` uses for `CommandGroup`'s trailer
//! (`owner_administration`, `restored_unverified`); see that module's doc
//! for the fuller argument, which transfers here unchanged:
//!
//! - An entry with no target (every entry stored before this field
//!   existed, and every space-scope entry from now on) renders exactly the
//!   bytes it rendered before `about` existed, and hashes to exactly the
//!   same place. `tests::a_deployment_entry_hashes_where_it_hashed` and
//!   `tests::a_space_entry_hashes_where_it_hashed` pin this as two numbers
//!   rather than an argument, computed against the encoder before this
//!   trailer existed.
//! - A flagged entry renders one additional byte for the tag, then the
//!   space as a length-prefixed string.
//! - Canonicity survives on the same rule: present tags are read in
//!   ascending order, and a decoder accepts a byte only if it is a known
//!   tag strictly greater than the last one seen. With one tag defined
//!   today that reduces to "at most one occurrence of `0x01`, and nothing
//!   else may follow it", but it is the same rule a second trailer tag
//!   would need, kept in place now rather than special-cased later.
//! - An explicit `0x00` is refused rather than accepted as "no target after
//!   all": `0x00` names no tag, so "no target" keeps exactly one spelling,
//!   which is silence.
//!
//! The one-directional cost is the one a new [`EventKind`] tag already
//! pays: an entry naming a target, read by a binary that predates this
//! field, refuses to decode rather than silently losing the space, which is
//! correct for an audit spine, per "The one lawful way it changes" above.
//!
//! `StreamEntry::about`'s own invariant, `Some` only when `scope` is
//! [`Scope::Deployment`], is not something the encoding can enforce
//! against a caller determined to break it; every producer that can name a
//! target goes through `Recorder::observe` (`crate::recorder`), which is
//! the one place that decides. [`encode_entry`] still asserts it in debug
//! builds, because a decoder that later read a violating entry back would
//! have no way to tell it was ever wrong.

use std::collections::BTreeSet;

use ordo_commandlog::{Actor, ChainHash, CorrelationId, SpaceId, TamperAt, TamperKind, Timestamp};
use sha2::{Digest, Sha256};

use crate::event::{
    subject_kind_from_tag, subject_kind_tag, subject_tag, EventKind, RecordedDecision, Scope,
    StreamEntry, Subject, TokenFacts, SUBJECT_WITH_TOKEN,
};

/// The domain prefix every security-stream body carries. Sixteen bytes,
/// chosen to be exactly as long as it needs to be to say what it is.
pub const DOMAIN: &[u8; 16] = b"ordo.secevent.v1";

/// An entry as stored: the entry plus its chain position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainedEntry {
    pub entry: StreamEntry,
    pub prev: ChainHash,
    pub hash: ChainHash,
}

/// `SHA-256(prev ‖ canonical-encoding(entry))`.
pub fn hash_entry(prev: ChainHash, entry: &StreamEntry) -> ChainHash {
    let mut hasher = Sha256::new();
    hasher.update(prev.0);
    hasher.update(encode_entry(entry));
    ChainHash(hasher.finalize().into())
}

/// Verify a slice against an explicit anchor — link first, then content.
///
/// `anchor` is the hash the first entry must point back at and `base_index`
/// is its position in the full stream, so a reported index still names a
/// real position after a lawful prune has removed the prefix. This is the
/// same shape as `ordo_commandlog::verify_chain_from` and for the same
/// reason: retention here drops whole prefixes ending at a checkpoint, and
/// verifying the surviving suffix as if it started at genesis would make a
/// lawful prune look identical to tampering.
pub fn verify_stream_from(
    scope: &Scope,
    entries: &[ChainedEntry],
    anchor: ChainHash,
    base_index: usize,
) -> Result<(), TamperAt> {
    let key = scope.key();
    let mut expected_prev = anchor;
    for (offset, e) in entries.iter().enumerate() {
        let index = base_index + offset;
        if e.prev != expected_prev {
            return Err(TamperAt {
                space: key,
                index,
                kind: TamperKind::BrokenLink,
            });
        }
        if hash_entry(e.prev, &e.entry) != e.hash {
            return Err(TamperAt {
                space: key,
                index,
                kind: TamperKind::HashMismatch,
            });
        }
        expected_prev = e.hash;
    }
    Ok(())
}

/// Verify from genesis. Correct only for a stream that has never been
/// pruned — see [`verify_stream_from`].
pub fn verify_stream(scope: &Scope, entries: &[ChainedEntry]) -> Result<(), TamperAt> {
    verify_stream_from(scope, entries, ChainHash::GENESIS, 0)
}

// ───────────────────────────── encoding ─────────────────────────────────

/// The canonical bytes an entry hashes as. Public because a durable store
/// persists exactly these: storing anything else would mean the thing
/// verified and the thing stored were two different artifacts.
pub fn encode_entry(entry: &StreamEntry) -> Vec<u8> {
    debug_assert!(
        entry.about.is_none() || entry.scope.is_deployment(),
        "a space-scope entry must not carry `about`: the scope already IS \
         the space, and a second field spelling the same fact is one that \
         can disagree with the first"
    );
    let mut out = Vec::with_capacity(96);
    out.extend_from_slice(DOMAIN);
    encode_scope(&mut out, &entry.scope);
    encode_subject(&mut out, &entry.subject);
    put_str(&mut out, &entry.op);
    encode_kind(&mut out, &entry.kind);
    out.extend_from_slice(&entry.first_at.0.to_be_bytes());
    out.extend_from_slice(&entry.last_at.0.to_be_bytes());
    out.extend_from_slice(&entry.count.to_be_bytes());
    out.extend_from_slice(&(entry.correlations.len() as u64).to_be_bytes());
    for c in &entry.correlations {
        put_str(&mut out, &c.0);
    }
    // Present only when set: see "The trailer" in the module doc. An entry
    // with no target renders exactly the bytes it rendered before `about`
    // existed, so every stored chain and every signed checkpoint over it
    // survives unchanged.
    if let Some(about) = &entry.about {
        out.push(TRAILER_ABOUT);
        put_str(&mut out, &about.0);
    }
    out
}

/// The trailer's one tag today: this entry names the space it was about.
/// Present only on a `Scope::Deployment` entry with `about` set. A future
/// tag would be `0x02`, read after this one; see the module doc.
const TRAILER_ABOUT: u8 = 0x01;

fn encode_scope(out: &mut Vec<u8>, scope: &Scope) {
    match scope {
        Scope::Deployment => out.push(0),
        Scope::Space(s) => {
            out.push(1);
            put_str(out, &s.0);
        }
    }
}

fn encode_subject(out: &mut Vec<u8>, subject: &Subject) {
    out.push(subject_tag(subject));
    if let Subject::Attributed { actor, kind, token } = subject {
        put_str(out, &actor.to_string());
        out.push(subject_kind_tag(*kind));
        if let Some(t) = token {
            put_str(out, &t.kind);
            put_str(out, &t.client);
            put_str(out, &t.sid);
        }
    }
}

fn encode_kind(out: &mut Vec<u8>, kind: &EventKind) {
    match kind {
        EventKind::Denied(decisions) => {
            out.push(0);
            out.extend_from_slice(&(decisions.len() as u64).to_be_bytes());
            for d in decisions {
                out.push(d.tag());
            }
        }
        EventKind::Authenticated => out.push(1),
        EventKind::AuthenticationFailed { reason } => {
            out.push(2);
            put_str(out, reason);
        }
        EventKind::StepUpDemanded => out.push(3),
        EventKind::StepUpSatisfied => out.push(4),
        EventKind::KeyReleased { purpose } => {
            out.push(5);
            put_str(out, purpose);
        }
        EventKind::MembershipSuspended => out.push(6),
        EventKind::MembershipReactivated => out.push(7),
        EventKind::CustodyAction { action } => {
            out.push(8);
            put_str(out, action);
        }
        EventKind::AuthenticatorLinked { added, vouched_by } => {
            out.push(9);
            put_str(out, added);
            match vouched_by {
                None => out.push(0),
                Some(v) => {
                    out.push(1);
                    put_str(out, v);
                }
            }
        }
        EventKind::AuthenticatorUnlinked { removed } => {
            out.push(10);
            put_str(out, removed);
        }
        EventKind::PrincipalOnboarded => out.push(11),
        EventKind::Suppressed { further } => {
            out.push(12);
            out.extend_from_slice(&further.to_be_bytes());
        }
        EventKind::EventsDropped { lost } => {
            out.push(13);
            out.extend_from_slice(&lost.to_be_bytes());
        }
        EventKind::SchemaMigration { migration, phase } => {
            out.push(14);
            put_str(out, migration);
            put_str(out, phase);
        }
        EventKind::Enrolment { act, detail } => {
            out.push(15);
            put_str(out, act);
            put_str(out, detail);
        }
        EventKind::SessionsEnded { reason } => {
            out.push(16);
            put_str(out, reason);
        }
        EventKind::Recovery { act, detail } => {
            out.push(17);
            put_str(out, act);
            put_str(out, detail);
        }
        EventKind::SecondFactor { act, detail } => {
            out.push(18);
            put_str(out, act);
            put_str(out, detail);
        }
        EventKind::SessionHandoff { act, detail } => {
            out.push(19);
            put_str(out, act);
            put_str(out, detail);
        }
        EventKind::UpstreamClaim { act, detail } => {
            out.push(20);
            put_str(out, act);
            put_str(out, detail);
        }
        EventKind::SealedUnreadable {
            entity,
            field,
            epoch,
        } => {
            out.push(21);
            put_str(out, entity);
            put_str(out, field);
            out.push(*epoch);
        }
        EventKind::ConsoleKey { act, detail } => {
            out.push(22);
            put_str(out, act);
            put_str(out, detail);
        }
        EventKind::DeploymentBooted { detail } => {
            out.push(23);
            put_str(out, detail);
        }
        EventKind::Deploy { act, detail } => {
            out.push(24);
            put_str(out, act);
            put_str(out, detail);
        }
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u64).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

// ───────────────────────────── decoding ─────────────────────────────────

/// Why a stored entry would not decode. Every variant means the bytes are
/// not something [`encode_entry`] produced — corruption or tampering, never
/// a lawful older format.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The body does not start with [`DOMAIN`] — including the case that
    /// matters most, a command-chain body stored in the security table.
    WrongDomain,
    Truncated,
    TrailingBytes,
    BadLength,
    BadUtf8,
    BadTag(u8),
    BadActor(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::WrongDomain => f.write_str("stored entry is not a security-stream body"),
            DecodeError::Truncated => f.write_str("stored entry ends mid-field"),
            DecodeError::TrailingBytes => f.write_str("stored entry has bytes after its end"),
            DecodeError::BadLength => f.write_str("stored entry has an impossible length prefix"),
            DecodeError::BadUtf8 => f.write_str("stored entry has a non-UTF-8 string"),
            DecodeError::BadTag(t) => write!(f, "stored entry has unknown tag {t}"),
            DecodeError::BadActor(s) => write!(f, "`{s}` is not a canonical actor attribution"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// The exact inverse of [`encode_entry`].
pub fn decode_entry(bytes: &[u8]) -> Result<StreamEntry, DecodeError> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(DOMAIN.len())? != DOMAIN.as_slice() {
        return Err(DecodeError::WrongDomain);
    }
    let scope = decode_scope(&mut r)?;
    let subject = decode_subject(&mut r)?;
    let op = r.str()?;
    let kind = decode_kind(&mut r)?;
    let first_at = Timestamp(r.u64()?);
    let last_at = Timestamp(r.u64()?);
    let count = r.u64()?;
    let n = r.usize_len()?;
    let mut correlations = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        correlations.push(CorrelationId(r.str()?));
    }
    // The trailer. Absent (the shape every entry written before `about`
    // existed takes) means no target; an explicit `0x00` is refused, so
    // "no target" keeps exactly one spelling. Present tags are read in
    // ascending order, the one order `encode_entry` ever produces: each
    // byte must be a known tag strictly greater than the last one seen, so
    // an unknown tag, a repeat, or a byte past the tags defined today all
    // fail the same way rather than needing their own case.
    let mut about = None;
    let mut last_tag = 0u8;
    while r.at != r.b.len() {
        let tag = r.byte()?;
        if tag <= last_tag {
            return Err(DecodeError::TrailingBytes);
        }
        match tag {
            TRAILER_ABOUT => about = Some(SpaceId(r.str()?)),
            _ => return Err(DecodeError::TrailingBytes),
        }
        last_tag = tag;
    }
    Ok(StreamEntry {
        scope,
        about,
        subject,
        op,
        kind,
        first_at,
        last_at,
        count,
        correlations,
    })
}

fn decode_scope(r: &mut Reader<'_>) -> Result<Scope, DecodeError> {
    match r.byte()? {
        0 => Ok(Scope::Deployment),
        1 => Ok(Scope::Space(SpaceId(r.str()?))),
        other => Err(DecodeError::BadTag(other)),
    }
}

fn decode_subject(r: &mut Reader<'_>) -> Result<Subject, DecodeError> {
    let subject_tag = r.byte()?;
    match subject_tag {
        0 => Ok(Subject::Unverified),
        1 | SUBJECT_WITH_TOKEN => {
            let rendered = r.str()?;
            let actor =
                Actor::parse(&rendered).map_err(|_| DecodeError::BadActor(rendered.clone()))?;
            let tag = r.byte()?;
            let kind = subject_kind_from_tag(tag).ok_or(DecodeError::BadTag(tag))?;
            let token = if subject_tag == SUBJECT_WITH_TOKEN {
                Some(TokenFacts {
                    kind: r.str()?,
                    client: r.str()?,
                    sid: r.str()?,
                })
            } else {
                None
            };
            Ok(Subject::Attributed { actor, kind, token })
        }
        other => Err(DecodeError::BadTag(other)),
    }
}

fn decode_kind(r: &mut Reader<'_>) -> Result<EventKind, DecodeError> {
    Ok(match r.byte()? {
        0 => {
            let n = r.usize_len()?;
            let mut set = BTreeSet::new();
            for _ in 0..n {
                let t = r.byte()?;
                set.insert(RecordedDecision::from_tag(t).ok_or(DecodeError::BadTag(t))?);
            }
            EventKind::Denied(set)
        }
        1 => EventKind::Authenticated,
        2 => EventKind::AuthenticationFailed { reason: r.str()? },
        3 => EventKind::StepUpDemanded,
        4 => EventKind::StepUpSatisfied,
        5 => EventKind::KeyReleased { purpose: r.str()? },
        6 => EventKind::MembershipSuspended,
        7 => EventKind::MembershipReactivated,
        8 => EventKind::CustodyAction { action: r.str()? },
        9 => {
            let added = r.str()?;
            let vouched_by = match r.byte()? {
                0 => None,
                1 => Some(r.str()?),
                other => return Err(DecodeError::BadTag(other)),
            };
            EventKind::AuthenticatorLinked { added, vouched_by }
        }
        10 => EventKind::AuthenticatorUnlinked { removed: r.str()? },
        11 => EventKind::PrincipalOnboarded,
        12 => EventKind::Suppressed { further: r.u64()? },
        13 => EventKind::EventsDropped { lost: r.u64()? },
        14 => {
            let migration = r.str()?;
            let phase = r.str()?;
            EventKind::SchemaMigration { migration, phase }
        }
        15 => {
            let act = r.str()?;
            let detail = r.str()?;
            EventKind::Enrolment { act, detail }
        }
        16 => EventKind::SessionsEnded { reason: r.str()? },
        17 => {
            let act = r.str()?;
            let detail = r.str()?;
            EventKind::Recovery { act, detail }
        }
        18 => {
            let act = r.str()?;
            let detail = r.str()?;
            EventKind::SecondFactor { act, detail }
        }
        19 => {
            let act = r.str()?;
            let detail = r.str()?;
            EventKind::SessionHandoff { act, detail }
        }
        20 => {
            let act = r.str()?;
            let detail = r.str()?;
            EventKind::UpstreamClaim { act, detail }
        }
        21 => {
            let entity = r.str()?;
            let field = r.str()?;
            let epoch = r.byte()?;
            EventKind::SealedUnreadable {
                entity,
                field,
                epoch,
            }
        }
        22 => {
            let act = r.str()?;
            let detail = r.str()?;
            EventKind::ConsoleKey { act, detail }
        }
        23 => EventKind::DeploymentBooted { detail: r.str()? },
        24 => {
            let act = r.str()?;
            let detail = r.str()?;
            EventKind::Deploy { act, detail }
        }
        other => return Err(DecodeError::BadTag(other)),
    })
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

    /// A length or count, refused before it sizes an allocation: a hostile
    /// blob's first move is a huge prefix over a short buffer.
    fn usize_len(&mut self) -> Result<usize, DecodeError> {
        let n = usize::try_from(self.u64()?).map_err(|_| DecodeError::BadLength)?;
        if n > self.b.len() - self.at {
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
