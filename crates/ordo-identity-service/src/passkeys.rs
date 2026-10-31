//! Deployment people's passkeys, through `ordo_identity`'s provider
//! (`plan-identity-service.md` 2.1).
//!
//! The ceremonies (challenges, signature checks, sign counts) are
//! `ordo_identity::passkey::PasskeyProvider`'s, against the service's own
//! relying party (`ORDO_IDENTITY_RP_ID`). This file stores the keys in the
//! service's `passkeys` table, whose columns are the console's, so a
//! console passkey moves in as a copy (6.1). It is the console's
//! `passkeys.rs`, moved; the console's copy goes in step 6.
//!
//! The provider's store trait is synchronous. The service calls the
//! provider on a blocking thread (`spawn_blocking`) and this store blocks
//! that thread on the pool, so no async worker ever waits. Open ceremonies
//! live in memory: one node, 300 second challenges, and a restart in
//! between costs one more touch.

use std::sync::Arc;

use ordo_exec::{sql_lit, Pool, PoolError, TxConn};
use ordo_identity::ceremony::MemoryCeremonyStore;
use ordo_identity::origin::Origin;
use ordo_identity::passkey::{
    KeyRole, PasskeyCredential, PasskeyProvider, PasskeyStore, PasskeyStoreError, RelyingParty,
    StoredPasskey,
};
use ordo_identity::webauthn::{b64url_decode, b64url_encode, CoseKey};
use ordo_identity::{StoreFault, Timestamp};
use tokio::runtime::Handle;

use crate::config::{ORIGIN_ENV, RP_ID_ENV};
use crate::error::IdentityError;
use crate::store::{dep, text};

/// The relying party's name, as an authenticator shows it.
pub const RP_NAME: &str = "Ordo deployment";

/// The passkeys of one deployment's people.
pub struct PgPasskeys {
    pool: Pool,
    deployment: String,
    rt: Handle,
}

fn unavailable(e: PoolError) -> PasskeyStoreError {
    PasskeyStoreError::Unavailable(StoreFault::caused_by(
        e.class(),
        "the identity service's passkey table did not answer",
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
            PasskeyStoreError::Unavailable(StoreFault::unavailable("a passkey row does not decode"))
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

    /// A passkey is stored with its person, in the transaction that makes
    /// or changes the person ([`insert`]), so the provider's own insert is
    /// never used.
    fn insert(&self, _cred: &StoredPasskey) -> Result<(), PasskeyStoreError> {
        Err(PasskeyStoreError::Unavailable(StoreFault::unavailable(
            "the identity service stores a passkey with its person, never alone",
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

/// The provider for one deployment, over the service's relying party.
/// `origins` are the pages its ceremonies may run in, the service's own
/// origin first. The touch's provider adds client origins under the RP ID
/// ([`ceremony_origins`]); every other ceremony's lists the service alone.
pub fn provider(
    pool: &Pool,
    deployment: &str,
    rp_id: &str,
    origins: &[String],
) -> Result<PasskeyProvider, IdentityError> {
    let origin = origins.first().map(String::as_str).unwrap_or_default();
    let listed: Vec<&str> = origins.iter().map(String::as_str).collect();
    let rp = RelyingParty::new(rp_id, &listed)
        .map_err(|e| {
            IdentityError::setting_caused(
                RP_ID_ENV,
                format!("`{rp_id}` and {ORIGIN_ENV} `{origin}` are not a usable relying party"),
                format!(
                    "Set {ORIGIN_ENV} to https://<host> (or http://localhost for a trial) and \
                     {RP_ID_ENV} to that host or a domain above it"
                ),
                e,
            )
        })?
        .with_name(RP_NAME);
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

/// The origins the touch may run in: the service's own, then each
/// client origin (`ORDO_IDENTITY_CLIENT_ORIGINS`) whose host is the RP ID
/// or under it. The console page runs the step-up touch in its own page
/// against `/touch/begin` and `/touch/finish` (`plan-identity-service.md`
/// 5.4 step 6), so the browser signs the console's origin into the
/// assertion. A client origin outside the RP ID could not assert these
/// passkeys anyway (the browser refuses), so it is left out rather than
/// refusing the boot; it can still ask for tokens. Only the touch uses this
/// list: sign-in, join, registration and recovery accept the service's own
/// origin alone, so a client page can never enrol a passkey or open a
/// session.
pub fn ceremony_origins(rp_id: &str, own: &str, clients: &[Origin]) -> Vec<String> {
    let mut out = vec![own.to_string()];
    for c in clients {
        let host = c.host();
        let under = host == rp_id || host.ends_with(&format!(".{rp_id}"));
        let text = c.to_string();
        if under && !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

/// Store a passkey for a person, inside `tx`.
pub async fn insert(
    tx: &TxConn,
    handle: &str,
    person: &str,
    p: &StoredPasskey,
    now: u64,
) -> Result<(), IdentityError> {
    tx.rows(&format!(
        "INSERT INTO passkeys (deployment, credential_id, person_id, algorithm, public_key, \
         sign_count, user_handle, backup_eligible, backed_up, registered_at, last_used_at, \
         created_at) VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, 0, {now})",
        sql_lit(handle),
        sql_lit(&b64url_encode(p.credential.id())),
        sql_lit(person),
        p.credential.algorithm(),
        sql_lit(&b64url_encode(&p.credential.key().to_bytes())),
        p.credential.sign_count(),
        sql_lit(&b64url_encode(&p.user_handle)),
        p.backup_eligible,
        p.backed_up,
        p.registered_at.0,
    ))
    .await?;
    Ok(())
}

/// Remove every passkey a person holds but `kept`, inside `tx`: after a
/// recovery code, the lost device's key goes. Never leaves the person with
/// none: if `kept` is not there, nothing is removed.
pub async fn retire_others(
    tx: &TxConn,
    handle: &str,
    person: &str,
    kept: &StoredPasskey,
) -> Result<u64, IdentityError> {
    let (d, p) = (dep(handle), sql_lit(person));
    let k = sql_lit(&b64url_encode(kept.credential.id()));
    Ok(tx
        .rows(&format!(
            "DELETE FROM passkeys WHERE {d} AND person_id = {p} AND credential_id <> {k} \
             AND EXISTS (SELECT 1 FROM passkeys WHERE {d} AND person_id = {p} \
             AND credential_id = {k}) RETURNING 1"
        ))
        .await?
        .len() as u64)
}

/// The live person a credential belongs to, from the provider's
/// `passkey:<id>` authenticator name.
pub async fn person_of(
    pool: &Pool,
    handle: &str,
    authenticator: &str,
) -> Result<Option<String>, IdentityError> {
    let Some(credential) = authenticator.strip_prefix("passkey:") else {
        return Ok(None);
    };
    let rows = pool
        .rows(&format!(
            "SELECT k.person_id FROM passkeys k JOIN people p ON p.id = k.person_id \
             AND p.deployment = k.deployment WHERE k.{} AND k.credential_id = {} \
             AND p.ended_at IS NULL",
            dep(handle),
            sql_lit(credential)
        ))
        .await?;
    Ok(rows.first().map(|r| text(r, 0)))
}

/// A person's credential ids, raw.
pub async fn credentials_of(
    pool: &Pool,
    handle: &str,
    person: &str,
) -> Result<Vec<Vec<u8>>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT credential_id FROM passkeys WHERE {} AND person_id = {} ORDER BY registered_at",
            dep(handle),
            sql_lit(person)
        ))
        .await?
        .iter()
        .filter_map(|r| b64url_decode(&text(r, 0)).ok())
        .collect())
}

/// One passkey as its holder sees it on the account page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The credential id, base64url.
    pub id: String,
    /// The WebAuthn user handle it was made under, base64url.
    pub user_handle: String,
    pub registered_at: u64,
    /// 0 when it has never signed in.
    pub last_used_at: u64,
    /// Synced by a passkey provider, so it lives on more than one device.
    pub backed_up: bool,
}

/// A person's passkeys, oldest first.
pub async fn list(pool: &Pool, handle: &str, person: &str) -> Result<Vec<Listed>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT credential_id, user_handle, registered_at, last_used_at, backed_up \
             FROM passkeys WHERE {} AND person_id = {} ORDER BY registered_at, credential_id",
            dep(handle),
            sql_lit(person)
        ))
        .await?
        .iter()
        .map(|r| Listed {
            id: text(r, 0),
            user_handle: text(r, 1),
            registered_at: text(r, 2).parse().unwrap_or(0),
            last_used_at: text(r, 3).parse().unwrap_or(0),
            backed_up: text(r, 4) == "t",
        })
        .collect())
}

/// Remove one of a person's passkeys, inside `tx`. The person's row is
/// locked first, so two removals at once cannot leave them with none.
/// Refused with [`IdentityError::LastPasskey`] when it is their only one,
/// and [`IdentityError::UnknownPasskey`] when they hold no such passkey.
pub async fn remove_one(
    tx: &TxConn,
    handle: &str,
    person: &str,
    credential: &str,
) -> Result<(), IdentityError> {
    let (d, p) = (dep(handle), sql_lit(person));
    tx.rows(&format!(
        "SELECT 1 FROM people WHERE {d} AND id = {p} FOR UPDATE"
    ))
    .await?;
    let held = tx
        .rows(&format!(
            "SELECT credential_id FROM passkeys WHERE {d} AND person_id = {p}"
        ))
        .await?;
    if !held.iter().any(|r| text(r, 0) == credential) {
        return Err(IdentityError::UnknownPasskey);
    }
    if held.len() < 2 {
        return Err(IdentityError::LastPasskey);
    }
    tx.rows(&format!(
        "DELETE FROM passkeys WHERE {d} AND person_id = {p} AND credential_id = {}",
        sql_lit(credential)
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origins(list: &[&str]) -> Vec<Origin> {
        list.iter().map(|o| Origin::parse(o).unwrap()).collect()
    }

    #[test]
    fn a_client_origin_under_the_rp_id_may_run_a_ceremony_and_one_outside_may_not() {
        let got = ceremony_origins(
            "console.example.com",
            "https://console.example.com:8082",
            &origins(&[
                "https://console.example.com",
                "https://ops.console.example.com",
                "https://other.example.com",
                "https://evilconsole.example.com",
            ]),
        );
        assert_eq!(
            got,
            vec![
                "https://console.example.com:8082",
                "https://console.example.com",
                "https://ops.console.example.com",
            ]
        );
        // Local trial: the console on 8081 and the service on 8082 share
        // `localhost`; the port does not matter to WebAuthn.
        let got = ceremony_origins(
            "localhost",
            "http://localhost:8082",
            &origins(&["http://localhost:8081"]),
        );
        assert_eq!(got, vec!["http://localhost:8082", "http://localhost:8081"]);
    }
}
