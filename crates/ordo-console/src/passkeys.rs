//! The console's passkeys, through `ordo_identity`'s provider.
//!
//! The ceremonies (challenges, signature checks, sign counts) are
//! `ordo_identity::passkey::PasskeyProvider`'s, against the console's own
//! relying party. This file only stores the keys in the console's database.
//!
//! The provider's store trait is synchronous. The console calls the
//! provider on a blocking thread (`spawn_blocking`) and this store blocks
//! that thread on the pool, so no async worker ever waits.
//!
//! Open ceremonies live in memory (`MemoryCeremonyStore`): the console is
//! one process, a challenge lives 300 seconds, and a restart in between
//! costs one more touch.

use std::sync::Arc;

use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_identity::ceremony::MemoryCeremonyStore;
use ordo_identity::passkey::{
    KeyRole, PasskeyCredential, PasskeyProvider, PasskeyStore, PasskeyStoreError, RelyingParty,
    StoredPasskey,
};
use ordo_identity::webauthn::{b64url_decode, b64url_encode, CoseKey};
use ordo_identity::{StoreFault, Timestamp};
use tokio::runtime::Handle;

use crate::config::{ORIGIN_ENV, RP_ID_ENV};
use crate::error::ConsoleError;

/// The passkeys of one deployment's console owners.
pub struct PgPasskeys {
    pool: Pool,
    deployment: String,
    rt: Handle,
}

fn unavailable(e: PoolError) -> PasskeyStoreError {
    PasskeyStoreError::Unavailable(StoreFault::caused_by(
        e.class(),
        "the console's passkey table did not answer",
        e,
    ))
}

impl PgPasskeys {
    fn run(&self, sql: String) -> Result<Vec<ordo_exec::TextRow>, PasskeyStoreError> {
        let pool = self.pool.clone();
        self.rt
            .block_on(async move { pool.rows(&sql).await })
            .map_err(unavailable)
    }
}

impl PasskeyStore for PgPasskeys {
    fn get(&self, id: &[u8]) -> Result<Option<StoredPasskey>, PasskeyStoreError> {
        let rows = self.run(format!(
            "SELECT algorithm, public_key, sign_count, user_handle, backup_eligible, backed_up, \
             registered_at, last_used_at FROM passkeys WHERE deployment = {} AND credential_id = {}",
            sql_lit(&self.deployment),
            sql_lit(&b64url_encode(id))
        ))?;
        let Some(r) = rows.first() else {
            return Ok(None);
        };
        let text = |i: usize| r.get(i).and_then(|c| c.as_deref()).unwrap_or_default();
        let int = |i: usize| text(i).parse::<i64>().unwrap_or(0);
        let corrupt = || {
            PasskeyStoreError::Unavailable(StoreFault::unavailable(
                "a console passkey row does not decode",
            ))
        };
        let key_bytes = b64url_decode(text(1)).map_err(|_| corrupt())?;
        let key = CoseKey::from_bytes(int(0), &key_bytes).map_err(|_| corrupt())?;
        Ok(Some(StoredPasskey {
            credential: PasskeyCredential::from_parts(
                id.to_vec(),
                key,
                u32::try_from(int(2)).unwrap_or(u32::MAX),
            ),
            role: KeyRole::Passkey,
            user_handle: b64url_decode(text(3)).map_err(|_| corrupt())?,
            backup_eligible: text(4) == "t",
            backed_up: text(5) == "t",
            registered_at: Timestamp(int(6).max(0) as u64),
            last_used_at: Timestamp(int(7).max(0) as u64),
        }))
    }

    /// The console stores a passkey together with its owner, in the same
    /// transaction that makes the owner (`Store::enrol_with_code`,
    /// `Store::add_passkey`), so the provider's own insert is never used.
    fn insert(&self, _cred: &StoredPasskey) -> Result<(), PasskeyStoreError> {
        Err(PasskeyStoreError::Unavailable(StoreFault::unavailable(
            "the console stores a passkey with its owner, never alone",
        )))
    }

    fn set_sign_count(
        &self,
        id: &[u8],
        count: u32,
        at: Timestamp,
    ) -> Result<bool, PasskeyStoreError> {
        let rows = self.run(format!(
            "UPDATE passkeys SET sign_count = GREATEST(sign_count, {count}), last_used_at = {} \
             WHERE deployment = {} AND credential_id = {} AND ({count} = 0 OR sign_count < {count}) \
             RETURNING 1",
            at.0,
            sql_lit(&self.deployment),
            sql_lit(&b64url_encode(id))
        ))?;
        Ok(!rows.is_empty())
    }

    fn remove(&self, id: &[u8]) -> Result<(), PasskeyStoreError> {
        self.run(format!(
            "DELETE FROM passkeys WHERE deployment = {} AND credential_id = {}",
            sql_lit(&self.deployment),
            sql_lit(&b64url_encode(id))
        ))?;
        Ok(())
    }
}

/// The provider for one deployment's console, over its own relying party.
pub fn provider(
    pool: &Pool,
    deployment: &str,
    rp_id: &str,
    origin: &str,
) -> Result<PasskeyProvider, ConsoleError> {
    let rp = RelyingParty::new(rp_id, &[origin])
        .map_err(|e| {
            ConsoleError::setting_caused(
                RP_ID_ENV,
                format!("`{rp_id}` and {ORIGIN_ENV} `{origin}` are not a usable relying party"),
                format!(
                    "Set {ORIGIN_ENV} to https://<host> (or http://localhost for a trial) and \
                     {RP_ID_ENV} to that host or a domain above it"
                ),
                e,
            )
        })?
        .with_name("Ordo console");
    Ok(PasskeyProvider::with_stores(
        rp,
        Arc::new(PgPasskeys {
            pool: pool.clone(),
            deployment: deployment.to_string(),
            rt: Handle::current(),
        }),
        Arc::new(MemoryCeremonyStore::new()),
    ))
}
