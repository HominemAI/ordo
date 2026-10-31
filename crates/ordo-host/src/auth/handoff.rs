//! One sign-in across subdomains (`plan-subdomain-signin.md`, owner rulings
//! 2026-10-09): the core site hands a signed-in person to another site with
//! a one-time code, and that site swaps the code for its own session.
//!
//! - `POST /auth/handoff`, with the core site's bearer token and
//!   `{return_to, binder_hash}`, files the code and answers the address to
//!   send the browser to.
//! - `POST /auth/handoff/redeem`, with `{code, binder}`, answers a new
//!   session for the same person.
//!
//! # Why the code in the URL is not a token
//!
//! The code is filed under the SHA-256 of a binder the starting page made
//! and kept. Redeeming takes the binder itself. A code read from a history,
//! a log or a referrer is useless to its reader, and a wrong binder leaves
//! the code in place, so its reader cannot burn it either.
//!
//! # Neither route signs anybody in
//!
//! The person already has a session. Nothing here resolves a method, mints
//! a principal, spends an invitation or joins a tenant, which is why this is
//! not a `kind` of `/auth/complete`.
//!
//! Every refusal is the one answer, with the reason in the security stream.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_identity::federation::{handle_of, random_secret};
use ordo_identity::{CeremonyRow, Timestamp};
use serde_json::{json, Value};

use super::{bearer, refused, unconfigured, AuthState, SharedAuth, StoreFailure};

/// How long a hand-off code lives when `ORDO_AUTH_HANDOFF_SECS` is unset.
pub const DEFAULT_SECS: u64 = 60;
/// The longest `ORDO_AUTH_HANDOFF_SECS` may be.
pub const MAX_SECS: u64 = 300;
/// The query parameter the code arrives in.
pub const PARAM: &str = "handoff_code";
/// The `via` of a session that came by hand-off.
pub const VIA: &str = "handoff";

/// The ceremony table's `connection` for a hand-off. A `$` cannot appear in
/// a connection name (1 to 32 of a-z, 0-9 and -), so no outside provider's
/// row can be mistaken for one of these.
const CONNECTION: &str = "$subdomain";
const PHASE: &str = "handoff";

const ISSUE: &str = "auth.handoff";
const REDEEM: &str = "auth.handoff.redeem";

/// What a hand-off code stands for: the session that issued it.
struct Handed {
    principal: String,
    origin: String,
    session: String,
    started_at: u64,
    second_step: Option<String>,
    client: Option<String>,
    via: Option<String>,
}

impl Handed {
    fn render(&self) -> String {
        json!({
            "principal": self.principal,
            "origin": self.origin,
            "session": self.session,
            "started_at": self.started_at,
            "second_step": self.second_step,
            "client": self.client,
            "via": self.via,
        })
        .to_string()
    }

    fn read(text: &str) -> Option<Handed> {
        let v: Value = serde_json::from_str(text).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        Some(Handed {
            principal: s("principal")?,
            origin: s("origin")?,
            session: s("session")?,
            started_at: v.get("started_at")?.as_u64()?,
            second_step: s("second_step"),
            client: s("client"),
            via: s("via"),
        })
    }
}

/// A binder's public half: the SHA-256 of the binder, base64url, unpadded.
fn plausible_hash(h: &str) -> bool {
    h.len() == 43
        && h.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn session_live(auth: &AuthState, claims: &ordo_identity::TokenClaims) -> bool {
    let Some(session) = claims.session.as_ref() else {
        return false;
    };
    match auth.revocations.as_ref() {
        Some(r) => {
            !r.is_revoked(&session.id) && !r.principal_ended(&claims.principal, session.started_at)
        }
        None => true,
    }
}

async fn deleted(auth: &SharedAuth, who: &str) -> Result<bool, StoreFailure> {
    let (auth2, who) = (auth.clone(), ordo_identity::PrincipalId(who.to_string()));
    let looked = tokio::task::spawn_blocking(move || {
        auth2
            .directory
            .lock()
            .expect("directory mutex poisoned")
            .refuse_deleted(&who)
    })
    .await;
    match looked {
        Ok(Ok(())) => Ok(false),
        Ok(Err(ordo_identity::DirectoryError::Deleted(_))) => Ok(true),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err(StoreFailure::task_failed()),
    }
}

fn record_act(auth: &AuthState, who: &str, op: &str, act: &str, detail: String, at: Timestamp) {
    auth.record(
        Some(&ordo_identity::PrincipalId(who.to_string())),
        op,
        ordo_secevent::EventKind::SessionHandoff {
            act: act.to_string(),
            detail,
        },
        at,
    );
}

/// `POST /auth/handoff`: file a one-time code for the caller's session and
/// answer where to send the browser.
pub async fn issue(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(token) = bearer(&headers) else {
        auth.record_refusal(ISSUE, "no-token", now);
        return refused();
    };
    let Ok(claims) = ordo_identity::verify(&auth.token_key, &token, now) else {
        auth.record_refusal(ISSUE, "token-unverified", now);
        return refused();
    };
    if claims.capability.is_some() || claims.delegation.is_some() {
        auth.record_refusal(ISSUE, "not-the-person", now);
        return refused();
    }
    if !session_live(&auth, &claims) {
        auth.record_refusal(ISSUE, "session-ended", now);
        return refused();
    }
    let text = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::trim);
    let (Some(return_to), Some(binder_hash)) = (text("return_to"), text("binder_hash")) else {
        auth.record_refusal(ISSUE, "malformed-request", now);
        return refused();
    };
    if !plausible_hash(binder_hash) {
        auth.record_refusal(ISSUE, "malformed-binder-hash", now);
        return refused();
    }
    let Some(returns) = auth.returns.as_ref() else {
        auth.record_refusal(
            ISSUE,
            "returns-unset: set ORDO_AUTH_RETURN_URLS to the origins a hand-off may return to",
            now,
        );
        return refused();
    };
    let origin = ordo_identity::origin::Origin::of_url(return_to);
    let origin = match origin {
        Some(o) if !return_to.contains('#') && returns.allows(&o) => o,
        _ => {
            auth.record_refusal(
                ISSUE,
                "return-not-allowed: add the return origin to ORDO_AUTH_RETURN_URLS \
                 (any path on a listed origin is allowed; https://*.<domain> needs \
                 ORDO_WILDCARD_ORIGINS=on)",
                now,
            );
            return refused();
        }
    };
    let Some(ceremonies) = auth.ceremonies.clone() else {
        auth.record_refusal(ISSUE, "no-ceremony-store", now);
        return unconfigured();
    };
    let who = claims.principal.0.clone();
    match deleted(&auth, &who).await {
        Ok(false) => {}
        Ok(true) => {
            auth.record_refusal(ISSUE, "identity-deleted", now);
            return refused();
        }
        Err(failure) => {
            auth.record_refusal(ISSUE, "directory-unavailable", now);
            return failure.into_response();
        }
    }
    let session = claims.session.as_ref().expect("checked live above");
    let handed = Handed {
        principal: who.clone(),
        origin: origin.to_string(),
        session: session.id.0.clone(),
        started_at: session.started_at.0,
        second_step: session.second_step.map(|k| k.as_str().to_string()),
        client: claims.client.as_ref().map(|c| c.0.clone()),
        via: session.via.clone(),
    };
    let code = random_secret();
    let expires_at = now.0.saturating_add(auth.handoff_secs);
    let row = CeremonyRow {
        handle: handle_of(&code),
        connection: CONNECTION.to_string(),
        phase: PHASE.to_string(),
        binder: binder_hash.to_string(),
        payload: handed.render(),
        minted_at: now,
        expires_at: Timestamp(expires_at),
    };
    let put = tokio::task::spawn_blocking(move || ceremonies.put(&row)).await;
    if let Err(failure) = StoreFailure::joined(put) {
        auth.record_refusal(ISSUE, "ceremony-store-unavailable", now);
        return failure.into_response();
    }
    record_act(
        &auth,
        &who,
        ISSUE,
        "issued",
        format!(
            "principal={who} origin={} via={}",
            handed.origin,
            handed.via.as_deref().unwrap_or("-")
        ),
        now,
    );
    let joiner = if return_to.contains('?') { '&' } else { '?' };
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Json(json!({
            "redirect": format!("{return_to}{joiner}{PARAM}={code}"),
            "expires_at": expires_at,
        })),
    )
        .into_response()
}

/// `POST /auth/handoff/redeem`: swap a code and its binder for a session.
pub async fn redeem(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now_secs = (auth.now)();
    let now = Timestamp(now_secs);
    let text = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(code), Some(binder)) = (text("code"), text("binder")) else {
        auth.record_refusal(REDEEM, "malformed-request", now);
        return refused();
    };
    let Some(ceremonies) = auth.ceremonies.clone() else {
        auth.record_refusal(REDEEM, "no-ceremony-store", now);
        return unconfigured();
    };
    let taken = tokio::task::spawn_blocking(move || {
        ceremonies.take_bound(
            &handle_of(&code),
            CONNECTION,
            PHASE,
            &handle_of(&binder),
            now,
        )
    })
    .await;
    let payload = match taken {
        Ok(Ok(Some(p))) => p,
        // Unknown, spent, expired and filed under another binder are one
        // reason: the store cannot tell them apart, by design.
        Ok(Ok(None)) => {
            auth.record_refusal(REDEEM, "unknown-spent-expired-or-unbound", now);
            return refused();
        }
        Ok(Err(e)) => {
            auth.record_refusal(REDEEM, "ceremony-store-unavailable", now);
            return StoreFailure::from(e).into_response();
        }
        Err(_) => {
            auth.record_refusal(REDEEM, "ceremony-store-unavailable", now);
            return StoreFailure::task_failed().into_response();
        }
    };
    let Some(handed) = Handed::read(&payload) else {
        auth.record_refusal(REDEEM, "malformed-ceremony", now);
        return refused();
    };
    // A browser always says which page is asking. A page on another origin
    // that came by the binder is still refused.
    if let Some(asking) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let same = ordo_identity::origin::Origin::parse(asking)
            .is_some_and(|o| o.to_string() == handed.origin);
        if !same {
            auth.record_refusal(REDEEM, "origin-mismatch", now);
            return refused();
        }
    }
    let principal = ordo_identity::PrincipalId(handed.principal.clone());
    let started_at = Timestamp(handed.started_at);
    if let Some(r) = auth.revocations.as_ref() {
        if r.is_revoked(&ordo_identity::SessionId(handed.session.clone()))
            || r.principal_ended(&principal, started_at)
        {
            auth.record_refusal(REDEEM, "issuing-session-ended", now);
            return refused();
        }
    }
    match deleted(&auth, &handed.principal).await {
        Ok(false) => {}
        Ok(true) => {
            auth.record_refusal(REDEEM, "identity-deleted", now);
            return refused();
        }
        Err(failure) => {
            auth.record_refusal(REDEEM, "directory-unavailable", now);
            return failure.into_response();
        }
    }
    let ceiling = started_at.0.saturating_add(auth.session_max_secs);
    if now_secs >= ceiling {
        auth.record_refusal(REDEEM, "ceiling-reached", now);
        return refused();
    }
    let session = ordo_identity::SessionClaim {
        id: ordo_identity::SessionId::mint(),
        started_at,
        second_step: handed
            .second_step
            .as_deref()
            .and_then(ordo_identity::SecondStepKind::parse),
        via: Some(VIA.to_string()),
        // A provider's two-step claim was made on the page that signed in,
        // and stays there, as a passkey's presence does.
        upstream_presence: None,
    };
    let expires_at = now_secs.saturating_add(auth.session_secs).min(ceiling);
    let spec = ordo_identity::TokenSpec {
        principal: principal.clone(),
        expires: Timestamp(expires_at),
        client: handed.client.clone().map(ordo_identity::ClientId),
        // Presence is a touch on that page at that moment. This page made
        // none, so it carries none (`plan-subdomain-signin.md`).
        user_presence_at: None,
        capability: None,
        delegation: None,
        session: Some(session),
    };
    let token = ordo_identity::mint(&auth.token_key, &spec);
    record_act(
        &auth,
        &handed.principal,
        REDEEM,
        "redeemed",
        format!("principal={} origin={}", handed.principal, handed.origin),
        now,
    );
    auth.record(
        Some(&principal),
        REDEEM,
        ordo_secevent::EventKind::Authenticated,
        now,
    );
    // The page on this subdomain is remembered as well ([`crate::renewal`]).
    let mut answer = json!({
        "token": token,
        "via": VIA,
        "expires_at": expires_at,
        "session_expires_at": ceiling,
    });
    let cookie = auth
        .finish_sign_in(&headers, Some(&body), &spec, None, &mut answer, now)
        .await;
    super::with_cookie(
        (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "no-store")],
            Json(answer),
        )
            .into_response(),
        cookie,
    )
}

/// The two routes, for [`super::router`] and the served door
/// (`serve::platform_router`).
pub fn routes<S>() -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
    Option<SharedAuth>: axum::extract::FromRef<S>,
{
    axum::Router::new()
        .route("/auth/handoff", axum::routing::post(issue))
        .route("/auth/handoff/redeem", axum::routing::post(redeem))
}
