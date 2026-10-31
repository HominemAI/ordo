//! Checkpoint signing — Ed25519, and verification from a public key alone.
//!
//! # Why the checkpoint signer is asymmetric, and why it is not a `KeyProvider`
//!
//! `security-logging-design.md` settles what a checkpoint is *for*: a
//! verified checkpoint lets verification and pruning start mid-chain, and
//! the attestation layers publish signed heads to receivers outside the
//! deployment — a webhook endpoint, a WORM bucket, a git repository,
//! OpenTimestamps, Rekor. The doc's own obligation list names "the
//! signature format and key discovery — how a receiver obtains the
//! verifying key", and layer 1 ("client pinning") has every client checking
//! head continuity on its own. **A receiver that needs the signing key to
//! check a signature is a receiver that can forge one**, which deletes the
//! entire property. So the signature must be asymmetric, and Ed25519 is the
//! obvious choice: 32-byte public key, 64-byte deterministic signature, no
//! nonce to get wrong, verifiers in every language an SDK might ship in.
//!
//! What it replaces is worse than weak. The old `StubSigner` was a 64-bit
//! FNV-style fold with **no key input at all** — `key_id` was a text prefix
//! on the output — so anybody who could read a checkpoint could write one.
//! It is now `#[cfg(test)]`, which is the same rule F0 states for a
//! `KeyProvider` that returns plaintext: a thing that secures nothing must
//! not be *selectable* outside unit tests, because "we would never configure
//! that" is not a control.
//!
//! ## The `KeyProvider` question
//!
//! `engine-plan.md` F0 says "`Signer` implementations ride `KeyProvider`",
//! and `ordo-crypto`'s trait was rewritten to expose **operations, never key
//! material**. Two readings are defensible — add a `sign`/`verify` pair to
//! `KeyProvider`, or let the signer hold its own keypair. This is the
//! second, for three reasons that are about custody rather than convenience:
//!
//! 1. **The lifecycles are opposites.** `KeyProvider` custody is per space,
//!    epoched, rotated on every membership revocation, and *destroyable* —
//!    `destroy_epoch` is the crypto-erasure primitive. A checkpoint key
//!    destroyed is every past checkpoint unverifiable, i.e. exactly the
//!    tamper-evidence the checkpoints exist to provide, deleted by the
//!    routine operation the confidentiality keys are designed around.
//! 2. **The scopes are opposites.** Confidentiality keys are per space and
//!    must never be comparable across tenants (that is why `smk()` grew a
//!    space argument). A checkpoint key is *deployment identity*: one key,
//!    published, stable, so a receiver holding last month's attestation can
//!    still check this month's.
//! 3. **The disclosure rules are opposites.** `KeyProvider`'s contract is
//!    that key material never leaves it. The checkpoint verifying key's
//!    whole job is to leave it — it is published. Threading a "release this
//!    one, it's fine" exception through a trait whose single invariant is
//!    "nothing leaves" would be the kind of exception that later swallows
//!    the rule.
//!
//! What the parenthetical *does* bind, and what is honoured here, is the
//! injection discipline: nothing constructs a signer at a call site.
//! [`Ed25519Signer::from_env`] mirrors `LocalKeyProvider::from_env` —
//! deployment-supplied seed, refused rather than defaulted — and a
//! KMS-backed signer is a second implementation of the same trait, since
//! `sign` is the only operation the platform needs and it is exactly the
//! operation a KMS exposes.

use ed25519_dalek::{Signature, Signer as DalekSigner, SigningKey, Verifier, VerifyingKey};

use crate::store::{Checkpoint, Signer};

/// Refusals from key loading. Deliberately shapeless about *why* a seed was
/// rejected beyond its length — the operator gets the length, an attacker
/// gets nothing they did not already supply.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignerError {
    /// The environment variable naming the key is unset.
    Missing(String),
    /// Not 64 hex characters (a 32-byte seed) / 64 hex characters (a
    /// 32-byte public key).
    BadKey(String),
}

impl std::fmt::Display for SignerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignerError::Missing(v) => write!(
                f,
                "{v} is not set — refusing to sign checkpoints with a default key"
            ),
            SignerError::BadKey(m) => write!(f, "checkpoint key: {m}"),
        }
    }
}

impl std::error::Error for SignerError {}

/// The signing half. Holds the private seed; never hands it out.
pub struct Ed25519Signer {
    key_id: String,
    signing: SigningKey,
}

impl Ed25519Signer {
    /// Build from a 32-byte seed the deployment supplies. The seed is the
    /// key: Ed25519 derives the scalar and the public key from it, so
    /// custody of these 32 bytes is custody of the signing identity.
    pub fn from_seed(key_id: impl Into<String>, seed: &[u8; 32]) -> Ed25519Signer {
        Ed25519Signer {
            key_id: key_id.into(),
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// Read the seed from `ORDO_CHECKPOINT_KEY` (64 hex chars) and the key
    /// id from `ORDO_CHECKPOINT_KEY_ID`.
    ///
    /// Unset is a refusal, not a default. A default checkpoint key is the
    /// same defect as a default master key: every deployment sharing it
    /// means every deployment can sign every other's history.
    pub fn from_env() -> Result<Ed25519Signer, SignerError> {
        // Or `ORDO_CHECKPOINT_KEY_FILE`, a file holding the same hex.
        let hex = ordo_env::secret("ORDO_CHECKPOINT_KEY")
            .map_err(|e| SignerError::BadKey(e.to_string()))?
            .ok_or_else(|| SignerError::Missing("ORDO_CHECKPOINT_KEY".into()))?;
        let seed = seed_from_hex(&hex)?;
        let key_id = std::env::var("ORDO_CHECKPOINT_KEY_ID")
            .map_err(|_| SignerError::Missing("ORDO_CHECKPOINT_KEY_ID".into()))?;
        Ok(Ed25519Signer::from_seed(key_id, &seed))
    }

    /// The public half, as a receiver would hold it. This is the only key
    /// material that ever leaves, and it is meant to.
    pub fn verifier(&self) -> CheckpointVerifier {
        CheckpointVerifier {
            key_id: self.key_id.clone(),
            verifying: self.signing.verifying_key(),
        }
    }
}

impl Signer for Ed25519Signer {
    fn key_id(&self) -> &str {
        &self.key_id
    }

    fn sign(&self, bytes: &[u8]) -> Vec<u8> {
        self.signing.sign(bytes).to_bytes().to_vec()
    }

    // The trait's default `verify` is re-sign-and-compare, which is correct
    // only for MAC-shaped signers. Ed25519 signatures ARE deterministic, so
    // the default would in fact answer correctly — and it would answer
    // correctly *by holding the private key*, which is the habit this whole
    // module exists to break. Verification goes through the public key here
    // so that the signer's own verification path and a third party's are the
    // same path.
    fn verify(&self, bytes: &[u8], signature: &[u8]) -> bool {
        self.verifier().verify(bytes, signature)
    }
}

/// The verifying half: a key id and 32 public bytes. Everything a receiver
/// needs, and nothing that could sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointVerifier {
    key_id: String,
    verifying: VerifyingKey,
}

impl CheckpointVerifier {
    /// Build from published material: the key id the checkpoint will name,
    /// and 64 hex characters of public key.
    pub fn from_public_hex(
        key_id: impl Into<String>,
        hex: &str,
    ) -> Result<CheckpointVerifier, SignerError> {
        let bytes = hex32(hex, "public key")?;
        let verifying = VerifyingKey::from_bytes(&bytes)
            .map_err(|_| SignerError::BadKey("not a point on the curve".into()))?;
        Ok(CheckpointVerifier {
            key_id: key_id.into(),
            verifying,
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// The published form — what goes in a key-discovery document.
    pub fn public_hex(&self) -> String {
        hex_of(self.verifying.as_bytes())
    }

    /// Raw signature check over arbitrary bytes.
    pub fn verify(&self, bytes: &[u8], signature: &[u8]) -> bool {
        let Ok(sig) = <[u8; 64]>::try_from(signature) else {
            return false;
        };
        self.verifying
            .verify(bytes, &Signature::from_bytes(&sig))
            .is_ok()
    }

    /// Check a checkpoint end to end: the key it names must be the key we
    /// hold, and the signature must cover the space, position, head and
    /// instant it claims.
    ///
    /// The key-id check is not decoration. Without it a checkpoint signed by
    /// key A verifies under a verifier for key A even though the checkpoint
    /// says "B" — so a receiver that trusts B would accept an A-signed
    /// marker the moment it happened to be handed A's public key. Key
    /// discovery is part of verification, so the binding is checked here.
    pub fn verify_checkpoint(&self, cp: &Checkpoint) -> bool {
        cp.key_id == self.key_id
            && self.verify(
                &Checkpoint::signed_bytes(&cp.space, cp.upto, cp.head, cp.at),
                &cp.signature,
            )
    }
}

fn seed_from_hex(hex: &str) -> Result<[u8; 32], SignerError> {
    hex32(hex, "seed")
}

fn hex32(hex: &str, what: &str) -> Result<[u8; 32], SignerError> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(SignerError::BadKey(format!(
            "{what} must be 64 hex characters (32 bytes), got {}",
            hex.len()
        )));
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| SignerError::BadKey(format!("{what} is not hexadecimal")))?;
    }
    Ok(out)
}

fn hex_of(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}
