//! Verifying a single `ordo.attestation/1` delivery — the other half of what
//! a receiver needs.
//!
//! A webhook receiver has two jobs and they are separate. The **transport**
//! check (`X-Ordo-Signature`) says *this request came from the platform, for
//! me, recently*, and it is HMAC under the secret they were shown at grant;
//! that check belongs at their ingest and is a dozen lines they write in
//! whatever language their endpoint is in — `ordo_egress::sign::verify` is
//! the reference. The **payload** check says *this signed head is what it
//! claims*, and it is Ed25519 under a key they discover; that is this
//! function, and it is the one they will run months later over records they
//! stored, when the request is long gone.
//!
//! # Why `signedBytes` is in the payload, and checked here
//!
//! The payload carries both the four fields and their length-prefixed
//! preimage. That looks redundant and is not: reconstructing
//! `Checkpoint::signed_bytes` means getting a u64 big-endian length prefix on
//! the space name right, and a receiver who gets it wrong sees a **failed
//! signature**, which is indistinguishable from tampering and is the one
//! error nobody debugs correctly. Comparing against the published preimage
//! separates "I built the bytes wrong" from "the signature is bad", and this
//! function reports them as different findings for exactly that reason.

use ordo_commandlog::{ChainHash, CheckpointVerifier, SpaceId, Timestamp};

use crate::{hex, unhex};

/// What one delivery checked out as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationVerdict {
    pub space: String,
    pub upto: u64,
    pub head: String,
    pub at: u64,
    pub key_id: String,
    /// The owner-key fingerprint the platform published. **Compare this with
    /// what you recorded on the first delivery**: a change means the
    /// destination consent was re-signed by a different key, which is either
    /// the owner re-pointing the sink or the platform pretending they did.
    pub destination_key_id: String,
    /// The cadence, so silence becomes alarmable: this many commands, or this
    /// many seconds, whichever lands first.
    pub cadence_every: u64,
    pub cadence_or_secs: u64,
    pub structure_only: bool,
    pub findings: Vec<crate::Finding>,
}

impl AttestationVerdict {
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }

    pub fn code(&self) -> i32 {
        if self.ok() {
            0
        } else {
            3
        }
    }

    pub fn summary(&self) -> String {
        if !self.ok() {
            return format!(
                "FAILED: {} finding{} on the attestation for `{}` at {}",
                self.findings.len(),
                if self.findings.len() == 1 { "" } else { "s" },
                self.space,
                self.upto
            );
        }
        if self.structure_only {
            format!(
                "UNVERIFIED (structure only): `{}` claims head {} at position {}, signature \
                 NOT checked",
                self.space, self.head, self.upto
            )
        } else {
            format!(
                "VERIFIED: `{}` was at head {} through index {} at unix {} — pin this",
                self.space,
                self.head,
                self.upto.saturating_sub(1),
                self.at
            )
        }
    }

    /// The bound before which the next delivery is due. Silence past this is
    /// the signal §7 says it is.
    pub fn next_due_by(&self) -> u64 {
        self.at + self.cadence_or_secs
    }
}

/// Verify one `ordo.attestation/1` payload.
pub fn verify_attestation(
    doc: &str,
    trusted: Option<&CheckpointVerifier>,
) -> Result<AttestationVerdict, String> {
    let v: serde_json::Value = serde_json::from_str(doc).map_err(|e| format!("not JSON: {e}"))?;
    let mut findings = Vec::new();

    match v["schema"].as_str() {
        Some("ordo.attestation/1") => {}
        Some(other) => findings.push(crate::Finding {
            step: "schema",
            detail: format!(
                "this verifier reads `ordo.attestation/1`; the document says `{other}`"
            ),
        }),
        None => findings.push(crate::Finding {
            step: "schema",
            detail: "the document names no schema".into(),
        }),
    }

    let space = v["space"].as_str().ok_or("no `space`")?.to_string();
    let upto = v["upto"].as_u64().ok_or("no `upto`")?;
    let at = v["at"].as_u64().ok_or("no `at`")?;
    let head_hex = v["head"].as_str().ok_or("no `head`")?.to_string();
    let key_id = v["keyId"].as_str().unwrap_or("").to_string();
    let Some(head) = unhex(&head_hex).and_then(|b| <[u8; 32]>::try_from(b).ok()) else {
        return Err("`head` is not 32 bytes of hex".into());
    };
    let head = ChainHash(head);

    // The preimage, rebuilt from the four fields — and compared against the
    // published one, so "I built the bytes wrong" is a different finding from
    // "the signature is bad".
    let rebuilt = ordo_commandlog::Checkpoint::signed_bytes(
        &SpaceId(space.clone()),
        upto,
        head,
        Timestamp(at),
    );
    if let Some(published) = v["signedBytes"].as_str() {
        if !published.eq_ignore_ascii_case(&hex(&rebuilt)) {
            findings.push(crate::Finding {
                step: "preimage",
                detail: "the published `signedBytes` is not what (space, upto, head, at) \
                         encode to. Either a field was altered after signing, or this \
                         verifier and the signer disagree about the encoding — check the \
                         schema version before concluding tampering."
                    .into(),
            });
        }
    }

    if let Some(trusted) = trusted {
        if key_id != trusted.key_id() {
            findings.push(crate::Finding {
                step: "signature",
                detail: format!(
                    "this attestation is signed by key `{key_id}` and you supplied `{}`. A \
                     record signed by a key you do not trust is not evidence.",
                    trusted.key_id()
                ),
            });
        } else {
            match v["signature"].as_str().and_then(unhex) {
                Some(sig) if trusted.verify(&rebuilt, &sig) => {}
                Some(_) => findings.push(crate::Finding {
                    step: "signature",
                    detail: format!("the signature does not verify under key `{key_id}`"),
                }),
                None => findings.push(crate::Finding {
                    step: "signature",
                    detail: "no usable signature".into(),
                }),
            }
        }
    }

    Ok(AttestationVerdict {
        space,
        upto,
        head: head_hex,
        at,
        key_id,
        destination_key_id: v["destinationKeyId"].as_str().unwrap_or("").to_string(),
        cadence_every: v["cadence"]["everyCommands"].as_u64().unwrap_or(0),
        cadence_or_secs: v["cadence"]["orSeconds"].as_u64().unwrap_or(0),
        structure_only: trusted.is_none(),
        findings,
    })
}
