//! The Tenants screen's routes (`plan-console.md` 4.4, step 5).
//!
//! Every call reaches the deployment through the console's one client to
//! the door (`crate::door`, `crate::link`), with the console key:
//!
//! | Route | Door op | Where |
//! |---|---|---|
//! | `GET tenants` | `std.deployment.tenants`, and `GET /health/ready` for the tenants held closed | home space |
//! | `POST tenants/create` | `std.space.create` | home space |
//! | `POST tenants/reap` | `std.space.reap` | home space |
//! | `POST tenants/pause`, `tenants/resume` | `std.deployment.pauseTenant`, `resumeTenant` | home space |
//! | `POST tenants/export`, `delete`, `attach-bucket`, `detach-bucket` | `std.space.export`, `delete`, `attachBucket`, `detachBucket` | the tenant's own space |
//! | `GET tenants/successions` | `std.identity.openCases` | home space |
//! | `POST tenants/succession/open`, `succession/complete` | `std.space.succeedOwner` | home space |
//! | `POST tenants/succession/resolve` | `std.identity.resolveObjection` | home space |
//!
//! **Owner succession** (owner, 2026-10-24) runs in the deployment's own
//! space, so it needs no role in the tenant. The form takes the successor's
//! id from the deployment owner and shows no list of the tenant's members:
//! the deployment owner holds no role in most tenants, and a member list
//! would show who is inside. The tenant's managers are told and may object;
//! an objection pauses the case until it is resolved here.
//!
//! Every write needs two touches, as every write at the deployment does:
//! the console owner's own (step-up at the console) and the deployment
//! owner's, relayed to the door (`crate::link`). One of each covers the
//! next 5 minutes. The tenant's own ops answer only where the deployment
//! owner holds the owner role, which the door decides. A delete also needs
//! the tenant's id typed back.

use super::*;

read!(tenants_route, tenants_list);
action!(create_route, "tenants.create", tenants_create);
action!(reap_route, "tenants.reap", tenants_reap);
action!(pause_route, "tenants.pause", tenants_pause);
action!(resume_route, "tenants.resume", tenants_resume);
action!(export_route, "tenants.export", tenants_export);
action!(delete_route, "tenants.delete", tenants_delete);
action!(attach_route, "tenants.attach_bucket", tenants_attach);
action!(detach_route, "tenants.detach_bucket", tenants_detach);
read!(successions_route, successions_list);
action!(
    succession_open_route,
    "tenants.succession_open",
    succession_open
);
action!(
    succession_complete_route,
    "tenants.succession_complete",
    succession_complete
);
action!(
    succession_resolve_route,
    "tenants.succession_resolve",
    succession_resolve
);

const LIST: &str = "std.deployment.tenants";
const CREATE: &str = "std.space.create";
const REAP: &str = "std.space.reap";
const PAUSE: &str = "std.deployment.pauseTenant";
const RESUME: &str = "std.deployment.resumeTenant";
const EXPORT: &str = "std.space.export";
const DELETE: &str = "std.space.delete";
const ATTACH: &str = "std.space.attachBucket";
const DETACH: &str = "std.space.detachBucket";
const OPEN_CASES: &str = "std.identity.openCases";
const SUCCEED: &str = "std.space.succeedOwner";
const RESOLVE: &str = "std.identity.resolveObjection";
/// The console key ops, read for the deployment owner's own principal.
const KEYS: &str = "std.deployment.consoleKeys";

/// The Tenants routes, under `/d/<handle>/api/tenants`.
pub(super) fn routes() -> Router<Console> {
    Router::new()
        .route("/d/{handle}/api/tenants", get(tenants_route))
        .route("/d/{handle}/api/tenants/create", post(create_route))
        .route("/d/{handle}/api/tenants/reap", post(reap_route))
        .route("/d/{handle}/api/tenants/pause", post(pause_route))
        .route("/d/{handle}/api/tenants/resume", post(resume_route))
        .route("/d/{handle}/api/tenants/export", post(export_route))
        .route("/d/{handle}/api/tenants/delete", post(delete_route))
        .route("/d/{handle}/api/tenants/attach-bucket", post(attach_route))
        .route("/d/{handle}/api/tenants/detach-bucket", post(detach_route))
        .route(
            "/d/{handle}/api/tenants/successions",
            get(successions_route),
        )
        .route(
            "/d/{handle}/api/tenants/succession/open",
            post(succession_open_route),
        )
        .route(
            "/d/{handle}/api/tenants/succession/complete",
            post(succession_complete_route),
        )
        .route(
            "/d/{handle}/api/tenants/succession/resolve",
            post(succession_resolve_route),
        )
}

/// A principal or case id as the door takes one: 1 to 256 letters, digits,
/// `_`, `-`, `.` and `:`.
fn id_of(field: &str, id: &str) -> Result<String, ConsoleError> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 256
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_-.:".contains(ch))
    {
        return Err(ConsoleError::BadRequest(format!(
            "`{field}` is an id: 1 to 256 letters, digits, `_`, `-`, `.` and `:`"
        )));
    }
    Ok(id.to_string())
}

/// A tenant id as the door takes one: letters, digits, `_` and `-`.
fn tenant_id(id: &str) -> Result<String, ConsoleError> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(ConsoleError::BadRequest(
            "`tenant` is a tenant id: 1 to 128 letters, digits, `_` and `-`".into(),
        ));
    }
    Ok(id.to_string())
}

fn first(rows: Vec<Value>) -> Value {
    rows.into_iter().next().unwrap_or(Value::Null)
}

#[derive(Deserialize)]
struct TenantBody {
    tenant: String,
}

/// Every tenant: `std.deployment.tenants`, with the door's readiness list
/// of tenants held closed merged in. The readiness call is best effort: a
/// deployment that is not ready answers 503 and the list still shows.
async fn tenants_list(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let link = who.door(c)?;
    let mut rows = link.op(LIST, true, &json!({})).await?;
    let (held, ready_error) = match link.ready().await {
        Ok(v) => (
            v.get("quarantined")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            None,
        ),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    for h in &held {
        let (Some(space), reason) = (h.get("space").and_then(Value::as_str), h.get("reason"))
        else {
            continue;
        };
        if let Some(r) = rows
            .iter_mut()
            .find(|r| r.get("id").and_then(Value::as_str) == Some(space))
        {
            if r.get("quarantined").is_none_or(Value::is_null) {
                r["quarantined"] = reason.cloned().unwrap_or(Value::Null);
            }
        }
    }
    Ok(Reply::json(json!({
        "tenants": rows,
        "readyError": ready_error,
    })))
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    /// The principal who will own the tenant. Unset: the deployment owner.
    #[serde(default)]
    owner: Option<String>,
}

/// The deployment owner's own principal, from this console's key at the
/// door.
async fn own_principal(link: &DoorLink) -> Result<String, ConsoleError> {
    let rows = link.op(KEYS, true, &json!({})).await?;
    let own = link.status().await?;
    let key = own.get("key").and_then(Value::as_str).unwrap_or_default();
    rows.iter()
        .find(|r| r.get("id").and_then(Value::as_str) == Some(key))
        .and_then(|r| r.get("principal").and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| {
            ConsoleError::Internal("the deployment did not name this console key's owner".into())
        })
}

/// Create a tenant (`std.space.create`).
async fn tenants_create(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: CreateBody = parse(body)?;
    let name = req.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 256 || name.chars().any(char::is_control) {
        return Err(ConsoleError::BadRequest(
            "`name` must be 1 to 256 printable characters".into(),
        ));
    }
    let link = who.door(c)?;
    let owner = match req
        .owner
        .map(|o| o.trim().to_string())
        .filter(|o| !o.is_empty())
    {
        Some(o) => o,
        // A deployment person signed in at the identity service is not an
        // app principal, so a tenant cannot default to them as its owner:
        // the form must name one.
        None if who.is_person() => {
            return Err(ConsoleError::BadRequest(
                "name the tenant's owner in `owner`: a deployment person is not one of the \
                 app's principals"
                    .into(),
            ))
        }
        None => own_principal(door_link(c)?).await?,
    };
    let rows = link
        .op_present(CREATE, &json!({"name": name, "owner": owner}))
        .await?;
    let made = first(rows);
    call.detail = format!(
        "tenant={} owner={owner}",
        made.get("space").and_then(Value::as_str).unwrap_or("")
    );
    Ok(Reply::json(json!({ "created": made })))
}

/// Release what a failed or stuck provision left (`std.space.reap`). The
/// door refuses a tenant that is serving.
async fn tenants_reap(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: TenantBody = parse(body)?;
    let tenant = tenant_id(&req.tenant)?;
    call.detail = format!("tenant={tenant}");
    let rows = who
        .door(c)?
        .op_present(REAP, &json!({"tenant": tenant}))
        .await?;
    Ok(Reply::json(json!({ "reaped": first(rows) })))
}

#[derive(Deserialize)]
struct PauseBody {
    tenant: String,
    /// A word from the deployment's closed list; the door checks it.
    #[serde(default)]
    note: Option<String>,
}

async fn tenants_pause(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: PauseBody = parse(body)?;
    let tenant = tenant_id(&req.tenant)?;
    let mut params = json!({"tenant": tenant});
    if let Some(n) = req
        .note
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
    {
        call.detail = format!("tenant={tenant} note={n}");
        params["note"] = json!(n);
    } else {
        call.detail = format!("tenant={tenant}");
    }
    let rows = who.door(c)?.op_present(PAUSE, &params).await?;
    Ok(Reply::json(json!({ "paused": first(rows) })))
}

async fn tenants_resume(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: TenantBody = parse(body)?;
    let tenant = tenant_id(&req.tenant)?;
    call.detail = format!("tenant={tenant}");
    let rows = who
        .door(c)?
        .op_present(RESUME, &json!({"tenant": tenant}))
        .await?;
    Ok(Reply::json(json!({ "resumed": first(rows) })))
}

#[derive(Deserialize)]
struct ExportBody {
    tenant: String,
    /// Keep sealed fields sealed in the bundle.
    #[serde(default)]
    ciphertext: Option<bool>,
}

/// Export a tenant the deployment owner owns (`std.space.export`). The
/// bundle is written on the deployment's disk; the answer names it.
async fn tenants_export(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: ExportBody = parse(body)?;
    let tenant = tenant_id(&req.tenant)?;
    call.detail = format!("tenant={tenant}");
    let mut params = json!({});
    if let Some(ct) = req.ciphertext {
        params["ciphertext"] = json!(ct);
    }
    let rows = who.door(c)?.op_present_in(&tenant, EXPORT, &params).await?;
    Ok(Reply::json(json!({ "exported": first(rows) })))
}

#[derive(Deserialize)]
struct DeleteBody {
    tenant: String,
    /// The tenant's id, typed by the person.
    confirm: String,
}

/// Delete a tenant the deployment owner owns (`std.space.delete`). The
/// person types the tenant's id; the door checks it again.
async fn tenants_delete(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: DeleteBody = parse(body)?;
    let tenant = tenant_id(&req.tenant)?;
    call.detail = format!("tenant={tenant}");
    if req.confirm.trim() != tenant {
        return Err(ConsoleError::BadRequest(
            "to delete a tenant, type its id exactly as shown; nothing was deleted".into(),
        ));
    }
    let rows = who
        .door(c)?
        .op_present_in(&tenant, DELETE, &json!({"confirm": tenant}))
        .await?;
    Ok(Reply::json(json!({ "deleted": first(rows) })))
}

#[derive(Deserialize)]
struct AttachBody {
    tenant: String,
    endpoint: String,
    region: String,
    bucket: String,
    /// The label of a credential the deployment holds, never a secret.
    credential: String,
    #[serde(rename = "platformSealed")]
    platform_sealed: bool,
    #[serde(default, rename = "pathStyle")]
    path_style: Option<bool>,
}

/// Attach a bucket to a tenant the deployment owner owns
/// (`std.space.attachBucket`).
async fn tenants_attach(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: AttachBody = parse(body)?;
    let tenant = tenant_id(&req.tenant)?;
    call.detail = format!("tenant={tenant} bucket={}", req.bucket.trim());
    let mut params = json!({
        "endpoint": req.endpoint.trim(),
        "region": req.region.trim(),
        "bucket": req.bucket.trim(),
        "credential": req.credential.trim(),
        "platformSealed": req.platform_sealed,
    });
    if let Some(p) = req.path_style {
        params["pathStyle"] = json!(p);
    }
    let rows = who.door(c)?.op_present_in(&tenant, ATTACH, &params).await?;
    Ok(Reply::json(json!({ "attached": first(rows) })))
}

async fn tenants_detach(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: TenantBody = parse(body)?;
    let tenant = tenant_id(&req.tenant)?;
    call.detail = format!("tenant={tenant}");
    let rows = who
        .door(c)?
        .op_present_in(&tenant, DETACH, &json!({}))
        .await?;
    Ok(Reply::json(json!({ "detached": first(rows) })))
}

/// The open recoveries through the door and successions, with when each
/// matures and every objection and its resolution
/// (`std.identity.openCases`).
async fn successions_list(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let rows = who.door(c)?.op(OPEN_CASES, true, &json!({})).await?;
    Ok(Reply::json(json!({ "cases": rows })))
}

#[derive(Deserialize)]
struct OpenSuccessionBody {
    tenant: String,
    /// The successor's principal id, typed by the deployment owner.
    principal: String,
    /// Who has left: the owners' principal ids, typed. Only they lose their
    /// delegations and grants when the succession completes.
    departed: Vec<String>,
    /// A class from the door's closed list; the door checks it.
    evidence: String,
    #[serde(default)]
    reference: Option<String>,
}

/// Open an owner succession (`std.space.succeedOwner` without a case).
async fn succession_open(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: OpenSuccessionBody = parse(body)?;
    let tenant = tenant_id(&req.tenant)?;
    let principal = id_of("principal", &req.principal)?;
    let departed = req
        .departed
        .iter()
        .filter(|d| !d.trim().is_empty())
        .map(|d| id_of("departed", d))
        .collect::<Result<Vec<_>, _>>()?;
    if departed.is_empty() {
        return Err(ConsoleError::BadRequest(
            "`departed` names at least one owner who has left".into(),
        ));
    }
    let evidence = req.evidence.trim().to_string();
    let mut params = json!({"tenant": tenant, "principal": principal, "evidence": evidence,
                            "departed": departed});
    let reference = req
        .reference
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    if let Some(r) = &reference {
        params["reference"] = json!(r);
    }
    call.detail = format!(
        "tenant={tenant} principal={principal} departed={} evidence={evidence} ref={}",
        departed.join(","),
        reference.as_deref().unwrap_or("-")
    );
    let rows = who.door(c)?.op_present(SUCCEED, &params).await?;
    Ok(Reply::json(json!({ "opened": first(rows) })))
}

#[derive(Deserialize)]
struct CompleteSuccessionBody {
    tenant: String,
    case: String,
    /// End the departed owners' delegations and grants to apps. Unset: the
    /// deployment's setting, on unless it says otherwise.
    #[serde(default, rename = "endGrants")]
    end_grants: Option<bool>,
}

/// Carry out a matured succession (`std.space.succeedOwner` with its case).
async fn succession_complete(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: CompleteSuccessionBody = parse(body)?;
    let tenant = tenant_id(&req.tenant)?;
    let case = id_of("case", &req.case)?;
    let mut params = json!({"tenant": tenant, "case": case});
    if let Some(e) = req.end_grants {
        params["endGrants"] = json!(e);
    }
    call.detail = format!(
        "tenant={tenant} case={case} end-grants={}",
        req.end_grants
            .map(|e| if e { "on" } else { "off" })
            .unwrap_or("default")
    );
    let rows = who.door(c)?.op_present(SUCCEED, &params).await?;
    Ok(Reply::json(json!({ "succeeded": first(rows) })))
}

#[derive(Deserialize)]
struct ResolveBody {
    case: String,
    /// `resume` or `cancel`; the door checks it.
    decision: String,
    /// A word from the decision's fixed list; the door checks it.
    reason: String,
}

/// Resolve the objections to a paused case
/// (`std.identity.resolveObjection`).
async fn succession_resolve(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: ResolveBody = parse(body)?;
    let case = id_of("case", &req.case)?;
    let (decision, reason) = (req.decision.trim(), req.reason.trim());
    call.detail = format!("case={case} decision={decision} reason={reason}");
    let rows = who
        .door(c)?
        .op_present(
            RESOLVE,
            &json!({"case": case, "decision": decision, "reason": reason}),
        )
        .await?;
    Ok(Reply::json(json!({ "resolved": first(rows) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenant_id_is_what_the_door_takes() {
        assert_eq!(tenant_id(" spc_ab-1 ").unwrap(), "spc_ab-1");
        for bad in ["", "a b", "a/b", "spc_é", &"x".repeat(129)] {
            assert!(tenant_id(bad).is_err(), "{bad}");
        }
    }
}
