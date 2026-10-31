//! The console's one typed error, and the one place it becomes an HTTP
//! answer (AGENTS.md rule 9).
//!
//! Every console failure is a [`ConsoleError`]. It keeps its cause: the
//! database client's [`PoolError`], the passkey provider's [`AuthError`], an
//! I/O error. It says what a caller can do through [`ConsoleError::class`],
//! read from the cause's type and never from its words. A refusal the
//! deployment owner can fix names the setting and the value to set
//! ([`ConsoleError::Setting`]).
//!
//! [`IntoResponse`] below is the only place a status and the error body are
//! chosen. A handler names what happened and never a number.

use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use ordo_exec::PoolError;
use ordo_fault::FaultClass;
use ordo_identity::AuthError;

/// The cause a setting refusal keeps.
pub type Cause = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Why a session no longer answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// Unused for longer than `ORDO_CONSOLE_SESSION_IDLE_SECS`.
    Idle { limit_secs: u64 },
    /// Older than `ORDO_CONSOLE_SESSION_MAX_SECS`.
    Absolute { limit_secs: u64 },
    /// Ended on purpose: signed out, its owner removed, or every session
    /// ended by `ordo console recover`.
    Ended,
}

/// Every way a console call or the console's boot can fail.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConsoleError {
    /// A setting is missing or wrong. `fix` says what to set it to.
    Setting {
        setting: &'static str,
        problem: String,
        fix: String,
        cause: Option<Cause>,
    },
    /// The console's own database did not answer as asked.
    Store(PoolError),
    /// A passkey ceremony was refused or could not be completed.
    Passkey(AuthError),
    /// No session cookie, or one this console does not know.
    NotSignedIn,
    /// The session existed and has ended.
    SessionEnded(SessionEnd),
    /// This call needs a passkey touch within the step-up window.
    StepUpNeeded { window_secs: u64 },
    /// A setup, invite, recover or recovery code that is wrong, used,
    /// voided or expired. One answer for all of them, so a guess learns
    /// nothing.
    CodeRefused,
    /// A recovery code signed this session in: it may only enrol a new
    /// passkey until it has.
    MustEnrolPasskey,
    /// Launch has at most this many console owners. More are v2.
    OwnerLimit { max: usize },
    /// Removing this owner would leave the console with none.
    LastOwner,
    /// No active owner with that id.
    UnknownOwner,
    /// `ordo console recover` found several owners and was not told which.
    OwnerNotNamed { owners: Vec<String> },
    /// No deployment with that handle.
    UnknownDeployment(String),
    /// The request body or a field in it is malformed.
    BadRequest(String),
    /// A state-changing request without the console's own `Origin`.
    OriginRefused,
    /// The console runs in door-only mode: it holds no runtime control.
    DoorOnly,
    /// `ordo-host check-config` refused the new settings. Nothing running
    /// was touched. `output` is what it said.
    SettingsRefused { revision: u64, output: String },
    /// The new revision did not come up, and the supervisor put the last
    /// good one back.
    RolledBack { revision: u64, to: u64, why: String },
    /// No runtime revision with that number.
    UnknownRevision(u64),
    /// A call to the deployment's door failed (`crate::door`).
    Door(crate::door::DoorError),
    /// The console holds no console key for this deployment yet: link it
    /// first (`plan-console.md` 8.2).
    NotLinked,
    /// The console is already linked to this deployment. Rotate the key,
    /// or unlink first.
    AlreadyLinked,
    /// The deployment needs the owner's own touch there (G15), relayed by
    /// the page, before this write.
    DeploymentTouchNeeded { window_secs: u64 },
    /// The deployment refused this console's key: it was revoked there, or
    /// the deployment no longer knows it.
    KeyRefused,
    /// The bearer token from the identity service did not verify
    /// (`crate::identity`): expired, signed by a key the service does not
    /// publish, or for another issuer or deployment.
    TokenRefused(ordo_identity::deployment_token::DeploymentTokenError),
    /// A machine key's token: the console is for people. Machine keys
    /// call the door themselves.
    PersonOnly,
    /// This console signs people in through the identity service, which
    /// now holds sign-in, passkeys, recovery codes and who may manage the
    /// deployment. The console's own routes for them, and the console key
    /// link to the door, answer this.
    MovedToIdentityService { origin: String },
    /// Deployment people live in the identity service, and this console
    /// has none set (`ORDO_IDENTITY_URL` unset).
    NoIdentityService,
    /// A management call to the identity service failed or was refused
    /// (`crate::identity::IdentityClient::call`). Its `presence_required`
    /// is turned into [`ConsoleError::StepUpNeeded`] where it is met.
    Identity(crate::identity::IdentityError),
    /// A call to the supervisor's socket failed or was refused
    /// (`crate::runtime`): it did not answer, refused the call, or a
    /// setting is wrong. `check-config` refusals and rollbacks are
    /// [`ConsoleError::SettingsRefused`] and [`ConsoleError::RolledBack`].
    Supervisor(ordo_supervisor::SupervisorError),
    /// Work handed to a blocking thread did not come back.
    Internal(String),
}

impl ConsoleError {
    /// A setting refusal with no cause.
    pub fn setting(
        setting: &'static str,
        problem: impl Into<String>,
        fix: impl Into<String>,
    ) -> ConsoleError {
        ConsoleError::Setting {
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
    ) -> ConsoleError {
        ConsoleError::Setting {
            setting,
            problem: problem.into(),
            fix: fix.into(),
            cause: Some(Box::new(cause)),
        }
    }

    /// What a caller can do about it, read from the cause's type.
    pub fn class(&self) -> FaultClass {
        match self {
            ConsoleError::Store(e) => e.class(),
            ConsoleError::Passkey(AuthError::StoreUnavailable(e)) => e.class(),
            ConsoleError::Passkey(_) => FaultClass::Refused,
            ConsoleError::Internal(_) => FaultClass::Broken,
            ConsoleError::Door(e) => e.class(),
            ConsoleError::Identity(e) if e.retryable() => FaultClass::Retryable,
            ConsoleError::Identity(crate::identity::IdentityError::Refused { .. }) => {
                FaultClass::Refused
            }
            ConsoleError::Identity(_) => FaultClass::Broken,
            ConsoleError::Supervisor(e) => match e.kind() {
                ordo_supervisor::Kind::Unavailable => FaultClass::Retryable,
                ordo_supervisor::Kind::Internal => FaultClass::Broken,
                _ => FaultClass::Refused,
            },
            ConsoleError::Setting { .. } => FaultClass::Refused,
            _ => FaultClass::Refused,
        }
    }

    /// The setting a fixable refusal names, if it names one.
    pub fn setting_name(&self) -> Option<&'static str> {
        match self {
            ConsoleError::Setting { setting, .. } => Some(setting),
            ConsoleError::SessionEnded(SessionEnd::Idle { .. }) => {
                Some(crate::config::SESSION_IDLE_ENV)
            }
            ConsoleError::SessionEnded(SessionEnd::Absolute { .. }) => {
                Some(crate::config::SESSION_MAX_ENV)
            }
            ConsoleError::DoorOnly => Some(crate::config::SUPERVISOR_SOCKET_ENV),
            ConsoleError::Supervisor(e) => e.setting_name(),
            ConsoleError::Door(crate::door::DoorError::Tls { .. }) => {
                Some(crate::config::DOOR_URL_ENV)
            }
            ConsoleError::TokenRefused(e) => token_setting(e),
            ConsoleError::MovedToIdentityService { .. } => Some(crate::identity::URL_ENV),
            ConsoleError::Identity(e) if e.code().is_none() => Some(crate::identity::URL_ENV),
            ConsoleError::NoIdentityService => Some(crate::identity::URL_ENV),
            _ => None,
        }
    }

    /// The one word a page or a script decides by.
    pub fn code(&self) -> &'static str {
        match self {
            ConsoleError::Setting { .. } => "setting_refused",
            ConsoleError::Store(e) => match e.class() {
                FaultClass::Busy => "server_busy",
                FaultClass::Retryable => "try_again",
                _ => "console_database_unavailable",
            },
            ConsoleError::Passkey(AuthError::StoreUnavailable(_)) => "console_database_unavailable",
            ConsoleError::Passkey(_) => "passkey_refused",
            ConsoleError::NotSignedIn => "not_signed_in",
            ConsoleError::SessionEnded(_) => "session_ended",
            ConsoleError::StepUpNeeded { .. } => "step_up_required",
            ConsoleError::CodeRefused => "code_refused",
            ConsoleError::MustEnrolPasskey => "enrol_a_new_passkey",
            ConsoleError::OwnerLimit { .. } => "owner_limit",
            ConsoleError::LastOwner => "last_owner",
            ConsoleError::UnknownOwner => "unknown_owner",
            ConsoleError::OwnerNotNamed { .. } => "owner_not_named",
            ConsoleError::UnknownDeployment(_) => "unknown_deployment",
            ConsoleError::BadRequest(_) => "bad_request",
            ConsoleError::OriginRefused => "origin_refused",
            ConsoleError::DoorOnly => "door_only",
            ConsoleError::SettingsRefused { .. } => "settings_refused",
            ConsoleError::RolledBack { .. } => "apply_rolled_back",
            ConsoleError::UnknownRevision(_) => "unknown_revision",
            ConsoleError::Door(e) if e.not_answering() => "deployment_not_answering",
            ConsoleError::Door(crate::door::DoorError::Busy { .. }) => "deployment_busy",
            ConsoleError::Door(crate::door::DoorError::Refused { .. }) => "deployment_refused",
            ConsoleError::Door(_) => "deployment_failed",
            ConsoleError::NotLinked => "not_linked",
            ConsoleError::AlreadyLinked => "already_linked",
            ConsoleError::DeploymentTouchNeeded { .. } => "deployment_touch_required",
            ConsoleError::KeyRefused => "console_key_refused",
            ConsoleError::TokenRefused(_) => "token_refused",
            ConsoleError::PersonOnly => "person_only",
            ConsoleError::MovedToIdentityService { .. } => "moved_to_identity_service",
            ConsoleError::NoIdentityService => "no_identity_service",
            ConsoleError::Identity(crate::identity::IdentityError::Refused { .. }) => {
                "identity_refused"
            }
            ConsoleError::Identity(_) => "identity_service_not_answering",
            ConsoleError::Supervisor(e) => match e.kind() {
                ordo_supervisor::Kind::Unavailable => "supervisor_not_answering",
                ordo_supervisor::Kind::NotAllowed => "not_allowed",
                ordo_supervisor::Kind::Setting => "setting_refused",
                _ => "supervisor_failed",
            },
            ConsoleError::Internal(_) => "internal",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            ConsoleError::Store(_) | ConsoleError::Passkey(AuthError::StoreUnavailable(_)) => {
                match self.class() {
                    FaultClass::Busy | FaultClass::Retryable => StatusCode::SERVICE_UNAVAILABLE,
                    FaultClass::Refused => StatusCode::BAD_REQUEST,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                }
            }
            ConsoleError::Setting { .. } | ConsoleError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            ConsoleError::Passkey(_)
            | ConsoleError::NotSignedIn
            | ConsoleError::SessionEnded(_)
            | ConsoleError::CodeRefused
            | ConsoleError::TokenRefused(_) => StatusCode::UNAUTHORIZED,
            ConsoleError::PersonOnly => StatusCode::FORBIDDEN,
            ConsoleError::MovedToIdentityService { .. } => StatusCode::GONE,
            ConsoleError::StepUpNeeded { .. }
            | ConsoleError::DeploymentTouchNeeded { .. }
            | ConsoleError::MustEnrolPasskey
            | ConsoleError::OriginRefused => StatusCode::FORBIDDEN,
            ConsoleError::OwnerLimit { .. } | ConsoleError::LastOwner | ConsoleError::DoorOnly => {
                StatusCode::CONFLICT
            }
            ConsoleError::SettingsRefused { .. } | ConsoleError::RolledBack { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            ConsoleError::UnknownOwner
            | ConsoleError::UnknownDeployment(_)
            | ConsoleError::UnknownRevision(_) => StatusCode::NOT_FOUND,
            ConsoleError::OwnerNotNamed { .. } | ConsoleError::BadRequest(_) => {
                StatusCode::BAD_REQUEST
            }
            ConsoleError::Door(e) => match e.class() {
                FaultClass::Busy | FaultClass::Retryable => StatusCode::SERVICE_UNAVAILABLE,
                FaultClass::Refused => StatusCode::FORBIDDEN,
                _ => StatusCode::BAD_GATEWAY,
            },
            ConsoleError::NotLinked | ConsoleError::AlreadyLinked | ConsoleError::KeyRefused => {
                StatusCode::CONFLICT
            }
            ConsoleError::Identity(crate::identity::IdentityError::Refused { status, .. }) => {
                match *status {
                    400 => StatusCode::BAD_REQUEST,
                    404 => StatusCode::NOT_FOUND,
                    409 => StatusCode::CONFLICT,
                    429 => StatusCode::TOO_MANY_REQUESTS,
                    s if s >= 500 => StatusCode::BAD_GATEWAY,
                    _ => StatusCode::FORBIDDEN,
                }
            }
            ConsoleError::NoIdentityService => StatusCode::CONFLICT,
            ConsoleError::Identity(e) if e.retryable() => StatusCode::SERVICE_UNAVAILABLE,
            ConsoleError::Supervisor(e) => match e.kind() {
                ordo_supervisor::Kind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                ordo_supervisor::Kind::NotAllowed => StatusCode::FORBIDDEN,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            ConsoleError::Identity(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

impl fmt::Display for ConsoleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsoleError::Setting {
                setting,
                problem,
                fix,
                ..
            } => write!(f, "{setting}: {problem}. {fix}"),
            ConsoleError::Store(e) => write!(f, "the console's database: {e}"),
            ConsoleError::Passkey(e) => write!(f, "the passkey was refused: {e}"),
            ConsoleError::NotSignedIn => f.write_str("sign in to the console first"),
            ConsoleError::SessionEnded(SessionEnd::Idle { limit_secs }) => write!(
                f,
                "the session was unused for more than {limit_secs} seconds; sign in again, \
                 or raise {} to keep idle sessions longer",
                crate::config::SESSION_IDLE_ENV
            ),
            ConsoleError::SessionEnded(SessionEnd::Absolute { limit_secs }) => write!(
                f,
                "the session is older than {limit_secs} seconds; sign in again, or raise {} \
                 to keep sessions longer",
                crate::config::SESSION_MAX_ENV
            ),
            ConsoleError::SessionEnded(SessionEnd::Ended) => {
                f.write_str("this session was ended; sign in again")
            }
            ConsoleError::StepUpNeeded { window_secs } => write!(
                f,
                "this needs a passkey touch in the last {window_secs} seconds; touch your \
                 passkey and try again"
            ),
            ConsoleError::CodeRefused => f.write_str("that code is wrong, used, voided or expired"),
            ConsoleError::MustEnrolPasskey => f.write_str(
                "a recovery code signed you in; enrol a new passkey before anything else",
            ),
            ConsoleError::OwnerLimit { max } => write!(
                f,
                "the console has {max} owners already, which is the most it holds at launch"
            ),
            ConsoleError::LastOwner => f.write_str(
                "that is the console's only owner; invite a second owner before removing it",
            ),
            ConsoleError::UnknownOwner => f.write_str("no active console owner has that id"),
            ConsoleError::OwnerNotNamed { owners } => write!(
                f,
                "the console has {} owners; name the one to recover with --owner <id> \
                 (one of: {})",
                owners.len(),
                owners.join(", ")
            ),
            ConsoleError::UnknownDeployment(h) => {
                write!(f, "this console manages no deployment named `{h}`")
            }
            ConsoleError::BadRequest(why) => write!(f, "bad request: {why}"),
            ConsoleError::OriginRefused => f.write_str(
                "a console write must come from the console's own page (its Origin header)",
            ),
            ConsoleError::DoorOnly => write!(
                f,
                "this console runs in door-only mode and cannot start, stop or change the \
                 deployment: {} is not set. Run the one image `ordo`, whose supervisor sets \
                 it, or change the deployment's settings on your own platform",
                crate::config::SUPERVISOR_SOCKET_ENV
            ),
            ConsoleError::SettingsRefused { revision, output } => write!(
                f,
                "revision {revision} was refused by `ordo-host check-config`, and nothing \
                 running was touched: {output}"
            ),
            ConsoleError::RolledBack { revision, to, why } => write!(
                f,
                "revision {revision} did not come up ({why}); revision {to} is running again"
            ),
            ConsoleError::UnknownRevision(n) => write!(f, "there is no runtime revision {n}"),
            ConsoleError::Door(e) => e.fmt(f),
            ConsoleError::NotLinked => f.write_str(
                "this console is not linked to the deployment yet; link it with your passkey \
                 at the deployment",
            ),
            ConsoleError::AlreadyLinked => f.write_str(
                "this console is already linked to the deployment; rotate its key, or unlink \
                 it first",
            ),
            ConsoleError::DeploymentTouchNeeded { window_secs } => write!(
                f,
                "the deployment needs your passkey touch there in the last {window_secs} \
                 seconds; touch your passkey for the deployment and try again"
            ),
            ConsoleError::KeyRefused => f.write_str(
                "the deployment refused this console's key: it was revoked there, or the \
                 deployment no longer knows it. Forget the link here and link again",
            ),
            ConsoleError::TokenRefused(e) => token_refusal(f, e),
            ConsoleError::PersonOnly => f.write_str(
                "only a person signed in at the identity service can use the console; a \
                 machine key calls the deployment's door itself, and a break-glass token is \
                 for the `ordo` CLI on the host",
            ),
            ConsoleError::Identity(e) => e.fmt(f),
            ConsoleError::Supervisor(e) => write!(f, "the supervisor: {e}"),
            ConsoleError::NoIdentityService => write!(
                f,
                "the people who manage the deployment live in the identity service, and this \
                 console has none; set {} to reach it",
                crate::identity::URL_ENV
            ),
            ConsoleError::MovedToIdentityService { origin } => write!(
                f,
                "this console signs people in through the identity service at {origin}, \
                 which keeps sign-in, passkeys, recovery codes and the people who manage the \
                 deployment. The console calls the deployment as you, with no console key. \
                 Unset {} only to go back to the console's own sign-in",
                crate::identity::URL_ENV
            ),
            ConsoleError::Internal(why) => write!(f, "internal: {why}"),
        }
    }
}

/// The setting a token refusal points at, when the deployment owner can
/// fix it by changing one.
fn token_setting(
    e: &ordo_identity::deployment_token::DeploymentTokenError,
) -> Option<&'static str> {
    use ordo_identity::deployment_token::DeploymentTokenError as T;
    match e {
        T::WrongIssuer => Some(crate::identity::ISSUER_ENV),
        T::WrongAudience => Some(crate::config::DEPLOYMENT_ENV),
        T::Unverifiable(j) if j.0 == "no-keys" => Some(crate::identity::URL_ENV),
        _ => None,
    }
}

fn token_refusal(
    f: &mut fmt::Formatter<'_>,
    e: &ordo_identity::deployment_token::DeploymentTokenError,
) -> fmt::Result {
    use ordo_identity::deployment_token::DeploymentTokenError as T;
    match e {
        T::Expired => f.write_str("your sign-in token has expired; the page asks for a new one"),
        T::WrongIssuer => write!(
            f,
            "the token was issued by another identity service than the one this console \
             trusts; set {} to the identity service's public origin, the value it has",
            crate::identity::ISSUER_ENV
        ),
        T::WrongAudience => write!(
            f,
            "the token is for another deployment; set {} on the identity service to this \
             console's handle, {}",
            crate::identity::DEPLOYMENT_ENV,
            crate::config::DEPLOYMENT_ENV
        ),
        T::Unverifiable(j) if j.0 == "no-keys" => write!(
            f,
            "the console holds no keys from the identity service yet; check that it runs \
             and that {} names it",
            crate::identity::URL_ENV
        ),
        other => write!(f, "the token was refused ({}); sign in again", other.word()),
    }
}

impl std::error::Error for ConsoleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConsoleError::Store(e) => Some(e),
            ConsoleError::Passkey(e) => Some(e),
            ConsoleError::Door(e) => Some(e),
            ConsoleError::TokenRefused(e) => Some(e),
            ConsoleError::Identity(e) => Some(e),
            ConsoleError::Supervisor(e) => Some(e),
            ConsoleError::Setting { cause, .. } => cause
                .as_deref()
                .map(|c| c as &(dyn std::error::Error + 'static)),
            _ => None,
        }
    }
}

impl From<PoolError> for ConsoleError {
    fn from(e: PoolError) -> ConsoleError {
        ConsoleError::Store(e)
    }
}

impl From<AuthError> for ConsoleError {
    fn from(e: AuthError) -> ConsoleError {
        ConsoleError::Passkey(e)
    }
}

/// The edge: the one place a [`ConsoleError`] becomes a status and a body.
///
/// The body is `{"error", "message", "retry"}`, plus `"setting"` when the
/// refusal names one. A broken store's text stays in the log: a statement
/// can name a table, and the page has no use for it.
impl IntoResponse for ConsoleError {
    fn into_response(self) -> Response {
        let status = self.status();
        let message = match &self {
            ConsoleError::Store(_) | ConsoleError::Passkey(AuthError::StoreUnavailable(_))
                if !self.class().may_retry() =>
            {
                eprintln!("ordo-console: {self}");
                "the console's database did not answer; the console's log has the cause".to_string()
            }
            ConsoleError::Internal(_) => {
                eprintln!("ordo-console: {self}");
                "the console failed; its log has the cause".to_string()
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
        if let Some(code) = match &self {
            ConsoleError::Door(e) => e.door_code(),
            _ => None,
        } {
            body["doorError"] = serde_json::Value::String(code.to_string());
        }
        if let ConsoleError::Identity(e) = &self {
            if let Some(code) = e.code() {
                body["identityError"] = serde_json::Value::String(code.to_string());
            }
        }
        if let ConsoleError::StepUpNeeded { window_secs }
        | ConsoleError::DeploymentTouchNeeded { window_secs } = &self
        {
            body["windowSecs"] = serde_json::json!(window_secs);
        }
        match &self {
            ConsoleError::SettingsRefused { revision, .. } => {
                body["revision"] = serde_json::json!(revision);
            }
            ConsoleError::RolledBack { revision, to, .. } => {
                body["revision"] = serde_json::json!(revision);
                body["rolledBackTo"] = serde_json::json!(to);
            }
            _ => {}
        }
        (status, axum::Json(body)).into_response()
    }
}
