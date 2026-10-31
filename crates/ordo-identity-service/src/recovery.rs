//! Recovery codes (`plan-identity-service.md` 2.1, 2.2).
//!
//! Ten single-use codes per person, shown once at sign-up and on renewal.
//! The shape and the hashing are `ordo_identity::codes` and argon2id, the
//! console's, so codes the console printed keep working once moved (6.1).
//! A code signs a browser in with a session that may only enrol a new
//! passkey; enrolling one retires the person's other passkeys and issues a
//! new set.

use std::sync::Arc;

use ordo_exec::{sql_lit, Pool, TxConn};
use ordo_identity::codes::{mint_recovery_set, split_recovery, RecoveryRow};
use ordo_identity::provider::PasswordHasher;

use crate::audit::{self, Entry};
use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::sessions::{self, Lifetimes, NewSession, Opened, SessionKind};
use crate::store::{dep, text};

/// Run slow work (argon2id) off the async workers.
pub async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, IdentityError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| IdentityError::Internal(format!("a blocking task did not finish: {e}")))
}

/// How many of a person's codes are still unused and not voided.
pub async fn left(pool: &Pool, handle: &str, person: &str) -> Result<u64, IdentityError> {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM recovery_codes WHERE {} AND person_id = {} \
             AND used_at IS NULL AND voided_at IS NULL",
            dep(handle),
            sql_lit(person)
        ))
        .await?;
    Ok(rows
        .first()
        .map(|r| text(r, 0))
        .and_then(|t| t.parse().ok())
        .unwrap_or(0))
}

/// Ten fresh codes: as shown once, and as stored.
pub async fn mint(
    hasher: &Arc<dyn PasswordHasher>,
) -> Result<(Vec<String>, Vec<RecoveryRow>), IdentityError> {
    let hasher = hasher.clone();
    blocking(move || mint_recovery_set(hasher.as_ref())).await
}

/// Void a person's live codes and store a new set, inside `tx`.
pub async fn replace(
    tx: &TxConn,
    handle: &str,
    person: &str,
    rows: &[RecoveryRow],
    now: u64,
) -> Result<(), IdentityError> {
    tx.rows(&format!(
        "UPDATE recovery_codes SET voided_at = {now} WHERE {} AND person_id = {} \
         AND used_at IS NULL AND voided_at IS NULL",
        dep(handle),
        sql_lit(person)
    ))
    .await?;
    for r in rows {
        tx.rows(&format!(
            "INSERT INTO recovery_codes (deployment, person_id, lookup, hash, issued_at, \
             created_at) VALUES ({}, {}, {}, {}, {now}, {now})",
            sql_lit(handle),
            sql_lit(person),
            sql_lit(&r.lookup),
            sql_lit(&r.hash)
        ))
        .await?;
    }
    Ok(())
}

/// Sign a browser in with a recovery code: a session that must enrol a
/// passkey before anything else. Every refusal is
/// [`IdentityError::CodeRefused`].
pub async fn redeem(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    hasher: &Arc<dyn PasswordHasher>,
    life: Lifetimes,
    typed: &str,
    now: u64,
) -> Result<(String, Opened), IdentityError> {
    let (lookup, secret) = split_recovery(typed).ok_or(IdentityError::CodeRefused)?;
    let rows = pool
        .rows(&format!(
            "SELECT c.person_id, c.hash FROM recovery_codes c JOIN people p ON p.id = c.person_id \
             AND p.deployment = c.deployment WHERE c.{} AND c.lookup = {} AND c.used_at IS NULL \
             AND c.voided_at IS NULL AND p.ended_at IS NULL",
            dep(handle),
            sql_lit(&lookup)
        ))
        .await?;
    let Some(r) = rows.first() else {
        return Err(IdentityError::CodeRefused);
    };
    let (person, hash) = (text(r, 0), text(r, 1));
    let h = hasher.clone();
    if !blocking(move || h.verify(&secret, &hash)).await? {
        return Err(IdentityError::CodeRefused);
    }
    let tx = pool.begin().await?;
    let spent = tx
        .rows(&format!(
            "UPDATE recovery_codes SET used_at = {now} WHERE {} AND lookup = {} \
             AND person_id = {} AND used_at IS NULL AND voided_at IS NULL RETURNING 1",
            dep(handle),
            sql_lit(&lookup),
            sql_lit(&person)
        ))
        .await?;
    if spent.is_empty() {
        return Err(IdentityError::CodeRefused);
    }
    audit::append(
        &tx,
        handle,
        keys,
        &Entry {
            actor: &person,
            action: "recovery-code.used",
            subject: &person,
            outcome: "ok",
            client: "console",
            detail: &lookup,
        },
        now,
    )
    .await?;
    let opened = sessions::open(
        &tx,
        handle,
        keys,
        life,
        NewSession {
            subject: &person,
            kind: SessionKind::Browser,
            client: "console",
            touched_at: None,
            must_enrol: true,
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok((person, opened))
}
