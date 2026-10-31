//! The HTTP shape of every route. What each does is in [`crate::flows`].
//!
//! | Route | Caller | What |
//! |---|---|---|
//! | `GET /health/live`, `/health/ready` | anyone | liveness; readiness with the database |
//! | `GET /.well-known/jwks.json` | anyone | the token keys, `max-age=600` |
//! | `GET /.well-known/ordo-audit-keys.json` | anyone | the audit checkpoint keys |
//! | `POST /join/begin`, `/join/finish` | invite holder | sign up by invite link (5.3) |
//! | `POST /sign-in/begin`, `/sign-in/finish` | anyone | passkey sign-in (5.4) |
//! | `POST /recover` | code holder | recovery code sign-in |
//! | `POST /token` | session | `{aud}`: a token (5.4, 5.5) |
//! | `POST /sessions/end` | session | sign out |
//! | `POST /touch/begin`, `/touch/finish` | browser session | a fresh touch and a token carrying it |
//! | `POST /passkeys/begin`, `/passkeys/finish` | browser session | enrol another passkey |
//! | `POST /recovery-codes` | browser session | a new set of recovery codes |
//! | `POST /me` | browser session | who is signed in, their passkeys, codes left (the account page) |
//! | `POST /passkeys/{id}/remove` | browser session | remove one passkey, never the last |
//! | `POST /cli/approve` | browser session | a CLI code for `ordo login` (5.5) |
//! | `POST /cli/redeem` | the CLI | the code and the PKCE verifier for a CLI session |
//! | `POST /machine/begin`, `/machine/complete` | machine | machine key sign-in (5.6) |
//! | `GET /invites`, `POST /invites`, `POST /invites/{id}/revoke` | bearer | invites |
//! | `GET /people`, `POST /people/{id}/end` | bearer | people (5.7) |
//! | `GET /machine-keys`, `POST /machine-keys`, `POST /machine-keys/{id}/end` | bearer | machine keys |
//! | `GET /audit?before=&limit=` | bearer | the audit log, newest first, and whether the chain checks out now (plan 7) |
//!
//! Bodies are JSON objects; binary fields are base64url, as the console's.
//!
//! The service's own page (sign-up, sign-in, CLI approval, account) is
//! served beside these by [`crate::page`].

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ordo_identity::webauthn::b64url_decode;
use ordo_identity::{ChallengeId, CompleteRequest};
use serde_json::{json, Value};

use crate::cli::Redeemed;
use crate::error::IdentityError;
use crate::flows::{Caller, Registration};
use crate::server::{Service, JWKS_MAX_AGE_SECS};
use crate::sessions::Opened;
use crate::throttle::Source;
use crate::token::Issued;
use crate::{invites, machine, people};

/// An answer: a JSON body, and a session cookie to set or clear.
struct Reply {
    body: Value,
    cookie: Option<String>,
}

impl Reply {
    fn json(body: Value) -> Reply {
        Reply { body, cookie: None }
    }
}

fn respond(r: Result<Reply, IdentityError>) -> Response {
    match r {
        Ok(reply) => {
            let mut res = Json(reply.body).into_response();
            if let Some(c) = reply.cookie.and_then(|c| HeaderValue::from_str(&c).ok()) {
                res.headers_mut().insert(SET_COOKIE, c);
            }
            res
        }
        Err(e) => e.into_response(),
    }
}

fn parse(body: &Bytes) -> Result<Value, IdentityError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(IdentityError::BadRequest(
            "the body is not a JSON object".into(),
        )),
    }
}

fn text<'a>(v: &'a Value, name: &str) -> Result<&'a str, IdentityError> {
    v.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| IdentityError::BadRequest(format!("`{name}` is required, as a string")))
}

fn bytes(v: &Value, name: &str) -> Result<Vec<u8>, IdentityError> {
    b64url_decode(text(v, name)?)
        .map_err(|_| IdentityError::BadRequest(format!("`{name}` is not base64url")))
}

fn registration(v: &Value) -> Result<Registration, IdentityError> {
    Ok(Registration {
        challenge: text(v, "challenge")?.to_string(),
        client_data_json: bytes(v, "clientDataJSON")?,
        attestation_object: bytes(v, "attestationObject")?,
    })
}

fn assertion(v: &Value) -> Result<CompleteRequest, IdentityError> {
    Ok(CompleteRequest::PasskeyAssertion {
        challenge: ChallengeId(text(v, "challenge")?.to_string()),
        credential_id: bytes(v, "credentialId")?,
        client_data_json: bytes(v, "clientDataJSON")?,
        authenticator_data: bytes(v, "authenticatorData")?,
        signature: bytes(v, "signature")?,
    })
}

fn token_json(issued: &Issued) -> Value {
    json!({
        "token": issued.token,
        "exp": issued.claims.exp,
        "sid": issued.claims.sid,
    })
}

fn signed_in(s: &Service, person: &str, opened: &Opened, mut body: Value) -> Reply {
    body["person"] = json!(person);
    body["sessionEndsAt"] = json!(opened.ends_at);
    Reply {
        body,
        cookie: Some(s.session_cookie(&opened.secret)),
    }
}

async fn caller(s: &Service, headers: &HeaderMap) -> Result<Caller, IdentityError> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(IdentityError::NotSignedIn)?;
    s.caller(token.trim()).await
}

pub(crate) fn routes() -> Router<Service> {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/.well-known/jwks.json", get(jwks))
        .route("/.well-known/ordo-audit-keys.json", get(audit_keys))
        .route("/join/begin", post(join_begin))
        .route("/join/finish", post(join_finish))
        .route("/sign-in/begin", post(sign_in_begin))
        .route("/sign-in/finish", post(sign_in_finish))
        .route("/recover", post(recover))
        .route("/token", post(token))
        .route("/sessions/end", post(sign_out))
        .route("/touch/begin", post(touch_begin))
        .route("/touch/finish", post(touch_finish))
        .route("/passkeys/begin", post(passkeys_begin))
        .route("/passkeys/finish", post(passkeys_finish))
        .route("/recovery-codes", post(recovery_codes))
        .route("/me", post(me))
        .route("/passkeys/{id}/remove", post(passkey_remove))
        .route("/cli/approve", post(cli_approve))
        .route("/cli/redeem", post(cli_redeem))
        .route("/machine/begin", post(machine_begin))
        .route("/machine/complete", post(machine_complete))
        .route("/invites", get(invites_list).post(invites_create))
        .route("/invites/{id}/revoke", post(invite_revoke))
        .route("/people", get(people_list))
        .route("/people/{id}/end", post(person_end))
        .route(
            "/machine-keys",
            get(machine_keys_list).post(machine_keys_create),
        )
        .route("/machine-keys/{id}/end", post(machine_key_end))
        .route("/audit", get(audit_page))
}

async fn live() -> Json<Value> {
    Json(json!({ "status": "live" }))
}

async fn ready(State(s): State<Service>) -> Json<Value> {
    let db = tokio::time::timeout(Duration::from_secs(1), s.store().pool().rows("SELECT 1")).await;
    let database = match db {
        Ok(Ok(_)) => "ok",
        _ => "unreachable",
    };
    Json(json!({ "status": "ready", "identityDatabase": database }))
}

async fn jwks(State(s): State<Service>) -> Response {
    let mut res = Json(s.keys().jwks()).into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("max-age={JWKS_MAX_AGE_SECS}")) {
        res.headers_mut().insert(CACHE_CONTROL, v);
    }
    res
}

async fn audit_keys(State(s): State<Service>) -> Response {
    let mut res = Json(s.keys().audit_keys()).into_response();
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("max-age=600"));
    res
}

// ───────────────────────── sign-up and sign-in ─────────────────────────

async fn join_begin(
    State(s): State<Service>,
    from: Source,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    respond(
        async {
            s.origin_ok(&headers)?;
            let b = parse(&body)?;
            Ok(Reply::json(s.join_begin(text(&b, "secret")?, &from).await?))
        }
        .await,
    )
}

async fn join_finish(
    State(s): State<Service>,
    from: Source,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    respond(
        async {
            s.origin_ok(&headers)?;
            let b = parse(&body)?;
            let joined = s
                .join_finish(
                    text(&b, "secret")?,
                    text(&b, "displayName")?,
                    registration(&b)?,
                    &from,
                )
                .await?;
            Ok(signed_in(
                &s,
                &joined.person,
                &joined.session,
                json!({ "recoveryCodes": joined.recovery_codes }),
            ))
        }
        .await,
    )
}

async fn sign_in_begin(State(s): State<Service>, headers: HeaderMap) -> Response {
    respond(
        async {
            s.origin_ok(&headers)?;
            Ok(Reply::json(s.sign_in_begin().await?))
        }
        .await,
    )
}

async fn sign_in_finish(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            s.origin_ok(&headers)?;
            let b = parse(&body)?;
            let client = match b.get("client").and_then(Value::as_str) {
                None | Some("console") => "console",
                Some(_) => {
                    return Err(IdentityError::BadRequest(
                        "`client` is `console` or absent".into(),
                    ))
                }
            };
            let (person, opened) = s.sign_in_finish(assertion(&b)?, client).await?;
            Ok(signed_in(&s, &person, &opened, json!({})))
        }
        .await,
    )
}

async fn recover(
    State(s): State<Service>,
    from: Source,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    respond(
        async {
            s.origin_ok(&headers)?;
            let b = parse(&body)?;
            let (person, opened) = s.recover(text(&b, "code")?, &from).await?;
            Ok(signed_in(
                &s,
                &person,
                &opened,
                json!({ "mustEnrol": true }),
            ))
        }
        .await,
    )
}

// ─────────────────────────────── sessions ───────────────────────────────

async fn token(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let secret = s.secret(&headers, &b)?;
            let issued = s.token(&secret, text(&b, "aud")?).await?;
            Ok(Reply::json(token_json(&issued)))
        }
        .await,
    )
}

async fn sign_out(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    let run = async {
        let b = parse(&body)?;
        let secret = s.secret(&headers, &b)?;
        s.sign_out(&secret).await
    };
    match run.await {
        Ok(()) | Err(IdentityError::NotSignedIn) => respond(Ok(Reply {
            body: json!({ "ended": true }),
            cookie: Some(s.clear_cookie()),
        })),
        Err(e) => e.into_response(),
    }
}

/// The live browser session a call carries.
async fn session(
    s: &Service,
    headers: &HeaderMap,
    b: &Value,
) -> Result<crate::sessions::Live, IdentityError> {
    let secret = s.secret(headers, b)?;
    s.browser(&secret).await
}

async fn touch_begin(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let live = session(&s, &headers, &b).await?;
            Ok(Reply::json(s.touch_begin(&live).await?))
        }
        .await,
    )
}

async fn touch_finish(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let live = session(&s, &headers, &b).await?;
            let issued = s.touch_finish(&live, assertion(&b)?).await?;
            let mut out = token_json(&issued);
            out["touchedAt"] = json!(issued.claims.touched_at);
            Ok(Reply::json(out))
        }
        .await,
    )
}

async fn passkeys_begin(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let live = session(&s, &headers, &b).await?;
            Ok(Reply::json(s.passkey_begin(&live).await?))
        }
        .await,
    )
}

async fn passkeys_finish(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let live = session(&s, &headers, &b).await?;
            let codes = s.passkey_finish(&live, registration(&b)?).await?;
            Ok(Reply::json(match codes {
                Some(c) => json!({ "added": true, "recoveryCodes": c }),
                None => json!({ "added": true }),
            }))
        }
        .await,
    )
}

async fn recovery_codes(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let live = session(&s, &headers, &b).await?;
            Ok(Reply::json(
                json!({ "recoveryCodes": s.renew_recovery(&live).await? }),
            ))
        }
        .await,
    )
}

async fn me(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let live = session(&s, &headers, &b).await?;
            let a = s.account(&live).await?;
            let keys: Vec<Value> = a
                .passkeys
                .iter()
                .map(|k| {
                    json!({
                        "id": k.id,
                        "userHandle": k.user_handle,
                        "registeredAt": k.registered_at,
                        "lastUsedAt": k.last_used_at,
                        "backedUp": k.backed_up,
                    })
                })
                .collect();
            let cfg = s.config();
            Ok(Reply::json(json!({
                "person": a.person.id,
                "displayName": a.person.display_name,
                "role": a.person.role,
                "passkeys": keys,
                "recoveryCodesLeft": a.recovery_codes_left,
                "mustEnrol": a.must_enrol,
                "touchFresh": a.touch_fresh,
                "sessionEndsAt": a.session_ends_at,
                "rpId": cfg.rp_id,
                "sessionIdleHours": cfg.session_idle_hours,
                "sessionMaxHours": cfg.session_max_hours,
            })))
        }
        .await,
    )
}

async fn passkey_remove(
    State(s): State<Service>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let live = session(&s, &headers, &b).await?;
            s.remove_passkey(&live, &id).await?;
            Ok(Reply::json(json!({ "removed": id })))
        }
        .await,
    )
}

// ───────────────────────────────── CLI ─────────────────────────────────

async fn cli_approve(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let live = session(&s, &headers, &b).await?;
            let code = s.cli_approve(&live, text(&b, "codeChallenge")?).await?;
            Ok(Reply::json(json!({ "code": code })))
        }
        .await,
    )
}

async fn cli_redeem(
    State(s): State<Service>,
    from: Source,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    respond(
        async {
            s.origin_ok(&headers)?;
            let b = parse(&body)?;
            let existing = b.get("session").and_then(Value::as_str);
            Ok(Reply::json(
                match s
                    .cli_redeem(text(&b, "code")?, text(&b, "verifier")?, existing, &from)
                    .await?
                {
                    Redeemed::Opened { person, session } => json!({
                        "person": person,
                        "session": session.secret,
                        "sid": session.id,
                        "sessionEndsAt": session.ends_at,
                    }),
                    Redeemed::Touched { person, session_id } => json!({
                        "person": person,
                        "sid": session_id,
                        "touched": true,
                    }),
                },
            ))
        }
        .await,
    )
}

// ─────────────────────────────── machines ───────────────────────────────

async fn machine_begin(State(s): State<Service>, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            Ok(Reply::json(
                json!({ "challenge": s.machine_begin(text(&b, "key")?) }),
            ))
        }
        .await,
    )
}

async fn machine_complete(State(s): State<Service>, from: Source, body: Bytes) -> Response {
    respond(
        async {
            let b = parse(&body)?;
            let issued = s
                .machine_complete(
                    text(&b, "key")?,
                    text(&b, "challenge")?,
                    text(&b, "signature")?,
                    &from,
                )
                .await?;
            Ok(Reply::json(token_json(&issued)))
        }
        .await,
    )
}

// ────────────────────────────── admin acts ──────────────────────────────

async fn invites_list(State(s): State<Service>, headers: HeaderMap) -> Response {
    respond(
        async {
            caller(&s, &headers).await?;
            let list = invites::list(s.store().pool(), s.handle(), s.now()).await?;
            Ok(Reply::json(json!({ "invites": list })))
        }
        .await,
    )
}

async fn invites_create(State(s): State<Service>, headers: HeaderMap, body: Bytes) -> Response {
    respond(
        async {
            let c = caller(&s, &headers).await?;
            c.may_write(s.now())?;
            let b = parse(&body)?;
            if let Some(role) = b.get("role") {
                if role.as_str() != Some(ordo_identity::deployment_token::ROLE_ADMIN) {
                    return Err(IdentityError::BadRequest(
                        "`role` is `admin`, the only role for now".into(),
                    ));
                }
            }
            let label = b.get("label").and_then(Value::as_str).unwrap_or("");
            let m = s.invite(c.actor(), label).await?;
            Ok(Reply::json(json!({
                "id": m.id,
                "link": m.link,
                "expiresAt": m.expires_at,
            })))
        }
        .await,
    )
}

async fn invite_revoke(
    State(s): State<Service>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            let c = caller(&s, &headers).await?;
            c.may_write(s.now())?;
            s.revoke_invite(c.actor(), &id).await?;
            Ok(Reply::json(json!({ "revoked": id })))
        }
        .await,
    )
}

async fn people_list(State(s): State<Service>, headers: HeaderMap) -> Response {
    respond(
        async {
            caller(&s, &headers).await?;
            let list: Vec<Value> = people::list(s.store().pool(), s.handle())
                .await?
                .into_iter()
                .map(|p| {
                    json!({
                        "id": p.id,
                        "displayName": p.display_name,
                        "role": p.role,
                        "createdAt": p.created_at,
                        "endedAt": p.ended_at,
                    })
                })
                .collect();
            Ok(Reply::json(json!({ "people": list })))
        }
        .await,
    )
}

async fn person_end(
    State(s): State<Service>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            let c = caller(&s, &headers).await?;
            c.may_write(s.now())?;
            let e = s.end_person(c.actor(), &id).await?;
            Ok(Reply::json(json!({
                "ended": id,
                "sessions": e.sessions,
                "recoveryCodes": e.recovery_codes,
                "invites": e.invites,
            })))
        }
        .await,
    )
}

async fn machine_keys_list(State(s): State<Service>, headers: HeaderMap) -> Response {
    respond(
        async {
            caller(&s, &headers).await?;
            let list = machine::list(s.store().pool(), s.handle()).await?;
            Ok(Reply::json(json!({ "machineKeys": list })))
        }
        .await,
    )
}

async fn machine_keys_create(
    State(s): State<Service>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    respond(
        async {
            let c = caller(&s, &headers).await?;
            c.may_write(s.now())?;
            let b = parse(&body)?;
            let public = match b.get("publicKey").and_then(Value::as_str) {
                None => None,
                Some(p) => Some(
                    b64url_decode(p)
                        .ok()
                        .and_then(|k| <[u8; 32]>::try_from(k).ok())
                        .ok_or_else(|| {
                            IdentityError::BadRequest(
                                "`publicKey` is 32 bytes of Ed25519 public key, base64url".into(),
                            )
                        })?,
                ),
            };
            let made = s
                .add_machine_key(c.actor(), text(&b, "name")?, public)
                .await?;
            Ok(Reply::json(json!({
                "id": made.id,
                "name": made.name,
                "secret": made.secret,
            })))
        }
        .await,
    )
}

async fn machine_key_end(
    State(s): State<Service>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            let c = caller(&s, &headers).await?;
            c.may_write(s.now())?;
            let sessions = s.end_machine_key(c.actor(), &id).await?;
            Ok(Reply::json(json!({ "ended": id, "sessions": sessions })))
        }
        .await,
    )
}

/// One page of the audit log, newest first, and a check of the whole
/// chain and its checkpoints against the audit keys, run now. A break is
/// an answer, not a failure: the console shows it as a red line.
async fn audit_page(
    State(s): State<Service>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    respond(
        async {
            caller(&s, &headers).await?;
            let number = |k: &str| -> Result<Option<u64>, IdentityError> {
                q.get(k)
                    .map(|v| {
                        v.parse::<u64>().map_err(|_| {
                            IdentityError::BadRequest(format!("`{k}` is a whole number"))
                        })
                    })
                    .transpose()
            };
            let pool = s.store().pool();
            let rows =
                crate::audit::page(pool, s.handle(), number("before")?, number("limit")?.unwrap_or(50))
                    .await?;
            let verifiers = crate::audit::verifiers(pool, s.handle()).await?;
            let verified = match crate::audit::verify(pool, s.handle(), &verifiers).await {
                Ok(c) => json!({ "ok": true, "rows": c.rows, "checkpoints": c.checkpoints, "at": s.now() }),
                Err(IdentityError::AuditBroken(b)) => {
                    json!({ "ok": false, "problem": b.to_string(), "at": s.now() })
                }
                Err(e) => return Err(e),
            };
            let entries: Vec<Value> = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "seq": r.seq,
                        "at": r.at,
                        "actor": r.actor,
                        "action": r.action,
                        "subject": r.subject,
                        "outcome": r.outcome,
                        "client": r.client,
                        "detail": r.detail,
                    })
                })
                .collect();
            Ok(Reply::json(json!({ "entries": entries, "verified": verified })))
        }
        .await,
    )
}
