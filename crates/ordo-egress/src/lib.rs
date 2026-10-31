//! The egress kernel: one outbound HTTP client, six pins, no way round them.
//!
//! `security-logging-design.md` §7b and `implementation.md` (Outbound egress)
//! are `[pinned]` and say the same six things in the same words. This crate
//! is those six things expressed as **types**, and the discipline it follows
//! is the one `ordo-identity`'s `UserPresence` follows: a rule enforced by a
//! check somebody has to remember is a rule that survives until the second
//! caller; a rule enforced by a constructor is a rule.
//!
//! # Why a crate and not a module
//!
//! Two arguments, and the second is the load-bearing one.
//!
//! *One client, one policy, one place to audit* — `implementation.md`'s own
//! phrasing. Attestation is the first consumer; feed delivery, notification
//! sinks and import bridges are named as future ones. A module inside
//! `ordo-exec` would be reachable only from `ordo-exec`, and the second
//! consumer would grow its own.
//!
//! *The fifth pin is a dependency edge or it is nothing.* "Egress runs under
//! an identity that holds nothing — no instance-credential access and no
//! `KeyProvider` reachability" is a **process** property, and this platform
//! ships as one process. What can be enforced statically is narrower and
//! still worth having: the code that opens a socket to a customer-named host
//! **cannot name** `ordo_crypto::KeyProvider`, cannot name `ordo_exec::Pool`,
//! and therefore cannot reach the database credentials or the key material,
//! because it does not depend on the crates that define them. `Cargo.toml`
//! carries the argument and `tests/holds_nothing.rs` asserts it by parsing
//! the manifest — so the pin is repealed by a diff a reviewer sees, not by an
//! import nobody notices.
//!
//! The honest remainder, stated rather than discovered: a separate **pod**
//! with a credential-less service account is a deployment obligation this
//! crate cannot discharge. `plan-runtime.md` R3's worker slot naming is what
//! makes that schedulable — `attest.deliver` is a named slot and
//! `ORDO_WORKERS` can be set to run it, and only it, in a pod of its own.
//! `docs/deploying.md` carries the obligation.
//!
//! # The six pins, and where each one lives
//!
//! | § | Pin | Enforced by |
//! |---|---|---|
//! | 1 | `https` only, certs validated, TLS 1.2 floor | [`HttpsUrl`] has one constructor and it refuses every other scheme; `rustls` has no code for TLS below 1.2; there is no `danger_accept_invalid_certs` in this crate or in its dependency's default feature set |
//! | 2 | Resolve-then-pin, connect to the resolved address | [`PinnedDestination`] is minted only by [`resolve_and_pin`] and **consumed by value** by [`deliver`]; it is not `Clone`, has no public constructor and no serialisation, so it cannot be stored, and a second delivery needs a second resolution |
//! | 3 | No redirect following, ever | the response reader has no loop and no `Location` handling — a 3xx becomes [`Refusal::RedirectAnswered`] |
//! | 4 | Signed deliveries | [`deliver`] takes a [`SinkSecret`] by value; there is no overload without one |
//! | 5 | An egress identity that holds nothing | the dependency graph — see above |
//! | 6 | Per-space consented allowlist | [`resolve_and_pin`] takes an [`Allowlist`]; a `PinnedDestination` for a host outside it cannot be constructed |
//!
//! Two of those are worth reading twice.
//!
//! **Pin 2 is a move, not a check.** The usual implementation resolves,
//! checks, and then hands a *URL* to a client that resolves again — the
//! rebind window §7b names. Here the only thing [`deliver`] accepts is a
//! `PinnedDestination`, whose only field the dialler reads is a
//! [`std::net::SocketAddr`], and which the call consumes. Re-checking "at
//! connect time" is not a discipline the worker has to observe; it is the
//! only way to get a second delivery, because the first one ate the value.
//!
//! **Pin 6 is checked before resolution, not after.** An allowlist consulted
//! after a DNS lookup has already performed the lookup, and a DNS lookup is
//! itself an outbound request to an attacker-chosen name — the leak §7b's
//! allowlist exists to stop happens before the check would run.
//!
//! # What does not come back
//!
//! §7b: *"sinks deliver payloads and log deliveries, so the response or its
//! metadata comes back to the tenant"*. That is the exfiltration half of
//! SSRF, and it is closed here by the return type rather than by a rule about
//! logging: [`Accepted`] carries a status code and nothing else. Response
//! headers are parsed only far enough to find the end of them, and the body
//! is read to a bounded cap and dropped without being decoded. There is no
//! path in this crate by which a byte the destination sent reaches a caller.
//! A receiver that wants to say something says it with a status code.
//!
//! # The one exception: `fetch`, for a verifier
//!
//! Sign-in through an outside provider needs the provider's answer: a token
//! response, a userinfo document (`plan-auth-federated.md` §3.4). [`fetch`]
//! is that second, narrow door. It shares the dialler and every pin above
//! except the fourth (an identity provider holds no sink secret; the request
//! is authenticated by TLS and, at a token endpoint, by the client secret the
//! caller passes in per call). What it returns is [`Fetched`]: no `Debug`, no
//! `Display`, no serialiser, readable once through
//! [`Fetched::into_verifier_input`]. Its consumers are parsers whose outputs
//! are claims, and the caller of a sign-in route is answered a token or the
//! one refusal, never a byte of the body. A body over its per-purpose cap is
//! refused before it is held.
//!
//! # Mail: `post` and `smtp`
//!
//! Sign-in mail (`plan-auth-federated.md` §7.6) leaves through two more
//! doors on the same dialler. [`post`] sends one request with a key the
//! caller passes in a header and answers the status, as [`deliver`] does: a
//! mail API's whole answer is "accepted" or not. [`smtp`] submits one message
//! to a relay over implicit TLS; it is pinned by [`resolve_and_pin_host`],
//! which applies the allowlist and the deny list to a host and port exactly
//! as [`resolve_and_pin`] applies them to a URL. Neither returns a byte the
//! far end chose beyond a status or a reply code.
//!
//! The request side ([`FetchRequest`]) is general on purpose: a backend
//! function's `fetch` is the same dialler and reader with a different front
//! door.
//!
//! # Functions: [`function`]
//!
//! A backend function's outbound call (`plan-backend-functions.md` §6) is
//! [`FnEgress`], behind the [`FnFetch`] trait. It returns the whole answer,
//! bounded, and fills in secret handles only for the hosts each secret is
//! bound to. The value arrives per call from the host's [`SecretSource`];
//! this crate still names no key provider.

pub mod client;
pub mod deny;
pub mod dest;
pub mod function;
pub mod refusal;
pub mod sign;
pub mod smtp;

pub use client::{
    deliver, fetch, post, Accepted, Delivery, FetchMethod, FetchRequest, Fetched, TlsRoots,
    DEFAULT_BODY_CAP, DEFAULT_TIMEOUT,
};
pub use deny::{Cidr, DeniedCategory, DenyList, PrivateHost, DENY_CIDRS_ENV, PRIVATE_HOSTS_ENV};
pub use dest::{resolve_and_pin, resolve_and_pin_host, Allowlist, HttpsUrl, PinnedDestination};
pub use function::{
    BodySide, FetchAction, FetchError, FnEgress, FnFetch, FnFetchRequest, FnFetchResponse,
    FnLimits, HeaderValue, SecretBytes, SecretSource,
};
pub use refusal::Refusal;
pub use sign::{SinkSecret, SIGNATURE_HEADER, SIGNATURE_VERSION, TIMESTAMP_HEADER};
