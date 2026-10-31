//! The OIDC return leg (`plan-auth-federated.md` §4.1): the OIDC arm of
//! `GET /auth/callback/{connection}`, and the typed connections it needs.
//!
//! Same contract as OAuth2's ([`super::federated`]): a `303` to the
//! `return_to` the ceremony began with, carrying `handoff=<code>`, or
//! `error=authentication_failed` or `error=identity_provider_unavailable`.
//! The reason goes to the security stream and nowhere else.

use std::collections::HashMap;
use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::oidc::{OidcCallback, OidcProvider};
use ordo_identity::{AuthError, Timestamp};

use super::federated::back_to;
use super::{auth_error_reason, refused, AuthState, SharedAuth};

impl AuthState {
    /// Give the callback route an OIDC connection. The same provider must be
    /// in the registry under the same name, or `begin` cannot reach it.
    pub fn with_oidc(mut self, provider: Arc<OidcProvider>) -> AuthState {
        self.oidc.insert(provider.name().to_string(), provider);
        self
    }

    /// The OIDC connection named `name`, typed.
    pub fn oidc(&self, name: &str) -> Option<Arc<OidcProvider>> {
        self.oidc.get(name).cloned()
    }
}

/// `503 identity_provider_unavailable`: decided before any identity is
/// known, so safe to keep distinct (§3.6). It names the connection and,
/// where a setting would fix it, the setting and its value. The specific
/// word is recorded.
pub(super) fn upstream_unavailable(
    auth: &AuthState,
    op: &str,
    connection: &str,
    why: &str,
    now: Timestamp,
) -> Response {
    auth.record_refusal(op, &format!("identity-provider-unavailable:{why}"), now);
    let issuer_host = auth
        .oidc(connection)
        .and_then(|p| ordo_identity::oauth2::host_of(&p.connection().issuer));
    let mut message =
        format!("the identity provider for connection `{connection}` cannot be reached right now");
    if let Some(fix) = setting_that_fixes(connection, why, issuer_host.as_deref()) {
        message.push_str(&format!("; {fix}"));
    }
    let mut refusal = EdgeError::new(EdgeCode::IdentityProviderUnavailable, message)
        .with("connection", connection);
    if let Some(setting) = setting_named(connection, why) {
        refusal = refusal.with("setting", setting);
    }
    refusal.into_response()
}

/// The setting a refusal names, when one would fix it.
fn setting_named(connection: &str, why: &str) -> Option<String> {
    match why {
        "denied_address" => Some(crate::mail::PRIVATE_HOSTS_ENV.to_string()),
        "discovery-issuer-mismatch" => {
            Some(format!("{}ISSUER", crate::federation::prefix(connection)))
        }
        _ => None,
    }
}

/// The sentence that says which setting, and to what.
fn setting_that_fixes(connection: &str, why: &str, issuer_host: Option<&str>) -> Option<String> {
    let setting = setting_named(connection, why)?;
    Some(match why {
        "denied_address" => format!(
            "its issuer resolves to a private address; if it is on the deployment's own \
             network, set {setting}={}",
            issuer_host.unwrap_or("<the issuer's host>")
        ),
        _ => format!(
            "the issuer's discovery document names another issuer; set {setting} to exactly \
             the `issuer` it publishes"
        ),
    })
}

/// A method that carries no two-step claim was asked for one. Only an OIDC
/// connection does. No setting changes that, so none is named.
pub(super) fn presence_not_available(method: &str) -> Response {
    EdgeError::new(
        EdgeCode::PresenceNotAvailable,
        format!(
            "`{method}` carries no two-step claim; only an OIDC connection can be asked \
             for presence"
        ),
    )
    .with("connection", method)
    .into_response()
}

/// A provider's word, as the stream keeps it: letters, digits and `:._-`,
/// at most 64 bytes. Anything else is written `?`.
fn word(w: &str) -> String {
    if !w.is_empty()
        && w.len() <= 64
        && w.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":._-".contains(&b))
    {
        w.to_string()
    } else {
        "?".to_string()
    }
}

/// Record what an outside provider said about how the person signed in
/// (`amr`, `acr`, `auth_time`), and, when its two-step claim counted, which
/// connection vouched for presence. Nothing for a sign-in that carried no
/// such claims.
pub(super) fn record_upstream_claim(
    auth: &AuthState,
    signed: &ordo_identity::SignIn,
    subject: &ordo_identity::AuthenticatedSubject,
    now: Timestamp,
) {
    let Some(a) = &subject.attributes.assurance else {
        return;
    };
    let amr: Vec<String> = a.amr.iter().take(8).map(|m| word(m)).collect();
    let mut detail = format!(
        "principal={} connection={} amr={} acr={} auth_time={}",
        signed.principal.0,
        signed.authenticator.via(signed.method),
        amr.join(","),
        a.acr.as_deref().map(word).unwrap_or_default(),
        a.auth_time.map(|t| t.to_string()).unwrap_or_default(),
    );
    let act = match &signed.upstream_presence {
        Some(up) => {
            detail.push_str(&format!(" vouched_by={} at={}", up.via(), up.at().0));
            "presence-vouched"
        }
        None => "noted",
    };
    auth.record(
        Some(&signed.principal),
        "auth.complete",
        ordo_secevent::EventKind::UpstreamClaim {
            act: act.to_string(),
            detail,
        },
        now,
    );
}

/// Record that a provider's two-step claim stood in for Ordo's second step
/// at sign-in (`plan-second-factor.md` §5.3): which connection, at which
/// level, and for which tenant when a tenant's acceptance did it.
pub(super) fn record_second_step_satisfied(
    auth: &AuthState,
    principal: &ordo_identity::PrincipalId,
    via: &str,
    level: crate::second_factors::UpstreamLevel,
    space: Option<&str>,
    now: Timestamp,
) {
    let mut detail = format!(
        "principal={} connection={via} level={}",
        principal.0,
        level.as_str()
    );
    if level == crate::second_factors::UpstreamLevel::Tenant {
        if let Some(space) = space {
            detail.push_str(&format!(" space={}", word(space)));
        }
    }
    auth.record(
        Some(principal),
        "auth.complete",
        ordo_secevent::EventKind::UpstreamClaim {
            act: "second-step-satisfied".to_string(),
            detail,
        },
        now,
    );
}

/// The OIDC arm of the callback.
pub(super) async fn callback(
    auth: SharedAuth,
    provider: Arc<OidcProvider>,
    q: HashMap<String, String>,
    now: Timestamp,
) -> Response {
    let Some(state) = q.get("state").cloned() else {
        auth.record_refusal("auth.callback", "malformed-request", now);
        return refused();
    };
    // O1: a token in the query is a flow this connection never asked for.
    let front_channel_token = q.contains_key("id_token") || q.contains_key("access_token");
    let (code, error, iss) = (
        q.get("code").cloned(),
        q.get("error").cloned(),
        q.get("iss").cloned(),
    );
    let connection = provider.name().to_string();
    let done = tokio::task::spawn_blocking(move || {
        provider.callback(
            &OidcCallback {
                state: &state,
                code: code.as_deref(),
                error: error.as_deref(),
                iss: iss.as_deref(),
                front_channel_token,
            },
            now,
        )
    })
    .await;
    let Ok(outcome) = done else {
        auth.record_refusal("auth.callback", "ceremony-task-failed", now);
        return refused();
    };
    if outcome.stale_discovery {
        // O15: a discovery refresh failed and the last good copy is in use.
        auth.record_refusal("auth.callback", "oidc-discovery-stale-copy-used", now);
    }
    if let Some(why) = outcome.presence_not_minted {
        // The page asked for a two-step claim and the provider's answer did
        // not make one: single-factor, stale, or no `auth_time`. The sign-in
        // stands, without presence.
        auth.record(
            None,
            "auth.callback",
            ordo_secevent::EventKind::UpstreamClaim {
                act: "presence-not-minted".to_string(),
                detail: format!("connection={connection} reason={why}"),
            },
            now,
        );
    }
    if outcome.stale_keys {
        // O10: the issuer's keys could not be refreshed and the cache past
        // its freshness verified this token. Recorded so the deployment
        // owner sees the outage before the grace runs out.
        auth.record_refusal("auth.callback", "oidc-jwks-stale-cache-used", now);
    }
    let Some(return_to) = outcome.return_to else {
        if let Err(e @ AuthError::StoreUnavailable(_)) = &outcome.result {
            auth.record_refusal("auth.callback", "ceremony-store-unavailable", now);
            return super::ceremony_refusal(e);
        }
        auth.record_refusal("auth.callback", "unknown-state", now);
        return refused();
    };
    match outcome.result {
        Ok(handoff) => back_to(&return_to, "handoff", &handoff),
        Err(e) => {
            let word = match &e {
                AuthError::UpstreamUnavailable(why) => {
                    auth.record_refusal(
                        "auth.callback",
                        &format!("identity-provider-unavailable:{why}"),
                        now,
                    );
                    "identity_provider_unavailable"
                }
                AuthError::StoreUnavailable(c) => {
                    auth.record_refusal("auth.callback", auth_error_reason(&e), now);
                    // The page the person is sent back to can say "try
                    // again" for a full pool instead of an outage.
                    if c.0.is_busy() {
                        "server_busy"
                    } else {
                        "identity_provider_unavailable"
                    }
                }
                _ => {
                    auth.record_refusal("auth.callback", auth_error_reason(&e), now);
                    "authentication_failed"
                }
            };
            back_to(&return_to, "error", word)
        }
    }
}
