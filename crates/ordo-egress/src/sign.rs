//! Pin 4: `X-Ordo-Signature`, and the two keys that must never be one key.
//!
//! # Two signatures, two claims, two keys
//!
//! An attestation delivery carries **two** independent signatures and they
//! answer different questions:
//!
//! | | signed material | key | claim |
//! |---|---|---|---|
//! | payload | `Checkpoint::signed_bytes(space, upto, head, at)` | the deployment's Ed25519 checkpoint signer | *this space's chain was at this head at this time* |
//! | transport | `v1 ‖ timestamp ‖ destination ‖ body` | a per-sink HMAC secret | *this HTTP request came from this deployment, for you, and is not a replay* |
//!
//! The payload signature is computed by `ordo_exec::cadence::attest_due` when
//! the record is minted and is copied out of `$attestations` verbatim; this
//! crate never sees the Ed25519 key and could not use it if handed one. That
//! is not tidiness. **The egress identity is the one built to hold nothing**,
//! and the checkpoint signer is the key that authors this deployment's claims
//! about its own history — the single worst key for it to hold. A design that
//! signed deliveries with the checkpoint key would put history-authoring
//! authority in the process whose entire job is to open sockets to
//! customer-named hosts. So: never the same key, and the crate boundary is
//! what makes it never.
//!
//! It also means a receiver needs no key discovery to authenticate the
//! *transport* — they have their own secret from the grant — and needs only a
//! public key to verify the *payload*. Those are different receivers in
//! practice: the first is a webhook endpoint checking a header at ingest, the
//! second is anyone auditing the stored record months later.
//!
//! # The signed material, and one widening of §7b
//!
//! §7b pins *"HMAC over `timestamp ‖ body`… with a replay window and the
//! timestamp inside the signed material"*. This implementation signs
//! **`version ‖ timestamp ‖ destination ‖ body`**, each part length-prefixed.
//! Two additions, both strict strengthenings, both recorded in
//! `corpus-rulings.md` (2026-09-08) rather than made quietly:
//!
//! - **The destination.** Without it, a delivery captured on the wire is
//!   valid at *any* path on any host that shares the secret — and a receiver
//!   who reuses one secret across two sinks, which nothing prevents, has
//!   given an attacker a cross-sink replay. Binding the canonical destination
//!   closes it for the cost of one field.
//! - **A version tag.** `v1` inside the signed bytes, not only in the header.
//!   A scheme whose version lives only outside the MAC is a scheme whose
//!   version an attacker chooses, which is how every downgrade works.
//!
//! Length-prefixing rather than concatenation, for the reason every
//! canonical-encoding note in this repo gives: `("1", "23")` and `("12", "3")`
//! must not produce the same bytes, or the parts are not really separate.
//! `ordo-commandlog`'s chain encoding uses u64 big-endian lengths, so this
//! does too — one convention in the platform, not two.
//!
//! # Replay
//!
//! The timestamp is inside the MAC and also in `X-Ordo-Timestamp`, so a
//! receiver can reject stale deliveries without parsing the body. The window
//! belongs to the receiver — it is their clock skew — so this crate publishes
//! [`DEFAULT_REPLAY_WINDOW`] as documentation and a reference check, and does
//! not enforce anything on the sending side, where enforcement would be
//! theatre.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// The header carrying the transport MAC, `v1=<64 lowercase hex>`.
pub const SIGNATURE_HEADER: &str = "x-ordo-signature";
/// The header carrying the seconds-since-epoch timestamp inside the MAC.
pub const TIMESTAMP_HEADER: &str = "x-ordo-timestamp";
/// The scheme version, in the header *and* in the signed bytes.
pub const SIGNATURE_VERSION: &str = "v1";
/// What a receiver should allow for clock skew. Advisory; the receiver owns
/// this decision and this crate does not enforce it on send.
pub const DEFAULT_REPLAY_WINDOW_SECS: u64 = 300;

/// A per-sink shared secret, shown once at grant.
///
/// No `Debug`, no `Display`, no `Clone` into a `String`: the one way a secret
/// leaks from a well-written system is a log line, and the way to not have
/// that log line is to not have the impl that writes it.
pub struct SinkSecret(Vec<u8>);

impl SinkSecret {
    /// From raw bytes. A grant mints 32 random bytes.
    pub fn from_bytes(bytes: &[u8]) -> SinkSecret {
        SinkSecret(bytes.to_vec())
    }

    /// From the lowercase-hex rendering stored beside the sink.
    pub fn from_hex(s: &str) -> Option<SinkSecret> {
        if !s.len().is_multiple_of(2) || s.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(s.len() / 2);
        for i in (0..s.len()).step_by(2) {
            out.push(u8::from_str_radix(s.get(i..i + 2)?, 16).ok()?);
        }
        Some(SinkSecret(out))
    }
}

impl std::fmt::Debug for SinkSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SinkSecret(<redacted>)")
    }
}

/// The exact bytes the transport MAC covers.
///
/// Public because a receiver's reference implementation must be able to
/// reproduce them, and because the published schema in
/// `docs/howto-audit.md` is generated from this function's doc rather than
/// retyped beside it.
///
/// ```text
/// signed := str("v1") ‖ str(timestamp-as-decimal) ‖ str(destination) ‖ str(body)
/// str    := u64-BE byte-length ‖ bytes
/// ```
pub fn signed_material(timestamp: u64, destination: &str, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(destination.len() + body.len() + 64);
    let ts = timestamp.to_string();
    for part in [
        SIGNATURE_VERSION.as_bytes(),
        ts.as_bytes(),
        destination.as_bytes(),
        body,
    ] {
        out.extend_from_slice(&(part.len() as u64).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// The value of `X-Ordo-Signature`.
pub fn signature_header(
    secret: &SinkSecret,
    timestamp: u64,
    destination: &str,
    body: &[u8],
) -> String {
    let mac = mac(secret, timestamp, destination, body);
    let mut s = String::with_capacity(3 + 64);
    s.push_str(SIGNATURE_VERSION);
    s.push('=');
    for b in mac {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn mac(secret: &SinkSecret, timestamp: u64, destination: &str, body: &[u8]) -> [u8; 32] {
    let mut h =
        <Hmac<Sha256> as Mac>::new_from_slice(&secret.0).expect("HMAC accepts a key of any length");
    h.update(&signed_material(timestamp, destination, body));
    h.finalize().into_bytes().into()
}

/// The receiver's half, published so the reference verifier and the test
/// listener use the same routine the sender does.
///
/// Constant-time comparison. `ordo_api::mac::ct_eq` is the same five lines,
/// and this is not a second copy of anything cryptographic — it is a
/// comparison — but the reason it is not imported is the reason this crate
/// has no `ordo-*` dependencies at all, which is the module doc of
/// `lib.rs`.
pub fn verify(
    secret: &SinkSecret,
    header: &str,
    timestamp: u64,
    destination: &str,
    body: &[u8],
) -> bool {
    let Some(hex) = header.strip_prefix("v1=") else {
        return false;
    };
    let Some(given) = decode_hex(hex) else {
        return false;
    };
    ct_eq(&given, &mac(secret, timestamp, destination, body))
}

/// Is this timestamp inside the receiver's replay window? Advisory; a
/// receiver enforces its own.
pub fn within_window(timestamp: u64, now: u64, window_secs: u64) -> bool {
    now.abs_diff(timestamp) <= window_secs
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> SinkSecret {
        SinkSecret::from_bytes(b"a per-sink secret shown once at grant")
    }

    #[test]
    fn round_trips() {
        let h = signature_header(
            &secret(),
            1_757_000_000,
            "https://hooks.example.com/a",
            b"{}",
        );
        assert!(verify(
            &secret(),
            &h,
            1_757_000_000,
            "https://hooks.example.com/a",
            b"{}"
        ));
    }

    #[test]
    fn the_destination_is_inside_the_mac() {
        // The widening over §7b: a delivery captured for /a must not verify
        // at /b, even under the same secret.
        let h = signature_header(
            &secret(),
            1_757_000_000,
            "https://hooks.example.com/a",
            b"{}",
        );
        assert!(!verify(
            &secret(),
            &h,
            1_757_000_000,
            "https://hooks.example.com/b",
            b"{}"
        ));
        assert!(!verify(
            &secret(),
            &h,
            1_757_000_000,
            "https://other.example.com/a",
            b"{}"
        ));
    }

    #[test]
    fn the_timestamp_is_inside_the_mac() {
        let h = signature_header(&secret(), 1_757_000_000, "https://h/a", b"{}");
        assert!(!verify(&secret(), &h, 1_757_000_001, "https://h/a", b"{}"));
    }

    #[test]
    fn parts_are_length_prefixed_so_they_cannot_slide() {
        // Without length prefixes these two would produce identical bytes.
        assert_ne!(
            signed_material(1, "https://h/ab", b"c"),
            signed_material(1, "https://h/a", b"bc")
        );
    }

    #[test]
    fn a_secret_does_not_render_itself() {
        assert_eq!(format!("{:?}", secret()), "SinkSecret(<redacted>)");
    }

    #[test]
    fn a_malformed_header_is_false_not_a_panic() {
        for h in ["", "v1=", "v1=zz", "deadbeef", "v2=00", "v1=0"] {
            assert!(!verify(&secret(), h, 1, "https://h/a", b"{}"), "{h}");
        }
    }

    #[test]
    fn hex_secrets_round_trip() {
        let h = signature_header(
            &SinkSecret::from_hex("00ff10").unwrap(),
            7,
            "https://h/",
            b"x",
        );
        assert!(verify(
            &SinkSecret::from_bytes(&[0x00, 0xff, 0x10]),
            &h,
            7,
            "https://h/",
            b"x"
        ));
        assert!(SinkSecret::from_hex("0").is_none());
        assert!(SinkSecret::from_hex("").is_none());
        assert!(SinkSecret::from_hex("gg").is_none());
    }

    #[test]
    fn the_replay_window_is_symmetric() {
        assert!(within_window(1000, 1200, 300));
        assert!(within_window(1200, 1000, 300));
        assert!(!within_window(1000, 1400, 300));
    }
}
