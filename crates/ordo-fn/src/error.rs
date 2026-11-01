//! The one typed error a function call ends in (AGENTS.md rule 9).
//!
//! Every way a call can fail is a variant here. Each says its outcome class
//! (the word `$fnruns` records and the door answers with) and what a caller
//! can do about it. The HTTP answer is made in one place, at the door.

use std::time::Duration;

/// Which limit a call ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Wall time, network waits included.
    Wall,
    /// CPU, counted as fuel. Waiting on the network does not count.
    Cpu,
}

/// What a caller can do about a failed call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// Try again later; nothing about the request was wrong.
    Retry,
    /// The function or the deployment refused; the same request will be
    /// refused again until the program or a setting changes.
    Refuse,
    /// The input was wrong; fix it and send again.
    FixInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FnError {
    /// The function threw. The message and stack are kept, scrubbed.
    Threw { message: String, stack: String },
    /// A limit stopped the call.
    Timeout { limit: Limit, after: Duration },
    /// The call wanted more memory than its limit.
    OutOfMemory { limit_bytes: usize },
    /// An outbound call was refused or failed, and the function let it end
    /// the call. `code` is the egress outcome class.
    Fetch { code: String, message: String },
    /// `ctx.blob` refused a file, and the function let it end the call.
    Blob(crate::blob::BlobError),
    /// Too many calls are running for this tenant or this process.
    Busy { scope: BusyScope },
    /// The input could not be read as JSON, or was over the size limit.
    BadInput { reason: String },
    /// The function returned something that is not JSON, or too much of it.
    BadOutput { reason: String },
    /// The host itself failed: the engine did not compile or trapped in a way
    /// no limit explains. Not the function's fault.
    Engine { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyScope {
    Tenant,
    Process,
}

impl FnError {
    /// The outcome class, as plan §10 names them.
    pub fn code(&self) -> &str {
        match self {
            FnError::Threw { .. } | FnError::BadOutput { .. } => "function_error",
            FnError::Timeout { .. } => "function_timeout",
            FnError::OutOfMemory { .. } => "function_oom",
            FnError::Fetch { code, .. } => code,
            FnError::Blob(e) => e.kind(),
            FnError::Busy { .. } => "function_busy",
            FnError::BadInput { .. } => "function_bad_input",
            FnError::Engine { .. } => "function_engine",
        }
    }

    pub fn remedy(&self) -> Remedy {
        match self {
            FnError::Busy { .. } | FnError::Engine { .. } => Remedy::Retry,
            FnError::Fetch { code, .. }
                if code == "timed_out" || code == "secret_store_unavailable" =>
            {
                Remedy::Retry
            }
            FnError::BadInput { .. } => Remedy::FixInput,
            FnError::Blob(e) => e.remedy(),
            _ => Remedy::Refuse,
        }
    }
}

impl std::fmt::Display for FnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FnError::Threw { message, .. } => write!(f, "the function threw: {message}"),
            FnError::Timeout {
                limit: Limit::Wall,
                after,
            } => write!(
                f,
                "the call ran past its wall time of {} ms",
                after.as_millis()
            ),
            FnError::Timeout {
                limit: Limit::Cpu,
                after,
            } => write!(f, "the call used its CPU limit of {} ms", after.as_millis()),
            FnError::OutOfMemory { limit_bytes } => {
                write!(
                    f,
                    "the call needed more than its memory limit of {limit_bytes} bytes"
                )
            }
            FnError::Fetch { code, message } => write!(f, "{code}: {message}"),
            FnError::Blob(e) => write!(f, "{}: {e}", e.kind()),
            FnError::Busy {
                scope: BusyScope::Tenant,
            } => f.write_str("too many calls are running for this tenant; try again shortly"),
            FnError::Busy {
                scope: BusyScope::Process,
            } => f.write_str("too many calls are running on this server; try again shortly"),
            FnError::BadInput { reason } => write!(f, "the input was refused: {reason}"),
            FnError::BadOutput { reason } => {
                write!(f, "the function's answer was refused: {reason}")
            }
            FnError::Engine { message } => write!(f, "the function host failed: {message}"),
        }
    }
}

impl std::error::Error for FnError {}
