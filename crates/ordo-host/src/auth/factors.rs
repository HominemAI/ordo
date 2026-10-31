//! A person's second factors: enrol, list, remove, and the policy that makes
//! their own sign-in owe a step (`plan-second-factor.md` §3, §4, §7).
//!
//! The routes take the session's bearer token. What proves the person before
//! a factor changes is [`authorise_change`], the one place that rule lives.
//!
//! Everything refused is the one answer. The stream has the reason.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::second_step::DeploymentMode;
use ordo_identity::{Timestamp, TokenClaims};
use serde_json::{json, Value};

use super::{refused, step_up_claims, unconfigured, AuthState, SharedAuth, StoreFailure};
use crate::second_factors::{ConfirmError, KeyEnrolError, SecondFactors};

/// The word a label is trimmed to fit: what the person called the factor.
const LABEL_MAX: usize = 60;
/// An account name shown in an authenticator app.
const ACCOUNT_MAX: usize = 120;

/// Who may enrol a factor with this request, and the wired second step.
///
/// Two ways in, and only two:
///
/// * **A person made to enrol.** A sign-in that a level requires a second step
///   of, by a person who holds none, has no token. Its handle and binder
///   (`{"second_step", "binder"}` in the body) authorise the enrolment routes
///   for that person until it expires, and nothing else.
/// * **A session, and proof.** A live session that is neither a capability nor
///   a delegation, in a deployment that has not turned the second step off.
///   Starting an enrolment (`needs_proof`) also needs a fresh first factor: a
///   fresh passkey assertion in the session, or a reauth handle in `x-reauth`
///   (with the second step behind it when the person holds a factor). Finishing
///   one does not: it is bound to the row the start made, which is bound to the
///   person.
///
/// Returns the person and the wired second step.
pub(super) async fn authorise_change(
    auth: &Arc<AuthState>,
    headers: &axum::http::HeaderMap,
    body: &Value,
    op: &str,
    now: Timestamp,
    needs_proof: bool,
) -> Result<(String, Arc<SecondFactors>), Box<Response>> {
    let Some(sf) = auth.second_factors().cloned() else {
        auth.record_refusal(op, "not-wired", now);
        return Err(Box::new(refused()));
    };
    if sf.config().mode == DeploymentMode::Off {
        auth.record_refusal(op, "second-factor-off", now);
        return Err(Box::new(refused()));
    }
    if let (Some(handle), Some(binder)) =
        (text(body, "second_step", 128), text(body, "binder", 128))
    {
        let auth2 = auth.clone();
        let peeked = tokio::task::spawn_blocking(move || {
            super::second_step::peek_enrolment(&auth2, &handle, &binder, now)
        })
        .await;
        return match peeked {
            Ok(Ok(principal)) => Ok((principal, sf)),
            Ok(Err(reason)) => {
                auth.record_refusal(op, reason, now);
                Err(Box::new(refused()))
            }
            Err(_) => {
                auth.record_refusal(op, "ceremony-task-failed", now);
                Err(Box::new(refused()))
            }
        };
    }
    let claims = step_up_claims(auth, headers, op, now)?;
    if needs_proof && !super::recover::fresh_presence(&claims, now) {
        prove_change(auth, headers, &claims.principal.0, op, now).await?;
    }
    Ok((claims.principal.0.clone(), sf))
}

/// What a change to a factor needs: a fresh passkey assertion in the session,
/// or a reauth handle. A passkey assertion stands for both proofs, as it does
/// for issuing backup codes (R4).
async fn proof_of_person(
    auth: &Arc<AuthState>,
    headers: &axum::http::HeaderMap,
    claims: &TokenClaims,
    op: &str,
    now: Timestamp,
) -> Result<(), Box<Response>> {
    if super::recover::fresh_presence(claims, now) {
        return Ok(());
    }
    prove_change(auth, headers, &claims.principal.0, op, now).await
}

/// The proof a change to a person's factors, or to their backup codes, needs
/// when they have no fresh presence: a reauth handle from `/auth/complete
/// {"reauth": true}`, for **this** person, spent here, and a second step behind
/// it when they hold a factor. A missing handle is a `403` that says what to
/// do, since the person is signed in and owed the reason. A handle that is
/// wrong in any way is the one answer.
///
/// **The handle stands in for presence only for a person who holds no
/// passkey.** A person with one has presence to give, and a handle is minted
/// after any first factor, a quarantined passkey's included. Accepting it
/// from them let somebody in through a stolen recovery code mint fresh codes
/// during the quarantine (audit F-1, 2026-10-24), against
/// `plan-passkey-recovery.md` §7. So a passkey holder is told to step up,
/// and a quarantined passkey gives no presence until its quarantine ends.
pub(super) async fn prove_change(
    auth: &Arc<AuthState>,
    headers: &axum::http::HeaderMap,
    principal: &str,
    op: &str,
    now: Timestamp,
) -> Result<(), Box<Response>> {
    let Some(sf) = auth.second_factors().cloned() else {
        auth.record_refusal(op, "not-wired", now);
        return Err(Box::new(refused()));
    };
    let (auth2, who) = (auth.clone(), principal.to_string());
    let holds = tokio::task::spawn_blocking(move || holds_passkey(&auth2, &who)).await;
    match holds {
        Ok(Ok(false)) => {}
        Ok(Ok(true)) => {
            auth.record_refusal(op, "presence-required", now);
            return Err(Box::new(
                EdgeError::new(
                    EdgeCode::PresenceRequired,
                    "this needs a fresh assertion from one of your passkeys: POST /auth/step-up \
                     first. A passkey added by a recovery counts once its quarantine ends",
                )
                .into_response(),
            ));
        }
        Ok(Err(failure)) => {
            auth.record_refusal(op, "directory-unavailable", now);
            return Err(Box::new(failure.into_response()));
        }
        Err(_) => {
            auth.record_refusal(op, "ceremony-task-failed", now);
            return Err(Box::new(refused()));
        }
    }
    if !headers.contains_key(super::second_step::REAUTH_HEADER) {
        auth.record_refusal(op, "reauth-missing", now);
        return Err(Box::new(
            EdgeError::new(
                EdgeCode::ReauthenticationRequired,
                "prove a first factor again: POST /auth/complete with \"reauth\": true, then send the handle in the x-reauth header",
            )
            .into_response(),
        ));
    }
    let (auth2, headers2, who) = (auth.clone(), headers.clone(), principal.to_string());
    let done = tokio::task::spawn_blocking(move || {
        let proof = super::second_step::redeem_reauth(&auth2, &headers2, &who, now)?;
        let holds = sf
            .holds_any(&who)
            .map_err(|_| "second-factor-store-unavailable")?;
        if holds && !proof.second_step_done {
            return Err("second-step-required");
        }
        Ok(())
    })
    .await;
    match done {
        Ok(Ok(())) => Ok(()),
        Ok(Err(reason)) => {
            auth.record_refusal(op, reason, now);
            Err(Box::new(refused()))
        }
        Err(_) => {
            auth.record_refusal(op, "ceremony-task-failed", now);
            Err(Box::new(refused()))
        }
    }
}

/// Does this person hold a passkey that is not retired? A quarantined one
/// counts: it is theirs, and it gives presence once the quarantine ends.
/// Synchronous, for a blocking task.
pub(super) fn holds_passkey(auth: &AuthState, principal: &str) -> Result<bool, StoreFailure> {
    let dir = auth.lock_directory();
    let links = dir
        .links_of(&ordo_identity::PrincipalId(principal.to_string()))
        .map_err(StoreFailure::from)?;
    Ok(links
        .iter()
        .any(|(a, meta)| a.0.starts_with("passkey:") && meta.retired.is_none()))
}

fn text(body: &Value, key: &str, max: usize) -> Option<String> {
    body.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(max).collect())
}

/// Start enrolling a code-generating factor: `POST /auth/factors/totp/begin`
/// `{"label"?, "account"?}` with a session's bearer.
///
/// Answers the `otpauth://` URI and the secret, **once**, `no-store`. Nothing
/// is enrolled: the secret waits in a ceremony row that expires, and only a
/// code from the person's app at [`totp_confirm`] writes the factor.
pub async fn totp_begin(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.factors.totp.begin";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (principal, sf) = match authorise_change(&auth, &headers, &body, OP, now, true).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let (Some(totp), Some(ceremonies)) = (sf.totp().cloned(), auth.ceremonies.clone()) else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let label = text(&body, "label", LABEL_MAX).unwrap_or_else(|| "Authenticator".to_string());
    let account = text(&body, "account", ACCOUNT_MAX).unwrap_or_else(|| principal.clone());
    let issuer = auth
        .passkey
        .as_ref()
        .map(|p| p.relying_party().name().to_string())
        .unwrap_or_else(|| "Ordo".to_string());
    let who = principal.clone();
    let label2 = label.clone();
    let done = tokio::task::spawn_blocking(move || {
        totp.begin_enrol(ceremonies.as_ref(), &who, &label2, &issuer, &account, now.0)
    })
    .await;
    match done {
        Ok(Ok(b)) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(json!({
                "factor": b.factor,
                "kind": "totp",
                "label": label,
                "secret": b.secret_base32,
                "uri": b.uri,
                "expires_at": b.expires_at,
            })),
        )
            .into_response(),
        Ok(Err(_)) => {
            auth.record_refusal(OP, "enrol-refused", now);
            refused()
        }
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}

/// Finish enrolling a code-generating factor: `POST /auth/factors/totp/confirm`
/// `{"factor", "code"}` with a session's bearer. One code from the person's
/// app against the secret `begin` sealed. A right code writes the factor; a
/// wrong one leaves the enrolment open until its attempts run out.
pub async fn totp_confirm(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.factors.totp.confirm";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (principal, sf) = match authorise_change(&auth, &headers, &body, OP, now, false).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let (Some(totp), Some(ceremonies)) = (sf.totp().cloned(), auth.ceremonies.clone()) else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let (Some(factor), Some(code)) = (text(&body, "factor", 80), text(&body, "code", 32)) else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let who = principal.clone();
    let done = tokio::task::spawn_blocking(move || {
        totp.confirm_enrol(ceremonies.as_ref(), &who, &factor, &code, now.0)
    })
    .await;
    match done {
        Ok(Ok(enrolled)) => {
            let f = enrolled.factor;
            auth.record(
                Some(&ordo_identity::PrincipalId(principal.clone())),
                OP,
                ordo_secevent::EventKind::SecondFactor {
                    act: "enrolled".into(),
                    detail: format!(
                        "principal={} kind={} factor={} by={}",
                        f.principal, f.kind, f.id, f.principal
                    ),
                },
                now,
            );
            (
                StatusCode::OK,
                Json(json!({
                    "factor": f.id, "kind": f.kind, "label": f.label,
                    "enrolled_at": f.enrolled_at,
                })),
            )
                .into_response()
        }
        Ok(Err(ConfirmError::Wrong)) => {
            auth.record_refusal(OP, "enrol-code-wrong", now);
            refused()
        }
        Ok(Err(ConfirmError::Refused(reason))) => {
            auth.record_refusal(OP, reason, now);
            refused()
        }
        Ok(Err(ConfirmError::Unavailable(reason, failure))) => {
            auth.record_refusal(OP, reason, now);
            failure.into_response()
        }
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}

/// Start registering a security key as a second factor:
/// `POST /auth/factors/key/begin` `{"label"?}` with a session's bearer.
///
/// The options are the ones a passkey registration answers with two
/// differences: the credential need not be discoverable (`resident_key:
/// discouraged`) and the user verification is `preferred`, because many
/// security keys have no PIN. Attestation stays `none`: the platform does not
/// verify what a key is (owner, 2026-09-29).
pub async fn key_begin(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.factors.key.begin";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (principal, sf) = match authorise_change(&auth, &headers, &body, OP, now, true).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let (Some(key), Some(passkey)) = (sf.key().cloned(), auth.passkey.clone()) else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let label = text(&body, "label", LABEL_MAX).unwrap_or_else(|| "Security key".to_string());
    let who = principal.clone();
    let done = tokio::task::spawn_blocking(move || key.begin_enrol(&who, now.0)).await;
    match done {
        Ok(Ok(options)) => {
            let mut answer = super::creation_options_json(
                &options,
                passkey.relying_party().name(),
                &label,
                auth.passkey_timeout_ms(),
            );
            answer["resident_key"] = json!("discouraged");
            (StatusCode::OK, Json(answer)).into_response()
        }
        Ok(Err(_)) => {
            auth.record_refusal(OP, "enrol-refused", now);
            refused()
        }
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}

/// Finish registering a security key: `POST /auth/factors/key/complete` with
/// the registration fields of `/auth/complete` (`kind: passkey_registration`)
/// and `{"label"?}`, and a session's bearer.
///
/// The credential is stored with the role `second-factor` and the factor is
/// written beside it. **Nothing is linked in `$authenticator`**, so the key is
/// not a way to sign in and the directory never counts it. The record says
/// *security key*, whether the key verified the user, and nothing about a model.
pub async fn key_complete(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.factors.key.complete";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let (principal, sf) = match authorise_change(&auth, &headers, &body, OP, now, false).await {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let Some(key) = sf.key().cloned() else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let Some(ordo_identity::CompleteRequest::PasskeyRegistration {
        challenge,
        client_data_json,
        attestation_object,
    }) = super::parse_complete(&body)
    else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let label = text(&body, "label", LABEL_MAX).unwrap_or_else(|| "Security key".to_string());
    let who = principal.clone();
    let done = tokio::task::spawn_blocking(move || {
        key.complete_enrol(
            &who,
            &challenge,
            &client_data_json,
            &attestation_object,
            &label,
            now.0,
        )
    })
    .await;
    match done {
        Ok(Ok(enrolled)) => {
            let f = enrolled.factor;
            auth.record(
                Some(&ordo_identity::PrincipalId(principal.clone())),
                OP,
                ordo_secevent::EventKind::SecondFactor {
                    act: "enrolled".into(),
                    detail: format!(
                        "principal={} kind={} factor={} by={} user_verified={}",
                        f.principal,
                        f.kind,
                        f.id,
                        f.principal,
                        if enrolled.user_verified { "yes" } else { "no" }
                    ),
                },
                now,
            );
            (
                StatusCode::OK,
                Json(json!({
                    "factor": f.id, "kind": f.kind, "label": f.label,
                    "enrolled_at": f.enrolled_at, "user_verified": enrolled.user_verified,
                })),
            )
                .into_response()
        }
        Ok(Err(KeyEnrolError::Refused(reason))) => {
            auth.record_refusal(OP, reason, now);
            refused()
        }
        Ok(Err(KeyEnrolError::Unavailable(reason, failure))) => {
            auth.record_refusal(OP, reason, now);
            failure.into_response()
        }
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}

/// The person's factors: `GET /auth/factors` with a session's bearer.
/// Each factor's id, kind, label, when it was enrolled and last used, and
/// whether the throttle locked it; how many backup codes remain; and the
/// person's own level and the deployment's. Never a secret.
pub async fn list(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    const OP: &str = "auth.factors";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let claims = match step_up_claims(&auth, &headers, OP, now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let Some(sf) = auth.second_factors().cloned() else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    let who = claims.principal.0.clone();
    let sf2 = sf.clone();
    let read = tokio::task::spawn_blocking(move || {
        Ok::<_, ordo_identity::StoreFault>((
            sf2.factors_of(&who)?,
            sf2.backup().and_then(|b| b.remaining(&who).ok()),
            sf2.person_mode(&who)?,
        ))
    })
    .await;
    let (factors, remaining, mode) = match StoreFailure::joined(read) {
        Ok(read) => read,
        Err(failure) => {
            auth.record_refusal(OP, "second-factor-store-unavailable", now);
            return failure.into_response();
        }
    };
    (
        StatusCode::OK,
        Json(json!({
            "factors": factors.iter().map(|f| json!({
                "id": f.id, "kind": f.kind, "label": f.label,
                "enrolled_at": f.enrolled_at, "last_used_at": f.last_used_at,
                "locked": f.locked(),
            })).collect::<Vec<_>>(),
            "backup_codes_remaining": remaining,
            "require_second_step": mode.as_str(),
            "second_step": sf.config().mode.as_str(),
        })),
    )
        .into_response()
}

/// Remove one of the person's factors: `POST /auth/factors/{id}/remove` with a
/// session's bearer and proof (a fresh passkey assertion, or a reauth handle
/// with the second step behind it).
///
/// **Every session the person has ends** (`SessionRevocations::end_all`), so a
/// factor removed because a device was lost does not leave the thief's session
/// running. This session is given a replacement token that starts a second
/// later, which the epoch does not reach, so the person who proved themselves
/// is not signed out for it. A key's credential goes with its factor. A
/// factor that is not the person's is the one refusal. Allowed when the
/// deployment has turned the second step off, so a person can clean up.
pub async fn remove(
    State(auth): State<Option<SharedAuth>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    const OP: &str = "auth.factors.remove";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now_secs = (auth.now)();
    let now = Timestamp(now_secs);
    let claims = match step_up_claims(&auth, &headers, OP, now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let (Some(sf), Some(revocations)) = (auth.second_factors().cloned(), auth.revocations.clone())
    else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    if let Err(r) = proof_of_person(&auth, &headers, &claims, OP, now).await {
        return *r;
    }
    let who = claims.principal.clone();
    let (who2, sf2) = (who.0.clone(), sf.clone());
    let removed = tokio::task::spawn_blocking(move || sf2.remove_factor(&who2, &id)).await;
    let removed = match removed {
        Ok(Ok(Some(f))) => f,
        Ok(Ok(None)) => {
            auth.record_refusal(OP, "no-such-factor", now);
            return refused();
        }
        Ok(Err(why)) => {
            auth.record_refusal(OP, "second-factor-store-unavailable", now);
            return StoreFailure::from(why).into_response();
        }
        Err(_) => {
            auth.record_refusal(OP, "second-factor-store-unavailable", now);
            return StoreFailure::task_failed().into_response();
        }
    };
    auth.record(
        Some(&who),
        OP,
        ordo_secevent::EventKind::SecondFactor {
            act: "removed".into(),
            detail: format!(
                "principal={} kind={} factor={} by=self",
                who.0, removed.kind, removed.id
            ),
        },
        now,
    );
    // Every session ends, this one included, and this one is handed back.
    let until = Timestamp(now_secs.saturating_add(auth.session_max_secs));
    let (rv, who3) = (revocations, who.clone());
    let ended = tokio::task::spawn_blocking(move || rv.end_all(&who3, now, until)).await;
    if !matches!(ended, Ok(Ok(()))) {
        auth.record_refusal(OP, "session-store-unavailable", now);
        return super::store_unavailable();
    }
    auth.record(
        Some(&who),
        OP,
        ordo_secevent::EventKind::SessionsEnded {
            reason: "second-factor-removed".into(),
        },
        now,
    );
    let session = ordo_identity::SessionClaim {
        id: ordo_identity::SessionId::mint(),
        started_at: Timestamp(now_secs + 1),
        second_step: claims.session.as_ref().and_then(|s| s.second_step),
        via: claims.session.as_ref().and_then(|s| s.via.clone()),
        upstream_presence: claims
            .session
            .as_ref()
            .and_then(|s| s.upstream_presence.clone()),
    };
    let spec = ordo_identity::TokenSpec {
        principal: who,
        expires: Timestamp(now_secs + auth.session_secs),
        client: claims.client.clone(),
        user_presence_at: claims.user_presence_at,
        capability: None,
        delegation: None,
        session: Some(session),
    };
    (
        StatusCode::OK,
        Json(json!({
            "removed": removed.id,
            "token": ordo_identity::mint(&auth.token_key, &spec),
            "expires_at": now_secs + auth.session_secs,
            "session_expires_at": now_secs + 1 + auth.session_max_secs,
        })),
    )
        .into_response()
}

/// A person's own level: `POST /auth/factors/policy`
/// `{"require_second_step": "when-enrolled" | "also-for-passkeys"}` with a
/// session's bearer and proof. `also-for-passkeys` asks for the step after a
/// passkey too. Recorded.
pub async fn policy(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.factors.policy";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let claims = match step_up_claims(&auth, &headers, OP, now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let Some(sf) = auth.second_factors().cloned() else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    let Some(mode) = text(&body, "require_second_step", 32)
        .and_then(|w| ordo_identity::second_step::PersonMode::parse(&w))
    else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    if let Err(r) = proof_of_person(&auth, &headers, &claims, OP, now).await {
        return *r;
    }
    let who = claims.principal.0.clone();
    let done = tokio::task::spawn_blocking(move || sf.set_person_mode(&who, mode, now.0)).await;
    if let Err(failure) = StoreFailure::joined(done) {
        auth.record_refusal(OP, "second-factor-store-unavailable", now);
        return failure.into_response();
    }
    auth.record(
        Some(&claims.principal),
        OP,
        ordo_secevent::EventKind::SecondFactor {
            act: "policy-changed".into(),
            detail: format!("principal={} mode={}", claims.principal.0, mode.as_str()),
        },
        now,
    );
    (
        StatusCode::OK,
        Json(json!({"require_second_step": mode.as_str()})),
    )
        .into_response()
}

/// The routes this module adds. Merged by [`super::second_step::routes`].
pub fn routes<S>() -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
    Option<SharedAuth>: axum::extract::FromRef<S>,
{
    axum::Router::new()
        .route("/auth/factors/totp/begin", axum::routing::post(totp_begin))
        .route(
            "/auth/factors/totp/confirm",
            axum::routing::post(totp_confirm),
        )
        .route("/auth/factors", axum::routing::get(list))
        .route("/auth/factors/policy", axum::routing::post(policy))
        .route("/auth/factors/{id}/remove", axum::routing::post(remove))
        .route("/auth/factors/key/begin", axum::routing::post(key_begin))
        .route(
            "/auth/factors/key/complete",
            axum::routing::post(key_complete),
        )
}
