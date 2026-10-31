//! The console's link to its deployment (`plan-console.md` 8, G14 and
//! G15): the console key, the short session it signs in for, and the
//! owner's touch relayed for writes.
//!
//! # Linking (8.2)
//!
//! 1. The page asks [`DoorLink::link_begin`] for the deployment's passkey
//!    options: an assertion, for an owner who already has a passkey there
//!    and whom the deployment named (`ORDO_DEPLOYMENT_OWNERS`). The claim
//!    step, which registered a passkey and claimed an ownerless deployment
//!    with its claim code, was removed with the claim code (owner,
//!    2026-10-30); `claim` is refused naming the settings that replace it.
//! 2. The owner touches their passkey in the console page, for the
//!    deployment's relying party (Related Origin Requests,
//!    `ORDO_AUTH_RELATED_ORIGINS`, or a shared domain).
//! 3. [`DoorLink::link_finish`] hands the credential to the door's
//!    `/auth/complete`, naming the deployment's own space. The answer is
//!    the owner's session, with presence.
//! 4. The console makes an Ed25519 pair, enrols its public half with
//!    `std.deployment.addConsoleKey` under that session, seals the private
//!    half in its own database, signs the owner's session out at the door,
//!    and signs in once with the new key to prove it.
//!
//! The owner's own session is never kept: the console holds the key and
//! nothing else.
//!
//! # Using it
//!
//! [`DoorLink::token`] answers a console key session, signing in again when
//! the one held has less than a minute left, unless the owner touched for
//! it in the last 300 seconds: that one is kept to its end. It carries no
//! presence. A write that needs a touch is preceded by
//! [`DoorLink::step_up_begin`] and [`DoorLink::step_up_finish`], which
//! relay the owner's passkey assertion for the one session the begin named
//! (`session`); the door marks it present for 300 seconds.
//!
//! # Rotating and unlinking
//!
//! Both need the session present (a relayed touch in the last 300 seconds):
//! a rotation enrols a new key with it and revokes the old one; an unlink
//! revokes the key and forgets it here.

use std::sync::Arc;

use ed25519_dalek::{Signer as _, SigningKey};
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::codes::Clock;
use crate::door::{DoorClient, DoorError};
use crate::error::ConsoleError;
use crate::sealing::ConsoleKey;
use crate::store::{CredentialRow, Store};

/// What a console signs before the challenge (the door's
/// `console_keys::SIGNED_PREFIX`).
pub const SIGNED_PREFIX: &[u8] = b"ordo-console-key/v1\0";
/// A session with less than this left is replaced before use.
pub const RENEW_MARGIN_SECS: u64 = 60;
/// How long the door keeps a session present after a touch.
pub const PRESENCE_SECS: u64 = 300;

const ADD: &str = "std.deployment.addConsoleKey";
const REVOKE: &str = "std.deployment.revokeConsoleKey";
const LIST: &str = "std.deployment.consoleKeys";
/// Seconds in a day, for the rotation schedule.
const DAY_SECS: u64 = 24 * 60 * 60;

/// How the owner proves themselves at the deployment while linking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// The owner already holds a passkey at the deployment.
    SignIn,
}

/// How a console without the identity service gets its deployment's
/// owner, said wherever a link cannot be made: the claim step is gone.
pub const HOW_TO_LINK: &str = "a console links to its deployment by sign-in, as an owner \
     the deployment named: register a passkey on the deployment's own sign-in page, set \
     ORDO_DEPLOYMENT_OWNERS to that principal id and restart it, then link with `sign-in`. \
     Or move sign-in to the identity service, which needs no link: set \
     ORDO_IDENTITY_DATABASE_URL (or ORDO_IDENTITY_DATABASE_URL_FILE), or set \
     ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true";

impl LinkKind {
    pub fn parse(word: &str) -> Result<LinkKind, ConsoleError> {
        match word {
            "sign-in" => Ok(LinkKind::SignIn),
            "claim" => Err(ConsoleError::BadRequest(format!(
                "the claim step was removed with the claim code (2026-10-30). Now {HOW_TO_LINK}"
            ))),
            other => Err(ConsoleError::BadRequest(format!(
                "`kind` is `{other}`; use `sign-in`"
            ))),
        }
    }
}

/// The session held.
#[derive(Clone)]
struct Held {
    key_id: String,
    /// This console's own name for the session, minted at sign-in. The page
    /// carries it from a touch's begin to its finish.
    lease: String,
    token: String,
    expires_at: u64,
    present_until: Option<u64>,
}

impl Held {
    fn present(&self, now: u64) -> bool {
        self.present_until.is_some_and(|t| t > now) && self.expires_at > now
    }
}

/// The session in use, and the one a touch was begun for when a renewal
/// has replaced it since.
#[derive(Default)]
struct Sessions {
    held: Option<Held>,
    touching: Option<Held>,
}

fn mint_lease() -> String {
    let mut b = [0u8; 12];
    getrandom::getrandom(&mut b).expect("the operating system CSPRNG is unavailable");
    b64url_encode(&b)
}

/// The link to one deployment. Cheap to clone.
#[derive(Clone)]
pub struct DoorLink {
    dep: String,
    space: String,
    client: DoorClient,
    store: Store,
    key: ConsoleKey,
    clock: Clock,
    held: Arc<Mutex<Sessions>>,
    /// `ORDO_CONSOLE_KEY_ROTATE_DAYS`.
    rotate_days: u64,
}

impl std::fmt::Debug for DoorLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DoorLink({}, {:?})", self.dep, self.client)
    }
}

fn signing_key(bytes: &[u8]) -> Result<SigningKey, ConsoleError> {
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| ConsoleError::Internal("the stored console key is not 32 bytes".into()))?;
    Ok(SigningKey::from_bytes(&seed))
}

fn new_pair() -> Result<SigningKey, ConsoleError> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| {
        ConsoleError::Internal(format!("the operating system gave no randomness: {e}"))
    })?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Sign a door challenge (base64url) with the prefix.
pub fn sign_challenge(key: &SigningKey, challenge: &str) -> Result<String, ConsoleError> {
    let bytes = b64url_decode(challenge).map_err(|_| {
        ConsoleError::Door(DoorError::Failed {
            status: 200,
            message: "the door's challenge is not base64url".into(),
        })
    })?;
    let mut msg = SIGNED_PREFIX.to_vec();
    msg.extend_from_slice(&bytes);
    Ok(b64url_encode(&key.sign(&msg).to_bytes()))
}

fn text(v: &Value, field: &str) -> Result<String, ConsoleError> {
    v.get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            ConsoleError::Door(DoorError::Failed {
                status: 200,
                message: format!("the door's answer has no `{field}`"),
            })
        })
}

impl DoorLink {
    pub fn new(
        dep: &str,
        space: &str,
        client: DoorClient,
        store: Store,
        key: ConsoleKey,
        clock: Clock,
    ) -> DoorLink {
        DoorLink {
            dep: dep.to_string(),
            space: space.to_string(),
            client,
            store,
            key,
            clock,
            held: Arc::new(Mutex::new(Sessions::default())),
            rotate_days: crate::config::DEFAULT_KEY_ROTATE_DAYS,
        }
    }

    /// The rotation schedule, `ORDO_CONSOLE_KEY_ROTATE_DAYS`.
    pub fn with_rotation_days(mut self, days: u64) -> DoorLink {
        self.rotate_days = days;
        self
    }

    pub fn client(&self) -> &DoorClient {
        &self.client
    }

    /// Where this console stands with the deployment. Says nothing secret.
    pub async fn status(&self) -> Result<Value, ConsoleError> {
        let held = self.held.lock().await.held.clone();
        let now = self.clock.now();
        Ok(match self.store.credential(&self.dep).await? {
            None => json!({
                "linked": false,
                "door": self.client.url().display(),
                "space": self.space,
                "howToLink": HOW_TO_LINK,
            }),
            Some(c) => json!({
                "linked": true,
                "door": self.client.url().display(),
                "space": c.space,
                "key": c.key_id,
                "name": c.name,
                "linkedAt": c.created_at,
                "rotatedAt": c.rotated_at,
                "sessionUntil": held.as_ref().map(|h| h.expires_at),
                "presentUntil": held.and_then(|h| h.present_until),
                "rotateDays": self.rotate_days,
                "rotateDueAt": self.rotate_due_at(&c),
                "rotateDue": now >= self.rotate_due_at(&c),
            }),
        })
    }

    /// When the key held is due for rotation: its enrolment plus the
    /// schedule.
    fn rotate_due_at(&self, c: &CredentialRow) -> u64 {
        c.created_at
            .saturating_add(self.rotate_days.saturating_mul(DAY_SECS))
    }

    async fn credential(&self) -> Result<(CredentialRow, SigningKey), ConsoleError> {
        let row = self
            .store
            .credential(&self.dep)
            .await?
            .ok_or(ConsoleError::NotLinked)?;
        let plain = self
            .key
            .open_credential(&self.dep, &row.key_id, &row.sealed)?;
        let sk = signing_key(&plain)?;
        Ok((row, sk))
    }

    /// Sign in with a key: the door's challenge, signed, for a session.
    async fn sign_in_with(&self, key_id: &str, sk: &SigningKey) -> Result<Held, ConsoleError> {
        self.sign_in_raw(key_id, sk).await.map_err(|e| match e {
            // The door's one sign-in refusal: the key is revoked or unknown.
            ConsoleError::Door(DoorError::Refused { status: 401, .. }) => ConsoleError::KeyRefused,
            other => other,
        })
    }

    async fn sign_in_raw(&self, key_id: &str, sk: &SigningKey) -> Result<Held, ConsoleError> {
        let begun = self
            .client
            .post(
                "/auth/begin",
                None,
                None,
                &json!({"method": "console", "key": key_id}),
            )
            .await?;
        let challenge = text(&begun, "challenge")?;
        let signature = sign_challenge(sk, &challenge)?;
        let done = self
            .client
            .post(
                "/auth/complete",
                None,
                None,
                &json!({"method": "console", "key": key_id, "challenge": challenge,
                        "signature": signature}),
            )
            .await?;
        Ok(Held {
            key_id: key_id.to_string(),
            lease: mint_lease(),
            token: text(&done, "token")?,
            expires_at: done.get("expires_at").and_then(Value::as_u64).unwrap_or(0),
            present_until: None,
        })
    }

    /// A console key session, signed in afresh when the one held is close
    /// to its end or belongs to a key no longer stored. A session the owner
    /// touched for is kept to its end: a renewal would drop the touch.
    pub async fn token(&self) -> Result<String, ConsoleError> {
        Ok(self.session().await?.token)
    }

    async fn session(&self) -> Result<Held, ConsoleError> {
        let now = self.clock.now();
        let mut s = self.held.lock().await;
        let (row, sk) = self.credential().await?;
        if let Some(h) = s.held.as_ref() {
            if h.key_id == row.key_id && (h.expires_at > now + RENEW_MARGIN_SECS || h.present(now))
            {
                return Ok(h.clone());
            }
        }
        let fresh = self.sign_in_with(&row.key_id, &sk).await?;
        s.held = Some(fresh.clone());
        Ok(fresh)
    }

    /// A session the owner touched for in the last 300 seconds, or the
    /// refusal that asks for a touch.
    async fn present_token(&self) -> Result<String, ConsoleError> {
        let now = self.clock.now();
        let s = self.held.lock().await;
        match s.held.as_ref() {
            Some(h) if h.present(now) => Ok(h.token.clone()),
            _ => Err(ConsoleError::DeploymentTouchNeeded {
                window_secs: PRESENCE_SECS,
            }),
        }
    }

    /// This link, calling as the console key (G14). Every method below
    /// that has no caller of its own uses it.
    pub fn as_key(&self) -> DoorAs<'_> {
        DoorAs {
            link: self,
            caller: Caller::ConsoleKey,
        }
    }

    /// This link, calling as a deployment person with the token the
    /// identity service gave them (`plan-identity-service.md` 5.4 step 5).
    /// The console key is never used on this path.
    pub fn as_person(&self, token: &str) -> DoorAs<'_> {
        DoorAs {
            link: self,
            caller: Caller::Person(token.to_string()),
        }
    }

    /// Call a platform op in the deployment's own space as the owner.
    pub async fn op(
        &self,
        name: &str,
        read: bool,
        params: &Value,
    ) -> Result<Vec<Value>, ConsoleError> {
        self.as_key().op(name, read, params).await
    }

    /// The same, with the owner's relayed touch: for a write that needs
    /// presence.
    pub async fn op_present(&self, name: &str, params: &Value) -> Result<Vec<Value>, ConsoleError> {
        self.as_key().op_present(name, params).await
    }

    /// [`DoorLink::op_present`], allowed `timeout`.
    pub async fn op_present_within(
        &self,
        name: &str,
        params: &Value,
        timeout: std::time::Duration,
    ) -> Result<Vec<Value>, ConsoleError> {
        self.as_key().op_present_within(name, params, timeout).await
    }

    /// A write in a tenant's own space, with the owner's relayed touch.
    pub async fn op_present_in(
        &self,
        space: &str,
        name: &str,
        params: &Value,
    ) -> Result<Vec<Value>, ConsoleError> {
        self.as_key().op_present_in(space, name, params).await
    }

    /// A read in a tenant's own space, with the owner's relayed touch.
    pub async fn read_present_in(
        &self,
        space: &str,
        name: &str,
        params: &Value,
    ) -> Result<Vec<Value>, ConsoleError> {
        self.as_key().read_present_in(space, name, params).await
    }

    /// One page of a tenant's own security record.
    pub async fn tenant_events(
        &self,
        space: &str,
        from: u64,
        limit: u64,
    ) -> Result<Value, ConsoleError> {
        self.as_key().tenant_events(space, from, limit).await
    }

    /// One space's usage records from `GET /usage`.
    pub async fn usage(&self, space: &str, from: u64, to: u64) -> Result<Vec<Value>, ConsoleError> {
        self.as_key().usage(space, from, to).await
    }

    /// The door's readiness answer.
    pub async fn ready(&self) -> Result<Value, ConsoleError> {
        self.as_key().ready().await
    }
    async fn linked_space(&self) -> Result<String, ConsoleError> {
        Ok(self
            .store
            .credential(&self.dep)
            .await?
            .map(|c| c.space)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.space.clone()))
    }

    /// G15, leg one: the door's passkey options for this console session.
    /// The answer carries `session`, which the page hands back to
    /// [`DoorLink::step_up_finish`]: the door binds the challenge to this
    /// one session, so the finish goes to it even if a renewal has replaced
    /// it meanwhile.
    pub async fn step_up_begin(&self) -> Result<Value, ConsoleError> {
        let held = self.session().await?;
        let mut options = self
            .client
            .post("/auth/step-up/begin", Some(&held.token), None, &json!({}))
            .await?;
        if let Value::Object(m) = &mut options {
            m.insert("session".into(), json!(held.lease));
        }
        self.held.lock().await.touching = Some(held);
        Ok(options)
    }

    /// G15, leg two: the owner's assertion, made in the page, handed to the
    /// door for the session `session` named at the begin. That session
    /// comes back present and is the one held from then on.
    pub async fn step_up_finish(
        &self,
        assertion: &Value,
        session: &str,
    ) -> Result<Value, ConsoleError> {
        let now = self.clock.now();
        let mut s = self.held.lock().await;
        let begun = [s.held.as_ref(), s.touching.as_ref()]
            .into_iter()
            .flatten()
            .find(|h| h.lease == session && h.expires_at > now)
            .cloned();
        let Some(mut h) = begun else {
            return Err(ConsoleError::DeploymentTouchNeeded {
                window_secs: PRESENCE_SECS,
            });
        };
        let done = self
            .client
            .post("/auth/step-up", Some(&h.token), None, assertion)
            .await?;
        h.token = text(&done, "token")?;
        if let Some(e) = done.get("expires_at").and_then(Value::as_u64) {
            h.expires_at = e;
        }
        // A quarantined passkey (a recovery added it) gives no presence yet.
        if done.get("presence_available_at").is_none() {
            h.present_until = Some(now + PRESENCE_SECS);
        }
        let answer = json!({
            "presentUntil": h.present_until,
            "sessionUntil": h.expires_at,
            "presenceAvailableAt": done.get("presence_available_at"),
        });
        s.touching = None;
        s.held = Some(h);
        Ok(answer)
    }

    /// The console keys the deployment holds (`std.deployment.consoleKeys`),
    /// each marked when it is this console's own.
    pub async fn keys(&self) -> Result<Vec<Value>, ConsoleError> {
        let own = self.store.credential(&self.dep).await?.map(|c| c.key_id);
        let mut rows = self.op(LIST, true, &json!({})).await?;
        for r in &mut rows {
            let mine = r.get("id").and_then(Value::as_str) == own.as_deref();
            r["thisConsole"] = json!(mine);
        }
        Ok(rows)
    }

    /// Revoke another console key at the deployment, with the owner's
    /// relayed touch. This console's own key is unlinked instead.
    pub async fn revoke_key(&self, id: &str) -> Result<Value, ConsoleError> {
        let (row, _) = self.credential().await?;
        if row.key_id == id {
            return Err(ConsoleError::BadRequest(
                "that is this console's own key; unlink it instead".into(),
            ));
        }
        let rows = self.op_present(REVOKE, &json!({"id": id})).await?;
        Ok(rows.into_iter().next().unwrap_or(Value::Null))
    }

    /// Forget the link here when the deployment refuses the key (revoked
    /// there). While the deployment still takes the key, unlink instead.
    pub async fn forget(&self) -> Result<String, ConsoleError> {
        let (row, sk) = self.credential().await?;
        match self.sign_in_with(&row.key_id, &sk).await {
            Err(ConsoleError::KeyRefused) => {
                self.store.delete_credential(&self.dep).await?;
                *self.held.lock().await = Sessions::default();
                Ok(row.key_id)
            }
            Err(other) => Err(other),
            Ok(_) => Err(ConsoleError::BadRequest(
                "the deployment still accepts this console's key; unlink it instead".into(),
            )),
        }
    }

    /// Health from the door as the console key; see [`DoorAs::health`].
    pub async fn health(&self) -> Value {
        self.as_key().health().await
    }
    /// Linking, leg one: the deployment's passkey options.
    pub async fn link_begin(&self, kind: LinkKind) -> Result<Value, ConsoleError> {
        if self.store.credential(&self.dep).await?.is_some() {
            return Err(ConsoleError::AlreadyLinked);
        }
        Ok(match kind {
            LinkKind::SignIn => {
                self.client
                    .post("/auth/begin", None, None, &json!({"method": "passkey"}))
                    .await?
            }
        })
    }

    /// Linking, leg two: the owner's credential, then the console key.
    /// `credential` is the door's `/auth/complete` passkey body as the page
    /// made it. Answers the new key's id.
    pub async fn link_finish(
        &self,
        _kind: LinkKind,
        credential: &Value,
        name: &str,
    ) -> Result<String, ConsoleError> {
        if self.store.credential(&self.dep).await?.is_some() {
            return Err(ConsoleError::AlreadyLinked);
        }
        let Value::Object(mut body) = credential.clone() else {
            return Err(ConsoleError::BadRequest(
                "`credential` must be the passkey body".into(),
            ));
        };
        body.insert("method".into(), json!("passkey"));
        body.insert("space".into(), json!(self.space));
        let owner = self
            .client
            .post("/auth/complete", None, None, &Value::Object(body))
            .await?;
        let owner_token = text(&owner, "token")?;
        let outcome = owner
            .pointer("/admission/outcome")
            .and_then(Value::as_str)
            .unwrap_or("");
        let result = if outcome == "admitted" || outcome == "already-member" {
            self.enrol(&owner_token, name).await
        } else {
            Err(ConsoleError::Door(DoorError::Refused {
                status: 403,
                code: "not_the_owner".into(),
                message: format!(
                    "the deployment did not admit this passkey to its own space ({outcome}): \
                     it is not an owner the deployment named. {HOW_TO_LINK}"
                ),
            }))
        };
        // The owner's own session is never kept, whatever happened.
        let _ = self
            .client
            .post("/auth/logout", Some(&owner_token), None, &json!({}))
            .await;
        result
    }

    /// Make a key pair, enrol it under `token` (which must be present),
    /// store it, and prove it by signing in.
    async fn enrol(&self, token: &str, name: &str) -> Result<String, ConsoleError> {
        let sk = new_pair()?;
        let rows = self
            .client
            .op(
                ADD,
                &self.space,
                token,
                false,
                &json!({"publicKey": b64url_encode(sk.verifying_key().as_bytes()), "name": name}),
            )
            .await?;
        let key_id = rows
            .first()
            .and_then(|r| r.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ConsoleError::Door(DoorError::Failed {
                    status: 200,
                    message: format!("{ADD} answered no key id"),
                })
            })?
            .to_string();
        let now = self.clock.now();
        let row = CredentialRow {
            sealed: self.key.seal_credential(&self.dep, &key_id, sk.as_bytes()),
            key_id: key_id.clone(),
            name: name.to_string(),
            space: self.space.clone(),
            created_at: now,
            rotated_at: None,
        };
        self.store.put_credential(&self.dep, &row, now).await?;
        let held = self.sign_in_with(&key_id, &sk).await?;
        *self.held.lock().await = Sessions {
            held: Some(held),
            touching: None,
        };
        Ok(key_id)
    }

    /// Rotate: enrol a new key with the present session, revoke the old
    /// one. Answers the new key's id.
    pub async fn rotate(&self, name: &str) -> Result<String, ConsoleError> {
        let (old, _) = self.credential().await?;
        let token = self.present_token().await?;
        let new_id = self.enrol(&token, name).await?;
        self.client
            .op(
                REVOKE,
                &old.space,
                &token,
                false,
                &json!({"id": old.key_id}),
            )
            .await?;
        Ok(new_id)
    }

    /// Unlink: revoke the key at the deployment, then forget it here.
    pub async fn unlink(&self) -> Result<String, ConsoleError> {
        let (row, _) = self.credential().await?;
        let token = self.present_token().await?;
        self.client
            .op(
                REVOKE,
                &row.space,
                &token,
                false,
                &json!({"id": row.key_id}),
            )
            .await?;
        self.store.delete_credential(&self.dep).await?;
        *self.held.lock().await = Sessions::default();
        Ok(row.key_id)
    }
}

/// Who a call to the door is made as.
#[derive(Clone)]
pub enum Caller {
    /// The console key (G14), with the owner's relayed touch (G15) for
    /// writes. Only when the console keeps its own sign-in.
    ConsoleKey,
    /// A deployment person, with the identity service's token they brought
    /// (`plan-identity-service.md` 5.4 step 5). Their touch rides in the
    /// token's `touched_at`, so the door decides presence itself.
    Person(String),
}

impl std::fmt::Debug for Caller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Caller::ConsoleKey => f.write_str("ConsoleKey"),
            // The token is a credential; it never reaches a log.
            Caller::Person(_) => f.write_str("Person(..)"),
        }
    }
}

/// The link to the deployment, bound to one caller. Every call the console
/// makes at the door goes through one of these.
pub struct DoorAs<'a> {
    link: &'a DoorLink,
    caller: Caller,
}

impl DoorAs<'_> {
    /// The caller this handle calls as.
    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    /// The bearer for a call that needs no touch.
    async fn bearer(&self) -> Result<String, ConsoleError> {
        match &self.caller {
            Caller::ConsoleKey => self.link.token().await,
            Caller::Person(t) => Ok(t.clone()),
        }
    }

    /// The bearer for a call that needs a touch. A console key session
    /// must have been touched for; a person's token is sent as it is, and
    /// the door answers `presence_required` when its touch is stale.
    async fn present(&self) -> Result<String, ConsoleError> {
        match &self.caller {
            Caller::ConsoleKey => self.link.present_token().await,
            Caller::Person(t) => Ok(t.clone()),
        }
    }

    /// For a person, the door's `presence_required` is the same step-up the
    /// console asks for itself: the page takes a fresh touch at the
    /// identity service and runs the call once more (5.4 step 6).
    fn answer<T>(&self, r: Result<T, DoorError>) -> Result<T, ConsoleError> {
        r.map_err(|e| match (&self.caller, &e) {
            (Caller::Person(_), DoorError::Refused { code, .. }) if code == "presence_required" => {
                ConsoleError::StepUpNeeded {
                    window_secs: PRESENCE_SECS,
                }
            }
            _ => ConsoleError::Door(e),
        })
    }

    /// Call a platform op in the deployment's own space.
    pub async fn op(
        &self,
        name: &str,
        read: bool,
        params: &Value,
    ) -> Result<Vec<Value>, ConsoleError> {
        let token = self.bearer().await?;
        let space = self.link.linked_space().await?;
        self.answer(
            self.link
                .client
                .op(name, &space, &token, read, params)
                .await,
        )
    }

    /// The same, for a write that needs presence.
    pub async fn op_present(&self, name: &str, params: &Value) -> Result<Vec<Value>, ConsoleError> {
        self.op_present_within(name, params, crate::door::CALL_TIMEOUT)
            .await
    }

    /// [`DoorAs::op_present`], allowed `timeout`: an apply runs schema
    /// changes and may wait for another apply's lock.
    pub async fn op_present_within(
        &self,
        name: &str,
        params: &Value,
        timeout: std::time::Duration,
    ) -> Result<Vec<Value>, ConsoleError> {
        let token = self.present().await?;
        let space = self.link.linked_space().await?;
        self.answer(
            self.link
                .client
                .op_within(name, &space, &token, false, params, timeout)
                .await,
        )
    }

    /// A write in a tenant's own space: the tenant ops the door opens to
    /// the deployment's people (`console_keys::TENANT_OPS_OPEN`), each with
    /// a fresh touch (owner, 2026-10-29).
    pub async fn op_present_in(
        &self,
        space: &str,
        name: &str,
        params: &Value,
    ) -> Result<Vec<Value>, ConsoleError> {
        let token = self.present().await?;
        self.answer(
            self.link
                .client
                .op(name, space, &token, false, params)
                .await,
        )
    }

    /// A read in a tenant's own space, with a fresh touch: the door takes
    /// a deployment caller in a tenant only with one, read or not.
    pub async fn read_present_in(
        &self,
        space: &str,
        name: &str,
        params: &Value,
    ) -> Result<Vec<Value>, ConsoleError> {
        let token = self.present().await?;
        self.answer(self.link.client.op(name, space, &token, true, params).await)
    }

    /// One page of a tenant's own security record, `GET /security/events`
    /// (`plan-console.md` 4.8), from full-stream index `from`.
    pub async fn tenant_events(
        &self,
        space: &str,
        from: u64,
        limit: u64,
    ) -> Result<Value, ConsoleError> {
        let token = self.present().await?;
        let path = format!(
            "/security/events?space={}&from={from}&limit={limit}",
            crate::door::encode(space)
        );
        self.answer(self.link.client.get(&path, Some(&token), None).await)
    }

    /// One space's usage records from `GET /usage` (`plan-console.md` 4.9):
    /// one record a period between `from` and `to`, and one for each gap.
    pub async fn usage(&self, space: &str, from: u64, to: u64) -> Result<Vec<Value>, ConsoleError> {
        let token = self.bearer().await?;
        let home = self.link.linked_space().await?;
        let path = format!(
            "/usage?space={}&from={from}&to={to}",
            crate::door::encode(space)
        );
        self.answer(
            self.link
                .client
                .get_lines(&path, Some(&token), Some(&home))
                .await,
        )
    }

    /// The door's readiness answer, which names the tenants this node holds
    /// closed (`quarantined`).
    pub async fn ready(&self) -> Result<Value, ConsoleError> {
        // As the deployment's caller: an anonymous asker gets the status only.
        self.as_owner("/health/ready").await
    }

    /// `GET path` on the door as this caller. A console key not yet linked
    /// has no session, and asks anonymously: the door answers the status
    /// only, which is all an unlinked console may see.
    pub(crate) async fn as_owner(&self, path: &str) -> Result<Value, ConsoleError> {
        match self.bearer().await {
            Ok(token) => {
                let space = self.link.linked_space().await.unwrap_or_default();
                self.answer(self.link.client.get(path, Some(&token), Some(&space)).await)
            }
            Err(ConsoleError::NotLinked) if path != "/health/deep" => {
                Ok(self.link.client.get(path, None, None).await?)
            }
            Err(e) => Err(e),
        }
    }

    /// Health from the door (`plan-console.md` 4.1): ready, deep and
    /// deploy. Each part answers on its own, so one that fails does not
    /// hide the others.
    pub async fn health(&self) -> Value {
        fn part(r: Result<Value, ConsoleError>) -> Value {
            match r {
                Ok(v) => v,
                Err(e) => json!({"error": e.code(), "message": e.to_string()}),
            }
        }
        let ready = self.as_owner("/health/ready").await;
        let deploy = self.as_owner("/health/deploy").await;
        let deep = self.as_owner("/health/deep").await;
        json!({"ready": part(ready), "deep": part(deep), "deploy": part(deploy)})
    }

    /// The deployment's own space, where platform ops are called.
    pub async fn home_space(&self) -> Result<String, ConsoleError> {
        self.link.linked_space().await
    }

    /// Whether the door answers at all: `GET /health/live`, anonymous. The
    /// Deployment screen's check when the console holds no link
    /// (`plan-identity-service.md` 6).
    pub async fn reachable(&self) -> Result<Value, ConsoleError> {
        Ok(self.link.client.get("/health/live", None, None).await?)
    }
}
