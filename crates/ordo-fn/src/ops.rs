//! Ops from functions (`plan-backend-functions.md` increment 4):
//! `ctx.op(name, args)`.
//!
//! A function calls the program's own ops through [`FnOps`], which the
//! caller of [`crate::FnHost::call`] hands in already bound to the person
//! calling. The host behind it runs each op as that person, by a delegation
//! that lasts this one call (ruled 2026-10-08, ask 4). So the function can
//! never do what the person cannot, and every write it makes lands in the
//! chain as `function:<name>@<version>` on behalf of the person.
//!
//! The function host checks two things of its own before the seam is
//! asked: the op is in the function's `uses`, and the call has not made
//! more than [`crate::Limits::max_ops`] op calls. The seam checks `uses`
//! again.
//!
//! **Sealed fields.** An op's answer marks which of its fields are sealed
//! ([`FnOpAnswer::sealed`]). Their values never enter the instance: the
//! function sees a handle, and `ctx.sealed(handle)` puts the value in a
//! request header to a host on its `egress` list, as a secret is.

use serde_json::{Map, Value};

/// Op calls one function call may make, by default.
pub const DEFAULT_MAX_OPS: u32 = 50;

/// What one op answered.
#[derive(Debug, Clone, PartialEq)]
pub struct FnOpAnswer {
    /// Its rows: a read's first page, or a write's summary.
    pub rows: Vec<Value>,
    /// The fields in `rows` that are sealed. Their values are held by the
    /// host and reach the function only as handles.
    pub sealed: Vec<String>,
}

/// Why an op call was refused. The one typed error of `ctx.op`
/// (AGENTS.md rule 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    /// The function's `uses` does not list the op.
    NotUsed { op: String },
    /// The call has made its limit of op calls.
    TooMany { limit: u32 },
    /// The one-call delegation is over: the function's call has ended.
    Ended,
    /// The person calling may not call this op themselves.
    Denied { op: String },
    /// The op deletes rows, and deletes are closed to delegated callers
    /// unless the app developer opens this one with `setting`.
    NotDelegable { op: String, setting: &'static str },
    /// The op ran and refused, with the door's own class and words.
    Refused {
        op: String,
        code: String,
        message: String,
        /// The same call may land later (busy, a conflict worth re-reading).
        retry: bool,
    },
}

impl OpError {
    /// The reason word the function sees as `e.code`, and the run's outcome
    /// class when the function lets it end the call. An op's own refusal
    /// keeps the door's class (`conflict`, `invalid_param`, ...).
    pub fn kind(&self) -> &str {
        match self {
            OpError::NotUsed { .. } => "op_not_in_uses",
            OpError::TooMany { .. } => "op_limit",
            OpError::Ended => "delegation_ended",
            OpError::Denied { .. } => "op_access_denied",
            OpError::NotDelegable { .. } => "op_not_delegable",
            OpError::Refused { code, .. } => code,
        }
    }

    pub fn remedy(&self) -> crate::Remedy {
        match self {
            OpError::Refused { retry: true, .. } => crate::Remedy::Retry,
            OpError::Refused { code, .. }
                if code == "invalid_param" || code == "missing_param" || code == "not_found" =>
            {
                crate::Remedy::FixInput
            }
            _ => crate::Remedy::Refuse,
        }
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::NotUsed { op } => write!(
                f,
                "this function does not list op `{op}`; add it to the function's `uses` line"
            ),
            OpError::TooMany { limit } => write!(f, "more than {limit} op calls in one call"),
            OpError::Ended => f.write_str(
                "the call this delegation was made for has ended; an op runs only while its \
                 function's call does",
            ),
            OpError::Denied { op } => write!(
                f,
                "the person calling may not call op `{op}`, so this function may not call it \
                 for them"
            ),
            OpError::NotDelegable { op, setting } => write!(
                f,
                "op `{op}` deletes rows, and a delete is closed to a caller acting for a \
                 person; the app developer opens it with {setting}={op}"
            ),
            OpError::Refused {
                op, code, message, ..
            } => write!(f, "op `{op}` refused: {code}: {message}"),
        }
    }
}

impl std::error::Error for OpError {}

/// Calls ops for one function call, as the person calling. Made per call
/// by the caller of [`crate::FnHost::call`], already bound to that person,
/// their tenant and the function's `uses`.
#[async_trait::async_trait]
pub trait FnOps: Send + Sync {
    /// Run op `op` with `args`, its declared parameters by name.
    async fn call(&self, op: &str, args: Map<String, Value>) -> Result<FnOpAnswer, OpError>;
}

/// No ops: every call is [`OpError::NotUsed`]. For a function with no
/// `uses`, and for hosts with no ops to offer.
pub struct NoOps;

#[async_trait::async_trait]
impl FnOps for NoOps {
    async fn call(&self, op: &str, _args: Map<String, Value>) -> Result<FnOpAnswer, OpError> {
        Err(OpError::NotUsed { op: op.to_string() })
    }
}
