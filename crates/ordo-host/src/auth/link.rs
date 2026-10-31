//! `POST /auth/link`: open a share link (`plan-sharing.md` L2).
//!
//! No `Authorization`. The secret in the body is the credential, and the
//! answer is a capability token for the link's seat: the share grant as it
//! was consented to (its pins come from the registry, never from the
//! deployment), bound to the one tenant, with no session and no client,
//! ending at the earlier of the link's end and one session life. That token
//! cannot step up, so no platform op is reachable through it, and it cannot
//! open a window or accept a share, because those routes want a session.
//!
//! # One answer
//!
//! A secret that is unknown, malformed, expired or revoked, a link whose
//! grant is no longer live, and a deployment that has switched links off are
//! **one answer**, `403 link_refused`; the stream keeps which. There is no
//! limit on misses: Ordo does not decide on a client's address.
//!
//! # A password
//!
//! Only after the secret matched a live link, a link with a password answers
//! one of three more, each retryable:
//!
//! | Answer | When |
//! |---|---|
//! | `401 link_password_required` | no `password` was sent |
//! | `401 link_password_wrong` | the password is not the link's |
//! | `429 too_many_attempts` | the always-on slowdown: wrong tries in a row on this link; `Retry-After` and `retry_after` say when, and `setting` names the tenant's limits op. Never a lock |
//! | `429 link_locked` | too many wrong tries, where a layer turned the lock on; `retry_after` says when, and `setting` names the tenant's limits op |
//!
//! A link that has reached its maximum number of opens is **used up** and
//! answers `403 link_refused`, like an expired one. An open counts once per
//! device: the body may carry a random `device` id, and a device that opened
//! before renews without being counted ([`crate::share_links`]).
//!
//! They reveal nothing the holder of the secret did not already know. The
//! password goes in the body beside the secret and is never recorded.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::Timestamp;
use serde_json::{json, Value};

use super::{unconfigured, SharedAuth};
use crate::share_links::OpenFailure;

fn link_refused() -> Response {
    EdgeError::new(EdgeCode::LinkRefused, "that link cannot be used").into_response()
}

fn link_unavailable(failure: super::StoreFailure) -> Response {
    failure.answer(
        ordo_api::EdgeCode::LinkUnavailable,
        "links cannot be checked right now",
    )
}

fn password_refusal(code: EdgeCode, message: &str) -> Response {
    EdgeError::new(code, message).into_response()
}

fn link_locked(until: u64, now: u64) -> Response {
    let retry_after = until.saturating_sub(now).max(1);
    EdgeError::new(
        EdgeCode::LinkLocked,
        format!(
            "too many wrong passwords: this link opens again in {retry_after} seconds. \
             The tenant owner sets the limits with {}",
            crate::share_links::SET_PASSWORD_LIMITS
        ),
    )
    .with("retry_after", retry_after)
    .with("setting", crate::share_links::SET_PASSWORD_LIMITS)
    .retry_after(retry_after)
    .into_response()
}

/// `POST /auth/link {"link", "password"?}`.
pub async fn open(State(auth): State<Option<SharedAuth>>, Json(body): Json<Value>) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(links) = auth.links().cloned() else {
        auth.record_refusal("auth.link", "link-not-wired", now);
        return link_refused();
    };
    let Some(secret) = body
        .get("link")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        auth.record_refusal("auth.link", "malformed-request", now);
        return link_refused();
    };
    let password = body.get("password").and_then(|v| v.as_str());
    let device = body.get("device").and_then(|v| v.as_str());
    let opened = match links.open(secret, password, device, now.0).await {
        Ok(o) => o,
        Err(OpenFailure::PasswordRequired) => {
            auth.record_refusal("auth.link", "link-password-required", now);
            return password_refusal(
                EdgeCode::LinkPasswordRequired,
                "this link needs its password",
            );
        }
        Err(OpenFailure::PasswordWrong) => {
            auth.record_refusal("auth.link", "link-password-wrong", now);
            return password_refusal(
                EdgeCode::LinkPasswordWrong,
                "that is not the link's password",
            );
        }
        Err(OpenFailure::Locked { until }) => {
            auth.record_refusal("auth.link", "link-locked", now);
            return link_locked(until, now.0);
        }
        Err(OpenFailure::Throttled {
            retry_after,
            setting,
        }) => {
            auth.record_throttled("auth.link", now);
            return super::throttled("this link", retry_after, setting);
        }
        Err(OpenFailure::Refused(why)) => {
            auth.record_refusal("auth.link", why, now);
            return link_refused();
        }
        Err(OpenFailure::Unavailable(why)) => {
            auth.record_refusal("auth.link", "link-store-unavailable", now);
            return link_unavailable(why.into());
        }
    };
    let w = opened.window;
    let expires = w.expires.min(now.0.saturating_add(auth.session_secs));
    let ops: Vec<String> = w.pins.keys().cloned().collect();
    let spec = ordo_identity::TokenSpec {
        principal: ordo_identity::PrincipalId(opened.link.clone()),
        expires: Timestamp(expires),
        client: None,
        user_presence_at: None,
        capability: Some(ordo_identity::CapabilityGrant::new(
            w.grant.clone(),
            ordo_identity::SpaceId(opened.space.clone()),
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
            "space": opened.space,
            "link": opened.link,
            "ops": ops,
        })),
    )
        .into_response()
}
