//! The consumer's one client to another Ordo deployment's door
//! (`plan-federation-grants.md` 4, 5 and 7, step G4).
//!
//! This deployment is the **consumer**; the deployment it calls is the
//! **holder**. Every call to a holder, and every request to the identity
//! service that makes one possible, goes through [`PeerClient`] and fails as
//! one [`PeerClientError`] (AGENTS.md rule 9).
//!
//! # What it does
//!
//! - Holds the **consumer key**, derived from the deployment's master key
//!   ([`consumer_key_from_custody`]), so no person makes or copies a key.
//!   Only its public half leaves: [`PeerClient::register_key`] registers it
//!   at the identity service with a token for this deployment's own admin
//!   (`POST /deployments/{handle}/consumer-key`, step G2).
//! - Signs a fresh client assertion (RFC 7523) for each token it asks for,
//!   and asks the service's token endpoint for a token addressed to the
//!   holder: as itself (`client_credentials`) or on behalf of a person (RFC
//!   8693 token exchange, with the person's ID token this deployment holds).
//! - Passes the call chain on: the chain of the call this deployment is
//!   serving goes to the service as `ordo_chain`, which writes it into the
//!   token under its signature (rule 9, the loop check).
//! - Calls the holder's door: `GET /op/<read>` or `POST /op/<write>`, bound
//!   to the holder's space, with that token.
//!
//! # Where it reaches
//!
//! The identity service at `ORDO_IDENTITY_URL`, under the rules
//! [`crate::identity_keys`] keeps. A holder at `ORDO_PEER_<NAME>_URL`,
//! where `<NAME>` is the `peer` block's nickname upper-cased; its handle at
//! the identity service is `ORDO_PEER_<NAME>_DEPLOYMENT` (default the
//! nickname) and the space a call is bound to is `ORDO_PEER_<NAME>_SPACE`.
//! The address is a setting, never the file, so one file works in every
//! environment. Plain `http://` reaches loopback only, unless
//! `ORDO_PEER_PLAIN_HTTP=on`: the token would cross the wire in the clear.
//! A holder is the deployment owner's own sibling, reached directly and not
//! through the egress kernel, as the identity service is.
//!
//! # One error
//!
//! [`PeerClientError`] keeps its cause and says what a caller can do
//! ([`PeerClientError::action`]): retry, refuse, or fix a setting. The apply
//! turns it into a refusal line; a door op into an [`ExecError`].

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use ordo_api::{ErrorCode, ExecError};
use ordo_identity::federation::form_encode;
use ordo_identity::peer_token::{
    self, ASSERTION_TYPE, CHAIN_PARAM, GRANT_CLIENT_CREDENTIALS, GRANT_TOKEN_EXCHANGE,
    TOKEN_TYPE_ID_TOKEN,
};
use serde_json::{json, Value};

use crate::identity_keys::{
    ServiceUrl, DEFAULT_DEPLOYMENT, DEPLOYMENT_ENV, ISSUER_ENV, PLAIN_HTTP_ENV as IDS_PLAIN_ENV,
    URL_ENV as IDS_URL_ENV,
};

/// `on` lets a holder's door be plain `http://` to a host that is not
/// loopback.
pub const PLAIN_HTTP_ENV: &str = "ORDO_PEER_PLAIN_HTTP";
/// How long one request may take, connect to last byte.
pub const REQUEST_SECS: u64 = 30;
/// The largest answer read from a holder or the identity service.
pub const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;
/// How long a client assertion lives. The service takes at most
/// `ORDO_IDENTITY_CLIENT_ASSERTION_SECS` (120 by default).
pub const ASSERTION_SECS: u64 = 60;
/// The token endpoint's path under the service's address.
pub const TOKEN_PATH: &str = "/oidc/token";

/// How a setting is read: the process environment, or a test's map.
pub type Settings = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// `ORDO_PEER_<NAME>_URL`: where the holder's door answers.
pub fn url_env(peer: &str) -> String {
    format!("ORDO_PEER_{}_URL", peer.to_ascii_uppercase())
}

/// `ORDO_PEER_<NAME>_DEPLOYMENT`: the holder's handle at the identity
/// service.
pub fn deployment_env(peer: &str) -> String {
    format!("ORDO_PEER_{}_DEPLOYMENT", peer.to_ascii_uppercase())
}

/// `ORDO_PEER_<NAME>_SPACE`: the holder's space a call is bound to.
pub fn space_env(peer: &str) -> String {
    format!("ORDO_PEER_{}_SPACE", peer.to_ascii_uppercase())
}

/// The consumer key from the deployment's custody: a seed the key provider
/// computes under its own key material, never returned, then
/// [`peer_token::consumer_key`] over it. Every node holding the deployment
/// derives the same key, and no person sees it.
pub fn consumer_key_from_custody(
    keys: &dyn ordo_crypto::KeyProvider,
    home: &str,
    handle: &str,
) -> Result<SigningKey, ordo_crypto::KeyError> {
    let seed = keys.blind_index(
        &ordo_crypto::SpaceId(home.to_string()),
        "$consumerkey",
        "v1",
        ordo_crypto::BlindValue::Text(handle),
    )?;
    Ok(peer_token::consumer_key(&seed, handle))
}

// ── the one error ─────────────────────────────────────────────────────────

/// What a caller can do about a [`PeerClientError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The same call can succeed later with nothing changed here.
    Retry,
    /// The holder or the service refused it; it will not succeed as asked.
    Refuse,
    /// A setting or a registration is missing or wrong; the message names
    /// it.
    FixInput,
}

/// Why a call to a holder, or a request to the identity service for one,
/// failed. Each variant names the address or the setting and keeps its
/// cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerClientError {
    /// A setting is missing or not usable. `setting` is its name.
    Setting { setting: String, why: String },
    /// The service or the holder could not be reached, or did not answer
    /// in time, or answered 5xx.
    Unreachable { what: String, cause: String },
    /// The identity service did not take this deployment's client
    /// assertion: no consumer key registered for it, or another one.
    KeyNotRegistered { handle: String, message: String },
    /// The identity service refused the token asked for: an audience that
    /// is not a registered deployment, a person's token it does not take.
    TokenRefused { code: String, message: String },
    /// The holder refused the call: the message says which grant line or
    /// setting would allow it.
    HolderRefused {
        peer: String,
        op: String,
        status: u16,
        code: String,
        message: String,
    },
    /// The identity service refused to register the key: the token is not
    /// this deployment's admin's, or holds no fresh touch.
    RegistrationRefused { status: u16, message: String },
    /// An answer that is not what the protocol promises.
    Answer { what: String, why: String },
    /// The key could not be derived from custody.
    Custody(String),
}

impl PeerClientError {
    /// What a caller can do.
    pub fn action(&self) -> Action {
        match self {
            PeerClientError::Unreachable { .. } => Action::Retry,
            PeerClientError::HolderRefused { status, .. } if *status == 429 => Action::Retry,
            PeerClientError::Setting { .. }
            | PeerClientError::KeyNotRegistered { .. }
            | PeerClientError::Custody(_) => Action::FixInput,
            PeerClientError::TokenRefused { .. }
            | PeerClientError::HolderRefused { .. }
            | PeerClientError::RegistrationRefused { .. }
            | PeerClientError::Answer { .. } => Action::Refuse,
        }
    }

    /// Can the same call succeed later with nothing changed here?
    pub fn retryable(&self) -> bool {
        self.action() == Action::Retry
    }

    /// The door's answer for it, at the one edge a door op has.
    pub fn to_exec_error(&self) -> ExecError {
        let code = match self.action() {
            Action::Retry => ErrorCode::ServerBusy,
            Action::FixInput => ErrorCode::InvalidParam,
            Action::Refuse => ErrorCode::ApplyRefused,
        };
        ExecError::new(code, self.to_string())
    }
}

impl fmt::Display for PeerClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerClientError::Setting { setting, why } => write!(f, "{setting} {why}"),
            PeerClientError::Unreachable { what, cause } => {
                write!(f, "{what} could not be reached ({cause}); try again")
            }
            PeerClientError::KeyNotRegistered { handle, message } => write!(
                f,
                "the identity service did not take this deployment's consumer key ({message}). \
                 Fix: register it once with `std.peer.registerKey`, giving a token for an \
                 admin of `{handle}`, or have the service's admin run `ordo-identity-service \
                 deployment consumer-key {handle} <key>`"
            ),
            PeerClientError::TokenRefused { code, message } => {
                write!(
                    f,
                    "the identity service refused the token: {code}: {message}"
                )
            }
            PeerClientError::HolderRefused {
                peer,
                op,
                status,
                code,
                message,
            } => write!(
                f,
                "peer `{peer}` refused `{op}` ({status} {code}): {message}"
            ),
            PeerClientError::RegistrationRefused { status, message } => write!(
                f,
                "the identity service did not register the consumer key ({status}): {message}. \
                 Fix: give a token for an admin of this deployment, signed in a moment ago"
            ),
            PeerClientError::Answer { what, why } => write!(f, "{what} answered {why}"),
            PeerClientError::Custody(why) => write!(
                f,
                "the consumer key could not be derived from this deployment's custody: {why}"
            ),
        }
    }
}

impl std::error::Error for PeerClientError {}

// ── settings ──────────────────────────────────────────────────────────────

/// Where this deployment reaches the identity service, and who it is there.
#[derive(Debug, Clone)]
struct ServiceSettings {
    handle: String,
    url: ServiceUrl,
    /// The service's issuer: the `aud` of a client assertion.
    issuer: String,
}

impl ServiceSettings {
    fn from_vars(var: &dyn Fn(&str) -> Option<String>) -> Result<Self, PeerClientError> {
        let set = |k: &str| {
            var(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let url_text = set(IDS_URL_ENV).ok_or_else(|| PeerClientError::Setting {
            setting: IDS_URL_ENV.into(),
            why: "is unset, so this deployment cannot ask the identity service for a token to \
                  call another deployment. Set it to where the identity service answers"
                .into(),
        })?;
        let plain_ok = set(IDS_PLAIN_ENV).as_deref() == Some("on");
        let url =
            ServiceUrl::parse(&url_text, plain_ok).map_err(|why| PeerClientError::Setting {
                setting: IDS_URL_ENV.into(),
                why,
            })?;
        // The issuer of the sign-in connection to the service, when there is
        // one; the service's origin otherwise.
        let issuer = crate::peer_door::PeerSettings::from_vars(var)
            .ok()
            .map(|s| s.issuer)
            .filter(|i| !i.is_empty())
            .or_else(|| set(ISSUER_ENV))
            .ok_or_else(|| PeerClientError::Setting {
                setting: ISSUER_ENV.into(),
                why: "is unset. Set it to the identity service's public origin, the `iss` of \
                      its tokens"
                    .into(),
            })?;
        let handle = set(DEPLOYMENT_ENV).unwrap_or_else(|| DEFAULT_DEPLOYMENT.into());
        if !peer_token::valid_handle(&handle) {
            return Err(PeerClientError::Setting {
                setting: DEPLOYMENT_ENV.into(),
                why: format!("is `{handle}`, which is not a deployment handle"),
            });
        }
        Ok(ServiceSettings {
            handle,
            url,
            issuer: issuer.trim_end_matches('/').to_string(),
        })
    }
}

/// One holder, as the settings place it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// The `peer` block's nickname.
    pub peer: String,
    /// The holder's handle at the identity service.
    pub handle: String,
    /// The space each call is bound to.
    pub space: String,
    url: ServiceUrl,
}

impl Holder {
    /// The door's address, for messages.
    pub fn address(&self) -> String {
        self.url.display()
    }
}

fn loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Who a call speaks for.
#[derive(Debug, Clone, Copy)]
pub enum Acting<'a> {
    /// The consumer alone: the holder needs a grant marked `as itself`.
    AsItself,
    /// A person, by the ID token the identity service issued this
    /// deployment for them: the holder applies its own rules for them.
    OnBehalf { id_token: &'a str },
}

/// An op's kind decides the method the door takes: `GET`, `POST` or
/// `DELETE` (`ordo_api::router::method_for`). A holder's grant answer
/// carries each granted op's kind ([`HolderGrant::kind`]).
pub use ordo_api::OpKind as Method;

// ── the client ────────────────────────────────────────────────────────────

/// The client. One per deployment; share it as an `Arc`.
pub struct PeerClient {
    service: Result<ServiceSettings, PeerClientError>,
    key: SigningKey,
    var: Settings,
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl fmt::Debug for PeerClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.service {
            Ok(s) => write!(f, "PeerClient({} at {})", s.handle, s.url.display()),
            Err(_) => f.write_str("PeerClient(unconfigured)"),
        }
    }
}

impl PeerClient {
    /// A client signing with `key`, reading its settings through `var`.
    pub fn new(
        key: SigningKey,
        var: Settings,
        now: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> PeerClient {
        PeerClient {
            service: ServiceSettings::from_vars(&*var),
            key,
            var,
            now,
        }
    }

    fn service(&self) -> Result<&ServiceSettings, PeerClientError> {
        self.service.as_ref().map_err(Clone::clone)
    }

    /// This deployment's handle at the identity service.
    pub fn handle(&self) -> Option<&str> {
        self.service.as_ref().ok().map(|s| s.handle.as_str())
    }

    /// The consumer key's public half, as the service registers it.
    pub fn public_key(&self) -> String {
        peer_token::public_text(&self.key.verifying_key())
    }

    /// The holder the nickname `peer` names, from its settings.
    pub fn holder(&self, peer: &str) -> Result<Holder, PeerClientError> {
        let set = |k: &str| {
            (self.var)(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let url_setting = url_env(peer);
        let text = set(&url_setting).ok_or_else(|| PeerClientError::Setting {
            setting: url_setting.clone(),
            why: format!(
                "is unset, and `peer {peer}` calls that deployment's door. Set it to where its \
                 door answers, such as https://notes.example.com"
            ),
        })?;
        let url = ServiceUrl::parse(&text, true).map_err(|_| PeerClientError::Setting {
            setting: url_setting.clone(),
            why: format!(
                "is `{text}`, which is not scheme://host[:port]. Set it to where the door of \
                 `peer {peer}` answers"
            ),
        })?;
        if !url.https && !loopback(&url.host) && set(PLAIN_HTTP_ENV).as_deref() != Some("on") {
            return Err(PeerClientError::Setting {
                setting: url_setting,
                why: format!(
                    "is `{text}`: plain http:// to a host that is not loopback would carry a \
                     token in the clear. Use https://, or set {PLAIN_HTTP_ENV}=on if both \
                     deployments share a private network you trust"
                ),
            });
        }
        let dep_setting = deployment_env(peer);
        let handle = set(&dep_setting).unwrap_or_else(|| peer.to_string());
        if !peer_token::valid_handle(&handle) {
            return Err(PeerClientError::Setting {
                setting: dep_setting,
                why: format!(
                    "is `{handle}`, which is not a deployment handle. Set it to the handle of \
                     `peer {peer}` at the identity service"
                ),
            });
        }
        let space_setting = space_env(peer);
        let space = set(&space_setting).ok_or_else(|| PeerClientError::Setting {
            setting: space_setting,
            why: format!(
                "is unset, and every call to `peer {peer}` is bound to one of its spaces. Set it \
                 to that deployment's own space (its ORDO_SPACE)"
            ),
        })?;
        Ok(Holder {
            peer: peer.to_string(),
            handle,
            space,
            url,
        })
    }

    // ── the identity service ──────────────────────────────────────────────

    /// Register this deployment's consumer key at the identity service,
    /// with `admin_token`: a token for an admin of this deployment, which
    /// the service takes for its own registration (step G2). Replaces a key
    /// registered before.
    pub async fn register_key(&self, admin_token: &str) -> Result<String, PeerClientError> {
        let s = self.service()?;
        let path = format!("{}/deployments/{}/consumer-key", s.url.prefix, s.handle);
        let body = json!({ "consumerKey": self.public_key() }).to_string();
        let (status, answer) = send(
            &s.url,
            "POST",
            &path,
            vec![
                ("authorization".into(), format!("Bearer {admin_token}")),
                ("content-type".into(), "application/json".into()),
            ],
            Some(body.into_bytes()),
            "the identity service",
        )
        .await?;
        match status {
            200 => Ok(self.public_key()),
            s if s >= 500 => Err(PeerClientError::Unreachable {
                what: format!("the identity service at {}", self.service()?.url.display()),
                cause: format!("it answered {s}"),
            }),
            s => Err(PeerClientError::RegistrationRefused {
                status: s,
                message: message_of(&answer),
            }),
        }
    }

    /// A token for `holder` (its handle at the service): as this deployment
    /// itself, or on behalf of the person whose ID token is given. `chain`
    /// is the chain of the call being served, oldest first; empty for a
    /// call that starts here.
    pub async fn token(
        &self,
        holder: &str,
        acting: Acting<'_>,
        chain: &[String],
    ) -> Result<String, PeerClientError> {
        let s = self.service()?;
        let now = (self.now)();
        let mut jti = [0u8; 16];
        ordo_crypto::fill_random(&mut jti);
        let assertion = peer_token::client_assertion(
            &s.handle,
            &s.issuer,
            &self.key,
            now,
            now + ASSERTION_SECS,
            &ordo_identity::webauthn::b64url_encode(&jti),
        );
        let chain_text = chain.join(" ");
        let mut form: Vec<(&str, &str)> = vec![
            ("client_assertion_type", ASSERTION_TYPE),
            ("client_assertion", &assertion),
            ("audience", holder),
        ];
        match acting {
            Acting::AsItself => form.push(("grant_type", GRANT_CLIENT_CREDENTIALS)),
            Acting::OnBehalf { id_token } => {
                form.push(("grant_type", GRANT_TOKEN_EXCHANGE));
                form.push(("subject_token", id_token));
                form.push(("subject_token_type", TOKEN_TYPE_ID_TOKEN));
            }
        }
        if !chain.is_empty() {
            form.push((CHAIN_PARAM, &chain_text));
        }
        let (status, answer) = send(
            &s.url,
            "POST",
            &format!("{}{TOKEN_PATH}", s.url.prefix),
            vec![(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            Some(form_encode(&form).into_bytes()),
            "the identity service",
        )
        .await?;
        let v: Value = serde_json::from_slice(&answer).unwrap_or(Value::Null);
        let code = v["error"].as_str().unwrap_or_default().to_string();
        match status {
            200 => v["access_token"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| PeerClientError::Answer {
                    what: "the identity service".into(),
                    why: "200 with no access_token".into(),
                }),
            s if s >= 500 => Err(PeerClientError::Unreachable {
                what: format!("the identity service at {}", self.service()?.url.display()),
                cause: format!("it answered {s}"),
            }),
            _ if code == "invalid_client" => Err(PeerClientError::KeyNotRegistered {
                handle: s.handle.clone(),
                message: message_of(&answer),
            }),
            _ => Err(PeerClientError::TokenRefused {
                code,
                message: message_of(&answer),
            }),
        }
    }

    // ── the holder's door ─────────────────────────────────────────────────

    /// Call `op` on the holder `peer` names. A read is `GET` with `params`
    /// as the query; a write is `POST` with `params` as the body. The whole
    /// answer on success.
    pub async fn call(
        &self,
        peer: &str,
        op: &str,
        method: Method,
        params: &Value,
        acting: Acting<'_>,
        chain: &[String],
    ) -> Result<Value, PeerClientError> {
        let holder = self.holder(peer)?;
        let token = self.token(&holder.handle, acting, chain).await?;
        self.call_with(&holder, &token, op, method, params).await
    }

    /// [`Self::call`] with a token already in hand.
    pub async fn call_with(
        &self,
        holder: &Holder,
        token: &str,
        op: &str,
        method: Method,
        params: &Value,
    ) -> Result<Value, PeerClientError> {
        let mut headers = vec![
            ("authorization".into(), format!("Bearer {token}")),
            ("accept".into(), "application/json".into()),
            (ordo_api::router::SPACE_HEADER.into(), holder.space.clone()),
        ];
        let (verb, path, body) = match method {
            Method::Read => {
                let query: Vec<(String, String)> = params
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            match v {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            },
                        )
                    })
                    .collect();
                let pairs: Vec<(&str, &str)> = query
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                let q = form_encode(&pairs);
                let path = if q.is_empty() {
                    format!("{}/op/{op}", holder.url.prefix)
                } else {
                    format!("{}/op/{op}?{q}", holder.url.prefix)
                };
                ("GET", path, None)
            }
            Method::Write | Method::Delete => {
                headers.push(("content-type".into(), "application/json".into()));
                (
                    ordo_api::router::method_for(method).1,
                    format!("{}/op/{op}", holder.url.prefix),
                    Some(params.to_string().into_bytes()),
                )
            }
        };
        let what = format!("peer `{}` at {}", holder.peer, holder.address());
        let (status, answer) = send(&holder.url, verb, &path, headers, body, &what).await?;
        let v: Value = serde_json::from_slice(&answer).map_err(|_| PeerClientError::Answer {
            what: what.clone(),
            why: format!("{status} with something that is not JSON"),
        })?;
        match status {
            200 => Ok(v),
            s if s >= 500 && s != 503 => Err(PeerClientError::Unreachable {
                what,
                cause: format!("it answered {s}: {}", message_of(&answer)),
            }),
            503 => Err(PeerClientError::Unreachable {
                what,
                cause: format!("it is busy: {}", message_of(&answer)),
            }),
            s => Err(PeerClientError::HolderRefused {
                peer: holder.peer.clone(),
                op: op.to_string(),
                status: s,
                code: v["error"]["code"].as_str().unwrap_or_default().to_string(),
                message: message_of(&answer),
            }),
        }
    }

    /// What the holder `peer` grants this deployment, and the shape of
    /// each op it grants: `std.peer.grant`, as itself.
    pub async fn grant(&self, peer: &str) -> Result<HolderGrant, PeerClientError> {
        let v = self
            .call(
                peer,
                crate::peer_ops::GRANT,
                Method::Read,
                &json!({}),
                Acting::AsItself,
                &[],
            )
            .await?;
        let row = v["rows"]
            .as_array()
            .and_then(|r| r.first())
            .ok_or_else(|| PeerClientError::Answer {
                what: format!("peer `{peer}`"),
                why: "no row for its grant".into(),
            })?;
        HolderGrant::from_row(row).map_err(|why| PeerClientError::Answer {
            what: format!("peer `{peer}`"),
            why,
        })
    }

    /// Tell the holder `peer` which of its ops this deployment's version
    /// `version` calls: `std.peer.recordUse`, as itself.
    pub async fn record_use(
        &self,
        peer: &str,
        version: &str,
        ops: &[String],
    ) -> Result<(), PeerClientError> {
        self.call(
            peer,
            crate::peer_ops::RECORD_USE,
            Method::Write,
            &json!({ "version": version, "ops": serde_json::to_string(ops).unwrap_or_default() }),
            Acting::AsItself,
            &[],
        )
        .await
        .map(|_| ())
    }
}

/// What a holder answers to `std.peer.grant`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolderGrant {
    /// The calling deployment, as the holder names it.
    pub consumer: String,
    /// The version the holder serves.
    pub version: Option<String>,
    /// Each grant that covers the consumer.
    pub grants: Vec<crate::consumer_grants::Expanded>,
    /// Each granted op's shape: `op`, `kind` (`read` or `write`) and
    /// `params`, as the holder's compiled program states them.
    pub ops: Vec<Value>,
}

impl HolderGrant {
    fn from_row(row: &Value) -> Result<HolderGrant, String> {
        let grants = row["grants"]
            .as_array()
            .ok_or("no `grants` list")?
            .iter()
            .map(|g| {
                Some(crate::consumer_grants::Expanded {
                    name: g["name"].as_str()?.to_string(),
                    provider: g["provider"].as_bool()?,
                    as_itself: g["asItself"].as_bool()?,
                    ops: g["ops"]
                        .as_array()?
                        .iter()
                        .filter_map(|o| o.as_str().map(str::to_string))
                        .collect(),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or("a grant without its name, kind or ops")?;
        Ok(HolderGrant {
            consumer: row["consumer"].as_str().unwrap_or_default().to_string(),
            version: row["version"].as_str().map(str::to_string),
            grants,
            ops: row["ops"].as_array().cloned().unwrap_or_default(),
        })
    }

    /// Every op some grant opens to the consumer.
    pub fn granted(&self) -> std::collections::BTreeSet<&str> {
        self.grants
            .iter()
            .flat_map(|g| g.ops.iter().map(String::as_str))
            .collect()
    }

    /// The grants that open `op`.
    pub fn grants_of(&self, op: &str) -> Vec<&crate::consumer_grants::Expanded> {
        self.grants
            .iter()
            .filter(|g| g.ops.iter().any(|o| o == op))
            .collect()
    }

    /// `op`'s shape, when granted.
    pub fn shape(&self, op: &str) -> Option<&Value> {
        self.ops.iter().find(|o| o["op"].as_str() == Some(op))
    }

    /// `op`'s kind, when granted: what [`PeerClient::call`] takes.
    pub fn kind(&self, op: &str) -> Option<Method> {
        match self.shape(op)?["kind"].as_str()? {
            "read" => Some(Method::Read),
            "write" => Some(Method::Write),
            "delete" => Some(Method::Delete),
            _ => None,
        }
    }
}

/// The message of an error answer, whichever shape it has.
fn message_of(body: &[u8]) -> String {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    v["error"]["message"]
        .as_str()
        .or_else(|| v["error_description"].as_str())
        .or_else(|| v["message"].as_str())
        .unwrap_or("no message")
        .to_string()
}

/// One HTTP exchange: the status and the body, at most
/// [`MAX_ANSWER_BYTES`], in at most [`REQUEST_SECS`].
async fn send(
    url: &ServiceUrl,
    method: &'static str,
    path: &str,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    what: &str,
) -> Result<(u16, Vec<u8>), PeerClientError> {
    use futures::StreamExt;
    let shown = format!("{what} at {}", url.display());
    let unreachable = |cause: String| PeerClientError::Unreachable {
        what: shown.clone(),
        cause,
    };
    let endpoint = ordo_blob::http::Endpoint {
        host: url.host.clone(),
        port: url.port,
        tls: url.https,
    };
    let body = body.map(|bytes| {
        let len = bytes.len() as u64;
        let stream: ordo_blob::ByteStream =
            Box::pin(futures::stream::once(async move { Ok(bytes) }));
        (len, stream)
    });
    let exchange = async {
        let response = ordo_blob::http::send(
            &endpoint,
            ordo_blob::http::Request {
                method,
                path: path.to_string(),
                headers,
                body,
            },
        )
        .await
        .map_err(|e| unreachable(e.to_string()))?;
        let status = response.status;
        let mut out = Vec::new();
        let mut stream = response.body;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| unreachable(e.to_string()))?;
            if out.len() + chunk.len() > MAX_ANSWER_BYTES {
                return Err(PeerClientError::Answer {
                    what: shown.clone(),
                    why: format!("more than {MAX_ANSWER_BYTES} bytes"),
                });
            }
            out.extend_from_slice(&chunk);
        }
        Ok((status, out))
    };
    match tokio::time::timeout(Duration::from_secs(REQUEST_SECS), exchange).await {
        Ok(r) => r,
        Err(_) => Err(unreachable(format!("no answer in {REQUEST_SECS} seconds"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn client(pairs: &[(&str, &str)]) -> PeerClient {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        PeerClient::new(
            peer_token::consumer_key(&[1; 32], "viewer"),
            Arc::new(move |k: &str| map.get(k).cloned()),
            Arc::new(|| 0),
        )
    }

    const SERVICE: &[(&str, &str)] = &[
        ("ORDO_IDENTITY_URL", "http://127.0.0.1:8082"),
        ("ORDO_IDENTITY_ORIGIN", "https://id.example"),
        ("ORDO_IDENTITY_DEPLOYMENT", "viewer"),
    ];

    #[test]
    fn each_missing_setting_is_named() {
        let c = client(&[]);
        let e = c.service().unwrap_err();
        assert!(
            e.to_string().starts_with("ORDO_IDENTITY_URL is unset"),
            "{e}"
        );
        assert_eq!(e.action(), Action::FixInput);

        let c = client(SERVICE);
        assert_eq!(c.handle(), Some("viewer"));
        let e = c.holder("notes").unwrap_err();
        assert!(
            e.to_string().starts_with("ORDO_PEER_NOTES_URL is unset"),
            "{e}"
        );

        let mut with_url = SERVICE.to_vec();
        with_url.push(("ORDO_PEER_NOTES_URL", "http://10.0.0.5:8080"));
        let e = client(&with_url).holder("notes").unwrap_err();
        assert!(e.to_string().contains(PLAIN_HTTP_ENV), "{e}");

        with_url[3] = ("ORDO_PEER_NOTES_URL", "https://notes.example");
        let e = client(&with_url).holder("notes").unwrap_err();
        assert!(
            e.to_string().starts_with("ORDO_PEER_NOTES_SPACE is unset"),
            "{e}"
        );

        with_url.push(("ORDO_PEER_NOTES_SPACE", "notes_home"));
        let h = client(&with_url).holder("notes").unwrap();
        assert_eq!(
            (h.handle.as_str(), h.space.as_str()),
            ("notes", "notes_home")
        );
        with_url.push(("ORDO_PEER_NOTES_DEPLOYMENT", "notes-prod"));
        assert_eq!(
            client(&with_url).holder("notes").unwrap().handle,
            "notes-prod"
        );
    }

    #[test]
    fn the_error_says_what_a_caller_can_do() {
        let busy = PeerClientError::Unreachable {
            what: "x".into(),
            cause: "y".into(),
        };
        assert!(busy.retryable());
        let refused = PeerClientError::HolderRefused {
            peer: "notes".into(),
            op: "archiveNote".into(),
            status: 403,
            code: "access_denied".into(),
            message: "add the line".into(),
        };
        assert_eq!(refused.action(), Action::Refuse);
        assert!(refused
            .to_string()
            .contains("peer `notes` refused `archiveNote`"));
        let key = PeerClientError::KeyNotRegistered {
            handle: "viewer".into(),
            message: "m".into(),
        };
        assert!(key.to_string().contains("std.peer.registerKey"));
    }
}
