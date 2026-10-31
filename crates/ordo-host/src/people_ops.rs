//! G5 (`plan-console.md` 4.5, step 8): who is in a tenant and what they
//! may do, as four platform reads.
//!
//! | Op | Answers |
//! |---|---|
//! | `std.space.members($after?)` | each person's member id, principal, name, role, status, joined, and the groups they are in |
//! | `std.space.grants` | every live capability grant in the tenant: whose, which client, which ops, when given, when it ends |
//! | `std.space.groupMembers($group, $after?)` | who is in one group |
//! | `std.space.callableBy($member)` | every op that member may call, with each via (role, group, override, token, delegation) |
//!
//! # Who may see
//!
//! **Manager and owner, and the engine decides it.** None of the four is
//! named in [`crate::platform_ops::owner_only`], so the engine's platform
//! loop gives each to the two roles that administer the answer, the same
//! floor `std.space.callersOf` has and for its reason: the answer is a map
//! of the tenant's privilege structure, and `grants` names other people's
//! connected apps. A viewer or an editor is refused at the door with the
//! one `access_denied`; a person with no role in the tenant is refused the
//! same way. As reads they ask for no touch unless the deployment is
//! strict (`ORDO_PRESENCE_ON_READS`), and no delegated token may call them
//! unless the app developer names them in `ORDO_STEP_UP_EXEMPT_OPS`.
//!
//! **`callableBy` answers only what the asker could see.** It lists an op
//! only when the asker reaches it too ([`AccessEngine::membership_reaches`],
//! the predicate `callersOf` gates with). A manager asking about an owner
//! learns which of the manager's own ops the owner holds, never the ops a
//! manager cannot call. The member is named by member id; one this tenant
//! does not hold answers `not_found`, so the parameter learns nothing about
//! other tenants.
//!
//! # One source each
//!
//! - `members` and `groupMembers` read the tenant's own `members` and
//!   `groupMembers` rows, as `std.space.groups` does.
//! - `grants` reads the grant registry the door and `callersOf` read. A
//!   registry that cannot answer is a refusal, never an empty list.
//! - `callableBy` asks the engine the door decides with
//!   ([`AccessEngine::callable_by`], the body `callersOf` answers from, cut
//!   to one person).
//!
//! # Paging
//!
//! `members` and `groupMembers` answer at most [`PAGE`] rows in member id
//! order. A full page means there may be more: ask again with `$after` set
//! to the last member id.

use std::collections::BTreeMap;

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_exec::sql_lit;
use ordo_identity::principal::{PrincipalId, SpaceId, Timestamp};
use serde_json::{json, Value};

use crate::platform_ops::PlatformOps;

pub const MEMBERS: &str = "std.space.members";
pub const GRANTS: &str = "std.space.grants";
pub const GROUP_MEMBERS: &str = "std.space.groupMembers";
pub const CALLABLE_BY: &str = "std.space.callableBy";

/// Every op of this module.
pub const OPS: [&str; 4] = [MEMBERS, GRANTS, GROUP_MEMBERS, CALLABLE_BY];

/// The most rows one page of `members` or `groupMembers` holds.
pub const PAGE: usize = 500;

fn text<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn cell(r: &[Option<String>], i: usize) -> Option<&str> {
    r.get(i).and_then(|c| c.as_deref())
}

fn rows(rows: Vec<Value>) -> ExecResult {
    ExecResult {
        position: None,
        rows,
        next_cursor: None,
    }
}

fn not_found(what: &str) -> ExecError {
    ExecError::new(
        ErrorCode::NotFound,
        format!("this tenant has no such {what}"),
    )
}

/// An id spliced into a statement is held to the shape the door
/// addresses; anything else is not in this tenant.
fn id_param<'a>(bound: &'a BoundParams, name: &str, what: &str) -> Result<&'a str, ExecError> {
    let id = text(bound, name)
        .ok_or_else(|| ExecError::internal(format!("`${name}` did not reach the platform op")))?;
    if ordo_api::well_formed_space(id) {
        Ok(id)
    } else {
        Err(not_found(what))
    }
}

/// `$after`, when given: a member id, or nothing matches.
fn after(bound: &BoundParams) -> String {
    match text(bound, "after") {
        Some(a) => format!(r#" AND "id" > {}"#, sql_lit(a)),
        None => String::new(),
    }
}

/// Run one of the four. `caller` is the asker's member id in `space`.
pub(crate) async fn execute(
    ops: &PlatformOps,
    name: &str,
    space: &str,
    bound: &BoundParams,
    caller: &str,
) -> Result<ExecResult, ExecError> {
    match name {
        MEMBERS => members(ops, space, bound).await,
        GROUP_MEMBERS => group_members(ops, space, bound).await,
        GRANTS => grants(ops, space).await,
        CALLABLE_BY => callable_by(ops, space, bound, caller).await,
        other => Err(ExecError::internal(format!(
            "`{other}` is not a people read"
        ))),
    }
}

fn tenant_rows(ops: &PlatformOps) -> Result<&crate::member_ops::MemberOps, ExecError> {
    ops.members.as_deref().ok_or_else(|| {
        ExecError::internal(
            "this deployment registered the people reads and wired no membership state for them",
        )
    })
}

/// The groups each member is in, by member id.
async fn groups_of(
    pool: &ordo_exec::Pool,
    space: &str,
) -> Result<BTreeMap<String, Vec<String>>, ExecError> {
    let exists = pool
        .rows(r#"SELECT to_regclass('"groupMembers"') IS NOT NULL"#)
        .await
        .map_err(crate::op_store_unavailable)?;
    if !matches!(
        exists.first().and_then(|r| cell(r, 0)),
        Some("t") | Some("true")
    ) {
        return Ok(BTreeMap::new());
    }
    let links = pool
        .rows(&format!(
            r#"SELECT "member", "group" FROM "groupMembers"
               WHERE "$space" = {} ORDER BY "member", "group""#,
            sql_lit(space)
        ))
        .await
        .map_err(crate::op_store_unavailable)?;
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for l in &links {
        if let (Some(m), Some(g)) = (cell(l, 0), cell(l, 1)) {
            out.entry(m.to_string()).or_default().push(g.to_string());
        }
    }
    Ok(out)
}

fn person(r: &[Option<String>], groups: Option<&Vec<String>>) -> Value {
    json!({
        "member": cell(r, 0),
        "principal": cell(r, 1),
        "name": cell(r, 2),
        // A guest's seat has no role (`plan-sharing.md` §3.1).
        "role": cell(r, 3),
        "status": cell(r, 4),
        "joined": cell(r, 5),
        "groups": groups.cloned().unwrap_or_default(),
    })
}

const PERSON: &str = r#""id", "principal", "displayName", "role", "status", "joined""#;

/// `std.space.members`: the people, never the groups.
async fn members(
    ops: &PlatformOps,
    space: &str,
    bound: &BoundParams,
) -> Result<ExecResult, ExecError> {
    let pool = tenant_rows(ops)?.pool_of(space).await?;
    let found = pool
        .rows(&format!(
            r#"SELECT {PERSON} FROM "members"
               WHERE "$space" = {} AND "kind" <> 'group'{}
               ORDER BY "id" LIMIT {PAGE}"#,
            sql_lit(space),
            after(bound),
        ))
        .await
        .map_err(crate::op_store_unavailable)?;
    let groups = groups_of(&pool, space).await?;
    Ok(rows(
        found
            .iter()
            .map(|r| person(r, cell(r, 0).and_then(|id| groups.get(id))))
            .collect(),
    ))
}

/// `std.space.groupMembers($group)`: the group must be one of this
/// tenant's, or the answer is `not_found`.
async fn group_members(
    ops: &PlatformOps,
    space: &str,
    bound: &BoundParams,
) -> Result<ExecResult, ExecError> {
    let group = id_param(bound, "group", "group")?;
    let pool = tenant_rows(ops)?.pool_of(space).await?;
    let s = sql_lit(space);
    let g = sql_lit(group);
    let is_group = pool
        .rows(&format!(
            r#"SELECT 1 FROM "members" WHERE "$space" = {s} AND "id" = {g} AND "kind" = 'group'"#
        ))
        .await
        .map_err(crate::op_store_unavailable)?;
    if is_group.is_empty() {
        return Err(not_found("group"));
    }
    let found = pool
        .rows(&format!(
            r#"SELECT {PERSON} FROM "members"
               WHERE "$space" = {s}{} AND "id" IN (
                 SELECT "member" FROM "groupMembers" WHERE "$space" = {s} AND "group" = {g})
               ORDER BY "id" LIMIT {PAGE}"#,
            after(bound),
        ))
        .await
        .map_err(crate::op_store_unavailable)?;
    let groups = groups_of(&pool, space).await?;
    Ok(rows(
        found
            .iter()
            .map(|r| person(r, cell(r, 0).and_then(|id| groups.get(id))))
            .collect(),
    ))
}

/// `std.space.grants`: one row a grant, its ops gathered.
async fn grants(ops: &PlatformOps, space: &str) -> Result<ExecResult, ExecError> {
    let sid = SpaceId(space.to_string());
    let now = Timestamp((ops.now)());
    let records = ops
        .grants
        .live_with_prefix(&sid, "", now)
        .await
        .map_err(|e| {
            ExecError::internal(format!(
                "the grant registry could not answer, so the list of grants would have been \
                 incomplete; refusing rather than answering partially: {e}"
            ))
        })?;
    let engine = ops.engine();
    let mut by_id: BTreeMap<String, Value> = BTreeMap::new();
    for r in records {
        let row = by_id.entry(r.id.0.clone()).or_insert_with(|| {
            json!({
                "grant": r.id.0,
                "kind": if r.id.0.starts_with("shr_") { "share" } else { "token" },
                "principal": r.principal.0,
                "member": engine.member_id_of(&sid, &r.principal),
                "client": r.client.as_ref().map(|c| c.0.clone()),
                "ops": [],
                "grantedAt": r.granted_at.0,
                "expires": r.expires.0,
            })
        });
        if let Some(list) = row["ops"].as_array_mut() {
            list.push(json!(r.op));
        }
    }
    Ok(rows(by_id.into_values().collect()))
}

/// `std.space.callableBy($member)`, cut to the asker's own reach.
async fn callable_by(
    ops: &PlatformOps,
    space: &str,
    bound: &BoundParams,
    caller: &str,
) -> Result<ExecResult, ExecError> {
    let member = id_param(bound, "member", "member")?;
    let sid = SpaceId(space.to_string());
    // One snapshot for the resolution, the cut and the answer.
    let engine = ops.engine();
    let Some(target) = engine.principal_of_member(&sid, member).cloned() else {
        return Err(not_found("member"));
    };
    let asker = engine
        .principal_of_member(&sid, caller)
        .cloned()
        .unwrap_or_else(|| PrincipalId(caller.to_string()));
    let now = Timestamp((ops.now)());
    // The token column, for this one person. A registry that cannot answer
    // is a refusal: an answer missing its tokens while looking complete is
    // the failure `callersOf` exists to prevent.
    let records = ops.grants.grants_of(&sid, &target).await.map_err(|e| {
        ExecError::internal(format!(
            "the grant registry could not answer, so `callableBy` would have omitted every \
             op reached by a token; refusing rather than answering partially: {e}"
        ))
    })?;
    let delegations = match &ops.delegations {
        Some(reg) => reg.live_in(&sid, now).await.map_err(|e| {
            ExecError::internal(format!(
                "the delegation registry could not answer, so `callableBy` would have omitted \
                 every op reached on somebody else's behalf; refusing rather than answering \
                 partially: {e}"
            ))
        })?,
        None => Vec::new(),
    };
    let seen: Vec<String> = engine
        .deployed_ops()
        .filter(|op| engine.membership_reaches(&sid, &asker, op))
        .map(str::to_string)
        .collect();
    let answer = engine
        .callable_by(
            &sid,
            &target,
            seen.iter().map(String::as_str),
            &records,
            &delegations,
            now,
        )
        .into_iter()
        .map(|(op, via)| {
            json!({
                "op": op,
                "via": via.kind(),
                "detail": via.detail(),
            })
        })
        .collect();
    Ok(rows(answer))
}

/// The four specs.
pub fn specs() -> Vec<OpSpec> {
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let spec = |name: &str, params: Vec<ParamSpec>, out: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind: OpKind::Read,
        params,
        output_fields: out.iter().map(|f| FieldSpec::new(*f)).collect(),
        // Membership, profile and grant writes would all have to
        // invalidate it, as for `callersOf`; it claims no live capability
        // it cannot honour.
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        // A condition over `principal` would be a filtered enumeration.
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    let person = [
        "member",
        "principal",
        "name",
        "role",
        "status",
        "joined",
        "groups",
    ];
    vec![
        spec(MEMBERS, vec![text("after", 256, false)], &person),
        spec(
            GRANTS,
            Vec::new(),
            &[
                "grant",
                "kind",
                "principal",
                "member",
                "client",
                "ops",
                "grantedAt",
                "expires",
            ],
        ),
        spec(
            GROUP_MEMBERS,
            vec![text("group", 256, true), text("after", 256, false)],
            &person,
        ),
        spec(
            CALLABLE_BY,
            vec![text("member", 256, true)],
            &["op", "via", "detail"],
        ),
    ]
}
