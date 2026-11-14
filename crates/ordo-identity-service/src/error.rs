//! The identity service's one typed error, and the one place it becomes an
//! HTTP answer (AGENTS.md rule 9).
//!
//! Every failure is an [`IdentityError`]. It keeps its cause (the database
//! client's [`PoolError`], the passkey provider's [`AuthError`], an I/O
//! error), says what a caller can do through [`IdentityError::class`], read
//! from the cause's type and never from its words, and, when the deployment
//! owner can fix it, names the setting ([`IdentityError::setting_name`]).
//!
//! [`IntoResponse`] below is the only place a status and the error body are
//! chosen.

use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use ordo_exec::PoolError;
use ordo_fault::FaultClass;
use ordo_identity::deployment_token::DeploymentTokenError;
use ordo_identity::AuthError;

use crate::config::{CLIENT_ORIGINS_ENV, DEPLOYMENT_ENV, SESSION_IDLE_ENV, SESSION_MAX_ENV};

/// The cause a setting refusal keeps.
pub type Cause = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Why a session no longer answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// Unused for longer than `ORDO_IDENTITY_SESSION_IDLE_HOURS`.
    Idle { limit_hours: u64 },
    /// Older than `ORDO_IDENTITY_SESSION_MAX_HOURS`.
    Absolute { limit_hours: u64 },
    /// Ended on purpose: signed out, or the person's access ended.
    Ended,
}

impl SessionEnd {
    /// The word stored as the session's `ended_why`.
    pub fn why(self) -> &'static str {
        match self {
            SessionEnd::Idle { .. } => "idle",
            SessionEnd::Absolute { .. } => "max",
            SessionEnd::Ended => "ended",
        }
    }
}

/// Every way a call to the identity service, or its boot, can fail.
#[derive(Debug)]
#[non_exhaustive]
pub enum IdentityError {
    /// A setting is missing or wrong. `fix` says what to set it to.
    Setting {
        setting: &'static str,
        problem: String,
        fix: String,
        cause: Option<Cause>,
    },
    /// The service's own database did not answer as asked.
    Store(PoolError),
    /// A passkey ceremony was refused or could not be completed.
    Passkey(AuthError),
    /// No session, or one this service does not know.
    NotSignedIn,
    /// The session existed and has ended.
    SessionEnded(SessionEnd),
    /// A bearer token this service did not issue, or no longer accepts.
    TokenRefused(DeploymentTokenError),
    /// A token was asked for another deployment's audience.
    WrongAudience { asked: String, serves: String },
    /// A browser call from a page this service does not serve.
    OriginRefused,
    /// An invite link, a recovery code or a CLI code that is unknown,
    /// expired, spent or revoked. One answer for all, so a guess learns
    /// nothing; the audit log has the reason.
    CodeRefused,
    /// A machine key sign-in that did not verify: unknown key, ended key,
    /// stale or foreign challenge, bad signature. One answer for all.
    MachineRefused,
    /// Too many tries of one kind on this node; try again after this many
    /// seconds.
    Throttled { retry_after_secs: u64 },
    /// A recovery code signed this session in: it may only enrol a passkey
    /// until it has.
    MustEnrolPasskey,
    /// This write needs a passkey touch within the window.
    TouchNeeded { window_secs: u64 },
    /// Ending this person would leave the deployment with no admin.
    LastAdmin,
    /// No live person with that id.
    UnknownPerson,
    /// No invite with that id that can still be revoked.
    UnknownInvite,
    /// No live machine key with that id.
    UnknownMachineKey,
    /// The signed-in person holds no passkey with that id.
    UnknownPasskey,
    /// Removing this passkey would leave the person with none.
    LastPasskey,
    /// A live machine key already has that name.
    NameTaken(String),
    /// A machine key asked to make a key its minting grant does not cover.
    /// Says what the deployment owner gives.
    MintRefused(String),
    /// A machine key lacks the grant this act needs. Names the grant and
    /// what the key holds.
    GrantNeeded {
        grant: &'static str,
        holds: Vec<String>,
    },
    /// The person holds no membership of the deployment a token was asked
    /// for. Names the handle.
    NotAMember(String),
    /// No live registered deployment has that handle.
    UnknownDeployment(String),
    /// A live deployment already has that handle.
    DeploymentTaken(String),
    /// A sign-in asked to send the browser back somewhere the deployment
    /// does not list.
    ReturnRefused,
    /// This act is the service's own: a token for a registered deployment
    /// may not do it. Names the act and the service's handle.
    HomeOnly { act: &'static str, home: String },
    /// The audit log or one of its checkpoints does not check out.
    AuditBroken(crate::audit::AuditBreak),
    /// The request body or a field in it is malformed.
    BadRequest(String),
    /// Something that should not happen; the log has the detail.
    Internal(String),
}

impl IdentityError {
    /// A setting refusal with no cause.
    pub fn setting(
        setting: &'static str,
        problem: impl Into<String>,
        fix: impl Into<String>,
    ) -> IdentityError {
        IdentityError::Setting {
            setting,
            problem: problem.into(),
            fix: fix.into(),
            cause: None,
        }
    }

    /// A setting refusal, keeping what caused it.
    pub fn setting_caused(
        setting: &'static str,
        problem: impl Into<String>,
        fix: impl Into<String>,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> IdentityError {
        IdentityError::Setting {
            setting,
            problem: problem.into(),
            fix: fix.into(),
            cause: Some(Box::new(cause)),
        }
    }

    /// What a caller can do about it, read from the cause's type.
    pub fn class(&self) -> FaultClass {
        match self {
            IdentityError::Store(e) => e.class(),
            IdentityError::Passkey(AuthError::StoreUnavailable(e)) => e.class(),
            IdentityError::Throttled { .. } => FaultClass::Busy,
            IdentityError::Internal(_) => FaultClass::Broken,
            _ => FaultClass::Refused,
        }
    }

    /// The setting a fixable refusal names, if it names one.
    pub fn setting_name(&self) -> Option<&'static str> {
        match self {
            IdentityError::Setting { setting, .. } => Some(setting),
            IdentityError::SessionEnded(SessionEnd::Idle { .. }) => Some(SESSION_IDLE_ENV),
            IdentityError::SessionEnded(SessionEnd::Absolute { .. }) => Some(SESSION_MAX_ENV),
            IdentityError::WrongAudience { .. } => Some(DEPLOYMENT_ENV),
            IdentityError::OriginRefused => Some(CLIENT_ORIGINS_ENV),
            _ => None,
        }
    }

    /// The one word a page or a script decides by.
    pub fn code(&self) -> &'static str {
        match self {
            IdentityError::Setting { .. } => "setting_refused",
            IdentityError::Store(e) => match e.class() {
                FaultClass::Busy => "server_busy",
                FaultClass::Retryable => "try_again",
                _ => "identity_database_unavailable",
            },
            IdentityError::Passkey(AuthError::StoreUnavailable(_)) => {
                "identity_database_unavailable"
            }
            IdentityError::Passkey(_) => "passkey_refused",
            IdentityError::NotSignedIn => "not_signed_in",
            IdentityError::SessionEnded(_) => "session_ended",
            IdentityError::TokenRefused(_) => "token_refused",
            IdentityError::WrongAudience { .. } => "wrong_audience",
            IdentityError::OriginRefused => "origin_refused",
            IdentityError::CodeRefused => "code_refused",
            IdentityError::MachineRefused => "machine_refused",
            IdentityError::Throttled { .. } => "too_many_attempts",
            IdentityError::MustEnrolPasskey => "enrol_a_new_passkey",
            IdentityError::TouchNeeded { .. } => "presence_required",
            IdentityError::LastAdmin => "last_admin",
            IdentityError::UnknownPerson => "unknown_person",
            IdentityError::UnknownInvite => "unknown_invite",
            IdentityError::UnknownMachineKey => "unknown_machine_key",
            IdentityError::UnknownPasskey => "unknown_passkey",
            IdentityError::LastPasskey => "last_passkey",
            IdentityError::NameTaken(_) => "name_taken",
            IdentityError::MintRefused(_) => "mint_refused",
            IdentityError::GrantNeeded { .. } => "grant_needed",
            IdentityError::NotAMember(_) => "not_a_member",
            IdentityError::UnknownDeployment(_) => "unknown_deployment",
            IdentityError::DeploymentTaken(_) => "deployment_taken",
            IdentityError::ReturnRefused => "return_refused",
            IdentityError::HomeOnly { .. } => "home_only",
            IdentityError::AuditBroken(_) => "audit_broken",
            IdentityError::BadRequest(_) => "bad_request",
            IdentityError::Internal(_) => "internal",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            IdentityError::Store(_) | IdentityError::Passkey(AuthError::StoreUnavailable(_)) => {
                match self.class() {
                    FaultClass::Busy | FaultClass::Retryable => StatusCode::SERVICE_UNAVAILABLE,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                }
            }
            IdentityError::Setting { .. }
            | IdentityError::Internal(_)
            | IdentityError::AuditBroken(_) => StatusCode::INTERNAL_SERVER_ERROR,
            IdentityError::Passkey(_)
            | IdentityError::NotSignedIn
            | IdentityError::SessionEnded(_)
            | IdentityError::TokenRefused(_)
            | IdentityError::CodeRefused
            | IdentityError::MachineRefused => StatusCode::UNAUTHORIZED,
            IdentityError::WrongAudience { .. }
            | IdentityError::OriginRefused
            | IdentityError::MustEnrolPasskey
            | IdentityError::MintRefused(_)
            | IdentityError::GrantNeeded { .. }
            | IdentityError::NotAMember(_)
            | IdentityError::HomeOnly { .. }
            | IdentityError::TouchNeeded { .. } => StatusCode::FORBIDDEN,
            IdentityError::DeploymentTaken(_) => StatusCode::CONFLICT,
            IdentityError::UnknownDeployment(_) => StatusCode::NOT_FOUND,
            IdentityError::ReturnRefused => StatusCode::BAD_REQUEST,
            IdentityError::Throttled { .. } => StatusCode::TOO_MANY_REQUESTS,
            IdentityError::LastAdmin | IdentityError::NameTaken(_) | IdentityError::LastPasskey => {
                StatusCode::CONFLICT
            }
            IdentityError::UnknownPerson
            | IdentityError::UnknownInvite
            | IdentityError::UnknownMachineKey
            | IdentityError::UnknownPasskey => StatusCode::NOT_FOUND,
            IdentityError::BadRequest(_) => StatusCode::BAD_REQUEST,
        }
    }
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityError::Setting {
                setting,
                problem,
                fix,
                ..
            } => write!(f, "{setting}: {problem}. {fix}"),
            IdentityError::Store(e) => write!(f, "the identity service's database: {e}"),
            IdentityError::Passkey(e) => write!(f, "the passkey was refused: {e}"),
            IdentityError::NotSignedIn => f.write_str("sign in to the identity service first"),
            IdentityError::SessionEnded(SessionEnd::Idle { limit_hours }) => write!(
                f,
                "the session was unused for more than {limit_hours} hours; sign in again, or \
                 raise {SESSION_IDLE_ENV} to keep idle sessions longer"
            ),
            IdentityError::SessionEnded(SessionEnd::Absolute { limit_hours }) => write!(
                f,
                "the session is older than {limit_hours} hours; sign in again, or raise \
                 {SESSION_MAX_ENV} to keep sessions longer"
            ),
            IdentityError::SessionEnded(SessionEnd::Ended) => {
                f.write_str("this session was ended; sign in again")
            }
            IdentityError::TokenRefused(e) => {
                write!(f, "{e}; get a new token from the identity service")
            }
            IdentityError::WrongAudience { asked, serves } => write!(
                f,
                "a token was asked for `{asked}`, and this identity service serves `{serves}` \
                 and the deployments registered with it; ask for one of those, set \
                 {DEPLOYMENT_ENV} to the deployment's handle, or register the deployment \
                 (`ordo-identity-service deployment add`)"
            ),
            IdentityError::OriginRefused => write!(
                f,
                "this page may not call the identity service; add its origin to \
                 {CLIENT_ORIGINS_ENV}"
            ),
            IdentityError::CodeRefused => f.write_str("this link or code does not work"),
            IdentityError::MachineRefused => f.write_str("the machine key did not sign in"),
            IdentityError::Throttled { retry_after_secs } => {
                write!(f, "too many tries; try again in {retry_after_secs} seconds")
            }
            IdentityError::MustEnrolPasskey => f.write_str(
                "a recovery code signed you in; enrol a new passkey before anything else",
            ),
            IdentityError::TouchNeeded { window_secs } => write!(
                f,
                "this needs a passkey touch in the last {window_secs} seconds; touch your \
                 passkey and try again"
            ),
            IdentityError::LastAdmin => f.write_str(
                "that is the deployment's last admin; invite another admin first, or use \
                 `ordo-identity-service invite --role admin` on the host (break-glass)",
            ),
            IdentityError::UnknownPerson => f.write_str("no live deployment person has that id"),
            IdentityError::UnknownInvite => f.write_str("no unspent, unrevoked invite has that id"),
            IdentityError::UnknownMachineKey => f.write_str("no live machine key has that id"),
            IdentityError::UnknownPasskey => f.write_str("you hold no passkey with that id"),
            IdentityError::LastPasskey => f.write_str(
                "that is your only passkey; add another passkey first, then remove this one",
            ),
            IdentityError::MintRefused(why) => f.write_str(why),
            IdentityError::GrantNeeded { grant, holds } => write!(
                f,
                "this machine key holds {} and this act needs the `{grant}` grant; a \
                 deployment owner makes a key whose `grants` names `{grant}`, or a person \
                 does this act",
                if holds.is_empty() {
                    "no grant".to_string()
                } else {
                    format!("`{}`", holds.join("`, `"))
                }
            ),
            IdentityError::NameTaken(name) => write!(
                f,
                "a live machine key is already named `{name}`; choose another name, or end it \
                 first"
            ),
            IdentityError::NotAMember(handle) => write!(
                f,
                "you are not a member of deployment `{handle}`; an admin of `{handle}` invites \
                 you, or the identity service's owner runs `ordo-identity-service member add \
                 {handle} <person>`"
            ),
            IdentityError::UnknownDeployment(handle) => write!(
                f,
                "no deployment `{handle}` is registered with this identity service; its owner \
                 runs `ordo-identity-service deployment add {handle} --origin <console origin>`"
            ),
            IdentityError::DeploymentTaken(handle) => write!(
                f,
                "deployment `{handle}` is already registered; end it first to change its \
                 origins (`ordo-identity-service deployment end {handle}`), or choose another \
                 handle"
            ),
            IdentityError::ReturnRefused => f.write_str(
                "that return address is not one this deployment lists; sign-in goes to the \
                 account page instead",
            ),
            IdentityError::HomeOnly { act, home } => write!(
                f,
                "{act} is done at the identity service's own deployment `{home}`, with a token \
                 for it"
            ),
            IdentityError::AuditBroken(b) => write!(f, "the audit log does not check out: {b}"),
            IdentityError::BadRequest(why) => write!(f, "bad request: {why}"),
            IdentityError::Internal(why) => write!(f, "internal: {why}"),
        }
    }
}

impl std::error::Error for IdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IdentityError::Store(e) => Some(e),
            IdentityError::Passkey(e) => Some(e),
            IdentityError::TokenRefused(e) => Some(e),
            IdentityError::AuditBroken(b) => Some(b),
            IdentityError::Setting { cause, .. } => cause
                .as_deref()
                .map(|c| c as &(dyn std::error::Error + 'static)),
            _ => None,
        }
    }
}

impl From<PoolError> for IdentityError {
    fn from(e: PoolError) -> IdentityError {
        IdentityError::Store(e)
    }
}

impl From<AuthError> for IdentityError {
    fn from(e: AuthError) -> IdentityError {
        IdentityError::Passkey(e)
    }
}

/// The edge: the one place an [`IdentityError`] becomes a status and a
/// body, `{"error", "message", "retry"}`, plus `"setting"` when the refusal
/// names one, `"windowSecs"` for a touch and `"retryAfterSecs"` for a
/// throttle. A broken store's text stays in the log.
impl IntoResponse for IdentityError {
    fn into_response(self) -> Response {
        let status = self.status();
        let message = match &self {
            IdentityError::Store(_) | IdentityError::Passkey(AuthError::StoreUnavailable(_))
                if !self.class().may_retry() =>
            {
                eprintln!("ordo-identity-service: {self}");
                "the identity service's database did not answer; its log has the cause".to_string()
            }
            IdentityError::Internal(_) | IdentityError::Setting { .. } => {
                eprintln!("ordo-identity-service: {self}");
                "the identity service failed; its log has the cause".to_string()
            }
            _ => self.to_string(),
        };
        let mut body = serde_json::json!({
            "error": self.code(),
            "message": message,
            "retry": self.class().may_retry(),
        });
        if let Some(setting) = self.setting_name() {
            body["setting"] = serde_json::Value::String(setting.to_string());
        }
        match &self {
            IdentityError::TouchNeeded { window_secs } => {
                body["windowSecs"] = serde_json::json!(window_secs);
            }
            IdentityError::Throttled { retry_after_secs } => {
                body["retryAfterSecs"] = serde_json::json!(retry_after_secs);
            }
            _ => {}
        }
        (status, axum::Json(body)).into_response()
    }
}
