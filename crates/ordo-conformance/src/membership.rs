//! The platform's **group-membership surface** — DL §3, Groups `[settled]`.
//!
//! Checklist 10i landed the half that reads: `groupMembers` is a real
//! platform entity and the acl-derived guards walk it. Nothing wrote it. A
//! group could be granted access and could not be given members, so the
//! second disjunct the derivation emits had an empty table under it and the
//! whole group feature was inert — correct, disclosed, and doing nothing.
//!
//! # Why this is a surface and not an op
//!
//! `members` and `groupMembers` are platform entities, which the write path
//! refuses to authored statements twice over (`ordo_plan::writes::
//! check_platform_entities` at registration, and again at this path's
//! dispatch). That refusal is not an obstacle to route around — it is the
//! rule DL states: *"there is no domain-authored path to minting one"*, and
//! `security-hardening-review-2.md` §B6 restates it for groups in particular
//! (*"group-membership edits are platform commands with their own door, not
//! domain-authored ops over a members-shaped entity"*). So membership is
//! written **here**, on the same footing `members` rows are: by the platform,
//! never by a program.
//!
//! # What it is not allowed to skip
//!
//! It is a *command*, not a mutation. DL: *"Group membership changes are
//! ordinary audited commands — the same runtime state as roles and profiles,
//! in the chain, never in IaC. Creating a group, adding a member, removing
//! one: three commands, three log entries."* Each call below appends exactly
//! one [`ordo_commandlog::CommandGroup`] with a real `Actor`, held back until
//! the effect commits, exactly as an op's group is.
//!
//! # The three refusals, and why each is the class it is
//!
//! - **`group_nesting`** — adding a `kind: group` row *as a member*. DL: *"Groups
//!   may not contain groups in v1 — nesting is rejected at write time
//!   (`group_nesting`)."* The class was documented with no producer for as long
//!   as groups have existed. It matters more than a v1 scope note: depth 1 is
//!   what makes **one** expansion sufficient everywhere, so a nested group would
//!   not fail loudly — it would quietly under-expand the guard walk, the
//!   `callersOf` enumeration, and `memberCount`, each in the safe direction and
//!   none of them audibly.
//! - **`not_found`** — an id that names no group, or names a `members` row that
//!   is not a group, or a member with no `members` row in this space. All one
//!   message ([`crate::write::NOT_FOUND`]), byte-identical, for R-G10's reason:
//!   telling a caller "that id exists but is a person" is an existence answer
//!   about a row they never saw. It also carries the rule
//!   `ordo_identity::AccessEngine::add_to_group` already enforces in memory —
//!   *"a group cannot smuggle in a principal who is not in the space"* — down to
//!   the rows, where the FK enforces the space and this check enforces the
//!   membership.
//! - **`access_guard`** — an anonymous or guest actor. A committed group carries
//!   an `Actor` and there is no anonymous one; a guest is defined by bootstrap
//!   rule 4 as a `members` row with a null `role`, which is exactly what
//!   [`crate::Fixture::caller_is_space_member`] answers.
//!
//! **Deliberately not decided here:** *which role* may administer groups, and
//! the no-escalation invariant `security-hardening-review-2.md` proposes (a
//! writer's own effective grant set must cover the authority they confer).
//! Both are `[proposed]`, not settled, and a surface is the wrong place to
//! settle them — the door is. What this enforces is the boundary that *is*
//! ruled: attributed, non-guest, in this space.
//!
//! # `memberCount`
//!
//! Maintained here, because here is the only place it can be: DL calls it
//! *"platform-maintained alongside the membership commands (same machinery as
//! a tree's `depth`)"*. Every add and every remove rewrites the group's count
//! in the same command group as the link row, so the two never disagree. The
//! count is `count(*)` over the group's link rows and that is exactly the
//! principal count **because** nesting is refused: with depth 1 every link row
//! is a person. It stays the *cheap upper bound* DL describes when summed
//! across acl rows — a principal reachable directly and through a group is
//! counted once per path, and that is a property of the summation, not of this
//! number.

use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, Image, SpaceId, Timestamp,
};
use serde_json::{json, Value};

use crate::write::{q_ident, sql_str, SpaceAudit, WriteDb, WriteLog, NOT_FOUND};
use crate::{Fixture, Pg};

/// The date a group row records as `joined`.
///
/// The harness's clock, not the wall clock: every other value this path
/// writes is deterministic, and a row whose `joined` moves between runs would
/// make the command group it is imaged into unreproducible.
const CLOCK: (i32, u8, u8) = ordo_sema::Clock::CORPUS.0;

/// One membership command. Three variants, because DL names three:
/// *"Creating a group, adding a member, removing one."*
#[derive(Debug, Clone, Copy)]
pub enum GroupCommand<'a> {
    /// Mint a `members` row with `kind: group`. Its `displayName` is *"the
    /// name given when the group was created"* (DL §3, `displayName`).
    Create { display_name: &'a str },
    /// Link a principal's `members` row to a group's.
    Add { group: &'a str, member: &'a str },
    /// Unlink one. DL: *"Revoking group access for one person is removing them
    /// from the group, which is a `groupMembers` command."*
    Remove { group: &'a str, member: &'a str },
    /// Delete a group: its link rows and its `members` row, as one command
    /// group. Refused with `group_in_use` while a profile line or an `acl`
    /// grant still names it.
    Delete { group: &'a str },
}

/// One way a principal holds a level on one acl-bearing row.
///
/// The `via_group` half is the whole point: two paths at different levels is
/// the Y-013 case, and a report that collapsed them to one number would be
/// exactly the silence the ruling exists to forbid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantPath {
    /// The acl link row that confers it.
    pub row: String,
    /// The level that row confers.
    pub level: String,
    /// `None` when the link row names this principal directly; the group's
    /// `members` id when it names a group they belong to.
    pub via_group: Option<String>,
}

/// One row a membership command changed, on its way to becoming a
/// [`Command`] in that command's group.
struct Touched {
    entity: &'static str,
    row_id: String,
    kind: CommandKind,
    before: Option<Image>,
    after: Option<Image>,
}

impl Fixture {
    /// Run a membership command against the psql harness and append its
    /// group.
    ///
    /// The same non-atomic caveat [`Fixture::run_write_as`] carries, for the
    /// same reason: each `psql` invocation is its own process, so the group is
    /// appended once the statements have already been applied. The
    /// transactional path is [`Fixture::group_command_on`] against an open
    /// transaction, where the append rides the commit.
    pub fn group_command_as(
        &self,
        pg: &Pg,
        log: &mut WriteLog,
        cmd: GroupCommand<'_>,
        actor: Option<&str>,
    ) -> Result<Value, String> {
        let out = self.group_command_on(pg, log, cmd, actor);
        match &out {
            Ok(_) => log.commit_pending()?,
            Err(_) => log.drop_pending(),
        }
        out
    }

    /// The membership protocol itself, against any [`WriteDb`].
    ///
    /// Every statement goes out through [`SpaceAudit`], which is not
    /// decoration: this surface writes the two entities every guard in the
    /// deployment reads, so an unscoped statement here would be a
    /// cross-tenant grant rather than a cross-tenant read.
    pub fn group_command_on(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        cmd: GroupCommand<'_>,
        actor: Option<&str>,
    ) -> Result<Value, String> {
        let audited = SpaceAudit {
            inner: db,
            space: self.space().0.clone(),
        };
        let db: &dyn WriteDb = &audited;

        // Attribution first, before any probe: a refusal that ran a query
        // first has already answered an existence question for a caller who
        // was never allowed to ask one.
        let Some(actor) = actor.filter(|a| !a.is_empty()) else {
            return Err(
                "access_guard: a membership command is an attributed command, and an \
                        anonymous caller has no attribution to carry"
                    .into(),
            );
        };
        if !self.caller_is_space_member(db, actor)? {
            return Err(
                "access_guard: membership commands are administered from inside the \
                        space — a guest (a members row with a null role) and a principal with \
                        no members row here are the same answer"
                    .into(),
            );
        }

        match cmd {
            GroupCommand::Create { display_name } => {
                self.create_group(db, log, actor, display_name)
            }
            GroupCommand::Add { group, member } => self.add_to_group(db, log, actor, group, member),
            GroupCommand::Remove { group, member } => {
                self.remove_from_group(db, log, actor, group, member)
            }
            GroupCommand::Delete { group } => self.delete_group(db, log, actor, group),
        }
    }

    /// **Every path by which `member` holds a level on one acl-bearing row**
    /// — the audit half of the Y-013 ruling.
    ///
    /// The ruling (`corpus-rulings.md`, 2026-09-05) is that the *more
    /// permissive* level wins **and the audit notes both paths**. The second
    /// clause is not decoration: once access can arrive two ways, "Dov may
    /// write" is an answer nobody can act on. Removing his direct row is a
    /// no-op if *Contractors* still holds `admin`, and an administrator who
    /// cannot see that will believe they revoked something. DL states the
    /// same obligation for revoke — *"'I revoked Dana' must not mean 'Dana is
    /// still in Contractors, which still holds write'"*.
    ///
    /// This is the computation `ordo_schema`'s acl derivation names as **not
    /// derived**, with its reason: R-G17's surviving-grant-path report *"needs
    /// the group expansion DL §3 describes … and nothing in the tree expands
    /// groups yet, so a report would be a list that is empty for the wrong
    /// reason"*. Groups expand now, and are writable, so the list can be
    /// right. Deriving the report as an **op** on every acl root is a separate
    /// change to the derivation and its goldens; this is the query under it,
    /// executable and asserted today.
    ///
    /// Rows come back deterministically ordered, direct paths first.
    pub fn grant_paths(
        &self,
        db: &dyn WriteDb,
        root: &str,
        row: &str,
        member: &str,
    ) -> Result<Vec<GrantPath>, String> {
        let e = self
            .schema
            .entity(root)
            .ok_or_else(|| format!("unknown_source: no entity `{root}`"))?;
        if e.acl_levels.is_none() {
            return Err(format!(
                "unknown_source: `{root}` carries no `acl`, so there are no grant paths to report"
            ));
        }
        // The derived shape, spelled the way the derivation spells it: the
        // link is `<Root>Access` and its ref back to the root is the root's
        // name with a trailing `s` dropped.
        let link = format!("{root}Access");
        let back = root.strip_suffix('s').unwrap_or(root);
        let rows = db.rows(&format!(
            "SELECT a.{id}, a.{lvl}, NULL FROM {link} a \
               WHERE a.{sp_col} = {sp} AND a.{back} = {row} AND a.{mem} = {who} \
             UNION ALL \
             SELECT a.{id}, a.{lvl}, gm.{grp} FROM {link} a \
               JOIN {gm} gm ON gm.{sp_col} = a.{sp_col} AND gm.{grp} = a.{mem} \
               WHERE a.{sp_col} = {sp} AND a.{back} = {row} AND gm.{mem} = {who} \
             ORDER BY 3 NULLS FIRST, 1;",
            id = q_ident("id"),
            lvl = q_ident("level"),
            link = q_ident(&link),
            gm = q_ident("groupMembers"),
            grp = q_ident("group"),
            mem = q_ident("member"),
            back = q_ident(back),
            sp_col = q_ident(ordo_schema::SPACE_COLUMN),
            sp = self.space_lit(),
            row = sql_str(row),
            who = sql_str(member),
        ))?;
        Ok(rows
            .into_iter()
            .map(|r| GrantPath {
                row: r.first().cloned().flatten().unwrap_or_default(),
                level: r.get(1).cloned().flatten().unwrap_or_default(),
                via_group: r.get(2).cloned().flatten(),
            })
            .collect())
    }

    /// The level actually in force across a report: **the maximum**, by
    /// position in the root's declared ladder.
    ///
    /// Not "the direct one", not "the most specific" — there is no such
    /// notion, and a rule that let a group *lower* a direct grant would make
    /// adding somebody to a group a silent revocation. A level outside the
    /// ladder has no position in it and is ignored rather than guessed at.
    pub fn effective_level(&self, root: &str, paths: &[GrantPath]) -> Option<String> {
        let levels = self.schema.entity(root)?.acl_levels.clone()?;
        paths
            .iter()
            .filter_map(|p| levels.iter().position(|l| *l == p.level).map(|i| (i, p)))
            .max_by_key(|(i, _)| *i)
            .map(|(_, p)| p.level.clone())
    }

    /// `kind` of a `members` row in this space, or `None` if there is none.
    fn member_kind(&self, db: &dyn WriteDb, id: &str) -> Result<Option<String>, String> {
        let rows = db.rows(&format!(
            "SELECT {} FROM {} WHERE {sp} AND {} = {};",
            q_ident("kind"),
            q_ident("members"),
            q_ident("id"),
            sql_str(id),
            sp = self.space_pred(),
        ))?;
        Ok(rows.first().and_then(|r| r.first().cloned()).flatten())
    }

    /// The link row joining `group` to `member`, if there is one. The unique
    /// key `(group, member)` is what makes "if there is one" well-formed:
    /// without it a removal would have to delete an unknown number of rows
    /// and could leave one behind.
    fn link_row(
        &self,
        db: &dyn WriteDb,
        group: &str,
        member: &str,
    ) -> Result<Option<String>, String> {
        let rows = db.rows(&format!(
            "SELECT {} FROM {} WHERE {sp} AND {} = {} AND {} = {};",
            q_ident("id"),
            q_ident("groupMembers"),
            q_ident("group"),
            sql_str(group),
            q_ident("member"),
            sql_str(member),
            sp = self.space_pred(),
        ))?;
        Ok(rows.first().and_then(|r| r.first().cloned()).flatten())
    }

    /// Ids minted by the platform's own surface, distinguishable from the
    /// write path's `<key>_w<n>` at a glance. A reader looking at a chain
    /// should be able to tell which door a row came through.
    fn platform_id(&self, key: &str, log: &mut WriteLog) -> String {
        format!("{key}_p{}", log.mint())
    }

    fn create_group(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        actor: &str,
        display_name: &str,
    ) -> Result<Value, String> {
        let key = self
            .schema
            .entity("members")
            .expect("members is platform-provided")
            .key
            .clone();
        let id = self.platform_id(&key, log);
        // `principal` is the deployment-wide id, and a group has none — it is
        // a collection, not a subject that authenticates. It gets one derived
        // from its own row id so the column stays non-null and unambiguous,
        // and so nothing can mistake it for a principal that could hold a
        // token: no `usr_`/`agt_`/`svc_` prefix is ever minted here.
        let principal = format!("grp_{id}");
        db.rows(&format!(
            "INSERT INTO {} ({}, {}, {}, {}, {}, {}, {}, {}, {}) \
             VALUES ({}, {}, {}, {}, 'group', 0, NULL, 'active', DATE '{:04}-{:02}-{:02}');",
            q_ident("members"),
            q_ident(ordo_schema::SPACE_COLUMN),
            q_ident("id"),
            q_ident("principal"),
            q_ident("displayName"),
            q_ident("kind"),
            q_ident("memberCount"),
            q_ident("role"),
            q_ident("status"),
            q_ident("joined"),
            self.space_lit(),
            sql_str(&id),
            sql_str(&principal),
            sql_str(display_name),
            CLOCK.0,
            CLOCK.1,
            CLOCK.2,
        ))?;
        let after = self
            .row_image(db, log, "members", &id)?
            .ok_or_else(|| "internal: the group row vanished after its insert".to_string())?;
        self.append(
            log,
            actor,
            vec![Touched {
                entity: "members",
                row_id: id.clone(),
                kind: CommandKind::Create,
                before: None,
                after: Some(after),
            }],
        )?;
        Ok(json!({ "created": id }))
    }

    fn add_to_group(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        actor: &str,
        group: &str,
        member: &str,
    ) -> Result<Value, String> {
        // A group is a `members` row with `kind: group`; anything else — a
        // person's row, an id that names nothing — is not a group, and both
        // answer alike.
        if self.member_kind(db, group)?.as_deref() != Some("group") {
            return Err(NOT_FOUND.to_string());
        }
        let Some(member_kind) = self.member_kind(db, member)? else {
            return Err(NOT_FOUND.to_string());
        };
        if member_kind == "group" {
            // The one refusal DL names by class and nothing produced. Not a
            // scope note: depth 1 is the premise the single expansion rests
            // on — the acl-derived guards emit exactly one
            // `member.groupMembers.group(member == $caller)` hop, and
            // `callersOf` enumerates one level — so a nested group would not
            // break anything loudly. It would silently stop conferring what
            // the acl row says it confers.
            return Err(format!(
                "group_nesting: `{member}` is a group, and groups may not contain groups — \
                 depth 1 is what makes a single membership expansion sufficient in every \
                 guard walk and headcount in the space"
            ));
        }
        if self.link_row(db, group, member)?.is_some() {
            return Err(format!(
                "duplicate: `{member}` is already in `{group}` — one row per (group, member), \
                 so that a removal removes all of it"
            ));
        }

        let before = self
            .row_image(db, log, "members", group)?
            .ok_or_else(|| "internal: the group row vanished mid-command".to_string())?;
        let key = self
            .schema
            .entity("groupMembers")
            .expect("groupMembers is platform-provided")
            .key
            .clone();
        let link = self.platform_id(&key, log);
        db.rows(&format!(
            "INSERT INTO {} ({}, {}, {}, {}) VALUES ({}, {}, {}, {});",
            q_ident("groupMembers"),
            q_ident(ordo_schema::SPACE_COLUMN),
            q_ident("id"),
            q_ident("group"),
            q_ident("member"),
            self.space_lit(),
            sql_str(&link),
            sql_str(group),
            sql_str(member),
        ))?;
        self.recount(db, group)?;

        let link_after = self
            .row_image(db, log, "groupMembers", &link)?
            .ok_or_else(|| "internal: the link row vanished after its insert".to_string())?;
        let after = self
            .row_image(db, log, "members", group)?
            .ok_or_else(|| "internal: the group row vanished mid-command".to_string())?;
        self.append(
            log,
            actor,
            vec![
                Touched {
                    entity: "groupMembers",
                    row_id: link.clone(),
                    kind: CommandKind::Create,
                    before: None,
                    after: Some(link_after),
                },
                // The count rides the same group as the link row. Two rows
                // changed, so two commands: a revert that put the link back
                // and left `memberCount` behind would restore a state the
                // platform never had.
                Touched {
                    entity: "members",
                    row_id: group.to_string(),
                    kind: CommandKind::Converge,
                    before: Some(before),
                    after: Some(after),
                },
            ],
        )?;
        Ok(json!({ "added": link }))
    }

    /// **Delete a group** as one command group: every link row and the
    /// group's `members` row.
    ///
    /// Refused with `group_in_use` while a profile line names the group:
    /// a delete that silently revoked access would be the quietest possible
    /// authorization change.
    ///
    /// A row elsewhere that still names the group (an `acl` grant) is a
    /// foreign key into `members`, so the delete is refused as `group_in_use`
    /// and the whole command rolls back.
    fn delete_group(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        actor: &str,
        group: &str,
    ) -> Result<Value, String> {
        if self.member_kind(db, group)?.as_deref() != Some("group") {
            return Err(NOT_FOUND.to_string());
        }
        let mut touched: Vec<Touched> = Vec::new();
        let links = db.rows(&format!(
            "SELECT {} FROM {} WHERE {sp} AND {} = {} ORDER BY {} FOR UPDATE;",
            q_ident("id"),
            q_ident("groupMembers"),
            q_ident("group"),
            sql_str(group),
            q_ident("id"),
            sp = self.space_pred(),
        ))?;
        let links: Vec<String> = links
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect();
        for link in &links {
            let before = self
                .row_image(db, log, "groupMembers", link)?
                .ok_or_else(|| "internal: the link row vanished mid-command".to_string())?;
            db.rows(&format!(
                "DELETE FROM {} WHERE {sp} AND {} = {};",
                q_ident("groupMembers"),
                q_ident("id"),
                sql_str(link),
                sp = self.space_pred(),
            ))?;
            touched.push(Touched {
                entity: "groupMembers",
                row_id: link.clone(),
                kind: CommandKind::Delete,
                before: Some(before),
                after: None,
            });
        }

        // A group that still grants through the tenant's profile is not
        // deleted: removing it would silently revoke access. The profile
        // tables are created on first use; make sure they are here to read.
        db.rows(crate::access_profile::OPACCESS_DDL)?;
        db.rows(crate::access_profile::MEMBERACCESS_DDL)?;
        let lines = db.rows(&format!(
            "SELECT \"op\", \"groups\" FROM \"$opaccess\" WHERE {sp} AND \"groups\" <> '' \
             ORDER BY \"op\";",
            sp = self.space_pred(),
        ))?;
        let named: Vec<String> = lines
            .iter()
            .filter(|r| {
                r.get(1)
                    .cloned()
                    .flatten()
                    .unwrap_or_default()
                    .split(',')
                    .any(|g| g.trim() == group)
            })
            .filter_map(|r| r.first().cloned().flatten())
            .collect();
        let overridden = db.rows(&format!(
            "SELECT \"op\" FROM \"$memberaccess\" WHERE {sp} AND \"member\" = {} \
             ORDER BY \"op\";",
            sql_str(group),
            sp = self.space_pred(),
        ))?;
        if !named.is_empty() || !overridden.is_empty() {
            let ops: Vec<String> = named
                .into_iter()
                .chain(
                    overridden
                        .iter()
                        .filter_map(|r| r.first().cloned().flatten()),
                )
                .collect();
            return Err(format!(
                "group_in_use: `{group}` still grants access through this tenant's profile, on {}. \
                 Take it out of those lines first (std.space.setOpAccess, or \
                 std.space.resetOpAccess to go back to the default), then delete the group",
                ops.join(", ")
            ));
        }

        let before = self
            .row_image(db, log, "members", group)?
            .ok_or_else(|| "internal: the group row vanished mid-command".to_string())?;
        db.rows(&format!(
            "DELETE FROM {} WHERE {sp} AND {} = {};",
            q_ident("members"),
            q_ident("id"),
            sql_str(group),
            sp = self.space_pred(),
        ))
        .map_err(|e| {
            if e.contains("foreign key") || e.contains("violates") {
                format!(
                    "group_in_use: `{group}` is still named by a row in this space, such as \
                     an access grant on a record. Remove those grants first, then delete the group"
                )
            } else {
                e
            }
        })?;
        touched.push(Touched {
            entity: "members",
            row_id: group.to_string(),
            kind: CommandKind::Delete,
            before: Some(before),
            after: None,
        });
        self.append(log, actor, touched)?;
        Ok(json!({ "deleted": group, "links": links.len() }))
    }

    pub(crate) fn remove_from_group(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        actor: &str,
        group: &str,
        member: &str,
    ) -> Result<Value, String> {
        let Some(link) = self.link_row(db, group, member)? else {
            return Err(NOT_FOUND.to_string());
        };
        let link_before = self
            .row_image(db, log, "groupMembers", &link)?
            .ok_or_else(|| "internal: the link row vanished mid-command".to_string())?;
        let before = self
            .row_image(db, log, "members", group)?
            .ok_or_else(|| "internal: the group row vanished mid-command".to_string())?;
        db.rows(&format!(
            "DELETE FROM {} WHERE {sp} AND {} = {};",
            q_ident("groupMembers"),
            q_ident("id"),
            sql_str(&link),
            sp = self.space_pred(),
        ))?;
        self.recount(db, group)?;
        let after = self
            .row_image(db, log, "members", group)?
            .ok_or_else(|| "internal: the group row vanished mid-command".to_string())?;
        self.append(
            log,
            actor,
            vec![
                Touched {
                    entity: "groupMembers",
                    row_id: link,
                    kind: CommandKind::Delete,
                    before: Some(link_before),
                    after: None,
                },
                Touched {
                    entity: "members",
                    row_id: group.to_string(),
                    kind: CommandKind::Converge,
                    before: Some(before),
                    after: Some(after),
                },
            ],
        )?;
        Ok(json!({ "removed": 1 }))
    }

    /// Rewrite one group's `memberCount` from its link rows, and bump `rev`
    /// the way every other converge on this path does.
    ///
    /// `count(*)` over the link rows **is** the principal count, because
    /// `group_nesting` refuses the only thing that could make it otherwise.
    /// The two statements are one statement on purpose: a read-then-write
    /// would be a lost update under concurrency, and this path's whole reason
    /// for existing is that these rows are the door's input.
    fn recount(&self, db: &dyn WriteDb, group: &str) -> Result<(), String> {
        db.rows(&format!(
            "UPDATE {m} SET {mc} = (SELECT count(*) FROM {gm} WHERE {sp} AND {g} = {gid}), \
             {rev} = {rev} + 1 WHERE {sp} AND {id} = {gid};",
            m = q_ident("members"),
            mc = q_ident("memberCount"),
            gm = q_ident("groupMembers"),
            g = q_ident("group"),
            id = q_ident("id"),
            rev = q_ident("rev"),
            gid = sql_str(group),
            sp = self.space_pred(),
        ))
        .map(|_| ())
    }

    /// **A member's seat, released** (checklist 27aq R2): the row stays,
    /// `status = left`, no role, and the principal unlinked as
    /// `left:<principal>:<seat>`. The name is kept unless the app developer
    /// set a replacement (`left_name`; owner ruling 2026-10-06: a leaver
    /// keeps their name, and the app decides how a left seat reads). Rows
    /// that ref the seat keep a valid target.
    ///
    /// **Why the principal is unlinked.** Every lookup that asks "does this
    /// person hold a seat here" matches on `principal`: the door's engine,
    /// a new invitation's `NOT EXISTS`, the account deletion's list of
    /// tenants. A left seat that still held the principal would read as a
    /// membership to all of them and would stop the person from ever being
    /// admitted again. The id is kept inside the new value, behind the
    /// `left:` prefix no principal has, so the person's left seats can still
    /// be found and marked when they delete their account.
    ///
    /// One converge, imaged before and after under a row lock, staged like
    /// every command on this surface, so a revert of the group puts the seat
    /// back as it was. The caller has already decided the leave is allowed;
    /// this is the write, not the decision. `false` when there is no such
    /// principal seat.
    pub(crate) fn release_seat(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        seat: &str,
        left_name: Option<&str>,
    ) -> Result<bool, String> {
        db.rows(&format!(
            "SELECT {} FROM {} WHERE {sp} AND {} = {} AND {} = 'principal' FOR UPDATE",
            q_ident("id"),
            q_ident("members"),
            q_ident("id"),
            sql_str(seat),
            q_ident("kind"),
            sp = self.space_pred(),
        ))?;
        let Some(before) = self.row_image(db, log, "members", seat)? else {
            return Ok(false);
        };
        let rename = match left_name {
            Some(name) => format!(", {} = {}", q_ident("displayName"), sql_str(name)),
            None => String::new(),
        };
        db.rows(&format!(
            "UPDATE {m} SET {st} = 'left', {role} = NULL, \
             {p} = 'left:' || coalesce({p}, '') || ':' || {id}{rename}, \
             {rev} = {rev} + 1 WHERE {sp} AND {id} = {sid} AND {kind} = 'principal';",
            m = q_ident("members"),
            st = q_ident("status"),
            role = q_ident("role"),
            p = q_ident("principal"),
            rev = q_ident("rev"),
            id = q_ident("id"),
            sid = sql_str(seat),
            kind = q_ident("kind"),
            sp = self.space_pred(),
        ))?;
        let after = self
            .row_image(db, log, "members", seat)?
            .ok_or_else(|| "internal: the seat vanished mid-command".to_string())?;
        self.append(
            log,
            seat,
            vec![Touched {
                entity: "members",
                row_id: seat.to_string(),
                kind: CommandKind::Converge,
                before: Some(before),
                after: Some(after),
            }],
        )?;
        Ok(true)
    }

    /// **A left seat whose person deleted their account** (owner ruling
    /// 2026-10-06): `status` moves from `left` to `deleted`, and nothing
    /// else changes. The name stays; the app reads the status and decides
    /// how the seat is shown. One converge, imaged and staged as every
    /// command on this surface is, attributed to the seat.
    ///
    /// `false`, with nothing written, when the seat is not a `left`
    /// principal seat: one already marked, or none.
    pub(crate) fn mark_seat_deleted(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        seat: &str,
    ) -> Result<bool, String> {
        let locked = db.rows(&format!(
            "SELECT {} FROM {} WHERE {sp} AND {} = {} AND {} = 'principal' AND {} = 'left' \
             FOR UPDATE",
            q_ident("id"),
            q_ident("members"),
            q_ident("id"),
            sql_str(seat),
            q_ident("kind"),
            q_ident("status"),
            sp = self.space_pred(),
        ))?;
        if locked.is_empty() {
            return Ok(false);
        }
        let Some(before) = self.row_image(db, log, "members", seat)? else {
            return Ok(false);
        };
        db.rows(&format!(
            "UPDATE {m} SET {st} = 'deleted', {rev} = {rev} + 1 \
             WHERE {sp} AND {id} = {sid} AND {kind} = 'principal' AND {st} = 'left';",
            m = q_ident("members"),
            st = q_ident("status"),
            rev = q_ident("rev"),
            id = q_ident("id"),
            sid = sql_str(seat),
            kind = q_ident("kind"),
            sp = self.space_pred(),
        ))?;
        let after = self
            .row_image(db, log, "members", seat)?
            .ok_or_else(|| "internal: the seat vanished mid-command".to_string())?;
        self.append(
            log,
            seat,
            vec![Touched {
                entity: "members",
                row_id: seat.to_string(),
                kind: CommandKind::Converge,
                before: Some(before),
                after: Some(after),
            }],
        )?;
        Ok(true)
    }

    /// One command group per membership command — *"three commands, three log
    /// entries"* — staged rather than appended, so a transaction that never
    /// commits leaves no entry claiming it did.
    fn append(&self, log: &mut WriteLog, actor: &str, touched: Vec<Touched>) -> Result<(), String> {
        let commands = touched
            .into_iter()
            .map(|t| Command {
                id: CommandId(format!("cmd_{}", log.mint())),
                space: SpaceId(self.space().0.clone()),
                entity: t.entity.to_string(),
                row_id: t.row_id,
                kind: t.kind,
                before: t.before,
                after: t.after,
                at: Timestamp(log.stamp()),
            })
            .collect();
        // Minted last, after every command id and timestamp — the same
        // ordering the op path keeps, so a group's correlation always sorts
        // after the commands it names. Inside a batch (a leave, which runs
        // several of these and the release effects as one group) it is the
        // batch's, and the group below extends the one already staged.
        let correlation = log.group_correlation();
        log.stage_group(CommandGroup {
            correlation,
            actor: Actor::User(actor.to_string()),
            commands,
            // A platform command with its own door, not a rule-6 one. The
            // flag names one capability — the owner administering acl rows
            // without holding an access row — and a membership edit is not
            // it, whoever makes it. An owner editing membership is flagged
            // no differently from anyone else, because rule 6 is not what
            // let them.
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        })
    }
}
