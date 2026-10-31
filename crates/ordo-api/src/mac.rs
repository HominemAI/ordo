//! HMAC-SHA256 and constant-time comparison — the two primitives every
//! authenticator in this crate goes through.
//!
//! Crate rule (SH2-18): **anything compared as a secret or an authenticator
//! is compared with [`ct_eq`]**, never `==`. Early-exit comparison on a MAC
//! verification path is a forgery oracle under repeated queries, and the
//! surface only grows once page tokens, fingerprints, and signatures all
//! carry MACs.

use sha2::{Digest, Sha256};

const BLOCK: usize = 64;

/// HMAC-SHA256 (RFC 2104) over `parts`, concatenated in order. Callers
/// length-prefix their fields so the concatenation is unambiguous.
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    // Key normalization: hash if longer than the block, zero-pad otherwise.
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        block[..32].copy_from_slice(&digest);
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= block[i];
        opad[i] ^= block[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    for part in parts {
        inner.update(part);
    }
    let inner = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

/// Constant-time byte comparison. Length is not secret (it is structural
/// here), the contents are: unequal lengths answer immediately, equal lengths
/// fold every byte before answering.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 test case 2 — a known-answer test, so the hand-rolled HMAC is
    /// checked against the standard rather than against itself.
    #[test]
    fn hmac_matches_rfc4231_case_2() {
        let mac = hmac_sha256(b"Jefe", &[b"what do ya want for nothing?"]);
        let expected = [
            0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95,
            0x75, 0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9,
            0x64, 0xec, 0x38, 0x43,
        ];
        assert_eq!(mac, expected);
    }

    #[test]
    fn hmac_is_key_and_input_dependent() {
        let a = hmac_sha256(b"k1", &[b"msg"]);
        assert_ne!(a, hmac_sha256(b"k2", &[b"msg"]));
        assert_ne!(a, hmac_sha256(b"k1", &[b"msh"]));
        // Concatenation order matters, which is why callers length-prefix.
        assert_eq!(
            hmac_sha256(b"k1", &[b"ab", b"c"]),
            hmac_sha256(b"k1", &[b"abc"])
        );
    }

    #[test]
    fn ct_eq_agrees_with_equality() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }
}
