//! The identity service's own database (`plan-identity-service.md` 2.2):
//! its tables and its migrations.
//!
//! Reached through the one database client (`ordo_exec::Pool`), opened with
//! `Pool::connect_own` so it gets none of a deployment's tables. Every
//! failure is the client's typed `PoolError`, carried up inside
//! [`IdentityError::Store`].
//!
//! Every table has `created_at` and a `deployment` handle, so the private
//! provisioning service can later hold many deployments in one service.
//! Migration 1 makes every table of 2.2, plus `guess_counts`, the guessing
//! limits every instance shares ([`crate::throttle`]). Once shipped, a
//! migration is never edited.

use ordo_exec::{sql_lit, Pool, PoolLimits, TextRow};

use crate::config::{IdentityConfig, DATABASE_URL_ENV};
use crate::error::IdentityError;

const MIGRATIONS: &[(i32, &str)] = &[(1, MIGRATION_1), (2, MIGRATION_2)];

/// The advisory lock migrations run under, so two services booting at once
/// do not race.
const MIGRATION_LOCK: i64 = 726_201_802;

/// Guessing limits per source address, plus a shared row (`source` empty)
/// for the ceiling over everyone (owner, 2026-10-27).
const MIGRATION_2: &str = r#"
ALTER TABLE guess_counts ADD COLUMN source text NOT NULL DEFAULT '';
ALTER TABLE guess_counts DROP CONSTRAINT guess_counts_pkey;
ALTER TABLE guess_counts ADD PRIMARY KEY (deployment, kind, source);
"#;

const MIGRATION_1: &str = r#"
CREATE TABLE people (
  id               text   PRIMARY KEY,
  deployment       text   NOT NULL,
  display_name     text   NOT NULL,
  role             text   NOT NULL CHECK (role IN ('admin')),
  console_owner_id text,
  created_at       bigint NOT NULL,
  ended_at         bigint,
  ended_by         text
);
CREATE UNIQUE INDEX people_by_console_owner ON people (deployment, console_owner_id)
  WHERE console_owner_id IS NOT NULL;

CREATE TABLE passkeys (
  deployment      text    NOT NULL,
  credential_id   text    NOT NULL,
  person_id       text    NOT NULL REFERENCES people (id),
  algorithm       bigint  NOT NULL,
  public_key      text    NOT NULL,
  sign_count      bigint  NOT NULL,
  user_handle     text    NOT NULL,
  backup_eligible boolean NOT NULL,
  backed_up       boolean NOT NULL,
  registered_at   bigint  NOT NULL,
  last_used_at    bigint  NOT NULL DEFAULT 0,
  created_at      bigint  NOT NULL,
  PRIMARY KEY (deployment, credential_id)
);
CREATE INDEX passkeys_by_user_handle ON passkeys (deployment, user_handle);

CREATE TABLE recovery_codes (
  id          bigserial PRIMARY KEY,
  deployment  text   NOT NULL,
  person_id   text   NOT NULL REFERENCES people (id),
  lookup      text   NOT NULL,
  hash        text   NOT NULL,
  issued_at   bigint NOT NULL,
  used_at     bigint,
  voided_at   bigint,
  created_at  bigint NOT NULL
);
CREATE UNIQUE INDEX recovery_codes_live ON recovery_codes (deployment, lookup)
  WHERE used_at IS NULL AND voided_at IS NULL;

CREATE TABLE invites (
  id           text   PRIMARY KEY,
  deployment   text   NOT NULL,
  secret_hash  text   NOT NULL UNIQUE,
  label        text   NOT NULL,
  role         text   NOT NULL CHECK (role IN ('admin')),
  issued_by    text   NOT NULL,
  issued_at    bigint NOT NULL,
  expires_at   bigint NOT NULL,
  used_at      bigint,
  used_by      text,
  revoked_at   bigint,
  created_at   bigint NOT NULL
);

CREATE TABLE sessions (
  id           text   PRIMARY KEY,
  deployment   text   NOT NULL,
  secret_hash  text   NOT NULL UNIQUE,
  subject      text   NOT NULL,
  kind         text   NOT NULL CHECK (kind IN ('browser', 'cli', 'machine')),
  client       text   NOT NULL,
  created_at   bigint NOT NULL,
  last_seen_at bigint NOT NULL,
  touched_at   bigint,
  must_enrol   boolean NOT NULL DEFAULT false,
  ends_at      bigint NOT NULL,
  ended_at     bigint,
  ended_why    text
);
CREATE INDEX sessions_live_by_subject ON sessions (deployment, subject) WHERE ended_at IS NULL;

CREATE TABLE cli_grants (
  code_hash      text   PRIMARY KEY,
  deployment     text   NOT NULL,
  person_id      text   NOT NULL REFERENCES people (id),
  code_challenge text   NOT NULL,
  touched_at     bigint NOT NULL,
  expires_at     bigint NOT NULL,
  used_at        bigint,
  created_at     bigint NOT NULL
);

CREATE TABLE machine_keys (
  id           text   PRIMARY KEY,
  deployment   text   NOT NULL,
  name         text   NOT NULL,
  public_key   text   NOT NULL,
  created_by   text   NOT NULL,
  created_at   bigint NOT NULL,
  last_used_at bigint,
  ended_at     bigint,
  ended_by     text
);
CREATE UNIQUE INDEX machine_keys_live_name ON machine_keys (deployment, name)
  WHERE ended_at IS NULL;

CREATE TABLE signing_keys (
  deployment   text   NOT NULL,
  kid          text   NOT NULL,
  purpose      text   NOT NULL CHECK (purpose IN ('token', 'audit')),
  public_key   text   NOT NULL,
  sealed       text   NOT NULL,
  created_at   bigint NOT NULL,
  active_from  bigint NOT NULL,
  retired_at   bigint,
  PRIMARY KEY (deployment, kid)
);

CREATE TABLE audit (
  deployment  text   NOT NULL,
  seq         bigint NOT NULL,
  at          bigint NOT NULL,
  actor       text   NOT NULL,
  action      text   NOT NULL,
  subject     text   NOT NULL,
  outcome     text   NOT NULL,
  client      text   NOT NULL,
  detail      text   NOT NULL,
  prev        text   NOT NULL,
  hash        text   NOT NULL,
  created_at  bigint NOT NULL,
  PRIMARY KEY (deployment, seq)
);

CREATE TABLE checkpoints (
  deployment  text   NOT NULL,
  upto        bigint NOT NULL,
  head        text   NOT NULL,
  at          bigint NOT NULL,
  kid         text   NOT NULL,
  signature   text   NOT NULL,
  created_at  bigint NOT NULL,
  PRIMARY KEY (deployment, upto)
);

CREATE TABLE guess_counts (
  deployment   text   NOT NULL,
  kind         text   NOT NULL,
  window_start bigint NOT NULL,
  tries        bigint NOT NULL,
  created_at   bigint NOT NULL,
  PRIMARY KEY (deployment, kind)
);

CREATE TABLE imports (
  deployment  text   NOT NULL,
  source      text   NOT NULL,
  rp_id       text   NOT NULL,
  people      bigint NOT NULL,
  passkeys    bigint NOT NULL,
  codes       bigint NOT NULL,
  created_at  bigint NOT NULL,
  PRIMARY KEY (deployment, source)
);
"#;

/// The service's database.
#[derive(Clone)]
pub struct Store {
    pool: Pool,
}

pub(crate) fn cell(row: &TextRow, i: usize) -> Option<&str> {
    row.get(i).and_then(|c| c.as_deref())
}

pub(crate) fn text(row: &TextRow, i: usize) -> String {
    cell(row, i).unwrap_or_default().to_string()
}

pub(crate) fn num(row: &TextRow, i: usize) -> Option<u64> {
    cell(row, i)
        .and_then(|v| v.parse::<i64>().ok())
        .map(|n| n.max(0) as u64)
}

pub(crate) fn opt_num(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "NULL".into())
}

impl Store {
    /// Open the service's database. One that cannot be reached is a boot
    /// refusal naming the setting, with the client's error kept.
    pub async fn open(cfg: &IdentityConfig) -> Result<Store, IdentityError> {
        let db = &cfg.database;
        if let Some(console) = &cfg.create_beside {
            // The console's role makes the service's database on the
            // console's server, once; a second start finds it there.
            let made = ordo_exec::create_database(console, &db.db)
                .await
                .map_err(|e| {
                    IdentityError::setting_caused(
                        DATABASE_URL_ENV,
                        format!(
                            "is not set, and database `{}` could not be made beside the \
                             console's on {}:{} with the console's role",
                            db.db, console.host, console.port
                        ),
                        format!(
                            "Give the console's role CREATEDB, or create the database yourself \
                             and set {DATABASE_URL_ENV} to it, or set \
                             {} to false and set {DATABASE_URL_ENV}",
                            crate::config::DATABASE_BESIDE_CONSOLE_ENV
                        ),
                        e,
                    )
                })?;
            if made {
                eprintln!(
                    "ordo-identity-service: made database `{}` beside the console's on {}:{} \
                     ({DATABASE_URL_ENV} is not set); back it up with the console's",
                    db.db, console.host, console.port
                );
            }
        }
        let pool = Pool::connect_own(db.clone(), PoolLimits::default())
            .await
            .map_err(|e| {
                IdentityError::setting_caused(
                    DATABASE_URL_ENV,
                    format!(
                        "names database `{}` on {}:{}, and it could not be opened",
                        db.db, db.host, db.port
                    ),
                    format!(
                        "Check that Postgres is running and listening on {}:{}, then the host, \
                         port, role and password in {DATABASE_URL_ENV}, and that the database \
                         exists (CREATE DATABASE {})",
                        db.host, db.port, db.db
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
    /// under an advisory lock.
    pub async fn migrate(&self, now: u64) -> Result<(), IdentityError> {
        self.pool
            .rows(
                "CREATE TABLE IF NOT EXISTS identity_migrations (\
                 version integer PRIMARY KEY, applied_at bigint NOT NULL)",
            )
            .await?;
        for (version, sql) in MIGRATIONS {
            let tx = self.pool.begin().await?;
            tx.rows(&format!("SELECT pg_advisory_xact_lock({MIGRATION_LOCK})"))
                .await?;
            let done = tx
                .rows(&format!(
                    "SELECT 1 FROM identity_migrations WHERE version = {version}"
                ))
                .await?;
            if done.is_empty() {
                tx.rows(sql).await?;
                tx.rows(&format!(
                    "INSERT INTO identity_migrations (version, applied_at) \
                     VALUES ({version}, {now})"
                ))
                .await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }

    /// The migrations applied, in order.
    pub async fn applied(&self) -> Result<Vec<u64>, IdentityError> {
        Ok(self
            .pool
            .rows("SELECT version FROM identity_migrations ORDER BY version")
            .await?
            .iter()
            .filter_map(|r| num(r, 0))
            .collect())
    }
}

/// `deployment = '<handle>'`, the filter every statement carries.
pub(crate) fn dep(handle: &str) -> String {
    format!("deployment = {}", sql_lit(handle))
}
