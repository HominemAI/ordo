//! Sign-in with a console key (`plan-console.md` 8.2, G14), at the routes
//! every sign-in uses: `POST /auth/begin` and `POST /auth/complete` with
//! `method: "console"`. [`crate::console_keys`] holds the keys and the
//! reasoning; this module is the two legs.
//!
//! - `begin {method: "console", key}` answers `{challenge, method}`. A
//!   challenge is minted for any well-formed key id, so the answer says
//!   nothing about which keys exist, and nothing is stored for it.
//! - `complete {method: "console", key, challenge, signature}` answers
//!   `{token, via, expires_at, session_expires_at, space, key}`. The token
//!   is a session with **no presence**, ending at `expires_at`; it cannot be
//!   refreshed, so it is renewed by signing again.
//!
//! Both come before the optional human check: the console is a server, and
//! its proof is the signature. Both count against the node's throttle
//! (`console_keys::MAX_ATTEMPTS_PER_WINDOW`), answered `too_many_attempts`.
//! Every other refusal is the one answer, with the reason in the security
//! record.
//!
//! [`gate`] is the layer over every `/auth/*` route that keeps a console
//! key session to the relayed touch and its own logout.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::{token, SessionClaim, Timestamp, TokenSpec};
use serde_json::{json, Value};

use super::{bearer, refused, AuthState, SharedAuth, StoreFailure};
use crate::console_keys::{console_may, Reach, SignInRefusal, Throttled, METHOD, VIA_PREFIX};

fn throttled(t: Throttled) -> Response {
    let mut res = EdgeError::new(
        EdgeCode::TooManyAttempts,
        "too many console sign-in attempts on this node; try again shortly",
    )
    .into_response();
    if let Ok(v) = t.retry_after_secs.max(1).to_string().parse() {
        res.headers_mut().insert("retry-after", v);
    }
    res
}

/// Console key sign-in while `ORDO_IDENTITY_URL` is set: refused, naming
/// the setting ([`crate::console_keys::ConsoleKeys::closed_by`]).
fn closed(
    auth: &AuthState,
    keys: &crate::console_keys::ConsoleKeys,
    route: &str,
    now: Timestamp,
) -> Option<Response> {
    let setting = keys.closed_by?;
    auth.record_refusal(route, "console-key-closed-identity-service", now);
    Some(
        EdgeError::new(
            EdgeCode::AuthUnconfigured,
            crate::console_keys::ConsoleKeys::closed_message(setting),
        )
        .into_response(),
    )
}

fn key_of(body: &Value) -> Option<&str> {
    body.get("key")
        .and_then(Value::as_str)
        .filter(|k| crate::console_keys::well_formed_id(k))
}

/// `POST /auth/begin {method: "console", key}`.
pub(super) async fn begin(auth: &AuthState, body: &Value, now: Timestamp) -> Response {
    let Some(keys) = auth.console_keys().cloned() else {
        auth.record_refusal("auth.begin", "unknown-method", now);
        return refused();
    };
    if let Some(res) = closed(auth, &keys, "auth.begin", now) {
        return res;
    }
    let Some(key) = key_of(body) else {
        auth.record_refusal("auth.begin", "malformed-request", now);
        return refused();
    };
    if let Err(t) = keys.attempt(now.0) {
        auth.record_refusal("auth.begin", "console-sign-in-throttled", now);
        return throttled(t);
    }
    (
        StatusCode::OK,
        Json(json!({"challenge": keys.begin(key, now.0), "method": METHOD})),
    )
        .into_response()
}

/// `POST /auth/complete {method: "console", key, challenge, signature}`.
pub(super) async fn complete(auth: &AuthState, body: &Value, now: Timestamp) -> Response {
    let Some(keys) = auth.console_keys().cloned() else {
        auth.record_refusal("auth.complete", "unknown-method", now);
        return refused();
    };
    if let Some(res) = closed(auth, &keys, "auth.complete", now) {
        return res;
    }
    let text = |k: &str| body.get(k).and_then(Value::as_str);
    let (Some(key), Some(challenge), Some(signature)) =
        (key_of(body), text("challenge"), text("signature"))
    else {
        auth.record_refusal("auth.complete", "malformed-request", now);
        return refused();
    };
    if let Err(t) = keys.attempt(now.0) {
        auth.record_refusal("auth.complete", "console-sign-in-throttled", now);
        return throttled(t);
    }
    let signed = match keys.complete(key, challenge, signature, now.0).await {
        Ok(s) => s,
        Err(SignInRefusal::Refused(reason)) => {
            auth.record_refusal("auth.complete", reason, now);
            return refused();
        }
        Err(SignInRefusal::Store(f)) => {
            auth.record_refusal("auth.complete", "console-key-store-unavailable", now);
            return StoreFailure::from(f).into_response();
        }
    };
    let expires = now.0.saturating_add(keys.session_secs);
    let via = format!("{VIA_PREFIX}{}", signed.key);
    let session = SessionClaim::starting_at(now).with_via(via.clone());
    let session_id = session.id.0.clone();
    let spec = TokenSpec {
        principal: signed.principal.clone(),
        expires: Timestamp(expires),
        client: None,
        // Never presence: a console key cannot satisfy step-up (G15).
        user_presence_at: None,
        capability: None,
        delegation: None,
        session: Some(session),
    };
    if let Err(f) = keys
        .store
        .signed_in(&signed.key, &session_id, expires, now.0)
        .await
    {
        // A session nobody could end by revoking its key is not minted.
        auth.record_refusal("auth.complete", "console-key-store-unavailable", now);
        return StoreFailure::from(f).into_response();
    }
    // The session row is written, so a revocation from here on ends it.
    // One that landed between the live check and the write would not see
    // it: look again, and mint nothing for a key revoked meanwhile.
    match keys.store.get(&signed.key).await {
        Ok(Some(row)) if row.live() => {}
        Ok(_) => {
            auth.record_refusal("auth.complete", "console-key-revoked", now);
            return refused();
        }
        Err(f) => {
            auth.record_refusal("auth.complete", "console-key-store-unavailable", now);
            return StoreFailure::from(f).into_response();
        }
    }
    if let Some(stream) = &auth.recorder {
        crate::console_keys::record(
            stream,
            &signed.principal.0,
            "auth.complete",
            "signed-in",
            format!("key={} principal={}", signed.key, signed.principal.0),
            now.0,
        );
    }
    let bearer = token::mint(&auth.token_key, &spec);
    (
        StatusCode::OK,
        Json(json!({
            "token": bearer,
            "via": via,
            "expires_at": expires,
            "session_expires_at": expires,
            "space": keys.home,
            "key": signed.key,
        })),
    )
        .into_response()
}

/// Is this session one a console key signed in?
pub fn is_console_session(claims: &ordo_identity::TokenClaims) -> bool {
    crate::console_keys::is_console_session(claims)
}

/// The layer over every `/auth/*` route (`auth::auth_routes`): a console
/// key session reaches only `console_keys::AUTH_ROUTES_OPEN`, and a
/// deployment token (3 segments) reaches none. A request
/// with no bearer, or one that does not verify, passes to the route, which
/// answers it as it always has.
pub async fn gate(
    State(auth): State<Option<SharedAuth>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if let (Some(auth), Some(token)) = (auth.as_ref(), bearer(req.headers())) {
        let now = Timestamp((auth.now)());
        // A deployment token has no app session to manage: its session
        // lives at the identity service (`plan-identity-service.md` 4.3).
        if ordo_identity::deployment_token::is_deployment_shape(&token) {
            auth.record_refusal(req.uri().path(), "deployment-token-at-auth-route", now);
            return refused();
        }
        if let Ok(claims) = ordo_identity::verify(&auth.token_key, &token, now) {
            if let Err(reason) = console_may(&claims, Reach::Auth(req.uri().path())) {
                auth.record_refusal(req.uri().path(), reason, now);
                return refused();
            }
        }
    }
    next.run(req).await
}
