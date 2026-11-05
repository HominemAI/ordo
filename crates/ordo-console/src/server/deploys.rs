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
//! | `GET deploys/revoked` | `std.deployment.revoked` (D8) | signed in |
//! | `POST deploys/revoke` | `std.deployment.revoke` | both touches |
//! | `POST deploys/unrevoke` | `std.deployment.unrevoke` | both touches |
//! | `GET deploys/settings` | `std.deployment.versionSettings` | signed in |
//! | `POST deploys/setting` | `std.deployment.setVersionSetting` | the console's touch; the deployment's only at `ORDO_SECURITY_LEVEL=high` |
//! | `POST deploys/pin` | `std.deployment.pin` | the console's touch; no touch at the deployment |
//! | `POST deploys/move` | `std.deployment.moveOffRevoked` | the console's touch; no touch at the deployment |
//! | `POST deploys/target` | `std.deployment.setMoveTarget` | the console's touch; no touch at the deployment |
//! | `GET deploys/touches` | `std.deployment.tenantTouches` | signed in |
//! | `POST deploys/touch` | `std.deployment.exposeTouch` | the console's touch; the deployment's only at `ORDO_SECURITY_LEVEL=high` |
//!
//! The touches tenants may choose (owner, 2026-11-05) are a deployment
//! setting stored in Ordo too: which situations a tenant may turn on
//! (`raise`), off (`lower`), or both, beside what the level asks. Nothing is
//! exposed by default. The door refuses an act on the deployment by name.
//!
//! The version settings (`versions.served`, `versions.minimum`,
//! `versions.new_tenant`) live in Ordo, not the environment (owner,
//! 2026-11-05). The screen shows them with their history and changes one at
//! a time with a reason; the door checks each change and its refusal, which
//! names the setting and the value, is shown as given.
//!
//! A revoke marks a stored version as not to be served, with a reason
//! (`plan-versions.md` 4, increment 2b). Revoking the version that serves
//! needs `moveTo`, the version the deployment moves to; the door refuses
//! it otherwise, and the screen shows that refusal as given. The revoked
//! list is the screen's warning: each revoked version that still has
//! tenants on it, and which.
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
/// `std.deployment.revoke`.
pub const REVOKE: &str = "std.deployment.revoke";
/// `std.deployment.unrevoke`.
pub const UNREVOKE: &str = "std.deployment.unrevoke";
/// `std.deployment.revoked`.
pub const REVOKED: &str = "std.deployment.revoked";
/// `std.deployment.versionSettings`.
pub const VERSION_SETTINGS: &str = "std.deployment.versionSettings";
/// `std.deployment.setVersionSetting`.
pub const SET_VERSION_SETTING: &str = "std.deployment.setVersionSetting";
/// `std.deployment.pin`.
pub const PIN: &str = "std.deployment.pin";
/// `std.deployment.moveOffRevoked`.
pub const MOVE_OFF_REVOKED: &str = "std.deployment.moveOffRevoked";
/// `std.deployment.setMoveTarget`.
pub const SET_MOVE_TARGET: &str = "std.deployment.setMoveTarget";
/// `std.deployment.tenantTouches`.
pub const TENANT_TOUCHES: &str = "std.deployment.tenantTouches";
/// `std.deployment.exposeTouch`.
pub const EXPOSE_TOUCH: &str = "std.deployment.exposeTouch";
/// How many setting changes the screen lists.
pub const SETTING_HISTORY_LIMIT: u32 = 20;
/// The longest reason the console passes on, the door's own bound.
pub const MAX_REASON_CHARS: usize = 2048;

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
        .route("/d/{handle}/api/deploys/revoked", get(revoked_route))
        .route("/d/{handle}/api/deploys/revoke", post(revoke_route))
        .route("/d/{handle}/api/deploys/unrevoke", post(unrevoke_route))
        .route("/d/{handle}/api/deploys/settings", get(settings_route))
        .route("/d/{handle}/api/deploys/setting", post(setting_route))
        .route("/d/{handle}/api/deploys/pin", post(pin_route))
        .route("/d/{handle}/api/deploys/move", post(move_route))
        .route("/d/{handle}/api/deploys/target", post(target_route))
        .route("/d/{handle}/api/deploys/touches", get(touches_route))
        .route("/d/{handle}/api/deploys/touch", post(touch_route))
}

read!(deploys_route, deploys_get);
read!(serving_route, serving_get);
action!(preview_route, "deploys.preview", deploys_preview);
action!(apply_program_route, "deploys.apply", deploys_apply);
action!(activate_route, "deploys.activate", deploys_activate);
read!(revoked_route, deploys_revoked);
action!(revoke_route, "deploys.revoke", deploys_revoke);
action!(unrevoke_route, "deploys.unrevoke", deploys_unrevoke);
read!(settings_route, deploys_settings);
action!(setting_route, "deploys.setting", deploys_setting);
action!(pin_route, "deploys.pin", deploys_pin);
action!(move_route, "deploys.move", deploys_move);
action!(target_route, "deploys.target", deploys_target);
read!(touches_route, deploys_touches);
action!(touch_route, "deploys.touch", deploys_touch);

#[derive(Deserialize)]
struct PinBody {
    tenant: String,
    /// Absent or empty clears a tenant's move target; a pin needs it.
    #[serde(default)]
    version: Option<String>,
}

/// A tenant id as the door takes one.
fn tenant_of(t: &str) -> Result<String, ConsoleError> {
    let t = t.trim();
    if t.is_empty() || t.len() > 128 {
        return Err(ConsoleError::BadRequest(
            "`tenant` must name a tenant, as the Tenants screen lists it".into(),
        ));
    }
    Ok(t.to_string())
}

/// Pin one tenant to a stored version now. The door refuses a revoked,
/// unserved or too-old version in its own words.
async fn deploys_pin(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: PinBody = parse(body)?;
    let tenant = tenant_of(&req.tenant)?;
    let version = version_of("version", req.version.as_deref().unwrap_or_default())?;
    call.detail = format!("tenant={tenant} version={version}");
    let rows = who
        .door(c)?
        .op_present(PIN, &json!({ "tenant": tenant, "version": version }))
        .await?;
    Ok(Reply::json(first(rows)))
}

/// Move one tenant off a revoked version (or one below the minimum) now,
/// to its target.
async fn deploys_move(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: PinBody = parse(body)?;
    let tenant = tenant_of(&req.tenant)?;
    call.detail = format!("tenant={tenant}");
    let rows = who
        .door(c)?
        .op_present(MOVE_OFF_REVOKED, &json!({ "tenant": tenant }))
        .await?;
    Ok(Reply::json(first(rows)))
}

/// Set the version the window sweep moves one tenant to, or clear it.
async fn deploys_target(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: PinBody = parse(body)?;
    let tenant = tenant_of(&req.tenant)?;
    let version = req
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| version_of("version", v))
        .transpose()?;
    call.detail = format!(
        "tenant={tenant} target={}",
        version.as_deref().unwrap_or("cleared")
    );
    let rows = who
        .door(c)?
        .op_present(
            SET_MOVE_TARGET,
            &json!({ "tenant": tenant, "version": version }),
        )
        .await?;
    Ok(Reply::json(first(rows)))
}

#[derive(Deserialize)]
struct TouchBody {
    situation: String,
    exposed: String,
    reason: String,
}

#[derive(Deserialize)]
struct SettingBody {
    setting: String,
    /// Absent or empty clears it.
    #[serde(default)]
    value: Option<String>,
    reason: String,
}

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

#[derive(Deserialize)]
struct RevokeBody {
    version: String,
    reason: String,
    /// Where the deployment moves when the version revoked is the one that
    /// serves.
    #[serde(default, rename = "moveTo")]
    move_to: Option<String>,
}

/// A stored version as the door takes one.
fn version_of(field: &str, v: &str) -> Result<String, ConsoleError> {
    let v = v.trim();
    if v.is_empty() || v.len() > 128 {
        return Err(ConsoleError::BadRequest(format!(
            "`{field}` must name a stored version"
        )));
    }
    Ok(v.to_string())
}

/// A reason: required, at most [`MAX_REASON_CHARS`] characters.
fn reason_of(r: &str) -> Result<String, ConsoleError> {
    let r = r.trim();
    if r.is_empty() {
        return Err(ConsoleError::BadRequest(
            "`reason` is required: say why, in a few words".into(),
        ));
    }
    if r.chars().count() > MAX_REASON_CHARS {
        return Err(ConsoleError::BadRequest(format!(
            "`reason` is at most {MAX_REASON_CHARS} characters"
        )));
    }
    Ok(r.to_string())
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

/// The revoked versions that still have tenants on them (D8's warning).
async fn deploys_revoked(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who.door(c)?.op(REVOKED, true, &json!({})).await?;
    Ok(Reply::json(first(rows)))
}

/// Mark a stored version revoked, moving the deployment off it when it is
/// the one that serves.
async fn deploys_revoke(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: RevokeBody = parse(body)?;
    let version = version_of("version", &req.version)?;
    let reason = reason_of(&req.reason)?;
    let mut params = json!({ "version": version, "reason": reason });
    let move_to = match req.move_to.as_deref().map(str::trim) {
        Some(m) if !m.is_empty() => Some(version_of("moveTo", m)?),
        _ => None,
    };
    if let Some(m) = &move_to {
        params["moveTo"] = json!(m);
    }
    call.detail = format!(
        "version={version} move-to={}",
        move_to.as_deref().unwrap_or("-")
    );
    let rows = who
        .door(c)?
        .op_present_within(REVOKE, &params, crate::door::DEPLOY_CALL_TIMEOUT)
        .await?;
    Ok(Reply::json(first(rows)))
}

/// Lift a version's revoke mark.
async fn deploys_unrevoke(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: RevokeBody = parse(body)?;
    let version = version_of("version", &req.version)?;
    let reason = reason_of(&req.reason)?;
    call.detail = format!("version={version}");
    let rows = who
        .door(c)?
        .op_present(UNREVOKE, &json!({ "version": version, "reason": reason }))
        .await?;
    Ok(Reply::json(first(rows)))
}

/// The three version settings and their recent history.
async fn deploys_settings(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who
        .door(c)?
        .op(
            VERSION_SETTINGS,
            true,
            &json!({ "limit": SETTING_HISTORY_LIMIT }),
        )
        .await?;
    Ok(Reply::json(first(rows)))
}

/// Change one version setting, or clear it, with a reason.
async fn deploys_setting(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: SettingBody = parse(body)?;
    let setting = match req.setting.trim() {
        s @ ("versions.served" | "versions.minimum" | "versions.new_tenant") => s.to_string(),
        other => {
            return Err(ConsoleError::BadRequest(format!(
                "`{other}` is not a version setting; there are versions.served, \
                 versions.minimum and versions.new_tenant"
            )))
        }
    };
    let value = req
        .value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    if value.as_ref().is_some_and(|v| v.len() > 4096) {
        return Err(ConsoleError::BadRequest(
            "`value` is at most 4096 characters".into(),
        ));
    }
    let reason = reason_of(&req.reason)?;
    call.detail = format!(
        "setting={setting} value={}",
        value.as_deref().unwrap_or("cleared")
    );
    let rows = who
        .door(c)?
        .op_present(
            SET_VERSION_SETTING,
            &json!({ "setting": setting, "value": value, "reason": reason }),
        )
        .await?;
    Ok(Reply::json(first(rows)))
}

/// Which touches tenants may choose, what the level asks, and the changes.
async fn deploys_touches(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who
        .door(c)?
        .op(
            TENANT_TOUCHES,
            true,
            &json!({ "limit": SETTING_HISTORY_LIMIT }),
        )
        .await?;
    Ok(Reply::json(first(rows)))
}

/// Expose one situation to tenants, or take it back, with a reason. The
/// door checks the situation and the direction; its refusal is shown as
/// given.
async fn deploys_touch(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: TouchBody = parse(body)?;
    let situation = req.situation.trim();
    if situation.is_empty() || situation.len() > 64 {
        return Err(ConsoleError::BadRequest(
            "`situation` names one situation, such as membership".into(),
        ));
    }
    let exposed = match req.exposed.trim() {
        e @ ("none" | "raise" | "lower" | "both") => e,
        other => {
            return Err(ConsoleError::BadRequest(format!(
                "`{other}` is not a direction; there are none, raise, lower and both"
            )))
        }
    };
    let reason = reason_of(&req.reason)?;
    call.detail = format!("situation={situation} exposed={exposed}");
    let rows = who
        .door(c)?
        .op_present(
            EXPOSE_TOUCH,
            &json!({ "situation": situation, "exposed": exposed, "reason": reason }),
        )
        .await?;
    Ok(Reply::json(first(rows)))
}
