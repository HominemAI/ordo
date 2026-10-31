//! A tenant's own **access profile**: which roles and groups may call each
//! op, and the per-member overrides on top (`glossary.md` "profile / role /
//! override", `security-logging-design.md` §6, owner ruling 2026-10-10).
//!
//! # What is stored
//!
//! The profile a tenant starts with is the deployment's template: the app
//! developer's default per op. A tenant stores only what it changed:
//!
//! - `$opaccess`: one row per op the tenant set. The row is the whole line
//!   for that op (the roles and the groups that may call it) and replaces
//!   the template's line for it. An op with no row follows the template.
//! - `$memberaccess`: one row per (member, op) override, `allow` or `deny`.
//!
//! So a deploy never changes a line the tenant set, and an op the app
//! developer adds later reaches every tenant at its default.
//!
//! # Every change is a command
//!
//! Each change images the row before and after and stages one command
//! group with a real actor, exactly as the membership surface beside it
//! does ([`crate::membership`]). The group joins the tenant's own chain in
//! the transaction that wrote the row, so "who changed who may call what"
//! is answered by the same chain as "who changed this row".
//!
//! The two tables are platform tables, not entities, so a revert of one of
//! these groups is refused as `unknown_entity`. The way back is another
//! profile command, which is also on the record.
//!
//! # What is checked here, and what is not
//!
//! Here: attribution, a caller who is a member of this space, a member id
//! that names a person in this space, a group id that names a group. The
//! door decides who may call these commands at all, and the platform op
//! decides which op names a tenant may list; neither is this file's.

use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, Image, SpaceId, Timestamp, Value as CV,
};
use serde_json::{json, Value};

use crate::write::{q_ident, sql_str, SpaceAudit, WriteDb, WriteLog, NOT_FOUND};
use crate::Fixture;

/// The tenant's own op lines. Created where the tenant's chain is, before
/// the first write, and read by the door when it is built.
pub const OPACCESS_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$opaccess" (
  "$space" text   NOT NULL,
  "op"     text   NOT NULL,
  "roles"  text   NOT NULL DEFAULT '',
  "groups" text   NOT NULL DEFAULT '',
  "rev"    bigint NOT NULL DEFAULT 1,
  PRIMARY KEY ("$space", "op")
)"#;

/// The tenant's per-member overrides.
pub const MEMBERACCESS_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$memberaccess" (
  "$space" text NOT NULL,
  "member" text NOT NULL,
  "op"     text NOT NULL,
  "effect" text NOT NULL,
  PRIMARY KEY ("$space", "member", "op")
)"#;

/// The entity names the command groups carry for these two tables.
pub const OPACCESS: &str = "$opaccess";
pub const MEMBERACCESS: &str = "$memberaccess";

/// One profile command.
#[derive(Debug, Clone, Copy)]
pub enum ProfileCommand<'a> {
    /// Set the whole line for `op`: these roles and these groups may call
    /// it. Replaces the template's line for this tenant.
    SetOp {
        op: &'a str,
        roles: &'a [String],
        groups: &'a [String],
    },
    /// Drop the tenant's line for `op`, so it follows the template again.
    ResetOp { op: &'a str },
    /// Allow or deny `op` for one member, on top of role and groups.
    /// `effect: None` removes the override.
    SetMember {
        member: &'a str,
        op: &'a str,
        effect: Option<&'a str>,
    },
}

fn text(v: &str) -> CV {
    CV::Text(v.to_string())
}

pub(crate) fn op_image(op: &str, roles: &str, groups: &str, rev: i64) -> Image {
    Image::from([
        ("op".to_string(), text(op)),
        ("roles".to_string(), text(roles)),
        ("groups".to_string(), text(groups)),
        ("rev".to_string(), CV::Int(rev)),
    ])
}

pub(crate) fn member_image(member: &str, op: &str, effect: &str) -> Image {
    Image::from([
        ("member".to_string(), CV::Id(member.to_string())),
        ("op".to_string(), text(op)),
        ("effect".to_string(), text(effect)),
    ])
}

impl Fixture {
    /// Run one profile command against `db` and stage its group on `log`.
    /// The caller commits (the host runs this inside
    /// `ordo_exec::Executor::platform_write`, so the group is appended to
    /// the tenant's chain in the same transaction).
    pub fn profile_command_on(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        cmd: ProfileCommand<'_>,
        actor: Option<&str>,
    ) -> Result<Value, String> {
        let audited = SpaceAudit {
            inner: db,
            space: self.space().0.clone(),
        };
        let db: &dyn WriteDb = &audited;
        let Some(actor) = actor.filter(|a| !a.is_empty()) else {
            return Err(
                "access_guard: a profile command is an attributed command, and an anonymous \
                 caller has no attribution to carry"
                    .into(),
            );
        };
        if !self.caller_is_space_member(db, actor)? {
            return Err("access_guard: a profile is administered from inside the space".into());
        }
        match cmd {
            ProfileCommand::SetOp { op, roles, groups } => {
                for g in groups {
                    if self.kind_of(db, g)?.as_deref() != Some("group") {
                        return Err(format!(
                            "not_found: `{g}` is not a group in this space. Make one with \
                             std.space.createGroup, or list them with std.space.groups"
                        ));
                    }
                }
                self.set_op(db, log, actor, op, &roles.join(","), &groups.join(","))
            }
            ProfileCommand::ResetOp { op } => self.reset_op(db, log, actor, op),
            ProfileCommand::SetMember { member, op, effect } => {
                if self.kind_of(db, member)?.as_deref() != Some("principal") {
                    return Err(NOT_FOUND.to_string());
                }
                self.set_member_access(db, log, actor, member, op, effect)
            }
        }
    }

    fn kind_of(&self, db: &dyn WriteDb, id: &str) -> Result<Option<String>, String> {
        let rows = db.rows(&format!(
            "SELECT {k} FROM {} WHERE {sp} AND {} = {} AND {} NOT IN ('left', 'deleted') \
             AND ({k} = 'group' OR {} IS NOT NULL);",
            q_ident("members"),
            q_ident("id"),
            sql_str(id),
            q_ident("status"),
            q_ident("role"),
            k = q_ident("kind"),
            sp = self.space_pred(),
        ))?;
        Ok(rows.first().and_then(|r| r.first().cloned()).flatten())
    }

    fn op_line(&self, db: &dyn WriteDb, op: &str) -> Result<Option<(String, String, i64)>, String> {
        let rows = db.rows(&format!(
            "SELECT \"roles\", \"groups\", \"rev\" FROM \"$opaccess\" WHERE {sp} AND \"op\" = {} \
             FOR UPDATE;",
            sql_str(op),
            sp = self.space_pred(),
        ))?;
        Ok(rows.first().map(|r| {
            let c = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
            (c(0), c(1), c(2).parse().unwrap_or(1))
        }))
    }

    fn set_op(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        actor: &str,
        op: &str,
        roles: &str,
        groups: &str,
    ) -> Result<Value, String> {
        let before = self.op_line(db, op)?;
        let rev = before.as_ref().map(|b| b.2 + 1).unwrap_or(1);
        db.rows(&format!(
            "INSERT INTO \"$opaccess\" (\"$space\", \"op\", \"roles\", \"groups\", \"rev\") \
             VALUES ({}, {}, {}, {}, {rev}) ON CONFLICT (\"$space\", \"op\") DO UPDATE SET \
             \"roles\" = EXCLUDED.\"roles\", \"groups\" = EXCLUDED.\"groups\", \"rev\" = {rev};",
            self.space_lit(),
            sql_str(op),
            sql_str(roles),
            sql_str(groups),
        ))?;
        let after = op_image(op, roles, groups, rev);
        let (kind, before) = match before {
            Some((r, g, v)) => (CommandKind::Converge, Some(op_image(op, &r, &g, v))),
            None => (CommandKind::Create, None),
        };
        self.stage(log, actor, OPACCESS, op, kind, before, Some(after))?;
        Ok(json!([{ "entity": OPACCESS, "op": op }]))
    }

    fn reset_op(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        actor: &str,
        op: &str,
    ) -> Result<Value, String> {
        let Some((roles, groups, rev)) = self.op_line(db, op)? else {
            // Nothing to reset: the op already follows the template. A
            // no-op records nothing.
            return Ok(json!([]));
        };
        db.rows(&format!(
            "DELETE FROM \"$opaccess\" WHERE {sp} AND \"op\" = {};",
            sql_str(op),
            sp = self.space_pred(),
        ))?;
        self.stage(
            log,
            actor,
            OPACCESS,
            op,
            CommandKind::Delete,
            Some(op_image(op, &roles, &groups, rev)),
            None,
        )?;
        Ok(json!([{ "entity": OPACCESS, "op": op }]))
    }

    fn set_member_access(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        actor: &str,
        member: &str,
        op: &str,
        effect: Option<&str>,
    ) -> Result<Value, String> {
        let rows = db.rows(&format!(
            "SELECT \"effect\" FROM \"$memberaccess\" WHERE {sp} AND \"member\" = {} \
             AND \"op\" = {} FOR UPDATE;",
            sql_str(member),
            sql_str(op),
            sp = self.space_pred(),
        ))?;
        let was: Option<String> = rows.first().and_then(|r| r.first().cloned()).flatten();
        let row_id = format!("{member}:{op}");
        match (was, effect) {
            (None, None) => Ok(json!([])),
            (Some(w), Some(e)) if w == e => Ok(json!([])),
            (Some(w), None) => {
                db.rows(&format!(
                    "DELETE FROM \"$memberaccess\" WHERE {sp} AND \"member\" = {} AND \"op\" = {};",
                    sql_str(member),
                    sql_str(op),
                    sp = self.space_pred(),
                ))?;
                self.stage(
                    log,
                    actor,
                    MEMBERACCESS,
                    &row_id,
                    CommandKind::Delete,
                    Some(member_image(member, op, &w)),
                    None,
                )?;
                Ok(json!([{ "entity": MEMBERACCESS, "op": op }]))
            }
            (was, Some(e)) => {
                db.rows(&format!(
                    "INSERT INTO \"$memberaccess\" (\"$space\", \"member\", \"op\", \"effect\") \
                     VALUES ({}, {}, {}, {}) ON CONFLICT (\"$space\", \"member\", \"op\") \
                     DO UPDATE SET \"effect\" = EXCLUDED.\"effect\";",
                    self.space_lit(),
                    sql_str(member),
                    sql_str(op),
                    sql_str(e),
                ))?;
                let (kind, before) = match was {
                    Some(w) => (CommandKind::Converge, Some(member_image(member, op, &w))),
                    None => (CommandKind::Create, None),
                };
                self.stage(
                    log,
                    actor,
                    MEMBERACCESS,
                    &row_id,
                    kind,
                    before,
                    Some(member_image(member, op, e)),
                )?;
                Ok(json!([{ "entity": MEMBERACCESS, "op": op }]))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn stage(
        &self,
        log: &mut WriteLog,
        actor: &str,
        entity: &str,
        row_id: &str,
        kind: CommandKind,
        before: Option<Image>,
        after: Option<Image>,
    ) -> Result<(), String> {
        let command = Command {
            id: CommandId(format!("cmd_{}", log.mint())),
            space: SpaceId(self.space().0.clone()),
            entity: entity.to_string(),
            row_id: row_id.to_string(),
            kind,
            before,
            after,
            at: Timestamp(log.stamp()),
        };
        let correlation = log.group_correlation();
        log.stage_group(CommandGroup {
            correlation,
            actor: Actor::User(actor.to_string()),
            commands: vec![command],
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        })
    }
}
