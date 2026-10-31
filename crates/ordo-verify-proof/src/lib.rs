//! The published offline verifier for `ordo.chainproof/1` and
//! `ordo.attestation/1`.
//!
//! `security-logging-design.md` §7 owes third parties five things for the
//! optional attestation layers, and this crate is three of them: *the
//! attestation payload schema, versioned and published before first use*;
//! *the verification procedure end to end*; and *a reference verifier, so the
//! common case is a library call rather than an implementation project*.
//!
//! # Why a separate binary rather than `ordo verify --proof`
//!
//! `ordo verify` exists, it is the right shape, and it is the wrong home for
//! this — for one reason, stated in `ordo-cli`'s own module doc: *"`ordo-cli`
//! now depends on `ordo-exec`, so building the `ordo` binary builds the
//! driver, the pool"*. A verifier is handed to somebody who **must not have
//! to trust the deployment**, and a binary that links a Postgres driver is a
//! binary a careful recipient has to audit before running. This one links
//! `ordo-commandlog` and `serde_json`. It has no socket, no driver, no
//! environment variable, and no way to reach the deployment that produced its
//! input, and `tests/offline.rs` asserts that by reading the manifest.
//!
//! `ordo verify --proof` is still offered, and it calls straight into
//! [`verify_proof`] here — one routine, two front doors, so an operator finds
//! it beside the other verification commands and an auditor gets a binary
//! they can read in an afternoon.
//!
//! # What a proof proves, and what it does not
//!
//! **Proves.** That the links in the segment are unbroken; that the segment
//! begins at the anchor it names; that every signed marker inside it agrees
//! with the link at its position; and that each of those signatures verifies
//! under a key **you supplied**. Taken together with a head you pinned
//! earlier — from an attestation delivery, a WORM object, or a response
//! header — that is: *the history I witnessed at time T is still what this
//! chain says.* Rewriting becomes provable, which is Certificate
//! Transparency's guarantee and is strong for the same reason.
//!
//! **Does not prove.** That a hash corresponds to any particular content. A
//! proof carries digests, not bodies, so a verifier has nothing to recompute
//! them from; checking content means verifying an export bundle, where the
//! groups are present. [`Verdict::caveats`] says so on every run, including
//! the successful ones — a verifier that only mentions its limits when it
//! fails has trained its reader to skip the part that matters.
//!
//! **Does not prove, second.** Anything at all, if you did not supply a key.
//! The key inside the proof document is the platform's claim about its own
//! key, and checking a signature against a key the signer chose is a
//! tautology. Without `--key` this reports [`Verdict::structure_only`] and
//! says the word "unverified" in its summary.

pub mod attestation;
pub mod proof;

pub use attestation::{verify_attestation, AttestationVerdict};
pub use proof::{verify_proof, Finding, Verdict};

/// Decode lowercase or uppercase hex. `None` on any non-hex byte or an odd
/// length — never a partial decode, because a verifier that silently
/// truncated its input would report on bytes nobody sent.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Lowercase hex, matching `ChainHash`'s `Display` and the export bundle's
/// encoding.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
