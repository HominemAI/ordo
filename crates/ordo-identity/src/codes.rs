//! Codes people type or paste: setup and invite codes, recovery codes.
//!
//! Shared by the console and the identity service, so a recovery code the
//! console printed still works once the identity service holds it
//! (`plan-identity-service.md` 2.1, 6.1).
//!
//! - A **code** is 120 random bits, shown as six groups of four Crockford
//!   base32 characters. When only its tagged SHA-256 is stored, at 120 bits
//!   a plain hash is the approved form (NIST SP 800-63B, look-up secrets),
//!   and the hash is what finds the row.
//! - A **recovery code** has the same shape. Its first [`LOOKUP_LEN`]
//!   characters find the row and are stored as they are; the other 16 (80
//!   bits) are stored only as a password hash ([`mint_recovery_set`]).

use sha2::{Digest, Sha256};

use crate::provider::PasswordHasher;

/// Crockford base32: no `I`, `L`, `O` or `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Twenty-four characters, five bits each.
pub const CODE_LEN: usize = 24;
/// The part of a recovery code that finds its row.
pub const LOOKUP_LEN: usize = 8;
/// Ten recovery codes to a set.
pub const RECOVERY_SET_SIZE: usize = 10;

/// Bytes from the operating system's generator. A service that cannot get
/// randomness must not mint anything.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b)
        .expect("the operating system CSPRNG is unavailable; refusing to mint a secret");
    b
}

/// A fresh code, 24 characters.
pub fn mint_code() -> String {
    let bytes = random_bytes::<15>();
    let mut out = String::with_capacity(CODE_LEN);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    out
}

/// A code as it is shown: six groups of four.
pub fn display_code(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// A code as typed, normalised: dashes and spaces dropped, upper case, and
/// the letters Crockford reads as digits read as digits. `None` when it
/// cannot be a code.
pub fn normalise_code(typed: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for c in typed.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_LEN).then_some(out)
}

/// A tagged SHA-256, hex.
pub fn tagged_hash(tag: &str, value: &str) -> String {
    let mut h = Sha256::new();
    h.update(tag.as_bytes());
    h.update([0u8]);
    h.update(value.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// One recovery code as stored: the lookup part, and the password hash of
/// the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRow {
    pub lookup: String,
    pub hash: String,
}

/// Ten fresh recovery codes: as shown once, and as stored. Slow by design
/// (argon2id); call it off the async workers.
pub fn mint_recovery_set(hasher: &dyn PasswordHasher) -> (Vec<String>, Vec<RecoveryRow>) {
    (0..RECOVERY_SET_SIZE)
        .map(|_| {
            let code = mint_code();
            let row = RecoveryRow {
                lookup: code[..LOOKUP_LEN].to_string(),
                hash: hasher.hash(&code[LOOKUP_LEN..]),
            };
            (display_code(&code), row)
        })
        .unzip()
}

/// A typed recovery code split into its lookup and its secret, or `None`
/// when it cannot be one.
pub fn split_recovery(typed: &str) -> Option<(String, String)> {
    let code = normalise_code(typed)?;
    let (lookup, secret) = code.split_at(LOOKUP_LEN);
    Some((lookup.to_string(), secret.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_round_trips_through_its_display_form() {
        let code = mint_code();
        assert_eq!(code.len(), CODE_LEN);
        let shown = display_code(&code);
        assert_eq!(shown.len(), CODE_LEN + 5);
        assert_eq!(normalise_code(&shown).as_deref(), Some(code.as_str()));
        assert_eq!(
            normalise_code(&shown.to_lowercase()).as_deref(),
            Some(code.as_str())
        );
        assert_eq!(normalise_code("too-short"), None);
        assert_ne!(mint_code(), mint_code());
    }

    #[test]
    fn a_recovery_set_verifies_its_own_codes_only() {
        let hasher = crate::Argon2idHasher::weak_for_tests();
        let (shown, rows) = mint_recovery_set(&hasher);
        assert_eq!(shown.len(), RECOVERY_SET_SIZE);
        let (lookup, secret) = split_recovery(&shown[3]).unwrap();
        assert_eq!(lookup, rows[3].lookup);
        assert!(hasher.verify(&secret, &rows[3].hash));
        assert!(!hasher.verify(&secret, &rows[4].hash));
    }
}
