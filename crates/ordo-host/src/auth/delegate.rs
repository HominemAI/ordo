//! The agent's side of the consent ceremony (11-4b, [`crate::consent`]).
//!
//! `POST /auth/delegation/request` with
//! `{"space", "agent", "person", "ops": [...], "hours"?}` answers
//! `{"request", "binder", "expiresAt"}`. The agent holds no token, so the
//! route asks for none. A refusal a setting explains names the setting and
//! its value; nothing about the tenant or the person is checked here, so the
//! answer is the same whether either exists. With `ORDO_AUTH_HUMAN_CHECK` on,
//! the body carries a `human` token, as a sign-in's does. Waiting requests
//! are capped per agent (`ORDO_DELEGATION_OPEN_PER_AGENT`) and per person
//! (`ORDO_DELEGATION_OPEN_PER_PERSON`); over either is `429 delegation_limit`.
//!
//! `POST /auth/delegation/redeem` with `{"request", "binder"}` answers the
//! agent's token once the person has approved:
//! `{"token", "expiresAt", "delegation", "space", "ops"}`. Before that it
//! answers `202 {"status": "waiting"}`. A wrong binder and an unknown id are
//! one answer. Only the holder of the binder learns that a request was
//! declined, expired or already redeemed.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{ApiError, EdgeCode, EdgeError, ErrorCode};
use ordo_identity::token;
use serde_json::{json, Value};

use super::{unconfigured, SharedAuth};
use crate::consent::{AgentRequest, DelegationSurface, Refusal};

fn answer(r: Refusal) -> Response {
    let (code, message) = match r {
        Refusal::Setting(m) => (EdgeCode::DelegationRefused, m),
        Refusal::Limit(m) => (EdgeCode::DelegationLimit, m),
        Refusal::Malformed(m) => (EdgeCode::InvalidRequest, m),
        Refusal::Unknown => {
            return ApiError::new(
                ErrorCode::NotFound,
                "no delegation request matches that id and binder",
            )
            .into_response()
        }
        Refusal::Waiting { expires_at } => {
            return (
                StatusCode::ACCEPTED,
                Json(json!({"status": "waiting", "expiresAt": expires_at})),
            )
                .into_response()
        }
        Refusal::Declined => (
            EdgeCode::DelegationDeclined,
            "the person declined this request".to_string(),
        ),
        Refusal::Expired => (
            EdgeCode::DelegationExpired,
            "this request lapsed, or the delegation ended before it was redeemed; ask again"
                .to_string(),
        ),
        Refusal::Redeemed => (
            EdgeCode::DelegationRedeemed,
            "this request was already redeemed; a request gives one token".to_string(),
        ),
        Refusal::Unavailable(why) => {
            return super::StoreFailure::from(why).answer(
                ordo_api::EdgeCode::DelegationStoreUnavailable,
                "delegation requests cannot be read right now",
            )
        }
    };
    EdgeError::new(code, message).into_response()
}

fn text(body: &Value, key: &str) -> Option<String> {
    body.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// `POST /auth/delegation/request`.
pub async fn request(State(auth): State<Option<SharedAuth>>, Json(body): Json<Value>) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let Some(consent) = auth.consent().cloned() else {
        return unconfigured();
    };
    // The route takes no token. With `ORDO_AUTH_HUMAN_CHECK` on, a request
    // carries a `human` token as a sign-in does, checked before any row is
    // written (audit 2026-10-11, finding 28).
    let now = ordo_identity::Timestamp((auth.now)());
    if let Err(refusal) = auth
        .check_human_for("auth.delegation.request", &body, now)
        .await
    {
        return *refusal;
    }
    let (Some(space), Some(agent), Some(person)) = (
        text(&body, "space"),
        text(&body, "agent"),
        text(&body, "person"),
    ) else {
        return answer(Refusal::Malformed(
            "a request names `space`, `agent` and `person`".into(),
        ));
    };
    let Some(ops) = body.get("ops").and_then(|v| v.as_array()) else {
        return answer(Refusal::Malformed("`ops` is a list of op names".into()));
    };
    let ops: Option<Vec<String>> = ops.iter().map(|o| o.as_str().map(str::to_string)).collect();
    let Some(ops) = ops else {
        return answer(Refusal::Malformed("`ops` is a list of op names".into()));
    };
    let hours = match body.get("hours") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_u64() {
            Some(h) => Some(h),
            None => {
                return answer(Refusal::Malformed("`hours` is a whole number".into()));
            }
        },
    };
    match consent
        .request(&AgentRequest {
            space,
            agent,
            person,
            ops,
            hours,
        })
        .await
    {
        Ok(r) => (
            StatusCode::CREATED,
            Json(json!({
                "request": r.request,
                "binder": r.binder,
                "expiresAt": r.expires_at,
                "review": crate::consent::REVIEW,
            })),
        )
            .into_response(),
        Err(r) => answer(r),
    }
}

/// `POST /auth/delegation/redeem`.
pub async fn redeem(State(auth): State<Option<SharedAuth>>, Json(body): Json<Value>) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let Some(consent) = auth.consent().cloned() else {
        return unconfigured();
    };
    let (Some(request), Some(binder)) = (text(&body, "request"), text(&body, "binder")) else {
        return answer(Refusal::Malformed(
            "a redeem names `request` and `binder`".into(),
        ));
    };
    match consent.redeem(&request, &binder).await {
        Ok(r) => {
            let bearer = token::mint(&auth.token_key, &r.spec);
            (
                StatusCode::OK,
                Json(json!({
                    "token": bearer,
                    "expiresAt": r.expires,
                    "delegation": r.delegation,
                    "space": r.space,
                    "ops": r.ops,
                })),
            )
                .into_response()
        }
        Err(r) => answer(r),
    }
}
