//! The supervisor's one typed error (AGENTS.md rule 9).
//!
//! Every failure at boot, in the loop and over the socket is a
//! [`SupervisorError`]. It keeps its cause and says what a caller can do
//! ([`SupervisorError::kind`]): fix a setting, fix the request, retry, or
//! nothing (the call is not allowed). A refusal the deployment owner can fix
//! names the setting and the value to set.
//!
//! The socket carries it as [`Wire`], and the client turns that back into
//! the same type, so the console sees the error the supervisor made.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The cause a refusal keeps.
pub type Cause = Box<dyn std::error::Error + Send + Sync + 'static>;

/// What a caller can do about an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A setting is missing or wrong: change it and start again.
    Setting,
    /// The request is malformed or names something unknown: fix it.
    BadRequest,
    /// The call is never allowed for this child, or for this caller.
    NotAllowed,
    /// The deployment's own `check-config` refused the new settings.
    SettingsRefused,
    /// The new revision did not come up and the last good one is back.
    RolledBack,
    /// The supervisor or a child could not be reached: try again.
    Unavailable,
    /// Something the supervisor holds is broken: read the log.
    Internal,
}

/// Every way the supervisor or a call to it can fail.
#[derive(Debug)]
#[non_exhaustive]
pub enum SupervisorError {
    /// A setting is missing or wrong. `fix` says what to set it to.
    Setting {
        setting: &'static str,
        problem: String,
        fix: String,
        cause: Option<Cause>,
    },
    /// The request is malformed.
    BadRequest(String),
    /// The call is not allowed for that child or that caller.
    NotAllowed(String),
    /// `ordo-host check-config` refused the revision.
    SettingsRefused { revision: u64, output: String },
    /// The revision did not come up; `to` is running again.
    RolledBack { revision: u64, to: u64, why: String },
    /// The supervisor could not be reached, or has stopped.
    Unavailable { what: String, cause: Option<Cause> },
    /// The supervisor's own state is broken.
    Internal(String),
}

impl SupervisorError {
    pub fn setting(
        setting: &'static str,
        problem: impl Into<String>,
        fix: impl Into<String>,
    ) -> SupervisorError {
        SupervisorError::Setting {
            setting,
            problem: problem.into(),
            fix: fix.into(),
            cause: None,
        }
    }

    pub fn setting_caused(
        setting: &'static str,
        problem: impl Into<String>,
        fix: impl Into<String>,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> SupervisorError {
        SupervisorError::Setting {
            setting,
            problem: problem.into(),
            fix: fix.into(),
            cause: Some(Box::new(cause)),
        }
    }

    pub fn unavailable(
        what: impl Into<String>,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> SupervisorError {
        SupervisorError::Unavailable {
            what: what.into(),
            cause: Some(Box::new(cause)),
        }
    }

    pub fn stopped() -> SupervisorError {
        SupervisorError::Unavailable {
            what: "the supervisor has stopped".into(),
            cause: None,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            SupervisorError::Setting { .. } => Kind::Setting,
            SupervisorError::BadRequest(_) => Kind::BadRequest,
            SupervisorError::NotAllowed(_) => Kind::NotAllowed,
            SupervisorError::SettingsRefused { .. } => Kind::SettingsRefused,
            SupervisorError::RolledBack { .. } => Kind::RolledBack,
            SupervisorError::Unavailable { .. } => Kind::Unavailable,
            SupervisorError::Internal(_) => Kind::Internal,
        }
    }

    /// The setting a refusal names, if it is one.
    pub fn setting_name(&self) -> Option<&'static str> {
        match self {
            SupervisorError::Setting { setting, .. } => Some(setting),
            _ => None,
        }
    }

    /// Whether trying the same call again later may work.
    pub fn retry(&self) -> bool {
        matches!(self, SupervisorError::Unavailable { .. })
    }

    /// The error as the socket carries it.
    pub fn to_wire(&self) -> Wire {
        let (revision, to) = match self {
            SupervisorError::SettingsRefused { revision, .. } => (Some(*revision), None),
            SupervisorError::RolledBack { revision, to, .. } => (Some(*revision), Some(*to)),
            _ => (None, None),
        };
        Wire {
            kind: self.kind(),
            message: self.to_string(),
            setting: self.setting_name().map(str::to_string),
            revision,
            to,
        }
    }
}

/// An error on the socket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Wire {
    pub kind: Kind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u64>,
}

impl From<Wire> for SupervisorError {
    fn from(w: Wire) -> SupervisorError {
        match w.kind {
            Kind::BadRequest | Kind::Setting => SupervisorError::BadRequest(w.message),
            Kind::NotAllowed => SupervisorError::NotAllowed(w.message),
            Kind::SettingsRefused => SupervisorError::SettingsRefused {
                revision: w.revision.unwrap_or(0),
                output: w.message,
            },
            Kind::RolledBack => SupervisorError::RolledBack {
                revision: w.revision.unwrap_or(0),
                to: w.to.unwrap_or(0),
                why: w.message,
            },
            Kind::Unavailable => SupervisorError::Unavailable {
                what: w.message,
                cause: None,
            },
            Kind::Internal => SupervisorError::Internal(w.message),
        }
    }
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SupervisorError::Setting {
                setting,
                problem,
                fix,
                ..
            } => write!(f, "{setting} {problem}. {fix}."),
            SupervisorError::BadRequest(m) => write!(f, "{m}"),
            SupervisorError::NotAllowed(m) => write!(f, "{m}"),
            SupervisorError::SettingsRefused { revision, output } => write!(
                f,
                "revision {revision} was refused by the deployment's check-config; nothing \
                 running changed: {output}"
            ),
            SupervisorError::RolledBack { revision, to, why } => write!(
                f,
                "revision {revision} did not come up ({why}); revision {to} runs again"
            ),
            SupervisorError::Unavailable { what, .. } => write!(f, "{what}"),
            SupervisorError::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SupervisorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SupervisorError::Setting { cause, .. } | SupervisorError::Unavailable { cause, .. } => {
                cause
                    .as_deref()
                    .map(|c| c as &(dyn std::error::Error + 'static))
            }
            _ => None,
        }
    }
}

/// The error and its causes, one line each, for a log or a refusal.
pub fn report(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cause = e.source();
    while let Some(c) = cause {
        out.push_str(&format!("\n  caused by: {c}"));
        cause = c.source();
    }
    out
}
