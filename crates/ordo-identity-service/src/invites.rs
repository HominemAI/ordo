//! Invite links (`plan-identity-service.md` 5.2, 5.3).
//!
//! An invite is a single-use secret (a 120 bit code) in a link,
//! `<origin>/join#<secret>`: the secret rides in the fragment, so it
//! reaches no server log. Only its tagged hash is stored. It lasts
//! `ORDO_IDENTITY_INVITE_DAYS` (default 7).
//!
//! Unknown, expired, spent and revoked all answer the same
//! ([`IdentityError::CodeRefused`]); the audit log has which.

use ordo_exec::{sql_lit, Pool, TxConn};
use ordo_identity::codes::{mint_code, normalise_code};
use ordo_identity::deployment_token::ROLE_ADMIN;
use serde_json::{json, Value};

use crate::audit::{self, Entry};
use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::people::Actor;
use crate::secrets::{invite_hash, mint_id};
use crate::store::{cell, dep, num, text};

/// The `issued_by` of the invite minted at first boot (5.2).
pub const FIRST_BOOT: &str = "boot";

/// A new invite, as its issuer sees it once.
#[derive(Debug, Clone)]
pub struct Minted {
    pub id: String,
    pub secret: String,
    pub link: String,
    pub expires_at: u64,
}

/// The link an invite secret travels in.
pub fn link(origin: &str, secret: &str) -> String {
    format!("{origin}/join#{secret}")
}

/// Mint an invite inside `tx`, with an `invite.issued` row.
#[allow(clippy::too_many_arguments)]
pub async fn mint(
    tx: &TxConn,
    handle: &str,
    keys: &KeyRing,
    origin: &str,
    label: &str,
    by: Actor<'_>,
    life_days: u64,
    now: u64,
) -> Result<Minted, IdentityError> {
    let label = label.trim();
    if label.chars().count() > 100 {
        return Err(IdentityError::BadRequest(
            "an invite's label is at most 100 characters".into(),
        ));
    }
    let id = mint_id("i_");
    let secret = mint_code();
    let expires_at = now + life_days * 86_400;
    tx.rows(&format!(
        "INSERT INTO invites (id, deployment, secret_hash, label, role, issued_by, issued_at, \
         expires_at, created_at) VALUES ({}, {}, {}, {}, {}, {}, {now}, {expires_at}, {now})",
        sql_lit(&id),
        sql_lit(handle),
        sql_lit(&invite_hash(&secret)),
        sql_lit(label),
        sql_lit(ROLE_ADMIN),
        sql_lit(by.id),
    ))
    .await?;
    audit::append(
        tx,
        handle,
        keys,
        &Entry {
            actor: by.id,
            action: "invite.issued",
            subject: &id,
            outcome: "ok",
            client: by.client,
            detail: label,
        },
        now,
    )
    .await?;
    Ok(Minted {
        link: link(origin, &secret),
        id,
        secret,
        expires_at,
    })
}

/// A live invite.
#[derive(Debug, Clone)]
pub struct Live {
    pub id: String,
    pub hash: String,
    pub label: String,
    pub issued_by: String,
}

/// The live invite `typed` opens, or [`IdentityError::CodeRefused`] with
/// the reason written to the audit log.
pub async fn check(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    typed: &str,
    now: u64,
) -> Result<Live, IdentityError> {
    let refuse = |subject: String, why: &'static str| async move {
        audit::record(
            pool,
            handle,
            keys,
            &Entry {
                actor: "anonymous",
                action: "invite.refused",
                subject: &subject,
                outcome: why,
                client: "",
                detail: "",
            },
            now,
        )
        .await?;
        Err::<Live, _>(IdentityError::CodeRefused)
    };
    let Some(secret) = normalise_code(typed) else {
        return refuse(String::new(), "malformed").await;
    };
    let hash = invite_hash(&secret);
    let rows = pool
        .rows(&format!(
            "SELECT id, label, issued_by, expires_at, used_at, revoked_at FROM invites \
             WHERE {} AND secret_hash = {}",
            dep(handle),
            sql_lit(&hash)
        ))
        .await?;
    let Some(r) = rows.first() else {
        return refuse(String::new(), "unknown").await;
    };
    let id = text(r, 0);
    if cell(r, 5).is_some() {
        return refuse(id, "revoked").await;
    }
    if cell(r, 4).is_some() {
        return refuse(id, "spent").await;
    }
    if num(r, 3).unwrap_or(0) <= now {
        return refuse(id, "expired").await;
    }
    Ok(Live {
        id,
        hash,
        label: text(r, 1),
        issued_by: text(r, 2),
    })
}

/// Spend an invite for `person` inside `tx`. `false` when it was spent,
/// revoked or expired in between.
pub async fn spend(
    tx: &TxConn,
    handle: &str,
    id: &str,
    person: &str,
    now: u64,
) -> Result<bool, IdentityError> {
    Ok(!tx
        .rows(&format!(
            "UPDATE invites SET used_at = {now}, used_by = {} WHERE {} AND id = {} \
             AND used_at IS NULL AND revoked_at IS NULL AND expires_at > {now} RETURNING 1",
            sql_lit(person),
            dep(handle),
            sql_lit(id)
        ))
        .await?
        .is_empty())
}

/// Revoke an unspent invite.
pub async fn revoke(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    id: &str,
    by: Actor<'_>,
    now: u64,
) -> Result<(), IdentityError> {
    let tx = pool.begin().await?;
    let done = tx
        .rows(&format!(
            "UPDATE invites SET revoked_at = {now} WHERE {} AND id = {} AND used_at IS NULL \
             AND revoked_at IS NULL RETURNING 1",
            dep(handle),
            sql_lit(id)
        ))
        .await?;
    if done.is_empty() {
        return Err(IdentityError::UnknownInvite);
    }
    audit::append(
        &tx,
        handle,
        keys,
        &Entry {
            actor: by.id,
            action: "invite.revoked",
            subject: id,
            outcome: "ok",
            client: by.client,
            detail: "",
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Every invite, newest first, without secrets.
pub async fn list(pool: &Pool, handle: &str, now: u64) -> Result<Vec<Value>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT id, label, role, issued_by, issued_at, expires_at, used_at, used_by, \
             revoked_at FROM invites WHERE {} ORDER BY issued_at DESC, id",
            dep(handle)
        ))
        .await?
        .iter()
        .map(|r| {
            let state = if cell(r, 8).is_some() {
                "revoked"
            } else if cell(r, 6).is_some() {
                "used"
            } else if num(r, 5).unwrap_or(0) <= now {
                "expired"
            } else {
                "open"
            };
            json!({
                "id": text(r, 0),
                "label": text(r, 1),
                "role": text(r, 2),
                "issuedBy": text(r, 3),
                "issuedAt": num(r, 4),
                "expiresAt": num(r, 5),
                "usedAt": num(r, 6),
                "usedBy": cell(r, 7),
                "revokedAt": num(r, 8),
                "state": state,
            })
        })
        .collect())
}
