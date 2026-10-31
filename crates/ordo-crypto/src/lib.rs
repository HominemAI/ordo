//! F0/F1 slice: key custody behind a trait, and the blind index.
//!
//! `implementation.md` settles the design — key custody behind a
//! `KeyProvider` trait from day one, HMAC blind indexes domain-separated per
//! entity and field, independent of the DEK epoch. This crate implements
//! exactly the parts the data paths need: BIK derivation and digesting for
//! the read path, and F4's AEAD sealing (XChaCha20-Poly1305) for the write
//! path. The envelope is `[epoch:1][nonce:24][ct||tag]` — the epoch tag
//! ships before first customer data (implementation.md, settled) so DEK
//! rotation is a re-seal job, never a wire-format migration. Every seal
//! draws a fresh random nonce and the store never deduplicates; the AAD
//! binds a ciphertext to its entity and field, so a payload moved to
//! another column refuses to open. Blobs are sealed by the **chunked
//! STREAM** construction in [`stream`] instead of by [`seal_with`] — a
//! per-chunk nonce carrying a counter and a final-block flag, so an object
//! nobody can hold in memory still catches reordering and truncation, and
//! a range read is arithmetic rather than a whole-object decrypt. The compiler
//! still refuses every READ that would require opening a payload; `open`
//! exists for the write path's converge recipes and for disclosure at the
//! API edge, both host-side by design.
//!
//! # The handle and the payload store
//!
//! A command image never holds a sealed value; it holds a 16-byte handle
//! ([`KeyProvider::mint_sealed_ref`]) whose payload lives in the sealed
//! payload store ([`sealed`]). The handle is **random per sealed image
//! write** and salted per space and per epoch *in its derivation* — never
//! a hash of the payload, never a counter — because a content-derived ref
//! would make the command log's plaintext envelope an unsalted, permanent
//! equality index over every sealed field, one that survives
//! crypto-erasure (`security-logging-design.md` §7). The two invariants
//! that keep that true are stated in both places and enforced in both: a
//! fresh nonce per seal, here, and no deduplication, there.
//!
//! The blind index conceals values, never nullness. A null does **not**
//! get a null digest: a `sealed indexed unique` field HMACs a reserved
//! null sentinel ([`NULL_SENTINEL`]), so `HMAC(null)` is never undefined
//! and uniqueness needs no special case — the language's one equality
//! (`null == null` is true) reaches the digest column unchanged, which is
//! the settled rule in `definition-language.md` (uniqueness) and
//! `implementation.md` ("Disclosure: the null sentinel is readable",
//! settled).
//!
//! **What that discloses, stated rather than softened.** The sentinel is
//! the same digest for every null row in a space's entity+field, so it is
//! the single most common value in that column: **whoever holds the
//! database can tell which rows have a null in a sealed field, without any
//! key.** The blind index conceals values; it never claimed to conceal
//! nullness or cardinality. `implementation.md` puts that disclosure on
//! the shape summary next to the field, because "is this field set" is
//! genuinely sensitive for some fields (a diagnosis, a flag, a termination
//! date) and an author choosing `sealed indexed unique` should be choosing
//! it knowingly. Carrying it is the schema's job; stating it truthfully is
//! this crate's.

pub mod blobref;

/// The refusal for an unset `ORDO_MASTER_KEY`: it names the setting and the
/// form (audit 33, 2026-10-09).
pub(crate) const MASTER_KEY_UNSET: &str = "ORDO_MASTER_KEY is not set, and Ordo refuses to run on \
    a default key. Set ORDO_MASTER_KEY to a secret of at least 16 characters (for example the \
    output of `openssl rand -hex 32`), or ORDO_MASTER_KEY_FILE to a file that holds it";
/// The refusal for a short `ORDO_MASTER_KEY`.
pub(crate) const MASTER_KEY_SHORT: &str = "ORDO_MASTER_KEY is too short to be a key. Set it to a \
    secret of at least 16 characters (for example the output of `openssl rand -hex 32`)";

pub mod custody;
pub mod inline;
pub mod master;
pub mod sealed;
pub mod stream;

pub use blobref::{BlobRef, BLOB_REF_DDL, BLOB_REF_TABLE, BLOB_TOMB_DDL, BLOB_TOMB_TABLE};
pub use custody::{
    probe, Anchor, BlindValue, BlobHandle, Epoch, KeyError, KeyLease, KeyProvider,
    LocalKeyProvider, MemoryKeyStore, Purpose, SealedHandle, SpaceId, TestProvider,
    WrappedKeyStore, BLOB_HANDLE_DOMAIN, NULL_SENTINEL, PROBE_DOMAIN, SEALED_HANDLE_DOMAIN,
};
pub use inline::{Inline, InlineError};
pub use sealed::{SealedPayload, SEALED_PAYLOAD_DDL, SEALED_PAYLOAD_TABLE};
pub use stream::{chunk_count, plan_range, sealed_len, NonceBase, RangePlan, StreamHeader, CHUNK};

use sha2::{Digest, Sha256};

/// The raw derivations, kept as free functions behind the custody trait.
/// Nothing outside this crate should reach them: handing key material to
/// a caller is the failure [`custody::KeyProvider`] exists to prevent.
/// Sealing failures — deliberately unexplanatory: which byte failed is an
/// oracle, so every bad envelope is the same error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealError;

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("sealed payload did not open")
    }
}

fn aad(entity: &str, field: &str) -> Vec<u8> {
    format!("seal:{entity}:{field}").into_bytes()
}

/// The associated data of a value bound to an anchor: the column's label
/// with the anchor's kind and identity after it. The anchor comes last, so
/// nothing an anchor spells can be read as part of the entity or field.
pub(crate) fn aad_at(entity: &str, field: &str, anchor: Anchor<'_>) -> Vec<u8> {
    format!("seal:{entity}:{field}:{}", anchor.label()).into_bytes()
}

/// Fill `out` from the operating system's generator, the one sealing draws
/// its nonces from.
///
/// For identifiers that must not repeat across processes. Nothing drawn
/// here is a key, and nothing should use it as one.
pub fn fill_random(out: &mut [u8]) {
    use chacha20poly1305::aead::{AeadCore, OsRng};
    use chacha20poly1305::XChaCha20Poly1305;
    for chunk in out.chunks_mut(24) {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        chunk.copy_from_slice(&nonce[..chunk.len()]);
    }
}

/// Seal a stored value: `[epoch][24-byte fresh random nonce][ciphertext||tag]`,
/// AAD-bound to its entity, its field **and** its [`Anchor`]. This is the
/// seal every stored value uses; [`seal_with`] remains for wrapping keys,
/// which belong to no row.
pub fn seal_at_with(
    key: &[u8; 32],
    epoch: u8,
    entity: &str,
    field: &str,
    anchor: Anchor<'_>,
    plaintext: &[u8],
) -> Vec<u8> {
    seal_aad(key, epoch, &aad_at(entity, field, anchor), plaintext)
}

/// Seal a **key** for custody or for the master file: AAD-bound to a kind
/// and a context, not to a row. A stored value never comes through here;
/// see [`seal_at_with`].
pub fn seal_with(
    key: &[u8; 32],
    epoch: u8,
    entity: &str,
    field: &str,
    plaintext: &[u8],
) -> Vec<u8> {
    seal_aad(key, epoch, &aad(entity, field), plaintext)
}

fn seal_aad(key: &[u8; 32], epoch: u8, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
    use chacha20poly1305::XChaCha20Poly1305;
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ct = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("XChaCha20-Poly1305 encryption is total over byte inputs");
    let mut out = Vec::with_capacity(1 + 24 + ct.len());
    out.push(epoch);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// The epoch an envelope names, without opening it. A reader needs this
/// to ask custody for the right key.
pub fn envelope_epoch(sealed: &[u8]) -> Option<u8> {
    sealed.first().copied()
}

/// Open an envelope written by [`seal_with`]. Any malformation — short
/// envelope, unknown epoch's key, wrong AAD, flipped bit — is the same
/// [`SealError`].
pub fn open_with(
    key: &[u8; 32],
    entity: &str,
    field: &str,
    sealed: &[u8],
) -> Result<Vec<u8>, SealError> {
    open_aad(key, &aad(entity, field), sealed)
}

/// [`open_with`], for a value sealed by [`seal_at_with`].
pub fn open_at_with(
    key: &[u8; 32],
    entity: &str,
    field: &str,
    anchor: Anchor<'_>,
    sealed: &[u8],
) -> Result<Vec<u8>, SealError> {
    open_aad(key, &aad_at(entity, field, anchor), sealed)
}

fn open_aad(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SealError> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::XChaCha20Poly1305;
    if sealed.len() < 1 + 24 + 16 {
        return Err(SealError);
    }
    let (nonce, ct) = sealed[1..].split_at(24);
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(nonce.into(), Payload { msg: ct, aad })
        .map_err(|_| SealError)
}

/// RFC 2104 HMAC over SHA-256. Hand-rolled over the existing sha2 dep
/// rather than pulling a new crate for twenty lines.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let d = Sha256::digest(key);
        k[..32].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = {
        let mut h = Sha256::new();
        h.update(ipad);
        h.update(msg);
        h.finalize()
    };
    let mut h = Sha256::new();
    h.update(opad);
    h.update(inner);
    h.finalize().into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        // Key "Jefe", data "what do ya want for nothing?".
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn an_anchored_seal_opens_only_at_its_anchor() {
        use crate::custody::{KeyProvider, Purpose, SpaceId, TestProvider};
        let p = TestProvider::new();
        let s = SpaceId("s1".into());
        let w = p.release(&s, Purpose::Write, "t").unwrap();
        let sealed = p
            .seal_at(&w, "orders", "memo", Anchor::Row("ord_1"), b"gift")
            .unwrap();
        assert_eq!(
            p.open_at(&w, "orders", "memo", Anchor::Row("ord_1"), &sealed)
                .unwrap(),
            b"gift"
        );
        // Another row, the same text spelled as another kind of anchor,
        // another field, and the unanchored label all refuse.
        for (entity, field, anchor) in [
            ("orders", "memo", Anchor::Row("ord_2")),
            ("orders", "memo", Anchor::Object("ord_1")),
            ("orders", "memo", Anchor::Key("ord_1")),
            ("orders", "note", Anchor::Row("ord_1")),
            ("carts", "memo", Anchor::Row("ord_1")),
        ] {
            assert!(
                p.open_at(&w, entity, field, anchor, &sealed).is_err(),
                "{entity}.{field} {anchor:?} must not open"
            );
        }
    }

    /// A value sealed to a column alone (the label that predates anchors)
    /// does not open at any anchor, and an anchored one does not open
    /// unanchored.
    #[test]
    fn the_column_label_and_the_anchored_label_do_not_open_each_other() {
        let key = [7u8; 32];
        let bare = seal_with(&key, 1, "orders", "memo", b"gift");
        let anchored = seal_at_with(&key, 1, "orders", "memo", Anchor::Row("ord_1"), b"gift");
        assert!(open_at_with(&key, "orders", "memo", Anchor::Row("ord_1"), &bare).is_err());
        assert!(open_with(&key, "orders", "memo", &anchored).is_err());
        assert_eq!(
            open_at_with(&key, "orders", "memo", Anchor::Row("ord_1"), &anchored).unwrap(),
            b"gift"
        );
    }

    #[test]
    fn blind_index_is_domain_separated() {
        use crate::custody::{BlindValue, KeyProvider, SpaceId, TestProvider};
        let p = TestProvider::new();
        let s = SpaceId("s1".into());
        let v = BlindValue::Text("ada@example.com");
        let a = p.blind_index(&s, "customers", "email", v).unwrap();
        let b = p.blind_index(&s, "people", "email", v).unwrap();
        assert_ne!(a, b, "same value, different entity: different digest");
    }

    /// The reserved sentinel is unforgeable *by construction*: the text
    /// path digests `str::as_bytes`, which is UTF-8, and the sentinel is
    /// not. No argument about which string nobody would type is needed.
    #[test]
    fn the_null_sentinel_is_not_a_text_value() {
        // Through a runtime slice, not the literal: the compiler is happy
        // to tell us at compile time that this is invalid UTF-8, which is
        // the very fact under test and lints as a useless call.
        let bytes = NULL_SENTINEL.to_vec();
        assert!(
            std::str::from_utf8(&bytes).is_err(),
            "a sentinel a caller can spell is a sentinel a caller can claim"
        );
    }

    /// Null is a value with a digest, not a missing digest — and it is a
    /// DIFFERENT value from the empty string and from the text "null".
    #[test]
    fn null_digests_the_sentinel_and_collides_only_with_itself() {
        use crate::custody::{BlindValue, KeyProvider, SpaceId, TestProvider};
        let p = TestProvider::new();
        let s = SpaceId("s1".into());
        let d = |v| p.blind_index(&s, "customers", "email", v).unwrap();

        let null = d(BlindValue::Null);
        assert_eq!(
            null,
            d(BlindValue::Null),
            "two nulls collide: `null == null`"
        );
        assert_eq!(
            null,
            d(BlindValue::from(None)),
            "`Option::None` is the same null"
        );
        for spelling in ["", "null", "NULL", "\u{0}"] {
            assert_ne!(
                null,
                d(BlindValue::Text(spelling)),
                "text {spelling:?} must not claim the null slot"
            );
        }
    }

    /// The sentinel is domain-separated like every other value: a null in
    /// `customers.email` is not comparable with a null in `people.email`,
    /// nor with another tenant's null.
    #[test]
    fn the_null_sentinel_is_domain_separated_and_per_space() {
        use crate::custody::{BlindValue, KeyProvider, SpaceId, TestProvider};
        let p = TestProvider::new();
        let (a, b) = (SpaceId("a".into()), SpaceId("b".into()));
        let n = BlindValue::Null;
        assert_ne!(
            p.blind_index(&a, "customers", "email", n).unwrap(),
            p.blind_index(&a, "people", "email", n).unwrap(),
        );
        assert_ne!(
            p.blind_index(&a, "customers", "email", n).unwrap(),
            p.blind_index(&a, "customers", "nickname", n).unwrap(),
        );
        assert_ne!(
            p.blind_index(&a, "customers", "email", n).unwrap(),
            p.blind_index(&b, "customers", "email", n).unwrap(),
            "one tenant's nulls must not be countable from another's"
        );
    }

    /// A DEK rotation must not move the null digest either — otherwise
    /// `sealed indexed unique` admits a second null across the boundary,
    /// which is the same duplicate bug the epoch-independence rule exists
    /// to prevent, on the most common value in the column.
    #[test]
    fn the_null_sentinel_survives_a_dek_rotation() {
        use crate::custody::{BlindValue, KeyProvider, SpaceId, TestProvider};
        let p = TestProvider::new();
        let s = SpaceId("s1".into());
        let before = p
            .blind_index(&s, "customers", "email", BlindValue::Null)
            .unwrap();
        p.rotate(&s, "membership revoked").unwrap();
        assert_eq!(
            before,
            p.blind_index(&s, "customers", "email", BlindValue::Null)
                .unwrap()
        );
    }
}

#[cfg(test)]
mod seal_tests {
    use super::custody::{BlindValue, KeyProvider, Purpose, SpaceId, TestProvider};

    fn sp() -> SpaceId {
        SpaceId("s1".into())
    }

    #[test]
    fn seals_round_trip_and_are_fresh() {
        let p = TestProvider::new();
        let w = p.release(&sp(), Purpose::Write, "t").unwrap();
        let a = p
            .seal_at(
                &w,
                "orders",
                "memo",
                crate::Anchor::Row("r1"),
                b"gift for June",
            )
            .unwrap();
        let b = p
            .seal_at(
                &w,
                "orders",
                "memo",
                crate::Anchor::Row("r1"),
                b"gift for June",
            )
            .unwrap();
        assert_eq!(a[0], 1, "epoch tag leads the envelope");
        assert_ne!(a, b, "fresh nonce per seal: equal values, unequal seals");
        assert_eq!(
            p.open_at(&w, "orders", "memo", crate::Anchor::Row("r1"), &a)
                .unwrap(),
            b"gift for June"
        );
        assert_eq!(
            p.open_at(&w, "orders", "memo", crate::Anchor::Row("r1"), &b)
                .unwrap(),
            b"gift for June"
        );
    }

    #[test]
    fn a_moved_or_tampered_payload_refuses_to_open() {
        let p = TestProvider::new();
        let w = p.release(&sp(), Purpose::Write, "t").unwrap();
        let sealed = p
            .seal_at(&w, "customers", "email", crate::Anchor::Row("r1"), b"a@b.c")
            .unwrap();
        assert!(p
            .open_at(&w, "customers", "name", crate::Anchor::Row("r1"), &sealed)
            .is_err());
        assert!(p
            .open_at(&w, "orders", "email", crate::Anchor::Row("r1"), &sealed)
            .is_err());
        for i in [0, 1, 30, sealed.len() - 1] {
            let mut t = sealed.clone();
            t[i] ^= 1;
            assert!(p
                .open_at(&w, "customers", "email", crate::Anchor::Row("r1"), &t)
                .is_err());
        }
        assert!(p
            .open_at(
                &w,
                "customers",
                "email",
                crate::Anchor::Row("r1"),
                &sealed[..20]
            )
            .is_err());
    }

    /// Keys are per SPACE. Two spaces sealing the same value produce
    /// different digests and cannot open each other's payloads — which is
    /// what makes per-space crypto-erasure and cross-tenant digest
    /// incomparability true rather than hoped for.
    #[test]
    fn custody_is_per_space() {
        let p = TestProvider::new();
        let (a, b) = (SpaceId("a".into()), SpaceId("b".into()));
        let la = p.release(&a, Purpose::Write, "t").unwrap();
        let lb = p.release(&b, Purpose::Write, "t").unwrap();

        let sealed = p
            .seal_at(
                &la,
                "customers",
                "email",
                crate::Anchor::Row("r1"),
                b"x@y.z",
            )
            .unwrap();
        assert!(
            p.open_at(&lb, "customers", "email", crate::Anchor::Row("r1"), &sealed)
                .is_err(),
            "one space must not open another's payload"
        );
        assert_ne!(
            p.blind_index(&a, "customers", "email", BlindValue::Text("x@y.z"))
                .unwrap(),
            p.blind_index(&b, "customers", "email", BlindValue::Text("x@y.z"))
                .unwrap(),
            "a digest must not be comparable across tenants"
        );
    }

    /// Rotation mints a new epoch; old payloads still open (reads accept
    /// any live epoch) and new writes use the newest.
    #[test]
    fn rotation_moves_writes_forward_without_stranding_reads() {
        let p = TestProvider::new();
        let s = sp();
        let w = p.release(&s, Purpose::Write, "t").unwrap();
        let old = p
            .seal_at(&w, "orders", "memo", crate::Anchor::Row("r1"), b"before")
            .unwrap();
        assert_eq!(p.current_epoch(&s).unwrap(), 1);

        assert_eq!(p.rotate(&s, "membership revoked").unwrap(), 2);
        assert_eq!(p.current_epoch(&s).unwrap(), 2);

        let new = p
            .seal_at(&w, "orders", "memo", crate::Anchor::Row("r1"), b"after")
            .unwrap();
        assert_eq!(new[0], 2, "new writes seal under the newest epoch");
        assert_eq!(old[0], 1);
        assert_eq!(
            p.open_at(&w, "orders", "memo", crate::Anchor::Row("r1"), &old)
                .unwrap(),
            b"before"
        );
        assert_eq!(
            p.open_at(&w, "orders", "memo", crate::Anchor::Row("r1"), &new)
                .unwrap(),
            b"after"
        );

        // The blind index does NOT move with a DEK rotation — the settled
        // rule, and the reason `sealed indexed unique` keeps meaning one
        // thing across the revocations that force rotation.
        let before = p
            .blind_index(&s, "customers", "email", BlindValue::Text("x@y.z"))
            .unwrap();
        p.rotate(&s, "again").unwrap();
        assert_eq!(
            p.blind_index(&s, "customers", "email", BlindValue::Text("x@y.z"))
                .unwrap(),
            before,
            "a digest must survive a DEK rotation byte for byte"
        );
    }

    /// Destroying a retired epoch is the narrow erasure primitive: exactly
    /// what is still tagged with it stops opening, and nothing else does.
    #[test]
    fn destroying_a_retired_epoch_orphans_only_its_own_payloads() {
        let p = TestProvider::new();
        let s = sp();
        let w = p.release(&s, Purpose::Write, "t").unwrap();
        let old = p
            .seal_at(&w, "orders", "memo", crate::Anchor::Row("r1"), b"epoch one")
            .unwrap();
        p.rotate(&s, "r").unwrap();
        let new = p
            .seal_at(&w, "orders", "memo", crate::Anchor::Row("r1"), b"epoch two")
            .unwrap();

        // The current epoch is refused — destroying what you still write
        // under is never what anyone meant.
        assert!(p.destroy_epoch(&s, 2).is_err());

        p.destroy_epoch(&s, 1).expect("a retired epoch destroys");
        assert!(
            p.open_at(&w, "orders", "memo", crate::Anchor::Row("r1"), &old)
                .is_err(),
            "its payloads are orphaned"
        );
        assert_eq!(
            p.open_at(&w, "orders", "memo", crate::Anchor::Row("r1"), &new)
                .unwrap(),
            b"epoch two",
            "and nothing else is"
        );
    }

    /// A read lease opens and does not seal. Narrow leases are the point.
    #[test]
    fn a_lease_scopes_what_it_authorizes() {
        let p = TestProvider::new();
        let s = sp();
        let w = p.release(&s, Purpose::Write, "job-1").unwrap();
        let sealed = p
            .seal_at(&w, "orders", "memo", crate::Anchor::Row("r1"), b"v")
            .unwrap();
        let r = p.release(&s, Purpose::Read, "req-9").unwrap();
        assert_eq!(
            p.open_at(&r, "orders", "memo", crate::Anchor::Row("r1"), &sealed)
                .unwrap(),
            b"v"
        );
        assert!(
            p.seal_at(&r, "orders", "memo", crate::Anchor::Row("r1"), b"v")
                .is_err(),
            "a read release must not write"
        );
    }

    // ── the sealed handle ────────────────────────────────────────────────

    /// **The handle is not a function of the payload.** Two seals of one
    /// plaintext, two handles — and neither handle is anything a test
    /// holding the plaintext AND the ciphertext can compute.
    ///
    /// The negative half is the one that matters, so it is not left to a
    /// "looks random" eyeball: every digest a content-addressing scheme
    /// would plausibly have used is computed here and compared. If the
    /// implementation ever regressed to hashing the payload — the exact
    /// mistake §7 corrects, and the shape the crate shipped before — one
    /// of these equalities would hold.
    #[test]
    fn a_handle_is_never_derived_from_what_it_names() {
        use crate::custody::{KeyProvider, Purpose, SpaceId, TestProvider};
        use sha2::{Digest, Sha256};

        let p = TestProvider::new();
        let s = SpaceId("s1".into());
        let w = p.release(&s, Purpose::Write, "t").unwrap();
        let plain = b"gift for June";

        let sealed_a = p
            .seal_at(&w, "orders", "memo", crate::Anchor::Row("r1"), plain)
            .unwrap();
        let sealed_b = p
            .seal_at(&w, "orders", "memo", crate::Anchor::Row("r1"), plain)
            .unwrap();
        let a = p.mint_sealed_ref(&w, 1).unwrap();
        let b = p.mint_sealed_ref(&w, 1).unwrap();

        assert_ne!(
            a, b,
            "two writes of the same plaintext must produce two different handles"
        );

        // Nothing a content hash could be. Every candidate is computed
        // from material the store actually holds.
        let sha16 = |bytes: &[u8]| {
            let d = Sha256::digest(bytes);
            let mut out = [0u8; 16];
            out.copy_from_slice(&d[..16]);
            out
        };
        let mut candidates: Vec<(&str, [u8; 16])> = vec![
            ("sha256(plaintext)", sha16(plain)),
            ("sha256(ciphertext a)", sha16(&sealed_a)),
            ("sha256(ciphertext b)", sha16(&sealed_b)),
            ("sha256(nonce a)", sha16(&sealed_a[1..25])),
            ("sha256(entity:field)", sha16(b"orders:memo")),
            ("hmac-ish over the plaintext", {
                let mut out = [0u8; 16];
                out.copy_from_slice(&crate::hmac_sha256(b"orders:memo", plain)[..16]);
                out
            }),
        ];
        // The literal prefixes and suffixes of both envelopes, in case the
        // handle were ever just *taken* from the ciphertext.
        for (name, bytes) in [("a", &sealed_a), ("b", &sealed_b)] {
            candidates.push((
                if name == "a" {
                    "prefix of a"
                } else {
                    "prefix of b"
                },
                <[u8; 16]>::try_from(&bytes[..16]).unwrap(),
            ));
            candidates.push((
                if name == "a" {
                    "suffix of a"
                } else {
                    "suffix of b"
                },
                <[u8; 16]>::try_from(&bytes[bytes.len() - 16..]).unwrap(),
            ));
        }
        for handle in [a, b] {
            for (what, candidate) in &candidates {
                assert_ne!(&handle, candidate, "the handle is {what}");
            }
        }

        // And not a counter, which is what it actually was: the low eight
        // bytes of a counter-derived handle are a small integer and the
        // high eight are zero.
        for handle in [a, b] {
            assert!(
                handle[..8].iter().any(|b| *b != 0),
                "a counter-derived handle has a zero half"
            );
        }
    }

    /// The salt is in the derivation, and it is per space and per epoch.
    ///
    /// This cannot be observed from one handle — that is the whole point,
    /// the stored value carries no tag — so it is observed the only way it
    /// can be: by holding the randomness still. `blind_index` is the
    /// keyed PRF the derivation runs over, so digesting one fixed seed
    /// under two spaces and two epochs shows the four streams are four
    /// different streams.
    #[test]
    fn the_handle_stream_is_keyed_per_space_and_per_epoch() {
        use crate::custody::{
            BlindValue, KeyProvider, SpaceId, TestProvider, SEALED_HANDLE_DOMAIN,
            SEALED_HANDLE_LABEL,
        };
        let p = TestProvider::new();
        let (a, b) = (SpaceId("a".into()), SpaceId("b".into()));
        let seed = "a seed the RNG handed to both";
        let stream = |space: &SpaceId, epoch: u8| {
            p.blind_index(
                space,
                SEALED_HANDLE_DOMAIN,
                &format!("{SEALED_HANDLE_LABEL}:{epoch}"),
                BlindValue::Text(seed),
            )
            .unwrap()
        };

        assert_ne!(
            stream(&a, 1),
            stream(&b, 1),
            "one repeated seed must not give two tenants the same handle"
        );
        assert_ne!(
            stream(&a, 1),
            stream(&a, 2),
            "one repeated seed must not give two epochs the same handle"
        );
        // And the handle domain is disjoint from every real field's index
        // key: a handle is not some entity's blind digest wearing a hat.
        assert_ne!(
            stream(&a, 1),
            p.blind_index(&a, "orders", "memo", BlindValue::Text(seed))
                .unwrap()
        );
    }

    /// Minting is a write act. A read release opens payloads; it does not
    /// mint the handles new images will carry.
    #[test]
    fn a_read_release_mints_no_handles() {
        use crate::custody::{KeyProvider, Purpose, SpaceId, TestProvider};
        let p = TestProvider::new();
        let s = SpaceId("s1".into());
        let r = p.release(&s, Purpose::Read, "req-9").unwrap();
        assert!(p.mint_sealed_ref(&r, 1).is_err());
        assert!(
            p.mint_blob_ref(&r, 1).is_err(),
            "an upload is a write; a read release does not address one"
        );
        let w = p.release(&s, Purpose::Write, "job-1").unwrap();
        assert!(p.mint_sealed_ref(&w, 1).is_ok());
        assert!(p.mint_blob_ref(&w, 1).is_ok());
    }

    /// The blob handle stream is the sealed-payload handle stream's
    /// sibling, and the two are **disjoint**: same derivation, different
    /// domain. Disjointness is the property that keeps a blob handle from
    /// ever resolving as a payload handle, which would be a cross-store
    /// read by a caller who only ever learned one of them.
    ///
    /// The three things the salt buys transfer unchanged from
    /// `mint_sealed_ref`: a repeated seed cannot make two tenants agree, a
    /// repeated seed cannot make two epochs agree, and the 16 bytes that
    /// land in an object key advertise neither — so a bucket listing does
    /// not print a revocation timeline.
    #[test]
    fn the_blob_handle_stream_is_disjoint_from_the_payload_handle_stream() {
        use crate::custody::{
            BlindValue, KeyProvider, SpaceId, TestProvider, BLOB_HANDLE_DOMAIN,
            SEALED_HANDLE_DOMAIN, SEALED_HANDLE_LABEL,
        };
        let p = TestProvider::new();
        let (a, b) = (SpaceId("a".into()), SpaceId("b".into()));
        let seed = "a seed the RNG handed to both";
        let stream = |domain: &str, space: &SpaceId, epoch: u8| {
            p.blind_index(
                space,
                domain,
                &format!("{SEALED_HANDLE_LABEL}:{epoch}"),
                BlindValue::Text(seed),
            )
            .unwrap()
        };

        assert_ne!(
            stream(BLOB_HANDLE_DOMAIN, &a, 1),
            stream(SEALED_HANDLE_DOMAIN, &a, 1),
            "a blob handle must never be a payload handle wearing a hat"
        );
        assert_ne!(
            stream(BLOB_HANDLE_DOMAIN, &a, 1),
            stream(BLOB_HANDLE_DOMAIN, &b, 1),
            "one repeated seed must not give two tenants the same object key"
        );
        assert_ne!(
            stream(BLOB_HANDLE_DOMAIN, &a, 1),
            stream(BLOB_HANDLE_DOMAIN, &a, 2),
            "one repeated seed must not give two epochs the same object key"
        );
    }

    /// The whole of §2's ruling, asserted the way
    /// `a_handle_is_never_derived_from_what_it_names` asserts it for
    /// payloads: a blob handle is not a function of the file.
    ///
    /// This is the test that would fail if someone re-adopted the
    /// withdrawn content-hash spelling for blobs — which is the specific
    /// mistake §2 exists to prevent, one deploy after the codebase wrote
    /// down why it was withdrawn.
    #[test]
    fn a_blob_handle_is_never_derived_from_the_file() {
        use crate::custody::{KeyProvider, Purpose, SpaceId, TestProvider};
        use sha2::{Digest, Sha256};

        let p = TestProvider::new();
        let s = SpaceId("s1".into());
        let w = p.release(&s, Purpose::Write, "upload-1").unwrap();
        let file = b"%PDF-1.7 the standard industry NDA";

        let a = p.mint_blob_ref(&w, 1).unwrap();
        let b = p.mint_blob_ref(&w, 1).unwrap();
        assert_ne!(
            a, b,
            "two uploads of the same file must produce two different object keys — \
             deduplication IS the equality oracle"
        );

        let sha16 = |bytes: &[u8]| {
            let d = Sha256::digest(bytes);
            let mut out = [0u8; 16];
            out.copy_from_slice(&d[..16]);
            out
        };
        let sealed = p
            .seal_at(&w, "docs", "file", crate::Anchor::Row("r1"), file)
            .unwrap();
        for (what, candidate) in [
            ("sha256(the file)", sha16(file)),
            ("sha256(the ciphertext)", sha16(&sealed)),
            ("sha256(the mime type)", sha16(b"application/pdf")),
            (
                "sha256(the size)",
                sha16(&(file.len() as u64).to_be_bytes()),
            ),
            ("sha256(entity:field)", sha16(b"docs:file")),
        ] {
            assert_ne!(a, candidate, "the handle is {what}");
            assert_ne!(b, candidate, "the handle is {what}");
        }
    }

    /// The local provider refuses to run on a default master key.
    #[test]
    fn a_deployment_without_a_master_key_is_refused() {
        use super::custody::{LocalKeyProvider, MemoryKeyStore};
        std::env::remove_var("ORDO_MASTER_KEY");
        let unset = match LocalKeyProvider::from_env(Box::new(MemoryKeyStore::new())) {
            Ok(_) => panic!("unset is refused"),
            Err(e) => e.to_string(),
        };
        // Audit 33: the refusal gives the form, not only the rule.
        assert!(
            unset.contains("ORDO_MASTER_KEY") && unset.contains("openssl rand -hex 32"),
            "{unset}"
        );
        std::env::set_var("ORDO_MASTER_KEY", "short");
        assert!(
            LocalKeyProvider::from_env(Box::new(MemoryKeyStore::new())).is_err(),
            "a key too short to be a key is refused too"
        );
        std::env::set_var("ORDO_MASTER_KEY", "a-real-deployment-secret-value");
        assert!(LocalKeyProvider::from_env(Box::new(MemoryKeyStore::new())).is_ok());
        std::env::remove_var("ORDO_MASTER_KEY");
    }
}
