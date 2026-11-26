//! The identity service as an OIDC provider for app users (owner,
//! 2026-11-17, ask 3: "the identity service is Ordo's built-in credential
//! system, and it signs in app users as well as deployment people").
//!
//! A deployment registered here ([`crate::deployments`]) is an OIDC client.
//! Its `client_id` is its handle, and its redirect URIs are its return
//! URLs, each matched exactly: scheme, host, port and path, with no query
//! ([`crate::deployments::Registered::allows_redirect`]). An app registers
//! its callback as a return URL only, never as a client origin, so its
//! page gets no credentialed call to the service. An app connects to the
//! service with Ordo's own OIDC client, `ORDO_AUTH_<NAME>_KIND=oidc`.
//!
//! | Route | What |
//! |---|---|
//! | `GET /.well-known/openid-configuration` | the discovery document |
//! | `GET /oidc/authorize` | the authorization code flow, PKCE `S256` required |
//! | `POST /oidc/token` | a code and its PKCE verifier for an ID token; a deployment's token exchange and client credentials grants ([`crate::exchange`]) |
//!
//! The rules:
//!
//! - The issuer is where the service is served: `ORDO_IDENTITY_ORIGIN`
//!   and `ORDO_IDENTITY_BASE_PATH`, and discovery is at
//!   `<issuer>/.well-known/openid-configuration`. With no base path it is
//!   the origin, the `iss` deployment tokens carry. Under a base path the
//!   two differ: deployment tokens keep the origin, which doors check
//!   against `ORDO_IDENTITY_ORIGIN`, and two services under different
//!   paths on one origin are two OIDC issuers, so an app's peer never
//!   reads one's subjects as the other's. `sub` is the person's id here,
//!   the same for every deployment, so two apps connected under different
//!   connection names see one person as one subject (`ordo-exec`'s
//!   `peer`).
//! - An unknown client or a redirect URI the deployment does not list is
//!   refused on the service's own page and never followed. Any other
//!   refusal goes back to the app as RFC 6749's `error`.
//! - Only a live member of the deployment, `admin` or `member`, gets a
//!   code, and only one still a member when the code is spent gets an ID
//!   token. A person not signed in goes to the page's sign-in, or to its
//!   sign-up when the deployment's mode is `open`, and comes back.
//! - A code is 256 random bits, lives [`CODE_SECS`], is spent by its
//!   first exchange whatever the outcome, and is bound to the client, the
//!   redirect URI, the PKCE challenge and the nonce. Only its tagged hash
//!   is stored. A code is too long to guess, so the token endpoint is not
//!   counted by the guessing limits; an app's server makes every exchange
//!   from one address and would trip them.
//! - Clients are public: PKCE proves the exchange, and there is no client
//!   secret to manage. The code only ever reaches a return URL the
//!   deployment owner registered, and only the app that began the sign-in
//!   holds the verifier. A deployment calling another proves itself
//!   with a client assertion instead ([`crate::exchange`]); that is the
//!   one place a client authenticates.
//! - The ID token is EdDSA under the service's token keys, `typ` `JWT`.
//!   Its `aud` is the bare handle and it carries none of a deployment
//!   token's claims, so a deployment token verifier refuses it.
//! - A browser session gets at most `ORDO_IDENTITY_AUTHORIZE_PER_SESSION`
//!   codes a minute; past it, the app is sent `temporarily_unavailable`.
//!   The minute tick deletes spent and expired codes ([`sweep_codes`]).
//! - Each exchange is an audit row, `oidc.token`, `ok` or `refused`. An
//!   authorization writes none: a refused row needs a live code, and codes
//!   are limited per session, so the log grows with sign-ins only.

use std::collections::HashMap;

use ed25519_dalek::Signer as _;
use ordo_exec::sql_lit;
use ordo_identity::federation::{form_encode, percent, percent_decode};
use ordo_identity::origin::plain_url_text;
use ordo_identity::peer_token;
use ordo_identity::webauthn::b64url_encode;
use serde_json::{json, Value};

use crate::audit::{self, Entry};
use crate::cli::{pkce_challenge, valid_challenge};
use crate::deployments::{self, Registered, SignUp};
use crate::error::IdentityError;
use crate::secrets::{mint_secret, oidc_code_hash};
use crate::server::Service;
use crate::sessions::{self, SessionKind};
use crate::store::{dep, num, text};

/// A code lives 60 seconds.
pub const CODE_SECS: u64 = 60;
pub const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";
pub const AUTHORIZE_PATH: &str = "/oidc/authorize";
pub const TOKEN_PATH: &str = "/oidc/token";
/// The scopes served. `profile` adds `name`.
pub const SCOPES: &[&str] = &["openid", "profile"];
/// The ID token header's `typ`. A deployment token's is
/// `ordo-deployment+jwt`.
pub const ID_TOKEN_TYP: &str = "JWT";
const MAX_NONCE: usize = 255;
const MAX_STATE: usize = 1024;

/// A query string or form body as name to value. A name given twice is
/// refused (RFC 6749 3.1).
pub fn form(body: &str) -> Result<HashMap<String, String>, IdentityError> {
    let mut out = HashMap::new();
    for pair in body.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (Some(k), Some(v)) = (percent_decode(k), percent_decode(v)) else {
            return Err(IdentityError::BadRequest(
                "a parameter is not form-encoded".into(),
            ));
        };
        if out.insert(k.clone(), v).is_some() {
            return Err(IdentityError::BadRequest(format!(
                "`{k}` is given more than once"
            )));
        }
    }
    Ok(out)
}

/// Where an authorization request sends the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authorized {
    /// Back to the app: the redirect URI with a code, or with an `error`.
    Back(String),
    /// To the service's page to sign in or sign up, which comes back here.
    Page(String),
}

fn refused(error: &'static str, why: impl Into<String>) -> IdentityError {
    IdentityError::AuthorizeRefused {
        error,
        why: why.into(),
    }
}

/// `url` with `pairs` added to its query.
fn with_query(url: &str, pairs: &[(&str, &str)]) -> String {
    let joiner = if url.contains('?') { '&' } else { '?' };
    format!("{url}{joiner}{}", form_encode(pairs))
}

impl Service {
    /// The OIDC issuer: the origin and the base path, where the service is
    /// served. With no base path it is `ORDO_IDENTITY_ORIGIN`, the `iss`
    /// deployment tokens carry; under one, deployment tokens keep the
    /// origin.
    pub fn oidc_issuer(&self) -> String {
        self.config().public_url()
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.config().public_url())
    }

    /// The discovery document. Every value is what the service does.
    pub fn discovery(&self) -> Value {
        json!({
            "issuer": self.oidc_issuer(),
            "authorization_endpoint": self.endpoint(AUTHORIZE_PATH),
            "token_endpoint": self.endpoint(TOKEN_PATH),
            "jwks_uri": self.endpoint("/.well-known/jwks.json"),
            "response_types_supported": ["code"],
            "response_modes_supported": ["query"],
            "grant_types_supported": [
                "authorization_code",
                peer_token::GRANT_TOKEN_EXCHANGE,
                peer_token::GRANT_CLIENT_CREDENTIALS,
            ],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["EdDSA"],
            "scopes_supported": SCOPES,
            "claims_supported": ["iss", "sub", "aud", "exp", "iat", "auth_time", "nonce", "name"],
            "code_challenge_methods_supported": ["S256"],
            // `none` for an app signing a person in; `private_key_jwt` for a
            // deployment calling another ([`crate::exchange`]).
            "token_endpoint_auth_methods_supported": ["none", "private_key_jwt"],
            "token_endpoint_auth_signing_alg_values_supported": ["EdDSA"],
            "authorization_response_iss_parameter_supported": true,
        })
    }

    /// An authorization request: `query` as the browser sent it, and the
    /// browser's session secret if it carries one. An `Err` is shown on the
    /// service's page; every refusal after the client and redirect URI
    /// check out goes back to the app.
    pub async fn authorize(
        &self,
        query: &str,
        session: Option<&str>,
    ) -> Result<Authorized, IdentityError> {
        let q = form(query)?;
        let get = |k: &str| q.get(k).map(String::as_str).filter(|v| !v.is_empty());
        let client = get("client_id")
            .ok_or_else(|| IdentityError::BadRequest("`client_id` is required".into()))?;
        let Some(d) = self.registered().into_iter().find(|d| d.handle == client) else {
            return Err(IdentityError::UnknownClient(client.to_string()));
        };
        let redirect = get("redirect_uri")
            .filter(|r| plain_url_text(r) && d.allows_redirect(r))
            .ok_or_else(|| IdentityError::RedirectRefused {
                client: d.handle.clone(),
            })?;
        let state = get("state");
        match self.authorize_for(&d, &q, query, redirect, session).await {
            Ok(a) => Ok(a),
            Err(e @ IdentityError::AuthorizeRefused { .. }) => {
                let message = e.to_string();
                let mut pairs = vec![("error", e.code()), ("error_description", message.as_str())];
                if let Some(s) = state {
                    pairs.push(("state", s));
                }
                let iss = self.oidc_issuer();
                pairs.push(("iss", &iss));
                Ok(Authorized::Back(with_query(redirect, &pairs)))
            }
            Err(e) => Err(e),
        }
    }

    async fn authorize_for(
        &self,
        d: &Registered,
        q: &HashMap<String, String>,
        query: &str,
        redirect: &str,
        session: Option<&str>,
    ) -> Result<Authorized, IdentityError> {
        let get = |k: &str| q.get(k).map(String::as_str).filter(|v| !v.is_empty());
        if get("response_type") != Some("code") {
            return Err(refused(
                "unsupported_response_type",
                "`response_type` must be `code`",
            ));
        }
        let scope = get("scope").unwrap_or_default();
        if !scope.split(' ').any(|s| s == "openid") {
            return Err(refused("invalid_scope", "`scope` must include `openid`"));
        }
        let challenge = get("code_challenge").unwrap_or_default();
        if get("code_challenge_method") != Some("S256") || !valid_challenge(challenge) {
            return Err(refused(
                "invalid_request",
                "PKCE is required: `code_challenge` (S256, base64url of 32 bytes) and \
                 `code_challenge_method=S256`",
            ));
        }
        let nonce = get("nonce")
            .filter(|n| n.len() <= MAX_NONCE && !n.chars().any(char::is_control))
            .ok_or_else(|| {
                refused(
                    "invalid_request",
                    format!("`nonce` is required, at most {MAX_NONCE} characters"),
                )
            })?;
        if get("state").is_some_and(|s| s.len() > MAX_STATE) {
            return Err(refused(
                "invalid_request",
                format!("`state` is at most {MAX_STATE} characters"),
            ));
        }
        let live = match session {
            None => None,
            Some(secret) => match self.browser(secret).await {
                Ok(l) => Some(l),
                Err(IdentityError::NotSignedIn | IdentityError::SessionEnded(_)) => None,
                Err(e) => return Err(e),
            },
        };
        let open = d.sign_up == SignUp::Open;
        // The page comes back to this very request once signed in.
        let here = format!("{AUTHORIZE_PATH}?{query}");
        let Some(live) = live.filter(|l| l.kind == SessionKind::Browser && !l.must_enrol) else {
            return Ok(Authorized::Page(self.page_link("/sign-in", d, &here, open)));
        };
        let pool = self.store().pool();
        let role = deployments::role_of(pool, self.handle(), &d.handle, &live.subject).await?;
        if role.is_none() {
            if open {
                // Signed in, not yet a member: the sign-up page joins them.
                return Ok(Authorized::Page(
                    self.page_link("/sign-up", d, &here, false),
                ));
            }
            return Err(refused(
                "access_denied",
                format!(
                    "this account is not a member of `{}`, which takes people by invite; ask \
                     the people who run it for an invite link",
                    d.handle
                ),
            ));
        }
        let now = self.now();
        // Each code is a row: one session gets so many a minute.
        match crate::throttle::attempt_per_session(
            pool,
            self.handle(),
            crate::throttle::Try::Authorize,
            &live.id,
            &self.config().limits,
            now,
        )
        .await
        {
            Ok(()) => {}
            Err(IdentityError::Throttled { retry_after_secs }) => {
                return Err(refused(
                    "temporarily_unavailable",
                    format!(
                        "this browser asked to sign in too many times in a minute; try again \
                         in {retry_after_secs} seconds"
                    ),
                ))
            }
            Err(e) => return Err(e),
        }
        let code = mint_secret();
        let auth_time = live.touched_at.unwrap_or(now);
        pool.rows(&format!(
            "INSERT INTO oidc_codes (code_hash, deployment, client, person_id, session_id, \
             redirect_uri, code_challenge, nonce, scope, auth_time, expires_at, created_at) \
             VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {auth_time}, {}, {now})",
            sql_lit(&oidc_code_hash(&code)),
            sql_lit(self.handle()),
            sql_lit(&d.handle),
            sql_lit(&live.subject),
            sql_lit(&live.id),
            sql_lit(redirect),
            sql_lit(challenge),
            sql_lit(nonce),
            sql_lit(scope),
            now + CODE_SECS,
        ))
        .await?;
        let mut pairs = vec![("code", code.as_str())];
        if let Some(s) = get("state") {
            pairs.push(("state", s));
        }
        let iss = self.oidc_issuer();
        pairs.push(("iss", &iss));
        Ok(Authorized::Back(with_query(redirect, &pairs)))
    }

    /// The page at `path` for `d`, coming back to `here`. With `sign_up`,
    /// the sign-in page also links to making an account.
    fn page_link(&self, path: &str, d: &Registered, here: &str, sign_up: bool) -> String {
        let mut link = format!(
            "{}{path}?deployment={}&return={}",
            self.config().base_path,
            percent(&d.handle),
            percent(here)
        );
        if sign_up {
            link.push_str("&signUp=open");
        }
        link
    }

    /// The token request: a form body. Answers the token response.
    pub async fn oidc_token(&self, body: &str) -> Result<Value, IdentityError> {
        let f = form(body)?;
        let get = |k: &str| f.get(k).map(String::as_str).filter(|v| !v.is_empty());
        if f.contains_key("client_secret") {
            return Err(IdentityError::ClientSecretRefused);
        }
        match get("grant_type") {
            Some("authorization_code") => {}
            Some(peer_token::GRANT_TOKEN_EXCHANGE) => return self.token_exchange(&f).await,
            Some(peer_token::GRANT_CLIENT_CREDENTIALS) => return self.client_token(&f).await,
            other => {
                return Err(IdentityError::UnsupportedGrant(
                    other.unwrap_or_default().to_string(),
                ))
            }
        }
        let need =
            |k: &str| get(k).ok_or_else(|| IdentityError::BadRequest(format!("`{k}` is required")));
        let (code, redirect, client, verifier) = (
            need("code")?,
            need("redirect_uri")?,
            need("client_id")?,
            need("code_verifier")?,
        );
        if !self.registered().iter().any(|d| d.handle == client) {
            return Err(IdentityError::UnknownClient(client.to_string()));
        }
        let now = self.now();
        let pool = self.store().pool();
        let tx = pool.begin().await?;
        // Spent by this try, whatever its outcome.
        let rows = tx
            .rows(&format!(
                "UPDATE oidc_codes SET used_at = {now} WHERE {} AND code_hash = {} \
                 AND used_at IS NULL RETURNING client, person_id, session_id, redirect_uri, \
                 code_challenge, nonce, scope, auth_time, expires_at",
                dep(self.handle()),
                sql_lit(&oidc_code_hash(code)),
            ))
            .await?;
        let Some(r) = rows.first() else {
            return Err(IdentityError::GrantRefused);
        };
        let person = text(r, 1);
        let session = text(r, 2);
        let verifier_ok =
            (43..=128).contains(&verifier.len()) && pkce_challenge(verifier) == text(r, 4);
        let why = if text(r, 0) != client {
            "another client"
        } else if text(r, 3) != redirect {
            "another redirect URI"
        } else if !verifier_ok {
            "the PKCE verifier"
        } else if num(r, 8).is_none_or(|exp| exp <= now) {
            "expired"
        } else if deployments::role_of(pool, self.handle(), client, &person)
            .await?
            .is_none()
        {
            "not a member"
        } else if !sessions::is_live(pool, self.handle(), &session).await? {
            "session ended"
        } else {
            ""
        };
        let entry = |outcome, detail| Entry {
            actor: "",
            action: "oidc.token",
            subject: client,
            outcome,
            client: "",
            detail,
        };
        if !why.is_empty() {
            audit::append(
                &tx,
                self.handle(),
                self.keys(),
                &Entry {
                    actor: &person,
                    ..entry("refused", why)
                },
                now,
            )
            .await?;
            tx.commit().await?;
            return Err(IdentityError::GrantRefused);
        }
        let scope = text(r, 6);
        let mut claims = json!({
            "iss": self.oidc_issuer(),
            "sub": person,
            "aud": client,
            "exp": now + self.config().token_secs,
            "iat": now,
            "auth_time": num(r, 7).unwrap_or(now),
            "nonce": text(r, 5),
        });
        if scope.split(' ').any(|s| s == "profile") {
            if let Some(p) = crate::people::get(pool, self.handle(), &person).await? {
                claims["name"] = json!(p.display_name);
            }
        }
        let id_token = self.sign_id_token(&claims, now)?;
        audit::append(
            &tx,
            self.handle(),
            self.keys(),
            &Entry {
                actor: &person,
                ..entry("ok", redirect)
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(json!({
            "id_token": id_token,
            // RFC 6749 requires one. It is random, stored nowhere and
            // accepted by nothing: there is no userinfo endpoint.
            "access_token": mint_secret(),
            "token_type": "Bearer",
            "expires_in": self.config().token_secs,
            "scope": scope,
        }))
    }

    /// Sign an ID token with the active token key.
    fn sign_id_token(&self, claims: &Value, now: u64) -> Result<String, IdentityError> {
        let (kid, key) = self.keys().token_signer(now)?;
        let header = json!({ "alg": "EdDSA", "kid": kid, "typ": ID_TOKEN_TYP });
        let input = format!(
            "{}.{}",
            b64url_encode(header.to_string().as_bytes()),
            b64url_encode(claims.to_string().as_bytes())
        );
        let sig = key.sign(input.as_bytes());
        Ok(format!("{input}.{}", b64url_encode(&sig.to_bytes())))
    }
}

/// Delete the codes no exchange can spend: used ones and expired ones.
/// Run on the minute tick, so the table holds about a minute of sign-ins.
pub async fn sweep_codes(
    pool: &ordo_exec::Pool,
    handle: &str,
    now: u64,
) -> Result<u64, IdentityError> {
    Ok(pool
        .rows(&format!(
            "DELETE FROM oidc_codes WHERE {} AND (used_at IS NOT NULL OR expires_at <= {now}) \
             RETURNING 1",
            dep(handle)
        ))
        .await?
        .len() as u64)
}

/// The service's page for an authorization request it will not follow:
/// an unknown client, a redirect URI not registered, a malformed request.
pub fn refusal_page(e: &IdentityError, base: &str) -> String {
    let message = crate::page::attr(&e.to_string());
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"color-scheme\" content=\"light dark\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <title>Sign-in refused · Ordo</title>\n\
         <link rel=\"stylesheet\" href=\"{base}/page/identity.css\">\n</head>\n<body>\n\
         <header class=\"brand\"><span class=\"mark\">Ordo</span> sign-in</header>\n<main>\n\
         <h1>This sign-in link does not work</h1>\n\
         <p class=\"problem\" role=\"alert\">{message}</p>\n\
         <p>Go back to the app and try again. If it happens again, tell the people who run \
         the app. The words above say what to change.</p>\n</main>\n</body>\n</html>\n"
    )
}
