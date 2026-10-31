//! A person's own authenticators: list them, add a passkey, remove one
//! (`plan-passkey-recovery.md` R2).
//!
//! This is recovery's path A. A person with two passkeys who loses one signs
//! in with the other and removes the lost one, and nothing weaker than a
//! passkey was introduced. It shares no code with recovery proper
//! (`security-logging-design.md`: recovery never shares a path with linking):
//! every act here is vouched for by a fresh assertion from an authenticator
//! the person still holds.
//!
//! # The vouching assertion travels in the request
//!
//! The token does not say which authenticator made its presence, and a token
//! minted with presence an hour ago says nothing about the person's hands
//! now. So link and unlink each carry their own assertion, minted through the
//! same step-up challenge the session route uses, and the authenticator that
//! made it is the one the audit event names as having vouched. A stolen
//! session token alone can therefore add nothing and remove nothing.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::{AuthenticatorId, CompleteRequest, LinkCeremony, PrincipalId, Timestamp};
use serde_json::{json, Value};

use super::{
    auth_error_reason, creation_options_json, parse_complete, refused, request_options_json,
    session_error_reason, step_up_claims, store_unavailable, unconfigured, AuthState, SharedAuth,
    StoreFailure,
};

/// The binding a link ceremony carries, so a registration challenge minted
/// for a sign-up or a ticket cannot finish here and this one cannot finish
/// there.
fn link_binding(principal: &PrincipalId) -> String {
    format!("link:{}", principal.0)
}

/// Why a step did not go through. `Refused` is the one answer to the caller
/// and a word in the stream; `Unavailable` is a store that could not be read.
enum Failure {
    Refused(&'static str),
    Unavailable(&'static str, StoreFailure),
    /// The person asked to remove the last authenticator they have. They are
    /// proven, so they are told.
    Last,
}

impl Failure {
    fn answer(self, auth: &AuthState, op: &str, now: Timestamp) -> Response {
        match self {
            Failure::Refused(reason) => {
                auth.record_refusal(op, reason, now);
                refused()
            }
            Failure::Unavailable(reason, failure) => {
                auth.record_refusal(op, reason, now);
                failure.into_response()
            }
            Failure::Last => {
                auth.record_refusal(op, "last-authenticator", now);
                EdgeError::new(
                    EdgeCode::LastAuthenticator,
                    "that is the only way this person can sign in; add another first",
                )
                .into_response()
            }
        }
    }
}

fn passkey_ids(list: &[AuthenticatorId]) -> Vec<Vec<u8>> {
    list.iter()
        .filter_map(|a| a.0.strip_prefix("passkey:"))
        .filter_map(|id| ordo_identity::passkey::credential_id_from_b64url(id).ok())
        .collect()
}

/// A body object read as a [`CompleteRequest`] of the given kind. The kind is
/// set here, so a caller cannot present an assertion where a registration is
/// wanted by naming it differently.
fn part(body: &Value, key: &str, kind: &str) -> Option<CompleteRequest> {
    let mut v = body.get(key)?.clone();
    v.as_object_mut()?.insert("kind".into(), json!(kind));
    parse_complete(&v)
}

/// Verify the vouching assertion in `body["vouch"]`. Returns the authenticator
/// that made it and the session's claims re-stamped with the presence it just
/// proved, which is what a [`LinkCeremony`] is built from.
fn verify_vouch(
    auth: &AuthState,
    claims: &ordo_identity::TokenClaims,
    body: &Value,
    now: Timestamp,
) -> Result<(AuthenticatorId, ordo_identity::TokenClaims), Failure> {
    let Some(passkey) = auth.passkey.clone() else {
        return Err(Failure::Refused("unknown-method"));
    };
    let Some(CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    }) = part(body, "vouch", "passkey_assertion")
    else {
        return Err(Failure::Refused("malformed-request"));
    };
    let (subject, bound_to, bound_session) = passkey
        .complete_step_up_for(
            &challenge,
            &credential_id,
            &client_data_json,
            &authenticator_data,
            &signature,
            now,
        )
        .map_err(|e| match e {
            ordo_identity::AuthError::StoreUnavailable(c) => {
                Failure::Unavailable("credential-store-unavailable", c.into())
            }
            other => Failure::Refused(auth_error_reason(&other)),
        })?;
    if bound_to != claims.principal {
        return Err(Failure::Refused("challenge-for-another-principal"));
    }
    // Every step-up is bound to the session that asked for it (G15): a
    // touch made for one session cannot vouch on another of the same
    // person.
    let session = claims.session.as_ref().map(|s| s.id.0.as_str());
    if session.is_none() || bound_session.as_deref() != session {
        return Err(Failure::Refused("challenge-for-another-session"));
    }
    let expires = Timestamp(now.0.saturating_add(auth.session_secs));
    let dir = auth.lock_directory();
    let spec = ordo_identity::refresh_presence(&dir, claims, &subject, expires)
        .map_err(|e| Failure::Refused(session_error_reason(&e)))?;
    // A passkey a recovery added cannot vouch until its quarantine ends: an
    // attacker who got in through recovery must not entrench during the
    // window (`plan-passkey-recovery.md` §7).
    let quarantined = dir
        .link_meta(&subject.authenticator)
        .map_err(|e| Failure::Unavailable("directory-unavailable", e.into()))?
        .is_some_and(|m| m.presence_not_before > now.0);
    if quarantined {
        return Err(Failure::Refused("presence-quarantined"));
    }
    let mut stamped = claims.clone();
    stamped.user_presence_at = spec.user_presence_at;
    Ok((subject.authenticator, stamped))
}

/// What a person holds: `GET /auth/authenticators` with a session's bearer.
///
/// One row per authenticator, and a notice when there is only one. The
/// notice is information and nothing is enforced on it: whether a person
/// must hold a second way in is the app developer's decision (K2).
pub async fn list(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let claims = match step_up_claims(&auth, &headers, "auth.authenticators", now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let auth2 = auth.clone();
    let listed = tokio::task::spawn_blocking(move || {
        let links = auth2
            .directory
            .lock()
            .expect("directory mutex poisoned")
            .links_of(&claims.principal)
            .map_err(StoreFailure::from)?;
        let mut rows = Vec::new();
        for (id, meta) in &links {
            let mut row = json!({"id": id.0, "kind": kind_of(id), "state": "active"});
            if let Some(r) = &meta.retired {
                row["state"] = json!("retired");
                row["contest_until"] = json!(r.until);
                row["recovery"] = json!(r.case);
            }
            if meta.presence_not_before > now.0 {
                row["presence_available_at"] = json!(meta.presence_not_before);
            }
            if meta.recovered_case.is_some() {
                row["recovered"] = json!(true);
            }
            if let (Some(passkey), Some(raw)) = (&auth2.passkey, id.0.strip_prefix("passkey:")) {
                if let Ok(raw) = ordo_identity::passkey::credential_id_from_b64url(raw) {
                    if let Ok(Some(s)) = passkey.stored(&raw) {
                        row["backup_eligible"] = json!(s.backup_eligible);
                        row["backed_up"] = json!(s.backed_up);
                        row["registered_at"] = json!(s.registered_at.0);
                        row["last_used_at"] = json!(s.last_used_at.0);
                    }
                }
            }
            rows.push(row);
        }
        let left = auth2
            .recovery()
            .and_then(|r| r.codes())
            .and_then(|c| c.remaining(&claims.principal.0).ok());
        Ok::<_, StoreFailure>((rows, left))
    })
    .await;
    match listed {
        Ok(Ok((rows, left))) => {
            let active = rows.iter().filter(|r| r["state"] == "active").count();
            let mut notices: Vec<&str> = if active == 1 {
                vec!["single_authenticator"]
            } else {
                vec![]
            };
            let mut answer = json!({"authenticators": rows});
            if let Some(left) = left {
                answer["recovery_codes_remaining"] = json!(left);
                // Loud at two or fewer, but only for a person who holds a
                // set: never having asked for one is the person's choice.
                if (1..=2).contains(&left) {
                    notices.push("recovery_codes_low");
                }
            }
            answer["notices"] = json!(notices);
            (StatusCode::OK, Json(answer)).into_response()
        }
        Ok(Err(failure)) => {
            auth.record_refusal("auth.authenticators", "directory-unavailable", now);
            failure.into_response()
        }
        Err(_) => {
            auth.record_refusal("auth.authenticators", "directory-unavailable", now);
            StoreFailure::task_failed().into_response()
        }
    }
}

fn kind_of(id: &AuthenticatorId) -> &'static str {
    match id.0.split(':').next() {
        Some("passkey") => "passkey",
        Some("password") => "password",
        _ => "other",
    }
}

/// Start adding a passkey: `POST /auth/authenticators/link/begin` with a
/// session's bearer. Answers both ceremonies the browser must run: the
/// creation options for the new passkey and the request options for the
/// vouching assertion, narrowed to the passkeys the person already holds.
///
/// A person who holds no passkey has nothing to vouch, and the answer is the
/// one refusal. A password-only person adds a first passkey through a ticket
/// (`/auth/register` with `ticket`), which is enrolment's.
pub async fn link_begin(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(passkey) = auth.passkey.clone() else {
        auth.record_refusal("auth.authenticators.link.begin", "unknown-method", now);
        return refused();
    };
    let claims = match step_up_claims(&auth, &headers, "auth.authenticators.link.begin", now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let auth2 = auth.clone();
    let p2 = passkey.clone();
    let minted = tokio::task::spawn_blocking(move || {
        let (held, display) = {
            let dir = auth2.lock_directory();
            let held = dir
                .authenticators_of(&claims.principal)
                .map_err(|e| Failure::Unavailable("directory-unavailable", e.into()))?;
            let display = dir
                .principal(&claims.principal)
                .ok()
                .flatten()
                .map(|p| p.display)
                .unwrap_or_default();
            (held, display)
        };
        let allow = passkey_ids(&held);
        if allow.is_empty() {
            return Err(Failure::Refused("no-passkey-to-vouch"));
        }
        let binding = link_binding(&claims.principal);
        let registration = p2
            .begin_registration_bound(now, &allow, Some(&binding))
            .map_err(|e| Failure::Unavailable("ceremony-store-unavailable", e.into()))?;
        let session = claims.session.as_ref().map(|s| s.id.0.clone());
        let vouch = p2
            .begin_step_up_for(now, &claims.principal, session.as_deref(), &allow)
            .map_err(|e| Failure::Unavailable("ceremony-store-unavailable", e.into()))?;
        Ok((registration, vouch, display))
    })
    .await;
    match minted {
        Ok(Ok((registration, vouch, display))) => {
            let shown = if display.is_empty() {
                passkey.relying_party().name().to_string()
            } else {
                display
            };
            let timeout = auth.passkey_timeout_ms();
            (
                StatusCode::OK,
                Json(json!({
                    "registration": creation_options_json(
                        &registration, passkey.relying_party().name(), &shown, timeout),
                    "vouch": request_options_json(&vouch, timeout),
                })),
            )
                .into_response()
        }
        Ok(Err(e)) => e.answer(&auth, "auth.authenticators.link.begin", now),
        Err(_) => {
            auth.record_refusal(
                "auth.authenticators.link.begin",
                "ceremony-task-failed",
                now,
            );
            refused()
        }
    }
}

/// Finish adding a passkey: `POST /auth/authenticators/link` with a session's
/// bearer and `{"registration": {...}, "vouch": {...}}`.
///
/// The order is the order of what cannot be undone. The vouching assertion is
/// verified and must be from an authenticator of this principal. The
/// registration is verified and must be the one this route began. Only then
/// does [`Directory::link`](ordo_identity::Directory::link) run, with a
/// [`LinkCeremony`] built from the assertion just proved, and only after the
/// link is the public key stored. A store that fails after the link takes
/// the link back.
pub async fn link(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.authenticators.link";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(passkey) = auth.passkey.clone() else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let claims = match step_up_claims(&auth, &headers, OP, now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let auth2 = auth.clone();
    let done = tokio::task::spawn_blocking(move || {
        let (voucher, stamped) = verify_vouch(&auth2, &claims, &body, now)?;
        let Some(CompleteRequest::PasskeyRegistration {
            challenge,
            client_data_json,
            attestation_object,
        }) = part(&body, "registration", "passkey_registration")
        else {
            return Err(Failure::Refused("malformed-request"));
        };
        let pending = passkey
            .verify_registration(&challenge, &client_data_json, &attestation_object, now)
            .map_err(|e| match e {
                ordo_identity::AuthError::StoreUnavailable(c) => {
                    Failure::Unavailable("credential-store-unavailable", c.into())
                }
                other => Failure::Refused(auth_error_reason(&other)),
            })?;
        if pending.binding() != Some(link_binding(&claims.principal).as_str()) {
            return Err(Failure::Refused("link-binding"));
        }
        let ceremony = LinkCeremony::from_claims(&stamped, &voucher)
            .map_err(|_| Failure::Refused("link-refused"))?;
        let added = pending.subject().authenticator.clone();
        let mut dir = auth2.lock_directory();
        let audit = dir
            .link(&claims.principal, &added, &ceremony, now)
            .map_err(|_| Failure::Refused("link-refused"))?;
        if let Err(e) = passkey.store_registration(&pending) {
            // The link was written and the key was not. A link nobody can
            // use is worse than none, so it is taken back.
            let _ = dir.unlink(&added, now);
            return Err(match e {
                ordo_identity::AuthError::StoreUnavailable(c) => {
                    Failure::Unavailable("credential-store-unavailable", c.into())
                }
                other => Failure::Refused(auth_error_reason(&other)),
            });
        }
        let count = dir
            .authenticators_of(&claims.principal)
            .map(|l| l.len())
            .unwrap_or(0);
        Ok((audit, added, count))
    })
    .await;
    match done {
        Ok(Ok((audit, added, count))) => {
            auth.record(
                Some(&audit.principal),
                OP,
                ordo_secevent::EventKind::from(&audit),
                now,
            );
            (
                StatusCode::OK,
                Json(json!({"authenticator": added.0, "authenticators": count})),
            )
                .into_response()
        }
        Ok(Err(e)) => e.answer(&auth, OP, now),
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}

/// Remove a passkey: `POST /auth/authenticators/unlink` with a session's
/// bearer and `{"authenticator", "vouch": {...}, "end_sessions"?: bool}`. The
/// challenge for the vouching assertion comes from `/auth/step-up/begin`.
///
/// Removes the directory's link and the public key, so an assertion from the
/// removed passkey finds nothing. **Every session of the person ends by
/// default**, this one included, because a passkey removed as lost may have
/// left a session behind it; `end_sessions: false` keeps them, and the choice
/// is recorded either way. The last authenticator is refused with a `409`
/// the person is told about.
pub async fn unlink(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.authenticators.unlink";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(passkey) = auth.passkey.clone() else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let Some(revocations) = auth.revocations.clone() else {
        return unconfigured();
    };
    let claims = match step_up_claims(&auth, &headers, OP, now) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let end_sessions = body
        .get("end_sessions")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // A tenant keeps an owner who signs in without its own providers
    // (`plan-tenant-providers.md` §8). The tenants' locks are held until
    // the unlink is done.
    let held = match body.get("authenticator").and_then(|a| a.as_str()) {
        Some(target) => {
            let target = AuthenticatorId(target.to_string());
            match super::tenant::unlink_guard(&auth, &claims.principal, &target, now).await {
                Ok(held) => held,
                Err(answer) => return *answer,
            }
        }
        None => Vec::new(),
    };
    let until = Timestamp(now.0.saturating_add(auth.session_max_secs));
    let auth2 = auth.clone();
    let claims2 = claims.clone();
    let done = tokio::task::spawn_blocking(move || {
        let (_voucher, _) = verify_vouch(&auth2, &claims2, &body, now)?;
        let Some(target) = body.get("authenticator").and_then(|a| a.as_str()) else {
            return Err(Failure::Refused("malformed-request"));
        };
        let Some(raw) = target
            .strip_prefix("passkey:")
            .and_then(|id| ordo_identity::passkey::credential_id_from_b64url(id).ok())
        else {
            return Err(Failure::Refused("not-a-passkey"));
        };
        let target = AuthenticatorId(target.to_string());
        let mut dir = auth2.lock_directory();
        match dir.principal_for(&target) {
            Ok(Some(owner)) if owner == claims2.principal => {}
            Ok(_) => return Err(Failure::Refused("authenticator-not-yours")),
            Err(e) => return Err(Failure::Unavailable("directory-unavailable", e.into())),
        }
        let audit = dir.unlink(&target, now).map_err(|e| match e {
            ordo_identity::DirectoryError::LastAuthenticator { .. } => Failure::Last,
            ordo_identity::DirectoryError::StoreUnavailable(c) => {
                Failure::Unavailable("directory-unavailable", c.into())
            }
            _ => Failure::Refused("unlink-refused"),
        })?;
        drop(dir);
        passkey
            .remove_credential(&raw)
            .map_err(|e| Failure::Unavailable("credential-store-unavailable", e.into()))?;
        if end_sessions {
            revocations
                .end_all(&claims2.principal, now, until)
                .map_err(|e| Failure::Unavailable("session-store-unavailable", e.into()))?;
        }
        Ok(audit)
    })
    .await;
    super::tenant::release(held).await;
    match done {
        Ok(Ok(audit)) => {
            auth.record(
                Some(&audit.principal),
                OP,
                ordo_secevent::EventKind::from(&audit),
                now,
            );
            if end_sessions {
                auth.record(
                    Some(&audit.principal),
                    OP,
                    ordo_secevent::EventKind::SessionsEnded {
                        reason: "authenticator-unlinked".to_string(),
                    },
                    now,
                );
            }
            (
                StatusCode::OK,
                Json(json!({
                    "removed": audit.authenticator.map(|a| a.0),
                    "sessions_ended": end_sessions,
                })),
            )
                .into_response()
        }
        Ok(Err(e)) => e.answer(&auth, OP, now),
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            store_unavailable()
        }
    }
}
