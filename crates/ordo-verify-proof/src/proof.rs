//! Verifying an `ordo.chainproof/1` document.
//!
//! The procedure, in the order it runs, and each step is a thing that can
//! fail on its own so that a report names *which* step:
//!
//! 1. **Schema.** The document says `ordo.chainproof/1`. A verifier that
//!    guesses at an unknown version is a verifier that will one day report
//!    "verified" about a document it did not understand.
//! 2. **Range.** `links` covers `from..=to` with no gap and no repeat, and
//!    every index is exactly one more than the last. A missing index in the
//!    middle is the shape a deleted group leaves.
//! 3. **Anchor.** `links[0].prev` equals the document's stated `anchor`, and
//!    if `genesis` is claimed, that anchor is 32 zero bytes. A proof that
//!    claims genesis and does not start at it is claiming a stronger thing
//!    than it has.
//! 4. **Links.** `links[i].prev == links[i-1].hash` for every `i`. This is
//!    the chain.
//! 5. **Head.** the document's `head` equals the last link's hash.
//! 6. **Markers.** every entry in `anchors` sits at a position inside the
//!    range, its `head` equals the link at that position, and its signature
//!    verifies over `Checkpoint::signed_bytes(space, upto, head, at)` under
//!    the trusted key.
//!
//! # The position convention, which is the one thing everybody gets wrong
//!
//! A checkpoint's `upto` is **one past** the index it covers: `upto = N`
//! means "the chain through index `N-1`", and its `head` is `links[N-1].hash`.
//! That is `Checkpoint`'s own convention (`upto` is `idx + 1` of the last
//! entry, read from the index and never from a count) and it is stated here
//! because an off-by-one in a verifier does not report an off-by-one — it
//! reports *tampering*, which is the most expensive possible way to be
//! wrong.

use ordo_commandlog::{ChainHash, CheckpointVerifier, SpaceId, Timestamp};

use crate::{hex, unhex};

/// One thing that was checked and did not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Which step, as a stable word: `schema`, `range`, `anchor`, `link`,
    /// `head`, `marker`, `signature`.
    pub step: &'static str,
    pub detail: String,
}

impl Finding {
    fn new(step: &'static str, detail: impl Into<String>) -> Finding {
        Finding {
            step,
            detail: detail.into(),
        }
    }
}

/// What a run concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub space: String,
    pub from: u64,
    pub to: u64,
    pub links: usize,
    /// Signed markers found inside the range.
    pub markers: usize,
    /// Markers whose signature verified under the supplied key.
    pub verified_markers: usize,
    /// No key was supplied, so signatures were not checked against anything
    /// trustworthy. The summary says "unverified" when this is true.
    pub structure_only: bool,
    pub findings: Vec<Finding>,
    /// The head this proof ends at — the value to pin, or to compare against
    /// one already pinned.
    pub head: String,
    /// The chain's first retained index, as the deployment reported it.
    /// **Outside every signature**, exactly as layer 1's `verifiable-from`
    /// is: an unsigned floor cannot make anybody *accept* anything, so it is
    /// an instruction about feasibility rather than a claim.
    pub verifiable_from: u64,
}

impl Verdict {
    /// Did every check pass?
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }

    /// The exit code. **3 for a failed verification, separate from 1 for "I
    /// could not read it"** — `ordo-cli`'s convention, adopted deliberately
    /// rather than reinvented: *"the chain at index 41 does not recompute"*
    /// and *"I could not parse the file"* must page different people, and a
    /// cron job that only knows `!= 0` will treat a bad download as evidence
    /// of tampering, cry wolf once, and be switched off.
    pub fn code(&self) -> i32 {
        if self.ok() {
            0
        } else {
            3
        }
    }

    /// What this run did **not** establish, printed on success as well as on
    /// failure. See the crate doc: a verifier that mentions its limits only
    /// when it fails has taught its reader to skip them.
    pub fn caveats(&self) -> Vec<String> {
        let mut out = vec![
            "A proof carries digests, not command bodies, so this run did not check that \
             any hash corresponds to any particular content. To check content, verify an \
             export bundle."
                .to_string(),
            "Verification runs forward from an anchor. It cannot tell you about history \
             before the point you started watching — no scheme can."
                .to_string(),
        ];
        if self.structure_only {
            out.insert(
                0,
                "NO KEY WAS SUPPLIED. Signatures were not checked. The key inside a proof \
                 is the platform's claim about its own key, and checking a signature \
                 against a key the signer chose proves nothing. Re-run with --key."
                    .to_string(),
            );
        }
        if self.markers == 0 {
            out.push(
                "This range contains no signed checkpoint or attestation, so nothing in it \
                 is anchored to a time. It shows an unbroken sequence of links and nothing \
                 more; ask for a range that spans one."
                    .to_string(),
            );
        }
        out
    }

    /// The human summary, one line.
    pub fn summary(&self) -> String {
        if !self.ok() {
            return format!(
                "FAILED: {} finding{} over {} links of `{}` [{}..={}]",
                self.findings.len(),
                if self.findings.len() == 1 { "" } else { "s" },
                self.links,
                self.space,
                self.from,
                self.to
            );
        }
        if self.structure_only {
            format!(
                "UNVERIFIED (structure only): {} links of `{}` [{}..={}] are unbroken, \
                 {} signed marker{} present and NOT checked",
                self.links,
                self.space,
                self.from,
                self.to,
                self.markers,
                if self.markers == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "VERIFIED: {} links of `{}` [{}..={}] are unbroken, {} signed marker{} \
                 checked, head {}",
                self.links,
                self.space,
                self.from,
                self.to,
                self.verified_markers,
                if self.verified_markers == 1 { "" } else { "s" },
                self.head
            )
        }
    }
}

/// Verify a proof document.
///
/// `trusted` is the key the *recipient* holds — from an earlier attestation,
/// the deployment's published key, or a key-discovery document. `None` runs
/// every structural check and skips every signature, reporting
/// [`Verdict::structure_only`].
///
/// `expect_head` is an optional head the recipient pinned earlier: supplying
/// it turns "these links are internally consistent" into "and they still end
/// where you last saw them", which is the check that actually detects a
/// rewrite.
pub fn verify_proof(
    doc: &str,
    trusted: Option<&CheckpointVerifier>,
    expect_head: Option<&str>,
) -> Result<Verdict, String> {
    let v: serde_json::Value = serde_json::from_str(doc).map_err(|e| format!("not JSON: {e}"))?;

    let space = v["space"]
        .as_str()
        .ok_or("the proof has no `space`")?
        .to_string();
    let from = v["from"].as_u64().ok_or("the proof has no `from`")?;
    let to = v["to"].as_u64().ok_or("the proof has no `to`")?;
    let verifiable_from = v["verifiableFrom"].as_u64().unwrap_or(0);

    let mut findings = Vec::new();

    // 1 — schema.
    match v["schema"].as_str() {
        Some("ordo.chainproof/1") => {}
        Some(other) => findings.push(Finding::new(
            "schema",
            format!("this verifier reads `ordo.chainproof/1`; the document says `{other}`"),
        )),
        None => findings.push(Finding::new("schema", "the document names no schema")),
    }

    let links = v["links"]
        .as_array()
        .ok_or("the proof has no `links` array")?;

    // 2 — range.
    let want = to.saturating_sub(from) + 1;
    if links.len() as u64 != want {
        findings.push(Finding::new(
            "range",
            format!(
                "the range {from}..={to} needs {want} links and the document carries {}",
                links.len()
            ),
        ));
    }
    let mut hashes: Vec<ChainHash> = Vec::with_capacity(links.len());
    let mut prevs: Vec<ChainHash> = Vec::with_capacity(links.len());
    for (i, l) in links.iter().enumerate() {
        let idx = l["idx"].as_u64();
        if idx != Some(from + i as u64) {
            findings.push(Finding::new(
                "range",
                format!(
                    "link {i} says index {:?}; a contiguous segment from {from} needs {}",
                    idx,
                    from + i as u64
                ),
            ));
        }
        let Some(p) = l["prev"].as_str().and_then(chain_hash) else {
            findings.push(Finding::new(
                "range",
                format!("link {i} has no usable `prev`"),
            ));
            return Ok(bail(
                space,
                from,
                to,
                links.len(),
                verifiable_from,
                trusted,
                findings,
            ));
        };
        let Some(h) = l["hash"].as_str().and_then(chain_hash) else {
            findings.push(Finding::new(
                "range",
                format!("link {i} has no usable `hash`"),
            ));
            return Ok(bail(
                space,
                from,
                to,
                links.len(),
                verifiable_from,
                trusted,
                findings,
            ));
        };
        prevs.push(p);
        hashes.push(h);
    }
    if hashes.is_empty() {
        findings.push(Finding::new("range", "the proof carries no links"));
        return Ok(bail(space, from, to, 0, verifiable_from, trusted, findings));
    }

    // 3 — anchor.
    let anchor = v["anchor"].as_str().and_then(chain_hash);
    match anchor {
        Some(a) if a == prevs[0] => {
            if v["genesis"].as_bool() == Some(true) && a != ChainHash::GENESIS {
                findings.push(Finding::new(
                    "anchor",
                    "the proof claims to start at genesis and its anchor is not 32 zero bytes",
                ));
            }
            if v["genesis"].as_bool() != Some(true) && a == ChainHash::GENESIS {
                findings.push(Finding::new(
                    "anchor",
                    "the anchor is 32 zero bytes but the proof does not claim genesis",
                ));
            }
        }
        Some(_) => findings.push(Finding::new(
            "anchor",
            "the first link's `prev` is not the anchor the document names",
        )),
        None => findings.push(Finding::new(
            "anchor",
            "the document names no usable anchor",
        )),
    }

    // 4 — links. The chain.
    for i in 1..hashes.len() {
        if prevs[i] != hashes[i - 1] {
            findings.push(Finding::new(
                "link",
                format!(
                    "index {} does not link to {}: its `prev` is {} and the previous \
                     `hash` is {}",
                    from + i as u64,
                    from + i as u64 - 1,
                    prevs[i],
                    hashes[i - 1]
                ),
            ));
        }
    }

    // 5 — head.
    let head = hashes[hashes.len() - 1];
    if v["head"].as_str().and_then(chain_hash) != Some(head) {
        findings.push(Finding::new(
            "head",
            "the document's `head` is not the last link's hash",
        ));
    }
    if let Some(pinned) = expect_head {
        if !pinned.eq_ignore_ascii_case(&head.to_string()) {
            findings.push(Finding::new(
                "head",
                format!(
                    "this segment ends at {head}, and you pinned {pinned}. If your pin \
                     is at this same position, the history has been rewritten."
                ),
            ));
        }
    }

    // 6 — markers.
    let sid = SpaceId(space.clone());
    let empty = Vec::new();
    let markers = v["anchors"].as_array().unwrap_or(&empty);
    let mut verified = 0usize;
    for (i, m) in markers.iter().enumerate() {
        let Some(upto) = m["upto"].as_u64() else {
            findings.push(Finding::new("marker", format!("marker {i} has no `upto`")));
            continue;
        };
        // The convention: `upto = N` covers through index `N-1`.
        if upto == 0 || upto - 1 < from || upto - 1 > to {
            findings.push(Finding::new(
                "marker",
                format!("marker {i} is at position {upto}, outside {from}..={to}"),
            ));
            continue;
        }
        let at_link = hashes[(upto - 1 - from) as usize];
        let Some(mhead) = m["head"].as_str().and_then(chain_hash) else {
            findings.push(Finding::new(
                "marker",
                format!("marker {i} has no usable `head`"),
            ));
            continue;
        };
        if mhead != at_link {
            findings.push(Finding::new(
                "marker",
                format!(
                    "the {} at position {upto} was signed over head {mhead}, and index {} \
                     of this segment hashes to {at_link}. One of the two has been changed.",
                    m["kind"].as_str().unwrap_or("marker"),
                    upto - 1
                ),
            ));
            continue;
        }
        let Some(trusted) = trusted else { continue };
        let at = m["at"].as_u64().unwrap_or(0);
        let key_id = m["keyId"].as_str().unwrap_or("");
        if key_id != trusted.key_id() {
            findings.push(Finding::new(
                "signature",
                format!(
                    "marker {i} is signed by key `{key_id}` and you supplied `{}`. A \
                     record signed by a key you do not trust is not evidence, however \
                     valid its signature.",
                    trusted.key_id()
                ),
            ));
            continue;
        }
        let Some(sig) = m["signature"].as_str().and_then(unhex) else {
            findings.push(Finding::new(
                "signature",
                format!("marker {i} has no usable signature"),
            ));
            continue;
        };
        // The same routine the platform signs with, over the same bytes a
        // checkpoint and an attestation share. One verifier, both artifacts,
        // which is what §7's documentation obligations require.
        let bytes = ordo_commandlog::Checkpoint::signed_bytes(&sid, upto, mhead, Timestamp(at));
        if trusted.verify(&bytes, &sig) {
            verified += 1;
        } else {
            findings.push(Finding::new(
                "signature",
                format!(
                    "the {} at position {upto} does not verify under key `{}`",
                    m["kind"].as_str().unwrap_or("marker"),
                    trusted.key_id()
                ),
            ));
        }
    }

    Ok(Verdict {
        space,
        from,
        to,
        links: links.len(),
        markers: markers.len(),
        verified_markers: verified,
        structure_only: trusted.is_none(),
        findings,
        head: head.to_string(),
        verifiable_from,
    })
}

#[allow(clippy::too_many_arguments)]
fn bail(
    space: String,
    from: u64,
    to: u64,
    links: usize,
    verifiable_from: u64,
    trusted: Option<&CheckpointVerifier>,
    findings: Vec<Finding>,
) -> Verdict {
    Verdict {
        space,
        from,
        to,
        links,
        markers: 0,
        verified_markers: 0,
        structure_only: trusted.is_none(),
        findings,
        head: String::new(),
        verifiable_from,
    }
}

fn chain_hash(s: &str) -> Option<ChainHash> {
    Some(ChainHash(<[u8; 32]>::try_from(unhex(s)?).ok()?))
}

/// The bytes an owner signs to nominate an attestation destination.
///
/// Published here, in the binary a third party already has, because the
/// signing happens **off** the platform: an owner runs this to learn what to
/// sign, signs it with a key the deployment never sees, and passes the
/// signature to `std.space.attestTo`. Without this the ceremony would require
/// the owner to reimplement a length-prefixed encoding from prose, which is
/// how an owner ends up signing the wrong bytes and concluding the platform
/// is broken.
///
/// Byte-identical to `ordo_exec::webhook::consent_bytes`, and
/// `ordo-host/tests/attest_webhook.rs` asserts the two agree — a second
/// implementation of a signed encoding is only safe if something compares
/// them.
pub fn consent_bytes(space: &str, canonical_url: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for part in [
        "ordo.attest-destination/1".as_bytes(),
        space.as_bytes(),
        canonical_url.as_bytes(),
    ] {
        out.extend_from_slice(&(part.len() as u64).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// Render [`consent_bytes`] as hex, which is what an owner signs.
pub fn consent_hex(space: &str, canonical_url: &str) -> String {
    hex(&consent_bytes(space, canonical_url))
}
