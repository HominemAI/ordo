//! Tokens for one Ordo deployment to call another
//! (`plan-federation-grants.md` 5 and 7, step G2).
//!
//! The **consumer** calls the **holder**'s door. Both are deployments
//! registered here. The token endpoint, `POST /oidc/token`, serves 2 more
//! grants beside `authorization_code`:
//!
//! | `grant_type` | What the consumer gets |
//! |---|---|
//! | `urn:ietf:params:oauth:grant-type:token-exchange` (RFC 8693) | on behalf of a person: `sub` the person, `act.sub` the consumer, `typ` `ordo-peer+jwt` |
//! | `client_credentials` | as itself: `sub` the consumer, `kind` `client`, `typ` `ordo-client+jwt` |
//!
//! Both carry `aud` `ordo-peer:<holder>`, `client_id` the consumer, `chain`
//! (the consumer's `ordo_chain` with the consumer added last), `iss`
//! the OIDC issuer, a fresh `jti`, and live
//! `ORDO_IDENTITY_PEER_TOKEN_SECS`. A token on behalf of a person never
//! outlives the person's own token it was exchanged for. The holder decides
//! what either may do (G3); this service only says who is calling.
//!
//! # How the consumer proves itself
//!
//! With a client assertion (RFC 7523, `private_key_jwt`):
//! `client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer`
//! and `client_assertion`, an EdDSA JWT whose `iss` and `sub` are its
//! handle, whose `aud` is this token endpoint or the issuer, with a `jti`
//! and an `exp` at most `ORDO_IDENTITY_CLIENT_ASSERTION_SECS` from now. It
//! is checked against the public half of the consumer key registered for
//! that handle ([`crate::deployments::Registered::consumer_key`]). Each
//! `jti` works once: its tagged hash is kept until the assertion expires.
//!
//! The consumer key is derived from the deployment's master key
//! (`ordo_identity::peer_token::consumer_key`), so no person makes or
//! stores a private key. The public half is registered with the
//! deployment (`consumerKey`, `deployment add --consumer-key`) or later
//! (`POST /deployments/{handle}/consumer-key`, `deployment consumer-key`).
//!
//! # What is refused
//!
//! - No assertion, or one that fails any check above, or a consumer with
//!   no key: `invalid_client`, 401. No audit row: nobody has proved who
//!   they are, so a stranger cannot grow the log.
//! - An `audience` that is not a live registered deployment, or is the
//!   consumer itself: `invalid_target`.
//! - A subject token that is not an ID token this service issued to the
//!   presenting consumer, live, for a person still a member of it:
//!   `invalid_grant`.
//!
//! Once the consumer has proved itself, each try is an audit row:
//! `oidc.exchange` on behalf of a person (actor the person, when known),
//! `oidc.client-token` as itself, `ok` or `refused` with the reason.

use std::collections::HashMap;

use ordo_exec::sql_lit;
use ordo_identity::jose::{self, Alg, Jwk, Jwks};
use ordo_identity::peer_token::{
    self, ASSERTION_TYPE, CHAIN_PARAM, KIND_CLIENT, KIND_PERSON, TOKEN_TYPE_ACCESS_TOKEN,
    TOKEN_TYPE_ID_TOKEN, TOKEN_TYPE_JWT, TYP_AS_ITSELF, TYP_ON_BEHALF, TYP_SESSION,
};
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use serde_json::{json, Map, Value};

use crate::audit::{self, Entry};
use crate::deployments::{self, Registered};
use crate::error::IdentityError;
use crate::oidc::{ID_TOKEN_TYP, TOKEN_PATH};
use crate::secrets::{assertion_jti_hash, mint_secret};
use crate::server::Service;
use crate::store::dep;

/// The audit action of a token on behalf of a person.
pub const ACTION_EXCHANGE: &str = "oidc.exchange";
/// The audit action of a token for a consumer as itself.
pub const ACTION_CLIENT: &str = "oidc.client-token";
const MAX_JTI: usize = 255;

/// The form's value for `k`, when given and not empty.
fn get<'a>(f: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    f.get(k).map(String::as_str).filter(|v| !v.is_empty())
}

fn need<'a>(f: &'a HashMap<String, String>, k: &str) -> Result<&'a str, IdentityError> {
    get(f, k).ok_or_else(|| IdentityError::BadRequest(format!("`{k}` is required")))
}

/// The claims of a JWT, decoded without checking its signature. Used only
/// to find which consumer's key to check it with.
fn unverified_claims(token: &str) -> Option<Map<String, Value>> {
    let payload = token.split('.').nth(1)?;
    match jose::strict_json(&b64url_decode(payload).ok()?).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// The header `typ` of a JWT, unverified.
fn header_typ(token: &str) -> Option<String> {
    let header = token.split('.').next()?;
    match jose::strict_json(&b64url_decode(header).ok()?).ok()? {
        Value::Object(m) => m.get("typ").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

/// Whether an `aud` claim, a string or a list, names `want`.
fn aud_names(aud: Option<&Value>, want: &[&str]) -> bool {
    match aud {
        Some(Value::String(a)) => want.contains(&a.as_str()),
        Some(Value::Array(list)) => list
            .iter()
            .any(|a| a.as_str().is_some_and(|a| want.contains(&a))),
        _ => false,
    }
}

impl Service {
    /// `grant_type=urn:ietf:params:oauth:grant-type:token-exchange`.
    pub(crate) async fn token_exchange(
        &self,
        f: &HashMap<String, String>,
    ) -> Result<Value, IdentityError> {
        let consumer = self.consumer(f).await?;
        let client = consumer.handle.as_str();
        let audience = need(f, "audience")?;
        let subject = need(f, "subject_token")?;
        let session = match get(f, "subject_token_type") {
            Some(TOKEN_TYPE_ID_TOKEN) => false,
            Some(TOKEN_TYPE_JWT) => true,
            _ => {
                return Err(IdentityError::BadRequest(format!(
                    "`subject_token_type` must be `{TOKEN_TYPE_ID_TOKEN}`, the person's ID \
                     token, or `{TOKEN_TYPE_JWT}`, the consumer's own session statement \
                     (`typ` `{TYP_SESSION}`)"
                )))
            }
        };
        no_extras(f)?;
        let chain = chain_of(f, client)?;
        let now = self.now();
        let holder = match self.holder(client, audience).await {
            Ok(h) => h,
            Err(e) => {
                self.record(
                    ACTION_EXCHANGE,
                    "",
                    client,
                    "refused",
                    &format!("audience {audience}"),
                )
                .await?;
                return Err(e);
            }
        };
        let checked = if session {
            self.session_subject(&consumer, subject, now).await?
        } else {
            self.subject(client, subject, now).await?
        };
        let (person, subject_exp) = match checked {
            Ok(p) => p,
            Err((person, why)) => {
                self.record(
                    ACTION_EXCHANGE,
                    &person,
                    client,
                    "refused",
                    &format!("{why} aud={holder}"),
                )
                .await?;
                return Err(IdentityError::SubjectRefused);
            }
        };
        // A token on behalf of a person never outlives the ID token it was
        // exchanged for. A session statement lives a minute; the token it
        // buys lives its own span.
        let exp = if session {
            now + self.config().peer_token_secs
        } else {
            (now + self.config().peer_token_secs).min(subject_exp)
        };
        let claims = json!({
            "iss": self.oidc_issuer(),
            "sub": person,
            "aud": peer_token::audience(&holder),
            "act": { "sub": client },
            "client_id": client,
            "kind": KIND_PERSON,
            "iat": now,
            "exp": exp,
            "jti": mint_secret(),
            "chain": chain,
        });
        let token = self.sign_typed(&claims, TYP_ON_BEHALF, now)?;
        self.record(
            ACTION_EXCHANGE,
            &person,
            client,
            "ok",
            &format!("aud={holder}"),
        )
        .await?;
        Ok(issued(token, exp - now))
    }

    /// `grant_type=client_credentials`: the consumer as itself.
    pub(crate) async fn client_token(
        &self,
        f: &HashMap<String, String>,
    ) -> Result<Value, IdentityError> {
        let consumer = self.consumer(f).await?;
        let client = consumer.handle.as_str();
        let audience = need(f, "audience")?;
        no_extras(f)?;
        let chain = chain_of(f, client)?;
        let holder = match self.holder(client, audience).await {
            Ok(h) => h,
            Err(e) => {
                self.record(
                    ACTION_CLIENT,
                    client,
                    client,
                    "refused",
                    &format!("audience {audience}"),
                )
                .await?;
                return Err(e);
            }
        };
        let now = self.now();
        let exp = now + self.config().peer_token_secs;
        let claims = json!({
            "iss": self.oidc_issuer(),
            "sub": client,
            "aud": peer_token::audience(&holder),
            "client_id": client,
            "kind": KIND_CLIENT,
            "iat": now,
            "exp": exp,
            "jti": mint_secret(),
            "chain": chain,
        });
        let token = self.sign_typed(&claims, TYP_AS_ITSELF, now)?;
        self.record(
            ACTION_CLIENT,
            client,
            client,
            "ok",
            &format!("aud={holder}"),
        )
        .await?;
        Ok(issued(token, exp - now))
    }

    /// The consumer, once its client assertion checks out. Spends the
    /// assertion's `jti`.
    async fn consumer(&self, f: &HashMap<String, String>) -> Result<Registered, IdentityError> {
        let assertion = get(f, "client_assertion");
        let claimed = get(f, "client_id")
            .map(str::to_string)
            .or_else(|| {
                assertion
                    .and_then(unverified_claims)
                    .and_then(|c| c.get("iss").and_then(Value::as_str).map(str::to_string))
            })
            .unwrap_or_default();
        let refused = || IdentityError::ClientAssertionRefused(claimed.clone());
        let (Some(assertion), Some(ASSERTION_TYPE)) = (assertion, get(f, "client_assertion_type"))
        else {
            return Err(refused());
        };
        let pool = self.store().pool();
        let Some(d) = deployments::get(pool, self.handle(), &claimed).await? else {
            return Err(refused());
        };
        let Some(public) = d.consumer_key else {
            return Err(refused());
        };
        let jwk = Jwk::parse(&json!({
            "kty": "OKP",
            "crv": "Ed25519",
            "x": b64url_encode(&public),
        }))
        .map_err(|_| refused())?;
        let verified =
            jose::verify_with_key(assertion, &jwk, &[Alg::EdDSA]).map_err(|_| refused())?;
        let c = verified.claims().map_err(|_| refused())?;
        let now = self.now();
        let text = |k: &str| c.get(k).and_then(Value::as_str);
        let endpoint = format!("{}{TOKEN_PATH}", self.config().public_url());
        let issuer = self.oidc_issuer();
        let exp = c.get("exp").and_then(Value::as_u64);
        let jti = text("jti").filter(|j| j.len() <= MAX_JTI);
        let ok = text("iss") == Some(d.handle.as_str())
            && text("sub") == Some(d.handle.as_str())
            && aud_names(c.get("aud"), &[&endpoint, &issuer])
            && exp.is_some_and(|e| e > now && e <= now + self.config().client_assertion_secs)
            && c.get("iat")
                .is_none_or(|i| i.as_u64().is_some_and(|i| i <= now))
            && jti.is_some();
        let (true, Some(jti), Some(exp)) = (ok, jti, exp) else {
            return Err(refused());
        };
        // Spent by this try, whatever follows.
        let fresh = !pool
            .rows(&format!(
                "INSERT INTO client_assertions (deployment, client, jti_hash, expires_at, \
                 created_at) VALUES ({}, {}, {}, {exp}, {now}) ON CONFLICT DO NOTHING RETURNING 1",
                sql_lit(self.handle()),
                sql_lit(&d.handle),
                sql_lit(&assertion_jti_hash(jti)),
            ))
            .await?
            .is_empty();
        if !fresh {
            return Err(refused());
        }
        Ok(d)
    }

    /// The holder `audience` names: a live registered deployment that is
    /// not the consumer.
    async fn holder(&self, client: &str, audience: &str) -> Result<String, IdentityError> {
        if audience == client {
            return Err(IdentityError::TargetRefused(format!(
                "`{audience}` is the calling deployment; a deployment calls its own data \
                 directly"
            )));
        }
        match deployments::get(self.store().pool(), self.handle(), audience).await? {
            Some(d) => Ok(d.handle),
            None => Err(IdentityError::TargetRefused(format!(
                "`{audience}` is not a deployment registered with this identity service; name \
                 the holder by its handle (`ordo-identity-service deployment list`)"
            ))),
        }
    }

    /// The person a subject token names and when it expires, or the person
    /// (when known) and why it is refused.
    async fn subject(
        &self,
        client: &str,
        token: &str,
        now: u64,
    ) -> Result<Result<(String, u64), (String, &'static str)>, IdentityError> {
        let jwks = Jwks::parse(self.keys().jwks().to_string().as_bytes())
            .map_err(|e| IdentityError::Internal(format!("the service's own key set: {e:?}")))?;
        let Ok(verified) = jose::verify_with_jwks(token, &jwks, &[Alg::EdDSA]) else {
            return Ok(Err((String::new(), "not signed by this service")));
        };
        let Ok(c) = verified.claims() else {
            return Ok(Err((String::new(), "not a claim set")));
        };
        let person = c
            .get("sub")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let why = if header_typ(token).as_deref() != Some(ID_TOKEN_TYP) {
            "not an ID token"
        } else if c.get("iss").and_then(Value::as_str) != Some(self.oidc_issuer().as_str()) {
            "another issuer"
        } else if c.get("aud").and_then(Value::as_str) != Some(client) {
            "issued to another client"
        } else if person.is_empty() {
            "no subject"
        } else if c
            .get("exp")
            .and_then(Value::as_u64)
            .is_none_or(|e| e <= now)
        {
            "expired"
        } else if deployments::role_of(self.store().pool(), self.handle(), client, &person)
            .await?
            .is_none()
        {
            "not a member"
        } else {
            ""
        };
        if !why.is_empty() {
            return Ok(Err((person, why)));
        }
        let exp = c.get("exp").and_then(Value::as_u64).unwrap_or(now);
        Ok(Ok((person, exp)))
    }

    /// The person a consumer's session statement names (`typ`
    /// `ordo-session+jwt`, step G5), or the person and why it is refused.
    ///
    /// Signed with the presenting consumer's own registered key, `iss` that
    /// consumer, `aud` this service's issuer, live for at most
    /// `ORDO_IDENTITY_CLIENT_ASSERTION_SECS`, and the person still a member
    /// of the consumer. The consumer says it holds a live session for them;
    /// the holder still decides what the person may do.
    async fn session_subject(
        &self,
        consumer: &Registered,
        token: &str,
        now: u64,
    ) -> Result<Result<(String, u64), (String, &'static str)>, IdentityError> {
        let Some(public) = consumer.consumer_key else {
            return Ok(Err((String::new(), "no consumer key")));
        };
        let Ok(jwk) = Jwk::parse(&json!({
            "kty": "OKP",
            "crv": "Ed25519",
            "x": b64url_encode(&public),
        })) else {
            return Ok(Err((String::new(), "no consumer key")));
        };
        let Ok(verified) = jose::verify_with_key(token, &jwk, &[Alg::EdDSA]) else {
            return Ok(Err((String::new(), "not signed by the consumer")));
        };
        let Ok(c) = verified.claims() else {
            return Ok(Err((String::new(), "not a claim set")));
        };
        let person = c
            .get("sub")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let exp = c.get("exp").and_then(Value::as_u64);
        let why = if header_typ(token).as_deref() != Some(TYP_SESSION) {
            "not a session statement"
        } else if c.get("iss").and_then(Value::as_str) != Some(consumer.handle.as_str()) {
            "issued by another deployment"
        } else if c.get("aud").and_then(Value::as_str) != Some(self.oidc_issuer().as_str()) {
            "addressed to another service"
        } else if person.is_empty() {
            "no subject"
        } else if exp.is_none_or(|e| e <= now || e > now + self.config().client_assertion_secs) {
            "expired, or living too long"
        } else if deployments::role_of(
            self.store().pool(),
            self.handle(),
            &consumer.handle,
            &person,
        )
        .await?
        .is_none()
        {
            "not a member"
        } else {
            ""
        };
        if !why.is_empty() {
            return Ok(Err((person, why)));
        }
        Ok(Ok((person, exp.unwrap_or(now))))
    }

    /// Sign `claims` with the active token key under header `typ`.
    fn sign_typed(&self, claims: &Value, typ: &str, now: u64) -> Result<String, IdentityError> {
        let (kid, key) = self.keys().token_signer(now)?;
        Ok(peer_token::sign_jwt(
            &json!({ "alg": "EdDSA", "kid": kid, "typ": typ }),
            claims,
            &key,
        ))
    }

    /// One audit row for a try by a consumer that proved itself.
    async fn record(
        &self,
        action: &str,
        actor: &str,
        client: &str,
        outcome: &str,
        detail: &str,
    ) -> Result<(), IdentityError> {
        let tx = self.store().pool().begin().await?;
        audit::append(
            &tx,
            self.handle(),
            self.keys(),
            &Entry {
                actor,
                action,
                subject: client,
                outcome,
                client: "",
                detail,
            },
            self.now(),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Parameters this service does not serve, refused by name rather than
/// ignored: an actor token (the consumer is the actor, proved by its
/// assertion) and `resource` (name the holder in `audience`).
fn no_extras(f: &HashMap<String, String>) -> Result<(), IdentityError> {
    for (k, why) in [
        (
            "actor_token",
            "the calling deployment is the actor; its client assertion proves it",
        ),
        ("resource", "name the holder deployment in `audience`"),
    ] {
        if f.contains_key(k) {
            return Err(IdentityError::BadRequest(format!(
                "`{k}` is not taken here: {why}"
            )));
        }
    }
    match get(f, "requested_token_type") {
        None | Some(TOKEN_TYPE_ACCESS_TOKEN) => Ok(()),
        Some(other) => Err(IdentityError::BadRequest(format!(
            "`requested_token_type` `{other}` is not issued here; leave it out, or ask for \
             `{TOKEN_TYPE_ACCESS_TOKEN}`"
        ))),
    }
}

/// The chain the token carries: the consumer's incoming chain
/// ([`CHAIN_PARAM`], when it sends one) with the consumer added last. The
/// holder refuses a call whose chain already names it (G3), so a loop of
/// grants between deployments ends at the first deployment it returns to.
fn chain_of(f: &HashMap<String, String>, client: &str) -> Result<Vec<String>, IdentityError> {
    let mut chain = match get(f, CHAIN_PARAM) {
        Some(text) => peer_token::parse_chain(text).map_err(IdentityError::BadRequest)?,
        None => Vec::new(),
    };
    chain.push(client.to_string());
    Ok(chain)
}

/// RFC 8693 2.2.1's answer.
fn issued(token: String, expires_in: u64) -> Value {
    json!({
        "access_token": token,
        "issued_token_type": TOKEN_TYPE_ACCESS_TOKEN,
        "token_type": "Bearer",
        "expires_in": expires_in,
    })
}

/// Delete spent assertions' `jti`s once the assertion could no longer be
/// taken anyway. Run on the minute tick.
pub async fn sweep_assertions(
    pool: &ordo_exec::Pool,
    handle: &str,
    now: u64,
) -> Result<u64, IdentityError> {
    Ok(pool
        .rows(&format!(
            "DELETE FROM client_assertions WHERE {} AND expires_at <= {now} RETURNING 1",
            dep(handle)
        ))
        .await?
        .len() as u64)
}
