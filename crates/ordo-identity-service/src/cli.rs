//! CLI grants: `ordo login` through the browser (`plan-identity-service.md`
//! 5.5).
//!
//! The CLI listens on loopback, makes a PKCE verifier, and opens the
//! browser at the service's approval page (step 2's page). A signed-in
//! person with a fresh touch approves: [`approve`] mints a one-time code,
//! 60 seconds, bound to the PKCE challenge, and the page redirects it to
//! the CLI's loopback port. The CLI calls [`redeem`] with the code and its
//! verifier and gets a CLI session. With its existing session secret (for
//! `ordo login --touch`) the same redeem refreshes that session's touch and
//! opens nothing new.

use ordo_exec::{sql_lit, Pool};
use ordo_identity::codes::mint_code;
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use sha2::{Digest, Sha256};

use crate::audit::{self, Entry};
use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::secrets::{cli_code_hash, session_hash};
use crate::sessions::{self, Lifetimes, NewSession, Opened, SessionKind};
use crate::store::{dep, num, text};

/// A code lives 60 seconds.
pub const GRANT_SECS: u64 = 60;

/// A PKCE S256 challenge: 32 bytes of SHA-256, base64url.
fn valid_challenge(c: &str) -> bool {
    b64url_decode(c).is_ok_and(|b| b.len() == 32)
}

/// What `S256(verifier)` is.
pub fn pkce_challenge(verifier: &str) -> String {
    b64url_encode(&Sha256::digest(verifier.as_bytes()))
}

/// Mint a code for `person`, whose last touch was `touched_at`.
pub async fn approve(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    person: &str,
    code_challenge: &str,
    touched_at: u64,
    now: u64,
) -> Result<String, IdentityError> {
    if !valid_challenge(code_challenge) {
        return Err(IdentityError::BadRequest(
            "`codeChallenge` is not a PKCE S256 challenge (base64url of 32 bytes)".into(),
        ));
    }
    let code = mint_code();
    let tx = pool.begin().await?;
    tx.rows(&format!(
        "INSERT INTO cli_grants (code_hash, deployment, person_id, code_challenge, touched_at, \
         expires_at, created_at) VALUES ({}, {}, {}, {}, {touched_at}, {}, {now})",
        sql_lit(&cli_code_hash(&code)),
        sql_lit(handle),
        sql_lit(person),
        sql_lit(code_challenge),
        now + GRANT_SECS,
    ))
    .await?;
    audit::append(
        &tx,
        handle,
        keys,
        &Entry {
            actor: person,
            action: "cli.approved",
            subject: person,
            outcome: "ok",
            client: "cli",
            detail: "",
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(code)
}

/// What a redeem did.
#[derive(Debug, Clone)]
pub enum Redeemed {
    /// A new CLI session.
    Opened { person: String, session: Opened },
    /// The CLI's existing session has a fresh touch.
    Touched { person: String, session_id: String },
}

/// Spend a code with its PKCE verifier. `existing` is the CLI's session
/// secret for a touch-only redeem. Every refusal is
/// [`IdentityError::CodeRefused`].
#[allow(clippy::too_many_arguments)]
pub async fn redeem(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    life: Lifetimes,
    code: &str,
    verifier: &str,
    existing: Option<&str>,
    now: u64,
) -> Result<Redeemed, IdentityError> {
    let tx = pool.begin().await?;
    let rows = tx
        .rows(&format!(
            "UPDATE cli_grants g SET used_at = {now} FROM people p WHERE g.{} \
             AND g.code_hash = {} AND g.used_at IS NULL AND g.expires_at > {now} \
             AND g.code_challenge = {} AND p.id = g.person_id AND p.deployment = g.deployment \
             AND p.ended_at IS NULL RETURNING g.person_id, g.touched_at",
            dep(handle),
            sql_lit(&cli_code_hash(code.trim())),
            sql_lit(&pkce_challenge(verifier)),
        ))
        .await?;
    let Some(r) = rows.first() else {
        return Err(IdentityError::CodeRefused);
    };
    let person = text(r, 0);
    let touched_at = num(r, 1).unwrap_or(now);
    if let Some(secret) = existing {
        let found = tx
            .rows(&format!(
                "UPDATE sessions SET touched_at = {touched_at}, last_seen_at = {now} WHERE {} \
                 AND secret_hash = {} AND subject = {} AND kind = 'cli' AND ended_at IS NULL \
                 RETURNING id",
                dep(handle),
                sql_lit(&session_hash(secret)),
                sql_lit(&person),
            ))
            .await?;
        let Some(s) = found.first() else {
            return Err(IdentityError::CodeRefused);
        };
        let session_id = text(s, 0);
        audit::append(
            &tx,
            handle,
            keys,
            &Entry {
                actor: &person,
                action: "touch",
                subject: &session_id,
                outcome: "ok",
                client: "cli",
                detail: "",
            },
            now,
        )
        .await?;
        tx.commit().await?;
        return Ok(Redeemed::Touched { person, session_id });
    }
    let session = sessions::open(
        &tx,
        handle,
        keys,
        life,
        NewSession {
            subject: &person,
            kind: SessionKind::Cli,
            client: "cli",
            touched_at: Some(touched_at),
            must_enrol: false,
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(Redeemed::Opened { person, session })
}
