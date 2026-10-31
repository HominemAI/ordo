//! Key custody: the provider seam, per-space keys, epochs, and rotation.
//!
//! # Why key material never leaves this trait
//!
//! The previous shape handed the master key out — `fn smk(&self) -> &[u8]`
//! — and every caller then derived and sealed for itself. Three failures
//! followed from that one signature, and all three were real:
//!
//! - **No injection seam.** Callers constructed `TestProvider` inline, so
//!   there was nothing to swap a real provider into. F0's own done-when
//!   says a provider returning plaintext must not be selectable outside
//!   unit tests; the inverse shipped.
//! - **No per-space custody.** With no space argument, one key served the
//!   deployment, which makes blind-index digests comparable across tenants
//!   and per-space crypto-erasure structurally impossible.
//! - **No release accounting.** A caller holding raw key material cannot
//!   be asked what it used the key for, so key-release breadth — a
//!   mandatory alert — had no substrate.
//!
//! So the trait exposes operations, never keys. Sealing, opening and
//! blind-index digesting all happen *inside* the provider, and a caller
//! that wants any of them must first take a [`KeyLease`] naming the space,
//! the purpose, and the correlation it is working under. A KMS-backed
//! provider can then keep the key inside the KMS, which is the whole point
//! of having a KMS.
//!
//! # Epochs are wrapped, not derived
//!
//! `DEK_n` is **random, generated at rotation, and stored wrapped under
//! the space's SMK**. It is emphatically not `KDF(SMK, n)`: a derived key
//! is a pure function of the master, so "destroy the retired epoch key"
//! cannot be expressed — the key comes back the moment anyone has the SMK.
//! Wrapping makes destruction a delete, which is what
//! `implementation.md`'s crypto-erasure and post-compromise-recovery
//! rulings actually require.
//!
//! Blind-index keys stay SMK-derived and epoch-independent, per the
//! settled rule: a DEK rotation must not touch any digest, or
//! `sealed indexed unique` admits duplicates across the rotation boundary
//! — during exactly the revocations that force rotation.

use std::collections::BTreeMap;
use std::sync::Mutex;

use ordo_fault::{FaultClass, StoreFault};

use crate::{hmac_sha256, SealError};

/// A space's identity for custody purposes. Keys are per space, so this
/// is not decoration: it is the isolation boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpaceId(pub String);

/// The **reserved null sentinel** a blind index digests in place of a null
/// value (`definition-language.md`, uniqueness — settled).
///
/// It is deliberately **not valid UTF-8**. The text path digests
/// `str::as_bytes`, which is UTF-8 by construction, so no text value can
/// ever produce these bytes — not the empty string, not the literal text
/// `"null"`, not any normalization of either. Unforgeability by
/// construction rather than by a chosen-string-nobody-would-type argument:
/// a sentinel a caller can spell is a sentinel a caller can claim.
///
/// Bumping the `v1` would move every null row's digest and is therefore a
/// re-index, exactly like an SMK rotation. It is frozen.
pub const NULL_SENTINEL: &[u8] = b"\xff\xffordo.blind.null.v1\xff\xff";

/// What a blind index digests: a normalized text value, or a null.
///
/// This is an enum rather than `&str` because the failure it exists to
/// stop was structural. With a `&str`-only API there is no way to *say*
/// null, so every caller holding a nullable field invented an answer, and
/// the answer they invented was to skip the digest and write SQL `NULL`
/// into the `$bidx` column — which makes `sealed indexed unique` constrain
/// nothing at all (Postgres `UNIQUE` permits unlimited NULLs) and makes
/// blind-index equality against a null match nothing, ever, silently.
/// Making nullness a variant means a caller must decide, and the compiler
/// asks.
///
/// `Text("")` and `Null` are **different values** with different digests —
/// the empty string is a value a field can hold, and collapsing the two
/// would let a writer address a null row by supplying `""`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlindValue<'a> {
    /// An ALREADY-normalized text value: `normalize(lower)` is a schema
    /// fact, applied before the HMAC and not by this crate.
    Text(&'a str),
    /// A null. Digests [`NULL_SENTINEL`]; see its disclosure note.
    Null,
}

impl<'a> BlindValue<'a> {
    /// The bytes actually HMAC'd. Text digests its own UTF-8; null
    /// digests the reserved sentinel.
    pub fn message(self) -> &'a [u8] {
        match self {
            BlindValue::Text(s) => s.as_bytes(),
            BlindValue::Null => NULL_SENTINEL,
        }
    }
}

impl<'a> From<Option<&'a str>> for BlindValue<'a> {
    fn from(v: Option<&'a str>) -> Self {
        match v {
            Some(s) => BlindValue::Text(s),
            None => BlindValue::Null,
        }
    }
}

/// What a sealed value is bound to, beyond its entity and field.
///
/// A closed set, so the kind of thing is part of the label and a row id
/// can never be mistaken for an object handle that spells the same text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor<'a> {
    /// The id of the row a column value lives on. A chain image of that
    /// value is bound to the same row, because it is a copy of the row's
    /// envelope.
    Row(&'a str),
    /// The handle of a stored object: its chunks, and the sealed `mime`,
    /// `size` and `integrity` on its reference row.
    Object(&'a str),
    /// The idempotency key a journalled response is filed under.
    Key(&'a str),
}

impl<'a> Anchor<'a> {
    /// The text the associated data carries for this anchor, and the text
    /// the sealed payload store keeps beside a payload so that it can be
    /// opened later.
    pub fn label(self) -> String {
        match self {
            Anchor::Row(id) => format!("row:{id}"),
            Anchor::Object(handle) => format!("obj:{handle}"),
            Anchor::Key(key) => format!("key:{key}"),
        }
    }

    /// The anchor a [`Self::label`] names. `None` for text that is not one.
    pub fn parse(label: &'a str) -> Option<Anchor<'a>> {
        if let Some(id) = label.strip_prefix("row:") {
            Some(Anchor::Row(id))
        } else if let Some(handle) = label.strip_prefix("obj:") {
            Some(Anchor::Object(handle))
        } else {
            label.strip_prefix("key:").map(Anchor::Key)
        }
    }
}

/// Which DEK generation sealed a payload. One byte, because it leads every
/// envelope and the envelope format is frozen; a deployment that exhausts
/// 255 epochs has a re-tag migration, not a design change.
pub type Epoch = u8;

/// The opaque handle a command image carries in place of a sealed value —
/// 128 bits, minted per sealed image write, resolving through the sealed
/// payload store ([`crate::sealed`]).
pub type SealedHandle = [u8; 16];

/// The reserved pseudo-entity the handle derivation is domain-separated
/// under. `$`-prefixed, so no schema can declare an entity that shares its
/// key: a handle's derivation key is not any real field's blind-index key.
pub const SEALED_HANDLE_DOMAIN: &str = "$sealed";

/// The opaque handle a blob is addressed by — 128 bits, minted per upload,
/// naming a row in `$blobref` and an object in the space's resolved store.
///
/// Deliberately the same width and the same derivation shape as
/// [`SealedHandle`], and deliberately **not the same type alias**: they are
/// drawn from disjoint keyed streams ([`BLOB_HANDLE_DOMAIN`] versus
/// [`SEALED_HANDLE_DOMAIN`]) and resolve through different stores, so a
/// signature that took either would be one refactor away from resolving a
/// payload handle against the bucket.
pub type BlobHandle = [u8; 16];

/// The reserved pseudo-entity blob handles are domain-separated under.
/// `$`-prefixed for [`SEALED_HANDLE_DOMAIN`]'s reason: no schema can
/// declare an entity that shares its key.
pub const BLOB_HANDLE_DOMAIN: &str = "$blob";

/// The derivation label's version. Bumping it changes nothing already
/// stored — handles are looked up, never recomputed — so unlike
/// [`NULL_SENTINEL`] this is not frozen.
pub const SEALED_HANDLE_LABEL: &str = "handle.v1";

/// The reserved pseudo-entity [`probe`] asks its question under. `$`-prefixed
/// for [`SEALED_HANDLE_DOMAIN`]'s reason: no schema can declare an entity
/// that shares its key, so the digest the probe computes addresses nothing.
pub const PROBE_DOMAIN: &str = "$custody";

/// **Can this provider actually open custody for `space`?**
///
/// One question, asked from two places that must not drift: `/health/deep`
/// before a replica reports ready, and `std.space.create`'s preflight before
/// it reserves a registry row (`bootstrap.md` §2 item 22 — *"unhealthy
/// provider ⇒ preflight rejection before the registry row exists"*).
///
/// # Why `blind_index` and not something cheaper
///
/// Because the cheap questions do not answer this one. `current_epoch` takes
/// the maximum epoch of the store's DEK rows and **unwraps nothing**; so does
/// `release`. A process booted with a mistyped `ORDO_MASTER_KEY` passes both,
/// reports healthy, and then fails every read of every sealed field — the
/// worst available shape, because by then it has been sent traffic.
///
/// `blind_index` is the cheapest question that genuinely unwraps: it derives
/// the per-field index key from the space's SMK, so a wrong master is a
/// `KeyError` here rather than at the first customer read. It takes no lease
/// and releases no DEK, which is what lets a readiness probe ask it.
/// `ordo-crypto/tests/master.rs`'s `a_wrong_master_is_invisible_to_the_presence_checks`
/// pins the distinction this rests on.
///
/// # Which space to ask about
///
/// One whose keys already exist. For readiness that is the control plane's
/// own space, never a tenant — a deployment pointed at a tenant nobody had
/// provisioned used to answer `not_ready`, letting one tenant's state decide
/// whether the whole deployment served anyone. For a tenant being created it
/// is the same space, because the tenant's own keys are what this call is
/// deciding whether to mint: asking about a space that does not exist yet
/// would answer about the wrong thing, or about nothing.
pub fn probe(keys: &dyn KeyProvider, space: &SpaceId) -> Result<(), KeyError> {
    keys.blind_index(space, PROBE_DOMAIN, "probe", BlindValue::Null)
        .map(|_| ())
}

/// Why a caller wants keys. Scoping matters because a rotation is the
/// moment the most keys are resident at once, held by the longest-running
/// process — and revocation *mandates* rotation, so an attacker who can
/// predict a departure can predict that window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Serving a read: opening payloads for a caller who passed the door.
    Read,
    /// Writing: sealing new values, and opening for converge recipes.
    Write,
    /// A re-seal worker moving rows from one epoch to the next. Needs two
    /// epochs live at once, which is why it is named separately.
    Reseal,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Read => "read",
            Purpose::Write => "write",
            Purpose::Reseal => "reseal",
        }
    }
}

/// A scoped, correlated grant of key use — `release(space, purpose,
/// correlation)` from the design.
///
/// It carries no key material. It is a capability the provider checks, and
/// the record that makes release countable: every seal and open names the
/// lease it ran under, so "how many spaces did this principal's keys touch
/// in an hour" is answerable rather than aspirational.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyLease {
    pub space: SpaceId,
    pub purpose: Purpose,
    /// The request or job this release belongs to. A release with no live
    /// correlation is refused by providers that track them.
    pub correlation: String,
}

/// Why a key custody call did not succeed, typed once.
///
/// The cause of a store fault is kept (`source()`), and every variant
/// answers [`KeyError::class`] with the shared [`FaultClass`], so a caller
/// never reads the words to decide what to do (AGENTS.md rule 9).
///
/// `#[non_exhaustive]`: a variant added later is decided by each caller
/// outside this crate, through its wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyError {
    /// No custody exists for this space.
    UnknownSpace,
    /// The epoch a payload names is not available — retired and destroyed,
    /// or never minted here. The store answered, and the epoch is not in
    /// its answer: this is the one certain "destroyed" case
    /// ([`KeyError::is_destroyed`]).
    UnknownEpoch(Epoch),
    /// The lease does not authorize this operation.
    NotLeased,
    /// The envelope did not open. Deliberately indistinguishable from any
    /// other sealing failure.
    Seal,
    /// The provider's backing store, or its configuration, failed. The
    /// fault carries its class: a full pool is [`FaultClass::Busy`],
    /// transient and never a statement about the key; a bad setting is
    /// [`FaultClass::Refused`]; anything else is [`FaultClass::Broken`].
    Store(StoreFault),
}

impl KeyError {
    /// A store fault of the given class, with the error that caused it.
    pub fn caused_by(
        class: FaultClass,
        reason: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> KeyError {
        KeyError::Store(StoreFault::caused_by(class, reason, source))
    }

    /// A store fault with no cause to keep: broken.
    pub fn broken(reason: impl Into<String>) -> KeyError {
        KeyError::Store(StoreFault::unavailable(reason))
    }

    /// A setting or an input the deployment owner must change; the same
    /// call gets the same answer.
    pub fn refused(reason: impl Into<String>) -> KeyError {
        KeyError::Store(StoreFault::of_class(FaultClass::Refused, reason))
    }

    /// A store fault of a given class with no cause kept.
    pub fn of_class(class: FaultClass, reason: impl Into<String>) -> KeyError {
        KeyError::Store(StoreFault::of_class(class, reason))
    }

    /// What a caller can do about this. A store fault answers with its own
    /// class; an envelope that failed to open with its key present is
    /// [`FaultClass::Unreadable`]; the rest are refusals (the key or the
    /// lease, not the service).
    pub fn class(&self) -> FaultClass {
        match self {
            KeyError::Store(f) => f.class(),
            KeyError::Seal => FaultClass::Unreadable,
            _ => FaultClass::Refused,
        }
    }

    /// Did the backing store fail (busy or broken), rather than the key or
    /// the envelope? Such a fault says nothing about whether a key exists,
    /// so it must never be read as a destroyed key.
    pub fn is_store_fault(&self) -> bool {
        matches!(self, KeyError::Store(_))
    }

    /// Was the backing store busy rather than broken?
    pub fn is_busy(&self) -> bool {
        self.class().is_busy()
    }

    /// Is the key certainly gone: the store answered and the epoch is not
    /// there. Never true of a store fault, and never of an envelope that
    /// merely failed to open.
    pub fn is_destroyed(&self) -> bool {
        matches!(self, KeyError::UnknownEpoch(_))
    }

    /// Did an envelope fail to authenticate or decrypt. Said of the value,
    /// not of the key: the key may be present, so this is "unreadable,
    /// possibly tampered", never "destroyed".
    pub fn is_unreadable(&self) -> bool {
        matches!(self, KeyError::Seal)
    }
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::UnknownSpace => f.write_str("no key custody for that space"),
            KeyError::UnknownEpoch(e) => write!(f, "epoch {e} is not available"),
            KeyError::NotLeased => f.write_str("no key release authorizes this"),
            KeyError::Seal => f.write_str("sealed payload did not open"),
            KeyError::Store(m) => write!(f, "key store: {m}"),
        }
    }
}

impl std::error::Error for KeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KeyError::Store(f) => Some(f),
            _ => None,
        }
    }
}

impl From<SealError> for KeyError {
    fn from(_: SealError) -> Self {
        KeyError::Seal
    }
}

/// Key custody. Implementations hold key material and never return it.
pub trait KeyProvider: Send + Sync {
    /// Take a scoped release. Providers that enforce correlation refuse a
    /// lease with no live job or request behind it.
    fn release(
        &self,
        space: &SpaceId,
        purpose: Purpose,
        correlation: &str,
    ) -> Result<KeyLease, KeyError>;

    /// Seal under the space's current epoch, bound to the thing the value
    /// belongs to. The envelope's leading epoch tag is what lets a later
    /// read find the right key.
    ///
    /// The associated data names the entity, the field **and** the
    /// [`Anchor`]: the row a column value lives on, the object a file's
    /// attribute describes, the key a journal entry is filed under. A value
    /// lifted out of one anchor does not open at another, so somebody who
    /// holds the database and no key cannot rearrange sealed values.
    ///
    /// A required method, for [`Self::destroy_space`]'s reason: a wrapping
    /// provider that inherited a default could forget to forward it.
    fn seal_at(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, KeyError>;

    /// Open a payload under whatever epoch it names. Reads accept any live
    /// epoch; writes always seal under the newest. The anchor must be the
    /// one the value was sealed at.
    fn open_at(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        sealed: &[u8],
    ) -> Result<Vec<u8>, KeyError>;

    /// The digest stored in `field$bidx`. Computed inside the provider, so
    /// the blind-index key never leaves it — which is what lets the digest
    /// be computed at the door, after the access check, rather than baked
    /// into a compiled artifact.
    ///
    /// The value arrives ALREADY normalized: `normalize(lower)` is a
    /// schema fact, not a crypto one.
    ///
    /// A null is [`BlindValue::Null`], never a skipped digest: the column
    /// carries [`NULL_SENTINEL`]'s HMAC so uniqueness and equality both
    /// hold over nulls with no special case. The disclosure that buys —
    /// nullness is readable without a key — is on [`NULL_SENTINEL`].
    fn blind_index(
        &self,
        space: &SpaceId,
        entity: &str,
        field: &str,
        value: BlindValue<'_>,
    ) -> Result<[u8; 32], KeyError>;

    /// Mint the opaque handle a command image carries in place of a sealed
    /// value — **random, never content-derived**, salted per space and per
    /// epoch *in the derivation*.
    ///
    /// # The derivation
    ///
    /// ```text
    /// seed   = 32 fresh CSPRNG bytes, drawn per call and discarded
    /// salt   = per-space key material, labelled ("$sealed", "handle.v1:<epoch>")
    /// handle = HMAC(salt, seed)[..16]
    /// ```
    ///
    /// The plaintext is not an input. Neither is the ciphertext, the
    /// nonce, the row, the field's previous value, or a counter — the only
    /// input that varies is the seed, and the seed is thrown away. Two
    /// writes of the same plaintext therefore produce two different
    /// handles, always, which is the conformance requirement §7 states.
    ///
    /// # What the salt buys, given the output is random either way
    ///
    /// Three things, and none of them is achievable by a bare
    /// `random_bytes(16)`:
    ///
    /// 1. **The RNG's failure domain stops at the space boundary.** A
    ///    CSPRNG that repeats — a restored VM snapshot, two replicas from
    ///    one image, a seeded container, the Debian OpenSSL shape of
    ///    accident — makes bare random handles *collide across spaces*,
    ///    and a matching handle in two tenants' plaintext envelopes is
    ///    precisely the cross-tenant equality signal the random handle
    ///    exists to prevent. Keyed per space, an identical seed still
    ///    yields unrelated handles, because the space's own key material
    ///    differs. A repeat can then only land inside one space-epoch,
    ///    where it is a primary-key violation the store reports rather
    ///    than a silent equality leak.
    /// 2. **Resolution is scoped by construction, not by a `WHERE`
    ///    clause.** A handle is drawn from a stream keyed by (space,
    ///    epoch), so handles minted in different spaces are values from
    ///    different keyed permutations. The store's `("$space", "ref")`
    ///    primary key enforces the same thing at the storage layer; two
    ///    independent mechanisms agree, which is the arrangement the
    ///    append lock and `(space, idx)` already use for the chain.
    /// 3. **The stored value stays metadata-free.** The salt is in the
    ///    *derivation*, so the 16 bytes that land in the command log's
    ///    **plaintext** envelope carry no space tag, no epoch tag and no
    ///    ordering. Had the epoch been salted in by concatenation — the
    ///    obvious cheaper spelling, `[epoch][15 random]` — every sealed
    ///    image in the log would advertise which epoch sealed it, and
    ///    since rotation is *mandatory on membership revocation*, reading
    ///    the epoch byte down the chain would give anyone holding the log
    ///    and no keys a **revocation timeline**. That is the same class of
    ///    leak §7 rejects the content hash for, and it survives
    ///    crypto-erasure for the same reason.
    ///
    /// # Why it routes through `blind_index`
    ///
    /// [`Self::blind_index`] *is* this trait's per-space keyed PRF, and it
    /// is the only one that never returns key material. Applying it to
    /// fresh randomness rather than to a value is what turns a digest into
    /// a handle. Reusing it means a wrapping provider (metering, a KMS)
    /// mints handles under its own custody for free, with no method to
    /// forget to forward. The `$sealed` domain keeps the handle key
    /// disjoint from every real field's index key.
    ///
    /// `epoch` is passed in rather than read from a payload: a signature
    /// that took bytes would be one refactor away from being
    /// content-derived, and a reviewer could not tell by looking.
    fn mint_sealed_ref(&self, lease: &KeyLease, epoch: Epoch) -> Result<SealedHandle, KeyError> {
        // Minting is part of writing an image; a read release does not do
        // it, for the same reason a read release does not seal.
        if lease.purpose == Purpose::Read {
            return Err(KeyError::NotLeased);
        }
        let seed = crate::hex(&random32());
        let tag = self.blind_index(
            &lease.space,
            SEALED_HANDLE_DOMAIN,
            &format!("{SEALED_HANDLE_LABEL}:{epoch}"),
            BlindValue::Text(&seed),
        )?;
        let mut handle = [0u8; 16];
        handle.copy_from_slice(&tag[..16]);
        Ok(handle)
    }

    /// Mint the opaque handle a **blob** is addressed by — the same
    /// derivation as [`Self::mint_sealed_ref`], one domain over.
    ///
    /// ```text
    /// seed   = 32 fresh CSPRNG bytes, drawn per upload and discarded
    /// salt   = per-space key material, labelled ("$blob", "handle.v1:<epoch>")
    /// handle = HMAC(salt, seed)[..16]
    /// ```
    ///
    /// # Never content-addressed, and this is a correction being kept
    ///
    /// Not the bytes, not the ciphertext, not the mime type, not the size,
    /// not the row, not a counter. [`crate::sealed`] and
    /// `ordo_commandlog::Value` both record that the sealed handle *was*
    /// once specified as a digest of the ciphertext and that the
    /// specification was withdrawn, because it makes a plaintext index an
    /// unsalted permanent equality oracle that **survives
    /// crypto-erasure**. Content-addressing a blob adopts the withdrawn
    /// spelling in a new place, and the leak is strictly worse there: a
    /// column holds a name or an amount, a blob holds a document, and a
    /// content hash in an object key answers *"do these two tenants hold
    /// the same file?"* to anyone with bucket-level read, forever.
    ///
    /// It is also closed by the type system for a sealed field: the image
    /// of a sealed field is [`SealedHandle`] by rule, and there is no
    /// legal encoding of a content digest in that slot. Content addressing
    /// would therefore mean two identities per object — one in the bucket,
    /// one in the log — and the mapping between them is itself the oracle.
    ///
    /// What content addressing was for is kept in both halves:
    /// **deduplication** is refused, exactly as [`crate::sealed`] refuses
    /// it (dedup *is* the oracle: two images naming one row would prove
    /// the two writes equal, whatever the handles looked like), and
    /// **integrity** is kept as a *sealed* digest on the reference row, so
    /// the platform can verify a fetched object and nobody without the DEK
    /// can compare two of them.
    ///
    /// # Why a separate domain rather than reusing `$sealed`
    ///
    /// [`SEALED_HANDLE_DOMAIN`] keys the handles a *command image* carries;
    /// [`BLOB_HANDLE_DOMAIN`] keys the handles an *object store* is
    /// addressed by. Disjoint keys mean a blob handle can never resolve as
    /// a payload handle and vice versa — the two namespaces cannot be
    /// confused into each other by a caller who learned one of them, and
    /// the confusion would otherwise be a cross-store read.
    ///
    /// Defaulted for [`Self::mint_sealed_ref`]'s reason: a wrapping
    /// provider (metering, a KMS front) mints blob handles under its own
    /// custody for free, with no method to forget to forward.
    fn mint_blob_ref(&self, lease: &KeyLease, epoch: Epoch) -> Result<BlobHandle, KeyError> {
        if lease.purpose == Purpose::Read {
            return Err(KeyError::NotLeased);
        }
        let seed = crate::hex(&random32());
        let tag = self.blind_index(
            &lease.space,
            BLOB_HANDLE_DOMAIN,
            &format!("{SEALED_HANDLE_LABEL}:{epoch}"),
            BlindValue::Text(&seed),
        )?;
        let mut handle = [0u8; 16];
        handle.copy_from_slice(&tag[..16]);
        Ok(handle)
    }

    /// Seal one chunk of a blob under the chunked STREAM construction
    /// ([`crate::stream`]).
    ///
    /// The caller carries the nonce base and the counter because they are
    /// properties of the **object**, not of the key, and the object's
    /// header holds them in plaintext — nonces are public. What does not
    /// leave this trait is the DEK, which is the whole point.
    ///
    /// `epoch` is passed rather than read from the current one, so every
    /// chunk of one object seals under one key even if a rotation lands
    /// mid-upload. A signature that resolved the epoch per chunk would
    /// produce an object whose chunks disagreed about which key opens
    /// them, and it would do it only under the exact conditions —
    /// a rotation, which revocation makes mandatory — where nobody is
    /// watching.
    ///
    /// # A required method, not a defaulted one
    ///
    /// [`Self::destroy_space`]'s argument, unchanged: a default cannot be
    /// written without key material, so it would have to fail — and a
    /// wrapping provider (metering, a KMS front) that inherited a failing
    /// default would be a deployment whose uploads break only for blobs,
    /// only above the inline cap. Required means the compiler names every
    /// provider that has to answer.
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        epoch: Epoch,
        base: &crate::stream::NonceBase,
        counter: u32,
        last: bool,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, KeyError>;

    /// Open one chunk. Reordering, truncation, a moved column and a
    /// spliced object all fail here, and all fail the same way — the
    /// crate's standing rule that a sealing failure is never
    /// explanatory.
    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        epoch: Epoch,
        base: &crate::stream::NonceBase,
        counter: u32,
        last: bool,
        chunk: &[u8],
    ) -> Result<Vec<u8>, KeyError>;

    /// The epoch new seals use.
    fn current_epoch(&self, space: &SpaceId) -> Result<Epoch, KeyError>;

    /// Mint the next epoch and make it current. Rotation is **mandatory on
    /// membership revocation**, so a provider that cannot do this is not a
    /// provider — the trait requires it rather than offering it.
    ///
    /// Rotation does not re-seal: old epochs stay live and readable until
    /// a re-seal job reports zero remaining references, at which point the
    /// epoch may be retired and (subject to the undo horizon) destroyed.
    fn rotate(&self, space: &SpaceId, reason: &str) -> Result<Epoch, KeyError>;

    /// Mint custody for a space that has none: its space master key, and
    /// epoch 1's data key under it.
    ///
    /// The first act of provisioning a space, and the reason it is first:
    /// a row written into a space with no DEK is a row whose sealed fields
    /// cannot be written and whose blind indexes cannot be computed, so
    /// there is no order in which rows may precede custody.
    ///
    /// **Idempotent.** A space that already has an SMK keeps the one it
    /// has and this answers `Ok`. Minting a second one would strand every
    /// payload sealed under the first, which is the failure a provisioning
    /// retry after a crash would otherwise cause every time.
    ///
    /// # A required method, not a defaulted one
    ///
    /// [`Self::destroy_space`]'s argument, in the other direction: a
    /// wrapping provider (metering, a KMS front) that inherited a refusing
    /// default would be a deployment that cannot take a second tenant, and
    /// would discover it at the first one it tried to take. Required means
    /// the compiler names every provider that has to answer.
    fn create_space(&self, space: &SpaceId) -> Result<(), KeyError>;

    /// Destroy a retired epoch's key. This is the narrow erasure
    /// primitive: it kills exactly what is still tagged with that epoch.
    ///
    /// Callers must not reach this while any command image inside the
    /// space's undo horizon names the epoch — the platform guarantees undo
    /// for that window, and destroying the key takes history out from
    /// under it. Enforcing the horizon needs the log, so the provider
    /// takes the instruction and the caller owes the check.
    fn destroy_epoch(&self, space: &SpaceId, epoch: Epoch) -> Result<(), KeyError>;

    /// Destroy **every** piece of key material a space has: all its DEKs,
    /// current one included, and the space master key under them.
    ///
    /// This is the erasure primitive space deletion stands on, and it is
    /// deliberately not expressible as a loop over [`Self::destroy_epoch`].
    /// That method refuses the current epoch on purpose — destroying the
    /// key new writes are sealing under is a bug in every context except
    /// this one, and there is no ordering of rotations that reaches "no
    /// live epoch" through it, because rotating to escape an epoch mints
    /// another one.
    ///
    /// # Why the SMK goes too, and why that is the point
    ///
    /// Dropping the DEKs alone would leave every field envelope
    /// unopenable, which is what "crypto-erasure" usually means. It would
    /// not finish the job, because the **blind-index** keys are derived
    /// from the SMK and are epoch-independent by design (that is what lets
    /// rotation leave a digest where it is). A surviving SMK therefore
    /// leaves anyone holding a backup able to recompute
    /// `HMAC(k_field, guess)` and confirm guesses against the `$bidx`
    /// columns in it — a working equality oracle over a space that was
    /// supposed to be gone. Erasure that stops at the DEKs answers *what
    /// was the value* and leaves *was the value X* answerable, which is
    /// the weaker half of the same question.
    ///
    /// # A required method, not a defaulted one
    ///
    /// Deliberately: a default would silently give every wrapping provider
    /// (metering, a KMS front) the wrong behaviour, and a wrapper that
    /// forgets to forward this one is a deletion that reports success and
    /// erases nothing. Required means the compiler names every provider
    /// that has to answer.
    ///
    /// Idempotent: a space with no custody is already erased, and saying
    /// so twice is not an error.
    fn destroy_space(&self, space: &SpaceId) -> Result<(), KeyError>;
}

// ── the wrapped-key store ────────────────────────────────────────────────

/// Where a provider keeps its wrapped keys.
///
/// Separate from the provider so a deployment's *storage* choice and its
/// *custody* choice are independent: `LocalKeyProvider` over Postgres is
/// the self-host default, and the same provider over an in-memory store is
/// what tests use. A KMS-backed provider needs neither.
pub trait WrappedKeyStore: Send + Sync {
    /// The space's wrapped SMK, if custody exists.
    fn smk(&self, space: &SpaceId) -> Result<Option<Vec<u8>>, KeyError>;
    fn put_smk(&self, space: &SpaceId, wrapped: &[u8]) -> Result<(), KeyError>;
    /// Wrapped DEKs by epoch, ascending.
    fn deks(&self, space: &SpaceId) -> Result<Vec<(Epoch, Vec<u8>)>, KeyError>;
    fn put_dek(&self, space: &SpaceId, epoch: Epoch, wrapped: &[u8]) -> Result<(), KeyError>;
    fn drop_dek(&self, space: &SpaceId, epoch: Epoch) -> Result<(), KeyError>;
    /// Remove the space's SMK and every DEK under it, in one act.
    ///
    /// One method rather than "drop every DEK, then drop the SMK" because
    /// a store that fails halfway through the loop leaves a space whose
    /// erasure is *partial*, and partial crypto-erasure is
    /// indistinguishable from none for the field it did not reach. A store
    /// backed by a database makes this one statement pair in one
    /// transaction; the in-memory one takes both locks.
    fn drop_space(&self, space: &SpaceId) -> Result<(), KeyError>;
}

/// In-memory store — tests, and single-process embeddings.
#[derive(Default)]
pub struct MemoryKeyStore {
    smks: Mutex<BTreeMap<SpaceId, Vec<u8>>>,
    deks: Mutex<BTreeMap<(SpaceId, Epoch), Vec<u8>>>,
}

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl WrappedKeyStore for MemoryKeyStore {
    fn smk(&self, space: &SpaceId) -> Result<Option<Vec<u8>>, KeyError> {
        Ok(self.smks.lock().expect("key store").get(space).cloned())
    }
    fn put_smk(&self, space: &SpaceId, wrapped: &[u8]) -> Result<(), KeyError> {
        self.smks
            .lock()
            .expect("key store")
            .insert(space.clone(), wrapped.to_vec());
        Ok(())
    }
    fn deks(&self, space: &SpaceId) -> Result<Vec<(Epoch, Vec<u8>)>, KeyError> {
        Ok(self
            .deks
            .lock()
            .expect("key store")
            .iter()
            .filter(|((s, _), _)| s == space)
            .map(|((_, e), v)| (*e, v.clone()))
            .collect())
    }
    fn put_dek(&self, space: &SpaceId, epoch: Epoch, wrapped: &[u8]) -> Result<(), KeyError> {
        self.deks
            .lock()
            .expect("key store")
            .insert((space.clone(), epoch), wrapped.to_vec());
        Ok(())
    }
    fn drop_dek(&self, space: &SpaceId, epoch: Epoch) -> Result<(), KeyError> {
        self.deks
            .lock()
            .expect("key store")
            .remove(&(space.clone(), epoch));
        Ok(())
    }
    fn drop_space(&self, space: &SpaceId) -> Result<(), KeyError> {
        // Both locks held across both removals, in the declaration order
        // of the fields, so no reader can observe a space whose DEKs are
        // gone and whose SMK is not — the half-erased state that would
        // read as "erased" and still answer a blind-index guess.
        let mut smks = self.smks.lock().expect("key store");
        let mut deks = self.deks.lock().expect("key store");
        deks.retain(|(s, _), _| s != space);
        smks.remove(space);
        Ok(())
    }
}

// ── the local provider ───────────────────────────────────────────────────

/// The self-host default: a deployment master key from the environment or
/// a file, wrapping per-space SMKs and per-epoch DEKs in whatever
/// [`WrappedKeyStore`] the deployment gives it.
///
/// **A first-class implementation, not a stub** (`implementation.md`,
/// settled) — requiring a cloud KMS to run a hobby app is exactly the
/// self-host-is-second-class failure the platform exists to avoid. What it
/// does not do is pretend: it cannot protect keys from someone holding the
/// machine, and that is stated on the consent page rather than softened.
/// Moving to a KMS later is a provider swap plus a re-wrap, never a
/// migration.
pub struct LocalKeyProvider {
    master: [u8; 32],
    store: Box<dyn WrappedKeyStore>,
}

impl LocalKeyProvider {
    /// Derive the deployment master key from secret material. Hashed, so a
    /// short secret still fills the key rather than being zero-padded.
    pub fn new(secret: &[u8], store: Box<dyn WrappedKeyStore>) -> Self {
        Self::from_master(crate::master::derive_from_secret(secret), store)
    }

    /// Build over the master key **itself**, already 32 bytes.
    ///
    /// The constructor a [`crate::master::MasterKeySource`] feeds:
    /// a sealed file holds the master, not a passphrase-shaped secret, and
    /// running it through [`new`]'s HMAC again would produce a provider that
    /// opens nothing an `ORDO_MASTER_KEY` deployment wrote — with no error
    /// until the first read of a sealed field.
    ///
    /// [`new`]: LocalKeyProvider::new
    pub fn from_master(master: [u8; 32], store: Box<dyn WrappedKeyStore>) -> Self {
        LocalKeyProvider { master, store }
    }

    /// Read the master key from `ORDO_MASTER_KEY`. A deployment that has
    /// not set one is refused rather than defaulted — a default master key
    /// is the constant this whole redesign exists to delete.
    pub fn from_env(store: Box<dyn WrappedKeyStore>) -> Result<Self, KeyError> {
        let secret = std::env::var("ORDO_MASTER_KEY")
            .map_err(|_| KeyError::refused(crate::MASTER_KEY_UNSET))?;
        if secret.len() < 16 {
            return Err(KeyError::refused(crate::MASTER_KEY_SHORT));
        }
        Ok(Self::new(secret.as_bytes(), store))
    }

    /// Create custody with a CHOSEN space master key.
    ///
    /// For tests and for importing an existing space's custody. A
    /// deployment minting fresh custody wants [`create_space`]'s random
    /// key; this exists because the conformance fixture's blind-index
    /// digests are baked into its seed file, so its SMK has to be the same
    /// on every run or every seeded digest becomes unmatchable.
    pub fn create_space_with(&self, space: &SpaceId, smk: [u8; 32]) -> Result<(), KeyError> {
        if self.store.smk(space)?.is_some() {
            return Ok(());
        }
        self.store
            .put_smk(space, &self.wrap(&self.master, &smk, "smk", &space.0))?;
        let dek = random32();
        self.store.put_dek(
            space,
            1,
            &self.wrap(&smk, &dek, "dek", &format!("{}:1", space.0)),
        )?;
        Ok(())
    }

    fn wrap(&self, under: &[u8; 32], key: &[u8; 32], kind: &str, ctx: &str) -> Vec<u8> {
        crate::seal_with(under, 0, kind, ctx, key)
    }

    fn unwrap_key(
        &self,
        under: &[u8; 32],
        wrapped: &[u8],
        kind: &str,
        ctx: &str,
    ) -> Result<[u8; 32], KeyError> {
        let plain = crate::open_with(under, kind, ctx, wrapped)?;
        let mut k = [0u8; 32];
        if plain.len() != 32 {
            return Err(KeyError::Seal);
        }
        k.copy_from_slice(&plain);
        Ok(k)
    }

    fn smk_of(&self, space: &SpaceId) -> Result<[u8; 32], KeyError> {
        let wrapped = self.store.smk(space)?.ok_or(KeyError::UnknownSpace)?;
        self.unwrap_key(&self.master, &wrapped, "smk", &space.0)
    }

    fn dek_of(&self, space: &SpaceId, epoch: Epoch) -> Result<[u8; 32], KeyError> {
        let smk = self.smk_of(space)?;
        let wrapped = self
            .store
            .deks(space)?
            .into_iter()
            .find(|(e, _)| *e == epoch)
            .map(|(_, w)| w)
            .ok_or(KeyError::UnknownEpoch(epoch))?;
        self.unwrap_key(&smk, &wrapped, "dek", &format!("{}:{epoch}", space.0))
    }
}

pub(crate) fn random32() -> [u8; 32] {
    use chacha20poly1305::aead::{AeadCore, OsRng};
    use chacha20poly1305::XChaCha20Poly1305;
    // Two 24-byte nonces give 48 random bytes; take 32. Reusing the AEAD's
    // CSPRNG avoids a second RNG dependency for one call.
    let a = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let b = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut out = [0u8; 32];
    out[..24].copy_from_slice(&a);
    out[24..].copy_from_slice(&b[..8]);
    out
}

impl KeyProvider for LocalKeyProvider {
    /// A random SMK, and epoch 1's random DEK under it.
    /// [`LocalKeyProvider::create_space_with`] is the same act with a
    /// chosen key, and carries the idempotence.
    fn create_space(&self, space: &SpaceId) -> Result<(), KeyError> {
        self.create_space_with(space, random32())
    }

    fn release(
        &self,
        space: &SpaceId,
        purpose: Purpose,
        correlation: &str,
    ) -> Result<KeyLease, KeyError> {
        // Custody must exist; a release for an unknown space is refused
        // rather than silently creating keys, so a typo cannot mint a
        // tenant.
        self.store.smk(space)?.ok_or(KeyError::UnknownSpace)?;
        Ok(KeyLease {
            space: space.clone(),
            purpose,
            correlation: correlation.to_string(),
        })
    }

    fn seal_at(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        if lease.purpose == Purpose::Read {
            return Err(KeyError::NotLeased);
        }
        let epoch = self.current_epoch(&lease.space)?;
        let dek = self.dek_of(&lease.space, epoch)?;
        Ok(crate::seal_at_with(
            &dek, epoch, entity, field, anchor, plaintext,
        ))
    }

    fn open_at(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        sealed: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        let epoch = crate::envelope_epoch(sealed).ok_or(KeyError::Seal)?;
        let dek = self.dek_of(&lease.space, epoch)?;
        crate::open_at_with(&dek, entity, field, anchor, sealed).map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        epoch: Epoch,
        base: &crate::stream::NonceBase,
        counter: u32,
        last: bool,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        if lease.purpose == Purpose::Read {
            return Err(KeyError::NotLeased);
        }
        let dek = self.dek_of(&lease.space, epoch)?;
        Ok(crate::stream::seal_chunk_with(
            &dek, entity, field, anchor, base, counter, last, plaintext,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        lease: &KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        epoch: Epoch,
        base: &crate::stream::NonceBase,
        counter: u32,
        last: bool,
        chunk: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        let dek = self.dek_of(&lease.space, epoch)?;
        crate::stream::open_chunk_with(&dek, entity, field, anchor, base, counter, last, chunk)
            .map_err(Into::into)
    }

    fn blind_index(
        &self,
        space: &SpaceId,
        entity: &str,
        field: &str,
        value: BlindValue<'_>,
    ) -> Result<[u8; 32], KeyError> {
        // SMK-derived and epoch-independent, per the settled rule: a DEK
        // rotation must not move a single digest, or `sealed indexed
        // unique` admits duplicates across the boundary — during exactly
        // the revocations that force rotation.
        let smk = self.smk_of(space)?;
        let bik = hmac_sha256(&smk, format!("blind-index:{entity}:{field}").as_bytes());
        // Null digests the reserved sentinel under the SAME per-field BIK
        // as every value: the sentinel is domain-separated by entity and
        // field like everything else, so `customers.email`'s nulls are not
        // comparable with `people.email`'s.
        Ok(hmac_sha256(&bik, value.message()))
    }

    fn current_epoch(&self, space: &SpaceId) -> Result<Epoch, KeyError> {
        self.store
            .deks(space)?
            .into_iter()
            .map(|(e, _)| e)
            .max()
            .ok_or(KeyError::UnknownSpace)
    }

    fn rotate(&self, space: &SpaceId, _reason: &str) -> Result<Epoch, KeyError> {
        let smk = self.smk_of(space)?;
        let next = self
            .current_epoch(space)?
            .checked_add(1)
            .ok_or_else(|| KeyError::broken("epoch space exhausted; re-tag required"))?;
        let dek = random32();
        self.store.put_dek(
            space,
            next,
            &self.wrap(&smk, &dek, "dek", &format!("{}:{next}", space.0)),
        )?;
        Ok(next)
    }

    fn destroy_epoch(&self, space: &SpaceId, epoch: Epoch) -> Result<(), KeyError> {
        // Refusing to destroy the current epoch is not the undo-horizon
        // check (the caller owes that, since it needs the log) — it is the
        // one case the provider CAN see, and destroying the key you are
        // still writing under is never what anybody meant.
        if self.current_epoch(space)? == epoch {
            return Err(KeyError::refused(
                "refusing to destroy the epoch new writes still seal under",
            ));
        }
        self.store.drop_dek(space, epoch)
    }

    fn destroy_space(&self, space: &SpaceId) -> Result<(), KeyError> {
        // No current-epoch guard, and no `UnknownSpace` for a space that
        // has none: this is the one caller for whom "there is no live key
        // left" is the goal rather than the hazard, and for whom being
        // asked twice must be quiet.
        self.store.drop_space(space)
    }
}

/// Fixed-key custody for tests and the conformance fixtures. Its master
/// key is a constant on purpose: nothing it seals is secret, and looking
/// secret would be worse than being plain.
pub struct TestProvider(LocalKeyProvider);

impl Default for TestProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TestProvider {
    pub fn new() -> Self {
        TestProvider(LocalKeyProvider::new(
            b"ordo-test-smk-00",
            Box::new(MemoryKeyStore::new()),
        ))
    }

    /// A deterministic SMK **per space** — reproducible across runs, so
    /// the fixture's seeded `$bidx` values keep matching, and different
    /// per space, so the test provider does not quietly lose the isolation
    /// property real custody has. Determinism is the only thing tests need
    /// from it; sameness across tenants is not.
    fn test_smk(space: &SpaceId) -> [u8; 32] {
        hmac_sha256(b"ordo-test-smk-00", space.0.as_bytes())
    }

    /// The conformance space, created so tests need no setup.
    pub fn with_space(space: &str) -> Self {
        let p = Self::new();
        let sid = SpaceId(space.to_string());
        p.0.create_space_with(&sid, Self::test_smk(&sid))
            .expect("test custody");
        p
    }
}

impl KeyProvider for TestProvider {
    fn create_space(&self, space: &SpaceId) -> Result<(), KeyError> {
        self.0.create_space_with(space, Self::test_smk(space))
    }

    fn release(&self, s: &SpaceId, p: Purpose, c: &str) -> Result<KeyLease, KeyError> {
        // Tests seal into spaces they did not set up; create on demand
        // rather than making every test say so.
        self.0.create_space_with(s, Self::test_smk(s))?;
        self.0.release(s, p, c)
    }
    fn seal_at(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.0.seal_at(l, e, f, a, v)
    }
    fn open_at(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.0.open_at(l, e, f, a, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        ep: Epoch,
        b: &crate::stream::NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.0.seal_chunk(l, e, f, a, ep, b, c, last, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        ep: Epoch,
        b: &crate::stream::NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.0.open_chunk(l, e, f, a, ep, b, c, last, v)
    }
    fn blind_index(
        &self,
        s: &SpaceId,
        e: &str,
        f: &str,
        v: BlindValue<'_>,
    ) -> Result<[u8; 32], KeyError> {
        self.0.create_space_with(s, Self::test_smk(s))?;
        self.0.blind_index(s, e, f, v)
    }
    fn current_epoch(&self, s: &SpaceId) -> Result<Epoch, KeyError> {
        self.0.current_epoch(s)
    }
    fn rotate(&self, s: &SpaceId, r: &str) -> Result<Epoch, KeyError> {
        self.0.rotate(s, r)
    }
    fn destroy_epoch(&self, s: &SpaceId, e: Epoch) -> Result<(), KeyError> {
        self.0.destroy_epoch(s, e)
    }
    /// Forwarded like everything else — but note that this provider
    /// **re-creates custody on demand** in `release` and `blind_index`, so
    /// a test that erases a space and then asks it for a lease gets a
    /// brand-new SMK rather than a refusal. That is convenience for
    /// fixtures, not a custody model; a test asserting that erased data
    /// stays erased must open with the lease it held *before* the erasure,
    /// or use [`LocalKeyProvider`] directly.
    fn destroy_space(&self, s: &SpaceId) -> Result<(), KeyError> {
        self.0.destroy_space(s)
    }
}
