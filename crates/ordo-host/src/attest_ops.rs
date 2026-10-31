//! `std.space.attestTo` (layer 2's setup) and `std.space.chainProof`
//! (layer 4) — the two platform ops attestation needs.
//!
//! Both follow 8r's recipe exactly: a spec in [`crate::platform_ops::specs`],
//! a name constant here, a role in `owner_only`, a dispatch arm. Neither
//! mints a refusal class it did not have to; the two it does mint are
//! argued below.
//!
//! # `attestTo` is owner-only; `chainProof` is manager-and-owner
//!
//! `platform_ops.rs` states the rule these follow: *"the exit ops are the
//! owner's alone, because ending a space and disclosing all of it are not
//! administration"*, while `callersOf` is manager-and-owner because *"an
//! audit surface its own administrator cannot read pushes deployments to
//! hand out owner"*. The two new ops land on opposite sides of that line, and
//! the deciding question is **what each one discloses or decides**.
//!
//! - **`attestTo` decides where a space's history is published.** That is a
//!   custody decision of the same class as export: the owner nominates a
//!   third party who will hold signed statements about their chain forever.
//!   Owner-only.
//! - **`chainProof` discloses hashes.** Not rows, not images, not actors, not
//!   correlation ids — the artifact is positions, 32-byte digests and
//!   signatures. It is *strictly less* disclosing than `callersOf`, which
//!   names principals by id. Refusing it to the manager would mean the role
//!   that administers a space cannot check whether its history is intact,
//!   which is `callersOf`'s argument word for word. Manager-and-owner.
//!
//! Both are step-up gated, and that is not a choice this module makes:
//! `deployment_engine`'s registration loop calls `require_step_up` for
//! **every** platform spec, unconditionally. It is worth naming the
//! consequence, because it is the property that matters most for `attestTo`:
//! `AccessEngine::step_up_decision` denies outright when `claims.capability`
//! is present, so **a capability token can never nominate an attestation
//! destination.** The platform has exactly one mechanism for *not delegable*
//! and this op gets it for free.
//!
//! # Two refusal classes, and why not zero
//!
//! `platform_ops.rs` sets the default at zero new classes — *"a class minted
//! for 'you are not a manager' would itself be the oracle"*. These two ops
//! need refusals that no existing class says, and each has one producer:
//!
//! - **`egress_refused`** ← [`AttestOps::attest_to`], for a destination that
//!   fails one of §7b's destination pins. Nothing existing fits: it is not
//!   `invalid_param` (the value bound correctly as text), not
//!   `bound_violation` (it is not a length), and not `access_denied` (which
//!   an executor may not raise anyway). It is one class for all six pins on
//!   purpose — a class per pin would let anyone who can name a hostname map
//!   the deployment's internal address plan one grant at a time.
//! - **`proof_unavailable`** ← [`AttestOps::chain_proof`], for a range the
//!   deployment cannot prove: below the pruning floor, past the head, or a
//!   segment that does not recompute. Deliberately **not** `not_found`: a
//!   range whose entries were lawfully pruned is a range the deployment is
//!   telling the truth about, and collapsing it into "no such thing" would
//!   let a deployment answer a tampering question with a shrug. The three
//!   causes are distinguished in the message and share the class, because
//!   they share the caller's next move — narrow the range.
//!
//! An owner signature that does not verify reuses **`not_confirmed`**, whose
//! inventory row already reads *"a refusal after the value bound correctly,
//! made by the op because what the value says is not what the op requires it
//! to say"*. A signature that does not verify is exactly that, and a new
//! class would be a second word for one idea.

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_commandlog::{ChainHash, SpaceId as LogSpaceId};
use ordo_exec::cadence::LogPolicy;
use ordo_exec::route::PoolResolver;
use ordo_exec::webhook::{self, SinkRefusal};
use ordo_exec::Pool;
use serde_json::json;

/// `std.space.attestTo` — nominate the owner-signed destination for layer 2.
pub const ATTEST_TO: &str = "std.space.attestTo";
/// `std.space.chainProof` — the exportable, offline-verifiable segment.
pub const CHAIN_PROOF: &str = "std.space.chainProof";
/// `std.space.attestation`: the destination `attestTo` set, read back
/// (owner ruling 2026-10-05, `attest-sink-readback`). Never the secret.
pub const ATTESTATION: &str = "std.space.attestation";
/// `std.space.attestationDeadLetters`: the deliveries to that destination
/// that ran out of attempts (owner ruling 2026-10-05). Read only: there is
/// no retry yet.
pub const ATTESTATION_DEAD_LETTERS: &str = "std.space.attestationDeadLetters";

/// Every op of this module.
pub const OPS: [&str; 4] = [
    ATTEST_TO,
    CHAIN_PROOF,
    ATTESTATION,
    ATTESTATION_DEAD_LETTERS,
];

/// The published proof schema. Versioned and inside the document, so a third
/// party writes a verifier against a name rather than against a sample.
pub const PROOF_SCHEMA: &str = "ordo.chainproof/1";

/// The most links one proof will carry.
///
/// A bound rather than a page, and the difference is deliberate: a proof is a
/// **single artifact a third party verifies as a unit**, so paging it would
/// hand somebody two files and the job of stitching them, which is the job
/// the verifier exists to do correctly. Ten thousand links is about 1.3 MB of
/// hex — large for a response and small for a file — and a caller who needs
/// more asks for two ranges and verifies each against its own anchor, which
/// is a thing the verifier supports and a stitched proof would not be.
pub const MAX_LINKS: u64 = 10_000;

/// State the two ops need that the access engine does not have.
///
/// `Option` on [`crate::platform_ops::PlatformOps`] for the reason its module
/// doc gives about the grant registry: a deployment either wires this **and**
/// registers the specs, or wires neither.
pub struct AttestOps {
    /// Where attestation sinks are registered. Which database the sink
    /// table belongs in once spaces can live elsewhere is the platform
    /// table split's to decide (`plan-space-routing.md` S4); until then it
    /// is the control database.
    pub pool: Pool,
    /// Which database a space's chain is in. A proof reads the chain, and
    /// the chain lives with the space's rows.
    pub pools: Arc<dyn PoolResolver>,
    pub policy: LogPolicy,
    /// The security-event stream `attestTo` records into. `None` in an
    /// embedding with no stream — and then the record is the one thing
    /// missing, which is worth a deployment's attention.
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
    /// Mints the per-sink secret. Injectable so a test is deterministic; a
    /// deployment supplies 32 bytes of `getrandom`.
    pub mint_secret: Arc<dyn Fn() -> Vec<u8> + Send + Sync>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl AttestOps {
    fn record(&self, space: &str, caller: &str, op: &str, action: &str, correlation: &str) {
        let Some(stream) = &self.stream else {
            return;
        };
        stream.observe(
            ordo_secevent::Observation {
                space: Some(LogSpaceId(space.to_string())),
                subject: ordo_secevent::Subject::member(caller),
                op: op.to_string(),
                // Beside `space_exported` and `space_erased`: a decision
                // *about* a space, not a change to its rows, which is the
                // half of the spine `exit_ops.rs` established this belongs
                // to. The command log's taxonomy is Create/Converge/Delete
                // over entity rows, and nominating a destination is none of
                // those.
                kind: ordo_secevent::EventKind::CustodyAction {
                    action: action.to_string(),
                },
                at: ordo_commandlog::Timestamp((self.now)()),
                correlation: ordo_secevent::correlation::adopt(correlation),
            },
            ordo_secevent::Relationship::Member,
        );
    }

    /// `std.space.attestTo`.
    ///
    /// The order of operations is the security argument, and it is the same
    /// order the delivery worker uses:
    ///
    /// 1. parse the URL as `https` and **canonicalise it** — the canonical
    ///    form is what the owner signed, so the platform must derive it the
    ///    same way twice or an honest destination fails on round trip;
    /// 2. **verify the owner's signature over `(space, canonical)`** before
    ///    anything is resolved. A destination the owner did not sign is not a
    ///    destination this platform will look up;
    /// 3. resolve and pin against the deny list, so a destination that could
    ///    never be delivered to is refused at setup rather than dead-lettered
    ///    an hour later.
    ///
    /// Step 3 is a *grant-time* check and §7b requires it at both ends —
    /// *"resolve at grant time **and** again at every delivery"*. Passing it
    /// here is not a promise it will pass later, and the worker re-runs it;
    /// what it buys is that the owner learns at the moment they configure it.
    pub async fn attest_to(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        // **No destination removes the sink** (owner ruling 2026-10-05,
        // `attest-sink-readback`). The owner who chose a witness can end it,
        // with the same authority that chose it.
        let destination = match bound
            .0
            .get("destination")
            .and_then(|v| v.as_str())
            .map(str::trim)
        {
            None | Some("") => {
                // A removal is the four parameters left out together. A key
                // or a signature with no destination is a set that lost its
                // destination on the way, and spending it as a removal
                // would end a witness the owner meant to keep.
                let stray = ["ownerKeyId", "ownerKey", "signature"]
                    .into_iter()
                    .find(|k| {
                        bound
                            .0
                            .get(*k)
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| !s.trim().is_empty())
                    });
                if let Some(k) = stray {
                    return Err(ExecError::new(
                        ErrorCode::NotConfirmed,
                        format!(
                            "`${k}` was sent with no `$destination`, which confirms nothing. \
                             Send the destination with the owner's key and signature to set \
                             it, or leave all four out to remove it. Nothing changed"
                        ),
                    ));
                }
                return self.remove_sink(space, caller).await;
            }
            Some(d) => d,
        };
        // A destination is set only with the owner's key and signature over
        // it. `not_confirmed`, the class a missing or bad signature already
        // answers: what was sent does not confirm the destination.
        let text = |k: &str| -> Result<&str, ExecError> {
            bound
                .0
                .get(k)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ExecError::new(
                        ErrorCode::NotConfirmed,
                        format!(
                            "a destination is set only with the owner's key and signature; \
                             `${k}` is missing. Leave `$destination` out to remove the \
                             destination instead"
                        ),
                    )
                })
        };
        let owner_key_id = text("ownerKeyId")?;
        let owner_key = text("ownerKey")?;
        let signature_hex = text("signature")?;

        let url = ordo_egress::HttpsUrl::parse(destination).map_err(egress_refusal)?;
        let canonical = url.canonical();
        let signature = unhex(signature_hex).ok_or_else(|| {
            ExecError::new(
                ErrorCode::NotConfirmed,
                "the destination signature is not hex",
            )
        })?;

        let secret_bytes = (self.mint_secret)();
        let secret_hex = hex(&secret_bytes);
        let sid = LogSpaceId(space.to_string());
        let now = (self.now)();

        let sink = webhook::register_sink(
            &self.pool,
            &sid,
            &canonical,
            owner_key_id,
            owner_key,
            &signature,
            &secret_hex,
            now,
        )
        .await
        .map_err(sink_refusal)?;

        // Grant-time resolution, after the signature has been checked. A
        // failure here leaves the sink registered and reports the reason: the
        // owner signed a destination and the deployment cannot reach it,
        // which is a fact about the network rather than a reason to discard
        // the owner's decision. The worker will retry and dead-letter, and
        // the ops surface shows it.
        let allow = ordo_egress::Allowlist::exactly(url.host());
        let deny = deny_list()?;
        let reachable = match ordo_egress::resolve_and_pin(&url, &allow, &deny).await {
            Ok(_) => json!(true),
            Err(e) => json!(e.disclosable()),
        };

        self.record(
            space,
            caller,
            ATTEST_TO,
            "attestation_destination_set",
            &canonical,
        );

        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "space": space,
                "destination": sink.destination,
                "destinationKeyId": sink.owner_key_id,
                // §7b: the per-sink secret is **shown once at grant**. There
                // is no op that reads it back, and the storage column is not
                // in any output. A receiver who loses it re-runs `attestTo`,
                // which is a fresh secret and a fresh consent.
                "secret": secret_hex,
                // Where the series starts. §7's "you can only verify from the
                // point you started watching", as a number the owner can see.
                "watchingFrom": sink.cursor,
                "reachable": reachable,
                "schema": webhook::PAYLOAD_SCHEMA,
                "outcome": "set",
            })],
            next_cursor: None,
        })
    }

    /// `attestTo` with no destination: the sink is deregistered, with the
    /// deliveries still waiting for it, and the act is recorded as the
    /// setting was. No sink is `unchanged` and records nothing, so a retry
    /// is safe. Dead letters stay: they are the record of what failed.
    async fn remove_sink(&self, space: &str, caller: &str) -> Result<ExecResult, ExecError> {
        let sid = LogSpaceId(space.to_string());
        webhook::ensure_tables(&self.pool)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        let was = webhook::read_sink(&self.pool, &sid)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        let removed = webhook::deregister_sink(&self.pool, &sid)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        if removed {
            let destination = was.as_ref().map(|s| s.destination.as_str()).unwrap_or("");
            self.record(
                space,
                caller,
                ATTEST_TO,
                "attestation_destination_removed",
                destination,
            );
        }
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "space": space,
                "destination": null,
                "destinationKeyId": null,
                "secret": null,
                "watchingFrom": null,
                "reachable": null,
                "schema": webhook::PAYLOAD_SCHEMA,
                "outcome": if removed { "removed" } else { "unchanged" },
            })],
            next_cursor: None,
        })
    }

    /// `std.space.attestation`: the destination set for this space, or a
    /// row of nulls when there is none. Never the secret: §7b shows it once,
    /// at grant. `deadLetters` is how many deliveries gave up, which
    /// `std.space.attestationDeadLetters` lists.
    pub async fn attestation(&self, space: &str) -> Result<ExecResult, ExecError> {
        let sid = LogSpaceId(space.to_string());
        webhook::ensure_tables(&self.pool)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        let sink = webhook::read_sink(&self.pool, &sid)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        let dead = webhook::dead_letters(&self.pool, &sid)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?
            .len();
        let row = match sink {
            Some(s) => json!({
                "space": space,
                "destination": s.destination,
                "destinationKeyId": s.owner_key_id,
                "setAt": s.created_at,
                "watchingFrom": s.cursor,
                "deadLetters": dead,
            }),
            None => json!({
                "space": space,
                "destination": null,
                "destinationKeyId": null,
                "setAt": null,
                "watchingFrom": null,
                "deadLetters": dead,
            }),
        };
        Ok(ExecResult {
            position: None,
            rows: vec![row],
            next_cursor: None,
        })
    }

    /// `std.space.attestationDeadLetters`: one row per delivery that ran out
    /// of attempts, oldest position first.
    ///
    /// The tenant owner is shown the **kind** of failure, never the stored
    /// reason. The reason is written for the deployment owner
    /// (`ordo_egress::Refusal::operational`) and can carry the address a
    /// name resolved to and the transport's own words, which describe the
    /// deployment's network. The supervisor overview shows the reason.
    pub async fn dead_letters(&self, space: &str) -> Result<ExecResult, ExecError> {
        let sid = LogSpaceId(space.to_string());
        webhook::ensure_tables(&self.pool)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
        let rows = webhook::dead_letters(&self.pool, &sid)
            .await
            .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?
            .into_iter()
            .map(|d| {
                json!({
                    "upto": d.upto,
                    "destination": d.destination,
                    "attempts": d.attempts,
                    "at": d.at,
                    "kind": failure_kind(&d.reason),
                })
            })
            .collect();
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }

    /// `std.space.chainProof($from, $to)`.
    ///
    /// # What the artifact contains, and what it deliberately does not
    ///
    /// `implementation.md` specifies *"head hashes, signed checkpoints, and
    /// per-group hashes"*. Per-group **hashes** — not bodies, and that is the
    /// design rather than an economy:
    ///
    /// - it is what a third party needs. Verification is *"the head I pinned
    ///   at time T is still what this chain says at that position"*, and that
    ///   is a question about links and signatures;
    /// - the bodies are already exportable. `std.space.export` ships
    ///   `commandlog.jsonl` with every group's content, under the owner's
    ///   authority and with a disclosure record. A second full-content path
    ///   with a different gate would be the weaker of two doors to the same
    ///   room;
    /// - so a chain proof discloses **no space content**, which is what lets
    ///   it be manager-reachable and what makes it safe to hand to the third
    ///   party it exists for.
    ///
    /// The limit that follows, stated so nobody discovers it: a verifier
    /// checking this artifact confirms the chain **links** and the
    /// **signatures**; it does not confirm that a hash corresponds to any
    /// particular content, because it has not been given the content. To
    /// check content, verify a bundle. The published verifier says so on
    /// every run.
    ///
    /// # The encoding agrees with the export bundle
    ///
    /// Each link is `{"idx", "prev", "hash"}` with lowercase-hex digests —
    /// the same three keys, the same strings, that `export.rs`'s
    /// `group_json` puts on each `commandlog.jsonl` line. A proof and a
    /// bundle taken from one deployment at one position are therefore
    /// comparable field by field with no conversion, which is the property
    /// `plan-production-ops.md`'s R-track wants where a restore marker and a
    /// chain proof overlap. `attest_ops::tests` asserts it rather than
    /// asserting it in prose.
    pub async fn chain_proof(
        &self,
        space: &str,
        bound: &BoundParams,
        _caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let num =
            |k: &str| -> Result<i64, ExecError> {
                bound.0.get(k).and_then(|v| v.as_i64()).ok_or_else(|| {
                    ExecError::internal(format!("`${k}` did not reach the platform op"))
                })
            };
        let from = num("from")?;
        let to = num("to")?;
        if from < 0 || to < 0 {
            return Err(ExecError::new(
                ErrorCode::ProofUnavailable,
                "a chain position is not negative",
            ));
        }
        let (from, to) = (from as u64, to as u64);
        if to < from {
            return Err(ExecError::new(
                ErrorCode::ProofUnavailable,
                format!("`from` is {from} and `to` is {to}; the range runs forwards"),
            ));
        }
        if to - from + 1 > MAX_LINKS {
            return Err(ExecError::new(
                ErrorCode::ProofUnavailable,
                format!(
                    "a proof carries at most {MAX_LINKS} links so that it stays one artifact \
                     a verifier checks as a unit; ask for two ranges"
                ),
            ));
        }

        let sid = LogSpaceId(space.to_string());
        let pool = self
            .pools
            .resolve(&ordo_crypto::SpaceId(space.to_string()))
            .await
            .map_err(|e| {
                ExecError::of_fault(
                    e.class(),
                    ErrorCode::Internal,
                    format!("the space's database: {e}"),
                )
            })?;
        let doc = build_proof(&pool, &sid, from, to).await?;
        // Serialised **here**, and the string is what the caller receives.
        // A structured value would be re-serialised by whatever renders the
        // response, and a verifier consuming re-serialised JSON is a verifier
        // whose input depends on a client library's key ordering. The bytes
        // that were built are the bytes that are checked.
        let proof = serde_json::to_string(&doc)
            .map_err(|e| ExecError::internal(format!("rendering the proof: {e}")))?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "space": space,
                "from": from,
                "to": to,
                "links": doc["links"].as_array().map(|a| a.len()).unwrap_or(0),
                "anchors": doc["anchors"].as_array().map(|a| a.len()).unwrap_or(0),
                "head": doc["head"],
                "proof": proof,
            })],
            next_cursor: None,
        })
    }
}

/// Build the proof document.
///
/// Split out so `tools/verify-chainproof`'s round-trip test can build one
/// from a real chain without a door in front of it.
pub async fn build_proof(
    pool: &Pool,
    space: &LogSpaceId,
    from: u64,
    to: u64,
) -> Result<serde_json::Value, ExecError> {
    let store =
        ordo_exec::commandlog::PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let floor = ordo_exec::cadence::verifiable_from(pool, space)
        .await
        .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;

    let entries = store
        .try_entries(space, from as usize, (to - from + 1) as usize)
        .await
        .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?;
    if entries.is_empty() {
        return Err(ExecError::new(
            ErrorCode::ProofUnavailable,
            format!(
                "nothing is retained at position {from}; this chain is verifiable from \
                 {floor} — a range below the pruning floor is a range this deployment \
                 cannot prove, which is a different answer from `no such thing`"
            ),
        ));
    }
    if (entries.len() as u64) < to - from + 1 {
        return Err(ExecError::new(
            ErrorCode::ProofUnavailable,
            format!(
                "the chain holds {} entries from {from}, not {}; `to` is past the head",
                entries.len(),
                to - from + 1
            ),
        ));
    }

    // The anchor the segment is checked against. Genesis when the range
    // starts at the beginning; otherwise the `prev` of the first entry, which
    // is a *claim* the platform makes and which the checkpoints below are
    // what turn into evidence. Stated in the document rather than implied, so
    // a verifier reading a mid-chain proof knows what it is trusting.
    let anchor = entries[0].prev;

    // The platform verifies its own segment before publishing it. This proves
    // nothing to a third party — that is the whole point of the exercise —
    // but it stops the deployment handing out a proof over a chain that does
    // not recompute locally, which would be a proof of the wrong thing
    // arriving with a confident schema on it.
    if let Err(t) = ordo_commandlog::verify_chain_from(space, &entries, anchor, from as usize) {
        return Err(ExecError::new(
            ErrorCode::ProofUnavailable,
            format!(
                "this deployment's own chain does not recompute at index {} ({:?}); \
                 refusing to publish a proof over it",
                t.index, t.kind
            ),
        ));
    }

    let links: Vec<serde_json::Value> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "idx": from as usize + i,
                "prev": e.prev.to_string(),
                "hash": e.hash.to_string(),
            })
        })
        .collect();

    // Every signed marker whose position falls inside the segment, from both
    // artifacts. `cadence.rs` keeps checkpoints and attestations in separate
    // tables and separate types on purpose — a checkpoint is the only legal
    // prune anchor — and this is the one place they are deliberately read
    // together, because for a *verifier* they are the same evidence: the same
    // signed bytes, checked by the same routine. The `kind` field keeps them
    // distinguishable so nobody can use this document to argue an attestation
    // is a cut point.
    let mut anchors = Vec::new();
    if let Ok(Some(cp)) = store.try_latest_checkpoint(space).await {
        if cp.upto > from && cp.upto <= to + 1 {
            anchors.push(json!({
                "kind": "checkpoint",
                "upto": cp.upto,
                "head": cp.head.to_string(),
                "at": cp.at.0,
                "keyId": cp.key_id,
                "signature": hex(&cp.signature),
            }));
        }
    }
    for a in ordo_exec::cadence::attestations(pool, space)
        .await
        .map_err(|e| ExecError::of_fault(e.class(), ErrorCode::Internal, e.to_string()))?
    {
        if a.upto > from && a.upto <= to + 1 {
            anchors.push(json!({
                "kind": "attestation",
                "upto": a.upto,
                "head": a.head.to_string(),
                "at": a.at.0,
                "keyId": a.key_id,
                "signature": hex(&a.signature),
            }));
        }
    }

    Ok(json!({
        "schema": PROOF_SCHEMA,
        "space": space.0,
        "from": from,
        "to": to,
        "verifiableFrom": floor,
        // What the first link's `prev` must equal. `genesis` when it is 32
        // zero bytes, which the verifier checks rather than takes on trust.
        "anchor": anchor.to_string(),
        "genesis": anchor == ChainHash::GENESIS,
        "head": entries[entries.len() - 1].hash.to_string(),
        "links": links,
        "anchors": anchors,
    }))
}

/// The stable word for why a delivery failed, from a stored reason.
///
/// An egress refusal is stored as `<kind>: <detail>`, and the kind is one
/// of `ordo_egress::Refusal::kind`'s fixed words. Anything else is the
/// drain's own fault (`attestdrain::Fault`) and is answered as one word.
pub fn failure_kind(reason: &str) -> &'static str {
    const KINDS: [&str; 11] = [
        "not_https",
        "malformed_url",
        "not_allowlisted",
        "unresolvable",
        "denied_address",
        "redirect_answered",
        "tls_failed",
        "connect_failed",
        "timed_out",
        "rejected",
        "malformed_response",
    ];
    let head = reason.split(':').next().unwrap_or("").trim();
    KINDS
        .iter()
        .find(|k| **k == head)
        .copied()
        .unwrap_or("delivery_failed")
}

/// The deny list from the environment, as the delivery worker builds it.
fn deny_list() -> Result<ordo_egress::DenyList, ExecError> {
    let cidrs = match std::env::var("ORDO_EGRESS_DENY_CIDRS") {
        Ok(s) => ordo_egress::DenyList::parse_cidrs(&s).map_err(|e| {
            ExecError::internal(format!("ORDO_EGRESS_DENY_CIDRS is not a CIDR list: {e}"))
        })?,
        Err(_) => Vec::new(),
    };
    Ok(ordo_egress::DenyList::categories_only().with_deployment_cidrs(&cidrs))
}

/// The one producer of `egress_refused`.
fn egress_refusal(e: ordo_egress::Refusal) -> ExecError {
    ExecError::new(ErrorCode::EgressRefused, e.disclosable())
}

fn sink_refusal(e: SinkRefusal) -> ExecError {
    match e {
        // The owner's signature did not verify. `not_confirmed`, whose
        // inventory row already covers "a refusal after the value bound
        // correctly, made by the op because what the value says is not what
        // the op requires it to say".
        SinkRefusal::NotSigned => ExecError::new(ErrorCode::NotConfirmed, e.to_string()),
        SinkRefusal::BadOwnerKey(_) => ExecError::new(ErrorCode::NotConfirmed, e.to_string()),
        SinkRefusal::NoSink => ExecError::internal(e.to_string()),
        SinkRefusal::Egress(d) => ExecError::internal(d),
        _ => ExecError::internal(e.to_string()),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || s.is_empty() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}
