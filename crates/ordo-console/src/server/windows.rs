//! The Maintenance windows screen's routes (owner, 2026-11-04: "the
//! console will need an interface to work with these as well").
//!
//! A window is a block of time the deployment owner sets. It does nothing
//! by itself. Every call reaches the deployment through the console's one
//! client to the door, in its home space:
//!
//! | Route | Door op | Needs |
//! |---|---|---|
//! | `GET windows` | `std.deployment.windows` | signed in |
//! | `POST windows/set` | `std.deployment.setWindow` | both touches |
//! | `POST windows/delete` | `std.deployment.deleteWindow` | both touches |
//! | `POST windows/skip` | `std.deployment.skipWindow` | both touches |
//! | `POST windows/delete-skip` | `std.deployment.deleteSkip` | both touches |
//!
//! The console checks only the shape of a body. What a window may be (a
//! target, a time, a cron, a length, a message) is the door's to say, and
//! the screen shows its refusal as given. A message is passed on exactly
//! as typed: it is any text and is never rendered.

use axum::routing::{get, post};
use axum::Router;

use super::*;

/// `std.deployment.windows`.
pub const WINDOWS: &str = "std.deployment.windows";
/// `std.deployment.setWindow`.
pub const SET_WINDOW: &str = "std.deployment.setWindow";
/// `std.deployment.deleteWindow`.
pub const DELETE_WINDOW: &str = "std.deployment.deleteWindow";
/// `std.deployment.skipWindow`.
pub const SKIP_WINDOW: &str = "std.deployment.skipWindow";
/// `std.deployment.deleteSkip`.
pub const DELETE_SKIP: &str = "std.deployment.deleteSkip";

pub(super) fn routes() -> Router<Console> {
    Router::new()
        .route("/d/{handle}/api/windows", get(windows_route))
        .route("/d/{handle}/api/windows/set", post(set_route))
        .route("/d/{handle}/api/windows/delete", post(delete_route))
        .route("/d/{handle}/api/windows/skip", post(skip_route))
        .route(
            "/d/{handle}/api/windows/delete-skip",
            post(delete_skip_route),
        )
}

read!(windows_route, windows_list);
action!(set_route, "windows.set", windows_set);
action!(delete_route, "windows.delete", windows_delete);
action!(skip_route, "windows.skip", windows_skip);
action!(
    delete_skip_route,
    "windows.delete_skip",
    windows_delete_skip
);

fn first(rows: Vec<Value>) -> Value {
    rows.into_iter().next().unwrap_or(Value::Null)
}

/// A text field trimmed, `None` when blank.
fn given(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// An id the door takes: 1 to 64 characters, no spaces. The door checks
/// the rest.
fn id_of(field: &str, id: &str) -> Result<String, ConsoleError> {
    let id = id.trim();
    if id.is_empty() || id.len() > 64 || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ConsoleError::BadRequest(format!(
            "`{field}` must name a window or skip id"
        )));
    }
    Ok(id.to_string())
}

/// Every window, skip and time, 12 months back to 12 months ahead.
async fn windows_list(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who.door(c)?.op(WINDOWS, true, &json!({})).await?;
    Ok(Reply::json(first(rows)))
}

#[derive(Deserialize)]
struct SetBody {
    /// Set: replace the window under this id. Unset: a new one.
    #[serde(default)]
    id: Option<String>,
    /// `all`, `only` or `except`; the door checks it.
    target: String,
    #[serde(default)]
    tenants: Vec<String>,
    #[serde(default)]
    starts: Option<String>,
    #[serde(default)]
    ends: Option<String>,
    #[serde(default)]
    cron: Option<String>,
    #[serde(default, rename = "lengthSecs")]
    length_secs: Option<i64>,
    message: String,
}

/// Create a window, or replace the one under `id`.
async fn windows_set(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: SetBody = parse(body)?;
    let target = req.target.trim();
    let tenants: Vec<&str> = req
        .tenants
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    let mut params = json!({ "target": target, "message": req.message });
    if let Some(id) = given(&req.id) {
        params["id"] = json!(id_of("id", id)?);
    }
    if !tenants.is_empty() {
        params["tenants"] = json!(tenants);
    }
    for (k, v) in [
        ("starts", given(&req.starts)),
        ("ends", given(&req.ends)),
        ("cron", given(&req.cron)),
    ] {
        if let Some(v) = v {
            params[k] = json!(v);
        }
    }
    if let Some(l) = req.length_secs {
        params["lengthSecs"] = json!(l);
    }
    call.detail = format!(
        "id={} target={target} tenants={}",
        given(&req.id).unwrap_or("new"),
        tenants.len()
    );
    let row = first(who.door(c)?.op_present(SET_WINDOW, &params).await?);
    call.detail = format!(
        "id={} target={target} tenants={}",
        row["id"].as_str().unwrap_or("?"),
        tenants.len()
    );
    Ok(Reply::json(row))
}

#[derive(Deserialize)]
struct IdBody {
    id: String,
}

/// Delete a window and its skips.
async fn windows_delete(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let id = id_of("id", &parse::<IdBody>(body)?.id)?;
    call.detail = format!("id={id}");
    let rows = who
        .door(c)?
        .op_present(DELETE_WINDOW, &json!({ "id": id }))
        .await?;
    Ok(Reply::json(first(rows)))
}

#[derive(Deserialize)]
struct SkipBody {
    window: String,
    #[serde(default)]
    id: Option<String>,
    starts: String,
    ends: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Suppress a cron window's times over a range.
async fn windows_skip(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: SkipBody = parse(body)?;
    let window = id_of("window", &req.window)?;
    let mut params = json!({
        "window": window,
        "starts": req.starts.trim(),
        "ends": req.ends.trim(),
    });
    if let Some(id) = given(&req.id) {
        params["id"] = json!(id_of("id", id)?);
    }
    if let Some(r) = given(&req.reason) {
        params["reason"] = json!(r);
    }
    call.detail = format!("window={window}");
    let row = first(who.door(c)?.op_present(SKIP_WINDOW, &params).await?);
    call.detail = format!("window={window} skip={}", row["id"].as_str().unwrap_or("?"));
    Ok(Reply::json(row))
}

/// Delete a skip.
async fn windows_delete_skip(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let id = id_of("id", &parse::<IdBody>(body)?.id)?;
    call.detail = format!("skip={id}");
    let rows = who
        .door(c)?
        .op_present(DELETE_SKIP, &json!({ "id": id }))
        .await?;
    Ok(Reply::json(first(rows)))
}
