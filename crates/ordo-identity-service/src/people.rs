//! Deployment people (`plan-identity-service.md` 2.2, 5.7).
//!
//! A person is `p_…`, a display name and a role (`admin`, the only one).
//! People arrive by invite (session 2) or by the console import (session
//! 3); both call [`add`] inside their own transaction. Access ends with
//! [`end`], which ends every session, voids recovery codes and revokes the
//! unspent invites the person issued. The last live admin cannot be ended.

use ordo_exec::{sql_lit, Pool, TxConn};
use ordo_identity::deployment_token::ROLE_ADMIN;

use crate::audit::{self, Entry};
use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::secrets::mint_id;
use crate::store::{dep, num, text};

/// One person as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub display_name: String,
    pub role: String,
    pub created_at: u64,
    pub ended_at: Option<u64>,
}

/// Who acts, and through what, for the audit row.
#[derive(Debug, Clone, Copy)]
pub struct Actor<'a> {
    /// `p_…`, `m_…`, `unix:<user>`, `boot`.
    pub id: &'a str,
    /// `console`, `cli`, a machine key's name, or empty.
    pub client: &'a str,
}

/// Add a person inside `tx`, with an audit row.
pub async fn add(
    tx: &TxConn,
    handle: &str,
    keys: &KeyRing,
    display_name: &str,
    by: Actor<'_>,
    now: u64,
) -> Result<Person, IdentityError> {
    let name = display_name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(IdentityError::BadRequest(
            "a display name is 1 to 100 characters".into(),
        ));
    }
    let id = mint_id("p_");
    tx.rows(&format!(
        "INSERT INTO people (id, deployment, display_name, role, created_at) \
         VALUES ({}, {}, {}, {}, {now})",
        sql_lit(&id),
        sql_lit(handle),
        sql_lit(name),
        sql_lit(ROLE_ADMIN),
    ))
    .await?;
    audit::append(
        tx,
        handle,
        keys,
        &Entry {
            actor: by.id,
            action: "person.added",
            subject: &id,
            outcome: "ok",
            client: by.client,
            detail: ROLE_ADMIN,
        },
        now,
    )
    .await?;
    Ok(Person {
        id,
        display_name: name.to_string(),
        role: ROLE_ADMIN.to_string(),
        created_at: now,
        ended_at: None,
    })
}

/// One person, live or ended.
pub async fn get(pool: &Pool, handle: &str, id: &str) -> Result<Option<Person>, IdentityError> {
    let rows = pool
        .rows(&format!(
            "SELECT id, display_name, role, created_at, ended_at FROM people \
             WHERE {} AND id = {}",
            dep(handle),
            sql_lit(id)
        ))
        .await?;
    Ok(rows.first().map(|r| Person {
        id: text(r, 0),
        display_name: text(r, 1),
        role: text(r, 2),
        created_at: num(r, 3).unwrap_or(0),
        ended_at: num(r, 4),
    }))
}

/// Every person, live and ended, oldest first.
pub async fn list(pool: &Pool, handle: &str) -> Result<Vec<Person>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT id, display_name, role, created_at, ended_at FROM people WHERE {} \
             ORDER BY created_at, id",
            dep(handle)
        ))
        .await?
        .iter()
        .map(|r| Person {
            id: text(r, 0),
            display_name: text(r, 1),
            role: text(r, 2),
            created_at: num(r, 3).unwrap_or(0),
            ended_at: num(r, 4),
        })
        .collect())
}

/// What ending a person ended with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended {
    pub sessions: u64,
    pub recovery_codes: u64,
    pub invites: u64,
}

/// End a person's access: every session, every live recovery code, every
/// unspent invite they issued. Refuses the last live admin.
pub async fn end(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    id: &str,
    by: Actor<'_>,
    now: u64,
) -> Result<Ended, IdentityError> {
    let tx = pool.begin().await?;
    // Every live admin row locked, so two admins ending each other at once
    // cannot leave none.
    let live: Vec<String> = tx
        .rows(&format!(
            "SELECT id FROM people WHERE {} AND ended_at IS NULL AND role = {} \
             ORDER BY id FOR UPDATE",
            dep(handle),
            sql_lit(ROLE_ADMIN)
        ))
        .await?
        .iter()
        .map(|r| text(r, 0))
        .collect();
    if !live.iter().any(|p| p == id) {
        return Err(IdentityError::UnknownPerson);
    }
    if live.len() == 1 {
        return Err(IdentityError::LastAdmin);
    }
    let (d, p) = (dep(handle), sql_lit(id));
    tx.rows(&format!(
        "UPDATE people SET ended_at = {now}, ended_by = {} WHERE {d} AND id = {p}",
        sql_lit(by.id)
    ))
    .await?;
    let sessions = tx
        .rows(&format!(
            "UPDATE sessions SET ended_at = {now}, ended_why = 'person-ended' \
             WHERE {d} AND subject = {p} AND ended_at IS NULL RETURNING id"
        ))
        .await?
        .len() as u64;
    let recovery_codes = tx
        .rows(&format!(
            "UPDATE recovery_codes SET voided_at = {now} WHERE {d} AND person_id = {p} \
             AND used_at IS NULL AND voided_at IS NULL RETURNING id"
        ))
        .await?
        .len() as u64;
    let invites = tx
        .rows(&format!(
            "UPDATE invites SET revoked_at = {now} WHERE {d} AND issued_by = {p} \
             AND used_at IS NULL AND revoked_at IS NULL RETURNING id"
        ))
        .await?
        .len() as u64;
    let detail = format!("sessions={sessions} recovery_codes={recovery_codes} invites={invites}");
    audit::append(
        &tx,
        handle,
        keys,
        &Entry {
            actor: by.id,
            action: "person.ended",
            subject: id,
            outcome: "ok",
            client: by.client,
            detail: &detail,
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(Ended {
        sessions,
        recovery_codes,
        invites,
    })
}
