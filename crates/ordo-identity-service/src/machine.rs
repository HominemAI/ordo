//! Machine keys (`plan-identity-service.md` 5.1, 5.6).
//!
//! A machine key is an Ed25519 pair for software that runs the deployment
//! with nobody present: the provisioning service, CI. The service keeps the
//! public half. When the service makes the pair, the private half is shown
//! once as `ordo-mk-v1:<m_id>:<base64url seed>`, one value to paste into a
//! CI secret; a key the machine made is registered by its public half.
//!
//! Sign-in: `begin` hands out a challenge that stores nothing (16 random
//! bytes, an expiry, and an HMAC under a key this process made at start),
//! `complete` checks it, the signature, and that it was not spent before,
//! and opens a machine session. One refusal for every failure
//! ([`IdentityError::MachineRefused`]).

use std::collections::HashMap;
use std::sync::Mutex;

use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use ordo_exec::{sql_lit, Pool, TxConn};
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use serde_json::{json, Value};

use crate::audit::{self, Entry};
use crate::error::IdentityError;
use crate::keys::KeyRing;
use crate::people::Actor;
use crate::secrets::{mint_id, random_bytes};
use crate::sessions::{self, Lifetimes, NewSession, Opened, SessionKind};
use crate::store::{cell, dep, num, text};

/// A challenge lives 60 seconds.
pub const CHALLENGE_SECS: u64 = 60;
/// The prefix of a machine key's secret form.
pub const SECRET_PREFIX: &str = "ordo-mk-v1";
/// The name the provisioning service's key takes at first boot (5.1).
pub const PROVISIONER: &str = "provisioner";
const CHALLENGE_LEN: usize = 16 + 8 + 32;

/// A machine key's name: 1 to 64 of lowercase letters, digits, `-`, `_`, `.`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
}

/// The secret form of a key the service made.
pub fn secret_form(id: &str, key: &SigningKey) -> String {
    format!("{SECRET_PREFIX}:{id}:{}", b64url_encode(&key.to_bytes()))
}

/// Read a secret form back: the key id and the signing key.
pub fn parse_secret(secret: &str) -> Option<(String, SigningKey)> {
    let mut parts = secret.trim().splitn(3, ':');
    let (Some(SECRET_PREFIX), Some(id), Some(seed)) = (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let seed: [u8; 32] = b64url_decode(seed).ok()?.try_into().ok()?;
    Some((id.to_string(), SigningKey::from_bytes(&seed)))
}

/// Sign a challenge as a machine does: the signature over the challenge's
/// bytes, base64url.
pub fn sign_challenge(key: &SigningKey, challenge: &str) -> Option<String> {
    let bytes = b64url_decode(challenge).ok()?;
    Some(b64url_encode(&key.sign(&bytes).to_bytes()))
}

/// What making a key returns.
#[derive(Debug, Clone)]
pub struct Made {
    pub id: String,
    pub name: String,
    /// `ordo-mk-v1:…`, when the service made the pair. Shown once.
    pub secret: Option<String>,
}

/// Make a machine key inside `tx`. `public` is a key the machine made;
/// `None` makes a pair here.
pub async fn add(
    tx: &TxConn,
    handle: &str,
    keys: &KeyRing,
    name: &str,
    public: Option<[u8; 32]>,
    by: Actor<'_>,
    now: u64,
) -> Result<Made, IdentityError> {
    if !valid_name(name) {
        return Err(IdentityError::BadRequest(
            "a machine key's name is 1 to 64 lowercase letters, digits, `-`, `_` or `.`".into(),
        ));
    }
    let taken = tx
        .rows(&format!(
            "SELECT 1 FROM machine_keys WHERE {} AND name = {} AND ended_at IS NULL",
            dep(handle),
            sql_lit(name)
        ))
        .await?;
    if !taken.is_empty() {
        return Err(IdentityError::NameTaken(name.to_string()));
    }
    let id = mint_id("m_");
    let (public, secret) = match public {
        Some(p) => {
            VerifyingKey::from_bytes(&p).map_err(|_| {
                IdentityError::BadRequest("`publicKey` is not an Ed25519 public key".into())
            })?;
            (p, None)
        }
        None => {
            let key = SigningKey::from_bytes(&random_bytes::<32>());
            (key.verifying_key().to_bytes(), Some(secret_form(&id, &key)))
        }
    };
    tx.rows(&format!(
        "INSERT INTO machine_keys (id, deployment, name, public_key, created_by, created_at) \
         VALUES ({}, {}, {}, {}, {}, {now})",
        sql_lit(&id),
        sql_lit(handle),
        sql_lit(name),
        sql_lit(&b64url_encode(&public)),
        sql_lit(by.id),
    ))
    .await?;
    audit::append(
        tx,
        handle,
        keys,
        &Entry {
            actor: by.id,
            action: "machine-key.made",
            subject: &id,
            outcome: "ok",
            client: by.client,
            detail: name,
        },
        now,
    )
    .await?;
    Ok(Made {
        id,
        name: name.to_string(),
        secret,
    })
}

/// End a machine key: new sign-ins refuse at once, its sessions end, its
/// tokens die within the token life.
pub async fn end(
    pool: &Pool,
    handle: &str,
    keys: &KeyRing,
    id: &str,
    by: Actor<'_>,
    now: u64,
) -> Result<u64, IdentityError> {
    let tx = pool.begin().await?;
    let done = tx
        .rows(&format!(
            "UPDATE machine_keys SET ended_at = {now}, ended_by = {} WHERE {} AND id = {} \
             AND ended_at IS NULL RETURNING name",
            sql_lit(by.id),
            dep(handle),
            sql_lit(id)
        ))
        .await?;
    let Some(r) = done.first() else {
        return Err(IdentityError::UnknownMachineKey);
    };
    let ended = sessions::end_all_of(&tx, handle, id, "machine-key-ended", now).await?;
    audit::append(
        &tx,
        handle,
        keys,
        &Entry {
            actor: by.id,
            action: "machine-key.ended",
            subject: id,
            outcome: "ok",
            client: by.client,
            detail: &format!("{} sessions={ended}", text(r, 0)),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(ended)
}

/// Every machine key, without key material but the public half.
pub async fn list(pool: &Pool, handle: &str) -> Result<Vec<Value>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT id, name, public_key, created_by, created_at, last_used_at, ended_at \
             FROM machine_keys WHERE {} ORDER BY created_at, id",
            dep(handle)
        ))
        .await?
        .iter()
        .map(|r| {
            json!({
                "id": text(r, 0),
                "name": text(r, 1),
                "publicKey": text(r, 2),
                "createdBy": text(r, 3),
                "createdAt": num(r, 4),
                "lastUsedAt": num(r, 5),
                "endedAt": num(r, 6),
            })
        })
        .collect())
}

/// Whether any machine key, live or ended, has ever had `name`.
pub async fn ever_named(tx: &TxConn, handle: &str, name: &str) -> Result<bool, IdentityError> {
    Ok(!tx
        .rows(&format!(
            "SELECT 1 FROM machine_keys WHERE {} AND name = {}",
            dep(handle),
            sql_lit(name)
        ))
        .await?
        .is_empty())
}

/// The sign-in half: challenges and the spent list.
pub struct MachineSignIn {
    challenge_key: [u8; 32],
    /// Challenges spent, by nonce, until they expire.
    spent: Mutex<HashMap<[u8; 16], u64>>,
}

impl Default for MachineSignIn {
    fn default() -> Self {
        MachineSignIn {
            challenge_key: random_bytes::<32>(),
            spent: Mutex::new(HashMap::new()),
        }
    }
}

fn mac(key: &[u8; 32], machine: &str, nonce: &[u8], expires: &[u8]) -> [u8; 32] {
    let mut msg = Vec::new();
    msg.extend_from_slice(b"ordo-identity/machine-challenge/v1\0");
    msg.extend_from_slice(&(machine.len() as u64).to_be_bytes());
    msg.extend_from_slice(machine.as_bytes());
    msg.extend_from_slice(nonce);
    msg.extend_from_slice(expires);
    ordo_crypto::hmac_sha256(key, &msg)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// What a machine sign-in opened.
#[derive(Debug, Clone)]
pub struct SignedIn {
    pub id: String,
    pub name: String,
    pub session: Opened,
}

impl MachineSignIn {
    /// A challenge for `machine`. Minted for any id, known or not, so the
    /// answer says nothing about which keys exist.
    pub fn begin(&self, machine: &str, now: u64) -> String {
        let nonce = random_bytes::<16>();
        let expires = now.saturating_add(CHALLENGE_SECS).to_be_bytes();
        let m = mac(&self.challenge_key, machine, &nonce, &expires);
        let mut bytes = Vec::with_capacity(CHALLENGE_LEN);
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&expires);
        bytes.extend_from_slice(&m);
        b64url_encode(&bytes)
    }

    /// Check a signed challenge and open a machine session.
    #[allow(clippy::too_many_arguments)]
    pub async fn complete(
        &self,
        pool: &Pool,
        handle: &str,
        keys: &KeyRing,
        life: Lifetimes,
        machine: &str,
        challenge: &str,
        signature: &str,
        now: u64,
    ) -> Result<SignedIn, IdentityError> {
        let refused = || IdentityError::MachineRefused;
        let bytes = b64url_decode(challenge)
            .ok()
            .filter(|b| b.len() == CHALLENGE_LEN)
            .ok_or_else(refused)?;
        let (nonce, rest) = bytes.split_at(16);
        let (expires, m) = rest.split_at(8);
        if !ct_eq(&mac(&self.challenge_key, machine, nonce, expires), m) {
            return Err(refused());
        }
        let expires_at = u64::from_be_bytes(expires.try_into().map_err(|_| refused())?);
        if expires_at <= now {
            return Err(refused());
        }
        let rows = pool
            .rows(&format!(
                "SELECT name, public_key, last_used_at FROM machine_keys WHERE {} AND id = {} \
                 AND ended_at IS NULL",
                dep(handle),
                sql_lit(machine)
            ))
            .await?;
        let r = rows.first().ok_or_else(refused)?;
        let public: [u8; 32] = cell(r, 1)
            .and_then(|p| b64url_decode(p).ok())
            .and_then(|p| p.try_into().ok())
            .ok_or_else(refused)?;
        let verifying = VerifyingKey::from_bytes(&public).map_err(|_| refused())?;
        let sig: [u8; 64] = b64url_decode(signature)
            .ok()
            .and_then(|s| s.try_into().ok())
            .ok_or_else(refused)?;
        verifying
            .verify(&bytes, &Signature::from_bytes(&sig))
            .map_err(|_| refused())?;
        {
            let mut spent = self.spent.lock().unwrap_or_else(|e| e.into_inner());
            spent.retain(|_, until| *until > now);
            let n: [u8; 16] = nonce.try_into().map_err(|_| refused())?;
            if spent.insert(n, expires_at).is_some() {
                return Err(refused());
            }
        }
        let name = text(r, 0);
        let first_use = cell(r, 2).is_none();
        let tx = pool.begin().await?;
        tx.rows(&format!(
            "UPDATE machine_keys SET last_used_at = {now} WHERE {} AND id = {}",
            dep(handle),
            sql_lit(machine)
        ))
        .await?;
        if first_use {
            audit::append(
                &tx,
                handle,
                keys,
                &Entry {
                    actor: machine,
                    action: "machine-key.first-used",
                    subject: machine,
                    outcome: "ok",
                    client: &name,
                    detail: "",
                },
                now,
            )
            .await?;
        }
        let session = sessions::open(
            &tx,
            handle,
            keys,
            life,
            NewSession {
                subject: machine,
                kind: SessionKind::Machine,
                client: &name,
                touched_at: None,
                must_enrol: false,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(SignedIn {
            id: machine.to_string(),
            name,
            session,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_secret_form_round_trips_and_names_are_checked() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let s = secret_form("m_abc", &key);
        assert!(s.starts_with("ordo-mk-v1:m_abc:"));
        let (id, back) = parse_secret(&s).unwrap();
        assert_eq!(id, "m_abc");
        assert_eq!(back.to_bytes(), key.to_bytes());
        assert!(parse_secret("ordo-mk-v2:m_abc:AAAA").is_none());
        assert!(valid_name("ci-main"));
        assert!(!valid_name("CI main"));
        assert!(!valid_name(""));
    }
}
