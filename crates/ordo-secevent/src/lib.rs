//! **F3 — the security-event stream.** The record of who *tried* what, on
//! the same machinery as the command chain and pointed at a different table,
//! with its own key and its own retention.
//!
//! Design companions: `security-logging-design.md` §6.1 (settled: event
//! kinds, the members-only rule, collapse, retention), `engine-plan.md` F3
//! (the four rules that are easy to build wrong), `implementation.md`
//! (Observability: three artifacts, three retentions, one correlation id).
//!
//! # Why this exists at all
//!
//! Ordo has one log today — the command chain — and it is **revertible by
//! design**: `revert_group` inverts a group and the inverse is applied
//! through the normal gate as new history. That is the right property for a
//! record of domain changes and the wrong one for a record of security
//! events, and the sentence the design keeps returning to is the whole
//! argument:
//!
//! > A security record that a domain revert can rewrite is not a security
//! > record.
//!
//! A revert cannot rewrite the command chain's past either — history is
//! append-only there too — but a revert *is* a domain operation that a space
//! member with write authority can perform, and a denial living in that
//! chain would be a denial living in a structure that the person being
//! denied may be able to grow, prune, and attest over. The separation is
//! about custody and retention, not about immutability.
//!
//! Four more reasons, from §6.1, each fatal alone: an outsider must never
//! write to a space's record; a denial must not contend for the write path's
//! per-space append lock; a rate-collapsed count cannot be incremented
//! inside an already-hashed entry; and entries cannot age out of a hash
//! chain one at a time.
//!
//! # The shape
//!
//! ```text
//!   door / custody / directory
//!            │  Observation { space?, subject, op, kind, at, correlation }
//!            ▼
//!      Recorder ── route ─► Scope::Space(spc_…)   (members only)
//!            │              Scope::Deployment     (everyone else)
//!            │
//!            ├─ denials ──► Collapser   (open window: ordinary mutable state)
//!            └─ the rest ─► pending
//!            │
//!            ▼  drain, off the request path
//!      StreamEntry ─ encode ─► SHA-256(prev ‖ body) ─► SecurityStore
//!            │
//!            ▼
//!      StreamCheckpoint (its own key, its own domain tag)
//!            │
//!            ▼
//!      Retention: drop whole prefixes ending at a checkpoint
//! ```
//!
//! What is shared with `ordo-commandlog` and what is not, precisely: the
//! [`ordo_commandlog::ChainHash`] type, genesis, the
//! `hash = SHA-256(prev ‖ body)` rule, the `TamperAt`/`TamperKind`
//! verification vocabulary, the `Signer` trait and its Ed25519
//! implementation, and the `Actor` attribution taxonomy. Not shared: the
//! body layout (domain-tagged, see [`chain::DOMAIN`]), the table, the lock,
//! the key, the retention policy, and the append path.
//!
//! # The four decisions, in one place
//!
//! 1. **Separately keyed.** A second `Ed25519Signer` from
//!    `ORDO_SECURITY_CHECKPOINT_KEY`, never the command chain's
//!    `ORDO_CHECKPOINT_KEY`, plus a domain tag in the signed bytes so the
//!    two checkpoint families are different messages even under one key.
//!    Argued in [`store`].
//! 2. **Separately retained.** [`store::Retention`] is its own number, and
//!    pruning drops whole prefixes ending at a verified checkpoint —
//!    verification then resumes from that checkpoint. Argued in [`store`].
//! 3. **Per space *or* deployment-wide, decided by routing** rather than by
//!    configuration, because the events do not all have a space. Argued in
//!    [`recorder`].
//! 4. **An unverifiable presentation names no principal.**
//!    [`event::Subject::Unverified`] has no payload — not a claimed subject,
//!    not a token fingerprint. Argued in [`event`].
//!
//! # What a deployment must still supply
//!
//! - **A durable store.** [`store::InMemorySecurityStore`] is the reference
//!   implementation; `ordo_exec::secevent::PgSecurityStore` is the durable
//!   one, and it lives in `ordo-exec` for the layering reason 8e settled —
//!   the leaf crate does not pull a database driver.
//! - **A drainer.** [`recorder::Recorder`] accumulates and nothing in this
//!   crate appends on its own. §6.1 pins that appends run off the request
//!   path; `ordo_host::secdrain::SecurityDrain` is the one this deployment
//!   runs — a background task on the same shape as the invalidation
//!   listener, calling [`recorder::Recorder::drain_ready`] on a tick and
//!   handing anything it could not commit back through
//!   [`recorder::Recorder::requeue`]. An embedder that wires none is not
//!   silently unbounded any more: the outbox has a hard cap and declares
//!   what it refuses (see [`recorder`]), but nothing is durable and a
//!   restart still loses the buffer, so wiring one is the deployment's
//!   step and not an optional polish.
//! - **A checkpoint cadence and a retention number.** Both are policy, and
//!   they are **two** rows in the ordinary schedule table with two periods
//!   (`secevent.checkpoint` and `secevent.retention`), not one. A marker is
//!   the only thing that can show entries were removed from the *end* of a
//!   stream, so how often one is signed is an evidence question; how much
//!   history to keep is a storage question. Wiring them together would let
//!   a deployment that keeps everything forever — a perfectly ordinary
//!   retention choice — silently lose truncation detection. Both are driven
//!   by the drainer above with the security checkpoint key: the set
//!   `ORDO_SECURITY_CHECKPOINT_KEY`, or the one `ordo_host::security_key`
//!   made at first boot and sealed in the control database. With neither
//!   it reports which of the two is inert.
//! - **The alerting stack.** [`alert`] evaluates one process's counters;
//!   what it cannot do — cross-replica aggregation above all — is listed
//!   there.
//! - **The deployment security log.** `implementation.md` describes it as
//!   ordinary structured logging. Here, deployment-scope events go into the
//!   same chained structure as space-scope ones, because the machinery was
//!   already built and an unchained operator log is the one an attacker can
//!   edit. That is a strengthening of the doc, not a departure from it: the
//!   load-bearing rule — *which* events may touch a space's stream — is
//!   unchanged, and a deployment that also wants JSON lines still needs a
//!   logger.
//!
//! # What is deliberately not here
//!
//! The reads over this crate's types live above it, and two of the three
//! now exist (7d, 2026-09-26): `ordo verify` walks both artifacts in one
//! command and reports them separately, and `GET /security/events` lets a
//! space owner read their own stream, gated in the space being read rather
//! than in the deployment's own. The dashboard's explorer/burst view
//! (`dashboard-plan.md` §3) is still unbuilt and consumes the second.
//!
//! Nothing of that is in *this* crate, which is the point of the sentence
//! that used to be here: the record and the windows onto it are separate,
//! and this crate is the record.

pub mod alert;
pub mod chain;
pub mod collapse;
pub mod correlation;
pub mod event;
pub mod recorder;
pub mod store;

#[cfg(test)]
mod tests;

pub use alert::{correlations_for, CounterSeries, DenyVolumeMonitor, DenyVolumeRule, Firing};
pub use chain::{
    decode_entry, encode_entry, hash_entry, verify_stream, verify_stream_from, ChainedEntry,
    DecodeError,
};
pub use collapse::{CollapseKey, Collapser, DEFAULT_CAP, DEFAULT_WINDOW_SECS};
pub use event::{
    sanitize_op, sanitize_space, EventKind, RecordedDecision, Scope, StreamEntry, Subject,
    SubjectKind, TokenFacts, DEPLOYMENT_KEY,
};
pub use recorder::{relationship, route, Observation, Recorder, Relationship, Stamp};
pub use store::{
    check_prune, checkpoint, checkpoint_at, prune_to_checkpoint, security_signer_from_env,
    verifiable_from, InMemorySecurityStore, Retention, SecurityStore, StreamCheckpoint,
    StreamError, DEPLOYMENT_RETENTION_DAYS_ENV, SECURITY_KEY_ENV, SECURITY_KEY_ID_ENV,
    SECURITY_PUBKEY_ENV,
};
