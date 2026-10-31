//! The People screen's routes (`plan-console.md` 4.5, step 8).
//!
//! Every call reaches the deployment through the console's one client to
//! the door (`crate::door`, `crate::link`), with the console key, in the
//! tenant's own space:
//!
//! | Route | Door op |
//! |---|---|
//! | `GET people?tenant` | `std.space.members`, `std.space.groups`, `std.space.grants` |
//! | `GET people/group?tenant&group` | `std.space.groupMembers` |
//! | `GET people/callable?tenant&member` | `std.space.callableBy` |
//! | `GET people/callers?tenant&op` | `std.space.callersOf` |
//! | `POST people/invite` | `std.space.invite` |
//! | `POST people/set-role` | `std.space.setRole` |
//! | `POST people/remove` | `std.space.remove` |
//! | `POST people/add-to-group` | `std.space.addToGroup` |
//! | `GET people/invitations?tenant` | `std.space.invitations` |
//! | `GET people/profile?tenant[&op]` | `std.space.profile` |
//! | `GET people/admission?tenant` | `std.space.admission` |
//! | `POST people/suspend`, `people/reinstate` | `std.space.suspend`, `std.space.reinstate` |
//! | `POST people/create-group`, `people/delete-group` | `std.space.createGroup`, `std.space.deleteGroup` |
//! | `POST people/remove-from-group` | `std.space.removeFromGroup` |
//! | `POST people/set-op-access`, `people/reset-op-access` | `std.space.setOpAccess`, `std.space.resetOpAccess` |
//! | `POST people/set-member-access` | `std.space.setMemberAccess` |
//! | `POST people/revoke-invitation` | `std.space.revokeInvitation` |
//! | `POST people/set-admission` | `std.space.setAdmission` (open, role, providers, firstComer) |
//! | `POST people/recover-member` | `std.space.recoverMember` |
//! | `POST people/approve-recovery` | `std.space.approveMemberRecovery` |
//! | `POST people/recovery-policy` | `std.space.setRecoveryPolicy` |
//!
//! The door takes a console session in a tenant only for the ops on
//! `ordo_host::console_keys::TENANT_OPS_OPEN`, only within 300 seconds of
//! the deployment owner's relayed touch, and only where the owner holds a
//! role that may call the op. So a read needs that touch too, and a tenant
//! where the owner holds no role answers `access_denied`.
//!
//! Every write needs both touches, as every write at the deployment does:
//! the console owner's own (step-up at the console) and the deployment
//! owner's, relayed. Each is audited here, and the door records it in the
//! tenant's security stream (G12).

use axum::extract::Query;

use super::*;

action!(invite_route, "people.invite", people_invite);
action!(set_role_route, "people.set_role", people_set_role);
action!(remove_route, "people.remove", people_remove);
action!(
    add_to_group_route,
    "people.add_to_group",
    people_add_to_group
);
action!(suspend_route, "people.suspend", people_suspend);
action!(reinstate_route, "people.reinstate", people_reinstate);
action!(
    create_group_route,
    "people.create_group",
    people_create_group
);
action!(
    delete_group_route,
    "people.delete_group",
    people_delete_group
);
action!(
    remove_from_group_route,
    "people.remove_from_group",
    people_remove_from_group
);
action!(
    set_op_access_route,
    "people.set_op_access",
    people_set_op_access
);
action!(
    reset_op_access_route,
    "people.reset_op_access",
    people_reset_op_access
);
action!(
    set_member_access_route,
    "people.set_member_access",
    people_set_member_access
);
action!(
    revoke_invitation_route,
    "people.revoke_invitation",
    people_revoke_invitation
);
action!(
    set_admission_route,
    "people.set_admission",
    people_set_admission
);
action!(
    recover_member_route,
    "people.recover_member",
    people_recover_member
);
action!(
    approve_recovery_route,
    "people.approve_recovery",
    people_approve_recovery
);
action!(
    recovery_policy_route,
    "people.recovery_policy",
    people_recovery_policy
);

const MEMBERS: &str = "std.space.members";
const GROUPS: &str = "std.space.groups";
const GRANTS: &str = "std.space.grants";
const GROUP_MEMBERS: &str = "std.space.groupMembers";
const CALLABLE_BY: &str = "std.space.callableBy";
const CALLERS_OF: &str = "std.space.callersOf";
const INVITE: &str = "std.space.invite";
const SET_ROLE: &str = "std.space.setRole";
const REMOVE: &str = "std.space.remove";
const ADD_TO_GROUP: &str = "std.space.addToGroup";
const INVITATIONS: &str = "std.space.invitations";
const PROFILE: &str = "std.space.profile";
const ADMISSION: &str = "std.space.admission";
const SUSPEND: &str = "std.space.suspend";
const REINSTATE: &str = "std.space.reinstate";
const CREATE_GROUP: &str = "std.space.createGroup";
const DELETE_GROUP: &str = "std.space.deleteGroup";
const REMOVE_FROM_GROUP: &str = "std.space.removeFromGroup";
const SET_OP_ACCESS: &str = "std.space.setOpAccess";
const RESET_OP_ACCESS: &str = "std.space.resetOpAccess";
const SET_MEMBER_ACCESS: &str = "std.space.setMemberAccess";
const REVOKE_INVITATION: &str = "std.space.revokeInvitation";
const SET_ADMISSION: &str = "std.space.setAdmission";
const RECOVER_MEMBER: &str = "std.space.recoverMember";
const APPROVE_MEMBER_RECOVERY: &str = "std.space.approveMemberRecovery";
const SET_RECOVERY_POLICY: &str = "std.space.setRecoveryPolicy";

/// The roles a member can hold.
const ROLES: [&str; 4] = ["owner", "manager", "editor", "viewer"];

/// The People routes, under `/d/<handle>/api/people`.
pub(super) fn routes() -> Router<Console> {
    Router::new()
        .route("/d/{handle}/api/people", get(people_route))
        .route("/d/{handle}/api/people/group", get(group_route))
        .route("/d/{handle}/api/people/callable", get(callable_route))
        .route("/d/{handle}/api/people/callers", get(callers_route))
        .route("/d/{handle}/api/people/invite", post(invite_route))
        .route("/d/{handle}/api/people/set-role", post(set_role_route))
        .route("/d/{handle}/api/people/remove", post(remove_route))
        .route(
            "/d/{handle}/api/people/add-to-group",
            post(add_to_group_route),
        )
        .route("/d/{handle}/api/people/invitations", get(invitations_route))
        .route("/d/{handle}/api/people/profile", get(profile_route))
        .route("/d/{handle}/api/people/admission", get(admission_route))
        .route("/d/{handle}/api/people/suspend", post(suspend_route))
        .route("/d/{handle}/api/people/reinstate", post(reinstate_route))
        .route(
            "/d/{handle}/api/people/create-group",
            post(create_group_route),
        )
        .route(
            "/d/{handle}/api/people/delete-group",
            post(delete_group_route),
        )
        .route(
            "/d/{handle}/api/people/remove-from-group",
            post(remove_from_group_route),
        )
        .route(
            "/d/{handle}/api/people/set-op-access",
            post(set_op_access_route),
        )
        .route(
            "/d/{handle}/api/people/reset-op-access",
            post(reset_op_access_route),
        )
        .route(
            "/d/{handle}/api/people/set-member-access",
            post(set_member_access_route),
        )
        .route(
            "/d/{handle}/api/people/revoke-invitation",
            post(revoke_invitation_route),
        )
        .route(
            "/d/{handle}/api/people/set-admission",
            post(set_admission_route),
        )
        .route(
            "/d/{handle}/api/people/recover-member",
            post(recover_member_route),
        )
        .route(
            "/d/{handle}/api/people/approve-recovery",
            post(approve_recovery_route),
        )
        .route(
            "/d/{handle}/api/people/recovery-policy",
            post(recovery_policy_route),
        )
}

/// A GET that reads with a query string.
macro_rules! read_query {
    ($name:ident, $inner:ident) => {
        async fn $name(
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
                Ok(()) => $inner(&c, &mut call, &headers, &query).await,
                Err(e) => Err(e),
            };
            match result {
                Ok(r) => r.into_response(),
                Err(e) => e.into_response(),
            }
        }
    };
}

read_query!(people_route, people_list);
read_query!(group_route, group_members);
read_query!(callable_route, callable_by);
read_query!(callers_route, callers_of);
read_query!(invitations_route, invitations);
read_query!(profile_route, profile);
read_query!(admission_route, admission);

/// An id as the door takes one: letters, digits, `_` and `-`.
fn id_of(field: &str, id: &str) -> Result<String, ConsoleError> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(ConsoleError::BadRequest(format!(
            "`{field}` is an id: 1 to 128 letters, digits, `_` and `-`"
        )));
    }
    Ok(id.to_string())
}

fn wanted(query: &HashMap<String, String>, field: &str) -> Result<String, ConsoleError> {
    id_of(field, query.get(field).map(String::as_str).unwrap_or(""))
}

fn role_of(role: &str) -> Result<String, ConsoleError> {
    let role = role.trim();
    if ROLES.contains(&role) {
        Ok(role.to_string())
    } else {
        Err(ConsoleError::BadRequest(
            "`role` is one of owner, manager, editor or viewer".into(),
        ))
    }
}

/// An op name as the door takes one, such as `notes.list`.
fn op_of(op: &str) -> Result<String, ConsoleError> {
    let op = op.trim();
    if op.is_empty()
        || op.len() > 256
        || !op
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '$'))
    {
        return Err(ConsoleError::BadRequest(
            "`op` is an op name such as notes.list: 1 to 256 letters, digits, `.`, `_`, `-`".into(),
        ));
    }
    Ok(op.to_string())
}

/// A comma list of roles, or of group ids, as `setOpAccess` takes it.
/// Empty is none.
fn list_of(field: &str, text: &str, roles: bool) -> Result<String, ConsoleError> {
    let mut out: Vec<String> = Vec::new();
    for w in text.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        let w = if roles { role_of(w)? } else { id_of(field, w)? };
        if !out.contains(&w) {
            out.push(w);
        }
    }
    Ok(out.join(","))
}

/// Free text the door takes as it is (a group's name, a provider rule):
/// at most `max` printable characters.
fn plain(field: &str, text: &str, max: usize) -> Result<String, ConsoleError> {
    let t = text.trim();
    if t.chars().count() > max || t.chars().any(char::is_control) {
        return Err(ConsoleError::BadRequest(format!(
            "`{field}` must be at most {max} printable characters"
        )));
    }
    Ok(t.to_string())
}

fn first(rows: Vec<Value>) -> Value {
    rows.into_iter().next().unwrap_or(Value::Null)
}

/// Members, groups and grants of one tenant, in one answer.
async fn people_list(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let tenant = wanted(query, "tenant")?;
    let link = who.door(c)?;
    let members = link.read_present_in(&tenant, MEMBERS, &json!({})).await?;
    let groups = link.read_present_in(&tenant, GROUPS, &json!({})).await?;
    let grants = link.read_present_in(&tenant, GRANTS, &json!({})).await?;
    Ok(Reply::json(json!({
        "tenant": tenant,
        "members": members,
        "groups": groups,
        "grants": grants,
    })))
}

async fn group_members(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let tenant = wanted(query, "tenant")?;
    let group = wanted(query, "group")?;
    let rows = who
        .door(c)?
        .read_present_in(&tenant, GROUP_MEMBERS, &json!({"group": group}))
        .await?;
    Ok(Reply::json(json!({ "group": group, "members": rows })))
}

async fn callable_by(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let tenant = wanted(query, "tenant")?;
    let member = wanted(query, "member")?;
    let rows = who
        .door(c)?
        .read_present_in(&tenant, CALLABLE_BY, &json!({"member": member}))
        .await?;
    Ok(Reply::json(json!({ "member": member, "ops": rows })))
}

async fn callers_of(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let tenant = wanted(query, "tenant")?;
    let op = op_of(query.get("op").map(String::as_str).unwrap_or(""))?;
    let rows = who
        .door(c)?
        .read_present_in(&tenant, CALLERS_OF, &json!({"op": op}))
        .await?;
    Ok(Reply::json(json!({ "op": op, "callers": rows })))
}

#[derive(Deserialize)]
struct InviteBody {
    tenant: String,
    #[serde(default)]
    role: Option<String>,
    /// How long the invitation lasts, such as `7d`. Unset: the door's
    /// default.
    #[serde(default)]
    expires: Option<String>,
    /// A note for the person reading the list, such as a name.
    #[serde(default)]
    label: Option<String>,
}

/// Invite someone (`std.space.invite`). The answer carries the ticket
/// once; the console neither keeps nor audits it.
async fn people_invite(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: InviteBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let mut params = json!({});
    let role = match req.role.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => {
            let r = role_of(r)?;
            params["role"] = json!(r);
            r
        }
        None => "default".to_string(),
    };
    if let Some(e) = req
        .expires
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        let ok = e.len() <= 8
            && e.ends_with(['m', 'h', 'd'])
            && e[..e.len() - 1].chars().all(|ch| ch.is_ascii_digit())
            && e.len() > 1;
        if !ok {
            return Err(ConsoleError::BadRequest(
                "`expires` is a number and a unit, such as 30m, 12h or 7d".into(),
            ));
        }
        params["expires"] = json!(e);
    }
    if let Some(l) = req
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        if l.chars().count() > 256 || l.chars().any(char::is_control) {
            return Err(ConsoleError::BadRequest(
                "`label` must be at most 256 printable characters".into(),
            ));
        }
        params["label"] = json!(l);
    }
    call.detail = format!("tenant={tenant} role={role}");
    let rows = who.door(c)?.op_present_in(&tenant, INVITE, &params).await?;
    Ok(Reply::json(json!({ "invited": first(rows) })))
}

#[derive(Deserialize)]
struct RoleBody {
    tenant: String,
    member: String,
    role: String,
}

async fn people_set_role(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: RoleBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let member = id_of("member", &req.member)?;
    let role = role_of(&req.role)?;
    call.detail = format!("tenant={tenant} member={member} role={role}");
    let rows = who
        .door(c)?
        .op_present_in(&tenant, SET_ROLE, &json!({"member": member, "role": role}))
        .await?;
    Ok(Reply::json(json!({ "changed": first(rows) })))
}

#[derive(Deserialize)]
struct MemberBody {
    tenant: String,
    member: String,
}

async fn people_remove(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: MemberBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let member = id_of("member", &req.member)?;
    call.detail = format!("tenant={tenant} member={member}");
    let rows = who
        .door(c)?
        .op_present_in(&tenant, REMOVE, &json!({"member": member}))
        .await?;
    Ok(Reply::json(json!({ "removed": first(rows) })))
}

#[derive(Deserialize)]
struct GroupBody {
    tenant: String,
    group: String,
    member: String,
}

async fn people_add_to_group(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: GroupBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let group = id_of("group", &req.group)?;
    let member = id_of("member", &req.member)?;
    call.detail = format!("tenant={tenant} group={group} member={member}");
    let rows = who
        .door(c)?
        .op_present_in(
            &tenant,
            ADD_TO_GROUP,
            &json!({"group": group, "member": member}),
        )
        .await?;
    Ok(Reply::json(json!({ "added": first(rows) })))
}

async fn invitations(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let tenant = wanted(query, "tenant")?;
    let rows = who
        .door(c)?
        .read_present_in(&tenant, INVITATIONS, &json!({}))
        .await?;
    Ok(Reply::json(json!({ "invitations": rows })))
}

/// The tenant's access profile: one row an op, with the roles, groups and
/// members that reach it. `op` narrows it to one.
async fn profile(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let tenant = wanted(query, "tenant")?;
    let params = match query.get("op").map(|o| o.trim()).filter(|o| !o.is_empty()) {
        Some(o) => json!({ "op": op_of(o)? }),
        None => json!({}),
    };
    let rows = who
        .door(c)?
        .read_present_in(&tenant, PROFILE, &params)
        .await?;
    Ok(Reply::json(json!({ "ops": rows })))
}

async fn admission(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let tenant = wanted(query, "tenant")?;
    let rows = who
        .door(c)?
        .read_present_in(&tenant, ADMISSION, &json!({}))
        .await?;
    Ok(Reply::json(json!({ "admission": first(rows) })))
}

/// One write in a tenant, once the caller checked both touches: the detail
/// for the audit row, the op through the door, its first row.
async fn write_in(
    c: &Console,
    who: &Who,
    call: &mut Call,
    tenant: &str,
    op: &str,
    params: Value,
    detail: String,
) -> Result<Value, ConsoleError> {
    call.detail = detail;
    Ok(first(
        who.door(c)?.op_present_in(tenant, op, &params).await?,
    ))
}

async fn people_suspend(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    suspend_or_reinstate(c, call, headers, body, SUSPEND).await
}

async fn people_reinstate(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    suspend_or_reinstate(c, call, headers, body, REINSTATE).await
}

async fn suspend_or_reinstate(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
    op: &str,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: MemberBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let member = id_of("member", &req.member)?;
    let detail = format!("tenant={tenant} member={member}");
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        op,
        json!({"member": member}),
        detail,
    )
    .await?;
    Ok(Reply::json(json!({ "changed": row })))
}

#[derive(Deserialize)]
struct NewGroupBody {
    tenant: String,
    name: String,
}

async fn people_create_group(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: NewGroupBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let name = plain("name", &req.name, 120)?;
    if name.is_empty() {
        return Err(ConsoleError::BadRequest("a group needs a name".into()));
    }
    let detail = format!("tenant={tenant}");
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        CREATE_GROUP,
        json!({"name": name}),
        detail,
    )
    .await?;
    Ok(Reply::json(json!({ "created": row })))
}

#[derive(Deserialize)]
struct OneGroupBody {
    tenant: String,
    group: String,
}

async fn people_delete_group(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: OneGroupBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let group = id_of("group", &req.group)?;
    let detail = format!("tenant={tenant} group={group}");
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        DELETE_GROUP,
        json!({"group": group}),
        detail,
    )
    .await?;
    Ok(Reply::json(json!({ "deleted": row })))
}

async fn people_remove_from_group(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: GroupBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let group = id_of("group", &req.group)?;
    let member = id_of("member", &req.member)?;
    let detail = format!("tenant={tenant} group={group} member={member}");
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        REMOVE_FROM_GROUP,
        json!({"group": group, "member": member}),
        detail,
    )
    .await?;
    Ok(Reply::json(json!({ "removed": row })))
}

#[derive(Deserialize)]
struct OpAccessBody {
    tenant: String,
    op: String,
    #[serde(default)]
    roles: String,
    #[serde(default)]
    groups: String,
}

/// Which roles and groups may call an op in this tenant.
async fn people_set_op_access(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: OpAccessBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let op = op_of(&req.op)?;
    let roles = list_of("roles", &req.roles, true)?;
    let groups = list_of("groups", &req.groups, false)?;
    let detail = format!("tenant={tenant} op={op} roles={roles} groups={groups}");
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        SET_OP_ACCESS,
        json!({"op": op, "roles": roles, "groups": groups}),
        detail,
    )
    .await?;
    Ok(Reply::json(json!({ "set": row })))
}

#[derive(Deserialize)]
struct OpBody {
    tenant: String,
    op: String,
}

async fn people_reset_op_access(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: OpBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let op = op_of(&req.op)?;
    let detail = format!("tenant={tenant} op={op}");
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        RESET_OP_ACCESS,
        json!({"op": op}),
        detail,
    )
    .await?;
    Ok(Reply::json(json!({ "reset": row })))
}

#[derive(Deserialize)]
struct MemberAccessBody {
    tenant: String,
    member: String,
    op: String,
    effect: String,
}

/// One member's own allowance on an op: allow, deny, or back to default.
async fn people_set_member_access(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: MemberAccessBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let member = id_of("member", &req.member)?;
    let op = op_of(&req.op)?;
    let effect = req.effect.trim();
    if !["allow", "deny", "default"].contains(&effect) {
        return Err(ConsoleError::BadRequest(
            "`effect` is allow, deny or default".into(),
        ));
    }
    let detail = format!("tenant={tenant} member={member} op={op} effect={effect}");
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        SET_MEMBER_ACCESS,
        json!({"member": member, "op": op, "effect": effect}),
        detail,
    )
    .await?;
    Ok(Reply::json(json!({ "set": row })))
}

#[derive(Deserialize)]
struct InvitationBody {
    tenant: String,
    id: String,
}

async fn people_revoke_invitation(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: InvitationBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let id = id_of("id", &req.id)?;
    let detail = format!("tenant={tenant} invitation={id}");
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        REVOKE_INVITATION,
        json!({"id": id}),
        detail,
    )
    .await?;
    Ok(Reply::json(json!({ "revoked": row })))
}

/// The four ways in a tenant's admission names. Unset leaves one alone.
/// The second factor and outside sign-in connections stay with the app.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdmissionBody {
    tenant: String,
    #[serde(default)]
    open: Option<bool>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    providers: Option<String>,
    #[serde(default)]
    first_comer: Option<bool>,
}

async fn people_set_admission(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: AdmissionBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let mut params = json!({});
    let mut said = vec![format!("tenant={tenant}")];
    if let Some(o) = req.open {
        params["open"] = json!(o);
        said.push(format!("open={o}"));
    }
    if let Some(r) = &req.role {
        let r = role_of(r)?;
        said.push(format!("role={r}"));
        params["role"] = json!(r);
    }
    if let Some(p) = &req.providers {
        let p = plain("providers", p, 2048)?;
        said.push("providers=changed".into());
        params["providers"] = json!(p);
    }
    if let Some(f) = req.first_comer {
        params["firstComer"] = json!(f);
        said.push(format!("first_comer={f}"));
    }
    if said.len() == 1 {
        return Err(ConsoleError::BadRequest(
            "name at least one of open, role, providers or firstComer".into(),
        ));
    }
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        SET_ADMISSION,
        params,
        said.join(" "),
    )
    .await?;
    Ok(Reply::json(json!({ "admission": row })))
}

#[derive(Deserialize)]
struct RecoverBody {
    tenant: String,
    member: String,
    /// The case opened before, to collect its ticket once it matured.
    #[serde(default)]
    case: Option<String>,
}

/// Recover a member the tenant created. The answer may carry a ticket,
/// shown once like an invitation's; the console neither keeps nor audits
/// it.
async fn people_recover_member(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: RecoverBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let member = id_of("member", &req.member)?;
    let mut params = json!({"member": member});
    let mut detail = format!("tenant={tenant} member={member}");
    if let Some(case) = req.case.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        let case = id_of("case", case)?;
        detail.push_str(&format!(" case={case}"));
        params["case"] = json!(case);
    }
    let row = write_in(c, &who, call, &tenant, RECOVER_MEMBER, params, detail).await?;
    Ok(Reply::json(json!({ "recovery": row })))
}

#[derive(Deserialize)]
struct ApproveBody {
    tenant: String,
    member: String,
    case: String,
}

async fn people_approve_recovery(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: ApproveBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let member = id_of("member", &req.member)?;
    let case = id_of("case", &req.case)?;
    let detail = format!("tenant={tenant} member={member} case={case}");
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        APPROVE_MEMBER_RECOVERY,
        json!({"member": member, "case": case}),
        detail,
    )
    .await?;
    Ok(Reply::json(json!({ "approval": row })))
}

/// How recovery treats the people the tenant created. A tenant may only
/// tighten what the deployment sets; the door says so when it refuses.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryPolicyBody {
    tenant: String,
    #[serde(default)]
    hold: Option<bool>,
    #[serde(default)]
    delay_secs: Option<u64>,
    #[serde(default)]
    approvals: Option<u64>,
    #[serde(default)]
    path_enabled: Option<bool>,
    #[serde(default)]
    codes_enabled: Option<bool>,
    #[serde(default)]
    managers_may_recover: Option<bool>,
}

async fn people_recovery_policy(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Touch).await?;
    let req: RecoveryPolicyBody = parse(body)?;
    let tenant = id_of("tenant", &req.tenant)?;
    let mut params = serde_json::Map::new();
    let mut said = vec![format!("tenant={tenant}")];
    let flags = [
        ("hold", req.hold),
        ("pathEnabled", req.path_enabled),
        ("codesEnabled", req.codes_enabled),
        ("managersMayRecover", req.managers_may_recover),
    ];
    for (k, v) in flags {
        if let Some(v) = v {
            params.insert(k.into(), json!(v));
            said.push(format!("{k}={v}"));
        }
    }
    for (k, v) in [("delaySecs", req.delay_secs), ("approvals", req.approvals)] {
        if let Some(v) = v {
            params.insert(k.into(), json!(v));
            said.push(format!("{k}={v}"));
        }
    }
    if params.is_empty() {
        return Err(ConsoleError::BadRequest(
            "name at least one setting: hold, delaySecs, approvals, pathEnabled, codesEnabled \
             or managersMayRecover"
                .into(),
        ));
    }
    let row = write_in(
        c,
        &who,
        call,
        &tenant,
        SET_RECOVERY_POLICY,
        Value::Object(params),
        said.join(" "),
    )
    .await?;
    Ok(Reply::json(json!({ "policy": row })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_roles_are_what_the_door_takes() {
        assert_eq!(id_of("member", " mem_a-1 ").unwrap(), "mem_a-1");
        for bad in ["", "a b", "a/b", "é", &"x".repeat(129)] {
            assert!(id_of("member", bad).is_err(), "{bad}");
        }
        assert_eq!(role_of(" editor ").unwrap(), "editor");
        assert!(role_of("guest").is_err());
    }

    #[test]
    fn lists_ops_and_text_are_what_the_door_takes() {
        assert_eq!(
            list_of("roles", " owner, manager,owner ", true).unwrap(),
            "owner,manager"
        );
        assert!(list_of("roles", "owner,guest", true).is_err());
        assert_eq!(list_of("groups", "", false).unwrap(), "");
        assert!(list_of("groups", "a b", false).is_err());
        assert_eq!(op_of(" notes.list ").unwrap(), "notes.list");
        assert!(op_of("a b").is_err());
        assert!(plain("name", "Night\nshift", 120).is_err());
        assert!(plain("name", &"x".repeat(121), 120).is_err());
    }
}
