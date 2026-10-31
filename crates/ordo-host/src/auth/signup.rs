//! What built-in provisioning adds to the sign-in routes
//! (`plan-provisioning-on-signup.md` P2 and P3).
//!
//! Kept out of `auth.rs` so that file carries one call at each of two places:
//! the registration route asks [`registration_allowed`] before it begins a
//! ceremony, and the sign-in route calls [`given`] after the session is
//! minted. The setting, the limits and the provisioning itself are
//! [`crate::self_serve`].

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_identity::Timestamp;
use serde_json::{json, Value};

use super::{AuthState, SharedAuth};
use crate::self_serve::{Counted, Given, Refusal};

/// The 429 a refused sign-up gets: the one `quota_exceeded` in the
/// inventory. It says which limit only in the stream, not to the caller.
pub(super) fn limit_reached() -> Response {
    ordo_api::ApiError::new(
        ordo_api::ErrorCode::QuotaExceeded,
        "too many sign-ups right now; try again later",
    )
    .into_response()
}

/// The stream reason for a sign-up over the deployment ceiling. The caller
/// is not told which limit; the deployment owner reads this one.
pub(super) fn limit_reason() -> String {
    format!(
        "signup-limit-deployment: the hour's sign-ups are used; raise {} or \
         std.deployment.setSelfServe($signupsPerHour) to lift the ceiling",
        crate::self_serve::PER_HOUR_ENV
    )
}

/// May a registration without a ticket begin? Counts it in when it may.
///
/// Called before the ceremony, so a refused sign-up mints nothing. Without
/// provisioning wired, or with the setting `off`, every registration may.
pub(super) async fn registration_allowed(
    auth: &AuthState,
    now: Timestamp,
) -> Result<(), Box<Response>> {
    sign_up_allowed(auth, "auth.register", now).await
}

/// May a sign-up go on? Counts it in when it may. `op` names the route in
/// the stream: a passkey registration is counted at `/auth/register`, and a
/// first sign-in through an outside provider at `/auth/complete`, the first
/// moment it is known to be new (`plan-auth-federated.md` §2.6). Both count
/// against the one deployment-wide ceiling.
pub(super) async fn sign_up_allowed(
    auth: &AuthState,
    op: &str,
    now: Timestamp,
) -> Result<(), Box<Response>> {
    let Some(signup) = auth.signup() else {
        return Ok(());
    };
    match signup.admit(Counted::Registration, now.0).await {
        Ok(()) => Ok(()),
        Err(Refusal::DeploymentFull) => {
            auth.record_refusal(op, &limit_reason(), now);
            Err(Box::new(limit_reached()))
        }
        Err(Refusal::Unavailable(failure)) => {
            auth.record_refusal(op, "signup-limit-unreadable", now);
            Err(Box::new(failure.into_response()))
        }
    }
}

/// The hook of `/auth/complete`: after the session is minted, give the
/// person a tenant of their own when the setting says so, and add the two
/// fields of §2.3 to the answer.
///
/// Adds nothing where provisioning is not wired. Where it is wired, the
/// fields are always there (`space: null` under `off`), so a client never
/// has to tell "off" from "absent".
pub(super) async fn given(
    auth: &AuthState,
    answer: &mut Value,
    principal: &str,
    display: &str,
    now: Timestamp,
) {
    let Some(signup) = auth.signup() else {
        return;
    };
    let given = signup.on_sign_in(principal, display, now.0).await;
    let (space, why) = given.fields();
    answer["space"] = json!(space);
    answer["provisioning"] = json!(why);
}

/// The person a bearer token proves, for the provisioning routes. A
/// capability or a delegation is an app's authority, not the person, and is
/// refused.
fn person(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
    op: &str,
    now: Timestamp,
) -> Result<ordo_identity::PrincipalId, Box<Response>> {
    let Some(token) = super::bearer(headers) else {
        auth.record_refusal(op, "no-token", now);
        return Err(Box::new(super::refused()));
    };
    let Ok(claims) = ordo_identity::verify(&auth.token_key, &token, now) else {
        auth.record_refusal(op, "token-unverified", now);
        return Err(Box::new(super::refused()));
    };
    if claims.capability.is_some() || claims.delegation.is_some() {
        auth.record_refusal(op, "not-a-person", now);
        return Err(Box::new(super::refused()));
    }
    if let (Some(session), Some(revocations)) = (claims.session.as_ref(), auth.revocations.as_ref())
    {
        if revocations.is_revoked(&session.id)
            || revocations.principal_ended(&claims.principal, session.started_at)
        {
            auth.record_refusal(op, "session-ended", now);
            return Err(Box::new(super::refused()));
        }
    }
    Ok(claims.principal)
}

/// Where the person's own tenant stands, with `ready` only once this
/// replica's door serves them in it.
async fn status_of(auth: &AuthState, principal: &ordo_identity::PrincipalId) -> Value {
    let Some(signup) = auth.signup() else {
        return crate::self_serve::OwnStatus::Off.to_json();
    };
    let engine = auth.spaces().map(|l| (l.engine)());
    let serves = |space: &str| match &engine {
        Some(engine) => engine
            .spaces_of(principal)
            .into_iter()
            .any(|(s, _, status)| {
                s.0 == space && status == ordo_identity::MembershipStatus::Active
            }),
        // No engine to ask: an active tenant is taken as served.
        None => true,
    };
    signup.status(&principal.0, serves).await.to_json()
}

/// `GET /auth/provisioning`: where the person's own tenant stands (owner,
/// 2026-10-21). `state` is `off`, `none`, `provisioning` (with `step`),
/// `ready` (with `space`), `failed` (with `reason`), or `unavailable`.
///
/// The SDK's `waitForTenant` reads this until it says `ready`, so an app
/// never writes the loop itself.
pub(super) async fn provisioning(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return super::unconfigured();
    };
    let now = Timestamp((auth.now)());
    let principal = match person(&auth, &headers, "auth.provisioning", now) {
        Ok(p) => p,
        Err(refusal) => return *refusal,
    };
    let body = status_of(&auth, &principal).await;
    let ready = body["state"] != "unavailable";
    ordo_api::error::status_report(ready, body)
}

/// `POST /auth/provisioning`: start the person's own tenant again after it
/// failed, or for the first time. Answers at once with the status, as `GET`
/// does; a tenant already made or being made is left alone.
pub(super) async fn retry_provisioning(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return super::unconfigured();
    };
    let now = Timestamp((auth.now)());
    let principal = match person(&auth, &headers, "auth.provisioning", now) {
        Ok(p) => p,
        Err(refusal) => return *refusal,
    };
    if let Some(signup) = auth.signup() {
        if let Given::LimitReached = signup.retry(&principal.0, now.0).await {
            auth.record_refusal("auth.provisioning", &limit_reason(), now);
            return limit_reached();
        }
    }
    let body = status_of(&auth, &principal).await;
    (StatusCode::OK, Json(body)).into_response()
}
