//! The console import (`plan-identity-service.md` 6.1, 6.2, Q7).
//!
//! An upgrade affects nothing: the console's owners become deployment
//! people, with their passkeys and their live recovery codes, so nobody
//! re-enrols. The service runs it itself before it serves, every start,
//! and it does something only when `ORDO_IDENTITY_IMPORT_CONSOLE_URL` is
//! set and `imports` has no row for `console:<handle>`.
//!
//! - The console's database is only read, in a read-only transaction. Its
//!   tables are never written, so a rollback to the old image finds them as
//!   they were.
//! - Everything is copied in one transaction on the service's own database,
//!   the `imports` row last. A crash part way imports nothing; the next
//!   start does it all. Each copy is `ON CONFLICT DO NOTHING` on the
//!   console's own ids, so a run again (or a second console) never makes a
//!   duplicate.
//! - Every owner moves, removed ones too: a live owner is a live admin, a
//!   removed one a person ended at `removed_at` by `console`, so old audit
//!   lines still name someone. Every passkey moves column for column, the
//!   user handle as it is. Every recovery code that is unused and not
//!   voided moves with its lookup and hash, so printed codes still work.
//!   Sessions and setup codes stay behind; each unspent setup code gets an
//!   audit row naming it voided, so an admin knows to issue a new link.
//! - The `imports` row keeps the RP ID in force. A later start under
//!   another RP ID warns that imported passkeys will not sign in (6.2).

use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError, PoolLimits, TextRow, TxConn};

use crate::audit::{self, Entry};
use crate::config::IMPORT_CONSOLE_URL_ENV;
use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::secrets::mint_id;
use crate::store::{cell, dep, num, text};

/// The `imports` row's source for a console of `handle`.
pub fn source(handle: &str) -> String {
    format!("console:{handle}")
}

/// What one import moved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    pub people: u64,
    /// Of them, live admins.
    pub live_admins: u64,
    pub passkeys: u64,
    pub recovery_codes: u64,
    /// Unspent console setup codes left behind, each named in the audit.
    pub setup_codes_voided: u64,
}

/// What the import step found at this start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportStep {
    /// What this start moved, when it ran.
    pub imported: Option<Imported>,
    /// `(at import, now)` when an earlier import ran under another RP ID:
    /// the imported passkeys will not sign in here.
    pub rp_id_changed: Option<(String, String)>,
}

/// Run the import if it is due.
pub async fn run(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    console: Option<&PoolConfig>,
    rp_id: &str,
    now: u64,
) -> Result<ImportStep, IdentityError> {
    let mut step = ImportStep::default();
    let done = pool
        .rows(&format!(
            "SELECT rp_id FROM imports WHERE {} AND source = {}",
            dep(handle),
            sql_lit(&source(handle))
        ))
        .await?;
    if let Some(r) = done.first() {
        let was = text(r, 0);
        if was != rp_id {
            eprintln!(
                "ordo-identity-service: warning: the console's passkeys were imported under \
                 RP ID `{was}`, and this start runs under `{rp_id}`. Those passkeys will not \
                 sign in here. Set ORDO_IDENTITY_RP_ID to `{was}` (and ORDO_IDENTITY_ORIGIN to \
                 a host at or under it) to keep them, or send each person a new invite."
            );
            step.rp_id_changed = Some((was, rp_id.to_string()));
        }
        return Ok(step);
    }
    let Some(console) = console else {
        return Ok(step);
    };
    let read = read_console(console, handle, now).await?;
    let tx = pool.begin().await?;
    tx.rows(&format!(
        "SELECT pg_advisory_xact_lock(hashtext({}))",
        sql_lit(&format!("ordo-identity-import:{handle}"))
    ))
    .await?;
    // Another instance may have finished it while this one read.
    let raced = !tx
        .rows(&format!(
            "SELECT 1 FROM imports WHERE {} AND source = {}",
            dep(handle),
            sql_lit(&source(handle))
        ))
        .await?
        .is_empty();
    if raced {
        tx.commit().await?;
        return Ok(step);
    }
    let imported = copy(&tx, handle, keys, &read, rp_id, now).await?;
    tx.commit().await?;
    eprintln!(
        "ordo-identity-service: moved the console's owners in: {} people ({} live admins), {} \
         passkeys, {} recovery codes. They sign in with the passkeys they have.",
        imported.people, imported.live_admins, imported.passkeys, imported.recovery_codes
    );
    step.imported = Some(imported);
    Ok(step)
}

/// The console's rows of one deployment.
#[derive(Debug, Default)]
struct Read {
    owners: Vec<TextRow>,
    passkeys: Vec<TextRow>,
    codes: Vec<TextRow>,
    setup_codes: Vec<TextRow>,
}

fn unreadable(console: &PoolConfig, e: PoolError) -> IdentityError {
    IdentityError::setting_caused(
        IMPORT_CONSOLE_URL_ENV,
        format!(
            "names database `{}` on {}:{}, and the console's owners could not be read from it",
            console.db, console.host, console.port
        ),
        format!(
            "Point {IMPORT_CONSOLE_URL_ENV} at the console's database with a role that may \
             read it, or unset it if there is no console to move owners from"
        ),
        e,
    )
}

async fn read_console(console: &PoolConfig, handle: &str, now: u64) -> Result<Read, IdentityError> {
    let mut cfg = console.clone();
    cfg.size = 1;
    let pool = Pool::connect_own(cfg, PoolLimits::default())
        .await
        .map_err(|e| unreadable(console, e))?;
    let tx = pool.begin().await.map_err(|e| unreadable(console, e))?;
    tx.rows("SET TRANSACTION READ ONLY")
        .await
        .map_err(|e| unreadable(console, e))?;
    let d = sql_lit(handle);
    let queries = [
        format!(
            "SELECT id, label, created_at, removed_at FROM owner WHERE deployment = {d} \
             ORDER BY created_at, id"
        ),
        format!(
            "SELECT credential_id, owner_id, algorithm, public_key, sign_count, user_handle, \
             backup_eligible, backed_up, registered_at, last_used_at FROM passkeys \
             WHERE deployment = {d} ORDER BY registered_at, credential_id"
        ),
        format!(
            "SELECT owner_id, lookup, hash, issued_at FROM recovery_codes WHERE deployment = {d} \
             AND used_at IS NULL AND voided_at IS NULL ORDER BY id"
        ),
        format!(
            "SELECT code_hash, kind, issued_by, issued_at FROM setup_codes WHERE deployment = {d} \
             AND used_at IS NULL AND voided_at IS NULL AND (expires_at IS NULL OR expires_at > \
             {now}) ORDER BY issued_at, code_hash"
        ),
    ];
    let mut out = Vec::with_capacity(4);
    for q in &queries {
        match tx.rows(q).await {
            Ok(rows) => out.push(rows),
            // A console database with no owner tables has no owners to move.
            Err(e) if e.is_missing_table() => return Ok(Read::default()),
            Err(e) => return Err(unreadable(console, e)),
        }
    }
    let _ = tx.rollback().await;
    let mut it = out.into_iter();
    Ok(Read {
        owners: it.next().unwrap_or_default(),
        passkeys: it.next().unwrap_or_default(),
        codes: it.next().unwrap_or_default(),
        setup_codes: it.next().unwrap_or_default(),
    })
}

async fn copy(
    tx: &TxConn,
    handle: &str,
    keys: &KeyRing,
    read: &Read,
    rp_id: &str,
    now: u64,
) -> Result<Imported, IdentityError> {
    let mut out = Imported::default();
    let d = sql_lit(handle);
    for o in &read.owners {
        let owner = text(o, 0);
        let label = cell(o, 1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or("Console owner")
            .chars()
            .take(100)
            .collect::<String>();
        let created = num(o, 2).unwrap_or(now);
        let removed = num(o, 3);
        let made = tx
            .rows(&format!(
                "INSERT INTO people (id, deployment, display_name, role, console_owner_id, \
                 created_at, ended_at, ended_by) VALUES ({}, {d}, {}, 'admin', {}, {created}, \
                 {}, {}) ON CONFLICT (deployment, console_owner_id) \
                 WHERE console_owner_id IS NOT NULL DO NOTHING RETURNING id",
                sql_lit(&mint_id("p_")),
                sql_lit(&label),
                sql_lit(&owner),
                removed
                    .map(|r| r.to_string())
                    .unwrap_or_else(|| "NULL".into()),
                if removed.is_some() {
                    "'console'"
                } else {
                    "NULL"
                },
            ))
            .await?;
        if let Some(r) = made.first() {
            let person = text(r, 0);
            out.people += 1;
            if removed.is_none() {
                out.live_admins += 1;
            }
            audit::append(
                tx,
                handle,
                keys,
                &Entry {
                    actor: "boot",
                    action: "person.imported",
                    subject: &person,
                    outcome: "ok",
                    client: "",
                    detail: &format!(
                        "console owner {owner}{}",
                        if removed.is_some() { ", removed" } else { "" }
                    ),
                },
                now,
            )
            .await?;
        }
    }
    // The person each console owner became, this run or an earlier one.
    let person_of = |owner: &str| {
        format!(
            "(SELECT id FROM people WHERE deployment = {d} AND console_owner_id = {})",
            sql_lit(owner)
        )
    };
    for p in &read.passkeys {
        let n = |i| num(p, i).unwrap_or(0);
        let flag = |i| {
            if cell(p, i) == Some("t") {
                "true"
            } else {
                "false"
            }
        };
        let moved = tx
            .rows(&format!(
                "INSERT INTO passkeys (deployment, credential_id, person_id, algorithm, \
                 public_key, sign_count, user_handle, backup_eligible, backed_up, registered_at, \
                 last_used_at, created_at) SELECT {d}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, \
                 {now} WHERE {} IS NOT NULL ON CONFLICT (deployment, credential_id) DO NOTHING \
                 RETURNING 1",
                sql_lit(&text(p, 0)),
                person_of(&text(p, 1)),
                cell(p, 2).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
                sql_lit(&text(p, 3)),
                n(4),
                sql_lit(&text(p, 5)),
                flag(6),
                flag(7),
                n(8),
                n(9),
                person_of(&text(p, 1)),
            ))
            .await?;
        out.passkeys += moved.len() as u64;
    }
    for c in &read.codes {
        let moved = tx
            .rows(&format!(
                "INSERT INTO recovery_codes (deployment, person_id, lookup, hash, issued_at, \
                 created_at) SELECT {d}, {}, {}, {}, {}, {now} WHERE {} IS NOT NULL \
                 ON CONFLICT DO NOTHING RETURNING 1",
                person_of(&text(c, 0)),
                sql_lit(&text(c, 1)),
                sql_lit(&text(c, 2)),
                num(c, 3).unwrap_or(now),
                person_of(&text(c, 0)),
            ))
            .await?;
        out.recovery_codes += moved.len() as u64;
    }
    for s in &read.setup_codes {
        let hash = text(s, 0);
        audit::append(
            tx,
            handle,
            keys,
            &Entry {
                actor: "boot",
                action: "setup-code.voided",
                subject: &format!("console-setup:{}", &hash[..hash.len().min(16)]),
                outcome: "ok",
                client: "",
                detail: &format!(
                    "an unspent console {} code issued by {} at {}; issue a new invite",
                    text(s, 1),
                    text(s, 2),
                    num(s, 3).unwrap_or(0)
                ),
            },
            now,
        )
        .await?;
        out.setup_codes_voided += 1;
    }
    let detail = format!(
        "source={} rp_id={rp_id} people={} live_admins={} passkeys={} recovery_codes={} \
         setup_codes_voided={}",
        source(handle),
        out.people,
        out.live_admins,
        out.passkeys,
        out.recovery_codes,
        out.setup_codes_voided
    );
    audit::append(
        tx,
        handle,
        keys,
        &Entry {
            actor: "boot",
            action: "import.console",
            subject: &source(handle),
            outcome: "ok",
            client: "",
            detail: &detail,
        },
        now,
    )
    .await?;
    tx.rows(&format!(
        "INSERT INTO imports (deployment, source, rp_id, people, passkeys, codes, created_at) \
         VALUES ({d}, {}, {}, {}, {}, {}, {now})",
        sql_lit(&source(handle)),
        sql_lit(rp_id),
        out.people,
        out.passkeys,
        out.recovery_codes,
    ))
    .await?;
    Ok(out)
}
