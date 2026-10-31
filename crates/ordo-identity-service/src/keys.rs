//! Signing keys: made, sealed, published, rotated
//! (`plan-identity-service.md` 4.2).
//!
//! Two kinds of Ed25519 pair, each in `signing_keys` with its private half
//! sealed with `ORDO_IDENTITY_KEY_FILE`:
//!
//! - **token** keys sign deployment tokens. Their public halves are the
//!   JWKS at `/.well-known/jwks.json`. They rotate.
//! - **audit** keys sign audit checkpoints. Published apart, at
//!   `/.well-known/ordo-audit-keys.json`, so a key that signs checkpoints is
//!   never a key a token verifier accepts. One per deployment, kept: a
//!   checkpoint key is the deployment's history identity, and rotating it
//!   only makes old checkpoints harder to check (the reasoning of
//!   `ordo_commandlog::sign`).
//!
//! Rotation of the token key, every `ORDO_IDENTITY_KEY_ROTATE_DAYS` or by
//! `rotate-keys`:
//!
//! 1. A new pair is published at once and signs from [`ACTIVATE_AFTER_SECS`]
//!    later, so every verifier's cache (10 minutes) has it first.
//! 2. The pair it replaces keeps verifying until 24 hours after the last
//!    token it signed expired ([`RETIRE_AFTER_SECS`] past the new key's
//!    start plus the token life), then is retired. A retired key is out of
//!    the JWKS and verifies nothing.
//!
//! `rotate-keys --now` (compromise) signs with the new pair at once and
//! retires every other token key at once.
//!
//! A key's id is its RFC 7638 thumbprint
//! (`ordo_identity::deployment_token::key_id`).

use std::sync::RwLock;

use ed25519_dalek::SigningKey;
use ordo_commandlog::Ed25519Signer;
use ordo_exec::{sql_lit, Pool, TxConn};
use ordo_identity::deployment_token::{key_id, public_jwk};
use ordo_identity::sealing::SealingKey;
use ordo_identity::webauthn::b64url_encode;
use serde_json::{json, Value};

use crate::audit::{self, Entry};
use crate::config::KEY_FILE_ENV;
use crate::error::IdentityError;
use crate::secrets::random_bytes;
use crate::store::{cell, dep, num, text};

/// A new token key signs this long after it is published.
pub const ACTIVATE_AFTER_SECS: u64 = 3600;
/// A replaced token key stays published this long after its last token
/// expired.
pub const RETIRE_AFTER_SECS: u64 = 24 * 3600;

/// What a pair is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Token,
    Audit,
}

impl Purpose {
    fn as_str(self) -> &'static str {
        match self {
            Purpose::Token => "token",
            Purpose::Audit => "audit",
        }
    }

    /// The seal's entity word: a key sealed as one purpose does not open
    /// as the other.
    fn entity(self) -> &'static str {
        match self {
            Purpose::Token => "identity_token_key",
            Purpose::Audit => "identity_audit_key",
        }
    }
}

/// A live pair: not retired.
#[derive(Clone)]
struct Pair {
    kid: String,
    purpose: Purpose,
    signing: SigningKey,
    created_at: u64,
    active_from: u64,
}

/// The deployment's live signing keys, held in memory, written through to
/// the database.
pub struct KeyRing {
    handle: String,
    sealing: SealingKey,
    rotate_days: u64,
    token_secs: u64,
    pairs: RwLock<Vec<Pair>>,
}

/// What a rotation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rotated {
    pub kid: String,
    pub active_from: u64,
    /// Keys retired by it: every other token key for `--now`, none
    /// otherwise.
    pub retired: Vec<String>,
}

impl KeyRing {
    /// Load the live keys and, at first boot, make the token key and the
    /// audit key. Then apply any rotation or retirement that is due.
    pub async fn boot(
        pool: &Pool,
        handle: &str,
        sealing: SealingKey,
        rotate_days: u64,
        token_secs: u64,
        now: u64,
    ) -> Result<KeyRing, IdentityError> {
        let ring = KeyRing {
            handle: handle.to_string(),
            sealing,
            rotate_days,
            token_secs,
            pairs: RwLock::new(Vec::new()),
        };
        ring.reload(pool).await?;
        // The audit key first: the rows below may reach a checkpoint.
        for purpose in [Purpose::Audit, Purpose::Token] {
            if !ring.held().iter().any(|p| p.purpose == purpose) {
                let tx = pool.begin().await?;
                ring.lock(&tx).await?;
                let pair = ring.insert(&tx, purpose, now, now).await?;
                ring.pairs_mut().push(pair.clone());
                audit::append(
                    &tx,
                    handle,
                    &ring,
                    &Entry {
                        actor: "boot",
                        action: "key.made",
                        subject: &pair.kid,
                        outcome: "ok",
                        client: "",
                        detail: purpose.as_str(),
                    },
                    now,
                )
                .await?;
                tx.commit().await?;
            }
        }
        ring.tick(pool, now).await?;
        Ok(ring)
    }

    fn held(&self) -> Vec<Pair> {
        self.pairs.read().map(|g| g.clone()).unwrap_or_default()
    }

    fn pairs_mut(&self) -> std::sync::RwLockWriteGuard<'_, Vec<Pair>> {
        // A poisoned lock means a panic mid-write; the list is still a
        // list, so take it.
        self.pairs.write().unwrap_or_else(|e| e.into_inner())
    }

    async fn lock(&self, tx: &TxConn) -> Result<(), IdentityError> {
        tx.rows(&format!(
            "SELECT pg_advisory_xact_lock(hashtext({}))",
            sql_lit(&format!("ordo-identity-keys:{}", self.handle))
        ))
        .await?;
        Ok(())
    }

    fn anchor(&self, kid: &str) -> String {
        format!("{}/{kid}", self.handle)
    }

    /// Read every live key from the database and open its private half.
    async fn reload(&self, pool: &Pool) -> Result<(), IdentityError> {
        let rows = pool
            .rows(&format!(
                "SELECT kid, purpose, sealed, created_at, active_from FROM signing_keys \
                 WHERE {} AND retired_at IS NULL ORDER BY active_from, created_at",
                dep(&self.handle)
            ))
            .await?;
        let mut pairs = Vec::with_capacity(rows.len());
        for r in &rows {
            let kid = text(r, 0);
            let purpose = match cell(r, 1) {
                Some("audit") => Purpose::Audit,
                _ => Purpose::Token,
            };
            let seed = self
                .sealing
                .open(
                    purpose.entity(),
                    &self.anchor(&kid),
                    cell(r, 2).unwrap_or(""),
                )
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
                .ok_or_else(|| {
                    IdentityError::setting(
                        KEY_FILE_ENV,
                        format!("does not open the {} signing key `{kid}`", purpose.as_str()),
                        format!(
                            "Point {KEY_FILE_ENV} at the key file this database's signing keys \
                             were sealed with (restore it from the backup made with the \
                             database). A new key file opens none of them"
                        ),
                    )
                })?;
            pairs.push(Pair {
                kid,
                purpose,
                signing: SigningKey::from_bytes(&seed),
                created_at: num(r, 3).unwrap_or(0),
                active_from: num(r, 4).unwrap_or(0),
            });
        }
        *self.pairs_mut() = pairs;
        Ok(())
    }

    /// Make and store a pair. The caller holds the key lock.
    async fn insert(
        &self,
        tx: &TxConn,
        purpose: Purpose,
        active_from: u64,
        now: u64,
    ) -> Result<Pair, IdentityError> {
        let signing = SigningKey::from_bytes(&random_bytes::<32>());
        let public = signing.verifying_key();
        let kid = key_id(&public);
        let sealed = self
            .sealing
            .seal(purpose.entity(), &self.anchor(&kid), &signing.to_bytes());
        tx.rows(&format!(
            "INSERT INTO signing_keys (deployment, kid, purpose, public_key, sealed, created_at, \
             active_from) VALUES ({}, {}, {}, {}, {}, {now}, {active_from})",
            sql_lit(&self.handle),
            sql_lit(&kid),
            sql_lit(purpose.as_str()),
            sql_lit(&b64url_encode(public.as_bytes())),
            sql_lit(&sealed),
        ))
        .await?;
        Ok(Pair {
            kid,
            purpose,
            signing,
            created_at: now,
            active_from,
        })
    }

    /// The token key that signs at `now`: the live one that started last.
    pub fn token_signer(&self, now: u64) -> Result<(String, SigningKey), IdentityError> {
        self.held()
            .into_iter()
            .filter(|p| p.purpose == Purpose::Token && p.active_from <= now)
            .max_by_key(|p| (p.active_from, p.created_at))
            .map(|p| (p.kid, p.signing))
            .ok_or_else(|| IdentityError::Internal("no token key is active".into()))
    }

    /// The audit key, as `ordo_commandlog`'s signer.
    pub fn audit_signer(&self) -> Result<Ed25519Signer, IdentityError> {
        self.held()
            .into_iter()
            .filter(|p| p.purpose == Purpose::Audit)
            .max_by_key(|p| (p.active_from, p.created_at))
            .map(|p| Ed25519Signer::from_seed(p.kid, &p.signing.to_bytes()))
            .ok_or_else(|| IdentityError::Internal("no audit key is held".into()))
    }

    /// The JWKS: every live token key, oldest first.
    pub fn jwks(&self) -> Value {
        let keys: Vec<Value> = self
            .held()
            .iter()
            .filter(|p| p.purpose == Purpose::Token)
            .map(|p| public_jwk(&p.kid, &p.signing.verifying_key()))
            .collect();
        json!({ "keys": keys })
    }

    /// The audit keys' public halves, for `audit verify` elsewhere.
    pub fn audit_keys(&self) -> Value {
        let keys: Vec<Value> = self
            .held()
            .iter()
            .filter(|p| p.purpose == Purpose::Audit)
            .map(|p| {
                json!({
                    "kid": p.kid,
                    "alg": "EdDSA",
                    "x": b64url_encode(p.signing.verifying_key().as_bytes()),
                })
            })
            .collect();
        json!({ "keys": keys })
    }

    /// Apply what is due at `now`: a scheduled rotation, and the
    /// retirement of keys whose last tokens expired a day ago.
    pub async fn tick(&self, pool: &Pool, now: u64) -> Result<(), IdentityError> {
        let tokens: Vec<Pair> = self
            .held()
            .into_iter()
            .filter(|p| p.purpose == Purpose::Token)
            .collect();
        let due_rotation = self.rotate_days > 0
            && tokens
                .iter()
                .map(|p| p.created_at)
                .max()
                .is_some_and(|newest| now >= newest + self.rotate_days * 86_400);
        if due_rotation {
            self.rotate(pool, now, false, "service").await?;
        }
        let tokens: Vec<Pair> = self
            .held()
            .into_iter()
            .filter(|p| p.purpose == Purpose::Token)
            .collect();
        let retire: Vec<String> = tokens
            .iter()
            .filter(|old| {
                // The key that replaced it: the first to start after it.
                tokens
                    .iter()
                    .filter(|n| n.active_from > old.active_from)
                    .map(|n| n.active_from)
                    .min()
                    .is_some_and(|start| now >= start + self.token_secs + RETIRE_AFTER_SECS)
            })
            .map(|p| p.kid.clone())
            .collect();
        if !retire.is_empty() {
            let tx = pool.begin().await?;
            self.lock(&tx).await?;
            for kid in &retire {
                self.retire(&tx, kid, now, "service", "replaced").await?;
            }
            tx.commit().await?;
            self.pairs_mut().retain(|p| !retire.contains(&p.kid));
        }
        Ok(())
    }

    async fn retire(
        &self,
        tx: &TxConn,
        kid: &str,
        now: u64,
        actor: &str,
        why: &str,
    ) -> Result<(), IdentityError> {
        tx.rows(&format!(
            "UPDATE signing_keys SET retired_at = {now} WHERE {} AND kid = {} \
             AND retired_at IS NULL",
            dep(&self.handle),
            sql_lit(kid)
        ))
        .await?;
        audit::append(
            tx,
            &self.handle,
            self,
            &Entry {
                actor,
                action: "key.retired",
                subject: kid,
                outcome: "ok",
                client: "",
                detail: why,
            },
            now,
        )
        .await?;
        Ok(())
    }

    /// Add a new token key. `immediate` (compromise) signs with it at once and
    /// retires every other token key; otherwise it signs from
    /// [`ACTIVATE_AFTER_SECS`] later and the old key retires on schedule.
    pub async fn rotate(
        &self,
        pool: &Pool,
        now: u64,
        immediate: bool,
        actor: &str,
    ) -> Result<Rotated, IdentityError> {
        let tx = pool.begin().await?;
        self.lock(&tx).await?;
        let active_from = if immediate {
            now
        } else {
            now + ACTIVATE_AFTER_SECS
        };
        let pair = self.insert(&tx, Purpose::Token, active_from, now).await?;
        let retired: Vec<String> = if immediate {
            self.held()
                .iter()
                .filter(|p| p.purpose == Purpose::Token)
                .map(|p| p.kid.clone())
                .collect()
        } else {
            Vec::new()
        };
        audit::append(
            &tx,
            &self.handle,
            self,
            &Entry {
                actor,
                action: "key.rotated",
                subject: &pair.kid,
                outcome: "ok",
                client: "",
                detail: if immediate { "now" } else { "scheduled" },
            },
            now,
        )
        .await?;
        for kid in &retired {
            self.retire(&tx, kid, now, actor, "rotated-now").await?;
        }
        tx.commit().await?;
        {
            let mut held = self.pairs_mut();
            held.retain(|p| !retired.contains(&p.kid));
            held.push(pair.clone());
        }
        Ok(Rotated {
            kid: pair.kid,
            active_from,
            retired,
        })
    }
}

/// Every token key ever made, public halves only: for tests and a later
/// `keys` command.
pub async fn all_token_kids(
    pool: &Pool,
    handle: &str,
) -> Result<Vec<(String, bool)>, IdentityError> {
    Ok(pool
        .rows(&format!(
            "SELECT kid, retired_at IS NOT NULL FROM signing_keys WHERE {} AND purpose = 'token' \
             ORDER BY created_at, active_from",
            dep(handle)
        ))
        .await?
        .iter()
        .map(|r| (text(r, 0), cell(r, 1) == Some("t")))
        .collect())
}
