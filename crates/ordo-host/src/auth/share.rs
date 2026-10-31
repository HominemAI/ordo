//! The share routes a guest calls (`plan-sharing.md` G3, G4 and G5's leave).
//!
//! | Route | Token | Answers |
//! |---|---|---|
//! | `POST /auth/share/peek {ticket}` | none | what accepting would disclose and open |
//! | `POST /auth/share/accept {ticket, display_name}` | session | the seat and the grant |
//! | `POST /auth/window {space}` | session | a capability token for the consented window |
//! | `POST /auth/share/leave {space}` | session | the seat closed, by the guest |
//!
//! The ceremony is [`crate::share_accept`]; these are the doors to it. The
//! three that need a person take the session's own bearer token and refuse a
//! capability or a delegation, as step-up does: a window is opened by the
//! person it belongs to, from their session, and never from another window.
//!
//! # One answer
//!
//! An unknown, spent, expired or mis-addressed ticket, a sharer who is gone,
//! a program that changed under the share and a row the sharer may not open
//! are **one answer**, `share_refused`, and the stream keeps which. A caller
//! who could tell them apart could probe who shares what with whom.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::Timestamp;
use serde_json::{json, Value};

use super::{step_up_claims, unconfigured, AuthState, SharedAuth};
use crate::share_accept::{ShareFailure, OWNER_REACH};
use crate::share_ops::ShareOps;

fn share_refused() -> Response {
    EdgeError::new(EdgeCode::ShareRefused, "that share cannot be used").into_response()
}

fn share_unavailable(failure: super::StoreFailure) -> Response {
    failure.answer(
        ordo_api::EdgeCode::ShareUnavailable,
        "shares cannot be checked right now",
    )
}

impl ShareFailure {
    fn answer(self, auth: &AuthState, op: &str, now: Timestamp) -> Response {
        match self {
            ShareFailure::Refused(reason) => {
                auth.record_refusal(op, reason, now);
                share_refused()
            }
            ShareFailure::Unavailable(why) => {
                auth.record_refusal(op, "share-store-unavailable", now);
                share_unavailable(why.into())
            }
        }
    }
}

fn text<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The share state, or the refusal a deployment without it gives.
fn sharing<'a>(
    auth: &'a AuthState,
    op: &str,
    now: Timestamp,
) -> Result<&'a std::sync::Arc<ShareOps>, Box<Response>> {
    auth.sharing().ok_or_else(|| {
        auth.record_refusal(op, "share-not-wired", now);
        Box::new(share_refused())
    })
}

/// `POST /auth/share/peek {"ticket"}`: what a ticket offers, without
/// spending it. No token: the page shows this before the person has signed
/// in or signed up.
pub async fn peek(State(auth): State<Option<SharedAuth>>, Json(body): Json<Value>) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(ticket) = text(&body, "ticket") else {
        auth.record_refusal("auth.share.peek", "malformed-request", now);
        return share_refused();
    };
    let share = match sharing(&auth, "auth.share.peek", now) {
        Ok(s) => s.clone(),
        Err(r) => return *r,
    };
    match share.peek(ticket).await {
        Ok(v) => (
            StatusCode::OK,
            Json(json!({
                "space": v.space,
                "name": v.name,
                "label": v.label,
                "ops": v.ops,
                "rows": v.rows.iter().map(|r| json!({
                    "entity": r.entity, "id": r.id, "level": r.level,
                })).collect::<Vec<_>>(),
                "window_ends_at": v.window_ends_at,
                "ticket_expires_at": v.ticket_expires_at,
                "named": v.named,
                // What accepting discloses: the name the person types on
                // accepting is shown to the sharer.
                "discloses": "the display name you enter is shown to the sharer",
                "notice": OWNER_REACH,
            })),
        )
            .into_response(),
        Err(f) => f.answer(&auth, "auth.share.peek", now),
    }
}

/// `POST /auth/share/accept {"ticket", "display_name"}` with the session's
/// bearer token.
pub async fn accept(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let claims = match step_up_claims(&auth, &headers, "auth.share.accept", now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let (Some(ticket), Some(name)) = (text(&body, "ticket"), text(&body, "display_name")) else {
        auth.record_refusal("auth.share.accept", "malformed-request", now);
        return share_refused();
    };
    let share = match sharing(&auth, "auth.share.accept", now) {
        Ok(s) => s.clone(),
        Err(r) => return *r,
    };
    match share.accept(ticket, &claims.principal.0, name).await {
        Ok(a) => (
            StatusCode::OK,
            Json(json!({
                "space": a.space,
                "name": a.name,
                "seat": a.seat,
                "grant": a.grant,
                "expires": a.expires,
                "ops": a.ops,
            })),
        )
            .into_response(),
        Err(f) => f.answer(&auth, "auth.share.accept", now),
    }
}

/// `POST /auth/window {"space"}` with the session's bearer token: a
/// capability token for the window the person holds in that tenant.
///
/// The token is the share grant as it was consented to (its pins come from
/// the registry, never from the deployment), bound to that one tenant, with
/// no session and no client, ending at the earlier of the grant's end and
/// one session life. It cannot satisfy step-up, so no platform op is
/// reachable through it whatever the presence setting says, and it cannot
/// itself open a window: this route wants a session. A revoked grant is
/// refused here as it is at the door, so a window cannot be re-minted after
/// the sharer closes it.
pub async fn window(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let claims = match step_up_claims(&auth, &headers, "auth.window", now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let Some(space) = text(&body, "space") else {
        auth.record_refusal("auth.window", "malformed-request", now);
        return share_refused();
    };
    let share = match sharing(&auth, "auth.window", now) {
        Ok(s) => s.clone(),
        Err(r) => return *r,
    };
    let w = match share.window(space, &claims.principal.0).await {
        Ok(w) => w,
        Err(f) => return f.answer(&auth, "auth.window", now),
    };
    let expires = w.expires.min(now.0.saturating_add(auth.session_secs));
    let ops: Vec<String> = w.pins.keys().cloned().collect();
    let spec = ordo_identity::TokenSpec {
        principal: claims.principal.clone(),
        expires: Timestamp(expires),
        client: None,
        user_presence_at: None,
        capability: Some(ordo_identity::CapabilityGrant::new(
            w.grant.clone(),
            ordo_identity::SpaceId(space.to_string()),
            w.pins,
            Timestamp(expires),
        )),
        delegation: None,
        session: None,
    };
    let token = ordo_identity::token::mint(&auth.token_key, &spec);
    (
        StatusCode::OK,
        Json(json!({
            "token": token,
            "expires_at": expires,
            "space": space,
            "grant": w.grant.0,
            "ops": ops,
        })),
    )
        .into_response()
}

/// `POST /auth/share/leave {"space"}` with the session's bearer token: the
/// guest closes their own window. The grant is revoked, the acl rows the
/// shares wrote for a seat with no role are taken back, and the seat is
/// suspended, exactly as `std.space.revokeShare` does for the sharer, so the
/// token the guest already holds is refused at the next call. A person who
/// holds a role in that tenant keeps their membership: leaving closes only
/// the shares.
///
/// A tenant the person has no seat in is the one refusal.
pub async fn leave(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let claims = match step_up_claims(&auth, &headers, "auth.share.leave", now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let Some(space) = text(&body, "space") else {
        auth.record_refusal("auth.share.leave", "malformed-request", now);
        return share_refused();
    };
    let share = match sharing(&auth, "auth.share.leave", now) {
        Ok(s) => s.clone(),
        Err(r) => return *r,
    };
    match share.leave(space, &claims.principal.0).await {
        Ok(out) => {
            let row = out.rows.first().cloned().unwrap_or(Value::Null);
            (
                StatusCode::OK,
                Json(json!({
                    "space": space,
                    "outcome": "left",
                    "grants": row["grants"],
                    "suspended": row["suspended"],
                })),
            )
                .into_response()
        }
        Err(e) if e.code == ordo_api::ErrorCode::NotFound => {
            ShareFailure::Refused("no-seat").answer(&auth, "auth.share.leave", now)
        }
        Err(e) => ShareFailure::from(e).answer(&auth, "auth.share.leave", now),
    }
}
