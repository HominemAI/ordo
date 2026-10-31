//! The seam between a function and the egress kernel.
//!
//! A function's `fetch` arrives here as an [`FnFetchRequest`]. The function
//! host has already checked the host against the function's `egress` list and
//! each secret name against its `secrets` list. What happens next (the deny
//! list, the secret's bound hosts, filling the value in, body caps, TLS)
//! belongs to the egress kernel behind [`FnFetch`].
//!
//! **Stand-in.** Part A of increment 1 builds the real kernel in
//! `crates/ordo-egress`. Until it merges, these types are defined here in the
//! same shape. The swap is this one file: replace its body with
//!
//! ```text
//! pub use ordo_egress::{FetchError, FnFetch, FnFetchRequest, FnFetchResponse, HeaderValue};
//! ```
//!
//! and add `ordo-egress` to this crate's `[dependencies]`. The host relies on
//! three things beyond the brief's shape: `FetchError::code()`, its `Display`,
//! and the default method [`FnFetch::scrub`].

use futures::future::BoxFuture;

/// One outbound request a function made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnFetchRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, HeaderValue)>,
    pub body: Vec<u8>,
}

/// A header value: plain text, or a secret the kernel fills in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderValue {
    Plain(String),
    /// `template` holds `{secret}` where the value goes, such as
    /// `Bearer {secret}`. The function names the secret; it never holds it.
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

/// A refused or failed outbound call. `code` is one of the plan's outcome
/// classes where it applies: `egress_refused`, `secret_missing`,
/// `secret_host_mismatch`; `fetch_failed` for a network failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchError {
    pub code: String,
    pub message: String,
}

impl FetchError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for FetchError {}

/// The egress kernel, as a function sees it. Held as `Arc<dyn FnFetch>`.
pub trait FnFetch: Send + Sync {
    /// Sends one request for `space`.
    fn fetch<'a>(
        &'a self,
        space: &'a str,
        req: FnFetchRequest,
    ) -> BoxFuture<'a, Result<FnFetchResponse, FetchError>>;

    /// Removes every live secret value of `space` from `text`. The function
    /// host runs each log line and error message through this before keeping
    /// it. Only the kernel holds the values, so only it can scrub them.
    fn scrub(&self, space: &str, text: &str) -> String {
        let _ = space;
        text.to_owned()
    }
}
