//! OAuth2 sign-in without OIDC: Discord, GitHub, and any provider with an
//! authorization endpoint, a token endpoint and a userinfo endpoint
//! (`plan-auth-federated.md` §5).
//!
//! # The flow
//!
//! 1. `begin` mints `state`, a PKCE verifier and a binder, files the ceremony
//!    under `SHA-256(state)` in phase `begun`, and answers the authorize URL
//!    and the binder.
//! 2. The provider returns the browser to `/auth/callback/<connection>`.
//!    [`OAuth2Provider::callback`] consumes the ceremony by `state`, exchanges
//!    `code` at the token endpoint with the verifier and the client secret,
//!    calls the userinfo endpoint with the access token it got back, reads the
//!    subject from the configured claim, and files a handoff (see
//!    [`crate::federation`]). The access token is dropped there.
//! 3. The application redeems the handoff with the binder at
//!    `/auth/complete`, which is [`AuthProvider::complete`] here.
//!
//! # The pins (§5.2)
//!
//! | # | Pin | Where |
//! |---|---|---|
//! | A1 | code flow, PKCE `S256` sent always, `state` one-shot and bound to the binder, one exact redirect URI built at boot, one callback path per connection | [`OAuth2Provider::begin`], [`OAuth2Provider::callback`] |
//! | A2 | the access token is used only if it came from this ceremony's own exchange; no route takes one from a caller | [`OAuth2Provider::callback`] is the only reader of a token response |
//! | A3 | a subject claim that names something mutable refuses the boot unless acknowledged | [`OAuth2Connection::check`] |
//! | A4 | the subject is a JSON string or integer, rendered canonically; anything else is a refusal | [`subject_of`] |
//! | A5 | email only with a configured verified-email claim that is the JSON boolean `true` | [`attributes_of`] |
//! | A6 | the userinfo answer is trusted because of TLS to a pinned host and nothing else | the transport; stated in `howto-sign-in.md` |
//! | O16 | the client secret never appears in a log, a refusal or an event | [`crate::federation::ClientSecret`] has no `Debug` |

use std::sync::Arc;

use serde_json::Value;

use crate::ceremony::{CeremonyRow, CeremonyStore};
use crate::federation::{
    form_encode, handle_of, random_secret, redeem_handoff, write_handoff, ClientSecret,
    FederationTransport, ProviderClient, PHASE_BEGUN,
};
use crate::principal::{AuthMethodKind, Timestamp};
use crate::provider::{
    AuthError, AuthProvider, AuthenticatedSubject, AuthenticatorId, BeginRequest,
    ChallengeOrRedirect, CompleteRequest, EmailEvidence, SubjectAttributes, VerifiedEmail,
    CEREMONY_TTL_SECS,
};
use crate::CredentialStoreError;

/// Claims that name something a person or a provider can change: a login, an
/// address, a display name. A connection whose subject is one of these
/// refuses the boot unless the deployment owner acknowledges it (A3), because
/// a renamed account or a reassigned address would become somebody else's
/// principal.
pub const MUTABLE_SUBJECT_CLAIMS: [&str; 6] = [
    "email",
    "login",
    "username",
    "name",
    "preferred_username",
    "global_name",
];

/// How an avatar address is made from the profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvatarRule {
    /// None is kept.
    None,
    /// The profile carries an `https` URL under this claim (GitHub's
    /// `avatar_url`).
    Claim(String),
    /// Discord's: `https://cdn.discordapp.com/avatars/<id>/<avatar>.png`,
    /// from the `id` and `avatar` claims.
    DiscordCdn,
}

/// A connection's fixed settings. Everything a [`Preset`] fills, plus what
/// the app developer supplies (client id and secret) and what the deployment
/// derives (the redirect URI and the return list).
pub struct OAuth2Connection {
    /// The connection name: `[a-z0-9-]{1,32}`. What a page passes to `begin`
    /// and what an authenticator id and the `via` claim carry.
    pub name: String,
    pub client_id: String,
    pub client_secret: ClientSecret,
    pub authorize_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    /// Space-separated, as the provider expects it.
    pub scope: String,
    pub subject_claim: String,
    /// The address claim, used only with [`OAuth2Connection::email_verified_claim`].
    pub email_claim: Option<String>,
    pub email_verified_claim: Option<String>,
    /// The display name is the first of these that is a non-empty string.
    pub display_claims: Vec<String>,
    pub avatar: AvatarRule,
    /// `<ORDO_PUBLIC_URL>/auth/callback/<name>`, built once at boot and sent
    /// byte for byte at `begin` and at the exchange (O13).
    pub redirect_uri: String,
    /// Where a callback may send the browser back to, matched by origin
    /// with any path (owner ruling 2026-10-09, `subdomain-signin` ask 2).
    pub returns: crate::origin::OriginList,
    /// A3's acknowledgement: the deployment owner knows the subject claim
    /// names something mutable.
    pub mutable_subject_acknowledged: bool,
}

/// A connection is refused at boot for one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionError {
    BadName,
    /// A setting is missing or empty. Names the setting, never its value.
    Missing(&'static str),
    /// An endpoint or the redirect URI is not `https://`.
    NotHttps(&'static str),
    /// A3: the subject claim is mutable and nobody acknowledged it.
    MutableSubject(String),
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::BadName => {
                f.write_str("a connection name is 1 to 32 of a-z, 0-9 and -")
            }
            ConnectionError::Missing(what) => write!(f, "{what} is required"),
            ConnectionError::NotHttps(what) => write!(f, "{what} must be an https:// URL"),
            ConnectionError::MutableSubject(claim) => write!(
                f,
                "the subject claim `{claim}` names something the person or the provider can \
                 change, so a renamed account could become somebody else's; choose an \
                 immutable id claim, or acknowledge it with _MUTABLE_SUBJECT=acknowledged"
            ),
        }
    }
}

impl std::error::Error for ConnectionError {}

/// Is this a usable connection name?
pub fn valid_connection_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn https(url: &str) -> bool {
    url.len() > "https://".len()
        && url[..8].eq_ignore_ascii_case("https://")
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
        && !url.contains('#')
}

impl OAuth2Connection {
    /// Refuse a connection that cannot be served safely. Called at boot.
    pub fn check(&self) -> Result<(), ConnectionError> {
        if !valid_connection_name(&self.name) {
            return Err(ConnectionError::BadName);
        }
        if self.client_id.trim().is_empty() {
            return Err(ConnectionError::Missing("the client id"));
        }
        if self.client_secret.is_empty() {
            return Err(ConnectionError::Missing("the client secret"));
        }
        if self.subject_claim.trim().is_empty() {
            return Err(ConnectionError::Missing("the subject claim"));
        }
        for (what, url) in [
            ("the authorization endpoint", &self.authorize_url),
            ("the token endpoint", &self.token_url),
            ("the userinfo endpoint", &self.userinfo_url),
            ("the redirect URI", &self.redirect_uri),
        ] {
            if !https(url) {
                return Err(ConnectionError::NotHttps(what));
            }
        }
        if self.returns.is_empty() {
            return Err(ConnectionError::Missing("ORDO_AUTH_RETURN_URLS"));
        }
        if MUTABLE_SUBJECT_CLAIMS.contains(&self.subject_claim.as_str())
            && !self.mutable_subject_acknowledged
        {
            return Err(ConnectionError::MutableSubject(self.subject_claim.clone()));
        }
        Ok(())
    }

    /// What identifies the subject space this connection's ids live in: its
    /// kind, its three endpoints and its subject claim. A boot that finds a
    /// different fingerprint under the same name refuses (§3.5), because
    /// every existing `oauth2:<name>:<sub>` would otherwise name a subject of
    /// a different provider.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest as _, Sha256};
        let mut h = Sha256::new();
        for part in [
            "oauth2",
            self.authorize_url.as_str(),
            self.token_url.as_str(),
            self.userinfo_url.as_str(),
            self.subject_claim.as_str(),
        ] {
            h.update((part.len() as u64).to_be_bytes());
            h.update(part.as_bytes());
        }
        crate::webauthn::b64url_encode(&h.finalize())
    }

    /// Every host this connection talks to: the egress allowlist for its
    /// transport (pin 6), and nothing else.
    pub fn hosts(&self) -> Vec<String> {
        let mut out: Vec<String> = [&self.token_url, &self.userinfo_url]
            .iter()
            .filter_map(|u| host_of(u))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// The host of an `https://host[:port]/...` URL, lowercased.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.get(8..)?;
    let authority = rest.split(['/', '?']).next()?;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// What a preset fills: everything but the client id and secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub userinfo_url: &'static str,
    pub scope: &'static str,
    pub subject_claim: &'static str,
    pub email_claim: Option<&'static str>,
    pub email_verified_claim: Option<&'static str>,
    pub display_claims: &'static [&'static str],
    pub avatar: PresetAvatar,
}

/// [`AvatarRule`] in a form a constant can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetAvatar {
    None,
    Claim(&'static str),
    DiscordCdn,
}

impl PresetAvatar {
    pub fn rule(self) -> AvatarRule {
        match self {
            PresetAvatar::None => AvatarRule::None,
            PresetAvatar::Claim(c) => AvatarRule::Claim(c.to_string()),
            PresetAvatar::DiscordCdn => AvatarRule::DiscordCdn,
        }
    }
}

/// The presets, reviewed once (§5.3). The app developer may override any
/// field with its own setting.
pub fn preset(name: &str) -> Option<Preset> {
    match name {
        // Discord: `id` is a snowflake, a string. `email` counts only when
        // `verified` is true. The scope is denizens' minus `guilds`, which
        // nothing reads; an app that wants it sets the scope.
        "discord" => Some(Preset {
            authorize_url: "https://discord.com/oauth2/authorize",
            token_url: "https://discord.com/api/oauth2/token",
            userinfo_url: "https://discord.com/api/users/@me",
            scope: "identify email",
            subject_claim: "id",
            email_claim: Some("email"),
            email_verified_claim: Some("verified"),
            display_claims: &["global_name", "username"],
            avatar: PresetAvatar::DiscordCdn,
        }),
        // GitHub: `id` is an integer, rendered as text (A4). Its verified
        // flag lives on a second endpoint, so A5 leaves email out.
        "github" => Some(Preset {
            authorize_url: "https://github.com/login/oauth/authorize",
            token_url: "https://github.com/login/oauth/access_token",
            userinfo_url: "https://api.github.com/user",
            scope: "read:user",
            subject_claim: "id",
            email_claim: None,
            email_verified_claim: None,
            display_claims: &["name", "login"],
            avatar: PresetAvatar::Claim("avatar_url"),
        }),
        _ => None,
    }
}

/// The names [`preset`] knows.
pub const PRESETS: [&str; 2] = ["discord", "github"];

/// What a callback came to: where the browser goes, and with what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackOutcome {
    /// The `return_to` the ceremony was begun with. `None` when the ceremony
    /// was not found, so there is nowhere to send the browser.
    pub return_to: Option<String>,
    /// The handoff code, or why there is none.
    pub result: Result<String, AuthError>,
}

/// The OAuth2 provider for one connection.
pub struct OAuth2Provider {
    conn: OAuth2Connection,
    ceremonies: Arc<dyn CeremonyStore>,
    client: ProviderClient,
    ttl_secs: u64,
}

impl OAuth2Provider {
    /// A provider over a checked connection.
    pub fn new(
        conn: OAuth2Connection,
        ceremonies: Arc<dyn CeremonyStore>,
        transport: Arc<dyn FederationTransport>,
    ) -> Result<OAuth2Provider, ConnectionError> {
        conn.check()?;
        Ok(OAuth2Provider {
            conn,
            ceremonies,
            client: ProviderClient::new(transport),
            ttl_secs: CEREMONY_TTL_SECS,
        })
    }

    pub fn connection(&self) -> &OAuth2Connection {
        &self.conn
    }

    pub fn name(&self) -> &str {
        &self.conn.name
    }

    /// The return leg: consume the ceremony, exchange the code, read the
    /// profile, file a handoff.
    ///
    /// `error` is the provider's own `error` parameter (the person declined,
    /// for one). The ceremony is consumed whatever happens next, so a
    /// callback is good once.
    pub fn callback(
        &self,
        state: &str,
        code: Option<&str>,
        error: Option<&str>,
        now: Timestamp,
    ) -> CallbackOutcome {
        let taken = match self
            .ceremonies
            .take(&handle_of(state), &self.conn.name, PHASE_BEGUN, now)
        {
            Ok(Some(p)) => p,
            Ok(None) => {
                return CallbackOutcome {
                    return_to: None,
                    result: Err(AuthError::StateMismatch(String::new())),
                }
            }
            Err(e) => {
                return CallbackOutcome {
                    return_to: None,
                    result: Err(AuthError::StoreUnavailable(CredentialStoreError::from(e))),
                }
            }
        };
        let Some(begun) = Begun::parse(&taken) else {
            return CallbackOutcome {
                return_to: None,
                result: Err(AuthError::StateMismatch(String::new())),
            };
        };
        let result = self.finish(&begun, code, error, now);
        CallbackOutcome {
            return_to: Some(begun.return_to),
            result,
        }
    }

    fn finish(
        &self,
        begun: &Begun,
        code: Option<&str>,
        error: Option<&str>,
        now: Timestamp,
    ) -> Result<String, AuthError> {
        if error.is_some() {
            return Err(AuthError::UpstreamRefused("provider-answered-an-error"));
        }
        let code = code
            .filter(|c| !c.is_empty())
            .ok_or(AuthError::EmptyAuthorizationCode)?;
        let access = self.exchange(code, &begun.verifier)?;
        let profile = self.userinfo(&access)?;
        // The access token goes out of scope here and is never stored or
        // returned (§4.1: upstream tokens are never kept).
        drop(access);
        let subject = self.subject_from(&profile)?;
        write_handoff(
            self.ceremonies.as_ref(),
            &self.conn.name,
            &begun.binder_hash,
            &subject,
            now,
        )
    }

    /// The token exchange. Answers the access token, and nothing about why
    /// an exchange failed except a fixed word.
    fn exchange(&self, code: &str, verifier: &str) -> Result<String, AuthError> {
        let form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.conn.redirect_uri.as_str()),
            ("client_id", self.conn.client_id.as_str()),
            ("code_verifier", verifier),
        ];
        let got = self
            .client
            .token_exchange(&self.conn.token_url, form, &self.conn.client_secret)
            .map_err(|f| upstream(f, "token-exchange-rejected"))?;
        let v: Value = serde_json::from_slice(got.bytes())
            .map_err(|_| AuthError::UpstreamRefused("token-response-not-json"))?;
        let token = v
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .ok_or(AuthError::UpstreamRefused(
                "token-response-without-access-token",
            ))?;
        let bearer = v
            .get("token_type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("bearer"));
        if !bearer {
            return Err(AuthError::UpstreamRefused("token-type-not-bearer"));
        }
        Ok(token.to_string())
    }

    fn userinfo(&self, access: &str) -> Result<Value, AuthError> {
        let got = self
            .client
            .userinfo(&self.conn.userinfo_url, access)
            .map_err(|f| upstream(f, "userinfo-rejected"))?;
        let v: Value = serde_json::from_slice(got.bytes())
            .map_err(|_| AuthError::UpstreamRefused("userinfo-not-json"))?;
        if !v.is_object() {
            return Err(AuthError::UpstreamRefused("userinfo-not-an-object"));
        }
        Ok(v)
    }

    /// The subject and the profile, from a userinfo answer.
    pub fn subject_from(&self, profile: &Value) -> Result<AuthenticatedSubject, AuthError> {
        let subject = subject_of(profile.get(&self.conn.subject_claim))
            .ok_or(AuthError::UpstreamRefused("subject-claim"))?;
        let attributes = attributes_of(&self.conn, profile, &subject);
        Ok(AuthenticatedSubject::without_presence(
            AuthenticatorId(format!("oauth2:{}:{subject}", self.conn.name)),
            AuthMethodKind::OAuth2,
        )
        .with_attributes(attributes))
    }
}

/// A transport failure as a refusal: [`ProviderError::into_auth_error`].
/// The provider answering a final status (a code it did not issue, a token
/// it does not know) is a refused sign-in; a 5xx, a 429 or no answer at all
/// is the provider being unavailable, which the browser is told apart
/// because it says nothing about the person.
fn upstream(f: crate::federation::ProviderError, refused: &'static str) -> AuthError {
    f.into_auth_error(Some(refused))
}

/// A4: a JSON string or an integer, rendered canonically. A float, an
/// object, an array, `null`, `true`, an empty string or one with a control
/// character is no subject.
pub fn subject_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => {
            let ok = !s.is_empty() && s.len() <= 255 && !s.chars().any(char::is_control);
            ok.then(|| s.clone())
        }
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Some(u.to_string())
            } else {
                n.as_i64().map(|i| i.to_string())
            }
        }
        _ => None,
    }
}

/// What the connection keeps from a profile. Nothing here decides who the
/// person is.
fn attributes_of(conn: &OAuth2Connection, profile: &Value, subject: &str) -> SubjectAttributes {
    let text = |k: &str| {
        profile
            .get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let display = conn.display_claims.iter().find_map(|c| text(c)).map(|d| {
        d.chars()
            .filter(|c| !c.is_control())
            .take(120)
            .collect::<String>()
    });
    let avatar = match &conn.avatar {
        AvatarRule::None => None,
        AvatarRule::Claim(c) => text(c)
            .filter(|u| https(u) && u.len() <= 2048)
            .map(str::to_string),
        AvatarRule::DiscordCdn => text("avatar")
            .filter(|h| {
                h.len() <= 64
                    && h.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
            .filter(|_| subject.bytes().all(|b| b.is_ascii_digit()))
            .map(|h| format!("https://cdn.discordapp.com/avatars/{subject}/{h}.png")),
    };
    // A5: an address counts only when a verified claim is configured and is
    // the JSON boolean `true`. `"true"`, `1` and absence are unverified.
    let email = match (&conn.email_claim, &conn.email_verified_claim) {
        (Some(e), Some(flag)) if profile.get(flag) == Some(&Value::Bool(true)) => {
            text(e).and_then(|address| {
                VerifiedEmail::new(
                    address,
                    EmailEvidence::ProviderClaim {
                        claim: flag.clone(),
                    },
                )
            })
        }
        _ => None,
    };
    SubjectAttributes {
        email,
        display,
        avatar,
        assurance: None,
    }
}

/// What a `begun` row holds: the PKCE verifier, where to send the browser,
/// and the binder's hash the handoff will be filed under.
struct Begun {
    verifier: String,
    return_to: String,
    binder_hash: String,
}

impl Begun {
    fn render(&self) -> String {
        serde_json::json!({"v": self.verifier, "r": self.return_to, "b": self.binder_hash})
            .to_string()
    }

    fn parse(payload: &str) -> Option<Begun> {
        let v: Value = serde_json::from_str(payload).ok()?;
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Begun {
            verifier: text("v")?,
            return_to: text("r")?,
            binder_hash: text("b")?,
        })
    }
}

/// PKCE `S256`: base64url(SHA-256(verifier)).
pub fn code_challenge(verifier: &str) -> String {
    use sha2::{Digest as _, Sha256};
    crate::webauthn::b64url_encode(&Sha256::digest(verifier.as_bytes()))
}

/// A shared provider is a provider, so the registry can hold one handle and
/// the callback route, which needs the concrete type, another.
impl AuthProvider for Arc<OAuth2Provider> {
    fn kind(&self) -> AuthMethodKind {
        (**self).kind()
    }

    fn begin(&self, req: &BeginRequest, now: Timestamp) -> Result<ChallengeOrRedirect, AuthError> {
        (**self).begin(req, now)
    }

    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        (**self).complete(resp, now)
    }
}

impl AuthProvider for OAuth2Provider {
    fn kind(&self) -> AuthMethodKind {
        AuthMethodKind::OAuth2
    }

    fn begin(&self, req: &BeginRequest, now: Timestamp) -> Result<ChallengeOrRedirect, AuthError> {
        let return_to = req
            .return_to
            .as_deref()
            .filter(|r| !r.contains('#') && self.conn.returns.allows_url(r))
            .ok_or(AuthError::ReturnNotAllowed)?;
        let state = random_secret();
        let verifier = random_secret();
        let binder = random_secret();
        let begun = Begun {
            verifier,
            return_to: return_to.to_string(),
            binder_hash: handle_of(&binder),
        };
        self.ceremonies
            .put(&CeremonyRow {
                handle: handle_of(&state),
                connection: self.conn.name.clone(),
                phase: PHASE_BEGUN.to_string(),
                binder: begun.binder_hash.clone(),
                payload: begun.render(),
                minted_at: now,
                expires_at: Timestamp(now.0.saturating_add(self.ttl_secs)),
            })
            .map_err(|e| AuthError::StoreUnavailable(CredentialStoreError::from(e)))?;
        let query = form_encode(&[
            ("response_type", "code"),
            ("client_id", &self.conn.client_id),
            ("redirect_uri", &self.conn.redirect_uri),
            ("scope", &self.conn.scope),
            ("state", &state),
            ("code_challenge", &code_challenge(&begun.verifier)),
            ("code_challenge_method", "S256"),
        ]);
        let joiner = if self.conn.authorize_url.contains('?') {
            '&'
        } else {
            '?'
        };
        Ok(ChallengeOrRedirect::Redirect {
            url: format!("{}{joiner}{query}", self.conn.authorize_url),
            binder,
        })
    }

    /// Redeem a handoff. The only completion this provider takes: the code
    /// and the callback are the callback route's, never a caller's (A2).
    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        match resp {
            CompleteRequest::Handoff { handoff, binder } => redeem_handoff(
                self.ceremonies.as_ref(),
                &self.conn.name,
                handoff,
                binder,
                now,
            ),
            _ => Err(AuthError::MethodMismatch {
                provider: AuthMethodKind::OAuth2,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ceremony::MemoryCeremonyStore;
    use crate::federation::{
        FederationMethod, FederationRequest, FederationResponse, ProviderError,
    };
    use std::sync::Mutex;

    #[test]
    fn a_provider_outage_or_rate_limit_is_unavailable_and_a_final_status_is_refused() {
        for status in [500u16, 502, 503, 429, 408] {
            let e = upstream(
                ProviderError::answered(status, true),
                "token-exchange-rejected",
            );
            assert_eq!(
                e,
                AuthError::UpstreamUnavailable("provider-busy"),
                "{status}"
            );
        }
        for status in [400u16, 401, 403] {
            let e = upstream(
                ProviderError::answered(status, false),
                "token-exchange-rejected",
            );
            assert_eq!(
                e,
                AuthError::UpstreamRefused("token-exchange-rejected"),
                "{status}"
            );
        }
        let e = upstream(ProviderError::unreachable("connect_failed"), "x");
        assert_eq!(e, AuthError::UpstreamUnavailable("connect_failed"));
    }

    /// A stand-in for the provider: answers the token exchange and the
    /// userinfo call from fixed bodies, and remembers what it was sent.
    struct Fake {
        token: &'static str,
        profile: String,
        seen: Mutex<Vec<(String, String, Option<String>)>>,
    }

    impl FederationTransport for Fake {
        fn fetch(&self, req: &FederationRequest<'_>) -> Result<FederationResponse, ProviderError> {
            let form = form_encode(&req.form);
            let secret = req.client_secret.map(|s| s.reveal_to_wire().to_string());
            self.seen
                .lock()
                .unwrap()
                .push((req.url.to_string(), form, secret));
            let body = match req.method {
                FederationMethod::PostForm => self.token.as_bytes().to_vec(),
                FederationMethod::Get => self.profile.as_bytes().to_vec(),
            };
            Ok(FederationResponse::new(body))
        }
    }

    fn connection(name: &str, p: &Preset) -> OAuth2Connection {
        OAuth2Connection {
            name: name.into(),
            client_id: "client-1".into(),
            client_secret: ClientSecret::new("s3cret".into()),
            authorize_url: p.authorize_url.into(),
            token_url: p.token_url.into(),
            userinfo_url: p.userinfo_url.into(),
            scope: p.scope.into(),
            subject_claim: p.subject_claim.into(),
            email_claim: p.email_claim.map(str::to_string),
            email_verified_claim: p.email_verified_claim.map(str::to_string),
            display_claims: p.display_claims.iter().map(|s| s.to_string()).collect(),
            avatar: p.avatar.rule(),
            redirect_uri: format!("https://api.example.com/auth/callback/{name}"),
            returns: crate::origin::OriginList::parse(
                "https://app.example.com/signed-in",
                true,
                false,
            )
            .expect("a return list"),
            mutable_subject_acknowledged: false,
        }
    }

    fn discord(profile: &str) -> (OAuth2Provider, Arc<Fake>) {
        let fake = Arc::new(Fake {
            token: r#"{"access_token":"at-1","token_type":"Bearer"}"#,
            profile: profile.to_string(),
            seen: Mutex::new(Vec::new()),
        });
        let p = OAuth2Provider::new(
            connection("discord", &preset("discord").unwrap()),
            Arc::new(MemoryCeremonyStore::new()),
            fake.clone(),
        )
        .expect("a valid connection");
        (p, fake)
    }

    fn begin(p: &OAuth2Provider) -> (String, String, String) {
        let Ok(ChallengeOrRedirect::Redirect { url, binder }) = p.begin(
            &BeginRequest {
                identifier: None,
                return_to: Some("https://app.example.com/signed-in".into()),
                presence: false,
            },
            Timestamp(1000),
        ) else {
            panic!("a redirect");
        };
        let state = url
            .split("state=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_string();
        (url, state, binder)
    }

    const NELLY: &str = r#"{"id":"80351110224678912","username":"nelly","global_name":"Nelly",
        "avatar":"8342729096ea3675442027381ff50dfe","email":"nelly@example.com","verified":true}"#;

    #[test]
    fn a_discord_round_trip_yields_the_subject_and_its_profile() {
        let (p, fake) = discord(NELLY);
        let (url, state, binder) = begin(&p);
        assert!(url.starts_with("https://discord.com/oauth2/authorize?response_type=code&"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fapi.example.com%2Fauth%2Fcallback%2Fdiscord")
        );
        let out = p.callback(&state, Some("code-1"), None, Timestamp(1001));
        assert_eq!(
            out.return_to.as_deref(),
            Some("https://app.example.com/signed-in")
        );
        let handoff = out.result.expect("a handoff");
        let s = p
            .complete(
                &CompleteRequest::Handoff {
                    handoff,
                    binder: binder.clone(),
                },
                Timestamp(1002),
            )
            .expect("redeemed");
        assert_eq!(s.authenticator.0, "oauth2:discord:80351110224678912");
        assert_eq!(s.method, AuthMethodKind::OAuth2);
        assert!(
            s.presence.is_none(),
            "an outside provider proves no presence"
        );
        assert_eq!(s.attributes.display.as_deref(), Some("Nelly"));
        assert_eq!(
            s.attributes.avatar.as_deref(),
            Some("https://cdn.discordapp.com/avatars/80351110224678912/8342729096ea3675442027381ff50dfe.png")
        );
        assert_eq!(
            s.attributes.email.as_ref().map(|e| e.address()),
            Some("nelly@example.com")
        );
        // The exchange carried the verifier, the exact redirect URI and the
        // secret; the userinfo call carried no secret.
        let seen = fake.seen.lock().unwrap();
        assert!(seen[0].1.contains("code_verifier="));
        assert!(seen[0].1.contains("grant_type=authorization_code"));
        assert_eq!(seen[0].2.as_deref(), Some("s3cret"));
        assert_eq!(seen[1].2, None);
    }

    #[test]
    fn the_subject_is_the_configured_claim_and_nothing_else() {
        // An `id` that is a float, an object, null or absent is no subject,
        // whatever else the profile says.
        for bad in [
            r#"{"id":1.5,"username":"x"}"#,
            r#"{"id":{"a":1}}"#,
            r#"{"id":null,"email":"a@b.c","verified":true}"#,
            r#"{"username":"x","sub":"y"}"#,
            r#"{"id":""}"#,
        ] {
            let (p, _) = discord(bad);
            let (_, state, _) = begin(&p);
            let out = p.callback(&state, Some("c"), None, Timestamp(1001));
            assert_eq!(
                out.result,
                Err(AuthError::UpstreamRefused("subject-claim")),
                "{bad}"
            );
        }
        // An integer is rendered as text, so `123` and `"123"` are one person.
        assert_eq!(
            subject_of(Some(&serde_json::json!(123))).as_deref(),
            Some("123")
        );
        assert_eq!(
            subject_of(Some(&serde_json::json!("123"))).as_deref(),
            Some("123")
        );
    }

    #[test]
    fn an_unverified_address_is_not_kept() {
        for profile in [
            r#"{"id":"1","email":"a@b.c","verified":false}"#,
            r#"{"id":"1","email":"a@b.c","verified":"true"}"#,
            r#"{"id":"1","email":"a@b.c"}"#,
        ] {
            let (p, _) = discord(profile);
            let s = p
                .subject_from(&serde_json::from_str(profile).unwrap())
                .unwrap();
            assert!(s.attributes.email.is_none(), "{profile}");
        }
    }

    #[test]
    fn a_state_is_good_once_and_a_return_address_must_be_listed() {
        let (p, _) = discord(NELLY);
        let (_, state, _) = begin(&p);
        assert!(p
            .callback(&state, Some("c"), None, Timestamp(1001))
            .result
            .is_ok());
        let again = p.callback(&state, Some("c"), None, Timestamp(1001));
        assert_eq!(again.return_to, None);
        assert!(again.result.is_err());
        for bad in [
            None,
            Some("https://app.example.com.evil.example/signed-in"),
            Some("https://app.example.com@evil.example/"),
            Some("https://app.example.com/signed-in#x"),
            Some("https://evil.example/"),
        ] {
            assert_eq!(
                p.begin(
                    &BeginRequest {
                        identifier: None,
                        return_to: bad.map(str::to_string),
                        presence: false,
                    },
                    Timestamp(1000)
                ),
                Err(AuthError::ReturnNotAllowed)
            );
        }
    }

    #[test]
    fn a_provider_error_or_a_missing_code_files_no_handoff() {
        let (p, fake) = discord(NELLY);
        let (_, state, _) = begin(&p);
        let out = p.callback(&state, None, Some("access_denied"), Timestamp(1001));
        assert!(
            out.return_to.is_some(),
            "the browser is sent back with an error"
        );
        assert!(out.result.is_err());
        assert!(fake.seen.lock().unwrap().is_empty(), "no request was made");
    }

    #[test]
    fn a_mutable_subject_claim_is_refused_unless_acknowledged() {
        let mut c = connection("gh", &preset("github").unwrap());
        c.subject_claim = "login".into();
        assert_eq!(
            c.check(),
            Err(ConnectionError::MutableSubject("login".into()))
        );
        c.mutable_subject_acknowledged = true;
        assert_eq!(c.check(), Ok(()));
    }

    #[test]
    fn github_has_no_email_and_an_integer_id() {
        let fake = Arc::new(Fake {
            token: r#"{"access_token":"gho_x","token_type":"bearer","scope":"read:user"}"#,
            profile: r#"{"id":583231,"login":"octocat","name":null,"avatar_url":"https://avatars.githubusercontent.com/u/583231?v=4","email":"octocat@github.com"}"#.into(),
            seen: Mutex::new(Vec::new()),
        });
        let p = OAuth2Provider::new(
            connection("github", &preset("github").unwrap()),
            Arc::new(MemoryCeremonyStore::new()),
            fake,
        )
        .unwrap();
        let (_, state, binder) = begin(&p);
        let handoff = p
            .callback(&state, Some("c"), None, Timestamp(1001))
            .result
            .unwrap();
        let s = p
            .complete(
                &CompleteRequest::Handoff { handoff, binder },
                Timestamp(1001),
            )
            .unwrap();
        assert_eq!(s.authenticator.0, "oauth2:github:583231");
        assert_eq!(s.attributes.display.as_deref(), Some("octocat"));
        assert!(
            s.attributes.email.is_none(),
            "A5: GitHub's flag is elsewhere"
        );
    }

    #[test]
    fn the_fingerprint_moves_with_the_endpoints_and_the_subject_claim() {
        let a = connection("discord", &preset("discord").unwrap());
        let mut b = connection("discord", &preset("discord").unwrap());
        assert_eq!(a.fingerprint(), b.fingerprint());
        b.client_id = "another".into();
        assert_eq!(
            a.fingerprint(),
            b.fingerprint(),
            "the client id is not identity"
        );
        b.userinfo_url = "https://evil.example/me".into();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn hosts_are_parsed_without_ports_or_paths() {
        assert_eq!(
            host_of("https://Discord.com/api/x").as_deref(),
            Some("discord.com")
        );
        assert_eq!(
            host_of("https://localhost:8443/t?x").as_deref(),
            Some("localhost")
        );
        assert_eq!(host_of("https://[::1]:8443/t").as_deref(), Some("::1"));
    }
}
