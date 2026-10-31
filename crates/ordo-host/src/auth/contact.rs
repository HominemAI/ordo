//! A signed-in person's own email address: read, add, change, remove
//! ([`crate::contact_email`], owner ruling 2026-10-22).
//!
//! | Route | Body | Answers |
//! |---|---|---|
//! | `GET /auth/email` | | `{address, source, verified_at, required, setting, mail}` |
//! | `POST /auth/email/begin` | `{address}` | `{challenge, binder}`: a code is mailed |
//! | `POST /auth/email/complete` | `{challenge, binder, code}` | `{address, source, verified_at, replaced}` |
//! | `POST /auth/email/remove` | | `{removed}` |
//!
//! Changing or removing an address needs a fresh passkey touch unless the
//! app developer turned that off (`403 presence_required`, naming
//! `ORDO_EMAIL_CHANGE_TOUCH` or `std.deployment.setEmailPolicy`). Adding a
//! first address needs none.
//!
//! Every route takes a session's bearer token: the person, never a share
//! link's or a delegated token. A wrong, expired, used or another browser's
//! code is the one refusal, as at sign-in. A deployment that sends no mail
//! answers `403 email_unavailable` naming `ORDO_MAIL_KIND`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::Timestamp;
use serde_json::{json, Value};

use super::{refused, step_up_claims, unconfigured, AuthState, SharedAuth, StoreFailure};
use crate::contact_email::{ContactEmails, SOURCE_CODE};

const READ: &str = "auth.email";
const BEGIN: &str = "auth.email.begin";
const COMPLETE: &str = "auth.email.complete";
const REMOVE: &str = "auth.email.remove";

pub fn routes<S>() -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
    Option<SharedAuth>: axum::extract::FromRef<S>,
{
    axum::Router::new()
        .route("/auth/email", axum::routing::get(read))
        .route("/auth/email/begin", axum::routing::post(begin))
        .route("/auth/email/complete", axum::routing::post(complete))
        .route("/auth/email/remove", axum::routing::post(remove))
}

/// The person, and the store. Every refusal is the one answer, recorded.
fn person(
    auth: &Arc<AuthState>,
    headers: &axum::http::HeaderMap,
    op: &str,
    now: Timestamp,
) -> Result<(String, Arc<ContactEmails>), Box<Response>> {
    person_claims(auth, headers, op, now).map(|(p, c, _)| (p, c))
}

fn person_claims(
    auth: &Arc<AuthState>,
    headers: &axum::http::HeaderMap,
    op: &str,
    now: Timestamp,
) -> Result<(String, Arc<ContactEmails>, ordo_identity::TokenClaims), Box<Response>> {
    let Some(contact) = auth.contact().cloned() else {
        return Err(Box::new(unconfigured()));
    };
    let claims = step_up_claims(auth, headers, op, now)?;
    Ok((claims.principal.0.clone(), contact, claims))
}

/// `Some(the 403)` when changing or removing the address needs a fresh
/// passkey touch and the session has none. The refusal names the setting.
async fn touch_needed(
    auth: &AuthState,
    contact: &ContactEmails,
    claims: &ordo_identity::TokenClaims,
    op: &str,
    now: Timestamp,
) -> Option<Response> {
    let (required, setting) = contact.change_touch().await;
    if !required || super::recover::fresh_presence(claims, now) {
        return None;
    }
    auth.record_refusal(op, "presence-required", now);
    Some(
        EdgeError::new(
            EdgeCode::PresenceRequired,
            format!(
                "changing or removing your email address needs a fresh passkey touch: \
                 POST /auth/step-up first. The app developer can turn this off with \
                 {setting}"
            ),
        )
        .with("setting", setting)
        .into_response(),
    )
}

fn mail_off() -> Response {
    EdgeError::new(
        EdgeCode::EmailUnavailable,
        format!(
            "this deployment sends no mail, so no address can be confirmed; the \
             deployment owner sets {} (smtp, resend or sendgrid)",
            crate::mail::KIND_ENV
        ),
    )
    .with("setting", crate::mail::KIND_ENV)
    .into_response()
}

/// `GET /auth/email`: the person's address, and what the app developer
/// requires. `address` is `null` when Ordo knows none.
pub async fn read(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (principal, contact) = match person(&auth, &headers, READ, now) {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let stored = match contact.address_of(&principal).await {
        Ok(s) => s,
        Err(e) => {
            auth.record_refusal(READ, "contact-store-unavailable", now);
            return StoreFailure::from(e).into_response();
        }
    };
    let (required, setting) = contact.requirement().await;
    let required = match required {
        crate::contact_email::Requirement::Off => json!("off"),
        crate::contact_email::Requirement::All => json!("all"),
        crate::contact_email::Requirement::Ops(ops) => json!(ops),
    };
    (
        StatusCode::OK,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "address": stored.as_ref().map(|s| s.address.clone()),
            "source": stored.as_ref().map(|s| s.source.clone()),
            "verified_at": stored.as_ref().map(|s| s.verified_at),
            "required": required,
            "setting": setting,
            "mail": contact.codes().is_some(),
        })),
    )
        .into_response()
}

/// `POST /auth/email/begin {address}`: mail a code to the address.
pub async fn begin(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (principal, contact) = match person(&auth, &headers, BEGIN, now) {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let Some(codes) = contact.codes().cloned() else {
        auth.record_refusal(BEGIN, "mail-off", now);
        return mail_off();
    };
    let Some(address) = body
        .get("address")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        auth.record_refusal(BEGIN, "malformed-request", now);
        return refused();
    };
    let begun =
        tokio::task::spawn_blocking(move || codes.start_contact(&principal, &address, now.0)).await;
    match begun {
        Ok(Ok(b)) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(json!({"challenge": b.challenge, "binder": b.binder})),
        )
            .into_response(),
        Ok(Err(ordo_identity::AuthError::BadCredentials)) => {
            auth.record_refusal(BEGIN, "malformed-address", now);
            refused()
        }
        Ok(Err(e)) => {
            auth.record_refusal(BEGIN, "credential-store-unavailable", now);
            StoreFailure::from(e).into_response()
        }
        Err(_) => {
            auth.record_refusal(BEGIN, "ceremony-task-failed", now);
            refused()
        }
    }
}

/// `POST /auth/email/complete {challenge, binder, code}`: the address is the
/// person's from now on. A different address it replaces is told.
pub async fn complete(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (principal, contact, claims) = match person_claims(&auth, &headers, COMPLETE, now) {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let Some(codes) = contact.codes().cloned() else {
        auth.record_refusal(COMPLETE, "mail-off", now);
        return mail_off();
    };
    // A first address needs no touch; replacing one does.
    match contact.added_address(&principal).await {
        Ok(Some(_)) => {
            if let Some(r) = touch_needed(&auth, &contact, &claims, COMPLETE, now).await {
                return r;
            }
        }
        Ok(None) => {}
        Err(e) => {
            auth.record_refusal(COMPLETE, "contact-store-unavailable", now);
            return StoreFailure::from(e).into_response();
        }
    }
    let text = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(challenge), Some(binder), Some(code)) =
        (text("challenge"), text("binder"), text("code"))
    else {
        auth.record_refusal(COMPLETE, "malformed-request", now);
        return refused();
    };
    let who = principal.clone();
    let checked = tokio::task::spawn_blocking(move || {
        codes.complete_contact(&who, &challenge, &binder, &code, now.0)
    })
    .await;
    let address = match checked {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => {
            auth.record_refusal(COMPLETE, super::auth_error_reason(&e), now);
            return super::ceremony_refusal(&e);
        }
        Err(_) => {
            auth.record_refusal(COMPLETE, "ceremony-task-failed", now);
            return refused();
        }
    };
    match contact
        .confirm(&principal, &address, SOURCE_CODE, now.0)
        .await
    {
        Ok(replaced) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(json!({
                "address": address,
                "source": SOURCE_CODE,
                "verified_at": now.0,
                "replaced": replaced.is_some(),
            })),
        )
            .into_response(),
        Err(e) => {
            auth.record_refusal(COMPLETE, "contact-store-unavailable", now);
            StoreFailure::from(e).into_response()
        }
    }
}

/// `POST /auth/email/remove`: take the added address off. The old address
/// is told. An emailed-code sign-in's address stays, since it is a way in.
pub async fn remove(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (principal, contact, claims) = match person_claims(&auth, &headers, REMOVE, now) {
        Ok(x) => x,
        Err(r) => return *r,
    };
    if let Some(r) = touch_needed(&auth, &contact, &claims, REMOVE, now).await {
        return r;
    }
    match contact.remove(&principal, now.0).await {
        Ok(removed) => (StatusCode::OK, Json(json!({"removed": removed}))).into_response(),
        Err(e) => {
            auth.record_refusal(REMOVE, "contact-store-unavailable", now);
            StoreFailure::from(e).into_response()
        }
    }
}

/// After a sign-in: an address the outside provider marked verified becomes
/// the person's, when they have none. An emailed code's address is the
/// sign-in itself and is already read from there.
pub(super) async fn adopt_verified(
    auth: &AuthState,
    principal: &str,
    subject: &ordo_identity::AuthenticatedSubject,
    connection: &str,
    now: Timestamp,
) {
    let Some(contact) = auth.contact() else {
        return;
    };
    let Some(email) = subject.attributes.email.as_ref() else {
        return;
    };
    if !matches!(
        email.evidence(),
        ordo_identity::provider::EmailEvidence::ProviderClaim { .. }
    ) {
        return;
    }
    if contact
        .adopt(principal, email.address(), connection, now.0)
        .await
        .is_err()
    {
        auth.record_refusal("auth.complete", "contact-email-unwritten", now);
    }
}
