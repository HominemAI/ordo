//! The half-signed-in state and the route that ends it
//! (`plan-second-factor.md` §5).
//!
//! `/auth/complete` runs the first ceremony as it always did. When a second
//! step is owed it does not mint a token: [`gate`] files a `second-step`
//! ceremony row and answers a handle and a binder. `POST /auth/second-step`
//! takes the row, checks one method's answer, and only then mints.
//!
//! The row is durable, so a second step begun on one replica completes on
//! another, and it is one-shot: [`CeremonyStore::take`] is a delete that
//! returns, so two racers cannot both finish it. The binder travels back with
//! the answer. A handle seen in a log is worthless without it, and a wrong
//! binder does not count against the person's attempts, so a handle alone
//! cannot be used to run their attempts out.
//!
//! Every failure is the one answer. The stream has the reason.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_identity::ceremony::CeremonyRow;
use ordo_identity::second_step::{Owes, PersonMode, TenantMode};
use ordo_identity::{token, SecondStepKind, SessionClaim, SignIn, Timestamp, TokenSpec};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{arrive, refused, step_up_claims, unconfigured, AuthState, SharedAuth, StoreFailure};
use crate::second_factors::{UpstreamLevel, Verdict};

/// The ceremony row's `connection` and `phase`.
const CONNECTION: &str = "second-step";
const PHASE: &str = "pending";

fn sha_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"ordo.secondstep.v1\0");
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// 128 random bits as hex, from the operating system's generator.
fn random_hex() -> String {
    ordo_identity::directory::random_principal_suffix()
}

/// What the first step left behind, kept in the ceremony row.
struct Half {
    principal: String,
    /// The first step's way in, the `via` word (`passkey`, `oauth2:discord`),
    /// carried into the session the second step mints.
    method: String,
    presence_at: Option<u64>,
    presence_available_at: Option<u64>,
    /// A provider's two-step claim that counted at the first step
    /// (`plan-auth-federated.md` §8.2): when, and which connection. Carried
    /// into the session as the first step's presence is.
    upstream_at: Option<u64>,
    upstream_via: Option<String>,
    space: Option<String>,
    invitation: Option<String>,
    claim_code: Option<String>,
    enrol: bool,
    /// The person is proving a fresh first factor to change a factor, not
    /// signing in: the ceremony ends in a reauth handle, never a token.
    reauth: bool,
    attempts_left: u32,
    binder_hash: String,
    minted_at: u64,
    expires_at: u64,
}

impl Half {
    fn to_json(&self) -> String {
        json!({
            "principal": self.principal,
            "method": self.method,
            "presence_at": self.presence_at,
            "presence_available_at": self.presence_available_at,
            "upstream_at": self.upstream_at,
            "upstream_via": self.upstream_via,
            "space": self.space,
            "invitation": self.invitation,
            "claim_code": self.claim_code,
            "enrol": self.enrol,
            "reauth": self.reauth,
            "attempts_left": self.attempts_left,
            "binder": self.binder_hash,
            "minted_at": self.minted_at,
            "expires_at": self.expires_at,
        })
        .to_string()
    }

    fn parse(text: &str) -> Option<Half> {
        let v: Value = serde_json::from_str(text).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64());
        Some(Half {
            principal: s("principal")?,
            method: s("method")?,
            presence_at: n("presence_at"),
            presence_available_at: n("presence_available_at"),
            upstream_at: n("upstream_at"),
            upstream_via: s("upstream_via"),
            space: s("space"),
            invitation: s("invitation"),
            claim_code: s("claim_code"),
            enrol: v.get("enrol").and_then(|x| x.as_bool()).unwrap_or(false),
            reauth: v.get("reauth").and_then(|x| x.as_bool()).unwrap_or(false),
            attempts_left: n("attempts_left")? as u32,
            binder_hash: s("binder")?,
            minted_at: n("minted_at")?,
            expires_at: n("expires_at")?,
        })
    }

    fn row(&self, handle_hash: &str) -> CeremonyRow {
        CeremonyRow {
            handle: handle_hash.to_string(),
            connection: CONNECTION.to_string(),
            phase: PHASE.to_string(),
            binder: self.binder_hash.clone(),
            payload: self.to_json(),
            minted_at: Timestamp(self.minted_at),
            expires_at: Timestamp(self.expires_at),
        }
    }
}

/// The second-step gate: the point where a first factor has succeeded and a
/// session would be minted. `None` lets `/auth/complete` go on and mint.
/// `Some` is the answer to send instead, either the half-signed-in state or a
/// refusal that fails closed.
///
/// Nothing is minted and nothing is recorded as a sign-in here. The
/// `Authenticated` event is written when the second step completes.
pub(super) async fn gate(
    auth: &Arc<AuthState>,
    headers: &axum::http::HeaderMap,
    signed: &SignIn,
    body: &Value,
    now: Timestamp,
) -> Option<Response> {
    // `"reauth": true` asks for a fresh first factor to change a factor, not
    // for a session. It is answered a handle or a second step, and **never a
    // token**, so a deployment with no second step wired refuses it instead of
    // letting the request fall through to a mint.
    let reauth = body.get("reauth").and_then(|v| v.as_bool()) == Some(true);
    let Some(sf) = auth.second_factors().cloned() else {
        if reauth {
            auth.record_refusal("auth.complete", "not-wired", now);
            return Some(refused());
        }
        return None;
    };
    let who = signed.principal.0.clone();
    if reauth {
        // The proof is for the person already signed in, and for nobody else.
        let claims = match step_up_claims(auth, headers, "auth.complete", now) {
            Ok(c) => c,
            Err(r) => return Some(*r),
        };
        if claims.principal != signed.principal {
            auth.record_refusal("auth.complete", "reauth-other-principal", now);
            return Some(refused());
        }
    }
    let first = signed.method;
    // A fresh first factor that is a passkey still owes the step when factors
    // are held: changing a factor is asked more of than signing in is.
    let tenant = if reauth {
        TenantMode::Inherit
    } else {
        tenant_level(&sf, body)
    };
    // A provider's two-step claim accepted at sign-in stands in for Ordo's
    // second step at the level that accepts it (`plan-second-factor.md`
    // §5.3, federated ask 11): the app developer's `on` for every level, the
    // named tenant's `presenceFrom` for that tenant's. Only a claim the
    // provider made fresh and two-step was minted at all. Never for a
    // reauth: changing a factor asks Ordo's own.
    let via = signed
        .upstream_presence
        .as_ref()
        .map(|u| u.via().to_string());
    let upstream = match (&via, reauth) {
        (Some(v), false) => sf
            .levels()
            .upstream_level(v, body.get("space").and_then(|s| s.as_str())),
        _ => None,
    };
    let sf2 = sf.clone();
    let who2 = who.clone();
    let owes = tokio::task::spawn_blocking(move || {
        // The person's own level, read with the factors they hold: a store
        // that cannot answer is a person who does not get in.
        let person = if reauth {
            PersonMode::AlsoForPasskeys
        } else {
            sf2.person_mode(&who2)?
        };
        let owed = sf2.owes(&who2, first, tenant, person)?;
        let left = match upstream {
            None => owed.clone(),
            Some(UpstreamLevel::Deployment) => Owes::Nothing,
            Some(UpstreamLevel::Tenant) => sf2.owes(&who2, first, TenantMode::Inherit, person)?,
        };
        Ok::<_, ordo_identity::StoreFault>((owed, left))
    })
    .await;
    let owes = match owes {
        Ok(Ok((owed, left))) => {
            if owed != Owes::Nothing && left == Owes::Nothing {
                if let (Some(level), Some(v)) = (upstream, &via) {
                    super::oidc::record_second_step_satisfied(
                        auth,
                        &signed.principal,
                        v,
                        level,
                        body.get("space").and_then(|s| s.as_str()),
                        now,
                    );
                }
            }
            left
        }
        // A factor table that cannot be read is a person who does not get in:
        // signing them in without the step they may have enrolled would turn
        // an outage into a bypass.
        Ok(Err(why)) => {
            auth.record_refusal("auth.complete", "second-factor-store-unavailable", now);
            return Some(StoreFailure::from(why).into_response());
        }
        Err(_) => {
            auth.record_refusal("auth.complete", "second-factor-store-unavailable", now);
            return Some(StoreFailure::task_failed().into_response());
        }
    };
    let (methods, enrol) = match owes {
        Owes::Nothing if reauth => return Some(issue_reauth(auth, &who, false, now).await),
        Owes::Nothing => return None,
        Owes::Step(m) => (m, false),
        // Somebody a level makes enrol is exactly who needs a proof to do it.
        Owes::Enrol if reauth => return Some(issue_reauth(auth, &who, false, now).await),
        Owes::Enrol => (Vec::new(), true),
    };
    let Some(ceremonies) = auth.ceremonies.clone() else {
        auth.record_refusal("auth.complete", "second-step-not-wired", now);
        return Some(unconfigured());
    };
    let handle = format!("ss_{}", random_hex());
    let binder = format!("{}{}", random_hex(), random_hex());
    let expires_at = now.0.saturating_add(sf.config().ceremony_secs);
    let text = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let half = Half {
        principal: who,
        method: signed.authenticator.via(first),
        presence_at: signed.presence.map(|p| p.at().0),
        presence_available_at: signed.presence_available_at.map(|t| t.0),
        upstream_at: signed.upstream_presence.as_ref().map(|u| u.at().0),
        upstream_via: signed
            .upstream_presence
            .as_ref()
            .map(|u| u.via().to_string()),
        space: text("space"),
        invitation: text("invitation"),
        claim_code: text("claim_code"),
        enrol,
        reauth,
        attempts_left: sf.config().attempts_per_ceremony,
        binder_hash: sha_hex(&binder),
        minted_at: now.0,
        expires_at,
    };
    let row = half.row(&sha_hex(&handle));
    let put = tokio::task::spawn_blocking(move || ceremonies.put(&row)).await;
    if let Err(failure) = StoreFailure::joined(put) {
        auth.record_refusal("auth.complete", "ceremony-store-unavailable", now);
        return Some(failure.into_response());
    }
    let mut answer = json!({
        "second_step": handle,
        "binder": binder,
        "expires_at": expires_at,
    });
    if enrol {
        answer["enrol"] = json!(true);
    } else {
        answer["methods"] = json!(methods.iter().map(|m| m.as_str()).collect::<Vec<_>>());
    }
    Some((StatusCode::OK, Json(answer)).into_response())
}

const REAUTH_CONNECTION: &str = "reauth";
const REAUTH_PHASE: &str = "ready";

/// A proof that the person just presented a first factor (and, when they hold
/// a second factor, a second step): one act's worth, good for
/// `ORDO_AUTH_REAUTH_SECS`. One-shot, so each change needs its own proof.
async fn issue_reauth(
    auth: &Arc<AuthState>,
    principal: &str,
    second_step_done: bool,
    now: Timestamp,
) -> Response {
    let (Some(ceremonies), Some(sf)) = (auth.ceremonies.clone(), auth.second_factors().cloned())
    else {
        auth.record_refusal("auth.complete", "second-step-not-wired", now);
        return unconfigured();
    };
    let handle = format!("ra_{}{}", random_hex(), random_hex());
    let expires_at = now.0.saturating_add(sf.config().reauth_secs);
    let row = CeremonyRow {
        handle: sha_hex(&handle),
        connection: REAUTH_CONNECTION.to_string(),
        phase: REAUTH_PHASE.to_string(),
        binder: String::new(),
        payload: json!({"principal": principal, "second_step_done": second_step_done,
                        "expires_at": expires_at})
        .to_string(),
        minted_at: now,
        expires_at: Timestamp(expires_at),
    };
    let put = tokio::task::spawn_blocking(move || ceremonies.put(&row)).await;
    if let Err(failure) = StoreFailure::joined(put) {
        auth.record_refusal("auth.complete", "ceremony-store-unavailable", now);
        return failure.into_response();
    }
    (
        StatusCode::OK,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(json!({"reauth": handle, "expires_at": expires_at})),
    )
        .into_response()
}

/// What a reauth handle proved.
pub(super) struct Reauth {
    /// A second step was completed as well as the first factor.
    pub second_step_done: bool,
}

/// The header a client sends the handle in.
pub(super) const REAUTH_HEADER: &str = "x-reauth";

/// Spend the reauth handle a request carries, for `principal`. One-shot: the
/// row is taken. A handle that is another person's is put back untouched and
/// refused. Synchronous, for a blocking task.
pub(super) fn redeem_reauth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
    principal: &str,
    now: Timestamp,
) -> Result<Reauth, &'static str> {
    let Some(handle) = headers.get(REAUTH_HEADER).and_then(|v| v.to_str().ok()) else {
        return Err("reauth-missing");
    };
    let Some(ceremonies) = auth.ceremonies.as_ref() else {
        return Err("second-step-not-wired");
    };
    let key = sha_hex(handle);
    let text = match ceremonies.take(&key, REAUTH_CONNECTION, REAUTH_PHASE, now) {
        Ok(Some(t)) => t,
        Ok(None) => return Err("reauth-unknown"),
        Err(_) => return Err("ceremony-store-unavailable"),
    };
    let v: Value = serde_json::from_str(&text).map_err(|_| "reauth-malformed")?;
    if v.get("principal").and_then(|p| p.as_str()) != Some(principal) {
        // Not theirs, so not theirs to spend either.
        let _ = ceremonies.put(&CeremonyRow {
            handle: key,
            connection: REAUTH_CONNECTION.to_string(),
            phase: REAUTH_PHASE.to_string(),
            binder: String::new(),
            payload: text,
            minted_at: now,
            expires_at: Timestamp(v.get("expires_at").and_then(|e| e.as_u64()).unwrap_or(0)),
        });
        return Err("reauth-not-theirs");
    }
    Ok(Reauth {
        second_step_done: v
            .get("second_step_done")
            .and_then(|b| b.as_bool())
            .unwrap_or(false),
    })
}

/// The kind a client names. The plan's own spellings for the answers
/// (`backup_code`, `passkey_assertion`) are accepted beside the words the
/// token and the record use.
fn parse_kind(word: &str) -> Option<SecondStepKind> {
    match word {
        "backup_code" => Some(SecondStepKind::BackupCode),
        "passkey_assertion" => Some(SecondStepKind::SecurityKey),
        other => SecondStepKind::parse(other),
    }
}

/// The level the tenant a sign-in names has chosen. A sign-in that names no
/// tenant meets none here, and the door asks again when the session reaches
/// one (`SecondStepLevels::refuses`).
fn tenant_level(sf: &crate::second_factors::SecondFactors, body: &Value) -> TenantMode {
    body.get("space")
        .and_then(|s| s.as_str())
        .map(|space| sf.levels().tenant(space))
        .unwrap_or_default()
}

/// What taking the row came to.
enum Taken {
    Half(Box<Half>, String),
    Refused(&'static str),
    Unavailable(&'static str, StoreFailure),
}

/// Take the half-signed-in row for `handle`, check its binder, and put it
/// back untouched when the binder is wrong. One-shot: of two callers racing
/// on one handle, one gets the row.
fn take_half(auth: &AuthState, handle: &str, binder: &str, now: Timestamp) -> Taken {
    let Some(ceremonies) = auth.ceremonies.as_ref() else {
        return Taken::Unavailable(
            "second-step-not-wired",
            StoreFailure::from(ordo_identity::StoreFault::unavailable(
                "no ceremony store is wired",
            )),
        );
    };
    let key = sha_hex(handle);
    let text = match ceremonies.take(&key, CONNECTION, PHASE, now) {
        Ok(Some(t)) => t,
        Ok(None) => return Taken::Refused("second-step-unknown"),
        Err(e) => return Taken::Unavailable("ceremony-store-unavailable", e.into()),
    };
    let Some(half) = Half::parse(&text) else {
        return Taken::Refused("second-step-malformed");
    };
    if !same(&sha_hex(binder), &half.binder_hash) {
        // Not counted: a wrong binder is somebody who saw the handle, and the
        // person's attempts are not theirs to spend.
        let _ = ceremonies.put(&half.row(&key));
        return Taken::Refused("second-step-binder");
    }
    Taken::Half(Box::new(half), key)
}

/// The person a half-signed-in state is for, **when it is one that owes an
/// enrolment** (`"enrol": true`), and checks its binder. The row is left as it
/// was: enrolling a factor is not answering the step, and the person then
/// answers it with the factor they enrolled.
pub(super) fn peek_enrolment(
    auth: &AuthState,
    handle: &str,
    binder: &str,
    now: Timestamp,
) -> Result<String, &'static str> {
    let (half, key) = match take_half(auth, handle, binder, now) {
        Taken::Half(h, k) => (h, k),
        Taken::Refused(r) => return Err(r),
        Taken::Unavailable(r, _) => return Err(r),
    };
    let ceremonies = auth.ceremonies.as_ref().expect("taken from it");
    let _ = ceremonies.put(&half.row(&key));
    if !half.enrol {
        return Err("second-step-not-an-enrolment");
    }
    Ok(half.principal)
}

/// `POST /auth/second-step/begin` `{handle, binder, kind}`: the challenge a
/// method binds its answer to, for a method that has one (a security key).
/// Nothing is consumed.
pub async fn second_step_begin(
    State(auth): State<Option<SharedAuth>>,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.second-step.begin";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    let Some(sf) = auth.second_factors().cloned() else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    let text = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(handle), Some(binder), Some(kind)) = (
        text("handle"),
        text("binder"),
        text("kind").and_then(|k| parse_kind(&k)),
    ) else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let Some(method) = sf.method(kind).cloned() else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let auth2 = auth.clone();
    let done = tokio::task::spawn_blocking(move || {
        match take_half(&auth2, &handle, &binder, now) {
            Taken::Half(half, key) => {
                // Put back before the method is asked, so a method that fails
                // leaves the ceremony as it was.
                let ceremonies = auth2.ceremonies.as_ref().expect("taken from it");
                let _ = ceremonies.put(&half.row(&key));
                match method.begin(&half.principal, now.0) {
                    Ok(Some(options)) => Ok(options),
                    Ok(None) => Err(("no-challenge", None)),
                    Err(why) => Err(("second-factor-store-unavailable", Some(why.into()))),
                }
            }
            Taken::Refused(r) => Err((r, None)),
            Taken::Unavailable(r, failure) => Err((r, Some(failure))),
        }
    })
    .await;
    match done {
        Ok(Ok(options)) => (StatusCode::OK, Json(options)).into_response(),
        Ok(Err((reason, failure))) => {
            auth.record_refusal(OP, reason, now);
            match failure {
                Some(failure) => failure.into_response(),
                None => refused(),
            }
        }
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            refused()
        }
    }
}

/// What checking an answer came to.
enum Checked {
    Accepted(Box<Half>),
    Refused(&'static str),
    /// A wrong answer. `ended` when it was the last attempt; `locked` when it
    /// locked a factor.
    Wrong {
        principal: String,
        locked: bool,
        ended: bool,
    },
    Unavailable(&'static str, StoreFailure),
}

/// `POST /auth/second-step` `{handle, binder, kind, ...}`: the second step.
///
/// The row is taken, the method asked, and only an accepted answer mints. A
/// wrong answer puts the row back with one fewer attempt, and the last one
/// ends the ceremony. The token carries `second_step: <kind>` in its session
/// claim and the presence the first factor had, which is none for a password.
pub async fn second_step(
    State(auth): State<Option<SharedAuth>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.second-step";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now_secs = (auth.now)();
    let now = Timestamp(now_secs);
    let Some(sf) = auth.second_factors().cloned() else {
        auth.record_refusal(OP, "not-wired", now);
        return refused();
    };
    let text = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(handle), Some(binder), Some(kind)) = (
        text("handle"),
        text("binder"),
        text("kind").and_then(|k| parse_kind(&k)),
    ) else {
        auth.record_refusal(OP, "malformed-request", now);
        return refused();
    };
    let Some(method) = sf.method(kind).cloned() else {
        auth.record_refusal(OP, "unknown-method", now);
        return refused();
    };
    let auth2 = auth.clone();
    let body2 = body.clone();
    let checked = tokio::task::spawn_blocking(move || {
        let (mut half, key) = match take_half(&auth2, &handle, &binder, now) {
            Taken::Half(h, k) => (h, k),
            Taken::Refused(r) => return Checked::Refused(r),
            Taken::Unavailable(r, failure) => return Checked::Unavailable(r, failure),
        };
        let ceremonies = auth2.ceremonies.as_ref().expect("taken from it");
        match method.verify(&half.principal, &body2, now.0) {
            Ok(Verdict::Accepted) => Checked::Accepted(half),
            Ok(Verdict::Rejected { locked }) => {
                half.attempts_left = half.attempts_left.saturating_sub(1);
                let ended = half.attempts_left == 0;
                if !ended {
                    let _ = ceremonies.put(&half.row(&key));
                }
                Checked::Wrong {
                    principal: half.principal.clone(),
                    locked,
                    ended,
                }
            }
            Err(why) => {
                let _ = ceremonies.put(&half.row(&key));
                Checked::Unavailable("second-factor-store-unavailable", why.into())
            }
        }
    })
    .await;
    let half = match checked {
        Ok(Checked::Accepted(h)) => h,
        Ok(Checked::Refused(reason)) => {
            auth.record_refusal(OP, reason, now);
            return refused();
        }
        Ok(Checked::Wrong {
            principal,
            locked,
            ended,
        }) => {
            if locked {
                auth.record(
                    Some(&ordo_identity::PrincipalId(principal.clone())),
                    OP,
                    ordo_secevent::EventKind::SecondFactor {
                        act: "disabled".into(),
                        detail: format!("principal={principal} kind={}", kind.as_str()),
                    },
                    now,
                );
            }
            let reason = if ended {
                "second-step-attempts-exhausted"
            } else {
                "second-step-wrong"
            };
            auth.record_refusal(OP, reason, now);
            return refused();
        }
        Ok(Checked::Unavailable(reason, failure)) => {
            auth.record_refusal(OP, reason, now);
            return failure.into_response();
        }
        Err(_) => {
            auth.record_refusal(OP, "ceremony-task-failed", now);
            return refused();
        }
    };
    if half.reauth {
        return issue_reauth(&auth, &half.principal, true, now).await;
    }
    let principal = ordo_identity::PrincipalId(half.principal.clone());
    auth.record(
        Some(&principal),
        OP,
        ordo_secevent::EventKind::Authenticated,
        now,
    );
    // The session names the second factor that proved it. The presence is the
    // first factor's, as it was minted, and a second factor adds none.
    let upstream = match (half.upstream_at, half.upstream_via.as_deref()) {
        (Some(at), Some(via)) => ordo_identity::UpstreamPresence::new(Timestamp(at), via),
        _ => None,
    };
    let session = SessionClaim::starting_at(now)
        .with_second_step(kind)
        .with_via(half.method.clone())
        .with_upstream_presence(upstream);
    let spec = TokenSpec {
        principal: principal.clone(),
        expires: Timestamp(now_secs + auth.session_secs),
        client: None,
        user_presence_at: half.presence_at.map(Timestamp),
        capability: None,
        delegation: None,
        session: Some(session),
    };
    let bearer = token::mint(&auth.token_key, &spec);
    let mut answer = json!({
        "token": bearer,
        "expires_at": now_secs + auth.session_secs,
        "session_expires_at": now_secs + auth.session_max_secs,
        "second_step": kind.as_str(),
    });
    if let Some(from) = half.presence_available_at {
        answer["presence_available_at"] = json!(from);
    }
    if let Some(recovery) = auth.recovery().cloned() {
        let who = half.principal.clone();
        let left = tokio::task::spawn_blocking(move || {
            recovery.codes().and_then(|c| c.remaining(&who).ok())
        })
        .await
        .ok()
        .flatten();
        if let Some(left) = left {
            answer["recovery_codes_remaining"] = json!(left);
            if kind == SecondStepKind::BackupCode {
                // A code spent as a second step is recorded as that use, at the
                // deployment, and the person is told loudly when few are left.
                auth.record(
                    Some(&principal),
                    OP,
                    ordo_secevent::EventKind::Recovery {
                        act: "code-used".into(),
                        detail: format!(
                            "principal={} used_as=second-step remaining={left}",
                            half.principal
                        ),
                    },
                    now,
                );
                answer["recovery_codes_low"] = json!(left <= 2);
            }
        }
    }
    if let Some(space) = half.space.as_deref() {
        let arrival = json!({"invitation": half.invitation, "claim_code": half.claim_code});
        // A first step through an outside provider still arrives by its
        // connection. The address's domain is not carried across the second
        // step, so a provider rule that names a domain does not match here.
        let provider = half.method.contains(':').then_some(crate::admission::Via {
            provider: half.method.as_str(),
            domain: None,
        });
        let outcome = arrive(&auth, &half.principal, space, &arrival, provider, now).await;
        answer["admission"] = super::admission_answer(space, outcome, &arrival);
    }
    let cookie = auth
        .finish_sign_in(
            &headers,
            Some(&body),
            &spec,
            half.space.clone(),
            &mut answer,
            now,
        )
        .await;
    super::with_cookie((StatusCode::OK, Json(answer)).into_response(), cookie)
}

/// The routes this module adds, for a router whose state can produce the
/// authentication state. One `merge` in each router keeps this module's
/// routes out of the shared route lists.
pub fn routes<S>() -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
    Option<SharedAuth>: axum::extract::FromRef<S>,
{
    axum::Router::new()
        .route("/auth/second-step", axum::routing::post(second_step))
        .route(
            "/auth/second-step/begin",
            axum::routing::post(second_step_begin),
        )
        .merge(super::factors::routes())
}
