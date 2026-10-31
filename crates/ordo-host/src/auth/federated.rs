//! The two routes outside-provider sign-in adds (`plan-auth-federated.md`
//! §3.2, §5.3): the return leg, `GET /auth/callback/{connection}`, and a
//! person's own linked identities, `GET /auth/identities`.
//!
//! # The return leg answers the browser, not the application
//!
//! The provider sends the browser here with `code` and `state`. What goes
//! back is a `303` to the `return_to` the ceremony was begun with: with
//! `handoff=<code>` when the provider's answer named a person, and with
//! `error=authentication_failed` (or `identity_provider_unavailable`) when it
//! did not. The application then redeems the handoff at `/auth/complete`
//! with its binder. No token is ever in a URL, the provider's code never
//! reaches application JavaScript, and a callback whose `state` names no
//! ceremony has nowhere to send the browser and is the one `401`.
//!
//! Every reason stays in the security stream. The browser is told one of two
//! words.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_identity::{AuthError, Timestamp};
use serde_json::{json, Value};

use super::{auth_error_reason, bearer, refused, unconfigured, SharedAuth, StoreFailure};

/// `GET /auth/callback/{connection}`: the provider's redirect URI.
pub async fn callback(
    State(auth): State<Option<SharedAuth>>,
    Path(connection): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    if let Some(provider) = auth.oidc(&connection) {
        return super::oidc::callback(auth, provider, q, now).await;
    }
    let Some(provider) = auth.oauth2(&connection) else {
        auth.record_refusal("auth.callback", "unknown-connection", now);
        return refused();
    };
    oauth2_callback(auth, provider, q, now).await
}

/// The OAuth2 arm of the callback, for a deployment's connection and a
/// tenant's alike.
pub(super) async fn oauth2_callback(
    auth: SharedAuth,
    provider: std::sync::Arc<ordo_identity::oauth2::OAuth2Provider>,
    q: HashMap<String, String>,
    now: Timestamp,
) -> Response {
    let Some(state) = q.get("state").cloned() else {
        auth.record_refusal("auth.callback", "malformed-request", now);
        return refused();
    };
    let code = q.get("code").cloned();
    let error = q.get("error").cloned();
    let done = tokio::task::spawn_blocking(move || {
        provider.callback(&state, code.as_deref(), error.as_deref(), now)
    })
    .await;
    let Ok(outcome) = done else {
        auth.record_refusal("auth.callback", "ceremony-task-failed", now);
        return refused();
    };
    let Some(return_to) = outcome.return_to else {
        // No ceremony, so nowhere to send the browser.
        let reason = match &outcome.result {
            Err(e @ AuthError::StoreUnavailable(_)) => {
                auth.record_refusal("auth.callback", "ceremony-store-unavailable", now);
                return super::ceremony_refusal(e);
            }
            _ => "unknown-state",
        };
        auth.record_refusal("auth.callback", reason, now);
        return refused();
    };
    match outcome.result {
        Ok(handoff) => back_to(&return_to, "handoff", &handoff),
        Err(e) => {
            auth.record_refusal("auth.callback", auth_error_reason(&e), now);
            let word = match e {
                // The page the person is sent back to can say "try again"
                // for a full pool instead of an outage.
                AuthError::StoreUnavailable(c) if c.0.is_busy() => "server_busy",
                AuthError::UpstreamUnavailable(_) | AuthError::StoreUnavailable(_) => {
                    "identity_provider_unavailable"
                }
                _ => "authentication_failed",
            };
            back_to(&return_to, "error", word)
        }
    }
}

/// `303` to `return_to` with one parameter added. `no-store`, and no
/// referrer, so the handoff code does not travel on from the page it lands
/// on.
pub(super) fn back_to(return_to: &str, key: &str, value: &str) -> Response {
    let joiner = if return_to.contains('?') { '&' } else { '?' };
    let location = format!(
        "{return_to}{joiner}{key}={}",
        ordo_identity::federation::percent(value)
    );
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, location),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
        ],
    )
        .into_response()
}

/// `GET /auth/identities` with a session's bearer token: the outside
/// identities linked to the caller, and the profile each connection keeps.
///
/// **Only the caller's own.** There is no parameter that names anybody else,
/// and a capability or delegated token is the one refusal: this is the person
/// reading what is kept about them. `email` is there only when the provider
/// marked it verified.
pub async fn identities(State(auth): State<Option<SharedAuth>>, headers: HeaderMap) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(token) = bearer(&headers) else {
        auth.record_refusal("auth.identities", "no-token", now);
        return refused();
    };
    let Ok(claims) = ordo_identity::verify(&auth.token_key, &token, now) else {
        auth.record_refusal("auth.identities", "token-unverified", now);
        return refused();
    };
    if claims.capability.is_some() || claims.delegation.is_some() {
        auth.record_refusal("auth.identities", "not-the-person", now);
        return refused();
    }
    let Some(session) = claims.session.as_ref() else {
        auth.record_refusal("auth.identities", "not-a-session", now);
        return refused();
    };
    if let Some(revocations) = auth.revocations.as_ref() {
        if revocations.is_revoked(&session.id)
            || revocations.principal_ended(&claims.principal, session.started_at)
        {
            auth.record_refusal("auth.identities", "session-ended", now);
            return refused();
        }
    }
    let Some(store) = auth.identity_attrs.clone() else {
        return (StatusCode::OK, Json(json!([]))).into_response();
    };
    let auth2 = auth.clone();
    let who = claims.principal.clone();
    let looked = tokio::task::spawn_blocking(move || {
        let dir = auth2.lock_directory();
        // `None` is a deleted person; anything else a store that failed.
        dir.refuse_deleted(&who).map_err(|e| match e {
            ordo_identity::DirectoryError::Deleted(_) => None,
            e => Some(StoreFailure::from(e)),
        })?;
        let linked = dir
            .authenticators_of(&who)
            .map_err(|e| Some(StoreFailure::from(e)))?;
        drop(dir);
        let rows = store.of(&who).map_err(|e| Some(StoreFailure::from(e)))?;
        // A row whose link is gone is not shown: what is listed is what
        // signs this person in today.
        Ok::<_, Option<StoreFailure>>(
            rows.into_iter()
                .filter(|r| linked.iter().any(|a| a.0 == r.authenticator))
                .collect::<Vec<_>>(),
        )
    })
    .await;
    let rows = match looked {
        Ok(Ok(rows)) => rows,
        Ok(Err(None)) => {
            auth.record_refusal("auth.identities", "identity-deleted", now);
            return refused();
        }
        Ok(Err(Some(failure))) => {
            auth.record_refusal("auth.identities", "store-unavailable", now);
            return failure.into_response();
        }
        Err(_) => {
            auth.record_refusal("auth.identities", "store-unavailable", now);
            return StoreFailure::task_failed().into_response();
        }
    };
    let list: Vec<Value> = rows
        .iter()
        .filter_map(|r| {
            // `oauth2:<connection>:<subject>`, or `email:<address>` for an
            // address an emailed code verified.
            let (connection, subject) = match r.authenticator.strip_prefix("email:") {
                Some(address) => ("email", address),
                None => {
                    let mut parts = r.authenticator.splitn(3, ':');
                    let (_, c, s) = (parts.next()?, parts.next()?, parts.next()?);
                    (c, s)
                }
            };
            Some(json!({
                "connection": connection,
                "subject": subject,
                "display": r.display,
                "avatar": r.avatar,
                "email": r.email,
                "linkedAt": r.linked_at,
            }))
        })
        .collect();
    (StatusCode::OK, Json(Value::Array(list))).into_response()
}
