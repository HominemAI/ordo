//! The Deploys screen's routes (`plan-console.md` 4.3, step 4): the app's
//! program at the deployment, through the door and nothing else.
//!
//! | Route | Door call | Needs |
//! |---|---|---|
//! | `GET deploys` | `std.deployment.versions` (G1) | signed in |
//! | `GET deploys/serving` | `GET /health/deploy` | signed in |
//! | `POST deploys/preview` | `std.deployment.plan` (G3) | the owner's touch at the deployment |
//! | `POST deploys/apply` | `std.deployment.apply` | both touches |
//! | `POST deploys/activate` | `std.deployment.activate` (G2) | both touches |
//!
//! A change asks for the console owner's own touch, as every console write
//! does, and for the owner's touch at the deployment, relayed (G15),
//! because the deployment never lets a console key stand in for a person.
//! A preview stores nothing, but it is a `POST` at the door (a program is
//! up to a megabyte) and it reads live rows, so the deployment asks for the
//! same touch; one covers the apply that follows within 300 seconds.
//!
//! Each change is audited here, and recorded at the deployment too: the
//! registry rows name the owner the door proved, and the deployment's
//! security record gets a `deploy` event (G12).

use axum::routing::{get, post};
use axum::Router;

use super::*;

/// `std.deployment.versions`.
pub const VERSIONS: &str = "std.deployment.versions";
/// `std.deployment.activate`.
pub const ACTIVATE: &str = "std.deployment.activate";
/// `std.deployment.plan`.
pub const PLAN: &str = "std.deployment.plan";
/// `std.deployment.apply`.
pub const APPLY: &str = "std.deployment.apply";

/// How many activations the screen lists.
pub const HISTORY_LIMIT: u32 = 50;
/// The largest program the console passes on, the door's own bound.
pub const MAX_PROGRAM_BYTES: usize = 1024 * 1024;

pub(super) fn routes() -> Router<Console> {
    Router::new()
        .route("/d/{handle}/api/deploys", get(deploys_route))
        .route("/d/{handle}/api/deploys/serving", get(serving_route))
        .route("/d/{handle}/api/deploys/preview", post(preview_route))
        .route("/d/{handle}/api/deploys/apply", post(apply_program_route))
        .route("/d/{handle}/api/deploys/activate", post(activate_route))
}

read!(deploys_route, deploys_get);
read!(serving_route, serving_get);
action!(preview_route, "deploys.preview", deploys_preview);
action!(apply_program_route, "deploys.apply", deploys_apply);
action!(activate_route, "deploys.activate", deploys_activate);

#[derive(Deserialize)]
struct ProgramBody {
    program: String,
    /// Absent is `true`, as `ordo apply` activates unless told not to.
    #[serde(default = "yes")]
    activate: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct VersionBody {
    version: String,
}

fn program(req: &ProgramBody) -> Result<&str, ConsoleError> {
    if req.program.trim().is_empty() {
        return Err(ConsoleError::BadRequest(
            "`program` is empty: choose a .ordo file".into(),
        ));
    }
    if req.program.len() > MAX_PROGRAM_BYTES {
        return Err(ConsoleError::BadRequest(format!(
            "the program is {} bytes; the deployment takes at most {MAX_PROGRAM_BYTES}",
            req.program.len()
        )));
    }
    Ok(&req.program)
}

fn first(rows: Vec<Value>) -> Value {
    rows.into_iter().next().unwrap_or(Value::Null)
}

/// The stored versions, the serving one and the activation log.
async fn deploys_get(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who
        .door(c)?
        .op(VERSIONS, true, &json!({ "limit": HISTORY_LIMIT }))
        .await?;
    Ok(Reply::json(first(rows)))
}

/// Which version each node serves (`GET /health/deploy`), for the wait
/// after an apply or a rollback.
async fn serving_get(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    // The node detail is the deployment owner's; the link reads it as one.
    Ok(Reply::json(who.door(c)?.as_owner("/health/deploy").await?))
}

/// What an apply would do, with nothing stored (G3).
async fn deploys_preview(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let req: ProgramBody = parse(body)?;
    let rows = who
        .door(c)?
        .op_present_within(
            PLAN,
            &json!({ "program": program(&req)? }),
            crate::door::DEPLOY_CALL_TIMEOUT,
        )
        .await?;
    let row = first(rows);
    call.detail = format!(
        "version={} would={}",
        row["version"].as_str().unwrap_or("?"),
        row["would"]["state"].as_str().unwrap_or("?")
    );
    Ok(Reply::json(row))
}

/// Apply a program: store it, and activate it unless asked not to.
async fn deploys_apply(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: ProgramBody = parse(body)?;
    call.detail = format!("activate={}", req.activate);
    let rows = who
        .door(c)?
        .op_present_within(
            APPLY,
            &json!({ "program": program(&req)?, "activate": req.activate }),
            crate::door::DEPLOY_CALL_TIMEOUT,
        )
        .await?;
    let row = first(rows);
    call.detail = format!(
        "version={} activation={}",
        row["version"].as_str().unwrap_or("?"),
        row["activation"]["state"].as_str().unwrap_or("?")
    );
    Ok(Reply::json(row))
}

/// Roll the app back: make a stored version the one that serves (G2).
async fn deploys_activate(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: VersionBody = parse(body)?;
    let version = req.version.trim();
    if version.is_empty() || version.len() > 128 {
        return Err(ConsoleError::BadRequest(
            "`version` must name a stored version".into(),
        ));
    }
    call.detail = format!("version={version}");
    let rows = who
        .door(c)?
        .op_present_within(
            ACTIVATE,
            &json!({ "version": version }),
            crate::door::DEPLOY_CALL_TIMEOUT,
        )
        .await?;
    let row = first(rows);
    call.detail = format!(
        "version={version} state={} from={}",
        row["state"].as_str().unwrap_or("?"),
        row["from"].as_str().unwrap_or("none")
    );
    Ok(Reply::json(row))
}
