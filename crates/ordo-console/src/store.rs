//! The console's own database (`plan-console.md` 5.2): its tables, its
//! migrations, and every statement the console runs.
//!
//! Reached through the one database client (`ordo_exec::Pool`), opened with
//! `Pool::connect_own` so it gets none of a deployment's tables. Every
//! failure is the client's typed `PoolError`, carried up inside
//! [`ConsoleError::Store`].
//!
//! Every row is keyed by a deployment handle (`plan-console.md` 11). Open
//! source holds one deployment; the private operator later holds many and
//! nothing here changes.

use ordo_exec::{sql_lit, Pool, PoolLimits, TextRow, TxConn};
use ordo_identity::passkey::StoredPasskey;
use ordo_identity::webauthn::b64url_encode;

use crate::config::{DatabaseSettings, DATABASE_URL_ENV};
use crate::error::ConsoleError;

/// The console's migrations, in order. A migration is never edited once
/// shipped; a change is a new one.
const MIGRATIONS: &[(i32, &str)] = &[(1, MIGRATION_1), (2, MIGRATION_2), (3, MIGRATION_3)];

const MIGRATION_1: &str = r#"
CREATE TABLE deployments (
  handle      text   PRIMARY KEY,
  door_url    text,
  mode        text   NOT NULL CHECK (mode IN ('supervise', 'door-only')),
  created_at  bigint NOT NULL
);
CREATE TABLE owner (
  id          text   PRIMARY KEY,
  deployment  text   NOT NULL REFERENCES deployments (handle),
  label       text   NOT NULL,
  created_at  bigint NOT NULL,
  removed_at  bigint
);
CREATE TABLE passkeys (
  deployment      text    NOT NULL REFERENCES deployments (handle),
  credential_id   text    NOT NULL,
  owner_id        text    NOT NULL REFERENCES owner (id),
  algorithm       bigint  NOT NULL,
  public_key      text    NOT NULL,
  sign_count      bigint  NOT NULL,
  user_handle     text    NOT NULL,
  backup_eligible boolean NOT NULL,
  backed_up       boolean NOT NULL,
  registered_at   bigint  NOT NULL,
  last_used_at    bigint  NOT NULL DEFAULT 0,
  PRIMARY KEY (deployment, credential_id)
);
CREATE TABLE sessions (
  id_hash      text    PRIMARY KEY,
  deployment   text    NOT NULL REFERENCES deployments (handle),
  owner_id     text    NOT NULL REFERENCES owner (id),
  created_at   bigint  NOT NULL,
  last_seen_at bigint  NOT NULL,
  touched_at   bigint,
  must_enrol   boolean NOT NULL DEFAULT false,
  ended_at     bigint,
  ended_why    text
);
CREATE INDEX sessions_by_owner ON sessions (deployment, owner_id) WHERE ended_at IS NULL;
CREATE TABLE recovery_codes (
  id          bigserial PRIMARY KEY,
  deployment  text   NOT NULL REFERENCES deployments (handle),
  owner_id    text   NOT NULL REFERENCES owner (id),
  lookup      text   NOT NULL,
  hash        text   NOT NULL,
  issued_at   bigint NOT NULL,
  used_at     bigint,
  voided_at   bigint
);
CREATE UNIQUE INDEX recovery_codes_live ON recovery_codes (deployment, lookup)
  WHERE used_at IS NULL AND voided_at IS NULL;
CREATE TABLE setup_codes (
  code_hash   text   PRIMARY KEY,
  deployment  text   NOT NULL REFERENCES deployments (handle),
  kind        text   NOT NULL CHECK (kind IN ('first-run', 'invite', 'recover')),
  owner_id    text   REFERENCES owner (id),
  issued_by   text   NOT NULL,
  issued_at   bigint NOT NULL,
  expires_at  bigint,
  used_at     bigint,
  voided_at   bigint
);
CREATE TABLE revisions (
  deployment  text   NOT NULL REFERENCES deployments (handle),
  number      bigint NOT NULL,
  settings    jsonb  NOT NULL DEFAULT '{}',
  secret_refs jsonb  NOT NULL DEFAULT '[]',
  actor       text   NOT NULL,
  reason      text   NOT NULL DEFAULT '',
  outcome     text,
  created_at  bigint NOT NULL,
  PRIMARY KEY (deployment, number)
);
CREATE TABLE secrets (
  deployment  text   NOT NULL REFERENCES deployments (handle),
  name        text   NOT NULL,
  sealed      text   NOT NULL,
  created_at  bigint NOT NULL,
  PRIMARY KEY (deployment, name)
);
CREATE TABLE credentials (
  deployment  text   PRIMARY KEY REFERENCES deployments (handle),
  sealed      text   NOT NULL,
  created_at  bigint NOT NULL,
  rotated_at  bigint
);
CREATE TABLE audit (
  id          bigserial PRIMARY KEY,
  deployment  text   NOT NULL,
  at          bigint NOT NULL,
  actor       text   NOT NULL,
  action      text   NOT NULL,
  outcome     text   NOT NULL,
  detail      text   NOT NULL DEFAULT ''
);
CREATE INDEX audit_by_time ON audit (deployment, at);
"#;

/// Step 2, the supervisor (`plan-console.md` 6): which revision is live,
/// when a revision's outcome was decided and why, and managed secrets kept
/// by version so a rollback restores the value its revision named.
const MIGRATION_2: &str = r#"
ALTER TABLE deployments ADD COLUMN live_revision bigint;
ALTER TABLE revisions ADD COLUMN detail text NOT NULL DEFAULT '';
ALTER TABLE revisions ADD COLUMN decided_at bigint;
ALTER TABLE secrets ADD COLUMN version bigint NOT NULL DEFAULT 1;
ALTER TABLE secrets DROP CONSTRAINT secrets_pkey;
ALTER TABLE secrets ADD PRIMARY KEY (deployment, name, version);
"#;

/// Step 3, the link to the deployment (`plan-console.md` 8.2, G14): the
/// console key's id at the deployment, its name, and the deployment's own
/// space. `sealed` holds the private half, sealed with the console's key.
const MIGRATION_3: &str = r#"
ALTER TABLE credentials ADD COLUMN key_id text NOT NULL DEFAULT '';
ALTER TABLE credentials ADD COLUMN name text NOT NULL DEFAULT '';
ALTER TABLE credentials ADD COLUMN space text NOT NULL DEFAULT '';
"#;

/// The console key for one deployment, as stored.
#[derive(Debug, Clone)]
pub struct CredentialRow {
    pub key_id: String,
    pub name: String,
    pub space: String,
    /// The private half, sealed (hex). Opened only by `crate::link`.
    pub sealed: String,
    pub created_at: u64,
    pub rotated_at: Option<u64>,
}

/// One runtime revision as stored.
#[derive(Debug, Clone)]
pub struct RevisionRow {
    pub number: u64,
    /// `{"NAME": "value"}`.
    pub settings: String,
    /// `[{"name", "kind", ...}]`, never a value.
    pub secret_refs: String,
    pub actor: String,
    pub reason: String,
    /// `None` while it is being applied.
    pub outcome: Option<String>,
    pub detail: String,
    pub created_at: u64,
    pub decided_at: Option<u64>,
}

/// What a setup code is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    /// Printed at first run: the first person to use it becomes owner.
    FirstRun,
    /// A second owner's invitation.
    Invite,
    /// `ordo console recover`: enrols a new passkey and ends every session.
    Recover,
}

impl CodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CodeKind::FirstRun => "first-run",
            CodeKind::Invite => "invite",
            CodeKind::Recover => "recover",
        }
    }

    fn parse(s: &str) -> Option<CodeKind> {
        match s {
            "first-run" => Some(CodeKind::FirstRun),
            "invite" => Some(CodeKind::Invite),
            "recover" => Some(CodeKind::Recover),
            _ => None,
        }
    }
}

/// A live setup code.
#[derive(Debug, Clone)]
pub struct SetupCode {
    pub kind: CodeKind,
    /// The owner a recover code enrols for, when it names one.
    pub owner_id: Option<String>,
}

/// An active console owner.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Owner {
    pub id: String,
    pub label: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
}

/// A session row.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub deployment: String,
    pub owner_id: String,
    pub created_at: u64,
    pub last_seen_at: u64,
    pub touched_at: Option<u64>,
    pub must_enrol: bool,
    pub ended: bool,
    pub owner_active: bool,
}

/// One recovery code as stored: the lookup part, and the argon2id hash of
/// the rest.
#[derive(Debug, Clone)]
pub struct RecoveryRow {
    pub lookup: String,
    pub hash: String,
}

/// What a new session row holds.
pub struct NewSession<'a> {
    pub hash: &'a str,
    pub owner_id: &'a str,
    pub touched: bool,
    pub must_enrol: bool,
}

/// What enrolling with a setup code did.
#[derive(Debug, Clone)]
pub struct Enrolled {
    pub owner_id: String,
    pub kind: CodeKind,
    /// Sessions a recover code ended.
    pub sessions_ended: u64,
}

/// The console's database.
#[derive(Clone)]
pub struct Store {
    pool: Pool,
}

fn cell(row: &TextRow, i: usize) -> Option<&str> {
    row.get(i).and_then(|c| c.as_deref())
}

fn num(row: &TextRow, i: usize) -> Option<u64> {
    cell(row, i)
        .and_then(|v| v.parse::<i64>().ok())
        .map(|n| n.max(0) as u64)
}

fn flag(row: &TextRow, i: usize) -> bool {
    cell(row, i) == Some("t")
}

fn opt_lit(v: Option<&str>) -> String {
    v.map(sql_lit).unwrap_or_else(|| "NULL".into())
}

fn opt_num(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".into())
}

/// The statement that stores one passkey for an owner.
fn insert_passkey(dep: &str, owner: &str, p: &StoredPasskey) -> String {
    format!(
        "INSERT INTO passkeys (deployment, credential_id, owner_id, algorithm, public_key, \
         sign_count, user_handle, backup_eligible, backed_up, registered_at, last_used_at) \
         VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, 0)",
        sql_lit(dep),
        sql_lit(&b64url_encode(p.credential.id())),
        sql_lit(owner),
        p.credential.algorithm(),
        sql_lit(&b64url_encode(&p.credential.key().to_bytes())),
        p.credential.sign_count(),
        sql_lit(&b64url_encode(&p.user_handle)),
        p.backup_eligible,
        p.backed_up,
        p.registered_at.0,
    )
}

/// Void an owner's live recovery codes and store a new set.
/// Retire every passkey an owner holds but `kept`, the one just enrolled
/// in this transaction. Never leaves the owner with none: if `kept` is not
/// there, nothing is retired.
async fn retire_other_passkeys(
    tx: &TxConn,
    dep: &str,
    owner: &str,
    kept: &StoredPasskey,
) -> Result<(), ConsoleError> {
    let (d, o) = (sql_lit(dep), sql_lit(owner));
    let k = sql_lit(&b64url_encode(kept.credential.id()));
    tx.rows(&format!(
        "DELETE FROM passkeys WHERE deployment = {d} AND owner_id = {o} \
         AND credential_id <> {k} AND EXISTS (SELECT 1 FROM passkeys \
         WHERE deployment = {d} AND owner_id = {o} AND credential_id = {k})"
    ))
    .await?;
    Ok(())
}

async fn replace_recovery(
    tx: &TxConn,
    dep: &str,
    owner: &str,
    rows: &[RecoveryRow],
    now: u64,
) -> Result<(), ConsoleError> {
    tx.rows(&format!(
        "UPDATE recovery_codes SET voided_at = {now} WHERE deployment = {} AND owner_id = {} \
         AND used_at IS NULL AND voided_at IS NULL",
        sql_lit(dep),
        sql_lit(owner)
    ))
    .await?;
    for r in rows {
        tx.rows(&format!(
            "INSERT INTO recovery_codes (deployment, owner_id, lookup, hash, issued_at) \
             VALUES ({}, {}, {}, {}, {now})",
            sql_lit(dep),
            sql_lit(owner),
            sql_lit(&r.lookup),
            sql_lit(&r.hash)
        ))
        .await?;
    }
    Ok(())
}

async fn insert_session(
    tx: &TxConn,
    dep: &str,
    s: &NewSession<'_>,
    now: u64,
) -> Result<(), ConsoleError> {
    tx.rows(&format!(
        "INSERT INTO sessions (id_hash, deployment, owner_id, created_at, last_seen_at, \
         touched_at, must_enrol) VALUES ({}, {}, {}, {now}, {now}, {}, {})",
        sql_lit(s.hash),
        sql_lit(dep),
        sql_lit(s.owner_id),
        if s.touched {
            now.to_string()
        } else {
            "NULL".into()
        },
        s.must_enrol
    ))
    .await?;
    Ok(())
}

async fn active_owner_ids(tx: &TxConn, dep: &str) -> Result<Vec<String>, ConsoleError> {
    Ok(tx
        .rows(&format!(
            "SELECT id FROM owner WHERE deployment = {} AND removed_at IS NULL \
             ORDER BY created_at, id FOR UPDATE",
            sql_lit(dep)
        ))
        .await?
        .iter()
        .filter_map(|r| cell(r, 0).map(str::to_string))
        .collect())
}

impl Store {
    /// Open the console's database. A database that cannot be reached is a
    /// boot refusal naming the setting, with the client's error kept.
    pub async fn open(db: &DatabaseSettings) -> Result<Store, ConsoleError> {
        let pool = Pool::connect_own(db.pool.clone(), PoolLimits::default())
            .await
            .map_err(|e| {
                ConsoleError::setting_caused(
                    DATABASE_URL_ENV,
                    format!(
                        "names database `{}` on {}:{}, and it could not be opened",
                        db.pool.db, db.pool.host, db.pool.port
                    ),
                    format!(
                        "Check that Postgres is running and listening on {}:{}, then the host, \
                         port, role and password in {DATABASE_URL_ENV}, and that the database \
                         exists (CREATE DATABASE {})",
                        db.pool.host, db.pool.port, db.pool.db
                    ),
                    e,
                )
            })?;
        Ok(Store { pool })
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Run every migration not yet applied, each in its own transaction,
    /// under an advisory lock so two consoles booting together do not race.
    pub async fn migrate(&self, now: u64) -> Result<(), ConsoleError> {
        self.pool
            .rows(
                "CREATE TABLE IF NOT EXISTS console_migrations (\
                 version integer PRIMARY KEY, applied_at bigint NOT NULL)",
            )
            .await?;
        for (version, sql) in MIGRATIONS {
            let tx = self.pool.begin().await?;
            tx.rows("SELECT pg_advisory_xact_lock(726201801)").await?;
            let done = tx
                .rows(&format!(
                    "SELECT 1 FROM console_migrations WHERE version = {version}"
                ))
                .await?;
            if done.is_empty() {
                tx.rows(sql).await?;
                tx.rows(&format!(
                    "INSERT INTO console_migrations (version, applied_at) VALUES ({version}, {now})"
                ))
                .await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }

    /// Make sure the deployment's row is there, with the mode the console
    /// booted in.
    pub async fn ensure_deployment(
        &self,
        handle: &str,
        door_url: Option<&str>,
        mode: &str,
        now: u64,
    ) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "INSERT INTO deployments (handle, door_url, mode, created_at) \
                 VALUES ({}, {}, {}, {now}) \
                 ON CONFLICT (handle) DO UPDATE SET door_url = EXCLUDED.door_url, \
                 mode = EXCLUDED.mode",
                sql_lit(handle),
                opt_lit(door_url),
                sql_lit(mode)
            ))
            .await?;
        Ok(())
    }

    /// File a new runtime revision, numbered one past the last, with no
    /// outcome yet. The deployment's row is locked so two applies never
    /// take one number.
    pub async fn insert_revision(
        &self,
        dep: &str,
        settings: &str,
        secret_refs: &str,
        actor: &str,
        reason: &str,
        now: u64,
    ) -> Result<u64, ConsoleError> {
        let tx = self.pool.begin().await?;
        tx.rows(&format!(
            "SELECT 1 FROM deployments WHERE handle = {} FOR UPDATE",
            sql_lit(dep)
        ))
        .await?;
        let next = tx
            .rows(&format!(
                "SELECT COALESCE(MAX(number), 0) + 1 FROM revisions WHERE deployment = {}",
                sql_lit(dep)
            ))
            .await?
            .first()
            .and_then(|r| num(r, 0))
            .unwrap_or(1);
        tx.rows(&format!(
            "INSERT INTO revisions (deployment, number, settings, secret_refs, actor, reason, \
             created_at) VALUES ({}, {next}, {}::jsonb, {}::jsonb, {}, {}, {now})",
            sql_lit(dep),
            sql_lit(settings),
            sql_lit(secret_refs),
            sql_lit(actor),
            sql_lit(reason)
        ))
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    /// File a revision under the number the supervisor gave it, as `good`:
    /// the supervisor's live revision, when the console's history has no
    /// row of that number (revision 1, built by the supervisor from the
    /// container's variables). A row already there is left as it is.
    #[allow(clippy::too_many_arguments)]
    pub async fn mirror_revision(
        &self,
        dep: &str,
        number: u64,
        settings: &str,
        secret_refs: &str,
        actor: &str,
        reason: &str,
        now: u64,
    ) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "INSERT INTO revisions (deployment, number, settings, secret_refs, actor, \
                 reason, outcome, detail, created_at, decided_at) VALUES ({}, {number}, \
                 {}::jsonb, {}::jsonb, {}, {}, 'good', 'filed from the supervisor', {now}, \
                 {now}) ON CONFLICT DO NOTHING",
                sql_lit(dep),
                sql_lit(settings),
                sql_lit(secret_refs),
                sql_lit(actor),
                sql_lit(reason)
            ))
            .await?;
        Ok(())
    }

    /// Rewrite a revision's settings and secret references, keeping its
    /// number, author and outcome. Used once per row, at boot, to seal a
    /// secret a revision filed before step 2b held in plain.
    pub async fn reseal_revision(
        &self,
        dep: &str,
        number: u64,
        settings: &str,
        secret_refs: &str,
    ) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "UPDATE revisions SET settings = {}::jsonb, secret_refs = {}::jsonb \
                 WHERE deployment = {} AND number = {number}",
                sql_lit(settings),
                sql_lit(secret_refs),
                sql_lit(dep)
            ))
            .await?;
        Ok(())
    }

    /// Record how a revision ended: `good`, `refused`, `failed`, `crashed`.
    pub async fn decide_revision(
        &self,
        dep: &str,
        number: u64,
        outcome: &str,
        detail: &str,
        now: u64,
    ) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "UPDATE revisions SET outcome = {}, detail = {}, decided_at = {now} \
                 WHERE deployment = {} AND number = {number}",
                sql_lit(outcome),
                sql_lit(detail),
                sql_lit(dep)
            ))
            .await?;
        Ok(())
    }

    const REVISION_COLUMNS: &'static str = "number, settings::text, secret_refs::text, actor, \
         reason, outcome, detail, created_at, decided_at";

    fn revision_row(r: &TextRow) -> RevisionRow {
        RevisionRow {
            number: num(r, 0).unwrap_or(0),
            settings: cell(r, 1).unwrap_or("{}").to_string(),
            secret_refs: cell(r, 2).unwrap_or("[]").to_string(),
            actor: cell(r, 3).unwrap_or_default().to_string(),
            reason: cell(r, 4).unwrap_or_default().to_string(),
            outcome: cell(r, 5).map(str::to_string),
            detail: cell(r, 6).unwrap_or_default().to_string(),
            created_at: num(r, 7).unwrap_or(0),
            decided_at: num(r, 8),
        }
    }

    /// One revision.
    pub async fn revision(
        &self,
        dep: &str,
        number: u64,
    ) -> Result<Option<RevisionRow>, ConsoleError> {
        Ok(self
            .pool
            .rows(&format!(
                "SELECT {} FROM revisions WHERE deployment = {} AND number = {number}",
                Self::REVISION_COLUMNS,
                sql_lit(dep)
            ))
            .await?
            .first()
            .map(Self::revision_row))
    }

    /// The newest revisions first.
    pub async fn revisions(&self, dep: &str, limit: u64) -> Result<Vec<RevisionRow>, ConsoleError> {
        Ok(self
            .pool
            .rows(&format!(
                "SELECT {} FROM revisions WHERE deployment = {} ORDER BY number DESC LIMIT {limit}",
                Self::REVISION_COLUMNS,
                sql_lit(dep)
            ))
            .await?
            .iter()
            .map(Self::revision_row)
            .collect())
    }

    /// The revision the supervisor runs.
    pub async fn live_revision(&self, dep: &str) -> Result<Option<u64>, ConsoleError> {
        Ok(self
            .pool
            .rows(&format!(
                "SELECT live_revision FROM deployments WHERE handle = {}",
                sql_lit(dep)
            ))
            .await?
            .first()
            .and_then(|r| num(r, 0)))
    }

    pub async fn set_live_revision(&self, dep: &str, number: u64) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "UPDATE deployments SET live_revision = {number} WHERE handle = {}",
                sql_lit(dep)
            ))
            .await?;
        Ok(())
    }

    /// Store a new version of a managed secret, already sealed (hex), and
    /// return its version.
    pub async fn put_secret(
        &self,
        dep: &str,
        name: &str,
        sealed_hex: &str,
        now: u64,
    ) -> Result<u64, ConsoleError> {
        let rows = self
            .pool
            .rows(&format!(
                "INSERT INTO secrets (deployment, name, version, sealed, created_at) \
                 SELECT {d}, {n}, COALESCE(MAX(version), 0) + 1, {s}, {now} FROM secrets \
                 WHERE deployment = {d} AND name = {n} RETURNING version",
                d = sql_lit(dep),
                n = sql_lit(name),
                s = sql_lit(sealed_hex)
            ))
            .await?;
        Ok(rows.first().and_then(|r| num(r, 0)).unwrap_or(1))
    }

    /// One version of a managed secret, sealed (hex).
    pub async fn secret(
        &self,
        dep: &str,
        name: &str,
        version: u64,
    ) -> Result<Option<String>, ConsoleError> {
        Ok(self
            .pool
            .rows(&format!(
                "SELECT sealed FROM secrets WHERE deployment = {} AND name = {} \
                 AND version = {version}",
                sql_lit(dep),
                sql_lit(name)
            ))
            .await?
            .first()
            .and_then(|r| cell(r, 0).map(str::to_string)))
    }

    pub async fn deployment_exists(&self, handle: &str) -> Result<bool, ConsoleError> {
        Ok(!self
            .pool
            .rows(&format!(
                "SELECT 1 FROM deployments WHERE handle = {}",
                sql_lit(handle)
            ))
            .await?
            .is_empty())
    }

    /// The deployment's active owners, oldest first.
    pub async fn owners(&self, dep: &str) -> Result<Vec<Owner>, ConsoleError> {
        Ok(self
            .pool
            .rows(&format!(
                "SELECT id, label, created_at FROM owner WHERE deployment = {} \
                 AND removed_at IS NULL ORDER BY created_at, id",
                sql_lit(dep)
            ))
            .await?
            .iter()
            .map(|r| Owner {
                id: cell(r, 0).unwrap_or_default().to_string(),
                label: cell(r, 1).unwrap_or_default().to_string(),
                created_at: num(r, 2).unwrap_or(0),
            })
            .collect())
    }

    /// Void every live code of a kind, then store a new one.
    #[allow(clippy::too_many_arguments)]
    pub async fn issue_setup_code(
        &self,
        dep: &str,
        kind: CodeKind,
        hash: &str,
        owner_id: Option<&str>,
        expires_at: Option<u64>,
        issued_by: &str,
        now: u64,
    ) -> Result<(), ConsoleError> {
        let tx = self.pool.begin().await?;
        tx.rows(&format!(
            "UPDATE setup_codes SET voided_at = {now} WHERE deployment = {} AND kind = {} \
             AND used_at IS NULL AND voided_at IS NULL",
            sql_lit(dep),
            sql_lit(kind.as_str())
        ))
        .await?;
        tx.rows(&format!(
            "INSERT INTO setup_codes (code_hash, deployment, kind, owner_id, issued_by, \
             issued_at, expires_at) VALUES ({}, {}, {}, {}, {}, {now}, {})",
            sql_lit(hash),
            sql_lit(dep),
            sql_lit(kind.as_str()),
            opt_lit(owner_id),
            sql_lit(issued_by),
            opt_num(expires_at)
        ))
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A setup code that is live: not used, voided or expired.
    pub async fn live_setup_code(
        &self,
        dep: &str,
        hash: &str,
        now: u64,
    ) -> Result<Option<SetupCode>, ConsoleError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT kind, owner_id FROM setup_codes WHERE deployment = {} AND code_hash = {} \
                 AND used_at IS NULL AND voided_at IS NULL \
                 AND (expires_at IS NULL OR expires_at > {now})",
                sql_lit(dep),
                sql_lit(hash)
            ))
            .await?;
        Ok(rows.first().and_then(|r| {
            Some(SetupCode {
                kind: CodeKind::parse(cell(r, 0)?)?,
                owner_id: cell(r, 1).map(str::to_string),
            })
        }))
    }

    /// Enrol a passkey with a setup code, in one transaction: use the code,
    /// make or pick the owner, store the passkey, store a new recovery set,
    /// end every session when the code is a recover code, and open the
    /// enroller's session.
    #[allow(clippy::too_many_arguments)]
    pub async fn enrol_with_code(
        &self,
        dep: &str,
        code_hash: &str,
        label: &str,
        passkey: &StoredPasskey,
        recovery: &[RecoveryRow],
        session_hash: &str,
        max_owners: usize,
        now: u64,
    ) -> Result<Enrolled, ConsoleError> {
        let tx = self.pool.begin().await?;
        let used = tx
            .rows(&format!(
                "UPDATE setup_codes SET used_at = {now} WHERE deployment = {} AND code_hash = {} \
                 AND used_at IS NULL AND voided_at IS NULL \
                 AND (expires_at IS NULL OR expires_at > {now}) RETURNING kind, owner_id",
                sql_lit(dep),
                sql_lit(code_hash)
            ))
            .await?;
        let Some(row) = used.first() else {
            return Err(ConsoleError::CodeRefused);
        };
        let kind = cell(row, 0)
            .and_then(CodeKind::parse)
            .ok_or(ConsoleError::CodeRefused)?;
        let named = cell(row, 1).map(str::to_string);
        let owners = active_owner_ids(&tx, dep).await?;
        let new_owner = || crate::codes::mint_owner_id();
        let owner_id = match kind {
            CodeKind::FirstRun if owners.is_empty() => new_owner(),
            CodeKind::FirstRun => return Err(ConsoleError::CodeRefused),
            CodeKind::Invite if owners.len() >= max_owners => {
                return Err(ConsoleError::OwnerLimit { max: max_owners })
            }
            CodeKind::Invite => new_owner(),
            CodeKind::Recover => match (named, owners.as_slice()) {
                (Some(id), list) if list.contains(&id) => id,
                (Some(_), _) => return Err(ConsoleError::UnknownOwner),
                (None, []) => new_owner(),
                (None, [only]) => only.clone(),
                (None, list) => {
                    return Err(ConsoleError::OwnerNotNamed {
                        owners: list.to_vec(),
                    })
                }
            },
        };
        if !owners.contains(&owner_id) {
            tx.rows(&format!(
                "INSERT INTO owner (id, deployment, label, created_at) VALUES ({}, {}, {}, {now})",
                sql_lit(&owner_id),
                sql_lit(dep),
                sql_lit(label)
            ))
            .await?;
        }
        tx.rows(&insert_passkey(dep, &owner_id, passkey)).await?;
        if kind == CodeKind::Recover {
            // The failsafe for lost passkeys: once the new one is enrolled,
            // in this transaction, the ones the owner held before are
            // retired, as after a recovery code.
            retire_other_passkeys(&tx, dep, &owner_id, passkey).await?;
        }
        replace_recovery(&tx, dep, &owner_id, recovery, now).await?;
        let mut sessions_ended = 0;
        if kind == CodeKind::Recover {
            sessions_ended = tx
                .rows(&format!(
                    "UPDATE sessions SET ended_at = {now}, ended_why = 'recover' \
                     WHERE deployment = {} AND ended_at IS NULL RETURNING 1",
                    sql_lit(dep)
                ))
                .await?
                .len() as u64;
        }
        insert_session(
            &tx,
            dep,
            &NewSession {
                hash: session_hash,
                owner_id: &owner_id,
                touched: true,
                must_enrol: false,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(Enrolled {
            owner_id,
            kind,
            sessions_ended,
        })
    }

    /// Add a passkey to a signed-in owner. When `recovery` is given (the
    /// session came from a recovery code), the owner's set is replaced and
    /// the session is cleared to do anything, with a fresh touch.
    pub async fn add_passkey(
        &self,
        dep: &str,
        owner: &str,
        passkey: &StoredPasskey,
        recovery: Option<&[RecoveryRow]>,
        session_hash: &str,
        now: u64,
    ) -> Result<(), ConsoleError> {
        let tx = self.pool.begin().await?;
        tx.rows(&insert_passkey(dep, owner, passkey)).await?;
        if let Some(rows) = recovery {
            // After a recovery code: the new passkey is enrolled and
            // verified, so the old ones are retired now, in the same
            // transaction, and the sessions they opened end. Not before: a
            // recovery whose enrolment never completes retires nothing.
            retire_other_passkeys(&tx, dep, owner, passkey).await?;
            tx.rows(&format!(
                "UPDATE sessions SET ended_at = {now}, ended_why = 'recovery code' \
                 WHERE deployment = {} AND owner_id = {} AND id_hash <> {} AND ended_at IS NULL",
                sql_lit(dep),
                sql_lit(owner),
                sql_lit(session_hash)
            ))
            .await?;
            replace_recovery(&tx, dep, owner, rows, now).await?;
            tx.rows(&format!(
                "UPDATE sessions SET must_enrol = false, touched_at = {now} WHERE id_hash = {}",
                sql_lit(session_hash)
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Replace an owner's recovery codes.
    pub async fn renew_recovery(
        &self,
        dep: &str,
        owner: &str,
        rows: &[RecoveryRow],
        now: u64,
    ) -> Result<(), ConsoleError> {
        let tx = self.pool.begin().await?;
        replace_recovery(&tx, dep, owner, rows, now).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The credential ids (base64url) an owner holds.
    pub async fn owner_credentials(
        &self,
        dep: &str,
        owner: &str,
    ) -> Result<Vec<String>, ConsoleError> {
        Ok(self
            .pool
            .rows(&format!(
                "SELECT credential_id FROM passkeys WHERE deployment = {} AND owner_id = {} \
                 ORDER BY registered_at",
                sql_lit(dep),
                sql_lit(owner)
            ))
            .await?
            .iter()
            .filter_map(|r| cell(r, 0).map(str::to_string))
            .collect())
    }

    /// The active owner a credential belongs to.
    pub async fn passkey_owner(
        &self,
        dep: &str,
        credential_b64: &str,
    ) -> Result<Option<String>, ConsoleError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT p.owner_id FROM passkeys p JOIN owner o ON o.id = p.owner_id \
                 WHERE p.deployment = {} AND p.credential_id = {} AND o.removed_at IS NULL",
                sql_lit(dep),
                sql_lit(credential_b64)
            ))
            .await?;
        Ok(rows.first().and_then(|r| cell(r, 0)).map(str::to_string))
    }

    /// Open a session.
    pub async fn create_session(
        &self,
        dep: &str,
        s: &NewSession<'_>,
        now: u64,
    ) -> Result<(), ConsoleError> {
        let tx = self.pool.begin().await?;
        insert_session(&tx, dep, s, now).await?;
        tx.commit().await?;
        Ok(())
    }

    /// A session by its token's hash.
    pub async fn session(&self, hash: &str) -> Result<Option<SessionRow>, ConsoleError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT s.deployment, s.owner_id, s.created_at, s.last_seen_at, s.touched_at, \
                 s.must_enrol, s.ended_at IS NOT NULL, o.removed_at IS NULL \
                 FROM sessions s JOIN owner o ON o.id = s.owner_id WHERE s.id_hash = {}",
                sql_lit(hash)
            ))
            .await?;
        Ok(rows.first().map(|r| SessionRow {
            deployment: cell(r, 0).unwrap_or_default().to_string(),
            owner_id: cell(r, 1).unwrap_or_default().to_string(),
            created_at: num(r, 2).unwrap_or(0),
            last_seen_at: num(r, 3).unwrap_or(0),
            touched_at: num(r, 4),
            must_enrol: flag(r, 5),
            ended: flag(r, 6),
            owner_active: flag(r, 7),
        }))
    }

    /// Note that a session was used now.
    pub async fn seen(&self, hash: &str, now: u64) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "UPDATE sessions SET last_seen_at = {now} WHERE id_hash = {}",
                sql_lit(hash)
            ))
            .await?;
        Ok(())
    }

    /// Note a fresh passkey touch on a session.
    pub async fn touched(&self, hash: &str, now: u64) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "UPDATE sessions SET touched_at = {now}, last_seen_at = {now} WHERE id_hash = {}",
                sql_lit(hash)
            ))
            .await?;
        Ok(())
    }

    /// End one session.
    pub async fn end_session(&self, hash: &str, why: &str, now: u64) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "UPDATE sessions SET ended_at = {now}, ended_why = {} \
                 WHERE id_hash = {} AND ended_at IS NULL",
                sql_lit(why),
                sql_lit(hash)
            ))
            .await?;
        Ok(())
    }

    /// The live recovery code a lookup names: its owner and its hash.
    pub async fn live_recovery(
        &self,
        dep: &str,
        lookup: &str,
    ) -> Result<Option<(String, String)>, ConsoleError> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT r.owner_id, r.hash FROM recovery_codes r JOIN owner o ON o.id = r.owner_id \
                 WHERE r.deployment = {} AND r.lookup = {} AND r.used_at IS NULL \
                 AND r.voided_at IS NULL AND o.removed_at IS NULL",
                sql_lit(dep),
                sql_lit(lookup)
            ))
            .await?;
        Ok(rows
            .first()
            .and_then(|r| Some((cell(r, 0)?.to_string(), cell(r, 1)?.to_string()))))
    }

    /// Use a recovery code and open a session that may only enrol a new
    /// passkey, in one transaction. `false` when another request used it
    /// first.
    pub async fn use_recovery(
        &self,
        dep: &str,
        lookup: &str,
        owner: &str,
        session_hash: &str,
        now: u64,
    ) -> Result<bool, ConsoleError> {
        let tx = self.pool.begin().await?;
        let used = tx
            .rows(&format!(
                "UPDATE recovery_codes SET used_at = {now} WHERE deployment = {} AND lookup = {} \
                 AND owner_id = {} AND used_at IS NULL AND voided_at IS NULL RETURNING 1",
                sql_lit(dep),
                sql_lit(lookup),
                sql_lit(owner)
            ))
            .await?;
        if used.is_empty() {
            return Ok(false);
        }
        insert_session(
            &tx,
            dep,
            &NewSession {
                hash: session_hash,
                owner_id: owner,
                touched: false,
                must_enrol: true,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Remove an owner: mark it removed, delete its passkeys, void its codes
    /// and end its sessions. Refuses the last owner.
    pub async fn remove_owner(&self, dep: &str, owner: &str, now: u64) -> Result<(), ConsoleError> {
        let tx = self.pool.begin().await?;
        let owners = active_owner_ids(&tx, dep).await?;
        if !owners.iter().any(|o| o == owner) {
            return Err(ConsoleError::UnknownOwner);
        }
        if owners.len() <= 1 {
            return Err(ConsoleError::LastOwner);
        }
        let (d, o) = (sql_lit(dep), sql_lit(owner));
        tx.rows(&format!(
            "UPDATE owner SET removed_at = {now} WHERE deployment = {d} AND id = {o}"
        ))
        .await?;
        tx.rows(&format!(
            "DELETE FROM passkeys WHERE deployment = {d} AND owner_id = {o}"
        ))
        .await?;
        tx.rows(&format!(
            "UPDATE recovery_codes SET voided_at = {now} WHERE deployment = {d} AND owner_id = {o} \
             AND used_at IS NULL AND voided_at IS NULL"
        ))
        .await?;
        tx.rows(&format!(
            "UPDATE sessions SET ended_at = {now}, ended_why = 'owner removed' \
             WHERE deployment = {d} AND owner_id = {o} AND ended_at IS NULL"
        ))
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Store the console key for a deployment, replacing the one before
    /// (a rotation).
    pub async fn put_credential(
        &self,
        dep: &str,
        row: &CredentialRow,
        now: u64,
    ) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "INSERT INTO credentials (deployment, sealed, created_at, key_id, name, space) \
                 VALUES ({d}, {s}, {now}, {k}, {n}, {sp}) \
                 ON CONFLICT (deployment) DO UPDATE SET sealed = EXCLUDED.sealed, \
                 key_id = EXCLUDED.key_id, name = EXCLUDED.name, space = EXCLUDED.space, \
                 rotated_at = {now}",
                d = sql_lit(dep),
                s = sql_lit(&row.sealed),
                k = sql_lit(&row.key_id),
                n = sql_lit(&row.name),
                sp = sql_lit(&row.space),
            ))
            .await?;
        Ok(())
    }

    /// The console key for a deployment, if it is linked.
    pub async fn credential(&self, dep: &str) -> Result<Option<CredentialRow>, ConsoleError> {
        Ok(self
            .pool
            .rows(&format!(
                "SELECT key_id, name, space, sealed, created_at, rotated_at FROM credentials \
                 WHERE deployment = {}",
                sql_lit(dep)
            ))
            .await?
            .first()
            .map(|r| CredentialRow {
                key_id: cell(r, 0).unwrap_or_default().to_string(),
                name: cell(r, 1).unwrap_or_default().to_string(),
                space: cell(r, 2).unwrap_or_default().to_string(),
                sealed: cell(r, 3).unwrap_or_default().to_string(),
                created_at: num(r, 4).unwrap_or(0),
                rotated_at: num(r, 5),
            }))
    }

    /// Forget the console key for a deployment (unlinked).
    pub async fn delete_credential(&self, dep: &str) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "DELETE FROM credentials WHERE deployment = {}",
                sql_lit(dep)
            ))
            .await?;
        Ok(())
    }

    /// Forget the console key and write its audit row, in one transaction,
    /// so a crash between the two never leaves a forgotten key unaudited.
    /// `None` when there was no key; the row then says `nothing`.
    pub async fn forget_credential_audited(
        &self,
        dep: &str,
        actor: &str,
        action: &str,
        now: u64,
    ) -> Result<Option<String>, ConsoleError> {
        let tx = self.pool.begin().await?;
        let gone = tx
            .rows(&format!(
                "DELETE FROM credentials WHERE deployment = {} RETURNING key_id",
                sql_lit(dep)
            ))
            .await?;
        let key = gone.first().and_then(|r| cell(r, 0)).map(str::to_string);
        let (outcome, detail) = match &key {
            Some(k) => ("ok", format!("key={k}")),
            None => ("nothing", String::new()),
        };
        tx.rows(&format!(
            "INSERT INTO audit (deployment, at, actor, action, outcome, detail) \
             VALUES ({}, {now}, {}, {}, {}, {})",
            sql_lit(dep),
            sql_lit(actor),
            sql_lit(action),
            sql_lit(outcome),
            sql_lit(&detail)
        ))
        .await?;
        tx.commit().await?;
        Ok(key)
    }

    /// Write one audit row.
    pub async fn audit(
        &self,
        dep: &str,
        actor: &str,
        action: &str,
        outcome: &str,
        detail: &str,
        now: u64,
    ) -> Result<(), ConsoleError> {
        self.pool
            .rows(&format!(
                "INSERT INTO audit (deployment, at, actor, action, outcome, detail) \
                 VALUES ({}, {now}, {}, {}, {}, {})",
                sql_lit(dep),
                sql_lit(actor),
                sql_lit(action),
                sql_lit(outcome),
                sql_lit(detail)
            ))
            .await?;
        Ok(())
    }

    /// One page of a deployment's audit log, newest first: rows before id
    /// `before` (or from the newest), whose action starts with `prefix`
    /// when one is given.
    pub async fn audit_page(
        &self,
        dep: &str,
        before: Option<u64>,
        prefix: Option<&str>,
        limit: u64,
    ) -> Result<Vec<AuditRow>, ConsoleError> {
        let mut clauses = vec![format!("deployment = {}", sql_lit(dep))];
        if let Some(b) = before {
            clauses.push(format!("id < {b}"));
        }
        if let Some(p) = prefix {
            let escaped = p
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            clauses.push(format!("action LIKE {}", sql_lit(&format!("{escaped}%"))));
        }
        Ok(self
            .pool
            .rows(&format!(
                "SELECT id, at, actor, action, outcome, detail FROM audit WHERE {} \
                 ORDER BY id DESC LIMIT {limit}",
                clauses.join(" AND ")
            ))
            .await?
            .iter()
            .map(|r| AuditRow {
                id: num(r, 0).unwrap_or(0),
                at: num(r, 1).unwrap_or(0),
                actor: cell(r, 2).unwrap_or_default().to_string(),
                action: cell(r, 3).unwrap_or_default().to_string(),
                outcome: cell(r, 4).unwrap_or_default().to_string(),
                detail: cell(r, 5).unwrap_or_default().to_string(),
            })
            .collect())
    }
}

/// One row of the console's audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRow {
    pub id: u64,
    pub at: u64,
    pub actor: String,
    pub action: String,
    pub outcome: String,
    pub detail: String,
}
