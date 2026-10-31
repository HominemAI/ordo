//! The blob door — a **second route**, and the argument for why it is not
//! an op.
//!
//! An op's contract is `dispatch(…, body: Bytes)`: fully buffered, with a
//! JSON envelope carrying named parameters. Bytes are not a parameter —
//! the compiler refuses `$f: blob` with class `blob_parameter` — and a
//! JSON envelope is not a place to put a hundred megabytes. So uploads and
//! downloads get their own two routes:
//!
//! ```text
//! PUT /blob/{entity}/{field}/{id}     the bytes in
//! GET /blob/{ref}                     the bytes out
//! ```
//!
//! # It is a second route, not a second door
//!
//! Every property the op door has, these have, and by *sharing the
//! mechanism* rather than by re-implementing it:
//!
//! - the same [`crate::spec::AccessChecker`], against the same bound space
//!   (`X-Ordo-Space`, exactly as an op call binds it);
//! - the same [`crate::error::ApiError`] inventory — **no new refusal
//!   class**, because the standing rule is that a new class is an oracle
//!   until argued, and everything these routes can refuse is something the
//!   door already has a word for: `bound_violation` for too large,
//!   `access_denied` for the door, `not_found` for a handle that is not
//!   there, and the same for a row the guard hides on a read;
//! - the same denial shape — a handle that does not exist and a handle in
//!   another space answer identically, for [`crate::router::door_denial`]'s
//!   reason.
//!
//! # The authorization ruling
//!
//! **Reading a blob is a read of its row; writing one is a write of its
//! row.** The access check is made against the derived std op for the
//! entity — `std.<entity>.get` for a `GET`, `std.<entity>.update` for a
//! `PUT` — so the blob door cannot grant what the op surface does not
//! already grant, and a deployment does not acquire a second, weaker
//! permission model by holding a file.
//!
//! That covers the *door*. The **row guard** is the platform's, and it is
//! [`BlobPort::authorize`]'s job: a guard that hides a row must hide its
//! file, or the file is a side channel around the guard. The trait says so
//! in its own doc rather than leaving it to a reviewer, because the
//! failure is silent — a blob door that skipped the guard would serve
//! every byte correctly, to the wrong person.
//!
//! # What is deliberately not here
//!
//! **Share links.** Settled in DL §2 (minting is an op, TTL bounded by
//! space policy, revocable, key in the URL fragment for `private`). They
//! wait on this serving path and on `private`, so building them first
//! would be building against two unbuilt things. The nearest existing
//! machinery is the capability grant's `expires` + `revoked_at`, which is
//! the right shape to reuse.
//!
//! **Presigned direct-to-bucket.** `blob private`'s, and F7's to open. A
//! presigned PUT into a space-sealed field's key would let a client put
//! *unsealed* bytes there, which is the one thing "uploads proxy through
//! the API" exists to prevent.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::Stream;

use crate::error::{ApiError, ErrorCode};

/// A stream of request or response bytes.
///
/// Declared here rather than borrowed from `ordo-blob` because this crate
/// depends on no sibling crate by design — the boundary types are the
/// contract and the platform fills them, which is exactly the arrangement
/// [`crate::spec::OpExecutor`] already has.
pub type BlobBody = Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>> + Send>>;

/// Which half of the door a request is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobVerb {
    Read,
    Write,
}

/// The op name `GET /blob/{ref}` is checked against at the door.
///
/// A **reserved pseudo-op**, and it has to be one: the entity is not in
/// the URL, so the door cannot name a per-entity op before the handle is
/// resolved — and resolving it is itself a read. Checking a per-entity op
/// here would turn the door into an oracle answering *"does this handle
/// exist, and in which entity?"* for anyone who can guess sixteen bytes.
///
/// `$blob` cannot collide with a real entity: the lexer cannot produce a
/// `$`-prefixed identifier, so no schema can declare one — the same
/// mechanism that reserves `$space`.
///
/// **It must be granted like any other op**, and the first version of this
/// was not: `deployment_engine` never listed it, so every `GET /blob/…`
/// answered `access_denied` for every principal and the row guard beneath
/// it was unreachable. Exported as a constant so the grant and the check
/// read the same name from the same place rather than agreeing by typing.
pub const BLOB_READ_OP: &str = "std.$blob.get";

/// The pseudo-entity [`BLOB_READ_OP`] is built from.
///
/// The router passes this to the gate and the gate derives the op name the
/// same way it does for a real entity, so there is one construction rather
/// than a literal at the check and another at the grant.
/// `the_read_pseudo_op_is_what_the_gate_derives` holds the two together.
pub const BLOB_ENTITY: &str = "$blob";

impl BlobVerb {
    /// The derived std op whose access decision governs.
    ///
    /// Named as a function rather than inlined at the two call sites so
    /// the ruling — "a blob is reached under its row's own permission" —
    /// has one place to be read and one place to change.
    pub fn std_op(self, entity: &str) -> String {
        match self {
            BlobVerb::Read => format!("std.{entity}.get"),
            BlobVerb::Write => format!("std.{entity}.update"),
        }
    }
}

/// What the door resolved before any byte moved.
#[derive(Debug, Clone)]
pub struct BlobRequest {
    pub space: String,
    pub principal: String,
    /// The agent presenting the token when it acts for `principal` under a
    /// delegation ([`crate::AccessDecision::Allow`]). An upload's command
    /// group names both; it is never a guard input.
    pub agent: Option<String>,
    pub verb: BlobVerb,
    /// Present on a write, which addresses a row and a column.
    pub entity: Option<String>,
    pub field: Option<String>,
    pub row_id: Option<String>,
    /// Present on a read, which addresses an object.
    pub handle_hex: Option<String>,
    /// The correlation every key release is scoped by.
    pub correlation: String,
}

/// What a read resolved to.
pub struct BlobRead {
    pub mime: String,
    /// The object's **plaintext** length, from the sealed `size` column.
    pub plain_len: u64,
    /// The bytes of the requested range.
    pub bytes: Vec<u8>,
    /// The range actually served, for `Content-Range`.
    pub from: u64,
    pub partial: bool,
}

/// A file's media type and size, without its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobMeta {
    pub mime: String,
    /// The object's plaintext length in bytes.
    pub size: u64,
}

/// The platform's half of the blob door.
///
/// # The obligation that is not visible in the signature
///
/// Both methods **must evaluate the row guard**. The access check the
/// router already made answers "may this principal call this op at all";
/// it does not answer "may this principal see this row". A blob door that
/// skipped the guard would serve every byte correctly to the wrong person,
/// and nothing about it would look wrong — so the obligation is stated
/// here, and `ordo-host`'s implementation names the query it runs.
#[async_trait::async_trait]
pub trait BlobPort: Send + Sync {
    /// Store an object and set the row's column, atomically with respect
    /// to each other: a `$blobref` row and a column that disagree is worse
    /// than either alone.
    ///
    /// `declared_len` is the plaintext length from `Content-Length`. The
    /// router has already refused it against the policy cap, so an
    /// oversize upload never reaches custody.
    async fn write(
        &self,
        req: &BlobRequest,
        declared_len: u64,
        mime: &str,
        body: BlobBody,
    ) -> Result<(), ApiError>;

    /// The media type and plaintext size of an object. **Runs the same row
    /// guard as [`BlobPort::read`]**, so whoever may not read the file may
    /// not learn its type or size either. Moves no bytes.
    async fn meta(&self, req: &BlobRequest) -> Result<BlobMeta, ApiError>;

    /// Read a plaintext range of a stored object.
    async fn read(
        &self,
        req: &BlobRequest,
        from: u64,
        len: Option<u64>,
    ) -> Result<BlobRead, ApiError>;
}

/// The bounds the door enforces before the platform is asked anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobLimits {
    /// The largest object any tier accepts. Mirrors
    /// `ordo_blob::BlobPolicy::max_object`; carried here because this
    /// crate cannot see that one, and checked here because the refusal
    /// must happen before the body is read rather than after.
    pub max_object: u64,
}

impl BlobLimits {
    pub const DEFAULT: BlobLimits = BlobLimits {
        max_object: 100 * 1024 * 1024,
    };
}

impl Default for BlobLimits {
    fn default() -> Self {
        BlobLimits::DEFAULT
    }
}

/// Everything the two routes need, alongside [`crate::router::ApiState`].
#[derive(Clone)]
pub struct BlobState {
    pub port: Arc<dyn BlobPort>,
    pub limits: BlobLimits,
}

/// `bound_violation`, which is the class the door already has for "outside
/// a declared bound". A `blob_too_large` would be a new class, and a new
/// class is an oracle until argued — this one needs no argument, because
/// "too large" is a bound.
pub fn too_large(limit: u64, declared: u64) -> ApiError {
    ApiError::new(
        ErrorCode::BoundViolation,
        format!("this upload is {declared} bytes; the object cap is {limit}"),
    )
}

/// A `Content-Length` that is absent, unparsable, or zero-width.
///
/// Refused rather than streamed-and-discovered, and the reason is not
/// convenience: the cap has to be enforced **before** the body arrives,
/// and a length discovered at the end is a cap enforced after 100 MB have
/// already been read. `missing_param` is the existing class for "the
/// request did not say something it had to say".
pub fn length_required() -> ApiError {
    ApiError::new(
        ErrorCode::MissingParam,
        "a blob upload declares its length: send Content-Length. The object cap is \
         enforced before the body is read, so a length discovered at the end would be \
         a cap enforced after the bytes arrived.",
    )
    .with_param("Content-Length")
}

/// What a route serving stored bytes is about to send.
///
/// The two shapes the door has: one stored file, whose type is the
/// uploader's claim, and an archive the door made itself, which is always a
/// save.
#[derive(Debug, Clone, Copy)]
pub enum Stored<'a> {
    /// One file, with the type recorded at upload. `shown` is the app's
    /// choice for this file; `None` takes [`DEFAULT_SHOWN`].
    File { mime: &'a str, shown: Option<Shown> },
    /// An archive the door built, saved under `filename`.
    Archive {
        content_type: &'static str,
        filename: &'a str,
    },
}

/// Whether a file renders in the browser or is saved.
///
/// The app developer's choice (owner ruling 2026-10-24): there are good
/// cases for either. Until an app can choose per field (filed for v2), every
/// file takes the default in [`DEFAULT_SHOWN`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// `inline`, under the file's own type and the sandbox policy.
    Inline,
    /// `attachment`, as `application/octet-stream`.
    Attachment,
}

/// The policy on every response that carries stored bytes, rendered or
/// saved: no script, no same origin, nothing loads, whatever the bytes are.
/// An HTML file an app chooses to render renders without script.
pub const STORED_BYTES_CSP: &str = "sandbox; default-src 'none'";

/// The one exception, a PDF shown inline. Chrome's PDF viewer shows a blank
/// page under `sandbox`, and with `default-src 'none'` alone it refuses the
/// viewer itself. So nothing loads but the viewer for this one document. A
/// PDF's own script runs inside the viewer, which gives it no page, no
/// fetch and no cookie on the door's origin.
pub const INLINE_PDF_CSP: &str = "default-src 'none'; object-src 'self'";

/// **The default, when the app has not chosen**: the types a browser shows
/// without running script are rendered; everything else, unknown types
/// included, is saved. A served type is the stored essence, except where
/// the table names one. An entry ending in `/` matches every subtype.
///
/// A stored type is an uploader's claim. Shown as claimed, a file uploaded
/// as `text/html` or `image/svg+xml` would run script on the door's origin,
/// which can read a fresh token from `/auth/refresh`. The stored type itself
/// is never changed; `GET /blob/{h}/meta` reports it.
pub const DEFAULT_SHOWN: &[(&str, Option<&str>)] = &[
    ("image/png", None),
    ("image/jpeg", None),
    ("image/gif", None),
    ("image/webp", None),
    ("image/avif", None),
    ("video/", None),
    ("audio/", None),
    ("application/pdf", None),
    ("text/plain", Some("text/plain; charset=utf-8")),
];

/// The essence of a media type: no parameters, lowercased.
fn essence_of(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// The [`DEFAULT_SHOWN`] entry for `essence`, if any.
fn default_entry(essence: &str) -> Option<&'static (&'static str, Option<&'static str>)> {
    DEFAULT_SHOWN
        .iter()
        .find(|(t, _)| match t.strip_suffix('/') {
            Some(top) => essence
                .strip_prefix(top)
                .and_then(|r| r.strip_prefix('/'))
                .is_some_and(|sub| !sub.is_empty() && !sub.contains('/')),
            None => essence == *t,
        })
}

/// How a stored file goes out: `(content type, disposition, policy)`.
pub fn served_as(mime: &str, shown: Option<Shown>) -> (String, &'static str, &'static str) {
    let essence = essence_of(mime);
    let entry = default_entry(&essence);
    let shown = shown.unwrap_or(if entry.is_some() {
        Shown::Inline
    } else {
        Shown::Attachment
    });
    match shown {
        Shown::Attachment => (
            "application/octet-stream".to_string(),
            "attachment",
            STORED_BYTES_CSP,
        ),
        Shown::Inline => {
            let served = entry
                .and_then(|(_, named)| named.map(str::to_string))
                .unwrap_or_else(|| {
                    if essence.contains('/') {
                        essence.clone()
                    } else {
                        "application/octet-stream".to_string()
                    }
                });
            let policy = if essence == "application/pdf" {
                INLINE_PDF_CSP
            } else {
                STORED_BYTES_CSP
            };
            (served, "inline", policy)
        }
    }
}

/// The one place the headers of stored bytes are set: the type, the
/// disposition, the policy and `nosniff`. Every route that sends stored
/// bytes builds its response through this
/// (`crates/ordo-host/tests/stored_bytes_scan.rs` fails a route that does
/// not).
pub fn stored_bytes(
    builder: axum::http::response::Builder,
    what: Stored<'_>,
) -> axum::http::response::Builder {
    let (content_type, disposition, policy) = match what {
        Stored::File { mime, shown } => {
            let (t, d, p) = served_as(mime, shown);
            (t, d.to_string(), p)
        }
        Stored::Archive {
            content_type,
            filename,
        } => (
            content_type.to_string(),
            format!("attachment; filename=\"{filename}\""),
            STORED_BYTES_CSP,
        ),
    };
    builder
        .header("content-type", content_type)
        .header("content-disposition", disposition)
        .header("content-security-policy", policy)
        // The type is a label. `nosniff` stops a browser from deciding the
        // bytes were HTML after all.
        .header("x-content-type-options", "nosniff")
}

/// Parse an HTTP `Range: bytes=from-[to]` header into plaintext
/// coordinates.
///
/// One range only. Multi-range responses are `multipart/byteranges`, a
/// second body format for a case nothing in the platform has asked for;
/// a request carrying more than one range is served its first, which is
/// what the RFC permits and what every client that sends one expects.
///
/// Returns `None` for an absent or unparsable header, which means "the
/// whole object" — an unparsable `Range` is explicitly a full response in
/// HTTP, not an error.
pub fn parse_range(header: Option<&str>) -> Option<(u64, Option<u64>)> {
    let raw = header?.trim();
    let spec = raw.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (from, to) = spec.split_once('-')?;
    // A suffix range (`bytes=-500`, the last 500 bytes) needs the object's
    // length to resolve and the door does not have it yet. Answered as a
    // full read rather than guessed at.
    if from.is_empty() {
        return None;
    }
    let from: u64 = from.parse().ok()?;
    let len = if to.is_empty() {
        None
    } else {
        let to: u64 = to.parse().ok()?;
        // HTTP ranges are INCLUSIVE at both ends; the pipeline's are
        // half-open. Converting in one place is the whole reason this is a
        // function.
        if to < from {
            return None;
        }
        Some(to - from + 1)
    };
    Some((from, len))
}

#[cfg(test)]
mod tests {
    use super::{served_as, Shown, INLINE_PDF_CSP, STORED_BYTES_CSP};

    /// An app that chooses to render HTML gets it rendered without script;
    /// one that chooses to save a PNG gets it saved.
    #[test]
    fn an_apps_choice_replaces_the_default_and_keeps_the_sandbox() {
        assert_eq!(
            served_as("text/html", Some(Shown::Inline)),
            ("text/html".to_string(), "inline", STORED_BYTES_CSP)
        );
        assert_eq!(
            served_as("image/png", Some(Shown::Attachment)),
            (
                "application/octet-stream".to_string(),
                "attachment",
                STORED_BYTES_CSP
            )
        );
        assert_eq!(
            served_as("application/pdf", None),
            ("application/pdf".to_string(), "inline", INLINE_PDF_CSP)
        );
        assert_eq!(
            served_as("application/pdf", Some(Shown::Attachment)).2,
            STORED_BYTES_CSP
        );
    }

    use super::*;

    /// The name the door checks and the name a deployment grants are the
    /// same string, derived once. They were not, once: the grant did not
    /// exist at all, and every `GET /blob/…` was `access_denied` for every
    /// principal with the row guard beneath it unreachable.
    #[test]
    fn the_read_pseudo_op_is_what_the_gate_derives() {
        assert_eq!(BlobVerb::Read.std_op(BLOB_ENTITY), BLOB_READ_OP);
        // And it cannot collide with a real entity: no schema can declare
        // a `$`-prefixed name, the same mechanism that reserves `$space`.
        assert!(BLOB_ENTITY.starts_with('$'));
    }

    #[test]
    fn the_std_op_names_are_the_rows_own() {
        assert_eq!(BlobVerb::Read.std_op("documents"), "std.documents.get");
        assert_eq!(BlobVerb::Write.std_op("documents"), "std.documents.update");
    }

    /// HTTP ranges are inclusive; the pipeline's are half-open. The
    /// conversion is off by one in exactly one direction, so it is tested
    /// rather than reviewed.
    #[test]
    fn a_range_header_converts_to_half_open_coordinates() {
        assert_eq!(parse_range(Some("bytes=0-499")), Some((0, Some(500))));
        assert_eq!(parse_range(Some("bytes=500-")), Some((500, None)));
        assert_eq!(parse_range(Some("bytes=0-0")), Some((0, Some(1))));
        assert_eq!(parse_range(Some(" bytes=10-19 ")), Some((10, Some(10))));
        // First of several, which is what the RFC permits.
        assert_eq!(parse_range(Some("bytes=0-9,20-29")), Some((0, Some(10))));
    }

    /// An unparsable or unsupported `Range` is a **full** response, which
    /// is what HTTP says — not an error, and not a guess.
    #[test]
    fn an_unusable_range_reads_the_whole_object() {
        for header in [
            None,
            Some("bytes=-500"), // suffix: needs a length the door lacks
            Some("items=0-1"),
            Some("bytes=9-1"),
            Some("bytes=abc-def"),
            Some("nonsense"),
        ] {
            assert_eq!(parse_range(header), None, "{header:?}");
        }
    }

    #[test]
    fn the_refusals_reuse_existing_classes() {
        assert_eq!(too_large(10, 20).code, ErrorCode::BoundViolation);
        assert_eq!(length_required().code, ErrorCode::MissingParam);
    }
}
