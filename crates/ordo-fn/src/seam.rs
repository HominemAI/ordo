//! The seam between a function and the egress kernel: `ordo-egress`'s.
//!
//! A function's `fetch` arrives at [`FnFetch`] as an [`FnFetchRequest`]. The
//! function host first checks the host against the function's `egress` list
//! and each secret name against its `secrets` list, then passes both lists
//! on, so the kernel checks them again. Everything after that (the deny
//! list, a secret's bound hosts, filling the value in, body caps, TLS,
//! replacing a value the far end echoes back) is the kernel's.

pub use ordo_egress::{FetchError, FnFetch, FnFetchRequest, FnFetchResponse, HeaderValue};
