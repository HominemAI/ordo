//! Linking by claim (`plan-auth-federated.md` §2.5 and step 7, owner ruling
//! R12: *"Existing account logs in and claims another account."*).
//!
//! A person signed in to the account that will survive proves it again, then
//! completes the ceremony of the sign-in they are claiming, inside one window
//! (`ORDO_AUTH_LINKING_SECS`, 300 s by default). The claimed sign-in then
//! names the same account.
//!
//! - `POST /auth/claim/begin`, with the session's bearer token and a body
//!   shaped like `/auth/complete`'s: a fresh ceremony of a sign-in the
//!   account already holds. Answers `{claim, expires_at}`. Nothing is linked.
//! - `POST /auth/claim/complete`, with the same bearer token and a body
//!   shaped like `/auth/complete`'s plus `claim`: the ceremony of the sign-in
//!   being claimed. Answers `{linked, via, authenticators}`.
//! - An emailed code being claimed is begun at `/auth/begin` with `claim`
//!   in the body. An open claim lets that address be mailed when it is
//!   unknown, as an invitation does (see [`opens_mail`]).
//!
//! # Why a stolen token alone cannot link
//!
//! The window opens only on a ceremony completed **now** by a sign-in linked
//! to the token's account. A token is not a ceremony. The window is filed
//! under the SHA-256 of a secret only the opener was given, belongs to the
//! session that opened it, and is spent by the claim that lands.
//!
//! # What a claim never does
//!
//! - It reads no attribute. A matching email, verified or not, links
//!   nothing (§2.5). Two ceremonies are the only proof.
//! - It takes no passkey assertion: a passkey already names the account it
//!   was registered for. A **new** passkey is added by a claim: after the
//!   window opens, `POST /auth/claim/passkey {claim}` answers creation
//!   options bound to this account, and the registration is the claimed
//!   ceremony. Any sign-in the account holds may vouch for it (owner: *"a
//!   login method is a login method"*). From then on it is an ordinary
//!   passkey. `/auth/authenticators/link` (a passkey vouching for a passkey)
//!   is unchanged.
//! - It moves nothing between accounts. A sign-in that already names another
//!   account, live or deleted, stays where it is (D5).
//! - It mints no principal, no session and no presence. A hand-off session
//!   (`via=handoff`) claims the way every presence-less session does: by two
//!   fresh ceremonies, and with nothing more.
//!
//! # Answers
//!
//! Every refusal is the one answer, with the reason in the security stream,
//! with two exceptions a proven person may be told. Linking switched off
//! names the setting that switches it on. A claimed sign-in that already
//! names an account is `409 identifier_taken`: the person completed its
//! ceremony, so they hold it, and a live account and a deleted one get the
//! same answer.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use ordo_api::{EdgeCode, EdgeError};
use ordo_identity::federation::{handle_of, random_secret};
use ordo_identity::{
    AuthenticatedSubject, AuthenticatorId, CeremonyRow, CompleteRequest, DirectoryError,
    PrincipalId, Timestamp,
};
use serde_json::{json, Value};

use super::{
    auth_error_reason, creation_options_json, parse_complete, refused, step_up_claims,
    unconfigured, AuthState, SharedAuth, StoreFailure,
};

/// The setting that switches linking by claim on or off.
pub const SETTING: &str = "ORDO_AUTH_LINKING";
/// The setting for the window between the two ceremonies.
pub const SECS_SETTING: &str = "ORDO_AUTH_LINKING_SECS";
/// The window when [`SECS_SETTING`] is unset (`plan-auth-federated.md` §2.5).
pub const DEFAULT_SECS: u64 = 300;
/// The shortest and longest window the setting takes.
pub const MIN_SECS: u64 = 60;
pub const MAX_SECS: u64 = 900;

/// The ceremony table's `connection` for an open claim. A `$` cannot appear
/// in a connection name, so no provider's row can be mistaken for one.
const CONNECTION: &str = "$claim";
const PHASE: &str = "claim";

const BEGIN: &str = "auth.claim.begin";
const COMPLETE: &str = "auth.claim";

/// The deployment's linking setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Linking {
    /// `ORDO_AUTH_LINKING`: `on` (the default) or `off`. Off restores the
    /// rule before step 7: a second sign-in is added only with a passkey's
    /// fresh presence.
    pub on: bool,
    /// `ORDO_AUTH_LINKING_SECS`: how long after the first ceremony the
    /// second may land.
    pub window_secs: u64,
}

impl Default for Linking {
    fn default() -> Linking {
        Linking {
            on: true,
            window_secs: DEFAULT_SECS,
        }
    }
}

/// Read `ORDO_AUTH_LINKING` and `ORDO_AUTH_LINKING_SECS`. A refusal names
/// the variable and says what it takes.
pub fn from_vars(env: &dyn Fn(&str) -> Option<String>) -> Result<Linking, (&'static str, String)> {
    let set = |k: &str| env(k).filter(|v| !v.trim().is_empty());
    let on = match set(SETTING).as_deref().map(str::trim) {
        None | Some("on") => true,
        Some("off") => false,
        Some(other) => {
            return Err((
                SETTING,
                format!("`{other}` is not `on` or `off`; unset it for the default, `on`"),
            ))
        }
    };
    let window_secs = match set(SECS_SETTING) {
        None => DEFAULT_SECS,
        Some(raw) => match raw.trim().parse::<u64>() {
            Ok(n) if (MIN_SECS..=MAX_SECS).contains(&n) => n,
            _ => {
                return Err((
                    SECS_SETTING,
                    format!(
                        "`{raw}` is not {MIN_SECS} to {MAX_SECS} seconds; unset it for the \
                         default, {DEFAULT_SECS}"
                    ),
                ))
            }
        },
    };
    Ok(Linking { on, window_secs })
}

/// What an open claim stands for: the account, the session that opened it,
/// and the sign-in that proved it, at the instant it did.
struct Open {
    principal: String,
    session: String,
    first: String,
    first_at: u64,
}

impl Open {
    fn to_payload(&self) -> String {
        json!({
            "principal": self.principal,
            "session": self.session,
            "first": self.first,
            "first_at": self.first_at,
        })
        .to_string()
    }

    fn from_payload(text: &str) -> Option<Open> {
        let v: Value = serde_json::from_str(text).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        Some(Open {
            principal: s("principal")?,
            session: s("session")?,
            first: s("first")?,
            first_at: v.get("first_at")?.as_u64()?,
        })
    }
}

fn linking_off(auth: &AuthState, op: &str, now: Timestamp) -> Response {
    auth.record_refusal(op, "linking-off: set ORDO_AUTH_LINKING=on to allow it", now);
    EdgeError::new(
        EdgeCode::LinkingOff,
        "linking by claim is switched off on this deployment; set ORDO_AUTH_LINKING=on to allow it",
    )
    .with("setting", SETTING)
    .with("value", "on")
    .into_response()
}

fn identifier_taken() -> Response {
    EdgeError::new(
        EdgeCode::IdentifierTaken,
        "that sign-in already belongs to another account; nothing was linked",
    )
    .into_response()
}

/// Refuse with the one answer and a reason in the stream.
fn refuse(auth: &AuthState, op: &str, reason: &str, now: Timestamp) -> Response {
    auth.record_refusal(op, reason, now);
    refused()
}

/// The stream's word for a directory refusal.
fn directory_reason(e: &DirectoryError) -> &'static str {
    match e {
        DirectoryError::Deleted(_) => "identity-deleted",
        DirectoryError::ExistingAuthenticatorNotLinked { .. } => "first-ceremony-not-this-account",
        DirectoryError::AuthenticatorRetired { .. } => "authenticator-retired",
        DirectoryError::ClaimRefused(why) => why,
        DirectoryError::AlreadyLinked { .. } => "already-linked",
        DirectoryError::UnknownPrincipal(_) => "unknown-principal",
        DirectoryError::StoreUnavailable(_) => "directory-unavailable",
        _ => "claim-refused",
    }
}

/// Run one ceremony through its provider and answer what it vouched for.
/// Resolves no principal: that is the caller's.
async fn ceremony(
    auth: &SharedAuth,
    body: &Value,
    op: &str,
    now: Timestamp,
) -> Result<AuthenticatedSubject, Box<Response>> {
    let Some(method) = body.get("method").and_then(|m| m.as_str()) else {
        return Err(Box::new(refuse(auth, op, "malformed-request", now)));
    };
    // A tenant's own provider (`plan-tenant-providers.md` §6).
    let tenant_provider = super::tenant::claim_provider(auth, op, body, method, now).await?;
    if tenant_provider.is_none() && auth.registry.get(method).is_err() {
        return Err(Box::new(refuse(auth, op, "unknown-method", now)));
    }
    let Some(req) = parse_complete(body) else {
        return Err(Box::new(refuse(auth, op, "malformed-request", now)));
    };
    // A passkey registration is a new key, so it is not a sign-in the
    // account holds and cannot open a window. As the claimed side it is read
    // by [`new_passkey`], never here.
    if matches!(req, CompleteRequest::PasskeyRegistration { .. }) {
        return Err(Box::new(refuse(
            auth,
            op,
            "registration-is-not-a-sign-in",
            now,
        )));
    }
    let auth2 = auth.clone();
    let method2 = method.to_string();
    let done = tokio::task::spawn_blocking(move || {
        if let Some(p) = &tenant_provider {
            return p.complete(&req, now);
        }
        auth2
            .registry
            .get(&method2)
            .expect("the method resolved above and the registry is immutable at serve time")
            .complete(&req, now)
    })
    .await;
    match done {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => {
            auth.record_refusal(op, auth_error_reason(&e), now);
            Err(Box::new(super::ceremony_refusal(&e)))
        }
        Err(_) => Err(Box::new(refuse(auth, op, "ceremony-task-failed", now))),
    }
}

/// `POST /auth/claim/begin`: prove the account again and open a window.
pub async fn begin(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    if !auth.linking.on {
        return linking_off(&auth, BEGIN, now);
    }
    let claims = match step_up_claims(&auth, &headers, BEGIN, now) {
        Ok(c) => c,
        Err(answer) => return *answer,
    };
    let Some(session) = claims.session.as_ref().map(|s| s.id.0.clone()) else {
        return refuse(&auth, BEGIN, "not-a-session", now);
    };
    let Some(store) = auth.ceremonies.clone() else {
        return refuse(&auth, BEGIN, "ceremony-store-not-wired", now);
    };
    let first = match ceremony(&auth, &body, BEGIN, now).await {
        Ok(s) => s,
        Err(answer) => return *answer,
    };
    // The ceremony must be one of this account's own sign-ins. A token
    // holder who completes a ceremony of their own account opens nothing
    // on the token's.
    let vouched = auth
        .directory
        .lock()
        .expect("directory mutex poisoned")
        .vouches(&claims.principal, &first.authenticator);
    if let Err(e) = vouched {
        return match e {
            DirectoryError::StoreUnavailable(c) => {
                auth.record_refusal(BEGIN, "directory-unavailable", now);
                StoreFailure::from(c).into_response()
            }
            other => refuse(&auth, BEGIN, directory_reason(&other), now),
        };
    }
    let secret = random_secret();
    let open = Open {
        principal: claims.principal.0.clone(),
        session,
        first: first.authenticator.0.clone(),
        first_at: now.0,
    };
    let expires_at = now.0.saturating_add(auth.linking.window_secs);
    let row = CeremonyRow {
        handle: handle_of(&secret),
        connection: CONNECTION.into(),
        phase: PHASE.into(),
        binder: String::new(),
        payload: open.to_payload(),
        minted_at: now,
        expires_at: Timestamp(expires_at),
    };
    let filed = tokio::task::spawn_blocking(move || store.put(&row)).await;
    if let Err(failure) = StoreFailure::joined(filed) {
        auth.record_refusal(BEGIN, "ceremony-store-unavailable", now);
        return failure.into_response();
    }
    auth.record(
        Some(&claims.principal),
        BEGIN,
        ordo_secevent::EventKind::Authenticated,
        now,
    );
    (
        StatusCode::OK,
        Json(json!({"claim": secret, "expires_at": expires_at})),
    )
        .into_response()
}

/// Read the open claim a secret names, if any, without spending it. `Err`
/// is a store that could not answer.
async fn peek(
    auth: &AuthState,
    secret: &str,
    now: Timestamp,
) -> Result<Option<Open>, StoreFailure> {
    let Some(store) = auth.ceremonies.clone() else {
        return Ok(None);
    };
    let handle = handle_of(secret);
    let found =
        tokio::task::spawn_blocking(move || store.peek(&handle, CONNECTION, PHASE, now)).await;
    Ok(StoreFailure::joined(found)?
        .as_deref()
        .and_then(Open::from_payload))
}

/// Does an email `begin` name an open claim? Then its address is mailed even
/// when nobody holds it, as an invitation lets it be. Only looked at, never
/// spent, and the answer to `begin` is the same either way. Off, absent, or
/// unreadable is `false`.
pub(crate) async fn opens_mail(auth: &AuthState, body: &Value, now: Timestamp) -> bool {
    if !auth.linking.on {
        return false;
    }
    let Some(secret) = body.get("claim").and_then(|c| c.as_str()) else {
        return false;
    };
    matches!(peek(auth, secret, now).await, Ok(Some(_)))
}

/// The binding a claim's passkey registration carries, so a registration
/// begun for a sign-up, a ticket or `/auth/authenticators/link` cannot land
/// here, and this one cannot land there.
fn principal_binding(principal: &PrincipalId) -> String {
    format!("claim:{}", principal.0)
}

/// `POST /auth/claim/passkey {claim}`: the creation options for a new
/// passkey, inside an open window. The account's own passkeys are excluded.
pub async fn passkey_begin(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    const OP: &str = "auth.claim.passkey";
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    if !auth.linking.on {
        return linking_off(&auth, OP, now);
    }
    let Some(passkey) = auth.passkey.clone() else {
        return refuse(&auth, OP, "unknown-method", now);
    };
    let claims = match step_up_claims(&auth, &headers, OP, now) {
        Ok(c) => c,
        Err(answer) => return *answer,
    };
    let Some(session) = claims.session.as_ref().map(|s| s.id.0.clone()) else {
        return refuse(&auth, OP, "not-a-session", now);
    };
    let Some(secret) = body.get("claim").and_then(|c| c.as_str()) else {
        return refuse(&auth, OP, "no-claim", now);
    };
    match peek(&auth, secret, now).await {
        Ok(Some(open)) if open.principal == claims.principal.0 && open.session == session => {}
        Ok(Some(_)) => return refuse(&auth, OP, "claim-of-another-session", now),
        Ok(None) => return refuse(&auth, OP, "claim-window-closed", now),
        Err(failure) => {
            auth.record_refusal(OP, "ceremony-store-unavailable", now);
            return failure.into_response();
        }
    }
    let (held, display) = {
        let dir = auth.lock_directory();
        let held = dir.authenticators_of(&claims.principal).unwrap_or_default();
        let display = dir
            .principal(&claims.principal)
            .ok()
            .flatten()
            .map(|p| p.display)
            .unwrap_or_default();
        (held, display)
    };
    let exclude: Vec<Vec<u8>> = held
        .iter()
        .filter_map(|a| a.0.strip_prefix("passkey:"))
        .filter_map(|id| ordo_identity::passkey::credential_id_from_b64url(id).ok())
        .collect();
    let binding = principal_binding(&claims.principal);
    let p2 = passkey.clone();
    let minted = tokio::task::spawn_blocking(move || {
        p2.begin_registration_bound(now, &exclude, Some(&binding))
    })
    .await;
    let options = match minted {
        Ok(Ok(options)) => options,
        Ok(Err(e)) => {
            auth.record_refusal(OP, "ceremony-store-unavailable", now);
            return StoreFailure::from(e).into_response();
        }
        Err(_) => {
            auth.record_refusal(OP, "ceremony-store-unavailable", now);
            return StoreFailure::task_failed().into_response();
        }
    };
    let rp = passkey.relying_party().name().to_string();
    let shown = if display.is_empty() {
        rp.clone()
    } else {
        display
    };
    (
        StatusCode::OK,
        Json(creation_options_json(
            &options,
            &rp,
            &shown,
            auth.passkey_timeout_ms(),
        )),
    )
        .into_response()
}

/// Verify a claim's passkey registration: the attestation, and that it was
/// begun at `/auth/claim/passkey` for this account. Nothing is stored.
async fn new_passkey(
    auth: &SharedAuth,
    body: &Value,
    binding: &str,
    now: Timestamp,
) -> Result<ordo_identity::passkey::PendingRegistration, Box<Response>> {
    let Some(passkey) = auth.passkey.clone() else {
        return Err(Box::new(refuse(auth, COMPLETE, "unknown-method", now)));
    };
    let Some(CompleteRequest::PasskeyRegistration {
        challenge,
        client_data_json,
        attestation_object,
    }) = parse_complete(body)
    else {
        return Err(Box::new(refuse(auth, COMPLETE, "malformed-request", now)));
    };
    let verified = tokio::task::spawn_blocking(move || {
        passkey.verify_registration(&challenge, &client_data_json, &attestation_object, now)
    })
    .await;
    let pending = match verified {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            auth.record_refusal(COMPLETE, auth_error_reason(&e), now);
            return Err(Box::new(super::ceremony_refusal(&e)));
        }
        Err(_) => {
            return Err(Box::new(refuse(
                auth,
                COMPLETE,
                "ceremony-task-failed",
                now,
            )))
        }
    };
    if pending.binding() != Some(binding) {
        return Err(Box::new(refuse(auth, COMPLETE, "claim-binding", now)));
    }
    Ok(pending)
}

/// `POST /auth/claim/complete`: the claimed sign-in's ceremony, then the
/// link.
pub async fn complete(
    State(auth): State<Option<SharedAuth>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(auth) = auth else {
        return unconfigured();
    };
    let now = Timestamp((auth.now)());
    if !auth.linking.on {
        return linking_off(&auth, COMPLETE, now);
    }
    let claims = match step_up_claims(&auth, &headers, COMPLETE, now) {
        Ok(c) => c,
        Err(answer) => return *answer,
    };
    let Some(session) = claims.session.as_ref().map(|s| s.id.0.clone()) else {
        return refuse(&auth, COMPLETE, "not-a-session", now);
    };
    let Some(secret) = body
        .get("claim")
        .and_then(|c| c.as_str())
        .map(str::to_string)
    else {
        return refuse(&auth, COMPLETE, "no-claim", now);
    };
    let Some(store) = auth.ceremonies.clone() else {
        return refuse(&auth, COMPLETE, "ceremony-store-not-wired", now);
    };
    // Looked at first, so a mistyped code does not spend the window, and
    // spent after the ceremony, so two claims on one window link once.
    let open = match peek(&auth, &secret, now).await {
        Ok(Some(open)) => open,
        Ok(None) => return refuse(&auth, COMPLETE, "claim-window-closed", now),
        Err(failure) => {
            auth.record_refusal(COMPLETE, "ceremony-store-unavailable", now);
            return failure.into_response();
        }
    };
    if open.principal != claims.principal.0 || open.session != session {
        return refuse(&auth, COMPLETE, "claim-of-another-session", now);
    }
    // The claimed sign-in's ceremony. A passkey assertion is refused: a
    // passkey already belongs to the account that registered it. A new
    // passkey is a registration begun at `/auth/claim/passkey` for this
    // account, and is stored only once it is linked.
    let kind = body.get("kind").and_then(|k| k.as_str());
    if kind == Some("passkey_assertion") {
        return refuse(&auth, COMPLETE, "passkey-assertion-not-claimable", now);
    }
    let (claimed, pending) = if kind == Some("passkey_registration") {
        match new_passkey(&auth, &body, &principal_binding(&claims.principal), now).await {
            Ok(p) => (p.subject().clone(), Some(p)),
            Err(answer) => return *answer,
        }
    } else {
        match ceremony(&auth, &body, COMPLETE, now).await {
            Ok(s) => (s, None),
            Err(answer) => return *answer,
        }
    };
    let handle = handle_of(&secret);
    let spent =
        tokio::task::spawn_blocking(move || store.take(&handle, CONNECTION, PHASE, now)).await;
    match StoreFailure::joined(spent) {
        Ok(Some(_)) => {}
        Ok(None) => return refuse(&auth, COMPLETE, "claim-window-closed", now),
        Err(failure) => {
            auth.record_refusal(COMPLETE, "ceremony-store-unavailable", now);
            return failure.into_response();
        }
    }
    let principal = PrincipalId(open.principal.clone());
    let first = AuthenticatorId(open.first.clone());
    let linked = auth
        .directory
        .lock()
        .expect("directory mutex poisoned")
        .claim(
            &principal,
            &first,
            Timestamp(open.first_at),
            &claimed.authenticator,
            now,
            auth.linking.window_secs,
        );
    let event = match linked {
        Ok(event) => event,
        // Already this account's: nothing to do, and nothing to hide from
        // the person who holds both.
        Err(DirectoryError::AlreadyLinked { to, .. }) if to == principal => {
            return (
                StatusCode::OK,
                Json(json!({"linked": claimed.authenticator.0,
                    "via": claimed.authenticator.via(claimed.method),
                    "already": true})),
            )
                .into_response();
        }
        Err(DirectoryError::AlreadyLinked { .. }) => {
            auth.record_refusal(COMPLETE, "identifier-taken", now);
            return identifier_taken();
        }
        // The claimed sign-in names a deleted account. The person completed
        // its ceremony, so they are told what a live account would tell
        // them, and the stream says which (D5, D7).
        Err(DirectoryError::Deleted(owner)) if owner != principal => {
            auth.record_refusal(COMPLETE, "identity-deleted: the claimed sign-in", now);
            return identifier_taken();
        }
        Err(DirectoryError::StoreUnavailable(c)) => {
            auth.record_refusal(COMPLETE, "directory-unavailable", now);
            return StoreFailure::from(c).into_response();
        }
        Err(e) => return refuse(&auth, COMPLETE, directory_reason(&e), now),
    };
    // A new passkey's key is stored now that it is linked. A store that
    // fails takes the link back: a link nobody can use is worse than none.
    if let (Some(pending), Some(passkey)) = (pending, auth.passkey.clone()) {
        let stored =
            tokio::task::spawn_blocking(move || passkey.store_registration(&pending)).await;
        if !matches!(stored, Ok(Ok(()))) {
            let _ = auth
                .directory
                .lock()
                .expect("directory mutex poisoned")
                .unlink(&claimed.authenticator, now);
            return match stored {
                Ok(Err(e @ ordo_identity::AuthError::StoreUnavailable(_))) => {
                    auth.record_refusal(COMPLETE, auth_error_reason(&e), now);
                    super::ceremony_refusal(&e)
                }
                Ok(Err(e)) => refuse(&auth, COMPLETE, auth_error_reason(&e), now),
                _ => refuse(&auth, COMPLETE, "ceremony-task-failed", now),
            };
        }
    }
    auth.record(
        Some(&principal),
        COMPLETE,
        ordo_secevent::EventKind::from(&event),
        now,
    );
    // What the provider vouched for is kept beside the link, as a sign-in
    // keeps it, so `GET /auth/identities` lists the claimed sign-in.
    let noted = claimed.authenticator.is_outside_provider()
        || claimed.method == ordo_identity::AuthMethodKind::EmailOtp;
    if noted {
        if let Some(attrs) = auth.identity_attrs.clone() {
            let (who, s2) = (principal.clone(), claimed.clone());
            let wrote = tokio::task::spawn_blocking(move || attrs.note(&who, &s2, now)).await;
            if !matches!(wrote, Ok(Ok(()))) {
                auth.record_refusal(COMPLETE, "identity-profile-unwritten", now);
            }
        }
    }
    let held = auth
        .directory
        .lock()
        .expect("directory mutex poisoned")
        .authenticators_of(&principal)
        .map(|l| l.len())
        .unwrap_or(0);
    (
        StatusCode::OK,
        Json(json!({
            "linked": claimed.authenticator.0,
            "via": claimed.authenticator.via(claimed.method),
            "authenticators": held,
        })),
    )
        .into_response()
}

/// The three claim routes, for [`super::router`] and the served door
/// (`serve::platform_router`).
pub fn routes<S>() -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
    Option<SharedAuth>: axum::extract::FromRef<S>,
{
    axum::Router::new()
        .route("/auth/claim/begin", axum::routing::post(begin))
        .route("/auth/claim/complete", axum::routing::post(complete))
        .route("/auth/claim/passkey", axum::routing::post(passkey_begin))
}
