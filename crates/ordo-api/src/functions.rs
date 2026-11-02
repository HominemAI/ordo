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
//!
//! **Files in** (increment 2). A `$file: blob` parameter carries a handle
//! the page got from the blob door. Before the runner is asked, the door
//! checks that the caller may read files at all (`std.$blob.get`, the
//! check `GET /blob/{handle}` makes), and refuses as that route would. The
//! runner then reads each file as the caller, so the row guard decides,
//! exactly as it does on `GET /blob/{handle}`.

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
    /// The op door bound to this caller, for `ctx.op` (increment 4).
    /// `None` when the call came with no token to bind it to.
    pub door: Option<CallerDoor>,
}

/// The op door, bound to one function call's caller: what a function's
/// `ctx.op` runs through (`plan-backend-functions.md` increment 4).
///
/// Each op meets the steps an op call from the page meets, for the same
/// token in the same tenant: the access check, the email requirement, the
/// quota gate, typed parameters, then the executor with `$caller` set to
/// the person. So a function can never do what the person cannot. The
/// page's call also passes the token gate and binds the space; the
/// function's own call already did both.
///
/// The token stays in here. It is never shown, and it never reaches the
/// function's instance.
#[derive(Clone)]
pub struct CallerDoor {
    state: crate::ApiState,
    token: String,
    space: String,
    principal: String,
}

impl std::fmt::Debug for CallerDoor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallerDoor")
            .field("space", &self.space)
            .field("principal", &self.principal)
            .finish_non_exhaustive()
    }
}

impl CallerDoor {
    /// `token` is the caller's own, which the door already resolved to
    /// `principal` in `space`.
    pub fn new(
        state: crate::ApiState,
        token: String,
        space: String,
        principal: String,
    ) -> CallerDoor {
        CallerDoor {
            state,
            token,
            space,
            principal,
        }
    }

    /// The serving program's op `name`, as the door sees it.
    pub fn spec(&self, name: &str) -> Option<OpSpec> {
        self.state.registry.current().get(name)
    }

    /// Runs `op` with `args` as the caller, recorded as `acting` on behalf
    /// of them: the command group names
    /// `agent:<acting> on behalf of user:<caller>`. The answer is the op's
    /// rows: a read's first page, a write's summary.
    ///
    /// A caller who may not call `op` is refused `access_denied`, as at the
    /// door. Every other refusal is the door's or the executor's own.
    pub async fn run(
        &self,
        op: &OpSpec,
        args: &Map<String, Value>,
        acting: &str,
    ) -> Result<Vec<Value>, crate::ApiError> {
        use crate::spec::{AccessDecision, ExecOutcome};
        use crate::{ApiError, ErrorCode};
        match self.state.access.check(&self.token, &self.space, &op.name) {
            AccessDecision::Allow { principal, .. } if principal == self.principal => {}
            AccessDecision::Allow { .. } => {
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    "the token answered for another person than the one the function runs as",
                )
                .with_op(&op.name))
            }
            AccessDecision::Deny => {
                return Err(ApiError::new(
                    ErrorCode::AccessDenied,
                    "the person calling may not call this op",
                )
                .with_op(&op.name))
            }
        }
        if let Some(held) = self
            .state
            .access
            .email_required(&self.token, &self.principal, &op.name)
            .await
        {
            return Err(if held.retryable {
                ApiError::email_unreadable(&op.name, &held.message)
            } else {
                ApiError::email_required(&op.name, &held.message)
            });
        }
        crate::router::quota_gate(&self.state, op, &self.space)?;
        if op.requires_idempotency_key {
            return Err(ApiError::for_op(
                ErrorCode::MissingParam,
                op,
                "this op requires an `Idempotency-Key`, and an op called from a function \
                 carries none yet. Nothing ran. Call it from the page instead",
            ));
        }
        let mut bound = crate::bind::bind_params(op, args)?;
        bound
            .0
            .insert("$caller".into(), Value::String(self.principal.clone()));
        bound.0.insert(
            crate::router::AGENT_PARAM.into(),
            Value::String(acting.to_string()),
        );
        match self
            .state
            .executor
            .execute(op, &self.space, bound, None, None, None)
            .await
        {
            Ok(ExecOutcome::Ran(r)) => Ok(r.rows),
            // No lease was sent, so nothing was already applied.
            Ok(ExecOutcome::AlreadyApplied) => Ok(Vec::new()),
            Err(e) => Err(ApiError::from_exec(op, e)),
        }
    }
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

    /// The largest body `POST /fn/<name>` reads, in bytes. A larger one is
    /// refused before it is read, naming [`FN_BODY_BYTES_ENV`].
    fn body_limit(&self) -> usize {
        crate::router::FN_BODY_LIMIT
    }
}

/// The setting that sets [`FunctionRunner::body_limit`]. Named here because
/// the door's refusal names it.
pub const FN_BODY_BYTES_ENV: &str = "ORDO_FN_BODY_BYTES";

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
