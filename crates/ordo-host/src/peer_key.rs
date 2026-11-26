//! Which consumer key this deployment signs with, and whether the identity
//! service knows it (`plan-federation-grants.md` G9).
//!
//! The key is derived from the home space's blind-index key under the
//! label `$consumerkey` and a version, `v1` at first ([`label`]). A
//! rotation bumps the version, so it needs no new index key. The version
//! and the public key last registered are kept in one row of
//! [`TABLE`] in the control database, so both survive a plan, an apply and
//! a restart, and every node of the deployment signs with the same key.
//!
//! Rotating the index key itself rotates the consumer key too: the key the
//! node derives no longer matches [`KeyRecord::registered`], the plan says
//! `not registered`, and the next `ordo apply` of a file with a `peer`
//! block registers the new one. Until it does, calls to other deployments
//! are refused as `KeyNotRegistered`.

use ordo_exec::{sql_lit, Pool, PoolError};

/// The one-row table holding [`KeyRecord`].
pub const TABLE: &str = "$consumerkey";

const DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$consumerkey" (
  "id"            int    PRIMARY KEY CHECK ("id" = 1),
  "version"       int    NOT NULL,
  "registered"    text,
  "registered_at" bigint NOT NULL DEFAULT 0
)"#;

/// The label version a key is derived under: `v1`, `v2`, ...
pub fn label(version: u32) -> String {
    format!("v{version}")
}

/// What is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRecord {
    /// The label version this deployment signs with.
    pub version: u32,
    /// The public key last registered at the identity service, base64url.
    pub registered: Option<String>,
    pub registered_at: u64,
}

impl Default for KeyRecord {
    fn default() -> Self {
        KeyRecord {
            version: 1,
            registered: None,
            registered_at: 0,
        }
    }
}

/// The stored record, in the control database.
pub struct KeyStore {
    pool: Pool,
}

impl KeyStore {
    pub fn new(pool: Pool) -> KeyStore {
        KeyStore { pool }
    }

    /// The record; the first version with nothing registered before any.
    pub async fn load(&self) -> Result<KeyRecord, PoolError> {
        let rows = match self
            .pool
            .rows(
                r#"SELECT "version"::text, "registered", "registered_at"::text
                   FROM "$consumerkey" WHERE "id" = 1"#,
            )
            .await
        {
            Ok(rows) => rows,
            Err(e) if e.is_missing_table() => return Ok(KeyRecord::default()),
            Err(e) => return Err(e),
        };
        let Some(row) = rows.into_iter().next() else {
            return Ok(KeyRecord::default());
        };
        let mut it = row.into_iter();
        let version = it
            .next()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let registered = it.next().flatten();
        let registered_at = it
            .next()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Ok(KeyRecord {
            version,
            registered,
            registered_at,
        })
    }

    /// Keep `record`.
    pub async fn save(&self, record: &KeyRecord) -> Result<(), PoolError> {
        self.pool.rows(DDL).await?;
        let registered = record
            .registered
            .as_deref()
            .map_or_else(|| "NULL".to_string(), sql_lit);
        self.pool
            .rows(&format!(
                r#"INSERT INTO "$consumerkey" ("id", "version", "registered", "registered_at")
                   VALUES (1, {v}, {registered}, {at})
                   ON CONFLICT ("id") DO UPDATE SET "version" = EXCLUDED."version",
                     "registered" = EXCLUDED."registered",
                     "registered_at" = EXCLUDED."registered_at""#,
                v = record.version,
                at = record.registered_at,
            ))
            .await
            .map(|_| ())
    }
}

/// What the plan and the status op say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyState {
    pub version: u32,
    /// This node's public key, base64url.
    pub public: String,
    /// The identity service was given exactly this key.
    pub registered: bool,
    pub registered_at: u64,
}

impl KeyState {
    /// One line for a plan.
    pub fn line(&self) -> String {
        let id = &self.public[..self.public.len().min(8)];
        if self.registered {
            format!(
                "peer key    consumer key {} ({id}) is registered at the identity service",
                label(self.version)
            )
        } else {
            format!(
                "peer key    consumer key {} ({id}) is not registered at the identity \
                 service. `ordo apply` registers it when the file has a `peer` block, or run \
                 `ordo peers rotate-key`",
                label(self.version)
            )
        }
    }
}
