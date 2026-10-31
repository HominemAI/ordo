//! The person's half of a recovery: register a new passkey with a recovery
//! ticket, and contest a recovery they did not want
//! (`plan-passkey-recovery.md` R3).
//!
//! These routes take no bearer token, because the person has none: that is
//! why they are recovering. What they present is a ticket a person with the
//! authority handed them (a recovery code, an administrator, the deployment
//! owner), or an assertion from a passkey a recovery retired.
//!
//! Everything refused is the one answer. The stream has the reason.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::{AuthenticatorId, CompleteRequest, Timestamp};
use serde_json::{json, Value};

use super::{
    auth_error_reason, creation_options_json, parse_complete, refused, unconfigured, SharedAuth,
    StoreFailure,
};
use crate::recovery::{CompleteError, State as CaseState};

fn ticket_of(body: &Value) -> Option<&str> {
    body.get("ticket").and_then(|t| t.as_str())
}

fn answer(e: CompleteError, auth: &super::AuthState, op: &str, now: Timestamp) -> Response {
    match e {
        CompleteError::Refused(reason) => {
            auth.record_refusal(op, reason, now);
            refused()
        }
        CompleteError::Unavailable(reason, failure) => {
            auth.record_refusal(op, reason, now);
            failure.into_response()
        }
    }
}

/// Start registering the passkey a recovery ends in: `POST /auth/recover/begin`
/// `{"ticket"}`. Answers the creation options, and binds the ceremony to the
/// ticket so it cannot finish with another one.
pub async fn begin(State(auth): State<Option<SharedAuth>>, Json(body): Json<Value>) -> Response {
    const OP: &str = "auth.recover.begin";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (Some(passkey), Some(recovery)) = (auth.passkey.clone(), auth.recovery().cloned()) else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let Some(ticket) = ticket_of(&body).map(str::to_string) else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let p2 = passkey.clone();
    let minted = tokio::task::spawn_blocking(move || {
        recovery.look(&ticket)?;
        let binding = crate::tickets::hash_of(&ticket);
        p2.begin_registration_bound(now, &[], Some(&binding))
            .map_err(|e| CompleteError::Unavailable("ceremony-store-unavailable", e.into()))
    })
    .await;
    match minted {
        Ok(Ok(options)) => (
            StatusCode::OK,
            Json(creation_options_json(
                &options,
                passkey.relying_party().name(),
                passkey.relying_party().name(),
                auth.passkey_timeout_ms(),
            )),
        )
            .into_response(),
        Ok(Err(e)) => answer(e, &auth, OP, now),
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}

/// Finish a recovery: `POST /auth/recover/complete` with the ticket and the
/// registration fields of `/auth/complete` (`kind: passkey_registration`),
/// and optionally `"lost": [authenticator ids]` naming what to retire (all of
/// them when absent).
///
/// No token comes back. Every session the person had has ended and the new
/// passkey is quarantined, so the person signs in with it in a moment, gets
/// a session that works for everything except step-up, and is told when
/// step-up becomes available.
pub async fn complete(State(auth): State<Option<SharedAuth>>, Json(body): Json<Value>) -> Response {
    const OP: &str = "auth.recover.complete";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (Some(passkey), Some(recovery), Some(revocations)) = (
        auth.passkey.clone(),
        auth.recovery().cloned(),
        auth.revocations.clone(),
    ) else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let Some(ticket) = ticket_of(&body).map(str::to_string) else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let Some(CompleteRequest::PasskeyRegistration {
        challenge,
        client_data_json,
        attestation_object,
    }) = parse_complete(&body)
    else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let lost: Vec<AuthenticatorId> = body
        .get("lost")
        .and_then(|l| l.as_array())
        .map(|l| {
            l.iter()
                .filter_map(|v| v.as_str())
                .map(|s| AuthenticatorId(s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let auth2 = auth.clone();
    let done = tokio::task::spawn_blocking(move || {
        let pending = passkey
            .verify_registration(&challenge, &client_data_json, &attestation_object, now)
            .map_err(|e| match e {
                ordo_identity::AuthError::StoreUnavailable(c) => {
                    CompleteError::Unavailable("credential-store-unavailable", c.into())
                }
                other => CompleteError::Refused(auth_error_reason(&other)),
            })?;
        if pending.binding() != Some(crate::tickets::hash_of(&ticket).as_str()) {
            return Err(CompleteError::Refused("ticket-binding"));
        }
        let new = pending.subject().authenticator.clone();
        // The key is stored before the recovery so the directory never
        // links a credential that verifies nothing. If the recovery refuses,
        // the key is taken out again.
        passkey.store_registration(&pending).map_err(|e| match e {
            ordo_identity::AuthError::StoreUnavailable(c) => {
                CompleteError::Unavailable("credential-store-unavailable", c.into())
            }
            other => CompleteError::Refused(auth_error_reason(&other)),
        })?;
        let raw = pending.stored().credential.id().to_vec();
        let mut dir = auth2.lock_directory();
        let out = recovery.complete(
            &mut dir,
            revocations.as_ref(),
            auth2.session_max_secs,
            &ticket,
            &new,
            &lost,
        );
        if out.is_err() {
            let _ = passkey.remove_credential(&raw);
        }
        out
    })
    .await;
    match done {
        Ok(Ok(c)) => (
            StatusCode::OK,
            Json(json!({
                "recovered": true,
                "case": c.case,
                "authenticator": c.added.0,
                "retired": c.retired.iter().map(|a| a.0.clone()).collect::<Vec<_>>(),
                "presence_available_at": c.presence_from.0,
                "contest_until": c.contest_until.0,
                "warnings": c.warnings,
            })),
        )
            .into_response(),
        Ok(Err(e)) => answer(e, &auth, OP, now),
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}

/// Contest a recovery: `POST /auth/recover/contest` with a `passkey_assertion`
/// body whose challenge came from `/auth/begin`.
///
/// Two people can do it, and both by holding a passkey the person had. A
/// passkey a recovery **retired** contests a completed recovery: the link the
/// recovery added goes, the retired ones come back, every session ends again.
/// A passkey still **active** cancels a recovery that has not completed yet,
/// unless an earlier recovery added it and that recovery's contest window
/// has not ended. Nothing else is done with either, and a passkey that is
/// neither is the one refusal.
pub async fn contest(State(auth): State<Option<SharedAuth>>, Json(body): Json<Value>) -> Response {
    const OP: &str = "auth.recover.contest";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (Some(passkey), Some(recovery), Some(revocations)) = (
        auth.passkey.clone(),
        auth.recovery().cloned(),
        auth.revocations.clone(),
    ) else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let Some(req @ CompleteRequest::PasskeyAssertion { .. }) = parse_complete(&body) else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let auth2 = auth.clone();
    let done = tokio::task::spawn_blocking(move || {
        let subject = ordo_identity::AuthProvider::complete(passkey.as_ref(), &req, now).map_err(
            |e| match e {
                ordo_identity::AuthError::StoreUnavailable(c) => {
                    CompleteError::Unavailable("credential-store-unavailable", c.into())
                }
                other => CompleteError::Refused(auth_error_reason(&other)),
            },
        )?;
        let by = subject.authenticator;
        let mut dir = auth2.lock_directory();
        let meta = dir
            .link_meta(&by)
            .map_err(|e| CompleteError::Unavailable("directory-unavailable", e.into()))?
            .ok_or(CompleteError::Refused("nothing-to-contest"))?;
        if meta.retired.is_some() {
            let c =
                recovery.contest(&mut dir, revocations.as_ref(), auth2.session_max_secs, &by)?;
            for gone in &c.removed {
                if let Some(raw) = gone
                    .0
                    .strip_prefix("passkey:")
                    .and_then(|id| ordo_identity::passkey::credential_id_from_b64url(id).ok())
                {
                    let _ = passkey.remove_credential(&raw);
                }
            }
            return Ok(json!({"contested": c.case, "restored": c.restored.len()}));
        }
        // An active passkey can stop a recovery that has not completed,
        // unless a recovery added it and that recovery's contest window is
        // still open (audit F-3, 2026-10-24). Whoever recovered with a
        // stolen code would otherwise cancel the case the real owner opens
        // to get back in. The window is the time the case was given, so a
        // deployment owner closing it early does not shorten this.
        if let Some(added_by) = meta.recovered_case.as_deref() {
            let window_open = recovery
                .case(added_by)
                .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
                .is_some_and(|c| {
                    matches!(c.state, CaseState::Completed | CaseState::Closed)
                        && c.contest_until > now.0
                });
            if window_open {
                return Err(CompleteError::Refused(
                    "recovered-passkey-in-contest-window",
                ));
            }
        }
        let principal = dir
            .principal_for(&by)
            .map_err(|e| CompleteError::Unavailable("directory-unavailable", e.into()))?
            .ok_or(CompleteError::Refused("nothing-to-contest"))?;
        let case = recovery
            .live_for(&principal.0)
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?
            .filter(|c| matches!(c.state, CaseState::Pending | CaseState::Matured))
            .ok_or(CompleteError::Refused("nothing-to-contest"))?;
        let moved = recovery
            .cancel(
                &case.id,
                ordo_secevent::Subject::member(&principal.0),
                "contested-before-completion",
            )
            .map_err(|e| CompleteError::Unavailable("recovery-store-unavailable", e.into()))?;
        if !moved {
            return Err(CompleteError::Refused("nothing-to-contest"));
        }
        Ok(json!({"cancelled": case.id}))
    })
    .await;
    match done {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)).into_response(),
        Ok(Err(e)) => answer(e, &auth, OP, now),
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}

// ── recovery codes (R4) ──────────────────────────────────────────────────

/// `Some(the 403)` unless the session holds a fresh passkey assertion: the
/// gate linking has. A password session, a stale one and a quarantined
/// passkey's session are each refused, and the refusal says what to do, since
/// the person is authenticated and owed the reason.
/// Whether the session holds presence proved inside the step-up window.
pub(super) fn fresh_presence(claims: &ordo_identity::TokenClaims, now: Timestamp) -> bool {
    claims.user_presence_at.is_some_and(|p| {
        now.0
            .checked_sub(p.0)
            .is_some_and(|age| age <= ordo_identity::access::DEFAULT_STEP_UP_MAX_AGE_SECS)
    })
}

fn require_presence(
    auth: &super::AuthState,
    claims: &ordo_identity::TokenClaims,
    op: &str,
    now: Timestamp,
    what: &str,
) -> Option<Response> {
    if fresh_presence(claims, now) {
        return None;
    }
    auth.record_refusal(op, "presence-required", now);
    Some(
        EdgeError::new(
            EdgeCode::PresenceRequired,
            format!("{what} needs a fresh passkey assertion: POST /auth/step-up first"),
        )
        .into_response(),
    )
}

/// Acknowledge a recovery: `POST /auth/recover/ack {"case"}` with a session's
/// bearer and **fresh presence**.
///
/// A person who was recovered is told at every sign-in, and stays told until
/// they say they have seen it. Saying so needs presence, so a session an
/// intruder holds cannot quietly clear the notice the real person would see.
/// The acknowledgement is recorded. `204` when it was theirs to acknowledge,
/// the one refusal otherwise.
pub async fn acknowledge(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.recover.ack";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(recovery) = auth.recovery().cloned() else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    let claims = match super::step_up_claims(&auth, &headers, OP, now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    if let Some(r) = require_presence(&auth, &claims, OP, now, "acknowledging a recovery") {
        return r;
    }
    let Some(case) = body
        .get("case")
        .and_then(|c| c.as_str())
        .map(str::to_string)
    else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let who = claims.principal.0.clone();
    let done = tokio::task::spawn_blocking(move || recovery.acknowledge(&case, &who)).await;
    match StoreFailure::joined(done) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => {
            auth.record_refusal(OP, "nothing-to-acknowledge", now);
            refused()
        }
        Err(failure) => {
            auth.record_refusal(OP, "recovery-store-unavailable", now);
            failure.into_response()
        }
    }
}

/// Issue a new set of recovery codes: `POST /auth/recovery-codes` with a
/// session's bearer token. The old set is voided.
///
/// **Needs fresh presence**, the gate linking has, so a stolen bearer token
/// or a password session cannot mint a way back into the account. A session
/// whose passkey is quarantined has no presence and is refused too, so
/// somebody who got in through a recovery cannot entrench during the window.
/// The codes are in the answer once, and nowhere else: never mailed, never
/// shown again, and the response is `no-store`.
pub async fn issue_codes(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    const OP: &str = "auth.recovery-codes";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(codes) = auth.recovery().and_then(|r| r.codes().cloned()) else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    let claims = match super::step_up_claims(&auth, &headers, OP, now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    // Presence is the proof. A person with no passkey has none, so a fresh
    // first factor stands in for it, plus the second step when they hold a
    // factor (`plan-second-factor.md` §6.4, §7.1). The proof is a reauth
    // handle from `/auth/complete {"reauth": true}`. A person who holds a
    // passkey, a quarantined one included, is refused the stand-in by
    // `prove_change`: a recovery's quarantine must not be one reauth away.
    if !fresh_presence(&claims, now) {
        if !headers.contains_key(super::second_step::REAUTH_HEADER) {
            if let Some(r) = require_presence(&auth, &claims, OP, now, "issuing recovery codes") {
                return r;
            }
        } else if let Err(r) =
            super::factors::prove_change(&auth, &headers, &claims.principal.0, OP, now).await
        {
            return *r;
        }
    }
    let who = claims.principal.0.clone();
    let c2 = codes.clone();
    let allowed = {
        let (r2, who2) = (auth.recovery().cloned(), who.clone());
        tokio::task::spawn_blocking(move || r2.is_none_or(|r| r.codes_allowed(&who2)))
            .await
            .unwrap_or(false)
    };
    if !allowed {
        auth.record_refusal(OP, "codes-disabled-by-tenant", now);
        return EdgeError::new(
            EdgeCode::CodesDisabled,
            "the tenant that created this account has switched recovery codes off",
        )
        .into_response();
    }
    let backup = auth.second_factors().and_then(|s| s.backup().cloned());
    let who3 = who.clone();
    let issued = tokio::task::spawn_blocking(move || {
        let list = c2.issue(&who)?;
        // A fresh set unlocks the person's code redemption.
        if let Some(b) = backup {
            b.clear_lock(&who3)?;
        }
        Ok::<_, ordo_identity::StoreFault>(list)
    })
    .await;
    match StoreFailure::joined(issued) {
        Ok(list) => {
            if let Some(recovery) = auth.recovery() {
                recovery.record(
                    "codes-issued",
                    ordo_secevent::Subject::member(&claims.principal.0),
                    &claims.principal.0,
                    format!("principal={} count={}", claims.principal.0, list.len()),
                );
            }
            (
                StatusCode::OK,
                [(axum::http::header::CACHE_CONTROL, "no-store")],
                Json(json!({"codes": list, "count": list.len()})),
            )
                .into_response()
        }
        Err(failure) => {
            auth.record_refusal(OP, "code-store-unavailable", now);
            failure.into_response()
        }
    }
}

/// Spend a recovery code: `POST /auth/recover/code` `{"code"}`.
///
/// **A code alone opens a recovery case and nothing else.** What comes back is
/// a recovery ticket for a new passkey, and everything recovery does follows:
/// every session ends when the person registers it, the new passkey is
/// quarantined from step-up, and the passkeys they had can contest. The code
/// works at once (owner, 2026-09-29) unless the deployment sets a delay for
/// this path, and then the case matures later and no ticket is in the answer.
///
/// The case is opened **before** the code is spent, so a person who already
/// has a live case does not burn a code on a refusal. A wrong, used or
/// malformed code is the one answer. A deployment that has been sent too many
/// wrong codes this minute answers `429`.
pub async fn redeem_code(
    State(auth): State<Option<SharedAuth>>,
    Json(body): Json<Value>,
) -> Response {
    use crate::recovery::{Open, OpenError, Path};
    use crate::recovery_codes::CodeError;
    const OP: &str = "auth.recover.code";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(recovery) = auth.recovery().cloned() else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    let Some(codes) = recovery.codes().cloned() else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    let Some(typed) = body
        .get("code")
        .and_then(|c| c.as_str())
        .map(str::to_string)
    else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let done = tokio::task::spawn_blocking(move || {
        let principal = codes.peek(&typed)?;
        // A tenant may have switched codes off for the people it created.
        if !recovery.codes_allowed(&principal) {
            return Err(CodeError::Refused);
        }
        let opened = recovery
            .open(&Open {
                principal: &principal,
                path: Path::Code,
                opened_by: &principal,
                authority: "recovery-code",
                evidence: "recovery-code",
                reference: "",
                space: "",
                extra_delay_secs: 0,
                approvals: 0,
            })
            .map_err(|e| match e {
                OpenError::Live => CodeError::LiveCase,
                // The deployment switched this path off: the code is not
                // spent, and the answer is the one refusal.
                OpenError::Disabled | OpenError::OutsideProvider => CodeError::Refused,
                OpenError::Unavailable(e) => CodeError::Unavailable(e),
            })?;
        match codes.consume(&typed, "recovery") {
            Ok((who, left)) => {
                recovery.record(
                    "code-used",
                    ordo_secevent::Subject::member(&who),
                    &who,
                    format!(
                        "case={} principal={who} used_as=recovery remaining={left}",
                        opened.case
                    ),
                );
                Ok((opened, left))
            }
            Err(e) => {
                // A racing request spent it first. The case it opened has
                // nothing behind it.
                let _ = recovery.cancel(
                    &opened.case,
                    ordo_secevent::Subject::member(&principal),
                    "code-not-spent",
                );
                Err(e)
            }
        }
    })
    .await;
    match done {
        Ok(Ok((opened, left))) => {
            let mut answer = json!({
                "case": opened.case,
                "matures_at": opened.matures_at,
                "remaining": left,
            });
            if let (Some(ticket), Some(expires)) = (opened.ticket, opened.ticket_expires_at) {
                answer["ticket"] = json!(ticket);
                answer["ticket_expires_at"] = json!(expires);
            }
            (
                StatusCode::OK,
                [(axum::http::header::CACHE_CONTROL, "no-store")],
                Json(answer),
            )
                .into_response()
        }
        Ok(Err(CodeError::Refused)) => {
            auth.record_refusal(OP, "code-refused", now);
            refused()
        }
        Ok(Err(CodeError::Throttled { retry_after_secs })) => {
            auth.record_refusal(OP, "throttled", now);
            EdgeError::new(
                EdgeCode::TooManyAttempts,
                "too many wrong recovery codes; wait and try again",
            )
            .retry_after(retry_after_secs)
            .into_response()
        }
        // A person with a live case and a store that could not answer are
        // told nothing that differs: the first is a refusal, the second says
        // the store is down.
        Ok(Err(CodeError::LiveCase)) => {
            auth.record_refusal(OP, "case-live", now);
            refused()
        }
        Ok(Err(CodeError::Unavailable(why))) => {
            auth.record_refusal(OP, "store-unavailable", now);
            StoreFailure::from(why).into_response()
        }
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}
