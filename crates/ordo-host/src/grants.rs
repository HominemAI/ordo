//! The Postgres grant registry.
//!
//! [`ordo_identity::grants`] states the contract and the boundary — in
//! particular that this is **not** consulted to admit a request, so a
//! registry outage is not an authorization outage. What follows is only what
//! building it decided.
//!
//! # `"$space"`, not `"space"`
//!
//! The command log uses a bare `"space"` column; this uses `"$space"`, the
//! same spelling the write protocol's `SpaceAudit` enforces on every
//! generated table. That is deliberate rather than cosmetic: a grant is
//! per-space by the strongest pin in `security-logging-design.md` §5 (*"a
//! grant is scoped to exactly one space… there is no deployment-wide
//! grant"*), and giving the column the name the audit already polices means
//! a future statement that forgets to constrain it is caught by the
//! mechanism that catches every other one.
//!
//! # What is stored
//!
//! The op name, its **own** pinned manifest hash, and the **whole pinned
//! shape** — the field set and the row-scope shape, canonically encoded.
//!
//! This column set is 8s. Before it, the table stored one *combined* hash
//! under a per-op column name and no field set at all, and this doc recorded
//! that as *"a known gap rather than a decision"*: `CapabilityGrant` carried
//! neither per-op hashes nor field sets, and recording what the grant does
//! not hold would have been inventing it. The grant carries both now, so the
//! table stores both.
//!
//! `"pin"` is not a second source of truth. The door decides from the token,
//! which carries its own pins; this column exists so a person asking *"what
//! did I agree to?"* gets an answer, and so a revoked grant can be shown as
//! what it was rather than as an op name.
//!
//! Nothing here is sealed. A grant record is metadata about access — who
//! consented, to what, when — and it is queried by op and by principal,
//! which sealing would prevent. It holds no row content and no payload.

use ordo_identity::grants::{GrantError, GrantRecord, GrantRegistry, NewGrant};
use ordo_identity::{ClientId, GrantId, ManifestHash, PrincipalId, SpaceId, Timestamp};

use ordo_exec::Pool;

/// A platform table, created at boot beside the others.
pub const GRANT_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS "$capabilitygrant" (
    "$space"        text   NOT NULL,
    "grant_id"      text   NOT NULL,
    "op"            text   NOT NULL,
    "principal"     text   NOT NULL,
    "client"        text,
    "manifest_hash" text   NOT NULL,
    "pin"           text   NOT NULL DEFAULT '',
    "granted_at"    bigint NOT NULL,
    "expires"       bigint NOT NULL,
    "revoked_at"    bigint,
    PRIMARY KEY ("$space", "grant_id", "op")
)"#;

/// The `"pin"` column arrived with 8s, after the table shipped. `CREATE TABLE
/// IF NOT EXISTS` adds nothing to a table that already exists, so the column
/// needs its own statement or every deployment that ever booted the old shape
/// would keep it. `DEFAULT ''` fills the rows that predate it: a grant minted
/// before pins existed has no pinned shape to record, and an empty string
/// says exactly that rather than pretending to one.
pub const GRANT_PIN_DDL: &str = r#"
ALTER TABLE "$capabilitygrant" ADD COLUMN IF NOT EXISTS "pin" text NOT NULL DEFAULT ''"#;

/// `callers_of` is keyed on the op, so the op leads the index. Without it the
/// audit surface degrades exactly as a space gets more interesting, which is
/// the wrong direction for the one query an auditor runs.
pub const GRANT_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS "$capabilitygrant$op"
    ON "$capabilitygrant" ("$space", "op")"#;

pub struct PgGrantRegistry {
    pool: Pool,
}

impl PgGrantRegistry {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn ensure_table(&self) -> Result<(), GrantError> {
        for ddl in [GRANT_DDL, GRANT_PIN_DDL, GRANT_INDEX_DDL] {
            self.pool.rows(ddl).await.map_err(|e| {
                GrantError::Unavailable(crate::auth::store_failure::IntoFault::fault(e))
            })?;
        }
        Ok(())
    }

    async fn query(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, GrantError> {
        self.pool
            .rows(sql)
            .await
            .map_err(|e| GrantError::Unavailable(crate::auth::store_failure::IntoFault::fault(e)))
    }
}

fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn hex(h: &ManifestHash) -> String {
    h.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<ManifestHash> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(ManifestHash(out))
}

const COLUMNS: &str = "\"grant_id\", \"$space\", \"principal\", \"client\", \"op\", \
                       \"manifest_hash\", \"pin\", \"granted_at\", \"expires\", \"revoked_at\"";

fn row_to_record(row: &[Option<String>]) -> Option<GrantRecord> {
    let get = |i: usize| row.get(i).cloned().flatten();
    Some(GrantRecord {
        id: GrantId(get(0)?),
        space: SpaceId(get(1)?),
        principal: PrincipalId(get(2)?),
        client: get(3).map(ClientId),
        op: get(4)?,
        manifest_hash: unhex(&get(5)?)?,
        pin: get(6)?,
        granted_at: Timestamp(get(7)?.parse().ok()?),
        expires: Timestamp(get(8)?.parse().ok()?),
        revoked_at: get(9).and_then(|s| s.parse().ok()).map(Timestamp),
    })
}

#[async_trait::async_trait]
impl GrantRegistry for PgGrantRegistry {
    async fn record(&self, grant: &NewGrant) -> Result<(), GrantError> {
        let NewGrant {
            id,
            space,
            principal,
            client,
            pins,
            granted_at,
            expires,
        } = grant;
        if pins.is_empty() {
            // A grant to nothing is not a grant, and recording one would put
            // a row in the registry that `callers_of` can never return and
            // `revoke` can never usefully touch.
            return Ok(());
        }
        let values: Vec<String> = pins
            .iter()
            .map(|(op, pin)| {
                // One op's pin, encoded on its own, so a row means exactly
                // what its `op` column says and a `callers_of` answer can be
                // read without the rest of the grant.
                let one = std::collections::BTreeMap::from([(op.clone(), pin.clone())]);
                format!(
                    "({}, {}, {}, {}, {}, {}, {}, {}, {}, NULL)",
                    lit(&id.0),
                    lit(&space.0),
                    lit(&principal.0),
                    client
                        .as_ref()
                        .map(|c| lit(&c.0))
                        .unwrap_or_else(|| "NULL".into()),
                    lit(op),
                    lit(&hex(&pin.manifest_hash)),
                    lit(&ordo_identity::encode_pins(&one)),
                    granted_at.0,
                    expires.0,
                )
            })
            .collect();
        // One statement for the whole grant. A grant recorded op-by-op could
        // be interrupted half-recorded, and half a grant in the registry is
        // worse than none: `callers_of` would answer for some of its ops and
        // not others, which is the exhaustiveness failure this table exists
        // to close.
        let sql = format!(
            "INSERT INTO \"$capabilitygrant\" ({COLUMNS}) VALUES {} \
             ON CONFLICT (\"$space\", \"grant_id\", \"op\") DO NOTHING",
            values.join(", ")
        );
        self.query(&sql).await.map(|_| ())
    }

    async fn revoke(&self, space: &SpaceId, id: &GrantId, at: Timestamp) -> Result<(), GrantError> {
        // `WHERE revoked_at IS NULL` is what makes this idempotent in the way
        // that matters: a second revocation must not overwrite the first
        // one's timestamp, because that timestamp is the audit answer to
        // "when did this stop".
        let sql = format!(
            "UPDATE \"$capabilitygrant\" SET \"revoked_at\" = {} \
             WHERE \"$space\" = {} AND \"grant_id\" = {} AND \"revoked_at\" IS NULL",
            at.0,
            lit(&space.0),
            lit(&id.0),
        );
        self.query(&sql).await.map(|_| ())
    }

    async fn callers_of(
        &self,
        space: &SpaceId,
        op: &str,
        now: Timestamp,
    ) -> Result<Vec<GrantRecord>, GrantError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM \"$capabilitygrant\" \
             WHERE \"$space\" = {} AND \"op\" = {} \
               AND \"revoked_at\" IS NULL AND \"expires\" > {} \
             ORDER BY \"principal\", \"grant_id\"",
            lit(&space.0),
            lit(op),
            now.0,
        );
        Ok(self
            .query(&sql)
            .await?
            .iter()
            .filter_map(|r| row_to_record(r))
            .collect())
    }

    async fn live_with_prefix(
        &self,
        space: &SpaceId,
        prefix: &str,
        now: Timestamp,
    ) -> Result<Vec<GrantRecord>, GrantError> {
        // The prefix is matched with `starts_with` rather than `LIKE`, so a
        // `_` or `%` in it is a character and never a wildcard.
        let sql = format!(
            "SELECT {COLUMNS} FROM \"$capabilitygrant\" \
             WHERE \"$space\" = {} AND starts_with(\"grant_id\", {}) \
               AND \"revoked_at\" IS NULL AND \"expires\" > {} \
             ORDER BY \"principal\", \"granted_at\", \"grant_id\", \"op\"",
            lit(&space.0),
            lit(prefix),
            now.0,
        );
        Ok(self
            .query(&sql)
            .await?
            .iter()
            .filter_map(|r| row_to_record(r))
            .collect())
    }

    async fn grants_of(
        &self,
        space: &SpaceId,
        principal: &PrincipalId,
    ) -> Result<Vec<GrantRecord>, GrantError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM \"$capabilitygrant\" \
             WHERE \"$space\" = {} AND \"principal\" = {} \
             ORDER BY \"granted_at\" DESC, \"grant_id\", \"op\"",
            lit(&space.0),
            lit(&principal.0),
        );
        Ok(self
            .query(&sql)
            .await?
            .iter()
            .filter_map(|r| row_to_record(r))
            .collect())
    }

    async fn is_live(
        &self,
        space: &SpaceId,
        id: &GrantId,
        now: Timestamp,
    ) -> Result<bool, GrantError> {
        let sql = format!(
            "SELECT 1 FROM \"$capabilitygrant\" \
             WHERE \"$space\" = {} AND \"grant_id\" = {} \
               AND \"revoked_at\" IS NULL AND \"expires\" > {} LIMIT 1",
            lit(&space.0),
            lit(&id.0),
            now.0,
        );
        Ok(!self.query(&sql).await?.is_empty())
    }
}
