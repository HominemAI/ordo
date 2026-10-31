//! A stand-in for a tenant's People ops (G5 and the writes the People
//! screen makes, step 8a and 8b), for the console's tests and its
//! end-to-end stack
//! (`examples/e2e_door.rs` includes this file).
//!
//! The door's check stays the real one (`ordo_host::IdentityAccess`, with
//! its home space and `console_keys::TENANT_OPS_OPEN`): this only answers
//! the ops once the door allowed them, over people in memory, in the
//! shapes `ordo-host` answers. `ordo-host/tests/people_reads.rs` covers the
//! real ones against Postgres.

#![allow(dead_code)]

use std::sync::Mutex;

use ordo_identity::{AccessEngine, AccessProfile, PrincipalId, Role, SpaceId};
use serde_json::{json, Value};

pub const MEMBERS: &str = "std.space.members";
pub const GROUPS: &str = "std.space.groups";
pub const GRANTS: &str = "std.space.grants";
pub const GROUP_MEMBERS: &str = "std.space.groupMembers";
pub const CALLABLE_BY: &str = "std.space.callableBy";
pub const CALLERS_OF: &str = "std.space.callersOf";
pub const INVITE: &str = "std.space.invite";
pub const SET_ROLE: &str = "std.space.setRole";
pub const REMOVE: &str = "std.space.remove";
pub const ADD_TO_GROUP: &str = "std.space.addToGroup";
pub const INVITATIONS: &str = "std.space.invitations";
pub const PROFILE: &str = "std.space.profile";
pub const ADMISSION: &str = "std.space.admission";
pub const SUSPEND: &str = "std.space.suspend";
pub const REINSTATE: &str = "std.space.reinstate";
pub const CREATE_GROUP: &str = "std.space.createGroup";
pub const DELETE_GROUP: &str = "std.space.deleteGroup";
pub const REMOVE_FROM_GROUP: &str = "std.space.removeFromGroup";
pub const SET_OP_ACCESS: &str = "std.space.setOpAccess";
pub const RESET_OP_ACCESS: &str = "std.space.resetOpAccess";
pub const SET_MEMBER_ACCESS: &str = "std.space.setMemberAccess";
pub const REVOKE_INVITATION: &str = "std.space.revokeInvitation";
pub const SET_ADMISSION: &str = "std.space.setAdmission";
pub const RECOVER_MEMBER: &str = "std.space.recoverMember";
pub const APPROVE_MEMBER_RECOVERY: &str = "std.space.approveMemberRecovery";
pub const SET_RECOVERY_POLICY: &str = "std.space.setRecoveryPolicy";

/// The reads: manager and owner, but admission, the owner's.
pub const READS: [&str; 9] = [
    MEMBERS,
    GROUPS,
    GRANTS,
    GROUP_MEMBERS,
    CALLABLE_BY,
    CALLERS_OF,
    INVITATIONS,
    PROFILE,
    ADMISSION,
];
/// The writes, each asking for a touch.
pub const WRITES: [&str; 17] = [
    INVITE,
    SET_ROLE,
    REMOVE,
    ADD_TO_GROUP,
    SUSPEND,
    REINSTATE,
    CREATE_GROUP,
    DELETE_GROUP,
    REMOVE_FROM_GROUP,
    SET_OP_ACCESS,
    RESET_OP_ACCESS,
    SET_MEMBER_ACCESS,
    REVOKE_INVITATION,
    SET_ADMISSION,
    RECOVER_MEMBER,
    APPROVE_MEMBER_RECOVERY,
    SET_RECOVERY_POLICY,
];
/// The owner's alone in a tenant, as `ordo_host::platform_ops::owner_only`
/// says.
const OWNER_ONLY: [&str; 7] = [
    SET_ROLE,
    REMOVE,
    SUSPEND,
    REINSTATE,
    ADMISSION,
    SET_ADMISSION,
    SET_RECOVERY_POLICY,
];

/// Every op this answers.
pub fn answers(op: &str) -> bool {
    READS.contains(&op) || WRITES.contains(&op)
}

/// The tenant whose people this holds: the one the deployment owner owns
/// in `tenants_door.rs` (Acme).
pub const TENANT: &str = "spc_mine";
/// The deployment owner's member id there.
pub const YOU: &str = "mem_you";
pub const MANAGER: &str = "mem_bea";
pub const EDITOR: &str = "mem_cal";
pub const VIEWER: &str = "mem_dee";
pub const GROUP: &str = "mem_grp_support";

/// The app's own ops, and the lowest role that holds each.
const APP_OPS: [(&str, &str); 4] = [
    ("notes.list", "viewer"),
    ("notes.create", "editor"),
    ("notes.delete", "manager"),
    ("billing.change", "owner"),
];

fn rank(role: &str) -> u8 {
    match role {
        "viewer" => 0,
        "editor" => 1,
        "manager" => 2,
        "owner" => 3,
        _ => 9,
    }
}

pub struct People {
    /// `(member, principal, name, role, status, joined)`.
    rows: Mutex<Vec<[String; 6]>>,
    /// `(group, name)`.
    groups: Mutex<Vec<(String, String)>>,
    /// `(group, member)`.
    links: Mutex<Vec<(String, String)>>,
    /// Open invitations, as `std.space.invitations` lists them.
    invitations: Mutex<Vec<Value>>,
    /// `(op, roles, groups)` the tenant set.
    op_access: Mutex<Vec<(String, String, String)>>,
    /// `(member, op, effect)`.
    overrides: Mutex<Vec<(String, String, String)>>,
    admission: Mutex<Value>,
    recovery_policy: Mutex<Value>,
    /// `(case, member, approvals)`.
    cases: Mutex<Vec<(String, String, u64)>>,
    next: Mutex<u64>,
}

impl Default for People {
    fn default() -> Self {
        People::new()
    }
}

fn seat(m: &str, p: &str, n: &str, r: &str) -> [String; 6] {
    [
        m.into(),
        p.into(),
        n.into(),
        r.into(),
        "active".into(),
        "2026-09-01".into(),
    ]
}

impl People {
    pub fn new() -> People {
        People {
            rows: Mutex::new(vec![
                seat(YOU, "usr_you", "You", "owner"),
                seat(MANAGER, "usr_bea", "Bea Lund", "manager"),
                seat(EDITOR, "usr_cal", "Cal Ortiz", "editor"),
                seat(VIEWER, "usr_dee", "Dee Park", "viewer"),
            ]),
            groups: Mutex::new(vec![(GROUP.into(), "Support".into())]),
            links: Mutex::new(vec![(GROUP.into(), EDITOR.into())]),
            invitations: Mutex::new(Vec::new()),
            op_access: Mutex::new(Vec::new()),
            overrides: Mutex::new(Vec::new()),
            admission: Mutex::new(json!({
                "open": false, "role": "viewer", "providers": "", "firstComer": false,
                "secondFactor": "inherit", "presenceFrom": "", "ceiling": "invitation,open,provider,first-comer",
            })),
            recovery_policy: Mutex::new(json!({
                "hold": false, "delaySecs": 0, "approvals": 1, "pathEnabled": true,
                "codesEnabled": true, "managersMayRecover": false,
            })),
            cases: Mutex::new(Vec::new()),
            next: Mutex::new(1),
        }
    }

    pub fn status_of(&self, member: &str) -> Option<String> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r[0] == member)
            .map(|r| r[4].clone())
    }

    pub fn group_names(&self) -> Vec<String> {
        self.groups
            .lock()
            .unwrap()
            .iter()
            .map(|(_, n)| n.clone())
            .collect()
    }

    pub fn admission(&self) -> Value {
        self.admission.lock().unwrap().clone()
    }

    pub fn recovery_policy(&self) -> Value {
        self.recovery_policy.lock().unwrap().clone()
    }

    pub fn role_of(&self, member: &str) -> Option<String> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r[0] == member)
            .map(|r| r[3].clone())
    }

    /// The door's rules: in [`TENANT`], the reads and invite and add to a
    /// group for manager and owner, role and removal for the owner, every
    /// write behind a touch; and the deployment owner holds the owner role
    /// there. Laid over whatever profile the tenant already has.
    pub fn grant(engine: &mut AccessEngine, owner: Option<&PrincipalId>) {
        let s = SpaceId(TENANT.into());
        let mut p = engine
            .profile(&s)
            .cloned()
            .unwrap_or_else(AccessProfile::new);
        for op in READS.iter().chain(WRITES.iter()) {
            p = p.allow(Role::Owner, op);
            if !OWNER_ONLY.contains(op) {
                p = p.allow(Role::Manager, op);
            }
        }
        engine.set_profile(&s, p);
        for op in WRITES {
            engine.require_step_up(op);
        }

        if let Some(o) = owner {
            engine.set_membership(&s, o, Role::Owner);
        }
    }

    fn person(&self, r: &[String; 6]) -> Value {
        let groups: Vec<String> = self
            .links
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, m)| *m == r[0])
            .map(|(g, _)| g.clone())
            .collect();
        json!({
            "member": r[0], "principal": r[1], "name": r[2], "role": r[3],
            "status": r[4], "joined": r[5], "groups": groups,
        })
    }

    /// Answer one op the door allowed. `Err` is the door's code and message.
    pub fn execute(
        &self,
        name: &str,
        space: &str,
        params: &serde_json::Map<String, Value>,
    ) -> Result<Vec<Value>, (String, String)> {
        let text = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let not_found = |m: &str| Err(("not_found".to_string(), m.to_string()));
        let refused = |m: &str| Err(("tenant_refused".to_string(), m.to_string()));
        if space != TENANT {
            return not_found("this tenant has no such member");
        }
        match name {
            MEMBERS => {
                let rows = self.rows.lock().unwrap().clone();
                Ok(rows.iter().map(|r| self.person(r)).collect())
            }
            GROUPS => {
                let links = self.links.lock().unwrap().clone();
                Ok(self
                    .groups
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(id, name)| {
                        let held: Vec<&String> = links
                            .iter()
                            .filter(|(g, _)| g == id)
                            .map(|(_, m)| m)
                            .collect();
                        json!({"group": id, "name": name, "members": held})
                    })
                    .collect())
            }
            GROUP_MEMBERS => {
                let group = text("group");
                if !self.has_group(&group) {
                    return not_found("this tenant has no such group");
                }
                let held: Vec<String> = self
                    .links
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(g, _)| *g == group)
                    .map(|(_, m)| m.clone())
                    .collect();
                let rows = self.rows.lock().unwrap().clone();
                Ok(rows
                    .iter()
                    .filter(|r| held.contains(&r[0]))
                    .map(|r| self.person(r))
                    .collect())
            }
            GRANTS => Ok(vec![json!({
                "grant": "grt_mail", "kind": "token", "principal": "usr_cal",
                "member": EDITOR, "client": "Mail helper", "ops": ["notes.list"],
                "grantedAt": 1_790_000_000u64, "expires": 1_900_000_000u64,
            })]),
            CALLABLE_BY => {
                let member = text("member");
                let Some(role) = self.role_of(&member) else {
                    return not_found("this tenant has no such member");
                };
                let mut out: Vec<Value> = APP_OPS
                    .iter()
                    .filter(|(_, floor)| rank(&role) >= rank(floor))
                    .map(|(op, _)| json!({"op": op, "via": "role", "detail": role}))
                    .collect();
                if member == EDITOR {
                    out.push(json!({"op": "notes.list", "via": "token",
                                    "detail": "grt_mail (client Mail helper)"}));
                }
                Ok(out)
            }
            CALLERS_OF => {
                let op = text("op");
                let Some((_, floor)) = APP_OPS.iter().find(|(o, _)| *o == op) else {
                    return not_found(
                        "no such op, or one you may not call; which answer this is, is \
                         deliberately not disclosed",
                    );
                };
                let rows = self.rows.lock().unwrap().clone();
                Ok(rows
                    .iter()
                    .filter(|r| rank(&r[3]) >= rank(floor))
                    .map(|r| json!({"principal": r[1], "via": "role", "detail": r[3]}))
                    .collect())
            }
            INVITE => {
                let mut n = self.next.lock().unwrap();
                let ticket = format!("inv_e2e_ticket_{n:04}");
                *n += 1;
                let role = text("role");
                self.invitations.lock().unwrap().push(json!({
                    "id": format!("{:016x}", 0xabc0_0000u64 + *n), "role": role,
                    "for": null, "label": text("label"), "issuedBy": YOU,
                    "issuedAt": 1_800_000_000u64, "expiresAt": 1_900_000_000u64,
                    "status": "open", "declinedAt": null,
                }));
                Ok(vec![json!({
                    "ticket": ticket, "expiresAt": 1_900_000_000u64,
                    "role": if role.is_empty() { Value::Null } else { json!(role) },
                    "outcome": "issued",
                })])
            }
            SET_ROLE => {
                let (member, role) = (text("member"), text("role"));
                let mut rows = self.rows.lock().unwrap();
                let Some(i) = rows.iter().position(|r| r[0] == member) else {
                    return not_found("this space has no such member");
                };
                let owners = rows.iter().filter(|r| r[3] == "owner").count();
                if rows[i][3] == "owner" && role != "owner" && owners == 1 {
                    return Err((
                        "tenant_refused".into(),
                        "that would leave the space with no active owner, which nothing can \
                         repair from inside. Make another member an owner first"
                            .into(),
                    ));
                }
                rows[i][3] = role.clone();
                Ok(vec![
                    json!({"member": member, "principal": rows[i][1], "role": role,
                               "status": "active", "outcome": "changed",
                               "door": "rebuilt here, announced to every replica"}),
                ])
            }
            REMOVE => {
                let member = text("member");
                let mut rows = self.rows.lock().unwrap();
                let Some(i) = rows.iter().position(|r| r[0] == member) else {
                    return not_found("this space has no such member");
                };
                if rows[i][3] == "owner" && rows.iter().filter(|r| r[3] == "owner").count() == 1 {
                    return Err((
                        "tenant_refused".into(),
                        "that would leave the space with no active owner, which nothing can \
                         repair from inside. Make another member an owner first"
                            .into(),
                    ));
                }
                let gone = rows.remove(i);
                self.links.lock().unwrap().retain(|(_, m)| *m != member);
                Ok(vec![
                    json!({"member": member, "principal": gone[1], "role": gone[3],
                               "status": "removed", "outcome": "removed",
                               "door": "rebuilt here, announced to every replica"}),
                ])
            }
            ADD_TO_GROUP => {
                let (group, member) = (text("group"), text("member"));
                if !self.has_group(&group) || self.role_of(&member).is_none() {
                    return not_found(
                        "this tenant has no such group or member. std.space.groups lists the \
                         groups",
                    );
                }
                let mut links = self.links.lock().unwrap();
                let outcome = if links.iter().any(|(g, m)| *g == group && *m == member) {
                    "already"
                } else {
                    links.push((group.clone(), member.clone()));
                    "added"
                };
                Ok(vec![
                    json!({"group": group, "member": member, "outcome": outcome,
                               "door": "rebuilt here, announced to every replica"}),
                ])
            }
            INVITATIONS => Ok(self.invitations.lock().unwrap().clone()),
            REVOKE_INVITATION => {
                let id = text("id");
                let mut all = self.invitations.lock().unwrap();
                let before = all.len();
                all.retain(|i| i["id"] != id.as_str());
                if all.len() == before {
                    return not_found("this tenant has no open invitation with that id");
                }
                Ok(vec![json!({"id": id, "outcome": "revoked"})])
            }
            SUSPEND | REINSTATE => {
                let member = text("member");
                let suspend = name == SUSPEND;
                let mut rows = self.rows.lock().unwrap();
                let Some(i) = rows.iter().position(|r| r[0] == member) else {
                    return not_found("this space has no such member");
                };
                let active_owners = rows
                    .iter()
                    .filter(|r| r[3] == "owner" && r[4] == "active")
                    .count();
                if suspend && rows[i][3] == "owner" && active_owners == 1 {
                    return refused(
                        "that would leave the space with no active owner, which nothing can \
                         repair from inside. Make another member an owner first",
                    );
                }
                rows[i][4] = if suspend { "suspended" } else { "active" }.into();
                Ok(vec![json!({
                    "member": member, "principal": rows[i][1], "role": rows[i][3],
                    "status": rows[i][4],
                    "outcome": if suspend { "suspended" } else { "reinstated" },
                    "door": "rebuilt here, announced to every replica",
                })])
            }
            CREATE_GROUP => {
                let mut n = self.next.lock().unwrap();
                let id = format!("mem_grp_{n:04}");
                *n += 1;
                let name = text("name");
                self.groups.lock().unwrap().push((id.clone(), name.clone()));
                Ok(vec![
                    json!({"group": id, "name": name, "outcome": "created",
                           "door": "rebuilt here, announced to every replica"}),
                ])
            }
            DELETE_GROUP => {
                let group = text("group");
                if !self.has_group(&group) {
                    return not_found("this tenant has no such group");
                }
                self.groups.lock().unwrap().retain(|(g, _)| *g != group);
                let mut links = self.links.lock().unwrap();
                let before = links.len();
                links.retain(|(g, _)| *g != group);
                Ok(vec![
                    json!({"group": group, "outcome": "deleted", "links": before - links.len(),
                           "door": "rebuilt here, announced to every replica"}),
                ])
            }
            REMOVE_FROM_GROUP => {
                let (group, member) = (text("group"), text("member"));
                if !self.has_group(&group) {
                    return not_found("this tenant has no such group");
                }
                self.links
                    .lock()
                    .unwrap()
                    .retain(|(g, m)| !(*g == group && *m == member));
                Ok(vec![
                    json!({"group": group, "member": member, "outcome": "removed",
                           "door": "rebuilt here, announced to every replica"}),
                ])
            }
            PROFILE => {
                let only = text("op");
                let set = self.op_access.lock().unwrap().clone();
                let over = self.overrides.lock().unwrap().clone();
                Ok(APP_OPS
                    .iter()
                    .filter(|(op, _)| only.is_empty() || *op == only)
                    .map(|(op, floor)| {
                        let mine = set.iter().find(|(o, _, _)| o == op);
                        let roles: Vec<&str> = match mine {
                            Some((_, r, _)) => r.split(',').filter(|r| !r.is_empty()).collect(),
                            None => ["owner", "manager", "editor", "viewer"]
                                .into_iter()
                                .filter(|r| rank(r) >= rank(floor))
                                .collect(),
                        };
                        let groups: Vec<&str> = mine
                            .map(|(_, _, g)| g.split(',').filter(|g| !g.is_empty()).collect())
                            .unwrap_or_default();
                        let side = |e: &str| -> Vec<String> {
                            over.iter()
                                .filter(|(_, o, eff)| o == op && eff == e)
                                .map(|(m, _, _)| m.clone())
                                .collect()
                        };
                        json!({"op": op, "roles": roles, "groups": groups,
                               "allow": side("allow"), "deny": side("deny"),
                               "changed": mine.is_some()})
                    })
                    .collect())
            }
            SET_OP_ACCESS | RESET_OP_ACCESS | SET_MEMBER_ACCESS => {
                let op = text("op");
                if !APP_OPS.iter().any(|(o, _)| *o == op) {
                    return not_found("this app has no such op");
                }
                let door = "rebuilt here, announced to every replica";
                match name {
                    SET_OP_ACCESS => {
                        let (roles, groups) = (text("roles"), text("groups"));
                        let mut set = self.op_access.lock().unwrap();
                        set.retain(|(o, _, _)| *o != op);
                        set.push((op.clone(), roles.clone(), groups.clone()));
                        Ok(vec![json!({"op": op, "roles": roles, "groups": groups,
                                       "outcome": "set", "door": door})])
                    }
                    RESET_OP_ACCESS => {
                        self.op_access.lock().unwrap().retain(|(o, _, _)| *o != op);
                        Ok(vec![
                            json!({"op": op, "outcome": "back to the app's default",
                                       "door": door}),
                        ])
                    }
                    _ => {
                        let (member, effect) = (text("member"), text("effect"));
                        if self.role_of(&member).is_none() {
                            return not_found("this space has no such member");
                        }
                        let mut over = self.overrides.lock().unwrap();
                        over.retain(|(m, o, _)| !(*m == member && *o == op));
                        if effect != "default" {
                            over.push((member.clone(), op.clone(), effect.clone()));
                        }
                        Ok(vec![json!({"member": member, "op": op, "effect": effect,
                                       "door": door})])
                    }
                }
            }
            ADMISSION => Ok(vec![self.admission()]),
            SET_ADMISSION => {
                let mut a = self.admission.lock().unwrap();
                for k in ["open", "firstComer"] {
                    if let Some(v) = params.get(k).and_then(Value::as_bool) {
                        a[k] = json!(v);
                    }
                }
                for k in ["role", "providers"] {
                    if let Some(v) = params.get(k).and_then(Value::as_str) {
                        a[k] = json!(v);
                    }
                }
                Ok(vec![a.clone()])
            }
            SET_RECOVERY_POLICY => {
                let mut p = self.recovery_policy.lock().unwrap();
                if params
                    .get("approvals")
                    .and_then(Value::as_u64)
                    .is_some_and(|a| a < 1)
                {
                    return refused(
                        "this deployment sets a floor of 1 approvals on a tenant's recovery \
                         of a member, and a tenant may only raise it",
                    );
                }
                for (k, v) in params {
                    if k != "tenant" {
                        p[k.as_str()] = v.clone();
                    }
                }
                let mut row = p.clone();
                row["space"] = json!(TENANT);
                row["outcome"] = json!("set");
                Ok(vec![row])
            }
            RECOVER_MEMBER => {
                let member = text("member");
                if self.role_of(&member).is_none_or(|r| r == "owner")
                    || self.recovery_policy()["pathEnabled"] == false
                {
                    return refused(
                        "a tenant may recover a member only when it created them, they belong \
                         nowhere else, and they are not an owner. Refer this person to the \
                         deployment owner",
                    );
                }
                let need = self.recovery_policy()["approvals"].as_u64().unwrap_or(1);
                let case_id = text("case");
                let mut cases = self.cases.lock().unwrap();
                if !case_id.is_empty() {
                    let Some(c) = cases.iter().find(|c| c.0 == case_id && c.1 == member) else {
                        return refused("that case is not one of this tenant's");
                    };
                    if c.2 < need {
                        return refused("that case has not matured, or already yielded its ticket");
                    }
                    return Ok(vec![json!({"case": case_id, "member": member,
                        "ticket": format!("rtk_e2e_{case_id}"), "ticketExpiresAt": 1_900_000_000u64,
                        "outcome": "ticket issued"})]);
                }
                let mut n = self.next.lock().unwrap();
                let case = format!("rec_e2e_{n:04}");
                *n += 1;
                cases.push((case.clone(), member.clone(), 1));
                let matured = need <= 1;
                Ok(vec![json!({
                    "case": case, "member": member, "principal": format!("usr_{member}"),
                    "state": if matured { "matured" } else { "pending" },
                    "maturesAt": 1_800_000_000u64, "approvalsRequired": need,
                    "ticket": if matured { json!(format!("rtk_e2e_{case}")) } else { Value::Null },
                    "ticketExpiresAt": if matured { json!(1_900_000_000u64) } else { Value::Null },
                    "outcome": "opened",
                })])
            }
            APPROVE_MEMBER_RECOVERY => {
                let (member, case) = (text("member"), text("case"));
                let need = self.recovery_policy()["approvals"].as_u64().unwrap_or(1);
                let mut cases = self.cases.lock().unwrap();
                let Some(c) = cases.iter_mut().find(|c| c.0 == case && c.1 == member) else {
                    return refused("that case is not one of this tenant's for that member");
                };
                c.2 += 1;
                Ok(vec![
                    json!({"case": case, "member": member, "approvals": c.2,
                    "approvalsRequired": need,
                    "outcome": if c.2 >= need { "approved" } else { "waiting for more approvals" }}),
                ])
            }
            other => not_found(&format!("no op {other}")),
        }
    }

    fn has_group(&self, group: &str) -> bool {
        self.groups.lock().unwrap().iter().any(|(g, _)| g == group)
    }
}
