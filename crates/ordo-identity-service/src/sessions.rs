//! Sessions (`plan-identity-service.md` 2.2, 5.4, 5.5, Q9).
//!
//! A session is what a sign-in leaves behind: a browser's cookie, a CLI's
//! renewal secret, or a machine key's sign-in. It has a public id (`s_…`,
//! the token's `sid`) and a secret only its holder has; only the secret's
//! tagged hash is stored.
//!
//! A session ends:
//!
//! - **idle**: unused for `ORDO_IDENTITY_SESSION_IDLE_HOURS` (default 168).
//!   Each token it gets counts as use.
//! - **max**: `ORDO_IDENTITY_SESSION_MAX_HOURS` after sign-in (default 720),
//!   however busy. The end is stored at sign-in; lowering the setting later
//!   shortens live sessions too, raising it lengthens none.
//! - **on purpose**: signed out, or the person's access ended.
//!
//! An end found on use is written to the session and to the audit log
//! before the refusal answers.

use ordo_exec::{sql_lit, Pool, TxConn};

use crate::audit::{self, Entry};
use crate::error::{IdentityError, SessionEnd};
use crate::keys::KeyRing;
use crate::secrets::{mint_id, mint_secret, session_hash};
use crate::store::{cell, dep, num, opt_num, text};

/// How the session was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// A browser's cookie at the service's own origin.
    Browser,
    /// `ordo login`'s renewal secret.
    Cli,
    /// A machine key's sign-in.
    Machine,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Browser => "browser",
            SessionKind::Cli => "cli",
            SessionKind::Machine => "machine",
        }
    }

    fn parse(s: &str) -> Option<SessionKind> {
        match s {
            "browser" => Some(SessionKind::Browser),
            "cli" => Some(SessionKind::Cli),
            "machine" => Some(SessionKind::Machine),
            _ => None,
        }
    }
}

/// How long sessions last, from the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lifetimes {
    pub idle_hours: u64,
    pub max_hours: u64,
}

impl Lifetimes {
    fn idle_secs(self) -> u64 {
        self.idle_hours * 3600
    }

    fn max_secs(self) -> u64 {
        self.max_hours * 3600
    }
}

/// What a new session hands its holder. `secret` is shown once.
#[derive(Debug, Clone)]
pub struct Opened {
    pub id: String,
    pub secret: String,
    pub ends_at: u64,
}

/// What to open.
#[derive(Debug, Clone, Copy)]
pub struct NewSession<'a> {
    /// `p_…` or `m_…`.
    pub subject: &'a str,
    pub kind: SessionKind,
    /// `console`, `cli`, or the machine key's name.
    pub client: &'a str,
    /// The passkey assertion that opened it. `None` for a machine.
    pub touched_at: Option<u64>,
    /// A recovery code opened it: it may enrol a passkey and nothing else.
    pub must_enrol: bool,
}

/// Open a session inside `tx`, with a `sign-in` audit row.
pub async fn open(
    tx: &TxConn,
    handle: &str,
    keys: &KeyRing,
    life: Lifetimes,
    s: NewSession<'_>,
    now: u64,
) -> Result<Opened, IdentityError> {
    let id = mint_id("s_");
    let secret = mint_secret();
    let ends_at = now + life.max_secs();
    tx.rows(&format!(
        "INSERT INTO sessions (id, deployment, secret_hash, subject, kind, client, created_at, \
         last_seen_at, touched_at, must_enrol, ends_at) VALUES ({}, {}, {}, {}, {}, {}, {now}, \
         {now}, {}, {}, {ends_at})",
        sql_lit(&id),
        sql_lit(handle),
        sql_lit(&session_hash(&secret)),
        sql_lit(s.subject),
        sql_lit(s.kind.as_str()),
        sql_lit(s.client),
        opt_num(s.touched_at),
        s.must_enrol,
    ))
    .await?;
    audit::append(
        tx,
        handle,
        keys,
        &Entry {
            actor: s.subject,
            action: "sign-in",
            subject: &id,
            outcome: "ok",
            client: s.client,
            detail: s.kind.as_str(),
        },
        now,
    )
    .await?;
    Ok(Opened {
        id,
        secret,
        ends_at,
    })
}

/// A live session, as a token is made from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Live {
    pub id: String,
    pub subject: String,
    pub kind: SessionKind,
    pub client: String,
    pub touched_at: Option<u64>,
    pub must_enrol: bool,
    pub ends_at: u64,
}

impl Live {
    /// Whether its touch is within `window_secs` of `now`.
    pub fn fresh_touch(&self, now: u64, window_secs: u64) -> bool {
        self.touched_at
            .is_some_and(|t| now.saturating_sub(t) <= window_secs)
    }
}

/// Find the session `secret` opens, check it has not ended, and mark it
/// used at `now`. An idle or absolute end found here is written before the
/// refusal.
pub async fn use_session(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    life: Lifetimes,
    secret: &str,
    now: u64,
) -> Result<Live, IdentityError> {
    let tx = pool.begin().await?;
    let rows = tx
        .rows(&format!(
            "SELECT s.id, s.subject, s.kind, s.client, s.touched_at, s.created_at, \
             s.last_seen_at, s.ends_at, s.ended_at, p.ended_at, m.ended_at, s.must_enrol \
             FROM sessions s \
             LEFT JOIN people p ON p.id = s.subject AND p.deployment = s.deployment \
             LEFT JOIN machine_keys m ON m.id = s.subject AND m.deployment = s.deployment \
             WHERE s.{} AND s.secret_hash = {} FOR UPDATE OF s",
            dep(handle),
            sql_lit(&session_hash(secret))
        ))
        .await?;
    let Some(r) = rows.first() else {
        return Err(IdentityError::NotSignedIn);
    };
    let id = text(r, 0);
    let subject = text(r, 1);
    let kind = cell(r, 2)
        .and_then(SessionKind::parse)
        .ok_or_else(|| IdentityError::Internal(format!("session {id} has no known kind")))?;
    let created_at = num(r, 5).unwrap_or(0);
    let last_seen = num(r, 6).unwrap_or(0);
    let ends_at = num(r, 7).unwrap_or(0).min(created_at + life.max_secs());
    let holder_ended = num(r, 9).is_some() || num(r, 10).is_some();
    if num(r, 8).is_some() || holder_ended {
        return Err(IdentityError::SessionEnded(SessionEnd::Ended));
    }
    let end = if now >= ends_at {
        Some(SessionEnd::Absolute {
            limit_hours: life.max_hours,
        })
    } else if now.saturating_sub(last_seen) >= life.idle_secs() {
        Some(SessionEnd::Idle {
            limit_hours: life.idle_hours,
        })
    } else {
        None
    };
    if let Some(end) = end {
        finish(&tx, handle, keys, &id, &subject, "service", end.why(), now).await?;
        tx.commit().await?;
        return Err(IdentityError::SessionEnded(end));
    }
    tx.rows(&format!(
        "UPDATE sessions SET last_seen_at = {now} WHERE {} AND id = {}",
        dep(handle),
        sql_lit(&id)
    ))
    .await?;
    tx.commit().await?;
    Ok(Live {
        id,
        subject,
        kind,
        client: text(r, 3),
        touched_at: num(r, 4),
        must_enrol: cell(r, 11) == Some("t"),
        ends_at,
    })
}

#[allow(clippy::too_many_arguments)]
async fn finish(
    tx: &TxConn,
    handle: &str,
    keys: &KeyRing,
    id: &str,
    subject: &str,
    actor: &str,
    why: &str,
    now: u64,
) -> Result<(), IdentityError> {
    tx.rows(&format!(
        "UPDATE sessions SET ended_at = {now}, ended_why = {} WHERE {} AND id = {} \
         AND ended_at IS NULL",
        sql_lit(why),
        dep(handle),
        sql_lit(id)
    ))
    .await?;
    audit::append(
        tx,
        handle,
        keys,
        &Entry {
            actor,
            action: "session.ended",
            subject: id,
            outcome: "ok",
            client: "",
            detail: &format!("{why} {subject}"),
        },
        now,
    )
    .await?;
    Ok(())
}

/// Sign out: end the session `secret` opens. Ending one already ended, or
/// one nobody holds, answers the same as ending a live one, so a stale
/// cookie signs out cleanly.
pub async fn sign_out(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    secret: &str,
    now: u64,
) -> Result<(), IdentityError> {
    let tx = pool.begin().await?;
    let rows = tx
        .rows(&format!(
            "SELECT id, subject FROM sessions WHERE {} AND secret_hash = {} \
             AND ended_at IS NULL FOR UPDATE",
            dep(handle),
            sql_lit(&session_hash(secret))
        ))
        .await?;
    if let Some(r) = rows.first() {
        let (id, subject) = (text(r, 0), text(r, 1));
        finish(
            &tx,
            handle,
            keys,
            &id,
            &subject,
            &subject,
            "signed-out",
            now,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Record a fresh passkey touch on a session, inside `tx`.
pub async fn touched(tx: &TxConn, handle: &str, id: &str, now: u64) -> Result<(), IdentityError> {
    tx.rows(&format!(
        "UPDATE sessions SET touched_at = {now}, last_seen_at = {now} WHERE {} AND id = {} \
         AND ended_at IS NULL",
        dep(handle),
        sql_lit(id)
    ))
    .await?;
    Ok(())
}

/// The session is no longer held to enrolling a passkey, and the passkey
/// just made is its touch. Inside `tx`.
pub async fn enrolled(tx: &TxConn, handle: &str, id: &str, now: u64) -> Result<(), IdentityError> {
    tx.rows(&format!(
        "UPDATE sessions SET must_enrol = false, touched_at = {now} WHERE {} AND id = {}",
        dep(handle),
        sql_lit(id)
    ))
    .await?;
    Ok(())
}

/// End every live session of `subject` for `why`, inside `tx`. Returns how
/// many ended.
pub async fn end_all_of(
    tx: &TxConn,
    handle: &str,
    subject: &str,
    why: &str,
    now: u64,
) -> Result<u64, IdentityError> {
    Ok(tx
        .rows(&format!(
            "UPDATE sessions SET ended_at = {now}, ended_why = {} WHERE {} AND subject = {} \
             AND ended_at IS NULL RETURNING id",
            sql_lit(why),
            dep(handle),
            sql_lit(subject)
        ))
        .await?
        .len() as u64)
}

/// Whether the session `id` is live. The service checks a bearer token's
/// `sid` with it on its own routes, so ending access there is at once.
pub async fn is_live(pool: &Pool, handle: &str, id: &str) -> Result<bool, IdentityError> {
    Ok(!pool
        .rows(&format!(
            "SELECT 1 FROM sessions WHERE {} AND id = {} AND ended_at IS NULL",
            dep(handle),
            sql_lit(id)
        ))
        .await?
        .is_empty())
}
