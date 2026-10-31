//! The seam between a function and the egress kernel.
//!
//! A function's `fetch` arrives here as an [`FnFetchRequest`]. The function
//! host checks the host against the function's `egress` list and each secret
//! name against its `secrets` list first, and passes both lists on, so the
//! kernel checks them again. What happens next (the deny list, the secret's
//! bound hosts, filling the value in, body caps, TLS, replacing an echoed
//! value with `[secret]`) belongs to the egress kernel behind [`FnFetch`].
//!
//! **Stand-in.** Part A of increment 1 builds the real kernel in
//! `crates/ordo-egress` (`ordo_host::secrets::fn_fetch` makes one). Until it
//! merges, these types are defined here in its shape. The swap is this one
//! file: replace its body with
//!
//! ```text
//! pub use ordo_egress::{FetchError, FnFetch, FnFetchRequest, FnFetchResponse, HeaderValue};
//! ```
//!
//! and add `ordo-egress` to this crate's `[dependencies]`. The host uses only
//! `FetchError::kind()` and its `Display`, so the stand-in's variants are a
//! subset with the same `kind()` words.

/// One outbound request a function made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnFetchRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, HeaderValue)>,
    pub body: Vec<u8>,
    /// The function's declared egress hosts. Empty refuses every host.
    pub egress: Vec<String>,
    /// The function's declared secrets.
    pub secrets: Vec<String>,
}

/// A header value: plain text, or a secret the kernel fills in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderValue {
    Plain(String),
    /// `template` holds `{}` exactly once, where the value goes, such as
    /// `Bearer {}`. The function names the secret; it never holds it.
    Secret {
        name: String,
        template: String,
    },
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnFetchResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A refused or failed outbound call: a subset of `ordo_egress::FetchError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    HostNotAllowed { host: String },
    SecretNotListed { name: String },
    SecretNotSet { name: String },
    SecretNotBound { secret: String, host: String },
    Timeout { after_ms: u64 },
    Upstream(String),
}

impl FetchError {
    /// The outcome class (`plan-backend-functions.md` §10).
    pub fn kind(&self) -> &'static str {
        match self {
            FetchError::HostNotAllowed { .. } | FetchError::SecretNotListed { .. } => {
                "egress_refused"
            }
            FetchError::SecretNotSet { .. } => "secret_missing",
            FetchError::SecretNotBound { .. } => "secret_host_mismatch",
            FetchError::Timeout { .. } | FetchError::Upstream(_) => "fetch_failed",
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::HostNotAllowed { host } => write!(f, "{host} is not an egress host"),
            FetchError::SecretNotListed { name } => write!(f, "{name} is not a listed secret"),
            FetchError::SecretNotSet { name } => write!(f, "secret {name} has no value here"),
            FetchError::SecretNotBound { secret, host } => {
                write!(f, "secret {secret} is not bound to {host}")
            }
            FetchError::Timeout { after_ms } => write!(f, "no answer after {after_ms} ms"),
            FetchError::Upstream(d) => write!(f, "{d}"),
        }
    }
}

impl std::error::Error for FetchError {}

/// The egress kernel, as a function sees it. Held as `Arc<dyn FnFetch>`.
#[async_trait::async_trait]
pub trait FnFetch: Send + Sync {
    /// Sends one request for `space`.
    async fn fetch(&self, space: &str, req: FnFetchRequest) -> Result<FnFetchResponse, FetchError>;
}
