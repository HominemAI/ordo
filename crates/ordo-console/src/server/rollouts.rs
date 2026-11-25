//! The Rollouts and Moves screens' routes (`plan-versions.md` §3.7 and
//! §2.1, increments 7, 10, 10c and 11a).
//!
//! A rollout moves tenants to a stored version in batches. A move is one
//! tenant still on its version while its database is brought to the
//! active one; a move with dual writes ends in a soak. Every call reaches
//! the deployment through the console's one client to the door, in its
//! home space:
//!
//! | Route | Door op | Needs |
//! |---|---|---|
//! | `GET rollouts` | `std.deployment.rollout.status` | signed in |
//! | `POST rollouts/start` | `std.deployment.rollout.start` | the console's touch; no touch at the deployment |
//! | `POST rollouts/resume` | `std.deployment.rollout.resume` | the same |
//! | `POST rollouts/skip` | `std.deployment.rollout.skip` | the same |
//! | `POST rollouts/retry` | `std.deployment.rollout.retry` | the same |
//! | `POST rollouts/abort` | `std.deployment.rollout.abort` | the same |
//! | `POST rollouts/finish` | `std.deployment.rollout.finish` | the same |
//! | `POST rollouts/hold` | `std.deployment.rollout.hold` | the same |
//! | `GET rollouts/check?version=` | `std.deployment.rollbackCheck` | signed in |
//! | `POST moves/close-soak` | `std.deployment.closeSoak` | both touches |
//! | `POST moves/revert-soak` | `std.deployment.revertSoak` | both touches |
//!
//! A rollout's controls are moves, and a move asks no touch at the
//! deployment (owner, 2026-11-05). The moves themselves are read from
//! `std.deployment.tenants` (`move`), on the Tenants route.
//!
//! The console checks only the shape of a body. What a rollout, a hold or
//! a soak may be is the door's to say, and the screen shows its refusal as
//! given: each one names the setting or command that fixes it.

use std::collections::HashMap;

use axum::extract::Query;
use axum::routing::{get, post};
use axum::Router;

use super::*;

/// `std.deployment.rollout.status`.
pub const STATUS: &str = "std.deployment.rollout.status";
/// `std.deployment.rollout.start`.
pub const START: &str = "std.deployment.rollout.start";
/// `std.deployment.rollout.resume`.
pub const RESUME: &str = "std.deployment.rollout.resume";
/// `std.deployment.rollout.skip`.
pub const SKIP: &str = "std.deployment.rollout.skip";
/// `std.deployment.rollout.retry`.
pub const RETRY: &str = "std.deployment.rollout.retry";
/// `std.deployment.rollout.abort`.
pub const ABORT: &str = "std.deployment.rollout.abort";
/// `std.deployment.rollout.finish`.
pub const FINISH: &str = "std.deployment.rollout.finish";
/// `std.deployment.rollout.hold`.
pub const HOLD: &str = "std.deployment.rollout.hold";
/// `std.deployment.rollbackCheck`.
pub const ROLLBACK_CHECK: &str = "std.deployment.rollbackCheck";
/// `std.deployment.closeSoak`.
pub const CLOSE_SOAK: &str = "std.deployment.closeSoak";
/// `std.deployment.revertSoak`.
pub const REVERT_SOAK: &str = "std.deployment.revertSoak";

/// The longest reason the console passes on, the door's own bound.
const MAX_REASON_CHARS: usize = 4096;

pub(super) fn routes() -> Router<Console> {
    Router::new()
        .route("/d/{handle}/api/rollouts", get(status_route))
        .route("/d/{handle}/api/rollouts/start", post(start_route))
        .route("/d/{handle}/api/rollouts/resume", post(resume_route))
        .route("/d/{handle}/api/rollouts/skip", post(skip_route))
        .route("/d/{handle}/api/rollouts/retry", post(retry_route))
        .route("/d/{handle}/api/rollouts/abort", post(abort_route))
        .route("/d/{handle}/api/rollouts/finish", post(finish_route))
        .route("/d/{handle}/api/rollouts/hold", post(hold_route))
        .route("/d/{handle}/api/rollouts/check", get(check_route))
        .route("/d/{handle}/api/moves/close-soak", post(close_soak_route))
        .route("/d/{handle}/api/moves/revert-soak", post(revert_soak_route))
}

read!(status_route, rollout_status);
action!(start_route, "rollouts.start", rollout_start);
action!(resume_route, "rollouts.resume", rollout_resume);
action!(skip_route, "rollouts.skip", rollout_skip);
action!(retry_route, "rollouts.retry", rollout_retry);
action!(abort_route, "rollouts.abort", rollout_abort);
action!(finish_route, "rollouts.finish", rollout_finish);
action!(hold_route, "rollouts.hold", rollout_hold);
action!(close_soak_route, "moves.close_soak", close_soak);
action!(revert_soak_route, "moves.revert_soak", revert_soak);

fn first(rows: Vec<Value>) -> Value {
    rows.into_iter().next().unwrap_or(Value::Null)
}

/// A text field trimmed, `None` when blank.
fn given(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// A tenant id as the door takes one.
fn tenant_of(t: &str) -> Result<String, ConsoleError> {
    let t = t.trim();
    if t.is_empty() || t.len() > 128 || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ConsoleError::BadRequest(
            "`tenant` must name a tenant, as the Tenants screen lists it".into(),
        ));
    }
    Ok(t.to_string())
}

/// A version as the door takes one: its name or `#n`.
fn version_of(v: &str) -> Result<String, ConsoleError> {
    let v = v.trim();
    if v.is_empty() || v.len() > 128 || v.chars().any(char::is_control) {
        return Err(ConsoleError::BadRequest(
            "`version` must name a stored version, as the Deploys screen lists it".into(),
        ));
    }
    Ok(v.to_string())
}

/// A reason, passed on as typed, within the door's bound.
fn reason_of(r: &Option<String>) -> Result<Option<String>, ConsoleError> {
    match given(r) {
        Some(r) if r.chars().count() > MAX_REASON_CHARS => Err(ConsoleError::BadRequest(format!(
            "a reason is {MAX_REASON_CHARS} characters at most"
        ))),
        other => Ok(other.map(str::to_string)),
    }
}

/// The open rollout, or the newest, with its tenants and health.
async fn rollout_status(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who.door(c)?.op(STATUS, true, &json!({})).await?;
    Ok(Reply::json(first(rows)))
}

/// `GET rollouts/check?version=`: a read with a query, so it is spelled
/// out rather than made with `read!`.
async fn check_route(
    State(c): State<Console>,
    Path(handle): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let mut call = Call {
        deployment: handle,
        actor: "anonymous".into(),
        detail: String::new(),
    };
    let result = match c.admit(&call, &headers, false) {
        Ok(()) => rollback_check(&c, &mut call, &headers, &query).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(r) => r.into_response(),
        Err(e) => e.into_response(),
    }
}

/// What a rollback to `version` would find, changing nothing: the
/// externals and peers it reads, and what it does to each tenant.
async fn rollback_check(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let version = version_of(query.get("version").map(String::as_str).unwrap_or(""))?;
    let rows = who
        .door(c)?
        .op(ROLLBACK_CHECK, true, &json!({ "version": version }))
        .await?;
    Ok(Reply::json(first(rows)))
}

#[derive(Deserialize)]
struct StartBody {
    version: String,
    #[serde(default)]
    strategy: Option<String>,
    #[serde(default, rename = "batchInterval")]
    batch_interval: Option<String>,
    #[serde(default, rename = "batchSize")]
    batch_size: Option<i64>,
    #[serde(default)]
    window: Option<String>,
    #[serde(default, rename = "failureThreshold")]
    failure_threshold: Option<i64>,
    #[serde(default, rename = "canaryCount")]
    canary_count: Option<i64>,
    #[serde(default, rename = "canaryWait")]
    canary_wait: Option<String>,
    #[serde(default, rename = "healthErrorRate")]
    health_error_rate: Option<String>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// Start a rollout of a stored version. Each option left out takes the
/// deployment's rollout setting.
async fn rollout_start(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: StartBody = parse(body)?;
    let version = version_of(&req.version)?;
    let mut params = json!({ "version": version });
    for (k, v) in [
        ("strategy", given(&req.strategy)),
        ("batchInterval", given(&req.batch_interval)),
        ("window", given(&req.window)),
        ("canaryWait", given(&req.canary_wait)),
        ("healthErrorRate", given(&req.health_error_rate)),
        ("group", given(&req.group)),
        ("query", given(&req.query)),
        ("order", given(&req.order)),
    ] {
        if let Some(v) = v {
            params[k] = json!(v);
        }
    }
    for (k, v) in [
        ("batchSize", req.batch_size),
        ("failureThreshold", req.failure_threshold),
        ("canaryCount", req.canary_count),
        ("limit", req.limit),
    ] {
        if let Some(v) = v {
            params[k] = json!(v);
        }
    }
    call.detail = format!(
        "version={version} strategy={}",
        given(&req.strategy).unwrap_or("setting")
    );
    let row = first(who.door(c)?.op_present(START, &params).await?);
    call.detail = format!(
        "rollout={} version={version} strategy={}",
        row["id"],
        row["strategy"].as_str().unwrap_or("?")
    );
    Ok(Reply::json(row))
}

/// Resume a paused rollout.
async fn rollout_resume(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    _body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let row = first(who.door(c)?.op_present(RESUME, &json!({})).await?);
    call.detail = format!("rollout={}", row["id"]);
    Ok(Reply::json(row))
}

#[derive(Deserialize)]
struct TenantBody {
    tenant: String,
    #[serde(default)]
    reason: Option<String>,
    /// A hold: lift it.
    #[serde(default)]
    clear: bool,
}

/// Skip one tenant: it stays where it is.
async fn rollout_skip(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: TenantBody = parse(body)?;
    let tenant = tenant_of(&req.tenant)?;
    let mut params = json!({ "tenant": tenant });
    if let Some(r) = reason_of(&req.reason)? {
        params["reason"] = json!(r);
    }
    call.detail = format!("tenant={tenant}");
    let rows = who.door(c)?.op_present(SKIP, &params).await?;
    Ok(Reply::json(first(rows)))
}

/// Try one failed tenant again, or one tenant's refused move.
async fn rollout_retry(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let tenant = tenant_of(&parse::<TenantBody>(body)?.tenant)?;
    call.detail = format!("tenant={tenant}");
    let rows = who
        .door(c)?
        .op_present(RETRY, &json!({ "tenant": tenant }))
        .await?;
    Ok(Reply::json(first(rows)))
}

#[derive(Deserialize)]
struct ReasonBody {
    #[serde(default)]
    reason: Option<String>,
}

/// Stop the open rollout. Moved tenants stay; the door's refusal says why
/// a reason is needed when none is given.
async fn rollout_abort(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let reason = reason_of(&parse::<ReasonBody>(body)?.reason)?;
    let row = first(
        who.door(c)?
            .op_present(ABORT, &json!({ "reason": reason.unwrap_or_default() }))
            .await?,
    );
    call.detail = format!("rollout={}", row["id"]);
    Ok(Reply::json(row))
}

/// Close the open rollout by hand, the rest skipped.
async fn rollout_finish(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let mut params = json!({});
    if let Some(r) = reason_of(&parse::<ReasonBody>(body)?.reason)? {
        params["reason"] = json!(r);
    }
    let row = first(who.door(c)?.op_present(FINISH, &params).await?);
    call.detail = format!("rollout={}", row["id"]);
    Ok(Reply::json(row))
}

/// Hold one tenant's moves with a reason, or lift the hold.
async fn rollout_hold(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: TenantBody = parse(body)?;
    let tenant = tenant_of(&req.tenant)?;
    let mut params = json!({ "tenant": tenant });
    if req.clear {
        params["clear"] = json!(true);
    } else if let Some(r) = reason_of(&req.reason)? {
        params["reason"] = json!(r);
    }
    call.detail = format!("tenant={tenant} clear={}", req.clear);
    let rows = who.door(c)?.op_present(HOLD, &params).await?;
    Ok(Reply::json(first(rows)))
}

/// End one tenant's soak now. Its old columns are orphaned.
async fn close_soak(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let tenant = tenant_of(&parse::<TenantBody>(body)?.tenant)?;
    call.detail = format!("tenant={tenant}");
    let rows = who
        .door(c)?
        .op_present(CLOSE_SOAK, &json!({ "tenant": tenant }))
        .await?;
    Ok(Reply::json(first(rows)))
}

/// Put one tenant in its soak back on the version it moved from, every
/// write kept.
async fn revert_soak(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let tenant = tenant_of(&parse::<TenantBody>(body)?.tenant)?;
    call.detail = format!("tenant={tenant}");
    let rows = who
        .door(c)?
        .op_present(REVERT_SOAK, &json!({ "tenant": tenant }))
        .await?;
    Ok(Reply::json(first(rows)))
}
