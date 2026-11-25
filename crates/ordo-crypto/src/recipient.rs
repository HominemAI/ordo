//! **Sealed to a recipient's public key**: what a staging clone travels in
//! (`plan-versions.md` §3.4, ruled 2026-11-02, D17).
//!
//! Production seals the masked bundle to staging's public key, so only
//! staging can open it. Neither side's data keys leave it: the bundle is
//! opened in staging and every value is sealed again under staging's own
//! keys as it is imported.
//!
//! The construction is X25519 key agreement with a fresh key per message,
//! then XChaCha20-Poly1305 under a key derived from the shared secret and
//! both public keys:
//!
//! ```text
//! sealed = "osr1" || ephemeral public (32) || nonce (24) || ciphertext || tag
//! key    = SHA-256("ordo.sealed-to.v1" || shared || ephemeral public || recipient public)
//! ```
//!
//! The associated data binds what the caller names (for a clone: the
//! tenant, the version and the environment), so a sealed bundle presented
//! as another does not open.

use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::XChaCha20Poly1305;
use curve25519_dalek::montgomery::MontgomeryPoint;
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 4] = b"osr1";
const LABEL: &[u8] = b"ordo.sealed-to.v1";
const NONCE: usize = 24;

/// Why a sealed message did not open, or a key did not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipientError {
    /// The public key is not 64 hex digits.
    Key(String),
    /// The message is not one this module sealed, or not to this key, or
    /// not with this associated data.
    Open,
}

impl std::fmt::Display for RecipientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecipientError::Key(why) => write!(f, "{why}"),
            RecipientError::Open => f.write_str(
                "the sealed bundle does not open with this deployment's key: it was sealed to \
                 another key, or changed on the way",
            ),
        }
    }
}

impl std::error::Error for RecipientError {}

/// A recipient's secret key. Kept by the recipient alone.
#[derive(Clone)]
pub struct RecipientSecret([u8; 32]);

impl std::fmt::Debug for RecipientSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecipientSecret(..)")
    }
}

impl RecipientSecret {
    /// A fresh secret from the operating system's generator.
    pub fn generate() -> RecipientSecret {
        let mut s = [0u8; 32];
        OsRng.fill_bytes(&mut s);
        RecipientSecret(s)
    }

    /// A secret from its stored bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> RecipientSecret {
        RecipientSecret(bytes)
    }

    /// The stored bytes. For sealing at rest, never for display.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    /// The public key others seal to.
    pub fn public(&self) -> RecipientPublic {
        RecipientPublic(MontgomeryPoint::mul_base_clamped(self.0).to_bytes())
    }

    /// Open what [`RecipientPublic::seal`] sealed to this key with `aad`.
    pub fn open(&self, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>, RecipientError> {
        if sealed.len() < MAGIC.len() + 32 + NONCE + 16 || &sealed[..4] != MAGIC {
            return Err(RecipientError::Open);
        }
        let mut eph = [0u8; 32];
        eph.copy_from_slice(&sealed[4..36]);
        let shared = MontgomeryPoint(eph).mul_clamped(self.0).to_bytes();
        if shared == [0u8; 32] {
            return Err(RecipientError::Open);
        }
        let key = derive(&shared, &eph, &self.public().0);
        let cipher = XChaCha20Poly1305::new((&key).into());
        let nonce = &sealed[36..36 + NONCE];
        cipher
            .decrypt(
                nonce.into(),
                Payload {
                    msg: &sealed[36 + NONCE..],
                    aad,
                },
            )
            .map_err(|_| RecipientError::Open)
    }
}

/// A recipient's public key: 32 bytes, written as 64 hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecipientPublic([u8; 32]);

impl RecipientPublic {
    /// From 64 hex digits.
    pub fn parse(hex: &str) -> Result<RecipientPublic, RecipientError> {
        let hex = hex.trim();
        let bad = || {
            RecipientError::Key(format!(
                "`{hex}` is not a clone key: 64 hex digits, as `ordo clones key` prints it on \
                 the deployment that receives the clone"
            ))
        };
        if hex.len() != 64 {
            return Err(bad());
        }
        let mut out = [0u8; 32];
        for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
            let s = std::str::from_utf8(pair).map_err(|_| bad())?;
            out[i] = u8::from_str_radix(s, 16).map_err(|_| bad())?;
        }
        if MontgomeryPoint(out).mul_clamped([1u8; 32]).to_bytes() == [0u8; 32] {
            return Err(bad());
        }
        Ok(RecipientPublic(out))
    }

    /// 64 lowercase hex digits.
    pub fn hex(&self) -> String {
        crate::hex(&self.0)
    }

    /// Seal `plain` so that only this key's secret opens it, bound to `aad`.
    pub fn seal(&self, plain: &[u8], aad: &[u8]) -> Vec<u8> {
        let mut eph_secret = [0u8; 32];
        OsRng.fill_bytes(&mut eph_secret);
        let eph = MontgomeryPoint::mul_base_clamped(eph_secret).to_bytes();
        let shared = MontgomeryPoint(self.0).mul_clamped(eph_secret).to_bytes();
        let key = derive(&shared, &eph, &self.0);
        let cipher = XChaCha20Poly1305::new((&key).into());
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let body = cipher
            .encrypt(&nonce, Payload { msg: plain, aad })
            .expect("XChaCha20-Poly1305 encryption is total over byte inputs");
        let mut out = Vec::with_capacity(4 + 32 + NONCE + body.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&eph);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&body);
        out
    }
}

fn derive(shared: &[u8; 32], eph: &[u8; 32], recipient: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(LABEL);
    h.update(shared);
    h.update(eph);
    h.update(recipient);
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_recipient_opens_and_only_with_the_same_aad() {
        let staging = RecipientSecret::generate();
        let other = RecipientSecret::generate();
        let public = RecipientPublic::parse(&staging.public().hex()).unwrap();
        let sealed = public.seal(b"masked rows", b"tenant=acme");
        assert_eq!(
            staging.open(&sealed, b"tenant=acme").unwrap(),
            b"masked rows"
        );
        assert_eq!(
            staging.open(&sealed, b"tenant=beta"),
            Err(RecipientError::Open)
        );
        assert_eq!(
            other.open(&sealed, b"tenant=acme"),
            Err(RecipientError::Open)
        );
        let mut changed = sealed.clone();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert_eq!(
            staging.open(&changed, b"tenant=acme"),
            Err(RecipientError::Open)
        );
        assert!(!sealed.windows(6).any(|w| w == b"masked"));
    }

    #[test]
    fn a_key_that_is_not_one_is_refused_naming_the_command() {
        let e = RecipientPublic::parse("abc").unwrap_err();
        assert!(e.to_string().contains("ordo clones key"), "{e}");
        assert!(RecipientPublic::parse(&"0".repeat(64)).is_err());
        assert!(RecipientPublic::parse(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn a_secret_round_trips_through_its_bytes() {
        let s = RecipientSecret::generate();
        let back = RecipientSecret::from_bytes(s.to_bytes());
        assert_eq!(s.public(), back.public());
    }
}
