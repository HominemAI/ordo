//! The supervisor's plain data: its delays, a child's state, how it ended,
//! and what an apply did. The console's screens read these words over the
//! socket, so they keep the names the console used.

use std::process::ExitStatus;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Every delay the supervisor keeps. [`Timings::plan`] holds the plan's
/// numbers; tests shorten them.
#[derive(Debug, Clone)]
pub struct Timings {
    pub grace: Duration,
    pub ready_timeout: Duration,
    pub ready_poll: Duration,
    pub backoff_first: Duration,
    pub backoff_max: Duration,
    pub backoff_reset: Duration,
    pub loop_window: Duration,
    pub loop_exits: usize,
    pub loop_retry: Duration,
    pub check_timeout: Duration,
}

impl Timings {
    /// `plan-console.md` 6.1 and 6.2, applied to each child (3.1), with the
    /// grace and ready limits from the settings.
    pub fn plan(grace: Duration, ready_timeout: Duration) -> Timings {
        Timings {
            grace,
            ready_timeout,
            ready_poll: Duration::from_millis(500),
            backoff_first: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            backoff_reset: Duration::from_secs(600),
            loop_window: Duration::from_secs(600),
            loop_exits: 5,
            loop_retry: Duration::from_secs(60),
            check_timeout: Duration::from_secs(30),
        }
    }
}

/// Where a child is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Started, not ready yet.
    Starting,
    /// Answering `/health/ready`.
    Running,
    /// Exited; starting again after the backoff.
    Restarting,
    /// 5 exits in the window; retrying slowly.
    CrashLooping,
    /// The supervisor is shutting down.
    Stopped,
}

/// A child's state in the words the screens use: [`Phase`], with a crash
/// loop that stopped automatic changes shown as `held`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChildState {
    Starting,
    Ready,
    Restarting,
    CrashLooping,
    /// Crash-looping after an automatic rollback: no more automatic
    /// changes until the owner applies or restarts.
    Held,
    Stopped,
}

impl ChildState {
    pub fn of(phase: Phase, held: bool) -> ChildState {
        match phase {
            Phase::CrashLooping if held => ChildState::Held,
            Phase::Starting => ChildState::Starting,
            Phase::Running => ChildState::Ready,
            Phase::Restarting => ChildState::Restarting,
            Phase::CrashLooping => ChildState::CrashLooping,
            Phase::Stopped => ChildState::Stopped,
        }
    }
}

/// What `status` answers for one child.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub child: String,
    /// Filled when read.
    pub state: ChildState,
    pub phase: Phase,
    pub pid: Option<u32>,
    /// The deployment's revision the running process was started with.
    pub revision: Option<u64>,
    /// The deployment's live revision. 0 for the other children.
    #[serde(rename = "liveRevision")]
    pub live: u64,
    pub restarts: u64,
    #[serde(rename = "lastExit")]
    pub last_exit: Option<ExitInfo>,
    #[serde(rename = "readySince")]
    pub ready_since: Option<u64>,
    /// A second crash loop after an apply stopped automatic changes.
    #[serde(rename = "autoChangesHeld")]
    pub held: bool,
    /// The revision an apply is bringing up.
    pub applying: Option<u64>,
    /// Whether the child runs as its own unix user, and what to know when
    /// it does not.
    pub privilege: Privilege,
}

/// Whether a child runs as its own unix user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Privilege {
    #[serde(rename = "separateUser")]
    pub separate: bool,
    pub uid: Option<u32>,
    /// What the deployment owner should know when it does not.
    pub note: Option<String>,
}

/// What a successful apply did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Applied {
    pub revision: u64,
    /// Names only: `added=A changed=B removed=C secrets=D`.
    pub diff: String,
}

/// How a child ended.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    /// Set when the supervisor could not start it at all.
    pub error: Option<String>,
    pub at: u64,
}

impl ExitInfo {
    pub fn from_status(s: &ExitStatus, at: u64) -> ExitInfo {
        #[cfg(unix)]
        let signal = std::os::unix::process::ExitStatusExt::signal(s);
        #[cfg(not(unix))]
        let signal = None;
        ExitInfo {
            code: s.code(),
            signal,
            error: None,
            at,
        }
    }

    pub fn failed(error: String, at: u64) -> ExitInfo {
        ExitInfo {
            code: None,
            signal: None,
            error: Some(error),
            at,
        }
    }

    pub fn describe(&self) -> String {
        match (&self.error, self.code, self.signal) {
            (Some(e), _, _) => format!("could not start: {e}"),
            (None, Some(c), _) => format!("exited with code {c}"),
            (None, None, Some(s)) => format!("killed by signal {s}"),
            _ => "exited".to_string(),
        }
    }
}
