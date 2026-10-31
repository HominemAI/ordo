//! Time-based one-time codes: RFC 6238 over RFC 4226
//! (`plan-second-factor.md` §3).
//!
//! The parameters are the ones every authenticator app implements, so they are
//! constants of the protocol and not settings: HMAC-SHA1 (the only algorithm
//! Google Authenticator, Microsoft Authenticator and most code-generating
//! tokens accept), six digits, thirty-second steps. An app developer cannot
//! move them because no app would follow.
//!
//! What is a setting is on the other side of this module: how many wrong codes
//! lock a factor, and whether a second step is owed at all.
//!
//! # What is written here
//!
//! HMAC over `sha1`, the same shape `token.rs` writes over `sha2`. Base32 in
//! the RFC 4648 alphabet, without padding, because that is what an
//! `otpauth://` URI carries. Nothing else: no clock, no storage, no random
//! draw except the secret's, which comes from the operating system.

use sha1::{Digest, Sha1};

/// Digits in a code.
pub const DIGITS: u32 = 6;
/// Seconds in one time step.
pub const STEP_SECS: u64 = 30;
/// Steps either side of the present that still verify: 90 seconds of clock
/// drift in all, the usual allowance.
pub const SKEW_STEPS: u64 = 1;
/// A secret is 20 bytes: 160 bits, RFC 4226 §4 R6's "at least 128" and the
/// natural size for HMAC-SHA1.
pub const SECRET_LEN: usize = 20;

/// A fresh secret from the operating system's generator.
pub fn generate_secret() -> [u8; SECRET_LEN] {
    let mut b = [0u8; SECRET_LEN];
    getrandom::getrandom(&mut b)
        .expect("the operating system CSPRNG is unavailable, refusing to mint a secret");
    b
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha1::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha1::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

/// RFC 4226 HOTP: the code for `counter`, `digits` long, zero padded.
pub fn hotp(secret: &[u8], counter: u64, digits: u32) -> String {
    let mac = hmac_sha1(secret, &counter.to_be_bytes());
    let offset = (mac[19] & 0x0f) as usize;
    let bin = ((mac[offset] as u32 & 0x7f) << 24)
        | ((mac[offset + 1] as u32) << 16)
        | ((mac[offset + 2] as u32) << 8)
        | (mac[offset + 3] as u32);
    let code = bin % 10u32.pow(digits);
    format!("{code:0width$}", width = digits as usize)
}

/// The time step a Unix time falls in.
pub fn step_of(unix: u64) -> u64 {
    unix / STEP_SECS
}

/// RFC 6238 TOTP: the code for the step `unix` falls in.
pub fn totp_at(secret: &[u8], unix: u64, digits: u32) -> String {
    hotp(secret, step_of(unix), digits)
}

/// What a typed code is, as digits: spaces and dashes dropped, exactly
/// [`DIGITS`] of them. `None` for anything else, so a malformed answer never
/// reaches a comparison.
pub fn normalize(typed: &str) -> Option<String> {
    let digits: String = typed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    (digits.len() == DIGITS as usize && digits.bytes().all(|b| b.is_ascii_digit()))
        .then_some(digits)
}

/// Whether `typed` is the code for a step within [`SKEW_STEPS`] of `now`, and
/// **later than `after_step`**. Returns the step it matched, which the caller
/// stores as the last accepted step: a code at or below it is refused, so a
/// code seen over a shoulder is worthless once it was used, inside its own
/// thirty seconds (RFC 6238 §5.2).
///
/// Every candidate step is compared, in constant time and without stopping at
/// the first match, so the time taken says nothing about which step matched.
pub fn matches(secret: &[u8], typed: &str, now: u64, after_step: u64) -> Option<u64> {
    let typed = normalize(typed)?;
    let centre = step_of(now);
    let mut found: Option<u64> = None;
    for step in centre.saturating_sub(SKEW_STEPS)..=centre.saturating_add(SKEW_STEPS) {
        let want = hotp(secret, step, DIGITS);
        let equal = want.len() == typed.len()
            && want
                .bytes()
                .zip(typed.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0;
        if equal && step > after_step && found.is_none_or(|f| step > f) {
            found = Some(step);
        }
    }
    found
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 §6 base32, no padding: how an authenticator app is given a secret.
pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for b in bytes {
        acc = (acc << 8) | *b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn percent(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The `otpauth://` URI an authenticator app reads from a QR code:
/// `otpauth://totp/<issuer>:<account>?secret=..&issuer=..&algorithm=SHA1&digits=6&period=30`.
pub fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        percent(issuer),
        percent(account),
        base32(secret),
        percent(issuer),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 appendix B, the SHA-1 column. The reference secret is the ASCII
    /// string `12345678901234567890`, and the appendix's codes are eight digits.
    #[test]
    fn totp_matches_the_rfc_6238_sha1_vectors() {
        let secret = b"12345678901234567890";
        for (time, code) in [
            (59u64, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ] {
            assert_eq!(totp_at(secret, time, 8), code, "at {time}");
        }
    }

    /// RFC 4226 appendix D: counters 0 to 9 over the same secret, six digits.
    #[test]
    fn hotp_matches_the_rfc_4226_vectors() {
        let secret = b"12345678901234567890";
        let want = [
            "755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583",
            "399871", "520489",
        ];
        for (counter, code) in want.iter().enumerate() {
            assert_eq!(hotp(secret, counter as u64, 6), *code, "counter {counter}");
        }
    }

    #[test]
    fn a_code_is_accepted_once_inside_the_skew_and_never_again() {
        let secret = generate_secret();
        let now = 1_700_000_000;
        let code = totp_at(&secret, now, DIGITS);
        let step = matches(&secret, &code, now, 0).expect("the present step");
        assert_eq!(step, step_of(now));
        // The same code after it was used: refused.
        assert_eq!(matches(&secret, &code, now, step), None);
        // A code one step old, and one step ahead, are inside the skew.
        let old = totp_at(&secret, now - STEP_SECS, DIGITS);
        assert!(matches(&secret, &old, now, 0).is_some());
        let ahead = totp_at(&secret, now + STEP_SECS, DIGITS);
        assert!(matches(&secret, &ahead, now, 0).is_some());
        // Two steps away is not.
        let far = totp_at(&secret, now - 3 * STEP_SECS, DIGITS);
        assert_eq!(matches(&secret, &far, now, 0), None);
    }

    #[test]
    fn what_is_typed_is_digits_or_nothing() {
        assert_eq!(normalize("123 456").as_deref(), Some("123456"));
        assert_eq!(normalize("123-456").as_deref(), Some("123456"));
        assert_eq!(normalize("12345"), None);
        assert_eq!(normalize("1234567"), None);
        assert_eq!(normalize("12345a"), None);
        assert_eq!(normalize(""), None);
    }

    #[test]
    fn base32_matches_the_rfc_4648_vectors_and_the_uri_carries_it() {
        // RFC 4648 §10, without padding.
        for (plain, coded) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(base32(plain.as_bytes()), coded);
        }
        let uri = otpauth_uri("Ordo Demo", "ana@example.test", b"12345678901234567890");
        assert_eq!(
            uri,
            "otpauth://totp/Ordo%20Demo:ana%40example.test?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ\
             &issuer=Ordo%20Demo&algorithm=SHA1&digits=6&period=30"
        );
    }
}
