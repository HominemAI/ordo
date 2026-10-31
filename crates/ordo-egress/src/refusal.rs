//! Why a delivery did not happen.
//!
//! One enum, and it is deliberately **coarser at the wire than it is here**.
//! `api-engine-design.md`'s inventory gets exactly one new class,
//! `egress_refused`, and every variant below folds into it. The reason is the
//! reason `platform_ops.rs` gives for minting zero classes: a class per pin
//! is an oracle. `egress_denied_private` and `egress_denied_pod_cidr` as
//! distinct wire words would let anyone who can name a hostname map the
//! deployment's internal address plan one grant at a time, which is a
//! reconnaissance primitive handed out by the error channel.
//!
//! What *is* disclosed is the **category**, because §7b requires the deny
//! list to be on the consent page — "this sink may not point inside the
//! deployment" is a promise a customer is owed, and a refusal that will not
//! say which promise it kept is a refusal nobody can act on. So the message
//! says "loopback" or "private"; it never says which address, and it never
//! says whether the range was a structural category or this deployment's own
//! pod CIDR. [`Refusal::disclosable`] is the rendering that goes to a
//! customer, and [`Refusal::operational`] is the one that goes to the
//! deployment's own log.

use std::fmt;
use std::net::IpAddr;

use crate::deny::DeniedCategory;

/// Every way an egress attempt ends other than "the destination answered 2xx".
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// Pin 1. The URL was not `https`, or was not a URL.
    NotHttps { detail: String },
    /// Pin 1. The URL parsed as `https` but is otherwise unusable — no host,
    /// a userinfo section (`https://user@host/`, the oldest phishing shape in
    /// the book and a way to smuggle a second host past a naive check), a
    /// port of zero, a fragment.
    MalformedUrl { detail: String },
    /// Pin 6. The host is not on the space's consented allowlist.
    NotAllowlisted { host: String },
    /// Pin 2. The hostname did not resolve at all.
    Unresolvable { host: String },
    /// Pin 2. A resolved address is inside the perimeter. `category` is
    /// disclosable; `address` is not, and lives here only so the deployment's
    /// own operational log can carry it.
    DeniedAddress {
        host: String,
        address: IpAddr,
        category: DeniedCategory,
    },
    /// Pin 3. The destination answered a 3xx. Not a hop, and not a retryable
    /// failure either: a receiver that redirects is misconfigured, and
    /// following it is exactly the move that makes the deny list irrelevant.
    RedirectAnswered { status: u16 },
    /// The TLS handshake failed — an untrusted chain, a name mismatch, a
    /// version floor. Not separable into those, on purpose: distinguishing
    /// "your certificate is for the wrong name" from "your certificate is not
    /// trusted" tells a prober which of the two they achieved.
    TlsFailed { detail: String },
    /// The socket failed: refused, reset, unreachable.
    ConnectFailed { detail: String },
    /// The whole attempt exceeded its bound.
    TimedOut { after_ms: u64 },
    /// The destination answered, and it was not 2xx and not 3xx.
    /// `retry_after_secs` is the answer's `Retry-After`, when it gave one in
    /// seconds (an HTTP-date is not read, and the caller's own backoff
    /// applies). Only a number: no other header and no body byte is kept.
    Rejected {
        status: u16,
        retry_after_secs: Option<u64>,
    },
    /// The destination answered something that is not HTTP/1.x, or headers
    /// exceeding the cap. Bounded parsing refusing rather than allocating.
    MalformedResponse { detail: String },
    /// A mail relay answered a reply code other than the one the step
    /// expects (`smtp`). A 4xx is the relay asking for a later attempt; a
    /// 5xx is a refusal. The relay's text is never kept: it can echo the
    /// recipient's address.
    RelayRefused { code: u16 },
    /// What was to be sent cannot be written without changing its meaning:
    /// an address with a line break, a subject with a control character. Refused before
    /// a socket opens.
    NotRepresentable { detail: String },
}

impl Refusal {
    /// The stable word for this refusal — a metric label and a dead-letter
    /// reason prefix. Not a wire class: the wire class is `egress_refused`
    /// for every one of these.
    pub fn kind(&self) -> &'static str {
        match self {
            Refusal::NotHttps { .. } => "not_https",
            Refusal::MalformedUrl { .. } => "malformed_url",
            Refusal::NotAllowlisted { .. } => "not_allowlisted",
            Refusal::Unresolvable { .. } => "unresolvable",
            Refusal::DeniedAddress { .. } => "denied_address",
            Refusal::RedirectAnswered { .. } => "redirect_answered",
            Refusal::TlsFailed { .. } => "tls_failed",
            Refusal::ConnectFailed { .. } => "connect_failed",
            Refusal::TimedOut { .. } => "timed_out",
            Refusal::Rejected { .. } => "rejected",
            Refusal::MalformedResponse { .. } => "malformed_response",
            Refusal::RelayRefused { .. } => "relay_refused",
            Refusal::NotRepresentable { .. } => "not_representable",
        }
    }

    /// Is another attempt worth making?
    ///
    /// The three pins that describe the *destination* are not retryable —
    /// re-resolving a name that answers `127.0.0.1` will answer `127.0.0.1`
    /// again, and retrying it five times is five more DNS queries to a name
    /// an attacker chose. A transport failure is retryable, because that is
    /// what a network is. A 3xx is **not**: it is a configuration fact.
    pub fn retryable(&self) -> bool {
        match self {
            Refusal::NotHttps { .. }
            | Refusal::MalformedUrl { .. }
            | Refusal::NotAllowlisted { .. }
            | Refusal::DeniedAddress { .. }
            | Refusal::RedirectAnswered { .. } => false,
            // A name that does not resolve today may resolve tomorrow; this
            // is the one destination-shaped refusal that is a transient.
            Refusal::Unresolvable { .. } => true,
            Refusal::TlsFailed { .. }
            | Refusal::ConnectFailed { .. }
            | Refusal::TimedOut { .. }
            | Refusal::MalformedResponse { .. } => true,
            // 5xx is the receiver having a bad day; 4xx is the receiver
            // saying no. Retrying a 4xx is how a webhook becomes a denial of
            // service against the customer who configured it, except the
            // three 4xx that ask for a later attempt: 408 (the request took
            // too long), 425 (too early) and 429 (too many requests).
            Refusal::Rejected { status, .. } => retryable_status(*status),
            // SMTP is the other way round from HTTP: 4xx is "try again
            // later" (RFC 5321 §4.2.1) and 5xx is "no".
            Refusal::RelayRefused { code } => (400..500).contains(code),
            Refusal::NotRepresentable { .. } => false,
        }
    }

    /// How long the destination asked to be left alone before the next
    /// attempt (`Retry-After`, in seconds), when this refusal is retryable
    /// and it said. A caller waits at least this long, and its own backoff
    /// when that is longer.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            Refusal::Rejected {
                retry_after_secs: Some(s),
                ..
            } if self.retryable() => Some(std::time::Duration::from_secs(*s)),
            _ => None,
        }
    }

    /// What a customer may be told. Never an address, never a port, never a
    /// response byte.
    pub fn disclosable(&self) -> String {
        match self {
            Refusal::NotHttps { detail } => {
                format!("destination must be https (got {detail})")
            }
            Refusal::MalformedUrl { detail } => {
                format!("destination is not a usable URL: {detail}")
            }
            Refusal::NotAllowlisted { host } => format!(
                "`{host}` is not on this space's egress allowlist; adding a host is a consent, \
                 not a config edit"
            ),
            Refusal::Unresolvable { host } => format!("`{host}` does not resolve"),
            Refusal::DeniedAddress { host, category, .. } => format!(
                "`{host}` resolves to a {} address; a sink may not point inside the deployment",
                category.as_str()
            ),
            Refusal::RedirectAnswered { status } => format!(
                "destination answered {status}; redirects are never followed, so a 3xx is a \
                 delivery failure"
            ),
            Refusal::TlsFailed { .. } => "TLS handshake failed".to_string(),
            Refusal::ConnectFailed { .. } => "could not connect".to_string(),
            Refusal::TimedOut { after_ms } => format!("timed out after {after_ms}ms"),
            Refusal::Rejected { status, .. } => format!("destination answered {status}"),
            Refusal::MalformedResponse { .. } => "destination did not answer HTTP/1.x".to_string(),
            Refusal::RelayRefused { code } => format!("the mail relay answered {code}"),
            Refusal::NotRepresentable { detail } => format!("not representable: {detail}"),
        }
    }

    /// What the deployment's own operators may be told: everything, including
    /// the resolved address the customer is not shown.
    pub fn operational(&self) -> String {
        match self {
            Refusal::DeniedAddress {
                host,
                address,
                category,
            } => format!(
                "denied_address: `{host}` -> {address} ({})",
                category.as_str()
            ),
            Refusal::TlsFailed { detail } => format!("tls_failed: {detail}"),
            Refusal::ConnectFailed { detail } => format!("connect_failed: {detail}"),
            Refusal::MalformedResponse { detail } => format!("malformed_response: {detail}"),
            other => format!("{}: {}", other.kind(), other.disclosable()),
        }
    }
}

/// Is an HTTP status one that asks for a later attempt? Every 5xx, and the
/// three 4xx that mean "not now": 408, 425 and 429. The one rule for every
/// HTTP caller of this crate (webhooks, mail APIs, sign-in providers).
pub fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500..=599)
}

/// The longest `Retry-After` read from an answer: one day. A larger value is
/// clamped to it, so a destination cannot park a delivery for a year.
pub const RETRY_AFTER_CAP_SECS: u64 = 86_400;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.disclosable())
    }
}

impl std::error::Error for Refusal {}
