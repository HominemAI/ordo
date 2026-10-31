//! A tenant's own **access profile**, and its **groups** (owner ruling
//! 2026-10-10, `op-role-limit` asks 2 and 4).
//!
//! Who may call which op is runtime state. The app developer ships a
//! default per op ([`crate::access_template`]); each tenant's profile starts
//! as that default and is then the tenant's own. A manager or owner may:
//!
//! | Op | Does |
//! |---|---|
//! | `std.space.profile` | lists every op the profile can set, with the roles, groups and member overrides that reach it |
//! | `std.space.setOpAccess` | sets which roles and groups may call one op, in this tenant only |
//! | `std.space.resetOpAccess` | puts one op back to the app developer's default |
//! | `std.space.setMemberAccess` | allows or denies one op for one member, on top of role and groups |
//! | `std.space.createGroup` | makes a named group (a `members` row with `kind: group`) |
//! | `std.space.addToGroup` / `removeFromGroup` | changes who is in a group |
//! | `std.space.deleteGroup` | deletes a group and its links as one command group; refused while a profile line or access grant names it |
//! | `std.space.groups` | lists the tenant's groups and who is in each |
//!
//! # Manager and owner
//!
//! `glossary.md`: a profile is *"owned by the space (managers tighten or
//! loosen; every change is a command)"*. Roles have no hierarchy, so the
//! two are listed explicitly. A group is a way of granting ops, so the
//! group commands are administered by the same two roles.
//!
//! # A manager hands out only what a manager holds
//!
//! Owner ruling 2026-10-10 (`op-role-limit` ask 5): a manager may change the
//! tenant's profile only to hand out ops they can call themselves. A change
//! that would let anyone (the manager, their role, a group, a person) call
//! an op the manager cannot call is refused, and the refusal names the op
//! and says only an owner can grant it. Narrowing is always allowed, and an
//! owner is not limited. The check lives in [`ProfileOps::limit`]: it asks
//! what the change adds, and holds the manager to it.
//!
//! # What a tenant cannot reach
//!
//! A profile lists the **program's** ops: the app's own and the derived
//! `std.<entity>.*`. Platform ops (`std.space.*` and the rest) and the
//! deployment's own surfaces are fixed by the platform and cannot be listed,
//! so an owner-only capability stays the owner's whatever a profile says.
//! What the deployment closes stays closed whatever a profile opens:
//! a delete closed to delegated tokens (`ORDO_DELEGABLE_OPS`) is closed
//! for every grant path, and an op's step-up requirement is the op's.
//!
//! # Every change is a command in the tenant's chain
//!
//! Each command runs inside `Executor::platform_write` on the tenant's own
//! executor, so its command group is appended to the tenant's chain in the
//! transaction that wrote the row (`ordo_conformance::access_profile`,
//! `ordo_conformance::membership`). The door is then rebuilt: every
//! tenant's profile is read again from its own rows, so the change reaches
//! this tenant on the next request and no other tenant's profile moves.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_conformance::access_profile::{ProfileCommand, MEMBERACCESS_DDL, OPACCESS_DDL};
use ordo_conformance::membership::GroupCommand;
use ordo_conformance::write::WriteDb;
use ordo_exec::{Pool, PoolError};
use ordo_identity::principal::{AccessProfile, GroupId, PrincipalId, Role, SpaceId};
use ordo_identity::AccessEngine;
use serde_json::{json, Value};

use crate::access_template::role_of;
use crate::member_ops::MemberOps;

pub const PROFILE: &str = "std.space.profile";
pub const SET_OP_ACCESS: &str = "std.space.setOpAccess";
pub const RESET_OP_ACCESS: &str = "std.space.resetOpAccess";
pub const SET_MEMBER_ACCESS: &str = "std.space.setMemberAccess";
pub const CREATE_GROUP: &str = "std.space.createGroup";
pub const ADD_TO_GROUP: &str = "std.space.addToGroup";
pub const REMOVE_FROM_GROUP: &str = "std.space.removeFromGroup";
pub const DELETE_GROUP: &str = "std.space.deleteGroup";
pub const GROUPS: &str = "std.space.groups";

/// Every op of this module.
pub const OPS: [&str; 9] = [
    PROFILE,
    SET_OP_ACCESS,
    RESET_OP_ACCESS,
    SET_MEMBER_ACCESS,
    CREATE_GROUP,
    ADD_TO_GROUP,
    REMOVE_FROM_GROUP,
    DELETE_GROUP,
    GROUPS,
];

/// The two reads.
pub fn is_read(op: &str) -> bool {
    op == PROFILE || op == GROUPS
}

// ── The tenants' rows, as the door reads them ─────────────────────────────

/// Group links: `(space, group member id, member id)`.
pub const LINKS_SQL: &str = r#"SELECT "$space", "group", "member" FROM "groupMembers" ORDER BY "$space", "group", "member""#;
/// The tenants' own op lines: `(space, op, roles, groups)`.
pub const LINES_SQL: &str =
    r#"SELECT "$space", "op", "roles", "groups" FROM "$opaccess" ORDER BY "$space", "op""#;
/// The tenants' per-member overrides: `(space, member id, op, effect)`.
pub const OVERRIDES_SQL: &str = r#"SELECT "$space", "member", "op", "effect" FROM "$memberaccess" ORDER BY "$space", "member", "op""#;

/// What every tenant changed about its profile, and who is in its groups.
/// Read beside `members` wherever the door is built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TenantAccess {
    pub links: Vec<Vec<Option<String>>>,
    pub lines: Vec<Vec<Option<String>>>,
    pub overrides: Vec<Vec<Option<String>>>,
}

fn exists_sql(table: &str) -> String {
    format!(r#"SELECT to_regclass('"{table}"') IS NOT NULL"#)
}

fn truthy(rows: &[Vec<Option<String>>]) -> bool {
    matches!(
        rows.first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref()),
        Some("t") | Some("true")
    )
}

impl TenantAccess {
    /// From one database, over the pool. A table that does not exist yet
    /// (no tenant has changed anything) reads as empty.
    pub async fn read(pool: &Pool) -> Result<TenantAccess, PoolError> {
        let mut out = TenantAccess::default();
        for (table, sql, slot) in [
            ("groupMembers", LINKS_SQL, &mut out.links),
            ("$opaccess", LINES_SQL, &mut out.lines),
            ("$memberaccess", OVERRIDES_SQL, &mut out.overrides),
        ] {
            if truthy(&pool.rows(&exists_sql(table)).await?) {
                *slot = pool.rows(sql).await?;
            }
        }
        Ok(out)
    }

    /// From one database, over the conformance harness.
    pub fn read_pg(pg: &ordo_conformance::Pg) -> Result<TenantAccess, String> {
        let mut out = TenantAccess::default();
        for (table, sql, slot) in [
            ("groupMembers", LINKS_SQL, &mut out.links),
            ("$opaccess", LINES_SQL, &mut out.lines),
            ("$memberaccess", OVERRIDES_SQL, &mut out.overrides),
        ] {
            if truthy(&pg.query(&exists_sql(table))?) {
                *slot = pg.query(sql)?;
            }
        }
        Ok(out)
    }

    /// Keep the rows of the spaces `keep` names.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        let ok = |r: &Vec<Option<String>>| r.first().and_then(|c| c.as_deref()).is_some_and(&keep);
        self.links.retain(ok);
        self.lines.retain(ok);
        self.overrides.retain(ok);
    }

    /// Add another database's rows.
    pub fn extend(&mut self, other: TenantAccess) {
        self.links.extend(other.links);
        self.lines.extend(other.lines);
        self.overrides.extend(other.overrides);
    }
}

fn cell(r: &[Option<String>], i: usize) -> &str {
    r.get(i).and_then(|c| c.as_deref()).unwrap_or("")
}

fn words(csv: &str) -> impl Iterator<Item = &str> {
    csv.split(',').map(str::trim).filter(|w| !w.is_empty())
}

/// Put every tenant's group links into the engine. Called once the members
/// are in it, because a group cannot hold a principal the space does not.
pub fn install_groups(engine: &mut AccessEngine, tenant: &TenantAccess) {
    for r in &tenant.links {
        let space = SpaceId(cell(r, 0).to_string());
        let Some(principal) = engine.principal_of_member(&space, cell(r, 2)).cloned() else {
            continue;
        };
        engine.add_to_group(&space, &principal, &GroupId(cell(r, 1).to_string()));
    }
}

/// One tenant's profile: the deployment's `base` with the tenant's own
/// lines and overrides laid over it. `program` is the ops a profile can
/// list; a line for an op the program no longer has is ignored.
pub fn tenant_profile(
    engine: &AccessEngine,
    base: &AccessProfile,
    space: &str,
    tenant: &TenantAccess,
    program: &BTreeSet<String>,
) -> AccessProfile {
    let mut profile = base.clone();
    for r in tenant.lines.iter().filter(|r| cell(r, 0) == space) {
        let op = cell(r, 1);
        if !program.contains(op) {
            continue;
        }
        for ops in profile.ops_by_role.values_mut() {
            ops.remove(op);
        }
        for ops in profile.ops_by_group.values_mut() {
            ops.remove(op);
        }
        for role in words(cell(r, 2)).filter_map(role_of) {
            profile = profile.allow(role, op);
        }
        for group in words(cell(r, 3)) {
            profile = profile.allow_group(&GroupId(group.to_string()), op);
        }
    }
    let sid = SpaceId(space.to_string());
    for r in tenant.overrides.iter().filter(|r| cell(r, 0) == space) {
        let op = cell(r, 2);
        if !program.contains(op) {
            continue;
        }
        let Some(principal) = engine.principal_of_member(&sid, cell(r, 1)).cloned() else {
            continue;
        };
        profile = match cell(r, 3) {
            "allow" => profile.override_allow(&principal, op),
            "deny" => profile.override_deny(&principal, op),
            _ => profile,
        };
    }
    profile
}

// ── The ops ───────────────────────────────────────────────────────────────

pub struct ProfileOps {
    /// The resolver every op call goes through: the program being served,
    /// cut to the tenant, its executor and its chain.
    pub spaces: Arc<crate::spaces::Spaces>,
    /// The record, and the door's rebuild.
    pub members: Arc<MemberOps>,
    /// The engine the door decides with now.
    pub engine: Arc<dyn Fn() -> Arc<AccessEngine> + Send + Sync>,
}

fn refused(msg: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, msg.into())
}

fn unavailable(e: impl crate::Faulty) -> ExecError {
    crate::op_store_unavailable(e)
}

fn text<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn required<'a>(bound: &'a BoundParams, name: &str) -> Result<&'a str, ExecError> {
    text(bound, name)
        .ok_or_else(|| ExecError::internal(format!("`${name}` did not reach the platform op")))
}

/// What a protocol message becomes on the wire. The protocol speaks in
/// `class: message`.
fn refusal_of(m: &str) -> ExecError {
    if m == ordo_conformance::write::NOT_FOUND {
        return ExecError::new(
            ErrorCode::NotFound,
            "this tenant has no such group or member. std.space.groups lists the groups",
        );
    }
    if let Some(rest) = m.strip_prefix("not_found: ") {
        return ExecError::new(ErrorCode::NotFound, rest.to_string());
    }
    if m.starts_with("duplicate:") {
        return ExecError::new(ErrorCode::Duplicate, m.to_string());
    }
    if m.starts_with("group_nesting:")
        || m.starts_with("group_in_use:")
        || m.starts_with("access_guard:")
    {
        return refused(m.to_string());
    }
    unavailable(m)
}

/// The role words, in their fixed order.
const ROLE_ORDER: [Role; 4] = [Role::Owner, Role::Manager, Role::Editor, Role::Viewer];

impl ProfileOps {
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op {
            PROFILE => self.profile(space, bound).await,
            SET_OP_ACCESS => self.set_op(space, bound, caller).await,
            RESET_OP_ACCESS => self.reset_op(space, bound, caller).await,
            SET_MEMBER_ACCESS => self.set_member(space, bound, caller).await,
            CREATE_GROUP => {
                let name = required(bound, "name")?.to_string();
                self.group(space, caller, Group::Create(name)).await
            }
            ADD_TO_GROUP | REMOVE_FROM_GROUP => {
                let group = required(bound, "group")?.to_string();
                let member = required(bound, "member")?.to_string();
                let cmd = if op == ADD_TO_GROUP {
                    Group::Add(group, member)
                } else {
                    Group::Remove(group, member)
                };
                self.group(space, caller, cmd).await
            }
            DELETE_GROUP => {
                let group = required(bound, "group")?.to_string();
                self.group(space, caller, Group::Delete(group)).await
            }
            GROUPS => self.groups(space).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a profile command"
            ))),
        }
    }

    /// The ops a profile in this tenant can list: the program's own.
    async fn program(&self, space: &str) -> Result<BTreeSet<String>, ExecError> {
        let bound = self.spaces.resolve(space).await.map_err(unavailable)?;
        Ok(crate::build_specs(&bound.view.fixture)
            .into_iter()
            .map(|s| s.name)
            .collect())
    }

    /// `op` is one a tenant's profile can list, or the refusal says what
    /// to change.
    async fn listable(&self, space: &str, op: &str) -> Result<(), ExecError> {
        if crate::session_policy::TEMPLATE_GOVERNED.contains(&op) {
            return Err(refused(format!(
                "`{op}` is the app developer's to open or close, for every tenant at once: \
                 name it in {} (for example {op}=owner)",
                crate::access_template::ENV
            )));
        }
        if crate::platform_ops::is_platform_op(op) || crate::access_template::reserved(op) {
            return Err(refused(format!(
                "`{op}` is a platform op. Who may call it is fixed by the platform and is not \
                 part of a tenant's profile. A profile lists this app's own ops; \
                 std.space.profile names them"
            )));
        }
        if !self.program(space).await?.contains(op) {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                format!(
                    "this app has no op `{op}`. std.space.profile lists the ops a profile can set"
                ),
            ));
        }
        Ok(())
    }

    /// The manager's limit. `gained` is every op the change would let
    /// somebody call who could not before. An owner is not limited; a
    /// manager must be able to call each one.
    fn limit(
        &self,
        space: &str,
        caller: &str,
        gained: impl FnOnce(&AccessEngine, &AccessProfile) -> BTreeSet<String>,
    ) -> Result<(), ExecError> {
        let engine = (self.engine)();
        let sid = SpaceId(space.to_string());
        let who = PrincipalId(caller.to_string());
        let Some(profile) = engine.profile(&sid) else {
            return Ok(());
        };
        let Some((role, groups)) = engine.standing(&sid, &who) else {
            return Ok(());
        };
        let owner = role == Some(Role::Owner)
            || groups
                .iter()
                .any(|g| profile.role_by_group.get(g) == Some(&Role::Owner));
        if owner {
            return Ok(());
        }
        let beyond: Vec<String> = gained(&engine, profile)
            .into_iter()
            .filter(|op| !engine.membership_reaches(&sid, &who, op))
            .collect();
        if beyond.is_empty() {
            return Ok(());
        }
        let names: Vec<String> = beyond.iter().map(|o| format!("`{o}`")).collect();
        let them = if beyond.len() == 1 { "it" } else { "them" };
        Err(refused(format!(
            "you cannot call {} yourself, so you cannot hand {them} to anyone. A manager can \
             only give out ops the manager holds; only an owner can grant {them}. Ask an owner, \
             or narrow the change to ops you hold",
            names.join(", "),
        )))
    }

    async fn ensure_tables(pool: &Pool) -> Result<(), ExecError> {
        for ddl in [OPACCESS_DDL, MEMBERACCESS_DDL] {
            if pool.rows(ddl).await.is_err() {
                pool.rows(ddl).await.map_err(unavailable)?;
            }
        }
        Ok(())
    }

    /// Run one protocol command in the tenant's chain. Retried from scratch
    /// on a serialization conflict.
    async fn write<W>(&self, space: &str, work: W) -> Result<Value, ExecError>
    where
        W: Fn(
                &ordo_conformance::Fixture,
                &dyn WriteDb,
                &mut ordo_conformance::WriteLog,
            ) -> Result<Value, String>
            + Clone
            + Send
            + 'static,
    {
        let bound = self.spaces.resolve(space).await.map_err(unavailable)?;
        Self::ensure_tables(&bound.view.exec.pool).await?;
        let mut tries = 0;
        loop {
            tries += 1;
            let f = bound.view.fixture.clone();
            let work = work.clone();
            let result = bound
                .view
                .exec
                .platform_write(
                    bound.log.clone(),
                    move |db: &dyn WriteDb, log: &mut ordo_conformance::WriteLog| work(&f, db, log),
                )
                .await;
            match result {
                Ok(v) => return Ok(v),
                Err(ordo_exec::ExecFail::Pool(PoolError::Serialization(_))) if tries < 3 => {
                    continue
                }
                Err(ordo_exec::ExecFail::Compile(m)) => return Err(refusal_of(&m)),
                Err(e) => return Err(unavailable(e)),
            }
        }
    }

    async fn set_op(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let op = required(bound, "op")?.to_string();
        self.listable(space, &op).await?;
        let mut roles: Vec<String> = Vec::new();
        for word in words(text(bound, "roles").unwrap_or("")) {
            if role_of(word).is_none() {
                return Err(refused(format!(
                    "`{word}` is not a role. The roles are owner, manager, editor and viewer. \
                     For a set of people of your own, make a group with std.space.createGroup \
                     and name it in `groups`"
                )));
            }
            if !roles.iter().any(|r| r == word) {
                roles.push(word.to_string());
            }
        }
        roles.sort_by_key(|r| role_of(r).map(Role::level).map(std::cmp::Reverse));
        let mut groups: Vec<String> = words(text(bound, "groups").unwrap_or(""))
            .map(str::to_string)
            .collect();
        groups.sort();
        groups.dedup();
        self.limit(space, caller, |_, p| {
            let new_role = roles
                .iter()
                .filter_map(|r| role_of(r))
                .any(|r| !p.ops_by_role.get(&r).is_some_and(|ops| ops.contains(&op)));
            let new_group = groups.iter().any(|g| {
                !p.ops_by_group
                    .get(&GroupId(g.clone()))
                    .is_some_and(|ops| ops.contains(&op))
            });
            if new_role || new_group {
                BTreeSet::from([op.clone()])
            } else {
                BTreeSet::new()
            }
        })?;
        let actor = caller.to_string();
        let (o, r, g) = (op.clone(), roles.clone(), groups.clone());
        self.write(space, move |f, db, log| {
            f.profile_command_on(
                db,
                log,
                ProfileCommand::SetOp {
                    op: &o,
                    roles: &r,
                    groups: &g,
                },
                Some(&actor),
            )
        })
        .await?;
        self.members.note(
            space,
            caller,
            "profile-op-set",
            format!(
                "op={op} roles={} groups={}",
                roles.join(","),
                groups.join(",")
            ),
        );
        let door = self.members.rebuilt(space).await;
        Ok(one(json!({
            "op": op,
            "roles": roles,
            "groups": groups,
            "outcome": "set",
            "door": door,
        })))
    }

    async fn reset_op(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let op = required(bound, "op")?.to_string();
        self.listable(space, &op).await?;
        // A reset puts the app developer's default back, so it hands the op
        // to whichever default roles do not hold it now, and drops the
        // groups (which only narrows).
        let writes = {
            let bound = self.spaces.resolve(space).await.map_err(unavailable)?;
            crate::build_specs(&bound.view.fixture)
                .iter()
                .find(|s| s.name == op)
                .is_some_and(|s| !matches!(s.kind, ordo_api::OpKind::Read))
        };
        let defaults =
            crate::access_template::default_roles(&crate::access_template::current(), &op, writes);
        self.limit(space, caller, |_, p| {
            let new_role = defaults
                .iter()
                .any(|r| !p.ops_by_role.get(r).is_some_and(|ops| ops.contains(&op)));
            if new_role {
                BTreeSet::from([op.clone()])
            } else {
                BTreeSet::new()
            }
        })?;
        let actor = caller.to_string();
        let o = op.clone();
        self.write(space, move |f, db, log| {
            f.profile_command_on(db, log, ProfileCommand::ResetOp { op: &o }, Some(&actor))
        })
        .await?;
        self.members
            .note(space, caller, "profile-op-reset", format!("op={op}"));
        let door = self.members.rebuilt(space).await;
        Ok(one(json!({
            "op": op,
            "outcome": "back to the app's default",
            "door": door,
        })))
    }

    async fn set_member(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let member = required(bound, "member")?.to_string();
        let op = required(bound, "op")?.to_string();
        let effect = match required(bound, "effect")? {
            "allow" => Some("allow"),
            "deny" => Some("deny"),
            "default" => None,
            other => {
                return Err(refused(format!(
                    "`{other}` is not an effect. Use allow, deny, or default to remove the \
                     member's override"
                )))
            }
        };
        self.listable(space, &op).await?;
        self.limit(space, caller, |engine, p| {
            let sid = SpaceId(space.to_string());
            if effect == Some("deny") {
                return BTreeSet::new();
            }
            let Some(target) = engine.principal_of_member(&sid, &member).cloned() else {
                return BTreeSet::new();
            };
            let Some((Some(role), groups)) = engine.standing(&sid, &target) else {
                return BTreeSet::new();
            };
            let before = p.effective_ops(&target, role, &groups);
            let mut after = p.clone();
            let over = after.overrides.entry(target.clone()).or_default();
            over.deny.remove(&op);
            over.allow.remove(&op);
            if effect == Some("allow") {
                over.allow.insert(op.clone());
            }
            after
                .effective_ops(&target, role, &groups)
                .difference(&before)
                .cloned()
                .collect()
        })?;
        let actor = caller.to_string();
        let (m, o) = (member.clone(), op.clone());
        self.write(space, move |f, db, log| {
            f.profile_command_on(
                db,
                log,
                ProfileCommand::SetMember {
                    member: &m,
                    op: &o,
                    effect,
                },
                Some(&actor),
            )
        })
        .await?;
        let effect = effect.unwrap_or("default");
        self.members.note(
            space,
            caller,
            "profile-member-set",
            format!("member={member} op={op} effect={effect}"),
        );
        let door = self.members.rebuilt(space).await;
        Ok(one(json!({
            "member": member,
            "op": op,
            "effect": effect,
            "door": door,
        })))
    }

    async fn group(&self, space: &str, caller: &str, cmd: Group) -> Result<ExecResult, ExecError> {
        if let Group::Create(name) = &cmd {
            if name.chars().count() > 120 {
                return Err(refused("a group's name is at most 120 characters"));
            }
        }
        if let Group::Add(group, member) = &cmd {
            self.limit(space, caller, |engine, p| {
                let sid = SpaceId(space.to_string());
                let Some(target) = engine.principal_of_member(&sid, member).cloned() else {
                    return BTreeSet::new();
                };
                let Some((Some(role), groups)) = engine.standing(&sid, &target) else {
                    return BTreeSet::new();
                };
                let before = p.effective_ops(&target, role, &groups);
                let mut joined = groups.clone();
                joined.insert(GroupId(group.clone()));
                p.effective_ops(&target, role, &joined)
                    .difference(&before)
                    .cloned()
                    .collect()
            })?;
        }
        let actor = caller.to_string();
        let c = cmd.clone();
        let out = self
            .write(space, move |f, db, log| {
                let g = match &c {
                    Group::Create(name) => GroupCommand::Create { display_name: name },
                    Group::Add(group, member) => GroupCommand::Add { group, member },
                    Group::Remove(group, member) => GroupCommand::Remove { group, member },
                    Group::Delete(group) => GroupCommand::Delete { group },
                };
                let out = f.group_command_on(db, log, g, Some(&actor))?;
                // The summary names what changed, so live reads over
                // `members` and `groupMembers` are told.
                Ok(json!([
                    { "entity": "members", "result": out },
                    { "entity": "groupMembers" },
                ]))
            })
            .await?;
        let result = out
            .as_array()
            .and_then(|a| a.first())
            .and_then(|s| s.get("result"))
            .cloned()
            .unwrap_or(Value::Null);
        let (act, detail, row) = match &cmd {
            Group::Create(name) => {
                let id = result["created"].as_str().unwrap_or_default().to_string();
                (
                    "group-created",
                    format!("group={id} name={name}"),
                    json!({ "group": id, "name": name, "outcome": "created" }),
                )
            }
            Group::Add(group, member) => (
                "group-member-added",
                format!("group={group} member={member}"),
                json!({ "group": group, "member": member, "outcome": "added" }),
            ),
            Group::Remove(group, member) => (
                "group-member-removed",
                format!("group={group} member={member}"),
                json!({ "group": group, "member": member, "outcome": "removed" }),
            ),
            Group::Delete(group) => (
                "group-deleted",
                format!("group={group}"),
                json!({
                    "group": group,
                    "outcome": "deleted",
                    "links": result["links"],
                }),
            ),
        };
        self.members.note(space, caller, act, detail);
        let mut row = row;
        row["door"] = json!(self.members.rebuilt(space).await);
        Ok(one(row))
    }

    /// `std.space.groups`: each group, its name, and who is in it.
    async fn groups(&self, space: &str) -> Result<ExecResult, ExecError> {
        let pool = self.members.pool_of(space).await?;
        let s = ordo_exec::sql_lit(space);
        let groups = pool
            .rows(&format!(
                r#"SELECT "id", "displayName" FROM "members"
                   WHERE "$space" = {s} AND "kind" = 'group' ORDER BY "displayName", "id""#
            ))
            .await
            .map_err(unavailable)?;
        let links = pool
            .rows(&format!(
                r#"SELECT "group", "member" FROM "groupMembers"
                   WHERE "$space" = {s} ORDER BY "group", "member""#
            ))
            .await
            .map_err(unavailable)?;
        let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for l in &links {
            members
                .entry(cell(l, 0).to_string())
                .or_default()
                .push(cell(l, 1).to_string());
        }
        let rows = groups
            .iter()
            .map(|g| {
                let id = cell(g, 0).to_string();
                let held = members.remove(&id).unwrap_or_default();
                json!({
                    "group": id,
                    "name": cell(g, 1),
                    "members": held,
                })
            })
            .collect();
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }

    /// `std.space.profile`: for every op the profile can list, the roles,
    /// groups and member overrides that reach it in this tenant, and
    /// whether the tenant set the line itself. Read from the engine the
    /// door decides with.
    async fn profile(&self, space: &str, bound: &BoundParams) -> Result<ExecResult, ExecError> {
        let only = text(bound, "op");
        let program = self.program(space).await?;
        let engine = (self.engine)();
        let sid = SpaceId(space.to_string());
        let empty = AccessProfile::new();
        let profile = engine.profile(&sid).unwrap_or(&empty);
        let member_of: BTreeMap<PrincipalId, String> =
            engine.members_of(&sid).into_iter().collect();
        let changed: BTreeSet<String> = {
            let pool = self.members.pool_of(space).await?;
            if truthy(
                &pool
                    .rows(&exists_sql("$opaccess"))
                    .await
                    .map_err(unavailable)?,
            ) {
                pool.rows(&format!(
                    r#"SELECT "op" FROM "$opaccess" WHERE "$space" = {}"#,
                    ordo_exec::sql_lit(space)
                ))
                .await
                .map_err(unavailable)?
                .iter()
                .map(|r| cell(r, 0).to_string())
                .collect()
            } else {
                BTreeSet::new()
            }
        };
        let rows = program
            .iter()
            .filter(|op| only.is_none_or(|o| o == op.as_str()))
            .map(|op| {
                let roles: Vec<&str> = ROLE_ORDER
                    .iter()
                    .filter(|r| profile.allows(**r, op))
                    .map(|r| r.as_str())
                    .collect();
                let groups: Vec<&str> = profile
                    .ops_by_group
                    .iter()
                    .filter(|(_, ops)| ops.contains(op))
                    .map(|(g, _)| g.0.as_str())
                    .collect();
                let side = |deny: bool| -> Vec<&str> {
                    profile
                        .overrides
                        .iter()
                        .filter(|(_, o)| {
                            if deny {
                                o.deny.contains(op)
                            } else {
                                o.allow.contains(op)
                            }
                        })
                        .filter_map(|(p, _)| member_of.get(p).map(String::as_str))
                        .collect()
                };
                json!({
                    "op": op,
                    "roles": roles,
                    "groups": groups,
                    "allow": side(false),
                    "deny": side(true),
                    "changed": changed.contains(op),
                })
            })
            .collect();
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }
}

#[derive(Debug, Clone)]
enum Group {
    Create(String),
    Add(String, String),
    Remove(String, String),
    Delete(String),
}

fn one(row: Value) -> ExecResult {
    ExecResult {
        position: None,
        rows: vec![row],
        next_cursor: None,
    }
}

/// The specs of this module's ops. Every parameter is text: an op name, a
/// member or group id, or a comma-separated list of roles or group ids.
pub fn specs() -> Vec<ordo_api::OpSpec> {
    use ordo_api::{
        FieldSpec, NotSyncable, OpCost, OpKind, OpSpec, ParamSpec, ParamTy, RefineTier, Syncability,
    };
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, out: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: out.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(match kind {
            OpKind::Read => NotSyncable::NoAnchor,
            _ => NotSyncable::NotARead,
        }),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            PROFILE,
            OpKind::Read,
            vec![text("op", 256, false)],
            &["op", "roles", "groups", "allow", "deny", "changed"],
        ),
        spec(
            SET_OP_ACCESS,
            OpKind::Write,
            vec![
                text("op", 256, true),
                text("roles", 64, false),
                text("groups", 4096, false),
            ],
            &["op", "roles", "groups", "outcome", "door"],
        ),
        spec(
            RESET_OP_ACCESS,
            OpKind::Write,
            vec![text("op", 256, true)],
            &["op", "outcome", "door"],
        ),
        spec(
            SET_MEMBER_ACCESS,
            OpKind::Write,
            vec![
                text("member", 256, true),
                text("op", 256, true),
                text("effect", 16, true),
            ],
            &["member", "op", "effect", "door"],
        ),
        spec(
            CREATE_GROUP,
            OpKind::Write,
            vec![text("name", 480, true)],
            &["group", "name", "outcome", "door"],
        ),
        spec(
            ADD_TO_GROUP,
            OpKind::Write,
            vec![text("group", 256, true), text("member", 256, true)],
            &["group", "member", "outcome", "door"],
        ),
        spec(
            REMOVE_FROM_GROUP,
            OpKind::Write,
            vec![text("group", 256, true), text("member", 256, true)],
            &["group", "member", "outcome", "door"],
        ),
        spec(
            DELETE_GROUP,
            OpKind::Write,
            vec![text("group", 256, true)],
            &["group", "outcome", "links", "door"],
        ),
        spec(
            GROUPS,
            OpKind::Read,
            Vec::new(),
            &["group", "name", "members"],
        ),
    ]
}
