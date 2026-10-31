//! Opaque, stateless, **authenticated** page tokens:
//! `base64url(JSON{v,k}) . base64url(HMAC)` where `v` is the op's version
//! hash and `k` the keyset values (last row's sort keys + id tiebreak — the
//! executor owns their meaning; here they are freight).
//!
//! # Why the token is MAC'd and principal-bound (SH-6)
//!
//! The payload used to be base64 and nothing else. Two consequences: a caller
//! could **forge** a cursor and seek arbitrarily in sort space, turning any
//! paged op into a binary-search oracle over the sort key; and a token that
//! leaked (query strings land in proxy logs, browser history, and `Referer`
//! headers) was usable by whoever found it.
//!
//! So every token carries `HMAC(page-key, "ordo.page.v1" ‖ space ‖ op ‖
//! principal ‖ payload)`, length-prefixed. Four properties fall out, and each
//! is checked before the payload is even parsed:
//!
//! - **Unforgeable.** A cursor the platform did not mint is `invalid_cursor`.
//! - **Principal-bound.** Another principal presenting this token is
//!   `invalid_cursor` — the MAC covers who it was minted for, without the
//!   principal id riding in the payload where it would leak into logs.
//! - **Space-bound.** A principal who is a member of two spaces cannot carry a
//!   cursor from one into the other. Without this the MAC still verifies — same
//!   deployment key, same op, same principal — and the executor seeks to the
//!   *other* space's keyset boundary, skipping or reshaping a result set no
//!   cursor issued in that space ever authorized. The space is already the
//!   isolation context for access and replay; the cursor now uses the same one.
//! - **Per-op.** The op name is in the MAC, so a token minted for op A is not
//!   accepted by op B even when their version hashes coincide. The version
//!   check stays a *separate*, explicit comparison against
//!   `op.version_hash`, so an honestly stale token still answers
//!   `version_skew` (restart from page one) rather than being confused with a
//!   forgery.
//!
//! Statelessness is untouched: any node with the page key resumes any token,
//! and no server-side cursor state appears.
//!
//! # What the MAC does NOT cover, and why that is safe *today*
//!
//! The MAC binds space, op name, principal, and the op's version hash — not
//! the request's **parameters** or its **refinement**. A cursor minted for
//! `cheapActive($cap: 1000)` therefore verifies when replayed against
//! `cheapActive($cap: 999999)`.
//!
//! That is harmless while three things hold, and it stops being harmless the
//! moment any of them changes:
//!
//! 1. The seek predicate is applied strictly OUTSIDE the caller's guarded
//!    row set, so no cursor can widen what a principal may see — the worst a
//!    swapped parameter does is start mid-way through a set the caller was
//!    already entitled to.
//! 2. Refinement is narrowing-only and cannot influence sort order, so it
//!    cannot make a cursor address a different row.
//! 3. The version hash covers the op's sort clause, so a redeployed order
//!    answers `version_skew` rather than seeking with stale keys.
//!
//! A refinement reaches the paging planner since 2026-10-05: its conditions
//! compile into the plan (`refinement-default` ask 1). Point 2 still holds,
//! because the envelope has no sort key and the call keeps the op's order,
//! so a cursor reused with a changed refinement starts the new filter after
//! the same row (`ordo-host/tests/refine_paging.rs`). If a refinement ever
//! gains a sort key, or a parameter is ever allowed to affect ordering, the
//! MAC must grow to cover the bound parameters. Recorded here rather than in a ledger because this is the
//! file that would have to change.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ApiError, ErrorCode};
use crate::mac::{ct_eq, hmac_sha256};
use crate::spec::OpSpec;

/// Domain tag: this key authenticates page tokens and nothing else.
const PAGE_DOMAIN: &[u8] = b"ordo.page.v1";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageCursor {
    pub keys: Vec<Value>,
}

/// The deployment's page-token key. Deliberately not `Debug`, not `Copy`, and
/// with no accessor: it exists to be handed to [`encode`] / [`decode`].
///
/// One key per deployment is enough — the token binds space, op and principal
/// itself. Rotating it invalidates outstanding cursors, which costs a client
/// one restart from page one.
#[derive(Clone)]
pub struct PageKey([u8; 32]);

impl PageKey {
    /// Derive a key from deployment secret material (a `KeyProvider` release
    /// in production, a fixed string in tests). Hashed rather than truncated
    /// so a short secret still fills the key.
    pub fn from_secret(secret: &[u8]) -> PageKey {
        let mut key = [0u8; 32];
        key.copy_from_slice(&hmac_sha256(b"ordo.page.key.v1", &[secret]));
        PageKey(key)
    }

    /// Read the deployment's page key from `ORDO_PAGE_KEY` — 64 hex
    /// characters.
    ///
    /// **Unset is a refusal, never a default.** The cursor MAC binds a page
    /// to its space, its op and its principal, and every one of those
    /// bindings is worth exactly as much as the key being this
    /// deployment's: a shared key makes a cursor minted in one deployment
    /// valid in another, which turns three careful bindings into
    /// decoration.
    ///
    /// The test harness's `PageKey::from_secret(b"ordo-deployment-test")`
    /// is what a binary with no loader copies, which is the whole reason
    /// this exists.
    pub fn from_env() -> Result<PageKey, PageKeyError> {
        // `ORDO_PAGE_KEY_FILE` names a file holding the same 64 hex
        // characters, the form the console's supervisor passes.
        let hex = ordo_env::secret("ORDO_PAGE_KEY")
            .map_err(|e| PageKeyError::File(e.to_string()))?
            .ok_or(PageKeyError::Missing)?;
        let hex = hex.trim();
        if hex.len() != 64 {
            return Err(PageKeyError::Malformed);
        }
        let mut raw = [0u8; 32];
        for (i, b) in raw.iter_mut().enumerate() {
            *b = u8::from_str_radix(
                hex.get(i * 2..i * 2 + 2).ok_or(PageKeyError::Malformed)?,
                16,
            )
            .map_err(|_| PageKeyError::Malformed)?;
        }
        Ok(PageKey::from_secret(&raw))
    }
}

/// Why `ORDO_PAGE_KEY` did not produce a key.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PageKeyError {
    Missing,
    Malformed,
    /// `ORDO_PAGE_KEY_FILE` could not be read, or both forms are set.
    File(String),
}

impl std::fmt::Display for PageKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PageKeyError::Missing => f.write_str(
                "ORDO_PAGE_KEY is not set (nor ORDO_PAGE_KEY_FILE) — refusing to authenticate \
                 cursors under a default key",
            ),
            PageKeyError::Malformed => {
                f.write_str("ORDO_PAGE_KEY must be 64 hex characters (openssl rand -hex 32)")
            }
            PageKeyError::File(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for PageKeyError {}

#[derive(Serialize, Deserialize)]
struct Wire {
    v: String,
    k: Vec<Value>,
}

fn authenticator(
    key: &PageKey,
    space: &str,
    op_name: &str,
    principal: &str,
    payload: &[u8],
) -> [u8; 32] {
    let space_len = (space.len() as u64).to_be_bytes();
    let op_len = (op_name.len() as u64).to_be_bytes();
    let principal_len = (principal.len() as u64).to_be_bytes();
    let payload_len = (payload.len() as u64).to_be_bytes();
    hmac_sha256(
        &key.0,
        &[
            PAGE_DOMAIN,
            &space_len,
            space.as_bytes(),
            &op_len,
            op_name.as_bytes(),
            &principal_len,
            principal.as_bytes(),
            &payload_len,
            payload,
        ],
    )
}

/// Mint a cursor for one space, one op, one version, one principal.
pub fn encode(
    key: &PageKey,
    space: &str,
    op: &OpSpec,
    principal: &str,
    cursor: &PageCursor,
) -> String {
    let wire = Wire {
        v: op.version_hash.clone(),
        k: cursor.keys.clone(),
    };
    let payload = serde_json::to_vec(&wire).expect("cursor serializes");
    let mac = authenticator(key, space, &op.name, principal, &payload);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&payload),
        URL_SAFE_NO_PAD.encode(mac)
    )
}

/// Verify and decode a cursor.
///
/// Anything that fails authentication — forged, tampered, replayed by another
/// principal, carried in from another space, or aimed at another op — is
/// `invalid_cursor`, one answer for
/// all of them, so the failure mode carries no information about which check
/// it failed. A token that authenticates but was minted under a different op
/// version is `version_skew`: keyset columns may no longer mean the same
/// thing, so the client restarts from page one. Tokens are never migrated.
pub fn decode(
    key: &PageKey,
    space: &str,
    op: &OpSpec,
    principal: &str,
    token: &str,
) -> Result<PageCursor, ApiError> {
    let invalid = || {
        ApiError::for_op(
            ErrorCode::InvalidCursor,
            op,
            "unreadable $page token".to_string(),
        )
    };
    let (payload_b64, mac_b64) = token.split_once('.').ok_or_else(invalid)?;
    let payload = URL_SAFE_NO_PAD.decode(payload_b64).map_err(|_| invalid())?;
    let presented = URL_SAFE_NO_PAD.decode(mac_b64).map_err(|_| invalid())?;

    // Authenticate before parsing: nothing in the payload is trusted, or even
    // read, until the MAC says the platform minted it for this caller.
    let expected = authenticator(key, space, &op.name, principal, &payload);
    if !ct_eq(&presented, &expected) {
        return Err(invalid());
    }

    let wire: Wire = serde_json::from_slice(&payload).map_err(|_| invalid())?;
    if wire.v != op.version_hash {
        return Err(ApiError::for_op(
            ErrorCode::VersionSkew,
            op,
            format!(
                "$page token was minted under a different version of `{}` — restart from page one",
                op.name
            ),
        ));
    }
    Ok(PageCursor { keys: wire.k })
}
