//! The correlation id — the one key that joins the observability artifacts.
//!
//! # What it is
//!
//! `implementation.md` (Observability) pins it: `ordo.correlation` is **the
//! command-group correlation id**, so a trace, a log line and a chain group
//! join on one value. This module mints it and gives it a type, and it uses
//! `ordo_commandlog::CorrelationId` rather than a new type so that the value
//! stamped on a security event and the value hashed into a command group are
//! the *same* value in the *same* type — not two strings a convention says
//! should match.
//!
//! # What it joins today
//!
//! Two of the three artifacts, plus custody:
//!
//! - **The security-event stream.** Every [`crate::event::StreamEntry`]
//!   carries a bounded sample of the correlations that produced it, so an
//!   alert that fires can be traced from a counter to the entries to the
//!   individual requests.
//! - **The command chain.** `CommandGroup::correlation` is this type. A
//!   request that is denied leaves a security entry and no group; a request
//!   that succeeds and writes leaves a group under the same id, so "what did
//!   this principal actually do in the window they were being denied in" is
//!   one lookup per space.
//! - **Key custody.** `KeyProvider::release(space, purpose, correlation)`
//!   already takes it, and `MeteredKeys` counts releases; the release event
//!   this crate records carries the lease's own correlation, so a
//!   key-release breadth question and a denial-volume question are asked
//!   with the same key.
//!
//! # What it does not join yet, stated plainly
//!
//! - **Traces.** There is no OpenTelemetry emitter anywhere in the
//!   workspace, so `ordo.correlation` has no span to be an attribute of.
//!   When one exists, the root span takes this value and the join is free.
//! - **Structured request logs.** The deployment's JSON-per-request log does
//!   not exist either. That is the artifact that would resolve a correlation
//!   back to *a principal* for key releases — which is precisely the gap
//!   `ordo-host`'s `Metrics::key_release` documents, and it is the same gap.
//! - **Cross-process propagation.** Nothing accepts an inbound correlation
//!   header, so a correlation is minted per process at the door and does not
//!   yet span a call that fans out to a worker.
//!
//! Until the door can hand the id it minted to the executor — which needs a
//! field on the access decision that does not exist today — a *denied*
//! request's correlation is recorded in the stream and joins nothing else,
//! because a denied request has nothing else. That is the honest state: the
//! id is minted, typed, and carried everywhere this crate reaches.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ordo_commandlog::CorrelationId;
use sha2::{Digest, Sha256};

/// Prefix for a minted correlation. Typed ids name their species on sight,
/// like every other id in the platform.
pub const PREFIX: &str = "cor";

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Mint a correlation id for one unit of work.
///
/// Process nonce ‖ monotone sequence, folded through SHA-256 into 16 hex
/// characters. The nonce is the process start instant, so two replicas that
/// both start their counters at zero do not mint the same ids; the sequence
/// is monotone within a process, so two requests in flight cannot collide.
/// It is not a UUID and does not claim to be one — it needs to be unique in
/// practice and cheap, and it deliberately encodes nothing about the caller.
pub fn mint() -> CorrelationId {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nonce = *process_nonce();
    let mut h = Sha256::new();
    h.update(b"ordo.correlation.v1");
    h.update(nonce.to_be_bytes());
    h.update(n.to_be_bytes());
    let digest = h.finalize();
    let mut hex = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    CorrelationId(format!("{PREFIX}_{hex}"))
}

fn process_nonce() -> &'static u64 {
    static NONCE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    NONCE.get_or_init(|| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        nanos ^ (std::process::id() as u64).rotate_left(32)
    })
}

/// A correlation supplied from outside — a job's, a lease's, a caller's
/// already-minted one. Bounded and sanitized for the same reason an op name
/// is: it ends up inside a hashed record other people read.
///
/// The rule is the same all-or-nothing one: an id made of
/// `[A-Za-z0-9._-]` and no longer than 64 bytes is taken as given, anything
/// else is replaced by a freshly minted id rather than partially scrubbed.
pub fn adopt(raw: &str) -> CorrelationId {
    let ok = !raw.is_empty()
        && raw.len() <= 64
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        CorrelationId(raw.to_string())
    } else {
        mint()
    }
}
