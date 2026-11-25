//! The Tenant builds screen's routes (`plan-versions.md` §3.8, increment
//! 16).
//!
//! A tenant build is a name that pins named tenants to its version, apart
//! from the active one: a beta tenant serves the branch while the rest
//! serve main. Activations, rollbacks, rollouts and the revoked sweep
//! leave its tenants alone; only these ops move them. Every call reaches
//! the deployment through the console's one client to the door, in its
//! home space:
//!
//! | Route | Door op | Needs |
//! |---|---|---|
//! | `GET builds` | `std.deployment.tenantBuilds` | signed in |
//! | `POST builds/set` | `std.deployment.setTenantBuild` | both touches |
//! | `POST builds/join` | `std.deployment.joinTenantBuild` | both touches |
//! | `POST builds/leave` | `std.deployment.leaveTenantBuild` | both touches |
//! | `POST builds/remove` | `std.deployment.removeTenantBuild` | both touches |
//!
//! The console checks only the shape of a body. What a build's name,
//! version and members may be is the door's to say, and the screen shows
//! its refusal as given: each one names the command that fixes it.

use axum::routing::{get, post};
use axum::Router;

use super::*;

/// `std.deployment.tenantBuilds`.
pub const BUILDS: &str = "std.deployment.tenantBuilds";
/// `std.deployment.setTenantBuild`.
pub const SET_BUILD: &str = "std.deployment.setTenantBuild";
/// `std.deployment.joinTenantBuild`.
pub const JOIN_BUILD: &str = "std.deployment.joinTenantBuild";
/// `std.deployment.leaveTenantBuild`.
pub const LEAVE_BUILD: &str = "std.deployment.leaveTenantBuild";
/// `std.deployment.removeTenantBuild`.
pub const REMOVE_BUILD: &str = "std.deployment.removeTenantBuild";

/// The door's own bounds.
const MAX_NAME: usize = 64;
const MAX_REASON: usize = 2048;

pub(super) fn routes() -> Router<Console> {
    Router::new()
        .route("/d/{handle}/api/builds", get(builds_route))
        .route("/d/{handle}/api/builds/set", post(set_route))
        .route("/d/{handle}/api/builds/join", post(join_route))
        .route("/d/{handle}/api/builds/leave", post(leave_route))
        .route("/d/{handle}/api/builds/remove", post(remove_route))
}

read!(builds_route, builds_list);
action!(set_route, "builds.set", builds_set);
action!(join_route, "builds.join", builds_join);
action!(leave_route, "builds.leave", builds_leave);
action!(remove_route, "builds.remove", builds_remove);

fn first(rows: Vec<Value>) -> Value {
    rows.into_iter().next().unwrap_or(Value::Null)
}

/// A field the door takes as text, trimmed, within its bound. The door
/// checks what it may say.
fn field(name: &str, v: &str, max: usize) -> Result<String, ConsoleError> {
    let v = v.trim();
    if v.chars().count() > max || v.chars().any(char::is_control) {
        return Err(ConsoleError::BadRequest(format!(
            "`{name}` is {max} characters at most, on one line"
        )));
    }
    Ok(v.to_string())
}

/// Every build, its version and the tenants it holds.
async fn builds_list(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who.door(c)?.op(BUILDS, true, &json!({})).await?;
    Ok(Reply::json(first(rows)))
}

#[derive(Deserialize)]
struct SetBody {
    name: String,
    version: String,
    reason: String,
}

/// Create a build, or set it to another version, moving its tenants.
async fn builds_set(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: SetBody = parse(body)?;
    let name = field("name", &req.name, MAX_NAME)?;
    let version = field("version", &req.version, 256)?;
    let reason = req.reason.trim();
    if reason.chars().count() > MAX_REASON {
        return Err(ConsoleError::BadRequest(format!(
            "a reason is {MAX_REASON} characters at most"
        )));
    }
    call.detail = format!("build={name} version={version}");
    let rows = who
        .door(c)?
        .op_present(
            SET_BUILD,
            &json!({ "name": name, "version": version, "reason": reason }),
        )
        .await?;
    Ok(Reply::json(first(rows)))
}

#[derive(Deserialize)]
struct MemberBody {
    #[serde(default)]
    name: Option<String>,
    tenant: String,
}

/// Put one tenant in a build.
async fn builds_join(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: MemberBody = parse(body)?;
    let name = field("name", req.name.as_deref().unwrap_or_default(), MAX_NAME)?;
    let tenant = field("tenant", &req.tenant, 128)?;
    call.detail = format!("build={name} tenant={tenant}");
    let rows = who
        .door(c)?
        .op_present(JOIN_BUILD, &json!({ "name": name, "tenant": tenant }))
        .await?;
    Ok(Reply::json(first(rows)))
}

/// Take one tenant out of its build, back to the active version.
async fn builds_leave(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let tenant = field("tenant", &parse::<MemberBody>(body)?.tenant, 128)?;
    call.detail = format!("tenant={tenant}");
    let rows = who
        .door(c)?
        .op_present(LEAVE_BUILD, &json!({ "tenant": tenant }))
        .await?;
    Ok(Reply::json(first(rows)))
}

#[derive(Deserialize)]
struct RemoveBody {
    name: String,
    reason: String,
}

/// Remove a build that holds no tenant.
async fn builds_remove(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: RemoveBody = parse(body)?;
    let name = field("name", &req.name, MAX_NAME)?;
    let reason = req.reason.trim();
    if reason.chars().count() > MAX_REASON {
        return Err(ConsoleError::BadRequest(format!(
            "a reason is {MAX_REASON} characters at most"
        )));
    }
    call.detail = format!("build={name}");
    let rows = who
        .door(c)?
        .op_present(REMOVE_BUILD, &json!({ "name": name, "reason": reason }))
        .await?;
    Ok(Reply::json(first(rows)))
}
