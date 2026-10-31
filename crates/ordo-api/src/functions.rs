//! `POST /fn/<name>`: a backend function called from the page
//! (`plan-backend-functions.md` §7, increment 1).
//!
//! A function goes through the same door as an op: the token gate, the
//! access check, the quota gate, the space binding, typed parameters and the
//! `Idempotency-Key`. Then the door hands the call to a [`FunctionRunner`],
//! which the host implements with the wasmtime function host. The door does
//! not depend on wasmtime.
//!
//! The runner answers JSON or an [`ExecError`], the same typed error an op's
//! executor answers. [`crate::ApiError::from_exec`] turns it into the HTTP
//! answer, so a function's refusals are mapped in the one place an op's are
//! (AGENTS.md rule 9).

use futures::future::BoxFuture;
use serde_json::{Map, Value};

use crate::spec::{ExecError, OpSpec};

/// One call, after the door let it through.
#[derive(Debug, Clone)]
pub struct FnInvocation {
    pub name: String,
    /// The tenant the call runs in, as the access check approved it.
    pub space: String,
    /// Who is calling: the signed-in person. The function runs as them, by
    /// a delegation that lasts this one call (ruled 2026-10-08, ask 4).
    pub principal: String,
    /// The principal presenting the token when it acts for `principal`
    /// under a delegation, else `None`.
    pub agent: Option<String>,
    /// The arguments, bound and typed against the declaration.
    pub args: Map<String, Value>,
    /// The `Idempotency-Key`, when the page sent one. Stable across retries,
    /// so the function may use it as its own key outside.
    pub idempotency_key: Option<String>,
}

/// The program's functions, as the door sees them.
pub trait FunctionRunner: Send + Sync {
    /// The door's view of function `name` in the serving program: an
    /// [`OpSpec`] of kind `Write` (so `POST`), with its typed parameters.
    /// `None` when the program declares no such function; the door then
    /// answers as it does for an unknown op.
    fn spec(&self, name: &str) -> Option<OpSpec>;

    /// Runs the call. The pause check, the run and its `$fnruns` row are
    /// the runner's.
    fn call<'a>(&'a self, call: FnInvocation) -> BoxFuture<'a, Result<Value, ExecError>>;
}

/// A door with no functions: every name is unknown.
pub struct NoFunctions;

impl FunctionRunner for NoFunctions {
    fn spec(&self, _name: &str) -> Option<OpSpec> {
        None
    }

    fn call<'a>(&'a self, call: FnInvocation) -> BoxFuture<'a, Result<Value, ExecError>> {
        Box::pin(async move {
            Err(ExecError::internal(format!(
                "no function host: `{}` cannot run",
                call.name
            )))
        })
    }
}
